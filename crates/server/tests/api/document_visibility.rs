//! Document grants are checked at registration and dispatch, not only on a timer.

use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

use crate::common::{Setup, TEST_WORKSPACE_ID};
use crate::rooms::{Incoming, UPDATE, frame, give, paragraphs, table, two_in_a_room};

const WAIT: Duration = Duration::from_secs(5);

#[tokio::test]
async fn board_revocation_between_document_check_and_registration_discloses_no_body() {
    let ts = Setup::native().start().await.init_basic().await;
    let room = table(&ts, &paragraphs(&["private"])).await;
    let user = ts.client2.me().await.id;
    give(&ts, &room.board_id, &user, "editor").await;
    let mut socket = ts.client2.connect_websocket().await;
    let (reached, resume) = ts.app.collab().pause_next_admission();
    socket
        .send(Message::Text(
            json!({"type": "joinDoc", "workspaceId": TEST_WORKSPACE_ID,
        "cardId": room.card_id})
            .to_string()
            .into(),
        ))
        .await
        .expect("join sent");
    tokio::time::timeout(WAIT, reached.notified())
        .await
        .expect("grant checked");
    ts.client
        .delete(&format!(
            "/w/{TEST_WORKSPACE_ID}/containers/{}/members/{user}",
            room.board_id
        ))
        .await
        .check_no_content();
    resume.notify_one();
    let message = tokio::time::timeout(WAIT, socket.next())
        .await
        .expect("admission resolves")
        .expect("a refusal")
        .expect("a protocol frame");
    let Message::Text(text) = message else {
        panic!("expected refusal, got {message:?}");
    };
    let refused: Value = serde_json::from_str(&text).expect("JSON");
    assert_eq!(
        refused["type"], "docRefused",
        "no stale grant may expose the document"
    );
    assert_eq!(
        refused["reason"], "not_found",
        "removed member sees no card"
    );
    ts.teardown().await;
}

#[tokio::test]
async fn a_removed_board_member_cannot_write_or_receive_a_document_update() {
    let ts = Setup::native().start().await.init_basic().await;
    let (room, mut one, mut two) = two_in_a_room(&ts, &paragraphs(&["private"])).await;
    let user = ts.client2.me().await.id;
    let late = two.editor().append_paragraph("not authorized");
    ts.client
        .delete(&format!(
            "/w/{TEST_WORKSPACE_ID}/containers/{}/members/{user}",
            room.board_id
        ))
        .await
        .check_no_content();
    two.try_send_binary(frame(two.doc, two.epoch, UPDATE, &late))
        .await;
    one.write_acked("authorized update").await;
    let message = two.incoming(WAIT).await;
    let Incoming::Text(left) = message else {
        panic!("expected eviction, got {message:?}");
    };
    assert_eq!(
        left["type"], "docLeft",
        "revoked writer is evicted before another update"
    );
    assert_eq!(left["reason"], "not_found", "removed member sees no card");
    one.settle().await;
    assert_eq!(
        one.lines(),
        ["private", "authorized update"],
        "no revoked write is applied"
    );
    ts.teardown().await;
}

/// A writer made a viewer is told before the next update is taken, and cannot write.
#[tokio::test]
async fn a_downgraded_writer_is_told_and_cannot_write() {
    let ts = Setup::native().start().await.init_basic().await;
    let (room, mut one, mut two) = two_in_a_room(&ts, &paragraphs(&["initial"])).await;
    one.write_acked("warm").await;
    assert!(two.step(WAIT).await, "warm update arrives");
    let user = ts.client2.me().await.id;
    give(&ts, &room.board_id, &user, "viewer").await;
    two.write("not authorized").await;
    let mode = two.text("docModeChanged").await;
    assert_eq!(
        mode["mode"], "read",
        "downgrade is observed before taking the update"
    );
    let nack = two.text("docNack").await;
    assert_eq!(
        nack["reason"], "document_read_only",
        "downgraded writer cannot write"
    );
    one.settle().await;
    assert!(
        !one.lines().iter().any(|line| line == "not authorized"),
        "denied edit is never stored"
    );
    ts.teardown().await;
}

#[tokio::test]
async fn a_writer_downgraded_during_document_admission_enters_as_a_reader() {
    let ts = Setup::native().start().await.init_basic().await;
    let room = table(&ts, &paragraphs(&["private"])).await;
    let user = ts.client2.me().await.id;
    give(&ts, &room.board_id, &user, "editor").await;
    let mut socket = ts.client2.connect_websocket().await;
    let (reached, resume) = ts.app.collab().pause_next_admission();
    socket
        .send(Message::Text(
            json!({"type": "joinDoc", "workspaceId": TEST_WORKSPACE_ID,
        "cardId": room.card_id})
            .to_string()
            .into(),
        ))
        .await
        .expect("join sent");
    tokio::time::timeout(WAIT, reached.notified())
        .await
        .expect("grant checked");
    give(&ts, &room.board_id, &user, "viewer").await;
    resume.notify_one();
    let message = tokio::time::timeout(WAIT, socket.next())
        .await
        .expect("admission resolves")
        .expect("admitted as a reader")
        .expect("a protocol frame");
    let Message::Text(text) = message else {
        panic!("expected admission, got {message:?}");
    };
    let joined: Value = serde_json::from_str(&text).expect("JSON");
    assert_eq!(joined["type"], "docJoined", "still readable");
    assert_eq!(joined["mode"], "read", "old write grant is not reused");
    ts.teardown().await;
}

#[tokio::test]
async fn warm_document_grants_end_when_the_card_is_deleted() {
    for write in [false, true] {
        let ts = Setup::native().start().await.init_basic().await;
        let (room, mut one, mut two) = two_in_a_room(&ts, &paragraphs(&["private"])).await;
        one.write_acked("warm").await;
        assert!(two.step(WAIT).await, "cache warmed");
        crate::blocks::trash_card(&ts.client, &room.board_id, &room.card_id)
            .await
            .check_no_content();
        if write {
            two.write("after deletion").await;
        } else {
            let vector = two.editor().state_vector();
            two.send_binary(frame(two.doc, two.epoch, crate::rooms::STEP_1, &vector))
                .await;
        }
        let message = two.incoming(WAIT).await;
        let Incoming::Text(left) = message else {
            panic!("deleted card must evict, got {message:?}");
        };
        assert_eq!(
            left["type"], "docLeft",
            "a deleted card has no usable cached grant"
        );
        assert_eq!(left["reason"], "not_found", "deleted card is not visible");
        ts.teardown().await;
    }
}

#[tokio::test]
async fn missing_state_and_replaced_bodies_do_not_reach_a_removed_member() {
    for replace in [false, true] {
        let ts = Setup::native().start().await.init_basic().await;
        let (room, mut one, mut two) = two_in_a_room(&ts, &paragraphs(&["private"])).await;
        one.write_acked("warm").await;
        assert!(two.step(WAIT).await, "cache warmed");
        let user = ts.client2.me().await.id;
        ts.client
            .delete(&format!(
                "/w/{TEST_WORKSPACE_ID}/containers/{}/members/{user}",
                room.board_id
            ))
            .await
            .check_no_content();
        if replace {
            crate::rooms::write_whole(&ts.client, &room.card_id, &paragraphs(&["new secret"]))
                .await;
        } else {
            let vector = two.editor().state_vector();
            two.send_binary(frame(two.doc, two.epoch, crate::rooms::STEP_1, &vector))
                .await;
        }
        let message = two.incoming(WAIT).await;
        let Incoming::Text(left) = message else {
            panic!("removed reader must be evicted, got {message:?}");
        };
        assert_eq!(
            left["type"], "docLeft",
            "all payload origins check the grant"
        );
        ts.teardown().await;
    }
}
