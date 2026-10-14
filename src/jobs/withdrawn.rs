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

/// The owner of a personal workspace is the only lead of its private project.
/// The family sweep reaches the same erasure as PostgreSQL
/// (`account_lifecycle` `maintenance_sweep_erases_withdrawn_user_with_private_personal_project`):
/// every project there carries the workspace stamp and the daily sweep purges
/// the workspace, while another personal workspace and the team workspace,
/// where the erased user still leads a private project, do not change.
#[cfg(test)]
pub(crate) mod personal_project_erasure {
    use super::*;
    use crate::attachments::ObjectStorage;
    use crate::db::backend::{Backend, DbTransaction};
    use crate::db::codec::Cell;
    use crate::jobs::family_maintenance_fixture::{acquired, policy};
    use crate::jobs::{FamilyMaintenanceClaimRequest, MaintenanceJobKey};
    use crate::mail::Mailer;
    use chrono::SubsecRound;
    use uuid::Uuid;

    /// (anonymized, placeholder email, sessions, identity links, personal
    /// workspace deleted, its memberships, its project grants, its live
    /// projects, its projects with the workspace stamp)
    type Erasure = (bool, bool, i64, i64, bool, i64, i64, i64, i64);
    /// Per workspace: (live, memberships, project grants, live projects,
    /// projects), then whether the other user is still unerased.
    type Untouched = (Vec<(bool, i64, i64, i64, i64)>, bool);

    struct Seeded {
        due: Uuid,
        keep: Uuid,
        personal: Uuid,
        kept: Uuid,
        team: Uuid,
    }

    async fn write(backend: &Backend, statements: &[(&'static str, Vec<Cell>)]) {
        let mut tx = backend.begin_write().await.unwrap();
        let DbTransaction::SqliteFamily(f) = &mut tx else {
            unreachable!()
        };
        f.replace_system_context(true);
        for (sql, params) in statements {
            f.execute(sql, params).await.unwrap();
        }
        tx.commit_with_cleanup().await.unwrap();
    }

    /// The first row's first `width` cells.
    async fn read(backend: &Backend, sql: &'static str, id: Uuid, width: usize) -> Vec<Cell> {
        let mut tx = backend.begin_read().await.unwrap();
        let DbTransaction::SqliteFamily(f) = &mut tx else {
            unreachable!()
        };
        f.replace_system_context(true);
        let rows = f.query(sql, &[Cell::uuid(id)]).await.unwrap();
        let cells = (0..width).map(|i| rows[0].cell(i).unwrap()).collect();
        tx.rollback().await.unwrap();
        cells
    }

    async fn seed(backend: &Backend, now: DateTime<Utc>) -> Seeded {
        let s = Seeded {
            due: Uuid::now_v7(),
            keep: Uuid::now_v7(),
            personal: Uuid::now_v7(),
            kept: Uuid::now_v7(),
            team: Uuid::now_v7(),
        };
        let withdrawn =
            Cell::instant(now - Duration::days(WITHDRAW_GRACE_DAYS) - Duration::minutes(1))
                .unwrap();
        let mut statements = vec![
            ("INSERT INTO workspaces(id,slug,name,kind) VALUES(?1,'erasure-lead','Lead','personal')", vec![Cell::uuid(s.personal)]),
            ("INSERT INTO workspaces(id,slug,name,kind) VALUES(?1,'erasure-keep','Keep','personal')", vec![Cell::uuid(s.kept)]),
            ("INSERT INTO workspaces(id,slug,name) VALUES(?1,'erasure-team','Team')", vec![Cell::uuid(s.team)]),
            ("INSERT INTO users(id,email,given_name,password_hash,personal_workspace_id,deleted_at,withdraw_cancel_token_hash) VALUES(?1,'lead@erasure.invalid','Lead','old',?2,?3,'old-cancel')", vec![Cell::uuid(s.due), Cell::uuid(s.personal), withdrawn]),
            ("INSERT INTO users(id,email,given_name,personal_workspace_id) VALUES(?1,'keep@erasure.invalid','Keep',?2)", vec![Cell::uuid(s.keep), Cell::uuid(s.kept)]),
            ("INSERT INTO sessions(id,user_id,token_hash,expires_at) VALUES(?1,?2,'erasure-lead',(unixepoch()+3600)*1000000)", vec![Cell::uuid(Uuid::now_v7()), Cell::uuid(s.due)]),
            ("INSERT INTO identity_links(id,user_id,provider,provider_user_id) VALUES(?1,?2,'google','erasure-lead')", vec![Cell::uuid(Uuid::now_v7()), Cell::uuid(s.due)]),
        ];
        for (ws, user, role) in [
            (s.personal, s.due, "owner"),
            (s.kept, s.keep, "owner"),
            (s.team, s.keep, "owner"),
            (s.team, s.due, "member"),
        ] {
            statements.push((
                "INSERT INTO memberships(workspace_id,user_id,role) VALUES(?1,?2,?3)",
                vec![Cell::uuid(ws), Cell::uuid(user), Cell::text(role)],
            ));
        }
        for (ws, user, key, visibility) in [
            (s.personal, s.due, "PRIV", "private"),
            (s.personal, s.due, "OPEN", "workspace"),
            (s.kept, s.keep, "KEEP", "private"),
            (s.team, s.due, "TEAM", "private"),
        ] {
            let project = Uuid::now_v7();
            statements.push((
                "INSERT INTO projects(id,workspace_id,key,name,visibility,created_by) VALUES(?1,?2,?3,?3,?4,?5)",
                vec![Cell::uuid(project), Cell::uuid(ws), Cell::text(key), Cell::text(visibility), Cell::uuid(user)],
            ));
            statements.push((
                "INSERT INTO project_members(id,workspace_id,project_id,user_id,role) VALUES(?1,?2,?3,?4,'lead')",
                vec![Cell::uuid(Uuid::now_v7()), Cell::uuid(ws), Cell::uuid(project), Cell::uuid(user)],
            ));
        }
        write(backend, &statements).await;
        s
    }

    async fn erasure(backend: &Backend, s: &Seeded) -> Erasure {
        let row = read(backend, "SELECT u.anonymized_at IS NOT NULL,u.email LIKE 'withdrawn-%@withdrawn.invalid',(SELECT count(*) FROM sessions x WHERE x.user_id=u.id),(SELECT count(*) FROM identity_links l WHERE l.user_id=u.id),w.deleted_at IS NOT NULL,(SELECT count(*) FROM memberships m WHERE m.workspace_id=w.id),(SELECT count(*) FROM project_members g WHERE g.workspace_id=w.id),(SELECT count(*) FROM projects p WHERE p.workspace_id=w.id AND p.deleted_at IS NULL),(SELECT count(*) FROM projects p WHERE p.workspace_id=w.id AND p.deleted_at=w.deleted_at) FROM users u JOIN workspaces w ON w.id=u.personal_workspace_id WHERE u.id=?1", s.due, 9).await;
        let b = |i: usize| row[i].boolean().unwrap();
        let n = |i: usize| row[i].integer().unwrap();
        (b(0), b(1), n(2), n(3), b(4), n(5), n(6), n(7), n(8))
    }

    async fn untouched(backend: &Backend, s: &Seeded) -> Untouched {
        let mut rows = Vec::new();
        for ws in [s.kept, s.team] {
            let row = read(backend, "SELECT w.deleted_at IS NULL,(SELECT count(*) FROM memberships m WHERE m.workspace_id=w.id),(SELECT count(*) FROM project_members g WHERE g.workspace_id=w.id),(SELECT count(*) FROM projects p WHERE p.workspace_id=w.id AND p.deleted_at IS NULL),(SELECT count(*) FROM projects p WHERE p.workspace_id=w.id) FROM workspaces w WHERE w.id=?1", ws, 5).await;
            let n = |i: usize| row[i].integer().unwrap();
            rows.push((row[0].boolean().unwrap(), n(1), n(2), n(3), n(4)));
        }
        let live = read(
            backend,
            "SELECT count(*) FROM users WHERE id=?1 AND deleted_at IS NULL AND anonymized_at IS NULL",
            s.keep,
            1,
        )
        .await;
        (rows, live[0].integer().unwrap() == 1)
    }

    pub(crate) async fn sweep_erases_personal_private_project(
        backend: &Backend,
        root: &std::path::Path,
    ) {
        let now = Utc::now().trunc_subsecs(6);
        let s = seed(backend, now).await;
        // Before: the owner's grants are the only leads of both projects.
        let before = erasure(backend, &s).await;
        assert_eq!(
            (before.4, before.5, before.6, before.7, before.8),
            (false, 1, 2, 2, 0)
        );
        let others = untouched(backend, &s).await;

        let request = FamilyMaintenanceClaimRequest::new(MaintenanceJobKey::Daily);
        let claim = acquired(&request, backend).await;
        let erased = run_withdrawn_anonymize_family(
            backend,
            Utc::now().trunc_subsecs(6),
            claim.proof(),
            policy(),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        claim.release().await.unwrap();
        assert_eq!(erased, 1);
        assert_eq!(
            erasure(backend, &s).await,
            (true, true, 0, 0, true, 0, 0, 0, 2)
        );
        assert_eq!(untouched(backend, &s).await, others);

        let storage = ObjectStorage::local(root.join("storage"));
        let stats = super::super::run_daily_sweep_family(
            backend,
            &storage,
            &Mailer::disabled(),
            &request,
            policy(),
            &CancellationToken::new(),
        )
        .await
        .unwrap()
        .expect("claimed daily sweep");
        assert_eq!(
            (stats.withdrawn_anonymized, stats.workspace.purged),
            (0, 1),
            "{stats:?}"
        );
        let purged = read(
            backend,
            "SELECT (SELECT count(*) FROM workspaces WHERE id=?1),(SELECT count(*) FROM projects WHERE workspace_id=?1)",
            s.personal,
            2,
        )
        .await;
        assert_eq!(
            (purged[0].integer().unwrap(), purged[1].integer().unwrap()),
            (0, 0)
        );
        assert_eq!(untouched(backend, &s).await, others);
    }

    #[tokio::test]
    async fn sweep_erases_withdrawn_user_with_private_personal_project() {
        let f = crate::jobs::family_maintenance_fixture::Fixture::new().await;
        sweep_erases_personal_private_project(&f.backend, &f.root).await;
        f.finish().await;
    }
}

#[cfg(all(test, feature = "db-tests"))]
mod personal_project_erasure_sqld {
    use crate::db::backend::{Backend, RemoteDatabase};
    use crate::db::libsql_finish_fixture::LibsqlFinishFixture;
    use futures_util::FutureExt;
    use uuid::Uuid;

    #[tokio::test]
    #[ignore = "explicit pinned official sqld selection required"]
    async fn remote_sweep_erases_withdrawn_user_with_private_personal_project() {
        let sqld = std::path::PathBuf::from(
            std::env::var_os("FVOCI_TEST_SQLD").expect("pinned official sqld required; no skip"),
        );
        let root = std::env::temp_dir().join(format!("fvoci-erasure-sqld-{}", Uuid::now_v7()));
        let driver = LibsqlFinishFixture::start(&sqld, &root).await.unwrap();
        let backend = Backend::LibsqlRemote(std::sync::Arc::new(RemoteDatabase::from_test_driver(
            driver.database().await.unwrap(),
            std::num::NonZeroU32::new(2).unwrap(),
        )));
        let result = std::panic::AssertUnwindSafe(async {
            crate::db::migrate::initialize_family_backend_for_test(&backend)
                .await
                .unwrap();
            super::personal_project_erasure::sweep_erases_personal_private_project(&backend, &root)
                .await;
        })
        .catch_unwind()
        .await;
        let close = backend.close().await;
        let finish = driver.finish().await;
        assert!(close.is_ok());
        let receipt = finish.unwrap();
        assert!(receipt.sqld_reaped && receipt.proxy_joined);
        std::fs::remove_dir_all(root).unwrap();
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }
}
