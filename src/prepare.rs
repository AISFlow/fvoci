//! Container startup of the Compose install (`fvoci-migrate --start`, the
//! image entrypoint).
//!
//! Without the database owner password in its environment it only execs
//! `fvoci-server` (the separate-`init` installs of `infra/rust/compose.yml`).
//! With it (`infra/rust/compose.user.yml`, values from the operator's `.env`):
//!
//! 1. validate the required settings (missing, placeholder, format), naming
//!    variables only;
//! 2. wait for PostgreSQL and Meilisearch with a deadline, leaving on
//!    SIGTERM/SIGINT;
//! 3. under an advisory lock as the owner: refuse a schema upgrade while
//!    another server holds app-role sessions, create the app role, migrate,
//!    grant, check the app role's password, ensure the scoped search key;
//! 4. close every prep connection and `exec` `fvoci-server` with the
//!    environment minus the owner password, the Meilisearch master key and
//!    the raw app password ([`PREP_ONLY`]); it gets `DATABASE_APP_URL`.
//!
//! The server keeps the process id, so signals, shutdown and child reaping
//! are the server's own. What remains shared: the same container and uid,
//! so the container configuration (`docker inspect`, `docker exec`) still
//! carries the prep values; the server process, its children and anything it
//! execs do not. Nothing here generates or stores keys.

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sqlx::postgres::PgPoolOptions;
use sqlx::{Connection, PgConnection};

use crate::db::migrate;
use crate::search::meili::ensure_meili_key_file;

/// Compose install defaults (service names and the owner/app/database names
/// of `infra/rust/compose.user.yml`); `POSTGRES_USER`, `POSTGRES_DB`,
/// `FVOCI_APP_ROLE`, `FVOCI_DB_HOST` and `FVOCI_MEILI_URL` override them.
pub const DEFAULT_OWNER: &str = "fvoci_owner";
pub const DEFAULT_DATABASE: &str = "fvoci";
pub const DEFAULT_APP_ROLE: &str = "fvoci_app";
pub const DEFAULT_DB_HOST: &str = "postgres:5432";
pub const DEFAULT_MEILI_URL: &str = "http://meilisearch:7700";
/// Scoped search key file (the image's `/run/fvoci/meili`, a Compose volume).
pub const DEFAULT_MEILI_KEY_FILE: &str = "/run/fvoci/meili/api_key";
/// Default readiness deadline; `FVOCI_PREPARE_TIMEOUT_SECS` overrides it.
pub const DEFAULT_PREPARE_TIMEOUT_SECS: u64 = 120;

/// Values only the preparation uses; never passed to `fvoci-server`.
pub const PREP_ONLY: &[&str] = &[
    "POSTGRES_PASSWORD",
    "DATABASE_URL",
    "FVOCI_MIGRATION_URL",
    "MEILI_MASTER_KEY",
    "FVOCI_MEILI_MASTER_KEY",
    "FVOCI_APP_PASSWORD",
];

/// Required when the owner password is given; checked before anything connects.
const REQUIRED: &[&str] = &[
    "POSTGRES_PASSWORD",
    "FVOCI_APP_PASSWORD",
    "MEILI_MASTER_KEY",
    "PASSWORD_PEPPER_KEYS",
    "PASSWORD_PEPPER_ACTIVE_KEY_ID",
    "ENCRYPTION_KEYS",
    "ENCRYPTION_ACTIVE_KEY_ID",
    "FVOCI_PUBLIC_ORIGIN",
];

/// Passwords and the master key: at least this long (Meilisearch's own
/// production minimum).
const MIN_SECRET_LEN: usize = 16;

/// Serializes concurrent preparations (e.g. `docker compose run` next to `up`).
const PREPARE_LOCK_KEY: i64 = 0x6676_6f63_7072_6570;

/// Database names; restricted so they need no quoting in URLs.
#[derive(Debug, PartialEq)]
pub struct DbNames {
    pub owner: String,
    pub database: String,
    pub app_role: String,
    pub host: String,
}

impl DbNames {
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let ident = |name: &str, default: &str| {
            let value = get(name).unwrap_or_else(|| default.to_string());
            let valid = !value.is_empty()
                && value.len() <= 63
                && value
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
                && !value.as_bytes()[0].is_ascii_digit();
            if valid {
                Ok(value)
            } else {
                Err(format!(
                    "{name} must be a lowercase identifier ([a-z_][a-z0-9_]*)"
                ))
            }
        };
        let host = get("FVOCI_DB_HOST").unwrap_or_else(|| DEFAULT_DB_HOST.to_string());
        if host.is_empty()
            || !host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-_:[]".contains(&b))
        {
            return Err("FVOCI_DB_HOST must be host[:port]".into());
        }
        Ok(Self {
            owner: ident("POSTGRES_USER", DEFAULT_OWNER)?,
            database: ident("POSTGRES_DB", DEFAULT_DATABASE)?,
            app_role: ident("FVOCI_APP_ROLE", DEFAULT_APP_ROLE)?,
            host,
        })
    }

    fn url(&self, user: &str, password: &str) -> String {
        format!(
            "postgres://{user}:{}@{}/{}",
            pct_encode(password),
            self.host,
            self.database
        )
    }
}

/// RFC 3986 percent-encoding of everything but unreserved characters.
fn pct_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Connection settings `fvoci-migrate` derives from the Compose install's
/// variables when their URL forms are unset: the owner `DATABASE_URL` from
/// `POSTGRES_PASSWORD`, `DATABASE_APP_URL` from `FVOCI_APP_PASSWORD`, and
/// with `MEILI_MASTER_KEY` the Meilisearch URL and key file. Owner commands
/// (`--recover-outbox`, `--rebuild-search`, ...) then work in the `fvoci`
/// container as they do in `init`. Both forms set is an error.
pub fn install_env(
    get: impl Fn(&str) -> Option<String>,
) -> Result<Vec<(&'static str, String)>, String> {
    let set = |name: &str| get(name).is_some();
    let mut out = Vec::new();
    if let Some(password) = get("POSTGRES_PASSWORD") {
        if let Some(var) = ["DATABASE_URL", "FVOCI_MIGRATION_URL"]
            .into_iter()
            .find(|v| set(v))
        {
            return Err(format!(
                "{var} and POSTGRES_PASSWORD are both set; set only one"
            ));
        }
        let names = DbNames::from_lookup(&get)?;
        out.push(("DATABASE_URL", names.url(&names.owner, &password)));
    }
    if let Some(password) = get("FVOCI_APP_PASSWORD") {
        if !set("DATABASE_APP_URL") && !set("FVOCI_APP_DATABASE_URL") {
            let names = DbNames::from_lookup(&get)?;
            out.push(("DATABASE_APP_URL", names.url(&names.app_role, &password)));
        }
    }
    if set("MEILI_MASTER_KEY") || set("FVOCI_MEILI_MASTER_KEY") {
        if !set("FVOCI_MEILI_URL") {
            out.push(("FVOCI_MEILI_URL", DEFAULT_MEILI_URL.to_string()));
        }
        if !set("FVOCI_MEILI_KEY") && !set("FVOCI_MEILI_KEY_FILE") {
            out.push(("FVOCI_MEILI_KEY_FILE", DEFAULT_MEILI_KEY_FILE.to_string()));
        }
    }
    Ok(out)
}

/// [`install_env`] applied to the process environment. Call before a
/// runtime or any other thread exists.
pub fn load_install_env() -> Result<(), String> {
    for (name, value) in install_env(|k| std::env::var(k).ok())? {
        std::env::set_var(name, value);
    }
    Ok(())
}

/// Every problem with the required settings, naming variables only.
pub fn validate(get: impl Fn(&str) -> Option<String>) -> Vec<String> {
    let mut problems = Vec::new();
    for name in REQUIRED {
        let Some(value) = get(name) else {
            problems.push(format!("{name} is not set (see .env.example)"));
            continue;
        };
        let lower = value.to_ascii_lowercase();
        if value.trim().is_empty() {
            problems.push(format!("{name} is empty (see .env.example)"));
        } else if value.contains('<')
            || value.contains('>')
            || [
                "changeme",
                "change-me",
                "change_me",
                "replaceme",
                "replace-me",
                "replace_me",
            ]
            .iter()
            .any(|w| lower.contains(w))
        {
            problems.push(format!(
                "{name} still holds an example placeholder; generate a value as .env.example shows"
            ));
        }
    }
    if !problems.is_empty() {
        return problems;
    }
    let value = |name: &str| get(name).unwrap_or_default();
    for name in [
        "POSTGRES_PASSWORD",
        "FVOCI_APP_PASSWORD",
        "MEILI_MASTER_KEY",
    ] {
        if value(name).trim().chars().count() < MIN_SECRET_LEN {
            problems.push(format!(
                "{name} must be at least {MIN_SECRET_LEN} characters (e.g. openssl rand -hex 32)"
            ));
        }
    }
    if value("POSTGRES_PASSWORD") == value("FVOCI_APP_PASSWORD") {
        problems.push("FVOCI_APP_PASSWORD must differ from POSTGRES_PASSWORD".into());
    }
    if let Err(e) = crate::auth::password::Keyring::parse(
        &value("PASSWORD_PEPPER_KEYS"),
        &value("PASSWORD_PEPPER_ACTIVE_KEY_ID"),
    ) {
        problems.push(format!(
            "PASSWORD_PEPPER_KEYS / PASSWORD_PEPPER_ACTIVE_KEY_ID: {e}"
        ));
    }
    if let Err(e) = crate::auth::password::Keyring::parse_named(
        &value("ENCRYPTION_KEYS"),
        &value("ENCRYPTION_ACTIVE_KEY_ID"),
        "ENCRYPTION_KEYS",
    ) {
        problems.push(format!("ENCRYPTION_KEYS / ENCRYPTION_ACTIVE_KEY_ID: {e}"));
    }
    if let Err(e) = crate::http::guard::normalize_public_origin(&value("FVOCI_PUBLIC_ORIGIN")) {
        problems.push(format!("FVOCI_PUBLIC_ORIGIN: {e}"));
    }
    problems
}

/// Whether this start prepares the install (the owner password is given).
pub fn wants_prepare() -> bool {
    std::env::var_os("POSTGRES_PASSWORD").is_some()
}

/// Steps 2-3. Runs inside a runtime; the caller races it against signals.
pub async fn prepare() -> Result<(), String> {
    let owner_url = std::env::var("DATABASE_URL").map_err(|_| "DATABASE_URL is not derived")?;
    let names = DbNames::from_lookup(|k| std::env::var(k).ok())?;
    let deadline = tokio::time::Instant::now()
        + Duration::from_secs(match std::env::var("FVOCI_PREPARE_TIMEOUT_SECS") {
            Ok(raw) => raw
                .trim()
                .parse::<u64>()
                .ok()
                .filter(|s| *s > 0)
                .ok_or("FVOCI_PREPARE_TIMEOUT_SECS must be a positive number of seconds")?,
            Err(_) => DEFAULT_PREPARE_TIMEOUT_SECS,
        });

    let mut conn = wait_for_postgres(&owner_url, &names, deadline).await?;
    let meili_url = std::env::var("FVOCI_MEILI_URL")
        .ok()
        .filter(|v| !v.trim().is_empty());
    if let Some(url) = &meili_url {
        wait_for_meili(url.trim(), deadline).await?;
    }
    eprintln!("fvoci: PostgreSQL and Meilisearch ready; preparing");
    let db = |e: sqlx::Error| e.to_string();
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(PREPARE_LOCK_KEY)
        .execute(&mut conn)
        .await
        .map_err(db)?;

    refuse_upgrade_with_live_writers(&owner_url, &mut conn, &names).await?;

    let app_password =
        std::env::var("FVOCI_APP_PASSWORD").map_err(|_| "FVOCI_APP_PASSWORD is not set")?;
    let create: Option<String> = sqlx::query_scalar(
        "SELECT format('CREATE ROLE %I LOGIN PASSWORD %L NOSUPERUSER NOBYPASSRLS', $1::text, $2::text)
         WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = $1::text)",
    )
    .bind(&names.app_role)
    .bind(&app_password)
    .fetch_optional(&mut conn)
    .await
    .map_err(db)?;
    if let Some(statement) = create {
        sqlx::raw_sql(&statement)
            .execute(&mut conn)
            .await
            .map_err(|e| format!("create app role {}: {e}", names.app_role))?;
        eprintln!("fvoci: created app role {}", names.app_role);
    }
    migrate::run_migrations(&owner_url)
        .await
        .map_err(|e| format!("migrate: {e}"))?;
    migrate::grant_app_role(&owner_url, &names.app_role)
        .await
        .map_err(|e| e.to_string())?;
    eprintln!(
        "fvoci: migrated; granted app role privileges to {}",
        names.app_role
    );

    let app_url = names.url(&names.app_role, &app_password);
    match PgConnection::connect(&app_url).await {
        Ok(app) => {
            let _ = app.close().await;
        }
        Err(e) if is_auth_failure(&e) => {
            return Err(format!(
                "FVOCI_APP_PASSWORD does not open the existing app role {}: the role keeps \
                 the password from the install's first start; restore that value",
                names.app_role
            ))
        }
        Err(e) => return Err(format!("connect as {}: {e}", names.app_role)),
    }

    if meili_url.is_some() {
        let key_file = std::env::var("FVOCI_MEILI_KEY_FILE")
            .unwrap_or_else(|_| DEFAULT_MEILI_KEY_FILE.to_string());
        ensure_meili_key_file(Path::new(&key_file)).await?;
        eprintln!("fvoci: search key ready");
    }
    sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(PREPARE_LOCK_KEY)
        .execute(&mut conn)
        .await
        .map_err(db)?;
    let _ = conn.close().await;
    Ok(())
}

fn is_auth_failure(e: &sqlx::Error) -> bool {
    // 28P01 invalid_password, 28000 invalid_authorization_specification
    matches!(
        e.as_database_error().and_then(|d| d.code()).as_deref(),
        Some("28P01" | "28000")
    )
}

async fn wait_for_postgres(
    url: &str,
    names: &DbNames,
    deadline: tokio::time::Instant,
) -> Result<PgConnection, String> {
    loop {
        match PgConnection::connect(url).await {
            Ok(conn) => return Ok(conn),
            Err(e) if is_auth_failure(&e) => {
                return Err(format!(
                    "POSTGRES_PASSWORD is not the password of {} in this database: PostgreSQL \
                     keeps the password from its first start; restore that value",
                    names.owner
                ))
            }
            Err(e) if tokio::time::Instant::now() >= deadline => {
                return Err(format!(
                    "PostgreSQL at {} not ready before the deadline: {e}",
                    names.host
                ))
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(500)).await,
        }
    }
}

async fn wait_for_meili(url: &str, deadline: tokio::time::Instant) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .map_err(|e| e.to_string())?;
    let health = format!("{}/health", url.trim_end_matches('/'));
    loop {
        let last = match client.get(&health).send().await {
            Ok(resp) if resp.status().is_success() => return Ok(()),
            Ok(resp) => format!("HTTP {}", resp.status()),
            Err(e) => e.to_string(),
        };
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "Meilisearch at {url} not ready before the deadline: {last}"
            ));
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// Migrating while another server writes is not supported (RUNNING.md
/// "Upgrade"): with migrations pending and app-role sessions open, refuse.
async fn refuse_upgrade_with_live_writers(
    owner_url: &str,
    conn: &mut PgConnection,
    names: &DbNames,
) -> Result<(), String> {
    let db = |e: sqlx::Error| e.to_string();
    let bootstrapped: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM information_schema.tables
                        WHERE table_schema = 'fvoci' AND table_name = 'schema_migrations')",
    )
    .fetch_one(&mut *conn)
    .await
    .map_err(db)?;
    if !bootstrapped {
        return Ok(());
    }
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(owner_url)
        .await
        .map_err(db)?;
    let applied: Vec<i32> =
        sqlx::query_scalar("SELECT version FROM fvoci.schema_migrations ORDER BY version")
            .fetch_all(&pool)
            .await
            .map_err(db)?;
    pool.close().await;
    let compiled = migrate::compiled_migration_versions();
    if applied.iter().any(|v| !compiled.contains(v)) {
        return Err(migrate::schema_gate(&applied, &compiled)
            .err()
            .unwrap_or_default());
    }
    let pending = compiled.iter().filter(|v| !applied.contains(v)).count();
    if pending == 0 {
        return Ok(());
    }
    let sessions: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_stat_activity WHERE usename = $1::text AND datname = current_database()",
    )
    .bind(&names.app_role)
    .fetch_one(&mut *conn)
    .await
    .map_err(db)?;
    if sessions > 0 {
        return Err(format!(
            "{pending} migration(s) are pending but {sessions} session(s) of {} are open: another \
             FVOCI server is still running. Stop every other server, take a backup and start \
             again (RUNNING.md \"Upgrade\")",
            names.app_role
        ));
    }
    eprintln!("fvoci: applying {pending} pending migration(s)");
    Ok(())
}

/// `fvoci-server` next to this executable.
fn server_path() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("current executable: {e}"))?;
    Ok(exe.with_file_name("fvoci-server"))
}

/// Replaces this process with `fvoci-server args`, its environment minus
/// [`PREP_ONLY`]. Returns only on failure.
pub fn exec_server(args: &[String]) -> String {
    let path = match server_path() {
        Ok(path) => path,
        Err(e) => return e,
    };
    let env = std::env::vars_os().filter(|(k, _)| {
        !PREP_ONLY.iter().any(|p| {
            k.to_str()
                .is_some_and(|k| k == *p || k == format!("{p}_FILE"))
        })
    });
    let err = std::process::Command::new(&path)
        .args(args)
        .env_clear()
        .envs(env)
        .exec();
    format!("exec {}: {err}", path.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup<'a>(pairs: &'a [(&'static str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k: &str| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        }
    }

    fn valid() -> Vec<(&'static str, String)> {
        let hex = |c: char| std::iter::repeat_n(c, 64).collect::<String>();
        vec![
            ("POSTGRES_PASSWORD", "p@ss/w:rd-0123456789".into()),
            ("FVOCI_APP_PASSWORD", hex('a')),
            ("MEILI_MASTER_KEY", hex('b')),
            (
                "PASSWORD_PEPPER_KEYS",
                format!(r#"{{"install":"{}"}}"#, hex('c')),
            ),
            ("PASSWORD_PEPPER_ACTIVE_KEY_ID", "install".into()),
            (
                "ENCRYPTION_KEYS",
                format!(r#"{{"install":"{}"}}"#, hex('d')),
            ),
            ("ENCRYPTION_ACTIVE_KEY_ID", "install".into()),
            ("FVOCI_PUBLIC_ORIGIN", "http://localhost:8080".into()),
        ]
    }

    fn get<'a>(env: &'a [(&'static str, String)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k: &str| env.iter().find(|(n, _)| *n == k).map(|(_, v)| v.clone())
    }

    #[test]
    fn validates_required_settings_without_echoing_values() {
        assert!(validate(get(&valid())).is_empty());

        let mut env = valid();
        env.retain(|(k, _)| *k != "MEILI_MASTER_KEY");
        env.iter_mut()
            .find(|(k, _)| *k == "ENCRYPTION_KEYS")
            .unwrap()
            .1 = String::new();
        let problems = validate(get(&env));
        assert_eq!(
            problems,
            vec![
                "MEILI_MASTER_KEY is not set (see .env.example)".to_string(),
                "ENCRYPTION_KEYS is empty (see .env.example)".to_string(),
            ]
        );

        for placeholder in [
            r#"{"install":"<openssl rand -hex 32>"}"#,
            "change-me-please-now",
        ] {
            let mut env = valid();
            env[0].1 = placeholder.into();
            let problems = validate(get(&env));
            assert_eq!(problems.len(), 1, "{problems:?}");
            assert!(problems[0].contains("example placeholder"), "{problems:?}");
        }

        let mut env = valid();
        env[1].1 = "short".into();
        env[3].1 = r#"{"install":"00"}"#.into();
        env[7].1 = "localhost:8080".into();
        let problems = validate(get(&env)).join("\n");
        assert!(
            problems.contains("FVOCI_APP_PASSWORD must be at least 16"),
            "{problems}"
        );
        assert!(problems.contains("PASSWORD_PEPPER_KEYS"), "{problems}");
        assert!(problems.contains("FVOCI_PUBLIC_ORIGIN"), "{problems}");
        assert!(
            !problems.contains("short") && !problems.contains("\"00\""),
            "{problems}"
        );

        let mut env = valid();
        env[1].1 = env[0].1.clone();
        assert!(validate(get(&env))[0].contains("must differ"));
    }

    #[test]
    fn derives_urls_from_install_variables() {
        let env = install_env(lookup(&[
            ("POSTGRES_PASSWORD", "p@ss/w:rd"),
            ("FVOCI_APP_PASSWORD", "app"),
            ("MEILI_MASTER_KEY", "m"),
        ]))
        .unwrap();
        assert_eq!(
            env,
            vec![
                (
                    "DATABASE_URL",
                    "postgres://fvoci_owner:p%40ss%2Fw%3Ard@postgres:5432/fvoci".to_string()
                ),
                (
                    "DATABASE_APP_URL",
                    "postgres://fvoci_app:app@postgres:5432/fvoci".to_string()
                ),
                ("FVOCI_MEILI_URL", DEFAULT_MEILI_URL.to_string()),
                ("FVOCI_MEILI_KEY_FILE", DEFAULT_MEILI_KEY_FILE.to_string()),
            ]
        );
        // The separate-init install (compose.yml) sets the URL forms: nothing derived.
        assert!(install_env(lookup(&[
            ("DATABASE_APP_URL", "postgres://x"),
            ("FVOCI_MEILI_URL", "http://m:7700"),
            ("FVOCI_MEILI_KEY_FILE", "/k"),
        ]))
        .unwrap()
        .is_empty());
        assert!(install_env(lookup(&[
            ("FVOCI_APP_PASSWORD", "app"),
            ("DATABASE_APP_URL", "postgres://x"),
        ]))
        .unwrap()
        .is_empty());
        let err =
            install_env(lookup(&[("POSTGRES_PASSWORD", "p"), ("DATABASE_URL", "")])).unwrap_err();
        assert!(
            err.starts_with("DATABASE_URL and POSTGRES_PASSWORD are both set"),
            "{err}"
        );
        assert!(install_env(lookup(&[
            ("POSTGRES_PASSWORD", "p"),
            ("POSTGRES_DB", "Bad")
        ]))
        .is_err());
        assert!(install_env(lookup(&[
            ("POSTGRES_PASSWORD", "p"),
            ("FVOCI_DB_HOST", "h/x@y")
        ]))
        .is_err());
    }
}
