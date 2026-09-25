//! Abandoned / expired multipart upload cleanup.
//!
//! Ports `gcStaleUploadRow` in `packages/jobs/src/sweep.ts` at source SHA
//! `393795261322b916e588043cf94feca999175843`. Concurrent complete holds the
//! same upload session lock across publish, so this sweep cannot delete a
//! key a late complete still recreates.

use std::time::Duration;

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use super::backend::ObjectStorage;
use crate::db::attachments::{gc_stale_upload_row, list_stale_uploading};

pub struct StaleUploadCleanupHandle {
    cancel: CancellationToken,
    join: Mutex<Option<JoinHandle<()>>>,
}

impl StaleUploadCleanupHandle {
    pub fn request_shutdown(&self) {
        self.cancel.cancel();
    }

    pub async fn join(self) -> Result<(), String> {
        self.cancel.cancel();
        if let Some(join) = self.join.lock().await.take() {
            join.await
                .map_err(|err| format!("stale-upload cleanup join failed: {err}"))?;
        }
        Ok(())
    }
}

pub fn spawn_stale_upload_cleanup(
    pool: PgPool,
    storage: ObjectStorage,
    ttl: Duration,
    interval: Duration,
) -> StaleUploadCleanupHandle {
    let cancel = CancellationToken::new();
    let child = cancel.child_token();
    let join = tokio::spawn(run_loop(pool, storage, ttl, interval, child));
    StaleUploadCleanupHandle {
        cancel,
        join: Mutex::new(Some(join)),
    }
}

async fn run_loop(
    pool: PgPool,
    storage: ObjectStorage,
    ttl: Duration,
    interval: Duration,
    cancel: CancellationToken,
) {
    loop {
        if cancel.is_cancelled() {
            break;
        }
        let cutoff = Utc::now()
            - chrono::Duration::from_std(ttl).unwrap_or_else(|_| chrono::Duration::hours(24));
        if let Err(err) = gc_stale_uploads(&pool, &storage, cutoff).await {
            warn!(error = %err, "stale upload cleanup cycle failed");
        }
        tokio::select! {
            () = cancel.cancelled() => break,
            () = tokio::time::sleep(interval) => {}
        }
    }
}

pub async fn gc_stale_uploads(
    pool: &PgPool,
    storage: &ObjectStorage,
    cutoff: DateTime<Utc>,
) -> Result<u32, sqlx::Error> {
    let rows = list_stale_uploading(pool, cutoff).await?;
    let mut purged = 0u32;
    for row in rows {
        // One failing row (e.g. a transient S3 error) keeps its DB row for the
        // next cycle and must not starve the rest of the sweep.
        match gc_stale_upload_row(pool, storage, row.workspace_id, row.id).await {
            Ok(true) => purged += 1,
            Ok(false) => {}
            Err(err) => {
                warn!(attachment_id = %row.id, error = %err, "stale upload cleanup failed for row");
            }
        }
    }
    Ok(purged)
}

/// Sweep cadence. The source sweeps stale uploads once a day; the TTL is
/// hours long, so a 10-minute cycle keeps cleanup prompt without querying
/// every workspace each minute.
pub fn cleanup_interval() -> Duration {
    Duration::from_secs(600)
}
