use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::FutureExt;
use sqlx::postgres::PgPool;
use tokio::sync::{watch, Mutex, Notify, OwnedSemaphorePermit, RwLock, Semaphore};
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::collab::config::CollabConfig;
use crate::collab::guard::RoomGuard;
use crate::collab::room::{JoinError, RoomHandle, RoomJoin, RoomKey};
use crate::db::collab::resolve_collab_admission;

struct LiveRoom {
    handle: RoomHandle,
    finished: tokio::sync::oneshot::Receiver<()>,
    last_activity: Instant,
    members: HashSet<Uuid>,
    permit: OwnedSemaphorePermit,
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
    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::Acquire)
    }

    #[cfg(feature = "db-tests")]
    pub async fn room_member_count(&self, key: RoomKey) -> usize {
        let Some(slot) = self.room_slot(key).await else {
            return 0;
        };
        let phase = slot.phase.lock().await;
        match &*phase {
            RoomPhase::Live(live) => live.members.len(),
            _ => 0,
        }
    }

    #[cfg(feature = "db-tests")]
    pub fn available_room_slots(&self) -> usize {
        self.room_permits.available_permits()
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

    pub async fn join_room(&self, key: RoomKey, join: RoomJoin) -> Result<(), JoinError> {
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
        let mut phase = slot.phase.lock().await;
        if let RoomPhase::Live(live) = &mut *phase {
            live.last_activity = Instant::now();
            let conn_id = join.conn.conn_id;
            let result = live.handle.join(join).await;
            if result.is_ok() {
                live.members.insert(conn_id);
            }
            return result;
        }
        Err(JoinError::EngineUnavailable)
    }

    pub async fn leave_room(&self, key: RoomKey, conn_id: Uuid) {
        let slot = self.room_slot(key).await;
        if let Some(slot) = slot {
            let mut phase = slot.phase.lock().await;
            if let RoomPhase::Live(live) = &mut *phase {
                if live.members.remove(&conn_id) {
                    live.handle.leave(conn_id).await;
                    live.last_activity = Instant::now();
                }
            }
        }
    }

    pub async fn send_frame(&self, key: RoomKey, conn_id: Uuid, bytes: Vec<u8>) {
        if self.shutting_down.load(Ordering::Acquire) {
            return;
        }
        let slot = self.room_slot(key).await;
        if let Some(slot) = slot {
            let mut phase = slot.phase.lock().await;
            if let RoomPhase::Live(live) = &mut *phase {
                if live.members.contains(&conn_id) {
                    live.last_activity = Instant::now();
                    live.handle.frame(conn_id, bytes).await;
                }
            }
        }
    }

    pub async fn shutdown(&self) {
        let _shutdown = self.shutdown_lock.lock().await;
        self.shutting_down.store(true, Ordering::Release);
        let _ = self.idle_stop.send(true);
        // Never abort an eviction while it owns an actor, completion receiver,
        // and permit. Finish that teardown before closing the remaining rooms.
        if let Some(task) = self.idle_task.lock().await.take() {
            if let Err(error) = task.await {
                tracing::error!(%error, "collaboration eviction task failed");
            }
        }
        let starts = std::mem::take(&mut *self.starts.lock().expect("room start task list"));
        for task in starts {
            if let Err(error) = task.await {
                tracing::error!(%error, "collaboration startup task failed");
            }
        }

        let entries = self
            .rooms
            .read()
            .await
            .iter()
            .map(|(key, slot)| (*key, slot.clone()))
            .collect::<Vec<_>>();
        for (key, slot) in entries {
            self.force_close_slot(key, slot).await;
        }
        self.rooms.write().await.clear();
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

        let permit = match self.room_permits.clone().acquire_owned().await {
            Ok(permit) => permit,
            Err(_) => {
                self.cleanup_starting(key, &slot).await;
                return Err(JoinError::RoomFull);
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

        let guard = match RoomGuard::try_acquire(&self.pool, key.1).await {
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

        let (workspace_id, document_id) = key;
        let spawn = crate::collab::room::spawn_room(
            workspace_id,
            document_id,
            self.config.clone(),
            self.pool.clone(),
            guard,
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
                    members: HashSet::new(),
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
            if live.finished.await.is_err() {
                tracing::error!(document_id = %key.1, "collaboration actor exited without completion");
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
    }
}

enum ReserveOutcome {
    Existing(Arc<RoomSlot>),
    Creator(Arc<RoomSlot>),
}

async fn idle_eviction_loop(
    rooms: Arc<RwLock<HashMap<RoomKey, Arc<RoomSlot>>>>,
    idle_ms: u64,
    mut stopped: watch::Receiver<bool>,
) {
    let idle = Duration::from_millis(idle_ms);
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
                if !live.members.is_empty() || live.last_activity.elapsed() < idle {
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
            live.handle.shutdown().await;
            if live.finished.await.is_err() {
                tracing::error!(document_id = %key.1, "evicted collaboration actor failed");
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
    }
}
