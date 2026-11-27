//! Copying a block of a board with what is below it.

use std::collections::HashSet;

use fvoci_model::{Block, BlockType, DuplicateAs};
use serde_json::Value;

use crate::blocks::{BLOCK_COLUMNS, OLDEST_FIRST, fetch_blocks, insert_blocks};
use crate::comment_versions::EDITED_AT_COLUMN;
use crate::db::Id;
use crate::file_refs::NewBodyFiles;
use crate::notification_noting::Telling;
use crate::trash::with_tree_of;
use crate::{Store, StoreError, card_values, containers};

/// Whether [`reachable_descendants`] includes nested card subtrees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Subcards {
    Include,
    Omit,
}

/// Keeps direct non-comment children of `root_id` and (when `subcards` is
/// [`Subcards::Include`]) recursively descends through nested [`BlockType::Card`]
/// descendants and their direct non-comment children, without descending through non-card
/// blocks.
fn reachable_descendants(root_id: &str, candidates: Vec<Block>, subcards: Subcards) -> Vec<Block> {
    let include = subcards == Subcards::Include;
    let mut allowed: HashSet<String> = HashSet::from([root_id.to_owned()]);
    let mut remaining: Vec<Block> = candidates
        .into_iter()
        .filter(|block| include || block.block_type != BlockType::Card)
        .collect();
    let mut kept = Vec::with_capacity(remaining.len());
    loop {
        let mut next = Vec::new();
        let before = kept.len();
        for block in remaining {
            if allowed.contains(&block.parent_id) {
                if include && block.block_type == BlockType::Card {
                    allowed.insert(block.id.clone());
                }
                kept.push(block);
            } else {
                next.push(block);
            }
        }
        if kept.len() == before {
            return kept;
        }
        remaining = next;
    }
}

impl Store {
    /// Copies a block of a board and its descendants, except comments, under new
    /// identifiers. Returns the copies, the copy of the block itself first.
    ///
    /// The copy of the block gets its `isTemplate` field set according to `duplicate_as`.
    /// An ordinary copy of a card keeps its `parent_id` (becoming a sibling under the same
    /// parent card) and copies its nested sub-cards and non-comment attachments. A
    /// template copy of a card clears its `parent_id` (top-level template) and omits
    /// descendant cards (and their attachments), since card templates neither belong to a
    /// parent nor hold sub-cards.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if the board has no such block.
    pub async fn duplicate_block(
        &self,
        container_id: &str,
        block_id: &str,
        user_id: &str,
        duplicate_as: DuplicateAs,
    ) -> Result<Vec<Block>, StoreError> {
        self.transaction(async |scope| {
            let not_found = || {
                StoreError::not_found(format_args!(
                    "block subtree BoardID={container_id} BlockID={block_id}"
                ))
            };
            if !containers::lock_container(scope, container_id).await? {
                return Err(not_found());
            }
            let mut sql = with_tree_of(scope, block_id);
            sql.push([
                "SELECT ",
                BLOCK_COLUMNS,
                ", ",
                EDITED_AT_COLUMN,
                " FROM blocks WHERE id IN (SELECT id FROM tree) AND container_id = ",
            ])
            .bind(Id(container_id))
            .push(OLDEST_FIRST);
            let family = fetch_blocks(scope, sql).await?;

            let (roots, others): (Vec<Block>, Vec<Block>) = family
                .into_iter()
                .filter(|block| block.block_type != BlockType::Comment)
                .partition(|block| block.id == block_id);
            let mut root = roots.into_iter().next().ok_or_else(not_found)?;
            let as_template = duplicate_as == DuplicateAs::Template;
            root.fields
                .insert("isTemplate".to_owned(), Value::Bool(as_template));
            let subcards = if as_template && root.block_type == BlockType::Card {
                root.parent_id.clear();
                Subcards::Omit
            } else {
                Subcards::Include
            };
            let children = reachable_descendants(block_id, others, subcards);

            let mut copies = Vec::with_capacity(children.len() + 1);
            copies.push(root);
            copies.extend(children);
            // What the source holds is settled before the copies are named, so that a
            // value that named its own card or a card gone from the board is dropped
            // first; `renew_block_ids` then remaps relations within the copied subtree
            // while keeping live external cards on the board.
            let definitions = containers::find_container(scope, container_id)
                .await?
                .map(|container| container.card_properties)
                .unwrap_or_default();
            card_values::clear_copied_values(&mut copies, &definitions);
            card_values::keep_cards_of_board(scope, &mut copies, container_id, &definitions)
                .await?;
            fvoci_model::renew_block_ids(
                &mut copies,
                &fvoci_model::RelationProperties::of_board(container_id, &definitions),
                fvoci_model::UnmatchedRelation::Kept,
            );
            // A copy names whom its original named: nobody is told again.
            let nobody = Telling::Nobody;
            insert_blocks(scope, &mut copies, user_id, NewBodyFiles::Unchecked, nobody).await?;
            Ok(copies)
        })
        .await
    }
}
