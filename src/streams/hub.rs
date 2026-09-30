use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use crate::streams::MAX_CONCURRENT_STREAMS;

/// Process-wide SSE admission: a slot count capped at
/// [`MAX_CONCURRENT_STREAMS`] and a shutdown flag.
///
/// A [`StreamGuard`] owns one slot. The routes take it before authentication,
/// so a rejected request holds it only until the handler returns; an admitted
/// stream moves it into the response body, which frees it when dropped
/// (stream end or client disconnect). Past the cap a request gets 429; after
/// [`begin_shutdown`](Self::begin_shutdown) it gets 503 `stream_stopped`.
///
/// Shutdown: `begin_shutdown` refuses new streams. A running producer exits at
/// the top of its next loop, after finishing its current sleep and at most one
/// more poll. The body then drains any queued items, each still authorized,
/// and ends, which lets the graceful HTTP drain finish.
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

/// One stream slot; dropping it frees the slot.
pub struct StreamGuard {
    hub: Arc<StreamHub>,
}

impl Drop for StreamGuard {
    fn drop(&mut self) {
        self.hub.active.fetch_sub(1, Ordering::AcqRel);
    }
}
