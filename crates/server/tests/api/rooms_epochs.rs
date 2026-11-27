//! The rooms when a card's document is made again, or opened again after the worker
//! that held it ended: everyone in the room is sent the document anew.

use serde_json::json;

use crate::common::Setup;
use crate::rooms::{
    Incoming, Peer, QUIET, STEP_2, UPDATE, card_holds, frame, lines_of, paragraphs, two_in_a_room,
    write_whole,
};

/// A card's body written whole over HTTP makes its document again: both in the room
/// are sent the new one, of another epoch, and hold it in place of the one before.
/// An update made against the document before is refused and goes to nobody, whether
/// it was sent before its sender read of the new document or after; what is written
/// in the new one is taken as before.
#[tokio::test]
async fn a_body_written_whole_is_sent_to_the_room_as_another_document() {
    let th = Setup::native().start().await.init_basic().await;
    let (t, mut one, mut two) = two_in_a_room(&th, &paragraphs(&["first"])).await;
    one.write_acked("typed before").await;
    two.settle().await;

    // Made against the document before, and sent once the card is written.
    let late = two.editor().append_paragraph("late");
    write_whole(&th.client, &t.card_id, &paragraphs(&["written whole"])).await;
    two.send_binary(frame(two.doc, 1, UPDATE, &late)).await;
    let nack = two.text("docNack").await;
    assert_eq!(
        nack,
        json!({"type": "docNack", "doc": two.doc, "epoch": 1, "reason": "update_out_of_step"})
    );
    for peer in [&mut one, &mut two] {
        let reset = peer.text("docReset").await;
        assert_eq!(
            reset,
            json!({"type": "docReset", "doc": peer.doc, "epoch": 2})
        );
        peer.settle().await;
        assert_eq!(peer.epoch, 2);
        assert_eq!(
            peer.lines(),
            ["written whole"],
            "the document before is dropped"
        );
        assert!(peer.unread().is_empty(), "{:?}", peer.unread());
    }

    // After its sender holds the new document, too.
    let stale = one.editor().append_paragraph("of the epoch before");
    one.send_binary(frame(one.doc, 1, UPDATE, &stale)).await;
    let nack = one.text("docNack").await;
    assert_eq!(
        (&nack["epoch"], &nack["reason"]),
        (&json!(1), &json!("update_out_of_step")),
        "{nack}"
    );
    assert_eq!(
        two.incoming(QUIET).await,
        Incoming::Nothing,
        "sent to nobody"
    );

    // The room goes on in the new document. (`one` holds a paragraph the server
    // refused: it comes in anew, as an editor does after a docNack.)
    one.joined(crate::common::TEST_WORKSPACE_ID, &t.card_id)
        .await;
    one.write_acked("one goes on").await;
    two.settle().await;
    two.write_acked("two goes on").await;
    one.settle().await;
    assert_eq!(one.lines(), ["written whole", "one goes on", "two goes on"]);
    assert_eq!(one.body(), two.body());
    let body = one.body().expect("a body");
    one.leave().await;
    two.leave().await;
    card_holds(&th.client, &t.card_id, &body).await;
    th.teardown().await;
}

/// The worker that holds the documents is killed: the next update finds it gone, the
/// document is opened again from what is stored, in another worker, and takes the
/// update; both in the room are sent the document again, of the same epoch, which
/// has all that was taken, and go on in it. The room of another card, whose document
/// nobody asked for since, is sent nothing.
#[cfg(unix)]
#[tokio::test]
async fn a_killed_worker_has_the_room_sent_the_document_again() {
    let th = Setup::native().start().await.init_basic().await;
    let (t, mut one, mut two) = two_in_a_room(&th, &paragraphs(&["first"])).await;
    one.write_acked("before").await;
    two.settle().await;
    let other = crate::rooms::table(&th, &paragraphs(&["another card"])).await;
    let mut elsewhere = Peer::connect(&th.client, 103).await;
    elsewhere
        .joined(crate::common::TEST_WORKSPACE_ID, &other.card_id)
        .await;
    let collab = th.app.collab();
    let killed = collab.worker_pids().await[0].expect("a worker runs");
    crate::collab_deaths::kill(killed);
    while collab.worker_pids().await[0].is_some() {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    assert_eq!(one.write_acked("after").await, 2, "taken by the new worker");
    for peer in [&mut one, &mut two] {
        let reset = peer.text("docReset").await;
        assert_eq!(
            reset,
            json!({"type": "docReset", "doc": peer.doc, "epoch": 1})
        );
        peer.settle().await;
        assert_eq!(peer.lines(), ["first", "before", "after"]);
        // What followed the `docReset` is the whole document, by itself.
        let whole = peer.taken.last().expect("a frame");
        assert_eq!(whole.sync, STEP_2, "{:?}", whole.sync);
        let alone = fvoci_collab::testing::Editor::join(104, &whole.payload);
        let body = alone.body().map(|body| body.document().clone());
        assert_eq!(lines_of(body.as_ref()), ["first", "before", "after"]);
    }
    assert_eq!(
        elsewhere.incoming(QUIET).await,
        Incoming::Nothing,
        "the room of another card hears nothing of this one"
    );
    assert_ne!(
        collab.worker_pids().await[0],
        Some(killed),
        "another worker"
    );
    two.write_acked("and on").await;
    one.settle().await;
    assert_eq!(one.lines(), ["first", "before", "after", "and on"]);
    assert_eq!(one.body(), two.body());
    let body = one.body().expect("a body");
    one.leave().await;
    two.leave().await;
    card_holds(&th.client, &t.card_id, &body).await;
    th.teardown().await;
}

/// A document that grew near its limit is made again from its body by the update that
/// brought it there: its sender is told the update is logged, and then both in the
/// room are sent the new document, which holds what the one before held.
#[tokio::test]
async fn a_document_made_again_for_its_size_is_sent_to_the_room() {
    let th = Setup::native()
        .configure(|config| {
            // Large updates, so that few of them bring the document near its limit:
            // every update is paid for by the size of the document.
            config.collab.collab_update_bytes = 1024 * 1024;
            // The test writes faster than an editor does.
            config.collab.collab_frames_per_second = 1_000_000;
        })
        .start()
        .await
        .init_basic()
        .await;
    let (t, mut one, mut two) = two_in_a_room(&th, &paragraphs(&["kept"])).await;
    // Each round leaves 480 KiB deleted in the state: three quarters of the 16 MiB a
    // state may be is reached around the 25th round (50 updates).
    let line = "z".repeat(480 * 1024);
    let mut updates = 0;
    while one.epoch == 1 {
        assert!(
            updates < 100,
            "made again before the state reached its limit"
        );
        let update = if updates % 2 == 0 {
            one.editor().append_paragraph(&line)
        } else {
            one.editor().delete_last()
        };
        updates += 1;
        one.send_update(&update).await;
        // Taken; or, sent before the `docReset` after the one before was read,
        // refused as of the document before.
        let answer = one.one_of(&["docAck", "docNack"]).await;
        if answer["type"] == json!("docNack") {
            assert_eq!(answer["reason"], json!("update_out_of_step"), "{answer}");
            break;
        }
    }
    assert!(
        updates >= 48,
        "only once it was near the limit: {updates} updates"
    );
    for peer in [&mut one, &mut two] {
        let reset = peer.text("docReset").await;
        assert_eq!(reset["epoch"], json!(2), "{reset}");
        peer.settle().await;
        assert!(peer.unread().is_empty(), "{:?}", peer.unread());
    }
    assert_eq!(one.body(), two.body());
    assert_eq!(one.lines()[0], "kept");
    assert!(one.lines().len() <= 2, "{:?}", one.lines().len());
    one.write_acked("in the new document").await;
    two.settle().await;
    assert_eq!(one.body(), two.body());
    let body = one.body().expect("a body");
    one.leave().await;
    two.leave().await;
    card_holds(&th.client, &t.card_id, &body).await;
    th.teardown().await;
}
