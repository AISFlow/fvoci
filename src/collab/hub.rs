use std::collections::HashMap;
#[cfg(feature = "db-tests")]
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::future::join_all;
use futures_util::FutureExt;
use sqlx::postgres::PgPool;
#[cfg(feature = "db-tests")]
use tokio::sync::oneshot;
use tokio::sync::{watch, Mutex, Notify, OwnedSemaphorePermit, RwLock, Semaphore};
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::collab::admission::{warn_join_db_error, MemoryLedger};
use crate::collab::config::{CollabConfig, FamilyRoomTimings};
use crate::collab::guard::{
    BackendRoomGuard, FamilyRoomOwnerRecord, FamilyRoomOwnerRecords, RoomGuard,
};
use crate::collab::room::{
    BodyWriteError, CapturedRevision, ConnectionLease, JoinDelivery, JoinError, LiveProjection,
    RevisionCaptureError, RevisionRestoreError, RoomHandle, RoomJoin, RoomKey,
};
use crate::db::backend::Backend;
use crate::db::collab::estimate_persisted_collab_bytes_kind;
use crate::db::collab::{
    abandon_family_document_room_start, acquire_family_document_room_for_start,
    estimate_family_document_bytes, resolve_collab_admission_kind_backend,
};

#[cfg(feature = "db-tests")]
pub const HUB_JOIN_BARRIER_BEFORE_ACTOR_JOIN: u8 = 0;
#[cfg(feature = "db-tests")]
pub const HUB_JOIN_BARRIER_AFTER_ACTOR_REPLY: u8 = 1;
#[cfg(feature = "db-tests")]
pub const HUB_JOIN_BARRIER_AFTER_SLOT_READY: u8 = 2;
/// HTTP borrow (`project_live`, `replace_body`, `restore_revision`) after its
/// room was returned Live, before the borrowed handle is taken.
#[cfg(feature = "db-tests")]
pub const HUB_BORROW_BARRIER_AFTER_SLOT_READY: u8 = 3;
/// Admission at the room cap (keyed by the admitted document) after its reclaim
/// scan chose a candidate, before the locked re-check of that candidate.
#[cfg(feature = "db-tests")]
pub const HUB_JOIN_BARRIER_AFTER_RECLAIM_SCAN: u8 = 4;
/// A join after its admission check passed, before it reserves or starts a
/// room (a join admitted before a concurrent same-ID MOVE commits).
#[cfg(feature = "db-tests")]
pub const HUB_JOIN_BARRIER_AFTER_ADMISSION: u8 = 5;
#[cfg(feature = "db-tests")]
pub const HUB_FAMILY_START_BEFORE_BEGIN: u8 = 6;

/// One pre-enqueue retry after a proven undelivered join or a Closing race.
const MAX_PRE_ENQUEUE_RETRIES: u8 = 1;

/// Rooms a single admission may reclaim at the room cap before it reports `RoomFull`.
const MAX_ADMISSION_RECLAIMS: u8 = 2;

/// Floor of the admission-reclaim grace; see [`CollabHub::reclaim_grace`].
const RECLAIM_GRACE_FLOOR_MS: u64 = 3_000;

#[cfg(feature = "db-tests")]
struct HubJoinBarrier {
    reached_tx: oneshot::Sender<()>,
    proceed_rx: oneshot::Receiver<()>,
}

#[cfg(feature = "db-tests")]
static HUB_JOIN_BARRIERS: std::sync::LazyLock<Mutex<HashMap<(Uuid, u8), HubJoinBarrier>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

#[cfg(feature = "db-tests")]
pub async fn arm_hub_join_barrier(
    document_id: Uuid,
    point: u8,
) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
    let (reached_tx, reached_rx) = oneshot::channel();
    let (proceed_tx, proceed_rx) = oneshot::channel();
    HUB_JOIN_BARRIERS.lock().await.insert(
        (document_id, point),
        HubJoinBarrier {
            reached_tx,
            proceed_rx,
        },
    );
    (reached_rx, proceed_tx)
}

#[cfg(feature = "db-tests")]
pub async fn disarm_hub_join_barrier(document_id: Uuid, point: u8) {
    HUB_JOIN_BARRIERS.lock().await.remove(&(document_id, point));
}

#[cfg(feature = "db-tests")]
async fn pause_for_hub_join_barrier(document_id: Uuid, point: u8) {
    let barrier = HUB_JOIN_BARRIERS.lock().await.remove(&(document_id, point));
    if let Some(barrier) = barrier {
        let _ = barrier.reached_tx.send(());
        let _ = barrier.proceed_rx.await;
    }
}

#[cfg(feature = "db-tests")]
struct ReclaimBarrier {
    reached_tx: oneshot::Sender<()>,
    proceed_rx: oneshot::Receiver<()>,
}

#[cfg(feature = "db-tests")]
static RECLAIM_BARRIERS: std::sync::LazyLock<Mutex<HashMap<Uuid, ReclaimBarrier>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

#[cfg(feature = "db-tests")]
pub async fn arm_reclaim_barrier(
    document_id: Uuid,
) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
    let (reached_tx, reached_rx) = oneshot::channel();
    let (proceed_tx, proceed_rx) = oneshot::channel();
    RECLAIM_BARRIERS.lock().await.insert(
        document_id,
        ReclaimBarrier {
            reached_tx,
            proceed_rx,
        },
    );
    (reached_rx, proceed_tx)
}

#[cfg(feature = "db-tests")]
pub async fn disarm_reclaim_barrier(document_id: Uuid) {
    RECLAIM_BARRIERS.lock().await.remove(&document_id);
}

#[cfg(feature = "db-tests")]
async fn pause_for_reclaim_barrier(document_id: Uuid) {
    let barrier = RECLAIM_BARRIERS.lock().await.remove(&document_id);
    if let Some(barrier) = barrier {
        let _ = barrier.reached_tx.send(());
        let _ = barrier.proceed_rx.await;
    }
}

#[cfg(feature = "db-tests")]
static IDLE_EVICTION_HOLDS: std::sync::LazyLock<std::sync::Mutex<HashSet<Uuid>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(HashSet::new()));

#[cfg(feature = "db-tests")]
pub fn hold_idle_eviction(document_id: Uuid) {
    IDLE_EVICTION_HOLDS
        .lock()
        .expect("idle eviction holds")
        .insert(document_id);
}

#[cfg(feature = "db-tests")]
pub fn release_idle_eviction(document_id: Uuid) {
    IDLE_EVICTION_HOLDS
        .lock()
        .expect("idle eviction holds")
        .remove(&document_id);
}

/// Blocks the background idle-eviction loop for one document until dropped.
#[cfg(feature = "db-tests")]
pub struct IdleEvictionHold {
    document_id: Uuid,
}

#[cfg(feature = "db-tests")]
impl IdleEvictionHold {
    pub fn arm(document_id: Uuid) -> Self {
        hold_idle_eviction(document_id);
        Self { document_id }
    }
}

#[cfg(feature = "db-tests")]
impl Drop for IdleEvictionHold {
    fn drop(&mut self) {
        release_idle_eviction(self.document_id);
    }
}

#[cfg(feature = "db-tests")]
static ROOM_START_COUNTS: std::sync::LazyLock<Mutex<HashMap<Uuid, usize>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

#[cfg(feature = "db-tests")]
pub async fn room_start_count(document_id: Uuid) -> usize {
    ROOM_START_COUNTS
        .lock()
        .await
        .get(&document_id)
        .copied()
        .unwrap_or(0)
}

#[cfg(feature = "db-tests")]
async fn increment_room_start_count(document_id: Uuid) {
    *ROOM_START_COUNTS
        .lock()
        .await
        .entry(document_id)
        .or_insert(0) += 1;
}

struct LiveRoom {
    handle: RoomHandle,
    finished: tokio::sync::oneshot::Receiver<()>,
    last_activity: Instant,
    live_conns: Arc<AtomicUsize>,
    /// Callers that got this room back Live and have not finished actor
    /// admission yet (see [`LiveSlot`]), plus HTTP operations (body write,
    /// projection, revision) borrowing the actor.
    joining: Arc<AtomicUsize>,
    permit: OwnedSemaphorePermit,
}

struct JoiningLease {
    counter: Arc<AtomicUsize>,
}

impl JoiningLease {
    fn register(counter: &Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::AcqRel);
        Self {
            counter: counter.clone(),
        }
    }
}

impl Drop for JoiningLease {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::AcqRel);
    }
}

/// A room returned Live by [`CollabHub::get_or_create_room`], with a joining
/// lease registered under the same phase lock that published or observed Live.
/// Admission reclaim and idle eviction therefore cannot close the room between
/// the return and the caller's own join or borrowed operation. A slot is Live at
/// most once, so while its phase is still Live the lease counts on that room.
struct LiveSlot {
    slot: Arc<RoomSlot>,
    lease: JoiningLease,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleEvictDecision {
    NotApplicable,
    DeferredMembers,
    DeferredJoining,
    DeferredActivity,
    WouldEvict,
}

enum RoomPhase {
    Starting,
    Booting(OwnedSemaphorePermit),
    Live(LiveRoom),
    Closing,
    Failed,
}

struct RoomSlot {
    phase: Mutex<RoomPhase>,
    ready: Notify,
    #[cfg(feature = "db-tests")]
    waiters: std::sync::atomic::AtomicUsize,
}

#[cfg(feature = "db-tests")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoomLifecyclePhase {
    Starting,
    Booting,
    Live,
    Closing,
    Failed,
    Absent,
}

struct SessionSocketTracker {
    counts: std::sync::Mutex<HashMap<Uuid, usize>>,
    max_per_session: usize,
}

/// Global + per-session socket lease. Dropping it releases both counters.
pub struct CollabSocketPermit {
    _global: OwnedSemaphorePermit,
    session_id: Uuid,
    tracker: Arc<SessionSocketTracker>,
}

impl Drop for CollabSocketPermit {
    fn drop(&mut self) {
        if let Ok(mut counts) = self.tracker.counts.lock() {
            if let Some(count) = counts.get_mut(&self.session_id) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    counts.remove(&self.session_id);
                }
            }
        }
    }
}

#[derive(Clone)]
pub struct CollabHub {
    config: CollabConfig,
    backend: Backend,
    family_timings: Option<FamilyRoomTimings>,
    // Unconfirmed startup owners remain service-owned; a later attempt cannot
    // overwrite or adopt one after an unknown original stream cleanup.
    pending_family_starts: FamilyRoomOwnerRecords,
    rooms: Arc<RwLock<HashMap<RoomKey, Arc<RoomSlot>>>>,
    room_permits: Arc<Semaphore>,
    socket_permits: Arc<Semaphore>,
    session_sockets: Arc<SessionSocketTracker>,
    shutting_down: Arc<AtomicBool>,
    idle_stop: watch::Sender<bool>,
    idle_task: Arc<Mutex<Option<JoinHandle<()>>>>,
    starts: Arc<std::sync::Mutex<Vec<JoinHandle<()>>>>,
    shutdown_lock: Arc<Mutex<()>>,
    abnormal_actor_completions: Arc<AtomicUsize>,
    memory_ledger: Arc<MemoryLedger>,
    #[cfg(feature = "db-tests")]
    shutdown_drain_witness: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

impl CollabHub {
    pub fn new(config: CollabConfig, pool: PgPool) -> Self {
        Self::build(config, Backend::Postgres(pool), None)
    }

    pub fn new_backend(
        config: CollabConfig,
        backend: Backend,
        family_timings: Option<FamilyRoomTimings>,
    ) -> Result<Self, String> {
        if !matches!(backend, Backend::Postgres(_)) && family_timings.is_none() {
            return Err("family collab requires explicit room lease and renewal timings".into());
        }
        Ok(Self::build(config, backend, family_timings))
    }

    fn build(
        config: CollabConfig,
        backend: Backend,
        family_timings: Option<FamilyRoomTimings>,
    ) -> Self {
        config.apply_runtime_limits();
        let room_cap = config.max_rooms;
        let rooms = Arc::new(RwLock::new(HashMap::new()));
        let room_permits = Arc::new(Semaphore::new(room_cap));
        let socket_permits = Arc::new(Semaphore::new(config.max_collab_sockets));
        let session_sockets = Arc::new(SessionSocketTracker {
            counts: std::sync::Mutex::new(HashMap::new()),
            max_per_session: config.max_collab_sockets_per_session,
        });
        let idle_ms = config.idle_evict_ms;
        let idle_rooms = rooms.clone();
        let abnormal_actor_completions = Arc::new(AtomicUsize::new(0));
        let idle_failures = abnormal_actor_completions.clone();
        let (idle_stop, stopped) = watch::channel(false);
        let idle_task = tokio::spawn(async move {
            idle_eviction_loop(idle_rooms, idle_ms, stopped, idle_failures).await;
        });
        let memory_ledger = MemoryLedger::new(config.memory_budget_bytes);
        Self {
            config,
            backend,
            family_timings,
            pending_family_starts: Arc::new(std::sync::Mutex::new(HashMap::new())),
            rooms,
            room_permits,
            socket_permits,
            session_sockets,
            shutting_down: Arc::new(AtomicBool::new(false)),
            idle_stop,
            idle_task: Arc::new(Mutex::new(Some(idle_task))),
            starts: Arc::new(std::sync::Mutex::new(Vec::new())),
            shutdown_lock: Arc::new(Mutex::new(())),
            abnormal_actor_completions,
            memory_ledger,
            #[cfg(feature = "db-tests")]
            shutdown_drain_witness: Arc::new(Mutex::new(None)),
        }
    }

    #[cfg(feature = "db-tests")]
    pub async fn arm_shutdown_drain_witness(&self) -> oneshot::Receiver<()> {
        let (tx, rx) = oneshot::channel();
        *self.shutdown_drain_witness.lock().await = Some(tx);
        rx
    }

    #[cfg(feature = "db-tests")]
    pub async fn disarm_shutdown_drain_witness(&self) {
        self.shutdown_drain_witness.lock().await.take();
    }

    #[cfg(feature = "db-tests")]
    async fn signal_shutdown_drain_witness(&self) {
        if let Some(tx) = self.shutdown_drain_witness.lock().await.take() {
            let _ = tx.send(());
        }
    }

    pub fn config(&self) -> &CollabConfig {
        &self.config
    }

    /// Synchronously stop admission. Idle eviction is asked to exit; in-flight
    /// eviction/startup still own their actor, guard, and permit until they finish.
    pub fn begin_shutdown(&self) {
        self.shutting_down.store(true, Ordering::Release);
        let _ = self.idle_stop.send(true);
    }

    /// Becomes `true` when [`Self::begin_shutdown`] runs. Transport waits on this
    /// so unauthenticated sockets can close instead of holding a permit until
    /// `auth_wait_ms`.
    pub fn subscribe_shutdown(&self) -> watch::Receiver<bool> {
        self.idle_stop.subscribe()
    }

    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::Acquire)
    }

    pub fn shutdown_progress(&self) -> ShutdownProgress {
        let rooms = self.rooms.try_read().map(|guard| guard.len()).ok();
        let sockets_held = self
            .config
            .max_collab_sockets
            .saturating_sub(self.socket_permits.available_permits());
        ShutdownProgress {
            rooms,
            sockets_held,
        }
    }

    async fn wait_if_shutting_down(&self) {
        if self.shutting_down.load(Ordering::Acquire) {
            return;
        }
        let mut stopped = self.idle_stop.subscribe();
        if *stopped.borrow() {
            return;
        }
        let _ = stopped.changed().await;
    }

    pub fn try_acquire_socket(&self, session_id: Uuid) -> Option<CollabSocketPermit> {
        if self.shutting_down.load(Ordering::Relaxed) {
            return None;
        }
        let mut counts = self.session_sockets.counts.lock().ok()?;
        let held = counts.get(&session_id).copied().unwrap_or(0);
        if held >= self.session_sockets.max_per_session {
            return None;
        }
        let global = self.socket_permits.clone().try_acquire_owned().ok()?;
        counts.insert(session_id, held + 1);
        Some(CollabSocketPermit {
            _global: global,
            session_id,
            tracker: self.session_sockets.clone(),
        })
    }

    #[cfg(feature = "db-tests")]
    pub fn available_collab_sockets(&self) -> usize {
        self.socket_permits.available_permits()
    }

    pub fn backend(&self) -> &Backend {
        &self.backend
    }

    pub fn engine_bin(&self) -> std::path::PathBuf {
        self.config.engine_bin.clone()
    }

    pub fn limits(&self) -> collab_engine::limits::Limits {
        self.config.limits
    }

    pub fn rpc_timeout(&self) -> Duration {
        Duration::from_millis(self.config.rpc_timeout_ms.max(1))
    }

    /// Borrow a live actor for one operation. The lease defers idle and
    /// admission reclaim until the operation has finished.
    async fn live_handle(&self, key: RoomKey) -> Option<(RoomHandle, JoiningLease)> {
        let slot = self.room_slot(key).await?;
        let mut phase = slot.phase.lock().await;
        match &mut *phase {
            RoomPhase::Live(live) if !live.handle.is_closed() => {
                live.last_activity = Instant::now();
                Some((live.handle.clone(), JoiningLease::register(&live.joining)))
            }
            _ => None,
        }
    }

    pub async fn capture_if_live(
        &self,
        key: impl Into<RoomKey>,
        actor_user_id: Uuid,
        session_id: Uuid,
    ) -> Option<Result<CapturedRevision, RevisionCaptureError>> {
        let key: RoomKey = key.into();
        let (handle, _lease) = self.live_handle(key).await?;
        Some(handle.capture_revision(actor_user_id, session_id).await)
    }

    pub(crate) async fn capture_if_live_guarded(
        &self,
        key: impl Into<RoomKey>,
        actor: Uuid,
        credential: Uuid,
    ) -> Option<Result<crate::collab::room::GuardedCapturedRevision, RevisionCaptureError>> {
        let (handle, _lease) = self.live_handle(key.into()).await?;
        Some(handle.capture_revision_guarded(actor, credential).await)
    }

    /// Tests only: drops the joining lease, so idle eviction or admission
    /// reclaim may close the room under the returned handle. Production
    /// borrows go through `borrow_live_room`, which keeps the lease.
    #[cfg(feature = "db-tests")]
    pub async fn ensure_live_room(&self, key: impl Into<RoomKey>) -> Result<RoomHandle, JoinError> {
        self.borrow_live_room(key, None)
            .await
            .map(|(handle, _lease)| handle)
    }

    /// Start or reuse the room and borrow its actor for one operation. A room
    /// whose actor has exited is reclaimed as a join reclaims it, so the next
    /// pass starts a successor instead of waiting for the idle timer.
    async fn borrow_live_room(
        &self,
        key: impl Into<RoomKey>,
        identity: Option<(Uuid, Uuid)>,
    ) -> Result<(RoomHandle, JoiningLease), JoinError> {
        let key: RoomKey = key.into();
        let mut retries = 0u8;
        loop {
            if self.shutting_down.load(Ordering::Acquire) {
                return Err(JoinError::EngineUnavailable);
            }
            let LiveSlot { slot, lease } = self.get_or_create_room(key, identity).await?;
            #[cfg(feature = "db-tests")]
            pause_for_hub_join_barrier(key.1, HUB_BORROW_BARRIER_AFTER_SLOT_READY).await;
            let borrow = {
                let mut phase = slot.phase.lock().await;
                Self::borrow_live_phase(&mut phase)
            };
            match borrow {
                LiveBorrow::Live(handle) => {
                    // The admission lease stays with the operation until it finishes.
                    return Ok((handle, lease));
                }
                LiveBorrow::Dead(live) => {
                    drop(lease);
                    self.spawn_reclaim(key, slot, live);
                }
                LiveBorrow::NotLive => {
                    drop(lease);
                    let _ = self.wait_for_live_or_retry(key, slot).await?;
                }
            }
            if !Self::allow_pre_enqueue_retry(&mut retries) {
                return Err(JoinError::EngineUnavailable);
            }
        }
    }

    pub async fn restore_revision(
        &self,
        key: impl Into<RoomKey>,
        actor_user_id: Uuid,
        session_id: Uuid,
        snap: Vec<u8>,
        intent: crate::db::revisions::RestoreRevisionInput,
    ) -> Result<Uuid, RevisionRestoreError> {
        let key: RoomKey = key.into();
        let (handle, _lease) = self
            .borrow_live_room(key, Some((actor_user_id, session_id)))
            .await
            .map_err(|_| RevisionRestoreError::Unavailable)?;
        handle
            .restore_from_snapshot(actor_user_id, session_id, snap, intent)
            .await
    }

    /// External body write through the room actor (source `replaceBody`).
    pub async fn replace_body(
        &self,
        key: impl Into<RoomKey>,
        actor_user_id: Uuid,
        session_id: Uuid,
        seed: Vec<u8>,
        expected_tail_seq: Option<i64>,
    ) -> Result<(), BodyWriteError> {
        let key: RoomKey = key.into();
        let (handle, _lease) = self
            .borrow_live_room(key, Some((actor_user_id, session_id)))
            .await
            .map_err(join_to_body_write_error)?;
        handle
            .replace_body(actor_user_id, session_id, seed, expected_tail_seq)
            .await
    }

    /// Live Tiptap projection for a read-modify-write (source `withLiveDoc`).
    pub async fn project_live(
        &self,
        key: impl Into<RoomKey>,
        actor_user_id: Uuid,
        session_id: Uuid,
    ) -> Result<LiveProjection, BodyWriteError> {
        let key: RoomKey = key.into();
        let (handle, _lease) = self
            .borrow_live_room(key, Some((actor_user_id, session_id)))
            .await
            .map_err(join_to_body_write_error)?;
        handle.project_live(actor_user_id, session_id).await
    }

    #[cfg(feature = "db-tests")]
    pub fn pending_family_start_count(&self) -> usize {
        self.pending_family_starts
            .lock()
            .expect("family startup owner mutex")
            .len()
    }

    #[cfg(feature = "db-tests")]
    pub fn unresolved_family_owner(&self, key: RoomKey) -> Option<Uuid> {
        self.pending_family_starts
            .lock()
            .expect("family room owner mutex")
            .get(&key)
            .map(|record| match record {
                FamilyRoomOwnerRecord::Startup { owner, .. } => *owner,
                FamilyRoomOwnerRecord::NativeWrite { fence, .. } => fence.owner_token,
            })
    }
    #[cfg(feature = "db-tests")]
    pub fn can_reserve_room_memory(&self) -> bool {
        self.memory_ledger.try_reserve(0).is_some()
    }

    /// Owned reservations only; global helper RSS is an admission input,
    /// not evidence that this hub retained or released its startup memory.
    #[cfg(feature = "db-tests")]
    pub fn outstanding_room_memory_bytes(&self) -> u64 {
        self.memory_ledger.outstanding()
    }

    #[cfg(feature = "db-tests")]
    pub async fn probe_actor(&self, key: impl Into<RoomKey>) -> crate::collab::room::ActorProbe {
        let key: RoomKey = key.into();
        let handle = {
            let Some(slot) = self.room_slot(key).await else {
                return crate::collab::room::ActorProbe {
                    connections: 0,
                    awareness_clients: 0,
                };
            };
            let phase = slot.phase.lock().await;
            match &*phase {
                RoomPhase::Live(live) => live.handle.clone(),
                _ => {
                    return crate::collab::room::ActorProbe {
                        connections: 0,
                        awareness_clients: 0,
                    };
                }
            }
        };
        handle.probe().await
    }

    #[cfg(feature = "db-tests")]
    pub async fn idle_evict_decision(&self, key: impl Into<RoomKey>) -> IdleEvictDecision {
        let key: RoomKey = key.into();
        let Some(slot) = self.room_slot(key).await else {
            return IdleEvictDecision::NotApplicable;
        };
        let phase = slot.phase.lock().await;
        match &*phase {
            RoomPhase::Live(live) => Self::idle_evict_decision_for(live, self.config.idle_evict_ms),
            _ => IdleEvictDecision::NotApplicable,
        }
    }

    /// Age a live room's last activity past the admission-reclaim grace.
    #[cfg(feature = "db-tests")]
    pub async fn age_room_past_reclaim_grace(&self, key: impl Into<RoomKey>) {
        let key: RoomKey = key.into();
        let Some(slot) = self.room_slot(key).await else {
            return;
        };
        let aged = Instant::now()
            .checked_sub(self.reclaim_grace() + Duration::from_millis(1))
            .expect("monotonic clock is older than the reclaim grace");
        let mut phase = slot.phase.lock().await;
        if let RoomPhase::Live(live) = &mut *phase {
            live.last_activity = aged;
        }
    }

    #[cfg(feature = "db-tests")]
    pub async fn force_room_idle_eligible(&self, key: impl Into<RoomKey>) {
        let key: RoomKey = key.into();
        let Some(slot) = self.room_slot(key).await else {
            return;
        };
        let mut phase = slot.phase.lock().await;
        if let RoomPhase::Live(live) = &mut *phase {
            live.last_activity =
                Instant::now() - Duration::from_millis(self.config.idle_evict_ms + 1);
        }
    }

    #[cfg(feature = "db-tests")]
    pub async fn execute_idle_evict_if_eligible(&self, key: impl Into<RoomKey>) -> bool {
        let key: RoomKey = key.into();
        let Some(slot) = self.room_slot(key).await else {
            return false;
        };
        let live = {
            let mut phase = slot.phase.lock().await;
            let Some(live) = Self::take_idle_evictable(&mut phase, self.config.idle_evict_ms)
            else {
                return false;
            };
            live
        };
        complete_owned_room_cleanup(
            self.rooms.clone(),
            key,
            slot,
            live,
            self.abnormal_actor_completions.clone(),
        )
        .await;
        true
    }

    /// Take the actor of a Live room that idle eviction may close, publishing
    /// Closing atomically with the idle observation. The caller owns the room
    /// from here and must finish it with [`complete_owned_room_cleanup`]; the
    /// slot never shows Failed before this exact actor and guard have finished.
    fn take_idle_evictable(phase: &mut RoomPhase, idle_ms: u64) -> Option<LiveRoom> {
        let RoomPhase::Live(live) = &*phase else {
            return None;
        };
        if Self::idle_evict_decision_for(live, idle_ms) != IdleEvictDecision::WouldEvict {
            return None;
        }
        match std::mem::replace(phase, RoomPhase::Closing) {
            RoomPhase::Live(live) => Some(live),
            _ => unreachable!("checked Live under the same lock"),
        }
    }

    fn idle_evict_decision_for(live: &LiveRoom, idle_ms: u64) -> IdleEvictDecision {
        if live.handle.is_closed() {
            return IdleEvictDecision::WouldEvict;
        }
        if live.joining.load(Ordering::Acquire) > 0 {
            return IdleEvictDecision::DeferredJoining;
        }
        if live.live_conns.load(Ordering::Acquire) > 0 {
            return IdleEvictDecision::DeferredMembers;
        }
        let idle = Duration::from_millis(idle_ms);
        if live.last_activity.elapsed() < idle {
            return IdleEvictDecision::DeferredActivity;
        }
        IdleEvictDecision::WouldEvict
    }

    #[cfg(feature = "db-tests")]
    pub async fn room_joining_count(&self, key: impl Into<RoomKey>) -> usize {
        let key: RoomKey = key.into();
        let Some(slot) = self.room_slot(key).await else {
            return 0;
        };
        let phase = slot.phase.lock().await;
        match &*phase {
            RoomPhase::Live(live) => live.joining.load(Ordering::Acquire),
            _ => 0,
        }
    }

    #[cfg(feature = "db-tests")]
    pub async fn room_member_count(&self, key: impl Into<RoomKey>) -> usize {
        let key: RoomKey = key.into();
        let Some(slot) = self.room_slot(key).await else {
            return 0;
        };
        let phase = slot.phase.lock().await;
        match &*phase {
            RoomPhase::Live(live) => live.live_conns.load(Ordering::Acquire),
            _ => 0,
        }
    }

    #[cfg(feature = "db-tests")]
    pub fn available_room_slots(&self) -> usize {
        self.room_permits.available_permits()
    }

    #[cfg(feature = "db-tests")]
    pub fn spawn_panicking_start_task_for_tests(&self) {
        self.starts
            .lock()
            .expect("room start task list")
            .push(tokio::spawn(async {
                panic!("injected collaboration startup failure");
            }));
    }

    #[cfg(feature = "db-tests")]
    pub async fn room_waiter_count(&self, key: impl Into<RoomKey>) -> usize {
        let key: RoomKey = key.into();
        self.room_slot(key)
            .await
            .map_or(0, |slot| slot.waiters.load(Ordering::Acquire))
    }

    #[cfg(feature = "db-tests")]
    pub async fn room_occupies_slot(&self, key: impl Into<RoomKey>) -> bool {
        let key: RoomKey = key.into();
        matches!(
            self.room_lifecycle_phase(key).await,
            RoomLifecyclePhase::Starting
                | RoomLifecyclePhase::Booting
                | RoomLifecyclePhase::Live
                | RoomLifecyclePhase::Closing
        )
    }

    #[cfg(feature = "db-tests")]
    pub async fn room_lifecycle_phase(&self, key: impl Into<RoomKey>) -> RoomLifecyclePhase {
        let key: RoomKey = key.into();
        let slot = self.room_slot(key).await;
        let Some(slot) = slot else {
            return RoomLifecyclePhase::Absent;
        };
        let phase = slot.phase.lock().await;
        match &*phase {
            RoomPhase::Starting => RoomLifecyclePhase::Starting,
            RoomPhase::Booting(_) => RoomLifecyclePhase::Booting,
            RoomPhase::Live(_) => RoomLifecyclePhase::Live,
            RoomPhase::Closing => RoomLifecyclePhase::Closing,
            RoomPhase::Failed => RoomLifecyclePhase::Failed,
        }
    }

    pub async fn join_room(
        &self,
        key: impl Into<RoomKey>,
        mut join: RoomJoin,
    ) -> Result<ConnectionLease, JoinError> {
        let key: RoomKey = key.into();
        if self.shutting_down.load(Ordering::Acquire) {
            return Err(JoinError::EngineUnavailable);
        }
        let admission = resolve_collab_admission_kind_backend(
            &self.backend,
            key.2,
            key.0,
            join.conn.session.user_id,
            join.conn.session.session_id,
            key.1,
        )
        .await
        .map_err(|err| {
            warn_join_db_error("hub.join_room.admission", key.0, key.1, &err);
            JoinError::DbError
        })?;
        admission.map_err(|_| JoinError::AdmissionDenied)?;
        #[cfg(feature = "db-tests")]
        pause_for_hub_join_barrier(key.1, HUB_JOIN_BARRIER_AFTER_ADMISSION).await;

        let mut retries = 0u8;
        loop {
            if self.shutting_down.load(Ordering::Acquire) {
                return Err(JoinError::EngineUnavailable);
            }
            let LiveSlot {
                slot,
                lease: joining_lease,
            } = self
                .get_or_create_room(
                    key,
                    Some((join.conn.session.user_id, join.conn.session.session_id)),
                )
                .await?;
            #[cfg(feature = "db-tests")]
            pause_for_hub_join_barrier(key.1, HUB_JOIN_BARRIER_AFTER_SLOT_READY).await;

            let borrow = {
                let mut phase = slot.phase.lock().await;
                Self::borrow_live_phase(&mut phase)
            };

            match borrow {
                LiveBorrow::Live(handle) => {
                    #[cfg(feature = "db-tests")]
                    pause_for_hub_join_barrier(key.1, HUB_JOIN_BARRIER_BEFORE_ACTOR_JOIN).await;
                    let delivery = handle.deliver_join(join).await;
                    drop(joining_lease);
                    #[cfg(feature = "db-tests")]
                    pause_for_hub_join_barrier(key.1, HUB_JOIN_BARRIER_AFTER_ACTOR_REPLY).await;
                    match delivery {
                        JoinDelivery::Replied(result) => return result,
                        JoinDelivery::QueueFull => return Err(JoinError::RoomFull),
                        JoinDelivery::NoReply => return Err(JoinError::EngineUnavailable),
                        JoinDelivery::NotDelivered(recovered) => {
                            if let Some(live) = self.claim_dead_live(key, &slot).await {
                                self.spawn_reclaim(key, slot, live);
                            }
                            join = recovered;
                            if !Self::allow_pre_enqueue_retry(&mut retries) {
                                return Err(JoinError::EngineUnavailable);
                            }
                        }
                    }
                }
                LiveBorrow::Dead(live) => {
                    drop(joining_lease);
                    self.spawn_reclaim(key, slot, live);
                    if !Self::allow_pre_enqueue_retry(&mut retries) {
                        return Err(JoinError::EngineUnavailable);
                    }
                }
                LiveBorrow::NotLive => {
                    drop(joining_lease);
                    let _ = self.wait_for_live_or_retry(key, slot).await?;
                    if !Self::allow_pre_enqueue_retry(&mut retries) {
                        return Err(JoinError::EngineUnavailable);
                    }
                }
            }
        }
    }

    fn allow_pre_enqueue_retry(retries: &mut u8) -> bool {
        if *retries >= MAX_PRE_ENQUEUE_RETRIES {
            return false;
        }
        *retries += 1;
        true
    }

    /// Take the actor of a slot a join or borrow got back Live. Refreshes the
    /// room's activity when the actor runs. When it has exited, moves the slot
    /// to Closing and hands its [`LiveRoom`] to the caller, who must pass it to
    /// [`Self::spawn_reclaim`]; dropping it would leave the slot Closing.
    fn borrow_live_phase(phase: &mut RoomPhase) -> LiveBorrow {
        if let Some(dead) = Self::take_dead_live(phase) {
            return LiveBorrow::Dead(dead);
        }
        match phase {
            RoomPhase::Live(live) => {
                live.last_activity = Instant::now();
                LiveBorrow::Live(live.handle.clone())
            }
            _ => LiveBorrow::NotLive,
        }
    }

    fn take_dead_live(phase: &mut RoomPhase) -> Option<LiveRoom> {
        match &*phase {
            RoomPhase::Live(live) if live.handle.is_closed() => {}
            _ => return None,
        }
        match std::mem::replace(phase, RoomPhase::Closing) {
            RoomPhase::Live(live) => Some(live),
            other => {
                *phase = other;
                None
            }
        }
    }

    async fn claim_dead_live(&self, key: RoomKey, slot: &Arc<RoomSlot>) -> Option<LiveRoom> {
        let current = self.room_slot(key).await?;
        if !Arc::ptr_eq(&current, slot) {
            return None;
        }
        let mut phase = current.phase.lock().await;
        Self::take_dead_live(&mut phase)
    }

    fn spawn_reclaim(&self, key: RoomKey, slot: Arc<RoomSlot>, live: LiveRoom) {
        let rooms = self.rooms.clone();
        let abnormal_actor_completions = self.abnormal_actor_completions.clone();
        let mut starts = self.starts.lock().expect("room start task list");
        starts.retain(|task| !task.is_finished());
        starts.push(tokio::spawn(async move {
            #[cfg(feature = "db-tests")]
            pause_for_reclaim_barrier(key.1).await;
            complete_owned_room_cleanup(rooms, key, slot, live, abnormal_actor_completions).await;
        }));
    }

    pub async fn leave_room(&self, key: impl Into<RoomKey>, conn_id: Uuid) {
        let key: RoomKey = key.into();
        let slot = self.room_slot(key).await;
        if let Some(slot) = slot {
            let handle = {
                let mut phase = slot.phase.lock().await;
                if let RoomPhase::Live(live) = &mut *phase {
                    if !live.handle.is_closed() {
                        live.last_activity = Instant::now();
                    }
                    Some(live.handle.clone())
                } else {
                    None
                }
            };
            if let Some(handle) = handle {
                handle.leave(conn_id).await;
            }
        }
    }

    pub async fn send_frame(&self, key: impl Into<RoomKey>, conn_id: Uuid, bytes: Vec<u8>) {
        let key: RoomKey = key.into();
        if self.shutting_down.load(Ordering::Acquire) {
            return;
        }
        let slot = self.room_slot(key).await;
        if let Some(slot) = slot {
            let handle = {
                let mut phase = slot.phase.lock().await;
                if let RoomPhase::Live(live) = &mut *phase {
                    if !live.handle.is_closed() {
                        live.last_activity = Instant::now();
                    }
                    Some(live.handle.clone())
                } else {
                    None
                }
            };
            if let Some(handle) = handle {
                handle.frame(conn_id, bytes).await;
            }
        }
    }

    pub async fn shutdown(&self) -> ShutdownStatus {
        let _shutdown = self.shutdown_lock.lock().await;
        self.begin_shutdown();
        // Eviction that already owns an actor keeps that ownership. Startup that
        // already holds a guard/actor is joined, not aborted. Independent live
        // rooms close concurrently so one lock-blocked actor cannot stall the rest.
        let idle = self.idle_task.lock().await.take();
        let starts = std::mem::take(&mut *self.starts.lock().expect("room start task list"));
        let live_rooms = self.take_live_rooms_for_shutdown().await;

        let idle_join = async {
            match idle {
                Some(task) => match task.await {
                    Ok(()) => false,
                    Err(error) => {
                        tracing::error!(%error, "collaboration eviction task failed");
                        true
                    }
                },
                None => false,
            }
        };
        let starts_join = async {
            #[cfg(feature = "db-tests")]
            self.signal_shutdown_drain_witness().await;
            let mut failures = 0usize;
            for result in join_all(starts).await {
                if let Err(error) = result {
                    tracing::error!(%error, "collaboration startup task failed");
                    failures += 1;
                }
            }
            failures
        };
        let rooms_join = async {
            // Each room leaves the map as soon as its own actor finished, so a
            // sibling blocked on persist cannot keep this room visible as Closing.
            join_all(live_rooms.into_iter().map(|(key, slot, live)| {
                complete_owned_room_cleanup(
                    self.rooms.clone(),
                    key,
                    slot,
                    live,
                    self.abnormal_actor_completions.clone(),
                )
            }))
            .await;
        };
        let (idle_task_failed, mut start_task_failures, _) =
            tokio::join!(idle_join, starts_join, rooms_join);

        loop {
            let more = std::mem::take(&mut *self.starts.lock().expect("room start task list"));
            if more.is_empty() {
                break;
            }
            for result in join_all(more).await {
                if let Err(error) = result {
                    tracing::error!(%error, "collaboration startup task failed");
                    start_task_failures += 1;
                }
            }
        }

        let leftover = self
            .rooms
            .read()
            .await
            .iter()
            .map(|(key, slot)| (*key, slot.clone()))
            .collect::<Vec<_>>();
        for (key, slot) in leftover {
            let closing = {
                let phase = slot.phase.lock().await;
                matches!(*phase, RoomPhase::Closing)
            };
            if closing {
                self.wait_for_closing_owner(key, slot).await;
                continue;
            }
            self.force_close_slot(key, slot).await;
        }
        self.rooms.write().await.clear();

        let socket_cap = u32::try_from(self.config.max_collab_sockets).unwrap_or(u32::MAX);
        if socket_cap > 0 {
            if let Ok(held) = self
                .socket_permits
                .clone()
                .acquire_many_owned(socket_cap)
                .await
            {
                drop(held);
            }
        }
        for (key, record) in self
            .pending_family_starts
            .lock()
            .expect("family room owner mutex")
            .iter()
        {
            match record {
                FamilyRoomOwnerRecord::Startup {
                    owner,
                    error,
                    deadline_expired,
                } => {
                    start_task_failures = start_task_failures.saturating_add(1);
                    tracing::error!(workspace_id=%key.0,document_id=%key.1,owner=%owner,deadline_expired,
                        error=?error.as_ref().map(|error|error.source_error()), "family startup owner remains unconfirmed");
                }
                FamilyRoomOwnerRecord::NativeWrite { fence, error } => {
                    tracing::error!(workspace_id=%key.0,document_id=%key.1,fence=fence.fence,%error,
                        "family native writer original remote outcome remains unconfirmed");
                }
            }
        }
        let actor_failures = self.abnormal_actor_completions.load(Ordering::Acquire);
        ShutdownStatus {
            idle_task_failed,
            start_task_failures,
            actor_failures,
        }
    }

    async fn room_slot(&self, key: RoomKey) -> Option<Arc<RoomSlot>> {
        self.rooms.read().await.get(&key).cloned()
    }

    async fn get_or_create_room(
        &self,
        key: RoomKey,
        identity: Option<(Uuid, Uuid)>,
    ) -> Result<LiveSlot, JoinError> {
        let mut reclaims = 0u8;
        loop {
            if self.shutting_down.load(Ordering::Acquire) {
                return Err(JoinError::EngineUnavailable);
            }

            if let Some(slot) = self.room_slot(key).await {
                if let Some(live_slot) = self.wait_for_live_or_retry(key, slot).await? {
                    return Ok(live_slot);
                }
            }

            let reserved = match self.try_reserve_starting(key).await {
                Err(JoinError::RoomFull) if reclaims < MAX_ADMISSION_RECLAIMS => {
                    reclaims += 1;
                    if self.reclaim_empty_room_for_admission(key).await {
                        continue;
                    }
                    return Err(JoinError::RoomFull);
                }
                other => other?,
            };
            match reserved {
                ReserveOutcome::Existing(slot) => {
                    if let Some(live_slot) = self.wait_for_live_or_retry(key, slot).await? {
                        return Ok(live_slot);
                    }
                }
                ReserveOutcome::Creator(slot) => {
                    // The hub, not a cancellable HTTP/socket caller, owns startup.
                    // An abandoned caller leaves a normal zero-client room which
                    // idle eviction reclaims; another caller can join it meanwhile.
                    // Its lease travels in the reply and is released when the
                    // reply is dropped undelivered.
                    let (reply, result) = tokio::sync::oneshot::channel();
                    let hub = self.clone();
                    let registered = {
                        let mut starts = self.starts.lock().expect("room start task list");
                        if self.shutting_down.load(Ordering::Acquire) {
                            false
                        } else {
                            starts.retain(|task| !task.is_finished());
                            let startup_slot = slot.clone();
                            starts.push(tokio::spawn(async move {
                                let outcome = std::panic::AssertUnwindSafe(
                                    hub.start_room(key, startup_slot.clone(), identity),
                                )
                                .catch_unwind()
                                .await;
                                let outcome = match outcome {
                                    Ok(outcome) => outcome,
                                    Err(_) => {
                                        hub.cleanup_starting(key, &startup_slot).await;
                                        tracing::error!(document_id = %key.1, "collaboration startup panicked");
                                        Err(JoinError::EngineUnavailable)
                                    }
                                };
                                let _ = reply.send(outcome);
                            }));
                            true
                        }
                    };
                    if !registered {
                        self.cleanup_starting(key, &slot).await;
                        return Err(JoinError::EngineUnavailable);
                    }
                    return result.await.map_err(|_| JoinError::EngineUnavailable)?;
                }
            }
        }
    }

    /// At the room cap, close the least recently active live room that has no
    /// members, no join in flight and no borrowed operation, and whose last
    /// activity is older than the reclaim grace, instead of making the new room
    /// wait for the idle timer. Rooms with members are never reclaimed, so a cap
    /// reached by active rooms still reports `RoomFull`.
    /// Returns whether a room was closed (the caller retries its reservation).
    async fn reclaim_empty_room_for_admission(&self, admitted: RoomKey) -> bool {
        let grace = self.reclaim_grace();
        let mut claimed = None;
        // A candidate lost between the scan and the locked re-check (to another
        // admission, idle eviction or a returning member) makes this admission
        // scan again instead of reporting `RoomFull`, so concurrent admissions at
        // the cap each take a different empty room. A lost candidate normally
        // stays out of later passes (Closing never returns to Live; a join or
        // borrow refreshes its activity), so each pass has one candidate fewer.
        // The bound, one pass more than the map can hold rooms, keeps a room
        // whose joins keep being cancelled from spinning the admission.
        for _ in 0..=self.config.max_rooms {
            if self.shutting_down.load(Ordering::Acquire) {
                return false;
            }
            let Some((key, slot)) = self.oldest_reclaimable_room(grace).await else {
                return false;
            };
            #[cfg(feature = "db-tests")]
            pause_for_hub_join_barrier(admitted.1, HUB_JOIN_BARRIER_AFTER_RECLAIM_SCAN).await;
            let live = {
                let mut phase = slot.phase.lock().await;
                if self.shutting_down.load(Ordering::Acquire) {
                    return false;
                }
                let still_reclaimable = matches!(
                    &*phase,
                    RoomPhase::Live(live) if Self::reclaimable_at_cap(live, grace)
                );
                if !still_reclaimable {
                    continue;
                }
                let RoomPhase::Live(live) = std::mem::replace(&mut *phase, RoomPhase::Closing)
                else {
                    unreachable!()
                };
                live
            };
            claimed = Some((key, slot, live));
            break;
        }
        let Some((key, slot, live)) = claimed else {
            return false;
        };
        tracing::info!(
            workspace_id = %key.0,
            document_id = %key.1,
            admitted_document_id = %admitted.1,
            "collab room reclaimed at room cap"
        );
        // The hub owns the Closing room from here; a cancelled caller must not
        // leave it Closing with its permit held.
        let rooms = self.rooms.clone();
        let abnormal_actor_completions = self.abnormal_actor_completions.clone();
        let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();
        {
            let mut starts = self.starts.lock().expect("room start task list");
            starts.retain(|task| !task.is_finished());
            starts.push(tokio::spawn(async move {
                live.handle.shutdown_after_queued().await;
                finish_owned_room_cleanup(rooms, key, slot, live, abnormal_actor_completions).await;
                let _ = done_tx.send(());
            }));
        }
        // The admission waits for the victim's teardown (engine kill and thread
        // join, fence guard release) plus the rest of a last-disconnect session
        // revision still running, which only a revision outlasting the reclaim
        // grace can be. No inner timeout: HTTP callers are already bounded by
        // rpc_timeout, and a WebSocket join would turn a near success into
        // RoomFull and a client backoff.
        done_rx.await.is_ok()
    }

    /// The least recently active room that [`Self::reclaimable_at_cap`] accepts.
    async fn oldest_reclaimable_room(&self, grace: Duration) -> Option<(RoomKey, Arc<RoomSlot>)> {
        let entries = self
            .rooms
            .read()
            .await
            .iter()
            .map(|(key, slot)| (*key, slot.clone()))
            .collect::<Vec<_>>();
        let mut oldest: Option<(Instant, RoomKey, Arc<RoomSlot>)> = None;
        for (key, slot) in entries {
            // Wait out a busy phase lock (held only for a state change) rather
            // than skip the room: another admission's re-check, a join or idle
            // eviction holding it must not hide the only reclaimable room.
            let at = {
                let phase = slot.phase.lock().await;
                match &*phase {
                    RoomPhase::Live(live) if Self::reclaimable_at_cap(live, grace) => {
                        live.last_activity
                    }
                    _ => continue,
                }
            };
            if oldest
                .as_ref()
                .is_none_or(|(oldest_at, _, _)| at < *oldest_at)
            {
                oldest = Some((at, key, slot));
            }
        }
        oldest.map(|(_, key, slot)| (key, slot))
    }

    /// How long an emptied room stays out of admission reclaim, so a reload or
    /// reconnect finds it still live: the RPC timeout with a floor of a few
    /// seconds, never longer than the idle timer that would evict it anyway.
    fn reclaim_grace(&self) -> Duration {
        let grace_ms = self
            .config
            .rpc_timeout_ms
            .max(RECLAIM_GRACE_FLOOR_MS)
            .min(self.config.idle_evict_ms);
        Duration::from_millis(grace_ms)
    }

    fn reclaimable_at_cap(live: &LiveRoom, grace: Duration) -> bool {
        live.handle.is_closed()
            || (live.joining.load(Ordering::Acquire) == 0
                && live.live_conns.load(Ordering::Acquire) == 0
                && live.last_activity.elapsed() >= grace)
    }

    async fn wait_for_live_or_retry(
        &self,
        key: RoomKey,
        slot: Arc<RoomSlot>,
    ) -> Result<Option<LiveSlot>, JoinError> {
        #[cfg(feature = "db-tests")]
        let _waiting = {
            struct Waiting(Arc<RoomSlot>);
            impl Drop for Waiting {
                fn drop(&mut self) {
                    self.0.waiters.fetch_sub(1, Ordering::AcqRel);
                }
            }
            slot.waiters.fetch_add(1, Ordering::AcqRel);
            Waiting(slot.clone())
        };
        loop {
            if self.shutting_down.load(Ordering::Acquire) {
                return Err(JoinError::EngineUnavailable);
            }

            let notified = slot.ready.notified();
            {
                let phase = slot.phase.lock().await;
                match &*phase {
                    RoomPhase::Live(live) => {
                        let lease = JoiningLease::register(&live.joining);
                        return Ok(Some(LiveSlot {
                            slot: slot.clone(),
                            lease,
                        }));
                    }
                    RoomPhase::Failed => return Ok(None),
                    RoomPhase::Starting | RoomPhase::Booting(_) | RoomPhase::Closing => {}
                }
            }
            notified.await;

            if self.rooms.read().await.get(&key).is_none() {
                return Ok(None);
            }
        }
    }

    async fn try_reserve_starting(&self, key: RoomKey) -> Result<ReserveOutcome, JoinError> {
        if self.shutting_down.load(Ordering::Acquire) {
            return Err(JoinError::EngineUnavailable);
        }

        if let Some(existing) = self.room_slot(key).await {
            let phase = existing.phase.lock().await;
            match *phase {
                RoomPhase::Live(_)
                | RoomPhase::Starting
                | RoomPhase::Booting(_)
                | RoomPhase::Closing => {
                    return Ok(ReserveOutcome::Existing(existing.clone()));
                }
                RoomPhase::Failed => {
                    drop(phase);
                    let mut rooms = self.rooms.write().await;
                    if rooms
                        .get(&key)
                        .is_some_and(|slot| Arc::ptr_eq(slot, &existing))
                    {
                        rooms.remove(&key);
                    }
                }
            }
        }

        let mut rooms = self.rooms.write().await;
        if rooms.contains_key(&key) {
            return Ok(ReserveOutcome::Existing(
                rooms.get(&key).expect("key present").clone(),
            ));
        }
        {
            let owners = self
                .pending_family_starts
                .lock()
                .expect("family room owner mutex");
            if owners.contains_key(&key) {
                return Err(JoinError::CapacityRetry);
            }
            let unresolved = owners
                .keys()
                .filter(|owned| !rooms.contains_key(*owned))
                .count();
            if rooms.len().saturating_add(unresolved) >= self.config.max_rooms {
                return Err(JoinError::RoomFull);
            }
        }

        let slot = Arc::new(RoomSlot {
            phase: Mutex::new(RoomPhase::Starting),
            ready: Notify::new(),
            #[cfg(feature = "db-tests")]
            waiters: std::sync::atomic::AtomicUsize::new(0),
        });
        rooms.insert(key, slot.clone());
        Ok(ReserveOutcome::Creator(slot))
    }

    async fn wait_for_closing_owner(&self, key: RoomKey, slot: Arc<RoomSlot>) {
        loop {
            let notified = slot.ready.notified();
            {
                let phase = slot.phase.lock().await;
                if !matches!(*phase, RoomPhase::Closing) {
                    return;
                }
            }
            if self
                .rooms
                .read()
                .await
                .get(&key)
                .is_none_or(|existing| !Arc::ptr_eq(existing, &slot))
            {
                return;
            }
            notified.await;
        }
    }

    async fn start_room(
        &self,
        key: RoomKey,
        slot: Arc<RoomSlot>,
        identity: Option<(Uuid, Uuid)>,
    ) -> Result<LiveSlot, JoinError> {
        #[cfg(feature = "db-tests")]
        increment_room_start_count(key.1).await;
        if self.shutting_down.load(Ordering::Acquire) {
            self.cleanup_starting(key, &slot).await;
            return Err(JoinError::EngineUnavailable);
        }

        let permit = tokio::select! {
            biased;
            result = self.room_permits.clone().acquire_owned() => match result {
                Ok(permit) => permit,
                Err(_) => {
                    self.cleanup_starting(key, &slot).await;
                    return Err(JoinError::RoomFull);
                }
            },
            () = self.wait_if_shutting_down() => {
                self.cleanup_starting(key, &slot).await;
                return Err(JoinError::EngineUnavailable);
            }
        };

        {
            let mut phase = slot.phase.lock().await;
            if !matches!(*phase, RoomPhase::Starting) {
                drop(permit);
                return Err(JoinError::EngineUnavailable);
            }
            *phase = RoomPhase::Booting(permit);
        }

        if self.shutting_down.load(Ordering::Acquire) {
            self.cleanup_starting(key, &slot).await;
            return Err(JoinError::EngineUnavailable);
        }

        let prepared = async {
            match &self.backend {
                Backend::Postgres(pool) => {
                    let mut pooled = tokio::select! {
                        biased;
                        result = pool.acquire() => match result {
                            Ok(pooled) => pooled,
                            Err(err) => {
                                warn_join_db_error("hub.start_room.acquire", key.0, key.1, &err);
                                        return Err(JoinError::DbError);
                            }
                        },
                        () = self.wait_if_shutting_down() => {
                                return Err(JoinError::EngineUnavailable);
                        }
                    };
                    if self.shutting_down.load(Ordering::Acquire) {
                        drop(pooled);
                        return Err(JoinError::EngineUnavailable);
                    }
                    let persisted_bytes = match estimate_persisted_collab_bytes_kind(
                        &mut pooled,
                        key.2,
                        key.0,
                        key.1,
                    )
                    .await
                    {
                        Ok(bytes) => bytes,
                        Err(err) => {
                            warn_join_db_error("hub.start_room.estimate_bytes", key.0, key.1, &err);
                            drop(pooled);
                            return Err(JoinError::DbError);
                        }
                    };
                    let Some(memory_reservation) = self.memory_ledger.try_reserve(persisted_bytes)
                    else {
                        drop(pooled);
                        return Err(JoinError::CapacityRetry);
                    };
                    let guard = match RoomGuard::try_lock_pooled(pooled, key.1).await {
                        Ok(Some(guard)) => guard,
                        Ok(None) => {
                            return Err(JoinError::WriterStale);
                        }
                        Err(err) => {
                            warn_join_db_error("hub.start_room.room_guard", key.0, key.1, &err);
                            return Err(JoinError::DbError);
                        }
                    };
                    if self.shutting_down.load(Ordering::Acquire) {
                        guard.release().await;
                        return Err(JoinError::EngineUnavailable);
                    }
                    // Under the guard: the resource must still belong to this room's
                    // workspace. A join admitted before a same-ID MOVE committed would
                    // otherwise start a source room that keeps the guard from the
                    // destination room until idle eviction.
                    match resource_in_room_workspace(pool, key).await {
                        Ok(true) => {}
                        Ok(false) => {
                            guard.release().await;
                            return Err(JoinError::AdmissionDenied);
                        }
                        Err(err) => {
                            warn_join_db_error("hub.start_room.resource", key.0, key.1, &err);
                            guard.release().await;
                            return Err(JoinError::DbError);
                        }
                    }

                    Ok((BackendRoomGuard::Postgres(guard), memory_reservation, None))
                }
                Backend::Sqlite(_) | Backend::LibsqlRemote(_) => {
                    if key.2 != crate::collab::wire::CollabKind::Document {
                        return Err(JoinError::UnsupportedKind);
                    }
                    let (actor, credential) = identity.ok_or(JoinError::AdmissionDenied)?;
                    let timings = self.family_timings.ok_or(JoinError::EngineUnavailable)?;
                    let bytes = estimate_family_document_bytes(
                        &self.backend,
                        key.0,
                        actor,
                        credential,
                        key.1,
                    )
                    .await
                    .map_err(|err| {
                        warn_join_db_error("hub.start_room.estimate_bytes", key.0, key.1, &err);
                        JoinError::DbError
                    })?
                    .map_err(|_| JoinError::AdmissionDenied)?;
                    let reservation = self
                        .memory_ledger
                        .try_reserve(bytes)
                        .ok_or(JoinError::CapacityRetry)?;
                    let acquisition_owner=Uuid::now_v7();
                    self.pending_family_starts.lock().expect("family startup owner mutex").insert(key,FamilyRoomOwnerRecord::Startup { owner: acquisition_owner, error: None, deadline_expired: false });
                    #[cfg(feature = "db-tests")]
                    pause_for_hub_join_barrier(key.1,HUB_FAMILY_START_BEFORE_BEGIN).await;
                    let acquired = tokio::time::timeout(
                        Duration::from_millis(self.config.rpc_timeout_ms),
                        acquire_family_document_room_for_start(
                        &self.backend,
                        key.0,
                        actor,
                        credential,
                        key.1,
                        acquisition_owner,
                        timings.lease(),
                    )).await;
                    let acquired = match acquired {
                        Ok(acquired) => acquired,
                        Err(_) => {
                            // Dropping the unfinished remote future starts owned
                            // stream quarantine, but is not a cleanup receipt.
                            // Retain its exact owner, refuse fresh reconciliation.
                            self.pending_family_starts.lock().expect("family room owner mutex").insert(key,
                                FamilyRoomOwnerRecord::Startup { owner: acquisition_owner, error: None, deadline_expired: true });
                            self.abnormal_actor_completions.fetch_add(1,Ordering::Relaxed);
                            tracing::error!(workspace_id=%key.0,document_id=%key.1,
                                "room startup deadline expired; original stream cleanup unconfirmed");
                            return Err(JoinError::DbError);
                        }
                    };
                    let claim=match acquired {
                        Ok(result)=>{
                            // Success transfers the known token to the guard;
                            // a domain refusal follows explicit rollback.
                            self.pending_family_starts.lock().expect("family startup owner mutex").remove(&key);
                            result.map_err(|error| match error {
                                crate::db::collab::CollabDbError::StaleWriter=>JoinError::WriterStale,
                                _=>JoinError::AdmissionDenied,
                            })?
                        },
                        Err(error)=>{
                            warn_join_db_error("hub.start_room.room_guard",key.0,key.1,error.source_error());
                            let cleaned = if error.may_reconcile() {
                                matches!(tokio::time::timeout(Duration::from_millis(self.config.rpc_timeout_ms),
                                    abandon_family_document_room_start(&self.backend,key.0,key.1,acquisition_owner)).await, Ok(Ok(())))
                            } else {
                                // No exact original-stream cleanup receipt means
                                // neither driver failure nor Drop proves cleanup.
                                false
                            };
                            if cleaned {
                                self.pending_family_starts.lock().expect("family startup owner mutex").remove(&key);
                            } else {
                                self.pending_family_starts.lock().expect("family room owner mutex").insert(key,
                                    FamilyRoomOwnerRecord::Startup { owner: acquisition_owner, error: Some(Arc::new(error)), deadline_expired: false });
                                self.abnormal_actor_completions.fetch_add(1,Ordering::Relaxed);
                                tracing::error!(workspace_id=%key.0,document_id=%key.1,
                                    "room startup cleanup unconfirmed; backend drain/quarantine required");
                            }
                            return Err(JoinError::DbError);
                        }
                    };
                    let guard =
                        BackendRoomGuard::family(self.backend.clone(), claim.fence, timings, self.pending_family_starts.clone());
                    Ok((guard, reservation, Some(claim.native.load)))
                }
            }
        }
        .await;
        let (guard, memory_reservation, initial_load) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                self.cleanup_starting(key, &slot).await;
                return Err(error);
            }
        };
        if self.shutting_down.load(Ordering::Acquire) {
            if let Err(err) = guard
                .release_bounded(Duration::from_millis(self.config.rpc_timeout_ms))
                .await
            {
                self.abnormal_actor_completions
                    .fetch_add(1, Ordering::Relaxed);
                warn_join_db_error("hub.start_room.release", key.0, key.1, &err);
            }
            self.cleanup_starting(key, &slot).await;
            return Err(JoinError::EngineUnavailable);
        }

        let live_conns = Arc::new(AtomicUsize::new(0));
        let spawn = crate::collab::room::spawn_room_backend(
            key,
            self.config.clone(),
            self.backend.clone(),
            guard,
            initial_load,
            live_conns.clone(),
            memory_reservation,
        )
        .await;

        if self.shutting_down.load(Ordering::Acquire) {
            if let Ok((handle, finished)) = spawn {
                handle.shutdown().await;
                let _ = finished.await;
            }
            self.cleanup_starting(key, &slot).await;
            return Err(JoinError::EngineUnavailable);
        }

        match spawn {
            Ok((handle, finished)) => {
                let mut phase = slot.phase.lock().await;
                let RoomPhase::Booting(permit) = std::mem::replace(&mut *phase, RoomPhase::Failed)
                else {
                    handle.shutdown().await;
                    let _ = finished.await;
                    slot.ready.notify_waiters();
                    return Err(JoinError::EngineUnavailable);
                };
                // The creator's lease exists before any other task can see Live.
                let joining = Arc::new(AtomicUsize::new(0));
                let lease = JoiningLease::register(&joining);
                *phase = RoomPhase::Live(LiveRoom {
                    handle,
                    finished,
                    last_activity: Instant::now(),
                    live_conns,
                    joining,
                    permit,
                });
                slot.ready.notify_waiters();
                Ok(LiveSlot {
                    slot: slot.clone(),
                    lease,
                })
            }
            Err(err) => {
                if err == JoinError::DbError {
                    self.abnormal_actor_completions
                        .fetch_add(1, Ordering::Relaxed);
                }
                self.cleanup_starting(key, &slot).await;
                Err(err)
            }
        }
    }

    /// Abandon a start that has not published Live: fail the slot, free its
    /// permit and remove it. A no-op once the slot has left Starting/Booting.
    async fn cleanup_starting(&self, key: RoomKey, slot: &Arc<RoomSlot>) {
        let permit = {
            let mut phase = slot.phase.lock().await;
            match std::mem::replace(&mut *phase, RoomPhase::Failed) {
                RoomPhase::Starting => None,
                RoomPhase::Booting(permit) => Some(permit),
                other => {
                    *phase = other;
                    return;
                }
            }
        };
        drop(permit);
        retire_slot(&self.rooms, key, slot).await;
    }

    /// Close the room of a resource that a committed personal MOVE took out
    /// of `key`'s workspace, and return only once no room for `key` holds the
    /// resource guard. Rooms are fenced by a guard keyed by the resource
    /// alone, so the source room would otherwise keep the destination room
    /// from starting until idle eviction. A Live room is closed through a
    /// hub-owned cleanup task (connections close, the actor finishes and
    /// releases the guard); a starting or closing room is left to its owner
    /// and awaited. A start that begins later finds the resource gone under
    /// its guard (see `start_room`). No room is a no-op; no other room is
    /// touched.
    pub async fn retire_moved_resource_room(&self, key: impl Into<RoomKey>) {
        let key: RoomKey = key.into();
        loop {
            let Some(slot) = self.room_slot(key).await else {
                return;
            };
            // Registered before the phase is read, so a starter or closer that
            // finishes in between cannot be missed.
            let notified = slot.ready.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let live = {
                let mut phase = slot.phase.lock().await;
                // Failed is terminal: set only after the actor finished (guard
                // released) or before a guard existed. Starting, Booting and
                // Closing are owned by their starter or closer, which notify
                // `ready` when they publish Live or retire the slot.
                if matches!(*phase, RoomPhase::Failed) {
                    return;
                }
                if matches!(*phase, RoomPhase::Live(_)) {
                    match std::mem::replace(&mut *phase, RoomPhase::Closing) {
                        RoomPhase::Live(live) => Some(live),
                        _ => unreachable!("phase was Live under the lock"),
                    }
                } else {
                    None
                }
            };
            if let Some(live) = live {
                // The hub owns the Closing room from here (as at the room cap):
                // a cancelled caller cannot leave it Closing. The caller waits
                // until the actor finished and the guard was released.
                let rooms = self.rooms.clone();
                let abnormal_actor_completions = self.abnormal_actor_completions.clone();
                let owned = slot.clone();
                let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();
                {
                    let mut starts = self.starts.lock().expect("room start task list");
                    starts.retain(|task| !task.is_finished());
                    starts.push(tokio::spawn(async move {
                        complete_owned_room_cleanup(
                            rooms,
                            key,
                            owned,
                            live,
                            abnormal_actor_completions,
                        )
                        .await;
                        let _ = done_tx.send(());
                    }));
                }
                let _ = done_rx.await;
                return;
            }
            #[cfg(feature = "db-tests")]
            let _waiting = {
                struct Waiting(Arc<RoomSlot>);
                impl Drop for Waiting {
                    fn drop(&mut self) {
                        self.0.waiters.fetch_sub(1, Ordering::AcqRel);
                    }
                }
                slot.waiters.fetch_add(1, Ordering::AcqRel);
                Waiting(slot.clone())
            };
            notified.await;
        }
    }

    /// Retire the source rooms of every resource a committed MOVE moved, each
    /// as [`Self::retire_moved_resource_room`], in ONE hub-owned task enrolled
    /// before this method's first await (as the room-cap reclaim): a caller
    /// cancelled while one room is still starting or closing does not leave
    /// the others unretired. The caller waits for the whole pair.
    pub async fn retire_moved_resource_rooms(&self, keys: Vec<RoomKey>) {
        let hub = self.clone();
        let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();
        {
            let mut starts = self.starts.lock().expect("room start task list");
            starts.retain(|task| !task.is_finished());
            starts.push(tokio::spawn(async move {
                for key in keys {
                    hub.retire_moved_resource_room(key).await;
                }
                let _ = done_tx.send(());
            }));
        }
        let _ = done_rx.await;
    }

    async fn force_close_slot(&self, key: RoomKey, slot: Arc<RoomSlot>) {
        let live = {
            let mut phase = slot.phase.lock().await;
            match std::mem::replace(&mut *phase, RoomPhase::Closing) {
                RoomPhase::Live(live) => Some(live),
                RoomPhase::Booting(permit) => {
                    drop(permit);
                    None
                }
                RoomPhase::Starting | RoomPhase::Failed | RoomPhase::Closing => None,
            }
        };
        match live {
            Some(live) => {
                complete_owned_room_cleanup(
                    self.rooms.clone(),
                    key,
                    slot,
                    live,
                    self.abnormal_actor_completions.clone(),
                )
                .await;
            }
            None => retire_slot(&self.rooms, key, &slot).await,
        }
    }

    async fn take_live_rooms_for_shutdown(&self) -> Vec<(RoomKey, Arc<RoomSlot>, LiveRoom)> {
        let entries = self
            .rooms
            .read()
            .await
            .iter()
            .map(|(key, slot)| (*key, slot.clone()))
            .collect::<Vec<_>>();
        let mut live = Vec::new();
        for (key, slot) in entries {
            let mut phase = slot.phase.lock().await;
            if matches!(*phase, RoomPhase::Live(_)) {
                if let RoomPhase::Live(room) = std::mem::replace(&mut *phase, RoomPhase::Closing) {
                    drop(phase);
                    live.push((key, slot, room));
                }
            }
        }
        live
    }
}

/// Observed helper/actor join failures during hub shutdown.
/// A clean value is not a deadline expiry; a non-clean value must not be a
/// process success.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ShutdownStatus {
    pub idle_task_failed: bool,
    pub start_task_failures: usize,
    pub actor_failures: usize,
}

impl ShutdownStatus {
    pub fn is_clean(self) -> bool {
        !self.idle_task_failed && self.start_task_failures == 0 && self.actor_failures == 0
    }
}

/// Observed rooms/sockets while shutdown is in progress. `rooms` is `None` if
/// the map lock is busy; never treat a missing count as proof of a clean exit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShutdownProgress {
    pub rooms: Option<usize>,
    pub sockets_held: usize,
}

/// See [`CollabHub::borrow_live_phase`].
enum LiveBorrow {
    Live(RoomHandle),
    Dead(LiveRoom),
    NotLive,
}

enum ReserveOutcome {
    Existing(Arc<RoomSlot>),
    Creator(Arc<RoomSlot>),
}

fn note_abnormal_actor_completion(
    counter: &AtomicUsize,
    document_id: Uuid,
    finished: Result<(), tokio::sync::oneshot::error::RecvError>,
) {
    if finished.is_err() {
        counter.fetch_add(1, Ordering::Relaxed);
        tracing::error!(
            document_id = %document_id,
            "collaboration actor exited without completion"
        );
    }
}

/// Whether the room's resource row exists in the room's workspace (deleted
/// rows included: only a resource that left the workspace is absent).
async fn resource_in_room_workspace(
    pool: &sqlx::PgPool,
    key: RoomKey,
) -> Result<bool, sqlx::Error> {
    let table = match key.2 {
        crate::collab::wire::CollabKind::Document => "documents",
        crate::collab::wire::CollabKind::Task => "tasks",
    };
    let mut tx = pool.begin().await?;
    crate::db::context::set_tenant(&mut tx, key.0).await?;
    let exists: bool = sqlx::query_scalar(&format!(
        "SELECT EXISTS(SELECT 1 FROM fvoci.{table} WHERE workspace_id=$1 AND id=$2)"
    ))
    .bind(key.0)
    .bind(key.1)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(exists)
}

async fn complete_owned_room_cleanup(
    rooms: Arc<RwLock<HashMap<RoomKey, Arc<RoomSlot>>>>,
    key: RoomKey,
    slot: Arc<RoomSlot>,
    live: LiveRoom,
    abnormal_actor_completions: Arc<AtomicUsize>,
) {
    live.handle.shutdown().await;
    finish_owned_room_cleanup(rooms, key, slot, live, abnormal_actor_completions).await;
}

/// After the actor was asked to stop: wait for it, free its slot and permit.
async fn finish_owned_room_cleanup(
    rooms: Arc<RwLock<HashMap<RoomKey, Arc<RoomSlot>>>>,
    key: RoomKey,
    slot: Arc<RoomSlot>,
    live: LiveRoom,
    abnormal_actor_completions: Arc<AtomicUsize>,
) {
    note_abnormal_actor_completion(&abnormal_actor_completions, key.1, live.finished.await);
    drop(live.permit);
    retire_slot(&rooms, key, &slot).await;
}

/// The last step of every room close: mark the slot Failed (terminal, so
/// setting it again is harmless), remove it from the map only if the map still
/// holds this slot, and wake its waiters. Callers free the room permit first.
async fn retire_slot(
    rooms: &RwLock<HashMap<RoomKey, Arc<RoomSlot>>>,
    key: RoomKey,
    slot: &Arc<RoomSlot>,
) {
    *slot.phase.lock().await = RoomPhase::Failed;
    let mut map = rooms.write().await;
    if map
        .get(&key)
        .is_some_and(|existing| Arc::ptr_eq(existing, slot))
    {
        map.remove(&key);
    }
    slot.ready.notify_waiters();
}

async fn idle_eviction_loop(
    rooms: Arc<RwLock<HashMap<RoomKey, Arc<RoomSlot>>>>,
    idle_ms: u64,
    mut stopped: watch::Receiver<bool>,
    abnormal_actor_completions: Arc<AtomicUsize>,
) {
    let tick = Duration::from_millis(idle_ms.max(1_000) / 2);
    loop {
        tokio::select! {
            biased;
            _ = stopped.changed() => return,
            _ = tokio::time::sleep(tick) => {}
        }
        let entries = rooms
            .read()
            .await
            .iter()
            .map(|(key, slot)| (*key, slot.clone()))
            .collect::<Vec<_>>();
        for (key, slot) in entries {
            if *stopped.borrow() {
                return;
            }
            #[cfg(feature = "db-tests")]
            if IDLE_EVICTION_HOLDS
                .lock()
                .expect("idle eviction holds")
                .contains(&key.1)
            {
                continue;
            }
            let live = {
                let mut phase = slot.phase.lock().await;
                let Some(live) = CollabHub::take_idle_evictable(&mut phase, idle_ms) else {
                    continue;
                };
                live
            };
            complete_owned_room_cleanup(
                rooms.clone(),
                key,
                slot,
                live,
                abnormal_actor_completions.clone(),
            )
            .await;
        }
    }
}

fn join_to_body_write_error(err: JoinError) -> BodyWriteError {
    match err {
        JoinError::AdmissionDenied | JoinError::UnsupportedKind => BodyWriteError::Rejected,
        JoinError::RoomFull
        | JoinError::CapacityRetry
        | JoinError::EngineUnavailable
        | JoinError::WriterStale
        | JoinError::DbError => BodyWriteError::Unavailable,
    }
}
