//! Monotonic wall clock shared across a [`crate::Store`] and its [`crate::db::Scope`]s.
//!
//! [`Clock::now_millis`] clamps wall-clock reads against the highest timestamp previously
//! observed on this [`Clock`] instance so backward NTP adjustments never produce decreasing
//! millisecond timestamps. A forward wall-clock jump raises the floor and holds that value
//! until the clock catches up or the process restarts; optimistic-concurrency writes bump
//! ties via `now.max(existing.updated_at + 1)`. Tests may install a deterministic source
//! via [`Clock::set_source`].

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

type TimeSource = Arc<dyn Fn() -> i64 + Send + Sync>;

/// Monotonic millisecond wall clock, cheap to clone and shared across handles.
#[derive(Clone)]
pub struct Clock {
    inner: Arc<ClockInner>,
}

struct ClockInner {
    has_source: AtomicBool,
    source: RwLock<Option<TimeSource>>,
    floor: AtomicI64,
}

impl Default for Clock {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for Clock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Clock")
            .field("has_source", &self.inner.has_source.load(Ordering::Relaxed))
            .field("floor", &self.inner.floor.load(Ordering::Relaxed))
            .finish()
    }
}

impl Clock {
    /// Creates a new monotonic clock backed by [`SystemTime::now`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(ClockInner {
                has_source: AtomicBool::new(false),
                source: RwLock::new(None),
                floor: AtomicI64::new(0),
            }),
        }
    }

    /// Returns the current time in milliseconds since the Unix epoch, never returning a
    /// value smaller than an earlier call on the same clock timeline.
    #[must_use]
    pub fn now_millis(&self) -> i64 {
        let raw = if self.inner.has_source.load(Ordering::Acquire) {
            let guard = self
                .inner
                .source
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.as_ref().map_or_else(system_millis, |f| f())
        } else {
            system_millis()
        };
        self.inner.floor.fetch_max(raw, Ordering::Relaxed).max(raw)
    }

    /// Replaces the backing time source and resets the monotonic floor to the new
    /// timeline (for deterministic tests).
    #[cfg(any(test, feature = "test-util"))]
    pub fn set_source(&self, source: impl Fn() -> i64 + Send + Sync + 'static) {
        let mut guard = self
            .inner
            .source
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *guard = Some(Arc::new(source));
        self.inner.floor.store(0, Ordering::SeqCst);
        self.inner.has_source.store(true, Ordering::Release);
    }

    /// Restores [`SystemTime::now`] as the backing time source.
    #[cfg(any(test, feature = "test-util"))]
    pub fn clear_source(&self) {
        let mut guard = self
            .inner
            .source
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *guard = None;
        self.inner.floor.store(0, Ordering::SeqCst);
        self.inner.has_source.store(false, Ordering::Release);
    }
}

pub(crate) fn system_millis() -> i64 {
    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(since_epoch.as_millis()).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicI64, Ordering};

    use super::Clock;

    #[test]
    fn clock_clamps_backward_steps_monotonically() {
        let now = AtomicI64::new(1_000);
        let clock = Clock::new();
        clock.set_source(move || now.load(Ordering::SeqCst));
        assert_eq!(clock.now_millis(), 1_000);

        let raw = AtomicI64::new(5_000);
        clock.set_source(move || raw.fetch_sub(100, Ordering::SeqCst));
        assert_eq!(clock.now_millis(), 5_000);
        assert_eq!(clock.now_millis(), 5_000, "backward step must clamp");
    }
}
