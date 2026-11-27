//! The limits of the rooms: how many a connection is in, how many are in one, how
//! fast a connection sends and how large, and what waits for one that does not read.

use std::time::Duration;

use fvoci_model::Card;
use serde_json::json;

use crate::common::{Setup, TEST_WORKSPACE_ID, TestServer};
use crate::rooms::{
    Incoming, PATIENCE, Peer, QUIET, STEP_1, UPDATE, frame, give, no_rooms, paragraphs, table,
    two_in_a_room,
};

/// A connection is in eight rooms and no ninth; one it leaves makes room for
/// another, and to come anew into one it is in takes none.
#[tokio::test]
async fn a_connection_is_in_eight_rooms_and_no_more() {
    let th = Setup::native()
        // Coming into ten rooms at once is more than a connection does in a second.
        .configure(|config| config.collab.collab_frames_per_second = 1000)
        .start()
        .await
        .init_basic()
        .await;
    let t = table(&th, &paragraphs(&["first"])).await;
    let mut cards = vec![t.card_id.clone()];
    for n in 1..9 {
        let card: Card = th
            .client
            .post(
                &format!("/w/{TEST_WORKSPACE_ID}/boards/{}/cards", t.board_id),
                &json!({"title": format!("card {n}")}),
            )
            .await
            .created_json();
        cards.push(card.id);
    }
    let mut peer = Peer::connect(&th.client, 101).await;
    let mut docs = Vec::new();
    for card_id in &cards[..8] {
        let joined = peer.joined(TEST_WORKSPACE_ID, card_id).await;
        docs.push(joined["doc"].clone());
    }
    let refused = peer.join(TEST_WORKSPACE_ID, &cards[8]).await;
    assert_eq!(
        refused,
        json!({
            "type": "docRefused", "workspaceId": TEST_WORKSPACE_ID, "cardId": cards[8],
            "reason": "too_many_documents_joined",
        })
    );
    // Anew into one it is in: no ninth.
    peer.joined(TEST_WORKSPACE_ID, &cards[7]).await;
    assert_eq!(th.app.collab().rooms(), 8);
    peer.send_json(json!({"type": "leaveDoc", "doc": docs[0]}))
        .await;
    peer.joined(TEST_WORKSPACE_ID, &cards[8]).await;
    drop(peer);
    no_rooms(&th).await;
    th.teardown().await;
}

/// Fifty connections are in the room of a body, and no fifty-first until one leaves.
#[tokio::test]
async fn fifty_connections_are_in_a_room_and_no_more() {
    let th = Setup::native()
        // One user's here: the limit of one user's is another test's.
        .configure(|config| config.collab.collab_connections_per_user = 50)
        .start()
        .await
        .init_basic()
        .await;
    let t = table(&th, &paragraphs(&["first"])).await;
    let mut peers = Vec::new();
    for client in 0..50 {
        let mut peer = Peer::connect(&th.client, 100 + client).await;
        peer.joined(TEST_WORKSPACE_ID, &t.card_id).await;
        peers.push(peer);
    }
    // Another user's, so that it is the room that is full and not the user's share.
    give(&th, &t.board_id, &th.client2.me().await.id, "viewer").await;
    let mut late = Peer::connect(&th.client2, 200).await;
    let refused = late.join(TEST_WORKSPACE_ID, &t.card_id).await;
    assert_eq!(refused["reason"], json!("document_full"), "{refused}");
    // Who is in it comes in anew all the same, and all are sent what one writes.
    peers[0].joined(TEST_WORKSPACE_ID, &t.card_id).await;
    peers[0].write_acked("to forty-nine").await;
    for peer in &mut peers[1..] {
        let frame = match peer.incoming(PATIENCE).await {
            Incoming::Frame(frame) => frame,
            other => panic!("the update reaches everyone: {other:?}"),
        };
        assert_eq!(frame.sync, UPDATE);
    }
    let mut left = peers.pop().expect("fifty");
    left.leave().await;
    while th
        .app
        .collab()
        .room_members(&room_app(&th).await, &t.card_id)
        .await
        == 50
    {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    late.joined(TEST_WORKSPACE_ID, &t.card_id).await;
    th.teardown().await;
}

/// Five connections of one user are in the room of a body, and no sixth until one of
/// them leaves; another user's comes in beside them, and one of the five comes in
/// anew.
#[tokio::test]
async fn five_connections_of_a_user_are_in_a_room_and_no_sixth() {
    let th = Setup::native().start().await.init_basic().await;
    let t = table(&th, &paragraphs(&["first"])).await;
    give(&th, &t.board_id, &th.client2.me().await.id, "viewer").await;
    let mut tabs = Vec::new();
    for client in 0..5 {
        let mut tab = Peer::connect(&th.client, 100 + client).await;
        tab.joined(TEST_WORKSPACE_ID, &t.card_id).await;
        tabs.push(tab);
    }
    let mut sixth = Peer::connect(&th.client, 200).await;
    let refused = sixth.join(TEST_WORKSPACE_ID, &t.card_id).await;
    assert_eq!(
        refused,
        json!({
            "type": "docRefused", "workspaceId": TEST_WORKSPACE_ID, "cardId": t.card_id,
            "reason": "too_many_connections_joined",
        })
    );
    let mut another = Peer::connect(&th.client2, 201).await;
    another.joined(TEST_WORKSPACE_ID, &t.card_id).await;
    tabs[0].joined(TEST_WORKSPACE_ID, &t.card_id).await;
    let app = room_app(&th).await;
    assert_eq!(th.app.collab().room_members(&app, &t.card_id).await, 6);
    tabs[1].leave().await;
    while th.app.collab().room_members(&app, &t.card_id).await == 6 {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    sixth.joined(TEST_WORKSPACE_ID, &t.card_id).await;
    th.teardown().await;
}

/// The application as the organization of the tests' workspace reaches it.
async fn room_app(th: &TestServer) -> fvoci_server::app::App {
    let user = th.client.me().await.id;
    th.app
        .enter_workspace_as(
            fvoci_server::app::Asker::with_password(&user),
            TEST_WORKSPACE_ID,
        )
        .await
        .expect("the membership is read")
        .expect("a member")
        .app
}

/// A connection that sends more frames than it may is put out of the room the frame
/// past the limit was for, after the answers to the frames before it, and a
/// `joinDoc` past the limit is refused; the other in the room is sent what was
/// taken, and goes on.
#[tokio::test]
async fn frames_past_the_rate_put_a_connection_out_of_the_room() {
    let th = Setup::native()
        .configure(|config| config.collab.collab_frames_per_second = 10)
        .start()
        .await
        .init_basic()
        .await;
    th.app.collab().pause_frame_rate_clock();
    let (t, mut one, mut two) = two_in_a_room(&th, &paragraphs(&["first"])).await;
    one.barrier(&th).await;
    // What coming in cost is made up for: ten frames at once from here.
    th.app
        .collab()
        .advance_frame_rate_clock(Duration::from_secs(1));
    // All of it before anything is read. Three updates, which are taken; seven frames
    // for a room it is not in, which spend the rest of the ten; an update past the
    // limit; and to come in again.
    for n in 0..3 {
        one.write(&format!("{n}")).await;
    }
    for _ in 0..7 {
        one.send_binary(frame(999, 1, UPDATE, &[0, 0])).await;
    }
    one.write("past the limit").await;
    one.send_json(json!({
        "type": "joinDoc", "workspaceId": TEST_WORKSPACE_ID, "cardId": t.card_id,
    }))
    .await;
    for _ in 0..3 {
        let answer = one.incoming(PATIENCE).await;
        assert!(
            matches!(&answer, Incoming::Text(ack) if ack["type"] == json!("docAck")),
            "the answers to what was taken come first: {answer:?}"
        );
    }
    assert_eq!(
        one.incoming(PATIENCE).await,
        Incoming::Text(json!({
            "type": "docLeft", "doc": one.doc, "reason": "too_many_frames",
        })),
        "put out by the update past the limit"
    );
    let refused = one.text("docRefused").await;
    assert_eq!(refused["reason"], json!("too_many_frames"), "{refused}");
    two.settle().await;
    assert_eq!(
        two.lines(),
        ["first", "0", "1", "2"],
        "what was taken, and no more"
    );

    // A join costs five frames (half a second at ten a second): 499 ms is not enough,
    // and one more millisecond is.
    one.barrier(&th).await;
    th.app
        .collab()
        .advance_frame_rate_clock(Duration::from_millis(499));
    let still_refused = one.join(TEST_WORKSPACE_ID, &t.card_id).await;
    assert_eq!(
        still_refused["reason"],
        json!("too_many_frames"),
        "{still_refused}"
    );
    one.barrier(&th).await;
    th.app
        .collab()
        .advance_frame_rate_clock(Duration::from_millis(1));
    one.joined(TEST_WORKSPACE_ID, &t.card_id).await;
    assert_eq!(one.body(), two.body());
    two.write_acked("goes on").await;
    one.settle().await;
    assert_eq!(one.body(), two.body());
    th.teardown().await;
}

/// An update of `count` changes of a client that each name the root of the body as
/// their parent and no neighbour, as its bytes: the Y library walks past every child
/// of the root to place each.
fn crowding(client: u64, count: u64) -> Vec<u8> {
    let number = |out: &mut Vec<u8>, mut number: u64| {
        while number >= 0x80 {
            out.push(number.to_le_bytes()[0] | 0x80);
            number >>= 7;
        }
        out.push(number.to_le_bytes()[0]);
    };
    let mut out = Vec::new();
    // The changes of one client, from its first clock.
    number(&mut out, 1);
    number(&mut out, count);
    number(&mut out, client);
    number(&mut out, 0);
    for _ in 0..count {
        // Deleted content of one place, under the root by its name.
        out.extend_from_slice(&[1, 1]);
        let root = fvoci_collab::FIELD.as_bytes();
        number(
            &mut out,
            u64::try_from(root.len()).expect("the length of a name"),
        );
        out.extend_from_slice(root);
        out.push(1);
    }
    // Nothing deleted.
    out.push(0);
    out
}

/// An update made to have the worker walk the children of the root for each of its
/// changes is refused by its name before it is placed, goes to nobody, and the room
/// goes on; the same changes in a number the worker places in time are taken.
#[tokio::test]
async fn an_update_made_to_walk_a_long_list_is_refused_and_the_room_goes_on() {
    let th = Setup::native().start().await.init_basic().await;
    let lines = vec!["x"; 20_000];
    let (_t, mut one, mut two) = two_in_a_room(&th, &paragraphs(&lines)).await;
    // 900 changes past 20,000 children are 18 million steps, past the 3 Mi of
    // `fvoci_collab::PLACING_STEPS`; 59 kB of them past 50,000 took 12 seconds.
    let began = std::time::Instant::now();
    one.send_update(&crowding(1 << 40, 900)).await;
    let nack = one.text("docNack").await;
    assert_eq!(
        nack,
        json!({"type": "docNack", "doc": one.doc, "epoch": 1, "reason": "update_refused"})
    );
    assert!(
        began.elapsed() < Duration::from_secs(5),
        "refused without the walk: {:?}",
        began.elapsed()
    );
    assert_eq!(
        two.incoming(QUIET).await,
        Incoming::Nothing,
        "sent to nobody"
    );
    one.send_update(&crowding(1 << 40, 20)).await;
    let ack = one.text("docAck").await;
    assert_eq!(ack["doc"], json!(one.doc), "{ack}");
    two.write_acked("goes on").await;
    th.teardown().await;
}

/// An update larger than an update may be is refused by its name and goes to nobody;
/// a frame larger than the websocket takes ends the connection, and the other in the
/// room goes on.
#[tokio::test]
async fn a_large_update_is_refused_and_a_larger_frame_ends_the_connection() {
    let th = Setup::native()
        .configure(|config| config.collab.collab_update_bytes = 2048)
        .start()
        .await
        .init_basic()
        .await;
    let (t, mut one, mut two) = two_in_a_room(&th, &paragraphs(&["first"])).await;
    one.write(&"x".repeat(3000)).await;
    let nack = one.text("docNack").await;
    assert_eq!(nack["reason"], json!("update_too_large"), "{nack}");
    assert_eq!(
        two.incoming(QUIET).await,
        Incoming::Nothing,
        "sent to nobody"
    );

    // Past what the websocket reads at all (64 KiB here).
    let large = vec![0_u8; 64 * 1024 + 1];
    // The server may close before the whole frame is written.
    one.try_send_binary(frame(one.doc, one.epoch, UPDATE, &large))
        .await;
    assert_eq!(one.incoming(PATIENCE).await, Incoming::Closed);
    two.write_acked("goes on").await;
    while th
        .app
        .collab()
        .room_members(&room_app(&th).await, &t.card_id)
        .await
        == 2
    {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(two.lines(), ["first", "goes on"]);
    th.teardown().await;
}

/// A connection that asks and asks for the document and reads nothing: once as many
/// messages wait for it as may (sixteen here, past what the system holds of a
/// connection nobody reads), it is put out of the room. Another connection in the
/// room, which reads, is sent everything that is written meanwhile and after, with
/// nothing held back for the one that does not; when that one reads at last it finds
/// that it was put out, comes in again and is sent the whole document.
#[tokio::test]
async fn a_connection_that_does_not_read_is_put_out_and_the_others_go_on() {
    let th = Setup::native()
        .configure(|config| {
            config.collab.collab_queue_messages = 16;
            config.collab.collab_frames_per_second = 1_000_000;
            // The bytes that may wait are far off: it is the messages that count.
            config.collab.collab_state_bytes = 64 * 1024 * 1024;
            // A worker's memory has room for a document that large (256 MiB and 32
            // times the state).
            config.collab.collab_worker_memory_bytes = 3 * 1024 * 1024 * 1024;
        })
        .start()
        .await
        .init_basic()
        .await;
    let t = table(&th, &paragraphs(&["first"])).await;
    let mut one = Peer::connect(&th.client, 101).await;
    // Its socket takes little of what it does not read: what waits for it waits in
    // the server, but for the few megabytes the server's own side of the socket holds.
    let mut slow = Peer::on(th.client.connect_websocket_receiving(64 * 1024).await, 102);
    let mut reader = Peer::connect(&th.client2, 103).await;
    give(&th, &t.board_id, &th.client2.me().await.id, "viewer").await;
    one.joined(TEST_WORKSPACE_ID, &t.card_id).await;
    slow.joined(TEST_WORKSPACE_ID, &t.card_id).await;
    reader.joined(TEST_WORKSPACE_ID, &t.card_id).await;
    let app = room_app(&th).await;
    // A document of some size, so that the answers fill that soon.
    one.write_acked(&"y".repeat(60 * 1024)).await;
    slow.settle().await;
    reader.settle().await;
    let mut written = vec!["first".to_owned(), "y".repeat(60 * 1024)];
    let (mut asked, mut sent_while_in) = (0, 0);
    while th.app.collab().room_members(&app, &t.card_id).await == 3 {
        // 60 KiB an answer: the bytes that may wait would hold some two thousand.
        assert!(
            asked < 1000,
            "not put out after {asked} answers: not for the messages that wait"
        );
        // Fewer at a time than may wait, so that it is in the room over several
        // updates whatever the system holds of what it does not read.
        for _ in 0..4 {
            slow.send_binary(frame(slow.doc, slow.epoch, STEP_1, &[0]))
                .await;
        }
        asked += 4;
        // The one that reads is sent each update as it is written: it holds it
        // before the next is.
        let line = format!("meanwhile {asked}");
        one.write_acked(&line).await;
        written.push(line);
        receive_update(&mut reader).await;
        assert_eq!(reader.lines(), written, "held by the one that reads");
        // Who is in a room stays until put out: still in now, it was in when the
        // update was sent on.
        if th.app.collab().room_members(&app, &t.card_id).await == 3 {
            sent_while_in += 1;
        }
    }
    assert!(
        sent_while_in >= 2,
        "the one that reads was sent {sent_while_in} updates while the other was in the room"
    );
    let line = "after it was put out";
    one.write_acked(line).await;
    written.push(line.to_owned());
    receive_update(&mut reader).await;
    assert_eq!(reader.lines(), written, "and after the other was put out");
    assert_eq!(reader.body(), one.body());
    assert_eq!(
        th.app.collab().room_members(&app, &t.card_id).await,
        2,
        "the two that read stay"
    );

    // It reads at last: answers that were on their way, and then the notice.
    let left = slow.text("docLeft").await;
    assert_eq!(
        left,
        json!({"type": "docLeft", "doc": slow.doc, "reason": "connection_too_slow"})
    );
    slow.joined(TEST_WORKSPACE_ID, &t.card_id).await;
    assert_eq!(slow.body(), one.body());
    assert_eq!(one.lines().last().map(String::as_str), Some(line));
    th.teardown().await;
}

/// Takes the next message of a connection, which is an update of the document it
/// holds, into that document.
async fn receive_update(peer: &mut Peer) {
    match peer.incoming(PATIENCE).await {
        Incoming::Frame(update) => {
            assert_eq!(
                (update.doc, update.epoch, update.sync),
                (peer.doc, peer.epoch, UPDATE),
                "an update of the document it holds"
            );
            peer.editor().receive(&update.payload);
        }
        other => panic!("the connection that reads is sent the update: {other:?}"),
    }
}

/// The connection that sent an update that ended the worker that took it is closed;
/// the other member's connection stays, and its user still writes.
#[tokio::test]
async fn the_connection_whose_update_ended_the_worker_is_closed() {
    let th = Setup::native()
        .configure(|config| config.collab.collab_worker_timeout_ms = 1000)
        .start()
        .await
        .init_basic()
        .await;
    let (t, mut one, mut two) = two_in_a_room(&th, &paragraphs(&["first"])).await;
    let app = room_app(&th).await;
    th.app.collab().stall_updates(&app, &t.card_id, 30_000);
    one.write("never").await;
    let closed = loop {
        match one.incoming(Duration::from_secs(20)).await {
            Incoming::Closed => break true,
            Incoming::Nothing => break false,
            _ => {}
        }
    };
    assert!(closed, "the connection that sent it is closed");
    th.app.collab().take_updates(&app, &t.card_id);
    two.barrier(&th).await;
    two.settle().await;
    two.joined(TEST_WORKSPACE_ID, &t.card_id).await;
    two.write_acked("still written").await;
    th.teardown().await;
}
