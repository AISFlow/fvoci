//! The websocket endpoint `/ws` as a transport: the frames of a connection read and
//! written over axum's socket. What a connection follows and is told is the hub's
//! (`crate::realtime`); this hands it every command and writes what it queues.

mod inbound;
mod serve;
mod writer;

use std::time::Duration;

pub(crate) use serve::serve;

/// How many undelivered messages a connection may have queued before it is dropped. A
/// client that reads this slowly has lost track of the board anyway and reloads on
/// reconnect.
const OUTBOX_CAPACITY: usize = 512;

/// How long a connection that is being closed may take to send what is already on its
/// way and the closing frame. A peer that has stopped reading gets neither.
const WRITER_GRACE: Duration = Duration::from_secs(5);

/// How often a connection's session is looked up again when the connections are not
/// pinged; when they are, it is looked up at every ping.
const SESSION_CHECK_WITHOUT_PING: Duration = Duration::from_mins(1);
