use sqlx::pool::PoolConnection;
use sqlx::postgres::{PgConnection, PgPool};
use sqlx::Postgres;
use uuid::Uuid;

use crate::db::collab::COLLAB_ROOM_SESSION_LOCK_NAMESPACE;
use crate::db::context::lock_key_from_uuid;

/// Detached session-level advisory lock for one document room (the room
/// fence). The lock lives with the dedicated connection: [`Self::release`]
/// unlocks it, and dropping the guard closes the connection, which releases it
/// too. The fence-lost path drops the guard for exactly that reason.
pub struct RoomGuard {
    conn: PgConnection,
    document_id: Uuid,
}

impl RoomGuard {
    /// Try to acquire the room fence without blocking. Returns None if another room holds it.
    pub async fn try_acquire(
        pool: &PgPool,
        document_id: Uuid,
    ) -> Result<Option<Self>, sqlx::Error> {
        let pooled = pool.acquire().await?;
        Self::try_lock_pooled(pooled, document_id).await
    }

    /// Finish the fence on a connection that has already been acquired.
    /// `pool.acquire()` may be cancelled at shutdown; this step must not be.
    pub async fn try_lock_pooled(
        pooled: PoolConnection<Postgres>,
        document_id: Uuid,
    ) -> Result<Option<Self>, sqlx::Error> {
        let mut conn = pooled.detach();
        let acquired: (bool,) = sqlx::query_as("SELECT pg_try_advisory_lock($1, $2)")
            .bind(COLLAB_ROOM_SESSION_LOCK_NAMESPACE)
            .bind(lock_key_from_uuid(document_id))
            .fetch_one(&mut conn)
            .await?;
        if acquired.0 {
            Ok(Some(Self { conn, document_id }))
        } else {
            Ok(None)
        }
    }

    pub fn connection_mut(&mut self) -> &mut PgConnection {
        &mut self.conn
    }

    pub async fn release(mut self) {
        let _ = sqlx::query("SELECT pg_advisory_unlock($1, $2)")
            .bind(COLLAB_ROOM_SESSION_LOCK_NAMESPACE)
            .bind(lock_key_from_uuid(self.document_id))
            .execute(&mut self.conn)
            .await;
    }
}
