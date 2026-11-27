//! The rooms: the connections that hold the document of one card's body together.
//!
//! One at a time works on a room (its turn): a connection that comes in is sent the
//! whole document, an update is taken into the document and logged
//! ([`Collab::take`]) and only then sent to the others, and what is sent anew
//! after the document was made again is sent to all, in the order the log has. A
//! room that the last connection left closes its document, which writes its body to
//! the card.
//!
//! While a room is there a task of its own hears what happens to its document
//! ([`Collab::subscribe`]): made again, or opened again after its worker ended, the
//! document is sent to everyone in the room anew. Another asks again, every
//! `collab_recheck_seconds`, whether each connection may be in the room, and how
//! ([`admission::admit`]): who may no longer read is put out, and who may no longer
//! write reads from then on. Permission-changing operations also ask immediately
//! ([`Collab::recheck_access`]). When an organization begins to take only sessions made
//! through its provider, whoever it shuts out is put out of its rooms at once
//! ([`Collab::put_out_shut_out`]).
//!
//! Every step of a room runs in a task of its own that the caller waits for
//! (`Seat`): a connection that ends while its update is taken leaves the update
//! logged and sent to the others, never logged and not sent.

mod access;
mod carets;
mod held;
mod index;
mod joining;
#[cfg(feature = "test-util")]
mod pauses;
mod removal;
mod sent;
mod turn_lock;
mod walls;

pub(crate) use access::AccessScope;
pub(super) use joining::{Entering, Holding};
pub(crate) use walls::{AccessChange, Wall};

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::frame::{self, Sync};
use super::messages::{self, reason_of};
use super::queue::{Capacity, NotQueued, Queue, Queued};
use super::versions::Left;
use super::{Collab, DocChange, DocEvent, Loaded, admission, refusals};
use crate::app::{App, Asker, SessionKept};
use crate::config::collab::CollabSettings;
use fvoci_store::{Making, SessionTaken};

use crate::error::{AppError, Reason};
use crate::permissions::DocMode;

/// How long after a room could not be sent its document anew, because the document
/// cannot be worked on for now, it is tried again.
const SEND_ANEW_AGAIN: Duration = Duration::from_secs(2);

/// How long after the store did not answer of somebody in a room it is asked of
/// them once more, before they are put out.
const ASKED_ONCE_MORE_AFTER: Duration = Duration::from_secs(2);

/// How many times a connection coming into a room is admitted again because a change
/// that reaches the room and its user came meanwhile, before it is refused as when the
/// document cannot be worked on.
const ADMITTED_AT_MOST: usize = 3;

/// How many users of a room the store is asked about at once, when the room asks
/// again who may be in it.
const ASKED_AT_ONCE: usize = 8;

/// How many times the bytes of the largest document may wait for one connection, of
/// all its rooms: a document whole, and as much again of what came after it.
const QUEUE_DOCUMENTS: usize = 2;

/// The rooms there are.
#[derive(Debug)]
pub(super) struct Rooms {
    /// By organization and board.
    index: Mutex<index::Index>,
    /// How often a room asks again whether its members may be in it.
    recheck: Duration,
    /// In how many rooms a connection may be.
    per_connection: usize,
    /// How many connections may be in a room.
    per_room: usize,
    /// How many of the connections in a room may be one user's.
    per_user: usize,
    /// How many commands and frames a connection may send in a second.
    frames_per_second: u32,
    /// The clock that counts each connection's frame rate.
    pub(super) rate_clock: super::seat::RateClock,
    /// How many messages may wait for a connection, of all its rooms.
    queue_messages: usize,
    /// The number of the next connection.
    next_connection: AtomicU64,
    /// When access facts changed: admissions and periodic answers read before a
    /// change of their organization cannot survive its immediate check.
    walls: walls::Walls,
    /// Where the tests stop the rooms.
    #[cfg(feature = "test-util")]
    pauses: pauses::Pauses,
    /// Cancelled when the server stops: nobody comes in any more, no update is taken,
    /// and the tasks of the rooms end.
    stopping: CancellationToken,
}

impl Rooms {
    /// No rooms, held to what `settings` say.
    pub(super) fn new(settings: &CollabSettings) -> Self {
        Self {
            index: Mutex::new(index::Index::default()),
            recheck: Duration::from_secs(settings.collab_recheck_seconds),
            per_connection: as_count(settings.collab_documents_per_connection),
            per_room: as_count(settings.collab_connections_per_document),
            per_user: as_count(settings.collab_connections_per_user),
            frames_per_second: settings.collab_frames_per_second,
            rate_clock: super::seat::RateClock::default(),
            queue_messages: as_count(settings.collab_queue_messages),
            next_connection: AtomicU64::new(0),
            walls: walls::Walls::new(Duration::from_secs(settings.collab_recheck_seconds)),
            #[cfg(feature = "test-util")]
            pauses: pauses::Pauses::default(),
            stopping: CancellationToken::new(),
        }
    }
}

/// Waits until `at`, or for ever if there is no such time.
async fn wait_until(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

/// A setting that counts things, as the count of a collection.
fn as_count(setting: u32) -> usize {
    usize::try_from(setting).unwrap_or(usize::MAX)
}

/// A connection as the rooms know it.
#[derive(Debug)]
pub(super) struct Connection {
    id: u64,
    /// The user of the connection's session; empty for a connection without one.
    pub(super) user_id: String,
    /// How the connection's session was made, and what it confirmed as it came in
    /// or since, as the websocket connection keeps it too: that of a password for a
    /// connection without a session.
    pub(super) making: SessionKept,
    pub(super) queue: Queue,
    joined: Mutex<Joined>,
    /// Cancelled when the connection is to be closed: an update it sent ended the
    /// worker that took it.
    pub(super) ended_worker: CancellationToken,
}

/// The rooms a connection is in.
#[derive(Debug, Default)]
struct Joined {
    /// By the number each is known under on this connection.
    rooms: HashMap<u32, Arc<Room>>,
    /// The connection ended: it comes into no room any more.
    ended: bool,
}

impl Connection {
    /// Whose the connection is, for what is asked of an organization for it.
    pub(super) fn asker(&self) -> Asker<'_> {
        Asker {
            user_id: &self.user_id,
            making: self.making.making(),
            delegation: None,
        }
    }

    /// The room the connection knows as `doc`, if it is in it.
    pub(super) fn room(&self, doc: u32) -> Option<Arc<Room>> {
        self.joined().rooms.get(&doc).map(Arc::clone)
    }

    /// Takes the room the connection knows as `doc` from those it is in.
    pub(super) fn forget(&self, doc: u32) -> Option<Arc<Room>> {
        self.joined().rooms.remove(&doc)
    }

    /// Whether the connection is in as many rooms as `limit` and in none of the card
    /// `card_id`: it comes into no room of that card, whatever else is asked.
    fn is_full_without(&self, limit: usize, card_id: &str) -> bool {
        let joined = self.joined();
        joined.rooms.len() >= limit && !joined.rooms.values().any(|room| room.card_id == card_id)
    }

    /// Takes every room from those it is in, for good: the connection ended, and a
    /// step that would let it into a room after this finds that it did.
    pub(super) fn end(&self) -> Vec<(u32, Arc<Room>)> {
        let mut joined = self.joined();
        joined.ended = true;
        joined.rooms.drain().collect()
    }

    /// Counts a room among those the connection is in, as `doc`; `false` if the
    /// connection ended, and is in no room.
    fn join(&self, doc: u32, room: &Arc<Room>) -> bool {
        let mut joined = self.joined();
        if joined.ended {
            return false;
        }
        joined.rooms.insert(doc, Arc::clone(room));
        true
    }

    fn joined(&self) -> std::sync::MutexGuard<'_, Joined> {
        self.joined
            .lock()
            .expect("the rooms of a connection are never held across a panic")
    }
}

/// The connections that hold one card's document.
#[derive(Debug)]
pub(super) struct Room {
    /// A deletion permanently disconnects this room from any restored card.
    deleted: AtomicBool,
    card_id: String,
    /// The board the card was on as the room was made: a card is not moved to another
    /// board while it has a room.
    container_id: String,
    /// The workspace the room's first connection named: the board's, as admission
    /// lets nobody in through another ([`admission::admit`]).
    workspace_id: String,
    /// The application as the card's organization reaches it.
    app: App,
    turn: turn_lock::TurnLock,
    /// Told when an immediate check answered of members it held.
    checked: tokio::sync::Notify,
    /// Cancelled when the room is closed: its task ends.
    gone: CancellationToken,
}

/// What the one that works on a room holds.
#[derive(Debug, Default)]
struct Turn {
    /// Which making of the document the members hold.
    epoch: i64,
    members: Vec<Member>,
    /// The room is no longer among the rooms: who waited for it looks again.
    closed: bool,
    /// The users whose leaving has the room ask for what was typed to be kept as a
    /// version of the card's body: those whose updates it took since it last asked,
    /// and, once it asked while one who typed stayed, all who were there to write.
    typed: Vec<String>,
}

#[derive(Debug)]
struct Member {
    connection: Arc<Connection>,
    /// The number the connection knows the room under.
    doc: u32,
    /// The workspace the connection named as it came in: the card's board is of it.
    workspace_id: String,
    mode: DocMode,
    /// The number of the change whose immediate check holds the member, if one does:
    /// what they send waits for its answer, and what the room sends waits for them.
    checking: Option<u64>,
    /// What the room did not send them while a check held them.
    missed: Option<held::Missed>,
    /// Where they are in the document, once they said so.
    caret: Option<carets::Caret>,
    /// The carets that went while a check held them, if any was told meanwhile.
    carets_missed: Option<carets::CaretsMissed>,
}

/// A member as a room asks about it again.
#[derive(Debug)]
struct Asked {
    connection: u64,
    doc: u32,
    who: Who,
}

/// Whom the store is asked about: a user, how their session was made and what it
/// confirmed, and the workspace they named as they came in. One answer is shared by
/// those of whom all three are the same, and by no others.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct Who {
    user_id: String,
    making: Making,
    workspace_id: String,
}

impl Who {
    /// Who asks, for what is asked of an organization for them.
    fn asker(&self) -> Asker<'_> {
        Asker {
            user_id: &self.user_id,
            making: self.making.as_deref(),
            delegation: None,
        }
    }
}

/// What the store answered of one: how they are in the room, `None` for who is not
/// let in; or the reason it could not be asked.
type Answer = Result<Option<DocMode>, String>;

impl Turn {
    /// How many of the members are connections of the user.
    fn of_user(&self, user_id: &str) -> usize {
        self.members
            .iter()
            .filter(|member| member.connection.user_id == user_id)
            .count()
    }

    /// Takes the connection out of the room, under whatever number it knew it as, with
    /// what waited for it there: it comes in anew.
    fn forget_member(&mut self, connection: &Connection) {
        let at = self
            .members
            .iter()
            .position(|member| member.connection.id == connection.id);
        if let Some(index) = at {
            let before = self.take_out(index);
            connection.forget(before.doc);
            connection.queue.forget_frames(before.doc);
        }
    }

    fn index_of(&self, connection: &Connection, doc: u32) -> Option<usize> {
        self.members
            .iter()
            .position(|member| member.connection.id == connection.id && member.doc == doc)
    }

    /// Puts a member out of the room and tells its connection why, in place of the
    /// sync messages that waited for it of the room and after the answers it was
    /// given there.
    fn put_out(&mut self, index: usize, reason: &str) {
        let member = self.take_out(index);
        member.connection.forget(member.doc);
        member
            .connection
            .queue
            .notice(Some(member.doc), messages::left(member.doc, reason));
    }

    /// Queues what `make` makes for each member `to` picks. A member whose connection
    /// has no room for it is put out: it does not read, or it ended, and the others
    /// go on.
    fn send<const N: usize>(
        &mut self,
        to: impl Fn(&Member) -> bool,
        make: impl Fn(&Member) -> [Queued; N],
    ) {
        self.queue_for_each(|member| {
            if to(member) {
                member.connection.queue.push(make(member))
            } else {
                Ok(())
            }
        });
    }

    /// Queues the answer to something a connection sent (`docAck`, `docNack`) for
    /// it, whatever waits for it: the answer to an update that is logged is never
    /// lost. A connection for which it was one too many is put out after it.
    fn answer(&mut self, connection: &Connection, doc: u32, answer: String) {
        let Some(index) = self.index_of(connection, doc) else {
            return;
        };
        if connection.queue.answer(Queued::text(doc, answer)).is_err() {
            self.put_out(index, Reason::ConnectionTooSlow.as_str());
        }
    }

    /// Has `queue` queue something for each member, and puts out who has no room
    /// for it.
    fn queue_for_each(&mut self, queue: impl Fn(&Member) -> Result<(), NotQueued>) {
        let mut index = 0;
        while index < self.members.len() {
            if queue(&self.members[index]).is_err() {
                self.put_out(index, Reason::ConnectionTooSlow.as_str());
            } else {
                index += 1;
            }
        }
    }

    /// Sends every member the document anew, in place of the sync messages that
    /// waited for them: those were of the document before, or are in this one.
    /// A member a check holds is sent it once the check keeps them.
    fn send_anew(&mut self, loaded: &Loaded) {
        if self.epoch != loaded.epoch {
            self.forget_carets();
        }
        self.epoch = loaded.epoch;
        self.miss(&held::Missed::Whole);
        self.push_anew(loaded, |member| !member.is_held());
    }

    /// Sends the members `to` picks the document anew, in place of the sync messages
    /// that waited for them of the room.
    fn push_anew(&mut self, loaded: &Loaded, to: impl Fn(&Member) -> bool) {
        let epoch = loaded.epoch;
        let whole = frame::message(Sync::Missing, &loaded.state);
        self.queue_for_each(|member| {
            if !to(member) {
                return Ok(());
            }
            let doc = member.doc;
            let anew = [
                Queued::text(doc, messages::reset(doc, epoch)),
                Queued::sync(doc, epoch, &whole),
            ];
            member.connection.queue.push_anew(doc, epoch, anew)
        });
    }
}

impl Collab {
    /// A connection of the user `user_id` for the rooms, in none yet, of the session
    /// `making` keeps: the same keeping as the websocket connection's, so that what
    /// the session confirms while it lasts is confirmed here too.
    pub(super) fn connection(&self, user_id: &str, making: SessionKept) -> Arc<Connection> {
        let rooms = &self.inner.rooms;
        Arc::new(Connection {
            id: rooms.next_connection.fetch_add(1, Ordering::Relaxed),
            user_id: user_id.to_owned(),
            making,
            queue: Queue::new(Capacity {
                messages: rooms.queue_messages,
                bytes: usize::try_from(self.inner.limits.state_bytes)
                    .unwrap_or(usize::MAX)
                    .saturating_mul(QUEUE_DOCUMENTS),
            }),
            joined: Mutex::new(Joined::default()),
            ended_worker: CancellationToken::new(),
        })
    }

    /// Nobody comes into a room any more, and the rooms take no update: the server
    /// stops.
    pub(super) fn end_rooms(&self) {
        self.inner.rooms.stopping.cancel();
    }

    /// How many commands and frames of the rooms a connection may send in a second.
    pub(super) fn frames_per_second(&self) -> u32 {
        self.inner.rooms.frames_per_second
    }

    fn rooms_ended(&self) -> bool {
        self.inner.rooms.stopping.is_cancelled()
    }

    /// Whether the room of a card or a page of `app`'s organization is open now.
    pub(super) fn room_is_open(&self, app: &App, card_id: &str) -> bool {
        self.rooms_index()
            .get(app.store().tenant(), card_id)
            .is_some()
    }

    /// The rooms there are, held.
    fn rooms_index(&self) -> std::sync::MutexGuard<'_, index::Index> {
        self.inner
            .rooms
            .index
            .lock()
            .expect("the rooms are never held across a panic")
    }

    /// How many rooms there are, for the tests of what a connection leaves behind.
    #[cfg(feature = "test-util")]
    #[doc(hidden)]
    #[must_use]
    pub fn rooms(&self) -> usize {
        self.rooms_index().len()
    }

    /// The room of a card of `app`'s organization, made if there is none, with its
    /// two tasks: the one that hears what happens to its document, and the one that
    /// asks again whether its members may be in it.
    fn room(&self, admitted: &admission::Admitted, workspace_id: &str, card_id: &str) -> Arc<Room> {
        let app = &admitted.app;
        let tenant = app.store().tenant();
        let mut rooms = self.rooms_index();
        if let Some(room) = rooms.get(tenant, card_id) {
            return Arc::clone(room);
        }
        let room = Arc::new(Room {
            deleted: AtomicBool::new(false),
            card_id: card_id.to_owned(),
            container_id: admitted.container_id.clone(),
            workspace_id: workspace_id.to_owned(),
            app: app.clone(),
            turn: turn_lock::TurnLock::default(),
            checked: tokio::sync::Notify::new(),
            gone: CancellationToken::new(),
        });
        rooms.insert(tenant, Arc::clone(&room));
        drop(rooms);
        // Subscribed before anyone is in the room: whatever happens to the document
        // from its first opening on is heard.
        let events = self.subscribe();
        tokio::spawn(self.clone().watch(Arc::clone(&room), events));
        tokio::spawn(self.clone().ask_again(Arc::clone(&room)));
        room
    }

    /// Hears what happens to the document of a room for as long as the room is
    /// there, and has the room sent the document anew.
    async fn watch(self, room: Arc<Room>, mut events: broadcast::Receiver<DocEvent>) {
        let tenant = room.app.store().tenant();
        let stopping = self.inner.rooms.stopping.clone();
        // When the room is tried to be sent its document again, which could not be
        // opened: kept across what is heard of other cards meanwhile.
        let mut again_at: Option<Instant> = None;
        loop {
            let heard = tokio::select! {
                () = room.gone.cancelled() => return,
                () = stopping.cancelled() => return,
                () = wait_until(again_at) => None,
                event = events.recv() => match event {
                    Ok(event) if event.tenant == tenant && event.card_id == room.card_id => {
                        Some(event.change)
                    }
                    Ok(_) => continue,
                    // Some were missed: the document may be another by now.
                    Err(RecvError::Lagged(_)) => None,
                    Err(RecvError::Closed) => return,
                },
            };
            let mut turn = room.turn.lock().await;
            // The server stops: the documents are written and closed, and none is
            // opened again for a room whose connections are about to end.
            if turn.closed || self.rooms_ended() {
                return;
            }
            // Nobody yet: who comes in opens the document as it is.
            if turn.members.is_empty() {
                continue;
            }
            again_at = if self.anew(&room, &mut turn, heard).await {
                None
            } else {
                Some(Instant::now() + SEND_ANEW_AGAIN)
            };
            self.shut_if_empty(&room, &mut turn).await;
        }
    }

    /// Asks again, every `collab_recheck_seconds`, whether each member of a room may
    /// be in it, for as long as the room is there. A task of its own: the room's
    /// other task hears of its document meanwhile. Built with `test-util`, it also
    /// asks whenever a test has every room ask now after the room was made: it
    /// listens from before its task first runs.
    #[cfg_attr(
        not(feature = "test-util"),
        expect(
            clippy::manual_async_fn,
            reason = "built with `test-util`, it listens before its future is first polled"
        )
    )]
    fn ask_again(self, room: Arc<Room>) -> impl Future<Output = ()> {
        #[cfg(feature = "test-util")]
        let mut asked_now = self.asked_now();
        async move {
            let mut due = tokio::time::interval(self.inner.rooms.recheck);
            due.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            // The first tick is at once; who is in the room was asked as they came in.
            due.tick().await;
            let stopping = self.inner.rooms.stopping.clone();
            loop {
                #[cfg(feature = "test-util")]
                let now = asked_now.changed();
                #[cfg(not(feature = "test-util"))]
                let now = std::future::pending::<Result<(), ()>>();
                tokio::select! {
                    () = room.gone.cancelled() => return,
                    () = stopping.cancelled() => return,
                    _ = due.tick() => {}
                    Ok(()) = now => {}
                }
                // Given up when the server stops: nothing is asked of a store that is
                // about to close.
                tokio::select! {
                    () = stopping.cancelled() => return,
                    () = self.recheck(&room) => {}
                }
            }
        }
    }

    /// Puts out of the rooms of the organization of `app` (a handle of one
    /// organization) every connection whose session was not made through `provider_id`,
    /// as the organization begins to take only sessions made through that provider: the
    /// wall's own rule ([`SessionTaken::opened_by`]), so nothing is asked of the store. It waits
    /// for each room's turn, nothing more: a room it empties is closed by a task of its
    /// own. For after that is stored, and before the request that stored it answers:
    /// what such a connection sends after that is taken by no room. A connection whose
    /// admission was answered before is admitted again as it comes in
    /// ([`Self::enter`]), so none is let in past it.
    pub(crate) async fn put_out_shut_out(&self, app: &App, provider_id: &str) {
        let rooms = &self.inner.rooms;
        let tenant = app.store().tenant();
        // Before the rooms are read: a connection that comes into one later sees it.
        rooms.walls.raise(tenant, AccessScope::organization());
        let of_organization: Vec<Arc<Room>> =
            self.rooms_index().of_tenant(tenant).cloned().collect();
        // As for a card that is not there: the organization is told to nobody.
        let reason = reason_of(&AppError::not_found("card"));
        let takes = SessionTaken::Through(provider_id.to_owned());
        for room in of_organization {
            let mut turn = room.turn.lock().await;
            let before = turn.members.len();
            // From the last: a member put out takes the place of none not yet looked at.
            for index in (0..turn.members.len()).rev() {
                let making = turn.members[index].connection.making.making();
                if !takes.opened_by(making) {
                    turn.put_out(index, &reason);
                }
            }
            if turn.members.len() < before {
                drop(turn);
                // Closing writes the body to the card, and so does keeping what one
                // who was put out typed: not for the request to wait on.
                let collab = self.clone();
                tokio::spawn(async move {
                    let mut turn = room.turn.lock().await;
                    collab.shut_if_empty(&room, &mut turn).await;
                });
            }
        }
    }

    /// Lets a connection into the room of a card of a board of `workspace_id`, to be
    /// known there as `doc`, and sends it `docJoined` and the document: the whole of
    /// it, or what the connection lacks of the document it says it holds
    /// ([`Self::first_of`]). A connection that is in the room already is in it anew:
    /// what it knew the room as before is no room of its any more. `home` is the
    /// application as the connection's handshake reached it.
    ///
    /// # Errors
    ///
    /// Returns a not-found error for who is not let in ([`admission::admit`]), which
    /// is also what a card that is not there answers, the errors of opening the
    /// document ([`Collab::open`]), and `connection_too_slow` for a connection that
    /// has no room for the document among what waits for it. A connection that is
    /// refused stays in the room as it was.
    pub(super) async fn enter(
        &self,
        connection: &Arc<Connection>,
        home: &App,
        doc: u32,
        entering: Entering<'_>,
    ) -> Result<(), AppError> {
        let (workspace_id, card_id) = (entering.workspace_id, entering.card_id);
        let limits = &self.inner.rooms;
        // Before anything is asked of the store: a connection that is in as many
        // rooms as it may be has nothing asked for it.
        if connection.is_full_without(limits.per_connection, card_id) {
            return Err(refusals::too_many_joined(limits.per_connection));
        }
        let mut raised = limits.walls.now();
        let mut admitted =
            admission::let_in(home, connection.asker(), workspace_id, card_id).await?;
        let mut admissions = 1;
        loop {
            #[cfg(feature = "test-util")]
            self.paused(pauses::Pause::AfterAdmission).await;
            let room = self.room(&admitted, workspace_id, card_id);
            let mut turn = room.turn.lock().await;
            if turn.closed || room.gone.is_cancelled() {
                continue;
            }
            // A change since admission that reaches this room and user; its immediate
            // check may have passed the room already. Ask again before adding the
            // connection.
            let tenant = room.app.store().tenant();
            if limits
                .walls
                .since(tenant, raised)
                .reaches(&room, &connection.user_id)
            {
                self.shut_if_empty(&room, &mut turn).await;
                drop(turn);
                if admissions == ADMITTED_AT_MOST {
                    return Err(refusals::unavailable());
                }
                raised = limits.walls.now();
                admitted =
                    admission::let_in(home, connection.asker(), workspace_id, card_id).await?;
                admissions += 1;
                continue;
            }
            if let Some(refused) = self.refusal(&turn, connection) {
                self.shut_if_empty(&room, &mut turn).await;
                return Err(refused);
            }
            let loaded = match self
                .open_while(&room.app, card_id, Some(room.gone.clone()))
                .await
            {
                Ok(loaded) => loaded,
                Err(error) => {
                    self.shut_if_empty(&room, &mut turn).await;
                    return Err(error);
                }
            };
            if room.gone.is_cancelled() {
                return Err(AppError::not_found("card"));
            }
            let epoch = loaded.epoch;
            let queued = self
                .welcome(&room, connection, doc, &loaded, admitted.mode, &entering)
                .await;
            // Counted among the connection's rooms before it is a member: a connection
            // that ended meanwhile is no member of a room nobody would take it out of.
            if queued.is_err() || !connection.join(doc, &room) {
                connection.queue.forget(doc);
                self.shut_if_empty(&room, &mut turn).await;
                return Err(refusals::too_slow());
            }
            turn.forget_member(connection);
            if !turn.members.is_empty() && turn.epoch != epoch {
                // Made again since the members were sent it, and not yet told.
                turn.send_anew(&loaded);
            }
            turn.epoch = epoch;
            turn.members.push(Member {
                connection: Arc::clone(connection),
                doc,
                workspace_id: workspace_id.to_owned(),
                mode: admitted.mode,
                checking: None,
                missed: None,
                caret: None,
                carets_missed: None,
            });
            turn.send_carets(turn.members.len() - 1);
            return Ok(());
        }
    }

    /// Why a connection that was admitted is not let into a room as it is, if it is not:
    /// the server stops, or one more would be more than the limits take.
    fn refusal(&self, turn: &Turn, connection: &Connection) -> Option<AppError> {
        let limits = &self.inner.rooms;
        let is_member = turn
            .members
            .iter()
            .any(|member| member.connection.id == connection.id);
        if self.rooms_ended() {
            Some(refusals::unavailable())
        } else if is_member {
            // In the room anew: one more neither of the connection's rooms nor of the
            // room's connections.
            None
        } else if connection.joined().rooms.len() >= limits.per_connection {
            Some(refusals::too_many_joined(limits.per_connection))
        } else if turn.of_user(&connection.user_id) >= limits.per_user {
            Some(refusals::too_many_of_user(limits.per_user))
        } else if turn.members.len() >= limits.per_room {
            Some(refusals::full(limits.per_room))
        } else {
            None
        }
    }

    /// Takes a connection out of a room it knew as `doc`.
    pub(super) async fn leave(&self, room: &Arc<Room>, connection: &Connection, doc: u32) {
        let mut turn = room.turn.lock().await;
        if let Some(index) = turn.index_of(connection, doc) {
            turn.take_out(index);
        }
        connection.queue.forget(doc);
        self.shut_if_empty(room, &mut turn).await;
    }

    /// Puts a connection out of a room it knows as `doc`, and tells it why.
    pub(super) async fn put_out(
        &self,
        room: &Arc<Room>,
        connection: &Connection,
        doc: u32,
        reason: &str,
    ) {
        let mut turn = room.turn.lock().await;
        if let Some(index) = turn.index_of(connection, doc) {
            turn.put_out(index, reason);
        }
        self.shut_if_empty(room, &mut turn).await;
    }

    /// How many connections are in the room of a card of `app`'s organization, for
    /// the tests of who is put out of it.
    #[cfg(feature = "test-util")]
    #[doc(hidden)]
    pub async fn room_members(&self, app: &App, card_id: &str) -> usize {
        let room = self
            .rooms_index()
            .get(app.store().tenant(), card_id)
            .map(Arc::clone);
        match room {
            Some(room) => room.turn.lock().await.members.len(),
            None => 0,
        }
    }

    /// Sends every member of a held room the document as it is now, after `heard`
    /// happened to it (`None` if what happened is not known). A document that was
    /// made again and is the one the members hold already is not sent: who came in
    /// since, or the update that made it again, brought it.
    ///
    /// `false` if the document cannot be worked on for now (`document_unavailable`):
    /// the members stay, with the document they hold, and are still to be sent it.
    /// If it cannot be opened for another reason (its card is gone, the server holds
    /// as many documents as it may) the members are put out, with that reason.
    async fn anew(&self, room: &Room, turn: &mut Turn, heard: Option<DocChange>) -> bool {
        let loaded = match self
            .open_while(&room.app, &room.card_id, Some(room.gone.clone()))
            .await
        {
            Ok(loaded) => loaded,
            Err(error) if error.reason() == Some(Reason::DocumentUnavailable) => return false,
            Err(error) => {
                error.log();
                let reason = reason_of(&error);
                while !turn.members.is_empty() {
                    turn.put_out(0, &reason);
                }
                return true;
            }
        };
        if room.gone.is_cancelled() {
            return true;
        }
        let made_again = matches!(heard, Some(DocChange::Remade | DocChange::Replaced));
        if made_again && loaded.epoch == turn.epoch {
            // Sent already, by the join or the update that found it made again.
            return true;
        }
        turn.send_anew(&loaded);
        true
    }

    /// Whether anybody who may write is in the room of the card `card_id` of `app`'s
    /// organization. The room's turn is not waited for: one who asks this may hold the
    /// card's slot, which whoever holds the turn may wait for.
    pub(super) fn has_editors(&self, app: &App, card_id: &str) -> bool {
        self.rooms_index()
            .get(app.store().tenant(), card_id)
            .is_some_and(|room| room.turn.writers() > 0)
    }

    /// Closes a held room that nobody is in: its document is closed, which writes its
    /// body to the card, and whoever comes next makes the room anew. The room is held
    /// until the document is closed, so that the next opens it after. A room that one
    /// who typed has left, or writes in no more, asks for what was typed to be kept as
    /// a version: it is, unless another who typed it is still there to write.
    async fn shut_if_empty(&self, room: &Arc<Room>, turn: &mut Turn) {
        if turn.closed {
            return;
        }
        if !turn.members.is_empty() {
            if turn.typed.is_empty() {
                return;
            }
            let mut writing: Vec<String> = turn
                .members
                .iter()
                .filter(|member| member.mode == DocMode::Write)
                .map(|member| member.connection.user_id.clone())
                .collect();
            writing.sort_unstable();
            writing.dedup();
            if turn.typed.iter().all(|by| writing.contains(by)) {
                return;
            }
            // Whoever of them typed is not known here: the leaving of each asks again.
            turn.typed = match self
                .version_without_writers(&room.app, &room.card_id, room.gone.clone(), &writing)
                .await
            {
                Left::Waiting => writing,
                Left::Settled => Vec::new(),
            };
            return;
        }
        turn.closed = true;
        room.gone.cancel();
        if let Err(error) = self.close_room_document(room).await {
            tracing::error!(%error, card_id = room.card_id, "cannot write a card's body as its room closes");
        }
        self.rooms_index()
            .remove_room(room.app.store().tenant(), room);
    }
}
