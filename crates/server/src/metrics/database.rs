//! The metrics of the calls of the database: how long each took, how many failed, and
//! how long they waited for a connection of the pool. The store tells the logic, which
//! counts here (`app::store_metrics`); this module knows no store.

use std::time::Duration;

use prometheus::{HistogramOpts, HistogramVec, IntCounterVec, Opts, Registry};

use super::{Metrics, PREFIX};

/// The kind of database a call was made on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Database {
    Sqlite,
    Postgres,
}

impl Database {
    /// The value of the `db` label: the name of the `database_kind` setting.
    fn as_str(self) -> &'static str {
        match self {
            Self::Sqlite => "sqlite",
            Self::Postgres => "postgres",
        }
    }
}

/// Whether a call of the database wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DbAccess {
    Read,
    Write,
}

impl DbAccess {
    /// The value of the `access` label.
    fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
        }
    }
}

/// Whether a call of the database got an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DbOutcome {
    /// It did, whatever the answer was: done, or refused (not there, taken, invalid).
    Answered,
    /// It did not: the database failed it.
    Failed,
    /// It was given up first: by a scrape's or a readiness check's timeout, or with
    /// its task (a websocket's, the server's shutdown). A request's handler is run to
    /// its end however its client goes.
    Abandoned,
}

/// The upper bounds of the buckets of the durations of calls, in seconds: from a read
/// of one row to a large import in one transaction.
const CALL_BUCKETS: [f64; 14] = [
    0.0005, 0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
];

/// The upper bounds of the buckets of the waits for a connection, in seconds: none to
/// speak of while the pool has an idle one, up to the pool's timeout when it has none.
const WAIT_BUCKETS: [f64; 12] = [
    0.0001, 0.0005, 0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0, 10.0, 30.0,
];

/// The metrics of the calls of the database.
#[derive(Debug)]
pub(super) struct DatabaseMetrics {
    calls: HistogramVec,
    failures: IntCounterVec,
    pool_wait: HistogramVec,
}

impl DatabaseMetrics {
    /// Makes the metrics and registers them in `registry`.
    pub(super) fn register(registry: &Registry) -> prometheus::Result<Self> {
        let calls = HistogramVec::new(
            HistogramOpts::new(
                "db_call_duration_seconds",
                "How long calls of the database took, each one transaction (or one \
                 connection for a read on SQLite) from its start to its end, the wait for \
                 a connection and on SQLite a writer's turn included, and also of the \
                 calls given up (a scrape's or a readiness check's timeout, a shutdown); \
                 by db (sqlite, postgres) and access \
                 (read, write: a transaction that may write, or a call in which a \
                 statement that may write started, also one given up); its count is \
                 the number of calls.",
            )
            .namespace(PREFIX)
            .buckets(CALL_BUCKETS.to_vec()),
            &["db", "access"],
        )?;
        let failures = IntCounterVec::new(
            Opts::new(
                "db_call_failures_total",
                "How many calls of the database failed: no connection came, the database \
                 gave an error or what cannot be read, or the server made the call \
                 without the organization it needs; an answer such as a record that is \
                 not there is no failure. By db and access (read also for a call that \
                 failed before its statements ran). The log says what.",
            )
            .namespace(PREFIX),
            &["db", "access"],
        )?;
        let pool_wait = HistogramVec::new(
            HistogramOpts::new(
                "db_pool_wait_seconds",
                "How long calls of the database waited for a connection of the pool, \
                 also those that got none, by db.",
            )
            .namespace(PREFIX)
            .buckets(WAIT_BUCKETS.to_vec()),
            &["db"],
        )?;
        registry.register(Box::new(calls.clone()))?;
        registry.register(Box::new(failures.clone()))?;
        registry.register(Box::new(pool_wait.clone()))?;
        Ok(Self {
            calls,
            failures,
            pool_wait,
        })
    }
}

impl Metrics {
    /// Makes the count of the failed calls on a database of `db`, at zero if it is not
    /// there yet, so that the first failure is an increase.
    pub(crate) fn db_known(&self, db: Database) {
        if let Some(kept) = &self.kept {
            for access in [DbAccess::Read, DbAccess::Write] {
                kept.database
                    .failures
                    .with_label_values(&[db.as_str(), access.as_str()]);
            }
        }
    }

    /// Measures a call of the database that took `took`, and counts it if it failed.
    pub(crate) fn db_called(
        &self,
        db: Database,
        access: DbAccess,
        outcome: DbOutcome,
        took: Duration,
    ) {
        let Some(kept) = &self.kept else {
            return;
        };
        let labels = [db.as_str(), access.as_str()];
        kept.database
            .calls
            .with_label_values(&labels)
            .observe(took.as_secs_f64());
        if outcome == DbOutcome::Failed {
            kept.database.failures.with_label_values(&labels).inc();
        }
    }

    /// Measures how long a call waited for a connection of the pool.
    pub(crate) fn db_waited(&self, db: Database, waited: Duration) {
        if let Some(kept) = &self.kept {
            kept.database
                .pool_wait
                .with_label_values(&[db.as_str()])
                .observe(waited.as_secs_f64());
        }
    }
}
