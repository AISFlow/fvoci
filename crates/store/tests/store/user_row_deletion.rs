//! Deleting a user row directly in the database: what cascades with the row, what foreign
//! keys refuse while the user is still named, and what comes back from the trash once the
//! row is gone.
//!
//! The store never deletes a user: an account that is closed is marked, and its row stays
//! for what the user made to point at. These tests delete the row past the store to pin
//! what the schema makes of that.

use std::collections::BTreeMap;

use fvoci_model::{
    Block, BlockType, Category, ClientStatePatch, Container, ContainerMember, ContainerPatch,
    Subscription, UserSettingsPatch, new_id,
};
use fvoci_store::{BlockFilter, Store};

use crate::common::{self, TestStore};
use crate::users::{has_session, open_session_under};

/// Deletes the row of a user on a connection of the test's own and returns how many
/// rows the statement deleted, or the database's refusal. The user's own space, which
/// the row does not go without, is removed first if it is there.
async fn delete_user_row(test: &TestStore, user_id: &str) -> Result<u64, sqlx::Error> {
    if !own_space_is_gone(test, user_id).await {
        common::remove_own_space(test, user_id).await;
    }
    delete_only_user_row(test, user_id).await
}

async fn own_space_is_gone(test: &TestStore, user_id: &str) -> bool {
    let query = format!("SELECT 'own' FROM organizations WHERE owner_id = '{user_id}'");
    let found = test.system().texts_unchecked(&query).await;
    found.expect("read").is_empty()
}

/// Deletes the row of a user and nothing before it.
async fn delete_only_user_row(test: &TestStore, user_id: &str) -> Result<u64, sqlx::Error> {
    common::try_execute_raw(
        &test.database_connection(),
        &format!("DELETE FROM users WHERE id = '{user_id}'"),
    )
    .await
}

/// A card directly on the board, not stored yet.
fn card_on(board_id: &str) -> Block {
    Block {
        id: new_id(),
        container_id: board_id.to_owned(),
        block_type: BlockType::Card,
        ..Block::default()
    }
}

/// The identifiers of the users who are members of the board.
async fn member_ids(store: &Store, board_id: &str) -> Vec<String> {
    store
        .get_members_for_container(board_id)
        .await
        .expect("the members of the board are listed")
        .into_iter()
        .map(|member| member.user_id)
        .collect()
}

/// Stores a session of the user under a digest that is the user's identifier. The user
/// is one [`common::user`] made: without a password hash.
async fn open_session(store: &Store, user_id: &str) {
    open_session_under(store, user_id, user_id, "").await;
}

/// Gives the user everything that is theirs alone: a session, a preference, a category
/// in the workspace with the board filed under it, a membership of the board and a
/// subscription to the block. Returns the identifier of the category.
async fn give_belongings(
    store: &Store,
    user_id: &str,
    workspace_id: &str,
    block: &Block,
) -> String {
    open_session(store, user_id).await;
    let client_state = ClientStatePatch {
        updated_fields: BTreeMap::from([("theme".to_owned(), "dark".to_owned())]),
        deleted_fields: Vec::new(),
    };
    store
        .patch_client_state(user_id, &client_state)
        .await
        .expect("the client state is stored");
    let settings = UserSettingsPatch::from_members(
        serde_json::json!({ "theme": "dark" })
            .as_object()
            .expect("object"),
    )
    .expect("valid settings");
    store
        .patch_user_settings(user_id, &settings)
        .await
        .expect("the settings are stored");
    let category = Category {
        id: new_id(),
        name: "Mine".to_owned(),
        user_id: user_id.to_owned(),
        workspace_id: workspace_id.to_owned(),
        ..Category::default()
    };
    store
        .create_category(&category)
        .await
        .expect("the category is created");
    store
        .add_update_category_container(
            user_id,
            &category.id,
            std::slice::from_ref(&block.container_id),
        )
        .await
        .expect("the board is filed under the category");
    let member = ContainerMember {
        container_id: block.container_id.clone(),
        user_id: user_id.to_owned(),
        role: fvoci_model::ContainerRole::Editor,
        synthetic: false,
        inherited_role: None,
    };
    store
        .save_member(&member)
        .await
        .expect("the user becomes a member of the board");
    store
        .create_subscription(Subscription {
            block_type: block.block_type,
            block_id: block.id.clone(),
            user_id: user_id.to_owned(),
            notified_at: 0,
            created_at: 0,
            deleted_at: None,
        })
        .await
        .expect("the user subscribes to the block");
    category.id
}

/// How many of the five kinds of belongings [`give_belongings`] gave the user are
/// there: the session, the preference, the category (found, and listed with the board
/// under it), the membership and the subscription.
async fn belongings_left(
    store: &Store,
    user_id: &str,
    workspace_id: &str,
    category_id: &str,
    block: &Block,
) -> Vec<&'static str> {
    let mut left = Vec::new();
    if has_session(store, user_id).await {
        left.push("session");
    }
    let client_state = store
        .get_client_state(user_id)
        .await
        .expect("the client state is read");
    let settings = store
        .get_user_settings(user_id)
        .await
        .expect("the settings are read");
    if !client_state.is_empty() || settings != fvoci_model::UserSettings::default() {
        left.push("preference");
    }
    let listed = store
        .get_user_category_containers(user_id, workspace_id)
        .await
        .expect("the categories are listed");
    let category = store.get_category(category_id).await;
    match (&category, listed.as_slice()) {
        (Ok(_), [only])
            if only.category.id == category_id
                && only.container_metadata.len() == 1
                && only.container_metadata[0].container_id == block.container_id =>
        {
            left.push("category");
        }
        (Err(error), []) if error.is_not_found() => {}
        _ => panic!("a category half there: found {category:?}, listed {listed:?}"),
    }
    let membership = store
        .get_member_for_container(&block.container_id, user_id)
        .await;
    let memberships = store
        .get_members_for_user(user_id)
        .await
        .expect("the memberships are listed");
    match (&membership, memberships.len()) {
        (Ok(_), 1) => left.push("membership"),
        (Err(error), 0) if error.is_not_found() => {}
        _ => panic!("a membership half there: found {membership:?}, listed {memberships:?}"),
    }
    let subscriptions = store
        .get_subscriptions(user_id)
        .await
        .expect("the subscriptions are listed");
    let subscribers = store
        .get_subscribers_for_block(&block.id)
        .await
        .expect("the subscribers are listed");
    let is_subscriber = subscribers
        .iter()
        .any(|subscriber| subscriber.user_id == user_id);
    match (subscriptions.len(), is_subscriber) {
        (1, true) => left.push("subscription"),
        (0, false) => {}
        _ => panic!("a subscription half there: {subscriptions:?}, subscribers {subscribers:?}"),
    }
    left
}

const ALL_BELONGINGS: [&str; 5] = [
    "session",
    "preference",
    "category",
    "membership",
    "subscription",
];

/// Deleting the row of a user takes what is theirs alone with it: their sessions,
/// preferences, categories with what is filed under them, memberships and
/// subscriptions. What is another user's stays, and so do the board and the block.
#[tokio::test]
async fn deleting_a_user_row_takes_what_is_theirs_alone_with_it() {
    let test = common::setup().await;
    let store = &test.store;
    let owner = common::user(store, "owner").await;
    let leaver = common::user(store, "leaver").await;
    let workspace_id = common::workspace_of(store, &[&owner, &leaver]).await;
    let boards = common::create_test_boards(store, &workspace_id, &owner, 1).await;
    let mut blocks = vec![card_on(&boards[0].id)];
    common::insert_blocks(store, &mut blocks, &owner).await;
    let block = &blocks[0];

    let of_owner = give_belongings(store, &owner, &workspace_id, block).await;
    let of_leaver = give_belongings(store, &leaver, &workspace_id, block).await;
    for (user_id, category_id) in [(&owner, &of_owner), (&leaver, &of_leaver)] {
        assert_eq!(
            belongings_left(store, user_id, &workspace_id, category_id, block).await,
            ALL_BELONGINGS,
            "before the deletion each user has everything"
        );
    }

    let error = delete_only_user_row(&test, &leaver)
        .await
        .expect_err("the row does not go while the user's own space is there");
    assert!(common::is_foreign_key_violation(&error), "got {error:?}");
    let deleted = delete_user_row(&test, &leaver)
        .await
        .expect("nothing that stays names the user: the row is deleted");
    assert_eq!(deleted, 1, "the one row of the user");

    let error = store
        .get_user_by_id(&leaver)
        .await
        .expect_err("the user is gone");
    assert!(error.is_not_found(), "got {error:?}");
    assert_eq!(
        belongings_left(store, &leaver, &workspace_id, &of_leaver, block).await,
        [""; 0],
        "everything that was the deleted user's alone went with the row"
    );
    assert_eq!(
        belongings_left(store, &owner, &workspace_id, &of_owner, block).await,
        ALL_BELONGINGS,
        "everything of the user who stays is there"
    );
    assert_eq!(
        member_ids(store, &block.container_id).await,
        std::slice::from_ref(&owner),
        "the board's other member stays"
    );
    let on_the_board = store
        .get_blocks(&BlockFilter::container(&block.container_id))
        .await
        .expect("the blocks of the board are listed");
    assert_eq!(
        common::sorted_ids(&on_the_board),
        std::slice::from_ref(&block.id),
        "the block the deleted user was subscribed to stays"
    );

    test.teardown().await;
}

/// The row of a user is not deleted while a board or a block names the user as its
/// creator or as the one who last changed it: the database refuses over the foreign
/// key, and the refused deletion takes nothing of the user's with it. A user whom
/// nothing names goes.
#[tokio::test]
async fn a_user_row_stays_while_a_board_or_a_block_names_the_user() {
    let test = common::setup().await;
    let store = &test.store;
    let board_creator = common::user(store, "board-creator").await;
    let board_modifier = common::user(store, "board-modifier").await;
    let block_creator = common::user(store, "block-creator").await;
    let block_modifier = common::user(store, "block-modifier").await;
    let bystander = common::user(store, "bystander").await;
    let workspace_id = common::workspace(store).await;

    let board = common::create_test_boards(store, &workspace_id, &board_creator, 1)
        .await
        .pop()
        .expect("one board was created");
    let retitled = ContainerPatch {
        title: Some("retitled".to_owned()),
        ..ContainerPatch::default()
    };
    let board: Container = store
        .patch_container(&board.id, &retitled, &board_modifier)
        .await
        .expect("the board is changed by another user");
    let mut block = card_on(&board.id);
    store
        .insert_block(&mut block, &block_creator)
        .await
        .expect("the block is created");
    block.title = "retitled".to_owned();
    store
        .insert_block(&mut block, &block_modifier)
        .await
        .expect("the block is changed by another user");
    assert_eq!(
        (
            board.created_by.as_str(),
            board.modified_by.as_str(),
            block.created_by.as_str(),
            block.modified_by.as_str(),
        ),
        (
            board_creator.as_str(),
            board_modifier.as_str(),
            block_creator.as_str(),
            block_modifier.as_str(),
        ),
        "each of the four users is named once"
    );
    let named = [
        (&board_creator, "the creator of a board"),
        (&board_modifier, "the last modifier of a board"),
        (&block_creator, "the creator of a block"),
        (&block_modifier, "the last modifier of a block"),
    ];
    for (user_id, _) in named {
        open_session(store, user_id).await;
    }
    open_session(store, &bystander).await;

    for (user_id, role) in named {
        let error = delete_user_row(&test, user_id)
            .await
            .expect_err("a user whom a board or a block names is not deleted");
        assert!(
            common::is_foreign_key_violation(&error),
            "{role}: refused over the foreign key: got {error:?}"
        );
        store
            .get_user_by_id(user_id)
            .await
            .unwrap_or_else(|error| panic!("{role} is still a user: {error:?}"));
        assert!(
            has_session(store, user_id).await,
            "{role}: the refused deletion did not take the user's session"
        );
    }

    let deleted = delete_user_row(&test, &bystander)
        .await
        .expect("a user whom nothing names is deleted");
    assert_eq!(deleted, 1, "the one row of the user");
    assert!(
        !has_session(store, &bystander).await,
        "the session went with its user"
    );

    // The board and the block are as they were, with the names of their users.
    let stored_board = store
        .get_container(&board.id)
        .await
        .expect("the board is found");
    assert_eq!(stored_board, board, "the board is unchanged");
    let stored_block = store
        .get_block(&block.id)
        .await
        .expect("the block is found");
    assert_eq!(stored_block, block, "the block is unchanged");

    test.teardown().await;
}

/// What is in the trash names its users without holding them: the row of a user goes
/// while a board or a block they made is there. What then comes back from the trash is
/// made by nobody, and the membership of the user who is gone does not come back.
#[tokio::test]
async fn what_comes_back_from_the_trash_forgets_a_creator_whose_row_is_gone() {
    let test = common::setup().await;
    let store = &test.store;
    let gone = common::user(store, "gone").await;
    let stays = common::user(store, "stays").await;
    let workspace_id = common::workspace(store).await;

    // A board the user made, with a block they made and both users as members; and a
    // block they made on a board of the user who stays.
    let board_of_gone = common::create_test_boards(store, &workspace_id, &gone, 1)
        .await
        .pop()
        .expect("one board was created");
    let board_of_stays = common::create_test_boards(store, &workspace_id, &stays, 1)
        .await
        .pop()
        .expect("one board was created");
    let mut blocks = vec![card_on(&board_of_gone.id), card_on(&board_of_stays.id)];
    common::insert_blocks(store, &mut blocks, &gone).await;
    let (with_its_board, on_its_own) = (&blocks[0], &blocks[1]);
    for user_id in [&gone, &stays] {
        let member = ContainerMember {
            container_id: board_of_gone.id.clone(),
            user_id: user_id.clone(),
            role: fvoci_model::ContainerRole::Admin,
            synthetic: false,
            inherited_role: None,
        };
        store
            .save_member(&member)
            .await
            .expect("the user becomes a member");
    }

    let refused = delete_user_row(&test, &gone)
        .await
        .expect_err("the board and the blocks hold the row of their creator");
    assert!(
        common::is_foreign_key_violation(&refused),
        "got {refused:?}"
    );

    store
        .delete_container(&board_of_gone.id, &stays)
        .await
        .expect("the board goes to the trash with its block");
    common::delete_blocks(store, std::slice::from_ref(on_its_own), &stays).await;
    let deleted = delete_user_row(&test, &gone)
        .await
        .expect("nothing that exists names the user: the row is deleted");
    assert_eq!(deleted, 1, "the one row of the user");

    let board = store
        .undelete_container(&board_of_gone.id, &stays)
        .await
        .expect("the board comes back although its creator is gone");
    assert_eq!(
        (board.created_by.as_str(), board.modified_by.as_str()),
        ("", stays.as_str()),
        "the board is made by nobody and restored by the user who stays"
    );
    let stored = store
        .get_container(&board.id)
        .await
        .expect("the restored board is found");
    assert_eq!(stored, board, "stored as returned");
    assert_eq!(
        member_ids(store, &board.id).await,
        std::slice::from_ref(&stays),
        "the membership of the user who is gone does not come back"
    );
    let came_with_it = store
        .get_block(&with_its_board.id)
        .await
        .expect("the block came back with its board");
    assert_eq!(
        (
            came_with_it.created_by.as_str(),
            came_with_it.modified_by.as_str()
        ),
        ("", stays.as_str()),
        "the block of the board is made by nobody"
    );

    let block = store
        .undelete_block(&on_its_own.id, &stays)
        .await
        .expect("the block comes back although its creator is gone");
    assert_eq!(
        (block.created_by.as_str(), block.modified_by.as_str()),
        ("", stays.as_str()),
        "the block is made by nobody and restored by the user who stays"
    );
    let stored = store
        .get_block(&block.id)
        .await
        .expect("the restored block is found");
    assert_eq!(stored, block, "stored as returned");

    test.teardown().await;
}
