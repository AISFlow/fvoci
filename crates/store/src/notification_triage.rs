//! What a person does with their notifications besides reading them, as an inbox is
//! sorted: listing them by whether they are read, by what they tell and by whether they
//! were put away (`archived_at`), marking one unread again, putting one away and taking
//! it back, and putting away every one that was read.
//!
//! One put away is read: putting away one that is not reads it, and marking one unread
//! takes it out of the archive too. More of it within the hour (`notification_outbox.rs`)
//! brings one put away back into the inbox, unread, as it makes one that was read unread.
//! What an organization the session does not open told its reader is out of reach here as
//! it is for reading ([`NotificationReader`]).

use std::collections::BTreeMap;

use fvoci_model::NotificationFilter;

use crate::Store;
use crate::blocks::Paging;
use crate::db::{Id, Scope};
use crate::error::StoreError;
use crate::notifications::{NOTIFICATION_COLUMNS, NotificationReader, StoredNotification};

/// What a change of one notification sets: the text after `SET`, with `now` bound where
/// it is written `{now}`.
const UNREAD: &str = "read_at = 0, archived_at = 0";
const ARCHIVE: &str = "archived_at = CASE WHEN archived_at = 0 THEN {now} ELSE archived_at END, \
     read_at = CASE WHEN read_at > 0 THEN read_at WHEN updated_at > {now} THEN updated_at \
     ELSE {now} END";
const UNARCHIVE: &str = "archived_at = 0";

impl Store {
    /// A page of the notifications of a reader that `filter` takes, of every organization
    /// their session opens, the newest first.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn notifications_found(
        &self,
        reader: NotificationReader<'_>,
        filter: NotificationFilter,
        paging: Paging,
    ) -> Result<Vec<StoredNotification>, StoreError> {
        self.system()
            .with_conn(async |scope| {
                let closed = reader.closed_organizations(scope).await?;
                let mut sql = scope.sql([
                    "SELECT ",
                    NOTIFICATION_COLUMNS,
                    " FROM notifications WHERE recipient_id = ",
                ]);
                reader.push_theirs(&closed, &mut sql);
                sql.push(if filter.archived {
                    " AND archived_at > 0"
                } else {
                    " AND archived_at = 0"
                });
                if filter.unread {
                    sql.push(" AND read_at = 0");
                }
                if let Some(kind) = filter.kind {
                    sql.push_bind(" AND kind = ", kind.as_str());
                }
                sql.push(" ORDER BY updated_at DESC, id DESC")
                    .push_page(paging);
                scope.all(sql).await
            })
            .await
    }

    /// Marks a notification of a reader unread again, and takes it out of the archive.
    /// Returns the organization it is of, where the reader has such a notification
    /// (as [`Store::read_notification`]).
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a statement fails.
    pub async fn unread_notification(
        &self,
        reader: NotificationReader<'_>,
        notification_id: &str,
    ) -> Result<Option<String>, StoreError> {
        self.change_notification(reader, notification_id, UNREAD, 0)
            .await
    }

    /// Puts a notification of a reader away at `now`, reading it if it was not; one put
    /// away keeps the time it was. Returns the organization it is of, where the reader
    /// has such a notification (as [`Store::read_notification`]).
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a statement fails.
    pub async fn archive_notification(
        &self,
        reader: NotificationReader<'_>,
        notification_id: &str,
        now: i64,
    ) -> Result<Option<String>, StoreError> {
        self.change_notification(reader, notification_id, ARCHIVE, now.max(1))
            .await
    }

    /// Takes a notification of a reader out of the archive, into the inbox; it stays
    /// read. Returns the organization it is of, where the reader has such a
    /// notification (as [`Store::read_notification`]).
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a statement fails.
    pub async fn unarchive_notification(
        &self,
        reader: NotificationReader<'_>,
        notification_id: &str,
    ) -> Result<Option<String>, StoreError> {
        self.change_notification(reader, notification_id, UNARCHIVE, 0)
            .await
    }

    /// Puts away at `now` every notification of a reader that was read and is in the
    /// inbox, of every organization their session opens. Returns how many, by the
    /// organization they are of.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the statement fails.
    pub async fn archive_read_notifications(
        &self,
        reader: NotificationReader<'_>,
        now: i64,
    ) -> Result<BTreeMap<String, u64>, StoreError> {
        self.system()
            .transaction(async |scope| {
                let closed = reader.closed_organizations(scope).await?;
                let mut sql = scope.sql("UPDATE notifications SET archived_at = ");
                sql.bind(now.max(1))
                    .push(" WHERE archived_at = 0 AND read_at > 0 AND recipient_id = ");
                reader.push_theirs(&closed, &mut sql);
                sql.push(" RETURNING org_id");
                let mut archived = BTreeMap::new();
                for row in scope.fetch_all(sql).await? {
                    *archived.entry(row.id(0)?).or_insert(0) += 1;
                }
                Ok(archived)
            })
            .await
    }

    /// Sets `set` on a notification of a reader, `now` bound for each `{now}` of it, and
    /// returns the organization it is of; `None` where the reader has no such
    /// notification that their session opens.
    async fn change_notification(
        &self,
        reader: NotificationReader<'_>,
        notification_id: &str,
        set: &'static str,
        now: i64,
    ) -> Result<Option<String>, StoreError> {
        self.system()
            .transaction(async |scope| change_one(scope, reader, notification_id, set, now).await)
            .await
    }
}

async fn change_one(
    scope: &mut Scope<'_>,
    reader: NotificationReader<'_>,
    notification_id: &str,
    set: &'static str,
    now: i64,
) -> Result<Option<String>, StoreError> {
    let closed = reader.closed_organizations(scope).await?;
    let mut sql = scope.sql("UPDATE notifications SET ");
    for (at, part) in set.split("{now}").enumerate() {
        if at > 0 {
            sql.bind(now);
        }
        sql.push(part);
    }
    sql.push(" WHERE recipient_id = ");
    reader.push_theirs(&closed, &mut sql);
    sql.push_bind(" AND id = ", Id(notification_id))
        .push(" RETURNING org_id");
    match scope.fetch_optional(sql).await? {
        Some(row) => Ok(Some(row.id(0)?)),
        None => Ok(None),
    }
}
