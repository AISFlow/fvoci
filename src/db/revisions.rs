use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::db::context::{session_is_live, set_tenant};
use crate::db::documents::workspace_is_live;
use crate::db::identity::{append_event, EventAppend};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevisionDbError {
    NotFound,
    Forbidden,
    /// Task revision write on an archived task (409 `task_archived`).
    TaskArchived,
    /// Task revision write in an archived project (409 `project_archived`).
    ProjectArchived,
}

/// Revision owner (`revisions.target_kind` / `target_id`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevisionTarget {
    Document(Uuid),
    Task(Uuid),
}

impl RevisionTarget {
    pub fn kind_str(self) -> &'static str {
        match self {
            Self::Document(_) => TARGET_DOCUMENT,
            Self::Task(_) => TARGET_TASK,
        }
    }

    pub fn id(self) -> Uuid {
        match self {
            Self::Document(id) | Self::Task(id) => id,
        }
    }

    fn matches(self, target_kind: &str, target_id: Uuid) -> bool {
        target_kind == self.kind_str() && target_id == self.id()
    }
}

#[derive(Debug, Clone)]
pub struct RevisionMeta {
    pub id: Uuid,
    pub target_kind: String,
    pub target_id: Uuid,
    pub reason: String,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct RevisionDetail {
    pub meta: RevisionMeta,
    pub content_json: Value,
    pub y_snapshot: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct RevisionListPage {
    pub items: Vec<RevisionMeta>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CreateRevisionInput {
    pub y_snapshot: Vec<u8>,
    pub content_json: Value,
    pub text: String,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct PersistedCollabSource {
    pub snapshot: Vec<u8>,
    pub tail: Vec<Vec<u8>>,
}

#[derive(Debug, Clone, Copy)]
pub struct RevisionCursor {
    pub created_at: DateTime<Utc>,
    pub id: Uuid,
}

const MANUAL_REASON: &str = "manual";
const TARGET_DOCUMENT: &str = "document";
const TARGET_TASK: &str = "task";

type RevisionMetaRow = (Uuid, String, Uuid, String, Option<Uuid>, DateTime<Utc>);
type RevisionDetailRow = (
    Uuid,
    String,
    Uuid,
    String,
    Option<Uuid>,
    DateTime<Utc>,
    Value,
    Vec<u8>,
);

pub fn encode_revision_cursor(cursor: RevisionCursor) -> String {
    use base64::Engine;
    let payload = serde_json::json!({
        "ca": cursor.created_at.to_rfc3339(),
        "id": cursor.id.to_string(),
    });
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.to_string())
}

pub fn decode_revision_cursor(raw: &str) -> Option<RevisionCursor> {
    use base64::Engine;
    if raw.len() > 1024 {
        return None;
    }
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(raw)
        .ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    let object = value.as_object()?;
    if object.keys().any(|key| key != "ca" && key != "id") {
        return None;
    }
    let created_at = DateTime::parse_from_rfc3339(object.get("ca")?.as_str()?)
        .ok()?
        .with_timezone(&Utc);
    let id = Uuid::parse_str(object.get("id")?.as_str()?).ok()?;
    Some(RevisionCursor { created_at, id })
}

async fn authorize_document(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
    write: bool,
) -> Result<Result<(), RevisionDbError>, sqlx::Error> {
    if !session_is_live(&mut *tx, actor_user_id, session_id).await? {
        return Ok(Err(RevisionDbError::Forbidden));
    }
    if !workspace_is_live(&mut *tx, workspace_id).await? {
        return Ok(Err(RevisionDbError::NotFound));
    }
    let min = if write {
        crate::projects::ProjectPermission::Edit
    } else {
        crate::projects::ProjectPermission::View
    };
    let permission = crate::db::documents::document_permission(
        tx,
        workspace_id,
        actor_user_id,
        document_id,
        true,
    )
    .await?;
    if !permission.at_least(min) {
        return Ok(Err(RevisionDbError::NotFound));
    }
    Ok(Ok(()))
}

/// Task revision access: a live task in a live project the caller can view
/// (read) or edit (write). Writes also refuse an archived project or task
/// (source `assertTaskWritable`). The project row is share-locked.
async fn authorize_task(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    task_id: Uuid,
    write: bool,
) -> Result<Result<(), RevisionDbError>, sqlx::Error> {
    if !session_is_live(&mut *tx, actor_user_id, session_id).await? {
        return Ok(Err(RevisionDbError::Forbidden));
    }
    if !workspace_is_live(&mut *tx, workspace_id).await? {
        return Ok(Err(RevisionDbError::NotFound));
    }
    let task: Option<(Uuid, Option<DateTime<Utc>>)> = sqlx::query_as(
        r#"
        SELECT project_id, archived_at
        FROM fvoci.tasks
        WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL
        "#,
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some((project_id, archived_at)) = task else {
        return Ok(Err(RevisionDbError::NotFound));
    };
    let Some((permission, project_archived)) = crate::db::projects::share_lock_project_permission(
        tx,
        workspace_id,
        actor_user_id,
        project_id,
    )
    .await?
    else {
        return Ok(Err(RevisionDbError::NotFound));
    };
    let min = if write {
        crate::projects::ProjectPermission::Edit
    } else {
        crate::projects::ProjectPermission::View
    };
    if !permission.at_least(min) {
        return Ok(Err(RevisionDbError::NotFound));
    }
    if write && project_archived {
        return Ok(Err(RevisionDbError::ProjectArchived));
    }
    if write && archived_at.is_some() {
        return Ok(Err(RevisionDbError::TaskArchived));
    }
    Ok(Ok(()))
}

async fn authorize_target(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    target: RevisionTarget,
    write: bool,
) -> Result<Result<(), RevisionDbError>, sqlx::Error> {
    match target {
        RevisionTarget::Document(id) => {
            authorize_document(tx, workspace_id, actor_user_id, session_id, id, write).await
        }
        RevisionTarget::Task(id) => {
            authorize_task(tx, workspace_id, actor_user_id, session_id, id, write).await
        }
    }
}

/// Revision access check for a document or task target without other work.
pub async fn authorize_revision_target(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    target: RevisionTarget,
    write: bool,
) -> Result<Result<(), RevisionDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let result = authorize_target(
        &mut tx,
        workspace_id,
        actor_user_id,
        session_id,
        target,
        write,
    )
    .await?;
    match result {
        Ok(()) => {
            tx.commit().await?;
            Ok(Ok(()))
        }
        Err(err) => {
            tx.rollback().await?;
            Ok(Err(err))
        }
    }
}

pub async fn authorize_revision_document(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
    write: bool,
) -> Result<Result<(), RevisionDbError>, sqlx::Error> {
    authorize_revision_target(
        pool,
        workspace_id,
        actor_user_id,
        session_id,
        RevisionTarget::Document(document_id),
        write,
    )
    .await
}

async fn collab_state_exists(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    target: RevisionTarget,
) -> Result<bool, sqlx::Error> {
    let sql = match target {
        RevisionTarget::Document(_) => {
            "SELECT EXISTS (SELECT 1 FROM fvoci.document_states WHERE workspace_id = $1 AND document_id = $2)"
        }
        RevisionTarget::Task(_) => {
            "SELECT EXISTS (SELECT 1 FROM fvoci.task_states WHERE workspace_id = $1 AND task_id = $2)"
        }
    };
    let exists: bool = sqlx::query_scalar(sql)
        .bind(workspace_id)
        .bind(target.id())
        .fetch_one(&mut **tx)
        .await?;
    Ok(exists)
}

pub async fn list_document_revisions(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
    limit: i64,
    before: Option<RevisionCursor>,
) -> Result<Result<RevisionListPage, RevisionDbError>, sqlx::Error> {
    list_revisions(
        pool,
        workspace_id,
        actor_user_id,
        session_id,
        RevisionTarget::Document(document_id),
        limit,
        before,
    )
    .await
}

pub async fn list_revisions(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    target: RevisionTarget,
    limit: i64,
    before: Option<RevisionCursor>,
) -> Result<Result<RevisionListPage, RevisionDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    match authorize_target(
        &mut tx,
        workspace_id,
        actor_user_id,
        session_id,
        target,
        false,
    )
    .await?
    {
        Ok(()) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    }
    let fetch_limit = limit.saturating_add(1);
    let rows: Vec<RevisionMetaRow> = match before {
        Some(cursor) => {
            sqlx::query_as(
                r#"
                SELECT id, target_kind, target_id, reason, created_by, created_at
                FROM fvoci.revisions
                WHERE workspace_id = $1
                  AND target_kind = $2
                  AND target_id = $3
                  AND (created_at, id) < ($4, $5)
                ORDER BY created_at DESC, id DESC
                LIMIT $6
                "#,
            )
            .bind(workspace_id)
            .bind(target.kind_str())
            .bind(target.id())
            .bind(cursor.created_at)
            .bind(cursor.id)
            .bind(fetch_limit)
            .fetch_all(&mut *tx)
            .await?
        }
        None => {
            sqlx::query_as(
                r#"
                SELECT id, target_kind, target_id, reason, created_by, created_at
                FROM fvoci.revisions
                WHERE workspace_id = $1
                  AND target_kind = $2
                  AND target_id = $3
                ORDER BY created_at DESC, id DESC
                LIMIT $4
                "#,
            )
            .bind(workspace_id)
            .bind(target.kind_str())
            .bind(target.id())
            .bind(fetch_limit)
            .fetch_all(&mut *tx)
            .await?
        }
    };
    tx.commit().await?;
    let mut items: Vec<RevisionMeta> = rows
        .into_iter()
        .map(
            |(id, target_kind, target_id, reason, created_by, created_at)| RevisionMeta {
                id,
                target_kind,
                target_id,
                reason,
                created_by,
                created_at,
            },
        )
        .collect();
    let next_cursor = if items.len() as i64 > limit {
        items.pop();
        items.last().map(|last| {
            encode_revision_cursor(RevisionCursor {
                created_at: last.created_at,
                id: last.id,
            })
        })
    } else {
        None
    };
    Ok(Ok(RevisionListPage { items, next_cursor }))
}

pub async fn get_document_revision(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
    revision_id: Uuid,
) -> Result<Result<RevisionDetail, RevisionDbError>, sqlx::Error> {
    get_revision(
        pool,
        workspace_id,
        actor_user_id,
        session_id,
        RevisionTarget::Document(document_id),
        revision_id,
    )
    .await
}

pub async fn get_revision(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    target: RevisionTarget,
    revision_id: Uuid,
) -> Result<Result<RevisionDetail, RevisionDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    match authorize_target(
        &mut tx,
        workspace_id,
        actor_user_id,
        session_id,
        target,
        false,
    )
    .await?
    {
        Ok(()) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    }
    let row: Option<RevisionDetailRow> = sqlx::query_as(
        r#"
        SELECT id, target_kind, target_id, reason, created_by, created_at, content_json, y_snapshot
        FROM fvoci.revisions
        WHERE workspace_id = $1 AND id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(revision_id)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    match row {
        Some((
            id,
            target_kind,
            target_id,
            reason,
            created_by,
            created_at,
            content_json,
            y_snapshot,
        )) if target.matches(&target_kind, target_id) => Ok(Ok(RevisionDetail {
            meta: RevisionMeta {
                id,
                target_kind,
                target_id,
                reason,
                created_by,
                created_at,
            },
            content_json,
            y_snapshot,
        })),
        _ => Ok(Err(RevisionDbError::NotFound)),
    }
}

pub async fn create_manual_document_revision(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
    input: CreateRevisionInput,
) -> Result<Result<Uuid, RevisionDbError>, sqlx::Error> {
    create_manual_revision(
        pool,
        workspace_id,
        actor_user_id,
        session_id,
        RevisionTarget::Document(document_id),
        input,
    )
    .await
}

pub async fn create_manual_revision(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    target: RevisionTarget,
    input: CreateRevisionInput,
) -> Result<Result<Uuid, RevisionDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    match authorize_target(
        &mut tx,
        workspace_id,
        actor_user_id,
        session_id,
        target,
        true,
    )
    .await?
    {
        Ok(()) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    }
    let recent: Option<(Uuid, Vec<u8>)> = sqlx::query_as(
        r#"
        SELECT id, y_snapshot
        FROM fvoci.revisions
        WHERE workspace_id = $1 AND target_kind = $2 AND target_id = $3
        ORDER BY created_at DESC, id DESC
        LIMIT 1
        "#,
    )
    .bind(workspace_id)
    .bind(target.kind_str())
    .bind(target.id())
    .fetch_optional(&mut *tx)
    .await?;
    if let Some((id, prev_snap)) = recent {
        if prev_snap == input.y_snapshot {
            tx.commit().await?;
            return Ok(Ok(id));
        }
    }
    let id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO fvoci.revisions (
            id, workspace_id, target_kind, target_id, y_snapshot, encoding,
            content_json, text, reason, created_by
        ) VALUES ($1, $2, $3, $4, $5, 1, $6, $7, $8, $9)
        "#,
    )
    .bind(id)
    .bind(workspace_id)
    .bind(target.kind_str())
    .bind(target.id())
    .bind(&input.y_snapshot)
    .bind(&input.content_json)
    .bind(&input.text)
    .bind(if input.reason.is_empty() {
        MANUAL_REASON
    } else {
        input.reason.as_str()
    })
    .bind(actor_user_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(id))
}

pub async fn resolve_document_restore(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
    revision_id: Uuid,
    client_ip: Option<&str>,
) -> Result<Result<Vec<u8>, RevisionDbError>, sqlx::Error> {
    resolve_restore(
        pool,
        workspace_id,
        actor_user_id,
        session_id,
        RevisionTarget::Document(document_id),
        revision_id,
        client_ip,
    )
    .await
}

pub async fn resolve_restore(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    target: RevisionTarget,
    revision_id: Uuid,
    client_ip: Option<&str>,
) -> Result<Result<Vec<u8>, RevisionDbError>, sqlx::Error> {
    let _ = client_ip;
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    match authorize_target(
        &mut tx,
        workspace_id,
        actor_user_id,
        session_id,
        target,
        true,
    )
    .await?
    {
        Ok(()) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    }
    if !collab_state_exists(&mut tx, workspace_id, target).await? {
        tx.rollback().await?;
        return Ok(Err(RevisionDbError::NotFound));
    }
    let row: Option<(String, Uuid, Vec<u8>)> = sqlx::query_as(
        r#"
        SELECT target_kind, target_id, y_snapshot
        FROM fvoci.revisions
        WHERE workspace_id = $1 AND id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(revision_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((target_kind, target_id, y_snapshot)) = row else {
        tx.rollback().await?;
        return Ok(Err(RevisionDbError::NotFound));
    };
    if !target.matches(&target_kind, target_id) {
        tx.rollback().await?;
        return Ok(Err(RevisionDbError::NotFound));
    }
    append_event(
        &mut tx,
        EventAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(workspace_id),
            actor_user_id: Some(actor_user_id),
            verb: format!("{}.updated", target.kind_str()),
            target_type: Some(target.kind_str().into()),
            target_id: Some(target.id()),
            payload: match target {
                RevisionTarget::Document(id) => json!({
                    "documentId": id,
                    "restoreRequested": revision_id,
                }),
                RevisionTarget::Task(id) => json!({
                    "taskId": id,
                    "restoreRequested": revision_id,
                }),
            },
        },
    )
    .await?;
    tx.commit().await?;
    Ok(Ok(y_snapshot))
}

pub async fn load_persisted_collab_source(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
) -> Result<Result<PersistedCollabSource, RevisionDbError>, sqlx::Error> {
    load_persisted_target_source(
        pool,
        workspace_id,
        actor_user_id,
        session_id,
        RevisionTarget::Document(document_id),
    )
    .await
}

pub async fn load_persisted_target_source(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    target: RevisionTarget,
) -> Result<Result<PersistedCollabSource, RevisionDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    match authorize_target(
        &mut tx,
        workspace_id,
        actor_user_id,
        session_id,
        target,
        true,
    )
    .await?
    {
        Ok(()) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    }
    let (state_sql, tail_sql) = match target {
        RevisionTarget::Document(_) => (
            "SELECT state, encoding, snapshot_cutoff_seq FROM fvoci.document_states WHERE workspace_id = $1 AND document_id = $2",
            "SELECT payload FROM fvoci.document_collab_updates WHERE workspace_id = $1 AND document_id = $2 AND seq > $3 ORDER BY seq ASC",
        ),
        RevisionTarget::Task(_) => (
            "SELECT state, encoding, snapshot_cutoff_seq FROM fvoci.task_states WHERE workspace_id = $1 AND task_id = $2",
            "SELECT payload FROM fvoci.task_collab_updates WHERE workspace_id = $1 AND task_id = $2 AND seq > $3 ORDER BY seq ASC",
        ),
    };
    let state: Option<(Vec<u8>, i16, i64)> = sqlx::query_as(state_sql)
        .bind(workspace_id)
        .bind(target.id())
        .fetch_optional(&mut *tx)
        .await?;
    let Some((snapshot, encoding, cutoff)) = state else {
        tx.rollback().await?;
        return Ok(Err(RevisionDbError::NotFound));
    };
    if encoding != 1 {
        tx.rollback().await?;
        return Ok(Err(RevisionDbError::NotFound));
    }
    let tail: Vec<(Vec<u8>,)> = sqlx::query_as(tail_sql)
        .bind(workspace_id)
        .bind(target.id())
        .bind(cutoff)
        .fetch_all(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Ok(PersistedCollabSource {
        snapshot,
        tail: tail.into_iter().map(|(payload,)| payload).collect(),
    }))
}
