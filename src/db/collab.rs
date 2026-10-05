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
use crate::db::identity::{append_event, AuditAppend, EventAppend};
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

/// Distinct task lineage. A document lease cannot authorize a task mutation.
/// Fields remain private; only the current authorized Task writer prepares it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FamilyTaskRoomFence {
    workspace_id: Uuid,
    task_id: Uuid,
    owner_token: Uuid,
    fence: i64,
}

/// Typed ON room lineage. Document ownership never certifies a Task writer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FamilyNativeRoomFence {
    Document(FamilyRoomFence),
    Task(FamilyTaskRoomFence),
}
impl FamilyNativeRoomFence {
    pub(crate) fn kind(self) -> CollabKind {
        match self {
            Self::Document(_) => CollabKind::Document,
            Self::Task(_) => CollabKind::Task,
        }
    }
    pub(crate) fn workspace(self) -> Uuid {
        match self {
            Self::Document(f) => f.workspace_id,
            Self::Task(f) => f.workspace_id,
        }
    }
    pub(crate) fn resource(self) -> Uuid {
        match self {
            Self::Document(f) => f.document_id,
            Self::Task(f) => f.task_id,
        }
    }
    pub(crate) fn owner(self) -> Uuid {
        match self {
            Self::Document(f) => f.owner_token,
            Self::Task(f) => f.owner_token,
        }
    }
    pub(crate) fn sequence(self) -> i64 {
        match self {
            Self::Document(f) => f.fence,
            Self::Task(f) => f.fence,
        }
    }
    pub(crate) fn matches(self, kind: CollabKind, workspace: Uuid, resource: Uuid) -> bool {
        self.kind() == kind && self.workspace() == workspace && self.resource() == resource
    }
    pub(crate) fn with_owner(self, owner: Uuid) -> Self {
        match self {
            Self::Document(mut f) => {
                f.owner_token = owner;
                Self::Document(f)
            }
            Self::Task(mut f) => {
                f.owner_token = owner;
                Self::Task(f)
            }
        }
    }
}

pub(crate) struct FamilyNativeRoomClaim {
    pub(crate) fence: FamilyNativeRoomFence,
    pub(crate) native: ClaimWriterResult,
}

/// Prepared on a borrowed writer; the caller must confirm that same commit
/// before acknowledging room ownership or exposing native state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedFamilyTaskRoomClaim {
    pub fence: FamilyTaskRoomFence,
    pub native: ClaimWriterResult,
}

/// Actor-native capture capability. Fields stay private to named product
/// operations; HTTP clients never choose or serialize this room/head proof.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FamilyNativeConsumerProof {
    pub(crate) room: FamilyNativeRoomFence,
    pub(crate) generation: i64,
    pub(crate) tail: i64,
}

/// Stable room lineage carried by the actor-issued socket lease. Activation
/// rotates only the known owner token, retaining the same global fence. A
/// successor or a purged/recreated document can never match this lineage.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FamilyRoomDeliveryFence {
    pub(crate) original: FamilyNativeRoomFence,
    pub(crate) writer_owner: Uuid,
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

impl OperationTx<'_, '_> {
    pub(crate) async fn verify_family_native_room_fence(
        &mut self,
        fence: FamilyNativeRoomFence,
    ) -> Result<bool, sqlx::Error> {
        match fence {
            FamilyNativeRoomFence::Document(fence) => self.verify_family_room_fence(fence).await,
            FamilyNativeRoomFence::Task(fence) => self.verify_family_task_room_fence(fence).await,
        }
    }
    async fn family_room_native_head(
        &mut self,
        proof: FamilyNativeConsumerProof,
    ) -> Result<Option<(i64, i64)>, sqlx::Error> {
        if !self.verify_family_native_room_fence(proof.room).await? {
            return Ok(None);
        }
        let Self::SqliteFamily(family) = self else {
            return Ok(None);
        };
        let sql = match proof.room.kind() {
            CollabKind::Document => "SELECT writer_generation,tail_seq FROM document_states WHERE workspace_id=?1 AND document_id=?2",
            CollabKind::Task => "SELECT writer_generation,tail_seq FROM task_states WHERE workspace_id=?1 AND task_id=?2",
        };
        let rows = family
            .query(
                sql,
                &[
                    Cell::uuid(proof.room.workspace()),
                    Cell::uuid(proof.room.resource()),
                ],
            )
            .await?;
        let head = rows
            .first()
            .map(|row| -> Result<(i64, i64), sqlx::Error> {
                Ok((row.cell(0)?.integer()?, row.cell(1)?.integer()?))
            })
            .transpose()?;
        if !self.verify_family_native_room_fence(proof.room).await? {
            return Ok(None);
        }
        Ok(head)
    }

    pub(crate) async fn verify_family_native_consumer_proof(
        &mut self,
        proof: FamilyNativeConsumerProof,
    ) -> Result<bool, sqlx::Error> {
        Ok(self.family_room_native_head(proof).await? == Some((proof.generation, proof.tail)))
    }
}

pub(crate) async fn verify_room_native_consumer(
    backend: &Backend,
    kind: CollabKind,
    workspace: Uuid,
    actor: Uuid,
    credential: Uuid,
    resource: Uuid,
    proof: Option<FamilyNativeConsumerProof>,
) -> Result<Result<(), CollabDbError>, sqlx::Error> {
    if matches!(backend, Backend::Postgres(_)) {
        return Ok(Ok(()));
    }
    let Some(proof) = proof else {
        return Ok(Err(CollabDbError::StaleWriter));
    };
    if !proof.room.matches(kind, workspace, resource) {
        return Ok(Err(CollabDbError::StaleWriter));
    }
    let mut tx = backend.begin_write().await?;
    let result = async {
        let mut op = tx.operation();
        op.set_tenant(workspace).await?;
        if let Err(error) = op
            .authorize_collab_read(
                kind,
                workspace,
                actor,
                credential,
                resource,
                &mut CollabDbStageTimings::default(),
            )
            .await?
        {
            return Ok(Err(error));
        }
        if !op.verify_family_native_consumer_proof(proof).await? {
            return Ok(Err(CollabDbError::StaleWriter));
        }
        Ok(Ok(()))
    }
    .await;
    finish_native_room_operation(tx, kind, result).await
}

/// Ambiguous room receipts carry current lineage + native generation. The
/// successful own COMMIT may have advanced tail beyond the actor's old cache.
pub(crate) async fn verify_room_collab_operation(
    backend: &Backend,
    kind: CollabKind,
    input: VerifyCollabInput<'_>,
    proof: Option<FamilyNativeConsumerProof>,
) -> Result<Result<CollabOperationLookup, CollabDbError>, sqlx::Error> {
    if matches!(backend, Backend::Postgres(_)) {
        return verify_collab_operation_kind_backend(backend, kind, input).await;
    }
    let Some(proof) = proof else {
        return Ok(Err(CollabDbError::StaleWriter));
    };
    if !proof
        .room
        .matches(kind, input.workspace_id, input.document_id)
    {
        return Ok(Err(CollabDbError::StaleWriter));
    }
    let mut tx = backend.begin_write().await?;
    let result = async {
        let mut op = tx.operation();
        op.set_tenant(input.workspace_id).await?;
        let result = op.verify_native_operation(kind, input).await?;
        if let Ok(receipt) = &result {
            let head = op.family_room_native_head(proof).await?;
            if !head.is_some_and(|(generation, tail)| {
                generation == proof.generation && tail >= receipt.seq
            }) {
                return Ok(Err(CollabDbError::StaleWriter));
            }
        }
        Ok(result)
    }
    .await;
    finish_native_room_operation(tx, kind, result).await
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
    claim_family_document_room_mode(
        backend,
        (workspace, document),
        actor,
        credential,
        owner,
        lease,
        NativeLoadMode::Writer,
    )
    .await
}

/// A reader room owns the real family guard without claiming native writer
/// generation. Its first authorized writer activates that same guard later.
pub async fn acquire_family_document_room(
    backend: &Backend,
    workspace: Uuid,
    actor: Uuid,
    credential: Uuid,
    document: Uuid,
    owner: Uuid,
    lease: std::time::Duration,
) -> Result<Result<FamilyRoomClaim, CollabDbError>, sqlx::Error> {
    claim_family_document_room_mode(
        backend,
        (workspace, document),
        actor,
        credential,
        owner,
        lease,
        NativeLoadMode::Reader,
    )
    .await
}

async fn claim_family_document_room_mode(
    backend: &Backend,
    target: (Uuid, Uuid),
    actor: Uuid,
    credential: Uuid,
    owner: Uuid,
    lease: std::time::Duration,
    mode: NativeLoadMode,
) -> Result<Result<FamilyRoomClaim, CollabDbError>, sqlx::Error> {
    let (workspace, document) = target;
    let (tx, claim) = match prepare_family_document_room_claim(
        backend,
        (workspace, document),
        actor,
        credential,
        owner,
        lease,
        mode,
    )
    .await?
    {
        Ok(prepared) => prepared,
        Err(error) => return Ok(Err(error)),
    };
    tx.commit().await.map_err(|error| error.source)?;
    Ok(Ok(claim))
}

/// Startup retains original COMMIT uncertainty separately from a driver error
/// before COMMIT. Only a confirmed exact-stream cleanup authorizes reconciliation.
#[derive(Debug, thiserror::Error)]
pub(crate) enum FamilyRoomStartError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error("family room startup COMMIT outcome unknown")]
    Commit {
        source: sqlx::Error,
        settlement: FamilyRoomStartSettlement,
    },
}
#[derive(Debug, Clone, Copy)]
pub(crate) enum FamilyRoomStartSettlement {
    /// SQLx local transaction rollback queue is serialized before a new writer.
    LocalSqlx,
    /// No supported receipt proves the original remote stream settled.
    RemoteUnconfirmed,
}
impl FamilyRoomStartError {
    pub(crate) fn source_error(&self) -> &sqlx::Error {
        match self {
            Self::Database(error) | Self::Commit { source: error, .. } => error,
        }
    }
    pub(crate) fn may_reconcile(&self) -> bool {
        matches!(
            self,
            Self::Commit {
                settlement: FamilyRoomStartSettlement::LocalSqlx,
                ..
            }
        )
    }
}

#[cfg(feature = "db-tests")]
struct RoomStartReplyFault {
    committed: bool,
    reached: tokio::sync::oneshot::Sender<()>,
    proceed: tokio::sync::oneshot::Receiver<()>,
}
#[cfg(feature = "db-tests")]
static ROOM_START_REPLY_FAULTS: std::sync::LazyLock<
    tokio::sync::Mutex<std::collections::HashMap<Uuid, RoomStartReplyFault>>,
> = std::sync::LazyLock::new(|| tokio::sync::Mutex::new(std::collections::HashMap::new()));
/// Inject only the application reply boundary around actual COMMIT/ROLLBACK.
/// This is not a substitute for an actual remote lost-COMMIT protocol fixture.
#[cfg(feature = "db-tests")]
pub async fn arm_family_room_start_reply_fault(
    document: Uuid,
    committed: bool,
) -> (
    tokio::sync::oneshot::Receiver<()>,
    tokio::sync::oneshot::Sender<()>,
) {
    let (reached, receive) = tokio::sync::oneshot::channel();
    let (send, proceed) = tokio::sync::oneshot::channel();
    assert!(ROOM_START_REPLY_FAULTS
        .lock()
        .await
        .insert(
            document,
            RoomStartReplyFault {
                committed,
                reached,
                proceed
            }
        )
        .is_none());
    (receive, send)
}

pub(crate) async fn acquire_family_native_room_for_start(
    backend: &Backend,
    target: (CollabKind, Uuid, Uuid),
    identity: (Uuid, Uuid),
    owner: Uuid,
    lease: std::time::Duration,
) -> Result<Result<FamilyNativeRoomClaim, CollabDbError>, FamilyRoomStartError> {
    let (kind, workspace, resource) = target;
    let (actor, credential) = identity;
    if kind == CollabKind::Document {
        return acquire_family_document_room_for_start(
            backend, workspace, actor, credential, resource, owner, lease,
        )
        .await
        .map(|result| {
            result.map(|claim| FamilyNativeRoomClaim {
                fence: FamilyNativeRoomFence::Document(claim.fence),
                native: claim.native,
            })
        });
    }
    let mut tx = backend.begin_write().await?;
    let result = tx
        .operation()
        .prepare_family_task_room_claim(
            (workspace, resource),
            identity,
            owner,
            lease,
            NativeLoadMode::Reader,
        )
        .await;
    let claim = match result {
        Ok(Ok(claim)) => claim,
        other => {
            return finish_family_task_room(tx, other)
                .await
                .map(|result| {
                    result.map(|claim| FamilyNativeRoomClaim {
                        fence: FamilyNativeRoomFence::Task(claim.fence),
                        native: claim.native,
                    })
                })
                .map_err(FamilyRoomStartError::Database)
        }
    };
    finish_family_room_start(tx, resource).await?;
    Ok(Ok(FamilyNativeRoomClaim {
        fence: FamilyNativeRoomFence::Task(claim.fence),
        native: claim.native,
    }))
}

pub(crate) async fn acquire_family_document_room_for_start(
    backend: &Backend,
    workspace: Uuid,
    actor: Uuid,
    credential: Uuid,
    document: Uuid,
    owner: Uuid,
    lease: std::time::Duration,
) -> Result<Result<FamilyRoomClaim, CollabDbError>, FamilyRoomStartError> {
    let (tx, claim) = match prepare_family_document_room_claim(
        backend,
        (workspace, document),
        actor,
        credential,
        owner,
        lease,
        NativeLoadMode::Reader,
    )
    .await?
    {
        Ok(prepared) => prepared,
        Err(error) => return Ok(Err(error)),
    };
    finish_family_room_start(tx, document).await?;
    Ok(Ok(claim))
}

async fn finish_family_room_start(
    tx: DbTransaction<'_>,
    _resource: Uuid,
) -> Result<(), FamilyRoomStartError> {
    #[cfg(feature = "db-tests")]
    let fault = ROOM_START_REPLY_FAULTS.lock().await.remove(&_resource);
    #[cfg(feature = "db-tests")]
    if let Some(fault) = fault {
        if fault.committed {
            commit_family_room_start(tx).await?;
        } else {
            tx.rollback().await?;
        }
        let _ = fault.reached.send(());
        let _ = fault.proceed.await;
        return Err(FamilyRoomStartError::Commit {
            source: sqlx::Error::Protocol(
                "fixture lost startup outcome reply after actual commit/rollback".into(),
            ),
            settlement: FamilyRoomStartSettlement::LocalSqlx,
        });
    }
    commit_family_room_start(tx).await?;
    Ok(())
}

/// Until an accepted public SDK boundary proves settlement of the original
/// failed-COMMIT stream, remote uncertainty cannot authorize fresh observation.
/// Local SQLite retains SQLx's serialized rollback-queue contract; the actual
/// local lost-reply controls below observe actual COMMIT/rollback separately.
async fn commit_family_room_start(tx: DbTransaction<'_>) -> Result<(), FamilyRoomStartError> {
    tx.commit_with_cleanup().await.map_err(|unknown| {
        let settlement = if unknown.permits_reconciliation() {
            FamilyRoomStartSettlement::LocalSqlx
        } else {
            FamilyRoomStartSettlement::RemoteUnconfirmed
        };
        FamilyRoomStartError::Commit {
            source: sqlx::Error::AnyDriverError(Box::new(unknown)),
            settlement,
        }
    })
}

async fn prepare_family_document_room_claim<'a>(
    backend: &'a Backend,
    target: (Uuid, Uuid),
    actor: Uuid,
    credential: Uuid,
    owner: Uuid,
    lease: std::time::Duration,
    mode: NativeLoadMode,
) -> Result<Result<(DbTransaction<'a>, FamilyRoomClaim), CollabDbError>, sqlx::Error> {
    let (workspace, document) = target;
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
            mode,
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
    if new_owner && matches!(mode, NativeLoadMode::Writer) {
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
    Ok(Ok((tx, FamilyRoomClaim { fence, native })))
}

/// Stable activation receipt is a fresh owner token chosen once by the room.
/// A failed commit reply is reconciled under current edit authority using that
/// same token; a reader guard never advances generation by itself.
pub async fn activate_family_document_writer(
    backend: &Backend,
    guard: FamilyRoomFence,
    actor: Uuid,
    credential: Uuid,
    writer_owner: Uuid,
) -> Result<Result<FamilyRoomClaim, CollabDbError>, sqlx::Error> {
    if matches!(backend, Backend::Postgres(_)) {
        return Err(sqlx::Error::Protocol(
            "PostgreSQL writer activation retains its session guard".into(),
        ));
    }
    if writer_owner == guard.owner_token {
        return Err(sqlx::Error::Protocol(
            "writer activation must retain a distinct stable owner token".into(),
        ));
    }
    let mut tx = backend.begin_write().await?;
    let native = tx
        .operation()
        .load_collab_native(
            CollabKind::Document,
            guard.workspace_id,
            actor,
            credential,
            guard.document_id,
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
    let mut activated = guard;
    activated.owner_token = writer_owner;
    let already_activated = tx.operation().verify_family_room_fence(activated).await?;
    if !already_activated {
        if !tx.operation().verify_family_room_fence(guard).await? {
            tx.rollback().await?;
            return Ok(Err(CollabDbError::StaleWriter));
        }
        let OperationTx::SqliteFamily(family) = tx.operation() else {
            return Err(sqlx::Error::Protocol(
                "family writer activation requires SQLite family".into(),
            ));
        };
        let now = family_room_now(family).await?;
        let changed=family.execute("UPDATE collab_room_fences SET owner_token=?5 WHERE workspace_id=?1 AND document_id=?2 AND owner_token=?3 AND fence=?4 AND expires_at>?6",&[Cell::uuid(guard.workspace_id),Cell::uuid(guard.document_id),Cell::uuid(guard.owner_token),Cell::Integer(guard.fence),Cell::uuid(writer_owner),Cell::Integer(now)]).await?;
        if changed != 1 {
            tx.rollback().await?;
            return Ok(Err(CollabDbError::StaleWriter));
        }
        let generation = tx
            .operation()
            .bump_native_writer_generation(
                CollabTables::for_kind(CollabKind::Document),
                guard.workspace_id,
                guard.document_id,
            )
            .await?;
        let Some(generation) = generation else {
            tx.rollback().await?;
            return Ok(Err(CollabDbError::NotFound));
        };
        native.writer_generation = generation;
        native.load.writer_generation = generation;
    }
    if !tx.operation().verify_family_room_fence(activated).await? {
        tx.rollback().await?;
        return Ok(Err(CollabDbError::StaleWriter));
    }
    tx.commit().await.map_err(|unknown| unknown.source)?;
    Ok(Ok(FamilyRoomClaim {
        fence: activated,
        native,
    }))
}

/// Resolve and retire only this hub-owned startup attempt after acquisition
/// failure/unknown COMMIT. The once-chosen private token is retained even when
/// no fence reply arrived. No caller authority is needed to retire its own
/// lease after revocation; no other owner or successor is ever adopted.
pub(crate) async fn abandon_family_document_room_start(
    backend: &Backend,
    workspace: Uuid,
    document: Uuid,
    owner: Uuid,
) -> Result<(), sqlx::Error> {
    let mut tx = backend.begin_write().await?;
    tx.operation().set_tenant(workspace).await?;
    let DbTransaction::SqliteFamily(family) = &mut tx else {
        return Err(sqlx::Error::Protocol(
            "family startup cleanup requires family transaction".into(),
        ));
    };
    let rows=family.query("SELECT fence FROM collab_room_fences WHERE workspace_id=?1 AND document_id=?2 AND owner_token=?3",&[Cell::uuid(workspace),Cell::uuid(document),Cell::uuid(owner)]).await?;
    if let Some(row) = rows.first() {
        let fence = row.cell(0)?.integer()?;
        if fence <= 0 {
            return Err(sqlx::Error::Protocol("invalid startup room fence".into()));
        }
        let now = family_room_now(family).await?;
        let changed=family.execute("UPDATE collab_room_fences SET expires_at=?5 WHERE workspace_id=?1 AND document_id=?2 AND owner_token=?3 AND fence=?4",&[Cell::uuid(workspace),Cell::uuid(document),Cell::uuid(owner),Cell::Integer(fence),Cell::Integer(now)]).await?;
        if changed != 1 {
            return Err(sqlx::Error::Protocol(
                "startup room cleanup scope changed".into(),
            ));
        }
    }
    tx.commit().await.map_err(|unknown| unknown.source)?;
    Ok(())
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
    /// Retained archive history installation on the publisher's existing
    /// writer, after its destination/graph/hash/journal checks. This is an
    /// initial INSERT of preserved identities, never a live room append/reset.
    pub(crate) async fn install_archived_native_history(
        &mut self,
        claim: &crate::db::import_jobs::ImportClaim,
        archive: &crate::native_archive::Archive,
    ) -> Result<(), crate::db::native_archive::NativeDbError> {
        use crate::db::native_archive::NativeDbError;
        use crate::native_archive::ArchiveError;
        if claim.source != crate::db::import_jobs::ImportSource::NativeArchive {
            return Err(NativeDbError::Fenced);
        }
        if let Self::SqliteFamily(writer) = self {
            writer.require_writer()?;
            writer.require_tenant(claim.workspace_id)?;
        }
        archive.validate()?;
        self.require_import_admin(claim.workspace_id, claim.created_by, claim.session_id)
            .await?
            .map_err(|_| NativeDbError::Forbidden)?;
        if !self.hold_import_claim(claim).await? {
            return Err(NativeDbError::Fenced);
        }
        for state in &archive.graph.states {
            let (kind, expected_body) = match state.target_kind.as_str() {
                "document" => (
                    CollabKind::Document,
                    &archive
                        .graph
                        .documents
                        .iter()
                        .find(|row| row.id == state.target_id)
                        .ok_or_else(|| {
                            ArchiveError::Invalid("native document target missing".into())
                        })?
                        .content_json,
                ),
                "task" => (
                    CollabKind::Task,
                    &archive
                        .graph
                        .tasks
                        .iter()
                        .find(|row| row.id == state.target_id)
                        .ok_or_else(|| ArchiveError::Invalid("native task target missing".into()))?
                        .content_json,
                ),
                _ => return Err(ArchiveError::Invalid("native target kind".into()).into()),
            };
            if !self.hold_import_claim(claim).await? {
                return Err(NativeDbError::Fenced);
            }
            let t = CollabTables::for_kind(kind);
            // The graph owner already inserted these exact destination rows.
            // Historical deleted targets remain part of the validated archive.
            if self
                .load_collab_resource_content(t, claim.workspace_id, state.target_id)
                .await?
                != *expected_body
                || self
                    .native_state_generation(t, claim.workspace_id, state.target_id)
                    .await?
                    .is_some()
            {
                return Err(NativeDbError::Conflict);
            }
            if let Self::SqliteFamily(writer) = self {
                if kind==CollabKind::Document && !writer.query("SELECT document_id FROM collab_room_fences WHERE workspace_id=?1 AND document_id=?2",&[Cell::uuid(claim.workspace_id),Cell::uuid(state.target_id)]).await?.is_empty() {
                    return Err(NativeDbError::Conflict);
                }
            }
            self.install_archived_native_state(claim, archive, state, kind)
                .await?;
        }
        self.install_archived_revisions(claim, archive).await?;
        if !self.hold_import_claim(claim).await? {
            return Err(NativeDbError::Fenced);
        }
        self.require_import_admin(claim.workspace_id, claim.created_by, claim.session_id)
            .await?
            .map_err(|_| NativeDbError::Forbidden)?;
        Ok(())
    }

    async fn install_archived_native_state(
        &mut self,
        claim: &crate::db::import_jobs::ImportClaim,
        archive: &crate::native_archive::Archive,
        state: &crate::native_archive::NativeState,
        kind: CollabKind,
    ) -> Result<(), crate::db::native_archive::NativeDbError> {
        let snapshot = archive.bytes(&state.state_entry)?;
        let created = super::revisions::archive_native_instant(&state.created_at)?;
        let updated = super::revisions::archive_native_instant(&state.updated_at)?;
        let compacted = state
            .compacted_at
            .as_deref()
            .map(super::revisions::archive_native_instant)
            .transpose()?;
        let workspace = claim.workspace_id;
        let resource = state.target_id;
        let t = CollabTables::for_kind(kind);
        match self {
            Self::Postgres(tx) => {
                sqlx::query(&t.sql("INSERT INTO {states}(workspace_id,{id},state,encoding,writer_generation,snapshot_cutoff_seq,tail_seq,created_at,updated_at,compacted_at) VALUES($1,$2,$3,$4,0,$5,$6,$7,$8,$9)"))
                    .bind(workspace).bind(resource).bind(&snapshot).bind(state.encoding).bind(state.snapshot_cutoff_seq).bind(state.tail_seq).bind(created).bind(updated).bind(compacted).execute(&mut ***tx).await?;
            }
            Self::SqliteFamily(writer) => {
                let statement=match kind {
                    CollabKind::Document=>"INSERT INTO document_states(workspace_id,document_id,state,encoding,writer_generation,snapshot_cutoff_seq,tail_seq,created_at,updated_at,compacted_at) VALUES(?1,?2,?3,?4,0,?5,?6,?7,?8,?9)",
                    CollabKind::Task=>"INSERT INTO task_states(workspace_id,task_id,state,encoding,writer_generation,snapshot_cutoff_seq,tail_seq,created_at,updated_at,compacted_at) VALUES(?1,?2,?3,?4,0,?5,?6,?7,?8,?9)",
                };
                writer
                    .execute(
                        statement,
                        &[
                            Cell::uuid(workspace),
                            Cell::uuid(resource),
                            Cell::Blob(snapshot),
                            Cell::Integer(i64::from(state.encoding)),
                            Cell::Integer(state.snapshot_cutoff_seq),
                            Cell::Integer(state.tail_seq),
                            Cell::instant(created)?,
                            Cell::instant(updated)?,
                            compacted
                                .map(Cell::instant)
                                .transpose()?
                                .unwrap_or(Cell::Null),
                        ],
                    )
                    .await?;
            }
        }
        for update in &state.updates {
            let payload = archive.bytes(&update.payload_entry)?;
            let created = super::revisions::archive_native_instant(&update.created_at)?;
            match self {
                Self::Postgres(tx) => {
                    sqlx::query(&t.sql("INSERT INTO {updates}(workspace_id,{id},seq,op_id,payload,created_at) VALUES($1,$2,$3,$4,$5,$6)"))
                        .bind(workspace).bind(resource).bind(update.seq).bind(update.op_id).bind(payload).bind(created).execute(&mut ***tx).await?;
                }
                Self::SqliteFamily(writer) => {
                    let statement=match kind {
                        CollabKind::Document=>"INSERT INTO document_collab_updates(workspace_id,document_id,seq,op_id,payload,created_at) VALUES(?1,?2,?3,?4,?5,?6)",
                        CollabKind::Task=>"INSERT INTO task_collab_updates(workspace_id,task_id,seq,op_id,payload,created_at) VALUES(?1,?2,?3,?4,?5,?6)",
                    };
                    writer
                        .execute(
                            statement,
                            &[
                                Cell::uuid(workspace),
                                Cell::uuid(resource),
                                Cell::Integer(update.seq),
                                Cell::uuid(update.op_id),
                                Cell::Blob(payload),
                                Cell::instant(created)?,
                            ],
                        )
                        .await?;
                }
            }
        }
        for receipt in &state.receipts {
            let digest = hex::decode(&receipt.payload_sha256)
                .map_err(|error| sqlx::Error::Decode(Box::new(error)))?;
            let created = super::revisions::archive_native_instant(&receipt.created_at)?;
            match self {
                Self::Postgres(tx) => {
                    sqlx::query(&t.sql("INSERT INTO {receipts}(workspace_id,{id},op_id,seq,actor_user_id,payload_len,payload_sha256,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8)"))
                        .bind(workspace).bind(resource).bind(receipt.op_id).bind(receipt.seq).bind(claim.created_by).bind(receipt.payload_len).bind(digest).bind(created).execute(&mut ***tx).await?;
                }
                Self::SqliteFamily(writer) => {
                    let statement=match kind {
                        CollabKind::Document=>"INSERT INTO document_collab_op_receipts(workspace_id,document_id,op_id,seq,actor_user_id,payload_len,payload_sha256,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                        CollabKind::Task=>"INSERT INTO task_collab_op_receipts(workspace_id,task_id,op_id,seq,actor_user_id,payload_len,payload_sha256,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                    };
                    writer
                        .execute(
                            statement,
                            &[
                                Cell::uuid(workspace),
                                Cell::uuid(resource),
                                Cell::uuid(receipt.op_id),
                                Cell::Integer(receipt.seq),
                                Cell::uuid(claim.created_by),
                                Cell::Integer(receipt.payload_len),
                                Cell::Blob(digest),
                                Cell::instant(created)?,
                            ],
                        )
                        .await?;
                }
            }
        }
        Ok(())
    }

    /// Current native head on the caller's existing writer. Scheduled native
    /// capture retains this generation/cutoff/tail through helper settlement.
    pub(crate) async fn durable_native_head(
        &mut self,
        kind: CollabKind,
        workspace: Uuid,
        resource: Uuid,
    ) -> Result<Option<(i64, i64, i64)>, sqlx::Error> {
        Ok(self
            .native_append_fence(CollabTables::for_kind(kind), workspace, resource)
            .await?
            .map(|(_, generation, cutoff, tail)| (generation, cutoff, tail)))
    }

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
        self.record_native_append_owned(t, record, None)
            .await
            .map(|_| ())
    }

    async fn record_native_append_owned(
        &mut self,
        t: &CollabTables,
        record: CollabAuditRecord<'_>,
        import: Option<&crate::db::import_jobs::ImportClaim>,
    ) -> Result<bool, sqlx::Error> {
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
        let event = EventAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(workspace_id),
            actor_user_id: Some(actor_user_id),
            verb: t.verb("collab_update_appended"),
            target_type: Some(t.target_type.to_string()),
            target_id: Some(document_id),
            payload: payload.clone(),
        };
        if !self
            .append_native_publication_event(event, "web", import)
            .await?
        {
            return Ok(false);
        }
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
        .await?;
        Ok(true)
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
/// rest. Callers map the result with [`OperationTx::authorize_collab_write`] or
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
impl OperationTx<'_, '_> {
    /// Writer check (claim, load, append, compaction, derived-body write): the
    /// actor prefix, then Edit on an unarchived resource. A dead session or a
    /// non-member is `Forbidden`. Existence (tenant, trash, affiliation) decides
    /// `NotFound` before permission decides `Forbidden`.
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
    OperationTx::Postgres(tx)
        .append_native_body_updated(t, workspace_id, document_id)
        .await
}

impl OperationTx<'_, '_> {
    async fn append_native_body_updated(
        &mut self,
        t: &CollabTables,
        workspace: Uuid,
        resource: Uuid,
    ) -> Result<(), sqlx::Error> {
        self.append_native_body_updated_owned(t, workspace, resource, None)
            .await
            .map(|_| ())
    }

    async fn append_native_body_updated_owned(
        &mut self,
        t: &CollabTables,
        workspace: Uuid,
        resource: Uuid,
        import: Option<&crate::db::import_jobs::ImportClaim>,
    ) -> Result<bool, sqlx::Error> {
        self.append_native_publication_event(
            EventAppend {
                id: Uuid::now_v7(),
                workspace_id: Some(workspace),
                actor_user_id: None,
                verb: t.verb("updated"),
                target_type: Some(t.target_type.to_string()),
                target_id: Some(resource),
                payload: json!({t.payload_key:resource.to_string(),"collab":true}),
            },
            "system",
            import,
        )
        .await
    }

    async fn append_native_publication_event(
        &mut self,
        event: EventAppend,
        channel: &str,
        import: Option<&crate::db::import_jobs::ImportClaim>,
    ) -> Result<bool, sqlx::Error> {
        if let Some(claim) = import {
            self.park_import_event(
                claim.workspace_id,
                crate::db::documents::ImportFence {
                    job_id: claim.job_id,
                    lease_token: claim.lease_token,
                },
                event,
                channel,
            )
            .await
        } else {
            self.append_event_channel(event, channel).await?;
            Ok(true)
        }
    }
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

#[derive(Clone, Copy)]
enum NativeWriteScope<'a> {
    Room(Option<FamilyNativeRoomFence>),
    Off(OffBodyWriter),
    Import(&'a crate::db::import_jobs::ImportClaim),
    SyncImport {
        workspace: Uuid,
        job: Uuid,
        actor: Uuid,
        credential: Uuid,
    },
}

impl OperationTx<'_, '_> {
    /// Persisted source only: caller has authorized the route's exact scope in
    /// this writer transaction. Unlike live sync this never seeds an absent
    /// state. The state lock keeps snapshot/head/tail from crossing a commit.
    pub(crate) async fn load_durable_native_source(
        &mut self,
        kind: CollabKind,
        workspace: Uuid,
        resource: Uuid,
    ) -> Result<Result<CollabLoadState, CollabDbError>, sqlx::Error> {
        let tables = CollabTables::for_kind(kind);
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
        Ok(Ok(state_row_to_load(state, tail)))
    }

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

/// A room reload may seed missing legacy state, so family reloads keep the
/// current room fence around the existing authorized operation and COMMIT.
pub(crate) async fn load_room_collab_readonly(
    backend: &Backend,
    kind: CollabKind,
    workspace: Uuid,
    actor: Uuid,
    credential: Uuid,
    resource: Uuid,
    fence: Option<FamilyNativeRoomFence>,
) -> Result<Result<CollabLoadState, CollabDbError>, sqlx::Error> {
    if matches!(backend, Backend::Postgres(_)) {
        return load_collab_readonly_kind_backend(
            backend, kind, workspace, actor, credential, resource,
        )
        .await;
    }
    let Some(fence) = fence else {
        return Ok(Err(CollabDbError::StaleWriter));
    };
    if !fence.matches(kind, workspace, resource) {
        return Ok(Err(CollabDbError::StaleWriter));
    }
    let mut tx = backend.begin_write().await?;
    let result = async {
        let mut op = tx.operation();
        op.set_tenant(workspace).await?;
        if !op.verify_family_native_room_fence(fence).await? {
            return Ok(Err(CollabDbError::StaleWriter));
        }
        let result = op
            .load_collab_native_readonly(kind, workspace, actor, credential, resource)
            .await?;
        if result.is_err() || !op.verify_family_native_room_fence(fence).await? {
            return Ok(result.and(Err(CollabDbError::StaleWriter)));
        }
        Ok(result)
    }
    .await;
    finish_native_room_operation(tx, kind, result).await
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
    append_family_document_room_update_timed(backend, fence, input)
        .await
        .map(|(result, _)| result)
}

pub(crate) async fn append_family_document_room_update_timed(
    backend: &Backend,
    fence: FamilyRoomFence,
    input: AppendCollabInput<'_>,
) -> Result<
    (
        Result<AppendCollabResult, CollabDbError>,
        CollabDbStageTimings,
    ),
    sqlx::Error,
> {
    if matches!(backend, Backend::Postgres(_)) {
        return Err(sqlx::Error::Protocol(
            "PostgreSQL room append requires its detached session connection".into(),
        ));
    }
    if input.payload.is_empty() || input.payload.len() > MAX_COLLAB_UPDATE_BYTES {
        return Ok((
            Err(CollabDbError::PayloadTooLarge),
            CollabDbStageTimings::default(),
        ));
    }
    let acquire_started = Instant::now();
    let tx = backend.begin_write().await?;
    let timings = CollabDbStageTimings {
        pool_wait_us: acquire_started.elapsed().as_micros() as u64,
        ..Default::default()
    };
    append_collab_update_in_tx(
        tx,
        CollabKind::Document,
        input,
        timings,
        None,
        Some(FamilyNativeRoomFence::Document(fence)),
    )
    .await
    .map(|(result, timings)| (result.map(|(appended, _)| appended), timings))
}

impl OperationTx<'_, '_> {
    /// ON native append program on the caller's current transaction. The
    /// caller owns commit/rollback, allowing projection/revision/command effects
    /// to be composed without reimplementing receipts or native tail policy.
    pub(crate) async fn append_collab_native(
        &mut self,
        kind: CollabKind,
        input: AppendCollabInput<'_>,
        timings: CollabDbStageTimings,
        restore: Option<&crate::db::revisions::RestoreRevisionAppend>,
        room_fence: Option<FamilyNativeRoomFence>,
    ) -> Result<
        (
            Result<(PreparedNativeAppend, Option<Uuid>), CollabDbError>,
            CollabDbStageTimings,
        ),
        sqlx::Error,
    > {
        self.append_collab_native_owned(
            kind,
            input,
            timings,
            restore,
            NativeWriteScope::Room(room_fence),
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn current_native_write_scope(
        &mut self,
        scope: NativeWriteScope<'_>,
        kind: CollabKind,
        workspace: Uuid,
        resource: Uuid,
        actor: Uuid,
        credential: Uuid,
    ) -> Result<bool, sqlx::Error> {
        match scope {
            NativeWriteScope::Off(proof) => Ok(proof.workspace == workspace
                && proof.resource == resource
                && proof.kind == kind
                && proof.actor == actor
                && proof.credential == credential
                && self.verify_off_body_writer(proof).await?),
            NativeWriteScope::Room(Some(fence)) => Ok(fence.matches(kind, workspace, resource)
                && self.verify_family_native_room_fence(fence).await?),
            NativeWriteScope::Room(None) => Ok(matches!(self, Self::Postgres(_))),
            NativeWriteScope::Import(claim) => {
                if kind != CollabKind::Document
                    || claim.workspace_id != workspace
                    || claim.created_by != actor
                    || claim.session_id != credential
                {
                    return Ok(false);
                }
                if self
                    .require_import_admin(workspace, actor, credential)
                    .await?
                    .is_err()
                {
                    return Ok(false);
                }
                self.import_claim_contains_ref(
                    claim,
                    crate::db::import_jobs::ImportRefKind::Document,
                    &resource.to_string(),
                )
                .await
            }
            NativeWriteScope::SyncImport {
                workspace: owned_workspace,
                job,
                actor: owned_actor,
                credential: owned_credential,
            } => {
                if kind != CollabKind::Document
                    || workspace != owned_workspace
                    || actor != owned_actor
                    || credential != owned_credential
                {
                    return Ok(false);
                }
                if self
                    .require_import_admin(workspace, actor, credential)
                    .await?
                    .is_err()
                {
                    return Ok(false);
                }
                self.sync_import_contains_document_ref(workspace, job, actor, resource)
                    .await
            }
        }
    }

    async fn append_collab_native_owned(
        &mut self,
        kind: CollabKind,
        input: AppendCollabInput<'_>,
        mut timings: CollabDbStageTimings,
        restore: Option<&crate::db::revisions::RestoreRevisionAppend>,
        write_scope: NativeWriteScope<'_>,
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
        if !self
            .current_native_write_scope(
                write_scope,
                kind,
                workspace_id,
                document_id,
                actor_user_id,
                session_id,
            )
            .await?
        {
            return Ok((Err(CollabDbError::StaleWriter), timings));
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
                if !self
                    .current_native_write_scope(
                        write_scope,
                        kind,
                        workspace_id,
                        document_id,
                        actor_user_id,
                        session_id,
                    )
                    .await?
                {
                    return Ok((Err(CollabDbError::StaleWriter), timings));
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

        if !self
            .record_native_append_owned(
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
                match write_scope {
                    NativeWriteScope::Import(claim) => Some(claim),
                    NativeWriteScope::Room(_)
                    | NativeWriteScope::Off(_)
                    | NativeWriteScope::SyncImport { .. } => None,
                },
            )
            .await?
        {
            return Ok((Err(CollabDbError::StaleWriter), timings));
        }

        if let Some(restore) = restore {
            self.apply_native_restore(t, workspace_id, actor_user_id, document_id, seq, restore)
                .await?;
        }
        if !self
            .current_native_write_scope(
                write_scope,
                kind,
                workspace_id,
                document_id,
                actor_user_id,
                session_id,
            )
            .await?
        {
            return Ok((Err(CollabDbError::StaleWriter), timings));
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
    room_fence: Option<FamilyNativeRoomFence>,
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
            if kind == CollabKind::Task && matches!(&tx, DbTransaction::SqliteFamily(_)) {
                if let Err(cleanup) = tx.rollback().await {
                    return Err(crate::db::backend::rollback_cleanup_unknown(
                        Some(Box::new(TaskRoomRefusal(error))),
                        cleanup,
                    ));
                }
            } else {
                tx.rollback().await?;
            }
            return Ok((Err(error), timings));
        }
        Ok(prepared) => prepared,
    };
    #[cfg(feature = "db-tests")]
    let committed_new_restore =
        restore.is_some() && matches!(prepared_result.0, PreparedNativeAppend::Appended { .. });
    let commit_started = Instant::now();
    if matches!(&tx, DbTransaction::SqliteFamily(_)) {
        tx.commit_with_cleanup()
            .await
            .map_err(|unknown| sqlx::Error::AnyDriverError(Box::new(unknown)))?;
    } else {
        tx.commit().await.map_err(|error| error.source)?;
    }

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
    verify_collab_operation_kind_backend(&Backend::Postgres(pool.clone()), kind, input).await
}

/// Reconcile an immutable receipt under current reader authority. A receipt
/// never revives a revoked credential or grants access to its former actor.
pub async fn verify_collab_operation_kind_backend(
    backend: &Backend,
    kind: CollabKind,
    input: VerifyCollabInput<'_>,
) -> Result<Result<CollabOperationLookup, CollabDbError>, sqlx::Error> {
    let mut tx = backend.begin_write().await?;
    let result = tx.operation().verify_native_operation(kind, input).await?;
    match result {
        Ok(receipt) => {
            tx.commit().await.map_err(|unknown| unknown.source)?;
            Ok(Ok(receipt))
        }
        Err(error) => {
            tx.rollback().await?;
            Ok(Err(error))
        }
    }
}

impl OperationTx<'_, '_> {
    pub(crate) async fn verify_native_operation(
        &mut self,
        kind: CollabKind,
        input: VerifyCollabInput<'_>,
    ) -> Result<Result<CollabOperationLookup, CollabDbError>, sqlx::Error> {
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
        self.set_tenant(workspace_id).await?;
        if let Err(error) = self
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
            return Ok(Err(error));
        }
        let Some(receipt) = self
            .native_operation_receipt(kind, workspace_id, document_id, op_id)
            .await?
        else {
            return Ok(Err(CollabDbError::NotFound));
        };
        if receipt.payload_len != expected_payload_len
            || receipt.payload_sha256.as_slice() != expected_payload_sha256
            || receipt.actor_user_id != expected_actor_user_id
        {
            return Ok(Err(CollabDbError::OpIdConflict));
        }
        Ok(Ok(receipt))
    }
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
    compact_collab_snapshot_kind_backend(&Backend::Postgres(pool.clone()), kind, input, None).await
}

/// An ON family compaction requires the live room's exact opaque proof. The
/// returned durable source is published only after the owned transaction commits.
pub async fn compact_collab_snapshot_kind_backend(
    backend: &Backend,
    kind: CollabKind,
    input: CompactCollabInput<'_>,
    room_fence: Option<FamilyRoomFence>,
) -> Result<Result<CollabLoadState, CollabDbError>, sqlx::Error> {
    compact_collab_snapshot_in_room(
        backend,
        kind,
        input,
        room_fence.map(FamilyNativeRoomFence::Document),
    )
    .await
}

pub(crate) async fn compact_collab_snapshot_in_room(
    backend: &Backend,
    kind: CollabKind,
    input: CompactCollabInput<'_>,
    room_fence: Option<FamilyNativeRoomFence>,
) -> Result<Result<CollabLoadState, CollabDbError>, sqlx::Error> {
    if let Err(error) = validate_compaction_snapshot(input.new_snapshot) {
        return Ok(Err(error));
    }
    let mut tx = backend.begin_write().await?;
    let result = tx.operation().compact_native(kind, input, room_fence).await;
    finish_native_room_operation(tx, kind, result)
        .await
        .map(|result| result.map(|prepared| prepared.load))
}

/// Uncommitted compaction result. Borrowed callers must finish their same
/// transaction before publishing this source or acknowledging a persist.
pub(crate) struct PreparedNativeCompaction {
    pub(crate) load: CollabLoadState,
}

fn validate_compaction_snapshot(snapshot: &[u8]) -> Result<(), CollabDbError> {
    if snapshot.is_empty() || snapshot.len() > MAX_COLLAB_SNAPSHOT_BYTES {
        Err(CollabDbError::PayloadTooLarge)
    } else {
        Ok(())
    }
}

impl OperationTx<'_, '_> {
    pub(crate) async fn compact_native(
        &mut self,
        kind: CollabKind,
        input: CompactCollabInput<'_>,
        room_fence: Option<FamilyNativeRoomFence>,
    ) -> Result<Result<PreparedNativeCompaction, CollabDbError>, sqlx::Error> {
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
        if let Err(error) = validate_compaction_snapshot(new_snapshot) {
            return Ok(Err(error));
        }
        self.set_tenant(workspace_id).await?;
        if matches!(self, Self::SqliteFamily(_)) {
            let Some(fence) = room_fence else {
                return Ok(Err(CollabDbError::StaleWriter));
            };
            if !fence.matches(kind, workspace_id, document_id)
                || !self.verify_family_native_room_fence(fence).await?
            {
                return Ok(Err(CollabDbError::StaleWriter));
            }
        }
        if let Err(error) = self
            .authorize_collab_write(
                kind,
                workspace_id,
                actor_user_id,
                session_id,
                document_id,
                &mut CollabDbStageTimings::default(),
            )
            .await?
        {
            return Ok(Err(error));
        }
        let content = self
            .load_collab_resource_content(t, workspace_id, document_id)
            .await?;
        if let Err(error) = self
            .ensure_collab_state(t, workspace_id, document_id, &content)
            .await?
        {
            return Ok(Err(error));
        }
        let Some(state) = self
            .fetch_native_state(t, workspace_id, document_id)
            .await?
        else {
            return Ok(Err(CollabDbError::NotFound));
        };
        if state.2 != writer_generation {
            return Ok(Err(CollabDbError::StaleWriter));
        }
        if expected_tail_seq != state.4 {
            return Ok(Err(CollabDbError::StaleCutoff));
        }
        if cutoff_seq < state.3 || cutoff_seq > state.4 {
            return Ok(Err(CollabDbError::InvalidCutoff));
        }
        let (newer, compactable) = self
            .native_compaction_tail_flags(t, workspace_id, document_id, cutoff_seq)
            .await?;
        if cutoff_seq < state.4 && newer {
            return Ok(Err(CollabDbError::InvalidCutoff));
        }
        let unchanged = cutoff_seq == state.3 && !compactable && state.0.as_slice() == new_snapshot;
        if !unchanged {
            self.compact_native_rows(
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
        let Some(refreshed) = self
            .fetch_native_state(t, workspace_id, document_id)
            .await?
        else {
            return Ok(Err(CollabDbError::NotFound));
        };
        let tail = match self
            .load_native_tail(
                t,
                workspace_id,
                document_id,
                refreshed.3,
                refreshed.0.len() as i64,
            )
            .await?
        {
            Ok(tail) => tail,
            Err(error) => return Ok(Err(error)),
        };
        if let Some(fence) = room_fence {
            if !self.verify_family_native_room_fence(fence).await? {
                return Ok(Err(CollabDbError::StaleWriter));
            }
        }
        Ok(Ok(PreparedNativeCompaction {
            load: state_row_to_load(refreshed, tail),
        }))
    }

    async fn native_compaction_tail_flags(
        &mut self,
        t: &CollabTables,
        workspace: Uuid,
        resource: Uuid,
        cutoff: i64,
    ) -> Result<(bool, bool), sqlx::Error> {
        match self {
            Self::Postgres(tx) => sqlx::query_as(&t.sql(
                "SELECT EXISTS(SELECT 1 FROM {updates} WHERE workspace_id=$1 AND {id}=$2 AND seq>$3), EXISTS(SELECT 1 FROM {updates} WHERE workspace_id=$1 AND {id}=$2 AND seq<=$3)"))
                .bind(workspace).bind(resource).bind(cutoff).fetch_one(&mut ***tx).await,
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_tenant(workspace)?;
                let statement = match t.kind {
                    CollabKind::Document => "SELECT EXISTS(SELECT 1 FROM document_collab_updates WHERE workspace_id=?1 AND document_id=?2 AND seq>?3),EXISTS(SELECT 1 FROM document_collab_updates WHERE workspace_id=?1 AND document_id=?2 AND seq<=?3)",
                    CollabKind::Task => "SELECT EXISTS(SELECT 1 FROM task_collab_updates WHERE workspace_id=?1 AND task_id=?2 AND seq>?3),EXISTS(SELECT 1 FROM task_collab_updates WHERE workspace_id=?1 AND task_id=?2 AND seq<=?3)",
                };
                let rows = tx.query(statement, &[Cell::uuid(workspace), Cell::uuid(resource), Cell::Integer(cutoff)]).await?;
                let row = rows.first().ok_or(sqlx::Error::RowNotFound)?;
                Ok((row.cell(0)?.boolean()?, row.cell(1)?.boolean()?))
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn compact_native_rows(
        &mut self,
        t: &CollabTables,
        workspace: Uuid,
        actor: Uuid,
        resource: Uuid,
        generation: i64,
        cutoff: i64,
        snapshot: &[u8],
        client_ip: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        match self {
            Self::Postgres(tx) => {
                sqlx::query(&t.sql("UPDATE {states} SET state=$3,snapshot_cutoff_seq=$4,updated_at=now() WHERE workspace_id=$1 AND {id}=$2 AND writer_generation=$5"))
                    .bind(workspace).bind(resource).bind(snapshot).bind(cutoff).bind(generation).execute(&mut ***tx).await?;
                sqlx::query(
                    &t.sql("DELETE FROM {updates} WHERE workspace_id=$1 AND {id}=$2 AND seq<=$3"),
                )
                .bind(workspace)
                .bind(resource)
                .bind(cutoff)
                .execute(&mut ***tx)
                .await?;
            }
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_tenant(workspace)?;
                let update = match t.kind {
                    CollabKind::Document => "UPDATE document_states SET state=?3,snapshot_cutoff_seq=?4,updated_at=CAST((julianday('now')-2440587.5)*86400000000 AS INTEGER) WHERE workspace_id=?1 AND document_id=?2 AND writer_generation=?5",
                    CollabKind::Task => "UPDATE task_states SET state=?3,snapshot_cutoff_seq=?4,updated_at=CAST((julianday('now')-2440587.5)*86400000000 AS INTEGER) WHERE workspace_id=?1 AND task_id=?2 AND writer_generation=?5",
                };
                let changed = tx
                    .execute(
                        update,
                        &[
                            Cell::uuid(workspace),
                            Cell::uuid(resource),
                            Cell::Blob(snapshot.to_vec()),
                            Cell::Integer(cutoff),
                            Cell::Integer(generation),
                        ],
                    )
                    .await?;
                if changed != 1 {
                    return Err(sqlx::Error::Protocol(
                        "locked native compaction target changed".into(),
                    ));
                }
                let delete = match t.kind {
                    CollabKind::Document => "DELETE FROM document_collab_updates WHERE workspace_id=?1 AND document_id=?2 AND seq<=?3",
                    CollabKind::Task => "DELETE FROM task_collab_updates WHERE workspace_id=?1 AND task_id=?2 AND seq<=?3",
                };
                tx.execute(
                    delete,
                    &[
                        Cell::uuid(workspace),
                        Cell::uuid(resource),
                        Cell::Integer(cutoff),
                    ],
                )
                .await?;
            }
        }
        let payload = json!({t.payload_key:resource.to_string(),"cutoffSeq":cutoff,"writerGeneration":generation});
        self.append_event(EventAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(workspace),
            actor_user_id: Some(actor),
            verb: t.verb("collab_snapshot_compacted"),
            target_type: Some(t.target_type.to_string()),
            target_id: Some(resource),
            payload: payload.clone(),
        })
        .await?;
        self.append_audit(AuditAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(workspace),
            actor_user_id: Some(actor),
            verb: t.verb("collab_snapshot_compacted"),
            target_type: Some(t.target_type.to_string()),
            target_id: Some(resource),
            payload,
            ip: client_ip.map(str::to_string),
        })
        .await?;
        Ok(())
    }
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

/// Current reader authority precedes the family estimate. This reads lengths
/// before loading native bytes so the hub can reserve helper memory first.
pub(crate) async fn estimate_family_room_bytes(
    backend: &Backend,
    kind: CollabKind,
    workspace: Uuid,
    actor: Uuid,
    credential: Uuid,
    document: Uuid,
) -> Result<Result<u64, CollabDbError>, sqlx::Error> {
    #[cfg(feature = "db-tests")]
    if FORCE_ESTIMATE_FAIL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .contains(&document)
    {
        return Err(sqlx::Error::Protocol("forced estimate fail".into()));
    }
    let mut tx = backend.begin_write().await?;
    tx.operation().set_tenant(workspace).await?;
    if let Err(error) = tx
        .operation()
        .authorize_collab_read(
            kind,
            workspace,
            actor,
            credential,
            document,
            &mut CollabDbStageTimings::default(),
        )
        .await?
    {
        tx.rollback().await?;
        return Ok(Err(error));
    }
    let DbTransaction::SqliteFamily(family) = &mut tx else {
        return Err(sqlx::Error::Protocol(
            "family estimate requires family transaction".into(),
        ));
    };
    let sql=match kind {
        CollabKind::Document=>"SELECT length(ds.state),coalesce((SELECT sum(length(payload)) FROM document_collab_updates u WHERE u.workspace_id=ds.workspace_id AND u.document_id=ds.document_id AND u.seq>ds.snapshot_cutoff_seq),0) FROM document_states ds WHERE ds.workspace_id=?1 AND ds.document_id=?2",
        CollabKind::Task=>"SELECT length(ds.state),coalesce((SELECT sum(length(payload)) FROM task_collab_updates u WHERE u.workspace_id=ds.workspace_id AND u.task_id=ds.task_id AND u.seq>ds.snapshot_cutoff_seq),0) FROM task_states ds WHERE ds.workspace_id=?1 AND ds.task_id=?2",
    };
    let rows = family
        .query(sql, &[Cell::uuid(workspace), Cell::uuid(document)])
        .await?;
    let bytes = match rows.first() {
        Some(row) => row
            .cell(0)?
            .integer()?
            .checked_add(row.cell(1)?.integer()?)
            .and_then(|n| u64::try_from(n).ok())
            .ok_or_else(|| sqlx::Error::Protocol("native size estimate overflow".into()))?,
        None => 2,
    };
    tx.commit().await.map_err(|unknown| unknown.source)?;
    Ok(Ok(bytes))
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
    project_derived_body_kind_backend(&Backend::Postgres(pool.clone()), kind, input, None).await
}

pub async fn project_derived_body_kind_backend(
    backend: &Backend,
    kind: CollabKind,
    input: ProjectDerivedBodyInput,
    room_fence: Option<FamilyRoomFence>,
) -> Result<Result<ProjectDerivedBodyResult, CollabDbError>, sqlx::Error> {
    project_derived_body_in_room(
        backend,
        kind,
        input,
        room_fence.map(FamilyNativeRoomFence::Document),
    )
    .await
}

pub(crate) async fn project_derived_body_in_room(
    backend: &Backend,
    kind: CollabKind,
    input: ProjectDerivedBodyInput,
    room_fence: Option<FamilyNativeRoomFence>,
) -> Result<Result<ProjectDerivedBodyResult, CollabDbError>, sqlx::Error> {
    let mut tx = backend.begin_write().await?;
    let result = tx
        .operation()
        .project_collab_derived_body(kind, input, room_fence)
        .await;
    finish_native_room_operation(tx, kind, result).await
}

impl OperationTx<'_, '_> {
    /// Derived body effect on the caller-owned current authorized native head.
    /// Metadata version is independent of this native tail/generation fence.
    pub(crate) async fn project_collab_derived_body(
        &mut self,
        kind: CollabKind,
        input: ProjectDerivedBodyInput,
        room_fence: Option<FamilyNativeRoomFence>,
    ) -> Result<Result<ProjectDerivedBodyResult, CollabDbError>, sqlx::Error> {
        self.project_collab_derived_body_owned(kind, input, NativeWriteScope::Room(room_fence))
            .await
    }

    async fn project_collab_derived_body_owned(
        &mut self,
        kind: CollabKind,
        input: ProjectDerivedBodyInput,
        write_scope: NativeWriteScope<'_>,
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
        self.set_tenant(workspace_id).await?;
        if !self
            .current_native_write_scope(
                write_scope,
                kind,
                workspace_id,
                document_id,
                actor_user_id,
                session_id,
            )
            .await?
        {
            return Ok(Err(CollabDbError::StaleWriter));
        }
        if let Err(error) = self
            .authorize_collab_write(
                kind,
                workspace_id,
                actor_user_id,
                session_id,
                document_id,
                &mut CollabDbStageTimings::default(),
            )
            .await?
        {
            return Ok(Err(error));
        }
        let state = self
            .native_append_fence(t, workspace_id, document_id)
            .await?;
        let Some((_, current_generation, _, current_tail_seq)) = state else {
            return Ok(Err(CollabDbError::NotFound));
        };
        if current_tail_seq == 0 {
            if !self
                .current_native_write_scope(
                    write_scope,
                    kind,
                    workspace_id,
                    document_id,
                    actor_user_id,
                    session_id,
                )
                .await?
            {
                return Ok(Err(CollabDbError::StaleWriter));
            }
            return Ok(Ok(ProjectDerivedBodyResult::SkippedSeed));
        }
        if current_generation != writer_generation {
            return Ok(Err(CollabDbError::StaleWriter));
        }
        if current_tail_seq != expected_tail_seq {
            return Ok(Err(CollabDbError::StaleCutoff));
        }
        let changed = self
            .write_native_body_projection(t, workspace_id, document_id, &prepared)
            .await?;
        if changed
            && !self
                .append_native_body_updated_owned(
                    t,
                    workspace_id,
                    document_id,
                    match write_scope {
                        NativeWriteScope::Import(claim) => Some(claim),
                        NativeWriteScope::Room(_)
                        | NativeWriteScope::Off(_)
                        | NativeWriteScope::SyncImport { .. } => None,
                    },
                )
                .await?
        {
            return Ok(Err(CollabDbError::StaleWriter));
        }
        if !self
            .current_native_write_scope(
                write_scope,
                kind,
                workspace_id,
                document_id,
                actor_user_id,
                session_id,
            )
            .await?
        {
            return Ok(Err(CollabDbError::StaleWriter));
        }
        if !changed {
            return Ok(Ok(ProjectDerivedBodyResult::Unchanged));
        }
        Ok(Ok(ProjectDerivedBodyResult::Updated))
    }

    /// First native publication of a newly created, current-job-owned wiki
    /// document. It composes the existing ON seed/append/receipt/body program
    /// on this writer; an existing state or retained room lineage is refused.
    pub(crate) async fn initialize_import_document_native(
        &mut self,
        owner: super::documents::ImportDocumentOwner<'_>,
        document: Uuid,
        op_id: Uuid,
        seed: &[u8],
        prepared: PreparedDerivedBody,
    ) -> Result<Result<(), CollabDbError>, sqlx::Error> {
        let kind = CollabKind::Document;
        let scope = match owner {
            super::documents::ImportDocumentOwner::Async(claim) => NativeWriteScope::Import(claim),
            super::documents::ImportDocumentOwner::Sync {
                workspace,
                job,
                actor,
                credential,
            } => NativeWriteScope::SyncImport {
                workspace,
                job,
                actor,
                credential,
            },
        };
        let workspace = owner.workspace();
        if !self
            .current_native_write_scope(
                scope,
                kind,
                workspace,
                document,
                owner.actor(),
                owner.credential(),
            )
            .await?
        {
            return Ok(Err(CollabDbError::StaleWriter));
        }
        if self
            .native_state_generation(CollabTables::for_kind(kind), workspace, document)
            .await?
            .is_some()
        {
            return Ok(Err(CollabDbError::StaleWriter));
        }
        if let Self::SqliteFamily(writer) = self {
            if !writer.query("SELECT document_id FROM collab_room_fences WHERE workspace_id=?1 AND document_id=?2", &[Cell::uuid(workspace),Cell::uuid(document)]).await?.is_empty() {
                return Ok(Err(CollabDbError::StaleWriter));
            }
        }
        let native = match self
            .load_collab_native(
                kind,
                workspace,
                owner.actor(),
                owner.credential(),
                document,
                NativeLoadMode::ClaimWriter,
            )
            .await?
        {
            Ok(native) => native,
            Err(error) => return Ok(Err(error)),
        };
        let (appended, _) = self
            .append_collab_native_owned(
                kind,
                AppendCollabInput {
                    workspace_id: workspace,
                    actor_user_id: owner.actor(),
                    session_id: owner.credential(),
                    document_id: document,
                    writer_generation: native.writer_generation,
                    expected_tail_seq: native.load.tail_seq,
                    op_id,
                    payload: seed,
                    client_ip: None,
                },
                CollabDbStageTimings::default(),
                None,
                scope,
            )
            .await?;
        let seq = match appended {
            Ok((appended, _)) => appended.seq(),
            Err(error) => return Ok(Err(error)),
        };
        self.project_collab_derived_body_owned(
            kind,
            ProjectDerivedBodyInput::new(
                workspace,
                owner.actor(),
                owner.credential(),
                document,
                native.writer_generation,
                seq,
                prepared,
            ),
            scope,
        )
        .await
        .map(|result| result.map(|_| ()))
    }

    async fn write_native_body_projection(
        &mut self,
        t: &CollabTables,
        workspace: Uuid,
        resource: Uuid,
        prepared: &PreparedDerivedBody,
    ) -> Result<bool, sqlx::Error> {
        match self {
            Self::Postgres(tx) => {
                let updated:Option<(Uuid,)>=sqlx::query_as(&t.sql(
                    "UPDATE {resource} SET content_json=$3,text=$4,chosung=$5,updated_at=now() WHERE workspace_id=$1 AND id=$2 AND content_json IS DISTINCT FROM $3::jsonb RETURNING id"
                )).bind(workspace).bind(resource).bind(prepared.content_json()).bind(prepared.text()).bind(prepared.chosung()).fetch_optional(&mut ***tx).await?;
                Ok(updated.is_some())
            }
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_tenant(workspace)?;
                let (read,write)=match t.kind {
                    CollabKind::Document=>("SELECT content_json FROM documents WHERE workspace_id=?1 AND id=?2","UPDATE documents SET content_json=?3,text=?4,chosung=?5,updated_at=unixepoch()*1000000+CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000 WHERE workspace_id=?1 AND id=?2"),
                    CollabKind::Task=>("SELECT content_json FROM tasks WHERE workspace_id=?1 AND id=?2","UPDATE tasks SET content_json=?3,text=?4,chosung=?5,updated_at=unixepoch()*1000000+CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000 WHERE workspace_id=?1 AND id=?2"),
                };
                let rows = tx
                    .query(read, &[Cell::uuid(workspace), Cell::uuid(resource)])
                    .await?;
                let current = rows
                    .first()
                    .ok_or(sqlx::Error::RowNotFound)?
                    .cell(0)?
                    .value()?;
                if current == *prepared.content_json() {
                    return Ok(false);
                }
                let changed = tx
                    .execute(
                        write,
                        &[
                            Cell::uuid(workspace),
                            Cell::uuid(resource),
                            Cell::json(prepared.content_json())?,
                            Cell::text(prepared.text()),
                            Cell::text(prepared.chosung()),
                        ],
                    )
                    .await?;
                if changed != 1 {
                    return Err(sqlx::Error::RowNotFound);
                }
                Ok(true)
            }
        }
    }
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

/// Issued only from the boot-OFF operation after current writer authorization.
/// The native generation is a fence; it never grants business authorization.
#[derive(Clone, Copy)]
pub(crate) struct OffBodyWriter {
    workspace: Uuid,
    resource: Uuid,
    kind: CollabKind,
    actor: Uuid,
    credential: Uuid,
    generation: i64,
}

impl OperationTx<'_, '_> {
    async fn off_room_absent(
        &mut self,
        workspace: Uuid,
        resource: Uuid,
        kind: CollabKind,
    ) -> Result<bool, sqlx::Error> {
        match self {
            Self::Postgres(tx) => {
                sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1,$2)")
                    .bind(COLLAB_ROOM_SESSION_LOCK_NAMESPACE)
                    .bind(lock_key_from_uuid(resource))
                    .fetch_one(&mut ***tx)
                    .await
            }
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_tenant(workspace)?;
                let now = family_room_now(tx).await?;
                let sql = match kind {
                    CollabKind::Document => "SELECT NOT EXISTS(SELECT 1 FROM collab_room_fences WHERE workspace_id=?1 AND document_id=?2 AND expires_at>?3)",
                    CollabKind::Task => "SELECT NOT EXISTS(SELECT 1 FROM task_collab_room_fences WHERE workspace_id=?1 AND task_id=?2 AND expires_at>?3)",
                };
                let rows = tx
                    .query(
                        sql,
                        &[
                            Cell::uuid(workspace),
                            Cell::uuid(resource),
                            Cell::Integer(now),
                        ],
                    )
                    .await?;
                rows.first()
                    .ok_or(sqlx::Error::RowNotFound)?
                    .cell(0)?
                    .boolean()
            }
        }
    }

    pub(crate) async fn load_off_body_read(
        &mut self,
        mode: crate::config::RealtimeMode,
        kind: CollabKind,
        workspace: Uuid,
        actor: Uuid,
        credential: Uuid,
        resource: Uuid,
    ) -> Result<Result<CollabLoadState, CollabDbError>, sqlx::Error> {
        self.set_tenant(workspace).await?;
        if mode != crate::config::RealtimeMode::Off
            || !self.off_room_absent(workspace, resource, kind).await?
        {
            return Ok(Err(CollabDbError::StaleWriter));
        }
        self.load_collab_native_readonly(kind, workspace, actor, credential, resource)
            .await
    }

    pub(crate) async fn load_off_body_writer(
        &mut self,
        mode: crate::config::RealtimeMode,
        kind: CollabKind,
        workspace: Uuid,
        actor: Uuid,
        credential: Uuid,
        resource: Uuid,
    ) -> Result<Result<(OffBodyWriter, CollabLoadState), CollabDbError>, sqlx::Error> {
        self.set_tenant(workspace).await?;
        if mode != crate::config::RealtimeMode::Off
            || !self.off_room_absent(workspace, resource, kind).await?
        {
            return Ok(Err(CollabDbError::StaleWriter));
        }
        let loaded = match self
            .load_collab_native(
                kind,
                workspace,
                actor,
                credential,
                resource,
                NativeLoadMode::Writer,
            )
            .await?
        {
            Ok(value) => value.load,
            Err(error) => return Ok(Err(error)),
        };
        let proof = OffBodyWriter {
            workspace,
            resource,
            kind,
            actor,
            credential,
            generation: loaded.writer_generation,
        };
        Ok(Ok((proof, loaded)))
    }

    pub(crate) async fn verify_off_body_writer(
        &mut self,
        proof: OffBodyWriter,
    ) -> Result<bool, sqlx::Error> {
        if !self
            .off_room_absent(proof.workspace, proof.resource, proof.kind)
            .await?
        {
            return Ok(false);
        }
        let state = self
            .fetch_native_state(
                CollabTables::for_kind(proof.kind),
                proof.workspace,
                proof.resource,
            )
            .await?;
        Ok(state.is_some_and(|state| state.2 == proof.generation))
    }

    pub(crate) async fn append_off_body(
        &mut self,
        proof: OffBodyWriter,
        input: AppendCollabInput<'_>,
    ) -> Result<Result<PreparedNativeAppend, CollabDbError>, sqlx::Error> {
        let (result, _) = self
            .append_collab_native_owned(
                proof.kind,
                input,
                CollabDbStageTimings::default(),
                None,
                NativeWriteScope::Off(proof),
            )
            .await?;
        Ok(result.map(|(value, _)| value))
    }

    pub(crate) async fn project_off_body(
        &mut self,
        proof: OffBodyWriter,
        input: ProjectDerivedBodyInput,
    ) -> Result<Result<ProjectDerivedBodyResult, CollabDbError>, sqlx::Error> {
        self.project_collab_derived_body_owned(proof.kind, input, NativeWriteScope::Off(proof))
            .await
    }

    /// Compact the exact validated post-append completeV1 on the same writer.
    /// All delete history and receipts remain; only incorporated tail rows go.
    pub(crate) async fn compact_off_body(
        &mut self,
        proof: OffBodyWriter,
        tail: i64,
        snapshot: &[u8],
        client_ip: Option<&str>,
    ) -> Result<Result<(), CollabDbError>, sqlx::Error> {
        if let Err(error) = validate_compaction_snapshot(snapshot) {
            return Ok(Err(error));
        }
        if !self.verify_off_body_writer(proof).await? {
            return Ok(Err(CollabDbError::StaleWriter));
        }
        if let Err(error) = self
            .authorize_collab_write(
                proof.kind,
                proof.workspace,
                proof.actor,
                proof.credential,
                proof.resource,
                &mut CollabDbStageTimings::default(),
            )
            .await?
        {
            return Ok(Err(error));
        }
        let tables = CollabTables::for_kind(proof.kind);
        let Some(state) = self
            .fetch_native_state(tables, proof.workspace, proof.resource)
            .await?
        else {
            return Ok(Err(CollabDbError::NotFound));
        };
        if state.4 != tail {
            return Ok(Err(CollabDbError::StaleCutoff));
        }
        self.compact_native_rows(
            tables,
            proof.workspace,
            proof.actor,
            proof.resource,
            proof.generation,
            tail,
            snapshot,
            client_ip,
        )
        .await?;
        if !self.verify_off_body_writer(proof).await? {
            return Ok(Err(CollabDbError::StaleWriter));
        }
        Ok(Ok(()))
    }
}

impl OperationTx<'_, '_> {
    /// The stable owner is prepared by the actual caller once, across failures.
    /// No commit or fresh observation occurs inside this borrowed operation.
    #[cfg(all(test, feature = "db-tests"))]
    pub async fn prepare_family_task_room_writer(
        &mut self,
        target: (Uuid, Uuid),
        identity: (Uuid, Uuid),
        owner: Uuid,
        lease: std::time::Duration,
    ) -> Result<Result<PreparedFamilyTaskRoomClaim, CollabDbError>, sqlx::Error> {
        self.prepare_family_task_room_claim(target, identity, owner, lease, NativeLoadMode::Writer)
            .await
    }
    async fn prepare_family_task_room_claim(
        &mut self,
        target: (Uuid, Uuid),
        identity: (Uuid, Uuid),
        owner: Uuid,
        lease: std::time::Duration,
        mode: NativeLoadMode,
    ) -> Result<Result<PreparedFamilyTaskRoomClaim, CollabDbError>, sqlx::Error> {
        let (workspace, task) = target;
        let (actor, credential) = identity;
        if owner.is_nil() {
            return Err(sqlx::Error::Protocol(
                "task room owner must be prepared once".into(),
            ));
        }
        if !matches!(self, Self::SqliteFamily(_)) {
            return Err(sqlx::Error::Protocol(
                "PG task rooms retain their session advisory guard".into(),
            ));
        }
        let mut native = match self
            .load_collab_native(CollabKind::Task, workspace, actor, credential, task, mode)
            .await?
        {
            Ok(native) => native,
            Err(error) => return Ok(Err(error)),
        };
        let (fence, new_owner) = match self
            .claim_family_task_room_fence(workspace, task, owner, lease)
            .await?
        {
            Ok(claim) => claim,
            Err(error) => return Ok(Err(error)),
        };
        if new_owner && matches!(mode, NativeLoadMode::Writer) {
            let Some(generation) = self
                .bump_native_writer_generation(
                    CollabTables::for_kind(CollabKind::Task),
                    workspace,
                    task,
                )
                .await?
            else {
                return Ok(Err(CollabDbError::NotFound));
            };
            native.writer_generation = generation;
            native.load.writer_generation = generation;
        }
        if !self.verify_family_task_room_fence(fence).await? {
            return Ok(Err(CollabDbError::StaleWriter));
        }
        Ok(Ok(PreparedFamilyTaskRoomClaim { fence, native }))
    }

    async fn claim_family_task_room_fence(
        &mut self,
        workspace: Uuid,
        task: Uuid,
        owner: Uuid,
        lease: std::time::Duration,
    ) -> Result<Result<(FamilyTaskRoomFence, bool), CollabDbError>, sqlx::Error> {
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
            "SELECT owner_token,fence,expires_at FROM task_collab_room_fences WHERE workspace_id=?1 AND task_id=?2",
            &[Cell::uuid(workspace),Cell::uuid(task)],
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
                    FamilyTaskRoomFence {
                        workspace_id: workspace,
                        task_id: task,
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
            "INSERT INTO task_collab_room_fences(workspace_id,task_id,owner_token,fence,expires_at) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(workspace_id,task_id) DO UPDATE SET owner_token=excluded.owner_token,fence=excluded.fence,expires_at=excluded.expires_at WHERE task_collab_room_fences.expires_at<=?6",
            &[Cell::uuid(workspace),Cell::uuid(task),Cell::uuid(owner),Cell::Integer(fence),Cell::Integer(expires),Cell::Integer(now)],
        ).await?;
        if changed != 1 {
            return Ok(Err(CollabDbError::StaleWriter));
        }
        Ok(Ok((
            FamilyTaskRoomFence {
                workspace_id: workspace,
                task_id: task,
                owner_token: owner,
                fence,
            },
            true,
        )))
    }

    pub async fn verify_family_task_room_fence(
        &mut self,
        fence: FamilyTaskRoomFence,
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
            "SELECT EXISTS(SELECT 1 FROM task_collab_room_fences WHERE workspace_id=?1 AND task_id=?2 AND owner_token=?3 AND fence=?4 AND expires_at>?5)",
            &[Cell::uuid(fence.workspace_id),Cell::uuid(fence.task_id),Cell::uuid(fence.owner_token),Cell::Integer(fence.fence),Cell::Integer(now)],
        ).await?;
        rows.first()
            .ok_or(sqlx::Error::RowNotFound)?
            .cell(0)?
            .boolean()
    }

    /// Renewal cannot revive an expired owner; all fields and DB time match.
    pub async fn renew_family_task_room_fence(
        &mut self,
        fence: FamilyTaskRoomFence,
        lease: std::time::Duration,
    ) -> Result<bool, sqlx::Error> {
        let Self::SqliteFamily(tx) = self else {
            return Err(sqlx::Error::Protocol(
                "task room lease requires SQLite family".into(),
            ));
        };
        tx.require_writer()?;
        tx.require_tenant(fence.workspace_id)?;
        let now = family_room_now(tx).await?;
        let expires = now.checked_add(room_lease_micros(lease)?).ok_or_else(|| {
            sqlx::Error::Protocol("task room lease expiry exceeds signed microseconds".into())
        })?;
        let changed=tx.execute("UPDATE task_collab_room_fences SET expires_at=?5 WHERE workspace_id=?1 AND task_id=?2 AND owner_token=?3 AND fence=?4 AND expires_at>?6",
            &[Cell::uuid(fence.workspace_id),Cell::uuid(fence.task_id),Cell::uuid(fence.owner_token),Cell::Integer(fence.fence),Cell::Integer(expires),Cell::Integer(now)]).await?;
        Ok(changed == 1)
    }
    /// Expiring this exact lineage never releases a successor or resets a fence.
    pub async fn release_family_task_room_fence(
        &mut self,
        fence: FamilyTaskRoomFence,
    ) -> Result<bool, sqlx::Error> {
        let Self::SqliteFamily(tx) = self else {
            return Err(sqlx::Error::Protocol(
                "task room lease requires SQLite family".into(),
            ));
        };
        tx.require_writer()?;
        tx.require_tenant(fence.workspace_id)?;
        let now = family_room_now(tx).await?;
        let changed=tx.execute("UPDATE task_collab_room_fences SET expires_at=?5 WHERE workspace_id=?1 AND task_id=?2 AND owner_token=?3 AND fence=?4",
            &[Cell::uuid(fence.workspace_id),Cell::uuid(fence.task_id),Cell::uuid(fence.owner_token),Cell::Integer(fence.fence),Cell::Integer(now)]).await?;
        Ok(changed == 1)
    }
}

#[derive(Debug, thiserror::Error)]
#[error("Task room operation refused: {0:?}")]
struct TaskRoomRefusal(CollabDbError);

/// Finish this original Task operation only; a fresh observer is not a receipt.
async fn finish_family_task_room<T>(
    tx: DbTransaction<'_>,
    result: Result<Result<T, CollabDbError>, sqlx::Error>,
) -> Result<Result<T, CollabDbError>, sqlx::Error> {
    match result {
        Ok(Ok(value)) => {
            tx.commit_with_cleanup()
                .await
                .map_err(|unknown| sqlx::Error::AnyDriverError(Box::new(unknown)))?;
            Ok(Ok(value))
        }
        Ok(Err(refusal)) => {
            if let Err(cleanup) = tx.rollback().await {
                return Err(crate::db::backend::rollback_cleanup_unknown(
                    Some(Box::new(TaskRoomRefusal(refusal))),
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

/// Document and PG callers retain their existing settlement path. Task room
/// consumers preserve the original writer's typed finish and awaited cleanup.
async fn finish_native_room_operation<T>(
    tx: DbTransaction<'_>,
    kind: CollabKind,
    result: Result<Result<T, CollabDbError>, sqlx::Error>,
) -> Result<Result<T, CollabDbError>, sqlx::Error> {
    if kind == CollabKind::Task && matches!(&tx, DbTransaction::SqliteFamily(_)) {
        return finish_family_task_room(tx, result).await;
    }
    let result = result?;
    if result.is_ok() {
        tx.commit().await.map_err(|unknown| unknown.source)?;
    } else {
        tx.rollback().await?;
    }
    Ok(result)
}

pub(crate) async fn activate_family_task_writer(
    backend: &Backend,
    guard: FamilyTaskRoomFence,
    actor: Uuid,
    credential: Uuid,
    writer_owner: Uuid,
) -> Result<Result<PreparedFamilyTaskRoomClaim, CollabDbError>, sqlx::Error> {
    if writer_owner.is_nil() || writer_owner == guard.owner_token {
        return Err(sqlx::Error::Protocol(
            "Task writer requires a distinct stable activation owner".into(),
        ));
    }
    let mut tx = backend.begin_write().await?;
    let result=async {
        let mut op=tx.operation();
        let mut native=match op.load_collab_native(CollabKind::Task,guard.workspace_id,actor,credential,guard.task_id,NativeLoadMode::Writer).await? {
            Ok(native)=>native,Err(error)=>return Ok(Err(error)),
        };
        let mut activated=guard;
        activated.owner_token=writer_owner;
        if !op.verify_family_task_room_fence(activated).await? {
            if !op.verify_family_task_room_fence(guard).await? { return Ok(Err(CollabDbError::StaleWriter)); }
            let OperationTx::SqliteFamily(family)=&mut op else { return Err(sqlx::Error::Protocol("Task activation requires family writer".into())); };
            let now=family_room_now(family).await?;
            let changed=family.execute("UPDATE task_collab_room_fences SET owner_token=?5 WHERE workspace_id=?1 AND task_id=?2 AND owner_token=?3 AND fence=?4 AND expires_at>?6",&[Cell::uuid(guard.workspace_id),Cell::uuid(guard.task_id),Cell::uuid(guard.owner_token),Cell::Integer(guard.fence),Cell::uuid(writer_owner),Cell::Integer(now)]).await?;
            if changed!=1 { return Ok(Err(CollabDbError::StaleWriter)); }
            let Some(generation)=op.bump_native_writer_generation(CollabTables::for_kind(CollabKind::Task),guard.workspace_id,guard.task_id).await? else { return Ok(Err(CollabDbError::NotFound)); };
            native.writer_generation=generation;native.load.writer_generation=generation;
        }
        if !op.verify_family_task_room_fence(activated).await? { return Ok(Err(CollabDbError::StaleWriter)); }
        Ok(Ok(PreparedFamilyTaskRoomClaim { fence:activated,native }))
    }.await;
    finish_family_task_room(tx, result).await
}

pub(crate) async fn renew_family_task_room(
    backend: &Backend,
    fence: FamilyTaskRoomFence,
    lease: std::time::Duration,
) -> Result<bool, sqlx::Error> {
    let mut tx = backend.begin_write().await?;
    let result = async {
        let mut op = tx.operation();
        op.set_tenant(fence.workspace_id).await?;
        Ok(Ok(op.renew_family_task_room_fence(fence, lease).await?))
    }
    .await;
    finish_family_task_room(tx, result)
        .await?
        .map_err(|error| sqlx::Error::AnyDriverError(Box::new(TaskRoomRefusal(error))))
}
pub(crate) async fn release_family_task_room(
    backend: &Backend,
    fence: FamilyTaskRoomFence,
) -> Result<bool, sqlx::Error> {
    let mut tx = backend.begin_write().await?;
    let result = async {
        let mut op = tx.operation();
        op.set_tenant(fence.workspace_id).await?;
        Ok(Ok(op.release_family_task_room_fence(fence).await?))
    }
    .await;
    finish_family_task_room(tx, result)
        .await?
        .map_err(|error| sqlx::Error::AnyDriverError(Box::new(TaskRoomRefusal(error))))
}

pub(crate) async fn abandon_family_task_room_start(
    backend: &Backend,
    workspace: Uuid,
    task: Uuid,
    owner: Uuid,
) -> Result<(), sqlx::Error> {
    let mut tx = backend.begin_write().await?;
    let result=async {
        let mut op=tx.operation();op.set_tenant(workspace).await?;
        let OperationTx::SqliteFamily(family)=&mut op else { return Err(sqlx::Error::Protocol("Task startup cleanup requires family writer".into())); };
        let rows=family.query("SELECT fence FROM task_collab_room_fences WHERE workspace_id=?1 AND task_id=?2 AND owner_token=?3",&[Cell::uuid(workspace),Cell::uuid(task),Cell::uuid(owner)]).await?;
        if let Some(row)=rows.first() {
            let sequence=row.cell(0)?.integer()?;
            if sequence<=0 { return Err(sqlx::Error::Protocol("invalid Task startup fence".into())); }
            let fence=FamilyTaskRoomFence {workspace_id:workspace,task_id:task,owner_token:owner,fence:sequence};
            if !op.release_family_task_room_fence(fence).await? { return Err(sqlx::Error::Protocol("Task startup cleanup scope changed".into())); }
        }
        Ok(Ok(()))
    }.await;
    finish_family_task_room(tx, result)
        .await?
        .map_err(|error| sqlx::Error::AnyDriverError(Box::new(TaskRoomRefusal(error))))
}

pub(crate) async fn append_family_task_room_update_timed(
    backend: &Backend,
    fence: FamilyTaskRoomFence,
    input: AppendCollabInput<'_>,
) -> Result<
    (
        Result<AppendCollabResult, CollabDbError>,
        CollabDbStageTimings,
    ),
    sqlx::Error,
> {
    if matches!(backend, Backend::Postgres(_)) {
        return Err(sqlx::Error::Protocol(
            "PG Task append retains detached session guard".into(),
        ));
    }
    if input.payload.is_empty() || input.payload.len() > MAX_COLLAB_UPDATE_BYTES {
        return Ok((
            Err(CollabDbError::PayloadTooLarge),
            CollabDbStageTimings::default(),
        ));
    }
    let started = Instant::now();
    let tx = backend.begin_write().await?;
    let timings = CollabDbStageTimings {
        pool_wait_us: started.elapsed().as_micros() as u64,
        ..Default::default()
    };
    append_collab_update_in_tx(
        tx,
        CollabKind::Task,
        input,
        timings,
        None,
        Some(FamilyNativeRoomFence::Task(fence)),
    )
    .await
    .map(|(result, timings)| (result.map(|(append, _)| append), timings))
}

pub(crate) async fn abandon_family_native_room_start(
    backend: &Backend,
    kind: CollabKind,
    workspace: Uuid,
    resource: Uuid,
    owner: Uuid,
) -> Result<(), sqlx::Error> {
    match kind {
        CollabKind::Document => {
            abandon_family_document_room_start(backend, workspace, resource, owner).await
        }
        CollabKind::Task => {
            abandon_family_task_room_start(backend, workspace, resource, owner).await
        }
    }
}

#[cfg(all(test, feature = "db-tests"))]
mod selected_on_task_room_tests {
    use super::*;
    use crate::db::attachment_preview::tests::Fixture;
    use crate::db::tasks::selected_task_detail_tests::setup;

    async fn state(f: &Fixture, task: Uuid) -> (Option<i64>, Option<i64>, i64, i64, i64, i64) {
        sqlx::query_as("SELECT (SELECT writer_generation FROM task_states WHERE workspace_id=?1 AND task_id=?2),(SELECT tail_seq FROM task_states WHERE workspace_id=?1 AND task_id=?2),(SELECT count(*) FROM task_collab_updates WHERE workspace_id=?1 AND task_id=?2),(SELECT count(*) FROM task_collab_op_receipts WHERE workspace_id=?1 AND task_id=?2),(SELECT count(*) FROM revisions WHERE workspace_id=?1 AND target_kind='task' AND target_id=?2),(SELECT count(*) FROM task_collab_room_fences WHERE workspace_id=?1 AND task_id=?2)")
            .bind(f.workspace.as_bytes().as_slice()).bind(task.as_bytes().as_slice()).fetch_one(&f.pool).await.unwrap()
    }
    async fn start(
        f: &Fixture,
        credential: Uuid,
        task: Uuid,
        owner: Uuid,
    ) -> FamilyNativeRoomClaim {
        acquire_family_native_room_for_start(
            &f.backend,
            (CollabKind::Task, f.workspace, task),
            (f.user, credential),
            owner,
            std::time::Duration::from_secs(60),
        )
        .await
        .unwrap()
        .unwrap()
    }
    fn task_fence(fence: FamilyNativeRoomFence) -> FamilyTaskRoomFence {
        let FamilyNativeRoomFence::Task(fence) = fence else {
            panic!("actual Task room lineage required")
        };
        fence
    }

    #[tokio::test]
    async fn on_task_reader_activation_wrong_native_proofs_expiry_and_healthy_successor() {
        let (f, credential, _, _, task) = setup().await;
        let owner = Uuid::now_v7();
        let reader = start(&f, credential, task, owner).await;
        assert_eq!(
            reader.native.writer_generation, 0,
            "reader startup never activates a writer"
        );
        let stable = reader.fence;
        let proof = FamilyNativeConsumerProof {
            room: stable,
            generation: 0,
            tail: 0,
        };
        verify_room_native_consumer(
            &f.backend,
            CollabKind::Task,
            f.workspace,
            f.user,
            credential,
            task,
            Some(proof),
        )
        .await
        .unwrap()
        .unwrap();
        let before = state(&f, task).await;
        let document = FamilyNativeRoomFence::Document(FamilyRoomFence {
            workspace_id: f.workspace,
            document_id: task,
            owner_token: owner,
            fence: stable.sequence(),
        });
        for wrong in [
            FamilyNativeConsumerProof {
                room: document,
                ..proof
            },
            FamilyNativeConsumerProof {
                room: stable.with_owner(Uuid::now_v7()),
                ..proof
            },
            FamilyNativeConsumerProof {
                generation: 1,
                ..proof
            },
            FamilyNativeConsumerProof { tail: 1, ..proof },
        ] {
            assert_eq!(
                verify_room_native_consumer(
                    &f.backend,
                    CollabKind::Task,
                    f.workspace,
                    f.user,
                    credential,
                    task,
                    Some(wrong)
                )
                .await
                .unwrap(),
                Err(CollabDbError::StaleWriter)
            );
            assert_eq!(state(&f, task).await, before);
        }
        assert_eq!(
            load_room_collab_readonly(
                &f.backend,
                CollabKind::Task,
                f.workspace,
                f.user,
                credential,
                task,
                Some(document)
            )
            .await
            .unwrap()
            .unwrap_err(),
            CollabDbError::StaleWriter
        );
        let writer_owner = Uuid::now_v7();
        let active = activate_family_task_writer(
            &f.backend,
            task_fence(stable),
            f.user,
            credential,
            writer_owner,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(active.native.writer_generation, 1);
        let replay = activate_family_task_writer(
            &f.backend,
            task_fence(stable),
            f.user,
            credential,
            writer_owner,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            replay.native.writer_generation, 1,
            "same logical activation cannot advance generation twice"
        );
        assert_eq!(replay.fence, active.fence);
        sqlx::query(
            "UPDATE task_collab_room_fences SET expires_at=1 WHERE workspace_id=?1 AND task_id=?2",
        )
        .bind(f.workspace.as_bytes().as_slice())
        .bind(task.as_bytes().as_slice())
        .execute(&f.pool)
        .await
        .unwrap();
        assert!(!renew_family_task_room(
            &f.backend,
            active.fence,
            std::time::Duration::from_secs(60)
        )
        .await
        .unwrap());
        assert!(matches!(
            activate_family_task_writer(
                &f.backend,
                task_fence(stable),
                f.user,
                credential,
                writer_owner
            )
            .await
            .unwrap(),
            Err(CollabDbError::StaleWriter)
        ));
        let next = start(&f, credential, task, Uuid::now_v7()).await;
        assert!(next.fence.sequence() > stable.sequence());
        assert!(!release_family_task_room(&f.backend, active.fence)
            .await
            .unwrap());
        let next = activate_family_task_writer(
            &f.backend,
            task_fence(next.fence),
            f.user,
            credential,
            Uuid::now_v7(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(next.native.writer_generation, 2);
        let mut check = f.backend.begin_write().await.unwrap();
        check.operation().set_tenant(f.workspace).await.unwrap();
        assert!(check
            .operation()
            .verify_family_task_room_fence(next.fence)
            .await
            .unwrap());
        check.commit_with_cleanup().await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("PRAGMA foreign_keys")
                .fetch_one(&f.pool)
                .await
                .unwrap(),
            1
        );
        f.close().await;
    }

    #[tokio::test]
    async fn on_task_original_deferred_fk_commit_error_no_ack_then_healthy_start() {
        let (f, credential, _, _, task) = setup().await;
        let before = state(&f, task).await;
        let counter: i64 =
            sqlx::query_scalar("SELECT next_fence FROM collab_fence_counter WHERE id=1")
                .fetch_one(&f.pool)
                .await
                .unwrap();
        sqlx::query("CREATE TABLE on_task_deferred_probe(workspace_id BLOB REFERENCES workspaces(id) DEFERRABLE INITIALLY DEFERRED)").execute(&f.pool).await.unwrap();
        sqlx::query("CREATE TRIGGER reject_on_task_start AFTER INSERT ON task_collab_room_fences BEGIN INSERT INTO on_task_deferred_probe VALUES(zeroblob(16)); END;").execute(&f.pool).await.unwrap();
        let error = match acquire_family_native_room_for_start(
            &f.backend,
            (CollabKind::Task, f.workspace, task),
            (f.user, credential),
            Uuid::now_v7(),
            std::time::Duration::from_secs(60),
        )
        .await
        {
            Err(error) => error,
            Ok(_) => panic!("actual original writer COMMIT must fail, never issue ownership ACK"),
        };
        let FamilyRoomStartError::Commit {
            source,
            settlement: FamilyRoomStartSettlement::LocalSqlx,
        } = error
        else {
            panic!("failure must originate at actual deferred COMMIT")
        };
        let sqlx::Error::AnyDriverError(error) = source else {
            panic!("original typed cleanup receipt required")
        };
        let original = error
            .downcast_ref::<crate::db::backend::CommitCleanupUnknown>()
            .expect("no stripped original COMMIT metadata");
        assert!(original
            .source
            .source
            .as_database_error()
            .unwrap()
            .is_foreign_key_violation());
        assert!(original.permits_reconciliation());
        assert_eq!(state(&f, task).await, before);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT next_fence FROM collab_fence_counter WHERE id=1")
                .fetch_one(&f.pool)
                .await
                .unwrap(),
            counter
        );
        sqlx::query("DROP TRIGGER reject_on_task_start")
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("DROP TABLE on_task_deferred_probe")
            .execute(&f.pool)
            .await
            .unwrap();
        let healthy = start(&f, credential, task, Uuid::now_v7()).await;
        assert_eq!(healthy.native.writer_generation, 0);
        assert_eq!(healthy.fence.sequence(), counter);
        f.close().await;
    }

    #[tokio::test]
    async fn on_task_cancel_borrowed_start_before_commit_and_lost_reply_keeps_stable_owner() {
        let (f, credential, _, _, task) = setup().await;
        let before = state(&f, task).await;
        let backend = f.backend.clone();
        let workspace = f.workspace;
        let actor = f.user;
        let owner = Uuid::now_v7();
        let (send, reached) = tokio::sync::oneshot::channel();
        let pending = tokio::spawn(async move {
            let mut tx = backend.begin_write().await.unwrap();
            tx.operation()
                .prepare_family_task_room_claim(
                    (workspace, task),
                    (actor, credential),
                    owner,
                    std::time::Duration::from_secs(60),
                    NativeLoadMode::Reader,
                )
                .await
                .unwrap()
                .unwrap();
            send.send(()).unwrap();
            std::future::pending::<()>().await;
            tx.commit_with_cleanup().await.unwrap();
        });
        reached.await.unwrap();
        pending.abort();
        assert!(pending.await.unwrap_err().is_cancelled());
        assert_eq!(
            state(&f, task).await,
            before,
            "actual SQLite rollback queue precedes next connection read"
        );
        let owner = Uuid::now_v7();
        let (committed, release) = arm_family_room_start_reply_fault(task, true).await;
        let backend = f.backend.clone();
        let pending = tokio::spawn(async move {
            acquire_family_native_room_for_start(
                &backend,
                (CollabKind::Task, workspace, task),
                (actor, credential),
                owner,
                std::time::Duration::from_secs(60),
            )
            .await
        });
        committed.await.unwrap();
        release.send(()).unwrap();
        let result = pending.await.unwrap();
        assert!(
            matches!(
                result,
                Err(FamilyRoomStartError::Commit {
                    settlement: FamilyRoomStartSettlement::LocalSqlx,
                    ..
                })
            ),
            "actual local lost reply is uncertainty, not ownership success"
        );
        let same = start(&f, credential, task, owner).await;
        let replay = start(&f, credential, task, owner).await;
        assert_eq!(same.fence, replay.fence);
        assert_eq!(same.native.writer_generation, 0);
        assert_eq!(state(&f, task).await, (Some(0), Some(0), 0, 0, 0, 1));
        assert!(release_family_task_room(&f.backend, task_fence(same.fence))
            .await
            .unwrap());
        let successor = start(&f, credential, task, Uuid::now_v7()).await;
        assert!(successor.fence.sequence() > same.fence.sequence());
        f.close().await;
    }

    #[tokio::test]
    async fn on_task_native_append_revision_wrong_scope_current_revocation_and_new_client_history()
    {
        let (f, credential, _, _, task) = setup().await;
        let reader = start(&f, credential, task, Uuid::now_v7()).await;
        let writer = activate_family_task_writer(
            &f.backend,
            task_fence(reader.fence),
            f.user,
            credential,
            Uuid::now_v7(),
        )
        .await
        .unwrap()
        .unwrap();
        let fence = FamilyNativeRoomFence::Task(writer.fence);
        let engine = crate::collab::CollabConfig::from_env()
            .expect("root-qualified current native engine is required, never skip");
        let json = serde_json::json!({"type":"doc","content":[{"type":"paragraph","attrs":{"id":"on-task-native-block"},"content":[{"type":"text","text":"actual ON Task 😀","marks":[{"type":"bold"}]}]}]});
        let update = crate::collab::seed::SeedEngine::new(engine.engine_bin.clone(), engine.limits)
            .tiptap_to_yjs_update(&json)
            .await
            .unwrap();
        let before = state(&f, task).await;
        for (proof, generation, tail) in [
            (
                FamilyNativeRoomFence::Document(FamilyRoomFence {
                    workspace_id: f.workspace,
                    document_id: task,
                    owner_token: fence.owner(),
                    fence: fence.sequence(),
                }),
                1,
                0,
            ),
            (fence.with_owner(Uuid::now_v7()), 1, 0),
            (fence, 2, 0),
            (fence, 1, 1),
        ] {
            let mut tx = f.backend.begin_write().await.unwrap();
            let result = tx
                .operation()
                .append_collab_native(
                    CollabKind::Task,
                    AppendCollabInput {
                        workspace_id: f.workspace,
                        actor_user_id: f.user,
                        session_id: credential,
                        document_id: task,
                        writer_generation: generation,
                        expected_tail_seq: tail,
                        op_id: Uuid::now_v7(),
                        payload: &update,
                        client_ip: None,
                    },
                    CollabDbStageTimings::default(),
                    None,
                    Some(proof),
                )
                .await
                .unwrap()
                .0;
            assert!(matches!(
                result,
                Err(CollabDbError::StaleWriter | CollabDbError::StaleCutoff)
            ));
            tx.rollback().await.unwrap();
            assert_eq!(state(&f, task).await, before);
        }
        let operation = Uuid::now_v7();
        // A genuine native update must not be acknowledged when its receipt
        // insert succeeds but this original writer's deferred FK COMMIT fails.
        sqlx::query("CREATE TABLE on_task_append_probe(workspace_id BLOB REFERENCES workspaces(id) DEFERRABLE INITIALLY DEFERRED)").execute(&f.pool).await.unwrap();
        sqlx::query("CREATE TRIGGER reject_on_task_append AFTER INSERT ON task_collab_op_receipts BEGIN INSERT INTO on_task_append_probe VALUES(zeroblob(16)); END;").execute(&f.pool).await.unwrap();
        let error = append_family_task_room_update_timed(
            &f.backend,
            writer.fence,
            AppendCollabInput {
                workspace_id: f.workspace,
                actor_user_id: f.user,
                session_id: credential,
                document_id: task,
                writer_generation: 1,
                expected_tail_seq: 0,
                op_id: operation,
                payload: &update,
                client_ip: None,
            },
        )
        .await
        .unwrap_err();
        let sqlx::Error::AnyDriverError(error) = error else {
            panic!("typed original native COMMIT failure required");
        };
        let unknown = error
            .downcast_ref::<crate::db::backend::CommitCleanupUnknown>()
            .unwrap();
        assert!(unknown
            .source
            .source
            .as_database_error()
            .unwrap()
            .is_foreign_key_violation());
        assert!(unknown.permits_reconciliation());
        assert_eq!(
            state(&f, task).await,
            before,
            "failed COMMIT produces no tail, operation receipt or history"
        );
        sqlx::query("DROP TRIGGER reject_on_task_append")
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("DROP TABLE on_task_append_probe")
            .execute(&f.pool)
            .await
            .unwrap();
        // Current credential is checked again after the real native producer.
        sqlx::query("UPDATE sessions SET revoked_at=1 WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(append_family_task_room_update_timed(
            &f.backend,
            writer.fence,
            AppendCollabInput {
                workspace_id: f.workspace,
                actor_user_id: f.user,
                session_id: credential,
                document_id: task,
                writer_generation: 1,
                expected_tail_seq: 0,
                op_id: operation,
                payload: &update,
                client_ip: None,
            },
        )
        .await
        .unwrap()
        .0
        .is_err());
        assert_eq!(state(&f, task).await, before);
        sqlx::query("UPDATE sessions SET revoked_at=NULL WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        // The same logical operation now makes one healthy confirmed append.
        let result = append_family_task_room_update_timed(
            &f.backend,
            writer.fence,
            AppendCollabInput {
                workspace_id: f.workspace,
                actor_user_id: f.user,
                session_id: credential,
                document_id: task,
                writer_generation: 1,
                expected_tail_seq: 0,
                op_id: operation,
                payload: &update,
                client_ip: None,
            },
        )
        .await
        .unwrap()
        .0
        .unwrap();
        assert_eq!(result, AppendCollabResult::Committed { seq: 1 });
        let load = load_room_collab_readonly(
            &f.backend,
            CollabKind::Task,
            f.workspace,
            f.user,
            credential,
            task,
            Some(fence),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(load.tail_seq, 1);
        assert_eq!(load.tail[0].payload, update);
        let captured = tokio::task::spawn_blocking(move || {
            crate::collab::revision::capture_revision_offline(
                engine.engine_bin,
                engine.limits,
                load.snapshot,
                load.tail.into_iter().map(|row| row.payload).collect(),
            )
        })
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            captured.content_json, json,
            "fresh maintained native reader preserves literal IDs, marks and text"
        );
        let prepared =
            crate::collab::derived_body::prepare_derived_body(captured.content_json.clone())
                .unwrap();
        project_derived_body_in_room(
            &f.backend,
            CollabKind::Task,
            ProjectDerivedBodyInput::new(f.workspace, f.user, credential, task, 1, 1, prepared),
            Some(fence),
        )
        .await
        .unwrap()
        .unwrap();
        let proof = FamilyNativeConsumerProof {
            room: fence,
            generation: 1,
            tail: 1,
        };
        let input = crate::db::revisions::CreateRevisionInput {
            text: crate::collab::revision::prepare_revision_text(&captured.content_json).unwrap(),
            content_json: captured.content_json,
            y_snapshot: captured.y_snapshot,
            reason: "manual".into(),
        };
        let saved = crate::db::revisions::create_manual_revision_with_room_proof(
            &f.backend,
            f.workspace,
            f.user,
            credential,
            crate::db::revisions::RevisionTarget::Task(task).into(),
            input.clone(),
            Some(proof),
        )
        .await
        .unwrap()
        .unwrap();
        let before = state(&f, task).await;
        assert_eq!(before, (Some(1), Some(1), 1, 1, 1, 1));
        assert!(!saved.is_nil());
        let mut wrong = proof;
        wrong.room = FamilyNativeRoomFence::Document(FamilyRoomFence {
            workspace_id: f.workspace,
            document_id: task,
            owner_token: fence.owner(),
            fence: fence.sequence(),
        });
        assert!(
            crate::db::revisions::create_manual_revision_with_room_proof(
                &f.backend,
                f.workspace,
                f.user,
                credential,
                crate::db::revisions::RevisionTarget::Task(task).into(),
                input.clone(),
                Some(wrong)
            )
            .await
            .unwrap()
            .is_err()
        );
        assert_eq!(state(&f, task).await, before);
        // Revoke current authority after actual native capture, before publication/replay.
        sqlx::query("UPDATE sessions SET revoked_at=1 WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(verify_room_native_consumer(
            &f.backend,
            CollabKind::Task,
            f.workspace,
            f.user,
            credential,
            task,
            Some(proof)
        )
        .await
        .unwrap()
        .is_err());
        assert!(
            crate::db::revisions::create_manual_revision_with_room_proof(
                &f.backend,
                f.workspace,
                f.user,
                credential,
                crate::db::revisions::RevisionTarget::Task(task).into(),
                input,
                Some(proof)
            )
            .await
            .unwrap()
            .is_err()
        );
        assert_eq!(state(&f, task).await, before);
        sqlx::query("UPDATE sessions SET revoked_at=NULL WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let new_client = crate::db::pool::connect_sqlite_app(&f.path, 1)
            .await
            .unwrap();
        let body: String =
            sqlx::query_scalar("SELECT content_json FROM tasks WHERE workspace_id=?1 AND id=?2")
                .bind(f.workspace.as_bytes().as_slice())
                .bind(task.as_bytes().as_slice())
                .fetch_one(&new_client)
                .await
                .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&body).unwrap(),
            json
        );
        new_client.close().await;
        verify_room_native_consumer(
            &f.backend,
            CollabKind::Task,
            f.workspace,
            f.user,
            credential,
            task,
            Some(proof),
        )
        .await
        .unwrap()
        .unwrap();
        f.close().await;
    }
}
