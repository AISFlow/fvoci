//! A reader by a shared board's link is kept out of the rooms, and what the rooms
//! write to a card reaches the reader naming nobody.

use serde_json::{Value, json};

use crate::collab::{body_of, card};
use crate::common::{Setup, TEST_WORKSPACE_ID};
use crate::rooms::{Incoming, Peer, QUIET, table};
use crate::websocket::PATIENCE;

/// Where the body of the card of this file's test names somebody.
const MENTION: &str = "/content/0/content/1/attrs/userId";

/// Reads `reader` until it is told of `card_id` with `text` in its summary, and then
/// until nothing comes for a while: every message is JSON that does not name `me`,
/// and none is a frame of a room. The message that told of the summary.
async fn told_of(reader: &mut Peer, card_id: &str, text: &str, me: &str) -> Value {
    let deadline = tokio::time::Instant::now() + PATIENCE;
    let mut told: Option<Value> = None;
    loop {
        let wait = if told.is_some() {
            QUIET
        } else {
            deadline.saturating_duration_since(tokio::time::Instant::now())
        };
        match reader.incoming(wait).await {
            Incoming::Nothing => {
                return told
                    .unwrap_or_else(|| panic!("the reader is told of the body with {text:?}"));
            }
            Incoming::Text(message) => {
                assert!(
                    !message.to_string().contains(me),
                    "nobody is named: {message}"
                );
                let about = message["type"] == "blockChanged"
                    && message["block"]["id"] == card_id
                    && message["summary"]["preview"]["text"]
                        .as_str()
                        .is_some_and(|preview| preview.contains(text));
                if about && told.is_none() {
                    told = Some(message);
                }
            }
            other => panic!("a reader is sent nothing of a room: {other:?}"),
        }
    }
}

/// The connection of a shared board's reader is refused the card's room as if the
/// card were not there (`rooms::who_may_not_read_is_refused_as_for_no_card` has the
/// reader without a session and a user of no workspace; here, also a member of the
/// workspace who is not on the board). What a member types in the room reaches the
/// reader as the summary of the card's body over the subscription and as its body over
/// HTTP, naming nobody, and no frame of the room reaches the reader; the member reads
/// the mention as it is.
#[tokio::test]
async fn a_reader_by_link_is_kept_out_of_the_room_and_told_its_body_naming_nobody() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let me = th.client.me().await.id;
    let body = json!({"type": "doc", "content": [{"type": "paragraph", "content": [
        {"type": "text", "text": "ask "},
        {"type": "mention", "attrs": {"userId": me}},
    ]}]});
    let t = table(&th, &body).await;
    let token = th.share(&t.board_id).await;

    let mut readers = [
        Peer::on(th.anonymous().as_browser().connect_websocket().await, 201),
        Peer::on(th.client2.connect_websocket().await, 202),
    ];
    for reader in &mut readers {
        reader
            .send_json(json!({
                "type": "subscribeBlocks", "workspaceId": TEST_WORKSPACE_ID,
                "readToken": token, "blockIds": [t.card_id],
            }))
            .await;
        reader
            .send_json(json!({
                "type": "joinDoc", "workspaceId": TEST_WORKSPACE_ID,
                "cardId": t.card_id, "readToken": token,
            }))
            .await;
        // Answered after the subscription ACK: the reader follows the card from here on.
        assert_eq!(
            reader.incoming(PATIENCE).await,
            Incoming::Text(json!({
                "type": "blocksSubscribed", "workspaceId": TEST_WORKSPACE_ID,
                "blockIds": [t.card_id],
            })),
            "subscription acknowledged before joinDoc"
        );
        assert_eq!(
            reader.incoming(PATIENCE).await,
            Incoming::Text(json!({
                "type": "docRefused", "workspaceId": TEST_WORKSPACE_ID,
                "cardId": t.card_id, "reason": "not_found",
            })),
            "reader by link refused from editing room"
        );
    }

    let mut writer = Peer::connect(&th.client, 101).await;
    writer.joined(TEST_WORKSPACE_ID, &t.card_id).await;
    // Where the writer is in the body is told to the room, which no reader is in.
    let place = json!({"tname": "default", "assoc": 0});
    let caret = json!({"cursor": {"anchor": place, "head": place}});
    writer
        .send_caret(&crate::rooms::awareness(writer.client(), 1, &caret))
        .await;
    writer.write_acked("typed in the room").await;
    assert_eq!(th.app.collab().room_members(&th.app, &t.card_id).await, 1);
    th.app
        .collab()
        .write_body(&th.app, &t.card_id)
        .await
        .expect("the room's body is written");

    for reader in &mut readers {
        let told = told_of(reader, &t.card_id, "typed in the room", &me).await;
        // The body is read over HTTP; the summary's preview has nothing of the mention.
        assert_eq!(told["block"]["body"], Value::Null, "{told}");
        assert_eq!(told["summary"]["hasBody"], json!(true), "{told}");
    }
    let read: Vec<Value> = th
        .anonymous()
        .get(&format!(
            "/shared/{}/blocks?blockId={}&readToken={token}",
            t.board_id, t.card_id
        ))
        .await
        .ok_json();
    let [shown] = read.as_slice() else {
        panic!("the card alone: {read:?}")
    };
    assert!(
        shown["body"].to_string().contains("typed in the room"),
        "{shown}"
    );
    assert!(!shown.to_string().contains(&me), "nobody is named: {shown}");
    assert_eq!(shown["body"].pointer(MENTION), Some(&json!("")), "{shown}");
    let member = body_of(&card(&th.client, &t.card_id).await).expect("a body");
    assert_eq!(member.pointer(MENTION), Some(&json!(me)), "{member}");
    th.teardown().await;
}
