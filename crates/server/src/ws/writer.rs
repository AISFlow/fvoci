//! The writing side of a websocket connection: queued changes, room messages and pings.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, Utf8Bytes, WebSocket, close_code};
use bytes::Bytes;
use futures_util::stream::SplitSink;
use futures_util::{Sink, SinkExt as _, Stream, StreamExt as _};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::collab::{Outbox, Outgoing};
use crate::metrics::Disconnect;

/// Writes until the connection ends. Cancellation discards queued changes before
/// closing. A send cancelled with an unflushed frame drops the transport instead.
pub(super) fn spawn(
    sink: SplitSink<WebSocket, Message>,
    inbox: mpsc::Receiver<Bytes>,
    docs: Outbox,
    ping_interval: Option<Duration>,
    closed: CancellationToken,
    closed_because: Arc<OnceLock<Disconnect>>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let ping = ping_interval.map(|period| {
            let mut interval = tokio::time::interval(period);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            interval
        });
        let messages = futures_util::stream::unfold(
            (inbox, docs, ping, true),
            async |(mut inbox, docs, mut ping, mut in_rooms)| {
                let message = next_message(&mut inbox, &docs, &mut ping, &mut in_rooms).await?;
                Some((message, (inbox, docs, ping, in_rooms)))
            },
        );
        futures_util::pin_mut!(messages);
        write(sink, messages, &closed, &closed_because).await;
    })
}

async fn next_message(
    inbox: &mut mpsc::Receiver<Bytes>,
    docs: &Outbox,
    ping: &mut Option<tokio::time::Interval>,
    in_rooms: &mut bool,
) -> Option<Message> {
    loop {
        return tokio::select! {
            message = inbox.recv() => match message.map(Utf8Bytes::try_from) {
                Some(Ok(text)) => Some(Message::Text(text)),
                // The hub queues only what it wrote as JSON.
                Some(Err(error)) => {
                    tracing::error!(%error, "websocket: a queued message is no text");
                    continue;
                }
                None => None,
            },
            outgoing = docs.next(), if *in_rooms => match outgoing {
                Some(Outgoing::Text(text)) => Some(Message::Text(text.into())),
                Some(Outgoing::Binary(frame)) => Some(Message::Binary(frame)),
                None => { *in_rooms = false; continue; }
            },
            () = next_tick(ping.as_mut()) => Some(Message::Ping(Vec::new().into())),
        };
    }
}

async fn write<S: Sink<Message> + Unpin>(
    mut sink: S,
    mut messages: impl Stream<Item = Message> + Unpin,
    closed: &CancellationToken,
    closed_because: &OnceLock<Disconnect>,
) {
    loop {
        let message = tokio::select! {
            biased;
            () = closed.cancelled() => break,
            message = messages.next() => match message { Some(message) => message, None => break },
        };
        tokio::select! {
            biased;
            // SinkExt::send may already have buffered this frame. Closing the sink
            // would flush it, so this path must drop it without sending a close frame.
            () = closed.cancelled() => return,
            result = sink.send(message) => if result.is_err() { return; },
        }
    }
    close(sink, closed_because.get().copied()).await;
}

/// Ends the connection, telling the page why when the reason has a close frame: 1008
/// for a session that ended, 1013 for what is to be read anew.
pub(super) async fn close<S>(mut sink: S, reason: Option<Disconnect>)
where
    S: Sink<Message> + Unpin,
{
    let frame = match reason {
        Some(Disconnect::SessionEnded) => Some((close_code::POLICY, "session ended")),
        Some(Disconnect::StoreUnavailable | Disconnect::ChecksOvertaken) => {
            Some((close_code::AGAIN, "try again"))
        }
        Some(
            Disconnect::ClientClosed
            | Disconnect::ReadError
            | Disconnect::SlowConsumer
            | Disconnect::EndedWorker
            | Disconnect::Quiet
            | Disconnect::Shutdown,
        )
        | None => None,
    };
    if let Some((code, reason)) = frame {
        let _ = sink
            .send(Message::Close(Some(CloseFrame {
                code,
                reason: reason.into(),
            })))
            .await;
    }
    let _ = sink.close().await;
}

/// Waits for the next tick of the interval, or forever if there is none.
async fn next_tick(interval: Option<&mut tokio::time::Interval>) {
    match interval {
        Some(interval) => {
            interval.tick().await;
        }
        None => std::future::pending().await,
    }
}

#[cfg(test)]
#[path = "writer_tests.rs"]
mod tests;
