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

    /// OFF body reads/writes cannot observe a remote predecessor whose finish
    /// is still active or explicitly unconfirmed. Local writers use the normal
    /// transaction reservation. This is not a fabricated remote Close receipt.
    pub(crate) async fn begin_off_body(&self) -> Result<DbTx, sqlx::Error> {
        match self {
            Self::LibsqlRemote(remote) => remote
                .begin_admitted(true, true)
                .await
                .map(|tx| DbTransaction::SqliteFamily(FamilyTx::Remote(tx))),
            _ => self.begin_write().await,
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

/// The original failed COMMIT and its own cleanup/uncertainty receipt.
/// Existing callers retain the original commit API; claim reconciliation uses
/// this boundary before opening a fresh writer. The pinned remote SDK cannot
/// certify failed-finish settlement: that branch explicitly refuses observation.
#[derive(Debug, thiserror::Error)]
#[error("database commit outcome is unknown; settlement receipt retained")]
pub struct CommitCleanupUnknown {
    #[source]
    pub source: CommitUnknown,
    pub settlement: CommitSettlement,
    pub cleanup_error: Option<sqlx::Error>,
}

/// An actual awaited rollback failed. The original domain/driver refusal is
/// retained separately; neither a failed rollback nor Drop proves settlement.
/// Named consumers create this only from their returned rollback error.
#[derive(Debug, thiserror::Error)]
#[error("database rollback settlement is unknown")]
pub(crate) struct RollbackCleanupUnknown {
    pub(crate) original: Option<Box<dyn std::error::Error + Send + Sync>>,
    #[source]
    pub(crate) cleanup: sqlx::Error,
}

pub(crate) fn rollback_cleanup_unknown(
    original: Option<Box<dyn std::error::Error + Send + Sync>>,
    cleanup: sqlx::Error,
) -> sqlx::Error {
    sqlx::Error::AnyDriverError(Box::new(RollbackCleanupUnknown { original, cleanup }))
}

pub(crate) fn is_rollback_cleanup_unknown(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::AnyDriverError(source)
        if source.downcast_ref::<RollbackCleanupUnknown>().is_some())
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommitSettlement {
    LocalWriterReconcile,
    RemoteUnconfirmed,
}
impl CommitCleanupUnknown {
    pub fn permits_reconciliation(&self) -> bool {
        self.settlement == CommitSettlement::LocalWriterReconcile && self.cleanup_error.is_none()
    }
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
                settlement: CommitSettlement::LocalWriterReconcile,
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
    unconfirmed_finish: bool,
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
    /// Isolated test-driver admission using the existing lifecycle. This does
    /// not bypass production endpoint/token checks or attest remote settlement.
    #[cfg(feature = "db-tests")]
    pub fn from_test_driver(database: libsql::Database, max: std::num::NonZeroU32) -> Self {
        Self {
            database,
            max_connections: max.get(),
            admission: Arc::new(Semaphore::new(max.get() as usize)),
            cleanup: Mutex::new(JoinSet::new()),
            lifecycle: Mutex::new(RemoteLifecycle {
                closing: false,
                active: 0,
                cleanup_failed: false,
                failure: None,
                unconfirmed_finish: false,
            }),
            idle: Notify::new(),
            close_serial: tokio::sync::Mutex::new(()),
        }
    }

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
                unconfirmed_finish: false,
            }),
            idle: Notify::new(),
            close_serial: tokio::sync::Mutex::new(()),
        }))
    }
    async fn begin(self: &Arc<Self>, write: bool) -> Result<RemoteTx, sqlx::Error> {
        self.begin_admitted(write, false).await
    }

    async fn begin_admitted(
        self: &Arc<Self>,
        write: bool,
        off_body: bool,
    ) -> Result<RemoteTx, sqlx::Error> {
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
            if off_body && state.unconfirmed_finish {
                return Err(sqlx::Error::AnyDriverError(Box::new(
                    RemoteSettlementUnconfirmed,
                )));
            }
            if off_body && state.active != 0 {
                return Err(sqlx::Error::PoolTimedOut);
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
            finish_started: false,
            #[cfg(test)]
            cleanup_pause: None,
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
        if state.unconfirmed_finish {
            return Err(sqlx::Error::AnyDriverError(Box::new(
                RemoteSettlementUnconfirmed,
            )));
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
    // The pinned SDK may Close/reset its baton before returning a finish
    // error. The same Connection object is then not an original-stream handle.
    finish_started: bool,
    #[cfg(test)]
    cleanup_pause: Option<(
        tokio::sync::oneshot::Sender<()>,
        tokio::sync::oneshot::Receiver<()>,
    )>,
}
impl RemoteTx {
    fn connection(&self) -> &libsql::Connection {
        self.connection
            .as_ref()
            .expect("owned unfinished remote transaction")
    }
    async fn finish(mut self, control: &'static str) -> Result<(), sqlx::Error> {
        self.finish_started = true;
        self.connection()
            .execute_batch(control)
            .await
            .map_err(remote_error)?;
        self.connection.take();
        self.lease.take();
        Ok(())
    }
    async fn commit_with_cleanup(mut self) -> Result<(), CommitCleanupUnknown> {
        self.finish_started = true;
        if let Err(error) = self.connection().execute_batch("COMMIT").await {
            let source = CommitUnknown {
                source: remote_error(error),
            };
            // The SDK hides the original Close receipt on an error. Quarantine
            // and retain explicit uncertainty; never submit a possible NEW
            // stream ROLLBACK or observe as though original finish was proved.
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
                settlement: CommitSettlement::RemoteUnconfirmed,
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
        let finish_started = self.finish_started;
        let owner = self.owner.clone();
        #[cfg(test)]
        let pause = self.cleanup_pause.take();
        let (send, receipt) = tokio::sync::oneshot::channel();
        self.owner
            .cleanup
            .lock()
            .expect("remote cleanup mutex poisoned")
            .spawn(async move {
                #[cfg(test)]
                if let Some((entered, go)) = pause {
                    let _ = entered.send(());
                    let _ = go.await;
                }
                let result = if finish_started {
                    owner
                        .lifecycle
                        .lock()
                        .expect("remote lifecycle mutex poisoned")
                        .unconfirmed_finish = true;
                    // No post-finish SQL cleanup call was made. SDK Drop
                    // Close has no public outcome; missing proof is a distinct
                    // service state, never a fabricated attempt error.
                    Ok(())
                } else {
                    connection
                        .execute_batch("ROLLBACK")
                        .await
                        .map(|_| ())
                        .map_err(remote_error)
                        .map_err(Arc::new)
                };
                // Object/lease disposal is not a server Close receipt. SDK
                // Drop may send a best-effort Close; failure remains failure.
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
#[derive(Debug, thiserror::Error)]
#[error("original remote stream settlement is unconfirmed: pinned SDK hides finish/Close receipt")]
pub struct RemoteSettlementUnconfirmed;
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

#[cfg(all(test, feature = "db-tests"))]
pub(crate) mod maintenance_claim_driver_tests {
    // Transport fixture, NOT Turso: pinned SDK emits real pipeline requests;
    // maintained full SQLite initializer/engine executes the actual SQL. Only
    // finish seam is exercised. Room controls also use real PRAGMA/cursor
    // admission; production TLS and external Turso remain unqualified.
    use super::*;
    use axum::{extract::State, response::IntoResponse, routing::post, Json, Router};
    use base64::Engine;
    use futures_util::StreamExt;
    use serde_json::{json, Value};
    use sqlx::{Column, Connection, Row, TypeInfo, ValueRef};
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU8, Ordering};
    use uuid::Uuid;

    struct Stream {
        conn: sqlx::SqliteConnection,
        active: bool,
        fault_on_finish: bool,
    }
    struct Model {
        pool: SqlitePool,
        streams: HashMap<String, Stream>,
        requests: Vec<Value>,
        sqlite_errors: Vec<String>,
        mode: Arc<AtomicU8>, // 1: COMMIT HTTP failure; 2: rollback failure; 3: truncated body.
        fault_sql: Option<String>,
        loss_pause: Option<(
            tokio::sync::oneshot::Sender<()>,
            tokio::sync::oneshot::Receiver<()>,
        )>,
    }
    fn control_sql(sql: &str) -> Option<&'static str> {
        let sql = sql.trim();
        match sql.strip_suffix(';').unwrap_or(sql).trim_end() {
            "BEGIN" => Some("BEGIN"),
            "BEGIN IMMEDIATE" => Some("BEGIN IMMEDIATE"),
            "COMMIT" => Some("COMMIT"),
            "ROLLBACK" => Some("ROLLBACK"),
            _ => None,
        }
    }
    async fn pipeline(
        State(state): State<Arc<tokio::sync::Mutex<Model>>>,
        bytes: axum::body::Bytes,
    ) -> axum::response::Response {
        // The pinned SDK sends JSON without an application/json header.
        let body: Value = match serde_json::from_slice(&bytes) {
            Ok(body) => body,
            Err(_) => return axum::http::StatusCode::BAD_REQUEST.into_response(),
        };
        let mut model = state.lock().await;
        model.requests.push(body.clone());
        let requests = body["requests"].as_array().unwrap();
        for request in requests {
            if let Some(steps) = request["batch"]["steps"].as_array() {
                for step in steps {
                    let sql = step["stmt"]["sql"].as_str().unwrap();
                    if control_sql(sql).is_some() {
                        println!("S16 actual pinned SDK wire control SQL {sql:?}");
                    }
                }
            }
        }
        let has_sql = |verb: &str| {
            requests.iter().any(|r| {
                r["batch"]["steps"].as_array().is_some_and(|steps| {
                    steps
                        .iter()
                        .any(|s| control_sql(s["stmt"]["sql"].as_str().unwrap()) == Some(verb))
                })
            })
        };
        if model.mode.load(Ordering::SeqCst) == 2 && has_sql("ROLLBACK") {
            return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        let baton = body["baton"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| Uuid::now_v7().to_string());
        if body["baton"].is_string() && !model.streams.contains_key(&baton) {
            // An already closed baton cannot open a fresh server stream.
            return axum::http::StatusCode::GONE.into_response();
        }
        let mut stream = match model.streams.remove(&baton) {
            Some(stream) => stream,
            None => Stream {
                conn: model.pool.acquire().await.unwrap().detach(),
                active: false,
                fault_on_finish: false,
            },
        };
        let mut responses = Vec::new();
        let mut close = false;
        for request in requests {
            let response = match request["type"].as_str().unwrap() {
                "batch" => {
                    let mut results = Vec::new();
                    let mut errors = Vec::new();
                    for step in request["batch"]["steps"].as_array().unwrap() {
                        let stmt = &step["stmt"];
                        let sql = stmt["sql"].as_str().unwrap();
                        if model
                            .fault_sql
                            .as_ref()
                            .is_some_and(|needle| sql.contains(needle))
                        {
                            stream.fault_on_finish = true;
                        }
                        match sqlite_statement(&mut stream, stmt).await {
                            Ok(result) => {
                                results.push(result);
                                errors.push(Value::Null);
                            }
                            Err(error) => {
                                model.sqlite_errors.push(format!("{error:?}"));
                                results.push(Value::Null);
                                errors.push(
                                    json!({"message":error.to_string(),"code":"SQLITE_ERROR"}),
                                );
                            }
                        }
                    }
                    json!({"type":"batch","result":{"step_results":results,"step_errors":errors}})
                }
                "describe" => match sqlite_describe(&mut stream, request).await {
                    Ok(result) => json!({"type":"describe","result":result}),
                    Err(error) => {
                        responses.push(json!({"type":"error","error":{"message":error.to_string(),"code":"SQLITE_ERROR"}}));
                        continue;
                    }
                },
                "get_autocommit" => json!({"type":"get_autocommit","is_autocommit":!stream.active}),
                "close" => {
                    // The actual original SQLite transaction is settled here,
                    // independently of the SDK's later flattened SQL error.
                    if stream.active {
                        sqlx::query("ROLLBACK")
                            .execute(&mut stream.conn)
                            .await
                            .unwrap();
                        stream.active = false;
                    }
                    close = true;
                    json!({"type":"close"})
                }
                other => panic!("unsupported fixture request {other}"),
            };
            responses.push(json!({"type":"ok","response":response}));
        }
        let fault_on_finish = model.fault_sql.is_none() || stream.fault_on_finish;
        if close {
            stream.conn.close().await.unwrap();
        } else {
            model.streams.insert(baton.clone(), stream);
        }
        let response = json!({"baton":if close { None } else { Some(baton) },"base_url":null,"results":responses});
        if model.mode.load(Ordering::SeqCst) == 1 && has_sql("COMMIT") && fault_on_finish {
            return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        if model.mode.load(Ordering::SeqCst) == 3 && has_sql("COMMIT") && fault_on_finish {
            let (entered, go) = model.loss_pause.take().unwrap();
            let first = futures_util::stream::once(async move {
                let _ = entered.send(());
                Ok::<_, std::io::Error>(bytes::Bytes::from_static(b"{\"baton\":"))
            });
            let lost = futures_util::stream::once(async move {
                let _ = go.await;
                Err::<bytes::Bytes, _>(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "fixture reply lost after headers/body prefix",
                ))
            });
            return axum::response::Response::new(axum::body::Body::from_stream(first.chain(lost)));
        }
        Json(response).into_response()
    }
    // Describe prepares on the same SQLx-owned SQLite handle, without stepping
    // the statement. The pinned SQLite engine supplies Hrana metadata; neither
    // the test service nor the SDK invents columns, parameters or readonly flags.
    async fn sqlite_describe(stream: &mut Stream, request: &Value) -> Result<Value, sqlx::Error> {
        use std::ffi::{CStr, CString};
        use std::ptr::NonNull;

        struct Prepared(NonNull<libsqlite3_sys::sqlite3_stmt>);
        impl Drop for Prepared {
            fn drop(&mut self) {
                // SAFETY: this uniquely owned statement is finalized once,
                // before the SQLx exclusive native-handle guard is released.
                unsafe { libsqlite3_sys::sqlite3_finalize(self.0.as_ptr()) };
            }
        }
        let sql = request["sql"]
            .as_str()
            .filter(|_| request["sql_id"].is_null())
            .ok_or_else(|| sqlx::Error::Protocol("fixture describe requires literal SQL".into()))?;
        let sql = CString::new(sql)
            .map_err(|_| sqlx::Error::Protocol("fixture describe SQL contains NUL".into()))?;
        let mut handle = stream.conn.lock_handle().await?;
        let db = handle.as_raw_handle().as_ptr();
        let mut statement = std::ptr::null_mut();
        // SAFETY: SQLx holds the exclusive handle guard; SQL is NUL-terminated,
        // and SQLite owns the prepared statement until the local guard drops.
        let code = unsafe {
            libsqlite3_sys::sqlite3_prepare_v2(
                db,
                sql.as_ptr(),
                -1,
                &mut statement,
                std::ptr::null_mut(),
            )
        };
        let statement = NonNull::new(statement).map(Prepared);
        if code != libsqlite3_sys::SQLITE_OK {
            // SAFETY: SQLite's error message is valid while the handle guard
            // is held. Copy it before another operation can replace it.
            let message = unsafe { CStr::from_ptr(libsqlite3_sys::sqlite3_errmsg(db)) };
            return Err(sqlx::Error::Protocol(format!(
                "SQLite describe failed ({code}): {}",
                message.to_string_lossy()
            )));
        }
        let statement = statement
            .ok_or_else(|| sqlx::Error::Protocol("fixture describe has no statement".into()))?;
        // SAFETY: all metadata pointers are SQLite-owned for this live prepared
        // statement; copy them before finalize. No sqlite3_step is performed.
        let result = unsafe {
            let text = |ptr: *const std::ffi::c_char| {
                (!ptr.is_null()).then(|| CStr::from_ptr(ptr).to_string_lossy().into_owned())
            };
            let stmt = statement.0.as_ptr();
            let params = (1..=libsqlite3_sys::sqlite3_bind_parameter_count(stmt))
                .map(|i| json!({"name":text(libsqlite3_sys::sqlite3_bind_parameter_name(stmt, i))}))
                .collect::<Vec<_>>();
            let cols = (0..libsqlite3_sys::sqlite3_column_count(stmt))
                .map(|i| {
                    let name = text(libsqlite3_sys::sqlite3_column_name(stmt, i))
                        .ok_or_else(|| sqlx::Error::Protocol("SQLite column name unavailable".into()))?;
                    Ok(json!({"name":name,"decltype":text(libsqlite3_sys::sqlite3_column_decltype(stmt, i))}))
                })
                .collect::<Result<Vec<_>, sqlx::Error>>()?;
            json!({"params":params,"cols":cols,"is_explain":libsqlite3_sys::sqlite3_stmt_isexplain(stmt)!=0,"is_readonly":libsqlite3_sys::sqlite3_stmt_readonly(stmt)!=0})
        };
        Ok(result)
    }

    // SQLx supplies both actual returned rows (including RETURNING/PRAGMA)
    // and affected counts. No transport response invents database contents.
    #[expect(
        deprecated,
        reason = "single bound SQLite statement needs both rows and QueryResult; raw_sql cannot bind Hrana args"
    )]
    async fn sqlite_statement(stream: &mut Stream, stmt: &Value) -> Result<Value, sqlx::Error> {
        let sql = stmt["sql"].as_str().unwrap();
        let mut query = sqlx::query(sql);
        for arg in stmt["args"].as_array().unwrap() {
            query = match arg["type"].as_str().unwrap() {
                "null" => query.bind(Option::<i64>::None),
                "integer" => query.bind(arg["value"].as_str().unwrap().parse::<i64>().unwrap()),
                "float" => query.bind(arg["value"].as_f64().unwrap()),
                "text" => query.bind(arg["value"].as_str().unwrap().to_owned()),
                "blob" => query.bind(
                    base64::engine::general_purpose::STANDARD_NO_PAD
                        .decode(arg["base64"].as_str().unwrap().trim_end_matches('='))
                        .unwrap(),
                ),
                other => panic!("unsupported fixture bind {other}"),
            };
        }
        let mut cols = Vec::new();
        let mut rows = Vec::new();
        let mut affected = 0;
        let mut results = query.fetch_many(&mut stream.conn);
        while let Some(result) = results.next().await {
            match result? {
                sqlx::Either::Left(result) => affected += result.rows_affected(),
                sqlx::Either::Right(row) => {
                    if cols.is_empty() {
                        cols = row
                            .columns()
                            .iter()
                            .map(|col| json!({"name":col.name(),"decltype":null}))
                            .collect();
                    }
                    let values = (0..row.len()).map(|i| {
                        let value = row.try_get_raw(i)?;
                        Ok(if value.is_null() { json!({"type":"null"}) } else {
                            match value.type_info().name() {
                                "INTEGER" | "BOOLEAN" => json!({"type":"integer","value":row.try_get::<i64,_>(i)?.to_string()}),
                                "REAL" => json!({"type":"float","value":row.try_get::<f64,_>(i)?}),
                                "TEXT" => json!({"type":"text","value":row.try_get::<String,_>(i)?}),
                                "BLOB" => json!({"type":"blob","base64":base64::engine::general_purpose::STANDARD_NO_PAD.encode(row.try_get::<Vec<u8>,_>(i)?)}),
                                other => panic!("unsupported actual SQLite value {other}"),
                            }
                        })
                    }).collect::<Result<Vec<Value>,sqlx::Error>>()?;
                    rows.push(values);
                }
            }
        }
        drop(results);
        match control_sql(sql) {
            Some("BEGIN" | "BEGIN IMMEDIATE") => stream.active = true,
            Some("COMMIT" | "ROLLBACK") => stream.active = false,
            _ => {}
        }
        Ok(json!({"cols":cols,"rows":rows,"affected_row_count":affected,"last_insert_rowid":null}))
    }

    async fn cursor(
        State(state): State<Arc<tokio::sync::Mutex<Model>>>,
        bytes: axum::body::Bytes,
    ) -> axum::response::Response {
        let body: Value = match serde_json::from_slice(&bytes) {
            Ok(body) => body,
            Err(_) => return axum::http::StatusCode::BAD_REQUEST.into_response(),
        };
        let mut model = state.lock().await;
        model.requests.push(body.clone());
        let baton = body["baton"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| Uuid::now_v7().to_string());
        if body["baton"].is_string() && !model.streams.contains_key(&baton) {
            return axum::http::StatusCode::GONE.into_response();
        }
        let mut stream = match model.streams.remove(&baton) {
            Some(stream) => stream,
            None => Stream {
                conn: model.pool.acquire().await.unwrap().detach(),
                active: false,
                fault_on_finish: false,
            },
        };
        let mut lines = vec![json!({"baton":baton,"base_url":null})];
        for (step, item) in body["batch"]["steps"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
        {
            let stmt = &item["stmt"];
            if model
                .fault_sql
                .as_ref()
                .is_some_and(|needle| stmt["sql"].as_str().unwrap().contains(needle))
            {
                stream.fault_on_finish = true;
            }
            match sqlite_statement(&mut stream, stmt).await {
                Ok(result) => {
                    lines.push(json!({"type":"step_begin","step":step,"cols":result["cols"]}));
                    for row in result["rows"].as_array().unwrap() {
                        lines.push(json!({"type":"row","row":row}));
                    }
                    lines.push(json!({"type":"step_end","affected_row_count":result["affected_row_count"],"last_inserted_rowid":null}));
                }
                Err(error) => {
                    model.sqlite_errors.push(format!("{error:?}"));
                    lines.push(json!({"type":"step_error","step":step,"error":{"message":error.to_string(),"code":"SQLITE_ERROR"}}));
                }
            }
        }
        model.streams.insert(baton, stream);
        let encoded = lines
            .iter()
            .map(|line| format!("{line}\n"))
            .collect::<String>();
        encoded.into_response()
    }

    pub(crate) struct Fixture {
        root: PathBuf,
        endpoint: String,
        model: Arc<tokio::sync::Mutex<Model>>,
        backend: Backend,
        stop: tokio::sync::oneshot::Sender<()>,
        server: tokio::task::JoinHandle<std::io::Result<()>>,
        mode: Arc<AtomicU8>,
    }
    impl Fixture {
        async fn new() -> Self {
            let root = std::env::temp_dir().join(format!("fvoci-s16-driver-{}", Uuid::now_v7()));
            std::fs::create_dir_all(&root).unwrap();
            let path = root.join("app.sqlite");
            crate::db::migrate::run_sqlite_migrations(&path)
                .await
                .unwrap();
            Self::new_at_path(root, &path).await
        }
        pub(crate) async fn for_existing_path(path: &std::path::Path) -> Self {
            let root = std::env::temp_dir().join(format!("fvoci-task-driver-{}", Uuid::now_v7()));
            std::fs::create_dir_all(&root).unwrap();
            Self::new_at_path(root, path).await
        }
        async fn new_at_path(root: PathBuf, path: &std::path::Path) -> Self {
            let pool = crate::db::pool::connect_sqlite_app(path, 3).await.unwrap();
            let mode = Arc::new(AtomicU8::new(0));
            let model = Arc::new(tokio::sync::Mutex::new(Model {
                pool,
                streams: HashMap::new(),
                requests: Vec::new(),
                sqlite_errors: Vec::new(),
                mode: mode.clone(),
                fault_sql: None,
                loss_pause: None,
            }));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let (stop, wait) = tokio::sync::oneshot::channel();
            let app = Router::new()
                .route("/v3/pipeline", post(pipeline))
                .route("/v3/cursor", post(cursor))
                .with_state(model.clone());
            let server = tokio::spawn(async move {
                axum::serve(listener, app)
                    .with_graceful_shutdown(async {
                        let _ = wait.await;
                    })
                    .await
            });
            // Test-only plain HTTP transport. Production connect's TLS URL
            // validation is unchanged and is NOT qualified by this fixture.
            let endpoint = format!("http://{addr}");
            let database = libsql::Builder::new_remote(endpoint.clone(), "fixture-only".into())
                .build()
                .await
                .unwrap();
            let backend = Backend::LibsqlRemote(Arc::new(RemoteDatabase {
                database,
                max_connections: 1,
                admission: Arc::new(Semaphore::new(1)),
                cleanup: Mutex::new(JoinSet::new()),
                lifecycle: Mutex::new(RemoteLifecycle {
                    closing: false,
                    active: 0,
                    cleanup_failed: false,
                    failure: None,
                    unconfirmed_finish: false,
                }),
                idle: Notify::new(),
                close_serial: tokio::sync::Mutex::new(()),
            }));
            println!(
                "S16 pinned SDK HTTP finish fixture {} {addr}",
                root.display()
            );
            Self {
                root,
                endpoint,
                model,
                backend,
                stop,
                server,
                mode,
            }
        }
        pub(crate) fn backend(&self) -> Backend {
            self.backend.clone()
        }
        pub(crate) async fn fail_finish_for(&self, sql: &str, mode: u8) {
            assert!(matches!(mode, 1 | 3));
            let mut model = self.model.lock().await;
            model.fault_sql = Some(sql.into());
            if mode == 3 {
                let (entered, _) = tokio::sync::oneshot::channel();
                let (go, wait) = tokio::sync::oneshot::channel();
                // Ordinary frame awaits the actual truncated response. The old
                // cancellation control still supplies its own barrier below.
                go.send(()).unwrap();
                model.loss_pause = Some((entered, wait));
            }
            self.mode.store(mode, Ordering::SeqCst);
        }
        pub(crate) async fn sql_log(&self) -> Vec<String> {
            self.model
                .lock()
                .await
                .requests
                .iter()
                .flat_map(|body| {
                    let batches: Vec<&Value> = if let Some(requests) = body["requests"].as_array() {
                        requests
                            .iter()
                            .filter_map(|request| request.get("batch"))
                            .collect()
                    } else {
                        body.get("batch").into_iter().collect()
                    };
                    batches
                        .into_iter()
                        .flat_map(|batch| {
                            batch["steps"]
                                .as_array()
                                .unwrap()
                                .iter()
                                .map(|step| step["stmt"]["sql"].as_str().unwrap().to_owned())
                        })
                        .collect::<Vec<_>>()
                })
                .collect()
        }
        pub(crate) async fn assert_original_finish_for(&self, needle: &str) {
            let model = self.model.lock().await;
            let selected = model
                .requests
                .iter()
                .rev()
                .find(|body| {
                    if let Some(requests) = body["requests"].as_array() {
                        requests.iter().any(|request| {
                            request["batch"]["steps"].as_array().is_some_and(|steps| {
                                steps.iter().any(|step| {
                                    step["stmt"]["sql"].as_str().unwrap().contains(needle)
                                })
                            })
                        })
                    } else {
                        body["batch"]["steps"].as_array().is_some_and(|steps| {
                            steps
                                .iter()
                                .any(|step| step["stmt"]["sql"].as_str().unwrap().contains(needle))
                        })
                    }
                })
                .expect("selected operation reached actual SDK transport");
            let baton = selected["baton"]
                .as_str()
                .expect("operation on the original admitted stream");
            assert!(
                model.requests.iter().any(|body| body["baton"] == baton
                    && body["requests"].as_array().is_some_and(|requests| requests
                        .iter()
                        .any(|request| request["type"] == "close")
                        && requests.iter().any(|request| request["batch"]["steps"]
                            .as_array()
                            .is_some_and(|steps| steps
                                .iter()
                                .any(|step| control_sql(step["stmt"]["sql"].as_str().unwrap())
                                    == Some("COMMIT")))))),
                "original stream COMMIT plus Close, not a new transaction acknowledgement"
            );
        }
        pub(crate) async fn shutdown(self, uncertain: bool, expect_fk: bool) {
            let result = self.backend.close().await;
            if uncertain {
                assert!(
                    matches!(result, Err(sqlx::Error::AnyDriverError(ref error)) if error.downcast_ref::<RemoteSettlementUnconfirmed>().is_some())
                );
            } else {
                result.unwrap();
            }
            let _ = self.stop.send(());
            self.server.await.unwrap().unwrap();
            let mut model = self.model.lock().await;
            if expect_fk {
                assert_eq!(model.sqlite_errors.len(), 1);
                assert!(model.sqlite_errors[0].contains("FOREIGN KEY constraint failed"));
            } else {
                assert!(
                    model.sqlite_errors.is_empty(),
                    "unexpected real SQLite error: {:?}",
                    model.sqlite_errors
                );
            }
            for (_, mut stream) in model.streams.drain() {
                if stream.active {
                    sqlx::query("ROLLBACK")
                        .execute(&mut stream.conn)
                        .await
                        .unwrap();
                }
                stream.conn.close().await.unwrap();
            }
            model.pool.close().await;
            drop(model);
            std::fs::remove_dir_all(self.root).unwrap();
        }
        async fn writer(&self) -> RemoteTx {
            let Backend::LibsqlRemote(owner) = &self.backend else {
                unreachable!()
            };
            let permit = owner.admission.clone().acquire_owned().await.unwrap();
            owner.lifecycle.lock().unwrap().active += 1;
            let tx = RemoteTx {
                write_reserved: true,
                tenant: None,
                system_context: false,
                connection: Some(owner.database.connect().unwrap()),
                lease: Some(RemoteLease {
                    _permit: permit,
                    owner: owner.clone(),
                }),
                owner: owner.clone(),
                finish_started: false,
                cleanup_pause: None,
            };
            // Actual SDK BEGIN and FK-on; the test targets finish directly and
            // does not pretend to execute the product query/cursor admission.
            tx.connection()
                .execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")
                .await
                .unwrap();
            tx
        }
        async fn fault(tx: &RemoteTx) {
            tx.connection()
                .execute_batch("PRAGMA defer_foreign_keys=ON")
                .await
                .unwrap();
            tx.connection()
                .execute(
                    "INSERT INTO memberships(workspace_id,user_id,role) VALUES(?1,?2,'owner')",
                    vec![
                        libsql::Value::Blob(Uuid::now_v7().as_bytes().to_vec()),
                        libsql::Value::Blob(Uuid::now_v7().as_bytes().to_vec()),
                    ],
                )
                .await
                .unwrap();
        }
        async fn no_wrong_rollback(&self) {
            let model = self.model.lock().await;
            assert!(
                model
                    .requests
                    .iter()
                    .all(|r| r["requests"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .all(|q| q["batch"]["steps"].as_array().is_none_or(|steps| steps
                            .iter()
                            .all(|s| control_sql(s["stmt"]["sql"].as_str().unwrap())
                                != Some("ROLLBACK"))))),
                "failed finish must never submit a possible new-stream rollback"
            );
            let commits: Vec<_> = model
                .requests
                .iter()
                .filter(|r| {
                    r["requests"].as_array().unwrap().iter().any(|q| {
                        q["batch"]["steps"].as_array().is_some_and(|s| {
                            s.iter().any(|s| {
                                control_sql(s["stmt"]["sql"].as_str().unwrap()) == Some("COMMIT")
                            })
                        })
                    })
                })
                .collect();
            assert!(!commits.is_empty());
            assert!(
                commits.iter().all(|r| r["baton"].is_string()
                    && r["requests"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|q| q["type"] == "close")),
                "pinned driver actually emitted original baton COMMIT plus Close"
            );
        }
        async fn close(self) {
            let error = self.backend.close().await.unwrap_err();
            if self.mode.load(Ordering::SeqCst) != 2 {
                assert!(matches!(error,sqlx::Error::AnyDriverError(ref error) if error.downcast_ref::<RemoteSettlementUnconfirmed>().is_some()), "unconfirmed settlement is a distinct service state, not a cleanup-attempt error");
            }
            let _ = self.stop.send(());
            self.server.await.unwrap().unwrap();
            let mut model = self.model.lock().await;
            for (_, mut stream) in model.streams.drain() {
                if stream.active {
                    sqlx::query("ROLLBACK")
                        .execute(&mut stream.conn)
                        .await
                        .unwrap();
                }
                stream.conn.close().await.unwrap();
            }
            model.pool.close().await;
            drop(model);
            std::fs::remove_dir_all(self.root).unwrap();
        }
    }
    fn unconfirmed(error: &CommitCleanupUnknown) {
        let sqlx::Error::AnyDriverError(original) = &error.source.source else {
            panic!("original pinned SDK error retained")
        };
        assert!(original.downcast_ref::<libsql::Error>().is_some());
        assert_eq!(error.settlement, CommitSettlement::RemoteUnconfirmed);
        assert!(
            error.cleanup_error.is_none(),
            "no cleanup attempt error was fabricated"
        );
        assert!(
            !error.permits_reconciliation(),
            "Ok disposal/None attempt error must not permit fresh observation"
        );
    }
    #[tokio::test]
    async fn maintenance_claim_driver_describe_real_metadata_no_effect_and_sql_error() {
        let f = Fixture::new().await;
        let pool = f.model.lock().await.pool.clone();
        sqlx::query("CREATE TABLE describe_probe(id INTEGER PRIMARY KEY,label TEXT NOT NULL)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO describe_probe VALUES(1,'original 中 😀')")
            .execute(&pool)
            .await
            .unwrap();
        let tx = f.writer().await;
        let sql = "SELECT id AS original_id,label,?1 AS bound FROM describe_probe WHERE id=:named AND label=?";
        // Actual unmodified SDK prepare must decode the pipeline describe reply.
        let prepared = tx.connection().prepare(sql).await.unwrap();
        assert_eq!(prepared.column_count(), 3);
        let columns = prepared.columns();
        assert_eq!(
            columns.iter().map(|c| c.name()).collect::<Vec<_>>(),
            vec!["original_id", "label", "bound"]
        );
        let baton = {
            let model = f.model.lock().await;
            let request = model
                .requests
                .iter()
                .rev()
                .find(|body| {
                    body["requests"].as_array().is_some_and(|requests| {
                        requests
                            .iter()
                            .any(|r| r["type"] == "describe" && r["sql"] == sql)
                    })
                })
                .expect("actual SDK describe reached its original stream");
            request["baton"].as_str().unwrap().to_owned()
        };
        let client = reqwest::Client::new();
        let describe = |request: Value| {
            client
                .post(format!("{}/v3/pipeline", f.endpoint))
                .json(&json!({"baton":baton,"requests":[request]}))
        };
        let body: Value = describe(json!({"type":"describe","sql":sql}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(body["baton"], baton);
        assert_eq!(
            body["results"][0]["response"]["result"],
            json!({
                "params":[{"name":"?1"},{"name":":named"},{"name":null}],
                "cols":[{"name":"original_id","decltype":"INTEGER"},
                        {"name":"label","decltype":"TEXT"},{"name":"bound","decltype":null}],
                "is_explain":false,"is_readonly":true
            })
        );
        let write_sql = "UPDATE describe_probe SET label=?1 WHERE id=?2 RETURNING id,label";
        let write: Value = describe(json!({"type":"describe","sql":write_sql}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            write["results"][0]["response"]["result"]["is_readonly"],
            false
        );
        assert_eq!(
            write["results"][0]["response"]["result"]["params"],
            json!([{"name":"?1"},{"name":"?2"}])
        );
        let explain: Value =
            describe(json!({"type":"describe","sql":"EXPLAIN SELECT id FROM describe_probe"}))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
        assert_eq!(
            explain["results"][0]["response"]["result"]["is_explain"],
            true
        );
        assert!(
            tx.connection()
                .prepare("SELECT missing FROM describe_probe")
                .await
                .is_err(),
            "invalid SQL cannot become fabricated successful metadata"
        );
        for request in [
            json!({"type":"describe","sql_id":1}),
            json!({"type":"describe","sql":""}),
            json!({"type":"describe","sql":"SELECT\u{0}1"}),
        ] {
            let refused: Value = describe(request)
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            assert_eq!(refused["results"][0]["type"], "error");
            assert!(refused["results"][0].get("response").is_none());
        }
        let original: (i64, String) = sqlx::query_as("SELECT id,label FROM describe_probe")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            original,
            (1, "original 中 😀".into()),
            "preparing write/explain/invalid SQL neither steps nor modifies data"
        );
        drop(columns);
        drop(prepared);
        DbTransaction::SqliteFamily(FamilyTx::Remote(tx))
            .commit_with_cleanup()
            .await
            .unwrap();
        f.shutdown(false, false).await;
    }

    #[tokio::test]
    async fn maintenance_claim_driver_fk_close_does_not_rollback_new_stream() {
        for (sql, expected) in [
            ("BEGIN IMMEDIATE", "BEGIN IMMEDIATE"),
            (" BEGIN IMMEDIATE; ", "BEGIN IMMEDIATE"),
            ("COMMIT", "COMMIT"),
            (" COMMIT; ", "COMMIT"),
            ("ROLLBACK", "ROLLBACK"),
            (" ROLLBACK; ", "ROLLBACK"),
        ] {
            assert_eq!(control_sql(sql), Some(expected));
        }
        for sql in [
            "COMMIT;;",
            "COMMIT; SELECT 1",
            "ROLLBACK TO savepoint;",
            "SELECT 'COMMIT';",
            "BEGIN DEFERRED;",
        ] {
            assert_eq!(control_sql(sql), None);
        }
        let f = Fixture::new().await;
        let malformed = reqwest::Client::new()
            .post(format!("{}/v3/pipeline", f.endpoint))
            .body("{")
            .send()
            .await
            .unwrap();
        assert_eq!(malformed.status(), reqwest::StatusCode::BAD_REQUEST);
        let model = f.model.lock().await;
        assert!(model.requests.is_empty() && model.streams.is_empty());
        drop(model);
        let owner = Uuid::now_v7();
        let tx = f.writer().await;
        assert!(f.model.lock().await.requests.iter().any(|r| {
            r["requests"].as_array().unwrap().iter().any(|q| {
                q["batch"]["steps"].as_array().is_some_and(|steps| {
                    steps.iter().any(|s| {
                        control_sql(s["stmt"]["sql"].as_str().unwrap()) == Some("BEGIN IMMEDIATE")
                    })
                })
            })
        }));
        println!("S16 malformed JSON rejected400; actual unmodified SDK BEGIN reached");
        tx.connection().execute("UPDATE maintenance_job_claims SET owner_token=?1,generation=1,expires_at=unixepoch()*1000000+60000000 WHERE job_key=1",vec![libsql::Value::Blob(owner.as_bytes().to_vec())]).await.unwrap();
        DbTransaction::SqliteFamily(FamilyTx::Remote(tx))
            .commit_with_cleanup()
            .await
            .unwrap();
        let tx = f.writer().await;
        Fixture::fault(&tx).await;
        let error = DbTransaction::SqliteFamily(FamilyTx::Remote(tx))
            .commit_with_cleanup()
            .await
            .unwrap_err();
        unconfirmed(&error);
        f.no_wrong_rollback().await;
        let model = f.model.lock().await;
        assert!(
            model.sqlite_errors.iter().any(|e| e.contains("787")),
            "real maintained SQLite FK COMMIT rejection"
        );
        let row: (Vec<u8>, i64) = sqlx::query_as(
            "SELECT owner_token,generation FROM maintenance_job_claims WHERE job_key=1",
        )
        .fetch_one(&model.pool)
        .await
        .unwrap();
        assert_eq!(row, (owner.as_bytes().to_vec(), 1));
        drop(model);
        f.close().await;
    }
    #[tokio::test]
    async fn maintenance_claim_driver_commit_reply_loss_retains_uncertainty() {
        for mode in [1, 3] {
            let f = Fixture::new().await;
            let tx = f.writer().await;
            tx.connection()
                .execute(
                    "UPDATE maintenance_job_claims SET generation=1 WHERE job_key=8",
                    (),
                )
                .await
                .unwrap();
            f.mode.store(mode, Ordering::SeqCst);
            let error = if mode == 3 {
                let (entered, receive) = tokio::sync::oneshot::channel();
                let (go, wait) = tokio::sync::oneshot::channel();
                f.model.lock().await.loss_pause = Some((entered, wait));
                let finish =
                    DbTransaction::SqliteFamily(FamilyTx::Remote(tx)).commit_with_cleanup();
                tokio::pin!(finish);
                tokio::select! { result=receive=>result.unwrap(), _=&mut finish=>panic!("body-loss barrier must be reached") }
                go.send(()).unwrap();
                finish.await.unwrap_err()
            } else {
                DbTransaction::SqliteFamily(FamilyTx::Remote(tx))
                    .commit_with_cleanup()
                    .await
                    .unwrap_err()
            };
            unconfirmed(&error);
            f.no_wrong_rollback().await;
            let model = f.model.lock().await;
            let generation: i64 =
                sqlx::query_scalar("SELECT generation FROM maintenance_job_claims WHERE job_key=8")
                    .fetch_one(&model.pool)
                    .await
                    .unwrap();
            assert_eq!(
                generation, 1,
                "server actually committed before reply was lost"
            );
            drop(model);
            f.close().await;
        }
    }
    #[tokio::test]
    async fn maintenance_claim_driver_cancelled_receiver_retains_owned_cleanup() {
        let f = Fixture::new().await;
        let mut tx = f.writer().await;
        Fixture::fault(&tx).await;
        let (entered, receive) = tokio::sync::oneshot::channel();
        let (go, wait) = tokio::sync::oneshot::channel();
        tx.cleanup_pause = Some((entered, wait));
        // Drop only the waiting receiver/future; no arbitrary task abort or
        // claim-release assertion. The original service-owned job stays live.
        tokio::select! { result=receive=>result.unwrap(), _=tx.commit_with_cleanup()=>panic!("cleanup must stay paused") }
        assert_eq!(f.backend.connection_stats().unwrap().size, 1);
        go.send(()).unwrap();
        f.no_wrong_rollback().await;
        f.close().await;
    }
    #[tokio::test]
    async fn maintenance_claim_driver_real_rollback_transport_failure_is_retained() {
        let f = Fixture::new().await;
        let tx = f.writer().await;
        f.mode.store(2, Ordering::SeqCst);
        drop(tx); // Ordinary pre-finish Drop still owns a tracked rollback.
        let error = f.backend.close().await.unwrap_err();
        assert!(
            matches!(error,sqlx::Error::AnyDriverError(ref e) if e.downcast_ref::<SharedCleanupError>().is_some())
        );
        let model = f.model.lock().await;
        let rollback = model
            .requests
            .iter()
            .find(|r| {
                r["requests"].as_array().unwrap().iter().any(|q| {
                    q["batch"]["steps"].as_array().is_some_and(|s| {
                        s.iter().any(|s| {
                            control_sql(s["stmt"]["sql"].as_str().unwrap()) == Some("ROLLBACK")
                        })
                    })
                })
            })
            .expect("actual rollback transport request");
        assert!(
            rollback["baton"].is_string(),
            "ordinary rollback used its existing stream"
        );
        drop(model);
        f.close().await;
    }
}
