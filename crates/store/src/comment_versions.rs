//! The texts of comments: what a comment's body said before it was changed.
//!
//! A comment whose body is changed gets a row for each version of its body, from the one
//! it was posted with to the one it has now (`comment_versions`). The first version and
//! the newest are kept, up to [`Store::set_comment_versions_kept`] versions before the
//! one it has now. A comment in the trash keeps its rows; they go when the comment is
//! gone for good. The files the kept versions show are not collected
//! ([`crate::file_refs`]).

use std::sync::atomic::Ordering;

use fvoci_model::{Block, BlockType, Body, CommentVersion};
use serde_json::Value;

use crate::Store;
use crate::db::{Id, OptId, Param, Scope, SqlFragment, to_json_text};
use crate::error::StoreError;

/// How many earlier versions of a comment the store keeps unless told otherwise.
pub const DEFAULT_COMMENT_VERSIONS_KEPT: u32 = 50;

impl Store {
    /// Sets how many earlier versions of a comment are kept: the first one, with which
    /// the comment was posted, and the newest. At least one, the first, is kept. Counts
    /// for the changes that come after.
    pub fn set_comment_versions_kept(&self, kept: u32) {
        let kept = usize::try_from(kept.max(1)).unwrap_or(usize::MAX);
        self.inner
            .comment_versions_kept
            .store(kept, Ordering::Relaxed);
    }

    /// Returns the earlier versions of a comment's body, the first first: all but the
    /// one it has now. Empty for a comment whose body was never changed.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn get_comment_versions(
        &self,
        block_id: &str,
    ) -> Result<Vec<CommentVersion>, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = scope.sql(
                "SELECT version, body, written_by, written_at FROM comment_versions \
                 WHERE block_id = ",
            );
            sql.bind(Id(block_id))
                .push(
                    " AND version < (SELECT MAX(version) FROM comment_versions \
                     WHERE block_id = ",
                )
                .bind(Id(block_id))
                .push(") ORDER BY version");
            scope
                .fetch_all(sql)
                .await?
                .iter()
                .map(|row| {
                    Ok(CommentVersion {
                        version: row.int(0)?,
                        body: match row.json(1, "body")? {
                            Value::Null => None,
                            document => Some(Body::from_stored(document)),
                        },
                        written_by: row.id(2)?,
                        written_at: row.int(3)?,
                    })
                })
                .collect()
        })
        .await
    }

    /// See [`Store::set_comment_versions_kept`].
    pub(crate) fn comment_versions_kept(&self) -> usize {
        self.inner.comment_versions_kept.load(Ordering::Relaxed)
    }
}

/// Writes down a change of a comment's body: `before` is the block as it was stored,
/// `after` as it is stored now, a comment, changed by `user_id`. Nothing for a block that
/// is no comment now, or whose body stays as it was. The first change also writes down
/// the version the comment was posted with, by its author at its creation. A block that
/// was no comment for a while (an editor may make a card of a comment and a comment
/// again) may have a body that its newest version does not have: that body is written
/// down first, by who changed the block last, when they did.
///
/// Returns whether anything was written: the files the block shows are then to be put
/// down again.
pub(crate) async fn record_change(
    scope: &mut Scope<'_>,
    before: &Block,
    after: &Block,
    user_id: &str,
) -> Result<bool, StoreError> {
    if after.block_type != BlockType::Comment || before.body == after.body {
        return Ok(false);
    }
    let mut sql = scope.sql("SELECT version, body FROM comment_versions WHERE block_id = ");
    sql.bind(Id(&after.id))
        .push(" ORDER BY version DESC LIMIT 1");
    let newest = match scope.fetch_optional(sql).await? {
        Some(row) => {
            let body = match row.json(1, "body")? {
                Value::Null => None,
                document => Some(Body::from_stored(document)),
            };
            Some((row.int(0)?, body))
        }
        None => None,
    };
    let mut version = match newest {
        None => {
            insert(scope, before, 1, &before.created_by, before.created_at).await?;
            1
        }
        Some((version, body)) if body != before.body => {
            insert(
                scope,
                before,
                version + 1,
                &before.modified_by,
                before.updated_at,
            )
            .await?;
            version + 1
        }
        Some((version, _)) => version,
    };
    version += 1;
    insert(scope, after, version, user_id, after.updated_at).await?;

    // The one it has now and the newest earlier ones, and the first.
    let kept = i64::try_from(scope.comment_versions_kept()).unwrap_or(i64::MAX);
    let mut sql = scope.sql("DELETE FROM comment_versions WHERE block_id = ");
    sql.bind(Id(&after.id))
        .push(" AND version > (SELECT MIN(version) FROM comment_versions WHERE block_id = ")
        .bind(Id(&after.id))
        .push_bind(") AND version <= ", version.saturating_sub(kept));
    scope.execute(sql).await?;
    Ok(true)
}

/// Inserts the version of a comment that its body has in `block`.
async fn insert(
    scope: &mut Scope<'_>,
    block: &Block,
    version: i64,
    written_by: &str,
    written_at: i64,
) -> Result<(), StoreError> {
    let files: Vec<&str> = block.body.as_ref().map(Body::file_ids).unwrap_or_default();
    let files = to_json_text(&files)?;
    let mut sql = scope.sql(
        "INSERT INTO comment_versions (block_id, version, body, files, written_by, written_at) \
         VALUES (",
    );
    sql.bind(Id(&block.id))
        .push_bind(", ", version)
        .push_bind(
            ", ",
            Param::opt_jsonb(block.body.as_ref().map(Body::document))?,
        )
        .push_bind(", ", files)
        .push_bind(", ", OptId(written_by))
        .push_bind(", ", written_at)
        .push(")");
    scope.execute(sql).await?;
    Ok(())
}

/// The stored names of the files that the kept versions of a comment show.
pub(crate) async fn files_of(
    scope: &mut Scope<'_>,
    block_id: &str,
) -> Result<Vec<String>, StoreError> {
    let mut sql = scope.sql("SELECT files FROM comment_versions WHERE block_id = ");
    sql.bind(Id(block_id));
    let mut names = Vec::new();
    for row in scope.fetch_all(sql).await? {
        let files: Vec<String> =
            serde_json::from_str(&row.text(0)?).map_err(|error| StoreError::Corrupt {
                column: "files",
                reason: error.to_string(),
            })?;
        names.extend(files);
    }
    Ok(names)
}

/// Forgets the versions of the comments that are gone for good: neither on a board nor
/// in the trash of the organization of the version.
pub(crate) async fn forget_gone_comments(scope: &mut Scope<'_>) -> Result<(), StoreError> {
    let mut statement = SqlFragment::of(&["DELETE FROM comment_versions WHERE NOT EXISTS \
         (SELECT 1 FROM blocks b WHERE b.id = comment_versions.block_id"]);
    statement
        .push_fragment(&scope.same_organization(&[("b", "comment_versions")]))
        .push(") AND NOT EXISTS (SELECT 1 FROM deleted_blocks d WHERE d.id = comment_versions.block_id")
        .push_fragment(&scope.same_organization(&[("d", "comment_versions")]))
        .push(")");
    scope.execute(scope.sql(&statement)).await?;
    Ok(())
}

/// The column that a block read with [`crate::blocks::BLOCK_COLUMNS`] has after them:
/// when its body was last changed, if it is a comment that has versions. A block that
/// was a comment and is no longer one keeps its versions, and has none of this.
pub(crate) const EDITED_AT_COLUMN: &str = "CASE WHEN blocks.type = 'comment' THEN \
     (SELECT MAX(v.written_at) FROM comment_versions v WHERE v.block_id = blocks.id) END";
