//! Whom a user sees of the other users: who shares a workspace they work in, who is on a
//! board with them in a workspace they are in, and themselves. A guest sees who is on
//! its boards. Anybody else is as one who does not exist.

use fvoci_model::{ContainerMember, ContainerVisibility, User, WorkspaceRole};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::boards::{member, new_board};
use crate::common::ROOT_WS;
use crate::common::TEST_WS;
use crate::common::{
    ApiResponse, Client, HIDDEN_WORKSPACE_ID, ROOT_WORKSPACE_ID, Setup, TEST_WORKSPACE_ID,
    TestServer, UNKNOWN_ID,
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

fn names(users: &[User]) -> Vec<&str> {
    let mut names: Vec<&str> = users.iter().map(|user| user.username.as_str()).collect();
    names.sort_unstable();
    names
}

/// A user who is in the hidden workspace only.
async fn somebody_elsewhere(th: &TestServer) -> String {
    let user_id = th.add_user("elsewhere").await.me().await.id;
    for workspace_id in [ROOT_WORKSPACE_ID, TEST_WORKSPACE_ID] {
        th.app
            .store()
            .execute_unchecked(&format!(
                "DELETE FROM workspace_members WHERE user_id = '{user_id}' \
                 AND workspace_id = '{workspace_id}'"
            ))
            .await
            .expect("the user leaves the workspace");
    }
    th.app
        .store()
        .add_workspace_member(HIDDEN_WORKSPACE_ID, &user_id, WorkspaceRole::Member)
        .await
        .expect("the user works in the hidden workspace");
    user_id
}

async fn search(client: &Client, workspace_id: &str, term: &str) -> Vec<User> {
    client
        .get(&format!("/w/{workspace_id}/users?search={term}"))
        .await
        .ok_json()
}

#[tokio::test]
async fn a_user_sees_who_shares_a_workspace_and_nobody_else() {
    let th = Setup::native().start().await.init_basic().await;
    let user2 = th.client2.me().await.id;
    let elsewhere = somebody_elsewhere(&th).await;

    let listed: Vec<User> = th.client.get(&format!("{TEST_WS}/users")).await.ok_json();
    let found = search(&th.client, TEST_WORKSPACE_ID, "elsewhere").await;
    let profile = th.client.get(&format!("{TEST_WS}/users/{elsewhere}")).await;
    let unknown = th
        .client
        .get(&format!("{TEST_WS}/users/{UNKNOWN_ID}"))
        .await;
    let by_ids = th
        .client
        .post(&format!("{TEST_WS}/users"), &[&user2, &elsewhere, &user2])
        .await;
    let in_workspace = th
        .client
        .post(&format!("{TEST_WS}/users"), &[&elsewhere])
        .await;
    let with_nobody = th
        .client
        .post(&format!("{TEST_WS}/users"), &[&user2, UNKNOWN_ID, &user2])
        .await;
    let coworker = th.client.get(&format!("{TEST_WS}/users/{user2}")).await;
    th.teardown().await;

    assert_eq!(names(&listed), ["user1", "user2"], "the workspace's people");
    assert!(found.is_empty(), "{found:?}");
    assert_eq!(answer(&profile), answer(&unknown), "a stranger is nobody");
    // Whom the user does not see is left out of the list, which is not refused, as
    // somebody who does not exist is; who is named twice is there once.
    assert_eq!(
        answer(&by_ids),
        answer(&with_nobody),
        "unseen is as missing"
    );
    assert_eq!(
        names(&by_ids.ok_json::<Vec<User>>()),
        ["user2"],
        "the coworker alone"
    );
    assert!(
        names(&in_workspace.ok_json::<Vec<User>>()).is_empty(),
        "nobody"
    );
    coworker.check_ok();
}

#[tokio::test]
async fn a_guest_sees_who_is_on_its_boards() {
    let th = Setup::native().start().await.init_basic().await;
    let guest = th.client2.me().await.id;
    th.app
        .store()
        .execute_unchecked(&format!(
            "UPDATE workspace_members SET role = 'guest' WHERE user_id = '{guest}' \
             AND workspace_id = '{TEST_WORKSPACE_ID}'"
        ))
        .await
        .expect("the second user becomes a guest of the test workspace");
    // A third user works in the test workspace and shares no board with the guest.
    let third = th.add_user("third").await.me().await.id;
    let board: fvoci_model::Container = th
        .client
        .create_board(&new_board(
            TEST_WORKSPACE_ID,
            ContainerVisibility::Private,
            "Shared",
        ))
        .await
        .created_json();
    th.client
        .add_member(&member(
            &board.id,
            &guest,
            fvoci_model::ContainerRole::Viewer,
        ))
        .await
        .check_ok();

    let listed: Vec<User> = th.client2.get(&format!("{TEST_WS}/users")).await.ok_json();
    let found = search(&th.client2, TEST_WORKSPACE_ID, "third").await;
    let third_by_id = th
        .client2
        .post(&format!("{TEST_WS}/users"), &[&third])
        .await;
    // In the root workspace, where the second user works, the third is a coworker.
    let third_from_root = search(&th.client2, ROOT_WORKSPACE_ID, "third").await;
    th.teardown().await;

    assert_eq!(names(&listed), ["user1", "user2"], "the board's people");
    assert!(found.is_empty(), "{found:?}");
    assert!(
        names(&third_by_id.ok_json::<Vec<User>>()).is_empty(),
        "the third is left out"
    );
    assert_eq!(names(&third_from_root), ["third"]);
}

/// Who left a workspace keeps the author of what they wrote: they are named to who
/// reads it, and found by nobody; a role they still have on a board shows nobody
/// (user decision of O4).
#[tokio::test]
async fn who_left_a_workspace_is_named_where_they_wrote_and_found_nowhere() {
    let th = Setup::native().start().await.init_basic().await;
    let leaver = th.client2.me().await.id;
    let board: fvoci_model::Container = th
        .client
        .create_board(&new_board(
            TEST_WORKSPACE_ID,
            ContainerVisibility::Private,
            "Ours",
        ))
        .await
        .created_json();
    th.client
        .add_member(&member(
            &board.id,
            &leaver,
            fvoci_model::ContainerRole::Editor,
        ))
        .await
        .check_ok();
    th.client2
        .post(
            &format!("{TEST_WS}/boards/{}/cards", board.id),
            &json!({"title": "Mine"}),
        )
        .await
        .check_created();
    for workspace_id in [ROOT_WORKSPACE_ID, TEST_WORKSPACE_ID] {
        th.app
            .store()
            .execute_unchecked(&format!(
                "DELETE FROM workspace_members WHERE user_id = '{leaver}' \
                 AND workspace_id = '{workspace_id}'"
            ))
            .await
            .expect("the second user leaves every workspace");
    }

    // The first user reads the board, where the leaver wrote a card.
    let by_id: Vec<User> = th
        .client
        .post(&format!("{TEST_WS}/users"), &[&leaver])
        .await
        .ok_json();
    let profile = th.client.get(&format!("{TEST_WS}/users/{leaver}")).await;
    let found: Vec<User> = th
        .client
        .get(&format!("{TEST_WS}/users?search=user2"))
        .await
        .ok_json();
    let members: Vec<ContainerMember> = th
        .client
        .get(&format!("{TEST_WS}/containers/{}/members", board.id))
        .await
        .ok_json();
    let archive = th
        .client
        .get(&format!("{TEST_WS}/containers/{}/archive/export", board.id))
        .await;
    archive.check_ok();
    let archive = archive_text(&archive.body);
    // The leaver sees nobody but themselves.
    let user1 = th.client.me().await.id;
    let seen_by_leaver = th.client2.get(&format!("{TEST_WS}/users/{user1}")).await;
    let themselves = th.client2.get(&format!("{TEST_WS}/users/{leaver}")).await;
    th.teardown().await;

    assert_eq!(names(&by_id), ["user2"], "the card's author is named");
    profile.check_ok();
    assert!(found.is_empty(), "nobody finds who left: {found:?}");
    assert!(
        members.iter().all(|member| member.user_id != leaver),
        "the board's members leave out who left: {members:?}"
    );
    assert!(
        !archive.contains(&format!("\"userId\":\"{leaver}\"")),
        "the archive's members leave out who left"
    );
    // The workspace is not there for who left it any more, whoever they ask about.
    seen_by_leaver.check_not_found();
    themselves.check_not_found();
}

#[tokio::test]
async fn a_guest_sees_by_identifier_who_is_on_a_board_with_a_role_and_nobody_else() {
    let th = Setup::native().start().await.init_basic().await;
    let guest = th.client2.me().await.id;
    th.app
        .store()
        .execute_unchecked(&format!(
            "UPDATE workspace_members SET role = 'guest' WHERE user_id = '{guest}'"
        ))
        .await
        .expect("the second user becomes a guest of every workspace");
    let user1 = th.client.me().await.id;
    let third = th.add_user("third").await.me().await.id;
    let shared: fvoci_model::Container = th
        .client
        .create_board(&new_board(
            TEST_WORKSPACE_ID,
            ContainerVisibility::Private,
            "Shared",
        ))
        .await
        .created_json();
    th.client
        .add_member(&member(
            &shared.id,
            &guest,
            fvoci_model::ContainerRole::Viewer,
        ))
        .await
        .check_ok();
    // On this board the third user is a viewer; a request without a role is refused and
    // leaves the guest off the board.
    let other_board: fvoci_model::Container = th
        .client
        .create_board(&new_board(
            TEST_WORKSPACE_ID,
            ContainerVisibility::Private,
            "No role",
        ))
        .await
        .created_json();
    th.client
        .add_member(&member(
            &other_board.id,
            &third,
            fvoci_model::ContainerRole::Viewer,
        ))
        .await
        .check_ok();
    th.client
        .post(
            &format!("{TEST_WS}/containers/{}/members", other_board.id),
            &json!({"userId": guest}),
        )
        .await
        .check_bad_request();

    let coworker = th.client2.get(&format!("{TEST_WS}/users/{user1}")).await;
    let not_shared = th
        .client2
        .post(&format!("{TEST_WS}/users"), &[&third])
        .await;
    let missing = th
        .client2
        .post(&format!("{TEST_WS}/users"), &[UNKNOWN_ID])
        .await;
    let found = search(&th.client2, TEST_WORKSPACE_ID, "third").await;
    th.teardown().await;

    coworker.check_ok();
    assert!(
        names(&not_shared.ok_json::<Vec<User>>()).is_empty(),
        "left out"
    );
    assert_eq!(
        answer(&not_shared),
        answer(&missing),
        "unseen is as missing"
    );
    assert!(found.is_empty(), "{found:?}");
}

#[tokio::test]
async fn a_guest_editing_an_open_board_adds_nobody() {
    let th = Setup::native().start().await.init_basic().await;
    let guest = th.client2.me().await.id;
    th.app
        .store()
        .execute_unchecked(&format!(
            "UPDATE workspace_members SET role = 'guest' WHERE user_id = '{guest}' \
             AND workspace_id = '{TEST_WORKSPACE_ID}'"
        ))
        .await
        .expect("the second user becomes a guest of the test workspace");
    let third = th.add_user("third").await.me().await.id;
    let board: fvoci_model::Container = th
        .client
        .create_board(&new_board(
            TEST_WORKSPACE_ID,
            ContainerVisibility::Open,
            "Open",
        ))
        .await
        .created_json();
    th.client
        .add_member(&member(
            &board.id,
            &guest,
            fvoci_model::ContainerRole::Editor,
        ))
        .await
        .check_ok();

    let added = th
        .client2
        .add_member(&member(
            &board.id,
            &third,
            fvoci_model::ContainerRole::Viewer,
        ))
        .await;
    let nobody = th
        .client2
        .add_member(&member(
            &board.id,
            UNKNOWN_ID,
            fvoci_model::ContainerRole::Viewer,
        ))
        .await;
    th.teardown().await;

    added.check_forbidden();
    assert_eq!(
        answer(&added),
        answer(&nobody),
        "a guest learns nothing of who is there"
    );
}

/// The text of every entry of an archive, one after the other.
fn archive_text(bytes: &[u8]) -> String {
    use std::io::Read as _;
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("the export is an archive");
    let mut text = String::new();
    for index in 0..archive.len() {
        archive
            .by_index(index)
            .expect("an entry")
            .read_to_string(&mut text)
            .expect("an entry of text");
    }
    text
}

/// Ends a user's membership of every workspace of the test server.
async fn leave_every_workspace(th: &TestServer, user_id: &str) {
    for workspace_id in [ROOT_WORKSPACE_ID, TEST_WORKSPACE_ID] {
        th.app
            .store()
            .execute_unchecked(&format!(
                "DELETE FROM workspace_members WHERE user_id = '{user_id}' \
                 AND workspace_id = '{workspace_id}'"
            ))
            .await
            .expect("the user leaves the workspace");
    }
}

/// An open board is read without joining by whoever works in the workspace (and by no
/// uninvited guest), while a private board names who left only to its members.
#[tokio::test]
async fn who_left_is_named_on_an_open_board_to_its_members_only() {
    let th = Setup::native().start().await.init_basic().await;
    let leaver = th.client2.me().await.id;
    let third = th.add_user("third").await;
    let private_board: fvoci_model::Container = th
        .client
        .create_board(&new_board(
            ROOT_WORKSPACE_ID,
            ContainerVisibility::Private,
            "Private",
        ))
        .await
        .created_json();
    th.client
        .add_member_in(
            ROOT_WORKSPACE_ID,
            &fvoci_model::ContainerMember::new(
                &private_board.id,
                &leaver,
                fvoci_model::ContainerRole::Editor,
            ),
        )
        .await
        .check_ok();
    th.client2
        .post(
            &format!("{ROOT_WS}/boards/{}/cards", private_board.id),
            &json!({"title": "Mine"}),
        )
        .await
        .check_created();
    leave_every_workspace(&th, &leaver).await;
    let by_id = async |client: &crate::common::Client| {
        let found: Vec<User> = client
            .post(&format!("{ROOT_WS}/users"), &[&leaver])
            .await
            .ok_json();
        found.len()
    };

    let before = (
        by_id(&third).await,
        third.get(&format!("{ROOT_WS}/users/{leaver}")).await.status,
    );
    let open: fvoci_model::Container = th
        .client
        .create_board(&new_board(
            ROOT_WORKSPACE_ID,
            ContainerVisibility::Open,
            "Open",
        ))
        .await
        .created_json();
    // Once the private board becomes open, workspace workers read it unjoined and see the author.
    th.client
        .patch(
            &format!("{ROOT_WS}/containers/{}", private_board.id),
            &json!({"visibility": "open"}),
        )
        .await
        .check_ok();
    drop(open);
    let after = (
        by_id(&third).await,
        third.get(&format!("{ROOT_WS}/users/{leaver}")).await.status,
    );
    th.teardown().await;

    assert_eq!(before, (0, StatusCode::NOT_FOUND));
    assert_eq!(after, (1, StatusCode::OK));
}

/// An open wiki is read without joining it: who left after writing a page of one is
/// named to whoever works in the workspace, joined or not, and to no guest.
#[tokio::test]
async fn who_left_is_named_on_an_open_wiki_to_whoever_works_there() {
    let th = Setup::native().start().await.init_basic().await;
    let leaver = th.client2.me().await.id;
    let third = th.add_user("third").await;
    let wiki = th
        .client
        .post(&format!("{ROOT_WS}/wikis"), &json!({"visibility": "open"}))
        .await
        .value()["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    th.client2
        .post_empty(&format!("{ROOT_WS}/containers/{wiki}/join"))
        .await
        .check_ok();
    th.client2
        .post(
            &format!("{ROOT_WS}/wikis/{wiki}/pages"),
            &json!({"title": "Mine"}),
        )
        .await
        .check_created();
    leave_every_workspace(&th, &leaver).await;
    let named = async |client: &crate::common::Client| {
        let found: Vec<User> = client
            .post(&format!("{ROOT_WS}/users"), &[&leaver])
            .await
            .ok_json();
        (
            found.len(),
            client
                .get(&format!("{ROOT_WS}/users/{leaver}"))
                .await
                .status,
        )
    };
    let unjoined = named(&third).await;
    let third_id = third.me().await.id;
    th.app
        .store()
        .execute_unchecked(&format!(
            "UPDATE workspace_members SET role = 'guest' WHERE user_id = '{third_id}'"
        ))
        .await
        .expect("the third user becomes a guest");
    let as_guest = named(&third).await;
    th.teardown().await;

    assert_eq!(unjoined, (1, StatusCode::OK), "unjoined, as a reader");
    assert_eq!(as_guest, (0, StatusCode::NOT_FOUND), "a guest, not");
}

/// A guest names who wrote on its board, also when they share no role there any more,
/// and finds them nowhere.
#[tokio::test]
async fn a_guest_names_who_wrote_on_its_board_and_finds_them_nowhere() {
    let th = Setup::native().start().await.init_basic().await;
    let guest = th.client2.me().await.id;
    th.app
        .store()
        .execute_unchecked(&format!(
            "UPDATE workspace_members SET role = 'guest' WHERE user_id = '{guest}'"
        ))
        .await
        .expect("the second user becomes a guest of every workspace");
    let writer = th.add_user("writer").await;
    let writer_id = writer.me().await.id;
    let board: fvoci_model::Container = th
        .client
        .create_board(&new_board(
            ROOT_WORKSPACE_ID,
            ContainerVisibility::Private,
            "Ours",
        ))
        .await
        .created_json();
    for (user_id, editor) in [(&guest, false), (&writer_id, true)] {
        th.client
            .add_member_in(
                ROOT_WORKSPACE_ID,
                &member(
                    &board.id,
                    user_id,
                    if editor {
                        fvoci_model::ContainerRole::Editor
                    } else {
                        fvoci_model::ContainerRole::Viewer
                    },
                ),
            )
            .await
            .check_ok();
    }
    writer
        .post(
            &format!("{ROOT_WS}/boards/{}/cards", board.id),
            &json!({"title": "Mine"}),
        )
        .await
        .check_created();
    th.client
        .delete(&format!(
            "{ROOT_WS}/containers/{}/members/{writer_id}",
            board.id
        ))
        .await
        .check_no_content();

    let named: Vec<User> = th
        .client2
        .post(&format!("{ROOT_WS}/users"), &[&writer_id])
        .await
        .ok_json();
    let found: Vec<User> = th
        .client2
        .get(&format!("{ROOT_WS}/users?search=writer"))
        .await
        .ok_json();
    th.teardown().await;

    assert_eq!(names(&named), ["writer"]);
    assert!(found.is_empty(), "{found:?}");
}
