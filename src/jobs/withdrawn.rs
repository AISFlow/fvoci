//! Final erasure of withdrawn accounts (source `anonymizeWithdrawnUsers`,
//! first step of the daily sweep). A bounded batch per sweep; each user is
//! claimed under its own row lock and deadline recheck.

use chrono::{DateTime, Duration, Utc};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::db::account::{
    anonymize_withdrawn_user, list_withdrawn_due, WITHDRAWN_ANONYMIZE_BATCH, WITHDRAW_GRACE_DAYS,
};

pub async fn run_withdrawn_anonymize(
    pool: &PgPool,
    now: DateTime<Utc>,
    cancel: &CancellationToken,
) -> Result<u32, sqlx::Error> {
    let cutoff = now - Duration::days(WITHDRAW_GRACE_DAYS);
    let targets = list_withdrawn_due(pool, cutoff, WITHDRAWN_ANONYMIZE_BATCH).await?;
    let mut erased = 0u32;
    for user_id in targets {
        if cancel.is_cancelled() {
            break;
        }
        match anonymize_withdrawn_user(pool, user_id, now, cutoff).await {
            Ok(true) => erased += 1,
            Ok(false) => {}
            // One failed claim rolls back alone; the next sweep retries it.
            Err(err) => {
                warn!(%user_id, error = %err, "maintenance.withdrawn_anonymize_user_failed")
            }
        }
    }
    Ok(erased)
}
