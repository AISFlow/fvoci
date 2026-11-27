//! The versions documents are written to the search server with
//! (`version_type=external_gte`), so that a write that lands late never undoes a later
//! one: a `_bulk` that ran out of its time may still be done by the search server after
//! the next one, and would put back a card that was removed meanwhile.
//!
//! A version is the time, in microseconds, at which what is sent was read from the
//! database, and never one this process gave before: what is read later is as new or
//! newer, so it is sent with a version as high or higher. A process that starts begins
//! past the highest version the index holds (`sent`), however its clock stands; and
//! past the version of a document the search server says it holds when it refuses one
//! for being lower (a removal's, which `sent` does not keep).

use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// The version a document is written with: not the version of the definition of the
/// index ([`super::index::VERSION`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct DocumentVersion(i64);

impl DocumentVersion {
    /// The version as the search server reads it.
    pub(crate) fn get(self) -> i64 {
        self.0
    }
}

/// Where the versions of this process stand.
#[derive(Debug, Default)]
pub(crate) struct Versions {
    /// The last version given. One atomic, and nothing else it guards: what it says
    /// alone is ordered.
    last: AtomicI64,
}

/// The time now, in microseconds since the Unix epoch.
fn now_micros() -> i64 {
    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(since_epoch.as_micros()).unwrap_or(i64::MAX)
}

impl Versions {
    /// The version of what is about to be read: the time now, or one past the last one
    /// given if the clock has not moved past it.
    pub(crate) fn next(&self) -> DocumentVersion {
        let now = now_micros();
        let given = |last: i64| now.max(last.saturating_add(1));
        let last = self
            .last
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |last| {
                Some(given(last))
            })
            .unwrap_or_else(|last| last);
        DocumentVersion(given(last))
    }

    /// Has every version given from now on be higher than `version`.
    pub(crate) fn past(&self, version: i64) {
        self.last.fetch_max(version, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_version_is_higher_than_the_one_before() {
        let versions = Versions::default();
        let mut last = versions.next();
        assert!(
            last.get() > 1_000_000_000_000_000,
            "the time in microseconds: {last:?}"
        );
        for _ in 0..10_000 {
            let next = versions.next();
            assert!(next > last, "{next:?} after {last:?}");
            last = next;
        }
    }

    #[test]
    fn a_clock_behind_the_index_starts_past_it() {
        let versions = Versions::default();
        let ahead = now_micros() + 3_600_000_000;
        versions.past(ahead);
        assert_eq!(versions.next().get(), ahead + 1);
        assert_eq!(versions.next().get(), ahead + 2);
        versions.past(1);
        assert_eq!(versions.next().get(), ahead + 3, "never back");
    }
}
