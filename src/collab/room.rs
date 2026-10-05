use std::collections::{HashMap, VecDeque};
use std::convert::Infallible;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use futures_util::future::FutureExt;
use futures_util::stream::{FuturesUnordered, StreamExt};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::MissedTickBehavior;

use collab_engine::b64;
use collab_engine::outcome::EngineStatus;
use collab_engine::outcome::LimitKind;
use collab_engine::process::is_slot_cap_refusal;
use collab_engine::protocol::Request;
use sha2::{Digest, Sha256};
use sqlx::postgres::PgPool;
use tokio::sync::{mpsc, oneshot, watch};
use uuid::Uuid;

use crate::collab::admission::{warn_join_db_error, MemoryReservation};
use crate::collab::awareness::{decode_awareness, AwarenessRegistry};
use crate::collab::config::CollabConfig;
use crate::collab::derived_body::prepare_derived_body;
use crate::collab::engine_bridge::{
    warn_engine_not_applied, BridgeError, EngineBridge, RecycleError,
};
use crate::collab::guard::{BackendRoomGuard, RoomGuard};
use crate::collab::revision::{capture_revision_offline, prepare_revision_text};
use crate::collab::validation::{
    classify_admission_load, validate_recovery_bundle, validate_snapshot_only, BundleValidation,
    ValidateStageTimings,
};
use crate::db::backend::Backend;
use crate::db::revisions::{
    create_system_revision_for_room_backend, latest_revision_y_snapshot_for_room_backend,
    load_durable_collab_for_room_backend, lookup_restored_revision, CreateRevisionInput,
    RestoreRevisionAppend, RestoreRevisionInput, RevisionDbError, RevisionTarget,
    SystemRevisionHead, SYSTEM_REVISION_HEAD_RETRIES,
};

const MAX_REJECTED_CANDIDATES_PER_USER: usize = 8;
const REJECTED_CANDIDATE_WINDOW: Duration = Duration::from_secs(30);
const USER_REJECT_COOLDOWN: Duration = Duration::from_secs(30);

struct UserRejectBudget {
    rejects: VecDeque<Instant>,
    cooldown_until: Option<Instant>,
}
use crate::collab::wire::CollabKind;
use crate::collab::wire::{encode, AuthMessage, DocumentMessage, SyncStep, WireFrame};
use crate::collab::y_sync::{encode_sync_payload, is_empty_update, parse_sync_payload};
use crate::db::collab::{
    append_collab_restore_kind,
    compact_collab_snapshot_in_room as compact_collab_snapshot_kind_backend,
    load_room_collab_readonly, project_derived_body_in_room as project_derived_body_kind_backend,
    resolve_collab_admission_kind_backend, verify_room_collab_operation,
    verify_room_native_consumer, AppendCollabInput, AppendCollabResult, ClaimWriterResult,
    CollabDbError, CompactCollabInput, FamilyNativeConsumerProof, FamilyNativeRoomFence,
    FamilyRoomDeliveryFence, ProjectDerivedBodyInput, ProjectDerivedBodyResult, VerifyCollabInput,
};
use crate::db::collab_delivery::{check_delivery_admission_kind_backend, DeliveryAdmission};
use crate::db::identity::LiveSession;

#[cfg(feature = "db-tests")]
static SPAWN_ROOM_BLOCKS: std::sync::LazyLock<
    tokio::sync::Mutex<HashMap<Uuid, tokio::sync::oneshot::Receiver<()>>>,
> = std::sync::LazyLock::new(|| tokio::sync::Mutex::new(HashMap::new()));

#[cfg(feature = "db-tests")]
pub async fn arm_spawn_room_block(document_id: Uuid) -> tokio::sync::oneshot::Sender<()> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    assert!(SPAWN_ROOM_BLOCKS
        .lock()
        .await
        .insert(document_id, rx)
        .is_none());
    tx
}

#[cfg(feature = "db-tests")]
pub async fn disarm_spawn_room_block(document_id: Uuid) {
    SPAWN_ROOM_BLOCKS.lock().await.remove(&document_id);
}

/// Whether a start armed with [`arm_spawn_room_block`] has reached the block
/// (it was admitted and now waits before its bridge exists).
#[cfg(feature = "db-tests")]
pub async fn spawn_room_block_reached(document_id: Uuid) -> bool {
    !SPAWN_ROOM_BLOCKS.lock().await.contains_key(&document_id)
}

#[cfg(feature = "db-tests")]
static JOIN_BARRIERS: std::sync::LazyLock<tokio::sync::Mutex<HashMap<Uuid, AppendRevokeBarrier>>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(HashMap::new()));

#[cfg(feature = "db-tests")]
pub async fn arm_join_barrier(document_id: Uuid) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
    let (reached_tx, reached_rx) = oneshot::channel();
    let (proceed_tx, proceed_rx) = oneshot::channel();
    JOIN_BARRIERS.lock().await.insert(
        document_id,
        AppendRevokeBarrier {
            reached_tx,
            proceed_rx,
        },
    );
    (reached_rx, proceed_tx)
}

#[cfg(feature = "db-tests")]
pub async fn disarm_join_barrier(document_id: Uuid) {
    JOIN_BARRIERS.lock().await.remove(&document_id);
}

#[cfg(feature = "db-tests")]
static ACTOR_PANIC_AFTER_JOIN_BARRIER: std::sync::LazyLock<
    tokio::sync::Mutex<std::collections::HashSet<Uuid>>,
> = std::sync::LazyLock::new(|| tokio::sync::Mutex::new(std::collections::HashSet::new()));

#[cfg(feature = "db-tests")]
pub async fn arm_actor_panic_after_join_barrier(document_id: Uuid) {
    ACTOR_PANIC_AFTER_JOIN_BARRIER
        .lock()
        .await
        .insert(document_id);
}

#[cfg(feature = "db-tests")]
pub async fn disarm_actor_panic_after_join_barrier(document_id: Uuid) {
    ACTOR_PANIC_AFTER_JOIN_BARRIER
        .lock()
        .await
        .remove(&document_id);
}

#[cfg(feature = "db-tests")]
async fn consume_actor_panic_after_join_barrier(document_id: Uuid) -> bool {
    ACTOR_PANIC_AFTER_JOIN_BARRIER
        .lock()
        .await
        .remove(&document_id)
}

#[cfg(feature = "db-tests")]
async fn pause_for_join_barrier(document_id: Uuid) {
    let barrier = JOIN_BARRIERS.lock().await.remove(&document_id);
    if let Some(barrier) = barrier {
        let _ = barrier.reached_tx.send(());
        let _ = barrier.proceed_rx.await;
        if consume_actor_panic_after_join_barrier(document_id).await {
            panic!("db-tests collab actor panic after join barrier");
        }
    }
}

/// Pause after isolated native work or capture, before its final authority proof.
/// A fixture mutates the actual database fence while this await is suspended.
#[cfg(feature = "db-tests")]
static NATIVE_CONSUMER_BARRIERS: std::sync::LazyLock<
    tokio::sync::Mutex<HashMap<(Uuid, u8), AppendRevokeBarrier>>,
> = std::sync::LazyLock::new(|| tokio::sync::Mutex::new(HashMap::new()));
#[cfg(feature = "db-tests")]
pub const NATIVE_CAPTURE_FINAL_PROOF: u8 = 0;
#[cfg(feature = "db-tests")]
pub const NATIVE_PROJECT_FINAL_PROOF: u8 = 1;
#[cfg(feature = "db-tests")]
pub const MANUAL_REVISION_BEFORE_WRITE: u8 = 2;
#[cfg(feature = "db-tests")]
pub const NATIVE_FORWARD_FINAL_PROOF: u8 = 3;
#[cfg(feature = "db-tests")]
pub async fn arm_native_consumer_barrier(
    document: Uuid,
    point: u8,
) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
    let (reached_tx, reached_rx) = oneshot::channel();
    let (proceed_tx, proceed_rx) = oneshot::channel();
    assert!(NATIVE_CONSUMER_BARRIERS
        .lock()
        .await
        .insert(
            (document, point),
            AppendRevokeBarrier {
                reached_tx,
                proceed_rx
            }
        )
        .is_none());
    (reached_rx, proceed_tx)
}
#[cfg(feature = "db-tests")]
pub(crate) async fn pause_native_consumer_barrier(document: Uuid, point: u8) {
    let barrier = NATIVE_CONSUMER_BARRIERS
        .lock()
        .await
        .remove(&(document, point));
    if let Some(barrier) = barrier {
        let _ = barrier.reached_tx.send(());
        let _ = barrier.proceed_rx.await;
    }
}

// Choose the legal lease-first select outcome deterministically after an actor barrier.
#[cfg(feature = "db-tests")]
static LEASE_DROP_PRIORITY: std::sync::LazyLock<
    tokio::sync::Mutex<std::collections::HashSet<Uuid>>,
> = std::sync::LazyLock::new(|| tokio::sync::Mutex::new(std::collections::HashSet::new()));

#[cfg(feature = "db-tests")]
pub async fn arm_lease_drop_priority(document_id: Uuid) {
    assert!(LEASE_DROP_PRIORITY.lock().await.insert(document_id));
}

#[cfg(feature = "db-tests")]
async fn consume_lease_drop_priority(document_id: Uuid) -> bool {
    LEASE_DROP_PRIORITY.lock().await.remove(&document_id)
}

#[cfg(feature = "db-tests")]
static JOIN_REPLY_BARRIERS: std::sync::LazyLock<
    tokio::sync::Mutex<HashMap<Uuid, AppendRevokeBarrier>>,
> = std::sync::LazyLock::new(|| tokio::sync::Mutex::new(HashMap::new()));

#[cfg(feature = "db-tests")]
pub async fn arm_join_reply_barrier(conn_id: Uuid) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
    let (reached_tx, reached_rx) = oneshot::channel();
    let (proceed_tx, proceed_rx) = oneshot::channel();
    assert!(JOIN_REPLY_BARRIERS
        .lock()
        .await
        .insert(
            conn_id,
            AppendRevokeBarrier {
                reached_tx,
                proceed_rx,
            }
        )
        .is_none());
    (reached_rx, proceed_tx)
}

#[cfg(feature = "db-tests")]
async fn pause_before_join_reply(conn_id: Uuid) {
    let barrier = JOIN_REPLY_BARRIERS.lock().await.remove(&conn_id);
    if let Some(barrier) = barrier {
        let _ = barrier.reached_tx.send(());
        let _ = barrier.proceed_rx.await;
    }
}

#[cfg(feature = "db-tests")]
static FORCE_PRIMARY_APPLY_FAIL: std::sync::LazyLock<
    tokio::sync::Mutex<std::collections::HashSet<Uuid>>,
> = std::sync::LazyLock::new(|| tokio::sync::Mutex::new(std::collections::HashSet::new()));

#[cfg(feature = "db-tests")]
static FORCE_PRIMARY_LOAD_FAIL: std::sync::LazyLock<
    tokio::sync::Mutex<std::collections::HashSet<Uuid>>,
> = std::sync::LazyLock::new(|| tokio::sync::Mutex::new(std::collections::HashSet::new()));

#[cfg(feature = "db-tests")]
static FORCE_PRIMARY_LOAD_FAIL_AFTER: std::sync::LazyLock<
    tokio::sync::Mutex<std::collections::HashMap<Uuid, u32>>,
> = std::sync::LazyLock::new(|| tokio::sync::Mutex::new(std::collections::HashMap::new()));

#[cfg(feature = "db-tests")]
static FORCE_PRIMARY_LOAD_FAIL_COUNT: std::sync::LazyLock<
    tokio::sync::Mutex<std::collections::HashMap<Uuid, u32>>,
> = std::sync::LazyLock::new(|| tokio::sync::Mutex::new(std::collections::HashMap::new()));

#[cfg(feature = "db-tests")]
static JOIN_CATCHUP_PROJECTION_ATTEMPTS: std::sync::LazyLock<
    tokio::sync::Mutex<std::collections::HashMap<Uuid, u32>>,
> = std::sync::LazyLock::new(|| tokio::sync::Mutex::new(std::collections::HashMap::new()));

#[cfg(feature = "db-tests")]
pub async fn arm_force_primary_apply_fail(document_id: Uuid) {
    FORCE_PRIMARY_APPLY_FAIL.lock().await.insert(document_id);
}

#[cfg(feature = "db-tests")]
pub async fn disarm_force_primary_apply_fail(document_id: Uuid) {
    FORCE_PRIMARY_APPLY_FAIL.lock().await.remove(&document_id);
}

#[cfg(feature = "db-tests")]
pub async fn arm_force_primary_load_fail(document_id: Uuid) {
    FORCE_PRIMARY_LOAD_FAIL.lock().await.insert(document_id);
}

#[cfg(feature = "db-tests")]
pub async fn disarm_force_primary_load_fail(document_id: Uuid) {
    FORCE_PRIMARY_LOAD_FAIL.lock().await.remove(&document_id);
}

#[cfg(feature = "db-tests")]
pub async fn arm_force_primary_load_fail_after(document_id: Uuid, after: u32) {
    FORCE_PRIMARY_LOAD_FAIL_AFTER
        .lock()
        .await
        .insert(document_id, after);
    FORCE_PRIMARY_LOAD_FAIL_COUNT
        .lock()
        .await
        .insert(document_id, 0);
}

#[cfg(feature = "db-tests")]
pub async fn disarm_force_primary_load_fail_after(document_id: Uuid) {
    FORCE_PRIMARY_LOAD_FAIL_AFTER
        .lock()
        .await
        .remove(&document_id);
    FORCE_PRIMARY_LOAD_FAIL_COUNT
        .lock()
        .await
        .remove(&document_id);
}

#[cfg(feature = "db-tests")]
pub async fn test_primary_load_attempt_count(document_id: Uuid) -> u32 {
    FORCE_PRIMARY_LOAD_FAIL_COUNT
        .lock()
        .await
        .get(&document_id)
        .copied()
        .unwrap_or(0)
}

#[cfg(feature = "db-tests")]
pub async fn test_join_catchup_projection_attempt_count(document_id: Uuid) -> u32 {
    JOIN_CATCHUP_PROJECTION_ATTEMPTS
        .lock()
        .await
        .get(&document_id)
        .copied()
        .unwrap_or(0)
}

#[cfg(feature = "db-tests")]
async fn consume_force_primary_apply_fail(document_id: Uuid) -> bool {
    FORCE_PRIMARY_APPLY_FAIL.lock().await.remove(&document_id)
}

#[cfg(feature = "db-tests")]
async fn should_fail_primary_load(document_id: Uuid) -> bool {
    if FORCE_PRIMARY_LOAD_FAIL.lock().await.contains(&document_id) {
        return true;
    }
    let after = FORCE_PRIMARY_LOAD_FAIL_AFTER
        .lock()
        .await
        .get(&document_id)
        .copied();
    if let Some(threshold) = after {
        let mut counts = FORCE_PRIMARY_LOAD_FAIL_COUNT.lock().await;
        let count = counts.entry(document_id).or_insert(0);
        *count += 1;
        return *count >= threshold;
    }
    false
}

#[cfg(feature = "db-tests")]
struct AppendRevokeBarrier {
    reached_tx: oneshot::Sender<()>,
    proceed_rx: oneshot::Receiver<()>,
}

#[cfg(feature = "db-tests")]
static APPEND_REVOKE_BARRIERS: std::sync::LazyLock<
    tokio::sync::Mutex<HashMap<Uuid, AppendRevokeBarrier>>,
> = std::sync::LazyLock::new(|| tokio::sync::Mutex::new(HashMap::new()));

#[cfg(feature = "db-tests")]
pub async fn arm_append_revoke_barrier(
    document_id: Uuid,
) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
    let (reached_tx, reached_rx) = oneshot::channel();
    let (proceed_tx, proceed_rx) = oneshot::channel();
    APPEND_REVOKE_BARRIERS.lock().await.insert(
        document_id,
        AppendRevokeBarrier {
            reached_tx,
            proceed_rx,
        },
    );
    (reached_rx, proceed_tx)
}

#[cfg(feature = "db-tests")]
pub async fn disarm_append_revoke_barrier(document_id: Uuid) {
    APPEND_REVOKE_BARRIERS.lock().await.remove(&document_id);
}

#[cfg(feature = "db-tests")]
async fn pause_for_append_revoke_barrier(document_id: Uuid) {
    let barrier = APPEND_REVOKE_BARRIERS.lock().await.remove(&document_id);
    if let Some(barrier) = barrier {
        let _ = barrier.reached_tx.send(());
        let _ = barrier.proceed_rx.await;
    }
}

#[cfg(feature = "db-tests")]
static APPEND_IN_TX_REJECT_BARRIERS: std::sync::LazyLock<
    tokio::sync::Mutex<HashMap<Uuid, AppendRevokeBarrier>>,
> = std::sync::LazyLock::new(|| tokio::sync::Mutex::new(HashMap::new()));

#[cfg(feature = "db-tests")]
pub async fn arm_append_in_tx_reject_barrier(
    document_id: Uuid,
) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
    let (reached_tx, reached_rx) = oneshot::channel();
    let (proceed_tx, proceed_rx) = oneshot::channel();
    APPEND_IN_TX_REJECT_BARRIERS.lock().await.insert(
        document_id,
        AppendRevokeBarrier {
            reached_tx,
            proceed_rx,
        },
    );
    (reached_rx, proceed_tx)
}

#[cfg(feature = "db-tests")]
pub async fn disarm_append_in_tx_reject_barrier(document_id: Uuid) {
    APPEND_IN_TX_REJECT_BARRIERS
        .lock()
        .await
        .remove(&document_id);
}

#[cfg(feature = "db-tests")]
async fn pause_for_append_in_tx_reject_barrier(document_id: Uuid) {
    let barrier = APPEND_IN_TX_REJECT_BARRIERS
        .lock()
        .await
        .remove(&document_id);
    if let Some(barrier) = barrier {
        let _ = barrier.reached_tx.send(());
        let _ = barrier.proceed_rx.await;
    }
}

#[cfg(feature = "db-tests")]
static APPEND_PROJECTION_BARRIERS: std::sync::LazyLock<
    tokio::sync::Mutex<HashMap<Uuid, AppendRevokeBarrier>>,
> = std::sync::LazyLock::new(|| tokio::sync::Mutex::new(HashMap::new()));

#[cfg(feature = "db-tests")]
pub async fn arm_append_projection_barrier(
    document_id: Uuid,
) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
    let (reached_tx, reached_rx) = oneshot::channel();
    let (proceed_tx, proceed_rx) = oneshot::channel();
    APPEND_PROJECTION_BARRIERS.lock().await.insert(
        document_id,
        AppendRevokeBarrier {
            reached_tx,
            proceed_rx,
        },
    );
    (reached_rx, proceed_tx)
}

#[cfg(feature = "db-tests")]
pub async fn disarm_append_projection_barrier(document_id: Uuid) {
    APPEND_PROJECTION_BARRIERS.lock().await.remove(&document_id);
}

#[cfg(feature = "db-tests")]
async fn pause_for_append_projection_barrier(document_id: Uuid) {
    let barrier = APPEND_PROJECTION_BARRIERS.lock().await.remove(&document_id);
    if let Some(barrier) = barrier {
        let _ = barrier.reached_tx.send(());
        let _ = barrier.proceed_rx.await;
    }
}

#[cfg(feature = "db-tests")]
static SESSION_REVISION_PERSIST_BARRIERS: std::sync::LazyLock<
    tokio::sync::Mutex<HashMap<Uuid, AppendRevokeBarrier>>,
> = std::sync::LazyLock::new(|| tokio::sync::Mutex::new(HashMap::new()));

#[cfg(feature = "db-tests")]
pub async fn arm_session_revision_persist_barrier(
    document_id: Uuid,
) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
    let (reached_tx, reached_rx) = oneshot::channel();
    let (proceed_tx, proceed_rx) = oneshot::channel();
    SESSION_REVISION_PERSIST_BARRIERS.lock().await.insert(
        document_id,
        AppendRevokeBarrier {
            reached_tx,
            proceed_rx,
        },
    );
    (reached_rx, proceed_tx)
}

#[cfg(feature = "db-tests")]
pub async fn disarm_session_revision_persist_barrier(document_id: Uuid) {
    SESSION_REVISION_PERSIST_BARRIERS
        .lock()
        .await
        .remove(&document_id);
}

#[cfg(feature = "db-tests")]
pub async fn session_revision_persist_barrier_armed(document_id: Uuid) -> bool {
    SESSION_REVISION_PERSIST_BARRIERS
        .lock()
        .await
        .contains_key(&document_id)
}

#[cfg(feature = "db-tests")]
async fn pause_for_session_revision_persist_barrier(document_id: Uuid) {
    let barrier = SESSION_REVISION_PERSIST_BARRIERS
        .lock()
        .await
        .remove(&document_id);
    if let Some(barrier) = barrier {
        let _ = barrier.reached_tx.send(());
        let _ = barrier.proceed_rx.await;
    }
}

#[cfg(feature = "db-tests")]
static ACTOR_PANIC_ON_FRAME: std::sync::LazyLock<
    tokio::sync::Mutex<std::collections::HashSet<Uuid>>,
> = std::sync::LazyLock::new(|| tokio::sync::Mutex::new(std::collections::HashSet::new()));

#[cfg(feature = "db-tests")]
pub async fn arm_actor_panic_on_next_frame(document_id: Uuid) {
    ACTOR_PANIC_ON_FRAME.lock().await.insert(document_id);
}

#[cfg(feature = "db-tests")]
pub async fn disarm_actor_panic_on_next_frame(document_id: Uuid) {
    ACTOR_PANIC_ON_FRAME.lock().await.remove(&document_id);
}

#[cfg(feature = "db-tests")]
async fn consume_actor_panic_on_frame(document_id: Uuid) -> bool {
    ACTOR_PANIC_ON_FRAME.lock().await.remove(&document_id)
}

#[cfg(feature = "db-tests")]
static TEARDOWN_BARRIERS: std::sync::LazyLock<
    tokio::sync::Mutex<HashMap<Uuid, AppendRevokeBarrier>>,
> = std::sync::LazyLock::new(|| tokio::sync::Mutex::new(HashMap::new()));

#[cfg(feature = "db-tests")]
pub async fn arm_teardown_barrier(
    document_id: Uuid,
) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
    let (reached_tx, reached_rx) = oneshot::channel();
    let (proceed_tx, proceed_rx) = oneshot::channel();
    TEARDOWN_BARRIERS.lock().await.insert(
        document_id,
        AppendRevokeBarrier {
            reached_tx,
            proceed_rx,
        },
    );
    (reached_rx, proceed_tx)
}

#[cfg(feature = "db-tests")]
pub async fn disarm_teardown_barrier(document_id: Uuid) {
    TEARDOWN_BARRIERS.lock().await.remove(&document_id);
}

#[cfg(feature = "db-tests")]
async fn pause_before_guard_release(document_id: Uuid) {
    let barrier = TEARDOWN_BARRIERS.lock().await.remove(&document_id);
    if let Some(barrier) = barrier {
        let _ = barrier.reached_tx.send(());
        let _ = barrier.proceed_rx.await;
    }
}

#[cfg(feature = "db-tests")]
static ENGINE_STOP_WITNESSES: std::sync::LazyLock<
    tokio::sync::Mutex<HashMap<Uuid, oneshot::Sender<bool>>>,
> = std::sync::LazyLock::new(|| tokio::sync::Mutex::new(HashMap::new()));

#[cfg(feature = "db-tests")]
pub async fn arm_engine_stop_witness(document_id: Uuid) -> oneshot::Receiver<bool> {
    let (tx, rx) = oneshot::channel();
    assert!(ENGINE_STOP_WITNESSES
        .lock()
        .await
        .insert(document_id, tx)
        .is_none());
    rx
}

#[cfg(feature = "db-tests")]
pub async fn disarm_engine_stop_witness(document_id: Uuid) {
    ENGINE_STOP_WITNESSES.lock().await.remove(&document_id);
}

#[cfg(feature = "db-tests")]
async fn signal_engine_stopped(document_id: Uuid, succeeded: bool) {
    if let Some(tx) = ENGINE_STOP_WITNESSES.lock().await.remove(&document_id) {
        let _ = tx.send(succeeded);
    }
}

#[cfg(feature = "db-tests")]
static JOIN_CHANNEL_ADMISSIONS: std::sync::LazyLock<
    tokio::sync::Mutex<HashMap<Uuid, oneshot::Sender<()>>>,
> = std::sync::LazyLock::new(|| tokio::sync::Mutex::new(HashMap::new()));

#[cfg(feature = "db-tests")]
pub async fn arm_join_channel_admission_witness(conn_id: Uuid) -> oneshot::Receiver<()> {
    let (tx, rx) = oneshot::channel();
    assert!(JOIN_CHANNEL_ADMISSIONS
        .lock()
        .await
        .insert(conn_id, tx)
        .is_none());
    rx
}

#[cfg(feature = "db-tests")]
pub async fn disarm_join_channel_admission_witness(conn_id: Uuid) {
    JOIN_CHANNEL_ADMISSIONS.lock().await.remove(&conn_id);
}

#[cfg(feature = "db-tests")]
async fn signal_join_channel_admitted(conn_id: Uuid) {
    if let Some(tx) = JOIN_CHANNEL_ADMISSIONS.lock().await.remove(&conn_id) {
        let _ = tx.send(());
    }
}

#[cfg(feature = "db-tests")]
static JOIN_DELIVERY_ATTEMPTS: std::sync::LazyLock<tokio::sync::Mutex<HashMap<Uuid, usize>>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(HashMap::new()));

#[cfg(feature = "db-tests")]
pub async fn join_delivery_attempt_count(conn_id: Uuid) -> usize {
    JOIN_DELIVERY_ATTEMPTS
        .lock()
        .await
        .get(&conn_id)
        .copied()
        .unwrap_or(0)
}

#[cfg(feature = "db-tests")]
async fn record_join_delivery_attempt(conn_id: Uuid) {
    *JOIN_DELIVERY_ATTEMPTS
        .lock()
        .await
        .entry(conn_id)
        .or_insert(0) += 1;
}

const OUTBOUND_FRAME_OVERHEAD: usize = 48;

async fn wait_spawn_room_block(_document_id: Uuid) {
    #[cfg(feature = "db-tests")]
    {
        let gate = SPAWN_ROOM_BLOCKS.lock().await.remove(&_document_id);
        if let Some(rx) = gate {
            let _ = rx.await;
        }
    }
}

/// Hub room identity: `(workspace_id, resource_id, kind)`. Tuple fields keep
/// `key.0` / `key.1` as workspace and resource; a `(workspace_id, document_id)`
/// pair converts to a document room.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RoomKey(pub Uuid, pub Uuid, pub CollabKind);

impl RoomKey {
    pub fn document(workspace_id: Uuid, document_id: Uuid) -> Self {
        Self(workspace_id, document_id, CollabKind::Document)
    }

    pub fn task(workspace_id: Uuid, task_id: Uuid) -> Self {
        Self(workspace_id, task_id, CollabKind::Task)
    }

    pub fn kind(&self) -> CollabKind {
        self.2
    }
}

impl From<(Uuid, Uuid)> for RoomKey {
    fn from((workspace_id, document_id): (Uuid, Uuid)) -> Self {
        Self::document(workspace_id, document_id)
    }
}

#[derive(Debug, Clone)]
pub struct CollabSession {
    pub session_id: Uuid,
    pub user_id: Uuid,
    pub given_name: String,
    pub family_name: Option<String>,
    pub locale: String,
}

impl From<LiveSession> for CollabSession {
    fn from(live: LiveSession) -> Self {
        Self {
            session_id: live.session_id,
            user_id: live.user_id,
            given_name: live.given_name,
            family_name: live.family_name,
            locale: if live.locale.is_empty() {
                "en".into()
            } else {
                live.locale
            },
        }
    }
}

struct ConnectionOutboundBudget {
    /// Counts the frames queued for this connection plus the one the transport
    /// is sending (a permit drops only after the send). The events channel has
    /// the same capacity, so while the transport is blocked mid-send a slot
    /// stays free for `enqueue_close_ordered` to queue Close behind the data
    /// (unaccounted pre-auth frames aside); otherwise Close falls back to the
    /// cancel watch.
    frame_sem: Arc<Semaphore>,
    queued_bytes: AtomicUsize,
    max_bytes: usize,
}

pub(crate) struct OutboundDeliveryPermit {
    #[allow(dead_code)]
    frame_permit: OwnedSemaphorePermit,
    accounted_bytes: usize,
    budget: Arc<ConnectionOutboundBudget>,
}

impl Drop for OutboundDeliveryPermit {
    fn drop(&mut self) {
        self.budget
            .queued_bytes
            .fetch_sub(self.accounted_bytes, Ordering::Relaxed);
    }
}

pub struct OutboundFrame {
    pub bytes: Vec<u8>,
    pub kind: OutboundKind,
    permit: Option<OutboundDeliveryPermit>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboundKind {
    Control,
    Data,
}

impl std::fmt::Debug for OutboundFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OutboundFrame")
            .field("bytes", &self.bytes.len())
            .field("kind", &self.kind)
            .field("permit", &self.permit.is_some())
            .finish()
    }
}

impl OutboundFrame {
    pub fn unaccounted(bytes: Vec<u8>) -> Self {
        Self {
            bytes,
            kind: OutboundKind::Control,
            permit: None,
        }
    }
}

/// Independent transport cancellation carrying the required close code/reason.
#[derive(Debug, Clone)]
pub struct ConnectionCancel {
    pub code: u16,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct AuthenticatedConnection {
    pub conn_id: Uuid,
    pub session: CollabSession,
    pub client_id: u32,
    pub read_only: bool,
    pub routing_key: String,
}

#[derive(Debug)]
pub enum RoomClientEvent {
    Outbound(OutboundFrame),
    Close { code: u16, reason: String },
}

pub struct RoomJoin {
    pub conn: AuthenticatedConnection,
    pub events: mpsc::Sender<RoomClientEvent>,
    /// Independent transport cancellation; not subject to the data queue.
    pub cancel: Option<watch::Sender<Option<ConnectionCancel>>>,
}

/// Actor-issued socket lifetime; dropping the sender closes the connection.
#[derive(Debug)]
#[must_use = "retain the lease for the connection lifetime"]
pub struct ConnectionLease {
    pub conn_id: Uuid,
    _hold: oneshot::Sender<Infallible>,
    pub(crate) family_room_delivery: Option<FamilyRoomDeliveryFence>,
}

struct JoinAdmission {
    lease: ConnectionLease,
    drop_rx: oneshot::Receiver<Infallible>,
    conn_generation: u64,
}

struct PendingLeaseDrop {
    conn_id: Uuid,
    generation: u64,
    queued_commands: usize,
}

struct ConnectionLeaseDrop {
    conn_id: Uuid,
    generation: u64,
    drop_rx: oneshot::Receiver<Infallible>,
}

impl Future for ConnectionLeaseDrop {
    type Output = (Uuid, u64);

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.drop_rx.poll_unpin(cx) {
            Poll::Ready(Ok(infallible)) => match infallible {},
            Poll::Ready(Err(_)) => Poll::Ready((self.conn_id, self.generation)),
            Poll::Pending => Poll::Pending,
        }
    }
}

enum RoomCommand {
    Join(
        RoomJoin,
        oneshot::Sender<Result<ConnectionLease, JoinError>>,
    ),
    Leave(Uuid),
    Frame {
        conn_id: Uuid,
        bytes: Vec<u8>,
    },
    CaptureRevision {
        actor_user_id: Uuid,
        session_id: Uuid,
        reply: oneshot::Sender<Result<GuardedCapturedRevision, RevisionCaptureError>>,
    },
    Restore {
        actor_user_id: Uuid,
        session_id: Uuid,
        snap: Vec<u8>,
        intent: RestoreRevisionInput,
        reply: oneshot::Sender<Result<Uuid, RevisionRestoreError>>,
    },
    /// External body write (PUT body / patch block): replace the fragment with
    /// the Doc seeded from `seed` as one forward system update.
    ReplaceBody {
        actor_user_id: Uuid,
        session_id: Uuid,
        seed: Vec<u8>,
        expected_tail_seq: Option<i64>,
        reply: oneshot::Sender<Result<(), BodyWriteError>>,
    },
    /// Live Tiptap JSON projection and the committed tail it reflects.
    ProjectLive {
        actor_user_id: Uuid,
        session_id: Uuid,
        reply: oneshot::Sender<Result<LiveProjection, BodyWriteError>>,
    },
    // Test-only entry into the existing receipt/readback consumer on the actual
    // hub-owned actor; errors still come from the original SDK transaction.
    #[cfg(all(test, feature = "db-tests"))]
    ReconcileForTest {
        actor: Uuid,
        credential: Uuid,
        op_id: Uuid,
        expected_tail: i64,
        payload: Vec<u8>,
        reply: oneshot::Sender<(Option<AppendCollabResult>, bool)>,
    },
    Shutdown,
    #[cfg(feature = "db-tests")]
    Probe(oneshot::Sender<ActorProbe>),
}

fn reject_room_command(cmd: RoomCommand) {
    match cmd {
        RoomCommand::Join(_, reply) => {
            let _ = reply.send(Err(JoinError::EngineUnavailable));
        }
        RoomCommand::CaptureRevision { reply, .. } => {
            let _ = reply.send(Err(RevisionCaptureError::Unavailable));
        }
        RoomCommand::Restore { reply, .. } => {
            let _ = reply.send(Err(RevisionRestoreError::Unavailable));
        }
        RoomCommand::ReplaceBody { reply, .. } => {
            let _ = reply.send(Err(BodyWriteError::Unavailable));
        }
        RoomCommand::ProjectLive { reply, .. } => {
            let _ = reply.send(Err(BodyWriteError::Unavailable));
        }
        #[cfg(all(test, feature = "db-tests"))]
        RoomCommand::ReconcileForTest { reply, .. } => {
            let _ = reply.send((None, false));
        }
        RoomCommand::Leave(_) | RoomCommand::Frame { .. } | RoomCommand::Shutdown => {}
        #[cfg(feature = "db-tests")]
        RoomCommand::Probe(reply) => {
            let _ = reply.send(ActorProbe {
                connections: 0,
                awareness_clients: 0,
            });
        }
    }
}

#[cfg(feature = "db-tests")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActorProbe {
    pub connections: usize,
    pub awareness_clients: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JoinError {
    AdmissionDenied,
    UnsupportedKind,
    /// Hub room cap, room-permit semaphore closed, actor queue full, or the
    /// per-room connection cap.
    RoomFull,
    /// Aggregate helper memory budget, or the primary helper pool at its cap.
    /// The WebSocket transport closes this and `RoomFull` with 1013 (1012
    /// while the hub shuts down).
    CapacityRetry,
    EngineUnavailable,
    WriterStale,
    DbError,
}

#[derive(Debug, Clone)]
pub struct CapturedRevision {
    pub y_snapshot: Vec<u8>,
    pub content_json: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevisionCaptureError {
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevisionRestoreError {
    Rejected,
    Unavailable,
    Conflict,
}

/// Outcome of an external body write applied through the room actor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyWriteError {
    /// Actor lost edit access, or the document is gone/archived.
    Rejected,
    Unavailable,
    /// `expected_tail_seq` no longer matches the committed tail.
    Conflict,
    /// Update or collab state budget exceeded.
    TooLarge,
    /// The engine refused the seed Doc.
    Invalid,
    /// Durable, but the derived body could not be projected.
    DeriveFailed,
}

#[derive(Debug, Clone)]
pub struct LiveProjection {
    pub content_json: serde_json::Value,
    pub tail_seq: i64,
}

/// Engine failures of a forward write; restore and body writes map them differently.
enum ForwardWriteError {
    Rejected,
    Unavailable,
    Conflict,
    EngineLimit,
    EngineMalformed,
    AppendTooLarge,
}

pub(crate) enum JoinDelivery {
    Replied(Result<ConnectionLease, JoinError>),
    /// Mailbox closed before enqueue; the only delivery that proves the join
    /// never reached the actor and may retry on a later generation.
    NotDelivered(RoomJoin),
    /// Mailbox full; backpressure, not stale-slot proof. Do not reclaim.
    QueueFull,
    /// Actor accepted the command then dropped the reply. This does not prove
    /// the join was undelivered; never retry the same conn_id.
    NoReply,
}

impl std::fmt::Debug for JoinDelivery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Replied(result) => f.debug_tuple("Replied").field(result).finish(),
            Self::NotDelivered(_) => f.write_str("NotDelivered(..)"),
            Self::QueueFull => f.write_str("QueueFull"),
            Self::NoReply => f.write_str("NoReply"),
        }
    }
}

#[derive(Clone)]
pub struct RoomHandle {
    tx: mpsc::Sender<RoomCommand>,
    /// Set before `Shutdown` is enqueued so in-flight session snapshot work can abort.
    session_cancel: watch::Sender<bool>,
}

impl RoomHandle {
    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }

    pub async fn join(&self, join: RoomJoin) -> Result<ConnectionLease, JoinError> {
        #[cfg(feature = "db-tests")]
        let conn_id = join.conn.conn_id;
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(RoomCommand::Join(join, reply_tx))
            .await
            .map_err(|_| JoinError::EngineUnavailable)?;
        #[cfg(feature = "db-tests")]
        signal_join_channel_admitted(conn_id).await;
        #[cfg(feature = "db-tests")]
        pause_before_join_reply(conn_id).await;
        reply_rx.await.map_err(|_| JoinError::EngineUnavailable)?
    }

    pub async fn leave(&self, conn_id: Uuid) {
        let _ = self.tx.send(RoomCommand::Leave(conn_id)).await;
    }

    #[cfg(feature = "db-tests")]
    pub async fn probe(&self) -> ActorProbe {
        let (reply_tx, reply_rx) = oneshot::channel();
        if self.tx.send(RoomCommand::Probe(reply_tx)).await.is_err() {
            return ActorProbe {
                connections: 0,
                awareness_clients: 0,
            };
        }
        reply_rx.await.unwrap_or(ActorProbe {
            connections: 0,
            awareness_clients: 0,
        })
    }

    pub async fn frame(&self, conn_id: Uuid, bytes: Vec<u8>) {
        let _ = self.tx.send(RoomCommand::Frame { conn_id, bytes }).await;
    }

    #[cfg(all(test, feature = "db-tests"))]
    async fn reconcile_for_test(
        &self,
        actor: Uuid,
        credential: Uuid,
        op_id: Uuid,
        expected_tail: i64,
        payload: Vec<u8>,
    ) -> (Option<AppendCollabResult>, bool) {
        let (reply, receive) = oneshot::channel();
        self.tx
            .send(RoomCommand::ReconcileForTest {
                actor,
                credential,
                op_id,
                expected_tail,
                payload,
                reply,
            })
            .await
            .expect("live actor test command");
        receive.await.expect("actual consumer completed")
    }

    pub async fn shutdown(&self) {
        let _ = self.session_cancel.send(true);
        let _ = self.tx.send(RoomCommand::Shutdown).await;
    }

    /// Close an empty room after the work already queued before it (no cancel
    /// signal). The last-disconnect session revision, with its bounded head
    /// retries, runs in the actor iteration that emptied the room, so this
    /// Shutdown is handled only after it.
    pub async fn shutdown_after_queued(&self) {
        let _ = self.tx.send(RoomCommand::Shutdown).await;
    }

    pub async fn capture_revision(
        &self,
        actor_user_id: Uuid,
        session_id: Uuid,
    ) -> Result<CapturedRevision, RevisionCaptureError> {
        self.capture_revision_guarded(actor_user_id, session_id)
            .await
            .map(|result| result.captured)
    }

    pub(crate) async fn capture_revision_guarded(
        &self,
        actor_user_id: Uuid,
        session_id: Uuid,
    ) -> Result<GuardedCapturedRevision, RevisionCaptureError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(RoomCommand::CaptureRevision {
                actor_user_id,
                session_id,
                reply: reply_tx,
            })
            .await
            .map_err(|_| RevisionCaptureError::Unavailable)?;
        reply_rx
            .await
            .map_err(|_| RevisionCaptureError::Unavailable)?
    }

    pub async fn restore_from_snapshot(
        &self,
        actor_user_id: Uuid,
        session_id: Uuid,
        snap: Vec<u8>,
        intent: RestoreRevisionInput,
    ) -> Result<Uuid, RevisionRestoreError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(RoomCommand::Restore {
                actor_user_id,
                session_id,
                snap,
                intent,
                reply: reply_tx,
            })
            .await
            .map_err(|_| RevisionRestoreError::Unavailable)?;
        reply_rx
            .await
            .map_err(|_| RevisionRestoreError::Unavailable)?
    }

    pub async fn replace_body(
        &self,
        actor_user_id: Uuid,
        session_id: Uuid,
        seed: Vec<u8>,
        expected_tail_seq: Option<i64>,
    ) -> Result<(), BodyWriteError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(RoomCommand::ReplaceBody {
                actor_user_id,
                session_id,
                seed,
                expected_tail_seq,
                reply: reply_tx,
            })
            .await
            .map_err(|_| BodyWriteError::Unavailable)?;
        reply_rx.await.map_err(|_| BodyWriteError::Unavailable)?
    }

    pub async fn project_live(
        &self,
        actor_user_id: Uuid,
        session_id: Uuid,
    ) -> Result<LiveProjection, BodyWriteError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(RoomCommand::ProjectLive {
                actor_user_id,
                session_id,
                reply: reply_tx,
            })
            .await
            .map_err(|_| BodyWriteError::Unavailable)?;
        reply_rx.await.map_err(|_| BodyWriteError::Unavailable)?
    }

    pub(crate) async fn deliver_join(&self, join: RoomJoin) -> JoinDelivery {
        #[cfg(feature = "db-tests")]
        let conn_id = join.conn.conn_id;
        #[cfg(feature = "db-tests")]
        record_join_delivery_attempt(conn_id).await;
        let (reply_tx, reply_rx) = oneshot::channel();
        match self.tx.try_send(RoomCommand::Join(join, reply_tx)) {
            Ok(()) => {
                #[cfg(feature = "db-tests")]
                signal_join_channel_admitted(conn_id).await;
                #[cfg(feature = "db-tests")]
                pause_before_join_reply(conn_id).await;
                match reply_rx.await {
                    Ok(result) => JoinDelivery::Replied(result),
                    Err(_) => JoinDelivery::NoReply,
                }
            }
            Err(tokio::sync::mpsc::error::TrySendError::Full(_cmd)) => JoinDelivery::QueueFull,
            Err(tokio::sync::mpsc::error::TrySendError::Closed(cmd)) => match cmd {
                RoomCommand::Join(join, _) => JoinDelivery::NotDelivered(join),
                _ => JoinDelivery::NoReply,
            },
        }
    }
}

pub(crate) struct GuardedCapturedRevision {
    pub(crate) captured: CapturedRevision,
    pub(crate) proof: Option<FamilyNativeConsumerProof>,
}

struct CommittedBundle {
    snapshot: Vec<u8>,
    tail_payloads: Vec<Vec<u8>>,
    tail_seq: i64,
    snapshot_cutoff_seq: i64,
}

struct ConnectionState {
    session: CollabSession,
    client_id: u32,
    read_only: bool,
    routing_key: String,
    events: mpsc::Sender<RoomClientEvent>,
    cancel: Option<watch::Sender<Option<ConnectionCancel>>>,
    outbound_budget: Arc<ConnectionOutboundBudget>,
    conn_generation: u64,
    pending_bytes: usize,
    poisoned: bool,
    pending_persist: VecDeque<PersistBarrier>,
    in_flight: bool,
}

struct PersistBarrier {
    request_id: Uuid,
    prefix_fifo: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProjectDerivedOutcome {
    Projected,
    Unchanged,
    /// Empty Yjs seed at `tail_seq == 0`; true no-op for manual persist.
    SkippedSeed,
    /// Source-compatible deterministic content rejection (Malformed, body caps, prepare).
    DeterministicSkip,
    /// Primary not loaded or dirty; retry after reload, not a successful derive.
    PrimaryNotReady,
    /// `(writer_generation, tail_seq)` fence mismatch after commit.
    StaleCutoff,
    /// Archived/trashed/forbidden at derive time.
    PermissionDenied,
    StaleWriter,
    /// Engine/helper operational failure; primary child was recycled when possible.
    EngineFailed,
    /// DB operational failure on derived write/event.
    DbFailed,
}

struct RoomActor {
    workspace_id: Uuid,
    /// Resource id of the room (document or task, see `kind`).
    document_id: Uuid,
    kind: CollabKind,
    config: CollabConfig,
    backend: Backend,
    engine: EngineBridge,
    room_guard: Option<BackendRoomGuard>,
    guard_cleanup_failed: bool,
    writer_generation: Option<i64>,
    committed: CommittedBundle,
    committed_writer_generation: i64,
    connections: HashMap<Uuid, ConnectionState>,
    connection_lease_drops: FuturesUnordered<ConnectionLeaseDrop>,
    pending_lease_drops: Vec<PendingLeaseDrop>,
    live_conns: Arc<AtomicUsize>,
    awareness: AwarenessRegistry,
    fifo_seq: u64,
    /// Last compaction attempt failed; auto-compact backs off until a manual persist succeeds.
    compact_unhealthy: bool,
    /// Tail row count when auto-compact last failed; retry after growth.
    compact_retry_at_tail_len: Option<usize>,
    primary_loaded: bool,
    /// True after durable collab state has been loaded into `committed`.
    /// Distinct from `primary_loaded`, which becomes true after reloading the
    /// engine from whatever `committed` currently holds (including the empty default).
    committed_loaded: bool,
    primary_dirty: bool,
    client_id_owner: HashMap<u32, (Uuid, Instant)>,
    pending_awareness: VecDeque<Vec<u8>>,
    flushing_awareness: bool,
    /// Set when the dedicated fence connection is lost; actor exits once empty.
    fence_lost: bool,
    user_reject_budgets: HashMap<Uuid, UserRejectBudget>,
    session_cancel_rx: watch::Receiver<bool>,
    /// Last-disconnect session snapshot (`captured` set after durable reload + primary capture).
    session_revision: Option<SessionRevisionState>,
    /// Hub memory admission for this room, held until the first helper load
    /// makes the helper's RSS visible to later admissions.
    admission_reservation: Option<MemoryReservation>,
}

struct SessionRevisionState {
    writer_generation: i64,
    /// Set after capture; retained across `StaleRevisionHead` retries without recapture.
    captured: Option<CapturedRevision>,
    head_retries: u32,
}

pub async fn spawn_room(
    key: RoomKey,
    config: CollabConfig,
    pool: PgPool,
    room_guard: RoomGuard,
    live_conns: Arc<AtomicUsize>,
    admission_reservation: MemoryReservation,
) -> Result<(RoomHandle, oneshot::Receiver<()>), JoinError> {
    spawn_room_backend(
        key,
        config,
        Backend::Postgres(pool),
        BackendRoomGuard::Postgres(room_guard),
        None,
        live_conns,
        admission_reservation,
    )
    .await
}

pub(crate) async fn spawn_room_backend(
    key: RoomKey,
    config: CollabConfig,
    backend: Backend,
    room_guard: BackendRoomGuard,
    initial_load: Option<crate::db::collab::CollabLoadState>,
    live_conns: Arc<AtomicUsize>,
    admission_reservation: MemoryReservation,
) -> Result<(RoomHandle, oneshot::Receiver<()>), JoinError> {
    let RoomKey(workspace_id, document_id, kind) = key;
    wait_spawn_room_block(document_id).await;
    // Starts only the bridge thread (it fails only if the OS refuses a thread);
    // the helper spawns on the actor's first reload.
    let engine = match EngineBridge::spawn(config.engine_bin.clone(), config.limits) {
        Ok(engine) => engine,
        Err(_) => {
            if let Err(error) = room_guard
                .release_bounded(Duration::from_millis(config.rpc_timeout_ms))
                .await
            {
                tracing::error!(%document_id, %error, "room startup guard cleanup unconfirmed");
                return Err(JoinError::DbError);
            }
            return Err(JoinError::EngineUnavailable);
        }
    };
    let (tx, mut rx) = mpsc::channel(config.max_queued_room_ops);
    let (session_cancel_tx, session_cancel_rx) = watch::channel(false);
    let (finished_tx, finished_rx) = oneshot::channel();
    let mut actor = RoomActor {
        workspace_id,
        document_id,
        kind,
        config,
        backend,
        engine,
        room_guard: Some(room_guard),
        guard_cleanup_failed: false,
        writer_generation: None,
        committed: CommittedBundle {
            snapshot: vec![0, 0],
            tail_payloads: Vec::new(),
            tail_seq: 0,
            snapshot_cutoff_seq: 0,
        },
        committed_writer_generation: 0,
        connections: HashMap::new(),
        connection_lease_drops: FuturesUnordered::new(),
        pending_lease_drops: Vec::new(),
        live_conns,
        awareness: AwarenessRegistry::new(),
        fifo_seq: 0,
        compact_unhealthy: false,
        compact_retry_at_tail_len: None,
        primary_loaded: false,
        committed_loaded: false,
        primary_dirty: false,
        client_id_owner: HashMap::new(),
        pending_awareness: VecDeque::new(),
        flushing_awareness: false,
        fence_lost: false,
        user_reject_budgets: HashMap::new(),
        session_cancel_rx,
        session_revision: None,
        admission_reservation: Some(admission_reservation),
    };
    if let Some(load) = initial_load {
        actor.set_committed_from_load(&load);
    }
    tokio::spawn(async move {
        let mut actor = actor;
        let document_id = actor.document_id;
        let exit = match AssertUnwindSafe(actor.run_loop(&mut rx))
            .catch_unwind()
            .await
        {
            Ok(exit) => exit,
            Err(_) => {
                tracing::error!(document_id = %document_id, "collab actor panicked");
                RoomExit::Panicked
            }
        };
        let teardown_clean = actor.teardown(rx, exit).await;
        if matches!(exit, RoomExit::Clean) && teardown_clean {
            let _ = finished_tx.send(());
        }
    });
    Ok((
        RoomHandle {
            tx,
            session_cancel: session_cancel_tx,
        },
        finished_rx,
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RoomExit {
    Clean,
    Panicked,
}

enum LockingAuth {
    Allow,
    Deny,
    DbError,
}

/// How a server-side close is queued relative to frames already queued for
/// the connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CloseOrder {
    /// Signal the transport's cancel watch at once, ahead of queued data.
    Preempt,
    /// Queue Close behind queued data so a committed ack is delivered first
    /// (see [`RoomActor::enqueue_close_ordered`]).
    AfterQueued,
}

impl RoomActor {
    fn publish_live_conns(&self) {
        self.live_conns
            .store(self.connections.len(), Ordering::Release);
    }

    #[cfg(feature = "db-tests")]
    async fn drain_ready_lease_drops(&mut self, queued_commands: usize) {
        while let Some(Some((conn_id, generation))) =
            self.connection_lease_drops.next().now_or_never()
        {
            self.handle_lease_drop(conn_id, generation, queued_commands)
                .await;
        }
    }

    async fn handle_lease_drop(&mut self, conn_id: Uuid, generation: u64, queued_commands: usize) {
        // Frames accepted before socket cleanup must run before lease retirement.
        // A separate drop future can win select ahead of both Frame and Leave.
        // Freeze the current mailbox prefix: later arrivals cannot extend it,
        // and a full mailbox needs no extra cleanup slot. Each frame still uses
        // the normal current-permission check and durable append path.
        if queued_commands == 0 {
            self.close_dropped_connection(conn_id, generation).await;
        } else {
            self.pending_lease_drops.push(PendingLeaseDrop {
                conn_id,
                generation,
                queued_commands,
            });
        }
    }

    async fn finish_pending_lease_drops(&mut self) {
        while let Some(index) = self
            .pending_lease_drops
            .iter()
            .position(|drop| drop.queued_commands == 0)
        {
            let drop = self.pending_lease_drops.swap_remove(index);
            self.close_dropped_connection(drop.conn_id, drop.generation)
                .await;
        }
    }

    async fn close_dropped_connection(&mut self, conn_id: Uuid, generation: u64) {
        if self
            .connections
            .get(&conn_id)
            .is_some_and(|conn| conn.conn_generation == generation)
        {
            self.close_connection(conn_id, 1000, "client leave").await;
        }
    }

    fn register_connection_lease(
        &mut self,
        conn_id: Uuid,
        generation: u64,
        drop_rx: oneshot::Receiver<Infallible>,
    ) {
        self.connection_lease_drops.push(ConnectionLeaseDrop {
            conn_id,
            generation,
            drop_rx,
        });
    }

    async fn run_loop(&mut self, rx: &mut mpsc::Receiver<RoomCommand>) -> RoomExit {
        let mut acl_tick = tokio::time::interval(Duration::from_millis(self.config.revoke_poll_ms));
        acl_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let renew_interval = self
            .room_guard
            .as_ref()
            .and_then(BackendRoomGuard::renew_interval);
        let mut renew_tick =
            tokio::time::interval(renew_interval.unwrap_or(Duration::from_secs(1)));
        renew_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            #[cfg(feature = "db-tests")]
            if consume_lease_drop_priority(self.document_id).await {
                self.drain_ready_lease_drops(rx.len()).await;
            }
            tokio::select! {
                cmd = rx.recv() => {
                    // Only commands in the frozen prefix count toward retirement.
                    // Decrement before dispatch so drops observed by Probe below
                    // start at the remaining mailbox, not at this command.
                    for drop in &mut self.pending_lease_drops {
                        drop.queued_commands = drop.queued_commands.saturating_sub(1);
                    }
                    match cmd {
                        Some(RoomCommand::Join(join, reply)) => {
                            match self.handle_join(join).await {
                                Ok(admission) => {
                                    self.register_connection_lease(
                                        admission.lease.conn_id,
                                        admission.conn_generation,
                                        admission.drop_rx,
                                    );
                                    let _ = reply.send(Ok(admission.lease));
                                }
                                Err(err) => {
                                    let _ = reply.send(Err(err));
                                }
                            }
                        }
                        Some(RoomCommand::Leave(conn_id)) => self.handle_leave(conn_id).await,
                        Some(RoomCommand::Frame { conn_id, bytes }) => {
                            self.handle_frame(conn_id, bytes).await;
                        }
                        Some(RoomCommand::CaptureRevision {
                            actor_user_id,
                            session_id,
                            reply,
                        }) => {
                            let _ = reply.send(
                                self.handle_capture_revision(actor_user_id, session_id)
                                    .await,
                            );
                        }
                        Some(RoomCommand::Restore {
                            actor_user_id,
                            session_id,
                            snap,
                            intent,
                            reply,
                        }) => {
                            let _ = reply
                                .send(self.handle_restore(actor_user_id, session_id, snap, intent).await);
                        }
                        Some(RoomCommand::ReplaceBody {
                            actor_user_id,
                            session_id,
                            seed,
                            expected_tail_seq,
                            reply,
                        }) => {
                            let _ = reply.send(
                                self.handle_replace_body(
                                    actor_user_id,
                                    session_id,
                                    seed,
                                    expected_tail_seq,
                                )
                                .await,
                            );
                        }
                        Some(RoomCommand::ProjectLive {
                            actor_user_id,
                            session_id,
                            reply,
                        }) => {
                            let _ = reply
                                .send(self.handle_project_live(actor_user_id, session_id).await);
                        }
                        #[cfg(all(test, feature = "db-tests"))]
                        Some(RoomCommand::ReconcileForTest {
                            actor, credential, op_id, expected_tail, payload, reply,
                        }) => {
                            eprintln!("P102 diagnostic handler before kind={:?} proof_present={} guard_present={} fence_lost={} connections={} writer_generation={:?} committed_generation={} committed_tail={} expected_tail={}", self.kind, self.native_consumer_proof().is_some(), self.room_guard.is_some(), self.fence_lost, self.connections.len(), self.writer_generation, self.committed_writer_generation, self.committed.tail_seq, expected_tail);
                            let digest = payload_digest(&payload);
                            let result = self.reconcile_ambiguous_append(
                                actor, credential, op_id, expected_tail, &payload, &digest,
                            ).await;
                            let reload = if result.is_none() {
                                self.reload_primary_or_close_room().await
                            } else { true };
                            eprintln!("P102 diagnostic handler after ack_present={} reloaded={} proof_present={} guard_present={} fence_lost={} connections={}", result.is_some(), reload, self.native_consumer_proof().is_some(), self.room_guard.is_some(), self.fence_lost, self.connections.len());
                            let _ = reply.send((result, reload));
                        }
                        Some(RoomCommand::Shutdown) => {
                            self.abort_session_revision_work();
                            break;
                        }
                        #[cfg(feature = "db-tests")]
                        Some(RoomCommand::Probe(reply)) => {
                            self.drain_ready_lease_drops(rx.len()).await;
                            let _ = reply.send(ActorProbe {
                                connections: self.connections.len(),
                                awareness_clients: self.awareness.tracked_client_count(),
                            });
                        }
                        None => break,
                    }
                }
                Some((conn_id, generation)) = self.connection_lease_drops.next(), if !self.connection_lease_drops.is_empty() => {
                    self.handle_lease_drop(conn_id, generation, rx.len()).await;
                }
                _ = acl_tick.tick() => {
                    self.poll_acl().await;
                }
                _ = renew_tick.tick(), if renew_interval.is_some() && !self.fence_lost => {
                    let renewal = if let Some(guard) = self.room_guard.as_mut() {
                        Some(tokio::time::timeout(Duration::from_millis(self.config.rpc_timeout_ms), guard.renew()).await)
                    } else { None };
                    match renewal {
                        Some(Ok(Ok(true))) => {},
                        Some(Ok(Err(error))) if self.is_remote_task_room() => self.fatal_remote_write_unconfirmed(error).await,
                        Some(Err(_)) if self.is_remote_task_room() => self.fatal_remote_write_unconfirmed(sqlx::Error::Protocol("Task lease renewal deadline expired; original finish unconfirmed".into())).await,
                        _ => self.fatal_fence_lost().await,
                    }
                }
            }
            self.finish_pending_lease_drops().await;
            self.publish_live_conns();
            // Finish the bounded `StaleRevisionHead` retries in this iteration: a
            // Shutdown queued behind it (admission reclaim) must not drop a
            // revision that only waits for its head retry.
            for _ in 0..SYSTEM_REVISION_HEAD_RETRIES {
                if self.session_revision.is_none() {
                    break;
                }
                if self.advance_session_revision().await {
                    break;
                }
            }
            if self.fence_lost && self.connections.is_empty() {
                break;
            }
        }
        RoomExit::Clean
    }

    async fn teardown(mut self, mut rx: mpsc::Receiver<RoomCommand>, exit: RoomExit) -> bool {
        let (close_code, close_reason) = match exit {
            RoomExit::Clean => (1001, "server shutdown"),
            RoomExit::Panicked => (1011, "collab unavailable"),
        };

        rx.close();
        while let Ok(cmd) = rx.try_recv() {
            reject_room_command(cmd);
        }

        for (_, conn) in self.connections.drain() {
            Self::enqueue_close(&conn.events, &conn.cancel, close_code, close_reason);
        }
        self.publish_live_conns();

        let engine = self.engine;
        let engine_stop_ok = engine.stop().await.is_ok();
        #[cfg(feature = "db-tests")]
        signal_engine_stopped(self.document_id, engine_stop_ok).await;

        #[cfg(feature = "db-tests")]
        pause_before_guard_release(self.document_id).await;

        let guard_clean = match self.room_guard.take() {
            Some(guard) => match guard
                .release_bounded(Duration::from_millis(self.config.rpc_timeout_ms))
                .await
            {
                Ok(()) => true,
                Err(error) => {
                    tracing::error!(%error, document_id=%self.document_id, "room guard cleanup failed");
                    false
                }
            },
            None => !self.guard_cleanup_failed,
        };

        matches!(exit, RoomExit::Clean) && engine_stop_ok && guard_clean
    }

    /// Called on every tick of the room's `revoke_poll_ms` interval (the only
    /// caller). The interval alone sets the cadence; a second elapsed-time gate
    /// here skipped every other tick and doubled revocation latency.
    async fn poll_acl(&mut self) {
        let mut to_close = Vec::new();
        let snapshots = self
            .connections
            .iter()
            .map(|(id, c)| (*id, c.session.clone(), c.read_only))
            .collect::<Vec<_>>();
        for (conn_id, session, read_only) in snapshots {
            match check_delivery_admission_kind_backend(
                &self.backend,
                self.kind,
                self.workspace_id,
                session.user_id,
                session.session_id,
                self.document_id,
                self.delivery_fence(),
            )
            .await
            {
                Ok(DeliveryAdmission::Allowed {
                    read_only: admission_ro,
                }) => {
                    if read_only || !admission_ro {
                        continue;
                    }
                    to_close.push((conn_id, 1008, "permission revoked"));
                }
                Ok(DeliveryAdmission::Denied) => {
                    to_close.push((conn_id, 1008, "permission revoked"));
                }
                Err(_) => {
                    // Poll is a sweep, not an authorization point. Keep the
                    // socket; the next Data-frame delivery read decides.
                }
            }
        }
        for (conn_id, code, reason) in to_close {
            self.close_connection(conn_id, code, reason).await;
        }
    }

    async fn locking_session_auth_by_ids(
        &self,
        user_id: Uuid,
        session_id: Uuid,
        read_only: bool,
    ) -> LockingAuth {
        match resolve_collab_admission_kind_backend(
            &self.backend,
            self.kind,
            self.workspace_id,
            user_id,
            session_id,
            self.document_id,
        )
        .await
        {
            Ok(Ok(admission)) => {
                if read_only || !admission.read_only {
                    LockingAuth::Allow
                } else {
                    LockingAuth::Deny
                }
            }
            Ok(Err(_)) => LockingAuth::Deny,
            Err(_) => LockingAuth::DbError,
        }
    }

    /// Remove the connection and queue its Close; returns its awareness
    /// tombstone for the caller's flush.
    async fn evict_connection(
        &mut self,
        conn_id: Uuid,
        code: u16,
        reason: &str,
        order: CloseOrder,
    ) -> Option<Vec<u8>> {
        let tombstone = if let Some(conn) = self.connections.get(&conn_id) {
            self.awareness
                .remove_client(conn.client_id, conn.conn_generation)
        } else {
            None
        };
        if let Some(conn) = self.connections.remove(&conn_id) {
            match order {
                CloseOrder::Preempt => {
                    Self::enqueue_close(&conn.events, &conn.cancel, code, reason);
                }
                CloseOrder::AfterQueued => {
                    Self::enqueue_close_ordered(&conn.events, &conn.cancel, code, reason);
                }
            }
        }
        tombstone
    }

    async fn close_connection(&mut self, conn_id: Uuid, code: u16, reason: &str) {
        self.close_connection_in_order(conn_id, code, reason, CloseOrder::Preempt)
            .await;
    }

    async fn close_connection_ordered(&mut self, conn_id: Uuid, code: u16, reason: &str) {
        self.close_connection_in_order(conn_id, code, reason, CloseOrder::AfterQueued)
            .await;
    }

    async fn close_connection_in_order(
        &mut self,
        conn_id: Uuid,
        code: u16,
        reason: &str,
        order: CloseOrder,
    ) {
        if let Some(encoded) = self.evict_connection(conn_id, code, reason, order).await {
            self.pending_awareness.push_back(encoded);
        }
        self.flush_pending_awareness().await;
        self.try_schedule_session_revision();
    }

    fn revision_target(&self) -> RevisionTarget {
        match self.kind {
            CollabKind::Document => RevisionTarget::Document(self.document_id),
            CollabKind::Task => RevisionTarget::Task(self.document_id),
        }
    }

    fn session_revision_scheduling_blocked(&self) -> bool {
        self.fence_lost || !self.connections.is_empty() || !self.committed_loaded
    }

    fn session_revision_cancelled(&self) -> bool {
        self.fence_lost || *self.session_cancel_rx.borrow()
    }

    fn abort_session_revision_work(&mut self) {
        self.session_revision = None;
    }

    async fn capture_committed_revision_primary(
        &mut self,
    ) -> Result<CapturedRevision, RevisionCaptureError> {
        let durable = match load_durable_collab_for_room_backend(
            &self.backend,
            self.workspace_id,
            self.revision_target(),
            self.family_fence(),
        )
        .await
        {
            Ok(durable) => durable,
            Err(error) => {
                if self.is_remote_task_room() {
                    self.fatal_remote_write_unconfirmed(error).await;
                }
                return Err(RevisionCaptureError::Unavailable);
            }
        };
        let durable = durable.map_err(|_| RevisionCaptureError::Unavailable)?;
        self.committed.snapshot = durable.snapshot;
        self.committed.tail_payloads = durable.tail;
        self.committed.tail_seq = durable.tail_seq;
        self.committed.snapshot_cutoff_seq = durable.snapshot_cutoff_seq;
        self.committed_loaded = true;
        self.reload_primary_from_committed()
            .await
            .map_err(|_| RevisionCaptureError::Unavailable)?;
        if self.session_revision_cancelled() {
            return Err(RevisionCaptureError::Unavailable);
        }
        if self.ensure_primary_capacity().await.is_err() {
            return Err(RevisionCaptureError::Unavailable);
        }
        if self.session_revision_cancelled() {
            return Err(RevisionCaptureError::Unavailable);
        }
        let snap = match self.engine.call(Request::RevisionSnapshot).await {
            Ok(report) => match report.outcome {
                EngineStatus::Ok {
                    update_b64: Some(bytes),
                    ..
                } => b64::decode(&bytes).map_err(|_| RevisionCaptureError::Unavailable)?,
                _ => return Err(RevisionCaptureError::Unavailable),
            },
            Err(_) => return Err(RevisionCaptureError::Unavailable),
        };
        if self.session_revision_cancelled() {
            return Err(RevisionCaptureError::Unavailable);
        }
        let content_json = match self.engine.call(Request::Project { encoding: 1 }).await {
            Ok(report) => match report.outcome {
                EngineStatus::Ok {
                    content_json: Some(json),
                    ..
                } => json,
                _ => return Err(RevisionCaptureError::Unavailable),
            },
            Err(_) => return Err(RevisionCaptureError::Unavailable),
        };
        Ok(CapturedRevision {
            y_snapshot: snap,
            content_json,
        })
    }

    async fn revision_snapshots_equal_primary(
        &mut self,
        left: &[u8],
        right: &[u8],
    ) -> Result<bool, RevisionCaptureError> {
        if left == right {
            return Ok(true);
        }
        match self
            .engine
            .call(Request::RevisionSnapshotsEqual {
                left_b64: left.to_vec(),
                right_b64: right.to_vec(),
            })
            .await
        {
            Ok(report) => match report.outcome {
                EngineStatus::Ok {
                    update_b64: Some(bytes),
                    ..
                } => {
                    let decoded =
                        b64::decode(&bytes).map_err(|_| RevisionCaptureError::Unavailable)?;
                    Ok(decoded.first() == Some(&1))
                }
                EngineStatus::Malformed { .. } | EngineStatus::ResourceLimit { .. } => {
                    Err(RevisionCaptureError::Unavailable)
                }
                _ => Err(RevisionCaptureError::Unavailable),
            },
            Err(_) => Err(RevisionCaptureError::Unavailable),
        }
    }

    fn try_schedule_session_revision(&mut self) {
        if !self.config.revision_session_snapshot {
            return;
        }
        if self.session_revision.is_some() {
            return;
        }
        if self.session_revision_scheduling_blocked() {
            return;
        }
        let Some(writer_generation) = self.writer_generation else {
            return;
        };
        self.session_revision = Some(SessionRevisionState {
            writer_generation,
            captured: None,
            head_retries: 0,
        });
    }

    /// Linear capture → compare → insert on the room primary (`StaleRevisionHead` retries head only).
    async fn advance_session_revision(&mut self) -> bool {
        if self.session_revision_cancelled() {
            self.abort_session_revision_work();
            return true;
        }
        let Some(mut work) = self.session_revision.take() else {
            return true;
        };
        let workspace_id = self.workspace_id;
        let target = self.revision_target();
        let backend = self.backend.clone();
        let room_fence = self.family_fence();

        if work.captured.is_none() {
            match self.capture_committed_revision_primary().await {
                Ok(captured) => work.captured = Some(captured),
                Err(_) => {
                    tracing::warn!(
                        workspace_id = %workspace_id,
                        target_kind = target.kind_str(),
                        target_id = %target.id(),
                        "collab.session_revision_capture_failed"
                    );
                    self.abort_session_revision_work();
                    return true;
                }
            }
        }

        let captured = work.captured.as_ref().expect("captured");
        if self.session_revision_cancelled() {
            self.abort_session_revision_work();
            return true;
        }

        let latest = match latest_revision_y_snapshot_for_room_backend(
            &backend,
            workspace_id,
            target,
            room_fence,
        )
        .await
        {
            Ok(row) => row,
            Err(err) => {
                tracing::warn!(
                    workspace_id = %workspace_id,
                    target_kind = target.kind_str(),
                    target_id = %target.id(),
                    error = %err,
                    "collab.session_revision_head_read_failed"
                );
                if self.is_remote_task_room() {
                    self.fatal_remote_write_unconfirmed(err).await;
                } else {
                    self.abort_session_revision_work();
                }
                return true;
            }
        };
        let head_fence = SystemRevisionHead::from_latest(latest.clone());

        if let Some((_, prev_snap)) = &latest {
            if self.session_revision_cancelled() {
                self.abort_session_revision_work();
                return true;
            }
            match self
                .revision_snapshots_equal_primary(prev_snap, &captured.y_snapshot)
                .await
            {
                Ok(true) => {
                    self.abort_session_revision_work();
                    return true;
                }
                Ok(false) => {}
                Err(_) => {
                    tracing::warn!(
                        workspace_id = %workspace_id,
                        target_kind = target.kind_str(),
                        target_id = %target.id(),
                        "collab.session_revision_compare_failed"
                    );
                    self.abort_session_revision_work();
                    return true;
                }
            }
        }

        if self.session_revision_cancelled() {
            self.abort_session_revision_work();
            return true;
        }

        let text = match prepare_revision_text(&captured.content_json) {
            Ok(text) => text,
            Err(_) => {
                tracing::warn!(
                    workspace_id = %workspace_id,
                    target_kind = target.kind_str(),
                    target_id = %target.id(),
                    "collab.session_revision_text_failed"
                );
                self.abort_session_revision_work();
                return true;
            }
        };
        let input = CreateRevisionInput {
            y_snapshot: captured.y_snapshot.clone(),
            content_json: captured.content_json.clone(),
            text,
            reason: "session".into(),
        };
        #[cfg(feature = "db-tests")]
        pause_for_session_revision_persist_barrier(self.document_id).await;
        if self.session_revision_cancelled() {
            self.abort_session_revision_work();
            return true;
        }
        let done = match create_system_revision_for_room_backend(
            &backend,
            workspace_id,
            target,
            input,
            work.writer_generation,
            head_fence,
            room_fence,
        )
        .await
        {
            Ok(Ok(_)) => true,
            Ok(Err(RevisionDbError::StaleRevisionHead))
                if work.head_retries + 1 < SYSTEM_REVISION_HEAD_RETRIES =>
            {
                work.head_retries += 1;
                self.session_revision = Some(work);
                return false;
            }
            Ok(Err(_)) => {
                tracing::warn!(
                    workspace_id = %workspace_id,
                    target_kind = target.kind_str(),
                    target_id = %target.id(),
                    "collab.session_revision_persist_skipped"
                );
                true
            }
            Err(err) => {
                tracing::warn!(
                    workspace_id = %workspace_id,
                    target_kind = target.kind_str(),
                    target_id = %target.id(),
                    error = %err,
                    "collab.session_revision_persist_failed"
                );
                if self.is_remote_task_room() {
                    self.fatal_remote_write_unconfirmed(err).await;
                }
                true
            }
        };
        if done {
            self.abort_session_revision_work();
        }
        done
    }

    fn signal_cancel(
        cancel: &Option<watch::Sender<Option<ConnectionCancel>>>,
        code: u16,
        reason: &str,
    ) {
        if let Some(cancel) = cancel {
            let _ = cancel.send_replace(Some(ConnectionCancel {
                code,
                reason: reason.into(),
            }));
        }
    }

    fn enqueue_close(
        events: &mpsc::Sender<RoomClientEvent>,
        cancel: &Option<watch::Sender<Option<ConnectionCancel>>>,
        code: u16,
        reason: &str,
    ) {
        Self::signal_cancel(cancel, code, reason);
        let close = RoomClientEvent::Close {
            code,
            reason: reason.into(),
        };
        let _ = events.try_send(close);
    }

    /// Queue Close after any already-enqueued Data frames. Used for post-commit
    /// server faults so a committed `SyncStatus` ack is not preempted. Falls back
    /// to the preemptive cancel path only when the outbound queue is full/closed.
    fn enqueue_close_ordered(
        events: &mpsc::Sender<RoomClientEvent>,
        cancel: &Option<watch::Sender<Option<ConnectionCancel>>>,
        code: u16,
        reason: &str,
    ) {
        let close = RoomClientEvent::Close {
            code,
            reason: reason.into(),
        };
        if events.try_send(close).is_err() {
            Self::enqueue_close(events, cancel, code, reason);
        }
    }

    fn native_consumer_proof(&self) -> Option<FamilyNativeConsumerProof> {
        self.family_fence().map(|room| FamilyNativeConsumerProof {
            room,
            generation: self.committed_writer_generation,
            tail: self.committed.tail_seq,
        })
    }

    async fn check_cached_native_consumer(
        &mut self,
        actor: Uuid,
        credential: Uuid,
    ) -> Result<Option<FamilyNativeConsumerProof>, ()> {
        let proof = self.native_consumer_proof();
        let checked = verify_room_native_consumer(
            &self.backend,
            self.kind,
            self.workspace_id,
            actor,
            credential,
            self.document_id,
            proof,
        )
        .await;
        match checked {
            Ok(Ok(())) => return Ok(proof),
            Err(error) if self.is_remote_task_room() => {
                self.fatal_remote_write_unconfirmed(error).await;
            }
            _ => self.fatal_fence_lost().await,
        }
        Err(())
    }

    fn family_fence(&self) -> Option<FamilyNativeRoomFence> {
        self.room_guard
            .as_ref()
            .and_then(BackendRoomGuard::family_fence)
    }

    fn delivery_fence(&self) -> Option<FamilyRoomDeliveryFence> {
        self.room_guard
            .as_ref()
            .and_then(BackendRoomGuard::delivery_fence)
    }

    async fn claim_writer(
        &mut self,
        actor: Uuid,
        credential: Uuid,
    ) -> Result<Result<ClaimWriterResult, CollabDbError>, sqlx::Error> {
        let Some(guard) = self.room_guard.as_mut() else {
            return Ok(Err(CollabDbError::StaleWriter));
        };
        let result = guard
            .claim_writer(
                &self.backend,
                self.kind,
                self.workspace_id,
                actor,
                credential,
                self.document_id,
            )
            .await;
        if self.is_remote_task_room() {
            match result {
                Err(error) => {
                    // Preserve the original writer's failure in the hub and
                    // stop this room. A new reader is not a finish receipt.
                    self.fatal_remote_write_unconfirmed(error).await;
                    return Err(sqlx::Error::Protocol(
                        "Task native activation outcome remains unconfirmed".into(),
                    ));
                }
                confirmed => return confirmed,
            }
        }
        result
    }

    async fn handle_join(&mut self, join: RoomJoin) -> Result<JoinAdmission, JoinError> {
        if self.fence_lost {
            return Err(JoinError::EngineUnavailable);
        }
        #[cfg(feature = "db-tests")]
        pause_for_join_barrier(self.document_id).await;
        if self.connections.len() >= self.config.max_connections_per_room {
            return Err(JoinError::RoomFull);
        }
        let admission = resolve_collab_admission_kind_backend(
            &self.backend,
            self.kind,
            self.workspace_id,
            join.conn.session.user_id,
            join.conn.session.session_id,
            self.document_id,
        )
        .await
        .map_err(|err| {
            warn_join_db_error(
                "room.handle_join.admission",
                self.workspace_id,
                self.document_id,
                &err,
            );
            JoinError::DbError
        })?;
        let admission = admission.map_err(|_| JoinError::AdmissionDenied)?;
        let read_only = join.conn.read_only || admission.read_only;
        if self.writer_generation.is_none() && !read_only {
            let claim = self
                .claim_writer(join.conn.session.user_id, join.conn.session.session_id)
                .await
                .map_err(|err| {
                    warn_join_db_error(
                        "room.handle_join.claim_writer",
                        self.workspace_id,
                        self.document_id,
                        &err,
                    );
                    JoinError::DbError
                })?;
            let claim = claim.map_err(|e| match e {
                CollabDbError::StaleWriter => JoinError::WriterStale,
                _ => JoinError::AdmissionDenied,
            })?;
            self.set_committed_from_load(&claim.load);
            if let Err(err) = self.reload_primary_from_committed().await {
                self.close_all_connections(1011, "engine reload failed")
                    .await;
                return Err(err);
            }
            self.writer_generation = Some(claim.writer_generation);
        } else if self.writer_generation.is_none() && read_only {
            let load = match load_room_collab_readonly(
                &self.backend,
                self.kind,
                self.workspace_id,
                join.conn.session.user_id,
                join.conn.session.session_id,
                self.document_id,
                self.family_fence(),
            )
            .await
            {
                Ok(load) => load,
                Err(err) => {
                    warn_join_db_error(
                        "room.handle_join.load_readonly",
                        self.workspace_id,
                        self.document_id,
                        &err,
                    );
                    if self.is_remote_task_room() {
                        self.fatal_remote_write_unconfirmed(err).await;
                    }
                    return Err(JoinError::DbError);
                }
            };
            let load = load.map_err(|_| JoinError::AdmissionDenied)?;
            self.set_committed_from_load(&load);
            if let Err(err) = self.reload_primary_from_committed().await {
                self.close_all_connections(1011, "engine reload failed")
                    .await;
                return Err(err);
            }
        }
        self.ensure_primary_capacity().await?;
        if !read_only && self.writer_generation.is_some() && self.committed.tail_seq >= 1 {
            #[cfg(feature = "db-tests")]
            {
                JOIN_CATCHUP_PROJECTION_ATTEMPTS
                    .lock()
                    .await
                    .entry(self.document_id)
                    .and_modify(|count| *count += 1)
                    .or_insert(1);
            }
            if matches!(
                self.maybe_project_derived_body(
                    self.committed.tail_seq,
                    join.conn.session.user_id,
                    join.conn.session.session_id,
                    true,
                )
                .await,
                ProjectDerivedOutcome::StaleWriter
            ) {
                return Err(JoinError::WriterStale);
            }
        }
        if !self.primary_loaded {
            return Err(JoinError::EngineUnavailable);
        }
        if !self.reserve_client_id(join.conn.client_id, join.conn.session.user_id) {
            return Err(JoinError::AdmissionDenied);
        }
        let conn_generation = self.awareness.connection_generation();
        self.awareness
            .claim_connection_client(join.conn.client_id, conn_generation);
        let conn_id = join.conn.conn_id;
        let routing_key = join.conn.routing_key.clone();
        let outbound_budget = Arc::new(ConnectionOutboundBudget {
            frame_sem: Arc::new(Semaphore::new(
                self.config.max_outbound_frames_per_connection,
            )),
            queued_bytes: AtomicUsize::new(0),
            max_bytes: self.config.max_outbound_bytes_per_connection,
        });
        let (lease_tx, drop_rx) = oneshot::channel();
        self.connections.insert(
            conn_id,
            ConnectionState {
                session: join.conn.session,
                client_id: join.conn.client_id,
                read_only,
                routing_key: routing_key.clone(),
                events: join.events,
                cancel: join.cancel,
                outbound_budget,
                conn_generation,
                pending_bytes: 0,
                poisoned: false,
                pending_persist: VecDeque::new(),
                in_flight: false,
            },
        );
        // Published before the join reply lets the hub drop its joining lease,
        // so reclaim never sees an empty admitted room.
        self.publish_live_conns();
        let encoded = self.awareness.encode_all();
        if !encoded.is_empty() {
            if let Ok(bytes) = encode(&WireFrame::Document {
                routing_key: routing_key.clone(),
                room: None,
                message: DocumentMessage::Awareness(encoded),
            }) {
                self.deliver_outbound(conn_id, bytes, OutboundKind::Data)
                    .await;
            }
        }
        self.flush_pending_awareness().await;
        Ok(JoinAdmission {
            lease: ConnectionLease {
                conn_id,
                _hold: lease_tx,
                family_room_delivery: self.delivery_fence(),
            },
            drop_rx,
            conn_generation,
        })
    }

    fn set_committed_from_load(&mut self, load: &crate::db::collab::CollabLoadState) {
        self.committed_writer_generation = load.writer_generation;
        self.committed.snapshot = load.snapshot.clone();
        self.committed.tail_payloads = load.tail.iter().map(|r| r.payload.clone()).collect();
        self.committed.tail_seq = load.tail_seq;
        self.committed.snapshot_cutoff_seq = load.snapshot_cutoff_seq;
        self.committed_loaded = true;
    }

    async fn close_all_connections(&mut self, code: u16, reason: &str) {
        for conn_id in self.connections.keys().cloned().collect::<Vec<_>>() {
            self.close_connection(conn_id, code, reason).await;
        }
    }

    fn is_remote_task_room(&self) -> bool {
        self.kind == CollabKind::Task && matches!(self.backend, Backend::LibsqlRemote(_))
    }

    async fn fatal_remote_write_unconfirmed(&mut self, error: sqlx::Error) {
        self.fence_lost = true;
        self.writer_generation = None;
        self.primary_loaded = false;
        self.primary_dirty = true;
        self.guard_cleanup_failed = true;
        self.abort_session_revision_work();
        for (_, conn) in self.connections.drain() {
            Self::enqueue_close(&conn.events, &conn.cancel, 1013, "try again later");
        }
        self.pending_awareness.clear();
        self.publish_live_conns();
        if let Some(guard) = self.room_guard.take() {
            // The original transaction owns its tracked cleanup/quarantine.
            // Neither this failure nor a later rollback is a stream receipt.
            guard.retain_remote_write_unknown(error);
        }
    }

    async fn fatal_fence_lost(&mut self) {
        self.fence_lost = true;
        self.writer_generation = None;
        self.primary_loaded = false;
        self.primary_dirty = true;
        self.abort_session_revision_work();
        // Publish transport cancellation for every socket before any cleanup
        // I/O or awareness/helper work can block this terminal failure path.
        for (_, conn) in self.connections.drain() {
            Self::enqueue_close(&conn.events, &conn.cancel, 1013, "try again later");
        }
        self.pending_awareness.clear();
        self.publish_live_conns();
        if let Some(guard) = self.room_guard.take() {
            if let Err(error) = guard
                .release_bounded(Duration::from_millis(self.config.rpc_timeout_ms))
                .await
            {
                self.guard_cleanup_failed = true;
                tracing::error!(%error, document_id=%self.document_id, "lost room guard cleanup unconfirmed");
            }
        }
    }

    async fn reload_primary_or_close_room(&mut self) -> bool {
        if self.is_remote_task_room() && self.guard_cleanup_failed {
            // An uncertain original stream has already transferred its guard
            // to the hub. A cache reload cannot supply its settlement receipt.
            return false;
        }
        match self.reload_primary_from_committed().await {
            Ok(()) => true,
            Err(err) => {
                tracing::error!(
                    document_id = %self.document_id,
                    error = ?err,
                    "primary reload failed; closing room"
                );
                self.fatal_fence_lost().await;
                false
            }
        }
    }

    fn user_write_reject_blocked(&mut self, user_id: Uuid) -> bool {
        let now = Instant::now();
        let budget = self
            .user_reject_budgets
            .entry(user_id)
            .or_insert_with(|| UserRejectBudget {
                rejects: VecDeque::new(),
                cooldown_until: None,
            });
        if let Some(until) = budget.cooldown_until {
            if now < until {
                return true;
            }
            budget.cooldown_until = None;
            budget.rejects.clear();
        }
        budget
            .rejects
            .retain(|t| now.duration_since(*t) < REJECTED_CANDIDATE_WINDOW);
        false
    }

    fn record_user_rejected_candidate(&mut self, user_id: Uuid) {
        let now = Instant::now();
        let budget = self
            .user_reject_budgets
            .entry(user_id)
            .or_insert_with(|| UserRejectBudget {
                rejects: VecDeque::new(),
                cooldown_until: None,
            });
        budget
            .rejects
            .retain(|t| now.duration_since(*t) < REJECTED_CANDIDATE_WINDOW);
        budget.rejects.push_back(now);
        if budget.rejects.len() >= MAX_REJECTED_CANDIDATES_PER_USER {
            budget.cooldown_until = Some(now + USER_REJECT_COOLDOWN);
            budget.rejects.clear();
        }
    }

    async fn recover_primary_after_engine_fault(&mut self) -> bool {
        self.primary_loaded = false;
        self.primary_dirty = true;
        self.reload_primary_from_committed().await.is_ok() && self.primary_loaded
    }

    fn is_definite_append_rejection(err: &CollabDbError) -> bool {
        matches!(
            err,
            CollabDbError::Forbidden
                | CollabDbError::NotFound
                | CollabDbError::OpIdConflict
                | CollabDbError::PayloadTooLarge
                | CollabDbError::StateBudgetExceeded
                | CollabDbError::InvalidCutoff
        )
    }

    fn reserve_client_id(&mut self, client_id: u32, user_id: Uuid) -> bool {
        let now = Instant::now();
        let ttl = Duration::from_millis(self.config.client_id_ttl_ms);
        let live: std::collections::HashSet<u32> = self
            .connections
            .values()
            .map(|conn| conn.client_id)
            .collect();
        self.client_id_owner
            .retain(|cid, (_, at)| live.contains(cid) || now.duration_since(*at) < ttl);
        if self
            .connections
            .values()
            .any(|conn| conn.client_id == client_id && conn.session.user_id != user_id)
        {
            return false;
        }
        if let Some((owner, _)) = self.client_id_owner.get(&client_id) {
            if *owner != user_id {
                return false;
            }
        }
        self.client_id_owner.insert(client_id, (user_id, now));
        true
    }

    async fn handle_leave(&mut self, conn_id: Uuid) {
        // The lease drop may already have evicted this connection; a stale Leave must not
        // schedule another session revision capture.
        if !self.connections.contains_key(&conn_id) {
            return;
        }
        self.close_connection(conn_id, 1000, "client leave").await;
    }

    async fn handle_frame(&mut self, conn_id: Uuid, bytes: Vec<u8>) {
        let Some(conn) = self.connections.get_mut(&conn_id) else {
            return;
        };
        #[cfg(feature = "db-tests")]
        if consume_actor_panic_on_frame(self.document_id).await {
            panic!("db-tests collab actor panic on frame");
        }
        if conn.pending_bytes + bytes.len() > self.config.max_pending_bytes_per_connection {
            self.close_connection(conn_id, 1009, "pending bytes exceeded")
                .await;
            return;
        }
        conn.pending_bytes += bytes.len();
        let routing_key = conn.routing_key.clone();
        let read_only = conn.read_only;
        let client_id = conn.client_id;
        let session = conn.session.clone();
        let conn_generation = conn.conn_generation;

        let frame = match crate::collab::wire::decode(&bytes) {
            Ok(frame) => frame,
            Err(_) => {
                self.close_connection(conn_id, 1003, "invalid frame").await;
                return;
            }
        };

        match frame {
            WireFrame::Connection(_) => {
                self.handle_connection_ping(conn_id).await;
            }
            WireFrame::Document {
                routing_key: key,
                room,
                message,
            } => {
                if key != routing_key {
                    if let Some(conn) = self.connections.get_mut(&conn_id) {
                        conn.pending_bytes = conn.pending_bytes.saturating_sub(bytes.len());
                    }
                    return;
                }
                if room.is_none() {
                    self.close_connection(conn_id, 1008, "invalid room").await;
                    return;
                }
                match check_delivery_admission_kind_backend(
                    &self.backend,
                    self.kind,
                    self.workspace_id,
                    session.user_id,
                    session.session_id,
                    self.document_id,
                    self.delivery_fence(),
                )
                .await
                {
                    Ok(DeliveryAdmission::Allowed {
                        read_only: admission_ro,
                    }) if read_only || !admission_ro => {}
                    Ok(DeliveryAdmission::Denied) | Ok(DeliveryAdmission::Allowed { .. }) => {
                        self.close_connection(conn_id, 1008, "permission revoked")
                            .await;
                        return;
                    }
                    Err(_) => {
                        self.close_connection(conn_id, 1011, "internal error").await;
                        return;
                    }
                }
                self.handle_document_message(
                    conn_id,
                    &routing_key,
                    read_only,
                    client_id,
                    &session,
                    conn_generation,
                    message,
                )
                .await;
            }
        }
        if let Some(conn) = self.connections.get_mut(&conn_id) {
            conn.pending_bytes = conn.pending_bytes.saturating_sub(bytes.len());
        }
    }

    async fn handle_connection_ping(&mut self, conn_id: Uuid) {
        let pong = encode(&WireFrame::Connection(
            crate::collab::wire::ConnectionMessage::Pong,
        ))
        .unwrap_or_default();
        if self.connections.contains_key(&conn_id) {
            self.deliver_outbound(conn_id, pong, OutboundKind::Control)
                .await;
            self.flush_pending_awareness().await;
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn handle_document_message(
        &mut self,
        conn_id: Uuid,
        routing_key: &str,
        read_only: bool,
        client_id: u32,
        session: &CollabSession,
        conn_generation: u64,
        message: DocumentMessage,
    ) {
        match message {
            DocumentMessage::Auth(auth) => {
                self.handle_auth(conn_id, routing_key, read_only, auth)
                    .await;
            }
            DocumentMessage::Sync(sync) => {
                self.handle_sync(conn_id, routing_key, read_only, sync)
                    .await;
            }
            DocumentMessage::Awareness(payload) => {
                match check_delivery_admission_kind_backend(
                    &self.backend,
                    self.kind,
                    self.workspace_id,
                    session.user_id,
                    session.session_id,
                    self.document_id,
                    self.delivery_fence(),
                )
                .await
                {
                    Ok(DeliveryAdmission::Allowed {
                        read_only: admission_ro,
                    }) if read_only || !admission_ro => {}
                    Ok(DeliveryAdmission::Denied) | Ok(DeliveryAdmission::Allowed { .. }) => {
                        self.close_connection(conn_id, 1008, "permission revoked")
                            .await;
                        return;
                    }
                    Err(_) => {
                        self.close_connection(conn_id, 1011, "internal error").await;
                        return;
                    }
                }
                self.handle_awareness(conn_id, client_id, session, conn_generation, payload)
                    .await;
            }
            DocumentMessage::QueryAwareness => {
                let encoded = self.awareness.encode_all();
                if !encoded.is_empty() {
                    self.deliver_document_message(
                        conn_id,
                        routing_key,
                        DocumentMessage::Awareness(encoded),
                    )
                    .await;
                }
            }
            DocumentMessage::Stateless(payload) => {
                self.handle_stateless(conn_id, payload).await;
            }
            DocumentMessage::Close { .. } => {
                self.close_connection(conn_id, 1000, "client close").await;
            }
            _ => {}
        }
    }

    async fn handle_auth(
        &mut self,
        conn_id: Uuid,
        routing_key: &str,
        read_only: bool,
        auth: AuthMessage,
    ) {
        if let AuthMessage::Token { .. } = auth {
            let scope = if read_only { "readonly" } else { "read-write" };
            self.deliver_document_message(
                conn_id,
                routing_key,
                DocumentMessage::Auth(AuthMessage::Authenticated {
                    scope: scope.into(),
                }),
            )
            .await;
        }
    }

    async fn handle_sync(
        &mut self,
        conn_id: Uuid,
        routing_key: &str,
        read_only: bool,
        sync: crate::collab::wire::SyncMessage,
    ) {
        let max_binary = crate::collab::wire::Limits::DEFAULT.max_binary_payload_bytes;
        let (step, payload) = match parse_sync_payload(&sync.y_protocol, max_binary) {
            Ok(parts) => parts,
            Err(_) => return,
        };
        match step {
            SyncStep::Step1 => {
                if self.ensure_primary_capacity().await.is_err() {
                    self.close_connection(conn_id, 1011, "engine unavailable")
                        .await;
                    return;
                }
                let report = match self
                    .engine
                    .call(Request::Sync {
                        state_vector_b64: payload,
                        encoding: 1,
                    })
                    .await
                {
                    Ok(report) => report,
                    Err(BridgeError::Dead) => {
                        self.recover_primary_after_engine_fault().await;
                        self.close_connection(conn_id, 1011, "sync failed").await;
                        return;
                    }
                };
                if matches!(report.outcome, EngineStatus::Malformed { .. }) {
                    self.recover_primary_after_engine_fault().await;
                    self.close_connection(conn_id, 1008, "invalid state vector")
                        .await;
                    return;
                }
                if !matches!(report.outcome, EngineStatus::Ok { .. }) {
                    self.recover_primary_after_engine_fault().await;
                    self.close_connection(conn_id, 1011, "sync failed").await;
                    return;
                }
                if let EngineStatus::Ok {
                    update_b64: Some(update),
                    ..
                } = report.outcome
                {
                    let update = b64::decode(&update).unwrap_or_default();
                    let y_protocol = encode_sync_payload(SyncStep::Step2, &update);
                    self.deliver_document_message(
                        conn_id,
                        routing_key,
                        DocumentMessage::Sync(crate::collab::wire::SyncMessage {
                            step: SyncStep::Step2,
                            y_protocol,
                        }),
                    )
                    .await;
                }
                let inspect = match self.engine.call(Request::Inspect).await {
                    Ok(report) => report,
                    Err(BridgeError::Dead) => {
                        self.recover_primary_after_engine_fault().await;
                        self.close_connection(conn_id, 1011, "sync failed").await;
                        return;
                    }
                };
                if !matches!(inspect.outcome, EngineStatus::Ok { .. }) {
                    self.recover_primary_after_engine_fault().await;
                    self.close_connection(conn_id, 1011, "sync failed").await;
                    return;
                }
                if let EngineStatus::Ok {
                    state_vector_b64: Some(sv_b64),
                    ..
                } = inspect.outcome
                {
                    let sv = b64::decode(&sv_b64).unwrap_or_default();
                    let y_protocol = encode_sync_payload(SyncStep::Step1, &sv);
                    self.deliver_document_message(
                        conn_id,
                        routing_key,
                        DocumentMessage::Sync(crate::collab::wire::SyncMessage {
                            step: SyncStep::Step1,
                            y_protocol,
                        }),
                    )
                    .await;
                }
            }
            SyncStep::Step2 | SyncStep::Update => {
                if payload.is_empty() {
                    if self.primary_dirty && !self.reload_primary_or_close_room().await {
                        return;
                    }
                    if let Some(c) = self.connections.get_mut(&conn_id) {
                        c.in_flight = false;
                        c.poisoned = true;
                    }
                    self.send_sync_status(conn_id, routing_key, false).await;
                    return;
                }
                if read_only && !is_empty_update(&payload) {
                    if let Some(c) = self.connections.get_mut(&conn_id) {
                        c.poisoned = true;
                    }
                    self.send_sync_status(conn_id, routing_key, false).await;
                    return;
                }
                if is_empty_update(&payload) {
                    self.send_sync_status(conn_id, routing_key, true).await;
                    return;
                }
                let conn = self.connections.get_mut(&conn_id);
                if conn.map(|c| c.poisoned || c.in_flight).unwrap_or(true) {
                    self.send_sync_status(conn_id, routing_key, false).await;
                    return;
                }
                let actor_user_id = self
                    .connections
                    .get(&conn_id)
                    .map(|c| c.session.user_id)
                    .unwrap_or_default();
                if self.user_write_reject_blocked(actor_user_id) {
                    self.close_connection_ordered(conn_id, 1008, "update rejected")
                        .await;
                    return;
                }
                if let Some(c) = self.connections.get_mut(&conn_id) {
                    c.in_flight = true;
                }

                if self.ensure_primary_capacity().await.is_err() {
                    if let Some(c) = self.connections.get_mut(&conn_id) {
                        c.in_flight = false;
                    }
                    self.close_connection(conn_id, 1011, "engine unavailable")
                        .await;
                    return;
                }

                if self.writer_generation.is_none() {
                    self.reject_candidate(conn_id, routing_key).await;
                    return;
                }

                let validate_started = std::time::Instant::now();
                let (validation, validate_tx) = self.validate_candidate_on_primary(&payload).await;
                tracing::info!(
                    target: "collab.stage",
                    stage = "validate",
                    elapsed_us = validate_started.elapsed().as_micros() as u64,
                    load_us = validate_tx.load_us,
                    document_id = %self.document_id,
                );
                if validation == BundleValidation::EngineUnavailable {
                    self.reject_candidate_engine_unavailable(conn_id).await;
                    return;
                }
                if validation != BundleValidation::Ok {
                    self.reject_candidate(conn_id, routing_key).await;
                    return;
                }
                #[cfg(feature = "db-tests")]
                pause_for_append_revoke_barrier(self.document_id).await;

                let writer_generation = self.writer_generation.expect("checked above");
                let (session_id, actor_user_id) = self
                    .connections
                    .get(&conn_id)
                    .map(|c| (c.session.session_id, c.session.user_id))
                    .unwrap_or_default();
                let Some(room_guard) = self.room_guard.as_mut() else {
                    tracing::error!(
                        document_id = %self.document_id,
                        "room fence connection missing during append"
                    );
                    self.fatal_fence_lost().await;
                    return;
                };

                #[cfg(feature = "db-tests")]
                pause_for_append_in_tx_reject_barrier(self.document_id).await;

                let op_id = Uuid::now_v7();
                let expected_tail = self.committed.tail_seq;
                let digest = payload_digest(&payload);
                let append_started = std::time::Instant::now();
                let timed_append = room_guard
                    .append(
                        self.kind,
                        AppendCollabInput {
                            workspace_id: self.workspace_id,
                            actor_user_id,
                            session_id,
                            document_id: self.document_id,
                            writer_generation,
                            expected_tail_seq: expected_tail,
                            op_id,
                            payload: &payload,
                            client_ip: None,
                        },
                    )
                    .await;
                let append_tx = timed_append
                    .as_ref()
                    .ok()
                    .map(|(_, timings)| *timings)
                    .unwrap_or_default();
                tracing::info!(
                    target: "collab.stage",
                    stage = "append_tx",
                    elapsed_us = append_started.elapsed().as_micros() as u64,
                    pool_wait_us = append_tx.pool_wait_us,
                    advisory_lock_us = append_tx.advisory_lock_us,
                    row_lock_us = append_tx.row_lock_us,
                    stmt_us = append_tx.stmt_us,
                    commit_us = append_tx.commit_us,
                    document_id = %self.document_id,
                );
                let append = match timed_append {
                    Err(err) => {
                        tracing::error!(
                            document_id = %self.document_id,
                            error = %err,
                            "room fence connection lost during append"
                        );
                        if self.is_remote_task_room() {
                            self.fatal_remote_write_unconfirmed(err).await;
                        } else {
                            self.fatal_fence_lost().await;
                        }
                        return;
                    }
                    Ok((result, _timings)) => result,
                };

                let committed = match append {
                    Ok(result) => result,
                    Err(CollabDbError::StaleWriter) => {
                        self.fatal_writer_stale(CloseOrder::Preempt).await;
                        self.reject_candidate(conn_id, routing_key).await;
                        return;
                    }
                    Err(err) if Self::is_definite_append_rejection(&err) => {
                        if !self.reload_primary_or_close_room().await {
                            return;
                        }
                        self.reject_candidate(conn_id, routing_key).await;
                        return;
                    }
                    Err(CollabDbError::StaleCutoff) | Err(_) => {
                        match self
                            .reconcile_ambiguous_append(
                                actor_user_id,
                                session_id,
                                op_id,
                                expected_tail,
                                &payload,
                                &digest,
                            )
                            .await
                        {
                            Some(result) => result,
                            None => {
                                if !self.reload_primary_or_close_room().await {
                                    return;
                                }
                                self.reject_candidate(conn_id, routing_key).await;
                                return;
                            }
                        }
                    }
                };

                let seq = match committed {
                    AppendCollabResult::Committed { seq }
                    | AppendCollabResult::DuplicateAck { seq } => {
                        if seq != expected_tail + 1 {
                            if !self.reload_primary_or_close_room().await {
                                return;
                            }
                            self.fatal_room_divergence(actor_user_id, session_id).await;
                            self.reject_candidate(conn_id, routing_key).await;
                            return;
                        }
                        seq
                    }
                };

                if self.committed.tail_seq < seq {
                    self.committed.tail_payloads.push(payload.clone());
                }
                self.committed.tail_seq = seq;
                self.fifo_seq += 1;
                let op_prefix = self.fifo_seq;

                let apply_started = std::time::Instant::now();
                let primary_ok = self
                    .integrate_committed_update(&payload, true)
                    .await
                    .is_ok();
                tracing::info!(
                    target: "collab.stage",
                    stage = "apply",
                    elapsed_us = apply_started.elapsed().as_micros() as u64,
                    document_id = %self.document_id,
                );
                if !primary_ok {
                    self.send_sync_status(conn_id, routing_key, true).await;
                    if let Some(c) = self.connections.get_mut(&conn_id) {
                        c.in_flight = false;
                    }
                    self.fatal_primary_unhealthy(actor_user_id, session_id)
                        .await;
                    return;
                }
                let broadcast_started = std::time::Instant::now();
                self.broadcast_update(&sync.y_protocol).await;
                tracing::info!(
                    target: "collab.stage",
                    stage = "broadcast",
                    elapsed_us = broadcast_started.elapsed().as_micros() as u64,
                    document_id = %self.document_id,
                );
                #[cfg(feature = "db-tests")]
                pause_for_append_projection_barrier(self.document_id).await;
                if let Some(c) = self.connections.get_mut(&conn_id) {
                    c.in_flight = false;
                }
                self.send_sync_status(conn_id, routing_key, true).await;
                match self
                    .maybe_project_derived_body(seq, actor_user_id, session_id, false)
                    .await
                {
                    ProjectDerivedOutcome::StaleWriter => {
                        self.fatal_writer_stale(CloseOrder::AfterQueued).await;
                        return;
                    }
                    _ if !self.primary_loaded => {
                        self.fatal_primary_unhealthy(actor_user_id, session_id)
                            .await;
                        return;
                    }
                    _ => {}
                }
                self.flush_connection_persist(conn_id, op_prefix).await;
                self.maybe_compact().await;
            }
        }
    }

    async fn locking_write_still_allowed(
        &self,
        user_id: Uuid,
        session_id: Uuid,
        read_only: bool,
    ) -> bool {
        matches!(
            self.locking_session_auth_by_ids(user_id, session_id, read_only)
                .await,
            LockingAuth::Allow
        )
    }

    async fn reject_candidate_engine_unavailable(&mut self, conn_id: Uuid) {
        if self.primary_dirty && !self.reload_primary_or_close_room().await {
            return;
        }
        if let Some(c) = self.connections.get_mut(&conn_id) {
            c.in_flight = false;
        }
        self.close_connection(conn_id, 1011, "engine unavailable")
            .await;
    }

    async fn reject_candidate(&mut self, conn_id: Uuid, routing_key: &str) {
        if let Some(user_id) = self.connections.get(&conn_id).map(|c| c.session.user_id) {
            self.record_user_rejected_candidate(user_id);
        }
        if self.primary_dirty && !self.reload_primary_or_close_room().await {
            return;
        }
        if let Some(c) = self.connections.get_mut(&conn_id) {
            c.in_flight = false;
            c.poisoned = true;
        }
        self.send_sync_status(conn_id, routing_key, false).await;
        self.close_connection_ordered(conn_id, 1008, "update rejected")
            .await;
    }

    async fn reconcile_ambiguous_append(
        &mut self,
        actor_user_id: Uuid,
        session_id: Uuid,
        op_id: Uuid,
        expected_tail: i64,
        payload: &[u8],
        digest: &[u8],
    ) -> Option<AppendCollabResult> {
        match verify_room_collab_operation(
            &self.backend,
            self.kind,
            VerifyCollabInput {
                workspace_id: self.workspace_id,
                actor_user_id,
                session_id,
                document_id: self.document_id,
                op_id,
                expected_payload_len: payload.len() as i64,
                expected_payload_sha256: digest,
                expected_actor_user_id: actor_user_id,
            },
            self.native_consumer_proof(),
        )
        .await
        {
            Ok(Ok(lookup)) => Some(AppendCollabResult::DuplicateAck { seq: lookup.seq }),
            Ok(Err(CollabDbError::NotFound)) => {
                match load_room_collab_readonly(
                    &self.backend,
                    self.kind,
                    self.workspace_id,
                    actor_user_id,
                    session_id,
                    self.document_id,
                    self.family_fence(),
                )
                .await
                {
                    Ok(Ok(load)) => {
                        // Receipt lookup returned NotFound; compare durable tail to the
                        // expected op_id before treating this as divergence.
                        if load.tail.iter().any(|r| r.op_id == op_id) {
                            self.fatal_room_divergence(actor_user_id, session_id).await;
                            return None;
                        }
                        if load.tail_seq > expected_tail {
                            self.fatal_room_divergence(actor_user_id, session_id).await;
                            return None;
                        }
                        None
                    }
                    Err(error) if self.is_remote_task_room() => {
                        self.fatal_remote_write_unconfirmed(error).await;
                        None
                    }
                    _ => {
                        self.fatal_room_divergence(actor_user_id, session_id).await;
                        None
                    }
                }
            }
            Ok(Err(CollabDbError::StaleWriter | CollabDbError::StaleCutoff)) => {
                self.fatal_room_divergence(actor_user_id, session_id).await;
                None
            }
            Err(error) if self.is_remote_task_room() => {
                self.fatal_remote_write_unconfirmed(error).await;
                None
            }
            Ok(Err(_)) | Err(_) => {
                self.fatal_room_divergence(actor_user_id, session_id).await;
                None
            }
        }
    }

    async fn fatal_writer_stale(&mut self, order: CloseOrder) {
        if !matches!(self.backend, Backend::Postgres(_)) {
            self.fatal_fence_lost().await;
            return;
        }
        self.writer_generation = None;
        // A newer writer owns durable state; `committed` may lag it, so the next
        // capture/join must reload instead of trusting the in-memory bundle.
        self.committed_loaded = false;
        for conn_id in self.connections.keys().cloned().collect::<Vec<_>>() {
            self.close_connection_in_order(conn_id, 1008, "writer stale", order)
                .await;
        }
    }

    async fn fatal_room_divergence(&mut self, actor_user_id: Uuid, session_id: Uuid) {
        if !matches!(self.backend, Backend::Postgres(_)) {
            self.fatal_fence_lost().await;
            return;
        }
        if let Ok(Ok(load)) = load_room_collab_readonly(
            &self.backend,
            self.kind,
            self.workspace_id,
            actor_user_id,
            session_id,
            self.document_id,
            self.family_fence(),
        )
        .await
        {
            self.set_committed_from_load(&load);
            let _ = self.reload_primary_from_committed().await;
        }
        self.writer_generation = None;
        for conn_id in self.connections.keys().cloned().collect::<Vec<_>>() {
            self.close_connection(conn_id, 1011, "room state diverged")
                .await;
        }
    }

    async fn fatal_primary_unhealthy(&mut self, actor_user_id: Uuid, session_id: Uuid) {
        if let Ok(Ok(load)) = load_room_collab_readonly(
            &self.backend,
            self.kind,
            self.workspace_id,
            actor_user_id,
            session_id,
            self.document_id,
            self.family_fence(),
        )
        .await
        {
            self.set_committed_from_load(&load);
            let _ = self.reload_primary_from_committed().await;
        }
        for conn_id in self.connections.keys().cloned().collect::<Vec<_>>() {
            self.close_connection_ordered(conn_id, 1011, "primary engine unhealthy")
                .await;
        }
    }

    async fn ensure_primary_capacity(&mut self) -> Result<(), JoinError> {
        if !self.primary_loaded || self.primary_dirty || self.engine.needs_recycle() {
            self.reload_primary_from_committed().await?;
        }
        Ok(())
    }

    /// Pre-commit admission on the room primary (no ephemeral validator spawn).
    async fn validate_candidate_on_primary(
        &mut self,
        payload: &[u8],
    ) -> (BundleValidation, ValidateStageTimings) {
        if payload.is_empty() {
            return (BundleValidation::Rejected, ValidateStageTimings::default());
        }
        if !self.engine.engine_bin().is_file() {
            return (
                BundleValidation::EngineUnavailable,
                ValidateStageTimings::default(),
            );
        }
        let apply_started = Instant::now();
        let apply_report = match self
            .engine
            .call(Request::Apply {
                update_b64: payload.to_vec(),
                encoding: 1,
            })
            .await
        {
            Ok(report) => report,
            Err(BridgeError::Dead) => {
                if !self.reload_primary_or_close_room().await {
                    return (
                        BundleValidation::EngineUnavailable,
                        ValidateStageTimings::default(),
                    );
                }
                return (
                    BundleValidation::EngineUnavailable,
                    ValidateStageTimings::default(),
                );
            }
        };
        let load_us = apply_started.elapsed().as_micros() as u64;
        let outcome = match classify_admission_load(&apply_report.outcome) {
            Ok(()) => {
                self.primary_dirty = true;
                BundleValidation::Ok
            }
            Err(outcome) => outcome,
        };
        if outcome != BundleValidation::Ok && !self.reload_primary_or_close_room().await {
            return (
                BundleValidation::EngineUnavailable,
                ValidateStageTimings { load_us },
            );
        }
        (outcome, ValidateStageTimings { load_us })
    }

    async fn maybe_project_derived_body(
        &mut self,
        seq: i64,
        actor_user_id: Uuid,
        session_id: Uuid,
        preemptive_stale_close: bool,
    ) -> ProjectDerivedOutcome {
        if seq < 1 {
            return ProjectDerivedOutcome::SkippedSeed;
        }
        let Some(writer_generation) = self.writer_generation else {
            tracing::warn!(
                target: "collab.derive_failed",
                document_id = %self.document_id,
                seq,
                reason = "no_writer",
                "collab derived body skipped"
            );
            return ProjectDerivedOutcome::EngineFailed;
        };
        if !self.primary_loaded || self.primary_dirty {
            tracing::warn!(
                target: "collab.derive_failed",
                document_id = %self.document_id,
                writer_generation,
                seq,
                reason = "primary_not_ready",
                "collab derived body skipped"
            );
            return ProjectDerivedOutcome::PrimaryNotReady;
        }
        if self.ensure_primary_capacity().await.is_err() {
            tracing::error!(
                target: "collab.derive_failed",
                document_id = %self.document_id,
                writer_generation,
                seq,
                reason = "primary_capacity",
                "collab derived body operational failure"
            );
            return ProjectDerivedOutcome::EngineFailed;
        }

        let report = match self.engine.call(Request::Project { encoding: 1 }).await {
            Ok(report) => report,
            Err(BridgeError::Dead) => {
                self.recover_primary_after_engine_fault().await;
                tracing::error!(
                    target: "collab.derive_failed",
                    document_id = %self.document_id,
                    writer_generation,
                    seq,
                    reason = "engine_dead",
                    "collab derived body operational failure"
                );
                return ProjectDerivedOutcome::EngineFailed;
            }
        };

        let (content_json, pending) = match report.outcome {
            EngineStatus::Ok {
                content_json: Some(json),
                pending,
                ..
            } => (json, pending),
            EngineStatus::Ok {
                content_json: None, ..
            } => {
                self.recover_primary_after_engine_fault().await;
                tracing::error!(
                    target: "collab.derive_failed",
                    document_id = %self.document_id,
                    writer_generation,
                    seq,
                    reason = "missing_content_json",
                    "collab derived body operational failure"
                );
                return ProjectDerivedOutcome::EngineFailed;
            }
            EngineStatus::Malformed { detail } => {
                self.recover_primary_after_engine_fault().await;
                tracing::error!(
                    target: "collab.derive_failed",
                    document_id = %self.document_id,
                    writer_generation,
                    seq,
                    reason = "malformed",
                    detail = %detail,
                    "collab derived body operational failure"
                );
                return ProjectDerivedOutcome::EngineFailed;
            }
            EngineStatus::Unsupported { detail, .. } => {
                self.recover_primary_after_engine_fault().await;
                tracing::error!(
                    target: "collab.derive_failed",
                    document_id = %self.document_id,
                    writer_generation,
                    seq,
                    reason = "unsupported",
                    detail = %detail,
                    "collab derived body operational failure"
                );
                return ProjectDerivedOutcome::EngineFailed;
            }
            EngineStatus::ResourceLimit { kind, detail } => {
                if Self::is_deterministic_project_limit(kind) {
                    self.recover_primary_after_engine_fault().await;
                    tracing::warn!(
                        target: "collab.derive_failed",
                        document_id = %self.document_id,
                        writer_generation,
                        seq,
                        reason = "resource_limit",
                        limit_kind = ?kind,
                        detail = %detail,
                        "collab derived body skipped"
                    );
                    return ProjectDerivedOutcome::DeterministicSkip;
                }
                self.recover_primary_after_engine_fault().await;
                tracing::error!(
                    target: "collab.derive_failed",
                    document_id = %self.document_id,
                    writer_generation,
                    seq,
                    reason = "engine_operational",
                    limit_kind = ?kind,
                    detail = %detail,
                    "collab derived body operational failure"
                );
                return ProjectDerivedOutcome::EngineFailed;
            }
            EngineStatus::WorkerFailure { detail, .. } => {
                self.recover_primary_after_engine_fault().await;
                tracing::error!(
                    target: "collab.derive_failed",
                    document_id = %self.document_id,
                    writer_generation,
                    seq,
                    reason = "engine_operational",
                    detail = %detail,
                    "collab derived body operational failure"
                );
                return ProjectDerivedOutcome::EngineFailed;
            }
        };

        let prepared = match prepare_derived_body(content_json) {
            Ok(prepared) => prepared,
            Err(err) => {
                tracing::warn!(
                    target: "collab.derive_failed",
                    document_id = %self.document_id,
                    writer_generation,
                    seq,
                    reason = "prepare_failed",
                    error = ?err,
                    "collab derived body skipped"
                );
                return ProjectDerivedOutcome::DeterministicSkip;
            }
        };

        match project_derived_body_kind_backend(
            &self.backend,
            self.kind,
            ProjectDerivedBodyInput::new(
                self.workspace_id,
                actor_user_id,
                session_id,
                self.document_id,
                writer_generation,
                seq,
                prepared,
            ),
            self.family_fence(),
        )
        .await
        {
            Ok(Ok(ProjectDerivedBodyResult::Updated)) => {
                tracing::info!(
                    target: "collab.derive",
                    document_id = %self.document_id,
                    writer_generation,
                    seq,
                    pending,
                    result = "updated",
                    "collab derived body projected"
                );
                ProjectDerivedOutcome::Projected
            }
            Ok(Ok(ProjectDerivedBodyResult::Unchanged)) => {
                tracing::info!(
                    target: "collab.derive",
                    document_id = %self.document_id,
                    writer_generation,
                    seq,
                    pending,
                    result = "unchanged",
                    "collab derived body unchanged"
                );
                ProjectDerivedOutcome::Unchanged
            }
            Ok(Ok(ProjectDerivedBodyResult::SkippedSeed)) => ProjectDerivedOutcome::SkippedSeed,
            Ok(Err(CollabDbError::StaleWriter)) => {
                if preemptive_stale_close {
                    self.fatal_writer_stale(CloseOrder::Preempt).await;
                }
                ProjectDerivedOutcome::StaleWriter
            }
            Ok(Err(CollabDbError::StaleCutoff)) => ProjectDerivedOutcome::StaleCutoff,
            Ok(Err(CollabDbError::Forbidden | CollabDbError::NotFound)) => {
                ProjectDerivedOutcome::PermissionDenied
            }
            Ok(Err(err)) => {
                tracing::error!(
                    target: "collab.derive_failed",
                    document_id = %self.document_id,
                    writer_generation,
                    seq,
                    reason = "db_rejected",
                    error = ?err,
                    "collab derived body db failure"
                );
                ProjectDerivedOutcome::DbFailed
            }
            Err(err) => {
                tracing::error!(
                    target: "collab.derive_failed",
                    document_id = %self.document_id,
                    writer_generation,
                    seq,
                    reason = "db_operational",
                    error = %err,
                    "collab derived body db failure"
                );
                if self.is_remote_task_room() {
                    self.fatal_remote_write_unconfirmed(err).await;
                }
                ProjectDerivedOutcome::DbFailed
            }
        }
    }

    /// Only Project `Output` budget refusals are deterministic today. Memory,
    /// Stack, depth and node budgets share kinds with operational failures.
    fn is_deterministic_project_limit(kind: LimitKind) -> bool {
        matches!(kind, LimitKind::Output)
    }

    fn manual_persist_derived_ok(outcome: ProjectDerivedOutcome) -> bool {
        matches!(
            outcome,
            ProjectDerivedOutcome::Projected
                | ProjectDerivedOutcome::Unchanged
                | ProjectDerivedOutcome::SkippedSeed
                | ProjectDerivedOutcome::DeterministicSkip
        )
    }

    fn manual_persist_projection_failed(outcome: ProjectDerivedOutcome) -> bool {
        !Self::manual_persist_derived_ok(outcome)
    }

    async fn integrate_committed_update(
        &mut self,
        payload: &[u8],
        admission_applied: bool,
    ) -> Result<(), JoinError> {
        if self.engine.needs_recycle() {
            return self.reload_primary_from_committed().await;
        }
        if admission_applied {
            #[cfg(feature = "db-tests")]
            if consume_force_primary_apply_fail(self.document_id).await {
                return self.reload_primary_from_committed().await;
            }
            self.primary_dirty = false;
            self.primary_loaded = true;
            return Ok(());
        }
        self.primary_dirty = true;
        if self.apply_primary(payload).await {
            self.primary_dirty = false;
            return Ok(());
        }
        self.reload_primary_from_committed().await
    }

    async fn apply_primary(&mut self, payload: &[u8]) -> bool {
        #[cfg(feature = "db-tests")]
        if consume_force_primary_apply_fail(self.document_id).await {
            return false;
        }
        let report = match self
            .engine
            .call(Request::Apply {
                update_b64: payload.to_vec(),
                encoding: 1,
            })
            .await
        {
            Ok(report) => report,
            Err(BridgeError::Dead) => return false,
        };
        report.outcome.is_applied_ok()
    }

    async fn reload_primary_from_committed(&mut self) -> Result<(), JoinError> {
        if let Err(err) = self.engine.recycle().await {
            self.primary_loaded = false;
            self.primary_dirty = true;
            return Err(match err {
                // Every primary slot taken is transient: the client retries
                // (1013) and the next reload spawns again. Debug, like the
                // transport's capacity refusals: a line per retry would grow
                // with the waiting clients.
                RecycleError::Spawn(report) if is_slot_cap_refusal(&report) => {
                    tracing::debug!(
                        workspace_id = %self.workspace_id,
                        document_id = %self.document_id,
                        "collab primary helper spawn refused at capacity"
                    );
                    JoinError::CapacityRetry
                }
                RecycleError::Spawn(report) => {
                    warn_engine_not_applied(
                        "room.reload_primary_spawn",
                        Some(self.workspace_id),
                        Some(self.document_id),
                        &report.outcome,
                    );
                    JoinError::EngineUnavailable
                }
                RecycleError::Dead => {
                    tracing::warn!(
                        workspace_id = %self.workspace_id,
                        document_id = %self.document_id,
                        "collab primary recycle failed: engine bridge dead"
                    );
                    JoinError::EngineUnavailable
                }
            });
        }
        match self.load_engine_primary().await {
            Ok(()) => {
                self.primary_loaded = true;
                self.primary_dirty = false;
                // The loaded helper's RSS is now in the live-child sum.
                self.admission_reservation = None;
                Ok(())
            }
            Err(err) => {
                self.primary_loaded = false;
                self.primary_dirty = true;
                Err(err)
            }
        }
    }

    async fn load_engine_primary(&mut self) -> Result<(), JoinError> {
        #[cfg(feature = "db-tests")]
        if should_fail_primary_load(self.document_id).await {
            return Err(JoinError::EngineUnavailable);
        }
        let tail_b64 = self.committed.tail_payloads.clone();
        let report = self
            .engine
            .call(Request::Load {
                snapshot_b64: Some(self.committed.snapshot.clone()),
                tail_b64,
                encoding: 1,
            })
            .await
            .map_err(|_| {
                tracing::warn!(
                    workspace_id = %self.workspace_id,
                    document_id = %self.document_id,
                    "collab primary load failed: engine bridge dead"
                );
                JoinError::EngineUnavailable
            })?;
        if report.outcome.is_applied_ok() {
            Ok(())
        } else {
            warn_engine_not_applied(
                "room.load_engine_primary",
                Some(self.workspace_id),
                Some(self.document_id),
                &report.outcome,
            );
            Err(JoinError::EngineUnavailable)
        }
    }

    async fn handle_awareness(
        &mut self,
        conn_id: Uuid,
        client_id: u32,
        session: &CollabSession,
        conn_generation: u64,
        payload: Vec<u8>,
    ) {
        let updates = decode_awareness(&payload).unwrap_or_default();
        let display = crate::collab::awareness::display_name(
            &session.given_name,
            session.family_name.as_deref(),
            &session.locale,
        );
        let color = crate::collab::awareness::user_color(&session.user_id);
        if let Some(encoded) = self.awareness.apply_connection_updates(
            client_id,
            &updates,
            &session.user_id.to_string(),
            &display,
            &color,
            conn_generation,
        ) {
            self.broadcast_awareness(&encoded).await;
        }
        let _ = conn_id;
    }

    async fn handle_stateless(&mut self, conn_id: Uuid, payload: String) {
        if let Some(request_id) = payload.strip_prefix("persist:") {
            if let Ok(id) = Uuid::parse_str(request_id) {
                let prefix = self.fifo_seq;
                if let Some(conn) = self.connections.get_mut(&conn_id) {
                    conn.pending_persist.push_back(PersistBarrier {
                        request_id: id,
                        prefix_fifo: prefix,
                    });
                }
                self.flush_connection_persist(conn_id, self.fifo_seq).await;
            }
        }
    }

    async fn flush_connection_persist(&mut self, conn_id: Uuid, current_fifo: u64) {
        let ready = {
            let Some(conn) = self.connections.get_mut(&conn_id) else {
                return;
            };
            conn.pending_persist
                .iter()
                .filter(|b| b.prefix_fifo <= current_fifo)
                .map(|b| b.request_id)
                .collect::<Vec<_>>()
        };
        for request_id in ready {
            if let Some(conn) = self.connections.get_mut(&conn_id) {
                conn.pending_persist.retain(|b| b.request_id != request_id);
            }
            let result = self.run_persist_for(conn_id, request_id).await;
            self.send_stateless(conn_id, result).await;
        }
    }

    fn any_pending_persist(&self) -> bool {
        self.connections
            .values()
            .any(|c| !c.pending_persist.is_empty())
    }

    async fn run_persist_for(&mut self, conn_id: Uuid, request_id: Uuid) -> String {
        let started = std::time::Instant::now();
        let result = self.run_persist_steps(conn_id, request_id, started).await;
        tracing::info!(
            target: "collab.stage",
            stage = "persist",
            elapsed_us = started.elapsed().as_micros() as u64,
            ok = result.starts_with("persisted:"),
            document_id = %self.document_id,
        );
        result
    }

    fn persist_step(&self, step: &'static str, started: std::time::Instant) {
        tracing::info!(
            target: "collab.stage",
            stage = "persist_step",
            step,
            elapsed_us = started.elapsed().as_micros() as u64,
            document_id = %self.document_id,
        );
    }

    async fn run_persist_steps(
        &mut self,
        conn_id: Uuid,
        request_id: Uuid,
        started: std::time::Instant,
    ) -> String {
        let Some(conn) = self.connections.get(&conn_id) else {
            return format!("persist-failed:{request_id}");
        };
        if conn.poisoned || conn.in_flight {
            return format!("persist-failed:{request_id}");
        }
        let actor_user_id = conn.session.user_id;
        let session_id = conn.session.session_id;
        if !self
            .locking_write_still_allowed(actor_user_id, session_id, conn.read_only)
            .await
        {
            return format!("persist-failed:{request_id}");
        }
        if conn.read_only {
            return format!("persisted:{request_id}");
        }

        if self.ensure_primary_capacity().await.is_err() || !self.primary_loaded {
            self.compact_unhealthy = true;
            self.compact_retry_at_tail_len = Some(self.committed.tail_payloads.len());
            return format!("persist-failed:{request_id}");
        }

        self.persist_step("allowed", started);
        let snapshot_report = match self.engine.call(Request::Snapshot).await {
            Ok(report) => report,
            Err(BridgeError::Dead) => {
                self.compact_unhealthy = true;
                self.compact_retry_at_tail_len = Some(self.committed.tail_payloads.len());
                return format!("persist-failed:{request_id}");
            }
        };
        let snapshot = match snapshot_report.outcome {
            EngineStatus::Ok {
                update_b64: Some(bytes_b64),
                ..
            } => match b64::decode(&bytes_b64) {
                Ok(bytes) => bytes,
                Err(_) => {
                    self.compact_unhealthy = true;
                    self.compact_retry_at_tail_len = Some(self.committed.tail_payloads.len());
                    return format!("persist-failed:{request_id}");
                }
            },
            _ => {
                self.compact_unhealthy = true;
                self.compact_retry_at_tail_len = Some(self.committed.tail_payloads.len());
                return format!("persist-failed:{request_id}");
            }
        };
        self.persist_step("snapshot", started);
        if !validate_snapshot_only(
            self.engine.engine_bin().to_path_buf(),
            self.engine.limits(),
            snapshot.clone(),
        )
        .await
        {
            self.compact_unhealthy = true;
            self.compact_retry_at_tail_len = Some(self.committed.tail_payloads.len());
            return format!("persist-failed:{request_id}");
        }
        self.persist_step("validated", started);
        if let Some(writer_generation) = self.writer_generation {
            let cutoff = self.committed.tail_seq;
            let compact = compact_collab_snapshot_kind_backend(
                &self.backend,
                self.kind,
                CompactCollabInput {
                    workspace_id: self.workspace_id,
                    actor_user_id,
                    session_id,
                    document_id: self.document_id,
                    writer_generation,
                    cutoff_seq: cutoff,
                    expected_tail_seq: cutoff,
                    new_snapshot: &snapshot,
                    client_ip: None,
                },
                self.family_fence(),
            )
            .await;
            self.persist_step("compacted", started);
            match compact {
                Ok(Ok(load)) => {
                    self.compact_unhealthy = false;
                    self.compact_retry_at_tail_len = None;
                    self.set_committed_from_load(&load);
                    if self.engine.needs_recycle()
                        && self.reload_primary_from_committed().await.is_err()
                    {
                        self.compact_unhealthy = true;
                        self.compact_retry_at_tail_len = Some(self.committed.tail_payloads.len());
                        return format!("persist-failed:{request_id}");
                    }
                    if Self::manual_persist_projection_failed(
                        self.maybe_project_derived_body(
                            load.tail_seq,
                            actor_user_id,
                            session_id,
                            true,
                        )
                        .await,
                    ) {
                        self.compact_unhealthy = true;
                        self.compact_retry_at_tail_len = Some(self.committed.tail_payloads.len());
                        return format!("persist-failed:{request_id}");
                    }
                    format!("persisted:{request_id}")
                }
                Ok(Err(CollabDbError::StaleCutoff | CollabDbError::StaleWriter)) => {
                    self.fatal_room_divergence(actor_user_id, session_id).await;
                    format!("persist-failed:{request_id}")
                }
                Err(error) if self.is_remote_task_room() => {
                    self.fatal_remote_write_unconfirmed(error).await;
                    format!("persist-failed:{request_id}")
                }
                _ => {
                    self.compact_unhealthy = true;
                    self.compact_retry_at_tail_len = Some(self.committed.tail_payloads.len());
                    format!("persist-failed:{request_id}")
                }
            }
        } else {
            format!("persist-failed:{request_id}")
        }
    }

    async fn maybe_compact(&mut self) {
        if self.any_pending_persist() {
            return;
        }
        if self.compact_unhealthy {
            let retry = self.compact_retry_at_tail_len.is_some_and(|at_fail| {
                self.committed.tail_payloads.len() >= at_fail.saturating_add(8)
            });
            if !retry {
                return;
            }
        } else if self.committed.tail_payloads.len() < 32 {
            return;
        }
        let conn_id = self
            .connections
            .iter()
            .find(|(_, c)| !c.read_only && !c.pending_persist.is_empty())
            .map(|(id, _)| *id)
            .or_else(|| {
                self.connections
                    .iter()
                    .find(|(_, c)| !c.read_only)
                    .map(|(id, _)| *id)
            });
        if let Some(conn_id) = conn_id {
            let request_id = Uuid::now_v7();
            let _ = self.run_persist_for(conn_id, request_id).await;
        }
    }

    async fn handle_capture_revision(
        &mut self,
        actor_user_id: Uuid,
        session_id: Uuid,
    ) -> Result<GuardedCapturedRevision, RevisionCaptureError> {
        // Without a writer generation nothing fences out-of-room appends
        // (import, a newer writer), so the committed tail may be behind.
        if !self.committed_loaded || self.writer_generation.is_none() {
            let load = match load_room_collab_readonly(
                &self.backend,
                self.kind,
                self.workspace_id,
                actor_user_id,
                session_id,
                self.document_id,
                self.family_fence(),
            )
            .await
            {
                Ok(load) => load,
                Err(error) => {
                    if self.is_remote_task_room() {
                        self.fatal_remote_write_unconfirmed(error).await;
                    }
                    return Err(RevisionCaptureError::Unavailable);
                }
            };
            let load = load.map_err(|_| RevisionCaptureError::Unavailable)?;
            self.set_committed_from_load(&load);
            self.reload_primary_from_committed()
                .await
                .map_err(|_| RevisionCaptureError::Unavailable)?;
        }
        if self.ensure_primary_capacity().await.is_err() {
            return Err(RevisionCaptureError::Unavailable);
        }
        self.check_cached_native_consumer(actor_user_id, session_id)
            .await
            .map_err(|_| RevisionCaptureError::Unavailable)?;
        let snap = match self.engine.call(Request::RevisionSnapshot).await {
            Ok(report) => match report.outcome {
                EngineStatus::Ok {
                    update_b64: Some(bytes),
                    ..
                } => collab_engine::b64::decode(&bytes)
                    .map_err(|_| RevisionCaptureError::Unavailable)?,
                _ => return Err(RevisionCaptureError::Unavailable),
            },
            Err(_) => return Err(RevisionCaptureError::Unavailable),
        };
        let content_json = match self.engine.call(Request::Project { encoding: 1 }).await {
            Ok(report) => match report.outcome {
                EngineStatus::Ok {
                    content_json: Some(json),
                    ..
                } => json,
                _ => return Err(RevisionCaptureError::Unavailable),
            },
            Err(_) => return Err(RevisionCaptureError::Unavailable),
        };
        #[cfg(feature = "db-tests")]
        pause_native_consumer_barrier(self.document_id, NATIVE_CAPTURE_FINAL_PROOF).await;
        let proof = self
            .check_cached_native_consumer(actor_user_id, session_id)
            .await
            .map_err(|_| RevisionCaptureError::Unavailable)?;
        Ok(GuardedCapturedRevision {
            captured: CapturedRevision {
                y_snapshot: snap,
                content_json,
            },
            proof,
        })
    }

    async fn handle_restore(
        &mut self,
        actor_user_id: Uuid,
        session_id: Uuid,
        snap: Vec<u8>,
        intent: RestoreRevisionInput,
    ) -> Result<Uuid, RevisionRestoreError> {
        self.prepare_forward_writer(actor_user_id, session_id)
            .await
            .map_err(|err| match err {
                ForwardWriteError::Rejected => RevisionRestoreError::Rejected,
                _ => RevisionRestoreError::Unavailable,
            })?;
        // Recheck current authorization before recovering a previous commit.
        // Recovery precedes the old tail comparison: our own committed restore
        // must not invalidate a response-loss retry of that exact operation.
        if let Some(restored) = lookup_restored_revision(
            self.backend
                .postgres("native revision restore")
                .map_err(|_| RevisionRestoreError::Unavailable)?,
            self.workspace_id,
            actor_user_id,
            session_id,
            intent,
        )
        .await
        .map_err(|_| RevisionRestoreError::Unavailable)?
        .map_err(|err| match err {
            RevisionDbError::RestoreConflict => RevisionRestoreError::Conflict,
            _ => RevisionRestoreError::Rejected,
        })? {
            return Ok(restored.revision_id);
        }
        let applied = self
            .apply_forward_write(
                actor_user_id,
                session_id,
                Request::RestoreFromSnapshot {
                    snap_b64: snap,
                    encoding: 1,
                },
                Some(intent.expected_tail_seq),
                Some(intent),
            )
            .await
            .map_err(|err| match err {
                ForwardWriteError::Rejected | ForwardWriteError::AppendTooLarge => {
                    RevisionRestoreError::Rejected
                }
                ForwardWriteError::Conflict => RevisionRestoreError::Conflict,
                _ => RevisionRestoreError::Unavailable,
            })?;
        if let Some(seq) = applied.0 {
            let _ = self
                .maybe_project_derived_body(seq, actor_user_id, session_id, false)
                .await;
        }
        applied.1.ok_or(RevisionRestoreError::Unavailable)
    }

    /// Source `replaceLiveCollabContent`: the whole fragment becomes the seed
    /// Doc's fragment as one forward update, durable before broadcast, then the
    /// derived body is projected like any other committed update.
    async fn handle_replace_body(
        &mut self,
        actor_user_id: Uuid,
        session_id: Uuid,
        seed: Vec<u8>,
        expected_tail_seq: Option<i64>,
    ) -> Result<(), BodyWriteError> {
        let applied = self
            .apply_forward_write(
                actor_user_id,
                session_id,
                Request::ReplaceFromUpdate {
                    update_b64: seed,
                    encoding: 1,
                },
                expected_tail_seq,
                None,
            )
            .await
            .map_err(|err| match err {
                ForwardWriteError::Rejected => BodyWriteError::Rejected,
                ForwardWriteError::Unavailable => BodyWriteError::Unavailable,
                ForwardWriteError::Conflict => BodyWriteError::Conflict,
                ForwardWriteError::EngineLimit | ForwardWriteError::AppendTooLarge => {
                    BodyWriteError::TooLarge
                }
                ForwardWriteError::EngineMalformed => BodyWriteError::Invalid,
            })?;
        let Some(seq) = applied.0 else {
            return Ok(());
        };
        // The update is durable and broadcast at this point: the write has
        // happened. A failed derived-body projection is logged and re-derived
        // by the next update or persist (source `onDeriveFailed`), not
        // reported as a failed write.
        match self
            .maybe_project_derived_body(seq, actor_user_id, session_id, false)
            .await
        {
            ProjectDerivedOutcome::Projected | ProjectDerivedOutcome::Unchanged => {}
            other => {
                tracing::warn!(
                    document_id = %self.document_id,
                    outcome = ?other,
                    "collab.body_write_derive_failed"
                );
            }
        }
        Ok(())
    }

    async fn handle_project_live(
        &mut self,
        actor_user_id: Uuid,
        session_id: Uuid,
    ) -> Result<LiveProjection, BodyWriteError> {
        self.prepare_forward_writer(actor_user_id, session_id)
            .await
            .map_err(|err| match err {
                ForwardWriteError::Rejected => BodyWriteError::Rejected,
                _ => BodyWriteError::Unavailable,
            })?;
        let content_json = match self.engine.call(Request::Project { encoding: 1 }).await {
            Ok(report) => match report.outcome {
                EngineStatus::Ok {
                    content_json: Some(json),
                    ..
                } => json,
                EngineStatus::ResourceLimit { .. } => return Err(BodyWriteError::TooLarge),
                _ => return Err(BodyWriteError::Unavailable),
            },
            Err(_) => return Err(BodyWriteError::Unavailable),
        };
        #[cfg(feature = "db-tests")]
        pause_native_consumer_barrier(self.document_id, NATIVE_PROJECT_FINAL_PROOF).await;
        self.check_cached_native_consumer(actor_user_id, session_id)
            .await
            .map_err(|_| BodyWriteError::Unavailable)?;
        Ok(LiveProjection {
            content_json,
            tail_seq: self.committed.tail_seq,
        })
    }

    /// Claims the writer (loading committed state) when this room has none yet,
    /// then rechecks the actor's edit access under the document locks.
    async fn prepare_forward_writer(
        &mut self,
        actor_user_id: Uuid,
        session_id: Uuid,
    ) -> Result<(), ForwardWriteError> {
        if self.writer_generation.is_none() {
            let claim = self
                .claim_writer(actor_user_id, session_id)
                .await
                .map_err(|_| ForwardWriteError::Unavailable)?;
            let claim = claim.map_err(|err| match err {
                CollabDbError::Forbidden | CollabDbError::NotFound => ForwardWriteError::Rejected,
                _ => ForwardWriteError::Unavailable,
            })?;
            self.set_committed_from_load(&claim.load);
            self.reload_primary_from_committed()
                .await
                .map_err(|_| ForwardWriteError::Unavailable)?;
            self.writer_generation = Some(claim.writer_generation);
        } else if self.ensure_primary_capacity().await.is_err() {
            return Err(ForwardWriteError::Unavailable);
        }

        #[cfg(feature = "db-tests")]
        pause_for_append_revoke_barrier(self.document_id).await;

        match self
            .locking_session_auth_by_ids(actor_user_id, session_id, false)
            .await
        {
            LockingAuth::Allow => self
                .check_cached_native_consumer(actor_user_id, session_id)
                .await
                .map(|_| ())
                .map_err(|_| ForwardWriteError::Unavailable),
            LockingAuth::Deny => Err(ForwardWriteError::Rejected),
            LockingAuth::DbError => Err(ForwardWriteError::Unavailable),
        }
    }

    /// Computes a forward update with `request`, validates it against the
    /// committed bundle, appends it durably, integrates and broadcasts it.
    /// Returns the committed seq, or `None` when the update is empty.
    async fn apply_forward_write(
        &mut self,
        actor_user_id: Uuid,
        session_id: Uuid,
        request: Request,
        expected_tail_seq: Option<i64>,
        restore: Option<RestoreRevisionInput>,
    ) -> Result<(Option<i64>, Option<Uuid>), ForwardWriteError> {
        self.prepare_forward_writer(actor_user_id, session_id)
            .await?;
        if expected_tail_seq.is_some_and(|expected| expected != self.committed.tail_seq) {
            return Err(ForwardWriteError::Conflict);
        }

        let payload = match self.engine.call(request).await {
            Ok(report) => match report.outcome {
                EngineStatus::Ok {
                    applied: true,
                    update_b64: Some(bytes),
                    ..
                } => collab_engine::b64::decode(&bytes)
                    .map_err(|_| ForwardWriteError::Unavailable)?,
                EngineStatus::ResourceLimit { .. } => return Err(ForwardWriteError::EngineLimit),
                EngineStatus::Malformed { .. } => return Err(ForwardWriteError::EngineMalformed),
                _ => return Err(ForwardWriteError::Unavailable),
            },
            Err(_) => return Err(ForwardWriteError::Unavailable),
        };
        if is_empty_update(&payload) && restore.is_none() {
            #[cfg(feature = "db-tests")]
            pause_native_consumer_barrier(self.document_id, NATIVE_FORWARD_FINAL_PROOF).await;
            self.check_cached_native_consumer(actor_user_id, session_id)
                .await
                .map_err(|_| ForwardWriteError::Unavailable)?;
            return Ok((None, None));
        }

        let validation = validate_recovery_bundle(
            self.engine.engine_bin().to_path_buf(),
            self.engine.limits(),
            self.committed.snapshot.clone(),
            self.committed.tail_payloads.clone(),
            payload.clone(),
        )
        .await;
        if validation == BundleValidation::EngineUnavailable {
            return Err(ForwardWriteError::Unavailable);
        }
        if validation != BundleValidation::Ok {
            return Err(ForwardWriteError::Rejected);
        }

        let writer_generation = self
            .writer_generation
            .ok_or(ForwardWriteError::Unavailable)?;
        let op_id = Uuid::now_v7();
        let expected_tail = self.committed.tail_seq;
        let digest = payload_digest(&payload);
        // The engine computes restore payloads without mutating its primary
        // Doc. Capture the exact future committed bundle in the existing
        // isolated helper; never snapshot a later peer state after HTTP apply.
        let capture = if let Some(intent) = restore {
            let engine_bin = self.engine.engine_bin().to_path_buf();
            let limits = self.engine.limits();
            let snapshot = self.committed.snapshot.clone();
            let mut tail = self.committed.tail_payloads.clone();
            tail.push(payload.clone());
            let captured = tokio::task::spawn_blocking(move || {
                capture_revision_offline(engine_bin, limits, snapshot, tail)
            })
            .await
            .map_err(|_| ForwardWriteError::Unavailable)?
            .map_err(|_| ForwardWriteError::Unavailable)?;
            let prepared_body = prepare_derived_body(captured.content_json)
                .map_err(|_| ForwardWriteError::EngineMalformed)?;
            Some(RestoreRevisionAppend {
                intent,
                revision_id: Uuid::now_v7(),
                y_snapshot: captured.y_snapshot,
                prepared_body,
            })
        } else {
            None
        };
        let input = AppendCollabInput {
            workspace_id: self.workspace_id,
            actor_user_id,
            session_id,
            document_id: self.document_id,
            writer_generation,
            expected_tail_seq: expected_tail,
            op_id,
            payload: &payload,
            client_ip: None,
        };
        let append = if let Some(capture) = capture.as_ref() {
            match self.backend.postgres("native revision restore") {
                Ok(pool) => append_collab_restore_kind(pool, self.kind, input, capture).await,
                Err(error) => Err(error),
            }
            .map(|result| result.map(|(append, revision_id)| (append, Some(revision_id))))
        } else {
            match self.room_guard.as_mut() {
                Some(guard) => guard
                    .append(self.kind, input)
                    .await
                    .map(|(result, _)| result.map(|append| (append, None))),
                None => Ok(Err(CollabDbError::StaleWriter)),
            }
        };

        let mut own_receipt_verified = false;
        let (committed, restored_revision_id) = match append {
            Ok(Ok(result)) => result,
            Ok(Err(CollabDbError::StaleWriter)) => {
                self.fatal_writer_stale(CloseOrder::Preempt).await;
                return Err(ForwardWriteError::Unavailable);
            }
            Ok(Err(CollabDbError::PayloadTooLarge | CollabDbError::StateBudgetExceeded)) => {
                return Err(ForwardWriteError::AppendTooLarge);
            }
            Ok(Err(CollabDbError::OpIdConflict | CollabDbError::StaleCutoff))
                if restore.is_some() =>
            {
                return Err(ForwardWriteError::Conflict);
            }
            Ok(Err(err)) if Self::is_definite_append_rejection(&err) => {
                return Err(ForwardWriteError::Rejected);
            }
            Err(error) if matches!(self.backend, Backend::LibsqlRemote(_)) => {
                self.fatal_remote_write_unconfirmed(error).await;
                return Err(ForwardWriteError::Unavailable);
            }
            Ok(Err(CollabDbError::StaleCutoff)) | Ok(Err(_)) | Err(_) => {
                match self
                    .reconcile_ambiguous_append(
                        actor_user_id,
                        session_id,
                        op_id,
                        expected_tail,
                        &payload,
                        &digest,
                    )
                    .await
                {
                    Some(result) => {
                        // Reconciliation verified our operation UUID, actor,
                        // length and digest. These are the exact committed bytes,
                        // unlike a correlation replay found before this append.
                        own_receipt_verified = true;
                        (result, capture.as_ref().map(|capture| capture.revision_id))
                    }
                    None => return Err(ForwardWriteError::Unavailable),
                }
            }
        };

        if restore.is_some() && !own_receipt_verified {
            if let AppendCollabResult::DuplicateAck { seq } = committed {
                // A concurrently recovered replay already owns its exact payload.
                // Reload durable bytes; do not broadcast our speculative payload.
                let load = load_room_collab_readonly(
                    &self.backend,
                    self.kind,
                    self.workspace_id,
                    actor_user_id,
                    session_id,
                    self.document_id,
                    self.family_fence(),
                )
                .await
                .map_err(|_| ForwardWriteError::Unavailable)?
                .map_err(|_| ForwardWriteError::Rejected)?;
                self.set_committed_from_load(&load);
                self.reload_primary_from_committed()
                    .await
                    .map_err(|_| ForwardWriteError::Unavailable)?;
                return Ok((Some(seq), restored_revision_id));
            }
        }

        let seq = match committed {
            AppendCollabResult::Committed { seq } | AppendCollabResult::DuplicateAck { seq } => {
                if seq != expected_tail + 1 {
                    self.fatal_room_divergence(actor_user_id, session_id).await;
                    return Err(ForwardWriteError::Unavailable);
                }
                seq
            }
        };

        if self.committed.tail_seq < seq {
            self.committed.tail_payloads.push(payload.clone());
        }
        self.committed.tail_seq = seq;
        self.fifo_seq += 1;

        if self
            .integrate_committed_update(&payload, false)
            .await
            .is_err()
        {
            self.fatal_primary_unhealthy(actor_user_id, session_id)
                .await;
            return Err(ForwardWriteError::Unavailable);
        }
        let y_protocol = encode_sync_payload(SyncStep::Update, &payload);
        self.broadcast_update(&y_protocol).await;
        if own_receipt_verified {
            if let Some(intent) = restore {
                // Receipt verification permits durable convergence under the
                // existing room read policy. It is not current write permission:
                // a demoted/revoked actor must not receive restore success.
                let recovered = lookup_restored_revision(
                    self.backend
                        .postgres("native revision restore")
                        .map_err(|_| ForwardWriteError::Unavailable)?,
                    self.workspace_id,
                    actor_user_id,
                    session_id,
                    intent,
                )
                .await
                .map_err(|_| ForwardWriteError::Unavailable)?
                .map_err(|_| ForwardWriteError::Rejected)?;
                if recovered.map(|value| value.revision_id) != restored_revision_id {
                    return Err(ForwardWriteError::Unavailable);
                }
            }
        }
        Ok((Some(seq), restored_revision_id))
    }

    async fn broadcast_update(&mut self, y_protocol: &[u8]) {
        let recipients = self
            .connections
            .iter()
            .map(|(id, conn)| (*id, conn.routing_key.clone()))
            .collect::<Vec<_>>();
        for (conn_id, routing_key) in recipients {
            let frame = encode(&WireFrame::Document {
                routing_key,
                room: None,
                message: DocumentMessage::Sync(crate::collab::wire::SyncMessage {
                    step: SyncStep::Update,
                    y_protocol: y_protocol.to_vec(),
                }),
            })
            .unwrap_or_default();
            self.deliver_outbound(conn_id, frame, OutboundKind::Data)
                .await;
        }
        self.flush_pending_awareness().await;
    }

    async fn broadcast_awareness(&mut self, encoded: &[u8]) {
        self.pending_awareness.push_back(encoded.to_vec());
        self.flush_pending_awareness().await;
    }

    async fn flush_pending_awareness(&mut self) {
        if self.flushing_awareness {
            return;
        }
        self.flushing_awareness = true;
        while let Some(encoded) = self.pending_awareness.pop_front() {
            let recipients = self
                .connections
                .iter()
                .map(|(id, conn)| (*id, conn.routing_key.clone()))
                .collect::<Vec<_>>();
            for (conn_id, routing_key) in recipients {
                let frame = encode(&WireFrame::Document {
                    routing_key,
                    room: None,
                    message: DocumentMessage::Awareness(encoded.clone()),
                })
                .unwrap_or_default();
                self.deliver_outbound(conn_id, frame, OutboundKind::Data)
                    .await;
            }
        }
        self.flushing_awareness = false;
    }

    /// 1009 eviction from `deliver_outbound`. The tombstone is left for the caller's awareness
    /// flush (this runs inside it); the session revision is scheduled like any other close.
    async fn evict_for_backpressure(&mut self, conn_id: Uuid) {
        if let Some(tombstone) = self
            .evict_connection(conn_id, 1009, "outbound queue full", CloseOrder::Preempt)
            .await
        {
            self.pending_awareness.push_back(tombstone);
        }
        self.try_schedule_session_revision();
    }

    async fn deliver_outbound(&mut self, conn_id: Uuid, bytes: Vec<u8>, kind: OutboundKind) {
        let Some(events) = self.connections.get(&conn_id).map(|c| c.events.clone()) else {
            return;
        };
        // Transport teardown closes this receiver before the actor drains its
        // admitted input prefix. It is not a slow peer: Leave/lease retirement
        // already orders cleanup after that prefix. Evicting here during an
        // earlier update's broadcast would discard the queued final edit.
        if events.is_closed() {
            return;
        }
        let accounted_bytes = bytes.len().saturating_add(OUTBOUND_FRAME_OVERHEAD);
        let budget = self
            .connections
            .get(&conn_id)
            .map(|conn| conn.outbound_budget.clone());
        let Some(budget) = budget else {
            return;
        };
        if accounted_bytes > budget.max_bytes {
            self.evict_for_backpressure(conn_id).await;
            return;
        }
        let current = budget.queued_bytes.load(Ordering::Relaxed);
        if current + accounted_bytes > budget.max_bytes {
            self.evict_for_backpressure(conn_id).await;
            return;
        }
        let frame_permit = match budget.frame_sem.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                self.evict_for_backpressure(conn_id).await;
                return;
            }
        };
        budget
            .queued_bytes
            .fetch_add(accounted_bytes, Ordering::Relaxed);
        let delivery_permit = OutboundDeliveryPermit {
            frame_permit,
            accounted_bytes,
            budget,
        };
        let frame = OutboundFrame {
            bytes,
            kind,
            permit: Some(delivery_permit),
        };
        // The receiver can close after the check above. A full live channel
        // still enforces the outbound budget and preempts the slow peer.
        if let Err(mpsc::error::TrySendError::Full(_)) =
            events.try_send(RoomClientEvent::Outbound(frame))
        {
            self.evict_for_backpressure(conn_id).await;
        }
    }

    async fn deliver_document_message(
        &mut self,
        conn_id: Uuid,
        routing_key: &str,
        message: DocumentMessage,
    ) {
        if let Ok(bytes) = encode(&WireFrame::Document {
            routing_key: routing_key.to_string(),
            room: None,
            message,
        }) {
            self.deliver_outbound(conn_id, bytes, OutboundKind::Data)
                .await;
            self.flush_pending_awareness().await;
        }
    }

    async fn send_sync_status(&mut self, conn_id: Uuid, routing_key: &str, applied: bool) {
        self.deliver_document_message(
            conn_id,
            routing_key,
            DocumentMessage::SyncStatus { applied },
        )
        .await;
    }

    async fn send_stateless(&mut self, conn_id: Uuid, payload: String) {
        if let Some(conn) = self.connections.get(&conn_id) {
            let routing_key = conn.routing_key.clone();
            self.deliver_document_message(
                conn_id,
                &routing_key,
                DocumentMessage::Stateless(payload),
            )
            .await;
        }
    }
}

fn payload_digest(payload: &[u8]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(payload);
    hasher.finalize().to_vec()
}

pub fn parse_client_id(token: &str) -> Option<u32> {
    if token.is_empty() || token.len() > 10 || !token.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let value = token.parse::<u64>().ok()?;
    if value > 0xFFFF_FFFF {
        return None;
    }
    Some(value as u32)
}

#[cfg(test)]
mod project_limit_tests {
    use collab_engine::outcome::LimitKind;

    use super::RoomActor;

    #[test]
    fn deterministic_project_limit_classifier() {
        assert!(
            RoomActor::is_deterministic_project_limit(LimitKind::Output),
            "Project Output budget is the only deterministic limit today"
        );
        for kind in [
            LimitKind::Memory,
            LimitKind::Stack,
            LimitKind::Ops,
            LimitKind::Input,
            LimitKind::Frame,
            LimitKind::Time,
        ] {
            assert!(
                !RoomActor::is_deterministic_project_limit(kind),
                "{kind:?} must stay operational"
            );
        }
    }
}

#[cfg(all(test, feature = "db-tests"))]
mod remote_task_finish_tests {
    use super::*;
    use crate::collab::config::FamilyRoomTimings;
    use crate::collab::guard::FamilyRoomOwnerRecord;
    use crate::collab::hub::CollabHub;
    use crate::collab::seed::SeedEngine;
    use crate::collab::wire::{decode, SyncMessage};
    use crate::db::attachment_preview::tests::Fixture;
    use crate::db::backend::{
        maintenance_claim_driver_tests::Fixture as DriverFixture, CommitCleanupUnknown,
        CommitSettlement,
    };
    use crate::db::tasks::selected_task_detail_tests::setup;

    // The pinned SDK describes the original SQL, then executes its maintained
    // parser's canonical SQL. Match only that actual execution representation,
    // with the complete projection, target and predicates; never Describe.
    const RELEASE: &str = "UPDATE task_collab_room_fences SET expires_at = ?5 WHERE workspace_id = ?1 AND task_id = ?2 AND owner_token = ?3 AND fence = ?4;";
    const RECEIPT: &str = "SELECT seq, payload_len, payload_sha256, actor_user_id FROM task_collab_op_receipts WHERE workspace_id = ?1 AND task_id = ?2 AND op_id = ?3;";
    const LOAD: &str = "SELECT state, encoding, writer_generation, snapshot_cutoff_seq, tail_seq, updated_at FROM task_states WHERE workspace_id = ?1 AND task_id = ?2;";

    #[test]
    fn task_finish_selectors_match_exact_sdk_execution_and_refuse_other_statements() {
        // Immutable 46da diagnostic cursor272, with original-stream COMMIT+Close283.
        let observed_receipt = "SELECT seq, payload_len, payload_sha256, actor_user_id FROM task_collab_op_receipts WHERE workspace_id = ?1 AND task_id = ?2 AND op_id = ?3;";
        assert!(observed_receipt.contains(RECEIPT));
        assert!(!"SELECT seq,payload_len,payload_sha256,actor_user_id FROM task_collab_op_receipts WHERE workspace_id=?1 AND task_id=?2 AND op_id=?3".contains(RECEIPT), "Describe is not executed receipt SQL");
        for other in [
            observed_receipt.replace("task_collab_op_receipts", "document_collab_op_receipts"),
            observed_receipt.replace("task_collab_op_receipts", "task_collab_op_receipts_extra"),
            observed_receipt.replace("seq, payload_len", "payload_len, seq"),
            observed_receipt.replace("op_id = ?3", "op_id = ?4"),
            observed_receipt.replace("AND op_id = ?3", "AND op_id != ?3"),
        ] {
            assert!(
                !other.contains(RECEIPT),
                "different receipt operation cannot arm this fault"
            );
        }
        // Same pinned parser rendering of the existing full Task load and release
        // SQL; their actual runtime paths still require separate allocation.
        let canonical_load = "SELECT state, encoding, writer_generation, snapshot_cutoff_seq, tail_seq, updated_at FROM task_states WHERE workspace_id = ?1 AND task_id = ?2;";
        assert!(canonical_load.contains(LOAD));
        assert!(!canonical_load
            .replace("task_states", "document_states")
            .contains(LOAD));
        assert!(!canonical_load
            .replace("task_id = ?2", "task_id = ?3")
            .contains(LOAD));
        assert!(!canonical_load
            .replace("AND task_id = ?2", "AND task_id != ?2")
            .contains(LOAD));
        let canonical_release = "UPDATE task_collab_room_fences SET expires_at = ?5 WHERE workspace_id = ?1 AND task_id = ?2 AND owner_token = ?3 AND fence = ?4;";
        assert_eq!(canonical_release, RELEASE);
        assert_ne!(
            canonical_release.replace("owner_token = ?3", "owner_token = ?6"),
            RELEASE
        );
        // Observed 46da renewal has an extra DB-clock live-owner predicate and is
        // not a release. Exact equality keeps the no-fresh-release oracle specific.
        assert_ne!("UPDATE task_collab_room_fences SET expires_at = ?5 WHERE workspace_id = ?1 AND task_id = ?2 AND owner_token = ?3 AND fence = ?4 AND expires_at > ?6;", RELEASE);
    }

    // Concrete normal Task consumer fixture. SQL is executed by the maintained
    // SQLite engine through the pinned SDK, native bytes by the actual child.
    // This is not production TLS or an actual external Turso qualification.
    struct TaskRoom {
        f: Fixture,
        driver: DriverFixture,
        hub: CollabHub,
        credential: Uuid,
        key: RoomKey,
        conn: Uuid,
        lease: ConnectionLease,
        handle: RoomHandle,
        events: mpsc::Receiver<RoomClientEvent>,
        cancel: watch::Receiver<Option<ConnectionCancel>>,
        payload: Vec<u8>,
        body: serde_json::Value,
    }

    async fn join(
        hub: &CollabHub,
        f: &Fixture,
        credential: Uuid,
        key: RoomKey,
    ) -> (
        Uuid,
        ConnectionLease,
        mpsc::Receiver<RoomClientEvent>,
        watch::Receiver<Option<ConnectionCancel>>,
    ) {
        let conn = Uuid::now_v7();
        let (events, receive) = mpsc::channel(64);
        let (cancel, cancelled) = watch::channel(None);
        let lease = hub
            .join_room(
                key,
                RoomJoin {
                    conn: AuthenticatedConnection {
                        conn_id: conn,
                        session: CollabSession {
                            session_id: credential,
                            user_id: f.user,
                            given_name: "Task writer".into(),
                            family_name: None,
                            locale: "en".into(),
                        },
                        client_id: 701,
                        read_only: false,
                        routing_key: format!("{}:task:{}", key.0, key.1),
                    },
                    events,
                    cancel: Some(cancel),
                },
            )
            .await
            .expect("actual current Task admission");
        (conn, lease, receive, cancelled)
    }

    impl TaskRoom {
        async fn new() -> Self {
            let (f, credential, _, _, task) = setup().await;
            let driver = DriverFixture::for_existing_path(&f.path).await;
            let config = CollabConfig::from_env()
                .expect("root must allocate the freshly qualified native engine; no fallback/skip");
            let hub = CollabHub::new_backend(
                config,
                driver.backend(),
                Some(FamilyRoomTimings::new(30_000, 5_000).unwrap()),
            )
            .unwrap();
            let key = RoomKey::task(f.workspace, task);
            let body = serde_json::json!({"type":"doc","content":[{"type":"paragraph","attrs":{"id":"89b4972a-8e36-4989-8899-23375f632fa1"},"content":[{"type":"text","text":"원래 Task 中 😀","marks":[{"type":"bold","attrs":{}}]}]}]});
            let payload = SeedEngine::from_hub(&hub)
                .tiptap_to_yjs_update(&body)
                .await
                .unwrap();
            assert!(!is_empty_update(&payload));
            let (conn, lease, mut events, cancel) = join(&hub, &f, credential, key).await;
            let handle = hub.ensure_live_room(key).await.unwrap();
            while events.try_recv().is_ok() {}
            Self {
                f,
                driver,
                hub,
                credential,
                key,
                conn,
                lease,
                handle,
                events,
                cancel,
                payload,
                body,
            }
        }
        async fn sync(&mut self) {
            let frame = encode(&WireFrame::Document {
                routing_key: format!("{}:task:{}", self.key.0, self.key.1),
                room: None,
                message: DocumentMessage::Sync(SyncMessage {
                    step: SyncStep::Update,
                    y_protocol: encode_sync_payload(SyncStep::Update, &self.payload),
                }),
            })
            .unwrap();
            self.handle.frame(self.conn, frame).await;
            self.handle.probe().await; // Actual actor mailbox barrier, no polling retry.
        }
        fn drain_status(&mut self) -> (usize, usize) {
            let (mut applied, mut rejected) = (0, 0);
            while let Ok(event) = self.events.try_recv() {
                let RoomClientEvent::Outbound(frame) = event else {
                    continue;
                };
                let WireFrame::Document {
                    message: DocumentMessage::SyncStatus { applied: ok },
                    ..
                } = decode(&frame.bytes).unwrap()
                else {
                    continue;
                };
                if ok {
                    applied += 1;
                } else {
                    rejected += 1;
                }
            }
            (applied, rejected)
        }
        async fn durable(&self) -> (i64, i64, i64) {
            sqlx::query_as("SELECT (SELECT tail_seq FROM task_states WHERE workspace_id=?1 AND task_id=?2),(SELECT count(*) FROM task_collab_updates WHERE workspace_id=?1 AND task_id=?2),(SELECT count(*) FROM task_collab_op_receipts WHERE workspace_id=?1 AND task_id=?2)")
                .bind(self.key.0.as_bytes().as_slice()).bind(self.key.1.as_bytes().as_slice()).fetch_one(&self.f.pool).await.unwrap()
        }
        fn assert_retained(&self, active: bool, fk: bool) -> Arc<sqlx::Error> {
            let expected = self.lease.family_room_delivery.expect("actual typed lease");
            let expected = if active {
                expected.original.with_owner(expected.writer_owner)
            } else {
                expected.original
            };
            let records = self.hub.family_owner_records_for_test();
            let records = records.lock().unwrap();
            assert_eq!(records.len(), 1);
            let FamilyRoomOwnerRecord::NativeWrite { fence, error } = records
                .get(&self.key)
                .expect("same hub retains this uncertain original owner")
            else {
                panic!("native uncertainty is not fabricated startup ownership")
            };
            assert_eq!(*fence, expected);
            assert_eq!(fence.kind(), CollabKind::Task);
            assert_eq!(
                (fence.workspace(), fence.resource()),
                (self.key.0, self.key.1)
            );
            let sqlx::Error::AnyDriverError(original) = error.as_ref() else {
                panic!("original SDK finish error retained")
            };
            let unknown = original
                .downcast_ref::<CommitCleanupUnknown>()
                .expect("typed original COMMIT error, not a replacement protocol error");
            assert_eq!(unknown.settlement, CommitSettlement::RemoteUnconfirmed);
            assert!(!unknown.permits_reconciliation());
            assert!(unknown.cleanup_error.is_none());
            let sqlx::Error::AnyDriverError(source) = &unknown.source.source else {
                panic!("SDK original source retained")
            };
            assert!(source.downcast_ref::<libsql::Error>().is_some());
            if fk {
                assert!(source.to_string().contains("FOREIGN KEY constraint failed"));
            }
            error.clone()
        }
        async fn assert_blocked(&mut self, active: bool, fk: bool) {
            assert_eq!(
                self.drain_status().0,
                0,
                "uncertain COMMIT cannot send success acknowledgement"
            );
            assert_eq!(
                self.cancel
                    .borrow()
                    .as_ref()
                    .expect("transport cancelled")
                    .code,
                1013
            );
            let original = self.assert_retained(active, fk);
            assert!(
                !self.driver.sql_log().await.iter().any(|sql| sql == RELEASE),
                "no fresh lease release after unknown original finish"
            );
            self.handle.shutdown_after_queued().await;
            self.handle.probe().await;
            assert!(
                matches!(
                    self.hub.ensure_live_room(self.key).await,
                    Err(JoinError::CapacityRetry)
                ),
                "actual same-hub successor remains refused by retained owner"
            );
            assert!(
                Arc::ptr_eq(&original, &self.assert_retained(active, fk)),
                "successor attempt cannot replace original error evidence"
            );
            assert!(!self.driver.sql_log().await.iter().any(|sql| sql == RELEASE));
            assert!(
                !self.hub.shutdown().await.is_clean(),
                "unconfirmed owner is not a successful drain"
            );
            assert!(Arc::ptr_eq(&original, &self.assert_retained(active, fk)));
            assert!(!self.driver.sql_log().await.iter().any(|sql| sql == RELEASE));
        }
        async fn close(self, uncertain: bool, fk: bool) {
            drop(self.lease);
            self.driver.shutdown(uncertain, fk).await;
            self.f.close().await;
        }
    }

    #[tokio::test]
    async fn remote_task_normal_sync_fk_failed_and_lost_commit_retain_original_owner() {
        // Real deferred-FK failure, HTTP error after a real COMMIT, and lost
        // original COMMIT reply all retain uncertainty. None supplies a receipt.
        for mode in [0, 1, 3] {
            let mut room = TaskRoom::new().await;
            if mode == 0 {
                sqlx::query("CREATE TABLE on_task_sync_probe(workspace_id BLOB REFERENCES workspaces(id) DEFERRABLE INITIALLY DEFERRED)").execute(&room.f.pool).await.unwrap();
                sqlx::query("CREATE TRIGGER reject_on_task_sync AFTER INSERT ON task_collab_op_receipts BEGIN INSERT INTO on_task_sync_probe VALUES(zeroblob(16)); END;").execute(&room.f.pool).await.unwrap();
            } else {
                room.driver
                    .fail_finish_for("INSERT INTO task_collab_op_receipts", mode)
                    .await;
            }
            room.sync().await;
            room.driver
                .assert_original_finish_for("INSERT INTO task_collab_op_receipts")
                .await;
            assert_eq!(room.durable().await, if mode == 0 { (0,0,0) } else { (1,1,1) }, "actual SQLite original transaction result is distinct from a deliverable confirmation");
            room.assert_blocked(true, mode == 0).await;
            room.close(true, mode == 0).await;
        }
    }

    #[tokio::test]
    async fn remote_task_ambiguous_receipt_and_readback_finish_retain_original_owner() {
        for (receipt, mode) in [(true, 1), (true, 3), (false, 1), (false, 3)] {
            let mut room = TaskRoom::new().await;
            // Writable join confirms activation before any Sync or receipt.
            // Witness that exact owner before arming an uncertain finish;
            // no fresh observer may settle or replace it after the failure.
            let delivery = room.lease.family_room_delivery.expect("actual typed lease");
            let (workspace, task, owner, fence, generation): (
                Vec<u8>, Vec<u8>, Vec<u8>, i64, i64,
            ) = sqlx::query_as(
                "SELECT f.workspace_id, f.task_id, f.owner_token, f.fence, s.writer_generation FROM task_collab_room_fences AS f JOIN task_states AS s ON s.workspace_id = f.workspace_id AND s.task_id = f.task_id WHERE f.workspace_id = ?1 AND f.task_id = ?2",
            )
            .bind(room.key.0.as_bytes().as_slice())
            .bind(room.key.1.as_bytes().as_slice())
            .fetch_one(&room.f.pool)
            .await
            .unwrap();
            let workspace = Uuid::from_slice(&workspace).unwrap();
            let task = Uuid::from_slice(&task).unwrap();
            let owner = Uuid::from_slice(&owner).unwrap();
            assert_eq!(delivery.original.kind(), CollabKind::Task);
            assert!(workspace == room.key.0 && workspace == delivery.original.workspace());
            assert!(task == room.key.1 && task == delivery.original.resource());
            assert!(
                owner == delivery.writer_owner,
                "confirmed join activation owner"
            );
            assert_eq!(fence, delivery.original.sequence());
            assert_eq!(generation, 1, "one confirmed writable join activation");
            let witnessed = delivery.original.with_owner(owner);
            assert!(witnessed == delivery.original.with_owner(delivery.writer_owner));
            assert_eq!(witnessed.sequence(), delivery.original.sequence());
            assert!(
                witnessed != delivery.original,
                "original owner is wrong even with the same Task scope and fence sequence"
            );
            eprintln!("P102 diagnostic subcase receipt={receipt} mode={mode}");
            let op_id = if receipt {
                room.sync().await;
                assert_eq!(
                    room.drain_status().0,
                    1,
                    "healthy initial confirmed operation"
                );
                let (bytes,payload): (Vec<u8>,Vec<u8>) = sqlx::query_as("SELECT op_id,payload FROM task_collab_updates WHERE workspace_id=?1 AND task_id=?2 AND seq=1")
                    .bind(room.key.0.as_bytes().as_slice()).bind(room.key.1.as_bytes().as_slice()).fetch_one(&room.f.pool).await.unwrap();
                assert_eq!(payload, room.payload);
                Uuid::from_slice(&bytes).unwrap()
            } else {
                Uuid::now_v7()
            };
            room.driver
                .fail_finish_for(if receipt { RECEIPT } else { LOAD }, mode)
                .await;
            let (ack, reloaded) = room
                .handle
                .reconcile_for_test(room.f.user, room.credential, op_id, 0, room.payload.clone())
                .await;
            eprintln!("P102 diagnostic returned receipt={receipt} mode={mode} ack_present={} reloaded={reloaded}", ack.is_some());
            room.driver
                .assert_original_finish_for(if receipt { RECEIPT } else { LOAD })
                .await;
            assert!(
                ack.is_none(),
                "uncertain original receipt/load COMMIT cannot be an acknowledgement"
            );
            assert!(
                !reloaded,
                "cache reload is not settlement proof and cannot trigger fresh cleanup"
            );
            assert_eq!(
                room.durable().await,
                if receipt { (1, 1, 1) } else { (0, 0, 0) }
            );
            room.assert_blocked(true, false).await;
            room.close(true, false).await;
        }
    }

    #[tokio::test]
    async fn remote_task_definite_refusal_confirmed_cleanup_and_healthy_successor() {
        let mut room = TaskRoom::new().await;
        let original = room.lease.family_room_delivery.unwrap().original;
        let (entered, continue_append) = arm_append_in_tx_reject_barrier(room.key.1).await;
        let frame = encode(&WireFrame::Document {
            routing_key: format!("{}:task:{}", room.key.0, room.key.1),
            room: None,
            message: DocumentMessage::Sync(SyncMessage {
                step: SyncStep::Update,
                y_protocol: encode_sync_payload(SyncStep::Update, &room.payload),
            }),
        })
        .unwrap();
        room.handle.frame(room.conn, frame).await;
        entered.await.unwrap();
        sqlx::query("UPDATE sessions SET revoked_at=1 WHERE id=?1")
            .bind(room.credential.as_bytes().as_slice())
            .execute(&room.f.pool)
            .await
            .unwrap();
        continue_append.send(()).unwrap();
        room.handle.probe().await;
        assert_eq!(
            room.durable().await,
            (0, 0, 0),
            "current-session refusal has no durable update or receipt"
        );
        assert_eq!(room.drain_status().0, 0);
        assert_eq!(room.cancel.borrow().as_ref().unwrap().code, 1008);
        assert!(
            room.hub.unresolved_family_owner(room.key).is_none(),
            "definite domain refusal cannot become remote uncertainty"
        );
        room.handle.shutdown_after_queued().await;
        room.handle.probe().await;
        sqlx::query("UPDATE sessions SET revoked_at=NULL WHERE id=?1")
            .bind(room.credential.as_bytes().as_slice())
            .execute(&room.f.pool)
            .await
            .unwrap();
        // Actual hub cleanup settles the first guard before admitting a new
        // authenticated owner; no unknown record is manually cleared.
        let (conn, lease, events, cancel) =
            join(&room.hub, &room.f, room.credential, room.key).await;
        let next = lease.family_room_delivery.unwrap().original;
        assert_ne!(next.owner(), original.owner());
        assert!(next.sequence() > original.sequence());
        drop(room.lease);
        room.lease = lease;
        room.conn = conn;
        room.events = events;
        room.cancel = cancel;
        room.handle = room.hub.ensure_live_room(room.key).await.unwrap();
        room.sync().await;
        assert_eq!(room.drain_status().0, 1);
        assert_eq!(room.durable().await, (1, 1, 1));
        let projected = room
            .hub
            .project_live(room.key, room.f.user, room.credential)
            .await
            .unwrap();
        assert_eq!(projected.tail_seq, 1);
        assert_eq!(
            projected.content_json, room.body,
            "same literal IDs, formatted text and native projection"
        );
        assert!(room.hub.unresolved_family_owner(room.key).is_none());
        assert!(room.hub.shutdown().await.is_clean());
        assert!(
            room.driver.sql_log().await.iter().any(|sql| sql == RELEASE),
            "confirmed healthy/domain paths still release their exact guard"
        );
        room.close(false, false).await;
    }
}
