//! A permission change asks again only what it reaches: a connection coming into a
//! room is admitted again, and a periodic answer dropped, for a change of its room's
//! board and its user, not for one of another board.

use fvoci_model::{ContainerMember, ContainerVisibility};
use serde_json::json;

use crate::common::{TEST_WORKSPACE_ID, TestServer};
use crate::rooms::{Incoming, PATIENCE, Peer, QUIET, give, paragraphs, table, two_in_a_room};
use crate::rooms_changes::seldom;

/// Comes into the room of `card_id` while `change` is made between the admission and
/// the room: whether the connection was admitted once more for it. An admission again
/// gets to its pause before the room answers, so the two are told apart without a
/// timeout.
async fn admitted_again_for(
    th: &TestServer,
    peer: &mut Peer,
    card_id: &str,
    change: impl Future<Output = ()>,
) -> bool {
    let collab = th.app.collab();
    let (admitted, go) = collab.pause_next_admission();
    let joining = peer.join(TEST_WORKSPACE_ID, card_id);
    tokio::pin!(joining);
    tokio::select! {
        () = admitted.notified() => {}
        answer = &mut joining => panic!("answered before it was admitted: {answer}"),
    }
    change.await;
    let (again, go_again) = collab.pause_next_admission();
    go.notify_one();
    let readmitted = tokio::select! {
        biased;
        () = again.notified() => {
            go_again.notify_one();
            true
        }
        answer = &mut joining => {
            assert_eq!(answer["type"], json!("docJoined"), "{answer}");
            false
        }
    };
    if readmitted {
        let answer = joining.await;
        assert_eq!(answer["type"], json!("docJoined"), "{answer}");
    }
    collab.unpause();
    readmitted
}

/// Changes go on on another board of the organization while a member comes into a
/// room: the member is let in at once, never admitted again for them (three changes
/// would refuse them). A change of the room's board for that member does admit
/// them again.
#[tokio::test]
async fn a_change_of_another_board_does_not_admit_a_joining_member_again() {
    let th = seldom().start().await.init_basic().await;
    let t = table(&th, &paragraphs(&["first"])).await;
    let elsewhere = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let user = th.client2.me().await.id;
    give(&th, &t.board_id, &user, "editor").await;
    let mut peer = Peer::connect(&th.client2, 102).await;
    for role in ["viewer", "editor", "commenter"] {
        let again = admitted_again_for(&th, &mut peer, &t.card_id, async {
            give(&th, &elsewhere.id, &user, role).await;
        })
        .await;
        assert!(
            !again,
            "admitted again for a change of another board ({role})"
        );
    }
    let again = admitted_again_for(&th, &mut peer, &t.card_id, async {
        give(&th, &t.board_id, &user, "editor").await;
    })
    .await;
    assert!(
        again,
        "a change of the room's board for its member admits them again"
    );
    th.teardown().await;
}

/// A member made a reader where the store alone says so (no immediate check), and a
/// change of another board for the same member between the periodic check's reads
/// and its answers: the periodic answer is applied all the same. The room asks when the
/// test has it ask: an answer dropped would not be asked for again within the hour.
#[tokio::test]
async fn a_periodic_answer_is_applied_though_another_board_changed_meanwhile() {
    let th = seldom().start().await.init_basic().await;
    let (t, _owner, mut member) = two_in_a_room(&th, &paragraphs(&["first"])).await;
    let elsewhere = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let user = th.client2.me().await.id;
    let collab = th.app.collab();
    // A pass that read before the store changed: let go without reading again.
    let (read, go) = collab.pause_next_recheck();
    collab.ask_again_now();
    tokio::time::timeout(PATIENCE, read.notified())
        .await
        .expect("the periodic check reads");
    th.app
        .store()
        .save_member(&ContainerMember {
            container_id: t.board_id.clone(),
            user_id: user.clone(),
            role: fvoci_model::ContainerRole::Viewer,
            synthetic: false,
            inherited_role: None,
        })
        .await
        .expect("the member is made a reader in the store alone");
    go.notify_one();
    // The next pass reads the reader's role; another board changes before it answers.
    let (read, go) = collab.pause_next_recheck();
    collab.ask_again_now();
    tokio::time::timeout(PATIENCE, read.notified())
        .await
        .expect("the periodic check reads again");
    give(&th, &elsewhere.id, &user, "editor").await;
    // Not this room's change: nothing told of it but the periodic answer.
    assert_eq!(member.incoming(QUIET).await, Incoming::Nothing);
    go.notify_one();
    assert_eq!(member.text("docModeChanged").await["mode"], json!("read"));
    th.teardown().await;
}
