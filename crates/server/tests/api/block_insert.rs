//! Test fixtures that make and delete the blocks of a board as a client of the API does:
//! each by the route of its type. A card is made by `createCard`, a view by
//! `createBoardView` (and placed by `patchBoardView` where the fixture names its
//! `order`), a comment by `createCardComment`, saying its title where the fixture has no
//! body, and an attachment by `createCardAttachment`, after an upload where the fixture
//! names no `fileId`. The server gives each its identifier and its times; a block of the
//! request that another names (as its `parentId`) is named by the identifier it was given.
//! What no route of its type sets, a field of its own or a number, cannot be asked for:
//! the fixture panics naming it.

use std::collections::HashMap;

use fvoci_model::{Block, BlockType, Card, JsonMap};
use serde_json::{Value, json};

use crate::common::Client;

/// The fields of a card that `createCard` sets.
const CARD_FIELDS: [&str; 3] = ["icon", "isTemplate", "properties"];

/// Makes `blocks` on `board_id` of `workspace_id`, each by the route of its type, cards
/// first and each below its parent, and returns them in the order of `blocks`.
pub(crate) async fn insert_in(
    client: &Client,
    workspace_id: &str,
    board_id: &str,
    blocks: &[Block],
) -> Vec<Block> {
    let base = format!("/w/{workspace_id}/boards/{board_id}");
    let mut made: Vec<Option<Block>> = vec![None; blocks.len()];
    let mut names = HashMap::<String, String>::new();
    let mut pending: Vec<usize> = (0..blocks.len()).collect();
    // Cards before the rest, a card after the card of the request it is below.
    pending.sort_by_key(|&at| blocks[at].block_type != BlockType::Card);
    while !pending.is_empty() {
        let ready = pending
            .iter()
            .position(|&at| {
                let parent = &blocks[at].parent_id;
                names.contains_key(parent)
                    || !blocks
                        .iter()
                        .any(|other| !parent.is_empty() && other.id == *parent)
            })
            .expect("every parent of the request is made before its children");
        let at = pending.remove(ready);
        let block = &blocks[at];
        let parent_id = names
            .get(&block.parent_id)
            .cloned()
            .unwrap_or_else(|| block.parent_id.clone());
        let stored = match block.block_type {
            BlockType::Card => make_card(client, &base, block, &parent_id).await,
            BlockType::View => make_view(client, &base, block).await,
            BlockType::Comment => make_comment(client, &base, block, &parent_id).await,
            BlockType::Attachment => {
                make_attachment(client, workspace_id, board_id, block, &parent_id).await
            }
            other => panic!("no route of a board makes a block of type {other}"),
        };
        if !block.id.is_empty() {
            names.insert(block.id.clone(), stored.id.clone());
        }
        made[at] = Some(stored);
    }
    made.into_iter()
        .map(|block| block.expect("every block is made"))
        .collect()
}

/// Deletes `block` of the test workspace by the route of its type, expecting success.
pub(crate) async fn delete_in(client: &Client, workspace_id: &str, block: &Block) {
    let base = format!("/w/{workspace_id}/boards/{}", block.container_id);
    let response = match block.block_type {
        BlockType::Card => {
            client
                .post(
                    &format!("{base}/cards/batch-delete"),
                    &json!({"cardIds": [block.id]}),
                )
                .await
        }
        BlockType::View => client.delete(&format!("{base}/views/{}", block.id)).await,
        BlockType::Comment => {
            client
                .delete(&format!(
                    "{base}/cards/{}/comments/{}",
                    block.parent_id, block.id
                ))
                .await
        }
        BlockType::Attachment => {
            client
                .delete(&format!(
                    "{base}/cards/{}/attachments/{}",
                    block.parent_id, block.id
                ))
                .await
        }
        other => panic!("no route of a board deletes a block of type {other}"),
    };
    response.check_no_content();
}

/// Panics if `block` has a field that is not one of `known`, which no route of its type sets.
fn only_fields(block: &Block, known: &[&str]) {
    if let Some(field) = block
        .fields
        .keys()
        .find(|key| !known.contains(&key.as_str()))
    {
        panic!(
            "no route sets the field {field} of a {}: {block:?}",
            block.block_type
        );
    }
}

async fn make_card(client: &Client, base: &str, block: &Block, parent_id: &str) -> Block {
    only_fields(block, &CARD_FIELDS);
    assert!(
        block.number.is_none(),
        "the server numbers a card: {block:?}"
    );
    let mut card = JsonMap::new();
    card.insert("title".to_owned(), json!(block.title));
    if !parent_id.is_empty() {
        card.insert("parentId".to_owned(), json!(parent_id));
    }
    for key in CARD_FIELDS {
        if let Some(value) = block.fields.get(key) {
            card.insert(key.to_owned(), value.clone());
        }
    }
    if let Some(body) = &block.body {
        card.insert(
            "body".to_owned(),
            serde_json::to_value(body).expect("a body is JSON"),
        );
    }
    let created: Card = client
        .post(&format!("{base}/cards"), &Value::Object(card))
        .await
        .created_json();
    Block::from(created)
}

async fn make_view(client: &Client, base: &str, block: &Block) -> Block {
    let mut fields = block.fields.clone();
    let order = fields.remove("order");
    let view_type = fields
        .get("viewType")
        .cloned()
        .unwrap_or_else(|| json!("board"));
    let write: Value = client
        .post(
            &format!("{base}/views"),
            &json!({"title": block.title, "viewType": view_type, "fields": fields}),
        )
        .await
        .created_json();
    let view: Block = serde_json::from_value(write["view"].clone()).expect("a view is a block");
    match order {
        None => view,
        Some(order) => client
            .patch(
                &format!("{base}/views/{}", view.id),
                &json!({"updatedFields": {"order": order}}),
            )
            .await
            .ok_json(),
    }
}

async fn make_comment(client: &Client, base: &str, block: &Block, card_id: &str) -> Block {
    only_fields(block, &[]);
    let body = match &block.body {
        Some(body) => serde_json::to_value(body).expect("a body is JSON"),
        None => json!({
            "type": "doc",
            "content": [{"type": "paragraph", "content": [{"type": "text", "text": block.title}]}],
        }),
    };
    client
        .post(
            &format!("{base}/cards/{card_id}/comments"),
            &json!({"body": body}),
        )
        .await
        .created_json()
}

async fn make_attachment(
    client: &Client,
    workspace_id: &str,
    board_id: &str,
    block: &Block,
    card_id: &str,
) -> Block {
    only_fields(block, &["fileId"]);
    let file_id = if let Some(file_id) = block.fields.get("fileId").and_then(Value::as_str) {
        file_id.to_owned()
    } else {
        let name = if block.title.is_empty() {
            "attached.txt"
        } else {
            block.title.as_str()
        };
        let uploaded =
            crate::files::upload_in(client, workspace_id, board_id, name, b"attached").await;
        uploaded.check_created();
        crate::files::uploaded_id(&uploaded)
    };
    client
        .post(
            &format!("/w/{workspace_id}/boards/{board_id}/cards/{card_id}/attachments"),
            &json!({"fileId": file_id, "title": block.title}),
        )
        .await
        .created_json()
}
