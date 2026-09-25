use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

const MIGRATIONS: &[(&str, i32)] = &[
    (include_str!("../../migrations/001_schema.sql"), 1),
    (include_str!("../../migrations/002_functions.sql"), 2),
    (include_str!("../../migrations/003_workspace.sql"), 3),
    (include_str!("../../migrations/004_documents.sql"), 4),
    (include_str!("../../migrations/005_collab_updates.sql"), 5),
    (include_str!("../../migrations/006_attachments.sql"), 6),
    (
        include_str!("../../migrations/007_attachment_extract.sql"),
        7,
    ),
    (include_str!("../../migrations/008_projects.sql"), 8),
    (include_str!("../../migrations/009_invitations.sql"), 9),
    (include_str!("../../migrations/010_revisions.sql"), 10),
    (include_str!("../../migrations/011_comments.sql"), 11),
    (include_str!("../../migrations/012_api_tokens.sql"), 12),
    (include_str!("../../migrations/013_outbox.sql"), 13),
    (include_str!("../../migrations/014_search_index.sql"), 14),
    (include_str!("../../migrations/015_task_labels.sql"), 15),
    (include_str!("../../migrations/016_groups.sql"), 16),
    (include_str!("../../migrations/017_task_milestones.sql"), 17),
    (include_str!("../../migrations/018_notifications.sql"), 18),
    (include_str!("../../migrations/019_schedule_ics.sql"), 19),
    (include_str!("../../migrations/020_mail_reset.sql"), 20),
    (include_str!("../../migrations/021_maintenance_gc.sql"), 21),
    (include_str!("../../migrations/022_task_activity.sql"), 22),
];

const MIGRATION_LOCK_KEY: i64 = 847_291_003_552;

const APP_ROLE_GRANTS: &str = include_str!("../../scripts/grant-app-role.sql");
const APP_ROLE_PLACEHOLDER: &str = ":\"app_role\"";

pub const SCHEMA_GATE_OPERATOR_HINT: &str =
    "run `fvoci-migrate` then `fvoci-migrate --grant-app-role <app-role>` before starting fvoci-server";

/// Latest migration version compiled into this binary.
pub fn latest_migration_version() -> i32 {
    MIGRATIONS.last().map(|(_, version)| *version).unwrap_or(0)
}

pub fn schema_version_gate(actual: Option<i32>, expected: i32) -> Result<(), String> {
    match actual {
        Some(version) if version == expected => Ok(()),
        Some(version) if version > expected => Err(format!(
            "database schema version {version} is newer than this binary ({expected}); deploy a matching fvoci-server"
        )),
        Some(version) => Err(format!(
            "database schema version {version} is behind compiled version {expected}; {SCHEMA_GATE_OPERATOR_HINT}"
        )),
        None => Err(format!(
            "database has no applied migrations (expected version {expected}); {SCHEMA_GATE_OPERATOR_HINT}"
        )),
    }
}

/// Verifies the connected database matches the compiled migration set.
pub async fn assert_schema_current(pool: &PgPool) -> Result<(), String> {
    let expected = latest_migration_version();
    let actual =
        sqlx::query_scalar::<_, Option<i32>>("SELECT max(version) FROM fvoci.schema_migrations")
            .fetch_one(pool)
            .await
            .map_err(|error| {
                format!(
                    "cannot read fvoci.schema_migrations ({error}); {SCHEMA_GATE_OPERATOR_HINT}"
                )
            })?;
    schema_version_gate(actual, expected)
}

// PostgreSQL grants EXECUTE to PUBLIC when a function is created. Revoke it for
// the migration owner's SECURITY DEFINER functions inside the same transaction
// that created them, so no window exists before grant-app-role.sql runs.
const REVOKE_PUBLIC_DEFINER_EXECUTE: &str = r#"
DO $$
DECLARE
    definer regprocedure;
BEGIN
    FOR definer IN
        SELECT p.oid::regprocedure
        FROM pg_proc p
        INNER JOIN pg_namespace n ON n.oid = p.pronamespace
        WHERE (n.nspname = 'fvoci' OR (n.nspname = 'public' AND p.proname LIKE 'app\_%'))
          AND p.prosecdef
          AND p.proowner = (SELECT oid FROM pg_roles WHERE rolname = current_user)
    LOOP
        EXECUTE format('REVOKE EXECUTE ON ROUTINE %s FROM PUBLIC', definer);
    END LOOP;
END
$$;
"#;

pub async fn run_migrations(url: &str) -> Result<(), sqlx::Error> {
    run_migrations_through(url, i32::MAX).await
}

pub async fn run_migrations_through(url: &str, max_version: i32) -> Result<(), sqlx::Error> {
    let pool = PgPoolOptions::new().max_connections(2).connect(url).await?;
    let result = apply_migrations(&pool, max_version).await;
    pool.close().await;
    result
}

async fn apply_migrations(pool: &PgPool, max_version: i32) -> Result<(), sqlx::Error> {
    for (sql, version) in MIGRATIONS {
        if *version > max_version {
            continue;
        }
        let mut tx = pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(MIGRATION_LOCK_KEY)
            .execute(&mut *tx)
            .await?;

        let bootstrapped: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                SELECT 1 FROM information_schema.tables
                WHERE table_schema = 'fvoci' AND table_name = 'schema_migrations'
            )",
        )
        .fetch_one(&mut *tx)
        .await?;

        if bootstrapped {
            let applied: Option<i32> = sqlx::query_scalar(
                "SELECT version FROM fvoci.schema_migrations WHERE version = $1",
            )
            .bind(version)
            .fetch_optional(&mut *tx)
            .await?;
            if applied.is_some() {
                tx.rollback().await?;
                continue;
            }
        }

        sqlx::raw_sql(sql).execute(&mut *tx).await?;
        sqlx::raw_sql(REVOKE_PUBLIC_DEFINER_EXECUTE)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO fvoci.schema_migrations (version) VALUES ($1) ON CONFLICT DO NOTHING",
        )
        .bind(version)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
    }

    Ok(())
}

#[derive(Debug)]
pub enum GrantError {
    Db(sqlx::Error),
    InvalidRole(String),
}

impl std::fmt::Display for GrantError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Db(error) => write!(
                f,
                "app role grant failed; no privileges were committed: {error}"
            ),
            Self::InvalidRole(reason) => write!(f, "app role grant refused: {reason}"),
        }
    }
}

impl std::error::Error for GrantError {}

impl From<sqlx::Error> for GrantError {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(error)
    }
}

/// Returns scripts/grant-app-role.sql with the psql `:"app_role"` variable
/// replaced by a quoted identifier.
pub fn app_role_grant_sql(role: &str) -> String {
    let quoted = format!("\"{}\"", role.replace('"', "\"\""));
    APP_ROLE_GRANTS.replace(APP_ROLE_PLACEHOLDER, &quoted)
}

/// Applies scripts/grant-app-role.sql for `role` as one transaction through the
/// migration owner URL. Any failure rolls back every statement, so the broad
/// table grant never survives without the narrowing revokes that follow it.
pub async fn grant_app_role(url: &str, role: &str) -> Result<(), GrantError> {
    let pool = PgPoolOptions::new().max_connections(1).connect(url).await?;
    let result = apply_app_role_grants(&pool, role).await;
    pool.close().await;
    result
}

pub async fn apply_app_role_grants(pool: &PgPool, role: &str) -> Result<(), GrantError> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(MIGRATION_LOCK_KEY)
        .execute(&mut *tx)
        .await?;
    let found: Option<(bool, bool)> =
        sqlx::query_as("SELECT rolsuper, rolbypassrls FROM pg_roles WHERE rolname = $1")
            .bind(role)
            .fetch_optional(&mut *tx)
            .await?;
    match found {
        None => {
            return Err(GrantError::InvalidRole(format!(
                "role {role:?} does not exist"
            )))
        }
        Some((true, _)) | Some((_, true)) => {
            return Err(GrantError::InvalidRole(format!(
                "role {role:?} must be non-superuser without BYPASSRLS"
            )))
        }
        Some(_) => {}
    }
    // The script revokes privileges from `role`; applied to an owner it would
    // strip the ACL that the SECURITY DEFINER functions run under.
    let owns_schema_objects: bool = sqlx::query_scalar(
        r#"
        SELECT $1 = current_user::text
            OR pg_has_role($1, n.nspowner, 'MEMBER')
            OR EXISTS (
                SELECT 1 FROM pg_class c
                WHERE c.relnamespace = n.oid AND pg_has_role($1, c.relowner, 'MEMBER')
            )
            OR EXISTS (
                SELECT 1 FROM pg_proc p
                INNER JOIN pg_namespace pn ON pn.oid = p.pronamespace
                WHERE (pn.oid = n.oid OR (pn.nspname = 'public' AND p.proname LIKE 'app\_%'))
                  AND pg_has_role($1, p.proowner, 'MEMBER')
            )
        FROM pg_namespace n
        WHERE n.nspname = 'fvoci'
        "#,
    )
    .bind(role)
    .fetch_one(&mut *tx)
    .await?;
    if owns_schema_objects {
        return Err(GrantError::InvalidRole(format!(
            "role {role:?} owns or inherits ownership of fvoci objects; use a separate app role"
        )));
    }
    sqlx::raw_sql(&app_role_grant_sql(role))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn assert_app_role(pool: &PgPool) -> Result<(), String> {
    let row: (bool, bool) =
        sqlx::query_as("SELECT rolsuper, rolbypassrls FROM pg_roles WHERE rolname = current_user")
            .fetch_one(pool)
            .await
            .map_err(|e| e.to_string())?;
    if row.0 || row.1 {
        return Err("DATABASE_APP_URL must use a non-superuser role without BYPASSRLS".into());
    }

    let owns_schema: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM pg_namespace n
            WHERE n.nspname = 'fvoci'
              AND pg_get_userbyid(n.nspowner) = current_user
        )
        "#,
    )
    .fetch_one(pool)
    .await
    .map_err(|e| e.to_string())?;
    if owns_schema {
        return Err("application role must not own the fvoci schema".into());
    }

    let owns_protected: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM pg_class c
            INNER JOIN pg_namespace n ON n.oid = c.relnamespace
            WHERE n.nspname = 'fvoci'
              AND c.relname IN (
                  'schema_migrations', 'users', 'workspaces', 'memberships',
                  'sessions', 'events', 'audit_log', 'documents', 'document_states',
                  'document_collab_updates', 'document_collab_op_receipts', 'attachments',
                  'projects', 'project_members', 'workflows', 'statuses', 'tasks',
                  'invitations', 'revisions', 'comments', 'api_tokens', 'outbox_consumers',
                  'outbox_failures', 'processed_events', 'attachment_text', 'labels',
                  'task_assignees', 'task_labels', 'milestones', 'task_dependencies',
                  'notifications', 'notification_prefs', 'workspace_holidays', 'ics_tokens',
                  'magic_tokens', 'task_activity'
              )
              AND pg_get_userbyid(c.relowner) = current_user
        )
        "#,
    )
    .fetch_one(pool)
    .await
    .map_err(|e| e.to_string())?;
    if owns_protected {
        return Err("application role must not own protected fvoci tables".into());
    }

    let inherits_owner: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM pg_namespace n
            CROSS JOIN LATERAL (
                SELECT pg_get_userbyid(n.nspowner) AS owner_name
            ) owner
            WHERE n.nspname = 'fvoci'
              AND pg_has_role(current_user, owner.owner_name, 'MEMBER')
              AND owner.owner_name <> current_user
        )
        "#,
    )
    .fetch_one(pool)
    .await
    .map_err(|e| e.to_string())?;
    if inherits_owner {
        return Err("application role must not inherit the migration owner role".into());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    // Applied migrations are skipped by version, so editing an accepted file would
    // silently diverge existing installations. Add a new migration instead and
    // append its digest here when it is accepted.
    const ACCEPTED_MIGRATION_SHA256: &[(i32, &str)] = &[
        (
            1,
            "1ed4ce976b341c67fca4932c59f4de151e904697df7ca5ecef3c15d25b2d1680",
        ),
        (
            2,
            "c929fff47207a0fa8d59dd18cb7ad28b36101eab91aba34e744f4e03e9bf1f4e",
        ),
        (
            3,
            "dd8741c56d5e2a2b3ab2fd177c72ee506b82eaa1a0c44cd3ac54c31865e292b0",
        ),
        (
            4,
            "a1a3720d7d57649779093c3eee7fd7f3d2e09e3a9870295c6a5c278e6e19d1ec",
        ),
        (
            5,
            "320f5c989410c0e03f05ec5056e96fb55854b54d8452ccb7f197ef5a7523989b",
        ),
        (
            6,
            "f567d29c7deca648adc0b2b80dda54feae59906bb02453fb62e729df788772d8",
        ),
        (
            7,
            "36752ac87f153e689ba298b5cfab6c9a1375b34b1ec3dc839942d36da8c0b0fa",
        ),
        (
            8,
            "03a94479c2f4e44f2a79e1dfe30f5f3c697eccfb35525b34dacc21f243bb030b",
        ),
        (
            9,
            "640c6063ba24cbe90a00dbd98cf54945752265fbfceef9221ee027bfa26289b6",
        ),
        (
            10,
            "64594b33a13df1ed29479d4b07d692850e0c806b0b76749cc0316e3eaea8dfa8",
        ),
        (
            11,
            "ab17b10baa2533ebccaf994ae5c4d1d0f1ce7bc82a56f9e77371c483f48bf3e5",
        ),
        (
            12,
            "b8964ac88a75c2b5e4264c25de083810971e350550214c61bdd08de6ed695912",
        ),
        (
            13,
            "2ac42b2dc796dddd23d48574135c9e0c50c60cb55c6c3d9960b7faffe9c66708",
        ),
        (
            14,
            "c772193b7b97c1484d6339c69c32a18d8b46644d3cc9700db1b7222157ec0637",
        ),
        (
            15,
            "8b6a424957ef8a5a67804ea8b3f3d9990bbff43c91ab4dad10f2b682e864ce35",
        ),
        (
            16,
            "68dcd7c2f0edd54ee4ba4638dda9edbceef4e58b6174e537fb92c9abfe006ff5",
        ),
        (
            17,
            "f09ba97f766cfa3265c7383b9fd91c6f6fe747581bf7156807f5beffdda84846",
        ),
        (
            18,
            "5999d2f08f5f62f274c445e965bdf938c84b90b52b5557fa60b81f5e29ba002c",
        ),
        (
            19,
            "5da12540460cbda0b823d7e63025ab8e3b371c90cfa1acb09b7b342a8f9b4fc6",
        ),
        (
            20,
            "cd4e1ec73457fed4f759ddbeff9d998a690afde5c3c4f6779ea20b45baf13fb4",
        ),
        (
            21,
            "7a9e03487d30c9e92e4f8b93e69d0bbc1b1ac20771161c6a71ba8ed48a90a50c",
        ),
        (
            22,
            "62ed9fa9bdc77189975c11ddf420a4dccf79ee039f6d2548355f85e6ed6eba3c",
        ),
    ];

    #[test]
    fn accepted_migrations_are_immutable_and_ordered() {
        assert_eq!(MIGRATIONS.len(), ACCEPTED_MIGRATION_SHA256.len());
        for ((sql, version), (expected_version, expected)) in
            MIGRATIONS.iter().zip(ACCEPTED_MIGRATION_SHA256)
        {
            assert_eq!(version, expected_version, "migration order changed");
            let digest: String = Sha256::digest(sql.as_bytes())
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            assert_eq!(
                &digest, expected,
                "accepted migration {version:03} changed; add a new migration instead"
            );
        }
    }

    #[test]
    fn schema_version_gate_distinguishes_ahead_behind_and_empty() {
        let expected = latest_migration_version();
        assert!(schema_version_gate(Some(expected), expected).is_ok());
        let behind = schema_version_gate(Some(expected - 1), expected).unwrap_err();
        assert!(
            behind.contains(&format!("behind compiled version {expected}")),
            "{behind}"
        );
        assert!(behind.contains(SCHEMA_GATE_OPERATOR_HINT), "{behind}");
        let ahead = schema_version_gate(Some(expected + 1), expected).unwrap_err();
        assert!(
            ahead.contains(&format!("newer than this binary ({expected})")),
            "{ahead}"
        );
        assert!(ahead.contains("deploy a matching fvoci-server"), "{ahead}");
        assert!(!ahead.contains("fvoci-migrate"), "{ahead}");
        let empty = schema_version_gate(None, expected).unwrap_err();
        assert!(empty.contains("no applied migrations"), "{empty}");
        assert!(empty.contains(SCHEMA_GATE_OPERATOR_HINT), "{empty}");
    }

    #[test]
    fn grant_sql_quotes_role_and_replaces_every_placeholder() {
        let sql = app_role_grant_sql("app\"x");
        assert!(!sql.contains(APP_ROLE_PLACEHOLDER));
        assert!(sql.contains("TO \"app\"\"x\";"));
        assert!(!sql.contains("BEGIN") && !sql.contains("COMMIT"));
    }
}
