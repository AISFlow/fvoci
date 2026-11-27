//! The list of the deleted cards of a board.

use fvoci_model::{BlockType, ContainerVisibility};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::blocks::{
    NO_BOARD, block_path, blocks_of, boards_with_roles, child_of, delete, insert_one, new_block,
};
use crate::common::TEST_WS;
use crate::common::{Setup, TEST_WORKSPACE_ID};

/// The deleted cards of a board are listed for its editors, the last deleted first, with
/// what it takes to decide on bringing one back; a viewer is refused, and for anybody
/// else the board is not there.
#[tokio::test]
async fn the_deleted_cards_of_a_board_are_listed_for_its_editors() {
    let th = Setup::native().start().await.init_basic().await;
    let boards = boards_with_roles(&th).await;
    let board_id = &boards.board.id;
    let editor = boards.editor.me().await;
    let first = insert_one(&th.client, &new_block(board_id, BlockType::Card, "first")).await;
    let second = insert_one(&th.client, &new_block(board_id, BlockType::Card, "second")).await;
    insert_one(&th.client, &new_block(board_id, BlockType::Card, "kept")).await;
    let view = insert_one(&th.client, &new_block(board_id, BlockType::View, "view")).await;
    delete(&th.client, &first).await;
    a_moment().await;
    delete(&boards.editor, &second).await;
    delete(&th.client, &view).await;
    let trash = format!("{TEST_WS}/boards/{board_id}/trash/cards");

    let listed = boards.editor.get(&trash).await.value();
    let entries: Vec<(Value, Value)> = listed
        .as_array()
        .expect("a list")
        .iter()
        .map(|entry| (entry["title"].clone(), entry["deletedBy"].clone()))
        .collect();
    assert_eq!(
        entries,
        [
            (json!("second"), json!(editor.id)),
            (json!("first"), json!(th.client.me().await.id)),
        ],
        "cards only, the last deleted first"
    );
    let members: Vec<&String> = listed[0].as_object().expect("an entry").keys().collect();
    assert_eq!(members, ["deletedAt", "deletedBy", "id", "title"]);
    assert_eq!(listed[0]["id"], json!(second.id));
    assert!(listed[0]["deletedAt"].as_i64().unwrap_or(0) > 0, "{listed}");

    let page: Vec<Value> = boards
        .editor
        .get(&format!("{trash}?page=1&perPage=1"))
        .await
        .ok_json();
    assert_eq!(page.len(), 1);
    assert_eq!(page[0]["title"], json!("first"), "the pages cut that order");

    boards.viewer.get(&trash).await.check_error(
        StatusCode::FORBIDDEN,
        "access denied to the trash of the board",
    );
    th.client2
        .get(&trash)
        .await
        .check_error(StatusCode::NOT_FOUND, NO_BOARD);
    th.anonymous().get(&trash).await.check_unauthorized();

    // Brought back, a card is no longer listed.
    th.client
        .post_empty(&format!("{}/restore", block_path(board_id, &second.id)))
        .await
        .check_ok();
    let listed: Vec<Value> = th.client.get(&trash).await.ok_json();
    assert_eq!(listed.len(), 1, "{listed:?}");
}

/// Two deletions a millisecond apart at least, so that their order is that of time.
async fn a_moment() {
    tokio::time::sleep(std::time::Duration::from_millis(3)).await;
}

/// The list of a board's deleted cards has each card that `restoreBlock` brings back by
/// itself: a card on another one deleted alone is there, a card that went with the card
/// it was on comes back with that
/// one and is not; no comment is a card. Paging as `getCards`; no list for a board in the
/// trash.
#[tokio::test]
async fn the_deleted_cards_are_those_that_come_back_by_themselves() {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let board_id = &board.id;
    let outer = insert_one(&th.client, &new_block(board_id, BlockType::Card, "outer")).await;
    let inner = insert_one(&th.client, &child_of(&outer, BlockType::Card, "inner")).await;
    let alone = insert_one(&th.client, &new_block(board_id, BlockType::Card, "alone")).await;
    let nested = insert_one(&th.client, &child_of(&alone, BlockType::Card, "nested")).await;
    insert_one(&th.client, &child_of(&alone, BlockType::Comment, "comment")).await;
    let comment = insert_one(&th.client, &child_of(&outer, BlockType::Comment, "said")).await;
    delete(&th.client, &comment).await;
    a_moment().await;
    delete(&th.client, &nested).await;
    a_moment().await;
    delete(&th.client, &outer).await;
    let trash = format!("{TEST_WS}/boards/{board_id}/trash/cards");
    let titles = |listed: &[Value]| -> Vec<Value> {
        listed.iter().map(|entry| entry["title"].clone()).collect()
    };

    let listed: Vec<Value> = th.client.get(&trash).await.ok_json();
    assert_eq!(titles(&listed), [json!("outer"), json!("nested")]);
    for (query, expected) in [
        ("?perPage=1000", vec![json!("outer"), json!("nested")]),
        ("?page=0&perPage=1", vec![json!("outer")]),
        ("?page=5&perPage=1", vec![]),
    ] {
        let listed: Vec<Value> = th.client.get(&format!("{trash}{query}")).await.ok_json();
        assert_eq!(titles(&listed), expected, "{query}");
    }
    for query in [
        "?page=one",
        "?perPage=1.5",
        "?page=-1",
        "?perPage=0",
        "?perPage=1001",
    ] {
        th.client
            .get(&format!("{trash}{query}"))
            .await
            .check_bad_request();
    }

    // The inner card is not listed, and comes back with the outer one.
    th.client
        .post_empty(&format!("{}/restore", block_path(board_id, &outer.id)))
        .await
        .check_ok();
    let blocks = blocks_of(&th.client, board_id).await;
    assert!(
        blocks.iter().any(|block| block.id == inner.id),
        "the inner card is back"
    );
    let listed: Vec<Value> = th.client.get(&trash).await.ok_json();
    assert_eq!(titles(&listed), [json!("nested")]);

    th.client
        .delete(&format!("{TEST_WS}/containers/{board_id}"))
        .await
        .check_no_content();
    th.client
        .get(&trash)
        .await
        .check_error(StatusCode::NOT_FOUND, NO_BOARD);
}
