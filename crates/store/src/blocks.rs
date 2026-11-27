//! The blocks that exist: cards, views, comments and what is on a card.
//!
//! A block is its board's, and below the top level its parent's, which is a block of the
//! same board. Deleting moves a block to the trash and restoring brings it back; both
//! are in [`crate::trash`]. Lists come oldest first, by creation time and then by
//! identifier.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::num::NonZeroU32;

use fvoci_model::{
    BODY_VERSION, Block, BlockError, BlockPatch, BlockPatchBatch, BlockType, Body, BodySummary,
    ContainerKind, TaskCount,
};
use serde_json::Value;

use crate::Store;
use crate::block_derived::{self, Before};
use crate::body_versions::{self, BodyWrite};
use crate::card_hierarchy::{self, ParentInfo};
use crate::card_numbers;
use crate::card_values::{self, NamedCards};
use crate::comment_places;
use crate::comment_versions::{self, EDITED_AT_COLUMN};
use crate::containers;
use crate::db::{Id, OptId, Param, Row, Scope, Sql, SqlFragment};
use crate::error::StoreError;
use crate::file_refs::NewBodyFiles;
use crate::notification_noting::{self, Telling};
use crate::page_places;

/// The name of every block column, in the order rows are read and written.
pub(crate) const BLOCK_COLUMNS: &str = "id, parent_id, created_by, modified_by, type, title, \
                             fields, created_at, updated_at, container_id, number, body";

/// The column of a block read with [`BLOCK_COLUMNS`] + [`EDITED_AT_COLUMN`] that says
/// when its body was last changed.
const EDITED_AT: usize = 12;

/// The columns that [`BLOCK_COLUMNS`] does not read: what is worked out from the body
/// when it is stored.
const BODY_SUMMARY_COLUMNS: &str =
    "body_version, body_text, body_tasks_total, body_tasks_done, body_first_image";

/// The columns of a listed block: [`BLOCK_COLUMNS`] with no body, and what a list says
/// about the body in its place.
const LISTED_BLOCK_COLUMNS: &str = "id, parent_id, created_by, modified_by, type, title, \
                             fields, created_at, updated_at, container_id, number, NULL, \
                             body IS NOT NULL, body_tasks_total, body_tasks_done";

/// When the last comment on the block `blocks.id` was posted, zero for none: its
/// comments (blocks of the type `comment` whose parent it is; deleted ones are in the
/// trash) and their `created_at`, which writing a comment again does not change. Read
/// after [`LISTED_BLOCK_COLUMNS`], at [`LISTED_LAST_COMMENT_AT`], and alone by
/// [`Store::get_last_comment_at`]. `blocks_parent_type_created_at_idx` answers it.
const LAST_COMMENT_AT_COLUMN: &str = "COALESCE((SELECT MAX(c.created_at) FROM blocks c \
                                      WHERE c.parent_id = blocks.id AND c.type = 'comment'), 0)";

/// The column of [`LISTED_BLOCK_COLUMNS`] that says whether there is a body.
const LISTED_HAS_BODY: usize = 12;
/// The column of [`LISTED_BLOCK_COLUMNS`] with the body's task items.
const LISTED_TASKS_TOTAL: usize = 13;
/// The column of [`LISTED_BLOCK_COLUMNS`] with the body's checked task items.
const LISTED_TASKS_DONE: usize = 14;
/// The column of [`LISTED_BLOCK_COLUMNS`] with when the block's last comment was posted.
const LISTED_LAST_COMMENT_AT: usize = 15;
/// The columns a list reads for previews, after the last comment's time: the beginning
/// of the plain text of the body and the first image's stored file identifier, both
/// worked out when the body was stored so listing never loads `blocks.body`.
const PREVIEW_COLUMNS: &str = "SUBSTR(body_text, 1, 201), body_first_image";
/// The column of the plain text of the body, read for previews.
const LISTED_BODY_TEXT: usize = 16;
/// The column of the first image file identifier of the body, read for previews.
const LISTED_FIRST_IMAGE: usize = 17;

/// The order of every list of blocks.
pub(crate) const OLDEST_FIRST: &str = " ORDER BY created_at, id";

/// Which blocks [`Store::get_blocks`] returns. Unset members do not filter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BlockFilter {
    /// Only blocks of this board.
    pub container_id: Option<String>,
    /// Only blocks with this parent.
    pub parent_id: Option<String>,
    /// Only blocks of this type.
    pub block_type: Option<BlockType>,
    /// Only one page of the result.
    pub paging: Option<Paging>,
}

impl BlockFilter {
    /// All blocks of a board.
    pub fn container(container_id: &str) -> Self {
        Self {
            container_id: Some(container_id.to_owned()),
            ..Self::default()
        }
    }

    /// Narrows the filter to blocks with the given parent.
    #[must_use]
    pub fn with_parent(mut self, parent_id: &str) -> Self {
        self.parent_id = Some(parent_id.to_owned());
        self
    }

    /// Narrows the filter to blocks of the given type.
    #[must_use]
    pub fn with_type(mut self, block_type: BlockType) -> Self {
        self.block_type = Some(block_type);
        self
    }
}

/// One page of a result. A read of every record takes no page (`Option<Paging>`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Paging {
    /// The zero-based page number.
    pub page: u32,
    /// The number of records per page.
    pub per_page: NonZeroU32,
}

impl Paging {
    /// How many records come before the page; at most `i64::MAX`, past every record.
    #[must_use]
    pub fn offset(self) -> i64 {
        i64::from(self.page).saturating_mul(i64::from(self.per_page.get()))
    }
}

/// The titles a copy of a board or a wiki is given, in the language of whoever asked
/// for it. The store has no words of its own: the caller brings them.
#[derive(Debug, Clone, Copy)]
pub struct CopyTitles {
    /// The title of a copy of the same kind as the source, built from the source title.
    pub copy_of: fn(&str) -> String,
    /// The title of a template made from a board.
    pub new_template: &'static str,
}

/// What a copy of a board changes relative to the source board. The store
/// knows neither what a caller keeps among a board's properties nor what day it is: the
/// caller brings both. The default changes nothing.
#[derive(Debug, Clone, Copy, Default)]
pub struct CopyChanges<'c> {
    /// The names of the board's own properties that the copy is made without.
    pub without_properties: &'c [&'c str],
    /// How many days later than the source dates the copy's dates are: the date values of
    /// its cards and the days its views filter dates by. Earlier for a negative number.
    pub days_later: i64,
}

/// A block as a list gives it: without its body, with what the list says about it.
#[derive(Debug, Clone, PartialEq)]
pub struct ListedBlock {
    /// The block, without its body.
    pub block: Block,
    /// What the list says about the body.
    pub summary: BodySummary,
    /// When the block's last comment was posted; zero if it has none.
    pub last_comment_at: i64,
    /// What a preview shows of the body, if the list was asked for previews.
    pub preview: Option<BodyPreview>,
}

/// What a list asked for previews reads of a block's body.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BodyPreview {
    /// The beginning of the body's plain text, as it is searched (a line per paragraph):
    /// its first 201 characters.
    pub text: String,
    /// The stored name of the file of the body's first image, if it has one.
    pub first_image: Option<String>,
}

/// Whether a list of blocks reads what previews show of their bodies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Previews {
    /// It does not.
    Without,
    /// It reads the plain text and the first image of each body.
    With,
}

impl Store {
    /// Writes the fields of a block as they are, without any check: for tests that need
    /// a block as an older server stored it.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the statement fails.
    #[cfg(feature = "test-util")]
    pub async fn set_block_fields_unchecked(
        &self,
        block_id: &str,
        fields: &fvoci_model::JsonMap,
    ) -> Result<(), StoreError> {
        self.with_write_conn(async |scope| {
            let mut sql = scope.sql("UPDATE blocks SET fields = ");
            sql.bind(Param::json(fields)?)
                .push_bind(" WHERE id = ", Id(block_id));
            scope.execute(sql).await?;
            Ok(())
        })
        .await
    }

    /// Returns the blocks matching `filter`, oldest first.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn get_blocks(&self, filter: &BlockFilter) -> Result<Vec<Block>, StoreError> {
        self.with_conn(async |scope| get_blocks(scope, filter).await)
            .await
    }

    /// Returns the blocks that match `filter` as [`Store::get_blocks`] does, but without
    /// their bodies: each comes with what a list says about its body instead, read from
    /// what was worked out when the body was stored, and when its last comment was posted.
    ///
    /// # Errors
    ///
    /// Returns the database's error, or [`StoreError::Corrupt`] for a row that cannot be
    /// read.
    pub async fn get_listed_blocks(
        &self,
        filter: &BlockFilter,
        previews: Previews,
    ) -> Result<Vec<ListedBlock>, StoreError> {
        self.with_conn(async |scope| {
            let columns: &[&str] = match previews {
                Previews::Without => &[LISTED_BLOCK_COLUMNS, ", ", LAST_COMMENT_AT_COLUMN],
                Previews::With => &[
                    LISTED_BLOCK_COLUMNS,
                    ", ",
                    LAST_COMMENT_AT_COLUMN,
                    ", ",
                    PREVIEW_COLUMNS,
                ],
            };
            let sql = filtered_blocks(scope, filter, columns);
            scope
                .fetch_all(sql)
                .await?
                .iter()
                .map(|row| {
                    let tasks = TaskCount {
                        total: count_column(row, LISTED_TASKS_TOTAL, "body_tasks_total")?,
                        done: count_column(row, LISTED_TASKS_DONE, "body_tasks_done")?,
                    };
                    let summary = BodySummary {
                        has_body: row.bool(LISTED_HAS_BODY)?,
                        tasks,
                    };
                    Ok(ListedBlock {
                        block: block_from_row(row)?,
                        summary,
                        last_comment_at: row.int(LISTED_LAST_COMMENT_AT)?,
                        preview: match previews {
                            Previews::Without => None,
                            Previews::With => Some(preview_from_row(row)?),
                        },
                    })
                })
                .collect()
        })
        .await
    }

    /// Returns when the last comment on a block was posted (its `created_at`); zero if
    /// it has none. Deleted comments are in the trash and do not count.
    ///
    /// # Errors
    ///
    /// Returns the database's error.
    pub async fn get_last_comment_at(&self, block_id: &str) -> Result<i64, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = scope.sql([
                "SELECT ",
                LAST_COMMENT_AT_COLUMN,
                " FROM blocks WHERE id = ",
            ]);
            sql.bind(Id(block_id));
            // No such block has no comments.
            let row = scope.fetch_optional(sql).await?;
            row.map_or(Ok(0), |row| row.int(0))
        })
        .await
    }

    /// Returns the blocks matching `ids`, oldest first.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotAllFound`] unless every identifier in `ids` resolves to
    /// a distinct live block (duplicate identifiers count as missing).
    pub async fn get_blocks_by_ids(&self, ids: &[String]) -> Result<Vec<Block>, StoreError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let unique: BTreeSet<&str> = ids.iter().map(String::as_str).collect();
        if unique.len() != ids.len() {
            return Err(StoreError::NotAllFound {
                entity: "block",
                ids: ids.to_vec(),
            });
        }
        self.with_conn(async |scope| {
            let mut sql = select_blocks(scope);
            sql.push(" WHERE id IN (")
                .bind_list(unique.iter().copied().map(Id))
                .push(")")
                .push(OLDEST_FIRST);
            let blocks = fetch_blocks(scope, sql).await?;
            if blocks.len() == ids.len() {
                Ok(blocks)
            } else {
                Err(StoreError::NotAllFound {
                    entity: "block",
                    ids: ids.to_vec(),
                })
            }
        })
        .await
    }

    /// Returns the block with the given identifier.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if there is no such block. A deleted block is
    /// not found.
    pub async fn get_block(&self, block_id: &str) -> Result<Block, StoreError> {
        self.with_conn(async |scope| get_block(scope, block_id).await)
            .await
    }

    /// Inserts `block`, or updates the block that already has its identifier.
    ///
    /// `block` is updated to what was stored: `user_id` as modifier and the current time
    /// as modification time and, for a new block, `user_id` as creator and the current
    /// time as creation time, whatever the caller put there: a block is created when it
    /// is stored, also one that is imported or copied. An update leaves the stored board,
    /// creator and creation time alone.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::InvalidBlock`] if the block fails
    /// [`Block::validate`](fvoci_model::Block::validate), its parent is not a block of
    /// its board, or the update would put it below itself; and [`StoreError::NotFound`]
    /// if its board does not exist or a block with the identifier exists on another
    /// board.
    pub async fn insert_block(&self, block: &mut Block, user_id: &str) -> Result<(), StoreError> {
        self.transaction(async |scope| insert_block(scope, block, user_id).await)
            .await
    }

    /// Inserts or updates every block as [`Store::insert_block`] does, all or none. A
    /// block may name another block of the same call as its parent, wherever in the
    /// list the two are.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::InvalidBlock`] for the first block that fails validation;
    /// nothing is written in that case.
    pub async fn insert_blocks(
        &self,
        blocks: &mut [Block],
        user_id: &str,
    ) -> Result<(), StoreError> {
        self.transaction(async |scope| {
            let telling = Telling::of(scope);
            insert_blocks(scope, blocks, user_id, NewBodyFiles::Recorded, telling).await
        })
        .await
    }

    /// Applies `patch` to the block with the given identifier and returns the updated
    /// block as stored.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if there is no such block and
    /// [`StoreError::InvalidBlock`] if the patched block fails validation.
    pub async fn patch_block(
        &self,
        block_id: &str,
        patch: &BlockPatch,
        user_id: &str,
    ) -> Result<Block, StoreError> {
        self.transaction(async |scope| {
            let telling = Telling::of(scope);
            patch_block(scope, block_id, patch, user_id, BodyWrite::Whole, telling).await
        })
        .await
    }

    /// Gives blocks of a board the bodies in `bodies` (by block identifier; `None` for
    /// none), all or none, as the rest of a write that made them: no one's change, so the
    /// blocks keep who changed them last and when, and a comment gets no version. An
    /// import points the images of the bodies it stored at the files it stored after them
    /// so. The files the blocks show are put down again.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if the board, or one of the blocks on it, is not
    /// there.
    pub async fn replace_bodies_as_written(
        &self,
        container_id: &str,
        bodies: Vec<(String, Option<Body>)>,
    ) -> Result<(), StoreError> {
        self.transaction(async |scope| {
            if !containers::lock_container(scope, container_id).await? {
                return Err(StoreError::not_found(format_args!(
                    "board ID={container_id}"
                )));
            }
            for (block_id, body) in bodies {
                if let Some(body) = &body {
                    body.check_size()
                        .map_err(|source| StoreError::InvalidBody {
                            field: "body",
                            source,
                        })?;
                }
                let mut sql = scope.sql("UPDATE blocks SET updated_at = updated_at");
                push_body(&mut sql, body.as_ref(), BodyColumns::Assign)?;
                sql.push_bind(" WHERE id = ", Id(&block_id))
                    .push_bind(" AND container_id = ", Id(container_id));
                if scope.execute(sql).await? == 0 {
                    return Err(StoreError::not_found(format_args!(
                        "block ID={block_id} on board {container_id}"
                    )));
                }
                let block = get_block(scope, &block_id).await?;
                block_derived::refresh(scope, &block, Before::Unknown, NewBodyFiles::Recorded)
                    .await?;
            }
            Ok(())
        })
        .await
    }

    /// Applies each patch of `batch` to the block at the same position, all or none.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::InvalidContainersAndBlocks`] if the batch has more identifiers
    /// than patches, and otherwise the errors of [`Store::patch_block`].
    pub async fn patch_blocks(
        &self,
        batch: &BlockPatchBatch,
        user_id: &str,
    ) -> Result<(), StoreError> {
        self.transaction(async |scope| {
            // Which boards the blocks are on is read first, so that the boards are
            // locked in one order whatever the order of the blocks.
            // A block that does not exist is reported where it is patched, below.
            let container_ids = find_container_ids(scope, &batch.block_ids).await?;
            containers::lock_containers(scope, container_ids.iter().map(String::as_str)).await?;
            for (index, block_id) in batch.block_ids.iter().enumerate() {
                let patch = batch
                    .block_patches
                    .get(index)
                    .ok_or(fvoci_model::ContainersAndBlocksError::BlockIdsAndPatchesMismatch)?;
                let telling = Telling::of(scope);
                patch_block(scope, block_id, patch, user_id, BodyWrite::Whole, telling).await?;
            }
            Ok(())
        })
        .await
    }

    /// Returns how many blocks of each type exist.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn get_block_counts_by_type(&self) -> Result<BTreeMap<String, i64>, StoreError> {
        self.with_conn(async |scope| {
            let sql = scope.sql("SELECT type, COUNT(*) FROM blocks GROUP BY type");
            scope
                .fetch_all(sql)
                .await?
                .iter()
                .map(|row| Ok((row.text(0)?, row.int(1)?)))
                .collect()
        })
        .await
    }

    /// Lightweight lookup of the live `container_id` owning `block_id`, without reading
    /// `body` or `fields`.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::InvalidId`] if `block_id` is invalid, or
    /// [`StoreError::Database`] if the query fails.
    pub async fn find_block_container_id(
        &self,
        block_id: &str,
    ) -> Result<Option<String>, StoreError> {
        self.with_conn(async |scope| find_container_id(scope, block_id).await)
            .await
    }

    /// Lightweight lookup of the `container_id` owning `block_id` in either `blocks` or
    /// `deleted_blocks`, without reading `body` or `fields`.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::InvalidId`] if `block_id` is invalid, or
    /// [`StoreError::Database`] if the query fails.
    pub async fn find_any_block_container_id(
        &self,
        block_id: &str,
    ) -> Result<Option<String>, StoreError> {
        self.with_conn(async |scope| {
            if let Some(id) = find_container_id(scope, block_id).await? {
                return Ok(Some(id));
            }
            let mut sql = scope.sql("SELECT container_id FROM deleted_blocks WHERE id = ");
            sql.bind(Id(block_id)).push(" LIMIT 1");
            scope
                .fetch_optional(sql)
                .await?
                .map(|row| row.id(0))
                .transpose()
        })
        .await
    }

    /// Bulk lookup of distinct live `container_id`s for `block_ids` without reading
    /// `body` or `fields`.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::InvalidId`] if an identifier is invalid, or
    /// [`StoreError::Database`] if a query fails.
    pub async fn find_block_container_ids(
        &self,
        block_ids: &[String],
    ) -> Result<Vec<String>, StoreError> {
        self.with_conn(async |scope| find_container_ids(scope, block_ids).await)
            .await
    }

    /// Returns the identifiers of all live room-bearing blocks (`card`, `page`, `entry`,
    /// `unit`) in `container_ids` using a single index scan per chunk, without reading
    /// `body`, `fields`, or comment timestamps.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::InvalidId`] if any container identifier is invalid, or
    /// [`StoreError::Database`] if the query fails.
    pub async fn room_block_ids_of_containers(
        &self,
        container_ids: &[String],
    ) -> Result<Vec<String>, StoreError> {
        const CHUNK: usize = 500;
        if container_ids.is_empty() {
            return Ok(Vec::new());
        }
        self.with_conn(async |scope| {
            let mut found = BTreeSet::new();
            for chunk in container_ids.chunks(CHUNK) {
                let mut sql = scope.sql("SELECT id FROM blocks WHERE container_id IN (");
                sql.bind_list(chunk.iter().map(|id| Id(id))).push(") AND ");
                sql.push(&body_versions::with_room_sql("type"));
                for row in scope.fetch_all(sql).await? {
                    found.insert(row.id(0)?);
                }
            }
            Ok(found.into_iter().collect())
        })
        .await
    }
}

/// Starts `SELECT <block columns> FROM blocks`.
pub(crate) fn select_blocks(scope: &Scope<'_>) -> Sql {
    scope.sql([
        "SELECT ",
        BLOCK_COLUMNS,
        ", ",
        EDITED_AT_COLUMN,
        " FROM blocks",
    ])
}

pub(crate) async fn fetch_blocks(
    scope: &mut Scope<'_>,
    sql: Sql,
) -> Result<Vec<Block>, StoreError> {
    scope
        .fetch_all(sql)
        .await?
        .iter()
        .map(|row| {
            let mut block = block_from_row(row)?;
            block.edited_at = row.int(EDITED_AT)?;
            Ok(block)
        })
        .collect()
}

/// Reads a [`BlockType`] column written by the store, rejecting unknown values as
/// [`StoreError::Corrupt`].
pub(crate) fn block_type_column(
    row: &Row,
    index: usize,
    column: &'static str,
) -> Result<BlockType, StoreError> {
    let raw = row.text(index)?;
    raw.parse().map_err(|_| StoreError::Corrupt {
        column,
        reason: format!("unknown block type {raw:?}"),
    })
}

/// The block of a row read with [`BLOCK_COLUMNS`] or [`LISTED_BLOCK_COLUMNS`]: without
/// [`Block::edited_at`].
fn block_from_row(row: &Row) -> Result<Block, StoreError> {
    Ok(Block {
        id: row.id(0)?,
        parent_id: row.id(1)?,
        created_by: row.id(2)?,
        modified_by: row.id(3)?,
        block_type: block_type_column(row, 4, "type")?,
        title: row.text(5)?,
        fields: row.json_map(6, "fields")?,
        created_at: row.int(7)?,
        updated_at: row.int(8)?,
        // A block that exists is not deleted.
        deleted_at: None,
        container_id: row.id(9)?,
        number: card_numbers::number_from_row(row, 10)?,
        body: match row.json(11, "body")? {
            Value::Null => None,
            document => Some(Body::from_stored(document)),
        },
        edited_at: 0,
    })
}

/// What a preview shows of the body of a listed row read with [`PREVIEW_COLUMNS`].
fn preview_from_row(row: &Row) -> Result<BodyPreview, StoreError> {
    Ok(BodyPreview {
        text: row.text(LISTED_BODY_TEXT)?,
        first_image: row.opt_text(LISTED_FIRST_IMAGE)?,
    })
}

pub(crate) async fn get_blocks(
    scope: &mut Scope<'_>,
    filter: &BlockFilter,
) -> Result<Vec<Block>, StoreError> {
    let sql = filtered_blocks(scope, filter, &[BLOCK_COLUMNS, ", ", EDITED_AT_COLUMN]);
    fetch_blocks(scope, sql).await
}

/// A count the store wrote, read back.
fn count_column(row: &Row, index: usize, column: &'static str) -> Result<u32, StoreError> {
    u32::try_from(row.int(index)?).map_err(|error| StoreError::Corrupt {
        column,
        reason: error.to_string(),
    })
}

/// The query for the blocks that match `filter`, reading `columns`.
fn filtered_blocks(scope: &Scope<'_>, filter: &BlockFilter, columns: &[&'static str]) -> Sql {
    let mut head = SqlFragment::of(&["SELECT "]);
    for part in columns {
        head.push(part);
    }
    head.push(" FROM blocks WHERE 1 = 1");
    let mut sql = scope.sql(&head);
    if let Some(container_id) = &filter.container_id {
        sql.push_bind(" AND container_id = ", Id(container_id));
    }
    if let Some(parent_id) = &filter.parent_id {
        sql.push_bind(" AND parent_id = ", Id(parent_id));
    }
    if let Some(block_type) = filter.block_type {
        sql.push_bind(" AND type = ", block_type.as_str());
    }
    sql.push(OLDEST_FIRST).push_page(filter.paging);
    sql
}

/// Lightweight lookup of the container ID owning `block_id` without reading `body` or
/// `fields`, used to determine which container lock to acquire before reading the block.
pub(crate) async fn find_container_id(
    scope: &mut Scope<'_>,
    block_id: &str,
) -> Result<Option<String>, StoreError> {
    let mut sql = scope.sql("SELECT container_id FROM blocks WHERE id = ");
    sql.bind(Id(block_id));
    scope
        .fetch_optional(sql)
        .await?
        .map(|row| row.id(0))
        .transpose()
}

/// Bulk lookup of distinct `container_id`s for `block_ids` without reading `body` or
/// `fields`, chunked well below bind-parameter limits.
pub(crate) async fn find_container_ids(
    scope: &mut Scope<'_>,
    block_ids: &[String],
) -> Result<Vec<String>, StoreError> {
    const CHUNK: usize = 500;
    if block_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut found = BTreeSet::new();
    for chunk in block_ids.chunks(CHUNK) {
        let mut sql = scope.sql("SELECT DISTINCT container_id FROM blocks WHERE id IN (");
        sql.bind_list(chunk.iter().map(|id| Id(id))).push(")");
        for row in scope.fetch_all(sql).await? {
            found.insert(row.id(0)?);
        }
    }
    Ok(found.into_iter().collect())
}

pub(crate) async fn find_block(
    scope: &mut Scope<'_>,
    block_id: &str,
) -> Result<Option<Block>, StoreError> {
    let mut sql = select_blocks(scope);
    sql.push_bind(" WHERE id = ", Id(block_id));
    Ok(fetch_blocks(scope, sql).await?.into_iter().next())
}

pub(crate) async fn get_block(scope: &mut Scope<'_>, block_id: &str) -> Result<Block, StoreError> {
    find_block(scope, block_id)
        .await?
        .ok_or_else(|| StoreError::not_found(format_args!("block ID={block_id}")))
}

/// Whether the block `block_id` is `ancestor_id` or below it, at any depth.
async fn is_below(
    scope: &mut Scope<'_>,
    block_id: &str,
    ancestor_id: &str,
) -> Result<bool, StoreError> {
    // `UNION` and not `UNION ALL`: it also ends the walk if parents ever form a cycle.
    let mut sql = scope
        .sql("WITH RECURSIVE up(id, parent_id) AS (SELECT id, parent_id FROM blocks WHERE id = ");
    sql.bind(Id(block_id))
        .push(
            " UNION SELECT b.id, b.parent_id FROM blocks AS b JOIN up ON b.id = up.parent_id) \
             SELECT 1 FROM up WHERE id = ",
        )
        .bind(Id(ancestor_id));
    Ok(scope.fetch_optional(sql).await?.is_some())
}

pub(crate) async fn insert_block(
    scope: &mut Scope<'_>,
    block: &mut Block,
    user_id: &str,
) -> Result<(), StoreError> {
    let how = Written {
        new_files: NewBodyFiles::Recorded,
        body_write: BodyWrite::Whole,
        telling: Telling::of(scope),
    };
    let named = insert_block_at(scope, block, user_id, None, how).await?;
    card_values::check_named_cards(scope, &named).await
}

/// Info about the parent of `block`, `None` for a block directly on its container.
/// Asked before the block is written, so that a parent that is none, or another
/// board's, is the request's mistake and not a constraint the database reports.
async fn info_of_parent(
    scope: &mut Scope<'_>,
    block: &Block,
) -> Result<Option<ParentInfo>, StoreError> {
    if block.parent_id.is_empty() {
        return Ok(None);
    }
    if block.parent_id == block.id {
        let source = if block.block_type == BlockType::Card {
            BlockError::ParentBelowBlock
        } else {
            BlockError::ParentNotOnContainer
        };
        return Err(StoreError::InvalidBlock {
            id: block.id.clone(),
            source,
        });
    }
    let found = card_hierarchy::parent_on_container(scope, &block.parent_id, &block.container_id);
    found
        .await?
        .ok_or_else(|| StoreError::InvalidBlock {
            id: block.id.clone(),
            source: BlockError::ParentNotOnContainer,
        })
        .map(Some)
}

/// How a block is written, beyond what it is and by whom.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Written<'a> {
    /// How the files its body names newly are asked for.
    pub(crate) new_files: NewBodyFiles,
    /// What the write is for the versions kept of a card's body.
    pub(crate) body_write: BodyWrite<'a>,
    /// Who is told of whom it newly names.
    pub(crate) telling: Telling,
}

/// Stores a block as created or changed at `at`, or now, once its board is held, if
/// that is `None`. The files its body names newly are asked, the versions of a card's
/// body kept, and the people it newly names told as `how` says. Returns the cards its
/// relation values newly name, which the caller asks for once the write is stored
/// ([`card_values::check_named_cards`]).
pub(crate) async fn insert_block_at(
    scope: &mut Scope<'_>,
    block: &mut Block,
    user_id: &str,
    at: Option<i64>,
    how: Written<'_>,
) -> Result<Vec<NamedCards>, StoreError> {
    insert_block_inner(scope, block, user_id, at, how, None).await
}

async fn insert_block_inner(
    scope: &mut Scope<'_>,
    block: &mut Block,
    user_id: &str,
    at: Option<i64>,
    how: Written<'_>,
    preloaded: Option<(ContainerKind, Block)>,
) -> Result<Vec<NamedCards>, StoreError> {
    let invalid = |block: &Block, source| StoreError::InvalidBlock {
        id: block.id.clone(),
        source,
    };
    block.validate().map_err(|source| invalid(block, source))?;
    let (kind, existing) = if let Some((kind, existing)) = preloaded {
        (kind, Some(existing))
    } else {
        // One writer per board at a time, from before anything is read: the parent
        // that is found below is still there when the block is stored.
        let Some(kind) = containers::lock_container_of_kind(scope, &block.container_id).await?
        else {
            return Err(StoreError::not_found(format_args!(
                "board ID={} of block {}",
                block.container_id, block.id
            )));
        };
        let existing = find_block(scope, &block.id).await?;
        (kind, existing)
    };
    let parent = info_of_parent(scope, block).await?;
    if let Some(before) = &existing
        && block.block_type != before.block_type
    {
        return Err(invalid(block, BlockError::TypeChanges));
    }
    let parent_type = parent.as_ref().map(|info| &info.block_type);
    let now = at.unwrap_or_else(|| scope.now_millis());
    block.updated_at = now;
    block.deleted_at = None;
    user_id.clone_into(&mut block.modified_by);

    page_places::check(block, kind, parent_type)?;
    comment_places::check(block, parent_type, existing.as_ref())?;
    comment_places::sign(block, existing.as_ref(), scope.agent());
    // A block that moves must not end up below itself: checked before card hierarchy
    // rules so moving a card onto a block below it always reports `ParentBelowBlock`.
    if let Some(before) = &existing
        && !block.parent_id.is_empty()
        && block.parent_id != before.parent_id
        && is_below(scope, &block.parent_id, &block.id).await?
    {
        return Err(invalid(block, BlockError::ParentBelowBlock));
    }
    card_hierarchy::check_write(scope, block, kind, parent.as_ref(), existing.as_ref()).await?;
    let named = card_values::check_written(scope, block, existing.as_ref()).await?;
    card_numbers::number_written(scope, block, existing.as_ref()).await?;
    if let Some(existing) = existing {
        write::update_existing_block_row(scope, &existing, block, user_id, now, how).await?;
    } else {
        user_id.clone_into(&mut block.created_by);
        block.created_at = now;
        block.edited_at = 0;
        crate::diaries::stamp_entry_body_time(block, None);
        insert_block_row(scope, block, how.new_files).await?;
        notification_noting::note_named(scope, block, None, user_id, how.telling).await?;
    }
    Ok(named)
}

mod write;
use write::{BodyColumns, push_body};
pub(crate) use write::{insert_block_row, insert_blocks, parents_first, patch_block};

#[cfg(test)]
mod tests;
