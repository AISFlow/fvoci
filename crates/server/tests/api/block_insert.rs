//! Test fixture helper routing `BlockType::Card` inserts through `POST .../cards` while
//! inserting non-card blocks via `POST .../blocks`.

use std::collections::{HashMap, HashSet};

use fvoci_model::{Block, BlockPatch, BlockType, Card, JsonMap};
use serde_json::{Value, json};

use crate::common::Client;

/// Inserts blocks on `board_id` in `workspace_id`, routing `BlockType::Card` fixtures
/// through `createCard` while inserting other blocks through `createBlocks`.
pub(crate) async fn insert_in(
    client: &Client,
    workspace_id: &str,
    board_id: &str,
    blocks: &[Block],
) -> Vec<Block> {
    if !blocks
        .iter()
        .any(|block| block.block_type == BlockType::Card)
    {
        return client
            .insert_blocks_in(workspace_id, board_id, blocks)
            .await
            .created_json();
    }
    let mut results: Vec<Option<Block>> = vec![None; blocks.len()];
    let mut id_map = HashMap::<String, String>::new();
    let temp_cards: HashSet<&str> = blocks
        .iter()
        .filter(|block| block.block_type == BlockType::Card && !block.id.is_empty())
        .map(|block| block.id.as_str())
        .collect();
    let mut pending_cards: Vec<usize> = blocks
        .iter()
        .enumerate()
        .filter_map(|(idx, block)| (block.block_type == BlockType::Card).then_some(idx))
        .collect();
    while !pending_cards.is_empty() {
        let pos = pending_cards
            .iter()
            .position(|&idx| {
                let parent = blocks[idx].parent_id.as_str();
                parent.is_empty() || !temp_cards.contains(parent) || id_map.contains_key(parent)
            })
            .expect("card hierarchy in test fixture is acyclic");
        let idx = pending_cards.remove(pos);
        let stored =
            insert_single_card(client, workspace_id, board_id, &blocks[idx], &mut id_map).await;
        results[idx] = Some(stored);
    }
    insert_non_cards(
        client,
        workspace_id,
        board_id,
        blocks,
        &id_map,
        &mut results,
    )
    .await;
    results
        .into_iter()
        .map(|entry| entry.expect("every block created"))
        .collect()
}

async fn insert_single_card(
    client: &Client,
    workspace_id: &str,
    board_id: &str,
    block: &Block,
    id_map: &mut HashMap<String, String>,
) -> Block {
    let mut card_json = serde_json::Map::new();
    card_json.insert("title".to_owned(), json!(block.title));
    let parent_id = id_map
        .get(&block.parent_id)
        .cloned()
        .unwrap_or_else(|| block.parent_id.clone());
    if !parent_id.is_empty() {
        card_json.insert("parentId".to_owned(), json!(parent_id));
    }
    for key in ["icon", "isTemplate", "properties"] {
        if let Some(val) = block.fields.get(key) {
            card_json.insert(key.to_owned(), val.clone());
        }
    }
    if let Some(body) = &block.body {
        card_json.insert(
            "body".to_owned(),
            serde_json::to_value(body).expect("body is JSON"),
        );
    }
    if let Some(number) = block.number {
        card_json.insert("number".to_owned(), json!(number));
    }
    let created: Card = client
        .post(
            &format!("/w/{workspace_id}/boards/{board_id}/cards"),
            &Value::Object(card_json),
        )
        .await
        .created_json();
    if !block.id.is_empty() {
        id_map.insert(block.id.clone(), created.id.clone());
    }
    let extra: JsonMap = block
        .fields
        .iter()
        .filter(|(k, _)| !matches!(k.as_str(), "icon" | "isTemplate" | "properties"))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    if !extra.is_empty() {
        client
            .patch(
                &format!("/w/{workspace_id}/boards/{board_id}/blocks/{}", created.id),
                &BlockPatch {
                    updated_fields: extra,
                    ..BlockPatch::default()
                },
            )
            .await
            .check_no_content();
    }
    let mut stored: Vec<Block> = client
        .get(&format!(
            "/w/{workspace_id}/boards/{board_id}/blocks?blockId={}",
            created.id
        ))
        .await
        .ok_json();
    stored.pop().expect("stored card block")
}

async fn insert_non_cards(
    client: &Client,
    workspace_id: &str,
    board_id: &str,
    blocks: &[Block],
    id_map: &HashMap<String, String>,
    results: &mut [Option<Block>],
) {
    let non_card_indices: Vec<usize> = blocks
        .iter()
        .enumerate()
        .filter_map(|(idx, block)| (block.block_type != BlockType::Card).then_some(idx))
        .collect();
    if non_card_indices.is_empty() {
        return;
    }
    let remapped: Vec<Block> = non_card_indices
        .iter()
        .map(|&idx| {
            let mut cloned = blocks[idx].clone();
            if let Some(mapped) = id_map.get(&cloned.parent_id) {
                cloned.parent_id.clone_from(mapped);
            }
            cloned
        })
        .collect();
    let created_non_cards: Vec<Block> = client
        .insert_blocks_in(workspace_id, board_id, &remapped)
        .await
        .created_json();
    for (idx, stored) in non_card_indices.into_iter().zip(created_non_cards) {
        results[idx] = Some(stored);
    }
}
