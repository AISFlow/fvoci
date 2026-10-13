//! Data for GET /api/v1/workspaces/{workspace_id}/export (source workspace-export.ts).
//!
//! Visibility matches notifications: private projects need membership view; wiki
//! documents need document_permission, not listTree ACL bypass.

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::db::context::{recheck_session, set_tenant};
use crate::db::documents::document_permission;
use crate::db::projects::project_permission_by_id;
use crate::db::workspace::{membership_role, workspace_is_live, WorkspaceRole};
use crate::projects::{effective_permission, ProjectPermission};

pub const EXPORT_TASK_PAGE: i64 = 10_000;
pub const EXPORT_TASK_MAX_PAGES: i32 = 100;
pub const EXPORT_COMMENT_PAGE: i64 = 500;
pub const EXPORT_COMMENT_MAX_PAGES: i32 = 200;
/// Refuse snapshots that would require holding more than this many document bodies at once.
pub const EXPORT_MAX_DOCUMENTS: usize = 20_000;
pub const EXPORT_MAX_ATTACHMENTS: usize = 60_000;
/// Serialized JSON metadata (documents/tasks/comments/attachments listings) per export.
pub const EXPORT_MAX_JSON_BYTES: u64 = 32 * 1024 * 1024;

#[cfg(feature = "db-tests")]
static FORCE_LOAD_SNAPSHOT_DB_ERROR: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashSet<Uuid>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashSet::new()));

#[cfg(feature = "db-tests")]
struct AttachmentPayloadBarrier {
    reached_tx: tokio::sync::oneshot::Sender<()>,
    proceed_rx: tokio::sync::oneshot::Receiver<()>,
}

#[cfg(feature = "db-tests")]
static ATTACHMENT_PAYLOAD_BARRIERS: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<Uuid, AttachmentPayloadBarrier>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

/// Fail `load_export_snapshot` with a database error so PG tests can assert inflight release.
#[cfg(feature = "db-tests")]
pub fn arm_workspace_export_snapshot_db_error(workspace_id: Uuid) {
    FORCE_LOAD_SNAPSHOT_DB_ERROR
        .lock()
        .expect("workspace export snapshot fail set")
        .insert(workspace_id);
}

#[cfg(feature = "db-tests")]
pub fn disarm_workspace_export_snapshot_db_error(workspace_id: Uuid) {
    if let Ok(mut set) = FORCE_LOAD_SNAPSHOT_DB_ERROR.lock() {
        set.remove(&workspace_id);
    }
}

/// Pause attachment byte delivery after storage open and before the post-open auth recheck.
#[cfg(feature = "db-tests")]
pub fn arm_attachment_payload_barrier(
    attachment_id: Uuid,
) -> (
    tokio::sync::oneshot::Receiver<()>,
    tokio::sync::oneshot::Sender<()>,
) {
    let (reached_tx, reached_rx) = tokio::sync::oneshot::channel();
    let (proceed_tx, proceed_rx) = tokio::sync::oneshot::channel();
    ATTACHMENT_PAYLOAD_BARRIERS
        .lock()
        .expect("attachment payload barriers")
        .insert(
            attachment_id,
            AttachmentPayloadBarrier {
                reached_tx,
                proceed_rx,
            },
        );
    (reached_rx, proceed_tx)
}

#[cfg(feature = "db-tests")]
pub fn disarm_attachment_payload_barrier(attachment_id: Uuid) {
    if let Ok(mut barriers) = ATTACHMENT_PAYLOAD_BARRIERS.lock() {
        barriers.remove(&attachment_id);
    }
}

#[cfg(feature = "db-tests")]
pub async fn pause_attachment_payload_barrier_if_armed(attachment_id: Uuid) {
    let barrier = ATTACHMENT_PAYLOAD_BARRIERS
        .lock()
        .ok()
        .and_then(|mut barriers| barriers.remove(&attachment_id));
    if let Some(barrier) = barrier {
        let _ = barrier.reached_tx.send(());
        let _ = barrier.proceed_rx.await;
    }
}

#[cfg(not(feature = "db-tests"))]
pub async fn pause_attachment_payload_barrier_if_armed(_attachment_id: Uuid) {}

/// Pause a document entry after its body fetch and before its delivery recheck.
/// Shares the attachment barrier map keyed by entry id; callers arm distinct ids.
#[cfg(feature = "db-tests")]
pub fn arm_document_delivery_barrier(
    document_id: Uuid,
) -> (
    tokio::sync::oneshot::Receiver<()>,
    tokio::sync::oneshot::Sender<()>,
) {
    arm_attachment_payload_barrier(document_id)
}

#[cfg(feature = "db-tests")]
pub async fn pause_document_delivery_barrier_if_armed(document_id: Uuid) {
    pause_attachment_payload_barrier_if_armed(document_id).await;
}

#[cfg(not(feature = "db-tests"))]
pub async fn pause_document_delivery_barrier_if_armed(_document_id: Uuid) {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceExportDbError {
    NotFound,
    Forbidden,
    Truncated,
}

#[derive(Debug, Clone)]
pub struct WorkspaceExportMeta {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub excluded_private_project_count: i32,
}

/// Identity and ordering only; bodies load at delivery after a fresh visibility check.
#[derive(Debug, Clone)]
pub struct ExportDocumentRef {
    pub id: Uuid,
    pub parent_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub title: String,
    pub status: String,
}

#[derive(Debug, Clone)]
pub struct ExportDocumentBody {
    pub text: String,
    pub content_json: Value,
}

#[derive(Debug, Clone)]
pub struct ExportTask {
    pub id: Uuid,
    pub project_id: Uuid,
    pub title: String,
    pub status_id: Uuid,
    pub task_type: String,
    pub parent_id: Option<Uuid>,
    pub archived_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct ExportCommentRef {
    pub id: Uuid,
    pub document_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct ExportAttachment {
    pub id: Uuid,
    pub document_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub name: String,
    pub mime: String,
    pub size_bytes: Option<i64>,
    pub scan_status: String,
    pub storage_key: String,
}

type ExportDocumentListingRow = (Uuid, Option<Uuid>, Option<Uuid>, String, String);

type ExportTaskPageRow = (
    Uuid,
    Uuid,
    String,
    Uuid,
    String,
    Option<Uuid>,
    Option<DateTime<Utc>>,
    DateTime<Utc>,
);

type ExportCommentPageRow = (Uuid, Option<Uuid>, Option<Uuid>, String, DateTime<Utc>);

type ExportAttachmentListingRow = (
    Uuid,
    Option<Uuid>,
    Option<Uuid>,
    String,
    String,
    Option<i64>,
    String,
    String,
);

#[derive(Debug, Clone)]
pub struct WorkspaceExportSnapshot {
    pub workspace: WorkspaceExportMeta,
    pub documents: Vec<ExportDocumentRef>,
    pub tasks: Vec<ExportTask>,
    pub comments: Vec<ExportCommentRef>,
    pub attachments: Vec<ExportAttachment>,
}

async fn require_manage(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<(), WorkspaceExportDbError>, sqlx::Error> {
    if !recheck_session(tx, actor_user_id, session_id).await? {
        return Ok(Err(WorkspaceExportDbError::Forbidden));
    }
    if !workspace_is_live(tx, workspace_id).await? {
        return Ok(Err(WorkspaceExportDbError::NotFound));
    }
    let role = membership_role(tx, workspace_id, actor_user_id).await?;
    if !role
        .map(|r| r.at_least(WorkspaceRole::Admin))
        .unwrap_or(false)
    {
        return Ok(Err(WorkspaceExportDbError::Forbidden));
    }
    Ok(Ok(()))
}

pub async fn recheck_export_authorization(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<(), WorkspaceExportDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let result = require_manage(&mut tx, workspace_id, actor_user_id, session_id).await?;
    tx.commit().await?;
    Ok(result)
}

pub async fn recheck_export_authorization_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<(), WorkspaceExportDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return recheck_export_authorization(pool, workspace_id, actor_user_id, session_id).await;
    }
    let mut tx = backend.begin_write().await?;
    let result = async {
        let DbTransaction::SqliteFamily(family) = &mut tx else {
            unreachable!()
        };
        family.set_tenant(workspace_id)?;
        family_export_manage(family, workspace_id, actor_user_id, session_id).await
    }
    .await;
    if let Err(cleanup) = tx.rollback().await {
        return Err(crate::db::backend::rollback_cleanup_unknown(
            match result {
                Err(error) => Some(Box::new(error) as Box<dyn std::error::Error + Send + Sync>),
                Ok(Err(refusal)) => Some(Box::new(sqlx::Error::Protocol(format!(
                    "workspace export refusal: {refusal:?}"
                )))
                    as Box<dyn std::error::Error + Send + Sync>),
                Ok(Ok(_)) => None,
            },
            cleanup,
        ));
    }
    result
}

/// Manage/session/live plus the attachment parent still being visible (revocation mid-pack).
pub async fn recheck_attachment_delivery(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Option<Uuid>,
    task_id: Option<Uuid>,
) -> Result<Result<(), WorkspaceExportDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    match require_manage(&mut tx, workspace_id, actor_user_id, session_id).await? {
        Ok(()) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    }
    if let Some(document_id) = document_id {
        let project_id: Option<Option<Uuid>> = sqlx::query_scalar(
            "SELECT project_id FROM fvoci.documents WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL",
        )
        .bind(workspace_id)
        .bind(document_id)
        .fetch_optional(&mut *tx)
        .await?;
        let project_id = match project_id {
            None => {
                tx.rollback().await?;
                return Ok(Err(WorkspaceExportDbError::Forbidden));
            }
            Some(project_id) => project_id,
        };
        let visible = match project_id {
            Some(pid) => {
                let permission =
                    project_permission_by_id(&mut tx, workspace_id, actor_user_id, pid).await?;
                permission
                    .map(|p| p.at_least(ProjectPermission::View))
                    .unwrap_or(false)
            }
            None => document_permission(&mut tx, workspace_id, actor_user_id, document_id, true)
                .await?
                .at_least(ProjectPermission::View),
        };
        if !visible {
            tx.rollback().await?;
            return Ok(Err(WorkspaceExportDbError::Forbidden));
        }
    } else if let Some(task_id) = task_id {
        let project_id: Option<Uuid> = sqlx::query_scalar(
            "SELECT project_id FROM fvoci.tasks WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL",
        )
        .bind(workspace_id)
        .bind(task_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(project_id) = project_id else {
            tx.rollback().await?;
            return Ok(Err(WorkspaceExportDbError::Forbidden));
        };
        let permission =
            project_permission_by_id(&mut tx, workspace_id, actor_user_id, project_id).await?;
        if !permission
            .map(|p| p.at_least(ProjectPermission::View))
            .unwrap_or(false)
        {
            tx.rollback().await?;
            return Ok(Err(WorkspaceExportDbError::Forbidden));
        }
    } else {
        tx.rollback().await?;
        return Ok(Err(WorkspaceExportDbError::Forbidden));
    }
    tx.commit().await?;
    Ok(Ok(()))
}

pub async fn recheck_attachment_delivery_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Option<Uuid>,
    task_id: Option<Uuid>,
) -> Result<Result<(), WorkspaceExportDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return recheck_attachment_delivery(
            pool,
            workspace_id,
            actor_user_id,
            session_id,
            document_id,
            task_id,
        )
        .await;
    }
    let mut tx = backend.begin_write().await?;
    let result = async {
        let DbTransaction::SqliteFamily(family) = &mut tx else {
            unreachable!()
        };
        family.set_tenant(workspace_id)?;
        if let Err(error) =
            family_export_manage(family, workspace_id, actor_user_id, session_id).await?
        {
            return Ok(Err(error));
        }
        Ok(
            if family_export_parent(family, workspace_id, actor_user_id, document_id, task_id)
                .await?
            {
                Ok(())
            } else {
                Err(WorkspaceExportDbError::Forbidden)
            },
        )
    }
    .await;
    if let Err(cleanup) = tx.rollback().await {
        return Err(crate::db::backend::rollback_cleanup_unknown(
            match result {
                Err(error) => Some(Box::new(error) as Box<dyn std::error::Error + Send + Sync>),
                Ok(Err(refusal)) => Some(Box::new(sqlx::Error::Protocol(format!(
                    "workspace export refusal: {refusal:?}"
                )))
                    as Box<dyn std::error::Error + Send + Sync>),
                Ok(Ok(_)) => None,
            },
            cleanup,
        ));
    }
    result
}

pub async fn recheck_document_delivery(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
    project_id: Option<Uuid>,
) -> Result<Result<(), WorkspaceExportDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    match require_manage(&mut tx, workspace_id, actor_user_id, session_id).await? {
        Ok(()) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    }
    let live: Option<bool> = sqlx::query_scalar(
        "SELECT deleted_at IS NULL FROM fvoci.documents WHERE workspace_id = $1 AND id = $2",
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_optional(&mut *tx)
    .await?;
    if !live.unwrap_or(false) {
        tx.rollback().await?;
        return Ok(Err(WorkspaceExportDbError::Forbidden));
    }
    let visible = match project_id {
        Some(pid) => {
            let permission =
                project_permission_by_id(&mut tx, workspace_id, actor_user_id, pid).await?;
            permission
                .map(|p| p.at_least(ProjectPermission::View))
                .unwrap_or(false)
        }
        None => document_permission(&mut tx, workspace_id, actor_user_id, document_id, true)
            .await?
            .at_least(ProjectPermission::View),
    };
    if !visible {
        tx.rollback().await?;
        return Ok(Err(WorkspaceExportDbError::Forbidden));
    }
    tx.commit().await?;
    Ok(Ok(()))
}

pub async fn recheck_document_delivery_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
    project_id: Option<Uuid>,
) -> Result<Result<(), WorkspaceExportDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return recheck_document_delivery(
            pool,
            workspace_id,
            actor_user_id,
            session_id,
            document_id,
            project_id,
        )
        .await;
    }
    let mut tx = backend.begin_write().await?;
    let result = async {
        let DbTransaction::SqliteFamily(family) = &mut tx else {
            unreachable!()
        };
        family.set_tenant(workspace_id)?;
        if let Err(error) =
            family_export_manage(family, workspace_id, actor_user_id, session_id).await?
        {
            return Ok(Err(error));
        }
        let _ = project_id;
        Ok(
            if family_export_parent(family, workspace_id, actor_user_id, Some(document_id), None)
                .await?
            {
                Ok(())
            } else {
                Err(WorkspaceExportDbError::Forbidden)
            },
        )
    }
    .await;
    if let Err(cleanup) = tx.rollback().await {
        return Err(crate::db::backend::rollback_cleanup_unknown(
            match result {
                Err(error) => Some(Box::new(error) as Box<dyn std::error::Error + Send + Sync>),
                Ok(Err(refusal)) => Some(Box::new(sqlx::Error::Protocol(format!(
                    "workspace export refusal: {refusal:?}"
                )))
                    as Box<dyn std::error::Error + Send + Sync>),
                Ok(Ok(_)) => None,
            },
            cleanup,
        ));
    }
    result
}

pub async fn recheck_task_delivery(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    project_id: Uuid,
) -> Result<Result<(), WorkspaceExportDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    match require_manage(&mut tx, workspace_id, actor_user_id, session_id).await? {
        Ok(()) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    }
    let permission =
        project_permission_by_id(&mut tx, workspace_id, actor_user_id, project_id).await?;
    if !permission
        .map(|p| p.at_least(ProjectPermission::View))
        .unwrap_or(false)
    {
        tx.rollback().await?;
        return Ok(Err(WorkspaceExportDbError::Forbidden));
    }
    tx.commit().await?;
    Ok(Ok(()))
}

pub async fn recheck_task_delivery_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    project_id: Uuid,
) -> Result<Result<(), WorkspaceExportDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return recheck_task_delivery(pool, workspace_id, actor_user_id, session_id, project_id)
            .await;
    }
    let mut tx = backend.begin_write().await?;
    let result = async {
        let DbTransaction::SqliteFamily(family) = &mut tx else {
            unreachable!()
        };
        family.set_tenant(workspace_id)?;
        if let Err(error) =
            family_export_manage(family, workspace_id, actor_user_id, session_id).await?
        {
            return Ok(Err(error));
        }
        Ok(
            if OperationTx::SqliteFamily(family)
                .project_permission_by_id(workspace_id, actor_user_id, project_id)
                .await?
                .is_some_and(|p| p.at_least(ProjectPermission::View))
            {
                Ok(())
            } else {
                Err(WorkspaceExportDbError::Forbidden)
            },
        )
    }
    .await;
    if let Err(cleanup) = tx.rollback().await {
        return Err(crate::db::backend::rollback_cleanup_unknown(
            match result {
                Err(error) => Some(Box::new(error) as Box<dyn std::error::Error + Send + Sync>),
                Ok(Err(refusal)) => Some(Box::new(sqlx::Error::Protocol(format!(
                    "workspace export refusal: {refusal:?}"
                )))
                    as Box<dyn std::error::Error + Send + Sync>),
                Ok(Ok(_)) => None,
            },
            cleanup,
        ));
    }
    result
}

pub async fn recheck_comment_delivery(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Option<Uuid>,
    task_id: Option<Uuid>,
) -> Result<Result<(), WorkspaceExportDbError>, sqlx::Error> {
    if let Some(document_id) = document_id {
        let mut tx = pool.begin().await?;
        set_tenant(&mut tx, workspace_id).await?;
        let project_id: Option<Option<Uuid>> = sqlx::query_scalar(
            "SELECT project_id FROM fvoci.documents WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL",
        )
        .bind(workspace_id)
        .bind(document_id)
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        let project_id = match project_id {
            None => return Ok(Err(WorkspaceExportDbError::Forbidden)),
            Some(project_id) => project_id,
        };
        return recheck_document_delivery(
            pool,
            workspace_id,
            actor_user_id,
            session_id,
            document_id,
            project_id,
        )
        .await;
    }
    if let Some(task_id) = task_id {
        let mut tx = pool.begin().await?;
        set_tenant(&mut tx, workspace_id).await?;
        let project_id: Option<Uuid> = sqlx::query_scalar(
            "SELECT project_id FROM fvoci.tasks WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL",
        )
        .bind(workspace_id)
        .bind(task_id)
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        let Some(project_id) = project_id else {
            return Ok(Err(WorkspaceExportDbError::Forbidden));
        };
        return recheck_task_delivery(pool, workspace_id, actor_user_id, session_id, project_id)
            .await;
    }
    Ok(Err(WorkspaceExportDbError::Forbidden))
}

pub async fn recheck_comment_delivery_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Option<Uuid>,
    task_id: Option<Uuid>,
) -> Result<Result<(), WorkspaceExportDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return recheck_comment_delivery(
            pool,
            workspace_id,
            actor_user_id,
            session_id,
            document_id,
            task_id,
        )
        .await;
    }
    let mut tx = backend.begin_write().await?;
    let result = async {
        let DbTransaction::SqliteFamily(family) = &mut tx else {
            unreachable!()
        };
        family.set_tenant(workspace_id)?;
        if let Err(error) =
            family_export_manage(family, workspace_id, actor_user_id, session_id).await?
        {
            return Ok(Err(error));
        }
        Ok(
            if family_export_parent(family, workspace_id, actor_user_id, document_id, task_id)
                .await?
            {
                Ok(())
            } else {
                Err(WorkspaceExportDbError::Forbidden)
            },
        )
    }
    .await;
    if let Err(cleanup) = tx.rollback().await {
        return Err(crate::db::backend::rollback_cleanup_unknown(
            match result {
                Err(error) => Some(Box::new(error) as Box<dyn std::error::Error + Send + Sync>),
                Ok(Err(refusal)) => Some(Box::new(sqlx::Error::Protocol(format!(
                    "workspace export refusal: {refusal:?}"
                )))
                    as Box<dyn std::error::Error + Send + Sync>),
                Ok(Ok(_)) => None,
            },
            cleanup,
        ));
    }
    result
}

pub async fn fetch_document_body(
    pool: &PgPool,
    workspace_id: Uuid,
    document_id: Uuid,
) -> Result<Option<ExportDocumentBody>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let row: Option<(String, Value)> = sqlx::query_as(
        r#"
        SELECT text, content_json
        FROM fvoci.documents
        WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(row.map(|(text, content_json)| ExportDocumentBody { text, content_json }))
}

pub async fn fetch_document_body_backend(
    backend: &Backend,
    workspace_id: Uuid,
    document_id: Uuid,
) -> Result<Option<ExportDocumentBody>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return fetch_document_body(pool, workspace_id, document_id).await;
    }
    let mut tx = backend.begin_read().await?;
    let result=async {
  let DbTransaction::SqliteFamily(family)=&mut tx else {unreachable!()};
  family.set_tenant(workspace_id)?;
  let rows=family.query("SELECT text,content_json FROM documents WHERE workspace_id=?1 AND id=?2 AND deleted_at IS NULL",&[Cell::uuid(workspace_id),Cell::uuid(document_id)]).await?; rows.first().map(|r|Ok(ExportDocumentBody {text:r.cell(0)?.string()?,content_json:r.cell(1)?.value()?})).transpose()
 }.await;
    if let Err(cleanup) = tx.rollback().await {
        return Err(crate::db::backend::rollback_cleanup_unknown(
            result
                .err()
                .map(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>),
            cleanup,
        ));
    }
    result
}

pub async fn fetch_comment_body(
    pool: &PgPool,
    workspace_id: Uuid,
    comment_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let body =
        sqlx::query_scalar("SELECT body FROM fvoci.comments WHERE workspace_id = $1 AND id = $2")
            .bind(workspace_id)
            .bind(comment_id)
            .fetch_optional(&mut *tx)
            .await?;
    tx.commit().await?;
    Ok(body)
}

pub async fn fetch_comment_body_backend(
    backend: &Backend,
    workspace_id: Uuid,
    comment_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return fetch_comment_body(pool, workspace_id, comment_id).await;
    }
    let mut tx = backend.begin_read().await?;
    let result = async {
        let DbTransaction::SqliteFamily(family) = &mut tx else {
            unreachable!()
        };
        family.set_tenant(workspace_id)?;
        let rows = family
            .query(
                "SELECT body FROM comments WHERE workspace_id=?1 AND id=?2",
                &[Cell::uuid(workspace_id), Cell::uuid(comment_id)],
            )
            .await?;
        rows.first().map(|r| r.cell(0)?.string()).transpose()
    }
    .await;
    if let Err(cleanup) = tx.rollback().await {
        return Err(crate::db::backend::rollback_cleanup_unknown(
            result
                .err()
                .map(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>),
            cleanup,
        ));
    }
    result
}

pub async fn load_export_snapshot(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<WorkspaceExportSnapshot, WorkspaceExportDbError>, sqlx::Error> {
    #[cfg(feature = "db-tests")]
    if FORCE_LOAD_SNAPSHOT_DB_ERROR
        .lock()
        .expect("workspace export snapshot fail set")
        .contains(&workspace_id)
    {
        return Err(sqlx::Error::Configuration(
            "workspace_export forced snapshot db error".into(),
        ));
    }

    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    match require_manage(&mut tx, workspace_id, actor_user_id, session_id).await? {
        Ok(()) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    }

    let workspace_row: Option<(Uuid, String, String)> = sqlx::query_as(
        "SELECT id, slug, name FROM fvoci.workspaces WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(workspace_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((id, slug, name)) = workspace_row else {
        tx.rollback().await?;
        return Ok(Err(WorkspaceExportDbError::NotFound));
    };

    let project_rows: Vec<(Uuid, String)> = sqlx::query_as(
        r#"
        SELECT id, visibility
        FROM fvoci.projects
        WHERE workspace_id = $1 AND deleted_at IS NULL
        ORDER BY key COLLATE "C"
        "#,
    )
    .bind(workspace_id)
    .fetch_all(&mut *tx)
    .await?;

    let workspace_role = membership_role(&mut tx, workspace_id, actor_user_id)
        .await?
        .unwrap_or(WorkspaceRole::Guest);

    let mut visible_project_ids = std::collections::HashSet::new();
    let mut excluded_private = 0i32;
    for (project_id, visibility) in &project_rows {
        let member_role = crate::db::projects::project_member_role(
            &mut tx,
            workspace_id,
            *project_id,
            actor_user_id,
        )
        .await?;
        let level = effective_permission(workspace_role, visibility, member_role);
        if level.at_least(ProjectPermission::View) {
            visible_project_ids.insert(*project_id);
        } else if visibility == "private" {
            excluded_private += 1;
        }
    }

    let doc_nodes: Vec<ExportDocumentListingRow> = sqlx::query_as(
        r#"
        SELECT id, parent_id, project_id, title, status
        FROM fvoci.documents
        WHERE workspace_id = $1 AND deleted_at IS NULL
        ORDER BY sort_key COLLATE "C"
        "#,
    )
    .bind(workspace_id)
    .fetch_all(&mut *tx)
    .await?;

    let mut documents = Vec::new();
    for (doc_id, parent_id, project_id, title, status) in doc_nodes {
        let visible = match project_id {
            Some(pid) => visible_project_ids.contains(&pid),
            None => document_permission(&mut tx, workspace_id, actor_user_id, doc_id, true)
                .await?
                .at_least(ProjectPermission::View),
        };
        if visible {
            documents.push(ExportDocumentRef {
                id: doc_id,
                parent_id,
                project_id,
                title,
                status,
            });
        }
    }

    if documents.len() > EXPORT_MAX_DOCUMENTS {
        tx.rollback().await?;
        return Ok(Err(WorkspaceExportDbError::Truncated));
    }

    let mut tasks = Vec::new();
    for (project_id, _) in &project_rows {
        if !visible_project_ids.contains(project_id) {
            continue;
        }
        let mut after: Option<(DateTime<Utc>, Uuid)> = None;
        let mut pages = 0i32;
        loop {
            if pages >= EXPORT_TASK_MAX_PAGES {
                tx.rollback().await?;
                return Ok(Err(WorkspaceExportDbError::Truncated));
            }
            let page_rows: Vec<ExportTaskPageRow> = sqlx::query_as(
                r#"
                SELECT id, project_id, title, status_id, type, parent_id, archived_at, created_at
                FROM fvoci.tasks
                WHERE workspace_id = $1 AND project_id = $2 AND deleted_at IS NULL
                  AND ($3::timestamptz IS NULL OR (created_at, id) > ($3, $4))
                ORDER BY created_at ASC, id ASC
                LIMIT $5
                "#,
            )
            .bind(workspace_id)
            .bind(project_id)
            .bind(after.map(|(at, _)| at))
            .bind(after.map(|(_, id)| id))
            .bind(EXPORT_TASK_PAGE + 1)
            .fetch_all(&mut *tx)
            .await?;
            if page_rows.len() > EXPORT_TASK_PAGE as usize {
                tx.rollback().await?;
                return Ok(Err(WorkspaceExportDbError::Truncated));
            }
            let full = page_rows.len() as i64 == EXPORT_TASK_PAGE;
            for row in page_rows {
                tasks.push(ExportTask {
                    id: row.0,
                    project_id: row.1,
                    title: row.2,
                    status_id: row.3,
                    task_type: row.4,
                    parent_id: row.5,
                    archived_at: row.6,
                    created_at: row.7,
                });
            }
            if !full {
                break;
            }
            let last = tasks.last().expect("full page");
            after = Some((last.created_at, last.id));
            pages += 1;
        }
    }

    let doc_id_set: std::collections::HashSet<Uuid> = documents.iter().map(|d| d.id).collect();
    let task_id_set: std::collections::HashSet<Uuid> = tasks.iter().map(|t| t.id).collect();

    let mut comments = Vec::new();
    let mut after_comment: Option<(DateTime<Utc>, Uuid)> = None;
    let mut comment_pages = 0i32;
    loop {
        if comment_pages >= EXPORT_COMMENT_MAX_PAGES {
            tx.rollback().await?;
            return Ok(Err(WorkspaceExportDbError::Truncated));
        }
        let page_rows: Vec<ExportCommentPageRow> = sqlx::query_as(
            r#"
                SELECT id, document_id, task_id, body, created_at
                FROM fvoci.comments
                WHERE workspace_id = $1
                  AND ($2::timestamptz IS NULL OR (created_at, id) > ($2, $3))
                ORDER BY created_at ASC, id ASC
                LIMIT $4
                "#,
        )
        .bind(workspace_id)
        .bind(after_comment.map(|(at, _)| at))
        .bind(after_comment.map(|(_, id)| id))
        .bind(EXPORT_COMMENT_PAGE)
        .fetch_all(&mut *tx)
        .await?;
        let full = page_rows.len() as i64 == EXPORT_COMMENT_PAGE;
        let page_cursor = page_rows.last().map(|row| (row.4, row.0));
        for (id, document_id, task_id, _body, created_at) in page_rows {
            let keep = if let Some(document_id) = document_id {
                doc_id_set.contains(&document_id)
            } else if let Some(task_id) = task_id {
                task_id_set.contains(&task_id)
            } else {
                false
            };
            if keep {
                comments.push(ExportCommentRef {
                    id,
                    document_id,
                    task_id,
                    created_at,
                });
            }
        }
        if !full {
            break;
        }
        after_comment = page_cursor;
        comment_pages += 1;
    }

    let doc_ids: Vec<Uuid> = doc_id_set.iter().copied().collect();
    let task_ids: Vec<Uuid> = task_id_set.iter().copied().collect();
    let mut attachments = Vec::new();
    let mut seen = std::collections::HashSet::new();
    if !doc_ids.is_empty() || !task_ids.is_empty() {
        let rows: Vec<ExportAttachmentListingRow> = sqlx::query_as(
            r#"
            SELECT id, document_id, task_id, name, mime, size_bytes, scan_status, storage_key
            FROM fvoci.attachments
            WHERE workspace_id = $1 AND status = 'stored'
              AND (
                (document_id IS NOT NULL AND document_id = ANY($2))
                OR (task_id IS NOT NULL AND task_id = ANY($3))
              )
            ORDER BY created_at ASC, id ASC
            "#,
        )
        .bind(workspace_id)
        .bind(&doc_ids)
        .bind(&task_ids)
        .fetch_all(&mut *tx)
        .await?;
        for (id, document_id, task_id, name, mime, size_bytes, scan_status, storage_key) in rows {
            if !seen.insert(id) {
                continue;
            }
            attachments.push(ExportAttachment {
                id,
                document_id,
                task_id,
                name,
                mime,
                size_bytes,
                scan_status,
                storage_key,
            });
        }
    }
    if attachments.len() > EXPORT_MAX_ATTACHMENTS {
        tx.rollback().await?;
        return Ok(Err(WorkspaceExportDbError::Truncated));
    }

    tx.commit().await?;
    Ok(Ok(WorkspaceExportSnapshot {
        workspace: WorkspaceExportMeta {
            id,
            slug,
            name,
            excluded_private_project_count: excluded_private,
        },
        documents,
        tasks,
        comments,
        attachments,
    }))
}

pub async fn load_export_snapshot_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<WorkspaceExportSnapshot, WorkspaceExportDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return load_export_snapshot(pool, workspace_id, actor_user_id, session_id).await;
    }
    let mut tx = backend.begin_write().await?;
    let result = async {
        let DbTransaction::SqliteFamily(family) = &mut tx else {
            unreachable!()
        };
        family.set_tenant(workspace_id)?;
        family_load_export_snapshot(family, workspace_id, actor_user_id, session_id).await
    }
    .await;
    if let Err(cleanup) = tx.rollback().await {
        return Err(crate::db::backend::rollback_cleanup_unknown(
            match result {
                Err(error) => Some(Box::new(error) as Box<dyn std::error::Error + Send + Sync>),
                Ok(Err(refusal)) => Some(Box::new(sqlx::Error::Protocol(format!(
                    "workspace export refusal: {refusal:?}"
                )))
                    as Box<dyn std::error::Error + Send + Sync>),
                Ok(Ok(_)) => None,
            },
            cleanup,
        ));
    }
    result
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceJson {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub excluded_private_project_count: i32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentJson<'a> {
    pub id: String,
    pub parent_id: Option<String>,
    pub title: &'a str,
    pub status: &'a str,
    pub text: &'a str,
    pub content_json: &'a Value,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskJson {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub status_id: String,
    #[serde(rename = "type")]
    pub task_type: String,
    pub parent_id: Option<String>,
    pub archived_at: Option<String>,
    pub created_at: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommentJson<'a> {
    pub id: String,
    pub document_id: Option<String>,
    pub task_id: Option<String>,
    pub body: &'a str,
    pub created_at: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentMetaJson<'a> {
    pub id: String,
    pub name: &'a str,
    pub mime: &'a str,
    pub size_bytes: Option<i64>,
    pub scan_status: &'a str,
}

pub fn js_iso(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

use crate::db::backend::{Backend, DbTransaction, FamilyTx, OperationTx};
use crate::db::codec::Cell;

async fn family_export_manage(
    tx: &mut FamilyTx,
    ws: Uuid,
    actor: Uuid,
    credential: Uuid,
) -> Result<Result<(), WorkspaceExportDbError>, sqlx::Error> {
    let mut op = OperationTx::SqliteFamily(tx);
    op.set_tenant(ws).await?;
    if !op.recheck_session(actor, credential).await? {
        return Ok(Err(WorkspaceExportDbError::Forbidden));
    }
    if !op.workspace_is_live(ws).await? {
        return Ok(Err(WorkspaceExportDbError::NotFound));
    }
    Ok(
        if op
            .membership_role(ws, actor, false)
            .await?
            .is_some_and(|r| r.at_least(WorkspaceRole::Admin))
        {
            Ok(())
        } else {
            Err(WorkspaceExportDbError::Forbidden)
        },
    )
}

async fn family_export_parent(
    tx: &mut FamilyTx,
    ws: Uuid,
    actor: Uuid,
    document: Option<Uuid>,
    task: Option<Uuid>,
) -> Result<bool, sqlx::Error> {
    let (table, id) = match (document, task) {
        (Some(id), _) => ("documents", id),
        (_, Some(id)) => ("tasks", id),
        _ => return Ok(false),
    };
    let rows=tx.query(match table {"tasks"=>"SELECT project_id FROM tasks WHERE workspace_id=?1 AND id=?2 AND deleted_at IS NULL",_=>"SELECT project_id FROM documents WHERE workspace_id=?1 AND id=?2 AND deleted_at IS NULL"},&[Cell::uuid(ws),Cell::uuid(id)]).await?;
    let Some(row) = rows.first() else {
        return Ok(false);
    };
    Ok(match row.cell(0)?.optional(Cell::id)? {
        Some(project) => OperationTx::SqliteFamily(tx)
            .project_permission_by_id(ws, actor, project)
            .await?
            .is_some_and(|p| p.at_least(ProjectPermission::View)),
        None => OperationTx::SqliteFamily(tx)
            .document_permission(ws, actor, id, true)
            .await?
            .at_least(ProjectPermission::View),
    })
}

async fn family_load_export_snapshot(
    tx: &mut FamilyTx,
    ws: Uuid,
    actor: Uuid,
    credential: Uuid,
) -> Result<Result<WorkspaceExportSnapshot, WorkspaceExportDbError>, sqlx::Error> {
    #[cfg(feature = "db-tests")]
    if FORCE_LOAD_SNAPSHOT_DB_ERROR
        .lock()
        .expect("snapshot fail set")
        .contains(&ws)
    {
        return Err(sqlx::Error::Configuration(
            "workspace_export forced snapshot db error".into(),
        ));
    }
    if let Err(error) = family_export_manage(tx, ws, actor, credential).await? {
        return Ok(Err(error));
    }
    let workspace = tx
        .query(
            "SELECT slug,name FROM workspaces WHERE id=?1 AND deleted_at IS NULL",
            &[Cell::uuid(ws)],
        )
        .await?;
    let Some(workspace) = workspace.first() else {
        return Ok(Err(WorkspaceExportDbError::NotFound));
    };
    let mut meta = WorkspaceExportMeta {
        id: ws,
        slug: workspace.cell(0)?.string()?,
        name: workspace.cell(1)?.string()?,
        excluded_private_project_count: 0,
    };
    let projects=tx.query("SELECT id,visibility FROM projects WHERE workspace_id=?1 AND deleted_at IS NULL ORDER BY key COLLATE BINARY",&[Cell::uuid(ws)]).await?;
    let mut visible = std::collections::HashSet::new();
    let mut tasks = Vec::new();
    for project in projects {
        let id = project.cell(0)?.id()?;
        if !OperationTx::SqliteFamily(&mut *tx)
            .project_permission_by_id(ws, actor, id)
            .await?
            .is_some_and(|p| p.at_least(ProjectPermission::View))
        {
            if project.cell(1)?.string()? == "private" {
                meta.excluded_private_project_count += 1;
            }
            continue;
        }
        visible.insert(id);
        let rows=tx.query("SELECT id,project_id,title,status_id,type,parent_id,archived_at,created_at FROM tasks WHERE workspace_id=?1 AND project_id=?2 AND deleted_at IS NULL ORDER BY created_at,id LIMIT ?3",&[Cell::uuid(ws),Cell::uuid(id),Cell::Integer(EXPORT_TASK_PAGE+1)]).await?;
        // The PG contract rejects a page requiring truncation, rather than silently omitting rows.
        if rows.len() > EXPORT_TASK_PAGE as usize {
            return Ok(Err(WorkspaceExportDbError::Truncated));
        }
        for r in rows {
            tasks.push(ExportTask {
                id: r.cell(0)?.id()?,
                project_id: r.cell(1)?.id()?,
                title: r.cell(2)?.string()?,
                status_id: r.cell(3)?.id()?,
                task_type: r.cell(4)?.string()?,
                parent_id: r.cell(5)?.optional(Cell::id)?,
                archived_at: r.cell(6)?.optional(Cell::datetime)?,
                created_at: r.cell(7)?.datetime()?,
            });
        }
    }
    let rows=tx.query("SELECT id,parent_id,project_id,title,status FROM documents WHERE workspace_id=?1 AND deleted_at IS NULL ORDER BY sort_key COLLATE BINARY",&[Cell::uuid(ws)]).await?;
    let mut documents = Vec::new();
    for r in rows {
        let id = r.cell(0)?.id()?;
        let project = r.cell(2)?.optional(Cell::id)?;
        let allowed = match project {
            Some(id) => visible.contains(&id),
            None => OperationTx::SqliteFamily(&mut *tx)
                .document_permission(ws, actor, id, true)
                .await?
                .at_least(ProjectPermission::View),
        };
        if allowed {
            documents.push(ExportDocumentRef {
                id,
                parent_id: r.cell(1)?.optional(Cell::id)?,
                project_id: project,
                title: r.cell(3)?.string()?,
                status: r.cell(4)?.string()?,
            });
        }
        if documents.len() > EXPORT_MAX_DOCUMENTS {
            return Ok(Err(WorkspaceExportDbError::Truncated));
        }
    }
    let docs: std::collections::HashSet<_> = documents.iter().map(|d| d.id).collect();
    let task_ids: std::collections::HashSet<_> = tasks.iter().map(|t| t.id).collect();
    let keep_parent = |doc: Option<Uuid>, task: Option<Uuid>| {
        doc.map(|id| docs.contains(&id))
            .unwrap_or_else(|| task.is_some_and(|id| task_ids.contains(&id)))
    };
    let mut comments = Vec::new();
    let mut after: Option<(i64, Uuid)> = None;
    for page in 0..EXPORT_COMMENT_MAX_PAGES {
        let rows=tx.query("SELECT id,document_id,task_id,created_at FROM comments WHERE workspace_id=?1 AND (?2 IS NULL OR (created_at,id)>(?2,?3)) ORDER BY created_at,id LIMIT ?4",&[Cell::uuid(ws),after.map(|v|Cell::Integer(v.0)).unwrap_or(Cell::Null),Cell::optional_uuid(after.map(|v|v.1)),Cell::Integer(EXPORT_COMMENT_PAGE)]).await?;
        let full = rows.len() == EXPORT_COMMENT_PAGE as usize;
        for r in rows {
            let id = r.cell(0)?.id()?;
            let document_id = r.cell(1)?.optional(Cell::id)?;
            let task_id = r.cell(2)?.optional(Cell::id)?;
            let created_at = r.cell(3)?.datetime()?;
            after = Some((created_at.timestamp_micros(), id));
            if keep_parent(document_id, task_id) {
                comments.push(ExportCommentRef {
                    id,
                    document_id,
                    task_id,
                    created_at,
                });
            }
        }
        if !full {
            break;
        }
        if page + 1 == EXPORT_COMMENT_MAX_PAGES {
            return Ok(Err(WorkspaceExportDbError::Truncated));
        }
    }
    let rows=tx.query("SELECT id,document_id,task_id,name,mime,size_bytes,scan_status,storage_key FROM attachments WHERE workspace_id=?1 AND status='stored' ORDER BY created_at,id",&[Cell::uuid(ws)]).await?;
    let mut attachments = Vec::new();
    for r in rows {
        let document_id = r.cell(1)?.optional(Cell::id)?;
        let task_id = r.cell(2)?.optional(Cell::id)?;
        if !keep_parent(document_id, task_id) {
            continue;
        }
        attachments.push(ExportAttachment {
            id: r.cell(0)?.id()?,
            document_id,
            task_id,
            name: r.cell(3)?.string()?,
            mime: r.cell(4)?.string()?,
            size_bytes: r.cell(5)?.optional(Cell::integer)?,
            scan_status: r.cell(6)?.string()?,
            storage_key: r.cell(7)?.string()?,
        });
        if attachments.len() > EXPORT_MAX_ATTACHMENTS {
            return Ok(Err(WorkspaceExportDbError::Truncated));
        }
    }
    Ok(Ok(WorkspaceExportSnapshot {
        workspace: meta,
        documents,
        tasks,
        comments,
        attachments,
    }))
}
