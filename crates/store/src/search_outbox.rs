//! What the search server is still to be told: the list a change of a block or a board
//! is put on in its own transaction, and that the server's worker works off.
//!
//! The search server is no part of a transaction, so a change is noted here with it,
//! and sent afterwards. A row says that a block, or everything of a board, is to be
//! sent again; what is sent is what is stored when the row is read, never something
//! the row carries. A row is taken off once the search server holds that, unless the
//! row was noted again meanwhile (`seq`): then what was sent may be behind, and the row
//! stays for the next round. Whatever stops in between, the list says what is left.
//!
//! Rows are noted only on a server that has a search server
//! ([`Store::keep_search_outbox`]). The database also remembers which index the list
//! has been kept for ([`SearchIndexMark`]): an index it was not kept for is made again
//! and every board listed.

use std::sync::atomic::Ordering;

use crate::Store;
use crate::db::{FromRow, Id, Row, Scope};
use crate::error::StoreError;

/// Bounded parameters however many blocks one change notes.
const BATCH: usize = 300;
/// Bounded rows settled in one multi-tuple `DELETE FROM search_outbox` statement.
const SETTLE_BATCH: usize = 100;

/// What a row of the list is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SearchTarget {
    /// One block: its document is put, or removed if it has none now.
    Block,
    /// A board: every document of it is sent again, and those of blocks that are
    /// gone removed.
    Container,
}

impl SearchTarget {
    /// The value of the `scope` column.
    fn as_str(self) -> &'static str {
        match self {
            Self::Block => "block",
            Self::Container => "container",
        }
    }

    fn parse(text: &str) -> Result<Self, StoreError> {
        match text {
            "block" => Ok(Self::Block),
            "container" => Ok(Self::Container),
            other => Err(StoreError::Corrupt {
                column: "scope",
                reason: format!("unknown scope {other:?}"),
            }),
        }
    }
}

/// A row of the list, as the worker reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchChange {
    /// What it is about.
    pub target: SearchTarget,
    /// The identifier of the block or the board.
    pub id: String,
    /// The organization it was noted in.
    pub org_id: String,
    /// What the row's `seq` was when it was read.
    pub seq: i64,
    /// How many times in a row it could not be sent.
    pub attempts: i64,
}

/// The index of the search server the list has been kept for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchIndexMark {
    /// Its name.
    pub name: String,
    /// The identifier the search server gave it when it was made.
    pub uuid: String,
    /// The version of the definition it was made from.
    pub version: i64,
}

/// How the list stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SearchOutboxCounts {
    /// The rows on it.
    pub waiting: i64,
    /// Those of them that could not be sent at least once in a row.
    pub failing: i64,
}

const CHANGE_COLUMNS: &str = "scope, id, org_id, seq, attempts";

/// What every statement that notes a row ends with: a row that is there already moves
/// its `seq` on, past whatever it was, and is due again now.
const NOTED_AGAIN: &str = " ON CONFLICT (org_id, scope, id) DO UPDATE SET seq = CASE \
     WHEN search_outbox.seq >= excluded.seq THEN search_outbox.seq + 1 ELSE excluded.seq END, \
     due_at = excluded.due_at, attempts = 0";

impl Store {
    /// Has every change of a block or a board noted for the search server from now on,
    /// by every handle to this database: for a server that has one.
    pub fn keep_search_outbox(&self) {
        self.inner.search_outbox.store(true, Ordering::Relaxed);
    }

    /// Forgets the list and the index it was kept for, and then stops noting changes:
    /// for a server that has no search server, or one that cannot hold the index. An
    /// index found later is one the list was not kept for. If it cannot be forgotten,
    /// changes are noted as before.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a statement fails.
    pub async fn drop_search_outbox(&self) -> Result<(), StoreError> {
        self.transaction(async |scope| {
            // Read first: a server that never had a search server writes nothing here.
            let sql = scope.sql(
                "SELECT (SELECT COUNT(*) FROM search_outbox) + \
                 (SELECT COUNT(*) FROM search_index_state)",
            );
            let rows = scope.fetch_optional(sql).await?;
            if rows.map(|row| row.int(0)).transpose()?.unwrap_or(0) == 0 {
                return Ok(());
            }
            for stmt in [
                "DELETE FROM search_outbox",
                "DELETE FROM search_index_state",
            ] {
                let sql = scope.sql(stmt);
                scope.execute(sql).await?;
            }
            Ok(())
        })
        .await?;
        // Only once the index is forgotten: while it is remembered, what changes is
        // noted, or the index would be taken for whole when it is found again. What
        // is noted between the two is forgotten or listed again by the next start.
        self.inner.search_outbox.store(false, Ordering::Relaxed);
        Ok(())
    }

    /// Up to `limit` rows of the list that are due at `now`, those due the longest
    /// first, among the rows this handle reaches.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn due_search_changes(
        &self,
        now: i64,
        limit: i64,
    ) -> Result<Vec<SearchChange>, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = scope.sql([
                "SELECT ",
                CHANGE_COLUMNS,
                " FROM search_outbox WHERE due_at <= ",
            ]);
            sql.bind(now)
                .push_bind(" ORDER BY due_at, scope, id LIMIT ", limit);
            scope.all(sql).await
        })
        .await
    }

    /// Takes a row off the list once the search server holds what was stored when the
    /// row was read. Returns whether it was taken off: a row noted again since
    /// (another `seq`) stays, for what was sent may be behind it.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the statement fails.
    pub async fn settle_search_change(&self, change: &SearchChange) -> Result<bool, StoreError> {
        Ok(self
            .settle_search_changes(std::slice::from_ref(change))
            .await?
            > 0)
    }

    /// Takes a batch of rows off the list in one transaction once the search server
    /// holds what was stored when they were read. Rows noted again since (`seq`
    /// advanced) stay on the list for the next round. Returns how many rows were
    /// deleted.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a statement fails.
    pub async fn settle_search_changes(&self, changes: &[SearchChange]) -> Result<u64, StoreError> {
        if changes.is_empty() {
            return Ok(0);
        }
        self.transaction(async |scope| {
            let mut total = 0_u64;
            for chunk in changes.chunks(SETTLE_BATCH) {
                let mut sql = scope.sql("DELETE FROM search_outbox WHERE ");
                for (index, change) in chunk.iter().enumerate() {
                    sql.push(if index == 0 { "(" } else { " OR (" })
                        .push_bind("org_id = ", Id(&change.org_id))
                        .push_bind(" AND scope = ", change.target.as_str())
                        .push_bind(" AND id = ", Id(&change.id))
                        .push_bind(" AND seq = ", change.seq)
                        .push(")");
                }
                total += scope.execute(sql).await?;
            }
            Ok(total)
        })
        .await
    }

    /// Notes that a row could not be sent, and when to try again. A row noted again
    /// since is due already, and left as it is.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the statement fails.
    pub async fn search_change_failed(
        &self,
        change: &SearchChange,
        retry_at: i64,
    ) -> Result<(), StoreError> {
        self.with_write_conn(async |scope| {
            let mut sql = scope.sql("UPDATE search_outbox SET attempts = attempts + 1, due_at = ");
            sql.bind(retry_at);
            push_row(&mut sql, change);
            scope.execute(sql).await?;
            Ok(())
        })
        .await
    }

    /// How the list stands, among the rows this handle reaches.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn search_outbox_counts(&self) -> Result<SearchOutboxCounts, StoreError> {
        self.with_conn(async |scope| {
            let sql = scope.sql(
                "SELECT COUNT(*), COALESCE(SUM(CASE WHEN attempts > 0 THEN 1 ELSE 0 END), 0) \
                 FROM search_outbox",
            );
            let row = scope.fetch_optional(sql).await?;
            Ok(match row {
                Some(row) => SearchOutboxCounts {
                    waiting: row.int(0)?,
                    failing: row.int(1)?,
                },
                None => SearchOutboxCounts::default(),
            })
        })
        .await
    }

    /// The index the list has been kept for; `None` if it has been kept for none.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn search_index_mark(&self) -> Result<Option<SearchIndexMark>, StoreError> {
        self.with_conn(async |scope| {
            let sql = scope.sql("SELECT name, uuid, version FROM search_index_state WHERE id = 1");
            scope
                .fetch_optional(sql)
                .await?
                .map(|row| {
                    Ok(SearchIndexMark {
                        name: row.text(0)?,
                        uuid: row.text(1)?,
                        version: row.int(2)?,
                    })
                })
                .transpose()
        })
        .await
    }

    /// Forgets which index the list has been kept for: the index the server finds next
    /// is made again, and every board listed.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the statement fails.
    pub async fn forget_search_index(&self) -> Result<(), StoreError> {
        self.with_write_conn(async |scope| {
            let sql = scope.sql("DELETE FROM search_index_state");
            scope.execute(sql).await?;
            Ok(())
        })
        .await
    }

    /// Lists every board that is no template, of every organization this handle
    /// reaches (for the handle of the housekeeping, which reaches all: the index is
    /// every organization's), and remembers `mark` as the index the list is kept for from here on,
    /// in one transaction. Returns how many boards were listed. For an index that was
    /// just made: what it lacks is everything, and the list says so before the
    /// database says the index is the one it keeps.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a statement fails; nothing is listed or
    /// remembered then.
    pub async fn list_every_container_for_search(
        &self,
        mark: &SearchIndexMark,
    ) -> Result<u64, StoreError> {
        self.transaction(async |scope| {
            let now = scope.now_millis();
            let mut sql = scope.sql(
                "INSERT INTO search_outbox (scope, id, org_id, seq, due_at) \
                 SELECT 'container', bo.id, w.org_id, ",
            );
            sql.bind(now)
                .push_bind(", ", now)
                .push(" FROM containers AS bo JOIN workspaces AS w ON w.id = bo.workspace_id")
                .push(&scope.same_organization(&[("w", "bo")]))
                // In the order every writer of several boards takes them.
                .push(" WHERE bo.is_template = false AND bo.is_builtin = false ORDER BY bo.id")
                .push(NOTED_AGAIN);
            let listed = scope.execute(sql).await?;
            let sql = scope.sql("DELETE FROM search_index_state");
            scope.execute(sql).await?;
            let mut sql =
                scope.sql("INSERT INTO search_index_state (id, name, uuid, version) VALUES (1, ");
            sql.bind(mark.name.as_str())
                .push_bind(", ", mark.uuid.as_str())
                .push_bind(", ", mark.version)
                .push(")");
            scope.execute(sql).await?;
            Ok(listed)
        })
        .await
    }
}

/// Narrows a statement to the row `change` was read from, as it was read.
fn push_row(sql: &mut crate::db::Sql, change: &SearchChange) {
    sql.push_bind(" WHERE org_id = ", Id(&change.org_id))
        .push_bind(" AND scope = ", change.target.as_str())
        .push_bind(" AND id = ", Id(&change.id))
        .push_bind(" AND seq = ", change.seq);
}

impl FromRow for SearchChange {
    fn from_row(row: &Row) -> Result<Self, StoreError> {
        Ok(SearchChange {
            target: SearchTarget::parse(&row.text(0)?)?,
            id: row.id(1)?,
            org_id: row.id(2)?,
            seq: row.int(3)?,
            attempts: row.int(4)?,
        })
    }
}

/// Notes that the blocks `ids` changed, in the transaction that changes them: while
/// their rows are among the blocks, which say whose they are. Nothing is noted on a
/// server without a search server.
pub(crate) async fn note_blocks(scope: &mut Scope<'_>, ids: &[&str]) -> Result<(), StoreError> {
    if !scope.keeps_search_outbox() {
        return Ok(());
    }
    let now = scope.now_millis();
    for batch in ids.chunks(BATCH) {
        let mut sql = scope.sql(
            "INSERT INTO search_outbox (scope, id, org_id, seq, due_at) \
             SELECT 'block', b.id, w.org_id, ",
        );
        sql.bind(now)
            .push_bind(", ", now)
            .push(" FROM blocks AS b JOIN containers AS bo ON bo.id = b.container_id")
            .push(&scope.same_organization(&[("bo", "b")]))
            .push(" JOIN workspaces AS w ON w.id = bo.workspace_id")
            .push(&scope.same_organization(&[("w", "bo")]))
            .push(" WHERE b.id IN (")
            .bind_list(batch.iter().map(|id| Id(id)))
            // In one order, whoever notes: two writers of the same blocks wait for
            // one another's rows, and never each for the other's.
            .push(") ORDER BY b.id")
            .push(NOTED_AGAIN);
        scope.execute(sql).await?;
    }
    Ok(())
}

/// Notes that everything of the board changed, in the transaction that changes it:
/// while its row is among the boards, which says whose it is. Nothing is noted on a
/// server without a search server.
pub(crate) async fn note_container(
    scope: &mut Scope<'_>,
    container_id: &str,
) -> Result<(), StoreError> {
    if !scope.keeps_search_outbox() {
        return Ok(());
    }
    note_container_regardless(scope, container_id).await
}

/// Notes that everything of the board changed whether or not the list is kept: for a
/// change of the data after the migrations, which is made before the server knows
/// whether it has a search server. A server without one forgets the list before it
/// serves.
pub(crate) async fn note_container_regardless(
    scope: &mut Scope<'_>,
    container_id: &str,
) -> Result<(), StoreError> {
    let now = scope.now_millis();
    let mut sql = scope.sql(
        "INSERT INTO search_outbox (scope, id, org_id, seq, due_at) \
         SELECT 'container', bo.id, w.org_id, ",
    );
    sql.bind(now)
        .push_bind(", ", now)
        .push(" FROM containers AS bo JOIN workspaces AS w ON w.id = bo.workspace_id")
        .push(&scope.same_organization(&[("w", "bo")]))
        .push_bind(" WHERE bo.id = ", Id(container_id))
        .push(NOTED_AGAIN);
    scope.execute(sql).await?;
    Ok(())
}
