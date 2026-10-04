//! Closed driver boundary. PostgreSQL-only callers are rejected explicitly
//! during the staged port; no synthetic PostgreSQL pool exists for SQLite.
use super::codec::{remote_error, Cell, FamilyRow};
use sqlx::{PgPool, Postgres, Sqlite, SqlitePool, Transaction};
use std::sync::{Arc, Mutex};
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinSet;

#[derive(Clone)]
pub enum Backend {
    Postgres(PgPool),
    Sqlite(SqlitePool),
    LibsqlRemote(Arc<RemoteDatabase>),
}

pub struct ConnectionStats {
    pub size: u32,
    pub idle: usize,
    pub max: u32,
}

impl Backend {
    pub fn connection_stats(&self) -> Result<ConnectionStats, sqlx::Error> {
        match self {
            Self::Postgres(pool) => Ok(ConnectionStats {
                size: pool.size(),
                idle: pool.num_idle(),
                max: pool.options().get_max_connections(),
            }),
            Self::Sqlite(pool) => Ok(ConnectionStats {
                size: pool.size(),
                idle: pool.num_idle(),
                max: pool.options().get_max_connections(),
            }),
            Self::LibsqlRemote(remote) => {
                let state = remote
                    .lifecycle
                    .lock()
                    .map_err(|_| sqlx::Error::Protocol("remote lifecycle lock poisoned".into()))?;
                let active = u32::try_from(state.active).map_err(|_| {
                    sqlx::Error::Protocol("remote active stream count overflow".into())
                })?;
                // Streams are exclusively owned until explicit finish/cleanup;
                // there is no idle remote connection cache masquerading as PG.
                Ok(ConnectionStats {
                    size: active,
                    idle: 0,
                    max: remote.max_connections,
                })
            }
        }
    }

    pub async fn ping(&self) -> Result<(), sqlx::Error> {
        if let Self::Postgres(pool) = self {
            return sqlx::query("SELECT 1").execute(pool).await.map(|_| ());
        }
        let mut tx = self.begin_read().await?;
        let result = match &mut tx {
            DbTransaction::Postgres(pg) => {
                sqlx::query("SELECT 1").execute(&mut **pg).await.map(|_| ())
            }
            DbTransaction::SqliteFamily(family) => family.query("SELECT 1", &[]).await.map(|_| ()),
        };
        tx.rollback().await?;
        result
    }
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Postgres(_) => "postgres",
            Self::Sqlite(_) => "sqlite",
            Self::LibsqlRemote(_) => "libsql-remote",
        }
    }
    pub fn postgres(&self, operation: &'static str) -> Result<&PgPool, sqlx::Error> {
        match self {
            Self::Postgres(pool) => Ok(pool),
            _ => Err(sqlx::Error::Protocol(format!(
                "operation {operation} is pending backend port for {}",
                self.kind()
            ))),
        }
    }
    pub async fn begin_write(&self) -> Result<DbTx, sqlx::Error> {
        match self {
            Self::Postgres(pool) => pool.begin().await.map(DbTx::Postgres),
            Self::Sqlite(pool) => pool.begin_with("BEGIN IMMEDIATE").await.map(|tx| {
                DbTx::SqliteFamily(FamilyTx::Local(LocalTx {
                    tx,
                    write_reserved: true,
                    tenant: None,
                    system_context: false,
                }))
            }),
            Self::LibsqlRemote(database) => database
                .begin(true)
                .await
                .map(|tx| DbTx::SqliteFamily(FamilyTx::Remote(tx))),
        }
    }
    pub async fn begin_read(&self) -> Result<DbTx, sqlx::Error> {
        match self {
            Self::Postgres(pool) => super::context::begin_read(pool).await.map(DbTx::Postgres),
            Self::Sqlite(pool) => pool.begin().await.map(|tx| {
                DbTx::SqliteFamily(FamilyTx::Local(LocalTx {
                    tx,
                    write_reserved: false,
                    tenant: None,
                    system_context: false,
                }))
            }),
            Self::LibsqlRemote(database) => database
                .begin(false)
                .await
                .map(|tx| DbTx::SqliteFamily(FamilyTx::Remote(tx))),
        }
    }
    /// Serialize current source reads and the ordered external search enqueue.
    /// The caller borrows this transaction for every named source operation,
    /// then releases it after enqueue and before waiting for confirmation.
    /// SQLite reserves its writer across this bounded step, including remote
    /// stream ownership; it does not claim PostgreSQL per-workspace concurrency.
    pub async fn begin_search_refresh(&self, workspace: uuid::Uuid) -> Result<DbTx, sqlx::Error> {
        let mut tx = self.begin_write().await?;
        if let DbTransaction::Postgres(pg) = &mut tx {
            sqlx::query("SELECT pg_advisory_xact_lock($1, $2)")
                .bind(super::context::SEARCH_INDEX_LOCK_NAMESPACE)
                .bind(super::context::lock_key_from_uuid(workspace))
                .execute(&mut **pg)
                .await?;
        }
        tx.operation().set_tenant(workspace).await?;
        Ok(tx)
    }
    pub async fn close(&self) -> Result<(), sqlx::Error> {
        match self {
            Self::Postgres(pool) => {
                pool.close().await;
                Ok(())
            }
            Self::Sqlite(pool) => {
                pool.close().await;
                Ok(())
            }
            Self::LibsqlRemote(remote) => remote.drain_cleanup().await,
        }
    }
}

pub type DbTx = DbTransaction<'static>;

pub enum DbTransaction<'connection> {
    Postgres(Transaction<'connection, Postgres>),
    SqliteFamily(FamilyTx),
}

/// Borrow the active transaction for a reusable product operation without
/// transferring commit ownership (personal-input/import/ordinary creation).
pub(crate) enum OperationTx<'operation, 'connection> {
    Postgres(&'operation mut Transaction<'connection, Postgres>),
    SqliteFamily(&'operation mut FamilyTx),
}

impl<'connection> DbTransaction<'connection> {
    pub(crate) fn operation(&mut self) -> OperationTx<'_, 'connection> {
        match self {
            Self::Postgres(tx) => OperationTx::Postgres(tx),
            Self::SqliteFamily(tx) => OperationTx::SqliteFamily(tx),
        }
    }
}

/// A failed COMMIT reply does not authorize a second write. Callers reconcile
/// using a fresh transaction, current authority and the original command hash.
#[derive(Debug, thiserror::Error)]
#[error("database commit outcome is unknown")]
pub struct CommitUnknown {
    #[source]
    pub source: sqlx::Error,
}

/// The original failed COMMIT and its own remote stream cleanup receipt.
/// Existing callers retain the original commit API; claim reconciliation uses
/// this boundary before opening a fresh writer.
#[derive(Debug, thiserror::Error)]
#[error("database commit outcome is unknown after owned cleanup")]
pub struct CommitCleanupUnknown {
    #[source]
    pub source: CommitUnknown,
    pub cleanup_error: Option<sqlx::Error>,
}
impl DbTransaction<'_> {
    pub async fn commit(self) -> Result<(), CommitUnknown> {
        let result = match self {
            Self::Postgres(tx) => tx.commit().await,
            Self::SqliteFamily(tx) => tx.commit().await,
        };
        result.map_err(|source| CommitUnknown { source })
    }
    pub async fn commit_with_cleanup(self) -> Result<(), CommitCleanupUnknown> {
        match self {
            Self::SqliteFamily(FamilyTx::Remote(tx)) => tx.commit_with_cleanup().await,
            other => other.commit().await.map_err(|source| CommitCleanupUnknown {
                source,
                cleanup_error: None,
            }),
        }
    }
    pub async fn rollback(self) -> Result<(), sqlx::Error> {
        match self {
            Self::Postgres(tx) => tx.rollback().await,
            Self::SqliteFamily(tx) => tx.rollback().await,
        }
    }
}

pub struct LocalTx {
    tx: Transaction<'static, Sqlite>,
    write_reserved: bool,
    tenant: Option<uuid::Uuid>,
    system_context: bool,
}

pub enum FamilyTx {
    Local(LocalTx),
    Remote(RemoteTx),
}
impl FamilyTx {
    pub(crate) fn replace_system_context(&mut self, enabled: bool) -> bool {
        let value = match self {
            Self::Local(tx) => &mut tx.system_context,
            Self::Remote(tx) => &mut tx.system_context,
        };
        std::mem::replace(value, enabled)
    }
    pub(crate) fn require_system_context(&self) -> Result<(), sqlx::Error> {
        let enabled = match self {
            Self::Local(tx) => tx.system_context,
            Self::Remote(tx) => tx.system_context,
        };
        if enabled {
            Ok(())
        } else {
            Err(sqlx::Error::Protocol(
                "global operation needs system transaction context".into(),
            ))
        }
    }
    pub(crate) fn tenant(&self) -> Option<uuid::Uuid> {
        match self {
            Self::Local(tx) => tx.tenant,
            Self::Remote(tx) => tx.tenant,
        }
    }
    pub(crate) fn require_writer(&self) -> Result<(), sqlx::Error> {
        let reserved = match self {
            Self::Local(tx) => tx.write_reserved,
            Self::Remote(tx) => tx.write_reserved,
        };
        if reserved {
            Ok(())
        } else {
            Err(sqlx::Error::Protocol(
                "write operation needs SQLite writer reservation".into(),
            ))
        }
    }
    pub(crate) fn set_tenant(&mut self, workspace: uuid::Uuid) -> Result<(), sqlx::Error> {
        let tenant = match self {
            Self::Local(tx) => &mut tx.tenant,
            Self::Remote(tx) => &mut tx.tenant,
        };
        if tenant.is_some_and(|id| id != workspace) {
            return Err(sqlx::Error::Protocol(
                "transaction tenant cannot change".into(),
            ));
        }
        *tenant = Some(workspace);
        Ok(())
    }
    pub(crate) fn require_tenant(&self, workspace: uuid::Uuid) -> Result<(), sqlx::Error> {
        let tenant = match self {
            Self::Local(tx) => tx.tenant,
            Self::Remote(tx) => tx.tenant,
        };
        if tenant == Some(workspace) {
            Ok(())
        } else {
            Err(sqlx::Error::Protocol(
                "named operation has wrong transaction tenant".into(),
            ))
        }
    }

    // Only named DB operation modules use these static, reviewed statements.
    // Both SQLite drivers receive identical SQL and exact bind values.
    pub(crate) async fn execute(
        &mut self,
        statement: &'static str,
        args: &[Cell],
    ) -> Result<u64, sqlx::Error> {
        match self {
            Self::Local(tx) => local_query(statement, args)
                .execute(&mut *tx.tx)
                .await
                .map(|r| r.rows_affected()),
            Self::Remote(tx) => tx
                .connection()
                .execute(statement, remote_params(args))
                .await
                .map_err(remote_error),
        }
    }
    /// Only the migration registry supplies compiled, fixed DDL. This keeps
    /// DDL and its applied marker on the caller's already reserved stream.
    pub(crate) async fn apply_migration_batch(
        &mut self,
        sql: &'static str,
    ) -> Result<(), sqlx::Error> {
        self.require_writer()?;
        match self {
            Self::Local(tx) => sqlx::raw_sql(sql).execute(&mut *tx.tx).await.map(|_| ()),
            Self::Remote(tx) => tx
                .connection()
                .execute_batch(sql)
                .await
                .map(|_| ())
                .map_err(remote_error),
        }
    }
    pub(crate) async fn query(
        &mut self,
        statement: &'static str,
        args: &[Cell],
    ) -> Result<Vec<FamilyRow>, sqlx::Error> {
        match self {
            Self::Local(tx) => local_query(statement, args)
                .fetch_all(&mut *tx.tx)
                .await
                .map(|rows| rows.into_iter().map(FamilyRow::Local).collect()),
            Self::Remote(tx) => {
                let mut rows = tx
                    .connection()
                    .query(statement, remote_params(args))
                    .await
                    .map_err(remote_error)?;
                let mut result = Vec::new();
                while let Some(row) = rows.next().await.map_err(remote_error)? {
                    result.push(FamilyRow::Remote(row));
                }
                Ok(result)
            }
        }
    }
    async fn commit(self) -> Result<(), sqlx::Error> {
        match self {
            Self::Local(tx) => tx.tx.commit().await,
            Self::Remote(tx) => tx.finish("COMMIT").await,
        }
    }
    async fn rollback(self) -> Result<(), sqlx::Error> {
        match self {
            Self::Local(tx) => tx.tx.rollback().await,
            Self::Remote(tx) => tx.finish("ROLLBACK").await,
        }
    }
}

fn local_query<'a>(
    sql: &'a str,
    args: &[Cell],
) -> sqlx::query::Query<'a, Sqlite, sqlx::sqlite::SqliteArguments<'a>> {
    let mut query = sqlx::query(sql);
    for value in args {
        query = match value {
            Cell::Null => query.bind(Option::<i64>::None),
            Cell::Integer(v) => query.bind(*v),
            Cell::Text(v) => query.bind(v.clone()),
            Cell::Blob(v) => query.bind(v.clone()),
        };
    }
    query
}
fn remote_params(args: &[Cell]) -> Vec<libsql::Value> {
    args.iter()
        .map(|v| match v {
            Cell::Null => libsql::Value::Null,
            Cell::Integer(v) => libsql::Value::Integer(*v),
            Cell::Text(v) => libsql::Value::Text(v.clone()),
            Cell::Blob(v) => libsql::Value::Blob(v.clone()),
        })
        .collect()
}

pub struct RemoteDatabase {
    database: libsql::Database,
    max_connections: u32,
    admission: Arc<Semaphore>,
    cleanup: Mutex<JoinSet<Result<(), sqlx::Error>>>,
    lifecycle: Mutex<RemoteLifecycle>,
    idle: Notify,
    close_serial: tokio::sync::Mutex<()>,
}
struct RemoteLifecycle {
    closing: bool,
    active: usize,
    cleanup_failed: bool,
    failure: Option<sqlx::Error>,
}

/// Admission remains held through an explicit rollback cleanup reply. Closing
/// cannot race an admitted BEGIN or a cancelled stream's cleanup job.
struct RemoteLease {
    _permit: OwnedSemaphorePermit,
    owner: Arc<RemoteDatabase>,
}
impl Drop for RemoteLease {
    fn drop(&mut self) {
        let mut state = self
            .owner
            .lifecycle
            .lock()
            .expect("remote lifecycle mutex poisoned");
        if state.active == 0 {
            state.cleanup_failed = true;
            state.failure.get_or_insert(sqlx::Error::Protocol(
                "remote active lease accounting failed".into(),
            ));
        } else {
            state.active -= 1;
        }
        drop(state);
        self.owner.idle.notify_one();
    }
}

impl RemoteDatabase {
    pub async fn connect(
        url: String,
        token: String,
        max_connections: u32,
    ) -> Result<Arc<Self>, sqlx::Error> {
        let parsed = url::Url::parse(&url)
            .map_err(|_| sqlx::Error::Protocol("invalid libSQL endpoint".into()))?;
        if !matches!(parsed.scheme(), "https" | "libsql")
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || token.is_empty()
        {
            return Err(sqlx::Error::Protocol(
                "remote libSQL requires a TLS primary endpoint and token".into(),
            ));
        }
        let database = libsql::Builder::new_remote(url, token)
            .build()
            .await
            .map_err(remote_error)?;
        Ok(Arc::new(Self {
            database,
            max_connections: max_connections.max(1),
            admission: Arc::new(Semaphore::new(max_connections.max(1) as usize)),
            cleanup: Mutex::new(JoinSet::new()),
            lifecycle: Mutex::new(RemoteLifecycle {
                closing: false,
                active: 0,
                cleanup_failed: false,
                failure: None,
            }),
            idle: Notify::new(),
            close_serial: tokio::sync::Mutex::new(()),
        }))
    }
    async fn begin(self: &Arc<Self>, write: bool) -> Result<RemoteTx, sqlx::Error> {
        self.reap_finished();
        let permit = self
            .admission
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| sqlx::Error::PoolClosed)?;
        {
            let mut state = self
                .lifecycle
                .lock()
                .expect("remote lifecycle mutex poisoned");
            if state.closing {
                return Err(sqlx::Error::PoolClosed);
            }
            state.active += 1;
        }
        let lease = RemoteLease {
            _permit: permit,
            owner: self.clone(),
        };
        let conn = self.database.connect().map_err(remote_error)?;
        // Own the cleanup guard before BEGIN can reach the server. Cancellation
        // never returns this connection/stream to another request.
        let tx = RemoteTx {
            connection: Some(conn),
            lease: Some(lease),
            owner: self.clone(),
            write_reserved: write,
            tenant: None,
            system_context: false,
        };
        let control = if write {
            "PRAGMA foreign_keys=ON; BEGIN IMMEDIATE;"
        } else {
            "PRAGMA foreign_keys=ON; BEGIN;"
        };
        tx.connection()
            .execute_batch(control)
            .await
            .map_err(remote_error)?;
        let mut fk = tx
            .connection()
            .query("PRAGMA foreign_keys", ())
            .await
            .map_err(remote_error)?;
        let enabled = fk
            .next()
            .await
            .map_err(remote_error)?
            .ok_or(sqlx::Error::RowNotFound)?
            .get::<i64>(0)
            .map_err(remote_error)?;
        if enabled != 1 {
            return Err(sqlx::Error::Protocol(
                "remote transaction foreign keys are disabled".into(),
            ));
        }
        Ok(tx)
    }
    fn record_cleanup(&self, result: Result<Result<(), sqlx::Error>, tokio::task::JoinError>) {
        let error = match result {
            Ok(Ok(())) => return,
            Ok(Err(error)) => error,
            Err(_) => sqlx::Error::Protocol("remote cleanup task failed".into()),
        };
        tracing::error!(
            event = "db.remote.cleanup_failed",
            "remote stream discarded after failed explicit rollback"
        );
        let mut state = self
            .lifecycle
            .lock()
            .expect("remote lifecycle mutex poisoned");
        state.cleanup_failed = true;
        if state.failure.is_none() {
            state.failure = Some(error);
        }
    }
    fn reap_finished(&self) {
        loop {
            let finished = self
                .cleanup
                .lock()
                .expect("remote cleanup mutex poisoned")
                .try_join_next();
            match finished {
                Some(result) => self.record_cleanup(result),
                None => break,
            }
        }
    }
    async fn drain_cleanup(&self) -> Result<(), sqlx::Error> {
        let _closing = self.close_serial.lock().await;
        self.lifecycle
            .lock()
            .expect("remote lifecycle mutex poisoned")
            .closing = true;
        self.admission.close();
        loop {
            let notified = self.idle.notified();
            if self
                .lifecycle
                .lock()
                .expect("remote lifecycle mutex poisoned")
                .active
                == 0
            {
                break;
            }
            notified.await;
        }
        // Keep the JoinSet owned by the database while polling. Cancellation
        // of a shutdown deadline must not drop/abort the cleanup jobs or lose
        // their failure evidence. No lock is held across an await.
        while let Some(result) = std::future::poll_fn(|cx| {
            self.cleanup
                .lock()
                .expect("remote cleanup mutex poisoned")
                .poll_join_next(cx)
        })
        .await
        {
            self.record_cleanup(result);
        }
        let mut state = self
            .lifecycle
            .lock()
            .expect("remote lifecycle mutex poisoned");
        if let Some(error) = state.failure.take() {
            return Err(error);
        }
        if state.cleanup_failed {
            return Err(sqlx::Error::Protocol(
                "remote cleanup previously failed".into(),
            ));
        }
        Ok(())
    }
}

/// Connection is deliberately neither Clone nor exposed to concurrent callers.
pub struct RemoteTx {
    write_reserved: bool,
    tenant: Option<uuid::Uuid>,
    system_context: bool,
    connection: Option<libsql::Connection>,
    lease: Option<RemoteLease>,
    owner: Arc<RemoteDatabase>,
}
impl RemoteTx {
    fn connection(&self) -> &libsql::Connection {
        self.connection
            .as_ref()
            .expect("owned unfinished remote transaction")
    }
    async fn finish(mut self, control: &'static str) -> Result<(), sqlx::Error> {
        self.connection()
            .execute_batch(control)
            .await
            .map_err(remote_error)?;
        self.connection.take();
        self.lease.take();
        Ok(())
    }
    async fn commit_with_cleanup(mut self) -> Result<(), CommitCleanupUnknown> {
        if let Err(error) = self.connection().execute_batch("COMMIT").await {
            let source = CommitUnknown {
                source: remote_error(error),
            };
            // The job is owned by the existing service JoinSet, not this
            // receiver/future. Cancellation cannot abort its rollback or lose
            // its cleanup error; no unrelated live transaction is awaited.
            let receipt = self.enqueue_cleanup().expect("unfinished owned stream");
            let cleanup_error = match receipt.await {
                Ok(Ok(())) => None,
                Ok(Err(error)) => Some(shared_cleanup_error(error)),
                Err(_) => Some(sqlx::Error::Protocol(
                    "owned remote cleanup ended without a receipt".into(),
                )),
            };
            return Err(CommitCleanupUnknown {
                source,
                cleanup_error,
            });
        }
        self.connection.take();
        self.lease.take();
        Ok(())
    }
    fn enqueue_cleanup(
        &mut self,
    ) -> Option<tokio::sync::oneshot::Receiver<Result<(), Arc<sqlx::Error>>>> {
        let connection = self.connection.take()?;
        let lease = self.lease.take();
        let (send, receipt) = tokio::sync::oneshot::channel();
        self.owner
            .cleanup
            .lock()
            .expect("remote cleanup mutex poisoned")
            .spawn(async move {
                let result = connection
                    .execute_batch("ROLLBACK")
                    .await
                    .map(|_| ())
                    .map_err(remote_error)
                    .map_err(Arc::new);
                // A receipt means this exact cleanup finished and the stream
                // and admission lease were disposed, even on an error reply.
                drop(connection);
                drop(lease);
                let _ = send.send(result.clone());
                result.map_err(shared_cleanup_error)
            });
        Some(receipt)
    }
}
#[derive(Debug, Clone, thiserror::Error)]
#[error(transparent)]
struct SharedCleanupError(Arc<sqlx::Error>);
fn shared_cleanup_error(error: Arc<sqlx::Error>) -> sqlx::Error {
    sqlx::Error::AnyDriverError(Box::new(SharedCleanupError(error)))
}
impl Drop for RemoteTx {
    fn drop(&mut self) {
        // This quarantined stream is never reused. An explicit rollback reply
        // is tracked; failures stay failures and require service observation.
        let _ = self.enqueue_cleanup();
    }
}
