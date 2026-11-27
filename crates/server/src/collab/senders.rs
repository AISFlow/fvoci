//! Who sent an update that ended a worker, by taking longer than it may or by breaking
//! it: their updates are refused for a little while, ten seconds the first time (and the
//! connection that sent it is closed), twice as long each further time, and so are
//! those of everyone in their organization from its third such sender within a day. An
//! update is no more than a worker's time limit to anybody else (the worker is started
//! again at once for it, [`super::worker::Blame::Sender`]); this keeps its sender from
//! sending it again and again.
//!
//! Of the updates of a user of an organization, one at a time is checked, sent to its
//! worker and, if it ended it, put down, whichever worker holds its document
//! ([`Senders::turn`]): of those they send together, to the documents of one worker or
//! of several, the ones after the first that ended a worker are refused before they
//! are sent. The turn is a user's: the users of an organization do not wait for each
//! other's turn, and each may end a worker once before they are held back (the
//! organization is held from its third). An update that waits for its turn holds its
//! card's room and slot, and its organization's gate: who writes into that card, opens
//! it or is sent what they lack of it, and the server writing its body, waits with it,
//! and so does a deletion in the organization and every request about a document there
//! after it; for each update of that user ahead of it (one worker's queue and call
//! each, up to the time limit), as they waited for those of the same worker before.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fvoci_store::Tenant;
use tokio::sync::OwnedMutexGuard;
use tokio::time::Instant;

/// How long the updates of a sender are refused after the first that ended a worker:
/// short, as one may end it by chance; one who sends it again is held longer.
const FIRST_HOLD: Duration = Duration::from_secs(10);

/// The longest the updates of a sender or of an organization are refused for.
const LONGEST_HOLD: Duration = Duration::from_mins(60);

/// How long a sender is remembered after their last death: the deaths within it of the
/// one before are counted together, and the organization counts the senders it
/// remembers. Longer than the longest hold, so that a sender held that long is held
/// again at the next.
const REMEMBERED: Duration = Duration::from_hours(24);

/// How many of the users an organization remembers ([`REMEMBERED`]) are held back
/// alone: when one more ends a worker, everyone in it is, and longer for each further
/// one. One that sends from many users gets no more than that before the organization
/// is held; one user is held alone however often.
const ORGANIZATION_SPARED: u32 = 2;

/// How many updates a sender may force the worker to rebuild a document's checkpoint to
/// refuse within [`ROLLBACK_WINDOW`] before being held back for [`ROLLBACK_HOLD`].
const ROLLBACKS_FORGIVEN: u32 = 8;

/// The window in which a sender's post-mutation rollbacks are counted together.
const ROLLBACK_WINDOW: Duration = Duration::from_secs(60);

/// How long a sender is held back after exceeding [`ROLLBACKS_FORGIVEN`].
const ROLLBACK_HOLD: Duration = Duration::from_secs(30);

/// Who sent an update, as what it does to a worker is put down: the key the sender is
/// held back by (the user, or a token of theirs as `token:<id>`), and the user it is
/// of, by whom the organization counts its senders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SenderOf<'a> {
    pub(super) key: &'a str,
    pub(super) owner: &'a str,
}

impl<'a> From<&'a str> for SenderOf<'a> {
    /// A user, by a session of theirs.
    fn from(user_id: &'a str) -> Self {
        Self {
            key: user_id,
            owner: user_id,
        }
    }
}

/// The deaths of a sender.
#[derive(Debug, Clone)]
struct Deaths {
    count: u32,
    last: Instant,
    /// Until when their updates are refused.
    held_until: Option<Instant>,
    /// The user the sender is of ([`SenderOf::owner`]).
    owner: String,
}

/// Recent post-mutation rollbacks caused by a sender.
#[derive(Debug, Clone, Copy)]
struct Rollbacks {
    count: u32,
    since: Instant,
}

/// What an organization remembers of the updates of its users that ended a worker.
#[derive(Debug, Default)]
struct Organization {
    /// Its senders, by their key ([`SenderOf::key`]): a user by a session (a room lets
    /// in nobody without one), or an agent by a token of theirs.
    senders: HashMap<String, Deaths>,
    /// Post-mutation rollbacks per sender within [`ROLLBACK_WINDOW`].
    rollbacks: HashMap<String, Rollbacks>,
    /// Until when the updates of all its users are refused.
    held_until: Option<Instant>,
}

impl Organization {
    /// Forgets the senders not remembered at `now`. `false` if nothing is left that it
    /// would need: no sender, and no hold.
    fn forget(&mut self, now: Instant) -> bool {
        let remembered = |last: Instant| now.duration_since(last) <= REMEMBERED;
        self.senders.retain(|_, deaths| remembered(deaths.last));
        self.rollbacks
            .retain(|_, seen| now.duration_since(seen.since) <= ROLLBACK_WINDOW);
        !self.senders.is_empty()
            || !self.rollbacks.is_empty()
            || self.held_until.is_some_and(|until| until > now)
    }

    /// Counts an update the worker took in place and had to rebuild from its checkpoint
    /// to refuse: past [`ROLLBACKS_FORGIVEN`] in [`ROLLBACK_WINDOW`], holds the sender
    /// alone for [`ROLLBACK_HOLD`] without escalating to an organization-wide hold.
    fn rolled_back(&mut self, sender: SenderOf<'_>, now: Instant) {
        let seen = self
            .rollbacks
            .entry(sender.key.to_owned())
            .or_insert(Rollbacks {
                count: 0,
                since: now,
            });
        if now.duration_since(seen.since) > ROLLBACK_WINDOW {
            *seen = Rollbacks {
                count: 0,
                since: now,
            };
        }
        seen.count = seen.count.saturating_add(1);
        if seen.count > ROLLBACKS_FORGIVEN {
            let deaths = self.deaths_of(sender, now);
            deaths.last = now;
            deaths.held_until = deaths.held_until.max(Some(now + ROLLBACK_HOLD));
        }
    }

    /// What it remembers of a sender, made at `now` if it remembers nothing.
    fn deaths_of(&mut self, sender: SenderOf<'_>, now: Instant) -> &mut Deaths {
        self.senders
            .entry(sender.key.to_owned())
            .or_insert_with(|| Deaths {
                count: 0,
                last: now,
                held_until: None,
                owner: sender.owner.to_owned(),
            })
    }

    /// Counts a death of a sender at `now`, and holds everyone back if the sender is of
    /// one more user that it remembers past [`ORGANIZATION_SPARED`]: a user and the
    /// tokens of theirs are one, so that one person's agents do not hold back the
    /// organization where the person alone would not.
    fn ended_worker(&mut self, sender: SenderOf<'_>, now: Instant) {
        let new_user = !self
            .senders
            .values()
            .any(|deaths| deaths.owner == sender.owner);
        let deaths = self.deaths_of(sender, now);
        deaths.count = deaths.count.saturating_add(1);
        deaths.last = now;
        if let Some(hold) = hold(deaths.count, 0) {
            deaths.held_until = Some(now + hold);
        }
        if new_user {
            let owners: std::collections::HashSet<&str> = self
                .senders
                .values()
                .map(|deaths| deaths.owner.as_str())
                .collect();
            let users = u32::try_from(owners.len()).unwrap_or(u32::MAX);
            // Users forgotten meanwhile may leave fewer than at the hold under way,
            // which this does not shorten.
            if let Some(hold) = hold(users, ORGANIZATION_SPARED) {
                self.held_until = self.held_until.max(Some(now + hold));
            }
        }
    }
}

/// The organizations whose users' updates ended a worker, and the turns of the users
/// whose updates are under way.
#[derive(Debug, Default)]
pub(super) struct Senders {
    organizations: Mutex<HashMap<Tenant, Organization>>,
    /// By organization and user, while an update of theirs holds or waits for its turn
    /// ([`Senders::turn`]).
    lines: Mutex<HashMap<(Tenant, String), Line>>,
}

/// The turn of a user of an organization, and how many of their updates hold or wait
/// for it.
#[derive(Debug)]
struct Line {
    turn: Arc<tokio::sync::Mutex<()>>,
    in_line: usize,
}

/// The turn of a user of an organization, held: no other update of theirs there is
/// checked, sent or put down meanwhile ([`Senders::turn`]). Given back as it is
/// dropped, however the one that holds it ends.
#[derive(Debug)]
#[must_use = "the turn is given back when this is dropped"]
pub(super) struct Turn<'senders> {
    // Before the place in the line, so that the turn is given back before the line may
    // leave the table: one who comes after finds the line, not a new turn while this
    // one is held.
    _held: OwnedMutexGuard<()>,
    _in_line: InLine<'senders>,
}

/// An update in the line of a user, from before their turn is waited for: whoever stops
/// waiting leaves it as well as whoever is done with the turn, and the last to leave
/// takes the line out of the table.
#[derive(Debug)]
struct InLine<'senders> {
    senders: &'senders Senders,
    key: (Tenant, String),
}

impl Drop for InLine<'_> {
    fn drop(&mut self) {
        // Nothing panics with the lines held; a destructor does not panic.
        let Ok(mut lines) = self.senders.lines.lock() else {
            return;
        };
        let was_in = match lines.get_mut(&self.key) {
            Some(line) => {
                line.in_line = line.in_line.saturating_sub(1);
                if line.in_line == 0 {
                    lines.remove(&self.key);
                }
                true
            }
            None => false,
        };
        // With the lines let go, which a panic would poison.
        drop(lines);
        debug_assert!(was_in, "an update leaves a line it is in");
    }
}

impl Senders {
    /// The turn of a user of an organization, once the updates of theirs that wait for
    /// it before this one are done with it, in the order they wait: an update is checked
    /// ([`Senders::held_until`]), sent to its worker and, if it ended it, put down
    /// ([`Senders::ended_worker`]) in it. Only their own updates wait for it, those of
    /// another user, or of the same user in another organization, do not; one that
    /// stops waiting leaves the line.
    pub(super) async fn turn(&self, tenant: Tenant, user_id: &str) -> Turn<'_> {
        let key = (tenant, user_id.to_owned());
        let turn = {
            let mut lines = self.lines.lock().expect("the lines are not poisoned");
            let line = lines.entry(key.clone()).or_insert_with(|| Line {
                turn: Arc::default(),
                in_line: 0,
            });
            line.in_line += 1;
            Arc::clone(&line.turn)
        };
        // Before the wait, so that one that stops waiting leaves the line.
        let in_line = InLine { senders: self, key };
        Turn {
            _held: turn.lock_owned().await,
            _in_line: in_line,
        }
    }

    /// How many users' updates hold or wait for their turn.
    #[cfg(any(test, feature = "test-util"))]
    pub(super) fn lines(&self) -> usize {
        self.lines.lock().expect("the lines are not poisoned").len()
    }

    /// Until when the updates of a user of an organization are refused, at `now`; `None`
    /// if they are not.
    pub(super) fn held_until(
        &self,
        tenant: Tenant,
        user_id: &str,
        now: Instant,
    ) -> Option<Instant> {
        let organizations = self
            .organizations
            .lock()
            .expect("the organizations are not poisoned");
        let organization = organizations.get(&tenant)?;
        let sender = organization
            .senders
            .get(user_id)
            .and_then(|deaths| deaths.held_until);
        [sender, organization.held_until]
            .into_iter()
            .flatten()
            .filter(|until| *until > now)
            .max()
    }

    /// Puts down that an update of a sender of an organization ended a worker at `now`,
    /// after forgetting the senders, and the organizations, no longer remembered.
    pub(super) fn ended_worker<'a>(
        &self,
        tenant: Tenant,
        sender: impl Into<SenderOf<'a>>,
        now: Instant,
    ) {
        let mut organizations = self
            .organizations
            .lock()
            .expect("the organizations are not poisoned");
        organizations.retain(|_, organization| organization.forget(now));
        organizations
            .entry(tenant)
            .or_default()
            .ended_worker(sender.into(), now);
    }

    /// Puts down that an update of a user of an organization forced a worker rollback at
    /// `now`, holding that user back briefly past [`ROLLBACKS_FORGIVEN`].
    pub(super) fn rolled_back<'a>(
        &self,
        tenant: Tenant,
        sender: impl Into<SenderOf<'a>>,
        now: Instant,
    ) {
        let mut organizations = self
            .organizations
            .lock()
            .expect("the organizations are not poisoned");
        organizations.retain(|_, organization| organization.forget(now));
        organizations
            .entry(tenant)
            .or_default()
            .rolled_back(sender.into(), now);
    }
}

/// How long updates are refused after `count` deaths, the first `spared` of which are
/// not held: [`FIRST_HOLD`], doubling up to [`LONGEST_HOLD`].
fn hold(count: u32, spared: u32) -> Option<Duration> {
    let past = count.checked_sub(spared)?.checked_sub(1)?;
    Some(
        FIRST_HOLD
            .saturating_mul(1 << past.min(16))
            .min(LONGEST_HOLD),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The holds are kept by tenant whatever it is: two that are no organization stand
    /// for two organizations here. Naming an organization is for the few files that
    /// make a store handle of one (the store's
    /// `the_server_names_an_organization_for_a_handle_in_few_places`).
    const ONE: Tenant = Tenant::System;

    const HOUR: Duration = Duration::from_hours(1);

    #[test]
    fn a_sender_is_held_for_longer_each_time_up_to_the_longest() {
        let senders = Senders::default();
        let start = Instant::now();
        assert_eq!(senders.held_until(ONE, "ann", start), None);
        let mut now = start;
        for expected in [10, 20, 40, 80, 160, 320, 640, 1280, 2560, 3600, 3600] {
            senders.ended_worker(ONE, "ann", now);
            let until = senders.held_until(ONE, "ann", now).expect("held");
            assert_eq!(until - now, Duration::from_secs(expected), "after {now:?}");
            // Sent again as soon as it is let.
            now = until;
            assert_eq!(senders.held_until(ONE, "ann", now), None, "let again");
        }
    }

    #[test]
    fn a_sender_is_forgotten_a_day_after_the_last_time() {
        let senders = Senders::default();
        let start = Instant::now();
        senders.ended_worker(ONE, "ann", start);
        senders.ended_worker(ONE, "ann", start);
        let later = start + REMEMBERED + Duration::from_secs(1);
        senders.ended_worker(ONE, "ann", later);
        assert_eq!(
            senders.held_until(ONE, "ann", later),
            Some(later + FIRST_HOLD),
            "counted from the first again"
        );
    }

    #[test]
    fn the_organization_is_held_from_its_third_sender() {
        let senders = Senders::default();
        let now = Instant::now();
        for _ in 0..5 {
            senders.ended_worker(ONE, "ann", now);
        }
        assert!(senders.held_until(ONE, "ann", now).is_some());
        assert_eq!(
            senders.held_until(ONE, "cat", now),
            None,
            "one user is held alone however often"
        );
        senders.ended_worker(ONE, "bob", now);
        assert_eq!(senders.held_until(ONE, "bob", now), Some(now + FIRST_HOLD));
        assert_eq!(
            senders.held_until(ONE, "ann", now),
            Some(now + FIRST_HOLD * 16),
            "her fifth"
        );
        assert_eq!(
            senders.held_until(ONE, "cat", now),
            None,
            "the others are not"
        );
        senders.ended_worker(ONE, "dan", now);
        assert_eq!(
            senders.held_until(ONE, "cat", now),
            Some(now + FIRST_HOLD),
            "everyone is"
        );
        let other = Tenant::Unset;
        assert_eq!(
            senders.held_until(other, "cat", now),
            None,
            "another organization"
        );
    }

    /// Two users whose deaths are more than a day apart are never three in the
    /// organization's day: the first is forgotten before they end a worker again.
    #[test]
    fn two_users_do_not_hold_the_organization_across_a_day() {
        let senders = Senders::default();
        let start = Instant::now();
        senders.ended_worker(ONE, "ann", start);
        senders.ended_worker(ONE, "bob", start + HOUR * 20);
        let later = start + HOUR * 25;
        senders.ended_worker(ONE, "ann", later);
        assert_eq!(senders.held_until(ONE, "cat", later), None);
        assert_eq!(
            senders.held_until(ONE, "ann", later),
            Some(later + FIRST_HOLD),
            "her first again"
        );
    }

    /// A user and the tokens of theirs are one sender to the organization: each is held
    /// back by itself, and the organization only from its third user.
    #[test]
    fn a_user_and_their_tokens_count_once_to_the_organization() {
        let senders = Senders::default();
        let now = Instant::now();
        let token = |key| SenderOf { key, owner: "ann" };
        senders.ended_worker(ONE, "ann", now);
        senders.ended_worker(ONE, token("token:one"), now);
        senders.ended_worker(ONE, token("token:two"), now);
        senders.ended_worker(ONE, "bob", now);
        assert_eq!(senders.held_until(ONE, "cat", now), None, "two users");
        assert_eq!(
            senders.held_until(ONE, "token:two", now),
            Some(now + FIRST_HOLD),
            "a token is held by itself"
        );
        senders.ended_worker(ONE, "cat", now);
        assert_eq!(
            senders.held_until(ONE, "dan", now),
            Some(now + FIRST_HOLD),
            "the third user"
        );
    }

    /// Users 23 hours apart, however many, are never more than two to the organization.
    #[test]
    fn users_23_hours_apart_never_hold_the_organization() {
        let senders = Senders::default();
        let start = Instant::now();
        for (day, user) in (0..12).zip(["ann", "bob", "cat", "dan"].into_iter().cycle()) {
            let now = start + Duration::from_hours(23) * day;
            senders.ended_worker(ONE, user, now);
            assert_eq!(senders.held_until(ONE, "eve", now), None, "day {day}");
        }
    }

    /// A user the organization remembers counts once in it however often they end a
    /// worker: they are held longer, and the others are not held again for them.
    #[test]
    fn a_sender_again_does_not_hold_the_organization_again() {
        let senders = Senders::default();
        let start = Instant::now();
        for user in ["ann", "bob", "cat"] {
            senders.ended_worker(ONE, user, start);
        }
        assert_eq!(
            senders.held_until(ONE, "dan", start),
            Some(start + FIRST_HOLD)
        );
        let later = start + FIRST_HOLD * 2;
        senders.ended_worker(ONE, "ann", later);
        assert_eq!(senders.held_until(ONE, "dan", later), None);
        assert_eq!(
            senders.held_until(ONE, "ann", later),
            Some(later + FIRST_HOLD * 2),
            "her second"
        );
    }

    /// A user back within a day is still remembered: one of three with two new ones.
    #[test]
    fn a_user_back_within_a_day_is_still_counted() {
        let senders = Senders::default();
        let start = Instant::now();
        senders.ended_worker(ONE, "ann", start);
        senders.ended_worker(ONE, "ann", start + HOUR * 23);
        let later = start + HOUR * 46;
        senders.ended_worker(ONE, "bob", later);
        senders.ended_worker(ONE, "cat", later);
        assert_eq!(
            senders.held_until(ONE, "dan", later),
            Some(later + FIRST_HOLD)
        );
    }

    #[test]
    fn a_sender_back_within_a_day_is_held_longer() {
        let senders = Senders::default();
        let start = Instant::now();
        senders.ended_worker(ONE, "ann", start);
        let later = start + HOUR * 23;
        senders.ended_worker(ONE, "ann", later);
        assert_eq!(
            senders.held_until(ONE, "ann", later),
            Some(later + FIRST_HOLD * 2)
        );
    }

    #[test]
    fn each_further_user_holds_the_organization_longer() {
        let senders = Senders::default();
        let now = Instant::now();
        for user in ["ann", "bob", "cat", "dan"] {
            senders.ended_worker(ONE, user, now);
        }
        assert_eq!(
            senders.held_until(ONE, "eve", now),
            Some(now + FIRST_HOLD * 2)
        );
    }

    /// The users the organization forgets meanwhile do not cut short a hold under way.
    #[test]
    fn a_hold_of_the_organization_is_not_cut_short() {
        let senders = Senders::default();
        let start = Instant::now();
        for user in ["ann", "bob"] {
            senders.ended_worker(ONE, user, start);
        }
        let near = start + REMEMBERED - Duration::from_secs(10);
        for user in ["cat", "dan", "eve"] {
            senders.ended_worker(ONE, user, near);
        }
        let held = Some(near + FIRST_HOLD * 4);
        assert_eq!(senders.held_until(ONE, "zed", near), held, "five users");
        // Ann and Bob are forgotten: four are left, which alone would hold it for less.
        let later = near + Duration::from_secs(11);
        senders.ended_worker(ONE, "fay", later);
        assert_eq!(senders.held_until(ONE, "zed", later), held);
    }

    /// How many of the updates of a user of an organization hold or wait for their turn.
    fn in_line(senders: &Senders, tenant: Tenant, user_id: &str) -> usize {
        let lines = senders.lines.lock().expect("not poisoned");
        lines
            .get(&(tenant, user_id.to_owned()))
            .map_or(0, |line| line.in_line)
    }

    /// The next update of a user waits for their turn, and one of another user, or of
    /// the same user in another organization, does not; nobody is kept once all are done.
    #[tokio::test(start_paused = true)]
    async fn only_the_updates_of_the_same_user_wait_for_their_turn() {
        let senders = Senders::default();
        let soon = Duration::from_secs(1);
        let ann = senders.turn(ONE, "ann").await;
        let next = tokio::time::timeout(soon, senders.turn(ONE, "ann")).await;
        assert!(next.is_err(), "her next update waits");
        assert_eq!(in_line(&senders, ONE, "ann"), 1, "and leaves as it stops");
        let bob = tokio::time::timeout(soon, senders.turn(ONE, "bob"))
            .await
            .expect("another user does not wait");
        let elsewhere = tokio::time::timeout(soon, senders.turn(Tenant::Unset, "ann"))
            .await
            .expect("nor does she in another organization");
        assert_eq!(senders.lines(), 3);
        drop((bob, elsewhere));
        assert_eq!(senders.lines(), 1, "hers alone");
        drop(ann);
        assert_eq!(senders.lines(), 0, "nobody is kept");
        let _again = tokio::time::timeout(soon, senders.turn(ONE, "ann"))
            .await
            .expect("her turn again");
    }

    /// A turn is given back, and its place in the line too, by an update stopped while
    /// it waits for the turn and by one stopped while it holds it, in two organizations.
    #[tokio::test(start_paused = true)]
    async fn an_update_stopped_while_it_waits_or_holds_its_turn_gives_it_back() {
        let senders = Arc::new(Senders::default());
        let mut stopped = Vec::new();
        for tenant in [ONE, Tenant::Unset] {
            let (held, holding) = tokio::sync::oneshot::channel();
            let holder = tokio::spawn({
                let senders = Arc::clone(&senders);
                async move {
                    let _turn = senders.turn(tenant, "ann").await;
                    let _ = held.send(());
                    std::future::pending::<()>().await;
                }
            });
            holding.await.expect("the turn is held");
            let waiter = tokio::spawn({
                let senders = Arc::clone(&senders);
                async move {
                    let _turn = senders.turn(tenant, "ann").await;
                }
            });
            while in_line(&senders, tenant, "ann") < 2 {
                tokio::task::yield_now().await;
            }
            stopped.extend([waiter, holder]);
        }
        assert_eq!(senders.lines(), 2);
        // The ones that wait first, then the ones that hold.
        for task in stopped {
            task.abort();
            let _ = task.await;
        }
        assert_eq!(senders.lines(), 0, "nobody is kept");
        for tenant in [ONE, Tenant::Unset] {
            let _again = tokio::time::timeout(Duration::from_secs(1), senders.turn(tenant, "ann"))
                .await
                .expect("given back");
        }
    }

    /// An organization none of whose users is remembered any more, and that is not
    /// held, is forgotten when the next update ends a worker: what is kept is of the
    /// organizations with a sender in the last day.
    #[test]
    fn an_organization_is_forgotten_a_day_after_its_last_sender() {
        let senders = Senders::default();
        let kept = || {
            let organizations = senders.organizations.lock().expect("not poisoned");
            organizations.len()
        };
        let start = Instant::now();
        senders.ended_worker(ONE, "ann", start);
        senders.ended_worker(Tenant::Unset, "bob", start);
        assert_eq!(kept(), 2);
        let within = start + HOUR * 23;
        senders.ended_worker(ONE, "cat", within);
        assert_eq!(kept(), 2, "both have a sender of the last day");
        let later = start + REMEMBERED + Duration::from_secs(1);
        senders.ended_worker(ONE, "dan", later);
        assert_eq!(kept(), 1, "the other organization is forgotten");
        assert_eq!(senders.held_until(Tenant::Unset, "bob", later), None);
    }

    #[test]
    fn repeated_rollbacks_hold_the_sender_alone_without_holding_the_organization() {
        let senders = Senders::default();
        let start = Instant::now();
        for _ in 0..ROLLBACKS_FORGIVEN {
            senders.rolled_back(ONE, "ann", start);
            assert_eq!(senders.held_until(ONE, "ann", start), None);
        }
        senders.rolled_back(ONE, "ann", start);
        assert_eq!(
            senders.held_until(ONE, "ann", start),
            Some(start + ROLLBACK_HOLD)
        );
        for user in ["bob", "cat", "dan"] {
            for _ in 0..=ROLLBACKS_FORGIVEN {
                senders.rolled_back(ONE, user, start);
            }
        }
        assert_eq!(
            senders.held_until(ONE, "eve", start),
            None,
            "rollbacks never escalate to an organization-wide hold"
        );
        let after_window = start + ROLLBACK_WINDOW + Duration::from_secs(1);
        senders.rolled_back(ONE, "ann", after_window);
        assert_eq!(
            senders.held_until(ONE, "ann", after_window),
            None,
            "window resets after a minute"
        );
    }
}
