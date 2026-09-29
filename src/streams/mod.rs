//! Server-sent task invalidation and workspace access signals (single-process hub).

mod events;
mod hub;

pub use events::{
    initial_cursor, poll_access_events, poll_task_events, project_stream_access,
    task_stream_wire_hint, workspace_stream_access, EventCursor, EventPage, StreamAccess,
    StreamEventRow,
};
pub use hub::{StreamAcquireError, StreamGuard, StreamHub};

/// Queue depth per SSE connection, which is also its memory bound: at most
/// this many queued hints plus one awaiting authorization, each under 100
/// bytes. A producer that finds the queue full ends the stream, and the
/// client resyncs on reconnect.
pub const STREAM_CHANNEL_CAPACITY: usize = 8;

/// Bounds the streams' poll transactions on the shared app pool (one per
/// stream per [`STREAM_POLL_INTERVAL`]); past it a new stream gets 429.
pub const MAX_CONCURRENT_STREAMS: usize = 64;
/// Poll period of every stream: one transaction per tick, and a settled event
/// reaches the client up to one tick later.
pub const STREAM_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(750);
pub const STREAM_KEEPALIVE: std::time::Duration = std::time::Duration::from_secs(15);
