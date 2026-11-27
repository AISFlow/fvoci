//! The bodies of cards: one document per card, written and read through the card
//! routes, checked before it is stored, and guarded against overwriting a change the
//! writer has not seen.

use fvoci_model::{BLOCK_TITLE_MAX_CHARS, Card, ContainerVisibility};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::common::TEST_WS;
use crate::common::{ApiResponse, Client, Setup, TEST_WORKSPACE_ID};

fn paragraph(text: &str) -> Value {
    json!({"type": "paragraph", "content": [{"type": "text", "text": text}]})
}

/// A task item; `checked` is the JSON of its attribute.
fn task(text: &str, checked: &Value) -> Value {
    json!({"type": "taskItem", "attrs": {"checked": checked}, "content": [paragraph(text)]})
}

/// A document with a paragraph and a task list of two items, one checked.
fn document() -> Value {
    json!({"type": "doc", "content": [
        paragraph("Plan"),
        {"type": "taskList", "content": [task("one", &json!(true)), task("two", &json!(false))]},
    ]})
}

/// Checks that the answer is a 400 that names the member it refuses.
#[track_caller]
fn check_refused_member(response: &ApiResponse, field: &str) {
    response.check_status(StatusCode::BAD_REQUEST);
    assert_eq!(
        response.refused_at(),
        format!("/{field}"),
        "{}",
        response.text()
    );
}

async fn new_card(client: &Client, board_id: &str, card: &Value) -> ApiResponse {
    client
        .post(&format!("{TEST_WS}/boards/{board_id}/cards"), card)
        .await
}

async fn card(client: &Client, card_id: &str) -> Card {
    client
        .get(&format!("{TEST_WS}/cards/{card_id}"))
        .await
        .ok_json()
}

/// What the list of cards says of a card's body: whether it has one, and its tasks,
/// done and all. The list reads them from what was stored with the body.
async fn listed(client: &Client, board_id: &str, card_id: &str) -> (bool, u64, u64) {
    let cards = client
        .get(&format!("{TEST_WS}/boards/{board_id}/cards"))
        .await
        .value();
    let Value::Array(cards) = cards else {
        panic!("a list: {cards}");
    };
    let card = cards
        .into_iter()
        .find(|card| card["id"] == json!(card_id))
        .unwrap_or_else(|| panic!("{card_id} is listed"));
    let count = |name: &str| {
        card[name]
            .as_u64()
            .unwrap_or_else(|| panic!("{name}: {card}"))
    };
    (
        card["hasBody"].as_bool().expect("hasBody"),
        count("tasksTotal"),
        count("tasksDone"),
    )
}

/// A card made with a body is read back with it; the list of cards says that it has
/// one and counts its tasks, without the body.
#[tokio::test]
async fn a_card_is_made_with_its_body_and_listed_without_it() {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;

    let created: Card = new_card(
        &th.client,
        &board.id,
        &json!({"title": "With a body", "body": document()}),
    )
    .await
    .created_json();
    assert_eq!(
        created.body.as_ref().map(fvoci_model::Body::document),
        Some(&document())
    );
    assert!(created.has_body, "{created:?}");
    assert_eq!((created.tasks_total, created.tasks_done), (2, 1));
    let read = card(&th.client, &created.id).await;
    assert_eq!(read, created, "the card is stored as answered");

    let empty: Card = new_card(
        &th.client,
        &board.id,
        &json!({"title": "Empty", "body": {"type": "doc", "content": [{"type": "paragraph"}]}}),
    )
    .await
    .created_json();
    assert_eq!(empty.body, None, "an empty document is no body");
    assert!(!empty.has_body);

    let listed = th
        .client
        .get(&format!("{TEST_WS}/boards/{}/cards", board.id))
        .await
        .value();
    let listed = listed.as_array().expect("a list");
    let with_body = listed
        .iter()
        .find(|card| card["id"] == json!(created.id))
        .expect("the card is listed");
    assert_eq!(with_body["body"], Value::Null, "{with_body}");
    assert_eq!(with_body["hasBody"], json!(true), "{with_body}");
    assert_eq!(with_body["tasksTotal"], json!(2), "{with_body}");
    assert_eq!(with_body["tasksDone"], json!(1), "{with_body}");

    th.teardown().await;
}

/// A body is changed with the `updatedAt` of the card as the writer read it; with
/// another one the patch is refused and changes nothing.
#[tokio::test]
async fn a_body_is_changed_only_from_the_card_as_it_is() {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let created: Card = new_card(&th.client, &board.id, &json!({"title": "Card"}))
        .await
        .created_json();
    let path = format!("{TEST_WS}/cards/{}", created.id);

    let response = th.client.patch(&path, &json!({"body": document()})).await;
    check_refused_member(&response, "readAt");

    let patched: Card = th
        .client
        .patch(
            &path,
            &json!({"body": document(), "readAt": created.updated_at}),
        )
        .await
        .ok_json();
    assert!(patched.has_body, "{patched:?}");
    assert_eq!(
        patched.body.as_ref().map(fvoci_model::Body::document),
        Some(&document())
    );
    assert_eq!(
        listed(&th.client, &board.id, &created.id).await,
        (true, 2, 1),
        "the list reads what the patch stored"
    );

    // The first version is no longer the card's.
    let stale = th
        .client
        .patch(
            &path,
            &json!({"body": null, "readAt": created.updated_at, "title": "Stale"}),
        )
        .await;
    stale.check_status(StatusCode::CONFLICT);
    assert_eq!(stale.problem_type(), "/problems/conflict");
    assert_eq!(
        card(&th.client, &created.id).await,
        patched,
        "a refused patch changes nothing"
    );
    // A patch that could not be stored anyway is refused for that first.
    let too_long = "a".repeat(BLOCK_TITLE_MAX_CHARS + 1);
    let invalid = th
        .client
        .patch(
            &path,
            &json!({"title": too_long, "readAt": created.updated_at}),
        )
        .await;
    invalid.check_status(StatusCode::BAD_REQUEST);

    // `null` takes the body away; a title alone needs no `readAt`.
    let cleared: Card = th
        .client
        .patch(&path, &json!({"body": null, "readAt": patched.updated_at}))
        .await
        .ok_json();
    assert_eq!(cleared.body, None);
    assert!(!cleared.has_body);
    assert_eq!((cleared.tasks_total, cleared.tasks_done), (0, 0));
    assert_eq!(
        listed(&th.client, &board.id, &created.id).await,
        (false, 0, 0)
    );
    th.client
        .patch(&path, &json!({"title": "Renamed"}))
        .await
        .check_ok();

    th.teardown().await;
}

/// A body that may not be stored is a 400 that names `body`, or the node of it that is
/// refused, on both routes.
#[tokio::test]
async fn a_body_that_is_no_document_is_refused() {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let created: Card = new_card(&th.client, &board.id, &json!({"title": "Card"}))
        .await
        .created_json();

    for (body, place) in [
        (json!("text"), "body"),
        (json!({"type": "doc", "content": []}), "body"),
        (
            json!({"type": "doc", "content": [{"type": "paragraph", "extra": true}]}),
            "body/content/0",
        ),
    ] {
        let response = new_card(&th.client, &board.id, &json!({"body": body})).await;
        check_refused_member(&response, place);
        let response = th
            .client
            .patch(
                &format!("{TEST_WS}/cards/{}", created.id),
                &json!({"body": body, "readAt": created.updated_at}),
            )
            .await;
        check_refused_member(&response, place);
    }
    assert_eq!(
        card(&th.client, &created.id).await,
        created,
        "nothing was stored"
    );

    th.teardown().await;
}

/// Every change of a card moves its `updatedAt` on, so a `readAt` from before a change
/// is refused even when the change came in the same millisecond, and also when the
/// patch carries no body.
#[tokio::test]
async fn a_read_at_from_before_a_change_is_refused() {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let created: Card = new_card(
        &th.client,
        &board.id,
        &json!({"title": "Card", "body": null}),
    )
    .await
    .created_json();
    assert_eq!(created.body, None, "`null` is no body");
    let path = format!("{TEST_WS}/cards/{}", created.id);

    // Two patches made from the same read, one right after the other.
    let first: Card = th
        .client
        .patch(
            &path,
            &json!({"body": document(), "readAt": created.updated_at}),
        )
        .await
        .ok_json();
    assert!(
        first.updated_at > created.updated_at,
        "{} after {}",
        first.updated_at,
        created.updated_at
    );
    let second = th
        .client
        .patch(
            &path,
            &json!({"title": "Second", "readAt": created.updated_at}),
        )
        .await;
    second.check_status(StatusCode::CONFLICT);
    assert_eq!(card(&th.client, &created.id).await, first, "unchanged");

    // The routes of blocks check `readAt` too.
    let block_path = format!("{TEST_WS}/boards/{}/blocks/{}", board.id, created.id);
    let stale = th
        .client
        .patch(
            &block_path,
            &json!({"title": "Stale", "readAt": created.updated_at}),
        )
        .await;
    stale.check_status(StatusCode::CONFLICT);
    let stale = th
        .client
        .patch(
            &block_path,
            &json!({"body": null, "readAt": created.updated_at}),
        )
        .await;
    stale.check_status(StatusCode::CONFLICT);
    assert_eq!(card(&th.client, &created.id).await, first, "unchanged");

    th.teardown().await;
}

/// A body goes to the trash with its card and comes back with it, and a copy of the
/// card has it too.
#[tokio::test]
async fn a_body_comes_back_from_the_trash_and_is_copied() {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let created: Card = new_card(
        &th.client,
        &board.id,
        &json!({"title": "Card", "body": document()}),
    )
    .await
    .created_json();
    let block_path = format!("{TEST_WS}/boards/{}/blocks/{}", board.id, created.id);

    th.client.delete(&block_path).await.check_no_content();
    th.client
        .post_empty(&format!("{block_path}/restore"))
        .await
        .check_ok();
    let restored = card(&th.client, &created.id).await;
    assert_eq!(restored.body, created.body, "{restored:?}");
    assert_eq!((restored.tasks_total, restored.tasks_done), (2, 1));
    assert_eq!(
        listed(&th.client, &board.id, &created.id).await,
        (true, 2, 1),
        "the list reads what was restored"
    );

    let copies: Vec<Value> = th
        .client
        .post_empty(&format!("{block_path}/duplicate"))
        .await
        .created_json();
    let copy = copies
        .iter()
        .find(|block| block["type"] == json!("card"))
        .expect("the copy of the card");
    assert_eq!(copy["body"], document(), "{copy}");
    let copy_id = copy["id"].as_str().expect("an id");
    let copy = card(&th.client, copy_id).await;
    assert_eq!((copy.tasks_total, copy.tasks_done), (2, 1));
    assert_eq!(
        listed(&th.client, &board.id, copy_id).await,
        (true, 2, 1),
        "the list reads what was copied"
    );

    th.teardown().await;
}

/// A card made through `createBlocks` with a body; `body` is that of the block.
fn card_block(board_id: &str, body: &Value) -> Value {
    json!({"id": "b1", "containerId": board_id, "type": "card", "createdAt": 1, "updatedAt": 1,
           "body": body})
}

/// A document with an image of the file of that stored name.
fn with_image(file_id: &str) -> Value {
    with_node("image", file_id)
}

/// A document with a node of `kind` (an image, a file block) naming the file of that
/// stored name.
fn with_node(kind: &str, file_id: &str) -> Value {
    json!({"type": "doc", "content": [
        paragraph("Picture"),
        {"type": kind, "attrs": {"fileId": file_id}},
    ]})
}

/// The routes of blocks take the body of a card as the routes of cards do: checked,
/// refused as `body`, changed only with `readAt`, and refused on other blocks.
#[tokio::test]
async fn the_routes_of_blocks_take_the_body_of_a_card() {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let blocks = format!("{TEST_WS}/boards/{}/blocks", board.id);

    // Made with a body, which is checked and counted.
    let made: Card = new_card(
        &th.client,
        &board.id,
        &json!({"title": "Card", "body": document()}),
    )
    .await
    .created_json();
    let card_id = made.id.clone();
    assert_eq!(
        made.body.as_ref().map(fvoci_model::Body::document),
        Some(&document()),
        "{made:?}"
    );
    assert_eq!(listed(&th.client, &board.id, &card_id).await, (true, 2, 1));
    // Changed with the `updatedAt` it was read at, through each route of patches.
    let path = format!("{blocks}/{card_id}");
    let read = card(&th.client, &card_id).await;
    let response = th.client.patch(&path, &json!({"body": null})).await;
    check_refused_member(&response, "readAt");
    let response = th
        .client
        .patch(
            &path,
            &json!({"body": {"type": "doc"}, "readAt": read.updated_at}),
        )
        .await;
    check_refused_member(&response, "body");
    th.client
        .patch(&path, &json!({"body": null, "readAt": read.updated_at}))
        .await
        .check_no_content();
    assert_eq!(listed(&th.client, &board.id, &card_id).await, (false, 0, 0));
    let stale = th
        .client
        .patch(
            &path,
            &json!({"body": document(), "readAt": read.updated_at}),
        )
        .await;
    stale.check_status(StatusCode::CONFLICT);

    let read = card(&th.client, &card_id).await;
    let batch = json!({"blockIds": [card_id],
                       "blockPatches": [{"body": document(), "readAt": read.updated_at}]});
    th.client.patch(&blocks, &batch).await.check_no_content();
    assert_eq!(card(&th.client, &card_id).await.body, Some(document_body()));
    let batch = json!({"blockIds": [card_id], "blockPatches": [{"body": null}]});
    check_refused_member(
        &th.client.patch(&blocks, &batch).await,
        "blockPatches/0/readAt",
    );

    let read = card(&th.client, &card_id).await;
    let request = json!({
        "containerIds": [board.id], "containerPatches": [{"title": "n"}],
        "blockIds": [card_id], "blockPatches": [{"body": null, "readAt": read.updated_at}],
    });
    th.client
        .patch(&format!("{TEST_WS}/containers-and-blocks"), &request)
        .await
        .check_ok();
    assert_eq!(card(&th.client, &card_id).await.body, None);

    // A new board and its cards, with a body.
    let made: Value = th
        .client
        .post(
            &format!("{TEST_WS}/containers-and-blocks"),
            &json!({
                "containers": [{"id": "bd", "workspaceId": TEST_WORKSPACE_ID, "visibility": "private"}],
                "blocks": [card_block("bd", &document())],
            }),
        )
        .await
        .created_json();
    assert_eq!(made["blocks"][0]["body"], document(), "{made}");

    th.teardown().await;
}

fn document_body() -> fvoci_model::Body {
    fvoci_model::Body::parse(document())
        .expect("allowed")
        .expect("not empty")
}

/// An image of a body shows a file of the card's board, on every route that takes one.
#[tokio::test]
async fn the_images_of_a_body_are_files_of_its_board() {
    files_of_a_body_are_files_of_its_board("image").await;
}

#[tokio::test]
async fn the_file_blocks_of_a_body_are_files_of_its_board() {
    files_of_a_body_are_files_of_its_board("file").await;
}

async fn files_of_a_body_are_files_of_its_board(kind: &str) {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let other = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let file_id = crate::files::uploaded_id(
        &crate::files::upload(&th.client, &board.id, "picture.png", b"png").await,
    );
    let elsewhere = crate::files::uploaded_id(
        &crate::files::upload(&th.client, &other.id, "other.png", b"png").await,
    );

    // A file of another board, or none, is refused on every route that takes a body.
    for missing in [elsewhere.as_str(), "7nofile.png"] {
        let response = new_card(
            &th.client,
            &board.id,
            &json!({"title": "Card", "body": with_node(kind, missing)}),
        )
        .await;
        check_refused_member(&response, "body");
    }
    let created: Card = new_card(
        &th.client,
        &board.id,
        &json!({"title": "Card", "body": with_node(kind, &file_id)}),
    )
    .await
    .created_json();
    for missing in [elsewhere.as_str(), "7nofile.png"] {
        let response = th
            .client
            .post(
                &format!("{TEST_WS}/boards/{}/blocks", board.id),
                &[json!({
                    "id": "c1",
                    "containerId": board.id,
                    "parentId": created.id,
                    "type": "comment",
                    "body": with_node(kind, missing),
                })],
            )
            .await;
        check_refused_member(&response, "0/body");
    }
    let response = th
        .client
        .patch(
            &format!("{TEST_WS}/cards/{}", created.id),
            &json!({"body": with_node(kind, &elsewhere), "readAt": created.updated_at}),
        )
        .await;
    check_refused_member(&response, "body");

    // Each route of patches refuses it too.
    let block_path = format!("{TEST_WS}/boards/{}/blocks/{}", board.id, created.id);
    let patch = json!({"body": with_node(kind, &elsewhere), "readAt": created.updated_at});
    check_refused_member(&th.client.patch(&block_path, &patch).await, "body");
    let batch = json!({"blockIds": [created.id], "blockPatches": [patch]});
    check_refused_member(
        &th.client
            .patch(&format!("{TEST_WS}/boards/{}/blocks", board.id), &batch)
            .await,
        "blockPatches/0/body",
    );
    let request = json!({
        "containerIds": [board.id], "containerPatches": [{"title": "n"}],
        "blockIds": [created.id], "blockPatches": [patch],
    });
    check_refused_member(
        &th.client
            .patch(&format!("{TEST_WS}/containers-and-blocks"), &request)
            .await,
        "blockPatches/0/body",
    );
    // A new board has no files.
    let response = th
        .client
        .post(
            &format!("{TEST_WS}/containers-and-blocks"),
            &json!({
                "containers": [{"id": board.id, "workspaceId": TEST_WORKSPACE_ID, "visibility": "private"}],
                "blocks": [card_block(&board.id, &with_node(kind, &file_id))],
            }),
        )
        .await;
    check_refused_member(&response, "blocks/0/body");
    assert_eq!(card(&th.client, &created.id).await, created, "unchanged");

    th.teardown().await;
}

/// What the routes of blocks refuse of bodies: what may not be stored, a body on a block
/// that is no card, and a card with a body that would become another block. A refused
/// request stores nothing.
#[tokio::test]
async fn the_routes_of_blocks_refuse_what_a_body_may_not_be() {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let blocks = format!("{TEST_WS}/boards/{}/blocks", board.id);
    let made: Card = new_card(
        &th.client,
        &board.id,
        &json!({"title": "Card", "body": document()}),
    )
    .await
    .created_json();
    let card_id = made.id;

    // What may not be stored, and a body on a block that cannot hold one.
    for (block, field) in [
        (
            json!({"id": "c1", "containerId": board.id, "parentId": card_id, "type": "comment",
                   "body": "attachment"}),
            "0/body",
        ),
        (
            json!({"id": "c2", "containerId": board.id, "parentId": card_id, "type": "comment",
                   "body": {"type": "doc", "content": [{"type": "table"}]}}),
            "0/body/content/0",
        ),
        (
            json!({"id": "t1", "containerId": board.id, "parentId": card_id, "type": "attachment",
                   "body": document()}),
            "0/body",
        ),
    ] {
        let response = th.client.post(&blocks, &[block]).await;
        check_refused_member(&response, field);
    }
    let response = th
        .client
        .post(
            &format!("{TEST_WS}/containers-and-blocks"),
            &json!({
                "containers": [{"id": "bd", "workspaceId": TEST_WORKSPACE_ID, "visibility": "private"}],
                "blocks": [card_block("bd", &json!("attachment"))],
            }),
        )
        .await;
    check_refused_member(&response, "blocks/0/body");
    // A card's type is fixed at creation and may not become another block.
    let response = th
        .client
        .patch(
            &format!("{blocks}/{card_id}"),
            &json!({"type": "attachment"}),
        )
        .await;
    check_refused_member(&response, "type");
    // Nothing of the refused requests was stored: no block, no board.
    let stored = th.client.get(&blocks).await.value();
    assert_eq!(
        stored.as_array().map(Vec::len),
        Some(1),
        "only the card: {stored}"
    );
    let boards = th.client.get(&format!("{TEST_WS}/boards")).await.value();
    assert_eq!(boards.as_array().map(Vec::len), Some(1), "{boards}");

    th.teardown().await;
}

/// A copy of a card, made from its board or of it or of its board, shows a copy of each
/// file of its body, and is answered as it is stored.
#[tokio::test]
async fn a_copy_of_a_body_shows_copies_of_its_files() {
    Box::pin(copies_of_a_body_show_copies_of_its_files("image")).await;
}

#[tokio::test]
async fn a_copy_of_a_body_shows_copies_of_its_file_blocks() {
    Box::pin(copies_of_a_body_show_copies_of_its_files("file")).await;
}

#[expect(
    clippy::too_many_lines,
    reason = "the three ways a card is copied, each checked for its files"
)]
async fn copies_of_a_body_show_copies_of_its_files(kind: &str) {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let other = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let file_id = crate::files::uploaded_id(
        &crate::files::upload(&th.client, &board.id, "picture.png", b"png").await,
    );
    let created: Card = new_card(
        &th.client,
        &board.id,
        &json!({"title": "Card", "body": with_node(kind, &file_id)}),
    )
    .await
    .created_json();

    // Blocks made from another board's (`sourceBoardId` for comments, `copy-to-workspace` for cards):
    if kind == "image" {
        let other_card: Card = new_card(&th.client, &other.id, &json!({"title": "Target"}))
            .await
            .created_json();
        let response = th
            .client
            .post(
                &format!(
                    "{TEST_WS}/boards/{}/blocks?sourceBoardId={}",
                    other.id, board.id
                ),
                &[json!({
                    "id": "c1",
                    "containerId": other.id,
                    "parentId": other_card.id,
                    "type": "comment",
                    "body": with_node(kind, &file_id),
                })],
            )
            .await;
        let made: Vec<Value> = response.created_json();
        let moved = made[0]["body"]["content"][1]["attrs"]["fileId"]
            .as_str()
            .expect("a file")
            .to_owned();
        assert_ne!(moved, file_id, "a copy of the file");
        th.client
            .get(&format!("{TEST_WS}/containers/{}/files/{moved}", other.id))
            .await
            .check_ok();
    } else {
        let copied_ws: Value = th
            .client
            .post(
                &format!("{TEST_WS}/containers/{}/copy-to-workspace", board.id),
                &json!({"targetWorkspaceId": TEST_WORKSPACE_ID}),
            )
            .await
            .created_json();
        let copied_board_id = copied_ws["container"]["id"]
            .as_str()
            .expect("copied board id");
        let copied_blocks: Vec<Value> = th
            .client
            .get(&format!(
                "{TEST_WS}/boards/{copied_board_id}/blocks?type=card"
            ))
            .await
            .ok_json();
        let moved = copied_blocks[0]["body"]["content"][1]["attrs"]["fileId"]
            .as_str()
            .expect("a file")
            .to_owned();
        assert_ne!(moved, file_id, "a copy of the file");
        th.client
            .get(&format!(
                "{TEST_WS}/containers/{copied_board_id}/files/{moved}"
            ))
            .await
            .check_ok();
    }

    // The copy of the card shows a copy of the file, a file of the same board.
    let copies: Vec<Value> = th
        .client
        .post_empty(&format!(
            "{TEST_WS}/boards/{}/blocks/{}/duplicate",
            board.id, created.id
        ))
        .await
        .created_json();
    let copy = copies
        .iter()
        .find(|block| block["type"] == json!("card"))
        .expect("the copy of the card");
    let copied_file = copy["body"]["content"][1]["attrs"]["fileId"]
        .as_str()
        .expect("the image of the copy names a file")
        .to_owned();
    assert_ne!(copied_file, file_id, "the copy shows a file of its own");
    let stored = card(&th.client, copy["id"].as_str().expect("an id")).await;
    assert_eq!(
        stored.body.as_ref().map(fvoci_model::Body::file_ids),
        Some(vec![copied_file.as_str()]),
        "the stored copy is what was answered"
    );
    assert_eq!(
        copy["updatedAt"],
        json!(stored.updated_at),
        "the answered copy is the stored one, to be edited from"
    );

    // A copy of the board too.
    let board_copy: Value = th
        .client
        .post_empty(&format!("{TEST_WS}/containers/{}/duplicate", board.id))
        .await
        .created_json();
    let new_board = board_copy["containers"][0]["id"]
        .as_str()
        .expect("a board")
        .to_owned();
    let mut with_images = 0;
    for block in board_copy["blocks"].as_array().expect("blocks") {
        if block["type"] == json!("card") && block["body"].is_object() {
            with_images += 1;
            let name = block["body"]["content"][1]["attrs"]["fileId"]
                .as_str()
                .expect("a file");
            th.client
                .get(&format!("{TEST_WS}/containers/{new_board}/files/{name}"))
                .await
                .check_ok();
            let stored = card(&th.client, block["id"].as_str().expect("an id")).await;
            assert_eq!(block["updatedAt"], json!(stored.updated_at), "{kind}");
        }
    }
    assert_eq!(
        with_images, 2,
        "both cards of the board were copied with their image"
    );
    th.client
        .get(&format!(
            "{TEST_WS}/containers/{}/files/{copied_file}",
            board.id
        ))
        .await
        .check_ok();

    th.teardown().await;
}

/// A copy of a card that shows an image begins with one version of its body, as any
/// copy of a card does: the body as copied, which shows the copy's own file, and none
/// of the source card's versions. The cards of a copy of its board begin without versions.
/// The source card keeps its versions.
#[tokio::test]
async fn a_copy_of_a_card_that_shows_an_image_begins_with_one_version() {
    a_copy_of_a_card_with_a_file_begins_with_one_version("image").await;
}

#[tokio::test]
async fn a_copy_of_a_card_with_a_file_block_begins_with_one_version() {
    a_copy_of_a_card_with_a_file_begins_with_one_version("file").await;
}

async fn a_copy_of_a_card_with_a_file_begins_with_one_version(kind: &str) {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let file_id = crate::files::uploaded_id(
        &crate::files::upload(&th.client, &board.id, "picture.png", b"png").await,
    );
    let created: Card = new_card(
        &th.client,
        &board.id,
        &json!({"title": "Card", "body": paragraph_doc("before")}),
    )
    .await
    .created_json();
    th.client
        .patch(
            &format!("{TEST_WS}/cards/{}", created.id),
            &json!({"body": with_node(kind, &file_id), "readAt": created.updated_at}),
        )
        .await
        .check_ok();
    let kept = crate::body_versions::versions(&th.client, &created.id).await;
    assert_eq!(
        kept.len(),
        2,
        "the body before and the one written: {kept:?}"
    );

    let copies: Vec<Value> = th
        .client
        .post_empty(&format!(
            "{TEST_WS}/boards/{}/blocks/{}/duplicate",
            board.id, created.id
        ))
        .await
        .created_json();
    let copy = copies
        .iter()
        .find(|block| block["type"] == json!("card"))
        .and_then(|block| block["id"].as_str())
        .expect("the copy of the card");
    let stored = card(&th.client, copy).await;
    let shown = stored.body.as_ref().map(fvoci_model::Body::file_ids);
    assert!(
        shown
            .as_ref()
            .is_some_and(|files| files.len() == 1 && files[0] != file_id),
        "the copy shows a copy of the file: {shown:?}"
    );
    let of_copy = crate::body_versions::versions(&th.client, copy).await;
    assert_eq!(
        crate::body_versions::said(&of_copy),
        [(1, Vec::<&str>::new(), true)],
        "one version, the copy's body"
    );
    let version: Value = th
        .client
        .get(&format!("{TEST_WS}/cards/{copy}/body-versions/1"))
        .await
        .ok_json();
    assert_eq!(
        version["body"]["content"][1]["attrs"]["fileId"].as_str(),
        shown.as_ref().and_then(|files| files.first().copied()),
        "the version shows the copy's file, not the source card's"
    );

    let board_copy: Value = th
        .client
        .post_empty(&format!("{TEST_WS}/containers/{}/duplicate", board.id))
        .await
        .created_json();
    let mut cards = 0;
    for block in board_copy["blocks"].as_array().expect("blocks") {
        if block["type"] == json!("card") && block["body"].is_object() {
            cards += 1;
            let id = block["id"].as_str().expect("an id");
            assert_eq!(
                crate::body_versions::versions(&th.client, id).await,
                Vec::<Value>::new(),
                "a card of a copy of the board begins without versions"
            );
            let answered = block["body"]["content"][1]["attrs"]["fileId"].as_str();
            let stored = card(&th.client, id).await;
            assert_eq!(
                stored.body.as_ref().map(fvoci_model::Body::file_ids),
                answered.map(|name| vec![name]),
                "the stored copy shows the copied file it was answered with"
            );
            assert_ne!(answered, Some(file_id.as_str()), "a copy of the file");
        }
    }
    assert_eq!(cards, 2, "both cards of the board were copied");
    assert_eq!(
        crate::body_versions::versions(&th.client, &created.id).await,
        kept,
        "the original keeps its versions"
    );
    th.teardown().await;
}

/// A card with an attachment below it is copied with the attachment pointed at a file of
/// its own, which is written as a patch of the attachment: the copy of the card begins
/// with one version of its body all the same.
#[tokio::test]
async fn a_copy_of_a_card_with_an_attachment_below_it_begins_with_one_version() {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let file_id = crate::files::uploaded_id(
        &crate::files::upload(&th.client, &board.id, "a.pdf", b"pdf").await,
    );
    let created: Card = new_card(
        &th.client,
        &board.id,
        &json!({"title": "Card", "body": paragraph_doc("the body")}),
    )
    .await
    .created_json();
    let mut fields = serde_json::Map::new();
    fields.insert("fileId".to_owned(), json!(file_id));
    let attachment = fvoci_model::Block {
        id: fvoci_model::new_id(),
        parent_id: created.id.clone(),
        container_id: board.id.clone(),
        block_type: fvoci_model::BlockType::Attachment,
        fields,
        created_at: 1,
        updated_at: 1,
        ..fvoci_model::Block::default()
    };
    th.client
        .insert_blocks(&board.id, &[attachment])
        .await
        .check_created();

    let copies: Vec<Value> = th
        .client
        .post_empty(&format!(
            "{TEST_WS}/boards/{}/blocks/{}/duplicate",
            board.id, created.id
        ))
        .await
        .created_json();
    let copied_file = copies
        .iter()
        .find(|block| block["type"] == json!("attachment"))
        .and_then(|block| block["fields"]["fileId"].as_str())
        .expect("the copy of the attachment");
    assert_ne!(copied_file, file_id, "a file of its own");
    let copy = copies
        .iter()
        .find(|block| block["type"] == json!("card"))
        .and_then(|block| block["id"].as_str())
        .expect("the copy of the card");
    let of_copy = crate::body_versions::versions(&th.client, copy).await;
    assert_eq!(
        crate::body_versions::said(&of_copy),
        [(1, Vec::<&str>::new(), true)],
        "one version, the copy's body"
    );
    th.teardown().await;
}

/// The copies of files that copying a card makes are shown by the copies and are not
/// collected, however much later; a file that was uploaded and never shown is.
#[tokio::test]
async fn copied_files_are_shown_and_an_unused_upload_is_collected() {
    copied_files_are_shown_and_unused_ones_collected("image").await;
}

#[tokio::test]
async fn copied_files_of_file_blocks_are_shown_and_an_unused_upload_is_collected() {
    copied_files_are_shown_and_unused_ones_collected("file").await;
}

async fn copied_files_are_shown_and_unused_ones_collected(kind: &str) {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let other = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let file_id = crate::files::uploaded_id(
        &crate::files::upload(&th.client, &board.id, "picture.png", b"png").await,
    );
    let unused = crate::files::uploaded_id(
        &crate::files::upload(&th.client, &board.id, "unused.png", b"png").await,
    );
    let created: Card = new_card(
        &th.client,
        &board.id,
        &json!({"title": "Card", "body": with_node(kind, &file_id)}),
    )
    .await
    .created_json();
    let copy: Value = th
        .client
        .post_empty(&format!(
            "{TEST_WS}/boards/{}/blocks/{}/duplicate",
            board.id, created.id
        ))
        .await
        .created_json();
    let shown = |block: &Value| {
        block["body"]["content"][1]["attrs"]["fileId"]
            .as_str()
            .expect("a file")
            .to_owned()
    };
    let other_copy = copy_to_other_board(&th.client, &board.id, &other.id, kind, &file_id).await;
    let copies = [(board.id.clone(), shown(&copy[0])), other_copy];

    let collected = th
        .app
        .store()
        .collect_unreferenced_files(i64::MAX)
        .await
        .expect("the files no block shows are collected");
    let names: Vec<&str> = collected
        .iter()
        .map(|file| file.stored_name.as_str())
        .collect();
    assert_eq!(
        names,
        [unused.as_str()],
        "only the upload that nothing shows"
    );
    for (board_id, name) in std::iter::once((board.id.clone(), file_id)).chain(copies) {
        th.client
            .get(&format!("{TEST_WS}/containers/{board_id}/files/{name}"))
            .await
            .check_ok();
    }

    th.teardown().await;
}

async fn copy_to_other_board(
    client: &Client,
    board_id: &str,
    other_id: &str,
    kind: &str,
    file_id: &str,
) -> (String, String) {
    let shown = |block: &Value| {
        block["body"]["content"][1]["attrs"]["fileId"]
            .as_str()
            .expect("a file")
            .to_owned()
    };
    if kind == "image" {
        let other_card: Card = new_card(client, other_id, &json!({"title": "Target"}))
            .await
            .created_json();
        let made: Vec<Value> = client
            .post(
                &format!("{TEST_WS}/boards/{other_id}/blocks?sourceBoardId={board_id}"),
                &[json!({
                    "id": "c1",
                    "containerId": other_id,
                    "parentId": other_card.id,
                    "type": "comment",
                    "body": with_node(kind, file_id),
                })],
            )
            .await
            .created_json();
        (other_id.to_owned(), shown(&made[0]))
    } else {
        let copied_ws: Value = client
            .post(
                &format!("{TEST_WS}/containers/{board_id}/copy-to-workspace"),
                &json!({"targetWorkspaceId": TEST_WORKSPACE_ID}),
            )
            .await
            .created_json();
        let copied_board_id = copied_ws["container"]["id"]
            .as_str()
            .expect("copied board id")
            .to_owned();
        let copied_blocks: Vec<Value> = client
            .get(&format!(
                "{TEST_WS}/boards/{copied_board_id}/blocks?type=card"
            ))
            .await
            .ok_json();
        (copied_board_id, shown(&copied_blocks[0]))
    }
}

/// The block of that identifier, as the routes of blocks list it.
async fn block_of(client: &Client, board_id: &str, block_id: &str) -> Value {
    let blocks: Vec<Value> = client
        .get(&format!("{TEST_WS}/boards/{board_id}/blocks"))
        .await
        .ok_json();
    blocks
        .into_iter()
        .find(|block| block["id"] == json!(block_id))
        .expect("the block is listed")
}

/// A comment has a body as a card does: checked, changed only
/// with `readAt`, its images files of its board, and copied with its card.
#[tokio::test]
async fn a_comment_has_a_body() {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let other = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let blocks = format!("{TEST_WS}/boards/{}/blocks", board.id);
    let file_id = crate::files::uploaded_id(
        &crate::files::upload(&th.client, &board.id, "picture.png", b"png").await,
    );
    let elsewhere = crate::files::uploaded_id(
        &crate::files::upload(&th.client, &other.id, "other.png", b"png").await,
    );
    let card: Card = new_card(&th.client, &board.id, &json!({"title": "Card"}))
        .await
        .created_json();
    let comment = |body: &Value| {
        json!([{"id": "c1", "containerId": board.id, "parentId": card.id, "type": "comment",
                "createdAt": 1, "updatedAt": 1, "body": body}])
    };

    check_refused_member(
        &th.client
            .post(&blocks, &comment(&with_image(&elsewhere)))
            .await,
        "0/body",
    );
    check_refused_member(
        &th.client
            .post(
                &blocks,
                &comment(&json!({"type": "doc", "content": [{"type": "table"}]})),
            )
            .await,
        "0/body/content/0",
    );
    let made: Vec<Value> = th
        .client
        .post(&blocks, &comment(&with_image(&file_id)))
        .await
        .created_json();
    let comment_id = made[0]["id"].as_str().expect("an id").to_owned();
    assert_eq!(made[0]["body"], with_image(&file_id), "{made:?}");
    let stored = block_of(&th.client, &board.id, &comment_id).await;
    assert_eq!(stored["body"], with_image(&file_id), "{stored}");

    // Changed with the `updatedAt` it was read at.
    let path = format!("{blocks}/{comment_id}");
    check_refused_member(
        &th.client.patch(&path, &json!({"body": document()})).await,
        "readAt",
    );
    th.client
        .patch(
            &path,
            &json!({"body": document(), "readAt": stored["updatedAt"]}),
        )
        .await
        .check_no_content();
    assert_eq!(
        block_of(&th.client, &board.id, &comment_id).await["body"],
        document()
    );
    let read = block_of(&th.client, &board.id, &comment_id).await;
    th.client
        .patch(
            &path,
            &json!({"body": with_image(&file_id), "readAt": read["updatedAt"]}),
        )
        .await
        .check_no_content();

    // Copying and moving leave comments behind; a comment created referencing another
    // board's file copies that file into the destination board.
    let other_card: Card = new_card(&th.client, &other.id, &json!({"title": "Card"}))
        .await
        .created_json();
    let made: Vec<Value> = th
        .client
        .post(
            &format!(
                "{TEST_WS}/boards/{}/blocks?sourceBoardId={}",
                other.id, board.id
            ),
            &json!([{"id": "c2", "containerId": other.id, "parentId": other_card.id,
                     "type": "comment", "createdAt": 1, "updatedAt": 1,
                     "body": with_image(&file_id)}]),
        )
        .await
        .created_json();
    let name = made[0]["body"]["content"][1]["attrs"]["fileId"]
        .as_str()
        .expect("a file");
    assert_ne!(name, file_id, "a copy of the file");
    th.client
        .get(&format!("{TEST_WS}/containers/{}/files/{name}", other.id))
        .await
        .check_ok();

    th.teardown().await;
}

/// A comment's body goes every way a card's does: through the batch routes, with a new
/// board, back from the trash; it may not stay on a block that becomes something else;
/// and the file it shows is not collected.
#[tokio::test]
async fn a_comment_body_goes_every_way_a_card_body_does() {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let blocks = format!("{TEST_WS}/boards/{}/blocks", board.id);
    let file_id = crate::files::uploaded_id(
        &crate::files::upload(&th.client, &board.id, "picture.png", b"png").await,
    );
    let card: Card = new_card(&th.client, &board.id, &json!({"title": "Card"}))
        .await
        .created_json();
    let made: Vec<Value> = th
        .client
        .post(
            &blocks,
            &json!([{"id": "c1", "containerId": board.id, "parentId": card.id,
                     "type": "comment", "createdAt": 1, "updatedAt": 1,
                     "body": with_image(&file_id)}]),
        )
        .await
        .created_json();
    let comment_id = made[0]["id"].as_str().expect("an id").to_owned();
    let path = format!("{blocks}/{comment_id}");

    let read = block_of(&th.client, &board.id, &comment_id).await;
    let batch = json!({"blockIds": [comment_id],
                       "blockPatches": [{"body": document(), "readAt": read["updatedAt"]}]});
    th.client.patch(&blocks, &batch).await.check_no_content();
    let read = block_of(&th.client, &board.id, &comment_id).await;
    assert_eq!(read["body"], document(), "{read}");
    let request = json!({
        "containerIds": [board.id], "containerPatches": [{"title": "n"}],
        "blockIds": [comment_id],
        "blockPatches": [{"body": with_image(&file_id), "readAt": read["updatedAt"]}],
    });
    th.client
        .patch(&format!("{TEST_WS}/containers-and-blocks"), &request)
        .await
        .check_ok();
    let read = block_of(&th.client, &board.id, &comment_id).await;
    assert_eq!(read["body"], with_image(&file_id), "{read}");

    // A comment's type is fixed at creation and may not become another block.
    check_refused_member(
        &th.client.patch(&path, &json!({"type": "attachment"})).await,
        "type",
    );
    // Back from the trash with its body.
    th.client.delete(&path).await.check_no_content();
    th.client
        .post_empty(&format!("{path}/restore"))
        .await
        .check_ok();
    let restored = block_of(&th.client, &board.id, &comment_id).await;
    assert_eq!(restored["body"], with_image(&file_id), "{restored}");
    // Its image keeps the file.
    let collected = th
        .app
        .store()
        .collect_unreferenced_files(i64::MAX)
        .await
        .expect("the files no block shows are collected");
    assert!(collected.is_empty(), "{collected:?}");

    // A new board with a comment that has a body.
    let made: Value = th
        .client
        .post(
            &format!("{TEST_WS}/containers-and-blocks"),
            &json!({
                "containers": [{"id": "bd", "workspaceId": TEST_WORKSPACE_ID, "visibility": "private"}],
                "blocks": [
                    {"id": "k1", "containerId": "bd", "type": "card", "createdAt": 1, "updatedAt": 1},
                    {"id": "m1", "containerId": "bd", "parentId": "k1", "type": "comment",
                     "createdAt": 1, "updatedAt": 1, "body": document()},
                ],
            }),
        )
        .await
        .created_json();
    let comment = made["blocks"]
        .as_array()
        .expect("blocks")
        .iter()
        .find(|block| block["type"] == json!("comment"))
        .expect("the comment");
    assert_eq!(comment["body"], document(), "{made}");

    th.teardown().await;
}

/// A commenter posts comments with bodies, as before with titles, and that is all: no
/// change of a comment, and no files copied into the board (the permissions of
/// comments stay).
#[tokio::test]
async fn a_commenter_posts_a_body_but_copies_no_files() {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let source = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let blocks = format!("{TEST_WS}/boards/{}/blocks", board.id);
    let file_id = crate::files::uploaded_id(
        &crate::files::upload(&th.client, &board.id, "picture.png", b"png").await,
    );
    let source_file = crate::files::uploaded_id(
        &crate::files::upload(&th.client, &source.id, "source.png", b"png").await,
    );
    let user2 = th.client2.me().await;
    for (board_id, role) in [
        (&board.id, fvoci_model::ContainerRole::Commenter),
        (&source.id, fvoci_model::ContainerRole::Viewer),
    ] {
        th.app
            .add_member_to_container(&fvoci_model::ContainerMember::new(
                board_id, &user2.id, role,
            ))
            .await
            .expect("user2 is a member");
    }
    let card: Card = new_card(&th.client, &board.id, &json!({"title": "Card"}))
        .await
        .created_json();
    let comment = |id: &str, body: &Value| {
        json!([{"id": id, "containerId": board.id, "parentId": card.id, "type": "comment",
                "createdAt": 1, "updatedAt": 1, "body": body}])
    };

    let made: Vec<Value> = th
        .client2
        .post(&blocks, &comment("c1", &with_image(&file_id)))
        .await
        .created_json();
    assert_eq!(made[0]["body"], with_image(&file_id), "{made:?}");
    let comment_id = made[0]["id"].as_str().expect("an id");
    // The commenter changes the body of their own comment.
    let read = block_of(&th.client, &board.id, comment_id).await;
    th.client2
        .patch(
            &format!("{blocks}/{comment_id}"),
            &json!({"body": document(), "readAt": read["updatedAt"]}),
        )
        .await
        .check_no_content();
    // The commenter may not upload, and so may not copy another board's files in.
    crate::files::upload(&th.client2, &board.id, "mine.png", b"png")
        .await
        .check_forbidden();
    th.client2
        .post(
            &format!("{blocks}?sourceBoardId={}", source.id),
            &comment("c2", &with_image(&source_file)),
        )
        .await
        .check_forbidden();
    let stored: Vec<Value> = th.client.get(&blocks).await.ok_json();
    assert_eq!(
        stored.len(),
        2,
        "the card and the first comment: {stored:?}"
    );

    th.teardown().await;
}

/// A comment whose body was changed says so (`editedAt`), and the board's members read
/// what it said before (`getCommentVersions`): the text it was posted with, by its
/// author, and each later one, by who wrote it. A reader by the read token, and anybody
/// who is no member, gets no versions.
#[tokio::test]
async fn a_changed_comment_says_so_and_its_members_read_what_it_said() {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let blocks = format!("{TEST_WS}/boards/{}/blocks", board.id);
    let me = th.client.me().await;
    let card: Card = new_card(&th.client, &board.id, &json!({"title": "Card"}))
        .await
        .created_json();
    let made: Vec<Value> = th
        .client
        .post(
            &blocks,
            &json!([{"containerId": board.id, "parentId": card.id, "type": "comment",
                     "createdAt": 1, "updatedAt": 1, "editedAt": 5, "body": paragraph_doc("first")}]),
        )
        .await
        .created_json();
    let comment_id = made[0]["id"].as_str().expect("an id").to_owned();
    let versions_path = format!("{blocks}/{comment_id}/versions");
    assert_eq!(
        made[0].get("editedAt"),
        None,
        "a client sets no editedAt: {made:?}"
    );
    let posted = block_of(&th.client, &board.id, &comment_id).await;
    assert_eq!(posted.get("editedAt"), None, "never changed: {posted}");
    assert_eq!(th.client.get(&versions_path).await.value(), json!([]));

    th.client
        .patch(
            &format!("{blocks}/{comment_id}"),
            &json!({"body": paragraph_doc("second"), "readAt": posted["updatedAt"]}),
        )
        .await
        .check_no_content();
    let changed = block_of(&th.client, &board.id, &comment_id).await;
    assert_eq!(changed["editedAt"], changed["updatedAt"], "{changed}");
    assert_eq!(changed["body"], paragraph_doc("second"));
    let versions = th.client.get(&versions_path).await.value();
    assert_eq!(
        versions,
        json!([{
            "version": 1,
            "body": paragraph_doc("first"),
            "writtenBy": me.id,
            "writtenAt": posted["createdAt"],
        }]),
        "the text it was posted with"
    );

    // Nobody but the members: not who is no member, nor a reader by the token.
    th.client2
        .get(&versions_path)
        .await
        .check_error(StatusCode::NOT_FOUND, "board not found");
    let token = th.share(&board.id).await;
    th.anonymous()
        .get(&format!("{versions_path}?readToken={token}"))
        .await
        .check_unauthorized();
    let user2 = th.client2.me().await;
    th.app
        .add_member_to_container(&fvoci_model::ContainerMember::new(
            &board.id,
            &user2.id,
            fvoci_model::ContainerRole::Viewer,
        ))
        .await
        .expect("user2 is a viewer");
    assert_eq!(
        th.client2.get(&versions_path).await.value(),
        versions,
        "a viewer reads them"
    );

    // A card has no versions.
    th.client
        .get(&format!("{blocks}/{}/versions", card.id))
        .await
        .check_error(StatusCode::NOT_FOUND, "comment not found");

    // In the trash the comment keeps them.
    th.client
        .delete(&format!("{blocks}/{comment_id}"))
        .await
        .check_no_content();
    assert_eq!(th.client.get(&versions_path).await.value(), versions);
}

/// A document of one paragraph with the text.
fn paragraph_doc(text: &str) -> Value {
    json!({"type": "doc", "content": [paragraph(text)]})
}
