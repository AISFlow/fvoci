//! Family global maintenance ownership. This record does not supply business
//! authority, scheduled consumers, or fencing of remote storage/SMTP effects.
use super::backend::{
    Backend, CommitCleanupUnknown, CommitSettlement, CommitUnknown, FamilyTx, OperationTx,
};
use super::codec::Cell;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Existing advisory-key namespace. Only Daily/Uploads/Revisions are scheduled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum MaintenanceJobKey {
    Daily = 1,
    Workspace = 2,
    Ics = 3,
    Magic = 4,
    Notifications = 5,
    Processed = 6,
    Digest = 7,
    Uploads = 8,
    Revisions = 9,
}

/// No implicit production TTL: callers must choose and validate both intervals.
#[derive(Clone, Copy, Debug)]
pub struct FamilyMaintenanceLeasePolicy {
    lease_us: i64,
    renew_us: i64,
}
impl FamilyMaintenanceLeasePolicy {
    pub fn new(lease: Duration, renew: Duration) -> Result<Self, sqlx::Error> {
        fn checked(duration: Duration) -> Result<i64, sqlx::Error> {
            if duration.is_zero() || !duration.subsec_nanos().is_multiple_of(1000) {
                return Err(invalid(
                    "maintenance duration must be positive whole microseconds",
                ));
            }
            i64::try_from(duration.as_micros())
                .map_err(|_| invalid("maintenance duration exceeds i64 microseconds"))
        }
        let lease_us = checked(lease)?;
        let renew_us = checked(renew)?;
        if renew_us >= lease_us {
            return Err(invalid(
                "maintenance renewal interval must be shorter than lease",
            ));
        }
        Ok(Self { lease_us, renew_us })
    }
    pub fn lease(self) -> Duration {
        Duration::from_micros(self.lease_us as u64)
    }
    pub fn renew_interval(self) -> Duration {
        Duration::from_micros(self.renew_us as u64)
    }
    fn expiry(self, now: i64) -> Result<i64, sqlx::Error> {
        let expiry = now
            .checked_add(self.lease_us)
            .ok_or_else(|| invalid("maintenance expiry overflow"))?;
        if expiry < 0 || chrono::DateTime::from_timestamp_micros(expiry).is_none() {
            return Err(invalid(
                "maintenance expiry exceeds supported instant range",
            ));
        }
        Ok(expiry)
    }
}

/// Opaque capability; constructors and owner/generation fields are private.
/// Clone permits borrowing a proof at successive *current* writer boundaries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FamilyMaintenanceProof {
    key: MaintenanceJobKey,
    owner: Uuid,
    generation: i64,
}
impl FamilyMaintenanceProof {
    /// Test-build observation only; never expose the ownership capability.
    #[cfg(feature = "db-tests")]
    pub fn e2e_owner_sha256(&self) -> String {
        use sha2::Digest;
        hex::encode(sha2::Sha256::digest(self.owner.as_bytes()))
    }
    pub fn key(&self) -> MaintenanceJobKey {
        self.key
    }
    pub fn generation(&self) -> i64 {
        self.generation
    }
}

/// Stable acquisition identity, prepared before BEGIN. Reuse only to reconcile
/// this same attempt; a new logical claimant uses a new request/token.
pub struct FamilyMaintenanceClaimRequest {
    key: MaintenanceJobKey,
    owner: Uuid,
}
impl FamilyMaintenanceClaimRequest {
    /// Hash the exact sixteen stored UUID bytes, not its display form.
    #[cfg(feature = "db-tests")]
    pub fn e2e_owner_sha256(&self) -> String {
        use sha2::Digest;
        hex::encode(sha2::Sha256::digest(self.owner.as_bytes()))
    }
    pub fn new(key: MaintenanceJobKey) -> Self {
        Self {
            key,
            owner: Uuid::now_v7(),
        }
    }
    pub fn key(&self) -> MaintenanceJobKey {
        self.key
    }
    pub async fn try_acquire(
        &self,
        backend: &Backend,
        policy: FamilyMaintenanceLeasePolicy,
        cancel: &CancellationToken,
    ) -> Result<FamilyClaimAcquisition, MaintenanceClaimError> {
        if cancel.is_cancelled() {
            return Ok(FamilyClaimAcquisition::Cancelled);
        }
        let mut tx = backend.begin_write().await?;
        let result = async {
            let mut op = tx.operation();
            op.set_system().await?;
            op.acquire_family_maintenance(self, policy).await
        }
        .await;
        let prepared = match result {
            Ok(Some(prepared)) => prepared,
            Ok(None) => {
                tx.rollback().await?;
                return Ok(FamilyClaimAcquisition::Busy);
            }
            Err(error) => {
                tx.rollback().await?;
                return Err(error.into());
            }
        };
        #[cfg(test)]
        test_hooks::wait(self.owner, 0).await;
        if cancel.is_cancelled() {
            tx.rollback().await?;
            return Ok(FamilyClaimAcquisition::Cancelled);
        }
        #[cfg(test)]
        test_hooks::commit_fault(&mut tx, self.owner, 0).await?;
        if let Err(unknown) = tx.commit_with_cleanup().await {
            if !unknown.permits_reconciliation() {
                return Err(unconfirmed(unknown, None));
            }
            match observe(backend, &prepared.proof).await {
                Ok(row)
                    if row.matches(&prepared.proof)
                        && row.live()
                        && row.expiry == Some(prepared.expiry) => {}
                observation => return Err(unconfirmed(unknown, observation.err())),
            }
        }
        // Cancellation during acknowledged COMMIT cannot erase durable ownership.
        Ok(FamilyClaimAcquisition::Acquired(FamilyMaintenanceClaim {
            backend: backend.clone(),
            proof: prepared.proof,
            policy,
        }))
    }
}

pub enum FamilyClaimAcquisition {
    Acquired(FamilyMaintenanceClaim),
    Busy,
    Cancelled,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FamilyLeaseAction {
    Confirmed,
    Lost,
    Cancelled,
}

#[derive(Debug, thiserror::Error)]
pub enum MaintenanceClaimError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error("maintenance COMMIT is unknown; current ownership was not confirmed")]
    CommitUnknown {
        #[source]
        source: CommitUnknown,
        settlement: CommitSettlement,
        cleanup_error: Option<sqlx::Error>,
        observation_error: Option<sqlx::Error>,
    },
}
fn unconfirmed(
    unknown: CommitCleanupUnknown,
    observation_error: Option<sqlx::Error>,
) -> MaintenanceClaimError {
    MaintenanceClaimError::CommitUnknown {
        source: unknown.source,
        settlement: unknown.settlement,
        cleanup_error: unknown.cleanup_error,
        observation_error,
    }
}

/// Dropping this value does not release a durable row. Explicit finish is
/// awaited; expiry/crash recovery never resets its generation.
#[must_use = "acquired maintenance ownership requires explicit awaited release"]
pub struct FamilyMaintenanceClaim {
    backend: Backend,
    proof: FamilyMaintenanceProof,
    policy: FamilyMaintenanceLeasePolicy,
}
impl FamilyMaintenanceClaim {
    pub fn proof(&self) -> &FamilyMaintenanceProof {
        &self.proof
    }
    pub fn policy(&self) -> FamilyMaintenanceLeasePolicy {
        self.policy
    }
    pub async fn renew(
        &mut self,
        cancel: &CancellationToken,
    ) -> Result<FamilyLeaseAction, MaintenanceClaimError> {
        self.finish(false, cancel).await
    }
    pub async fn release(self) -> Result<FamilyLeaseAction, MaintenanceClaimError> {
        // Shutdown cancellation must not suppress the explicit awaited finish.
        self.finish(true, &CancellationToken::new()).await
    }
    async fn finish(
        &self,
        release: bool,
        cancel: &CancellationToken,
    ) -> Result<FamilyLeaseAction, MaintenanceClaimError> {
        if cancel.is_cancelled() {
            return Ok(FamilyLeaseAction::Cancelled);
        }
        let mut tx = self.backend.begin_write().await?;
        let result = async {
            let mut op = tx.operation();
            op.set_system().await?;
            if release {
                op.release_family_maintenance(&self.proof)
                    .await
                    .map(|ok| ok.then_some(None))
            } else {
                op.renew_family_maintenance_claim(&self.proof, self.proof.key, self.policy)
                    .await
                    .map(|expiry| expiry.map(Some))
            }
        }
        .await;
        let expected_expiry = match result {
            Ok(Some(expiry)) => expiry,
            Ok(None) => {
                tx.rollback().await?;
                return Ok(FamilyLeaseAction::Lost);
            }
            Err(error) => {
                tx.rollback().await?;
                return Err(error.into());
            }
        };
        if cancel.is_cancelled() {
            tx.rollback().await?;
            return Ok(FamilyLeaseAction::Cancelled);
        }
        #[cfg(test)]
        test_hooks::commit_fault(&mut tx, self.proof.owner, if release { 2 } else { 1 }).await?;
        if let Err(unknown) = tx.commit_with_cleanup().await {
            if !unknown.permits_reconciliation() {
                return Err(unconfirmed(unknown, None));
            }
            match observe(&self.backend, &self.proof).await {
                Ok(row)
                    if row.generation == self.proof.generation
                        && if release {
                            row.owner.is_none() && row.expiry.is_none()
                        } else {
                            row.matches(&self.proof) && row.live() && row.expiry == expected_expiry
                        } => {}
                observation => return Err(unconfirmed(unknown, observation.err())),
            }
        }
        Ok(FamilyLeaseAction::Confirmed)
    }
}

struct PreparedClaim {
    proof: FamilyMaintenanceProof,
    expiry: i64,
}
struct ClaimRow {
    owner: Option<Uuid>,
    generation: i64,
    expiry: Option<i64>,
    now: i64,
}
impl ClaimRow {
    fn live(&self) -> bool {
        self.expiry.is_some_and(|expiry| expiry > self.now)
    }
    fn matches(&self, proof: &FamilyMaintenanceProof) -> bool {
        self.owner == Some(proof.owner) && self.generation == proof.generation
    }
}
fn invalid(message: &str) -> sqlx::Error {
    sqlx::Error::Protocol(message.into())
}
fn tuple(proof: &FamilyMaintenanceProof) -> [Cell; 3] {
    [
        Cell::Integer(proof.key as i64),
        Cell::uuid(proof.owner),
        Cell::Integer(proof.generation),
    ]
}

impl OperationTx<'_, '_> {
    fn maintenance_writer(&mut self) -> Result<&mut FamilyTx, sqlx::Error> {
        let Self::SqliteFamily(tx) = self else {
            return Err(invalid(
                "family maintenance proof cannot replace PostgreSQL session ownership",
            ));
        };
        tx.require_writer()?;
        Ok(tx)
    }
    fn maintenance_system_writer(&mut self) -> Result<&mut FamilyTx, sqlx::Error> {
        let tx = self.maintenance_writer()?;
        tx.require_system_context()?;
        if tx.tenant().is_some() {
            return Err(invalid(
                "claim acquisition/release cannot broaden a tenant transaction",
            ));
        }
        Ok(tx)
    }
    async fn maintenance_row(&mut self, key: MaintenanceJobKey) -> Result<ClaimRow, sqlx::Error> {
        let tx = self.maintenance_writer()?;
        let rows = tx.query("SELECT owner_token,generation,expires_at,unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000 FROM maintenance_job_claims WHERE job_key=?1", &[Cell::Integer(key as i64)]).await?;
        let row = rows.first().ok_or_else(|| {
            invalid("compiled maintenance claim row is missing; migration required")
        })?;
        let owner = row.cell(0)?.optional(|cell| cell.id())?;
        let generation = row.cell(1)?.integer()?;
        let expiry = row.cell(2)?.optional(|cell| {
            cell.datetime()?;
            cell.integer()
        })?;
        let now = row.cell(3)?.integer()?;
        if generation < 0
            || owner.is_some() != expiry.is_some()
            || owner.is_some_and(|value| value.is_nil())
            || (owner.is_some() && generation == 0)
            || expiry.is_some_and(|value| value < 0)
            || now < 0
        {
            return Err(invalid("malformed maintenance claim/current DB clock"));
        }
        Ok(ClaimRow {
            owner,
            generation,
            expiry,
            now,
        })
    }
    async fn acquire_family_maintenance(
        &mut self,
        request: &FamilyMaintenanceClaimRequest,
        policy: FamilyMaintenanceLeasePolicy,
    ) -> Result<Option<PreparedClaim>, sqlx::Error> {
        self.maintenance_system_writer()?;
        let row = self.maintenance_row(request.key).await?;
        if row.live() {
            if row.owner == Some(request.owner) {
                return Ok(Some(PreparedClaim {
                    proof: FamilyMaintenanceProof {
                        key: request.key,
                        owner: request.owner,
                        generation: row.generation,
                    },
                    expiry: row.expiry.expect("validated live expiry"),
                }));
            }
            return Ok(None);
        }
        let generation = row
            .generation
            .checked_add(1)
            .ok_or_else(|| invalid("maintenance generation overflow"))?;
        let expiry = policy.expiry(row.now)?;
        let changed = self.maintenance_system_writer()?.execute("UPDATE maintenance_job_claims SET owner_token=?2,generation=?3,expires_at=?4 WHERE job_key=?1 AND generation=?5 AND (owner_token IS NULL OR expires_at<=?6)", &[Cell::Integer(request.key as i64),Cell::uuid(request.owner),Cell::Integer(generation),Cell::Integer(expiry),Cell::Integer(row.generation),Cell::Integer(row.now)]).await?;
        if changed != 1 {
            return Err(invalid(
                "maintenance acquisition lost reserved writer/current row",
            ));
        }
        Ok(Some(PreparedClaim {
            proof: FamilyMaintenanceProof {
                key: request.key,
                owner: request.owner,
                generation,
            },
            expiry,
        }))
    }
    /// Metadata only. Does not set system/tenant or authorize business writes.
    /// Caller keeps this actual writer through effects and repeats before COMMIT.
    pub(crate) async fn check_family_maintenance_claim(
        &mut self,
        proof: &FamilyMaintenanceProof,
        expected_key: MaintenanceJobKey,
    ) -> Result<bool, sqlx::Error> {
        self.maintenance_writer()?;
        if proof.key != expected_key {
            return Ok(false);
        }
        let row = self.maintenance_row(expected_key).await?;
        Ok(row.matches(proof) && row.live())
    }
    /// Same-consumer-writer renewal: no nested BEGIN or second pool. Current
    /// proof is still required, and expired generations are never resurrected.
    pub(crate) async fn renew_family_maintenance_claim(
        &mut self,
        proof: &FamilyMaintenanceProof,
        expected_key: MaintenanceJobKey,
        policy: FamilyMaintenanceLeasePolicy,
    ) -> Result<Option<i64>, sqlx::Error> {
        self.maintenance_writer()?;
        if proof.key != expected_key {
            return Ok(None);
        }
        let row = self.maintenance_row(expected_key).await?;
        if !row.matches(proof) || !row.live() {
            return Ok(None);
        }
        let expiry = policy.expiry(row.now)?;
        let mut args = tuple(proof).to_vec();
        args.extend([Cell::Integer(expiry), Cell::Integer(row.now)]);
        let changed = self.maintenance_writer()?.execute("UPDATE maintenance_job_claims SET expires_at=?4 WHERE job_key=?1 AND owner_token=?2 AND generation=?3 AND expires_at>?5", &args).await?;
        if changed != 1 {
            return Err(invalid(
                "maintenance renewal lost reserved writer/current row",
            ));
        }
        Ok(Some(expiry))
    }
    async fn release_family_maintenance(
        &mut self,
        proof: &FamilyMaintenanceProof,
    ) -> Result<bool, sqlx::Error> {
        self.maintenance_system_writer()?;
        let row = self.maintenance_row(proof.key).await?;
        if !row.matches(proof) || !row.live() {
            return Ok(false);
        }
        let mut args = tuple(proof).to_vec();
        args.push(Cell::Integer(row.now));
        let changed = self.maintenance_system_writer()?.execute("UPDATE maintenance_job_claims SET owner_token=NULL,expires_at=NULL WHERE job_key=?1 AND owner_token=?2 AND generation=?3 AND expires_at>?4", &args).await?;
        Ok(changed == 1)
    }
}

/// Called only after confirmed settlement. Failed remote finishes currently
/// retain explicit uncertainty (public SDK hides Close) and never call this.
/// Local SQLx
/// rollback is queued before connection reuse; this fresh reserved writer also
/// waits for that work. No unbounded retry or effects during observation.
async fn observe(
    backend: &Backend,
    proof: &FamilyMaintenanceProof,
) -> Result<ClaimRow, sqlx::Error> {
    let mut tx = backend.begin_write().await?;
    let result = tx.operation().maintenance_row(proof.key).await;
    tx.rollback().await?;
    result
}

#[cfg(test)]
mod test_hooks {
    use super::*;
    use std::collections::{HashMap, HashSet};
    use std::sync::{LazyLock, Mutex};
    type Pause = (
        tokio::sync::oneshot::Sender<()>,
        tokio::sync::oneshot::Receiver<()>,
    );
    static PAUSES: LazyLock<Mutex<HashMap<(Uuid, u8), Pause>>> = LazyLock::new(Default::default);
    static FAULTS: LazyLock<Mutex<HashSet<(Uuid, u8)>>> = LazyLock::new(Default::default);
    pub(super) fn pause(
        owner: Uuid,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (entered, receive) = tokio::sync::oneshot::channel();
        let (proceed, wait) = tokio::sync::oneshot::channel();
        PAUSES.lock().unwrap().insert((owner, 0), (entered, wait));
        (receive, proceed)
    }
    pub(super) async fn wait(owner: Uuid, stage: u8) {
        let pause = PAUSES.lock().unwrap().remove(&(owner, stage));
        if let Some((entered, proceed)) = pause {
            let _ = entered.send(());
            let _ = proceed.await;
        }
    }
    pub(super) fn fault(owner: Uuid, stage: u8) {
        FAULTS.lock().unwrap().insert((owner, stage));
    }
    pub(super) async fn commit_fault(
        tx: &mut super::super::backend::DbTransaction<'_>,
        owner: Uuid,
        stage: u8,
    ) -> Result<(), sqlx::Error> {
        let armed = FAULTS.lock().unwrap().remove(&(owner, stage));
        if !armed {
            return Ok(());
        }
        let super::super::backend::DbTransaction::SqliteFamily(family) = tx else {
            return Err(invalid("test needs actual family writer"));
        };
        family.require_writer()?;
        family.execute("PRAGMA defer_foreign_keys=ON", &[]).await?;
        // Actual existing FK, not a fabricated COMMIT result or test schema.
        family
            .execute(
                "INSERT INTO memberships(workspace_id,user_id,role) VALUES(?1,?2,'owner')",
                &[Cell::uuid(Uuid::now_v7()), Cell::uuid(Uuid::now_v7())],
            )
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod policy_tests {
    use super::*;
    #[test]
    fn maintenance_claim_policy_rejects_zero_precision_and_overflow() {
        for (lease, renew) in [
            (Duration::ZERO, Duration::from_micros(1)),
            (Duration::from_secs(1), Duration::ZERO),
            (Duration::from_secs(1), Duration::from_secs(1)),
            (Duration::from_secs(1), Duration::from_secs(2)),
            (Duration::from_nanos(1001), Duration::from_micros(1)),
            (Duration::from_secs(u64::MAX), Duration::from_secs(1)),
        ] {
            assert!(FamilyMaintenanceLeasePolicy::new(lease, renew).is_err());
        }
        let policy =
            FamilyMaintenanceLeasePolicy::new(Duration::from_secs(10), Duration::from_secs(1))
                .unwrap();
        assert_eq!(policy.lease(), Duration::from_secs(10));
        assert_eq!(policy.renew_interval(), Duration::from_secs(1));
        assert!(policy.expiry(i64::MAX).is_err());
        assert!(
            policy.expiry(-20_000_000).is_err(),
            "negative expiry cannot be stored"
        );
    }
}

#[cfg(all(test, feature = "db-tests"))]
mod tests {
    use super::*;
    #[cfg(feature = "db-tests")]
    #[test]
    fn e2e_observation_hashes_exact_capability_bytes() {
        use sha2::Digest;
        let owner = Uuid::from_bytes([17; 16]);
        let request = FamilyMaintenanceClaimRequest {
            key: MaintenanceJobKey::Daily,
            owner,
        };
        let proof = FamilyMaintenanceProof {
            key: request.key,
            owner,
            generation: 42,
        };
        assert_eq!(
            request.e2e_owner_sha256(),
            hex::encode(sha2::Sha256::digest([17; 16]))
        );
        assert_eq!(request.e2e_owner_sha256(), proof.e2e_owner_sha256());
        assert_ne!(
            request.e2e_owner_sha256(),
            hex::encode(sha2::Sha256::digest(owner.to_string().as_bytes()))
        );
    }
    use sqlx::SqlitePool;
    use std::path::PathBuf;
    struct Fixture {
        root: PathBuf,
        path: PathBuf,
        backend: Backend,
        pool: SqlitePool,
    }
    impl Fixture {
        async fn new() -> Self {
            let root = std::env::temp_dir().join(format!("fvoci-s16-{}", Uuid::now_v7()));
            std::fs::create_dir_all(&root).unwrap();
            let path = root.join("app.sqlite");
            super::super::migrate::run_sqlite_migrations(&path)
                .await
                .unwrap();
            let pool = super::super::pool::connect_sqlite_app(&path, 1)
                .await
                .unwrap();
            let backend = Backend::Sqlite(pool.clone());
            let gate = super::super::migrate::assert_sqlite_schema_current(&backend)
                .await
                .unwrap();
            assert_eq!(
                gate.applied_steps,
                super::super::migrate::compiled_sqlite_steps().len()
            );
            println!(
                "S16 actual current schema4 {} {}",
                gate.schema_sha256,
                root.display()
            );
            Self {
                root,
                path,
                backend,
                pool,
            }
        }
        async fn second(&self) -> Backend {
            Backend::Sqlite(
                super::super::pool::connect_sqlite_app(&self.path, 1)
                    .await
                    .unwrap(),
            )
        }
        async fn row(&self, key: MaintenanceJobKey) -> (Option<Vec<u8>>, i64, Option<i64>) {
            sqlx::query_as("SELECT owner_token,generation,expires_at FROM maintenance_job_claims WHERE job_key=?1")
                .bind(key as i64).fetch_one(&self.pool).await.unwrap()
        }
        async fn close(self) {
            self.backend.close().await.unwrap();
            std::fs::remove_dir_all(self.root).unwrap();
        }
    }
    fn policy() -> FamilyMaintenanceLeasePolicy {
        FamilyMaintenanceLeasePolicy::new(Duration::from_secs(60), Duration::from_secs(10)).unwrap()
    }
    async fn acquired(
        request: &FamilyMaintenanceClaimRequest,
        backend: &Backend,
    ) -> FamilyMaintenanceClaim {
        match request
            .try_acquire(backend, policy(), &CancellationToken::new())
            .await
            .unwrap()
        {
            FamilyClaimAcquisition::Acquired(claim) => claim,
            _ => panic!("mandatory healthy acquisition"),
        }
    }
    #[tokio::test]
    async fn maintenance_claim_two_actual_writers_contention_keys_and_release() {
        let f = Fixture::new().await;
        let other = f.second().await;
        let request = FamilyMaintenanceClaimRequest::new(MaintenanceJobKey::Daily);
        let mut first = acquired(&request, &f.backend).await;
        let rival = FamilyMaintenanceClaimRequest::new(MaintenanceJobKey::Daily);
        assert!(matches!(
            rival
                .try_acquire(&other, policy(), &CancellationToken::new())
                .await
                .unwrap(),
            FamilyClaimAcquisition::Busy
        ));
        let uploads = acquired(
            &FamilyMaintenanceClaimRequest::new(MaintenanceJobKey::Uploads),
            &other,
        )
        .await;
        assert_eq!(uploads.proof.generation, 1);
        assert_eq!(
            first.renew(&CancellationToken::new()).await.unwrap(),
            FamilyLeaseAction::Confirmed
        );
        // Same prepared owner observes the live committed identity, no new generation.
        let replay = acquired(&request, &other).await;
        assert_eq!(replay.proof, first.proof);
        drop(replay);
        assert_eq!(first.release().await.unwrap(), FamilyLeaseAction::Confirmed);
        assert_eq!(f.row(MaintenanceJobKey::Daily).await, (None, 1, None));
        let successor = acquired(&rival, &other).await;
        assert_eq!(successor.proof.generation, 2);
        assert_eq!(
            successor.release().await.unwrap(),
            FamilyLeaseAction::Confirmed
        );
        assert_eq!(
            uploads.release().await.unwrap(),
            FamilyLeaseAction::Confirmed
        );
        other.close().await.unwrap();
        f.close().await;
    }
    #[tokio::test]
    async fn maintenance_claim_expired_generation_wrong_key_and_tenant_proof() {
        let f = Fixture::new().await;
        let other = f.second().await;
        let mut old = acquired(
            &FamilyMaintenanceClaimRequest::new(MaintenanceJobKey::Uploads),
            &f.backend,
        )
        .await;
        let stale = old.proof.clone();
        sqlx::query("UPDATE maintenance_job_claims SET expires_at=0 WHERE job_key=8")
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            old.renew(&CancellationToken::new()).await.unwrap(),
            FamilyLeaseAction::Lost
        );
        let successor = acquired(
            &FamilyMaintenanceClaimRequest::new(MaintenanceJobKey::Uploads),
            &other,
        )
        .await;
        assert_eq!(successor.proof.generation, 2);
        assert_eq!(old.release().await.unwrap(), FamilyLeaseAction::Lost);
        let current = f.row(MaintenanceJobKey::Uploads).await;
        let workspace = Uuid::now_v7();
        sqlx::query("INSERT INTO workspaces(id,slug,name) VALUES(?1,'s16-proof','original')")
            .bind(workspace.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let mut tx = f.backend.begin_write().await.unwrap();
        let mut op = tx.operation();
        op.set_tenant(workspace).await.unwrap();
        assert!(!op
            .check_family_maintenance_claim(&stale, MaintenanceJobKey::Uploads)
            .await
            .unwrap());
        assert!(!op
            .check_family_maintenance_claim(successor.proof(), MaintenanceJobKey::Daily)
            .await
            .unwrap());
        assert!(op
            .renew_family_maintenance_claim(&stale, MaintenanceJobKey::Uploads, policy())
            .await
            .unwrap()
            .is_none());
        if op
            .check_family_maintenance_claim(&stale, MaintenanceJobKey::Uploads)
            .await
            .unwrap()
        {
            op.maintenance_writer()
                .unwrap()
                .execute(
                    "UPDATE workspaces SET name='stale mutation' WHERE id=?1",
                    &[Cell::uuid(workspace)],
                )
                .await
                .unwrap();
        }
        let family = op.maintenance_writer().unwrap();
        assert_eq!(family.tenant(), Some(workspace));
        assert!(family.require_system_context().is_err());
        assert!(family.require_tenant(Uuid::now_v7()).is_err());
        assert!(op
            .check_family_maintenance_claim(successor.proof(), MaintenanceJobKey::Uploads)
            .await
            .unwrap());
        assert!(op
            .renew_family_maintenance_claim(successor.proof(), MaintenanceJobKey::Uploads, policy())
            .await
            .unwrap()
            .is_some());
        op.maintenance_writer()
            .unwrap()
            .execute(
                "UPDATE workspaces SET name='confirmed unit' WHERE id=?1",
                &[Cell::uuid(workspace)],
            )
            .await
            .unwrap();
        assert!(op
            .check_family_maintenance_claim(successor.proof(), MaintenanceJobKey::Uploads)
            .await
            .unwrap());
        tx.commit().await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT name FROM workspaces WHERE id=?1")
                .bind(workspace.as_bytes().as_slice())
                .fetch_one(&f.pool)
                .await
                .unwrap(),
            "confirmed unit"
        );
        assert_eq!(f.row(MaintenanceJobKey::Uploads).await.0, current.0);
        let mut read = f.backend.begin_read().await.unwrap();
        assert!(read
            .operation()
            .check_family_maintenance_claim(successor.proof(), MaintenanceJobKey::Uploads)
            .await
            .is_err());
        read.rollback().await.unwrap();
        successor.release().await.unwrap();
        other.close().await.unwrap();
        f.close().await;
    }
    #[tokio::test]
    async fn maintenance_claim_cancelled_writer_settles_and_reuses_max_one() {
        let f = Fixture::new().await;
        let request = FamilyMaintenanceClaimRequest::new(MaintenanceJobKey::Revisions);
        let (entered, go) = test_hooks::pause(request.owner);
        let backend = f.backend.clone();
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let run =
            tokio::spawn(async move { request.try_acquire(&backend, policy(), &token).await });
        entered.await.unwrap();
        cancel.cancel();
        go.send(()).unwrap();
        assert!(matches!(
            run.await.unwrap().unwrap(),
            FamilyClaimAcquisition::Cancelled
        ));
        assert_eq!(f.row(MaintenanceJobKey::Revisions).await, (None, 0, None));
        let request = FamilyMaintenanceClaimRequest::new(MaintenanceJobKey::Revisions);
        assert!(matches!(
            request
                .try_acquire(&f.backend, policy(), &cancel)
                .await
                .unwrap(),
            FamilyClaimAcquisition::Cancelled
        ));
        let claim = acquired(&request, &f.backend).await;
        assert_eq!(claim.proof.generation, 1);
        claim.release().await.unwrap();
        f.close().await;
    }
    #[tokio::test]
    async fn maintenance_claim_real_commit_failure_never_confirms_and_same_token_recovers() {
        let f = Fixture::new().await;
        let request = FamilyMaintenanceClaimRequest::new(MaintenanceJobKey::Daily);
        test_hooks::fault(request.owner, 0);
        let error = match request
            .try_acquire(&f.backend, policy(), &CancellationToken::new())
            .await
        {
            Err(error) => error,
            _ => panic!("actual FK COMMIT must not claim"),
        };
        assert!(
            matches!(&error, MaintenanceClaimError::CommitUnknown { source, .. } if source.source.as_database_error().is_some())
        );
        println!("S16 actual claim FK COMMIT {error:?}");
        assert_eq!(f.row(MaintenanceJobKey::Daily).await, (None, 0, None));
        let mut claim = acquired(&request, &f.backend).await;
        let mut before = f.row(MaintenanceJobKey::Daily).await;
        // Give renewal a distinct durable target without depending on a timer.
        sqlx::query(
            "UPDATE maintenance_job_claims SET expires_at=expires_at-1000000 WHERE job_key=1",
        )
        .execute(&f.pool)
        .await
        .unwrap();
        before.2 = before.2.map(|v| v - 1_000_000);
        test_hooks::fault(request.owner, 1);
        assert!(matches!(
            claim.renew(&CancellationToken::new()).await,
            Err(MaintenanceClaimError::CommitUnknown { .. })
        ));
        assert_eq!(f.row(MaintenanceJobKey::Daily).await, before);
        test_hooks::fault(request.owner, 2);
        assert!(matches!(
            claim.release().await,
            Err(MaintenanceClaimError::CommitUnknown { .. })
        ));
        assert_eq!(f.row(MaintenanceJobKey::Daily).await, before);
        let recovered = acquired(&request, &f.backend).await;
        assert_eq!(recovered.proof.generation, 1);
        assert_eq!(recovered.proof.owner, request.owner);
        recovered.release().await.unwrap();
        f.close().await;
    }
    #[tokio::test]
    async fn maintenance_claim_schema_shape_missing_row_and_checked_generation_expiry() {
        let f = Fixture::new().await;
        for statement in [
            "UPDATE maintenance_job_claims SET owner_token=x'01' WHERE job_key=1",
            "UPDATE maintenance_job_claims SET owner_token=zeroblob(16) WHERE job_key=1",
            "UPDATE maintenance_job_claims SET generation=-1 WHERE job_key=1",
            "UPDATE maintenance_job_claims SET expires_at=-1 WHERE job_key=1",
            "UPDATE maintenance_job_claims SET job_key=10 WHERE job_key=1",
        ] {
            assert!(sqlx::query(statement).execute(&f.pool).await.is_err());
        }
        sqlx::query(
            "UPDATE maintenance_job_claims SET generation=9223372036854775807 WHERE job_key=1",
        )
        .execute(&f.pool)
        .await
        .unwrap();
        let request = FamilyMaintenanceClaimRequest::new(MaintenanceJobKey::Daily);
        assert!(request
            .try_acquire(&f.backend, policy(), &CancellationToken::new())
            .await
            .is_err());
        assert_eq!(
            f.row(MaintenanceJobKey::Daily).await,
            (None, i64::MAX, None)
        );
        let huge = FamilyMaintenanceLeasePolicy::new(
            Duration::from_micros(i64::MAX as u64),
            Duration::from_secs(1),
        )
        .unwrap();
        assert!(
            FamilyMaintenanceClaimRequest::new(MaintenanceJobKey::Uploads)
                .try_acquire(&f.backend, huge, &CancellationToken::new())
                .await
                .is_err()
        );
        assert_eq!(f.row(MaintenanceJobKey::Uploads).await, (None, 0, None));
        sqlx::query("DELETE FROM maintenance_job_claims WHERE job_key=9")
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(
            FamilyMaintenanceClaimRequest::new(MaintenanceJobKey::Revisions)
                .try_acquire(&f.backend, policy(), &CancellationToken::new())
                .await
                .is_err()
        );
        f.close().await;
    }
    #[tokio::test]
    async fn maintenance_claim_current_durable_identity_reconciles_actual_failed_commit() {
        let f = Fixture::new().await;
        let other = f.second().await;
        let request = FamilyMaintenanceClaimRequest::new(MaintenanceJobKey::Daily);
        let first = acquired(&request, &f.backend).await;
        let proof = first.proof.clone();
        let before = f.row(MaintenanceJobKey::Daily).await;
        drop(first); // Explicitly no auto-release: emulate a lost caller result.
        test_hooks::fault(request.owner, 0);
        // This COMMIT genuinely fails FK, but the SAME stable identity already
        // has an independently confirmed live durable row. Fresh writer read
        // can recover that ownership without advancing generation or expiry.
        let recovered = acquired(&request, &other).await;
        assert_eq!(recovered.proof, proof);
        assert_eq!(f.row(MaintenanceJobKey::Daily).await, before);
        println!(
            "S16 existing durable owner recovered after actual FK COMMIT; generation={}",
            proof.generation
        );
        assert!(matches!(
            FamilyMaintenanceClaimRequest::new(MaintenanceJobKey::Daily)
                .try_acquire(&f.backend, policy(), &CancellationToken::new())
                .await
                .unwrap(),
            FamilyClaimAcquisition::Busy
        ));
        recovered.release().await.unwrap();
        other.close().await.unwrap();
        f.close().await;
    }
    #[tokio::test]
    async fn maintenance_claim_common_entry_retains_identity_and_stale_generation() {
        use crate::jobs::{GlobalClaimAcquisition, GlobalJobClaim};
        let f = Fixture::new().await;
        let other = f.second().await;
        let request = FamilyMaintenanceClaimRequest::new(MaintenanceJobKey::Uploads);
        let cancel = CancellationToken::new();
        test_hooks::fault(request.owner, 0);
        let error = match GlobalJobClaim::try_claim(&f.backend, &request, policy(), &cancel).await {
            Err(error) => error,
            _ => panic!("common entry must retain actual failed COMMIT"),
        };
        assert!(
            matches!(&error, MaintenanceClaimError::CommitUnknown { source, settlement: CommitSettlement::LocalWriterReconcile, cleanup_error: None, observation_error: None }
            if source.source.as_database_error().is_some())
        );
        assert_eq!(f.row(MaintenanceJobKey::Uploads).await, (None, 0, None));
        let first = match GlobalJobClaim::try_claim(&f.backend, &request, policy(), &cancel)
            .await
            .unwrap()
        {
            GlobalClaimAcquisition::Acquired(GlobalJobClaim::Family(claim)) => claim,
            _ => panic!("same caller-retained request must make healthy progress"),
        };
        let old_proof = first.proof.clone();
        let before = f.row(MaintenanceJobKey::Uploads).await;
        assert_eq!(old_proof.owner, request.owner);
        assert_eq!(old_proof.generation, 1);
        drop(first); // Durable ownership remains; no automatic release.
        test_hooks::fault(request.owner, 0);
        let replay = match GlobalJobClaim::try_claim(&other, &request, policy(), &cancel)
            .await
            .unwrap()
        {
            GlobalClaimAcquisition::Acquired(GlobalJobClaim::Family(claim)) => claim,
            _ => panic!("common entry must reconcile the same live durable identity"),
        };
        assert_eq!(replay.proof, old_proof);
        assert_eq!(f.row(MaintenanceJobKey::Uploads).await, before);
        sqlx::query("UPDATE maintenance_job_claims SET expires_at=0 WHERE job_key=8")
            .execute(&f.pool)
            .await
            .unwrap();
        let next = match GlobalJobClaim::try_claim(&f.backend, &request, policy(), &cancel)
            .await
            .unwrap()
        {
            GlobalClaimAcquisition::Acquired(GlobalJobClaim::Family(claim)) => claim,
            _ => panic!("same owner expiry must advance persisted generation"),
        };
        assert_eq!(next.proof.owner, old_proof.owner);
        assert_eq!(next.proof.generation, 2);
        let mut tx = f.backend.begin_write().await.unwrap();
        let mut op = tx.operation();
        assert!(!op
            .check_family_maintenance_claim(&old_proof, MaintenanceJobKey::Uploads)
            .await
            .unwrap());
        assert!(op
            .renew_family_maintenance_claim(&old_proof, MaintenanceJobKey::Uploads, policy())
            .await
            .unwrap()
            .is_none());
        assert!(op
            .check_family_maintenance_claim(next.proof(), MaintenanceJobKey::Uploads)
            .await
            .unwrap());
        tx.rollback().await.unwrap();
        assert_eq!(replay.release().await.unwrap(), FamilyLeaseAction::Lost);
        assert_eq!(f.row(MaintenanceJobKey::Uploads).await.1, 2);
        next.release().await.unwrap();
        println!("S16 common retained owner actual FK failure/replay and same-owner generation1->2 refusal");
        other.close().await.unwrap();
        f.close().await;
    }
    #[tokio::test]
    async fn maintenance_claim_db_clock_expiry_before_commit_rolls_back_unit() {
        let f = Fixture::new().await;
        let workspace = Uuid::now_v7();
        sqlx::query("INSERT INTO workspaces(id,slug,name) VALUES(?1,'s16-clock','before')")
            .bind(workspace.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let request = FamilyMaintenanceClaimRequest::new(MaintenanceJobKey::Daily);
        let policy =
            FamilyMaintenanceLeasePolicy::new(Duration::from_secs(1), Duration::from_millis(100))
                .unwrap();
        let claim = match request
            .try_acquire(&f.backend, policy, &CancellationToken::new())
            .await
            .unwrap()
        {
            FamilyClaimAcquisition::Acquired(claim) => claim,
            _ => panic!("healthy current claim"),
        };
        let mut tx = f.backend.begin_write().await.unwrap();
        let mut op = tx.operation();
        op.set_tenant(workspace).await.unwrap();
        assert!(op
            .check_family_maintenance_claim(claim.proof(), MaintenanceJobKey::Daily)
            .await
            .unwrap());
        op.maintenance_writer()
            .unwrap()
            .execute(
                "UPDATE workspaces SET name='uncommitted effect' WHERE id=?1",
                &[Cell::uuid(workspace)],
            )
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while op
                .check_family_maintenance_claim(claim.proof(), MaintenanceJobKey::Daily)
                .await
                .unwrap()
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("actual DB clock passes stored expiry");
        // A real consumer must refuse COMMIT after its final current check.
        tx.rollback().await.unwrap();
        let current: String = sqlx::query_scalar("SELECT name FROM workspaces WHERE id=?1")
            .bind(workspace.as_bytes().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(current, "before");
        assert_eq!(claim.release().await.unwrap(), FamilyLeaseAction::Lost);
        let successor = acquired(
            &FamilyMaintenanceClaimRequest::new(MaintenanceJobKey::Daily),
            &f.backend,
        )
        .await;
        assert_eq!(successor.proof.generation, 2);
        successor.release().await.unwrap();
        f.close().await;
    }
}
