//! Blocks at the edges: many writers at once, lookups by identifier and by page, calls that
//! change nothing, and what the store will not keep.

use super::*;

/// Restoring a block brings back what was deleted below it.
#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "sequential delete/undelete subtree cases share fixture state"
)]
async fn undelete_block_children() {
    let test = common::setup().await;
    let store = &test.store;
    let user_id = common::user(store, "test-user").await;
    let user_id = user_id.as_str();
    let workspace_id = common::workspace(store).await;

    let boards = common::create_test_boards(store, &workspace_id, user_id, 2).await;
    let board_delete_id = boards[0].id.clone();
    let board_keep_id = boards[1].id.clone();

    // Some blocks to be deleted.
    let cards_delete = create_test_cards(store, user_id, &board_delete_id, 3).await;
    let blocks_delete = create_test_blocks_for_card(store, &cards_delete[0].id, 5).await;
    assert_eq!(blocks_delete.len(), 5, "five content blocks to delete");

    // Some blocks to keep.
    let cards_keep = create_test_cards(store, user_id, &board_keep_id, 3).await;
    let blocks_keep = create_test_blocks_for_card(store, &cards_keep[0].id, 4).await;
    assert_eq!(blocks_keep.len(), 4, "four content blocks to keep");

    // "undelete block children for card"
    let deleted_card = &cards_delete[0];
    let kept_card = &cards_keep[0];
    let text_children = |card: &Block| {
        BlockFilter::container(&card.container_id)
            .with_parent(&card.id)
            .with_type(BlockType::Attachment)
    };

    store
        .delete_block(&deleted_card.id, user_id)
        .await
        .expect("the card is deleted");

    // The card was deleted,
    let error = store
        .get_block(&deleted_card.id)
        .await
        .expect_err("the deleted card is gone");
    assert_not_found(&error);

    // and so were its children,
    let blocks = store
        .get_blocks(&text_children(deleted_card))
        .await
        .expect("the children of the deleted card are read");
    assert!(
        blocks.is_empty(),
        "the children of the deleted card are gone: {blocks:?}"
    );

    // while the children of the other card remain.
    let blocks = store
        .get_blocks(&text_children(kept_card))
        .await
        .expect("the children of the other card are read");
    assert_eq!(
        blocks.len(),
        blocks_keep.len(),
        "the children of the other card remain: {blocks:?}"
    );

    store
        .undelete_block(&deleted_card.id, user_id)
        .await
        .expect("the card is undeleted");

    // The card was restored,
    existing_block(store, &deleted_card.id).await;

    // and so were its children.
    let blocks = store
        .get_blocks(&text_children(deleted_card))
        .await
        .expect("the children of the restored card are read");
    assert_eq!(
        blocks.len(),
        blocks_delete.len(),
        "the children of the restored card are back: {blocks:?}"
    );

    // "undelete block children for board"
    store
        .delete_container(&board_delete_id, user_id)
        .await
        .expect("the board is deleted");

    // The board was deleted,
    let error = store
        .get_container(&board_delete_id)
        .await
        .expect_err("the deleted board is gone");
    assert_not_found(&error);

    // and so were all its cards and blocks,
    let blocks = store
        .get_blocks(&BlockFilter::container(&board_delete_id))
        .await
        .expect("the blocks of the deleted board are read");
    assert!(
        blocks.is_empty(),
        "the blocks of the deleted board are gone: {blocks:?}"
    );

    // while the cards and blocks of the other board remain.
    let blocks = store
        .get_blocks(&BlockFilter::container(&board_keep_id))
        .await
        .expect("the blocks of the other board are read");
    assert_eq!(
        blocks.len(),
        blocks_keep.len() + cards_keep.len(),
        "the blocks of the other board remain"
    );

    store
        .undelete_container(&board_delete_id, user_id)
        .await
        .expect("the board is undeleted");

    // The board was restored,
    store
        .get_container(&board_delete_id)
        .await
        .expect("the board is back");

    // and so were its cards and blocks.
    let blocks = store
        .get_blocks(&BlockFilter::container(&board_delete_id))
        .await
        .expect("the blocks of the restored board are read");
    assert_eq!(
        blocks.len(),
        blocks_delete.len() + cards_delete.len(),
        "the blocks of the restored board are back"
    );

    test.teardown().await;
}

/// One task of [`concurrent_writers_on_one_board_all_succeed`].
struct Writer {
    store: Store,
    task: usize,
    /// The user the task writes as.
    user_id: String,
    board_id: String,
    /// The block that every task patches.
    shared_id: String,
    /// The block the task inserts in each of its rounds.
    own_ids: Vec<String>,
}

impl Writer {
    /// Per round, four writes and one read. On failure, says which operation failed.
    async fn write_rounds(self) -> Result<(), String> {
        let Self {
            store,
            task,
            user_id,
            board_id,
            shared_id,
            own_ids,
        } = self;
        let failed = |round: usize, operation: &str, error: StoreError| {
            format!("task {task}, round {round}: {operation} failed: {error:?}")
        };

        for (round, own_id) in own_ids.iter().enumerate() {
            let mut own = Block {
                parent_id: shared_id.clone(),
                block_type: BlockType::Attachment,
                ..block(own_id, &board_id)
            };
            store
                .insert_block(&mut own, &user_id)
                .await
                .map_err(|error| failed(round, "insert", error))?;
            store
                .patch_block(own_id, &title_patch("patched"), &user_id)
                .await
                .map_err(|error| failed(round, "patch", error))?;
            let shared_title = format!("task {task}, round {round}");
            store
                .patch_block(&shared_id, &title_patch(&shared_title), &user_id)
                .await
                .map_err(|error| failed(round, "patch of the shared block", error))?;
            store
                .get_blocks(&BlockFilter::container(&board_id).with_parent(&shared_id))
                .await
                .map_err(|error| failed(round, "read", error))?;
            if round % 2 == 0 {
                store
                    .delete_block(own_id, &user_id)
                    .await
                    .map_err(|error| failed(round, "delete", error))?;
            } else {
                store
                    .patch_block(own_id, &title_patch("kept"), &user_id)
                    .await
                    .map_err(|error| failed(round, "second patch", error))?;
            }
        }
        Ok(())
    }
}

/// Checks the blocks one task of [`concurrent_writers_on_one_board_all_succeed`] wrote
/// against the blocks of the board and the trash afterwards: the block of a round that
/// ended by deleting is in the trash as the task left it, and the block of any other
/// round exists as the task left it.
async fn assert_blocks_of_a_writer_add_up(
    store: &Store,
    task: usize,
    writer_id: &str,
    own_ids: &[String],
    blocks: &[Block],
) {
    for (round, own_id) in own_ids.iter().enumerate() {
        let existing = blocks.iter().find(|block| &block.id == own_id);
        if round % 2 == 0 {
            assert!(
                existing.is_none(),
                "the block of task {task}, round {round} was deleted: {existing:?}"
            );
            let deleted = block_in_trash(store, own_id).await;
            assert_eq!(
                (
                    deleted.deleted_by.as_str(),
                    deleted.block.title.as_str(),
                    deleted.block.modified_by.as_str()
                ),
                (writer_id, "patched", writer_id),
                "the block of task {task}, round {round} is in the trash as its task left it"
            );
            assert!(
                deleted.block.deleted_at.is_some_and(|at| at > 0),
                "the block of task {task}, round {round} has a deletion time"
            );
        } else {
            let existing = existing
                .unwrap_or_else(|| panic!("the block of task {task}, round {round} should exist"));
            assert_eq!(
                (
                    existing.title.as_str(),
                    existing.created_by.as_str(),
                    existing.modified_by.as_str()
                ),
                ("kept", writer_id, writer_id),
                "the block of task {task}, round {round} exists as its task left it"
            );
            assert_not_in_trash(store, own_id).await;
        }
    }
}

/// Many tasks insert, patch and delete blocks of one board through clones of one store
/// at the same time. Every operation must succeed, and afterwards the blocks and the
/// trash must add up.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_writers_on_one_board_all_succeed() {
    const TASKS: usize = 16;
    /// Four writes per round, so twenty writes per task.
    const ROUNDS: usize = 5;
    /// The rounds of a task that end by deleting the block they inserted.
    const DELETING_ROUNDS: usize = ROUNDS.div_ceil(2);

    let test = common::setup().await;
    let store = &test.store;
    let Fixture {
        user_id, board_id, ..
    } = fixture(store).await;
    let board_id = board_id.as_str();

    // One block that every task patches, so that writes to the same block race too.
    let shared_id = new_id();
    let mut shared = Block {
        block_type: BlockType::Card,
        ..block(&shared_id, board_id)
    };
    store
        .insert_block(&mut shared, &user_id)
        .await
        .expect("the shared block is inserted");

    // Every task writes as a user of its own, and its blocks have their identifiers
    // before it starts, so that the test can look for them afterwards.
    let mut writer_ids = Vec::with_capacity(TASKS);
    for task in 0..TASKS {
        writer_ids.push(common::user(store, &format!("writer-{task}")).await);
    }
    let own_ids: Vec<Vec<String>> = (0..TASKS)
        .map(|_| (0..ROUNDS).map(|_| new_id()).collect())
        .collect();

    let mut writers = JoinSet::new();
    for (task, (writer_id, own_ids)) in writer_ids.iter().zip(&own_ids).enumerate() {
        // A task owns what it works with.
        let writer = Writer {
            store: store.clone(),
            task,
            user_id: writer_id.clone(),
            board_id: board_id.to_owned(),
            shared_id: shared_id.clone(),
            own_ids: own_ids.clone(),
        };
        writers.spawn(writer.write_rounds());
    }
    while let Some(joined) = writers.join_next().await {
        let outcome = joined.expect("a writer task runs to its end");
        assert_eq!(outcome, Ok(()), "every operation of every task succeeds");
    }

    // The blocks: the shared one and, per task, those of the rounds that did not delete.
    let blocks = board_blocks(store, board_id).await;
    assert_eq!(
        blocks.len(),
        1 + TASKS * (ROUNDS - DELETING_ROUNDS),
        "the shared block and the surviving blocks of every task: {:?}",
        common::sorted_ids(&blocks)
    );
    for (task, (writer_id, own_ids)) in writer_ids.iter().zip(&own_ids).enumerate() {
        assert_blocks_of_a_writer_add_up(store, task, writer_id, own_ids, &blocks).await;
    }

    // The shared block is as one of the patches left it, whole: the title a task gave it
    // and that task's user as modifier.
    let shared = existing_block(store, &shared_id).await;
    let last_writer = shared
        .title
        .strip_prefix("task ")
        .and_then(|rest| rest.split_once(", round "))
        .and_then(|(task, _)| task.parse::<usize>().ok())
        .and_then(|task| writer_ids.get(task))
        .unwrap_or_else(|| panic!("the shared block has the title of a patch: {shared:?}"));
    assert_eq!(
        &shared.modified_by, last_writer,
        "the shared block names the user of its last patch, {:?}, as modifier",
        shared.title
    );
    assert_eq!(
        shared.created_by, user_id,
        "the shared block keeps its creator"
    );

    test.teardown().await;
}

/// `get_blocks_by_ids` succeeds only if it finds exactly one block per identifier.
#[tokio::test]
async fn get_blocks_by_ids_wants_one_block_per_identifier() {
    let test = common::setup().await;
    let store = &test.store;
    let Fixture {
        user_id, board_id, ..
    } = fixture(store).await;

    let mut sample = typed_sample_blocks(&board_id);
    common::insert_blocks(store, &mut sample, &user_id).await;
    let block1_id = id_of(&sample, "block1");

    let all = common::sorted_ids(&sample);
    let blocks = store
        .get_blocks_by_ids(&all)
        .await
        .expect("all five blocks exist");
    assert_eq!(
        common::sorted_ids(&blocks),
        all,
        "exactly the blocks asked for"
    );

    let blocks = store
        .get_blocks_by_ids(&[])
        .await
        .expect("asking for no blocks finds all of them");
    assert!(
        blocks.is_empty(),
        "no identifiers, no blocks: {}",
        blocks.len()
    );

    // An identifier that no block has, and text that is not even one.
    for missing in [new_id().as_str(), "missing"] {
        let asked = strings([block1_id, missing]);
        let error = store
            .get_blocks_by_ids(&asked)
            .await
            .expect_err("one block is missing");
        match &error {
            StoreError::NotAllFound { entity, ids } => {
                assert_eq!(*entity, "block", "the error names the kind of record");
                assert_eq!(ids, &asked, "the error names the identifiers asked for");
            }
            other => panic!("expected a not-all-found error, got {other:?}"),
        }
        assert!(
            error.is_not_found(),
            "the error counts as not found: {error:?}"
        );
    }

    // Preserved quirk: the same identifier twice finds one block for two identifiers.
    let error = store
        .get_blocks_by_ids(&strings([block1_id, block1_id]))
        .await
        .expect_err("one block for two identifiers is not all found");
    assert!(
        matches!(error, StoreError::NotAllFound { .. }),
        "expected a not-all-found error, got {error:?}"
    );

    test.teardown().await;
}

/// `get_blocks` returns the page asked for, together with the other filters.
#[tokio::test]
async fn get_blocks_pages_the_result() {
    let test = common::setup().await;
    let store = &test.store;
    let Fixture {
        user_id, board_id, ..
    } = fixture(store).await;

    let cards = create_test_cards(store, &user_id, &board_id, 5).await;
    create_test_blocks_for_card(store, &cards[0].id, 3).await;
    let card_ids: BTreeSet<String> = cards.iter().map(|card| card.id.clone()).collect();

    // Cards only, as the handler that lists the cards of a board asks for them; no
    // page size is no page.
    let page = |page: u32, per_page: u32| BlockFilter {
        paging: std::num::NonZeroU32::new(per_page).map(|per_page| Paging { page, per_page }),
        ..BlockFilter::container(&board_id).with_type(BlockType::Card)
    };

    let mut seen = BTreeSet::new();
    for (number, expected_len) in [(0, 2), (1, 2), (2, 1), (3, 0)] {
        let blocks = store
            .get_blocks(&page(number, 2))
            .await
            .expect("a page is read");
        assert_eq!(
            blocks.len(),
            expected_len,
            "page {number} of five cards, two per page: {blocks:?}"
        );
        seen.extend(blocks.into_iter().map(|block| block.id));
    }
    assert_eq!(seen, card_ids, "the pages together hold every card once");

    let blocks = store
        .get_blocks(&page(0, 10))
        .await
        .expect("a page is read");
    assert_eq!(
        blocks.len(),
        5,
        "a page larger than the result holds all of it: {blocks:?}"
    );

    // No page reads every card.
    let blocks = store
        .get_blocks(&page(0, 0))
        .await
        .expect("every card is read");
    assert_eq!(blocks.len(), 5, "no page does not limit: {blocks:?}");

    test.teardown().await;
}

/// An unset type does not filter; a set `BlockType` returns only blocks of that type.
#[tokio::test]
async fn get_blocks_does_not_filter_when_type_is_unset() {
    let test = common::setup().await;
    let store = &test.store;
    let Fixture {
        user_id, board_id, ..
    } = fixture(store).await;

    let specs = [
        &TYPED_SAMPLE[..],
        &[("view-a", "", "view"), ("view-b", "", "view")],
    ]
    .concat();
    let mut sample = named_blocks(&board_id, &specs);
    common::insert_blocks(store, &mut sample, &user_id).await;

    let filter = BlockFilter::container(&board_id);
    let blocks = store.get_blocks(&filter).await.expect("the query runs");
    assert_eq!(
        blocks.len(),
        7,
        "an unset type selects every block of the board: {:?}",
        sorted_names(&blocks)
    );
    let below = filter.with_parent(id_of(&sample, "block1"));
    let blocks = store.get_blocks(&below).await.expect("the query runs");
    assert_eq!(
        sorted_names(&blocks),
        ["block2", "block3", "block4"],
        "an unset type leaves the other filters in force"
    );

    let filter = BlockFilter::container(&board_id).with_type(BlockType::View);
    let blocks = store.get_blocks(&filter).await.expect("the query runs");
    assert_eq!(sorted_names(&blocks), ["view-a", "view-b"]);

    test.teardown().await;
}

/// A batch with more identifiers than patches is rejected as a whole.
#[tokio::test]
async fn patch_blocks_with_more_ids_than_patches_changes_nothing() {
    let test = common::setup().await;
    let store = &test.store;
    let Fixture {
        user_id, board_id, ..
    } = fixture(store).await;
    let board_id = board_id.as_str();
    let user_id = user_id.as_str();

    let first_id = new_id();
    let second_id = new_id();
    let mut blocks = [
        Block {
            title: "first".to_owned(),
            ..block(&first_id, board_id)
        },
        Block {
            title: "second".to_owned(),
            ..block(&second_id, board_id)
        },
    ];
    store
        .insert_blocks(&mut blocks, user_id)
        .await
        .expect("the blocks are inserted");
    let before = board_blocks(store, board_id).await;

    let batch = BlockPatchBatch {
        block_ids: strings([&first_id, &second_id]),
        block_patches: vec![title_patch("patched")],
    };
    let error = store
        .patch_blocks(&batch, user_id)
        .await
        .expect_err("the second identifier has no patch");
    assert!(
        matches!(
            error,
            StoreError::InvalidContainersAndBlocks(
                ContainersAndBlocksError::BlockIdsAndPatchesMismatch
            )
        ),
        "expected a mismatch error, got {error:?}"
    );

    let first = existing_block(store, &first_id).await;
    assert_eq!(
        first.title, "first",
        "the patch of the first block was rolled back"
    );
    let after = board_blocks(store, board_id).await;
    assert_eq!(
        after, before,
        "the rejected batch left both blocks as they were, modifier and time included"
    );

    // Extra patches beyond the listed block IDs are ignored.
    let batch = BlockPatchBatch {
        block_ids: strings([&first_id]),
        block_patches: vec![title_patch("patched"), title_patch("ignored")],
    };
    store
        .patch_blocks(&batch, user_id)
        .await
        .expect("surplus patches are ignored");
    let first = existing_block(store, &first_id).await;
    assert_eq!(first.title, "patched", "the first block is patched");
    let second = existing_block(store, &second_id).await;
    assert_eq!(second.title, "second", "the second block is untouched");

    test.teardown().await;
}

/// Deleting a block that does not exist succeeds and writes nothing.
#[tokio::test]
async fn delete_block_of_a_missing_block_writes_nothing() {
    let test = common::setup().await;
    let store = &test.store;
    let Fixture {
        user_id, board_id, ..
    } = fixture(store).await;

    let mut sample = three_blocks(&board_id);
    common::insert_blocks(store, &mut sample, &user_id).await;
    let before = board_blocks(store, &board_id).await;

    let missing = new_id();
    store
        .delete_block(&missing, &user_id)
        .await
        .expect("deleting a missing block succeeds");

    let blocks = board_blocks(store, &board_id).await;
    assert_eq!(
        sorted_names(&blocks),
        ["block1", "block2", "block3"],
        "the blocks that exist are untouched"
    );
    assert_eq!(
        blocks, before,
        "the blocks that exist are as they were, modifier and time included"
    );
    assert_not_in_trash(store, &missing).await;

    test.teardown().await;
}

/// Undeleting a block that is not deleted finds nothing in the trash under its
/// identifier and writes nothing.
#[tokio::test]
async fn undelete_block_of_a_block_that_is_not_deleted_writes_nothing() {
    let test = common::setup().await;
    let store = &test.store;
    let Fixture { board_id, .. } = fixture(store).await;
    let creator_id = common::user(store, "creator").await;
    let someone_else_id = common::user(store, "someone-else").await;

    // A deleted child, which undeleting the parent would restore if it did anything.
    let mut sample = named_blocks(
        &board_id,
        &[("parent", "", "card"), ("child", "parent", "attachment")],
    );
    common::insert_blocks(store, &mut sample, &creator_id).await;
    let parent_id = id_of(&sample, "parent");
    let child_id = id_of(&sample, "child");
    store
        .delete_block(child_id, &creator_id)
        .await
        .expect("the child is deleted");
    let before = existing_block(store, parent_id).await;
    let child_before = block_in_trash(store, child_id).await;

    assert_undelete_finds_nothing(store, parent_id, &someone_else_id).await;

    let after = existing_block(store, parent_id).await;
    assert_eq!(after, before, "the block is untouched");
    assert_no_block(store, child_id).await;
    assert_eq!(
        block_in_trash(store, child_id).await,
        child_before,
        "the deleted child is in the trash as it was"
    );

    test.teardown().await;
}

/// The empty string is not a valid block identifier: no block is stored under it,
/// deleting it touches nothing, and undeleting it finds nothing in the trash.
#[tokio::test]
async fn the_empty_id_names_no_block_and_deleting_it_leaves_its_board_alone() {
    let test = common::setup().await;
    let store = &test.store;
    let Fixture {
        user_id, board_id, ..
    } = fixture(store).await;
    let user_id = user_id.as_str();

    let mut sample = subtree_sample_blocks(&board_id);
    common::insert_blocks(store, &mut sample, user_id).await;

    let mut without_id = block("", &board_id);
    let error = store
        .insert_block(&mut without_id, user_id)
        .await
        .expect_err("a block is not stored under the empty identifier");
    assert_invalid_id(&error, "");
    assert_no_block(store, "").await;

    store
        .delete_block("", user_id)
        .await
        .expect("deleting the block with the empty identifier succeeds");
    let blocks = board_blocks(store, &board_id).await;
    assert_eq!(
        common::sorted_ids(&blocks),
        common::sorted_ids(&sample),
        "every block of the board is still there"
    );

    assert_not_in_trash(store, "").await;

    // Some of the board's blocks are in the trash, where undeleting the empty identifier
    // must leave them.
    let child1_id = id_of(&sample, "child1");
    store
        .delete_block(child1_id, user_id)
        .await
        .expect("a block and what is below it are deleted");
    let left = board_blocks(store, &board_id).await;
    assert_eq!(
        sorted_names(&left),
        ["child2", "grandchild2", "greatgrandchild1", "parent"],
        "child1 took what is below it along"
    );

    assert_undelete_finds_nothing(store, "", user_id).await;
    let blocks = board_blocks(store, &board_id).await;
    assert_eq!(
        blocks, left,
        "undeleting the empty identifier brought nothing back and changed nothing"
    );
    block_in_trash(store, child1_id).await;

    test.teardown().await;
}

/// Deleting an image or an attachment, or a card that holds one, succeeds and leaves the
/// record of the file alone: a file is its board's, and its record goes and comes back
/// with the board.
#[tokio::test]
async fn deleting_blocks_that_hold_files_leaves_the_file_records() {
    let test = common::setup().await;
    let store = &test.store;
    let Fixture {
        user_id,
        workspace_id,
        board_id,
    } = fixture(store).await;
    let board_id = board_id.as_str();
    let user_id = user_id.as_str();

    // A block names its file by the name the file is stored under.
    let mut files = Vec::new();
    for stored_name in ["direct.png", "image.png", "attachment.pdf", "untouched.png"] {
        files.push(save_file(store, board_id, stored_name).await);
    }
    // Another board has a file under the name of one whose block is deleted here.
    let other_board_id = another_board(store, &workspace_id, user_id).await;
    files.push(save_file(store, &other_board_id, "direct.png").await);

    let cards = create_test_cards(store, user_id, board_id, 2).await;
    let mut blocks: Vec<Block> = [
        (0, "attachment", "fileId", "direct.png"),
        (1, "attachment", "fileId", "image.png"),
        (1, "attachment", "attachmentId", "attachment.pdf"),
        (0, "attachment", "fileId", "untouched.png"),
    ]
    .into_iter()
    .map(|(card, block_type, field, stored_name)| Block {
        parent_id: cards[card].id.clone(),
        block_type: block_type.parse().expect("known block type"),
        fields: fields(json!({ field: stored_name })),
        ..block(&new_id(), board_id)
    })
    .collect();
    common::insert_blocks(store, &mut blocks, user_id).await;
    let direct_id = blocks[0].id.as_str();
    let kept_id = blocks[3].id.as_str();

    // An image on its own, then a card with an image and an attachment.
    store
        .delete_block(direct_id, user_id)
        .await
        .expect("an image block is deleted");
    store
        .delete_block(&cards[1].id, user_id)
        .await
        .expect("a card that holds files is deleted");

    let left = board_blocks(store, board_id).await;
    let mut expected_left = strings([&cards[0].id, kept_id]);
    expected_left.sort();
    assert_eq!(
        common::sorted_ids(&left),
        expected_left,
        "the first card and its remaining image are left"
    );

    assert_file_records_are(store, &files, "after their blocks were deleted").await;

    // The blocks come back to the records they named.
    store
        .undelete_block(direct_id, user_id)
        .await
        .expect("the image block is restored");
    store
        .undelete_block(&cards[1].id, user_id)
        .await
        .expect("the card that holds files is restored");
    let restored = board_blocks(store, board_id).await;
    assert_eq!(
        restored.len(),
        cards.len() + blocks.len(),
        "both cards and the four blocks that name files exist again: {restored:?}"
    );
    assert_file_records_are(store, &files, "after their blocks were restored").await;

    test.teardown().await;
}

/// `get_block_counts_by_type` counts the blocks that exist, per type.
#[tokio::test]
async fn get_block_counts_by_type_counts_existing_blocks() {
    let test = common::setup().await;
    let store = &test.store;
    let Fixture {
        user_id,
        workspace_id,
        board_id,
    } = fixture(store).await;

    let counts = store
        .get_block_counts_by_type()
        .await
        .expect("the counts are read");
    assert!(
        counts.is_empty(),
        "a store without blocks has no counts: {counts:?}"
    );

    let mut sample = typed_sample_blocks(&board_id);
    common::insert_blocks(store, &mut sample, &user_id).await;
    let other_board_id = another_board(store, &workspace_id, &user_id).await;
    let mut other_board = Block {
        block_type: BlockType::View,
        ..block(&new_id(), &other_board_id)
    };
    store
        .insert_block(&mut other_board, &user_id)
        .await
        .expect("a block of another board is inserted");
    store
        .delete_block(id_of(&sample, "block5"), &user_id)
        .await
        .expect("a block is deleted");

    let counts = store
        .get_block_counts_by_type()
        .await
        .expect("the counts are read");
    let expected = [
        ("attachment".to_owned(), 1),
        ("card".to_owned(), 3),
        ("view".to_owned(), 1),
    ];
    assert_eq!(
        counts.into_iter().collect::<Vec<_>>(),
        expected,
        "blocks of every board, without the deleted one"
    );

    test.teardown().await;
}

/// Rewrites a JSON column of a SQLite table as blobs, on a connection of its own.
async fn rewrite_as_blobs(test: &common::TestStore, table: &str, column: &str) {
    let mut raw = sqlite_connection(test).await;
    let rewritten = sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE {table} SET {column} = CAST({column} AS BLOB)"
    )))
    .execute(&mut raw)
    .await
    .expect("the column is rewritten as blobs");
    assert_ne!(
        rewritten.rows_affected(),
        0,
        "the {table} table has rows to rewrite"
    );
    raw.close()
        .await
        .expect("the second connection closes cleanly");
}

/// This store writes JSON to SQLite as text, and also reads JSON fields stored as SQLite
/// BLOBs in both live and trashed blocks.
#[tokio::test]
async fn sqlite_fields_stored_as_a_blob_are_read() {
    if common::db_type() != DbType::Sqlite {
        // PostgreSQL has JSON columns there, which hold JSON and nothing else.
        return;
    }
    let test = common::setup().await;
    let store = &test.store;
    let Fixture {
        user_id, board_id, ..
    } = fixture(store).await;
    let user_id = user_id.as_str();

    let card_id = new_id();
    let mut card = Block {
        block_type: BlockType::Card,
        fields: fields(json!({ "icon": "🎨", "contentOrder": ["a", ["b", "c"]] })),
        ..block(&card_id, &board_id)
    };
    store
        .insert_block(&mut card, user_id)
        .await
        .expect("the card is inserted");

    rewrite_as_blobs(&test, "blocks", "fields").await;
    let stored = existing_block(store, &card_id).await;
    assert_eq!(
        stored.fields, card.fields,
        "the fields of a block are read from a blob"
    );

    // Deleting reads the fields and writes the whole block into the trash.
    store
        .delete_block(&card_id, user_id)
        .await
        .expect("a block with blob fields is deleted");
    rewrite_as_blobs(&test, "deleted_blocks", "block").await;
    let deleted = block_in_trash(store, &card_id).await;
    assert_eq!(
        deleted.block.fields, card.fields,
        "a block in the trash is read from a blob"
    );

    store
        .undelete_block(&card_id, user_id)
        .await
        .expect("a block that is a blob in the trash is undeleted");
    let restored = existing_block(store, &card_id).await;
    assert_eq!(
        restored.fields, card.fields,
        "the fields survive a delete and an undelete"
    );

    test.teardown().await;
}

/// A block is not stored with `properties` that are not well-formed card property values,
/// whether inserted or patched, and the stored block stays unchanged.
#[tokio::test]
async fn blocks_with_malformed_properties_are_not_stored() {
    let test = common::setup().await;
    let store = &test.store;
    let Fixture {
        user_id, board_id, ..
    } = fixture(store).await;
    let user_id = user_id.as_str();
    let card_with = |id: &str, properties: Value| {
        let mut card = Block {
            block_type: BlockType::Card,
            ..block(id, &board_id)
        };
        card.fields.insert("properties".to_owned(), properties);
        card
    };

    for (what, properties) in [
        ("a number", json!({"p1": 3})),
        ("an object", json!({"p1": {"nested": true}})),
        ("a string", json!("no object")),
    ] {
        let id = new_id();
        let error = store
            .insert_block(&mut card_with(&id, properties.clone()), user_id)
            .await
            .expect_err("malformed properties");
        assert!(
            matches!(
                error,
                StoreError::InvalidBlock {
                    source: BlockError::InvalidCardProperties(_),
                    ..
                }
            ),
            "properties {properties}: got {error:?}"
        );
        assert!(
            store.get_block(&id).await.unwrap_err().is_not_found(),
            "{what}: nothing was stored"
        );
    }

    let fine_id = new_id();
    let well_formed = json!({"p1": "open", "p2": ["a", "b"], "p3": null});
    store
        .insert_block(&mut card_with(&fine_id, well_formed.clone()), user_id)
        .await
        .expect("well-formed properties are stored");
    let mut updated_fields = JsonMap::new();
    updated_fields.insert("properties".to_owned(), json!({"p1": 3}));
    let patch = BlockPatch {
        title: Some("renamed".to_owned()),
        updated_fields,
        ..BlockPatch::default()
    };
    store
        .patch_block(&fine_id, &patch, user_id)
        .await
        .expect_err("a patch to malformed properties");
    let stored = store.get_block(&fine_id).await.unwrap();
    assert_eq!(stored.fields["properties"], well_formed, "unchanged");
    assert_eq!(stored.title, "", "and so is the rest of the block");

    test.teardown().await;
}
