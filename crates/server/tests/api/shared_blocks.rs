//! The blocks of a shared board as its link reads them: nobody is named, and nothing
//! but the board's blocks is read.

use fvoci_model::{Block, BlockType, Container};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::blocks::{
    NO_BLOCK, NO_BOARD, blocks_of, boards_with_roles, card_with_content, fields, insert_one,
    new_block,
};
use crate::common::{Setup, TEST_WS, TestServer};

/// Fills the board with user identifiers: person properties, a card whose body mentions
/// `me` and whose values name `me` and `editor`, and a view that filters and groups by
/// them.
async fn board_with_people(th: &TestServer, board_id: &str, me: &str, editor: &str) {
    let people = json!([
        {"id": "owner", "name": "Owner", "type": "person"},
        {"id": "workspace", "name": "Workspace", "type": "multiPerson"},
        {"id": "by", "name": "By", "type": "createdBy"},
        {"id": "note", "name": "Note", "type": "text"},
    ]);
    th.client
        .patch(
            &format!("{TEST_WS}/containers/{board_id}"),
            &json!({"updatedCardProperties": people}),
        )
        .await
        .check_ok();
    let mention = json!({"type": "mention", "attrs": {"userId": me}});
    let body = json!({"type": "doc", "content": [{"type": "paragraph", "content": [mention]}]});
    th.client
        .post(
            &format!("{TEST_WS}/boards/{board_id}/cards"),
            &json!({
                "title": "mentions",
                "body": body,
                "properties": {"owner": me, "workspace": [me, editor], "note": me, "gone": me},
            }),
        )
        .await
        .check_created();
    let mut view = new_block(board_id, BlockType::View, "view");
    view.fields = fields(json!({
        "filter": {"operation": "and", "filters": [
            {"propertyId": "owner", "condition": "includes", "values": [me]},
            {"operation": "or", "filters": [
                {"propertyId": "by", "condition": "includes", "values": [me]},
                {"propertyId": "note", "condition": "includes", "values": [me]},
            ]},
        ]},
        "groupBy": "owner",
        "hiddenGroups": [me, ""],
        "collapsedGroups": [editor],
    }));
    insert_one(&th.client, &view).await;
}

/// What the blocks of [`board_with_people`] say about people.
fn people_in(blocks: &Value) -> Value {
    let of_type = |block_type: &str| {
        blocks
            .as_array()
            .into_iter()
            .flatten()
            .find(|block| block["type"] == block_type)
            .cloned()
            .unwrap_or_else(|| panic!("a {block_type} is read: {blocks}"))
    };
    let card = of_type("card");
    let view = of_type("view");
    json!({
        "createdBy": card["createdBy"],
        "mention": card["body"]["content"][0]["content"][0]["attrs"]["userId"],
        "properties": card["fields"]["properties"],
        "filter": view["fields"]["filter"],
        "hidden": view["fields"]["hiddenGroups"],
        "collapsed": view["fields"]["collapsedGroups"],
        "viewBy": view["modifiedBy"],
    })
}

/// Whoever reads a shared board by its link, without a session or signed in, a member
/// of the board too, is told no user's identifier: not who wrote a block, nor who its
/// body mentions, nor whom person properties name in values, filters and the groups of
/// views. A member reading the board's own routes is.
#[tokio::test]
async fn a_reader_by_read_token_is_not_told_who_is_mentioned() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let boards = boards_with_roles(&th).await;
    let me = th.client.me().await;
    let editor = boards.editor.me().await;
    board_with_people(&th, &boards.board.id, &me.id, &editor.id).await;
    let read_token = th.share(&boards.board.id).await;
    let path = format!("/shared/{}/blocks?readToken={read_token}", boards.board.id);
    let hidden = json!({
        "createdBy": "",
        "mention": "",
        "properties": {"owner": "", "workspace": ["", ""], "note": me.id},
        "filter": {"operation": "and", "filters": [
            {"propertyId": "owner", "condition": "includes", "values": [""]},
            {"operation": "or", "filters": [
                {"propertyId": "by", "condition": "includes", "values": [""]},
                {"propertyId": "note", "condition": "includes", "values": [me.id]},
            ]},
        ]},
        "hidden": [""],
        "collapsed": [],
        "viewBy": "",
    });

    // As JSON: the answer is a shape that is only sent, which the allowlist of a write
    // refuses.
    let blocks = th.anonymous().get(&path).await.value();
    assert_eq!(people_in(&blocks), hidden, "nobody, to the token alone");
    for client in [&th.client2, &th.client] {
        let blocks = client.get(&path).await.value();
        assert_eq!(
            people_in(&blocks),
            hidden,
            "nobody, to whoever is signed in"
        );
    }
    let own = blocks_of(&th.client, &boards.board.id).await;
    let shown = people_in(&serde_json::to_value(own).expect("blocks are JSON"));
    assert_eq!(shown["mention"], json!(me.id), "the user, to a member");
    assert_eq!(
        shown["properties"]["workspace"],
        json!([me.id, editor.id]),
        "{shown}"
    );
    assert_eq!(shown["properties"]["gone"], json!(me.id), "{shown}");
    assert_eq!(
        shown["filter"]["filters"][0]["values"],
        json!([me.id]),
        "{shown}"
    );
    assert_eq!(shown["hidden"], json!([me.id, ""]), "{shown}");
    th.teardown().await;
}

/// A shared board's link lets anyone read its blocks, and nothing else.
#[tokio::test]
async fn get_blocks_with_a_read_token() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let boards = boards_with_roles(&th).await;
    let content = card_with_content(&th, &boards).await;
    let read_token = th.share(&boards.board.id).await;
    let anonymous = th.anonymous();
    let with_token =
        |board: &Container| format!("/shared/{}/blocks?readToken={read_token}", board.id);

    let blocks: Vec<Block> = anonymous.get(&with_token(&boards.board)).await.ok_json();
    let mut kinds: Vec<BlockType> = blocks.iter().map(|block| block.block_type).collect();
    kinds.sort_unstable_by_key(ToString::to_string);
    assert_eq!(
        kinds,
        [BlockType::Attachment, BlockType::Card, BlockType::Comment],
        "the token reads the board"
    );

    // A signed-in user who is no member reads with the token too.
    let blocks: Vec<Block> = th.client2.get(&with_token(&boards.board)).await.ok_json();
    assert_eq!(blocks.len(), 3, "the token counts for a non-member");

    let wrong = format!("/shared/{}/blocks?readToken=wrong", boards.board.id);
    for client in [&anonymous, &th.client2] {
        client
            .get(&wrong)
            .await
            .check_error(StatusCode::NOT_FOUND, NO_BOARD);
    }

    // The token of one board opens no other board, by path or by block identifier.
    anonymous
        .get(&with_token(&boards.other_board))
        .await
        .check_error(StatusCode::NOT_FOUND, NO_BOARD);
    let response = anonymous
        .get(&format!(
            "{}&blockId={}",
            with_token(&boards.board),
            content.other_card.id
        ))
        .await;
    response.check_error(StatusCode::NOT_FOUND, NO_BLOCK);

    // It does not let anyone write.
    anonymous
        .post(
            &format!(
                "{TEST_WS}/boards/{}/cards?readToken={read_token}",
                boards.board.id
            ),
            &json!({"title": "by the link"}),
        )
        .await
        .check_unauthorized();

    th.teardown().await;
}
