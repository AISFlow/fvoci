//! Joining and leaving a board, and what the application does when a board is patched or
//! filed.

use super::*;

// -- joining a board ------------------------------------------------------------------

/// A workspace user can join a public (open) board.
#[tokio::test]
async fn join_container_create_and_join_public_board() {
    let th = Setup::native().start().await.init_basic().await;
    let me = th.client.me().await;
    let user2 = th.client2.me().await;
    let board: Container = th
        .client
        .create_board(&new_board(
            TEST_WORKSPACE_ID,
            ContainerVisibility::Open,
            "Test Public board",
        ))
        .await
        .created_json();
    assert_eq!(board.created_by, me.id, "created by the session's user");
    assert_eq!(
        board.minimum_role,
        MinimumRole::Editor,
        "a board that names none starts with editor"
    );

    let joined: ContainerMember = join_board(&th.client2, &board.id).await.ok_json();

    assert_eq!(joined.container_id, board.id, "of the board");
    assert_eq!(joined.user_id, user2.id, "the joining user");
    assert_eq!(
        category_name_of(&th, &user2.id, TEST_WORKSPACE_ID, &board.id).await,
        Some("Boards".to_owned()),
        "the board is filed under the new member's default category"
    );

    // Joining again (or joining as the existing admin) returns the existing membership.
    let again: ContainerMember = join_board(&th.client2, &board.id).await.ok_json();
    assert_eq!(again, joined, "the same membership");
    let admin: ContainerMember = join_board(&th.client, &board.id).await.ok_json();
    assert_eq!(
        admin.role,
        ContainerRole::Admin,
        "the admin stays an admin: {admin:?}"
    );
    th.teardown().await;
}

/// Joining a public board assigns the role configured in `minimumRole` for every role value.
#[tokio::test]
async fn join_container_should_match_the_minimum_role_in_the_membership() {
    let th = Setup::native().start().await.init_basic().await;
    let user2 = th.client2.me().await;

    for (minimum_role, expected) in [
        (MinimumRole::Commenter, ContainerRole::Commenter),
        (MinimumRole::Viewer, ContainerRole::Viewer),
        (MinimumRole::Editor, ContainerRole::Editor),
    ] {
        let board: Container = th
            .client
            .create_board(&Container {
                minimum_role,
                ..new_board(
                    TEST_WORKSPACE_ID,
                    ContainerVisibility::Open,
                    "Public board with a role",
                )
            })
            .await
            .created_json();

        let joined: ContainerMember = join_board(&th.client2, &board.id).await.ok_json();

        assert_eq!(
            (joined.container_id.as_str(), joined.user_id.as_str()),
            (board.id.as_str(), user2.id.as_str()),
            "the membership of the user on the board"
        );
        assert_eq!(
            joined.role, expected,
            "joining a board whose minimum role is {minimum_role:?}"
        );
    }
    th.teardown().await;
}

/// Joining a public board whose `minimumRole` is unset assigns the editor role.
#[tokio::test]
async fn join_container_should_match_editor_role_when_the_board_names_no_minimum_role() {
    let th = Setup::native().start().await.init_basic().await;
    let user2 = th.client2.me().await;
    let board: Container = th
        .client
        .create_board(&new_board(
            TEST_WORKSPACE_ID,
            ContainerVisibility::Open,
            "Public board for editors",
        ))
        .await
        .created_json();

    let joined: ContainerMember = join_board(&th.client2, &board.id).await.ok_json();

    assert_eq!(
        joined.role,
        ContainerRole::Editor,
        "new member should be editor and nothing else"
    );
    assert_eq!(joined.user_id, user2.id, "the user who joined");
    th.teardown().await;
}

/// A member below the board's minimum role who joins again is raised to it. Joining
/// never lowers anybody: a member with the minimum role or a higher one stays as they
/// are, the administrator included.
#[tokio::test]
async fn joining_again_raises_to_the_minimum_role_and_never_lowers() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let user2 = th.client2.me().await;
    let board = board_with_admin(
        &th,
        Container {
            minimum_role: MinimumRole::Commenter,
            ..new_board(TEST_WORKSPACE_ID, ContainerVisibility::Open, "open")
        },
        &user1.id,
    )
    .await;
    let stored_role = async || {
        let members = members_of(&th, &board.id).await;
        let member = members
            .iter()
            .find(|member| member.user_id == user2.id)
            .expect("user2 is a member");
        member.role
    };

    // The administrator made user2 a viewer: below the board's minimum role.
    add_member(&th, &viewer(&board.id, &user2.id)).await;
    assert_eq!(
        stored_role().await,
        ContainerRole::Viewer,
        "as the administrator said"
    );
    let joined: ContainerMember = join_board(&th.client2, &board.id).await.ok_json();
    assert_eq!(
        joined.role,
        ContainerRole::Commenter,
        "raised by joining again"
    );
    assert_eq!(
        stored_role().await,
        ContainerRole::Commenter,
        "and stored so"
    );

    // With the minimum role: joining again changes nothing.
    let again: ContainerMember = join_board(&th.client2, &board.id).await.ok_json();
    assert_eq!(again.role, ContainerRole::Commenter, "as before");

    // Above it: an editor stays an editor.
    th.client
        .put(
            &format!("{TEST_WS}/containers/{}/members/{}", board.id, user2.id),
            &editor(&board.id, &user2.id),
        )
        .await
        .check_ok();
    let as_editor: ContainerMember = join_board(&th.client2, &board.id).await.ok_json();
    assert_eq!(as_editor.role, ContainerRole::Editor, "not lowered");
    assert_eq!(
        stored_role().await,
        ContainerRole::Editor,
        "nor in the store"
    );

    // The minimum role goes up: members below it stay where they are until they join
    // again, and then are raised.
    patch_board(&th.client, &board.id, &json!({"minimumRole": "viewer"}))
        .await
        .check_ok();
    th.client
        .put(
            &format!("{TEST_WS}/containers/{}/members/{}", board.id, user2.id),
            &viewer(&board.id, &user2.id),
        )
        .await
        .check_ok();
    patch_board(&th.client, &board.id, &json!({"minimumRole": "editor"}))
        .await
        .check_ok();
    assert_eq!(
        stored_role().await,
        ContainerRole::Viewer,
        "raising the minimum raises nobody"
    );
    th.client2
        .post(
            &format!("{TEST_WS}/boards/{}/cards", board.id),
            &json!({"title": "Card 1"}),
        )
        .await
        .check_forbidden();
    let raised: ContainerMember = join_board(&th.client2, &board.id).await.ok_json();
    assert_eq!(raised.role, ContainerRole::Editor, "joining again does");
    th.client2
        .post(
            &format!("{TEST_WS}/boards/{}/cards", board.id),
            &json!({"title": "Card 1"}),
        )
        .await
        .check_created();

    // The administrator joins: still the administrator.
    let admin: ContainerMember = join_board(&th.client, &board.id).await.ok_json();
    assert_eq!(
        admin.role,
        ContainerRole::Admin,
        "the administrator is not lowered: {admin:?}"
    );

    th.teardown().await;
}

/// The role that counts is the role a member has. An administrator can give a member
/// less than the board's minimum role, and the member then has less: the minimum role
/// is what joining gives, not something every member has anyway.
#[tokio::test]
async fn a_member_below_the_minimum_role_has_the_role_they_were_given() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let user2 = th.client2.me().await;
    // A private board cannot be joined, so the member cannot raise themself.
    let board = board_with_admin(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Private, "private"),
        &user1.id,
    )
    .await;
    assert_eq!(board.minimum_role, MinimumRole::Editor, "the default");

    add_member(&th, &viewer(&board.id, &user2.id)).await;

    th.client2
        .get(&format!("{TEST_WS}/boards/{}/cards", board.id))
        .await
        .check_ok();
    th.client2
        .post(
            &format!("{TEST_WS}/boards/{}/cards", board.id),
            &json!({"title": "Card 1"}),
        )
        .await
        .check_forbidden();
    join_board(&th.client2, &board.id)
        .await
        .check_error(StatusCode::FORBIDDEN, "cannot join a non Open board");

    th.teardown().await;
}

/// Templates cannot be joined: open templates are meant to be duplicated rather than joined.
#[tokio::test]
async fn join_container_refuses_a_template() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let template = board_with_admin(
        &th,
        Container {
            is_template: true,
            ..new_board(
                TEST_WORKSPACE_ID,
                ContainerVisibility::Open,
                "an open template",
            )
        },
        &user1.id,
    )
    .await;

    let response = join_board(&th.client2, &template.id).await;
    response.check_error(StatusCode::FORBIDDEN, "a template cannot be joined");
    assert_eq!(
        members_of(&th, &template.id).await.len(),
        1,
        "nobody joined"
    );
    // It can still be read and copied by who is no member.
    get_board(&th.client2, &template.id).await.check_ok();

    th.teardown().await;
}

/// Joining a private board is rejected.
#[tokio::test]
async fn join_container_create_and_join_private_board_should_not_succeed() {
    let th = Setup::native().start().await.init_basic().await;
    let board: Container = th
        .client
        .create_board(&new_board(
            TEST_WORKSPACE_ID,
            ContainerVisibility::Private,
            "Private board",
        ))
        .await
        .created_json();

    // `allow_admin` query flags do not bypass private board join checks.
    for query in ["", "?allow_admin", "?allow_admin=true"] {
        let response = th
            .client2
            .post_empty(&format!("{TEST_WS}/containers/{}/join{query}", board.id))
            .await;
        // A private board does not exist for who is no member, so there is nothing
        // to join.
        response.check_error(StatusCode::NOT_FOUND, "board not found");
        assert_eq!(response.problem_type(), "/problems/not-found", "{query}");
    }
    assert_eq!(members_of(&th, &board.id).await.len(), 1, "nobody joined");
    th.teardown().await;
}

/// Joining a nonexistent board returns 404.
#[tokio::test]
async fn join_container_join_invalid_board() {
    let th = Setup::native().start().await.init_basic().await;

    for board_id in [UNKNOWN_ID, "nonexistent-board-ID"] {
        let response = join_board(&th.client2, board_id).await;
        response.check_error(StatusCode::NOT_FOUND, "board not found");
    }
    th.teardown().await;
}

// -- leaving a board ------------------------------------------------------------------

/// Leaving a board: a regular member can leave, the last admin cannot, and a non-member is refused.
#[tokio::test]
async fn leave_container_cases() {
    let th = Setup::native().start().await.init_basic().await;
    let board = board_with_users(&th).await;

    leave_board(&th.anonymous(), &board.id)
        .await
        .check_unauthorized();
    for board_id in [UNKNOWN_ID, "nonexistent"] {
        let response = leave_board(&th.client, board_id).await;
        response.check_error(StatusCode::NOT_FOUND, "board not found");
    }
    let outsider = th.add_user("user3").await;
    leave_board(&outsider, &board.id).await.check_no_content();

    let response = leave_board(&th.client2, &board.id).await;
    response.check_no_content();
    assert_eq!(members_of(&th, &board.id).await.len(), 1, "user2 left");
    leave_board(&th.client2, &board.id).await.check_no_content();

    let response = leave_board(&th.client, &board.id).await;
    response.check_status(StatusCode::CONFLICT);
    assert_eq!(response.error_reason(), "last_admin", "the reason");
    assert_eq!(
        response.error_message(),
        "cannot leave a board with no admins",
        "the last admin stays"
    );

    // Memberships outlive a deleted board, but it cannot be left any more.
    join_board(&th.client2, &board.id).await.check_ok();
    th.client
        .delete(&format!("{TEST_WS}/containers/{}", board.id))
        .await
        .check_no_content();
    leave_board(&th.client2, &board.id).await.check_not_found();

    th.teardown().await;
}

// -- Application-layer patch_board test -----------------------------------------------

/// Application-layer `patch_container` updates a board's title through the real store.
#[tokio::test]
async fn app_patch_container_base_case_title_patch() {
    let th = Setup::native().start().await;
    let user = th.stranger_in(TEST_WORKSPACE_ID).await;
    let board = board_with_admin(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Open, "Title"),
        &user,
    )
    .await;

    let patched = th
        .app
        .patch_container(&title_patch("Patched Title"), &board.id, &user)
        .await
        .expect("the board is patched");

    assert_eq!(patched.title, "Patched Title", "the new title");
    assert_eq!(stored_board(&th, &board.id).await, patched, "and stored");
    th.teardown().await;
}

// -- Application-layer default category filing test -----------------------------------

/// A user who has custom categories but no default category gets the default category
/// created automatically when a new board needs filing.
#[tokio::test]
async fn board_category_no_boards_default_category_exists() {
    let th = Setup::native().start().await;
    let user = th.stranger_in(TEST_WORKSPACE_ID).await;
    for name in ["Category 1", "Category 2", "Category 3"] {
        th.app
            .create_category(Category {
                name: name.to_owned(),
                user_id: user.clone(),
                workspace_id: TEST_WORKSPACE_ID.to_owned(),
                ..Category::default()
            })
            .await
            .expect("the category is created");
    }

    let mut board_ids = Vec::new();
    for title in ["board 1", "board 2", "board 3"] {
        let board = board_with_admin(
            &th,
            new_board(TEST_WORKSPACE_ID, ContainerVisibility::Open, title),
            &user,
        )
        .await;
        board_ids.push(board.id);
    }

    let categories = categories_of(&th, &user, TEST_WORKSPACE_ID).await;
    assert_eq!(categories.len(), 4, "the default category was added");
    let default = categories
        .iter()
        .find(|entry| entry.category.name == "Boards")
        .expect("the default category exists");
    let mut filed: Vec<String> = default
        .container_metadata
        .iter()
        .map(|metadata| metadata.container_id.clone())
        .collect();
    filed.sort();
    board_ids.sort();
    assert_eq!(filed, board_ids, "the boards are in the default category");
    th.teardown().await;
}
