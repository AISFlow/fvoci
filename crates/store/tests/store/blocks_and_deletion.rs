//! A block written while its board is deleted.

use crate::blocks::{
    Fixture, another_board, assert_no_block, block, block_in_trash, board_blocks, fixture,
    sorted_names,
};
use crate::boards::RACE_ROUNDS;
use crate::common;
use fvoci_model::{Block, BlockType, new_id};
use fvoci_store::StoreError;
use std::sync::Arc;
use tokio::sync::Barrier;

/// A block is put on a board, directly or on a card, while the board is deleted. Either
/// it came first, and then it is in the trash with the board and comes back with it, or
/// the board was gone, and then that is a not-found error and the block is nowhere. It
/// is never lost, and neither call fails for another reason.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_block_inserted_while_its_board_is_deleted_goes_with_the_board_or_is_not_found() {
    let test = common::setup().await;
    let store = &test.store;
    let Fixture {
        user_id,
        workspace_id,
        ..
    } = fixture(store).await;

    for round in 0..RACE_ROUNDS {
        let raced_board_id = another_board(store, &workspace_id, &user_id).await;
        let mut card = Block {
            block_type: BlockType::Card,
            title: "card".to_owned(),
            ..block(&new_id(), &raced_board_id)
        };
        store
            .insert_block(&mut card, &user_id)
            .await
            .expect("a card of the board is inserted");
        let late_id = new_id();
        let mut late = Block {
            parent_id: if round % 2 == 0 {
                card.id.clone()
            } else {
                String::new()
            },
            block_type: if round % 2 == 0 {
                BlockType::Attachment
            } else {
                BlockType::View
            },
            title: "late".to_owned(),
            ..block(&late_id, &raced_board_id)
        };

        let start = Arc::new(Barrier::new(2));
        let deleting = tokio::spawn({
            let (store, start) = (store.clone(), Arc::clone(&start));
            let (board_id, user_id) = (raced_board_id.clone(), user_id.clone());
            async move {
                start.wait().await;
                store.delete_container(&board_id, &user_id).await
            }
        });
        let inserting = tokio::spawn({
            let (store, start) = (store.clone(), Arc::clone(&start));
            let user_id = user_id.clone();
            async move {
                start.wait().await;
                store.insert_block(&mut late, &user_id).await
            }
        });
        let deleted = deleting.await.expect("the deleting task runs to its end");
        let inserted = inserting.await.expect("the inserting task runs to its end");

        assert!(
            deleted.is_ok(),
            "round {round}: the board is deleted, got {deleted:?}"
        );
        let blocks = board_blocks(store, &raced_board_id).await;
        assert!(
            blocks.is_empty(),
            "round {round}: the deleted board has no blocks: {blocks:?}"
        );
        block_in_trash(store, &card.id).await;
        assert_no_block(store, &late_id).await;
        let in_trash = store.get_deleted_block(&late_id).await;
        let expected_back = match &inserted {
            Ok(()) => {
                assert!(
                    in_trash.is_ok(),
                    "round {round}: the block that was stored went into the trash with its \
                     board, got {in_trash:?}"
                );
                vec!["card", "late"]
            }
            Err(error) => {
                assert!(
                    matches!(error, StoreError::NotFound(_)),
                    "round {round}: a block that comes after its board is gone is a \
                     not-found error, got {error:?}"
                );
                assert!(
                    matches!(in_trash, Err(StoreError::NotFound(_))),
                    "round {round}: the block that was refused is not in the trash, got \
                     {in_trash:?}"
                );
                vec!["card"]
            }
        };

        store
            .undelete_container(&raced_board_id, &user_id)
            .await
            .unwrap_or_else(|error| panic!("round {round}: the board is restored: {error:?}"));
        let back = board_blocks(store, &raced_board_id).await;
        assert_eq!(
            sorted_names(&back),
            expected_back,
            "round {round}: the board came back with what went into the trash with it"
        );
    }

    test.teardown().await;
}
