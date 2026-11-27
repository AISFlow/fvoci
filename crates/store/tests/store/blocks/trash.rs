//! A deletion takes everything below a block along and a restore brings the whole deletion
//! back, while what follows the blocks waits.

use super::*;

/// Deleting a block takes everything below it along, at any depth, as one deletion: each
/// of them is in the trash as it was, with the one deletion time and the deleting user.
#[tokio::test]
async fn delete_block_takes_everything_below_it_along_as_one_deletion() {
    let test = common::setup().await;
    let store = &test.store;
    let Fixture {
        user_id, board_id, ..
    } = fixture(store).await;
    let deleter_id = common::user(store, "deleter").await;
    let tree = insert_card_tree(store, &board_id, &user_id).await;

    store
        .delete_block(id_of(&tree, "card"), &deleter_id)
        .await
        .expect("the card is deleted");

    let left = board_blocks(store, &board_id).await;
    assert_eq!(
        names(&left),
        ["other card", "other text"],
        "the other card and what is on it are all that is left"
    );
    for name in ["other card", "other text"] {
        let stored = block_named(&tree, name);
        assert_eq!(
            &existing_block(store, &stored.id).await,
            stored,
            "{name} is as it was"
        );
        assert_not_in_trash(store, &stored.id).await;
    }

    let deleted_at = block_in_trash(store, id_of(&tree, "card"))
        .await
        .block
        .deleted_at
        .expect("deletion time");
    assert_within_a_second(now_millis(), deleted_at, "the deletion time of the card");
    for name in CARD_AND_BELOW {
        let stored = block_named(&tree, name);
        assert_no_block(store, &stored.id).await;
        let deleted = block_in_trash(store, &stored.id).await;
        assert_eq!(
            deleted.deleted_by, deleter_id,
            "the trash names who deleted {name}"
        );
        let expected = Block {
            deleted_at: Some(deleted_at),
            ..stored.clone()
        };
        assert_eq!(
            deleted.block, expected,
            "{name} is in the trash as it was, with the deletion time of the card"
        );
    }

    test.teardown().await;
}

/// Undeleting a card brings back everything that was deleted with it, each on the block
/// it was on, and returns the card.
#[tokio::test]
async fn undelete_block_of_a_card_brings_back_everything_deleted_with_it() {
    let test = common::setup().await;
    let store = &test.store;
    let Fixture {
        user_id, board_id, ..
    } = fixture(store).await;
    let restorer_id = common::user(store, "restorer").await;
    let tree = insert_card_tree(store, &board_id, &user_id).await;
    let card_id = id_of(&tree, "card");

    store
        .delete_block(card_id, &user_id)
        .await
        .expect("the card is deleted");
    let card = store
        .undelete_block(card_id, &restorer_id)
        .await
        .expect("the card is restored");

    assert_eq!(card.id, card_id, "the restored card is returned");
    assert_eq!(
        card,
        existing_block(store, card_id).await,
        "the card is returned as it is stored"
    );
    assert_within_a_second(now_millis(), card.updated_at, "the time of the restore");
    assert_restored(store, &tree, &CARD_AND_BELOW, &restorer_id, card.updated_at).await;

    let blocks = board_blocks(store, &board_id).await;
    assert_eq!(
        sorted_names(&blocks),
        sorted_names(&tree),
        "the board has every block once"
    );
    for name in ["other card", "other text"] {
        let stored = block_named(&tree, name);
        assert_eq!(
            &existing_block(store, &stored.id).await,
            stored,
            "{name}, which was not deleted, is as it was"
        );
    }

    test.teardown().await;
}

/// Undeleting a block that was deleted along with its card brings back the whole
/// deletion, the card included, and returns the block that was asked for.
#[tokio::test]
async fn undelete_block_of_a_child_brings_back_the_whole_deletion() {
    let test = common::setup().await;
    let store = &test.store;
    let Fixture {
        user_id, board_id, ..
    } = fixture(store).await;
    let restorer_id = common::user(store, "restorer").await;
    let tree = insert_card_tree(store, &board_id, &user_id).await;
    let deepest_id = id_of(&tree, "deepest");

    store
        .delete_block(id_of(&tree, "card"), &user_id)
        .await
        .expect("the card is deleted");
    let deepest = store
        .undelete_block(deepest_id, &restorer_id)
        .await
        .expect("the block at the bottom is restored");

    assert_eq!(deepest.id, deepest_id, "the block asked for is returned");
    assert_eq!(
        deepest,
        existing_block(store, deepest_id).await,
        "the block is returned as it is stored"
    );
    assert_restored(
        store,
        &tree,
        &CARD_AND_BELOW,
        &restorer_id,
        deepest.updated_at,
    )
    .await;
    let blocks = board_blocks(store, &board_id).await;
    assert_eq!(
        sorted_names(&blocks),
        sorted_names(&tree),
        "the board has every block once"
    );

    test.teardown().await;
}

/// A block that was deleted on its own has nothing to come back to while the card it was
/// on is in the trash: undeleting it finds nothing and changes nothing. Restoring the
/// card does not bring it back, since it went with another deletion; after that it can
/// be restored.
#[tokio::test]
async fn a_child_deleted_before_its_card_comes_back_only_after_the_card() {
    let test = common::setup().await;
    let store = &test.store;
    let Fixture {
        user_id, board_id, ..
    } = fixture(store).await;
    let restorer_id = common::user(store, "restorer").await;
    let tree = insert_card_tree(store, &board_id, &user_id).await;
    let card_id = id_of(&tree, "card");
    let below_id = id_of(&tree, "below");
    let deepest_id = id_of(&tree, "deepest");

    // The attachment goes on its own, then the card with the sub-cards and comment left on it.
    store
        .delete_block(deepest_id, &user_id)
        .await
        .expect("the attachment is deleted");
    store
        .delete_block(card_id, &user_id)
        .await
        .expect("the card is deleted");
    let left = board_blocks(store, &board_id).await;
    assert_eq!(
        names(&left),
        ["other card", "other text"],
        "the card and everything that was on it are gone"
    );
    let trash = trash_of(store, &tree, &CARD_AND_BELOW).await;

    assert_undelete_finds_nothing(store, deepest_id, &restorer_id).await;
    assert_eq!(
        board_blocks(store, &board_id).await,
        left,
        "undeleting deepest while the card is in the trash brought nothing back"
    );
    assert_eq!(
        trash_of(store, &tree, &CARD_AND_BELOW).await,
        trash,
        "undeleting deepest while the card is in the trash left the trash as it was"
    );

    let card = store
        .undelete_block(card_id, &restorer_id)
        .await
        .expect("the card is restored");
    assert_restored(
        store,
        &tree,
        &["card", "text", "comment", "below"],
        &restorer_id,
        card.updated_at,
    )
    .await;
    assert_eq!(
        trash_of(store, &tree, &["deepest"]).await,
        trash_of_names(&trash, &tree, &["deepest"]),
        "what was deleted before the card is still in the trash as it was"
    );
    assert_no_block(store, deepest_id).await;

    let restored_deepest = store
        .undelete_block(deepest_id, &restorer_id)
        .await
        .expect("the attachment is restored once its card exists");
    assert_eq!(
        restored_deepest.parent_id, below_id,
        "the attachment is back on its card"
    );
    assert_restored(
        store,
        &tree,
        &["deepest"],
        &restorer_id,
        restored_deepest.updated_at,
    )
    .await;
    let blocks = board_blocks(store, &board_id).await;
    assert_eq!(
        sorted_names(&blocks),
        sorted_names(&tree),
        "the board has every block once"
    );

    test.teardown().await;
}

/// The entries of `trash` for the named blocks of a tree, in the order of the names.
fn trash_of_names(trash: &[DeletedBlock], tree: &[Block], deleted: &[&str]) -> Vec<DeletedBlock> {
    deleted
        .iter()
        .map(|name| {
            let id = id_of(tree, name);
            trash
                .iter()
                .find(|entry| entry.block.id == id)
                .unwrap_or_else(|| panic!("{name} was in the trash"))
                .clone()
        })
        .collect()
}

/// Undeleting finds nothing under the identifier of a block that exists, under one that
/// nothing has and under text that is no identifier, and writes nothing.
#[tokio::test]
async fn undelete_block_of_what_is_not_in_the_trash_is_not_found_and_writes_nothing() {
    let test = common::setup().await;
    let store = &test.store;
    let Fixture {
        user_id, board_id, ..
    } = fixture(store).await;
    let restorer_id = common::user(store, "restorer").await;
    let tree = insert_card_tree(store, &board_id, &user_id).await;

    // Something is in the trash, which none of the calls may bring back.
    store
        .delete_block(id_of(&tree, "other card"), &user_id)
        .await
        .expect("the other card is deleted");
    let blocks = board_blocks(store, &board_id).await;
    let trash = trash_of(store, &tree, &["other card", "other text"]).await;

    let unknown_id = new_id();
    for (what, block_id) in [
        ("a block that exists", id_of(&tree, "card")),
        ("a block below one that exists", id_of(&tree, "deepest")),
        ("an identifier that nothing has", unknown_id.as_str()),
        ("text that is no identifier", "no-identifier"),
        ("no text at all", ""),
    ] {
        match store.undelete_block(block_id, &restorer_id).await {
            Ok(block) => panic!("undeleting {what} should find nothing, got {block:?}"),
            Err(error) => assert!(
                matches!(error, StoreError::NotFound(_)),
                "undeleting {what} should be a not-found error, got {error:?}"
            ),
        }
        assert_eq!(
            board_blocks(store, &board_id).await,
            blocks,
            "undeleting {what} left the blocks as they were, modifier and time included"
        );
        assert_eq!(
            trash_of(store, &tree, &["other card", "other text"]).await,
            trash,
            "undeleting {what} left the trash as it was"
        );
        assert_not_in_trash(store, block_id).await;
    }

    test.teardown().await;
}

/// The subscriptions to a block pause in the trash, return on restore, and end on purge.
#[tokio::test]
async fn subscriptions_to_a_deleted_block_pause_in_trash_and_return_on_restore() {
    let test = common::setup().await;
    let store = &test.store;
    let Fixture {
        user_id, board_id, ..
    } = fixture(store).await;
    let watcher_id = common::user(store, "watcher").await;
    let tree = insert_card_tree(&store.telling_nobody(), &board_id, &user_id).await;
    let card_id = id_of(&tree, "card");
    let deepest_id = id_of(&tree, "deepest");
    let other_card_id = id_of(&tree, "other card");

    for name in ["card", "deepest", "other card"] {
        let sub = subscription(block_named(&tree, name), &watcher_id);
        store.create_subscription(sub).await.expect("subscribe");
    }
    let second = subscription(block_named(&tree, "card"), &user_id);
    store.create_subscription(second).await.expect("subscribe");

    store.delete_block(card_id, &user_id).await.expect("delete");
    for block_id in [card_id, deepest_id] {
        let err = store
            .get_subscription(block_id, &watcher_id)
            .await
            .expect_err("paused in trash");
        assert!(err.is_not_found(), "{block_id}: {err:?}");
        let subs = store
            .get_subscribers_for_block(block_id)
            .await
            .expect("read");
        assert!(subs.is_empty(), "{block_id}: {subs:?}");
    }
    assert_eq!(
        subscribed_block_ids(store, &watcher_id).await,
        [other_card_id]
    );
    assert_eq!(
        subscribed_block_ids(store, &user_id).await,
        Vec::<String>::new()
    );

    store
        .undelete_block(card_id, &user_id)
        .await
        .expect("restore");
    existing_block(store, deepest_id).await;
    for (block_id, who) in [
        (card_id, &watcher_id),
        (deepest_id, &watcher_id),
        (card_id, &user_id),
    ] {
        store
            .get_subscription(block_id, who)
            .await
            .expect("restored");
    }
    let mut expected = strings([card_id, deepest_id, other_card_id]);
    expected.sort();
    assert_eq!(subscribed_block_ids(store, &watcher_id).await, expected);
    assert_eq!(subscribed_block_ids(store, &user_id).await, [card_id]);

    store
        .delete_block(card_id, &user_id)
        .await
        .expect("re-delete");
    store.purge_trash(i64::MAX).await.expect("purge trash");
    let err = store.get_deleted_block(card_id).await.expect_err("purged");
    assert!(err.is_not_found(), "got {err:?}");
    assert_eq!(
        subscribed_block_ids(store, &watcher_id).await,
        [other_card_id]
    );
    assert_eq!(
        subscribed_block_ids(store, &user_id).await,
        Vec::<String>::new()
    );

    test.teardown().await;
}
