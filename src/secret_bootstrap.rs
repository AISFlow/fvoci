//! `fvoci-migrate --install`: the one-shot `init` service of the standalone
//! Compose install (`infra/rust/compose.user.yml`).
//!
//! PostgreSQL and Meilisearch generate their own owner password and master
//! key on first start (a thin wrapper in each service). `init` mounts those two
//! files read-only and the server's settings volume read-write under
//! [`INSTALL_ROOT`]:
//!
//! | path under `INSTALL_ROOT` | written by | read by |
//! | ------------------------- | ---------- | ------- |
//! | `postgres/postgres_password` | postgres service | postgres, init |
//! | `meilisearch/master_key` | meilisearch service | meilisearch, init |
//! | `server/*` ([`crate::config::INSTALL_SETTING_FILES`]) | init | init, server (as `/run/fvoci/secrets`) |
//!
//! Under a PostgreSQL advisory lock `--install` generates the app role password,
//! password pepper and encryption keyrings once (files first, marker last;
//! never overwritten), creates the app role, migrates, grants, ensures the
//! scoped Meilisearch key and records the Meilisearch URL for the server. When
//! the server files are missing but the database already holds an install
//! (the app role or the `fvoci` schema exists), it fails instead: new keys
//! cannot open that data. The server mounts only its own settings, so it never
//! holds the owner password or the master key. Secret values are never printed.

use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use sqlx::{Connection, PgConnection};

use crate::config::{install_setting_file, read_setting_file};
use crate::db::migrate;
use crate::init_env::hex_secret;
use crate::search::meili::ensure_meili_key_file;

/// Mount root of the install layout inside the `init` container.
pub const INSTALL_ROOT: &str = "/run/fvoci/install";
pub const OWNER_PASSWORD_FILE: &str = "postgres/postgres_password";
pub const MASTER_KEY_FILE: &str = "meilisearch/master_key";
pub const SERVER_DIR: &str = "server";
pub const MARKER: &str = ".fvoci-install-complete";
const TEMP_PREFIX: &str = ".fvoci-install-tmp-";

/// Install defaults; the Compose file's service names and `POSTGRES_*` values
/// match them. `POSTGRES_USER`, `POSTGRES_DB`, `FVOCI_APP_ROLE`,
/// `FVOCI_DB_HOST` and `FVOCI_MEILI_URL` override them.
pub const DEFAULT_OWNER: &str = "fvoci_owner";
pub const DEFAULT_DATABASE: &str = "fvoci";
pub const DEFAULT_APP_ROLE: &str = "fvoci_app";
pub const DEFAULT_DB_HOST: &str = "postgres:5432";
pub const DEFAULT_MEILI_URL: &str = "http://meilisearch:7700";

/// Key id written with fresh pepper and encryption keyrings (same as `--init-env`).
const KEY_ID: &str = "install";
/// Serializes concurrent `--install` runs (e.g. two `docker compose up`).
const INSTALL_LOCK_KEY: i64 = 0x6676_6f63_6969_6e73;

/// Generated server secrets, in write order; the marker follows them.
const SERVER_SECRETS: &[&str] = &[
    "DATABASE_APP_URL",
    "PASSWORD_PEPPER_KEYS",
    "PASSWORD_PEPPER_ACTIVE_KEY_ID",
    "ENCRYPTION_KEYS",
    "ENCRYPTION_ACTIVE_KEY_ID",
];

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

/// Settings `fvoci-migrate` takes from the install layout under `root` when
/// its files are mounted (only the `init` container mounts them): the owner
/// `DATABASE_URL`, `MEILI_MASTER_KEY`, and the Meilisearch URL and scoped key
/// file. Every owner command (`--install`, `--rebuild-search`,
/// `--recover-outbox`, ...) then works in that container without the values
/// in its environment or argv. Setting a variable that a present file would
/// provide is an error.
pub fn install_env(
    root: &Path,
    get: impl Fn(&str) -> Option<String>,
) -> Result<Vec<(&'static str, String)>, String> {
    let optional = |rel: &str| -> Result<Option<String>, String> {
        let path = root.join(rel);
        match fs::metadata(&path) {
            Ok(_) => read_setting_file(rel, &path).map(Some),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("{rel}: cannot read {}: {e}", path.display())),
        }
    };
    let conflict = |vars: &[&str], rel: &str| -> Result<(), String> {
        match vars.iter().find(|v| get(v).is_some()) {
            Some(var) => Err(format!(
                "{var} is set and {} exists; set only one",
                root.join(rel).display()
            )),
            None => Ok(()),
        }
    };
    let mut out = Vec::new();
    if let Some(password) = optional(OWNER_PASSWORD_FILE)? {
        conflict(
            &["DATABASE_URL", "FVOCI_MIGRATION_URL"],
            OWNER_PASSWORD_FILE,
        )?;
        let names = DbNames::from_lookup(&get)?;
        out.push(("DATABASE_URL", names.url(&names.owner, &password)));
    }
    if let Some(master) = optional(MASTER_KEY_FILE)? {
        conflict(
            &["MEILI_MASTER_KEY", "FVOCI_MEILI_MASTER_KEY"],
            MASTER_KEY_FILE,
        )?;
        out.push(("MEILI_MASTER_KEY", master));
        if get("FVOCI_MEILI_URL").is_none() {
            out.push(("FVOCI_MEILI_URL", DEFAULT_MEILI_URL.to_string()));
        }
    }
    let key_file = root
        .join(SERVER_DIR)
        .join(install_setting_file("FVOCI_MEILI_KEY"));
    if key_file.is_file()
        && get("FVOCI_MEILI_KEY").is_none()
        && get("FVOCI_MEILI_KEY_FILE").is_none()
    {
        out.push(("FVOCI_MEILI_KEY_FILE", key_file.display().to_string()));
    }
    Ok(out)
}

/// [`install_env`] for [`INSTALL_ROOT`], applied to the process environment.
/// Call before a runtime or any other thread exists.
pub fn load_install_env() -> Result<(), String> {
    for (name, value) in install_env(Path::new(INSTALL_ROOT), |k| std::env::var(k).ok())? {
        std::env::set_var(name, value);
    }
    Ok(())
}

#[derive(Debug, PartialEq)]
enum ServerSecrets {
    Complete { app_password: String },
    Incomplete { missing: Vec<String> },
}

fn server_secrets(dir: &Path, names: &DbNames) -> Result<ServerSecrets, String> {
    let mut missing = Vec::new();
    for file in
        std::iter::once(MARKER).chain(SERVER_SECRETS.iter().map(|v| install_setting_file(v)))
    {
        match fs::symlink_metadata(dir.join(file)) {
            Ok(meta) if meta.is_file() && meta.len() > 0 => {}
            Ok(_) => missing.push(file.to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => missing.push(file.to_string()),
            Err(e) => return Err(format!("stat {}: {e}", dir.join(file).display())),
        }
    }
    if !missing.is_empty() {
        return Ok(ServerSecrets::Incomplete { missing });
    }
    let file = install_setting_file("DATABASE_APP_URL");
    let raw = read_setting_file(file, &dir.join(file))?;
    let url = url::Url::parse(&raw).map_err(|_| format!("{file} is not a URL"))?;
    if url.username() != names.app_role || url.path() != format!("/{}", names.database) {
        return Err(format!(
            "{file} names another role or database than {}/{} (keep the install's names)",
            names.app_role, names.database
        ));
    }
    let app_password = url.password().unwrap_or_default().to_string();
    if app_password.is_empty() || !app_password.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("{file} has no generated app role password"));
    }
    Ok(ServerSecrets::Complete { app_password })
}

/// Writes fresh server secrets (files, then the marker) and returns the app
/// role password. Only called under the install lock and while the database
/// holds no install, so replacing an interrupted run's files is safe.
fn generate_server_secrets(dir: &Path, names: &DbNames) -> Result<String, String> {
    let app_password = hex_secret();
    let keyring = || format!(r#"{{"{KEY_ID}":"{}"}}"#, hex_secret());
    remove_if_present(&dir.join(MARKER))?;
    remove_stale_temps(dir)?;
    for var in SERVER_SECRETS {
        let value = match *var {
            "DATABASE_APP_URL" => names.url(&names.app_role, &app_password),
            "PASSWORD_PEPPER_KEYS" | "ENCRYPTION_KEYS" => keyring(),
            _ => KEY_ID.to_string(),
        };
        write_atomic(dir, install_setting_file(var), value.as_bytes())?;
    }
    write_atomic(dir, MARKER, uuid::Uuid::now_v7().to_string().as_bytes())?;
    Ok(app_password)
}

/// `--install`, after [`load_install_env`].
pub async fn install() -> Result<(), String> {
    let server_dir = Path::new(INSTALL_ROOT).join(SERVER_DIR);
    if !server_dir.is_dir() {
        return Err(format!(
            "{} is missing (mount the server settings volume)",
            server_dir.display()
        ));
    }
    let owner_url = std::env::var("DATABASE_URL").map_err(|_| {
        format!(
            "{} is missing (mount the postgres secret volume)",
            Path::new(INSTALL_ROOT).join(OWNER_PASSWORD_FILE).display()
        )
    })?;
    let names = DbNames::from_lookup(|k| std::env::var(k).ok())?;
    let mut conn = PgConnection::connect(&owner_url)
        .await
        .map_err(|e| format!("connect to PostgreSQL as {}: {e}", names.owner))?;
    let db = |e: sqlx::Error| e.to_string();
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(INSTALL_LOCK_KEY)
        .execute(&mut conn)
        .await
        .map_err(db)?;

    let app_password = match server_secrets(&server_dir, &names)? {
        ServerSecrets::Complete { app_password } => {
            eprintln!("server keys present in {}; kept", server_dir.display());
            app_password
        }
        ServerSecrets::Incomplete { missing } => {
            let existing: Option<String> = sqlx::query_scalar(
                "SELECT CASE
                   WHEN EXISTS (SELECT 1 FROM pg_roles WHERE rolname = $1::text) THEN 'role ' || $1::text
                   WHEN EXISTS (SELECT 1 FROM pg_namespace WHERE nspname = 'fvoci') THEN 'schema fvoci'
                 END",
            )
            .bind(&names.app_role)
            .fetch_one(&mut conn)
            .await
            .map_err(db)?;
            if let Some(existing) = existing {
                return Err(format!(
                    "refusing to generate new server keys: the database already holds an install ({existing}) \
                     but {} is missing {}. Restore that volume from the backup taken with this data; \
                     new keys cannot open it.",
                    server_dir.display(),
                    missing.join(", ")
                ));
            }
            let password = generate_server_secrets(&server_dir, &names)?;
            eprintln!("generated server keys in {}", server_dir.display());
            password
        }
    };

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
        eprintln!("created app role {}", names.app_role);
    }
    migrate::run_migrations(&owner_url)
        .await
        .map_err(|e| format!("migrate: {e}"))?;
    migrate::grant_app_role(&owner_url, &names.app_role)
        .await
        .map_err(|e| e.to_string())?;
    eprintln!(
        "migrated; granted app role privileges to {}",
        names.app_role
    );

    if let Some(url) = std::env::var("FVOCI_MEILI_URL")
        .ok()
        .filter(|v| !v.trim().is_empty())
    {
        ensure_meili_key_file(&server_dir.join(install_setting_file("FVOCI_MEILI_KEY"))).await?;
        write_if_changed(
            &server_dir,
            install_setting_file("FVOCI_MEILI_URL"),
            url.trim(),
        )?;
        eprintln!("search key ready");
    }
    sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(INSTALL_LOCK_KEY)
        .execute(&mut conn)
        .await
        .map_err(db)?;
    let _ = conn.close().await;
    Ok(())
}

fn write_if_changed(dir: &Path, name: &str, value: &str) -> Result<(), String> {
    match fs::read_to_string(dir.join(name)) {
        Ok(current) if current == value => Ok(()),
        _ => write_atomic(dir, name, value.as_bytes()),
    }
}

fn remove_if_present(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(format!("remove {}: {err}", path.display())),
    }
}

fn remove_stale_temps(dir: &Path) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|e| format!("read {}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("read {}: {e}", dir.display()))?;
        if entry.file_name().to_string_lossy().starts_with(TEMP_PREFIX) {
            remove_if_present(&entry.path())?;
        }
    }
    Ok(())
}

/// Mode 0600 temp file (create_new, fsync) renamed over `name`, then the
/// directory is fsynced. The file belongs to the running uid (init runs as the
/// server's uid 1000).
fn write_atomic(dir: &Path, name: &str, body: &[u8]) -> Result<(), String> {
    let temp = dir.join(format!("{TEMP_PREFIX}{}", uuid::Uuid::now_v7().simple()));
    let dest = dir.join(name);
    let written = (|| -> std::io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)?;
        file.write_all(body)?;
        file.sync_all()?;
        fs::rename(&temp, &dest)?;
        fs::File::open(dir)?.sync_all()
    })();
    if let Err(err) = written {
        let _ = fs::remove_file(&temp);
        return Err(format!("write {}: {err}", dest.display()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("fvoci-install-{}", uuid::Uuid::now_v7()));
            fs::create_dir_all(dir.join(SERVER_DIR)).unwrap();
            Self(dir)
        }
        fn server(&self) -> PathBuf {
            self.0.join(SERVER_DIR)
        }
        fn read(&self, var: &str) -> String {
            fs::read_to_string(self.server().join(install_setting_file(var))).unwrap()
        }
        fn snapshot(&self) -> Vec<(PathBuf, Vec<u8>)> {
            let mut all: Vec<_> = fs::read_dir(self.server())
                .unwrap()
                .map(|e| {
                    let path = e.unwrap().path();
                    let body = fs::read(&path).unwrap();
                    (path, body)
                })
                .collect();
            all.sort();
            all
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn names() -> DbNames {
        DbNames::from_lookup(|_| None).unwrap()
    }

    #[test]
    fn generates_server_secrets_once_and_reads_them_back() {
        let tmp = TempDir::new();
        assert!(matches!(
            server_secrets(&tmp.server(), &names()).unwrap(),
            ServerSecrets::Incomplete { .. }
        ));
        let password = generate_server_secrets(&tmp.server(), &names()).unwrap();
        assert_eq!(password.len(), 64);
        assert_eq!(
            tmp.read("DATABASE_APP_URL"),
            format!("postgres://fvoci_app:{password}@postgres:5432/fvoci")
        );
        let pepper = tmp.read("PASSWORD_PEPPER_KEYS");
        let encryption = tmp.read("ENCRYPTION_KEYS");
        assert_ne!(pepper, encryption);
        crate::auth::password::Keyring::parse(&pepper, &tmp.read("PASSWORD_PEPPER_ACTIVE_KEY_ID"))
            .unwrap();
        crate::auth::password::Keyring::parse_named(
            &encryption,
            &tmp.read("ENCRYPTION_ACTIVE_KEY_ID"),
            "ENCRYPTION_KEYS",
        )
        .unwrap();
        for (path, _) in tmp.snapshot() {
            let mode = fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "{}", path.display());
        }
        assert_eq!(
            server_secrets(&tmp.server(), &names()).unwrap(),
            ServerSecrets::Complete {
                app_password: password
            }
        );
        // The files are exactly what the server reads from its install directory.
        let resolved = crate::config::resolve_secret_files(|_| None, &tmp.server()).unwrap();
        let vars: Vec<_> = resolved.iter().map(|(n, _)| *n).collect();
        assert_eq!(vars, SERVER_SECRETS);
    }

    #[test]
    fn missing_file_or_marker_is_incomplete() {
        let tmp = TempDir::new();
        generate_server_secrets(&tmp.server(), &names()).unwrap();
        fs::remove_file(tmp.server().join("encryption_keys")).unwrap();
        assert_eq!(
            server_secrets(&tmp.server(), &names()).unwrap(),
            ServerSecrets::Incomplete {
                missing: vec!["encryption_keys".into()]
            }
        );
        generate_server_secrets(&tmp.server(), &names()).unwrap();
        // An interrupted run: files written, marker not yet; stale temp left.
        fs::remove_file(tmp.server().join(MARKER)).unwrap();
        fs::write(tmp.server().join(format!("{TEMP_PREFIX}x")), "p").unwrap();
        assert!(matches!(
            server_secrets(&tmp.server(), &names()).unwrap(),
            ServerSecrets::Incomplete { .. }
        ));
        generate_server_secrets(&tmp.server(), &names()).unwrap();
        assert!(!tmp.server().join(format!("{TEMP_PREFIX}x")).exists());
    }

    #[test]
    fn renamed_role_or_foreign_url_is_refused() {
        let tmp = TempDir::new();
        generate_server_secrets(&tmp.server(), &names()).unwrap();
        let other =
            DbNames::from_lookup(|k| (k == "FVOCI_APP_ROLE").then(|| "other".into())).unwrap();
        assert!(server_secrets(&tmp.server(), &other)
            .unwrap_err()
            .contains("another role"));
        fs::write(
            tmp.server().join("database_app_url"),
            "postgres://fvoci_app:not-hex@postgres:5432/fvoci",
        )
        .unwrap();
        let err = server_secrets(&tmp.server(), &names()).unwrap_err();
        assert!(err.contains("no generated app role password"), "{err}");
        assert!(!err.contains("not-hex"));
    }

    #[test]
    fn install_env_reads_mounted_files_only() {
        let tmp = TempDir::new();
        let none = |_: &str| None;
        assert!(install_env(&tmp.0, none).unwrap().is_empty());

        fs::create_dir_all(tmp.0.join("postgres")).unwrap();
        fs::create_dir_all(tmp.0.join("meilisearch")).unwrap();
        fs::write(tmp.0.join(OWNER_PASSWORD_FILE), "p@ss/w:rd\n").unwrap();
        fs::write(tmp.0.join(MASTER_KEY_FILE), "m".repeat(64)).unwrap();
        fs::write(tmp.server().join("meili_api_key"), "k".repeat(64)).unwrap();
        let env = install_env(&tmp.0, none).unwrap();
        assert_eq!(
            env,
            vec![
                (
                    "DATABASE_URL",
                    "postgres://fvoci_owner:p%40ss%2Fw%3Ard@postgres:5432/fvoci".to_string()
                ),
                ("MEILI_MASTER_KEY", "m".repeat(64)),
                ("FVOCI_MEILI_URL", DEFAULT_MEILI_URL.to_string()),
                (
                    "FVOCI_MEILI_KEY_FILE",
                    tmp.server().join("meili_api_key").display().to_string()
                ),
            ]
        );
        for var in ["DATABASE_URL", "FVOCI_MIGRATION_URL", "MEILI_MASTER_KEY"] {
            let err = install_env(&tmp.0, |k| (k == var).then(String::new)).unwrap_err();
            assert!(err.starts_with(&format!("{var} is set and")), "{err}");
            assert!(!err.contains("p@ss"), "{err}");
        }
        assert!(install_env(&tmp.0, |k| (k == "POSTGRES_DB").then(|| "Bad-Name".into())).is_err());
        assert!(install_env(&tmp.0, |k| (k == "FVOCI_DB_HOST").then(|| "h/x@y".into())).is_err());
    }
}
