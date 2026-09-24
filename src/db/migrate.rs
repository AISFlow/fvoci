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
];

const MIGRATION_LOCK_KEY: i64 = 847_291_003_552;

const APP_ROLE_GRANTS: &str = include_str!("../../scripts/grant-app-role.sql");
const APP_ROLE_PLACEHOLDER: &str = ":\"app_role\"";

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
        WHERE n.nspname IN ('fvoci', 'public')
          AND p.prosecdef
          AND p.proowner = (SELECT oid FROM pg_roles WHERE rolname = current_user)
    LOOP
        EXECUTE format('REVOKE EXECUTE ON FUNCTION %s FROM PUBLIC', definer);
    END LOOP;
END
$$;
"#;

pub async fn run_migrations(url: &str) -> Result<(), sqlx::Error> {
    let pool = PgPoolOptions::new().max_connections(2).connect(url).await?;
    let result = apply_migrations(&pool).await;
    pool.close().await;
    result
}

async fn apply_migrations(pool: &PgPool) -> Result<(), sqlx::Error> {
    for (sql, version) in MIGRATIONS {
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
            Self::Db(error) => write!(f, "app role grant failed and was rolled back: {error}"),
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
    let is_owner: bool = sqlx::query_scalar("SELECT $1 = current_user::text")
        .bind(role)
        .fetch_one(&mut *tx)
        .await?;
    if is_owner {
        return Err(GrantError::InvalidRole(
            "the app role must differ from the migration owner".into(),
        ));
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
                  'document_collab_updates', 'document_collab_op_receipts', 'attachments'
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
    fn grant_sql_quotes_role_and_replaces_every_placeholder() {
        let sql = app_role_grant_sql("app\"x");
        assert!(!sql.contains(APP_ROLE_PLACEHOLDER));
        assert!(sql.contains("TO \"app\"\"x\";"));
        assert!(!sql.contains("BEGIN") && !sql.contains("COMMIT"));
    }
}
