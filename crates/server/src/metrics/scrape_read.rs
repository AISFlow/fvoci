//! A value a scrape reads by asking the database: one read at a time, held until the
//! database has answered, and no longer awaited than a scrape may take.

use std::fmt::Display;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use prometheus::{IntCounter, Opts, Registry};
use tokio::time::Instant;

use super::PREFIX;

/// How long a scrape waits for the database to answer one read. Shorter than
/// Prometheus's own timeout of a scrape (10 s by default), so that a pool that is
/// exhausted, which the pool's metrics are there to show, does not cost the scrape.
const READ_TIMEOUT: Duration = Duration::from_secs(2);

/// The reads of one value by the scrapes, and how many of them failed.
#[derive(Debug)]
pub(super) struct ScrapeRead {
    /// Held while a read is under way, until the database has answered, also after the
    /// scrape that started it stopped waiting: one read at a time, whatever the scrapes
    /// do. A scrape that finds it held keeps the value read before.
    reading: Arc<tokio::sync::Mutex<()>>,
    /// How long a value read stays fresh: a scrape before then reads nothing.
    fresh_for: Duration,
    /// When the last read started.
    last_read: Mutex<Option<Instant>>,
    failures: IntCounter,
}

impl ScrapeRead {
    /// Makes the counter of the failed reads, `name` with `help`, and registers it. A
    /// value read is read again by the first scrape after `fresh_for`.
    pub(super) fn register(
        registry: &Registry,
        name: &str,
        help: &str,
        fresh_for: Duration,
    ) -> prometheus::Result<Self> {
        let failures = IntCounter::with_opts(Opts::new(name, help).namespace(PREFIX))?;
        registry.register(Box::new(failures.clone()))?;
        Ok(Self {
            reading: Arc::new(tokio::sync::Mutex::new(())),
            fresh_for,
            last_read: Mutex::new(None),
            failures,
        })
    }

    /// Reads with `read` and gives what it answers to `keep`, waiting two seconds at
    /// most. Nothing is read if another read is under way or the value is fresh. A read
    /// that fails, or does not answer in time, is counted and logged as a failure to
    /// read `what` (once); a read that answers late is kept when it answers.
    pub(super) async fn read<T, E>(
        &self,
        what: &'static str,
        read: impl Future<Output = Result<T, E>> + Send + 'static,
        keep: impl FnOnce(T) + Send + 'static,
    ) where
        T: Send + 'static,
        E: Display + Send + 'static,
    {
        let Ok(reading) = Arc::clone(&self.reading).try_lock_owned() else {
            return;
        };
        let started = Instant::now();
        {
            let mut last_read = self.last_read.lock().expect("lock poisoned");
            if last_read.is_some_and(|last| started.duration_since(last) < self.fresh_for) {
                return;
            }
            *last_read = Some(started);
        }
        let failures = self.failures.clone();
        let task = tokio::spawn(async move {
            let _reading = reading;
            match read.await {
                Ok(value) => keep(value),
                // Past the timeout, the scrape has counted it already.
                Err(error) if started.elapsed() < READ_TIMEOUT => {
                    failures.inc();
                    tracing::warn!(%error, what, "a scrape could not read the database");
                }
                Err(error) => {
                    tracing::warn!(%error, what, "a read of the database failed after its scrape");
                }
            }
        });
        if tokio::time::timeout(READ_TIMEOUT, task).await.is_err() {
            self.failures.inc();
            tracing::warn!(what, "a scrape could not read the database in time");
        }
    }

    /// How many reads failed, for tests.
    #[cfg(test)]
    fn failures(&self) -> u64 {
        self.failures.get()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    fn scrape_read(fresh_for: Duration) -> ScrapeRead {
        ScrapeRead::register(&Registry::new(), "read_failures_total", "help", fresh_for)
            .expect("the counter registers")
    }

    /// A place a read keeps its value in.
    fn kept() -> (Arc<AtomicU64>, impl Fn() -> u64) {
        let value = Arc::new(AtomicU64::new(0));
        let read = Arc::clone(&value);
        (value, move || read.load(Ordering::SeqCst))
    }

    fn keep_in(place: &Arc<AtomicU64>) -> impl FnOnce(u64) + Send + 'static {
        let place = Arc::clone(place);
        move |value| place.store(value, Ordering::SeqCst)
    }

    #[tokio::test]
    async fn a_value_is_kept_and_a_failure_counted() {
        let reads = scrape_read(Duration::ZERO);
        let (place, value) = kept();
        reads
            .read("x", async { Ok::<_, String>(7) }, keep_in(&place))
            .await;
        assert_eq!((value(), reads.failures()), (7, 0));
        reads
            .read("x", async { Err::<u64, _>("no") }, keep_in(&place))
            .await;
        assert_eq!((value(), reads.failures()), (7, 1), "the value read before");
    }

    #[tokio::test(start_paused = true)]
    async fn a_late_read_is_one_failure_holds_its_turn_and_is_kept_when_it_answers() {
        let reads = scrape_read(Duration::ZERO);
        let (place, value) = kept();
        let (answer, answered) = tokio::sync::oneshot::channel::<u64>();
        let late = async move { answered.await.map_err(|_| "dropped") };
        reads.read("x", late, keep_in(&place)).await;
        assert_eq!(reads.failures(), 1, "not in time");

        // Under way still: another scrape reads nothing, and does not poll its read.
        let polled = Arc::new(AtomicU64::new(0));
        let counted = Arc::clone(&polled);
        let other = async move {
            counted.fetch_add(1, Ordering::SeqCst);
            Ok::<u64, String>(9)
        };
        reads.read("x", other, keep_in(&place)).await;
        assert_eq!(polled.load(Ordering::SeqCst), 0, "not polled");
        assert_eq!((value(), reads.failures()), (0, 1), "nothing counted");

        answer.send(5).expect("the read waits");
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;
        assert_eq!(value(), 5, "kept when it answers");
        reads
            .read("x", async { Ok::<_, String>(6) }, keep_in(&place))
            .await;
        assert_eq!(value(), 6, "the turn is free again");
    }

    #[tokio::test(start_paused = true)]
    async fn a_fresh_value_is_not_read_again() {
        let reads = scrape_read(Duration::from_secs(60));
        let (place, value) = kept();
        reads
            .read("x", async { Ok::<_, String>(1) }, keep_in(&place))
            .await;
        reads
            .read("x", async { Ok::<_, String>(2) }, keep_in(&place))
            .await;
        assert_eq!(value(), 1, "fresh");
        tokio::time::advance(Duration::from_secs(61)).await;
        reads
            .read("x", async { Ok::<_, String>(3) }, keep_in(&place))
            .await;
        assert_eq!(value(), 3, "read again");
    }
}
