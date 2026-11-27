//! Integration and application-layer tests for board CRUD, templates, memberships,
//! join/leave flows, and board category filing.
//!
//! Prerequisite fixtures outside the route under test (blocks, sharing, categories)
//! are arranged directly through the store or application layer.

use fvoci_model::PublicContainers;
use fvoci_model::{
    Block, BlockType, Category, CategoryContainers, Container, ContainerMember, ContainerPatch,
    ContainerRole, ContainerVisibility, MinimumRole, User, Workspace, is_id, new_id,
};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use crate::common::ROOT_WS;
use crate::common::TEST_WS;
use crate::common::{
    ApiResponse, Client, ROOT_WORKSPACE_ID, Setup, TEST_WORKSPACE_ID, TestServer, UNKNOWN_ID,
};

mod joining;
mod members;

// -- helpers --------------------------------------------------------------------------

/// The refusal of a board visibility outside the set.
const TYPE_REFUSAL: &str = "the visibility of a board is `open` or `private`";
/// The refusal of a minimum role outside the set.
const ROLE_REFUSAL: &str = "the minimum role of a board is `viewer`, `commenter` or `editor`";

pub(crate) fn new_board(
    workspace_id: &str,
    board_type: ContainerVisibility,
    title: &str,
) -> Container {
    Container {
        workspace_id: workspace_id.to_owned(),
        visibility: board_type,
        title: title.to_owned(),
        ..Container::default()
    }
}

/// `th.Server.App().CreateBoard(board, userID, true)`
pub(crate) async fn board_with_admin(
    th: &TestServer,
    board: Container,
    user_id: &str,
) -> Container {
    th.app
        .create_container_with_admin(board, user_id)
        .await
        .expect("the board is created with its creator as admin")
}

/// `th.Server.App().CreateBoard(board, userID, false)`
pub(crate) async fn board_without_members(
    th: &TestServer,
    board: Container,
    user_id: &str,
) -> Container {
    th.app
        .create_container(board, user_id)
        .await
        .expect("the board is created without members")
}

/// Stores one more workspace, next to the two every test server has, and returns its
/// identifier.
pub(crate) async fn another_workspace(th: &TestServer, member: &str) -> String {
    let workspace = Workspace {
        id: new_id(),
        ..Workspace::default()
    };
    let (store, role) = (th.app.store(), fvoci_model::WorkspaceRole::Member);
    store
        .create_workspace(&workspace)
        .await
        .expect("the workspace is stored");
    let let_in = store
        .add_workspace_member(&workspace.id, member, role)
        .await;
    let_in.expect("the user is let into the workspace");
    workspace.id
}

/// Stores a user who never signs in and returns their identifier: for tests of the
/// application layer that need a second user next to [`TestServer::stranger`].
pub(crate) async fn stored_user(th: &TestServer, username: &str) -> String {
    let mut user = User {
        id: new_id(),
        username: username.to_owned(),
        email: format!("{username}@sample.com"),
        ..User::default()
    };
    th.app
        .store()
        .create_user(&mut user)
        .await
        .expect("the user is stored");
    user.id
}

pub(crate) fn member(board_id: &str, user_id: &str, role: ContainerRole) -> ContainerMember {
    ContainerMember::new(board_id, user_id, role)
}

pub(crate) fn editor(board_id: &str, user_id: &str) -> ContainerMember {
    ContainerMember::new(board_id, user_id, ContainerRole::Editor)
}

/// A membership as a viewer and nothing else.
fn viewer(board_id: &str, user_id: &str) -> ContainerMember {
    ContainerMember::new(board_id, user_id, ContainerRole::Viewer)
}

/// `th.Server.App().AddMemberToBoard(member)`
pub(crate) async fn add_member(th: &TestServer, member: &ContainerMember) -> ContainerMember {
    th.app
        .add_member_to_container(member)
        .await
        .expect("the member is added")
        .expect("the board exists")
}

/// `th.Server.App().GetMembersForBoard(boardID)`
pub(crate) async fn members_of(th: &TestServer, board_id: &str) -> Vec<ContainerMember> {
    th.app
        .members_for_container(board_id)
        .await
        .expect("the members are read")
}

/// `th.Server.App().InsertBlock(block, userID)`: stores a block without going through the
/// blocks routes.
pub(crate) async fn insert_block(th: &TestServer, mut block: Block, user_id: &str) -> Block {
    th.app
        .store()
        .insert_block(&mut block, user_id)
        .await
        .expect("the block is stored");
    block
}

pub(crate) fn view_block(board_id: &str) -> Block {
    Block {
        id: new_id(),
        container_id: board_id.to_owned(),
        created_at: 1,
        updated_at: 1,
        title: "View 1".to_owned(),
        block_type: BlockType::View,
        ..Block::default()
    }
}

pub(crate) async fn get_board(client: &Client, id: &str) -> ApiResponse {
    client.get(&format!("{TEST_WS}/containers/{id}")).await
}

async fn get_members(client: &Client, board_id: &str) -> ApiResponse {
    client
        .get(&format!("{TEST_WS}/containers/{board_id}/members"))
        .await
}

pub(crate) async fn update_member(client: &Client, member: &ContainerMember) -> ApiResponse {
    client
        .put(
            &format!(
                "{TEST_WS}/containers/{}/members/{}",
                member.container_id, member.user_id
            ),
            member,
        )
        .await
}

pub(crate) async fn delete_member(client: &Client, board_id: &str, user_id: &str) -> ApiResponse {
    client
        .delete(&format!(
            "{TEST_WS}/containers/{board_id}/members/{user_id}"
        ))
        .await
}

async fn join_board(client: &Client, board_id: &str) -> ApiResponse {
    client
        .post_empty(&format!("{TEST_WS}/containers/{board_id}/join"))
        .await
}

async fn leave_board(client: &Client, board_id: &str) -> ApiResponse {
    client
        .post_empty(&format!("{TEST_WS}/containers/{board_id}/leave"))
        .await
}

pub(crate) async fn duplicate_board(client: &Client, board_id: &str, query: &str) -> ApiResponse {
    duplicate_board_in(client, TEST_WORKSPACE_ID, board_id, query).await
}

/// `duplicateContainer` of a board of the workspace.
pub(crate) async fn duplicate_board_in(
    client: &Client,
    workspace_id: &str,
    board_id: &str,
    query: &str,
) -> ApiResponse {
    client
        .post_empty(&format!(
            "/w/{workspace_id}/containers/{board_id}/duplicate{query}"
        ))
        .await
}

/// A user's categories in a workspace, each with the identifiers of its boards.
async fn categories_of(
    th: &TestServer,
    user_id: &str,
    workspace_id: &str,
) -> Vec<CategoryContainers> {
    th.app
        .user_category_containers(user_id, workspace_id)
        .await
        .expect("the categories are read")
}

/// The name of the category a user has filed a board under, if any.
pub(crate) async fn category_name_of(
    th: &TestServer,
    user_id: &str,
    workspace_id: &str,
    board_id: &str,
) -> Option<String> {
    categories_of(th, user_id, workspace_id)
        .await
        .into_iter()
        .find(|entry| {
            entry
                .container_metadata
                .iter()
                .any(|metadata| metadata.container_id == board_id)
        })
        .map(|entry| entry.category.name)
}

pub(crate) fn sorted_ids(boards: &[Container]) -> Vec<String> {
    let mut ids: Vec<String> = boards.iter().map(|board| board.id.clone()).collect();
    ids.sort();
    ids
}

// -- the boards of a workspace --------------------------------------------------------

/// Unauthenticated `GET /boards` is rejected.
#[tokio::test]
async fn get_boards_a_non_authenticated_client_should_be_rejected() {
    let th = Setup::native().start().await.init_basic().await;
    board_without_members(
        &th,
        new_board(ROOT_WORKSPACE_ID, ContainerVisibility::Open, ""),
        &th.stranger().await,
    )
    .await;

    let response = th.anonymous().get(&format!("{ROOT_WS}/boards")).await;

    response.check_unauthorized();
    th.teardown().await;
}

/// `GET /boards` returns only the boards the authenticated user is a member of.
#[tokio::test]
async fn get_boards_should_only_return_the_boards_that_the_user_is_a_member_of() {
    let th = Setup::native().start().await.init_basic().await;
    let workspace_id = ROOT_WORKSPACE_ID;
    let other_workspace_id = TEST_WORKSPACE_ID;
    let user1 = th.me_as_no_admin(&th.client).await;
    let user2 = th.client2.me().await;

    let board1 = board_with_admin(
        &th,
        new_board(workspace_id, ContainerVisibility::Open, "Board 1"),
        &user1.id,
    )
    .await;
    let board2 = board_without_members(
        &th,
        new_board(workspace_id, ContainerVisibility::Open, "Board 2"),
        &user2.id,
    )
    .await;
    let board3 = board_with_admin(
        &th,
        new_board(workspace_id, ContainerVisibility::Private, "Board 3"),
        &user1.id,
    )
    .await;
    board_without_members(
        &th,
        new_board(workspace_id, ContainerVisibility::Private, "Board 4"),
        &user1.id,
    )
    .await;
    board_with_admin(
        &th,
        new_board(workspace_id, ContainerVisibility::Private, "Board 5"),
        &user2.id,
    )
    .await;
    let board6 = board_with_admin(
        &th,
        new_board(other_workspace_id, ContainerVisibility::Open, ""),
        &user1.id,
    )
    .await;

    let mut listed: Vec<Container> = th
        .client
        .get(&format!("/w/{workspace_id}/boards"))
        .await
        .ok_json();
    listed.sort_by(|a, b| a.title.cmp(&b.title));
    assert_eq!(
        listed,
        vec![board1, board2, board3],
        "the user's boards and the open boards of the workspace"
    );

    let from_other_workspace: Vec<Container> = th
        .client
        .get(&format!("/w/{other_workspace_id}/boards"))
        .await
        .ok_json();
    assert_eq!(
        sorted_ids(&from_other_workspace),
        vec![board6.id],
        "only the board of the other workspace"
    );

    th.teardown().await;
}

/// A workspace without boards returns an empty JSON array (not `null`), and the workspace
/// must exist.
#[tokio::test]
async fn get_boards_of_a_workspace_without_boards_is_an_empty_list() {
    let th = Setup::native().start().await.init_basic().await;

    let response = th.client.get(&format!("{TEST_WS}/boards")).await;

    response.check_ok();
    assert_eq!(response.text(), "[]", "an empty JSON list");

    for no_workspace in [UNKNOWN_ID, "empty-workspace"] {
        th.client
            .get(&format!("/w/{no_workspace}/boards"))
            .await
            .check_error(StatusCode::NOT_FOUND, "workspace not found");
    }
    th.teardown().await;
}

// -- creating a board -----------------------------------------------------------------

/// Unauthenticated `POST /boards` is rejected.
#[tokio::test]
async fn create_board_a_non_authenticated_user_should_be_rejected() {
    let th = Setup::native().start().await.init_basic().await;

    let response = th
        .anonymous()
        .create_board(&new_board(
            TEST_WORKSPACE_ID,
            ContainerVisibility::Open,
            "board title",
        ))
        .await;

    response.check_unauthorized();
    th.teardown().await;
}

/// Creating a public (open) board sets expected metadata and creator membership.
/// Assertions about the board's blocks are in
/// [`create_board_create_public_board_and_its_blocks`].
#[tokio::test]
async fn create_board_create_public_board() {
    let th = Setup::native().start().await.init_basic().await;
    let me = th.client.me().await;
    let title = "board title 1";

    let board: Container = th
        .client
        .create_board(&new_board(
            TEST_WORKSPACE_ID,
            ContainerVisibility::Open,
            title,
        ))
        .await
        .created_json();
    assert!(
        is_id(&board.id),
        "the board got an identifier: {}",
        board.id
    );
    assert_eq!(board.title, title, "the title is kept");
    assert_eq!(
        board.visibility,
        ContainerVisibility::Open,
        "the type is kept"
    );
    assert_eq!(
        board.workspace_id, TEST_WORKSPACE_ID,
        "the workspace is kept"
    );
    assert_eq!(board.created_by, me.id, "created by the session's user");
    assert_eq!(board.modified_by, me.id, "modified by the session's user");

    // "creating a board should make the creator an admin"
    let members = members_of(&th, &board.id).await;
    assert_eq!(members.len(), 1, "one member: {members:?}");
    assert_eq!(members[0].user_id, me.id, "the creator is the member");
    assert_eq!(members[0].container_id, board.id, "of this board");
    assert_eq!(members[0].role, ContainerRole::Admin, "as an admin");

    // "creator should be able to access the public board and its blocks"
    let read: Container = get_board(&th.client, &board.id).await.ok_json();
    assert_eq!(read, board, "the creator reads the board back");

    // "A non-member user should be able to access the public board but not its blocks"
    let read: Container = get_board(&th.client2, &board.id).await.ok_json();
    assert_eq!(
        read, board,
        "another user of the workspace reads an open board"
    );

    th.teardown().await;
}

/// Blocks on a newly created public board are readable by workspace members.
#[tokio::test]
async fn create_board_create_public_board_and_its_blocks() {
    let th = Setup::native().start().await.init_basic().await;
    let board: Container = th
        .client
        .create_board(&new_board(
            TEST_WORKSPACE_ID,
            ContainerVisibility::Open,
            "board title 1",
        ))
        .await
        .created_json();

    // "creator should be able to access the public board and its blocks"
    let blocks = th
        .client
        .get(&format!("{TEST_WS}/boards/{}/blocks", board.id))
        .await;
    assert_eq!(blocks.ok_json::<Vec<Block>>(), vec![], "no blocks yet");

    // Workspace members can read an open board's blocks without joining, while creating cards is refused until joined.
    th.client2
        .get(&format!("{TEST_WS}/boards/{}/blocks", board.id))
        .await
        .check_ok();
    th.client2
        .post(
            &format!("{TEST_WS}/boards/{}/cards", board.id),
            &json!({"title": "nope"}),
        )
        .await
        .check_forbidden();

    th.teardown().await;
}

/// Creating a private board restricts visibility to members.
#[tokio::test]
async fn create_board_create_private_board() {
    let th = Setup::native().start().await.init_basic().await;
    let me = th.client.me().await;
    let title = "private board title";

    let board: Container = th
        .client
        .create_board(&new_board(
            TEST_WORKSPACE_ID,
            ContainerVisibility::Private,
            title,
        ))
        .await
        .created_json();
    assert!(
        is_id(&board.id),
        "the board got an identifier: {}",
        board.id
    );
    assert_eq!(board.title, title, "the title is kept");
    assert_eq!(
        board.visibility,
        ContainerVisibility::Private,
        "the type is kept"
    );
    assert_eq!(
        board.workspace_id, TEST_WORKSPACE_ID,
        "the workspace is kept"
    );
    assert_eq!(board.created_by, me.id, "created by the session's user");
    assert_eq!(board.modified_by, me.id, "modified by the session's user");

    // "creating a board should make the creator an admin"
    let members = members_of(&th, &board.id).await;
    assert_eq!(members.len(), 1, "one member: {members:?}");
    assert_eq!(members[0].user_id, me.id, "the creator is the member");
    assert_eq!(members[0].container_id, board.id, "of this board");
    assert_eq!(members[0].role, ContainerRole::Admin, "as an admin");

    // "creator should be able to access the private board and its blocks"
    let read: Container = get_board(&th.client, &board.id).await.ok_json();
    assert_eq!(read, board, "the creator reads the board back");

    // "unauthorized user should not be able to access the private board or its blocks":
    // for them it does not exist.
    get_board(&th.client2, &board.id)
        .await
        .check_error(StatusCode::NOT_FOUND, "board not found");

    th.teardown().await;
}

/// Blocks on a newly created private board are accessible only to members.
#[tokio::test]
async fn create_board_create_private_board_and_its_blocks() {
    let th = Setup::native().start().await.init_basic().await;
    let board: Container = th
        .client
        .create_board(&new_board(
            TEST_WORKSPACE_ID,
            ContainerVisibility::Private,
            "private",
        ))
        .await
        .created_json();

    // "creator should be able to access the private board and its blocks"
    let blocks = th
        .client
        .get(&format!("{TEST_WS}/boards/{}/blocks", board.id))
        .await;
    assert_eq!(blocks.ok_json::<Vec<Block>>(), vec![], "no blocks yet");

    // "unauthorized user should not be able to access the private board or its blocks":
    // for them it does not exist.
    th.client2
        .get(&format!("{TEST_WS}/boards/{}/blocks", board.id))
        .await
        .check_error(StatusCode::NOT_FOUND, "board not found");

    th.teardown().await;
}

async fn assert_no_boards(th: &TestServer, user_id: &str) {
    let boards = th
        .app
        .containers_for_user_and_workspace(user_id, TEST_WORKSPACE_ID, PublicContainers::Include)
        .await
        .expect("the boards are read");
    assert!(boards.is_empty(), "no board was created: {boards:?}");
}

/// Rejects creating a board with an invalid board type.
#[tokio::test]
async fn create_board_create_invalid_board_invalid_board_type() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;

    let response = th
        .client
        .post(
            &format!("{TEST_WS}/boards"),
            &json!({"title": "invalid board title", "workspaceId": TEST_WORKSPACE_ID, "visibility": "invalid"}),
        )
        .await;

    response.check_bad_request();
    assert_eq!(response.error_message(), TYPE_REFUSAL);
    assert_eq!(
        response.refused_at(),
        "/visibility",
        "the member, not its value"
    );
    assert_no_boards(&th, &user1.id).await;
    th.teardown().await;
}

/// Rejects creating a board when `visibility` is empty, missing, or `null`.
#[tokio::test]
async fn create_board_create_invalid_board_no_type() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;

    for body in [
        json!({"title": "invalid board title", "workspaceId": TEST_WORKSPACE_ID, "visibility": ""}),
        json!({"title": "invalid board title", "workspaceId": TEST_WORKSPACE_ID}),
        json!({"title": "invalid board title", "workspaceId": TEST_WORKSPACE_ID, "visibility": null}),
    ] {
        let response = th.client.post(&format!("{TEST_WS}/boards"), &body).await;
        response.check_bad_request();
        assert_eq!(response.error_message(), TYPE_REFUSAL, "{body}");
    }

    assert_no_boards(&th, &user1.id).await;
    th.teardown().await;
}

/// Workspace ID validation on board creation: a body naming a different, nonexistent, or
/// malformed workspace is a bad request, while omitting `workspaceId` uses the path workspace.
#[tokio::test]
async fn create_board_create_invalid_board_no_workspace_id() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;

    for body in [
        json!({"title": "x", "workspaceId": ROOT_WORKSPACE_ID, "visibility": "open"}),
        json!({"title": "x", "workspaceId": UNKNOWN_ID, "visibility": "open"}),
        json!({"title": "x", "workspaceId": "nope", "visibility": "open"}),
    ] {
        let response = th.client.post(&format!("{TEST_WS}/boards"), &body).await;
        response.check_error(
            StatusCode::BAD_REQUEST,
            "the board is made in the workspace of the path",
        );
        assert_eq!(
            response.refused_at(),
            "/workspaceId",
            "the member at fault: {body}"
        );
    }
    assert_no_boards(&th, &user1.id).await;

    let made: Container = th
        .client
        .post(
            &format!("{TEST_WS}/boards"),
            &json!({"title": "x", "visibility": "open"}),
        )
        .await
        .created_json();
    assert_eq!(made.workspace_id, TEST_WORKSPACE_ID, "the path's workspace");
    th.teardown().await;
}

/// Additional body validation cases for `POST /boards`.
#[tokio::test]
async fn create_board_validates_the_body() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;

    // Not JSON, or JSON of the wrong shape: a bad request.
    for body in [
        "{\"workspaceId\":",
        "",
        "{\"workspaceId\":\"t\",\"visibility\":5}",
        "[]",
        "5",
        "null",
    ] {
        let response = th
            .client
            .send_raw_body(Method::POST, &format!("{TEST_WS}/boards"), body)
            .await;
        response.check_bad_request();
        assert_eq!(response.problem_type(), "/problems/bad-request", "{body}");
    }

    for role in ["boss", "", "admin"] {
        let response = th
            .client
            .post(
                &format!("{TEST_WS}/boards"),
                &json!({"workspaceId": TEST_WORKSPACE_ID, "visibility": "open", "minimumRole": role}),
            )
            .await;
        response.check_bad_request();
        assert_eq!(response.error_message(), ROLE_REFUSAL, "{role:?}");
    }

    // The visibility is checked before the minimum role.
    let response = th
        .client
        .post(
            &format!("{TEST_WS}/boards"),
            &json!({"workspaceId": TEST_WORKSPACE_ID, "visibility": "X", "minimumRole": "boss"}),
        )
        .await;
    response.check_bad_request();
    assert_eq!(response.error_message(), TYPE_REFUSAL, "visibility first");

    // A new board cannot specify its own identifier.
    let response = th
        .client
        .post(
            &format!("{TEST_WS}/boards"),
            &json!({"id": "my-id", "workspaceId": TEST_WORKSPACE_ID, "visibility": "open"}),
        )
        .await;
    response.check_error(StatusCode::BAD_REQUEST, "new board cannot have an ID");
    assert_eq!(response.refused_at(), "/id", "the member at fault");

    assert_no_boards(&th, &user1.id).await;
    th.teardown().await;
}

/// Checks the JSON wire format of a board: unset `properties` and `cardProperties` are
/// serialized as empty values rather than `null`, there is no `channelId`, and template
/// and kind fields are included.
#[tokio::test]
async fn create_board_wire_format() {
    let th = Setup::native().start().await.init_basic().await;
    let me = th.client.me().await;

    let response = th
        .client
        .post(
            &format!("{TEST_WS}/boards"),
            &json!({"workspaceId": TEST_WORKSPACE_ID, "visibility": "open", "title": "open one"}),
        )
        .await;
    let value = response.value();
    response.check_created();

    assert_eq!(
        value,
        json!({
            "id": value["id"],
            "workspaceId": TEST_WORKSPACE_ID,
            "createdBy": me.id,
            "modifiedBy": me.id,
            "visibility": "open", "kind": "board",
            "minimumRole": "editor",
            "title": "open one",
            "description": "",
            "icon": "",
            "showDescription": false,
            "isTemplate": false,
            "isBuiltin": false,
            "templateVersion": 0,
            "properties": {},
            "cardProperties": [],
            "createdAt": value["createdAt"],
            "updatedAt": value["createdAt"],
        }),
        "the members of a board on the wire"
    );
    assert!(value["createdAt"].as_i64() > Some(0), "stamped: {value}");
    th.teardown().await;
}

/// Board creation files the board under the creator's default category; templates are not filed.
#[tokio::test]
async fn create_board_adds_the_board_to_the_default_category() {
    let th = Setup::native().start().await.init_basic().await;
    let me = th.client.me().await;

    let board: Container = th
        .client
        .create_board(&new_board(
            TEST_WORKSPACE_ID,
            ContainerVisibility::Open,
            "filed",
        ))
        .await
        .created_json();
    let template: Container = th
        .client
        .create_board(&Container {
            is_template: true,
            ..new_board(TEST_WORKSPACE_ID, ContainerVisibility::Open, "a template")
        })
        .await
        .created_json();

    assert_eq!(
        category_name_of(&th, &me.id, TEST_WORKSPACE_ID, &board.id).await,
        Some("Boards".to_owned()),
        "the board is in the default category"
    );
    assert_eq!(
        category_name_of(&th, &me.id, TEST_WORKSPACE_ID, &template.id).await,
        None,
        "the template is in no category"
    );
    th.teardown().await;
}

// -- creating a board template --------------------------------------------------------

/// Creating a public (open) board template sets template metadata and visibility.
#[tokio::test]
async fn create_board_template_create_public_board_template() {
    let th = Setup::native().start().await.init_basic().await;
    let me = th.client.me().await;
    let title = "board template 1";

    let board: Container = th
        .client
        .create_board(&Container {
            is_template: true,
            ..new_board(TEST_WORKSPACE_ID, ContainerVisibility::Open, title)
        })
        .await
        .created_json();
    assert!(
        is_id(&board.id),
        "the board got an identifier: {}",
        board.id
    );
    assert_eq!(board.title, title, "the title is kept");
    assert_eq!(
        board.visibility,
        ContainerVisibility::Open,
        "the type is kept"
    );
    assert_eq!(
        board.workspace_id, TEST_WORKSPACE_ID,
        "the workspace is kept"
    );
    assert_eq!(board.created_by, me.id, "created by the session's user");
    assert_eq!(board.modified_by, me.id, "modified by the session's user");
    assert!(board.is_template, "a template");

    // "creating a board template should make the creator an admin"
    let members = members_of(&th, &board.id).await;
    assert_eq!(members.len(), 1, "one member: {members:?}");
    assert_eq!(members[0].user_id, me.id, "the creator is the member");
    assert_eq!(members[0].container_id, board.id, "of this board");
    assert_eq!(members[0].role, ContainerRole::Admin, "as an admin");

    // "creator should be able to access the public board template and its blocks"
    let read: Container = get_board(&th.client, &board.id).await.ok_json();
    assert_eq!(read, board, "the creator reads the template back");

    // "another user should be able to access the public board template and its blocks"
    let read: Container = get_board(&th.client2, &board.id).await.ok_json();
    assert_eq!(read, board, "another user reads an open template");

    th.teardown().await;
}

/// Blocks on a public board template are readable by workspace members.
#[tokio::test]
async fn create_board_template_create_public_board_template_and_its_blocks() {
    let th = Setup::native().start().await.init_basic().await;
    let board: Container = th
        .client
        .create_board(&Container {
            is_template: true,
            ..new_board(
                TEST_WORKSPACE_ID,
                ContainerVisibility::Open,
                "board template 1",
            )
        })
        .await
        .created_json();

    // "creator should be able to access the public board template and its blocks"
    th.client
        .get(&format!("{TEST_WS}/boards/{}/blocks", board.id))
        .await
        .check_ok();

    // "another user should be able to access the public board template and its blocks"
    th.client2
        .get(&format!("{TEST_WS}/boards/{}/blocks", board.id))
        .await
        .check_ok();

    th.teardown().await;
}

/// Creating a private board template restricts visibility to members.
#[tokio::test]
async fn create_board_template_create_private_board_template() {
    let th = Setup::native().start().await.init_basic().await;
    let me = th.client.me().await;
    let title = "private board template title";

    let board: Container = th
        .client
        .create_board(&Container {
            is_template: true,
            ..new_board(TEST_WORKSPACE_ID, ContainerVisibility::Private, title)
        })
        .await
        .created_json();
    assert!(
        is_id(&board.id),
        "the board got an identifier: {}",
        board.id
    );
    assert_eq!(board.title, title, "the title is kept");
    assert_eq!(
        board.visibility,
        ContainerVisibility::Private,
        "the type is kept"
    );
    assert_eq!(
        board.workspace_id, TEST_WORKSPACE_ID,
        "the workspace is kept"
    );
    assert_eq!(board.created_by, me.id, "created by the session's user");
    assert_eq!(board.modified_by, me.id, "modified by the session's user");

    // "creating a board template should make the creator an admin"
    let members = members_of(&th, &board.id).await;
    assert_eq!(members.len(), 1, "one member: {members:?}");
    assert_eq!(members[0].user_id, me.id, "the creator is the member");
    assert_eq!(members[0].container_id, board.id, "of this board");
    assert_eq!(members[0].role, ContainerRole::Admin, "as an admin");

    // "creator should be able to access the private board template and its blocks"
    let read: Container = get_board(&th.client, &board.id).await.ok_json();
    assert_eq!(read, board, "the creator reads the template back");

    // "unauthorized user should not be able to access the private board template or its
    // blocks": for them it does not exist.
    get_board(&th.client2, &board.id)
        .await
        .check_error(StatusCode::NOT_FOUND, "board not found");

    th.teardown().await;
}

/// Blocks on a private board template are accessible only to members.
#[tokio::test]
async fn create_board_template_create_private_board_template_and_its_blocks() {
    let th = Setup::native().start().await.init_basic().await;
    let board: Container = th
        .client
        .create_board(&Container {
            is_template: true,
            ..new_board(
                TEST_WORKSPACE_ID,
                ContainerVisibility::Private,
                "private template",
            )
        })
        .await
        .created_json();

    // "creator should be able to access the private board template and its blocks"
    th.client
        .get(&format!("{TEST_WS}/boards/{}/blocks", board.id))
        .await
        .check_ok();

    // "unauthorized user should not be able to access the private board template or its
    // blocks": for them it does not exist.
    th.client2
        .get(&format!("{TEST_WS}/boards/{}/blocks", board.id))
        .await
        .check_error(StatusCode::NOT_FOUND, "board not found");

    th.teardown().await;
}

// -- reading a board ------------------------------------------------------------------

/// Unauthenticated `GET /boards/{board_id}` is rejected.
#[tokio::test]
async fn get_container_a_non_authenticated_user_should_be_rejected() {
    let th = Setup::native().start().await.init_basic().await;

    let response = get_board(&th.anonymous(), "boar-id").await;

    response.check_unauthorized();
    assert_eq!(
        response.error_message(),
        "no session token",
        "the session check's refusal: the board's routes need a session"
    );
    th.teardown().await;
}

/// A valid share read token allows fetching a board without authentication.
#[tokio::test]
async fn get_container_valid_read_token_should_be_enough_to_get_the_board() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let board = board_with_admin(
        &th,
        new_board(
            TEST_WORKSPACE_ID,
            ContainerVisibility::Open,
            "public board where user1 is admin",
        ),
        &user1.id,
    )
    .await;
    let sharing_token = th.share(&board.id).await;

    // Without a session the board cannot be read...
    let anonymous = th.anonymous();
    get_board(&anonymous, &board.id).await.check_unauthorized();
    anonymous
        .get(&format!("/shared/{}?readToken=wrong", board.id))
        .await
        .check_not_found();

    // ...but it can by its link. Who reads by the link is not told who made the board,
    // nor its workspace.
    let shared_path = format!("/shared/{}?readToken={sharing_token}", board.id);
    let read: Container = anonymous.get(&shared_path).await.ok_json();
    assert_eq!(
        read,
        Container {
            workspace_id: String::new(),
            created_by: String::new(),
            modified_by: String::new(),
            ..board.clone()
        },
        "the shared board, without its workspace, creator and modifier"
    );

    // Whoever is signed in reads by the link as its reader, a member of the board too;
    // without the link, the member reads as a member.
    for client in [&th.client, &th.client2] {
        let read: Container = client.get(&shared_path).await.ok_json();
        assert_eq!(
            (read.created_by.as_str(), read.modified_by.as_str()),
            ("", ""),
            "by the link, as its reader"
        );
    }
    let read: Container = th
        .client2
        .get(&format!("{TEST_WS}/containers/{}", board.id))
        .await
        .ok_json();
    assert_eq!(
        read.created_by, user1.id,
        "without it, as a user of the workspace"
    );

    th.teardown().await;
}

/// Fetching a nonexistent board returns 404.
#[tokio::test]
async fn get_container_nonexisting_board() {
    let th = Setup::native().start().await.init_basic().await;

    for board_id in [UNKNOWN_ID, "nonexistent board"] {
        let response = get_board(&th.client, board_id).await;
        response.check_error(StatusCode::NOT_FOUND, "board not found");
    }
    th.teardown().await;
}

/// A user without permissions to a private board cannot retrieve it.
#[tokio::test]
async fn get_container_a_user_without_permissions_to_a_private_board_cannot_retrieve_it() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let board = board_without_members(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Private, ""),
        &user1.id,
    )
    .await;

    let response = get_board(&th.client, &board.id).await;

    // Non-members receive the same 404 response as for a nonexistent board.
    response.check_error(StatusCode::NOT_FOUND, "board not found");
    for board_id in [UNKNOWN_ID, "nonexistent board"] {
        let missing = get_board(&th.client, board_id).await;
        assert_eq!(response.status, missing.status, "the same status");
        assert_eq!(
            response.error_message(),
            missing.error_message(),
            "and the same message"
        );
    }
    th.teardown().await;
}

/// A user with permissions to a private board can retrieve it.
#[tokio::test]
async fn get_container_a_user_with_permissions_to_a_private_board_can_retrieve_it() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let board = board_with_admin(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Private, ""),
        &user1.id,
    )
    .await;

    let read: Container = get_board(&th.client, &board.id).await.ok_json();

    assert_eq!(read, board, "the member reads the board");
    th.teardown().await;
}

/// A workspace member without explicit board membership can retrieve an open board.
#[tokio::test]
async fn get_container_a_user_with_permissions_to_the_workspace_can_retrieve_a_public_board() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let board = board_without_members(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Open, "title"),
        &user1.id,
    )
    .await;

    let read: Container = get_board(&th.client, &board.id).await.ok_json();

    assert_eq!(read, board, "a user of the workspace reads an open board");
    th.teardown().await;
}

// -- board metadata -------------------------------------------------------------------
//
// There is no board metadata route; `admin::routes_of_the_host_application_do_not_exist`
// checks that retired routes answer 404 like any other unknown path.

// -- patching a board -----------------------------------------------------------------

fn title_patch(title: &str) -> ContainerPatch {
    ContainerPatch {
        title: Some(title.to_owned()),
        ..ContainerPatch::default()
    }
}

pub(crate) async fn patch_board(
    client: &Client,
    board_id: &str,
    patch: &impl serde::Serialize,
) -> ApiResponse {
    client
        .patch(&format!("{TEST_WS}/containers/{board_id}"), patch)
        .await
}

async fn stored_board(th: &TestServer, board_id: &str) -> Container {
    th.app.container(board_id).await.expect("the board exists")
}

/// Unauthenticated `PATCH /boards/{board_id}` is rejected.
#[tokio::test]
async fn patch_container_a_non_authenticated_user_should_be_rejected() {
    let th = Setup::native().start().await.init_basic().await;
    let initial_title = "title 1";
    let board = board_without_members(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Open, initial_title),
        &th.stranger().await,
    )
    .await;

    let response = patch_board(&th.anonymous(), &board.id, &title_patch("a new title 1")).await;

    response.check_unauthorized();
    assert_eq!(
        stored_board(&th, &board.id).await.title,
        initial_title,
        "the board is unchanged"
    );
    th.teardown().await;
}

/// Patching a nonexistent board returns 404.
#[tokio::test]
async fn patch_container_non_existing_board() {
    let th = Setup::native().start().await.init_basic().await;

    for board_id in [UNKNOWN_ID, "non-existing-board"] {
        let response = patch_board(&th.client, board_id, &title_patch("a new title 2")).await;
        response.check_error(StatusCode::NOT_FOUND, "board not found");
    }
    th.teardown().await;
}

/// Rejects an invalid patch on a board when the caller has permission.
#[tokio::test]
async fn patch_container_invalid_patch_on_a_board_with_permissions() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let board = board_with_admin(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Open, "title"),
        &user1.id,
    )
    .await;

    let response = patch_board(&th.client, &board.id, &json!({"visibility": "invalid"})).await;
    response.check_bad_request();
    assert_eq!(response.error_message(), TYPE_REFUSAL, "the visibility");

    // Empty is no minimum role, and neither is admin: nobody becomes an administrator of
    // a board by joining it.
    for role in ["boss", "", "admin", "Editor"] {
        let response = patch_board(&th.client, &board.id, &json!({"minimumRole": role})).await;
        response.check_bad_request();
        assert_eq!(response.error_message(), ROLE_REFUSAL, "{role:?}");
    }
    assert_eq!(
        stored_board(&th, &board.id).await.minimum_role,
        MinimumRole::Editor,
        "the board keeps the minimum role it started with"
    );

    // Permission is checked before validating the patch body.
    let response = patch_board(&th.client2, &board.id, &json!({"visibility": "invalid"})).await;
    response.check_error(
        StatusCode::FORBIDDEN,
        "access denied to modifying board properties",
    );

    assert_eq!(
        stored_board(&th, &board.id).await,
        board,
        "the board is unchanged"
    );
    th.teardown().await;
}

/// Applies a valid patch to a board when the caller has permission.
#[tokio::test]
async fn patch_container_valid_patch_on_a_board_with_permissions() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let board = board_with_admin(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Open, "title"),
        &user1.id,
    )
    .await;
    let new_title = "a new title";

    let patched: Container = patch_board(&th.client, &board.id, &title_patch(new_title))
        .await
        .ok_json();

    assert_eq!(patched.title, new_title, "the new title is answered");
    assert_eq!(
        stored_board(&th, &board.id).await,
        patched,
        "and stored as answered"
    );
    th.teardown().await;
}

/// Rejects a valid patch when the caller lacks permission on the board.
#[tokio::test]
async fn patch_container_valid_patch_on_a_board_without_permissions() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let initial_title = "title";
    let board = board_without_members(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Open, initial_title),
        &user1.id,
    )
    .await;

    let response = patch_board(&th.client, &board.id, &title_patch("a new title")).await;

    response.check_forbidden();
    assert_eq!(
        response.error_message(),
        "access denied to modifying board properties",
        "why"
    );
    assert_eq!(
        stored_board(&th, &board.id).await.title,
        initial_title,
        "the board is unchanged"
    );
    th.teardown().await;
}

/// An editor can change a board's title, while only an admin can change its type or
/// minimum role; each refusal returns its own error message.
#[tokio::test]
async fn patch_container_permissions_by_what_the_patch_touches() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let user2 = th.client2.me().await;
    let board = board_with_admin(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Open, "title"),
        &user1.id,
    )
    .await;
    add_member(&th, &editor(&board.id, &user2.id)).await;

    let patched: Container = patch_board(&th.client2, &board.id, &title_patch("by the editor"))
        .await
        .ok_json();
    assert_eq!(
        patched.title, "by the editor",
        "an editor changes the title"
    );
    assert_eq!(patched.modified_by, user2.id, "and is recorded as modifier");

    for (patch, message) in [
        (
            json!({"visibility": "private"}),
            "access denied to modifying board type",
        ),
        (
            json!({"minimumRole": "viewer"}),
            "access denied to modifying board type",
        ),
    ] {
        let response = patch_board(&th.client2, &board.id, &patch).await;
        response.check_forbidden();
        assert_eq!(response.error_message(), message, "{patch}");
    }

    // The admin may do all of it.
    let patched: Container = patch_board(
        &th.client,
        &board.id,
        &json!({"visibility": "private", "minimumRole": "viewer", "channelId": "channel"}),
    )
    .await
    .ok_json();
    assert_eq!(patched.visibility, ContainerVisibility::Private, "the type");
    assert_eq!(
        patched.minimum_role,
        MinimumRole::Viewer,
        "the minimum role"
    );
    // Unknown fields like `channelId` are ignored: neither refused, stored, nor returned.
    let answered: Value = get_board(&th.client, &board.id).await.value();
    assert!(answered.get("channelId").is_none(), "{answered}");

    // The minimum role is the board's: a membership does not repeat it.
    let members: Value = get_members(&th.client, &board.id).await.value();
    assert!(
        members.as_array().is_some_and(|members| members
            .iter()
            .all(|member| member.get("minimumRole").is_none())),
        "{members}"
    );

    // Malformed bodies (including `null`) are rejected as bad requests.
    for body in ["{bad", "", "[]", "null", "{\"visibility\":5}"] {
        th.client
            .send_raw_body(
                Method::PATCH,
                &format!("{TEST_WS}/containers/{}", board.id),
                body,
            )
            .await
            .check_bad_request();
    }

    th.teardown().await;
}

// -- deleting a board -----------------------------------------------------------------

/// Unauthenticated `DELETE /boards/{board_id}` is rejected.
#[tokio::test]
async fn delete_container_a_non_authenticated_user_should_be_rejected() {
    let th = Setup::native().start().await.init_basic().await;
    let board = board_without_members(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Open, "title"),
        &th.stranger().await,
    )
    .await;

    let response = th
        .anonymous()
        .delete(&format!("{TEST_WS}/containers/{}", board.id))
        .await;

    response.check_unauthorized();
    assert_eq!(stored_board(&th, &board.id).await, board, "still there");
    th.teardown().await;
}

/// Rejects deleting a board without permission.
#[tokio::test]
async fn delete_container_a_user_without_permissions_should_be_rejected() {
    let th = Setup::native().start().await.init_basic().await;
    let board = board_without_members(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Open, "title"),
        &th.stranger().await,
    )
    .await;

    let response = th
        .client
        .delete(&format!("{TEST_WS}/containers/{}", board.id))
        .await;

    response.check_forbidden();
    assert_eq!(
        response.error_message(),
        "access denied to delete board",
        "why"
    );
    assert_eq!(stored_board(&th, &board.id).await, board, "still there");
    th.teardown().await;
}

/// Deleting a nonexistent board returns 404.
#[tokio::test]
async fn delete_container_non_existing_board() {
    let th = Setup::native().start().await.init_basic().await;

    for board_id in [UNKNOWN_ID, "non-existing-board"] {
        let response = th
            .client
            .delete(&format!("{TEST_WS}/containers/{board_id}"))
            .await;
        response.check_error(StatusCode::NOT_FOUND, "board not found");
    }
    th.teardown().await;
}

/// Deleting an existing board succeeds and removes it from active listings.
#[tokio::test]
async fn delete_container_an_existing_board_should_be_correctly_deleted() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let board = board_with_admin(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Open, "title"),
        &user1.id,
    )
    .await;

    let response = th
        .client
        .delete(&format!("{TEST_WS}/containers/{}", board.id))
        .await;

    response.check_no_content();
    let error = th
        .app
        .container(&board.id)
        .await
        .expect_err("the board is gone");
    assert!(error.is_not_found(), "not found: {error:?}");

    // Deleting an already-deleted board returns 404.
    th.client
        .delete(&format!("{TEST_WS}/containers/{}", board.id))
        .await
        .check_not_found();
    th.teardown().await;
}

// -- restoring a board ----------------------------------------------------------------

pub(crate) async fn undelete_board(client: &Client, board_id: &str) -> ApiResponse {
    client
        .post_empty(&format!("{TEST_WS}/containers/{board_id}/restore"))
        .await
}

async fn assert_board_is_gone(th: &TestServer, board_id: &str) {
    let error = th
        .app
        .container(board_id)
        .await
        .expect_err("the board stays deleted");
    assert!(error.is_not_found(), "not found: {error:?}");
}

/// Unauthenticated `POST /boards/{board_id}/restore` is rejected.
#[tokio::test]
async fn undelete_container_a_non_authenticated_user_should_be_rejected() {
    let th = Setup::native().start().await.init_basic().await;
    let board = board_without_members(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Open, "title"),
        &th.stranger().await,
    )
    .await;
    th.app
        .delete_container(&board.id, &th.stranger().await)
        .await
        .expect("the board is deleted");

    let response = undelete_board(&th.anonymous(), &board.id).await;

    response.check_unauthorized();
    assert_board_is_gone(&th, &board.id).await;
    th.teardown().await;
}

/// Rejects undeleting a board when the user has no membership on it.
#[tokio::test]
async fn undelete_container_a_user_without_membership_should_be_rejected() {
    let th = Setup::native().start().await.init_basic().await;
    let board = board_without_members(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Open, "title"),
        &th.stranger().await,
    )
    .await;
    th.app
        .delete_container(&board.id, &th.stranger().await)
        .await
        .expect("the board is deleted");

    let response = undelete_board(&th.client, &board.id).await;

    // A deleted board the user was no member of answers as one that never existed.
    response.check_error(StatusCode::NOT_FOUND, "board not found");
    assert_board_is_gone(&th, &board.id).await;
    th.teardown().await;
}

/// Rejects undeleting a board when the user is a member (editor) without delete/restore permission.
#[tokio::test]
async fn undelete_container_a_user_with_membership_but_without_permissions_should_be_rejected() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let stranger = th.stranger().await;
    let board = board_without_members(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Open, "title"),
        &stranger,
    )
    .await;
    add_member(&th, &editor(&board.id, &stranger)).await;
    add_member(&th, &editor(&board.id, &user1.id)).await;
    th.app
        .delete_container(&board.id, &stranger)
        .await
        .expect("the board is deleted");

    let response = undelete_board(&th.client, &board.id).await;

    response.check_forbidden();
    assert_board_is_gone(&th, &board.id).await;
    th.teardown().await;
}

/// Undeleting a nonexistent board returns 404.
#[tokio::test]
async fn undelete_container_non_existing_board() {
    let th = Setup::native().start().await.init_basic().await;

    for board_id in [UNKNOWN_ID, "non-existing-board"] {
        let response = undelete_board(&th.client, board_id).await;
        response.check_error(StatusCode::NOT_FOUND, "board not found");
    }
    th.teardown().await;
}

/// Undeleting a previously deleted board restores it.
#[tokio::test]
async fn undelete_container_an_existing_deleted_board_should_be_correctly_undeleted() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let board = board_with_admin(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Open, "title"),
        &user1.id,
    )
    .await;

    // A board that is not deleted is not in the trash: there is nothing to restore.
    undelete_board(&th.client, &board.id)
        .await
        .check_error(StatusCode::NOT_FOUND, "board not found");

    th.app
        .delete_container(&board.id, &th.stranger().await)
        .await
        .expect("the board is deleted");
    assert_board_is_gone(&th, &board.id).await;

    let response = undelete_board(&th.client, &board.id).await;

    let answered: Container = response.ok_json();
    assert_eq!(answered.id, board.id, "the answer is the board");
    assert_eq!(answered.deleted_at, None, "as it is after the restoring");
    let restored = stored_board(&th, &board.id).await;
    assert_eq!(restored, answered, "the board is back, as answered");
    assert_eq!(restored.title, board.title, "with the title it had");
    th.teardown().await;
}

/// An open board with user1 as admin and user2 as editor.
async fn board_with_users(th: &TestServer) -> Container {
    let user1 = th.client.me().await;
    let user2 = th.client2.me().await;
    let board = board_with_admin(
        th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Open, "title"),
        &user1.id,
    )
    .await;
    add_member(th, &editor(&board.id, &user2.id)).await;
    board
}
