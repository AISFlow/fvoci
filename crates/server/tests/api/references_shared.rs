//! What the references of a body name to whoever reads a board by its link: the cards
//! of that board, and nothing else. A reference to anything else of the workspace goes
//! out without its identifier, whatever it named, so that the reader learns neither
//! what is elsewhere nor whether anything is.

use std::time::Duration;

use serde_json::{Value, json};

use crate::common::{Client, Setup, TEST_WORKSPACE_ID, TEST_WS, TestServer, UNKNOWN_ID};
use crate::references::{HIDDEN, made};
use crate::references_to::{body_naming, card_names, card_of, comment_naming};
use crate::websocket::Socket;

/// The identifiers the references below `node` hold, in the order they stand.
pub(crate) fn references_in(node: &Value) -> Vec<String> {
    let mut held = Vec::new();
    if node["type"] == "reference" {
        held.push(node["attrs"]["id"].as_str().expect("an id").to_owned());
    }
    for child in node["content"].as_array().into_iter().flatten() {
        held.extend(references_in(child));
    }
    held
}

/// The block with the identifier among the blocks of an answer.
fn block_of<'a>(blocks: &'a Value, id: &str) -> &'a Value {
    blocks
        .as_array()
        .expect("a list")
        .iter()
        .find(|block| block["id"] == id)
        .unwrap_or_else(|| panic!("no block {id} in {blocks}"))
}

/// A shared board whose card `source` names a card beside it, a template of the
/// board's cards, itself, and much that is not of the board: a card and a page
/// elsewhere, a page, a card of the board in the trash, and nothing. A comment on it
/// names a card elsewhere and the card beside it.
struct Shared {
    board: String,
    token: String,
    source: String,
    comment: String,
    beside: String,
    /// What the card names, as it was written.
    named: Vec<String>,
    /// What of that is not of the board.
    outside: Vec<String>,
    /// What whoever reads by the link is told the card names.
    kept: Vec<String>,
}

async fn shared_board(th: &TestServer) -> Shared {
    let owner = &th.client;
    let shared = made(owner, TEST_WS, "private", "shared").await;
    let elsewhere = made(owner, TEST_WS, "private", HIDDEN).await;
    let board = shared.board.as_str();
    let beside = card_of(owner, TEST_WS, board, "beside").await;
    let gone = card_of(owner, TEST_WS, board, HIDDEN).await;
    let source = card_of(owner, TEST_WS, board, "source").await;
    let template = owner
        .post(
            &format!("{TEST_WS}/boards/{board}/cards"),
            &json!({"title": "a template", "isTemplate": true}),
        )
        .await;
    template.check_created();
    let template = template.value()["id"].as_str().expect("an id").to_owned();
    let outside = vec![
        elsewhere.card.clone(),
        elsewhere.page.clone(),
        shared.page.clone(),
        gone.clone(),
        UNKNOWN_ID.to_owned(),
    ];
    let mut named = vec![beside.clone()];
    named.extend(outside.iter().cloned());
    named.extend([source.clone(), template.clone()]);
    let targets: Vec<&str> = named.iter().map(String::as_str).collect();
    card_names(owner, TEST_WS, &source, &targets).await;
    let comment = comment_naming(owner, board, &source, &[&elsewhere.card, &beside]).await;
    crate::blocks::trash_card(owner, board, &gone)
        .await
        .check_no_content();
    let mut kept = vec![beside.clone()];
    kept.extend(outside.iter().map(|_| String::new()));
    kept.extend([source.clone(), template]);
    Shared {
        token: th.share(board).await,
        board: shared.board,
        source,
        comment,
        beside,
        named,
        outside,
        kept,
    }
}

/// Whoever reads by the link, with a session or without, gets the cards of the board
/// that a body names (a template among them) and an empty identifier for each of the
/// rest; a member on the board's own routes gets them all.
#[tokio::test]
async fn a_reader_by_a_link_is_told_of_the_cards_of_the_board_and_of_nothing_else() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let owner = &th.client;
    let shared = shared_board(&th).await;
    let (board, token, source) = (&shared.board, &shared.token, &shared.source);
    let tells_nothing = |text: &str, what: &str| {
        for id in &shared.outside {
            assert!(!text.contains(id.as_str()), "{what} tells of {id}: {text}");
        }
        assert!(!text.contains(HIDDEN), "{what}: {text}");
    };
    let anonymous = th.anonymous();
    for reader in [&anonymous, &th.client2, owner] {
        let all = reader
            .get(&format!(
                "/shared/{board}/blocks?all=true&readToken={token}"
            ))
            .await;
        all.check_ok();
        tells_nothing(&all.text(), "every block");
        let blocks = all.value();
        assert_eq!(
            references_in(&block_of(&blocks, source)["body"]),
            shared.kept
        );
        assert_eq!(
            references_in(&block_of(&blocks, &shared.comment)["body"]),
            ["", shared.beside.as_str()]
        );
        // The cards come with no body (an empty one), so with nothing a body names.
        let cards = reader
            .get(&format!("/shared/{board}/cards?readToken={token}"))
            .await
            .value();
        let cards = cards.as_array().expect("a list");
        assert!(
            !cards.is_empty() && cards.iter().all(|card| card["body"].is_null()),
            "{cards:?}"
        );
        for rest in [
            format!("blocks?blockId={source}"),
            format!("blocks?parentId={source}&type=comment"),
            "cards?".to_owned(),
            "cards?preview=true".to_owned(),
        ] {
            let answer = reader
                .get(&format!("/shared/{board}/{rest}&readToken={token}"))
                .await;
            answer.check_ok();
            tells_nothing(&answer.text(), &rest);
        }
    }
    // A member on the board's own routes is told every identifier, as it was written.
    let own = owner
        .get(&format!("{TEST_WS}/cards/{source}"))
        .await
        .value();
    assert_eq!(references_in(&own["body"]), shared.named);
    th.teardown().await;
}

/// A connection that follows the card by the link is told of a change of the card (its
/// summary: the body is read over HTTP, above), and of a comment on it with its body,
/// without what is not of the board: one without a session,
/// and one of a user who is signed in and not on the board.
#[tokio::test]
async fn a_connection_by_a_link_is_told_of_changes_without_what_is_not_of_the_board() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let shared = shared_board(&th).await;
    for reader in [th.anonymous().as_browser(), th.client2.clone()] {
        let mut socket = Socket::connect_as(&reader).await;
        socket
            .send(json!({
                "type": "subscribeBlocks", "workspaceId": TEST_WORKSPACE_ID,
                "readToken": shared.token, "blockIds": [shared.source],
            }))
            .await;
        let told = told_of_the_card(&mut socket, &th.client, &shared).await;
        assert_eq!(told["block"]["body"], Value::Null, "{told}");
        assert_eq!(told["summary"]["hasBody"], json!(true), "{told}");
        // A comment on the card is told to who follows the card.
        let elsewhere = shared.outside[0].as_str();
        comment_naming(
            &th.client,
            &shared.board,
            &shared.source,
            &[elsewhere, &shared.beside],
        )
        .await;
        let of_the_comment = loop {
            let message = socket.next_json(Duration::from_secs(5)).await;
            let message = message.expect("the reader is told of the comment");
            if message["block"]["type"] == "comment" {
                break message;
            }
        };
        assert_eq!(
            references_in(&of_the_comment["block"]["body"]),
            ["", shared.beside.as_str()],
            "{of_the_comment}"
        );
        for told in [told, of_the_comment] {
            let text = told.to_string();
            for id in &shared.outside {
                assert!(!text.contains(id.as_str()), "tells of {id}: {text}");
            }
        }
    }
    th.teardown().await;
}

/// The card is written again until the connection, which follows it by the link, is
/// told: the first thing it is told of the card.
async fn told_of_the_card(socket: &mut Socket, owner: &Client, shared: &Shared) -> Value {
    let named: Vec<&str> = shared.named.iter().map(String::as_str).collect();
    // No acknowledgement: the card is written until the subscription shows.
    let path = format!("{TEST_WS}/cards/{}", shared.source);
    for round in 0..50 {
        let read_at = owner.get(&path).await.value()["updatedAt"].clone();
        let body = body_naming(&format!("round {round}"), &named);
        owner
            .patch(&path, &json!({"readAt": read_at, "body": body}))
            .await
            .check_ok();
        while let Some(message) = socket.next_json(Duration::from_millis(100)).await {
            if message["type"] == json!("blocksSubscribed") {
                continue;
            }
            assert_eq!(message["type"], json!("blockChanged"), "{message}");
            return message;
        }
    }
    panic!("the reader is not told of the card");
}
