//! What the room sends a member while an immediate permission check holds them: nothing
//! of the others' changes until the store answered of them. Put out, they were sent
//! none of what was written after the change; kept, they are sent all of it at once,
//! after `docModeChanged`.

use std::time::Duration;

use serde_json::{Value, json};

use crate::common::{Setup, TEST_WORKSPACE_ID, TestServer};
use crate::rooms::{
    Frame, Incoming, PATIENCE, Peer, STEP_1, STEP_2, UPDATE, frame, give, paragraphs,
    two_in_a_room, write_whole,
};

/// How long the held member reads for what should not come.
const READ_WHILE_HELD: Duration = Duration::from_millis(400);

/// A deadline for the immediate check far past what the tests take.
async fn server() -> TestServer {
    Setup::native()
        .configure(|config| config.collab.collab_recheck_seconds = 30)
        .start()
        .await
        .init_basic()
        .await
}

/// Runs `meanwhile` once the next immediate check holds its members and before it
/// reads the store, and lets it read after; the check is `change`'s, which answers
/// once it has. What `meanwhile` gave.
async fn while_held<T>(
    th: &TestServer,
    change: impl Future<Output = ()>,
    meanwhile: impl Future<Output = T>,
) -> T {
    let (reached, go) = th.app.collab().pause_next_access_read();
    let held = async {
        reached.notified().await;
        let given = meanwhile.await;
        go.notify_one();
        given
    };
    let ((), given) = tokio::join!(change, held);
    given
}

/// Reads what comes to a held member for a while: the texts of what they hold then.
async fn held_lines(member: &mut Peer) -> Vec<String> {
    while member.step(READ_WHILE_HELD).await {}
    member.lines()
}

/// The next message, which is to be the JSON message of `action`.
async fn next_text(peer: &mut Peer, action: &str) -> Value {
    match peer.incoming(PATIENCE).await {
        Incoming::Text(message) if message["type"] == json!(action) => message,
        other => panic!("{action} was to come next: {other:?}"),
    }
}

/// The frames a peer took since it had taken `before`.
fn taken_since(peer: &Peer, before: usize) -> &[Frame] {
    &peer.taken[before..]
}

/// The member is taken off the board, and the owner writes after that is stored and
/// before the store is read for the member. The member is put out, and was sent
/// nothing of what was written after they could no longer read the card.
#[tokio::test]
async fn a_member_taken_off_the_board_is_sent_nothing_written_after() {
    let th = server().await;
    let (table, mut owner, mut member) = two_in_a_room(&th, &paragraphs(&["first"])).await;
    let user = th.client2.me().await.id;
    let remove = async {
        th.client
            .delete(&format!(
                "/w/{TEST_WORKSPACE_ID}/containers/{}/members/{user}",
                table.board_id
            ))
            .await
            .check_no_content();
    };
    let held = while_held(&th, remove, async {
        owner.write_acked("after the removal").await;
        held_lines(&mut member).await
    })
    .await;
    assert_eq!(
        held,
        ["first"],
        "the held member was sent the owner's update"
    );
    assert_eq!(
        next_text(&mut member, "docLeft").await["reason"],
        json!("not_found")
    );
    member.settle().await;
    assert_eq!(member.lines(), ["first"]);
    th.teardown().await;
}

/// The member's role goes down to viewer; the owner writes twice while the member is
/// held. The member is sent none of it while held, and once kept (reading from then
/// on) `docModeChanged` and then all of it in one update.
#[tokio::test]
async fn a_member_kept_is_sent_at_once_what_was_held_from_them() {
    let th = server().await;
    let (table, mut owner, mut member) = two_in_a_room(&th, &paragraphs(&["first"])).await;
    let user = th.client2.me().await.id;
    let before = member.taken.len();
    let held = while_held(&th, give(&th, &table.board_id, &user, "viewer"), async {
        owner.write_acked("one").await;
        owner.write_acked("two").await;
        held_lines(&mut member).await
    })
    .await;
    assert_eq!(
        held,
        ["first"],
        "the held member was sent the owner's updates"
    );
    assert_eq!(
        next_text(&mut member, "docModeChanged").await["mode"],
        json!("read")
    );
    member.settle().await;
    assert_eq!(member.lines(), ["first", "one", "two"]);
    let syncs: Vec<u64> = taken_since(&member, before)
        .iter()
        .map(|frame| frame.sync)
        .collect();
    assert_eq!(syncs, [UPDATE], "what was held is sent in one update");
    // Kept: what the owner writes next comes as it always does.
    owner.write_acked("three").await;
    member.settle().await;
    assert_eq!(member.lines(), ["first", "one", "two", "three"]);
    th.teardown().await;
}

/// A second change of the same member while the first one's read waits: the second
/// check answers for the member and sends what was held from them under both checks,
/// once; the first one's answer, read before the second change, is dropped.
#[tokio::test]
async fn under_two_checks_what_was_held_is_sent_once() {
    let th = server().await;
    let (table, mut owner, mut member) = two_in_a_room(&th, &paragraphs(&["first"])).await;
    let user = th.client2.me().await.id;
    let before = member.taken.len();
    // Boxed: the second change makes the future large.
    let meanwhile = Box::pin(async {
        owner.write_acked("under the first").await;
        give(&th, &table.board_id, &user, "commenter").await;
        held_lines(&mut member).await
    });
    let held = while_held(&th, give(&th, &table.board_id, &user, "viewer"), meanwhile).await;
    assert_eq!(
        held,
        ["first", "under the first"],
        "not sent once the later check kept them"
    );
    assert_eq!(member.text("docModeChanged").await["mode"], json!("read"));
    owner.write_acked("after both").await;
    member.settle().await;
    assert_eq!(member.lines(), ["first", "under the first", "after both"]);
    assert_eq!(
        taken_since(&member, before).len(),
        2,
        "one update for what was held, one for what came after"
    );
    let answered_again = member.unread().into_iter().find(|message| {
        ["docModeChanged", "docLeft", "docReset"].contains(&message["type"].as_str().unwrap_or(""))
    });
    assert_eq!(answered_again, None, "the earlier answer took effect");
    th.teardown().await;
}

/// A member held across the document being made anew (the card's body written whole
/// over HTTP) is sent the new document once kept, after `docModeChanged`, and not before.
#[tokio::test]
async fn a_member_kept_after_the_document_was_made_anew_is_sent_it_then() {
    let th = server().await;
    let (table, mut owner, mut member) = two_in_a_room(&th, &paragraphs(&["first"])).await;
    let user = th.client2.me().await.id;
    let epoch_before = member.epoch;
    let (owner_lines, held, held_epoch) =
        while_held(&th, give(&th, &table.board_id, &user, "commenter"), async {
            write_whole(&th.client, &table.card_id, &paragraphs(&["replaced"])).await;
            owner.settle().await;
            let held = held_lines(&mut member).await;
            (owner.lines(), held, member.epoch)
        })
        .await;
    assert_eq!(owner_lines, ["replaced"]);
    assert_eq!(
        (held, held_epoch),
        (vec!["first".to_owned()], epoch_before),
        "the held member was sent the new document"
    );
    assert_eq!(
        next_text(&mut member, "docModeChanged").await["mode"],
        json!("read")
    );
    member.step(PATIENCE).await;
    assert!(member.epoch > epoch_before, "docReset comes next");
    member.settle().await;
    assert_eq!(member.lines(), ["replaced"]);
    th.teardown().await;
}

/// A held member asks what they are missing (a sync step 1): answered once the check
/// keeps them, after `docModeChanged`, and not before.
#[tokio::test]
async fn a_held_member_asking_what_they_miss_is_answered_once_kept() {
    let th = server().await;
    let (table, _owner, mut member) = two_in_a_room(&th, &paragraphs(&["first"])).await;
    let user = th.client2.me().await.id;
    let before = member.taken.len();
    let frames_while_held = while_held(&th, give(&th, &table.board_id, &user, "viewer"), async {
        member
            .send_binary(frame(member.doc, member.epoch, STEP_1, &[0]))
            .await;
        while member.step(READ_WHILE_HELD).await {}
        member.taken.len()
    })
    .await;
    assert_eq!(
        frames_while_held, before,
        "the held member's sync step 1 was answered while held"
    );
    assert_eq!(
        next_text(&mut member, "docModeChanged").await["mode"],
        json!("read")
    );
    member.settle().await;
    let answered: Vec<u64> = taken_since(&member, before)
        .iter()
        .map(|frame| frame.sync)
        .collect();
    assert_eq!(answered, [STEP_2]);
    assert_eq!(member.lines(), ["first"]);
    th.teardown().await;
}
