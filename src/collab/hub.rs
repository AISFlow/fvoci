use std::collections::HashMap;
use std::sync::Arc;

use sqlx::postgres::PgPool;
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;

use crate::collab::config::CollabConfig;
use crate::collab::room::{JoinError, RoomHandle, RoomJoin, RoomKey};

struct LiveRoom {
    handle: RoomHandle,
}

pub struct CollabHub {
    config: CollabConfig,
    pool: PgPool,
    rooms: RwLock<HashMap<RoomKey, Arc<Mutex<LiveRoom>>>>,
}

impl CollabHub {
    pub fn new(config: CollabConfig, pool: PgPool) -> Self {
        Self {
            config,
            pool,
            rooms: RwLock::new(HashMap::new()),
        }
    }

    pub fn config(&self) -> &CollabConfig {
        &self.config
    }

    pub async fn join_room(&self, key: RoomKey, join: RoomJoin) -> Result<(), JoinError> {
        let room = self.get_or_create_room(key).await?;
        let guard = room.lock().await;
        guard.handle.join(join).await
    }

    pub async fn leave_room(&self, key: RoomKey, conn_id: Uuid) {
        if let Some(room) = self.rooms.read().await.get(&key).cloned() {
            let guard = room.lock().await;
            guard.handle.leave(conn_id).await;
        }
    }

    pub async fn send_frame(&self, key: RoomKey, conn_id: Uuid, bytes: Vec<u8>) {
        if let Some(room) = self.rooms.read().await.get(&key).cloned() {
            let guard = room.lock().await;
            guard.handle.frame(conn_id, bytes).await;
        }
    }

    pub async fn shutdown(&self) {
        let rooms = self.rooms.read().await.clone();
        for (_, room) in rooms {
            let guard = room.lock().await;
            guard.handle.shutdown().await;
        }
    }

    async fn get_or_create_room(&self, key: RoomKey) -> Result<Arc<Mutex<LiveRoom>>, JoinError> {
        if let Some(existing) = self.rooms.read().await.get(&key).cloned() {
            return Ok(existing);
        }
        if self.rooms.read().await.len() >= self.config.max_rooms {
            return Err(JoinError::RoomFull);
        }
        let (workspace_id, document_id) = key;
        let (handle, _finished) = crate::collab::room::spawn_room(
            workspace_id,
            document_id,
            self.config.clone(),
            self.pool.clone(),
        )
        .await?;
        let live = Arc::new(Mutex::new(LiveRoom { handle }));
        self.rooms.write().await.insert(key, live.clone());
        Ok(live)
    }
}
