//! The lifetime of a websocket connection and its command reader.

use std::net::IpAddr;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use bytes::Bytes;
use futures_util::stream::SplitSink;
use futures_util::{Stream, StreamExt as _};
use tokio::sync::mpsc;

use super::inbound::{Came, Inbound};
use super::{OUTBOX_CAPACITY, SESSION_CHECK_WITHOUT_PING, WRITER_GRACE, writer};
use crate::app::App;
use crate::collab::Seat;
use crate::error::{AppError, ErrorKind};
use crate::metrics::{Disconnect, Metrics};
use crate::realtime::{Connection, Identity, Opened};

/// Serves one websocket connection until it closes. `identity` is the session the
/// handshake came with, if any; without one it follows shared blocks and announces
/// anonymous visits by read token. `app` is the application the handshake reached,
/// `address` the client's (none through the administration socket).
pub(crate) async fn serve(
    app: &App,
    socket: WebSocket,
    identity: Option<Identity>,
    address: Option<IpAddr>,
) {
    let hub = app.ws();
    let (sink, stream) = socket.split();
    let (outbox, inbox) = mpsc::channel::<Bytes>(OUTBOX_CAPACITY);
    let mut inbound = Inbound::new(stream, outbox.clone(), hub.quiet_limit());
    let Opened {
        connection,
        seat: mut docs,
        rooms: docs_outbox,
    } = hub.open(app, identity, address, outbox);
    let docs_overrun = docs.overrun();
    let docs_ended_worker = docs.ended_worker();
    let metrics = hub.metrics();
    let _open = metrics.websocket_opened();

    if let Err(error) = connection.session_kept().await {
        drop(connection);
        close_missing_session_on_open(metrics, &error, sink).await;
        return;
    }

    let ping_interval = hub.ping_interval();
    let session_check = ping_interval.unwrap_or(SESSION_CHECK_WITHOUT_PING);
    let closed = connection.closed();
    let closed_because = connection.closed_because();
    let mut writer = writer::spawn(
        sink,
        inbox,
        docs_outbox,
        ping_interval,
        closed.clone(),
        Arc::clone(closed_because),
    );

    let reason = tokio::select! {
        // First: when the hub gave the connection up, its reason is the one counted,
        // also if the client closed it at the same moment.
        biased;
        // The hub gave the connection up: it does not keep up, its session ended, or
        // what it may be told could not be read.
        () = closed.cancelled() => {
            closed_because.get().copied().unwrap_or(Disconnect::Shutdown)
        }
        // It reads nothing of what the rooms send it.
        _ = &mut writer => Disconnect::ReadError,
        never = Box::pin(connection.tell_presence(app)) => match never {},
        () = docs_overrun.cancelled() => Disconnect::SlowConsumer,
        () = docs_ended_worker.cancelled() => Disconnect::EndedWorker,
        reason = read_commands(app, &connection, &mut inbound, &mut docs) => reason,
        // The session ended by its idle or absolute limit, which nobody announces:
        // the sessions that ended so are deleted only by the hourly clean-up.
        () = connection.session_ends(session_check) => Disconnect::SessionEnded,
    };
    let _ = closed_because.set(reason);
    closed.cancel();
    metrics.websocket_closed(reason);
    docs.leave_all().await;

    // The cancelled writer discards queued changes. A peer that does not read
    // must not hold its closing handshake forever.
    drop(connection);
    if !writer.is_finished()
        && tokio::time::timeout(WRITER_GRACE, &mut writer)
            .await
            .is_err()
    {
        writer.abort();
    }
}

/// Counts a connection whose session was gone before it was served, and tells the
/// page to come again when the session could not be read.
async fn close_missing_session_on_open(
    metrics: &Metrics,
    error: &AppError,
    sink: SplitSink<WebSocket, Message>,
) {
    if error.kind() == ErrorKind::Unauthorized {
        metrics.websocket_closed(Disconnect::SessionEnded);
    } else {
        tracing::warn!(%error, "a websocket connection ends: its session could not be read");
        metrics.websocket_closed(Disconnect::StoreUnavailable);
        writer::close(sink, Some(Disconnect::StoreUnavailable)).await;
    }
}

/// Reads the commands of a connection and carries them out, until the client closes
/// it, goes away, sends what is no websocket frame or nothing at all for the quiet
/// limit; returns which. It reads on while it carries one out
/// ([`Inbound::meanwhile`]).
async fn read_commands(
    app: &App,
    connection: &Connection,
    inbound: &mut Inbound<impl Stream<Item = Result<Message, axum::Error>> + Unpin>,
    docs: &mut Seat,
) -> Disconnect {
    // Held from a command that came until nothing read is left to carry out.
    #[cfg(feature = "test-util")]
    let mut working = None;
    loop {
        #[cfg(feature = "test-util")]
        if inbound.nothing_ahead() {
            working = None;
        }
        let came = inbound.next().await;
        #[cfg(feature = "test-util")]
        working.get_or_insert_with(|| connection.begin_work());
        let text = match came {
            Ok(Came::Text(text)) => text,
            Ok(Came::Binary(frame)) => {
                inbound.meanwhile(docs.frame(frame)).await;
                continue;
            }
            Err(reason) => return reason,
        };
        // A command is small, whatever a frame of a body edited together may be.
        if text.len() > crate::api::WEBSOCKET_MESSAGE_LIMIT {
            return Disconnect::ReadError;
        }
        if inbound.meanwhile(docs.command(&text)).await {
            continue;
        }
        if !inbound.meanwhile(connection.carry_out(app, &text)).await {
            return Disconnect::ClientClosed;
        }
    }
}
