//! Who has left a workspace, and is still in the organization, finds none of its boards
//! through the routes that list or search boards; a workspace the user is no member of
//! exports nothing but a 404; and an import leaves out who is not in the workspace,
//! saying how many.

use fvoci_model::{Container, ContainerMember, ContainerVisibility};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::boards::new_board;
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

/// Ends a user's membership of the test workspace, and of nothing else.
pub(crate) async fn remove_from_the_test_workspace(th: &TestServer, user_id: &str) {
    th.app
        .store()
        .execute_unchecked(&format!(
            "DELETE FROM workspace_members WHERE user_id = '{user_id}' \
             AND workspace_id = '{TEST_WORKSPACE_ID}'"
        ))
        .await
        .expect("the membership of the test workspace ends");
}

#[tokio::test]
async fn who_left_a_workspace_finds_none_of_its_boards() {
    let th = Setup::native().start().await.init_basic().await;
    let user2 = th.client2.me().await.id;
    let board: Container = th
        .client2
        .create_board(&new_board(
            TEST_WORKSPACE_ID,
            ContainerVisibility::Private,
            "Quarterly secret",
        ))
        .await
        .created_json();
    remove_from_the_test_workspace(&th, &user2).await;

    let search_where_left = th
        .client2
        .get(&format!("{TEST_WS}/boards/search?q=Quarterly"))
        .await;
    let found_from_root: Vec<Container> = th
        .client2
        .get(&format!(
            "/w/{}/boards/search?q=Quarterly",
            crate::common::ROOT_WORKSPACE_ID
        ))
        .await
        .ok_json();
    let memberships: Vec<ContainerMember> = th.client2.get("/users/me/memberships").await.ok_json();
    let export = th.client2.get(&format!("{TEST_WS}/archive/export")).await;
    th.teardown().await;

    search_where_left.check_not_found();
    assert!(found_from_root.is_empty(), "{found_from_root:?}");
    assert!(
        memberships
            .iter()
            .all(|member| member.container_id != board.id),
        "{memberships:?}"
    );
    export.check_not_found();
}

#[tokio::test]
async fn a_workspace_the_user_is_no_member_of_exports_a_404() {
    let th = Setup::native().start().await.init_basic().await;
    let hidden = th
        .client
        .get(&format!("/w/{HIDDEN_WORKSPACE_ID}/archive/export"))
        .await;
    let missing = th
        .client
        .get(&format!("/w/{UNKNOWN_ID}/archive/export"))
        .await;
    let own = th.client.get(&format!("{TEST_WS}/archive/export")).await;
    th.teardown().await;

    hidden.check_not_found();
    assert_eq!(answer(&hidden), answer(&missing));
    own.check_ok();
}

#[tokio::test]
async fn an_admin_who_cannot_act_as_one_does_not_keep_a_board_from_its_last_admin() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await.id;
    let user2 = th.client2.me().await.id;
    let board: Container = th
        .client
        .create_board(&new_board(
            TEST_WORKSPACE_ID,
            ContainerVisibility::Private,
            "Ours",
        ))
        .await
        .created_json();
    // The second user is stored as an admin of the board, past the routes, and then
    // leaves the workspace: the role is left and opens nothing.
    th.app
        .store()
        .save_member(&ContainerMember {
            container_id: board.id.clone(),
            user_id: user2.clone(),
            role: fvoci_model::ContainerRole::Admin,
            synthetic: false,
            inherited_role: None,
        })
        .await
        .expect("a second admin is stored");
    remove_from_the_test_workspace(&th, &user2).await;

    let left = th
        .client
        .post(
            &format!("{TEST_WS}/containers/{}/leave", board.id),
            &json!({}),
        )
        .await;
    let stored = th
        .app
        .store()
        .get_member_for_container(&board.id, &user1)
        .await;
    th.teardown().await;

    assert_eq!(left.status, StatusCode::CONFLICT, "{}", left.text());
    assert_eq!(left.error_reason(), "last_admin");
    assert!(stored.is_ok(), "the last admin who can act stays");
}

#[tokio::test]
async fn an_import_leaves_out_who_is_not_in_the_workspace_and_says_how_many() {
    let th = Setup::native().start().await.init_basic().await;
    let user2 = th.client2.me().await.id;
    let board: Container = th
        .client
        .create_board(&new_board(
            TEST_WORKSPACE_ID,
            ContainerVisibility::Private,
            "Shared",
        ))
        .await
        .created_json();
    th.client
        .add_member(&crate::boards::member(
            &board.id,
            &user2,
            fvoci_model::ContainerRole::Editor,
        ))
        .await
        .check_ok();
    let archive = th
        .client
        .get(&format!("{TEST_WS}/containers/{}/archive/export", board.id))
        .await;
    archive.check_ok();

    let kept = crate::export::import(&th.client, TEST_WORKSPACE_ID, &archive.body).await;
    remove_from_the_test_workspace(&th, &user2).await;
    let left_out = crate::export::import(&th.client, TEST_WORKSPACE_ID, &archive.body).await;
    let shared: Vec<Container> = th.client.get(&format!("{TEST_WS}/boards")).await.ok_json();
    let mut held = 0;
    for listed in shared.iter().filter(|listed| listed.title == "Shared") {
        let members = th
            .app
            .store()
            .get_members_for_container(&listed.id)
            .await
            .expect("the members of a board");
        held += members
            .iter()
            .filter(|member| member.user_id == user2)
            .count();
    }
    th.teardown().await;

    assert_eq!(kept.value()["membersLeftOut"], json!(0));
    assert_eq!(left_out.value()["membersLeftOut"], json!(1));
    // The board, and the first import, have the second user; the second import not.
    assert_eq!(held, 2);
}

#[tokio::test]
async fn an_import_counts_each_user_once_and_no_membership_without_a_role() {
    let th = Setup::native().start().await.init_basic().await;
    let user2 = th.client2.me().await.id;
    let third = th.add_user("third").await.me().await.id;
    let mut boards = Vec::new();
    for title in ["One", "Two"] {
        let board: Container = th
            .client
            .create_board(&new_board(
                TEST_WORKSPACE_ID,
                ContainerVisibility::Private,
                title,
            ))
            .await
            .created_json();
        th.client
            .add_member(&crate::boards::member(
                &board.id,
                &user2,
                fvoci_model::ContainerRole::Editor,
            ))
            .await
            .check_ok();
        boards.push(board);
    }
    // A former member of the workspace is omitted from the exported archive.
    th.app
        .store()
        .save_member(&crate::boards::member(
            &boards[1].id,
            &third,
            fvoci_model::ContainerRole::Viewer,
        ))
        .await
        .expect("a membership of a third user");
    remove_from_the_test_workspace(&th, &third).await;
    let archive = th.client.get(&format!("{TEST_WS}/archive/export")).await;
    archive.check_ok();

    remove_from_the_test_workspace(&th, &user2).await;
    let imported = crate::export::import(&th.client, TEST_WORKSPACE_ID, &archive.body).await;
    th.teardown().await;

    // The second user, on both boards, once; the third, already gone at export, not at all.
    assert_eq!(imported.value()["membersLeftOut"], json!(1));
}
