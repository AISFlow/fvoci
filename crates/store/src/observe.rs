//! What a [`Store`](crate::Store) tells an [`Observer`] about its calls: how long each
//! took, whether it read or wrote, how it ended, and how long it waited for a
//! connection. For metrics, which this crate does not keep: the server gives a store an
//! observer that counts into them ([`Store::observe`](crate::Store::observe)).
//!
//! A call is told also when its caller gives it up (a request that went away): as
//! [`Outcome::Abandoned`], and its wait for a connection as far as it went.
//!
//! A store without an observer pays one atomic load a call, and reads no clock.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use sqlx::pool::PoolConnection;

use crate::db::Scope;
use crate::{DbType, StoreError};

/// Who is told about the calls of a store. Its methods are called on the task that made
/// the call, after the fact, also while a call that was given up is dropped: they are to
/// be quick and not to block.
pub trait Observer: Send + Sync + std::fmt::Debug {
    /// A call of the store ended.
    fn called(&self, call: Call);

    /// A call waited `waited` for a connection of the pool of a database of `db`.
    fn waited_for_connection(&self, db: DbType, waited: Duration);
}

/// A call of a store: one transaction, or one connection for a read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Call {
    /// The kind of database.
    pub db: DbType,
    /// Whether it wrote.
    pub access: Access,
    /// How it ended.
    pub outcome: Outcome,
    /// How long it took, from its start to the end of its transaction: what it waited
    /// for a connection included, and on SQLite what a transaction waited for its turn
    /// among the writers.
    pub took: Duration,
}

/// Whether a call wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Every statement only read. A call on one connection that ended before its
    /// statements ran (no connection came, or the transaction did not begin) is a read
    /// too: what it would have done is not known.
    Read,
    /// It was a transaction that may write, or some statement that may write started:
    /// also one that its caller gave up before it ended.
    Write,
}

impl Access {
    /// Whether a call on one connection wrote: whether a statement in `scope` did.
    pub(crate) fn of(scope: &Scope<'_>) -> Self {
        if scope.wrote() {
            Self::Write
        } else {
            Self::Read
        }
    }
}

/// How a call ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// It did what it was asked.
    Done,
    /// It answered that it could not: what was asked for is not there, is taken, or is
    /// not valid. The database did its part.
    Refused,
    /// The call did not get an answer: the database failed it, gave back what cannot be
    /// read, or is not one the server can use; or the call was made without the
    /// organization it needs, a fault of the server.
    Failed,
    /// Its caller gave it up before it ended. What it did is not known; a transaction
    /// that did not end is rolled back.
    Abandoned,
}

impl Outcome {
    /// How a call that returned `result` ended.
    pub(crate) fn of<T>(result: &Result<T, StoreError>) -> Self {
        let Err(error) = result else {
            return Self::Done;
        };
        // Every error is named, so that a new one is sorted where it is made.
        match error {
            StoreError::Database(_)
            | StoreError::Corrupt { .. }
            | StoreError::Schema(_)
            | StoreError::SchemaNotOwned { .. }
            | StoreError::Config(_)
            | StoreError::InvalidPath(_)
            | StoreError::NoOrganization => Self::Failed,
            StoreError::TeamSelfJoining
            | StoreError::GuestCannotJoinTeam
            | StoreError::DuplicateReorderId(_)
            | StoreError::NotFound(_)
            | StoreError::TooManyClientStateEntries { .. }
            | StoreError::NotFirstUser
            | StoreError::NoFirstUser
            | StoreError::TooManyOrganizations { .. }
            | StoreError::NoRows
            | StoreError::UserNotFound(_)
            | StoreError::NotAllFound { .. }
            | StoreError::InvalidBlock { .. }
            | StoreError::InvalidBody { .. }
            | StoreError::Block(_)
            | StoreError::InvalidContainer(_)
            | StoreError::InvalidContainersAndBlocks(_)
            | StoreError::BlockNotInDeletedContainers { .. }
            | StoreError::InvalidCategory(_)
            | StoreError::InvalidSubscription(_)
            | StoreError::Refused(_)
            | StoreError::InvalidId(_)
            | StoreError::Conflict(_)
            | StoreError::NoLongerWorksIn(_)
            | StoreError::NotRestorable { .. }
            | StoreError::Stale { .. }
            | StoreError::DocumentWritten { .. } => Self::Refused,
        }
    }
}

/// A call being watched, from its start. Dropped before it is finished, it tells the
/// call as [`Outcome::Abandoned`].
pub(crate) struct Watch<'a> {
    observer: &'a dyn Observer,
    db: DbType,
    /// What the call is known to be when it starts: a transaction writes, a call on
    /// one connection reads until a statement writes.
    access: Access,
    /// Set by the call's scope when a statement that may write starts
    /// ([`Scope::reporting_writes_to`]): a call on one connection given up after that wrote.
    writes: AtomicBool,
    started: Instant,
    finished: bool,
}

impl<'a> Watch<'a> {
    /// Starts watching a call on a database of `db`, if there is an observer.
    pub(crate) fn start(
        observer: Option<&'a dyn Observer>,
        db: DbType,
        access: Access,
    ) -> Option<Self> {
        observer.map(|observer| Self {
            observer,
            db,
            access,
            writes: AtomicBool::new(false),
            started: Instant::now(),
            finished: false,
        })
    }

    /// Takes a connection from `pool`, and tells the observer how long that took, if
    /// there is one: also if no connection came.
    pub(crate) async fn acquire<DB: sqlx::Database>(
        watch: Option<&Self>,
        pool: &sqlx::Pool<DB>,
    ) -> Result<PoolConnection<DB>, sqlx::Error> {
        let Some(watch) = watch else {
            return pool.acquire().await;
        };
        let _waiting = Waiting {
            watch,
            asked: Instant::now(),
        };
        pool.acquire().await
    }

    /// What the call's scope sets when a statement that may write starts.
    pub(crate) fn writes(&self) -> &AtomicBool {
        &self.writes
    }

    /// Tells the observer how the call ended.
    pub(crate) fn finish<T>(mut self, access: Access, result: &Result<T, StoreError>) {
        self.finished = true;
        self.tell(access, Outcome::of(result));
    }

    fn tell(&self, access: Access, outcome: Outcome) {
        self.observer.called(Call {
            db: self.db,
            access,
            outcome,
            took: self.started.elapsed(),
        });
    }
}

impl Drop for Watch<'_> {
    fn drop(&mut self) {
        if !self.finished {
            let access = if self.writes.load(Ordering::Relaxed) {
                Access::Write
            } else {
                self.access
            };
            self.tell(access, Outcome::Abandoned);
        }
    }
}

/// A wait for a connection, told when it ends: when a connection came, when none could,
/// or when the call was given up while it waited.
struct Waiting<'w, 'a> {
    watch: &'w Watch<'a>,
    asked: Instant,
}

impl Drop for Waiting<'_, '_> {
    fn drop(&mut self) {
        self.watch
            .observer
            .waited_for_connection(self.watch.db, self.asked.elapsed());
    }
}
