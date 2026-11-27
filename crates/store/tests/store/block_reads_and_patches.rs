//! Reading and patching blocks: two patches at once, and the blocks directly on a board.

use std::sync::Arc;

use fvoci_model::{Block, BlockPatch, BlockType, new_id};
use fvoci_store::BlockFilter;
use serde_json::json;
use tokio::sync::Barrier;
use tokio::task::JoinSet;

use crate::blocks::{
    Fixture, RACE_ROUNDS, another_board, block, existing_block, fields, fixture, id_of,
    named_blocks, sorted_names, title_patch,
};
use crate::common;

/// A patch is applied to the block as the patch before it left it: of two patches at
/// once, one of the title and one of a field, both changes are there afterwards. Neither
/// writes back what it read of the other's part before the other wrote.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_patches_of_a_block_at_once_both_take_effect() {
    let test = common::setup().await;
    let store = &test.store;
    let Fixture {
        user_id, board_id, ..
    } = fixture(store).await;
    let mut card = Block {
        block_type: BlockType::Card,
        title: "before".to_owned(),
        fields: fields(json!({ "kept": "as it is", "mark": "before" })),
        ..block(&new_id(), &board_id)
    };
    store
        .insert_block(&mut card, &user_id)
        .await
        .expect("the card is inserted");

    for round in 0..2 * RACE_ROUNDS {
        let title = format!("title {round}");
        let mark = json!(format!("mark {round}"));
        let patches = [
            title_patch(&title),
            BlockPatch {
                updated_fields: fields(json!({ "mark": mark })),
                ..BlockPatch::default()
            },
        ];

        let start = Arc::new(Barrier::new(2));
        let mut racers = JoinSet::new();
        for patch in patches {
            let (store, start) = (store.clone(), Arc::clone(&start));
            let (card_id, user_id) = (card.id.clone(), user_id.clone());
            racers.spawn(async move {
                start.wait().await;
                store.patch_block(&card_id, &patch, &user_id).await
            });
        }
        for outcome in racers.join_all().await {
            outcome.unwrap_or_else(|error| panic!("round {round}: a patch failed: {error:?}"));
        }

        let stored = existing_block(store, &card.id).await;
        assert_eq!(
            (stored.title.as_str(), stored.fields.get("mark")),
            (title.as_str(), Some(&mark)),
            "round {round}: the title of one patch and the field of the other"
        );
        assert_eq!(
            stored.fields.get("kept"),
            Some(&json!("as it is")),
            "round {round}: what neither patch named is as it was"
        );
    }

    test.teardown().await;
}

/// The parent of a list is a block: the board named as the parent is no block, and
/// finds nothing, of its own blocks or another board's. A block as parent finds the
/// blocks directly below it.
#[tokio::test]
async fn get_blocks_with_the_board_as_parent_finds_nothing() {
    let test = common::setup().await;
    let store = &test.store;
    let Fixture {
        user_id,
        workspace_id,
        board_id,
    } = fixture(store).await;
    let other_board_id = another_board(store, &workspace_id, &user_id).await;

    let mut tree = named_blocks(
        &board_id,
        &[
            ("card", "", "card"),
            ("view", "", "view"),
            ("text", "card", "card"),
            ("checkbox", "text", "attachment"),
        ],
    );
    common::insert_blocks(store, &mut tree, &user_id).await;
    let mut elsewhere = named_blocks(&other_board_id, &[("other card", "", "card")]);
    common::insert_blocks(store, &mut elsewhere, &user_id).await;

    for (container, parent) in [
        (&board_id, &board_id),
        (&other_board_id, &other_board_id),
        (&board_id, &other_board_id),
    ] {
        let found = store
            .get_blocks(&BlockFilter::container(container).with_parent(parent))
            .await
            .expect("the query runs");
        assert!(found.is_empty(), "a board is no parent: {found:?}");
    }

    let on_the_card = store
        .get_blocks(&BlockFilter::container(&board_id).with_parent(id_of(&tree, "card")))
        .await
        .expect("the query runs");
    assert_eq!(
        sorted_names(&on_the_card),
        ["text"],
        "the block on the card"
    );

    test.teardown().await;
}

#[tokio::test]
async fn card_visibility_changes_for_type_and_trash_but_not_content() {
    let test = common::setup().await;
    let store = &test.store;
    let Fixture {
        user_id, board_id, ..
    } = fixture(store).await;
    let mut card = Block {
        block_type: BlockType::Card,
        ..block(&new_id(), &board_id)
    };
    store
        .insert_block(&mut card, &user_id)
        .await
        .expect("card inserted");
    let before = store.visibility_version().await;
    card.title = "New title".to_owned();
    store
        .insert_block(&mut card, &user_id)
        .await
        .expect("content changed");
    assert_eq!(
        before,
        store.visibility_version().await,
        "ordinary content preserves grants"
    );
    card.block_type = BlockType::View;
    let refused = store
        .insert_block(&mut card, &user_id)
        .await
        .expect_err("block_type is immutable once stored");
    assert!(
        matches!(
            refused,
            fvoci_store::StoreError::InvalidBlock {
                source: fvoci_model::BlockError::TypeChanges,
                ..
            }
        ),
        "expected TypeChanges, got {refused:?}"
    );
    card.block_type = BlockType::Card;
    assert_eq!(
        before,
        store.visibility_version().await,
        "refused type change leaves grants intact"
    );
    store
        .delete_block(&card.id, &user_id)
        .await
        .expect("block trashed");
    let deleted = store.visibility_version().await;
    assert_ne!(before, deleted, "trash invalidates document existence");
    store
        .undelete_block(&card.id, &user_id)
        .await
        .expect("block restored");
    assert_ne!(
        deleted,
        store.visibility_version().await,
        "restoration invalidates cached absence"
    );
    test.teardown().await;
}
