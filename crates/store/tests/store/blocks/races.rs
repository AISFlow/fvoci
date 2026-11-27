//! Writes that race one another, and the moves that would put a block below itself.

use super::*;

/// A card with a text block on it and a text block on that: the name of each block, the
/// name of its parent and its type.
const SMALL_TREE: [(&str, &str, &str); 3] = [
    ("card", "", "card"),
    ("text", "card", "card"),
    ("below", "text", "card"),
];

/// Several callers delete the same card at the same time. Deleting what is deleted
/// already succeeds, so every call succeeds, and the card is in the trash once, with
/// everything below it, as one deletion.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_deletes_of_one_block_all_succeed_and_delete_it_once() {
    let test = common::setup().await;
    let store = &test.store;
    let Fixture {
        user_id, board_id, ..
    } = fixture(store).await;

    for round in 0..RACE_ROUNDS {
        let mut tree = named_blocks(&board_id, &SMALL_TREE);
        common::insert_blocks(store, &mut tree, &user_id).await;
        let card_id = id_of(&tree, "card");

        let start = Arc::new(Barrier::new(RACERS));
        let mut deletes = JoinSet::new();
        for _ in 0..RACERS {
            let (store, start) = (store.clone(), Arc::clone(&start));
            let (card_id, user_id) = (card_id.to_owned(), user_id.clone());
            deletes.spawn(async move {
                start.wait().await;
                store.delete_block(&card_id, &user_id).await
            });
        }
        while let Some(joined) = deletes.join_next().await {
            let outcome = joined.expect("a deleting task runs to its end");
            assert!(
                outcome.is_ok(),
                "round {round}: every delete of the card succeeds, got {outcome:?}"
            );
        }

        let blocks = board_blocks(store, &board_id).await;
        assert!(
            blocks.is_empty(),
            "round {round}: the card and what was below it are gone: {blocks:?}"
        );
        let deleted_at = block_in_trash(store, card_id)
            .await
            .block
            .deleted_at
            .expect("deletion time");
        for stored in &tree {
            let expected = Block {
                deleted_at: Some(deleted_at),
                ..stored.clone()
            };
            assert_eq!(
                block_in_trash(store, &stored.id).await.block,
                expected,
                "round {round}: {} is in the trash as it was, as part of one deletion",
                stored.title
            );
        }
    }

    test.teardown().await;
}

/// Several callers undelete the same deletion at the same time, some by its card and
/// some by a block that went with it. One of them restores it; the others find nothing
/// in the trash. Every block is back once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_undeletes_of_one_deletion_restore_it_once() {
    let test = common::setup().await;
    let store = &test.store;
    let Fixture {
        user_id, board_id, ..
    } = fixture(store).await;

    for round in 0..RACE_ROUNDS {
        let mut tree = named_blocks(&board_id, &SMALL_TREE);
        common::insert_blocks(store, &mut tree, &user_id).await;
        store
            .delete_block(id_of(&tree, "card"), &user_id)
            .await
            .expect("the card is deleted");

        let start = Arc::new(Barrier::new(RACERS));
        let mut undeletes = JoinSet::new();
        for racer in 0..RACERS {
            let (store, start) = (store.clone(), Arc::clone(&start));
            let name = if racer % 2 == 0 { "card" } else { "below" };
            let (block_id, user_id) = (id_of(&tree, name).to_owned(), user_id.clone());
            undeletes.spawn(async move {
                start.wait().await;
                store.undelete_block(&block_id, &user_id).await
            });
        }
        let mut restored = 0;
        while let Some(joined) = undeletes.join_next().await {
            match joined.expect("an undeleting task runs to its end") {
                Ok(_) => restored += 1,
                Err(error) => assert!(
                    matches!(error, StoreError::NotFound(_)),
                    "round {round}: an undelete that comes too late finds nothing, got \
                     {error:?}"
                ),
            }
        }
        assert_eq!(
            restored, 1,
            "round {round}: one undelete restores the deletion"
        );

        let blocks = board_blocks(store, &board_id).await;
        assert_eq!(
            sorted_names(&blocks),
            ["below", "card", "text"],
            "round {round}: every block is back once"
        );
        for stored in &tree {
            assert_not_in_trash(store, &stored.id).await;
        }
        // The board is empty again for the next round.
        store
            .delete_block(id_of(&tree, "card"), &user_id)
            .await
            .expect("the card is deleted again");
    }

    test.teardown().await;
}

/// A block is put on a card while the card is deleted. Either it came first, and then
/// it went into the trash with the card, or the card was gone, and then it is refused
/// over its parent. It is never lost, and neither call fails for another reason.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_block_inserted_while_its_card_is_deleted_goes_with_the_card_or_is_refused() {
    let test = common::setup().await;
    let store = &test.store;
    let Fixture {
        user_id, board_id, ..
    } = fixture(store).await;

    for round in 0..RACE_ROUNDS {
        let mut tree = named_blocks(&board_id, &SMALL_TREE);
        common::insert_blocks(store, &mut tree, &user_id).await;
        let card_id = id_of(&tree, "card");
        let mut late = Block {
            parent_id: id_of(&tree, "below").to_owned(),
            block_type: BlockType::Attachment,
            title: "late".to_owned(),
            ..block(&new_id(), &board_id)
        };

        let start = Barrier::new(2);
        let (deleted, inserted) = tokio::join!(
            async {
                start.wait().await;
                store.delete_block(card_id, &user_id).await
            },
            async {
                start.wait().await;
                store.insert_block(&mut late, &user_id).await
            }
        );

        assert!(
            deleted.is_ok(),
            "round {round}: the card is deleted, got {deleted:?}"
        );
        let blocks = board_blocks(store, &board_id).await;
        assert!(
            blocks.is_empty(),
            "round {round}: the card and everything below it are gone: {blocks:?}"
        );
        for stored in &tree {
            block_in_trash(store, &stored.id).await;
        }
        match inserted {
            Ok(()) => {
                let in_trash = store.get_deleted_block(&late.id).await;
                assert!(
                    in_trash.is_ok(),
                    "round {round}: the block that was stored went into the trash with its \
                     card, got {in_trash:?}"
                );
            }
            Err(error) => {
                assert_invalid_block(&error, &BlockError::ParentNotOnContainer);
                assert_not_in_trash(store, &late.id).await;
            }
        }
    }

    test.teardown().await;
}

/// A block is not stored on a board that does not exist, on one that is in the trash or
/// under text that names no board: that is a not-found error, alone and in a batch, and
/// nothing of the batch is stored.
#[tokio::test]
async fn insert_block_on_a_board_that_does_not_exist_is_not_found() {
    let test = common::setup().await;
    let store = &test.store;
    let Fixture {
        user_id,
        workspace_id,
        board_id,
    } = fixture(store).await;
    let user_id = user_id.as_str();
    let mut card = Block {
        block_type: BlockType::Card,
        ..block(&new_id(), &board_id)
    };
    store
        .insert_block(&mut card, user_id)
        .await
        .expect("a card of the board that exists is inserted");
    let deleted_board_id = another_board(store, &workspace_id, user_id).await;
    store
        .delete_container(&deleted_board_id, user_id)
        .await
        .expect("a board is deleted");
    let all_blocks = async || {
        store
            .get_blocks(&BlockFilter::default())
            .await
            .expect("all blocks are read")
    };
    let before = all_blocks().await;

    let unknown_board_id = new_id();
    for (what, missing_board) in [
        ("a board that does not exist", unknown_board_id.as_str()),
        ("a board in the trash", deleted_board_id.as_str()),
        ("text that is no identifier", "no-identifier"),
    ] {
        // Directly on the board, and on a block that exists on another board.
        for parent_id in ["", card.id.as_str()] {
            let mut homeless = Block {
                parent_id: parent_id.to_owned(),
                ..block(&new_id(), missing_board)
            };
            match store.insert_block(&mut homeless, user_id).await {
                Ok(()) => panic!("a block on {what} should not be stored"),
                Err(error) => assert!(
                    matches!(error, StoreError::NotFound(_)),
                    "a block on {what} with the parent {parent_id:?} should be a not-found \
                     error, got {error:?}"
                ),
            }
            assert_no_block(store, &homeless.id).await;
            assert_not_in_trash(store, &homeless.id).await;
        }

        // The first block of the batch alone would be fine.
        let mut batch = [block(&new_id(), &board_id), block(&new_id(), missing_board)];
        match store.insert_blocks(&mut batch, user_id).await {
            Ok(()) => panic!("a batch with a block on {what} should not be stored"),
            Err(error) => assert!(
                matches!(error, StoreError::NotFound(_)),
                "a batch with a block on {what} should be a not-found error, got {error:?}"
            ),
        }
        assert_eq!(
            all_blocks().await,
            before,
            "nothing was stored for {what}, of the batch either"
        );
    }

    test.teardown().await;
}

/// A patch that puts a block on another block or, with the empty text, at the top of
/// its board.
fn parent_patch(parent_id: &str) -> BlockPatch {
    BlockPatch {
        parent_id: Some(parent_id.to_owned()),
        ..BlockPatch::default()
    }
}

/// The two ways of giving a block another parent.
#[derive(Debug, Clone, Copy)]
enum MoveBy {
    /// Writing the whole block with `insert_block`.
    Write,
    /// Patching the parent with `patch_block`.
    Patch,
}

/// Puts a block that exists on another block or, with the empty text, at the top of its
/// board.
async fn move_block(
    store: &Store,
    block_id: &str,
    parent_id: &str,
    user_id: &str,
    by: MoveBy,
) -> Result<(), StoreError> {
    match by {
        MoveBy::Write => {
            let mut block = existing_block(store, block_id).await;
            parent_id.clone_into(&mut block.parent_id);
            store.insert_block(&mut block, user_id).await
        }
        MoveBy::Patch => store
            .patch_block(block_id, &parent_patch(parent_id), user_id)
            .await
            .map(|_| ()),
    }
}

/// Asserts that a move was refused because it would put the block below itself.
#[track_caller]
fn assert_refused_as_below_itself(outcome: &Result<(), StoreError>, moved_id: &str, what: &str) {
    assert!(
        matches!(
            outcome,
            Err(StoreError::InvalidBlock { id, source: BlockError::ParentBelowBlock })
                if id == moved_id
        ),
        "{what} should be refused as putting the block below itself, got {outcome:?}"
    );
}

/// The name of each block with the name of the block it is on, sorted by name.
fn names_with_parents(blocks: &[Block]) -> Vec<(&str, &str)> {
    let mut pairs: Vec<(&str, &str)> = blocks
        .iter()
        .map(|block| {
            let parent = blocks
                .iter()
                .find(|parent| parent.id == block.parent_id)
                .map_or("", |parent| parent.title.as_str());
            (block.title.as_str(), parent)
        })
        .collect();
    pairs.sort_unstable();
    pairs
}

/// A block is not moved onto a block below it, at any depth, by a write of the block, by
/// a patch or within a batch, and nothing changes. It may move onto any other block of
/// its board and to the top, and what is below it is what counts at the time.
#[tokio::test]
async fn an_update_that_would_put_a_block_below_itself_is_refused() {
    let test = common::setup().await;
    let store = &test.store;
    let Fixture {
        user_id, board_id, ..
    } = fixture(store).await;
    let user_id = user_id.as_str();

    let specs = [&SMALL_TREE[..], &[("other card", "", "card")]].concat();
    let mut tree = named_blocks(&board_id, &specs);
    common::insert_blocks(store, &mut tree, user_id).await;
    let id = |name: &str| id_of(&tree, name);
    let blocks = board_blocks(store, &board_id).await;

    for (moved, onto) in [("card", "text"), ("card", "below"), ("text", "below")] {
        for by in [MoveBy::Write, MoveBy::Patch] {
            let what = format!("moving {moved} onto {onto} by {by:?}");
            let outcome = move_block(store, id(moved), id(onto), user_id, by).await;
            assert_refused_as_below_itself(&outcome, id(moved), &what);
            assert_eq!(
                board_blocks(store, &board_id).await,
                blocks,
                "{what} changed nothing, modifier and time included"
            );
        }
    }

    // Two blocks at the top that one batch puts on each other.
    let mut batch = [
        Block {
            parent_id: id("other card").to_owned(),
            ..block_named(&tree, "card").clone()
        },
        Block {
            parent_id: id("card").to_owned(),
            ..block_named(&tree, "other card").clone()
        },
    ];
    let error = store
        .insert_blocks(&mut batch, user_id)
        .await
        .expect_err("a batch that puts two blocks on each other is refused");
    assert_invalid_block(&error, &BlockError::ParentBelowBlock);
    assert_eq!(
        board_blocks(store, &board_id).await,
        blocks,
        "the refused batch changed nothing"
    );

    // Moves that put no block below itself: up, to another card, to the top, and onto
    // a block that was below the moved one until it moved away.
    for (moved, onto, by) in [
        ("below", "card", MoveBy::Patch),
        ("text", "other card", MoveBy::Write),
        ("below", "", MoveBy::Patch),
        ("below", "", MoveBy::Write),
        ("card", "other card", MoveBy::Write),
    ] {
        let parent_id = if onto.is_empty() { "" } else { id(onto) };
        move_block(store, id(moved), parent_id, user_id, by)
            .await
            .unwrap_or_else(|error| {
                panic!("moving {moved} onto {onto:?} by {by:?} should work: {error:?}")
            });
        assert_eq!(
            existing_block(store, id(moved)).await.parent_id,
            parent_id,
            "{moved} is on {onto:?} after moving by {by:?}"
        );
    }
    let moved = board_blocks(store, &board_id).await;
    assert_eq!(
        names_with_parents(&moved),
        [
            ("below", ""),
            ("card", "other card"),
            ("other card", ""),
            ("text", "other card")
        ],
        "every block is where it was moved to, and the others where they were"
    );

    // The card is now below the other card.
    for by in [MoveBy::Write, MoveBy::Patch] {
        let what = format!("moving the other card onto the card by {by:?}");
        let outcome = move_block(store, id("other card"), id("card"), user_id, by).await;
        assert_refused_as_below_itself(&outcome, id("other card"), &what);
        assert_eq!(
            board_blocks(store, &board_id).await,
            moved,
            "{what} changed nothing"
        );
    }

    test.teardown().await;
}

/// Two blocks at the top of a board are moved onto each other at the same time. One move
/// succeeds; the other is refused, because by then its block has the other below it. The
/// two never end up on each other, out of reach from the board.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_blocks_moved_onto_each_other_at_once_never_form_a_cycle() {
    let test = common::setup().await;
    let store = &test.store;
    let Fixture {
        user_id, board_id, ..
    } = fixture(store).await;

    for round in 0..RACE_ROUNDS {
        let mut pair = named_blocks(&board_id, &[("a", "", "card"), ("b", "", "card")]);
        common::insert_blocks(store, &mut pair, &user_id).await;
        // Both ways of moving a block take part, in every pairing.
        let ways = [
            [MoveBy::Write, MoveBy::Write],
            [MoveBy::Write, MoveBy::Patch],
            [MoveBy::Patch, MoveBy::Write],
            [MoveBy::Patch, MoveBy::Patch],
        ][round % 4];

        let start = Arc::new(Barrier::new(2));
        let mut racers = JoinSet::new();
        for (moved, onto, by) in [("a", "b", ways[0]), ("b", "a", ways[1])] {
            let (store, start) = (store.clone(), Arc::clone(&start));
            let (moved_id, onto_id) = (
                id_of(&pair, moved).to_owned(),
                id_of(&pair, onto).to_owned(),
            );
            let user_id = user_id.clone();
            racers.spawn(async move {
                start.wait().await;
                let outcome = move_block(&store, &moved_id, &onto_id, &user_id, by).await;
                (moved, moved_id, outcome)
            });
        }
        let mut on_the_other = Vec::new();
        while let Some(joined) = racers.join_next().await {
            let (moved, moved_id, outcome) = joined.expect("a moving task runs to its end");
            if outcome.is_ok() {
                on_the_other.push(moved);
            } else {
                assert_refused_as_below_itself(
                    &outcome,
                    &moved_id,
                    &format!("round {round}: the move of {moved} that came second"),
                );
            }
        }
        assert_eq!(
            on_the_other.len(),
            1,
            "round {round}, moving by {ways:?}: one of the two moves succeeds: \
             {on_the_other:?}"
        );

        let blocks = board_blocks(store, &board_id).await;
        let expected = if on_the_other == ["a"] {
            [("a", "b"), ("b", "")]
        } else {
            [("a", ""), ("b", "a")]
        };
        assert_eq!(
            names_with_parents(&blocks),
            expected,
            "round {round}: the block that was moved is on the other, which is at the top"
        );

        // Deleting the one at the top takes the other along: the board is empty again.
        let top = if on_the_other == ["a"] { "b" } else { "a" };
        store
            .delete_block(id_of(&pair, top), &user_id)
            .await
            .expect("the block at the top is deleted");
        let left = board_blocks(store, &board_id).await;
        assert!(
            left.is_empty(),
            "round {round}: both blocks went with the one at the top: {left:?}"
        );
    }

    test.teardown().await;
}
