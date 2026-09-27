//! Server-sent task invalidation and workspace access signals (single-process hub).

mod events;
mod hub;

pub use events::{
    access_event_targets_user, initial_cursor, poll_access_events, poll_task_events, EventCursor,
    StreamEventRow,
};
pub use hub::{StreamAcquireError, StreamGuard, StreamHub};

/// Bounded queue depth per SSE connection (slow readers block then disconnect).
pub const STREAM_CHANNEL_CAPACITY: usize = 8;

pub const MAX_CONCURRENT_STREAMS: usize = 64;
pub const STREAM_HIGH_WATER_MARK: usize = 64 * 1024;
pub const STREAM_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(750);
pub const STREAM_KEEPALIVE: std::time::Duration = std::time::Duration::from_secs(15);
