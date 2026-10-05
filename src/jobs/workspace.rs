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
/// (idempotent) and then removes the workspace. Deleting the rows also
/// journals every original and preview key through the migration-030 trigger,
/// and `reclaim_attachment_objects` purges them again (as already missing).
/// Storage-first predates that journal and stays; a DB-only purge would hand
/// every key to the reclaimer's batched drain (`OBJECT_CLEANUP_BATCH` rows
/// per upload-GC run).
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

pub(crate) async fn run_workspace_purge_family(
    backend: &crate::db::backend::Backend,
    storage: &ObjectStorage,
    now: DateTime<Utc>,
    proof: &super::claim::FamilyMaintenanceProof,
    policy: super::claim::FamilyMaintenanceLeasePolicy,
    cancel: &CancellationToken,
) -> Result<WorkspacePurgeStats, super::MaintenanceConsumerError> {
    let cutoff = now - chrono::Duration::days(WORKSPACE_PURGE_AFTER_DAYS);
    if cancel.is_cancelled() {
        return Err(super::MaintenanceConsumerError::Cancelled);
    }
    let mut read = backend.begin_read().await?;
    let previous = read.operation().set_system().await?;
    let ids = read
        .operation()
        .maintenance_deleted_workspaces(cutoff)
        .await?;
    read.operation().restore_system(previous).await?;
    read.rollback().await?;
    let mut stats = WorkspacePurgeStats::default();
    for workspace in ids.into_iter().take(WORKSPACE_PURGE_BATCH as usize) {
        if cancel.is_cancelled() {
            return Err(super::MaintenanceConsumerError::Cancelled);
        }
        stats.claimed += 1;
        match purge_one_family(backend, storage, workspace, cutoff, proof, policy, cancel).await {
            Ok(PurgeOne::Purged { storage_deleted }) => {
                stats.purged += 1;
                stats.storage_deleted += storage_deleted;
            }
            Ok(PurgeOne::Skipped) => stats.skipped += 1,
            Ok(PurgeOne::StorageFailed { failed }) => {
                stats.storage_failed += failed;
                stats.skipped += 1;
            }
            Err(err) if err.stops_on_backend(backend) => return Err(err),
            Err(err) => {
                tracing::error!(workspace_id=%workspace,error=%err,"cleanup.workspace_purge_failed");
                stats.skipped += 1;
            }
        }
    }
    Ok(stats)
}

async fn purge_one_family(
    backend: &crate::db::backend::Backend,
    storage: &ObjectStorage,
    workspace: Uuid,
    cutoff: DateTime<Utc>,
    proof: &super::claim::FamilyMaintenanceProof,
    policy: super::claim::FamilyMaintenanceLeasePolicy,
    cancel: &CancellationToken,
) -> Result<PurgeOne, super::MaintenanceConsumerError> {
    use super::{commit_maintenance_writer, renew_maintenance_writer, rollback_maintenance_writer};
    let key = super::claim::MaintenanceJobKey::Daily;
    let mut tx = backend.begin_write().await?;
    let result = async {
        renew_maintenance_writer(&mut tx, proof, key, policy, cancel).await?;
        let previous = tx.operation().set_system().await?;
        tx.operation().set_tenant(workspace).await?;
        let Some(keys) = tx.operation().maintenance_workspace_purge_keys(workspace, cutoff).await? else {
            tx.operation().restore_system(previous).await?;
            return Ok::<_,super::MaintenanceConsumerError>(PurgeOne::Skipped);
        };
        let doomed = tx.operation().maintenance_workspace_purge_attachment_ids(workspace).await?;
        // The authoritative physical-key predicate requires the current
        // tenant with system scope off. Do not broaden it for foreign rows.
        tx.operation().restore_system(previous).await?;
        for object in &keys {
            if tx.operation().attachment_cleanup_key_referenced_globally(workspace, object,
                crate::db::attachments::AttachmentCleanupReferenceExclusion::DoomedRows(&doomed)).await? {
                tx.operation().restore_system(previous).await?;
                return Ok(PurgeOne::Skipped);
            }
        }
        let mut failed = 0;
        let mut deleted = 0;
        for object in keys {
            // Retain the actual writer across the maintained storage operation.
            // Do not introduce a new timeout/abort for a live provider cleanup.
            renew_maintenance_writer(&mut tx, proof, key, policy, cancel).await?;
            if tx.operation().attachment_cleanup_key_referenced_globally(workspace, &object,
                crate::db::attachments::AttachmentCleanupReferenceExclusion::DoomedRows(&doomed)).await? {
                tx.operation().restore_system(previous).await?;
                return Ok(PurgeOne::Skipped);
            }
            match storage.purge_key(&object).await {
                Ok(()) => deleted += 1,
                Err(err) => {
                    failed += 1;
                    tracing::warn!(workspace_id=%workspace,error=%err,"cleanup.attachment_delete_failed");
                }
            }
        }
        let outcome = if failed > 0 {
            PurgeOne::StorageFailed { failed }
        } else {
            renew_maintenance_writer(&mut tx, proof, key, policy, cancel).await?;
            tx.operation().set_system().await?;
            let purged = tx.operation().maintenance_purge_workspace(workspace).await?;
            if purged.purged { PurgeOne::Purged { storage_deleted: deleted } } else { PurgeOne::Skipped }
        };
        tx.operation().restore_system(previous).await?;
        Ok(outcome)
    }.await;
    let outcome = match result {
        Ok(outcome) => outcome,
        Err(err) => return Err(rollback_maintenance_writer(tx, err).await),
    };
    commit_maintenance_writer(tx, proof, key, cancel).await?;
    Ok(outcome)
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

#[cfg(test)]
mod family_tests {
    use super::*;
    use crate::db::backend::OperationTx;
    use crate::db::codec::Cell;
    use crate::db::notifications::family_runtime_fixture::Fixture;
    use chrono::SubsecRound;

    #[tokio::test]
    async fn maintenance_workspace_current_tombstone_keys_cascade_and_journal() {
        let f = Fixture::new().await;
        let now = Utc::now().trunc_subsecs(6);
        let cutoff = now - chrono::Duration::days(WORKSPACE_PURGE_AFTER_DAYS);
        let attachment = Uuid::now_v7();
        sqlx::query("UPDATE workspaces SET deleted_at=?2 WHERE id=?1")
            .bind(f.workspace.as_bytes().as_slice())
            .bind((cutoff - chrono::Duration::seconds(1)).timestamp_micros())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE workspaces SET deleted_at=?2 WHERE id=?1")
            .bind(f.other_workspace.as_bytes().as_slice())
            .bind((cutoff + chrono::Duration::hours(1)).timestamp_micros())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO attachments(id,workspace_id,document_id,uploader_id,status,name,size_bytes,reserved_size_bytes,storage_key,completed_at,variants) VALUES(?1,?2,?3,?4,'stored','old object',5,5,'workspace-original',1,'{\"preview\":{\"key\":\"workspace-preview\"}}')")
            .bind(attachment.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(f.document.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        let mut denied = f.backend.begin_write().await.unwrap();
        denied.operation().set_tenant(f.workspace).await.unwrap();
        assert!(denied
            .operation()
            .maintenance_workspace_purge_keys(f.workspace, cutoff)
            .await
            .is_err());
        denied.rollback().await.unwrap();
        let mut tx = f.backend.begin_write().await.unwrap();
        let previous = tx.operation().set_system().await.unwrap();
        assert_eq!(
            tx.operation()
                .maintenance_deleted_workspaces(cutoff)
                .await
                .unwrap(),
            vec![f.workspace]
        );
        tx.operation().set_tenant(f.workspace).await.unwrap();
        let mut keys = tx
            .operation()
            .maintenance_workspace_purge_keys(f.workspace, cutoff)
            .await
            .unwrap()
            .unwrap();
        keys.sort();
        assert_eq!(keys, vec!["workspace-original", "workspace-preview"]);
        let OperationTx::SqliteFamily(writer) = tx.operation() else {
            panic!("actual family writer required")
        };
        writer
            .execute(
                "UPDATE workspaces SET deleted_at=NULL WHERE id=?1",
                &[Cell::uuid(f.workspace)],
            )
            .await
            .unwrap();
        assert!(tx
            .operation()
            .maintenance_workspace_purge_keys(f.workspace, cutoff)
            .await
            .unwrap()
            .is_none());
        assert!(
            !tx.operation()
                .maintenance_purge_workspace(f.workspace)
                .await
                .unwrap()
                .purged,
            "current restored workspace cannot purge"
        );
        let OperationTx::SqliteFamily(writer) = tx.operation() else {
            panic!("actual family writer required")
        };
        writer
            .execute(
                "UPDATE workspaces SET deleted_at=1 WHERE id=?1",
                &[Cell::uuid(f.workspace)],
            )
            .await
            .unwrap();
        assert!(tx
            .operation()
            .maintenance_workspace_purge_keys(f.workspace, cutoff)
            .await
            .unwrap()
            .is_some());
        let purged = tx
            .operation()
            .maintenance_purge_workspace(f.workspace)
            .await
            .unwrap();
        assert!(purged.purged);
        assert_eq!(purged.storage_keys, vec!["workspace-original"]);
        tx.operation().restore_system(previous).await.unwrap();
        tx.commit().await.unwrap();
        let rows: (i64,i64,i64,i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM workspaces WHERE id=?1),(SELECT count(*) FROM documents WHERE workspace_id=?1),(SELECT count(*) FROM attachments WHERE workspace_id=?1),(SELECT count(*) FROM memberships WHERE workspace_id=?1),(SELECT count(*) FROM workspaces WHERE id=?2)")
            .bind(f.workspace.as_bytes().as_slice()).bind(f.other_workspace.as_bytes().as_slice()).fetch_one(&f.pool).await.unwrap();
        assert_eq!(rows, (0, 0, 0, 0, 1));
        let journal: Vec<String> = sqlx::query_scalar("SELECT storage_key FROM attachment_object_cleanups WHERE attachment_id=?1 ORDER BY storage_key")
            .bind(attachment.as_bytes().as_slice()).fetch_all(&f.pool).await.unwrap();
        assert_eq!(journal, vec!["workspace-original", "workspace-preview"]);
        f.backend.close().await.unwrap();
        std::fs::remove_dir_all(f.dir).unwrap();
    }
}

#[cfg(test)]
mod claimed_storage_tests {
    use super::*;
    use crate::jobs::claim::FamilyMaintenanceClaimRequest;
    use crate::jobs::family_maintenance_fixture::{acquired, policy, Fixture};
    use crate::jobs::MaintenanceJobKey;

    #[tokio::test]
    async fn maintenance_workspace_global_preview_ref_refuses_before_storage_then_healthy_progress()
    {
        let f = Fixture::new().await;
        let storage = ObjectStorage::local(f.root.join("objects"));
        let document = f.document(f.workspace).await;
        let key = Uuid::now_v7().to_string();
        f.stored_attachment(f.workspace, document, &key, None).await;
        storage.put_bytes(&key, b"part".to_vec()).await.unwrap();
        let other = f.other_workspace().await;
        let outside = f.document(other).await;
        let outside_key = Uuid::now_v7().to_string();
        let outside_attachment = f
            .stored_attachment(other, outside, &outside_key, Some(&key))
            .await;
        storage
            .put_bytes(&outside_key, b"live".to_vec())
            .await
            .unwrap();
        let now = crate::jobs::family_maintenance_now();
        sqlx::query("UPDATE workspaces SET deleted_at=?2 WHERE id=?1")
            .bind(f.workspace.as_bytes().as_slice())
            .bind((now - chrono::Duration::days(31)).timestamp_micros())
            .execute(&f.pool)
            .await
            .unwrap();
        let request = FamilyMaintenanceClaimRequest::new(MaintenanceJobKey::Daily);
        let owner = acquired(&request, &f.backend).await;
        let stats = run_workspace_purge_family(
            &f.backend,
            &storage,
            now,
            owner.proof(),
            policy(),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(
            (stats.purged, stats.storage_deleted, stats.skipped),
            (0, 0, 1)
        );
        assert_eq!(storage.read_range(&key, 0, 3).await.unwrap(), b"part");
        assert_eq!(
            storage.read_range(&outside_key, 0, 3).await.unwrap(),
            b"live"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM attachments")
                .fetch_one(&f.pool)
                .await
                .unwrap(),
            2
        );
        sqlx::query("UPDATE attachments SET variants='{}' WHERE id=?1")
            .bind(outside_attachment.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let stats = run_workspace_purge_family(
            &f.backend,
            &storage,
            now,
            owner.proof(),
            policy(),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!((stats.purged, stats.storage_deleted), (1, 1));
        assert_eq!(storage.head(&key).await.unwrap(), None);
        assert_eq!(
            storage.read_range(&outside_key, 0, 3).await.unwrap(),
            b"live"
        );
        let retained: Vec<Vec<u8>> = sqlx::query_scalar("SELECT id FROM attachments")
            .fetch_all(&f.pool)
            .await
            .unwrap();
        assert_eq!(retained, vec![outside_attachment.as_bytes().to_vec()]);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM workspaces WHERE id=?1")
                .bind(other.as_bytes().as_slice())
                .fetch_one(&f.pool)
                .await
                .unwrap(),
            1
        );
        owner.release().await.unwrap();
        f.finish().await;
    }
}
