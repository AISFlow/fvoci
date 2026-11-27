//! How a write is checked and kept: its parent, its board, its place among the others, its
//! creator, and the trash it is purged from.

use super::*;

/// The parent of a block is a block of the same board that exists. A block with any
/// other parent is refused as invalid, whether it is new or exists, and nothing is
/// written.
#[tokio::test]
async fn insert_block_refuses_a_parent_that_is_not_a_block_of_the_board() {
    let test = common::setup().await;
    let store = &test.store;
    let Fixture {
        user_id,
        workspace_id,
        board_id,
    } = fixture(store).await;
    let user_id = user_id.as_str();
    let other_board_id = another_board(store, &workspace_id, user_id).await;

    let mut sample = named_blocks(&board_id, &[("card", "", "card"), ("deleted", "", "card")]);
    common::insert_blocks(store, &mut sample, user_id).await;
    let card_id = id_of(&sample, "card");
    let deleted_id = id_of(&sample, "deleted");
    store
        .delete_block(deleted_id, user_id)
        .await
        .expect("a card is deleted");
    let mut foreign = Block {
        block_type: BlockType::Card,
        ..block(&new_id(), &other_board_id)
    };
    store
        .insert_block(&mut foreign, user_id)
        .await
        .expect("a card of another board is inserted");
    let blocks = board_blocks(store, &board_id).await;
    let other_blocks = board_blocks(store, &other_board_id).await;

    let unknown_id = new_id();
    let own_id = new_id();
    for (what, child_id, parent_id) in [
        ("a block of another board", new_id(), foreign.id.as_str()),
        ("a block that does not exist", new_id(), unknown_id.as_str()),
        ("the block itself", own_id.clone(), own_id.as_str()),
        ("a block in the trash", new_id(), deleted_id),
        ("text that is no identifier", new_id(), "no-identifier"),
        // A block that exists is refused such a parent too.
        (
            "itself, for a block that exists",
            card_id.to_owned(),
            card_id,
        ),
        (
            "a block of another board, for a block that exists",
            card_id.to_owned(),
            foreign.id.as_str(),
        ),
    ] {
        let mut child = Block {
            parent_id: parent_id.to_owned(),
            block_type: BlockType::Attachment,
            title: "refused".to_owned(),
            ..block(&child_id, &board_id)
        };
        match store.insert_block(&mut child, user_id).await {
            Ok(()) => panic!("a block whose parent is {what} should be refused"),
            Err(error) => assert!(
                matches!(
                    &error,
                    StoreError::InvalidBlock { id, source: BlockError::ParentNotOnContainer }
                        if *id == child_id
                ),
                "a block whose parent is {what} should be refused over its parent, got \
                 {error:?}"
            ),
        }
        assert_eq!(
            board_blocks(store, &board_id).await,
            blocks,
            "the block whose parent is {what} was not stored and changed nothing"
        );
        assert_eq!(
            board_blocks(store, &other_board_id).await,
            other_blocks,
            "the other board is as it was after a block whose parent is {what}"
        );
    }

    test.teardown().await;
}

/// The board is no parent of its blocks: a block that names it as its parent is refused
/// as one whose parent is no block of the board, when it is inserted, when it is updated
/// and in a batch, and asking for the blocks below the board finds none.
#[tokio::test]
async fn a_block_that_names_its_board_as_parent_is_refused() {
    let test = common::setup().await;
    let store = &test.store;
    let Fixture {
        user_id, board_id, ..
    } = fixture(store).await;
    let refused = |result: Result<(), StoreError>, what: &str| {
        assert!(
            matches!(
                result,
                Err(StoreError::InvalidBlock {
                    source: BlockError::ParentNotOnContainer,
                    ..
                })
            ),
            "{what}: {result:?}"
        );
    };

    let card_id = new_id();
    let mut card = Block {
        parent_id: board_id.clone(),
        block_type: BlockType::Card,
        ..block(&card_id, &board_id)
    };
    refused(store.insert_block(&mut card, &user_id).await, "a new block");
    card.parent_id.clear();
    store
        .insert_block(&mut card, &user_id)
        .await
        .expect("a block without a parent is directly on its board");
    card.parent_id.clone_from(&board_id);
    refused(store.insert_block(&mut card, &user_id).await, "an update");
    assert_eq!(
        existing_block(store, &card_id).await.parent_id,
        "",
        "unchanged"
    );

    let mut batch = [Block {
        parent_id: board_id.clone(),
        block_type: BlockType::Card,
        ..block(&new_id(), &board_id)
    }];
    refused(store.insert_blocks(&mut batch, &user_id).await, "a batch");

    let below_board = store
        .get_blocks(&BlockFilter::container(&board_id).with_parent(&board_id))
        .await
        .expect("the query runs");
    assert!(below_board.is_empty(), "no block is below the board");

    test.teardown().await;
}

/// A batch may give children before their parents, at any depth: all of it is stored.
/// A batch with a block whose parent is neither stored nor in the batch stores nothing.
#[tokio::test]
async fn insert_blocks_stores_children_given_before_their_parents() {
    let test = common::setup().await;
    let store = &test.store;
    let Fixture {
        user_id, board_id, ..
    } = fixture(store).await;

    // The deepest block first and the card it is below last.
    let mut batch: Vec<Block> = named_blocks(&board_id, &CARD_TREE)
        .into_iter()
        .rev()
        .collect();
    assert_eq!(
        names(&batch)[..2],
        ["other text", "other card"],
        "the batch gives a child right before its parent"
    );
    let given_parents: Vec<String> = batch.iter().map(|block| block.parent_id.clone()).collect();
    store
        .insert_blocks(&mut batch, &user_id)
        .await
        .expect("a batch with children before their parents is stored");

    let stored = board_blocks(store, &board_id).await;
    assert_eq!(
        stored.len(),
        CARD_TREE.len(),
        "every block of the batch is stored once: {stored:?}"
    );
    for (block, given_parent) in batch.iter().zip(&given_parents) {
        assert_eq!(
            &existing_block(store, &block.id).await,
            block,
            "{} is stored as the batch says it was",
            block.title
        );
        assert_eq!(
            &block.parent_id, given_parent,
            "{} is on the block it was given",
            block.title
        );
    }

    // All or none: the first two blocks alone would be fine.
    let mut late: Vec<Block> = named_blocks(
        &board_id,
        &[
            ("late card", "", "card"),
            ("late text", "late card", "attachment"),
        ],
    )
    .into_iter()
    .rev()
    .collect();
    late.push(Block {
        parent_id: new_id(),
        block_type: BlockType::Attachment,
        title: "orphan".to_owned(),
        ..block(&new_id(), &board_id)
    });
    let error = store
        .insert_blocks(&mut late, &user_id)
        .await
        .expect_err("a batch with a block on a block that does not exist is refused");
    assert_invalid_block(&error, &BlockError::ParentNotOnContainer);
    assert_eq!(
        board_blocks(store, &board_id).await,
        stored,
        "the refused batch stored none of its blocks"
    );

    test.teardown().await;
}

/// Every list of blocks comes oldest first, by creation time and then by identifier,
/// whatever the order in which the blocks were written and asked for.
#[tokio::test]
async fn lists_of_blocks_are_ordered_by_creation_time_then_identifier() {
    /// The children of the card: which of the identifiers each has, counted from the
    /// smallest, its creation time and its type. They are written in this order, which is
    /// neither that of the identifiers nor that of the creation times.
    const CHILDREN: [(usize, i64, &str); 6] = [
        (2, 4000, "comment"),
        (4, 3000, "attachment"),
        (1, 5000, "attachment"),
        (5, 2000, "attachment"),
        (0, 4000, "attachment"),
        (3, 3000, "comment"),
    ];
    /// The children by the number of their identifier, oldest first; of two that are
    /// equally old the one with the smaller identifier first.
    const OLDEST_FIRST: [&str; 6] = ["5", "3", "4", "0", "2", "1"];

    let test = common::setup().await;
    let store = &test.store;
    let Fixture {
        user_id, board_id, ..
    } = fixture(store).await;
    let board = || BlockFilter::container(&board_id);

    let mut ids: Vec<String> = (0..CHILDREN.len()).map(|_| new_id()).collect();
    ids.sort();
    // The card is written first and is the youngest, with the greatest identifier.
    let card_id = new_id();
    let mut blocks = vec![Block {
        block_type: BlockType::Card,
        title: "card".to_owned(),
        ..block(&card_id, &board_id)
    }];
    blocks.extend(CHILDREN.into_iter().map(|(number, _, block_type)| Block {
        parent_id: card_id.clone(),
        block_type: block_type.parse().expect("known block type"),
        title: number.to_string(),
        ..block(&ids[number], &board_id)
    }));
    common::insert_blocks(store, &mut blocks, &user_id).await;

    // The store creates every block now, so the creation times are set past it: two
    // pairs of blocks are equally old, which the clock would make a matter of luck.
    let set_times: Vec<String> = CHILDREN
        .iter()
        .map(|&(number, created_at, _)| (ids[number].as_str(), created_at))
        .chain([(card_id.as_str(), 9000)])
        .map(|(id, created_at)| {
            format!("UPDATE blocks SET created_at = {created_at} WHERE id = '{id}';")
        })
        .collect();
    common::execute_raw(&test.database_connection(), &set_times.join("\n")).await;

    let listed = async |filter: BlockFilter| -> Vec<String> {
        let blocks = store.get_blocks(&filter).await.expect("the query runs");
        blocks.into_iter().map(|block| block.title).collect()
    };
    let with_card = [&OLDEST_FIRST[..], &["card"]].concat();

    assert_eq!(
        listed(board()).await,
        with_card,
        "the blocks of a board, the card last although it was written first"
    );
    assert_eq!(
        listed(BlockFilter::default()).await,
        with_card,
        "all blocks there are"
    );
    assert_eq!(
        listed(board().with_parent(&card_id)).await,
        OLDEST_FIRST,
        "the blocks on a card"
    );
    assert_eq!(
        listed(board().with_type(BlockType::Attachment)).await,
        ["5", "4", "0", "1"],
        "the blocks of a type"
    );
    assert_eq!(
        listed(board().with_parent(&card_id).with_type(BlockType::Comment)).await,
        ["3", "2"],
        "the blocks of a type on a card"
    );
    for (page, expected) in [(0, &OLDEST_FIRST[..4]), (1, &OLDEST_FIRST[4..])] {
        let filter = BlockFilter {
            paging: Some(Paging {
                page,
                per_page: std::num::NonZeroU32::new(4).expect("a page size"),
            }),
            ..board().with_parent(&card_id)
        };
        assert_eq!(
            listed(filter).await,
            expected,
            "page {page} of the blocks on a card, four per page"
        );
    }

    // Asked for from the smallest identifier to the greatest.
    let mut asked = ids.clone();
    asked.push(card_id.clone());
    let by_ids = store
        .get_blocks_by_ids(&asked)
        .await
        .expect("all seven blocks exist");
    assert_eq!(
        names(&by_ids),
        with_card,
        "the blocks with given identifiers"
    );

    test.teardown().await;
}

/// Purging the trash removes for good what was deleted before the given time and leaves
/// what was deleted later, which can still be restored, and what exists.
#[tokio::test]
async fn purge_trash_removes_what_was_deleted_before_the_cutoff() {
    let test = common::setup().await;
    let store = &test.store;
    let Fixture {
        user_id, board_id, ..
    } = fixture(store).await;
    let user_id = user_id.as_str();
    let tree = insert_card_tree(store, &board_id, user_id).await;
    let card_id = id_of(&tree, "card");
    let other_card_id = id_of(&tree, "other card");
    let mut kept = block(&new_id(), &board_id);
    store
        .insert_block(&mut kept, user_id)
        .await
        .expect("a block that stays is inserted");

    store
        .delete_block(card_id, user_id)
        .await
        .expect("the card is deleted");
    let older = block_in_trash(store, card_id)
        .await
        .block
        .deleted_at
        .expect("older deletion time");
    // A later millisecond, so that the two deletions have different times.
    tokio::time::sleep(Duration::from_millis(2)).await;
    store
        .delete_block(other_card_id, user_id)
        .await
        .expect("the other card is deleted");
    let newer = block_in_trash(store, other_card_id)
        .await
        .block
        .deleted_at
        .expect("newer deletion time");
    assert!(
        newer > older,
        "the second deletion is later than the first: {newer} after {older}"
    );
    let newer_trash = trash_of(store, &tree, &["other card", "other text"]).await;

    let purged = store.purge_trash(older).await.expect("the trash is purged");
    assert_eq!(
        purged,
        common::purged_without_files(0),
        "what was deleted at the cutoff, and not before it, stays"
    );
    block_in_trash(store, card_id).await;

    let purged = store.purge_trash(newer).await.expect("the trash is purged");
    assert_eq!(
        purged,
        common::purged_without_files(5),
        "the card and the four blocks below it were deleted before the cutoff"
    );
    for name in CARD_AND_BELOW {
        let block_id = id_of(&tree, name);
        assert_not_in_trash(store, block_id).await;
        assert_no_block(store, block_id).await;
    }
    assert_undelete_finds_nothing(store, card_id, user_id).await;
    assert_undelete_finds_nothing(store, id_of(&tree, "deepest"), user_id).await;
    assert_eq!(
        trash_of(store, &tree, &["other card", "other text"]).await,
        newer_trash,
        "what was deleted at the cutoff is in the trash as it was"
    );
    assert_eq!(
        board_blocks(store, &board_id).await,
        [kept.clone()],
        "the block that exists is as it was"
    );

    let other_card = store
        .undelete_block(other_card_id, user_id)
        .await
        .expect("what the purge left in the trash is restored");
    assert_restored(
        store,
        &tree,
        &["other card", "other text"],
        user_id,
        other_card.updated_at,
    )
    .await;

    let purged = store
        .purge_trash(now_millis() + 60_000)
        .await
        .expect("the empty trash is purged");
    assert_eq!(
        purged,
        common::purged_without_files(0),
        "nothing is left in the trash to purge"
    );
    let blocks = board_blocks(store, &board_id).await;
    assert_eq!(
        sorted_names(&blocks),
        ["", "other card", "other text"],
        "a purge removes nothing that exists"
    );

    test.teardown().await;
}

/// A block stays on its board: writing a block under an identifier that a block of
/// another board has is a not-found error and changes neither board.
#[tokio::test]
async fn insert_block_of_an_identifier_on_another_board_is_not_found() {
    let test = common::setup().await;
    let store = &test.store;
    let workspace_id = common::workspace(store).await;
    let user_a = common::user(store, "user-a").await;
    let user_b = common::user(store, "user-b").await;
    let board_a = another_board(store, &workspace_id, &user_a).await;
    let board_b = another_board(store, &workspace_id, &user_b).await;

    let moving_id = new_id();
    let mut original = Block {
        title: "original".to_owned(),
        ..block(&moving_id, &board_a)
    };
    store
        .insert_block(&mut original, &user_a)
        .await
        .expect("the block is inserted");
    let before = existing_block(store, &moving_id).await;

    let mut moved = Block {
        title: "moved".to_owned(),
        ..block(&moving_id, &board_b)
    };
    let error = store
        .insert_block(&mut moved, &user_b)
        .await
        .expect_err("the block is not written under another board");
    assert_not_found(&error);
    let mut batch = [moved];
    let error = store
        .insert_blocks(&mut batch, &user_b)
        .await
        .expect_err("a batch does not write the block under another board either");
    assert_not_found(&error);

    let after = existing_block(store, &moving_id).await;
    assert_eq!(after, before, "the block is untouched");
    let blocks = board_blocks(store, &board_b).await;
    assert!(
        blocks.is_empty(),
        "the other board did not get the block: {blocks:?}"
    );

    test.teardown().await;
}

/// An update, by a write of the whole block or by a patch, leaves the stored creator and
/// creation time alone, whatever the written block says, and sets modifier and
/// modification time.
#[tokio::test]
async fn an_update_keeps_the_creator_and_the_creation_time() {
    let test = common::setup().await;
    let store = &test.store;
    let Fixture { board_id, .. } = fixture(store).await;
    let creator_id = common::user(store, "creator").await;
    let updater_id = common::user(store, "updater").await;

    let block_id = new_id();
    let mut new = block(&block_id, &board_id);
    store
        .insert_block(&mut new, &creator_id)
        .await
        .expect("the block is inserted");
    let created = existing_block(store, &block_id).await;

    for title in ["written", "patched"] {
        let before = existing_block(store, &block_id).await;
        // A later millisecond, so that the modification times can be told apart.
        tokio::time::sleep(Duration::from_millis(2)).await;
        if title == "written" {
            let mut update = Block {
                title: title.to_owned(),
                created_by: updater_id.clone(),
                created_at: 631_155_600_000,
                ..block(&block_id, &board_id)
            };
            store
                .insert_block(&mut update, &updater_id)
                .await
                .expect("the block is updated");
        } else {
            store
                .patch_block(&block_id, &title_patch(title), &updater_id)
                .await
                .expect("the block is patched");
        }

        let updated = existing_block(store, &block_id).await;
        assert_eq!(
            (updated.created_by.as_str(), updated.created_at),
            (creator_id.as_str(), created.created_at),
            "the block that was {title} keeps its creator and creation time"
        );
        assert_eq!(
            (updated.title.as_str(), updated.modified_by.as_str()),
            (title, updater_id.as_str()),
            "the block that was {title} has the new title and modifier"
        );
        assert!(
            updated.updated_at > before.updated_at,
            "the block that was {title} has a later modification time: {} after {}",
            updated.updated_at,
            before.updated_at
        );
    }

    test.teardown().await;
}

/// One batch may write the same block many times: the first write inserts it, the others
/// update it in the order given, and the block is in the state of the last write.
#[tokio::test]
async fn one_batch_writing_a_block_many_times_keeps_the_last_write() {
    const WRITES: usize = 100;

    let test = common::setup().await;
    let store = &test.store;
    let Fixture {
        user_id, board_id, ..
    } = fixture(store).await;

    let batched_id = new_id();
    let mut batch: Vec<Block> = (0..WRITES)
        .map(|write| Block {
            title: format!("write {write}"),
            ..block(&batched_id, &board_id)
        })
        .collect();
    store
        .insert_blocks(&mut batch, &user_id)
        .await
        .expect("a batch may write one block many times");

    let blocks = board_blocks(store, &board_id).await;
    assert_eq!(
        names(&blocks),
        [format!("write {}", WRITES - 1)],
        "the board has the block once, in the state of the last write"
    );

    test.teardown().await;
}
