//! Row locks: how a statement holds what it reads, the rank each lock is taken at, and
//! compile-time witnesses of what a transaction holds.
//!
//! A statement holds rows only through [`Scope::row_lock`], [`Scope::rows_lock`],
//! [`Scope::hold`] and [`Scope::first_held`], which record the lock at its rank as they
//! take it: a debug build asserts that a transaction takes its locks in the order of
//! [`LockRank`]. A statement that skips what others hold ([`Scope::skipping_held`])
//! waits for nobody and is not recorded, nor are the locks no rank fits
//! ([`Scope::unranked_row_lock`], [`Unranked`]). Their clause is a [`LockClause`], which
//! only [`Sql::push_lock`] takes.
//!
//! No other module of the crate writes a locking clause: `cargo xtask check` refuses one
//! in a literal ("layers"), and a debug build one in a statement that no
//! [`Sql::push_lock`] ended ([`check_clause`]), however its text was put together.

use std::marker::PhantomData;

use super::{Dialect, FromRow, Scope, Sql, SqlFragment, is_sql_ident};
use crate::error::StoreError;

/// The clause a lock of [`Scope::row_lock`], [`Scope::skipping_held`] or
/// [`Scope::unranked_row_lock`] is taken by, for [`Sql::push_lock`]: what is recorded is
/// held only by the statement that it ends.
#[derive(Debug)]
#[must_use = "a lock that is recorded and not pushed is not taken"]
pub(crate) struct LockClause(SqlFragment);

impl Sql {
    /// Ends the statement with `clause`, by which it holds the rows it reads. The clause
    /// is used up: one recorded lock ends one statement.
    pub(crate) fn push_lock(&mut self, clause: LockClause) -> &mut Self {
        let LockClause(clause) = clause;
        self.may_lock = true;
        self.push(&clause)
    }
}

/// Fails, in a debug build, if `sql` holds a locking clause ([`holds_locking_clause`])
/// and no [`Sql::push_lock`] ended it: a lock no rank records, written however its
/// text was put together. A statement of raw text, which only tests write, is let be.
pub(super) fn check_clause(sql: &Sql) {
    debug_assert!(
        sql.may_lock || !holds_locking_clause(&sql.text),
        "a locking clause that no Scope::row_lock wrote: {}",
        sql.text
    );
}

/// The words after `FOR` that make a locking clause, in lowercase.
const LOCKS: [&[&str]; 4] = [
    &["update"],
    &["no", "key", "update"],
    &["share"],
    &["key", "share"],
];

/// Whether `text` holds a locking clause: `FOR` and then `UPDATE`, `NO KEY UPDATE`,
/// `SHARE` or `KEY SHARE`, as words in any case with anything but a word between.
fn holds_locking_clause(text: &str) -> bool {
    let words: Vec<String> = without_comments(text)
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect();
    words.iter().enumerate().any(|(at, word)| {
        let after = &words[at + 1..];
        word == "for"
            && LOCKS.iter().any(|lock| {
                lock.len() <= after.len() && after.iter().zip(lock.iter()).all(|(a, b)| a == b)
            })
    })
}

/// `text` with its comments (`/* … */`, `-- …`) put out of it, each as a space.
fn without_comments(text: &str) -> String {
    let mut kept = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(['/', '-']) {
        let (before, from) = rest.split_at(at);
        kept.push_str(before);
        let (end, after) = if let Some(inside) = from.strip_prefix("/*") {
            ("*/", inside)
        } else if let Some(inside) = from.strip_prefix("--") {
            ("\n", inside)
        } else {
            kept.push_str(&from[..1]);
            rest = &from[1..];
            continue;
        };
        kept.push(' ');
        rest = after.split_once(end).map_or("", |(_, past)| past);
    }
    kept.push_str(rest);
    kept
}

/// Why a lock has no place in the order of [`LockRank`]: as two orders of the store take
/// it on either side of another lock, no rank fits it. A new one is a new variant, with
/// its reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Unranked {
    /// The marks of a board's files (`file_refs::hold_marks`), taken only under the
    /// board, held live ([`LockRank::Container`]) or in its trash
    /// ([`LockRank::DeletedContainers`]). Boards made whole, by a copy or an import
    /// (`containers_and_blocks::create_containers`), mark their files before the
    /// membership of whoever makes them is held (`members::hold_creator`), as a rank
    /// before [`LockRank::WorkspaceMembers`] would have them do. What keeps them from
    /// that rank is a board that comes back from the trash: it marks its files after
    /// the members it reads (`members::has_acting_admin`). The two orders meet at no
    /// row: what boards made whole mark are rows they made themselves, and the files of
    /// a board in the trash are written only under its trash. The rank is theirs once a
    /// board that comes back marks its files before it reads its members.
    MarksOfBoardFiles,
    /// The link a registration was made by (`signups::hold_link_of`), held to give its
    /// use back: after the account that a sign-in through a provider signs in, and,
    /// under the turn of the links, after an organization taken again, whose row is held
    /// already. An acceptance holds a link before the trash and the members of its
    /// workspace ([`LockRank::Invitation`]).
    LinkOfRegistration,
}

/// How a statement holds the rows it reads, on PostgreSQL. SQLite has no row locks: a
/// transaction that writes holds the whole database there (`BEGIN IMMEDIATE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RowLock {
    /// `FOR UPDATE`: as a deletion, or a change of the row's key, holds it.
    Update,
    /// `FOR NO KEY UPDATE`: as a change of the row holds it; what refers to the row is
    /// still added meanwhile.
    NoKeyUpdate,
    /// `FOR SHARE`: against any change, alongside other readers that hold it so.
    Share,
    /// `FOR KEY SHARE`: against a deletion or a change of the key only, as a reference
    /// to the row holds it.
    KeyShare,
}

impl RowLock {
    /// Where the lock stands by what it waits for, on PostgreSQL: each waits for all
    /// that those weaker than it wait for, and more (`KEY SHARE` for an `UPDATE` alone,
    /// `UPDATE` for any).
    #[cfg(debug_assertions)]
    const fn strength(self) -> u8 {
        match self {
            Self::KeyShare => 0,
            Self::Share => 1,
            Self::NoKeyUpdate => 2,
            Self::Update => 3,
        }
    }

    /// This lock on the rows of `table` alone (`OF table`), a table or an alias of the
    /// statement's join.
    ///
    /// # Panics
    ///
    /// Panics if `table` is no plain name.
    pub(crate) fn of(self, table: &'static str) -> Locking {
        assert!(
            is_sql_ident(table),
            "invalid SQL table alias in a row lock: {table:?}"
        );
        Locking {
            lock: self,
            of: Some(table),
        }
    }
}

/// A [`RowLock`], on the rows of all the tables a statement reads or of one of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Locking {
    lock: RowLock,
    of: Option<&'static str>,
}

impl From<RowLock> for Locking {
    fn from(lock: RowLock) -> Self {
        Self { lock, of: None }
    }
}

impl Locking {
    /// The clause, for the end of a `SELECT` on PostgreSQL.
    fn clause(self) -> SqlFragment {
        let mut clause = SqlFragment::of(&[match self.lock {
            RowLock::Update => " FOR UPDATE",
            RowLock::NoKeyUpdate => " FOR NO KEY UPDATE",
            RowLock::Share => " FOR SHARE",
            RowLock::KeyShare => " FOR KEY SHARE",
        }]);
        if let Some(table) = self.of {
            clause.push(" OF ").push(table);
        }
        clause
    }

    /// The clause, as [`Locking::clause`], of the rows nobody else holds.
    fn skipping_clause(self) -> SqlFragment {
        let mut clause = self.clause();
        clause.push(" SKIP LOCKED");
        clause
    }

    /// The clause on PostgreSQL; nothing on SQLite.
    fn in_dialect(self, dialect: Dialect, clause: fn(Self) -> SqlFragment) -> LockClause {
        LockClause(match dialect {
            Dialect::Postgres => clause(self),
            Dialect::Sqlite => SqlFragment::new(),
        })
    }
}

impl Scope<'_> {
    /// The clause by which the statement it ends holds the rows it reads under `lock`,
    /// which is recorded as taken at `(rank, key)`: on PostgreSQL. On SQLite, nothing.
    /// Pushed last, as the statement is about to run.
    ///
    /// The key of a lock on one row is the row's; of a lock on the rows of something
    /// (the files of a board), that something's.
    pub(crate) fn row_lock(
        &mut self,
        rank: LockRank,
        key: &str,
        lock: impl Into<Locking>,
    ) -> LockClause {
        let lock = lock.into();
        self.record_lock(rank, key, lock.lock);
        lock.in_dialect(self.dialect(), Locking::clause)
    }

    /// The clause by which the statement it ends holds the rows it reads under `lock`,
    /// each recorded as taken at `rank` under its own key, in the order of `keys`: the
    /// order the statement takes them in, which its `ORDER BY` is to give. On
    /// PostgreSQL; on SQLite, nothing.
    pub(crate) fn rows_lock<'k>(
        &mut self,
        rank: LockRank,
        keys: impl IntoIterator<Item = &'k str>,
        lock: impl Into<Locking>,
    ) -> LockClause {
        let lock = lock.into();
        for key in keys {
            self.record_lock(rank, key, lock.lock);
        }
        lock.in_dialect(self.dialect(), Locking::clause)
    }

    /// The clause by which the statement it ends holds, under `lock`, the rows it reads
    /// that no other transaction holds, and leaves out those one does (`SKIP LOCKED`):
    /// on PostgreSQL. On SQLite, nothing. Such a statement waits for nobody, so the lock
    /// has no place in the order of [`LockRank`] and is not recorded.
    pub(crate) fn skipping_held(&self, lock: impl Into<Locking>) -> LockClause {
        lock.into()
            .in_dialect(self.dialect(), Locking::skipping_clause)
    }

    /// The clause by which the statement it ends holds the rows it reads under `lock`,
    /// recorded at no rank, for the reason [`Unranked`] gives: on PostgreSQL. On SQLite,
    /// nothing.
    pub(crate) fn unranked_row_lock(&self, _: Unranked, lock: impl Into<Locking>) -> LockClause {
        lock.into().in_dialect(self.dialect(), Locking::clause)
    }

    /// Holds the rows `sql` selects under `lock`, recorded at `(rank, key)` as
    /// [`Scope::row_lock`] records it. Only PostgreSQL runs the statement; what it reads
    /// is not looked at.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the statement fails.
    pub(crate) async fn hold(
        &mut self,
        rank: LockRank,
        key: &str,
        mut sql: Sql,
        lock: impl Into<Locking>,
    ) -> Result<(), StoreError> {
        let clause = self.row_lock(rank, key, lock);
        if self.dialect() == Dialect::Postgres {
            sql.push_lock(clause);
            self.fetch_all(sql).await?;
        }
        Ok(())
    }

    /// The first row `sql` reads, held under `lock` and recorded at `rank` under the
    /// identifier its first column holds: for a row found through what it is joined
    /// to, whose key is not known before the statement reads it. Nothing is recorded
    /// if there is no row. The record comes after the lock is taken: a lock out of
    /// order is waited for before it is seen.
    ///
    /// # Errors
    ///
    /// Returns what [`Scope::first`] returns.
    pub(crate) async fn first_held<T: FromRow>(
        &mut self,
        rank: LockRank,
        mut sql: Sql,
        lock: impl Into<Locking>,
    ) -> Result<Option<T>, StoreError> {
        let lock = lock.into();
        sql.push_lock(lock.in_dialect(self.dialect(), Locking::clause));
        let Some(row) = self.fetch_optional(sql).await? else {
            return Ok(None);
        };
        let key: String = FromRow::from_row(&row)?;
        self.record_lock(rank, &key, lock.lock);
        T::from_row(&row).map(Some)
    }
}

/// Global lock acquisition rank within a transaction (`debug_assert!` checked).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum LockRank {
    Instance,
    LinkTurn,
    Organization,
    /// The pending settings of an organization's provider, keyed by the organization:
    /// a trial holds them before the provider it writes them to.
    PendingProvider,
    /// The row of an organization's provider, held before the account that signs in,
    /// tries settings or makes a token through it.
    Provider,
    Workspace,
    /// A board, and with it the file records of it: those are held under the board at
    /// its place, keyed by it, so that a write that spans boards takes each with its
    /// files in the order of the boards.
    Container,
    /// A link into a workspace or a board, under the board it is of, before the trash
    /// and the membership of the workspace an acceptance reads.
    Invitation,
    /// An exam or a meeting of a course, under the course's board, before the
    /// membership and the account of whoever it is written for.
    CourseItem,
    DeletedContainers,
    Block,
    WorkspaceMembers,
    Users,
    /// A session, under its account.
    Session,
    /// An API token, under its account and its session.
    ApiToken,
}

/// Compile-time witness that [`crate::users::lock_instance`] holds the instance row in
/// this transaction scope.
#[derive(Debug, Clone, Copy)]
pub(crate) struct InstanceHeld<'c>(PhantomData<fn(&'c ()) -> &'c ()>);

impl InstanceHeld<'_> {
    pub(crate) const fn new() -> Self {
        Self(PhantomData)
    }
}

/// Compile-time witness that an organization row is held in this transaction scope.
#[derive(Debug, Clone, Copy)]
pub(crate) struct OrganizationHeld<'c>(PhantomData<fn(&'c ()) -> &'c ()>);

impl OrganizationHeld<'_> {
    pub(crate) const fn new() -> Self {
        Self(PhantomData)
    }
}

/// Compile-time witness that [`crate::trash::hold_deleted_containers`] holds a
/// workspace's trashed containers in this transaction scope.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DeletedContainersHeld<'c>(PhantomData<fn(&'c ()) -> &'c ()>);

impl DeletedContainersHeld<'_> {
    pub(crate) const fn new() -> Self {
        Self(PhantomData)
    }
}

/// The lock a pass of `rank` goes under, for the ranks a transaction takes again after
/// deeper locks: the rows of several organizations under the turn of several links, in
/// any order, since the turn lets one such transaction through at a time; and the
/// workspaces of an organization under its row, one pass a workspace in the order of
/// their keys, each with what it holds under it.
#[cfg(debug_assertions)]
const fn pass_under(rank: LockRank) -> Option<LockRank> {
    match rank {
        LockRank::Organization => Some(LockRank::LinkTurn),
        LockRank::Workspace => Some(LockRank::Organization),
        _ => None,
    }
}

/// A lock a transaction took, as the check of their order records it.
#[cfg(debug_assertions)]
#[derive(Debug)]
pub(super) struct TakenLock {
    pub(super) rank: LockRank,
    pub(super) key: String,
    pub(super) lock: RowLock,
}

#[cfg(debug_assertions)]
impl TakenLock {
    /// The rank, key and strength of the lock.
    #[cfg(test)]
    pub(super) fn parts(&self) -> (LockRank, &str, RowLock) {
        (self.rank, &self.key, self.lock)
    }
}

/// Records the lock of `key` at `rank`, asserting that it comes after what `locks` holds:
/// a deeper rank than the last lock, or the same rank and a greater key (any key for
/// [`LockRank::WorkspaceMembers`]). A new pass ([`pass_under`]) is checked against the
/// previous pass under the same lock, and nothing recorded is forgotten: what each pass
/// takes is checked within it, and a lock that is not a new pass against the last.
/// Nothing is a new pass once [`LockRank::Users`] is held. A lock held already is
/// taken again as [`strengthen`] says.
#[cfg(debug_assertions)]
pub(super) fn push_checked_lock(
    locks: &mut Vec<TakenLock>,
    rank: LockRank,
    key: &str,
    lock: RowLock,
) {
    if let Some(at) = locks
        .iter()
        .position(|taken| taken.rank == rank && taken.key == key)
    {
        strengthen(locks, at, lock);
        return;
    }
    let taken = TakenLock {
        rank,
        key: key.to_owned(),
        lock,
    };
    let holds_users = locks.iter().any(|taken| taken.rank >= LockRank::Users);
    if !holds_users
        && let Some(under) = pass_under(rank)
        && let Some(start) = locks.iter().rposition(|taken| taken.rank == under)
    {
        if rank == LockRank::Organization {
            locks.push(taken);
            return;
        }
        if let Some(previous) = locks[start + 1..]
            .iter()
            .rev()
            .find(|taken| taken.rank == rank)
        {
            debug_assert!(
                previous.key.as_str() < key,
                "out-of-order {rank:?} pass: after ({rank:?}, {:?}), acquiring ({rank:?}, {key:?}); all={locks:?}",
                previous.key,
            );
            locks.push(taken);
            return;
        }
    }
    if let Some(last) = locks.last() {
        debug_assert!(
            rank > last.rank
                || (rank == last.rank
                    && (rank == LockRank::WorkspaceMembers || last.key.as_str() < key)),
            "lock rank violation: holding ({:?}, {:?}), acquiring ({rank:?}, {key:?}); all={locks:?}",
            last.rank,
            last.key,
        );
    }
    locks.push(taken);
}

/// Takes the lock `locks[at]` again as `lock`. One no stronger than it waits for
/// nobody. A stronger one waits for whoever holds the row more weakly, as a new lock of
/// the row would: so nothing taken since may come after it in the order, a deeper rank
/// or a greater key of its own (any key for [`LockRank::WorkspaceMembers`]).
#[cfg(debug_assertions)]
fn strengthen(locks: &mut [TakenLock], at: usize, lock: RowLock) {
    let held = &locks[at];
    if lock.strength() <= held.lock.strength() {
        return;
    }
    let since = locks[at + 1..].iter().find(|later| {
        later.rank > held.rank
            || (later.rank == held.rank
                && held.rank != LockRank::WorkspaceMembers
                && later.key > held.key)
    });
    debug_assert!(
        since.is_none(),
        "lock strength upgrade violation: holding ({:?}, {:?}) as {:?} and since then {since:?}, acquiring it as {lock:?}; all={locks:?}",
        held.rank,
        held.key,
        held.lock,
    );
    locks[at].lock = lock;
}

#[cfg(test)]
mod clauses {
    use super::super::IntoSql as _;
    use super::{Dialect, LockClause, Locking, RowLock, SqlFragment, holds_locking_clause};

    fn text(fragment: &SqlFragment) -> String {
        fragment.into_sql_string()
    }

    #[test]
    fn each_lock_writes_its_clause() {
        for (lock, clause) in [
            (RowLock::Update, " FOR UPDATE"),
            (RowLock::NoKeyUpdate, " FOR NO KEY UPDATE"),
            (RowLock::Share, " FOR SHARE"),
            (RowLock::KeyShare, " FOR KEY SHARE"),
        ] {
            assert_eq!(text(&Locking::from(lock).clause()), clause);
        }
        assert_eq!(
            text(&RowLock::KeyShare.of("e").clause()),
            " FOR KEY SHARE OF e"
        );
    }

    #[test]
    fn a_lock_that_skips_what_others_hold_says_so_last() {
        assert_eq!(
            text(&Locking::from(RowLock::NoKeyUpdate).skipping_clause()),
            " FOR NO KEY UPDATE SKIP LOCKED"
        );
        assert_eq!(
            text(&RowLock::Update.of("files").skipping_clause()),
            " FOR UPDATE OF files SKIP LOCKED"
        );
    }

    #[test]
    fn sqlite_gets_no_clause() {
        let clause = |dialect| {
            let LockClause(clause) =
                Locking::from(RowLock::Update).in_dialect(dialect, Locking::clause);
            text(&clause)
        };
        assert_eq!(clause(Dialect::Sqlite), "");
        assert_eq!(clause(Dialect::Postgres), " FOR UPDATE");
    }

    #[test]
    #[should_panic(expected = "invalid SQL table alias")]
    fn a_table_that_is_no_name_is_refused() {
        let _ = RowLock::Share.of("u; DROP");
    }

    #[test]
    fn a_clause_is_seen_however_a_statement_writes_it() {
        for statement in [
            "SELECT id FROM users WHERE id = $1 FOR UPDATE",
            "SELECT 1 FROM t for  no key\n update",
            "SELECT 1 FROM t FOR /* held */ SHARE",
            "SELECT 1 FROM t FOR -- held\n KEY SHARE OF t",
        ] {
            assert!(holds_locking_clause(statement), "{statement:?}");
        }
        for lock in [
            RowLock::Update,
            RowLock::NoKeyUpdate,
            RowLock::Share,
            RowLock::KeyShare,
        ] {
            assert!(holds_locking_clause(&text(&Locking::from(lock).clause())));
        }
    }

    #[test]
    fn words_that_only_look_alike_are_no_clause() {
        for statement in [
            "UPDATE users SET name = $1",
            "SELECT 1 -- waiting for updates",
            "SELECT 'for sharing'",
            "SELECT 1 FROM t WHERE a - b > 0 /* for */ AND share = 1",
            "FORUPDATE",
            "FOR KEY",
        ] {
            assert!(!holds_locking_clause(statement), "{statement:?}");
        }
    }
}

#[cfg(all(test, debug_assertions))]
mod tests {
    use super::super::{Dialect, Sql};
    use super::LockRank::{
        ApiToken, Block, Container, CourseItem, DeletedContainers, Instance, Invitation, LinkTurn,
        Organization, PendingProvider, Provider, Session, Users, Workspace, WorkspaceMembers,
    };
    use super::{
        LockClause, LockRank, Locking, RowLock, TakenLock, check_clause, push_checked_lock,
    };

    /// A statement of `text` as the scope starts one.
    fn statement(text: &str) -> Sql {
        Sql {
            dialect: Dialect::Postgres,
            text: text.to_owned(),
            params: Vec::new(),
            invalid_id: None,
            changes_no_access: false,
            may_lock: false,
        }
    }

    /// A statement of raw text, as a test writes one (`Scope::sql_unchecked`).
    fn raw(text: &str) -> Sql {
        Sql {
            may_lock: true,
            ..statement(text)
        }
    }

    #[test]
    #[should_panic(expected = "a locking clause that no Scope::row_lock wrote")]
    fn a_clause_put_together_from_pieces_is_caught_as_it_runs() {
        let mut sql = statement("SELECT id FROM t");
        sql.push([" FOR ", "UPDATE"]);
        check_clause(&sql);
    }

    #[test]
    fn a_clause_pushed_as_a_lock_runs() {
        let mut sql = statement("SELECT id FROM t");
        sql.push_lock(LockClause(Locking::from(RowLock::Update).clause()));
        check_clause(&sql);
        check_clause(&raw("SELECT 1 FROM t FOR UPDATE"));
        check_clause(&statement("SELECT 1 FROM t"));
    }

    #[test]
    fn the_new_ranks_take_their_places() {
        // A trial's settings before the provider; a link and a course's exam under the
        // board, before the trash; an account before its session and its token.
        take(&[
            (Organization, "o"),
            (PendingProvider, "o"),
            (Provider, "p"),
            (Workspace, "w"),
            (Container, "c"),
            (Invitation, "i"),
            (CourseItem, "e"),
            (DeletedContainers, "w"),
            (Block, "b"),
            (WorkspaceMembers, "w:u"),
            (Users, "u"),
            (Session, "s"),
            (ApiToken, "t"),
        ]);
    }

    #[test]
    fn an_erasure_holds_the_providers_of_its_organizations_before_their_links() {
        take(&[
            (Instance, "1"),
            (LinkTurn, "links"),
            (Organization, "o1"),
            (Organization, "o2"),
            (Provider, "o1"),
            (Provider, "o2"),
            (Invitation, "o1"),
            (Invitation, "o2"),
        ]);
    }

    #[test]
    #[should_panic(expected = "lock rank violation")]
    fn a_provider_is_not_held_after_a_link() {
        take(&[(Provider, "o1"), (Invitation, "o1"), (Provider, "o2")]);
    }

    #[test]
    #[should_panic(expected = "lock rank violation")]
    fn a_link_is_not_held_after_the_trash() {
        take(&[
            (Container, "c"),
            (DeletedContainers, "w"),
            (Invitation, "i"),
        ]);
    }

    #[test]
    #[should_panic(expected = "lock rank violation")]
    fn a_session_is_not_held_before_its_account() {
        take(&[(Session, "s"), (Users, "u")]);
    }

    /// Records `sequence` in its order, as a transaction takes its locks, each as a
    /// change of its row holds it.
    fn take(sequence: &[(LockRank, &str)]) -> Vec<TakenLock> {
        let held: Vec<(LockRank, &str, RowLock)> = sequence
            .iter()
            .map(|(rank, key)| (*rank, *key, RowLock::NoKeyUpdate))
            .collect();
        take_as(&held)
    }

    /// Records `sequence` in its order, each lock as strong as it says.
    fn take_as(sequence: &[(LockRank, &str, RowLock)]) -> Vec<TakenLock> {
        let mut locks = Vec::new();
        for (rank, key, lock) in sequence {
            push_checked_lock(&mut locks, *rank, key, *lock);
        }
        locks
    }

    #[test]
    fn a_lock_is_made_stronger_while_nothing_after_it_is_held() {
        let locks = take_as(&[
            (Organization, "o", RowLock::Update),
            (Container, "a", RowLock::KeyShare),
            (Container, "a", RowLock::Share),
            (Container, "a", RowLock::NoKeyUpdate),
            (Container, "a", RowLock::Update),
            // Nothing deeper, nor a greater key, came since the first lock of the row.
            (Organization, "o", RowLock::Update),
            (Block, "b", RowLock::Update),
        ]);
        let taken: Vec<_> = locks.iter().map(TakenLock::parts).collect();
        assert_eq!(
            taken,
            [
                (Organization, "o", RowLock::Update),
                (Container, "a", RowLock::Update),
                (Block, "b", RowLock::Update),
            ]
        );
    }

    #[test]
    fn a_lock_no_stronger_is_taken_again_after_deeper_ones() {
        // As each deletion of a board holds again what its write held before.
        let locks = take_as(&[
            (Container, "a", RowLock::Update),
            (Container, "b", RowLock::Update),
            (Block, "x", RowLock::Update),
            (Container, "a", RowLock::Update),
            (Block, "x", RowLock::Update),
            (Container, "b", RowLock::NoKeyUpdate),
            (Container, "a", RowLock::KeyShare),
        ]);
        let taken: Vec<_> = locks.iter().map(TakenLock::parts).collect();
        assert_eq!(
            taken,
            [
                (Container, "a", RowLock::Update),
                (Container, "b", RowLock::Update),
                (Block, "x", RowLock::Update),
            ]
        );
    }

    #[test]
    #[should_panic(expected = "lock strength upgrade violation")]
    fn a_board_is_not_held_for_its_deletion_once_its_blocks_are() {
        take_as(&[
            (Container, "a", RowLock::NoKeyUpdate),
            (Block, "x", RowLock::Update),
            (Container, "a", RowLock::Update),
        ]);
    }

    #[test]
    #[should_panic(expected = "lock strength upgrade violation")]
    fn a_board_is_not_held_more_strongly_once_a_later_board_is() {
        take_as(&[
            (Container, "a", RowLock::NoKeyUpdate),
            (Container, "b", RowLock::NoKeyUpdate),
            (Container, "a", RowLock::Update),
        ]);
    }

    #[test]
    #[should_panic(expected = "lock strength upgrade violation")]
    fn a_shared_row_is_not_taken_for_a_change_after_deeper_ones() {
        take_as(&[
            (Workspace, "w", RowLock::KeyShare),
            (Container, "c", RowLock::NoKeyUpdate),
            (Workspace, "w", RowLock::Share),
        ]);
    }

    #[test]
    fn members_are_made_stronger_whatever_their_keys() {
        take_as(&[
            (WorkspaceMembers, "w:b", RowLock::Share),
            (WorkspaceMembers, "w:a", RowLock::Share),
            (WorkspaceMembers, "w:b", RowLock::Update),
        ]);
    }

    #[test]
    fn each_workspace_of_an_organization_is_a_pass_with_its_own_order() {
        // As an account leaves its workspaces: each one's boards, trash and members.
        let locks = take(&[
            (Organization, "o"),
            (Workspace, "w1"),
            (Container, "c5"),
            (Container, "c7"),
            (DeletedContainers, "w1"),
            (WorkspaceMembers, "c7"),
            (WorkspaceMembers, "c5"),
            (Workspace, "w2"),
            (Container, "c1"),
            (Block, "b1"),
            (Workspace, "w3"),
        ]);
        assert_eq!(locks.len(), 11, "nothing is forgotten: {locks:?}");
    }

    #[test]
    fn the_turn_takes_organizations_in_any_order() {
        take(&[
            (Instance, "1"),
            (LinkTurn, "links"),
            (Organization, "o2"),
            (Workspace, "w9"),
            (Container, "c9"),
            (Organization, "o1"),
            (Workspace, "w1"),
        ]);
    }

    #[test]
    #[should_panic(expected = "out-of-order Workspace pass")]
    fn workspaces_are_passed_in_the_order_of_their_keys() {
        take(&[
            (Organization, "o"),
            (Workspace, "w2"),
            (Container, "c1"),
            (Workspace, "w1"),
        ]);
    }

    #[test]
    #[should_panic(expected = "lock rank violation")]
    fn a_board_is_not_taken_before_the_first_workspace() {
        take(&[(Organization, "o"), (Container, "c1"), (Workspace, "w1")]);
    }

    #[test]
    #[should_panic(expected = "lock rank violation")]
    fn the_boards_of_a_pass_are_taken_in_order() {
        take(&[
            (Organization, "o"),
            (Workspace, "w1"),
            (Container, "c2"),
            (Container, "c1"),
        ]);
    }

    #[test]
    #[should_panic(expected = "lock rank violation")]
    fn the_boards_of_a_later_pass_are_taken_in_order() {
        take(&[
            (Organization, "o"),
            (Workspace, "w1"),
            (Container, "c9"),
            (Workspace, "w2"),
            (Container, "c2"),
            (Container, "c1"),
        ]);
    }

    #[test]
    #[should_panic(expected = "lock rank violation")]
    fn no_workspace_is_passed_without_its_organization() {
        take(&[(Workspace, "w1"), (Container, "c1"), (Workspace, "w2")]);
    }

    #[test]
    #[should_panic(expected = "lock rank violation")]
    fn no_pass_begins_once_an_account_is_held() {
        take(&[
            (Organization, "o"),
            (Workspace, "w1"),
            (Users, "u"),
            (Workspace, "w2"),
        ]);
    }
}
