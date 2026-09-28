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

/// Every key's recent hits. The clock is a parameter so tests can move it.
#[derive(Default)]
struct Counters {
    map: HashMap<String, Vec<Instant>>,
}

impl Counters {
    fn allow_at(
        &mut self,
        key: &str,
        limit: u32,
        window: Duration,
        now: Instant,
    ) -> Result<(), u32> {
        let map = &mut self.map;
        if !map.contains_key(key) && map.len() >= MAX_KEYS {
            evict_stale(map, now, window);
            if map.len() >= MAX_KEYS {
                evict_one(map);
            }
        }
        let entries = map.entry(key.to_string()).or_default();
        entries.retain(|t| now.duration_since(*t) < window);
        if entries.len() >= limit as usize {
            let retry_after = entries
                .first()
                .map(|oldest| {
                    let remaining = window.saturating_sub(now.duration_since(*oldest));
                    remaining.as_secs().max(1) as u32
                })
                .unwrap_or(1);
            return Err(retry_after);
        }
        entries.push(now);
        Ok(())
    }
}

fn evict_stale(map: &mut HashMap<String, Vec<Instant>>, now: Instant, window: Duration) {
    map.retain(|_, entries| {
        entries.retain(|t| now.duration_since(*t) < window);
        !entries.is_empty()
    });
}

fn evict_one(map: &mut HashMap<String, Vec<Instant>>) {
    if let Some(key) = map.keys().next().cloned() {
        map.remove(&key);
    }
}

pub fn peer_ip(ip: IpAddr) -> String {
    ip.to_string()
}
