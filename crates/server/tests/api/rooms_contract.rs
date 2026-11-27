//! The JSON messages of the bodies edited together, as the server sends them over a
//! real connection, against `contracts/websocket.json`.

use std::collections::BTreeSet;

use fvoci_server::collab::Outgoing;
use serde_json::{Value, json};

use crate::common::{Setup, TEST_WORKSPACE_ID, TestServer};
use crate::rooms::{Peer, UPDATE, frame, give, paragraphs, table};
use crate::websocket::{check_message, read_contract};

/// A connection of `user_id` that reads nothing of what it is sent, in the room of
/// the card with `one` and `two`: the `docLeft` it finds when it reads at last.
async fn left_for_not_reading(
    th: &TestServer,
    card_id: &str,
    user_id: &str,
    one: &mut Peer,
    two: &mut Peer,
) -> Value {
    // A connection that reads nothing of what it is sent: its place among the rooms
    // is taken from the server itself, and what waits for it is not asked for. With
    // as many messages waiting as may, it is put out, and the others go on.
    let collab = th.app.collab();
    let join = json!({
        "type": "joinDoc", "workspaceId": TEST_WORKSPACE_ID, "cardId": card_id,
    })
    .to_string();
    let (mut slow, waiting) = collab.seat(
        &th.app,
        user_id,
        fvoci_server::app::SessionKept::with_password(),
    );
    assert!(slow.command(&join).await, "a command of the rooms");
    let app = th
        .app
        .enter_workspace_as(
            fvoci_server::app::Asker::with_password(user_id),
            TEST_WORKSPACE_ID,
        )
        .await
        .expect("the membership is read")
        .expect("a member")
        .app;
    // Sixty-four may wait here. `docJoined` and the document wait already: 62 more,
    // and the next is one too many.
    for n in 0..62 {
        one.write_acked(&format!("{n}")).await;
    }
    assert_eq!(collab.room_members(&app, card_id).await, 3, "still in");
    one.write_acked("one too many").await;
    assert_eq!(collab.room_members(&app, card_id).await, 2, "put out");
    // The answer it was given as it came in waits on; the document and the updates
    // that waited are dropped for the notice.
    let Some(Outgoing::Text(joined)) = waiting.next().await else {
        panic!("the answer to joinDoc waits on");
    };
    assert!(joined.contains("docJoined"), "{joined}");
    let Some(Outgoing::Text(left)) = waiting.next().await else {
        panic!("what waited of the document is dropped for the notice");
    };
    let left: Value = serde_json::from_str(&left).expect("JSON");
    assert_eq!(
        left,
        json!({"type": "docLeft", "doc": 1, "reason": "connection_too_slow"}),
        "put out for what waited"
    );
    assert!(!slow.overrun().is_cancelled(), "the connection stays");
    two.settle().await;
    assert_eq!(two.lines().len(), 64, "the others were sent all of it");
    assert_eq!(one.body(), two.body(), "and hold the same");
    // It comes in again, and is sent the whole document.
    assert!(slow.command(&join).await, "a command of the rooms");
    let Some(Outgoing::Text(joined)) = waiting.next().await else {
        panic!("the answer to joinDoc");
    };
    let joined: Value = serde_json::from_str(&joined).expect("JSON");
    assert_eq!(
        (&joined["type"], &joined["doc"]),
        (&json!("docJoined"), &json!(2)),
        "{joined}"
    );
    assert!(
        matches!(waiting.next().await, Some(Outgoing::Binary(_))),
        "the whole document follows"
    );
    slow.leave_all().await;
    left
}

/// Every message the server sends about a body edited together has exactly the
/// members `contracts/websocket.json` lists for it.
#[tokio::test]
async fn the_messages_of_the_rooms_match_the_websocket_contract() {
    let contract = read_contract("websocket.json");
    let openapi = read_contract("openapi.json");
    let th = Setup::native()
        .configure(|config| {
            config.collab.collab_recheck_seconds = 1;
            // The test writes faster than an editor does.
            config.collab.collab_frames_per_second = 1_000_000;
            config.collab.collab_queue_messages = 64;
        })
        .start()
        .await
        .init_basic()
        .await;
    let t = table(&th, &paragraphs(&["first"])).await;
    let user2 = th.client2.me().await.id;
    give(&th, &t.board_id, &user2, "editor").await;
    let mut heard = BTreeSet::new();
    let mut check = |message: &Value| {
        check_message(&contract, &openapi, message);
        heard.insert(message["type"].as_str().expect("a message type").to_owned());
    };

    let mut one = Peer::connect(&th.client, 101).await;
    let mut two = Peer::connect(&th.client2, 102).await;
    check(&one.joined(TEST_WORKSPACE_ID, &t.card_id).await);
    check(&one.join(TEST_WORKSPACE_ID, "no card").await);
    one.write("typed").await;
    check(&one.text("docAck").await);
    one.send_binary(frame(one.doc, one.epoch, UPDATE, b"no update"))
        .await;
    check(&one.text("docNack").await);
    one.settle().await;

    // The card's body is written whole over HTTP, which makes its document again;
    // who comes in next is sent the new one, and so is who was there.
    crate::rooms::write_whole(&th.client, &t.card_id, &paragraphs(&["written whole"])).await;
    two.joined(TEST_WORKSPACE_ID, &t.card_id).await;
    let reset = one.text("docReset").await;
    check(&reset);
    assert_eq!(reset["epoch"], json!(two.epoch), "{reset}");
    assert_ne!(two.epoch, 1, "another document");
    one.settle().await;
    assert_eq!(one.lines(), ["written whole"]);
    assert_eq!(one.body(), two.body());

    // A role lowered while its holder is in the room: told at the next asking.
    give(&th, &t.board_id, &user2, "viewer").await;
    check(&two.text("docModeChanged").await);
    give(&th, &t.board_id, &user2, "editor").await;
    assert_eq!(two.text("docModeChanged").await["mode"], json!("write"));

    check(&left_for_not_reading(&th, &t.card_id, &user2, &mut one, &mut two).await);

    let listed: BTreeSet<String> = contract["messages"]
        .as_object()
        .expect("the messages are an object")
        .keys()
        .filter(|kind| kind.starts_with("doc"))
        .cloned()
        .collect();
    assert_eq!(
        heard, listed,
        "the test hears the messages of the rooms the contract lists"
    );
    th.teardown().await;
}
