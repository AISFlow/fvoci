//! The metrics of the websocket connections: how many are open, the messages the hub
//! queues for them by kind, and why they ended.

use prometheus::{IntCounterVec, IntGauge, Opts, Registry};

use super::{Metrics, PREFIX};

/// Why a websocket connection ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Disconnect {
    /// The client sent a close frame.
    ClientClosed,
    /// The connection broke, or what came was no websocket frame: a client gone
    /// without closing (a closed tab, a proxy that dropped an idle connection, a lost
    /// network), a message over the limit, a broken frame.
    ReadError,
    /// Its session ended: logout, a changed password, the idle or absolute limit, or
    /// the user went. Also when the session could not be read back right after the
    /// handshake, which is logged. (What ends of a workspace or a shared board ends
    /// only what follows it, and is counted among the messages queued.)
    SessionEnded,
    /// What it may be told could not be read of the store: it connects again and
    /// reads anew, rather than miss what it was not sent.
    StoreUnavailable,
    /// What it may be told was checked again too often, overtaken each time by
    /// permission writes: it connects again and reads anew.
    ChecksOvertaken,
    /// It did not read its messages as fast as they came.
    SlowConsumer,
    /// An update it sent ended the collab worker that took it; its user is held back
    /// from sending updates for a while.
    EndedWorker,
    /// It sent nothing at all for the quiet limit (`ws/inbound.rs`): a client gone
    /// without a word.
    Quiet,
    /// The server stopped. Counted as it stops, after the metrics listener closed, so a
    /// scrape does not see it.
    Shutdown,
}

impl Disconnect {
    const ALL: [Self; 9] = [
        Self::ClientClosed,
        Self::ReadError,
        Self::SessionEnded,
        Self::StoreUnavailable,
        Self::ChecksOvertaken,
        Self::SlowConsumer,
        Self::EndedWorker,
        Self::Quiet,
        Self::Shutdown,
    ];

    /// The value of the `reason` label.
    fn as_str(self) -> &'static str {
        match self {
            Self::ClientClosed => "client_closed",
            Self::ReadError => "read_error",
            Self::SessionEnded => "session_ended",
            Self::StoreUnavailable => "store_unavailable",
            Self::ChecksOvertaken => "checks_overtaken",
            Self::SlowConsumer => "slow_consumer",
            Self::EndedWorker => "ended_worker",
            Self::Quiet => "quiet",
            Self::Shutdown => "shutdown",
        }
    }
}

/// The metrics of the websocket connections.
#[derive(Debug)]
pub(super) struct WebsocketMetrics {
    connections: IntGauge,
    queued: IntCounterVec,
    disconnections: IntCounterVec,
}

impl WebsocketMetrics {
    /// Makes the metrics and registers them in `registry`.
    pub(super) fn register(registry: &Registry) -> prometheus::Result<Self> {
        let connections = IntGauge::with_opts(
            Opts::new(
                "websocket_connections",
                "How many websocket connections are open; one that ended is counted until \
                 its writer closes, for five seconds at most.",
            )
            .namespace(PREFIX),
        )?;
        let queued = IntCounterVec::new(
            Opts::new(
                "websocket_messages_queued_total",
                "How many messages the server queued for websocket connections, one per \
                 connection, by the message's type. A connection that ends may not get \
                 what was queued for it.",
            )
            .namespace(PREFIX),
            &["type"],
        )?;
        let disconnections = IntCounterVec::new(
            Opts::new(
                "websocket_disconnections_total",
                "How many websocket connections ended, by reason: client_closed (a close \
                 frame), read_error (broken off, or no websocket frame), session_ended, \
                 store_unavailable (what it may be \
                 told could not be read; it connects again), checks_overtaken (checked \
                 again too often as permissions changed; it connects again), slow_consumer, quiet (it sent nothing for the quiet limit), \
                 shutdown (not \
                 seen: counted as the server stops).",
            )
            .namespace(PREFIX),
            &["reason"],
        )?;
        registry.register(Box::new(connections.clone()))?;
        registry.register(Box::new(queued.clone()))?;
        registry.register(Box::new(disconnections.clone()))?;
        // Every reason is there from the start, at zero, so that the first of each is
        // an increase.
        for reason in Disconnect::ALL {
            disconnections.with_label_values(&[reason.as_str()]).reset();
        }
        Ok(Self {
            connections,
            queued,
            disconnections,
        })
    }
}

/// An open websocket connection, counted while it lives.
#[derive(Debug)]
pub(crate) struct OpenConnection(Option<IntGauge>);

impl Drop for OpenConnection {
    fn drop(&mut self) {
        if let Some(gauge) = &self.0 {
            gauge.dec();
        }
    }
}

impl Metrics {
    /// Counts a websocket connection as open until the returned value is dropped.
    pub(crate) fn websocket_opened(&self) -> OpenConnection {
        OpenConnection(self.kept.as_ref().map(|kept| {
            let gauge = kept.websocket.connections.clone();
            gauge.inc();
            gauge
        }))
    }

    /// Counts `count` messages of `message_type` as queued. The `type` is one of
    /// the hub's own, never one a client named.
    pub(crate) fn websocket_queued(&self, message_type: &'static str, count: usize) {
        if let Some(kept) = &self.kept
            && count > 0
        {
            kept.websocket
                .queued
                .with_label_values(&[message_type])
                .inc_by(u64::try_from(count).unwrap_or(u64::MAX));
        }
    }

    /// Counts a websocket connection as ended, for `reason`.
    pub(crate) fn websocket_closed(&self, reason: Disconnect) {
        if let Some(kept) = &self.kept {
            kept.websocket
                .disconnections
                .with_label_values(&[reason.as_str()])
                .inc();
        }
    }
}
