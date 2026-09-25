use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::pool::PoolConnection;
use sqlx::{Acquire, PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use crate::attachments::{
    initial_extract_status, is_hwp_attachment, is_image_mime, StagedPart, UploadLimits,
    ATTACHMENT_LOCK_NAMESPACE, MAX_PART_COUNT, STORAGE_LOCK_NAMESPACE,
};
use crate::attachments::{ObjectStorage, StorageError};
use crate::db::context::{lock_key_from_uuid, restore_system, set_system, set_tenant};
use crate::db::documents::{
    document_permission, lock_membership_users, membership_role, membership_role_for_update,
    recheck_session, session_is_live, workspace_is_live,
};
use crate::db::identity::{append_audit, append_event, AuditAppend, EventAppend};
use crate::db::projects::project_member_role;
use crate::db::quota::{StorageQuota, StorageQuotaError};
use crate::projects::effective_permission;
use crate::projects::ProjectPermission;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadMeta {
    pub part_size_bytes: i64,
    pub part_count: i32,
    pub declared_size_bytes: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upload_ref: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AttachmentRow {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub document_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub uploader_id: Uuid,
    pub status: String,
    pub name: String,
    pub mime: String,
    pub declared_mime: Option<String>,
    pub size_bytes: Option<i64>,
    pub reserved_size_bytes: i64,
    pub storage_key: String,
    pub image: bool,
    pub scan_status: String,
    pub extract_status: String,
    pub upload_meta: Option<Value>,
    pub variants: Value,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

/// Source `attachments_parent_xor_check`: exactly one parent, fixed at insert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentParent {
    Document(Uuid),
    Task(Uuid),
}

/// Source `previewVariantOf`: a published preview object and its size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewVariant {
    pub key: String,
    pub width: i64,
    pub height: i64,
    pub bytes: i64,
}

pub fn preview_variant_of(variants: &Value) -> Option<PreviewVariant> {
    let preview = variants.get("preview")?;
    let key = preview.get("key")?.as_str()?;
    let positive = |name: &str| {
        preview
            .get(name)
            .and_then(Value::as_i64)
            .filter(|v| *v > 0 && *v <= 9_007_199_254_740_991)
    };
    if key.is_empty() {
        return None;
    }
    Some(PreviewVariant {
        key: key.to_string(),
        width: positive("width")?,
        height: positive("height")?,
        bytes: positive("bytes")?,
    })
}

impl AttachmentRow {
    pub fn preview(&self) -> Option<PreviewVariant> {
        preview_variant_of(&self.variants)
    }

    pub fn parent(&self) -> AttachmentParent {
        match (self.document_id, self.task_id) {
            (Some(document_id), _) => AttachmentParent::Document(document_id),
            (None, Some(task_id)) => AttachmentParent::Task(task_id),
            (None, None) => unreachable!("attachments_parent_xor_check"),
        }
    }
}

/// Where a new upload is created. The document routes carry the affiliation
/// of the URL (wiki or one project) so a document is only reachable through
/// its own route (source `getDocument` + `affiliationFromParams`).
#[derive(Debug, Clone, Copy)]
pub enum UploadTarget {
    WikiDocument(Uuid),
    ProjectDocument { project_id: Uuid, document_id: Uuid },
    Task(Uuid),
}

#[derive(Debug, PartialEq, Eq)]
pub enum AttachmentDbError {
    NotFound,
    Forbidden,
    UploadForbidden,
    UploadState,
    TooLarge,
    PartTooLarge,
    InvalidInput,
    EtagMismatch,
    Infected,
    ProjectArchived,
    TaskArchived,
    NotHwp,
    StorageLimit,
    UploadLimit,
}

pub struct CreateUploadInput {
    pub name: String,
    pub size_bytes: i64,
    pub declared_mime: Option<String>,
}

struct AttachmentSessionLock {
    conn: Option<PoolConnection<Postgres>>,
    lock_key: i32,
    held: bool,
}

impl AttachmentSessionLock {
    async fn try_acquire(pool: &PgPool, attachment_id: Uuid) -> Result<Option<Self>, sqlx::Error> {
        let mut conn = pool.acquire().await?;
        // Fail-safe: close_on_drop before try-lock so cancellation during
        // acquisition cannot return a lock-holding connection to the pool.
        // Moving this after a successful lock would leak a session advisory lock
        // if the task is cancelled between acquire and the flag. Connection churn
        // while losers poll is a tracked follow-up (review N3), not this change.
        conn.close_on_drop();
        let lock_key = lock_key_from_uuid(attachment_id);
        let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1, $2)")
            .bind(ATTACHMENT_LOCK_NAMESPACE)
            .bind(lock_key)
            .fetch_one(&mut *conn)
            .await?;
        if !locked {
            return Ok(None);
        }
        Ok(Some(Self {
            conn: Some(conn),
            lock_key,
            held: true,
        }))
    }

    async fn begin(&mut self) -> Result<Transaction<'_, Postgres>, sqlx::Error> {
        self.conn.as_mut().expect("lock connection").begin().await
    }

    async fn release(mut self) {
        if self.held {
            if let Some(mut conn) = self.conn.take() {
                let _ = sqlx::query("SELECT pg_advisory_unlock($1, $2)")
                    .bind(ATTACHMENT_LOCK_NAMESPACE)
                    .bind(self.lock_key)
                    .execute(&mut *conn)
                    .await;
            }
        }
        self.held = false;
    }
}

fn parse_upload_meta(value: &Value) -> Result<UploadMeta, AttachmentDbError> {
    serde_json::from_value(value.clone()).map_err(|_| AttachmentDbError::InvalidInput)
}

async fn lock_workspace_storage(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT pg_advisory_xact_lock($1, $2)")
        .bind(STORAGE_LOCK_NAMESPACE)
        .bind(lock_key_from_uuid(workspace_id))
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn with_upload_xact_lock(
    tx: &mut Transaction<'_, Postgres>,
    attachment_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT pg_advisory_xact_lock($1, $2)")
        .bind(ATTACHMENT_LOCK_NAMESPACE)
        .bind(lock_key_from_uuid(attachment_id))
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Source `countWorkspaceReservedBytes`: every row of the workspace counts,
/// stored ones included, so the limit bounds total storage, not just uploads
/// in flight.
async fn count_reserved_bytes(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
) -> Result<i64, sqlx::Error> {
    let row: (i64,) = sqlx::query_as(
        r#"
        SELECT COALESCE(SUM(reserved_size_bytes), 0)::bigint
        FROM fvoci.attachments
        WHERE workspace_id = $1
        "#,
    )
    .bind(workspace_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(row.0)
}

fn row_to_attachment(row: &sqlx::postgres::PgRow) -> AttachmentRow {
    AttachmentRow {
        id: row.get("id"),
        workspace_id: row.get("workspace_id"),
        document_id: row.get("document_id"),
        task_id: row.get("task_id"),
        uploader_id: row.get("uploader_id"),
        status: row.get("status"),
        name: row.get("name"),
        mime: row.get("mime"),
        declared_mime: row.get("declared_mime"),
        size_bytes: row.get("size_bytes"),
        reserved_size_bytes: row.get("reserved_size_bytes"),
        storage_key: row.get("storage_key"),
        image: row.get("image"),
        scan_status: row.get("scan_status"),
        extract_status: row.get("extract_status"),
        upload_meta: row.get("upload_meta"),
        variants: row.get("variants"),
        created_at: row.get("created_at"),
        completed_at: row.get("completed_at"),
    }
}

const ATTACHMENT_COLUMNS: &str = "id, workspace_id, document_id, task_id, uploader_id, status, \
     name, mime, declared_mime, size_bytes, reserved_size_bytes, storage_key, image, scan_status, \
     extract_status, upload_meta, variants, created_at, completed_at";

async fn fetch_attachment(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    attachment_id: Uuid,
) -> Result<Option<AttachmentRow>, sqlx::Error> {
    let row = sqlx::query(&format!(
        "SELECT {ATTACHMENT_COLUMNS} FROM fvoci.attachments WHERE workspace_id = $1 AND id = $2"
    ))
    .bind(workspace_id)
    .bind(attachment_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.map(|row| row_to_attachment(&row)))
}

/// The caller's effective level on an attachment parent and whether the parent
/// accepts writes (source `parentProjectId` + `requirePermission` +
/// `assertAttachmentParentWritable`). A trashed parent or a deleted project is
/// `NotFound`. `lock` takes the parent (and project) row locks writers use so
/// a concurrent trash/archive cannot interleave with the write.
struct ParentAccess {
    permission: ProjectPermission,
    project_id: Option<Uuid>,
    writable: Result<(), AttachmentDbError>,
}

async fn parent_access(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    parent: AttachmentParent,
    lock: bool,
) -> Result<Result<ParentAccess, AttachmentDbError>, sqlx::Error> {
    let (project_id, task_archived) = match parent {
        AttachmentParent::Document(document_id) => {
            let sql = if lock {
                "SELECT project_id, deleted_at FROM fvoci.documents WHERE workspace_id = $1 AND id = $2 FOR UPDATE"
            } else {
                "SELECT project_id, deleted_at FROM fvoci.documents WHERE workspace_id = $1 AND id = $2"
            };
            let row: Option<(Option<Uuid>, Option<DateTime<Utc>>)> = sqlx::query_as(sql)
                .bind(workspace_id)
                .bind(document_id)
                .fetch_optional(&mut **tx)
                .await?;
            match row {
                Some((project_id, None)) => (project_id, false),
                _ => return Ok(Err(AttachmentDbError::NotFound)),
            }
        }
        AttachmentParent::Task(task_id) => {
            let sql = if lock {
                "SELECT project_id, archived_at FROM fvoci.tasks WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL FOR NO KEY UPDATE"
            } else {
                "SELECT project_id, archived_at FROM fvoci.tasks WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL"
            };
            let row: Option<(Uuid, Option<DateTime<Utc>>)> = sqlx::query_as(sql)
                .bind(workspace_id)
                .bind(task_id)
                .fetch_optional(&mut **tx)
                .await?;
            match row {
                Some((project_id, archived_at)) => (Some(project_id), archived_at.is_some()),
                None => return Ok(Err(AttachmentDbError::NotFound)),
            }
        }
    };
    let Some(project_id) = project_id else {
        let AttachmentParent::Document(document_id) = parent else {
            unreachable!("tasks always belong to a project");
        };
        let permission =
            document_permission(tx, workspace_id, actor_user_id, document_id, true).await?;
        return Ok(Ok(ParentAccess {
            permission,
            project_id: None,
            writable: Ok(()),
        }));
    };
    let sql = if lock {
        "SELECT visibility, status FROM fvoci.projects WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL FOR NO KEY UPDATE"
    } else {
        "SELECT visibility, status FROM fvoci.projects WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL"
    };
    let project: Option<(String, String)> = sqlx::query_as(sql)
        .bind(workspace_id)
        .bind(project_id)
        .fetch_optional(&mut **tx)
        .await?;
    let Some((visibility, status)) = project else {
        return Ok(Err(AttachmentDbError::NotFound));
    };
    let permission = match membership_role(tx, workspace_id, actor_user_id).await? {
        None => ProjectPermission::None,
        Some(role) => {
            let member = project_member_role(tx, workspace_id, project_id, actor_user_id).await?;
            effective_permission(role, &visibility, member)
        }
    };
    let writable = if status == "archived" {
        Err(AttachmentDbError::ProjectArchived)
    } else if task_archived {
        Err(AttachmentDbError::TaskArchived)
    } else {
        Ok(())
    };
    Ok(Ok(ParentAccess {
        permission,
        project_id: Some(project_id),
        writable,
    }))
}

/// Source `requireUploadAccess`: edit on the parent, then uploader, then a
/// writable parent.
async fn require_upload_access(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    att: &AttachmentRow,
) -> Result<Result<(), AttachmentDbError>, sqlx::Error> {
    let access = match parent_access(tx, workspace_id, actor_user_id, att.parent(), true).await? {
        Ok(access) => access,
        Err(err) => return Ok(Err(err)),
    };
    if !access.permission.at_least(ProjectPermission::Edit) {
        return Ok(Err(AttachmentDbError::Forbidden));
    }
    if att.uploader_id != actor_user_id {
        return Ok(Err(AttachmentDbError::UploadForbidden));
    }
    Ok(access.writable)
}

async fn require_view_access(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    att: &AttachmentRow,
) -> Result<Result<ParentAccess, AttachmentDbError>, sqlx::Error> {
    let access = match parent_access(tx, workspace_id, actor_user_id, att.parent(), false).await? {
        Ok(access) => access,
        Err(err) => return Ok(Err(err)),
    };
    if !access.permission.at_least(ProjectPermission::View) {
        return Ok(Err(AttachmentDbError::Forbidden));
    }
    Ok(Ok(access))
}

async fn recheck_upload_write_access(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    attachment_id: Uuid,
) -> Result<Result<AttachmentRow, AttachmentDbError>, sqlx::Error> {
    lock_membership_users(tx, &[actor_user_id]).await?;
    if !recheck_session(tx, actor_user_id, session_id).await? {
        return Ok(Err(AttachmentDbError::Forbidden));
    }
    if !workspace_is_live(tx, workspace_id).await? {
        return Ok(Err(AttachmentDbError::NotFound));
    }
    let att = match fetch_attachment(tx, workspace_id, attachment_id).await? {
        Some(att) => att,
        None => return Ok(Err(AttachmentDbError::NotFound)),
    };
    match require_upload_access(tx, workspace_id, actor_user_id, &att).await? {
        Ok(()) => {}
        Err(err) => return Ok(Err(err)),
    }
    if att.status != "uploading" && att.status != "assembling" && att.status != "stored" {
        return Ok(Err(AttachmentDbError::UploadState));
    }
    Ok(Ok(att))
}

async fn record_attachment_event(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    verb: &str,
    attachment_id: Uuid,
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
            target_type: Some("attachment".to_string()),
            target_id: Some(attachment_id),
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
            target_type: Some("attachment".to_string()),
            target_id: Some(attachment_id),
            payload,
            ip: client_ip.map(str::to_string),
        },
    )
    .await?;
    Ok(())
}

/// Source event payload parent fields: `documentId` (null for a task
/// attachment) plus `taskId` when the parent is a task.
fn parent_payload(att: &AttachmentRow, payload: &mut serde_json::Map<String, Value>) {
    payload.insert(
        "documentId".into(),
        att.document_id
            .map(|id| Value::String(id.to_string()))
            .unwrap_or(Value::Null),
    );
    if let Some(task_id) = att.task_id {
        payload.insert("taskId".into(), Value::String(task_id.to_string()));
    }
}

/// What a new upload reserves a row against: a parent from the URL, or the
/// parent of an HWP/HWPX attachment being saved as an edited copy (source
/// `createDerivedCopyUpload`).
#[derive(Debug, Clone, Copy)]
pub enum UploadReservation {
    Target(UploadTarget),
    DerivedCopy { source_attachment_id: Uuid },
}

/// Authorizes a reservation under the caller's transaction and returns the
/// parent the new row hangs off plus the source's declared MIME for a copy.
async fn authorize_reservation(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    reservation: UploadReservation,
) -> Result<Result<(AttachmentParent, Option<String>), AttachmentDbError>, sqlx::Error> {
    match reservation {
        UploadReservation::Target(target) => {
            let (parent, affiliation) = match target {
                UploadTarget::WikiDocument(id) => (AttachmentParent::Document(id), Some(None)),
                UploadTarget::ProjectDocument {
                    project_id,
                    document_id,
                } => (
                    AttachmentParent::Document(document_id),
                    Some(Some(project_id)),
                ),
                UploadTarget::Task(id) => (AttachmentParent::Task(id), None),
            };
            let access = match parent_access(tx, workspace_id, actor_user_id, parent, true).await? {
                Ok(access) => access,
                Err(err) => return Ok(Err(err)),
            };
            if affiliation.is_some_and(|expected| expected != access.project_id) {
                return Ok(Err(AttachmentDbError::NotFound));
            }
            if !access.permission.at_least(ProjectPermission::Edit) {
                return Ok(Err(AttachmentDbError::Forbidden));
            }
            if let Err(err) = access.writable {
                return Ok(Err(err));
            }
            Ok(Ok((parent, None)))
        }
        UploadReservation::DerivedCopy {
            source_attachment_id,
        } => {
            let Some(source) = fetch_attachment(tx, workspace_id, source_attachment_id).await?
            else {
                return Ok(Err(AttachmentDbError::NotFound));
            };
            let access = match parent_access(tx, workspace_id, actor_user_id, source.parent(), true)
                .await?
            {
                Ok(access) => access,
                Err(err) => return Ok(Err(err)),
            };
            if !access.permission.at_least(ProjectPermission::View) {
                return Ok(Err(AttachmentDbError::Forbidden));
            }
            if source.status != "stored" {
                return Ok(Err(AttachmentDbError::NotFound));
            }
            if source.scan_status == "infected" {
                return Ok(Err(AttachmentDbError::Infected));
            }
            if !is_hwp_attachment(&source.name, &source.mime) {
                return Ok(Err(AttachmentDbError::NotHwp));
            }
            if !access.permission.at_least(ProjectPermission::Edit) {
                return Ok(Err(AttachmentDbError::Forbidden));
            }
            if let Err(err) = access.writable {
                return Ok(Err(err));
            }
            Ok(Ok((source.parent(), source.declared_mime)))
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn create_upload(
    pool: &PgPool,
    storage: &ObjectStorage,
    limits: &UploadLimits,
    quota: &StorageQuota,
    workspace_id: Uuid,
    reservation: UploadReservation,
    actor_user_id: Uuid,
    session_id: Uuid,
    input: CreateUploadInput,
    _client_ip: Option<&str>,
) -> Result<Result<(AttachmentRow, UploadMeta), AttachmentDbError>, sqlx::Error> {
    if input.size_bytes > limits.max_file_size_bytes {
        return Ok(Err(AttachmentDbError::TooLarge));
    }
    let part_count =
        ((input.size_bytes + limits.part_size_bytes - 1) / limits.part_size_bytes) as i32;
    if part_count > MAX_PART_COUNT {
        return Ok(Err(AttachmentDbError::InvalidInput));
    }
    let attachment_id = Uuid::now_v7();
    let storage_key = Uuid::now_v7().to_string();
    let mut upload_meta = UploadMeta {
        part_size_bytes: limits.part_size_bytes,
        part_count,
        declared_size_bytes: input.size_bytes,
        upload_ref: None,
    };

    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    lock_membership_users(&mut tx, &[actor_user_id]).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(AttachmentDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(AttachmentDbError::NotFound));
    }
    membership_role_for_update(&mut tx, workspace_id, actor_user_id).await?;
    let (parent, inherited_mime) =
        match authorize_reservation(&mut tx, workspace_id, actor_user_id, reservation).await? {
            Ok(v) => v,
            Err(err) => {
                tx.rollback().await?;
                return Ok(Err(err));
            }
        };
    lock_workspace_storage(&mut tx, workspace_id).await?;
    let reserved = count_reserved_bytes(&mut tx, workspace_id).await?;
    if let Err(err) = quota.check(reserved, input.size_bytes) {
        tx.rollback().await?;
        return Ok(Err(match err {
            StorageQuotaError::Upload => AttachmentDbError::UploadLimit,
            StorageQuotaError::Storage => AttachmentDbError::StorageLimit,
        }));
    }

    let (document_id, task_id) = match parent {
        AttachmentParent::Document(id) => (Some(id), None),
        AttachmentParent::Task(id) => (None, Some(id)),
    };
    sqlx::query(
        r#"
        INSERT INTO fvoci.attachments (
            id, workspace_id, document_id, task_id, uploader_id, status, name, declared_mime,
            reserved_size_bytes, storage_key, upload_meta
        ) VALUES ($1, $2, $3, $4, $5, 'uploading', $6, $7, $8, $9, $10)
        "#,
    )
    .bind(attachment_id)
    .bind(workspace_id)
    .bind(document_id)
    .bind(task_id)
    .bind(actor_user_id)
    .bind(&input.name)
    .bind(input.declared_mime.or(inherited_mime))
    .bind(input.size_bytes)
    .bind(&storage_key)
    .bind(json!(upload_meta))
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    match storage.create_multipart(&storage_key).await {
        Ok(None) => {}
        Ok(Some(upload_ref)) => {
            upload_meta.upload_ref = Some(upload_ref.clone());
            if let Err(err) =
                persist_upload_ref(pool, workspace_id, attachment_id, &upload_ref).await
            {
                let _ = cleanup_reserved_upload(
                    pool,
                    storage,
                    workspace_id,
                    attachment_id,
                    &storage_key,
                )
                .await;
                return Err(err);
            }
        }
        Err(err) => {
            let _ =
                cleanup_reserved_upload(pool, storage, workspace_id, attachment_id, &storage_key)
                    .await;
            return Err(sqlx::Error::Io(std::io::Error::other(err.to_string())));
        }
    }

    let row = fetch_after_create(pool, workspace_id, attachment_id).await?;
    Ok(Ok((row, upload_meta)))
}

async fn fetch_after_create(
    pool: &PgPool,
    workspace_id: Uuid,
    attachment_id: Uuid,
) -> Result<AttachmentRow, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let row = fetch_attachment(&mut tx, workspace_id, attachment_id)
        .await?
        .expect("attachment row");
    tx.commit().await?;
    Ok(row)
}

/// Aborts every multipart upload that could still publish `storage_key`, then
/// removes any published object. Errors propagate so callers keep the row and
/// a later sweep retries instead of orphaning remote state.
async fn persist_upload_ref(
    pool: &PgPool,
    workspace_id: Uuid,
    attachment_id: Uuid,
    upload_ref: &str,
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let updated = sqlx::query(
        r#"
        UPDATE fvoci.attachments
        SET upload_meta = jsonb_set(COALESCE(upload_meta, '{}'::jsonb), '{upload_ref}', to_jsonb($3::text), true)
        WHERE workspace_id = $1 AND id = $2 AND status = 'uploading'
        "#,
    )
    .bind(workspace_id)
    .bind(attachment_id)
    .bind(upload_ref)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    if updated.rows_affected() == 0 {
        return Err(sqlx::Error::RowNotFound);
    }
    Ok(())
}

pub async fn cleanup_reserved_upload(
    pool: &PgPool,
    storage: &ObjectStorage,
    workspace_id: Uuid,
    attachment_id: Uuid,
    storage_key: &str,
) -> Result<(), sqlx::Error> {
    if storage.purge_key(storage_key).await.is_err() {
        // Keep the reserved row: the stale-upload sweep retries the storage
        // cleanup after the TTL instead of losing track of a live upload.
        tracing::warn!(%attachment_id, "reserved upload storage cleanup failed; left for sweep");
        return Ok(());
    }
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    sqlx::query("DELETE FROM fvoci.attachments WHERE workspace_id = $1 AND id = $2")
        .bind(workspace_id)
        .bind(attachment_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn authorize_upload_part(
    pool: &PgPool,
    workspace_id: Uuid,
    attachment_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    part_number: i32,
) -> Result<Result<(String, u64, Option<String>), AttachmentDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    lock_membership_users(&mut tx, &[actor_user_id]).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(AttachmentDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(AttachmentDbError::NotFound));
    }
    let att = match fetch_attachment(&mut tx, workspace_id, attachment_id).await? {
        Some(att) => att,
        None => {
            tx.rollback().await?;
            return Ok(Err(AttachmentDbError::NotFound));
        }
    };
    match require_upload_access(&mut tx, workspace_id, actor_user_id, &att).await? {
        Ok(()) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    }
    if att.status != "uploading" {
        tx.rollback().await?;
        return Ok(Err(AttachmentDbError::UploadState));
    }
    let meta = match parse_upload_meta(att.upload_meta.as_ref().unwrap_or(&json!({}))) {
        Ok(meta) => meta,
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    };
    if part_number > meta.part_count {
        tx.rollback().await?;
        return Ok(Err(AttachmentDbError::InvalidInput));
    }
    let max_bytes = if part_number < meta.part_count {
        meta.part_size_bytes as u64
    } else {
        (meta.declared_size_bytes - meta.part_size_bytes * (meta.part_count as i64 - 1)) as u64
    };
    let storage_key = att.storage_key.clone();
    let upload_ref = meta.upload_ref.clone();
    tx.commit().await?;
    Ok(Ok((storage_key, max_bytes, upload_ref)))
}

#[allow(clippy::too_many_arguments)]
pub async fn commit_upload_part(
    pool: &PgPool,
    storage: &ObjectStorage,
    workspace_id: Uuid,
    attachment_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    part_number: i32,
    staged: &mut StagedPart,
) -> Result<Result<crate::attachments::PartInfo, AttachmentDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    with_upload_xact_lock(&mut tx, attachment_id).await?;
    let att = match recheck_upload_write_access(
        &mut tx,
        workspace_id,
        actor_user_id,
        session_id,
        attachment_id,
    )
    .await?
    {
        Ok(att) => att,
        Err(err) => {
            tx.rollback().await?;
            ObjectStorage::discard_staged_part(staged).await;
            return Ok(Err(err));
        }
    };
    if att.status != "uploading" {
        tx.rollback().await?;
        ObjectStorage::discard_staged_part(staged).await;
        return Ok(Err(AttachmentDbError::UploadState));
    }
    let meta = parse_upload_meta(att.upload_meta.as_ref().unwrap_or(&json!({})))
        .map_err(|e| sqlx::Error::Io(std::io::Error::other(e.to_string())))?;
    if part_number > meta.part_count {
        tx.rollback().await?;
        ObjectStorage::discard_staged_part(staged).await;
        return Ok(Err(AttachmentDbError::InvalidInput));
    }
    let storage_key = att.storage_key.clone();
    let published = match storage
        .publish_staged_part(&storage_key, part_number, staged)
        .await
    {
        Ok(part) => part,
        Err(StorageError::UploadGone) => {
            tx.rollback().await?;
            ObjectStorage::discard_staged_part(staged).await;
            return Ok(Err(AttachmentDbError::UploadState));
        }
        Err(StorageError::PartTooLarge) => {
            tx.rollback().await?;
            ObjectStorage::discard_staged_part(staged).await;
            return Ok(Err(AttachmentDbError::PartTooLarge));
        }
        Err(err) => {
            tx.rollback().await?;
            ObjectStorage::discard_staged_part(staged).await;
            return Err(sqlx::Error::Io(std::io::Error::other(err.to_string())));
        }
    };
    tx.commit().await?;
    Ok(Ok(published))
}

pub async fn resume_upload(
    pool: &PgPool,
    storage: &ObjectStorage,
    workspace_id: Uuid,
    attachment_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<
    Result<(AttachmentRow, UploadMeta, Vec<(i32, String)>, Vec<i32>), AttachmentDbError>,
    sqlx::Error,
> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    lock_membership_users(&mut tx, &[actor_user_id]).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(AttachmentDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(AttachmentDbError::NotFound));
    }
    let att = match fetch_attachment(&mut tx, workspace_id, attachment_id).await? {
        Some(att) => att,
        None => {
            tx.rollback().await?;
            return Ok(Err(AttachmentDbError::NotFound));
        }
    };
    match require_upload_access(&mut tx, workspace_id, actor_user_id, &att).await? {
        Ok(()) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    }
    if att.status != "uploading" {
        tx.rollback().await?;
        return Ok(Err(AttachmentDbError::UploadState));
    }
    let meta = match parse_upload_meta(att.upload_meta.as_ref().unwrap_or(&json!({}))) {
        Ok(meta) => meta,
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    };
    tx.commit().await?;
    let uploaded = storage
        .list_parts(&att.storage_key, meta.upload_ref.as_deref())
        .await
        .map_err(|e| sqlx::Error::Io(std::io::Error::other(e.to_string())))?;
    let done = uploaded
        .iter()
        .map(|p| (p.part_number, p.etag.clone()))
        .collect::<Vec<_>>();
    let done_set = done
        .iter()
        .map(|(n, _)| *n)
        .collect::<std::collections::HashSet<_>>();
    let remaining = (1..=meta.part_count)
        .filter(|n| !done_set.contains(n))
        .collect();
    Ok(Ok((att, meta, done, remaining)))
}

#[allow(clippy::too_many_arguments)]
pub async fn complete_upload(
    pool: &PgPool,
    storage: &ObjectStorage,
    workspace_id: Uuid,
    attachment_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    parts: Vec<(i32, String)>,
    client_ip: Option<&str>,
) -> Result<Result<AttachmentRow, AttachmentDbError>, sqlx::Error> {
    const ASSEMBLE_WAIT: Duration = Duration::from_secs(60 * 60);
    const ASSEMBLE_POLL: Duration = Duration::from_millis(100);
    let deadline = std::time::Instant::now() + ASSEMBLE_WAIT;

    loop {
        match try_complete_owned(
            pool,
            storage,
            workspace_id,
            attachment_id,
            actor_user_id,
            session_id,
            &parts,
            client_ip,
        )
        .await?
        {
            CompleteAttempt::Done(row) => return Ok(Ok(row)),
            CompleteAttempt::Denied(err) => return Ok(Err(err)),
            CompleteAttempt::Retry => {}
        }

        if std::time::Instant::now() >= deadline {
            return Ok(Err(AttachmentDbError::UploadState));
        }
        tokio::time::sleep(ASSEMBLE_POLL).await;
    }
}

#[allow(clippy::large_enum_variant)]
enum CompleteAttempt {
    Done(AttachmentRow),
    Denied(AttachmentDbError),
    Retry,
}

#[allow(clippy::too_many_arguments)]
async fn try_complete_owned(
    pool: &PgPool,
    storage: &ObjectStorage,
    workspace_id: Uuid,
    attachment_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    parts: &[(i32, String)],
    client_ip: Option<&str>,
) -> Result<CompleteAttempt, sqlx::Error> {
    let Some(mut lock) = AttachmentSessionLock::try_acquire(pool, attachment_id).await? else {
        let mut tx = pool.begin().await?;
        set_tenant(&mut tx, workspace_id).await?;
        lock_membership_users(&mut tx, &[actor_user_id]).await?;
        if !recheck_session(&mut tx, actor_user_id, session_id).await? {
            tx.rollback().await?;
            return Ok(CompleteAttempt::Denied(AttachmentDbError::Forbidden));
        }
        if !workspace_is_live(&mut tx, workspace_id).await? {
            tx.rollback().await?;
            return Ok(CompleteAttempt::Denied(AttachmentDbError::NotFound));
        }
        let att = match fetch_attachment(&mut tx, workspace_id, attachment_id).await? {
            Some(att) => att,
            None => {
                tx.rollback().await?;
                return Ok(CompleteAttempt::Denied(AttachmentDbError::NotFound));
            }
        };
        match require_upload_access(&mut tx, workspace_id, actor_user_id, &att).await? {
            Ok(()) => {}
            Err(err) => {
                tx.rollback().await?;
                return Ok(CompleteAttempt::Denied(err));
            }
        }
        if att.status == "stored" {
            tx.commit().await?;
            return Ok(CompleteAttempt::Done(att));
        }
        tx.rollback().await?;
        return Ok(CompleteAttempt::Retry);
    };

    let result = complete_owned_inner(
        &mut lock,
        storage,
        workspace_id,
        attachment_id,
        actor_user_id,
        session_id,
        parts,
        client_ip,
    )
    .await;
    lock.release().await;
    result
}

#[allow(clippy::too_many_arguments)]
async fn complete_owned_inner(
    lock: &mut AttachmentSessionLock,
    storage: &ObjectStorage,
    workspace_id: Uuid,
    attachment_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    parts: &[(i32, String)],
    client_ip: Option<&str>,
) -> Result<CompleteAttempt, sqlx::Error> {
    let mut tx = lock.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    with_upload_xact_lock(&mut tx, attachment_id).await?;
    let att = match recheck_upload_write_access(
        &mut tx,
        workspace_id,
        actor_user_id,
        session_id,
        attachment_id,
    )
    .await?
    {
        Ok(att) => att,
        Err(err) => {
            tx.rollback().await?;
            return Ok(CompleteAttempt::Denied(err));
        }
    };
    if att.status == "stored" {
        tx.commit().await?;
        return Ok(CompleteAttempt::Done(att));
    }

    let meta = parse_upload_meta(att.upload_meta.as_ref().unwrap_or(&json!({})))
        .map_err(|e| sqlx::Error::Io(std::io::Error::other(e.to_string())))?;
    if parts.len() as i32 != meta.part_count {
        tx.rollback().await?;
        return Ok(CompleteAttempt::Denied(AttachmentDbError::InvalidInput));
    }

    let storage_key = att.storage_key.clone();
    let att_name = att.name.clone();
    let needs_assembly = if att.status == "assembling" {
        true
    } else if att.status == "uploading" {
        // Retry must durably finish a removal even if a previous attempt
        // already unlinked the payload before cancellation or an IO error.
        storage
            .discard_uncommitted_payload(&storage_key)
            .await
            .map_err(|e| sqlx::Error::Io(std::io::Error::other(e.to_string())))?;
        let rows = sqlx::query(
            "UPDATE fvoci.attachments SET status = 'assembling' WHERE workspace_id = $1 AND id = $2 AND status = 'uploading'",
        )
        .bind(workspace_id)
        .bind(attachment_id)
        .execute(&mut *tx)
        .await?;
        if rows.rows_affected() == 0 {
            tx.rollback().await?;
            return Ok(CompleteAttempt::Retry);
        }
        true
    } else {
        tx.rollback().await?;
        return Ok(CompleteAttempt::Denied(AttachmentDbError::UploadState));
    };
    tx.commit().await?;

    if needs_assembly {
        let assemble = storage
            .assemble_multipart(&storage_key, meta.upload_ref.as_deref(), parts)
            .await;
        match assemble {
            Ok(size) if size as i64 != meta.declared_size_bytes => {
                let _ = storage.delete_object(&storage_key).await;
                delete_attachment_row(lock, workspace_id, attachment_id).await?;
                return Ok(CompleteAttempt::Denied(AttachmentDbError::InvalidInput));
            }
            // EntityTooSmall: a non-final part below the S3 minimum is a
            // client-side part list problem, not a server failure.
            Err(StorageError::EtagMismatch | StorageError::PartTooSmall) => {
                revert_assembling_on_conn(lock, workspace_id, attachment_id).await?;
                return Ok(CompleteAttempt::Denied(AttachmentDbError::EtagMismatch));
            }
            Err(StorageError::UploadGone) => {
                revert_assembling_on_conn(lock, workspace_id, attachment_id).await?;
                return Ok(CompleteAttempt::Denied(AttachmentDbError::UploadState));
            }
            Err(err) => {
                revert_assembling_on_conn(lock, workspace_id, attachment_id).await?;
                return Err(sqlx::Error::Io(std::io::Error::other(err.to_string())));
            }
            Ok(_) => {}
        }
    }

    let size_bytes = storage
        .head(&storage_key)
        .await
        .map_err(|e| sqlx::Error::Io(std::io::Error::other(e.to_string())))?
        .ok_or_else(|| sqlx::Error::Io(std::io::Error::other("missing payload")))?;
    if size_bytes as i64 != meta.declared_size_bytes {
        let _ = storage.delete_object(&storage_key).await;
        delete_attachment_row(lock, workspace_id, attachment_id).await?;
        return Ok(CompleteAttempt::Denied(AttachmentDbError::InvalidInput));
    }

    let mime = storage
        .sniff_mime(&storage_key)
        .await
        .map_err(|e| sqlx::Error::Io(std::io::Error::other(e.to_string())))?;
    let image = is_image_mime(&mime);
    let extract_status = initial_extract_status(&att_name, &mime);
    let preview_status = if image && crate::attachments::preview::preview_mime_supported(&mime) {
        "pending"
    } else {
        "skipped"
    };

    #[cfg(feature = "db-tests")]
    test_barrier::wait_pre_mark_stored_barrier(attachment_id).await;

    let mut tx = lock.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    with_upload_xact_lock(&mut tx, attachment_id).await?;
    let att = match recheck_upload_write_access(
        &mut tx,
        workspace_id,
        actor_user_id,
        session_id,
        attachment_id,
    )
    .await?
    {
        Ok(att) => att,
        Err(err) => {
            tx.rollback().await?;
            return Ok(CompleteAttempt::Denied(err));
        }
    };
    if att.status == "stored" {
        tx.commit().await?;
        let _ = storage.finalize_multipart(&storage_key).await;
        return Ok(CompleteAttempt::Done(att));
    }
    if att.status != "uploading" && att.status != "assembling" {
        tx.rollback().await?;
        return Ok(CompleteAttempt::Denied(AttachmentDbError::UploadState));
    }

    let updated = sqlx::query(
        r#"
        UPDATE fvoci.attachments
        SET status = 'stored', mime = $3, size_bytes = $4, image = $5,
            scan_status = 'skipped', extract_status = $6, preview_status = $7,
            upload_meta = NULL, completed_at = now()
        WHERE workspace_id = $1 AND id = $2 AND status IN ('uploading', 'assembling')
        "#,
    )
    .bind(workspace_id)
    .bind(attachment_id)
    .bind(&mime)
    .bind(size_bytes as i64)
    .bind(image)
    .bind(extract_status)
    .bind(preview_status)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() == 0 {
        let stored = fetch_attachment(&mut tx, workspace_id, attachment_id).await?;
        if stored.as_ref().is_some_and(|row| row.status == "stored") {
            tx.commit().await?;
            let _ = storage.finalize_multipart(&storage_key).await;
            return Ok(CompleteAttempt::Done(stored.expect("stored row")));
        }
        tx.rollback().await?;
        return Ok(CompleteAttempt::Retry);
    }
    let mut payload = serde_json::Map::new();
    payload.insert("name".into(), json!(att_name));
    parent_payload(&att, &mut payload);
    payload.insert("sizeBytes".into(), json!(size_bytes));
    payload.insert("mime".into(), json!(mime));
    record_attachment_event(
        &mut tx,
        workspace_id,
        actor_user_id,
        "attachment.completed",
        attachment_id,
        Value::Object(payload),
        client_ip,
    )
    .await?;
    let row = fetch_attachment(&mut tx, workspace_id, attachment_id)
        .await?
        .expect("stored row");
    tx.commit().await?;
    let _ = storage.finalize_multipart(&storage_key).await;
    Ok(CompleteAttempt::Done(row))
}

async fn delete_attachment_row(
    lock: &mut AttachmentSessionLock,
    workspace_id: Uuid,
    attachment_id: Uuid,
) -> Result<(), sqlx::Error> {
    let mut tx = lock.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    with_upload_xact_lock(&mut tx, attachment_id).await?;
    sqlx::query("DELETE FROM fvoci.attachments WHERE workspace_id = $1 AND id = $2")
        .bind(workspace_id)
        .bind(attachment_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

async fn revert_assembling_on_conn(
    lock: &mut AttachmentSessionLock,
    workspace_id: Uuid,
    attachment_id: Uuid,
) -> Result<(), sqlx::Error> {
    let mut tx = lock.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    with_upload_xact_lock(&mut tx, attachment_id).await?;
    sqlx::query(
        "UPDATE fvoci.attachments SET status = 'uploading' WHERE workspace_id = $1 AND id = $2 AND status = 'assembling'",
    )
    .bind(workspace_id)
    .bind(attachment_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn get_attachment_meta(
    pool: &PgPool,
    workspace_id: Uuid,
    attachment_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<AttachmentRow, AttachmentDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(AttachmentDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(AttachmentDbError::NotFound));
    }
    let att = match fetch_attachment(&mut tx, workspace_id, attachment_id).await? {
        Some(att) => att,
        None => {
            tx.rollback().await?;
            return Ok(Err(AttachmentDbError::NotFound));
        }
    };
    match require_view_access(&mut tx, workspace_id, actor_user_id, &att).await {
        Ok(Ok(_)) => {}
        Ok(Err(err)) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
        Err(err) => return Err(err),
    }
    if att.status != "stored" {
        tx.rollback().await?;
        return Ok(Err(AttachmentDbError::NotFound));
    }
    tx.commit().await?;
    Ok(Ok(att))
}

pub async fn open_download(
    pool: &PgPool,
    workspace_id: Uuid,
    attachment_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<AttachmentRow, AttachmentDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(AttachmentDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(AttachmentDbError::NotFound));
    }
    let att = match fetch_attachment(&mut tx, workspace_id, attachment_id).await? {
        Some(att) => att,
        None => {
            tx.rollback().await?;
            return Ok(Err(AttachmentDbError::NotFound));
        }
    };
    match require_view_access(&mut tx, workspace_id, actor_user_id, &att).await {
        Ok(Ok(_)) => {}
        Ok(Err(err)) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
        Err(err) => return Err(err),
    }
    if att.status != "stored" {
        tx.rollback().await?;
        return Ok(Err(AttachmentDbError::NotFound));
    }
    if att.scan_status == "infected" {
        tx.rollback().await?;
        return Ok(Err(AttachmentDbError::Infected));
    }
    tx.commit().await?;
    Ok(Ok(att))
}

/// Stored extract text of an attachment the caller already opened through
/// [`open_download`] (source `att.extractText`).
pub async fn attachment_extract_text(
    pool: &PgPool,
    workspace_id: Uuid,
    attachment_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let text: Option<String> = sqlx::query_scalar(
        "SELECT extract_text FROM fvoci.attachments WHERE workspace_id = $1 AND id = $2 AND status = 'stored'",
    )
    .bind(workspace_id)
    .bind(attachment_id)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(text)
}

/// Parent kind of an attachment, for API-token scope checks before an
/// operation runs (source `authorizeTarget`). The parent never changes after
/// insert, so reading it ahead of the operation's own transaction is sound.
pub async fn attachment_parent(
    pool: &PgPool,
    workspace_id: Uuid,
    attachment_id: Uuid,
) -> Result<Option<AttachmentParent>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let att = fetch_attachment(&mut tx, workspace_id, attachment_id).await?;
    tx.commit().await?;
    Ok(att.map(|att| att.parent()))
}

/// Source `listTaskAttachments`: view on the task's project, every row of the
/// task (any status), oldest first.
pub async fn list_task_attachments(
    pool: &PgPool,
    workspace_id: Uuid,
    task_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<Vec<AttachmentRow>, AttachmentDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(AttachmentDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(AttachmentDbError::NotFound));
    }
    let parent = AttachmentParent::Task(task_id);
    match parent_access(&mut tx, workspace_id, actor_user_id, parent, false).await? {
        Ok(access) if access.permission.at_least(ProjectPermission::View) => {}
        Ok(_) => {
            tx.rollback().await?;
            return Ok(Err(AttachmentDbError::Forbidden));
        }
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    }
    let rows = sqlx::query(&format!(
        "SELECT {ATTACHMENT_COLUMNS} FROM fvoci.attachments \
         WHERE workspace_id = $1 AND task_id = $2 ORDER BY created_at, id"
    ))
    .bind(workspace_id)
    .bind(task_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(rows.iter().map(row_to_attachment).collect()))
}

/// Source `purgeAttachment`: the uploader needs edit on the parent, anyone
/// else manage; a stored attachment also needs a writable parent. The row and
/// its `attachment.deleted` event/audit commit together; the delete trigger
/// journals every storage key in the same transaction, and
/// [`reclaim_attachment_objects`] removes the objects afterwards.
pub async fn delete_attachment(
    pool: &PgPool,
    workspace_id: Uuid,
    attachment_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    client_ip: Option<&str>,
) -> Result<Result<(), AttachmentDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    lock_membership_users(&mut tx, &[actor_user_id]).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(AttachmentDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(AttachmentDbError::NotFound));
    }
    // No upload advisory lock here: a complete holds it for its whole
    // assembly. The row lock taken by DELETE orders this against complete's
    // mark-stored UPDATE, which then finds no row, and the object journal
    // waits for that complete's session lock before reclaiming the key.
    let Some(att) = fetch_attachment(&mut tx, workspace_id, attachment_id).await? else {
        tx.rollback().await?;
        return Ok(Err(AttachmentDbError::NotFound));
    };
    let access =
        match parent_access(&mut tx, workspace_id, actor_user_id, att.parent(), true).await? {
            Ok(access) => access,
            Err(err) => {
                tx.rollback().await?;
                return Ok(Err(err));
            }
        };
    let needed = if att.uploader_id == actor_user_id {
        ProjectPermission::Edit
    } else {
        ProjectPermission::Manage
    };
    if !access.permission.at_least(needed) {
        tx.rollback().await?;
        return Ok(Err(AttachmentDbError::Forbidden));
    }
    if att.status == "stored" {
        if let Err(err) = access.writable {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    }
    let deleted = sqlx::query("DELETE FROM fvoci.attachments WHERE workspace_id = $1 AND id = $2")
        .bind(workspace_id)
        .bind(attachment_id)
        .execute(&mut *tx)
        .await?;
    if deleted.rows_affected() == 0 {
        tx.rollback().await?;
        return Ok(Err(AttachmentDbError::NotFound));
    }
    let mut payload = serde_json::Map::new();
    payload.insert("name".into(), json!(att.name));
    parent_payload(&att, &mut payload);
    payload.insert(
        "projectId".into(),
        access
            .project_id
            .map(|id| Value::String(id.to_string()))
            .unwrap_or(Value::Null),
    );
    record_attachment_event(
        &mut tx,
        workspace_id,
        actor_user_id,
        "attachment.deleted",
        attachment_id,
        Value::Object(payload),
        client_ip,
    )
    .await?;
    tx.commit().await?;
    Ok(Ok(()))
}

#[derive(Debug, Clone)]
pub struct AttachmentEditContext {
    pub source_attachment_id: Uuid,
    pub name: String,
    pub mime: String,
    pub editable: bool,
}

/// Source `getAttachmentEditContext`: view access to a stored, clean
/// attachment; `editable` only for HWP/HWPX the caller may copy-edit.
pub async fn attachment_edit_context(
    pool: &PgPool,
    workspace_id: Uuid,
    attachment_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<AttachmentEditContext, AttachmentDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(AttachmentDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(AttachmentDbError::NotFound));
    }
    let Some(att) = fetch_attachment(&mut tx, workspace_id, attachment_id).await? else {
        tx.rollback().await?;
        return Ok(Err(AttachmentDbError::NotFound));
    };
    let access = match require_view_access(&mut tx, workspace_id, actor_user_id, &att).await? {
        Ok(access) => access,
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    };
    if att.status != "stored" {
        tx.rollback().await?;
        return Ok(Err(AttachmentDbError::NotFound));
    }
    if att.scan_status == "infected" {
        tx.rollback().await?;
        return Ok(Err(AttachmentDbError::Infected));
    }
    let editable = is_hwp_attachment(&att.name, &att.mime)
        && access.permission.at_least(ProjectPermission::Edit)
        && access.writable.is_ok();
    tx.commit().await?;
    Ok(Ok(AttachmentEditContext {
        source_attachment_id: att.id,
        name: att.name,
        mime: att.mime,
        editable,
    }))
}

/// Retry delay for a journal row whose object is busy or failed to delete
/// (source `ATTACHMENT_OBJECT_RETRY_MS`).
pub const OBJECT_CLEANUP_RETRY: Duration = Duration::from_secs(60);

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ObjectCleanupStats {
    pub claimed: u32,
    pub reclaimed: u32,
    /// A complete for the same attachment held its upload session lock.
    pub busy: u32,
    pub failed: u32,
}

/// Drains due `attachment_object_cleanups` rows (source
/// `cleanupAttachmentObjects`), optionally only those of one attachment right
/// after it was deleted. Each row takes the attachment's upload session lock
/// so an in-flight complete cannot recreate a key after it was deleted; busy
/// or failed rows are rescheduled. A key still referenced by a live row is
/// never deleted (the journal row is simply dropped).
pub async fn reclaim_attachment_objects(
    pool: &PgPool,
    storage: &ObjectStorage,
    only: Option<(Uuid, Uuid)>,
    limit: i64,
) -> Result<ObjectCleanupStats, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let previous = set_system(&mut tx).await?;
    let rows: Vec<(Uuid, Uuid, Uuid, String)> = sqlx::query_as(
        r#"
        SELECT id, workspace_id, attachment_id, storage_key
        FROM fvoci.attachment_object_cleanups
        WHERE due_at <= clock_timestamp()
          AND ($1::uuid IS NULL OR (workspace_id = $1 AND attachment_id = $2))
        ORDER BY due_at, id
        LIMIT $3
        "#,
    )
    .bind(only.map(|(ws, _)| ws))
    .bind(only.map(|(_, id)| id))
    .bind(limit)
    .fetch_all(&mut *tx)
    .await?;
    restore_system(&mut tx, &previous).await?;
    tx.commit().await?;

    let mut stats = ObjectCleanupStats::default();
    for (id, workspace_id, attachment_id, key) in rows {
        stats.claimed += 1;
        let Some(mut lock) = AttachmentSessionLock::try_acquire(pool, attachment_id).await? else {
            stats.busy += 1;
            reschedule_object_cleanup(pool, id, false).await?;
            continue;
        };
        let outcome = reclaim_one_object(&mut lock, storage, id, workspace_id, &key).await;
        lock.release().await;
        match outcome {
            Ok(true) => stats.reclaimed += 1,
            Ok(false) => {
                stats.failed += 1;
                reschedule_object_cleanup(pool, id, true).await?;
            }
            Err(err) => {
                stats.failed += 1;
                tracing::warn!(%attachment_id, error = %err, "attachment.object_cleanup_failed");
                reschedule_object_cleanup(pool, id, true).await?;
            }
        }
    }
    Ok(stats)
}

async fn reclaim_one_object(
    lock: &mut AttachmentSessionLock,
    storage: &ObjectStorage,
    id: Uuid,
    workspace_id: Uuid,
    key: &str,
) -> Result<bool, sqlx::Error> {
    let mut tx = lock.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let referenced: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM fvoci.attachments
            WHERE workspace_id = $1
              AND (storage_key = $2 OR variants -> 'preview' ->> 'key' = $2)
        )
        "#,
    )
    .bind(workspace_id)
    .bind(key)
    .fetch_one(&mut *tx)
    .await?;
    if !referenced {
        if let Err(err) = storage.purge_key(key).await {
            tracing::warn!(error = %err, "attachment.object_cleanup_storage_failed");
            tx.rollback().await?;
            return Ok(false);
        }
        match storage.head(key).await {
            Ok(None) => {}
            _ => {
                tx.rollback().await?;
                return Ok(false);
            }
        }
    }
    sqlx::query("DELETE FROM fvoci.attachment_object_cleanups WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(true)
}

async fn reschedule_object_cleanup(
    pool: &PgPool,
    id: Uuid,
    failed: bool,
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    let previous = set_system(&mut tx).await?;
    sqlx::query(
        r#"
        UPDATE fvoci.attachment_object_cleanups
        SET due_at = clock_timestamp() + ($2 * interval '1 second'),
            attempts = attempts + CASE WHEN $3 THEN 1 ELSE 0 END
        WHERE id = $1
        "#,
    )
    .bind(id)
    .bind(OBJECT_CLEANUP_RETRY.as_secs() as f64)
    .bind(failed)
    .execute(&mut *tx)
    .await?;
    restore_system(&mut tx, &previous).await?;
    tx.commit().await?;
    Ok(())
}

impl std::fmt::Display for AttachmentDbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

#[derive(Debug, Clone)]
pub struct StaleUpload {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub created_at: chrono::DateTime<Utc>,
}

/// Position in the global `(created_at, id)` order of stale uploads.
pub type StaleUploadCursor = (chrono::DateTime<Utc>, Uuid);

/// At most `limit` incomplete uploads created before `cutoff`, across every
/// workspace, in global `(created_at, id)` order strictly after `after`.
/// Resuming from the previous batch's last row means rows that are skipped or
/// fail on every run cannot keep later rows (in any workspace) out of reach.
///
/// `fvoci.attachments` RLS has no system-context bypass, so this enumerates
/// workspaces under the system context (which `fvoci.workspaces` allows) and
/// reads each tenant's stale rows under that tenant's own context, all in one
/// read-only transaction served by `attachments_uploading_created_at_idx`.
pub async fn list_stale_uploading(
    pool: &PgPool,
    cutoff: chrono::DateTime<Utc>,
    after: Option<StaleUploadCursor>,
    limit: i64,
) -> Result<Vec<StaleUpload>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let previous = set_system(&mut tx).await?;
    let workspace_ids: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM fvoci.workspaces ORDER BY id")
            .fetch_all(&mut *tx)
            .await?;
    restore_system(&mut tx, &previous).await?;
    let (after_at, after_id) = match after {
        Some((at, id)) => (Some(at), Some(id)),
        None => (None, None),
    };
    let mut stale = Vec::new();
    for workspace_id in workspace_ids {
        set_tenant(&mut tx, workspace_id).await?;
        // Each workspace contributes at most `limit` rows; the global merge
        // below keeps the oldest `limit` of them.
        let rows: Vec<(Uuid, chrono::DateTime<Utc>)> = sqlx::query_as(
            r#"
            SELECT id, created_at
            FROM fvoci.attachments
            WHERE workspace_id = $1
              AND status IN ('uploading', 'assembling')
              AND created_at < $2
              AND ($3::timestamptz IS NULL OR (created_at, id) > ($3, $4::uuid))
            ORDER BY created_at ASC, id ASC
            LIMIT $5
            "#,
        )
        .bind(workspace_id)
        .bind(cutoff)
        .bind(after_at)
        .bind(after_id)
        .bind(limit)
        .fetch_all(&mut *tx)
        .await?;
        stale.extend(rows.into_iter().map(|(id, created_at)| StaleUpload {
            id,
            workspace_id,
            created_at,
        }));
    }
    tx.commit().await?;
    stale.sort_by_key(|row| (row.created_at, row.id));
    stale.truncate(usize::try_from(limit).unwrap_or(0));
    Ok(stale)
}

#[derive(Debug, Clone)]
pub struct StoredObject {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub storage_key: String,
    pub size_bytes: i64,
}

/// Every stored attachment's key and size in one workspace, read under that
/// workspace's tenant context (the app role has no cross-tenant bypass).
pub async fn list_workspace_stored_objects(
    pool: &PgPool,
    workspace_id: Uuid,
) -> Result<Vec<StoredObject>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let rows: Vec<(Uuid, String, i64)> = sqlx::query_as(
        r#"
        SELECT id, storage_key, size_bytes
        FROM fvoci.attachments
        WHERE workspace_id = $1 AND status = 'stored'
        ORDER BY id
        "#,
    )
    .bind(workspace_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(rows
        .into_iter()
        .map(|(id, storage_key, size_bytes)| StoredObject {
            id,
            workspace_id,
            storage_key,
            size_bytes,
        })
        .collect())
}

/// All workspace ids, including soft-deleted ones whose attachments still
/// hold storage until purge.
pub async fn list_all_workspace_ids(pool: &PgPool) -> Result<Vec<Uuid>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let previous = set_system(&mut tx).await?;
    let ids = sqlx::query_scalar("SELECT id FROM fvoci.workspaces ORDER BY id")
        .fetch_all(&mut *tx)
        .await?;
    restore_system(&mut tx, &previous).await?;
    tx.commit().await?;
    Ok(ids)
}

pub async fn gc_stale_upload_row(
    pool: &PgPool,
    storage: &ObjectStorage,
    workspace_id: Uuid,
    attachment_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let Some(mut lock) = AttachmentSessionLock::try_acquire(pool, attachment_id).await? else {
        return Ok(false);
    };
    let result = gc_stale_upload_locked(&mut lock, storage, workspace_id, attachment_id).await;
    lock.release().await;
    result
}

async fn gc_stale_upload_locked(
    lock: &mut AttachmentSessionLock,
    storage: &ObjectStorage,
    workspace_id: Uuid,
    attachment_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let mut tx = lock.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let row: Option<(String, String)> = sqlx::query_as(
        r#"
        SELECT status, storage_key
        FROM fvoci.attachments
        WHERE workspace_id = $1 AND id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(attachment_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((status, storage_key)) = row else {
        tx.rollback().await?;
        return Ok(false);
    };
    if status != "uploading" && status != "assembling" {
        tx.rollback().await?;
        return Ok(false);
    }
    tx.commit().await?;

    storage
        .purge_key(&storage_key)
        .await
        .map_err(|e| sqlx::Error::Io(std::io::Error::other(e.to_string())))?;

    let mut tx = lock.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let deleted = sqlx::query(
        "DELETE FROM fvoci.attachments WHERE workspace_id = $1 AND id = $2 AND status IN ('uploading', 'assembling')",
    )
    .bind(workspace_id)
    .bind(attachment_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(deleted.rows_affected() > 0)
}

#[cfg(feature = "db-tests")]
pub mod test_barrier {
    use std::collections::HashMap;
    use std::sync::{LazyLock, Mutex};

    use uuid::Uuid;

    type BarrierPair = (
        tokio::sync::oneshot::Sender<()>,
        tokio::sync::oneshot::Receiver<()>,
    );
    type BarrierMap = Mutex<HashMap<Uuid, BarrierPair>>;

    static BARRIERS: LazyLock<BarrierMap> = LazyLock::new(|| Mutex::new(HashMap::new()));

    pub struct PreMarkStoredBarrier {
        entered_rx: tokio::sync::oneshot::Receiver<()>,
        proceed_tx: Option<tokio::sync::oneshot::Sender<()>>,
    }

    pub fn arm_pre_mark_stored(attachment_id: Uuid) -> PreMarkStoredBarrier {
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (proceed_tx, proceed_rx) = tokio::sync::oneshot::channel();
        BARRIERS
            .lock()
            .expect("barrier mutex")
            .insert(attachment_id, (entered_tx, proceed_rx));
        PreMarkStoredBarrier {
            entered_rx,
            proceed_tx: Some(proceed_tx),
        }
    }

    pub fn disarm_pre_mark_stored(attachment_id: Uuid) {
        BARRIERS
            .lock()
            .expect("barrier mutex")
            .remove(&attachment_id);
    }

    impl PreMarkStoredBarrier {
        pub async fn wait_entered(&mut self) -> Result<(), tokio::sync::oneshot::error::RecvError> {
            (&mut self.entered_rx).await
        }

        pub fn proceed(&mut self) {
            if let Some(proceed_tx) = self.proceed_tx.take() {
                let _ = proceed_tx.send(());
            }
        }
    }

    pub async fn wait_pre_mark_stored_barrier(attachment_id: Uuid) {
        let entry = BARRIERS
            .lock()
            .expect("barrier mutex")
            .remove(&attachment_id);
        if let Some((entered_tx, proceed_rx)) = entry {
            let _ = entered_tx.send(());
            let _ = proceed_rx.await;
        }
    }

    static PUBLISH_BARRIERS: LazyLock<BarrierMap> = LazyLock::new(|| Mutex::new(HashMap::new()));

    pub fn arm_pre_publish(attachment_id: Uuid) -> PreMarkStoredBarrier {
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (proceed_tx, proceed_rx) = tokio::sync::oneshot::channel();
        PUBLISH_BARRIERS
            .lock()
            .expect("barrier mutex")
            .insert(attachment_id, (entered_tx, proceed_rx));
        PreMarkStoredBarrier {
            entered_rx,
            proceed_tx: Some(proceed_tx),
        }
    }

    pub fn disarm_pre_publish(attachment_id: Uuid) {
        PUBLISH_BARRIERS
            .lock()
            .expect("barrier mutex")
            .remove(&attachment_id);
    }

    pub async fn wait_pre_publish_barrier(attachment_id: Uuid) {
        let entry = PUBLISH_BARRIERS
            .lock()
            .expect("barrier mutex")
            .remove(&attachment_id);
        if let Some((entered_tx, proceed_rx)) = entry {
            let _ = entered_tx.send(());
            let _ = proceed_rx.await;
        }
    }
}
