//! The types of new blocks and the limits of their fields.

use crate::blocks::{
    block_path, blocks_of, blocks_path, child_of, create_board_in_store, fields, ids, insert,
    insert_one, new_block,
};
use crate::common::TEST_WS;
use crate::common::{Setup, TEST_WORKSPACE_ID};
use fvoci_model::{
    BLOCK_FIELDS_MAX_CHARS, Block, BlockPatch, BlockType, CARD_FIELDS_MAX_CHARS,
    ContainerVisibility,
};
use serde_json::json;

// -- the types of new blocks and the limits of their fields ------------------------------

/// A new block is a card, a view, a comment or an attachment: the content blocks went
/// with the card's content order. Any other type is refused
/// as the type on the routes that make blocks, and nothing is stored.
#[tokio::test]
async fn a_new_block_of_another_type_is_refused() {
    let th = Setup::one_user().start().await;
    let board = create_board_in_store(&th, ContainerVisibility::Open).await;
    let card = insert_one(&th.client, &new_block(&board.id, BlockType::Card, "card")).await;
    for name in [
        "text",
        "image",
        "checkbox",
        "divider",
        "board",
        "h1",
        "list-item",
    ] {
        let fine = child_of(&card, BlockType::Attachment, "fine");
        let mut block =
            serde_json::to_value(child_of(&card, BlockType::Card, name)).expect("a block is JSON");
        block["type"] = json!(name);
        let response = th
            .client
            .post(&blocks_path(&board.id), &json!([fine, block]))
            .await;
        response.check_bad_request();
        assert_eq!(response.refused_at(), "/1/type", "the second block: {name}");

        let response = th
            .client
            .post(
                &format!("{TEST_WS}/containers-and-blocks"),
                &json!({
                    "containers": [{"id": "b1", "workspaceId": TEST_WORKSPACE_ID, "visibility": "open"}],
                    "blocks": [{
                        "id": "x1", "containerId": "b1", "type": name, "createdAt": 1, "updatedAt": 1,
                    }],
                }),
            )
            .await;
        response.check_bad_request();
        assert_eq!(
            response.refused_at(),
            "/blocks/0/type",
            "{name} with a board"
        );
    }
    assert_eq!(
        ids(&blocks_of(&th.client, &board.id).await),
        [card.id.as_str()],
        "the card alone"
    );
    th.teardown().await;
}

/// A page, a diary entry, a unit or a chapter sent to the board block creation routes is
/// refused with the message naming its own container's operations.
#[tokio::test]
async fn a_new_block_of_another_containers_type_names_its_operations() {
    let th = Setup::one_user().start().await;
    let board = create_board_in_store(&th, ContainerVisibility::Open).await;
    let card = insert_one(&th.client, &new_block(&board.id, BlockType::Card, "card")).await;
    for (name, expected) in [
        (
            "page",
            "a page is made and changed by the operations of pages",
        ),
        (
            "entry",
            "an entry is made and changed by the operations of diaries",
        ),
        (
            "unit",
            "a unit or a chapter is made and changed by the operations of courses",
        ),
        (
            "chapter",
            "a unit or a chapter is made and changed by the operations of courses",
        ),
    ] {
        let mut block =
            serde_json::to_value(child_of(&card, BlockType::Card, name)).expect("a block is JSON");
        block["type"] = json!(name);
        let response = th
            .client
            .post(&blocks_path(&board.id), &json!([block]))
            .await;
        response.check_bad_request();
        assert_eq!(response.refused_at(), "/0/type", "{name} on createBlocks");
        assert_eq!(
            response.value()["detail"],
            json!(expected),
            "{name} detail on createBlocks: {}",
            response.value()
        );

        let response = th
            .client
            .post(
                &format!("{TEST_WS}/containers-and-blocks"),
                &json!({
                    "containers": [{"id": "b1", "workspaceId": TEST_WORKSPACE_ID, "visibility": "open"}],
                    "blocks": [{
                        "id": "x1", "containerId": "b1", "type": name, "createdAt": 1, "updatedAt": 1,
                    }],
                }),
            )
            .await;
        response.check_bad_request();
        assert_eq!(
            response.refused_at(),
            "/blocks/0/type",
            "{name} on createContainersAndBlocks"
        );
        assert_eq!(
            response.value()["detail"],
            json!(expected),
            "{name} detail on createContainersAndBlocks: {}",
            response.value()
        );
    }
    th.teardown().await;
}

/// A patch, alone or in a batch, may not give a block one of those types.
#[tokio::test]
async fn a_patch_does_not_make_a_block_of_another_type() {
    let th = Setup::one_user().start().await;
    let board = create_board_in_store(&th, ContainerVisibility::Open).await;
    let card = insert_one(&th.client, &new_block(&board.id, BlockType::Card, "card")).await;
    for attempted in ["text", "view", "comment", "card"] {
        let to_type = json!({"type": attempted});
        let response = th
            .client
            .patch(&block_path(&board.id, &card.id), &to_type)
            .await;
        response.check_bad_request();
        assert_eq!(response.refused_at(), "/type", "one patch: {attempted}");
        let batch = json!({
            "blockIds": [card.id],
            "blockPatches": [to_type],
        });
        let response = th.client.patch(&blocks_path(&board.id), &batch).await;
        response.check_bad_request();
        assert_eq!(
            response.refused_at(),
            "/blockPatches/0/type",
            "a batch: {attempted}"
        );
        let combined = json!({
            "containerIds": [board.id],
            "containerPatches": [{}],
            "blockIds": [card.id],
            "blockPatches": [to_type],
        });
        let response = th
            .client
            .patch(&format!("{TEST_WS}/containers-and-blocks"), &combined)
            .await;
        response.check_bad_request();
        assert_eq!(
            response.refused_at(),
            "/blockPatches/0/type",
            "containers-and-blocks: {attempted}"
        );
    }
    assert_eq!(
        blocks_of(&th.client, &board.id).await[0].block_type,
        BlockType::Card,
        "still a card"
    );

    th.teardown().await;
}

/// The serialized fields of a card, a comment or an attachment have at most 64,000
/// characters, those of a view 800,000, when the block is made
/// and when it is patched.
#[tokio::test]
async fn the_fields_of_a_block_are_limited_by_its_type() {
    let th = Setup::one_user().start().await;
    let board = create_board_in_store(&th, ContainerVisibility::Open).await;
    let card = insert_one(&th.client, &new_block(&board.id, BlockType::Card, "card")).await;
    let sized = |block_type: BlockType, len: usize| match block_type {
        BlockType::View => Block {
            fields: fields(json!({"groupBy": "a".repeat(len)})),
            ..new_block(&board.id, BlockType::View, "sized")
        },
        _ => Block {
            fields: fields(json!({"k": "a".repeat(len)})),
            ..child_of(&card, block_type, "sized")
        },
    };
    for (block_type, limit, overhead) in [
        (BlockType::Comment, CARD_FIELDS_MAX_CHARS, 8),
        (BlockType::Attachment, CARD_FIELDS_MAX_CHARS, 8),
        (BlockType::View, BLOCK_FIELDS_MAX_CHARS, 14),
    ] {
        insert(
            &th.client,
            &board.id,
            &[sized(block_type, limit - overhead)],
        )
        .await;
        let response = th
            .client
            .insert_blocks(&board.id, &[sized(block_type, limit - overhead + 1)])
            .await;
        response.check_bad_request();
        assert!(
            response.value()["detail"]
                .as_str()
                .is_some_and(|message| message.contains("block fields size limit exceeded")),
            "{block_type} over the limit: {}",
            response.value()
        );
    }
    let card_at_limit = json!({
        "containers": [{"id": "b1", "workspaceId": TEST_WORKSPACE_ID, "visibility": "open"}],
        "blocks": [{
            "id": "c1",
            "containerId": "b1",
            "type": "card",
            "fields": {"k": "a".repeat(CARD_FIELDS_MAX_CHARS - 8)},
        }],
    });
    th.client
        .post(&format!("{TEST_WS}/containers-and-blocks"), &card_at_limit)
        .await
        .check_created();
    let card_over_limit = json!({
        "containers": [{"id": "b2", "workspaceId": TEST_WORKSPACE_ID, "visibility": "open"}],
        "blocks": [{
            "id": "c2",
            "containerId": "b2",
            "type": "card",
            "fields": {"k": "a".repeat(CARD_FIELDS_MAX_CHARS - 7)},
        }],
    });
    let response = th
        .client
        .post(
            &format!("{TEST_WS}/containers-and-blocks"),
            &card_over_limit,
        )
        .await;
    response.check_bad_request();
    assert!(
        response.value()["detail"]
            .as_str()
            .is_some_and(|message| message.contains("block fields size limit exceeded")),
        "card over the limit: {}",
        response.value()
    );

    let grow = BlockPatch {
        updated_fields: fields(json!({"more": "b".repeat(CARD_FIELDS_MAX_CHARS)})),
        ..BlockPatch::default()
    };
    let response = th
        .client
        .patch(&block_path(&board.id, &card.id), &grow)
        .await;
    response.check_bad_request();
    let stored = th.app.block(&card.id).await.expect("the card is stored");
    assert!(!stored.fields.contains_key("more"), "the card is as it was");
    th.teardown().await;
}
