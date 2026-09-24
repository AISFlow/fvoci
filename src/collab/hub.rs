use std::collections::HashMap;
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

use crate::collab::config::CollabConfig;
use crate::collab::guard::RoomGuard;
use crate::collab::room::{ConnectionLease, JoinError, RoomHandle, RoomJoin, RoomKey};
use crate::db::collab::resolve_collab_admission;

#[cfg(feature = "db-tests")]
pub const HUB_JOIN_BARRIER_BEFORE_ACTOR_JOIN: u8 = 0;
#[cfg(feature = "db-tests")]
pub const HUB_JOIN_BARRIER_AFTER_ACTOR_REPLY: u8 = 1;

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
}

impl CollabHub {
    pub fn new(config: CollabConfig, pool: PgPool) -> Self {
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
        let (idle_stop, stopped) = watch::channel(false);
        let idle_task = tokio::spawn(async move {
            idle_eviction_loop(idle_rooms, idle_ms, stopped).await;
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
        complete_idle_eviction(self.rooms.clone(), key, slot, live).await;
        true
    }

    fn idle_evict_decision_for(live: &LiveRoom, idle_ms: u64) -> IdleEvictDecision {
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
        join: RoomJoin,
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

        let slot = self.get_or_create_room(key).await?;
        let (handle, joining_lease) = {
            let mut phase = slot.phase.lock().await;
            match &mut *phase {
                RoomPhase::Live(live) => {
                    live.last_activity = Instant::now();
                    (live.handle.clone(), JoiningLease::register(&live.joining))
                }
                _ => return Err(JoinError::EngineUnavailable),
            }
        };
        #[cfg(feature = "db-tests")]
        pause_for_hub_join_barrier(key.1, HUB_JOIN_BARRIER_BEFORE_ACTOR_JOIN).await;
        let lease = handle.join(join).await;
        drop(joining_lease);
        #[cfg(feature = "db-tests")]
        pause_for_hub_join_barrier(key.1, HUB_JOIN_BARRIER_AFTER_ACTOR_REPLY).await;
        lease
    }

    pub async fn leave_room(&self, key: RoomKey, conn_id: Uuid) {
        let slot = self.room_slot(key).await;
        if let Some(slot) = slot {
            let handle = {
                let mut phase = slot.phase.lock().await;
                if let RoomPhase::Live(live) = &mut *phase {
                    live.last_activity = Instant::now();
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
                    live.last_activity = Instant::now();
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
            let mut failures = 0usize;
            for failed in join_all(live_rooms.into_iter().map(|(key, live)| {
                let hub = self.clone();
                async move {
                    live.handle.shutdown().await;
                    let failed = live.finished.await.is_err();
                    if failed {
                        tracing::error!(
                            document_id = %key.1,
                            "collaboration actor exited without completion"
                        );
                    }
                    drop(live.permit);
                    // Drop the slot as soon as this actor finished so a sibling
                    // blocked on persist cannot keep this room visible as Closing.
                    hub.forget_closed_room(key).await;
                    failed
                }
            }))
            .await
            {
                if failed {
                    failures += 1;
                }
            }
            failures
        };
        let (idle_task_failed, start_task_failures, mut actor_failures) =
            tokio::join!(idle_join, starts_join, rooms_join);

        let leftover = self
            .rooms
            .read()
            .await
            .iter()
            .map(|(key, slot)| (*key, slot.clone()))
            .collect::<Vec<_>>();
        for (key, slot) in leftover {
            if self.force_close_slot(key, slot).await {
                actor_failures += 1;
            }
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

    async fn start_room(
        &self,
        key: RoomKey,
        slot: Arc<RoomSlot>,
    ) -> Result<Arc<RoomSlot>, JoinError> {
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

        let pooled = tokio::select! {
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

    async fn force_close_slot(&self, key: RoomKey, slot: Arc<RoomSlot>) -> bool {
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
        let mut actor_failed = false;
        if let Some(live) = live {
            live.handle.shutdown().await;
            if live.finished.await.is_err() {
                tracing::error!(document_id = %key.1, "collaboration actor exited without completion");
                actor_failed = true;
            }
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
        actor_failed
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

async fn complete_idle_eviction(
    rooms: Arc<RwLock<HashMap<RoomKey, Arc<RoomSlot>>>>,
    key: RoomKey,
    slot: Arc<RoomSlot>,
    live: LiveRoom,
) {
    live.handle.shutdown().await;
    if live.finished.await.is_err() {
        tracing::error!(document_id = %key.1, "idle eviction actor failed");
    }
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
            complete_idle_eviction(rooms.clone(), key, slot, live).await;
        }
    }
}
