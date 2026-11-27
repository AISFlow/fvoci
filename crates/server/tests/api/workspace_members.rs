//! The members of a workspace: its administrators list them, change what one is, and
//! take one out; a board that would be left without an admin passes to who did it.

use fvoci_model::AuditAction;
use fvoci_model::{
    Container, ContainerMember, ContainerRole, Invitation, UserWorkspace, WorkspaceMember,
    WorkspaceRole,
};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::common::{
    ApiResponse, Client, PASSWORD, ROOT_WORKSPACE_ID, Setup, TestServer, UNKNOWN_ID, container_path,
};

const MANAGERS_ONLY: &str = "only the administrators of the workspace manage its members";

fn members_path() -> String {
    format!("/w/{ROOT_WORKSPACE_ID}/members")
}

fn member_path(user_id: &str) -> String {
    format!("/w/{ROOT_WORKSPACE_ID}/members/{user_id}")
}

/// What an answer says, without what names the one request.
fn said(response: &ApiResponse) -> (StatusCode, Value) {
    let mut body: Value = serde_json::from_slice(&response.body).unwrap_or(Value::Null);
    if let Some(object) = body.as_object_mut() {
        object.remove("instance");
        object.remove("requestId");
    }
    (response.status, body)
}

async fn set_role(client: &Client, user_id: &str, role: &str) -> ApiResponse {
    client
        .put(&member_path(user_id), &json!({"role": role}))
        .await
}

/// What the user is in the root workspace; `None` for who is not in it.
async fn role_of(client: &Client) -> Option<WorkspaceRole> {
    let workspaces: Vec<UserWorkspace> = client.get("/users/me/workspaces").await.ok_json();
    workspaces
        .iter()
        .find(|workspace| workspace.id == ROOT_WORKSPACE_ID)
        .map(|workspace| workspace.role)
}

/// A board for the invited only in the root workspace, made by `client`, who is its
/// admin.
async fn private_board(client: &Client, title: &str) -> Container {
    client
        .post(
            &format!("/w/{ROOT_WORKSPACE_ID}/boards"),
            &json!({"workspaceId": ROOT_WORKSPACE_ID, "visibility": "private", "title": title}),
        )
        .await
        .created_json()
}

async fn add_to_board(client: &Client, board: &Container, user_id: &str, role: ContainerRole) {
    let member = ContainerMember::new(board.id.clone(), user_id.to_owned(), role);
    client
        .add_member_in(&board.workspace_id, &member)
        .await
        .check_ok();
}

/// The roles on a board by the identifier of the user, as the store has them.
async fn roles_on(th: &TestServer, board: &Container) -> Vec<(String, Option<ContainerRole>)> {
    th.app
        .store()
        .get_members_for_container(&board.id)
        .await
        .expect("the members of the board")
        .into_iter()
        .map(|member| (member.user_id.clone(), Some(member.role)))
        .collect()
}

/// The boards the audit log says the user took, or added themselves to.
async fn boards_taken_by(th: &TestServer, user_id: &str) -> Vec<String> {
    th.app
        .store()
        .get_audit_log(ROOT_WORKSPACE_ID)
        .await
        .expect("the audit log")
        .into_iter()
        .filter(|entry| {
            entry.actor.person_id() == Some(user_id)
                && entry.action == AuditAction::SelfAddedToContainer
        })
        .map(|entry| entry.target_id)
        .collect()
}

/// A guest of the board, made by a link.
async fn guest_of(th: &TestServer, inviter: &Client, board: &Container, username: &str) -> Client {
    let link: Invitation = inviter
        .post(
            &format!("/w/{ROOT_WORKSPACE_ID}/invitations"),
            &json!({"role": "guest", "containerId": board.id}),
        )
        .await
        .created_json();
    th.anonymous()
        .post(
            "/invitations/accept",
            &json!({"token": link.token, "account": {
                "username": username, "email": format!("{username}@sample.com"),
                "password": PASSWORD,
            }}),
        )
        .await
        .check_ok();
    th.login(username, PASSWORD).await
}

/// The administrators of a workspace list who is in it, its guests too, the oldest
/// membership first; nobody else does.
#[tokio::test]
async fn the_administrators_of_a_workspace_list_its_members() {
    let th = Setup::native().start().await.init_basic().await;
    let board = private_board(&th.client, "Plans").await;
    let visitor = guest_of(&th, &th.client, &board, "visitor").await;
    th.client2
        .patch("/users/me", &json!({"customName": "The Second"}))
        .await
        .check_ok();

    let listed: Vec<WorkspaceMember> = th.client.get(&members_path()).await.ok_json();
    let seen: Vec<(&str, &str, WorkspaceRole)> = listed
        .iter()
        .map(|member| {
            (
                member.username.as_str(),
                member.display_name.as_str(),
                member.role,
            )
        })
        .collect();
    assert_eq!(
        seen,
        [
            ("user1", "", WorkspaceRole::Admin),
            ("user2", "The Second", WorkspaceRole::Member),
            ("visitor", "", WorkspaceRole::Guest),
        ]
    );
    assert_eq!(listed[0].user_id, th.client.me().await.id);
    assert!(
        listed.iter().all(|member| member.created_at > 0),
        "{listed:?}"
    );
    let raw = th.client.get(&members_path()).await.value();
    let mut keys: Vec<&str> = raw[0]
        .as_object()
        .expect("a member")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "createdAt",
            "credentials",
            "customName",
            "displayName",
            "firstName",
            "joinedAsOrgAdmin",
            "lastName",
            "latinFirstName",
            "latinLastName",
            "nameShown",
            "role",
            "title",
            "userId",
            "username"
        ],
        "the name whole, no address, and nothing of the password"
    );

    th.client2
        .get(&members_path())
        .await
        .check_error(StatusCode::FORBIDDEN, MANAGERS_ONLY);
    visitor
        .get(&members_path())
        .await
        .check_error(StatusCode::NOT_FOUND, "workspace not found");
    th.anonymous()
        .get(&members_path())
        .await
        .check_unauthorized();

    th.teardown().await;
}

/// An administrator is made a member and a member an administrator, and nothing else
/// by a change of role but a member a guest; the workspace keeps an administrator.
#[tokio::test]
async fn what_a_member_is_in_a_workspace_is_changed_by_its_administrators() {
    let th = Setup::native().start().await.init_basic().await;
    let third = th.add_user("third").await;
    let (first_id, second_id) = (th.client.me().await.id, th.client2.me().await.id);
    let board = private_board(&th.client, "Plans").await;
    let visitor = guest_of(&th, &th.client, &board, "visitor").await;
    let visitor_id = visitor.me().await.id;

    // Only the administrators, whoever is named.
    for client in [&th.client2, &third] {
        set_role(client, &first_id, "member")
            .await
            .check_error(StatusCode::FORBIDDEN, MANAGERS_ONLY);
    }
    set_role(&visitor, &first_id, "member")
        .await
        .check_error(StatusCode::NOT_FOUND, "workspace not found");

    // Somebody who is no member of the workspace is not there, whatever they are else.
    let absent = said(&set_role(&th.client, UNKNOWN_ID, "member").await);
    assert_eq!(absent.0, StatusCode::NOT_FOUND);
    let stranger = th.stranger().await;
    th.app
        .store()
        .execute_unchecked(&format!(
            "DELETE FROM workspace_members WHERE user_id = '{stranger}'"
        ))
        .await
        .expect("somebody with an account and no workspace");
    for user_id in [stranger.as_str(), "no identifier"] {
        assert_eq!(said(&set_role(&th.client, user_id, "member").await), absent);
        let removed = th.client.delete(&member_path(user_id)).await;
        assert_eq!(said(&removed), absent, "{user_id}");
    }

    // The last administrator stays one.
    let last = set_role(&th.client, &first_id, "member").await;
    last.check_status(StatusCode::CONFLICT);
    assert_eq!(last.error_reason(), "last_admin");
    assert_eq!(role_of(&th.client).await, Some(WorkspaceRole::Admin));

    // A member is made an administrator, and manages the members from then on.
    let made: WorkspaceMember = set_role(&th.client, &second_id, "admin").await.ok_json();
    assert_eq!(
        (made.user_id.as_str(), made.role),
        (second_id.as_str(), WorkspaceRole::Admin)
    );
    assert_eq!(role_of(&th.client2).await, Some(WorkspaceRole::Admin));
    th.client2.get(&members_path()).await.check_ok();
    // The first is no longer the last: made a member by the other, or by themselves.
    set_role(&th.client2, &first_id, "member").await.check_ok();
    assert_eq!(role_of(&th.client).await, Some(WorkspaceRole::Member));
    set_role(&th.client, &second_id, "member")
        .await
        .check_error(StatusCode::FORBIDDEN, MANAGERS_ONLY);
    let again: WorkspaceMember = set_role(&th.client2, &first_id, "member").await.ok_json();
    assert_eq!(again.role, WorkspaceRole::Member, "what they are already");
    set_role(&th.client2, &first_id, "admin").await.check_ok();
    let stepped_down: WorkspaceMember = set_role(&th.client2, &second_id, "member").await.ok_json();
    assert_eq!(stepped_down.role, WorkspaceRole::Member, "by themselves");

    // What a change of role does not make.
    for (user_id, role, case) in [
        (
            &visitor_id,
            "member",
            "a guest comes in by a link for members",
        ),
        (&visitor_id, "admin", "a guest is no administrator at once"),
        (&first_id, "guest", "an administrator is a member first"),
    ] {
        let refused = set_role(&th.client, user_id, role).await;
        refused.check_bad_request();
        assert_eq!(refused.refused_at(), "/role", "{case}");
    }
    th.client
        .put(&member_path(&second_id), &json!({"role": "owner"}))
        .await
        .check_bad_request();
    assert_eq!(role_of(&visitor).await, Some(WorkspaceRole::Guest));

    th.teardown().await;
}

/// A member who is made a guest keeps the boards they are on, as an editor at most; a
/// board they were the only admin of passes to who made the change, and that is
/// recorded where they could not read it before.
#[tokio::test]
async fn a_member_made_a_guest_edits_their_boards_and_administers_none() {
    let th = Setup::native().start().await.init_basic().await;
    let (first_id, second_id) = (th.client.me().await.id, th.client2.me().await.id);
    let theirs = private_board(&th.client2, "Theirs alone").await;
    let shared = private_board(&th.client2, "With another admin").await;
    add_to_board(&th.client2, &shared, &first_id, ContainerRole::Admin).await;
    let viewed = private_board(&th.client, "They view").await;
    add_to_board(&th.client, &viewed, &second_id, ContainerRole::Viewer).await;

    let made: WorkspaceMember = set_role(&th.client, &second_id, "guest").await.ok_json();
    assert_eq!(made.role, WorkspaceRole::Guest);
    assert_eq!(role_of(&th.client2).await, Some(WorkspaceRole::Guest));

    let sorted = |mut roles: Vec<(String, Option<ContainerRole>)>| {
        roles.sort();
        roles
    };
    let both = |first: ContainerRole, second: ContainerRole| {
        sorted(vec![
            (first_id.clone(), Some(first)),
            (second_id.clone(), Some(second)),
        ])
    };
    assert_eq!(
        sorted(roles_on(&th, &theirs).await),
        both(ContainerRole::Admin, ContainerRole::Editor),
        "the board passed to who made the change"
    );
    assert_eq!(
        sorted(roles_on(&th, &shared).await),
        both(ContainerRole::Admin, ContainerRole::Editor),
        "the other admin was there"
    );
    assert_eq!(
        sorted(roles_on(&th, &viewed).await),
        both(ContainerRole::Admin, ContainerRole::Viewer),
        "a role below is as it was"
    );
    assert_eq!(
        boards_taken_by(&th, &first_id).await,
        std::slice::from_ref(&theirs.id),
        "recorded: the one board that was not theirs to read before"
    );

    // The guest reads and edits, and manages nothing.
    th.client2.get(&container_path(&theirs)).await.check_ok();
    th.client2
        .patch(&container_path(&theirs), &json!({"title": "Edited"}))
        .await
        .check_ok();
    th.client2
        .delete(&container_path(&theirs))
        .await
        .check_status(StatusCode::FORBIDDEN);
    th.client.get(&container_path(&theirs)).await.check_ok();

    th.teardown().await;
}

/// Who is taken out of a workspace finds none of it; what they wrote stays under their
/// name, a board they were the only admin of passes to who took them out, and the link
/// they made lets nobody in. They come back by an invitation, with nothing of before.
#[tokio::test]
async fn a_member_taken_out_finds_no_workspace_and_their_boards_pass_on() {
    let th = Setup::native().start().await.init_basic().await;
    let (first_id, second) = (th.client.me().await.id, th.client2.me().await);
    let theirs = private_board(&th.client2, "Theirs alone").await;
    let shared = private_board(&th.client2, "With another admin").await;
    add_to_board(&th.client2, &shared, &first_id, ContainerRole::Admin).await;
    let card: Value = th
        .client2
        .post(
            &format!("/w/{}/boards/{}/cards", theirs.workspace_id, theirs.id),
            &json!({"title": "Written before leaving"}),
        )
        .await
        .value();
    let link: Invitation = th
        .client2
        .post(
            &format!("/w/{ROOT_WORKSPACE_ID}/invitations"),
            &json!({"role": "guest", "containerId": theirs.id}),
        )
        .await
        .created_json();

    // Nobody but an administrator, and no administrator themselves.
    th.client2
        .delete(&member_path(&first_id))
        .await
        .check_error(StatusCode::FORBIDDEN, MANAGERS_ONLY);
    th.client.delete(&member_path(&first_id)).await.check_error(
        StatusCode::FORBIDDEN,
        "another administrator takes an administrator out of the workspace",
    );
    assert_eq!(role_of(&th.client).await, Some(WorkspaceRole::Admin));

    th.client
        .delete(&member_path(&second.id))
        .await
        .check_no_content();
    assert_eq!(role_of(&th.client2).await, None);
    let absent = said(&th.client2.get(&format!("/w/{UNKNOWN_ID}")).await);
    for path in [
        format!("/w/{ROOT_WORKSPACE_ID}"),
        format!("/w/{ROOT_WORKSPACE_ID}/boards"),
        format!("/w/{ROOT_WORKSPACE_ID}/users"),
    ] {
        assert_eq!(said(&th.client2.get(&path).await), absent, "{path}");
    }
    for board in [&theirs, &shared] {
        th.client2
            .get(&container_path(board))
            .await
            .check_status(StatusCode::NOT_FOUND);
    }
    th.client
        .delete(&member_path(&second.id))
        .await
        .check_status(StatusCode::NOT_FOUND);

    // Their boards: the one they alone managed passed on, and they are on neither.
    let admin_alone = vec![(first_id.clone(), Some(ContainerRole::Admin))];
    assert_eq!(roles_on(&th, &theirs).await, admin_alone);
    assert_eq!(roles_on(&th, &shared).await, admin_alone);
    assert_eq!(
        boards_taken_by(&th, &first_id).await,
        std::slice::from_ref(&theirs.id)
    );
    // What they wrote is there, under their name.
    let cards: Vec<Value> = th
        .client
        .get(&format!(
            "/w/{}/boards/{}/cards",
            theirs.workspace_id, theirs.id
        ))
        .await
        .ok_json();
    assert_eq!(cards.len(), 1, "{cards:?}");
    assert_eq!(cards[0]["id"], card["id"]);
    assert_eq!(cards[0]["createdBy"], json!(second.id));
    let writer: Vec<Value> = th
        .client
        .post(
            &format!("/w/{ROOT_WORKSPACE_ID}/users"),
            &json!([second.id]),
        )
        .await
        .ok_json();
    assert_eq!(writer[0]["username"], json!(second.username));
    // The link they made lets nobody in.
    th.anonymous()
        .post("/invitations/preview", &json!({"token": link.token}))
        .await
        .check_status(StatusCode::NOT_FOUND);

    // Back by an invitation: a member, with none of the boards of before.
    let back = th.member_link().await;
    th.client2
        .post("/invitations/accept", &json!({"token": back}))
        .await
        .check_ok();
    assert_eq!(role_of(&th.client2).await, Some(WorkspaceRole::Member));
    th.client2
        .get(&container_path(&theirs))
        .await
        .check_status(StatusCode::NOT_FOUND);
    assert_eq!(roles_on(&th, &theirs).await, admin_alone);

    th.teardown().await;
}

/// Being, or no longer being, an administrator of the instance changes what nobody
/// is in a workspace: that is changed by the route of its members alone, and there a
/// workspace keeps its last administrator.
#[tokio::test]
async fn the_instance_role_changes_no_role_in_a_workspace_and_its_last_administrator_stays() {
    let th = Setup::native().start().await.init_basic().await;
    let (first_id, second_id) = (th.client.me().await.id, th.client2.me().await.id);
    let instance_admin = |client: &Client, user_id: &str, is_admin: bool| {
        let path = format!("/users/{user_id}/admin");
        let client = client.clone();
        async move { client.put(&path, &json!({"isAdmin": is_admin})).await }
    };
    // The second becomes an administrator of the instance: a member of the workspace
    // still, who does not manage its people.
    instance_admin(&th.client, &second_id, true)
        .await
        .check_ok();
    assert_eq!(role_of(&th.client2).await, Some(WorkspaceRole::Member));
    set_role(&th.client2, &first_id, "member")
        .await
        .check_status(StatusCode::FORBIDDEN);

    // The first, the only administrator of the workspace, stops being one of the
    // instance: the workspace still has them.
    instance_admin(&th.client2, &first_id, false)
        .await
        .check_ok();
    assert!(
        !th.client.me().await.is_admin,
        "no administrator of the instance"
    );
    assert_eq!(role_of(&th.client).await, Some(WorkspaceRole::Admin));
    // By the route of the workspace's members its last administrator stays one.
    let refused = set_role(&th.client, &first_id, "member").await;
    refused.check_status(StatusCode::CONFLICT);
    assert_eq!(refused.error_reason(), "last_admin");
    assert_eq!(role_of(&th.client).await, Some(WorkspaceRole::Admin));

    th.teardown().await;
}

/// The sidebar of a guest has the boards they are on, and no other: not one they
/// filed while they worked in the workspace and never were on. The routes that change
/// the sidebar are not a guest's.
#[tokio::test]
async fn the_sidebar_of_a_guest_has_the_boards_they_are_on_and_no_other() {
    let th = Setup::native().start().await.init_basic().await;
    let second_id = th.client2.me().await.id;
    let joined = private_board(&th.client2, "Theirs").await;
    let open: Container = th
        .client
        .post(
            &format!("/w/{ROOT_WORKSPACE_ID}/boards"),
            &json!({"workspaceId": ROOT_WORKSPACE_ID, "visibility": "open", "title": "Open, never joined"}),
        )
        .await
        .created_json();
    let sidebar = format!("/w/{ROOT_WORKSPACE_ID}/categories");
    let filed = async || -> (String, Vec<String>) {
        let categories = th.client2.get(&sidebar).await.value();
        let boards = categories
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|category| {
                category["containerMetadata"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
            })
            .filter_map(|entry| entry["containerId"].as_str().map(str::to_owned))
            .collect();
        (
            categories[0]["id"].as_str().unwrap_or_default().to_owned(),
            boards,
        )
    };
    let (category_id, _) = filed().await;
    let file_the_open_board = format!("{sidebar}/{category_id}/containers/{}", open.id);
    th.client2
        .post_empty(&file_the_open_board)
        .await
        .check_no_content();
    let mut both = vec![joined.id.clone(), open.id.clone()];
    both.sort();
    let mut before = filed().await.1;
    before.sort();
    assert_eq!(before, both, "as a member: what they filed");

    set_role(&th.client, &second_id, "guest").await.check_ok();
    assert_eq!(
        filed().await.1,
        std::slice::from_ref(&joined.id),
        "as a guest: the board they are on"
    );
    th.client2
        .post_empty(&file_the_open_board)
        .await
        .check_error(StatusCode::NOT_FOUND, "workspace not found");
    th.client2
        .post(&sidebar, &json!({"name": "Mine"}))
        .await
        .check_status(StatusCode::NOT_FOUND);

    th.teardown().await;
}
