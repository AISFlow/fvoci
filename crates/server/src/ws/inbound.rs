//! The reading side of a websocket connection: what the client sends, a `ping` of
//! which is answered at once (`pong`), and how long it may send nothing at all.
//!
//! A page cannot see the websocket's own Ping and Pong, so it asks with a `ping` of its
//! own whether its connection still carries (`contracts/websocket.md` 3). A connection
//! that sends nothing for its hub's quiet limit ([`QuietLimit`]: no message, no `ping`,
//! no Pong to the server's Ping) is taken for dead and closed, and so goes from its rooms.
//!
//! While the server does what the connection asked (a step of a room can wait for an
//! immediate check of its member, up to two intervals of `collab_recheck_seconds`), it
//! reads on: a `ping` is answered meanwhile, and what else comes is kept, in order, to
//! be done next, up to [`AHEAD_MESSAGES`] messages or [`AHEAD_BYTES`]. The quiet limit
//! runs only while the server waits for the client.

use std::borrow::Cow;
use std::collections::VecDeque;
use std::pin::pin;

use axum::extract::ws::{Message, Utf8Bytes};
use bytes::Bytes;
use futures_util::{Stream, StreamExt as _};
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::metrics::Disconnect;
use crate::realtime::quiet::QuietLimit;

/// How many messages are read ahead while the server does what the connection asked:
/// a minute of typing as a page sends it (at most 33 frames a second) is about two
/// thousand, and an empty message costs its place in the queue all the same.
const AHEAD_MESSAGES: usize = 4096;

/// How many bytes of messages are read ahead while the server does what the connection
/// asked: a minute of typing, as a page sends it, is a few hundred KiB. The message
/// that passes it is kept too, and then no more is read until the work is done.
const AHEAD_BYTES: usize = 1024 * 1024;

/// The longest text that is looked at as a `ping`: `{"type":"ping"}` with room for
/// spaces.
const LONGEST_PING: usize = 64;

/// The answer to a `ping`.
const PONG: &str = r#"{"type":"pong"}"#;

/// The `type` of a command, and nothing else of it.
#[derive(Deserialize)]
struct CommandType<'text> {
    #[serde(borrow, default, rename = "type")]
    kind: Cow<'text, str>,
}

/// What came from the client that is no `ping`, Ping or Pong.
#[derive(Debug)]
pub(super) enum Came {
    Text(Utf8Bytes),
    Binary(axum::body::Bytes),
}

/// One thing read of the stream: a message, or its end.
type Read = Option<Result<Message, axum::Error>>;

/// The reading side of one connection.
pub(super) struct Inbound<S> {
    stream: S,
    /// Where the answers to `ping` go: the connection's messages.
    outbox: mpsc::Sender<Bytes>,
    /// What was read while the server worked for the connection, in order.
    ahead: VecDeque<Read>,
    ahead_bytes: usize,
    /// When the client last sent anything.
    heard: Instant,
    quiet_limit: QuietLimit,
}

impl<S> Inbound<S>
where
    S: Stream<Item = Result<Message, axum::Error>> + Unpin,
{
    /// The reading side of `stream`, answering `ping` on `outbox`; a connection that
    /// sends nothing for `quiet_limit` ends.
    pub(super) fn new(stream: S, outbox: mpsc::Sender<Bytes>, quiet_limit: QuietLimit) -> Self {
        Self {
            stream,
            outbox,
            ahead: VecDeque::new(),
            ahead_bytes: 0,
            heard: Instant::now(),
            quiet_limit,
        }
    }

    /// The next message the connection is to carry out, or why it ends: the client
    /// closed it or went away, sent what is no websocket frame, or said nothing for the
    /// quiet limit.
    pub(super) async fn next(&mut self) -> Result<Came, Disconnect> {
        loop {
            let read = if let Some(read) = self.ahead.pop_front() {
                self.ahead_bytes = self.ahead_bytes.saturating_sub(payload_len(&read));
                read
            } else {
                let deadline = self.heard + self.quiet_limit.get();
                let Ok(read) = tokio::time::timeout_at(deadline, self.stream.next()).await else {
                    return Err(Disconnect::Quiet);
                };
                self.heard = Instant::now();
                if self.answer_if_ping(&read) {
                    continue;
                }
                read
            };
            match read {
                Some(Ok(Message::Text(text))) => return Ok(Came::Text(text)),
                Some(Ok(Message::Binary(frame))) => return Ok(Came::Binary(frame)),
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                Some(Ok(Message::Close(_))) | None => return Err(Disconnect::ClientClosed),
                Some(Err(_)) => return Err(Disconnect::ReadError),
            }
        }
    }

    /// Does `work`, reading on meanwhile: a `ping` is answered, anything else is kept
    /// for [`Self::next`] while there is room for it and the stream goes on. The quiet
    /// limit does not run meanwhile, and starts again once `work` is done.
    pub(super) async fn meanwhile<T>(&mut self, work: impl Future<Output = T>) -> T {
        let mut work = pin!(work);
        loop {
            let room = self.ahead.len() < AHEAD_MESSAGES
                && self.ahead_bytes < AHEAD_BYTES
                && !self.ended();
            tokio::select! {
                biased;
                done = &mut work => {
                    // The client is waited for again from now: it may have sent
                    // nothing while the server worked.
                    self.heard = Instant::now();
                    return done;
                }
                read = self.stream.next(), if room => {
                    self.heard = Instant::now();
                    if !self.answer_if_ping(&read) && !is_beat(&read) {
                        self.ahead_bytes += payload_len(&read);
                        self.ahead.push_back(read);
                    }
                }
            }
        }
    }

    /// Whether nothing that was read ahead waits to be carried out.
    #[cfg(feature = "test-util")]
    pub(super) fn nothing_ahead(&self) -> bool {
        self.ahead.is_empty()
    }

    /// Whether the stream's end is among what was read ahead: nothing comes after it.
    fn ended(&self) -> bool {
        self.ahead
            .back()
            .is_some_and(|read| !matches!(read, Some(Ok(Message::Text(_) | Message::Binary(_)))))
    }

    /// Answers `read` if it is a `ping`; whether it was one. An answer that finds the
    /// connection's messages full is not sent: the page lets go of the connection
    /// for that, and the hub closes it at the next message it cannot queue.
    fn answer_if_ping(&self, read: &Read) -> bool {
        let Some(Ok(Message::Text(text))) = read else {
            return false;
        };
        if !is_ping(text.as_str()) {
            return false;
        }
        let _ = self.outbox.try_send(Bytes::from_static(PONG.as_bytes()));
        true
    }
}

/// Whether `text` is the command `ping`.
fn is_ping(text: &str) -> bool {
    text.len() <= LONGEST_PING
        && serde_json::from_str::<CommandType<'_>>(text).is_ok_and(|command| command.kind == "ping")
}

/// Whether `read` is the websocket's own Ping or Pong: heard, and nothing to do.
fn is_beat(read: &Read) -> bool {
    matches!(read, Some(Ok(Message::Ping(_) | Message::Pong(_))))
}

fn payload_len(read: &Read) -> usize {
    match read {
        Some(Ok(Message::Text(text))) => text.len(),
        Some(Ok(Message::Binary(frame))) => frame.len(),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use std::pin::Pin;
    use std::time::Duration;

    use futures_util::stream;

    use super::*;
    use crate::realtime::quiet::QUIET_LIMIT;

    type Fed = mpsc::UnboundedSender<Result<Message, axum::Error>>;
    type Feeding = Pin<Box<dyn Stream<Item = Result<Message, axum::Error>> + Send>>;

    /// A reading side fed by the test, with the quiet limit of the server, and where
    /// its answers go.
    fn fed() -> (Inbound<Feeding>, Fed, mpsc::Receiver<Bytes>) {
        let (feed, received) = mpsc::unbounded_channel();
        let (outbox, answers) = mpsc::channel(8);
        let stream: Feeding = Box::pin(stream::unfold(received, async |mut received| {
            received.recv().await.map(|message| (message, received))
        }));
        let inbound = Inbound::new(stream, outbox, QuietLimit::default());
        (inbound, feed, answers)
    }

    #[expect(
        clippy::unnecessary_wraps,
        reason = "what a stream of a websocket yields"
    )]
    fn text(text: &str) -> Result<Message, axum::Error> {
        Ok(Message::Text(text.into()))
    }

    /// Just short of the limit.
    const ALMOST: Duration = QUIET_LIMIT.saturating_sub(Duration::from_millis(1));

    /// Whether `next` is still waiting after `after`.
    async fn waits(
        inbound: &mut Inbound<impl Stream<Item = Result<Message, axum::Error>> + Unpin>,
        after: Duration,
    ) -> bool {
        tokio::time::timeout(after, inbound.next()).await.is_err()
    }

    /// The limit the contract says (`contracts/websocket.md` 3, "죽은 연결"): past
    /// what a page in the background sends, a `ping` a minute or so apart, with
    /// thirty seconds for a slow network.
    /// It restates the constant on purpose: the contract and the page's waits are
    /// written for this number.
    #[test]
    fn the_limit_is_ninety_seconds() {
        assert_eq!(QUIET_LIMIT, Duration::from_secs(90));
    }

    /// A `ping` whose `type` is written with an escape is one; a longer text is not
    /// looked at, and neither is another command `type`.
    #[test]
    fn a_ping_is_its_type() {
        assert!(is_ping(r#"{"type":"ping"}"#));
        assert!(is_ping(r#"{ "type" : "pi\u006eg" }"#));
        assert!(!is_ping(r#"{"type":"pong"}"#));
        assert!(!is_ping(&format!(
            r#"{{"type":"ping","pad":"{}"}}"#,
            " ".repeat(64)
        )));
    }

    #[tokio::test(start_paused = true)]
    async fn a_connection_that_sends_nothing_ends_at_the_limit_and_not_before() {
        let (mut inbound, _feed, _answers) = fed();
        assert!(waits(&mut inbound, ALMOST).await);
        assert!(matches!(inbound.next().await, Err(Disconnect::Quiet)));
    }

    /// Anything heard starts the limit again: a `ping`, which is answered and not
    /// handed on, the websocket's own Pong, and a message.
    #[tokio::test(start_paused = true)]
    async fn what_is_heard_starts_the_limit_again() {
        let (mut inbound, feed, mut answers) = fed();
        for heard in [
            text(r#"{"type":"ping"}"#),
            Ok(Message::Pong(Vec::new().into())),
            Ok(Message::Ping(Vec::new().into())),
        ] {
            // Three of these are past the limit, which each starts again.
            tokio::time::sleep(Duration::from_secs(80)).await;
            feed.send(heard).expect("fed");
            assert!(
                waits(&mut inbound, Duration::from_secs(2)).await,
                "heard, and nothing to carry out"
            );
        }
        assert_eq!(answers.try_recv().ok().as_deref(), Some(PONG.as_bytes()));
        assert!(answers.try_recv().is_err(), "one ping, one pong");
        feed.send(text(r#"{"type":"subscribeWorkspace"}"#))
            .expect("fed");
        assert!(matches!(inbound.next().await, Ok(Came::Text(_))));
        assert!(waits(&mut inbound, ALMOST).await);
        assert!(matches!(inbound.next().await, Err(Disconnect::Quiet)));
    }

    /// While the server works for the connection, longer than the limit, a `ping` is
    /// answered at once, what else comes is kept in order, and the limit starts once
    /// the work is done.
    #[tokio::test(start_paused = true)]
    async fn a_ping_is_answered_while_the_server_works_for_the_connection() {
        let (mut inbound, feed, mut answers) = fed();
        feed.send(text(r#"{"type":"joinDoc"}"#)).expect("fed");
        feed.send(Ok(Message::Binary(vec![1].into()))).expect("fed");
        feed.send(text(r#"{"type":"ping"}"#)).expect("fed");
        feed.send(Ok(Message::Binary(vec![2].into()))).expect("fed");
        let work = async {
            tokio::time::sleep(QUIET_LIMIT * 3).await;
        };
        let answered_meanwhile = async {
            let answer = answers.recv().await;
            (tokio::time::Instant::now(), answer)
        };
        let began = tokio::time::Instant::now();
        let ((), (at, answer)) = tokio::join!(inbound.meanwhile(work), answered_meanwhile);
        assert_eq!(answer.as_deref(), Some(PONG.as_bytes()));
        assert!(at - began < Duration::from_secs(1), "answered at once");
        assert!(
            matches!(inbound.next().await, Ok(Came::Text(_))),
            "the command first"
        );
        for wanted in [1, 2] {
            match inbound.next().await {
                Ok(Came::Binary(frame)) => assert_eq!(frame.as_ref(), [wanted]),
                _ => panic!("the frames in order"),
            }
        }
        assert!(waits(&mut inbound, ALMOST).await);
        assert!(matches!(inbound.next().await, Err(Disconnect::Quiet)));
    }

    /// The end of the stream read while the server works is the end once what came
    /// before it is carried out; nothing is read after it.
    #[tokio::test(start_paused = true)]
    async fn the_end_read_meanwhile_comes_after_what_came_before_it() {
        let messages = vec![
            Ok(Message::Binary(vec![1].into())),
            Ok(Message::Close(None)),
        ];
        let (outbox, _answers) = mpsc::channel(8);
        let mut inbound = Inbound::new(
            stream::iter(messages).chain(stream::pending()),
            outbox,
            QuietLimit::default(),
        );
        inbound
            .meanwhile(tokio::time::sleep(Duration::from_secs(1)))
            .await;
        assert!(matches!(inbound.next().await, Ok(Came::Binary(_))));
        assert!(matches!(
            inbound.next().await,
            Err(Disconnect::ClientClosed)
        ));
    }
}
