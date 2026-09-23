use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use sqlx::postgres::PgPool;
use tokio::sync::{Mutex, Notify, RwLock, Semaphore};
use uuid::Uuid;

use crate::collab::config::CollabConfig;
use crate::collab::guard::RoomGuard;
use crate::collab::room::{JoinError, RoomHandle, RoomJoin, RoomKey};


struct LiveRoom {
    handle: RoomHandle,
    finished: tokio::sync::oneshot::Receiver<()>,
    last_activity: Instant,
    connection_count: usize,
}

enum RoomEntry {
    Starting,
    Live(LiveRoom),
}

pub struct CollabHub {
    config: CollabConfig,
    pool: PgPool,
    rooms: Arc<RwLock<HashMap<RoomKey, Arc<Mutex<RoomEntry>>>>>,
    room_permits: Arc<Semaphore>,
    starting_wait: Arc<Notify>,
}

impl CollabHub {
    pub fn new(config: CollabConfig, pool: PgPool) -> Self {
        let room_cap = config.max_rooms;
        let hub = Self {
            config,
            pool,
            rooms: Arc::new(RwLock::new(HashMap::new())),
            room_permits: Arc::new(Semaphore::new(room_cap)),
            starting_wait: Arc::new(Notify::new()),
        };
        let idle_ms = hub.config.idle_evict_ms;
        let rooms = hub.rooms.clone();
        let pool = hub.pool.clone();
        let config = hub.config.clone();
        let permits = hub.room_permits.clone();
        tokio::spawn(async move {
            idle_eviction_loop(rooms, pool, config, permits, idle_ms).await;
        });
        hub
    }

    pub fn config(&self) -> &CollabConfig {
        &self.config
    }

    pub async fn join_room(&self, key: RoomKey, join: RoomJoin) -> Result<(), JoinError> {
        let room = self.get_or_create_room(key).await?;
        let mut guard = room.lock().await;
        if let RoomEntry::Live(live) = &mut *guard {
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
        if let Some(room) = self.rooms.read().await.get(&key).cloned() {
            let mut guard = room.lock().await;
            if let RoomEntry::Live(live) = &mut *guard {
                live.handle.leave(conn_id).await;
                live.connection_count = live.connection_count.saturating_sub(1);
                live.last_activity = Instant::now();
            }
        }
    }

    pub async fn send_frame(&self, key: RoomKey, conn_id: Uuid, bytes: Vec<u8>) {
        if let Some(room) = self.rooms.read().await.get(&key).cloned() {
            let mut guard = room.lock().await;
            if let RoomEntry::Live(live) = &mut *guard {
                live.last_activity = Instant::now();
                live.handle.frame(conn_id, bytes).await;
            }
        }
    }

    pub async fn shutdown(&self) {
        let keys = self.rooms.read().await.keys().cloned().collect::<Vec<_>>();
        for key in keys {
            self.evict_room(key, true).await;
        }
    }

    async fn get_or_create_room(&self, key: RoomKey) -> Result<Arc<Mutex<RoomEntry>>, JoinError> {
        for _ in 0..120 {
            if let Some(existing) = self.rooms.read().await.get(&key).cloned() {
                let state = existing.lock().await;
                let is_live = matches!(*state, RoomEntry::Live(_));
                let is_starting = matches!(*state, RoomEntry::Starting);
                drop(state);
                if is_live {
                    return Ok(existing);
                }
                if !is_starting {
                    continue;
                }
                let notify = self.starting_wait.clone();
                let wait = tokio::time::timeout(Duration::from_secs(30), async {
                    loop {
                        let state = existing.lock().await;
                        if matches!(*state, RoomEntry::Live(_)) {
                            break;
                        }
                        if !matches!(*state, RoomEntry::Starting) {
                            break;
                        }
                        drop(state);
                        notify.notified().await;
                    }
                });
                let _ = wait.await;
                let state = existing.lock().await;
                let is_live = matches!(*state, RoomEntry::Live(_));
                drop(state);
                if is_live {
                    return Ok(existing);
                }
            }

            let reserved = {
                let mut rooms = self.rooms.write().await;
                if let Some(entry) = rooms.get(&key) {
                    if matches!(*entry.lock().await, RoomEntry::Live(_)) {
                        continue;
                    }
                    if matches!(*entry.lock().await, RoomEntry::Starting) {
                        continue;
                    }
                }
                if !rooms.contains_key(&key) && rooms.len() >= self.config.max_rooms {
                    return Err(JoinError::RoomFull);
                }
                match rooms.entry(key) {
                    std::collections::hash_map::Entry::Occupied(_) => false,
                    std::collections::hash_map::Entry::Vacant(slot) => {
                        slot.insert(Arc::new(Mutex::new(RoomEntry::Starting)));
                        true
                    }
                }
            };
            if !reserved {
                continue;
            }

            let permit = match self.room_permits.clone().acquire_owned().await {
                Ok(p) => p,
                Err(_) => {
                    self.remove_starting(key).await;
                    return Err(JoinError::RoomFull);
                }
            };

            let guard = match RoomGuard::try_acquire(&self.pool, key.1).await {
                Ok(Some(g)) => g,
                Ok(None) => {
                    self.remove_starting(key).await;
                    permit.forget();
                    return Err(JoinError::WriterStale);
                }
                Err(_) => {
                    self.remove_starting(key).await;
                    permit.forget();
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

            match spawn {
                Ok((handle, finished)) => {
                    let live = Arc::new(Mutex::new(RoomEntry::Live(LiveRoom {
                        handle,
                        finished,
                        last_activity: Instant::now(),
                        connection_count: 0,
                    })));
                    self.rooms.write().await.insert(key, live.clone());
                    self.starting_wait.notify_waiters();
                    return Ok(live);
                }
                Err(err) => {
                    self.remove_starting(key).await;
                    permit.forget();
                    return Err(err);
                }
            }
        }
        Err(JoinError::EngineUnavailable)
    }

    async fn remove_starting(&self, key: RoomKey) {
        let mut rooms = self.rooms.write().await;
        if let Some(entry) = rooms.get(&key) {
            if matches!(*entry.lock().await, RoomEntry::Starting) {
                rooms.remove(&key);
            }
        }
        self.starting_wait.notify_waiters();
    }

    async fn evict_room(&self, key: RoomKey, shutdown: bool) {
        let entry = self.rooms.write().await.remove(&key);
        if let Some(room) = entry {
            let mut guard = room.lock().await;
            if let RoomEntry::Live(live) = &mut *guard {
                if shutdown {
                    live.handle.shutdown().await;
                }
                let _ = tokio::time::timeout(Duration::from_secs(30), &mut live.finished).await;
            }
        }
        self.room_permits.add_permits(1);
    }
}

async fn idle_eviction_loop(
    rooms: Arc<RwLock<HashMap<RoomKey, Arc<Mutex<RoomEntry>>>>>,
    pool: PgPool,
    config: CollabConfig,
    permits: Arc<Semaphore>,
    idle_ms: u64,
) {
    let idle = Duration::from_millis(idle_ms);
    loop {
        tokio::time::sleep(Duration::from_millis(idle_ms.max(1_000) / 2)).await;
        let keys = rooms.read().await.keys().cloned().collect::<Vec<_>>();
        for key in keys {
            let should_evict = {
                let map = rooms.read().await;
                let Some(entry) = map.get(&key) else {
                    continue;
                };
                let guard = entry.lock().await;
                match &*guard {
                    RoomEntry::Live(live) => {
                        live.connection_count == 0 && live.last_activity.elapsed() >= idle
                    }
                    RoomEntry::Starting => false,
                }
            };
            if should_evict {
                let entry = rooms.write().await.remove(&key);
                if let Some(room) = entry {
                    let mut guard = room.lock().await;
                    if let RoomEntry::Live(live) = &mut *guard {
                        live.handle.shutdown().await;
                        let _ =
                            tokio::time::timeout(Duration::from_secs(30), &mut live.finished).await;
                    }
                }
                permits.add_permits(1);
            }
        }
        let _ = &pool;
        let _ = &config;
    }
}
