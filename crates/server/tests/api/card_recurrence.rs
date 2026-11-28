//! Cards that repeat through the API: only a board with a rule of done and a due date
//! property lets a card repeat (`recurrence_needs_board_rules`), and the write that
//! completes one answers the card naming its next one, which is on the board and is
//! told of to whoever listens.

use fvoci_model::Card;
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::board_broadcasts::WorkspaceSocket;
use crate::cards::cards_path;
use crate::common::{Client, Setup, TEST_WORKSPACE_ID, TEST_WS};

/// The property of the rule of done, and its options.
pub(crate) const STATUS: &str = "status";
/// The due date property.
pub(crate) const DUE: &str = "due";

/// How a card repeats every week from its due date.
pub(crate) fn weekly() -> Value {
    json!({"every": 1, "unit": "week", "anchor": "due"})
}

/// How the next card of one due on the 15th of January 2099 that repeats weekly does:
/// counted from that due date, the first of its chain.
fn weekly_from_the_15th() -> Value {
    json!({"every": 1, "unit": "week", "anchor": "due", "from": "2099-01-15"})
}

/// A board of the test workspace with a status (`todo`, `done`) and a due date, `done`
/// its rule of done and `due` the due date it names: its cards may repeat.
pub(crate) async fn chores(client: &Client) -> String {
    chores_ruled(
        client,
        Some(json!({"propertyId": STATUS, "optionIds": ["done"]})),
    )
    .await
}

/// The same board, `done_when` its rule of done; with one, it names `due` as its due date.
async fn chores_ruled(client: &Client, done_when: Option<Value>) -> String {
    let mut board = json!({
        "workspaceId": TEST_WORKSPACE_ID, "visibility": "private", "title": "Chores",
        "cardProperties": [
            {"id": STATUS, "name": "Status", "type": "select", "options": [
                {"id": "todo", "label": "Todo"}, {"id": "done", "label": "Done"},
            ]},
            {"id": DUE, "name": "Due", "type": "date", "options": []},
        ],
    });
    if let Some(rule) = done_when {
        board["doneWhen"] = rule;
        board["dueDatePropertyId"] = json!(DUE);
    }
    let made: Value = client
        .post(&format!("{TEST_WS}/boards"), &board)
        .await
        .created_json();
    made["id"].as_str().expect("an identifier").to_owned()
}

fn card_path(card_id: &str) -> String {
    format!("{TEST_WS}/cards/{card_id}")
}

#[tokio::test]
async fn a_card_repeats_only_where_the_board_has_the_rules_for_it() {
    let th = Setup::native().start().await.init_basic().await;
    let bare = chores_ruled(&th.client, None).await;
    let refused = th
        .client
        .post(
            &cards_path(&bare),
            &json!({"title": "sweep", "recurrence": weekly()}),
        )
        .await;
    refused.check_status(StatusCode::BAD_REQUEST);
    let problem: Value = refused.json();
    assert_eq!(
        problem["reason"], "recurrence_needs_board_rules",
        "{problem}"
    );
    assert_eq!(problem["errors"][0]["pointer"], "/recurrence", "{problem}");
    let card: Card = th
        .client
        .post(&cards_path(&bare), &json!({"title": "sweep"}))
        .await
        .created_json();
    th.client
        .patch(&card_path(&card.id), &json!({"recurrence": weekly()}))
        .await
        .check_status(StatusCode::BAD_REQUEST);
    th.client
        .post(
            &cards_path(&bare),
            &json!({"title": "sweep", "recurrence": {"every": 0, "unit": "day", "anchor": "due"}}),
        )
        .await
        .check_status(StatusCode::BAD_REQUEST);
    th.teardown().await;
}

#[tokio::test]
async fn completing_a_card_that_repeats_makes_and_announces_the_next() {
    let th = Setup::native().start().await.init_basic().await;
    let board_id = chores(&th.client).await;
    let card: Card = th
        .client
        .post(
            &cards_path(&board_id),
            &json!({"title": "water", "recurrence": weekly(),
                    "properties": {STATUS: "todo", DUE: {"from": "2099-01-15"}}}),
        )
        .await
        .created_json();
    assert_eq!(
        serde_json::to_value(card.recurrence).expect("JSON"),
        weekly()
    );
    let me = th.client.me().await.id;
    let mut socket = WorkspaceSocket::subscribe(&th, &th.client, TEST_WORKSPACE_ID, &me).await;

    let completed: Value = th
        .client
        .patch(
            &card_path(&card.id),
            &json!({"updatedProperties": {STATUS: "done"}}),
        )
        .await
        .ok_json();
    let next_id = completed["recurredTo"]
        .as_str()
        .expect("the completed card names the next")
        .to_owned();
    assert_eq!(completed.get("recurrence"), None, "{completed}");
    socket.expect("blockChanged", "/block/id", &card.id).await;
    let told = socket.expect("blockChanged", "/block/id", &next_id).await;
    assert_eq!(told["block"]["title"], "water", "{told}");

    let next: Value = th.client.get(&card_path(&next_id)).await.ok_json();
    assert_eq!(next["recurrence"], weekly_from_the_15th(), "{next}");
    assert_eq!(
        next["properties"],
        json!({STATUS: "todo", DUE: {"from": "2099-01-22"}}),
        "{next}"
    );
    // Undone, it repeats again; the next card is the client's to take away.
    let undone: Value = th
        .client
        .patch(
            &card_path(&card.id),
            &json!({"updatedProperties": {STATUS: "todo"}, "recurrence": weekly()}),
        )
        .await
        .ok_json();
    assert_eq!(undone.get("recurredTo"), None, "{undone}");
    assert_eq!(undone["recurrence"], weekly(), "{undone}");
    th.teardown().await;
}

#[tokio::test]
async fn completing_cards_that_repeat_at_once_tells_of_each_next_after_them() {
    let th = Setup::native().start().await.init_basic().await;
    let board_id = chores(&th.client).await;
    let mut cards = Vec::new();
    for title in ["water", "feed"] {
        let card: Card = th
            .client
            .post(
                &cards_path(&board_id),
                &json!({"title": title, "recurrence": weekly(),
                        "properties": {STATUS: "todo", DUE: {"from": "2099-01-15"}}}),
            )
            .await
            .created_json();
        cards.push(card.id);
    }
    let me = th.client.me().await.id;
    let mut socket = WorkspaceSocket::subscribe(&th, &th.client, TEST_WORKSPACE_ID, &me).await;

    let completed: Vec<Value> = th
        .client
        .post(
            &format!("{TEST_WS}/boards/{board_id}/cards/batch-patch"),
            &json!({"cardIds": cards, "updatedProperties": {STATUS: "done"}}),
        )
        .await
        .ok_json();
    let next: Vec<String> = completed
        .iter()
        .map(|card| {
            assert_eq!(card.get("recurrence"), None, "{card}");
            card["recurredTo"]
                .as_str()
                .expect("each completed card names its next")
                .to_owned()
        })
        .collect();
    // The completed cards, then their next cards, in the order of the batch.
    for id in cards.iter().chain(&next) {
        socket.expect("blockChanged", "/block/id", id).await;
    }
    for id in &next {
        let made: Value = th.client.get(&card_path(id)).await.ok_json();
        assert_eq!(made["recurrence"], weekly_from_the_15th(), "{made}");
        assert_eq!(
            made["properties"][DUE],
            json!({"from": "2099-01-22"}),
            "{made}"
        );
    }
    th.teardown().await;
}
