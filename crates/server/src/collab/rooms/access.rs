//! Immediate access changes and the periodic fallback for the rooms.

use std::collections::{BTreeSet, HashMap};
use std::future::Future;
use std::hash::Hash;
use std::sync::Arc;
use std::time::Duration;

use fvoci_store::Tenant;
use tokio::time::Instant;

use super::super::messages::reason_of;
use super::super::queue::Queued;
use super::super::{Collab, admission, messages, refusals};
use super::turn_lock::TurnGuard;
use super::{
    ASKED_AT_ONCE, ASKED_ONCE_MORE_AFTER, Answer, Asked, Connection, Member, Room, Turn, Who,
};
use crate::app::App;
use crate::error::AppError;
use crate::permissions::DocMode;

/// How many intervals of the periodic check (`collab_recheck_seconds`) an immediate
/// check may take in all, each read being given one: past that, whom it did not
/// answer of yet is put out as not answered of.
const IMMEDIATE_PASS_INTERVALS: u32 = 2;

/// The facts changed in this part of the application's organization, for everybody
/// or for some users.
#[derive(Debug, Clone)]
pub(crate) struct AccessScope {
    place: Place,
    /// The users whose facts changed; everybody's when `None`.
    users: Option<BTreeSet<String>>,
}

#[derive(Debug, Clone)]
enum Place {
    Containers(Vec<String>),
    /// The cards themselves: who may be in their rooms changes as they are deleted.
    Cards(Vec<String>),
    Workspace(String),
    Organization,
}

impl AccessScope {
    /// One board, including all its cards.
    pub(crate) fn container(container_id: &str) -> Self {
        Self::of(Place::Containers(vec![container_id.to_owned()]))
    }

    /// Boards changed at once (a batch, an import), including all their cards: one
    /// check for all of them.
    pub(crate) fn containers<'id>(container_ids: impl IntoIterator<Item = &'id String>) -> Self {
        Self::of(Place::Containers(
            container_ids.into_iter().cloned().collect(),
        ))
    }

    /// The rooms of these cards, as they are deleted.
    pub(super) fn cards(card_ids: &[String]) -> Self {
        Self::of(Place::Cards(card_ids.to_vec()))
    }

    /// Every board of a workspace.
    pub(crate) fn workspace(workspace_id: &str) -> Self {
        Self::of(Place::Workspace(workspace_id.to_owned()))
    }

    /// Every workspace of the organization.
    pub(crate) fn organization() -> Self {
        Self::of(Place::Organization)
    }

    /// The same place, where only `user_id`'s facts changed: the others in its rooms
    /// are not asked about.
    pub(crate) fn of_user(self, user_id: &str) -> Self {
        self.of_users([user_id])
    }

    /// The same place, where only the facts of `users` changed (the people of a
    /// team): the others in its rooms are not asked about. A change of nobody's facts
    /// is no change: a caller has none to recheck.
    pub(crate) fn of_users<'id>(self, users: impl IntoIterator<Item = &'id str>) -> Self {
        let users: BTreeSet<String> = users.into_iter().map(str::to_owned).collect();
        debug_assert!(!users.is_empty(), "a change of nobody's facts at {self:?}");
        Self {
            users: Some(users),
            ..self
        }
    }

    fn of(place: Place) -> Self {
        Self { place, users: None }
    }

    /// Whether the change reached the room of `room`'s card.
    pub(super) fn contains(&self, room: &Room) -> bool {
        self.reaches(&room.card_id, &room.container_id, &room.workspace_id)
    }

    /// Whether the change reached the card `card_id` of the board `container_id` of the
    /// workspace `workspace_id`.
    pub(super) fn reaches(&self, card_id: &str, container_id: &str, workspace_id: &str) -> bool {
        match &self.place {
            Place::Containers(ids) => ids.iter().any(|id| id == container_id),
            Place::Cards(ids) => ids.iter().any(|id| id == card_id),
            Place::Workspace(id) => id == workspace_id,
            Place::Organization => true,
        }
    }

    /// The boards the change named, if it named boards.
    pub(super) fn named_containers(&self) -> Option<&[String]> {
        match &self.place {
            Place::Containers(ids) => Some(ids),
            _ => None,
        }
    }

    /// The cards the change named, if it named cards.
    pub(super) fn named_cards(&self) -> Option<&[String]> {
        match &self.place {
            Place::Cards(ids) => Some(ids),
            _ => None,
        }
    }

    /// Whether the change concerns `user_id`'s facts.
    pub(super) fn concerns(&self, user_id: &str) -> bool {
        self.users
            .as_ref()
            .is_none_or(|changed| changed.contains(user_id))
    }
}

/// The members an immediate check holds, by room: what each sends waits until the
/// check answered of them. Dropped before it did (the check ended early), it puts
/// them out as members the store did not answer of, in a task of its own: what they
/// send is never left waiting, nor taken unasked.
#[derive(Debug)]
struct Holds {
    collab: Collab,
    /// The number of the change the check is for: of two checks of one member, the
    /// later change's answers.
    change: u64,
    rooms: Vec<(Arc<Room>, Vec<Asked>)>,
}

impl Holds {
    /// Lets go of every member held, each with what `answer_of` says of them (`None`:
    /// not answered of), sends who stays what the room held from them meanwhile, and
    /// closes a room that nobody is left in.
    async fn let_go<'answers>(
        mut self,
        answer_of: impl Fn(&Room, &Who) -> Option<&'answers Answer>,
    ) {
        // Taken from the holds once its members are let go of: a room not done stays
        // held, for the drop (letting go of a member twice does nothing).
        while let Some((room, members)) = self.rooms.last() {
            let room = Arc::clone(room);
            let mut turn = room.turn.lock().await;
            for member in members {
                if let Some(index) = turn.let_go(member, self.change) {
                    turn.settle(index, answer_of(&room, &member.who));
                    self.collab.send_missed(&room, &mut turn, member).await;
                    turn.send_carets_missed(member);
                }
            }
            room.checked.notify_waiters();
            self.rooms.pop();
            self.collab.shut_if_empty(&room, &mut turn).await;
        }
    }
}

impl Drop for Holds {
    fn drop(&mut self) {
        if self.rooms.is_empty() {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            tracing::error!("an immediate check ended outside the runtime; its members stay held");
            return;
        };
        tracing::error!("an immediate check ended before it answered; whom it held is put out");
        let held = Self {
            collab: self.collab.clone(),
            change: self.change,
            rooms: std::mem::take(&mut self.rooms),
        };
        runtime.spawn(held.let_go(|_, _| None));
    }
}

impl Asked {
    /// A member as it is asked about, by its connection and the room's number on it.
    fn of(member: &Member) -> Self {
        Self {
            connection: member.connection.id,
            doc: member.doc,
            who: Who {
                user_id: member.connection.user_id.clone(),
                making: member.connection.making.making().into_owned(),
                workspace_id: member.workspace_id.clone(),
            },
        }
    }

    pub(super) fn is(&self, member: &Member) -> bool {
        member.connection.id == self.connection && member.doc == self.doc
    }
}

impl Turn {
    /// Has an answer of the store take effect for a member: told how they are in the
    /// room if that changed (`docModeChanged`), or put out if they are not let in, if the
    /// store could not be asked (`None`: not answered in time).
    fn settle(&mut self, index: usize, answer: Option<&Answer>) {
        let member = &mut self.members[index];
        match answer {
            Some(Ok(Some(mode))) if *mode == member.mode => {}
            Some(Ok(Some(mode))) => {
                member.mode = *mode;
                let (doc, connection, mode) = (member.doc, member.connection.id, *mode);
                if mode == DocMode::Read {
                    // Who reads shows no caret.
                    self.withdraw_caret(index);
                }
                self.send(
                    |member| member.connection.id == connection && member.doc == doc,
                    |_| [Queued::text(doc, messages::mode(doc, mode))],
                );
            }
            Some(Ok(None)) => self.put_out(index, &reason_of(&AppError::not_found("card"))),
            Some(Err(reason)) => self.put_out(index, reason),
            None => self.put_out(index, &reason_of(&refusals::unavailable())),
        }
    }

    /// Holds the members `scope` concerns for the check of `change`, but those a later
    /// change's check holds already: that check answers of them. Whom it holds, to be
    /// asked about.
    fn hold(&mut self, scope: &AccessScope, change: u64) -> Vec<Asked> {
        self.members
            .iter_mut()
            .filter(|member| scope.concerns(&member.connection.user_id))
            .filter(|member| member.checking.is_none_or(|held| held < change))
            .map(|member| {
                member.checking = Some(change);
                Asked::of(member)
            })
            .collect()
    }

    /// Lets go of the member `asked` names if the check of `change` holds them, and
    /// says where they are; `None` if they left, or a later change's check holds
    /// them or answered of them already.
    fn let_go(&mut self, asked: &Asked, change: u64) -> Option<usize> {
        let index = self.members.iter().position(|member| asked.is(member))?;
        let member = &mut self.members[index];
        if member.checking == Some(change) {
            member.checking = None;
            Some(index)
        } else {
            None
        }
    }
}

impl Collab {
    /// Raises the wall for `scope` before a change that narrows access is written, so
    /// that an admission under way reads the facts again; `recheck_access` after the
    /// write still holds and asks the members already in.
    pub(crate) fn narrowing_access(&self, app: &App, scope: &AccessScope) {
        self.inner
            .rooms
            .walls
            .raise(app.store().tenant(), scope.clone());
    }

    /// Rechecks affected rooms before a permission-changing operation answers.
    /// The task survives a caller going away. The store is asked once per board,
    /// user and session kind, regardless of the number of cards or connections.
    pub(crate) async fn recheck_access(&self, app: &App, scope: AccessScope) {
        let (collab, scoped) = (self.clone(), app.clone());
        let task = app.spawn_tracked(async move { collab.access_changed(&scoped, scope).await });
        if let Err(error) = task.await {
            tracing::error!(%error, "the immediate room access check ended");
        }
    }

    /// Holds whom the change concerns in the rooms it reaches, asks the store about
    /// them while no room is held, and has each answer take effect, unless a later
    /// change concerned the same member meanwhile: that change's check answers of
    /// them instead. Each read has a deadline of its own: a slow one keeps its own
    /// member waiting, and puts them out at its deadline, and nobody else.
    async fn access_changed(&self, app: &App, scope: AccessScope) {
        let rooms = &self.inner.rooms;
        let tenant = app.store().tenant();
        // Admissions that began before this change must read the new facts.
        let change = rooms.walls.raise(tenant, scope.clone());
        let holds = self.hold(tenant, &scope, change).await;
        // Each board and user once, however many rooms and connections they have.
        let mut asked: HashMap<(String, Who), App> = HashMap::new();
        for (room, members) in &holds.rooms {
            for member in members {
                asked
                    .entry((room.container_id.clone(), member.who.clone()))
                    .or_insert_with(|| room.app.clone());
            }
        }
        let until = Instant::now() + rooms.recheck * IMMEDIATE_PASS_INTERVALS;
        let asking = answered(asked, rooms.recheck, until, |(key, app)| {
            let (container_id, who) = key.clone();
            (key, async move {
                #[cfg(feature = "test-util")]
                self.before_access_read().await;
                admission::container_mode(&app, who.asker(), &who.workspace_id, &container_id).await
            })
        });
        // Given up when the server stops: whom it holds is put out, as not answered of.
        let answers = tokio::select! {
            answers = asking => answers,
            () = rooms.stopping.cancelled() => HashMap::new(),
        };
        let mut by_container: HashMap<String, HashMap<Who, Answer>> = HashMap::new();
        for ((container_id, who), answer) in answers {
            by_container
                .entry(container_id)
                .or_default()
                .insert(who, answer);
        }
        holds
            .let_go(|room, who| by_container.get(&room.container_id)?.get(who))
            .await;
    }

    /// Holds, for the check of `change`, the members `scope` concerns in the rooms of
    /// `tenant` it reaches.
    async fn hold(&self, tenant: Tenant, scope: &AccessScope, change: u64) -> Holds {
        let reached = self.rooms_index().reached(tenant, scope);
        let mut holds = Holds {
            collab: self.clone(),
            change,
            rooms: Vec::new(),
        };
        for room in reached {
            let mut turn = room.turn.lock().await;
            if turn.closed {
                continue;
            }
            let members = turn.hold(scope, change);
            drop(turn);
            if !members.is_empty() {
                holds.rooms.push((room, members));
            }
        }
        holds
    }

    /// The room's turn for what the member `doc` of `connection` sent, once no
    /// immediate check holds them, or the server stops (and refuses it); `None` if
    /// they are no member of the room (any more), or it is gone.
    pub(super) async fn turn_to_send<'room>(
        &self,
        room: &'room Room,
        connection: &Connection,
        doc: u32,
    ) -> Option<TurnGuard<'room>> {
        loop {
            // Before the turn is looked at: an answer given after that is heard.
            let checked = room.checked.notified();
            let turn = room.turn.lock().await;
            let index = turn.index_of(connection, doc)?;
            if turn.members[index].checking.is_none() {
                return Some(turn);
            }
            drop(turn);
            tokio::select! {
                () = checked => {}
                () = self.inner.rooms.stopping.cancelled() => return Some(room.turn.lock().await),
                // Deleted: its members are put out, and what they sent is taken by no room.
                () = room.gone.cancelled() => return None,
            }
        }
    }

    /// Asks again whether each member of a room may be in it, and how: who may no
    /// longer read the card's board is put out, and who is to be in the room otherwise
    /// than they are is told so (`docModeChanged`). Nobody stays unasked: a member the store
    /// could not be asked about, or was not asked about within the time of one
    /// asking, is asked about once more two seconds later, and put out if the store
    /// does not answer then either. The store is asked while the room is not held; what
    /// a member sent meanwhile was taken as they were.
    pub(super) async fn recheck(&self, room: &Arc<Room>) {
        let (tenant, revision) = (room.app.store().tenant(), self.inner.rooms.walls.now());
        let asked: Vec<Asked> = room
            .turn
            .lock()
            .await
            .members
            .iter()
            .map(Asked::of)
            .collect();
        // Each user once, however many connections they have in the room.
        let mut users: Vec<Who> = asked.iter().map(|asked| asked.who.clone()).collect();
        users.sort_unstable();
        users.dedup();
        let mut answers = self.answers(room, users.clone()).await;
        // Whom the store did not answer of is asked once more, a little later: a store
        // that fails for a moment empties no room. Who is still not answered of then
        // is put out.
        users.retain(|who| !matches!(answers.get(who), Some(Ok(_))));
        if !users.is_empty() {
            #[cfg(feature = "test-util")]
            self.paused(super::pauses::Pause::BeforeAskingOnceMore)
                .await;
            tokio::time::sleep(ASKED_ONCE_MORE_AFTER).await;
            answers.extend(self.answers(room, users).await);
        }
        #[cfg(feature = "test-util")]
        self.paused(super::pauses::Pause::AfterRecheck).await;
        let mut turn = room.turn.lock().await;
        if turn.closed || self.rooms_ended() {
            return;
        }
        // A change since that reaches a member was asked about again by its own check:
        // what was read before it is stale for them. The others are answered.
        let changed = self.inner.rooms.walls.since(tenant, revision);
        for asked in asked {
            if changed.reaches(room, &asked.who.user_id) {
                continue;
            }
            if let Some(index) = turn.members.iter().position(|member| asked.is(member)) {
                turn.settle(index, answers.get(&asked.who));
            }
        }
        self.shut_if_empty(room, &mut turn).await;
    }

    /// What the store answers of each of `users` in a room: how they are in it, `None`
    /// for who is not let in, or the reason the store could not be asked. All within
    /// the time between two askings: who is not answered of by then has no answer.
    async fn answers(&self, room: &Room, users: Vec<Who>) -> HashMap<Who, Answer> {
        let (app, card_id) = (&room.app, room.card_id.as_str());
        let recheck = self.inner.rooms.recheck;
        answered(users, recheck, Instant::now() + recheck, |who| {
            (who.clone(), async move {
                let admitted = admission::admit(app, who.asker(), &who.workspace_id, card_id).await;
                admitted.map(|admitted| admitted.map(|admitted| admitted.mode))
            })
        })
        .await
    }
}

/// Asks the store each of `questions` with `ask`, which says what its answer is
/// kept under, some at a time ([`ASKED_AT_ONCE`]), each within `per_read` of its
/// asking: a slow one costs the others none of their time. One not answered in time
/// is answered `document_unavailable`; a reason the store could not be asked is
/// logged, and answered as the reason. Whatever is not answered by `until` has no
/// answer.
async fn answered<Q, K, A, F>(
    questions: impl IntoIterator<Item = Q>,
    per_read: Duration,
    until: Instant,
    mut ask: A,
) -> HashMap<K, Answer>
where
    K: Eq + Hash,
    A: FnMut(Q) -> (K, F),
    F: Future<Output = Result<Option<DocMode>, AppError>>,
{
    use futures_util::StreamExt as _;

    let asking = futures_util::stream::iter(questions)
        .map(|question| {
            let (key, asking) = ask(question);
            async move {
                let answer = match tokio::time::timeout(per_read, asking).await {
                    Ok(Ok(mode)) => Ok(mode),
                    Ok(Err(error)) => {
                        error.log();
                        Err(reason_of(&error))
                    }
                    Err(_) => {
                        tracing::warn!(
                            ?per_read,
                            "the store did not answer of a member of a room in time"
                        );
                        Err(reason_of(&refusals::unavailable()))
                    }
                };
                (key, answer)
            }
        })
        .buffer_unordered(ASKED_AT_ONCE);
    tokio::pin!(asking);
    let mut answers = HashMap::new();
    loop {
        tokio::select! {
            answer = asking.next() => match answer {
                Some((key, answer)) => {
                    answers.insert(key, answer);
                }
                None => return answers,
            },
            () = tokio::time::sleep_until(until) => {
                tracing::warn!(answered = answers.len(), "the store was not asked about everybody in a room in time");
                return answers;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::super::{Joined, Turn};
    use super::*;
    use tokio_util::sync::CancellationToken;

    use crate::collab::queue::{Capacity, Queue};

    /// A room with one member, `user`'s connection.
    fn turn_of(user: &str) -> Turn {
        let connection = Arc::new(Connection {
            id: 1,
            user_id: user.to_owned(),
            making: crate::app::SessionKept::with_password(),
            queue: Queue::new(Capacity {
                messages: 8,
                bytes: 1024,
            }),
            joined: Mutex::new(Joined::default()),
            ended_worker: CancellationToken::new(),
        });
        Turn {
            members: vec![Member {
                connection,
                doc: 1,
                workspace_id: "workspace".to_owned(),
                mode: DocMode::Write,
                checking: None,
                missed: None,
                caret: None,
                carets_missed: None,
            }],
            ..Turn::default()
        }
    }

    fn change_of(user: &str) -> AccessScope {
        AccessScope::container("board").of_user(user)
    }

    #[test]
    fn of_two_checks_held_in_order_the_later_answers() {
        let mut turn = turn_of("user");
        let earlier = turn.hold(&change_of("user"), 1);
        let later = turn.hold(&change_of("user"), 2);
        assert_eq!((earlier.len(), later.len()), (1, 1));
        assert_eq!(
            turn.let_go(&earlier[0], 1),
            None,
            "the earlier answer is dropped"
        );
        assert_eq!(turn.let_go(&later[0], 2), Some(0));
        assert_eq!(turn.members[0].checking, None);
    }

    #[test]
    fn an_earlier_check_held_after_a_later_one_neither_holds_nor_answers() {
        let mut turn = turn_of("user");
        let later = turn.hold(&change_of("user"), 2);
        assert!(turn.hold(&change_of("user"), 1).is_empty());
        assert_eq!(turn.members[0].checking, Some(2));
        assert_eq!(turn.let_go(&later[0], 2), Some(0));
    }

    #[test]
    fn a_check_answered_before_the_next_is_held_lets_the_next_hold() {
        let mut turn = turn_of("user");
        let earlier = turn.hold(&change_of("user"), 1);
        assert_eq!(turn.let_go(&earlier[0], 1), Some(0));
        let later = turn.hold(&change_of("user"), 2);
        assert_eq!(turn.let_go(&earlier[0], 1), None, "an answer is taken once");
        assert_eq!(turn.let_go(&later[0], 2), Some(0));
    }

    #[test]
    fn a_change_reaches_its_place_and_its_user() {
        let card = ("card", "board", "workspace");
        let reaches = |scope: &AccessScope| scope.reaches(card.0, card.1, card.2);
        let cards = ["card".to_owned()];
        for (scope, reached) in [
            (AccessScope::container("board"), true),
            (AccessScope::container("another"), false),
            (AccessScope::cards(&cards), true),
            (AccessScope::cards(&["another".to_owned()]), false),
            (AccessScope::workspace("workspace"), true),
            (AccessScope::workspace("another"), false),
            (AccessScope::organization(), true),
        ] {
            assert_eq!(reaches(&scope), reached, "{scope:?}");
            let of_user = scope.of_user("user");
            assert_eq!(reaches(&of_user), reached, "{of_user:?}");
            assert!(of_user.concerns("user") && !of_user.concerns("another user"));
        }
        assert!(AccessScope::organization().concerns("anybody"));
        let of_users = AccessScope::container("board").of_users(["one", "two"]);
        assert!(of_users.concerns("one") && of_users.concerns("two"));
        assert!(!of_users.concerns("three"));
    }

    #[test]
    fn a_change_of_another_user_holds_nobody_here() {
        let mut turn = turn_of("user");
        assert!(turn.hold(&change_of("somebody else"), 1).is_empty());
        assert_eq!(turn.members[0].checking, None);
    }
}
