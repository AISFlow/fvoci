//! Daily trash retention purge of documents (source `purgeTrashedDocuments`,
//! run from the same daily sweep). See `db::document_purge` for ordering.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::attachments::ObjectStorage;
use crate::db::document_purge::{
    list_expired_trashed_documents, list_live_workspace_ids, purge_trashed_document,
    TrashPurgeOutcome,
};
use crate::db::documents::trash_retention_cutoff;

/// Documents examined per workspace per run; the rest wait for the next sweep.
pub const DOCUMENT_PURGE_BATCH: i64 = 200;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DocumentPurgeStats {
    pub purged: u32,
    pub storage_deleted: u32,
    pub skipped: u32,
    pub failed: u32,
}

pub async fn run_document_trash_purge(
    pool: &PgPool,
    storage: &ObjectStorage,
    now: DateTime<Utc>,
    cancel: &CancellationToken,
) -> Result<DocumentPurgeStats, sqlx::Error> {
    let cutoff = trash_retention_cutoff(now);
    let mut stats = DocumentPurgeStats::default();
    for workspace_id in list_live_workspace_ids(pool).await? {
        if cancel.is_cancelled() {
            break;
        }
        let ids = list_expired_trashed_documents(pool, workspace_id, cutoff, DOCUMENT_PURGE_BATCH)
            .await?;
        for document_id in ids {
            if cancel.is_cancelled() {
                return Ok(stats);
            }
            match purge_trashed_document(pool, storage, workspace_id, document_id, cutoff).await {
                Ok(TrashPurgeOutcome::Purged { storage_deleted }) => {
                    stats.purged += 1;
                    stats.storage_deleted += storage_deleted;
                }
                Ok(TrashPurgeOutcome::Skipped) => stats.skipped += 1,
                Ok(TrashPurgeOutcome::StorageFailed) => stats.failed += 1,
                Err(err) => {
                    stats.failed += 1;
                    warn!(
                        workspace_id = %workspace_id,
                        document_id = %document_id,
                        error = %err,
                        "maintenance.document_purge_failed"
                    );
                }
            }
        }
    }
    Ok(stats)
}
