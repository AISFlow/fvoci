use sqlx::pool::PoolConnection;
use sqlx::{PgPool, Postgres};

/// Session advisory locks for maintenance jobs. Distinct from membership (1907006)
/// and collab room (1907007) namespaces.
pub const JOB_LOCK_NAMESPACE: i32 = 1_907_020;

pub const JOB_KEY_DAILY: i32 = 1;
pub const JOB_KEY_WORKSPACE: i32 = 2;
pub const JOB_KEY_ICS: i32 = 3;
pub const JOB_KEY_MAGIC: i32 = 4;
pub const JOB_KEY_NOTIFICATIONS: i32 = 5;
pub const JOB_KEY_PROCESSED: i32 = 6;
pub const JOB_KEY_DIGEST: i32 = 7;

/// Session-level claim held on one pool connection for the job's duration.
/// Another process calling `try_claim` with the same key gets `None`.
pub struct JobClaim {
    conn: Option<PoolConnection<Postgres>>,
    ns: i32,
    key: i32,
}

impl JobClaim {
    pub async fn try_claim(pool: &PgPool, key: i32) -> Result<Option<Self>, sqlx::Error> {
        let mut conn = pool.acquire().await?;
        let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1, $2)")
            .bind(JOB_LOCK_NAMESPACE)
            .bind(key)
            .fetch_one(&mut *conn)
            .await?;
        if !locked {
            return Ok(None);
        }
        Ok(Some(Self {
            conn: Some(conn),
            ns: JOB_LOCK_NAMESPACE,
            key,
        }))
    }

    pub async fn release(mut self) {
        self.unlock().await;
    }

    async fn unlock(&mut self) {
        let Some(conn) = self.conn.as_mut() else {
            return;
        };
        let _ = sqlx::query("SELECT pg_advisory_unlock($1, $2)")
            .bind(self.ns)
            .bind(self.key)
            .execute(&mut **conn)
            .await;
        self.conn = None;
    }
}

impl Drop for JobClaim {
    fn drop(&mut self) {
        if self.conn.is_none() {
            return;
        }
        // Connection drop ends the PG session and releases the advisory lock.
        // Returning the connection to the pool without unlock would leak the lock
        // onto the next borrower; dropping the handle closes it instead.
        tracing::warn!(
            job_key = self.key,
            "maintenance claim dropped without release; detaching the connection so the lock dies with the session"
        );
        if let Some(conn) = self.conn.take() {
            drop(conn.detach());
        }
    }
}
