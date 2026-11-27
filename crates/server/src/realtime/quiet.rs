//! How long a connection may send nothing at all before it is taken for dead: no
//! message, no `ping`, no Pong to the server's Ping (`crate::ws::inbound` reads them).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// How long a connection may send nothing before it is taken for dead and closed.
///
/// A page that is in the background has its timers slowed: Chrome lets them run once a
/// minute at most after five minutes hidden, so the `ping` of such a page comes a
/// minute or so apart; thirty seconds more are for a slow network. The browser answers
/// the server's Ping (`websocket_ping_seconds`, 1 to 59, 30 by default) whatever its timers do,
/// which keeps a live connection well within this. The time a connection that died
/// without a word stays in its rooms (its seat counted against the limits of the room
/// and of its user) is this at most.
pub(crate) const QUIET_LIMIT: Duration = Duration::from_secs(90);

/// The part of [`QUIET_LIMIT`] kept for a browser's Pong to the server's Ping to come
/// back over a slow network: a Ping period this much shorter than the limit keeps a
/// page whose timers are stopped.
const PONG_MARGIN: Duration = Duration::from_secs(30);

// The longest Ping period the configuration takes keeps that margin.
const _: () = assert!(
    crate::config::MAX_WEBSOCKET_PING_SECONDS < QUIET_LIMIT.as_secs() - PONG_MARGIN.as_secs(),
    "a ping period the configuration takes leaves no time for the Pong"
);

/// The quiet limit of a hub's connections: [`QUIET_LIMIT`], unless a test shortened
/// it, which the connections that wait see from their next wait on. Cloning is cheap;
/// every clone is the same limit.
#[derive(Debug, Clone)]
pub(crate) struct QuietLimit(Arc<AtomicU64>);

impl Default for QuietLimit {
    fn default() -> Self {
        Self(Arc::new(AtomicU64::new(millis(QUIET_LIMIT))))
    }
}

impl QuietLimit {
    /// The limit as it is now.
    pub(crate) fn get(&self) -> Duration {
        Duration::from_millis(self.0.load(Ordering::Relaxed))
    }

    /// Has the connections closed after `limit` of sending nothing, from their next
    /// wait on.
    #[cfg(feature = "test-util")]
    pub(super) fn set(&self, limit: Duration) {
        self.0.store(millis(limit), Ordering::Relaxed);
    }
}

fn millis(limit: Duration) -> u64 {
    u64::try_from(limit.as_millis()).unwrap_or(u64::MAX)
}
