//! The reactions to the blocks in the trash: they go into the trash with the block, in
//! its JSON of `deleted_blocks` (under [`KEY`]), and come back with it. A reaction of
//! one whose account is no longer in use, or who left the organization, a member of it
//! or a guest of its last workspace ([`forget_reactions_of_organization`]), comes back to
//! nothing, even when that is under way as the block comes back
//! ([`KeptReactions::make_again`]).

use std::collections::{BTreeSet, HashMap, HashSet};

use serde_json::Value;

use super::watchers::{forget_kept, trash_of_organization};
use crate::db::{Dialect, Id, Scope};
use crate::error::StoreError;
use crate::users;

/// Where a block of the trash keeps its reactions: no member of a block's own JSON.
const KEY: &str = "reactedWith";

/// How many identifiers one statement binds.
const BATCH: usize = 300;

/// A reaction as the trash keeps it, without its block: its own type, so that what is
/// stored does not change with the model.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct StoredReaction {
    user_id: String,
    emoji: String,
    created_at: i64,
}

/// The reactions to those of `block_ids` that have any, by block, each block's in the
/// order they were made.
pub(super) async fn reactions_of(
    scope: &mut Scope<'_>,
    block_ids: &[&str],
) -> Result<HashMap<String, Vec<StoredReaction>>, StoreError> {
    let mut reactions: HashMap<String, Vec<StoredReaction>> = HashMap::new();
    for batch in block_ids.chunks(BATCH) {
        let mut sql = scope
            .sql("SELECT block_id, user_id, emoji, created_at FROM reactions WHERE block_id IN (");
        sql.bind_list(batch.iter().map(|id| Id(id)))
            .push(") ORDER BY block_id, created_at, user_id, emoji");
        for row in scope.fetch_all(sql).await? {
            reactions
                .entry(row.id(0)?)
                .or_default()
                .push(StoredReaction {
                    user_id: row.id(1)?,
                    emoji: row.text(2)?,
                    created_at: row.int(3)?,
                });
        }
    }
    Ok(reactions)
}

/// Puts `reactions` beside the block in its JSON of the trash, `value`.
pub(super) fn keep(
    value: &mut Value,
    reactions: Option<Vec<StoredReaction>>,
) -> Result<(), StoreError> {
    if let (Some(reactions), Some(members)) = (reactions, value.as_object_mut()) {
        let kept = serde_json::to_value(reactions).map_err(|error| StoreError::Corrupt {
            column: "block",
            reason: error.to_string(),
        })?;
        members.insert(KEY.to_owned(), kept);
    }
    Ok(())
}

/// Takes the reactions kept in a parsed block JSON of the trash: none for one that kept
/// none, as what went into the trash before they were kept.
pub(super) fn take_kept(block: &mut Value) -> Result<Vec<StoredReaction>, StoreError> {
    match block.get_mut(KEY).map(Value::take) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(kept) => serde_json::from_value(kept).map_err(|error| StoreError::Corrupt {
            column: "block",
            reason: error.to_string(),
        }),
    }
}

/// The reactions that blocks taken out of the trash kept, by block, to be made again
/// once those blocks are back ([`KeptReactions::make_again`]).
#[derive(Debug, Default)]
pub(crate) struct KeptReactions(Vec<(String, Vec<StoredReaction>)>);

impl KeptReactions {
    /// Keeps the reactions `block_id` kept in the trash, to make them again.
    pub(super) fn keep(&mut self, block_id: String, reactions: Vec<StoredReaction>) {
        if !reactions.is_empty() {
            self.0.push((block_id, reactions));
        }
    }

    /// Makes again the reactions kept, of those whose accounts are in use and, on
    /// PostgreSQL, still in the block's organization: members of it, or guests of one of
    /// its workspaces, who are in no `organization_members`. The blocks are back among
    /// the blocks. One made since stays as it is.
    ///
    /// Each account is held first, in the order of their identifiers
    /// ([`users::hold_in_use`]): an erasure of one, or a leaving of the organization,
    /// that is under way is waited for, and one that comes later waits for this and
    /// then takes these reactions away. Holding accounts, it comes after every other
    /// lock of the transaction ([`crate::db::LockRank::Users`]).
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a statement fails.
    pub(crate) async fn make_again(self, scope: &mut Scope<'_>) -> Result<(), StoreError> {
        let accounts: BTreeSet<&str> = self
            .0
            .iter()
            .flat_map(|(_, reactions)| reactions.iter().map(|one| one.user_id.as_str()))
            .collect();
        let mut in_use: HashSet<&str> = HashSet::new();
        for user_id in accounts {
            if users::hold_in_use(scope, user_id).await? {
                in_use.insert(user_id);
            }
        }
        let postgres = scope.dialect() == Dialect::Postgres;
        for (block_id, reactions) in &self.0 {
            for reaction in reactions {
                if !in_use.contains(reaction.user_id.as_str()) {
                    continue;
                }
                let mut sql = scope.sql(if postgres {
                    "INSERT INTO reactions (org_id, block_id, user_id, emoji, created_at) \
                     SELECT b.org_id, b.id, "
                } else {
                    "INSERT INTO reactions (block_id, user_id, emoji, created_at) SELECT b.id, "
                });
                sql.bind(Id(&reaction.user_id))
                    .push_bind(", ", reaction.emoji.as_str())
                    .push_bind(", ", reaction.created_at)
                    .push_bind(" FROM blocks AS b WHERE b.id = ", Id(block_id));
                if postgres {
                    sql.push(
                        " AND (EXISTS (SELECT 1 FROM organization_members AS om \
                         WHERE om.org_id = b.org_id AND om.user_id = ",
                    )
                    .bind(Id(&reaction.user_id))
                    .push(
                        ") OR EXISTS (SELECT 1 FROM workspace_members AS wm \
                         JOIN workspaces AS w ON w.id = wm.workspace_id AND wm.org_id = w.org_id \
                         WHERE w.org_id = b.org_id AND wm.user_id = ",
                    )
                    .bind(Id(&reaction.user_id))
                    .push("))");
                }
                sql.push(" ON CONFLICT DO NOTHING");
                scope.execute(sql).await?;
            }
        }
        Ok(())
    }
}

/// Forgets the reactions of `user_id` that the blocks in the trash of the scope's
/// organization keep (of its live and its trashed containers alike).
pub(crate) async fn forget_reactions_of_organization(
    scope: &mut Scope<'_>,
    user_id: &str,
) -> Result<(), StoreError> {
    let sql = trash_of_organization(scope)?;
    forget_kept(scope, sql, KEY, user_id).await
}
