use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use sqlx::postgres::PgPool;
use tokio::sync::{Mutex, Notify, OwnedSemaphorePermit, RwLock, Semaphore};
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
    connection_count: usize,
    permit: OwnedSemaphorePermit,
}

struct ClosingRoom {
    finished: tokio::sync::oneshot::Receiver<()>,
    permit: OwnedSemaphorePermit,
}

enum RoomPhase {
    Starting,
    Booting(OwnedSemaphorePermit),
    Live(LiveRoom),
    Closing(ClosingRoom),
    Failed,
}

struct RoomSlot {
    phase: Mutex<RoomPhase>,
    ready: Notify,
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

pub struct CollabHub {
    config: CollabConfig,
    pool: PgPool,
    rooms: Arc<RwLock<HashMap<RoomKey, Arc<RoomSlot>>>>,
    room_permits: Arc<Semaphore>,
    shutting_down: AtomicBool,
    idle_task: Mutex<Option<JoinHandle<()>>>,
}

impl CollabHub {
    pub fn new(config: CollabConfig, pool: PgPool) -> Self {
        let room_cap = config.max_rooms;
        let rooms = Arc::new(RwLock::new(HashMap::new()));
        let room_permits = Arc::new(Semaphore::new(room_cap));
        let idle_ms = config.idle_evict_ms;
        let idle_rooms = rooms.clone();
        let idle_task = tokio::spawn(async move {
            idle_eviction_loop(idle_rooms, idle_ms).await;
        });
        Self {
            config,
            pool,
            rooms,
            room_permits,
            shutting_down: AtomicBool::new(false),
            idle_task: Mutex::new(Some(idle_task)),
        }
    }

    pub fn config(&self) -> &CollabConfig {
        &self.config
    }

    #[cfg(feature = "db-tests")]
    pub fn available_room_slots(&self) -> usize {
        self.room_permits.available_permits()
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
            RoomPhase::Closing(_) => RoomLifecyclePhase::Closing,
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
            let result = live.handle.join(join).await;
            if result.is_ok() {
                live.connection_count += 1;
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
                live.handle.leave(conn_id).await;
                live.connection_count = live.connection_count.saturating_sub(1);
                live.last_activity = Instant::now();
            }
        }
    }

    pub async fn send_frame(&self, key: RoomKey, conn_id: Uuid, bytes: Vec<u8>) {
        let slot = self.room_slot(key).await;
        if let Some(slot) = slot {
            let mut phase = slot.phase.lock().await;
            if let RoomPhase::Live(live) = &mut *phase {
                live.last_activity = Instant::now();
                live.handle.frame(conn_id, bytes).await;
            }
        }
    }

    pub async fn shutdown(&self) {
        self.shutting_down.store(true, Ordering::Release);
        if let Some(task) = self.idle_task.lock().await.take() {
            task.abort();
            let _ = task.await;
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
                ReserveOutcome::Creator(slot) => return self.start_room(key, slot).await,
            }
        }
    }

    async fn wait_for_live_or_retry(
        &self,
        key: RoomKey,
        slot: Arc<RoomSlot>,
    ) -> Result<Option<Arc<RoomSlot>>, JoinError> {
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
                    RoomPhase::Starting | RoomPhase::Booting(_) | RoomPhase::Closing(_) => {}
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
            let mut phase = existing.phase.lock().await;
            match *phase {
                RoomPhase::Live(_)
                | RoomPhase::Starting
                | RoomPhase::Booting(_)
                | RoomPhase::Closing(_) => {
                    return Ok(ReserveOutcome::Existing(existing.clone()));
                }
                RoomPhase::Failed => {
                    *phase = RoomPhase::Starting;
                    existing.ready.notify_waiters();
                    return Ok(ReserveOutcome::Creator(existing.clone()));
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
                    connection_count: 0,
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
            match std::mem::replace(&mut *phase, RoomPhase::Failed) {
                RoomPhase::Live(live) => Some(live),
                RoomPhase::Booting(permit) => {
                    drop(permit);
                    None
                }
                RoomPhase::Closing(closing) => {
                    *phase = RoomPhase::Closing(closing);
                    None
                }
                RoomPhase::Starting | RoomPhase::Failed => None,
            }
        };

        if let Some(live) = live {
            live.handle.shutdown().await;
            let _ = live.finished.await;
            drop(live.permit);
        } else {
            self.await_closing_finished(&slot).await;
        }

        let mut rooms = self.rooms.write().await;
        if rooms
            .get(&key)
            .is_some_and(|existing| Arc::ptr_eq(existing, &slot))
        {
            rooms.remove(&key);
        }
        slot.ready.notify_waiters();
    }

    async fn await_closing_finished(&self, slot: &Arc<RoomSlot>) {
        let closing = {
            let mut phase = slot.phase.lock().await;
            match *phase {
                RoomPhase::Closing(_) => {
                    let RoomPhase::Closing(closing) =
                        std::mem::replace(&mut *phase, RoomPhase::Failed)
                    else {
                        return;
                    };
                    closing
                }
                _ => return,
            }
        };
        let _ = closing.finished.await;
        drop(closing.permit);
    }
}

enum ReserveOutcome {
    Existing(Arc<RoomSlot>),
    Creator(Arc<RoomSlot>),
}

async fn idle_eviction_loop(rooms: Arc<RwLock<HashMap<RoomKey, Arc<RoomSlot>>>>, idle_ms: u64) {
    let idle = Duration::from_millis(idle_ms);
    let tick = Duration::from_millis(idle_ms.max(1_000) / 2);
    loop {
        tokio::time::sleep(tick).await;
        let keys = rooms.read().await.keys().cloned().collect::<Vec<_>>();
        for key in keys {
            let slot = rooms.read().await.get(&key).cloned();
            let Some(slot) = slot else {
                continue;
            };

            let should_close = {
                let phase = slot.phase.lock().await;
                match &*phase {
                    RoomPhase::Live(live) => {
                        live.connection_count == 0 && live.last_activity.elapsed() >= idle
                    }
                    _ => false,
                }
            };
            if !should_close {
                continue;
            }

            let live = {
                let mut phase = slot.phase.lock().await;
                let RoomPhase::Live(live) = &*phase else {
                    continue;
                };
                if live.connection_count != 0 || live.last_activity.elapsed() < idle {
                    continue;
                }
                let RoomPhase::Live(live) = std::mem::replace(&mut *phase, RoomPhase::Failed)
                else {
                    continue;
                };
                live
            };

            live.handle.shutdown().await;
            {
                let mut phase = slot.phase.lock().await;
                *phase = RoomPhase::Closing(ClosingRoom {
                    finished: live.finished,
                    permit: live.permit,
                });
            }

            let closing = {
                let mut phase = slot.phase.lock().await;
                let RoomPhase::Closing(closing) = std::mem::replace(&mut *phase, RoomPhase::Failed)
                else {
                    continue;
                };
                closing
            };
            let _ = closing.finished.await;
            drop(closing.permit);

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
