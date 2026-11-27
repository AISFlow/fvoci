//! Reads that agree with each other: several statements run in one consistent snapshot
//! of the database.

use sqlx::Connection as _;

use crate::db::{ConnRef, Scope};
use crate::error::StoreError;
use crate::observe::{Access, Watch};
use crate::{Pool, Store};

/// A scope that reads one consistent snapshot of the database: what its statements
/// find agrees, whatever is written meanwhile. Only [`Store::read_snapshot`] opens one.
pub(crate) struct Snapshot<'c> {
    scope: Scope<'c>,
}

impl<'c> Snapshot<'c> {
    /// The scope the statements of the snapshot run in.
    pub(crate) fn scope(&mut self) -> &mut Scope<'c> {
        &mut self.scope
    }
}

impl Store {
    /// Runs a multi-statement read in a single consistent snapshot (`BEGIN DEFERRED` on
    /// SQLite, `BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY` on PostgreSQL) without
    /// acquiring SQLite's writer lock.
    pub(crate) async fn read_snapshot<T>(
        &self,
        operation: impl AsyncFnOnce(&mut Snapshot<'_>) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let watch = self.watch(Access::Read);
        let kept = self.versions_kept();
        let result = match &self.inner.pool {
            Pool::Sqlite(pool) => match Watch::acquire(watch.as_ref(), pool).await {
                Ok(mut conn) => match conn.begin_with("BEGIN DEFERRED").await {
                    Ok(mut tx) => {
                        let mut snapshot = Snapshot {
                            scope: Scope::new(ConnRef::Sqlite(&mut tx), self.tenant, kept)
                                .with_clock(&self.inner.clock),
                        };
                        let res = operation(&mut snapshot).await;
                        drop(tx);
                        res
                    }
                    Err(error) => Err(error.into()),
                },
                Err(error) => Err(error.into()),
            },
            Pool::Postgres(pool) => match Watch::acquire(watch.as_ref(), pool).await {
                Ok(mut conn) => match conn
                    .begin_with(self.tenant.begin_read_snapshot_on_postgres())
                    .await
                {
                    Ok(mut tx) => {
                        let mut snapshot = Snapshot {
                            scope: Scope::new(ConnRef::Postgres(&mut tx), self.tenant, kept)
                                .with_clock(&self.inner.clock),
                        };
                        let res = operation(&mut snapshot).await;
                        drop(tx);
                        res
                    }
                    Err(error) => Err(error.into()),
                },
                Err(error) => Err(error.into()),
            },
        };
        if let Some(watch) = watch {
            watch.finish(Access::Read, &result);
        }
        result
    }
}
