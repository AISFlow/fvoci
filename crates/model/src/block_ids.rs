//! Regenerating the identifiers of a tree of blocks.

use std::collections::{HashMap, HashSet};
use std::hash::BuildHasher;

use serde_json::Value;

use crate::block::{Block, BlockType};
use crate::id::new_id;
use crate::relation::{RELATION_LIMIT, RelationProperties, relation_link_card};

/// What becomes of an identifier in a card relation value that names no card among the
/// blocks whose identifiers are renewed ([`renew_block_ids`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnmatchedRelation {
    /// It stays: a request may name the cards that are on the board already, and the
    /// store checks what it names.
    Kept,
    /// It goes: a copy names the copies of the cards it copies, and no other card.
    Dropped,
}

/// Gives every block a new identifier while keeping references between them intact.
///
/// A reference is another block's identifier appearing as a `container_id`, a `parent_id`,
/// inside the `contentOrder`, `cardOrder` or `defaultTemplateId` fields, or in a
/// reference of the body (a card or a page a body names). References to
/// identifiers outside `blocks` are left as they are, so the tree keeps its shape and
/// stays attached to whatever it pointed at.
///
/// Fields of an unexpected JSON type are left untouched. A block that comes without an
/// identifier gets one like any other, and an empty `parent_id` or `container_id` stays
/// empty. Card relation values are left as they are; [`renew_block_ids`] renames them.
pub fn generate_block_ids(blocks: &mut [Block]) {
    renew_block_ids(blocks, &RelationProperties::new(), UnmatchedRelation::Kept);
}

/// Does what [`generate_block_ids`] does and renames the cards that card relation
/// values name, too: the values of the `relations` of each card's board. An identifier
/// in such a value that is a card among `blocks` gets that card's new identifier; one
/// that is not is kept or dropped as `unmatched` says; kept, an identifier of another
/// block among them is renamed too, so that the store can tell what it names. Dropped,
/// a card of another board among them goes too, and a value keeps at most
/// [`RELATION_LIMIT`] of what is left.
pub fn renew_block_ids(
    blocks: &mut [Block],
    relations: &RelationProperties,
    unmatched: UnmatchedRelation,
) {
    renew_block_ids_told(blocks, relations, unmatched, &|_| false);
}

/// Does what [`renew_block_ids`] does and returns the new identifier of each block
/// that `told` picks by its old one, by that old one. What names such a block from outside `blocks` (a body
/// of another board of the same archive) can then be pointed at its copy.
pub(crate) fn renew_block_ids_told(
    blocks: &mut [Block],
    relations: &RelationProperties,
    unmatched: UnmatchedRelation,
    told: &dyn Fn(&str) -> bool,
) -> HashMap<String, String> {
    // The board of each card: a copy names a copied card of its own board only.
    let card_boards: HashMap<String, String> = blocks
        .iter()
        .filter(|block| block.block_type == BlockType::Card)
        .map(|block| (block.id.clone(), block.container_id.clone()))
        .collect();
    let referenced = referenced_ids(blocks, relations);

    // The new identifiers are made in the order of the blocks: identifiers sort in the
    // order they were made, and a list of blocks is by creation time and then by
    // identifier, so blocks that are stored in one millisecond come back in the order
    // they were sent in. Blocks that share an identifier share the new one too, which
    // is only kept for the blocks something refers to and those asked for (`told`): no
    // other block needs to learn the new value of the rest.
    let mut renamed: HashMap<String, String> = HashMap::new();
    let new_ids: Vec<String> = blocks
        .iter()
        .map(|block| {
            if referenced.contains(block.id.as_str()) || told(&block.id) {
                renamed
                    .entry(block.id.clone())
                    .or_insert_with(new_id)
                    .clone()
            } else {
                new_id()
            }
        })
        .collect();

    rename_all_relations(blocks, relations, &renamed, &card_boards, unmatched);

    for (block, new) in blocks.iter_mut().zip(new_ids) {
        rewrite_block_non_relation_ids(block, new, &renamed);
    }
    renamed.retain(|old, _| told(old));
    renamed
}

/// Collects every non-empty block identifier that `block` references (its board, its
/// parent, its `cardOrder`/`contentOrder`/`defaultTemplateId` fields, its card relation
/// values under `relations`, and the references and board views inside its body) into
/// `out`.
///
/// Matches [`renew_block_ids`]: when `contentOrder` is present and not a JSON array,
/// collection stops before inspecting `defaultTemplateId`, as renewing does.
pub fn collect_block_references<S: BuildHasher>(
    block: &Block,
    relations: &RelationProperties,
    out: &mut HashSet<String, S>,
) {
    let mut insert = |id: &str| {
        if !id.is_empty() {
            out.insert(id.to_owned());
        }
    };
    for id in related_ids(block, relations) {
        insert(id);
    }
    insert(&block.container_id);
    insert(&block.parent_id);
    if let Some(body) = &block.body {
        for id in body.linked_ids() {
            insert(id);
        }
    }
    if let Some(Value::Array(items)) = block.fields.get("cardOrder") {
        for id in order_ids(items) {
            insert(id);
        }
    }
    if let Some(content_order) = block.fields.get("contentOrder") {
        let Some(items) = content_order.as_array() else {
            return;
        };
        for id in order_ids(items) {
            insert(id);
        }
    }
    if let Some(Value::String(template_id)) = block.fields.get("defaultTemplateId") {
        insert(template_id);
    }
}

/// Rewrites `block`'s identifier and every reference it carries using precomputed
/// mappings (`renamed` for referenced block IDs and `card_boards` mapping old card IDs
/// to their new board IDs), matching [`renew_block_ids`] one block at a time.
pub fn rewrite_block_ids<S1: BuildHasher, S2: BuildHasher>(
    block: &mut Block,
    new_block_id: String,
    renamed: &HashMap<String, String, S1>,
    card_boards: &HashMap<String, String, S2>,
    relations: &RelationProperties,
    unmatched: UnmatchedRelation,
) {
    rename_block_relations(block, relations, renamed, card_boards, unmatched);
    rewrite_block_non_relation_ids(block, new_block_id, renamed);
}

fn rename_block_relations<S1: BuildHasher, S2: BuildHasher>(
    block: &mut Block,
    relations: &RelationProperties,
    renamed: &HashMap<String, String, S1>,
    card_boards: &HashMap<String, String, S2>,
    unmatched: UnmatchedRelation,
) {
    let keep_at_most = match unmatched {
        UnmatchedRelation::Kept => usize::MAX,
        UnmatchedRelation::Dropped => RELATION_LIMIT,
    };
    let container_id = block.container_id.clone();
    let rename_rel = |id: &str| match (renamed.get(id), unmatched) {
        (Some(new), UnmatchedRelation::Kept) => Some(new.clone()),
        (Some(new), UnmatchedRelation::Dropped) if card_boards.get(id) == Some(&container_id) => {
            Some(new.clone())
        }
        (None, UnmatchedRelation::Kept) => Some(id.to_owned()),
        (_, UnmatchedRelation::Dropped) => None,
    };
    rename_relations(block, relations, rename_rel, keep_at_most);
}

fn rewrite_block_non_relation_ids<S: BuildHasher>(
    block: &mut Block,
    new_block_id: String,
    renamed: &HashMap<String, String, S>,
) {
    let renamed_or_same = |id: &mut String| {
        if let Some(new) = renamed.get(id.as_str()) {
            id.clone_from(new);
        }
    };
    block.id = new_block_id;
    renamed_or_same(&mut block.container_id);
    renamed_or_same(&mut block.parent_id);

    for field in ["contentOrder", "cardOrder"] {
        if let Some(Value::Array(items)) = block.fields.get_mut(field) {
            for item in items {
                match item {
                    Value::String(id) => renamed_or_same(id),
                    Value::Array(column) => {
                        for cell in column {
                            if let Value::String(id) = cell {
                                renamed_or_same(id);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    if let Some(Value::String(template_id)) = block.fields.get_mut("defaultTemplateId") {
        renamed_or_same(template_id);
    }
    if let Some(body) = block.body.as_mut() {
        body.rename_links(|id| renamed.get(id).cloned());
    }
}

/// What the blocks refer to by identifier ([`generate_block_ids`]): boards, parents,
/// what the order and template fields and card relations name, and what bodies name and
/// show ([`Body::linked_ids`](crate::Body::linked_ids)).
fn referenced_ids<'b>(blocks: &'b [Block], relations: &RelationProperties) -> HashSet<&'b str> {
    let mut referenced: HashSet<&str> = HashSet::new();
    for block in blocks {
        referenced.extend(related_ids(block, relations));
        referenced.insert(&block.container_id);
        referenced.insert(&block.parent_id);
        if let Some(body) = &block.body {
            referenced.extend(body.linked_ids());
        }
        // A view's card order names cards that nothing else may refer to: a card whose
        // content is its body has no blocks below it.
        if let Some(Value::Array(items)) = block.fields.get("cardOrder") {
            referenced.extend(order_ids(items));
        }

        if let Some(content_order) = block.fields.get("contentOrder") {
            let Some(items) = content_order.as_array() else {
                // Non-array `contentOrder` stops scanning the rest of this block, including
                // `defaultTemplateId`.
                continue;
            };
            referenced.extend(order_ids(items));
        }
        if let Some(Value::String(template_id)) = block.fields.get("defaultTemplateId") {
            referenced.insert(template_id);
        }
    }

    // No identifier is no reference. A block without a parent would otherwise refer to
    // every block of the request that came without an identifier: those would share
    // one new identifier, and the blocks without a parent would get it as theirs.
    referenced.remove("");
    referenced
}

/// Renames what the relation values of the cards among `blocks` name ([`renew_block_ids`]):
/// `renamed` holds the new identifiers, and `card_boards` the board of each card by its
/// old identifier. What a copy drops makes room: a copied value keeps at most
/// [`RELATION_LIMIT`] of what is left.
fn rename_all_relations(
    blocks: &mut [Block],
    relations: &RelationProperties,
    renamed: &HashMap<String, String>,
    card_boards: &HashMap<String, String>,
    unmatched: UnmatchedRelation,
) {
    for block in blocks.iter_mut() {
        rename_block_relations(block, relations, renamed, card_boards, unmatched);
    }
}

/// The identifiers the relation values of a card name.
fn related_ids<'b>(block: &'b Block, relations: &RelationProperties) -> Vec<&'b str> {
    let (Some(properties), Some(Value::Object(values))) = (
        relations.of(&block.container_id),
        block.fields.get("properties"),
    ) else {
        return Vec::new();
    };
    if block.block_type != BlockType::Card {
        return Vec::new();
    }
    properties
        .iter()
        .filter_map(|property| values.get(property).and_then(Value::as_array))
        .flatten()
        .filter_map(relation_link_card)
        .collect()
}

/// Puts in each relation value of a card, for each relation link in it, what `rename`
/// gives for its `card` identifier (preserving `kind` and `lagDays`), and leaves out
/// links for which `rename` gives nothing. Other items stay; of all, the first
/// `keep_at_most`.
fn rename_relations(
    block: &mut Block,
    relations: &RelationProperties,
    rename: impl Fn(&str) -> Option<String>,
    keep_at_most: usize,
) {
    let Some(properties) = relations.of(&block.container_id) else {
        return;
    };
    if block.block_type != BlockType::Card {
        return;
    }
    let Some(Value::Object(values)) = block.fields.get_mut("properties") else {
        return;
    };
    for property in properties {
        let Some(Value::Array(items)) = values.get_mut(property) else {
            continue;
        };
        items.retain_mut(|item| {
            let Some(object) = item.as_object_mut() else {
                return true;
            };
            let Some(Value::String(id)) = object.get_mut("card") else {
                return true;
            };
            match rename(id) {
                Some(new) => {
                    *id = new;
                    true
                }
                None => false,
            }
        });
        items.truncate(keep_at_most);
    }
}

/// Yields the identifiers in an order list, which holds strings and lists of strings.
fn order_ids(items: &[Value]) -> impl Iterator<Item = &str> {
    items.iter().flat_map(|item| -> Vec<&str> {
        match item {
            Value::String(id) => vec![id.as_str()],
            Value::Array(column) => column.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        }
    })
}
