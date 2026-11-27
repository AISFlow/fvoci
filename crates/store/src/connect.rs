//! Turning the server's `database_kind` and `database_connection` settings into
//! connection options.
//!
//! `database_connection` is the path of an SQLite file (or `:memory:`), with
//! `?busy_timeout_ms=...` if need be, or a `postgres://` URL. Parameters this server does
//! not understand are an error rather than silently dropped, so a setting that has no
//! effect is noticed at startup.

use std::str::FromStr;
use std::time::Duration;

use sqlx::postgres::{PgConnectOptions, PgSslMode};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous};

use crate::error::StoreError;

/// The kind of database a store connects to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DbType {
    /// SQLite, the `sqlite` setting.
    Sqlite,
    /// PostgreSQL, the `postgres` setting.
    Postgres,
}

impl DbType {
    /// Returns the name used in the `database_kind` setting.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sqlite => "sqlite",
            Self::Postgres => "postgres",
        }
    }
}

impl FromStr for DbType {
    type Err = StoreError;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        match name {
            "sqlite" => Ok(Self::Sqlite),
            "postgres" => Ok(Self::Postgres),
            other => Err(StoreError::Config(format!(
                "unknown database_kind {other:?}; use \"sqlite\" or \"postgres\""
            ))),
        }
    }
}

/// Connection settings for SQLite derived from a `database_connection`.
#[derive(Debug, Clone)]
pub(crate) struct SqliteSettings {
    pub(crate) options: SqliteConnectOptions,
    /// The database lives in memory and disappears with its last connection.
    pub(crate) in_memory: bool,
    /// How long a statement waits for another writer before it fails.
    pub(crate) busy_timeout: Duration,
}

/// The parameter of an SQLite `database_connection` that sets how long a statement
/// waits for another writer, in milliseconds.
const BUSY_TIMEOUT: &str = "busy_timeout_ms";

/// Parses an SQLite `database_connection`: `<path>[?busy_timeout_ms=<milliseconds>]`.
/// The path `:memory:` is a database in memory, of this store alone.
pub(crate) fn sqlite_settings(connection: &str) -> Result<SqliteSettings, StoreError> {
    let (path, query) = connection.split_once('?').unwrap_or((connection, ""));
    if path.is_empty() {
        return Err(StoreError::Config(
            "database_connection names no database file".to_owned(),
        ));
    }

    let in_memory = path == ":memory:";
    let mut busy_timeout = Duration::from_secs(5);
    for (key, value) in query_pairs(query) {
        if key != BUSY_TIMEOUT {
            return Err(unsupported_parameter(key));
        }
        let millis: u64 = value.parse().map_err(|_| not_a_number(key, value))?;
        busy_timeout = Duration::from_millis(millis);
    }

    let mut options = if in_memory {
        // A fresh, uniquely named in-memory database with a shared cache, so that every
        // connection of this store sees the same data.
        SqliteConnectOptions::from_str("sqlite::memory:")?
    } else {
        // A file is kept with a write-ahead log. With the rollback journal a commit
        // holds the whole file, and waits for every read under way before it does: a
        // read begun meanwhile waits in turn, up to the busy timeout, and then fails.
        // With the log who reads and who writes do not wait for each other; writers
        // still take turns. The mode is the file's from then on, whoever opens it, and
        // a file kept the other way is changed when it is first opened here. The log
        // (`-wal`) and its index (`-shm`) are beside the file while it is open.
        //
        // Every commit is on the disk before it is answered (`FULL`), as on PostgreSQL:
        // `NORMAL` would lose the last commits to a machine that goes down, after the
        // client was told they were kept and files, mails and the search were sent on
        // their word. It costs one sync of the log a commit, less than the journal took.
        SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Full)
    };
    // SQLite enforces foreign keys only where a connection asks for it. The schema
    // relies on them: said here, and not left to the driver's default.
    options = options.foreign_keys(true).busy_timeout(busy_timeout);
    Ok(SqliteSettings {
        options,
        in_memory,
        busy_timeout,
    })
}

/// Connection settings for PostgreSQL derived from a `database_connection`.
#[derive(Debug, Clone)]
pub(crate) struct PostgresSettings {
    pub(crate) options: PgConnectOptions,
    /// How long to wait for a connection, from `connect_timeout`.
    pub(crate) connect_timeout: Option<Duration>,
}

/// Parses a PostgreSQL `database_connection`: a `postgres://` (or `postgresql://`) URL.
///
/// Recognized parameters are `sslmode`, `sslrootcert`, `sslcert`, `sslkey`,
/// `application_name`, `search_path` and `connect_timeout` (seconds).
pub(crate) fn postgres_settings(connection: &str) -> Result<PostgresSettings, StoreError> {
    if !(connection.starts_with("postgres://") || connection.starts_with("postgresql://")) {
        return Err(StoreError::Config(
            "database_connection is no postgres:// URL".to_owned(),
        ));
    }
    let (base, query) = connection.split_once('?').unwrap_or((connection, ""));
    let mut options = PgConnectOptions::from_str(base).map_err(|error| {
        StoreError::Config(format!("database_connection is not a valid URL: {error}"))
    })?;

    let mut connect_timeout = None;
    for (key, value) in query_pairs(query) {
        options = match key {
            "sslmode" => options
                .ssl_mode(PgSslMode::from_str(value).map_err(|_| unsupported_value(key, value))?),
            "sslrootcert" => options.ssl_root_cert(value),
            "sslcert" => options.ssl_client_cert(value),
            "sslkey" => options.ssl_client_key(value),
            "application_name" => options.application_name(value),
            "search_path" => options.options([("search_path", value)]),
            "connect_timeout" => {
                let seconds: u64 = value.parse().map_err(|_| not_a_number(key, value))?;
                connect_timeout = Some(Duration::from_secs(seconds));
                options
            }
            _ => return Err(unsupported_parameter(key)),
        };
    }
    Ok(PostgresSettings {
        options,
        connect_timeout,
    })
}

/// Splits a URL query string into its `key=value` pairs. A key without `=` has an empty
/// value.
fn query_pairs(query: &str) -> impl Iterator<Item = (&str, &str)> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| pair.split_once('=').unwrap_or((pair, "")))
}

fn unsupported_parameter(key: &str) -> StoreError {
    StoreError::Config(format!(
        "database_connection parameter {key:?} is not supported"
    ))
}

fn unsupported_value(key: &str, value: &str) -> StoreError {
    StoreError::Config(format!(
        "database_connection parameter {key}={value:?} is not supported"
    ))
}

fn not_a_number(key: &str, value: &str) -> StoreError {
    StoreError::Config(format!(
        "database_connection parameter {key}={value:?} is not a number"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_error<T: std::fmt::Debug>(result: Result<T, StoreError>) -> String {
        match result {
            Err(StoreError::Config(message)) => message,
            other => panic!("expected a configuration error, got {other:?}"),
        }
    }

    #[test]
    fn db_type_names() {
        assert_eq!("sqlite".parse::<DbType>().expect("known"), DbType::Sqlite);
        assert_eq!(
            "postgres".parse::<DbType>().expect("known"),
            DbType::Postgres
        );
        assert_eq!(DbType::Sqlite.as_str(), "sqlite");
        assert_eq!(DbType::Postgres.as_str(), "postgres");
        for unknown in ["mysql", "sqlite3", "Postgres"] {
            assert!(
                config_error(unknown.parse::<DbType>()).contains("unknown database_kind"),
                "{unknown}"
            );
        }
    }

    #[test]
    fn sqlite_connections() {
        let plain = sqlite_settings("./data/fvoci.db").expect("a plain path");
        assert!(!plain.in_memory, "a path is a file database");
        assert_eq!(
            plain.options.get_filename().to_str(),
            Some("./data/fvoci.db")
        );
        assert_eq!(plain.busy_timeout, Duration::from_secs(5), "the default");

        let with_timeout = sqlite_settings("./fvoci.db?busy_timeout_ms=1500").expect("a timeout");
        assert_eq!(
            with_timeout.options.get_filename().to_str(),
            Some("./fvoci.db")
        );
        assert_eq!(with_timeout.busy_timeout, Duration::from_millis(1500));

        let memory = sqlite_settings(":memory:?busy_timeout_ms=5000").expect("in memory");
        assert!(memory.in_memory, ":memory: is an in-memory database");
    }

    #[test]
    fn sqlite_connections_refuse_what_they_do_not_understand() {
        assert!(config_error(sqlite_settings("")).contains("no database file"));
        assert!(config_error(sqlite_settings("a.db?_loc=UTC")).contains("\"_loc\""));
        assert!(
            config_error(sqlite_settings("a.db?busy_timeout_ms=soon")).contains("not a number")
        );
        for other in [
            "_busy_timeout=5000",
            "mode=memory",
            "cache=shared",
            "_journal=WAL",
        ] {
            let (key, _) = other.split_once('=').expect("a pair");
            assert!(
                config_error(sqlite_settings(&format!("a.db?{other}")))
                    .contains(&format!("{key:?} is not supported")),
                "{other}"
            );
        }
    }

    #[test]
    fn postgres_connections() {
        let url = postgres_settings(
            "postgres://fvoci:secret@db.example/fvoci?sslmode=disable&connect_timeout=10",
        )
        .expect("the documented URL form");
        assert_eq!(url.options.get_host(), "db.example");
        assert_eq!(url.options.get_username(), "fvoci");
        assert_eq!(url.options.get_database(), Some("fvoci"));
        assert_eq!(url.connect_timeout, Some(Duration::from_secs(10)));

        postgres_settings("postgresql://u@h/db?search_path=s&application_name=a")
            .expect("search_path and application_name are supported");
    }

    #[test]
    fn postgres_connections_refuse_what_they_do_not_understand() {
        assert!(
            config_error(postgres_settings("postgres://u@h/db?fallback=1"))
                .contains("\"fallback\"")
        );
        assert!(
            config_error(postgres_settings("postgres://u@h/db?sslmode=maybe")).contains("sslmode")
        );
        assert!(
            config_error(postgres_settings("dbname=fvoci sslmode=disable"))
                .contains("no postgres:// URL")
        );
        assert!(
            config_error(postgres_settings("postgres://u@h/db?binary_parameters=yes"))
                .contains("\"binary_parameters\"")
        );
        assert!(
            config_error(postgres_settings("postgres://u@h/db?host=other")).contains("\"host\"")
        );
        assert!(
            config_error(postgres_settings("postgres://u@h/db?connect_timeout=x"))
                .contains("not a number")
        );
    }
}
