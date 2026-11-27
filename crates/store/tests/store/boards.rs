//! Integration tests for board storage, memberships, templates, trash, and administrator
//! invariants. Every user, workspace, and board is stored with a generated UUID identifier,
//! and malformed text names nothing.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use fvoci_model::PublicContainers;
use fvoci_model::{
    Block, BlockType, Body, Category, Container, ContainerKind, ContainerKinds, ContainerMember,
    ContainerPatch, ContainerVisibility, FileRecord, JsonMap, MinimumRole, PublicLinkChange,
    Subscription, new_id,
};
use fvoci_store::{BlockFilter, Joined, MemberChange, Purged, Store, StoreError};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::Barrier;
use tokio::task::JoinSet;

use crate::common;

mod concurrent;
mod members;
mod templates;
mod trash;

/// Text that is not an identifier, and so names no row.
pub(crate) const NOT_AN_ID: &str = "nonexistent-id";

/// A board with the three members nearly every test sets.
pub(crate) fn new_board(
    id: &str,
    workspace_id: &str,
    board_type: ContainerVisibility,
) -> Container {
    Container {
        id: id.to_owned(),
        workspace_id: workspace_id.to_owned(),
        visibility: board_type,
        ..Container::default()
    }
}

/// Stores an open board of a workspace, without members, and returns its identifier.
async fn stored_board(store: &Store, workspace_id: &str, user_id: &str) -> String {
    let mut board = new_board(&new_id(), workspace_id, ContainerVisibility::Open);
    store
        .insert_container(&mut board, user_id)
        .await
        .expect("the board is inserted");
    board.id
}

pub(crate) fn object(value: Value) -> JsonMap {
    serde_json::from_value(value).expect("the value is a JSON object")
}

fn sorted<const N: usize>(ids: [&str; N]) -> Vec<String> {
    let mut ids: Vec<String> = ids.into_iter().map(str::to_owned).collect();
    ids.sort();
    ids
}

pub(crate) fn sorted_board_ids<'a>(boards: impl IntoIterator<Item = &'a Container>) -> Vec<String> {
    let mut ids: Vec<String> = boards.into_iter().map(|board| board.id.clone()).collect();
    ids.sort();
    ids
}

fn sorted_by_id(mut boards: Vec<Container>) -> Vec<Container> {
    boards.sort_by(|a, b| a.id.cmp(&b.id));
    boards
}

fn sorted_user_ids(members: &[ContainerMember]) -> Vec<String> {
    let mut ids: Vec<String> = members
        .iter()
        .map(|member| member.user_id.clone())
        .collect();
    ids.sort();
    ids
}

#[tokio::test]
async fn get_container() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace(&test).await;

    // "existing board"
    {
        let mut board = new_board(&new_id(), &workspace_id, ContainerVisibility::Open);
        test.insert_container(&mut board, &user_id)
            .await
            .expect("the board is inserted");

        let r_board = test
            .get_container(&board.id)
            .await
            .expect("the board exists");
        assert_eq!(board.id, r_board.id, "the identifier");
        assert_eq!(board.workspace_id, r_board.workspace_id, "the workspace");
        assert_eq!(user_id, r_board.created_by, "the creator");
        assert_eq!(user_id, r_board.modified_by, "the modifier");
        assert_eq!(board.visibility, r_board.visibility, "the type");
        assert_ne!(r_board.created_at, 0, "the creation time is set");
        assert_ne!(r_board.updated_at, 0, "the modification time is set");
    }

    // "nonexisting board": an identifier no board has, and text that is no identifier.
    for board_id in [new_id(), NOT_AN_ID.to_owned()] {
        let error = test
            .get_container(&board_id)
            .await
            .expect_err("there is no such board");
        assert!(matches!(error, StoreError::NotFound(_)), "got {error:?}");
        assert!(
            error.is_not_found(),
            "Should be ErrNotFound compatible error"
        );
        assert_eq!(error.to_string(), "{boards} not found", "board {board_id}");
    }

    test.teardown().await;
}

#[tokio::test]
async fn get_containers_for_user_and_workspace() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let other_user_id = common::user(&test, "other-user").await;
    let workspace_id_1 = common::workspace_of(&test, &[&user_id, &other_user_id]).await;
    let workspace_id_2 = common::workspace_of(&test, &[&user_id, &other_user_id]).await;

    // "should return empty list if no results are found"
    {
        let boards = test
            .get_containers_for_user_and_workspace(
                &user_id,
                &workspace_id_1,
                PublicContainers::Include,
                ContainerKinds::BOARDS,
            )
            .await
            .expect("the query succeeds");
        assert!(boards.is_empty(), "got {boards:?}");
    }

    // Returns only the boards of the workspace that the user is a member of.
    {
        // workspace 1 boards
        let mut board1 = new_board(&new_id(), &workspace_id_1, ContainerVisibility::Open);
        test.insert_container_with_admin(&mut board1, &user_id)
            .await
            .expect("board 1 is inserted");

        let mut board2 = new_board(&new_id(), &workspace_id_1, ContainerVisibility::Private);
        test.insert_container_with_admin(&mut board2, &user_id)
            .await
            .expect("board 2 is inserted");

        let mut board3 = new_board(&new_id(), &workspace_id_1, ContainerVisibility::Open);
        test.insert_container(&mut board3, &other_user_id)
            .await
            .expect("board 3 is inserted");

        let mut board4 = new_board(&new_id(), &workspace_id_1, ContainerVisibility::Private);
        test.insert_container(&mut board4, &other_user_id)
            .await
            .expect("board 4 is inserted");

        // workspace 2 boards
        let mut board5 = new_board(&new_id(), &workspace_id_2, ContainerVisibility::Open);
        test.insert_container_with_admin(&mut board5, &user_id)
            .await
            .expect("board 5 is inserted");

        let mut board6 = new_board(&new_id(), &workspace_id_1, ContainerVisibility::Private);
        test.insert_container(&mut board6, &other_user_id)
            .await
            .expect("board 6 is inserted");

        // "should only find the two boards that the user is a member of for workspace 1 plus
        // the one open board"
        {
            let found = test
                .get_containers_for_user_and_workspace(
                    &user_id,
                    &workspace_id_1,
                    PublicContainers::Include,
                    ContainerKinds::BOARDS,
                )
                .await
                .expect("the query succeeds");
            assert_eq!(
                sorted_by_id(found),
                sorted_by_id(vec![board1.clone(), board2.clone(), board3]),
                "the user's boards of workspace 1 and its open board"
            );
        }

        // Only the two boards the user is a member of in workspace 1.
        {
            let found = test
                .get_containers_for_user_and_workspace(
                    &user_id,
                    &workspace_id_1,
                    PublicContainers::Exclude,
                    ContainerKinds::BOARDS,
                )
                .await
                .expect("the query succeeds");
            assert_eq!(
                sorted_by_id(found),
                sorted_by_id(vec![board1, board2]),
                "the user's boards of workspace 1"
            );
        }

        // Only the one board the user is a member of in workspace 2.
        {
            let found = test
                .get_containers_for_user_and_workspace(
                    &user_id,
                    &workspace_id_2,
                    PublicContainers::Include,
                    ContainerKinds::BOARDS,
                )
                .await
                .expect("the query succeeds");
            assert_eq!(found.len(), 1, "got {found:?}");
            assert_eq!(board5.id, found[0].id, "the user's board of workspace 2");
        }
    }

    test.teardown().await;
}

/// A table-driven case for fetching workspace boards by a list of IDs.
struct BoardsByIdsCase {
    name: &'static str,
    board_ids: [String; 2],
    expected_error: bool,
    expected_len: usize,
}

#[tokio::test]
async fn get_containers_in_workspace_by_ids() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace_of(&test, &[&user_id]).await;

    // "should return err not all found if one or more of the ids are not found"
    {
        let board_id_1 = new_id();
        let board_id_2 = new_id();
        for board_id in [&board_id_1, &board_id_2] {
            let mut board = new_board(board_id, &workspace_id, ContainerVisibility::Open);
            test.insert_container_with_admin(&mut board, &user_id)
                .await
                .expect("the board is inserted");
        }

        let cases = [
            BoardsByIdsCase {
                name: "if none of the IDs are found",
                // An identifier no board has, and text that is no identifier.
                board_ids: [new_id(), NOT_AN_ID.to_owned()],
                expected_error: true,
                expected_len: 0,
            },
            BoardsByIdsCase {
                name: "if not all of the IDs are found",
                board_ids: [new_id(), board_id_1.clone()],
                expected_error: true,
                expected_len: 1,
            },
            BoardsByIdsCase {
                name: "if all of the IDs are found",
                board_ids: [board_id_1, board_id_2],
                expected_error: false,
                expected_len: 2,
            },
        ];

        for case in cases {
            let board_ids = case.board_ids;
            let result = test
                .get_containers_in_workspace_by_ids(&board_ids, &workspace_id)
                .await;
            if case.expected_error {
                let error = result.expect_err(case.name);
                // A `Result` carries either the boards or the error, so how many of the
                // boards exist is verified with a separate query below.
                match &error {
                    StoreError::NotAllFound { entity, ids } => {
                        assert_eq!(*entity, "board", "{}", case.name);
                        assert_eq!(ids, &board_ids, "{}", case.name);
                    }
                    other => panic!("{}: got {other:?}", case.name),
                }
                assert!(
                    error.is_not_found(),
                    "Should be ErrNotFound compatible error"
                );
                let found = test
                    .get_containers_for_user_and_workspace(
                        &user_id,
                        &workspace_id,
                        PublicContainers::Include,
                        ContainerKinds::BOARDS,
                    )
                    .await
                    .expect("the query succeeds")
                    .iter()
                    .filter(|board| board_ids.contains(&board.id))
                    .count();
                assert_eq!(found, case.expected_len, "{}", case.name);
            } else {
                let boards = result.expect(case.name);
                assert_eq!(boards.len(), case.expected_len, "{}", case.name);
            }
        }
    }

    test.teardown().await;
}

#[tokio::test]
async fn insert_container() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let user_id_2 = common::user(&test, "user2").await;
    let workspace_id = common::workspace(&test).await;
    let public_board_id = new_id();

    // "valid public board" and "valid private board"
    for (id, visibility) in [
        (public_board_id.clone(), ContainerVisibility::Open),
        (new_id(), ContainerVisibility::Private),
    ] {
        let mut board = new_board(&id, &workspace_id, visibility);
        test.insert_container(&mut board, &user_id)
            .await
            .expect("the board is inserted");
        assert_eq!(board.id, id, "the identifier is kept");
        assert_eq!(board.visibility, visibility, "the visibility is kept");
        assert_ne!(board.created_at, 0, "the creation time is set");
        assert_ne!(board.updated_at, 0, "the modification time is set");
        assert_eq!(board.deleted_at, None, "the board is not deleted");
        assert_eq!(user_id, board.created_by, "the creator");
        assert_eq!(board.created_by, board.modified_by, "the modifier");
    }

    // "invalid properties field board"
    // Unserializable property values are prevented by type (`JsonMap`).

    // "update board"
    {
        let mut board = Container {
            id: public_board_id,
            workspace_id: workspace_id.clone(),
            title: "New title".to_owned(),
            ..Container::default()
        };
        test.insert_container(&mut board, &user_id_2)
            .await
            .expect("the board is updated");
        assert_eq!("New title", board.title, "the title is the new one");
        assert_eq!(user_id_2, board.modified_by, "the modifier");
    }

    // "test update board type"
    {
        let type_board_id = new_id();

        // A board must belong to a workspace: one without a workspace is refused, and
        // nothing is stored.
        let mut board = Container {
            id: type_board_id.clone(),
            title: "Public board".to_owned(),
            visibility: ContainerVisibility::Open,
            ..Container::default()
        };
        let error = test
            .insert_container(&mut board, &user_id)
            .await
            .expect_err("a board needs a workspace");
        assert!(
            matches!(&error, StoreError::InvalidId(invalid) if invalid.is_empty()),
            "got {error:?}"
        );
        let error = test
            .get_container(&type_board_id)
            .await
            .expect_err("nothing was stored");
        assert!(error.is_not_found(), "got {error:?}");

        board.workspace_id.clone_from(&workspace_id);
        test.insert_container(&mut board, &user_id)
            .await
            .expect("the board is inserted");
        assert_eq!(
            ContainerVisibility::Open,
            board.visibility,
            "the board is open"
        );

        let mut board_update = Container {
            id: type_board_id,
            workspace_id,
            visibility: ContainerVisibility::Private,
            ..Container::default()
        };
        test.insert_container(&mut board_update, &user_id)
            .await
            .expect("the board is updated");
        assert_eq!(
            ContainerVisibility::Private,
            board_update.visibility,
            "the board is private now"
        );
    }

    test.teardown().await;
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "sequential board-patching cases share one store fixture"
)]
async fn patch_container() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace(&test).await;

    // "should return error if the board doesn't exist"
    for board_id in [new_id(), NOT_AN_ID.to_owned()] {
        let patch = ContainerPatch {
            title: Some("A new title".to_owned()),
            ..ContainerPatch::default()
        };
        let error = test
            .patch_container(&board_id, &patch, &user_id)
            .await
            .expect_err("there is no such board");
        assert!(error.is_not_found(), "board {board_id}: got {error:?}");
    }

    // "should correctly apply a simple patch"
    {
        let board_id = new_id();
        let user_id_2 = common::user(&test, "user2").await;

        let mut board = Container {
            title: "A simple title".to_owned(),
            description: "A simple description".to_owned(),
            ..new_board(&board_id, &workspace_id, ContainerVisibility::Open)
        };
        test.insert_container(&mut board, &user_id)
            .await
            .expect("the board is inserted");
        assert_eq!(user_id, board.created_by, "the creator");

        let new_title = "A new title";
        let new_description = "A new description";
        let patch = ContainerPatch {
            title: Some(new_title.to_owned()),
            description: Some(new_description.to_owned()),
            ..ContainerPatch::default()
        };
        let patched_board = test
            .patch_container(&board_id, &patch, &user_id_2)
            .await
            .expect("the patch is applied");
        assert_eq!(new_title, patched_board.title, "the title is patched");
        assert_eq!(
            new_description, patched_board.description,
            "the description is patched"
        );
        assert_eq!(user_id, patched_board.created_by, "the creator stays");
        assert_eq!(user_id_2, patched_board.modified_by, "the modifier");
    }

    // "should correctly update the board properties"
    {
        let board_id = new_id();

        let mut board = Container {
            properties: object(json!({ "one": "1", "two": "2" })),
            ..new_board(&board_id, &workspace_id, ContainerVisibility::Open)
        };
        test.insert_container(&mut board, &user_id)
            .await
            .expect("the board is inserted");
        assert_eq!(board.properties["one"], json!("1"), "property one");
        assert_eq!(board.properties["two"], json!("2"), "property two");

        let patch = ContainerPatch {
            updated_properties: object(json!({ "three": "3" })),
            deleted_properties: vec!["one".to_owned()],
            ..ContainerPatch::default()
        };
        let patched_board = test
            .patch_container(&board_id, &patch, &user_id)
            .await
            .expect("the patch is applied");
        assert!(
            !patched_board.properties.contains_key("one"),
            "got {:?}",
            patched_board.properties
        );
        assert_eq!(patched_board.properties["two"], json!("2"), "property two");
        assert_eq!(
            patched_board.properties["three"],
            json!("3"),
            "property three"
        );
    }

    // "should correctly modify the board's type"
    {
        let board_id = new_id();

        let mut board = new_board(&board_id, &workspace_id, ContainerVisibility::Open);
        test.insert_container(&mut board, &user_id)
            .await
            .expect("the board is inserted");
        assert_eq!(
            board.visibility,
            ContainerVisibility::Open,
            "the board is open"
        );

        let patch = ContainerPatch {
            visibility: Some(ContainerVisibility::Private),
            ..ContainerPatch::default()
        };
        let patched_board = test
            .patch_container(&board_id, &patch, &user_id)
            .await
            .expect("the patch is applied");
        assert_eq!(
            ContainerVisibility::Private,
            patched_board.visibility,
            "the board is private now"
        );
    }

    // "a patch that doesn't include any of the properties should not modify them"
    {
        let board_id = new_id();
        let properties = object(json!({ "prop1": "val1" }));
        // Card property definitions require an `id`.
        let card_properties = vec![fvoci_model::CardPropertyDefinition::new(
            "prop2",
            "val2",
            fvoci_model::PropertyType::Text,
        )];

        let mut board = Container {
            properties: properties.clone(),
            card_properties: card_properties.clone(),
            ..new_board(&board_id, &workspace_id, ContainerVisibility::Open)
        };
        test.insert_container(&mut board, &user_id)
            .await
            .expect("the board is inserted");
        assert_eq!(
            board.visibility,
            ContainerVisibility::Open,
            "the board is open"
        );
        assert_eq!(properties, board.properties, "the properties");
        assert_eq!(
            card_properties, board.card_properties,
            "the card properties"
        );

        let patch = ContainerPatch {
            visibility: Some(ContainerVisibility::Private),
            ..ContainerPatch::default()
        };
        let patched_board = test
            .patch_container(&board_id, &patch, &user_id)
            .await
            .expect("the patch is applied");
        assert_eq!(
            ContainerVisibility::Private,
            patched_board.visibility,
            "the board is private now"
        );
        assert_eq!(
            properties, patched_board.properties,
            "the properties are unchanged"
        );
        assert_eq!(
            card_properties, patched_board.card_properties,
            "the card properties are unchanged"
        );
    }

    // "a patch that removes a card property and updates another should work correctly"
    patch_removes_one_card_property_and_updates_another(&test, &workspace_id, &user_id).await;

    test.teardown().await;
}

/// The last subtest of [`patch_container`]: a patch that removes one card property and updates another.
async fn patch_removes_one_card_property_and_updates_another(
    test: &common::TestStore,
    workspace_id: &str,
    user_id: &str,
) {
    let board_id = new_id();
    let prop1 =
        fvoci_model::CardPropertyDefinition::new("prop1", "val1", fvoci_model::PropertyType::Text);
    let prop2 =
        fvoci_model::CardPropertyDefinition::new("prop2", "val2", fvoci_model::PropertyType::Text);
    let prop3 =
        fvoci_model::CardPropertyDefinition::new("prop3", "val3", fvoci_model::PropertyType::Text);
    let card_properties = vec![prop1, prop2, prop3.clone()];

    let mut board = Container {
        card_properties: card_properties.clone(),
        ..new_board(&board_id, workspace_id, ContainerVisibility::Open)
    };
    test.insert_container(&mut board, user_id)
        .await
        .expect("the board is inserted");
    assert_eq!(
        board.visibility,
        ContainerVisibility::Open,
        "the board is open"
    );
    assert_eq!(
        card_properties, board.card_properties,
        "the card properties"
    );

    let new_prop1 = fvoci_model::CardPropertyDefinition::new(
        "prop1",
        "newval1",
        fvoci_model::PropertyType::Text,
    );
    let patch = ContainerPatch {
        updated_card_properties: vec![new_prop1.clone()],
        deleted_card_properties: vec!["prop2".to_owned()],
        ..ContainerPatch::default()
    };
    let patched_board = test
        .patch_container(&board_id, &patch, user_id)
        .await
        .expect("the patch is applied");
    // Compare definitions without regard to order.
    let by_property_id = |a: &fvoci_model::CardPropertyDefinition,
                          b: &fvoci_model::CardPropertyDefinition| {
        a.id.cmp(&b.id)
    };
    let mut expected = vec![new_prop1, prop3];
    expected.sort_by(by_property_id);
    let mut got = patched_board.card_properties;
    got.sort_by(by_property_id);
    assert_eq!(expected, got, "one definition updated, one removed");
}

#[tokio::test]
async fn delete_container() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace(&test).await;

    // "should return an error if the board doesn't exist"
    for board_id in [new_id(), NOT_AN_ID.to_owned()] {
        let error = test
            .delete_container(&board_id, &user_id)
            .await
            .expect_err("there is no such board");
        assert!(error.is_not_found(), "board {board_id}: got {error:?}");
    }

    // "should correctly delete the board"
    {
        let board_id = new_id();

        let mut board = new_board(&board_id, &workspace_id, ContainerVisibility::Open);
        test.insert_container(&mut board, &user_id)
            .await
            .expect("the board is inserted");

        test.get_container(&board_id)
            .await
            .expect("the board exists");

        test.delete_container(&board_id, &user_id)
            .await
            .expect("the board is deleted");

        let error = test
            .get_container(&board_id)
            .await
            .expect_err("the board is gone");
        assert!(
            error.is_not_found(),
            "Should be ErrNotFound compatible error"
        );
    }

    test.teardown().await;
}

#[tokio::test]
async fn undelete_container() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace(&test).await;

    // "existing id"
    {
        let board_id = new_id();

        let mut board = Container {
            title: "Dunder Mifflin Scranton".to_owned(),
            minimum_role: MinimumRole::Commenter,
            description: "Bears, beets, Battlestar Gallectica".to_owned(),
            icon: "🐻".to_owned(),
            show_description: true,
            is_template: false,
            properties: object(json!({ "prop_1": "value_1" })),
            card_properties: vec![fvoci_model::CardPropertyDefinition::new(
                "prop_1",
                "value_1",
                fvoci_model::PropertyType::Text,
            )],
            ..new_board(&board_id, &workspace_id, ContainerVisibility::Open)
        };
        test.insert_container(&mut board, &user_id)
            .await
            .expect("the board is inserted");

        test.delete_container(&board_id, &user_id)
            .await
            .expect("the board is deleted");

        test.get_container(&board_id)
            .await
            .expect_err("the board is gone");

        test.undelete_container(&board_id, &user_id)
            .await
            .expect("the board is restored");

        let board = test
            .get_container(&board_id)
            .await
            .expect("the board is back");

        // verifying the data after un-delete
        assert_eq!("Dunder Mifflin Scranton", board.title, "the title");
        assert_eq!(workspace_id, board.workspace_id, "the workspace");
        assert_eq!(user_id, board.created_by, "the creator");
        assert_eq!(user_id, board.modified_by, "the modifier");
        assert_eq!(
            MinimumRole::Commenter,
            board.minimum_role,
            "the minimum role"
        );
        assert_eq!(
            "Bears, beets, Battlestar Gallectica", board.description,
            "the description"
        );
        assert_eq!("🐻", board.icon, "the icon");
        assert!(board.show_description, "the description is shown");
        assert!(!board.is_template, "the board is no template");
        assert_eq!(board.properties["prop_1"], json!("value_1"), "a property");
        assert_eq!(1, board.card_properties.len(), "the card properties");
        assert_eq!(board.card_properties[0].name, "value_1", "a card property");
    }

    // "existing id multiple times"
    {
        let board_id = new_id();

        let mut board = new_board(&board_id, &workspace_id, ContainerVisibility::Open);
        test.insert_container(&mut board, &user_id)
            .await
            .expect("the board is inserted");

        test.delete_container(&board_id, &user_id)
            .await
            .expect("the board is deleted");

        test.get_container(&board_id)
            .await
            .expect_err("the board is gone");

        test.undelete_container(&board_id, &user_id)
            .await
            .expect("the board is restored");

        test.get_container(&board_id)
            .await
            .expect("the board is back");

        // Only a board currently in the trash can be restored.
        let error = test
            .undelete_container(&board_id, &user_id)
            .await
            .expect_err("the board is not in the trash any more");
        assert!(error.is_not_found(), "got {error:?}");

        test.get_container(&board_id)
            .await
            .expect("the board is still there");
    }

    // Restoring a non-existent or invalid board ID fails with not-found.
    for board_id in [new_id(), NOT_AN_ID.to_owned()] {
        let error = test
            .undelete_container(&board_id, &user_id)
            .await
            .expect_err("no such board is in the trash");
        assert!(error.is_not_found(), "board {board_id}: got {error:?}");

        test.get_container(&board_id)
            .await
            .expect_err("there is still no such board");
    }

    test.teardown().await;
}

#[tokio::test]
async fn insert_container_with_admin() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace_of(&test, &[&user_id]).await;

    // "should correctly create a board and the admin membership with the creator"
    {
        let board_id = new_id();

        let mut board = new_board(&board_id, &workspace_id, ContainerVisibility::Open);
        let new_member = test
            .insert_container_with_admin(&mut board, &user_id)
            .await
            .expect("the board and its admin are inserted");
        assert_eq!(user_id, board.created_by, "the creator");
        assert_eq!(user_id, board.modified_by, "the modifier");
        assert_eq!(user_id, new_member.user_id, "the member is the creator");
        assert_eq!(board_id, new_member.container_id, "the member's board");
        assert_eq!(
            new_member.role,
            fvoci_model::ContainerRole::Admin,
            "the creator administers the board"
        );

        // The membership that is returned is the one that is stored.
        let stored = test
            .get_member_for_container(&board_id, &user_id)
            .await
            .expect("the membership exists");
        assert_eq!(stored, new_member, "the stored membership");
    }

    test.teardown().await;
}

/// Verifies which board fields an update overwrites versus leaves untouched, returning
/// the board as stored.
#[tokio::test]
async fn insert_container_update_keeps_workspace_creator_and_creation_time() {
    let test = common::setup().await;
    let creator_id = common::user(&test, "creator").await;
    let editor_id = common::user(&test, "editor").await;
    let workspace_id = common::workspace(&test).await;
    let another_workspace_id = common::workspace(&test).await;
    let board_id = new_id();

    let mut board = new_board(&board_id, &workspace_id, ContainerVisibility::Open);
    test.insert_container(&mut board, &creator_id)
        .await
        .expect("the board is inserted");

    // Whatever the caller says about the workspace, the creator and the creation time: none
    // of it, another of each, and the same again.
    let claims = [
        (another_workspace_id.as_str(), "", 0),
        (another_workspace_id.as_str(), editor_id.as_str(), 12_345),
        (workspace_id.as_str(), creator_id.as_str(), board.created_at),
    ];
    for (claimed_workspace, claimed_creator, claimed_created_at) in claims {
        let mut update = Container {
            title: format!("Updated by a caller who claims {claimed_created_at}"),
            created_by: claimed_creator.to_owned(),
            created_at: claimed_created_at,
            ..new_board(&board_id, claimed_workspace, ContainerVisibility::Private)
        };
        test.insert_container(&mut update, &editor_id)
            .await
            .expect("the board is updated");
        assert_eq!(
            update.workspace_id, workspace_id,
            "the stored workspace comes back"
        );
        assert_eq!(
            update.created_by, creator_id,
            "the stored creator comes back, not {claimed_creator:?}"
        );
        assert_eq!(
            update.created_at, board.created_at,
            "the stored creation time comes back, not {claimed_created_at}"
        );
        assert_eq!(update.modified_by, editor_id, "the modifier");

        let stored = test
            .get_container(&board_id)
            .await
            .expect("the board exists");
        assert_eq!(stored.workspace_id, workspace_id, "the workspace stays");
        assert_eq!(stored.created_by, creator_id, "the creator stays");
        assert_eq!(
            stored.created_at, board.created_at,
            "the creation time stays"
        );
        assert_eq!(
            stored.visibility,
            ContainerVisibility::Private,
            "the type is the new one"
        );
        assert_eq!(update, stored, "the caller's board is the stored board");
    }

    test.teardown().await;
}

/// A board is not stored with a malformed card property definition, whether it is
/// inserted or patched, and the stored board stays as it was.
#[tokio::test]
async fn boards_with_malformed_card_property_definitions_are_not_stored() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace(&test).await;
    let board_id = new_id();
    let well_formed = vec![fvoci_model::CardPropertyDefinition::new(
        "p1",
        "Status",
        fvoci_model::PropertyType::Text,
    )];

    let mut malformed = Container {
        card_properties: vec![fvoci_model::CardPropertyDefinition::new(
            "",
            "no id",
            fvoci_model::PropertyType::Text,
        )],
        ..new_board(&board_id, &workspace_id, ContainerVisibility::Open)
    };
    let error = test
        .insert_container(&mut malformed, &user_id)
        .await
        .expect_err("a definition without an identifier");
    assert_eq!(
        error.to_string(),
        "card property definition 1 needs a non-empty string as id",
        "what is wrong with the definition"
    );
    assert!(
        test.get_container(&board_id)
            .await
            .unwrap_err()
            .is_not_found(),
        "nothing was stored"
    );

    let mut board = Container {
        card_properties: well_formed.clone(),
        ..new_board(&board_id, &workspace_id, ContainerVisibility::Open)
    };
    test.insert_container(&mut board, &user_id)
        .await
        .expect("a well-formed board is inserted");

    let patch = ContainerPatch {
        title: Some("renamed".to_owned()),
        updated_card_properties: vec![
            fvoci_model::CardPropertyDefinition::new("p2", "One", fvoci_model::PropertyType::Text),
            fvoci_model::CardPropertyDefinition::new("p2", "Two", fvoci_model::PropertyType::Text),
        ],
        ..ContainerPatch::default()
    };
    let error = test
        .patch_container(&board_id, &patch, &user_id)
        .await
        .expect_err("duplicate property id");
    assert!(
        error
            .to_string()
            .contains("two card property definitions have the id \"p2\""),
        "got {error}"
    );
    let stored = test.get_container(&board_id).await.unwrap();
    assert_eq!(
        stored.card_properties, well_formed,
        "the definitions are unchanged"
    );
    assert_ne!(stored.title, "renamed", "and so is the rest of the board");

    test.teardown().await;
}

// Trash behavior: deleted boards and blocks move to trash tables and can be restored or purged.

/// A membership that administers the board.
fn admin_of(board_id: &str, user_id: &str) -> ContainerMember {
    ContainerMember {
        container_id: board_id.to_owned(),
        user_id: user_id.to_owned(),
        role: fvoci_model::ContainerRole::Admin,
        synthetic: false,
        inherited_role: None,
    }
}

/// A membership that may edit the board and does not administer it.
fn editor_of(board_id: &str, user_id: &str) -> ContainerMember {
    ContainerMember {
        container_id: board_id.to_owned(),
        user_id: user_id.to_owned(),
        role: fvoci_model::ContainerRole::Editor,
        synthetic: false,
        inherited_role: None,
    }
}

/// A block of a board: on the block `parent_id`, or directly on the board if that is
/// empty.
fn new_block(board_id: &str, parent_id: &str, block_type: BlockType) -> Block {
    Block {
        id: new_id(),
        container_id: board_id.to_owned(),
        parent_id: parent_id.to_owned(),
        title: format!("a {}", block_type.as_str()),
        block_type,
        ..Block::default()
    }
}

fn sorted_blocks(mut blocks: Vec<Block>) -> Vec<Block> {
    blocks.sort_by(|a, b| a.id.cmp(&b.id));
    blocks
}

fn sorted_members(mut members: Vec<ContainerMember>) -> Vec<ContainerMember> {
    members.sort_by(|a, b| a.user_id.cmp(&b.user_id));
    members
}

/// The blocks of a board that exist, sorted by identifier.
async fn blocks_of(store: &Store, board_id: &str) -> Vec<Block> {
    let blocks = store
        .get_blocks(&BlockFilter::container(board_id))
        .await
        .expect("the blocks are read");
    sorted_blocks(blocks)
}

/// The memberships of a board, sorted by user.
async fn members_of(store: &Store, board_id: &str) -> Vec<ContainerMember> {
    let members = store
        .get_members_for_container(board_id)
        .await
        .expect("the members are read");
    sorted_members(members)
}

/// Subscribes a user to a card.
async fn subscribe(store: &Store, card_id: &str, user_id: &str) {
    let subscription = Subscription {
        block_type: BlockType::Card,
        block_id: card_id.to_owned(),
        user_id: user_id.to_owned(),
        notified_at: 0,
        created_at: 0,
        deleted_at: None,
    };
    store
        .create_subscription(subscription)
        .await
        .expect("the subscription is stored");
}

/// The identifiers of the blocks a user subscribes to, sorted.
async fn subscribed_block_ids(store: &Store, user_id: &str) -> Vec<String> {
    let mut block_ids: Vec<String> = store
        .get_subscriptions(user_id)
        .await
        .expect("the subscriptions are read")
        .into_iter()
        .map(|subscription| subscription.block_id)
        .collect();
    block_ids.sort();
    block_ids
}

/// A board with everything that hangs on one.
struct FullBoard {
    workspace_id: String,
    /// Made the board and administers it.
    admin_id: String,
    /// May edit the board, has it in a sidebar category and subscribes to its card.
    editor_id: String,
    board: Container,
    /// A view and a card on the board, a text on the card and a checkbox on the text,
    /// sorted by identifier.
    blocks: Vec<Block>,
    /// The identifier of the card.
    card_id: String,
    /// The memberships as they read while the board exists, sorted by user.
    members: Vec<ContainerMember>,
    file: FileRecord,
}

impl FullBoard {
    async fn store(store: &Store) -> Self {
        let admin_id = common::user(store, "admin").await;
        let editor_id = common::user(store, "editor").await;
        let workspace_id = common::workspace_of(store, &[&admin_id]).await;

        let mut board = Container {
            title: "Roadmap".to_owned(),
            description: "What comes when".to_owned(),
            icon: "🗺".to_owned(),
            show_description: true,
            minimum_role: MinimumRole::Commenter,
            properties: object(json!({ "color": "blue" })),
            card_properties: vec![fvoci_model::CardPropertyDefinition::new(
                "status",
                "Status",
                fvoci_model::PropertyType::Text,
            )],
            ..new_board(&new_id(), &workspace_id, ContainerVisibility::Private)
        };
        store
            .insert_container_with_admin(&mut board, &admin_id)
            .await
            .expect("the board and its administrator are stored");
        store
            .save_member(&editor_of(&board.id, &editor_id))
            .await
            .expect("the editor is stored");
        let members = members_of(store, &board.id).await;

        let view = new_block(&board.id, "", BlockType::View);
        let card = new_block(&board.id, "", BlockType::Card);
        let text = new_block(&board.id, &card.id, BlockType::Card);
        let checkbox = new_block(&board.id, &text.id, BlockType::Attachment);
        let card_id = card.id.clone();
        let mut blocks = vec![view, card, text, checkbox];
        store
            .insert_blocks(&mut blocks, &admin_id)
            .await
            .expect("the blocks are stored");

        let file = FileRecord {
            name: "diagram.png".to_owned(),
            created_at: board.created_at,
            ..file_of(&board.id, "7f3a9c.png")
        };
        store
            .save_file_record(&file)
            .await
            .expect("the file record is stored");

        store
            .set_public_link(&board.id, PublicLinkChange::On, "read-token", &admin_id)
            .await
            .expect("the board is shared");

        let category = Category {
            id: new_id(),
            name: "Plans".to_owned(),
            user_id: editor_id.clone(),
            workspace_id: workspace_id.clone(),
            created_at: board.created_at,
            updated_at: board.created_at,
            ..Category::default()
        };
        store
            .create_category(&category)
            .await
            .expect("the category is stored");
        store
            .add_update_category_container(
                &editor_id,
                &category.id,
                std::slice::from_ref(&board.id),
            )
            .await
            .expect("the board is filed under the category");

        subscribe(store, &card_id, &editor_id).await;

        Self {
            workspace_id,
            admin_id,
            editor_id,
            board,
            blocks: sorted_blocks(blocks),
            card_id,
            members,
            file,
        }
    }

    /// The boards the editor has filed in their one category.
    async fn filed_by_editor(&self, store: &Store) -> Vec<String> {
        let categories = store
            .get_user_category_containers(&self.editor_id, &self.workspace_id)
            .await
            .expect("the categories are read");
        assert_eq!(categories.len(), 1, "the category stays: {categories:?}");
        categories[0]
            .container_metadata
            .iter()
            .map(|metadata| metadata.container_id.clone())
            .collect()
    }

    /// The board is not shared and the subscribers to its card match `want`.
    async fn assert_sharing_gone_and_watchers(&self, store: &Store, want: &[&str]) {
        let err = store
            .get_public_link(&self.board.id)
            .await
            .expect_err("none");
        assert!(err.is_not_found(), "got {err:?}");
        let subs = store
            .get_subscribers_for_block(&self.card_id)
            .await
            .expect("subs");
        let mut got: Vec<&str> = subs.iter().map(|s| s.user_id.as_str()).collect();
        let mut expected = want.to_vec();
        got.sort_unstable();
        expected.sort_unstable();
        assert_eq!(got, expected, "card watchers");
    }
}

/// Stores a board with a card and a text on the card, and returns the identifiers of
/// the board, the card and the text.
async fn board_with_card_and_text(
    store: &Store,
    workspace_id: &str,
    user_id: &str,
) -> (String, String, String) {
    let board_id = stored_board(store, workspace_id, user_id).await;
    let card = new_block(&board_id, "", BlockType::Card);
    let text = new_block(&board_id, &card.id, BlockType::Attachment);
    let (card_id, text_id) = (card.id.clone(), text.id.clone());
    store
        .insert_blocks(&mut [card, text], user_id)
        .await
        .expect("the blocks are stored");
    (board_id, card_id, text_id)
}

/// When a board in the trash was deleted, in milliseconds since the Unix epoch.
async fn deleted_at(store: &Store, board_id: &str) -> i64 {
    store
        .get_deleted_container(board_id)
        .await
        .expect("the board is in the trash")
        .container
        .deleted_at
        .expect("deletion time")
}

/// A file record of the board, not stored yet.
fn file_of(board_id: &str, stored_name: &str) -> FileRecord {
    FileRecord {
        id: new_id(),
        container_id: board_id.to_owned(),
        stored_name: stored_name.to_owned(),
        name: format!("uploaded as {stored_name}"),
        extension: ".png".to_owned(),
        size: 2048,
        path: format!("boards/{board_id}/{stored_name}"),
        created_at: 1,
        ..FileRecord::default()
    }
}

/// How many boards the tests of races go through.
pub(crate) const RACE_ROUNDS: usize = 25;
