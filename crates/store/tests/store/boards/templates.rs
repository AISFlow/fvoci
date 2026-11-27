//! Templates: the built-in ones, the ones a user may see, and the removal of the defaults.

use super::*;

// Additional store invariants: updates, deletion/trash, templates, and membership rules.

/// A template stored on behalf of no user is one the server ships with: it is built in,
/// whatever the caller says of it, and nothing else is. It stays built in whoever
/// writes it later.
#[tokio::test]
async fn a_template_stored_on_behalf_of_nobody_is_built_in_and_stays_so() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace(&test).await;

    let mut builtin = Container {
        title: "hello".to_owned(),
        is_template: true,
        ..new_board(&new_id(), &workspace_id, ContainerVisibility::Open)
    };
    test.insert_container(&mut builtin, "")
        .await
        .expect("the template is inserted");
    assert!(
        builtin.is_builtin,
        "a board stored on behalf of no user is built in"
    );
    assert_eq!(builtin.created_by, "", "nobody made it");
    assert!(
        builtin.properties.is_empty(),
        "and it has the properties it came with, none: {:?}",
        builtin.properties
    );
    let stored = test
        .get_container(&builtin.id)
        .await
        .expect("the template exists");
    assert_eq!(stored, builtin, "what was stored is the caller's board");

    // What a caller says about `is_builtin` does not count.
    let mut of_user = Container {
        title: "hello".to_owned(),
        is_template: true,
        is_builtin: true,
        ..new_board(&new_id(), &workspace_id, ContainerVisibility::Open)
    };
    test.insert_container(&mut of_user, &user_id)
        .await
        .expect("the template is inserted");
    assert!(!of_user.is_builtin, "a user's template is not built in");

    let mut board = Container {
        title: "hello".to_owned(),
        ..new_board(&new_id(), &workspace_id, ContainerVisibility::Open)
    };
    test.insert_container(&mut board, "")
        .await
        .expect("the board is inserted");
    assert!(!board.is_builtin, "only a template is built in");

    // A built-in template stays one whoever writes it later.
    let mut renamed = Container {
        title: "world".to_owned(),
        is_template: true,
        ..new_board(&builtin.id, &workspace_id, ContainerVisibility::Open)
    };
    test.insert_container(&mut renamed, &user_id)
        .await
        .expect("the template is updated");
    assert!(renamed.is_builtin, "the template is still built in");

    test.teardown().await;
}

/// Workspace template queries return open templates and templates the user is a member of;
/// built-in templates are queried separately.
#[tokio::test]
async fn get_template_boards_returns_open_templates_and_templates_the_user_is_member_of() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let other_user_id = common::user(&test, "other-user").await;
    let workspace_id = common::workspace_of(&test, &[&user_id, &other_user_id]).await;
    let another_workspace_id = common::workspace(&test).await;

    let mut open_template = Container {
        is_template: true,
        ..new_board(&new_id(), &workspace_id, ContainerVisibility::Open)
    };
    test.insert_container(&mut open_template, &other_user_id)
        .await
        .expect("the template is inserted");
    let mut own_private_template = Container {
        is_template: true,
        ..new_board(&new_id(), &workspace_id, ContainerVisibility::Private)
    };
    test.insert_container_with_admin(&mut own_private_template, &user_id)
        .await
        .expect("the template is inserted");
    let mut foreign_private_template = Container {
        is_template: true,
        ..new_board(&new_id(), &workspace_id, ContainerVisibility::Private)
    };
    test.insert_container_with_admin(&mut foreign_private_template, &other_user_id)
        .await
        .expect("the template is inserted");
    let mut other_workspace_template = Container {
        is_template: true,
        ..new_board(&new_id(), &another_workspace_id, ContainerVisibility::Open)
    };
    test.insert_container(&mut other_workspace_template, &user_id)
        .await
        .expect("the template is inserted");
    let mut builtin_template = Container {
        is_template: true,
        ..new_board(&new_id(), &workspace_id, ContainerVisibility::Open)
    };
    test.insert_container(&mut builtin_template, "")
        .await
        .expect("the template is inserted");
    let mut board = new_board(&new_id(), &workspace_id, ContainerVisibility::Open);
    test.insert_container_with_admin(&mut board, &user_id)
        .await
        .expect("the board is inserted");

    let templates = test
        .get_template_boards(&workspace_id, &user_id)
        .await
        .expect("the templates are read");
    assert_eq!(
        sorted_by_id(templates),
        sorted_by_id(vec![open_template.clone(), own_private_template]),
        "the open template of the workspace and the user's private one"
    );

    // No user is a member of nothing: the open templates are left. The built-in one,
    // open and of this workspace, is not among them either way.
    let templates = test
        .get_template_boards(&workspace_id, "")
        .await
        .expect("the templates are read");
    assert_eq!(templates, vec![open_template], "the open template");

    let builtin = test
        .get_builtin_templates()
        .await
        .expect("the built-in templates are read");
    assert_eq!(builtin, vec![builtin_template], "the built-in template");

    test.teardown().await;
}

/// Built-in templates are stored on behalf of no user, and `remove_builtin_templates`
/// permanently removes them and their blocks without sending them to the trash.
#[tokio::test]
async fn remove_default_templates_removes_system_templates_and_their_blocks_past_the_trash() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace(&test).await;

    let mut builtin_template = Container {
        is_template: true,
        ..new_board(&new_id(), &workspace_id, ContainerVisibility::Open)
    };
    test.insert_container(&mut builtin_template, "")
        .await
        .expect("the template is inserted");
    let mut user_template = Container {
        is_template: true,
        ..new_board(&new_id(), &workspace_id, ContainerVisibility::Open)
    };
    test.insert_container(&mut user_template, &user_id)
        .await
        .expect("the template is inserted");
    let mut plain_board = new_board(&new_id(), &workspace_id, ContainerVisibility::Open);
    test.insert_container(&mut plain_board, "")
        .await
        .expect("the board is inserted");

    let new_block_of = |board: &Container| Block {
        id: new_id(),
        container_id: board.id.clone(),
        parent_id: String::new(),
        ..Block::default()
    };
    let mut blocks = vec![
        new_block_of(&builtin_template),
        new_block_of(&user_template),
        new_block_of(&plain_board),
    ];
    common::insert_blocks(&test, &mut blocks, "").await;

    // Only a template stored on behalf of no user is built in. A board that is no
    // template is a board without a creator, and nothing else.
    let builtin = test
        .get_builtin_templates()
        .await
        .expect("the built-in templates are read");
    assert_eq!(
        sorted_board_ids(&builtin),
        sorted_board_ids([&builtin_template]),
        "the templates stored on behalf of no user"
    );

    test.remove_builtin_templates()
        .await
        .expect("the built-in templates are removed");

    let blocks_of = async |board: &Container| {
        test.get_blocks(&BlockFilter::container(&board.id))
            .await
            .expect("the blocks are read")
            .len()
    };
    let error = test
        .get_container(&builtin_template.id)
        .await
        .expect_err("the built-in template is gone");
    assert!(error.is_not_found(), "got {error:?}");
    assert_eq!(blocks_of(&builtin_template).await, 0, "and so is its block");

    test.get_container(&user_template.id)
        .await
        .expect("a template created by a user stays");
    assert_eq!(blocks_of(&user_template).await, 1, "and so does its block");

    // A non-template board is not built-in and stays intact with its blocks.
    let kept = test
        .get_container(&plain_board.id)
        .await
        .expect("a board without a creator that is no template stays");
    assert!(!kept.is_builtin, "it is not built in: {kept:?}");
    assert_eq!(blocks_of(&plain_board).await, 1, "and so does its block");
    let builtin = test
        .get_builtin_templates()
        .await
        .expect("the built-in templates are read");
    assert!(builtin.is_empty(), "got {builtin:?}");

    // Nothing is kept: neither the template nor its block is in the trash.
    let error = test
        .get_deleted_container(&builtin_template.id)
        .await
        .expect_err("the built-in template is not in the trash");
    assert!(error.is_not_found(), "got {error:?}");
    let error = test
        .get_deleted_block(&blocks[0].id)
        .await
        .expect_err("the block of the built-in template is not in the trash");
    assert!(error.is_not_found(), "got {error:?}");
    let error = test
        .undelete_container(&builtin_template.id, &user_id)
        .await
        .expect_err("there is nothing to restore");
    assert!(error.is_not_found(), "got {error:?}");

    test.teardown().await;
}
