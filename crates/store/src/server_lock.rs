//! The lock that makes one process the server of a database: what runs in the
//! background and what the start cleans up run in that process only.
//!
//! On PostgreSQL it is an advisory lock of a session, on a connection of its own outside
//! the pool, keyed by the schema the tables are in: it lasts as long as that connection
//! does, and nothing the transactions of the pool set touches it. It needs a session of
//! its own on the database server: a pooler that hands a server's session to several
//! clients (the transaction mode of `PgBouncer`) shares the lock out. On SQLite it is a lock
//! of a file beside the database's (`<database>-server.lock`), held while the file is
//! open. An in-memory database is no other process's, and has none.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};

use sqlx::Connection as _;
use sqlx::postgres::PgConnection;

use crate::db::Dialect;
use crate::error::StoreError;
use crate::{Pool, Store};

/// The first key of the advisory lock, the same for every server: `fvoc`. The second
/// is the hash of the schema the tables are in ([`SCHEMA_KEY`]), so that servers of
/// other schemas of one database do not hold each other back.
const LOCK_CLASS: i32 = i32::from_be_bytes(*b"fvoc");

/// The second key of the lock, as the database works it out.
const SCHEMA_KEY: &str = "hashtext(COALESCE(current_schema(), ''))";

/// The condition on `pg_locks` (as `l`) that is a lock of a server of this database
/// whose second key is `key`, an expression.
fn the_lock(key: &str) -> String {
    format!(
        "l.locktype = 'advisory' AND l.granted \
         AND l.database = (SELECT oid FROM pg_database WHERE datname = current_database()) \
         AND l.classid = {LOCK_CLASS}::oid AND l.objid = ({key})::int4::oid AND l.objsubid = 2"
    )
}

/// How the database server keeps the connection that holds the lock: it asks the
/// client every few seconds once the connection is quiet, so that the lock of a server
/// whose host went away is let go in about half a minute, not in the hours of the
/// system's defaults. Set once connected, not in the startup packet, which some
/// poolers refuse to carry settings in.
const KEEPALIVES: &str = "SET tcp_keepalives_idle = 10; SET tcp_keepalives_interval = 5; \
     SET tcp_keepalives_count = 3";

/// The lock of the server of a database, held until it is released or dropped (see the
/// module).
#[derive(Debug)]
pub struct ServerLock {
    held: Held,
}

#[derive(Debug)]
enum Held {
    /// The connection whose session holds the advisory lock, and its second key.
    Postgres {
        conn: tokio::sync::Mutex<PgConnection>,
        key: i32,
    },
    /// The open lock file.
    Sqlite(File),
    /// An in-memory database: nothing to hold.
    Memory,
}

/// The lock file of the SQLite database `database`: beside the file the name leads to,
/// symbolic links followed, so that every such name of one database has one lock file.
/// A hard link is a name of its own, and has a lock file of its own.
pub(crate) fn file_beside(database: &Path) -> PathBuf {
    let database = std::fs::canonicalize(database).unwrap_or_else(|error| {
        tracing::warn!(%error, path = %database.display(), "the file of the database could not be followed; its lock file goes beside the name given");
        database.to_path_buf()
    });
    let mut name = database.into_os_string();
    name.push("-server.lock");
    PathBuf::from(name)
}

fn io(error: std::io::Error) -> StoreError {
    StoreError::Database(sqlx::Error::Io(error))
}

impl Store {
    /// Takes the lock of the server of the database if no other process holds it, and
    /// leaves `holder` with it: a few words that whoever is refused the lock is told
    /// ([`Store::server_lock_holder`]). Returns `None` if another holds it.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the database cannot be reached or the lock
    /// file cannot be opened.
    pub async fn try_lock_server(&self, holder: &str) -> Result<Option<ServerLock>, StoreError> {
        let held = match &self.inner.pool {
            Pool::Postgres(pool) => {
                let options = (*pool.connect_options()).clone().application_name(holder);
                let mut conn = PgConnection::connect_with(&options).await?;
                sqlx::raw_sql(KEEPALIVES).execute(&mut conn).await?;
                // The key of the schema now, which stays the lock's.
                let key: i32 =
                    sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT {SCHEMA_KEY}")))
                        .fetch_one(&mut conn)
                        .await?;
                let taken: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1, $2)")
                    .bind(LOCK_CLASS)
                    .bind(key)
                    .fetch_one(&mut conn)
                    .await?;
                if !taken {
                    conn.close().await?;
                    return Ok(None);
                }
                Held::Postgres {
                    conn: tokio::sync::Mutex::new(conn),
                    key,
                }
            }
            Pool::Sqlite(_) => match &self.inner.lock_file {
                None => Held::Memory,
                Some(path) => match lock_file(path, holder).map_err(io)? {
                    Some(file) => Held::Sqlite(file),
                    None => return Ok(None),
                },
            },
        };
        Ok(Some(ServerLock { held }))
    }

    /// Who holds the lock of the server of the database, in the words it was taken
    /// with, if anyone does. On PostgreSQL the backend of its session is named too,
    /// which `pg_terminate_backend` takes.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails or the lock file cannot be
    /// read.
    pub async fn server_lock_holder(&self) -> Result<Option<String>, StoreError> {
        if self.dialect() == Dialect::Postgres {
            return self
                .with_conn(async |scope| {
                    // Of a session of another role, only the backend may be shown.
                    let mut sql = scope.sql(
                        "SELECT concat_ws(', ', NULLIF(a.application_name, ''), \
                         'backend ' || l.pid, 'connected since ' || a.backend_start::text) \
                         FROM pg_locks l LEFT JOIN pg_stat_activity a ON a.pid = l.pid \
                         WHERE l.locktype = 'advisory' AND l.granted \
                         AND l.database = (SELECT oid FROM pg_database WHERE datname = current_database()) \
                         AND l.classid = (",
                    );
                    sql.bind(i64::from(LOCK_CLASS)).push([
                        ")::oid AND l.objid = (",
                        SCHEMA_KEY,
                        ")::int4::oid AND l.objsubid = 2",
                    ]);
                    match scope.fetch_optional(sql).await? {
                        Some(row) => row.text(0).map(Some),
                        None => Ok(None),
                    }
                })
                .await;
        }
        let Some(path) = &self.inner.lock_file else {
            return Ok(None);
        };
        let mut text = String::new();
        match File::open(path) {
            Ok(mut file) => file.read_to_string(&mut text).map_err(io)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(io(error)),
        };
        Ok(Some(format!("{text} (the lock file {})", path.display())))
    }
}

/// Opens and locks the lock file at `path`, and writes `holder` into it; `None` if
/// another holds it.
fn lock_file(path: &Path, holder: &str) -> std::io::Result<Option<File>> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    match file.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => return Ok(None),
        Err(TryLockError::Error(error)) => return Err(error),
    }
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(holder.as_bytes())?;
    file.sync_data()?;
    Ok(Some(file))
}

impl ServerLock {
    /// Whether the lock is still held: on PostgreSQL, the connection answers and its
    /// session holds the lock. A lock file is held as long as it is open.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the connection does not answer, after which
    /// the lock is to be taken for lost.
    pub async fn is_held(&self) -> Result<bool, StoreError> {
        match &self.held {
            Held::Postgres { conn, key } => {
                let mut conn = conn.lock().await;
                let held: bool = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                    "SELECT EXISTS (SELECT 1 FROM pg_locks l \
                     WHERE l.pid = pg_backend_pid() AND {})",
                    the_lock("$1")
                )))
                .bind(*key)
                .fetch_one(&mut *conn)
                .await?;
                Ok(held)
            }
            Held::Sqlite(_) | Held::Memory => Ok(true),
        }
    }

    /// Lets the lock go, for the next server.
    pub async fn release(self) {
        match self.held {
            // The session ends with its connection, and its lock with it.
            Held::Postgres { conn, .. } => {
                if let Err(error) = conn.into_inner().close().await {
                    tracing::warn!(%error, "the connection of the server's lock did not close cleanly");
                }
            }
            Held::Sqlite(file) => {
                if let Err(error) = file.unlock() {
                    tracing::warn!(%error, "the lock file of the server could not be unlocked");
                }
            }
            Held::Memory => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lock_class_is_fvoc() {
        assert_eq!(LOCK_CLASS, 1_719_037_795);
        assert!(the_lock("$1").contains("1719037795::oid"));
    }
}
