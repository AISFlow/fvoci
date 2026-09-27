use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::db::context::{session_is_live, set_system, set_tenant};
use crate::db::documents::workspace_is_live;
use crate::db::identity::{append_event, EventAppend};

const SESSION_REVISION_HEAD_RETRIES: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevisionDbError {
    NotFound,
    /// Observed revision head changed between compare and INSERT.
    StaleRevisionHead,
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

/// Observed revision head before semantic compare; re-checked under row locks at INSERT.
#[derive(Debug, Clone)]
pub struct SystemRevisionHead {
    pub latest_revision_id: Option<Uuid>,
    pub latest_y_snapshot: Option<Vec<u8>>,
}

impl SystemRevisionHead {
    pub fn from_latest(row: Option<(Uuid, Vec<u8>)>) -> Self {
        match row {
            None => Self {
                latest_revision_id: None,
                latest_y_snapshot: None,
            },
            Some((id, snap)) => Self {
                latest_revision_id: Some(id),
                latest_y_snapshot: Some(snap),
            },
        }
    }

    fn matches_current(&self, current: Option<(Uuid, Vec<u8>)>) -> bool {
        match (self.latest_revision_id, current) {
            (None, None) => true,
            (Some(expected_id), Some((id, snap))) => {
                expected_id == id
                    && self
                        .latest_y_snapshot
                        .as_deref()
                        .is_some_and(|fenced| fenced == snap.as_slice())
            }
            _ => false,
        }
    }
}

pub const SYSTEM_REVISION_HEAD_RETRIES: u32 = SESSION_REVISION_HEAD_RETRIES;

#[derive(Debug, Clone)]
pub struct PersistedCollabSource {
    pub snapshot: Vec<u8>,
    pub tail: Vec<Vec<u8>>,
}

/// Durable collab snapshot + tail from DB (session revision capture; no user gate).
#[derive(Debug, Clone)]
pub struct DurableCollabSnapshot {
    pub snapshot: Vec<u8>,
    pub tail: Vec<Vec<u8>>,
    pub tail_seq: i64,
    pub snapshot_cutoff_seq: i64,
}

/// Load persisted collab bytes for automatic session snapshots (tenant-scoped, live target only).
pub async fn load_durable_collab_for_system(
    pool: &PgPool,
    workspace_id: Uuid,
    target: RevisionTarget,
) -> Result<Result<DurableCollabSnapshot, RevisionDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(RevisionDbError::NotFound));
    }
    let (state_sql, tail_sql) = match target {
        RevisionTarget::Document(_) => (
            "SELECT state, encoding, snapshot_cutoff_seq, tail_seq FROM fvoci.document_states WHERE workspace_id = $1 AND document_id = $2",
            "SELECT payload FROM fvoci.document_collab_updates WHERE workspace_id = $1 AND document_id = $2 AND seq > $3 ORDER BY seq ASC",
        ),
        RevisionTarget::Task(_) => (
            "SELECT state, encoding, snapshot_cutoff_seq, tail_seq FROM fvoci.task_states WHERE workspace_id = $1 AND task_id = $2",
            "SELECT payload FROM fvoci.task_collab_updates WHERE workspace_id = $1 AND task_id = $2 AND seq > $3 ORDER BY seq ASC",
        ),
    };
    let state: Option<(Vec<u8>, i16, i64, i64)> = sqlx::query_as(state_sql)
        .bind(workspace_id)
        .bind(target.id())
        .fetch_optional(&mut *tx)
        .await?;
    let Some((snapshot, encoding, snapshot_cutoff_seq, tail_seq)) = state else {
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
        .bind(snapshot_cutoff_seq)
        .fetch_all(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Ok(DurableCollabSnapshot {
        snapshot,
        tail: tail.into_iter().map(|(payload,)| payload).collect(),
        tail_seq,
        snapshot_cutoff_seq,
    }))
}

#[derive(Debug, Clone, Copy)]
pub struct RevisionCursor {
    pub created_at: DateTime<Utc>,
    pub id: Uuid,
}

const MANUAL_REASON: &str = "manual";
const SESSION_REASON: &str = "session";
pub const SCHEDULED_REASON: &str = "scheduled";
const TARGET_DOCUMENT: &str = "document";
const TARGET_TASK: &str = "task";

type TaskRevisionLockRow = (Uuid, Option<DateTime<Utc>>);

fn is_automatic_revision_reason(reason: &str) -> bool {
    reason == SESSION_REASON || reason == SCHEDULED_REASON
}

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
    let recent: Option<(Uuid, Vec<u8>, String)> = sqlx::query_as(
        r#"
        SELECT id, y_snapshot, reason
        FROM fvoci.revisions
        WHERE workspace_id = $1 AND target_kind = $2 AND target_id = $3
        ORDER BY created_at DESC, id DESC
        LIMIT 1
        FOR UPDATE
        "#,
    )
    .bind(workspace_id)
    .bind(target.kind_str())
    .bind(target.id())
    .fetch_optional(&mut *tx)
    .await?;
    if let Some((id, prev_snap, reason)) = recent {
        if prev_snap == input.y_snapshot {
            if is_automatic_revision_reason(&reason) {
                sqlx::query(
                    r#"
                    UPDATE fvoci.revisions
                    SET reason = $3, created_by = $4
                    WHERE workspace_id = $1 AND id = $2
                    "#,
                )
                .bind(workspace_id)
                .bind(id)
                .bind(MANUAL_REASON)
                .bind(actor_user_id)
                .execute(&mut *tx)
                .await?;
            }
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

async fn lock_system_revision_target(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    target: RevisionTarget,
    expected_writer_generation: Option<i64>,
) -> Result<Result<(), RevisionDbError>, sqlx::Error> {
    if !workspace_is_live(&mut *tx, workspace_id).await? {
        return Ok(Err(RevisionDbError::NotFound));
    }
    let writer_generation = match target {
        RevisionTarget::Document(document_id) => {
            let affiliation: Option<(Option<Uuid>,)> = sqlx::query_as(
                "SELECT project_id FROM fvoci.documents WHERE workspace_id = $1 AND id = $2",
            )
            .bind(workspace_id)
            .bind(document_id)
            .fetch_optional(&mut **tx)
            .await?;
            let Some((expected_project_id,)) = affiliation else {
                return Ok(Err(RevisionDbError::NotFound));
            };
            if let Some(project_id) = expected_project_id {
                let project: Option<(Uuid,)> = sqlx::query_as(
                    r#"
                    SELECT id FROM fvoci.projects
                    WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL
                    FOR SHARE
                    "#,
                )
                .bind(workspace_id)
                .bind(project_id)
                .fetch_optional(&mut **tx)
                .await?;
                if project.is_none() {
                    return Ok(Err(RevisionDbError::NotFound));
                }
            }
            let row: Option<(Option<Uuid>, Option<DateTime<Utc>>)> = sqlx::query_as(
                r#"
                SELECT project_id, deleted_at
                FROM fvoci.documents
                WHERE workspace_id = $1 AND id = $2
                FOR UPDATE
                "#,
            )
            .bind(workspace_id)
            .bind(document_id)
            .fetch_optional(&mut **tx)
            .await?;
            let Some((project_id, deleted_at)) = row else {
                return Ok(Err(RevisionDbError::NotFound));
            };
            if deleted_at.is_some() || project_id != expected_project_id {
                return Ok(Err(RevisionDbError::NotFound));
            }
            let state: Option<(i64,)> = sqlx::query_as(
                r#"
                SELECT writer_generation
                FROM fvoci.document_states
                WHERE workspace_id = $1 AND document_id = $2
                FOR UPDATE
                "#,
            )
            .bind(workspace_id)
            .bind(document_id)
            .fetch_optional(&mut **tx)
            .await?;
            state.map(|(g,)| g)
        }
        RevisionTarget::Task(task_id) => {
            let expected: Option<(Uuid,)> = sqlx::query_as(
                r#"
                SELECT project_id FROM fvoci.tasks
                WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL
                "#,
            )
            .bind(workspace_id)
            .bind(task_id)
            .fetch_optional(&mut **tx)
            .await?;
            let Some((expected_project_id,)) = expected else {
                return Ok(Err(RevisionDbError::NotFound));
            };
            let project: Option<(Uuid,)> = sqlx::query_as(
                r#"
                SELECT id FROM fvoci.projects
                WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL
                FOR SHARE
                "#,
            )
            .bind(workspace_id)
            .bind(expected_project_id)
            .fetch_optional(&mut **tx)
            .await?;
            if project.is_none() {
                return Ok(Err(RevisionDbError::NotFound));
            }
            let row: Option<TaskRevisionLockRow> = sqlx::query_as(
                r#"
                SELECT project_id, deleted_at
                FROM fvoci.tasks
                WHERE workspace_id = $1 AND id = $2
                FOR NO KEY UPDATE
                "#,
            )
            .bind(workspace_id)
            .bind(task_id)
            .fetch_optional(&mut **tx)
            .await?;
            let Some((project_id, deleted_at)) = row else {
                return Ok(Err(RevisionDbError::NotFound));
            };
            if deleted_at.is_some() || project_id != expected_project_id {
                return Ok(Err(RevisionDbError::NotFound));
            }
            let state: Option<(i64,)> = sqlx::query_as(
                r#"
                SELECT writer_generation
                FROM fvoci.task_states
                WHERE workspace_id = $1 AND task_id = $2
                FOR UPDATE
                "#,
            )
            .bind(workspace_id)
            .bind(task_id)
            .fetch_optional(&mut **tx)
            .await?;
            state.map(|(g,)| g)
        }
    };
    let Some(writer_generation) = writer_generation else {
        return Ok(Err(RevisionDbError::NotFound));
    };
    if let Some(expected) = expected_writer_generation {
        if writer_generation != expected {
            return Ok(Err(RevisionDbError::NotFound));
        }
    }
    Ok(Ok(()))
}

async fn latest_revision_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    target: RevisionTarget,
) -> Result<Option<(Uuid, Vec<u8>)>, sqlx::Error> {
    sqlx::query_as(
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
    .fetch_optional(&mut **tx)
    .await
}

/// System-authored revision (`created_by` null). No user permission gate.
pub async fn latest_revision_y_snapshot(
    pool: &PgPool,
    workspace_id: Uuid,
    target: RevisionTarget,
) -> Result<Option<(Uuid, Vec<u8>)>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let row: Option<(Uuid, Vec<u8>)> = sqlx::query_as(
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
    tx.commit().await?;
    Ok(row)
}

pub async fn create_system_revision(
    pool: &PgPool,
    workspace_id: Uuid,
    target: RevisionTarget,
    input: CreateRevisionInput,
    expected_writer_generation: i64,
    head_fence: SystemRevisionHead,
) -> Result<Result<Uuid, RevisionDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    match lock_system_revision_target(
        &mut tx,
        workspace_id,
        target,
        Some(expected_writer_generation),
    )
    .await?
    {
        Ok(()) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    }
    let recent = latest_revision_in_tx(&mut tx, workspace_id, target).await?;
    if !head_fence.matches_current(recent.clone()) {
        tx.rollback().await?;
        return Ok(Err(RevisionDbError::StaleRevisionHead));
    }
    if let Some((id, prev_snap)) = recent {
        if prev_snap == input.y_snapshot {
            tx.commit().await?;
            return Ok(Ok(id));
        }
    }
    let id = Uuid::now_v7();
    let reason = if input.reason.is_empty() {
        SESSION_REASON
    } else {
        input.reason.as_str()
    };
    sqlx::query(
        r#"
        INSERT INTO fvoci.revisions (
            id, workspace_id, target_kind, target_id, y_snapshot, encoding,
            content_json, text, reason, created_by
        ) VALUES ($1, $2, $3, $4, $5, 1, $6, $7, $8, NULL)
        "#,
    )
    .bind(id)
    .bind(workspace_id)
    .bind(target.kind_str())
    .bind(target.id())
    .bind(&input.y_snapshot)
    .bind(&input.content_json)
    .bind(&input.text)
    .bind(reason)
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScheduledRevisionCursor {
    pub workspace_id: Uuid,
    /// `0` = document, `1` = task.
    pub target_kind: u8,
    pub target_id: Uuid,
}

#[derive(Debug, Clone)]
pub struct ScheduledRevisionCandidate {
    pub workspace_id: Uuid,
    pub target: RevisionTarget,
    pub writer_generation: i64,
    pub state_updated_at: DateTime<Utc>,
    pub anchor_at: DateTime<Utc>,
}

type ScheduledRevisionListingRow = (
    Uuid,
    DateTime<Utc>,
    i64,
    DateTime<Utc>,
    Option<DateTime<Utc>>,
);

pub async fn list_live_workspace_ids_batch(
    pool: &PgPool,
    after: Option<Uuid>,
    inclusive_after: bool,
    limit: i64,
) -> Result<Vec<Uuid>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_system(&mut tx).await?;
    let rows: Vec<(Uuid,)> = if let Some(after) = after {
        if inclusive_after {
            sqlx::query_as(
                r#"
                SELECT id
                FROM fvoci.workspaces
                WHERE deleted_at IS NULL AND id >= $1
                ORDER BY id ASC
                LIMIT $2
                "#,
            )
            .bind(after)
            .bind(limit)
            .fetch_all(&mut *tx)
            .await?
        } else {
            sqlx::query_as(
                r#"
                SELECT id
                FROM fvoci.workspaces
                WHERE deleted_at IS NULL AND id > $1
                ORDER BY id ASC
                LIMIT $2
                "#,
            )
            .bind(after)
            .bind(limit)
            .fetch_all(&mut *tx)
            .await?
        }
    } else {
        sqlx::query_as(
            r#"
            SELECT id
            FROM fvoci.workspaces
            WHERE deleted_at IS NULL
            ORDER BY id ASC
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(&mut *tx)
        .await?
    };
    tx.commit().await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Collab targets in one workspace that may need a scheduled snapshot (predicate
/// applied in Rust: `anchor_at < cutoff` and `state_updated_at > anchor_at`).
pub async fn list_scheduled_revision_candidates_for_workspace(
    pool: &PgPool,
    workspace_id: Uuid,
    after: Option<ScheduledRevisionCursor>,
    limit: i64,
) -> Result<Vec<ScheduledRevisionCandidate>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let mut out = Vec::new();
    let doc_after = match after {
        None => None,
        Some(ScheduledRevisionCursor {
            target_kind: 0,
            target_id,
            ..
        }) => Some(target_id),
        Some(ScheduledRevisionCursor { .. }) => None,
    };
    let doc_rows: Vec<ScheduledRevisionListingRow> = if after.is_none()
        || after.is_some_and(|c| c.target_kind == 0)
    {
        let sql = if doc_after.is_some() {
            r#"
                SELECT ds.document_id, ds.updated_at, ds.writer_generation, d.created_at, lr.created_at
                FROM fvoci.document_states ds
                INNER JOIN fvoci.documents d
                    ON d.workspace_id = ds.workspace_id
                    AND d.id = ds.document_id
                    AND d.deleted_at IS NULL
                LEFT JOIN LATERAL (
                    SELECT r.created_at
                    FROM fvoci.revisions r
                    WHERE r.workspace_id = ds.workspace_id
                        AND r.target_kind = 'document'
                        AND r.target_id = ds.document_id
                    ORDER BY r.created_at DESC, r.id DESC
                    LIMIT 1
                ) lr ON TRUE
                WHERE ds.workspace_id = $1 AND ds.document_id > $2
                ORDER BY ds.document_id ASC
                LIMIT $3
                "#
        } else {
            r#"
                SELECT ds.document_id, ds.updated_at, ds.writer_generation, d.created_at, lr.created_at
                FROM fvoci.document_states ds
                INNER JOIN fvoci.documents d
                    ON d.workspace_id = ds.workspace_id
                    AND d.id = ds.document_id
                    AND d.deleted_at IS NULL
                LEFT JOIN LATERAL (
                    SELECT r.created_at
                    FROM fvoci.revisions r
                    WHERE r.workspace_id = ds.workspace_id
                        AND r.target_kind = 'document'
                        AND r.target_id = ds.document_id
                    ORDER BY r.created_at DESC, r.id DESC
                    LIMIT 1
                ) lr ON TRUE
                WHERE ds.workspace_id = $1
                ORDER BY ds.document_id ASC
                LIMIT $2
                "#
        };
        if let Some(after_id) = doc_after {
            sqlx::query_as(sql)
                .bind(workspace_id)
                .bind(after_id)
                .bind(limit)
                .fetch_all(&mut *tx)
                .await?
        } else {
            sqlx::query_as(sql)
                .bind(workspace_id)
                .bind(limit)
                .fetch_all(&mut *tx)
                .await?
        }
    } else {
        Vec::new()
    };
    for (document_id, state_updated_at, writer_generation, created_at, last_rev_at) in doc_rows {
        let anchor_at = last_rev_at.unwrap_or(created_at);
        out.push(ScheduledRevisionCandidate {
            workspace_id,
            target: RevisionTarget::Document(document_id),
            writer_generation,
            state_updated_at,
            anchor_at,
        });
        if out.len() as i64 >= limit {
            tx.commit().await?;
            return Ok(out);
        }
    }

    let remaining = limit - out.len() as i64;
    if remaining > 0 {
        let task_after = match after {
            Some(ScheduledRevisionCursor {
                target_kind: 1,
                target_id,
                ..
            }) => Some(target_id),
            _ => None,
        };
        let task_sql = if task_after.is_some() {
            r#"
            SELECT ts.task_id, ts.updated_at, ts.writer_generation, t.created_at, lr.created_at
            FROM fvoci.task_states ts
            INNER JOIN fvoci.tasks t
                ON t.workspace_id = ts.workspace_id
                AND t.id = ts.task_id
                AND t.deleted_at IS NULL
            LEFT JOIN LATERAL (
                SELECT r.created_at
                FROM fvoci.revisions r
                WHERE r.workspace_id = ts.workspace_id
                    AND r.target_kind = 'task'
                    AND r.target_id = ts.task_id
                ORDER BY r.created_at DESC, r.id DESC
                LIMIT 1
            ) lr ON TRUE
            WHERE ts.workspace_id = $1 AND ts.task_id > $2
            ORDER BY ts.task_id ASC
            LIMIT $3
            "#
        } else {
            r#"
            SELECT ts.task_id, ts.updated_at, ts.writer_generation, t.created_at, lr.created_at
            FROM fvoci.task_states ts
            INNER JOIN fvoci.tasks t
                ON t.workspace_id = ts.workspace_id
                AND t.id = ts.task_id
                AND t.deleted_at IS NULL
            LEFT JOIN LATERAL (
                SELECT r.created_at
                FROM fvoci.revisions r
                WHERE r.workspace_id = ts.workspace_id
                    AND r.target_kind = 'task'
                    AND r.target_id = ts.task_id
                ORDER BY r.created_at DESC, r.id DESC
                LIMIT 1
            ) lr ON TRUE
            WHERE ts.workspace_id = $1
            ORDER BY ts.task_id ASC
            LIMIT $2
            "#
        };
        let task_rows: Vec<ScheduledRevisionListingRow> = if let Some(after_id) = task_after {
            sqlx::query_as(task_sql)
                .bind(workspace_id)
                .bind(after_id)
                .bind(remaining)
                .fetch_all(&mut *tx)
                .await?
        } else {
            sqlx::query_as(task_sql)
                .bind(workspace_id)
                .bind(remaining)
                .fetch_all(&mut *tx)
                .await?
        };
        for (task_id, state_updated_at, writer_generation, created_at, last_rev_at) in task_rows {
            let anchor_at = last_rev_at.unwrap_or(created_at);
            out.push(ScheduledRevisionCandidate {
                workspace_id,
                target: RevisionTarget::Task(task_id),
                writer_generation,
                state_updated_at,
                anchor_at,
            });
        }
    }
    tx.commit().await?;
    Ok(out)
}

pub fn scheduled_revision_cursor(
    candidate: &ScheduledRevisionCandidate,
) -> ScheduledRevisionCursor {
    ScheduledRevisionCursor {
        workspace_id: candidate.workspace_id,
        target_kind: match candidate.target {
            RevisionTarget::Document(_) => 0,
            RevisionTarget::Task(_) => 1,
        },
        target_id: candidate.target.id(),
    }
}

/// Delete oldest automatic revision rows beyond `keep` per target (manual never deleted).
pub async fn gc_automatic_revisions_batch(
    pool: &PgPool,
    workspace_id: Uuid,
    keep: u32,
    batch: i32,
) -> Result<u32, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let deleted = sqlx::query(
        r#"
        WITH ranked AS (
            SELECT id,
                row_number() OVER (
                    PARTITION BY target_kind, target_id
                    ORDER BY created_at DESC, id DESC
                ) AS rn
            FROM fvoci.revisions
            WHERE workspace_id = $1 AND reason IN ('session', 'scheduled')
        ),
        doomed AS (
            SELECT id FROM ranked WHERE rn > $2 LIMIT $3
        ),
        locked AS (
            SELECT r.id, r.reason
            FROM fvoci.revisions r
            INNER JOIN doomed d ON d.id = r.id
            WHERE r.workspace_id = $1
            FOR UPDATE OF r
        )
        DELETE FROM fvoci.revisions r
        USING locked l
        WHERE r.workspace_id = $1
          AND r.id = l.id
          AND l.reason IN ('session', 'scheduled')
        "#,
    )
    .bind(workspace_id)
    .bind(i64::from(keep))
    .bind(batch)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    tx.commit().await?;
    Ok(deleted as u32)
}
