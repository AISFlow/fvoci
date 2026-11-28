//! The numbers of the cards of a board: every way a card is stored takes
//! the board's next number, also many at once, a number is never given again, and a
//! card that comes back from the trash has its own.

use std::collections::BTreeSet;
use std::sync::Arc;

use fvoci_model::DuplicateAs;
use fvoci_model::{
    Block, BlockPatch, BlockType, Container, ContainerVisibility, ContainersAndBlocks, new_id,
};
use fvoci_store::{
    BlockFilter, CopyChanges, CopyTitles, Creator, DbType, NewBodyFiles, NewContainers, Previews,
    Store, StoreError,
};
use serde_json::Value;
use tokio::sync::Barrier;
use tokio::task::JoinSet;

use crate::common::{self, TestStore};

const TITLES: CopyTitles = CopyTitles {
    copy_of: |title| format!("{title} copy"),
    new_template: "New board template",
};

/// A board of a workspace the user works in.
async fn board(store: &Store, workspace_id: &str, user_id: &str) -> String {
    let mut board = Container {
        id: new_id(),
        workspace_id: workspace_id.to_owned(),
        visibility: ContainerVisibility::Open,
        title: "board".to_owned(),
        ..Container::default()
    };
    store
        .insert_container(&mut board, user_id)
        .await
        .expect("the board is stored");
    board.id
}

/// A user, a workspace they work in and a board of it.
async fn board_of_a_user(test: &TestStore) -> (String, String, String) {
    let user_id = common::user(test, "numberer").await;
    let workspace_id = common::workspace_of(test, &[&user_id]).await;
    let board_id = board(test, &workspace_id, &user_id).await;
    (user_id, workspace_id, board_id)
}

/// A block of the type on the board, titled; as a request sends it, with a number of
/// its own that the store is not to take.
fn block(board_id: &str, block_type: BlockType, title: &str) -> Block {
    Block {
        id: new_id(),
        container_id: board_id.to_owned(),
        block_type,
        title: title.to_owned(),
        number: Some(999),
        ..Block::default()
    }
}

fn card(board_id: &str, title: &str) -> Block {
    block(board_id, BlockType::Card, title)
}

/// Stores a new card and returns it as stored.
async fn new_card(store: &Store, board_id: &str, title: &str, user_id: &str) -> Block {
    let mut card = card(board_id, title);
    store
        .insert_block(&mut card, user_id)
        .await
        .expect("the card is stored");
    card
}

/// The number of the block as it is stored now, as one read alone has it.
async fn number_of(store: &Store, block_id: &str) -> Option<u32> {
    store
        .get_block(block_id)
        .await
        .expect("the block is there")
        .number
}

/// The numbers of the board's cards, by title, as the list of the board reads them;
/// and that the blocks of the board that are no cards have none.
async fn numbers_by_title(store: &Store, board_id: &str) -> Vec<(String, Option<u32>)> {
    let listed = store
        .get_listed_blocks(&BlockFilter::container(board_id), Previews::Without)
        .await
        .expect("the board's blocks are listed");
    let mut numbers = Vec::new();
    for listed in listed {
        let block = listed.block;
        if block.block_type == BlockType::Card {
            numbers.push((block.title, block.number));
        } else {
            assert_eq!(block.number, None, "{:?} has no number", block.block_type);
        }
    }
    numbers.sort();
    numbers
}

fn patch_fields(field: &str, value: Value) -> BlockPatch {
    let mut patch = BlockPatch::default();
    patch.updated_fields.insert(field.to_owned(), value);
    patch
}

#[tokio::test]
async fn the_cards_of_a_board_are_numbered_from_one_as_they_are_made_and_nothing_else_is() {
    let test = common::setup().await;
    let (user_id, workspace_id, board_id) = board_of_a_user(&test).await;
    let other_board = board(&test, &workspace_id, &user_id).await;

    let first = new_card(&test, &board_id, "a", &user_id).await;
    assert_eq!(first.number, Some(1), "not the number the request sent");
    let mut template = card(&board_id, "template");
    template
        .fields
        .insert("isTemplate".to_owned(), Value::Bool(true));
    let mut comment = block(&board_id, BlockType::Comment, "comment");
    comment.parent_id.clone_from(&first.id);
    // In the order they are given, whatever else they are.
    let mut batch = [
        card(&board_id, "b"),
        block(&board_id, BlockType::View, "view"),
        template,
        comment,
        card(&board_id, "c"),
    ];
    test.insert_blocks(&mut batch, &user_id)
        .await
        .expect("the batch is stored");
    let numbers: Vec<Option<u32>> = batch.iter().map(|block| block.number).collect();
    assert_eq!(numbers, [Some(2), None, None, None, Some(3)]);
    assert_eq!(
        new_card(&test, &other_board, "x", &user_id).await.number,
        Some(1),
        "each board has its own"
    );
    assert_eq!(
        numbers_by_title(&test, &board_id).await,
        [
            ("a".to_owned(), Some(1)),
            ("b".to_owned(), Some(2)),
            ("c".to_owned(), Some(3)),
            ("template".to_owned(), None),
        ]
    );

    // A card that is written again keeps its number, whatever the write says.
    let mut again = Block {
        title: "a again".to_owned(),
        number: Some(3),
        ..first.clone()
    };
    test.insert_block(&mut again, &user_id)
        .await
        .expect("the card is written again");
    assert_eq!(again.number, Some(1), "what the write gives back");
    let title = BlockPatch {
        title: Some("a patched".to_owned()),
        ..BlockPatch::default()
    };
    test.patch_block(&first.id, &title, &user_id)
        .await
        .expect("the card is patched");
    assert_eq!(number_of(&test, &first.id).await, Some(1));
    assert_eq!(
        new_card(&test, &board_id, "d", &user_id).await.number,
        Some(4)
    );

    let found = test.get_card_by_number(&board_id, 2).await.expect("asked");
    assert_eq!(found.map(|card| card.id), Some(batch[0].id.clone()));
    for (on, number) in [(&board_id, 5), (&board_id, 0), (&other_board, 2)] {
        let found = test.get_card_by_number(on, number).await.expect("asked");
        assert_eq!(found, None, "no card {number} on {on}");
    }
    test.teardown().await;
}

#[tokio::test]
async fn a_card_made_a_template_keeps_its_stored_number_and_restores_it_when_made_a_card_again() {
    let test = common::setup().await;
    let (user_id, _, board_id) = board_of_a_user(&test).await;
    let first = new_card(&test, &board_id, "card", &user_id).await;
    assert_eq!(first.number, Some(1));
    let mut fresh_template = card(&board_id, "fresh template");
    fresh_template
        .fields
        .insert("isTemplate".to_owned(), Value::Bool(true));
    test.insert_block(&mut fresh_template, &user_id)
        .await
        .expect("the fresh template is stored");
    assert_eq!(fresh_template.number, None);
    let mut view = block(&board_id, BlockType::View, "view");
    test.insert_block(&mut view, &user_id)
        .await
        .expect("the view is stored");
    assert_eq!(view.number, None);

    let to_template = patch_fields("isTemplate", Value::Bool(true));
    test.patch_block(&first.id, &to_template, &user_id)
        .await
        .expect("the card becomes a template");
    let as_template = test.get_block(&first.id).await.expect("stored");
    assert_eq!(
        as_template.number,
        Some(1),
        "DB blocks.number is preserved while it is a template"
    );
    assert_eq!(
        as_template.shown_number(),
        None,
        "a template hides its number from clients"
    );
    assert_eq!(
        test.get_card_by_number(&board_id, 1).await.expect("asked"),
        None,
        "get_card_by_number excludes templates"
    );

    // Creating a new card while #1 is a template takes #2 without colliding.
    let second = new_card(&test, &board_id, "second", &user_id).await;
    assert_eq!(second.number, Some(2));

    // Duplicating a template that holds a preserved number does not collide:
    // a copy takes a new number, and a template duplicate takes none.
    let copied = common::duplicate_card(&test, &board_id, &first.id, &user_id, DuplicateAs::Copy)
        .await
        .expect("copy from template");
    assert_eq!(copied[0].number, Some(3));
    let dup_template =
        common::duplicate_card(&test, &board_id, &first.id, &user_id, DuplicateAs::Template)
            .await
            .expect("template copy from template");
    assert_eq!(dup_template[0].number, None);

    // Deleting the template to trash and restoring it preserves its hidden number.
    test.delete_block(&first.id, &user_id)
        .await
        .expect("template deleted");
    let restored = test
        .undelete_block(&first.id, &user_id)
        .await
        .expect("template restored");
    assert_eq!(restored.number, Some(1));
    assert_eq!(restored.shown_number(), None);
    assert_eq!(
        test.get_card_by_number(&board_id, 1).await.expect("asked"),
        None
    );

    // Turning it back into an ordinary card restores #1 without consuming #4.
    let to_card = patch_fields("isTemplate", Value::Bool(false));
    test.patch_block(&first.id, &to_card, &user_id)
        .await
        .expect("the template becomes a card again");
    assert_eq!(number_of(&test, &first.id).await, Some(1));
    assert_eq!(
        test.get_card_by_number(&board_id, 1)
            .await
            .expect("asked")
            .map(|found| found.id),
        Some(first.id.clone())
    );

    // A template that was created without a number takes the next number (#4) when made a card.
    test.patch_block(&fresh_template.id, &to_card, &user_id)
        .await
        .expect("fresh template becomes a card");
    assert_eq!(number_of(&test, &fresh_template.id).await, Some(4));
    assert_eq!(number_of(&test, &view.id).await, None);
    assert_eq!(
        new_card(&test, &board_id, "next", &user_id).await.number,
        Some(5)
    );
    test.teardown().await;
}

#[tokio::test]
async fn a_board_deleted_with_a_template_holding_a_preserved_number_restores_cleanly() {
    let test = common::setup().await;
    let (user_id, _, board_id) = board_of_a_user(&test).await;
    let first = new_card(&test, &board_id, "first", &user_id).await;
    let second = new_card(&test, &board_id, "second", &user_id).await;
    assert_eq!((first.number, second.number), (Some(1), Some(2)));

    let to_template = patch_fields("isTemplate", Value::Bool(true));
    test.patch_block(&first.id, &to_template, &user_id)
        .await
        .expect("first becomes a template");
    test.delete_container(&board_id, &user_id)
        .await
        .expect("board deleted");
    test.undelete_container(&board_id, &user_id)
        .await
        .expect("board restored");

    let to_card = patch_fields("isTemplate", Value::Bool(false));
    test.patch_block(&first.id, &to_card, &user_id)
        .await
        .expect("first becomes a card again");
    assert_eq!(number_of(&test, &first.id).await, Some(1));
    assert_eq!(
        new_card(&test, &board_id, "third", &user_id).await.number,
        Some(3),
        "restoring #1 did not consume #3"
    );
    test.teardown().await;
}

#[tokio::test]
async fn a_number_is_never_given_again_and_what_comes_back_from_the_trash_has_its_own() {
    let test = common::setup().await;
    let (user_id, _, board_id) = board_of_a_user(&test).await;
    let mut cards = Vec::new();
    for title in ["1", "2", "3"] {
        cards.push(new_card(&test, &board_id, title, &user_id).await);
    }
    let mut comment = block(&board_id, BlockType::Comment, "on 2");
    comment.parent_id.clone_from(&cards[1].id);
    test.insert_block(&mut comment, &user_id)
        .await
        .expect("the comment is stored");

    // The newest goes for good: its number is not the next one.
    test.delete_block(&cards[2].id, &user_id)
        .await
        .expect("deleted");
    test.purge_trash(i64::MAX).await.expect("purged");
    assert_eq!(
        new_card(&test, &board_id, "4", &user_id).await.number,
        Some(4)
    );
    test.delete_block(&cards[1].id, &user_id)
        .await
        .expect("deleted");
    assert_eq!(
        test.get_card_by_number(&board_id, 2).await.expect("asked"),
        None
    );
    assert_eq!(
        new_card(&test, &board_id, "5", &user_id).await.number,
        Some(5)
    );
    let restored = test
        .undelete_block(&cards[1].id, &user_id)
        .await
        .expect("restored");
    assert_eq!(restored.number, Some(2), "its own");
    assert_eq!(
        number_of(&test, &comment.id).await,
        None,
        "a comment has none"
    );
    test.teardown().await;
}

#[tokio::test]
async fn a_board_comes_back_from_the_trash_with_its_cards_and_its_next_number() {
    let test = common::setup().await;
    let (user_id, _, board_id) = board_of_a_user(&test).await;
    let mut cards = Vec::new();
    for title in ["1", "2", "3"] {
        cards.push(new_card(&test, &board_id, title, &user_id).await);
    }
    let mut template = card(&board_id, "template");
    template
        .fields
        .insert("isTemplate".to_owned(), Value::Bool(true));
    test.insert_block(&mut template, &user_id)
        .await
        .expect("the template is stored");
    test.delete_block(&cards[2].id, &user_id)
        .await
        .expect("the newest card is deleted");
    test.delete_container(&board_id, &user_id)
        .await
        .expect("the board is deleted");
    test.undelete_container(&board_id, &user_id)
        .await
        .expect("the board is restored");
    assert_eq!(
        numbers_by_title(&test, &board_id).await,
        [
            ("1".to_owned(), Some(1)),
            ("2".to_owned(), Some(2)),
            ("template".to_owned(), None)
        ]
    );
    assert_eq!(
        new_card(&test, &board_id, "4", &user_id).await.number,
        Some(4),
        "not the number of the card that is still in the trash"
    );
    let restored = test
        .undelete_block(&cards[2].id, &user_id)
        .await
        .expect("the card is restored");
    assert_eq!(restored.number, Some(3));
    test.teardown().await;
}

/// A card that went to the trash before cards had numbers, or with its board, has none
/// there: it gets the board's next numbers when it comes back, those made first first.
#[tokio::test]
async fn a_card_from_the_trash_of_before_numbers_gets_the_next_ones() {
    let test = common::setup().await;
    let (user_id, _, board_id) = board_of_a_user(&test).await;
    let mut cards = Vec::new();
    for title in ["1", "2", "3"] {
        cards.push(new_card(&test, &board_id, title, &user_id).await);
    }
    test.delete_block(&cards[0].id, &user_id)
        .await
        .expect("deleted");
    test.delete_container(&board_id, &user_id)
        .await
        .expect("the board is deleted");
    let without_numbers = match common::db_type() {
        DbType::Sqlite => format!(
            "UPDATE deleted_blocks SET block = json_remove(block, '$.number') \
             WHERE container_id = '{board_id}'; \
             UPDATE deleted_containers SET next_card_number = 1 WHERE id = '{board_id}';"
        ),
        DbType::Postgres => format!(
            "UPDATE deleted_blocks SET block = (block::jsonb - 'number')::json \
             WHERE container_id = '{board_id}'; \
             UPDATE deleted_containers SET next_card_number = 1 WHERE id = '{board_id}';"
        ),
    };
    common::execute_raw(&test.database_connection(), &without_numbers).await;

    test.undelete_container(&board_id, &user_id)
        .await
        .expect("the board is restored");
    assert_eq!(
        numbers_by_title(&test, &board_id).await,
        [("2".to_owned(), Some(1)), ("3".to_owned(), Some(2))]
    );
    let restored = test
        .undelete_block(&cards[0].id, &user_id)
        .await
        .expect("the card is restored");
    assert_eq!(
        restored.number,
        Some(3),
        "after those that came back before it"
    );
    assert_eq!(
        new_card(&test, &board_id, "4", &user_id).await.number,
        Some(4)
    );
    test.teardown().await;
}

#[tokio::test]
async fn a_copy_and_an_import_are_new_cards_with_new_numbers() {
    let test = common::setup().await;
    let (user_id, workspace_id, board_id) = board_of_a_user(&test).await;
    let first = new_card(&test, &board_id, "1", &user_id).await;
    new_card(&test, &board_id, "2", &user_id).await;

    let copies = common::duplicate_card(&test, &board_id, &first.id, &user_id, DuplicateAs::Copy)
        .await
        .expect("the card is copied");
    assert_eq!(copies[0].number, Some(3), "the copy is a new card");
    let templates =
        common::duplicate_card(&test, &board_id, &first.id, &user_id, DuplicateAs::Template)
            .await
            .expect("a template is made of the card");
    assert_eq!(templates[0].number, None, "a template has none");
    assert_eq!(
        new_card(&test, &board_id, "4", &user_id).await.number,
        Some(4)
    );

    let (copy, _) = test
        .duplicate_container(
            &board_id,
            &user_id,
            Some(&workspace_id),
            DuplicateAs::Copy,
            &TITLES,
            CopyChanges::default(),
        )
        .await
        .expect("the board is copied");
    let copied_board = copy.containers[0].id.clone();
    // The copy numbers its cards from one in the order the board lists them, which
    // here is the order they were numbered in: each copied card has its card's number,
    // and the template none. A copy made in another order gives "1" another number.
    let on_the_board = numbers_by_title(&test, &board_id).await;
    assert_eq!(numbers_by_title(&test, &copied_board).await, on_the_board);

    // An import makes its boards with what the archive says, which has numbers.
    let imported_board = Container {
        id: new_id(),
        workspace_id: workspace_id.clone(),
        visibility: ContainerVisibility::Private,
        ..Container::default()
    };
    let mut archive = ContainersAndBlocks {
        blocks: vec![
            Block {
                number: Some(7),
                ..card(&imported_board.id, "seventh")
            },
            Block {
                number: Some(2),
                ..card(&imported_board.id, "second")
            },
        ],
        containers: vec![imported_board.clone()],
    };
    test.create_containers(
        NewContainers::whole(&mut archive)
            .body_files(NewBodyFiles::Unchecked)
            .creator(Creator::Nobody),
        &user_id,
    )
    .await
    .map_err(StoreError::from)
    .expect("the archive is stored");
    assert_eq!(
        numbers_by_title(&test, &imported_board.id).await,
        [
            ("second".to_owned(), Some(2)),
            ("seventh".to_owned(), Some(1))
        ],
        "in the order they come in"
    );
    test.teardown().await;
}

/// How many of each write a round of [`writers_at_once_never_take_one_number`] makes.
const SINGLE_WRITES: usize = 4;
const BATCHES: usize = 2;
const BATCH_SIZE: usize = 3;
const COPIES: usize = 2;
const ROUNDS: usize = 3;

/// Writers at once on one board, every way a card is stored: alone, in a batch (as the
/// block routes and the archive of a board store theirs), copied, and back from the
/// trash; and a board made with its cards, as an import makes one, beside them. They
/// queue on the board's lock: no two cards have one number, none is left out, and the
/// next is the one after the last.
#[tokio::test]
async fn writers_at_once_never_take_one_number() {
    let test = common::setup().await;
    let (user_id, workspace_id, board_id) = board_of_a_user(&test).await;
    let seed = new_card(&test, &board_id, "seed", &user_id).await;
    let mut trashed = Vec::new();
    for round in 0..ROUNDS {
        let card = new_card(&test, &board_id, &format!("trashed {round}"), &user_id).await;
        test.delete_block(&card.id, &user_id)
            .await
            .expect("deleted");
        trashed.push(card);
    }

    let mut imported = Vec::new();
    for (round, trashed) in trashed.iter().enumerate() {
        let writers = SINGLE_WRITES + BATCHES + COPIES + 2;
        let start = Arc::new(Barrier::new(writers));
        let mut written = JoinSet::new();
        let at_once = |written: &mut JoinSet<Vec<Block>>, write| {
            let (store, start, user_id) = (test.store.clone(), Arc::clone(&start), user_id.clone());
            let board_id = board_id.clone();
            written.spawn(async move {
                start.wait().await;
                write_once(&store, &board_id, &user_id, write).await
            });
        };
        for _ in 0..SINGLE_WRITES {
            at_once(&mut written, Write::Single);
        }
        for _ in 0..BATCHES {
            at_once(&mut written, Write::Batch);
        }
        for _ in 0..COPIES {
            at_once(&mut written, Write::Copy(seed.id.clone()));
        }
        at_once(&mut written, Write::Restore(trashed.id.clone()));
        let import = Container {
            id: new_id(),
            workspace_id: workspace_id.clone(),
            visibility: ContainerVisibility::Private,
            ..Container::default()
        };
        imported.push(import.id.clone());
        at_once(&mut written, Write::Import(Box::new(import)));
        let stored: Vec<Block> = written.join_all().await.into_iter().flatten().collect();
        assert_eq!(
            stored.len(),
            SINGLE_WRITES + BATCHES * BATCH_SIZE + COPIES + 1 + BATCH_SIZE,
            "round {round}"
        );
    }

    let numbers: Vec<u32> = numbers_by_title(&test, &board_id)
        .await
        .into_iter()
        .map(|(title, number)| number.unwrap_or_else(|| panic!("{title} has a number")))
        .collect();
    let distinct: BTreeSet<u32> = numbers.iter().copied().collect();
    assert_eq!(
        distinct.len(),
        numbers.len(),
        "no number twice: {numbers:?}"
    );
    let all = 1 + ROUNDS * (1 + SINGLE_WRITES + BATCHES * BATCH_SIZE + COPIES);
    let expected: BTreeSet<u32> = (1..=u32::try_from(all).expect("a few")).collect();
    assert_eq!(distinct, expected, "none left out");
    for (round, card) in trashed.iter().enumerate() {
        assert_eq!(
            number_of(&test, &card.id).await,
            Some(u32::try_from(round + 2).expect("a few")),
            "back with its own"
        );
    }
    assert_eq!(
        new_card(&test, &board_id, "last", &user_id).await.number,
        Some(u32::try_from(all + 1).expect("a few"))
    );
    for board_id in imported {
        let mut numbers: Vec<Option<u32>> = numbers_by_title(&test, &board_id)
            .await
            .into_iter()
            .map(|(_, number)| number)
            .collect();
        numbers.sort();
        assert_eq!(numbers, [Some(1), Some(2), Some(3)], "a board of its own");
    }
    test.teardown().await;
}

/// One write of [`writers_at_once_never_take_one_number`].
enum Write {
    Single,
    Batch,
    Copy(String),
    Restore(String),
    Import(Box<Container>),
}

/// Makes the write and returns the cards it stored.
async fn write_once(store: &Store, board_id: &str, user_id: &str, write: Write) -> Vec<Block> {
    match write {
        Write::Single => vec![new_card(store, board_id, "single", user_id).await],
        Write::Batch => {
            let mut batch: Vec<Block> =
                (0..BATCH_SIZE).map(|_| card(board_id, "batched")).collect();
            store
                .insert_blocks(&mut batch, user_id)
                .await
                .expect("the batch is stored");
            batch
        }
        Write::Copy(card_id) => {
            common::duplicate_card(store, board_id, &card_id, user_id, DuplicateAs::Copy)
                .await
                .expect("the card is copied")
        }
        Write::Restore(card_id) => vec![
            store
                .undelete_block(&card_id, user_id)
                .await
                .expect("the card is restored"),
        ],
        Write::Import(container) => {
            let mut archive = ContainersAndBlocks {
                blocks: (0..BATCH_SIZE)
                    .map(|_| card(&container.id, "imported"))
                    .collect(),
                containers: vec![*container],
            };
            store
                .create_containers(
                    NewContainers::whole(&mut archive)
                        .body_files(NewBodyFiles::Unchecked)
                        .creator(Creator::Nobody),
                    user_id,
                )
                .await
                .map_err(StoreError::from)
                .expect("the archive is stored");
            archive.blocks
        }
    }
}
