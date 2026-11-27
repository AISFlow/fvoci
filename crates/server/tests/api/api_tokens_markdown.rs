//! A card's body in Markdown, as an agent reads and writes it: read with
//! `format=markdown`, made and written whole from `bodyMarkdown`, and added to
//! (`appendCardMarkdown`); a body Markdown does not hold, or one people edit now, is
//! not written whole by it.

use std::fmt::Write as _;

use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::api_tokens::agent_writing;
use crate::common::{ApiResponse, Client, Setup, TEST_WORKSPACE_ID, TEST_WS, TestServer};
use crate::files::{upload_in, uploaded_id};
use crate::rooms::{PATIENCE, Peer, QUIET, paragraphs, two_in_a_room};

fn card_path(card_id: &str) -> String {
    format!("{TEST_WS}/cards/{card_id}")
}

async fn in_markdown(client: &Client, card_id: &str) -> Value {
    client
        .get(&format!("{}?format=markdown", card_path(card_id)))
        .await
        .ok_json()
}

async fn append(client: &Client, card_id: &str, markdown: &str) -> ApiResponse {
    client
        .post(
            &format!("{}/body/markdown", card_path(card_id)),
            &json!({"markdown": markdown}),
        )
        .await
}

async fn write_markdown(client: &Client, card: &Value, markdown: &str) -> ApiResponse {
    let id = card["id"].as_str().expect("an id");
    client
        .patch(
            &card_path(id),
            &json!({"bodyMarkdown": markdown, "readAt": card["updatedAt"]}),
        )
        .await
}

#[track_caller]
fn check_refused(response: &ApiResponse, status: StatusCode, reason: &str) {
    assert_eq!(response.status, status, "{}", response.text());
    assert_eq!(response.error_reason(), reason, "{}", response.text());
}

/// An open board of the test workspace with a moon's picture, a report, and a card
/// the agent made of Markdown that shows them and names the user.
async fn a_card_of_markdown(th: &TestServer) -> (String, String, Value) {
    let board = th
        .create_board(TEST_WORKSPACE_ID, fvoci_model::ContainerVisibility::Open)
        .await;
    let image = uploaded_id(
        &upload_in(
            &th.client,
            TEST_WORKSPACE_ID,
            &board.id,
            "moon.png",
            b"moon",
        )
        .await,
    );
    let report = uploaded_id(
        &upload_in(
            &th.client,
            TEST_WORKSPACE_ID,
            &board.id,
            "report.pdf",
            b"pdf",
        )
        .await,
    );
    let me = th.client.me().await.id;
    let agent = agent_writing(th, &th.client).await;
    let markdown = format!(
        "#### Findings\n\n- one for @[me](fvoci-user://{me})\n- two with $x^2$\n\n\
         ![the moon](fvoci-file://{image})\n\n[the report](fvoci-file://{report})"
    );
    let card: Value = agent
        .post(
            &format!("{TEST_WS}/boards/{}/cards", board.id),
            &json!({"title": "made by an agent", "bodyMarkdown": markdown}),
        )
        .await
        .created_json();
    (board.id, image, card)
}

#[tokio::test]
async fn an_agent_makes_reads_and_writes_a_card_in_markdown() {
    let th = Setup::native().start().await.init_basic().await;
    let (_, image, card) = a_card_of_markdown(&th).await;
    let card_id = card["id"].as_str().expect("an id").to_owned();
    assert_eq!(card["body"]["content"][0]["attrs"]["level"], 3, "{card}");
    assert_eq!(card["body"]["content"][2]["type"], "image", "{card}");
    let agent = agent_writing(&th, &th.client).await;

    let read = in_markdown(&agent, &card_id).await;
    assert_eq!(read.get("body"), Some(&Value::Null), "{read}");
    assert_eq!(read["hasComplexBlocks"], false, "{read}");
    assert_eq!(read["complexBlocks"], json!([]), "{read}");
    let markdown = read["bodyMarkdown"].as_str().expect("Markdown");
    let shown_as = th.client.me().await.username;
    for said in [
        "### Findings".to_owned(),
        format!("@[{shown_as}](fvoci-user://"),
        "$x^2$".to_owned(),
        format!("![the moon](fvoci-file://{image})"),
        "[report.pdf](fvoci-file://".to_owned(),
    ] {
        assert!(markdown.contains(&said), "{said} in {markdown}");
    }
    let plain: Value = agent.get(&card_path(&card_id)).await.ok_json();
    assert!(plain.get("bodyMarkdown").is_none(), "{plain}");

    // Written back as it was read, the body is the same; past 64 KiB, it is written.
    let same: Value = write_markdown(&agent, &plain, markdown).await.ok_json();
    assert_eq!(same["body"], plain["body"], "{markdown}");
    let mut long = String::new();
    for line in 0..1500 {
        writeln!(
            long,
            "Line {line} of what the agent wrote down, at length.\n"
        )
        .expect("written");
    }
    let rewritten: Value = write_markdown(&agent, &same, &long).await.ok_json();
    let said = rewritten["body"].to_string();
    assert!(
        said.contains("Line 1499 of") && !said.contains("Findings"),
        "rewritten"
    );

    let stale = write_markdown(&agent, &plain, "too late").await;
    assert_eq!(stale.status, StatusCode::CONFLICT, "{}", stale.text());
    let both = agent
        .patch(
            &card_path(&card_id),
            &json!({"bodyMarkdown": "a", "body": paragraphs(&["b"]), "readAt": rewritten["updatedAt"]}),
        )
        .await;
    both.check_status(StatusCode::BAD_REQUEST);
    assert_eq!(both.refused_at(), "/bodyMarkdown", "{}", both.text());
    let unknown = agent
        .get(&format!("{}?format=html", card_path(&card_id)))
        .await;
    unknown.check_status(StatusCode::BAD_REQUEST);

    th.teardown().await;
}

#[tokio::test]
async fn a_body_markdown_does_not_hold_is_added_to_and_not_written_over() {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, fvoci_model::ContainerVisibility::Open)
        .await;
    let cell = |text: &str, attrs: Value| {
        json!({"type": "tableHeader", "attrs": attrs,
               "content": [{"type": "paragraph", "content": [{"type": "text", "text": text}]}]})
    };
    let merged = json!({"type": "doc", "content": [{"type": "table", "content": [
        {"type": "tableRow", "content": [cell("both columns", json!({"colspan": 2}))]},
        {"type": "tableRow", "content": [cell("a", json!({})), cell("b", json!({}))]},
    ]}]});
    // A body the session wrote and no room ever held: it is not in any document yet.
    let card: Value = th
        .client
        .post(
            &format!("{TEST_WS}/boards/{}/cards", board.id),
            &json!({"title": "merged", "body": merged}),
        )
        .await
        .created_json();
    let card_id = card["id"].as_str().expect("an id").to_owned();
    let agent = agent_writing(&th, &th.client).await;

    let read = in_markdown(&agent, &card_id).await;
    assert_eq!(read["hasComplexBlocks"], true, "{read}");
    assert_eq!(
        read["complexBlocks"][0],
        json!({"path": "/content/0/content/0/content/0", "reason": "merged_cell"}),
        "{read}"
    );
    check_refused(
        &write_markdown(&agent, &card, "plain now").await,
        StatusCode::CONFLICT,
        "complex_blocks_use_append",
    );

    let added: Value = append(&agent, &card_id, "#### Added after\n\n[[not a reference]]")
        .await
        .ok_json();
    let content = added["body"]["content"].as_array().expect("blocks");
    assert_eq!(content.len(), 3, "{added}");
    assert_eq!(
        content[0]["content"][0]["content"][0]["attrs"]["colspan"], 2,
        "{added}"
    );
    assert_eq!(content[1]["attrs"]["level"], 3, "{added}");
    assert_eq!(
        content[2]["content"][0]["text"], "[[not a reference]]",
        "{added}"
    );

    let not_the_boards = append(&agent, &card_id, "![x](fvoci-file://elsewhere.png)").await;
    not_the_boards.check_status(StatusCode::BAD_REQUEST);
    assert_eq!(
        not_the_boards.refused_at(),
        "/markdown",
        "{}",
        not_the_boards.text()
    );

    th.teardown().await;
}

#[tokio::test]
async fn an_agent_writes_no_body_whole_while_people_edit_it() {
    let th = Setup::native().start().await.init_basic().await;
    let (table, mut one, _two) = two_in_a_room(&th, &paragraphs(&["before"])).await;
    let agent = agent_writing(&th, &th.client).await;
    one.write_acked("typed").await;
    let card: Value = agent.get(&card_path(&table.card_id)).await.ok_json();

    check_refused(
        &write_markdown(&agent, &card, "over it").await,
        StatusCode::CONFLICT,
        "room_has_active_editors",
    );
    let whole = agent
        .patch(
            &card_path(&table.card_id),
            &json!({"body": paragraphs(&["over it"]), "readAt": card["updatedAt"]}),
        )
        .await;
    check_refused(&whole, StatusCode::CONFLICT, "room_has_active_editors");
    assert_eq!(
        one.lines(),
        ["before", "typed"],
        "the editor keeps the document"
    );
    append(&agent, &table.card_id, "after it").await.check_ok();

    th.teardown().await;
}

/// Waits for the server to have the update a peer sent.
async fn acked(peer: &mut Peer) {
    let ack = peer.text("docAck").await;
    assert_eq!(ack["doc"], json!(peer.doc), "{ack}");
}

/// An agent's whole write that waits for the card while an editor's update holds it,
/// as another editor's update waits for the room, is answered, and the editors go on:
/// the write holds the card and does not wait for the room the update waits in.
#[tokio::test]
async fn an_agent_writing_whole_between_two_editors_is_answered_and_they_go_on() {
    let th = Setup::native().start().await.init_basic().await;
    let (table, mut one, mut two) = two_in_a_room(&th, &paragraphs(&["before"])).await;
    let agent = agent_writing(&th, &th.client).await;
    let card: Value = agent.get(&card_path(&table.card_id)).await.ok_json();

    let (held, go) = th.app.collab().pause_next_log();
    one.write("first").await;
    tokio::time::timeout(PATIENCE, held.notified())
        .await
        .expect("the first update holds the room and the card");
    two.write("second").await;
    let (written, ()) = tokio::join!(
        tokio::time::timeout(PATIENCE, write_markdown(&agent, &card, "over it")),
        async {
            // The second update waits for the room and the write for the card.
            tokio::time::sleep(QUIET).await;
            go.notify_one();
        },
    );
    let written = written.expect("the whole write is answered");
    check_refused(&written, StatusCode::CONFLICT, "room_has_active_editors");
    acked(&mut one).await;
    acked(&mut two).await;

    for round in 0..8 {
        one.write(&format!("one {round}")).await;
        two.write(&format!("two {round}")).await;
        let card: Value = agent.get(&card_path(&table.card_id)).await.ok_json();
        let written = tokio::time::timeout(PATIENCE, write_markdown(&agent, &card, "over it"))
            .await
            .unwrap_or_else(|_| panic!("the whole write of round {round} is answered"));
        check_refused(&written, StatusCode::CONFLICT, "room_has_active_editors");
        acked(&mut one).await;
        acked(&mut two).await;
    }
    two.settle().await;
    assert!(
        two.lines().contains(&"one 7".to_owned()),
        "the editors' writing goes on: {:?}",
        two.lines()
    );

    th.teardown().await;
}
