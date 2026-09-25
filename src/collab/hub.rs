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

use crate::collab::admission::memory_budget_exceeded;
use crate::collab::config::CollabConfig;
use crate::collab::guard::RoomGuard;
use crate::collab::room::{
    CapturedRevision, ConnectionLease, JoinDelivery, JoinError, RevisionCaptureError,
    RevisionRestoreError, RoomHandle, RoomJoin, RoomKey,
};
use crate::db::collab::estimate_persisted_collab_bytes;
use crate::db::collab::resolve_collab_admission;

#[cfg(feature = "db-tests")]
pub const HUB_JOIN_BARRIER_BEFORE_ACTOR_JOIN: u8 = 0;
#[cfg(feature = "db-tests")]
pub const HUB_JOIN_BARRIER_AFTER_ACTOR_REPLY: u8 = 1;
#[cfg(feature = "db-tests")]
pub const HUB_JOIN_BARRIER_AFTER_SLOT_READY: u8 = 2;

/// One pre-enqueue retry after a proven undelivered join or a Closing race.
const MAX_PRE_ENQUEUE_RETRIES: u8 = 1;

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
    /// In-flight hub joins that have not yet finished actor admission.
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
    pool: PgPool,
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
    #[cfg(feature = "db-tests")]
    shutdown_drain_witness: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

impl CollabHub {
    pub fn new(config: CollabConfig, pool: PgPool) -> Self {
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
        Self {
            config,
            pool,
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

    pub fn pool(&self) -> &PgPool {
        &self.pool
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

    async fn live_handle(&self, key: RoomKey) -> Option<RoomHandle> {
        let slot = self.room_slot(key).await?;
        let phase = slot.phase.lock().await;
        match &*phase {
            RoomPhase::Live(live) if !live.handle.is_closed() => Some(live.handle.clone()),
            _ => None,
        }
    }

    pub async fn capture_if_live(
        &self,
        key: RoomKey,
        actor_user_id: Uuid,
        session_id: Uuid,
    ) -> Option<Result<CapturedRevision, RevisionCaptureError>> {
        let handle = self.live_handle(key).await?;
        Some(handle.capture_revision(actor_user_id, session_id).await)
    }

    pub async fn ensure_live_room(&self, key: RoomKey) -> Result<RoomHandle, JoinError> {
        let mut retries = 0u8;
        loop {
            if self.shutting_down.load(Ordering::Acquire) {
                return Err(JoinError::EngineUnavailable);
            }
            let slot = self.get_or_create_room(key).await?;
            if let Some(handle) = self.live_handle(key).await {
                return Ok(handle);
            }
            let _ = self.wait_for_live_or_retry(key, slot).await?;
            if !Self::allow_pre_enqueue_retry(&mut retries) {
                return Err(JoinError::EngineUnavailable);
            }
        }
    }

    pub async fn restore_revision(
        &self,
        key: RoomKey,
        actor_user_id: Uuid,
        session_id: Uuid,
        snap: Vec<u8>,
    ) -> Result<(), RevisionRestoreError> {
        let handle = self
            .ensure_live_room(key)
            .await
            .map_err(|_| RevisionRestoreError::Unavailable)?;
        handle
            .restore_from_snapshot(actor_user_id, session_id, snap)
            .await
    }

    #[cfg(feature = "db-tests")]
    pub async fn probe_actor(&self, key: RoomKey) -> crate::collab::room::ActorProbe {
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
    pub async fn idle_evict_decision(&self, key: RoomKey) -> IdleEvictDecision {
        let Some(slot) = self.room_slot(key).await else {
            return IdleEvictDecision::NotApplicable;
        };
        let phase = slot.phase.lock().await;
        match &*phase {
            RoomPhase::Live(live) => Self::idle_evict_decision_for(live, self.config.idle_evict_ms),
            _ => IdleEvictDecision::NotApplicable,
        }
    }

    #[cfg(feature = "db-tests")]
    pub async fn force_room_idle_eligible(&self, key: RoomKey) {
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
    pub async fn execute_idle_evict_if_eligible(&self, key: RoomKey) -> bool {
        let Some(slot) = self.room_slot(key).await else {
            return false;
        };
        let live = {
            let mut phase = slot.phase.lock().await;
            let RoomPhase::Live(live) = &*phase else {
                return false;
            };
            if Self::idle_evict_decision_for(live, self.config.idle_evict_ms)
                != IdleEvictDecision::WouldEvict
            {
                return false;
            }
            let RoomPhase::Live(live) = std::mem::replace(&mut *phase, RoomPhase::Closing) else {
                unreachable!()
            };
            live
        };
        complete_idle_eviction(
            self.rooms.clone(),
            key,
            slot,
            live,
            self.abnormal_actor_completions.clone(),
        )
        .await;
        true
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
    pub async fn room_joining_count(&self, key: RoomKey) -> usize {
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
    pub async fn room_member_count(&self, key: RoomKey) -> usize {
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
    pub async fn room_waiter_count(&self, key: RoomKey) -> usize {
        self.room_slot(key)
            .await
            .map_or(0, |slot| slot.waiters.load(Ordering::Acquire))
    }

    #[cfg(feature = "db-tests")]
    pub async fn room_occupies_slot(&self, key: RoomKey) -> bool {
        matches!(
            self.room_lifecycle_phase(key).await,
            RoomLifecyclePhase::Starting
                | RoomLifecyclePhase::Booting
                | RoomLifecyclePhase::Live
                | RoomLifecyclePhase::Closing
        )
    }

    #[cfg(feature = "db-tests")]
    pub async fn room_lifecycle_phase(&self, key: RoomKey) -> RoomLifecyclePhase {
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
        key: RoomKey,
        mut join: RoomJoin,
    ) -> Result<ConnectionLease, JoinError> {
        if self.shutting_down.load(Ordering::Acquire) {
            return Err(JoinError::EngineUnavailable);
        }
        let admission = resolve_collab_admission(
            &self.pool,
            key.0,
            join.conn.session.user_id,
            join.conn.session.session_id,
            key.1,
        )
        .await
        .map_err(|_| JoinError::DbError)?;
        admission.map_err(|_| JoinError::AdmissionDenied)?;

        let mut retries = 0u8;
        loop {
            if self.shutting_down.load(Ordering::Acquire) {
                return Err(JoinError::EngineUnavailable);
            }
            let slot = self.get_or_create_room(key).await?;
            #[cfg(feature = "db-tests")]
            pause_for_hub_join_barrier(key.1, HUB_JOIN_BARRIER_AFTER_SLOT_READY).await;

            let deliver = {
                let mut phase = slot.phase.lock().await;
                let closed_live =
                    matches!(&*phase, RoomPhase::Live(live) if live.handle.is_closed());
                if closed_live {
                    Self::take_dead_live(&mut phase).map(Err)
                } else if let RoomPhase::Live(live) = &mut *phase {
                    live.last_activity = Instant::now();
                    Some(Ok((
                        live.handle.clone(),
                        JoiningLease::register(&live.joining),
                    )))
                } else {
                    None
                }
            };

            match deliver {
                Some(Ok((handle, joining_lease))) => {
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
                Some(Err(live)) => {
                    self.spawn_reclaim(key, slot, live);
                    if !Self::allow_pre_enqueue_retry(&mut retries) {
                        return Err(JoinError::EngineUnavailable);
                    }
                }
                None => {
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

    pub async fn leave_room(&self, key: RoomKey, conn_id: Uuid) {
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

    pub async fn send_frame(&self, key: RoomKey, conn_id: Uuid, bytes: Vec<u8>) {
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
        let abnormal_actor_completions = self.abnormal_actor_completions.clone();
        let rooms_join = async {
            join_all(live_rooms.into_iter().map(|(key, live)| {
                let hub = self.clone();
                let abnormal_actor_completions = abnormal_actor_completions.clone();
                async move {
                    live.handle.shutdown().await;
                    note_abnormal_actor_completion(
                        &abnormal_actor_completions,
                        key.1,
                        live.finished.await,
                    );
                    drop(live.permit);
                    // Drop the slot as soon as this actor finished so a sibling
                    // blocked on persist cannot keep this room visible as Closing.
                    hub.forget_closed_room(key).await;
                }
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

    async fn get_or_create_room(&self, key: RoomKey) -> Result<Arc<RoomSlot>, JoinError> {
        loop {
            if self.shutting_down.load(Ordering::Acquire) {
                return Err(JoinError::EngineUnavailable);
            }

            if let Some(slot) = self.room_slot(key).await {
                if let Some(live_slot) = self.wait_for_live_or_retry(key, slot).await? {
                    return Ok(live_slot);
                }
            }

            match self.try_reserve_starting(key).await? {
                ReserveOutcome::Existing(slot) => {
                    if let Some(live_slot) = self.wait_for_live_or_retry(key, slot).await? {
                        return Ok(live_slot);
                    }
                }
                ReserveOutcome::Creator(slot) => {
                    // The hub, not a cancellable HTTP/socket caller, owns startup.
                    // An abandoned caller leaves a normal zero-client room which
                    // idle eviction reclaims; another caller can join it meanwhile.
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
                                    hub.start_room(key, startup_slot.clone()),
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

    async fn wait_for_live_or_retry(
        &self,
        key: RoomKey,
        slot: Arc<RoomSlot>,
    ) -> Result<Option<Arc<RoomSlot>>, JoinError> {
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
                    RoomPhase::Live(_) => return Ok(Some(slot.clone())),
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
        if rooms.len() >= self.config.max_rooms {
            return Err(JoinError::RoomFull);
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
    ) -> Result<Arc<RoomSlot>, JoinError> {
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
            self.fail_starting(key, &slot).await;
            return Err(JoinError::EngineUnavailable);
        }

        let mut pooled = tokio::select! {
            biased;
            result = self.pool.acquire() => match result {
                Ok(pooled) => pooled,
                Err(_) => {
                    self.fail_starting(key, &slot).await;
                    return Err(JoinError::DbError);
                }
            },
            () = self.wait_if_shutting_down() => {
                self.fail_starting(key, &slot).await;
                return Err(JoinError::EngineUnavailable);
            }
        };
        if self.shutting_down.load(Ordering::Acquire) {
            drop(pooled);
            self.fail_starting(key, &slot).await;
            return Err(JoinError::EngineUnavailable);
        }
        let persisted_bytes = match estimate_persisted_collab_bytes(&mut pooled, key.0, key.1).await
        {
            Ok(bytes) => bytes,
            Err(_) => {
                drop(pooled);
                self.fail_starting(key, &slot).await;
                return Err(JoinError::DbError);
            }
        };
        if memory_budget_exceeded(self.config.memory_budget_bytes, persisted_bytes) {
            drop(pooled);
            self.fail_starting(key, &slot).await;
            return Err(JoinError::CapacityRetry);
        }
        let guard = match RoomGuard::try_lock_pooled(pooled, key.1).await {
            Ok(Some(guard)) => guard,
            Ok(None) => {
                self.fail_starting(key, &slot).await;
                return Err(JoinError::WriterStale);
            }
            Err(_) => {
                self.fail_starting(key, &slot).await;
                return Err(JoinError::DbError);
            }
        };
        if self.shutting_down.load(Ordering::Acquire) {
            guard.release().await;
            self.fail_starting(key, &slot).await;
            return Err(JoinError::EngineUnavailable);
        }

        let (workspace_id, document_id) = key;
        let live_conns = Arc::new(AtomicUsize::new(0));
        let spawn = crate::collab::room::spawn_room(
            workspace_id,
            document_id,
            self.config.clone(),
            self.pool.clone(),
            guard,
            live_conns.clone(),
        )
        .await;

        if self.shutting_down.load(Ordering::Acquire) {
            if let Ok((handle, finished)) = spawn {
                handle.shutdown().await;
                let _ = finished.await;
            }
            self.fail_starting(key, &slot).await;
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
                *phase = RoomPhase::Live(LiveRoom {
                    handle,
                    finished,
                    last_activity: Instant::now(),
                    live_conns,
                    joining: Arc::new(AtomicUsize::new(0)),
                    permit,
                });
                slot.ready.notify_waiters();
                Ok(slot.clone())
            }
            Err(err) => {
                self.fail_starting(key, &slot).await;
                Err(err)
            }
        }
    }

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
        let mut rooms = self.rooms.write().await;
        if rooms
            .get(&key)
            .is_some_and(|existing| Arc::ptr_eq(existing, slot))
        {
            rooms.remove(&key);
        }
        slot.ready.notify_waiters();
    }

    async fn fail_starting(&self, key: RoomKey, slot: &Arc<RoomSlot>) {
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
        let mut rooms = self.rooms.write().await;
        if rooms
            .get(&key)
            .is_some_and(|existing| Arc::ptr_eq(existing, slot))
        {
            rooms.remove(&key);
        }
        slot.ready.notify_waiters();
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
        if let Some(live) = live {
            live.handle.shutdown().await;
            note_abnormal_actor_completion(
                &self.abnormal_actor_completions,
                key.1,
                live.finished.await,
            );
            drop(live.permit);
        }
        *slot.phase.lock().await = RoomPhase::Failed;
        let mut rooms = self.rooms.write().await;
        if rooms
            .get(&key)
            .is_some_and(|existing| Arc::ptr_eq(existing, &slot))
        {
            rooms.remove(&key);
        }
        slot.ready.notify_waiters();
    }

    async fn forget_closed_room(&self, key: RoomKey) {
        let Some(slot) = self.rooms.read().await.get(&key).cloned() else {
            return;
        };
        *slot.phase.lock().await = RoomPhase::Failed;
        let mut rooms = self.rooms.write().await;
        if rooms
            .get(&key)
            .is_some_and(|existing| Arc::ptr_eq(existing, &slot))
        {
            rooms.remove(&key);
        }
        slot.ready.notify_waiters();
    }

    async fn take_live_rooms_for_shutdown(&self) -> Vec<(RoomKey, LiveRoom)> {
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
                    live.push((key, room));
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

async fn complete_idle_eviction(
    rooms: Arc<RwLock<HashMap<RoomKey, Arc<RoomSlot>>>>,
    key: RoomKey,
    slot: Arc<RoomSlot>,
    live: LiveRoom,
    abnormal_actor_completions: Arc<AtomicUsize>,
) {
    complete_owned_room_cleanup(rooms, key, slot, live, abnormal_actor_completions).await;
}

async fn complete_owned_room_cleanup(
    rooms: Arc<RwLock<HashMap<RoomKey, Arc<RoomSlot>>>>,
    key: RoomKey,
    slot: Arc<RoomSlot>,
    live: LiveRoom,
    abnormal_actor_completions: Arc<AtomicUsize>,
) {
    live.handle.shutdown().await;
    note_abnormal_actor_completion(&abnormal_actor_completions, key.1, live.finished.await);
    drop(live.permit);
    *slot.phase.lock().await = RoomPhase::Failed;
    let mut map = rooms.write().await;
    if map
        .get(&key)
        .is_some_and(|existing| Arc::ptr_eq(existing, &slot))
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
                let RoomPhase::Live(live) = &*phase else {
                    continue;
                };
                if CollabHub::idle_evict_decision_for(live, idle_ms)
                    != IdleEvictDecision::WouldEvict
                {
                    continue;
                }
                // Publish Closing atomically with the idle observation. Never
                // expose Failed until this exact actor and guard have finished.
                let RoomPhase::Live(live) = std::mem::replace(&mut *phase, RoomPhase::Closing)
                else {
                    unreachable!()
                };
                live
            };
            complete_idle_eviction(
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
