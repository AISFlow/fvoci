use sqlx::postgres::{PgConnection, PgPoolOptions};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{Executor, PgPool, Row};
use sqlx::{SqliteConnection, SqlitePool};
use std::path::Path;

pub async fn connect_app(url: &str) -> Result<PgPool, sqlx::Error> {
    connect_app_with_max(url, crate::collab::config::APP_POOL_MAX_CONNECTIONS).await
}

pub const SQLITE_VERSION: &str = "3.53.4";
pub const SQLITE_SOURCE_ID: &str =
    "2026-07-24 19:02:57 bf7c7f30031888f4e796e429ab3978879485813aaca6f641c7b33e4e09459bcc";

pub async fn connect_sqlite_app(
    path: &Path,
    max_connections: u32,
) -> Result<SqlitePool, sqlx::Error> {
    connect_sqlite(path, max_connections, false).await
}

/// Preparation alone may create a database. Normal startup never interprets
/// a misspelled path as a new, empty installation.
pub(crate) async fn connect_sqlite_prepare(path: &Path) -> Result<SqlitePool, sqlx::Error> {
    connect_sqlite(path, 1, true).await
}

async fn connect_sqlite(
    path: &Path,
    max_connections: u32,
    create: bool,
) -> Result<SqlitePool, sqlx::Error> {
    if !path.is_absolute() || path.file_name().is_none() {
        return Err(sqlx::Error::Protocol(
            "SQLite requires an absolute persistent database file".into(),
        ));
    }
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(create)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Full)
        .foreign_keys(true);
    SqlitePoolOptions::new()
        .max_connections(max_connections.max(1))
        .test_before_acquire(false)
        .after_connect(|conn, _| Box::pin(assert_sqlite_runtime(conn)))
        .before_acquire(|conn, _| Box::pin(sqlite_idle_clean(conn)))
        .connect_with(options)
        .await
}

async fn sqlite_idle_clean(conn: &mut SqliteConnection) -> Result<bool, sqlx::Error> {
    let outside = {
        let mut handle = conn.lock_handle().await?;
        // SAFETY: SQLx holds its exclusive native-handle guard, so the worker
        // cannot access SQLite concurrently; the pinned ABI matches SQLx.
        unsafe { libsqlite3_sys::sqlite3_get_autocommit(handle.as_raw_handle().as_ptr()) != 0 }
    };
    if !outside {
        tracing::warn!(event = "db.sqlite.release_in_transaction");
        // Reject/close the idle connection. Closing SQLite rolls back its own
        // uncommitted transaction; no subsequent actor receives that handle.
        return Ok(false);
    }
    assert_sqlite_connection_settings(conn).await?;
    Ok(true)
}

async fn assert_sqlite_runtime(conn: &mut SqliteConnection) -> Result<(), sqlx::Error> {
    let (version, source): (String, String) =
        sqlx::query_as("SELECT sqlite_version(), sqlite_source_id()")
            .fetch_one(&mut *conn)
            .await?;
    if version != SQLITE_VERSION || source != SQLITE_SOURCE_ID {
        return Err(sqlx::Error::Protocol(
            "SQLite runtime does not match the compiled supported engine pin".into(),
        ));
    }
    let flags: Vec<String> = sqlx::query_scalar("PRAGMA compile_options")
        .fetch_all(&mut *conn)
        .await?;
    for required in [
        "THREADSAFE=1",
        "ENABLE_COLUMN_METADATA",
        "ENABLE_UNLOCK_NOTIFY",
    ] {
        if !flags.iter().any(|flag| flag == required) {
            return Err(sqlx::Error::Protocol(format!(
                "SQLite runtime missing required option {required}"
            )));
        }
    }
    for forbidden in ["OMIT_FOREIGN_KEY", "OMIT_TRIGGER", "OMIT_WAL"] {
        if flags.iter().any(|flag| flag == forbidden) {
            return Err(sqlx::Error::Protocol(format!(
                "SQLite runtime has forbidden option {forbidden}"
            )));
        }
    }
    assert_sqlite_connection_settings(conn).await
}

async fn assert_sqlite_connection_settings(conn: &mut SqliteConnection) -> Result<(), sqlx::Error> {
    let fk: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(&mut *conn)
        .await?;
    let sync: i64 = sqlx::query_scalar("PRAGMA synchronous")
        .fetch_one(&mut *conn)
        .await?;
    let journal: String = sqlx::query_scalar("PRAGMA journal_mode")
        .fetch_one(&mut *conn)
        .await?;
    if fk != 1 || sync != 2 || !journal.eq_ignore_ascii_case("wal") {
        return Err(sqlx::Error::Protocol(
            "SQLite app connection requires FK ON, WAL and synchronous FULL".into(),
        ));
    }
    Ok(())
}

/// The app pool. An idle connection that its last user released inside a
/// server-side transaction is closed instead of handed out again (see
/// [`idle_outside_transaction`]).
///
/// Only this pool needs the check: it is the one that request handlers,
/// whose futures a client abort drops, and the long-lived consumers share.
/// The migration, owner and reset tools run one-shot pools no request can
/// cancel.
///
/// The check runs as `before_acquire` on every idle connection and replaces
/// sqlx's acquire-time ping (`test_before_acquire(false)`), so a checkout
/// still costs one round trip there. It is the liveness probe as well: in
/// sqlx-core 0.8.6 `check_idle_conn` an `Err` closes the connection hard and
/// the acquire connects a fresh one, as a failed ping did. `Pool::try_acquire`,
/// `try_begin` and `try_begin_with` pop an idle connection with neither the
/// ping nor `before_acquire`, so they must not be used on this pool.
pub async fn connect_app_with_max(url: &str, max_connections: u32) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(max_connections.max(1))
        .test_before_acquire(false)
        .before_acquire(|conn, _| Box::pin(idle_outside_transaction(conn)))
        .connect(url)
        .await
}

/// True only for a statement sent alone in a simple-protocol Query message
/// outside a transaction block; see [`idle_outside_transaction`]. Functions
/// and operator are schema-qualified so no `search_path` can replace them.
const OUTSIDE_TRANSACTION_SQL: &str =
    "SELECT pg_catalog.now() OPERATOR(pg_catalog.=) pg_catalog.statement_timestamp()";

/// The acquire check's statement text, for tests that leave it out of a
/// statement log (it appears only when an idle connection is reused).
#[cfg(feature = "db-tests")]
pub const ACQUIRE_CHECK_SQL: &str = OUTSIDE_TRANSACTION_SQL;

/// Event for a connection closed because it was idle inside a transaction.
const RELEASE_IN_TRANSACTION: &str = "db.pool.release_in_transaction";

/// `before_acquire` check: hands the idle connection out (`Ok(true)`) only
/// when the server has no transaction open on it.
///
/// Why: sqlx 0.8.6 `PgTransactionManager::begin` queues `BEGIN`, awaits the
/// reply, and only then counts the transaction (`transaction_depth += 1`); its
/// drop guard's `start_rollback` does nothing at depth 0. A begin future
/// dropped after `BEGIN` reached the server (a client abort) therefore returns
/// the connection with the transaction open and no `ROLLBACK` queued (fixed
/// upstream only in 0.9, launchbadge/sqlx#3980). Its next users would run
/// inside it: uncommitted writes, a frozen REPEATABLE READ snapshot pinning
/// xmin, or, once a later `begin_read` fails there, `current transaction is
/// aborted` for everyone on the connection. sqlx's public
/// `Connection::is_in_transaction` reads that depth counter, not the server
/// status, so the check asks the server.
///
/// Guard and removal: `tests/pool_release_integration.rs`
/// `cancelled_begin_does_not_leak_its_transaction` reproduces the upstream
/// bug. The `released_*` tests there pin this check's own contract (no
/// connection is handed out inside a server-side transaction, whatever opened
/// it), so they fail without the hook on any sqlx version. On sqlx >= 0.9,
/// once `cancelled_begin_*` passes without the hook, the hook is defense in
/// depth only; removing it and restoring `test_before_acquire` also drops the
/// `released_*` guarantees, which is a separate decision.
///
/// Ordering: sqlx-core 0.8.6 `Floating::return_to_pool` (pool/connection.rs)
/// always runs `ping` (`write_sync` + `wait_until_ready`) before `release`
/// puts the connection in the idle queue. That flushes the write buffer (the
/// `ROLLBACK` a dropped `Transaction` queued, or a cancelled begin's unsent
/// `BEGIN`) and reads every pending `ReadyForQuery`, also draining a statement
/// whose future was dropped. A failing ping closes the connection instead. The
/// idle queue holds only such settled connections and fresh ones (the reaper
/// re-queues only what was already idle), so at acquire the check sees the
/// server's true state: a normally ended or dropped `Transaction` reads as
/// idle and is kept, and a 25P02 comes from the check itself, not from an
/// earlier statement. A leaked session waits `idle in transaction`, holding
/// whatever it took (a cancelled begin ran only `BEGIN`: no snapshot, xid or
/// lock), in the FIFO idle queue until its next checkout (seconds, with the
/// outbox dispatcher acquiring every second), so the warning appears on that
/// later, unrelated checkout, not on the aborted request.
///
/// The check: a `&str` without arguments goes out as one simple-protocol
/// Query message (`PgConnection::run` takes the extended path only with bound
/// arguments, which `sqlx::query` always has). Outside a block PostgreSQL
/// starts an implicit transaction for that message and copies its start
/// (`now()`) from the message's receipt time (`statement_timestamp()`), so the
/// two are equal. In an open block `now()` is the receipt time of the earlier
/// `BEGIN` message, and at least the release ping's round trip separates the
/// two receipts, so the microsecond timestamps differ. In an aborted block
/// the statement fails with 25P02. The extended protocol would not work: its
/// transaction starts at Parse or Bind and Execute moves
/// `statement_timestamp()`, so the comparison is false outside a block as well.
///
/// Slow-statement logs: sqlx times each statement from before its
/// `wait_until_ready` and warns past one second with the statement text. The
/// release ping drains abandoned statements outside that timer, so such a
/// warning naming this check measures only the check's own round trip.
///
/// Closing, not rolling back: its owner never committed whatever the open
/// transaction holds, and a new connection is cheap next to a rare abort.
/// `Ok(false)` makes sqlx close gracefully (Terminate; the server rolls back)
/// without a log line of its own, so this logs the one warning. Any other
/// error (I/O, protocol, a server that ended the session) is logged here with
/// its kind and SQLSTATE only, never the message, and returned, so sqlx
/// closes the connection hard and connects a fresh one. sqlx's own warning for
/// it (target `sqlx_core::pool::inner`, with the message) is below the default
/// log filter. A server restart thus logs this warning once per idle
/// connection, where the skipped ping logged at info.
async fn idle_outside_transaction(conn: &mut PgConnection) -> Result<bool, sqlx::Error> {
    let checked = conn
        .fetch_one(OUTSIDE_TRANSACTION_SQL)
        .await
        .and_then(|row| row.try_get::<bool, _>(0));
    match checked {
        Ok(true) => Ok(true),
        Ok(false) => {
            tracing::warn!(event = RELEASE_IN_TRANSACTION, state = "open");
            Ok(false)
        }
        Err(sqlx::Error::Database(error)) if error.code().as_deref() == Some("25P02") => {
            tracing::warn!(event = RELEASE_IN_TRANSACTION, state = "aborted");
            Ok(false)
        }
        Err(error) => {
            let sqlstate = error.as_database_error().and_then(|db| db.code());
            tracing::warn!(
                event = "db.pool.acquire_check_failed",
                kind = error_kind(&error),
                sqlstate = sqlstate.as_deref().unwrap_or(""),
            );
            Err(error)
        }
    }
}

/// The error's variant, never its message (a server message can quote data).
fn error_kind(error: &sqlx::Error) -> &'static str {
    match error {
        sqlx::Error::Database(_) => "database",
        sqlx::Error::Io(_) => "io",
        sqlx::Error::Tls(_) => "tls",
        sqlx::Error::Protocol(_) => "protocol",
        sqlx::Error::ColumnDecode { .. } | sqlx::Error::Decode(_) => "decode",
        _ => "other",
    }
}
