use chrono::{DateTime, Utc};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::attachments::ObjectStorage;
use crate::db::context::{set_system, set_tenant};
use crate::db::workspace::{list_deleted_workspace_ids, purge_workspace};

pub const WORKSPACE_PURGE_AFTER_DAYS: i64 = 30;
pub const WORKSPACE_PURGE_BATCH: i64 = 50;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct WorkspacePurgeStats {
    pub claimed: u32,
    pub purged: u32,
    pub storage_deleted: u32,
    pub storage_failed: u32,
    pub skipped: u32,
}

/// Crash-safe ordering: attachment rows are implicit tombstones.
/// Storage is cleaned first through `ObjectStorage::purge_key`: every open
/// multipart upload for each key (the row's own and orphans that never had
/// their id persisted) is aborted, then the object is deleted. A key that is
/// already gone counts as deleted; any other storage error (403, wrong
/// bucket, network) keeps every DB row for the next sweep. The DB purge
/// commits only after every key for that workspace was cleaned.
/// A crash mid-storage leaves the rows for the next sweep. A crash after
/// storage and before the DB purge: the next sweep deletes missing objects
/// (idempotent) and then removes the workspace. DB-first without a journal
/// table would orphan objects; a tombstone table would need a migration.
pub async fn run_workspace_purge(
    pool: &PgPool,
    storage: &ObjectStorage,
    now: DateTime<Utc>,
    cancel: &CancellationToken,
) -> Result<WorkspacePurgeStats, sqlx::Error> {
    let cutoff = now - chrono::Duration::days(WORKSPACE_PURGE_AFTER_DAYS);
    let ids = list_deleted_workspace_ids(pool, cutoff).await?;
    let mut stats = WorkspacePurgeStats::default();
    for id in ids.into_iter().take(WORKSPACE_PURGE_BATCH as usize) {
        if cancel.is_cancelled() {
            break;
        }
        stats.claimed += 1;
        match purge_one(pool, storage, id).await {
            Ok(PurgeOne::Purged { storage_deleted }) => {
                stats.purged += 1;
                stats.storage_deleted += storage_deleted;
            }
            Ok(PurgeOne::Skipped) => stats.skipped += 1,
            Ok(PurgeOne::StorageFailed { failed }) => {
                stats.storage_failed += failed;
                stats.skipped += 1;
            }
            Err(err) => {
                tracing::error!(
                    workspace_id = %id,
                    error = %err,
                    "cleanup.workspace_purge_failed"
                );
                stats.skipped += 1;
            }
        }
    }
    Ok(stats)
}

enum PurgeOne {
    Purged { storage_deleted: u32 },
    Skipped,
    StorageFailed { failed: u32 },
}

async fn purge_one(
    pool: &PgPool,
    storage: &ObjectStorage,
    workspace_id: Uuid,
) -> Result<PurgeOne, sqlx::Error> {
    let keys = list_storage_keys(pool, workspace_id).await?;
    let mut failed = 0u32;
    let mut deleted = 0u32;
    for key in &keys {
        match storage.purge_key(key).await {
            Ok(()) => deleted += 1,
            Err(err) => {
                failed += 1;
                tracing::warn!(
                    workspace_id = %workspace_id,
                    error = %err,
                    "cleanup.attachment_delete_failed"
                );
            }
        }
    }
    if failed > 0 {
        return Ok(PurgeOne::StorageFailed { failed });
    }
    let result = purge_workspace(pool, workspace_id).await?;
    if result.purged {
        Ok(PurgeOne::Purged {
            storage_deleted: deleted,
        })
    } else {
        Ok(PurgeOne::Skipped)
    }
}

async fn list_storage_keys(pool: &PgPool, workspace_id: Uuid) -> Result<Vec<String>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let previous = set_system(&mut tx).await?;
    set_tenant(&mut tx, workspace_id).await?;
    // Originals and their published preview objects.
    let rows: Vec<(String,)> = sqlx::query_as(
        r#"
        SELECT storage_key FROM fvoci.attachments WHERE workspace_id = $1
        UNION ALL
        SELECT variants -> 'preview' ->> 'key' FROM fvoci.attachments
        WHERE workspace_id = $1 AND jsonb_typeof(variants -> 'preview' -> 'key') = 'string'
        "#,
    )
    .bind(workspace_id)
    .fetch_all(&mut *tx)
    .await?;
    crate::db::context::restore_system(&mut tx, &previous).await?;
    tx.commit().await?;
    Ok(rows.into_iter().map(|(key,)| key).collect())
}
