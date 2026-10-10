//! Per-test PostgreSQL database and app role, cloned from a migrated template.
//!
//! Every test gets its own database and its own `fvoci_app_*` login role
//! (NOSUPERUSER NOBYPASSRLS, granted through `migrate::apply_app_role_grants`),
//! exactly as when each test ran the migrations itself. Only the schema build is
//! shared: the compiled migration lineage is applied once per server into
//! `fvoci_tpl_<hash>` and each test database is `CREATE DATABASE ... TEMPLATE`
//! of it. The hash covers the compiled steps, the runner source and the server
//! version, so a changed migration builds a new template.
//!
//! Template build, stale sweep and clone run on one server-database session that
//! holds `pg_advisory_lock(TEMPLATE_LOCK_KEY)`. Advisory locks are scoped to a
//! database, so every fixture on a server must use the same admin user (the
//! default database of that user is where the lock lives). A template is built
//! under a temporary `_b<uuid>` name, closed to connections and only then
//! renamed, so the final name always means a complete template that no session
//! can connect to. `TestDb::create_fresh` keeps the migrate-into-an-empty-database
//! path for tests that exercise the migrations themselves.
#![allow(dead_code)]

use fvoci_server::db::migrate;
use rand::RngCore;
use sqlx::postgres::PgPoolOptions;
use sqlx::{Connection, PgConnection};
use uuid::Uuid;

/// Session-level lock serializing template build, sweep and clone on a server.
/// Distinct from the runner's migration lock.
const TEMPLATE_LOCK_KEY: i64 = 847_291_003_553;
const TEMPLATE_PREFIX: &str = "fvoci_tpl_";
/// `fvoci_tpl_` plus 16 hex digits; longer names are `_b<uuid>` builds.
const TEMPLATE_NAME_LEN: usize = TEMPLATE_PREFIX.len() + 16;
/// Fails a fixture loudly instead of waiting forever behind a stuck holder.
const TEMPLATE_LOCK_TIMEOUT: &str = "120s";
/// Text of the migration runner: its preflight and post-step SQL are applied
/// into the template too. The app-role grant script is not hashed because it
/// runs per clone, outside the template.
const MIGRATION_RUNNER_SOURCE: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/db/migrate.rs"));

pub struct TestDb {
    pub admin_url: String,
    pub app_url: String,
    db_name: String,
    role_name: String,
}

pub fn server_db_url(url: &str) -> String {
    let parsed = url::Url::parse(url).expect("database url");
    let mut server = parsed;
    server.set_path("");
    server.to_string().trim_end_matches('/').to_string()
}

pub fn join_db_url(server_url: &str, db_name: &str) -> String {
    let mut parsed = url::Url::parse(server_url).expect("server url");
    parsed.set_path(&format!("/{}", db_name));
    parsed.to_string()
}

fn admin_base_url() -> String {
    std::env::var("TEST_DATABASE_URL")
        .or_else(|_| std::env::var("FVOCI_TEST_DATABASE_URL"))
        .expect("TEST_DATABASE_URL missing; these tests require real PostgreSQL")
}

/// `fvoci_tpl_<16 hex>` over the server version (the preflight differs by
/// major), the compiled PostgreSQL lineage and the runner source.
pub fn template_name(server_version_num: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(server_version_num.as_bytes());
    hasher.update([0]);
    hasher.update(migrate::POSTGRES_LINEAGE.as_bytes());
    for step in migrate::compiled_postgres_steps() {
        hasher.update(step.version.to_be_bytes());
        hasher.update(step.name.as_bytes());
        hasher.update([0]);
        hasher.update(step.sha256.as_bytes());
        hasher.update(step.sql.as_bytes());
        hasher.update([0]);
    }
    hasher.update(MIGRATION_RUNNER_SOURCE.as_bytes());
    format!("{TEMPLATE_PREFIX}{}", &hex::encode(hasher.finalize())[..16])
}

impl TestDb {
    /// Clones the migrated template into `<prefix><uuid>` and provisions its app role.
    pub async fn create(prefix: &str) -> Self {
        let server_url = server_db_url(&admin_base_url());
        let db_name = format!("{prefix}{}", Uuid::now_v7().simple());
        clone_template(&server_url, &db_name).await;
        Self::provision(&server_url, db_name).await
    }

    /// Creates `<prefix><uuid>` empty and runs the full migration lineage into it.
    pub async fn create_fresh(prefix: &str) -> Self {
        let server_url = server_db_url(&admin_base_url());
        let db_name = format!("{prefix}{}", Uuid::now_v7().simple());
        let admin_pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(&server_url)
            .await
            .expect("connect admin");
        sqlx::query(&format!("CREATE DATABASE \"{}\"", db_name))
            .execute(&admin_pool)
            .await
            .expect("create database");
        admin_pool.close().await;
        migrate::run_migrations(&join_db_url(&server_url, &db_name))
            .await
            .expect("migrate");
        Self::provision(&server_url, db_name).await
    }

    async fn provision(server_url: &str, db_name: String) -> Self {
        let role_name = format!("fvoci_app_{}", db_name.replace('-', "_"));
        let mut password_bytes = [0u8; 24];
        rand::rng().fill_bytes(&mut password_bytes);
        let role_password = hex::encode(password_bytes);
        let admin_url = join_db_url(server_url, &db_name);
        let migration_pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(&admin_url)
            .await
            .expect("connect migration db");
        sqlx::query(&format!(
            "CREATE ROLE \"{}\" LOGIN PASSWORD '{}' NOSUPERUSER NOBYPASSRLS",
            role_name, role_password
        ))
        .execute(&migration_pool)
        .await
        .expect("create role");
        migrate::apply_app_role_grants(&migration_pool, &role_name)
            .await
            .expect("grant");
        migration_pool.close().await;
        let mut app = url::Url::parse(&admin_url).expect("database url");
        app.set_username(&role_name).ok();
        app.set_password(Some(&role_password)).ok();
        Self {
            admin_url,
            app_url: app.to_string(),
            db_name,
            role_name,
        }
    }

    pub fn db_name(&self) -> &str {
        &self.db_name
    }

    pub fn role_name(&self) -> &str {
        &self.role_name
    }

    pub async fn database_exists(admin_url: &str, db_name: &str) -> Result<bool, String> {
        let server_url = server_db_url(admin_url);
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(&server_url)
            .await
            .map_err(|e| format!("connect admin to probe database: {e}"))?;
        let (exists,): (bool,) =
            sqlx::query_as("SELECT EXISTS(SELECT 1 FROM pg_database WHERE datname = $1)")
                .bind(db_name)
                .fetch_one(&pool)
                .await
                .map_err(|e| format!("probe database {db_name}: {e}"))?;
        pool.close().await;
        Ok(exists)
    }

    pub async fn role_exists(admin_url: &str, role_name: &str) -> Result<bool, String> {
        let server_url = server_db_url(admin_url);
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(&server_url)
            .await
            .map_err(|e| format!("connect admin to probe role: {e}"))?;
        let (exists,): (bool,) =
            sqlx::query_as("SELECT EXISTS(SELECT 1 FROM pg_roles WHERE rolname = $1)")
                .bind(role_name)
                .fetch_one(&pool)
                .await
                .map_err(|e| format!("probe role {role_name}: {e}"))?;
        pool.close().await;
        Ok(exists)
    }

    /// Terminates sessions on this test's database, then drops the database and
    /// its role. Every step is attempted; the errors are joined.
    pub async fn drop_owned(self) -> Result<(), String> {
        let server_url = server_db_url(&self.admin_url);
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(&server_url)
            .await
            .map_err(|e| format!("connect admin for cleanup: {e}"))?;
        let mut errors = Vec::new();
        if let Err(error) = sqlx::query(&format!(
            "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '{}'",
            self.db_name
        ))
        .execute(&pool)
        .await
        {
            errors.push(format!("terminate backends for {}: {error}", self.db_name));
        }
        if let Err(error) = sqlx::query(&format!("DROP DATABASE IF EXISTS \"{}\"", self.db_name))
            .execute(&pool)
            .await
        {
            errors.push(format!("drop database {}: {error}", self.db_name));
        }
        if let Err(error) = sqlx::query(&format!("DROP ROLE IF EXISTS \"{}\"", self.role_name))
            .execute(&pool)
            .await
        {
            errors.push(format!("drop role {}: {error}", self.role_name));
        }
        pool.close().await;
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }
}

/// Under the template lock: drop interrupted builds, build the current template
/// if it is missing (then sweep older ones), and clone it into `db_name`.
async fn clone_template(server_url: &str, db_name: &str) {
    let mut lock = PgConnection::connect(server_url)
        .await
        .expect("connect admin for template lock");
    sqlx::query(&format!("SET lock_timeout = '{TEMPLATE_LOCK_TIMEOUT}'"))
        .execute(&mut lock)
        .await
        .expect("bound template lock wait");
    let server_version: String = sqlx::query_scalar("SHOW server_version_num")
        .fetch_one(&mut lock)
        .await
        .expect("server version");
    let template = template_name(&server_version);
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(TEMPLATE_LOCK_KEY)
        .execute(&mut lock)
        .await
        .expect("acquire template lock (another fixture holds it past lock_timeout)");
    drop_templates(&mut lock, "length(datname) > $3", &template).await;
    let (ready,): (bool,) =
        sqlx::query_as("SELECT EXISTS(SELECT 1 FROM pg_database WHERE datname = $1)")
            .bind(&template)
            .fetch_one(&mut lock)
            .await
            .expect("probe template");
    if !ready {
        build_template(&mut lock, server_url, &template).await;
        // Keep the newest other template so two checkouts with different
        // migrations on one server do not rebuild each other's on every clone.
        drop_templates(
            &mut lock,
            "length(datname) = $3 ORDER BY oid DESC OFFSET 1",
            &template,
        )
        .await;
    }
    sqlx::query(&format!(
        "CREATE DATABASE \"{db_name}\" TEMPLATE \"{template}\""
    ))
    .execute(&mut lock)
    .await
    .expect("clone template database");
    sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(TEMPLATE_LOCK_KEY)
        .execute(&mut lock)
        .await
        .expect("release template lock");
    lock.close().await.expect("close template lock session");
}

async fn build_template(lock: &mut PgConnection, server_url: &str, template: &str) {
    let build = format!("{template}_b{}", Uuid::now_v7().simple());
    sqlx::query(&format!("CREATE DATABASE \"{build}\""))
        .execute(&mut *lock)
        .await
        .expect("create template build database");
    migrate::run_migrations(&join_db_url(server_url, &build))
        .await
        .expect("migrate template build database");
    // The migration pool is closed, but its backends may still be exiting;
    // wait for them server-side before the database is closed and renamed.
    sqlx::query("SELECT pg_terminate_backend(pid, 5000) FROM pg_stat_activity WHERE datname = $1")
        .bind(&build)
        .execute(&mut *lock)
        .await
        .expect("terminate template build sessions");
    sqlx::query(&format!(
        "ALTER DATABASE \"{build}\" WITH ALLOW_CONNECTIONS false"
    ))
    .execute(&mut *lock)
    .await
    .expect("close template to connections");
    sqlx::query(&format!(
        "ALTER DATABASE \"{build}\" RENAME TO \"{template}\""
    ))
    .execute(&mut *lock)
    .await
    .expect("publish template database");
}

/// Best-effort drop of `fvoci_tpl_*` databases other than `current` that match
/// `filter` (`$3` is the final-name length): `_b<uuid>` builds left by
/// interrupted runs, or templates of other migration sets. Runs under the
/// template lock, so no fixture is building or cloning from them; failures are
/// reported and do not fail the test.
async fn drop_templates(lock: &mut PgConnection, filter: &str, current: &str) {
    let stale: Vec<String> = match sqlx::query_scalar(&format!(
        "SELECT datname FROM pg_database WHERE left(datname, $1) = $2 AND datname <> $4 AND {filter}"
    ))
    .bind(TEMPLATE_PREFIX.len() as i32)
    .bind(TEMPLATE_PREFIX)
    .bind(TEMPLATE_NAME_LEN as i32)
    .bind(current)
    .fetch_all(&mut *lock)
    .await
    {
        Ok(stale) => stale,
        Err(error) => {
            eprintln!("test_db: listing stale templates failed: {error}");
            return;
        }
    };
    for name in stale {
        if let Err(error) = sqlx::query(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
            .execute(&mut *lock)
            .await
        {
            eprintln!("test_db: dropping stale template {name} failed: {error}");
        }
    }
}
