use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

const MIGRATIONS: &[(&str, i32)] = &[
    (include_str!("../../migrations/001_schema.sql"), 1),
    (include_str!("../../migrations/002_functions.sql"), 2),
    (include_str!("../../migrations/003_workspace.sql"), 3),
    (include_str!("../../migrations/004_documents.sql"), 4),
    (include_str!("../../migrations/005_collab_updates.sql"), 5),
    (include_str!("../../migrations/006_attachments.sql"), 6),
    (include_str!("../../migrations/007_attachment_extract.sql"), 7),
];

const MIGRATION_LOCK_KEY: i64 = 847_291_003_552;

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
