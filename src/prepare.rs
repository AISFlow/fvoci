//! Container startup of the Compose install (`fvoci-migrate --start`, the
//! image entrypoint).
//!
//! It first refuses, with exit 2, the `<VAR>_FILE` secret settings of older
//! `compose.yml` files ([`retired_secret_files`]), whether or not the owner
//! password is given. Then, without the database owner password it only execs
//! `fvoci-server` (the separate-`init` installs of `infra/rust/compose.yml`).
//! With it (`infra/rust/compose.user.yml`, which passes the `.env` values as
//! container environment):
//!
//! 1. validate the required settings (missing, placeholder, format), naming
//!    variables only;
//! 2. wait for PostgreSQL and Meilisearch with a deadline, leaving on
//!    SIGTERM/SIGINT;
//! 3. under an advisory lock as the owner: refuse a schema upgrade while
//!    another server holds app-role sessions, create the app role, migrate,
//!    grant, check the app role's password, ensure the scoped search key;
//! 4. close every prep connection and `exec` `fvoci-server`, as the image's
//!    service uid/gid when started as root ([`exec_server`]), with the
//!    environment minus the owner password, the Meilisearch master key and
//!    the raw app password ([`PREP_ONLY`]); it gets `DATABASE_APP_URL`.
//!
//! The server keeps the process id, so signals, shutdown and child reaping
//! are the server's own. The boundary is the uid: the preparation is root's,
//! the server and its children run as uid 1000. The [`PREP_ONLY`] filter
//! shapes only the server's own environment: the values stay in the container
//! configuration, so every `docker exec` and healthcheck process starts with
//! them (by default as root, whose environment uid 1000 cannot read), and
//! anyone with Docker access can read them. Nothing here generates or stores
//! keys.

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

/// The image's service account (`infra/rust/Dockerfile` `useradd`); the server
/// runs as it when the entrypoint starts as root.
pub const SERVER_UID: u32 = 1000;
pub const SERVER_GID: u32 = 1000;

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
pub(crate) const PREPARE_LOCK_KEY: i64 = 0x6676_6f63_7072_6570;

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
/// `POSTGRES_PASSWORD`, with it `DATABASE_APP_URL` from `FVOCI_APP_PASSWORD`, and
/// with `MEILI_MASTER_KEY` the Meilisearch URL and key file. Owner commands
/// (`--recover-outbox`, `--rebuild-search`, ...) then work in the `fvoci`
/// container as they do in `init`. Both forms set is an error.
pub fn install_env(
    get: impl Fn(&str) -> Option<String>,
) -> Result<Vec<(&'static str, String)>, String> {
    let set = |name: &str| get(name).is_some();
    let mut out = Vec::new();
    let selected = get("FVOCI_DATABASE_BACKEND").unwrap_or_else(|| "postgres".into());
    if selected != "postgres" {
        crate::config::DatabaseSettings::from_lookup(&get)?;
        for name in [
            "POSTGRES_PASSWORD",
            "FVOCI_APP_PASSWORD",
            "DATABASE_URL",
            "FVOCI_MIGRATION_URL",
        ] {
            if set(name) {
                return Err(format!("{name} conflicts with FVOCI_DATABASE_BACKEND"));
            }
        }
    }
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
    if let (true, Some(password)) = (set("POSTGRES_PASSWORD"), get("FVOCI_APP_PASSWORD")) {
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
    let postgres = get("FVOCI_DATABASE_BACKEND").is_none_or(|kind| kind == "postgres");
    if !postgres {
        if let Err(error) = crate::config::DatabaseSettings::from_lookup(&get) {
            problems.push(error);
        }
    }
    for name in REQUIRED {
        if !postgres
            && (matches!(*name, "POSTGRES_PASSWORD" | "FVOCI_APP_PASSWORD")
                || (*name == "MEILI_MASTER_KEY" && get("FVOCI_MEILI_URL").is_none()))
        {
            continue;
        }
        let Some(value) = get(name) else {
            problems.push(format!("{name} is not set (see the env example)"));
            continue;
        };
        let lower = value.to_ascii_lowercase();
        if value.trim().is_empty() {
            problems.push(format!("{name} is empty (see the env example)"));
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
                "{name} still holds an example placeholder; generate a value as the env example shows"
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
        if !postgres
            && (matches!(name, "POSTGRES_PASSWORD" | "FVOCI_APP_PASSWORD")
                || (name == "MEILI_MASTER_KEY" && get("FVOCI_MEILI_URL").is_none()))
        {
            continue;
        }
        if value(name).trim().chars().count() < MIN_SECRET_LEN {
            problems.push(format!(
                "{name} must be at least {MIN_SECRET_LEN} characters (e.g. openssl rand -hex 32)"
            ));
        }
    }
    if postgres && value("POSTGRES_PASSWORD") == value("FVOCI_APP_PASSWORD") {
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

/// Values the Compose files of 0.2.0 and earlier passed as secret files named
/// by `<VAR>_FILE`.
const RETIRED_SECRET_FILE_VALUES: &[&str] = &[
    "POSTGRES_PASSWORD",
    "FVOCI_APP_PASSWORD",
    "MEILI_MASTER_KEY",
    "PASSWORD_PEPPER_KEYS",
    "ENCRYPTION_KEYS",
];

/// One problem per `<VAR>_FILE` setting of such an older `compose.yml`, which
/// nothing reads any more, naming the variable only. `--start` refuses them
/// before anything else: with only those names set it would skip the
/// preparation and the server would fail without naming the cause.
pub fn retired_secret_files(get: impl Fn(&str) -> Option<String>) -> Vec<String> {
    RETIRED_SECRET_FILE_VALUES
        .iter()
        .filter_map(|name| {
            let file = format!("{name}_FILE");
            get(&file).map(|_| {
                format!(
                    "{file} is no longer read; use this release's compose.yml, which passes {name} from .env"
                )
            })
        })
        .collect()
}

/// Whether this start prepares the install (the owner password is given).
pub fn wants_prepare() -> bool {
    std::env::var_os("POSTGRES_PASSWORD").is_some()
        || std::env::var_os("FVOCI_DATABASE_BACKEND").is_some_and(|kind| kind != "postgres")
}

#[derive(Debug, thiserror::Error)]
pub enum PreparationError {
    #[error("{0}")]
    Ordinary(String),
    #[error("SQLite preparation original result {original:?}; owned drain/thread join {drain:?}; test control {control:?}")]
    Sqlite {
        #[source]
        original: Option<sqlx::Error>,
        drain: Option<Result<migrate::SqliteMigrationDrain, sqlx::Error>>,
        control: Option<String>,
    },
    #[error("preparation cancelled; SQLite owned drain/thread join: {sqlite_drain:?}")]
    Cancelled {
        sqlite_drain: Option<migrate::SqliteMigrationDrain>,
    },
    /// The qualified remote helper refused or failed. The bounded error keeps
    /// its closed code, gate, stage, settlement and drain meaning; its Display
    /// and Debug never carry the settings, endpoint, token or raw driver text.
    #[error("remote preparation refused ({code}, gate {gate}, settlement {settlement}): {0}", code = .0.code(), gate = .0.gate().unwrap_or("none"), settlement = .0.settlement())]
    Remote(#[source] migrate::RemoteMigrationError),
    /// A signal arrived after the remote migration settled: the settled
    /// result stays truthful and the server is not started.
    #[error(
        "preparation cancelled after the remote migration settled; no server start: {outcome:?}"
    )]
    RemoteCancelledAfterSettlement {
        outcome: migrate::RemoteMigrationOutcome,
    },
}

/// Startup refusal text of the remote lane for `fvoci-server`: the bounded
/// error display with its closed codes; never the settings, endpoint, token
/// or raw driver text.
pub fn remote_startup_refusal(error: &migrate::RemoteMigrationError) -> String {
    format!(
        "remote normal startup refused ({}, gate {}, settlement {}): {error}",
        error.code(),
        error.gate().unwrap_or("none"),
        error.settlement()
    )
}

/// Exit code and operator text of the bare remote migrator. `0` only for a
/// settled `Current`, `Installed` or `Resumed` without a signal (the helper
/// has already drained its stream, otherwise it returns `Drain`); a signal
/// exits with its own code after the original future settled and keeps the
/// settled result in the text; every error is non-zero with the bounded
/// display (an unknown commit already carries the same-command rerun
/// guidance). Nothing is retried here.
pub fn remote_migrator_exit(
    signal_code: Option<i32>,
    result: &Result<migrate::RemoteMigrationOutcome, migrate::RemoteMigrationError>,
) -> (i32, String) {
    let settled = match result {
        Ok(outcome) => format!("remote migration settled: {outcome:?}"),
        Err(error) => format!(
            "remote migration failed ({}, gate {}, settlement {}): {error}",
            error.code(),
            error.gate().unwrap_or("none"),
            error.settlement()
        ),
    };
    match (signal_code, result) {
        (Some(code), _) => (
            code,
            format!("signal during the remote migration; original result kept, no server start: {settled}"),
        ),
        (None, Ok(_)) => (0, settled),
        (None, Err(_)) => (1, settled),
    }
}

#[cfg(feature = "db-tests")]
async fn start_preparation_migration_with_test_gate(
    path: &Path,
) -> Result<
    (
        migrate::SqliteMigration,
        Option<tokio::task::JoinHandle<Result<(), std::io::Error>>>,
    ),
    PreparationError,
> {
    use std::io::{Read, Write};
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    let Some(socket) = std::env::var_os("FVOCI_TEST_SQLITE_GATE_SOCKET") else {
        return migrate::start_sqlite_migration(path)
            .map(|migration| (migration, None))
            .map_err(|error| PreparationError::Sqlite {
                original: Some(error),
                drain: None,
                control: None,
            });
    };
    let socket = PathBuf::from(socket);
    let parent = socket
        .parent()
        .ok_or("isolated migration gate parent missing")?;
    let parent_meta = std::fs::symlink_metadata(parent).map_err(|error| error.to_string())?;
    let socket_meta = std::fs::symlink_metadata(&socket).map_err(|error| error.to_string())?;
    let own_uid = std::fs::metadata("/proc/self")
        .map_err(|error| error.to_string())?
        .uid();
    if !socket.is_absolute()
        || !parent_meta.is_dir()
        || parent_meta.uid() != own_uid
        || parent_meta.mode() & 0o077 != 0
        || !socket_meta.file_type().is_socket()
        || socket_meta.uid() != own_uid
    {
        return Err(
            "migration gate requires an existing socket in this UID's private isolated directory"
                .into(),
        );
    }
    let phase = std::env::var("FVOCI_TEST_SQLITE_GATE_PHASE")
        .map_err(|_| "migration gate phase missing")?;
    if !matches!(phase.as_str(), "commit" | "close") {
        return Err("migration gate phase must be commit or close".into());
    }
    let mut stream =
        std::os::unix::net::UnixStream::connect(socket).map_err(|error| error.to_string())?;
    let (connected, connection) = tokio::sync::oneshot::channel();
    let (proceed, start) = tokio::sync::oneshot::channel();
    let (cleanup_started, cleanup) = tokio::sync::oneshot::channel();
    let migration = migrate::start_sqlite_migration_controlled(
        path,
        migrate::SqliteMigrationTestControl {
            connected,
            proceed: start,
            cleanup_started: Some(cleanup_started),
        },
    )
    .map_err(|error| PreparationError::Sqlite {
        original: Some(error),
        drain: None,
        control: None,
    })?;
    let setup: Result<Option<tokio::task::JoinHandle<Result<(), std::io::Error>>>, String> =
        async {
            let connected = connection
                .await
                .map_err(|_| "migration gate owner ended before connection")?;
            let mut held = connected
                .acquire()
                .await
                .map_err(|error| error.to_string())?;
            if phase == "commit" {
                {
                    let mut native = held
                        .lock_handle()
                        .await
                        .map_err(|error| error.to_string())?;
                    let mut first = true;
                    native.set_commit_hook(move || {
                        if !first {
                            return true;
                        }
                        first = false;
                        let mut release = [0];
                        // Supported SQLx hook pauses the actual SQLite worker's
                        // first real DDL/marker COMMIT. No fabricated SQL result.
                        stream.write_all(b"C").is_ok()
                            && stream.read_exact(&mut release).is_ok()
                            && release == *b"R"
                    });
                }
                drop(held);
                let _ = proceed.send(());
                Ok(None)
            } else {
                // Cancel the actual owner while holding its original pool connection;
                // it enters real explicit close, which cannot acquire that connection.
                // External SIGTERM must still await this owner instead of exiting.
                migration.cancel();
                let _ = proceed.send(());
                stream
                    .set_nonblocking(true)
                    .map_err(|error| error.to_string())?;
                let mut stream =
                    tokio::net::UnixStream::from_std(stream).map_err(|error| error.to_string())?;
                let helper = tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let result = async {
                        cleanup.await.map_err(|_| {
                            std::io::Error::other("migration gate owner ended before close")
                        })?;
                        stream.write_all(b"L").await?;
                        let mut release = [0];
                        stream.read_exact(&mut release).await?;
                        if release != *b"R" {
                            return Err(std::io::Error::other("migration gate release invalid"));
                        }
                        Ok(())
                    }
                    .await;
                    drop(held);
                    result
                });
                Ok(Some(helper))
            }
        }
        .await;
    match setup {
        Ok(helper) => Ok((migration, helper)),
        Err(control) => {
            migration.cancel();
            let outcome = migration
                .wait_with_cancel(&tokio_util::sync::CancellationToken::new())
                .await;
            Err(PreparationError::Sqlite {
                original: outcome.result.err(),
                drain: Some(outcome.drain),
                control: Some(control),
            })
        }
    }
}
impl From<String> for PreparationError {
    fn from(value: String) -> Self {
        Self::Ordinary(value)
    }
}
impl From<&str> for PreparationError {
    fn from(value: &str) -> Self {
        Self::Ordinary(value.to_owned())
    }
}

/// Root installation uses a dedicated directory. Protect it before any
/// root migration; hand it to the service only after the original owner joins.
struct SqliteInstallDirectory {
    directory: PathBuf,
    ancestors: Vec<(PathBuf, std::fs::File)>,
    existing_inode: Option<(u64, u64)>,
}
impl SqliteInstallDirectory {
    fn protect(path: &Path) -> Result<Self, String> {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
        let directory = path
            .parent()
            .filter(|parent| parent.parent().is_some())
            .ok_or("SQLite installation requires a dedicated existing parent directory")?
            .to_path_buf();
        let mut ancestors = Vec::new();
        let mut literal = PathBuf::new();
        for component in directory.components() {
            match component {
                std::path::Component::RootDir | std::path::Component::Normal(_) => {
                    literal.push(component)
                }
                _ => return Err("SQLite installation rejects nonliteral parent components".into()),
            }
            let file = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(migrate::SQLITE_NOFOLLOW)
                .open(&literal)
                .map_err(|error| format!("SQLite installation parent open: {error}"))?;
            let meta = file.metadata().map_err(|error| error.to_string())?;
            let parent = literal == directory;
            if !meta.is_dir()
                || meta.mode() & 0o022 != 0
                || if parent {
                    !matches!(meta.uid(), 0 | SERVER_UID)
                } else {
                    meta.uid() != 0
                        || (meta.mode() & 0o001 == 0
                            && !(meta.gid() == SERVER_GID && meta.mode() & 0o010 != 0))
                }
            {
                return Err("SQLite installation requires root-controlled ancestors and an owned dedicated parent".into());
            }
            ancestors.push((literal.clone(), file));
        }
        let mut install = Self {
            directory,
            ancestors,
            existing_inode: None,
        };
        install.verify_literal()?;
        install.owned_entries(path)?;
        let admission = match std::fs::symlink_metadata(path) {
            Ok(_) => Some(
                migrate::SqliteAdmission::installation_handoff(path)
                    .map_err(|error| format!("SQLite installation admission: {error}"))?,
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.to_string()),
        };
        if let Some(admission) = &admission {
            let meta = admission
                .admitted_file()
                .metadata()
                .map_err(|error| error.to_string())?;
            install.existing_inode = Some((meta.dev(), meta.ino()));
            install.verify_admitted_file(path, admission.admitted_file())?;
        }
        // Admission refusal above leaves a live server's directory unchanged.
        // Root takes this exact open directory, never a path-based chown.
        let parent = &install.ancestors.last().expect("dedicated directory").1;
        std::os::unix::fs::fchown(parent, Some(0), Some(0)).map_err(|error| error.to_string())?;
        parent
            .set_permissions(std::fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?;
        install.verify_literal()?;
        install.owned_entries(path)?;
        if let Some(admission) = &admission {
            install.verify_admitted_file(path, admission.admitted_file())?;
        }
        // Parent is now root/private. Actual migration reuses the same inode
        // and admission policy; no service process can swap an entry here.
        drop(admission);
        Ok(install)
    }

    fn verify_literal(&self) -> Result<(), String> {
        use std::os::unix::fs::MetadataExt;
        for (path, file) in &self.ancestors {
            let actual = file.metadata().map_err(|error| error.to_string())?;
            let named = std::fs::symlink_metadata(path).map_err(|error| error.to_string())?;
            if !named.is_dir() || named.dev() != actual.dev() || named.ino() != actual.ino() {
                return Err("SQLite installation ancestor/parent inode changed".into());
            }
        }
        Ok(())
    }

    fn owned_entries(&self, path: &Path) -> Result<Vec<std::fs::File>, String> {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        let name = path
            .file_name()
            .ok_or("SQLite installation file name missing")?;
        let mut wal = name.to_os_string();
        wal.push("-wal");
        let mut shm = name.to_os_string();
        shm.push("-shm");
        let mut files = Vec::new();
        for entry in std::fs::read_dir(&self.directory).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            if entry.file_name() != name && entry.file_name() != wal && entry.file_name() != shm {
                return Err("SQLite installation parent contains unrelated entries".into());
            }
            let file = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(migrate::SQLITE_NOFOLLOW)
                .open(entry.path())
                .map_err(|error| format!("SQLite installation DB entry no-follow open: {error}"))?;
            let meta = file.metadata().map_err(|error| error.to_string())?;
            let named =
                std::fs::symlink_metadata(entry.path()).map_err(|error| error.to_string())?;
            if !meta.is_file()
                || meta.nlink() != 1
                || !matches!(meta.uid(), 0 | SERVER_UID)
                || meta.mode() & 0o077 != 0
                || !named.is_file()
                || (named.dev(), named.ino()) != (meta.dev(), meta.ino())
            {
                return Err(
                    "SQLite installation refuses foreign/symlink/hardlinked/nonprivate DB entries"
                        .into(),
                );
            }
            files.push(file);
        }
        Ok(files)
    }

    fn verify_admitted_file(&self, path: &Path, file: &std::fs::File) -> Result<(), String> {
        use std::os::unix::fs::MetadataExt;
        self.verify_literal()?;
        let actual = file.metadata().map_err(|error| error.to_string())?;
        let named = std::fs::symlink_metadata(path).map_err(|error| error.to_string())?;
        if !named.is_file()
            || actual.nlink() != 1
            || !matches!(actual.uid(), 0 | SERVER_UID)
            || (named.dev(), named.ino()) != (actual.dev(), actual.ino())
            || self
                .existing_inode
                .is_some_and(|inode| inode != (actual.dev(), actual.ino()))
        {
            return Err("SQLite installation admitted DB inode/owner differs".into());
        }
        Ok(())
    }

    fn handoff(self, path: &Path) -> Result<(), String> {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let admission = migrate::SqliteAdmission::installation_handoff(path)
            .map_err(|error| format!("SQLite installation final admission: {error}"))?;
        self.verify_admitted_file(path, admission.admitted_file())?;
        let files = self.owned_entries(path)?;
        self.verify_literal()?;
        for file in files {
            std::os::unix::fs::fchown(&file, Some(SERVER_UID), Some(SERVER_GID))
                .map_err(|error| error.to_string())?;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|error| error.to_string())?;
            let meta = file.metadata().map_err(|error| error.to_string())?;
            if meta.uid() != SERVER_UID || meta.gid() != SERVER_GID || meta.mode() & 0o777 != 0o600
            {
                return Err("SQLite installation file handoff unconfirmed".into());
            }
        }
        self.verify_admitted_file(path, admission.admitted_file())?;
        let parent = &self.ancestors.last().expect("dedicated directory").1;
        parent
            .set_permissions(std::fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?;
        std::os::unix::fs::fchown(parent, Some(SERVER_UID), Some(SERVER_GID))
            .map_err(|error| error.to_string())?;
        let meta = parent.metadata().map_err(|error| error.to_string())?;
        if meta.uid() != SERVER_UID || meta.gid() != SERVER_GID || meta.mode() & 0o777 != 0o700 {
            return Err("SQLite installation directory handoff unconfirmed".into());
        }
        self.verify_literal()?;
        // Same exclusive admission is retained through the confirmed handoff.
        drop(admission);
        Ok(())
    }
}

/// Steps 2-3 for explicit preparation without a signal cancellation owner.
pub async fn prepare() -> Result<(), String> {
    prepare_with_cancel(&tokio_util::sync::CancellationToken::new())
        .await
        .map_err(|error| error.to_string())
}

/// The signal owner keeps this future alive until cancellation has joined
/// the actual SQLite migration owner. Dropping it is not an exit receipt.
pub async fn prepare_with_cancel(
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<(), PreparationError> {
    // Preserve the owner-only PostgreSQL preparation entry: it does not need
    // to parse a normal app connection before deriving/checking that role.
    match std::env::var("FVOCI_DATABASE_BACKEND").as_deref() {
        Err(std::env::VarError::NotPresent) | Ok("postgres") => {
            return tokio::select! {
                result = prepare_postgres() => result.map_err(PreparationError::Ordinary),
                _ = cancel.cancelled() => Err(PreparationError::Cancelled { sqlite_drain: None }),
            };
        }
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err("FVOCI_DATABASE_BACKEND must be Unicode".into())
        }
        _ => {}
    }
    match crate::config::DatabaseSettings::from_env()? {
        crate::config::DatabaseSettings::Postgres { .. } => unreachable!(),
        crate::config::DatabaseSettings::Sqlite { path } => {
            // Preparation owns creation and the controlled migration actor;
            // normal server startup opens only this prepared existing file.
            let deadline = prepare_deadline()?;
            let meili = std::env::var("FVOCI_MEILI_URL")
                .ok()
                .filter(|value| !value.trim().is_empty());
            if let Some(url) = &meili {
                tokio::select! {
                    result = wait_for_meili(url.trim(), deadline) => result?,
                    _ = cancel.cancelled() => return Err(PreparationError::Cancelled { sqlite_drain: None }),
                }
            }
            if cancel.is_cancelled() {
                return Err(PreparationError::Cancelled { sqlite_drain: None });
            }
            let install = if running_as_root()? {
                Some(SqliteInstallDirectory::protect(&path)?)
            } else {
                None
            };
            #[cfg(not(feature = "db-tests"))]
            let migration = migrate::start_sqlite_migration(&path).map_err(|error| {
                PreparationError::Sqlite {
                    original: Some(error),
                    // No owner started; no invented close/join receipt.
                    drain: None,
                    control: None,
                }
            })?;
            #[cfg(feature = "db-tests")]
            let (migration, control) = start_preparation_migration_with_test_gate(&path).await?;
            let outcome = migration.wait_with_cancel(cancel).await;
            #[cfg(feature = "db-tests")]
            let control_error = match control {
                Some(helper) => match helper.await {
                    Ok(Ok(())) => None,
                    Ok(Err(error)) => Some(error.to_string()),
                    Err(error) => Some(error.to_string()),
                },
                None => None,
            };
            #[cfg(not(feature = "db-tests"))]
            let control_error = None;
            if outcome.result.is_err()
                || !matches!(
                    outcome.drain.as_ref(),
                    Ok(migrate::SqliteMigrationDrain::Closed)
                )
                || control_error.is_some()
            {
                return Err(PreparationError::Sqlite {
                    original: outcome.result.err(),
                    drain: Some(outcome.drain),
                    control: control_error,
                });
            }
            tracing::info!(
                drain = "Closed",
                thread_joined = true,
                "SQLite preparation owner finished"
            );
            if cancel.is_cancelled() {
                return Err(PreparationError::Cancelled {
                    sqlite_drain: Some(migrate::SqliteMigrationDrain::Closed),
                });
            }
            if let Some(install) = install {
                install.handoff(&path)?;
            }
            if meili.is_some() {
                let key_file = std::env::var("FVOCI_MEILI_KEY_FILE")
                    .unwrap_or_else(|_| DEFAULT_MEILI_KEY_FILE.to_string());
                tokio::select! {
                    result = ensure_meili_key_file(Path::new(&key_file)) => result?,
                    _ = cancel.cancelled() => return Err(PreparationError::Cancelled {
                        sqlite_drain: Some(migrate::SqliteMigrationDrain::Closed),
                    }),
                }
            }
            Ok(())
        }
        settings @ crate::config::DatabaseSettings::LibsqlRemote { .. } => {
            // Remote primary preparation through the qualified helper: one
            // admitted stream, blank or exact-prefix states installed or
            // resumed, current left untouched, every other state refused
            // before any write, always closed and drained. There is no
            // file-system install directory or handoff for a remote primary,
            // and preparation never advertises readiness: the server start
            // after it goes through its own current-only startup gate.
            let deadline = prepare_deadline()?;
            let meili = std::env::var("FVOCI_MEILI_URL")
                .ok()
                .filter(|value| !value.trim().is_empty());
            if let Some(url) = &meili {
                tokio::select! {
                    result = wait_for_meili(url.trim(), deadline) => result?,
                    _ = cancel.cancelled() => return Err(PreparationError::Cancelled { sqlite_drain: None }),
                }
            }
            if cancel.is_cancelled() {
                return Err(PreparationError::Cancelled { sqlite_drain: None });
            }
            let outcome = migrate::run_remote_migrations(&settings, cancel)
                .await
                .map_err(PreparationError::Remote)?;
            tracing::info!(outcome = ?outcome, "remote primary preparation settled");
            if cancel.is_cancelled() {
                return Err(PreparationError::RemoteCancelledAfterSettlement { outcome });
            }
            if meili.is_some() {
                let key_file = std::env::var("FVOCI_MEILI_KEY_FILE")
                    .unwrap_or_else(|_| DEFAULT_MEILI_KEY_FILE.to_string());
                tokio::select! {
                    result = ensure_meili_key_file(Path::new(&key_file)) => result?,
                    _ = cancel.cancelled() => return Err(PreparationError::RemoteCancelledAfterSettlement { outcome }),
                }
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod remote_caller_tests {
    use super::*;
    use crate::db::migrate::{GateStage, RemoteMigrationError, RemoteMigrationOutcome};

    const SECRET: &str = "https://primary.secret.example/?authToken=TOKEN-VALUE";

    fn driver_secret() -> sqlx::Error {
        sqlx::Error::AnyDriverError(SECRET.into())
    }
    fn protocol_secret() -> sqlx::Error {
        sqlx::Error::Protocol(format!("gate text leaking {SECRET}"))
    }
    fn assert_secret_free(text: &str) {
        for needle in ["secret.example", "TOKEN-VALUE", "authToken", "leaking"] {
            assert!(!text.contains(needle), "{needle} leaked: {text}");
        }
    }

    #[test]
    fn startup_refusal_repeats_only_closed_codes_and_bounded_text() {
        let error = RemoteMigrationError::Gate {
            stage: GateStage::StartupGate,
            source: driver_secret(),
            drain: Some(protocol_secret()),
        };
        let text = remote_startup_refusal(&error);
        assert_secret_free(&text);
        assert!(text.starts_with("remote normal startup refused (REMOTE_SCHEMA_GATE_REFUSED, gate none, settlement drain-failed): remote startup gate refused"), "{text}");
        let ahead = RemoteMigrationError::Gate {
            stage: GateStage::StartupGate,
            source: sqlx::Error::Protocol(
                "SQLite schema is ahead, incomplete or unprepared".into(),
            ),
            drain: None,
        };
        let text = remote_startup_refusal(&ahead);
        assert!(
            text.contains("gate GATE_AHEAD_INCOMPLETE_UNPREPARED")
                && text.contains("settlement no-write-opened"),
            "{text}"
        );
    }

    #[test]
    fn migrator_exit_is_zero_only_for_a_settled_outcome_without_a_signal() {
        let (code, text) = remote_migrator_exit(None, &Ok(RemoteMigrationOutcome::Current));
        assert_eq!(code, 0);
        assert_eq!(text, "remote migration settled: Current");
        let (code, text) = remote_migrator_exit(
            None,
            &Ok(RemoteMigrationOutcome::Resumed { from: 1, to: 12 }),
        );
        assert_eq!(
            (code, text.as_str()),
            (0, "remote migration settled: Resumed { from: 1, to: 12 }")
        );
        // A signal after settlement keeps the truthful result and exits with the signal code.
        let (code, text) = remote_migrator_exit(
            Some(143),
            &Ok(RemoteMigrationOutcome::Installed { steps: 12 }),
        );
        assert_eq!(code, 143);
        assert!(
            text.contains("no server start") && text.contains("Installed { steps: 12 }"),
            "{text}"
        );
        // Unknown commit: non-zero, closed code, rerun guidance, no secret.
        let unknown = RemoteMigrationError::CommitUnknown {
            version: 3,
            source: driver_secret(),
            drain: None,
        };
        let (code, text) = remote_migrator_exit(None, &Err(unknown));
        assert_eq!(code, 1);
        assert_secret_free(&text);
        assert!(
            text.contains("REMOTE_MIGRATION_COMMIT_UNKNOWN")
                && text.contains("settlement commit-unknown")
                && text.contains("rerun resumes from the ledger"),
            "{text}"
        );
        // A signal during a failing run still reports the failure, never zero.
        let cancelled = RemoteMigrationError::Cancelled {
            last_settled: 2,
            drain: Some(protocol_secret()),
        };
        let (code, text) = remote_migrator_exit(Some(130), &Err(cancelled));
        assert_eq!(code, 130);
        assert_secret_free(&text);
        assert!(
            text.contains("REMOTE_MIGRATION_CANCELLED") && text.contains("settlement drain-failed"),
            "{text}"
        );
        let (code, _) = remote_migrator_exit(
            None,
            &Err(RemoteMigrationError::Drain {
                source: driver_secret(),
            }),
        );
        assert_eq!(code, 1);
    }

    #[test]
    fn preparation_error_variants_keep_meaning_without_secrets() {
        let error = PreparationError::Remote(RemoteMigrationError::Step {
            version: 7,
            source: protocol_secret(),
            drain: Some(driver_secret()),
        });
        let display = error.to_string();
        let debug = format!("{error:?}");
        assert_secret_free(&display);
        assert_secret_free(&debug);
        assert!(display.starts_with("remote preparation refused (REMOTE_MIGRATION_STEP_FAILED, gate none, settlement drain-failed): remote migration step 7 failed"), "{display}");
        assert!(
            debug.contains("REMOTE_MIGRATION_STEP_FAILED") && debug.contains("<withheld>"),
            "{debug}"
        );
        assert!(std::error::Error::source(&error).is_some());
        let settled = PreparationError::RemoteCancelledAfterSettlement {
            outcome: RemoteMigrationOutcome::Resumed { from: 4, to: 12 },
        };
        assert_eq!(
            settled.to_string(),
            "preparation cancelled after the remote migration settled; no server start: Resumed { from: 4, to: 12 }"
        );
    }
}

fn prepare_deadline() -> Result<tokio::time::Instant, String> {
    Ok(tokio::time::Instant::now()
        + Duration::from_secs(match std::env::var("FVOCI_PREPARE_TIMEOUT_SECS") {
            Ok(raw) => raw
                .trim()
                .parse::<u64>()
                .ok()
                .filter(|value| *value > 0)
                .ok_or("FVOCI_PREPARE_TIMEOUT_SECS must be a positive number of seconds")?,
            Err(_) => DEFAULT_PREPARE_TIMEOUT_SECS,
        }))
}

async fn prepare_postgres() -> Result<(), String> {
    let owner_url = std::env::var("DATABASE_URL").map_err(|_| "DATABASE_URL is not derived")?;
    let names = DbNames::from_lookup(|k| std::env::var(k).ok())?;
    let deadline = prepare_deadline()?;

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
    // The ledger is read on the owner connection: a retired development lineage
    // or any receipt set that is not an exact prefix of the compiled lineage is
    // refused here, before pending steps are counted.
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(owner_url)
        .await
        .map_err(db)?;
    let ledger = async {
        let mut owner = pool.acquire().await?;
        migrate::read_postgres_ledger(&mut owner).await
    }
    .await;
    pool.close().await;
    let compiled = migrate::compiled_postgres_steps();
    let pending = match ledger.map_err(db)? {
        migrate::LedgerState::Unprepared => return Ok(()),
        retired @ migrate::LedgerState::Retired { .. } => {
            return Err(migrate::schema_gate(&retired, &compiled)
                .err()
                .unwrap_or_default());
        }
        migrate::LedgerState::Applied(applied) => migrate::pending_steps(&applied, &compiled)?,
    };
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

/// Whether this process runs as root (the owner of `/proc/self`).
pub fn running_as_root() -> Result<bool, String> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata("/proc/self")
        .map(|m| m.uid() == 0)
        .map_err(|e| format!("cannot tell the process uid from /proc/self: {e}"))
}

/// Whether `exec_server` keeps the variable `name`.
fn passed_to_server(name: &str) -> bool {
    !PREP_ONLY.contains(&name)
}

/// Replaces this process with `fvoci-server args`, its environment minus
/// [`PREP_ONLY`]. Started as root (the Compose install), it first becomes
/// [`SERVER_UID`]:[`SERVER_GID`] with no supplementary groups, so the server
/// and its children hold no capability. The server keeps the keyrings and
/// `DATABASE_APP_URL` in its environment; its same-uid helpers cannot read
/// them only because `server_main` makes the server non-dumpable before it
/// starts any helper, and each helper starts with a cleared environment.
/// Every descriptor std opens is close-on-exec. Returns only on failure.
pub fn exec_server(args: &[String]) -> String {
    let path = match server_path() {
        Ok(path) => path,
        Err(e) => return e,
    };
    let root = match running_as_root() {
        Ok(root) => root,
        Err(e) => return e,
    };
    let env = std::env::vars_os().filter(|(k, _)| k.to_str().is_none_or(passed_to_server));
    let mut command = std::process::Command::new(&path);
    command.args(args).env_clear().envs(env);
    if root {
        // std sets the gid, clears the supplementary groups, then the uid.
        command
            .gid(SERVER_GID)
            .uid(SERVER_UID)
            .env("HOME", "/nonexistent");
    }
    let err = command.exec();
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

    #[test]
    fn sqlite_preparation_validates_keys_without_deriving_postgres_credentials() {
        let mut selected = valid();
        selected.retain(|(name, _)| {
            !matches!(
                *name,
                "POSTGRES_PASSWORD" | "FVOCI_APP_PASSWORD" | "MEILI_MASTER_KEY"
            )
        });
        selected.extend([
            ("FVOCI_DATABASE_BACKEND", "sqlite".into()),
            ("FVOCI_SQLITE_PATH", "/owned/wiki.sqlite".into()),
        ]);
        assert!(validate(get(&selected)).is_empty());
        assert!(install_env(get(&selected)).unwrap().is_empty());
        for name in [
            "POSTGRES_PASSWORD",
            "FVOCI_APP_PASSWORD",
            "DATABASE_URL",
            "FVOCI_MIGRATION_URL",
        ] {
            let mut mixed = selected.clone();
            mixed.push((name, "synthetic-owner-secret".into()));
            let error = install_env(get(&mixed)).unwrap_err();
            assert!(error.contains(name));
            assert!(!error.contains("synthetic-owner-secret"));
        }
        selected.retain(|(name, _)| *name != "ENCRYPTION_KEYS");
        assert!(validate(get(&selected))
            .iter()
            .any(|error| error.contains("ENCRYPTION_KEYS")));
        selected.push(("FVOCI_MEILI_URL", "http://127.0.0.1:1".into()));
        assert!(validate(get(&selected))
            .iter()
            .any(|error| error.contains("MEILI_MASTER_KEY")));
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
                "MEILI_MASTER_KEY is not set (see the env example)".to_string(),
                "ENCRYPTION_KEYS is empty (see the env example)".to_string(),
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
        // compose.yml's init has the app password but no owner password:
        // its database names may differ, so no app URL is guessed.
        assert!(install_env(lookup(&[("FVOCI_APP_PASSWORD", "app")]))
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

    #[test]
    fn server_env_keeps_keyrings_but_no_prep_value() {
        for kept in [
            "PASSWORD_PEPPER_KEYS",
            "ENCRYPTION_KEYS",
            "DATABASE_APP_URL",
            "FVOCI_BIND",
            // Operator settings from compose `environment:` (e.g. the
            // metrics override) must reach the server unchanged.
            "METRICS_ALLOW_IPS",
            "FVOCI_COLLAB_MEMORY_BUDGET",
        ] {
            assert!(passed_to_server(kept), "{kept}");
        }
        for dropped in [
            "POSTGRES_PASSWORD",
            "FVOCI_APP_PASSWORD",
            "MEILI_MASTER_KEY",
            "FVOCI_MEILI_MASTER_KEY",
            "DATABASE_URL",
            "FVOCI_MIGRATION_URL",
        ] {
            assert!(!passed_to_server(dropped), "{dropped}");
        }
    }

    #[test]
    fn refuses_retired_secret_file_settings_by_name() {
        let mut env = valid();
        assert!(retired_secret_files(get(&env)).is_empty());
        env.push((
            "PASSWORD_PEPPER_KEYS_FILE",
            "/run/secrets/secret-path".into(),
        ));
        env.push(("POSTGRES_PASSWORD_FILE", String::new()));
        let problems = retired_secret_files(get(&env));
        assert_eq!(
            problems,
            [
                "POSTGRES_PASSWORD_FILE is no longer read; use this release's compose.yml, which passes POSTGRES_PASSWORD from .env",
                "PASSWORD_PEPPER_KEYS_FILE is no longer read; use this release's compose.yml, which passes PASSWORD_PEPPER_KEYS from .env",
            ]
        );
        assert!(problems.iter().all(|p| !p.contains("secret-path")));
        // FVOCI_MEILI_KEY_FILE (the scoped key the preparation writes) is a
        // current setting, not a retired one.
        assert!(retired_secret_files(lookup(&[(
            "FVOCI_MEILI_KEY_FILE",
            "/run/fvoci/meili/api_key"
        )]))
        .is_empty());
    }
}
