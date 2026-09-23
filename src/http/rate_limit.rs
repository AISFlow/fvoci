use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

const WINDOW: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, Default)]
pub struct RateLimiter {
    inner: Arc<Mutex<HashMap<String, Vec<Instant>>>>,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn allow(&self, key: &str, limit: u32) -> bool {
        let now = Instant::now();
        let mut map = self.inner.lock().await;
        let entries = map.entry(key.to_string()).or_default();
        entries.retain(|t| now.duration_since(*t) < WINDOW);
        if entries.len() >= limit as usize {
            return false;
        }
        entries.push(now);
        true
    }
}
