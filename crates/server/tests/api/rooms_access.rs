//! Who is in a room is asked again while they are there: a role that was lowered
//! reads from then on, and who may no longer read is put out.

use serde_json::json;

use crate::common::{Setup, TEST_WORKSPACE_ID, TestServer};
use crate::rooms::{
    Incoming, PATIENCE, QUIET, UPDATE, frame, give, no_rooms, paragraphs, two_in_a_room,
};
use crate::rooms_changes::seldom;

/// Has every room ask now whether who is in it may be there, as at the end of its
/// interval: the servers of these tests ask once an hour otherwise ([`seldom`]), and
/// each read may take as long, however slow the machine.
fn ask_now(th: &TestServer) {
    th.app.collab().ask_again_now();
}

/// An editor in a room is made a viewer: the change's own check tells it that it
/// reads, and what it writes is refused; made an editor again, it writes. Taken off
/// the board, it is put out, as for a card that is not there, and the room goes on
/// without it. Who stays is told nothing.
#[tokio::test]
async fn a_lowered_role_reads_and_who_is_taken_off_the_board_is_put_out() {
    let th = seldom().start().await.init_basic().await;
    let (t, mut one, mut two) = two_in_a_room(&th, &paragraphs(&["first"])).await;
    let user2 = th.client2.me().await.id;
    two.write_acked("as an editor").await;

    give(&th, &t.board_id, &user2, "viewer").await;
    let mode = two.text("docModeChanged").await;
    assert_eq!(
        mode,
        json!({"type": "docModeChanged", "doc": two.doc, "mode": "read"})
    );
    two.write("as a viewer").await;
    let nack = two.text("docNack").await;
    assert_eq!(nack["reason"], json!("document_read_only"), "{nack}");
    one.settle().await;
    assert_eq!(one.lines(), ["first", "as an editor"]);
    // It still reads.
    one.write_acked("to a viewer").await;
    two.settle().await;
    assert!(two.lines().contains(&"to a viewer".to_owned()));

    // An editor again: it comes in anew to drop what was refused, and writes.
    give(&th, &t.board_id, &user2, "editor").await;
    let mode = two.text("docModeChanged").await;
    assert_eq!(mode["mode"], json!("write"), "{mode}");
    two.joined(TEST_WORKSPACE_ID, &t.card_id).await;
    two.write_acked("as an editor again").await;
    one.settle().await;
    assert_eq!(one.body(), two.body());

    // Off the board: out of the room, and what it sends reaches nobody.
    th.client
        .delete(&format!(
            "/w/{TEST_WORKSPACE_ID}/containers/{}/members/{user2}",
            t.board_id
        ))
        .await
        .check_no_content();
    let left = two.text("docLeft").await;
    assert_eq!(
        left,
        json!({"type": "docLeft", "doc": two.doc, "reason": "not_found"})
    );
    let late = two.editor().append_paragraph("after it was put out");
    two.send_binary(frame(two.doc, two.epoch, UPDATE, &late))
        .await;
    assert_eq!(two.incoming(QUIET).await, Incoming::Nothing, "not answered");
    let refused = two.join(TEST_WORKSPACE_ID, &t.card_id).await;
    assert_eq!(refused["reason"], json!("not_found"), "{refused}");
    one.write_acked("without it").await;
    assert_eq!(
        one.lines(),
        [
            "first",
            "as an editor",
            "to a viewer",
            "as an editor again",
            "without it"
        ]
    );
    assert!(one.unread().is_empty(), "{:?}", one.unread());
    assert_eq!(th.app.collab().rooms(), 1);
    th.teardown().await;
}

/// Who leaves the card's workspace in the store alone, with a role left on the board, is
/// put out at the next asking too: no immediate check is made for it.
#[tokio::test]
async fn who_is_taken_out_of_the_workspace_is_put_out_of_the_room() {
    let th = seldom().start().await.init_basic().await;
    let (_t, mut one, mut two) = two_in_a_room(&th, &paragraphs(&["first"])).await;
    let user2 = th.client2.me().await.id;
    crate::workspace_removal::remove_from_the_test_workspace(&th, &user2).await;
    ask_now(&th);
    let left = two.text("docLeft").await;
    assert_eq!(left["reason"], json!("not_found"), "{left}");
    one.write_acked("goes on").await;
    assert!(one.unread().is_empty(), "{:?}", one.unread());
    th.teardown().await;
}

/// A card that is deleted while its room is open: who is in the room is put out, as for
/// a card that is not there.
#[tokio::test]
async fn the_room_of_a_deleted_card_is_emptied() {
    let th = seldom().start().await.init_basic().await;
    let (t, mut one, mut two) = two_in_a_room(&th, &paragraphs(&["first"])).await;
    crate::blocks::trash_card(&th.client, &t.board_id, &t.card_id)
        .await
        .check_no_content();
    for peer in [&mut one, &mut two] {
        let left = peer.text("docLeft").await;
        assert_eq!(
            left,
            json!({"type": "docLeft", "doc": peer.doc, "reason": "not_found"})
        );
    }
    crate::rooms::no_rooms(&th).await;
    th.teardown().await;
}

/// Nobody has the room ask: at the end of its interval it asks by itself, whether the
/// store answers within that time or not. Both ends of the check count, so how long the
/// reads take does not matter.
#[tokio::test]
async fn a_room_asks_again_by_itself_at_the_end_of_its_interval() {
    let th = Setup::native()
        .configure(|config| config.collab.collab_recheck_seconds = 1)
        .start()
        .await
        .init_basic()
        .await;
    // Set before the room is: its first check is then the one paused.
    let collab = th.app.collab();
    let (answered, apply) = collab.pause_next_recheck();
    let (unanswered, once_more) = collab.pause_next_asking_once_more();
    let (_t, _one, _two) = two_in_a_room(&th, &paragraphs(&["first"])).await;
    tokio::time::timeout(PATIENCE, async {
        tokio::select! {
            () = answered.notified() => {}
            () = unanswered.notified() => {}
        }
    })
    .await
    .expect("the room asked again by itself");
    collab.unpause();
    apply.notify_one();
    once_more.notify_one();
    th.teardown().await;
}

/// The store cannot be asked about who is in a room: asked once more two seconds
/// later, it answers, and nobody is put out; when it does not answer then either,
/// everybody is, with the reason it gave. The memberships of the boards are out of the
/// store's reach meanwhile: their table has another name. The room asks when the test
/// has it ask, and is held where the table is to come back.
#[tokio::test]
async fn a_store_that_fails_for_a_moment_empties_no_room_and_one_that_stays_failed_does() {
    let th = seldom().start().await.init_basic().await;
    let (t, mut one, mut two) = two_in_a_room(&th, &paragraphs(&["first"])).await;
    let collab = th.app.collab();
    let rename = async |from: &str, to: &str| {
        th.app
            .store()
            .execute_unchecked(&format!("ALTER TABLE {from} RENAME TO {to}"))
            .await
            .expect("the table of the memberships is renamed");
    };
    let reached = async |at: &tokio::sync::Notify, step: &str| {
        tokio::time::timeout(PATIENCE, at.notified())
            .await
            .unwrap_or_else(|_| panic!("the periodic check {step}"));
    };
    // Away when the room asks, and back before it asks once more.
    let (unanswered, once_more) = collab.pause_next_asking_once_more();
    rename("container_members", "container_members_away").await;
    collab.ask_again_now();
    reached(&unanswered, "is not answered").await;
    rename("container_members_away", "container_members").await;
    let (answered, apply) = collab.pause_next_recheck();
    once_more.notify_one();
    reached(&answered, "is answered the second time").await;
    // The next asking gets as far as its answers once those of this one took effect.
    let (next, go) = collab.pause_next_recheck();
    apply.notify_one();
    collab.ask_again_now();
    reached(&next, "asks again").await;
    go.notify_one();
    for peer in [&mut one, &mut two] {
        assert_eq!(peer.incoming(QUIET).await, Incoming::Nothing, "stays in");
    }
    one.write_acked("after the store came back").await;
    two.settle().await;
    assert_eq!(two.lines(), ["first", "after the store came back"]);

    rename("container_members", "container_members_away").await;
    collab.ask_again_now();
    for peer in [&mut one, &mut two] {
        let left = peer.text("docLeft").await;
        assert_eq!(
            left,
            json!({"type": "docLeft", "doc": peer.doc, "reason": "internal"})
        );
    }
    rename("container_members_away", "container_members").await;
    no_rooms(&th).await;
    one.joined(TEST_WORKSPACE_ID, &t.card_id).await;
    assert_eq!(one.lines(), ["first", "after the store came back"]);
    th.teardown().await;
}

/// Removing the direct role keeps a collaborator who reads through a team writing in
/// the room, and their leaving the team puts them out before it is answered: the
/// periodic check is too seldom to do either.
#[tokio::test]
async fn losing_the_team_role_puts_the_member_out_of_the_room() {
    use fvoci_model::{Team, WorkspaceRole};
    let th = seldom().start().await.init_basic().await;
    let (t, mut one, mut two) = two_in_a_room(&th, &paragraphs(&["first"])).await;
    let owner = th.client.me().await.id;
    let user = th.client2.me().await.id;
    th.app
        .store()
        .change_workspace_role(
            TEST_WORKSPACE_ID,
            &owner,
            (WorkspaceRole::Member, WorkspaceRole::Admin),
            &owner,
        )
        .await
        .unwrap();
    let team: Team = th
        .client
        .post(
            &format!("/w/{TEST_WORKSPACE_ID}/teams"),
            &json!({"name":"Collaborators"}),
        )
        .await
        .ok_json();
    let link = format!("/w/{TEST_WORKSPACE_ID}/teams/{}/members/{user}", team.id);
    th.client.put(&link, &json!({})).await.check_no_content();
    th.client
        .put(
            &format!(
                "/w/{TEST_WORKSPACE_ID}/containers/{}/teams/{}",
                t.board_id, team.id
            ),
            &json!({"role":"editor"}),
        )
        .await
        .check_no_content();
    th.client
        .delete(&format!(
            "/w/{TEST_WORKSPACE_ID}/containers/{}/members/{user}",
            t.board_id
        ))
        .await
        .check_no_content();
    two.write_acked("through a team").await;
    th.client.delete(&link).await.check_no_content();
    // Out of the room as soon as the leaving has answered.
    let collab = th.app.collab();
    assert_eq!(collab.room_members(&th.app, &t.card_id).await, 1);
    let left = two.text("docLeft").await;
    assert_eq!(
        left,
        json!({"type": "docLeft", "doc": two.doc, "reason": "not_found"})
    );
    let refused = two.join(TEST_WORKSPACE_ID, &t.card_id).await;
    assert_eq!(refused["reason"], json!("not_found"), "{refused}");
    one.write_acked("after the team member left").await;
    th.teardown().await;
}
