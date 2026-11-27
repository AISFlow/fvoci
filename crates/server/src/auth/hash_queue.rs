//! The queue of password work.
//!
//! Hashing or checking a password takes 19 MiB and some milliseconds of a processor, on
//! purpose. A bounded number run at a time, a bounded number wait their turn, and nobody
//! waits long: what does not fit is told to come back, at once, without a hash being
//! spent on it. Without the bounds, a burst of logins would take a processor each, and
//! the memory to go with it, and every real login would wait behind it.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::sync::Semaphore;

/// Why password work was not done.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueError {
    /// Too many are waiting, or the wait was too long: try again after this long.
    Full(Duration),
    /// The work was started and did not finish: the task that ran it failed.
    Failed,
}

/// Runs password work off the async runtime, a bounded number at a time.
#[derive(Debug, Clone)]
pub struct HashQueue {
    permits: Arc<Semaphore>,
    waiting: Arc<AtomicUsize>,
    max_waiting: usize,
    max_wait: Duration,
}

impl HashQueue {
    /// A queue that runs `concurrency` pieces of work at a time, lets `max_waiting`
    /// more wait for their turn, and none of them for longer than `max_wait`.
    pub fn new(concurrency: usize, max_waiting: usize, max_wait: Duration) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(concurrency.max(1))),
            waiting: Arc::new(AtomicUsize::new(0)),
            max_waiting,
            max_wait,
        }
    }

    /// Runs `work` on the blocking pool when its turn comes.
    ///
    /// The turn is held until the work is done, also if the caller stops waiting for
    /// the result: work that was started still takes its memory and its processor.
    ///
    /// # Errors
    ///
    /// Returns [`QueueError::Full`] without running the work if too many are waiting
    /// already or the turn does not come in time.
    pub async fn run<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Result<T, QueueError> {
        let permit = if let Ok(permit) = Arc::clone(&self.permits).try_acquire_owned() {
            permit
        } else {
            let waiting = Waiting::join(&self.waiting, self.max_waiting)
                .ok_or(QueueError::Full(self.max_wait))?;
            let turn = Arc::clone(&self.permits).acquire_owned();
            let permit = tokio::time::timeout(self.max_wait, turn)
                .await
                .map_err(|_| QueueError::Full(self.max_wait))?
                .map_err(|_| QueueError::Failed)?;
            drop(waiting);
            permit
        };
        tokio::task::spawn_blocking(move || {
            let result = work();
            drop(permit);
            result
        })
        .await
        .map_err(|error| {
            // The task panicked or was cancelled; the caller answers a 500 and this is
            // the only place that knows why.
            tracing::error!(%error, "password work did not finish");
            QueueError::Failed
        })
    }
}

/// A place among those who wait; given up when dropped.
struct Waiting<'q>(&'q AtomicUsize);

impl<'q> Waiting<'q> {
    /// Takes a place unless `max` are taken.
    fn join(waiting: &'q AtomicUsize, max: usize) -> Option<Self> {
        waiting
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |count| {
                (count < max).then_some(count + 1)
            })
            .ok()
            .map(|_| Self(waiting))
    }
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    /// Work that waits until the test lets it go.
    fn held() -> (impl FnOnce() -> u8 + Send + 'static, mpsc::Sender<()>) {
        let (release, wait) = mpsc::channel::<()>();
        (
            move || {
                let _released = wait.recv();
                7
            },
            release,
        )
    }

    #[tokio::test]
    async fn work_runs_and_its_result_comes_back() {
        let queue = HashQueue::new(2, 2, Duration::from_secs(5));
        assert_eq!(queue.run(|| 1 + 1).await, Ok(2));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn what_does_not_fit_is_refused_at_once_and_what_waits_too_long_is_refused() {
        let queue = HashQueue::new(1, 1, Duration::from_millis(200));
        let (running_work, release_running) = held();
        let running = tokio::spawn({
            let queue = queue.clone();
            async move { queue.run(running_work).await }
        });
        // Until the one turn is taken.
        while queue.permits.available_permits() > 0 {
            tokio::task::yield_now().await;
        }

        // One may wait; it gives up after the longest wait.
        let waiting = tokio::spawn({
            let queue = queue.clone();
            async move { queue.run(|| 1).await }
        });
        while queue.waiting.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        // No place left to wait in: refused without waiting.
        let started = std::time::Instant::now();
        assert_eq!(
            queue.run(|| 2).await,
            Err(QueueError::Full(Duration::from_millis(200)))
        );
        assert!(
            started.elapsed() < Duration::from_millis(150),
            "refused at once, after {:?}",
            started.elapsed()
        );
        assert_eq!(
            waiting.await.expect("the waiting task ends"),
            Err(QueueError::Full(Duration::from_millis(200))),
            "its turn did not come in time"
        );
        assert_eq!(queue.waiting.load(Ordering::SeqCst), 0, "nobody waits now");

        release_running.send(()).expect("the work is still there");
        assert_eq!(running.await.expect("the task ends"), Ok(7));
        assert_eq!(queue.run(|| 3).await, Ok(3), "there is room again");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_turn_is_held_until_the_work_is_done_even_if_nobody_waits_for_it() {
        let queue = HashQueue::new(1, 0, Duration::from_millis(50));
        let (work, release) = held();
        let caller = tokio::spawn({
            let queue = queue.clone();
            async move { queue.run(work).await }
        });
        while queue.permits.available_permits() > 0 {
            tokio::task::yield_now().await;
        }
        caller.abort();
        let _aborted = caller.await;

        assert!(
            matches!(queue.run(|| 1).await, Err(QueueError::Full(_))),
            "the abandoned work still has the turn"
        );
        release.send(()).expect("the work is still there");
        while queue.permits.available_permits() == 0 {
            tokio::task::yield_now().await;
        }
        assert_eq!(queue.run(|| 1).await, Ok(1));
    }
}
