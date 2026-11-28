//! What card property definitions and values must look like to be stored.
//!
//! Validation rejects malformed property definitions and card property values (missing
//! definition IDs, non-string property values, or non-object card property maps) so one
//! member cannot break rendering or card listing for the rest of a board. Every way of
//! writing them now answers 400 and stores nothing.

use std::io::{Cursor, Write as _};

use fvoci_model::{Card, Container};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::common::TEST_WS;
use crate::common::{ApiResponse, Client, Setup, TEST_WORKSPACE_ID, TestServer};
use crate::export::import;

/// Card property definitions with missing/empty IDs (semantic validation failures).
fn id_missing_definitions() -> Vec<(&'static str, Value)> {
    vec![
        ("no id", json!({"name": "Status", "type": "select"})),
        ("an empty id", json!({"id": "", "name": "Status"})),
        (
            "an option with an empty id",
            json!({"id": "p1", "type": "select", "options": [{"id": "", "label": "Open"}]}),
        ),
        (
            "two options with the same id",
            json!({"id": "p1", "type": "select", "options": [
                {"id": "o1", "label": "Open"},
                {"id": "o1", "label": "Closed"},
            ]}),
        ),
    ]
}

/// Card property definitions whose wire shape, property type, or option fields/colors
/// violate the static [`CardPropertyDefinition`] contract.
fn wire_invalid_definitions() -> Vec<(&'static str, Value)> {
    vec![
        ("a number as id", json!({"id": 7, "name": "Status"})),
        ("a number as name", json!({"id": "p1", "name": 7})),
        ("a list as type", json!({"id": "p1", "type": ["select"]})),
        ("an unknown type", json!({"id": "p1", "type": "unknown"})),
        (
            "an unknown definition field",
            json!({"id": "p1", "propertyType": "date"}),
        ),
        (
            "options that are no list",
            json!({"id": "p1", "options": "none"}),
        ),
        (
            "options that are null",
            json!({"id": "p1", "options": null}),
        ),
        (
            "an option that is no object",
            json!({"id": "p1", "options": ["a"]}),
        ),
        (
            "an option with legacy value key instead of label",
            json!({"id": "p1", "options": [{"id": "o1", "value": "Open", "color": "red"}]}),
        ),
        (
            "an option with legacy value key alongside label",
            json!({"id": "p1", "options": [{"id": "o1", "label": "Open", "value": "Legacy", "color": "red"}]}),
        ),
        (
            "an option with legacy prefixed color",
            json!({"id": "p1", "options": [{"id": "o1", "label": "Open", "color": concat!("prop", "ColorRed")}]}),
        ),
        (
            "an option with an unknown color",
            json!({"id": "p1", "options": [{"id": "o1", "label": "Open", "color": "neon"}]}),
        ),
        (
            "an option with a number as label",
            json!({"id": "p1", "options": [{"id": "o1", "label": 1, "color": "red"}]}),
        ),
    ]
}

/// Card property values that are malformed.
fn malformed_values() -> Vec<(&'static str, Value)> {
    vec![
        ("an object", json!({"nested": "x"})),
        ("a number", json!(3)),
        ("a boolean", json!(true)),
        ("a list with a number", json!(["a", 1])),
        ("a list with a list", json!([["a"]])),
    ]
}

async fn new_board(client: &Client, card_properties: Value) -> ApiResponse {
    client
        .post(
            &format!("{TEST_WS}/boards"),
            &json!({
                "workspaceId": TEST_WORKSPACE_ID, "visibility": "private", "title": "Properties",
                "cardProperties": card_properties,
            }),
        )
        .await
}

fn card_block(board_id: &str, properties: Value) -> Value {
    let mut block = json!({
        "id": "c1", "containerId": board_id, "type": "card", "title": "Card",
        "createdAt": 1, "updatedAt": 1, "fields": {},
    });
    block["fields"]["properties"] = properties;
    block
}

/// Checks that a request was refused as a bad request that says what is wrong.
#[track_caller]
fn check_refused(response: &ApiResponse, what: &str) {
    assert_eq!(
        response.status,
        StatusCode::BAD_REQUEST,
        "{what}: {}",
        response.text()
    );
    assert!(
        response.error_message().contains("card propert"),
        "{what}: the refusal names the property data: {}",
        response.text()
    );
}

/// A private board of the first user with one well-formed definition and one card that
/// has a value for it.
async fn board_with_card(th: &TestServer) -> (Container, Card) {
    let board: Container = new_board(
        &th.client,
        json!([{
            "id": "p1",
            "name": "Status",
            "type": "select",
            "options": [
                {"id": "open", "label": "Open", "color": "default"},
                {"id": "x", "label": "X", "color": "default"},
            ],
        }]),
    )
    .await
    .created_json();
    let card: Card = th
        .client
        .post(
            &format!("{TEST_WS}/boards/{}/cards", board.id),
            &json!({"title": "Card", "properties": {"p1": "open"}}),
        )
        .await
        .created_json();
    (board, card)
}

async fn board_count(client: &Client) -> usize {
    let boards: Vec<Container> = client.get(&format!("{TEST_WS}/boards")).await.ok_json();
    boards.len()
}

// -- definitions --------------------------------------------------------------------------

#[tokio::test]
async fn a_board_with_a_malformed_card_property_definition_is_not_created() {
    let th = Setup::native().start().await.init_basic().await;

    for (what, definition) in id_missing_definitions() {
        // Among well-formed ones, too.
        let response = new_board(&th.client, json!([{"id": "ok"}, definition])).await;
        check_refused(&response, what);
    }
    for (what, definition) in wire_invalid_definitions() {
        let response = new_board(&th.client, json!([{"id": "ok"}, definition])).await;
        assert_eq!(
            response.status,
            StatusCode::BAD_REQUEST,
            "{what}: {}",
            response.text()
        );
    }
    let not_objects = new_board(&th.client, json!(["p1"])).await;
    assert_eq!(
        not_objects.status,
        StatusCode::BAD_REQUEST,
        "a definition that is no object: {}",
        not_objects.text()
    );
    assert_eq!(board_count(&th.client).await, 0, "no board was created");
    th.teardown().await;
}

#[tokio::test]
async fn a_board_patch_with_a_malformed_card_property_definition_changes_nothing() {
    let th = Setup::native().start().await.init_basic().await;
    let (board, _card) = board_with_card(&th).await;
    let path = format!("{TEST_WS}/containers/{}", board.id);

    for (what, definition) in id_missing_definitions() {
        let response = th
            .client
            .patch(
                &path,
                &json!({"title": "renamed", "updatedCardProperties": [{"id": "p2"}, definition]}),
            )
            .await;
        check_refused(&response, what);
    }
    for (what, definition) in wire_invalid_definitions() {
        let response = th
            .client
            .patch(
                &path,
                &json!({"title": "renamed", "updatedCardProperties": [{"id": "p2"}, definition]}),
            )
            .await;
        assert_eq!(
            response.status,
            StatusCode::BAD_REQUEST,
            "{what}: {}",
            response.text()
        );
    }
    let stored: Container = th.client.get(&path).await.ok_json();
    assert_eq!(
        stored.title, "Properties",
        "the title was not changed either"
    );
    assert_eq!(
        stored.card_properties, board.card_properties,
        "the definitions are as they were"
    );
    th.teardown().await;
}

#[tokio::test]
async fn well_formed_card_property_definitions_are_stored_as_sent() {
    let th = Setup::native().start().await.init_basic().await;
    let sent = json!([
        {"id": "p1"},
        {"id": "p2", "name": "Status", "type": "select", "options": [
            {"id": "o1", "label": "Open", "color": "red"},
            {"id": "o2", "label": ""},
        ]},
        {"id": "p3", "name": "", "type": "text", "options": []},
    ]);
    let expected = json!([
        {"id": "p1", "name": "", "type": "text", "options": []},
        {"id": "p2", "name": "Status", "type": "select", "options": [
            {"id": "o1", "label": "Open", "color": "red"},
            {"id": "o2", "label": "", "color": "default"},
        ]},
        {"id": "p3", "name": "", "type": "text", "options": []},
    ]);

    let board: Value = new_board(&th.client, sent).await.created_json();
    assert_eq!(
        board["cardProperties"], expected,
        "created as normalized CardPropertyDefinition"
    );

    let added = json!({"id": "p4", "name": "Owner", "type": "person", "options": []});
    let patched: Value = th
        .client
        .patch(
            &format!(
                "{TEST_WS}/containers/{}",
                board["id"].as_str().expect("an id")
            ),
            &json!({"updatedCardProperties": [added]}),
        )
        .await
        .ok_json();
    assert_eq!(
        patched["cardProperties"][3], added,
        "a well-formed definition is added by a patch"
    );
    th.teardown().await;
}

// -- values -------------------------------------------------------------------------------

/// A batch with a malformed value changes none of its cards, and values that are no
/// object are refused.
#[tokio::test]
async fn a_batch_patch_with_malformed_properties_changes_nothing() {
    let th = Setup::native().start().await.init_basic().await;
    let (board, card) = board_with_card(&th).await;
    let second: Card = th
        .client
        .post(
            &format!("{TEST_WS}/boards/{}/cards", board.id),
            &json!({"title": "Second"}),
        )
        .await
        .created_json();

    for (what, value) in malformed_values() {
        let response = th
            .client
            .post(
                &format!("{TEST_WS}/boards/{}/cards/batch-patch", board.id),
                &json!({"cardIds": [second.id, card.id], "updatedProperties": {"p1": value}}),
            )
            .await;
        check_refused(&response, &format!("a batch with {what}"));
    }
    // Properties that are no object are refused as the body is read.
    th.client
        .patch(
            &format!("{TEST_WS}/cards/{}", card.id),
            &json!({"updatedProperties": "text"}),
        )
        .await
        .check_bad_request();

    let stored: Card = th
        .client
        .get(&format!("{TEST_WS}/cards/{}", card.id))
        .await
        .ok_json();
    assert_eq!(stored.title, "Card", "the title was not changed");
    assert_eq!(
        stored.properties, card.properties,
        "the values are as they were"
    );
    let other: Card = th
        .client
        .get(&format!("{TEST_WS}/cards/{}", second.id))
        .await
        .ok_json();
    assert_eq!(
        other.title, "Second",
        "nor was the other block of the batch"
    );
    th.teardown().await;
}

#[tokio::test]
async fn a_card_with_malformed_property_values_is_neither_created_nor_patched() {
    let th = Setup::native().start().await.init_basic().await;
    let (board, card) = board_with_card(&th).await;

    for (what, value) in malformed_values() {
        let created = th
            .client
            .post(
                &format!("{TEST_WS}/boards/{}/cards", board.id),
                &json!({"title": "Another", "properties": {"p1": value}}),
            )
            .await;
        check_refused(&created, what);
        let patched = th
            .client
            .patch(
                &format!("{TEST_WS}/cards/{}", card.id),
                &json!({"title": "renamed", "updatedProperties": {"p1": value}}),
            )
            .await;
        check_refused(&patched, what);
    }
    let cards: Vec<Card> = th
        .client
        .get(&format!("{TEST_WS}/boards/{}/cards", board.id))
        .await
        .ok_json();
    assert_eq!(cards.len(), 1, "no card was created");
    assert_eq!(cards[0].title, "Card", "and the card was not changed");
    assert_eq!(cards[0].properties, card.properties);
    th.teardown().await;
}

#[tokio::test]
async fn well_formed_property_values_are_stored_as_sent() {
    let th = Setup::native().start().await.init_basic().await;
    let (board, card) = board_with_card(&th).await;
    let values = json!({
        "text": "hello", "empty": "", "many": ["a", "b"], "none": [], "unset": null,
    });

    let created: Value = th
        .client
        .post(
            &format!("{TEST_WS}/boards/{}/cards", board.id),
            &json!({"title": "Another", "properties": values}),
        )
        .await
        .created_json();
    assert_eq!(created["properties"], values, "a card is created with them");

    let patched: Value = th
        .client
        .patch(
            &format!("{TEST_WS}/cards/{}", card.id),
            &json!({"updatedProperties": values}),
        )
        .await
        .ok_json();
    for (id, value) in values.as_object().expect("an object") {
        assert_eq!(
            patched["properties"].get(id),
            Some(value),
            "patched with {id}, merged into the values the card had"
        );
    }

    let batched: Vec<Value> = th
        .client
        .post(
            &format!("{TEST_WS}/boards/{}/cards/batch-patch", board.id),
            &json!({"cardIds": [card.id], "updatedProperties": {"p1": "x"}}),
        )
        .await
        .ok_json();
    assert_eq!(batched[0]["properties"]["p1"], json!("x"), "{batched:?}");
    th.teardown().await;
}

// -- the import ---------------------------------------------------------------------------

/// An archive with one board and one card on it.
fn archive(card_properties: &Value, values: Value) -> Vec<u8> {
    let board = json!({"type": "container", "data": {
        "id": "b1", "workspaceId": TEST_WORKSPACE_ID, "visibility": "private", "title": "Imported",
        "cardProperties": card_properties, "createdAt": 1, "updatedAt": 1,
    }});
    let data = card_block("b1", values);
    let block = json!({"type": "block", "data": data});
    let options = zip::write::SimpleFileOptions::default();
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    writer.start_file("version.json", options).expect("start");
    writer
        .write_all(br#"{"version":1,"date":1}"#)
        .expect("write");
    writer
        .start_file("b1/container.jsonl", options)
        .expect("start");
    writer
        .write_all(format!("{board}\n{block}\n").as_bytes())
        .expect("write");
    writer.finish().expect("finish").into_inner()
}

/// An archive is another way in, open to every signed-in user.
#[tokio::test]
async fn an_archive_with_malformed_property_data_is_refused() {
    let th = Setup::native().start().await.init_basic().await;

    let response = import(
        &th.client,
        TEST_WORKSPACE_ID,
        &archive(&json!([{"name": "no id"}]), json!({})),
    )
    .await;
    check_refused(&response, "an archive with a definition without id");
    assert_eq!(board_count(&th.client).await, 0, "no board was imported");

    let response = import(
        &th.client,
        TEST_WORKSPACE_ID,
        &archive(&json!([{"id": "p1"}]), json!({"p1": 3})),
    )
    .await;
    check_refused(&response, "an archive with a number as value");
    assert_eq!(
        board_count(&th.client).await,
        0,
        "the board of the archive is not left behind without its card"
    );

    // The same archive, well-formed, is imported.
    import(
        &th.client,
        TEST_WORKSPACE_ID,
        &archive(&json!([{"id": "p1"}]), json!({"p1": "v"})),
    )
    .await
    .check_ok();
    th.teardown().await;
}

/// A date property holds a date value: a calendar date, a
/// range of them or a time, or `null`. Text in its place is refused, and so is a date
/// value in another property, on the routes of cards and of blocks.
#[tokio::test]
async fn date_properties_hold_date_values() {
    let th = Setup::native().start().await.init_basic().await;
    let board: Container = new_board(
        &th.client,
        json!([
            {"id": "due", "name": "Due", "type": "date"},
            {"id": "note", "name": "Note", "type": "text"},
        ]),
    )
    .await
    .created_json();
    let cards = format!("{TEST_WS}/boards/{}/cards", board.id);

    for due in [
        json!({"from": "2026-03-05"}),
        json!({"from": "2026-03-05", "to": "2026-03-09"}),
        json!({"from": 1_772_703_000_000_i64}),
        Value::Null,
    ] {
        let created: Value = th
            .client
            .post(
                &cards,
                &json!({"title": "Card", "properties": {"due": due}}),
            )
            .await
            .created_json();
        assert_eq!(created["properties"]["due"], due, "{created}");
    }
    let card: Card = th
        .client
        .post(
            &cards,
            &json!({"title": "Card", "properties": {"note": "x"}}),
        )
        .await
        .created_json();

    assert_refused_date_cases(&th.client, &cards, &card.id).await;
    let stored: Card = th
        .client
        .get(&format!("{TEST_WS}/cards/{}", card.id))
        .await
        .ok_json();
    assert_eq!(
        stored.properties, card.properties,
        "the refused patches changed nothing"
    );

    th.teardown().await;
}

async fn assert_refused_date_cases(client: &Client, cards_path: &str, card_id: &str) {
    for (what, properties) in [
        ("a date as text", json!({"due": "{\"from\":1772679600000}"})),
        ("no calendar day", json!({"due": {"from": "2026-02-30"}})),
        (
            "a time zone",
            json!({"due": {"from": 1, "timeZone": "UTC"}}),
        ),
        (
            "to before from",
            json!({"due": {"from": "2026-03-05", "to": "2026-03-01"}}),
        ),
        (
            "a date in a text property",
            json!({"note": {"from": "2026-03-05"}}),
        ),
        (
            "a date in no property",
            json!({"other": {"from": "2026-03-05"}}),
        ),
    ] {
        check_refused(
            &client
                .post(
                    cards_path,
                    &json!({"title": "Card", "properties": properties}),
                )
                .await,
            what,
        );
        check_refused(
            &client
                .patch(
                    &format!("{TEST_WS}/cards/{card_id}"),
                    &json!({"updatedProperties": properties}),
                )
                .await,
            what,
        );
        check_refused(
            &client
                .post(
                    &format!("{cards_path}/batch-patch"),
                    &json!({"cardIds": [card_id], "updatedProperties": properties}),
                )
                .await,
            what,
        );
    }
}

/// A change of the board's definitions leaves the cards' values as they are: a card
/// whose value no longer fits is still written, copied and exported, and the value is
/// checked when it is written again. A copy clears such a value.
#[tokio::test]
async fn values_left_behind_by_a_change_of_definitions_stay_until_written() {
    let th = Setup::native().start().await.init_basic().await;
    let board: Container = new_board(
        &th.client,
        json!([
            {"id": "due", "name": "Due", "type": "date"},
            {"id": "note", "name": "Note", "type": "text"},
        ]),
    )
    .await
    .created_json();
    let card: Card = th
        .client
        .post(
            &format!("{TEST_WS}/boards/{}/cards", board.id),
            &json!({"title": "Card", "properties": {"due": {"from": "2026-03-05"}, "note": "x"}}),
        )
        .await
        .created_json();
    th.client
        .patch(
            &format!("{TEST_WS}/containers/{}", board.id),
            &json!({"deletedCardProperties": ["due"],
                    "updatedCardProperties": [{"id": "note", "name": "Note", "type": "date"}]}),
        )
        .await
        .check_ok();

    let path = format!("{TEST_WS}/cards/{}", card.id);
    let renamed: Card = th
        .client
        .patch(&path, &json!({"title": "Renamed"}))
        .await
        .ok_json();
    assert_eq!(renamed.properties, card.properties, "the values stay");
    check_refused(
        &th.client
            .patch(&path, &json!({"updatedProperties": {"note": "y"}}))
            .await,
        "a text written to what is now a date",
    );
    th.client
        .patch(
            &path,
            &json!({"updatedProperties": {"note": {"from": "2026-03-06"}}}),
        )
        .await
        .check_ok();

    let copy: Value = th
        .client
        .post_empty(&format!(
            "{TEST_WS}/boards/{}/cards/{}/duplicate",
            board.id, card.id
        ))
        .await
        .created_json();
    let copy_values = &copy["properties"];
    assert!(
        copy_values["due"].is_null(),
        "the date of the deleted property is cleared in the copy: {copy}"
    );
    assert_eq!(copy_values["note"], json!({"from": "2026-03-06"}), "{copy}");
    th.client
        .post_empty(&format!("{TEST_WS}/containers/{}/duplicate", board.id))
        .await
        .check_status(StatusCode::CREATED);

    th.teardown().await;
}

/// A batch of cards takes date values as a card does. An archive keeps a date value of
/// a date property and clears a value of the other kind, as a copy of a card does: its
/// board may have changed its definitions since.
#[tokio::test]
async fn date_values_in_a_batch_and_in_an_archive() {
    let th = Setup::native().start().await.init_basic().await;
    let (board, card) = board_with_card(&th).await;
    th.client
        .patch(
            &format!("{TEST_WS}/containers/{}", board.id),
            &json!({"updatedCardProperties": [{"id": "due", "name": "Due", "type": "date"}]}),
        )
        .await
        .check_ok();
    let batch = format!("{TEST_WS}/boards/{}/cards/batch-patch", board.id);
    let text = json!({"cardIds": [card.id], "updatedProperties": {"p1": "open", "due": "soon"}});
    check_refused(&th.client.post(&batch, &text).await, "batchPatchCards");
    let date = json!({"cardIds": [card.id],
                      "updatedProperties": {"p1": "open", "due": {"from": "2026-03-05"}}});
    th.client.post(&batch, &date).await.check_ok();

    import(
        &th.client,
        TEST_WORKSPACE_ID,
        &archive(
            &json!([
                {"id": "due", "name": "Due", "type": "date"},
                {"id": "note", "name": "Note", "type": "text"},
            ]),
            json!({"due": {"from": "2026-03-05"}, "note": {"from": "2026-03-06"}}),
        ),
    )
    .await
    .check_ok();
    let boards: Vec<Container> = th.client.get(&format!("{TEST_WS}/boards")).await.ok_json();
    let imported = boards
        .iter()
        .find(|board| board.title == "Imported")
        .expect("the imported board");
    let blocks = crate::blocks::stored_blocks(&th, &imported.id).await;
    assert_eq!(
        blocks[0].fields["properties"],
        json!({"due": {"from": "2026-03-05"}, "note": null}),
        "the date stays and the value of the other kind is cleared"
    );

    th.teardown().await;
}
