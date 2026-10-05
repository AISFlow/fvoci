use std::collections::{BTreeMap, BTreeSet, HashMap};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use crate::db::backend::{Backend, OperationTx};
use crate::db::codec::{Cell, FamilyRow};

use crate::collab::derived_body::to_chosung;
use crate::db::context::{
    begin_read, lock_membership_users, recheck_session, session_is_live, set_tenant,
};
use crate::db::documents::{assert_document_writable, document_permission, DocumentDbError};
use crate::db::groups::list_group_member_user_ids;
use crate::db::identity::{append_audit, append_event, AuditAppend, EventAppend};
use crate::db::projects::{load_live_project, lock_project, project_permission, LiveProject};
use crate::db::workspace::workspace_is_live;
use crate::projects::ProjectPermission;

pub const COMMENT_BODY_MAX: usize = 8000;
const MENTION_MAX: usize = 50;
const REACTION_TRIES: usize = 8;
pub(crate) const VALID_REACTIONS: &[&str] = &["👍", "❤️", "🎉"];

/// Unwraps a helper's inner result, returning its refusal as `Ok(Err(_))`.
/// Database errors travel in the outer `Result` (`?`) and reach the route as a
/// logged 500; only refusals become `CommentDbError`.
macro_rules! commit_comment {
    ($expr:expr) => {
        match $expr {
            Ok(value) => value,
            Err(error) => return Ok(Err(error)),
        }
    };
}

#[derive(Debug)]
pub enum CommentDbError {
    NotFound,
    InvalidInput,
    InvalidCursor,
    Conflict,
    ProjectArchived,
    TaskArchived,
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
    pub mentioned_group_ids: &'a [Uuid],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentWriteKind {
    Document,
    Task,
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

fn body_unit_len(body: &str) -> usize {
    // Source zod `.max(8000)` uses JS string `.length` (UTF-16 code units).
    body.encode_utf16().count()
}

fn normalize_body(body: &str) -> Result<String, CommentDbError> {
    let trimmed = body.trim();
    if trimmed.is_empty() || body_unit_len(trimmed) > COMMENT_BODY_MAX {
        return Err(CommentDbError::InvalidInput);
    }
    Ok(trimmed.to_string())
}

fn normalize_mentions(ids: &[Uuid]) -> Result<Vec<Uuid>, CommentDbError> {
    let unique: Vec<Uuid> = ids
        .iter()
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if unique.len() > MENTION_MAX {
        return Err(CommentDbError::InvalidInput);
    }
    Ok(unique)
}

async fn expand_mentioned_user_ids(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    mentioned_user_ids: &[Uuid],
    mentioned_group_ids: &[Uuid],
) -> Result<Result<Vec<Uuid>, CommentDbError>, sqlx::Error> {
    let direct = match normalize_mentions(mentioned_user_ids) {
        Ok(value) => value,
        Err(err) => return Ok(Err(err)),
    };
    let group_ids = match normalize_mentions(mentioned_group_ids) {
        Ok(value) => value,
        Err(err) => return Ok(Err(err)),
    };
    if group_ids.is_empty() {
        return Ok(Ok(direct));
    }
    let mut expanded = direct.into_iter().collect::<BTreeSet<_>>();
    for group_id in group_ids {
        let members = list_group_member_user_ids(tx, workspace_id, group_id).await?;
        expanded.extend(members);
    }
    Ok(Ok(expanded.into_iter().collect()))
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

pub(crate) fn row_to_comment(row: &sqlx::postgres::PgRow) -> CommentRow {
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

/// Locks the comment row after the parent authorization has taken its project or
/// document lock (parent -> comment, the order cascades and other writers use)
/// and returns its current state, or `None` when a concurrent purge won.
async fn lock_comment(
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
        FOR UPDATE
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
    project_id: Option<Uuid>,
}

struct TaskTarget {
    task_id: Uuid,
    project_id: Uuid,
    archived_at: Option<DateTime<Utc>>,
}

enum ParentTarget {
    Document(DocumentTarget),
    Task(TaskTarget),
}

/// The live document, or `None` when it is missing or trashed.
async fn document_target(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    document_id: Uuid,
) -> Result<Option<DocumentTarget>, sqlx::Error> {
    let row: Option<(Option<Uuid>, Option<DateTime<Utc>>)> = sqlx::query_as(
        "SELECT project_id, deleted_at FROM fvoci.documents WHERE workspace_id = $1 AND id = $2",
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(match row {
        Some((project_id, None)) => Some(DocumentTarget {
            document_id,
            project_id,
        }),
        _ => None,
    })
}

/// The live task, or `None` when it is missing or trashed.
async fn task_target(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    task_id: Uuid,
) -> Result<Option<TaskTarget>, sqlx::Error> {
    type TaskTargetRow = (Uuid, Option<DateTime<Utc>>, Option<DateTime<Utc>>);
    let row: Option<TaskTargetRow> = sqlx::query_as(
        "SELECT project_id, deleted_at, archived_at FROM fvoci.tasks WHERE workspace_id = $1 AND id = $2",
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(match row {
        Some((project_id, None, archived_at)) => Some(TaskTarget {
            task_id,
            project_id,
            archived_at,
        }),
        _ => None,
    })
}

async fn target_of_comment(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    comment: &CommentRow,
) -> Result<Result<ParentTarget, CommentDbError>, sqlx::Error> {
    let target = if let Some(document_id) = comment.document_id {
        document_target(tx, workspace_id, document_id)
            .await?
            .map(ParentTarget::Document)
    } else if let Some(task_id) = comment.task_id {
        task_target(tx, workspace_id, task_id)
            .await?
            .map(ParentTarget::Task)
    } else {
        None
    };
    Ok(target.ok_or(CommentDbError::NotFound))
}

fn map_document_error(_err: DocumentDbError) -> CommentDbError {
    // Every document-side refusal is reported as not found (no existence leak).
    CommentDbError::NotFound
}

async fn require_wiki_document_access(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    document: &DocumentTarget,
    min: ProjectPermission,
    writable: bool,
) -> Result<Result<(), CommentDbError>, sqlx::Error> {
    if document.project_id.is_some() {
        return Ok(Err(CommentDbError::NotFound));
    }
    // Single wiki document permission path (db::documents::document_permission).
    let permission =
        document_permission(tx, workspace_id, actor_user_id, document.document_id, true).await?;
    if !permission.at_least(min) || permission == ProjectPermission::None {
        return Ok(Err(CommentDbError::NotFound));
    }
    if writable {
        let writable =
            assert_document_writable(tx, workspace_id, document.document_id, None).await?;
        commit_comment!(writable.map_err(map_document_error));
    }
    Ok(Ok(()))
}

/// The parent's live project row: locked for a comment write, so a concurrent
/// archive, trash or member removal serializes with it; read without a lock
/// for a comment list (its read transaction gives the check and the rows one
/// snapshot).
async fn parent_project(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
    writable: bool,
) -> Result<Option<LiveProject>, sqlx::Error> {
    if writable {
        lock_project(tx, workspace_id, project_id).await
    } else {
        load_live_project(tx, workspace_id, project_id).await
    }
}

async fn require_project_document_access(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    expected_project_id: Option<Uuid>,
    document: &DocumentTarget,
    min: ProjectPermission,
    writable: bool,
) -> Result<Result<(), CommentDbError>, sqlx::Error> {
    let Some(project_id) = document.project_id else {
        return Ok(Err(CommentDbError::NotFound));
    };
    if expected_project_id.is_some_and(|expected| expected != project_id) {
        return Ok(Err(CommentDbError::NotFound));
    }
    let Some(project) = parent_project(tx, workspace_id, project_id, writable).await? else {
        return Ok(Err(CommentDbError::NotFound));
    };
    let permission = project_permission(tx, workspace_id, actor_user_id, &project).await?;
    if !permission.at_least(min) {
        return Ok(Err(CommentDbError::NotFound));
    }
    if writable {
        if project.status == "archived" {
            return Ok(Err(CommentDbError::ProjectArchived));
        }
        let writable =
            assert_document_writable(tx, workspace_id, document.document_id, Some(project_id))
                .await?;
        commit_comment!(writable.map_err(map_document_error));
    }
    Ok(Ok(()))
}

async fn require_task_access(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    task: &TaskTarget,
    min: ProjectPermission,
    writable: bool,
) -> Result<Result<(), CommentDbError>, sqlx::Error> {
    let Some(project) = parent_project(tx, workspace_id, task.project_id, writable).await? else {
        return Ok(Err(CommentDbError::NotFound));
    };
    let permission = project_permission(tx, workspace_id, actor_user_id, &project).await?;
    if !permission.at_least(min) {
        return Ok(Err(CommentDbError::NotFound));
    }
    if writable {
        if project.status == "archived" {
            return Ok(Err(CommentDbError::ProjectArchived));
        }
        // `task` was read before the project lock. Task trash, restore and
        // archive take this project lock before changing the task row, so a
        // fresh read now sees any of them that committed while we waited.
        let Some(current) = task_target(tx, workspace_id, task.task_id).await? else {
            return Ok(Err(CommentDbError::NotFound));
        };
        if current.project_id != task.project_id {
            return Ok(Err(CommentDbError::NotFound));
        }
        if current.archived_at.is_some() {
            return Ok(Err(CommentDbError::TaskArchived));
        }
    }
    Ok(Ok(()))
}

async fn require_parent_access(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    target: &ParentTarget,
    min: ProjectPermission,
    writable: bool,
) -> Result<Result<(), CommentDbError>, sqlx::Error> {
    match target {
        ParentTarget::Document(doc) => {
            if doc.project_id.is_some() {
                require_project_document_access(
                    tx,
                    workspace_id,
                    actor_user_id,
                    None,
                    doc,
                    min,
                    writable,
                )
                .await
            } else {
                require_wiki_document_access(tx, workspace_id, actor_user_id, doc, min, writable)
                    .await
            }
        }
        ParentTarget::Task(task) => {
            require_task_access(tx, workspace_id, actor_user_id, task, min, writable).await
        }
    }
}

async fn require_author_or_level(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    comment: &CommentRow,
    min: ProjectPermission,
    writable: bool,
) -> Result<Result<(), CommentDbError>, sqlx::Error> {
    let target = commit_comment!(target_of_comment(tx, workspace_id, comment).await?);
    commit_comment!(
        require_parent_access(
            tx,
            workspace_id,
            actor_user_id,
            &target,
            ProjectPermission::View,
            writable,
        )
        .await?
    );
    if comment.created_by == actor_user_id {
        return Ok(Ok(()));
    }
    require_parent_access(tx, workspace_id, actor_user_id, &target, min, writable).await
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
) -> Result<Result<CommentListPage, CommentDbError>, sqlx::Error> {
    if !(1..=100).contains(&query.limit) {
        return Ok(Err(CommentDbError::InvalidInput));
    }
    let limit = query.limit;
    let scope = comment_scope(workspace_id, kind, target_id);
    let after = if let Some(cursor) = &query.cursor {
        if cursor.len() > 1024 {
            return Ok(Err(CommentDbError::InvalidInput));
        }
        let id = commit_comment!(decode_cursor(cursor, &scope));
        let Some(anchor) = fetch_comment(tx, workspace_id, id).await? else {
            return Ok(Err(CommentDbError::InvalidCursor));
        };
        let matches = match kind {
            "document" => anchor.document_id == Some(target_id),
            "task" => anchor.task_id == Some(target_id),
            _ => false,
        };
        if !matches {
            return Ok(Err(CommentDbError::InvalidCursor));
        }
        Some(id)
    } else {
        None
    };
    let rows = list_rows(tx, workspace_id, kind, target_id, limit + 1, after).await?;
    let has_more = rows.len() > limit as usize;
    let items = rows.into_iter().take(limit as usize).collect::<Vec<_>>();
    let next_cursor = if has_more {
        items.last().map(|row| encode_cursor(row.id, &scope))
    } else {
        None
    };
    Ok(Ok(CommentListPage { items, next_cursor }))
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
    let mentioned_user_ids = match expand_mentioned_user_ids(
        tx,
        workspace_id,
        input.mentioned_user_ids,
        input.mentioned_group_ids,
    )
    .await?
    {
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
            "mentionedGroupIds": input
                .mentioned_group_ids
                .iter()
                .copied()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>(),
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
    let mut tx = begin_read(pool).await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(CommentDbError::NotFound));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(CommentDbError::NotFound));
    }
    let Some(target) = document_target(&mut tx, workspace_id, document_id).await? else {
        return Ok(Err(CommentDbError::NotFound));
    };
    commit_comment!(
        require_wiki_document_access(
            &mut tx,
            workspace_id,
            actor_user_id,
            &target,
            ProjectPermission::View,
            false,
        )
        .await?
    );
    let page =
        commit_comment!(comment_page(&mut tx, workspace_id, "document", document_id, query).await?);
    tx.commit().await?;
    Ok(Ok(page))
}

/// Selected wiki read; the PostgreSQL API and all comment writes stay intact.
pub async fn list_document_comments_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
    query: CommentListQuery,
) -> Result<Result<CommentListPage, CommentDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return list_document_comments(
            pool,
            workspace_id,
            actor_user_id,
            session_id,
            document_id,
            query,
        )
        .await;
    }
    let mut tx = backend.begin_read().await?;
    let result = async {
        let mut op = tx.operation();
        op.set_tenant(workspace_id).await?;
        if !op.session_is_live(actor_user_id, session_id).await?
            || !op.workspace_is_live(workspace_id).await?
            || !op
                .document_permission(workspace_id, actor_user_id, document_id, true)
                .await?
                .at_least(ProjectPermission::View)
        {
            return Ok(Err(CommentDbError::NotFound));
        }
        // document_permission admits only a live wiki document, including its
        // current group grants. All page reads share that authorization snapshot.
        let OperationTx::SqliteFamily(family) = op else {
            unreachable!()
        };
        family_comment_page(family, workspace_id, document_id, query).await
    }
    .await;
    match result {
        Ok(Ok(page)) => {
            tx.commit_with_cleanup()
                .await
                .map_err(|error| sqlx::Error::AnyDriverError(Box::new(error)))?;
            Ok(Ok(page))
        }
        Ok(Err(refusal)) => {
            if let Err(cleanup) = tx.rollback().await {
                return Err(crate::db::backend::rollback_cleanup_unknown(
                    Some(Box::new(CommentReadRefusal(refusal))),
                    cleanup,
                ));
            }
            Ok(Err(refusal))
        }
        Err(original) => {
            if let Err(cleanup) = tx.rollback().await {
                return Err(crate::db::backend::rollback_cleanup_unknown(
                    Some(Box::new(original)),
                    cleanup,
                ));
            }
            Err(original)
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("comment read refused: {0:?}")]
struct CommentReadRefusal(CommentDbError);

fn family_comment_row(row: &FamilyRow) -> Result<CommentRow, sqlx::Error> {
    Ok(CommentRow {
        id: row.cell(0)?.id()?,
        workspace_id: row.cell(1)?.id()?,
        document_id: row.cell(2)?.optional(Cell::id)?,
        task_id: row.cell(3)?.optional(Cell::id)?,
        parent_id: row.cell(4)?.optional(Cell::id)?,
        created_by: row.cell(5)?.id()?,
        body: row.cell(6)?.string()?,
        resolved_at: row.cell(7)?.optional(Cell::datetime)?,
        reactions: row.cell(8)?.value()?,
        created_at: row.cell(9)?.datetime()?,
        updated_at: row.cell(10)?.datetime()?,
    })
}

async fn family_comment_page(
    family: &mut crate::db::backend::FamilyTx,
    workspace: Uuid,
    document: Uuid,
    query: CommentListQuery,
) -> Result<Result<CommentListPage, CommentDbError>, sqlx::Error> {
    family.require_tenant(workspace)?;
    if !(1..=100).contains(&query.limit) {
        return Ok(Err(CommentDbError::InvalidInput));
    }
    let scope = comment_scope(workspace, "document", document);
    let after = if let Some(cursor) = &query.cursor {
        if cursor.len() > 1024 {
            return Ok(Err(CommentDbError::InvalidInput));
        }
        let id = match decode_cursor(cursor, &scope) {
            Ok(id) => id,
            Err(error) => return Ok(Err(error)),
        };
        let rows = family
            .query(
                "SELECT document_id FROM comments WHERE workspace_id=?1 AND id=?2",
                &[Cell::uuid(workspace), Cell::uuid(id)],
            )
            .await?;
        let Some(anchor) = rows.first() else {
            return Ok(Err(CommentDbError::InvalidCursor));
        };
        if anchor.cell(0)?.optional(Cell::id)? != Some(document) {
            return Ok(Err(CommentDbError::InvalidCursor));
        }
        Some(id)
    } else {
        None
    };
    let rows = family.query(
        "SELECT c.id,c.workspace_id,c.document_id,c.task_id,c.parent_id,c.created_by,c.body,c.resolved_at,c.reactions,c.created_at,c.updated_at
         FROM comments c WHERE c.workspace_id=?1 AND c.document_id=?2
         AND (?3 IS NULL OR (c.created_at,c.id)>(SELECT created_at,id FROM comments WHERE workspace_id=?1 AND id=?3))
         ORDER BY c.created_at ASC,c.id ASC LIMIT ?4",
        &[Cell::uuid(workspace),Cell::uuid(document),Cell::optional_uuid(after),Cell::Integer(i64::from(query.limit)+1)],
    ).await?;
    let mut items = rows
        .iter()
        .map(family_comment_row)
        .collect::<Result<Vec<_>, _>>()?;
    let has_more = items.len() > query.limit as usize;
    items.truncate(query.limit as usize);
    let next_cursor = if has_more {
        items.last().map(|row| encode_cursor(row.id, &scope))
    } else {
        None
    };
    Ok(Ok(CommentListPage { items, next_cursor }))
}

pub async fn list_task_comments(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    task_id: Uuid,
    query: CommentListQuery,
) -> Result<Result<CommentListPage, CommentDbError>, sqlx::Error> {
    let mut tx = begin_read(pool).await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(CommentDbError::NotFound));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(CommentDbError::NotFound));
    }
    let Some(target) = task_target(&mut tx, workspace_id, task_id).await? else {
        return Ok(Err(CommentDbError::NotFound));
    };
    commit_comment!(
        require_parent_access(
            &mut tx,
            workspace_id,
            actor_user_id,
            &ParentTarget::Task(target),
            ProjectPermission::View,
            false,
        )
        .await?
    );
    let page = commit_comment!(comment_page(&mut tx, workspace_id, "task", task_id, query).await?);
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
    let Some(target) = document_target(&mut tx, workspace_id, document_id).await? else {
        return Ok(Err(CommentDbError::NotFound));
    };
    commit_comment!(
        require_wiki_document_access(
            &mut tx,
            workspace_id,
            actor_user_id,
            &target,
            ProjectPermission::Edit,
            true,
        )
        .await?
    );
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

/// Normal wiki comment publication on the selected backend. PG keeps its
/// public wrapper and lock order; family authorization, parent validation,
/// comment, event and audit all share the already reserved current writer.
pub async fn create_document_comment_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
    input: CreateCommentInput<'_>,
    client_ip: Option<&str>,
) -> Result<Result<CommentRow, CommentDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return create_document_comment(
            pool,
            workspace_id,
            actor_user_id,
            session_id,
            document_id,
            input,
            client_ip,
        )
        .await;
    }
    let mut tx = backend.begin_write().await?;
    let result = async {
        let mut op = tx.operation();
        op.set_tenant(workspace_id).await?;
        op.lock_membership_users(&[actor_user_id]).await?;
        if !op.recheck_session(actor_user_id,session_id).await?
            || !op.workspace_is_live(workspace_id).await?
        { return Ok(Err(CommentDbError::NotFound)); }
        op.lock_tree(workspace_id).await?;
        let Some(document) = op.document_row(workspace_id,document_id).await? else {
            return Ok(Err(CommentDbError::NotFound));
        };
        if document.8.is_some() || !op.document_permission(workspace_id,actor_user_id,document_id,true).await?.at_least(ProjectPermission::Edit) {
            return Ok(Err(CommentDbError::NotFound));
        }
        let body = match normalize_body(input.body) {
            Ok(body) => body,
            Err(error) => return Ok(Err(error)),
        };
        let direct = match normalize_mentions(input.mentioned_user_ids) {
            Ok(ids) => ids,
            Err(error) => return Ok(Err(error)),
        };
        let groups = match normalize_mentions(input.mentioned_group_ids) {
            Ok(ids) => ids,
            Err(error) => return Ok(Err(error)),
        };
        let OperationTx::SqliteFamily(family) = &mut op else { unreachable!() };
        family.require_writer()?;
        family.require_tenant(workspace_id)?;
        let mut mentioned = direct.into_iter().collect::<BTreeSet<_>>();
        for group in &groups {
            // Same tenant-scoped expansion as list_group_member_user_ids; do
            // not impose a new cap on the original union of group members.
            let members = family.query(
                "SELECT user_id FROM group_members WHERE workspace_id=?1 AND group_id=?2 ORDER BY user_id",
                &[Cell::uuid(workspace_id),Cell::uuid(*group)],
            ).await?;
            for member in members { mentioned.insert(member.cell(0)?.id()?); }
        }
        if let Some(parent) = input.parent_id {
            let parents = family.query(
                "SELECT document_id,task_id FROM comments WHERE workspace_id=?1 AND id=?2",
                &[Cell::uuid(workspace_id),Cell::uuid(parent)],
            ).await?;
            let Some(parent) = parents.first() else { return Ok(Err(CommentDbError::NotFound)); };
            if parent.cell(0)?.optional(Cell::id)? != Some(document_id)
                || parent.cell(1)?.optional(Cell::id)?.is_some()
            { return Ok(Err(CommentDbError::InvalidInput)); }
        }
        let comment_id = Uuid::now_v7();
        let rows = family.query(
            "INSERT INTO comments(id,workspace_id,document_id,parent_id,created_by,body,chosung)
             VALUES(?1,?2,?3,?4,?5,?6,?7)
             RETURNING id,workspace_id,document_id,task_id,parent_id,created_by,body,resolved_at,reactions,created_at,updated_at",
            &[Cell::uuid(comment_id),Cell::uuid(workspace_id),Cell::uuid(document_id),Cell::optional_uuid(input.parent_id),
              Cell::uuid(actor_user_id),Cell::text(&body),Cell::text(to_chosung(&body))],
        ).await?;
        let Some(row) = rows.first() else { return Ok(Err(CommentDbError::NotFound)); };
        let created = family_comment_row(row)?;
        let payload = json!({
            "commentId":comment_id.to_string(),"documentId":document_id.to_string(),"taskId":null,
            "mentionedUserIds":mentioned.into_iter().map(|id|id.to_string()).collect::<Vec<_>>(),
            "mentionedGroupIds":groups.into_iter().map(|id|id.to_string()).collect::<Vec<_>>(),
            "parentId":input.parent_id.map(|id|id.to_string()),
        });
        op.append_event(EventAppend {
            id:Uuid::now_v7(),workspace_id:Some(workspace_id),actor_user_id:Some(actor_user_id),
            verb:"comment.created".into(),target_type:Some("comment".into()),target_id:Some(comment_id),payload:payload.clone(),
        }).await?;
        op.append_audit(AuditAppend {
            id:Uuid::now_v7(),workspace_id:Some(workspace_id),actor_user_id:Some(actor_user_id),
            verb:"comment.created".into(),target_type:Some("comment".into()),target_id:Some(comment_id),payload,
            ip:client_ip.map(str::to_string),
        }).await?;
        Ok(Ok(created))
    }.await;
    match result {
        Ok(Ok(created)) => {
            tx.commit_with_cleanup()
                .await
                .map_err(|error| sqlx::Error::AnyDriverError(Box::new(error)))?;
            Ok(Ok(created))
        }
        Ok(Err(refusal)) => {
            if let Err(cleanup) = tx.rollback().await {
                return Err(crate::db::backend::rollback_cleanup_unknown(
                    Some(Box::new(CommentWriteRefusal(refusal))),
                    cleanup,
                ));
            }
            Ok(Err(refusal))
        }
        Err(original) => {
            if let Err(cleanup) = tx.rollback().await {
                return Err(crate::db::backend::rollback_cleanup_unknown(
                    Some(Box::new(original)),
                    cleanup,
                ));
            }
            Err(original)
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("comment write refused: {0:?}")]
struct CommentWriteRefusal(CommentDbError);

pub async fn list_project_document_comments(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    project_id: Uuid,
    document_id: Uuid,
    query: CommentListQuery,
) -> Result<Result<CommentListPage, CommentDbError>, sqlx::Error> {
    let mut tx = begin_read(pool).await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(CommentDbError::NotFound));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(CommentDbError::NotFound));
    }
    let Some(target) = document_target(&mut tx, workspace_id, document_id).await? else {
        return Ok(Err(CommentDbError::NotFound));
    };
    commit_comment!(
        require_project_document_access(
            &mut tx,
            workspace_id,
            actor_user_id,
            Some(project_id),
            &target,
            ProjectPermission::View,
            false,
        )
        .await?
    );
    let page =
        commit_comment!(comment_page(&mut tx, workspace_id, "document", document_id, query).await?);
    tx.commit().await?;
    Ok(Ok(page))
}

pub async fn create_project_document_comment(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    project_and_document: (Uuid, Uuid),
    input: CreateCommentInput<'_>,
    client_ip: Option<&str>,
) -> Result<Result<CommentRow, CommentDbError>, sqlx::Error> {
    let (project_id, document_id) = project_and_document;
    let mut tx =
        commit_comment!(begin_write_tx(pool, workspace_id, actor_user_id, session_id).await?);
    let Some(target) = document_target(&mut tx, workspace_id, document_id).await? else {
        return Ok(Err(CommentDbError::NotFound));
    };
    commit_comment!(
        require_project_document_access(
            &mut tx,
            workspace_id,
            actor_user_id,
            Some(project_id),
            &target,
            ProjectPermission::Edit,
            true,
        )
        .await?
    );
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

pub async fn comment_write_kind(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    comment_id: Uuid,
) -> Result<Result<CommentWriteKind, CommentDbError>, sqlx::Error> {
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
    let comment = fetch_comment(&mut tx, workspace_id, comment_id).await?;
    let Some(comment) = comment else {
        return Ok(Err(CommentDbError::NotFound));
    };
    let kind = if comment.document_id.is_some() {
        CommentWriteKind::Document
    } else if comment.task_id.is_some() {
        CommentWriteKind::Task
    } else {
        return Ok(Err(CommentDbError::NotFound));
    };
    tx.commit().await?;
    Ok(Ok(kind))
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
    let Some(target) = task_target(&mut tx, workspace_id, task_id).await? else {
        return Ok(Err(CommentDbError::NotFound));
    };
    commit_comment!(
        require_parent_access(
            &mut tx,
            workspace_id,
            actor_user_id,
            &ParentTarget::Task(target),
            ProjectPermission::Edit,
            true,
        )
        .await?
    );
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
    let comment = fetch_comment(&mut tx, workspace_id, comment_id).await?;
    let comment = match comment {
        Some(value) => value,
        None => return Ok(Err(CommentDbError::NotFound)),
    };
    commit_comment!(
        require_author_or_level(
            &mut tx,
            workspace_id,
            actor_user_id,
            &comment,
            ProjectPermission::Edit,
            true,
        )
        .await?
    );
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
    let updated = fetch_comment(&mut tx, workspace_id, comment_id).await?;
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
    let comment = fetch_comment(&mut tx, workspace_id, comment_id).await?;
    let comment = match comment {
        Some(value) => value,
        None => return Ok(Err(CommentDbError::NotFound)),
    };
    commit_comment!(
        require_author_or_level(
            &mut tx,
            workspace_id,
            actor_user_id,
            &comment,
            ProjectPermission::Manage,
            true,
        )
        .await?
    );
    if lock_comment(&mut tx, workspace_id, comment_id)
        .await?
        .is_none()
    {
        return Ok(Err(CommentDbError::NotFound));
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
    let comment = fetch_comment(&mut tx, workspace_id, comment_id).await?;
    let comment = match comment {
        Some(value) => value,
        None => return Ok(Err(CommentDbError::NotFound)),
    };
    if comment.parent_id.is_some() {
        return Ok(Err(CommentDbError::InvalidInput));
    }
    let target = commit_comment!(target_of_comment(&mut tx, workspace_id, &comment).await?);
    commit_comment!(
        require_parent_access(
            &mut tx,
            workspace_id,
            actor_user_id,
            &target,
            ProjectPermission::Edit,
            true,
        )
        .await?
    );
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
    let updated = fetch_comment(&mut tx, workspace_id, comment_id).await?;
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
    let comment = fetch_comment(&mut tx, workspace_id, comment_id).await?;
    let comment = match comment {
        Some(value) => value,
        None => return Ok(Err(CommentDbError::NotFound)),
    };
    if comment.parent_id.is_some() {
        return Ok(Err(CommentDbError::InvalidInput));
    }
    let target = commit_comment!(target_of_comment(&mut tx, workspace_id, &comment).await?);
    commit_comment!(
        require_parent_access(
            &mut tx,
            workspace_id,
            actor_user_id,
            &target,
            ProjectPermission::Edit,
            true,
        )
        .await?
    );
    let comment = match lock_comment(&mut tx, workspace_id, comment_id).await? {
        Some(value) => value,
        None => return Ok(Err(CommentDbError::NotFound)),
    };
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
    let updated = fetch_comment(&mut tx, workspace_id, comment_id).await?;
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
    let comment = fetch_comment(&mut tx, workspace_id, comment_id).await?;
    let comment = match comment {
        Some(value) => value,
        None => return Ok(Err(CommentDbError::NotFound)),
    };
    let target = commit_comment!(target_of_comment(&mut tx, workspace_id, &comment).await?);
    commit_comment!(
        require_parent_access(
            &mut tx,
            workspace_id,
            actor_user_id,
            &target,
            ProjectPermission::View,
            true,
        )
        .await?
    );
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
            let updated = fetch_comment(&mut tx, workspace_id, comment_id).await?;
            let updated = match updated {
                Some(value) => value,
                None => return Ok(Err(CommentDbError::NotFound)),
            };
            tx.commit().await?;
            return Ok(Ok(updated));
        }
        current = match fetch_comment(&mut tx, workspace_id, comment_id).await? {
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

#[cfg(all(test, feature = "db-tests"))]
mod selected_wiki_comment_read_tests {
    use super::*;
    use crate::db::attachment_preview::tests::Fixture;

    #[tokio::test]
    async fn wiki_aux_comments_selected_literal_paging_scope_current_parent_and_session() {
        let f = Fixture::new().await;
        let credential = Uuid::now_v7();
        sqlx::query("INSERT INTO sessions(id,user_id,token_hash,expires_at) VALUES(?1,?2,'comments-read',?3)")
            .bind(credential.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice())
            .bind(chrono::Utc::now().timestamp_micros()+86_400_000_000).execute(&f.pool).await.unwrap();
        let ids = [
            Uuid::from_u128(101),
            Uuid::from_u128(102),
            Uuid::from_u128(103),
        ];
        for (index, id) in ids.iter().enumerate() {
            let parent = (index == 1).then_some(ids[0]);
            sqlx::query("INSERT INTO comments(id,workspace_id,document_id,parent_id,created_by,body,reactions,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,1000000,1000000)")
                .bind(id.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(f.document.as_bytes().as_slice())
                .bind(parent.map(|id|id.as_bytes().to_vec())).bind(f.user.as_bytes().as_slice())
                .bind(format!("댓글 한글 😀 {index}")).bind(serde_json::json!({"👍":[f.user]}).to_string()).execute(&f.pool).await.unwrap();
        }
        let first = list_document_comments_backend(
            &f.backend,
            f.workspace,
            f.user,
            credential,
            f.document,
            CommentListQuery {
                limit: 2,
                cursor: None,
            },
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            first.items.iter().map(|row| row.id).collect::<Vec<_>>(),
            ids[..2]
        );
        assert_eq!(first.items[0].body, "댓글 한글 😀 0");
        assert_eq!(first.items[1].parent_id, Some(ids[0]));
        assert_eq!(first.items[0].document_id, Some(f.document));
        assert_eq!(first.items[0].reactions, serde_json::json!({"👍":[f.user]}));
        let cursor = first.next_cursor.unwrap();
        let last = list_document_comments_backend(
            &f.backend,
            f.workspace,
            f.user,
            credential,
            f.document,
            CommentListQuery {
                limit: 2,
                cursor: Some(cursor.clone()),
            },
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            last.items.iter().map(|row| row.id).collect::<Vec<_>>(),
            vec![ids[2]]
        );
        assert!(last.next_cursor.is_none());
        for bad in [
            "not-base64".to_string(),
            encode_cursor(
                ids[0],
                &comment_scope(Uuid::now_v7(), "document", f.document),
            ),
            encode_cursor(
                Uuid::now_v7(),
                &comment_scope(f.workspace, "document", f.document),
            ),
        ] {
            assert!(matches!(
                list_document_comments_backend(
                    &f.backend,
                    f.workspace,
                    f.user,
                    credential,
                    f.document,
                    CommentListQuery {
                        limit: 2,
                        cursor: Some(bad)
                    }
                )
                .await
                .unwrap(),
                Err(CommentDbError::InvalidCursor)
            ));
        }
        for limit in [0, 101] {
            assert!(matches!(
                list_document_comments_backend(
                    &f.backend,
                    f.workspace,
                    f.user,
                    credential,
                    f.document,
                    CommentListQuery {
                        limit,
                        cursor: None
                    }
                )
                .await
                .unwrap(),
                Err(CommentDbError::InvalidInput)
            ));
        }
        assert!(matches!(
            list_document_comments_backend(
                &f.backend,
                f.workspace,
                f.user,
                credential,
                f.document,
                CommentListQuery {
                    limit: 2,
                    cursor: Some("x".repeat(1025))
                }
            )
            .await
            .unwrap(),
            Err(CommentDbError::InvalidInput)
        ));
        let other_document = Uuid::now_v7();
        sqlx::query("INSERT INTO documents(id,workspace_id,title,path,sort_key,number,status,schema_version,created_by,content_json) VALUES(?1,?2,'Other',?3,'W',2,'published',2,?4,'{}')")
            .bind(other_document.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(other_document.simple().to_string()).bind(f.user.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        assert!(matches!(
            list_document_comments_backend(
                &f.backend,
                f.workspace,
                f.user,
                credential,
                other_document,
                CommentListQuery {
                    limit: 2,
                    cursor: Some(cursor)
                }
            )
            .await
            .unwrap(),
            Err(CommentDbError::InvalidCursor)
        ));
        sqlx::query("UPDATE sessions SET revoked_at=1 WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            list_document_comments_backend(
                &f.backend,
                f.workspace,
                f.user,
                credential,
                f.document,
                CommentListQuery {
                    limit: 50,
                    cursor: None
                }
            )
            .await
            .unwrap(),
            Err(CommentDbError::NotFound)
        ));
        sqlx::query("UPDATE sessions SET revoked_at=NULL WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE documents SET deleted_at=1 WHERE id=?1")
            .bind(f.document.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            list_document_comments_backend(
                &f.backend,
                f.workspace,
                f.user,
                credential,
                f.document,
                CommentListQuery {
                    limit: 50,
                    cursor: None
                }
            )
            .await
            .unwrap(),
            Err(CommentDbError::NotFound)
        ));
        sqlx::query("UPDATE documents SET deleted_at=NULL WHERE id=?1")
            .bind(f.document.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let healthy = list_document_comments_backend(
            &f.backend,
            f.workspace,
            f.user,
            credential,
            f.document,
            CommentListQuery {
                limit: 50,
                cursor: None,
            },
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            healthy.items.iter().map(|row| row.id).collect::<Vec<_>>(),
            ids
        );
        // A driver failure is not a successful empty page or a masked refusal.
        sqlx::query("ALTER TABLE comments RENAME TO comments_read_failure")
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(list_document_comments_backend(
            &f.backend,
            f.workspace,
            f.user,
            credential,
            f.document,
            CommentListQuery {
                limit: 50,
                cursor: None
            }
        )
        .await
        .is_err());
        sqlx::query("ALTER TABLE comments_read_failure RENAME TO comments")
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            list_document_comments_backend(
                &f.backend,
                f.workspace,
                f.user,
                credential,
                f.document,
                CommentListQuery {
                    limit: 50,
                    cursor: None
                }
            )
            .await
            .unwrap()
            .unwrap()
            .items
            .len(),
            3
        );
        f.pool.close().await;
        std::fs::remove_dir_all(&f.root).unwrap();
    }
}

#[cfg(all(test, feature = "db-tests"))]
mod selected_comment_write_tests {
    use super::*;
    use crate::db::attachment_preview::tests::Fixture;

    async fn credential(f: &Fixture) -> Uuid {
        let id = Uuid::now_v7();
        let mut tx = f.backend.begin_write().await.unwrap();
        tx.operation()
            .create_session(
                id,
                f.user,
                "comment-write-session",
                Utc::now() + chrono::Duration::days(1),
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();
        id
    }
    fn input(body: &str, parent: Option<Uuid>) -> CreateCommentInput<'_> {
        CreateCommentInput {
            body,
            parent_id: parent,
            mentioned_user_ids: &[],
            mentioned_group_ids: &[],
        }
    }
    async fn effects(f: &Fixture) -> (i64, i64, i64, i64) {
        sqlx::query_as("SELECT (SELECT count(*) FROM comments),(SELECT count(*) FROM events),(SELECT count(*) FROM audit_log),(SELECT coalesce(max(seq),0) FROM events)")
            .fetch_one(&f.pool).await.unwrap()
    }

    #[tokio::test]
    async fn wiki_aux_mutation_comment_literal_parent_mentions_event_audit_atomic_retry() {
        let f = Fixture::new().await;
        let credential = credential(&f).await;
        let parent = create_document_comment_backend(
            &f.backend,
            f.workspace,
            f.user,
            credential,
            f.document,
            input(" 부모 😀 ", None),
            Some("127.0.0.1"),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(parent.body, "부모 😀");
        assert_eq!(
            (
                parent.document_id,
                parent.task_id,
                parent.parent_id,
                parent.created_by
            ),
            (Some(f.document), None, None, f.user)
        );
        let member = Uuid::now_v7();
        sqlx::query("INSERT INTO users(id,email,given_name) VALUES(?1,'comment-mention@example.test','Mention')").bind(member.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        sqlx::query("INSERT INTO memberships(workspace_id,user_id,role) VALUES(?1,?2,'member')")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(member.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let group = Uuid::now_v7();
        sqlx::query("INSERT INTO groups(id,workspace_id,name) VALUES(?1,?2,'Comment mentions')")
            .bind(group.as_bytes().as_slice())
            .bind(f.workspace.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        for user in [member, f.user] {
            sqlx::query(
                "INSERT INTO group_members(workspace_id,group_id,user_id) VALUES(?1,?2,?3)",
            )
            .bind(f.workspace.as_bytes().as_slice())
            .bind(group.as_bytes().as_slice())
            .bind(user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        }
        let direct = [f.user, f.user];
        let groups = [group, group];
        let reply = create_document_comment_backend(
            &f.backend,
            f.workspace,
            f.user,
            credential,
            f.document,
            CreateCommentInput {
                body: " 실제 댓글 中 😀 ",
                parent_id: Some(parent.id),
                mentioned_user_ids: &direct,
                mentioned_group_ids: &groups,
            },
            Some("127.0.0.1"),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(reply.body, "실제 댓글 中 😀");
        assert_eq!(reply.parent_id, Some(parent.id));
        assert_eq!(reply.reactions, json!({}));
        let chosung: String = sqlx::query_scalar("SELECT chosung FROM comments WHERE id=?1")
            .bind(reply.id.as_bytes().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(chosung, to_chosung("실제 댓글 中 😀"));
        let expected = json!({"commentId":reply.id.to_string(),"documentId":f.document.to_string(),"taskId":null,
            "mentionedUserIds":[f.user,member].into_iter().collect::<BTreeSet<_>>().into_iter().map(|id|id.to_string()).collect::<Vec<_>>(),
            "mentionedGroupIds":[group.to_string()],"parentId":parent.id.to_string()});
        let event: (String, String, String) =
            sqlx::query_as("SELECT verb,channel,payload FROM events WHERE target_id=?1")
                .bind(reply.id.as_bytes().as_slice())
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert_eq!(
            (event.0.as_str(), event.1.as_str()),
            ("comment.created", "web")
        );
        assert_eq!(serde_json::from_str::<Value>(&event.2).unwrap(), expected);
        let audit: (String, String) =
            sqlx::query_as("SELECT payload,ip FROM audit_log WHERE target_id=?1")
                .bind(reply.id.as_bytes().as_slice())
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert_eq!(serde_json::from_str::<Value>(&audit.0).unwrap(), expected);
        assert_eq!(audit.1, "127.0.0.1");
        let baseline = effects(&f).await;
        sqlx::query("CREATE TRIGGER reject_comment_audit BEFORE INSERT ON audit_log WHEN NEW.verb='comment.created' BEGIN SELECT RAISE(ABORT,'comment audit failure'); END;").execute(&f.pool).await.unwrap();
        assert!(create_document_comment_backend(
            &f.backend,
            f.workspace,
            f.user,
            credential,
            f.document,
            input("retry literal", Some(parent.id)),
            None
        )
        .await
        .is_err());
        assert_eq!(effects(&f).await, baseline);
        let missing: i64 =
            sqlx::query_scalar("SELECT count(*) FROM comments WHERE body='retry literal'")
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert_eq!(missing, 0);
        sqlx::query("DROP TRIGGER reject_comment_audit")
            .execute(&f.pool)
            .await
            .unwrap();
        let retry = create_document_comment_backend(
            &f.backend,
            f.workspace,
            f.user,
            credential,
            f.document,
            input("retry literal", Some(parent.id)),
            None,
        )
        .await
        .unwrap()
        .unwrap();
        assert_ne!(retry.id, reply.id);
        assert_eq!(retry.parent_id, Some(parent.id));
        assert_eq!(
            effects(&f).await,
            (
                baseline.0 + 1,
                baseline.1 + 1,
                baseline.2 + 1,
                baseline.3 + 1
            )
        );
        let page = list_document_comments_backend(
            &f.backend,
            f.workspace,
            f.user,
            credential,
            f.document,
            CommentListQuery {
                limit: 100,
                cursor: None,
            },
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            page.items.iter().map(|row| row.id).collect::<Vec<_>>(),
            [parent.id, reply.id, retry.id]
        );
        assert_eq!(
            page.items
                .iter()
                .map(|row| row.body.as_str())
                .collect::<Vec<_>>(),
            ["부모 😀", "실제 댓글 中 😀", "retry literal"]
        );
        f.pool.close().await;
        std::fs::remove_dir_all(&f.root).unwrap();
    }

    #[tokio::test]
    async fn wiki_aux_mutation_comment_current_actor_tenant_parent_limits_denials_and_healthy_retry(
    ) {
        let f = Fixture::new().await;
        let credential = credential(&f).await;
        let parent = create_document_comment_backend(
            &f.backend,
            f.workspace,
            f.user,
            credential,
            f.document,
            input("Parent", None),
            None,
        )
        .await
        .unwrap()
        .unwrap();
        let other_doc = Uuid::now_v7();
        sqlx::query("INSERT INTO documents(id,workspace_id,title,path,sort_key,number,status,schema_version,created_by,content_json) VALUES(?1,?2,'Other wiki',?3,'V',2,'published',2,?4,'{}')")
            .bind(other_doc.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(other_doc.simple().to_string()).bind(f.user.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        let baseline = effects(&f).await;
        assert!(matches!(
            create_document_comment_backend(
                &f.backend,
                f.workspace,
                Uuid::now_v7(),
                credential,
                f.document,
                input("Wrong actor", None),
                None
            )
            .await
            .unwrap(),
            Err(CommentDbError::NotFound)
        ));
        assert!(matches!(
            create_document_comment_backend(
                &f.backend,
                Uuid::now_v7(),
                f.user,
                credential,
                f.document,
                input("Wrong tenant", None),
                None
            )
            .await
            .unwrap(),
            Err(CommentDbError::NotFound)
        ));
        assert!(matches!(
            create_document_comment_backend(
                &f.backend,
                f.workspace,
                f.user,
                credential,
                other_doc,
                input("Wrong reply scope", Some(parent.id)),
                None
            )
            .await
            .unwrap(),
            Err(CommentDbError::InvalidInput)
        ));
        assert!(matches!(
            create_document_comment_backend(
                &f.backend,
                f.workspace,
                f.user,
                credential,
                f.document,
                input("Missing parent", Some(Uuid::now_v7())),
                None
            )
            .await
            .unwrap(),
            Err(CommentDbError::NotFound)
        ));
        for body in [" ".to_string(), "😀".repeat(4001)] {
            assert!(matches!(
                create_document_comment_backend(
                    &f.backend,
                    f.workspace,
                    f.user,
                    credential,
                    f.document,
                    input(&body, None),
                    None
                )
                .await
                .unwrap(),
                Err(CommentDbError::InvalidInput)
            ));
        }
        let too_many = (1..=51).map(Uuid::from_u128).collect::<Vec<_>>();
        for (users, groups) in [
            (too_many.as_slice(), &[][..]),
            (&[][..], too_many.as_slice()),
        ] {
            assert!(matches!(
                create_document_comment_backend(
                    &f.backend,
                    f.workspace,
                    f.user,
                    credential,
                    f.document,
                    CreateCommentInput {
                        body: "Too many",
                        parent_id: None,
                        mentioned_user_ids: users,
                        mentioned_group_ids: groups
                    },
                    None
                )
                .await
                .unwrap(),
                Err(CommentDbError::InvalidInput)
            ));
        }
        sqlx::query("UPDATE sessions SET revoked_at=1 WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            create_document_comment_backend(
                &f.backend,
                f.workspace,
                f.user,
                credential,
                f.document,
                input("Revoked", None),
                None
            )
            .await
            .unwrap(),
            Err(CommentDbError::NotFound)
        ));
        sqlx::query("UPDATE sessions SET revoked_at=NULL WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE memberships SET role='guest' WHERE workspace_id=?1 AND user_id=?2")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let viewer_group = Uuid::now_v7();
        sqlx::query("INSERT INTO groups(id,workspace_id,name) VALUES(?1,?2,'Write test viewers')")
            .bind(viewer_group.as_bytes().as_slice())
            .bind(f.workspace.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO group_members(workspace_id,group_id,user_id) VALUES(?1,?2,?3)")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(viewer_group.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO document_members(id,workspace_id,document_id,group_id,role) VALUES(?1,?2,?3,?4,'viewer')")
            .bind(Uuid::now_v7().as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(f.document.as_bytes().as_slice()).bind(viewer_group.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        assert_eq!(
            list_document_comments_backend(
                &f.backend,
                f.workspace,
                f.user,
                credential,
                f.document,
                CommentListQuery {
                    limit: 100,
                    cursor: None
                }
            )
            .await
            .unwrap()
            .unwrap()
            .items[0]
                .id,
            parent.id
        );
        assert!(matches!(
            create_document_comment_backend(
                &f.backend,
                f.workspace,
                f.user,
                credential,
                f.document,
                input("Viewer", None),
                None
            )
            .await
            .unwrap(),
            Err(CommentDbError::NotFound)
        ));
        sqlx::query("UPDATE memberships SET role='owner' WHERE workspace_id=?1 AND user_id=?2")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE documents SET deleted_at=1 WHERE id=?1")
            .bind(f.document.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            create_document_comment_backend(
                &f.backend,
                f.workspace,
                f.user,
                credential,
                f.document,
                input("Trashed", None),
                None
            )
            .await
            .unwrap(),
            Err(CommentDbError::NotFound)
        ));
        sqlx::query("UPDATE documents SET deleted_at=NULL WHERE id=?1")
            .bind(f.document.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let project = Uuid::now_v7();
        sqlx::query("INSERT INTO projects(id,workspace_id,key,name,visibility,created_by) VALUES(?1,?2,'CMT','Comment project','workspace',?3)").bind(project.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        sqlx::query("UPDATE documents SET project_id=?1 WHERE id=?2")
            .bind(project.as_bytes().as_slice())
            .bind(f.document.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            create_document_comment_backend(
                &f.backend,
                f.workspace,
                f.user,
                credential,
                f.document,
                input("Moved", None),
                None
            )
            .await
            .unwrap(),
            Err(CommentDbError::NotFound)
        ));
        assert_eq!(effects(&f).await, baseline);
        sqlx::query("UPDATE documents SET project_id=NULL WHERE id=?1")
            .bind(f.document.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let retry = create_document_comment_backend(
            &f.backend,
            f.workspace,
            f.user,
            credential,
            f.document,
            input("Healthy after refusal", Some(parent.id)),
            None,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(retry.parent_id, Some(parent.id));
        assert_eq!(
            effects(&f).await,
            (
                baseline.0 + 1,
                baseline.1 + 1,
                baseline.2 + 1,
                baseline.3 + 1
            )
        );
        f.pool.close().await;
        std::fs::remove_dir_all(&f.root).unwrap();
    }

    #[tokio::test]
    async fn wiki_aux_mutation_waiting_writers_recheck_committed_revocation_without_effects() {
        let f = Fixture::new().await;
        let credential = credential(&f).await;
        let race_pool = crate::db::pool::connect_sqlite_app(&f.path, 3)
            .await
            .unwrap();
        let backend = Backend::Sqlite(race_pool.clone());
        let baseline = effects(&f).await;
        let mut revoker = backend.begin_write().await.unwrap();
        let (workspace, user, document) = (f.workspace, f.user, f.document);
        let (tag_started, tag_waiting) = tokio::sync::oneshot::channel();
        let tag_backend = backend.clone();
        let tag = tokio::spawn(async move {
            tag_started.send(()).unwrap();
            let actor = crate::db::collections::Actor {
                user_id: user,
                credential_id: credential,
                client_ip: None,
            };
            crate::db::document_tags::create_tag_backend(
                &tag_backend,
                workspace,
                &actor,
                "Queued before revocation",
                "blue",
            )
            .await
        });
        let (comment_started, comment_waiting) = tokio::sync::oneshot::channel();
        let comment_backend = backend.clone();
        let comment = tokio::spawn(async move {
            comment_started.send(()).unwrap();
            create_document_comment_backend(
                &comment_backend,
                workspace,
                user,
                credential,
                document,
                input("Queued before revocation", None),
                None,
            )
            .await
        });
        tag_waiting.await.unwrap();
        comment_waiting.await.unwrap();
        let OperationTx::SqliteFamily(family) = revoker.operation() else {
            unreachable!()
        };
        family
            .execute(
                "UPDATE sessions SET revoked_at=1 WHERE id=?1",
                &[Cell::uuid(credential)],
            )
            .await
            .unwrap();
        revoker.commit().await.unwrap();
        assert!(matches!(
            tag.await.unwrap().unwrap(),
            Err(crate::db::document_tags::TagDbError::NotFound)
        ));
        assert!(matches!(
            comment.await.unwrap().unwrap(),
            Err(CommentDbError::NotFound)
        ));
        assert_eq!(effects(&f).await, baseline);
        let tags: i64 = sqlx::query_scalar("SELECT count(*) FROM document_tags")
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(tags, 0);
        sqlx::query("UPDATE sessions SET revoked_at=NULL WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            create_document_comment_backend(
                &f.backend,
                workspace,
                user,
                credential,
                document,
                input("Healthy after waiting", None),
                None
            )
            .await
            .unwrap()
            .unwrap()
            .body,
            "Healthy after waiting"
        );
        race_pool.close().await;
        f.pool.close().await;
        std::fs::remove_dir_all(&f.root).unwrap();
    }
}
