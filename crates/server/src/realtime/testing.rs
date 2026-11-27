//! The seams of the tests: between validating a subscription and registering it,
//! whether the connections carried out what they read, and the quiet limit of a
//! connection.

use std::sync::Arc;

use tokio::sync::{Notify, watch};

use super::Hub;

/// Stops the next validated subscription before it is registered. Dropping the
/// handle releases it, so a failed test cannot leave the connection waiting.
#[derive(Debug, Clone, Default)]
pub struct SubscriptionPause {
    reached: Arc<Notify>,
    resumed: Arc<Notify>,
}

impl SubscriptionPause {
    /// Waits until the subscription has passed its authorization check.
    pub async fn reached(&self) {
        self.reached.notified().await;
    }

    /// Lets the subscription attempt register itself.
    pub fn resume(&self) {
        self.resumed.notify_one();
    }
}

impl Drop for SubscriptionPause {
    fn drop(&mut self) {
        self.resume();
    }
}

impl Hub {
    /// Pauses the next valid subscription immediately before registration.
    ///
    /// # Panics
    ///
    /// Panics when another pause is already installed.
    pub fn pause_next_subscription(&self) -> SubscriptionPause {
        let pause = SubscriptionPause::default();
        let mut next = self
            .inner
            .subscription_pause
            .lock()
            .expect("the test pause lock");
        assert!(
            next.is_none(),
            "only one subscription may be paused at a time"
        );
        *next = Some(pause.clone());
        pause
    }

    pub(super) async fn pause_subscription(&self) {
        let pause = self
            .inner
            .subscription_pause
            .lock()
            .expect("the test pause lock")
            .take();
        if let Some(pause) = pause {
            pause.reached.notify_one();
            pause.resumed.notified().await;
        }
    }
}

impl Hub {
    /// Pauses the next authorized dispatch before its final revision check and enqueue.
    ///
    /// # Panics
    /// Panics when another dispatch pause is installed.
    pub fn pause_next_dispatch(&self) -> SubscriptionPause {
        let pause = SubscriptionPause::default();
        let mut next = self
            .inner
            .dispatch_pause
            .lock()
            .expect("dispatch test pause lock");
        assert!(next.is_none(), "only one dispatch may be paused");
        *next = Some(pause.clone());
        pause
    }

    pub(super) async fn pause_dispatch(&self) {
        let pause = self
            .inner
            .dispatch_pause
            .lock()
            .expect("dispatch test pause lock")
            .take();
        if let Some(pause) = pause {
            pause.reached.notify_one();
            pause.resumed.notified().await;
        }
    }
}

impl Hub {
    /// Has the connections closed after `limit` of sending nothing, in place of the
    /// ninety seconds of `realtime/quiet.rs`: those open now from their next wait on.
    pub fn set_quiet_limit(&self, limit: std::time::Duration) {
        self.inner.quiet_limit.set(limit);
    }
}

/// How many connections have read a command they have not carried out yet.
#[derive(Debug)]
pub(super) struct AtWork(watch::Sender<usize>);

impl Default for AtWork {
    fn default() -> Self {
        Self(watch::Sender::new(0))
    }
}

impl AtWork {
    /// Counts a connection that read a command, until what is returned is dropped.
    pub(super) fn begin(&self) -> Working {
        self.0.send_modify(|connections| *connections += 1);
        Working(self.0.clone())
    }
}

/// A connection that read a command and has not carried it out yet.
#[derive(Debug)]
pub(crate) struct Working(watch::Sender<usize>);

impl Drop for Working {
    fn drop(&mut self) {
        self.0.send_modify(|connections| {
            debug_assert!(*connections > 0, "counted once for every `Working`");
            *connections = connections.saturating_sub(1);
        });
    }
}

impl Hub {
    /// Returns once no connection holds a command it read and has not carried out.
    ///
    /// The websocket's Pong to a Ping says only that the server read what was sent
    /// before the Ping: a connection reads on while it carries a command out. It is
    /// counted here from the command it read, before it reads anything after it, so
    /// a test that has the Pong and then waits for this knows its commands were
    /// carried out. A connection held by another seam of the tests keeps this waiting.
    pub async fn carried_out(&self) {
        let mut at_work = self.inner.at_work.0.subscribe();
        // The hub holds the sender: the wait does not end for want of one.
        let _ = at_work.wait_for(|connections| *connections == 0).await;
    }
}
