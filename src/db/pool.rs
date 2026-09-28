use sqlx::postgres::{PgConnection, PgPoolOptions};
use sqlx::{Executor, PgPool, Row};

pub async fn connect_app(url: &str) -> Result<PgPool, sqlx::Error> {
    connect_app_with_max(url, crate::collab::config::APP_POOL_MAX_CONNECTIONS).await
}

/// The app pool. A connection released inside a server-side transaction is
/// closed instead of reused (see [`release_outside_transaction`]).
///
/// Only this pool needs the check: it is the one that request handlers,
/// whose futures a client abort drops, and the long-lived consumers share.
/// The migration, owner and reset tools run one-shot pools no request can
/// cancel. No `before_acquire` twin is needed either: a connection reaches
/// this pool's idle queue only fresh from `connect` or through the release
/// path below (the reaper re-queues only what was already idle), and a pool
/// lives in one process, so no connection predates the check.
pub async fn connect_app_with_max(url: &str, max_connections: u32) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(max_connections.max(1))
        .after_release(|conn, _| Box::pin(release_outside_transaction(conn)))
        .connect(url)
        .await
}

/// True only for a statement sent alone in a simple-protocol Query message
/// outside a transaction block; see [`release_outside_transaction`].
const OUTSIDE_TRANSACTION_SQL: &str = "SELECT pg_catalog.now() = pg_catalog.statement_timestamp()";

/// `after_release` check: keeps the connection (`Ok(true)`) only when the
/// server has no transaction open on it.
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
/// runs this hook before its own ping, and the check statement goes through
/// `PgConnection::run`, which first calls `wait_until_ready`: it flushes the
/// write buffer (the `ROLLBACK` a dropped `Transaction` queued, or a cancelled
/// begin's unsent `BEGIN`) and reads every pending `ReadyForQuery`. The check
/// thus sees the state after those, so a normally dropped `Transaction` reads
/// as idle and is kept.
///
/// The check: a `&str` without arguments goes out as one simple-protocol
/// Query message (`PgConnection::run` takes the extended path only with bound
/// arguments, which `sqlx::query` always has). Outside a block PostgreSQL
/// starts an implicit transaction for that message and copies its start
/// (`now()`) from the message's receipt time (`statement_timestamp()`), so the
/// two are equal. In an open block `now()` is the receipt time of the earlier
/// `BEGIN` message, and `wait_until_ready` read that message's reply before
/// writing this one: a full client round trip separates the two receipts, so
/// the microsecond timestamps differ. In an aborted block the statement fails
/// with 25P02. The extended protocol would not work: its transaction starts at
/// Parse or Bind and Execute moves `statement_timestamp()`, so the comparison
/// is false outside a block as well.
///
/// Closing, not rolling back: its owner never committed whatever the open
/// transaction holds, and a new connection is cheap next to a rare abort.
/// `Ok(false)` makes sqlx close gracefully (Terminate; the server rolls back)
/// without a log line of its own, so this logs the one warning. Other errors
/// (I/O, protocol) are returned: sqlx then closes hard and logs them once.
async fn release_outside_transaction(conn: &mut PgConnection) -> Result<bool, sqlx::Error> {
    match conn.fetch_one(OUTSIDE_TRANSACTION_SQL).await {
        Ok(row) => {
            let outside: bool = row.try_get(0)?;
            if !outside {
                tracing::warn!(event = "db.pool.release_in_transaction", state = "open");
            }
            Ok(outside)
        }
        Err(sqlx::Error::Database(error)) if error.code().as_deref() == Some("25P02") => {
            tracing::warn!(event = "db.pool.release_in_transaction", state = "aborted");
            Ok(false)
        }
        Err(error) => Err(error),
    }
}
