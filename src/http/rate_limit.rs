use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

const WINDOW: Duration = Duration::from_secs(5 * 60);
const MAX_KEYS: usize = 10_000;

#[derive(Clone, Default)]
pub struct RateLimiter {
    inner: Arc<Mutex<HashMap<String, Vec<Instant>>>>,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn allow(&self, key: &str, limit: u32) -> Result<(), u32> {
        let now = Instant::now();
        let mut map = self.inner.lock().await;
        if !map.contains_key(key) && map.len() >= MAX_KEYS {
            evict_stale(&mut map, now);
            if map.len() >= MAX_KEYS {
                evict_one(&mut map);
            }
        }
        let entries = map.entry(key.to_string()).or_default();
        entries.retain(|t| now.duration_since(*t) < WINDOW);
        if entries.len() >= limit as usize {
            let retry_after = entries
                .first()
                .map(|oldest| {
                    let remaining = WINDOW.saturating_sub(now.duration_since(*oldest));
                    remaining.as_secs().max(1) as u32
                })
                .unwrap_or(1);
            return Err(retry_after);
        }
        entries.push(now);
        Ok(())
    }
}

fn evict_stale(map: &mut HashMap<String, Vec<Instant>>, now: Instant) {
    map.retain(|_, entries| {
        entries.retain(|t| now.duration_since(*t) < WINDOW);
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
