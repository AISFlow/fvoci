//! A connection that comes back to a room with the document it held: it says which
//! making of the document and what of it it holds, is sent what it lacks and the
//! server's state vector, and sends what the server lacks. Nothing either side wrote
//! meanwhile is lost or taken twice, a deletion made unseen among it. A document of
//! another making, or a join that says nothing the server can read, is sent whole.
//!
//! Nothing here differs between the databases: the document is asked of its worker,
//! and its log is written as for any update.

use serde_json::json;

use crate::common::{Setup, TEST_WORKSPACE_ID, TestServer};
use crate::rooms::{
    Peer, STEP_1, STEP_2, UPDATE, card_holds, frame, paragraphs, two_in_a_room, write_whole,
};

/// The second client's peer after its connection ended: another connection, in no
/// room, with the editor and the epoch the one before held.
async fn away(th: &TestServer, mut peer: Peer) -> Peer {
    let (editor, epoch) = (peer.editor.take(), peer.epoch);
    drop(peer);
    let mut back = Peer::connect(&th.client2, 102).await;
    (back.editor, back.epoch) = (editor, epoch);
    back
}

/// The sync types of the frames a peer took since it had taken `before`.
fn kinds(peer: &Peer, before: usize) -> Vec<u64> {
    peer.taken[before..]
        .iter()
        .map(|frame| frame.sync)
        .collect()
}

/// Both wrote while one was away: the one who comes back is sent the server's state
/// vector and what it lacks, not the whole document, and sends what the server lacks
/// as one update, which is logged and sent on. Both hold the same, the deletion made
/// unseen too, and the card is given it.
#[tokio::test]
async fn who_comes_back_is_sent_what_it_lacks_and_sends_what_the_server_lacks() {
    let th = Setup::native().start().await.init_basic().await;
    let lines: Vec<String> = (0..40)
        .map(|line| format!("line {line} of a body"))
        .collect();
    let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
    let (t, mut one, mut two) = two_in_a_room(&th, &paragraphs(&lines)).await;
    let whole = two.taken[0].payload.len();
    two.write_acked("written by two before").await;
    one.settle().await;

    // Away: what two writes and deletes now reaches nobody.
    let mut two = away(&th, two).await;
    let epoch = two.epoch;
    let _unsent = two.editor().delete_last();
    let _unsent = two.editor().append_paragraph("written by two while away");
    one.write_acked("written by one meanwhile").await;

    // Back, saying what it holds.
    let before = two.taken.len();
    let joined = two.join_holding(TEST_WORKSPACE_ID, &t.card_id).await;
    assert_eq!(
        (&joined["epoch"], &joined["mode"]),
        (&json!(epoch), &json!("write"))
    );
    assert_eq!(
        kinds(&two, before),
        [STEP_1, STEP_2],
        "the server's state vector, then what the peer lacks"
    );
    let lacked = &two.taken[before + 1];
    assert!(
        lacked.payload.len() * 4 < whole,
        "a difference, not the whole: {} of {whole}",
        lacked.payload.len()
    );
    assert!(two.lines().contains(&"written by one meanwhile".to_owned()));

    // What the server lacks, as the answer to its state vector.
    let asked = two.asked.take().expect("the server's state vector");
    let missing = two.editor().missing_from(&asked);
    two.send_binary(frame(two.doc, two.epoch, STEP_2, &missing))
        .await;
    let ack = two.text("docAck").await;
    assert_eq!(ack["epoch"], json!(epoch), "{ack}");
    one.settle().await;
    assert_eq!(one.lines(), two.lines());
    let lines = one.lines();
    assert!(
        lines.contains(&"written by two while away".to_owned()),
        "{lines:?}"
    );
    assert!(
        lines.contains(&"written by one meanwhile".to_owned()),
        "{lines:?}"
    );
    assert!(
        !lines.contains(&"written by two before".to_owned()),
        "the deletion made while away is taken: {lines:?}"
    );

    // Sent again (its answer was lost with a connection), it changes nothing.
    two.send_binary(frame(two.doc, two.epoch, UPDATE, &missing))
        .await;
    two.text("docAck").await;
    one.settle().await;
    assert_eq!(one.lines(), lines);
    let body = one.body().expect("a body");
    one.leave().await;
    two.leave().await;
    card_holds(&th.client, &t.card_id, &body).await;
    th.teardown().await;
}

/// A document made again while the connection was away is another: saying it holds
/// the one before, the connection is sent the new one whole, with its epoch, and no
/// state vector to answer.
#[tokio::test]
async fn a_document_of_another_making_is_sent_whole() {
    let th = Setup::native().start().await.init_basic().await;
    let (t, mut one, two) = two_in_a_room(&th, &paragraphs(&["first"])).await;
    let mut two = away(&th, two).await;
    let _unsent = two.editor().append_paragraph("written while away");
    write_whole(&th.client, &t.card_id, &paragraphs(&["written whole"])).await;
    one.text("docReset").await;
    one.settle().await;

    let (epoch, before) = (two.epoch, two.taken.len());
    let joined = two.join_holding(TEST_WORKSPACE_ID, &t.card_id).await;
    assert_eq!(joined["epoch"], json!(epoch + 1), "{joined}");
    assert_eq!(
        kinds(&two, before),
        [STEP_2],
        "the whole, and nothing to answer"
    );
    assert_eq!(two.asked, None);
    assert_eq!(two.lines(), ["written whole"]);
    th.teardown().await;
}

/// A join that says it holds something the server cannot read as a state vector, or
/// names no state vector, or one that is no base64, is let in and sent the whole
/// document, as a connection that holds none.
#[tokio::test]
async fn a_join_that_says_nothing_readable_is_sent_the_whole() {
    let th = Setup::native().start().await.init_basic().await;
    let (t, mut one, mut two) = two_in_a_room(&th, &paragraphs(&["first", "second"])).await;
    let whole = two.taken[0].payload.clone();
    // Five clients, it says, and names none; no base64; nothing.
    for said in ["BQ==", "not base64!", ""] {
        let before = two.taken.len();
        let joined = two
            .join_saying(TEST_WORKSPACE_ID, &t.card_id, two.epoch, said)
            .await;
        assert_eq!(joined["type"], json!("docJoined"), "{said}: {joined}");
        assert_eq!(kinds(&two, before), [STEP_2], "{said}");
        assert_eq!(
            two.taken[before].payload, whole,
            "{said}: the whole document"
        );
        assert_eq!(two.asked, None, "{said}");
    }
    // The room goes on for both.
    two.write_acked("third").await;
    one.settle().await;
    assert_eq!(one.lines(), ["first", "second", "third"]);
    th.teardown().await;
}

/// Who comes back with nothing new on either side is sent the server's state vector
/// and a difference of nothing, and has nothing to send.
#[tokio::test]
async fn who_comes_back_to_what_it_holds_is_sent_no_document() {
    let th = Setup::native().start().await.init_basic().await;
    let lines: Vec<String> = (0..40)
        .map(|line| format!("line {line} of a body"))
        .collect();
    let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
    let (t, _one, mut two) = two_in_a_room(&th, &paragraphs(&lines)).await;
    let whole = two.taken[0].payload.len();
    let before = two.taken.len();
    two.join_holding(TEST_WORKSPACE_ID, &t.card_id).await;
    assert_eq!(kinds(&two, before), [STEP_1, STEP_2]);
    assert!(two.taken[before + 1].payload.len() * 10 < whole);
    let asked = two.asked.take().expect("the server's state vector");
    assert_eq!(two.editor().state_vector(), asked, "both hold the same");
    assert_eq!(two.lines().len(), 40);
    th.teardown().await;
}

/// What the other deleted while the connection was away is gone for who comes back:
/// the difference carries the deletion, which no state vector shows.
#[tokio::test]
async fn what_was_deleted_meanwhile_is_gone_for_who_comes_back() {
    let th = Setup::native().start().await.init_basic().await;
    let (t, mut one, two) = two_in_a_room(&th, &paragraphs(&["kept", "deleted meanwhile"])).await;
    let mut two = away(&th, two).await;
    let deleted = one.editor().delete_last();
    one.send_update(&deleted).await;
    one.text("docAck").await;
    assert_eq!(one.lines(), ["kept"]);

    let before = two.taken.len();
    two.join_holding(TEST_WORKSPACE_ID, &t.card_id).await;
    assert_eq!(kinds(&two, before), [STEP_1, STEP_2]);
    assert_eq!(two.lines(), ["kept"]);
    th.teardown().await;
}

/// Who is let in to read is sent the whole though they name the document they hold:
/// they have nothing to send, and what they hold that the server lacks (written
/// before they were made a reader) shows against the whole alone.
#[tokio::test]
async fn a_reader_who_comes_back_is_sent_the_whole() {
    let th = Setup::native().start().await.init_basic().await;
    let (t, mut one, two) = two_in_a_room(&th, &paragraphs(&["first"])).await;
    let user2 = th.client2.me().await.id;
    let mut two = away(&th, two).await;
    crate::rooms::give(&th, &t.board_id, &user2, "viewer").await;
    one.write_acked("written meanwhile").await;

    let before = two.taken.len();
    let joined = two.join_holding(TEST_WORKSPACE_ID, &t.card_id).await;
    assert_eq!(joined["mode"], json!("read"), "{joined}");
    assert_eq!(
        kinds(&two, before),
        [STEP_2],
        "the whole, and no state vector"
    );
    assert_eq!(two.asked, None);
    assert_eq!(two.lines(), ["first", "written meanwhile"]);
    th.teardown().await;
}

/// A join that names the document held counts among the frames a connection may send
/// as any join does: as five.
#[tokio::test]
async fn a_join_that_names_its_document_counts_as_any_join() {
    let th = Setup::native()
        .configure(|config| config.collab.collab_frames_per_second = 10)
        .start()
        .await
        .init_basic()
        .await;
    th.app.collab().pause_frame_rate_clock();
    let (t, _one, mut two) = two_in_a_room(&th, &paragraphs(&["first"])).await;
    // Its first join was five of the ten; this is the other five.
    two.join_holding(TEST_WORKSPACE_ID, &t.card_id).await;
    two.send_json(json!({
        "type": "joinDoc", "workspaceId": TEST_WORKSPACE_ID, "cardId": t.card_id,
        "epoch": two.epoch, "stateVector": "AA==",
    }))
    .await;
    let refused = two.text("docRefused").await;
    assert_eq!(refused["reason"], json!("too_many_frames"), "{refused}");
    th.teardown().await;
}
