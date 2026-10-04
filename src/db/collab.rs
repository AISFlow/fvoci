//! Collaboration DB boundary for document and task rooms: access checks,
//! durable state (snapshot, update tail, op receipts) and the writer fence.
//! The database stays the ACL, durability and fence authority.
//!
//! ## Caps
//! - `MAX_COLLAB_SNAPSHOT_BYTES` / `MAX_COLLAB_UPDATE_BYTES`: 8 MiB each.
//! - `MAX_COLLAB_TAIL_UPDATES`: 64 tail rows.
//! - `MAX_COLLAB_LOAD_BYTES`: snapshot + tail combined 32 MiB (engine reload budget).
//! - Engine Load framed JSON cap 48 MiB (DB refuses tails the engine cannot reload).
//! - Op receipts are append-only identity rows (seq/actor/len/digest); history grows without
//!   automatic retention or a per-document receipt count cap. Memory/recovery stays bounded by
//!   the 32 MiB / 64-row tail load budget above.
//!
//! ## Op receipts (immutable identity, no raw payload)
//! Receipts retain `seq`, `actor_user_id`, `payload_len`, and `payload_sha256` only.
//! They do not store historical update bytes; callers cannot recover raw payload from a
//! receipt alone and must load the canonical snapshot plus tail for payload bytes.
//! Compaction keeps receipts and fences its cutoff exactly.
//!
//! ## Lock order within one transaction
//! Every authorizing transaction starts with `lock_collab_actor` (steps 1-5):
//! 1. `lock_membership_users` for the actor (the membership advisory lock;
//!    keys sorted when several users are locked)
//! 2. `users` + `sessions` `FOR UPDATE` via `recheck_session`
//! 3. `memberships` `FOR UPDATE` via `membership_role_for_update`
//! 4. project documents and tasks: `projects` `FOR SHARE` (before the resource
//!    row, matching the project → document/task order of their mutations)
//! 5. `documents` `FOR UPDATE`, or `tasks` `FOR NO KEY UPDATE`
//! 6. the state row (`document_states` / `task_states`) `FOR UPDATE`
//!
//! Empty-state seed only: a transaction advisory lock in
//! `COLLAB_INIT_LOCK_NAMESPACE`, keyed by `lock_key_from_uuid(resource_id)`.
//! Room fence: `collab::guard::RoomGuard` holds a session advisory lock in
//! `COLLAB_ROOM_SESSION_LOCK_NAMESPACE`, keyed the same way, on a dedicated
//! connection for the room's lifetime (not acquired in this module).

use std::time::Instant;

use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{Connection, PgConnection, PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::collab::derived_body::PreparedDerivedBody;
use crate::db::backend::{Backend, DbTransaction, FamilyTx, OperationTx};
use crate::db::codec::Cell;
use crate::db::context::{lock_key_from_uuid, set_tenant};
use crate::db::documents::empty_document_json;
use crate::db::identity::{append_audit, append_event, AuditAppend, EventAppend};
use crate::projects::ProjectPermission;

pub use crate::collab::derived_body::DOCUMENT_MAX_BODY_BYTES;

/// Stage timings of one collab transaction, logged as `collab.stage` fields.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CollabDbStageTimings {
    /// Not measured (always 0): the timed append runs on the room's fence
    /// connection; the pool variant discards its timings. Kept as a stable log
    /// field.
    pub pool_wait_us: u64,
    pub advisory_lock_us: u64,
    pub row_lock_us: u64,
    pub stmt_us: u64,
    pub commit_us: u64,
}

/// Serializes seeding a missing state row (transaction lock keyed by the
/// resource id), so concurrent first claims insert it once.
pub const COLLAB_INIT_LOCK_NAMESPACE: i32 = 1_907_004;
/// Room fence: `RoomGuard` holds this session lock, keyed by the resource id,
/// for the room's lifetime, so at most one room, in any server process, owns a
/// resource at a time. No other two-int advisory lock may use this namespace
/// (see `db::context` tests).
pub const COLLAB_ROOM_SESSION_LOCK_NAMESPACE: i32 = 1_907_007;
pub const COLLAB_STATE_ENCODING_V1: i16 = 1;
pub const MAX_COLLAB_SNAPSHOT_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_COLLAB_UPDATE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_COLLAB_TAIL_UPDATES: i64 = 64;
pub const MAX_COLLAB_LOAD_BYTES: i64 = 32 * 1024 * 1024;

/// Minimal fixed empty Yjs updateV1 bytes for the canonical empty Tiptap seed only.
/// This layer does not parse CRDT payloads.
const EMPTY_YJS_STATE_V1: &[u8] = &[0, 0];

pub use crate::collab::wire::CollabKind;

/// Per-kind collab tables. The task tables (037) mirror the document tables
/// (004/005) column for column; the closed [`CollabKind`] selects them.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CollabTables {
    kind: CollabKind,
    states: &'static str,
    updates: &'static str,
    receipts: &'static str,
    id_col: &'static str,
    /// Resource table holding `content_json` / `text` / `chosung`.
    resource: &'static str,
    /// Event/audit `target_type` and verb prefix.
    target_type: &'static str,
    /// Event payload key for the resource id.
    payload_key: &'static str,
}

const DOCUMENT_TABLES: CollabTables = CollabTables {
    kind: CollabKind::Document,
    states: "fvoci.document_states",
    updates: "fvoci.document_collab_updates",
    receipts: "fvoci.document_collab_op_receipts",
    id_col: "document_id",
    resource: "fvoci.documents",
    target_type: "document",
    payload_key: "documentId",
};

const TASK_TABLES: CollabTables = CollabTables {
    kind: CollabKind::Task,
    states: "fvoci.task_states",
    updates: "fvoci.task_collab_updates",
    receipts: "fvoci.task_collab_op_receipts",
    id_col: "task_id",
    resource: "fvoci.tasks",
    target_type: "task",
    payload_key: "taskId",
};

impl CollabTables {
    pub(crate) fn for_kind(kind: CollabKind) -> &'static Self {
        match kind {
            CollabKind::Document => &DOCUMENT_TABLES,
            CollabKind::Task => &TASK_TABLES,
        }
    }

    /// Fill the fixed table/column placeholders of a static SQL template.
    fn sql(&self, template: &str) -> String {
        template
            .replace("{states}", self.states)
            .replace("{updates}", self.updates)
            .replace("{receipts}", self.receipts)
            .replace("{id}", self.id_col)
            .replace("{resource}", self.resource)
    }

    fn verb(&self, action: &str) -> String {
        format!("{}.{action}", self.target_type)
    }
}

fn payload_sha256(payload: &[u8]) -> Vec<u8> {
    Sha256::digest(payload).to_vec()
}

fn load_budget_allows(
    snapshot_len: i64,
    tail_count: i64,
    tail_bytes: i64,
) -> Result<(), CollabDbError> {
    if tail_count > MAX_COLLAB_TAIL_UPDATES {
        return Err(CollabDbError::StateBudgetExceeded);
    }
    if snapshot_len + tail_bytes > MAX_COLLAB_LOAD_BYTES {
        return Err(CollabDbError::StateBudgetExceeded);
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollabDbError {
    NotFound,
    Forbidden,
    StaleWriter,
    PayloadTooLarge,
    OpIdConflict,
    StaleCutoff,
    InvalidCutoff,
    StateBudgetExceeded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollabUpdateRow {
    pub seq: i64,
    pub op_id: Uuid,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollabLoadState {
    pub snapshot: Vec<u8>,
    pub tail: Vec<CollabUpdateRow>,
    pub writer_generation: i64,
    pub snapshot_cutoff_seq: i64,
    pub tail_seq: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimWriterResult {
    pub writer_generation: i64,
    pub load: CollabLoadState,
}

/// Opaque ownership of the document room's SQLite-family lease. A PostgreSQL
/// room instead retains its detached session advisory-lock connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FamilyRoomFence {
    pub(crate) workspace_id: Uuid,
    pub(crate) document_id: Uuid,
    pub(crate) owner_token: Uuid,
    pub(crate) fence: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FamilyRoomClaim {
    pub fence: FamilyRoomFence,
    pub native: ClaimWriterResult,
}

async fn family_room_now(tx: &mut FamilyTx) -> Result<i64, sqlx::Error> {
    let rows = tx
        .query(
            "SELECT unixepoch()*1000000+CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000",
            &[],
        )
        .await?;
    rows.first()
        .ok_or(sqlx::Error::RowNotFound)?
        .cell(0)?
        .integer()
}

fn room_lease_micros(lease: std::time::Duration) -> Result<i64, sqlx::Error> {
    let micros = i64::try_from(lease.as_micros())
        .map_err(|_| sqlx::Error::Protocol("room lease exceeds signed microseconds".into()))?;
    if micros == 0 {
        return Err(sqlx::Error::Protocol("room lease must be positive".into()));
    }
    Ok(micros)
}

impl OperationTx<'_, '_> {
    /// Caller has already authorized the document in this reserved writer.
    /// A retry of an unconfirmed claim keeps its original owner token; a live
    /// matching owner returns the same fence without advancing generation.
    async fn claim_family_room_fence(
        &mut self,
        workspace: Uuid,
        document: Uuid,
        owner: Uuid,
        lease: std::time::Duration,
    ) -> Result<Result<(FamilyRoomFence, bool), CollabDbError>, sqlx::Error> {
        let Self::SqliteFamily(tx) = self else {
            return Err(sqlx::Error::Protocol(
                "family room lease requires SQLite family".into(),
            ));
        };
        tx.require_writer()?;
        tx.require_tenant(workspace)?;
        let now = family_room_now(tx).await?;
        let expires = now.checked_add(room_lease_micros(lease)?).ok_or_else(|| {
            sqlx::Error::Protocol("room lease expiry exceeds signed microseconds".into())
        })?;
        let rows = tx.query(
            "SELECT owner_token,fence,expires_at FROM collab_room_fences WHERE workspace_id=?1 AND document_id=?2",
            &[Cell::uuid(workspace),Cell::uuid(document)],
        ).await?;
        if let Some(row) = rows.first() {
            let old_owner = row.cell(0)?.id()?;
            let fence = row.cell(1)?.integer()?;
            let old_expiry = row.cell(2)?.integer()?;
            if fence <= 0 {
                return Err(sqlx::Error::Protocol("room fence must be positive".into()));
            }
            if old_owner == owner {
                if old_expiry <= now {
                    return Ok(Err(CollabDbError::StaleWriter));
                }
                return Ok(Ok((
                    FamilyRoomFence {
                        workspace_id: workspace,
                        document_id: document,
                        owner_token: owner,
                        fence,
                    },
                    false,
                )));
            }
            if old_expiry > now {
                return Ok(Err(CollabDbError::StaleWriter));
            }
        }
        // Allocation is in the same writer transaction as lease, native
        // generation and state. Purging a target cannot reset this counter.
        let rows = tx.query(
            "UPDATE collab_fence_counter SET next_fence=next_fence+1 WHERE id=1 AND next_fence<9223372036854775807 RETURNING next_fence-1",
            &[],
        ).await?;
        let fence = rows
            .first()
            .ok_or_else(|| sqlx::Error::Protocol("room fence counter absent or exhausted".into()))?
            .cell(0)?
            .integer()?;
        let changed = tx.execute(
            "INSERT INTO collab_room_fences(workspace_id,document_id,owner_token,fence,expires_at) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(workspace_id,document_id) DO UPDATE SET owner_token=excluded.owner_token,fence=excluded.fence,expires_at=excluded.expires_at WHERE collab_room_fences.expires_at<=?6",
            &[Cell::uuid(workspace),Cell::uuid(document),Cell::uuid(owner),Cell::Integer(fence),Cell::Integer(expires),Cell::Integer(now)],
        ).await?;
        if changed != 1 {
            return Ok(Err(CollabDbError::StaleWriter));
        }
        Ok(Ok((
            FamilyRoomFence {
                workspace_id: workspace,
                document_id: document,
                owner_token: owner,
                fence,
            },
            true,
        )))
    }

    pub(crate) async fn verify_family_room_fence(
        &mut self,
        fence: FamilyRoomFence,
    ) -> Result<bool, sqlx::Error> {
        let Self::SqliteFamily(tx) = self else {
            return Err(sqlx::Error::Protocol(
                "family room lease requires SQLite family".into(),
            ));
        };
        tx.require_writer()?;
        tx.require_tenant(fence.workspace_id)?;
        let now = family_room_now(tx).await?;
        let rows = tx.query(
            "SELECT EXISTS(SELECT 1 FROM collab_room_fences WHERE workspace_id=?1 AND document_id=?2 AND owner_token=?3 AND fence=?4 AND expires_at>?5)",
            &[Cell::uuid(fence.workspace_id),Cell::uuid(fence.document_id),Cell::uuid(fence.owner_token),Cell::Integer(fence.fence),Cell::Integer(now)],
        ).await?;
        rows.first()
            .ok_or(sqlx::Error::RowNotFound)?
            .cell(0)?
            .boolean()
    }
}

/// Atomically authorize, claim the family document room and claim native
/// writer generation. No product lease default is selected here: the caller
/// supplies the measured runtime bound, and protocol fixtures supply theirs.
pub async fn claim_family_document_room(
    backend: &Backend,
    workspace: Uuid,
    actor: Uuid,
    credential: Uuid,
    document: Uuid,
    owner: Uuid,
    lease: std::time::Duration,
) -> Result<Result<FamilyRoomClaim, CollabDbError>, sqlx::Error> {
    if matches!(backend, Backend::Postgres(_)) {
        return Err(sqlx::Error::Protocol(
            "PostgreSQL rooms require the session guard".into(),
        ));
    }
    let mut tx = backend.begin_write().await?;
    let native = tx
        .operation()
        .load_collab_native(
            CollabKind::Document,
            workspace,
            actor,
            credential,
            document,
            NativeLoadMode::Writer,
        )
        .await?;
    let mut native = match native {
        Ok(native) => native,
        Err(error) => {
            tx.rollback().await?;
            return Ok(Err(error));
        }
    };
    let (fence, new_owner) = match tx
        .operation()
        .claim_family_room_fence(workspace, document, owner, lease)
        .await?
    {
        Ok(claimed) => claimed,
        Err(error) => {
            tx.rollback().await?;
            return Ok(Err(error));
        }
    };
    if new_owner {
        let generation = tx
            .operation()
            .bump_native_writer_generation(
                CollabTables::for_kind(CollabKind::Document),
                workspace,
                document,
            )
            .await?;
        let Some(generation) = generation else {
            tx.rollback().await?;
            return Ok(Err(CollabDbError::NotFound));
        };
        native.writer_generation = generation;
        native.load.writer_generation = generation;
    }
    if !tx.operation().verify_family_room_fence(fence).await? {
        tx.rollback().await?;
        return Ok(Err(CollabDbError::StaleWriter));
    }
    tx.commit().await.map_err(|error| error.source)?;
    Ok(Ok(FamilyRoomClaim { fence, native }))
}

/// An expired original owner cannot renew or release another owner's lease.
pub async fn renew_family_document_room(
    backend: &Backend,
    fence: FamilyRoomFence,
    lease: std::time::Duration,
) -> Result<bool, sqlx::Error> {
    let mut tx = backend.begin_write().await?;
    tx.operation().set_tenant(fence.workspace_id).await?;
    let crate::db::backend::DbTransaction::SqliteFamily(family) = &mut tx else {
        return Err(sqlx::Error::Protocol(
            "family room lease requires SQLite family".into(),
        ));
    };
    let now = family_room_now(family).await?;
    let expires = now.checked_add(room_lease_micros(lease)?).ok_or_else(|| {
        sqlx::Error::Protocol("room lease expiry exceeds signed microseconds".into())
    })?;
    let changed = family.execute(
        "UPDATE collab_room_fences SET expires_at=?5 WHERE workspace_id=?1 AND document_id=?2 AND owner_token=?3 AND fence=?4 AND expires_at>?6",
        &[Cell::uuid(fence.workspace_id),Cell::uuid(fence.document_id),Cell::uuid(fence.owner_token),Cell::Integer(fence.fence),Cell::Integer(expires),Cell::Integer(now)],
    ).await?;
    tx.commit().await.map_err(|error| error.source)?;
    Ok(changed == 1)
}

pub async fn release_family_document_room(
    backend: &Backend,
    fence: FamilyRoomFence,
) -> Result<bool, sqlx::Error> {
    let mut tx = backend.begin_write().await?;
    tx.operation().set_tenant(fence.workspace_id).await?;
    let crate::db::backend::DbTransaction::SqliteFamily(family) = &mut tx else {
        return Err(sqlx::Error::Protocol(
            "family room lease requires SQLite family".into(),
        ));
    };
    let now = family_room_now(family).await?;
    let changed = family.execute(
        "UPDATE collab_room_fences SET expires_at=?5 WHERE workspace_id=?1 AND document_id=?2 AND owner_token=?3 AND fence=?4",
        &[Cell::uuid(fence.workspace_id),Cell::uuid(fence.document_id),Cell::uuid(fence.owner_token),Cell::Integer(fence.fence),Cell::Integer(now)],
    ).await?;
    tx.commit().await.map_err(|error| error.source)?;
    Ok(changed == 1)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppendCollabResult {
    Committed { seq: i64 },
    DuplicateAck { seq: i64 },
}

/// A borrowed native program has not committed and cannot issue a persist ACK.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PreparedNativeAppend {
    Appended { seq: i64 },
    Replay { seq: i64 },
}

impl PreparedNativeAppend {
    pub(crate) fn seq(self) -> i64 {
        match self {
            Self::Appended { seq } | Self::Replay { seq } => seq,
        }
    }
}

pub struct AppendCollabInput<'a> {
    pub workspace_id: Uuid,
    pub actor_user_id: Uuid,
    pub session_id: Uuid,
    pub document_id: Uuid,
    pub writer_generation: i64,
    pub expected_tail_seq: i64,
    pub op_id: Uuid,
    pub payload: &'a [u8],
    pub client_ip: Option<&'a str>,
}

pub struct CompactCollabInput<'a> {
    pub workspace_id: Uuid,
    pub actor_user_id: Uuid,
    pub session_id: Uuid,
    pub document_id: Uuid,
    pub writer_generation: i64,
    pub cutoff_seq: i64,
    pub expected_tail_seq: i64,
    pub new_snapshot: &'a [u8],
    pub client_ip: Option<&'a str>,
}

struct CollabAuditRecord<'a> {
    workspace_id: Uuid,
    actor_user_id: Uuid,
    document_id: Uuid,
    op_id: Uuid,
    seq: i64,
    writer_generation: i64,
    client_ip: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollabOperationLookup {
    pub seq: i64,
    pub payload_len: i64,
    pub payload_sha256: Vec<u8>,
    pub actor_user_id: Uuid,
}

pub struct VerifyCollabInput<'a> {
    pub workspace_id: Uuid,
    pub actor_user_id: Uuid,
    pub session_id: Uuid,
    pub document_id: Uuid,
    pub op_id: Uuid,
    pub expected_payload_len: i64,
    pub expected_payload_sha256: &'a [u8],
    pub expected_actor_user_id: Uuid,
}

pub struct ProjectDerivedBodyInput {
    pub workspace_id: Uuid,
    pub actor_user_id: Uuid,
    pub session_id: Uuid,
    pub document_id: Uuid,
    pub writer_generation: i64,
    pub expected_tail_seq: i64,
    prepared: PreparedDerivedBody,
}

impl ProjectDerivedBodyInput {
    pub fn new(
        workspace_id: Uuid,
        actor_user_id: Uuid,
        session_id: Uuid,
        document_id: Uuid,
        writer_generation: i64,
        expected_tail_seq: i64,
        prepared: PreparedDerivedBody,
    ) -> Self {
        Self {
            workspace_id,
            actor_user_id,
            session_id,
            document_id,
            writer_generation,
            expected_tail_seq,
            prepared,
        }
    }

    pub fn prepared(&self) -> &PreparedDerivedBody {
        &self.prepared
    }

    pub fn into_parts(self) -> (Uuid, Uuid, Uuid, Uuid, i64, i64, PreparedDerivedBody) {
        (
            self.workspace_id,
            self.actor_user_id,
            self.session_id,
            self.document_id,
            self.writer_generation,
            self.expected_tail_seq,
            self.prepared,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectDerivedBodyResult {
    Updated,
    Unchanged,
    /// Empty Yjs seed at `tail_seq == 0` must not overwrite the paragraph seed JSON.
    SkippedSeed,
}

type StateRow = (Vec<u8>, i16, i64, i64, i64, DateTime<Utc>);
/// `project_id, archived_at, deleted_at` of a locked task row.
type TaskLockRow = (Uuid, Option<DateTime<Utc>>, Option<DateTime<Utc>>);

async fn lock_collab_init(
    tx: &mut Transaction<'_, Postgres>,
    document_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT pg_advisory_xact_lock($1, $2)")
        .bind(COLLAB_INIT_LOCK_NAMESPACE)
        .bind(lock_key_from_uuid(document_id))
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Result of the single collab access check (wiki/project document or task).
pub(crate) struct CollabDocumentAccess {
    pub(crate) permission: ProjectPermission,
    /// Document status `archived` or the owning project archived: the room is read-only.
    pub(crate) archived: bool,
}

/// Locks and authorizes a live wiki or project document for collab.
///
/// Project documents use the project's effective permission (visibility, direct
/// and group grants) under a `FOR SHARE` project row lock taken before the
/// document row lock, matching the project → document order of project document
/// mutations. Wiki documents use `document_permission`. Returns `None` when the
/// document is missing, trashed, in a trashed project, or changed affiliation
/// between the unlocked read and the row lock.
impl OperationTx<'_, '_> {
    async fn lock_collab_document_access(
        &mut self,
        workspace_id: Uuid,
        actor_user_id: Uuid,
        document_id: Uuid,
    ) -> Result<Option<CollabDocumentAccess>, sqlx::Error> {
        let affiliation = self
            .collab_document_affiliation(workspace_id, document_id)
            .await?;
        let Some((expected_project_id,)) = affiliation else {
            return Ok(None);
        };
        let project_access = match expected_project_id {
            Some(project_id) => {
                match self
                    .share_lock_project_permission(workspace_id, actor_user_id, project_id)
                    .await?
                {
                    Some(access) => Some(access),
                    None => return Ok(None),
                }
            }
            None => None,
        };
        let row = self.collab_document_lock(workspace_id, document_id).await?;
        let Some((project_id, status, deleted_at)) = row else {
            return Ok(None);
        };
        if deleted_at.is_some() || project_id != expected_project_id {
            return Ok(None);
        }
        let (permission, project_archived) = match project_access {
            Some(access) => access,
            None => (
                self.document_permission(workspace_id, actor_user_id, document_id, true)
                    .await?,
                false,
            ),
        };
        Ok(Some(CollabDocumentAccess {
            permission,
            archived: project_archived || status == "archived",
        }))
    }

    /// Locks and authorizes a live task for collab: the owning project `FOR SHARE`
    /// (effective project permission) and then the task row `FOR NO KEY UPDATE`,
    /// the same project → task order as task mutations. Returns `None` when the
    /// task is missing or trashed, its project is trashed, or the task moved to
    /// another project between the unlocked read and the row lock. An archived
    /// task or project yields a read-only room.
    async fn lock_collab_task_access(
        &mut self,
        workspace_id: Uuid,
        actor_user_id: Uuid,
        task_id: Uuid,
    ) -> Result<Option<CollabDocumentAccess>, sqlx::Error> {
        let expected = self.collab_task_affiliation(workspace_id, task_id).await?;
        let Some((expected_project_id,)) = expected else {
            return Ok(None);
        };
        let Some((permission, project_archived)) = self
            .share_lock_project_permission(workspace_id, actor_user_id, expected_project_id)
            .await?
        else {
            return Ok(None);
        };
        let row = self.collab_task_lock(workspace_id, task_id).await?;
        let Some((project_id, archived_at, deleted_at)) = row else {
            return Ok(None);
        };
        if deleted_at.is_some() || project_id != expected_project_id {
            return Ok(None);
        }
        Ok(Some(CollabDocumentAccess {
            permission,
            archived: project_archived || archived_at.is_some(),
        }))
    }

    async fn lock_collab_access(
        &mut self,
        kind: CollabKind,
        workspace_id: Uuid,
        actor_user_id: Uuid,
        resource_id: Uuid,
    ) -> Result<Option<CollabDocumentAccess>, sqlx::Error> {
        match kind {
            CollabKind::Document => {
                self.lock_collab_document_access(workspace_id, actor_user_id, resource_id)
                    .await
            }
            CollabKind::Task => {
                self.lock_collab_task_access(workspace_id, actor_user_id, resource_id)
                    .await
            }
        }
    }

    async fn collab_document_affiliation(
        &mut self,
        workspace: Uuid,
        document: Uuid,
    ) -> Result<Option<(Option<Uuid>,)>, sqlx::Error> {
        match self {
            Self::Postgres(tx) => {
                sqlx::query_as(
                    "SELECT project_id FROM fvoci.documents WHERE workspace_id=$1 AND id=$2",
                )
                .bind(workspace)
                .bind(document)
                .fetch_optional(&mut ***tx)
                .await
            }
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_tenant(workspace)?;
                tx.query(
                    "SELECT project_id FROM documents WHERE workspace_id=?1 AND id=?2",
                    &[Cell::uuid(workspace), Cell::uuid(document)],
                )
                .await?
                .first()
                .map(|row| Ok((row.cell(0)?.optional(Cell::id)?,)))
                .transpose()
            }
        }
    }
    async fn collab_document_lock(
        &mut self,
        workspace: Uuid,
        document: Uuid,
    ) -> Result<Option<(Option<Uuid>, String, Option<DateTime<Utc>>)>, sqlx::Error> {
        match self {
            Self::Postgres(tx) => sqlx::query_as(
                "SELECT project_id,status,deleted_at FROM fvoci.documents WHERE workspace_id=$1 AND id=$2 FOR UPDATE"
            ).bind(workspace).bind(document).fetch_optional(&mut ***tx).await,
            Self::SqliteFamily(tx) => {
                tx.require_writer()?; tx.require_tenant(workspace)?;
                tx.query("SELECT project_id,status,deleted_at FROM documents WHERE workspace_id=?1 AND id=?2",
                    &[Cell::uuid(workspace),Cell::uuid(document)]).await?.first()
                    .map(|row| Ok((row.cell(0)?.optional(Cell::id)?,row.cell(1)?.string()?,row.cell(2)?.optional(Cell::datetime)?))).transpose()
            }
        }
    }
    async fn collab_task_affiliation(
        &mut self,
        workspace: Uuid,
        task: Uuid,
    ) -> Result<Option<(Uuid,)>, sqlx::Error> {
        match self {
            Self::Postgres(tx) => sqlx::query_as(
                "SELECT project_id FROM fvoci.tasks WHERE workspace_id=$1 AND id=$2 AND deleted_at IS NULL"
            ).bind(workspace).bind(task).fetch_optional(&mut ***tx).await,
            Self::SqliteFamily(tx) => {
                tx.require_writer()?; tx.require_tenant(workspace)?;
                tx.query("SELECT project_id FROM tasks WHERE workspace_id=?1 AND id=?2 AND deleted_at IS NULL",
                    &[Cell::uuid(workspace),Cell::uuid(task)]).await?.first()
                    .map(|row| Ok((row.cell(0)?.id()?,))).transpose()
            }
        }
    }
    async fn collab_task_lock(
        &mut self,
        workspace: Uuid,
        task: Uuid,
    ) -> Result<Option<TaskLockRow>, sqlx::Error> {
        match self {
            Self::Postgres(tx) => sqlx::query_as(
                "SELECT project_id,archived_at,deleted_at FROM fvoci.tasks WHERE workspace_id=$1 AND id=$2 FOR NO KEY UPDATE"
            ).bind(workspace).bind(task).fetch_optional(&mut ***tx).await,
            Self::SqliteFamily(tx) => {
                tx.require_writer()?; tx.require_tenant(workspace)?;
                tx.query("SELECT project_id,archived_at,deleted_at FROM tasks WHERE workspace_id=?1 AND id=?2",
                    &[Cell::uuid(workspace),Cell::uuid(task)]).await?.first()
                    .map(|row| Ok((row.cell(0)?.id()?,row.cell(1)?.optional(Cell::datetime)?,row.cell(2)?.optional(Cell::datetime)?))).transpose()
            }
        }
    }
}

async fn load_resource_content(
    tx: &mut Transaction<'_, Postgres>,
    t: &CollabTables,
    workspace_id: Uuid,
    resource_id: Uuid,
) -> Result<(Value,), sqlx::Error> {
    OperationTx::Postgres(tx)
        .load_collab_resource_content(t, workspace_id, resource_id)
        .await
        .map(|body| (body,))
}

async fn ensure_collab_state_row(
    tx: &mut Transaction<'_, Postgres>,
    t: &CollabTables,
    workspace_id: Uuid,
    document_id: Uuid,
    content_json: &Value,
) -> Result<Result<(), CollabDbError>, sqlx::Error> {
    OperationTx::Postgres(tx)
        .ensure_collab_state(t, workspace_id, document_id, content_json)
        .await
}

impl OperationTx<'_, '_> {
    async fn load_collab_resource_content(
        &mut self,
        t: &CollabTables,
        workspace: Uuid,
        resource: Uuid,
    ) -> Result<Value, sqlx::Error> {
        match self {
            Self::Postgres(tx) => {
                let (body,): (Value,) = sqlx::query_as(&t.sql(
                    "SELECT content_json FROM {resource} WHERE workspace_id = $1 AND id = $2",
                ))
                .bind(workspace)
                .bind(resource)
                .fetch_one(&mut ***tx)
                .await?;
                Ok(body)
            }
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_tenant(workspace)?;
                let statement = match t.kind {
                    CollabKind::Document => {
                        "SELECT content_json FROM documents WHERE workspace_id=?1 AND id=?2"
                    }
                    CollabKind::Task => {
                        "SELECT content_json FROM tasks WHERE workspace_id=?1 AND id=?2"
                    }
                };
                let rows = tx
                    .query(statement, &[Cell::uuid(workspace), Cell::uuid(resource)])
                    .await?;
                rows.first()
                    .ok_or(sqlx::Error::RowNotFound)?
                    .cell(0)?
                    .value()
            }
        }
    }

    // The same empty-only seed decision applies to both families; the PG
    // advisory lock/recheck stays in its original position. Family callers
    // already own BEGIN IMMEDIATE, so no second initialization owner can enter.
    async fn ensure_collab_state(
        &mut self,
        t: &CollabTables,
        workspace: Uuid,
        resource: Uuid,
        content: &Value,
    ) -> Result<Result<(), CollabDbError>, sqlx::Error> {
        if self
            .native_state_generation(t, workspace, resource)
            .await?
            .is_some()
        {
            return Ok(Ok(()));
        }
        match self {
            Self::Postgres(tx) => lock_collab_init(tx, resource).await?,
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_tenant(workspace)?;
            }
        }
        if self
            .native_state_generation(t, workspace, resource)
            .await?
            .is_some()
        {
            return Ok(Ok(()));
        }
        if content != &empty_document_json() {
            return Ok(Err(CollabDbError::NotFound));
        }
        self.insert_empty_native_state(t, workspace, resource)
            .await?;
        Ok(Ok(()))
    }

    async fn native_state_generation(
        &mut self,
        t: &CollabTables,
        workspace: Uuid,
        resource: Uuid,
    ) -> Result<Option<i64>, sqlx::Error> {
        match self {
            Self::Postgres(tx) => {
                let row: Option<(i64,)> = sqlx::query_as(&t.sql("SELECT writer_generation FROM {states} WHERE workspace_id = $1 AND {id} = $2 FOR UPDATE"))
                    .bind(workspace).bind(resource).fetch_optional(&mut ***tx).await?;
                Ok(row.map(|(generation,)| generation))
            }
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_tenant(workspace)?;
                let statement = match t.kind {
                    CollabKind::Document => "SELECT writer_generation FROM document_states WHERE workspace_id=?1 AND document_id=?2",
                    CollabKind::Task => "SELECT writer_generation FROM task_states WHERE workspace_id=?1 AND task_id=?2",
                };
                tx.query(statement, &[Cell::uuid(workspace), Cell::uuid(resource)])
                    .await?
                    .first()
                    .map(|row| row.cell(0)?.integer())
                    .transpose()
            }
        }
    }

    async fn insert_empty_native_state(
        &mut self,
        t: &CollabTables,
        workspace: Uuid,
        resource: Uuid,
    ) -> Result<(), sqlx::Error> {
        match self {
            Self::Postgres(tx) => {
                sqlx::query(&t.sql("INSERT INTO {states} (workspace_id, {id}, state, encoding, writer_generation, snapshot_cutoff_seq, tail_seq) VALUES ($1, $2, $3, $4, 0, 0, 0)"))
                    .bind(workspace).bind(resource).bind(EMPTY_YJS_STATE_V1).bind(COLLAB_STATE_ENCODING_V1)
                    .execute(&mut ***tx).await?;
            }
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_tenant(workspace)?;
                let statement = match t.kind {
                    CollabKind::Document => "INSERT INTO document_states(workspace_id,document_id,state,encoding,writer_generation,snapshot_cutoff_seq,tail_seq) VALUES(?1,?2,?3,?4,0,0,0)",
                    CollabKind::Task => "INSERT INTO task_states(workspace_id,task_id,state,encoding,writer_generation,snapshot_cutoff_seq,tail_seq) VALUES(?1,?2,?3,?4,0,0,0)",
                };
                tx.execute(
                    statement,
                    &[
                        Cell::uuid(workspace),
                        Cell::uuid(resource),
                        Cell::Blob(EMPTY_YJS_STATE_V1.to_vec()),
                        Cell::Integer(i64::from(COLLAB_STATE_ENCODING_V1)),
                    ],
                )
                .await?;
            }
        }
        Ok(())
    }
}

async fn fetch_state_fence_for_update(
    tx: &mut Transaction<'_, Postgres>,
    t: &CollabTables,
    workspace_id: Uuid,
    document_id: Uuid,
) -> Result<Option<(i64, i64)>, sqlx::Error> {
    sqlx::query_as(&t.sql(
        r#"
        SELECT writer_generation, tail_seq
        FROM {states}
        WHERE workspace_id = $1 AND {id} = $2
        FOR UPDATE
        "#,
    ))
    .bind(workspace_id)
    .bind(document_id)
    .fetch_optional(&mut **tx)
    .await
}

/// The fields an append checks, from the locked state row: the snapshot's
/// stored size (`octet_length` reads the length without detoasting, so the
/// snapshot bytes never leave the server), `writer_generation`,
/// `snapshot_cutoff_seq` and `tail_seq`.
async fn fetch_append_fence_for_update(
    tx: &mut Transaction<'_, Postgres>,
    t: &CollabTables,
    workspace_id: Uuid,
    document_id: Uuid,
) -> Result<Option<(i64, i64, i64, i64)>, sqlx::Error> {
    sqlx::query_as(&t.sql(
        r#"
        SELECT octet_length(state)::bigint, writer_generation, snapshot_cutoff_seq, tail_seq
        FROM {states}
        WHERE workspace_id = $1 AND {id} = $2
        FOR UPDATE
        "#,
    ))
    .bind(workspace_id)
    .bind(document_id)
    .fetch_optional(&mut **tx)
    .await
}

pub(crate) async fn fetch_state_for_update(
    tx: &mut Transaction<'_, Postgres>,
    t: &CollabTables,
    workspace: Uuid,
    resource: Uuid,
) -> Result<Option<StateRow>, sqlx::Error> {
    OperationTx::Postgres(tx)
        .fetch_native_state(t, workspace, resource)
        .await
}

pub(crate) async fn load_tail_updates(
    tx: &mut Transaction<'_, Postgres>,
    t: &CollabTables,
    workspace: Uuid,
    resource: Uuid,
    snapshot_cutoff_seq: i64,
    snapshot_len: i64,
) -> Result<Result<Vec<CollabUpdateRow>, CollabDbError>, sqlx::Error> {
    OperationTx::Postgres(tx)
        .load_native_tail(t, workspace, resource, snapshot_cutoff_seq, snapshot_len)
        .await
}

impl OperationTx<'_, '_> {
    async fn native_append_fence(
        &mut self,
        t: &CollabTables,
        workspace: Uuid,
        resource: Uuid,
    ) -> Result<Option<(i64, i64, i64, i64)>, sqlx::Error> {
        match self {
            Self::Postgres(tx) => fetch_append_fence_for_update(tx, t, workspace, resource).await,
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_tenant(workspace)?;
                let statement = match t.kind {
                    CollabKind::Document => "SELECT length(state),writer_generation,snapshot_cutoff_seq,tail_seq FROM document_states WHERE workspace_id=?1 AND document_id=?2",
                    CollabKind::Task => "SELECT length(state),writer_generation,snapshot_cutoff_seq,tail_seq FROM task_states WHERE workspace_id=?1 AND task_id=?2",
                };
                tx.query(statement, &[Cell::uuid(workspace), Cell::uuid(resource)])
                    .await?
                    .first()
                    .map(|r| {
                        Ok((
                            r.cell(0)?.integer()?,
                            r.cell(1)?.integer()?,
                            r.cell(2)?.integer()?,
                            r.cell(3)?.integer()?,
                        ))
                    })
                    .transpose()
            }
        }
    }

    pub(crate) async fn native_operation_receipt(
        &mut self,
        kind: CollabKind,
        workspace: Uuid,
        resource: Uuid,
        operation: Uuid,
    ) -> Result<Option<CollabOperationLookup>, sqlx::Error> {
        let t = CollabTables::for_kind(kind);
        let row: Option<(i64,i64,Vec<u8>,Uuid)> = match self {
            Self::Postgres(tx) => sqlx::query_as(&t.sql("SELECT seq,payload_len,payload_sha256,actor_user_id FROM {receipts} WHERE workspace_id=$1 AND {id}=$2 AND op_id=$3"))
                .bind(workspace).bind(resource).bind(operation).fetch_optional(&mut ***tx).await?,
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_tenant(workspace)?;
                let statement = match kind {
                    CollabKind::Document => "SELECT seq,payload_len,payload_sha256,actor_user_id FROM document_collab_op_receipts WHERE workspace_id=?1 AND document_id=?2 AND op_id=?3",
                    CollabKind::Task => "SELECT seq,payload_len,payload_sha256,actor_user_id FROM task_collab_op_receipts WHERE workspace_id=?1 AND task_id=?2 AND op_id=?3",
                };
                tx.query(statement, &[Cell::uuid(workspace),Cell::uuid(resource),Cell::uuid(operation)]).await?
                    .first().map(|r| Ok::<_,sqlx::Error>((r.cell(0)?.integer()?,r.cell(1)?.integer()?,r.cell(2)?.bytes()?,r.cell(3)?.id()?))).transpose()?
            }
        };
        Ok(row.map(
            |(seq, payload_len, payload_sha256, actor_user_id)| CollabOperationLookup {
                seq,
                payload_len,
                payload_sha256,
                actor_user_id,
            },
        ))
    }

    async fn advance_native_tail(
        &mut self,
        t: &CollabTables,
        workspace: Uuid,
        resource: Uuid,
        generation: i64,
        expected_tail: i64,
    ) -> Result<Option<i64>, sqlx::Error> {
        match self {
            Self::Postgres(tx) => {
                let row: Option<(i64,)> = sqlx::query_as(&t.sql("UPDATE {states} SET tail_seq=tail_seq+1,updated_at=now() WHERE workspace_id=$1 AND {id}=$2 AND writer_generation=$3 AND tail_seq=$4 RETURNING tail_seq"))
                    .bind(workspace).bind(resource).bind(generation).bind(expected_tail).fetch_optional(&mut ***tx).await?;
                Ok(row.map(|(seq,)| seq))
            }
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_tenant(workspace)?;
                let statement = match t.kind {
                    CollabKind::Document => "UPDATE document_states SET tail_seq=tail_seq+1,updated_at=unixepoch()*1000000+CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000 WHERE workspace_id=?1 AND document_id=?2 AND writer_generation=?3 AND tail_seq=?4 RETURNING tail_seq",
                    CollabKind::Task => "UPDATE task_states SET tail_seq=tail_seq+1,updated_at=unixepoch()*1000000+CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000 WHERE workspace_id=?1 AND task_id=?2 AND writer_generation=?3 AND tail_seq=?4 RETURNING tail_seq",
                };
                tx.query(
                    statement,
                    &[
                        Cell::uuid(workspace),
                        Cell::uuid(resource),
                        Cell::Integer(generation),
                        Cell::Integer(expected_tail),
                    ],
                )
                .await?
                .first()
                .map(|r| r.cell(0)?.integer())
                .transpose()
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn insert_native_update_and_receipt(
        &mut self,
        t: &CollabTables,
        workspace: Uuid,
        resource: Uuid,
        actor: Uuid,
        operation: Uuid,
        seq: i64,
        payload: &[u8],
        digest: &[u8],
    ) -> Result<(), sqlx::Error> {
        match self {
            Self::Postgres(tx) => {
                sqlx::query(&t.sql("INSERT INTO {updates}(workspace_id,{id},seq,op_id,payload) VALUES($1,$2,$3,$4,$5)"))
                    .bind(workspace).bind(resource).bind(seq).bind(operation).bind(payload).execute(&mut ***tx).await?;
                sqlx::query(&t.sql("INSERT INTO {receipts}(workspace_id,{id},op_id,seq,payload_len,payload_sha256,actor_user_id) VALUES($1,$2,$3,$4,$5,$6,$7)"))
                    .bind(workspace).bind(resource).bind(operation).bind(seq).bind(payload.len() as i64).bind(digest).bind(actor).execute(&mut ***tx).await?;
            }
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_tenant(workspace)?;
                let (update,receipt) = match t.kind {
                    CollabKind::Document => (
                        "INSERT INTO document_collab_updates(workspace_id,document_id,seq,op_id,payload) VALUES(?1,?2,?3,?4,?5)",
                        "INSERT INTO document_collab_op_receipts(workspace_id,document_id,op_id,seq,payload_len,payload_sha256,actor_user_id) VALUES(?1,?2,?3,?4,?5,?6,?7)",
                    ),
                    CollabKind::Task => (
                        "INSERT INTO task_collab_updates(workspace_id,task_id,seq,op_id,payload) VALUES(?1,?2,?3,?4,?5)",
                        "INSERT INTO task_collab_op_receipts(workspace_id,task_id,op_id,seq,payload_len,payload_sha256,actor_user_id) VALUES(?1,?2,?3,?4,?5,?6,?7)",
                    ),
                };
                tx.execute(
                    update,
                    &[
                        Cell::uuid(workspace),
                        Cell::uuid(resource),
                        Cell::Integer(seq),
                        Cell::uuid(operation),
                        Cell::Blob(payload.to_vec()),
                    ],
                )
                .await?;
                tx.execute(
                    receipt,
                    &[
                        Cell::uuid(workspace),
                        Cell::uuid(resource),
                        Cell::uuid(operation),
                        Cell::Integer(seq),
                        Cell::Integer(payload.len() as i64),
                        Cell::Blob(digest.to_vec()),
                        Cell::uuid(actor),
                    ],
                )
                .await?;
            }
        }
        Ok(())
    }

    async fn record_native_append(
        &mut self,
        t: &CollabTables,
        record: CollabAuditRecord<'_>,
    ) -> Result<(), sqlx::Error> {
        let CollabAuditRecord {
            workspace_id,
            actor_user_id,
            document_id,
            op_id,
            seq,
            writer_generation,
            client_ip,
        } = record;
        let payload = json!({t.payload_key:document_id.to_string(),"opId":op_id.to_string(),"seq":seq,"writerGeneration":writer_generation});
        self.append_event(EventAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(workspace_id),
            actor_user_id: Some(actor_user_id),
            verb: t.verb("collab_update_appended"),
            target_type: Some(t.target_type.to_string()),
            target_id: Some(document_id),
            payload: payload.clone(),
        })
        .await?;
        self.append_audit(AuditAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(workspace_id),
            actor_user_id: Some(actor_user_id),
            verb: t.verb("collab_update_appended"),
            target_type: Some(t.target_type.to_string()),
            target_id: Some(document_id),
            payload,
            ip: client_ip.map(str::to_string),
        })
        .await
    }

    async fn bump_native_writer_generation(
        &mut self,
        t: &CollabTables,
        workspace: Uuid,
        resource: Uuid,
    ) -> Result<Option<i64>, sqlx::Error> {
        match self {
            Self::Postgres(tx) => {
                let row: Option<(i64,)> = sqlx::query_as(&t.sql("UPDATE {states} SET writer_generation = writer_generation + 1, updated_at = now() WHERE workspace_id = $1 AND {id} = $2 RETURNING writer_generation"))
                    .bind(workspace).bind(resource).fetch_optional(&mut ***tx).await?;
                Ok(row.map(|(generation,)| generation))
            }
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_tenant(workspace)?;
                let statement = match t.kind {
                    CollabKind::Document => "UPDATE document_states SET writer_generation=writer_generation+1,updated_at=unixepoch()*1000000+CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000 WHERE workspace_id=?1 AND document_id=?2 RETURNING writer_generation",
                    CollabKind::Task => "UPDATE task_states SET writer_generation=writer_generation+1,updated_at=unixepoch()*1000000+CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000 WHERE workspace_id=?1 AND task_id=?2 RETURNING writer_generation",
                };
                tx.query(statement, &[Cell::uuid(workspace), Cell::uuid(resource)])
                    .await?
                    .first()
                    .map(|row| row.cell(0)?.integer())
                    .transpose()
            }
        }
    }

    async fn fetch_native_state(
        &mut self,
        t: &CollabTables,
        workspace: Uuid,
        resource: Uuid,
    ) -> Result<Option<StateRow>, sqlx::Error> {
        match self {
            Self::Postgres(tx) => sqlx::query_as(&t.sql("SELECT state, encoding, writer_generation, snapshot_cutoff_seq, tail_seq, updated_at FROM {states} WHERE workspace_id = $1 AND {id} = $2 FOR UPDATE"))
                .bind(workspace).bind(resource).fetch_optional(&mut ***tx).await,
            Self::SqliteFamily(tx) => {
                tx.require_writer()?; tx.require_tenant(workspace)?;
                let statement = match t.kind {
                    CollabKind::Document => "SELECT state,encoding,writer_generation,snapshot_cutoff_seq,tail_seq,updated_at FROM document_states WHERE workspace_id=?1 AND document_id=?2",
                    CollabKind::Task => "SELECT state,encoding,writer_generation,snapshot_cutoff_seq,tail_seq,updated_at FROM task_states WHERE workspace_id=?1 AND task_id=?2",
                };
                tx.query(statement, &[Cell::uuid(workspace), Cell::uuid(resource)]).await?.first().map(|row| {
                    Ok((row.cell(0)?.bytes()?, i16::try_from(row.cell(1)?.integer()?).map_err(|error| sqlx::Error::Decode(Box::new(error)))?,
                        row.cell(2)?.integer()?, row.cell(3)?.integer()?, row.cell(4)?.integer()?, row.cell(5)?.datetime()?))
                }).transpose()
            }
        }
    }

    async fn native_tail_stats(
        &mut self,
        t: &CollabTables,
        workspace: Uuid,
        resource: Uuid,
        cutoff: i64,
    ) -> Result<(i64, i64), sqlx::Error> {
        match self {
            Self::Postgres(tx) => sqlx::query_as(&t.sql("SELECT count(*)::bigint, coalesce(sum(octet_length(payload)), 0)::bigint FROM {updates} WHERE workspace_id = $1 AND {id} = $2 AND seq > $3"))
                .bind(workspace).bind(resource).bind(cutoff).fetch_one(&mut ***tx).await,
            Self::SqliteFamily(tx) => {
                tx.require_writer()?; tx.require_tenant(workspace)?;
                let statement = match t.kind {
                    CollabKind::Document => "SELECT count(*),coalesce(sum(length(payload)),0) FROM document_collab_updates WHERE workspace_id=?1 AND document_id=?2 AND seq>?3",
                    CollabKind::Task => "SELECT count(*),coalesce(sum(length(payload)),0) FROM task_collab_updates WHERE workspace_id=?1 AND task_id=?2 AND seq>?3",
                };
                let rows = tx.query(statement, &[Cell::uuid(workspace), Cell::uuid(resource), Cell::Integer(cutoff)]).await?;
                let row = rows.first().ok_or(sqlx::Error::RowNotFound)?;
                Ok((row.cell(0)?.integer()?, row.cell(1)?.integer()?))
            }
        }
    }

    async fn load_native_tail(
        &mut self,
        t: &CollabTables,
        workspace: Uuid,
        resource: Uuid,
        cutoff: i64,
        snapshot_len: i64,
    ) -> Result<Result<Vec<CollabUpdateRow>, CollabDbError>, sqlx::Error> {
        let (count, bytes) = self
            .native_tail_stats(t, workspace, resource, cutoff)
            .await?;
        if let Err(error) = load_budget_allows(snapshot_len, count, bytes) {
            return Ok(Err(error));
        }
        let rows: Vec<(i64, Uuid, Vec<u8>)> = match self {
            Self::Postgres(tx) => sqlx::query_as(&t.sql("SELECT seq, op_id, payload FROM {updates} WHERE workspace_id = $1 AND {id} = $2 AND seq > $3 ORDER BY seq ASC LIMIT $4"))
                .bind(workspace).bind(resource).bind(cutoff).bind(MAX_COLLAB_TAIL_UPDATES).fetch_all(&mut ***tx).await?,
            Self::SqliteFamily(tx) => {
                tx.require_writer()?; tx.require_tenant(workspace)?;
                let statement = match t.kind {
                    CollabKind::Document => "SELECT seq,op_id,payload FROM document_collab_updates WHERE workspace_id=?1 AND document_id=?2 AND seq>?3 ORDER BY seq ASC LIMIT ?4",
                    CollabKind::Task => "SELECT seq,op_id,payload FROM task_collab_updates WHERE workspace_id=?1 AND task_id=?2 AND seq>?3 ORDER BY seq ASC LIMIT ?4",
                };
                tx.query(statement, &[Cell::uuid(workspace), Cell::uuid(resource), Cell::Integer(cutoff), Cell::Integer(MAX_COLLAB_TAIL_UPDATES)]).await?
                    .iter().map(|row| Ok((row.cell(0)?.integer()?, row.cell(1)?.id()?, row.cell(2)?.bytes()?))).collect::<Result<_,sqlx::Error>>()?
            }
        };
        Ok(Ok(rows
            .into_iter()
            .map(|(seq, op_id, payload)| CollabUpdateRow {
                seq,
                op_id,
                payload,
            })
            .collect()))
    }
}

fn state_row_to_load(row: StateRow, tail: Vec<CollabUpdateRow>) -> CollabLoadState {
    let (snapshot, _encoding, writer_generation, snapshot_cutoff_seq, tail_seq, _updated_at) = row;
    CollabLoadState {
        snapshot,
        tail,
        writer_generation,
        snapshot_cutoff_seq,
        tail_seq,
    }
}

/// What the actor prefix found, before a writer or reader maps it to an error.
enum ActorAccess {
    /// The session or its user is no longer live.
    SessionInactive,
    WorkspaceGone,
    NotMember,
    /// Missing, trashed, in a trashed project, or moved between the unlocked
    /// read and the row lock (see [`OperationTx::lock_collab_access`]).
    ResourceGone,
    Access(CollabDocumentAccess),
}

/// The actor prefix of every authorizing collab transaction, in the lock
/// order of the module doc: membership advisory lock, session recheck, live
/// workspace, membership row, then the resource rows. Runs after `set_tenant`.
/// Records `advisory_lock_us` for the advisory lock and `row_lock_us` for the
/// rest. Callers map the result with [`authorize_collab_write`] or
/// [`authorize_collab_read`].
impl OperationTx<'_, '_> {
    async fn lock_collab_actor(
        &mut self,
        kind: CollabKind,
        workspace_id: Uuid,
        actor_user_id: Uuid,
        session_id: Uuid,
        resource_id: Uuid,
        timings: &mut CollabDbStageTimings,
    ) -> Result<ActorAccess, sqlx::Error> {
        let advisory_started = Instant::now();
        self.lock_membership_users(&[actor_user_id]).await?;
        timings.advisory_lock_us = advisory_started.elapsed().as_micros() as u64;
        let row_started = Instant::now();
        let access = async {
            if !self.recheck_session(actor_user_id, session_id).await? {
                return Ok(ActorAccess::SessionInactive);
            }
            if !self.workspace_is_live(workspace_id).await? {
                return Ok(ActorAccess::WorkspaceGone);
            }
            if self
                .membership_role(workspace_id, actor_user_id, true)
                .await?
                .is_none()
            {
                return Ok(ActorAccess::NotMember);
            }
            let access = self
                .lock_collab_access(kind, workspace_id, actor_user_id, resource_id)
                .await?;
            Ok::<_, sqlx::Error>(access.map_or(ActorAccess::ResourceGone, ActorAccess::Access))
        }
        .await;
        timings.row_lock_us = row_started.elapsed().as_micros() as u64;
        access
    }
}

/// Writer check (claim, load, append, compaction, derived-body write): the
/// actor prefix, then Edit on an unarchived resource. A dead session or a
/// non-member is `Forbidden`. Existence (tenant, trash, affiliation) decides
/// `NotFound` before permission decides `Forbidden`.
async fn authorize_collab_write(
    tx: &mut Transaction<'_, Postgres>,
    kind: CollabKind,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
    timings: &mut CollabDbStageTimings,
) -> Result<Result<(), CollabDbError>, sqlx::Error> {
    OperationTx::Postgres(tx)
        .authorize_collab_write(
            kind,
            workspace_id,
            actor_user_id,
            session_id,
            document_id,
            timings,
        )
        .await
}
impl OperationTx<'_, '_> {
    pub(crate) async fn authorize_collab_write(
        &mut self,
        kind: CollabKind,
        workspace_id: Uuid,
        actor_user_id: Uuid,
        session_id: Uuid,
        document_id: Uuid,
        timings: &mut CollabDbStageTimings,
    ) -> Result<Result<(), CollabDbError>, sqlx::Error> {
        let access = self
            .lock_collab_actor(
                kind,
                workspace_id,
                actor_user_id,
                session_id,
                document_id,
                timings,
            )
            .await?;
        Ok(match access {
            ActorAccess::SessionInactive | ActorAccess::NotMember => Err(CollabDbError::Forbidden),
            ActorAccess::WorkspaceGone | ActorAccess::ResourceGone => Err(CollabDbError::NotFound),
            ActorAccess::Access(access)
                if access.permission.at_least(ProjectPermission::Edit) && !access.archived =>
            {
                Ok(())
            }
            ActorAccess::Access(_) => Err(CollabDbError::Forbidden),
        })
    }
}

/// Reader check (admission, receipt lookup and verify, read-only load): the
/// actor prefix, then at least View. Only a dead session is `Forbidden`;
/// every other refusal, including a permission below View, is `NotFound`.
async fn authorize_collab_read(
    tx: &mut Transaction<'_, Postgres>,
    kind: CollabKind,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
    timings: &mut CollabDbStageTimings,
) -> Result<Result<CollabDocumentAccess, CollabDbError>, sqlx::Error> {
    OperationTx::Postgres(tx)
        .authorize_collab_read(
            kind,
            workspace_id,
            actor_user_id,
            session_id,
            document_id,
            timings,
        )
        .await
}
impl OperationTx<'_, '_> {
    pub(crate) async fn authorize_collab_read(
        &mut self,
        kind: CollabKind,
        workspace_id: Uuid,
        actor_user_id: Uuid,
        session_id: Uuid,
        document_id: Uuid,
        timings: &mut CollabDbStageTimings,
    ) -> Result<Result<CollabDocumentAccess, CollabDbError>, sqlx::Error> {
        let access = self
            .lock_collab_actor(
                kind,
                workspace_id,
                actor_user_id,
                session_id,
                document_id,
                timings,
            )
            .await?;
        Ok(match access {
            ActorAccess::SessionInactive => Err(CollabDbError::Forbidden),
            ActorAccess::WorkspaceGone | ActorAccess::NotMember | ActorAccess::ResourceGone => {
                Err(CollabDbError::NotFound)
            }
            ActorAccess::Access(access) if access.permission.at_least(ProjectPermission::View) => {
                Ok(access)
            }
            ActorAccess::Access(_) => Err(CollabDbError::NotFound),
        })
    }
}

/// System `document.updated` / `task.updated` event (`collab: true`) after a
/// derived body projection changed the resource row.
async fn append_system_updated_event(
    tx: &mut Transaction<'_, Postgres>,
    t: &CollabTables,
    workspace_id: Uuid,
    document_id: Uuid,
) -> Result<(), sqlx::Error> {
    let payload = json!({
        t.payload_key: document_id.to_string(),
        "collab": true,
    });
    sqlx::query(
        r#"
        INSERT INTO fvoci.events (
            id, workspace_id, actor_user_id, verb, target_type, target_id, payload, channel
        ) VALUES ($1, $2, NULL, $5, $6, $3, $4, 'system')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(workspace_id)
    .bind(document_id)
    .bind(payload)
    .bind(t.verb("updated"))
    .bind(t.target_type)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn record_collab_event_and_audit(
    tx: &mut Transaction<'_, Postgres>,
    t: &CollabTables,
    record: CollabAuditRecord<'_>,
) -> Result<(), sqlx::Error> {
    OperationTx::Postgres(tx)
        .record_native_append(t, record)
        .await
}

/// Test fixtures only (`db-tests`): the unchanged per-append event and audit
/// write of [`append_collab_update_kind`], for a fixture that writes a batch
/// of genuine appends in one transaction. No policy is copied here.
#[cfg(feature = "db-tests")]
#[allow(clippy::too_many_arguments)]
pub async fn record_collab_append_for_tests(
    tx: &mut Transaction<'_, Postgres>,
    kind: CollabKind,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    document_id: Uuid,
    op_id: Uuid,
    seq: i64,
    writer_generation: i64,
) -> Result<(), sqlx::Error> {
    record_collab_event_and_audit(
        tx,
        CollabTables::for_kind(kind),
        CollabAuditRecord {
            workspace_id,
            actor_user_id,
            document_id,
            op_id,
            seq,
            writer_generation,
            client_ip: None,
        },
    )
    .await
}

/// Native load policy is shared by PG and the SQLite family; driver methods
/// only perform the named reads and writes on this same reserved transaction.
#[derive(Clone, Copy)]
enum NativeLoadMode {
    ClaimWriter,
    Writer,
    Reader,
}

impl OperationTx<'_, '_> {
    #[allow(clippy::too_many_arguments)]
    async fn load_collab_native(
        &mut self,
        kind: CollabKind,
        workspace: Uuid,
        actor: Uuid,
        credential: Uuid,
        resource: Uuid,
        mode: NativeLoadMode,
    ) -> Result<Result<ClaimWriterResult, CollabDbError>, sqlx::Error> {
        self.set_tenant(workspace).await?;
        let mut timings = CollabDbStageTimings::default();
        let authorized = match mode {
            NativeLoadMode::Reader => self
                .authorize_collab_read(kind, workspace, actor, credential, resource, &mut timings)
                .await?
                .map(|_| ()),
            NativeLoadMode::ClaimWriter | NativeLoadMode::Writer => {
                self.authorize_collab_write(
                    kind,
                    workspace,
                    actor,
                    credential,
                    resource,
                    &mut timings,
                )
                .await?
            }
        };
        if let Err(error) = authorized {
            return Ok(Err(error));
        }
        let tables = CollabTables::for_kind(kind);
        let content = self
            .load_collab_resource_content(tables, workspace, resource)
            .await?;
        if let Err(error) = self
            .ensure_collab_state(tables, workspace, resource, &content)
            .await?
        {
            return Ok(Err(error));
        }
        if matches!(mode, NativeLoadMode::ClaimWriter)
            && self
                .bump_native_writer_generation(tables, workspace, resource)
                .await?
                .is_none()
        {
            return Ok(Err(CollabDbError::NotFound));
        }
        let Some(state) = self.fetch_native_state(tables, workspace, resource).await? else {
            return Ok(Err(CollabDbError::NotFound));
        };
        if state.1 != COLLAB_STATE_ENCODING_V1 {
            return Ok(Err(CollabDbError::NotFound));
        }
        let tail = match self
            .load_native_tail(tables, workspace, resource, state.3, state.0.len() as i64)
            .await?
        {
            Ok(tail) => tail,
            Err(error) => return Ok(Err(error)),
        };
        let load = state_row_to_load(state, tail);
        Ok(Ok(ClaimWriterResult {
            writer_generation: load.writer_generation,
            load,
        }))
    }
}

#[allow(clippy::too_many_arguments)]
async fn load_collab_native_backend(
    backend: &Backend,
    kind: CollabKind,
    workspace: Uuid,
    actor: Uuid,
    credential: Uuid,
    resource: Uuid,
    mode: NativeLoadMode,
) -> Result<Result<ClaimWriterResult, CollabDbError>, sqlx::Error> {
    // Read-only native sync can seed an empty state, and the current authority
    // prefix takes writer locks on PG. Both retain that existing behavior.
    let mut tx = backend.begin_write().await?;
    let result = tx
        .operation()
        .load_collab_native(kind, workspace, actor, credential, resource, mode)
        .await?;
    if result.is_err() {
        tx.rollback().await?;
    } else {
        tx.commit().await.map_err(|error| error.source)?;
    }
    Ok(result)
}

pub async fn claim_writer_and_load_kind_backend(
    backend: &Backend,
    kind: CollabKind,
    workspace: Uuid,
    actor: Uuid,
    credential: Uuid,
    resource: Uuid,
) -> Result<Result<ClaimWriterResult, CollabDbError>, sqlx::Error> {
    if !matches!(backend, Backend::Postgres(_)) {
        return Err(sqlx::Error::Protocol(
            "SQLite-family writer claim requires claim_family_document_room ownership".into(),
        ));
    }
    load_collab_native_backend(
        backend,
        kind,
        workspace,
        actor,
        credential,
        resource,
        NativeLoadMode::ClaimWriter,
    )
    .await
}

pub async fn load_collab_readonly_kind_backend(
    backend: &Backend,
    kind: CollabKind,
    workspace: Uuid,
    actor: Uuid,
    credential: Uuid,
    resource: Uuid,
) -> Result<Result<CollabLoadState, CollabDbError>, sqlx::Error> {
    let mut tx = backend.begin_write().await?;
    let result = tx
        .operation()
        .load_collab_native_readonly(kind, workspace, actor, credential, resource)
        .await?;
    if result.is_err() {
        tx.rollback().await?;
    } else {
        tx.commit().await.map_err(|error| error.source)?;
    }
    Ok(result)
}

pub async fn claim_writer_and_load(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
) -> Result<Result<ClaimWriterResult, CollabDbError>, sqlx::Error> {
    claim_writer_and_load_kind(
        pool,
        CollabKind::Document,
        workspace_id,
        actor_user_id,
        session_id,
        document_id,
    )
    .await
}

pub async fn claim_writer_and_load_kind(
    pool: &PgPool,
    kind: CollabKind,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
) -> Result<Result<ClaimWriterResult, CollabDbError>, sqlx::Error> {
    claim_writer_and_load_kind_backend(
        &Backend::Postgres(pool.clone()),
        kind,
        workspace_id,
        actor_user_id,
        session_id,
        document_id,
    )
    .await
}

pub async fn load_collab_document(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
) -> Result<Result<CollabLoadState, CollabDbError>, sqlx::Error> {
    load_collab_document_kind(
        pool,
        CollabKind::Document,
        workspace_id,
        actor_user_id,
        session_id,
        document_id,
    )
    .await
}

pub async fn load_collab_document_kind(
    pool: &PgPool,
    kind: CollabKind,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
) -> Result<Result<CollabLoadState, CollabDbError>, sqlx::Error> {
    load_collab_native_backend(
        &Backend::Postgres(pool.clone()),
        kind,
        workspace_id,
        actor_user_id,
        session_id,
        document_id,
        NativeLoadMode::Writer,
    )
    .await
    .map(|result| result.map(|claimed| claimed.load))
}

pub async fn append_collab_update(
    pool: &PgPool,
    input: AppendCollabInput<'_>,
) -> Result<Result<AppendCollabResult, CollabDbError>, sqlx::Error> {
    append_collab_update_kind(pool, CollabKind::Document, input).await
}

pub async fn append_collab_update_kind(
    pool: &PgPool,
    kind: CollabKind,
    input: AppendCollabInput<'_>,
) -> Result<Result<AppendCollabResult, CollabDbError>, sqlx::Error> {
    if input.payload.is_empty() || input.payload.len() > MAX_COLLAB_UPDATE_BYTES {
        return Ok(Err(CollabDbError::PayloadTooLarge));
    }
    let timings = CollabDbStageTimings::default();
    let tx = pool.begin().await?;
    append_collab_update_in_tx(
        DbTransaction::Postgres(tx),
        kind,
        input,
        timings,
        None,
        None,
    )
    .await
    .map(|(result, _)| result.map(|(append, _)| append))
}

/// A restore commits its new revision in the existing forward append tx.
pub async fn append_collab_restore_kind(
    pool: &PgPool,
    kind: CollabKind,
    input: AppendCollabInput<'_>,
    restore: &crate::db::revisions::RestoreRevisionAppend,
) -> Result<Result<(AppendCollabResult, Uuid), CollabDbError>, sqlx::Error> {
    if input.payload.is_empty() || input.payload.len() > MAX_COLLAB_UPDATE_BYTES {
        return Ok(Err(CollabDbError::PayloadTooLarge));
    }
    let tx = pool.begin().await?;
    append_collab_update_in_tx(
        DbTransaction::Postgres(tx),
        kind,
        input,
        CollabDbStageTimings::default(),
        Some(restore),
        None,
    )
    .await
    .map(|(result, _)| {
        result.and_then(|(append, revision_id)| {
            revision_id
                .map(|id| (append, id))
                .ok_or(CollabDbError::OpIdConflict)
        })
    })
}

/// Append on a room's dedicated session connection (no pool acquire).
pub async fn append_collab_update_on_conn_timed(
    conn: &mut PgConnection,
    kind: CollabKind,
    input: AppendCollabInput<'_>,
) -> Result<
    (
        Result<AppendCollabResult, CollabDbError>,
        CollabDbStageTimings,
    ),
    sqlx::Error,
> {
    let timings = CollabDbStageTimings::default();
    if input.payload.is_empty() || input.payload.len() > MAX_COLLAB_UPDATE_BYTES {
        return Ok((Err(CollabDbError::PayloadTooLarge), timings));
    }
    let tx = conn.begin().await?;
    append_collab_update_in_tx(
        DbTransaction::Postgres(tx),
        kind,
        input,
        timings,
        None,
        None,
    )
    .await
    .map(|(result, timings)| (result.map(|(append, _)| append), timings))
}

impl OperationTx<'_, '_> {
    /// Current native snapshot/head/tail, under the existing reader policy,
    /// without transferring transaction finish ownership to this operation.
    pub(crate) async fn load_collab_native_readonly(
        &mut self,
        kind: CollabKind,
        workspace: Uuid,
        actor: Uuid,
        credential: Uuid,
        resource: Uuid,
    ) -> Result<Result<CollabLoadState, CollabDbError>, sqlx::Error> {
        self.load_collab_native(
            kind,
            workspace,
            actor,
            credential,
            resource,
            NativeLoadMode::Reader,
        )
        .await
        .map(|result| result.map(|claimed| claimed.load))
    }

    async fn prepare_native_restore(
        &mut self,
        t: &CollabTables,
        workspace: Uuid,
        actor: Uuid,
        credential: Uuid,
        resource: Uuid,
        restore: &crate::db::revisions::RestoreRevisionAppend,
    ) -> Result<Result<Option<crate::db::revisions::RestoredRevision>, CollabDbError>, sqlx::Error>
    {
        use crate::db::revisions::{
            authorize_restore_in_tx, lookup_restored_revision_in_tx, RevisionDbError,
        };
        let Self::Postgres(tx) = self else {
            // The first slice adds ordinary ON appends; family revision restore
            // remains required and refuses before any native mutation.
            return Err(sqlx::Error::Protocol(
                "native revision restore is pending SQLite-family port".into(),
            ));
        };
        if let Err(error) =
            authorize_restore_in_tx(tx, workspace, actor, credential, restore.intent).await?
        {
            return Ok(Err(if error == RevisionDbError::RestoreConflict {
                CollabDbError::OpIdConflict
            } else {
                CollabDbError::Forbidden
            }));
        }
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(format!(
                "revision-restore:{workspace}:{}",
                restore.intent.correlation_id
            ))
            .execute(&mut ***tx)
            .await?;
        match lookup_restored_revision_in_tx(tx, workspace, actor, restore.intent).await? {
            Ok(Some(record)) => return Ok(Ok(Some(record))),
            Ok(None) => {}
            Err(_) => return Ok(Err(CollabDbError::OpIdConflict)),
        }
        let source: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM fvoci.revisions WHERE workspace_id=$1 AND id=$2 AND target_kind=$3 AND target_id=$4 FOR SHARE",
        ).bind(workspace).bind(restore.intent.source_revision_id).bind(t.target_type).bind(resource)
            .fetch_optional(&mut ***tx).await?;
        Ok(if source.is_some() {
            Ok(None)
        } else {
            Err(CollabDbError::NotFound)
        })
    }

    async fn apply_native_restore(
        &mut self,
        t: &CollabTables,
        workspace_id: Uuid,
        actor_user_id: Uuid,
        document_id: Uuid,
        seq: i64,
        restore: &crate::db::revisions::RestoreRevisionAppend,
    ) -> Result<(), sqlx::Error> {
        let Self::Postgres(tx) = self else {
            return Err(sqlx::Error::Protocol(
                "native revision restore is pending SQLite-family port".into(),
            ));
        };
        let tx = &mut **tx;
        sqlx::query(
            "INSERT INTO fvoci.revisions (id, workspace_id, target_kind, target_id, y_snapshot, encoding, content_json, text, reason, created_by, restored_from_id, restore_correlation_id, restore_base_tail_seq, restore_committed_tail_seq) VALUES ($1,$2,$3,$4,$5,1,$6,$7,'restore',$8,$9,$10,$11,$12)",
        )
        .bind(restore.revision_id).bind(workspace_id).bind(t.target_type).bind(document_id)
        .bind(&restore.y_snapshot).bind(restore.prepared_body.content_json()).bind(restore.prepared_body.text()).bind(actor_user_id)
        .bind(restore.intent.source_revision_id).bind(restore.intent.correlation_id)
        .bind(restore.intent.expected_tail_seq).bind(seq).execute(&mut **tx).await?;
        let mut payload = serde_json::json!({
            "restoreRequested": restore.intent.source_revision_id,
            "restoredFromRevisionId": restore.intent.source_revision_id,
            "restoredRevisionId": restore.revision_id,
            "correlationId": restore.intent.correlation_id,
        });
        payload[t.payload_key] = serde_json::json!(document_id);
        append_event(
            tx,
            EventAppend {
                id: Uuid::now_v7(),
                workspace_id: Some(workspace_id),
                actor_user_id: Some(actor_user_id),
                verb: t.verb("updated"),
                target_type: Some(t.target_type.into()),
                target_id: Some(document_id),
                payload,
            },
        )
        .await?;

        // This exact future canonical body belongs to the already-authorized
        // restore commit. Project it while the actor/resource/state locks are
        // held: post-commit revocation can deny recovery without leaving the
        // durable body API on the content that the committed restore replaced.
        // Correlation/receipt replays returned above and never project again.
        let updated: Option<(Uuid,)> = sqlx::query_as(&t.sql(
            r#"
            UPDATE {resource}
            SET content_json = $3,
                text = $4,
                chosung = $5,
                updated_at = now()
            WHERE workspace_id = $1
              AND id = $2
              AND content_json IS DISTINCT FROM $3::jsonb
            RETURNING id
            "#,
        ))
        .bind(workspace_id)
        .bind(document_id)
        .bind(restore.prepared_body.content_json())
        .bind(restore.prepared_body.text())
        .bind(restore.prepared_body.chosung())
        .fetch_optional(&mut **tx)
        .await?;
        if updated.is_some() {
            append_system_updated_event(tx, t, workspace_id, document_id).await?;
        }
        Ok(())
    }
}

/// Ordinary ON append through a family's real room owner. The same native
/// receipt, generation, tail budget, event and audit program serves PG too.
pub async fn append_family_document_room_update(
    backend: &Backend,
    fence: FamilyRoomFence,
    input: AppendCollabInput<'_>,
) -> Result<Result<AppendCollabResult, CollabDbError>, sqlx::Error> {
    if matches!(backend, Backend::Postgres(_)) {
        return Err(sqlx::Error::Protocol(
            "PostgreSQL room append requires its detached session connection".into(),
        ));
    }
    if input.payload.is_empty() || input.payload.len() > MAX_COLLAB_UPDATE_BYTES {
        return Ok(Err(CollabDbError::PayloadTooLarge));
    }
    let tx = backend.begin_write().await?;
    append_collab_update_in_tx(
        tx,
        CollabKind::Document,
        input,
        CollabDbStageTimings::default(),
        None,
        Some(fence),
    )
    .await
    .map(|(result, _)| result.map(|(appended, _)| appended))
}

impl OperationTx<'_, '_> {
    /// ON native append program on the caller's current transaction. The
    /// caller owns commit/rollback, allowing projection/revision/command effects
    /// to be composed without reimplementing receipts or native tail policy.
    pub(crate) async fn append_collab_native(
        &mut self,
        kind: CollabKind,
        input: AppendCollabInput<'_>,
        mut timings: CollabDbStageTimings,
        restore: Option<&crate::db::revisions::RestoreRevisionAppend>,
        room_fence: Option<FamilyRoomFence>,
    ) -> Result<
        (
            Result<(PreparedNativeAppend, Option<Uuid>), CollabDbError>,
            CollabDbStageTimings,
        ),
        sqlx::Error,
    > {
        if input.payload.is_empty() || input.payload.len() > MAX_COLLAB_UPDATE_BYTES {
            return Ok((Err(CollabDbError::PayloadTooLarge), timings));
        }
        let AppendCollabInput {
            workspace_id,
            actor_user_id,
            session_id,
            document_id,
            writer_generation,
            expected_tail_seq,
            op_id,
            payload,
            client_ip,
        } = input;
        let t = CollabTables::for_kind(kind);
        self.set_tenant(workspace_id).await?;
        if matches!(self, Self::SqliteFamily(_)) {
            let Some(fence) = room_fence else {
                return Ok((Err(CollabDbError::StaleWriter), timings));
            };
            if fence.workspace_id != workspace_id
                || fence.document_id != document_id
                || kind != CollabKind::Document
                || !self.verify_family_room_fence(fence).await?
            {
                return Ok((Err(CollabDbError::StaleWriter), timings));
            }
        }
        if let Err(err) = self
            .authorize_collab_write(
                kind,
                workspace_id,
                actor_user_id,
                session_id,
                document_id,
                &mut timings,
            )
            .await?
        {
            return Ok((Err(err), timings));
        }
        if let Some(restore) = restore {
            if restore.intent.scope.target().id() != document_id
                || restore.intent.scope.target().kind_str() != t.target_type
                || restore.intent.expected_tail_seq != expected_tail_seq
            {
                return Ok((Err(CollabDbError::OpIdConflict), timings));
            }
            match self
                .prepare_native_restore(
                    t,
                    workspace_id,
                    actor_user_id,
                    session_id,
                    document_id,
                    restore,
                )
                .await?
            {
                Ok(Some(record)) => {
                    return Ok((
                        Ok((
                            PreparedNativeAppend::Replay {
                                seq: record.committed_tail_seq,
                            },
                            Some(record.revision_id),
                        )),
                        timings,
                    ));
                }
                Ok(None) => {}
                Err(error) => {
                    return Ok((Err(error), timings));
                }
            }
        }
        let state_started = Instant::now();
        let mut fence = self
            .native_append_fence(t, workspace_id, document_id)
            .await?;
        if fence.is_none() {
            // Claim creates the state row, so only a row that never existed gets
            // here. Seed it the way claim does (NotFound unless the body is still
            // the empty seed); only this path reads content_json.
            let content = self
                .load_collab_resource_content(t, workspace_id, document_id)
                .await?;
            if let Err(err) = self
                .ensure_collab_state(t, workspace_id, document_id, &content)
                .await?
            {
                return Ok((Err(err), timings));
            }
            fence = self
                .native_append_fence(t, workspace_id, document_id)
                .await?;
        }
        timings.row_lock_us += state_started.elapsed().as_micros() as u64;
        let stmt_started = Instant::now();
        let Some((snapshot_len, current_generation, snapshot_cutoff_seq, tail_seq)) = fence else {
            return Ok((Err(CollabDbError::NotFound), timings));
        };
        if current_generation != writer_generation {
            return Ok((Err(CollabDbError::StaleWriter), timings));
        }

        let incoming_len = payload.len() as i64;
        let incoming_digest = payload_sha256(payload);
        let existing = self
            .native_operation_receipt(kind, workspace_id, document_id, op_id)
            .await?;
        if let Some(CollabOperationLookup {
            seq,
            payload_len: existing_len,
            payload_sha256: existing_digest,
            actor_user_id: existing_actor,
        }) = existing
        {
            if existing_len == incoming_len
                && existing_digest.as_slice() == incoming_digest.as_slice()
                && existing_actor == actor_user_id
            {
                if let Some(fence) = room_fence {
                    if !self.verify_family_room_fence(fence).await? {
                        return Ok((Err(CollabDbError::StaleWriter), timings));
                    }
                }
                timings.stmt_us = stmt_started.elapsed().as_micros() as u64;
                return Ok((
                    Ok((
                        PreparedNativeAppend::Replay { seq },
                        restore.map(|value| value.revision_id),
                    )),
                    timings,
                ));
            }
            return Ok((Err(CollabDbError::OpIdConflict), timings));
        }
        if tail_seq != expected_tail_seq {
            return Ok((Err(CollabDbError::StaleCutoff), timings));
        }

        let (count, bytes) = self
            .native_tail_stats(t, workspace_id, document_id, snapshot_cutoff_seq)
            .await?;
        if count + 1 > MAX_COLLAB_TAIL_UPDATES
            || snapshot_len + bytes + incoming_len > MAX_COLLAB_LOAD_BYTES
        {
            return Ok((Err(CollabDbError::StateBudgetExceeded), timings));
        }
        let next_seq = self
            .advance_native_tail(
                t,
                workspace_id,
                document_id,
                writer_generation,
                expected_tail_seq,
            )
            .await?;
        let Some(seq) = next_seq else {
            return Ok((Err(CollabDbError::StaleWriter), timings));
        };

        self.insert_native_update_and_receipt(
            t,
            workspace_id,
            document_id,
            actor_user_id,
            op_id,
            seq,
            payload,
            &incoming_digest,
        )
        .await?;

        self.record_native_append(
            t,
            CollabAuditRecord {
                workspace_id,
                actor_user_id,
                document_id,
                op_id,
                seq,
                writer_generation,
                client_ip,
            },
        )
        .await?;

        if let Some(restore) = restore {
            self.apply_native_restore(t, workspace_id, actor_user_id, document_id, seq, restore)
                .await?;
        }
        if let Some(fence) = room_fence {
            if !self.verify_family_room_fence(fence).await? {
                return Ok((Err(CollabDbError::StaleWriter), timings));
            }
        }
        timings.stmt_us = stmt_started.elapsed().as_micros() as u64;
        Ok((
            Ok((
                PreparedNativeAppend::Appended { seq },
                restore.map(|value| value.revision_id),
            )),
            timings,
        ))
    }
}

#[derive(Debug, thiserror::Error)]
#[error("native append failed and rollback was not confirmed: {rollback}")]
struct NativeRollbackUnconfirmed {
    #[source]
    original: sqlx::Error,
    rollback: sqlx::Error,
}

async fn append_collab_update_in_tx(
    mut tx: DbTransaction<'_>,
    kind: CollabKind,
    input: AppendCollabInput<'_>,
    timings: CollabDbStageTimings,
    restore: Option<&crate::db::revisions::RestoreRevisionAppend>,
    room_fence: Option<FamilyRoomFence>,
) -> Result<
    (
        Result<(AppendCollabResult, Option<Uuid>), CollabDbError>,
        CollabDbStageTimings,
    ),
    sqlx::Error,
> {
    #[cfg(feature = "db-tests")]
    let (workspace_id, document_id) = (input.workspace_id, input.document_id);
    let (result, mut timings) = match tx
        .operation()
        .append_collab_native(kind, input, timings, restore, room_fence)
        .await
    {
        Ok(result) => result,
        Err(original) => {
            if let Err(rollback) = tx.rollback().await {
                return Err(sqlx::Error::Decode(Box::new(NativeRollbackUnconfirmed {
                    original,
                    rollback,
                })));
            }
            return Err(original);
        }
    };
    let prepared_result = match result {
        Err(error) => {
            tx.rollback().await?;
            return Ok((Err(error), timings));
        }
        Ok(prepared) => prepared,
    };
    #[cfg(feature = "db-tests")]
    let committed_new_restore =
        restore.is_some() && matches!(prepared_result.0, PreparedNativeAppend::Appended { .. });
    let commit_started = Instant::now();
    tx.commit().await.map_err(|error| error.source)?;
    timings.commit_us = commit_started.elapsed().as_micros() as u64;
    #[cfg(feature = "db-tests")]
    if committed_new_restore {
        let barrier = RESTORE_COMMIT_AMBIGUITY
            .lock()
            .await
            .remove(&(workspace_id, document_id));
        if let Some((reached, proceed)) = barrier {
            let _ = reached.send(());
            let _ = proceed.await;
            return Err(sqlx::Error::Io(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                "fixture restore committed; reply lost",
            )));
        }
    }
    let (prepared, revision) = prepared_result;
    let seq = prepared.seq();
    let confirmed = match prepared {
        PreparedNativeAppend::Appended { .. } => AppendCollabResult::Committed { seq },
        PreparedNativeAppend::Replay { .. } => AppendCollabResult::DuplicateAck { seq },
    };
    Ok((Ok((confirmed, revision)), timings))
}

pub async fn lookup_collab_operation(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
    op_id: Uuid,
) -> Result<Result<Option<CollabOperationLookup>, CollabDbError>, sqlx::Error> {
    let kind = CollabKind::Document;
    let t = CollabTables::for_kind(kind);
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if let Err(err) = authorize_collab_read(
        &mut tx,
        kind,
        workspace_id,
        actor_user_id,
        session_id,
        document_id,
        &mut CollabDbStageTimings::default(),
    )
    .await?
    {
        tx.rollback().await?;
        return Ok(Err(err));
    }
    let row: Option<(i64, i64, Vec<u8>, Uuid)> = sqlx::query_as(&t.sql(
        r#"
        SELECT seq, payload_len, payload_sha256, actor_user_id
        FROM {receipts}
        WHERE workspace_id = $1 AND {id} = $2 AND op_id = $3
        "#,
    ))
    .bind(workspace_id)
    .bind(document_id)
    .bind(op_id)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(row.map(
        |(seq, payload_len, payload_sha256, actor_user_id)| CollabOperationLookup {
            seq,
            payload_len,
            payload_sha256,
            actor_user_id,
        },
    )))
}

pub async fn verify_collab_operation(
    pool: &PgPool,
    input: VerifyCollabInput<'_>,
) -> Result<Result<CollabOperationLookup, CollabDbError>, sqlx::Error> {
    verify_collab_operation_kind(pool, CollabKind::Document, input).await
}

pub async fn verify_collab_operation_kind(
    pool: &PgPool,
    kind: CollabKind,
    input: VerifyCollabInput<'_>,
) -> Result<Result<CollabOperationLookup, CollabDbError>, sqlx::Error> {
    let t = CollabTables::for_kind(kind);
    let VerifyCollabInput {
        workspace_id,
        actor_user_id,
        session_id,
        document_id,
        op_id,
        expected_payload_len,
        expected_payload_sha256,
        expected_actor_user_id,
    } = input;
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if let Err(err) = authorize_collab_read(
        &mut tx,
        kind,
        workspace_id,
        actor_user_id,
        session_id,
        document_id,
        &mut CollabDbStageTimings::default(),
    )
    .await?
    {
        tx.rollback().await?;
        return Ok(Err(err));
    }
    let row: Option<(i64, i64, Vec<u8>, Uuid)> = sqlx::query_as(&t.sql(
        r#"
        SELECT seq, payload_len, payload_sha256, actor_user_id
        FROM {receipts}
        WHERE workspace_id = $1 AND {id} = $2 AND op_id = $3
        "#,
    ))
    .bind(workspace_id)
    .bind(document_id)
    .bind(op_id)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    let Some((seq, payload_len, payload_sha256, stored_actor)) = row else {
        return Ok(Err(CollabDbError::NotFound));
    };
    if payload_len != expected_payload_len
        || payload_sha256.as_slice() != expected_payload_sha256
        || stored_actor != expected_actor_user_id
    {
        return Ok(Err(CollabDbError::OpIdConflict));
    }
    Ok(Ok(CollabOperationLookup {
        seq,
        payload_len,
        payload_sha256,
        actor_user_id: stored_actor,
    }))
}

pub async fn compact_collab_snapshot(
    pool: &PgPool,
    input: CompactCollabInput<'_>,
) -> Result<Result<CollabLoadState, CollabDbError>, sqlx::Error> {
    compact_collab_snapshot_kind(pool, CollabKind::Document, input).await
}

pub async fn compact_collab_snapshot_kind(
    pool: &PgPool,
    kind: CollabKind,
    input: CompactCollabInput<'_>,
) -> Result<Result<CollabLoadState, CollabDbError>, sqlx::Error> {
    let t = CollabTables::for_kind(kind);
    let CompactCollabInput {
        workspace_id,
        actor_user_id,
        session_id,
        document_id,
        writer_generation,
        cutoff_seq,
        expected_tail_seq,
        new_snapshot,
        client_ip,
    } = input;
    if new_snapshot.is_empty() || new_snapshot.len() > MAX_COLLAB_SNAPSHOT_BYTES {
        return Ok(Err(CollabDbError::PayloadTooLarge));
    }

    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if let Err(err) = authorize_collab_write(
        &mut tx,
        kind,
        workspace_id,
        actor_user_id,
        session_id,
        document_id,
        &mut CollabDbStageTimings::default(),
    )
    .await?
    {
        tx.rollback().await?;
        return Ok(Err(err));
    }
    let content = load_resource_content(&mut tx, t, workspace_id, document_id).await?;
    match ensure_collab_state_row(&mut tx, t, workspace_id, document_id, &content.0).await? {
        Ok(()) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    }
    let state = fetch_state_for_update(&mut tx, t, workspace_id, document_id).await?;
    let Some(state) = state else {
        tx.rollback().await?;
        return Ok(Err(CollabDbError::NotFound));
    };
    if state.2 != writer_generation {
        tx.rollback().await?;
        return Ok(Err(CollabDbError::StaleWriter));
    }
    if expected_tail_seq != state.4 {
        tx.rollback().await?;
        return Ok(Err(CollabDbError::StaleCutoff));
    }
    if cutoff_seq < state.3 || cutoff_seq > state.4 {
        tx.rollback().await?;
        return Ok(Err(CollabDbError::InvalidCutoff));
    }

    let newer: Option<(i64,)> = sqlx::query_as(&t.sql(
        r#"
        SELECT seq
        FROM {updates}
        WHERE workspace_id = $1 AND {id} = $2 AND seq > $3
        ORDER BY seq ASC
        LIMIT 1
        "#,
    ))
    .bind(workspace_id)
    .bind(document_id)
    .bind(cutoff_seq)
    .fetch_optional(&mut *tx)
    .await?;
    if cutoff_seq < state.4 && newer.is_some() {
        tx.rollback().await?;
        return Ok(Err(CollabDbError::InvalidCutoff));
    }

    // Saving the already committed snapshot again (same locked cutoff, no
    // compactable rows, byte-identical bytes) changes nothing. Skip the row
    // rewrite, tail delete, event and audit; every check above still applied.
    let compactable: bool = sqlx::query_scalar(&t.sql(
        r#"
        SELECT EXISTS(
            SELECT 1 FROM {updates}
            WHERE workspace_id = $1 AND {id} = $2 AND seq <= $3
        )
        "#,
    ))
    .bind(workspace_id)
    .bind(document_id)
    .bind(cutoff_seq)
    .fetch_one(&mut *tx)
    .await?;
    let unchanged = cutoff_seq == state.3 && !compactable && state.0.as_slice() == new_snapshot;
    if !unchanged {
        compact_rows(
            &mut tx,
            t,
            workspace_id,
            actor_user_id,
            document_id,
            writer_generation,
            cutoff_seq,
            new_snapshot,
            client_ip,
        )
        .await?;
    }

    let refreshed = fetch_state_for_update(&mut tx, t, workspace_id, document_id).await?;
    let Some(refreshed) = refreshed else {
        tx.rollback().await?;
        return Ok(Err(CollabDbError::NotFound));
    };
    let tail = match load_tail_updates(
        &mut tx,
        t,
        workspace_id,
        document_id,
        refreshed.3,
        refreshed.0.len() as i64,
    )
    .await?
    {
        Ok(tail) => tail,
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    };
    let load = state_row_to_load(refreshed, tail);
    tx.commit().await?;
    Ok(Ok(load))
}

/// The committing half of a compaction: new snapshot and cutoff, compacted
/// tail removal, and the collab_snapshot_compacted event and audit.
#[allow(clippy::too_many_arguments)]
async fn compact_rows(
    tx: &mut Transaction<'_, Postgres>,
    t: &CollabTables,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    document_id: Uuid,
    writer_generation: i64,
    cutoff_seq: i64,
    new_snapshot: &[u8],
    client_ip: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(&t.sql(
        r#"
        UPDATE {states}
        SET state = $3,
            snapshot_cutoff_seq = $4,
            updated_at = now()
        WHERE workspace_id = $1 AND {id} = $2 AND writer_generation = $5
        "#,
    ))
    .bind(workspace_id)
    .bind(document_id)
    .bind(new_snapshot)
    .bind(cutoff_seq)
    .bind(writer_generation)
    .execute(&mut **tx)
    .await?;

    sqlx::query(&t.sql(
        r#"
        DELETE FROM {updates}
        WHERE workspace_id = $1 AND {id} = $2 AND seq <= $3
        "#,
    ))
    .bind(workspace_id)
    .bind(document_id)
    .bind(cutoff_seq)
    .execute(&mut **tx)
    .await?;

    let payload = json!({
        t.payload_key: document_id.to_string(),
        "cutoffSeq": cutoff_seq,
        "writerGeneration": writer_generation,
    });
    append_event(
        &mut *tx,
        EventAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(workspace_id),
            actor_user_id: Some(actor_user_id),
            verb: t.verb("collab_snapshot_compacted"),
            target_type: Some(t.target_type.to_string()),
            target_id: Some(document_id),
            payload: payload.clone(),
        },
    )
    .await?;
    append_audit(
        &mut *tx,
        AuditAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(workspace_id),
            actor_user_id: Some(actor_user_id),
            verb: t.verb("collab_snapshot_compacted"),
            target_type: Some(t.target_type.to_string()),
            target_id: Some(document_id),
            payload,
            ip: client_ip.map(str::to_string),
        },
    )
    .await?;

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollabAdmission {
    pub read_only: bool,
    pub archived: bool,
}

/// Resolve whether a live session may join a wiki or project document collab room.
pub async fn resolve_collab_admission(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
) -> Result<Result<CollabAdmission, CollabDbError>, sqlx::Error> {
    resolve_collab_admission_kind(
        pool,
        CollabKind::Document,
        workspace_id,
        actor_user_id,
        session_id,
        document_id,
    )
    .await
}

/// Resolve whether a live session may join a document or task collab room.
pub async fn resolve_collab_admission_kind(
    pool: &PgPool,
    kind: CollabKind,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
) -> Result<Result<CollabAdmission, CollabDbError>, sqlx::Error> {
    resolve_collab_admission_kind_backend(
        &Backend::Postgres(pool.clone()),
        kind,
        workspace_id,
        actor_user_id,
        session_id,
        document_id,
    )
    .await
}

pub async fn resolve_collab_admission_kind_backend(
    backend: &Backend,
    kind: CollabKind,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
) -> Result<Result<CollabAdmission, CollabDbError>, sqlx::Error> {
    let mut tx = backend.begin_write().await?;
    tx.operation().set_tenant(workspace_id).await?;
    let access = match tx
        .operation()
        .authorize_collab_read(
            kind,
            workspace_id,
            actor_user_id,
            session_id,
            document_id,
            &mut CollabDbStageTimings::default(),
        )
        .await?
    {
        Ok(access) => access,
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    };
    tx.commit().await.map_err(|error| error.source)?;
    Ok(Ok(CollabAdmission {
        read_only: access.archived || !access.permission.at_least(ProjectPermission::Edit),
        archived: access.archived,
    }))
}

/// Read-only collab load for sync without claiming writer generation.
pub async fn load_collab_readonly_kind(
    pool: &PgPool,
    kind: CollabKind,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
) -> Result<Result<CollabLoadState, CollabDbError>, sqlx::Error> {
    load_collab_readonly_kind_backend(
        &Backend::Postgres(pool.clone()),
        kind,
        workspace_id,
        actor_user_id,
        session_id,
        document_id,
    )
    .await
}

/// One-shot, per-fixture fault after a real restore commit. No production hook.
#[cfg(feature = "db-tests")]
type RestoreCommitBarrier = (
    tokio::sync::oneshot::Sender<()>,
    tokio::sync::oneshot::Receiver<()>,
);
#[cfg(feature = "db-tests")]
static RESTORE_COMMIT_AMBIGUITY: std::sync::LazyLock<
    tokio::sync::Mutex<std::collections::HashMap<(Uuid, Uuid), RestoreCommitBarrier>>,
> = std::sync::LazyLock::new(|| tokio::sync::Mutex::new(std::collections::HashMap::new()));
#[cfg(feature = "db-tests")]
pub async fn arm_restore_committed_ambiguity(
    workspace_id: Uuid,
    document_id: Uuid,
) -> (
    tokio::sync::oneshot::Receiver<()>,
    tokio::sync::oneshot::Sender<()>,
) {
    let (reached_tx, reached_rx) = tokio::sync::oneshot::channel();
    let (proceed_tx, proceed_rx) = tokio::sync::oneshot::channel();
    assert!(RESTORE_COMMIT_AMBIGUITY
        .lock()
        .await
        .insert((workspace_id, document_id), (reached_tx, proceed_rx))
        .is_none());
    (reached_rx, proceed_tx)
}

/// Documents whose persisted-bytes estimate fails while armed. Keyed by document so a
/// test arming it cannot fail room starts of other tests running in the same
/// process (libtest runs tests in parallel threads).
#[cfg(feature = "db-tests")]
static FORCE_ESTIMATE_FAIL: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<Uuid>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashSet::new()));

#[cfg(feature = "db-tests")]
pub fn arm_force_estimate_fail(document_id: Uuid) {
    FORCE_ESTIMATE_FAIL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(document_id);
}

#[cfg(feature = "db-tests")]
pub fn disarm_force_estimate_fail(document_id: Uuid) {
    FORCE_ESTIMATE_FAIL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(&document_id);
}

/// Best-effort persisted collab bytes for memory admission (snapshot + tail payloads).
/// Must run on a connection with tenant context (`set_tenant`) so RLS returns rows.
pub async fn estimate_persisted_collab_bytes(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    document_id: Uuid,
) -> Result<u64, sqlx::Error> {
    estimate_persisted_collab_bytes_kind(conn, CollabKind::Document, workspace_id, document_id)
        .await
}

pub async fn estimate_persisted_collab_bytes_kind(
    conn: &mut PgConnection,
    kind: CollabKind,
    workspace_id: Uuid,
    document_id: Uuid,
) -> Result<u64, sqlx::Error> {
    #[cfg(feature = "db-tests")]
    if FORCE_ESTIMATE_FAIL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .contains(&document_id)
    {
        return Err(sqlx::Error::Protocol("forced estimate fail".into()));
    }
    let t = CollabTables::for_kind(kind);
    let mut tx = conn.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let row: Option<(i64, i64)> = sqlx::query_as(&t.sql(
        r#"
        SELECT
            coalesce(octet_length(ds.state), 0)::bigint,
            coalesce((
                SELECT sum(octet_length(payload))::bigint
                FROM {updates}
                WHERE workspace_id = ds.workspace_id
                  AND {id} = ds.{id}
                  AND seq > ds.snapshot_cutoff_seq
            ), 0)::bigint
        FROM {states} ds
        WHERE ds.workspace_id = $1 AND ds.{id} = $2
        "#,
    ))
    .bind(workspace_id)
    .bind(document_id)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(match row {
        Some((snapshot_len, tail_len)) => (snapshot_len + tail_len) as u64,
        None => 2,
    })
}

pub async fn project_derived_body(
    pool: &PgPool,
    input: ProjectDerivedBodyInput,
) -> Result<Result<ProjectDerivedBodyResult, CollabDbError>, sqlx::Error> {
    project_derived_body_kind(pool, CollabKind::Document, input).await
}

pub async fn project_derived_body_kind(
    pool: &PgPool,
    kind: CollabKind,
    input: ProjectDerivedBodyInput,
) -> Result<Result<ProjectDerivedBodyResult, CollabDbError>, sqlx::Error> {
    let t = CollabTables::for_kind(kind);
    let ProjectDerivedBodyInput {
        workspace_id,
        actor_user_id,
        session_id,
        document_id,
        writer_generation,
        expected_tail_seq,
        prepared,
    } = input;
    let content_json = prepared.content_json();
    let text = prepared.text();
    let chosung = prepared.chosung();

    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if let Err(err) = authorize_collab_write(
        &mut tx,
        kind,
        workspace_id,
        actor_user_id,
        session_id,
        document_id,
        &mut CollabDbStageTimings::default(),
    )
    .await?
    {
        tx.rollback().await?;
        return Ok(Err(err));
    }

    let state = fetch_state_fence_for_update(&mut tx, t, workspace_id, document_id).await?;
    let Some((current_generation, current_tail_seq)) = state else {
        tx.rollback().await?;
        return Ok(Err(CollabDbError::NotFound));
    };
    if current_tail_seq == 0 {
        tx.rollback().await?;
        return Ok(Ok(ProjectDerivedBodyResult::SkippedSeed));
    }
    if current_generation != writer_generation {
        tx.rollback().await?;
        return Ok(Err(CollabDbError::StaleWriter));
    }
    if current_tail_seq != expected_tail_seq {
        tx.rollback().await?;
        return Ok(Err(CollabDbError::StaleCutoff));
    }

    let updated: Option<(Uuid,)> = sqlx::query_as(&t.sql(
        r#"
        UPDATE {resource}
        SET content_json = $3,
            text = $4,
            chosung = $5,
            updated_at = now()
        WHERE workspace_id = $1
          AND id = $2
          AND content_json IS DISTINCT FROM $3::jsonb
        RETURNING id
        "#,
    ))
    .bind(workspace_id)
    .bind(document_id)
    .bind(content_json)
    .bind(text)
    .bind(chosung)
    .fetch_optional(&mut *tx)
    .await?;

    if updated.is_none() {
        tx.commit().await?;
        return Ok(Ok(ProjectDerivedBodyResult::Unchanged));
    }

    append_system_updated_event(&mut tx, t, workspace_id, document_id).await?;
    tx.commit().await?;
    Ok(Ok(ProjectDerivedBodyResult::Updated))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_yjs_seed_is_fixed_bytes() {
        assert_eq!(EMPTY_YJS_STATE_V1, &[0, 0]);
    }

    #[test]
    fn load_budget_allows_within_caps() {
        assert!(load_budget_allows(16 * 1024 * 1024, 32, 15 * 1024 * 1024).is_ok());
    }

    #[test]
    fn load_budget_rejects_tail_row_count_over_cap() {
        assert_eq!(
            load_budget_allows(0, MAX_COLLAB_TAIL_UPDATES + 1, 0),
            Err(CollabDbError::StateBudgetExceeded)
        );
    }

    #[test]
    fn load_budget_rejects_snapshot_plus_tail_bytes_over_cap() {
        assert_eq!(
            load_budget_allows(MAX_COLLAB_LOAD_BYTES, 1, 1),
            Err(CollabDbError::StateBudgetExceeded)
        );
    }
}
