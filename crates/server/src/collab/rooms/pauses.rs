//! Where the tests stop the rooms, to look at what happens meanwhile: built with the
//! feature `test-util` only.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{Notify, watch};

use super::super::Collab;

/// Where a room's step waits for a test.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Pause {
    /// A periodic check was not answered of somebody, and is about to ask once more.
    BeforeAskingOnceMore,
    /// A periodic check read its answers and has not applied them.
    AfterRecheck,
    /// A connection coming into a room was admitted, and the room is not held yet.
    AfterAdmission,
    /// An immediate check holds its members, and is about to read the store.
    BeforeAccessRead,
}

/// A pause: told when the step gets there, and waited for to go on.
type Waiting = (Arc<Notify>, Arc<Notify>);

/// The pauses set, each for the next step that gets there.
#[derive(Debug, Default)]
pub(super) struct Pauses {
    /// The next immediate permission read is left unanswered: told when it is asked.
    access_stall: Mutex<Option<Arc<Notify>>>,
    /// How long every immediate permission read takes besides the store's own time.
    access_delay: Mutex<Option<Duration>>,
    /// Told when the step gets there, and waited for to go on.
    at: Mutex<HashMap<Pause, Waiting>>,
    /// Counts the times a test had every room ask now: each room's periodic check
    /// asks when it changes, besides every `collab_recheck_seconds`.
    asking_now: watch::Sender<u64>,
}

impl Collab {
    /// Leaves the next immediate permission read unanswered, to exercise its deadline.
    #[doc(hidden)]
    #[must_use]
    pub fn stall_next_access_check(&self) -> Arc<Notify> {
        let waiting = Arc::new(Notify::new());
        *self
            .inner
            .rooms
            .pauses
            .access_stall
            .lock()
            .expect("the pauses are not poisoned") = Some(Arc::clone(&waiting));
        waiting
    }

    /// Has every immediate permission read take `delay` more, as a slow store would.
    #[doc(hidden)]
    pub fn slow_access_reads(&self, delay: Duration) {
        *self
            .inner
            .rooms
            .pauses
            .access_delay
            .lock()
            .expect("the pauses are not poisoned") = Some(delay);
    }

    /// Pauses the next periodic check of any room after its reads, before applying its
    /// answers: the first is told when it gets there, and it goes on when the second is.
    #[doc(hidden)]
    #[must_use]
    pub fn pause_next_recheck(&self) -> (Arc<Notify>, Arc<Notify>) {
        self.pause_next(Pause::AfterRecheck)
    }

    /// Pauses the next periodic check of any room that was not answered of somebody,
    /// before it waits to ask once more: the first is told when it gets there, and it
    /// goes on when the second is.
    #[doc(hidden)]
    #[must_use]
    pub fn pause_next_asking_once_more(&self) -> (Arc<Notify>, Arc<Notify>) {
        self.pause_next(Pause::BeforeAskingOnceMore)
    }

    /// Has the periodic check of every room ask now, as at the end of its interval: a
    /// test asks when it is ready, and leaves `collab_recheck_seconds`, which is also
    /// how long each read of a check may take, long. A room in the middle of a check
    /// asks again after it.
    #[doc(hidden)]
    pub fn ask_again_now(&self) {
        self.inner
            .rooms
            .pauses
            .asking_now
            .send_modify(|times| *times += 1);
    }

    /// Changes when a test has every room ask now ([`Self::ask_again_now`]).
    pub(super) fn asked_now(&self) -> watch::Receiver<u64> {
        self.inner.rooms.pauses.asking_now.subscribe()
    }

    /// Pauses the next connection coming into a room once it is admitted, before the
    /// room is held: the first is told when it gets there, and it goes on when the
    /// second is.
    #[doc(hidden)]
    #[must_use]
    pub fn pause_next_admission(&self) -> (Arc<Notify>, Arc<Notify>) {
        self.pause_next(Pause::AfterAdmission)
    }

    /// Pauses the next immediate permission read, once its check holds its members:
    /// the first is told when it gets there, and it goes on when the second is.
    #[doc(hidden)]
    #[must_use]
    pub fn pause_next_access_read(&self) -> (Arc<Notify>, Arc<Notify>) {
        self.pause_next(Pause::BeforeAccessRead)
    }

    /// Takes away the pauses set and not got to, so that nothing waits at them.
    #[doc(hidden)]
    pub fn unpause(&self) {
        self.inner
            .rooms
            .pauses
            .at
            .lock()
            .expect("the pauses are not poisoned")
            .clear();
    }

    fn pause_next(&self, at: Pause) -> Waiting {
        let pause = (Arc::new(Notify::new()), Arc::new(Notify::new()));
        self.inner
            .rooms
            .pauses
            .at
            .lock()
            .expect("the pauses are not poisoned")
            .insert(at, pause.clone());
        pause
    }

    /// Waits at `at` if a test paused the next step there.
    pub(super) async fn paused(&self, at: Pause) {
        let pause = self
            .inner
            .rooms
            .pauses
            .at
            .lock()
            .expect("the pauses are not poisoned")
            .remove(&at);
        if let Some((waiting, go)) = pause {
            waiting.notify_one();
            go.notified().await;
        }
    }

    /// Before an immediate permission read: waits if a test paused the next one, never
    /// answers if a test stalled it, and waits as long as a test slowed every one.
    pub(super) async fn before_access_read(&self) {
        self.paused(Pause::BeforeAccessRead).await;
        let pauses = &self.inner.rooms.pauses;
        let delay = *pauses
            .access_delay
            .lock()
            .expect("the pauses are not poisoned");
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }
        let waiting = pauses
            .access_stall
            .lock()
            .expect("the pauses are not poisoned")
            .take();
        if let Some(waiting) = waiting {
            waiting.notify_one();
            std::future::pending::<()>().await;
        }
    }
}
