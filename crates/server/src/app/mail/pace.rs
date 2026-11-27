//! The pace of all outgoing mail: `mail_per_second` attempts, recovery first,
//! invitations held back while the way keeps asking for later, and one attempt now and
//! then while the way is closed.

use std::pin::pin;
use std::sync::Mutex;
use std::time::Duration;

use tokio::sync::Notify;
use tokio::time::Instant;

use super::outbound::Outcome;
use crate::metrics::MailKind;

/// How many attempts in a row the way asks to try later before invitations are held
/// back: one could be a busy moment; this many look like the provider's daily limit.
const LATER_STREAK: u32 = 3;
/// How long invitations are held back when the way did not say for how long.
const PAUSE: Duration = Duration::from_mins(1);
/// The longest invitations are held back for at once, whatever the way asked: the
/// next attempt of a recovery message, or the end of this, finds out again.
const LONGEST_PAUSE: Duration = Duration::from_mins(15);
/// While the way is closed ([`Outcome::PathDown`]), one attempt goes this often, a
/// waiting recovery message before any invitation: enough to find out it opened again,
/// few enough not to hammer a provider that refuses the keys.
const PROBE_INTERVAL: Duration = Duration::from_mins(1);

/// The turns of the attempts to send, shared by every message.
#[derive(Debug)]
pub(super) struct Pace {
    /// The time between two attempts.
    interval: Duration,
    state: Mutex<State>,
    /// Told whenever a turn is taken or the pause changes.
    changed: Notify,
}

#[derive(Debug)]
struct State {
    /// When the next attempt may start.
    next: Instant,
    /// How many recovery messages wait for a turn: invitations wait until none does.
    resets_waiting: usize,
    /// The attempts in a row that the way asked to try later.
    later_streak: u32,
    /// Until when invitations wait, while the way keeps asking for later.
    invitations_paused_until: Option<Instant>,
    /// While the way is closed, when the next attempt may go.
    next_probe: Option<Instant>,
}

/// What an attempt told of the way as a whole, when it changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PathChange {
    /// The way was open and is closed now.
    Closed,
    /// The way was closed and took a message, or refused one with a reply of its own,
    /// which it gives after it read the server's keys.
    Opened,
}

/// What a message of a kind waits for.
enum Wait {
    /// Nothing: it takes the turn.
    Nothing,
    /// The time when it may.
    Until(Instant),
    /// Another message: a recovery message that waits.
    Others,
}

impl Pace {
    /// A pace of `per_second` attempts a second; at least one.
    pub(super) fn new(per_second: u32) -> Self {
        Self {
            interval: Duration::from_secs(1) / per_second.max(1),
            state: Mutex::new(State {
                next: Instant::now(),
                resets_waiting: 0,
                later_streak: 0,
                invitations_paused_until: None,
                next_probe: None,
            }),
            changed: Notify::new(),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .expect("the mail pace is never held across a panic")
    }

    /// Whether the way is closed now.
    pub(super) fn is_closed(&self) -> bool {
        self.state().next_probe.is_some()
    }

    /// Waits until a message of `kind` may make an attempt, and takes that turn.
    pub(super) async fn turn(&self, kind: MailKind) {
        let _waiting = kind.goes_first().then(|| ResetWaiting::new(self));
        loop {
            let mut changed = pin!(self.changed.notified());
            changed.as_mut().enable();
            let wait = {
                let mut state = self.state();
                let now = Instant::now();
                let wait = state.wait_of(kind, now);
                if matches!(wait, Wait::Nothing) {
                    state.next = state.next.max(now) + self.interval;
                    if let Some(probe) = state.next_probe.as_mut() {
                        *probe = now + PROBE_INTERVAL;
                    }
                }
                wait
            };
            match wait {
                Wait::Nothing => break,
                Wait::Until(at) => {
                    tokio::select! {
                        () = &mut changed => {}
                        () = tokio::time::sleep_until(at) => {}
                    }
                }
                Wait::Others => changed.await,
            }
        }
        self.changed.notify_waiters();
    }

    /// Waits for a turn of the pace and takes it without sending, returning immediately
    /// if the way is closed or closes while waiting: so a passed turn never consumes a
    /// closed-way probe turn meant for a real delivery attempt.
    pub(super) async fn pass_turn(&self, kind: MailKind) {
        let _waiting = kind.goes_first().then(|| ResetWaiting::new(self));
        loop {
            let mut changed = pin!(self.changed.notified());
            changed.as_mut().enable();
            let wait = {
                let mut state = self.state();
                if state.next_probe.is_some() {
                    return;
                }
                let now = Instant::now();
                let wait = state.wait_of(kind, now);
                if matches!(wait, Wait::Nothing) {
                    state.next = state.next.max(now) + self.interval;
                }
                wait
            };
            match wait {
                Wait::Nothing => break,
                Wait::Until(at) => {
                    tokio::select! {
                        () = &mut changed => {}
                        () = tokio::time::sleep_until(at) => {}
                    }
                }
                Wait::Others => changed.await,
            }
        }
        self.changed.notify_waiters();
    }

    /// Learns from what an attempt became: the way asking for later, again and again,
    /// holds invitations back; a closed way holds all mail but one attempt now and then;
    /// a message it took lets them go again. Returns how the way as a whole changed.
    pub(super) fn observe(&self, outcome: Outcome) -> Option<PathChange> {
        let mut state = self.state();
        let now = Instant::now();
        let change = match outcome {
            Outcome::Sent => {
                state.later_streak = 0;
                state.invitations_paused_until = None;
                state.next_probe.take().map(|_| PathChange::Opened)
            }
            Outcome::Later { after, .. } => {
                state.later_streak = state.later_streak.saturating_add(1);
                if state.later_streak >= LATER_STREAK {
                    let pause = after.map_or(PAUSE, |after| after.clamp(PAUSE, LONGEST_PAUSE));
                    let until = now + pause;
                    if state
                        .invitations_paused_until
                        .is_none_or(|paused| paused <= now)
                    {
                        tracing::warn!(
                            seconds = pause.as_secs(),
                            "mail keeps being deferred: invitations wait, recovery goes on"
                        );
                    }
                    // A longer pause the way asked for stays.
                    state.invitations_paused_until = Some(
                        state
                            .invitations_paused_until
                            .map_or(until, |paused| paused.max(until)),
                    );
                }
                None
            }
            Outcome::PathDown { .. } => {
                if state.next_probe.is_some() {
                    None
                } else {
                    state.next_probe = Some(now + PROBE_INTERVAL);
                    Some(PathChange::Closed)
                }
            }
            // One message refused says nothing of the way's limits, but a reply of the
            // way came after it read the server's keys. A refusal without a reply (a
            // message that could not be made) says nothing of the way.
            Outcome::Refused { code: Some(_) } => {
                state.next_probe.take().map(|_| PathChange::Opened)
            }
            Outcome::Refused { code: None } => None,
        };
        drop(state);
        self.changed.notify_waiters();
        change
    }
}

impl State {
    fn wait_of(&self, kind: MailKind, now: Instant) -> Wait {
        if !kind.goes_first() {
            if self.resets_waiting > 0 {
                return Wait::Others;
            }
            if let Some(until) = self.invitations_paused_until
                && until > now
            {
                return Wait::Until(until);
            }
        }
        let next = self
            .next_probe
            .map_or(self.next, |probe| probe.max(self.next));
        if next > now {
            Wait::Until(next)
        } else {
            Wait::Nothing
        }
    }
}

/// A recovery message waiting for its turn, counted while it waits.
struct ResetWaiting<'a>(&'a Pace);

impl<'a> ResetWaiting<'a> {
    fn new(pace: &'a Pace) -> Self {
        pace.state().resets_waiting += 1;
        Self(pace)
    }
}

impl Drop for ResetWaiting<'_> {
    fn drop(&mut self) {
        if let Ok(mut state) = self.0.state.lock() {
            state.resets_waiting -= 1;
        }
        self.0.changed.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    /// When each of `kinds`, all asking at once, took its turn, from now, in seconds.
    async fn turns(pace: &Arc<Pace>, kinds: &[MailKind]) -> Vec<(MailKind, u64)> {
        let start = Instant::now();
        let order = Arc::new(Mutex::new(Vec::new()));
        let mut tasks = tokio::task::JoinSet::new();
        for &kind in kinds {
            let pace = Arc::clone(pace);
            let order = Arc::clone(&order);
            tasks.spawn(async move {
                pace.turn(kind).await;
                let at = start.elapsed().as_secs();
                order.lock().expect("order").push((kind, at));
            });
        }
        tasks.join_all().await;
        Arc::try_unwrap(order)
            .expect("the tasks are done")
            .into_inner()
            .expect("order")
    }

    #[tokio::test(start_paused = true)]
    async fn all_mail_keeps_one_pace_and_recovery_overtakes_waiting_invitations() {
        let pace = Arc::new(Pace::new(1));
        pace.turn(MailKind::Invitation).await;
        let taken = turns(
            &pace,
            &[
                MailKind::Invitation,
                MailKind::Invitation,
                MailKind::PasswordReset,
                MailKind::PasswordReset,
            ],
        )
        .await;
        assert_eq!(
            taken,
            [
                (MailKind::PasswordReset, 1),
                (MailKind::PasswordReset, 2),
                (MailKind::Invitation, 3),
                (MailKind::Invitation, 4),
            ],
            "one a second, recovery first"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn deferred_attempts_in_a_row_hold_invitations_back_but_not_recovery() {
        let pace = Arc::new(Pace::new(10));
        let later = Outcome::Later {
            after: None,
            code: Some(451),
        };
        for _ in 0..LATER_STREAK - 1 {
            pace.observe(later);
        }
        assert_eq!(
            turns(&pace, &[MailKind::Invitation]).await[0].1,
            0,
            "not yet"
        );
        pace.observe(Outcome::Refused { code: Some(550) });
        pace.observe(later);
        let taken = turns(&pace, &[MailKind::Invitation, MailKind::PasswordReset]).await;
        assert_eq!(
            taken,
            [
                (MailKind::PasswordReset, 0),
                (MailKind::Invitation, PAUSE.as_secs())
            ]
        );

        for _ in 0..LATER_STREAK {
            pace.observe(Outcome::Later {
                after: Some(Duration::from_hours(24)),
                code: Some(421),
            });
        }
        let invitation = {
            let pace = Arc::clone(&pace);
            tokio::spawn(async move { turns(&pace, &[MailKind::Invitation]).await })
        };
        tokio::time::sleep(Duration::from_mins(2)).await;
        assert!(
            !invitation.is_finished(),
            "the way asked for a day: at most a while"
        );
        pace.observe(Outcome::Sent);
        let taken = invitation.await.expect("the invitation's turn");
        assert_eq!(
            taken[0].1, 120,
            "a message the way took lets invitations go"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_pause_ends_by_itself_at_its_longest() {
        let pace = Arc::new(Pace::new(10));
        for _ in 0..LATER_STREAK {
            pace.observe(Outcome::Later {
                after: Some(Duration::from_hours(24)),
                code: None,
            });
        }
        let taken = turns(&pace, &[MailKind::Invitation]).await;
        assert_eq!(taken[0].1, LONGEST_PAUSE.as_secs());
    }

    #[tokio::test(start_paused = true)]
    async fn pass_turn_leaves_when_the_way_closes_and_steals_no_probe_turn() {
        let pace = Arc::new(Pace::new(1));
        pace.turn(MailKind::PasswordReset).await;
        let passing = {
            let pace = Arc::clone(&pace);
            tokio::spawn(async move { pace.pass_turn(MailKind::PasswordReset).await })
        };
        // Yield so `pass_turn` enters its sleep waiting for the next second.
        tokio::task::yield_now().await;
        assert!(!passing.is_finished(), "waiting for the next pace turn");
        assert_eq!(
            pace.observe(Outcome::PathDown { code: Some(535) }),
            Some(PathChange::Closed)
        );
        // Closing the way wakes `pass_turn`, which leaves without waiting for the probe.
        tokio::time::timeout(Duration::from_millis(100), passing)
            .await
            .expect("pass_turn exits promptly when the way closes")
            .expect("task joined");
        let start = Instant::now();
        pace.turn(MailKind::PasswordReset).await;
        assert_eq!(
            start.elapsed().as_secs(),
            PROBE_INTERVAL.as_secs(),
            "the first probe turn is still available after one minute"
        );
    }
}
