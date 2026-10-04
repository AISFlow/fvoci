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
use crate::attachments::{ObjectStorage, PartInfo, StorageError, TransferMode};
use crate::db::context::{
    lock_key_from_uuid, lock_membership_users, recheck_session, restore_system, session_is_live,
    set_system, set_tenant,
};
use crate::db::documents::document_permission;
use crate::db::identity::{append_audit, append_event, AuditAppend, EventAppend};
use crate::db::projects::project_member_role;
use crate::db::quota::{StorageQuota, StorageQuotaError};
use crate::db::workspace::{membership_role, membership_role_for_update, workspace_is_live};
use crate::projects::effective_permission;
use crate::projects::ProjectPermission;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadMeta {
    pub part_size_bytes: i64,
    pub part_count: i32,
    pub declared_size_bytes: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upload_ref: Option<String>,
    /// The transfer mode fixed when the session was created. Rows written
    /// before #149 have no field and are proxy sessions. Nothing rewrites it,
    /// so the session never changes path (the jsonb column has no CHECK and
    /// is cleared when the row is stored, so this needs no migration).
    #[serde(default)]
    pub transfer: TransferMode,
}

impl UploadMeta {
    /// Exact byte length of part `n` (1-based): `part_size_bytes` for every
    /// part but the last, which carries the remainder.
    pub fn part_len(&self, n: i32) -> u64 {
        if n < self.part_count {
            self.part_size_bytes as u64
        } else {
            (self.declared_size_bytes - self.part_size_bytes * (self.part_count as i64 - 1)) as u64
        }
    }

    /// Whether storage holds exactly the parts a presigned session must have
    /// before `CompleteMultipartUpload`: numbers `1..=part_count`, each of its
    /// exact length, and `submitted` naming each one once with the ETag
    /// storage reports. The server never saw these bytes, so this is checked
    /// before anything is published rather than after.
    pub fn listed_parts_match(&self, submitted: &[(i32, String)], listed: &[PartInfo]) -> bool {
        if listed.len() != self.part_count as usize || submitted.len() != listed.len() {
            return false;
        }
        let mut submitted: Vec<(i32, &str)> = submitted
            .iter()
            .map(|(n, etag)| (*n, etag.trim().trim_matches('"')))
            .collect();
        submitted.sort_by_key(|(n, _)| *n);
        listed
            .iter()
            .zip(submitted)
            .zip(1..)
            .all(|((part, (n, etag)), expected)| {
                part.part_number == expected
                    && n == expected
                    && part.size_bytes == self.part_len(expected)
                    && part.etag == etag
            })
    }
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
    /// Further attachment ids held on the same connection (a transfer's
    /// destination id); released with the main lock.
    also: Vec<i32>,
}

impl AttachmentSessionLock {
    async fn try_acquire(pool: &PgPool, attachment_id: Uuid) -> Result<Option<Self>, sqlx::Error> {
        let mut conn = pool.acquire().await?;
        // Fail-safe: close_on_drop before try-lock so cancellation during
        // acquisition cannot return a lock-holding connection to the pool.
        // Moving this after a successful lock would leak a session advisory lock
        // if the task is cancelled between acquire and the flag. The cost: every
        // try_acquire, winner or loser, closes its connection on drop, and
        // complete's loser repeats that every ASSEMBLE_POLL (100 ms) for up to
        // ASSEMBLE_WAIT.
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
            also: Vec::new(),
        }))
    }

    /// Takes the session lock of another attachment id on this same
    /// connection (no second pool connection); `false` when it is busy.
    /// A key that collides with one this session already holds re-enters it
    /// (PostgreSQL counts session locks); `release` unlocks once per
    /// acquisition, and on error or cancellation the connection is closed on
    /// drop (`close_on_drop` in `try_acquire`), which drops every key.
    async fn try_also(&mut self, attachment_id: Uuid) -> Result<bool, sqlx::Error> {
        let key = lock_key_from_uuid(attachment_id);
        let conn = self.conn.as_mut().expect("lock connection");
        let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1, $2)")
            .bind(ATTACHMENT_LOCK_NAMESPACE)
            .bind(key)
            .fetch_one(&mut **conn)
            .await?;
        if locked {
            self.also.push(key);
        }
        Ok(locked)
    }

    async fn begin(&mut self) -> Result<Transaction<'_, Postgres>, sqlx::Error> {
        self.conn.as_mut().expect("lock connection").begin().await
    }

    async fn release(mut self) {
        if self.held {
            if let Some(mut conn) = self.conn.take() {
                for key in std::iter::once(self.lock_key).chain(self.also.iter().copied()) {
                    let _ = sqlx::query("SELECT pg_advisory_unlock($1, $2)")
                        .bind(ATTACHMENT_LOCK_NAMESPACE)
                        .bind(key)
                        .execute(&mut *conn)
                        .await;
                }
            }
        }
        self.held = false;
    }
}

fn parse_upload_meta(value: &Value) -> Result<UploadMeta, AttachmentDbError> {
    serde_json::from_value(value.clone()).map_err(|_| AttachmentDbError::InvalidInput)
}

pub(crate) async fn lock_workspace_storage(
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

/// Upload write access for one attachment, in the write-fence order: the
/// membership advisory lock, the session row `FOR UPDATE` (the revocation
/// fence), a live workspace, the row, then edit access and "the actor is the
/// uploader". The part, resume and losing-complete entry points call it
/// without the upload lock; writers take `with_upload_xact_lock` first and
/// call it again right before they publish, so a revocation in between is
/// seen. Callers check the status they need on the returned row.
async fn check_upload_write_access(
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
    transfer: TransferMode,
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
        transfer,
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
    let att = match check_upload_write_access(
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
            return Ok(Err(err));
        }
    };
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
    // A presigned session's parts go to storage directly; taking one here too
    // would open a second path for the same part.
    if meta.transfer != TransferMode::Proxy {
        tx.rollback().await?;
        return Ok(Err(AttachmentDbError::UploadState));
    }
    if part_number > meta.part_count {
        tx.rollback().await?;
        return Ok(Err(AttachmentDbError::InvalidInput));
    }
    let max_bytes = meta.part_len(part_number);
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
    let att = match check_upload_write_access(
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
            staged.discard().await;
            return Ok(Err(err));
        }
    };
    if att.status != "uploading" {
        tx.rollback().await?;
        staged.discard().await;
        return Ok(Err(AttachmentDbError::UploadState));
    }
    let meta = parse_upload_meta(att.upload_meta.as_ref().unwrap_or(&json!({})))
        .map_err(|e| sqlx::Error::Io(std::io::Error::other(e.to_string())))?;
    if part_number > meta.part_count {
        tx.rollback().await?;
        staged.discard().await;
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
            staged.discard().await;
            return Ok(Err(AttachmentDbError::UploadState));
        }
        Err(StorageError::PartTooLarge) => {
            tx.rollback().await?;
            staged.discard().await;
            return Ok(Err(AttachmentDbError::PartTooLarge));
        }
        Err(err) => {
            tx.rollback().await?;
            staged.discard().await;
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
    let att = match check_upload_write_access(
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
            return Ok(Err(err));
        }
    };
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
        // A presigned part of the wrong length (storage that did not enforce
        // the signed length) is sent again rather than reported as done.
        .filter(|p| {
            meta.transfer == TransferMode::Proxy || p.size_bytes == meta.part_len(p.part_number)
        })
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
        let att = match check_upload_write_access(
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
    let att = match check_upload_write_access(
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
        // A retried or repeated complete also clears parts that an earlier
        // run's finalize never removed (cancelled, crashed or failed).
        finalize_stored_parts(storage, attachment_id, &att.storage_key).await;
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

    if needs_assembly && meta.transfer == TransferMode::Presigned {
        match storage
            .list_parts(&storage_key, meta.upload_ref.as_deref())
            .await
        {
            Ok(listed) if meta.listed_parts_match(parts, &listed) => {}
            Ok(_) => {
                revert_assembling_on_conn(lock, workspace_id, attachment_id).await?;
                return Ok(CompleteAttempt::Denied(AttachmentDbError::EtagMismatch));
            }
            // Gone: an earlier attempt verified the parts and completed the
            // upload before it could mark the row stored. Complete below
            // reports the published object, or `UploadGone` if there is none.
            Err(StorageError::UploadGone) => {}
            Err(err) => {
                revert_assembling_on_conn(lock, workspace_id, attachment_id).await?;
                return Err(sqlx::Error::Io(std::io::Error::other(err.to_string())));
            }
        }
    }

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
    let att = match check_upload_write_access(
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
        finalize_stored_parts(storage, attachment_id, &storage_key).await;
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
            finalize_stored_parts(storage, attachment_id, &storage_key).await;
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
    finalize_stored_parts(storage, attachment_id, &storage_key).await;
    Ok(CompleteAttempt::Done(row))
}

/// Removes the local part copies of an upload whose 'stored' row has
/// committed (a no-op on S3). Never before that commit: an 'assembling'
/// retry re-lists and re-checks the parts. The upload is complete either
/// way, so a failure is logged rather than returned; the leftover copy is
/// removed by the next complete of this upload or when it is deleted.
async fn finalize_stored_parts(storage: &ObjectStorage, attachment_id: Uuid, storage_key: &str) {
    if let Err(err) = storage.finalize_multipart(storage_key).await {
        tracing::warn!(%attachment_id, error = %err, "attachment.finalize_parts_failed");
    }
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
    // Serialise with `publish_preview`, which holds this row while it
    // publishes a journaled preview key: whoever locks first decides.
    let still_journaled: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM fvoci.attachment_object_cleanups WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    if still_journaled.is_none() {
        tx.commit().await?;
        return Ok(true);
    }
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

// Selected-backend cleanup borrows the actual family writer, never a session
// mutex. PostgreSQL counterparts retain their detached upload-session owner.
#[derive(Debug, Clone)]
pub(crate) struct AttachmentObjectCleanup {
    pub(crate) id: Uuid,
    pub(crate) workspace_id: Uuid,
    pub(crate) attachment_id: Uuid,
    pub(crate) storage_key: String,
}

impl crate::db::backend::OperationTx<'_, '_> {
    fn attachment_cleanup_global_family(
        &mut self,
    ) -> Result<&mut crate::db::backend::FamilyTx, sqlx::Error> {
        let Self::SqliteFamily(tx) = self else {
            return Err(sqlx::Error::Protocol(
                "PostgreSQL cleanup uses its detached upload-session transaction".into(),
            ));
        };
        tx.require_system_context()?;
        if tx.tenant().is_some() {
            return Err(sqlx::Error::Protocol(
                "global attachment cleanup cannot broaden tenant context".into(),
            ));
        }
        Ok(tx)
    }

    fn attachment_cleanup_tenant_family(
        &mut self,
        workspace: Uuid,
    ) -> Result<&mut crate::db::backend::FamilyTx, sqlx::Error> {
        let Self::SqliteFamily(tx) = self else {
            return Err(sqlx::Error::Protocol(
                "PostgreSQL cleanup uses its detached upload-session transaction".into(),
            ));
        };
        tx.require_writer()?;
        tx.require_tenant(workspace)?;
        if tx.require_system_context().is_ok() {
            return Err(sqlx::Error::Protocol(
                "scoped attachment cleanup refuses system broadening".into(),
            ));
        }
        Ok(tx)
    }

    pub(crate) async fn list_due_attachment_objects(
        &mut self,
        only: Option<(Uuid, Uuid)>,
        limit: i64,
    ) -> Result<Vec<AttachmentObjectCleanup>, sqlx::Error> {
        if limit < 0 {
            return Err(sqlx::Error::Protocol(
                "negative attachment cleanup limit".into(),
            ));
        }
        let tx = self.attachment_cleanup_global_family()?;
        let rows = tx.query("SELECT id,workspace_id,attachment_id,storage_key FROM attachment_object_cleanups WHERE due_at <= unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000 AND (?1 IS NULL OR (workspace_id=?1 AND attachment_id=?2)) ORDER BY due_at,id LIMIT ?3", &[crate::db::codec::Cell::optional_uuid(only.map(|v|v.0)),crate::db::codec::Cell::optional_uuid(only.map(|v|v.1)),crate::db::codec::Cell::Integer(limit)]).await?;
        rows.iter()
            .map(|r| {
                Ok(AttachmentObjectCleanup {
                    id: r.cell(0)?.id()?,
                    workspace_id: r.cell(1)?.id()?,
                    attachment_id: r.cell(2)?.id()?,
                    storage_key: r.cell(3)?.string()?,
                })
            })
            .collect()
    }

    /// Check the actual journal identity under the same writer as publication.
    /// A stale snapshot cannot drop a replacement row or purge its new key.
    pub(crate) async fn attachment_cleanup_journal_current(
        &mut self,
        row: &AttachmentObjectCleanup,
    ) -> Result<bool, sqlx::Error> {
        let tx = self.attachment_cleanup_tenant_family(row.workspace_id)?;
        let rows = tx.query("SELECT id FROM attachment_object_cleanups WHERE id=?1 AND workspace_id=?2 AND attachment_id=?3 AND storage_key=?4", &cleanup_cells(row)).await?;
        Ok(!rows.is_empty())
    }

    pub(crate) async fn attachment_cleanup_key_referenced(
        &mut self,
        workspace: Uuid,
        key: &str,
    ) -> Result<bool, sqlx::Error> {
        let tx = self.attachment_cleanup_tenant_family(workspace)?;
        let rows=tx.query("SELECT EXISTS(SELECT 1 FROM attachments WHERE workspace_id=?1 AND (storage_key=?2 OR json_extract(variants,'$.preview.key')=?2))", &[crate::db::codec::Cell::uuid(workspace),crate::db::codec::Cell::text(key)]).await?;
        rows[0].cell(0)?.boolean()
    }

    pub(crate) async fn remove_attachment_cleanup_journal(
        &mut self,
        row: &AttachmentObjectCleanup,
    ) -> Result<bool, sqlx::Error> {
        let tx = self.attachment_cleanup_tenant_family(row.workspace_id)?;
        Ok(tx.execute("DELETE FROM attachment_object_cleanups WHERE id=?1 AND workspace_id=?2 AND attachment_id=?3 AND storage_key=?4", &cleanup_cells(row)).await?==1)
    }

    pub(crate) async fn reschedule_attachment_object(
        &mut self,
        row: &AttachmentObjectCleanup,
        failed: bool,
    ) -> Result<(), sqlx::Error> {
        let tx = self.attachment_cleanup_tenant_family(row.workspace_id)?;
        tx.execute("UPDATE attachment_object_cleanups SET due_at=unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000+?5,attempts=attempts+?6 WHERE id=?1 AND workspace_id=?2 AND attachment_id=?3 AND storage_key=?4", &[crate::db::codec::Cell::uuid(row.id),crate::db::codec::Cell::uuid(row.workspace_id),crate::db::codec::Cell::uuid(row.attachment_id),crate::db::codec::Cell::text(&row.storage_key),crate::db::codec::Cell::Integer(OBJECT_CLEANUP_RETRY.as_secs() as i64*1_000_000),crate::db::codec::Cell::Integer(i64::from(failed))]).await?;
        Ok(())
    }

    pub(crate) async fn list_stale_attachment_uploads(
        &mut self,
        cutoff: DateTime<Utc>,
        after: Option<StaleUploadCursor>,
        limit: i64,
    ) -> Result<Vec<StaleUpload>, sqlx::Error> {
        if limit < 0 {
            return Err(sqlx::Error::Protocol("negative stale upload limit".into()));
        }
        let tx = self.attachment_cleanup_global_family()?;
        let rows=tx.query("SELECT id,workspace_id,created_at FROM attachments WHERE status IN ('uploading','assembling') AND created_at<?1 AND (?2 IS NULL OR (created_at,id)>(?2,?3)) ORDER BY created_at,id LIMIT ?4", &[crate::db::codec::Cell::instant(cutoff)?,after.map(|v|crate::db::codec::Cell::instant(v.0)).transpose()?.unwrap_or(crate::db::codec::Cell::Null),crate::db::codec::Cell::optional_uuid(after.map(|v|v.1)),crate::db::codec::Cell::Integer(limit)]).await?;
        rows.iter()
            .map(|r| {
                Ok(StaleUpload {
                    id: r.cell(0)?.id()?,
                    workspace_id: r.cell(1)?.id()?,
                    created_at: r.cell(2)?.datetime()?,
                })
            })
            .collect()
    }
}

fn cleanup_cells(row: &AttachmentObjectCleanup) -> [crate::db::codec::Cell; 4] {
    [
        crate::db::codec::Cell::uuid(row.id),
        crate::db::codec::Cell::uuid(row.workspace_id),
        crate::db::codec::Cell::uuid(row.attachment_id),
        crate::db::codec::Cell::text(&row.storage_key),
    ]
}

/// Selected-backend journal drain. PG retains its detached upload-session lock;
/// family holds the actual writer through reference check, storage and commit.
pub async fn reclaim_attachment_objects_backend(
    backend: &crate::db::backend::Backend,
    storage: &ObjectStorage,
    only: Option<(Uuid, Uuid)>,
    limit: i64,
) -> Result<ObjectCleanupStats, sqlx::Error> {
    reclaim_attachment_objects_backend_with_cancel(
        backend,
        storage,
        only,
        limit,
        &tokio_util::sync::CancellationToken::new(),
    )
    .await
}

pub(crate) async fn reclaim_attachment_objects_backend_with_cancel(
    backend: &crate::db::backend::Backend,
    storage: &ObjectStorage,
    only: Option<(Uuid, Uuid)>,
    limit: i64,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<ObjectCleanupStats, sqlx::Error> {
    if cancel.is_cancelled() {
        return Ok(ObjectCleanupStats::default());
    }
    if let crate::db::backend::Backend::Postgres(pool) = backend {
        return reclaim_attachment_objects(pool, storage, only, limit).await;
    }
    let mut tx = backend.begin_read().await?;
    let mut op = tx.operation();
    let previous = op.set_system().await?;
    let rows = op.list_due_attachment_objects(only, limit).await;
    op.restore_system(previous).await?;
    tx.rollback().await?;
    let rows = rows?;
    let mut stats = ObjectCleanupStats::default();
    for row in rows {
        if cancel.is_cancelled() {
            break;
        }
        stats.claimed += 1;
        match reclaim_family_object(backend, storage, &row, cancel).await {
            Ok(CleanupDisposition::Reclaimed) => stats.reclaimed += 1,
            Ok(CleanupDisposition::Cancelled) => break,
            Ok(CleanupDisposition::Busy) => {
                stats.busy += 1;
                reschedule_family_object(backend, &row, false).await?;
            }
            Ok(CleanupDisposition::Retry) => {
                stats.failed += 1;
                reschedule_family_object(backend, &row, true).await?;
            }
            Err(CleanupFailure::Known(err)) => {
                stats.failed += 1;
                tracing::warn!(attachment_id=%row.attachment_id,error=%err,"attachment.object_cleanup_failed");
                reschedule_family_object(backend, &row, true).await?;
            }
            // A lost COMMIT reply does not authorize retrying a destructive
            // step or incrementing attempts. Reconciliation was awaited.
            Err(CleanupFailure::Unknown(err)) => return Err(err),
        }
    }
    Ok(stats)
}

#[derive(Debug, PartialEq, Eq)]
enum CleanupDisposition {
    Reclaimed,
    Busy,
    Retry,
    Cancelled,
}
enum CleanupFailure {
    Known(sqlx::Error),
    Unknown(sqlx::Error),
}

async fn reclaim_family_object(
    backend: &crate::db::backend::Backend,
    storage: &ObjectStorage,
    row: &AttachmentObjectCleanup,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<CleanupDisposition, CleanupFailure> {
    // BEGIN is always awaited; do not drop a partly acquired writer on token
    // cancellation. Token cancellation is cooperative, not task-abort safety.
    let mut tx = backend.begin_write().await.map_err(CleanupFailure::Known)?;
    let result = async {
        let mut op = tx.operation();
        op.set_tenant(row.workspace_id).await?;
        reclaim_attachment_object_borrowed(&mut op, storage, row, cancel).await
    }
    .await;
    match result {
        Ok(CleanupDisposition::Reclaimed) if cancel.is_cancelled() => {
            tx.rollback().await.map_err(CleanupFailure::Known)?;
            Ok(CleanupDisposition::Cancelled)
        }
        Ok(CleanupDisposition::Reclaimed) => {
            match tx.commit().await {
                Ok(()) => Ok(CleanupDisposition::Reclaimed),
                Err(unknown) => {
                    #[cfg(test)]
                    cleanup_test_hooks::wait(row.id, 4).await;
                    // Observe a stable current journal/reference state under
                    // the writer again, never repeat purge on an unknown reply.
                    reconcile_attachment_cleanup(backend, storage, row)
                        .await
                        .map_err(CleanupFailure::Unknown)?;
                    Err(CleanupFailure::Unknown(sqlx::Error::AnyDriverError(
                        Box::new(unknown),
                    )))
                }
            }
        }
        Ok(outcome) => {
            tx.rollback().await.map_err(CleanupFailure::Known)?;
            Ok(outcome)
        }
        Err(err) => {
            tx.rollback().await.map_err(CleanupFailure::Known)?;
            Err(CleanupFailure::Known(err))
        }
    }
}

/// Deliberately retains the caller's actual writer during bounded storage I/O.
/// Releasing after the reference check permits a publisher to make this key
/// live before purge. Await the maintained storage calls even if cancelled;
/// their existing read/operation limits remain unchanged. No Drop guarantee,
/// detached filesystem purge or outer timeout destroys the writer owner.
async fn reclaim_attachment_object_borrowed(
    op: &mut crate::db::backend::OperationTx<'_, '_>,
    storage: &ObjectStorage,
    row: &AttachmentObjectCleanup,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<CleanupDisposition, sqlx::Error> {
    if cancel.is_cancelled() {
        return Ok(CleanupDisposition::Cancelled);
    }
    if !op.attachment_cleanup_journal_current(row).await? {
        return Ok(CleanupDisposition::Reclaimed);
    }
    if op
        .attachment_cleanup_key_referenced(row.workspace_id, &row.storage_key)
        .await?
    {
        op.remove_attachment_cleanup_journal(row).await?;
        return Ok(CleanupDisposition::Reclaimed);
    }
    if op
        .attachment_cleanup_assembly_busy(row.workspace_id, row.attachment_id)
        .await?
    {
        return Ok(CleanupDisposition::Busy);
    }
    #[cfg(test)]
    cleanup_test_hooks::wait(row.id, 0).await;
    if cancel.is_cancelled() {
        return Ok(CleanupDisposition::Cancelled);
    }
    if let Err(err) = storage.purge_key(&row.storage_key).await {
        tracing::warn!(error=%err,"attachment.object_cleanup_storage_failed");
        #[cfg(test)]
        cleanup_test_hooks::wait(row.id, 2).await;
        return Ok(CleanupDisposition::Retry);
    }
    #[cfg(test)]
    cleanup_test_hooks::wait(row.id, 1).await;
    let head = storage.head(&row.storage_key).await;
    if cancel.is_cancelled() {
        return Ok(CleanupDisposition::Cancelled);
    }
    if !matches!(head, Ok(None)) {
        return Ok(CleanupDisposition::Retry);
    }
    op.remove_attachment_cleanup_journal(row).await?;
    #[cfg(test)]
    cleanup_test_hooks::defer_fk_fault(op, row).await?;
    Ok(CleanupDisposition::Reclaimed)
}

async fn reconcile_attachment_cleanup(
    backend: &crate::db::backend::Backend,
    storage: &ObjectStorage,
    row: &AttachmentObjectCleanup,
) -> Result<(), sqlx::Error> {
    let mut tx = backend.begin_write().await?;
    let result = async {
        let mut op = tx.operation();
        op.set_tenant(row.workspace_id).await?;
        let journal = op.attachment_cleanup_journal_current(row).await?;
        let referenced = op
            .attachment_cleanup_key_referenced(row.workspace_id, &row.storage_key)
            .await?;
        let head = storage.head(&row.storage_key).await;
        tracing::warn!(
            journal,
            referenced,
            head_missing = matches!(head, Ok(None)),
            "attachment.cleanup_commit_unknown_reconciled"
        );
        Ok::<_, sqlx::Error>(())
    }
    .await;
    tx.rollback().await?;
    result
}

async fn reschedule_family_object(
    backend: &crate::db::backend::Backend,
    row: &AttachmentObjectCleanup,
    failed: bool,
) -> Result<(), sqlx::Error> {
    let mut tx = backend.begin_write().await?;
    let result = async {
        let mut op = tx.operation();
        op.set_tenant(row.workspace_id).await?;
        op.reschedule_attachment_object(row, failed).await
    }
    .await;
    match result {
        Ok(()) => tx
            .commit()
            .await
            .map_err(|e| sqlx::Error::AnyDriverError(Box::new(e))),
        Err(err) => {
            tx.rollback().await?;
            Err(err)
        }
    }
}

pub async fn list_stale_uploading_backend(
    backend: &crate::db::backend::Backend,
    cutoff: DateTime<Utc>,
    after: Option<StaleUploadCursor>,
    limit: i64,
) -> Result<Vec<StaleUpload>, sqlx::Error> {
    if let crate::db::backend::Backend::Postgres(pool) = backend {
        return list_stale_uploading(pool, cutoff, after, limit).await;
    }
    let mut tx = backend.begin_read().await?;
    let mut op = tx.operation();
    let previous = op.set_system().await?;
    let result = op.list_stale_attachment_uploads(cutoff, after, limit).await;
    op.restore_system(previous).await?;
    tx.rollback().await?;
    result
}

pub async fn gc_stale_upload_row_backend(
    backend: &crate::db::backend::Backend,
    storage: &ObjectStorage,
    workspace: Uuid,
    attachment: Uuid,
) -> Result<bool, sqlx::Error> {
    gc_stale_upload_row_backend_with_cancel(
        backend,
        storage,
        workspace,
        attachment,
        &tokio_util::sync::CancellationToken::new(),
    )
    .await
}

pub(crate) async fn gc_stale_upload_row_backend_with_cancel(
    backend: &crate::db::backend::Backend,
    storage: &ObjectStorage,
    workspace: Uuid,
    attachment: Uuid,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<bool, sqlx::Error> {
    if cancel.is_cancelled() {
        return Ok(false);
    }
    if let crate::db::backend::Backend::Postgres(pool) = backend {
        return gc_stale_upload_row(pool, storage, workspace, attachment).await;
    }
    let mut tx = backend.begin_write().await?;
    let result = async {
        let mut op = tx.operation();
        op.set_tenant(workspace).await?;
        let Some(key) = op
            .prepare_stale_attachment_cleanup(workspace, attachment)
            .await?
        else {
            return Ok(false);
        };
        #[cfg(test)]
        cleanup_test_hooks::wait(attachment, 0).await;
        if cancel.is_cancelled() {
            return Ok(false);
        }
        // The row retains the exact key until successful purge+head+DELETE.
        // Cancellation waits for owned purge and head, leaving the row pointer.
        storage
            .purge_key(&key)
            .await
            .map_err(|e| sqlx::Error::Io(std::io::Error::other(e.to_string())))?;
        #[cfg(test)]
        cleanup_test_hooks::wait(attachment, 1).await;
        let head = storage.head(&key).await;
        if cancel.is_cancelled() {
            return Ok(false);
        }
        if !matches!(head, Ok(None)) {
            return Err(sqlx::Error::Io(std::io::Error::other(
                "stale attachment purge did not confirm absence",
            )));
        }
        op.remove_stale_attachment_cleanup(workspace, attachment, &key)
            .await
    }
    .await;
    match result {
        Ok(true) if cancel.is_cancelled() => {
            tx.rollback().await?;
            Ok(false)
        }
        Ok(true) => match tx.commit().await {
            Ok(()) => Ok(true),
            Err(unknown) => {
                // Await a fresh writer/current-row observation, no destructive
                // retry after an uncertain DELETE/trigger-journal commit.
                let mut observe = backend.begin_write().await?;
                let observed = async {
                    let mut op = observe.operation();
                    op.set_tenant(workspace).await?;
                    op.prepare_stale_attachment_cleanup(workspace, attachment)
                        .await
                }
                .await;
                observe.rollback().await?;
                observed?;
                Err(sqlx::Error::AnyDriverError(Box::new(unknown)))
            }
        },
        Ok(false) => {
            tx.rollback().await?;
            Ok(false)
        }
        Err(err) => {
            tx.rollback().await?;
            Err(err)
        }
    }
}

impl crate::db::backend::OperationTx<'_, '_> {
    /// Missing S18 selected complete/session owner: never treat an assembling
    /// row as abandoned merely because its timestamp is old. Pagination still
    /// advances and healthy uploading/journal work proceeds. No invented TTL.
    async fn attachment_cleanup_assembly_busy(
        &mut self,
        workspace: Uuid,
        attachment: Uuid,
    ) -> Result<bool, sqlx::Error> {
        let tx = self.attachment_cleanup_tenant_family(workspace)?;
        let rows=tx.query("SELECT EXISTS(SELECT 1 FROM attachments WHERE workspace_id=?1 AND id=?2 AND status='assembling')", &[crate::db::codec::Cell::uuid(workspace),crate::db::codec::Cell::uuid(attachment)]).await?;
        rows[0].cell(0)?.boolean()
    }
    pub(crate) async fn prepare_stale_attachment_cleanup(
        &mut self,
        workspace: Uuid,
        attachment: Uuid,
    ) -> Result<Option<String>, sqlx::Error> {
        let tx = self.attachment_cleanup_tenant_family(workspace)?;
        let rows=tx.query("SELECT storage_key FROM attachments a WHERE a.workspace_id=?1 AND a.id=?2 AND a.status='uploading' AND NOT EXISTS(SELECT 1 FROM attachments other WHERE other.workspace_id=?1 AND ((other.id<>?2 AND other.storage_key=a.storage_key) OR json_extract(other.variants,'$.preview.key')=a.storage_key))", &[crate::db::codec::Cell::uuid(workspace),crate::db::codec::Cell::uuid(attachment)]).await?;
        rows.first().map(|r| r.cell(0)?.string()).transpose()
    }
    pub(crate) async fn remove_stale_attachment_cleanup(
        &mut self,
        workspace: Uuid,
        attachment: Uuid,
        key: &str,
    ) -> Result<bool, sqlx::Error> {
        let tx = self.attachment_cleanup_tenant_family(workspace)?;
        Ok(tx.execute("DELETE FROM attachments WHERE workspace_id=?1 AND id=?2 AND storage_key=?3 AND status='uploading'", &[crate::db::codec::Cell::uuid(workspace),crate::db::codec::Cell::uuid(attachment),crate::db::codec::Cell::text(key)]).await?==1)
    }
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
    /// Published preview object (`variants.preview`), if any.
    pub preview: Option<PreviewVariant>,
}

/// Every stored attachment's key and size (and its published preview) in one
/// workspace, read under that workspace's tenant context (the app role has no
/// cross-tenant bypass).
pub async fn list_workspace_stored_objects(
    pool: &PgPool,
    workspace_id: Uuid,
) -> Result<Vec<StoredObject>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let rows: Vec<(Uuid, String, i64, Value)> = sqlx::query_as(
        r#"
        SELECT id, storage_key, size_bytes, variants
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
        .map(|(id, storage_key, size_bytes, variants)| StoredObject {
            id,
            workspace_id,
            storage_key,
            size_bytes,
            preview: preview_variant_of(&variants),
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

    /// Transfer staging, keyed by the destination attachment id: after the
    /// fresh keys are journaled, before any byte is written, with the
    /// staging locks held.
    static STAGE_BARRIERS: LazyLock<BarrierMap> = LazyLock::new(|| Mutex::new(HashMap::new()));

    pub fn arm_transfer_stage_copy(destination_attachment_id: Uuid) -> PreMarkStoredBarrier {
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (proceed_tx, proceed_rx) = tokio::sync::oneshot::channel();
        STAGE_BARRIERS
            .lock()
            .expect("barrier mutex")
            .insert(destination_attachment_id, (entered_tx, proceed_rx));
        PreMarkStoredBarrier {
            entered_rx,
            proceed_tx: Some(proceed_tx),
        }
    }

    pub async fn wait_transfer_stage_copy_barrier(destination_attachment_id: Uuid) {
        let entry = STAGE_BARRIERS
            .lock()
            .expect("barrier mutex")
            .remove(&destination_attachment_id);
        if let Some((entered_tx, proceed_rx)) = entry {
            let _ = entered_tx.send(());
            let _ = proceed_rx.await;
        }
    }
}

/// Import asset reservation (source `storeImportedAsset`, first `importTx`):
/// the creator is still a workspace admin, the parent document is live, the
/// workspace storage quota admits the bytes under the storage lock, and the
/// `uploading` row plus its storage key in the job's `created_refs` commit
/// together, so a run that dies during the object write leaves the key to
/// the restart recovery or the orphan sweep. `Ok(Ok(None))` = fence lost.
#[allow(clippy::too_many_arguments)]
pub async fn create_import_attachment(
    pool: &PgPool,
    quota: &StorageQuota,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
    name: &str,
    size_bytes: i64,
    fence: crate::db::documents::ImportFence,
) -> Result<Result<Option<(Uuid, String)>, AttachmentDbError>, sqlx::Error> {
    let attachment_id = Uuid::now_v7();
    let storage_key = Uuid::now_v7().to_string();
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
    let role = membership_role_for_update(&mut tx, workspace_id, actor_user_id).await?;
    if !role.is_some_and(|r| r.at_least(crate::db::workspace::WorkspaceRole::Admin)) {
        tx.rollback().await?;
        return Ok(Err(AttachmentDbError::Forbidden));
    }
    let parent: Option<(Option<DateTime<Utc>>,)> = sqlx::query_as(
        "SELECT deleted_at FROM fvoci.documents WHERE workspace_id = $1 AND id = $2 FOR SHARE",
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_optional(&mut *tx)
    .await?;
    if !matches!(parent, Some((None,))) {
        tx.rollback().await?;
        return Ok(Err(AttachmentDbError::NotFound));
    }
    lock_workspace_storage(&mut tx, workspace_id).await?;
    let reserved = count_reserved_bytes(&mut tx, workspace_id).await?;
    if let Err(err) = quota.check(reserved, size_bytes) {
        tx.rollback().await?;
        return Ok(Err(match err {
            StorageQuotaError::Upload => AttachmentDbError::UploadLimit,
            StorageQuotaError::Storage => AttachmentDbError::StorageLimit,
        }));
    }
    sqlx::query(
        r#"
        INSERT INTO fvoci.attachments (
            id, workspace_id, document_id, task_id, uploader_id, status, name, declared_mime,
            reserved_size_bytes, storage_key, upload_meta
        ) VALUES ($1, $2, $3, NULL, $4, 'uploading', $5, NULL, $6, $7, '{}'::jsonb)
        "#,
    )
    .bind(attachment_id)
    .bind(workspace_id)
    .bind(document_id)
    .bind(actor_user_id)
    .bind(name)
    .bind(size_bytes)
    .bind(&storage_key)
    .execute(&mut *tx)
    .await?;
    if !crate::db::import_jobs::append_import_ref(
        &mut tx,
        workspace_id,
        fence.job_id,
        fence.lease_token,
        crate::db::import_jobs::ImportRefKind::StoredKey,
        &storage_key,
    )
    .await?
    {
        tx.rollback().await?;
        return Ok(Ok(None));
    }
    tx.commit().await?;
    Ok(Ok(Some((attachment_id, storage_key))))
}

/// Import asset finalize (source `markStored` in the second `importTx`): the
/// bytes are in storage; the row becomes `stored` with the sniffed MIME and
/// is queued for extraction / preview like an uploaded file. `false` = the
/// fence was lost or the row is no longer `uploading`.
#[allow(clippy::too_many_arguments)]
pub async fn mark_import_attachment_stored(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    attachment_id: Uuid,
    name: &str,
    mime: &str,
    size_bytes: i64,
    fence: crate::db::documents::ImportFence,
) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !crate::db::import_jobs::hold_import_fence(&mut tx, workspace_id, fence).await? {
        tx.rollback().await?;
        return Ok(false);
    }
    let image = is_image_mime(mime);
    let preview_status = if image && crate::attachments::preview::preview_mime_supported(mime) {
        "pending"
    } else {
        "skipped"
    };
    let row: Option<(Option<Uuid>,)> = sqlx::query_as(
        r#"
        UPDATE fvoci.attachments
        SET status = 'stored', mime = $3, size_bytes = $4, image = $5,
            scan_status = 'skipped', extract_status = $6, preview_status = $7,
            upload_meta = NULL, completed_at = now()
        WHERE workspace_id = $1 AND id = $2 AND status = 'uploading'
        RETURNING document_id
        "#,
    )
    .bind(workspace_id)
    .bind(attachment_id)
    .bind(mime)
    .bind(size_bytes)
    .bind(image)
    .bind(initial_extract_status(name, mime))
    .bind(preview_status)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((document_id,)) = row else {
        tx.rollback().await?;
        return Ok(false);
    };
    record_attachment_event(
        &mut tx,
        workspace_id,
        actor_user_id,
        "attachment.completed",
        attachment_id,
        json!({
            "name": name,
            "documentId": document_id.map(|id| id.to_string()),
            "sizeBytes": size_bytes,
            "mime": mime,
        }),
        None,
    )
    .await?;
    tx.commit().await?;
    Ok(true)
}

/// Grace before a staged transfer key becomes reclaimable: the transfer that
/// publishes it must take its journal row before then (as
/// [`crate::db::attachment_preview::journal_preview_key`] does for previews).
pub const TRANSFER_STAGE_GRACE_SECS: i32 = 600;

/// One object copied to a fresh key and journaled in the destination
/// workspace; the publishing transaction locks and deletes `journal_id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedObject {
    pub journal_id: Uuid,
    pub key: String,
    pub size_bytes: i64,
    /// SHA-256 observed while the source object was streamed, equal to the
    /// fresh object's read-back. The attachment row stores no full-object
    /// hash, so this proves copy fidelity, not a stored checksum.
    pub sha256: [u8; 32],
}

/// A stored attachment staged for a transfer: the source row as checked and
/// its original (and published preview) copied to fresh journaled keys.
#[derive(Debug, Clone)]
pub struct StagedAttachment {
    pub source: AttachmentRow,
    pub original: StagedObject,
    /// The staged preview and its `variants.preview` value with the fresh key.
    pub preview: Option<(StagedObject, Value)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageAttachmentError {
    NotFound,
    Forbidden,
    /// Not a clean stored object right now: infected, or an extract/preview
    /// lease is live.
    NotReady,
    /// The object is missing, or its size or content changed while copied.
    Changed,
    Storage,
}

/// Copies one stored attachment of the source workspace to fresh keys for a
/// transfer, in the preview publication order: the source row is checked as
/// a download is (live session and workspace, view access on the parent,
/// stored, not infected) and must hold no live extract/preview lease; each
/// fresh key is journaled in `attachment_object_cleanups` of the destination
/// workspace under `destination_attachment_id` (the source id for a MOVE, the
/// new id for a COPY) before any byte is written, so a failure or crash leaves only
/// unreferenced journaled keys for the reclaim job; the bytes are streamed
/// with their SHA-256 and length counted, the written size is checked
/// against the stored size, and the copy is read back and must hash the
/// same. Quota and publication belong to the caller's transaction, which
/// must lock each journal row (absent: reclaim took it) and delete it.
///
/// The attachment's session lock (the one reclaim, delete and complete
/// take) - and, when they differ, the destination id's lock under which the
/// fresh keys are journaled - is held on one connection from the row checks
/// through the copy and read-back, so the reclaim job cannot take a staged
/// key while it is written; both are released before returning and never
/// reacquired in the caller's transaction. A busy lock is `NotReady`.
#[allow(clippy::too_many_arguments)]
pub async fn stage_attachment_for_transfer(
    pool: &PgPool,
    storage: &ObjectStorage,
    source_workspace_id: Uuid,
    destination_workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    attachment_id: Uuid,
    destination_attachment_id: Uuid,
) -> Result<Result<StagedAttachment, StageAttachmentError>, sqlx::Error> {
    let Some(mut lock) = AttachmentSessionLock::try_acquire(pool, attachment_id).await? else {
        return Ok(Err(StageAttachmentError::NotReady));
    };
    // The fresh keys are journaled under the destination id, which is what
    // the reclaim job locks: hold that id too (a COPY's new id) for the
    // whole write, on the same connection.
    if destination_attachment_id != attachment_id
        && !lock.try_also(destination_attachment_id).await?
    {
        lock.release().await;
        return Ok(Err(StageAttachmentError::NotReady));
    }
    let staged = stage_locked(
        &mut lock,
        storage,
        source_workspace_id,
        destination_workspace_id,
        actor_user_id,
        session_id,
        attachment_id,
        destination_attachment_id,
    )
    .await;
    lock.release().await;
    staged
}

/// Runs every short transaction on the lock's own connection, so staging
/// never holds one pool connection while waiting for another.
#[allow(clippy::too_many_arguments)]
async fn stage_locked(
    lock: &mut AttachmentSessionLock,
    storage: &ObjectStorage,
    source_workspace_id: Uuid,
    destination_workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    attachment_id: Uuid,
    destination_attachment_id: Uuid,
) -> Result<Result<StagedAttachment, StageAttachmentError>, sqlx::Error> {
    // The download checks (open_download's own helpers, same order).
    let mut tx = lock.begin().await?;
    set_tenant(&mut tx, source_workspace_id).await?;
    if !session_is_live(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(StageAttachmentError::Forbidden));
    }
    if !workspace_is_live(&mut tx, source_workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(StageAttachmentError::NotFound));
    }
    let Some(att) = fetch_attachment(&mut tx, source_workspace_id, attachment_id).await? else {
        tx.rollback().await?;
        return Ok(Err(StageAttachmentError::NotFound));
    };
    match require_view_access(&mut tx, source_workspace_id, actor_user_id, &att).await? {
        Ok(_) => {}
        Err(AttachmentDbError::NotFound) => {
            tx.rollback().await?;
            return Ok(Err(StageAttachmentError::NotFound));
        }
        Err(_) => {
            tx.rollback().await?;
            return Ok(Err(StageAttachmentError::Forbidden));
        }
    }
    if att.status != "stored" {
        tx.rollback().await?;
        return Ok(Err(StageAttachmentError::NotFound));
    }
    if att.scan_status == "infected" {
        tx.rollback().await?;
        return Ok(Err(StageAttachmentError::NotReady));
    }
    let leased: Option<bool> = sqlx::query_scalar(
        r#"
        SELECT coalesce(extract_lease_expires_at > clock_timestamp(), false)
            OR coalesce(preview_lease_expires_at > clock_timestamp(), false)
        FROM fvoci.attachments WHERE workspace_id = $1 AND id = $2
        "#,
    )
    .bind(source_workspace_id)
    .bind(attachment_id)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    match leased {
        None => return Ok(Err(StageAttachmentError::NotFound)),
        Some(true) => return Ok(Err(StageAttachmentError::NotReady)),
        Some(false) => {}
    }
    let Some(size) = att.size_bytes.filter(|size| *size >= 0) else {
        return Ok(Err(StageAttachmentError::Changed));
    };
    let preview = preview_variant_of(&att.variants);
    let original_key = Uuid::now_v7().to_string();
    let preview_key = preview.as_ref().map(|_| Uuid::now_v7().to_string());
    let mut journal = vec![(Uuid::now_v7(), original_key.clone())];
    if let Some(key) = &preview_key {
        journal.push((Uuid::now_v7(), key.clone()));
    }
    let mut tx = lock.begin().await?;
    set_tenant(&mut tx, destination_workspace_id).await?;
    for (id, key) in &journal {
        sqlx::query(
            r#"
            INSERT INTO fvoci.attachment_object_cleanups (id, workspace_id, attachment_id, storage_key, due_at)
            VALUES ($1, $2, $3, $4, clock_timestamp() + ($5 * interval '1 second'))
            "#,
        )
        .bind(id)
        .bind(destination_workspace_id)
        .bind(destination_attachment_id)
        .bind(key)
        .bind(TRANSFER_STAGE_GRACE_SECS)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    #[cfg(feature = "db-tests")]
    test_barrier::wait_transfer_stage_copy_barrier(destination_attachment_id).await;
    let original = match copy_verified(storage, &att.storage_key, &original_key, size).await {
        Ok(sha256) => StagedObject {
            journal_id: journal[0].0,
            key: original_key,
            size_bytes: size,
            sha256,
        },
        Err(err) => return Ok(Err(err)),
    };
    let preview = match (preview, preview_key) {
        (Some(variant), Some(key)) => {
            let sha256 = match copy_verified(storage, &variant.key, &key, variant.bytes).await {
                Ok(sha256) => sha256,
                Err(err) => return Ok(Err(err)),
            };
            let mut value = att.variants.get("preview").cloned().unwrap_or(Value::Null);
            if let Some(object) = value.as_object_mut() {
                object.insert("key".into(), Value::String(key.clone()));
            }
            Some((
                StagedObject {
                    journal_id: journal[1].0,
                    key,
                    size_bytes: variant.bytes,
                    sha256,
                },
                value,
            ))
        }
        _ => None,
    };
    Ok(Ok(StagedAttachment {
        source: att,
        original,
        preview,
    }))
}

/// Destination storage admission for a transfer, in the caller's transaction
/// with the destination tenant set: takes the workspace storage lock the
/// upload paths take (held to commit) and admits each moved size in turn
/// against the same quota as an upload.
pub async fn admit_transfer_storage(
    tx: &mut Transaction<'_, Postgres>,
    quota: &StorageQuota,
    workspace_id: Uuid,
    sizes: &[i64],
) -> Result<Result<(), AttachmentDbError>, sqlx::Error> {
    lock_workspace_storage(tx, workspace_id).await?;
    let mut reserved = count_reserved_bytes(tx, workspace_id).await?;
    for size in sizes {
        if let Err(err) = quota.check(reserved, *size) {
            return Ok(Err(match err {
                StorageQuotaError::Upload => AttachmentDbError::UploadLimit,
                StorageQuotaError::Storage => AttachmentDbError::StorageLimit,
            }));
        }
        reserved = reserved.saturating_add(*size);
    }
    Ok(Ok(()))
}

/// Streams `from` to the fresh key `to`, counting length and SHA-256, then
/// checks the stored size and that the copy reads back with the same hash.
async fn copy_verified(
    storage: &ObjectStorage,
    from: &str,
    to: &str,
    size: i64,
) -> Result<[u8; 32], StageAttachmentError> {
    use futures_util::StreamExt;
    use sha2::{Digest, Sha256};
    use std::sync::{Arc, Mutex};

    let len = u64::try_from(size).map_err(|_| StageAttachmentError::Changed)?;
    match storage.head(from).await {
        Ok(Some(found)) if found == len => {}
        Ok(_) => return Err(StageAttachmentError::Changed),
        Err(_) => return Err(StageAttachmentError::Storage),
    }
    let read = Arc::new(Mutex::new((Sha256::new(), 0u64)));
    let counted = Arc::clone(&read);
    let source = if len == 0 {
        futures_util::stream::empty::<Result<bytes::Bytes, std::io::Error>>().boxed()
    } else {
        storage
            .open_payload_stream(from, 0, len - 1)
            .await
            .map_err(|_| StageAttachmentError::Storage)?
            .boxed()
    };
    let stream = source.map(move |chunk| {
        let chunk = chunk?;
        let mut state = counted.lock().expect("hash state");
        state.0.update(&chunk);
        state.1 += chunk.len() as u64;
        Ok::<_, std::io::Error>(chunk)
    });
    if storage.put_stream(to, stream, len).await.is_err() {
        return Err(StageAttachmentError::Storage);
    }
    let (hasher, copied) =
        std::mem::replace(&mut *read.lock().expect("hash state"), (Sha256::new(), 0));
    let sha256: [u8; 32] = hasher.finalize().into();
    match storage.head(to).await {
        Ok(Some(found)) if found == len && copied == len => {}
        Ok(_) => return Err(StageAttachmentError::Changed),
        Err(_) => return Err(StageAttachmentError::Storage),
    }
    let mut check = Sha256::new();
    if len > 0 {
        let mut back = storage
            .open_payload_stream(to, 0, len - 1)
            .await
            .map_err(|_| StageAttachmentError::Storage)?;
        while let Some(chunk) = back.next().await {
            check.update(&chunk.map_err(|_| StageAttachmentError::Storage)?);
        }
    }
    let readback: [u8; 32] = check.finalize().into();
    if readback != sha256 {
        return Err(StageAttachmentError::Changed);
    }
    Ok(sha256)
}

#[cfg(test)]
mod transfer_stage_tests {
    use super::*;
    use sha2::{Digest, Sha256};

    #[tokio::test]
    async fn copy_verified_streams_exact_bytes_and_writes_nothing_for_a_changed_source() {
        let root = std::env::temp_dir().join(format!("fvoci-transfer-stage-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&root).unwrap();
        let storage = ObjectStorage::local(root.clone());
        // Real storage keys: local storage accepts only UUID keys.
        let key = || Uuid::now_v7().to_string();
        let (source, copy, other, missing, third, empty, empty_copy) =
            (key(), key(), key(), key(), key(), key(), key());
        let bytes: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        storage.put_bytes(&source, bytes.clone()).await.unwrap();
        let len = bytes.len() as i64;
        let sha256 = copy_verified(&storage, &source, &copy, len).await.unwrap();
        let expected: [u8; 32] = Sha256::digest(&bytes).into();
        assert_eq!(sha256, expected);
        assert_eq!(
            storage
                .read_range(&copy, 0, bytes.len() as u64 - 1)
                .await
                .unwrap(),
            bytes
        );
        assert_eq!(
            copy_verified(&storage, &source, &other, len + 1).await,
            Err(StageAttachmentError::Changed)
        );
        assert_eq!(storage.head(&other).await.unwrap(), None);
        assert_eq!(
            copy_verified(&storage, &missing, &third, 3).await,
            Err(StageAttachmentError::Changed)
        );
        assert_eq!(storage.head(&third).await.unwrap(), None);
        storage.put_bytes(&empty, Vec::new()).await.unwrap();
        let none: [u8; 32] = Sha256::digest(b"").into();
        assert_eq!(
            copy_verified(&storage, &empty, &empty_copy, 0).await,
            Ok(none)
        );
        let _ = std::fs::remove_dir_all(root);
    }
}

#[cfg(test)]
mod upload_meta_tests {
    use super::*;

    fn meta(part_size: i64, part_count: i32, declared: i64) -> UploadMeta {
        UploadMeta {
            part_size_bytes: part_size,
            part_count,
            declared_size_bytes: declared,
            upload_ref: Some("u".into()),
            transfer: TransferMode::Presigned,
        }
    }

    fn part(n: i32, etag: &str, size: u64) -> PartInfo {
        PartInfo {
            part_number: n,
            etag: etag.into(),
            size_bytes: size,
        }
    }

    #[test]
    fn rows_without_a_transfer_field_are_proxy_sessions() {
        let legacy: UploadMeta = serde_json::from_value(json!({
            "part_size_bytes": 5, "part_count": 1, "declared_size_bytes": 5, "upload_ref": "u"
        }))
        .unwrap();
        assert_eq!(legacy.transfer, TransferMode::Proxy);
        let bound = serde_json::to_value(meta(5, 1, 5)).unwrap();
        assert_eq!(bound["transfer"], json!("presigned"));
    }

    #[test]
    fn part_len_gives_the_remainder_to_the_last_part() {
        let m = meta(10, 3, 25);
        assert_eq!((m.part_len(1), m.part_len(2), m.part_len(3)), (10, 10, 5));
        assert_eq!(meta(10, 2, 20).part_len(2), 10);
    }

    #[test]
    fn listed_parts_must_match_numbers_lengths_and_etags() {
        let m = meta(10, 2, 15);
        let listed = [part(1, "a", 10), part(2, "b", 5)];
        let ok = [(2, "\"b\"".to_string()), (1, "a".to_string())];
        assert!(m.listed_parts_match(&ok, &listed));
        // Wrong ETag, duplicate or missing numbers, wrong lengths, extra parts.
        assert!(!m.listed_parts_match(&[(1, "a".into()), (2, "x".into())], &listed));
        assert!(!m.listed_parts_match(&[(1, "a".into()), (1, "a".into())], &listed));
        assert!(!m.listed_parts_match(&[(1, "a".into())], &listed));
        assert!(!m.listed_parts_match(&ok, &[part(1, "a", 10), part(2, "b", 6)]));
        assert!(!m.listed_parts_match(&ok, &[part(1, "a", 9), part(2, "b", 5)]));
        assert!(!m.listed_parts_match(&ok, &[part(1, "a", 10)]));
        assert!(!m.listed_parts_match(&ok, &[part(1, "a", 10), part(2, "b", 5), part(3, "c", 1)]));
        assert!(!m.listed_parts_match(&ok, &[part(1, "a", 10), part(3, "b", 5)]));
    }
}

#[cfg(test)]
pub(crate) mod cleanup_test_hooks {
    use std::collections::HashMap;
    use std::sync::{LazyLock, Mutex};
    use tokio::sync::oneshot;
    use uuid::Uuid;
    type Pair = (oneshot::Sender<()>, oneshot::Receiver<()>);
    static HOOKS: LazyLock<Mutex<HashMap<(Uuid, u8), Pair>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));
    pub(crate) fn arm(id: Uuid, phase: u8) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
        let (entered_tx, entered_rx) = oneshot::channel();
        let (go_tx, go_rx) = oneshot::channel();
        assert!(HOOKS
            .lock()
            .unwrap()
            .insert((id, phase), (entered_tx, go_rx))
            .is_none());
        (entered_rx, go_tx)
    }
    static COMMIT_FAULTS: LazyLock<Mutex<HashMap<Uuid, (Uuid, Uuid)>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));
    pub(super) fn arm_deferred_fk(journal: Uuid, attachment: Uuid, nonexistent_document: Uuid) {
        assert!(COMMIT_FAULTS
            .lock()
            .unwrap()
            .insert(journal, (attachment, nonexistent_document))
            .is_none());
    }
    pub(super) async fn defer_fk_fault(
        op: &mut crate::db::backend::OperationTx<'_, '_>,
        row: &super::AttachmentObjectCleanup,
    ) -> Result<(), sqlx::Error> {
        let fault = COMMIT_FAULTS.lock().unwrap().remove(&row.id);
        if let Some((attachment, document)) = fault {
            let crate::db::backend::OperationTx::SqliteFamily(tx) = op else {
                return Err(sqlx::Error::Protocol(
                    "deferred-FK fixture requires actual family writer".into(),
                ));
            };
            tx.require_writer()?;
            tx.require_tenant(row.workspace_id)?;
            tx.execute("PRAGMA defer_foreign_keys=ON", &[]).await?;
            let changed = tx
                .execute(
                    "UPDATE attachments SET document_id=?2 WHERE id=?1 AND workspace_id=?3",
                    &[
                        crate::db::codec::Cell::uuid(attachment),
                        crate::db::codec::Cell::uuid(document),
                        crate::db::codec::Cell::uuid(row.workspace_id),
                    ],
                )
                .await?;
            if changed != 1 {
                return Err(sqlx::Error::Protocol(
                    "actual deferred-FK target missing".into(),
                ));
            }
        }
        Ok(())
    }
    pub(super) async fn wait(id: Uuid, phase: u8) {
        let hook = HOOKS.lock().unwrap().remove(&(id, phase));
        if let Some((entered, go)) = hook {
            let _ = entered.send(());
            let _ = go.await;
        }
    }
}

#[cfg(test)]
pub(crate) mod cleanup_tests {
    use super::*;
    pub(crate) use crate::db::attachment_preview::tests::Fixture;
    use crate::db::attachment_preview::{
        claim_preview_backend, journal_preview_key_backend, publish_preview_backend,
    };
    use crate::db::backend::Backend;
    use tokio_util::sync::CancellationToken;
    const BYTES: &[u8] = b"S17 literal object bytes for real cleanup/publish fencing";

    pub(crate) async fn journal(
        f: &Fixture,
        attachment: Uuid,
        key: &str,
        due: i64,
    ) -> AttachmentObjectCleanup {
        let row = AttachmentObjectCleanup {
            id: Uuid::now_v7(),
            workspace_id: f.workspace,
            attachment_id: attachment,
            storage_key: key.into(),
        };
        sqlx::query("INSERT INTO attachment_object_cleanups(id,workspace_id,attachment_id,storage_key,due_at) VALUES(?1,?2,?3,?4,?5)").bind(row.id.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(attachment.as_bytes().as_slice()).bind(key).bind(due).execute(&f.pool).await.unwrap();
        row
    }
    pub(crate) fn storage(f: &Fixture) -> ObjectStorage {
        ObjectStorage::local(f.root.join("s17-storage"))
    }
    async fn current(f: &Fixture, id: Uuid) -> (String, i64, i64) {
        sqlx::query_as(
            "SELECT storage_key,attempts,due_at FROM attachment_object_cleanups WHERE id=?1",
        )
        .bind(id.as_bytes().as_slice())
        .fetch_one(&f.pool)
        .await
        .unwrap()
    }
    async fn bytes(storage: &ObjectStorage, key: &str) {
        let read = storage
            .read_range(key, 0, BYTES.len() as u64 - 1)
            .await
            .unwrap();
        assert_eq!(read, BYTES);
        use sha2::Digest;
        println!(
            "S17 actual object bytes={} sha256={:x}",
            read.len(),
            sha2::Sha256::digest(&read)
        );
    }
    async fn drain(f: &Fixture, storage: &ObjectStorage) -> ObjectCleanupStats {
        reclaim_attachment_objects_backend(&f.backend, storage, None, 20)
            .await
            .unwrap()
    }
    async fn entered(receiver: tokio::sync::oneshot::Receiver<()>) {
        tokio::time::timeout(Duration::from_secs(2), receiver)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn cleanup_selected_due_filter_scope_checked_cells_and_fk() {
        let f = Fixture::new().await;
        let key = Uuid::now_v7().to_string();
        let att = Uuid::now_v7();
        let a = journal(&f, att, &key, 1).await;
        let b = journal(&f, Uuid::now_v7(), &Uuid::now_v7().to_string(), 2).await;
        journal(&f, att, &Uuid::now_v7().to_string(), i64::MAX).await;
        let mut tx = f.backend.begin_read().await.unwrap();
        assert!(tx
            .operation()
            .list_due_attachment_objects(None, 2)
            .await
            .is_err());
        tx.operation().set_system().await.unwrap();
        let rows = tx
            .operation()
            .list_due_attachment_objects(None, 2)
            .await
            .unwrap();
        assert_eq!(
            rows.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![a.id, b.id]
        );
        assert_eq!(
            tx.operation()
                .list_due_attachment_objects(Some((f.workspace, att)), 20)
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(tx
            .operation()
            .remove_attachment_cleanup_journal(&a)
            .await
            .is_err());
        tx.rollback().await.unwrap();
        let mut tx = f.backend.begin_write().await.unwrap();
        tx.operation().set_tenant(Uuid::now_v7()).await.unwrap();
        assert!(tx
            .operation()
            .attachment_cleanup_journal_current(&a)
            .await
            .is_err());
        tx.rollback().await.unwrap();
        let mut tx = f.backend.begin_write().await.unwrap();
        tx.operation().set_tenant(f.workspace).await.unwrap();
        tx.operation().set_system().await.unwrap();
        assert!(tx
            .operation()
            .reschedule_attachment_object(&a, true)
            .await
            .is_err());
        assert!(tx
            .operation()
            .list_due_attachment_objects(None, 2)
            .await
            .is_err());
        tx.rollback().await.unwrap();
        let mut tx = f.backend.begin_read().await.unwrap();
        tx.operation().set_tenant(f.workspace).await.unwrap();
        assert!(tx
            .operation()
            .attachment_cleanup_key_referenced(f.workspace, &key)
            .await
            .is_err());
        tx.rollback().await.unwrap();
        let (id, _) = f.attachment(1, "image/png").await;
        assert!(
            sqlx::query("UPDATE attachments SET document_id=?2 WHERE id=?1")
                .bind(id.as_bytes().as_slice())
                .bind(Uuid::now_v7().as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .is_err()
        );
        assert_eq!(current(&f, a.id).await.1, 0);
        f.close().await;
    }

    #[tokio::test]
    async fn cleanup_selected_real_positive_and_both_live_references() {
        let f = Fixture::new().await;
        let s = storage(&f);
        let (att, key) = f.attachment(BYTES.len() as i64, "image/png").await;
        s.put_bytes(&key, BYTES.to_vec()).await.unwrap();
        let original = journal(&f, att, &key, 1).await;
        // An original key protects even an uploading row, not only stored.
        sqlx::query("UPDATE attachments SET status='uploading',size_bytes=NULL,completed_at=NULL WHERE id=?1").bind(att.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        let preview = Uuid::now_v7().to_string();
        s.put_bytes(&preview, BYTES.to_vec()).await.unwrap();
        sqlx::query("UPDATE attachments SET variants=json_object('preview',json_object('key',?2)) WHERE id=?1").bind(att.as_bytes().as_slice()).bind(&preview).execute(&f.pool).await.unwrap();
        journal(&f, att, &preview, 2).await;
        let orphan = Uuid::now_v7().to_string();
        s.put_bytes(&orphan, BYTES.to_vec()).await.unwrap();
        journal(&f, Uuid::now_v7(), &orphan, 3).await;
        let result = drain(&f, &s).await;
        assert_eq!(
            result,
            ObjectCleanupStats {
                claimed: 3,
                reclaimed: 3,
                busy: 0,
                failed: 0
            }
        );
        bytes(&s, &key).await;
        bytes(&s, &preview).await;
        assert_eq!(s.head(&orphan).await.unwrap(), None);
        assert!(f.journals().await.is_empty());
        assert_eq!(drain(&f, &s).await.claimed, 0);
        println!(
            "S17 reference winner journal {} removed without deleting live bytes",
            original.id
        );
        f.close().await;
    }

    #[tokio::test]
    async fn cleanup_selected_storage_and_head_failures_retain_retry_pointer() {
        let f = Fixture::new().await;
        let s = storage(&f);
        let key = Uuid::now_v7().to_string();
        s.put_bytes(&key, BYTES.to_vec()).await.unwrap();
        let row = journal(&f, Uuid::now_v7(), &key, 0).await;
        let blocked = f.root.join("s17-storage/tmp").join(&key);
        std::fs::create_dir_all(blocked.parent().unwrap()).unwrap();
        std::fs::write(&blocked, b"real non-directory abort failure").unwrap();
        let before: i64 = sqlx::query_scalar(
            "SELECT unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000",
        )
        .fetch_one(&f.pool)
        .await
        .unwrap();
        assert_eq!(drain(&f, &s).await.failed, 1);
        let pointer = current(&f, row.id).await;
        assert_eq!((&pointer.0, pointer.1), (&key, 1));
        assert!(pointer.2 >= before + 60_000_000);
        bytes(&s, &key).await;
        std::fs::remove_file(blocked).unwrap();
        sqlx::query("UPDATE attachment_object_cleanups SET due_at=0 WHERE id=?1")
            .bind(row.id.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        for surviving in [false, true] {
            let (wait, go) = cleanup_test_hooks::arm(row.id, 1);
            let backend = f.backend.clone();
            let st = s.clone();
            let run = tokio::spawn(async move {
                reclaim_attachment_objects_backend(&backend, &st, None, 20)
                    .await
                    .unwrap()
            });
            entered(wait).await;
            let path = f.root.join("s17-storage/objects").join(&key);
            if surviving {
                s.put_bytes(&key, BYTES.to_vec()).await.unwrap();
            } else {
                std::fs::write(&path, b"real head ENOTDIR").unwrap();
            }
            go.send(()).unwrap();
            assert_eq!(run.await.unwrap().failed, 1);
            assert_eq!(current(&f, row.id).await.1, if surviving { 3 } else { 2 });
            if surviving {
                bytes(&s, &key).await;
            } else {
                assert!(s.head(&key).await.is_err());
                std::fs::remove_file(path).unwrap();
            }
            sqlx::query("UPDATE attachment_object_cleanups SET due_at=0 WHERE id=?1")
                .bind(row.id.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
        }
        assert_eq!(drain(&f, &s).await.reclaimed, 1);
        assert!(f.journals().await.is_empty());
        assert_eq!(s.head(&key).await.unwrap(), None);
        f.close().await;
    }

    #[tokio::test]
    async fn cleanup_selected_cancel_awaited_before_and_after_purge() {
        let f = Fixture::new().await;
        let s = storage(&f);
        for phase in [0, 1] {
            let key = Uuid::now_v7().to_string();
            s.put_bytes(&key, BYTES.to_vec()).await.unwrap();
            let row = journal(&f, Uuid::now_v7(), &key, 0).await;
            let (wait, go) = cleanup_test_hooks::arm(row.id, phase);
            let token = CancellationToken::new();
            let c = token.clone();
            let backend = f.backend.clone();
            let st = s.clone();
            let run = tokio::spawn(async move {
                reclaim_attachment_objects_backend_with_cancel(&backend, &st, None, 20, &c)
                    .await
                    .unwrap()
            });
            entered(wait).await;
            token.cancel();
            go.send(()).unwrap();
            let result = run.await.unwrap();
            assert_eq!((result.claimed, result.reclaimed, result.failed), (1, 0, 0));
            assert_eq!(current(&f, row.id).await, (key.clone(), 0, 0));
            if phase == 0 {
                bytes(&s, &key).await;
            } else {
                assert_eq!(s.head(&key).await.unwrap(), None);
            }
            assert_eq!(drain(&f, &s).await.reclaimed, 1);
            assert_eq!(s.head(&key).await.unwrap(), None);
        }
        assert!(f.journals().await.is_empty());
        f.close().await;
    }

    #[tokio::test]
    async fn cleanup_selected_stale_journal_identity_cannot_purge_old_key() {
        let f = Fixture::new().await;
        let s = storage(&f);
        let old = Uuid::now_v7().to_string();
        let new = Uuid::now_v7().to_string();
        s.put_bytes(&old, BYTES.to_vec()).await.unwrap();
        s.put_bytes(&new, BYTES.to_vec()).await.unwrap();
        let row = journal(&f, Uuid::now_v7(), &old, 0).await;
        sqlx::query("UPDATE attachment_object_cleanups SET storage_key=?2 WHERE id=?1")
            .bind(row.id.as_bytes().as_slice())
            .bind(&new)
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            reclaim_family_object(&f.backend, &s, &row, &CancellationToken::new()).await,
            Ok(CleanupDisposition::Reclaimed)
        ));
        assert_eq!(current(&f, row.id).await.0, new);
        bytes(&s, &old).await;
        bytes(&s, &new).await;
        assert_eq!(drain(&f, &s).await.reclaimed, 1);
        bytes(&s, &old).await;
        assert_eq!(s.head(&new).await.unwrap(), None);
        f.close().await;
    }

    #[tokio::test]
    async fn cleanup_selected_gc_wins_real_writer_then_late_publisher_refused() {
        let f = Fixture::new().await;
        let s = storage(&f);
        let (att, _) = f.attachment(BYTES.len() as i64, "image/png").await;
        let claim = claim_preview_backend(&f.backend).await.unwrap().unwrap();
        assert_eq!(claim.attachment_id, att);
        let key = Uuid::now_v7().to_string();
        let jid = journal_preview_key_backend(&f.backend, &claim, &key)
            .await
            .unwrap()
            .unwrap();
        s.put_bytes(&key, BYTES.to_vec()).await.unwrap();
        bytes(&s, &key).await;
        sqlx::query("UPDATE attachment_object_cleanups SET due_at=0 WHERE id=?1")
            .bind(jid.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let other = crate::db::pool::connect_sqlite_app(&f.path, 1)
            .await
            .unwrap();
        let second = Backend::Sqlite(other.clone());
        let (wait, go) = cleanup_test_hooks::arm(jid, 0);
        let b = f.backend.clone();
        let st = s.clone();
        let gc = tokio::spawn(async move {
            reclaim_attachment_objects_backend(&b, &st, None, 20)
                .await
                .unwrap()
        });
        entered(wait).await;
        let k = key.clone();
        let mut publisher = tokio::spawn(async move {
            publish_preview_backend(&second, &claim, jid, &k, 1, 1, BYTES.len() as u64)
                .await
                .unwrap()
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut publisher)
                .await
                .is_err(),
            "actual second-pool BEGIN must wait while GC holds writer through purge"
        );
        go.send(()).unwrap();
        assert_eq!(gc.await.unwrap().reclaimed, 1);
        assert!(!publisher.await.unwrap());
        assert_eq!(s.head(&key).await.unwrap(), None);
        assert!(f.journals().await.is_empty());
        assert_eq!(f.row(att).await.1, "{}");
        other.close().await;
        f.close().await;
    }

    #[tokio::test]
    async fn cleanup_selected_publisher_wins_real_writer_preserves_hash_fresh_readback() {
        let f = Fixture::new().await;
        let s = storage(&f);
        let (att, _) = f.attachment(BYTES.len() as i64, "image/png").await;
        let claim = claim_preview_backend(&f.backend).await.unwrap().unwrap();
        let key = Uuid::now_v7().to_string();
        let jid = journal_preview_key_backend(&f.backend, &claim, &key)
            .await
            .unwrap()
            .unwrap();
        s.put_bytes(&key, BYTES.to_vec()).await.unwrap();
        sqlx::query("UPDATE attachment_object_cleanups SET due_at=0 WHERE id=?1")
            .bind(jid.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let other = crate::db::pool::connect_sqlite_app(&f.path, 1)
            .await
            .unwrap();
        let second = Backend::Sqlite(other.clone());
        let mut tx = f.backend.begin_write().await.unwrap();
        let mut op = tx.operation();
        op.set_tenant(f.workspace).await.unwrap();
        assert!(op
            .publish_attachment_preview(&claim, jid, &key, 1, 1, BYTES.len() as u64)
            .await
            .unwrap());
        let st = s.clone();
        let mut gc = tokio::spawn(async move {
            reclaim_attachment_objects_backend(&second, &st, None, 20)
                .await
                .unwrap()
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut gc)
                .await
                .is_err(),
            "actual GC second-pool writer must wait for publication commit"
        );
        tx.commit().await.unwrap();
        let result = gc.await.unwrap();
        assert_eq!((result.claimed, result.reclaimed), (1, 1));
        bytes(&s, &key).await;
        assert!(f.journals().await.is_empty());
        other.close().await;
        f.pool.close().await;
        let fresh = crate::db::pool::connect_sqlite_app(&f.path, 1)
            .await
            .unwrap();
        let (variants,fk):(String,i64)=sqlx::query_as("SELECT variants,(SELECT foreign_keys FROM pragma_foreign_keys) FROM attachments WHERE id=?1").bind(att.as_bytes().as_slice()).fetch_one(&fresh).await.unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&variants).unwrap()["preview"]["key"],
            key
        );
        assert_eq!(fk, 1);
        fresh.close().await;
        println!("S17 fresh DB published preview readback {} preserved", key);
        f.close().await;
    }

    #[tokio::test]
    async fn cleanup_selected_actual_commit_failure_reconciles_stable_journal() {
        let f = Fixture::new().await;
        let s = storage(&f);
        let (att, _) = f.attachment(1, "image/png").await;
        let key = Uuid::now_v7().to_string();
        s.put_bytes(&key, BYTES.to_vec()).await.unwrap();
        let row = journal(&f, Uuid::now_v7(), &key, 0).await;
        cleanup_test_hooks::arm_deferred_fk(row.id, att, Uuid::now_v7());
        let (failed_commit, continue_reconcile) = cleanup_test_hooks::arm(row.id, 4);
        let backend = f.backend.clone();
        let st = s.clone();
        let mut consumer = tokio::spawn(async move {
            reclaim_attachment_objects_backend_with_cancel(
                &backend,
                &st,
                None,
                20,
                &CancellationToken::new(),
            )
            .await
        });
        entered(failed_commit).await;
        // The real COMMIT has failed. Reserve a real independent writer before
        // allowing the consumer's own fresh-writer reconciliation to proceed.
        let other = crate::db::pool::connect_sqlite_app(&f.path, 1)
            .await
            .unwrap();
        let second = Backend::Sqlite(other.clone());
        let held = second.begin_write().await.unwrap();
        continue_reconcile.send(()).unwrap();
        let wait = tokio::time::timeout(Duration::from_millis(50), &mut consumer).await;
        let waited_for_actual_writer = wait.is_err();
        held.rollback().await.unwrap();
        let outcome = match wait {
            Ok(joined) => joined.unwrap(),
            Err(_) => consumer.await.unwrap(),
        };
        let pointer = current(&f, row.id).await;
        let head = s.head(&key).await.unwrap();
        let document: Vec<u8> =
            sqlx::query_scalar("SELECT document_id FROM attachments WHERE id=?1")
                .bind(att.as_bytes().as_slice())
                .fetch_one(&f.pool)
                .await
                .unwrap();
        let source_is_actual_fk = match &outcome {
            Err(sqlx::Error::AnyDriverError(source)) => source
                .downcast_ref::<crate::db::backend::CommitUnknown>()
                .is_some_and(|e| e.source.as_database_error().is_some()),
            _ => false,
        };
        // Fresh writer reuse must work after the consumer returns. No manual
        // call to reconciliation can conceal a missing/unchecked await.
        let reused = f.backend.begin_write().await.unwrap();
        reused.rollback().await.unwrap();
        println!("S17 real consumer commit-error outcome={outcome:?} awaited_second_writer={waited_for_actual_writer} journal_key={} attempts={} due={} storage_head={head:?}",pointer.0,pointer.1,pointer.2);
        other.close().await;
        let expected_document = f.document.as_bytes().to_vec();
        f.close().await;
        assert!(
            waited_for_actual_writer,
            "actual consumer must await fresh-writer reconciliation, not return immediately"
        );
        assert!(
            source_is_actual_fk,
            "real consumer must return original unknown COMMIT error, never Reclaimed: {outcome:?}"
        );
        assert_eq!(pointer, (key, 0, 0));
        assert_eq!(head, None);
        assert_eq!(document, expected_document);
    }

    #[tokio::test]
    async fn cleanup_selected_cancelled_failed_purge_keeps_attempt_and_due() {
        let f = Fixture::new().await;
        let s = storage(&f);
        let key = Uuid::now_v7().to_string();
        s.put_bytes(&key, BYTES.to_vec()).await.unwrap();
        let row = journal(&f, Uuid::now_v7(), &key, 0).await;
        let blocked = f.root.join("s17-storage/tmp").join(&key);
        std::fs::create_dir_all(blocked.parent().unwrap()).unwrap();
        std::fs::write(&blocked, b"real abort ENOTDIR").unwrap();
        let (settled, go) = cleanup_test_hooks::arm(row.id, 2);
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let backend = f.backend.clone();
        let st = s.clone();
        let consumer = tokio::spawn(async move {
            reclaim_attachment_objects_backend_with_cancel(&backend, &st, None, 20, &token)
                .await
                .unwrap()
        });
        entered(settled).await;
        // Actual purge already returned ENOTDIR, not a synthetic callback Err.
        cancel.cancel();
        go.send(()).unwrap();
        let stats = consumer.await.unwrap();
        let pointer = current(&f, row.id).await;
        bytes(&s, &key).await;
        println!(
            "S17 cancelled real failed purge stats={stats:?} journal_key={} attempts={} due={}",
            pointer.0, pointer.1, pointer.2
        );
        if pointer != (key.clone(), 0, 0) || (stats.claimed, stats.failed) != (1, 0) {
            // Retain the closed actual failed fixture for root diagnostics.
            f.backend.close().await.unwrap();
            println!("S17 closed original failed fixture {}", f.root.display());
            assert_eq!(
                pointer,
                (key.clone(), 0, 0),
                "cancelled settled purge must retain exact pointer/attempt/due"
            );
            assert_eq!((stats.claimed, stats.failed), (1, 0));
        }
        let before: i64 = sqlx::query_scalar(
            "SELECT unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000",
        )
        .fetch_one(&f.pool)
        .await
        .unwrap();
        let (settled, go) = cleanup_test_hooks::arm(row.id, 2);
        let backend = f.backend.clone();
        let st = s.clone();
        let consumer = tokio::spawn(async move {
            reclaim_attachment_objects_backend(&backend, &st, None, 20)
                .await
                .unwrap()
        });
        entered(settled).await;
        go.send(()).unwrap();
        let stats = consumer.await.unwrap();
        assert_eq!((stats.claimed, stats.failed), (1, 1));
        let pointer = current(&f, row.id).await;
        assert_eq!((&pointer.0, pointer.1), (&key, 1));
        assert!(pointer.2 >= before + 60_000_000);
        bytes(&s, &key).await;
        std::fs::remove_file(blocked).unwrap();
        sqlx::query("UPDATE attachment_object_cleanups SET due_at=0 WHERE id=?1")
            .bind(row.id.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(drain(&f, &s).await.reclaimed, 1);
        f.close().await;
    }

    #[tokio::test]
    async fn cleanup_selected_stale_tuple_ties_and_late_stored_row_guard() {
        let f = Fixture::new().await;
        let s = storage(&f);
        let mut ids = Vec::new();
        let at = 1_700_000_000_000_007i64;
        for _ in 0..3 {
            let (id, key) = f.attachment(4, "application/octet-stream").await;
            s.put_bytes(&key, b"part".to_vec()).await.unwrap();
            sqlx::query("UPDATE attachments SET status='uploading',size_bytes=NULL,completed_at=NULL,created_at=?2 WHERE id=?1").bind(id.as_bytes().as_slice()).bind(at).execute(&f.pool).await.unwrap();
            ids.push((id, key));
        }
        ids.sort_by_key(|v| v.0);
        let cutoff = DateTime::from_timestamp_micros(at + 1).unwrap();
        let a = list_stale_uploading_backend(&f.backend, cutoff, None, 1)
            .await
            .unwrap();
        assert_eq!(a[0].id, ids[0].0);
        let b =
            list_stale_uploading_backend(&f.backend, cutoff, Some((a[0].created_at, a[0].id)), 1)
                .await
                .unwrap();
        assert_eq!(b[0].id, ids[1].0);
        let c =
            list_stale_uploading_backend(&f.backend, cutoff, Some((b[0].created_at, b[0].id)), 1)
                .await
                .unwrap();
        assert_eq!(c[0].id, ids[2].0);
        sqlx::query(
            "UPDATE attachments SET status='stored',size_bytes=4,completed_at=?2 WHERE id=?1",
        )
        .bind(ids[1].0.as_bytes().as_slice())
        .bind(at)
        .execute(&f.pool)
        .await
        .unwrap();
        assert!(
            !gc_stale_upload_row_backend(&f.backend, &s, f.workspace, ids[1].0)
                .await
                .unwrap()
        );
        assert_eq!(s.read_range(&ids[1].1, 0, 3).await.unwrap(), b"part");
        assert!(
            gc_stale_upload_row_backend(&f.backend, &s, f.workspace, ids[0].0)
                .await
                .unwrap()
        );
        assert!(
            gc_stale_upload_row_backend(&f.backend, &s, f.workspace, ids[2].0)
                .await
                .unwrap()
        );
        assert_eq!(s.head(&ids[0].1).await.unwrap(), None);
        assert_eq!(s.head(&ids[2].1).await.unwrap(), None);
        f.close().await;
    }

    #[tokio::test]
    async fn cleanup_selected_assembling_busy_does_not_block_healthy_journal() {
        let f = Fixture::new().await;
        let s = storage(&f);
        let (att, _) = f.attachment(1, "image/png").await;
        sqlx::query("UPDATE attachments SET status='assembling',size_bytes=NULL,completed_at=NULL WHERE id=?1").bind(att.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        let busykey = Uuid::now_v7().to_string();
        s.put_bytes(&busykey, BYTES.to_vec()).await.unwrap();
        let busy = journal(&f, att, &busykey, 0).await;
        let healthy = Uuid::now_v7().to_string();
        s.put_bytes(&healthy, BYTES.to_vec()).await.unwrap();
        journal(&f, Uuid::now_v7(), &healthy, 1).await;
        let result = drain(&f, &s).await;
        assert_eq!(
            result,
            ObjectCleanupStats {
                claimed: 2,
                reclaimed: 1,
                busy: 1,
                failed: 0
            }
        );
        let row = current(&f, busy.id).await;
        assert_eq!(row.1, 0);
        assert!(row.2 > 60_000_000);
        bytes(&s, &busykey).await;
        assert_eq!(s.head(&healthy).await.unwrap(), None);
        f.close().await;
    }
}
