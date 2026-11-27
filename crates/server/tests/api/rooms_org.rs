//! The rooms beside what an organization's administrators do to its workspaces: a
//! workspace put in the trash or removed for good, and an administrator who leaves a
//! workspace they joined, empty the rooms of its cards of the people they concern, at
//! once the workspace operation completes.

use fvoci_model::{Card, ContainerMember, ContainerVisibility, WorkspaceRole};
use serde_json::json;

use crate::boards::new_board;
use crate::common::{TEST_WORKSPACE_ID, TestServer};
use crate::org_admin::{let_in, make_workspace, org_of};
use crate::rooms::{Peer, give_in, no_rooms, paragraphs, table};
use crate::rooms_changes::seldom;

/// A card with a body on a private board of the workspace, made by the first user.
async fn card_in(th: &TestServer, workspace_id: &str) -> (String, String) {
    let board = th
        .create_board(workspace_id, ContainerVisibility::Private)
        .await;
    let card: Card = th
        .client
        .post(
            &format!("/w/{workspace_id}/boards/{}/cards", board.id),
            &json!({"title": "Theirs", "body": paragraphs(&["first"])}),
        )
        .await
        .created_json();
    (board.id, card.id)
}

/// A workspace put in the trash is not there for its members: who is in the room of
/// one of its cards is put out of the room before the operation answers, and keeps
/// its connection.
/// Brought back, the card is as it was, with what was typed. Put in the trash again,
/// its room empties again before the operation answers, and removing it for good
/// later finds no room of it, while the server goes on with the rooms of other
/// workspaces.
#[tokio::test]
async fn a_workspace_in_the_trash_or_removed_for_good_empties_the_rooms_of_its_cards() {
    let th = seldom().start().await.init_basic().await;
    let org_id = org_of(&th).await;
    let member = th.client2.me().await.id;
    let workspace = make_workspace(&th, &org_id, "Ops").await;
    let place = format!("/org/{org_id}/workspaces/{workspace}");
    th.client
        .post_empty(&format!("{place}/join"))
        .await
        .check_ok();
    let_in(&th, &workspace, &member, WorkspaceRole::Member).await;
    let (board_id, card_id) = card_in(&th, &workspace).await;
    give_in(&th, &workspace, &board_id, &member, "editor").await;
    let mut one = Peer::connect(&th.client, 101).await;
    let mut two = Peer::connect(&th.client2, 102).await;
    one.joined(&workspace, &card_id).await;
    two.joined(&workspace, &card_id).await;
    one.write_acked("typed").await;
    two.settle().await;

    let collab = th.app.collab();
    assert_eq!(collab.room_members(&th.app, &card_id).await, 2);
    th.client.delete(&place).await.check_no_content();
    // Out of the room as soon as the trashing has answered.
    assert_eq!(collab.room_members(&th.app, &card_id).await, 0);
    for peer in [&mut one, &mut two] {
        let left = peer.text("docLeft").await;
        assert_eq!(
            left,
            json!({"type": "docLeft", "doc": peer.doc, "reason": "not_found"}),
            "workspace trash puts out of the room"
        );
    }
    for peer in [&mut one, &mut two] {
        let refused = peer.join(&workspace, &card_id).await;
        assert_eq!(refused["reason"], json!("not_found"), "{refused}");
    }
    no_rooms(&th).await;

    th.client
        .post_empty(&format!("{place}/restore"))
        .await
        .check_ok();
    one.joined(&workspace, &card_id).await;
    two.joined(&workspace, &card_id).await;
    assert_eq!(one.lines(), ["first", "typed"], "as it was");

    // In the trash again: its room empties as the operation answers, and then for
    // good, which finds the room gone already.
    assert_eq!(collab.room_members(&th.app, &card_id).await, 2);
    th.client.delete(&place).await.check_no_content();
    assert_eq!(collab.room_members(&th.app, &card_id).await, 0);
    for peer in [&mut one, &mut two] {
        let left = peer.text("docLeft").await;
        assert_eq!(
            left,
            json!({"type": "docLeft", "doc": peer.doc, "reason": "not_found"}),
            "the trash again puts out of the room"
        );
    }
    th.app
        .store()
        .execute_unchecked(&format!(
            "UPDATE workspaces SET deleted_at = 1 WHERE id = '{workspace}'"
        ))
        .await
        .expect("the deletion is made an old one");
    fvoci_server::keep_house(&th.app).await;
    for peer in [&mut one, &mut two] {
        let refused = peer.join(&workspace, &card_id).await;
        assert_eq!(refused["reason"], json!("not_found"), "{refused}");
    }
    no_rooms(&th).await;

    // The rooms of another workspace are as they were.
    let t = table(&th, &paragraphs(&["elsewhere"])).await;
    one.joined(TEST_WORKSPACE_ID, &t.card_id).await;
    one.write_acked("goes on").await;
    assert_eq!(one.lines(), ["elsewhere", "goes on"]);
    th.teardown().await;
}

/// An administrator of the organization joins a workspace as its administrator, gives
/// themselves a role on one of its boards and writes a card's body there with a
/// member. When they leave the workspace again, the role goes with the joining: they
/// are put out of the room before the operation answers, keep their connection, and
/// the member writes on.
#[tokio::test]
async fn an_org_admin_who_leaves_a_workspace_they_joined_is_put_out_of_its_rooms() {
    let th = seldom().start().await.init_basic().await;
    let org_id = org_of(&th).await;
    let (admin, member) = (th.client.me().await.id, th.client2.me().await.id);
    let workspace = make_workspace(&th, &org_id, "Research").await;
    let place = format!("/org/{org_id}/workspaces/{workspace}");
    let_in(&th, &workspace, &member, WorkspaceRole::Member).await;
    let board: fvoci_model::Container = th
        .client2
        .create_board(&new_board(
            &workspace,
            ContainerVisibility::Private,
            "Theirs",
        ))
        .await
        .created_json();
    let card: Card = th
        .client2
        .post(
            &format!("/w/{workspace}/boards/{}/cards", board.id),
            &json!({"title": "Theirs", "body": paragraphs(&["first"])}),
        )
        .await
        .created_json();
    let mut joined = Peer::connect(&th.client, 101).await;
    let mut theirs = Peer::connect(&th.client2, 102).await;
    theirs.joined(&workspace, &card.id).await;

    // No member of the workspace: the card is not there for them.
    let refused = joined.join(&workspace, &card.id).await;
    assert_eq!(refused["reason"], json!("not_found"), "{refused}");
    th.client
        .post_empty(&format!("{place}/join"))
        .await
        .check_ok();
    // Its administrator now, who reads no board for the invited only without a role.
    let refused = joined.join(&workspace, &card.id).await;
    assert_eq!(refused["reason"], json!("not_found"), "{refused}");
    let as_editor = ContainerMember {
        container_id: board.id.clone(),
        user_id: admin,
        role: fvoci_model::ContainerRole::Editor,
        synthetic: false,
        inherited_role: None,
    };
    th.client
        .add_member_in(&workspace, &as_editor)
        .await
        .check_ok();
    let entered = joined.joined(&workspace, &card.id).await;
    assert_eq!(entered["mode"], json!("write"), "{entered}");
    joined.write_acked("by the administrator").await;
    theirs.settle().await;

    th.client
        .post_empty(&format!("{place}/leave"))
        .await
        .check_ok();
    // Out of the room as soon as the leaving has answered; the member stays.
    let collab = th.app.collab();
    assert_eq!(collab.room_members(&th.app, &card.id).await, 1);
    let left = joined.text("docLeft").await;
    assert_eq!(
        left,
        json!({"type": "docLeft", "doc": joined.doc, "reason": "not_found"}),
        "leaving puts out of the room"
    );
    let refused = joined.join(&workspace, &card.id).await;
    assert_eq!(refused["reason"], json!("not_found"), "{refused}");
    theirs.write_acked("by the member, after").await;
    assert_eq!(
        theirs.lines(),
        ["first", "by the administrator", "by the member, after"]
    );
    assert!(theirs.unread().is_empty(), "{:?}", theirs.unread());
    drop((joined, theirs));
    no_rooms(&th).await;
    th.teardown().await;
}
