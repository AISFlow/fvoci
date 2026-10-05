use chrono::{DateTime, Utc};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;

use crate::db::backend::OperationTx;
use crate::db::codec::Cell;
use crate::db::context::set_system;

pub const TOKEN_GC_BATCH: i64 = 5_000;

pub async fn run_ics_token_gc(
    pool: &PgPool,
    now: DateTime<Utc>,
    cancel: &CancellationToken,
) -> Result<u32, sqlx::Error> {
    if cancel.is_cancelled() {
        return Ok(0);
    }
    let mut tx = pool.begin().await?;
    set_system(&mut tx).await?;
    let deleted = OperationTx::Postgres(&mut tx)
        .maintenance_ics_token_gc(now)
        .await?;
    tx.commit().await?;
    Ok(deleted)
}

impl OperationTx<'_, '_> {
    /// The maintenance consumer owns BEGIN, authority/proof checks and COMMIT.
    /// This operation borrows that same writer and does not grant system scope.
    pub(crate) async fn maintenance_ics_token_gc(
        &mut self,
        now: DateTime<Utc>,
    ) -> Result<u32, sqlx::Error> {
        let deleted = match self {
            Self::Postgres(tx) => sqlx::query(
                r#"
        WITH doomed AS (
            SELECT id
            FROM fvoci.ics_tokens
            WHERE expires_at IS NOT NULL
              AND expires_at <= $1
            ORDER BY expires_at ASC, id ASC
            LIMIT $2
            FOR UPDATE SKIP LOCKED
        )
        DELETE FROM fvoci.ics_tokens AS t
        USING doomed
        WHERE t.id = doomed.id
        "#,
            )
            .bind(now)
            .bind(TOKEN_GC_BATCH)
            .execute(&mut ***tx)
            .await?
            .rows_affected(),
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_system_context()?;
                tx.execute(
                    "DELETE FROM ics_tokens WHERE id IN (SELECT id FROM ics_tokens WHERE expires_at IS NOT NULL AND expires_at <= ?1 ORDER BY expires_at, id LIMIT ?2)",
                    &[Cell::instant(now)?, Cell::Integer(TOKEN_GC_BATCH)],
                ).await?
            }
        };
        u32::try_from(deleted)
            .map_err(|_| sqlx::Error::Protocol("ICS cleanup count overflow".into()))
    }

    pub(crate) async fn maintenance_magic_token_gc(
        &mut self,
        now: DateTime<Utc>,
    ) -> Result<u32, sqlx::Error> {
        match self {
            Self::Postgres(tx) => {
                let deleted: i32 =
                    sqlx::query_scalar("SELECT fvoci.app_magic_purge_expired($1, $2)")
                        .bind(now)
                        .bind(TOKEN_GC_BATCH as i32)
                        .fetch_one(&mut ***tx)
                        .await?;
                let ephemeral: i32 =
                    sqlx::query_scalar("SELECT fvoci.app_auth_ephemeral_purge_expired($1, $2)")
                        .bind(now)
                        .bind(TOKEN_GC_BATCH as i32)
                        .fetch_one(&mut ***tx)
                        .await?;
                Ok(deleted.max(0) as u32 + ephemeral.max(0) as u32)
            }
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_system_context()?;
                let parameters = [Cell::instant(now)?, Cell::Integer(TOKEN_GC_BATCH)];
                // PG021 and PG029 each bound their own table. These deletes
                // remain atomic in the original caller-owned transaction.
                let magic = tx.execute("DELETE FROM magic_tokens WHERE token_hash IN (SELECT token_hash FROM magic_tokens WHERE expires_at <= ?1 ORDER BY expires_at, token_hash LIMIT ?2)", &parameters).await?;
                let mfa = tx.execute("DELETE FROM mfa_challenges WHERE token_hash IN (SELECT token_hash FROM mfa_challenges WHERE expires_at <= ?1 ORDER BY expires_at, token_hash LIMIT ?2)", &parameters).await?;
                let oidc = tx.execute("DELETE FROM oidc_states WHERE state_hash IN (SELECT state_hash FROM oidc_states WHERE expires_at <= ?1 ORDER BY expires_at, state_hash LIMIT ?2)", &parameters).await?;
                u32::try_from(magic + mfa + oidc)
                    .map_err(|_| sqlx::Error::Protocol("magic cleanup count overflow".into()))
            }
        }
    }
}

pub async fn run_magic_token_gc(
    pool: &PgPool,
    now: DateTime<Utc>,
    cancel: &CancellationToken,
) -> Result<u32, sqlx::Error> {
    if cancel.is_cancelled() {
        return Ok(0);
    }
    let mut tx = pool.begin().await?;
    set_system(&mut tx).await?;
    let deleted = OperationTx::Postgres(&mut tx)
        .maintenance_magic_token_gc(now)
        .await?;
    tx.commit().await?;
    Ok(deleted)
}

pub(crate) enum TokenCleanup {
    Ics,
    Magic,
}

pub(crate) async fn run_token_gc_family(
    backend: &crate::db::backend::Backend,
    now: DateTime<Utc>,
    proof: &super::claim::FamilyMaintenanceProof,
    policy: super::claim::FamilyMaintenanceLeasePolicy,
    cancel: &CancellationToken,
    which: TokenCleanup,
) -> Result<u32, super::MaintenanceConsumerError> {
    use super::{commit_maintenance_writer, renew_maintenance_writer, rollback_maintenance_writer};
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
        let deleted = match which {
            TokenCleanup::Ics => tx.operation().maintenance_ics_token_gc(now).await?,
            TokenCleanup::Magic => tx.operation().maintenance_magic_token_gc(now).await?,
        };
        tx.operation().restore_system(previous).await?;
        Ok::<_, super::MaintenanceConsumerError>(deleted)
    }
    .await;
    let deleted = match result {
        Ok(deleted) => deleted,
        Err(err) => return Err(rollback_maintenance_writer(tx, err).await),
    };
    commit_maintenance_writer(tx, proof, super::claim::MaintenanceJobKey::Daily, cancel).await?;
    Ok(deleted)
}

#[cfg(test)]
mod family_tests {
    use super::*;
    use crate::db::notifications::family_runtime_fixture::Fixture;
    use uuid::Uuid;

    #[tokio::test]
    async fn maintenance_token_consumer_wrong_expired_commit_cancel_and_healthy_current_proof() {
        use crate::jobs::claim::{
            FamilyMaintenanceClaimRequest, GlobalClaimAcquisition, GlobalJobClaim,
        };
        use crate::jobs::family_maintenance_fixture::{
            acquired, policy, Fixture as CurrentFixture,
        };
        use crate::jobs::{
            maintenance_test_hooks, FamilyMaintenanceLeasePolicy, MaintenanceConsumerError,
            MaintenanceJobKey,
        };
        let f = CurrentFixture::new().await;
        let now = DateTime::from_timestamp_micros(1_800_000_000_000_000).unwrap();
        let seed = |id: Uuid| {
            sqlx::query("INSERT INTO ics_tokens(id,workspace_id,user_id,token_hash,expires_at) VALUES(?1,?2,?3,?4,1)")
            .bind(id.as_bytes().to_vec()).bind(f.workspace.as_bytes().to_vec()).bind(f.user.as_bytes().to_vec()).bind(id.to_string())
        };
        seed(Uuid::now_v7()).execute(&f.pool).await.unwrap();
        // This real trigger distinguishes initial denial from mutating first
        // and merely rolling back at the final check. Restore canonical schema
        // before testing healthy progress; never weaken the schema gate.
        sqlx::raw_sql("CREATE TRIGGER maintenance_forbidden_token_delete BEFORE DELETE ON ics_tokens BEGIN SELECT RAISE(ABORT,'forbidden initial token effect'); END;")
            .execute(&f.pool).await.unwrap();
        let wrong_request = FamilyMaintenanceClaimRequest::new(MaintenanceJobKey::Uploads);
        let wrong = acquired(&wrong_request, &f.backend).await;
        assert!(matches!(
            run_token_gc_family(
                &f.backend,
                now,
                wrong.proof(),
                policy(),
                &CancellationToken::new(),
                TokenCleanup::Ics
            )
            .await,
            Err(MaintenanceConsumerError::OwnershipLost)
        ));
        wrong.release().await.unwrap();
        let request = FamilyMaintenanceClaimRequest::new(MaintenanceJobKey::Daily);
        let old = acquired(&request, &f.backend).await;
        let stale = old.proof().clone();
        sqlx::query("UPDATE maintenance_job_claims SET expires_at=0 WHERE job_key=1")
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            run_token_gc_family(
                &f.backend,
                now,
                &stale,
                policy(),
                &CancellationToken::new(),
                TokenCleanup::Ics
            )
            .await,
            Err(MaintenanceConsumerError::OwnershipLost)
        ));
        let current = acquired(&request, &f.backend).await;
        assert_eq!(current.proof().generation(), stale.generation() + 1);
        assert_eq!(
            old.release().await.unwrap(),
            crate::jobs::claim::FamilyLeaseAction::Lost
        );
        assert!(
            matches!(
                run_token_gc_family(
                    &f.backend,
                    now,
                    &stale,
                    policy(),
                    &CancellationToken::new(),
                    TokenCleanup::Ics
                )
                .await,
                Err(MaintenanceConsumerError::OwnershipLost)
            ),
            "same prepared owner cannot bypass old generation"
        );
        sqlx::raw_sql("DROP TRIGGER maintenance_forbidden_token_delete")
            .execute(&f.pool)
            .await
            .unwrap();
        crate::db::migrate::assert_sqlite_schema_current(&f.backend)
            .await
            .unwrap();
        assert_eq!(
            run_token_gc_family(
                &f.backend,
                now,
                current.proof(),
                policy(),
                &CancellationToken::new(),
                TokenCleanup::Ics
            )
            .await
            .unwrap(),
            1
        );
        seed(Uuid::now_v7()).execute(&f.pool).await.unwrap();
        maintenance_test_hooks::arm_commit_fault(current.proof());
        let error = run_token_gc_family(
            &f.backend,
            now,
            current.proof(),
            policy(),
            &CancellationToken::new(),
            TokenCleanup::Ics,
        )
        .await
        .expect_err("the same real writer must reject deferred FK at COMMIT");
        let MaintenanceConsumerError::CommitUnknown(unknown) = error else {
            panic!("typed actual commit uncertainty required")
        };
        let sqlx::Error::Database(database) = unknown.source.source else {
            panic!("actual SQLite FK source required")
        };
        assert_eq!(database.code().as_deref(), Some("787"));
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM ics_tokens")
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert!(matches!(
            run_token_gc_family(
                &f.backend,
                now,
                current.proof(),
                policy(),
                &cancel,
                TokenCleanup::Ics
            )
            .await,
            Err(MaintenanceConsumerError::Cancelled)
        ));
        let proof = current.proof().clone();
        let (reached, proceed) = maintenance_test_hooks::arm_before_finish(&proof);
        let backend = f.backend.clone();
        let cancel = CancellationToken::new();
        let child = cancel.clone();
        let mut job = tokio::spawn(async move {
            run_token_gc_family(&backend, now, &proof, policy(), &child, TokenCleanup::Ics).await
        });
        maintenance_test_hooks::wait_reached(reached, &mut job).await;
        cancel.cancel();
        proceed.send(()).unwrap();
        assert!(matches!(
            job.await.unwrap(),
            Err(MaintenanceConsumerError::Cancelled)
        ));
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM ics_tokens")
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            count, 1,
            "late cancel after actual DELETE cannot acknowledge COMMIT"
        );
        assert_eq!(
            run_token_gc_family(
                &f.backend,
                now,
                current.proof(),
                policy(),
                &CancellationToken::new(),
                TokenCleanup::Ics
            )
            .await
            .unwrap(),
            1
        );
        current.release().await.unwrap();
        seed(Uuid::now_v7()).execute(&f.pool).await.unwrap();
        let short = FamilyMaintenanceLeasePolicy::new(
            std::time::Duration::from_secs(2),
            std::time::Duration::from_millis(500),
        )
        .unwrap();
        let short_request = FamilyMaintenanceClaimRequest::new(MaintenanceJobKey::Daily);
        let short_claim = match GlobalJobClaim::try_claim(
            &f.backend,
            &short_request,
            short,
            &CancellationToken::new(),
        )
        .await
        .unwrap()
        {
            GlobalClaimAcquisition::Acquired(GlobalJobClaim::Family(claim)) => claim,
            _ => panic!("actual short live owner required"),
        };
        let proof = short_claim.proof().clone();
        let (reached, proceed) = maintenance_test_hooks::arm_before_finish(&proof);
        let backend = f.backend.clone();
        let mut job = tokio::spawn(async move {
            run_token_gc_family(
                &backend,
                now,
                &proof,
                short,
                &CancellationToken::new(),
                TokenCleanup::Ics,
            )
            .await
        });
        maintenance_test_hooks::wait_reached(reached, &mut job).await;
        tokio::time::sleep(std::time::Duration::from_millis(2200)).await;
        proceed.send(()).unwrap();
        assert!(
            matches!(
                job.await.unwrap(),
                Err(MaintenanceConsumerError::OwnershipLost)
            ),
            "real DB clock expiry at held writer must reject late success"
        );
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM ics_tokens")
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
        let generation = short_claim.proof().generation();
        assert_eq!(
            short_claim.release().await.unwrap(),
            crate::jobs::claim::FamilyLeaseAction::Lost
        );
        let recovered = acquired(&short_request, &f.backend).await;
        assert_eq!(recovered.proof().generation(), generation + 1);
        assert_eq!(
            run_token_gc_family(
                &f.backend,
                now,
                recovered.proof(),
                policy(),
                &CancellationToken::new(),
                TokenCleanup::Ics
            )
            .await
            .unwrap(),
            1
        );
        recovered.release().await.unwrap();
        f.finish().await;
    }

    #[tokio::test]
    async fn maintenance_tokens_current_writer_context_expiry_and_failed_commit() {
        let f = Fixture::new().await;
        let fk: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(fk, 1);
        let now = DateTime::from_timestamp_micros(1_800_000_000_000_000).unwrap();
        let expired = Uuid::now_v7();
        let permanent = Uuid::now_v7();
        for (id, workspace, user, expiry) in [
            (expired, f.workspace, f.user, Some(now.timestamp_micros())),
            (permanent, f.other_workspace, f.other_user, None),
        ] {
            sqlx::query("INSERT INTO ics_tokens(id,workspace_id,user_id,token_hash,expires_at) VALUES(?1,?2,?3,?4,?5)")
                .bind(id.as_bytes().as_slice()).bind(workspace.as_bytes().as_slice())
                .bind(user.as_bytes().as_slice()).bind(id.to_string()).bind(expiry)
                .execute(&f.pool).await.unwrap();
        }
        for (hash, expiry) in [
            ("expired", now.timestamp_micros()),
            ("future", now.timestamp_micros() + 1),
        ] {
            sqlx::query("INSERT INTO magic_tokens(token_hash,kind,user_id,generation,expires_at) VALUES(?1,'login',?2,0,?3)")
                .bind(hash).bind(f.user.as_bytes().as_slice()).bind(expiry).execute(&f.pool).await.unwrap();
            sqlx::query("INSERT INTO mfa_challenges(token_hash,user_id,generation,expires_at) VALUES(?1,?2,0,?3)")
                .bind(hash).bind(f.user.as_bytes().as_slice()).bind(expiry).execute(&f.pool).await.unwrap();
            sqlx::query("INSERT INTO oidc_states(state_hash,payload,expires_at) VALUES(?1,'enc:v2:fixture',?2)")
                .bind(hash).bind(expiry).execute(&f.pool).await.unwrap();
        }
        let mut read = f.backend.begin_read().await.unwrap();
        read.operation().set_system().await.unwrap();
        assert!(read
            .operation()
            .maintenance_ics_token_gc(now)
            .await
            .is_err());
        read.rollback().await.unwrap();
        let mut denied = f.backend.begin_write().await.unwrap();
        assert!(denied
            .operation()
            .maintenance_magic_token_gc(now)
            .await
            .is_err());
        denied.rollback().await.unwrap();

        let mut tx = f.backend.begin_write().await.unwrap();
        let previous = tx.operation().set_system().await.unwrap();
        assert_eq!(
            tx.operation().maintenance_ics_token_gc(now).await.unwrap(),
            1
        );
        assert_eq!(
            tx.operation()
                .maintenance_magic_token_gc(now)
                .await
                .unwrap(),
            3
        );
        // A real deferred FK failure must roll back every table's deletion.
        let OperationTx::SqliteFamily(writer) = tx.operation() else {
            panic!("actual SQLite writer required")
        };
        writer
            .execute("PRAGMA defer_foreign_keys=ON", &[])
            .await
            .unwrap();
        writer
            .execute(
                "INSERT INTO memberships(workspace_id,user_id,role) VALUES(?1,?2,'guest')",
                &[Cell::uuid(Uuid::now_v7()), Cell::uuid(f.user)],
            )
            .await
            .unwrap();
        tx.operation().restore_system(previous).await.unwrap();
        let unknown = tx
            .commit()
            .await
            .expect_err("real FK rejection is mandatory");
        assert!(matches!(unknown.source, sqlx::Error::Database(_)));
        let tokens: i64 = sqlx::query_scalar("SELECT count(*) FROM ics_tokens")
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(tokens, 2);
        let magic: i64 = sqlx::query_scalar("SELECT count(*) FROM magic_tokens")
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(magic, 2);

        let mut retry = f.backend.begin_write().await.unwrap();
        let previous = retry.operation().set_system().await.unwrap();
        assert_eq!(
            retry
                .operation()
                .maintenance_ics_token_gc(now)
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            retry
                .operation()
                .maintenance_magic_token_gc(now)
                .await
                .unwrap(),
            3
        );
        retry.operation().restore_system(previous).await.unwrap();
        retry.commit().await.unwrap();
        let remaining: (i64,i64,i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM ics_tokens WHERE id=?1 AND expires_at IS NULL),(SELECT count(*) FROM magic_tokens WHERE token_hash='future'),(SELECT count(*) FROM mfa_challenges WHERE token_hash='future'),(SELECT count(*) FROM oidc_states WHERE state_hash='future')")
            .bind(permanent.as_bytes().as_slice()).fetch_one(&f.pool).await.unwrap();
        assert_eq!(remaining, (1, 1, 1, 1));
        f.backend.close().await.unwrap();
        std::fs::remove_dir_all(f.dir).unwrap();
    }
}
