//! The built-in templates of a server whose file storage did not take their files.
//!
//! A server starts without the templates then, and owes them: it stores them when the
//! storage takes the files again. It tries when somebody would have been shown them,
//! and every [`RESTOCK_INTERVAL`]. Until then the templates that are listed are those
//! the database holds: none on a new one, the older ones on one that had them.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::time::Instant;

use super::App;
use super::templates::NotStored;
use crate::error::AppError;
use crate::scheduler::RecurringTask;

/// How long after one attempt to store the owed templates the recurring task makes the
/// next.
const RESTOCK_INTERVAL: Duration = Duration::from_secs(60);

/// How long after an attempt a request that would have shown the templates starts the
/// next: sooner than the recurring task, which would otherwise have tried by then
/// every time, and not with every request.
const RESTOCK_FOR_A_REQUEST: Duration = Duration::from_secs(10);

/// Whether the built-in templates are owed, and when storing them was last tried.
#[derive(Debug, Default)]
pub(super) struct TemplateStock {
    /// Set by a start whose file storage did not take the files, and cleared once the
    /// templates are stored or found current.
    owed: AtomicBool,
    /// Held by the attempt under way. When the last one that stored none ended.
    attempted: tokio::sync::Mutex<Option<Instant>>,
}

impl TemplateStock {
    /// The templates are stored, or were found current: nothing is owed.
    pub(super) fn settle(&self) {
        self.owed.store(false, Ordering::Release);
    }

    fn is_owed(&self) -> bool {
        self.owed.load(Ordering::Acquire)
    }
}

impl App {
    /// Stores the built-in templates as the server starts ([`App::init_templates`]).
    /// A file storage that does not take their files does not keep the server from
    /// starting: the templates are owed, and stored later
    /// ([`App::restock_templates`]).
    ///
    /// # Errors
    ///
    /// Returns what [`App::init_templates`] does, except where the file storage did
    /// not take a file.
    pub async fn stock_templates_at_start(&self) -> Result<(), AppError> {
        match self.try_init_templates().await {
            Ok(_) => Ok(()),
            Err(NotStored::Files(error)) => {
                tracing::warn!(
                    %error,
                    "the file storage did not take the files of the built-in templates: \
                     the server starts without them and tries again later"
                );
                self.inner
                    .template_stock
                    .owed
                    .store(true, Ordering::Release);
                Ok(())
            }
            Err(NotStored::Other(error)) => Err(error),
        }
    }

    /// Whether the built-in templates are owed: a start did not store them, and no
    /// attempt since has.
    pub fn owes_templates(&self) -> bool {
        self.inner.template_stock.is_owed()
    }

    /// Stores the built-in templates if they are owed, after any attempt under way.
    /// Returns whether nothing is owed any more.
    pub async fn restock_templates(&self) -> bool {
        self.restock(Duration::ZERO).await
    }

    /// Starts an attempt to store the owed templates in the background, for a request
    /// that would have shown them, unless one is under way or ended less than
    /// [`RESTOCK_FOR_A_REQUEST`] ago: a request does not wait for the file storage.
    pub(super) fn restock_templates_soon(&self) {
        let stock = &self.inner.template_stock;
        if !stock.is_owed() {
            return;
        }
        let due = stock.attempted.try_lock().is_ok_and(|attempted| {
            attempted.is_none_or(|ended| ended.elapsed() >= RESTOCK_FOR_A_REQUEST)
        });
        if due {
            let app = self.clone();
            self.spawn_background(async move {
                app.restock(RESTOCK_FOR_A_REQUEST).await;
            });
        }
    }

    /// [`App::restock_templates`], unless an attempt ended less than `no_sooner` ago.
    async fn restock(&self, no_sooner: Duration) -> bool {
        let stock = &self.inner.template_stock;
        if !stock.is_owed() {
            return true;
        }
        let mut attempted = stock.attempted.lock().await;
        // Again, under the lock: the attempt this one waited for may have stored them.
        if !stock.is_owed() {
            return true;
        }
        if attempted.is_some_and(|ended| ended.elapsed() < no_sooner) {
            return false;
        }
        match self.init_templates().await {
            Ok(_) => {
                tracing::info!("the built-in templates that were owed are stored");
                true
            }
            Err(error) => {
                // From its end: a storage that takes long to refuse is not asked
                // again the moment it has.
                *attempted = Some(Instant::now());
                tracing::warn!(%error, "the built-in templates are still owed");
                false
            }
        }
    }
}

/// Starts the task that stores the built-in templates while they are owed; a run does
/// nothing otherwise.
pub(crate) fn start_restock(app: &App) -> RecurringTask {
    let app = app.clone();
    RecurringTask::spawn("restockTemplates", RESTOCK_INTERVAL, move || {
        let app = app.clone();
        async move {
            app.restock_templates().await;
        }
    })
}
