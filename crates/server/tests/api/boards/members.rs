//! The members of a board through the routes and the application: listing, adding, changing
//! and removing them.

use super::*;

// -- the members of a board -----------------------------------------------------------

/// Unauthenticated `GET /boards/{board_id}/members` is rejected.
#[tokio::test]
async fn get_members_for_container_a_non_authenticated_user_should_be_rejected() {
    let th = Setup::native().start().await.init_basic().await;
    let board = board_with_users(&th).await;

    let response = get_members(&th.anonymous(), &board.id).await;

    response.check_unauthorized();
    th.teardown().await;
}

/// Listing members of a private board without membership returns 404, while unjoined
/// workspace members on an open board can read its members.
#[tokio::test]
async fn get_members_for_container_a_user_without_permissions_should_be_rejected() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let user2 = th.client2.me().await;
    let open = board_with_users(&th).await;
    th.app
        .delete_container_member(&open.id, &user2.id)
        .await
        .expect("user2 is removed");
    get_members(&th.client2, &open.id).await.check_ok();

    let private = board_with_admin(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Private, "private"),
        &user1.id,
    )
    .await;
    get_members(&th.client2, &private.id)
        .await
        .check_not_found();
    th.teardown().await;
}

/// Listing members of a nonexistent board returns 404.
#[tokio::test]
async fn get_members_for_container_non_existing_board() {
    let th = Setup::native().start().await.init_basic().await;

    for board_id in [UNKNOWN_ID, "non-existing-board"] {
        let response = get_members(&th.client, board_id).await;
        response.check_error(StatusCode::NOT_FOUND, "board not found");
    }
    th.teardown().await;
}

/// Returns board members as stored, without elevating any user to admin.
#[tokio::test]
async fn get_members_for_container_should_correctly_return_board_members_for_a_valid_board() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let user2 = th.client2.me().await;
    let board = board_with_users(&th).await;

    let response = get_members(&th.client, &board.id).await;
    let mut members: Vec<ContainerMember> = response.ok_json();
    members.sort_by_key(|member| member.user_id != user1.id);

    assert_eq!(
        members,
        vec![
            member(&board.id, &user1.id, ContainerRole::Admin),
            editor(&board.id, &user2.id),
        ],
        "the admin and the editor"
    );
    assert_eq!(
        members_of(&th, &board.id).await.len(),
        2,
        "the application layer answers the same members"
    );

    // The wire format of a membership.
    let first = response
        .value()
        .as_array()
        .and_then(|members| {
            members
                .iter()
                .find(|member| member["userId"] == json!(user2.id))
                .cloned()
        })
        .expect("user2 is listed");
    assert_eq!(
        first,
        json!({
            "containerId": board.id,
            "userId": user2.id,
            "role": "editor",
        }),
        "the members of a membership on the wire"
    );
    th.teardown().await;
}

/// `GET /users/me/memberships` returns memberships as stored; they move to trash with
/// their board and return when it is undeleted.
#[tokio::test]
async fn get_members_for_user_returns_the_memberships_as_stored() {
    let th = Setup::native().start().await.init_basic().await;
    let user2 = th.client2.me().await;
    let board = board_with_users(&th).await;

    let memberships: Vec<ContainerMember> = th.client2.get("/users/me/memberships").await.ok_json();
    assert_eq!(
        memberships,
        vec![editor(&board.id, &user2.id)],
        "an editor, not an admin"
    );

    th.app
        .delete_container(&board.id, &user2.id)
        .await
        .expect("the board is deleted");
    let memberships: Vec<ContainerMember> = th.client2.get("/users/me/memberships").await.ok_json();
    assert_eq!(memberships, [], "a deleted board has no members");

    th.app
        .undelete_container(&board.id, &user2.id)
        .await
        .expect("the board is restored");
    let memberships: Vec<ContainerMember> = th.client2.get("/users/me/memberships").await.ok_json();
    assert_eq!(
        memberships,
        vec![editor(&board.id, &user2.id)],
        "the membership is back with the board, as it was"
    );

    th.teardown().await;
}

// -- adding a member ------------------------------------------------------------------

/// Unauthenticated `POST /boards/{board_id}/members` is rejected.
#[tokio::test]
async fn add_member_a_non_authenticated_user_should_be_rejected() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let board = board_without_members(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Open, "title"),
        &th.stranger().await,
    )
    .await;

    let response = th
        .anonymous()
        .add_member(&editor(&board.id, &user1.id))
        .await;

    response.check_unauthorized();
    th.teardown().await;
}

/// Adding a member without permission is rejected.
#[tokio::test]
async fn add_member_a_user_without_permissions_should_be_rejected() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let board = board_without_members(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Private, "title"),
        &th.stranger().await,
    )
    .await;

    let response = th.client.add_member(&editor(&board.id, &user1.id)).await;

    // A private board the user is no member of does not exist for them.
    response.check_error(StatusCode::NOT_FOUND, "board not found");
    th.teardown().await;
}

/// Adding a member to a nonexistent board returns 404.
#[tokio::test]
async fn add_member_non_existing_board() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;

    for board_id in [UNKNOWN_ID, "non-existing-board-id"] {
        let response = th.client.add_member(&editor(board_id, &user1.id)).await;
        response.check_error(StatusCode::NOT_FOUND, "board not found");
    }
    th.teardown().await;
}

/// An admin can add a new member to a private board.
#[tokio::test]
async fn add_member_a_private_board_through_an_admin_user() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let user2 = th.client2.me().await;
    let board = board_with_admin(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Private, "title"),
        &user1.id,
    )
    .await;
    let new_member = editor(&board.id, &user2.id);

    let added: ContainerMember = th.client.add_member(&new_member).await.ok_json();

    assert_eq!(added, new_member, "the membership as asked for");
    assert_eq!(
        category_name_of(&th, &user2.id, TEST_WORKSPACE_ID, &board.id).await,
        Some("Boards".to_owned()),
        "the board is filed under the new member's default category"
    );
    th.teardown().await;
}

/// A workspace user who is not yet a member can add themselves to a public board.
#[tokio::test]
async fn add_member_a_public_board_through_a_user_that_is_not_yet_a_member() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let user2 = th.client2.me().await;
    let board = board_with_admin(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Open, "title"),
        &user1.id,
    )
    .await;
    let new_member = editor(&board.id, &user2.id);

    th.client2.add_member(&new_member).await.check_forbidden();
    let initial_members: Vec<ContainerMember> = get_members(&th.client2, &board.id).await.ok_json();
    assert_eq!(
        initial_members.len(),
        1,
        "unjoined member reads open board members"
    );

    // Join board - will become an editor
    let joined: ContainerMember = join_board(&th.client2, &board.id).await.ok_json();
    assert_eq!(joined, new_member, "joined as an editor");

    let added: ContainerMember = th.client2.add_member(&new_member).await.ok_json();
    assert_eq!(added, new_member, "the existing membership is answered");

    let members: Vec<ContainerMember> = get_members(&th.client2, &board.id).await.ok_json();
    assert_eq!(members.len(), 2, "two members: {members:?}");
    th.teardown().await;
}

/// Adding a new member assigns the requested board role.
#[tokio::test]
async fn add_member_should_always_add_a_new_member_as_given_board_role() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let user2 = th.client2.me().await;
    let board = board_with_admin(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Private, "title"),
        &user1.id,
    )
    .await;
    let new_member = member(&board.id, &user2.id, ContainerRole::Commenter);

    let added: ContainerMember = th.client.add_member(&new_member).await.ok_json();

    assert_eq!(added, new_member, "a commenter and nothing else");
    th.teardown().await;
}

/// Adding a user who is already a member returns the existing membership unchanged.
#[tokio::test]
async fn add_member_should_do_nothing_if_the_member_already_exists() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let board = board_with_admin(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Private, "title"),
        &user1.id,
    )
    .await;
    let members = members_of(&th, &board.id).await;
    assert_eq!(members.len(), 1, "one member: {members:?}");
    assert_eq!(members[0].role, ContainerRole::Admin, "an admin");

    let answered: ContainerMember = th
        .client
        .add_member(&editor(&board.id, &user1.id))
        .await
        .ok_json();

    assert_eq!(
        answered.role,
        ContainerRole::Admin,
        "the existing membership is answered: {answered:?}"
    );
    assert_eq!(
        members_of(&th, &board.id).await,
        members,
        "and nothing was changed"
    );
    th.teardown().await;
}

/// Body validation rules for `POST /boards/{board_id}/members`.
#[tokio::test]
async fn add_member_validates_the_body() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let user2 = th.client2.me().await;
    let board = board_with_admin(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Open, "title"),
        &user1.id,
    )
    .await;
    let path = format!("{TEST_WS}/containers/{}/members", board.id);

    for body in ["{bad", "[]", "null"] {
        th.client
            .send_raw_body(Method::POST, &path, body)
            .await
            .check_bad_request();
    }
    let response = th.client.post(&path, &json!({"role": "editor"})).await;
    response.check_bad_request();
    assert_eq!(response.error_message(), "empty userId", "no user named");

    // Only the user and the role are taken from the body; the board comes from the path.
    let added: ContainerMember = th
        .client
        .post(
            &path,
            &json!({
                "userId": user2.id,
                "containerId": "another-board",
                "role": "commenter",
                "roles": "x",
                "minimumRole": "admin",
                "synthetic": true,
            }),
        )
        .await
        .ok_json();
    assert_eq!(
        added,
        member(&board.id, &user2.id, ContainerRole::Commenter),
        "the rest of the body is ignored"
    );

    // The target user must exist.
    for no_user in [UNKNOWN_ID, "no-such-user"] {
        let response = th
            .client
            .post(&path, &json!({"userId": no_user, "role": "editor"}))
            .await;
        response.check_error(StatusCode::BAD_REQUEST, "no such user");
        assert_eq!(
            response.refused_at(),
            "/userId",
            "the member at fault: {no_user}"
        );
    }

    // A role manager must name a valid role; missing, null or unknown roles are refused.
    let user3 = th.add_user("user3").await.me().await;
    for bad in [
        json!({"userId": user3.id}),
        json!({"userId": user3.id, "role": null}),
        json!({"userId": user3.id, "role": ""}),
        json!({"userId": user3.id, "role": "owner"}),
    ] {
        let response = th.client.post(&path, &bad).await;
        response.check_bad_request();
        assert_eq!(response.refused_at(), "/role", "{bad}");
    }

    th.teardown().await;
}

/// Granting the admin role when adding a member requires permission to manage the
/// board's roles; an editor on an open board cannot elevate another account to admin.
#[tokio::test]
async fn add_member_as_admin_needs_the_permission_to_manage_roles() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let user2 = th.client2.me().await;
    let client3 = th.add_user("user3").await;
    let user3 = client3.me().await;
    let board = board_with_admin(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Open, "victim"),
        &user1.id,
    )
    .await;

    // user2 joins the open board, which makes them an editor.
    join_board(&th.client2, &board.id).await.check_ok();

    // The attack: the editor adds an accomplice as admin.
    let response = th
        .client2
        .add_member(&member(&board.id, &user3.id, ContainerRole::Admin))
        .await;
    response.check_forbidden();
    assert_eq!(
        response.error_message(),
        "access denied to modify board members",
        "expected refusal message for unauthorized role grant"
    );
    assert_eq!(
        members_of(&th, &board.id).await.len(),
        2,
        "the accomplice is not a member"
    );
    client3
        .delete(&format!("{TEST_WS}/containers/{}", board.id))
        .await
        .check_forbidden();
    delete_member(&client3, &board.id, &user1.id)
        .await
        .check_forbidden();

    // Naming an existing member with the admin role is refused as well, not answered
    // with that membership.
    th.client2
        .add_member(&member(&board.id, &user2.id, ContainerRole::Admin))
        .await
        .check_forbidden();

    th.teardown().await;
}

/// The counterpart of [`add_member_as_admin_needs_the_permission_to_manage_roles`]: an
/// editor of an open board still adds members, and an admin still adds admins.
#[tokio::test]
async fn add_member_by_an_editor_of_an_open_board_and_as_admin_by_an_admin_still_work() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let user3 = th.add_user("user3").await.me().await;
    let board = board_with_admin(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Open, "open"),
        &user1.id,
    )
    .await;
    join_board(&th.client2, &board.id).await.check_ok();

    let response = th.client2.add_member(&viewer(&board.id, &user3.id)).await;
    let added: ContainerMember = response.ok_json();
    assert_eq!(added.user_id, user3.id, "added: {added:?}");

    // An admin still adds admins.
    let client4 = th.add_user("user4").await;
    let user4 = client4.me().await;
    let added: ContainerMember = th
        .client
        .add_member(&member(&board.id, &user4.id, ContainerRole::Admin))
        .await
        .ok_json();
    assert_eq!(
        added.role,
        ContainerRole::Admin,
        "added as an admin: {added:?}"
    );

    th.teardown().await;
}

/// A member who may add members on an open board without managing roles always grants
/// the board's minimum role (editor unless configured otherwise), ignoring requested roles.
/// Both the response and stored membership reflect the granted role.
#[tokio::test]
async fn add_member_without_managing_roles_gives_the_minimum_role_of_the_board() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;

    // The empty one is the board that says nothing about its minimum role.
    let cases = [
        ("", ContainerRole::Editor),
        ("viewer", ContainerRole::Viewer),
        ("commenter", ContainerRole::Commenter),
        ("editor", ContainerRole::Editor),
    ];
    for (index, (minimum_role, expected)) in cases.into_iter().enumerate() {
        let board = board_with_admin(
            &th,
            new_board(TEST_WORKSPACE_ID, ContainerVisibility::Open, "open"),
            &user1.id,
        )
        .await;
        // The adder is an editor of the board whatever its minimum role.
        add_member(&th, &editor(&board.id, &th.client2.me().await.id)).await;
        if !minimum_role.is_empty() {
            th.client
                .patch(
                    &format!("{TEST_WS}/containers/{}", board.id),
                    &json!({"minimumRole": minimum_role}),
                )
                .await
                .check_ok();
        }
        let asked_for_editor = th.add_user(&format!("asked-editor-{index}")).await;
        let asked_for_nothing = th.add_user(&format!("asked-nothing-{index}")).await;
        let editor_id = asked_for_editor.me().await.id;
        let nothing_id = asked_for_nothing.me().await.id;
        let members_path = format!("{TEST_WS}/containers/{}/members", board.id);

        for (client, user_id, body) in [
            (
                &asked_for_editor,
                editor_id.as_str(),
                json!({"userId": editor_id, "role": "editor"}),
            ),
            (
                &asked_for_nothing,
                nothing_id.as_str(),
                json!({"userId": nothing_id}),
            ),
        ] {
            let added: ContainerMember = th.client2.post(&members_path, &body).await.ok_json();
            assert_eq!(
                added.role, expected,
                "minimum role {minimum_role:?}, body {body}: the answer"
            );
            let stored = members_of(&th, &board.id)
                .await
                .into_iter()
                .find(|member| member.user_id == user_id)
                .expect("the membership is stored");
            assert_eq!(
                stored.role, expected,
                "minimum role {minimum_role:?}: the stored membership"
            );
            // The role is real: only an editor may create a card.
            let insert = client
                .post(
                    &format!("{TEST_WS}/boards/{}/cards", board.id),
                    &json!({"title": "Card 1"}),
                )
                .await;
            if expected == ContainerRole::Editor {
                insert.check_created();
            } else {
                insert.check_forbidden();
            }
        }
    }
    th.teardown().await;
}

/// Who manages the board's roles still chooses them: an admin's request is stored as it
/// is, also on a board with a minimum role, and a request without a role is refused.
#[tokio::test]
async fn add_member_by_a_manager_of_roles_gives_the_roles_of_the_request() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let board = board_with_admin(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Open, "open"),
        &user1.id,
    )
    .await;
    th.client
        .patch(
            &format!("{TEST_WS}/containers/{}", board.id),
            &json!({"minimumRole": "commenter"}),
        )
        .await
        .check_ok();

    let user3 = th.add_user("user3").await.me().await;
    let added: ContainerMember = th
        .client
        .add_member(&editor(&board.id, &user3.id))
        .await
        .ok_json();
    assert_eq!(added.role, ContainerRole::Editor, "above the minimum role");

    let user4 = th.add_user("user4").await.me().await;
    let added: ContainerMember = th
        .client
        .add_member(&viewer(&board.id, &user4.id))
        .await
        .ok_json();
    assert_eq!(added.role, ContainerRole::Viewer, "below the minimum role");

    let user5 = th.add_user("user5").await.me().await;
    let refused = th
        .client
        .post(
            &format!("{TEST_WS}/containers/{}/members", board.id),
            &json!({"userId": user5.id}),
        )
        .await;
    refused.check_error(StatusCode::BAD_REQUEST, "missing role");
    assert_eq!(refused.refused_at(), "/role");

    th.teardown().await;
}

/// Member addition permissions: an editor may add members on an open board but not on a
/// private one; a commenter may add members on neither.
#[tokio::test]
async fn add_member_permissions_by_board_type_and_role() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let user2 = th.client2.me().await;
    let user3 = th.add_user("user3").await.me().await;
    let open = board_with_admin(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Open, "open"),
        &user1.id,
    )
    .await;
    let private = board_with_admin(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Private, "private"),
        &user1.id,
    )
    .await;

    // A commenter of an open board.
    add_member(&th, &member(&open.id, &user2.id, ContainerRole::Commenter)).await;
    th.client2
        .add_member(&editor(&open.id, &user3.id))
        .await
        .check_forbidden();

    // An editor of a private board.
    add_member(&th, &editor(&private.id, &user2.id)).await;
    th.client2
        .add_member(&editor(&private.id, &user3.id))
        .await
        .check_forbidden();

    // An editor of an open board.
    th.app
        .update_container_member(&editor(&open.id, &user2.id))
        .await
        .expect("user2 becomes an editor");
    th.client2
        .add_member(&editor(&open.id, &user3.id))
        .await
        .check_ok();

    th.teardown().await;
}

// -- Application-layer add_member tests -----------------------------------------------

/// Application-layer `add_member` creates a membership in the store.
#[tokio::test]
async fn add_member_to_container_base_case() {
    let th = Setup::native().start().await;
    let creator = th.stranger().await;
    let user = stored_user(&th, "member").await;
    let board = board_without_members(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Open, ""),
        &creator,
    )
    .await;
    let new_member = editor(&board.id, &user);

    let added = add_member(&th, &new_member).await;

    assert_eq!(added, new_member, "the membership as asked for");
    assert_eq!(
        members_of(&th, &board.id).await,
        vec![new_member],
        "and stored"
    );
    assert_eq!(
        category_name_of(&th, &user, TEST_WORKSPACE_ID, &board.id).await,
        Some("Boards".to_owned()),
        "the board is in the member's default category"
    );

    // A board that does not exist has no members to add to.
    for board_id in [UNKNOWN_ID, "no-such-board"] {
        let nothing = th
            .app
            .add_member_to_container(&editor(board_id, &user))
            .await
            .expect("not an error");
        assert_eq!(nothing, None, "nothing is answered for {board_id}");
    }

    th.teardown().await;
}

/// Application-layer `add_member` returns an existing non-synthetic membership unchanged.
#[tokio::test]
async fn add_member_to_container_return_existing_non_synthetic_membership_if_any() {
    let th = Setup::native().start().await;
    let creator = th.stranger().await;
    let user = stored_user(&th, "member").await;
    let board = board_without_members(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Open, ""),
        &creator,
    )
    .await;
    let existing = add_member(&th, &editor(&board.id, &user)).await;

    let answered = add_member(&th, &member(&board.id, &user, ContainerRole::Admin)).await;

    assert_eq!(answered, existing, "the existing membership, unchanged");
    assert_eq!(
        members_of(&th, &board.id).await,
        vec![existing],
        "and nothing was written"
    );
    th.teardown().await;
}

// -- changing a member ----------------------------------------------------------------

/// Unauthenticated `PUT /boards/{board_id}/members/{user_id}` is rejected.
#[tokio::test]
async fn update_member_a_non_authenticated_user_should_be_rejected() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let board = board_with_admin(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Open, "title"),
        &user1.id,
    )
    .await;

    let response = update_member(&th.anonymous(), &editor(&board.id, &user1.id)).await;

    response.check_unauthorized();
    th.teardown().await;
}

/// Updating a member without permission is rejected.
#[tokio::test]
async fn update_member_a_user_without_permissions_should_be_rejected() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let board = board_with_admin(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Open, "title"),
        &user1.id,
    )
    .await;

    let response = update_member(&th.client2, &editor(&board.id, &user1.id)).await;

    response.check_forbidden();
    assert_eq!(
        response.error_message(),
        "access denied to modify board members",
        "why"
    );
    th.teardown().await;
}

/// Updating a member on a nonexistent board returns 404.
#[tokio::test]
async fn update_member_non_existing_board() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;

    for board_id in [UNKNOWN_ID, "non-existent-board-id"] {
        let response = update_member(&th.client, &editor(board_id, &user1.id)).await;
        response.check_error(StatusCode::NOT_FOUND, "board not found");
    }
    th.teardown().await;
}

/// An admin can update an existing member's roles on a valid board.
#[tokio::test]
async fn update_member_should_correctly_update_a_member_for_a_valid_board() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let user2 = th.client2.me().await;
    let board = board_with_admin(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Open, "title"),
        &user1.id,
    )
    .await;
    let added = add_member(&th, &editor(&board.id, &user2.id)).await;
    assert_eq!(added.role, ContainerRole::Editor, "an editor");
    let update = member(&board.id, &user2.id, ContainerRole::Admin);

    let updated: ContainerMember = update_member(&th.client, &update).await.ok_json();

    assert_eq!(updated, update, "the new roles are answered");
    let stored = th
        .app
        .store()
        .get_member_for_container(&board.id, &user2.id);
    assert_eq!(
        stored.await.expect("the membership is read"),
        update,
        "and stored"
    );
    th.teardown().await;
}

/// Rejects demoting the last admin so a board is never left without an admin.
#[tokio::test]
async fn update_member_should_not_leave_a_board_without_any_admin() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let board = board_with_admin(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Open, "title"),
        &user1.id,
    )
    .await;

    let response = update_member(&th.client, &editor(&board.id, &user1.id)).await;

    // The request is fine; the state of the board stands against it.
    response.check_error(StatusCode::CONFLICT, "cannot leave a board with no admins");
    assert_eq!(response.error_reason(), "last_admin", "the reason");
    let members = members_of(&th, &board.id).await;
    assert_eq!(members.len(), 1, "one member: {members:?}");
    assert_eq!(members[0].role, ContainerRole::Admin, "still an admin");
    th.teardown().await;
}

/// The order of the checks of `PUT /boards/{board_id}/members/{user_id}` and what it
/// answers for a user who is not a member.
#[tokio::test]
async fn update_member_checks() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let user2 = th.client2.me().await;
    let board = board_with_admin(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Private, "title"),
        &user1.id,
    )
    .await;
    let path = format!("{TEST_WS}/containers/{}/members/{}", board.id, user2.id);

    // Non-members of a private board receive 404 regardless of body or target user.
    for target in [user2.id.as_str(), UNKNOWN_ID, "no-such-user"] {
        for body in ["{bad", "{\"role\":\"editor\"}"] {
            let response = th
                .client2
                .send_raw_body(
                    Method::PUT,
                    &format!("{TEST_WS}/containers/{}/members/{target}", board.id),
                    body,
                )
                .await;
            response.check_error(StatusCode::NOT_FOUND, "board not found");
        }
    }

    // Then the body.
    for body in [
        "{bad",
        "[]",
        "null",
        "{}",
        "{\"role\":null}",
        "{\"role\":\"owner\"}",
    ] {
        th.client
            .send_raw_body(Method::PUT, &path, body)
            .await
            .check_bad_request();
    }

    // Updating a user who is not a member (or does not exist) returns 404.
    for target in [user2.id.as_str(), UNKNOWN_ID, "no-such-user"] {
        let response = th
            .client
            .put(
                &format!("{TEST_WS}/containers/{}/members/{target}", board.id),
                &json!({"role": "editor"}),
            )
            .await;
        response.check_error(StatusCode::NOT_FOUND, "member not found");
    }
    assert_eq!(
        members_of(&th, &board.id).await.len(),
        1,
        "nobody was added"
    );

    // The user and the board come from the path, whatever the body names.
    add_member(&th, &editor(&board.id, &user2.id)).await;
    let updated: ContainerMember = th
        .client
        .put(
            &path,
            &json!({"userId": "ignored", "containerId": "ignored", "role": "admin"}),
        )
        .await
        .ok_json();
    assert_eq!(
        updated,
        member(&board.id, &user2.id, ContainerRole::Admin),
        "the path decides whose membership it is"
    );

    // With a second admin, the first may be demoted, and then no longer manages roles.
    update_member(&th.client, &editor(&board.id, &user1.id))
        .await
        .check_ok();
    update_member(&th.client, &editor(&board.id, &user2.id))
        .await
        .check_forbidden();
    // The remaining admin cannot demote themselves.
    update_member(&th.client2, &editor(&board.id, &user2.id))
        .await
        .check_status(StatusCode::CONFLICT);

    th.teardown().await;
}

// -- removing a member ----------------------------------------------------------------

/// Unauthenticated `DELETE /boards/{board_id}/members/{user_id}` is rejected.
#[tokio::test]
async fn delete_member_a_non_authenticated_user_should_be_rejected() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let board = board_with_admin(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Open, "title"),
        &user1.id,
    )
    .await;

    let response = delete_member(&th.anonymous(), &board.id, &user1.id).await;

    response.check_unauthorized();
    th.teardown().await;
}

/// Removing a member without permission is rejected.
#[tokio::test]
async fn delete_member_a_user_without_permissions_should_be_rejected() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let board = board_with_admin(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Open, "title"),
        &user1.id,
    )
    .await;

    let response = delete_member(&th.client2, &board.id, &user1.id).await;

    response.check_forbidden();
    assert_eq!(
        response.error_message(),
        "access denied to modify board members",
        "why"
    );
    th.teardown().await;
}

/// Removing a member from a nonexistent board returns 404.
#[tokio::test]
async fn delete_member_non_existing_board() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;

    for board_id in [UNKNOWN_ID, "non-existent-board-id"] {
        let response = delete_member(&th.client, board_id, &user1.id).await;
        response.check_error(StatusCode::NOT_FOUND, "board not found");
    }
    th.teardown().await;
}

/// Fixture: a private board with user1 as admin and user2 as editor, for member deletion tests.
async fn private_board_with_users(th: &TestServer) -> Container {
    let user1 = th.client.me().await;
    let user2 = th.client2.me().await;
    let board = board_with_admin(
        th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Private, "title"),
        &user1.id,
    )
    .await;
    let added = add_member(th, &editor(&board.id, &user2.id)).await;
    assert_eq!(added.role, ContainerRole::Editor, "an editor");
    assert_eq!(members_of(th, &board.id).await.len(), 2, "two members");
    board
}

/// An admin can remove another member from a private board.
#[tokio::test]
async fn delete_member_admin_removing_a_user() {
    let th = Setup::native().start().await.init_basic().await;
    let user2 = th.client2.me().await;
    let board = private_board_with_users(&th).await;

    let response = delete_member(&th.client, &board.id, &user2.id).await;

    response.check_no_content();
    assert_eq!(members_of(&th, &board.id).await.len(), 1, "one member left");
    th.teardown().await;
}

/// A user cannot remove themselves via `DELETE /members/{user_id}`; they must use the leave route.
#[tokio::test]
async fn delete_member_user_removing_themselves() {
    let th = Setup::native().start().await.init_basic().await;
    let user2 = th.client2.me().await;
    let board = private_board_with_users(&th).await;

    let response = delete_member(&th.client2, &board.id, &user2.id).await;

    response.check_forbidden();
    assert_eq!(members_of(&th, &board.id).await.len(), 2, "still two");
    th.teardown().await;
}

/// A non-admin member cannot remove another member from a board.
#[tokio::test]
async fn delete_member_a_non_admin_user_should_not_be_able_to_remove_another_user() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let board = private_board_with_users(&th).await;

    let response = delete_member(&th.client2, &board.id, &user1.id).await;

    response.check_forbidden();
    assert_eq!(members_of(&th, &board.id).await.len(), 2, "still two");
    th.teardown().await;
}

/// Rejects deleting the last admin so a board always keeps at least one admin.
#[tokio::test]
async fn delete_member_should_not_leave_a_board_without_any_admin() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let board = board_with_admin(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Private, "title"),
        &user1.id,
    )
    .await;

    let response = delete_member(&th.client, &board.id, &user1.id).await;

    // The request is fine; the state of the board stands against it.
    response.check_error(StatusCode::CONFLICT, "cannot leave a board with no admins");
    assert_eq!(response.error_reason(), "last_admin", "the reason");
    let members = members_of(&th, &board.id).await;
    assert_eq!(members.len(), 1, "one member: {members:?}");
    assert_eq!(members[0].role, ContainerRole::Admin, "still an admin");

    // Removing a user who is not a member is idempotent and succeeds.
    for no_member in [UNKNOWN_ID, "no-such-user"] {
        delete_member(&th.client, &board.id, no_member)
            .await
            .check_no_content();
    }
    th.teardown().await;
}
