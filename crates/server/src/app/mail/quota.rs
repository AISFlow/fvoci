//! How many messages each key (an account, a workspace, an organization) was given
//! within the last hour.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::Duration;

use tokio::time::Instant;

use crate::auth::limiter::RetryAfter;

/// The window the hourly limit counts messages in.
const WINDOW: Duration = Duration::from_hours(1);

/// Why messages were not taken; nothing was recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Refusal<E> {
    /// The key would pass its limit; enough earlier messages leave after this.
    Limit(RetryAfter),
    /// The messages fit within the hour, but finding a place for them failed so.
    Unplaced(E),
}

/// Messages admitted per key, as (when, how many) oldest first, kept in memory.
#[derive(Debug, Default)]
pub(super) struct HourlyQuota {
    admitted: Mutex<HashMap<String, VecDeque<(Instant, u32)>>>,
}

impl HourlyQuota {
    /// Records `messages` for `key` if they fit within `per_hour` over the last hour
    /// and `place` finds room for them, returning what it found. Both are decided under
    /// one lock, so no other request is counted in between. The limit is the caller's,
    /// as it may differ from key to key.
    ///
    /// # Errors
    /// Returns [`Refusal::Limit`] with how long until enough earlier messages leave the
    /// window, or [`Refusal::Unplaced`] with what `place` failed with; nothing is
    /// recorded.
    pub(super) fn take<T, E>(
        &self,
        key: &str,
        messages: u32,
        per_hour: u32,
        now: Instant,
        place: impl FnOnce() -> Result<T, E>,
    ) -> Result<T, Refusal<E>> {
        let mut admitted = self.admitted.lock().expect("mail quota lock poisoned");
        // Forget keys with nothing in the window, so the map stays as small as the
        // keys that were given mail within the hour.
        admitted.retain(|_, entries| {
            while entries
                .front()
                .is_some_and(|(at, _)| now.saturating_duration_since(*at) >= WINDOW)
            {
                entries.pop_front();
            }
            !entries.is_empty()
        });
        let earlier = admitted.get(key);
        let mut used: u32 = earlier.map_or(0, |entries| entries.iter().map(|(_, n)| n).sum());
        if used.saturating_add(messages) > per_hour {
            // The configuration keeps a request's messages within every hourly limit,
            // so the window always empties far enough.
            let mut wait = WINDOW;
            for (at, count) in earlier.into_iter().flatten() {
                used -= count;
                if used.saturating_add(messages) <= per_hour {
                    wait = (*at + WINDOW).saturating_duration_since(now);
                    break;
                }
            }
            return Err(Refusal::Limit(RetryAfter(wait)));
        }
        let placed = place().map_err(Refusal::Unplaced)?;
        admitted
            .entry(key.to_owned())
            .or_default()
            .push_back((now, messages));
        Ok(placed)
    }

    /// Checks each key in `keys` against `per_hour` at `now` under one lock, recording
    /// one message for each key that fits within its hourly window and returning `true`
    /// at its index, or `false` without recording anything for keys already at `per_hour`.
    pub(super) fn take_each<S: AsRef<str>>(
        &self,
        keys: &[S],
        per_hour: u32,
        now: Instant,
    ) -> Vec<bool> {
        let mut admitted = self.admitted.lock().expect("mail quota lock poisoned");
        admitted.retain(|_, entries| {
            while entries
                .front()
                .is_some_and(|(at, _)| now.saturating_duration_since(*at) >= WINDOW)
            {
                entries.pop_front();
            }
            !entries.is_empty()
        });
        keys.iter()
            .map(|key| {
                let key = key.as_ref();
                let used: u32 = admitted
                    .get(key)
                    .map_or(0, |entries| entries.iter().map(|(_, n)| n).sum());
                if used.saturating_add(1) > per_hour {
                    false
                } else {
                    admitted
                        .entry(key.to_owned())
                        .or_default()
                        .push_back((now, 1));
                    true
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn take(quota: &HourlyQuota, key: &str, messages: u32, at: Instant) -> Option<Duration> {
        match quota.take(key, messages, 5, at, || Ok::<_, ()>(())) {
            Ok(()) => None,
            Err(Refusal::Limit(RetryAfter(wait))) => Some(wait),
            Err(Refusal::Unplaced(())) => panic!("placing always succeeds here"),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_key_waits_until_its_oldest_messages_leave_the_hour() {
        let quota = HourlyQuota::default();
        let start = Instant::now();
        assert_eq!(take(&quota, "w1", 3, start), None);
        assert_eq!(take(&quota, "w1", 2, start + Duration::from_mins(10)), None);
        assert_eq!(
            take(&quota, "w1", 1, start + Duration::from_mins(15)),
            Some(Duration::from_mins(45)),
            "the first batch leaves first"
        );
        assert_eq!(
            take(&quota, "w2", 5, start + Duration::from_mins(15)),
            None,
            "another key counts on its own"
        );
        assert_eq!(
            take(&quota, "w1", 4, start + Duration::from_mins(15)),
            Some(Duration::from_mins(55)),
            "needs both batches gone"
        );
        assert_eq!(
            take(&quota, "w1", 3, start + WINDOW),
            None,
            "the first batch left the window"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn messages_without_a_place_take_nothing_of_the_hour() {
        let quota = HourlyQuota::default();
        let start = Instant::now();
        assert_eq!(
            quota.take("w1", 5, 5, start, || Err::<(), _>("full")),
            Err(Refusal::Unplaced("full"))
        );
        assert_eq!(take(&quota, "w1", 5, start), None, "the hour is untouched");
        let mut asked = false;
        let refused = quota.take("w1", 1, 5, start, || {
            asked = true;
            Ok::<_, ()>(())
        });
        assert_eq!(
            refused,
            Err(Refusal::Limit(RetryAfter(WINDOW))),
            "the limit is told before a place is looked for"
        );
        assert!(!asked);
    }
}
