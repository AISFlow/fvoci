//! Abandoned / expired multipart upload cleanup.
//!
//! Ports `gcStaleUploadRow` in `packages/jobs/src/sweep.ts` at source SHA
//! `393795261322b916e588043cf94feca999175843`. The source runs it inside the
//! daily sweep; here it is its own scheduler job with a shorter cadence,
//! because the TTL is hours long and stale S3 multipart uploads keep costing
//! storage until aborted. Concurrent complete holds the same upload session
//! lock across publish, so this sweep cannot delete a key a late complete
//! still recreates.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::attachments::ObjectStorage;
use crate::db::attachments::{gc_stale_upload_row, list_stale_uploading};

/// Rows examined per run. Anything left over waits for the next cadence.
pub const UPLOAD_GC_BATCH: i64 = 200;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct StaleUploadGcStats {
    pub claimed: u32,
    pub purged: u32,
    /// Rows whose storage cleanup failed; kept for the next run.
    pub failed: u32,
}

/// Removes incomplete uploads created before `cutoff`: every open multipart
/// upload for the key is aborted, the object deleted, then the row. A row
/// whose storage cleanup fails keeps its DB row and does not stop the rest.
pub async fn run_stale_upload_gc(
    pool: &PgPool,
    storage: &ObjectStorage,
    cutoff: DateTime<Utc>,
    cancel: &CancellationToken,
) -> Result<StaleUploadGcStats, sqlx::Error> {
    let rows = list_stale_uploading(pool, cutoff, UPLOAD_GC_BATCH).await?;
    let mut stats = StaleUploadGcStats::default();
    for row in rows {
        if cancel.is_cancelled() {
            break;
        }
        stats.claimed += 1;
        match gc_stale_upload_row(pool, storage, row.workspace_id, row.id).await {
            Ok(true) => stats.purged += 1,
            Ok(false) => {}
            Err(err) => {
                stats.failed += 1;
                warn!(attachment_id = %row.id, error = %err, "maintenance.upload_gc_row_failed");
            }
        }
    }
    Ok(stats)
}
