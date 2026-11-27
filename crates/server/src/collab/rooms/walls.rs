//! What changed of who may be in the rooms, by organization, and what each change
//! reached: what an admission or a periodic answer read before a change that reaches
//! its room and user is asked again, and nothing else is.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fvoci_store::Tenant;
use tokio::sync::broadcast;
use tokio::time::Instant;

use super::super::Collab;
use super::{AccessScope, Room};
use crate::app::App;

/// How long a change is remembered at least. What was read before a change no longer
/// remembered is asked again, whatever the change reached.
const REMEMBERED_FOR: Duration = Duration::from_mins(10);

/// How many intervals of the periodic check a change is remembered at least: longer
/// than a periodic check reads (two intervals, and a little), and than an immediate
/// one (two).
const REMEMBERED_INTERVALS: u32 = 3;

/// How many changes a listener outside the rooms may fall behind; one further behind
/// is told it lagged, and takes everything as changed.
const ANNOUNCED_AT_MOST: usize = 256;

/// How many changes of one organization are remembered at most, the latest.
const REMEMBERED_AT_MOST: usize = 1024;

/// Every how many changes those of every organization no longer remembered are
/// forgotten, and organizations with none left with them.
const SWEPT_EVERY: u64 = 64;

/// The changes of access facts, numbered in one sequence for the whole server: an
/// admission that does not know its organization yet can be measured against the
/// change of any.
#[derive(Debug)]
pub(super) struct Walls {
    changes: Mutex<Changes>,
    /// How long a change is remembered: [`REMEMBERED_FOR`], or longer for a long
    /// interval of the periodic check ([`REMEMBERED_INTERVALS`]).
    remembered_for: Duration,
    /// Each change as it is raised, for what keeps access read outside the rooms.
    announced: broadcast::Sender<AccessChange>,
}

/// The organization a place is in, as access changes are told apart by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Wall(Tenant);

impl Wall {
    /// The organization `app` reaches.
    pub(crate) fn of(app: &App) -> Self {
        Self(app.store().tenant())
    }

    /// An organization for the tests.
    #[cfg(test)]
    pub(crate) fn for_tests() -> Self {
        Self(Tenant::default())
    }

    /// The wall of `tenant`, for the tests ([`super::super::another_wall_for_tests`]).
    #[cfg(test)]
    pub(crate) fn of_tenant(tenant: Tenant) -> Self {
        Self(tenant)
    }
}

/// A change of access facts, announced as it is raised to what holds access read
/// before it outside the rooms (who is on a board).
#[derive(Debug, Clone)]
pub(crate) struct AccessChange {
    tenant: Tenant,
    scope: Arc<AccessScope>,
}

impl AccessChange {
    /// Whether the change may have reached the card `card_id` (none if empty) of the
    /// board `container_id` of the workspace `workspace_id`, behind `wall`; for anybody
    /// there, whichever user's facts it was about.
    pub(crate) fn reaches(
        &self,
        wall: Wall,
        workspace_id: &str,
        container_id: &str,
        card_id: &str,
    ) -> bool {
        self.tenant == wall.0 && self.scope.reaches(card_id, container_id, workspace_id)
    }

    /// Whether the change is about `user_id`'s facts (or everybody's).
    pub(crate) fn concerns(&self, user_id: &str) -> bool {
        self.scope.concerns(user_id)
    }

    /// A change of `scope` behind `wall`.
    #[cfg(test)]
    pub(crate) fn for_tests(scope: AccessScope, wall: Wall) -> Self {
        Self {
            tenant: wall.0,
            scope: Arc::new(scope),
        }
    }
}

impl Collab {
    /// The access changes raised from now on, of every organization.
    pub(crate) fn access_changes(&self) -> broadcast::Receiver<AccessChange> {
        self.inner.rooms.walls.announced.subscribe()
    }
}

#[derive(Debug, Default)]
struct Changes {
    /// The number of the latest change.
    now: u64,
    /// The number of the latest change of an organization that is no longer
    /// remembered at all (none of its changes was left).
    forgotten: u64,
    by_tenant: HashMap<Tenant, Kept>,
}

/// The changes of one organization remembered, oldest first.
#[derive(Debug, Default)]
struct Kept {
    /// The number of its latest change no longer remembered.
    forgotten: u64,
    changes: VecDeque<Change>,
}

#[derive(Debug)]
struct Change {
    number: u64,
    at: Instant,
    /// What it reached; shared with whoever asks what changed since.
    scope: Arc<AccessScope>,
}

impl Kept {
    /// Forgets the changes made before `before`, and more than the last
    /// [`REMEMBERED_AT_MOST`].
    fn forget(&mut self, before: Instant) {
        while let Some(oldest) = self.changes.front()
            && (self.changes.len() > REMEMBERED_AT_MOST || oldest.at < before)
        {
            self.forgotten = oldest.number;
            self.changes.pop_front();
        }
    }
}

/// The changes of one organization after a number, as what they reached: what was
/// read before them is stale where they reach.
#[derive(Debug)]
pub(super) enum Since {
    Known(Vec<Arc<AccessScope>>),
    /// Some are no longer remembered: they may have reached anything.
    Forgotten,
}

impl Since {
    /// Whether one of the changes reaches `user_id` in `room`.
    pub(super) fn reaches(&self, room: &Room, user_id: &str) -> bool {
        match self {
            Self::Known(scopes) => scopes
                .iter()
                .any(|scope| scope.contains(room) && scope.concerns(user_id)),
            Self::Forgotten => true,
        }
    }
}

impl Walls {
    /// No change yet, for rooms asked about again every `recheck`.
    pub(super) fn new(recheck: Duration) -> Self {
        Self {
            changes: Mutex::default(),
            remembered_for: REMEMBERED_FOR.max(recheck.saturating_mul(REMEMBERED_INTERVALS)),
            announced: broadcast::Sender::new(ANNOUNCED_AT_MOST),
        }
    }

    /// The number of the latest change of any organization: what is read from now on
    /// is measured against it.
    pub(super) fn now(&self) -> u64 {
        self.changes().now
    }

    /// Records that `tenant`'s access facts changed where `scope` reaches; the number
    /// of the change.
    pub(super) fn raise(&self, tenant: Tenant, scope: AccessScope) -> u64 {
        let mut changes = self.changes();
        changes.now += 1;
        let (number, at) = (changes.now, Instant::now());
        let before = at.checked_sub(self.remembered_for).unwrap_or(at);
        // An organization swept away comes back with what was forgotten of it then.
        let forgotten = changes.forgotten;
        let kept = changes.by_tenant.entry(tenant).or_insert_with(|| Kept {
            forgotten,
            changes: VecDeque::new(),
        });
        let scope = Arc::new(scope);
        kept.changes.push_back(Change {
            number,
            at,
            scope: Arc::clone(&scope),
        });
        kept.forget(before);
        if number.is_multiple_of(SWEPT_EVERY) {
            changes.sweep(before);
        }
        drop(changes);
        // Nobody listening is no error: there is nothing outside the rooms to recheck.
        let _ = self.announced.send(AccessChange { tenant, scope });
        number
    }

    /// The changes of `tenant` after the change numbered `since`.
    pub(super) fn since(&self, tenant: Tenant, since: u64) -> Since {
        let changes = self.changes();
        let Some(kept) = changes.by_tenant.get(&tenant) else {
            return if since < changes.forgotten {
                Since::Forgotten
            } else {
                Since::Known(Vec::new())
            };
        };
        if since < kept.forgotten {
            return Since::Forgotten;
        }
        let scopes = kept
            .changes
            .iter()
            .rev()
            .take_while(|change| change.number > since)
            .map(|change| Arc::clone(&change.scope))
            .collect();
        Since::Known(scopes)
    }

    fn changes(&self) -> std::sync::MutexGuard<'_, Changes> {
        self.changes.lock().expect("the walls are not poisoned")
    }
}

impl Changes {
    /// Forgets every organization's changes made before `before`, and the
    /// organizations with none left.
    fn sweep(&mut self, before: Instant) {
        let mut forgotten = self.forgotten;
        self.by_tenant.retain(|_, kept| {
            kept.forget(before);
            let left = !kept.changes.is_empty();
            if !left {
                forgotten = forgotten.max(kept.forgotten);
            }
            left
        });
        self.forgotten = forgotten;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How many changes of the organization came after `since`; `None` if some are
    /// forgotten.
    fn after(walls: &Walls, since: u64) -> Option<usize> {
        match walls.since(Tenant::default(), since) {
            Since::Known(scopes) => Some(scopes.len()),
            Since::Forgotten => None,
        }
    }

    fn raise(walls: &Walls) {
        walls.raise(Tenant::default(), AccessScope::container("board"));
    }

    #[test]
    fn what_was_read_is_measured_against_the_changes_after_it() {
        let walls = Walls::new(Duration::from_secs(30));
        let before = walls.now();
        raise(&walls);
        let between = walls.now();
        raise(&walls);
        assert_eq!(after(&walls, before), Some(2));
        assert_eq!(after(&walls, between), Some(1));
        assert_eq!(after(&walls, walls.now()), Some(0));
    }

    #[test]
    fn what_was_read_before_a_change_no_longer_remembered_is_read_again() {
        let walls = Walls::new(Duration::from_secs(30));
        let before = walls.now();
        for _ in 0..=REMEMBERED_AT_MOST {
            raise(&walls);
        }
        assert_eq!(after(&walls, before), None);
        assert_eq!(after(&walls, walls.now()), Some(0));
    }

    #[tokio::test(start_paused = true)]
    async fn changes_are_forgotten_with_time() {
        let walls = Walls::new(Duration::from_secs(30));
        let before = walls.now();
        raise(&walls);
        tokio::time::advance(REMEMBERED_FOR + Duration::from_secs(1)).await;
        raise(&walls);
        assert_eq!(after(&walls, before), None, "a forgotten change counts");
        assert_eq!(after(&walls, walls.now()), Some(0));
        assert_eq!(
            walls.changes().by_tenant[&Tenant::default()].changes.len(),
            1
        );
    }

    #[tokio::test(start_paused = true)]
    async fn an_organization_whose_changes_are_all_forgotten_is_swept_away() {
        let walls = Walls::new(Duration::from_secs(30));
        let before = walls.now();
        raise(&walls);
        tokio::time::advance(REMEMBERED_FOR + Duration::from_secs(1)).await;
        walls.changes().sweep(Instant::now() - REMEMBERED_FOR);
        assert!(walls.changes().by_tenant.is_empty());
        assert_eq!(
            after(&walls, before),
            None,
            "what was read before it is read again"
        );
        assert_eq!(after(&walls, walls.now()), Some(0));
    }
}
