//! Copies of boards and templates (`duplicateContainer`).

use fvoci_model::DuplicateAs;
use fvoci_model::{
    Block, BlockType, Category, Container, ContainerVisibility, ContainersAndBlocks, new_id,
};
use fvoci_store::BlockFilter;
use reqwest::StatusCode;

use crate::common::TEST_WS;
use crate::common::{ROOT_WORKSPACE_ID, Setup, TEST_WORKSPACE_ID, TestServer, UNKNOWN_ID};

use crate::boards::{
    add_member, another_workspace, board_with_admin, category_name_of, duplicate_board,
    duplicate_board_in, editor, get_board, insert_block, member, members_of, new_board, sorted_ids,
    view_block,
};

// -- duplicating a board --------------------------------------------------------------

/// Creating and duplicating an open board files the copy under the default category.
#[tokio::test]
async fn duplicate_container_create_and_duplicate_public_board() {
    let th = Setup::native().start().await.init_basic().await;
    let me = th.client.me().await;
    let user2 = th.client2.me().await;
    let board: Container = th
        .client
        .create_board(&new_board(
            TEST_WORKSPACE_ID,
            ContainerVisibility::Open,
            "Public board",
        ))
        .await
        .created_json();
    assert_eq!(board.created_by, me.id, "created by the session's user");
    insert_block(&th, view_block(&board.id), &me.id).await;
    th.client
        .add_member(&editor(&board.id, &user2.id))
        .await
        .check_ok();
    assert_eq!(members_of(&th, &board.id).await.len(), 2, "two members");

    // Duplicate the board
    let copy: ContainersAndBlocks = duplicate_board(
        &th.client,
        &board.id,
        &format!("?asTemplate=false&toWorkspace={TEST_WORKSPACE_ID}"),
    )
    .await
    .created_json();

    assert_eq!(copy.containers.len(), 1, "one board");
    assert_eq!(copy.blocks.len(), 1, "one block");
    let duplicate = &copy.containers[0];
    assert_eq!(
        duplicate.visibility,
        ContainerVisibility::Private,
        "Duplicated board should be private"
    );
    assert_ne!(duplicate.id, board.id, "under a new identifier");
    assert_eq!(copy.blocks[0].container_id, duplicate.id, "with its block");
    assert_eq!(copy.blocks[0].title, "View 1", "a copy of the view");

    let members = members_of(&th, &duplicate.id).await;
    assert_eq!(
        members.len(),
        1,
        "Duplicated board should only have one member"
    );
    assert_eq!(members[0].user_id, me.id, "the user who duplicated");
    assert_eq!(members[0].container_id, duplicate.id, "of the copy");
    assert_eq!(
        members[0].role,
        fvoci_model::ContainerRole::Admin,
        "as an admin"
    );

    assert_eq!(
        category_name_of(&th, &me.id, TEST_WORKSPACE_ID, &duplicate.id).await,
        Some("Boards".to_owned()),
        "the copy is in the default category, like its source"
    );
    th.teardown().await;
}

/// Duplicating a board from a custom category files the copy into the same category.
#[tokio::test]
async fn duplicate_container_create_and_duplicate_public_board_from_a_custom_category() {
    let th = Setup::native().start().await.init_basic().await;
    let me = th.client.me().await;
    let user2 = th.client2.me().await;
    let category = th
        .app
        .create_category(Category {
            name: "My Category".to_owned(),
            user_id: me.id.clone(),
            workspace_id: TEST_WORKSPACE_ID.to_owned(),
            ..Category::default()
        })
        .await
        .expect("the category is created");
    let board: Container = th
        .client
        .create_board(&new_board(
            TEST_WORKSPACE_ID,
            ContainerVisibility::Open,
            "Public board",
        ))
        .await
        .created_json();

    // move board to custom category
    th.app
        .add_update_user_category_container(
            TEST_WORKSPACE_ID,
            &me.id,
            &category.id,
            std::slice::from_ref(&board.id),
        )
        .await
        .expect("the board is moved to the custom category");
    insert_block(&th, view_block(&board.id), &me.id).await;
    th.client
        .add_member(&editor(&board.id, &user2.id))
        .await
        .check_ok();
    assert_eq!(members_of(&th, &board.id).await.len(), 2, "two members");

    // Duplicate the board
    let copy: ContainersAndBlocks = duplicate_board(
        &th.client,
        &board.id,
        &format!("?asTemplate=false&toWorkspace={TEST_WORKSPACE_ID}"),
    )
    .await
    .created_json();

    assert_eq!(copy.containers.len(), 1, "one board");
    assert_eq!(copy.blocks.len(), 1, "one block");
    let duplicate = &copy.containers[0];
    assert_eq!(
        duplicate.visibility,
        ContainerVisibility::Private,
        "Duplicated board should be private"
    );
    assert_eq!(duplicate.title, "Public board copy", "the title says so");

    let members = members_of(&th, &duplicate.id).await;
    assert_eq!(
        members.len(),
        1,
        "Duplicated board should only have one member"
    );
    assert_eq!(members[0].user_id, me.id, "the user who duplicated");
    assert_eq!(
        members[0].role,
        fvoci_model::ContainerRole::Admin,
        "as an admin"
    );

    // verify duplicated board is in the same custom category
    assert_eq!(
        category_name_of(&th, &me.id, TEST_WORKSPACE_ID, &duplicate.id).await,
        Some("My Category".to_owned()),
        "the copy is filed where its source is"
    );
    th.teardown().await;
}

/// Duplicating a board as a board files it in a category; duplicating as a template does not.
#[tokio::test]
async fn duplicate_container_base_case_and_as_template() {
    let th = Setup::native().start().await;
    let user = th.stranger_in(TEST_WORKSPACE_ID).await;
    let board = board_with_admin(
        &th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Open, "Source"),
        &user,
    )
    .await;
    insert_block(&th, view_block(&board.id), &user).await;

    let (copy, members) = th
        .app
        .duplicate_container(&board.id, &user, TEST_WORKSPACE_ID, DuplicateAs::Copy, None)
        .await
        .expect("the board is duplicated");
    assert_eq!(copy.containers.len(), 1, "one board");
    assert_eq!(copy.blocks.len(), 1, "one block");
    assert_eq!(members.len(), 1, "one membership");
    assert_eq!(
        category_name_of(&th, &user, TEST_WORKSPACE_ID, &copy.containers[0].id).await,
        Some("Boards".to_owned()),
        "a copy is filed under a category"
    );

    let (template, members) = th
        .app
        .duplicate_container(
            &board.id,
            &user,
            TEST_WORKSPACE_ID,
            DuplicateAs::Template,
            None,
        )
        .await
        .expect("the board is duplicated as a template");
    assert!(template.containers[0].is_template, "a template");
    assert_eq!(members.len(), 1, "one membership");
    assert_eq!(
        category_name_of(&th, &user, TEST_WORKSPACE_ID, &template.containers[0].id).await,
        None,
        "a template is filed nowhere"
    );
    th.teardown().await;
}

/// `duplicateContainer` of `board_id` by `client`, with an `Accept-Language` if one is
/// given: the title of the copy.
async fn copy_title(
    client: &crate::common::Client,
    board_id: &str,
    query: &str,
    accept_language: Option<&str>,
) -> String {
    let path = format!("{TEST_WS}/containers/{board_id}/duplicate{query}");
    let mut request = client.request(reqwest::Method::POST, &path);
    if let Some(accept_language) = accept_language {
        request = request.header(reqwest::header::ACCEPT_LANGUAGE, accept_language);
    }
    let copy: ContainersAndBlocks = crate::common::Client::send(request).await.created_json();
    copy.containers[0].title.clone()
}

/// The words a copy's title is given are in the language of the account's settings.
/// Where the settings leave the language to the device, or name one the server writes
/// nothing in, they are in the language the request accepts first, and failing both in
/// English. A board made from a template keeps the template's title in any language.
#[tokio::test]
async fn a_copy_is_titled_in_the_language_of_whoever_asked() {
    const SETTINGS: &str = "/users/me/settings";
    let th = Setup::native().start().await.init_basic().await;
    let spec = new_board(TEST_WORKSPACE_ID, ContainerVisibility::Open, "로드맵");
    let board: Container = th.client.create_board(&spec).await.created_json();
    let wiki = crate::search_pages::new_wiki(&th.client).await;
    let wiki_title: Container = th
        .client
        .patch(
            &format!("{TEST_WS}/containers/{wiki}"),
            &serde_json::json!({"title": "Handbook"}),
        )
        .await
        .json();
    assert_eq!(wiki_title.title, "Handbook", "the wiki has a title");
    let set_language = async |language: serde_json::Value| {
        th.client
            .patch(SETTINGS, &serde_json::json!({"language": language}))
            .await
            .check_ok();
    };
    let template = "?asTemplate=true";

    // Nothing says a language: English.
    assert_eq!(
        copy_title(&th.client, &board.id, "", None).await,
        "로드맵 copy"
    );
    assert_eq!(
        copy_title(&th.client, &board.id, template, None).await,
        "New board template"
    );

    // The settings leave it to the device: what the request accepts.
    let korean_first = Some("ja, ko-KR;q=0.9, en;q=0.8");
    assert_eq!(
        copy_title(&th.client, &board.id, "", korean_first).await,
        "로드맵 사본"
    );
    assert_eq!(
        copy_title(&th.client, &board.id, template, korean_first).await,
        "새 보드 템플릿"
    );
    assert_eq!(
        copy_title(&th.client, &wiki, "", korean_first).await,
        "Handbook 사본",
        "a wiki is titled as a board is"
    );
    assert_eq!(
        copy_title(&th.client, &board.id, "", Some("ja, de;q=0.5")).await,
        "로드맵 copy",
        "no language of the server's is accepted"
    );

    // The settings name a language of the server's: the request's is not looked at.
    set_language("en-GB".into()).await;
    assert_eq!(
        copy_title(&th.client, &board.id, "", korean_first).await,
        "로드맵 copy"
    );
    set_language("ko".into()).await;
    assert_eq!(
        copy_title(&th.client, &board.id, "", Some("en")).await,
        "로드맵 사본"
    );
    assert_eq!(
        copy_title(&th.client, &wiki, "", None).await,
        "Handbook 사본"
    );
    let made: ContainersAndBlocks = duplicate_board(&th.client, &board.id, template)
        .await
        .created_json();
    assert_eq!(made.containers[0].title, "새 보드 템플릿");
    // A template of a template is a copy of its kind, and a board of a template keeps
    // the template's title.
    let made = &made.containers[0].id;
    assert_eq!(
        copy_title(&th.client, made, template, None).await,
        "새 보드 템플릿 사본"
    );
    assert_eq!(
        copy_title(&th.client, made, "", None).await,
        "새 보드 템플릿"
    );

    // The settings name a language the server writes nothing in: as left to the device.
    set_language("ja".into()).await;
    assert_eq!(
        copy_title(&th.client, &board.id, "", korean_first).await,
        "로드맵 사본"
    );
    assert_eq!(
        copy_title(&th.client, &board.id, "", None).await,
        "로드맵 copy"
    );

    // Another account's settings are its own.
    th.client
        .add_member(&editor(&board.id, &th.client2.me().await.id))
        .await
        .check_ok();
    set_language("ko".into()).await;
    assert_eq!(
        copy_title(&th.client2, &board.id, "", None).await,
        "로드맵 copy"
    );
    th.teardown().await;
}

/// An open board of the test workspace that the user administers, with a card titled
/// "Card 1" and a comment on that card.
async fn board_with_a_commented_card(th: &TestServer, user_id: &str) -> Container {
    let board = board_with_admin(
        th,
        new_board(TEST_WORKSPACE_ID, ContainerVisibility::Open, "Public board"),
        user_id,
    )
    .await;
    let card = insert_block(
        th,
        Block {
            id: new_id(),
            container_id: board.id.clone(),
            block_type: BlockType::Card,
            title: "Card 1".to_owned(),
            ..Block::default()
        },
        user_id,
    )
    .await;
    insert_block(
        th,
        Block {
            id: new_id(),
            container_id: board.id.clone(),
            parent_id: card.id,
            block_type: BlockType::Comment,
            title: "a comment".to_owned(),
            ..Block::default()
        },
        user_id,
    )
    .await;
    board
}

/// Permissions and query parameters (`asTemplate`, `toWorkspace`) when duplicating boards.
#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "every role against every parameter of the copy, on one board"
)]
async fn duplicate_container_permissions_and_parameters() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let user2 = th.client2.me().await;
    let board = board_with_a_commented_card(&th, &user1.id).await;

    duplicate_board(&th.anonymous(), &board.id, "")
        .await
        .check_unauthorized();
    for board_id in [UNKNOWN_ID, "nonexistent"] {
        duplicate_board(&th.client, board_id, "")
            .await
            .check_error(StatusCode::NOT_FOUND, "board not found");
    }

    // An open board that is not a template can only be duplicated by its members.
    let response = duplicate_board(&th.client2, &board.id, "").await;
    response.check_forbidden();
    assert_eq!(
        response.error_message(),
        "access denied to copy board",
        "why"
    );

    // Without `toWorkspace` the copy stays in the workspace of the source; comments are not
    // copied.
    let copy: ContainersAndBlocks = duplicate_board(&th.client, &board.id, "")
        .await
        .created_json();
    assert_eq!(
        copy.containers[0].workspace_id, TEST_WORKSPACE_ID,
        "the source's workspace"
    );
    assert_eq!(copy.containers[0].title, "Public board copy", "a copy");
    assert_eq!(copy.blocks.len(), 1, "the card without its comment");
    assert_eq!(copy.blocks[0].title, "Card 1", "the card");

    // `asTemplate=true` makes a template, which is filed under no category. Only the
    // exact value counts.
    let template: ContainersAndBlocks = duplicate_board(&th.client, &board.id, "?asTemplate=true")
        .await
        .created_json();
    assert!(template.containers[0].is_template, "a template");
    assert_eq!(
        template.containers[0].title, "New board template",
        "its title"
    );
    assert_eq!(
        category_name_of(
            &th,
            &user1.id,
            TEST_WORKSPACE_ID,
            &template.containers[0].id
        )
        .await,
        None,
        "not in a category"
    );
    let not_a_template: ContainersAndBlocks =
        duplicate_board(&th.client, &board.id, "?asTemplate=True")
            .await
            .created_json();
    assert!(
        !not_a_template.containers[0].is_template,
        "`True` is not `true`"
    );

    // `toWorkspace` puts the copy, and its category, into another workspace.
    let other_workspace = another_workspace(&th, &user1.id).await;
    let moved: ContainersAndBlocks = duplicate_board(
        &th.client,
        &board.id,
        &format!("?toWorkspace={other_workspace}"),
    )
    .await
    .created_json();
    assert_eq!(
        moved.containers[0].workspace_id, other_workspace,
        "the other workspace"
    );
    assert_eq!(
        category_name_of(&th, &user1.id, &other_workspace, &moved.containers[0].id).await,
        Some("Boards".to_owned()),
        "in the default category of that workspace"
    );

    // The target workspace must exist.
    for no_workspace in [UNKNOWN_ID, "other-workspace"] {
        duplicate_board(
            &th.client,
            &board.id,
            &format!("?toWorkspace={no_workspace}"),
        )
        .await
        .check_error(StatusCode::NOT_FOUND, "workspace not found");
    }

    // A viewer of a private board may duplicate it, and administers the copy.
    let private = board_with_admin(
        &th,
        new_board(
            TEST_WORKSPACE_ID,
            ContainerVisibility::Private,
            "Private one",
        ),
        &user1.id,
    )
    .await;
    insert_block(&th, view_block(&private.id), &user1.id).await;
    add_member(
        &th,
        &member(&private.id, &user2.id, fvoci_model::ContainerRole::Viewer),
    )
    .await;
    let copy: ContainersAndBlocks = duplicate_board(&th.client2, &private.id, "")
        .await
        .created_json();
    assert_eq!(
        copy.containers[0].created_by, user2.id,
        "created by the viewer"
    );
    let members = members_of(&th, &copy.containers[0].id).await;
    assert_eq!(members.len(), 1, "one member: {members:?}");
    assert_eq!(members[0].user_id, user2.id, "the viewer");
    assert_eq!(
        members[0].role,
        fvoci_model::ContainerRole::Admin,
        "administers the copy"
    );

    th.teardown().await;
}

/// Any workspace member can duplicate an open workspace template or built-in template,
/// whereas private templates require membership.
#[tokio::test]
async fn duplicate_container_from_a_template() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await;
    let user2 = th.client2.me().await;
    let mut templates = Vec::new();
    for (workspace_id, board_type, title) in [
        (
            TEST_WORKSPACE_ID,
            ContainerVisibility::Open,
            "Open template",
        ),
        (
            ROOT_WORKSPACE_ID,
            ContainerVisibility::Open,
            "Global template",
        ),
        (
            ROOT_WORKSPACE_ID,
            ContainerVisibility::Private,
            "Private template",
        ),
    ] {
        let template = board_with_admin(
            &th,
            Container {
                is_template: true,
                ..new_board(workspace_id, board_type, title)
            },
            &user1.id,
        )
        .await;
        insert_block(&th, view_block(&template.id), &user1.id).await;
        templates.push(template);
    }

    // user2 is not a member of any of them.
    let copy: ContainersAndBlocks = duplicate_board(
        &th.client2,
        &templates[0].id,
        &format!("?asTemplate=false&toWorkspace={TEST_WORKSPACE_ID}"),
    )
    .await
    .created_json();
    let board = &copy.containers[0];
    assert_eq!(board.title, "Open template", "the template's title");
    assert!(!board.is_template, "a board, not a template");
    assert_eq!(
        board.visibility,
        ContainerVisibility::Private,
        "private to its user"
    );
    assert_eq!(board.created_by, user2.id, "created by the user");
    assert_eq!(copy.blocks.len(), 1, "with the template's blocks");
    let read: Container = get_board(&th.client2, &board.id).await.ok_json();
    assert_eq!(&read, board, "the user reads their new board");
    let stored_blocks = th
        .app
        .store()
        .get_blocks(&BlockFilter::container(&board.id))
        .await
        .expect("the blocks are read");
    assert_eq!(
        stored_blocks.len(),
        copy.blocks.len(),
        "the blocks are stored"
    );
    assert_eq!(
        category_name_of(&th, &user2.id, TEST_WORKSPACE_ID, &board.id).await,
        Some("Boards".to_owned()),
        "filed under the user's default category"
    );

    // A template of a template keeps being one, and says "copy".
    let copy: ContainersAndBlocks =
        duplicate_board(&th.client2, &templates[0].id, "?asTemplate=true")
            .await
            .created_json();
    assert!(copy.containers[0].is_template, "a template");
    assert_eq!(copy.containers[0].title, "Open template copy", "a copy");

    // A built-in template copied into the user's workspace.
    let copy: ContainersAndBlocks = duplicate_board_in(
        &th.client2,
        ROOT_WORKSPACE_ID,
        &templates[1].id,
        &format!("?toWorkspace={TEST_WORKSPACE_ID}"),
    )
    .await
    .created_json();
    assert_eq!(
        copy.containers[0].workspace_id, TEST_WORKSPACE_ID,
        "in the user's workspace"
    );

    // A private template is for its members only; for others it does not exist.
    duplicate_board(
        &th.client2,
        &templates[2].id,
        &format!("?toWorkspace={TEST_WORKSPACE_ID}"),
    )
    .await
    .check_error(StatusCode::NOT_FOUND, "board not found");

    th.teardown().await;
}

/// A board without blocks is duplicated as a board without blocks.
#[tokio::test]
async fn duplicate_container_without_blocks() {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;

    let copy: ContainersAndBlocks = duplicate_board(&th.client, &board.id, "")
        .await
        .created_json();

    assert_eq!(copy.containers.len(), 1, "one board");
    assert!(copy.blocks.is_empty(), "no blocks");
    let boards: Vec<Container> = th.client.get(&format!("{TEST_WS}/boards")).await.ok_json();
    let mut expected = vec![board.id.clone(), copy.containers[0].id.clone()];
    expected.sort_unstable();
    assert_eq!(sorted_ids(&boards), expected, "the board and its copy");
    th.teardown().await;
}
