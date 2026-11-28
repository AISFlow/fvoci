//! The execution layer: one SQL text and one set of parameters for both databases.
//!
//! Queries are written once with `$n` placeholders, which SQLite and PostgreSQL both
//! accept. [`Sql`] builds the text and collects the parameters, [`Scope`] runs it on one
//! connection, and [`Row`] reads the result. The few places where the two databases
//! differ go through [`Dialect`].

use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use fvoci_model::is_id;
use sqlx::AssertSqlSafe;
use sqlx::postgres::PgConnection;
use sqlx::sqlite::SqliteConnection;
use sqlx::types::Uuid;

use crate::Tenant;
use crate::clock::Clock;
use crate::error::StoreError;

mod locks;
pub(crate) use locks::{
    DeletedContainersHeld, InstanceHeld, LockRank, OrganizationHeld, RowLock, Unranked,
};

/// The SQL dialect of the connected database.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    /// SQLite: the database of a server of one organization.
    Sqlite,
    /// PostgreSQL.
    Postgres,
}

/// Compile-time-checked SQL text fragment built exclusively from `'static` SQL pieces
/// and validated table aliases. Prevents accidental `format!` runtime interpolation into
/// SQL statements (`H-6`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SqlFragment(String);

impl SqlFragment {
    pub(crate) fn new() -> Self {
        Self(String::new())
    }

    pub(crate) fn of(parts: &[&'static str]) -> Self {
        Self(parts.concat())
    }

    pub(crate) fn push(&mut self, part: &'static str) -> &mut Self {
        self.0.push_str(part);
        self
    }

    pub(crate) fn push_fragment(&mut self, part: &Self) -> &mut Self {
        self.0.push_str(&part.0);
        self
    }
}

mod sealed {
    pub(crate) trait Sealed {}
}

/// Sealed trait implemented only for `'static` SQL string slices/arrays and
/// [`SqlFragment`], rejecting dynamic `String` / `&str` formatting at compile time.
pub(crate) trait IntoSql: sealed::Sealed {
    fn into_sql_string(self) -> String;
    fn push_sql_into(self, out: &mut String);
}

impl sealed::Sealed for &'static str {}
impl IntoSql for &'static str {
    fn into_sql_string(self) -> String {
        self.to_owned()
    }

    fn push_sql_into(self, out: &mut String) {
        out.push_str(self);
    }
}

impl<const N: usize> sealed::Sealed for [&'static str; N] {}
impl<const N: usize> IntoSql for [&'static str; N] {
    fn into_sql_string(self) -> String {
        self.concat()
    }

    fn push_sql_into(self, out: &mut String) {
        for part in self {
            out.push_str(part);
        }
    }
}

impl<const N: usize> sealed::Sealed for &[&'static str; N] {}
impl<const N: usize> IntoSql for &[&'static str; N] {
    fn into_sql_string(self) -> String {
        self.concat()
    }

    fn push_sql_into(self, out: &mut String) {
        for part in self {
            out.push_str(part);
        }
    }
}

impl sealed::Sealed for &[&'static str] {}
impl IntoSql for &[&'static str] {
    fn into_sql_string(self) -> String {
        self.concat()
    }

    fn push_sql_into(self, out: &mut String) {
        for part in self {
            out.push_str(part);
        }
    }
}

impl sealed::Sealed for &SqlFragment {}
impl IntoSql for &SqlFragment {
    fn into_sql_string(self) -> String {
        self.0.clone()
    }

    fn push_sql_into(self, out: &mut String) {
        out.push_str(&self.0);
    }
}

/// A query parameter.
#[derive(Debug, Clone)]
pub(crate) enum Param {
    Text(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    /// JSON text bound to a `json` column (`TEXT` on SQLite).
    Json(String),
    /// JSON text bound to a `jsonb` column (`TEXT` on SQLite).
    Jsonb(String),
    /// An identifier, bound to a `uuid` column (`TEXT` on SQLite). `None` binds `NULL`.
    Id(Option<Uuid>),
    /// JSON text for a `jsonb` column (`TEXT` on SQLite) that may be `NULL`. `None`
    /// binds `NULL`.
    OptJsonb(Option<String>),
    /// An integer that may be `NULL`.
    OptInt(Option<i64>),
    /// Text that may be `NULL`.
    OptText(Option<String>),
    /// Bytes, bound to a `bytea` column (`BLOB` on SQLite).
    Bytes(Vec<u8>),
}

/// An identifier as a query parameter.
///
/// The value is whatever a caller was given: text that is not an identifier (see
/// [`fvoci_model::is_id`]) names nothing. A statement that reads then finds no row
/// under it, and one that writes is refused with [`StoreError::InvalidId`].
#[derive(Debug, Clone, Copy)]
pub(crate) struct Id<'a>(pub(crate) &'a str);

/// A reference that may be absent, as a query parameter: the empty string binds `NULL`,
/// anything else is bound as [`Id`] binds it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct OptId<'a>(pub(crate) &'a str);

/// A parameter of a statement, and whether it carried text that is no identifier where
/// one was needed.
#[derive(Debug, Clone)]
pub(crate) struct Bound {
    param: Param,
    invalid_id: Option<String>,
}

impl<T: Into<Param>> From<T> for Bound {
    fn from(value: T) -> Self {
        Self {
            param: value.into(),
            invalid_id: None,
        }
    }
}

impl From<Id<'_>> for Bound {
    fn from(Id(text): Id<'_>) -> Self {
        if is_id(text)
            && let Ok(uuid) = Uuid::try_parse(text)
        {
            return Self {
                param: Param::Id(Some(uuid)),
                invalid_id: None,
            };
        }
        // No row has the nil identifier: it is no identifier itself, so no write stores
        // it. A query bound to it matches nothing.
        Self {
            param: Param::Id(Some(Uuid::nil())),
            invalid_id: Some(text.to_owned()),
        }
    }
}

impl From<&Id<'_>> for Bound {
    fn from(id: &Id<'_>) -> Self {
        Self::from(*id)
    }
}

impl From<OptId<'_>> for Bound {
    fn from(OptId(text): OptId<'_>) -> Self {
        if text.is_empty() {
            Self {
                param: Param::Id(None),
                invalid_id: None,
            }
        } else {
            Self::from(Id(text))
        }
    }
}

impl From<&str> for Param {
    fn from(value: &str) -> Self {
        Self::Text(value.to_owned())
    }
}

impl From<&String> for Param {
    fn from(value: &String) -> Self {
        Self::Text(value.clone())
    }
}

impl From<String> for Param {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<i64> for Param {
    fn from(value: i64) -> Self {
        Self::Int(value)
    }
}

impl From<f64> for Param {
    fn from(value: f64) -> Self {
        Self::Float(value)
    }
}

impl From<bool> for Param {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl Param {
    /// A JSON value for a `json` column.
    pub(crate) fn json(value: &impl serde::Serialize) -> Result<Self, StoreError> {
        Ok(Self::Json(to_json_text(value)?))
    }

    /// A JSON value for a `jsonb` column that may be `NULL`: `None` binds `NULL`.
    pub(crate) fn opt_jsonb<T: serde::Serialize>(value: Option<&T>) -> Result<Self, StoreError> {
        Ok(Self::OptJsonb(value.map(to_json_text).transpose()?))
    }

    /// A JSON value for a `jsonb` column.
    pub(crate) fn jsonb(value: &impl serde::Serialize) -> Result<Self, StoreError> {
        Ok(Self::Jsonb(to_json_text(value)?))
    }
}

pub(crate) fn to_json_text(value: &impl serde::Serialize) -> Result<String, StoreError> {
    serde_json::to_string(value).map_err(|error| StoreError::Corrupt {
        column: "(parameter)",
        reason: error.to_string(),
    })
}

/// A SQL statement under construction: text with `$n` placeholders and their values.
#[derive(Debug, Clone)]
pub(crate) struct Sql {
    dialect: Dialect,
    text: String,
    params: Vec<Param>,
    /// The first bound value that had to be an identifier and was not.
    invalid_id: Option<String>,
    /// Set by [`Self::changing_no_access`].
    changes_no_access: bool,
    /// Whether it may hold a locking clause: one [`Self::push_lock`] ended it with, or
    /// raw text of a test ([`locks::check_clause`]).
    may_lock: bool,
}

impl Sql {
    /// Marks a statement that writes only what no check of access reads, such as the
    /// number the next card of a board gets: it leaves the revision of what is
    /// visible ([`crate::visibility`]), so the permissions read before it stay cached.
    /// A statement that writes anything else is not marked.
    pub(crate) fn changing_no_access(&mut self) -> &mut Self {
        self.changes_no_access = true;
        self
    }

    /// The statement that asks the database how it would answer this one, which is a
    /// query: its plan, a line a row. PostgreSQL runs the query to say what it read.
    #[cfg(any(test, feature = "test-util"))]
    pub(crate) fn explained(mut self) -> Self {
        let asked = match self.dialect {
            Dialect::Sqlite => "EXPLAIN QUERY PLAN ",
            Dialect::Postgres => "EXPLAIN (ANALYZE, BUFFERS) ",
        };
        self.text.insert_str(0, asked);
        self
    }

    /// Appends static SQL text. The text must not contain values; bind those instead.
    pub(crate) fn push(&mut self, text: impl IntoSql) -> &mut Self {
        text.push_sql_into(&mut self.text);
        self
    }

    /// Appends a placeholder and records its value.
    pub(crate) fn bind(&mut self, bound: impl Into<Bound>) -> &mut Self {
        let Bound { param, invalid_id } = bound.into();
        if self.invalid_id.is_none() {
            self.invalid_id = invalid_id;
        }
        let index = self.params.len() + 1;
        // PostgreSQL needs the type of a JSON parameter spelled out; SQLite stores JSON as
        // plain text and would mangle the value if asked to cast it.
        let cast = match (&param, self.dialect) {
            (Param::Json(_), Dialect::Postgres) => "::json",
            (Param::Jsonb(_) | Param::OptJsonb(_), Dialect::Postgres) => "::jsonb",
            _ => "",
        };
        // Writing to a String cannot fail.
        let _ = write!(self.text, "${index}{cast}");
        self.params.push(param);
        self
    }

    /// Appends `text` followed by a placeholder: `push("id = ").bind(id)` in one call.
    pub(crate) fn push_bind(&mut self, text: impl IntoSql, param: impl Into<Bound>) -> &mut Self {
        self.push(text).bind(param)
    }

    /// Limits the statement to the records of `paging`; to none for no page, which
    /// reads every record.
    pub(crate) fn push_page(&mut self, paging: impl Into<Option<crate::Paging>>) -> &mut Self {
        if let Some(paging) = paging.into() {
            self.push_bind(" LIMIT ", i64::from(paging.per_page.get()))
                .push_bind(" OFFSET ", paging.offset());
        }
        self
    }

    /// Appends a comma-separated list of placeholders, one per item.
    ///
    /// An empty list produces `NULL`, so that `IN (...)` matches nothing.
    pub(crate) fn bind_list<I>(&mut self, items: I) -> &mut Self
    where
        I: IntoIterator,
        I::Item: Into<Bound>,
    {
        let mut first = true;
        for item in items {
            if !first {
                self.text.push_str(", ");
            }
            first = false;
            self.bind(item);
        }
        if first {
            self.text.push_str("NULL");
        }
        self
    }
}

/// A borrowed connection to either database.
#[derive(Debug)]
pub(crate) enum ConnRef<'c> {
    Sqlite(&'c mut SqliteConnection),
    Postgres(&'c mut PgConnection),
}

/// One connection, or one transaction. Every query of an operation runs through the same
/// scope.
#[derive(Debug)]
pub(crate) struct Scope<'c> {
    conn: ConnRef<'c>,
    /// Whose rows the operation reaches. On PostgreSQL the transaction was told so when
    /// it began.
    tenant: Tenant,
    /// Whether `fvoci.system` is currently `'on'` in this PostgreSQL transaction.
    wall_open: bool,
    /// Monotonic clock shared with the owning [`crate::Store`].
    clock: Option<&'c Clock>,
    /// Whether a statement ran that may have changed something: anything but a plain
    /// `SELECT`. Set when the statement starts.
    wrote: bool,
    /// Told the same, as the statement starts, if the call is watched: so that a call
    /// given up while such a statement is under way is told as one that wrote
    /// ([`crate::observe::Watch`]).
    writes: Option<&'c AtomicBool>,
    visibility: Option<&'c crate::visibility::Writing<'c>>,
    /// How many versions are kept of a comment and of a card's body.
    versions_kept: VersionsKept,
    /// Whether the store notes changes for the search server
    /// ([`crate::Store::keep_search_outbox`]); `None` for a scope that notes none.
    search_outbox: Option<&'c AtomicBool>,
    /// Whether the writes of blocks of this scope note whom to tell
    /// ([`crate::Store::telling_nobody`], [`crate::Store::quietly`]).
    noting: Noting,
    /// The agent whose writes these are ([`crate::Store::by_agent`]).
    agent: Option<std::sync::Arc<crate::body_versions::AgentOfHandle>>,
    /// The locks taken in this scope, for debug lock-order assertions.
    #[cfg(debug_assertions)]
    locks: Vec<locks::TakenLock>,
}

/// Whom the writes of blocks of a handle, and of its scopes, note to be told.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Noting {
    /// Whom a write newly names: those its body mentions, those a card is given to.
    #[default]
    Named,
    /// Nobody, as its writer asked: what is written was said before (a copy, a change
    /// taken back or made again). Its writer watches what it writes new all the same.
    Quietly,
    /// Nobody, and nobody comes to watch what is written: for the fixtures of tests
    /// (`Store::telling_nobody`).
    #[cfg(feature = "test-util")]
    Nobody,
}

/// How many versions and how many body bytes the store keeps, as set when a scope begins.
#[derive(Debug, Clone, Copy)]
pub(crate) struct VersionsKept {
    /// Earlier versions of a comment ([`crate::Store::set_comment_versions_kept`]).
    pub(crate) of_comments: u32,
    /// Versions of a card's body ([`crate::Store::set_body_versions_kept`]).
    pub(crate) of_bodies: u32,
    /// Maximum total serialized bytes of kept versions of a card's body
    /// ([`crate::Store::set_body_versions_max_bytes`]).
    pub(crate) body_max_bytes: u64,
}

/// Binds every parameter of `$sql` to `$query` for one database. `$id` turns an
/// identifier into what that database's identifier columns hold.
macro_rules! bind_params {
    ($query:expr, $params:expr, $id:expr) => {{
        let mut query = $query;
        for param in $params {
            query = match param {
                Param::Text(value) | Param::Json(value) | Param::Jsonb(value) => query.bind(value),
                Param::Int(value) => query.bind(value),
                Param::Float(value) => query.bind(value),
                Param::Bool(value) => query.bind(value),
                Param::Id(value) => query.bind(value.map($id)),
                Param::OptJsonb(value) => query.bind(value),
                Param::OptInt(value) => query.bind(value),
                Param::OptText(value) => query.bind(value),
                Param::Bytes(value) => query.bind(value),
            };
        }
        query
    }};
}

/// SQLite keeps identifiers as their canonical text.
fn sqlite_id(uuid: Uuid) -> String {
    uuid.hyphenated().to_string()
}

/// PostgreSQL has a type for them.
fn postgres_id(uuid: Uuid) -> Uuid {
    uuid
}

impl<'c> Scope<'c> {
    pub(crate) fn new(conn: ConnRef<'c>, tenant: Tenant, versions_kept: VersionsKept) -> Self {
        Self {
            conn,
            tenant,
            wall_open: matches!(tenant, Tenant::System),
            clock: None,
            wrote: false,
            writes: None,
            visibility: None,
            versions_kept,
            search_outbox: None,
            noting: Noting::Named,
            agent: None,
            #[cfg(debug_assertions)]
            locks: Vec::new(),
        }
    }

    /// Records a lock acquisition at `(rank, key)`, as strong as `lock`, and asserts
    /// global lock ordering in debug and test builds.
    pub(crate) fn record_lock(&mut self, rank: LockRank, key: &str, lock: RowLock) {
        #[cfg(debug_assertions)]
        locks::push_checked_lock(&mut self.locks, rank, key, lock);
        #[cfg(not(debug_assertions))]
        let _ = (rank, key, lock);
    }

    /// Attaches the owning store's monotonic [`Clock`] to this scope.
    pub(crate) fn with_clock(self, clock: &'c Clock) -> Self {
        Self {
            clock: Some(clock),
            ..self
        }
    }

    /// Current monotonic time in milliseconds since the Unix epoch.
    pub(crate) fn now_millis(&self) -> i64 {
        self.clock
            .map_or_else(crate::clock::system_millis, Clock::now_millis)
    }

    pub(crate) fn is_wall_open(&self) -> bool {
        self.wall_open
    }

    pub(crate) fn open_wall(&mut self) {
        self.wall_open = true;
    }

    pub(crate) fn close_wall(&mut self) {
        self.wall_open = false;
    }

    /// The same scope, whose writes of blocks note whom to tell as `noting` says.
    pub(crate) fn noting(self, noting: Noting) -> Self {
        Self { noting, ..self }
    }

    /// The same scope, whose writes are those of `agent`, if any.
    pub(crate) fn acting(
        self,
        agent: Option<std::sync::Arc<crate::body_versions::AgentOfHandle>>,
    ) -> Self {
        Self { agent, ..self }
    }

    /// The agent whose writes these are; `None` for a person's.
    pub(crate) fn agent(&self) -> Option<crate::ByAgent<'_>> {
        self.agent
            .as_deref()
            .map(crate::body_versions::AgentOfHandle::by)
    }

    /// Whom the writes of blocks of this scope note to be told.
    pub(crate) fn notes(&self) -> Noting {
        self.noting
    }

    /// The same scope, which notes changes for the search server while `kept` says so.
    pub(crate) fn noting_for_search(self, kept: &'c AtomicBool) -> Self {
        Self {
            search_outbox: Some(kept),
            ..self
        }
    }

    /// Whether the changes of this scope are noted for the search server.
    pub(crate) fn keeps_search_outbox(&self) -> bool {
        self.search_outbox
            .is_some_and(|kept| kept.load(Ordering::Relaxed))
    }

    /// The same scope, which also sets `writes` when a statement that may write starts.
    pub(crate) fn reporting_writes_to(self, writes: Option<&'c AtomicBool>) -> Self {
        Self { writes, ..self }
    }

    pub(crate) fn tracking_visibility(
        self,
        visibility: &'c crate::visibility::Writing<'c>,
    ) -> Self {
        Self {
            visibility: Some(visibility),
            ..self
        }
    }

    /// Notes that a statement that may write starts.
    fn starts_writing(&mut self, sql: &Sql) {
        if let Some(visibility) = self.visibility
            && !sql.changes_no_access
        {
            visibility.statement(&sql.text);
        }
        self.wrote = true;
        if let Some(writes) = self.writes {
            writes.store(true, Ordering::Relaxed);
        }
    }

    /// The operation changes what is read of the organization `org_id` without
    /// writing a table of it, whatever organization the scope reaches: that one's
    /// cached reads are thrown away, and no other's.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::InvalidId`] if `org_id` is no identifier.
    pub(crate) fn invalidate_visibility_of(&self, org_id: &str) -> Result<(), StoreError> {
        let Tenant::Org(org) = Tenant::org(org_id)? else {
            return Err(StoreError::InvalidId(org_id.to_owned()));
        };
        if let Some(visibility) = self.visibility {
            visibility.invalidate_of(org);
        }
        Ok(())
    }

    /// Whether the operation ran a statement that may have changed something. One that
    /// only read has nothing to commit.
    pub(crate) fn wrote(&self) -> bool {
        self.wrote
    }

    /// The organization whose rows the operation reaches, as a query parameter: for
    /// the statements that name the organization themselves.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the store's handle names none.
    pub(crate) fn org(&self) -> Result<Param, StoreError> {
        if let Tenant::Org(org_id) = self.tenant {
            Ok(Param::Id(Some(org_id)))
        } else {
            Err(StoreError::NoOrganization)
        }
    }

    /// The identifier of the organization the operation reaches, if it names one.
    pub(crate) fn org_id(&self) -> Option<String> {
        self.tenant.org_id()
    }

    /// Whether the store's handle names an organization.
    pub(crate) fn has_org(&self) -> bool {
        matches!(self.tenant, Tenant::Org(_))
    }

    /// Runs `operation` as the organization `org_id`, which this transaction makes, in
    /// place of the one the scope reaches: the scope names it, and on PostgreSQL the
    /// transaction's wall is moved to it and back, so that the rows `operation` stores
    /// are the new organization's, and nothing of any other is in its reach meanwhile
    /// unless the scope has the system's reach, which stays what it was: it is neither
    /// asked for nor given up here. Only inside a transaction, whose setting the wall
    /// is. Nothing here knows that the organization is new: the one caller makes its
    /// identifier (`accounts::make_own_space`), and a test of the sources holds it to
    /// be the only one.
    ///
    /// For an organization nobody read anything of yet: its writes move the revision
    /// of the scope's own organization, not the new one's, which has no cached read to
    /// throw away ([`crate::visibility`]).
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::InvalidId`] if `org_id` is no identifier, the error of
    /// `operation`, and [`StoreError::Database`] if the wall cannot be moved. The wall
    /// is moved back whatever `operation` answered; where that fails after it failed,
    /// its error is the one returned, and the transaction is one PostgreSQL takes
    /// nothing more in.
    pub(crate) async fn in_new_organization<T>(
        &mut self,
        org_id: &str,
        operation: impl AsyncFnOnce(&mut Scope<'_>) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let within = Tenant::org(org_id)?;
        let outer = self.tenant;
        self.move_wall_to(within).await?;
        self.tenant = within;
        let result = operation(self).await;
        self.tenant = outer;
        let back = self.move_wall_to(outer).await;
        match (result, back) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(error), _) | (Ok(_), Err(error)) => Err(error),
        }
    }

    /// Tells the policies of a PostgreSQL transaction which organization's rows it
    /// reaches from here on: `tenant`'s, or no single one's. SQLite has no wall.
    async fn move_wall_to(&mut self, tenant: Tenant) -> Result<(), StoreError> {
        if self.dialect() != Dialect::Postgres {
            return Ok(());
        }
        let mut sql = self.sql(["SELECT set_config('", crate::ORG_SETTING, "', "]);
        sql.bind(tenant.org_id().unwrap_or_default())
            .push(", true)");
        self.fetch_optional(sql).await?;
        Ok(())
    }

    /// The conditions that hold each row of `pairs` to the organization of the row
    /// paired with it: `" AND a.org_id = b.org_id"` for each pair `(a, b)` of table
    /// names or aliases, where rows carry their organization (PostgreSQL), and nothing
    /// on SQLite. For the statements that reach across every organization and match rows
    /// by an identifier that the keys of the schema do not hold to one organization, or
    /// that do not take the keys on trust.
    pub(crate) fn same_organization(&self, pairs: &[(&'static str, &'static str)]) -> SqlFragment {
        for &(row, parent) in pairs {
            assert!(
                is_sql_ident(row) && is_sql_ident(parent),
                "invalid SQL table alias in same_organization: ({row:?}, {parent:?})"
            );
        }
        if self.dialect() == Dialect::Sqlite {
            return SqlFragment::new();
        }
        let mut out = SqlFragment::new();
        for &(row, parent) in pairs {
            out.push(" AND ")
                .push(row)
                .push(".org_id = ")
                .push(parent)
                .push(".org_id");
        }
        out
    }

    /// How many earlier versions of a comment are kept, at least one.
    pub(crate) fn comment_versions_kept(&self) -> usize {
        usize::try_from(self.versions_kept.of_comments).unwrap_or(usize::MAX)
    }

    /// How many versions of a card's body are kept, at least one.
    pub(crate) fn body_versions_kept(&self) -> i64 {
        i64::from(self.versions_kept.of_bodies)
    }

    /// Maximum total serialized bytes of kept versions of a card's body, at least one.
    pub(crate) fn body_versions_max_bytes(&self) -> u64 {
        self.versions_kept.body_max_bytes
    }

    /// The dialect of the connected database.
    pub(crate) fn dialect(&self) -> Dialect {
        match self.conn {
            ConnRef::Sqlite(_) => Dialect::Sqlite,
            ConnRef::Postgres(_) => Dialect::Postgres,
        }
    }

    /// Starts a statement with static SQL text.
    pub(crate) fn sql(&self, text: impl IntoSql) -> Sql {
        Sql {
            dialect: self.dialect(),
            text: text.into_sql_string(),
            params: Vec::new(),
            invalid_id: None,
            changes_no_access: false,
            may_lock: false,
        }
    }

    /// Starts a statement from raw SQL text in test code only.
    #[cfg(any(test, feature = "test-util"))]
    pub(crate) fn sql_unchecked(&self, text: &str) -> Sql {
        Sql {
            dialect: self.dialect(),
            text: text.to_owned(),
            params: Vec::new(),
            invalid_id: None,
            changes_no_access: false,
            may_lock: true,
        }
    }

    /// Runs a query and returns all its rows.
    ///
    /// A value bound as an identifier that is not one matches no row.
    pub(crate) async fn fetch_all(&mut self, sql: Sql) -> Result<Vec<Row>, StoreError> {
        locks::check_clause(&sql);
        if !is_plain_select(&sql.text) {
            self.starts_writing(&sql);
        }
        // The text is built from literals in this crate; every value is a bound
        // parameter.
        let text = AssertSqlSafe(sql.text);
        match &mut self.conn {
            ConnRef::Sqlite(conn) => {
                let rows = bind_params!(sqlx::query(text), sql.params, sqlite_id)
                    .fetch_all(&mut **conn)
                    .await?;
                Ok(rows.into_iter().map(Row::Sqlite).collect())
            }
            ConnRef::Postgres(conn) => {
                let rows = bind_params!(sqlx::query(text), sql.params, postgres_id)
                    .fetch_all(&mut **conn)
                    .await?;
                Ok(rows.into_iter().map(Row::Postgres).collect())
            }
        }
    }

    /// Runs a plain `SELECT` and returns all its rows, unless it runs past `by`: then
    /// the database is told to stop it, and `None` is returned. On PostgreSQL the
    /// statement runs under a `statement_timeout` of what is left of the time, in the
    /// scope's transaction; the transaction cannot be used after a timeout. On SQLite a
    /// progress handler stops it, and is taken off the connection again before it goes
    /// back to the pool (or left harmless, should the call be given up).
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails otherwise.
    pub(crate) async fn fetch_all_by(
        &mut self,
        sql: Sql,
        by: Instant,
    ) -> Result<Option<Vec<Row>>, StoreError> {
        debug_assert!(is_plain_select(&sql.text), "{}", sql.text);
        let Some(left) = by
            .checked_duration_since(Instant::now())
            .filter(|left| !left.is_zero())
        else {
            return Ok(None);
        };
        match self.dialect() {
            Dialect::Postgres => {
                // 0 would be no limit: a millisecond at least. Bound, so that the
                // statement is one, whatever time is left.
                let millis = left.as_millis().max(1);
                let mut setting = self.sql("SELECT set_config('statement_timeout', ");
                setting.bind(format!("{millis}ms")).push(", true)");
                self.fetch_all(setting).await?;
                match self.fetch_all(sql).await {
                    Err(StoreError::Database(error)) if is_query_canceled(&error) => Ok(None),
                    rows => rows.map(Some),
                }
            }
            Dialect::Sqlite => {
                let stop = SqliteStop::set(self.sqlite(), by).await?;
                let rows = self.fetch_all(sql).await;
                let stopped = stop.take_off(self.sqlite()).await?;
                match rows {
                    Err(StoreError::Database(_)) if stopped => Ok(None),
                    rows => rows.map(Some),
                }
            }
        }
    }

    /// The connection of a scope of SQLite.
    fn sqlite(&mut self) -> &mut SqliteConnection {
        let ConnRef::Sqlite(conn) = &mut self.conn else {
            unreachable!("a SQLite scope holds a SQLite connection");
        };
        conn
    }

    /// Runs a query and returns its first row, if any.
    pub(crate) async fn fetch_optional(&mut self, sql: Sql) -> Result<Option<Row>, StoreError> {
        Ok(self.fetch_all(sql).await?.into_iter().next())
    }

    /// Runs a statement that writes and returns the number of rows it affected.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::InvalidId`], without running the statement, if a value
    /// bound as an identifier is not one.
    pub(crate) async fn execute(&mut self, sql: Sql) -> Result<u64, StoreError> {
        if let Some(invalid) = sql.invalid_id {
            return Err(StoreError::InvalidId(invalid));
        }
        locks::check_clause(&sql);
        self.starts_writing(&sql);
        let text = AssertSqlSafe(sql.text);
        match &mut self.conn {
            ConnRef::Sqlite(conn) => Ok(bind_params!(sqlx::query(text), sql.params, sqlite_id)
                .execute(&mut **conn)
                .await?
                .rows_affected()),
            ConnRef::Postgres(conn) => Ok(bind_params!(sqlx::query(text), sql.params, postgres_id)
                .execute(&mut **conn)
                .await?
                .rows_affected()),
        }
    }
}

/// Whether PostgreSQL stopped the statement: its `statement_timeout` ran out
/// (`query_canceled`).
fn is_query_canceled(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .and_then(sqlx::error::DatabaseError::code)
        .is_some_and(|code| code == "57014")
}

/// How many virtual machine instructions of SQLite run between two looks at the clock.
const SQLITE_STEPS_A_LOOK: i32 = 1000;

/// The progress handler that stops a statement of SQLite at a time
/// ([`Scope::fetch_all_by`]). Dropped without [`SqliteStop::take_off`] (the call was
/// given up), it disarms the handler it left on the connection: that handler then lets
/// every statement run, until another takes its place.
struct SqliteStop {
    armed: Arc<AtomicBool>,
    stopped: Arc<AtomicBool>,
}

impl SqliteStop {
    async fn set(conn: &mut SqliteConnection, by: Instant) -> Result<Self, StoreError> {
        let armed = Arc::new(AtomicBool::new(true));
        let stopped = Arc::new(AtomicBool::new(false));
        let handler = {
            let armed = Arc::clone(&armed);
            let stopped = Arc::clone(&stopped);
            move || {
                if !armed.load(Ordering::Relaxed) || Instant::now() < by {
                    return true;
                }
                stopped.store(true, Ordering::Relaxed);
                false
            }
        };
        conn.lock_handle()
            .await?
            .set_progress_handler(SQLITE_STEPS_A_LOOK, handler);
        Ok(Self { armed, stopped })
    }

    /// Takes the handler off the connection. Returns whether it stopped the statement.
    async fn take_off(self, conn: &mut SqliteConnection) -> Result<bool, StoreError> {
        conn.lock_handle().await?.remove_progress_handler();
        Ok(self.stopped.load(Ordering::Relaxed))
    }
}

impl Drop for SqliteStop {
    fn drop(&mut self) {
        self.armed.store(false, Ordering::Relaxed);
    }
}

/// Whether a statement is a `SELECT` and nothing else, as this crate writes them: one
/// that changes nothing. A statement that begins otherwise (`WITH ... DELETE`, an
/// `UPDATE ... RETURNING`) counts as one that may write, whatever it does.
fn is_plain_select(text: &str) -> bool {
    text.trim_start().starts_with("SELECT ")
}

fn is_sql_ident(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

mod json;
mod reads;
mod row;
pub(crate) use row::{FromRow, Row};

#[cfg(test)]
mod tests;
