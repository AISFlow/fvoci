//! One process is the server of a database (`docs/configuration.md`): it holds the lock
//! of the server (`fvoci_store::ServerLock`) from before the start changes or cleans up
//! anything, and the housekeeping runs only while it holds it. A process that cannot
//! take the lock within `server_lock_wait_seconds` does not start, and says who holds
//! it; one that loses it is not ready, stops its housekeeping, and goes down.

use std::sync::{Arc, Weak};
use std::time::Duration;

use anyhow::bail;
use fvoci_store::{ServerLock, Store};
use tokio::sync::watch;
use tokio::time::Instant;

use super::App;
use crate::config::Configuration;
use crate::error::AppError;

/// How often the lock is looked at while it is held.
const CHECK_EVERY: Duration = Duration::from_secs(5);

/// How long a look at the lock may take before the lock is taken for lost.
const CHECK_TIMEOUT: Duration = Duration::from_secs(10);

/// How often a process that waits for the lock tries to take it.
const TRY_EVERY: Duration = Duration::from_millis(250);

/// The longest one try to take the lock may take, connecting included.
const TRY_TIMEOUT: Duration = Duration::from_secs(10);

/// Whether this process holds the lock of the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Holding {
    /// Not taken, or let go.
    No,
    /// Taken by this process, and not lost.
    Held,
    /// It was held and is lost: the process goes down.
    Lost,
}

/// The holding of the lock, as every handle of the application sees it.
#[derive(Debug)]
pub(super) struct Holder(watch::Sender<Holding>);

impl Default for Holder {
    fn default() -> Self {
        Self(watch::Sender::new(Holding::No))
    }
}

impl Holder {
    /// Marks the lock let go, unless it was lost.
    fn let_go(&self) {
        self.0.send_if_modified(|holding| {
            let was_held = *holding == Holding::Held;
            if was_held {
                *holding = Holding::No;
            }
            was_held
        });
    }
}

/// The lock of the server, held, and the task that keeps looking at it. Dropped, as
/// when a start fails after taking it, the lock goes and the task ends.
#[derive(Debug)]
pub(crate) struct HeldLock {
    app: App,
    lock: Option<Arc<ServerLock>>,
    watching: tokio::task::JoinHandle<()>,
}

impl HeldLock {
    /// Lets the lock go, for the next server, once a look under way ended.
    pub(crate) async fn release(mut self) {
        self.app.inner.server_lock.let_go();
        self.watching.abort();
        let _ = (&mut self.watching).await;
        if let Some(lock) = self.lock.take().and_then(|lock| Arc::try_unwrap(lock).ok()) {
            lock.release().await;
        }
    }

    /// Closes the database, then lets the lock go, last and only once no connection of
    /// the pool is out: a transaction still under way (a request or an import after the
    /// drain) could commit after the next server took over. If the pool is not closed
    /// within `timeout`, the lock is let go once it is, or goes with the process, which
    /// cuts every connection.
    pub(crate) async fn close_store_and_release(self, timeout: Duration) {
        let store = self.app.store().clone();
        if tokio::time::timeout(timeout, store.close()).await.is_ok() {
            self.release().await;
            return;
        }
        tracing::warn!(
            "connections of the database are still in use at shutdown: the lock of the \
             server goes once they are given back, or with the process"
        );
        tokio::spawn(async move {
            store.close().await;
            self.release().await;
            tracing::info!(
                "the last connection of the database was given back: the lock of the server is let go"
            );
        });
    }
}

impl Drop for HeldLock {
    fn drop(&mut self) {
        self.app.inner.server_lock.let_go();
        self.watching.abort();
    }
}

impl App {
    /// Whether this process holds the lock of the server of its database: from when a
    /// server took it at its start until it lets it go or loses it.
    pub fn holds_server_lock(&self) -> bool {
        *self.inner.server_lock.0.borrow() == Holding::Held
    }

    /// Refuses the work of the server of the database (the housekeeping, the clean-up
    /// at the start) unless this process holds the lock.
    ///
    /// # Errors
    ///
    /// Returns a 503 error if the lock is not held.
    pub(crate) fn require_server_lock(&self) -> Result<(), AppError> {
        if self.holds_server_lock() {
            Ok(())
        } else {
            Err(AppError::service_unavailable(
                "this process does not hold the lock of the server of its database",
            ))
        }
    }

    /// Returns once the lock, held, is lost; never if it is not lost.
    pub(crate) async fn server_lock_lost(&self) {
        let mut holding = self.inner.server_lock.0.subscribe();
        // The sender lives as long as the application, which outlives this future.
        let _ = holding.wait_for(|holding| *holding == Holding::Lost).await;
    }

    /// Whether the database reads the table of the sessions, and this process holds the
    /// lock of the server in it: what the readiness asks of the database.
    pub(super) async fn check_ready_as_the_server(&self) -> Result<(), String> {
        if !self.holds_server_lock() {
            return Err("this process does not hold the lock of the server".to_owned());
        }
        self.store()
            .check_ready()
            .await
            .map_err(|error| error.to_string())
    }

    /// Holds `lock`, taken with [`take_server_lock`] for this application's database,
    /// and keeps looking at it until it is let go ([`HeldLock::release`]).
    pub(crate) fn hold_server_lock(&self, lock: ServerLock) -> HeldLock {
        let lock = Arc::new(lock);
        self.inner.server_lock.0.send_replace(Holding::Held);
        let watching = tokio::spawn(watch_lock(self.clone(), Arc::downgrade(&lock)));
        HeldLock {
            app: self.clone(),
            lock: Some(lock),
            watching,
        }
    }
}

/// Takes the lock of the server of the database, trying for as long as
/// `server_lock_wait_seconds` says: a server that is restarted waits for the one that
/// is going down to let it go. A try that fails, as when the database is out of
/// connections for a moment, is tried again within the wait. Taken before the schema is
/// brought up to date, so that a start that is refused changes nothing.
///
/// # Errors
///
/// Fails if the lock cannot be taken in that time, naming the process that holds it,
/// or, with the last error, if no try reached the database.
pub(crate) async fn take_server_lock(
    store: &Store,
    config: &Configuration,
) -> anyhow::Result<ServerLock> {
    let holder = format!("fvoci-server process {}", std::process::id());
    let wait = config.server_lock_wait();
    let started = Instant::now();
    let mut said = false;
    loop {
        // Each try has its own time, also the one try of no wait.
        let tried = tokio::time::timeout(TRY_TIMEOUT, store.try_lock_server(&holder)).await;
        let error = match tried {
            Ok(Ok(Some(lock))) => return Ok(lock),
            Ok(Ok(None)) => None,
            Ok(Err(error)) => Some(anyhow::Error::from(error)),
            Err(_) => Some(anyhow::anyhow!("a try to take it did not end in time")),
        };
        if started.elapsed() >= wait {
            if let Some(error) = error {
                return Err(error.context("unable to take the lock of the server"));
            }
            let holder = match store.server_lock_holder().await {
                Ok(Some(holder)) => holder,
                Ok(None) => "a process that let it go just now".to_owned(),
                Err(error) => format!("a process that could not be named ({error})"),
            };
            bail!(
                "another process is the server of this database: {holder} holds its lock, \
                 and it was not let go in {} seconds (server_lock_wait_seconds). One server \
                 serves a database",
                wait.as_secs()
            );
        }
        if let Some(error) = &error {
            tracing::warn!(%error, "the lock of the server could not be asked for; trying again");
        } else if !said {
            tracing::warn!(
                seconds = wait.as_secs(),
                "another process holds the lock of the server of this database; waiting for it"
            );
            said = true;
        }
        tokio::time::sleep(TRY_EVERY.min(wait.saturating_sub(started.elapsed()))).await;
    }
}

/// Looks at the lock every [`CHECK_EVERY`] while it is held, and marks it lost when the
/// database says it is not held or does not answer in [`CHECK_TIMEOUT`]: a server that
/// cannot tell whether it is the server stops. Ends once the lock is let go.
async fn watch_lock(app: App, lock: Weak<ServerLock>) {
    loop {
        tokio::time::sleep(CHECK_EVERY).await;
        let Some(lock) = lock.upgrade() else {
            return;
        };
        let error = match tokio::time::timeout(CHECK_TIMEOUT, lock.is_held()).await {
            Ok(Ok(true)) => continue,
            Ok(Ok(false)) => "the database says it is not held".to_owned(),
            Ok(Err(error)) => error.to_string(),
            Err(_) => "the database did not answer in time".to_owned(),
        };
        tracing::error!(
            %error,
            "the lock of the server is lost: the housekeeping stops and the server goes down"
        );
        app.inner.server_lock.0.send_replace(Holding::Lost);
        return;
    }
}
