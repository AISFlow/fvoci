//! What people were told inside the application (`notifications`), as they read it:
//! what is noted of a write (`notification_noting.rs`) and made of that by the server
//! that holds the lock (`notification_outbox.rs`) is listed, counted and read here.
//!
//! A notification holds no title and no name. Whoever lists them is asked again, of
//! each, whether they read what it is about; the list here is narrowed by its reader
//! and by nothing else, across every organization.

use std::collections::{BTreeMap, BTreeSet};

use fvoci_model::{BlockType, ContainerKind, NotificationKind, SearchKind};

use crate::Store;
use crate::blocks::Paging;
use crate::db::{Dialect, Id, Row, Scope, Sql};
use crate::directory::{HAS_PASSWORD, Making, org_kind, taken};
use crate::error::StoreError;
use crate::search_docs::select_documents;

/// How many identifiers one statement binds.
const BATCH: usize = 300;

/// Who reads their notifications, and with which session: an organization that takes
/// only the sessions made through its provider keeps what it told its people from
/// any other session of theirs, as it keeps its boards
/// (`Store::container_memberships_of_user`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotificationReader<'a> {
    /// The user of the session.
    pub user_id: &'a str,
    /// How the session was made, and what it confirmed since.
    pub making: Making<&'a str>,
}

impl NotificationReader<'_> {
    /// The organizations the reader was told something in that their session does not
    /// open, by the one rule of it ([`crate::directory::SessionTaken::opened_by`]):
    /// organizations that people work in that require their SSO provider, and personal
    /// spaces whose account has a password that this session has not confirmed.
    async fn closed_organizations(self, scope: &mut Scope<'_>) -> Result<Vec<String>, StoreError> {
        let mut sql = scope.sql([
            "SELECT DISTINCT n.org_id, o.kind, p.id, ",
            HAS_PASSWORD,
            " FROM notifications AS n \
             JOIN organizations AS o ON o.id = n.org_id \
             JOIN users AS u ON u.id = n.recipient_id \
             LEFT JOIN oidc_providers AS p ON p.org_id = n.org_id AND p.sso_required = ",
        ]);
        sql.bind(true)
            .push_bind(" WHERE n.recipient_id = ", Id(self.user_id));
        let mut closed = Vec::new();
        for row in scope.fetch_all(sql).await? {
            let kind = org_kind(&row, 1)?;
            let takes = taken(kind, row.id(2)?, &row.text(3)?);
            if !takes.opened_by(self.making) {
                closed.push(row.id(0)?);
            }
        }
        Ok(closed)
    }

    /// Binds the reader as the recipient and leaves out the organizations of `closed`
    /// ([`NotificationReader::closed_organizations`]): for a statement on
    /// `notifications`, with no other name for the table, whose text ends at
    /// `recipient_id = `.
    fn push_theirs(self, closed: &[String], sql: &mut Sql) {
        sql.bind(Id(self.user_id));
        if !closed.is_empty() {
            sql.push(" AND org_id NOT IN (")
                .bind_list(closed.iter().map(|org_id| Id(org_id)))
                .push(")");
        }
    }
}

/// A notification as it is stored: what it names, and nothing of what that is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredNotification {
    /// Its identifier.
    pub id: String,
    /// The organization of what it is about.
    pub org_id: String,
    /// Of what it tells.
    pub kind: NotificationKind,
    /// Where it happened.
    pub source_id: String,
    /// The card or the page it is about.
    pub block_id: String,
    /// The board or the wiki of that, when the latest of it happened.
    pub container_id: String,
    /// Who did the latest of it; empty where nobody is known to have.
    pub actor_id: String,
    /// The name of the token an agent of `actor_id` did the latest of it through, if
    /// one did.
    pub actor_token_name: Option<String>,
    /// When it was made: when the first of it happened.
    pub created_at: i64,
    /// When the latest of it happened.
    pub updated_at: i64,
    /// How many times it happened.
    pub times: i64,
    /// How many people did it, as far as they are counted.
    pub people: i64,
    /// When its reader read it; 0 until then, and since the latest of it.
    pub read_at: i64,
}

/// Where a card or a page a notification is about is now, and what it is called.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationPlace {
    /// The card or the page.
    pub block_id: String,
    /// What it is.
    pub block_kind: SearchKind,
    /// Its title.
    pub title: String,
    /// The board it is on, or the wiki it is in.
    pub container_id: String,
    /// What that container is.
    pub container_kind: ContainerKind,
    /// The workspace of the container.
    pub workspace_id: String,
}

/// Live container metadata for resolving `added_to_container` notifications.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerNotificationPlace {
    /// Container identifier.
    pub container_id: String,
    /// Container kind (`board`, `wiki`, `course`).
    pub container_kind: ContainerKind,
    /// Current container title.
    pub title: String,
    /// Workspace identifier of the container.
    pub workspace_id: String,
}

const NOTIFICATION_COLUMNS: &str = "id, org_id, kind, source_id, block_id, container_id, \
     actor_id, created_at, updated_at, times, doers, read_at, actor_token_name";

impl Store {
    /// A page of the notifications of a reader, of every organization their session
    /// opens, the newest first. What each is about is named and no more: whether the
    /// user reads it is for the caller to ask, of each.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn notifications_of_user(
        &self,
        reader: NotificationReader<'_>,
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
                sql.push(" ORDER BY updated_at DESC, id DESC")
                    .push_page(paging);
                scope
                    .fetch_all(sql)
                    .await?
                    .iter()
                    .map(notification_from_row)
                    .collect()
            })
            .await
    }

    /// How many notifications a reader has not read, of every organization their
    /// session opens.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn unread_notifications_of_user(
        &self,
        reader: NotificationReader<'_>,
    ) -> Result<i64, StoreError> {
        self.system()
            .with_conn(async |scope| {
                let closed = reader.closed_organizations(scope).await?;
                let mut sql = scope.sql(
                    "SELECT COUNT(*) FROM notifications WHERE read_at = 0 AND recipient_id = ",
                );
                reader.push_theirs(&closed, &mut sql);
                match scope.fetch_optional(sql).await? {
                    Some(row) => row.int(0),
                    None => Ok(0),
                }
            })
            .await
    }

    /// Whether what the organization `org_id` told the reader is kept from their
    /// session: either an organization requiring its SSO provider or a personal space
    /// whose account has a password that this session has not confirmed
    /// ([`crate::directory::SessionTaken::opened_by`]).
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn notifications_kept_from(
        &self,
        reader: NotificationReader<'_>,
        org_id: &str,
    ) -> Result<bool, StoreError> {
        self.system()
            .with_conn(async |scope| {
                let mut sql = scope.sql([
                    "SELECT o.kind, p.id, ",
                    HAS_PASSWORD,
                    " FROM organizations AS o \
                     JOIN users AS u ON u.id = ",
                ]);
                sql.bind(Id(reader.user_id))
                    .push_bind(
                        " LEFT JOIN oidc_providers AS p ON p.org_id = o.id AND p.sso_required = ",
                        true,
                    )
                    .push_bind(" WHERE o.id = ", Id(org_id));
                for row in scope.fetch_all(sql).await? {
                    let kind = org_kind(&row, 0)?;
                    let takes = taken(kind, row.id(1)?, &row.text(2)?);
                    if !takes.opened_by(reader.making) {
                        return Ok(true);
                    }
                }
                Ok(false)
            })
            .await
    }

    /// Marks a notification of a reader as read at `now`, in whichever organization
    /// their session opens it is; one that was read keeps the time it was. Returns
    /// the organization it is of, where the reader has such a notification: another's
    /// is none of theirs, and neither is one of an organization the session does not
    /// open.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a statement fails.
    pub async fn read_notification(
        &self,
        reader: NotificationReader<'_>,
        notification_id: &str,
        now: i64,
    ) -> Result<Option<String>, StoreError> {
        self.system()
            .transaction(async |scope| {
                let closed = reader.closed_organizations(scope).await?;
                let mut sql =
                    scope.sql("SELECT org_id, read_at FROM notifications WHERE recipient_id = ");
                reader.push_theirs(&closed, &mut sql);
                sql.push_bind(" AND id = ", Id(notification_id));
                let Some(row) = scope.fetch_optional(sql).await? else {
                    return Ok(None);
                };
                if row.int(1)? == 0 {
                    let read_at = now.max(1);
                    let mut sql = match scope.dialect() {
                        Dialect::Postgres => {
                            scope.sql("UPDATE notifications SET read_at = GREATEST(")
                        }
                        Dialect::Sqlite => scope.sql("UPDATE notifications SET read_at = MAX("),
                    };
                    sql.bind(read_at)
                        .push(", updated_at)")
                        .push_bind(" WHERE recipient_id = ", Id(reader.user_id))
                        .push_bind(" AND id = ", Id(notification_id))
                        .push(" AND read_at = 0");
                    scope.execute(sql).await?;
                }
                Ok(Some(row.id(0)?))
            })
            .await
    }

    /// Marks as read at `now` every notification of a reader whose latest happened up
    /// to `through` and that is not read yet, of every organization their session
    /// opens. Returns how many, by the organization they are of.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the statement fails.
    pub async fn read_notifications_of_user(
        &self,
        reader: NotificationReader<'_>,
        through: i64,
        now: i64,
    ) -> Result<BTreeMap<String, u64>, StoreError> {
        self.system()
            .transaction(async |scope| {
                let closed = reader.closed_organizations(scope).await?;
                let read_at = now.max(1);
                let mut sql = match scope.dialect() {
                    Dialect::Postgres => scope.sql("UPDATE notifications SET read_at = GREATEST("),
                    Dialect::Sqlite => scope.sql("UPDATE notifications SET read_at = MAX("),
                };
                sql.bind(read_at)
                    .push(", updated_at) WHERE read_at = 0 AND recipient_id = ");
                reader.push_theirs(&closed, &mut sql);
                sql.push_bind(" AND updated_at <= ", through)
                    .push(" RETURNING org_id");
                let mut read = BTreeMap::new();
                for row in scope.fetch_all(sql).await? {
                    *read.entry(row.id(0)?).or_insert(0) += 1;
                }
                Ok(read)
            })
            .await
    }

    /// Where those of `block_ids` are now that a notification may be about, among the
    /// rows this handle reaches: the cards and the pages that a search would find
    /// (none of the trash, no template, on no template). What is none of them has no
    /// place.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a query fails.
    pub async fn notification_places(
        &self,
        block_ids: &[&str],
    ) -> Result<Vec<NotificationPlace>, StoreError> {
        let ids: BTreeSet<&str> = block_ids.iter().copied().collect();
        let ids: Vec<&str> = ids.into_iter().collect();
        self.with_conn(async |scope| {
            let mut places = Vec::with_capacity(ids.len());
            for batch in ids.chunks(BATCH) {
                let mut sql = select_documents(
                    scope,
                    "b.id, b.type, b.title, b.container_id, bo.kind, bo.workspace_id",
                );
                sql.push(" AND b.type IN (")
                    .bind_list([
                        BlockType::Card.as_str(),
                        BlockType::Page.as_str(),
                        BlockType::Unit.as_str(),
                    ])
                    .push(") AND bo.kind IN (")
                    .bind_list([
                        ContainerKind::Board.as_str(),
                        ContainerKind::Wiki.as_str(),
                        ContainerKind::Course.as_str(),
                    ])
                    .push(") AND b.id IN (")
                    .bind_list(batch.iter().map(|id| Id(id)))
                    .push(")");
                for row in scope.fetch_all(sql).await? {
                    places.push(place_from_row(&row)?);
                }
            }
            Ok(places)
        })
        .await
    }

    /// The comments among `comment_ids` that are there now, among the rows this
    /// handle reaches, each with the block it is on. One in the trash is not there.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a query fails.
    pub async fn comments_with_parents(
        &self,
        comment_ids: &[&str],
    ) -> Result<Vec<(String, String)>, StoreError> {
        let ids: BTreeSet<&str> = comment_ids.iter().copied().collect();
        let ids: Vec<&str> = ids.into_iter().collect();
        self.with_conn(async |scope| {
            let mut found = Vec::with_capacity(ids.len());
            for batch in ids.chunks(BATCH) {
                let mut sql = scope.sql("SELECT id, parent_id FROM blocks WHERE type = ");
                sql.bind(BlockType::Comment.as_str())
                    .push(" AND id IN (")
                    .bind_list(batch.iter().map(|id| Id(id)))
                    .push(")");
                for row in scope.fetch_all(sql).await? {
                    found.push((row.id(0)?, row.id(1)?));
                }
            }
            Ok(found)
        })
        .await
    }

    /// Resolves live non-template containers among `container_ids` visible to this
    /// handle for rendering `added_to_container` notifications.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a query fails.
    pub async fn container_notification_places(
        &self,
        container_ids: &[&str],
    ) -> Result<Vec<ContainerNotificationPlace>, StoreError> {
        let ids: BTreeSet<&str> = container_ids.iter().copied().collect();
        let ids: Vec<&str> = ids.into_iter().collect();
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        self.with_conn(async |scope| {
            let mut places = Vec::with_capacity(ids.len());
            for batch in ids.chunks(BATCH) {
                let mut sql = scope.sql(
                    "SELECT c.id, c.kind, c.title, c.workspace_id \
                     FROM containers AS c \
                     JOIN workspaces AS w ON w.id = c.workspace_id \
                     JOIN organizations AS o ON o.id = w.org_id \
                     WHERE c.is_template = ",
                );
                sql.bind(false)
                    .push(" AND w.deleted_at IS NULL AND o.deleted_at IS NULL")
                    .push(&scope.same_organization(&[("c", "w")]))
                    .push(" AND c.id IN (")
                    .bind_list(batch.iter().map(|id| Id(id)))
                    .push(")");
                for row in scope.fetch_all(sql).await? {
                    places.push(ContainerNotificationPlace {
                        container_id: row.id(0)?,
                        container_kind: crate::container_kinds::kind_from_text(&row.text(1)?)?,
                        title: row.text(2)?,
                        workspace_id: row.id(3)?,
                    });
                }
            }
            Ok(places)
        })
        .await
    }
}

pub(crate) fn kind_from_row(row: &Row, index: usize) -> Result<NotificationKind, StoreError> {
    row.text(index)?
        .parse()
        .map_err(
            |error: fvoci_model::UnknownNotificationKind| StoreError::Corrupt {
                column: "kind",
                reason: error.to_string(),
            },
        )
}

fn notification_from_row(row: &Row) -> Result<StoredNotification, StoreError> {
    Ok(StoredNotification {
        id: row.id(0)?,
        org_id: row.id(1)?,
        kind: kind_from_row(row, 2)?,
        source_id: row.id(3)?,
        block_id: row.id(4)?,
        container_id: row.id(5)?,
        actor_id: row.id(6)?,
        created_at: row.int(7)?,
        updated_at: row.int(8)?,
        times: row.int(9)?,
        people: people_of(&row.text(10)?),
        read_at: row.int(11)?,
        actor_token_name: row.opt_text(12)?,
    })
}

/// How many people a line's doers name (`doers`, a JSON list); none for a list that
/// does not read.
fn people_of(doers: &str) -> i64 {
    serde_json::from_str::<Vec<serde_json::Value>>(doers)
        .map_or(0, |doers| i64::try_from(doers.len()).unwrap_or(i64::MAX))
}

fn place_from_row(row: &Row) -> Result<NotificationPlace, StoreError> {
    let block_type = crate::blocks::block_type_column(row, 1, "type")?;
    let block_kind = SearchKind::of_block(&block_type).ok_or_else(|| StoreError::Corrupt {
        column: "type",
        reason: format!("a block of type {block_type} is no card, page or unit"),
    })?;
    let container_kind = crate::container_kinds::kind_from_text(&row.text(4)?)?;
    Ok(NotificationPlace {
        block_id: row.id(0)?,
        block_kind,
        title: row.text(2)?,
        container_id: row.id(3)?,
        container_kind,
        workspace_id: row.id(5)?,
    })
}
