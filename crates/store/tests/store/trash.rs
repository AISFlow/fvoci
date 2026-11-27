//! What a deletion takes along and brings back: [`Store::get_blocks_below`] before it,
//! [`Store::get_deletion`] after it, and where a block may come back.

use fvoci_model::{Block, BlockError, BlockPatch, BlockType, new_id};
use fvoci_store::{Store, StoreError};

use crate::common::{self, sorted_ids};

/// A block of the board below `parent_id` (none if empty).
fn block(id: &str, board_id: &str, parent_id: &str, block_type: BlockType) -> Block {
    Block {
        id: id.to_owned(),
        container_id: board_id.to_owned(),
        parent_id: parent_id.to_owned(),
        block_type,
        ..Block::default()
    }
}

/// The identifiers of the blocks, in their order.
fn ids(blocks: &[Block]) -> Vec<&str> {
    blocks.iter().map(|block| block.id.as_str()).collect()
}

#[tokio::test]
async fn a_deletion_is_the_block_with_all_below_it_the_block_first() {
    let test = common::setup().await;
    let store: &Store = &test.store;
    let user_id = common::user(store, "author").await;
    let workspace_id = common::workspace(store).await;
    let board = common::create_test_boards(store, &workspace_id, &user_id, 1)
        .await
        .remove(0);
    // The middle block's identifier sorts after the one below it, so that the order of
    // the deletion is not that of the identifiers.
    let mut pair = [new_id(), new_id()];
    pair.sort();
    let [leaf_id, middle_id] = pair;
    let card_id = new_id();
    let mut blocks = [
        block(&card_id, &board.id, "", BlockType::Card),
        block(&middle_id, &board.id, &card_id, BlockType::Card),
        block(&leaf_id, &board.id, &middle_id, BlockType::Attachment),
    ];
    common::insert_blocks(store, &mut blocks, &user_id).await;

    let below = store.get_blocks_below(&card_id).await.expect("read");
    assert_eq!(ids(&below), sorted_ids(&blocks[1..]), "at any depth, by id");
    for nothing in [leaf_id.as_str(), "no-such-block"] {
        let below = store.get_blocks_below(nothing).await.expect("read");
        assert!(below.is_empty(), "{nothing}: {below:?}");
    }

    store
        .delete_block(&middle_id, &user_id)
        .await
        .expect("deleted");
    for named in [&middle_id, &leaf_id] {
        let deletion = store.get_deletion(named).await.expect("in the trash");
        assert_eq!(ids(&deletion), [middle_id.as_str(), leaf_id.as_str()]);
    }
    let error = store.get_deletion(&card_id).await.expect_err("not deleted");
    assert!(error.is_not_found(), "{error:?}");

    // A later deletion is one of its own.
    store
        .delete_block(&card_id, &user_id)
        .await
        .expect("deleted");
    let deletion = store.get_deletion(&card_id).await.expect("in the trash");
    assert_eq!(ids(&deletion), [card_id.as_str()]);
    let deletion = store.get_deletion(&leaf_id).await.expect("in the trash");
    assert_eq!(ids(&deletion), [middle_id.as_str(), leaf_id.as_str()]);
}

/// A block stored before a comment had to be on a card, with nothing below it, is still
/// changed in other ways: only what a write changes is checked.
#[tokio::test]
async fn a_block_stored_off_its_place_is_still_changed() {
    let test = common::setup().await;
    let store: &Store = &test.store;
    let user_id = common::user(store, "author").await;
    let workspace_id = common::workspace(store).await;
    let board = common::create_test_boards(store, &workspace_id, &user_id, 1)
        .await
        .remove(0);
    let card_id = new_id();
    let comment_id = new_id();
    let below_id = new_id();
    let mut blocks = [
        block(&card_id, &board.id, "", BlockType::Card),
        block(&comment_id, &board.id, &card_id, BlockType::Comment),
        block(&below_id, &board.id, &card_id, BlockType::Attachment),
    ];
    common::insert_blocks(store, &mut blocks, &user_id).await;
    // The comment on the board itself, the attachment below the comment.
    for statement in [
        format!("UPDATE blocks SET parent_id = NULL WHERE id = '{comment_id}'"),
        format!("UPDATE blocks SET parent_id = '{comment_id}' WHERE id = '{below_id}'"),
    ] {
        common::execute_raw(&test.database_connection(), &statement).await;
    }
    for id in [&comment_id, &below_id] {
        let patch = BlockPatch {
            title: Some("changed".to_owned()),
            ..BlockPatch::default()
        };
        store
            .patch_block(id, &patch, &user_id)
            .await
            .expect("a change of the title alone is taken");
    }
}

/// A block comes back from the trash only where it may be: a comment whose card became
/// another type meanwhile is refused, and stays in the trash.
#[tokio::test]
async fn a_comment_does_not_come_back_onto_what_is_no_card() {
    let test = common::setup().await;
    let store: &Store = &test.store;
    let user_id = common::user(store, "author").await;
    let workspace_id = common::workspace(store).await;
    let board = common::create_test_boards(store, &workspace_id, &user_id, 1)
        .await
        .remove(0);
    let card_id = new_id();
    let comment_id = new_id();
    let mut blocks = [
        block(&card_id, &board.id, "", BlockType::Card),
        block(&comment_id, &board.id, &card_id, BlockType::Comment),
    ];
    common::insert_blocks(store, &mut blocks, &user_id).await;
    store
        .delete_block(&comment_id, &user_id)
        .await
        .expect("deleted");
    test.execute_unchecked(&format!(
        "UPDATE blocks SET type = 'view' WHERE id = '{card_id}'"
    ))
    .await
    .expect("legacy view parent");
    let error = store
        .undelete_block(&comment_id, &user_id)
        .await
        .expect_err("not onto a view");
    assert!(
        matches!(
            &error,
            StoreError::NotRestorable { id, source: BlockError::CommentNotOnCardOrPage } if *id == comment_id
        ),
        "{error:?}"
    );
    store
        .get_deleted_block(&comment_id)
        .await
        .expect("still in the trash");

    // A comment stored on the board itself before the rule does not come back either.
    let other_card = new_id();
    let mut old = [
        block(&other_card, &board.id, "", BlockType::Card),
        block(&new_id(), &board.id, &other_card, BlockType::Comment),
    ];
    common::insert_blocks(store, &mut old, &user_id).await;
    let old = &old[1];
    let on_board = format!("UPDATE blocks SET parent_id = NULL WHERE id = '{}'", old.id);
    common::execute_raw(&test.database_connection(), &on_board).await;
    store
        .delete_block(&old.id, &user_id)
        .await
        .expect("deleted");
    let error = store
        .undelete_block(&old.id, &user_id)
        .await
        .expect_err("not onto the board");
    assert!(
        matches!(
            &error,
            StoreError::NotRestorable {
                source: BlockError::CommentNotOnCardOrPage,
                ..
            }
        ),
        "{error:?}"
    );
}

/// A corrupt snapshot row in `deleted_blocks` is skipped during trash listing without
/// failing the container's trash read, while single-row fetch and restore still fail
/// closed.
#[tokio::test]
async fn corrupt_trash_row_is_skipped_when_listing_deleted_blocks() {
    let test = common::setup().await;
    let store = &test.store;
    let alice = common::user(store, "alice-trash").await;
    let workspace_id = common::workspace(store).await;
    common::works_in(store, &workspace_id, &alice).await;

    let mut private_board = fvoci_model::Container {
        id: new_id(),
        workspace_id: workspace_id.clone(),
        visibility: fvoci_model::ContainerVisibility::Private,
        title: "Private Trash Board".to_owned(),
        ..fvoci_model::Container::default()
    };
    store
        .insert_container_with_admin(&mut private_board, &alice)
        .await
        .expect("private board");

    let card_id = new_id();
    let mut cards = [block(&card_id, &private_board.id, "", BlockType::Card)];
    common::insert_blocks(store, &mut cards, &alice).await;
    store
        .delete_block(&card_id, &alice)
        .await
        .expect("card trashed");
    let corrupt_id = new_id();
    let cast = if common::db_type() == fvoci_store::DbType::Postgres {
        "::jsonb"
    } else {
        ""
    };
    store
        .execute_unchecked(&format!(
            "INSERT INTO deleted_blocks (id, container_id, parent_id, deleted_at, deleted_by, deletion_id, block, block_type) \
             VALUES ('{corrupt_id}', '{}', NULL, 999999, '{alice}', '{}', '123'{cast}, 'card')",
            private_board.id,
            new_id()
        ))
        .await
        .expect("corrupt snapshot inserted");

    let listed = store
        .get_deleted_cards(&private_board.id, None)
        .await
        .expect("listing skips corrupt row");
    assert_eq!(
        listed
            .iter()
            .map(|d| d.block.id.as_str())
            .collect::<Vec<_>>(),
        [card_id.as_str()],
        "valid trashed card returned while corrupt row is skipped"
    );
    assert!(
        matches!(
            store.get_deleted_block(&corrupt_id).await,
            Err(StoreError::Corrupt { .. })
        ),
        "corrupt deleted_block must fail get_deleted_block"
    );
    assert!(
        matches!(
            store.undelete_block(&corrupt_id, &alice).await,
            Err(StoreError::Corrupt { .. })
        ),
        "corrupt deleted_block must fail undelete_block"
    );

    test.teardown().await;
}
