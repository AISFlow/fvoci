//! The search of what cards and pages say, through a search server that is set up apart
//! (`search_url`): Elasticsearch or `OpenSearch`, with the nori analysis plugin.
//!
//! The browser never reaches the search server. The server keeps one index there; a
//! search asks it for documents of the user's workspace, and then asks the database
//! about every document that comes back (`App::search_workspace`): the index may be
//! behind what is stored, and what it says is never an answer by itself.
//!
//! What changes is sent after it is stored: the store notes every change that a
//! search would find differently, in the transaction that makes it, and the upkeep
//! ([`tend`]) sends what is stored of what was noted, with the reach of the
//! housekeeping, as the one server of its database.
//!
//! Without `search_url` the database is searched instead (`database`): by the
//! letters of the titles and the bodies, behind the same API and the same check, and
//! nothing is noted. A search server that is configured is never stood in for: while
//! it does not answer, and where it cannot hold the index, a search is a 503.

mod client;
mod database;
mod document;
mod find;
mod index;
mod limit;
mod query;
mod snippet;
mod upkeep;
mod version;
mod worker;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::time::Instant;

pub use find::{Asked, AskedError, MAX_LIMIT, MAX_TEXT_CHARS};
pub use upkeep::{reindex, tend};
pub(crate) use upkeep::{settle_at_start, start_upkeep, stop_upkeep};

use client::{Client, ClientError};
use limit::Limiter;
use version::Versions;

use crate::config::search::SearchSettings;

/// How long the upkeep leaves a search server that did not answer alone, at first
/// and at most; doubled with every failure in a row.
const FIRST_PAUSE: Duration = Duration::from_secs(1);
/// See [`FIRST_PAUSE`].
const LONGEST_PAUSE: Duration = Duration::from_secs(60);
/// How many searches of the database run at once, of all users: each holds a
/// connection of the pool while its statement runs, and the pool serves everything
/// else too.
const DATABASE_SEARCHES_AT_ONCE: usize = 4;

/// How often the log says that searches are not answered, while they are not for the
/// same reason.
const FAILURES_SAID_EVERY: Duration = Duration::from_secs(30);

/// The search of bodies, as the server is configured: on a search server, or in the
/// database.
#[derive(Debug)]
pub struct Search {
    /// `None` where no search server is configured: the database is searched.
    engine: Option<Engine>,
    /// How often one user may search, either way.
    limiter: Limiter,
    /// How long a search may take, all its pages and checks together.
    timeout: Duration,
    /// The searches of the database that may run at once ([`DATABASE_SEARCHES_AT_ONCE`]).
    database_searches: Arc<tokio::sync::Semaphore>,
}

/// Where a search is asked.
#[derive(Debug, Clone, Copy)]
enum Way<'a> {
    /// The search server.
    Engine(&'a Engine),
    /// The database, for a server without a search server.
    Database,
}

/// The search server and what is known of the index on it.
#[derive(Debug)]
pub(crate) struct Engine {
    client: Client,
    index: Mutex<IndexState>,
    /// Held by the upkeep while it runs: one at a time.
    upkeep: tokio::sync::Mutex<Pause>,
    /// The versions documents are written with.
    versions: Versions,
    /// When a search that the search server did not answer was last said in the log,
    /// and why it was not answered.
    search_failure_said: Mutex<Option<(Instant, ClientError)>>,
}

/// What is known of the index on the search server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IndexState {
    /// Nothing yet: the search server has not answered.
    Unknown,
    /// It is there, made from this server's definition.
    Ready,
    /// The search server cannot hold it. Bodies are not searched until the server is
    /// started again.
    Refused,
}

/// How long the upkeep leaves the search server alone after it failed to answer.
#[derive(Debug, Default)]
struct Pause {
    until: Option<Instant>,
    failures: u32,
    /// Why the last run failed.
    last: Option<ClientError>,
    /// How many runs in a row found nothing to send.
    idle_runs: u32,
}

impl Pause {
    fn is_over(&self, now: Instant) -> bool {
        self.until.is_none_or(|until| now >= until)
    }

    /// Notes a failure at `now`. Returns whether it is news: the first in a row, or
    /// another failure than the one before.
    fn failed(&mut self, now: Instant, error: ClientError) -> bool {
        self.wait(now);
        let news = self.last.as_ref() != Some(&error);
        self.last = Some(error);
        news
    }

    /// Leaves the search alone for a while from `now`, longer each time in a row: for
    /// what failed otherwise than by the search server.
    fn wait(&mut self, now: Instant) {
        let pause = FIRST_PAUSE
            .saturating_mul(2_u32.saturating_pow(self.failures))
            .min(LONGEST_PAUSE);
        self.until = Some(now + pause);
        self.failures = self.failures.saturating_add(1);
    }
}

impl Search {
    /// The search the configuration asks for.
    ///
    /// # Errors
    ///
    /// Returns an error if the keys of the search server do not hold
    /// ([`SearchSettings::server`]) or its client cannot be set up.
    pub fn from_config(settings: &SearchSettings) -> anyhow::Result<Self> {
        let engine = match settings.server()? {
            Some(server) => Some(Engine {
                client: Client::new(&server)?,
                index: Mutex::new(IndexState::Unknown),
                upkeep: tokio::sync::Mutex::default(),
                versions: Versions::default(),
                search_failure_said: Mutex::new(None),
            }),
            None => None,
        };
        Ok(Self {
            engine,
            limiter: Limiter::new(settings.search_requests_per_second),
            timeout: settings.timeout,
            database_searches: Arc::new(tokio::sync::Semaphore::new(DATABASE_SEARCHES_AT_ONCE)),
        })
    }

    /// Whether the configuration names a search server, whatever became of it.
    pub(crate) fn configured(&self) -> bool {
        self.engine.is_some()
    }

    /// The search server, if bodies can be searched there.
    fn engine(&self) -> Option<&Engine> {
        self.engine
            .as_ref()
            .filter(|engine| engine.index_state() != IndexState::Refused)
    }

    /// Where a search is asked: the search server that is configured, or the database
    /// where none is. `None` for a search server that cannot hold the index: then every
    /// search is a 503, as while it does not answer, and the database does not stand in
    /// for it.
    fn way(&self) -> Option<Way<'_>> {
        match &self.engine {
            None => Some(Way::Database),
            Some(_) => self.engine().map(Way::Engine),
        }
    }
}

impl Engine {
    pub(super) fn index_state(&self) -> IndexState {
        *self
            .index
            .lock()
            .expect("the lock of the index state is not poisoned")
    }

    /// Whether a search that failed at `now` for `error` is to be said in the log: the
    /// first, one that failed otherwise than the one said last, and then one every
    /// [`FAILURES_SAID_EVERY`].
    pub(super) fn search_failure_is_news(&self, now: Instant, error: &ClientError) -> bool {
        let mut said = self
            .search_failure_said
            .lock()
            .expect("the lock of the failures said is not poisoned");
        let news = said.as_ref().is_none_or(|(at, last)| {
            last != error || now.duration_since(*at) >= FAILURES_SAID_EVERY
        });
        if news {
            *said = Some((now, error.clone()));
        }
        news
    }

    /// Notes that the index turned out not to be there: the upkeep looks for it
    /// again. A search that was turned off stays off.
    pub(super) fn lose_index(&self) {
        let mut state = self
            .index
            .lock()
            .expect("the lock of the index state is not poisoned");
        if *state == IndexState::Ready {
            *state = IndexState::Unknown;
        }
    }

    pub(super) fn set_index_state(&self, state: IndexState) {
        *self
            .index
            .lock()
            .expect("the lock of the index state is not poisoned") = state;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_server_that_does_not_answer_is_left_alone_twice_as_long_each_time() {
        let start = Instant::now();
        let down = || ClientError::Unreachable(client::NoAnswer::Connection);
        let mut pause = Pause::default();
        assert!(pause.is_over(start));
        assert!(pause.failed(start, down()), "the first of a row");
        assert!(!pause.is_over(start));
        assert!(pause.is_over(start + FIRST_PAUSE));
        assert!(!pause.failed(start, down()), "the same again is no news");
        assert!(!pause.is_over(start + FIRST_PAUSE));
        assert!(pause.is_over(start + FIRST_PAUSE * 2));
        assert!(
            pause.failed(start, ClientError::Unreadable),
            "another failure"
        );
        for _ in 0..40 {
            pause.failed(start, down());
        }
        assert!(pause.is_over(start + LONGEST_PAUSE), "never longer");
    }

    #[test]
    fn a_failed_search_is_said_at_once_when_it_failed_otherwise() {
        let settings = SearchSettings {
            search_url: "http://127.0.0.1:9".to_owned(),
            ..SearchSettings::default()
        };
        let search = Search::from_config(&settings).expect("a search server");
        let engine = search.engine.as_ref().expect("configured");
        let start = Instant::now();
        let down = ClientError::Unreachable(client::NoAnswer::Connection);
        assert!(engine.search_failure_is_news(start, &down), "the first");
        assert!(
            !engine.search_failure_is_news(start + Duration::from_secs(1), &down),
            "the same again"
        );
        assert!(
            engine.search_failure_is_news(start + Duration::from_secs(2), &ClientError::Partial),
            "another failure, at once"
        );
        assert!(
            engine.search_failure_is_news(start + Duration::from_secs(3), &down),
            "and back to the first"
        );
        assert!(
            !engine.search_failure_is_news(start + Duration::from_secs(32), &down),
            "the same within the time"
        );
        assert!(
            engine.search_failure_is_news(start + Duration::from_secs(33), &down),
            "the same, once the time is past"
        );
    }
}
