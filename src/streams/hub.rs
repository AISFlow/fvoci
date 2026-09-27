use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use crate::streams::MAX_CONCURRENT_STREAMS;

pub struct StreamHub {
    active: AtomicUsize,
    shutting_down: AtomicBool,
}

impl Default for StreamHub {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamHub {
    pub fn new() -> Self {
        Self {
            active: AtomicUsize::new(0),
            shutting_down: AtomicBool::new(false),
        }
    }

    pub fn new_arc() -> Arc<Self> {
        Arc::new(Self::new())
    }

    pub fn accepting(&self) -> bool {
        !self.shutting_down.load(Ordering::Acquire)
    }

    pub fn begin_shutdown(&self) {
        self.shutting_down.store(true, Ordering::Release);
    }

    pub fn active_count(&self) -> usize {
        self.active.load(Ordering::Acquire)
    }

    pub fn try_acquire(self: &Arc<Self>) -> Result<StreamGuard, StreamAcquireError> {
        if !self.accepting() {
            return Err(StreamAcquireError::Stopped);
        }
        let prev = self.active.fetch_add(1, Ordering::AcqRel);
        if prev >= MAX_CONCURRENT_STREAMS {
            self.active.fetch_sub(1, Ordering::AcqRel);
            return Err(StreamAcquireError::Capacity);
        }
        Ok(StreamGuard { hub: self.clone() })
    }
}

pub enum StreamAcquireError {
    Capacity,
    Stopped,
}

pub struct StreamGuard {
    hub: Arc<StreamHub>,
}

impl Drop for StreamGuard {
    fn drop(&mut self) {
        self.hub.active.fetch_sub(1, Ordering::AcqRel);
    }
}
