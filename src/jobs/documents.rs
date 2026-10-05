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

pub(crate) async fn run_document_trash_purge_family(
    backend: &crate::db::backend::Backend,
    storage: &ObjectStorage,
    limits: DocumentPurgeLimits,
    proof: &super::claim::FamilyMaintenanceProof,
    policy: super::claim::FamilyMaintenanceLeasePolicy,
    cancel: &CancellationToken,
) -> Result<DocumentPurgeStats, super::MaintenanceConsumerError> {
    if cancel.is_cancelled() {
        return Err(super::MaintenanceConsumerError::Cancelled);
    }
    let deadline = Instant::now() + limits.budget;
    let mut read = backend.begin_read().await?;
    let previous = read.operation().set_system().await?;
    let ids = read.operation().maintenance_live_workspace_ids().await?;
    read.operation().restore_system(previous).await?;
    read.rollback().await?;
    let mut queue: VecDeque<(Uuid, Vec<Uuid>)> =
        ids.into_iter().map(|id| (id, Vec::new())).collect();
    let mut stats = DocumentPurgeStats::default();
    let consumer = FamilyDocumentPurge {
        backend,
        storage,
        proof,
        policy,
        cancel,
    };
    while let Some((workspace, mut examined)) = queue.pop_front() {
        if cancel.is_cancelled() {
            return Err(super::MaintenanceConsumerError::Cancelled);
        }
        if Instant::now() >= deadline {
            break;
        }
        let listed = async {
            let mut read = backend.begin_read().await?;
            read.operation().set_tenant(workspace).await?;
            let ids = read
                .operation()
                .maintenance_expired_documents(workspace, &examined, limits.batch)
                .await?;
            read.rollback().await?;
            Ok::<_, sqlx::Error>(ids)
        }
        .await;
        let ids = match listed {
            Ok(ids) => ids,
            Err(err) => {
                if matches!(backend, crate::db::backend::Backend::LibsqlRemote(_)) {
                    return Err(super::MaintenanceConsumerError::AdapterStopped {
                        source: err,
                        abandon_claim: true,
                    });
                }
                warn!(workspace_id=%workspace,error=%err,"maintenance.document_purge_list_failed");
                continue;
            }
        };
        if ids.is_empty() {
            continue;
        }
        for document in ids {
            if cancel.is_cancelled() {
                return Err(super::MaintenanceConsumerError::Cancelled);
            }
            if Instant::now() >= deadline {
                stats.deferred += 1;
                continue;
            }
            match consumer.purge(workspace, document, deadline).await {
                Ok(TrashPurgeOutcome::Purged { storage_deleted }) => {
                    stats.purged += 1;
                    stats.storage_deleted += storage_deleted;
                }
                Ok(TrashPurgeOutcome::Skipped) => {
                    stats.skipped += 1;
                    examined.push(document);
                }
                Ok(TrashPurgeOutcome::StorageFailed) => {
                    stats.failed += 1;
                    examined.push(document);
                }
                Ok(TrashPurgeOutcome::Deferred) => stats.deferred += 1,
                Err(err) if err.stops_on_backend(backend) => return Err(err),
                Err(err) => {
                    stats.failed += 1;
                    examined.push(document);
                    warn!(workspace_id=%workspace,document_id=%document,error=%err,"maintenance.document_purge_failed");
                }
            }
        }
        queue.push_back((workspace, examined));
    }
    Ok(stats)
}

struct FamilyDocumentPurge<'a> {
    backend: &'a crate::db::backend::Backend,
    storage: &'a ObjectStorage,
    proof: &'a super::claim::FamilyMaintenanceProof,
    policy: super::claim::FamilyMaintenanceLeasePolicy,
    cancel: &'a CancellationToken,
}

impl FamilyDocumentPurge<'_> {
    async fn purge(
        &self,
        workspace: Uuid,
        document: Uuid,
        deadline: Instant,
    ) -> Result<TrashPurgeOutcome, super::MaintenanceConsumerError> {
        use super::{
            commit_maintenance_writer, renew_maintenance_writer, rollback_maintenance_writer,
        };
        let key = super::claim::MaintenanceJobKey::Daily;
        let mut tx = self.backend.begin_write().await?;
        let result = async {
            renew_maintenance_writer(&mut tx,self.proof,key,self.policy,self.cancel).await?;
            tx.operation().set_tenant(workspace).await?;
            let Some(keys) = tx.operation().maintenance_document_purge_keys(workspace,document).await? else {
                return Ok::<_,super::MaintenanceConsumerError>(TrashPurgeOutcome::Skipped);
            };
            let doomed = tx.operation().maintenance_document_purge_attachment_ids(workspace,document).await?;
            for object in &keys {
                if tx.operation().attachment_cleanup_key_referenced_globally(workspace,object,
                    crate::db::attachments::AttachmentCleanupReferenceExclusion::DoomedRows(&doomed)).await? {
                    return Ok(TrashPurgeOutcome::Skipped);
                }
            }
            let mut storage_deleted = 0;
            for object in &keys {
                if Instant::now() >= deadline { return Ok(TrashPurgeOutcome::Deferred) }
                renew_maintenance_writer(&mut tx,self.proof,key,self.policy,self.cancel).await?;
                if tx.operation().attachment_cleanup_key_referenced_globally(workspace,object,
                    crate::db::attachments::AttachmentCleanupReferenceExclusion::DoomedRows(&doomed)).await? {
                    return Ok(TrashPurgeOutcome::Skipped);
                }
                let failure = match tokio::time::timeout(crate::db::document_purge::PURGE_KEY_TIMEOUT,self.storage.purge_key(object)).await {
                    Ok(Ok(())) => None,
                    Ok(Err(err)) => Some(err.to_string()),
                    Err(_) => Some(format!("timed out after {}s",crate::db::document_purge::PURGE_KEY_TIMEOUT.as_secs())),
                };
                if let Some(error) = failure {
                    warn!(workspace_id=%workspace,document_id=%document,error=%error,"maintenance.document_purge_storage_failed");
                    return Ok(TrashPurgeOutcome::StorageFailed);
                }
                storage_deleted += 1;
            }
            renew_maintenance_writer(&mut tx,self.proof,key,self.policy,self.cancel).await?;
            let Some(current) = tx.operation().maintenance_document_purge_keys(workspace,document).await? else { return Ok(TrashPurgeOutcome::Skipped) };
            if current.iter().any(|object|!keys.contains(object)) { return Ok(TrashPurgeOutcome::Skipped) }
            if !tx.operation().maintenance_delete_document(workspace,document).await? { return Ok(TrashPurgeOutcome::Skipped) }
            Ok(TrashPurgeOutcome::Purged { storage_deleted })
        }.await;
        let outcome = match result {
            Ok(outcome) => outcome,
            Err(err) => return Err(rollback_maintenance_writer(tx, err).await),
        };
        commit_maintenance_writer(tx, self.proof, key, self.cancel).await?;
        Ok(outcome)
    }
}

#[cfg(test)]
mod family_tests {
    use super::*;
    use crate::db::backend::OperationTx;
    use crate::db::codec::Cell;
    use crate::db::notifications::family_runtime_fixture::Fixture;

    #[tokio::test]
    async fn maintenance_document_current_children_uploads_history_and_journal() {
        let f = Fixture::new().await;
        let child = Uuid::now_v7();
        let attachment = Uuid::now_v7();
        let revision = Uuid::now_v7();
        sqlx::query("UPDATE documents SET deleted_at=1 WHERE id=?1")
            .bind(f.document.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO documents(id,workspace_id,parent_id,title,path,sort_key,number,status,schema_version,created_by,content_json) VALUES(?1,?2,?3,'current child',?4,'b',2,'published',2,?5,'{}')")
            .bind(child.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(f.document.as_bytes().as_slice())
            .bind(format!("{}.{}",f.document.simple(),child.simple())).bind(f.actor.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        sqlx::query("INSERT INTO attachments(id,workspace_id,document_id,uploader_id,status,name,reserved_size_bytes,storage_key,variants) VALUES(?1,?2,?3,?4,'assembling','old object',5,'maintenance-original','{\"preview\":{\"key\":\"maintenance-preview\"}}')")
            .bind(attachment.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(f.document.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        sqlx::query("INSERT INTO revisions(id,workspace_id,target_kind,target_id,y_snapshot,content_json,text,reason) VALUES(?1,?2,'document',?3,X'00','{}','history','manual')")
            .bind(revision.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(f.document.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        let mut denied = f.backend.begin_write().await.unwrap();
        assert!(denied
            .operation()
            .maintenance_document_purge_keys(f.workspace, f.document)
            .await
            .is_err());
        denied.rollback().await.unwrap();
        let mut tx = f.backend.begin_write().await.unwrap();
        tx.operation().set_tenant(f.workspace).await.unwrap();
        assert_eq!(
            tx.operation()
                .maintenance_expired_documents(f.workspace, &[], 200)
                .await
                .unwrap(),
            vec![f.document]
        );
        assert!(tx
            .operation()
            .maintenance_expired_documents(f.workspace, &[f.document], 200)
            .await
            .unwrap()
            .is_empty());
        assert!(
            tx.operation()
                .maintenance_document_purge_keys(f.workspace, f.document)
                .await
                .unwrap()
                .is_none(),
            "a live child prevents purge"
        );
        let OperationTx::SqliteFamily(writer) = tx.operation() else {
            panic!("actual SQLite writer required")
        };
        writer
            .execute(
                "DELETE FROM documents WHERE id=?1 AND workspace_id=?2",
                &[Cell::uuid(child), Cell::uuid(f.workspace)],
            )
            .await
            .unwrap();
        assert!(
            tx.operation()
                .maintenance_document_purge_keys(f.workspace, f.document)
                .await
                .unwrap()
                .is_none(),
            "assembling belongs to stale upload cleanup"
        );
        let OperationTx::SqliteFamily(writer) = tx.operation() else {
            panic!("actual SQLite writer required")
        };
        writer
            .execute(
                "UPDATE attachments SET status='stored',size_bytes=5,completed_at=1 WHERE id=?1",
                &[Cell::uuid(attachment)],
            )
            .await
            .unwrap();
        assert_eq!(
            tx.operation()
                .maintenance_document_purge_keys(f.workspace, f.document)
                .await
                .unwrap(),
            Some(vec!["maintenance-original".into()])
        );
        assert!(tx
            .operation()
            .maintenance_delete_document(f.workspace, f.document)
            .await
            .unwrap());
        tx.commit().await.unwrap();
        let rows: (i64,i64,i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM documents WHERE id=?1),(SELECT count(*) FROM attachments WHERE id=?2),(SELECT count(*) FROM revisions WHERE id=?3),(SELECT count(*) FROM comments WHERE document_id=?1)")
            .bind(f.document.as_bytes().as_slice()).bind(attachment.as_bytes().as_slice()).bind(revision.as_bytes().as_slice()).fetch_one(&f.pool).await.unwrap();
        assert_eq!(rows, (0, 0, 0, 0));
        let journal: Vec<String> = sqlx::query_scalar("SELECT storage_key FROM attachment_object_cleanups WHERE attachment_id=?1 ORDER BY storage_key")
            .bind(attachment.as_bytes().as_slice()).fetch_all(&f.pool).await.unwrap();
        assert_eq!(journal, vec!["maintenance-original", "maintenance-preview"]);
        let event: (String, String) =
            sqlx::query_as("SELECT verb,channel FROM events WHERE target_id=?1")
                .bind(f.document.as_bytes().as_slice())
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert_eq!(event, ("document.purged".into(), "system".into()));
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
    async fn maintenance_document_global_refs_protect_other_parent_and_tenant_before_purge() {
        for same_workspace in [true, false] {
            let f = Fixture::new().await;
            let storage = ObjectStorage::local(f.root.join("objects"));
            let doomed = f.document(f.workspace).await;
            let key = Uuid::now_v7().to_string();
            f.stored_attachment(f.workspace, doomed, &key, None).await;
            storage.put_bytes(&key, b"part".to_vec()).await.unwrap();
            let outside_workspace = if same_workspace {
                f.workspace
            } else {
                f.other_workspace().await
            };
            let outside_document = f.document(outside_workspace).await;
            let outside_key = Uuid::now_v7().to_string();
            let outside = f
                .stored_attachment(
                    outside_workspace,
                    outside_document,
                    &outside_key,
                    Some(&key),
                )
                .await;
            storage
                .put_bytes(&outside_key, b"live".to_vec())
                .await
                .unwrap();
            sqlx::query("UPDATE documents SET deleted_at=?2 WHERE id=?1")
                .bind(doomed.as_bytes().as_slice())
                .bind(
                    (crate::jobs::family_maintenance_now() - chrono::Duration::days(31))
                        .timestamp_micros(),
                )
                .execute(&f.pool)
                .await
                .unwrap();
            let request = FamilyMaintenanceClaimRequest::new(MaintenanceJobKey::Daily);
            let owner = acquired(&request, &f.backend).await;
            let stats = run_document_trash_purge_family(
                &f.backend,
                &storage,
                DocumentPurgeLimits::default(),
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
                .bind(outside.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
            let stats = run_document_trash_purge_family(
                &f.backend,
                &storage,
                DocumentPurgeLimits::default(),
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
            assert_eq!(retained, vec![outside.as_bytes().to_vec()]);
            assert_eq!(
                sqlx::query_scalar::<_, i64>("SELECT count(*) FROM documents WHERE id=?1")
                    .bind(outside_document.as_bytes().as_slice())
                    .fetch_one(&f.pool)
                    .await
                    .unwrap(),
                1
            );
            owner.release().await.unwrap();
            f.finish().await;
        }
    }
}
