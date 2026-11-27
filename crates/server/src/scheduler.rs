//! Work that repeats at a fixed interval until it is cancelled.

use std::time::Duration;

use tokio::task::JoinHandle;
use tokio::time::{Instant, MissedTickBehavior, interval_at};
use tokio_util::sync::{CancellationToken, DropGuard};

/// A piece of work that runs again and again, one interval apart, in a task of its own.
///
/// Dropping the handle stops the task after the run that is under way;
/// [`RecurringTask::cancel`] also waits for that.
#[derive(Debug)]
pub struct RecurringTask {
    name: &'static str,
    interval: Duration,
    /// Cancels the task when the handle goes away.
    stop: DropGuard,
    running: JoinHandle<()>,
}

impl RecurringTask {
    /// Starts running `work` every `interval`, for the first time one interval from now.
    ///
    /// A run that takes longer than the interval makes the task skip the runs it missed
    /// rather than catch up with them.
    ///
    /// # Panics
    ///
    /// Panics if `interval` is zero, or if called outside a Tokio runtime.
    pub fn spawn<Work, Run>(name: &'static str, interval: Duration, work: Work) -> Self
    where
        Work: FnMut() -> Run + Send + 'static,
        Run: Future<Output = ()> + Send,
    {
        Self::spawn_from(name, interval, Instant::now() + interval, work)
    }

    /// Starts running `work` every `interval`, for the first time right away: for
    /// housekeeping that a server which is restarted more often than the interval would
    /// otherwise never get to.
    ///
    /// # Panics
    ///
    /// Panics if `interval` is zero, or if called outside a Tokio runtime.
    pub fn spawn_starting_now<Work, Run>(name: &'static str, interval: Duration, work: Work) -> Self
    where
        Work: FnMut() -> Run + Send + 'static,
        Run: Future<Output = ()> + Send,
    {
        Self::spawn_from(name, interval, Instant::now(), work)
    }

    fn spawn_from<Work, Run>(
        name: &'static str,
        interval: Duration,
        first: Instant,
        mut work: Work,
    ) -> Self
    where
        Work: FnMut() -> Run + Send + 'static,
        Run: Future<Output = ()> + Send,
    {
        let stop = CancellationToken::new();
        let stopped = stop.clone();
        let running = tokio::spawn(async move {
            let mut ticker = interval_at(first, interval);
            ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    biased;
                    () = stopped.cancelled() => return,
                    _ = ticker.tick() => work().await,
                }
            }
        });
        Self {
            name,
            interval,
            stop: stop.drop_guard(),
            running,
        }
    }

    /// The name the task was given.
    pub fn name(&self) -> &str {
        self.name
    }

    /// The time between two runs.
    pub fn interval(&self) -> Duration {
        self.interval
    }

    /// Stops the task and waits until it has stopped. A run that is under way finishes
    /// first; no further run starts.
    pub async fn cancel(self) {
        let Self {
            name,
            stop,
            running,
            ..
        } = self;
        drop(stop);
        if let Err(error) = running.await {
            tracing::error!(task = name, %error, "a recurring task ended abnormally");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    use tokio::time::sleep;

    use super::*;

    /// A task that counts its runs.
    fn counting_task(name: &'static str, interval: Duration) -> (RecurringTask, Arc<AtomicU32>) {
        let count = Arc::new(AtomicU32::new(0));
        let counted = Arc::clone(&count);
        let task = RecurringTask::spawn(name, interval, move || {
            let counted = Arc::clone(&counted);
            async move {
                counted.fetch_add(1, Ordering::SeqCst);
            }
        });
        (task, count)
    }

    /// A recurring task runs at each interval and stops running once cancelled.
    #[tokio::test(start_paused = true)]
    async fn create_recurring_task() {
        let interval = Duration::from_millis(500);
        let (task, count) = counting_task("Test Recurring Task", interval);
        assert_eq!(
            count.load(Ordering::SeqCst),
            0,
            "no run before the interval"
        );

        sleep(interval + Duration::from_millis(200)).await;
        assert_eq!(
            count.load(Ordering::SeqCst),
            1,
            "one run after one interval"
        );

        sleep(interval).await;
        assert_eq!(
            count.load(Ordering::SeqCst),
            2,
            "two runs after two intervals"
        );
        assert_eq!(task.name(), "Test Recurring Task");
        assert_eq!(task.interval(), interval);

        task.cancel().await;
        sleep(interval * 10).await;
        assert_eq!(count.load(Ordering::SeqCst), 2, "no run after cancelling");
    }

    /// A recurring task cancelled before its first run never runs.
    #[tokio::test(start_paused = true)]
    async fn cancel_task() {
        let interval = Duration::from_millis(100);
        let (task, count) = counting_task("Test Task", interval);
        assert_eq!(
            count.load(Ordering::SeqCst),
            0,
            "no run before the interval"
        );

        task.cancel().await;

        sleep(interval * 2).await;
        assert_eq!(
            count.load(Ordering::SeqCst),
            0,
            "a cancelled task never runs"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn dropping_the_handle_stops_the_task() {
        let interval = Duration::from_millis(100);
        let (task, count) = counting_task("Dropped Task", interval);
        sleep(interval + Duration::from_millis(50)).await;
        assert_eq!(
            count.load(Ordering::SeqCst),
            1,
            "one run after one interval"
        );

        drop(task);

        sleep(interval * 10).await;
        assert_eq!(count.load(Ordering::SeqCst), 1, "no run after the drop");
    }

    /// Cancelling waits for the run that is under way instead of cutting it short.
    #[tokio::test(start_paused = true)]
    async fn cancel_lets_the_current_run_finish() {
        let interval = Duration::from_millis(100);
        let finished = Arc::new(AtomicU32::new(0));
        let counted = Arc::clone(&finished);
        let task = RecurringTask::spawn("Slow Task", interval, move || {
            let counted = Arc::clone(&counted);
            async move {
                sleep(Duration::from_millis(250)).await;
                counted.fetch_add(1, Ordering::SeqCst);
            }
        });
        // The first run starts at 100 ms and is in its sleep at 150 ms.
        sleep(Duration::from_millis(150)).await;
        assert_eq!(finished.load(Ordering::SeqCst), 0, "the run is under way");

        task.cancel().await;

        assert_eq!(finished.load(Ordering::SeqCst), 1, "the run finished");
    }
}
