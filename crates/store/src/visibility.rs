//! Revisions for cached authorization reads, spanning the whole writing transaction.
//!
//! Each organization has a revision of its own, which its writes move on, behind a
//! lock of its own: what one organization writes neither empties another's cached
//! reads nor waits for another's dispatch. A reader of no organization takes a
//! revision that every write moves on.
//!
//! A write through a handle of no organization or of every one ([`Tenant::Unset`],
//! [`Tenant::System`]: the housekeeping, the steps after a migration, finding out whose
//! something is) moves no organization's revision for what it throws away of the
//! trash, which changes nobody's access. One that writes a table of who may see what
//! (memberships, sharing, boards, workspaces, organizations, providers) cannot say
//! whose rows those were, and moves every organization's: none does today, as what
//! changes an organization's people's access is written through that organization's
//! handle ([`Store::for_org`]).
//!
//! A write that changes what is read of one organization without writing a table of
//! it says which, whatever handle it is written through ([`Writing::invalidate_of`]):
//! an account that asks to be deleted, or takes that back, or is given its first
//! password, changes what its sessions and the links of its own space open, by its
//! own row alone. That moves the space's revision and no other organization's.
//!
//! A write abandoned before its outcome was known (its future was dropped between its
//! first authorization statement and its commit or rollback) may still land. For
//! a while after that ([`settle_after`]) nothing of its scope is cached: every check
//! reads the store anew, and a dispatch is still checked against concurrent writers,
//! but none is refused for it.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::{Duration, Instant};

use sqlx::types::Uuid;
use tokio::sync::watch;

use crate::Tenant;

/// The least time after an abandoned authorization write that reads are not cached:
/// longer than a commit that was already sent takes on PostgreSQL with asynchronous
/// or local commit (a synchronous standby that stalls can hold it longer).
const SETTLE: Duration = Duration::from_secs(60);

/// Maximum time [`Visibility::version`] waits for an active authorization writer to
/// finish before falling back to an uncached snapshot (`cacheable: false`), preventing a
/// long-running write transaction from stalling reads across the organization.
const VERSION_WAIT: Duration = Duration::from_millis(250);

/// How long after an abandoned authorization write reads are not cached, for a store
/// whose statements may wait up to `longest_wait` for another writer (SQLite's busy
/// timeout): by then the write has landed or never will. A commit later than that
/// is not seen until the next write of the same scope moves its revision on.
pub(crate) fn settle_after(longest_wait: Duration) -> Duration {
    SETTLE.max(longest_wait.saturating_mul(2))
}

mod count;
use count::{Count, Guarded};
pub use count::{VisibilityScope, VisibilityVersion};

/// Whose rows a reader depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Reach {
    Org(Uuid),
    Every,
}

impl Reach {
    fn of_reader(tenant: Tenant) -> Self {
        match tenant {
            Tenant::Org(org) => Self::Org(org),
            // What it reads may be of any organization.
            Tenant::Unset | Tenant::System => Self::Every,
        }
    }
}

/// What a write moves on once it touches who may see what: its organization's
/// revision, or, for a handle of no organization or every one, the revision every
/// organization's includes. The revision of every write moves with either.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Moves {
    Org(Uuid),
    All,
}

impl From<Tenant> for Moves {
    fn from(tenant: Tenant) -> Self {
        match tenant {
            Tenant::Org(org) => Self::Org(org),
            Tenant::Unset | Tenant::System => Self::All,
        }
    }
}

#[derive(Debug)]
pub(crate) struct Visibility {
    /// Every write: for readers of no organization.
    every: Guarded,
    /// Writes of no organization or of every one to a table of who may see what,
    /// which every organization's revision includes. A dispatch of an organization
    /// reads it shared, so they do not wait for each other; such a write, which no
    /// request makes, waits for them.
    shared: RwLock<Count>,
    /// Each organization's, by its identifier. The map is held only to find one.
    orgs: Mutex<HashMap<Uuid, Guarded>>,
    /// Moves on whenever a writer starts or ends, for whoever waits for one to end.
    changed: watch::Sender<u64>,
    settle: Duration,
}

impl Default for Visibility {
    fn default() -> Self {
        Self::settling_for(SETTLE)
    }
}

impl Visibility {
    pub(crate) fn settling_for(settle: Duration) -> Self {
        Self {
            every: Guarded::default(),
            shared: RwLock::default(),
            orgs: Mutex::default(),
            changed: watch::channel(0).0,
            settle,
        }
    }

    /// The count of an organization, made the first time it is asked for.
    fn org(&self, org: Uuid) -> Guarded {
        Arc::clone(
            self.orgs
                .lock()
                .expect("visibility organizations lock")
                .entry(org)
                .or_default(),
        )
    }

    /// The count of an organization, if already created and its map lock is unpoisoned.
    fn try_org(&self, org: Uuid) -> Option<Guarded> {
        self.orgs.lock().ok()?.get(&org).map(Arc::clone)
    }

    pub(crate) fn writing(&self, tenant: Tenant) -> Writing<'_> {
        Writing {
            visibility: self,
            moves: tenant.into(),
            touched: AtomicBool::new(false),
            started: Mutex::default(),
            finished: AtomicBool::new(false),
        }
    }

    /// Starts a writer on the primary count (`org` or `shared`).
    fn start_primary(&self, moves: Moves) {
        match moves {
            Moves::Org(org) => lock(&self.org(org)).start(),
            Moves::All => self.shared.write().expect("visibility shared lock").start(),
        }
    }

    /// Starts a writer on `every`.
    fn start_every(&self) {
        lock(&self.every).start();
    }

    /// Ends a writer on the primary count if its lock is unpoisoned (for `Drop`, which
    /// must never panic during unwinding).
    fn try_end_primary(&self, moves: Moves, unsettled: Option<Instant>) {
        match moves {
            Moves::Org(org) => {
                if let Some(guarded) = self.try_org(org)
                    && let Ok(mut count) = guarded.lock()
                {
                    count.end(unsettled);
                }
            }
            Moves::All => {
                if let Ok(mut count) = self.shared.write() {
                    count.end(unsettled);
                }
            }
        }
    }

    /// Ends `writers` increments on `every` if its lock is unpoisoned.
    fn try_end_every(&self, writers: usize, unsettled: Option<Instant>) {
        if let Ok(mut count) = self.every.lock() {
            count.end_many(writers, unsettled);
        }
    }

    /// Wakes readers waiting on active writers.
    fn notify(&self) {
        self.changed
            .send_modify(|step| *step = step.wrapping_add(1));
    }

    /// Reads the counts of `reach` under their locks and gives the version they make
    /// to `with`, still under them: the shared count's and the organization's for an
    /// organization, the count of every write for none.
    fn under_locks<T>(&self, reach: Reach, with: impl FnOnce(usize, VisibilityVersion) -> T) -> T {
        let now = Instant::now();
        match reach {
            Reach::Org(org) => {
                let shared = self.shared.read().expect("visibility shared lock");
                let count = self.org(org);
                let own = lock(&count);
                with(
                    shared.writers + own.writers,
                    VisibilityVersion {
                        reach,
                        shared: shared.revision,
                        revision: own.revision,
                        cacheable: shared.settled_at(now) && own.settled_at(now),
                    },
                )
            }
            Reach::Every => {
                let every = lock(&self.every);
                with(
                    every.writers,
                    VisibilityVersion {
                        reach,
                        shared: 0,
                        revision: every.revision,
                        cacheable: every.settled_at(now),
                    },
                )
            }
        }
    }

    pub(crate) fn if_current(&self, version: VisibilityVersion, send: impl FnOnce()) -> bool {
        self.under_locks(version.reach, |writers, now| {
            if writers != 0 || (now.shared, now.revision) != (version.shared, version.revision) {
                return false;
            }
            send();
            true
        })
    }

    pub(crate) async fn version(&self, tenant: Tenant) -> VisibilityVersion {
        self.version_within(tenant, VERSION_WAIT).await
    }

    async fn version_within(&self, tenant: Tenant, max_wait: Duration) -> VisibilityVersion {
        let reach = Reach::of_reader(tenant);
        let mut changes = self.changed.subscribe();
        let deadline = tokio::time::Instant::now() + max_wait;
        loop {
            let settled =
                self.under_locks(reach, |writers, version| (writers == 0).then_some(version));
            if let Some(version) = settled {
                return version;
            }
            // Subscribed before the counts were read, so a commit cannot be missed.
            if tokio::time::timeout_at(deadline, changes.changed())
                .await
                .is_err()
            {
                return self.under_locks(reach, |_, mut version| {
                    version.cacheable = false;
                    version
                });
            }
        }
    }
}

fn lock(count: &Mutex<Count>) -> MutexGuard<'_, Count> {
    count.lock().expect("visibility revision lock")
}

/// Which counts a [`Writing`] actually incremented, so `Drop` releases only what
/// succeeded and never panics if a lock was poisoned.
#[derive(Debug, Default)]
struct Started {
    /// The organizations named via [`Writing::invalidate_of`].
    named: Vec<Uuid>,
    /// The scopes whose primary count (`org` or `shared`) was incremented.
    primaries: Vec<Moves>,
    /// How many times `every` was incremented.
    every: usize,
}

/// Lives outside the connection/transaction future, including its commit or rollback.
/// Abandonment moves the revision on and keeps reads from being cached for a while.
#[derive(Debug)]
pub(crate) struct Writing<'a> {
    visibility: &'a Visibility,
    moves: Moves,
    touched: AtomicBool,
    /// What this write has incremented on [`Visibility`]. Never held while locking
    /// `self.visibility`.
    started: Mutex<Started>,
    finished: AtomicBool,
}

impl Writing<'_> {
    pub(crate) fn statement(&self, text: &str) {
        if self.touched.load(Ordering::Relaxed) || !affects_visibility(text) {
            return;
        }
        // A handle of no organization cannot say whose rows it wrote: it moves every
        // organization's revision, unless it wrote only what changes no access.
        if self.moves == Moves::All && !writes_access(text) {
            return;
        }
        self.invalidate();
    }

    pub(crate) fn invalidate(&self) {
        if self.touched.swap(true, Ordering::Relaxed) {
            return;
        }
        self.start(self.moves);
    }

    /// Moves on the revision of the organization `org`, which the write changes what
    /// is read of without writing a table of it, whatever the handle: from here to the
    /// end of the write, as a write through the organization's own handle would.
    pub(crate) fn invalidate_of(&self, org: Uuid) {
        if self.moves == Moves::Org(org) {
            return self.invalidate();
        }
        {
            let mut started = self.started.lock().expect("visibility started lock");
            if started.named.contains(&org) {
                return;
            }
            started.named.push(org);
        }
        self.start(Moves::Org(org));
    }

    /// Increments the primary and `every` counts for `moves`, recording each step only
    /// after it succeeds so unwinding drops only what was actually started.
    fn start(&self, moves: Moves) {
        self.visibility.start_primary(moves);
        self.started
            .lock()
            .expect("visibility started lock")
            .primaries
            .push(moves);
        self.visibility.start_every();
        self.started.lock().expect("visibility started lock").every += 1;
        self.visibility.notify();
    }

    pub(crate) fn settled(&self) {
        self.finished.store(true, Ordering::Relaxed);
    }
}

impl Drop for Writing<'_> {
    fn drop(&mut self) {
        let Ok(started) = self.started.get_mut() else {
            return;
        };
        if started.primaries.is_empty() && started.every == 0 {
            return;
        }
        let unsettled = (!self.finished.load(Ordering::Relaxed))
            .then(|| Instant::now() + self.visibility.settle);
        for moves in std::mem::take(&mut started.primaries) {
            self.visibility.try_end_primary(moves, unsettled);
        }
        if started.every > 0 {
            self.visibility.try_end_every(started.every, unsettled);
        }
        self.visibility.notify();
    }
}

/// Unqualified table identifier of a SQL token, stripping any `schema.` prefix.
fn bare_table(token: &str) -> &str {
    token.rsplit('.').next().unwrap_or(token)
}

/// Tokenizes SQL text preserving `.` inside schema-qualified table names (`public.users`).
fn sql_tokens(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '.')
        .map(|token| token.trim_matches('.'))
        .filter(|token| !token.is_empty())
}

/// Whether a statement writes a table of who may see what, by the tables it writes and
/// not those it only reads: not the trash (`deleted_containers`, `deleted_blocks`), nor
/// blocks, whose going changes nobody's access, as the purge of the trash writes them;
/// a user's row only as it goes, with the user's memberships.
fn writes_access(text: &str) -> bool {
    const TABLES: [&str; 11] = [
        "workspace_members",
        "teams",
        "team_members",
        "container_teams",
        "organization_members",
        "container_members",
        "public_links",
        "workspaces",
        "organizations",
        "containers",
        "oidc_providers",
    ];
    let words: Vec<&str> = sql_tokens(text).collect();
    for (at, &verb) in words.iter().enumerate() {
        let deletes = verb.eq_ignore_ascii_case("from")
            && at > 0
            && words[at - 1].eq_ignore_ascii_case("delete");
        let written =
            verb.eq_ignore_ascii_case("update") || verb.eq_ignore_ascii_case("into") || deletes;
        if !written {
            continue;
        }
        let mut target_at = at + 1;
        if words
            .get(target_at)
            .is_some_and(|w| w.eq_ignore_ascii_case("only"))
        {
            target_at += 1;
        }
        let Some(&raw_table) = words.get(target_at) else {
            continue;
        };
        let table = bare_table(raw_table);
        if TABLES.iter().any(|name| table.eq_ignore_ascii_case(name))
            || (deletes && table.eq_ignore_ascii_case("users"))
        {
            return true;
        }
    }
    false
}

/// Every statement is built by the store. Treat any write mentioning these tables
/// conservatively, including writes with subqueries and the test-only raw SQL path.
/// Ordinary card/body writes stay cacheable; type changes invalidate explicitly,
/// and deleting or restoring cards changes their visibility through the trash.
/// Of a user's row a cached check reads only whether it has a password (which
/// sessions the user's own space takes), and the write of a first one says so
/// ([`Writing::invalidate_of`]); changing it otherwise (a name, another password) moves
/// nothing. Deleting one does, as it takes the user's memberships with it.
fn affects_visibility(text: &str) -> bool {
    const TABLES: [&str; 13] = [
        "workspace_members",
        // A team's people and its roles on boards make board members too.
        "teams",
        "team_members",
        "container_teams",
        "organization_members",
        "container_members",
        "public_links",
        "workspaces",
        "organizations",
        "containers",
        "deleted_containers",
        "deleted_blocks",
        // Which provider an organization takes sessions of (`sso_required`).
        "oidc_providers",
    ];
    if notes_for_later(text) {
        return false;
    }
    let mut deletes = false;
    let mut cascading = false;
    for token in sql_tokens(text) {
        let word = bare_table(token);
        if TABLES.iter().any(|table| word.eq_ignore_ascii_case(table)) {
            return true;
        }
        deletes |= word.eq_ignore_ascii_case("delete");
        cascading |= word.eq_ignore_ascii_case("blocks") || word.eq_ignore_ascii_case("users");
    }
    deletes && cascading
}

/// The lists a write keeps of what to do later: whom to tell, what to index again, who
/// watches what. None of them lets anybody see anything, though the statements that
/// fill them join the boards and the workspaces to find whose rows they note.
const NOTED_FOR_LATER: [&str; 3] = ["notification_outbox", "search_outbox", "subscriptions"];

/// Whether `text` only inserts rows into one of [`NOTED_FOR_LATER`].
fn notes_for_later(text: &str) -> bool {
    if text.contains(';') {
        return false;
    }
    let mut words = sql_tokens(text);
    matches!(
        (words.next(), words.next(), words.next()),
        (Some(insert), Some(into), Some(table))
            if insert.eq_ignore_ascii_case("insert")
                && into.eq_ignore_ascii_case("into")
                && NOTED_FOR_LATER
                    .iter()
                    .any(|list| bare_table(table).eq_ignore_ascii_case(list))
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Store;

    #[tokio::test]
    async fn what_a_write_notes_for_later_moves_no_revision() {
        // A handle of one organization: one of no organization moves nothing on what
        // only reads the tables of access anyway.
        let visibility = Visibility::default();
        let of_first = org(1);
        let old = visibility.version(of_first).await;
        let writing = visibility.writing(of_first);
        for text in [
            "INSERT INTO subscriptions (block_id) SELECT b.id FROM blocks AS b \
             JOIN containers AS bo ON bo.id = b.container_id",
            "INSERT INTO notification_outbox (org_id) SELECT w.org_id FROM containers AS bo \
             JOIN workspaces AS w ON w.id = bo.workspace_id",
            "INSERT INTO search_outbox (scope) SELECT 'block' FROM blocks AS b \
             JOIN containers AS bo ON bo.id = b.container_id",
        ] {
            writing.statement(text);
        }
        writing.settled();
        drop(writing);
        assert_eq!(visibility.version(of_first).await, old, "no access changed");

        for text in [
            "INSERT INTO container_members (container_id) SELECT id FROM containers",
            "UPDATE subscriptions SET notified_at = 1 WHERE block_id IN \
             (SELECT id FROM blocks WHERE container_id IN (SELECT id FROM containers))",
        ] {
            let before = visibility.version(of_first).await;
            let writing = visibility.writing(of_first);
            writing.statement(text);
            writing.settled();
            drop(writing);
            assert_ne!(visibility.version(of_first).await, before, "{text}");
        }
    }

    fn org(n: u128) -> Tenant {
        Tenant::Org(Uuid::from_u128(n))
    }

    #[tokio::test]
    async fn a_dispatch_requires_the_same_settled_revision() {
        let visibility = Visibility::default();
        let old = visibility.version(Tenant::Unset).await;
        let writing = visibility.writing(Tenant::Unset);
        writing.statement("UPDATE workspace_members SET role = 'guest'");
        assert!(!visibility.if_current(old, || panic!("a writer is active")));
        writing.settled();
        drop(writing);
        assert!(!visibility.if_current(old, || panic!("the revision changed")));
        let current = visibility.version(Tenant::Unset).await;
        let mut sent = false;
        assert!(visibility.if_current(current, || sent = true));
        assert!(sent);
    }

    #[tokio::test]
    async fn an_abandoned_write_stops_caching_for_a_while_but_not_dispatch() {
        let visibility = Visibility::settling_for(Duration::from_millis(50));
        let writing = visibility.writing(Tenant::Unset);
        writing.statement("DELETE FROM public_links");
        drop(writing);
        let unsettled = visibility.version(Tenant::Unset).await;
        assert!(!unsettled.cacheable(), "its outcome may still land");
        let mut sent = false;
        assert!(
            visibility.if_current(unsettled, || sent = true),
            "a dispatch checked against the store anew goes on"
        );
        assert!(sent);
        let writing = visibility.writing(Tenant::Unset);
        writing.statement("UPDATE containers SET title = 'later'");
        writing.settled();
        drop(writing);
        assert!(
            !visibility.version(Tenant::Unset).await.cacheable(),
            "another successful write does not prove the abandoned one settled"
        );
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(
            visibility.version(Tenant::Unset).await.cacheable(),
            "after the settle time reads are cached again"
        );
    }

    #[tokio::test]
    async fn one_organizations_writes_leave_anothers_revision() {
        let visibility = Visibility::default();
        let (first, second) = (org(1), org(2));
        let of_second = visibility.version(second).await;
        let of_everything = visibility.version(Tenant::Unset).await;
        let writing = visibility.writing(first);
        writing.statement("UPDATE container_members SET role = 'admin'");
        assert!(
            visibility.if_current(of_second, || {}),
            "a writer of the first organization does not hold up the second's dispatch"
        );
        writing.settled();
        drop(writing);
        assert_eq!(visibility.version(second).await, of_second);
        assert_ne!(visibility.version(Tenant::Unset).await, of_everything);
        assert!(!visibility.if_current(of_everything, || panic!("everything moved on")));

        assert_ne!(
            visibility.version(second).await.scope(),
            visibility.version(first).await.scope()
        );
    }

    #[tokio::test]
    async fn the_systems_purge_of_the_trash_moves_no_revision() {
        let visibility = Visibility::default();
        let of_first = visibility.version(org(1)).await;
        let of_everything = visibility.version(Tenant::Unset).await;
        let writing = visibility.writing(Tenant::System);
        writing.statement("DELETE FROM deleted_containers WHERE deleted_at < 1");
        writing.statement(
            "DELETE FROM deleted_blocks WHERE NOT EXISTS (SELECT 1 FROM containers b \
             WHERE b.id = deleted_blocks.container_id)",
        );
        writing.statement("DELETE FROM blocks WHERE container_id = $1");
        assert!(
            visibility.if_current(of_first, || {}),
            "a pending purge holds up no organization's dispatch"
        );
        assert!(visibility.if_current(of_everything, || {}));
        drop(writing);
        assert_eq!(visibility.version(org(1)).await, of_first);
        assert_eq!(
            visibility.version(Tenant::Unset).await,
            of_everything,
            "not even abandoned"
        );
    }

    #[tokio::test]
    async fn a_write_of_no_organization_to_who_may_see_what_moves_every_organization() {
        for tenant in [Tenant::Unset, Tenant::System] {
            let visibility = Visibility::default();
            let of_first = visibility.version(org(1)).await;
            let of_everything = visibility.version(Tenant::Unset).await;
            let writing = visibility.writing(tenant);
            writing.statement("UPDATE workspace_members SET role = 'guest'");
            assert!(
                !visibility.if_current(of_first, || panic!("a writer is active")),
                "{tenant:?} cannot say whose rows it writes"
            );
            assert!(!visibility.if_current(of_everything, || panic!("a writer is active")));
            writing.settled();
            drop(writing);
            assert_ne!(visibility.version(org(1)).await, of_first);
            assert_ne!(visibility.version(Tenant::Unset).await, of_everything);
        }
    }

    #[test]
    fn what_a_statement_writes_is_told_from_what_it_reads() {
        assert!(writes_access("UPDATE workspace_members SET role = $1"));
        assert!(writes_access("INSERT INTO public_links (id) VALUES ($1)"));
        assert!(writes_access("DELETE FROM users WHERE id = $1"));
        assert!(writes_access(
            "WITH gone AS (DELETE FROM container_members RETURNING 1) SELECT 1"
        ));
        assert!(!writes_access("UPDATE users SET props = $1"));
        assert!(!writes_access(
            "DELETE FROM deleted_containers WHERE deleted_at < $1"
        ));
        assert!(!writes_access(
            "DELETE FROM deleted_blocks WHERE NOT EXISTS (SELECT 1 FROM containers b)"
        ));
        assert!(!writes_access(
            "DELETE FROM blocks WHERE container_id IN (SELECT id FROM containers)"
        ));
    }

    #[test]
    fn a_dispatch_holds_only_its_own_organizations_writers() {
        let visibility = Visibility::default();
        let version = VisibilityVersion {
            reach: Reach::Org(Uuid::from_u128(1)),
            shared: 0,
            revision: 0,
            cacheable: true,
        };
        std::thread::scope(|scope| {
            let (entered, entering) = std::sync::mpsc::sync_channel(0);
            assert!(visibility.if_current(version, || {
                let visibility = &visibility;
                scope.spawn(move || {
                    let writing = visibility.writing(org(2));
                    writing.statement("DELETE FROM container_members");
                    writing.settled();
                    drop(writing);
                    entered.send(()).expect("writer entered");
                });
                entering
                    .recv_timeout(Duration::from_secs(5))
                    .expect("another organization's writer goes on during the dispatch");
            }));
        });
    }

    #[tokio::test]
    async fn an_abandoned_write_unsettles_its_own_organization_only() {
        let visibility = Visibility::settling_for(Duration::from_secs(60));
        let writing = visibility.writing(org(1));
        writing.statement("DELETE FROM container_members");
        drop(writing);
        assert!(!visibility.version(org(1)).await.cacheable());
        assert!(!visibility.version(Tenant::Unset).await.cacheable());
        assert!(
            visibility.version(org(2)).await.cacheable(),
            "another organization keeps its cache"
        );
        let writing = visibility.writing(Tenant::Unset);
        writing.statement("UPDATE organizations SET deleted_at = 1");
        drop(writing);
        assert!(
            !visibility.version(org(2)).await.cacheable(),
            "an abandoned write of every organization unsettles each"
        );
    }

    #[test]
    fn a_write_of_a_teams_people_or_roles_moves_the_revision() {
        for statement in [
            "INSERT INTO team_members (team_id, org_id, workspace_id, user_id) VALUES ($1, $2, $3, $4)",
            "INSERT INTO container_teams (container_id, team_id, workspace_id, org_id, role) VALUES ($1, $2, $3, $4, $5) ON CONFLICT (container_id, team_id, org_id) DO UPDATE SET role = EXCLUDED.role",
            "DELETE FROM teams WHERE id = $1",
        ] {
            assert!(affects_visibility(statement), "{statement}");
            assert!(writes_access(statement), "{statement}");
        }
    }

    #[test]
    fn a_users_row_moves_the_revision_only_as_it_goes() {
        assert!(!affects_visibility(
            "UPDATE users SET props = $1 WHERE id = $2"
        ));
        assert!(!affects_visibility(
            "UPDATE users SET password = $1 WHERE id = $2"
        ));
        assert!(affects_visibility("DELETE FROM users WHERE id = $1"));
        assert!(affects_visibility("DELETE FROM blocks WHERE id = $1"));
        assert!(!affects_visibility("UPDATE blocks SET title = $1"));
    }

    #[tokio::test]
    async fn a_store_settles_for_twice_its_busy_timeout() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/visibility-tests")
            .join(fvoci_model::new_id());
        std::fs::create_dir_all(&directory).expect("a test directory");
        let path = directory.join("settle.db");
        let store = Store::open(
            crate::DbType::Sqlite,
            &format!("{}?busy_timeout_ms=120000", path.display()),
        )
        .await
        .expect("a store");
        assert_eq!(store.inner.visibility.settle, Duration::from_secs(240));
        store.close().await;
        std::fs::remove_dir_all(&directory).expect("the test directory goes");
    }

    #[test]
    fn the_settle_time_outlasts_the_longest_wait() {
        assert_eq!(settle_after(Duration::from_secs(5)), SETTLE);
        assert_eq!(
            settle_after(Duration::from_secs(120)),
            Duration::from_secs(240)
        );
    }

    #[test]
    fn authorization_writers_cannot_enter_a_checked_dispatch() {
        let visibility = Visibility::default();
        let version = VisibilityVersion {
            reach: Reach::Every,
            shared: 0,
            revision: 0,
            cacheable: true,
        };
        std::thread::scope(|scope| {
            let (started, starting) = std::sync::mpsc::sync_channel(0);
            let (entered, entering) = std::sync::mpsc::sync_channel(0);
            assert!(visibility.if_current(version, || {
                let visibility = &visibility;
                scope.spawn(move || {
                    let writing = visibility.writing(Tenant::Unset);
                    started.send(()).expect("writer starts");
                    writing.statement("DELETE FROM container_members");
                    entered.send(()).expect("writer entered");
                    writing.settled();
                });
                starting.recv().expect("writer attempted to enter");
                assert!(
                    entering
                        .recv_timeout(std::time::Duration::from_millis(20))
                        .is_err(),
                    "writer cannot enter until enqueue ends"
                );
            }));
            entering
                .recv_timeout(std::time::Duration::from_secs(5))
                .expect("writer enters after enqueue ends");
        });
    }

    #[tokio::test]
    async fn a_write_marked_as_changing_no_access_leaves_the_revision() {
        use crate::StoreError;
        use crate::testing::TestDatabase;
        let directory =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/visibility-tests");
        let database = TestDatabase::create(&directory).await;
        let store = Store::connect(database.db_type(), database.database_connection())
            .await
            .expect("test store");
        let write = async |marked: bool| {
            store
                .transaction(async |scope| {
                    let mut sql = scope.sql("UPDATE containers SET next_card_number = 1");
                    if marked {
                        sql.changing_no_access();
                    }
                    scope.execute(sql).await?;
                    Ok::<(), StoreError>(())
                })
                .await
                .expect("the write");
        };
        let before = store.visibility_version().await;
        write(true).await;
        assert_eq!(store.visibility_version().await, before, "a marked write");
        write(false).await;
        assert_ne!(
            store.visibility_version().await,
            before,
            "the same write unmarked"
        );
        store.close().await;
        database.remove().await;
    }

    #[tokio::test]
    async fn confirmed_rollback_is_settled_but_abandoned_transaction_is_not() {
        use crate::StoreError;
        use crate::testing::TestDatabase;
        let directory =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/visibility-tests");
        let database = TestDatabase::create(&directory).await;
        let store = Store::connect(database.db_type(), database.database_connection())
            .await
            .expect("test store");
        let error: Result<(), StoreError> = store
            .transaction(async |scope| {
                let write = scope.sql("UPDATE containers SET title = title");
                scope.execute(write).await?;
                let invalid = scope.sql("SELECT missing_visibility_test_column FROM users");
                scope.fetch_optional(invalid).await?;
                Ok(())
            })
            .await;
        assert!(matches!(error, Err(StoreError::Database(_))));
        assert!(
            store.visibility_version().await.cacheable(),
            "rollback confirmed the failed write settled"
        );
        let reached = tokio::sync::Notify::new();
        let mut writing = Box::pin(store.transaction(async |scope| {
            let write = scope.sql("UPDATE containers SET title = title");
            scope.execute(write).await?;
            reached.notify_one();
            std::future::pending::<Result<(), StoreError>>().await
        }));
        tokio::select! {
            result = &mut writing => panic!("transaction must remain pending: {result:?}"),
            () = reached.notified() => {}
        }
        drop(writing);
        let version = store.inner.visibility.version(store.tenant).await;
        assert!(
            !version.cacheable(),
            "an abandoned transaction's reads are not cached until it settles"
        );
        let mut sent = false;
        assert!(store.if_visibility_current(version, || sent = true));
        assert!(sent, "a dispatch checked anew is not refused for it");
        store.close().await;
        database.remove().await;
    }

    #[tokio::test]
    async fn a_poisoned_organization_lock_does_not_abort_when_writing_drops() {
        let visibility = Visibility::default();
        let poisoned_org = Uuid::from_u128(1);
        let healthy_org = Uuid::from_u128(2);
        let version = visibility.version(Tenant::Org(poisoned_org)).await;
        let panic_in_send = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            visibility.if_current(version, || panic!("caller callback panicked"));
        }));
        assert!(panic_in_send.is_err(), "callback panic propagates");

        // Writing to the poisoned org panics on start, and unwinding through Drop must
        // not double-panic and abort the process.
        let direct = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let writing = visibility.writing(Tenant::Org(poisoned_org));
            writing.invalidate();
        }));
        assert!(direct.is_err(), "poisoned lock still panics on start");

        // Same for `invalidate_of` from a write of another organization: unwinding
        // drops `Writing`, releasing the healthy organization's count without aborting.
        let cross = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let writing = visibility.writing(Tenant::Org(healthy_org));
            writing.invalidate();
            writing.invalidate_of(poisoned_org);
        }));
        assert!(cross.is_err(), "poisoned named org panics on start");

        // The healthy organization's writer count and `every` writer count were both
        // released on unwind.
        let healthy_version = visibility.version(Tenant::Org(healthy_org)).await;
        assert!(
            !healthy_version.cacheable(),
            "abandoned write unsettles healthy org"
        );
        let mut dispatched = false;
        assert!(visibility.if_current(healthy_version, || dispatched = true));
        assert!(dispatched);

        let every_version = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            visibility.version(Tenant::Unset),
        )
        .await
        .expect("every writer count released on unwind");
        assert!(
            !every_version.cacheable(),
            "abandoned write unsettles every"
        );
    }
}
