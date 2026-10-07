//! In-process maintenance scheduler.
//!
//! One concept: named jobs, session advisory claim (`pg_try_advisory_lock`),
//! bounded batches, cancel-aware drain. There is no extra daemon. Source
//! BullMQ `daily-sweep` (`0 4 * * *`) is the same work under one cluster lock;
//! here each replica ticks and only the claimant runs.
//!
//! Source does not purge `events`, `audit_log`, or collab receipts. Those
//! tables stay append-only (app role cannot DELETE them).
//!
//! Abandoned-upload cleanup is a second job in the same loop with its own
//! cadence and claim key.

mod claim;
mod documents;
mod retention;
mod revisions;
mod tokens;
mod uploads;
mod withdrawn;
mod workspace;

use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::attachments::ObjectStorage;
use crate::mail::Mailer;

pub use claim::{
    FamilyLeaseAction, FamilyMaintenanceClaim, FamilyMaintenanceClaimRequest,
    FamilyMaintenanceLeasePolicy, FamilyMaintenanceProof, GlobalClaimAcquisition,
    GlobalClaimRelease, GlobalJobClaim, JobClaim, MaintenanceClaimError, MaintenanceJobKey,
    JOB_KEY_DAILY, JOB_KEY_DIGEST, JOB_KEY_ICS, JOB_KEY_MAGIC, JOB_KEY_NOTIFICATIONS,
    JOB_KEY_PROCESSED, JOB_KEY_REVISIONS, JOB_KEY_UPLOADS, JOB_KEY_WORKSPACE, JOB_LOCK_NAMESPACE,
};
pub use documents::{
    run_document_trash_purge, run_document_trash_purge_with, DocumentPurgeLimits,
    DocumentPurgeStats, DOCUMENT_PURGE_BATCH, DOCUMENT_PURGE_TIME_BUDGET,
};
pub use retention::{
    run_integration_gc, run_notification_gc, run_processed_gc, GC_DELETE_BATCH, GC_DELETE_ROUNDS,
    NOTIFICATION_ARCHIVED_RETENTION_DAYS, NOTIFICATION_READ_RETENTION_DAYS,
    PROCESSED_GC_WINDOW_DAYS,
};
pub use revisions::{
    run_automatic_revision_gc, run_revision_maintenance_batch, run_revision_maintenance_sweep,
    RevisionMaintenanceEngine, RevisionMaintenanceParams, RevisionMaintenanceResume,
    RevisionMaintenanceStats, REVISION_GC_ROUNDS, SCHEDULED_REVISION_TARGET_BATCH,
    WORKSPACE_SCAN_BATCH,
};
pub use tokens::{run_ics_token_gc, run_magic_token_gc, TOKEN_GC_BATCH};
#[cfg(test)]
pub(crate) use uploads::run_stale_upload_gc_claimed_backend;
pub use uploads::{
    run_stale_upload_gc, run_stale_upload_gc_backend, StaleUploadGcStats, UPLOAD_GC_BATCH,
};
pub use withdrawn::run_withdrawn_anonymize;
pub use workspace::{
    run_workspace_purge, WorkspacePurgeStats, WORKSPACE_PURGE_AFTER_DAYS, WORKSPACE_PURGE_BATCH,
};

/// Journal rows reclaimed per upload-GC run (source `OBJECT_CLEANUP_BATCH`).
pub const OBJECT_CLEANUP_BATCH: i64 = 100;

/// Internal consumer outcomes preserve failed COMMIT/finish sources rather
/// than treating an uncertain write or an external send as confirmed progress.
#[derive(Debug, thiserror::Error)]
pub enum MaintenanceConsumerError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    CommitUnknown(#[from] crate::db::backend::CommitCleanupUnknown),
    #[error(transparent)]
    Claim(#[from] Box<claim::MaintenanceClaimError>),
    #[error("owned maintenance adapter stopped: {source}")]
    AdapterStopped {
        #[source]
        source: sqlx::Error,
        /// The producer cannot confirm cleanup/settlement. Keep its original
        /// typed error, and do not issue a release writer after this outcome.
        abandon_claim: bool,
    },
    #[error("maintenance finish failed after {source}: {finish}")]
    FinishAfterFailure {
        #[source]
        source: Box<MaintenanceConsumerError>,
        finish: Box<MaintenanceConsumerError>,
    },
    #[error("maintenance unit cancelled before confirmed finish")]
    Cancelled,
    #[error("maintenance proof is wrong, expired or no longer current")]
    OwnershipLost,
    #[error("scheduled revision current source refused: {0:?}")]
    RevisionRefused(crate::db::revisions::RevisionDbError),
    #[error("digest SMTP settled but database finish is unconfirmed")]
    DigestFinishUnconfirmed {
        #[source]
        source: Box<MaintenanceConsumerError>,
        /// None means SMTP acknowledged acceptance; Some retains its actual
        /// failed/unknown send outcome. Neither confirms the database finish.
        smtp_error: Option<crate::mail::MailSendError>,
    },
}

impl From<claim::MaintenanceClaimError> for MaintenanceConsumerError {
    fn from(error: claim::MaintenanceClaimError) -> Self {
        Self::Claim(Box::new(error))
    }
}

impl MaintenanceConsumerError {
    fn claim_error(&self) -> Option<&claim::MaintenanceClaimError> {
        match self {
            Self::Claim(error) => Some(error.as_ref()),
            _ => None,
        }
    }

    pub(crate) fn stops_on_backend(&self, backend: &crate::db::backend::Backend) -> bool {
        // This value is returned only after the target writer's acknowledged
        // rollback. A failed rollback wraps it and is never a healthy skip.
        if matches!(self, Self::RevisionRefused(_)) {
            return false;
        }
        self.stops_sweep() || matches!(backend, crate::db::backend::Backend::LibsqlRemote(_))
    }

    fn confirmed_shutdown(&self, cancel: &CancellationToken) -> bool {
        if !cancel.is_cancelled() {
            return false;
        }
        match self {
            Self::Cancelled => true,
            Self::AdapterStopped {
                source: sqlx::Error::AnyDriverError(source),
                abandon_claim: false,
            } => {
                matches!(
                    source.downcast_ref::<crate::db::attachments::UploadMaintenanceStop>(),
                    Some(crate::db::attachments::UploadMaintenanceStop::Cancelled)
                )
            }
            _ => false,
        }
    }

    pub(crate) fn stops_sweep(&self) -> bool {
        // A confirmed rollback of one ordinary SQL failure can leave later
        // rows healthy. Loss/cancellation/finish uncertainty stops this owner.
        match (self, self.claim_error()) {
            (Self::RevisionRefused(_), _) => false,
            (Self::Database(error), _)
            | (_, Some(claim::MaintenanceClaimError::Database(error))) => {
                if crate::db::backend::is_rollback_cleanup_unknown(error) {
                    return true;
                }
                let mut source: Option<&(dyn std::error::Error + 'static)> = Some(error);
                while let Some(error) = source {
                    if error.is::<crate::db::backend::CommitUnknown>()
                        || error.is::<crate::db::backend::CommitCleanupUnknown>()
                        || error.is::<crate::db::backend::RemoteSettlementUnconfirmed>()
                        || error.is::<claim::MaintenanceClaimError>()
                        || error.is::<crate::db::attachments::UploadMaintenanceStop>()
                    {
                        return true;
                    }
                    source = error.source();
                }
                false
            }
            _ => true,
        }
    }

    pub(crate) fn remote_settlement_unconfirmed(&self) -> bool {
        use crate::db::backend::{
            CommitCleanupUnknown, CommitSettlement, RemoteSettlementUnconfirmed,
        };
        match (self, self.claim_error()) {
            (Self::FinishAfterFailure { source, finish }, _) => {
                source.remote_settlement_unconfirmed() || finish.remote_settlement_unconfirmed()
            }
            (Self::CommitUnknown(error), _) => {
                error.settlement == CommitSettlement::RemoteUnconfirmed
            }
            (_, Some(claim::MaintenanceClaimError::CommitUnknown { settlement, .. })) => {
                *settlement == CommitSettlement::RemoteUnconfirmed
            }
            (Self::DigestFinishUnconfirmed { source, .. }, _) => {
                source.remote_settlement_unconfirmed()
            }
            (Self::Database(error), _) => {
                let mut source: Option<&(dyn std::error::Error + 'static)> = Some(error);
                while let Some(error) = source {
                    if error.is::<RemoteSettlementUnconfirmed>()
                        || error
                            .downcast_ref::<CommitCleanupUnknown>()
                            .is_some_and(|unknown| {
                                unknown.settlement == CommitSettlement::RemoteUnconfirmed
                            })
                        || matches!(
                            error.downcast_ref::<claim::MaintenanceClaimError>(),
                            Some(claim::MaintenanceClaimError::CommitUnknown {
                                settlement: CommitSettlement::RemoteUnconfirmed,
                                ..
                            })
                        )
                    {
                        return true;
                    }
                    source = error.source();
                }
                false
            }
            _ => false,
        }
    }

    pub(crate) fn cleanup_unconfirmed(&self) -> bool {
        match (self, self.claim_error()) {
            (Self::FinishAfterFailure { source, finish }, _) => {
                source.cleanup_unconfirmed() || finish.cleanup_unconfirmed()
            }
            (Self::AdapterStopped { abandon_claim, .. }, _) => *abandon_claim,
            (_, Some(claim::MaintenanceClaimError::CommitUnknown { cleanup_error, .. })) => {
                cleanup_error.is_some()
            }
            (Self::DigestFinishUnconfirmed { source, .. }, _) => source.cleanup_unconfirmed(),
            (Self::CommitUnknown(error), _) => error.cleanup_error.is_some(),
            (Self::Database(error), _)
            | (_, Some(claim::MaintenanceClaimError::Database(error))) => {
                if crate::db::backend::is_rollback_cleanup_unknown(error) {
                    return true;
                }
                let mut source: Option<&(dyn std::error::Error + 'static)> = Some(error);
                while let Some(error) = source {
                    if error
                        .downcast_ref::<crate::db::backend::CommitCleanupUnknown>()
                        .is_some_and(|unknown| unknown.cleanup_error.is_some())
                        || matches!(
                            error.downcast_ref::<claim::MaintenanceClaimError>(),
                            Some(claim::MaintenanceClaimError::CommitUnknown {
                                cleanup_error: Some(_),
                                ..
                            })
                        )
                    {
                        return true;
                    }
                    source = error.source();
                }
                false
            }
            _ => false,
        }
    }

    fn abandon_claim(&self, backend: &crate::db::backend::Backend) -> bool {
        if let Self::FinishAfterFailure { source, finish } = self {
            return source.abandon_claim(backend) || finish.abandon_claim(backend);
        }
        if self.cleanup_unconfirmed() || self.remote_settlement_unconfirmed() {
            return true;
        }
        // A producer exposing only the original CommitUnknown has no positive
        // remote settlement receipt. Neither a fresh BEGIN nor lease release
        // may be used as evidence that its original stream finished.
        if matches!(backend, crate::db::backend::Backend::LibsqlRemote(_)) {
            match self {
                Self::Database(_) | Self::Claim(_) | Self::AdapterStopped { .. } => true,
                Self::DigestFinishUnconfirmed { source, .. } => source.abandon_claim(backend),
                _ => false,
            }
        } else {
            false
        }
    }
}

/// Metadata only: no system/tenant grant and no nested BEGIN. Consumers borrow
/// their actual writer through business checks/effects and final proof check.
pub(crate) async fn renew_maintenance_writer(
    tx: &mut crate::db::backend::DbTx,
    proof: &claim::FamilyMaintenanceProof,
    key: claim::MaintenanceJobKey,
    policy: claim::FamilyMaintenanceLeasePolicy,
    cancel: &CancellationToken,
) -> Result<(), MaintenanceConsumerError> {
    if cancel.is_cancelled() {
        return Err(MaintenanceConsumerError::Cancelled);
    }
    if !tx
        .operation()
        .check_family_maintenance_claim(proof, key)
        .await?
        || tx
            .operation()
            .renew_family_maintenance_claim(proof, key, policy)
            .await?
            .is_none()
    {
        return Err(MaintenanceConsumerError::OwnershipLost);
    }
    if cancel.is_cancelled() {
        return Err(MaintenanceConsumerError::Cancelled);
    }
    Ok(())
}

pub(crate) async fn rollback_maintenance_writer(
    tx: crate::db::backend::DbTx,
    source: MaintenanceConsumerError,
) -> MaintenanceConsumerError {
    match tx.rollback().await {
        Ok(()) => source,
        Err(cleanup) => MaintenanceConsumerError::Database(
            crate::db::backend::rollback_cleanup_unknown(Some(Box::new(source)), cleanup),
        ),
    }
}

pub(crate) async fn commit_maintenance_writer(
    mut tx: crate::db::backend::DbTx,
    proof: &claim::FamilyMaintenanceProof,
    key: claim::MaintenanceJobKey,
    cancel: &CancellationToken,
) -> Result<(), MaintenanceConsumerError> {
    // Do not renew a late/expired unit into apparent success.
    let result = async {
        #[cfg(test)]
        maintenance_test_hooks::before_finish(proof).await;
        #[cfg(test)]
        maintenance_test_hooks::commit_fault(&mut tx, proof).await?;
        if cancel.is_cancelled() {
            return Err(MaintenanceConsumerError::Cancelled);
        }
        if !tx
            .operation()
            .check_family_maintenance_claim(proof, key)
            .await?
        {
            return Err(MaintenanceConsumerError::OwnershipLost);
        }
        if cancel.is_cancelled() {
            return Err(MaintenanceConsumerError::Cancelled);
        }
        Ok(())
    }
    .await;
    if let Err(err) = result {
        return Err(rollback_maintenance_writer(tx, err).await);
    }
    tx.commit_with_cleanup()
        .await
        .map_err(MaintenanceConsumerError::CommitUnknown)
}

// These three maintenance enumerations end an actual borrowed read. A failed
// explicit rollback is settlement uncertainty even when the read itself worked.
fn finish_maintenance_enumeration(
    result: Result<Vec<uuid::Uuid>, sqlx::Error>,
    cleanup: Result<(), sqlx::Error>,
) -> Result<Vec<uuid::Uuid>, sqlx::Error> {
    match cleanup {
        Ok(()) => result,
        Err(cleanup) => {
            let original = result
                .err()
                .map(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>);
            Err(crate::db::backend::rollback_cleanup_unknown(
                original, cleanup,
            ))
        }
    }
}

#[cfg(test)]
pub(crate) mod maintenance_test_hooks {
    use super::*;
    use crate::db::backend::{DbTx, OperationTx};
    use crate::db::codec::Cell;
    use std::sync::{LazyLock, Mutex};
    use uuid::Uuid;
    static FAULTS: LazyLock<Mutex<Vec<FamilyMaintenanceProof>>> =
        LazyLock::new(|| Mutex::new(Vec::new()));
    type Barrier = (
        tokio::sync::oneshot::Sender<()>,
        tokio::sync::oneshot::Receiver<()>,
    );
    static BARRIERS: LazyLock<Mutex<Vec<(FamilyMaintenanceProof, Barrier)>>> =
        LazyLock::new(|| Mutex::new(Vec::new()));
    static DIGEST_BARRIERS: LazyLock<Mutex<Vec<(FamilyMaintenanceProof, Barrier)>>> =
        LazyLock::new(|| Mutex::new(Vec::new()));
    type EnumerationFault = (
        FamilyMaintenanceProof,
        &'static str,
        Option<Uuid>,
        tokio::sync::oneshot::Sender<Vec<Uuid>>,
    );
    static ENUMERATION_FAULTS: LazyLock<Mutex<Vec<EnumerationFault>>> =
        LazyLock::new(|| Mutex::new(Vec::new()));
    pub(crate) fn arm_enumeration_cleanup(
        proof: &FamilyMaintenanceProof,
        phase: &'static str,
        workspace: Option<Uuid>,
    ) -> tokio::sync::oneshot::Receiver<Vec<Uuid>> {
        let (observed, rx) = tokio::sync::oneshot::channel();
        let mut faults = ENUMERATION_FAULTS.lock().unwrap();
        assert!(!faults
            .iter()
            .any(|(p, s, w, _)| p == proof && *s == phase && *w == workspace));
        faults.push((proof.clone(), phase, workspace, observed));
        rx
    }
    pub(super) fn enumeration_cleanup(
        proof: &FamilyMaintenanceProof,
        phase: &'static str,
        workspace: Option<Uuid>,
        result: &Result<Vec<Uuid>, sqlx::Error>,
        cleanup: Result<(), sqlx::Error>,
    ) -> Result<(), sqlx::Error> {
        let observed = {
            let mut faults = ENUMERATION_FAULTS.lock().unwrap();
            faults
                .iter()
                .position(|(p, s, w, _)| p == proof && *s == phase && *w == workspace)
                .map(|position| faults.remove(position).3)
        };
        match (observed, result, cleanup) {
            (Some(observed), Ok(ids), Ok(())) => {
                // Real enumeration and real local rollback have acknowledged.
                // This labelled synthetic returned error tests propagation;
                // it does not simulate/prove an actual provider cleanup loss.
                observed.send(ids.clone()).unwrap();
                Err(sqlx::Error::Protocol(
                    "synthetic enumeration cleanup error after acknowledged real rollback".into(),
                ))
            }
            (_, _, cleanup) => cleanup,
        }
    }
    pub(crate) async fn wait_reached<T: std::fmt::Debug>(
        reached: tokio::sync::oneshot::Receiver<()>,
        job: &mut tokio::task::JoinHandle<T>,
    ) {
        tokio::time::timeout(Duration::from_secs(2), async {
            tokio::select! {
                result = reached => result.expect("actual writer/effect boundary must be reached"),
                result = job => panic!("maintenance unit ended before its required actual boundary: {result:?}"),
            }
        }).await.expect("actual maintenance boundary must be reachable");
    }
    pub(crate) fn arm_after_digest_send(
        proof: &FamilyMaintenanceProof,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (reached, rx) = tokio::sync::oneshot::channel();
        let (proceed, go) = tokio::sync::oneshot::channel();
        let mut barriers = DIGEST_BARRIERS.lock().unwrap();
        assert!(!barriers.iter().any(|(value, _)| value == proof));
        barriers.push((proof.clone(), (reached, go)));
        (rx, proceed)
    }
    pub(crate) async fn after_digest_send(proof: &FamilyMaintenanceProof) {
        let barrier = {
            let mut barriers = DIGEST_BARRIERS.lock().unwrap();
            barriers
                .iter()
                .position(|(value, _)| value == proof)
                .map(|position| barriers.remove(position).1)
        };
        if let Some((reached, go)) = barrier {
            reached.send(()).unwrap();
            go.await.unwrap();
        }
    }
    pub(crate) fn arm_before_finish(
        proof: &FamilyMaintenanceProof,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (reached, rx) = tokio::sync::oneshot::channel();
        let (proceed, go) = tokio::sync::oneshot::channel();
        let mut barriers = BARRIERS.lock().unwrap();
        assert!(!barriers.iter().any(|(value, _)| value == proof));
        barriers.push((proof.clone(), (reached, go)));
        (rx, proceed)
    }
    pub(super) async fn before_finish(proof: &FamilyMaintenanceProof) {
        let barrier = {
            let mut barriers = BARRIERS.lock().unwrap();
            barriers
                .iter()
                .position(|(value, _)| value == proof)
                .map(|position| barriers.remove(position).1)
        };
        if let Some((reached, go)) = barrier {
            reached.send(()).unwrap();
            go.await.unwrap();
        }
    }
    pub(crate) fn arm_commit_fault(proof: &FamilyMaintenanceProof) {
        let mut faults = FAULTS.lock().unwrap();
        assert!(!faults.contains(proof));
        faults.push(proof.clone());
    }
    pub(super) async fn commit_fault(
        tx: &mut DbTx,
        proof: &FamilyMaintenanceProof,
    ) -> Result<(), sqlx::Error> {
        let armed = {
            let mut faults = FAULTS.lock().unwrap();
            match faults.iter().position(|value| value == proof) {
                Some(position) => {
                    faults.remove(position);
                    true
                }
                None => false,
            }
        };
        if armed {
            let OperationTx::SqliteFamily(writer) = tx.operation() else {
                return Err(sqlx::Error::Protocol(
                    "commit fault needs actual SQLite writer".into(),
                ));
            };
            writer.require_writer()?;
            writer.execute("PRAGMA defer_foreign_keys=ON", &[]).await?;
            // The same owning writer's genuine FK violation reaches COMMIT.
            writer
                .execute(
                    "INSERT INTO memberships(workspace_id,user_id,role) VALUES(?1,?2,'guest')",
                    &[Cell::uuid(Uuid::now_v7()), Cell::uuid(Uuid::now_v7())],
                )
                .await?;
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod family_maintenance_fixture {
    use super::*;
    use crate::db::backend::Backend;
    use uuid::Uuid;
    pub(crate) struct Fixture {
        pub backend: Backend,
        pub pool: sqlx::SqlitePool,
        pub workspace: Uuid,
        pub user: Uuid,
        pub root: std::path::PathBuf,
    }
    impl Fixture {
        pub(crate) async fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("fvoci-maintenance-consumer-{}", Uuid::now_v7()));
            std::fs::create_dir(&root).unwrap();
            let file = root.join("app.sqlite");
            crate::db::migrate::run_sqlite_migrations(&file)
                .await
                .unwrap();
            let pool = crate::db::pool::connect_sqlite_app(&file, 1).await.unwrap();
            let backend = Backend::Sqlite(pool.clone());
            let gate = crate::db::migrate::assert_sqlite_schema_current(&backend)
                .await
                .unwrap();
            assert_eq!(
                gate.applied_steps,
                crate::db::migrate::compiled_sqlite_steps().len()
            );
            let fk: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(fk, 1);
            let keys: Vec<(i64, i64)> = sqlx::query_as(
                "SELECT job_key,generation FROM maintenance_job_claims ORDER BY job_key",
            )
            .fetch_all(&pool)
            .await
            .unwrap();
            assert_eq!(keys, (1..=9).map(|key| (key, 0)).collect::<Vec<_>>());
            let workspace = Uuid::now_v7();
            let user = Uuid::now_v7();
            sqlx::query("INSERT INTO users(id,email,given_name) VALUES(?1,'maintenance@fixture.invalid','maintenance')").bind(user.as_bytes().as_slice()).execute(&pool).await.unwrap();
            sqlx::query(
                "INSERT INTO workspaces(id,slug,name) VALUES(?1,'maintenance-test','maintenance')",
            )
            .bind(workspace.as_bytes().as_slice())
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query("INSERT INTO memberships(workspace_id,user_id,role) VALUES(?1,?2,'owner')")
                .bind(workspace.as_bytes().as_slice())
                .bind(user.as_bytes().as_slice())
                .execute(&pool)
                .await
                .unwrap();
            Self {
                backend,
                pool,
                workspace,
                user,
                root,
            }
        }
        pub(crate) async fn document(&self, workspace: Uuid) -> Uuid {
            let id = Uuid::now_v7();
            let number: i64 = sqlx::query_scalar(
                "SELECT coalesce(max(number),0)+1 FROM documents WHERE workspace_id=?1",
            )
            .bind(workspace.as_bytes().as_slice())
            .fetch_one(&self.pool)
            .await
            .unwrap();
            sqlx::query("INSERT INTO documents(id,workspace_id,title,path,sort_key,number,status,schema_version,created_by,content_json) VALUES(?1,?2,'maintenance',?3,'V',?4,'published',2,?5,'{}')")
                .bind(id.as_bytes().as_slice()).bind(workspace.as_bytes().as_slice())
                .bind(id.simple().to_string()).bind(number).bind(self.user.as_bytes().as_slice())
                .execute(&self.pool).await.unwrap();
            id
        }

        pub(crate) async fn stored_attachment(
            &self,
            workspace: Uuid,
            document: Uuid,
            key: &str,
            preview: Option<&str>,
        ) -> Uuid {
            let id = Uuid::now_v7();
            let variants = preview
                .map(|key| serde_json::json!({"preview":{"key":key}}))
                .unwrap_or_else(|| serde_json::json!({}));
            sqlx::query("INSERT INTO attachments(id,workspace_id,document_id,uploader_id,status,name,mime,size_bytes,reserved_size_bytes,storage_key,image,completed_at,variants) VALUES(?1,?2,?3,?4,'stored','maintenance.png','image/png',4,4,?5,1,1,?6)")
                .bind(id.as_bytes().as_slice()).bind(workspace.as_bytes().as_slice()).bind(document.as_bytes().as_slice())
                .bind(self.user.as_bytes().as_slice()).bind(key).bind(variants.to_string()).execute(&self.pool).await.unwrap();
            id
        }

        pub(crate) async fn other_workspace(&self) -> Uuid {
            let id = Uuid::now_v7();
            sqlx::query("INSERT INTO workspaces(id,slug,name) VALUES(?1,?2,'other')")
                .bind(id.as_bytes().as_slice())
                .bind(id.simple().to_string())
                .execute(&self.pool)
                .await
                .unwrap();
            id
        }

        pub(crate) async fn finish(self) {
            self.backend.close().await.unwrap();
            std::fs::remove_dir_all(self.root).unwrap();
        }
    }
    pub(crate) fn policy() -> FamilyMaintenanceLeasePolicy {
        FamilyMaintenanceLeasePolicy::new(Duration::from_secs(300), Duration::from_secs(60))
            .unwrap()
    }
    pub(crate) async fn acquired(
        request: &claim::FamilyMaintenanceClaimRequest,
        backend: &Backend,
    ) -> claim::FamilyMaintenanceClaim {
        match claim::GlobalJobClaim::try_claim(
            backend,
            request,
            policy(),
            &CancellationToken::new(),
        )
        .await
        .unwrap()
        {
            claim::GlobalClaimAcquisition::Acquired(claim::GlobalJobClaim::Family(claim)) => claim,
            _ => panic!("actual family acquisition required"),
        }
    }
}

const DEFAULT_TICK: Duration = Duration::from_secs(60);
const DEFAULT_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const DEFAULT_REVISION_SWEEP_INTERVAL: Duration = Duration::from_secs(60 * 60);
const DEFAULT_UPLOAD_GC_INTERVAL: Duration = Duration::from_secs(10 * 60);
const DEFAULT_UPLOAD_INCOMPLETE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone)]
pub struct MaintenanceSettings {
    pub tick: Duration,
    /// Cadence of the daily sweep.
    pub interval: Duration,
    /// Cadence of scheduled revision snapshots + automatic retention (source hourly compact bundle).
    pub revision_sweep_interval: Duration,
    /// Cadence of the abandoned-upload cleanup.
    pub upload_gc_interval: Duration,
    /// Incomplete uploads older than this are removed
    /// (`UPLOAD_INCOMPLETE_TTL_HOURS`, validated in `Config`).
    pub upload_incomplete_ttl: Duration,
    pub revision: RevisionMaintenanceParams,
}

impl Default for MaintenanceSettings {
    fn default() -> Self {
        Self {
            tick: DEFAULT_TICK,
            interval: DEFAULT_INTERVAL,
            revision_sweep_interval: DEFAULT_REVISION_SWEEP_INTERVAL,
            upload_gc_interval: DEFAULT_UPLOAD_GC_INTERVAL,
            upload_incomplete_ttl: DEFAULT_UPLOAD_INCOMPLETE_TTL,
            revision: RevisionMaintenanceParams {
                settings: crate::config::RevisionSettings::default(),
                engine: None,
            },
        }
    }
}

impl MaintenanceSettings {
    pub fn from_env(upload_incomplete_ttl: Duration, revision: RevisionMaintenanceParams) -> Self {
        Self {
            upload_gc_interval: Duration::from_secs(parse_positive_u64(
                "FVOCI_UPLOAD_GC_INTERVAL_SECS",
                std::env::var("FVOCI_UPLOAD_GC_INTERVAL_SECS")
                    .ok()
                    .as_deref(),
                DEFAULT_UPLOAD_GC_INTERVAL.as_secs(),
            )),
            upload_incomplete_ttl,
            tick: Duration::from_secs(parse_positive_u64(
                "FVOCI_MAINTENANCE_TICK_SECS",
                std::env::var("FVOCI_MAINTENANCE_TICK_SECS").ok().as_deref(),
                DEFAULT_TICK.as_secs(),
            )),
            interval: Duration::from_secs(parse_positive_u64(
                "FVOCI_MAINTENANCE_INTERVAL_SECS",
                std::env::var("FVOCI_MAINTENANCE_INTERVAL_SECS")
                    .ok()
                    .as_deref(),
                DEFAULT_INTERVAL.as_secs(),
            )),
            revision_sweep_interval: Duration::from_secs(parse_positive_u64(
                "FVOCI_REVISION_SWEEP_INTERVAL_SECS",
                std::env::var("FVOCI_REVISION_SWEEP_INTERVAL_SECS")
                    .ok()
                    .as_deref(),
                DEFAULT_REVISION_SWEEP_INTERVAL.as_secs(),
            )),
            revision,
        }
    }
}

fn parse_positive_u64(name: &str, raw: Option<&str>, default: u64) -> u64 {
    let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return default;
    };
    match raw.parse::<u64>() {
        Ok(value) if value > 0 => value,
        _ => {
            warn!(name, raw, "invalid maintenance duration; using default");
            default
        }
    }
}

pub struct MaintenanceHandle {
    cancel: CancellationToken,
    join: tokio::task::JoinHandle<Result<(), String>>,
    // The caller retains acquisition identity even if the selected task stops
    // with uncertainty; the task cannot consume/recreate its prepared owners.
    _family_requests: Option<Arc<FamilyMaintenanceRequests>>,
}

struct FamilyMaintenanceRequests {
    uploads: claim::FamilyMaintenanceClaimRequest,
    revisions: claim::FamilyMaintenanceClaimRequest,
    daily: claim::FamilyMaintenanceClaimRequest,
}

// Observations belong to the existing private server log. No environment knob,
// file writer, lease mutation or failure handling is added to the scheduler.
#[cfg(feature = "db-tests")]
fn e2e_maintenance_receipt(
    key: MaintenanceJobKey,
    owner: String,
    generation: Option<i64>,
    outcome: &str,
) {
    info!(
        "FVOCI_E2E_MAINTENANCE_RECEIPT {}",
        serde_json::json!({
            "schema": 1, "pid": std::process::id(), "key": key as i32,
            "ownerSha256": owner, "generation": generation.map(|n| n.to_string()), "outcome": outcome
        })
    );
}

impl MaintenanceHandle {
    /// Observation only. The existing owned join consumes the actual result;
    /// completion is not a successful settlement or a remote stream receipt.
    pub fn is_finished(&self) -> bool {
        self.join.is_finished()
    }

    pub fn request_shutdown(&self) {
        self.cancel.cancel();
    }

    pub async fn join(self) -> Result<(), String> {
        self.join
            .await
            .map_err(|err| format!("maintenance scheduler join failed: {err}"))?
    }
}

pub fn spawn_maintenance(
    settings: MaintenanceSettings,
    pool: PgPool,
    storage: ObjectStorage,
    mailer: Arc<Mailer>,
) -> MaintenanceHandle {
    let cancel = CancellationToken::new();
    let child = cancel.clone();
    let join = tokio::spawn(async move {
        run_maintenance_loop(settings, pool, storage, mailer, child).await;
        Ok(())
    });
    MaintenanceHandle {
        cancel,
        join,
        _family_requests: None,
    }
}

/// Selected startup entry. PostgreSQL keeps its dedicated-session scheduler;
/// family startup receives an explicit, validated lease policy from main.
pub fn spawn_maintenance_backend(
    settings: MaintenanceSettings,
    backend: crate::db::backend::Backend,
    storage: ObjectStorage,
    mailer: Arc<Mailer>,
    family_policy: FamilyMaintenanceLeasePolicy,
) -> MaintenanceHandle {
    if let crate::db::backend::Backend::Postgres(pool) = backend {
        return spawn_maintenance(settings, pool, storage, mailer);
    }
    let requests = Arc::new(FamilyMaintenanceRequests {
        uploads: claim::FamilyMaintenanceClaimRequest::new(MaintenanceJobKey::Uploads),
        revisions: claim::FamilyMaintenanceClaimRequest::new(MaintenanceJobKey::Revisions),
        daily: claim::FamilyMaintenanceClaimRequest::new(MaintenanceJobKey::Daily),
    });
    #[cfg(feature = "db-tests")]
    for request in [&requests.uploads, &requests.revisions, &requests.daily] {
        e2e_maintenance_receipt(request.key(), request.e2e_owner_sha256(), None, "prepared");
    }
    let retained = requests.clone();
    let cancel = CancellationToken::new();
    let child = cancel.clone();
    let join = tokio::spawn(async move {
        // Classify original typed uncertainty inside the selected loop. Only
        // its final, already-stopped outcome becomes the public drain string.
        run_family_maintenance_loop(
            settings,
            backend,
            storage,
            mailer,
            family_policy,
            requests,
            child,
        )
        .await
        .map_err(maintenance_failure_report)
    });
    MaintenanceHandle {
        cancel,
        join,
        _family_requests: Some(retained),
    }
}

fn maintenance_failure_report(error: MaintenanceConsumerError) -> String {
    // Reporting happens only after the actual typed stop decision. Keep the
    // original driver/cleanup chain visible in the public drain diagnostic.
    let mut report = error.to_string();
    let mut source = std::error::Error::source(&error);
    while let Some(error) = source {
        report.push_str(": ");
        report.push_str(&error.to_string());
        if let Some(receipt) = error.downcast_ref::<crate::db::backend::RollbackCleanupUnknown>() {
            if let Some(original) = &receipt.original {
                report.push_str("; original refusal: ");
                report.push_str(&original.to_string());
            }
        }
        source = error.source();
    }
    report
}

fn family_maintenance_now() -> chrono::DateTime<Utc> {
    // This is our scheduler-generated instant, not a wire/user timestamp.
    // Family storage's declared precision is microseconds; caller-provided
    // cutoff codecs still reject finer inputs instead of silently rounding.
    chrono::DateTime::from_timestamp_micros(Utc::now().timestamp_micros())
        .expect("current UTC clock is a representable microsecond instant")
}

async fn run_family_maintenance_loop(
    settings: MaintenanceSettings,
    backend: crate::db::backend::Backend,
    storage: ObjectStorage,
    mailer: Arc<Mailer>,
    policy: FamilyMaintenanceLeasePolicy,
    requests: Arc<FamilyMaintenanceRequests>,
    cancel: CancellationToken,
) -> Result<(), MaintenanceConsumerError> {
    let mut last_daily = None;
    let mut last_revision_sweep = None;
    let mut last_upload_gc = None;
    let mut upload_gc_cursor = None;
    let mut revision_resume = RevisionMaintenanceResume::default();
    let mut ticker = tokio::time::interval(settings.tick);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return Ok(()),
            _ = ticker.tick() => {}
        }
        // Original order/cadences and resume advancement after known success.
        if is_due(last_upload_gc, settings.upload_gc_interval) {
            match run_stale_upload_sweep_family(
                &backend,
                &storage,
                settings.upload_incomplete_ttl,
                upload_gc_cursor,
                &requests.uploads,
                policy,
                &cancel,
            )
            .await
            {
                Ok(Some(stats)) => {
                    last_upload_gc = Some(Instant::now());
                    upload_gc_cursor = stats.resume_after;
                }
                Ok(None) => {}
                Err(error) if error.confirmed_shutdown(&cancel) => return Ok(()),
                Err(error) if error.stops_on_backend(&backend) => return Err(error),
                Err(error) => warn!(%error, "maintenance.upload_gc_failed"),
            }
        }
        if cancel.is_cancelled() {
            return Ok(());
        }
        if is_due(last_revision_sweep, settings.revision_sweep_interval) {
            match run_revision_sweep_family(
                &backend,
                &settings.revision,
                revision_resume,
                &requests.revisions,
                policy,
                &cancel,
            )
            .await
            {
                Ok(Some((stats, resume))) => {
                    if resume.sweep_complete() {
                        last_revision_sweep = Some(Instant::now());
                        revision_resume = RevisionMaintenanceResume::default();
                    } else {
                        revision_resume = resume;
                    }
                    if stats.snapshots_created > 0
                        || stats.snapshots_deduped > 0
                        || stats.revisions_deleted > 0
                    {
                        info!(
                            snapshots_created = stats.snapshots_created,
                            snapshots_deduped = stats.snapshots_deduped,
                            snapshots_skipped = stats.snapshots_skipped,
                            snapshots_failed = stats.snapshots_failed,
                            revisions_deleted = stats.revisions_deleted,
                            "maintenance.revision_sweep"
                        );
                    }
                }
                Ok(None) => {}
                Err(error) if error.confirmed_shutdown(&cancel) => return Ok(()),
                Err(error) if error.stops_on_backend(&backend) => return Err(error),
                Err(error) => warn!(%error, "maintenance.revision_sweep_failed"),
            }
        }
        if cancel.is_cancelled() {
            return Ok(());
        }
        if is_due(last_daily, settings.interval) {
            match run_daily_sweep_family(
                &backend,
                &storage,
                &mailer,
                &requests.daily,
                policy,
                &cancel,
            )
            .await
            {
                Ok(Some(_)) => last_daily = Some(Instant::now()),
                Ok(None) => {}
                Err(error) if error.confirmed_shutdown(&cancel) => return Ok(()),
                Err(error) if error.stops_on_backend(&backend) => return Err(error),
                Err(error) => warn!(%error, "maintenance.daily_sweep_failed"),
            }
        }
    }
}

async fn acquire_family_claim(
    backend: &crate::db::backend::Backend,
    request: &claim::FamilyMaintenanceClaimRequest,
    expected: MaintenanceJobKey,
    policy: FamilyMaintenanceLeasePolicy,
    cancel: &CancellationToken,
) -> Result<Option<claim::FamilyMaintenanceClaim>, MaintenanceConsumerError> {
    if request.key() != expected || matches!(backend, crate::db::backend::Backend::Postgres(_)) {
        return Err(MaintenanceConsumerError::OwnershipLost);
    }
    let acquisition = claim::GlobalJobClaim::try_claim(backend, request, policy, cancel).await;
    #[cfg(feature = "db-tests")]
    if acquisition.is_err() {
        e2e_maintenance_receipt(
            request.key(),
            request.e2e_owner_sha256(),
            None,
            "acquire-error",
        );
    }
    match acquisition? {
        claim::GlobalClaimAcquisition::Acquired(claim::GlobalJobClaim::Family(owner)) => {
            #[cfg(feature = "db-tests")]
            e2e_maintenance_receipt(
                owner.proof().key(),
                owner.proof().e2e_owner_sha256(),
                Some(owner.proof().generation()),
                "acquired",
            );
            Ok(Some(owner))
        }
        claim::GlobalClaimAcquisition::Acquired(claim::GlobalJobClaim::Postgres(owner)) => {
            owner.release().await;
            Err(MaintenanceConsumerError::OwnershipLost)
        }
        claim::GlobalClaimAcquisition::Busy => {
            #[cfg(feature = "db-tests")]
            e2e_maintenance_receipt(request.key(), request.e2e_owner_sha256(), None, "busy");
            Ok(None)
        }
        claim::GlobalClaimAcquisition::Cancelled => {
            #[cfg(feature = "db-tests")]
            e2e_maintenance_receipt(request.key(), request.e2e_owner_sha256(), None, "cancelled");
            Err(MaintenanceConsumerError::Cancelled)
        }
    }
}

/// The only family-sweep exit which may open a release writer. Unknown remote
/// finish or actual cleanup uncertainty returns before this operation; dropping
/// the proof is not confirmation or release, and prepared owners stay retained.
async fn finish_family_claim(
    backend: &crate::db::backend::Backend,
    owner: claim::FamilyMaintenanceClaim,
    failure: Option<&MaintenanceConsumerError>,
) -> Result<(), MaintenanceConsumerError> {
    #[cfg(feature = "db-tests")]
    let receipt = (
        owner.proof().key(),
        owner.proof().e2e_owner_sha256(),
        owner.proof().generation(),
    );
    if let Some(error) = failure {
        if error.abandon_claim(backend) {
            #[cfg(feature = "db-tests")]
            e2e_maintenance_receipt(receipt.0, receipt.1, Some(receipt.2), "abandoned");
            return Ok(());
        }
    }
    let release = owner.release().await;
    #[cfg(feature = "db-tests")]
    e2e_maintenance_receipt(
        receipt.0,
        receipt.1,
        Some(receipt.2),
        match &release {
            Ok(claim::FamilyLeaseAction::Confirmed) => "released",
            Ok(claim::FamilyLeaseAction::Lost) => "lost",
            Ok(claim::FamilyLeaseAction::Cancelled) => "release-cancelled",
            Err(_) => "release-error",
        },
    );
    match release? {
        claim::FamilyLeaseAction::Confirmed => Ok(()),
        claim::FamilyLeaseAction::Lost => Err(MaintenanceConsumerError::OwnershipLost),
        claim::FamilyLeaseAction::Cancelled => Err(MaintenanceConsumerError::Cancelled),
    }
}

fn upload_consumer_error(
    backend: &crate::db::backend::Backend,
    error: sqlx::Error,
) -> MaintenanceConsumerError {
    // Preserve the producer's boxed type, including private rollback failures.
    // A known LostClaim/Cancelled can finish a settled writer; an opaque driver
    // failure has no confirmed cleanup receipt and must not open release.
    let known_stop = matches!(&error, sqlx::Error::AnyDriverError(source)
        if source.is::<crate::db::attachments::UploadMaintenanceStop>());
    if crate::db::attachments::upload_maintenance_must_stop(&error)
        || matches!(backend, crate::db::backend::Backend::LibsqlRemote(_))
    {
        MaintenanceConsumerError::AdapterStopped {
            abandon_claim: !known_stop,
            source: error,
        }
    } else {
        error.into()
    }
}

async fn run_stale_upload_sweep_family(
    backend: &crate::db::backend::Backend,
    storage: &ObjectStorage,
    ttl: Duration,
    after: Option<crate::db::attachments::StaleUploadCursor>,
    request: &claim::FamilyMaintenanceClaimRequest,
    policy: FamilyMaintenanceLeasePolicy,
    cancel: &CancellationToken,
) -> Result<Option<StaleUploadGcStats>, MaintenanceConsumerError> {
    let Some(owner) =
        acquire_family_claim(backend, request, MaintenanceJobKey::Uploads, policy, cancel).await?
    else {
        return Ok(None);
    };
    let ttl = chrono::Duration::from_std(ttl).unwrap_or(chrono::Duration::MAX);
    let cutoff = family_maintenance_now()
        .checked_sub_signed(ttl)
        .unwrap_or(chrono::DateTime::<Utc>::MIN_UTC);
    let result = async {
        let stats = uploads::run_stale_upload_gc_claimed_backend(
            backend,
            storage,
            cutoff,
            after,
            UPLOAD_GC_BATCH,
            cancel,
            owner.proof(),
            policy,
        )
        .await
        .map_err(|error| upload_consumer_error(backend, error))?;
        // Same Uploads owner: journal drain follows only a known batch finish.
        if !cancel.is_cancelled() {
            match crate::db::attachments::reclaim_attachment_objects_claimed_backend(
                backend,
                storage,
                None,
                OBJECT_CLEANUP_BATCH,
                owner.proof(),
                policy,
                cancel,
            )
            .await
            {
                Ok(cleanup) if cleanup.claimed > 0 => info!(
                    claimed = cleanup.claimed,
                    reclaimed = cleanup.reclaimed,
                    busy = cleanup.busy,
                    failed = cleanup.failed,
                    "maintenance.attachment_object_cleanup"
                ),
                Ok(_) => {}
                Err(error) => {
                    let error = upload_consumer_error(backend, error);
                    if error.stops_on_backend(backend) {
                        return Err(error);
                    }
                    warn!(%error, "maintenance.attachment_object_cleanup_failed");
                }
            }
        }
        if stats.claimed > 0 {
            info!(
                claimed = stats.claimed,
                purged = stats.purged,
                failed = stats.failed,
                "maintenance.upload_gc"
            );
        }
        Ok::<_, MaintenanceConsumerError>(stats)
    }
    .await;
    if let Err(finish) = finish_family_claim(backend, owner, result.as_ref().err()).await {
        return Err(match result {
            Err(source) => MaintenanceConsumerError::FinishAfterFailure {
                source: Box::new(source),
                finish: Box::new(finish),
            },
            Ok(_) => finish,
        });
    }
    result.map(Some)
}

async fn run_revision_sweep_family(
    backend: &crate::db::backend::Backend,
    params: &RevisionMaintenanceParams,
    resume: RevisionMaintenanceResume,
    request: &claim::FamilyMaintenanceClaimRequest,
    policy: FamilyMaintenanceLeasePolicy,
    cancel: &CancellationToken,
) -> Result<Option<(RevisionMaintenanceStats, RevisionMaintenanceResume)>, MaintenanceConsumerError>
{
    let Some(owner) = acquire_family_claim(
        backend,
        request,
        MaintenanceJobKey::Revisions,
        policy,
        cancel,
    )
    .await?
    else {
        return Ok(None);
    };
    let result = revisions::run_revision_maintenance_batch_family(
        backend,
        params,
        resume,
        owner.proof(),
        policy,
        cancel,
    )
    .await;
    if let Err(finish) = finish_family_claim(backend, owner, result.as_ref().err()).await {
        return Err(match result {
            Err(source) => MaintenanceConsumerError::FinishAfterFailure {
                source: Box::new(source),
                finish: Box::new(finish),
            },
            Ok(_) => finish,
        });
    }
    result.map(Some)
}

async fn run_daily_sweep_family(
    backend: &crate::db::backend::Backend,
    storage: &ObjectStorage,
    mailer: &Mailer,
    request: &claim::FamilyMaintenanceClaimRequest,
    policy: FamilyMaintenanceLeasePolicy,
    cancel: &CancellationToken,
) -> Result<Option<DailySweepStats>, MaintenanceConsumerError> {
    let Some(owner) =
        acquire_family_claim(backend, request, MaintenanceJobKey::Daily, policy, cancel).await?
    else {
        return Ok(None);
    };
    let result =
        run_daily_jobs_family(backend, storage, mailer, owner.proof(), policy, cancel).await;
    if let Err(finish) = finish_family_claim(backend, owner, result.as_ref().err()).await {
        return Err(match result {
            Err(source) => MaintenanceConsumerError::FinishAfterFailure {
                source: Box::new(source),
                finish: Box::new(finish),
            },
            Ok(_) => finish,
        });
    }
    result.map(Some)
}

async fn run_daily_jobs_family(
    backend: &crate::db::backend::Backend,
    storage: &ObjectStorage,
    mailer: &Mailer,
    proof: &FamilyMaintenanceProof,
    policy: FamilyMaintenanceLeasePolicy,
    cancel: &CancellationToken,
) -> Result<DailySweepStats, MaintenanceConsumerError> {
    let now = family_maintenance_now();
    let mut stats = DailySweepStats::default();
    // Keep the original order, including withdrawal before personal-workspace
    // purge and orphan import before digest. Ordinary settled SQL failures
    // isolate a leaf; uncertainty/current-proof loss stops before the next.
    if !cancel.is_cancelled() {
        match withdrawn::run_withdrawn_anonymize_family(backend, now, proof, policy, cancel).await {
            Ok(erased) => {
                info!(erased, "maintenance.withdrawn_anonymize");
                stats.withdrawn_anonymized = erased;
            }
            Err(error) if error.stops_on_backend(backend) => return Err(error),
            Err(error) => warn!(%error, "maintenance.withdrawn_anonymize_failed"),
        }
    }
    if !cancel.is_cancelled() {
        match workspace::run_workspace_purge_family(backend, storage, now, proof, policy, cancel)
            .await
        {
            Ok(workspace) => {
                info!(
                    purged = workspace.purged,
                    storage_deleted = workspace.storage_deleted,
                    skipped = workspace.skipped,
                    "maintenance.workspace_purge"
                );
                stats.workspace = workspace;
            }
            Err(error) if error.stops_on_backend(backend) => return Err(error),
            Err(error) => warn!(%error, "maintenance.workspace_purge_failed"),
        }
    }
    if !cancel.is_cancelled() {
        match documents::run_document_trash_purge_family(
            backend,
            storage,
            DocumentPurgeLimits::default(),
            proof,
            policy,
            cancel,
        )
        .await
        {
            Ok(documents) => {
                info!(
                    purged = documents.purged,
                    storage_deleted = documents.storage_deleted,
                    skipped = documents.skipped,
                    failed = documents.failed,
                    deferred = documents.deferred,
                    "maintenance.document_purge"
                );
                stats.documents = documents;
            }
            Err(error) if error.stops_on_backend(backend) => return Err(error),
            Err(error) => warn!(%error, "maintenance.document_purge_failed"),
        }
    }
    if !cancel.is_cancelled() {
        match tokens::run_token_gc_family(
            backend,
            now,
            proof,
            policy,
            cancel,
            tokens::TokenCleanup::Ics,
        )
        .await
        {
            Ok(deleted) => {
                info!(deleted, "maintenance.ics_token_gc");
                stats.ics_deleted = deleted;
            }
            Err(error) if error.stops_on_backend(backend) => return Err(error),
            Err(error) => warn!(%error, "maintenance.ics_token_gc_failed"),
        }
    }
    if !cancel.is_cancelled() {
        match tokens::run_token_gc_family(
            backend,
            now,
            proof,
            policy,
            cancel,
            tokens::TokenCleanup::Magic,
        )
        .await
        {
            Ok(deleted) => {
                info!(deleted, "maintenance.magic_token_gc");
                stats.magic_deleted = deleted;
            }
            Err(error) if error.stops_on_backend(backend) => return Err(error),
            Err(error) => warn!(%error, "maintenance.magic_token_gc_failed"),
        }
    }
    if !cancel.is_cancelled() {
        match retention::run_notification_gc_family(backend, proof, policy, cancel).await {
            Ok((read, archived)) => {
                info!(read, archived, "maintenance.notification_gc");
                stats.notifications_read = read;
                stats.notifications_archived = archived;
            }
            Err(error) if error.stops_on_backend(backend) => return Err(error),
            Err(error) => warn!(%error, "maintenance.notification_gc_failed"),
        }
    }
    if !cancel.is_cancelled() {
        match retention::run_processed_gc_family(backend, proof, policy, cancel).await {
            Ok(deleted) => {
                info!(deleted, "maintenance.processed_gc");
                stats.processed = deleted;
            }
            Err(error) if error.stops_on_backend(backend) => return Err(error),
            Err(error) => warn!(%error, "maintenance.processed_gc_failed"),
        }
    }
    if !cancel.is_cancelled() {
        match retention::run_integration_gc_family(backend, proof, policy, cancel).await {
            Ok((webhook, github)) => {
                info!(webhook, github, "maintenance.integration_gc");
                stats.webhook_deliveries = webhook;
                stats.github_deliveries = github;
            }
            Err(error) if error.stops_on_backend(backend) => return Err(error),
            Err(error) => warn!(%error, "maintenance.integration_gc_failed"),
        }
    }
    if !cancel.is_cancelled() {
        match crate::import_job::sweep_orphan_imports_with_maintenance_claim_backend(
            backend, storage, cancel, proof, policy,
        )
        .await
        {
            Ok(swept) => {
                info!(swept, "maintenance.import_sweep");
                stats.imports_swept = swept;
            }
            Err(error)
                if crate::import_job::import_database_error_stops_scheduler(backend, &error) =>
            {
                let abandon_claim = matches!(backend, crate::db::backend::Backend::LibsqlRemote(_))
                    || matches!(error, sqlx::Error::AnyDriverError(_));
                return Err(MaintenanceConsumerError::AdapterStopped {
                    source: error,
                    abandon_claim,
                });
            }
            Err(error) => warn!(%error, "maintenance.import_sweep_failed"),
        }
    }
    if !cancel.is_cancelled() {
        match crate::mail::digest::send_due_digests_maintenance_backend(
            backend, mailer, now, proof, policy, cancel,
        )
        .await
        {
            Ok(sent) => {
                info!(sent, "maintenance.digest");
                stats.digests_sent = sent;
            }
            Err(error) if error.stops_on_backend(backend) => return Err(error),
            Err(error) => warn!(%error, "maintenance.digest_failed"),
        }
    }
    Ok(stats)
}

async fn run_maintenance_loop(
    settings: MaintenanceSettings,
    pool: PgPool,
    storage: ObjectStorage,
    mailer: Arc<Mailer>,
    cancel: CancellationToken,
) {
    let mut last_daily: Option<Instant> = None;
    let mut last_revision_sweep: Option<Instant> = None;
    let mut last_upload_gc: Option<Instant> = None;
    let mut upload_gc_cursor = None;
    let mut revision_resume = RevisionMaintenanceResume::default();
    let revision_params = settings.revision.clone();
    let mut ticker = tokio::time::interval(settings.tick);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = ticker.tick() => {
                if is_due(last_upload_gc, settings.upload_gc_interval) {
                    match run_stale_upload_sweep(
                        &pool,
                        &storage,
                        settings.upload_incomplete_ttl,
                        upload_gc_cursor,
                        &cancel,
                    )
                    .await
                    {
                        Ok(Some(stats)) => {
                            last_upload_gc = Some(Instant::now());
                            upload_gc_cursor = stats.resume_after;
                        }
                        Ok(None) => {}
                        Err(err) => warn!(error = %err, "maintenance.upload_gc_failed"),
                    }
                }
                if cancel.is_cancelled() {
                    return;
                }
                if is_due(last_revision_sweep, settings.revision_sweep_interval) {
                    match run_revision_maintenance_sweep(
                        &pool,
                        &revision_params,
                        revision_resume,
                        &cancel,
                    )
                    .await
                    {
                        Ok(Some((stats, resume))) => {
                            if resume.sweep_complete() {
                                last_revision_sweep = Some(Instant::now());
                                revision_resume = RevisionMaintenanceResume::default();
                            } else {
                                revision_resume = resume;
                            }
                            if stats.snapshots_created > 0
                                || stats.snapshots_deduped > 0
                                || stats.revisions_deleted > 0
                            {
                                info!(
                                    snapshots_created = stats.snapshots_created,
                                    snapshots_deduped = stats.snapshots_deduped,
                                    snapshots_skipped = stats.snapshots_skipped,
                                    snapshots_failed = stats.snapshots_failed,
                                    revisions_deleted = stats.revisions_deleted,
                                    "maintenance.revision_sweep"
                                );
                            }
                        }
                        Ok(None) => {}
                        Err(err) => warn!(error = %err, "maintenance.revision_sweep_failed"),
                    }
                }
                if cancel.is_cancelled() {
                    return;
                }
                if is_due(last_daily, settings.interval) {
                    match run_daily_sweep(&pool, &storage, &mailer, &cancel).await {
                        Ok(Some(_)) => last_daily = Some(Instant::now()),
                        Ok(None) => {}
                        Err(err) => warn!(error = %err, "maintenance.daily_sweep_failed"),
                    }
                }
            }
        }
    }
}

fn is_due(last: Option<Instant>, interval: Duration) -> bool {
    last.is_none_or(|started| started.elapsed() >= interval)
}

/// Claim the upload cleanup lock and remove one bounded batch of incomplete
/// uploads older than `ttl`, resuming after `after` (the previous batch's
/// `resume_after`). `None` means another process holds the lock.
pub async fn run_stale_upload_sweep(
    pool: &PgPool,
    storage: &ObjectStorage,
    ttl: Duration,
    after: Option<crate::db::attachments::StaleUploadCursor>,
    cancel: &CancellationToken,
) -> Result<Option<StaleUploadGcStats>, sqlx::Error> {
    let Some(claim) = JobClaim::try_claim(pool, JOB_KEY_UPLOADS).await? else {
        return Ok(None);
    };
    let ttl = chrono::Duration::from_std(ttl).unwrap_or(chrono::Duration::MAX);
    let cutoff = Utc::now()
        .checked_sub_signed(ttl)
        .unwrap_or(chrono::DateTime::<Utc>::MIN_UTC);
    let result = run_stale_upload_gc(pool, storage, cutoff, after, UPLOAD_GC_BATCH, cancel).await;
    // Same claim: drain the object journal written by attachment deletes.
    let cleanup = if cancel.is_cancelled() {
        Ok(Default::default())
    } else {
        crate::db::attachments::reclaim_attachment_objects(
            pool,
            storage,
            None,
            OBJECT_CLEANUP_BATCH,
        )
        .await
    };
    claim.release().await;
    match cleanup {
        Ok(cleanup) if cleanup.claimed > 0 => info!(
            claimed = cleanup.claimed,
            reclaimed = cleanup.reclaimed,
            busy = cleanup.busy,
            failed = cleanup.failed,
            "maintenance.attachment_object_cleanup"
        ),
        Ok(_) => {}
        Err(err) => warn!(error = %err, "maintenance.attachment_object_cleanup_failed"),
    }
    let stats = result?;
    if stats.claimed > 0 {
        info!(
            claimed = stats.claimed,
            purged = stats.purged,
            failed = stats.failed,
            "maintenance.upload_gc"
        );
    }
    Ok(Some(stats))
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DailySweepStats {
    pub withdrawn_anonymized: u32,
    pub workspace: WorkspacePurgeStats,
    pub documents: DocumentPurgeStats,
    pub ics_deleted: u32,
    pub magic_deleted: u32,
    pub notifications_read: u32,
    pub notifications_archived: u32,
    pub processed: u32,
    pub digests_sent: u32,
    pub imports_swept: u32,
    pub webhook_deliveries: u32,
    pub github_deliveries: u32,
}

/// Claim the daily sweep lock, run every job, then release. `None` means
/// another process holds the lock.
pub async fn run_daily_sweep(
    pool: &PgPool,
    storage: &ObjectStorage,
    mailer: &Mailer,
    cancel: &CancellationToken,
) -> Result<Option<DailySweepStats>, sqlx::Error> {
    let Some(claim) = JobClaim::try_claim(pool, JOB_KEY_DAILY).await? else {
        return Ok(None);
    };
    let result = run_daily_jobs(pool, storage, mailer, cancel).await;
    claim.release().await;
    result.map(Some)
}

async fn run_daily_jobs(
    pool: &PgPool,
    storage: &ObjectStorage,
    mailer: &Mailer,
    cancel: &CancellationToken,
) -> Result<DailySweepStats, sqlx::Error> {
    let now = Utc::now();
    let mut stats = DailySweepStats::default();

    // Source order: anonymize withdrawn users first so their personal
    // workspace is marked deleted before the purge below removes it.
    if !cancel.is_cancelled() {
        match run_withdrawn_anonymize(pool, now, cancel).await {
            Ok(erased) => {
                info!(erased, "maintenance.withdrawn_anonymize");
                stats.withdrawn_anonymized = erased;
            }
            Err(err) => warn!(error = %err, "maintenance.withdrawn_anonymize_failed"),
        }
    }

    if !cancel.is_cancelled() {
        match run_workspace_purge(pool, storage, now, cancel).await {
            Ok(workspace) => {
                info!(
                    purged = workspace.purged,
                    storage_deleted = workspace.storage_deleted,
                    skipped = workspace.skipped,
                    "maintenance.workspace_purge"
                );
                stats.workspace = workspace;
            }
            Err(err) => warn!(error = %err, "maintenance.workspace_purge_failed"),
        }
    }

    // Source `purgeTrashedDocuments`: 30-day document trash retention.
    if !cancel.is_cancelled() {
        match run_document_trash_purge(pool, storage, cancel).await {
            Ok(documents) => {
                info!(
                    purged = documents.purged,
                    storage_deleted = documents.storage_deleted,
                    skipped = documents.skipped,
                    failed = documents.failed,
                    deferred = documents.deferred,
                    "maintenance.document_purge"
                );
                stats.documents = documents;
            }
            Err(err) => warn!(error = %err, "maintenance.document_purge_failed"),
        }
    }

    if !cancel.is_cancelled() {
        match run_ics_token_gc(pool, now, cancel).await {
            Ok(deleted) => {
                info!(deleted, "maintenance.ics_token_gc");
                stats.ics_deleted = deleted;
            }
            Err(err) => warn!(error = %err, "maintenance.ics_token_gc_failed"),
        }
    }

    if !cancel.is_cancelled() {
        match run_magic_token_gc(pool, now, cancel).await {
            Ok(deleted) => {
                info!(deleted, "maintenance.magic_token_gc");
                stats.magic_deleted = deleted;
            }
            Err(err) => warn!(error = %err, "maintenance.magic_token_gc_failed"),
        }
    }

    if !cancel.is_cancelled() {
        match run_notification_gc(pool, cancel).await {
            Ok((read, archived)) => {
                info!(read, archived, "maintenance.notification_gc");
                stats.notifications_read = read;
                stats.notifications_archived = archived;
            }
            Err(err) => warn!(error = %err, "maintenance.notification_gc_failed"),
        }
    }

    if !cancel.is_cancelled() {
        match run_processed_gc(pool, cancel).await {
            Ok(deleted) => {
                info!(deleted, "maintenance.processed_gc");
                stats.processed = deleted;
            }
            Err(err) => warn!(error = %err, "maintenance.processed_gc_failed"),
        }
    }

    if !cancel.is_cancelled() {
        match run_integration_gc(pool, cancel).await {
            Ok((webhook, github)) => {
                info!(webhook, github, "maintenance.integration_gc");
                stats.webhook_deliveries = webhook;
                stats.github_deliveries = github;
            }
            Err(err) => warn!(error = %err, "maintenance.integration_gc_failed"),
        }
    }

    // Source `sweepOrphanImports` runs in the same daily sweep, with storage
    // available because compensation may delete attachment objects.
    if !cancel.is_cancelled() {
        match crate::import_job::sweep_orphan_imports(pool, storage, cancel).await {
            Ok(swept) => {
                info!(swept, "maintenance.import_sweep");
                stats.imports_swept = swept;
            }
            Err(err) => warn!(error = %err, "maintenance.import_sweep_failed"),
        }
    }

    if !cancel.is_cancelled() {
        match crate::mail::send_due_digests(pool, mailer, now, cancel).await {
            Ok(sent) => {
                info!(sent, "maintenance.digest");
                stats.digests_sent = sent;
            }
            Err(err) => warn!(error = %err, "maintenance.digest_failed"),
        }
    }

    Ok(stats)
}

#[cfg(test)]
mod selected_scheduler_tests {
    use super::*;
    use family_maintenance_fixture::{policy, Fixture};
    use uuid::Uuid;

    async fn upload(f: &Fixture, created_at: i64) -> (Uuid, String) {
        let document = Uuid::now_v7();
        sqlx::query("INSERT INTO documents(id,workspace_id,title,path,sort_key,number,status,schema_version,created_by,content_json) VALUES(?1,?2,'maintenance',?3,'V',?4,'published',2,?5,'{}')")
            .bind(document.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice())
            .bind(document.simple().to_string()).bind(created_at).bind(f.user.as_bytes().as_slice())
            .execute(&f.pool).await.unwrap();
        let attachment = Uuid::now_v7();
        let key = Uuid::now_v7().to_string();
        sqlx::query("INSERT INTO attachments(id,workspace_id,document_id,uploader_id,status,name,reserved_size_bytes,storage_key,created_at) VALUES(?1,?2,?3,?4,'uploading','maintenance.txt',4,?5,?6)")
            .bind(attachment.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice())
            .bind(document.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).bind(&key)
            .bind(created_at).execute(&f.pool).await.unwrap();
        (attachment, key)
    }

    async fn attachment_count(f: &Fixture) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM attachments")
            .fetch_one(&f.pool)
            .await
            .unwrap()
    }

    async fn finished(handle: &MaintenanceHandle) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while !handle.is_finished() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("owned selected scheduler must settle and expose its result");
    }

    fn start(f: &Fixture, storage: ObjectStorage) -> MaintenanceHandle {
        spawn_maintenance_backend(
            MaintenanceSettings::default(),
            f.backend.clone(),
            storage,
            Arc::new(Mailer::disabled()),
            policy(),
        )
    }

    #[tokio::test]
    async fn selected_scheduler_upload_late_cancel_retains_row_then_known_cleanup_progress() {
        let f = Fixture::new().await;
        let storage = ObjectStorage::local(f.root.join("storage"));
        let (attachment, key) = upload(&f, 1).await;
        storage.put_bytes(&key, b"part".to_vec()).await.unwrap();
        let (settled, go) = crate::db::attachments::cleanup_test_hooks::arm(attachment, 1);
        let handle = start(&f, storage.clone());
        let requests = handle._family_requests.as_ref().unwrap().clone();
        tokio::time::timeout(Duration::from_secs(2), settled)
            .await
            .unwrap()
            .unwrap();
        // Actual purge settled while the same writer still owns the DB row.
        // Shutdown must not publish DB deletion or start revisions/Daily.
        handle.request_shutdown();
        go.send(()).unwrap();
        finished(&handle).await;
        handle.join().await.unwrap();
        assert_eq!(attachment_count(&f).await, 1);
        assert_eq!(storage.head(&key).await.unwrap(), None);
        let skipped: Vec<(i64,i64)> = sqlx::query_as("SELECT job_key,generation FROM maintenance_job_claims WHERE job_key IN(1,9) ORDER BY job_key")
            .fetch_all(&f.pool).await.unwrap();
        assert_eq!(skipped, vec![(1, 0), (9, 0)]);
        // The original caller retains its actual prepared identity. A new
        // known batch handles an already absent object without a false ACK.
        let result = run_stale_upload_sweep_family(
            &f.backend,
            &storage,
            DEFAULT_UPLOAD_INCOMPLETE_TTL,
            None,
            &requests.uploads,
            policy(),
            &CancellationToken::new(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            (
                result.claimed,
                result.purged,
                result.failed,
                result.resume_after
            ),
            (1, 1, 0, None)
        );
        assert_eq!(attachment_count(&f).await, 0);
        assert_eq!(storage.head(&key).await.unwrap(), None);
        let owner: (Option<Vec<u8>>, i64) = sqlx::query_as(
            "SELECT owner_token,generation FROM maintenance_job_claims WHERE job_key=8",
        )
        .fetch_one(&f.pool)
        .await
        .unwrap();
        assert_eq!(owner, (None, 2));
        f.finish().await;
    }

    #[tokio::test]
    async fn selected_scheduler_real_upload_commit_fk_failure_stops_before_release_or_next_effect()
    {
        let f = Fixture::new().await;
        let storage = ObjectStorage::local(f.root.join("storage"));
        let (first, first_key) = upload(&f, 1).await;
        let (_, next_key) = upload(&f, 2).await;
        for key in [&first_key, &next_key] {
            storage.put_bytes(key, b"part".to_vec()).await.unwrap();
        }
        // A real canonical deferred FK supplies the negative COMMIT oracle.
        // The injected trigger adds a collection with an absent parent only
        // after this actual DELETE; FK=1 and SQL execution are never mocked.
        let connector = Uuid::now_v7();
        sqlx::query("INSERT INTO zotero_connectors(id,workspace_id,owner_user_id,library_type,remote_library_id,library_url) VALUES(?1,?2,?3,'user',1,'https://fixture.invalid')")
            .bind(connector.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice())
            .execute(&f.pool).await.unwrap();
        let trigger = format!("CREATE TRIGGER maintenance_upload_commit_fault AFTER DELETE ON attachments WHEN OLD.id=X'{}' BEGIN INSERT INTO zotero_collections(workspace_id,owner_user_id,connector_id,collection_key,remote_version,name,parent_key) VALUES(OLD.workspace_id,OLD.uploader_id,X'{}','ABCDEFGH',1,'COMMIT fault','JKLMNPQR'); END",first.simple(),connector.simple());
        sqlx::query(&trigger).execute(&f.pool).await.unwrap();
        let handle = start(&f, storage.clone());
        let requests = handle._family_requests.as_ref().unwrap().clone();
        finished(&handle).await;
        let error = handle
            .join()
            .await
            .expect_err("actual failed writer must be reported, never success");
        assert!(error.contains("maintenance adapter stopped"));
        assert!(
            error.contains("787"),
            "must be the original real FK COMMIT failure: {error}"
        );
        assert!(error.contains("FOREIGN KEY constraint failed"));
        // Fresh actual SQLite/storage reads establish what settled. This is
        // local evidence only, never an original remote-stream receipt.
        assert_eq!(attachment_count(&f).await, 2);
        assert_eq!(storage.head(&first_key).await.unwrap(), None);
        assert_eq!(storage.read_range(&next_key, 0, 3).await.unwrap(), b"part");
        let claims: Vec<(i64,Option<Vec<u8>>,i64)> = sqlx::query_as("SELECT job_key,owner_token,generation FROM maintenance_job_claims WHERE job_key IN(1,8,9) ORDER BY job_key")
            .fetch_all(&f.pool).await.unwrap();
        assert_eq!(claims[0], (1, None, 0));
        assert_eq!(claims[2], (9, None, 0));
        assert_eq!(claims[1].0, 8);
        assert!(
            claims[1].1.is_some(),
            "uncertain producer must not open a release writer"
        );
        assert_eq!(claims[1].2, 1);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM zotero_collections")
                .fetch_one(&f.pool)
                .await
                .unwrap(),
            0
        );
        sqlx::query("DROP TRIGGER maintenance_upload_commit_fault")
            .execute(&f.pool)
            .await
            .unwrap();
        crate::db::migrate::assert_sqlite_schema_current(&f.backend)
            .await
            .unwrap();
        // Only the explicit test operator expires the stopped owner's row;
        // the consumer never fabricates finish/reconciliation or resets gen.
        sqlx::query("UPDATE maintenance_job_claims SET expires_at=0 WHERE job_key=8")
            .execute(&f.pool)
            .await
            .unwrap();
        let stats = run_stale_upload_sweep_family(
            &f.backend,
            &storage,
            DEFAULT_UPLOAD_INCOMPLETE_TTL,
            None,
            &requests.uploads,
            policy(),
            &CancellationToken::new(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!((stats.claimed, stats.purged, stats.failed), (2, 2, 0));
        assert_eq!(attachment_count(&f).await, 0);
        assert_eq!(storage.head(&next_key).await.unwrap(), None);
        let generation: i64 =
            sqlx::query_scalar("SELECT generation FROM maintenance_job_claims WHERE job_key=8")
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert_eq!(generation, 2);
        f.finish().await;
    }

    #[tokio::test]
    async fn selected_scheduler_wrong_upload_request_has_no_claim_or_storage_effect() {
        let f = Fixture::new().await;
        let storage = ObjectStorage::local(f.root.join("storage"));
        let (_, key) = upload(&f, 1).await;
        storage.put_bytes(&key, b"part".to_vec()).await.unwrap();
        let wrong = claim::FamilyMaintenanceClaimRequest::new(MaintenanceJobKey::Daily);
        assert!(matches!(
            run_stale_upload_sweep_family(
                &f.backend,
                &storage,
                DEFAULT_UPLOAD_INCOMPLETE_TTL,
                None,
                &wrong,
                policy(),
                &CancellationToken::new()
            )
            .await,
            Err(MaintenanceConsumerError::OwnershipLost)
        ));
        assert_eq!(attachment_count(&f).await, 1);
        assert_eq!(storage.read_range(&key, 0, 3).await.unwrap(), b"part");
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT sum(generation) FROM maintenance_job_claims")
                .fetch_one(&f.pool)
                .await
                .unwrap(),
            0
        );
        f.finish().await;
    }
}

#[cfg(test)]
mod enumeration_finish_tests {
    use super::*;
    use family_maintenance_fixture::{acquired, policy, Fixture};
    use uuid::Uuid;

    #[tokio::test]
    async fn maintenance_enumeration_synthetic_after_real_ack_stops_loop_effects_release_then_healthy_progress(
    ) {
        for phase in ["workspaces", "document-workspaces", "workspace-documents"] {
            let f = Fixture::new().await;
            let second = f.other_workspace().await;
            let control = f.other_workspace().await;
            sqlx::query("INSERT INTO memberships(workspace_id,user_id,role) VALUES(?1,?2,'owner')")
                .bind(control.as_bytes().as_slice())
                .bind(f.user.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
            let storage = ObjectStorage::local(f.root.join("storage"));
            let mut documents = Vec::new();
            let mut keys = Vec::new();
            for workspace in [f.workspace, second] {
                let document = f.document(workspace).await;
                let key = Uuid::now_v7().to_string();
                f.stored_attachment(workspace, document, &key, None).await;
                storage
                    .put_bytes(&key, b"enumeration-literal-untouched".to_vec())
                    .await
                    .unwrap();
                let expired = family_maintenance_now() - chrono::Duration::days(31);
                sqlx::query("UPDATE documents SET deleted_at=?2 WHERE id=?1")
                    .bind(document.as_bytes().as_slice())
                    .bind(expired.timestamp_micros())
                    .execute(&f.pool)
                    .await
                    .unwrap();
                if phase == "workspaces" {
                    sqlx::query("UPDATE workspaces SET deleted_at=?2 WHERE id=?1")
                        .bind(workspace.as_bytes().as_slice())
                        .bind(expired.timestamp_micros())
                        .execute(&f.pool)
                        .await
                        .unwrap();
                }
                documents.push(document);
                keys.push(key);
            }
            let token = Uuid::now_v7();
            sqlx::query("INSERT INTO ics_tokens(id,workspace_id,user_id,token_hash,expires_at) VALUES(?1,?2,?3,?4,1)")
                .bind(token.as_bytes().as_slice()).bind(control.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice())
                .bind(token.to_string()).execute(&f.pool).await.unwrap();
            // Caller preparation precedes every BEGIN. The actual S16 owner
            // admits replay of this SAME live identity without a new generation.
            let requests = Arc::new(FamilyMaintenanceRequests {
                uploads: FamilyMaintenanceClaimRequest::new(MaintenanceJobKey::Uploads),
                revisions: FamilyMaintenanceClaimRequest::new(MaintenanceJobKey::Revisions),
                daily: FamilyMaintenanceClaimRequest::new(MaintenanceJobKey::Daily),
            });
            let owner = acquired(&requests.daily, &f.backend).await;
            let first_workspace = std::cmp::min(f.workspace, second);
            let fault_workspace = (phase == "workspace-documents").then_some(first_workspace);
            let observed = maintenance_test_hooks::arm_enumeration_cleanup(
                owner.proof(),
                phase,
                fault_workspace,
            );
            let settings = MaintenanceSettings {
                tick: Duration::from_millis(10),
                interval: Duration::from_millis(10),
                upload_gc_interval: Duration::from_millis(10),
                revision_sweep_interval: Duration::from_millis(10),
                ..MaintenanceSettings::default()
            };
            let cancel = CancellationToken::new();
            let mut job = tokio::spawn(run_family_maintenance_loop(
                settings,
                f.backend.clone(),
                storage.clone(),
                Arc::new(Mailer::disabled()),
                policy(),
                requests.clone(),
                cancel.clone(),
            ));
            let result = match tokio::time::timeout(Duration::from_secs(2), &mut job).await {
                Ok(result) => result.unwrap(),
                Err(_) => {
                    cancel.cancel();
                    let _ = job.await;
                    panic!("typed enumeration uncertainty must stop the actual loop before another tick");
                }
            };
            let error = result.expect_err("cleanup uncertainty cannot become successful drain");
            let MaintenanceConsumerError::Database(sqlx::Error::AnyDriverError(source)) = &error
            else {
                panic!("canonical read cleanup receipt must survive the actual loop: {error}");
            };
            let receipt = source
                .downcast_ref::<crate::db::backend::RollbackCleanupUnknown>()
                .unwrap();
            assert!(
                receipt.original.is_none(),
                "successful enumeration has no fabricated original refusal"
            );
            assert!(matches!(&receipt.cleanup, sqlx::Error::Protocol(message)
                if message == "synthetic enumeration cleanup error after acknowledged real rollback"));
            assert!(error.stops_sweep());
            assert!(error.cleanup_unconfirmed());
            let listed = observed
                .await
                .expect("actual successful enumeration and real rollback ACK required");
            if phase == "workspace-documents" {
                let first_document = documents[usize::from(first_workspace == second)];
                assert_eq!(listed, vec![first_document]);
            } else {
                assert!(listed.contains(&f.workspace) && listed.contains(&second));
            }
            for (document, key) in documents.iter().zip(&keys) {
                let present: i64 = sqlx::query_scalar("SELECT count(*) FROM documents WHERE id=?1")
                    .bind(document.as_bytes().as_slice())
                    .fetch_one(&f.pool)
                    .await
                    .unwrap();
                assert_eq!(present, 1, "first and second workspace remain untouched");
                assert_eq!(
                    storage.read_range(key, 0, 28).await.unwrap(),
                    b"enumeration-literal-untouched"
                );
            }
            let tokens: i64 = sqlx::query_scalar("SELECT count(*) FROM ics_tokens WHERE id=?1")
                .bind(token.as_bytes().as_slice())
                .fetch_one(&f.pool)
                .await
                .unwrap();
            assert_eq!(
                tokens, 1,
                "next Daily token effect cannot run after uncertain read finish"
            );
            let claims: Vec<(i64, i64, Option<Vec<u8>>)> = sqlx::query_as(
                "SELECT job_key,generation,owner_token FROM maintenance_job_claims WHERE job_key IN (1,8,9) ORDER BY job_key")
                .fetch_all(&f.pool).await.unwrap();
            assert_eq!(
                claims
                    .iter()
                    .map(|(key, generation, _)| (*key, *generation))
                    .collect::<Vec<_>>(),
                vec![(1, 1), (8, 1), (9, 1)],
                "no next tick can acquire another upload/revision generation"
            );
            assert!(
                claims[0].2.is_some(),
                "typed uncertainty must abandon before a release writer"
            );
            // Explicit fixture operator recovery follows acknowledged LOCAL
            // rollback plus a synthetic propagation fault, not provider loss.
            sqlx::query("UPDATE maintenance_job_claims SET expires_at=0 WHERE job_key=1")
                .execute(&f.pool)
                .await
                .unwrap();
            let healthy = run_daily_sweep_family(
                &f.backend,
                &storage,
                &Mailer::disabled(),
                &requests.daily,
                policy(),
                &CancellationToken::new(),
            )
            .await
            .unwrap()
            .unwrap();
            if phase == "workspaces" {
                assert_eq!(healthy.workspace.purged, 2);
            } else {
                assert_eq!(healthy.documents.purged, 2);
            }
            assert_eq!(
                healthy.ics_deleted, 1,
                "known healthy read finish permits actual later work"
            );
            for key in &keys {
                assert_eq!(storage.head(key).await.unwrap(), None);
            }
            let claim: (i64, Option<Vec<u8>>) = sqlx::query_as(
                "SELECT generation,owner_token FROM maintenance_job_claims WHERE job_key=1",
            )
            .fetch_one(&f.pool)
            .await
            .unwrap();
            assert_eq!(
                claim,
                (2, None),
                "healthy finish releases the monotonically new owner"
            );
            assert_eq!(owner.release().await.unwrap(), FamilyLeaseAction::Lost);
            crate::db::migrate::assert_sqlite_schema_current(&f.backend)
                .await
                .unwrap();
            f.finish().await;
        }
    }

    #[tokio::test]
    async fn maintenance_enumeration_retains_actual_wrong_tenant_driver_on_synthetic_cleanup_error()
    {
        let f = Fixture::new().await;
        let other = f.other_workspace().await;
        let mut read = f.backend.begin_read().await.unwrap();
        read.operation().set_tenant(f.workspace).await.unwrap();
        let result = read
            .operation()
            .maintenance_expired_documents(other, &[], 200)
            .await;
        assert!(matches!(&result, Err(sqlx::Error::Protocol(_))));
        read.rollback().await.unwrap();
        // Actual wrong-tenant driver refusal and actual rollback ACK precede
        // this explicitly synthetic returned error; no provider claim follows.
        let error = finish_maintenance_enumeration(
            result,
            Err(sqlx::Error::Protocol(
                "synthetic returned cleanup error after real enumeration rollback ACK".into(),
            )),
        )
        .err()
        .unwrap();
        assert!(crate::db::backend::is_rollback_cleanup_unknown(&error));
        let sqlx::Error::AnyDriverError(source) = error else {
            panic!("shared receipt required")
        };
        let receipt = source
            .downcast_ref::<crate::db::backend::RollbackCleanupUnknown>()
            .unwrap();
        assert!(matches!(
            receipt
                .original
                .as_ref()
                .unwrap()
                .downcast_ref::<sqlx::Error>(),
            Some(sqlx::Error::Protocol(_))
        ));
        assert!(matches!(&receipt.cleanup,sqlx::Error::Protocol(message)
            if message=="synthetic returned cleanup error after real enumeration rollback ACK"));
        f.finish().await;
    }
}
