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
use collab_engine::protocol::Request;
use sha2::{Digest, Sha256};
use sqlx::postgres::PgPool;
use tokio::sync::{mpsc, oneshot, watch};
use uuid::Uuid;

use crate::collab::awareness::{decode_awareness, AwarenessRegistry};
use crate::collab::config::CollabConfig;
use crate::collab::derived_body::prepare_derived_body;
use crate::collab::engine_bridge::{BridgeError, EngineBridge};
use crate::collab::guard::RoomGuard;
use crate::collab::validation::{
    classify_admission_load, validate_recovery_bundle, validate_snapshot_only, BundleValidation,
    ValidateStageTimings,
};

const MAX_REJECTED_CANDIDATES_PER_CONN: usize = 8;
const REJECTED_CANDIDATE_WINDOW: Duration = Duration::from_secs(30);
use crate::collab::wire::{encode, AuthMessage, DocumentMessage, SyncStep, WireFrame};
use crate::collab::y_sync::{encode_sync_payload, is_empty_update, parse_sync_payload};
use crate::db::collab::verify_collab_operation;
use crate::db::collab::{
    append_collab_update, append_collab_update_on_conn_timed, claim_writer_and_load,
    compact_collab_snapshot, load_collab_readonly, project_derived_body, resolve_collab_admission,
    AppendCollabInput, AppendCollabResult, CollabDbError, CompactCollabInput,
    ProjectDerivedBodyInput, ProjectDerivedBodyResult, VerifyCollabInput,
};
use crate::db::collab_delivery::{check_delivery_admission, DeliveryAdmission};
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

pub type RoomKey = (Uuid, Uuid);

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
}

struct JoinAdmission {
    lease: ConnectionLease,
    drop_rx: oneshot::Receiver<Infallible>,
    conn_generation: u64,
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
        reply: oneshot::Sender<Result<CapturedRevision, RevisionCaptureError>>,
    },
    Restore {
        actor_user_id: Uuid,
        session_id: Uuid,
        snap: Vec<u8>,
        reply: oneshot::Sender<Result<(), RevisionRestoreError>>,
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
    RoomFull,
    /// Hub room count, helper child cap, or aggregate memory budget exhausted.
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

    pub async fn shutdown(&self) {
        let _ = self.tx.send(RoomCommand::Shutdown).await;
    }

    pub async fn capture_revision(
        &self,
        actor_user_id: Uuid,
        session_id: Uuid,
    ) -> Result<CapturedRevision, RevisionCaptureError> {
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
    ) -> Result<(), RevisionRestoreError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(RoomCommand::Restore {
                actor_user_id,
                session_id,
                snap,
                reply: reply_tx,
            })
            .await
            .map_err(|_| RevisionRestoreError::Unavailable)?;
        reply_rx
            .await
            .map_err(|_| RevisionRestoreError::Unavailable)?
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
    revoked: bool,
    rejected_candidates: VecDeque<Instant>,
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
    document_id: Uuid,
    config: CollabConfig,
    pool: PgPool,
    engine: EngineBridge,
    room_guard: Option<RoomGuard>,
    writer_generation: Option<i64>,
    committed: CommittedBundle,
    connections: HashMap<Uuid, ConnectionState>,
    connection_lease_drops: FuturesUnordered<ConnectionLeaseDrop>,
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
    shutting_down: bool,
    pending_awareness: VecDeque<Vec<u8>>,
    flushing_awareness: bool,
    /// Set when the dedicated fence connection is lost; actor exits once empty.
    fence_lost: bool,
}

pub async fn spawn_room(
    workspace_id: Uuid,
    document_id: Uuid,
    config: CollabConfig,
    pool: PgPool,
    room_guard: RoomGuard,
    live_conns: Arc<AtomicUsize>,
) -> Result<(RoomHandle, oneshot::Receiver<()>), JoinError> {
    wait_spawn_room_block(document_id).await;
    let engine = match EngineBridge::spawn(config.engine_bin.clone(), config.limits) {
        Ok(engine) => engine,
        Err(report) => {
            if matches!(
                report.outcome,
                collab_engine::EngineStatus::ResourceLimit { .. }
            ) {
                return Err(JoinError::CapacityRetry);
            }
            return Err(JoinError::EngineUnavailable);
        }
    };
    let (tx, mut rx) = mpsc::channel(config.max_queued_room_ops);
    let (finished_tx, finished_rx) = oneshot::channel();
    let actor = RoomActor {
        workspace_id,
        document_id,
        config,
        pool,
        engine,
        room_guard: Some(room_guard),
        writer_generation: None,
        committed: CommittedBundle {
            snapshot: vec![0, 0],
            tail_payloads: Vec::new(),
            tail_seq: 0,
            snapshot_cutoff_seq: 0,
        },
        connections: HashMap::new(),
        connection_lease_drops: FuturesUnordered::new(),
        live_conns,
        awareness: AwarenessRegistry::new(),
        fifo_seq: 0,
        compact_unhealthy: false,
        compact_retry_at_tail_len: None,
        primary_loaded: false,
        committed_loaded: false,
        primary_dirty: false,
        client_id_owner: HashMap::new(),
        shutting_down: false,
        pending_awareness: VecDeque::new(),
        flushing_awareness: false,
        fence_lost: false,
    };
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
    Ok((RoomHandle { tx }, finished_rx))
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

impl RoomActor {
    fn publish_live_conns(&self) {
        self.live_conns
            .store(self.connections.len(), Ordering::Release);
    }

    #[cfg(feature = "db-tests")]
    async fn drain_ready_lease_drops(&mut self) {
        while let Some(Some((conn_id, generation))) =
            self.connection_lease_drops.next().now_or_never()
        {
            self.handle_lease_drop(conn_id, generation).await;
        }
    }

    async fn handle_lease_drop(&mut self, conn_id: Uuid, generation: u64) {
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
        loop {
            tokio::select! {
                cmd = rx.recv() => {
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
                            reply,
                        }) => {
                            let _ = reply
                                .send(self.handle_restore(actor_user_id, session_id, snap).await);
                        }
                        Some(RoomCommand::Shutdown) => {
                            self.shutting_down = true;
                            break;
                        }
                        #[cfg(feature = "db-tests")]
                        Some(RoomCommand::Probe(reply)) => {
                            self.drain_ready_lease_drops().await;
                            let _ = reply.send(ActorProbe {
                                connections: self.connections.len(),
                                awareness_clients: self.awareness.tracked_client_count(),
                            });
                        }
                        None => break,
                    }
                }
                Some((conn_id, generation)) = self.connection_lease_drops.next(), if !self.connection_lease_drops.is_empty() => {
                    self.handle_lease_drop(conn_id, generation).await;
                }
                _ = acl_tick.tick() => {
                    self.poll_acl().await;
                }
            }
            self.publish_live_conns();
            if self.fence_lost && self.connections.is_empty() {
                break;
            }
            if self.connections.is_empty() && self.shutting_down {
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

        if let Some(guard) = self.room_guard.take() {
            guard.release().await;
        }

        matches!(exit, RoomExit::Clean) && engine_stop_ok
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
            match check_delivery_admission(
                &self.pool,
                self.workspace_id,
                session.user_id,
                session.session_id,
                self.document_id,
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
        match resolve_collab_admission(
            &self.pool,
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

    async fn evict_connection(
        &mut self,
        conn_id: Uuid,
        code: u16,
        reason: &str,
    ) -> Option<Vec<u8>> {
        if let Some(conn) = self.connections.get_mut(&conn_id) {
            conn.revoked = true;
        }
        let tombstone = if let Some(conn) = self.connections.get(&conn_id) {
            self.awareness
                .remove_client(conn.client_id, conn.conn_generation)
        } else {
            None
        };
        if let Some(conn) = self.connections.remove(&conn_id) {
            Self::enqueue_close(&conn.events, &conn.cancel, code, reason);
        }
        tombstone
    }

    async fn evict_connection_ordered(
        &mut self,
        conn_id: Uuid,
        code: u16,
        reason: &str,
    ) -> Option<Vec<u8>> {
        if let Some(conn) = self.connections.get_mut(&conn_id) {
            conn.revoked = true;
        }
        let tombstone = if let Some(conn) = self.connections.get(&conn_id) {
            self.awareness
                .remove_client(conn.client_id, conn.conn_generation)
        } else {
            None
        };
        if let Some(conn) = self.connections.remove(&conn_id) {
            Self::enqueue_close_ordered(&conn.events, &conn.cancel, code, reason);
        }
        tombstone
    }

    async fn close_connection(&mut self, conn_id: Uuid, code: u16, reason: &str) {
        if let Some(encoded) = self.evict_connection(conn_id, code, reason).await {
            self.pending_awareness.push_back(encoded);
        }
        self.flush_pending_awareness().await;
    }

    async fn close_connection_ordered(&mut self, conn_id: Uuid, code: u16, reason: &str) {
        if let Some(encoded) = self.evict_connection_ordered(conn_id, code, reason).await {
            self.pending_awareness.push_back(encoded);
        }
        self.flush_pending_awareness().await;
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

    async fn handle_join(&mut self, join: RoomJoin) -> Result<JoinAdmission, JoinError> {
        if self.shutting_down {
            return Err(JoinError::EngineUnavailable);
        }
        if self.fence_lost {
            return Err(JoinError::EngineUnavailable);
        }
        #[cfg(feature = "db-tests")]
        pause_for_join_barrier(self.document_id).await;
        if self.connections.len() >= self.config.max_connections_per_room {
            return Err(JoinError::RoomFull);
        }
        let admission = resolve_collab_admission(
            &self.pool,
            self.workspace_id,
            join.conn.session.user_id,
            join.conn.session.session_id,
            self.document_id,
        )
        .await
        .map_err(|_| JoinError::DbError)?;
        let admission = admission.map_err(|_| JoinError::AdmissionDenied)?;
        let read_only = join.conn.read_only || admission.read_only;
        if self.writer_generation.is_none() && !read_only {
            let claim = claim_writer_and_load(
                &self.pool,
                self.workspace_id,
                join.conn.session.user_id,
                join.conn.session.session_id,
                self.document_id,
            )
            .await
            .map_err(|_| JoinError::DbError)?;
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
            let load = load_collab_readonly(
                &self.pool,
                self.workspace_id,
                join.conn.session.user_id,
                join.conn.session.session_id,
                self.document_id,
            )
            .await
            .map_err(|_| JoinError::DbError)?;
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
                revoked: false,
                rejected_candidates: VecDeque::new(),
            },
        );
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
            },
            drop_rx,
            conn_generation,
        })
    }

    fn set_committed_from_load(&mut self, load: &crate::db::collab::CollabLoadState) {
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

    async fn fatal_fence_lost(&mut self) {
        self.fence_lost = true;
        self.writer_generation = None;
        self.primary_loaded = false;
        self.primary_dirty = true;
        self.room_guard = None;
        self.close_all_connections(1013, "try again later").await;
    }

    async fn reload_primary_or_close_room(&mut self) -> bool {
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

    fn connection_reject_limit_exceeded(&mut self, conn_id: Uuid) -> bool {
        let Some(conn) = self.connections.get_mut(&conn_id) else {
            return true;
        };
        let now = Instant::now();
        conn.rejected_candidates
            .retain(|t| now.duration_since(*t) < REJECTED_CANDIDATE_WINDOW);
        conn.rejected_candidates.len() >= MAX_REJECTED_CANDIDATES_PER_CONN
    }

    fn record_rejected_candidate(&mut self, conn_id: Uuid) -> bool {
        let Some(conn) = self.connections.get_mut(&conn_id) else {
            return true;
        };
        let now = Instant::now();
        conn.rejected_candidates
            .retain(|t| now.duration_since(*t) < REJECTED_CANDIDATE_WINDOW);
        conn.rejected_candidates.push_back(now);
        conn.rejected_candidates.len() >= MAX_REJECTED_CANDIDATES_PER_CONN
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
                match check_delivery_admission(
                    &self.pool,
                    self.workspace_id,
                    session.user_id,
                    session.session_id,
                    self.document_id,
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
                match check_delivery_admission(
                    &self.pool,
                    self.workspace_id,
                    session.user_id,
                    session.session_id,
                    self.document_id,
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
                if self.connection_reject_limit_exceeded(conn_id) {
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
                    slot_wait_us = validate_tx.slot_wait_us,
                    spawn_us = validate_tx.spawn_us,
                    load_us = validate_tx.load_us,
                    snapshot_us = validate_tx.snapshot_us,
                    document_id = %self.document_id,
                );
                if validation == BundleValidation::CapacityPressure {
                    self.reject_candidate_capacity_pressure(conn_id).await;
                    return;
                }
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
                let room_conn = room_guard.connection_mut();

                #[cfg(feature = "db-tests")]
                pause_for_append_in_tx_reject_barrier(self.document_id).await;

                let op_id = Uuid::now_v7();
                let expected_tail = self.committed.tail_seq;
                let digest = payload_digest(&payload);
                let append_started = std::time::Instant::now();
                let timed_append = append_collab_update_on_conn_timed(
                    room_conn,
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
                        self.fatal_fence_lost().await;
                        return;
                    }
                    Ok((result, _timings)) => result,
                };

                let committed = match append {
                    Ok(result) => result,
                    Err(CollabDbError::StaleWriter) => {
                        if !self.reload_primary_or_close_room().await {
                            return;
                        }
                        self.fatal_writer_stale().await;
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
                        self.fatal_writer_stale_ordered().await;
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

    async fn reject_candidate_capacity_pressure(&mut self, conn_id: Uuid) {
        if self.primary_dirty && !self.reload_primary_or_close_room().await {
            return;
        }
        if let Some(c) = self.connections.get_mut(&conn_id) {
            c.in_flight = false;
        }
        self.close_connection(conn_id, 1013, "try again later")
            .await;
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
        let _limit_exceeded = self.record_rejected_candidate(conn_id);
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
        match verify_collab_operation(
            &self.pool,
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
        )
        .await
        {
            Ok(Ok(lookup)) => Some(AppendCollabResult::DuplicateAck { seq: lookup.seq }),
            Ok(Err(CollabDbError::NotFound)) => {
                match load_collab_readonly(
                    &self.pool,
                    self.workspace_id,
                    actor_user_id,
                    session_id,
                    self.document_id,
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
            Ok(Err(_)) | Err(_) => {
                self.fatal_room_divergence(actor_user_id, session_id).await;
                None
            }
        }
    }

    async fn fatal_writer_stale(&mut self) {
        self.writer_generation = None;
        for conn_id in self.connections.keys().cloned().collect::<Vec<_>>() {
            self.close_connection(conn_id, 1008, "writer stale").await;
        }
    }

    async fn fatal_writer_stale_ordered(&mut self) {
        self.writer_generation = None;
        for conn_id in self.connections.keys().cloned().collect::<Vec<_>>() {
            self.close_connection_ordered(conn_id, 1008, "writer stale")
                .await;
        }
    }

    async fn fatal_room_divergence(&mut self, actor_user_id: Uuid, session_id: Uuid) {
        if let Ok(Ok(load)) = load_collab_readonly(
            &self.pool,
            self.workspace_id,
            actor_user_id,
            session_id,
            self.document_id,
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
        if let Ok(Ok(load)) = load_collab_readonly(
            &self.pool,
            self.workspace_id,
            actor_user_id,
            session_id,
            self.document_id,
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
                ValidateStageTimings {
                    load_us,
                    ..ValidateStageTimings::default()
                },
            );
        }
        (
            outcome,
            ValidateStageTimings {
                load_us,
                ..ValidateStageTimings::default()
            },
        )
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

        match project_derived_body(
            &self.pool,
            ProjectDerivedBodyInput::new(
                self.workspace_id,
                actor_user_id,
                session_id,
                self.document_id,
                writer_generation,
                seq,
                prepared,
            ),
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
                    self.fatal_writer_stale().await;
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
        if self.engine.recycle().await.is_err() {
            self.primary_loaded = false;
            self.primary_dirty = true;
            return Err(JoinError::EngineUnavailable);
        }
        match self.load_engine_primary().await {
            Ok(()) => {
                self.primary_loaded = true;
                self.primary_dirty = false;
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
            .map_err(|_| JoinError::EngineUnavailable)?;
        if report.outcome.is_applied_ok() {
            Ok(())
        } else {
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
        if let Some(writer_generation) = self.writer_generation {
            let cutoff = self.committed.tail_seq;
            let compact = compact_collab_snapshot(
                &self.pool,
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
            )
            .await;
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
    ) -> Result<CapturedRevision, RevisionCaptureError> {
        if !self.committed_loaded {
            let load = load_collab_readonly(
                &self.pool,
                self.workspace_id,
                actor_user_id,
                session_id,
                self.document_id,
            )
            .await
            .map_err(|_| RevisionCaptureError::Unavailable)?;
            let load = load.map_err(|_| RevisionCaptureError::Unavailable)?;
            self.set_committed_from_load(&load);
            self.reload_primary_from_committed()
                .await
                .map_err(|_| RevisionCaptureError::Unavailable)?;
        }
        if self.ensure_primary_capacity().await.is_err() {
            return Err(RevisionCaptureError::Unavailable);
        }
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
        Ok(CapturedRevision {
            y_snapshot: snap,
            content_json,
        })
    }

    async fn handle_restore(
        &mut self,
        actor_user_id: Uuid,
        session_id: Uuid,
        snap: Vec<u8>,
    ) -> Result<(), RevisionRestoreError> {
        if self.writer_generation.is_none() {
            let claim = claim_writer_and_load(
                &self.pool,
                self.workspace_id,
                actor_user_id,
                session_id,
                self.document_id,
            )
            .await
            .map_err(|_| RevisionRestoreError::Unavailable)?;
            let claim = claim.map_err(|err| match err {
                CollabDbError::Forbidden | CollabDbError::NotFound => {
                    RevisionRestoreError::Rejected
                }
                _ => RevisionRestoreError::Unavailable,
            })?;
            self.set_committed_from_load(&claim.load);
            self.reload_primary_from_committed()
                .await
                .map_err(|_| RevisionRestoreError::Unavailable)?;
            self.writer_generation = Some(claim.writer_generation);
        } else if self.ensure_primary_capacity().await.is_err() {
            return Err(RevisionRestoreError::Unavailable);
        }

        #[cfg(feature = "db-tests")]
        pause_for_append_revoke_barrier(self.document_id).await;

        match self
            .locking_session_auth_by_ids(actor_user_id, session_id, false)
            .await
        {
            LockingAuth::Allow => {}
            LockingAuth::Deny => return Err(RevisionRestoreError::Rejected),
            LockingAuth::DbError => return Err(RevisionRestoreError::Unavailable),
        }

        let payload = match self
            .engine
            .call(Request::RestoreFromSnapshot {
                snap_b64: snap,
                encoding: 1,
            })
            .await
        {
            Ok(report) => match report.outcome {
                EngineStatus::Ok {
                    applied: true,
                    update_b64: Some(bytes),
                    ..
                } => collab_engine::b64::decode(&bytes)
                    .map_err(|_| RevisionRestoreError::Unavailable)?,
                _ => return Err(RevisionRestoreError::Unavailable),
            },
            Err(_) => return Err(RevisionRestoreError::Unavailable),
        };
        if is_empty_update(&payload) {
            return Ok(());
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
            return Err(RevisionRestoreError::Unavailable);
        }
        if validation != BundleValidation::Ok {
            return Err(RevisionRestoreError::Rejected);
        }

        let writer_generation = self
            .writer_generation
            .ok_or(RevisionRestoreError::Unavailable)?;
        let op_id = Uuid::now_v7();
        let expected_tail = self.committed.tail_seq;
        let digest = payload_digest(&payload);
        let append = append_collab_update(
            &self.pool,
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

        let committed = match append {
            Ok(Ok(result)) => result,
            Ok(Err(CollabDbError::StaleWriter)) => {
                self.fatal_writer_stale().await;
                return Err(RevisionRestoreError::Unavailable);
            }
            Ok(Err(err)) if Self::is_definite_append_rejection(&err) => {
                return Err(RevisionRestoreError::Rejected);
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
                    Some(result) => result,
                    None => return Err(RevisionRestoreError::Unavailable),
                }
            }
        };

        let seq = match committed {
            AppendCollabResult::Committed { seq } | AppendCollabResult::DuplicateAck { seq } => {
                if seq != expected_tail + 1 {
                    self.fatal_room_divergence(actor_user_id, session_id).await;
                    return Err(RevisionRestoreError::Unavailable);
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
            return Err(RevisionRestoreError::Unavailable);
        }
        let y_protocol = encode_sync_payload(SyncStep::Update, &payload);
        self.broadcast_update(&y_protocol).await;
        let _ = self
            .maybe_project_derived_body(seq, actor_user_id, session_id, false)
            .await;
        Ok(())
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

    async fn deliver_outbound(&mut self, conn_id: Uuid, bytes: Vec<u8>, kind: OutboundKind) {
        if self
            .connections
            .get(&conn_id)
            .is_none_or(|conn| conn.revoked)
        {
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
            if let Some(tombstone) = self
                .evict_connection(conn_id, 1009, "outbound queue full")
                .await
            {
                self.pending_awareness.push_back(tombstone);
            }
            return;
        }
        let current = budget.queued_bytes.load(Ordering::Relaxed);
        if current + accounted_bytes > budget.max_bytes {
            if let Some(tombstone) = self
                .evict_connection(conn_id, 1009, "outbound queue full")
                .await
            {
                self.pending_awareness.push_back(tombstone);
            }
            return;
        }
        let frame_permit = match budget.frame_sem.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                if let Some(tombstone) = self
                    .evict_connection(conn_id, 1009, "outbound queue full")
                    .await
                {
                    self.pending_awareness.push_back(tombstone);
                }
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
        let events = self.connections.get(&conn_id).map(|c| c.events.clone());
        let Some(events) = events else {
            return;
        };
        let frame = OutboundFrame {
            bytes,
            kind,
            permit: Some(delivery_permit),
        };
        if events.try_send(RoomClientEvent::Outbound(frame)).is_err() {
            if let Some(tombstone) = self
                .evict_connection(conn_id, 1009, "outbound queue full")
                .await
            {
                self.pending_awareness.push_back(tombstone);
            }
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
