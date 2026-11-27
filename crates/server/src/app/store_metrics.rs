//! What the store tells about its calls, counted into the metrics: the observer the
//! server gives its store when it keeps metrics.

use std::time::Duration;

use fvoci_store::{Access, Call, DbType, Observer, Outcome, Store};

use crate::config::Configuration;
use crate::metrics::{Database, DbAccess, DbOutcome, Metrics};

/// The metrics the configuration asks for, which do nothing without `metrics_address`;
/// kept ones are told the store's database and count its calls.
///
/// # Errors
///
/// Returns an error if `metrics_address` is no address, or if the metrics cannot be kept.
pub(super) fn set_up(config: &Configuration, store: &Store) -> anyhow::Result<Metrics> {
    let metrics = if config.metrics_listen_address()?.is_some() {
        Metrics::kept()?
    } else {
        Metrics::none()
    };
    if metrics.is_kept() {
        metrics.db_known(database(store.db_type()));
        if !store.observe(Box::new(StoreMetrics(metrics.clone()))) {
            tracing::warn!("the store has an observer already; its calls are not measured");
        }
    }
    Ok(metrics)
}

/// Counts the calls of a store into the metrics.
#[derive(Debug)]
struct StoreMetrics(Metrics);

impl Observer for StoreMetrics {
    fn called(&self, call: Call) {
        let access = match call.access {
            Access::Read => DbAccess::Read,
            Access::Write => DbAccess::Write,
        };
        let outcome = match call.outcome {
            Outcome::Done | Outcome::Refused => DbOutcome::Answered,
            Outcome::Failed => DbOutcome::Failed,
            Outcome::Abandoned => DbOutcome::Abandoned,
        };
        self.0
            .db_called(database(call.db), access, outcome, call.took);
    }

    fn waited_for_connection(&self, db: DbType, waited: Duration) {
        self.0.db_waited(database(db), waited);
    }
}

/// The kind of database, as the metrics name it.
fn database(db: DbType) -> Database {
    match db {
        DbType::Sqlite => Database::Sqlite,
        DbType::Postgres => Database::Postgres,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rendered(metrics: &Metrics) -> String {
        let text = metrics
            .render()
            .expect("the metrics are kept")
            .expect("the metrics encode");
        String::from_utf8(text).expect("the text format is UTF-8")
    }

    #[test]
    fn a_failed_call_is_measured_and_counted_and_a_refused_or_abandoned_one_only_measured() {
        let metrics = Metrics::kept().expect("the metrics register");
        let observer = StoreMetrics(metrics.clone());
        let call = |access, outcome| Call {
            db: DbType::Postgres,
            access,
            outcome,
            took: Duration::from_millis(3),
        };
        observer.called(call(Access::Read, Outcome::Done));
        observer.called(call(Access::Read, Outcome::Refused));
        observer.called(call(Access::Write, Outcome::Failed));
        observer.called(call(Access::Write, Outcome::Abandoned));
        observer.waited_for_connection(DbType::Postgres, Duration::from_micros(50));
        let text = rendered(&metrics);
        for line in [
            "fvoci_db_call_duration_seconds_count{access=\"read\",db=\"postgres\"} 2",
            "fvoci_db_call_duration_seconds_count{access=\"write\",db=\"postgres\"} 2",
            "fvoci_db_call_failures_total{access=\"write\",db=\"postgres\"} 1",
            "fvoci_db_pool_wait_seconds_count{db=\"postgres\"} 1",
        ] {
            assert!(text.contains(line), "{line}: {text}");
        }
        assert!(
            !text.contains("fvoci_db_call_failures_total{access=\"read\""),
            "a refusal is an answer: {text}"
        );
    }
}
