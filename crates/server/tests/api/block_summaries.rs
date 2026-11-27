//! A card's, a page's and an entry's `blockChanged` carries a summary of the body in place
//! of the body (`contracts/websocket.md` 3): whether there is one, its task items and the
//! preview a gallery shows, as a list of cards gives them. A comment's carries its body.

use std::time::Duration;

use fvoci_model::{Block, BlockType, Body, Container, ContainerVisibility, new_id};
use serde_json::{Value, json};

use crate::cards::cards_path;
use crate::common::{ROOT_WORKSPACE_ID, Setup, TEST_WORKSPACE_ID, TEST_WS, TestServer};
use crate::rooms::{Peer, give, table};
use crate::websocket::{PATIENCE, Socket, check_message, read_contract, subscribe_to_workspace};

/// Whether a task item is checked.
#[derive(Clone, Copy)]
enum Task {
    Done,
    Open,
}

/// A task item saying `words`.
fn task(state: Task, words: &str) -> Value {
    let checked = matches!(state, Task::Done);
    json!({"type": "taskItem", "attrs": {"checked": checked}, "content": [
        {"type": "paragraph", "content": [{"type": "text", "text": words}]},
    ]})
}

/// A body of `tasks` and a paragraph after them.
fn with_tasks(tasks: &[Value]) -> Value {
    json!({"type": "doc", "content": [
        {"type": "taskList", "content": tasks},
        {"type": "paragraph", "content": [{"type": "text", "text": "notes"}]},
    ]})
}

/// Reads `socket` until it is told of the block `id` with `words` in the preview of its
/// summary, skipping other messages. Checks it against the websocket contract.
async fn told_of(socket: &mut Socket, id: &str, words: &str) -> Value {
    told_when(socket, id, |summary| {
        summary["preview"]["text"]
            .as_str()
            .is_some_and(|text| text.contains(words))
    })
    .await
}

/// Reads `socket` until it is told of the block `id` with a summary that `until` takes.
async fn told_when(socket: &mut Socket, id: &str, until: impl Fn(&Value) -> bool) -> Value {
    let contract = read_contract("websocket.json");
    let openapi = read_contract("openapi.json");
    let deadline = tokio::time::Instant::now() + PATIENCE;
    loop {
        let wait = deadline.saturating_duration_since(tokio::time::Instant::now());
        let message = socket
            .next_json(wait)
            .await
            .unwrap_or_else(|| panic!("told of {id}"));
        if message["type"] != "blockChanged" || message["block"]["id"] != id {
            continue;
        }
        check_message(&contract, &openapi, &message);
        if until(&message["summary"]) {
            return message;
        }
    }
}

/// What the card list asked for previews says of the card's body, as a summary is.
async fn listed_summary(th: &TestServer, board_id: &str, card_id: &str) -> Value {
    let cards: Vec<Value> = th
        .client
        .get(&format!("{}?preview=true", cards_path(board_id)))
        .await
        .ok_json();
    let card = cards
        .iter()
        .find(|card| card["id"] == card_id)
        .unwrap_or_else(|| panic!("{card_id} is listed: {cards:?}"));
    json!({
        "hasBody": card["hasBody"], "tasksTotal": card["tasksTotal"],
        "tasksDone": card["tasksDone"], "preview": card["preview"],
    })
}

/// A body of `paragraphs` paragraphs of a thousand letters, after a task list of three
/// items of which the first is checked.
fn long_body(paragraphs: usize) -> Value {
    let mut content = vec![json!({"type": "taskList", "content": [
        task(Task::Done, "write"), task(Task::Open, "read"), task(Task::Open, "send"),
    ]})];
    let line = "a".repeat(1000);
    content.extend(
        (0..paragraphs)
            .map(|_| json!({"type": "paragraph", "content": [{"type": "text", "text": line}]})),
    );
    json!({"type": "doc", "content": content})
}

/// A private board of the user's in the root workspace with a card of `body`, stored.
async fn card_with(th: &TestServer, user_id: &str, body: Value) -> Block {
    let mut board = Container {
        id: new_id(),
        workspace_id: ROOT_WORKSPACE_ID.to_owned(),
        visibility: ContainerVisibility::Private,
        title: "Roadmap".to_owned(),
        ..Container::default()
    };
    th.app
        .store()
        .insert_container_with_admin(&mut board, user_id)
        .await
        .expect("the board is stored");
    let mut card = Block {
        id: new_id(),
        container_id: board.id.clone(),
        block_type: BlockType::Card,
        title: "A long card".to_owned(),
        body: Body::parse(body).expect("the body is allowed"),
        ..Block::default()
    };
    th.app
        .store()
        .insert_block(&mut card, user_id)
        .await
        .expect("the card is stored");
    card
}

/// What one listener is sent when a card of a body just under the limit (8 MiB) changes,
/// as when the room of its body writes it: the summary, a few hundred bytes, where the
/// body went before. The sizes are printed for the report.
#[tokio::test]
async fn a_long_body_is_not_sent_to_whoever_follows_the_board() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let me = th.client.me().await;
    let body = long_body(7_900);
    let body_bytes = body.to_string().len();
    assert!(
        (8_000_000..fvoci_model::BODY_MAX_BYTES).contains(&body_bytes),
        "a body just under the limit: {body_bytes} bytes"
    );
    let card = card_with(&th, &me.id, body).await;
    let mut socket = Socket::connect_as(&th.client).await;
    subscribe_to_workspace(&th, &mut socket, ROOT_WORKSPACE_ID, &me.id).await;

    th.app
        .ws()
        .broadcast_block_change(ROOT_WORKSPACE_ID, &card)
        .await;
    let told = socket.next_json(PATIENCE).await.expect("the card is told");
    let sent = told.to_string().len();
    println!("blockChanged of a card of a {body_bytes}-byte body: {sent} bytes");
    assert!(sent < 2_000, "{sent} bytes sent");
    assert_eq!(told["block"]["body"], Value::Null, "{}", told["block"]);
    assert_eq!(
        told["summary"],
        json!({"hasBody": true, "tasksTotal": 3, "tasksDone": 1,
            "preview": {"imageFileId": "", "text": format!("write\nread\nsend\n{}", "a".repeat(184))}}),
    );
    th.teardown().await;
}

/// What another user types in the card's room reaches who follows the board, when the
/// room writes the body, as the body's summary: no body, the task items counted, the
/// preview as the card list gives it.
#[tokio::test]
async fn a_body_its_room_writes_is_told_as_its_summary() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let me = th.client.me().await.id;
    let t = table(
        &th,
        &with_tasks(&[task(Task::Done, "one"), task(Task::Open, "two")]),
    )
    .await;
    let user2 = th.client2.me().await.id;
    give(&th, &t.board_id, &user2, "editor").await;
    let mut listener = Socket::connect_as(&th.client).await;
    subscribe_to_workspace(&th, &mut listener, TEST_WORKSPACE_ID, &me).await;

    let mut writer = Peer::connect(&th.client2, 102).await;
    writer.joined(TEST_WORKSPACE_ID, &t.card_id).await;
    writer.write_acked("typed in the room").await;
    th.app
        .collab()
        .write_body(&th.app, &t.card_id)
        .await
        .expect("the room's body is written");

    let told = told_of(&mut listener, &t.card_id, "typed in the room").await;
    assert_eq!(told["block"]["body"], Value::Null, "{told}");
    assert_eq!(told["block"]["modifiedBy"], json!(user2), "{told}");
    assert_eq!(
        (
            &told["summary"]["hasBody"],
            &told["summary"]["tasksTotal"],
            &told["summary"]["tasksDone"]
        ),
        (&json!(true), &json!(2), &json!(1)),
        "{told}"
    );
    assert_eq!(
        told["summary"],
        listed_summary(&th, &t.board_id, &t.card_id).await,
        "the summary is what the card list says"
    );
    th.teardown().await;
}

/// A body another user writes over HTTP reaches who follows the board as its summary;
/// a body taken away is told as none.
#[tokio::test]
async fn a_body_written_over_http_is_told_as_its_summary() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let me = th.client.me().await.id;
    let t = table(&th, &with_tasks(&[task(Task::Open, "one")])).await;
    let user2 = th.client2.me().await.id;
    give(&th, &t.board_id, &user2, "editor").await;
    let mut listener = Socket::connect_as(&th.client).await;
    subscribe_to_workspace(&th, &mut listener, TEST_WORKSPACE_ID, &me).await;

    let path = format!("{TEST_WS}/cards/{}", t.card_id);
    let read_at = th.client2.get(&path).await.value()["updatedAt"].clone();
    let tasks = [
        task(Task::Done, "one"),
        task(Task::Done, "two"),
        task(Task::Open, "three"),
    ];
    th.client2
        .patch(
            &path,
            &json!({"readAt": read_at, "body": with_tasks(&tasks)}),
        )
        .await
        .check_ok();
    let told = told_of(&mut listener, &t.card_id, "three").await;
    assert_eq!(told["block"]["body"], Value::Null, "{told}");
    assert_eq!(
        (
            &told["summary"]["tasksTotal"],
            &told["summary"]["tasksDone"]
        ),
        (&json!(3), &json!(2)),
        "{told}"
    );
    assert_eq!(
        told["summary"],
        listed_summary(&th, &t.board_id, &t.card_id).await,
        "the summary is what the card list says"
    );

    let read_at = th.client2.get(&path).await.value()["updatedAt"].clone();
    th.client2
        .patch(&path, &json!({"readAt": read_at, "body": null}))
        .await
        .check_ok();
    let told = told_when(&mut listener, &t.card_id, |summary| {
        summary["hasBody"] == json!(false)
    })
    .await;
    assert_eq!(
        told["summary"],
        json!({"hasBody": false, "tasksTotal": 0, "tasksDone": 0,
            "preview": {"imageFileId": "", "text": ""}}),
        "{told}"
    );
    th.teardown().await;
}

/// A comment's message carries its body, as before, and no summary; so does a view's,
/// which has no body. Both have the members of the contract.
#[tokio::test]
async fn a_comment_and_a_view_are_told_whole() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let me = th.client.me().await;
    let card = card_with(&th, &me.id, with_tasks(&[task(Task::Open, "one")])).await;
    let said = json!({"type": "doc", "content": [
        {"type": "paragraph", "content": [{"type": "text", "text": "a remark"}]},
    ]});
    let comment = Block {
        id: new_id(),
        parent_id: card.id.clone(),
        container_id: card.container_id.clone(),
        block_type: BlockType::Comment,
        body: Body::parse(said.clone()).expect("the body is allowed"),
        ..Block::default()
    };
    let view = Block {
        id: new_id(),
        container_id: card.container_id.clone(),
        block_type: BlockType::View,
        title: "Table".to_owned(),
        ..Block::default()
    };
    let mut socket = Socket::connect_as(&th.client).await;
    subscribe_to_workspace(&th, &mut socket, ROOT_WORKSPACE_ID, &me.id).await;
    let contract = read_contract("websocket.json");
    let openapi = read_contract("openapi.json");

    for (block, body) in [(&comment, said), (&view, Value::Null)] {
        th.app
            .ws()
            .broadcast_block_change(ROOT_WORKSPACE_ID, block)
            .await;
        let told = socket.next_json(PATIENCE).await.expect("the block is told");
        check_message(&contract, &openapi, &told);
        assert_eq!(told["block"]["id"], json!(block.id), "{told}");
        assert_eq!(told["block"]["body"], body, "{told}");
        assert!(told.get("summary").is_none(), "no summary: {told}");
    }
    // Nothing else comes of them.
    assert_eq!(socket.next_json(Duration::from_millis(300)).await, None);
    th.teardown().await;
}

/// A diary entry's body is edited in a room too (`BlockType::WITH_ROOM`), so its
/// `blockChanged` carries a summary in place of its body and matches the contract.
#[tokio::test]
async fn a_diary_entry_body_is_told_as_its_summary() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let me = th.client.me().await;
    let own = crate::own_space::own_workspace(&th.client).await.id;
    let diary = th
        .client
        .post(&format!("/w/{own}/diaries"), &json!({}))
        .await;
    diary.check_created();
    let diary_id = diary.value()["id"]
        .as_str()
        .expect("diary identifier")
        .to_owned();
    let entry = th
        .client
        .post(
            &format!("/w/{own}/diaries/{diary_id}/entries"),
            &json!({"date": "2024-02-29"}),
        )
        .await;
    entry.check_created();
    let made = entry.value();
    let entry_id = made["id"].as_str().expect("entry identifier").to_owned();

    let mut listener = Socket::connect_as(&th.client).await;
    subscribe_to_workspace(&th, &mut listener, &own, &me.id).await;

    let path = format!("/w/{own}/diaries/{diary_id}/entries/{entry_id}");
    th.client
        .patch(
            &path,
            &json!({
                "readAt": made["updatedAt"],
                "body": with_tasks(&[task(Task::Done, "walk"), task(Task::Open, "journal")]),
            }),
        )
        .await
        .check_ok();

    let told = told_of(&mut listener, &entry_id, "journal").await;
    assert_eq!(told["block"]["type"], json!("entry"), "{told}");
    assert_eq!(told["block"]["body"], Value::Null, "{told}");
    assert_eq!(
        told["summary"],
        json!({
            "hasBody": true,
            "tasksTotal": 2,
            "tasksDone": 1,
            "preview": {"imageFileId": "", "text": "walk\njournal\nnotes"},
        }),
        "{told}"
    );
    th.teardown().await;
}
