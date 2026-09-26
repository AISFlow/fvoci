//! Daily trash retention purge of documents (source `purgeTrashedDocuments`,
//! run from the same daily sweep). See `db::document_purge` for ordering.

use std::collections::VecDeque;
use std::time::Duration;

use sqlx::PgPool;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::warn;
use uuid::Uuid;

use crate::attachments::ObjectStorage;
use crate::db::document_purge::{
    list_expired_trashed_documents, list_live_workspace_ids, purge_trashed_document,
    TrashPurgeOutcome,
};

/// Documents listed per workspace per round; workspaces take turns.
pub const DOCUMENT_PURGE_BATCH: i64 = 200;
/// Wall-clock budget of one sweep; what is left waits for the next sweep.
pub const DOCUMENT_PURGE_TIME_BUDGET: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Clone, Copy)]
pub struct DocumentPurgeLimits {
    pub batch: i64,
    pub budget: Duration,
}

impl Default for DocumentPurgeLimits {
    fn default() -> Self {
        Self {
            batch: DOCUMENT_PURGE_BATCH,
            budget: DOCUMENT_PURGE_TIME_BUDGET,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DocumentPurgeStats {
    pub purged: u32,
    pub storage_deleted: u32,
    pub skipped: u32,
    pub failed: u32,
    /// Documents left because the time budget ran out.
    pub deferred: u32,
}

pub async fn run_document_trash_purge(
    pool: &PgPool,
    storage: &ObjectStorage,
    cancel: &CancellationToken,
) -> Result<DocumentPurgeStats, sqlx::Error> {
    run_document_trash_purge_with(pool, storage, DocumentPurgeLimits::default(), cancel).await
}

/// Round-robin over live workspaces in batches until every workspace has no
/// unexamined expired document, the budget runs out, or shutdown. Ids examined
/// in this run (skipped or failed) are excluded from later batches, so failing
/// documents never stall the rest; a listing error drops only that workspace.
pub async fn run_document_trash_purge_with(
    pool: &PgPool,
    storage: &ObjectStorage,
    limits: DocumentPurgeLimits,
    cancel: &CancellationToken,
) -> Result<DocumentPurgeStats, sqlx::Error> {
    let deadline = Instant::now() + limits.budget;
    let mut stats = DocumentPurgeStats::default();
    let mut queue: VecDeque<(Uuid, Vec<Uuid>)> = list_live_workspace_ids(pool)
        .await?
        .into_iter()
        .map(|id| (id, Vec::new()))
        .collect();
    while let Some((workspace_id, mut examined)) = queue.pop_front() {
        if cancel.is_cancelled() || Instant::now() >= deadline {
            break;
        }
        let ids = match list_expired_trashed_documents(pool, workspace_id, &examined, limits.batch)
            .await
        {
            Ok(ids) => ids,
            Err(err) => {
                warn!(
                    workspace_id = %workspace_id,
                    error = %err,
                    "maintenance.document_purge_list_failed"
                );
                continue;
            }
        };
        if ids.is_empty() {
            continue;
        }
        for document_id in ids {
            if cancel.is_cancelled() {
                return Ok(stats);
            }
            if Instant::now() >= deadline {
                stats.deferred += 1;
                continue;
            }
            match purge_trashed_document(pool, storage, workspace_id, document_id, deadline).await {
                Ok(TrashPurgeOutcome::Purged { storage_deleted }) => {
                    stats.purged += 1;
                    stats.storage_deleted += storage_deleted;
                }
                Ok(TrashPurgeOutcome::Skipped) => {
                    stats.skipped += 1;
                    examined.push(document_id);
                }
                Ok(TrashPurgeOutcome::StorageFailed) => {
                    stats.failed += 1;
                    examined.push(document_id);
                }
                Ok(TrashPurgeOutcome::Deferred) => stats.deferred += 1,
                Err(err) => {
                    stats.failed += 1;
                    examined.push(document_id);
                    warn!(
                        workspace_id = %workspace_id,
                        document_id = %document_id,
                        error = %err,
                        "maintenance.document_purge_failed"
                    );
                }
            }
        }
        queue.push_back((workspace_id, examined));
    }
    Ok(stats)
}
