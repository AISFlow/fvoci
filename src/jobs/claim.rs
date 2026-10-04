use sqlx::{Connection, PgConnection, PgPool};

/// Session advisory locks for maintenance jobs.
pub const JOB_LOCK_NAMESPACE: i32 = 1_907_020;

pub const JOB_KEY_DAILY: i32 = 1;
pub const JOB_KEY_WORKSPACE: i32 = 2;
pub const JOB_KEY_ICS: i32 = 3;
pub const JOB_KEY_MAGIC: i32 = 4;
pub const JOB_KEY_NOTIFICATIONS: i32 = 5;
pub const JOB_KEY_PROCESSED: i32 = 6;
pub const JOB_KEY_DIGEST: i32 = 7;
pub const JOB_KEY_UPLOADS: i32 = 8;
pub const JOB_KEY_REVISIONS: i32 = 9;

/// Session-level claim on a connection detached from the pool. The lock lives
/// at most as long as that session: release unlocks and closes it, and drop (or
/// a cancelled claim attempt whose reply was lost) closes the connection, so a
/// lock-holding connection is never returned to the pool. Costs one connection outside the
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

    /// Unlocks, then ends the session. `close()` only sends Terminate; the
    /// backend drops session locks when it exits, after `close()` returns, so a
    /// caller that reclaims right away could still see the key held. The
    /// acknowledged unlock makes the key free before this returns. If the unlock
    /// fails (or this future is dropped mid-way) the connection is still closed
    /// or dropped, never pooled, and the lock dies with the session.
    pub async fn release(mut self) {
        if let Some(mut conn) = self.conn.take() {
            let unlocked = sqlx::query_scalar::<_, bool>("SELECT pg_advisory_unlock($1, $2)")
                .bind(JOB_LOCK_NAMESPACE)
                .bind(self.key)
                .fetch_one(&mut conn)
                .await;
            match unlocked {
                Ok(true) => {}
                Ok(false) => tracing::warn!(
                    job_key = self.key,
                    "maintenance claim was not held at release; closing its session"
                ),
                Err(err) => tracing::warn!(
                    job_key = self.key,
                    error = %err,
                    "maintenance claim unlock failed; closing its session"
                ),
            }
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


pub use crate::db::maintenance_claim::{
    FamilyLeaseAction, FamilyMaintenanceClaim, FamilyMaintenanceClaimRequest,
    FamilyMaintenanceLeasePolicy, FamilyMaintenanceProof, MaintenanceClaimError, MaintenanceJobKey,
};

/// Preserve PG dedicated-session lifetime; family holds only a durable proof
/// between units. No scheduler/consumer is silently enabled by this adapter.
pub enum GlobalJobClaim {
    Postgres(JobClaim),
    Family(FamilyMaintenanceClaim),
}
pub enum GlobalClaimAcquisition {
    Acquired(GlobalJobClaim),
    Busy,
    Cancelled,
}
pub enum GlobalClaimRelease {
    PostgresFinishAwaited,
    Family(FamilyLeaseAction),
}
impl GlobalJobClaim {
    /// Caller retains the prepared identity across uncertainty and cancellation;
    /// a later observation/replay uses this same request, not a new token.
    pub async fn try_claim(
        backend: &crate::db::backend::Backend,
        request: &FamilyMaintenanceClaimRequest,
        family_policy: FamilyMaintenanceLeasePolicy,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<GlobalClaimAcquisition, MaintenanceClaimError> {
        if cancel.is_cancelled() {
            return Ok(GlobalClaimAcquisition::Cancelled);
        }
        if let crate::db::backend::Backend::Postgres(pool) = backend {
            return Ok(
                match JobClaim::try_claim(pool, request.key() as i32).await? {
                    Some(claim) => GlobalClaimAcquisition::Acquired(Self::Postgres(claim)),
                    None => GlobalClaimAcquisition::Busy,
                },
            );
        }
        use crate::db::maintenance_claim::FamilyClaimAcquisition;
        Ok(
            match request.try_acquire(backend, family_policy, cancel).await? {
                FamilyClaimAcquisition::Acquired(claim) => {
                    GlobalClaimAcquisition::Acquired(Self::Family(claim))
                }
                FamilyClaimAcquisition::Busy => GlobalClaimAcquisition::Busy,
                FamilyClaimAcquisition::Cancelled => GlobalClaimAcquisition::Cancelled,
            },
        )
    }
    pub async fn release(self) -> Result<GlobalClaimRelease, MaintenanceClaimError> {
        match self {
            Self::Postgres(claim) => {
                claim.release().await;
                Ok(GlobalClaimRelease::PostgresFinishAwaited)
            }
            Self::Family(claim) => claim.release().await.map(GlobalClaimRelease::Family),
        }
    }
}
