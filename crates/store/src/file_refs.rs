//! Which block shows which file of its board, and since when a file is shown by none.
//!
//! Every write of a block puts down the files it shows. A block in the trash keeps them:
//! undoing and restoring stay safe for the grace period. A file that no block shows,
//! live or in the trash, is marked with the time it became so, and a file marked longer
//! ago than the grace period is removed by [`Store::collect_unreferenced_files`], and
//! its object doomed.
//!
//! A write that makes a block's body name a file newly (an image, a file block) asks, in
//! its transaction, that the board has the file, and holds its record until it commits
//! ([`NewBodyFiles::Recorded`]): what a request checked before the write began is not
//! collected under it.

use std::collections::BTreeSet;

use fvoci_model::{Block, BlockError, BlockType, FileRecord};

use crate::Store;
use crate::block_derived::Refreshing;
use crate::body_versions;
use crate::comment_versions;
use crate::db::{Dialect, Id, Scope, Sql, SqlFragment};
use crate::error::StoreError;
use crate::files::{FILE_COLUMN_COUNT, FILE_COLUMNS, file_from_row};
use crate::loose_objects;

/// Bounded parameters even when a purge visits every stored file.
const MARK_BATCH: usize = 300;

impl Store {
    /// Removes the records of the files that no block has shown since before `before`
    /// (milliseconds), dooms their objects in the same transaction
    /// ([`Store::due_doomed_objects`]), and returns the records.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a statement fails; nothing is removed then.
    pub async fn collect_unreferenced_files(
        &self,
        before: i64,
    ) -> Result<Vec<FileRecord>, StoreError> {
        self.transaction(async |scope| {
            // One statement decides and deletes. On PostgreSQL a record that a write
            // holds (`hold_new_body_files`) or changes is skipped, not waited for: left to
            // the next run, and no two transactions wait for each other's records. A
            // record whose mark a write took away since the statement began is looked
            // at again as committed, and kept. A purge can have marked a record from
            // an older snapshot while a new reference committed: ask the references
            // again here, in the transaction that would doom the object.
            let org = loose_objects::org_column(scope);
            let skip_held = match scope.dialect() {
                Dialect::Postgres => " FOR UPDATE SKIP LOCKED",
                Dialect::Sqlite => "",
            };
            let same_org = scope.same_organization(&[("r", "files")]);
            let mut sql = scope.sql(
                "DELETE FROM files WHERE id IN (SELECT id FROM files \
                 WHERE unreferenced_at IS NOT NULL AND unreferenced_at < ",
            );
            sql.bind(before)
                .push(
                    " AND NOT EXISTS (SELECT 1 FROM file_refs r \
                       WHERE r.container_id = files.container_id AND r.name = files.name",
                )
                .push(&same_org)
                .push([")", skip_held, ") RETURNING ", FILE_COLUMNS]);
            if let Some(column) = org {
                sql.push([", ", column]);
            }
            let rows = scope.fetch_all(sql).await?;
            let mut files = Vec::with_capacity(rows.len());
            let mut doomed = Vec::with_capacity(rows.len());
            for row in &rows {
                let file = file_from_row(row)?;
                let org_id = org.map(|_| row.id(FILE_COLUMN_COUNT)).transpose()?;
                doomed.push((file.path.clone(), org_id));
                files.push(file);
            }
            loose_objects::doom(scope, &doomed).await?;
            let gone: Vec<&str> = files.iter().map(|file| file.id.as_str()).collect();
            crate::file_versions::forget(scope, &gone).await?;
            Ok(files)
        })
        .await
    }
}

/// What a write of blocks asks of the files that the images and file blocks of their
/// bodies come to name: those a block did not show before the write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewBodyFiles {
    /// Each is a file of the block's board, asked in the write's transaction, and its
    /// record is held until the write ends: the collector of unreferenced files
    /// ([`Store::collect_unreferenced_files`]) skips it, or took the record first and
    /// the write is refused with [`BlockError::BodyFileNotOfContainer`]. For what a client
    /// writes.
    Recorded,
    /// Not asked: for a write that copies blocks and points their bodies at copies of
    /// their files afterwards, as a duplicate and an import do.
    Unchecked,
}

/// Puts down the files a block shows now, in place of those it showed, and marks the
/// files of its board whose state that changes. A comment shows the files of its kept
/// versions too, also while it is made a block of another type, and a card or a page
/// those of the kept versions of its body. What the images and file blocks of
/// the body newly name is asked as `new_files` says. Only
/// [`crate::block_derived::refresh`] makes it, with the rest of what follows from a block.
pub(crate) async fn refresh(
    scope: &mut Scope<'_>,
    block: &Block,
    new_files: NewBodyFiles,
    _: Refreshing,
) -> Result<(), StoreError> {
    let mut sql = scope.sql("DELETE FROM file_refs WHERE block_id = ");
    sql.bind(Id(&block.id)).push(" RETURNING name");
    let mut names = scope
        .fetch_all(sql)
        .await?
        .iter()
        .map(|row| row.text(0))
        .collect::<Result<Vec<_>, _>>()?;
    if new_files == NewBodyFiles::Recorded {
        hold_new_body_files(scope, block, &names).await?;
    }
    // What the block shows, and what the kept versions of a comment or of a room-backed
    // body show. Because `block_type` is immutable after creation, non-comment blocks
    // never query `comment_versions` and blocks without rooms never query `body_versions`.
    let mut shown: BTreeSet<String> = block.shown_files().into_iter().collect();
    if block.block_type == BlockType::Comment {
        shown.extend(comment_versions::files_of(scope, &block.id).await?);
    } else if block.block_type.has_room() {
        shown.extend(body_versions::files_of(scope, &block.id).await?);
    }
    if !shown.is_empty() {
        let mut sql = scope.sql("INSERT INTO file_refs (block_id, container_id, name) VALUES ");
        for (index, name) in shown.iter().enumerate() {
            sql.push(if index == 0 { "(" } else { ", (" })
                .bind(Id(&block.id))
                .push_bind(", ", Id(&block.container_id))
                .push_bind(", ", name.as_str())
                .push(")");
        }
        scope.execute(sql).await?;
        names.extend(shown);
    }
    if names.is_empty() {
        return Ok(());
    }
    names.sort_unstable();
    names.dedup();
    mark(scope, Some((&block.container_id, &names))).await
}

/// Holds the records of the files that the images and file blocks of a block's body
/// name and that it did not show before (`before`), until the transaction ends;
/// refuses the block if its board has no record of one. On PostgreSQL the collector skips a record this
/// lock holds ([`Store::collect_unreferenced_files`]), and a record it deleted first is
/// not found here once its transaction ended; on SQLite the transaction is the only
/// writer.
async fn hold_new_body_files(
    scope: &mut Scope<'_>,
    block: &Block,
    before: &[String],
) -> Result<(), StoreError> {
    let Some(body) = &block.body else {
        return Ok(());
    };
    let new: BTreeSet<&str> = body
        .file_ids()
        .into_iter()
        .filter(|name| !before.iter().any(|shown| shown == name))
        .collect();
    if new.is_empty() {
        return Ok(());
    }
    let mut sql = scope.sql("SELECT name FROM files WHERE container_id = ");
    sql.bind(Id(&block.container_id))
        .push(" AND name IN (")
        .bind_list(new.iter().copied())
        .push(")");
    if scope.dialect() == Dialect::Postgres {
        sql.push(" FOR KEY SHARE");
    }
    let recorded = scope
        .fetch_all(sql)
        .await?
        .iter()
        .map(|row| row.text(0))
        .collect::<Result<BTreeSet<_>, _>>()?;
    if new.iter().all(|name| recorded.contains(*name)) {
        Ok(())
    } else {
        Err(StoreError::InvalidBlock {
            id: block.id.clone(),
            source: BlockError::BodyFileNotOfContainer,
        })
    }
}

/// Has a block show the files of `names` too, before a write of its body does: those
/// it does not show yet are asked of its board and held as [`NewBodyFiles::Recorded`]
/// holds them, then put down as shown and their marks taken away. For the files of a
/// body edited together, whose updates are logged a little before the body is written
/// ([`crate::Store::append_body_updates`]); the write puts down what the body shows in
/// place of these.
pub(crate) async fn show_body_files(
    scope: &mut Scope<'_>,
    block_id: &str,
    names: &[String],
) -> Result<(), StoreError> {
    let mut sql = scope.sql("SELECT name FROM file_refs WHERE block_id = ");
    sql.bind(Id(block_id));
    let shown = scope
        .fetch_all(sql)
        .await?
        .iter()
        .map(|row| row.text(0))
        .collect::<Result<BTreeSet<_>, _>>()?;
    let new: Vec<String> = names
        .iter()
        .filter(|name| !shown.contains(*name))
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if new.is_empty() {
        return Ok(());
    }
    let mut sql = scope.sql("SELECT container_id FROM blocks WHERE id = ");
    sql.bind(Id(block_id));
    let container_id = scope
        .fetch_optional(sql)
        .await?
        .ok_or_else(|| StoreError::not_found(format_args!("block ID={block_id}")))?
        .id(0)?;
    let mut sql = scope.sql("SELECT name FROM files WHERE container_id = ");
    sql.bind(Id(&container_id))
        .push(" AND name IN (")
        .bind_list(new.iter().map(String::as_str))
        .push(")");
    if scope.dialect() == Dialect::Postgres {
        sql.push(" FOR KEY SHARE");
    }
    let recorded = scope
        .fetch_all(sql)
        .await?
        .iter()
        .map(|row| row.text(0))
        .collect::<Result<BTreeSet<_>, _>>()?;
    if !new.iter().all(|name| recorded.contains(name)) {
        return Err(StoreError::InvalidBlock {
            id: block_id.to_owned(),
            source: BlockError::BodyFileNotOfContainer,
        });
    }
    for name in &new {
        let mut sql = scope.sql("INSERT INTO file_refs (block_id, container_id, name) VALUES (");
        sql.bind(Id(block_id))
            .push_bind(", ", Id(&container_id))
            .push_bind(", ", name.as_str())
            .push(")");
        scope.execute(sql).await?;
    }
    mark(scope, Some((&container_id, &new))).await
}

/// Forgets what the blocks that are gone for good showed: neither on a board nor in the
/// trash of the organization of the row.
pub(crate) async fn forget_gone_blocks(scope: &mut Scope<'_>) -> Result<(), StoreError> {
    let mut statement = SqlFragment::of(&[
        "DELETE FROM file_refs WHERE NOT EXISTS (SELECT 1 FROM blocks b WHERE b.id = file_refs.block_id",
    ]);
    statement
        .push_fragment(&scope.same_organization(&[("b", "file_refs")]))
        .push(") AND NOT EXISTS (SELECT 1 FROM deleted_blocks d WHERE d.id = file_refs.block_id")
        .push_fragment(&scope.same_organization(&[("d", "file_refs")]))
        .push(")");
    let sql = scope.sql(&statement);
    scope.execute(sql).await?;
    Ok(())
}

/// Marks file records: one that a block of its organization shows has no mark, and one
/// that no such block shows gets the time of now if it had none. Only the records whose
/// state flips are written. `only` narrows it to some stored names of one board; `None`
/// looks at every record. PostgreSQL holds the affected rows before asking references
/// in a fresh statement, including rows whose mark a writer need not change. Otherwise
/// a purge's older snapshot could mark a file while a writer leaves its null mark alone.
pub(crate) async fn mark(
    scope: &mut Scope<'_>,
    only: Option<(&str, &[String])>,
) -> Result<(), StoreError> {
    let mut changing = SqlFragment::of(&["(unreferenced_at IS NULL) = (NOT EXISTS (\
         SELECT 1 FROM file_refs r WHERE r.container_id = files.container_id \
         AND r.name = files.name"]);
    changing
        .push_fragment(&scope.same_organization(&[("r", "files")]))
        .push("))");
    if scope.dialect() == Dialect::Postgres {
        let ids = hold_marks(scope, only, &changing).await?;
        for batch in ids.chunks(MARK_BATCH) {
            let mut sql = mark_statement(scope, &changing);
            sql.push(" AND id IN (")
                .bind_list(batch.iter().map(|id| Id(id)))
                .push(")");
            scope.execute(sql).await?;
        }
    } else {
        // SQLite's transaction is already the only writer.
        let mut sql = mark_statement(scope, &changing);
        narrow_marks(&mut sql, only);
        scope.execute(sql).await?;
    }
    Ok(())
}

fn mark_statement(scope: &Scope<'_>, changing: &SqlFragment) -> Sql {
    let mut sql =
        scope.sql("UPDATE files SET unreferenced_at = CASE WHEN unreferenced_at IS NULL THEN ");
    sql.bind(scope.now_millis())
        .push(" ELSE NULL END WHERE ")
        .push(changing);
    sql
}

fn narrow_marks(sql: &mut Sql, only: Option<(&str, &[String])>) {
    if let Some((container_id, names)) = only {
        sql.push_bind(" AND container_id = ", Id(container_id))
            .push(" AND name IN (")
            .bind_list(names.iter().map(String::as_str))
            .push(")");
    }
}

/// NO KEY UPDATE is compatible with an image writer's earlier KEY SHARE: two
/// writers can wait their turn here without an upgrade deadlock. The purge skips
/// held rows so it never waits for a writer holding another row it already locked.
async fn hold_marks(
    scope: &mut Scope<'_>,
    only: Option<(&str, &[String])>,
    changing: &SqlFragment,
) -> Result<Vec<String>, StoreError> {
    let mut sql = scope.sql("SELECT id FROM files WHERE ");
    // Writers hold every affected row, even if their current snapshot needs no flip.
    if only.is_some() {
        sql.push("TRUE");
    } else {
        sql.push(changing);
    }
    narrow_marks(&mut sql, only);
    sql.push(" ORDER BY id FOR NO KEY UPDATE");
    if only.is_none() {
        sql.push(" SKIP LOCKED");
    }
    scope
        .fetch_all(sql)
        .await?
        .iter()
        .map(|row| row.id(0))
        .collect()
}
