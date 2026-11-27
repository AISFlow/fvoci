//! A guest of a workspace: on the boards they were invited to, one by one, as no more
//! than an editor, without a public link, and told of the changes of those boards.

use fvoci_model::{ContainerMember, ContainerVisibility, WorkspaceRole};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::board_broadcasts::WorkspaceSocket;
use crate::boards::{editor, member, new_board, update_member};
use crate::common::TEST_WS;
use crate::common::{
    ApiResponse, HIDDEN_WORKSPACE_ID, Setup, TEST_WORKSPACE_ID, TestServer, UNKNOWN_ID,
};

/// The problem of an answer without what identifies the request.
fn answer(response: &ApiResponse) -> (StatusCode, Value) {
    let mut body = response.value();
    if let Some(object) = body.as_object_mut() {
        object.remove("instance");
        object.remove("requestId");
    }
    (response.status, body)
}

/// Makes the second user a guest of the test workspace and returns their identifier.
pub(crate) async fn guest_of_the_workspace(th: &TestServer) -> String {
    let user_id = th.client2.me().await.id;
    th.app
        .store()
        .execute_unchecked(&format!(
            "UPDATE workspace_members SET role = '{}' WHERE user_id = '{user_id}' \
             AND workspace_id = '{TEST_WORKSPACE_ID}'",
            WorkspaceRole::Guest.as_str()
        ))
        .await
        .expect("the second user becomes a guest of the workspace");
    user_id
}

/// A board of the test workspace that the first user makes and administers.
async fn board_of_user1(th: &TestServer, board_type: ContainerVisibility, title: &str) -> String {
    let board: fvoci_model::Container = th
        .client
        .create_board(&new_board(TEST_WORKSPACE_ID, board_type, title))
        .await
        .created_json();
    board.id
}

fn viewer(board_id: &str, user_id: &str) -> ContainerMember {
    member(board_id, user_id, fvoci_model::ContainerRole::Viewer)
}

#[tokio::test]
async fn a_guest_opens_only_the_boards_it_was_invited_to() {
    let th = Setup::native().start().await.init_basic().await;
    let guest = guest_of_the_workspace(&th).await;
    let invited = board_of_user1(&th, ContainerVisibility::Private, "Invited").await;
    let open = board_of_user1(&th, ContainerVisibility::Open, "Open").await;
    let private = board_of_user1(&th, ContainerVisibility::Private, "Not invited").await;
    let mut elsewhere = fvoci_model::Container {
        id: fvoci_model::new_id(),
        ..new_board(HIDDEN_WORKSPACE_ID, ContainerVisibility::Open, "Elsewhere")
    };
    th.app
        .store()
        .insert_container_with_admin(&mut elsewhere, &th.stranger_in(HIDDEN_WORKSPACE_ID).await)
        .await
        .expect("a board of another workspace");
    th.client
        .add_member(&viewer(&invited, &guest))
        .await
        .check_ok();

    // A board's own routes are under `/boards`, those of a board or a wiki under
    // `/containers`.
    for (under, path) in [
        ("containers", ""),
        ("boards", "/blocks"),
        ("boards", "/cards"),
        ("containers", "/members"),
    ] {
        th.client2
            .get(&format!("{TEST_WS}/{under}/{invited}{path}"))
            .await
            .check_ok();
    }
    let missing = th
        .client2
        .get(&format!("{TEST_WS}/containers/{UNKNOWN_ID}"))
        .await;
    for hidden in [&open, &private, &elsewhere.id] {
        let response = th
            .client2
            .get(&format!("{TEST_WS}/containers/{hidden}"))
            .await;
        assert_eq!(answer(&response), answer(&missing), "board {hidden}");
    }
    th.client2
        .post(&format!("{TEST_WS}/containers/{open}/join"), &json!({}))
        .await
        .check_not_found();
    let listed: Vec<fvoci_model::Container> =
        th.client2.get(&format!("{TEST_WS}/boards")).await.ok_json();
    th.teardown().await;
    assert_eq!(
        listed
            .iter()
            .map(|board| board.id.as_str())
            .collect::<Vec<_>>(),
        [invited.as_str()],
        "the guest's list of the workspace is its invitation"
    );
}

#[tokio::test]
async fn a_guest_is_given_no_more_than_editor_and_who_left_is_changed_no_more() {
    let th = Setup::native().start().await.init_basic().await;
    let guest = guest_of_the_workspace(&th).await;
    let board = board_of_user1(&th, ContainerVisibility::Private, "Guest's board").await;
    let as_admin = member(&board, &guest, fvoci_model::ContainerRole::Admin);

    let added_as_admin = th.client.add_member(&as_admin).await;
    let stored_before = th
        .app
        .store()
        .get_member_for_container(&board, &guest)
        .await;
    th.client
        .add_member(&editor(&board, &guest))
        .await
        .check_ok();
    let made_admin = update_member(&th.client, &as_admin).await;
    let stored = th
        .app
        .store()
        .get_member_for_container(&board, &guest)
        .await
        .expect("the guest is a member");
    // Who is no member of the board is not found, before what the request asks for.
    let elsewhere = board_of_user1(&th, ContainerVisibility::Private, "Not the guest's").await;
    let not_on_board = update_member(
        &th.client,
        &member(&elsewhere, &guest, fvoci_model::ContainerRole::Admin),
    )
    .await;
    // Who has left the workspace keeps a role that is changed no more.
    let leaver = th.add_user("leaver").await.me().await.id;
    th.client
        .add_member(&editor(&board, &leaver))
        .await
        .check_ok();
    th.app
        .store()
        .execute_unchecked(&format!(
            "DELETE FROM workspace_members WHERE user_id = '{leaver}' \
             AND workspace_id = '{TEST_WORKSPACE_ID}'"
        ))
        .await
        .expect("the leaver leaves the workspace");
    let raised = update_member(
        &th.client,
        &member(&board, &leaver, fvoci_model::ContainerRole::Admin),
    )
    .await;
    th.teardown().await;

    not_on_board.check_not_found();
    raised.check_not_found();

    for refused in [&added_as_admin, &made_admin] {
        assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.text());
        assert_eq!(refused.error_reason(), "guest_role_limit");
        assert_eq!(refused.problem_type(), "/problems/guest-role-limit");
    }
    assert!(
        stored_before.is_err(),
        "nothing was stored by the refused request"
    );
    assert_eq!(
        stored.role,
        fvoci_model::ContainerRole::Editor,
        "the guest stays an editor"
    );
}

#[tokio::test]
async fn a_guest_makes_no_public_link_and_manages_no_members_whatever_is_stored() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let guest = guest_of_the_workspace(&th).await;
    let board = board_of_user1(&th, ContainerVisibility::Private, "Guest's board").await;
    // A role a guest cannot be given, stored past the routes: as a member who was an
    // admin of the board before being made a guest would have it.
    th.app
        .store()
        .save_member(&member(&board, &guest, fvoci_model::ContainerRole::Admin))
        .await
        .expect("an admin role is stored");

    let shared = th
        .client2
        .post(
            &format!("{TEST_WS}/containers/{board}/public-link"),
            &json!({"enabled": true}),
        )
        .await;
    let user1 = th.client.me().await.id;
    let lowered = update_member(&th.client2, &viewer(&board, &user1)).await;
    let edited = th
        .client2
        .patch(
            &format!("{TEST_WS}/containers/{board}"),
            &json!({"title": "Changed by the guest"}),
        )
        .await;
    th.teardown().await;

    shared.check_forbidden();
    lowered.check_forbidden();
    edited.check_ok();
}

#[tokio::test]
async fn a_guest_is_told_the_changes_of_its_boards_and_of_no_other() {
    let th = Setup::native().start().await.init_basic().await;
    let guest = guest_of_the_workspace(&th).await;
    let invited = board_of_user1(&th, ContainerVisibility::Private, "Invited").await;
    let other = board_of_user1(&th, ContainerVisibility::Open, "Other").await;
    th.client
        .add_member(&viewer(&invited, &guest))
        .await
        .check_ok();
    let mut socket = WorkspaceSocket::subscribe(&th, &th.client2, TEST_WORKSPACE_ID, &guest).await;

    for board in [&other, &invited] {
        th.client
            .patch(
                &format!("{TEST_WS}/containers/{board}"),
                &json!({"title": "Renamed"}),
            )
            .await
            .check_ok();
    }
    let mut messages = Vec::new();
    for _ in 0..20 {
        messages.extend(socket.drain().await);
        if messages
            .iter()
            .any(|message| message["container"]["id"] == json!(invited))
        {
            break;
        }
    }
    th.teardown().await;

    let heard: Vec<&Value> = messages
        .iter()
        .filter(|message| message["type"] == json!("containerChanged"))
        .collect();
    assert!(
        heard
            .iter()
            .any(|message| message["container"]["id"] == json!(invited)
                && message["container"]["title"] == json!("Renamed")),
        "the change of the invited board is told: {messages:?}"
    );
    let about_other = messages
        .iter()
        .any(|message| message.to_string().contains(&other));
    assert!(!about_other, "nothing of the other board: {messages:?}");
}
