use sqlx::{Connection, PgConnection, PgPool};

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

/// Session-level claim on a connection detached from the pool. The lock lives
/// exactly as long as that session: release (or drop, or a cancelled claim
/// attempt whose reply was lost) closes the connection, so a lock-holding
/// connection is never returned to the pool. Costs one connection outside the
/// app pool while a job runs (covered by the connection reserve).
pub struct JobClaim {
    conn: Option<PgConnection>,
    key: i32,
}

impl JobClaim {
    pub async fn try_claim(pool: &PgPool, key: i32) -> Result<Option<Self>, sqlx::Error> {
        let mut conn = pool.acquire().await?.detach();
        let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1, $2)")
            .bind(JOB_LOCK_NAMESPACE)
            .bind(key)
            .fetch_one(&mut conn)
            .await?;
        if !locked {
            let _ = conn.close().await;
            return Ok(None);
        }
        Ok(Some(Self {
            conn: Some(conn),
            key,
        }))
    }

    /// Ends the session, which releases the lock whether or not an explicit
    /// unlock would have succeeded.
    pub async fn release(mut self) {
        if let Some(conn) = self.conn.take() {
            let _ = conn.close().await;
        }
    }
}

impl Drop for JobClaim {
    fn drop(&mut self) {
        if self.conn.is_some() {
            // Dropping the detached connection closes the session; the lock dies with it.
            tracing::warn!(
                job_key = self.key,
                "maintenance claim dropped without release; closing its session"
            );
        }
    }
}
