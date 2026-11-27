//! Independent checks of what a refusal reads, backpressure, and work in flight at
//! disconnect: from the review of the rooms by another reader (Codex).

use std::time::Duration;

use fvoci_collab::testing::Editor;
use fvoci_server::collab::Outgoing;
use serde_json::{Value, json};

use crate::common::{Setup, TEST_WORKSPACE_ID, UNKNOWN_ID};
use crate::rooms::{
    Incoming, PATIENCE, Peer, QUIET, UPDATE, card_holds, frame, no_rooms, paragraphs, table,
    two_in_a_room,
};

/// The refusal of a `joinDoc`, which must be the one of a card that is not there.
async fn refusal(peer: &mut Peer, card_id: &str) -> Value {
    let refused = peer.join(TEST_WORKSPACE_ID, card_id).await;
    assert_eq!(
        refused,
        json!({
            "type": "docRefused", "workspaceId": TEST_WORKSPACE_ID,
            "cardId": card_id, "reason": "not_found",
        }),
        "both identifiers must have the same refusal shape"
    );
    refused
}

/// A card on a private board must not turn a refused `joinDoc` into an existence
/// oracle by what its body costs to read: who is refused has nothing of the body read
/// for them. The review measured it as time (a body of 18,001 nodes made the refusal
/// of the hidden card fifteen times as long as that of a missing one on SQLite, five
/// times on PostgreSQL), which a busy machine blurs; here the column of the bodies is
/// taken away for the length of the refusals, so that any read of a body fails, and
/// the hidden card is refused as the missing one all the same. Who may read the card
/// is not let in meanwhile: its body is read for them.
#[tokio::test]
async fn a_hidden_cards_body_does_not_make_its_refusal_distinguishable_from_a_missing_card() {
    let th = Setup::native().start().await.init_basic().await;
    let t = table(&th, &paragraphs(&["private"])).await;
    let mut outside = Peer::connect(&th.client2, 301).await;
    let mut member = Peer::connect(&th.client, 302).await;
    let rename = async |from: &str, to: &str| {
        th.app
            .store()
            .execute_unchecked(&format!("ALTER TABLE blocks RENAME COLUMN {from} TO {to}"))
            .await
            .expect("the column of the bodies is renamed");
    };
    rename("body", "body_not_read").await;
    refusal(&mut outside, &t.card_id).await;
    refusal(&mut outside, UNKNOWN_ID).await;
    let unread = member.join(TEST_WORKSPACE_ID, &t.card_id).await;
    rename("body_not_read", "body").await;
    assert_eq!(
        (&unread["type"], &unread["reason"]),
        (&json!("docRefused"), &json!("internal")),
        "the body was out of reach of whatever read it: {unread}"
    );
    member.joined(TEST_WORKSPACE_ID, &t.card_id).await;
    drop((outside, member));
    no_rooms(&th).await;
    th.teardown().await;
}

/// Filling a sender's queue with its ACKs must not lose the answer to the next
/// accepted update. The room may put it out, after telling it what was committed.
#[tokio::test]
async fn an_accepted_update_keeps_its_ack_when_the_senders_queue_fills() {
    let th = Setup::native()
        .configure(|config| {
            config.collab.collab_queue_messages = 16;
            config.collab.collab_frames_per_second = 1000;
        })
        .start()
        .await
        .init_basic()
        .await;
    let t = table(&th, &paragraphs(&["first"])).await;
    let user = th.client.me().await.id;
    let app = th
        .app
        .enter_workspace_as(
            fvoci_server::app::Asker::with_password(&user),
            TEST_WORKSPACE_ID,
        )
        .await
        .expect("membership is read")
        .expect("a member")
        .app;
    let collab = th.app.collab();
    let mut healthy = Peer::connect(&th.client, 302).await;
    healthy.joined(TEST_WORKSPACE_ID, &t.card_id).await;
    // The same controllable outbox used by rooms_contract: after the initial sync,
    // this connection keeps sending updates but does not read its answers.
    let (mut slow, outbox) = collab.seat(
        &th.app,
        &user,
        fvoci_server::app::SessionKept::with_password(),
    );
    slow.command(
        &json!({"type": "joinDoc", "workspaceId": TEST_WORKSPACE_ID, "cardId": t.card_id})
            .to_string(),
    )
    .await;
    assert!(matches!(outbox.next().await, Some(Outgoing::Text(_))));
    assert!(matches!(outbox.next().await, Some(Outgoing::Binary(_))));
    let loaded = collab.open(&app, &t.card_id).await.expect("opened");
    let editor = Editor::join(303, &loaded.state);
    for n in 1..=17 {
        let update = editor.append_paragraph(&format!("accepted {n}"));
        slow.frame(frame(1, loaded.epoch, UPDATE, &update).into())
            .await;
        assert!(
            healthy.step(PATIENCE).await,
            "the other receives update {n}"
        );
    }
    let stored = app
        .store()
        .get_body_doc(&t.card_id)
        .await
        .expect("read")
        .expect("a document");
    assert_eq!(stored.log_end, 17, "all seventeen updates were committed");
    assert_eq!(healthy.lines().len(), 18, "all were broadcast too");
    let mut acks = Vec::new();
    loop {
        let outgoing = tokio::time::timeout(PATIENCE, outbox.next())
            .await
            .expect("the answers end in docLeft");
        let Some(Outgoing::Text(text)) = outgoing else {
            panic!("an ACK or docLeft: {outgoing:?}");
        };
        let message: Value = serde_json::from_str(&text).expect("JSON");
        match message["type"].as_str() {
            Some("docAck") => acks.push(message["seq"].as_i64().expect("a sequence")),
            Some("docLeft") => {
                assert_eq!(message["reason"], json!("connection_too_slow"));
                break;
            }
            _ => panic!("an ACK or docLeft: {message}"),
        }
    }
    slow.leave_all().await;
    drop(healthy);
    no_rooms(&th).await;
    th.teardown().await;
    assert_eq!(
        acks,
        (1..=17).collect::<Vec<i64>>(),
        "docLeft must follow the answers to every accepted update"
    );
}

/// Disconnecting the sender while its update awaits the log must leave the update
/// committed and broadcast, and remove the sender from its room afterwards.
#[tokio::test]
async fn disconnecting_during_a_log_write_finishes_the_update_and_leaves_the_room() {
    let th = Setup::native().start().await.init_basic().await;
    let (t, mut one, mut two) = two_in_a_room(&th, &paragraphs(&["first"])).await;
    let (held, go) = th.app.collab().pause_next_log();
    one.write("committed after disconnect").await;
    tokio::time::timeout(PATIENCE, held.notified())
        .await
        .expect("the update reaches the log barrier");
    drop(one);
    assert_eq!(two.incoming(QUIET).await, Incoming::Nothing);
    go.notify_one();
    assert!(
        two.step(PATIENCE).await,
        "the committed update is delivered"
    );
    assert_eq!(two.lines(), ["first", "committed after disconnect"]);
    let body = two.body().expect("a body");
    drop(two);
    no_rooms(&th).await;
    card_holds(&th.client, &t.card_id, &body).await;
    th.teardown().await;
}

/// Shutdown with live room members must close the room and workers and project the
/// acknowledged body before a new server opens the same data.
#[tokio::test]
async fn shutdown_with_live_room_members_keeps_the_accepted_body_and_releases_the_room() {
    let th = Setup::native().start().await.init_basic().await;
    let (t, mut one, mut two) = two_in_a_room(&th, &paragraphs(&["first"])).await;
    one.write_acked("before shutdown").await;
    two.settle().await;
    let body = one.body().expect("a body");
    let old_app = th.app.clone();
    let th = th.killed_and_started_again(async |_| {}).await;
    let deadline = tokio::time::Instant::now() + PATIENCE;
    while old_app.collab().rooms() > 0 {
        assert!(tokio::time::Instant::now() < deadline, "old rooms remain");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        old_app
            .collab()
            .worker_pids()
            .await
            .iter()
            .all(Option::is_none)
    );
    card_holds(&th.client, &t.card_id, &body).await;
    let mut again = Peer::connect(&th.client, 304).await;
    again.joined(TEST_WORKSPACE_ID, &t.card_id).await;
    assert_eq!(again.body(), Some(body));
    drop((one, two, again));
    no_rooms(&th).await;
    th.teardown().await;
}
