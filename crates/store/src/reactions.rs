//! Reactions: who answered a block with which emoji (`fvoci_model::Reaction`).
//!
//! A reaction is a row of its own beside the block: adding or taking one away writes
//! nothing of the block, so it is not "changed" (`updatedAt`) and whoever is changing it
//! is not told it changed under them. A block that goes into the trash takes its
//! reactions along and brings back those of who is an account still
//! (`trash/reactions.rs`); a block gone for good takes them with it (the schema's
//! keys). Who leaves an organization, or whose account is erased, takes theirs out of
//! it ([`forget_user_in_organization`]).

use std::collections::HashMap;

use fvoci_model::{Emoji, REACTION_KINDS_PER_BLOCK, Reaction};

use crate::Store;
use crate::db::{Dialect, Id, LockRank, RowLock, Scope};
use crate::error::StoreError;
use crate::users;

/// How many identifiers one statement binds.
const BATCH: usize = 300;

/// What became of adding a reaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reacted {
    /// The reaction is there now, as it was not before.
    Added,
    /// The user had answered the block with the emoji already: nothing changed.
    AlreadyThere,
    /// The block has [`REACTION_KINDS_PER_BLOCK`] other emoji already: nothing changed.
    TooManyKinds,
}

impl Store {
    /// Adds the reaction `emoji` of `user_id` to the block `block_id`, made now. The
    /// block is held while its emoji are counted, so two people who add a new emoji at
    /// once are counted one after the other. The user is to be an account in use and in
    /// the workspace of the block: one who left meanwhile adds nothing.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if there is no such block, or the user is no
    /// longer in its workspace; [`StoreError::UserNotFound`] if the user's account is
    /// not in use; and [`StoreError::Database`] if a statement fails.
    pub async fn add_reaction(
        &self,
        block_id: &str,
        user_id: &str,
        emoji: &Emoji,
    ) -> Result<Reacted, StoreError> {
        self.transaction(async |scope| {
            let mut sql = scope.sql("SELECT id FROM blocks WHERE id = ");
            sql.bind(Id(block_id));
            sql.push_lock(scope.row_lock(LockRank::Block, block_id, RowLock::NoKeyUpdate));
            if scope.fetch_optional(sql).await?.is_none() {
                return Err(StoreError::not_found("block"));
            }
            if !users::hold_in_use(scope, user_id).await? {
                return Err(StoreError::UserNotFound(user_id.to_owned()));
            }
            if !is_in_the_workspace_of(scope, block_id, user_id).await? {
                return Err(StoreError::not_found("block"));
            }
            let kinds = emoji_of(scope, block_id).await?;
            if kinds.iter().any(|kind| kind == emoji.as_str()) {
                if has_reacted(scope, block_id, user_id, emoji).await? {
                    return Ok(Reacted::AlreadyThere);
                }
            } else if kinds.len() >= REACTION_KINDS_PER_BLOCK {
                return Ok(Reacted::TooManyKinds);
            }
            let now = scope.now_millis();
            let mut sql = scope.sql(if scope.dialect() == Dialect::Postgres {
                "INSERT INTO reactions (org_id, block_id, user_id, emoji, created_at) \
                 SELECT b.org_id, b.id, "
            } else {
                "INSERT INTO reactions (block_id, user_id, emoji, created_at) SELECT b.id, "
            });
            sql.bind(Id(user_id))
                .push_bind(", ", emoji.as_str())
                .push_bind(", ", now)
                .push_bind(" FROM blocks AS b WHERE b.id = ", Id(block_id))
                .push(" ON CONFLICT DO NOTHING");
            Ok(if scope.execute(sql).await? == 0 {
                Reacted::AlreadyThere
            } else {
                Reacted::Added
            })
        })
        .await
    }

    /// Takes the reaction `emoji` of `user_id` off the block `block_id`. Whether there
    /// was one to take off: one that is not there is no error.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the statement fails.
    pub async fn remove_reaction(
        &self,
        block_id: &str,
        user_id: &str,
        emoji: &Emoji,
    ) -> Result<bool, StoreError> {
        self.with_write_conn(async |scope| {
            let mut sql = scope.sql("DELETE FROM reactions WHERE block_id = ");
            sql.bind(Id(block_id))
                .push_bind(" AND user_id = ", Id(user_id))
                .push_bind(" AND emoji = ", emoji.as_str());
            Ok(scope.execute(sql).await? > 0)
        })
        .await
    }

    /// The reactions to those of `block_ids` that have any, by block. A block's come by
    /// the emoji, the one it was first answered with first; the users of each emoji
    /// come in the order they answered with it. Two at the same moment come by the
    /// identifier of the user, then by the emoji.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Corrupt`] for a stored emoji that is none, and
    /// [`StoreError::Database`] if a statement fails.
    pub async fn reactions_of(
        &self,
        block_ids: &[&str],
    ) -> Result<HashMap<String, Vec<Reaction>>, StoreError> {
        self.with_conn(async |scope| reactions_of(scope, block_ids).await)
            .await
    }
}

/// [`Store::reactions_of`] in `scope`.
async fn reactions_of(
    scope: &mut Scope<'_>,
    block_ids: &[&str],
) -> Result<HashMap<String, Vec<Reaction>>, StoreError> {
    let mut reactions: HashMap<String, Vec<Reaction>> = HashMap::new();
    for batch in block_ids.chunks(BATCH) {
        let mut sql =
            scope.sql("SELECT block_id, emoji, user_id FROM reactions WHERE block_id IN (");
        // The emoji by their code points on both databases, not by PostgreSQL's locale.
        sql.bind_list(batch.iter().map(|id| Id(id)))
            .push(match scope.dialect() {
                Dialect::Postgres => {
                    ") ORDER BY block_id, created_at, user_id, emoji COLLATE \"C\""
                }
                Dialect::Sqlite => ") ORDER BY block_id, created_at, user_id, emoji",
            });
        for row in scope.fetch_all(sql).await? {
            let text = row.text(1)?;
            let emoji: Emoji = text.parse().map_err(|error| StoreError::Corrupt {
                column: "emoji",
                reason: format!("{text:?}: {error}"),
            })?;
            let of_block = reactions.entry(row.id(0)?).or_default();
            let user_id = row.id(2)?;
            match of_block.iter_mut().find(|reaction| reaction.emoji == emoji) {
                Some(reaction) => reaction.user_ids.push(user_id),
                None => of_block.push(Reaction {
                    emoji,
                    user_ids: vec![user_id],
                }),
            }
        }
    }
    Ok(reactions)
}

/// The different emoji the block is answered with.
async fn emoji_of(scope: &mut Scope<'_>, block_id: &str) -> Result<Vec<String>, StoreError> {
    let mut sql = scope.sql("SELECT DISTINCT emoji FROM reactions WHERE block_id = ");
    sql.bind(Id(block_id));
    scope.all(sql).await
}

/// Whether the user answered the block with the emoji.
async fn has_reacted(
    scope: &mut Scope<'_>,
    block_id: &str,
    user_id: &str,
    emoji: &Emoji,
) -> Result<bool, StoreError> {
    let mut sql = scope.sql("SELECT 1 FROM reactions WHERE block_id = ");
    sql.bind(Id(block_id))
        .push_bind(" AND user_id = ", Id(user_id))
        .push_bind(" AND emoji = ", emoji.as_str());
    scope.exists(sql).await
}

/// Whether the user is in the workspace of the block's board.
async fn is_in_the_workspace_of(
    scope: &mut Scope<'_>,
    block_id: &str,
    user_id: &str,
) -> Result<bool, StoreError> {
    let mut sql =
        scope.sql("SELECT 1 FROM blocks AS b JOIN containers AS c ON c.id = b.container_id");
    sql.push(&scope.same_organization(&[("c", "b")]))
        .push(" JOIN workspace_members AS wm ON wm.workspace_id = c.workspace_id")
        .push(&scope.same_organization(&[("wm", "c")]))
        .push_bind(" WHERE b.id = ", Id(block_id))
        .push_bind(" AND wm.user_id = ", Id(user_id));
    scope.exists(sql).await
}

/// Takes the reactions of `user_id` out of the scope's organization: those to its
/// blocks, and those its blocks in the trash keep. The caller holds the user's row
/// (`users::hold_user_for_cleanup`), as [`Store::add_reaction`] waits for it.
pub(crate) async fn forget_user_in_organization(
    scope: &mut Scope<'_>,
    user_id: &str,
) -> Result<(), StoreError> {
    let mut sql = scope.sql("DELETE FROM reactions WHERE user_id = ");
    sql.bind(Id(user_id))
        .push(
            " AND block_id IN (SELECT b.id FROM blocks AS b \
             JOIN containers AS c ON c.id = b.container_id \
             JOIN workspaces AS w ON w.id = c.workspace_id WHERE w.org_id = ",
        )
        .bind(scope.org()?)
        .push(")");
    scope.execute(sql).await?;
    crate::trash::forget_reactions_of_organization(scope, user_id).await
}
