//! One websocket connection's place among the rooms: what the websocket hands its
//! text commands and binary frames to, and takes what is to be sent from.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde::Deserialize;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::frame::{self, Carried, Received, Sync};
use super::messages::{self, reason_of};
use super::queue::Outgoing;
use super::rooms::{Connection, Entering, Holding};
use super::{Collab, caret, refusals};
use crate::app::{App, SessionKept};
use crate::error::AppError;

/// The longest identifier a command of the rooms may name: longer names nothing, and
/// is not answered, so that an answer never carries more than this of a command.
const LONGEST_ID: usize = 64;

/// A command of the rooms.
#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Command {
    #[serde(rename = "type")]
    kind: String,
    workspace_id: String,
    card_id: String,
    doc: u32,
    /// Of a `joinDoc`: the making of the document the connection holds, if it names
    /// one (`state_vector`).
    epoch: i64,
    /// Of a `joinDoc`: the state vector of the document the connection holds, in
    /// base64; empty for a connection that holds none.
    state_vector: String,
}

/// What a command or frame that is answered with a whole document counts as, in
/// frames: `joinDoc`, and a sync step 1.
const WHOLE_DOCUMENT: u64 = 5;

/// How many seconds' worth of frames a connection may have saved up at most while
/// the server was at its steps.
const MOST_SAVED_SECONDS: u64 = 60;

/// The clock [`FrameRate`] reads: real monotonic time normally, or a paused instant
/// advanced by tests under `test-util`.
#[derive(Debug, Default)]
pub(super) struct RateClock {
    #[cfg(feature = "test-util")]
    held: std::sync::Mutex<Option<Instant>>,
}

impl RateClock {
    /// The current instant of the frame-rate clock.
    #[cfg_attr(
        not(feature = "test-util"),
        expect(
            clippy::unused_self,
            reason = "reads `self.held` when `test-util` is enabled"
        )
    )]
    pub(super) fn now(&self) -> Instant {
        #[cfg(feature = "test-util")]
        if let Some(now) = *self.held.lock().expect("rate clock lock poisoned") {
            return now;
        }
        Instant::now()
    }

    /// Holds the clock at its current instant until advanced by [`Self::advance`].
    #[cfg(feature = "test-util")]
    pub(super) fn pause(&self) {
        let mut held = self.held.lock().expect("rate clock lock poisoned");
        held.get_or_insert_with(Instant::now);
    }

    /// Moves the held clock forward by `by`, pausing it first if it was not held.
    #[cfg(feature = "test-util")]
    pub(super) fn advance(&self, by: Duration) {
        let mut held = self.held.lock().expect("rate clock lock poisoned");
        let now = held.get_or_insert_with(Instant::now);
        *now += by;
    }
}

/// How much a connection may send of the rooms: `per_second` frames a second, and
/// as many at once after a pause.
///
/// It is counted when the server comes to a frame, not when the client sent it: the
/// time the server spent on the connection's own steps is time the client could send
/// in, and what it could have sent then is not lost to it, so that a client within
/// its rate is not past it because the server was slow.
#[derive(Debug)]
struct FrameRate {
    per_second: u32,
    /// The thousandths of a frame the connection may send now.
    left: u64,
    /// Up to when `left` is counted.
    counted_to: Instant,
    /// How long, since then, the server spent on the connection's steps.
    at_steps: Duration,
}

impl FrameRate {
    fn new(per_second: u32, now: Instant) -> Self {
        Self {
            per_second,
            left: u64::from(per_second) * 1000,
            counted_to: now,
            at_steps: Duration::ZERO,
        }
    }

    /// Counts `frames` more at `now`; whether they are within what the connection may
    /// send.
    fn admits(&mut self, frames: u64, now: Instant) -> bool {
        let rate = u64::from(self.per_second);
        let (full, most) = (rate * 1000, rate * 1000 * MOST_SAVED_SECONDS);
        // In whole milliseconds, each worth `rate` thousandths; the rest is counted
        // the next time.
        let millis = |time: Duration| u64::try_from(time.as_millis()).unwrap_or(u64::MAX);
        let passed = millis(now.saturating_duration_since(self.counted_to));
        let at_steps = millis(self.at_steps).min(passed);
        self.counted_to += Duration::from_millis(passed);
        self.at_steps = self
            .at_steps
            .saturating_sub(Duration::from_millis(at_steps));
        // What the connection did not send while it could is not saved past a
        // second's worth; what it could not send while the server was busy is.
        let while_free = (passed - at_steps).saturating_mul(rate);
        if self.left < full {
            self.left = self.left.saturating_add(while_free).min(full);
        }
        self.left = self
            .left
            .saturating_add(at_steps.saturating_mul(rate))
            .min(most);
        let cost = frames.saturating_mul(1000);
        let admitted = self.left >= cost;
        if admitted {
            self.left -= cost;
        }
        admitted
    }
}

/// The reading side of a connection among the rooms. Dropped, the connection is out
/// of every room.
#[derive(Debug)]
pub struct Seat {
    collab: Collab,
    /// The application as the connection's handshake reached it.
    home: App,
    connection: Arc<Connection>,
    /// The number the next room it comes into is known under on this connection.
    next_doc: u32,
    rate: FrameRate,
}

/// The writing side of a connection among the rooms.
#[derive(Debug)]
pub struct Outbox {
    connection: Arc<Connection>,
}

impl Outbox {
    /// The next message to send to the connection; `None` once it ended. Waiting for
    /// it may be given up at any time.
    pub async fn next(&self) -> Option<Outgoing> {
        self.connection.queue.next().await
    }
}

impl Collab {
    /// The place among the rooms of a websocket connection of the user `user_id`
    /// (empty for a connection without a session, which is let into no room), whose
    /// handshake reached `app`, of the session `making` keeps.
    #[must_use]
    pub fn seat(&self, app: &App, user_id: &str, making: SessionKept) -> (Seat, Outbox) {
        let connection = self.connection(user_id, making);
        let seat = Seat {
            collab: self.clone(),
            home: app.clone(),
            connection: Arc::clone(&connection),
            next_doc: 1,
            rate: FrameRate::new(self.frames_per_second(), self.rate_now()),
        };
        (seat, Outbox { connection })
    }

    fn rate_now(&self) -> Instant {
        self.inner.rooms.rate_clock.now()
    }

    /// Holds the clock that counts each connection's frame rate at its current instant
    /// until [`Collab::advance_frame_rate_clock`] moves it forward: for tests that
    /// exercise frame-rate limits without waiting on the wall clock.
    #[cfg(feature = "test-util")]
    #[doc(hidden)]
    pub fn pause_frame_rate_clock(&self) {
        self.inner.rooms.rate_clock.pause();
    }

    /// Moves the frame-rate clock forward by `by`, holding it first if it was not
    /// already paused ([`Collab::pause_frame_rate_clock`]).
    #[cfg(feature = "test-util")]
    #[doc(hidden)]
    pub fn advance_frame_rate_clock(&self, by: Duration) {
        self.inner.rooms.rate_clock.advance(by);
    }

    /// The most bytes of a binary frame a client sends that is not refused for its
    /// size alone: the largest update and the head of its frame.
    #[must_use]
    pub fn frame_bytes(&self) -> usize {
        usize::try_from(self.inner.limits.update_bytes)
            .unwrap_or(usize::MAX)
            .saturating_add(frame::HEAD_ROOM)
    }
}

impl Seat {
    /// Cancelled when the connection is to be closed: it reads nothing of what it is
    /// sent, not even that it was put out of its rooms.
    #[must_use]
    pub fn overrun(&self) -> CancellationToken {
        self.connection.queue.overrun().clone()
    }

    /// Cancelled when the connection is to be closed: an update it sent ended the
    /// worker that took it, and its sender is held back for a while.
    #[must_use]
    pub fn ended_worker(&self) -> CancellationToken {
        self.connection.ended_worker.clone()
    }

    /// Runs a step of a room in a task of its own and waits for it: a connection that
    /// ends meanwhile leaves the step done whole.
    async fn step<T, F>(&mut self, work: F) -> Option<T>
    where
        T: Send + 'static,
        F: Future<Output = T> + Send + 'static,
    {
        let began = self.collab.rate_now();
        let done = self.home.spawn_tracked(work).await;
        self.rate.at_steps += self.collab.rate_now().saturating_duration_since(began);
        match done {
            Ok(done) => Some(done),
            Err(error) => {
                tracing::error!(%error, "a step of a room ended");
                None
            }
        }
    }

    /// Carries out a text message if it is a command of the rooms (`joinDoc`,
    /// `leaveDoc`); whether it was one.
    pub async fn command(&mut self, text: &str) -> bool {
        let Ok(command) = serde_json::from_str::<Command>(text) else {
            return false;
        };
        match command.kind.as_str() {
            "joinDoc" => self.join(command).await,
            "leaveDoc" => self.leave(command.doc).await,
            _ => return false,
        }
        true
    }

    async fn join(&mut self, command: Command) {
        let Command {
            workspace_id,
            card_id,
            epoch,
            state_vector,
            ..
        } = command;
        let now = self.collab.rate_now();
        let admitted = self.rate.admits(WHOLE_DOCUMENT, now);
        if workspace_id.len() > LONGEST_ID || card_id.len() > LONGEST_ID {
            tracing::debug!("websocket: a joinDoc names what is no identifier");
            return;
        }
        let doc = self.next_doc;
        let entered = if !admitted {
            Some(Err(refusals::too_many_frames(self.rate.per_second)))
        } else if let Some(next) = doc.checked_add(1) {
            self.next_doc = next;
            let (collab, home) = (self.collab.clone(), self.home.clone());
            let connection = Arc::clone(&self.connection);
            let (workspace, card) = (workspace_id.clone(), card_id.clone());
            let holding = Holding::of(epoch, &state_vector);
            // Boxed: what letting a connection in holds is large, and is no part of
            // what serving the connection holds.
            self.step(Box::pin(async move {
                let entering = Entering {
                    workspace_id: &workspace,
                    card_id: &card,
                    holding,
                };
                collab.enter(&connection, &home, doc, entering).await
            }))
            .await
        } else {
            None
        };
        let refused = match entered {
            Some(Ok(())) => return,
            Some(Err(error)) => error,
            None => AppError::internal("the connection cannot come into a room"),
        };
        refused.log();
        self.connection.queue.notice(
            None,
            messages::refused(&workspace_id, &card_id, &reason_of(&refused)),
        );
    }

    async fn leave(&mut self, doc: u32) {
        // Counted, and carried out whatever the count says: it makes room.
        let now = self.collab.rate_now();
        let _ = self.rate.admits(1, now);
        let Some(room) = self.connection.forget(doc) else {
            return;
        };
        let (collab, connection) = (self.collab.clone(), Arc::clone(&self.connection));
        self.step(async move { collab.leave(&room, &connection, doc).await })
            .await;
    }

    /// Carries out a binary frame: a sync message or a caret for one of the
    /// connection's rooms. A frame for a room the connection is not in (it left, or
    /// was put out and told so), and one that is no frame of this shape, is dropped.
    pub async fn frame(&mut self, frame: Bytes) {
        // Counted whatever it is, and whatever it is for: a frame that is none, or
        // for no room of the connection's, costs a count too.
        let read = frame::room_of(frame).map(|(doc, rest)| (doc, frame::received(rest)));
        let asks_whole = |received: &Received| {
            matches!(
                received.carried,
                Carried::Sync {
                    sync: Sync::StateVector,
                    ..
                }
            )
        };
        let cost = match &read {
            Some((_, Some(received))) if asks_whole(received) => WHOLE_DOCUMENT,
            _ => 1,
        };
        let now = self.collab.rate_now();
        let admitted = self.rate.admits(cost, now);
        let Some((doc, received)) = read else {
            return;
        };
        let Some(room) = self.connection.room(doc) else {
            return;
        };
        let (collab, connection) = (self.collab.clone(), Arc::clone(&self.connection));
        match received {
            Some(Received {
                epoch,
                carried: Carried::Carets(update),
            }) => {
                // One past the rate, or larger than a caret is, is dropped: the room
                // missed nothing of the document, and a caret is said again.
                if admitted && update.len() <= caret::MOST_BYTES {
                    self.step(async move {
                        collab.caret(&room, &connection, doc, epoch, update).await;
                    })
                    .await;
                }
            }
            _ if !admitted => {
                // What the frame carried is lost to the room: the connection is out
                // of step with it, and comes in anew.
                let reason = reason_of(&refusals::too_many_frames(self.rate.per_second));
                self.step(async move { collab.put_out(&room, &connection, doc, &reason).await })
                    .await;
            }
            Some(Received {
                epoch,
                carried: Carried::Sync { sync, payload },
            }) => {
                self.step(async move {
                    match sync {
                        Sync::StateVector => {
                            collab
                                .missing(&room, &connection, doc, epoch, payload)
                                .await;
                        }
                        Sync::Missing | Sync::Update => {
                            collab.update(&room, &connection, doc, epoch, payload).await;
                        }
                    }
                })
                .await;
            }
            None => tracing::debug!(doc, "websocket: a binary frame that is no frame of a room"),
        }
    }

    /// Takes the connection out of every room, for good: it ended. The rooms are
    /// left in one step, which is done whole also if the wait for it is given up.
    pub async fn leave_all(&mut self) {
        self.connection.queue.close();
        let rooms = self.connection.end();
        if rooms.is_empty() {
            return;
        }
        let (collab, connection) = (self.collab.clone(), Arc::clone(&self.connection));
        self.step(async move {
            for (doc, room) in rooms {
                collab.leave(&room, &connection, doc).await;
            }
        })
        .await;
    }
}

impl Drop for Seat {
    /// A connection whose serving was given up before it left its rooms leaves them
    /// here.
    fn drop(&mut self) {
        self.connection.queue.close();
        let rooms = self.connection.end();
        if rooms.is_empty() {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            tracing::error!("a connection ended outside the runtime; its rooms keep it");
            return;
        };
        let (collab, connection) = (self.collab.clone(), Arc::clone(&self.connection));
        runtime.spawn(async move {
            for (doc, room) in rooms {
                collab.leave(&room, &connection, doc).await;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A connection sends as many frames at once as it may in a second, and then
    /// one more with every part of a second that passes; a command or frame that is
    /// answered with a whole document counts as five.
    #[test]
    fn frames_are_admitted_at_their_rate() {
        let mut now = Instant::now();
        let mut rate = FrameRate::new(10, now);
        assert!((0..10).all(|_| rate.admits(1, now)), "ten at once");
        assert!(!rate.admits(1, now), "and no eleventh");
        now += Duration::from_millis(99);
        assert!(!rate.admits(1, now), "not yet a tenth of a second");
        now += Duration::from_millis(1);
        assert!(rate.admits(1, now), "one more");
        assert!(!rate.admits(1, now), "and only one");
        // Parts of a millisecond are not lost between two counts.
        for _ in 0..200 {
            now += Duration::from_micros(500);
            let _ = rate.admits(1, now);
        }
        now += Duration::from_millis(500);
        assert!(rate.admits(WHOLE_DOCUMENT, now), "five after half a second");
        assert!(!rate.admits(1, now), "which were all");
        // A pause saves a second's worth and no more.
        now += Duration::from_secs(30);
        assert!(
            rate.admits(WHOLE_DOCUMENT, now) && rate.admits(WHOLE_DOCUMENT, now),
            "ten again"
        );
        assert!(!rate.admits(1, now), "and no more for the wait");
        // A timestamp earlier than `counted_to` does not panic or add budget.
        let earlier = now.checked_sub(Duration::from_secs(1)).unwrap_or(now);
        assert!(!rate.admits(1, earlier), "earlier timestamp adds no budget");
    }

    /// What a client sent within its rate while the server was at the connection's
    /// steps is admitted when the server comes to it, all at once.
    #[test]
    fn the_time_of_the_servers_steps_is_not_the_clients() {
        let mut now = Instant::now();
        let mut rate = FrameRate::new(10, now);
        assert!((0..10).all(|_| rate.admits(1, now)), "ten at once");
        // A step of six seconds, during which sixty frames came in.
        now += Duration::from_secs(6);
        rate.at_steps += Duration::from_secs(6);
        assert!((0..60).all(|_| rate.admits(1, now)), "sixty that waited");
        assert!(!rate.admits(1, now), "and no more");
        // However long the server took, no more than a minute's worth is saved.
        now += Duration::from_secs(600);
        rate.at_steps += Duration::from_secs(600);
        assert!((0..600).all(|_| rate.admits(1, now)), "a minute's worth");
        assert!(!rate.admits(1, now), "and no more");
    }

    /// A paused rate clock holds its instant across reads and moves only when advanced.
    #[cfg(feature = "test-util")]
    #[test]
    fn paused_rate_clock_moves_only_when_advanced() {
        let clock = RateClock::default();
        clock.pause();
        let start = clock.now();
        assert_eq!(clock.now(), start, "held at the paused instant");
        clock.pause();
        assert_eq!(clock.now(), start, "pausing again keeps the same instant");
        clock.advance(Duration::from_millis(250));
        assert_eq!(
            clock.now().duration_since(start),
            Duration::from_millis(250),
            "advanced by the requested duration"
        );

        // Advancing an unpaused clock pauses it first and holds the advanced instant.
        let unpaused = RateClock::default();
        let before = unpaused.now();
        unpaused.advance(Duration::from_millis(100));
        let advanced = unpaused.now();
        assert!(
            advanced.saturating_duration_since(before) >= Duration::from_millis(100),
            "advanced at least the requested duration"
        );
        assert_eq!(unpaused.now(), advanced, "stays held after auto-pausing");
    }
}
