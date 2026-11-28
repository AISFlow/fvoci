//! Card hierarchy rules on boards: parent validity, acyclicity, maximum depth,
//! template card constraints, and trash restore validation.
//!
//! Every write of a block passes [`check_write`] under the board's lock, batch inserts
//! pass [`check_batch_cycles`], and restores from the trash pass [`check_restore`].

use std::collections::{HashMap, HashSet};

use fvoci_model::{Block, BlockError, BlockType, ContainerKind, MAX_CARD_DEPTH};
use serde_json::Value;

use crate::blocks;
use crate::comment_places;
use crate::db::{Id, Scope};
use crate::error::StoreError;
use crate::trash::block_not_in_trash;

/// What a write needs to know about a block's parent on the same container.
#[derive(Debug, Clone)]
pub(crate) struct ParentInfo {
    /// The parent block's type.
    pub(crate) block_type: BlockType,
    /// Whether the parent block has `isTemplate == true`.
    pub(crate) is_template: bool,
}

/// Reads the type and template flag of a block on `container_id`, if present.
pub(crate) async fn parent_on_container(
    scope: &mut Scope<'_>,
    block_id: &str,
    container_id: &str,
) -> Result<Option<ParentInfo>, StoreError> {
    let mut sql = scope.sql("SELECT type, fields FROM blocks WHERE id = ");
    sql.bind(Id(block_id))
        .push_bind(" AND container_id = ", Id(container_id));
    match scope.fetch_optional(sql).await? {
        Some(row) => {
            let block_type = crate::blocks::block_type_column(&row, 0, "type")?;
            let fields = row.json_map(1, "fields")?;
            let is_template = fields.get("isTemplate") == Some(&Value::Bool(true));
            Ok(Some(ParentInfo {
                block_type,
                is_template,
            }))
        }
        None => Ok(None),
    }
}

/// Rejects card parent cycles within a single batch of blocks before any row is written.
pub(crate) fn check_batch_cycles(blocks: &[Block]) -> Result<(), StoreError> {
    let card_parents: HashMap<&str, &str> = blocks
        .iter()
        .filter(|block| block.block_type == BlockType::Card && !block.id.is_empty())
        .map(|block| (block.id.as_str(), block.parent_id.as_str()))
        .collect();
    for block in blocks {
        if block.block_type != BlockType::Card || block.parent_id.is_empty() {
            continue;
        }
        let mut visited: HashSet<&str> = HashSet::from([block.id.as_str()]);
        let mut cursor = block.parent_id.as_str();
        while !cursor.is_empty() {
            if !visited.insert(cursor) {
                return Err(StoreError::InvalidBlock {
                    id: block.id.clone(),
                    source: BlockError::ParentBelowBlock,
                });
            }
            let Some(&parent) = card_parents.get(cursor) else {
                break;
            };
            cursor = parent;
        }
    }
    Ok(())
}

/// Enforces board card hierarchy rules under the board lock.
///
/// Ordinary cards on a board may sit at depth 1 (no parent), depth 2, or depth 3, always
/// below another live, non-template card of the same board. Template cards cannot have a
/// parent or child cards.
pub(crate) async fn check_write(
    scope: &mut Scope<'_>,
    block: &Block,
    kind: ContainerKind,
    parent: Option<&ParentInfo>,
    existing: Option<&Block>,
) -> Result<(), StoreError> {
    if kind != ContainerKind::Board || block.block_type != BlockType::Card {
        return Ok(());
    }
    let invalid = |source| StoreError::InvalidBlock {
        id: block.id.clone(),
        source,
    };
    if block.is_template() {
        if !block.parent_id.is_empty() {
            return Err(invalid(BlockError::TemplateCardBelowBlock));
        }
        if existing.is_some() && has_child_cards(scope, &block.id).await? {
            return Err(invalid(BlockError::TemplateCardHasChildren));
        }
        return Ok(());
    }
    if block.parent_id.is_empty() {
        return Ok(());
    }
    let Some(parent) = parent else {
        return Err(invalid(BlockError::ParentNotOnContainer));
    };
    if parent.block_type != BlockType::Card {
        return Err(invalid(BlockError::CardNotBelowCard));
    }
    if parent.is_template {
        return Err(invalid(BlockError::CardBelowTemplate));
    }
    let unchanged = existing.is_some_and(|before| {
        before.block_type == BlockType::Card
            && !before.is_template()
            && before.parent_id == block.parent_id
    });
    if unchanged {
        return Ok(());
    }
    let parent_depth = card_depth(scope, &block.container_id, &block.parent_id).await?;
    let subtree_height = if existing.is_some() {
        card_subtree_height(scope, &block.id).await?
    } else {
        1
    };
    if parent_depth + subtree_height > max_depth() {
        return Err(invalid(BlockError::CardTooDeep {
            limit: MAX_CARD_DEPTH,
        }));
    }
    Ok(())
}

/// Validates a deletion family before restoring it from the trash.
///
/// Returns the identifiers of top-level cards in `family` whose parent card is no longer
/// live and should therefore be re-rooted to the top level (`parent_id == ""`) on restore,
/// matching wiki page restore semantics.
pub(crate) async fn check_restore(
    scope: &mut Scope<'_>,
    family: &[Block],
) -> Result<HashSet<String>, StoreError> {
    let coming_back: HashSet<&str> = family.iter().map(|block| block.id.as_str()).collect();
    let mut rerooted = HashSet::new();
    for block in family {
        if coming_back.contains(block.parent_id.as_str()) {
            continue;
        }
        if block.block_type == BlockType::Card {
            check_card_restore_top(scope, block, family, &mut rerooted).await?;
            continue;
        }
        let parent = if block.parent_id.is_empty() {
            None
        } else {
            let Some(parent) = blocks::find_block(scope, &block.parent_id).await? else {
                return Err(block_not_in_trash());
            };
            Some(parent)
        };
        let parent_type = parent.as_ref().map(|parent| parent.block_type);
        if let Some(source) = comment_places::restore_refusal(block.block_type, parent_type) {
            return Err(StoreError::NotRestorable {
                id: block.id.clone(),
                source,
            });
        }
    }
    Ok(rerooted)
}

/// Validates a single card at the top of a restored deletion family.
async fn check_card_restore_top(
    scope: &mut Scope<'_>,
    block: &Block,
    family: &[Block],
    rerooted: &mut HashSet<String>,
) -> Result<(), StoreError> {
    if block.parent_id.is_empty() {
        return Ok(());
    }
    let not_restorable = |source| StoreError::NotRestorable {
        id: block.id.clone(),
        source,
    };
    let Some(parent) = blocks::find_block(scope, &block.parent_id).await? else {
        // Parent card is gone (in the trash or purged): restore at top level.
        rerooted.insert(block.id.clone());
        return Ok(());
    };
    if block.is_template() {
        return Err(not_restorable(BlockError::TemplateCardBelowBlock));
    }
    if parent.container_id != block.container_id {
        return Err(not_restorable(BlockError::ParentNotOnContainer));
    }
    if parent.block_type == BlockType::Comment {
        return Err(not_restorable(BlockError::ParentIsComment));
    }
    if parent.block_type == BlockType::View {
        return Err(not_restorable(BlockError::ParentIsView));
    }
    if parent.block_type == BlockType::Attachment {
        return Err(not_restorable(BlockError::ParentIsAttachment));
    }
    if parent.block_type != BlockType::Card {
        return Err(not_restorable(BlockError::CardNotBelowCard));
    }
    if parent.is_template() {
        return Err(not_restorable(BlockError::CardBelowTemplate));
    }
    let parent_depth = card_depth(scope, &block.container_id, &parent.id).await?;
    let height = family_card_height(family, &block.id);
    if parent_depth + height > max_depth() {
        return Err(not_restorable(BlockError::CardTooDeep {
            limit: MAX_CARD_DEPTH,
        }));
    }
    Ok(())
}

/// Whether `block_id` currently has at least one direct child card.
async fn has_child_cards(scope: &mut Scope<'_>, block_id: &str) -> Result<bool, StoreError> {
    let mut sql = scope.sql("SELECT 1 FROM blocks WHERE parent_id = ");
    sql.bind(Id(block_id)).push(" AND type = 'card' LIMIT 1");
    scope.exists(sql).await
}

/// The 1-based depth of a live card on its board, bounded and cycle-safe.
async fn card_depth(
    scope: &mut Scope<'_>,
    container_id: &str,
    card_id: &str,
) -> Result<usize, StoreError> {
    let limit = max_depth().saturating_add(1);
    let mut depth = 0usize;
    let mut current = card_id.to_owned();
    let mut seen: HashSet<String> = HashSet::new();
    while !current.is_empty() && depth < limit && seen.insert(current.clone()) {
        let mut sql = scope.sql("SELECT parent_id FROM blocks WHERE id = ");
        sql.bind(Id(&current))
            .push_bind(" AND container_id = ", Id(container_id))
            .push(" AND type = 'card'");
        let Some(row) = scope.fetch_optional(sql).await? else {
            break;
        };
        depth += 1;
        current = row.id(0)?;
    }
    Ok(depth)
}

/// The height of the card subtree rooted at `card_id` (1 for a leaf card), counting only
/// child blocks of type `card` so comments and attachments do not count toward card depth.
///
/// Uses a bounded, cycle-safe `WITH RECURSIVE ... UNION` query with fixed parameters so
/// cards with many direct sub-cards never hit database bind-parameter limits.
async fn card_subtree_height(scope: &mut Scope<'_>, card_id: &str) -> Result<usize, StoreError> {
    let limit = i64::from(MAX_CARD_DEPTH).saturating_add(1);
    let mut sql = scope.sql(
        "WITH RECURSIVE down(id, depth) AS (\
             SELECT id, CAST(1 AS BIGINT) FROM blocks WHERE id = ",
    );
    sql.bind(Id(card_id))
        .push(
            " UNION \
             SELECT b.id, down.depth + 1 FROM blocks AS b \
             JOIN down ON b.parent_id = down.id \
             WHERE b.type = 'card' AND down.depth < ",
        )
        .bind(limit)
        .push(") SELECT CAST(COALESCE(MAX(depth), 1) AS BIGINT) FROM down");
    let height = match scope.fetch_optional(sql).await? {
        Some(row) => row.int(0)?,
        None => 1,
    };
    Ok(usize::try_from(height).unwrap_or(1))
}

/// The height of the card subtree rooted at `root_id` within an in-memory deletion family.
fn family_card_height(family: &[Block], root_id: &str) -> usize {
    let mut children: HashMap<&str, Vec<&str>> = HashMap::new();
    for block in family {
        if block.block_type == BlockType::Card && !block.parent_id.is_empty() {
            children
                .entry(block.parent_id.as_str())
                .or_default()
                .push(block.id.as_str());
        }
    }
    let mut height = 1usize;
    let mut seen: HashSet<&str> = HashSet::from([root_id]);
    let mut frontier: Vec<&str> = vec![root_id];
    while !frontier.is_empty() {
        let mut next = Vec::new();
        for id in frontier {
            for &child in children.get(id).into_iter().flatten() {
                if seen.insert(child) {
                    next.push(child);
                }
            }
        }
        if next.is_empty() {
            break;
        }
        height += 1;
        frontier = next;
    }
    height
}

fn max_depth() -> usize {
    usize::try_from(MAX_CARD_DEPTH).unwrap_or(usize::MAX)
}
