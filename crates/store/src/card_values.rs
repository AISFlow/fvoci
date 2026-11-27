//! The checks of the card property values and view filter/dependency rules a write gives
//! a block, beyond their shape ([`fvoci_model::Block::validate`]): what fits the kinds
//! of the board's properties, what a card relation names, and
//! whether an active Gantt dependency relation stays acyclic.
//!
//! A relation value names cards of the block's board, in the trash or out of it. Which
//! ones it names is asked once every block of a write is stored, so that two new cards
//! of one write may name each other. A card that is another board's is answered as one
//! that is not there at all: who may not see that board learns nothing of it.

use std::collections::{HashMap, HashSet};

use fvoci_model::{
    Block, BlockError, BlockType, CardPropertyDefinition, JsonMap, RELATION_LIMIT, RelationError,
    RelationProblem, check_relation_value, closes_relation_cycle, gantt_dependency_property_ids,
    relation_link_card, relation_links_to_value, relation_property_ids, tidy_copied_relations,
    validate_card_property_kinds, validate_view_filter_values,
};
use serde_json::Value;

use crate::containers;
use crate::db::{Dialect, Id, Scope};
use crate::error::StoreError;

/// How many identifiers one query of [`types_on_board`] asks for, twice (live and in the
/// trash): within what either database binds in one statement.
const LOOKUP_CHUNK: usize = 500;

/// The cards a relation value of a written block names that it did not name before:
/// those are asked for when the write is stored ([`check_named_cards`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NamedCards {
    block_id: String,
    board_id: String,
    property: String,
    ids: Vec<String>,
}

/// Checks the property values or view filter/dependency settings a write changes of a
/// block, against what `existing` (the block as it is stored, if it is) has.
///
/// Returns what the relation values name that they did not name before, to be asked for
/// once the write is stored: an identifier a value had is not asked for again.
///
/// # Errors
///
/// Returns [`StoreError::InvalidBlock`] for the first value that is refused.
pub(crate) async fn check_written(
    scope: &mut Scope<'_>,
    block: &mut Block,
    existing: Option<&Block>,
) -> Result<Vec<NamedCards>, StoreError> {
    if block.block_type == BlockType::View {
        check_written_view(scope, block, existing).await?;
    }
    let Some(Value::Object(values)) = block.fields.get("properties") else {
        return Ok(Vec::new());
    };
    let before = existing
        .and_then(|existing| existing.fields.get("properties"))
        .and_then(Value::as_object);
    let written: JsonMap = values
        .iter()
        .filter(|(id, value)| before.and_then(|before| before.get(*id)) != Some(*value))
        .map(|(id, value)| (id.clone(), value.clone()))
        .collect();
    if written.is_empty() {
        return Ok(Vec::new());
    }
    let Some(board) = containers::find_container(scope, &block.container_id).await? else {
        return Ok(Vec::new());
    };
    let invalid = |id: &str, source: BlockError| StoreError::InvalidBlock {
        id: id.to_owned(),
        source,
    };
    validate_card_property_kinds(&written, &board.card_properties)
        .map_err(|error| invalid(&block.id, error.into()))?;
    if block.block_type != BlockType::Card {
        return Ok(Vec::new());
    }
    collect_written_relations(block, &written, before, &board.card_properties)
}

async fn check_written_view(
    scope: &mut Scope<'_>,
    block: &Block,
    existing: Option<&Block>,
) -> Result<(), StoreError> {
    let filter_changed = block.fields.get("filter").is_some_and(|v| !v.is_null())
        && existing.and_then(|e| e.fields.get("filter")) != block.fields.get("filter");
    let gantt_dep = gantt_dep_of(&block.fields);
    let prev_gantt_dep = existing.and_then(|e| gantt_dep_of(&e.fields));
    let dep_changed = gantt_dep.is_some() && gantt_dep != prev_gantt_dep;
    if !filter_changed && !dep_changed {
        return Ok(());
    }
    let Some(board) = containers::find_container(scope, &block.container_id).await? else {
        return Ok(());
    };
    if filter_changed {
        validate_view_filter_values(&block.fields, &board.card_properties).map_err(|error| {
            StoreError::InvalidBlock {
                id: block.id.clone(),
                source: error.into(),
            }
        })?;
    }
    if let Some(dep_id) = gantt_dep.filter(|_| dep_changed)
        && relation_property_ids(&board.card_properties).contains(dep_id)
        && board_has_relation_cycle(scope, &board.id, dep_id).await?
    {
        return Err(StoreError::InvalidBlock {
            id: block.id.clone(),
            source: RelationError::new(dep_id, RelationProblem::Cycle).into(),
        });
    }
    Ok(())
}

fn gantt_dep_of(fields: &JsonMap) -> Option<&str> {
    if fields.get("viewType").and_then(Value::as_str) != Some("gantt") {
        return None;
    }
    fields
        .get("dependencyPropertyId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
}

fn collect_written_relations(
    block: &mut Block,
    written: &JsonMap,
    before: Option<&JsonMap>,
    definitions: &[CardPropertyDefinition],
) -> Result<Vec<NamedCards>, StoreError> {
    let invalid = |id: &str, source: BlockError| StoreError::InvalidBlock {
        id: id.to_owned(),
        source,
    };
    let relations = relation_property_ids(definitions);
    let mut named = Vec::new();
    let mut normalized = Vec::new();
    for (property, value) in written {
        if !relations.contains(property.as_str()) {
            continue;
        }
        let had: HashSet<&str> = before
            .and_then(|before| before.get(property))
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(relation_link_card).collect())
            .unwrap_or_default();
        let Some(links) = check_relation_value(&block.id, property, value, &had)
            .map_err(|error| invalid(&block.id, error.into()))?
        else {
            continue;
        };
        normalized.push((property.clone(), relation_links_to_value(&links)));
        let ids: Vec<String> = links
            .into_iter()
            .filter(|link| !had.contains(link.card.as_str()))
            .map(|link| link.card)
            .collect();
        if !ids.is_empty() {
            named.push(NamedCards {
                block_id: block.id.clone(),
                board_id: block.container_id.clone(),
                property: property.clone(),
                ids,
            });
        }
    }
    if !normalized.is_empty() {
        if let Some(Value::Object(values)) = block.fields.get_mut("properties") {
            for (property, value) in normalized {
                values.insert(property, value);
            }
        }
        block
            .validate()
            .map_err(|source| invalid(&block.id, source))?;
    }
    Ok(named)
}

/// Checks that what relation values name are cards of their blocks' boards, in the
/// trash or out of it, and that active Gantt dependency properties remain acyclic.
///
/// # Errors
///
/// Returns [`StoreError::InvalidBlock`] with a [`RelationError`] for the first value
/// that names a block of its board that is no card
/// ([`RelationProblem::NotACard`]), a card that is not on its board
/// ([`RelationProblem::CardNotFound`]), or an edge closing a Gantt dependency cycle
/// ([`RelationProblem::Cycle`]).
pub(crate) async fn check_named_cards(
    scope: &mut Scope<'_>,
    named: &[NamedCards],
) -> Result<(), StoreError> {
    if named.is_empty() {
        return Ok(());
    }
    let mut by_board: HashMap<&str, HashSet<&str>> = HashMap::new();
    for value in named {
        by_board
            .entry(&value.board_id)
            .or_default()
            .extend(value.ids.iter().map(String::as_str));
    }
    let mut types: HashMap<&str, HashMap<String, BlockType>> = HashMap::new();
    for (board_id, ids) in by_board {
        let ids: Vec<&str> = ids.into_iter().collect();
        types.insert(board_id, types_on_board(scope, board_id, &ids).await?);
    }
    for value in named {
        let found = types.get(value.board_id.as_str());
        for id in &value.ids {
            let problem = match found.and_then(|found| found.get(id)) {
                Some(BlockType::Card) => continue,
                Some(_) => RelationProblem::NotACard,
                None => RelationProblem::CardNotFound,
            };
            return Err(StoreError::InvalidBlock {
                id: value.block_id.clone(),
                source: RelationError::new(&value.property, problem).into(),
            });
        }
    }
    check_gantt_dependency_cycles(scope, named).await
}

async fn check_gantt_dependency_cycles(
    scope: &mut Scope<'_>,
    named: &[NamedCards],
) -> Result<(), StoreError> {
    let mut boards_seen = HashSet::new();
    for entry in named {
        if !boards_seen.insert(entry.board_id.as_str()) {
            continue;
        }
        let board_id = entry.board_id.as_str();
        let Some(board) = containers::find_container(scope, board_id).await? else {
            continue;
        };
        let views = live_view_fields(scope, board_id).await?;
        let active_deps = gantt_dependency_property_ids(&views, &board.card_properties);
        if active_deps.is_empty() {
            continue;
        }
        let mut checked_props: HashMap<&str, HashMap<String, Vec<String>>> = HashMap::new();
        for item in named.iter().filter(|n| n.board_id == board_id) {
            if !active_deps.contains(item.property.as_str()) {
                continue;
            }
            let preds_by_card = if let Some(map) = checked_props.get(item.property.as_str()) {
                map
            } else {
                let map = load_predecessors_map(scope, board_id, &item.property).await?;
                checked_props.entry(item.property.as_str()).or_insert(map)
            };
            if let Some(preds) = preds_by_card.get(&item.block_id)
                && closes_relation_cycle(preds_by_card, &item.block_id, preds)
            {
                return Err(StoreError::InvalidBlock {
                    id: item.block_id.clone(),
                    source: RelationError::new(&item.property, RelationProblem::Cycle).into(),
                });
            }
        }
    }
    Ok(())
}

async fn board_has_relation_cycle(
    scope: &mut Scope<'_>,
    board_id: &str,
    property_id: &str,
) -> Result<bool, StoreError> {
    let preds_by_card = load_predecessors_map(scope, board_id, property_id).await?;
    Ok(preds_by_card
        .iter()
        .any(|(card_id, preds)| closes_relation_cycle(&preds_by_card, card_id, preds)))
}

async fn live_view_fields(
    scope: &mut Scope<'_>,
    board_id: &str,
) -> Result<Vec<JsonMap>, StoreError> {
    let mut sql = scope.sql("SELECT fields FROM blocks WHERE container_id = ");
    sql.bind(Id(board_id))
        .push_bind(" AND type = ", BlockType::View.as_str());
    scope
        .fetch_all(sql)
        .await?
        .iter()
        .map(|row| row.json_map(0, "fields"))
        .collect()
}

async fn load_predecessors_map(
    scope: &mut Scope<'_>,
    board_id: &str,
    property_id: &str,
) -> Result<HashMap<String, Vec<String>>, StoreError> {
    let mut sql = scope.sql("SELECT id, fields FROM blocks WHERE container_id = ");
    sql.bind(Id(board_id))
        .push_bind(" AND type = ", BlockType::Card.as_str());
    let mut map = HashMap::new();
    for row in scope.fetch_all(sql).await? {
        let card_id = row.id(0)?;
        let fields = row.json_map(1, "fields")?;
        if fields.get("isTemplate") == Some(&Value::Bool(true)) {
            continue;
        }
        let Some(items) = fields
            .get("properties")
            .and_then(Value::as_object)
            .and_then(|props| props.get(property_id))
            .and_then(Value::as_array)
        else {
            continue;
        };
        let preds: Vec<String> = items
            .iter()
            .filter_map(relation_link_card)
            .map(str::to_owned)
            .collect();
        if !preds.is_empty() {
            map.insert(card_id, preds);
        }
    }
    Ok(map)
}

/// The types of the blocks of a board among `ids`, in the trash or out of it.
async fn types_on_board(
    scope: &mut Scope<'_>,
    board_id: &str,
    ids: &[&str],
) -> Result<HashMap<String, BlockType>, StoreError> {
    let trashed_type = match scope.dialect() {
        Dialect::Sqlite => {
            "CASE WHEN block_type <> '' THEN block_type ELSE json_extract(block, '$.type') END"
        }
        Dialect::Postgres => "CASE WHEN block_type <> '' THEN block_type ELSE block->>'type' END",
    };
    let mut types = HashMap::with_capacity(ids.len());
    for chunk in ids.chunks(LOOKUP_CHUNK) {
        let mut sql = scope.sql("SELECT id, type FROM blocks WHERE container_id = ");
        sql.bind(Id(board_id))
            .push(" AND id IN (")
            .bind_list(chunk.iter().map(|id| Id(id)))
            .push([
                ") UNION ALL SELECT id, ",
                trashed_type,
                " FROM deleted_blocks WHERE container_id = ",
            ])
            .bind(Id(board_id))
            .push(" AND id IN (")
            .bind_list(chunk.iter().map(|id| Id(id)))
            .push(")");
        for row in scope.fetch_all(sql).await? {
            types.insert(
                row.id(0)?,
                crate::blocks::block_type_column(&row, 1, "type")?,
            );
        }
    }
    Ok(types)
}

/// Clears what copied blocks can no longer be stored with: malformed fields
/// ([`Block::drop_malformed_fields`]), the values of cards that no longer fit their
/// board's definitions, as [`fvoci_model::clear_values_of_another_kind`] does, and of
/// relation values what a write would refuse ([`tidy_copied_relations`]).
pub(crate) fn clear_copied_values(blocks: &mut [Block], definitions: &[CardPropertyDefinition]) {
    for block in blocks {
        block.drop_malformed_fields();
        let id = block.id.clone();
        if let Some(Value::Object(values)) = block.fields.get_mut("properties") {
            fvoci_model::clear_values_of_another_kind(values, definitions);
            tidy_copied_relations(values, &id, definitions);
        }
    }
}

/// Leaves out of the relation values of copied blocks of a board what names no card of
/// the board, in the trash or out of it, and of the rest what is past
/// [`fvoci_model::RELATION_LIMIT`]: copies of cards stored before the rules may name
/// cards that are gone for good, or blocks that are no cards.
pub(crate) async fn keep_cards_of_board(
    scope: &mut Scope<'_>,
    blocks: &mut [Block],
    board_id: &str,
    definitions: &[CardPropertyDefinition],
) -> Result<(), StoreError> {
    let relations = relation_property_ids(definitions);
    let relation_values = |block: &Block| -> Vec<String> {
        let Some(Value::Object(values)) = block.fields.get("properties") else {
            return Vec::new();
        };
        relations
            .iter()
            .filter_map(|property| values.get(*property).and_then(Value::as_array))
            .flatten()
            .filter_map(relation_link_card)
            .map(str::to_owned)
            .collect()
    };
    let named: HashSet<String> = blocks.iter().flat_map(relation_values).collect();
    if named.is_empty() {
        return Ok(());
    }
    let named: Vec<&str> = named.iter().map(String::as_str).collect();
    let types = types_on_board(scope, board_id, &named).await?;
    for block in blocks.iter_mut() {
        let Some(Value::Object(values)) = block.fields.get_mut("properties") else {
            continue;
        };
        for property in &relations {
            if let Some(Value::Array(items)) = values.get_mut(*property) {
                items.retain(|item| {
                    relation_link_card(item)
                        .is_some_and(|id| types.get(id) == Some(&BlockType::Card))
                });
                items.truncate(RELATION_LIMIT);
            }
        }
    }
    Ok(())
}
