//! Markdown an agent adds after the last of a card's body (`appendCardMarkdown`): the
//! editors in its room are sent it as another editor's update and load nothing anew,
//! what one of them typed and had not sent stays, and a card that no room holds, or
//! that has no body yet, takes it as well, past the limit of an editor's update.

use std::fmt::Write as _;

use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::api_tokens::agent_writing;
use crate::common::{Client, Setup, TEST_WORKSPACE_ID, TEST_WS};
use crate::rooms::{QUIET, paragraphs, two_in_a_room};

/// Adds `markdown` to the card as `client`.
async fn append(client: &Client, card_id: &str, markdown: &str) -> crate::common::ApiResponse {
    client
        .post(
            &format!("{TEST_WS}/cards/{card_id}/body/markdown"),
            &json!({"markdown": markdown}),
        )
        .await
}

/// Whether a body says each of `texts`.
#[track_caller]
fn check_says(body: Option<&Value>, texts: &[&str]) {
    let said = body.map_or_default(Value::to_string);
    for text in texts {
        assert!(said.contains(text), "{text} in {said}");
    }
}

#[tokio::test]
async fn the_editors_are_sent_what_an_agent_adds_and_keep_what_they_typed() {
    let th = Setup::native().start().await.init_basic().await;
    let (table, mut one, mut two) = two_in_a_room(&th, &paragraphs(&["before"])).await;
    let epochs = (one.epoch, two.epoch);
    // The first editor types, and has not sent it yet.
    let unsent = one.editor().append_paragraph("typed by one");
    let agent = agent_writing(&th, &th.client).await;

    let card: Value = append(&agent, &table.card_id, "From the **agent**")
        .await
        .ok_json();
    check_says(Some(&card["body"]), &["before", "From the ", "agent"]);
    while one.step(QUIET).await {}
    while two.step(QUIET).await {}
    assert_eq!(
        (one.epoch, two.epoch),
        epochs,
        "nobody loads the document anew"
    );
    assert!(one.editor.is_some() && two.editor.is_some(), "no docReset");
    check_says(two.body().as_ref(), &["before", "agent"]);
    check_says(one.body().as_ref(), &["typed by one", "agent"]);

    // What the first typed is taken after it, as it was made before it.
    one.send_update(&unsent).await;
    one.text("docAck").await;
    while two.step(QUIET).await {}
    assert_eq!(one.body(), two.body(), "the editors hold one document");
    check_says(two.body().as_ref(), &["before", "typed by one", "agent"]);
    th.app
        .collab()
        .close(&th.app, &table.card_id)
        .await
        .expect("closed");
    let stored: Value = th
        .client
        .get(&format!("{TEST_WS}/cards/{}", table.card_id))
        .await
        .ok_json();
    check_says(Some(&stored["body"]), &["before", "typed by one", "agent"]);

    th.teardown().await;
}

#[tokio::test]
async fn a_card_no_room_holds_takes_more_than_an_editors_update_and_a_reader_adds_nothing() {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, fvoci_model::ContainerVisibility::Open)
        .await;
    let card: Value = th
        .client
        .post(
            &format!("{TEST_WS}/boards/{}/cards", board.id),
            &json!({"title": "no body yet"}),
        )
        .await
        .created_json();
    let card_id = card["id"].as_str().expect("an id").to_owned();
    let agent = agent_writing(&th, &th.client).await;
    // Past the 64 KiB of one editor's update, and of a request without a session.
    let mut markdown = String::new();
    for line in 0..1500 {
        writeln!(
            markdown,
            "Line {line} of what the agent read, written down at length.\n"
        )
        .expect("written");
    }
    assert!(markdown.len() > 64 * 1024, "{}", markdown.len());

    let written: Value = append(&agent, &card_id, &markdown).await.ok_json();
    check_says(Some(&written["body"]), &["Line 0 of", "Line 1499 of"]);
    let more: Value = append(&agent, &card_id, "## And after").await.ok_json();
    check_says(Some(&more["body"]), &["Line 1499 of", "And after"]);

    let refused = append(&agent, &card_id, "   ").await;
    refused.check_status(StatusCode::BAD_REQUEST);
    assert_eq!(refused.refused_at(), "/markdown", "{}", refused.text());

    th.teardown().await;
}
