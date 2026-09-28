use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

const WINDOW: Duration = Duration::from_secs(5 * 60);
const MAX_KEYS: usize = 10_000;
pub const REVISION_WRITE_WINDOW: Duration = Duration::from_secs(60);
pub const REVISION_WRITE_LIMIT: u32 = 30;

#[derive(Clone, Default)]
pub struct RateLimiter {
    inner: Arc<Mutex<Counters>>,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn allow(&self, key: &str, limit: u32) -> Result<(), u32> {
        self.allow_window(key, limit, WINDOW).await
    }

    pub async fn allow_window(&self, key: &str, limit: u32, window: Duration) -> Result<(), u32> {
        let now = Instant::now();
        let mut counters = self.inner.lock().await;
        counters.allow_at(key, limit, window, now)
    }
}

/// Every key's recent hits, at most `MAX_KEYS` keys. The clock is a
/// parameter so tests can move it.
#[derive(Default)]
struct Counters {
    map: HashMap<String, Counter>,
    /// How many keys of `map` each namespace holds, kept in step with it.
    namespaces: HashMap<String, usize>,
}

struct Counter {
    /// The longest window a caller has used for this key. Hits are kept that
    /// long, both by each call and by pruning a full map, so neither pruning
    /// nor a caller with a shorter window drops a hit a longer-window caller
    /// of the same key still counts.
    window: Duration,
    /// In time order, oldest first.
    hits: Vec<Instant>,
}

impl Counters {
    fn allow_at(
        &mut self,
        key: &str,
        limit: u32,
        window: Duration,
        now: Instant,
    ) -> Result<(), u32> {
        if !self.map.contains_key(key) && self.map.len() >= MAX_KEYS {
            self.evict_stale(now);
            if self.map.len() >= MAX_KEYS {
                self.evict_from_largest_namespace();
            }
        }
        let counter = match self.map.entry(key.to_string()) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                match self.namespaces.get_mut(namespace(key)) {
                    Some(size) => *size += 1,
                    None => {
                        self.namespaces.insert(namespace(key).to_string(), 1);
                    }
                }
                entry.insert(Counter {
                    window,
                    hits: Vec::new(),
                })
            }
        };
        counter.window = counter.window.max(window);
        let kept = counter.window;
        counter.hits.retain(|t| now.duration_since(*t) < kept);
        // Hits are in time order; this caller counts only its own window.
        let first = counter
            .hits
            .partition_point(|t| now.duration_since(*t) >= window);
        let counted = &counter.hits[first..];
        if counted.len() >= limit as usize {
            let retry_after = counted
                .first()
                .map(|oldest| {
                    let remaining = window.saturating_sub(now.duration_since(*oldest));
                    remaining.as_secs().max(1) as u32
                })
                .unwrap_or(1);
            return Err(retry_after);
        }
        counter.hits.push(now);
        Ok(())
    }

    /// Drops the hits each key's own window no longer counts, then the keys
    /// left without hits.
    fn evict_stale(&mut self, now: Instant) {
        let namespaces = &mut self.namespaces;
        self.map.retain(|key, counter| {
            let window = counter.window;
            counter.hits.retain(|t| now.duration_since(*t) < window);
            let live = !counter.hits.is_empty();
            if !live {
                forget_key(namespaces, key);
            }
            live
        });
    }

    /// Makes room in a map that is still full of live counters. The
    /// namespace with the most keys gives up its counter with the fewest
    /// hits. One client can mint unlimited fresh keys (`invite-accept` takes
    /// any token), so a flood evicts its own one-hit keys and never resets
    /// another namespace's counters.
    fn evict_from_largest_namespace(&mut self) {
        let Some(largest) = self
            .namespaces
            .iter()
            .max_by_key(|(_, size)| **size)
            .map(|(namespace, _)| namespace.as_str())
        else {
            return;
        };
        let mut victim: Option<(&String, usize)> = None;
        for (key, counter) in &self.map {
            if namespace(key) != largest {
                continue;
            }
            let hits = counter.hits.len();
            if victim.is_none_or(|(_, fewest)| hits < fewest) {
                victim = Some((key, hits));
                if hits <= 1 {
                    break;
                }
            }
        }
        let Some(key) = victim.map(|(key, _)| key.clone()) else {
            return;
        };
        self.map.remove(&key);
        forget_key(&mut self.namespaces, &key);
    }
}

/// A key's namespace: the text before its first ':' (`ai-user`,
/// `invite-accept`, `login`, ...).
fn namespace(key: &str) -> &str {
    key.split_once(':').map_or(key, |(namespace, _)| namespace)
}

fn forget_key(namespaces: &mut HashMap<String, usize>, key: &str) {
    let namespace = namespace(key);
    if let Some(size) = namespaces.get_mut(namespace) {
        *size -= 1;
        if *size == 0 {
            namespaces.remove(namespace);
        }
    }
}

pub fn peer_ip(ip: IpAddr) -> String {
    ip.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE_MIN: Duration = Duration::from_secs(60);
    const FIFTEEN_MIN: Duration = Duration::from_secs(15 * 60);

    fn saturate(counters: &mut Counters, key: &str, limit: u32, window: Duration, now: Instant) {
        for _ in 0..limit {
            counters
                .allow_at(key, limit, window, now)
                .expect("under the limit");
        }
        assert!(counters.allow_at(key, limit, window, now).is_err());
    }

    fn flood_invite_accept(counters: &mut Counters, now: Instant) {
        for i in 0..2 * MAX_KEYS {
            let key = format!("invite-accept:203.0.113.7:{i:08x}");
            counters
                .allow_at(&key, 30, WINDOW, now)
                .expect("a fresh key is under its limit");
            assert!(counters.map.len() <= MAX_KEYS);
        }
        assert_eq!(
            counters.namespaces.values().sum::<usize>(),
            counters.map.len(),
            "namespace sizes out of step with the map"
        );
    }

    #[test]
    fn limit_and_retry_after_are_unchanged() {
        let mut counters = Counters::default();
        let t0 = Instant::now();
        saturate(&mut counters, "login:ip:192.0.2.1", 3, WINDOW, t0);
        assert_eq!(
            counters.allow_at(
                "login:ip:192.0.2.1",
                3,
                WINDOW,
                t0 + Duration::from_secs(100)
            ),
            Err(200)
        );
        assert_eq!(
            counters.allow_at("login:ip:192.0.2.1", 3, WINDOW, t0 + WINDOW),
            Ok(())
        );
    }

    /// One anonymous client can mint unlimited invite-accept keys. Once the
    /// map is full, those keys must evict each other, not the per-user
    /// counters of other namespaces (an evicted counter starts again at 0).
    #[test]
    fn a_flood_of_new_keys_does_not_reset_other_namespaces() {
        let mut counters = Counters::default();
        let t0 = Instant::now();
        for user in 0..100 {
            saturate(&mut counters, &format!("ai-user:{user}"), 10, WINDOW, t0);
        }
        flood_invite_accept(&mut counters, t0);
        let reset = (0..100)
            .filter(|user| {
                counters
                    .allow_at(&format!("ai-user:{user}"), 10, WINDOW, t0)
                    .is_ok()
            })
            .count();
        assert_eq!(reset, 0, "{reset} of 100 ai-user counters were reset");
    }

    /// Inside the flooded namespace the counters that hold the most hits
    /// survive: the flood evicts its own one-hit keys first.
    #[test]
    fn a_flood_keeps_a_saturated_counter_in_its_own_namespace() {
        let mut counters = Counters::default();
        let t0 = Instant::now();
        let target = "invite-accept:203.0.113.7:target";
        saturate(&mut counters, target, 30, WINDOW, t0);
        flood_invite_accept(&mut counters, t0);
        assert!(counters.allow_at(target, 30, WINDOW, t0).is_err());
    }

    /// Pruning a full map drops each counter's hits by that counter's own
    /// window: a new 60 s key must not trim a 15-minute counter to 60 s.
    #[test]
    fn pruning_a_full_map_keeps_each_counters_own_window() {
        let mut counters = Counters::default();
        let t0 = Instant::now();
        saturate(&mut counters, "import-user:u", 5, FIFTEEN_MIN, t0);
        for i in 0..MAX_KEYS - 1 {
            counters
                .allow_at(&format!("share-ip:{i}"), 60, ONE_MIN, t0)
                .unwrap();
        }
        assert_eq!(counters.map.len(), MAX_KEYS);
        let later = t0 + Duration::from_secs(120);
        counters
            .allow_at("share-ip:new", 60, ONE_MIN, later)
            .unwrap();
        assert!(
            counters
                .allow_at("import-user:u", 5, FIFTEEN_MIN, later)
                .is_err(),
            "the 15-minute counter was pruned with a 60 s window"
        );
    }

    /// A key used with two windows keeps the longer window's history: a
    /// 60 s caller counts only its own minute and must not trim the hits a
    /// 15-minute caller of the same key still counts.
    #[test]
    fn a_key_used_with_two_windows_keeps_the_longer_history() {
        let mut counters = Counters::default();
        let t0 = Instant::now();
        let key = "shared:k";
        saturate(&mut counters, key, 5, FIFTEEN_MIN, t0);
        let later = t0 + Duration::from_secs(120);
        assert_eq!(
            counters.allow_at(key, 5, ONE_MIN, later),
            Ok(()),
            "the 60 s caller counted hits older than its window"
        );
        assert_eq!(
            counters.allow_at(key, 5, FIFTEEN_MIN, later),
            Err(780),
            "the 60 s caller trimmed the 15-minute history"
        );
    }
}
