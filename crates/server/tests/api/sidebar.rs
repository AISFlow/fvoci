//! Integration and application-layer tests for sidebar categories and board filing:
//! category CRUD, ordering, default category creation, validation, and cross-user isolation.
//!
//! All tests run against a real store.

use fvoci_model::{
    Category, CategoryContainerMetadata, CategoryContainers, CategoryType, Container,
    ContainerVisibility, User, is_id, new_id,
};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use crate::common::ROOT_WS;
use crate::common::TEST_WS;
use crate::common::{
    ApiResponse, Client, ROOT_WORKSPACE_ID, Setup, TEST_WORKSPACE_ID, TestServer, UNKNOWN_ID,
};

mod isolation;
mod routes;

/// What a write answers that was given text where it needs an identifier.
pub(crate) const NOT_AN_IDENTIFIER: &str = "an identifier in the request is not a valid identifier";

/// Stores the user of an application-level test, a member of [`TEST_WORKSPACE_ID`] who never
/// signs in, and returns the identifier; somebody else is the harness's stranger.
async fn stored_user(th: &TestServer) -> String {
    let mut user = User {
        id: new_id(),
        username: "category-owner".to_owned(),
        email: "category-owner@sample.com".to_owned(),
        ..User::default()
    };
    th.app
        .store()
        .create_user(&mut user)
        .await
        .expect("the user is stored");
    th.let_into_the_second_workspace(&user.id).await;
    user.id
}

pub(crate) fn category(workspace_id: &str, user_id: &str, name: &str) -> Category {
    Category {
        name: name.to_owned(),
        user_id: user_id.to_owned(),
        workspace_id: workspace_id.to_owned(),
        ..Category::default()
    }
}

/// Puts a board into the store without going through the board routes, with `user_id` as
/// its administrator.
async fn board_with_admin(th: &TestServer, workspace_id: &str, user_id: &str) -> Container {
    let mut board = Container {
        id: new_id(),
        workspace_id: workspace_id.to_owned(),
        visibility: ContainerVisibility::Open,
        ..Container::default()
    };
    th.app
        .store()
        .insert_container_with_admin(&mut board, user_id)
        .await
        .expect("the board is inserted");
    board
}

/// Puts an open board into the store that `creator_id` created but is not a member of.
async fn board_without_member(th: &TestServer, workspace_id: &str, creator_id: &str) -> Container {
    let mut board = Container {
        id: new_id(),
        workspace_id: workspace_id.to_owned(),
        visibility: ContainerVisibility::Open,
        ..Container::default()
    };
    th.app
        .store()
        .insert_container(&mut board, creator_id)
        .await
        .expect("the board is inserted");
    board
}

/// Puts an open board into the store, of which nobody is a member, and returns its
/// identifier: every user of the workspace sees it, and it is filed nowhere by itself.
pub(crate) async fn open_board(th: &TestServer, workspace_id: &str) -> String {
    board_without_member(th, workspace_id, &th.stranger().await)
        .await
        .id
}

/// Puts `count` open boards into the store, as [`open_board`] does.
pub(crate) async fn open_boards(th: &TestServer, workspace_id: &str, count: usize) -> Vec<String> {
    let mut boards = Vec::with_capacity(count);
    for _ in 0..count {
        boards.push(open_board(th, workspace_id).await);
    }
    boards
}

/// Puts a private board into the store that the test's users are no members of, and
/// returns its identifier.
async fn board_of_somebody_else(th: &TestServer, workspace_id: &str) -> String {
    let mut board = Container {
        id: new_id(),
        workspace_id: workspace_id.to_owned(),
        visibility: ContainerVisibility::Private,
        ..Container::default()
    };
    th.app
        .store()
        .insert_container(&mut board, &th.stranger().await)
        .await
        .expect("the board is inserted");
    board.id
}

fn categories_path(workspace_id: &str) -> String {
    format!("/w/{workspace_id}/categories")
}

/// `GetUserCategoryBoards`
pub(crate) async fn category_boards(
    client: &Client,
    workspace_id: &str,
) -> Vec<CategoryContainers> {
    client.get(&categories_path(workspace_id)).await.ok_json()
}

/// `CreateCategory`
pub(crate) async fn create_category(client: &Client, category: &Category) -> Category {
    client
        .post(&categories_path(&category.workspace_id), category)
        .await
        .created_json()
}

/// `UpdateCategoryBoard`
pub(crate) async fn file_board(
    client: &Client,
    workspace_id: &str,
    category_id: &str,
    board_id: &str,
) {
    let response = client
        .post_empty(&format!(
            "/w/{workspace_id}/categories/{category_id}/containers/{board_id}"
        ))
        .await;
    response.check_no_content();
}

pub(crate) async fn put_empty(client: &Client, path: &str) -> ApiResponse {
    Client::send(client.request(Method::PUT, path)).await
}

fn names(category_boards: &[CategoryContainers]) -> Vec<&str> {
    category_boards
        .iter()
        .map(|entry| entry.category.name.as_str())
        .collect()
}

pub(crate) fn ids(category_boards: &[CategoryContainers]) -> Vec<&str> {
    category_boards
        .iter()
        .map(|entry| entry.category.id.as_str())
        .collect()
}

pub(crate) fn board_ids(entry: &CategoryContainers) -> Vec<&str> {
    entry
        .container_metadata
        .iter()
        .map(|metadata| metadata.container_id.as_str())
        .collect()
}

fn sorted(mut values: Vec<&str>) -> Vec<&str> {
    values.sort_unstable();
    values
}

fn visible(board_id: &str) -> CategoryContainerMetadata {
    CategoryContainerMetadata {
        container_id: board_id.to_owned(),
        hidden: false,
    }
}

/// The one entry with the given category identifier.
#[track_caller]
pub(crate) fn entry<'a>(
    category_boards: &'a [CategoryContainers],
    category_id: &str,
) -> &'a CategoryContainers {
    category_boards
        .iter()
        .find(|entry| entry.category.id == category_id)
        .unwrap_or_else(|| panic!("category {category_id} is listed"))
}

#[track_caller]
pub(crate) fn check_error(response: &ApiResponse, status: StatusCode, message: &str) {
    response.check_error(status, message);
}

// ---------------------------------------------------------------------------------------
// Sidebar HTTP route workflows

/// End-to-end sidebar workflow: creating categories, filing boards, reordering, and deleting.
///
/// Creates its boards through the board routes, which file a new board under the
/// creator's default category. [`sidebar_with_boards_from_the_store`] is the same walk
/// without those routes.
#[tokio::test]
async fn sidebar() {
    let th = Setup::one_user().start().await;
    let user_id = th.client.me().await.id;

    // A new board ends up in the default "Boards" category.
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;

    let listed = category_boards(&th.client, TEST_WORKSPACE_ID).await;
    assert_eq!(names(&listed), ["Boards"], "only the default category");
    assert_eq!(board_ids(&listed[0]), [board.id.as_str()], "the new board");

    // A second board is moved into a new category.
    let board2 = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let created = create_category(
        &th.client,
        &category(TEST_WORKSPACE_ID, &user_id, "Category 2"),
    )
    .await;
    file_board(&th.client, TEST_WORKSPACE_ID, &created.id, &board2.id).await;

    // New categories end up on top.
    let listed = category_boards(&th.client, TEST_WORKSPACE_ID).await;
    assert_eq!(names(&listed), ["Category 2", "Boards"], "two categories");
    assert_eq!(
        board_ids(&listed[0]),
        [board2.id.as_str()],
        "the moved board"
    );

    // Deleting the category moves its boards to the default one.
    th.client
        .delete(&format!(
            "{}/{}",
            categories_path(TEST_WORKSPACE_ID),
            created.id
        ))
        .await
        .check_no_content();
    let listed = category_boards(&th.client, TEST_WORKSPACE_ID).await;
    assert_eq!(names(&listed), ["Boards"], "the default category is left");
    assert_eq!(
        listed[0].container_metadata.len(),
        2,
        "both boards are in it"
    );
    assert!(
        listed[0].container_metadata.contains(&visible(&board.id)),
        "the first board is there"
    );
    assert!(
        listed[0].container_metadata.contains(&visible(&board2.id)),
        "the second board is there"
    );

    th.teardown().await;
}

/// Sidebar workflow with boards inserted directly into the store: the first is filed when
/// the default category is created, and the second is filed explicitly.
#[tokio::test]
async fn sidebar_with_boards_from_the_store() {
    let th = Setup::one_user().start().await;
    let user_id = th.client.me().await.id;

    let board = board_with_admin(&th, TEST_WORKSPACE_ID, &user_id).await;
    let listed = category_boards(&th.client, TEST_WORKSPACE_ID).await;
    assert_eq!(names(&listed), ["Boards"], "only the default category");
    assert_eq!(board_ids(&listed[0]), [board.id.as_str()], "the board");
    assert_eq!(
        listed[0].category.category_type,
        CategoryType::System,
        "the default category is a system category"
    );

    let board2 = board_with_admin(&th, TEST_WORKSPACE_ID, &user_id).await;
    let created = create_category(
        &th.client,
        &category(TEST_WORKSPACE_ID, &user_id, "Category 2"),
    )
    .await;
    file_board(&th.client, TEST_WORKSPACE_ID, &created.id, &board2.id).await;

    let listed = category_boards(&th.client, TEST_WORKSPACE_ID).await;
    assert_eq!(names(&listed), ["Category 2", "Boards"], "two categories");
    assert_eq!(
        board_ids(&listed[0]),
        [board2.id.as_str()],
        "the moved board"
    );
    assert_eq!(
        board_ids(&listed[1]),
        [board.id.as_str()],
        "the other stays"
    );

    th.client
        .delete(&format!(
            "{}/{}",
            categories_path(TEST_WORKSPACE_ID),
            created.id
        ))
        .await
        .check_no_content();
    let listed = category_boards(&th.client, TEST_WORKSPACE_ID).await;
    assert_eq!(names(&listed), ["Boards"], "the default category is left");
    assert_eq!(
        sorted(board_ids(&listed[0])),
        sorted(vec![board.id.as_str(), board2.id.as_str()]),
        "both boards are in it"
    );
    assert!(
        listed[0]
            .container_metadata
            .iter()
            .all(|metadata| !metadata.hidden),
        "and neither is hidden"
    );

    th.teardown().await;
}

/// Hiding and unhiding a board in a sidebar category.
///
/// The board comes from the store rather than from the board routes; it is filed under
/// the default category when that is created, which is all this test needs of it.
#[tokio::test]
async fn hide_unhide_container() {
    let th = Setup::one_user().start().await;
    let user_id = th.client.me().await.id;
    board_with_admin(&th, TEST_WORKSPACE_ID, &user_id).await;

    // The board is not hidden to begin with.
    let listed = category_boards(&th.client, TEST_WORKSPACE_ID).await;
    assert_eq!(names(&listed), ["Boards"], "only the default category");
    assert_eq!(listed[0].container_metadata.len(), 1, "one board");
    assert!(!listed[0].container_metadata[0].hidden, "not hidden");

    let place = format!(
        "{}/{}/containers/{}",
        categories_path(TEST_WORKSPACE_ID),
        listed[0].category.id,
        listed[0].container_metadata[0].container_id
    );
    let hidden = async || {
        category_boards(&th.client, TEST_WORKSPACE_ID).await[0].container_metadata[0].hidden
    };

    let response = put_empty(&th.client, &format!("{place}/hide")).await;
    response.check_no_content();
    assert!(hidden().await, "hidden now");

    // Hiding a hidden board has no effect.
    put_empty(&th.client, &format!("{place}/hide"))
        .await
        .check_no_content();
    assert!(hidden().await, "still hidden");

    let response = put_empty(&th.client, &format!("{place}/unhide")).await;
    response.check_no_content();
    assert!(!hidden().await, "shown again");

    // Showing a visible board has no effect.
    put_empty(&th.client, &format!("{place}/unhide"))
        .await
        .check_no_content();
    assert!(!hidden().await, "still shown");

    th.teardown().await;
}

// ---------------------------------------------------------------------------------------
// Application-layer category tests

/// Creating a custom category persists it with the expected defaults.
#[tokio::test]
async fn create_category_base_case() {
    let th = Setup::native().start().await;
    let user_id = stored_user(&th).await;

    let created = th
        .app
        .create_category(category(TEST_WORKSPACE_ID, &user_id, "Category"))
        .await
        .expect("a valid category is created");

    assert!(is_id(&created.id), "it got an identifier: {}", created.id);
    assert_eq!(created.name, "Category", "the name is kept");
    assert_eq!(
        created.category_type,
        CategoryType::Custom,
        "custom by default"
    );
    assert!(created.created_at > 0, "it got a creation time");
    assert_eq!(
        created.updated_at, created.created_at,
        "and the same update time"
    );
    let stored = th.app.category(&created.id).await.expect("it is stored");
    assert_eq!(stored, created, "what is returned is what is stored");

    th.teardown().await;
}

/// Rejects invalid category payloads at the application layer.
///
/// Unknown category types are prevented by the `CategoryType` enum and checked over HTTP
/// in [`create_category_refuses_invalid_requests`].
#[tokio::test]
async fn create_category_creating_invalid_category() {
    let th = Setup::native().start().await;
    let user_id = stored_user(&th).await;
    let cases = [
        (
            category(TEST_WORKSPACE_ID, &user_id, ""),
            "the category has no name",
        ),
        (
            category(TEST_WORKSPACE_ID, "", "Name"),
            "the category names no user",
        ),
        (
            category("", &user_id, "Name"),
            "the category names no workspace",
        ),
    ];

    for (invalid, message) in cases {
        let error = th
            .app
            .create_category(invalid)
            .await
            .expect_err("an invalid category is refused");
        assert_eq!(error.status(), StatusCode::BAD_REQUEST, "{error}");
        assert_eq!(error.to_string(), message, "the reason");
    }
    let stored = th
        .app
        .store()
        .get_user_categories(&user_id, TEST_WORKSPACE_ID)
        .await
        .expect("the categories are read");
    assert!(stored.is_empty(), "nothing was stored: {stored:?}");

    th.teardown().await;
}

/// Updating a custom category persists its new name and state.
#[tokio::test]
async fn update_category_base_case() {
    let th = Setup::native().start().await;
    let user_id = stored_user(&th).await;
    let created = th
        .app
        .create_category(category(TEST_WORKSPACE_ID, &user_id, "Category"))
        .await
        .expect("the category is created");

    let change = Category {
        name: "Renamed".to_owned(),
        collapsed: true,
        // Members that an update does not touch.
        sort_order: 990,
        created_at: 1,
        category_type: CategoryType::System,
        ..created.clone()
    };
    let updated = th
        .app
        .update_category(change)
        .await
        .expect("the owner updates the category");

    assert_eq!(updated.name, "Renamed", "the name changed");
    assert!(updated.collapsed, "the collapsed state changed");
    assert!(
        updated.updated_at >= created.updated_at,
        "the update time moved"
    );
    assert_eq!(updated.sort_order, created.sort_order, "the place is kept");
    assert_eq!(
        updated.created_at, created.created_at,
        "the creation time is kept"
    );
    assert_eq!(
        updated.category_type,
        CategoryType::Custom,
        "the type is kept"
    );
    let stored = th.app.category(&created.id).await.expect("it is stored");
    assert_eq!(stored, updated, "what is returned is what is stored");

    th.teardown().await;
}

/// Rejects invalid category updates at the application layer.
///
/// The unknown-type case is checked over the API in
/// [`update_category_refuses_invalid_requests`].
#[tokio::test]
async fn update_category_updating_invalid_category() {
    let th = Setup::native().start().await;
    let user_id = stored_user(&th).await;
    let created = th
        .app
        .create_category(category(TEST_WORKSPACE_ID, &user_id, "Category"))
        .await
        .expect("the category is created");

    // Without an identifier the update is given a fresh one, which names no category.
    let no_id = Category {
        id: String::new(),
        ..created.clone()
    };
    let error = th
        .app
        .update_category(no_id)
        .await
        .expect_err("an update needs the identifier");
    assert_eq!(error.status(), StatusCode::NOT_FOUND, "{error}");

    let cases = [
        (
            Category {
                name: String::new(),
                ..created.clone()
            },
            "the category has no name",
        ),
        (
            Category {
                user_id: String::new(),
                ..created.clone()
            },
            "the category names no user",
        ),
        (
            Category {
                workspace_id: String::new(),
                ..created.clone()
            },
            "the category names no workspace",
        ),
    ];
    for (invalid, message) in cases {
        let error = th
            .app
            .update_category(invalid)
            .await
            .expect_err("an invalid category is refused");
        assert_eq!(error.status(), StatusCode::BAD_REQUEST, "{error}");
        assert_eq!(error.to_string(), message, "the reason");
    }
    let stored = th.app.category(&created.id).await.expect("it is stored");
    assert_eq!(stored, created, "nothing changed");

    th.teardown().await;
}

/// Rejects updating a category owned by another user.
#[tokio::test]
async fn update_category_trying_to_update_someone_elses_category() {
    let th = Setup::native().start().await;
    let user_id = stored_user(&th).await;
    let created = th
        .app
        .create_category(category(TEST_WORKSPACE_ID, &user_id, "Category"))
        .await
        .expect("the category is created");

    let change = Category {
        name: "Taken over".to_owned(),
        user_id: th.stranger().await,
        ..created.clone()
    };
    let error = th
        .app
        .update_category(change)
        .await
        .expect_err("another user's category cannot be updated");

    // Another user's category, like one of another workspace, does not exist for this one.
    assert_eq!(error.status(), StatusCode::NOT_FOUND, "{error}");
    assert_eq!(error.to_string(), "category not found", "the answer");
    let stored = th.app.category(&created.id).await.expect("it is stored");
    assert_eq!(stored, created, "nothing changed");

    th.teardown().await;
}

/// Rejects updating a category belonging to another workspace.
#[tokio::test]
async fn update_category_trying_to_update_some_other_workspaces_category() {
    let th = Setup::native().start().await;
    let user_id = stored_user(&th).await;
    let created = th
        .app
        .create_category(category(TEST_WORKSPACE_ID, &user_id, "Category"))
        .await
        .expect("the category is created");

    let change = Category {
        name: "Moved".to_owned(),
        workspace_id: ROOT_WORKSPACE_ID.to_owned(),
        ..created.clone()
    };
    let error = th
        .app
        .update_category(change)
        .await
        .expect_err("a category of another workspace cannot be updated");

    // Another user's category, like one of another workspace, does not exist for this one.
    assert_eq!(error.status(), StatusCode::NOT_FOUND, "{error}");
    assert_eq!(error.to_string(), "category not found", "the answer");
    let stored = th.app.category(&created.id).await.expect("it is stored");
    assert_eq!(stored, created, "nothing changed");

    th.teardown().await;
}

/// The answer that creates the default category of a user who has other categories is
/// the same as the next one: the categories in their order, with their `sortOrder` as
/// stored. (It was off once: the new category last, the others not yet moved.)
#[tokio::test]
async fn the_answer_that_creates_the_default_category_is_the_next_one() {
    let th = Setup::native().start().await;
    let user_id = stored_user(&th).await;
    for name in ["first", "second"] {
        th.app
            .create_category(Category {
                name: name.to_owned(),
                user_id: user_id.clone(),
                workspace_id: TEST_WORKSPACE_ID.to_owned(),
                ..Category::default()
            })
            .await
            .expect("a category is made");
    }
    let creating = th
        .app
        .user_category_containers(&user_id, TEST_WORKSPACE_ID)
        .await
        .expect("listed");
    let next = th
        .app
        .user_category_containers(&user_id, TEST_WORKSPACE_ID)
        .await
        .expect("listed");
    assert_eq!(creating, next);
    assert_eq!(creating.len(), 3, "{creating:?}");
    th.teardown().await;
}

/// The default category of a user, created by listing the user's categories.
async fn default_category(th: &TestServer, user_id: &str, workspace_id: &str) -> Category {
    let listed = th
        .app
        .user_category_containers(user_id, workspace_id)
        .await
        .expect("the categories are listed");
    listed
        .into_iter()
        .map(|entry| entry.category)
        .find(|category| category.category_type == CategoryType::System)
        .expect("the default category exists")
}

/// Renaming a system category is refused.
#[tokio::test]
async fn update_category_should_not_be_allowed_to_rename_system_category() {
    let th = Setup::native().start().await;
    let user_id = stored_user(&th).await;
    let system = default_category(&th, &user_id, TEST_WORKSPACE_ID).await;

    let change = Category {
        name: "Updated Name".to_owned(),
        ..system.clone()
    };
    let updated = th
        .app
        .update_category(change)
        .await
        .expect("the update is accepted");

    assert_eq!(updated.name, "Boards", "the name is not changed");
    assert_eq!(
        updated.category_type,
        CategoryType::System,
        "still a system one"
    );

    th.teardown().await;
}

/// Both system and custom categories can be collapsed and expanded.
#[tokio::test]
async fn update_category_should_be_allowed_to_collapse_and_expand_any_category_type() {
    let th = Setup::native().start().await;
    let user_id = stored_user(&th).await;
    let system = default_category(&th, &user_id, TEST_WORKSPACE_ID).await;
    assert!(!system.collapsed, "expanded to begin with");

    let change = Category {
        name: "Updated Name".to_owned(),
        collapsed: true,
        // The type of the request is ignored: the category stays a system one.
        category_type: CategoryType::Custom,
        ..system.clone()
    };
    let updated = th
        .app
        .update_category(change)
        .await
        .expect("the update is accepted");

    assert_eq!(
        updated.name, "Boards",
        "the name should not have been updated"
    );
    assert!(updated.collapsed, "collapsed now");
    assert_eq!(
        updated.category_type,
        CategoryType::System,
        "still a system one"
    );

    let expanded = th
        .app
        .update_category(Category {
            collapsed: false,
            ..updated
        })
        .await
        .expect("the update is accepted");
    assert!(!expanded.collapsed, "expanded again");

    th.teardown().await;
}

/// Deleting a custom category succeeds and returns it.
#[tokio::test]
async fn delete_category_base_case() {
    let th = Setup::native().start().await;
    let user_id = stored_user(&th).await;
    let system = default_category(&th, &user_id, TEST_WORKSPACE_ID).await;
    let created = th
        .app
        .create_category(category(TEST_WORKSPACE_ID, &user_id, "Category 1"))
        .await
        .expect("the category is created");

    let deleted = th
        .app
        .delete_category(&created.id, &user_id, TEST_WORKSPACE_ID)
        .await
        .expect("the owner deletes the category");

    assert_eq!(deleted.id, created.id, "the deleted category is returned");
    assert!(
        deleted.deleted_at.is_some_and(|at| at > 0),
        "marked as deleted"
    );
    let listed = th
        .app
        .user_category_containers(&user_id, TEST_WORKSPACE_ID)
        .await
        .expect("the categories are listed");
    assert_eq!(
        ids(&listed),
        [system.id.as_str()],
        "only the default is left"
    );

    th.teardown().await;
}

/// Deleting an already-deleted category returns not found.
#[tokio::test]
async fn delete_category_trying_to_delete_already_deleted_category() {
    let th = Setup::native().start().await;
    let user_id = stored_user(&th).await;
    let created = th
        .app
        .create_category(category(TEST_WORKSPACE_ID, &user_id, "Category 1"))
        .await
        .expect("the category is created");
    let deleted = th
        .app
        .delete_category(&created.id, &user_id, TEST_WORKSPACE_ID)
        .await
        .expect("the owner deletes the category");

    // A deleted category is gone: there is nothing to delete a second time, and the
    // answer does not give the category again.
    let error = th
        .app
        .delete_category(&created.id, &user_id, TEST_WORKSPACE_ID)
        .await
        .expect_err("there is no such category any more");
    assert_eq!(error.status(), StatusCode::NOT_FOUND, "{error}");
    assert!(
        deleted.deleted_at.is_some_and(|at| at > 0),
        "the first answer said when it was deleted"
    );
    let error = th
        .app
        .category(&created.id)
        .await
        .expect_err("it is not stored any more");
    assert_eq!(error.status(), StatusCode::NOT_FOUND, "{error}");

    th.teardown().await;
}

/// Deleting a system category is refused.
#[tokio::test]
async fn delete_category_trying_to_delete_system_category() {
    let th = Setup::native().start().await;
    let user_id = stored_user(&th).await;
    let system = default_category(&th, &user_id, TEST_WORKSPACE_ID).await;

    let error = th
        .app
        .delete_category(&system.id, &user_id, TEST_WORKSPACE_ID)
        .await
        .expect_err("a system category cannot be deleted");

    assert_eq!(error.status(), StatusCode::FORBIDDEN, "{error}");
    assert_eq!(
        error.to_string(),
        "a system category cannot be deleted",
        "the refusal"
    );
    let stored = th.app.category(&system.id).await.expect("it is stored");
    assert_eq!(stored.deleted_at, None, "the category is not deleted");

    th.teardown().await;
}

/// Deleting a custom category moves its boards into the existing default category.
///
/// The helper is private; deleting a category exercises it.
#[tokio::test]
async fn move_containers_to_default_category_when_default_category_already_exists() {
    let th = Setup::native().start().await;
    let user_id = stored_user(&th).await;
    let system = default_category(&th, &user_id, TEST_WORKSPACE_ID).await;
    let custom = th
        .app
        .create_category(category(TEST_WORKSPACE_ID, &user_id, "Custom Category 1"))
        .await
        .expect("the category is created");
    let board_ids_filed = open_boards(&th, TEST_WORKSPACE_ID, 2).await;
    th.app
        .add_update_user_category_container(
            TEST_WORKSPACE_ID,
            &user_id,
            &custom.id,
            &board_ids_filed,
        )
        .await
        .expect("the boards are filed");

    th.app
        .delete_category(&custom.id, &user_id, TEST_WORKSPACE_ID)
        .await
        .expect("the category is deleted");

    let listed = th
        .app
        .user_category_containers(&user_id, TEST_WORKSPACE_ID)
        .await
        .expect("the categories are listed");
    assert_eq!(
        ids(&listed),
        [system.id.as_str()],
        "only the default is left"
    );
    assert_eq!(
        sorted(board_ids(&listed[0])),
        sorted(board_ids_filed.iter().map(String::as_str).collect()),
        "the boards moved to the default category"
    );

    th.teardown().await;
}

/// Deleting a custom category creates the default category when absent and moves boards into it.
#[tokio::test]
async fn move_containers_to_default_category_when_default_category_doesnt_already_exists() {
    let th = Setup::native().start().await;
    let user_id = stored_user(&th).await;
    // Created without listing the categories first, so there is no default one yet. The
    // board is filed through the store for the same reason.
    let custom = th
        .app
        .create_category(category(TEST_WORKSPACE_ID, &user_id, "Custom Category 1"))
        .await
        .expect("the category is created");
    let board_id = open_board(&th, TEST_WORKSPACE_ID).await;
    th.app
        .store()
        .add_update_category_container(&user_id, &custom.id, std::slice::from_ref(&board_id))
        .await
        .expect("the board is filed");
    let stored = th
        .app
        .store()
        .get_user_categories(&user_id, TEST_WORKSPACE_ID)
        .await
        .expect("the categories are read");
    assert_eq!(stored.len(), 1, "only the custom category: {stored:?}");

    th.app
        .delete_category(&custom.id, &user_id, TEST_WORKSPACE_ID)
        .await
        .expect("the category is deleted");

    let listed = th
        .app
        .user_category_containers(&user_id, TEST_WORKSPACE_ID)
        .await
        .expect("the categories are listed");
    assert_eq!(names(&listed), ["Boards"], "a default category was created");
    assert_eq!(
        listed[0].category.category_type,
        CategoryType::System,
        "as a system category"
    );
    assert_eq!(
        board_ids(&listed[0]),
        [board_id.as_str()],
        "and got the board"
    );

    th.teardown().await;
}

/// Three categories of the user in [`TEST_WORKSPACE_ID`]: the default one and two custom
/// ones, in the order they are listed.
async fn three_categories(th: &TestServer, user_id: &str) -> Vec<String> {
    default_category(th, user_id, TEST_WORKSPACE_ID).await;
    for name in ["Category 2", "Category 3"] {
        th.app
            .create_category(category(TEST_WORKSPACE_ID, user_id, name))
            .await
            .expect("the category is created");
    }
    let listed = th
        .app
        .user_category_containers(user_id, TEST_WORKSPACE_ID)
        .await
        .expect("the categories are listed");
    assert_eq!(
        names(&listed),
        ["Category 3", "Category 2", "Boards"],
        "new categories go on top"
    );
    listed.into_iter().map(|entry| entry.category.id).collect()
}

async fn listed_ids(th: &TestServer, user_id: &str) -> Vec<String> {
    th.app
        .user_category_containers(user_id, TEST_WORKSPACE_ID)
        .await
        .expect("the categories are listed")
        .into_iter()
        .map(|entry| entry.category.id)
        .collect()
}

/// Reordering categories persists the requested order.
#[tokio::test]
async fn reorder_categories_base_case() {
    let th = Setup::native().start().await;
    let user_id = stored_user(&th).await;
    let listed = three_categories(&th, &user_id).await;
    let new_order = vec![listed[2].clone(), listed[0].clone(), listed[1].clone()];

    let answered = th
        .app
        .reorder_categories(&user_id, TEST_WORKSPACE_ID, new_order.clone())
        .await
        .expect("an order that names every category is accepted");

    assert_eq!(answered, new_order, "the new order is returned");
    assert_eq!(
        listed_ids(&th, &user_id).await,
        new_order,
        "and is the listed order"
    );

    th.teardown().await;
}

/// Reordering categories fails when not all existing categories are included.
#[tokio::test]
async fn reorder_categories_not_specifying_all_categories_should_fail() {
    let th = Setup::native().start().await;
    let user_id = stored_user(&th).await;
    let listed = three_categories(&th, &user_id).await;

    let error = th
        .app
        .reorder_categories(
            &user_id,
            TEST_WORKSPACE_ID,
            vec![listed[1].clone(), listed[0].clone()],
        )
        .await
        .expect_err("an order that leaves a category out is refused");

    assert_eq!(error.status(), StatusCode::BAD_REQUEST, "{error}");
    assert_eq!(
        listed_ids(&th, &user_id).await,
        listed,
        "the order is unchanged"
    );

    th.teardown().await;
}

/// Category reorder verification succeeds when the new order matches all existing categories.
///
/// The verification helper is private; this goes through the route that calls it.
#[tokio::test]
async fn verify_new_categories_match_existing_base_case() {
    let th = Setup::native().start().await.init_basic().await;
    let user = th.client.me().await;
    let default_id = category_boards(&th.client, ROOT_WORKSPACE_ID).await[0]
        .category
        .id
        .clone();
    let second = create_category(
        &th.client,
        &category(ROOT_WORKSPACE_ID, &user.id, "Category 2"),
    )
    .await;
    let third = create_category(
        &th.client,
        &category(ROOT_WORKSPACE_ID, &user.id, "Category 3"),
    )
    .await;
    let new_order = [second.id.as_str(), third.id.as_str(), default_id.as_str()];

    let response = th
        .client
        .put(&format!("{ROOT_WS}/category-order"), &new_order)
        .await;

    assert_eq!(
        response.ok_json::<Vec<String>>(),
        new_order,
        "the new order is answered"
    );
    let listed = category_boards(&th.client, ROOT_WORKSPACE_ID).await;
    assert_eq!(ids(&listed), new_order, "and is the listed order");

    th.teardown().await;
}

/// Category reorder verification fails when category counts differ.
#[tokio::test]
async fn verify_new_categories_match_existing_different_category_counts() {
    let th = Setup::native().start().await.init_basic().await;
    let user = th.client.me().await;
    category_boards(&th.client, ROOT_WORKSPACE_ID).await;
    let second = create_category(
        &th.client,
        &category(ROOT_WORKSPACE_ID, &user.id, "Category 2"),
    )
    .await;
    let third = create_category(
        &th.client,
        &category(ROOT_WORKSPACE_ID, &user.id, "Category 3"),
    )
    .await;
    let before = category_boards(&th.client, ROOT_WORKSPACE_ID).await;

    let response = th
        .client
        .put(
            &format!("{ROOT_WS}/category-order"),
            &[second.id.as_str(), third.id.as_str()],
        )
        .await;

    check_error(
        &response,
        StatusCode::BAD_REQUEST,
        "the new order must list every category once: it has 2, the user has 3",
    );
    let after = category_boards(&th.client, ROOT_WORKSPACE_ID).await;
    assert_eq!(ids(&after), ids(&before), "the order is unchanged");

    th.teardown().await;
}

// ---------------------------------------------------------------------------------------
// Application-layer category-boards tests

/// Listing categories creates a default Boards category populated with existing boards.
#[tokio::test]
async fn get_user_category_containers_user_had_no_default_category_and_had_boards() {
    let th = Setup::native().start().await.init_basic().await;
    let user = th.client.me().await;
    let mut boards = Vec::new();
    for _ in 0..3 {
        boards.push(board_with_admin(&th, ROOT_WORKSPACE_ID, &user.id).await.id);
    }

    let listed = category_boards(&th.client, ROOT_WORKSPACE_ID).await;

    assert_eq!(
        names(&listed),
        ["Boards"],
        "the default category was created"
    );
    assert_eq!(
        listed[0].container_metadata.len(),
        3,
        "with the three boards"
    );
    for board_id in &boards {
        assert!(
            listed[0].container_metadata.contains(&visible(board_id)),
            "board {board_id} is in it, not hidden"
        );
    }

    th.teardown().await;
}

/// Listing categories creates an empty default Boards category when the user has no boards.
#[tokio::test]
async fn get_user_category_containers_user_had_no_default_category_but_had_no_boards() {
    let th = Setup::native().start().await.init_basic().await;

    let response = th.client.get(&format!("{ROOT_WS}/categories")).await;

    let listed: Vec<CategoryContainers> = response.ok_json();
    assert_eq!(
        names(&listed),
        ["Boards"],
        "the default category was created"
    );
    assert!(listed[0].container_metadata.is_empty(), "with no boards");
    // The exact wire format of an entry.
    let user = th.client.me().await;
    let entry = &response.value()[0];
    assert_eq!(
        *entry,
        json!({
            "id": listed[0].category.id,
            "name": "Boards",
            "userId": user.id,
            "workspaceId": ROOT_WORKSPACE_ID,
            "createdAt": listed[0].category.created_at,
            "updatedAt": listed[0].category.created_at,
            "collapsed": false,
            "type": "system",
            "containerMetadata": [],
            // Preserved quirk: a category created at the top is stored one place down.
            "sortOrder": 10,
        }),
        "the entry"
    );

    th.teardown().await;
}

/// Listing categories returns the existing default Boards category and its boards unchanged.
#[tokio::test]
async fn get_user_category_containers_user_already_had_a_default_boards_category_with_boards_in_it()
{
    let th = Setup::native().start().await.init_basic().await;
    let user = th.client.me().await;
    board_with_admin(&th, ROOT_WORKSPACE_ID, &user.id).await;
    board_with_admin(&th, ROOT_WORKSPACE_ID, &user.id).await;
    let first = category_boards(&th.client, ROOT_WORKSPACE_ID).await;
    assert_eq!(
        first[0].container_metadata.len(),
        2,
        "two boards to begin with"
    );

    // A board that appears later is not filed by listing the categories.
    board_with_admin(&th, ROOT_WORKSPACE_ID, &user.id).await;
    let second = category_boards(&th.client, ROOT_WORKSPACE_ID).await;

    assert_eq!(second, first, "the same category with the same boards");

    th.teardown().await;
}

/// Default Boards category initialization when the user has neither implicit nor explicit board access.
///
/// The helper is private; listing the categories of a user who has none calls it.
#[tokio::test]
async fn create_containers_category_user_doesnt_have_any_boards_implicit_or_explicit() {
    let th = Setup::native().start().await;
    let user_id = stored_user(&th).await;

    let listed = th
        .app
        .user_category_containers(&user_id, TEST_WORKSPACE_ID)
        .await
        .expect("the categories are listed");

    assert_eq!(names(&listed), ["Boards"], "the default category");
    assert!(listed[0].container_metadata.is_empty(), "with no boards");
    assert_eq!(listed[0].category.user_id, user_id, "of the user");
    assert_eq!(
        listed[0].category.workspace_id, TEST_WORKSPACE_ID,
        "in the workspace"
    );

    th.teardown().await;
}

/// Default Boards category initialization when the user has implicit access to an open
/// board without explicit membership.
#[tokio::test]
async fn create_containers_category_user_has_implicit_access_to_some_board() {
    let th = Setup::native().start().await;
    let user_id = stored_user(&th).await;
    for _ in 0..3 {
        board_without_member(&th, TEST_WORKSPACE_ID, &th.stranger().await).await;
    }

    let listed = th
        .app
        .user_category_containers(&user_id, TEST_WORKSPACE_ID)
        .await
        .expect("the categories are listed");

    assert_eq!(names(&listed), ["Boards"], "the default category");
    assert!(
        listed[0].container_metadata.is_empty(),
        "boards the user is not a member of are not filed"
    );

    th.teardown().await;
}

/// Default Boards category initialization when the user has explicit board membership.
#[tokio::test]
async fn create_containers_category_user_has_explicit_access_to_some_board() {
    let th = Setup::native().start().await;
    let user_id = stored_user(&th).await;
    let mut boards = Vec::new();
    for _ in 0..3 {
        boards.push(board_with_admin(&th, TEST_WORKSPACE_ID, &user_id).await.id);
    }
    // A board of another workspace is not filed in this workspace's default category.
    board_with_admin(&th, ROOT_WORKSPACE_ID, &user_id).await;

    let listed = th
        .app
        .user_category_containers(&user_id, TEST_WORKSPACE_ID)
        .await
        .expect("the categories are listed");

    assert_eq!(names(&listed), ["Boards"], "the default category");
    assert_eq!(
        sorted(board_ids(&listed[0])),
        sorted(boards.iter().map(String::as_str).collect()),
        "the three boards the user is a member of"
    );
    // What was returned at creation is what is stored.
    let again = th
        .app
        .user_category_containers(&user_id, TEST_WORKSPACE_ID)
        .await
        .expect("the categories are listed");
    assert_eq!(ids(&again), ids(&listed), "the same category");
    assert_eq!(
        sorted(board_ids(&again[0])),
        sorted(board_ids(&listed[0])),
        "the same boards"
    );

    th.teardown().await;
}

/// Default Boards category initialization when the user has both implicit and explicit board access.
#[tokio::test]
async fn create_containers_category_user_has_both_implicit_and_explicit_access_to_some_board() {
    let th = Setup::native().start().await;
    let user_id = stored_user(&th).await;
    let own = board_with_admin(&th, TEST_WORKSPACE_ID, &user_id).await;
    board_without_member(&th, TEST_WORKSPACE_ID, &th.stranger().await).await;
    board_without_member(&th, TEST_WORKSPACE_ID, &th.stranger().await).await;

    let listed = th
        .app
        .user_category_containers(&user_id, TEST_WORKSPACE_ID)
        .await
        .expect("the categories are listed");

    assert_eq!(names(&listed), ["Boards"], "the default category");
    assert_eq!(
        board_ids(&listed[0]),
        [own.id.as_str()],
        "only the board the user is a member of"
    );

    th.teardown().await;
}

/// A category of the user in [`TEST_WORKSPACE_ID`] with `count` boards filed in it, and those
/// boards in the order they are listed.
async fn category_with_boards(
    th: &TestServer,
    user_id: &str,
    count: usize,
) -> (Category, Vec<String>) {
    let created = th
        .app
        .create_category(category(TEST_WORKSPACE_ID, user_id, "Category 1"))
        .await
        .expect("the category is created");
    let board_ids_filed = open_boards(th, TEST_WORKSPACE_ID, count).await;
    th.app
        .add_update_user_category_container(
            TEST_WORKSPACE_ID,
            user_id,
            &created.id,
            &board_ids_filed,
        )
        .await
        .expect("the boards are filed");
    let order = th
        .app
        .reorder_category_containers(user_id, TEST_WORKSPACE_ID, &created.id, board_ids_filed)
        .await
        .expect("the boards are put in order");
    (created, order)
}

async fn listed_board_ids(th: &TestServer, user_id: &str, category_id: &str) -> Vec<String> {
    let listed = th
        .app
        .user_category_containers(user_id, TEST_WORKSPACE_ID)
        .await
        .expect("the categories are listed");
    board_ids(entry(&listed, category_id))
        .into_iter()
        .map(str::to_owned)
        .collect()
}

/// Reordering boards within a category persists the new board order.
#[tokio::test]
async fn reorder_category_containers_base_case() {
    let th = Setup::native().start().await;
    let user_id = stored_user(&th).await;
    let (created, boards) = category_with_boards(&th, &user_id, 2).await;
    assert_eq!(
        listed_board_ids(&th, &user_id, &created.id).await,
        boards,
        "the order to begin with"
    );
    let reversed = vec![boards[1].clone(), boards[0].clone()];

    let new_order = th
        .app
        .reorder_category_containers(&user_id, TEST_WORKSPACE_ID, &created.id, reversed.clone())
        .await
        .expect("an order that names every board is accepted");

    assert_eq!(new_order, reversed, "the new order");
    assert_eq!(
        listed_board_ids(&th, &user_id, &created.id).await,
        reversed,
        "and is the listed order"
    );

    th.teardown().await;
}

/// Reordering boards within a category fails when not all boards in the category are listed.
#[tokio::test]
async fn reorder_category_containers_not_specifying_all_boards() {
    let th = Setup::native().start().await;
    let user_id = stored_user(&th).await;
    let (created, boards) = category_with_boards(&th, &user_id, 3).await;

    let error = th
        .app
        .reorder_category_containers(
            &user_id,
            TEST_WORKSPACE_ID,
            &created.id,
            vec![boards[1].clone(), boards[0].clone()],
        )
        .await
        .expect_err("an order that leaves a board out is refused");

    assert_eq!(error.status(), StatusCode::BAD_REQUEST, "{error}");
    assert_eq!(
        listed_board_ids(&th, &user_id, &created.id).await,
        boards,
        "the order is unchanged"
    );

    th.teardown().await;
}
