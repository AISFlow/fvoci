//! The working off of the list of whom a change may be told to (`notification_outbox`,
//! noted by `notification_noting.rs`), by the server that holds the lock: a row is
//! taken off, and a person told of it ([`Store::tell_of`]) or not
//! ([`Store::drop_notification_event`]), in one transaction, so that nobody is told
//! twice.
//!
//! What a person is told of one card or page, of one kind, within an hour of the first
//! of it, is one line: it counts how many times it happened and who did it, and names
//! the latest of it.

use fvoci_model::{NotificationKind, new_id};

use crate::Store;
use crate::db::{Dialect, Id, OptId, Param, Row, Scope, Sql};
use crate::error::StoreError;
use crate::notifications::kind_from_row;

/// The most people a line names as having done what it tells of: past them it counts
/// the times, and names no one more.
const MOST_DOERS: usize = 100;

/// A row of the list, as the worker reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationEvent {
    /// The organization it was noted in.
    pub org_id: String,
    /// Who may be told.
    pub recipient_id: String,
    /// Of what.
    pub kind: NotificationKind,
    /// Where it happened: the card or the page itself, or a comment on it.
    pub source_id: String,
    /// The card or the page it is about.
    pub block_id: String,
    /// The board or the wiki of that.
    pub container_id: String,
    /// Who did it; empty where nobody is known to have.
    pub actor_id: String,
    /// The name of the token an agent of `actor_id` did it through, if one did: then
    /// `actor_id` may be the recipient, whose token tells them of what it does.
    pub actor_token_name: Option<String>,
    /// What the row's `seq` was when it was read.
    pub seq: i64,
    /// How many times in a row it could not be worked off.
    pub attempts: i64,
}

/// The bounds a notification is made within.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotificationLimits {
    /// For how long after the first of a line what happens to the same card or page,
    /// of the same kind, for the same person, joins it, in milliseconds.
    pub same_within_millis: i64,
    /// The most lines one person makes for another within
    /// [`NotificationLimits::made_within_millis`]: past them, what they do makes no
    /// line, and joins only one that is there.
    pub most_made_by_one: i64,
    /// See [`NotificationLimits::most_made_by_one`].
    pub made_within_millis: i64,
    /// The most notifications a person has. Past it the oldest go, those that were
    /// read before those that were not.
    pub most_of_a_person: i64,
}

/// What came of telling a person of a row of the list ([`Store::tell_of`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use]
pub enum Told {
    /// A new line, by this identifier.
    Made(String),
    /// The line of the same card or page, of the same kind, of this identifier, which
    /// counts it now and is unread again.
    Joined(String),
    /// Nothing more: the line of this identifier counted the same doer's doing in the
    /// same place already (a mention taken out and put back), and stays as it was.
    Counted(String),
    /// Nothing: whoever did it made as many lines for the person as one may
    /// ([`NotificationLimits::most_made_by_one`]).
    OverTheLimit,
    /// Nothing: the row was noted again since it was read (another `seq`), and stays
    /// for the next round.
    NotedAgain,
}

const EVENT_COLUMNS: &str = "org_id, recipient_id, kind, source_id, block_id, container_id, actor_id, seq, attempts, \
     actor_token_name";

impl Store {
    /// Up to `limit` rows of the list that are due at `now`, those due the longest
    /// first, among the rows this handle reaches.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn due_notification_events(
        &self,
        now: i64,
        limit: i64,
    ) -> Result<Vec<NotificationEvent>, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = scope.sql([
                "SELECT ",
                EVENT_COLUMNS,
                " FROM notification_outbox WHERE due_at <= ",
            ]);
            sql.bind(now).push_bind(
                " ORDER BY due_at, org_id, recipient_id, kind, source_id LIMIT ",
                limit,
            );
            scope
                .fetch_all(sql)
                .await?
                .iter()
                .map(event_from_row)
                .collect()
        })
        .await
    }

    /// How many rows the list has, among the rows this handle reaches.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn waiting_notification_events(&self) -> Result<i64, StoreError> {
        self.with_conn(async |scope| {
            let sql = scope.sql("SELECT COUNT(*) FROM notification_outbox");
            match scope.fetch_optional(sql).await? {
                Some(row) => row.int(0),
                None => Ok(0),
            }
        })
        .await
    }

    /// Tells the person of `event` at `now`: takes its row off the list and, in the
    /// same transaction, joins it to the line the person has of the same card or page
    /// and kind, made within [`NotificationLimits::same_within_millis`] (unless that
    /// line counted its doer's doing of its source already), or makes a new line,
    /// unless its doer made [`NotificationLimits::most_made_by_one`] for the person
    /// already. Where the person then has more than
    /// [`NotificationLimits::most_of_a_person`], the oldest go, those that were read
    /// first. For a handle that reaches every organization: a person's notifications
    /// are counted across them.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a statement fails; nothing is taken off or
    /// made then.
    pub async fn tell_of(
        &self,
        event: &NotificationEvent,
        now: i64,
        limits: NotificationLimits,
    ) -> Result<Told, StoreError> {
        self.transaction(async |scope| {
            if !take_off(scope, event).await? {
                return Ok(Told::NotedAgain);
            }
            let mut sql = scope
                .sql("SELECT org_id, id, doers, counted FROM notifications WHERE recipient_id = ");
            sql.bind(Id(&event.recipient_id))
                .push_bind(" AND block_id = ", Id(&event.block_id))
                .push_bind(" AND kind = ", event.kind.as_str())
                .push_bind(
                    " AND created_at > ",
                    now.saturating_sub(limits.same_within_millis),
                )
                .push(" ORDER BY created_at DESC, id DESC LIMIT 1");
            if let Some(row) = scope.fetch_optional(sql).await? {
                let (org_id, id) = (row.id(0)?, row.id(1)?);
                let Some(counted) = with_counted(&row.text(3)?, &event.source_id, &event.actor_id)
                else {
                    return Ok(Told::Counted(id));
                };
                let doers = with_doer(&row.text(2)?, &event.actor_id);
                let mut sql = match scope.dialect() {
                    Dialect::Postgres => {
                        scope.sql("UPDATE notifications SET updated_at = GREATEST(")
                    }
                    Dialect::Sqlite => scope.sql("UPDATE notifications SET updated_at = MAX("),
                };
                sql.bind(now)
                    .push(", updated_at + 1), times = times + 1, read_at = 0, actor_id = ")
                    .bind(OptId(&event.actor_id))
                    .push_bind(
                        ", actor_token_name = ",
                        Param::OptText(event.actor_token_name.clone()),
                    )
                    .push_bind(", source_id = ", Id(&event.source_id))
                    .push_bind(", container_id = ", Id(&event.container_id))
                    .push_bind(", doers = ", doers)
                    .push_bind(", counted = ", counted)
                    .push_bind(" WHERE org_id = ", Id(&org_id))
                    .push_bind(" AND id = ", Id(&id));
                scope.execute(sql).await?;
                return Ok(Told::Joined(id));
            }
            if made_by_one(scope, event, now, limits).await? >= limits.most_made_by_one {
                return Ok(Told::OverTheLimit);
            }
            let id = new_id();
            let mut sql = scope.sql(
                "INSERT INTO notifications (recipient_id, id, org_id, kind, source_id, \
                 block_id, container_id, actor_id, made_by, created_at, updated_at, times, \
                 doers, counted, read_at, actor_token_name) VALUES (",
            );
            sql.bind(Id(&event.recipient_id))
                .push_bind(", ", Id(&id))
                .push_bind(", ", Id(&event.org_id))
                .push_bind(", ", event.kind.as_str())
                .push_bind(", ", Id(&event.source_id))
                .push_bind(", ", Id(&event.block_id))
                .push_bind(", ", Id(&event.container_id))
                .push_bind(", ", OptId(&event.actor_id))
                .push_bind(", ", OptId(&event.actor_id))
                .push_bind(", ", now)
                .push_bind(", ", now)
                .push_bind(", 1, ", with_doer("[]", &event.actor_id))
                .push_bind(", ", first_counted(&event.source_id, &event.actor_id))
                .push_bind(", 0, ", Param::OptText(event.actor_token_name.clone()))
                .push(")");
            scope.execute(sql).await?;
            keep_newest(scope, &event.recipient_id, limits.most_of_a_person).await?;
            Ok(Told::Made(id))
        })
        .await
    }

    /// Takes the row of `event` off the list, telling nobody. Returns whether it was
    /// taken off: a row noted again since it was read (another `seq`) stays.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the statement fails.
    pub async fn drop_notification_event(
        &self,
        event: &NotificationEvent,
    ) -> Result<bool, StoreError> {
        self.with_write_conn(async |scope| take_off(scope, event).await)
            .await
    }

    /// Notes that a row could not be worked off, and when to try again. A row noted
    /// again since is due already, and left as it is.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the statement fails.
    pub async fn notification_event_failed(
        &self,
        event: &NotificationEvent,
        retry_at: i64,
    ) -> Result<(), StoreError> {
        self.with_write_conn(async |scope| {
            let mut sql =
                scope.sql("UPDATE notification_outbox SET attempts = attempts + 1, due_at = ");
            sql.bind(retry_at);
            push_event(&mut sql, event);
            scope.execute(sql).await?;
            Ok(())
        })
        .await
    }

    /// Forgets the notifications that were read before `read_before`, among the rows
    /// this handle reaches. Returns how many went.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the statement fails.
    pub async fn forget_read_notifications(&self, read_before: i64) -> Result<u64, StoreError> {
        self.with_write_conn(async |scope| {
            let mut sql = scope.sql("DELETE FROM notifications WHERE read_at > 0 AND read_at < ");
            sql.bind(read_before);
            scope.execute(sql).await
        })
        .await
    }
}

/// How many lines the doer of `event` made for its recipient within
/// [`NotificationLimits::made_within_millis`]; none for what nobody is known to have
/// done.
async fn made_by_one(
    scope: &mut Scope<'_>,
    event: &NotificationEvent,
    now: i64,
    limits: NotificationLimits,
) -> Result<i64, StoreError> {
    if event.actor_id.is_empty() {
        return Ok(0);
    }
    let mut sql = scope.sql("SELECT COUNT(*) FROM notifications WHERE recipient_id = ");
    sql.bind(Id(&event.recipient_id))
        .push_bind(" AND made_by = ", Id(&event.actor_id))
        .push_bind(
            " AND created_at > ",
            now.saturating_sub(limits.made_within_millis),
        );
    match scope.fetch_optional(sql).await? {
        Some(row) => row.int(0),
        None => Ok(0),
    }
}

/// The doers of a line as it stores them (`doers`, a JSON list), with `doer` after
/// them where it is not among them yet and they are fewer than [`MOST_DOERS`]. A list
/// that does not read is taken for none: it counts people, and holds nothing else.
fn with_doer(doers: &str, doer: &str) -> String {
    let mut doers: Vec<String> = serde_json::from_str(doers).unwrap_or_default();
    if !doer.is_empty() && doers.len() < MOST_DOERS && !doers.iter().any(|known| known == doer) {
        doers.push(doer.to_owned());
    }
    serde_json::to_string(&doers).expect("string lists serialize to JSON")
}

/// What a line counted (`counted`, a JSON list of [source, doer] pairs) with the
/// doing of `doer` in `source` after it, or `None` where it is among them already: the
/// same person doing the same in the same place again. A doing of nobody known is
/// counted without being kept, and so is any past [`MOST_DOERS`] pairs. A list that
/// does not read is taken for none.
fn with_counted(counted: &str, source: &str, doer: &str) -> Option<String> {
    let mut counted: Vec<(String, String)> = serde_json::from_str(counted).unwrap_or_default();
    if counted
        .iter()
        .any(|(was_in, by)| was_in == source && by == doer)
    {
        return None;
    }
    if !doer.is_empty() && counted.len() < MOST_DOERS {
        counted.push((source.to_owned(), doer.to_owned()));
    }
    Some(serde_json::to_string(&counted).expect("string lists serialize to JSON"))
}

/// What a new line counted: the doing of `doer` in `source`, where somebody known did it.
fn first_counted(source: &str, doer: &str) -> String {
    let counted: &[(&str, &str)] = if doer.is_empty() {
        &[]
    } else {
        &[(source, doer)]
    };
    serde_json::to_string(counted).expect("string lists serialize to JSON")
}

/// Takes the row of `event` off the list, if it is as it was read.
async fn take_off(scope: &mut Scope<'_>, event: &NotificationEvent) -> Result<bool, StoreError> {
    let mut sql = scope.sql("DELETE FROM notification_outbox");
    push_event(&mut sql, event);
    Ok(scope.execute(sql).await? > 0)
}

/// Narrows a statement to the row `event` was read from, as it was read.
fn push_event(sql: &mut Sql, event: &NotificationEvent) {
    sql.push_bind(" WHERE org_id = ", Id(&event.org_id))
        .push_bind(" AND recipient_id = ", Id(&event.recipient_id))
        .push_bind(" AND kind = ", event.kind.as_str())
        .push_bind(" AND source_id = ", Id(&event.source_id))
        .push_bind(" AND seq = ", event.seq);
}

/// Leaves a person the `most` newest of their notifications: of the others the oldest
/// go, those that were read before those that were not.
pub(crate) async fn keep_newest(
    scope: &mut Scope<'_>,
    user_id: &str,
    most: i64,
) -> Result<(), StoreError> {
    let mut sql = scope.sql("SELECT COUNT(*) FROM notifications WHERE recipient_id = ");
    sql.bind(Id(user_id));
    let held = match scope.fetch_optional(sql).await? {
        Some(row) => row.int(0)?,
        None => 0,
    };
    let over = held.saturating_sub(most);
    if over <= 0 {
        return Ok(());
    }
    let mut sql = scope.sql("DELETE FROM notifications WHERE recipient_id = ");
    sql.bind(Id(user_id))
        .push_bind(
            " AND id IN (SELECT id FROM notifications WHERE recipient_id = ",
            Id(user_id),
        )
        .push_bind(
            " ORDER BY CASE WHEN read_at = 0 THEN 1 ELSE 0 END, updated_at, id LIMIT ",
            over,
        )
        .push(")");
    scope.execute(sql).await?;
    Ok(())
}

fn event_from_row(row: &Row) -> Result<NotificationEvent, StoreError> {
    Ok(NotificationEvent {
        org_id: row.id(0)?,
        recipient_id: row.id(1)?,
        kind: kind_from_row(row, 2)?,
        source_id: row.id(3)?,
        block_id: row.id(4)?,
        container_id: row.id(5)?,
        actor_id: row.id(6)?,
        seq: row.int(7)?,
        attempts: row.int(8)?,
        actor_token_name: row.opt_text(9)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_names_each_doer_once_in_the_order_they_came_and_a_hundred_at_most() {
        assert_eq!(with_doer("[]", "a"), r#"["a"]"#);
        assert_eq!(with_doer(r#"["a"]"#, "b"), r#"["a","b"]"#);
        assert_eq!(with_doer(r#"["a","b"]"#, "a"), r#"["a","b"]"#);
        assert_eq!(with_doer(r#"["a"]"#, ""), r#"["a"]"#, "nobody known");
        assert_eq!(with_doer("not a list", "a"), r#"["a"]"#);
        let full: Vec<String> = (0..MOST_DOERS).map(|n| n.to_string()).collect();
        let full = serde_json::to_string(&full).expect("JSON");
        assert_eq!(with_doer(&full, "x"), full);
    }

    #[test]
    fn a_line_counts_a_doing_once_and_keeps_a_hundred_at_most() {
        let one = first_counted("s", "a");
        assert_eq!(one, r#"[["s","a"]]"#);
        assert_eq!(first_counted("s", ""), "[]", "nobody known");
        assert_eq!(with_counted(&one, "s", "a"), None, "the same again");
        assert_eq!(
            with_counted(&one, "s", "b").as_deref(),
            Some(r#"[["s","a"],["s","b"]]"#),
            "somebody else"
        );
        assert_eq!(
            with_counted(&one, "t", "a").as_deref(),
            Some(r#"[["s","a"],["t","a"]]"#),
            "somewhere else"
        );
        assert_eq!(
            with_counted(&one, "s", "").as_deref(),
            Some(one.as_str()),
            "nobody known is counted each time"
        );
        assert_eq!(
            with_counted("not a list", "s", "a").as_deref(),
            Some(r#"[["s","a"]]"#)
        );
        let full: Vec<(String, String)> = (0..MOST_DOERS)
            .map(|n| (n.to_string(), "a".to_owned()))
            .collect();
        let full = serde_json::to_string(&full).expect("JSON");
        assert_eq!(
            with_counted(&full, "x", "a").as_deref(),
            Some(full.as_str()),
            "counted, not kept"
        );
    }
}
