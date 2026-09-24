use std::collections::{BTreeMap, HashMap, HashSet};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use crate::collab::derived_body::to_chosung;
use crate::db::context::set_tenant;
use crate::db::documents::{
    assert_document_writable, document_permission, lock_membership_users, recheck_session,
    session_is_live, workspace_is_live, DocumentDbError, DocumentPermission,
};
use crate::db::identity::{append_audit, append_event, AuditAppend, EventAppend};
use crate::db::projects::{lock_project, project_permission};
use crate::projects::ProjectPermission;

pub const COMMENT_BODY_MAX: usize = 8000;
const MENTION_MAX: usize = 50;
const REACTION_TRIES: usize = 8;
const VALID_REACTIONS: &[&str] = &["👍", "❤️", "🎉"];

macro_rules! commit_comment {
    ($expr:expr) => {
        match $expr {
            Ok(value) => value,
            Err(error) => return Ok(Err(error)),
        }
    };
}

macro_rules! comment_result {
    ($expr:expr) => {
        match $expr {
            Ok(value) => value,
            Err(sqlx::Error::RowNotFound) => return Ok(Err(CommentDbError::NotFound)),
            Err(error) => return Err(error),
        }
    };
}

#[derive(Debug)]
pub enum CommentDbError {
    NotFound,
    InvalidInput,
    InvalidCursor,
    Conflict,
}

#[derive(Debug, Clone)]
pub struct CommentRow {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub document_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub parent_id: Option<Uuid>,
    pub created_by: Uuid,
    pub body: String,
    pub resolved_at: Option<DateTime<Utc>>,
    pub reactions: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub struct CreateCommentInput<'a> {
    pub body: &'a str,
    pub parent_id: Option<Uuid>,
    pub mentioned_user_ids: &'a [Uuid],
}

pub struct PatchCommentInput<'a> {
    pub body: Option<&'a str>,
}

pub struct ReactionInput<'a> {
    pub emoji: &'a str,
    pub on: bool,
}

pub struct CommentListQuery {
    pub limit: i32,
    pub cursor: Option<String>,
}

pub struct CommentListPage {
    pub items: Vec<CommentRow>,
    pub next_cursor: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct CommentCursorPayload {
    id: Uuid,
    f: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReactionSummary {
    pub count: usize,
    pub reacted_by_me: bool,
}

fn normalize_body(body: &str) -> Result<String, CommentDbError> {
    let trimmed = body.trim();
    if trimmed.is_empty() || trimmed.len() > COMMENT_BODY_MAX {
        return Err(CommentDbError::InvalidInput);
    }
    Ok(trimmed.to_string())
}

fn normalize_mentions(ids: &[Uuid]) -> Result<Vec<Uuid>, CommentDbError> {
    let unique: Vec<Uuid> = ids
        .iter()
        .copied()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    if unique.len() > MENTION_MAX {
        return Err(CommentDbError::InvalidInput);
    }
    Ok(unique)
}

fn comment_scope(workspace_id: Uuid, kind: &str, target_id: Uuid) -> String {
    format!("{workspace_id}:{kind}:{target_id}")
}

fn encode_cursor(id: Uuid, scope: &str) -> String {
    let payload = CommentCursorPayload {
        id,
        f: scope.to_string(),
    };
    URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).expect("cursor json"))
}

fn decode_cursor(raw: &str, scope: &str) -> Result<Uuid, CommentDbError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(raw)
        .map_err(|_| CommentDbError::InvalidCursor)?;
    let payload: CommentCursorPayload =
        serde_json::from_slice(&bytes).map_err(|_| CommentDbError::InvalidCursor)?;
    if payload.f != scope {
        return Err(CommentDbError::InvalidCursor);
    }
    Ok(payload.id)
}

fn row_to_comment(row: &sqlx::postgres::PgRow) -> CommentRow {
    CommentRow {
        id: row.get("id"),
        workspace_id: row.get("workspace_id"),
        document_id: row.get("document_id"),
        task_id: row.get("task_id"),
        parent_id: row.get("parent_id"),
        created_by: row.get("created_by"),
        body: row.get("body"),
        resolved_at: row.get("resolved_at"),
        reactions: row.get("reactions"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
}

async fn fetch_comment(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    comment_id: Uuid,
) -> Result<Option<CommentRow>, sqlx::Error> {
    let row = sqlx::query(
        r#"
        SELECT id, workspace_id, document_id, task_id, parent_id, created_by, body,
               resolved_at, reactions, created_at, updated_at
        FROM fvoci.comments
        WHERE workspace_id = $1 AND id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(comment_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.map(|row| row_to_comment(&row)))
}

struct DocumentTarget {
    document_id: Uuid,
}

struct TaskTarget {
    project_id: Uuid,
    archived_at: Option<DateTime<Utc>>,
}

enum ParentTarget {
    Document(DocumentTarget),
    Task(TaskTarget),
}

async fn document_target(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    document_id: Uuid,
) -> Result<DocumentTarget, sqlx::Error> {
    let row: Option<(Option<DateTime<Utc>>,)> = sqlx::query_as(
        "SELECT deleted_at FROM fvoci.documents WHERE workspace_id = $1 AND id = $2",
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_optional(&mut **tx)
    .await?;
    match row {
        Some((None,)) => Ok(DocumentTarget { document_id }),
        _ => Err(sqlx::Error::RowNotFound),
    }
}

async fn task_target(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    task_id: Uuid,
) -> Result<TaskTarget, sqlx::Error> {
    type TaskTargetRow = (Uuid, Option<DateTime<Utc>>, Option<DateTime<Utc>>);
    let row: Option<TaskTargetRow> = sqlx::query_as(
        "SELECT project_id, deleted_at, archived_at FROM fvoci.tasks WHERE workspace_id = $1 AND id = $2",
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_optional(&mut **tx)
    .await?;
    match row {
        Some((project_id, None, archived_at)) => Ok(TaskTarget {
            project_id,
            archived_at,
        }),
        _ => Err(sqlx::Error::RowNotFound),
    }
}

async fn target_of_comment(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    comment: &CommentRow,
) -> Result<ParentTarget, CommentDbError> {
    if let Some(document_id) = comment.document_id {
        return document_target(tx, workspace_id, document_id)
            .await
            .map(ParentTarget::Document)
            .map_err(|_| CommentDbError::NotFound);
    }
    if let Some(task_id) = comment.task_id {
        return task_target(tx, workspace_id, task_id)
            .await
            .map(ParentTarget::Task)
            .map_err(|_| CommentDbError::NotFound);
    }
    Err(CommentDbError::NotFound)
}

fn map_document_error(err: DocumentDbError) -> CommentDbError {
    match err {
        DocumentDbError::Forbidden => CommentDbError::NotFound,
        DocumentDbError::NotFound
        | DocumentDbError::AffiliationMismatch
        | DocumentDbError::DepthLimit
        | DocumentDbError::InvalidSortKey => CommentDbError::NotFound,
    }
}

async fn require_document_access(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    document_id: Uuid,
    min: DocumentPermission,
    writable: bool,
) -> Result<(), CommentDbError> {
    let permission = document_permission(tx, workspace_id, actor_user_id, document_id)
        .await
        .map_err(|_| CommentDbError::NotFound)?;
    let permission = permission.map_err(map_document_error)?;
    if !permission.at_least(min) {
        return Err(CommentDbError::NotFound);
    }
    if writable {
        let writable = assert_document_writable(tx, workspace_id, document_id)
            .await
            .map_err(|_| CommentDbError::NotFound)?;
        writable.map_err(map_document_error)?;
    }
    Ok(())
}

async fn require_task_access(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    task: &TaskTarget,
    min: ProjectPermission,
    writable: bool,
) -> Result<(), CommentDbError> {
    let locked = lock_project(tx, workspace_id, task.project_id)
        .await
        .map_err(|_| CommentDbError::NotFound)?
        .ok_or(CommentDbError::NotFound)?;
    if locked.status == "archived" {
        return Err(CommentDbError::NotFound);
    }
    let permission = project_permission(tx, workspace_id, actor_user_id, &locked)
        .await
        .map_err(|_| CommentDbError::NotFound)?;
    if !permission.at_least(min) {
        return Err(CommentDbError::NotFound);
    }
    if writable && task.archived_at.is_some() {
        return Err(CommentDbError::NotFound);
    }
    Ok(())
}

async fn require_parent_access(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    target: &ParentTarget,
    min_doc: DocumentPermission,
    min_task: ProjectPermission,
    writable: bool,
) -> Result<(), CommentDbError> {
    match target {
        ParentTarget::Document(doc) => {
            require_document_access(
                tx,
                workspace_id,
                actor_user_id,
                doc.document_id,
                min_doc,
                writable,
            )
            .await
        }
        ParentTarget::Task(task) => {
            require_task_access(tx, workspace_id, actor_user_id, task, min_task, writable).await
        }
    }
}

async fn require_author_or_level(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    comment: &CommentRow,
    min_doc: DocumentPermission,
    min_task: ProjectPermission,
    writable: bool,
) -> Result<(), CommentDbError> {
    let target = target_of_comment(tx, workspace_id, comment).await?;
    require_parent_access(
        tx,
        workspace_id,
        actor_user_id,
        &target,
        DocumentPermission::View,
        ProjectPermission::View,
        writable,
    )
    .await?;
    if comment.created_by == actor_user_id {
        return Ok(());
    }
    require_parent_access(
        tx,
        workspace_id,
        actor_user_id,
        &target,
        min_doc,
        min_task,
        writable,
    )
    .await
}

async fn record_comment_event(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    verb: &str,
    comment_id: Uuid,
    payload: Value,
    client_ip: Option<&str>,
) -> Result<(), sqlx::Error> {
    append_event(
        tx,
        EventAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(workspace_id),
            actor_user_id: Some(actor_user_id),
            verb: verb.to_string(),
            target_type: Some("comment".to_string()),
            target_id: Some(comment_id),
            payload: payload.clone(),
        },
    )
    .await?;
    append_audit(
        tx,
        AuditAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(workspace_id),
            actor_user_id: Some(actor_user_id),
            verb: verb.to_string(),
            target_type: Some("comment".to_string()),
            target_id: Some(comment_id),
            payload,
            ip: client_ip.map(str::to_string),
        },
    )
    .await?;
    Ok(())
}

async fn record_comment_parent_event(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    comment: &CommentRow,
    verb: &str,
    client_ip: Option<&str>,
) -> Result<(), sqlx::Error> {
    let (target_type, target_id) = if let Some(task_id) = comment.task_id {
        ("task", task_id)
    } else {
        ("document", comment.document_id.expect("comment xor target"))
    };
    let payload = json!({
        "commentId": comment.id.to_string(),
        "documentId": comment.document_id.map(|id| id.to_string()),
        "taskId": comment.task_id.map(|id| id.to_string()),
    });
    append_event(
        tx,
        EventAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(workspace_id),
            actor_user_id: Some(actor_user_id),
            verb: verb.to_string(),
            target_type: Some(target_type.to_string()),
            target_id: Some(target_id),
            payload: payload.clone(),
        },
    )
    .await?;
    append_audit(
        tx,
        AuditAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(workspace_id),
            actor_user_id: Some(actor_user_id),
            verb: verb.to_string(),
            target_type: Some(target_type.to_string()),
            target_id: Some(target_id),
            payload,
            ip: client_ip.map(str::to_string),
        },
    )
    .await?;
    Ok(())
}

async fn list_rows(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    kind: &str,
    target_id: Uuid,
    limit: i32,
    after: Option<Uuid>,
) -> Result<Vec<CommentRow>, sqlx::Error> {
    let column = if kind == "document" {
        "document_id"
    } else {
        "task_id"
    };
    let sql = if after.is_some() {
        format!(
            r#"
            SELECT c.id, c.workspace_id, c.document_id, c.task_id, c.parent_id, c.created_by,
                   c.body, c.resolved_at, c.reactions, c.created_at, c.updated_at
            FROM fvoci.comments c
            WHERE c.workspace_id = $1 AND c.{column} = $2
              AND (c.created_at, c.id) > (
                SELECT created_at, id FROM fvoci.comments
                WHERE workspace_id = $1 AND id = $3
              )
            ORDER BY c.created_at ASC, c.id ASC
            LIMIT $4
            "#
        )
    } else {
        format!(
            r#"
            SELECT id, workspace_id, document_id, task_id, parent_id, created_by, body,
                   resolved_at, reactions, created_at, updated_at
            FROM fvoci.comments
            WHERE workspace_id = $1 AND {column} = $2
            ORDER BY created_at ASC, id ASC
            LIMIT $3
            "#
        )
    };
    let rows = if let Some(after) = after {
        sqlx::query(&sql)
            .bind(workspace_id)
            .bind(target_id)
            .bind(after)
            .bind(limit)
            .fetch_all(&mut **tx)
            .await?
    } else {
        sqlx::query(&sql)
            .bind(workspace_id)
            .bind(target_id)
            .bind(limit)
            .fetch_all(&mut **tx)
            .await?
    };
    Ok(rows.iter().map(row_to_comment).collect())
}

async fn comment_page(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    kind: &str,
    target_id: Uuid,
    query: CommentListQuery,
) -> Result<CommentListPage, CommentDbError> {
    let limit = query.limit.clamp(1, 100);
    let scope = comment_scope(workspace_id, kind, target_id);
    let after = if let Some(cursor) = &query.cursor {
        let id = decode_cursor(cursor, &scope)?;
        let anchor = fetch_comment(tx, workspace_id, id)
            .await
            .map_err(|_| CommentDbError::NotFound)?
            .ok_or(CommentDbError::InvalidCursor)?;
        let matches = match kind {
            "document" => anchor.document_id == Some(target_id),
            "task" => anchor.task_id == Some(target_id),
            _ => false,
        };
        if !matches {
            return Err(CommentDbError::InvalidCursor);
        }
        Some(id)
    } else {
        None
    };
    let rows = list_rows(tx, workspace_id, kind, target_id, limit + 1, after)
        .await
        .map_err(|_| CommentDbError::NotFound)?;
    let has_more = rows.len() > limit as usize;
    let items = rows.into_iter().take(limit as usize).collect::<Vec<_>>();
    let next_cursor = if has_more {
        items.last().map(|row| encode_cursor(row.id, &scope))
    } else {
        None
    };
    Ok(CommentListPage { items, next_cursor })
}

async fn insert_comment(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    document_id: Option<Uuid>,
    task_id: Option<Uuid>,
    input: CreateCommentInput<'_>,
    client_ip: Option<&str>,
) -> Result<Result<CommentRow, CommentDbError>, sqlx::Error> {
    let body = match normalize_body(input.body) {
        Ok(value) => value,
        Err(err) => return Ok(Err(err)),
    };
    let mentioned_user_ids = match normalize_mentions(input.mentioned_user_ids) {
        Ok(value) => value,
        Err(err) => return Ok(Err(err)),
    };
    if let Some(parent_id) = input.parent_id {
        let parent = match fetch_comment(tx, workspace_id, parent_id).await? {
            Some(value) => value,
            None => return Ok(Err(CommentDbError::NotFound)),
        };
        if parent.document_id != document_id || parent.task_id != task_id {
            return Ok(Err(CommentDbError::InvalidInput));
        }
    }
    let comment_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO fvoci.comments (
            id, workspace_id, document_id, task_id, parent_id, created_by, body, chosung
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        "#,
    )
    .bind(comment_id)
    .bind(workspace_id)
    .bind(document_id)
    .bind(task_id)
    .bind(input.parent_id)
    .bind(actor_user_id)
    .bind(&body)
    .bind(to_chosung(&body))
    .execute(&mut **tx)
    .await?;
    record_comment_event(
        tx,
        workspace_id,
        actor_user_id,
        "comment.created",
        comment_id,
        json!({
            "commentId": comment_id.to_string(),
            "documentId": document_id.map(|id| id.to_string()),
            "taskId": task_id.map(|id| id.to_string()),
            "mentionedUserIds": mentioned_user_ids.iter().map(|id| id.to_string()).collect::<Vec<_>>(),
            "parentId": input.parent_id.map(|id| id.to_string()),
        }),
        client_ip,
    )
    .await?;
    let created = match fetch_comment(tx, workspace_id, comment_id).await? {
        Some(value) => value,
        None => return Ok(Err(CommentDbError::NotFound)),
    };
    Ok(Ok(created))
}

async fn begin_write_tx(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<Transaction<'_, Postgres>, CommentDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    lock_membership_users(&mut tx, &[actor_user_id]).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(CommentDbError::NotFound));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(CommentDbError::NotFound));
    }
    Ok(Ok(tx))
}

pub async fn list_document_comments(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
    query: CommentListQuery,
) -> Result<Result<CommentListPage, CommentDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(CommentDbError::NotFound));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(CommentDbError::NotFound));
    }
    let target = comment_result!(document_target(&mut tx, workspace_id, document_id).await);
    match require_parent_access(
        &mut tx,
        workspace_id,
        actor_user_id,
        &ParentTarget::Document(target),
        DocumentPermission::View,
        ProjectPermission::View,
        false,
    )
    .await
    {
        Ok(()) => {}
        Err(err) => return Ok(Err(err)),
    }
    let page = match comment_page(&mut tx, workspace_id, "document", document_id, query).await {
        Ok(value) => value,
        Err(err) => return Ok(Err(err)),
    };
    tx.commit().await?;
    Ok(Ok(page))
}

pub async fn list_task_comments(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    task_id: Uuid,
    query: CommentListQuery,
) -> Result<Result<CommentListPage, CommentDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(CommentDbError::NotFound));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(CommentDbError::NotFound));
    }
    let target = comment_result!(task_target(&mut tx, workspace_id, task_id).await);
    match require_parent_access(
        &mut tx,
        workspace_id,
        actor_user_id,
        &ParentTarget::Task(target),
        DocumentPermission::View,
        ProjectPermission::View,
        false,
    )
    .await
    {
        Ok(()) => {}
        Err(err) => return Ok(Err(err)),
    }
    let page = match comment_page(&mut tx, workspace_id, "task", task_id, query).await {
        Ok(value) => value,
        Err(err) => return Ok(Err(err)),
    };
    tx.commit().await?;
    Ok(Ok(page))
}

pub async fn create_document_comment(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
    input: CreateCommentInput<'_>,
    client_ip: Option<&str>,
) -> Result<Result<CommentRow, CommentDbError>, sqlx::Error> {
    let mut tx =
        commit_comment!(begin_write_tx(pool, workspace_id, actor_user_id, session_id).await?);
    let target = comment_result!(document_target(&mut tx, workspace_id, document_id).await);
    match require_parent_access(
        &mut tx,
        workspace_id,
        actor_user_id,
        &ParentTarget::Document(target),
        DocumentPermission::Edit,
        ProjectPermission::Edit,
        true,
    )
    .await
    {
        Ok(()) => {}
        Err(err) => return Ok(Err(err)),
    }
    let created = match insert_comment(
        &mut tx,
        workspace_id,
        actor_user_id,
        Some(document_id),
        None,
        input,
        client_ip,
    )
    .await?
    {
        Ok(value) => value,
        Err(err) => return Ok(Err(err)),
    };
    tx.commit().await?;
    Ok(Ok(created))
}

pub async fn create_task_comment(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    task_id: Uuid,
    input: CreateCommentInput<'_>,
    client_ip: Option<&str>,
) -> Result<Result<CommentRow, CommentDbError>, sqlx::Error> {
    let mut tx =
        commit_comment!(begin_write_tx(pool, workspace_id, actor_user_id, session_id).await?);
    let target = comment_result!(task_target(&mut tx, workspace_id, task_id).await);
    match require_parent_access(
        &mut tx,
        workspace_id,
        actor_user_id,
        &ParentTarget::Task(target),
        DocumentPermission::Edit,
        ProjectPermission::Edit,
        true,
    )
    .await
    {
        Ok(()) => {}
        Err(err) => return Ok(Err(err)),
    }
    let created = match insert_comment(
        &mut tx,
        workspace_id,
        actor_user_id,
        None,
        Some(task_id),
        input,
        client_ip,
    )
    .await?
    {
        Ok(value) => value,
        Err(err) => return Ok(Err(err)),
    };
    tx.commit().await?;
    Ok(Ok(created))
}

pub async fn update_comment(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    comment_id: Uuid,
    input: PatchCommentInput<'_>,
    client_ip: Option<&str>,
) -> Result<Result<CommentRow, CommentDbError>, sqlx::Error> {
    let mut tx =
        commit_comment!(begin_write_tx(pool, workspace_id, actor_user_id, session_id).await?);
    let comment = comment_result!(fetch_comment(&mut tx, workspace_id, comment_id).await);
    let comment = match comment {
        Some(value) => value,
        None => return Ok(Err(CommentDbError::NotFound)),
    };
    match require_author_or_level(
        &mut tx,
        workspace_id,
        actor_user_id,
        &comment,
        DocumentPermission::Edit,
        ProjectPermission::Edit,
        true,
    )
    .await
    {
        Ok(()) => {}
        Err(err) => return Ok(Err(err)),
    }
    if let Some(body) = input.body {
        let body = match normalize_body(body) {
            Ok(value) => value,
            Err(err) => return Ok(Err(err)),
        };
        if body != comment.body {
            let chosung = to_chosung(&body);
            sqlx::query(
                "UPDATE fvoci.comments SET body = $3, chosung = $4, updated_at = now() WHERE workspace_id = $1 AND id = $2",
            )
            .bind(workspace_id)
            .bind(comment_id)
            .bind(&body)
            .bind(chosung)
            .execute(&mut *tx)
            .await?;
            record_comment_parent_event(
                &mut tx,
                workspace_id,
                actor_user_id,
                &comment,
                "comment.updated",
                client_ip,
            )
            .await?;
        }
    }
    let updated = comment_result!(fetch_comment(&mut tx, workspace_id, comment_id).await);
    let updated = match updated {
        Some(value) => value,
        None => return Ok(Err(CommentDbError::NotFound)),
    };
    tx.commit().await?;
    Ok(Ok(updated))
}

pub async fn purge_comment(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    comment_id: Uuid,
    client_ip: Option<&str>,
) -> Result<Result<(), CommentDbError>, sqlx::Error> {
    let mut tx =
        commit_comment!(begin_write_tx(pool, workspace_id, actor_user_id, session_id).await?);
    let comment = comment_result!(fetch_comment(&mut tx, workspace_id, comment_id).await);
    let comment = match comment {
        Some(value) => value,
        None => return Ok(Err(CommentDbError::NotFound)),
    };
    match require_author_or_level(
        &mut tx,
        workspace_id,
        actor_user_id,
        &comment,
        DocumentPermission::Edit,
        ProjectPermission::Manage,
        true,
    )
    .await
    {
        Ok(()) => {}
        Err(err) => return Ok(Err(err)),
    }
    sqlx::query("DELETE FROM fvoci.comments WHERE workspace_id = $1 AND id = $2")
        .bind(workspace_id)
        .bind(comment_id)
        .execute(&mut *tx)
        .await?;
    record_comment_parent_event(
        &mut tx,
        workspace_id,
        actor_user_id,
        &comment,
        "comment.deleted",
        client_ip,
    )
    .await?;
    tx.commit().await?;
    Ok(Ok(()))
}

pub async fn resolve_comment(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    comment_id: Uuid,
    client_ip: Option<&str>,
) -> Result<Result<CommentRow, CommentDbError>, sqlx::Error> {
    let mut tx =
        commit_comment!(begin_write_tx(pool, workspace_id, actor_user_id, session_id).await?);
    let comment = comment_result!(fetch_comment(&mut tx, workspace_id, comment_id).await);
    let comment = match comment {
        Some(value) => value,
        None => return Ok(Err(CommentDbError::NotFound)),
    };
    if comment.parent_id.is_some() {
        return Ok(Err(CommentDbError::InvalidInput));
    }
    let target = match target_of_comment(&mut tx, workspace_id, &comment).await {
        Ok(value) => value,
        Err(err) => return Ok(Err(err)),
    };
    match require_parent_access(
        &mut tx,
        workspace_id,
        actor_user_id,
        &target,
        DocumentPermission::Edit,
        ProjectPermission::Edit,
        true,
    )
    .await
    {
        Ok(()) => {}
        Err(err) => return Ok(Err(err)),
    }
    sqlx::query(
        "UPDATE fvoci.comments SET resolved_at = now(), updated_at = now() WHERE workspace_id = $1 AND id = $2",
    )
    .bind(workspace_id)
    .bind(comment_id)
    .execute(&mut *tx)
    .await?;
    record_comment_event(
        &mut tx,
        workspace_id,
        actor_user_id,
        "comment.resolved",
        comment_id,
        json!({
            "commentId": comment_id.to_string(),
            "documentId": comment.document_id.map(|id| id.to_string()),
            "taskId": comment.task_id.map(|id| id.to_string()),
        }),
        client_ip,
    )
    .await?;
    let updated = comment_result!(fetch_comment(&mut tx, workspace_id, comment_id).await);
    let updated = match updated {
        Some(value) => value,
        None => return Ok(Err(CommentDbError::NotFound)),
    };
    tx.commit().await?;
    Ok(Ok(updated))
}

pub async fn unresolve_comment(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    comment_id: Uuid,
    client_ip: Option<&str>,
) -> Result<Result<CommentRow, CommentDbError>, sqlx::Error> {
    let mut tx =
        commit_comment!(begin_write_tx(pool, workspace_id, actor_user_id, session_id).await?);
    let comment = comment_result!(fetch_comment(&mut tx, workspace_id, comment_id).await);
    let comment = match comment {
        Some(value) => value,
        None => return Ok(Err(CommentDbError::NotFound)),
    };
    if comment.parent_id.is_some() {
        return Ok(Err(CommentDbError::InvalidInput));
    }
    let target = match target_of_comment(&mut tx, workspace_id, &comment).await {
        Ok(value) => value,
        Err(err) => return Ok(Err(err)),
    };
    match require_parent_access(
        &mut tx,
        workspace_id,
        actor_user_id,
        &target,
        DocumentPermission::Edit,
        ProjectPermission::Edit,
        true,
    )
    .await
    {
        Ok(()) => {}
        Err(err) => return Ok(Err(err)),
    }
    if comment.resolved_at.is_some() {
        sqlx::query(
            "UPDATE fvoci.comments SET resolved_at = NULL, updated_at = now() WHERE workspace_id = $1 AND id = $2",
        )
        .bind(workspace_id)
        .bind(comment_id)
        .execute(&mut *tx)
        .await?;
        record_comment_parent_event(
            &mut tx,
            workspace_id,
            actor_user_id,
            &comment,
            "comment.updated",
            client_ip,
        )
        .await?;
    }
    let updated = comment_result!(fetch_comment(&mut tx, workspace_id, comment_id).await);
    let updated = match updated {
        Some(value) => value,
        None => return Ok(Err(CommentDbError::NotFound)),
    };
    tx.commit().await?;
    Ok(Ok(updated))
}

fn reactions_map(value: &Value) -> BTreeMap<String, Vec<Uuid>> {
    let mut out = BTreeMap::new();
    if let Some(obj) = value.as_object() {
        for (emoji, ids) in obj {
            let parsed = ids
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().and_then(|s| Uuid::parse_str(s).ok()))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            out.insert(emoji.clone(), parsed);
        }
    }
    out
}

fn reactions_to_json(map: &BTreeMap<String, Vec<Uuid>>) -> Value {
    let mut obj = serde_json::Map::new();
    for (emoji, ids) in map {
        obj.insert(
            emoji.clone(),
            Value::Array(ids.iter().map(|id| json!(id.to_string())).collect()),
        );
    }
    Value::Object(obj)
}

pub async fn set_comment_reaction(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    comment_id: Uuid,
    input: ReactionInput<'_>,
    client_ip: Option<&str>,
) -> Result<Result<CommentRow, CommentDbError>, sqlx::Error> {
    if !VALID_REACTIONS.contains(&input.emoji) {
        return Ok(Err(CommentDbError::InvalidInput));
    }
    let mut tx =
        commit_comment!(begin_write_tx(pool, workspace_id, actor_user_id, session_id).await?);
    let comment = comment_result!(fetch_comment(&mut tx, workspace_id, comment_id).await);
    let comment = match comment {
        Some(value) => value,
        None => return Ok(Err(CommentDbError::NotFound)),
    };
    let target = match target_of_comment(&mut tx, workspace_id, &comment).await {
        Ok(value) => value,
        Err(err) => return Ok(Err(err)),
    };
    match require_parent_access(
        &mut tx,
        workspace_id,
        actor_user_id,
        &target,
        DocumentPermission::View,
        ProjectPermission::View,
        true,
    )
    .await
    {
        Ok(()) => {}
        Err(err) => return Ok(Err(err)),
    }
    let mut current = comment;
    for _ in 0..REACTION_TRIES {
        let mut next = reactions_map(&current.reactions);
        let list = next.remove(input.emoji).unwrap_or_default();
        let updated = if input.on {
            if list.contains(&actor_user_id) {
                list
            } else {
                let mut copy = list;
                copy.push(actor_user_id);
                copy
            }
        } else {
            list.into_iter().filter(|id| *id != actor_user_id).collect()
        };
        if !updated.is_empty() {
            next.insert(input.emoji.to_string(), updated);
        }
        let expected = current.reactions.clone();
        let next_json = reactions_to_json(&next);
        let wrote: Option<(Uuid,)> = sqlx::query_as(
            "UPDATE fvoci.comments SET reactions = $4, updated_at = now() WHERE workspace_id = $1 AND id = $2 AND reactions = $3 RETURNING id",
        )
        .bind(workspace_id)
        .bind(comment_id)
        .bind(&expected)
        .bind(next_json)
        .fetch_optional(&mut *tx)
        .await?;
        if wrote.is_some() {
            if reactions_map(&expected) != next {
                record_comment_parent_event(
                    &mut tx,
                    workspace_id,
                    actor_user_id,
                    &current,
                    "comment.updated",
                    client_ip,
                )
                .await?;
            }
            let updated = comment_result!(fetch_comment(&mut tx, workspace_id, comment_id).await);
            let updated = match updated {
                Some(value) => value,
                None => return Ok(Err(CommentDbError::NotFound)),
            };
            tx.commit().await?;
            return Ok(Ok(updated));
        }
        current = match comment_result!(fetch_comment(&mut tx, workspace_id, comment_id).await) {
            Some(value) => value,
            None => return Ok(Err(CommentDbError::NotFound)),
        };
    }
    tx.rollback().await?;
    Ok(Err(CommentDbError::Conflict))
}

pub fn comment_output(
    comment: &CommentRow,
    viewer_user_id: Uuid,
) -> (HashMap<String, ReactionSummary>, i32) {
    let mut reactions = HashMap::new();
    let mut other_reaction_count = 0;
    if let Some(obj) = comment.reactions.as_object() {
        for (emoji, ids) in obj {
            let list = ids
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().and_then(|s| Uuid::parse_str(s).ok()))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if VALID_REACTIONS.contains(&emoji.as_str()) {
                reactions.insert(
                    emoji.clone(),
                    ReactionSummary {
                        count: list.len(),
                        reacted_by_me: list.contains(&viewer_user_id),
                    },
                );
            } else {
                other_reaction_count += list.len() as i32;
            }
        }
    }
    (reactions, other_reaction_count)
}
