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
