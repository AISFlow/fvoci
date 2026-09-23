use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

const MIGRATIONS: &[(&str, i32)] = &[
    (include_str!("../../migrations/001_schema.sql"), 1),
    (include_str!("../../migrations/002_functions.sql"), 2),
];

pub async fn run_migrations(url: &str) -> Result<(), sqlx::Error> {
    let pool = PgPoolOptions::new().max_connections(2).connect(url).await?;
    let bootstrapped: bool = sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM information_schema.tables
            WHERE table_schema = 'fvoci' AND table_name = 'schema_migrations'
        )",
    )
    .fetch_one(&pool)
    .await?;

    for (sql, version) in MIGRATIONS {
        if bootstrapped {
            let applied: Option<i32> = sqlx::query_scalar(
                "SELECT version FROM fvoci.schema_migrations WHERE version = $1",
            )
            .bind(version)
            .fetch_optional(&pool)
            .await?;
            if applied.is_some() {
                continue;
            }
        }

        let mut tx = pool.begin().await?;
        sqlx::raw_sql(sql).execute(&mut *tx).await?;
        sqlx::query(
            "INSERT INTO fvoci.schema_migrations (version) VALUES ($1) ON CONFLICT DO NOTHING",
        )
        .bind(version)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
    }

    pool.close().await;
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
    Ok(())
}
