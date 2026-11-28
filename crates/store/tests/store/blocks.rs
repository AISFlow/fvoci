//! Integration tests for block storage, patching, deletion, trash restoration, subtree
//! queries, pagination, concurrent writers, and property validation.
//!
//! Every user, workspace, board, and block uses a generated UUID identifier, and
//! referenced parents and boards are stored first. Deleted blocks move to the trash
//! tables rather than keeping a full per-edit history.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fvoci_model::{
    BLOCK_FIELDS_MAX_CHARS, BLOCK_TITLE_MAX_CHARS, BODY_VERSION, Block, BlockError, BlockPatch,
    BlockPatchBatch, BlockType, Body, BodyChange, ContainersAndBlocksError, FileRecord, JsonMap,
    Subscription, new_id,
};
use fvoci_store::{BlockFilter, DbType, DeletedBlock, Paging, Store, StoreError};
use serde_json::{Value, json};
use sqlx::Connection as _;
use sqlx::sqlite::SqliteConnection;
use tokio::sync::Barrier;
use tokio::task::JoinSet;

use crate::common;

mod edges;
mod races;
mod trash;
mod writes;

/// Sample block tree (name, parent name, block type) for subtree and cascade-delete tests.
const SUBTREE_SAMPLE: [(&str, &str, &str); 6] = [
    ("parent", "", "card"),
    ("child1", "parent", "card"),
    ("child2", "parent", "card"),
    ("grandchild1", "child1", "attachment"),
    ("grandchild2", "child2", "card"),
    ("greatgrandchild1", "grandchild2", "attachment"),
];

/// Five sample blocks of varying types used by pagination and filter tests.
const TYPED_SAMPLE: [(&str, &str, &str); 5] = [
    ("block1", "", "card"),
    ("block2", "block1", "card"),
    ("block3", "block1", "card"),
    ("block4", "block1", "attachment"),
    ("block5", "block2", "card"),
];

/// What has to exist before a block does: a user to write it and a board to hold it,
/// which in turn is a workspace's.
#[expect(
    clippy::struct_field_names,
    reason = "the members are identifiers, named like the variables that hold them"
)]
pub(crate) struct Fixture {
    pub(crate) user_id: String,
    pub(crate) workspace_id: String,
    pub(crate) board_id: String,
}

/// Stores a user, a workspace and a board of that workspace.
pub(crate) async fn fixture(store: &Store) -> Fixture {
    let user_id = common::user(store, "test-user").await;
    let workspace_id = common::workspace(store).await;
    let board_id = another_board(store, &workspace_id, &user_id).await;
    Fixture {
        user_id,
        workspace_id,
        board_id,
    }
}

/// Stores one more board of a workspace and returns its identifier.
pub(crate) async fn another_board(store: &Store, workspace_id: &str, user_id: &str) -> String {
    common::create_test_boards(store, workspace_id, user_id, 1)
        .await
        .into_iter()
        .next()
        .expect("one board was asked for")
        .id
}

/// Stores `N` users, `user-1`, `user-2` and so on, and returns their identifiers.
async fn users<const N: usize>(store: &Store) -> [String; N] {
    let mut user_ids = Vec::with_capacity(N);
    for number in 1..=N {
        user_ids.push(common::user(store, &format!("user-{number}")).await);
    }
    user_ids
        .try_into()
        .expect("one identifier per user that was stored")
}

/// A block with the given identifier and board and nothing else.
/// Every insert overwrites `modified_by`, so it is only set when a test checks it.
pub(crate) fn block(id: &str, board_id: &str) -> Block {
    Block {
        id: id.to_owned(),
        container_id: board_id.to_owned(),
        ..Block::default()
    }
}

/// Blocks of a board with new identifiers, from the name, the parent's name and the type
/// of each. A parent comes before its children; the empty name means no parent.
///
/// The name of a block is its title, and [`id_of`] finds its identifier by it.
pub(crate) fn named_blocks(board_id: &str, specs: &[(&str, &str, &str)]) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::with_capacity(specs.len());
    for &(name, parent, block_type) in specs {
        let parent_id = if parent.is_empty() {
            String::new()
        } else {
            id_of(&blocks, parent).to_owned()
        };
        blocks.push(Block {
            parent_id,
            block_type: block_type.parse().expect("known block type"),
            title: name.to_owned(),
            ..block(&new_id(), board_id)
        });
    }
    blocks
}

/// The identifier of the block that [`named_blocks`] made under the given name.
#[track_caller]
pub(crate) fn id_of<'a>(blocks: &'a [Block], name: &str) -> &'a str {
    blocks
        .iter()
        .find(|block| block.title == name)
        .map_or_else(|| panic!("no block is named {name}"), |block| &block.id)
}

/// The names of blocks that [`named_blocks`] made, sorted.
pub(crate) fn sorted_names(blocks: &[Block]) -> Vec<&str> {
    let mut names: Vec<&str> = blocks.iter().map(|block| block.title.as_str()).collect();
    names.sort_unstable();
    names
}

/// The blocks of [`SUBTREE_SAMPLE`] on a board.
fn subtree_sample_blocks(board_id: &str) -> Vec<Block> {
    named_blocks(board_id, &SUBTREE_SAMPLE)
}

/// The blocks of [`TYPED_SAMPLE`] on a board.
fn typed_sample_blocks(board_id: &str) -> Vec<Block> {
    named_blocks(board_id, &TYPED_SAMPLE)
}

/// The three blocks that `delete_block` and `undelete_block` insert.
fn three_blocks(board_id: &str) -> Vec<Block> {
    named_blocks(
        board_id,
        &[
            ("block1", "", "card"),
            ("block2", "", "card"),
            ("block3", "", "card"),
        ],
    )
}

/// Four fields that together exceed the size limit of a block's fields.
fn oversized_fields() -> JsonMap {
    [("one", "1"), ("two", "2"), ("three", "3"), ("four", "4")]
        .into_iter()
        .map(|(key, digit)| {
            let value = Value::String(digit.repeat(BLOCK_FIELDS_MAX_CHARS / 4));
            (key.to_owned(), value)
        })
        .collect()
}

/// The fields of a block, from a JSON object literal.
pub(crate) fn fields(object: Value) -> JsonMap {
    serde_json::from_value(object).expect("the literal is a JSON object")
}

/// A patch that sets the title.
pub(crate) fn title_patch(title: &str) -> BlockPatch {
    BlockPatch {
        title: Some(title.to_owned()),
        ..BlockPatch::default()
    }
}

/// Owned strings from string slices: a list of identifiers, or of field names.
fn strings<const N: usize>(strings: [&str; N]) -> Vec<String> {
    strings.into_iter().map(str::to_owned).collect()
}

pub(crate) fn now_millis() -> i64 {
    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is past 1970");
    i64::try_from(since_epoch.as_millis()).expect("the time fits 64 bits")
}

/// `assert.WithinDurationf(expected, actual, 1*time.Second, ...)`.
#[track_caller]
fn assert_within_a_second(expected: i64, actual: i64, what: &str) {
    assert!(
        (actual - expected).abs() <= 1000,
        "{what} should be the current time: expected about {expected}, got {actual}"
    );
}

/// `require.ErrorAs(t, err, &nf)` with `nf` a `*model.ErrNotFound`.
#[track_caller]
fn assert_not_found(error: &StoreError) {
    assert!(
        matches!(error, StoreError::NotFound(_)),
        "expected a not-found error, got {error:?}"
    );
}

/// `require.ErrorIs(t, err, <a block validation error>)`.
#[track_caller]
fn assert_invalid_block(error: &StoreError, expected: &BlockError) {
    assert!(
        matches!(error, StoreError::InvalidBlock { source, .. } if source == expected),
        "expected the validation error {expected:?}, got {error:?}"
    );
}

/// Asserts that a write was refused because `text` is no identifier.
#[track_caller]
fn assert_invalid_id(error: &StoreError, text: &str) {
    assert!(
        matches!(error, StoreError::InvalidId(refused) if refused == text),
        "expected {text:?} to be refused as no identifier, got {error:?}"
    );
}

/// `GetBlocksForBoard` followed by `require.NoError`.
pub(crate) async fn board_blocks(store: &Store, board_id: &str) -> Vec<Block> {
    store
        .get_blocks(&BlockFilter::container(board_id))
        .await
        .expect("the blocks of a board are read")
}

/// `GetBlock` followed by `require.NoError` and `require.NotNil`.
pub(crate) async fn existing_block(store: &Store, block_id: &str) -> Block {
    store
        .get_block(block_id)
        .await
        .unwrap_or_else(|error| panic!("block {block_id} should exist: {error:?}"))
}

/// `GetBlock` followed by `require.ErrorAs(t, err, &nf)` and `require.Nil(t, block)`.
pub(crate) async fn assert_no_block(store: &Store, block_id: &str) {
    match store.get_block(block_id).await {
        Ok(block) => panic!("block {block_id} should not exist, got {block:?}"),
        Err(error) => assert_not_found(&error),
    }
}

/// Inserts `count` top-level cards on `board_id`.
async fn create_test_cards(
    store: &Store,
    user_id: &str,
    board_id: &str,
    count: usize,
) -> Vec<Block> {
    let mut cards = Vec::with_capacity(count);
    for index in 0..count {
        let mut card = Block {
            parent_id: String::new(),
            block_type: BlockType::Card,
            created_by: user_id.to_owned(),
            title: format!("card {index}"),
            ..block(&new_id(), board_id)
        };
        store
            .insert_block(&mut card, user_id)
            .await
            .expect("a test card is inserted");
        cards.push(card);
    }
    cards
}

/// Inserts `count` child blocks under `card_id`.
async fn create_test_blocks_for_card(store: &Store, card_id: &str, count: usize) -> Vec<Block> {
    let card = existing_block(store, card_id).await;
    assert_eq!(
        card.block_type,
        BlockType::Card,
        "content blocks are created for a card"
    );

    let mut blocks = Vec::with_capacity(count);
    for index in 0..count {
        let mut text = Block {
            parent_id: card.id.clone(),
            block_type: BlockType::Attachment,
            created_by: card.created_by.clone(),
            title: format!("text {index}"),
            ..block(&new_id(), &card.container_id)
        };
        store
            .insert_block(&mut text, &card.created_by)
            .await
            .expect("a test content block is inserted");
        blocks.push(text);
    }
    blocks
}

/// A second connection to the SQLite database of a test, for what the store has no
/// operation for.
async fn sqlite_connection(test: &common::TestStore) -> SqliteConnection {
    SqliteConnection::connect(&format!("sqlite://{}", test.database_connection()))
        .await
        .expect("the test database opens a second time")
}

/// Stores the record of a file that a board has under the given stored name, and
/// returns it.
async fn save_file(store: &Store, board_id: &str, stored_name: &str) -> FileRecord {
    let file_record = FileRecord {
        id: new_id(),
        container_id: board_id.to_owned(),
        stored_name: stored_name.to_owned(),
        name: format!("uploaded as {stored_name}"),
        size: 1,
        path: format!("{board_id}/{stored_name}"),
        created_at: now_millis(),
        ..FileRecord::default()
    };
    store
        .save_file_record(&file_record)
        .await
        .expect("the record of a file is stored");
    file_record
}

/// Asserts that every one of the file records is stored as it was saved.
async fn assert_file_records_are(store: &Store, files: &[FileRecord], when: &str) {
    for file in files {
        let stored = store
            .get_file_record(&file.container_id, &file.stored_name)
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "file {} of board {} should have a record {when}: {error:?}",
                    file.stored_name, file.container_id
                )
            });
        assert_eq!(
            &stored, file,
            "the record of file {} is as it was saved, {when}",
            file.stored_name
        );
    }
}

/// `get_deleted_block` followed by a check that the block is in the trash.
pub(crate) async fn block_in_trash(store: &Store, block_id: &str) -> DeletedBlock {
    store
        .get_deleted_block(block_id)
        .await
        .unwrap_or_else(|error| panic!("block {block_id} should be in the trash: {error:?}"))
}

/// Asserts that no block of the identifier is in the trash.
async fn assert_not_in_trash(store: &Store, block_id: &str) {
    match store.get_deleted_block(block_id).await {
        Ok(deleted) => panic!("block {block_id} should not be in the trash, got {deleted:?}"),
        Err(error) => assert_not_found(&error),
    }
}

/// `undelete_block` followed by a check that it found nothing to restore.
async fn assert_undelete_finds_nothing(store: &Store, block_id: &str, user_id: &str) {
    match store.undelete_block(block_id, user_id).await {
        Ok(block) => panic!("block {block_id:?} should not be restored, got {block:?}"),
        Err(error) => assert_not_found(&error),
    }
}

/// Inserting a block.
#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "sequential block-insertion cases share one store fixture"
)]
async fn insert_block() {
    let test = common::setup().await;
    let store = &test.store;
    let Fixture {
        user_id: owner_id,
        workspace_id,
        board_id,
    } = fixture(store).await;
    let board_id = board_id.as_str();
    let other_board_id = another_board(store, &workspace_id, &owner_id).await;
    let [user1, user2, user3, user4, user5, user6] = users(store).await;

    let initial_count = board_blocks(store, board_id).await.len();

    // "valid block"
    let test_id = new_id();
    let mut valid = block(&test_id, board_id);
    store
        .insert_block(&mut valid, &user1)
        .await
        .expect("a valid block is inserted");
    let blocks = board_blocks(store, board_id).await;
    assert_eq!(
        blocks.len(),
        initial_count + 1,
        "the board has one more block: {blocks:?}"
    );

    // "invalid rootid"
    let mut without_board = block(&test_id, "");
    let error = store
        .insert_block(&mut without_board, &user1)
        .await
        .expect_err("a block without a board is rejected");
    assert_invalid_block(&error, &BlockError::EmptyContainerId);
    let blocks = board_blocks(store, board_id).await;
    assert_eq!(
        blocks.len(),
        initial_count + 1,
        "the rejected block changed nothing: {blocks:?}"
    );

    // Unserializable field values are prevented by type (`JsonMap`).

    // "block with title too large"
    let mut long_title = Block {
        title: "A".repeat(BLOCK_TITLE_MAX_CHARS + 1),
        ..block(&test_id, board_id)
    };
    let error = store
        .insert_block(&mut long_title, &user1)
        .await
        .expect_err("a title over the limit is rejected");
    assert_invalid_block(&error, &BlockError::TitleSizeLimitExceeded);

    // "block with aggregated fields size too large"
    let mut large_fields = Block {
        fields: oversized_fields(),
        ..block(&test_id, board_id)
    };
    let error = store
        .insert_block(&mut large_fields, &user2)
        .await
        .expect_err("fields over the limit are rejected");
    assert_invalid_block(&error, &BlockError::FieldsSizeLimitExceeded);

    // An empty ID is rejected; then insert a valid block with a generated ID.
    let mut without_id = block("", board_id);
    let error = store
        .insert_block(&mut without_id, &user2)
        .await
        .expect_err("a block without an identifier is refused");
    assert_invalid_id(&error, "");
    let blocks = board_blocks(store, board_id).await;
    assert_eq!(
        blocks.len(),
        initial_count + 1,
        "the refused block changed nothing: {blocks:?}"
    );

    let mut new_block = block(&new_id(), board_id);
    store
        .insert_block(&mut new_block, &user2)
        .await
        .expect("a new block is inserted");
    assert_eq!(
        new_block.created_by, user2,
        "the creator of a new block is the inserting user"
    );

    // "update existing block"
    let id2 = new_id();
    let mut old = Block {
        title: "Old Title".to_owned(),
        ..block(&id2, &other_board_id)
    };
    store
        .insert_block(&mut old, &user2)
        .await
        .expect("the block is inserted");
    assert_eq!(
        old.created_by, user2,
        "the creator of a new block is the inserting user"
    );

    let mut new = Block {
        created_by: user3.clone(),
        title: "New Title".to_owned(),
        ..block(&id2, &other_board_id)
    };
    store
        .insert_block(&mut new, &user4)
        .await
        .expect("the block is updated");
    assert_eq!(
        new.created_by, user3,
        "the creator given for an existing block is not altered"
    );
    assert_eq!(new.title, "New Title", "the title is as given");
    // Updating an existing block preserves its original `created_by`.
    let stored = existing_block(store, &id2).await;
    assert_eq!(
        (stored.created_by.as_str(), stored.title.as_str()),
        (user2.as_str(), "New Title"),
        "an update changes the title and not the stored creator"
    );

    // "update existing block with title too large"
    let id3 = new_id();
    let mut existing = Block {
        created_by: user3.clone(),
        title: "New Title".to_owned(),
        ..block(&id3, &other_board_id)
    };
    store
        .insert_block(&mut existing, &user3)
        .await
        .expect("the block is inserted");
    assert_eq!(
        existing.created_by, user3,
        "the creator of a new block is the inserting user"
    );

    let mut long_title = Block {
        created_by: user3.clone(),
        title: "A".repeat(BLOCK_TITLE_MAX_CHARS + 1),
        ..block(&id3, &other_board_id)
    };
    let error = store
        .insert_block(&mut long_title, &user3)
        .await
        .expect_err("an update with a title over the limit is rejected");
    assert_invalid_block(&error, &BlockError::TitleSizeLimitExceeded);

    // "update existing block with aggregated fields size too large"
    let mut existing = Block {
        created_by: user3.clone(),
        title: "New Title".to_owned(),
        ..block(&id3, &other_board_id)
    };
    store
        .insert_block(&mut existing, &user3)
        .await
        .expect("the block is written again");
    assert_eq!(
        existing.created_by, user3,
        "the creator given for an existing block is not altered"
    );

    let mut large_fields = Block {
        created_by: user3.clone(),
        fields: oversized_fields(),
        ..block(&id3, &other_board_id)
    };
    let error = store
        .insert_block(&mut large_fields, &user3)
        .await
        .expect_err("an update with fields over the limit is rejected");
    assert_invalid_block(&error, &BlockError::FieldsSizeLimitExceeded);

    // "data tamper attempt"
    insert_a_block_with_tampered_data(store, &other_board_id, [user1, user5, user6]).await;
    test.teardown().await;
}

/// The last subtest of [`insert_block`], "data tamper attempt": `user1` inserts a block
/// that names another creator and modifier (`user5`, `user6`) and other times.
async fn insert_a_block_with_tampered_data(
    store: &Store,
    other_board_id: &str,
    [user1, user5, user6]: [String; 3],
) {
    // Fixed UTC timestamps: `1990-01-01T01:00:00Z` and `1990-01-02T01:00:00Z`.
    let created_at = 631_155_600_000;
    let updated_at = 631_242_000_000;
    let id10 = new_id();
    let mut tampered = Block {
        title: "Old Title".to_owned(),
        created_at,
        updated_at,
        created_by: user5,
        modified_by: user6,
        ..block(&id10, other_board_id)
    };
    store
        .insert_block(&mut tampered, &user1)
        .await
        .expect("the block is inserted");
    let expected_time = now_millis();

    let retrieved = existing_block(store, &id10).await;
    assert_eq!(
        retrieved.container_id, other_board_id,
        "the board is as given"
    );
    assert_eq!(
        retrieved.created_by, user1,
        "the creator is the inserting user, not the one given"
    );
    assert_eq!(
        retrieved.modified_by, user1,
        "the modifier is the inserting user, not the one given"
    );
    assert_within_a_second(expected_time, retrieved.created_at, "create time");
    assert_within_a_second(expected_time, retrieved.updated_at, "update time");
    // Caller-supplied block timestamps are overwritten with what was stored.
    assert_eq!(
        (tampered.created_at, tampered.updated_at),
        (retrieved.created_at, retrieved.updated_at),
        "the argument has the times that were stored, not the ones given"
    );
}

/// Inserting several blocks at once.
#[tokio::test]
async fn insert_blocks() {
    let test = common::setup().await;
    let store = &test.store;
    let Fixture {
        user_id, board_id, ..
    } = fixture(store).await;

    let initial_count = board_blocks(store, &board_id).await.len();

    // "invalid block"
    let block_id = new_id();
    let mut new_blocks = [block(&block_id, &board_id), block(&block_id, "")];
    let error = store
        .insert_blocks(&mut new_blocks, &user_id)
        .await
        .expect_err("a batch with an invalid block is rejected");
    assert_invalid_block(&error, &BlockError::EmptyContainerId);

    let blocks = board_blocks(store, &board_id).await;
    assert_eq!(
        blocks.len(),
        initial_count,
        "no blocks should have been inserted: {blocks:?}"
    );

    test.teardown().await;
}

/// Patching a block.
#[tokio::test]
async fn patch_block() {
    let test = common::setup().await;
    let store = &test.store;
    let Fixture { board_id, .. } = fixture(store).await;
    let board_id = board_id.as_str();
    let [user1, user2] = users(store).await;

    let block_id = new_id();
    let mut block = Block {
        title: "oldTitle".to_owned(),
        fields: fields(json!({ "test": "test value", "test2": "test value 2" })),
        ..block(&block_id, board_id)
    };
    store
        .insert_block(&mut block, &user1)
        .await
        .expect("the block is inserted");

    let initial_count = board_blocks(store, board_id).await.len();

    // Unknown UUID and non-UUID text both return not-found.
    for missing in [new_id().as_str(), "invalid-block-id"] {
        let error = store
            .patch_block(missing, &BlockPatch::default(), &user1)
            .await
            .expect_err("a missing block cannot be patched");
        assert_not_found(&error);
        assert!(
            error.is_not_found(),
            "the error counts as not found: {error:?}"
        );
    }
    let blocks = board_blocks(store, board_id).await;
    assert_eq!(
        blocks.len(),
        initial_count,
        "the failed patch changed nothing: {blocks:?}"
    );

    // Patch field values are typed as `JsonMap` and always serialize to JSON.

    // "update block fields"
    store
        .patch_block(&block_id, &title_patch("New title"), &user2)
        .await
        .expect("the title is patched");
    let retrieved = existing_block(store, &block_id).await;
    assert_eq!(
        retrieved.modified_by, user2,
        "the modifier is the patching user"
    );
    assert_eq!(retrieved.title, "New title", "the title is the patched one");

    // "update block custom fields"
    let patch = BlockPatch {
        updated_fields: fields(json!({ "test": "new test value", "test3": "new value" })),
        ..BlockPatch::default()
    };
    store
        .patch_block(&block_id, &patch, &user2)
        .await
        .expect("the fields are patched");
    let retrieved = existing_block(store, &block_id).await;
    assert_eq!(
        retrieved.modified_by, user2,
        "the modifier is the patching user"
    );
    assert_eq!(
        retrieved.fields.get("test"),
        Some(&json!("new test value")),
        "an existing field is replaced"
    );
    assert_eq!(
        retrieved.fields.get("test2"),
        Some(&json!("test value 2")),
        "a field the patch does not name is kept"
    );
    assert_eq!(
        retrieved.fields.get("test3"),
        Some(&json!("new value")),
        "a new field is added"
    );

    // "remove block custom fields"
    let patch = BlockPatch {
        deleted_fields: strings(["test", "test3", "test100"]),
        ..BlockPatch::default()
    };
    store
        .patch_block(&block_id, &patch, &user2)
        .await
        .expect("the fields are removed");
    let retrieved = existing_block(store, &block_id).await;
    assert_eq!(
        retrieved.modified_by, user2,
        "the modifier is the patching user"
    );
    assert_eq!(
        retrieved.fields.get("test"),
        None,
        "a deleted field is gone"
    );
    assert_eq!(
        retrieved.fields.get("test2"),
        Some(&json!("test value 2")),
        "a field the patch does not name is kept"
    );
    assert_eq!(
        retrieved.fields.get("test3"),
        None,
        "a deleted field is gone"
    );

    test.teardown().await;
}

/// A block's `updated_at` moves on with every change, even when the clock has not: a
/// patch made against the time a block was read at must see every change after it.
#[tokio::test]
async fn updated_at_moves_on_with_every_change() {
    let test = common::setup().await;
    let store = &test.store;
    let Fixture { board_id, .. } = fixture(store).await;
    let [user_id] = users(store).await;
    let block_id = new_id();
    let mut block = block(&block_id, &board_id);
    store
        .insert_block(&mut block, &user_id)
        .await
        .expect("the block is inserted");

    // A time an hour ahead of the clock, as a block written by a clock that ran fast.
    let ahead = existing_block(store, &block_id).await.updated_at + 3_600_000;
    common::execute_raw(
        &test.database_connection(),
        &format!("UPDATE blocks SET updated_at = {ahead} WHERE id = '{block_id}'"),
    )
    .await;
    store
        .patch_block(&block_id, &title_patch("once"), &user_id)
        .await
        .expect("the block is patched");
    let once = existing_block(store, &block_id).await.updated_at;
    assert_eq!(
        once,
        ahead + 1,
        "the patch moved updated_at past the stored one"
    );

    store
        .patch_block(&block_id, &title_patch("twice"), &user_id)
        .await
        .expect("the block is patched again");
    let twice = existing_block(store, &block_id).await.updated_at;
    assert_eq!(twice, once + 1, "a second patch moved it on again");

    test.teardown().await;
}

/// The store keeps beside a body the version of the allowlist it was checked against
/// and its words for searching, and clears both with the body.
#[tokio::test]
async fn a_body_is_stored_with_its_version_and_text() {
    let test = common::setup().await;
    let store = &test.store;
    let Fixture { board_id, .. } = fixture(store).await;
    let [user_id] = users(store).await;
    let document = json!({"type": "doc", "content": [
        {"type": "heading", "attrs": {"level": 1}, "content": [{"type": "text", "text": "Plan"}]},
        {"type": "paragraph", "content": [
            {"type": "text", "text": "x is "}, {"type": "inlineMath", "attrs": {"latex": "a^2"}},
        ]},
        {"type": "blockMath", "attrs": {"latex": "b"}},
    ]});
    let block_id = new_id();
    let mut card = Block {
        block_type: BlockType::Card,
        body: Body::parse(document).expect("the body is allowed"),
        ..block(&block_id, &board_id)
    };
    store
        .insert_block(&mut card, &user_id)
        .await
        .expect("the card is inserted");

    // What the store has no reading for, read through the title.
    let derived = async || {
        common::execute_raw(
            &test.database_connection(),
            &format!(
                "UPDATE blocks SET title = COALESCE(CAST(body_version AS TEXT), '-') || '|' || \
                 COALESCE(body_text, '-') WHERE id = '{block_id}'"
            ),
        )
        .await;
        existing_block(store, &block_id).await.title
    };
    assert_eq!(derived().await, format!("{BODY_VERSION}|Plan\nx is a^2\nb"));

    let clear = BlockPatch {
        body: BodyChange::Clear,
        ..BlockPatch::default()
    };
    store
        .patch_block(&block_id, &clear, &user_id)
        .await
        .expect("the body is cleared");
    assert_eq!(derived().await, "-|-");

    test.teardown().await;
}

/// A list asked for previews reads the beginning of a card's plain text, 201 characters
/// of it, so that a letter and its accent that the 200 units of a preview would split are
/// left out together.
#[tokio::test]
async fn a_preview_reads_one_character_past_its_length() {
    let test = common::setup().await;
    let store = &test.store;
    let Fixture { board_id, .. } = fixture(store).await;
    let [user_id] = users(store).await;
    let opening = "a".repeat(199);
    let written = format!("{opening}e\u{301} and more");
    let mut card = Block {
        block_type: BlockType::Card,
        body: Body::parse(json!({"type": "doc", "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": written}]},
        ]}))
        .expect("the body is allowed"),
        ..block(&new_id(), &board_id)
    };
    store
        .insert_block(&mut card, &user_id)
        .await
        .expect("the card is inserted");

    let listed = store
        .get_listed_blocks(
            &BlockFilter::container(&board_id),
            fvoci_store::Previews::With,
        )
        .await
        .expect("the cards are listed");
    let [listed] = listed.as_slice() else {
        panic!("one card: {listed:?}");
    };
    let preview = listed.preview.as_ref().expect("a preview");
    assert_eq!(preview.text, format!("{opening}e\u{301}"), "201 characters");
    assert_eq!(
        fvoci_model::CardPreview::new(String::new(), &preview.text).text,
        opening,
        "the accented letter is left out whole"
    );

    test.teardown().await;
}

/// Patching several blocks at once.
#[tokio::test]
async fn patch_blocks() {
    let test = common::setup().await;
    let store = &test.store;
    let Fixture {
        user_id,
        workspace_id,
        board_id,
    } = fixture(store).await;
    // Place the two blocks on separate boards.
    let second_board_id = another_board(store, &workspace_id, &user_id).await;

    let first_id = new_id();
    let second_id = new_id();
    let mut insert_blocks = [
        Block {
            title: "oldTitle".to_owned(),
            ..block(&first_id, &board_id)
        },
        Block {
            title: "oldTitle2".to_owned(),
            ..block(&second_id, &second_board_id)
        },
    ];
    store
        .insert_blocks(&mut insert_blocks, &user_id)
        .await
        .expect("the blocks are inserted");

    // "successful updated existing blocks"
    let title = "updatedTitle";
    let batch = BlockPatchBatch {
        block_ids: strings([&first_id, &second_id]),
        block_patches: vec![title_patch(title), title_patch(title)],
    };
    store
        .patch_blocks(&batch, &user_id)
        .await
        .expect("both blocks are patched");
    let retrieved = existing_block(store, &first_id).await;
    assert_eq!(retrieved.title, title, "the first block has the new title");
    let retrieved2 = existing_block(store, &second_id).await;
    assert_eq!(
        retrieved2.title, title,
        "the second block has the new title"
    );

    // Invalid block ID rolls back the entire batch on both SQLite and PostgreSQL.
    let title = "Another Title";
    let batch = BlockPatchBatch {
        block_ids: strings([&first_id, &new_id()]),
        block_patches: vec![title_patch(title), title_patch(title)],
    };
    let error = store
        .patch_blocks(&batch, &user_id)
        .await
        .expect_err("a batch that names a missing block fails");
    assert_not_found(&error);
    let retrieved = existing_block(store, &first_id).await;
    assert_ne!(
        retrieved.title, title,
        "the patch of the first block was rolled back"
    );

    test.teardown().await;
}

/// Deleting a block.
#[tokio::test]
async fn delete_block() {
    let test = common::setup().await;
    let store = &test.store;
    let Fixture {
        user_id, board_id, ..
    } = fixture(store).await;
    let user_id = user_id.as_str();
    let [user1] = users(store).await;

    let initial_count = board_blocks(store, &board_id).await.len();

    let mut blocks_to_insert = three_blocks(&board_id);
    common::insert_blocks(store, &mut blocks_to_insert, &user1).await;
    let block1_id = id_of(&blocks_to_insert, "block1");

    let blocks = board_blocks(store, &board_id).await;
    assert_eq!(
        blocks.len(),
        initial_count + 3,
        "the three blocks were added: {blocks:?}"
    );

    // "existing id"
    store
        .delete_block(block1_id, user_id)
        .await
        .expect("an existing block is deleted");
    // The deleted block moves to the trash while the other two stay live.
    assert_no_block(store, block1_id).await;
    let deleted = block_in_trash(store, block1_id).await;
    assert_eq!(
        deleted.deleted_by, user_id,
        "the trash names who deleted the block"
    );
    let blocks = board_blocks(store, &board_id).await;
    assert_eq!(
        sorted_names(&blocks),
        ["block2", "block3"],
        "the other blocks are still there"
    );

    // "existing id multiple times"
    store
        .delete_block(block1_id, user_id)
        .await
        .expect("deleting a deleted block succeeds");
    store
        .delete_block(block1_id, user_id)
        .await
        .expect("deleting a deleted block succeeds again");

    // "from not existing id"
    store
        .delete_block(&new_id(), user_id)
        .await
        .expect("deleting a block that never existed succeeds");
    assert_eq!(
        block_in_trash(store, block1_id).await,
        deleted,
        "deleting the block again left what is in the trash as it was"
    );

    common::delete_blocks(store, &blocks_to_insert, user_id).await;
    test.teardown().await;
}

/// Undeleting a block that is not deleted or never existed returns not-found because no
/// such block exists in the trash.
#[tokio::test]
async fn undelete_block() {
    let test = common::setup().await;
    let store = &test.store;
    let Fixture {
        user_id, board_id, ..
    } = fixture(store).await;
    let user_id = user_id.as_str();
    let [user1] = users(store).await;

    let initial_count = board_blocks(store, &board_id).await.len();

    let mut blocks_to_insert = three_blocks(&board_id);
    common::insert_blocks(store, &mut blocks_to_insert, &user1).await;
    let block1_id = id_of(&blocks_to_insert, "block1");

    let blocks = board_blocks(store, &board_id).await;
    assert_eq!(
        blocks.len(),
        initial_count + 3,
        "the three blocks were added: {blocks:?}"
    );

    // "existing id"
    store
        .delete_block(block1_id, user_id)
        .await
        .expect("the block is deleted");
    assert_no_block(store, block1_id).await;

    store
        .undelete_block(block1_id, user_id)
        .await
        .expect("the block is undeleted");
    existing_block(store, block1_id).await;

    // "existing id multiple times"
    store
        .delete_block(block1_id, user_id)
        .await
        .expect("the block is deleted a second time");
    assert_no_block(store, block1_id).await;

    store
        .undelete_block(block1_id, user_id)
        .await
        .expect("the block is undeleted a second time");
    existing_block(store, block1_id).await;

    assert_undelete_finds_nothing(store, block1_id, user_id).await;
    existing_block(store, block1_id).await;

    // "from not existing id"
    let missing = new_id();
    assert_undelete_finds_nothing(store, &missing, user_id).await;
    assert_no_block(store, &missing).await;

    common::delete_blocks(store, &blocks_to_insert, user_id).await;
    test.teardown().await;
}

/// Querying blocks with parent, type, and pagination options.
#[tokio::test]
async fn get_blocks() {
    let test = common::setup().await;
    let store = &test.store;
    let Fixture {
        user_id, board_id, ..
    } = fixture(store).await;
    let board_id = board_id.as_str();
    let board = || BlockFilter::container(board_id);

    let mut blocks_to_insert = typed_sample_blocks(board_id);
    common::insert_blocks(store, &mut blocks_to_insert, &user_id).await;
    let block1_id = id_of(&blocks_to_insert, "block1");
    let block2_id = id_of(&blocks_to_insert, "block2");
    let block4_id = id_of(&blocks_to_insert, "block4");
    // An identifier that nothing has.
    let missing = new_id();

    // "not existing parent"
    let filter = board().with_parent(&missing).with_type(BlockType::Card);
    let blocks = store.get_blocks(&filter).await.expect("the query runs");
    assert!(
        blocks.is_empty(),
        "no block has the missing parent: {blocks:?}"
    );

    // "not existing type"
    let filter = board().with_parent(block1_id).with_type(BlockType::Page);
    let blocks = store.get_blocks(&filter).await.expect("the query runs");
    assert!(
        blocks.is_empty(),
        "no block has the missing type: {blocks:?}"
    );

    // "valid parent and type"
    let filter = board().with_parent(block1_id).with_type(BlockType::Card);
    let blocks = store.get_blocks(&filter).await.expect("the query runs");
    assert_eq!(
        blocks.len(),
        2,
        "two children of block1 have the type: {blocks:?}"
    );

    // "not existing parent"
    let filter = board().with_parent(&missing);
    let blocks = store.get_blocks(&filter).await.expect("the query runs");
    assert!(
        blocks.is_empty(),
        "no block has the missing parent: {blocks:?}"
    );

    // "valid parent"
    let filter = board().with_parent(block1_id);
    let blocks = store.get_blocks(&filter).await.expect("the query runs");
    assert_eq!(blocks.len(), 3, "block1 has three children: {blocks:?}");

    // "not existing type"
    let filter = board().with_type(BlockType::Page);
    let blocks = store.get_blocks(&filter).await.expect("the query runs");
    assert!(
        blocks.is_empty(),
        "no block has the missing type: {blocks:?}"
    );

    // "valid type"
    let filter = board().with_type(BlockType::Card);
    let blocks = store.get_blocks(&filter).await.expect("the query runs");
    assert_eq!(blocks.len(), 4, "four blocks have the type: {blocks:?}");

    // Unknown UUID and non-UUID board IDs both return an empty block list.
    for missing_board in [missing.as_str(), "not-exists"] {
        let blocks = board_blocks(store, missing_board).await;
        assert!(
            blocks.is_empty(),
            "the missing board {missing_board} has no blocks: {blocks:?}"
        );
    }

    // "all blocks of the a board"
    let blocks = board_blocks(store, board_id).await;
    assert_eq!(blocks.len(), 5, "the board has all five blocks: {blocks:?}");

    // "several blocks by ids"
    let blocks = store
        .get_blocks_by_ids(&strings([block2_id, block4_id]))
        .await
        .expect("both blocks exist");
    assert_eq!(blocks.len(), 2, "both blocks are returned: {blocks:?}");

    // Asking for a batch where any block ID is missing returns an error.
    let error = store
        .get_blocks_by_ids(&strings([block2_id, &missing]))
        .await
        .expect_err("one of the two blocks does not exist");
    assert!(
        matches!(error, StoreError::NotAllFound { .. }),
        "expected a not-all-found error, got {error:?}"
    );
    assert!(
        error.is_not_found(),
        "the error counts as not found: {error:?}"
    );

    // "blocks by ids where none are found"
    let error = store
        .get_blocks_by_ids(&strings([&missing, &new_id()]))
        .await
        .expect_err("neither block exists");
    assert!(
        matches!(error, StoreError::NotAllFound { .. }),
        "expected a not-all-found error, got {error:?}"
    );
    assert!(
        error.is_not_found(),
        "the error counts as not found: {error:?}"
    );

    common::delete_blocks(store, &blocks_to_insert, &user_id).await;
    test.teardown().await;
}

/// Fetching a single block by ID.
#[tokio::test]
async fn get_block() {
    let test = common::setup().await;
    let store = &test.store;
    let Fixture { board_id, .. } = fixture(store).await;
    let [user1] = users(store).await;

    // "get a block"
    let block_id = new_id();
    let mut block = Block {
        modified_by: user1.clone(),
        ..block(&block_id, &board_id)
    };
    store
        .insert_block(&mut block, &user1)
        .await
        .expect("the block is inserted");

    let fetched = existing_block(store, &block_id).await;
    assert_eq!(fetched.id, block_id, "the identifier is as inserted");
    assert_eq!(fetched.container_id, board_id, "the board is as inserted");
    assert_eq!(
        fetched.created_by, user1,
        "the creator is the inserting user"
    );
    assert_eq!(
        fetched.modified_by, user1,
        "the modifier is the inserting user"
    );
    assert_within_a_second(now_millis(), fetched.created_at, "create time");
    assert_within_a_second(now_millis(), fetched.updated_at, "update time");

    // Unknown UUID, non-UUID text, and empty string all return not-found.
    for missing in [new_id().as_str(), "non-existing-id", ""] {
        assert_no_block(store, missing).await;
    }

    test.teardown().await;
}

/// A card with what is on it, down to the third level below it, and a second card with
/// one text block, which no deletion of the first card may touch: the name of each
/// block, the name of its parent and its type.
const CARD_TREE: [(&str, &str, &str); 7] = [
    ("card", "", "card"),
    ("text", "card", "card"),
    ("comment", "card", "comment"),
    ("below", "text", "card"),
    ("deepest", "below", "attachment"),
    ("other card", "", "card"),
    ("other text", "other card", "attachment"),
];

/// The blocks of [`CARD_TREE`] that are the first card and everything below it.
const CARD_AND_BELOW: [&str; 5] = ["card", "text", "comment", "below", "deepest"];

/// Stores the blocks of [`CARD_TREE`] on a board and returns them as they were stored.
async fn insert_card_tree(store: &Store, board_id: &str, user_id: &str) -> Vec<Block> {
    let mut tree = named_blocks(board_id, &CARD_TREE);
    common::insert_blocks(store, &mut tree, user_id).await;
    tree
}

/// The block that [`named_blocks`] made under the given name.
#[track_caller]
fn block_named<'a>(blocks: &'a [Block], name: &str) -> &'a Block {
    blocks
        .iter()
        .find(|block| block.title == name)
        .unwrap_or_else(|| panic!("no block is named {name}"))
}

/// The titles of the blocks, in the order they came.
fn names(blocks: &[Block]) -> Vec<&str> {
    blocks.iter().map(|block| block.title.as_str()).collect()
}

/// Asserts that the named blocks of a tree exist again as they were stored, parents
/// included, except that `restorer_id` modified them at `restored_at`, and that none of
/// them is in the trash any more.
async fn assert_restored(
    store: &Store,
    tree: &[Block],
    restored: &[&str],
    restorer_id: &str,
    restored_at: i64,
) {
    for name in restored {
        let stored = block_named(tree, name);
        let expected = Block {
            modified_by: restorer_id.to_owned(),
            updated_at: restored_at,
            ..stored.clone()
        };
        assert_eq!(
            existing_block(store, &stored.id).await,
            expected,
            "{name} is back as it was, with the restoring user as modifier at the time of \
             the restore"
        );
        assert_not_in_trash(store, &stored.id).await;
    }
}

/// The blocks of a tree that are in the trash under the given names, as the trash has
/// them.
async fn trash_of(store: &Store, tree: &[Block], deleted: &[&str]) -> Vec<DeletedBlock> {
    let mut in_trash = Vec::with_capacity(deleted.len());
    for name in deleted {
        in_trash.push(block_in_trash(store, id_of(tree, name)).await);
    }
    in_trash
}

/// A subscription of a user to a block.
fn subscription(block: &Block, user_id: &str) -> Subscription {
    Subscription {
        block_type: block.block_type,
        block_id: block.id.clone(),
        user_id: user_id.to_owned(),
        notified_at: 0,
        created_at: 0,
        deleted_at: None,
    }
}

/// The identifiers of the blocks a user is subscribed to, sorted.
async fn subscribed_block_ids(store: &Store, user_id: &str) -> Vec<String> {
    let mut block_ids: Vec<String> = store
        .get_subscriptions(user_id)
        .await
        .expect("the subscriptions of a user are read")
        .into_iter()
        .map(|subscription| subscription.block_id)
        .collect();
    block_ids.sort();
    block_ids
}

/// How many times each race below is run.
pub(crate) const RACE_ROUNDS: usize = 40;

/// How many calls compete in a round of a race below.
const RACERS: usize = 4;
