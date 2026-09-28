use sqlx::postgres::{PgConnection, PgPoolOptions};
use sqlx::{Executor, PgPool, Row};

pub async fn connect_app(url: &str) -> Result<PgPool, sqlx::Error> {
    connect_app_with_max(url, crate::collab::config::APP_POOL_MAX_CONNECTIONS).await
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
/// it (target `sqlx_core::pool::inner`, with the message) does not reach the
/// default log: `main.rs` adds only `fvoci_server=info` to `RUST_LOG`, which
/// compose leaves empty, so other targets log at ERROR only. A server restart
/// thus logs this warning once per idle connection, where the skipped ping
/// logged at info.
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
