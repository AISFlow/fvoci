//! Final erasure of withdrawn accounts (source `anonymizeWithdrawnUsers`,
//! first step of the daily sweep). A bounded batch per sweep; each user is
//! claimed under its own row lock and deadline recheck.

use chrono::{DateTime, Duration, Utc};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::db::account::{
    anonymize_withdrawn_user, list_withdrawn_due, WITHDRAWN_ANONYMIZE_BATCH, WITHDRAW_GRACE_DAYS,
};

pub async fn run_withdrawn_anonymize(
    pool: &PgPool,
    now: DateTime<Utc>,
    cancel: &CancellationToken,
) -> Result<u32, sqlx::Error> {
    let cutoff = now - Duration::days(WITHDRAW_GRACE_DAYS);
    let targets = list_withdrawn_due(pool, cutoff, WITHDRAWN_ANONYMIZE_BATCH).await?;
    let mut erased = 0u32;
    for user_id in targets {
        if cancel.is_cancelled() {
            break;
        }
        match anonymize_withdrawn_user(pool, user_id, now, cutoff).await {
            Ok(true) => erased += 1,
            Ok(false) => {}
            // One failed claim rolls back alone; the next sweep retries it.
            Err(err) => {
                warn!(%user_id, error = %err, "maintenance.withdrawn_anonymize_user_failed")
            }
        }
    }
    Ok(erased)
}

pub(crate) async fn run_withdrawn_anonymize_family(
    backend: &crate::db::backend::Backend,
    now: DateTime<Utc>,
    proof: &super::claim::FamilyMaintenanceProof,
    policy: super::claim::FamilyMaintenanceLeasePolicy,
    cancel: &CancellationToken,
) -> Result<u32, super::MaintenanceConsumerError> {
    use super::{commit_maintenance_writer, renew_maintenance_writer, rollback_maintenance_writer};
    let cutoff = now - Duration::days(WITHDRAW_GRACE_DAYS);
    if cancel.is_cancelled() {
        return Err(super::MaintenanceConsumerError::Cancelled);
    }
    let mut read = backend.begin_read().await?;
    let previous = read.operation().set_system().await?;
    let targets = read
        .operation()
        .maintenance_withdrawn_due(cutoff, WITHDRAWN_ANONYMIZE_BATCH)
        .await?;
    read.operation().restore_system(previous).await?;
    read.rollback().await?;
    let mut erased = 0;
    for user in targets {
        if cancel.is_cancelled() {
            return Err(super::MaintenanceConsumerError::Cancelled);
        }
        let mut tx = backend.begin_write().await?;
        let result = async {
            renew_maintenance_writer(
                &mut tx,
                proof,
                super::claim::MaintenanceJobKey::Daily,
                policy,
                cancel,
            )
            .await?;
            let previous = tx.operation().set_system().await?;
            let changed = tx
                .operation()
                .maintenance_anonymize_withdrawn(user, now, cutoff)
                .await?;
            tx.operation().restore_system(previous).await?;
            Ok::<_, super::MaintenanceConsumerError>(changed)
        }
        .await;
        let changed = match result {
            Ok(changed) => changed,
            Err(err) => {
                let err = rollback_maintenance_writer(tx, err).await;
                if err.stops_on_backend(backend) {
                    return Err(err);
                }
                warn!(%user,error=%err,"maintenance.withdrawn_anonymize_user_failed");
                continue;
            }
        };
        commit_maintenance_writer(tx, proof, super::claim::MaintenanceJobKey::Daily, cancel)
            .await?;
        if changed {
            erased += 1
        }
    }
    Ok(erased)
}

#[cfg(test)]
mod family_tests {
    use super::*;
    use crate::db::notifications::family_runtime_fixture::Fixture;
    use crate::settings::messages::Message;
    use chrono::SubsecRound;

    #[tokio::test]
    async fn maintenance_withdrawn_current_deadline_erasure_and_personal_history() {
        let f = Fixture::new().await;
        let now = Utc::now().trunc_subsecs(6);
        let cutoff = now - Duration::days(WITHDRAW_GRACE_DAYS);
        sqlx::query("UPDATE users SET deleted_at=?2,personal_workspace_id=?3,password_hash='old',withdraw_cancel_token_hash='old-cancel' WHERE id=?1")
            .bind(f.user.as_bytes().as_slice()).bind((cutoff-Duration::seconds(1)).timestamp_micros())
            .bind(f.workspace.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        sqlx::query("UPDATE workspaces SET kind='personal' WHERE id=?1")
            .bind(f.workspace.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE users SET deleted_at=?2 WHERE id=?1")
            .bind(f.other_user.as_bytes().as_slice())
            .bind((cutoff + Duration::hours(1)).timestamp_micros())
            .execute(&f.pool)
            .await
            .unwrap();
        let mut denied = f.backend.begin_write().await.unwrap();
        assert!(denied
            .operation()
            .maintenance_anonymize_withdrawn(f.user, now, cutoff)
            .await
            .is_err());
        denied.rollback().await.unwrap();
        let mut tx = f.backend.begin_write().await.unwrap();
        let previous = tx.operation().set_system().await.unwrap();
        assert_eq!(
            tx.operation()
                .maintenance_withdrawn_due(cutoff, WITHDRAWN_ANONYMIZE_BATCH)
                .await
                .unwrap(),
            vec![f.user]
        );
        assert!(
            !tx.operation()
                .maintenance_anonymize_withdrawn(f.other_user, now, now)
                .await
                .unwrap(),
            "supplied future cutoff cannot bypass actual DB grace"
        );
        assert!(tx
            .operation()
            .maintenance_anonymize_withdrawn(f.user, now, cutoff)
            .await
            .unwrap());
        tx.operation().restore_system(previous).await.unwrap();
        tx.commit().await.unwrap();
        let erased: (String,Option<String>,Option<String>,Option<String>,i64,Option<i64>) = sqlx::query_as("SELECT given_name,family_name,password_hash,withdraw_cancel_token_hash,auth_generation,anonymized_at FROM users WHERE id=?1")
            .bind(f.user.as_bytes().as_slice()).fetch_one(&f.pool).await.unwrap();
        assert_eq!(
            erased,
            (
                Message::WithdrawnDisplayName.default_text().to_string(),
                None,
                None,
                None,
                1,
                Some(now.timestamp_micros())
            )
        );
        let email: String = sqlx::query_scalar("SELECT email FROM users WHERE id=?1")
            .bind(f.user.as_bytes().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert!(email.starts_with("withdrawn-") && email.ends_with("@withdrawn.invalid"));
        let credentials: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions WHERE user_id=?1")
            .bind(f.user.as_bytes().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(credentials, 0);
        let personal: (Option<i64>,i64) = sqlx::query_as("SELECT deleted_at,(SELECT count(*) FROM memberships WHERE workspace_id=?1) FROM workspaces WHERE id=?1")
            .bind(f.workspace.as_bytes().as_slice()).fetch_one(&f.pool).await.unwrap();
        assert!(personal.0.is_some());
        assert_eq!(personal.1, 0);
        let events: Vec<(String, String)> =
            sqlx::query_as("SELECT verb,channel FROM events ORDER BY seq")
                .fetch_all(&f.pool)
                .await
                .unwrap();
        assert_eq!(
            events,
            vec![
                ("workspace.deleted".into(), "system".into()),
                ("user.anonymized".into(), "system".into())
            ]
        );
        let audits: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM audit_log WHERE verb IN ('workspace.deleted','user.anonymized')",
        )
        .fetch_one(&f.pool)
        .await
        .unwrap();
        assert_eq!(audits, 2);
        let other: Option<i64> = sqlx::query_scalar("SELECT anonymized_at FROM users WHERE id=?1")
            .bind(f.other_user.as_bytes().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(other, None);
        f.backend.close().await.unwrap();
        std::fs::remove_dir_all(f.dir).unwrap();
    }
}
