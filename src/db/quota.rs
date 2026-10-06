use sqlx::{Postgres, Transaction};
use std::sync::Arc;
use uuid::Uuid;

use crate::db::context::{restore_system, set_system};
use crate::db::workspace::WorkspaceRole;

/// Dedicated xact lock for billable membership changes. Distinct from
/// `MIGRATION_LOCK_KEY` so admissions do not serialize against migrate.
pub const ADMISSION_LOCK_KEY: i64 = 1_907_008_552;
pub const INSTANCE_SEAT_LIMIT: i32 = 10;

/// Source `QuotaLimit`: a byte ceiling or `"unlimited"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QuotaLimit {
    #[default]
    Unlimited,
    Bytes(i64),
}

/// The storage half of source `QuotaPolicy` (`storageBytes`, `uploadBytes`).
/// Both signed limits are resolved from one entitlement at reservation time.
/// The fixed variant is used by explicit quota tests and offline callers.
#[derive(Debug, Clone, Default)]
pub enum StorageQuota {
    #[default]
    Unlimited,
    Signed(Arc<crate::license::Entitlements>),
    Fixed {
        storage_bytes: QuotaLimit,
        upload_bytes: QuotaLimit,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageQuotaError {
    /// `limit.upload`: one upload is larger than the per-upload limit.
    Upload,
    /// `limit.storage`: the workspace's reserved bytes would exceed the limit.
    Storage,
}

impl StorageQuota {
    pub fn from_license(license: Arc<crate::license::Entitlements>) -> Self {
        Self::Signed(license)
    }

    pub fn fixed(storage_bytes: QuotaLimit, upload_bytes: QuotaLimit) -> Self {
        Self::Fixed {
            storage_bytes,
            upload_bytes,
        }
    }

    /// Source `requireStorageReservation`. `reserved_bytes` is the sum of
    /// `reserved_size_bytes` over every attachment row of the workspace
    /// (uploading, assembling and stored), read under the workspace storage
    /// lock so concurrent reservations cannot both pass.
    pub fn check(&self, reserved_bytes: i64, size_bytes: i64) -> Result<(), StorageQuotaError> {
        let (storage_bytes, upload_bytes) = match self {
            Self::Unlimited => (QuotaLimit::Unlimited, QuotaLimit::Unlimited),
            Self::Fixed {
                storage_bytes,
                upload_bytes,
            } => (*storage_bytes, *upload_bytes),
            Self::Signed(license) => {
                let limits = license.limits();
                let convert = |limit| match limit {
                    crate::license::Limit::Value(n) => QuotaLimit::Bytes(n as i64),
                    crate::license::Limit::Unlimited => QuotaLimit::Unlimited,
                };
                (convert(limits.storage_bytes), convert(limits.upload_bytes))
            }
        };
        Self::check_resolved(reserved_bytes, size_bytes, storage_bytes, upload_bytes)
    }

    fn check_resolved(
        reserved_bytes: i64,
        size_bytes: i64,
        storage_bytes: QuotaLimit,
        upload_bytes: QuotaLimit,
    ) -> Result<(), StorageQuotaError> {
        if let QuotaLimit::Bytes(limit) = upload_bytes {
            if size_bytes > limit {
                return Err(StorageQuotaError::Upload);
            }
        }
        if let QuotaLimit::Bytes(limit) = storage_bytes {
            if reserved_bytes.saturating_add(size_bytes) > limit {
                return Err(StorageQuotaError::Storage);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaError {
    SeatLimit,
    #[allow(dead_code)]
    GuestLimit,
}

pub async fn acquire_admission_lock(tx: &mut Transaction<'_, Postgres>) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(ADMISSION_LOCK_KEY)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Source `requireInstanceSeat`. Call only while holding the admission lock.
pub async fn require_instance_seat(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Option<Uuid>,
    license: &crate::license::Entitlements,
) -> Result<Result<(), QuotaError>, sqlx::Error> {
    let previous = set_system(tx).await?;
    let outcome = require_instance_seat_inner(tx, user_id, license).await;
    restore_system(tx, &previous).await?;
    outcome
}

async fn require_instance_seat_inner(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Option<Uuid>,
    license: &crate::license::Entitlements,
) -> Result<Result<(), QuotaError>, sqlx::Error> {
    if let Some(user_id) = user_id {
        let already_billable: i32 = sqlx::query_scalar("SELECT fvoci.app_quota_billable_users($1)")
            .bind(user_id)
            .fetch_one(&mut **tx)
            .await?;
        if already_billable > 0 {
            return Ok(Ok(()));
        }
    }
    let billable: i32 = sqlx::query_scalar("SELECT fvoci.app_quota_billable_users(NULL::uuid)")
        .fetch_one(&mut **tx)
        .await?;
    let seat_limit = match license.limits().seats {
        crate::license::Limit::Value(n) => Some(n),
        crate::license::Limit::Unlimited => None,
    };
    if seat_exceeded(billable, seat_limit) {
        return Ok(Err(QuotaError::SeatLimit));
    }
    Ok(Ok(()))
}

fn seat_exceeded(billable: i32, limit: Option<u64>) -> bool {
    limit.is_some_and(|limit| u64::from(billable.max(0) as u32) >= limit)
}

/// Source `requireNewInstanceBillableUser`. Call only while holding the admission lock.
pub async fn require_new_instance_billable_user(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Option<Uuid>,
    license: &crate::license::Entitlements,
) -> Result<Result<(), QuotaError>, sqlx::Error> {
    require_instance_seat(tx, user_id, license).await
}

impl super::backend::OperationTx<'_, '_> {
    pub(crate) async fn acquire_admission_lock(&mut self) -> Result<(), sqlx::Error> {
        match self {
            Self::Postgres(tx) => acquire_admission_lock(tx).await,
            Self::SqliteFamily(tx) => tx.require_writer(),
        }
    }

    pub(crate) async fn require_new_instance_billable_user(
        &mut self,
        user: Option<Uuid>,
        license: &crate::license::Entitlements,
    ) -> Result<Result<(), QuotaError>, sqlx::Error> {
        if let Self::Postgres(tx) = self {
            return require_new_instance_billable_user(tx, user, license).await;
        }
        let limit = match license.limits().seats {
            crate::license::Limit::Value(n) => Some(n),
            crate::license::Limit::Unlimited => None,
        };
        self.family_instance_seat(user, limit).await
    }

    async fn family_instance_seat(
        &mut self,
        user: Option<Uuid>,
        limit: Option<u64>,
    ) -> Result<Result<(), QuotaError>, sqlx::Error> {
        self.acquire_admission_lock().await?;
        let previous = self.set_system().await?;
        let result = async {
            if let Some(user) = user {
                if self.family_billable_users(Some(user)).await? > 0 {
                    return Ok(Ok(()));
                }
            }
            let billable = self.family_billable_users(None).await?;
            Ok(if seat_exceeded(billable, limit) {
                Err(QuotaError::SeatLimit)
            } else {
                Ok(())
            })
        }
        .await;
        let restore = self.restore_system(previous).await;
        quota_after_context_restore(result, restore)
    }

    async fn family_billable_users(&mut self, user: Option<Uuid>) -> Result<i32, sqlx::Error> {
        let Self::SqliteFamily(tx) = self else {
            return Err(sqlx::Error::Protocol(
                "family seat aggregate requires family writer".into(),
            ));
        };
        tx.require_writer()?;
        tx.require_system_context()?;
        // Exact app_quota_billable_users policy: count users once, excluding
        // anonymized users, with team or mapped personal ownership eligibility.
        let rows = tx.query(
            "SELECT count(*) FROM users u WHERE (?1 IS NULL OR u.id=?1) AND u.anonymized_at IS NULL AND (u.is_instance_admin=1 OR EXISTS(SELECT 1 FROM memberships m JOIN workspaces w ON w.id=m.workspace_id WHERE m.user_id=u.id AND m.role<>'guest' AND w.kind='team' AND w.deleted_at IS NULL) OR EXISTS(SELECT 1 FROM memberships m JOIN workspaces w ON w.id=u.personal_workspace_id WHERE m.workspace_id=w.id AND m.user_id=u.id AND m.role='owner' AND w.kind='personal' AND w.deleted_at IS NULL))",
            &[super::codec::Cell::optional_uuid(user)],
        ).await?;
        rows.first()
            .ok_or(sqlx::Error::RowNotFound)?
            .cell(0)?
            .int32()
    }
}

#[derive(Debug, thiserror::Error)]
#[error("seat admission system context restoration failed after {original:?}")]
struct QuotaContextRestoreFailure {
    #[source]
    restore: sqlx::Error,
    original: Result<Result<(), QuotaError>, sqlx::Error>,
}

fn quota_after_context_restore(
    original: Result<Result<(), QuotaError>, sqlx::Error>,
    restore: Result<(), sqlx::Error>,
) -> Result<Result<(), QuotaError>, sqlx::Error> {
    match restore {
        Ok(()) => original,
        Err(restore) => Err(sqlx::Error::AnyDriverError(Box::new(
            QuotaContextRestoreFailure { restore, original },
        ))),
    }
}

/// Source `requireMembershipAdmission`. Call only while holding the admission lock.
pub async fn require_membership_admission(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    role: WorkspaceRole,
    current_role: Option<WorkspaceRole>,
    license: &crate::license::Entitlements,
) -> Result<Result<(), QuotaError>, sqlx::Error> {
    if current_role == Some(role) {
        return Ok(Ok(()));
    }
    if role == WorkspaceRole::Guest {
        // Default self-host policy is guests: unlimited, so GuestLimit cannot
        // be produced until a workspace guest quota provider exists.
        return Ok(Ok(()));
    }
    require_instance_seat(tx, Some(user_id), license).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_quota_matches_source_reservation_rules() {
        assert_eq!(
            StorageQuota::default().check(i64::MAX - 1, i64::MAX),
            Ok(())
        );
        let quota = StorageQuota::fixed(QuotaLimit::Bytes(100), QuotaLimit::Bytes(40));
        assert_eq!(quota.check(0, 40), Ok(()));
        assert_eq!(quota.check(0, 41), Err(StorageQuotaError::Upload));
        assert_eq!(quota.check(60, 40), Ok(()), "exactly at the limit");
        assert_eq!(quota.check(61, 40), Err(StorageQuotaError::Storage));
        // Upload limit is checked first, like the source.
        assert_eq!(quota.check(100, 41), Err(StorageQuotaError::Upload));
    }

    #[test]
    fn signed_seat_admission_limit_and_unlimited() {
        assert!(!seat_exceeded(1, Some(2)));
        assert!(seat_exceeded(2, Some(2)));
        assert!(!seat_exceeded(100, None));
    }
}

#[cfg(all(test, feature = "db-tests"))]
mod selected_seat_admission_tests {
    use super::*;
    use crate::db::backend::{DbTransaction, OperationTx};
    use crate::db::codec::Cell;
    use crate::db::workspace::selected_personal_workspace_tests::{
        fixture, foreign_keys, snapshot,
    };
    use serde_json::json;

    fn signed_seats(seats: serde_json::Value) -> crate::license::Entitlements {
        // Same ephemeral test-trust path as tests/support/license.rs; production
        // trust, claims parser and verification policy are unchanged.
        use base64::{
            engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
            Engine,
        };
        use ring::signature::{Ed25519KeyPair, KeyPair};
        use sha2::{Digest, Sha256};
        let pair = Ed25519KeyPair::from_seed_unchecked(&[7; 32]).unwrap();
        let mut der = hex::decode("302a300506032b6570032100").unwrap();
        der.extend_from_slice(pair.public_key().as_ref());
        let manifest=json!({"version":1,"keys":[{"kid":"bootstrap-test","alg":"ed25519","spki":format!("-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----\n",STANDARD.encode(&der)),"spkiSha256":hex::encode(Sha256::digest(&der))}]}).to_string();
        let header =
            URL_SAFE_NO_PAD.encode(json!({"alg":"ed25519","kid":"bootstrap-test"}).to_string());
        let payload=URL_SAFE_NO_PAD.encode(json!({"v":1,"licensee":"bootstrap-test","plan":"selfhost-pro","features":[],"limits":{"seats":seats},"iat":"2026-01-01T00:00:00Z","nbf":"2026-01-01T00:00:00Z","exp":"2030-01-01T00:00:00Z"}).to_string());
        let signed = format!("{header}.{payload}");
        let token = format!(
            "FVOCI2-{signed}.{}",
            URL_SAFE_NO_PAD.encode(pair.sign(signed.as_bytes()).as_ref())
        );
        crate::license::load(Some(&token), &manifest)
    }

    #[tokio::test]
    async fn sqlite_seat_admission_exact_billable_predicate_limit_existing_and_unlimited() {
        let (f, _) = fixture().await;
        let one = signed_seats(json!(1));
        let unlimited = signed_seats(json!("unlimited"));
        assert_eq!(one.limits().seats, crate::license::Limit::Value(1));
        assert_eq!(unlimited.limits().seats, crate::license::Limit::Unlimited);
        let mut tx = f.backend.begin_write().await.unwrap();
        tx.operation().set_tenant(f.workspace).await.unwrap();
        assert!(matches!(
            tx.operation()
                .require_new_instance_billable_user(None, &one)
                .await
                .unwrap(),
            Err(QuotaError::SeatLimit)
        ));
        assert!(tx
            .operation()
            .require_new_instance_billable_user(Some(f.user), &one)
            .await
            .unwrap()
            .is_ok());
        assert!(tx
            .operation()
            .require_new_instance_billable_user(None, &unlimited)
            .await
            .unwrap()
            .is_ok());
        let mut op = tx.operation();
        let prior = op.set_system().await.unwrap();
        assert_eq!(op.family_billable_users(None).await.unwrap(), 1);
        let OperationTx::SqliteFamily(family) = &mut op else {
            panic!("real family writer required")
        };
        let personal = Uuid::now_v7();
        family.execute("INSERT INTO workspaces(id,slug,name,kind) VALUES(?1,'quota-personal','Personal','personal')",&[Cell::uuid(personal)]).await.unwrap();
        family
            .execute(
                "INSERT INTO memberships(workspace_id,user_id,role) VALUES(?1,?2,'owner')",
                &[Cell::uuid(personal), Cell::uuid(f.user)],
            )
            .await
            .unwrap();
        family
            .execute(
                "UPDATE users SET personal_workspace_id=?1 WHERE id=?2",
                &[Cell::uuid(personal), Cell::uuid(f.user)],
            )
            .await
            .unwrap();
        assert_eq!(
            op.family_billable_users(None).await.unwrap(),
            1,
            "one user with two eligible memberships consumes one seat"
        );
        let OperationTx::SqliteFamily(family) = &mut op else {
            panic!("real family writer required")
        };
        family
            .execute(
                "UPDATE workspaces SET deleted_at=1 WHERE id=?1",
                &[Cell::uuid(f.workspace)],
            )
            .await
            .unwrap();
        assert_eq!(
            op.family_billable_users(None).await.unwrap(),
            1,
            "mapped personal owner still billable"
        );
        let OperationTx::SqliteFamily(family) = &mut op else {
            panic!("real family writer required")
        };
        family
            .execute(
                "UPDATE users SET personal_workspace_id=NULL WHERE id=?1",
                &[Cell::uuid(f.user)],
            )
            .await
            .unwrap();
        assert_eq!(
            op.family_billable_users(None).await.unwrap(),
            0,
            "unmapped personal ownership does not qualify"
        );
        let OperationTx::SqliteFamily(family) = &mut op else {
            panic!("real family writer required")
        };
        family
            .execute(
                "UPDATE workspaces SET deleted_at=NULL WHERE id=?1",
                &[Cell::uuid(f.workspace)],
            )
            .await
            .unwrap();
        family
            .execute(
                "UPDATE memberships SET role='guest' WHERE workspace_id=?1",
                &[Cell::uuid(f.workspace)],
            )
            .await
            .unwrap();
        assert_eq!(
            op.family_billable_users(None).await.unwrap(),
            0,
            "team guest excluded"
        );
        let OperationTx::SqliteFamily(family) = &mut op else {
            panic!("real family writer required")
        };
        family
            .execute(
                "UPDATE users SET is_instance_admin=1,deleted_at=1,suspended_at=1 WHERE id=?1",
                &[Cell::uuid(f.user)],
            )
            .await
            .unwrap();
        assert_eq!(
            op.family_billable_users(None).await.unwrap(),
            1,
            "exact PG aggregate retains nonanonymized admin; caller separately proves live actor"
        );
        let OperationTx::SqliteFamily(family) = &mut op else {
            panic!("real family writer required")
        };
        family
            .execute(
                "UPDATE users SET anonymized_at=1 WHERE id=?1",
                &[Cell::uuid(f.user)],
            )
            .await
            .unwrap();
        assert_eq!(op.family_billable_users(None).await.unwrap(), 0);
        op.restore_system(prior).await.unwrap();
        tx.rollback().await.unwrap();
        // At the default10 seat ceiling a new guest cannot be admitted, but an
        // already billable actor does not consume a second seat.
        sqlx::query("UPDATE memberships SET role='guest' WHERE user_id=?1")
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        for n in 0..10 {
            sqlx::query(
                "INSERT INTO users(id,email,given_name,is_instance_admin) VALUES(?1,?2,'Seat',1)",
            )
            .bind(Uuid::now_v7().as_bytes().as_slice())
            .bind(format!("seat{n}@quota.test"))
            .execute(&f.pool)
            .await
            .unwrap();
        }
        let before = snapshot(&f).await;
        let mut tx = f.backend.begin_write().await.unwrap();
        tx.operation().set_tenant(f.workspace).await.unwrap();
        assert!(matches!(
            tx.operation()
                .require_new_instance_billable_user(Some(f.user), &crate::license::absent())
                .await
                .unwrap(),
            Err(QuotaError::SeatLimit)
        ));
        let DbTransaction::SqliteFamily(family) = &mut tx else {
            panic!("real family writer required")
        };
        assert!(family.require_system_context().is_err());
        family.require_tenant(f.workspace).unwrap();
        tx.rollback().await.unwrap();
        assert_eq!(snapshot(&f).await, before);
        foreign_keys(&f).await;
        f.close().await;
    }

    #[tokio::test]
    async fn sqlite_seat_admission_writer_context_driver_failure_and_healthy_retry() {
        let (f, _) = fixture().await;
        let before = snapshot(&f).await;
        let mut read = f.backend.begin_read().await.unwrap();
        assert!(matches!(
            read.operation()
                .require_new_instance_billable_user(None, &crate::license::absent())
                .await,
            Err(sqlx::Error::Protocol(_))
        ));
        read.rollback().await.unwrap();
        sqlx::query("ALTER TABLE users RENAME TO quota_fault_users")
            .execute(&f.pool)
            .await
            .unwrap();
        let mut tx = f.backend.begin_write().await.unwrap();
        tx.operation().set_tenant(f.workspace).await.unwrap();
        assert!(matches!(
            tx.operation()
                .require_new_instance_billable_user(None, &crate::license::absent())
                .await,
            Err(sqlx::Error::Database(_))
        ));
        let DbTransaction::SqliteFamily(family) = &mut tx else {
            panic!("real family writer required")
        };
        assert!(family.require_system_context().is_err());
        family.require_tenant(f.workspace).unwrap();
        tx.rollback().await.unwrap();
        sqlx::query("ALTER TABLE quota_fault_users RENAME TO users")
            .execute(&f.pool)
            .await
            .unwrap();
        let mut tx = f.backend.begin_write().await.unwrap();
        let mut op = tx.operation();
        op.set_tenant(f.workspace).await.unwrap();
        let previous = op.set_system().await.unwrap();
        assert!(op
            .require_new_instance_billable_user(Some(f.user), &crate::license::absent())
            .await
            .unwrap()
            .is_ok());
        let OperationTx::SqliteFamily(family) = &mut op else {
            panic!("real family writer required")
        };
        family.require_system_context().unwrap();
        family.require_tenant(f.workspace).unwrap();
        op.restore_system(previous).await.unwrap();
        tx.rollback().await.unwrap();
        assert_eq!(snapshot(&f).await, before);
        foreign_keys(&f).await;
        f.close().await;
    }

    #[test]
    fn quota_context_restore_failure_retains_original_refusal_or_driver() {
        for (index, original) in [
            Ok(Ok(())),
            Ok(Err(QuotaError::SeatLimit)),
            Err(sqlx::Error::Protocol("original seat query".into())),
        ]
        .into_iter()
        .enumerate()
        {
            // Pure returned-error propagation, not provider context/cleanup proof.
            let error = quota_after_context_restore(
                original,
                Err(sqlx::Error::Protocol("synthetic restore refusal".into())),
            )
            .unwrap_err();
            let sqlx::Error::AnyDriverError(source) = error else {
                panic!("typed restore failure required")
            };
            let receipt = source.downcast_ref::<QuotaContextRestoreFailure>().unwrap();
            assert!(
                matches!(&receipt.restore,sqlx::Error::Protocol(s) if s=="synthetic restore refusal")
            );
            match index {
                0 => assert!(matches!(&receipt.original, Ok(Ok(())))),
                1 => assert!(matches!(&receipt.original, Ok(Err(QuotaError::SeatLimit)))),
                _ => assert!(
                    matches!(&receipt.original,Err(sqlx::Error::Protocol(s)) if s=="original seat query")
                ),
            }
        }
    }
}
