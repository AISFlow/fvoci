//! The limit on how often one user searches.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use tokio::time::Instant;

/// What one search costs, in the thousandths of a search the limit counts in.
const COST: u64 = 1000;

/// How many users are remembered before those who have not searched for a while are
/// forgotten.
const MOST_USERS: usize = 10_000;

/// A user's allowance: what is left of it, and when that was so.
#[derive(Debug, Clone, Copy)]
struct Allowance {
    /// In thousandths of a search.
    left: u64,
    at: Instant,
}

/// How many searches a user may make: `per_second` in a second, and twice as many at
/// once after a pause. Each user is counted apart.
#[derive(Debug)]
pub(crate) struct Limiter {
    /// 0 for no limit.
    per_second: u64,
    users: Mutex<HashMap<String, Allowance>>,
}

impl Limiter {
    pub(crate) fn new(per_second: u32) -> Self {
        Self {
            per_second: u64::from(per_second),
            users: Mutex::default(),
        }
    }

    /// The most a user has saved up.
    fn full(&self) -> u64 {
        self.per_second.saturating_mul(2 * COST)
    }

    /// Counts a search of `user_id` at `now`.
    ///
    /// # Errors
    ///
    /// Returns how long the user is to wait if they have searched too often.
    pub(crate) fn take(&self, user_id: &str, now: Instant) -> Result<(), Duration> {
        if self.per_second == 0 {
            return Ok(());
        }
        let mut users = self
            .users
            .lock()
            .expect("the lock of the search limit is not poisoned");
        if users.len() >= MOST_USERS && !users.contains_key(user_id) {
            // Whoever has their whole allowance again is as good as never seen.
            let full = self.full();
            let per_second = self.per_second;
            users.retain(|_, allowance| refilled(*allowance, now, per_second, full) < full);
        }
        let allowance = users.entry(user_id.to_owned()).or_insert(Allowance {
            left: self.full(),
            at: now,
        });
        let left = refilled(*allowance, now, self.per_second, self.full());
        if left < COST {
            let short = COST - left;
            return Err(Duration::from_millis(short.div_ceil(self.per_second)));
        }
        *allowance = Allowance {
            left: left - COST,
            at: now,
        };
        Ok(())
    }
}

/// What is left of an allowance at `now`: a thousandth of `per_second` searches comes
/// back every millisecond, up to `full`.
fn refilled(allowance: Allowance, now: Instant, per_second: u64, full: u64) -> u64 {
    let waited =
        u64::try_from(now.saturating_duration_since(allowance.at).as_millis()).unwrap_or(u64::MAX);
    allowance
        .left
        .saturating_add(waited.saturating_mul(per_second))
        .min(full)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_user_searches_twice_the_rate_at_once_and_then_at_the_rate() {
        let limiter = Limiter::new(5);
        let start = Instant::now();
        for _ in 0..10 {
            limiter.take("a", start).expect("within the allowance");
        }
        let wait = limiter
            .take("a", start)
            .expect_err("the allowance is spent");
        assert_eq!(wait, Duration::from_millis(200));
        limiter
            .take("b", start)
            .expect("another user is counted apart");
        limiter
            .take("a", start + Duration::from_millis(200))
            .expect("one came back");
        limiter
            .take("a", start + Duration::from_millis(200))
            .expect_err("and only one");
        for _ in 0..10 {
            limiter
                .take("a", start + Duration::from_secs(60))
                .expect("after a pause the whole allowance");
        }
    }

    #[test]
    fn no_limit_takes_every_search() {
        let limiter = Limiter::new(0);
        let now = Instant::now();
        for _ in 0..1000 {
            limiter.take("a", now).expect("no limit");
        }
    }
}
