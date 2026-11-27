//! Integration tests for block subscriptions, subscriber queries, and cascade cleanup.

use std::time::Duration;

use fvoci_model::{
    Block, BlockType, ContainerMember, Subscription, SubscriptionError, User, new_id,
};
use fvoci_store::{BlockFilter, MemberChange, Store, StoreError};

use crate::common;

/// Creates `num` test users with unique usernames and email addresses.
async fn create_test_users(store: &Store, num: usize) -> Vec<User> {
    let mut users = Vec::with_capacity(num);
    for index in 0..num {
        let id = new_id();
        let mut user = User {
            username: format!("mooncake.{index}.{id}"),
            email: format!("mooncake.{index}.{id}@example.com"),
            id,
            ..User::default()
        };
        store
            .create_user(&mut user)
            .await
            .expect("the user is created");
        users.push(user);
    }
    users
}

/// Creates `num` test blocks, each on its own board in a fresh workspace, written quietly
/// ([`Store::telling_nobody`]) so tests control subscriptions explicitly.
async fn create_test_blocks(store: &Store, user_id: &str, num: usize) -> Vec<Block> {
    let workspace_id = common::workspace(store).await;
    let boards = common::create_test_boards(store, &workspace_id, user_id, num).await;
    let mut blocks: Vec<Block> = boards
        .into_iter()
        .map(|board| card_on(&board.id, user_id))
        .collect();
    common::insert_blocks(&store.telling_nobody(), &mut blocks, user_id).await;
    blocks
}

/// A card directly on the board, not stored yet.
fn card_on(board_id: &str, user_id: &str) -> Block {
    Block {
        id: new_id(),
        container_id: board_id.to_owned(),
        block_type: BlockType::Card,
        created_by: user_id.to_owned(),
        ..Block::default()
    }
}

/// The identifiers of the blocks the user is subscribed to, sorted.
async fn subscribed_blocks(store: &Store, user: &User) -> Vec<String> {
    let mut ids: Vec<String> = store
        .get_subscriptions(&user.id)
        .await
        .expect("the subscriptions of a user are listed")
        .into_iter()
        .map(|subscription| subscription.block_id)
        .collect();
    ids.sort();
    ids
}

/// Builds a user subscription to `block`.
fn subscription(block: &Block, user: &User) -> Subscription {
    Subscription {
        block_type: block.block_type,
        block_id: block.id.clone(),
        user_id: user.id.clone(),
        notified_at: 0,
        created_at: 0,
        deleted_at: None,
    }
}

async fn first_user(store: &Store) -> User {
    create_test_users(store, 1)
        .await
        .pop()
        .expect("one user was created")
}

async fn first_block(store: &Store, user_id: &str) -> Block {
    create_test_blocks(store, user_id, 1)
        .await
        .pop()
        .expect("one block was created")
}

/// Creating a subscription.
#[tokio::test]
async fn create_subscription() {
    let test = common::setup().await;
    let store = &test.store;

    // "create subscriptions"
    {
        let users = create_test_users(store, 10).await;
        let blocks = create_test_blocks(store, &users[0].id, 50).await;

        for (i, user) in users.iter().enumerate() {
            for block in &blocks[..i] {
                let sub = subscription(block, user);
                let sub_new = store
                    .create_subscription(sub)
                    .await
                    .expect("create subscription should not error");

                assert_ne!(sub_new.notified_at, 0);
                assert_ne!(sub_new.created_at, 0);
                assert_eq!(sub_new.deleted_at, None);
            }
        }

        // ensure each user has the right number of subscriptions
        for (i, user) in users.iter().enumerate() {
            let subs = store
                .get_subscriptions(&user.id)
                .await
                .expect("get subscriptions should not error");
            assert_eq!(subs.len(), i);
        }
    }

    // "duplicate subscription"
    {
        let admin = first_user(store).await;
        let user = first_user(store).await;
        let block = first_block(store, &admin.id).await;

        let sub = subscription(&block, &user);
        let sub_new = store
            .create_subscription(sub)
            .await
            .expect("create subscription should not error");

        let sub = subscription(&block, &user);

        let sub_dup = store
            .create_subscription(sub)
            .await
            .expect("create duplicate subscription should not error");

        assert_eq!(sub_new.block_id, sub_dup.block_id);
        assert_eq!(sub_new.user_id, sub_dup.user_id);
    }

    // "invalid subscription"
    {
        let admin = first_user(store).await;
        let user = first_user(store).await;
        let block = first_block(store, &admin.id).await;

        let mut sub = Subscription {
            block_type: BlockType::default(),
            block_id: String::new(),
            user_id: String::new(),
            notified_at: 0,
            created_at: 0,
            deleted_at: None,
        };
        let assert_invalid = |result: Result<Subscription, StoreError>| {
            assert!(
                matches!(result, Err(StoreError::InvalidSubscription(_))),
                "invalid subscription should error: got {result:?}"
            );
        };

        assert_invalid(store.create_subscription(sub.clone()).await);

        sub.block_type = block.block_type;
        assert_invalid(store.create_subscription(sub.clone()).await);

        sub.block_id.clone_from(&block.id);
        assert_invalid(store.create_subscription(sub.clone()).await);

        sub.user_id.clone_from(&user.id);
        let sub_new = store
            .create_subscription(sub)
            .await
            .expect("valid subscription should not error");

        let valid: Result<(), SubscriptionError> = sub_new.validate();
        assert_eq!(valid, Ok(()), "created subscription should be valid");

        // Non-UUID block or user IDs are rejected.
        for (block_id, user_id, invalid) in [
            ("bogus-block", user.id.as_str(), "bogus-block"),
            (block.id.as_str(), "bogus-user", "bogus-user"),
        ] {
            let sub = Subscription {
                block_id: block_id.to_owned(),
                user_id: user_id.to_owned(),
                ..sub_new.clone()
            };
            let error = store
                .create_subscription(sub)
                .await
                .expect_err("a subscription that names nothing should error");
            assert!(
                matches!(&error, StoreError::InvalidId(refused) if refused == invalid),
                "got {error:?}"
            );
        }
    }

    test.teardown().await;
}

/// Deleting a subscription.
#[tokio::test]
async fn delete_subscription() {
    let test = common::setup().await;
    let store = &test.store;

    // "delete subscription"
    {
        let user = first_user(store).await;
        let block = first_block(store, &user.id).await;

        let sub = subscription(&block, &user);
        let sub_new = store
            .create_subscription(sub)
            .await
            .expect("create subscription should not error");

        // check the subscription exists
        let subs = store
            .get_subscriptions(&user.id)
            .await
            .expect("get subscriptions should not error");
        assert_eq!(subs.len(), 1);
        assert_eq!(sub_new.block_id, subs[0].block_id);
        assert_eq!(sub_new.user_id, subs[0].user_id);

        store
            .delete_subscription(&block.id, &user.id)
            .await
            .expect("delete subscription should not error");

        // check the subscription was deleted
        let subs = store
            .get_subscriptions(&user.id)
            .await
            .expect("get subscriptions should not error");
        assert!(subs.is_empty(), "got {subs:?}");
    }

    // "delete non-existent subscription"
    {
        let error = store
            .delete_subscription(&new_id(), &new_id())
            .await
            .expect_err("delete non-existent subscription should error");
        assert!(
            error.is_not_found(),
            "Should be ErrNotFound compatible error: got {error:?}"
        );

        // Non-UUID text is rejected when deleting a subscription.
        let error = store
            .delete_subscription("bogus", "bogus")
            .await
            .expect_err("delete non-existent subscription should error");
        assert!(
            matches!(&error, StoreError::InvalidId(refused) if refused == "bogus"),
            "got {error:?}"
        );
    }

    test.teardown().await;
}

/// Restoring a deleted subscription.
#[tokio::test]
async fn undelete_subscription() {
    let test = common::setup().await;
    let store = &test.store;

    // "undelete subscription"
    let user = first_user(store).await;
    let block = first_block(store, &user.id).await;

    let sub = subscription(&block, &user);
    let sub_new = store
        .create_subscription(sub.clone())
        .await
        .expect("create subscription should not error");

    // check the subscription exists
    let subs = store
        .get_subscriptions(&user.id)
        .await
        .expect("get subscriptions should not error");
    assert_eq!(subs.len(), 1);
    assert_eq!(sub_new.block_id, subs[0].block_id);
    assert_eq!(sub_new.user_id, subs[0].user_id);

    store
        .delete_subscription(&block.id, &user.id)
        .await
        .expect("delete subscription should not error");

    // check the subscription was deleted
    let subs = store
        .get_subscriptions(&user.id)
        .await
        .expect("get subscriptions should not error");
    assert!(subs.is_empty(), "got {subs:?}");

    // re-create the subscription
    let sub_undeleted = store
        .create_subscription(sub)
        .await
        .expect("create subscription should not error");

    // check the undeleted subscription exists
    let subs = store
        .get_subscriptions(&user.id)
        .await
        .expect("get subscriptions should not error");
    assert_eq!(subs.len(), 1);
    assert_eq!(sub_undeleted.block_id, subs[0].block_id);
    assert_eq!(sub_undeleted.user_id, subs[0].user_id);

    test.teardown().await;
}

/// Reading a single subscription by block and subscriber.
#[tokio::test]
async fn get_subscription() {
    let test = common::setup().await;
    let store = &test.store;

    // "get subscription"
    {
        let user = first_user(store).await;
        let block = first_block(store, &user.id).await;

        let sub = subscription(&block, &user);
        let sub_new = store
            .create_subscription(sub)
            .await
            .expect("create subscription should not error");

        // make sure subscription can be fetched
        let sub = store
            .get_subscription(&block.id, &user.id)
            .await
            .expect("get subscription should not error");
        assert_eq!(sub_new, sub);
    }

    // Non-UUID text and unknown UUIDs both return not-found.
    let nothing = new_id();
    for (block_id, subscriber_id) in [("bogus", "bogus"), (nothing.as_str(), nothing.as_str())] {
        let error = store
            .get_subscription(block_id, subscriber_id)
            .await
            .expect_err("get non-existent subscription should error");
        assert!(
            error.is_not_found(),
            "Should be ErrNotFound compatible error: got {error:?}"
        );
    }

    test.teardown().await;
}

/// Listing all active subscriptions for a user.
#[tokio::test]
async fn get_subscriptions() {
    let test = common::setup().await;
    let store = &test.store;

    // "get subscriptions"
    {
        let author = first_user(store).await;
        let user = first_user(store).await;
        let blocks = create_test_blocks(store, &author.id, 50).await;

        for block in &blocks {
            let sub = subscription(block, &user);
            store
                .create_subscription(sub)
                .await
                .expect("create subscription should not error");
        }

        // ensure user has the right number of subscriptions
        let subs = store
            .get_subscriptions(&user.id)
            .await
            .expect("get subscriptions should not error");
        assert_eq!(subs.len(), blocks.len());

        // ensure author has no subscriptions
        let subs = store
            .get_subscriptions(&author.id)
            .await
            .expect("get subscriptions should not error");
        assert!(subs.is_empty(), "got {subs:?}");
    }

    // Non-UUID text and unknown user IDs both return an empty list.
    let nobody = new_id();
    for user_id in ["bogus", nobody.as_str()] {
        let subs = store
            .get_subscriptions(user_id)
            .await
            .expect("get subscriptions should not error");
        assert!(subs.is_empty(), "got {subs:?}");
    }

    test.teardown().await;
}

/// Listing subscribers for a block.
#[tokio::test]
async fn get_subscribers_for_block() {
    let test = common::setup().await;
    let store = &test.store;

    // "get subscribers for block"
    {
        let users = create_test_users(store, 50).await;
        let blocks = create_test_blocks(store, &users[0].id, 2).await;

        for user in &users {
            let sub = subscription(&blocks[1], user);
            store
                .create_subscription(sub)
                .await
                .expect("create subscription should not error");
        }

        // make sure block[1] has the right number of users subscribed
        let subs = store
            .get_subscribers_for_block(&blocks[1].id)
            .await
            .expect("get subscribers for block should not error");
        assert_eq!(subs.len(), 50);

        let count = store
            .get_subscribers_count_for_block(&blocks[1].id)
            .await
            .expect("get subscribers for block should not error");
        assert_eq!(count, 50);

        // make sure block[0] has zero users subscribed
        let subs = store
            .get_subscribers_for_block(&blocks[0].id)
            .await
            .expect("get subscribers for block should not error");
        assert!(subs.is_empty(), "got {subs:?}");

        let count = store
            .get_subscribers_count_for_block(&blocks[0].id)
            .await
            .expect("get subscribers for block should not error");
        assert_eq!(count, 0);
    }

    // Non-UUID text and unknown block IDs both return an empty list.
    let nothing = new_id();
    for block_id in ["bogus", nothing.as_str()] {
        let subs = store
            .get_subscribers_for_block(block_id)
            .await
            .expect("get subscribers for block should not error");
        assert!(subs.is_empty(), "got {subs:?}");

        let count = store
            .get_subscribers_count_for_block(block_id)
            .await
            .expect("get subscribers count for block should not error");
        assert_eq!(count, 0);
    }

    test.teardown().await;
}

// Subscription lifecycle invariants: a subscription is a live row while active and ends
// when its block, its subscriber, or the subscriber's board membership ends.

/// Subscribing to what one is subscribed to keeps the subscription: its creation time
/// and its types stay, whatever the second request gives, and the notification time is
/// renewed. What comes back is the subscription as it is stored.
#[tokio::test]
async fn resubscribing_keeps_the_stored_creation_time() {
    let test = common::setup().await;
    let store = &test.store;

    let user = first_user(store).await;
    let block = first_block(store, &user.id).await;
    let sub = subscription(&block, &user);

    let first = store
        .create_subscription(sub.clone())
        .await
        .expect("the user subscribes");
    assert_eq!(
        first.notified_at, first.created_at,
        "a new subscription counts as notified when it was made"
    );
    assert_ne!(first.created_at, 0, "stamped with the current time");
    assert_eq!(
        first.deleted_at, None,
        "a subscription that exists is not ended"
    );
    let stored = store
        .get_subscription(&block.id, &user.id)
        .await
        .expect("the subscription is found");
    assert_eq!(stored, first, "returned as stored");

    tokio::time::sleep(Duration::from_millis(5)).await;
    let second = store
        .create_subscription(Subscription {
            block_type: BlockType::Attachment,
            ..sub
        })
        .await
        .expect("subscribing twice is accepted");
    assert!(
        second.notified_at > first.notified_at,
        "the notification time is renewed: {} after {}",
        second.notified_at,
        first.notified_at
    );
    assert_eq!(
        second,
        Subscription {
            notified_at: second.notified_at,
            ..first.clone()
        },
        "the creation time and the types of the first subscription stay"
    );
    let stored = store
        .get_subscription(&block.id, &user.id)
        .await
        .expect("the subscription is found");
    assert_eq!(stored, second, "returned as stored");

    assert_eq!(
        subscribed_blocks(store, &user).await,
        std::slice::from_ref(&block.id),
        "one subscription, not two"
    );
    let count = store
        .get_subscribers_count_for_block(&block.id)
        .await
        .expect("the subscribers are counted");
    assert_eq!(count, 1, "one subscriber, not two");

    test.teardown().await;
}

/// The subscriptions of a subscriber are listed oldest first, and those made at the
/// same time by the identifier of their block. Subscribing again does not move a
/// subscription: it keeps its creation time.
#[tokio::test]
async fn subscriptions_of_a_subscriber_are_listed_oldest_first() {
    let test = common::setup().await;
    let store = &test.store;

    let user = first_user(store).await;
    let blocks = create_test_blocks(store, &user.id, 4).await;
    // Subscribed to in an order that is not the order of the identifiers.
    let in_order = [&blocks[2], &blocks[0], &blocks[3], &blocks[1]];
    for block in in_order {
        store
            .create_subscription(subscription(block, &user))
            .await
            .expect("the user subscribes");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let listed_blocks = async || -> Vec<String> {
        store
            .get_subscriptions(&user.id)
            .await
            .expect("the subscriptions of the user are listed")
            .into_iter()
            .map(|subscription| subscription.block_id)
            .collect()
    };
    let expected: Vec<String> = in_order.iter().map(|block| block.id.clone()).collect();
    assert_eq!(listed_blocks().await, expected, "oldest first");

    store
        .create_subscription(subscription(in_order[0], &user))
        .await
        .expect("subscribing twice is accepted");
    assert_eq!(
        listed_blocks().await,
        expected,
        "the subscription that was renewed is still the oldest"
    );

    // Made at the same time, as far as the store knows: listed by block.
    common::execute_raw(
        &test.database_connection(),
        "UPDATE subscriptions SET created_at = 1",
    )
    .await;
    assert_eq!(
        listed_blocks().await,
        common::sorted_ids(&blocks),
        "subscriptions of one time come by the identifier of their block"
    );

    test.teardown().await;
}

/// Unsubscribing deletes the subscription: it is not found, not listed with its
/// subscriber or its block and not counted, and unsubscribing a second time returns
/// not-found.
#[tokio::test]
async fn unsubscribing_deletes_the_subscription() {
    let test = common::setup().await;
    let store = &test.store;

    let users = create_test_users(store, 2).await;
    let (leaver, stayer) = (&users[0], &users[1]);
    let block = first_block(store, &leaver.id).await;
    for user in [leaver, stayer] {
        store
            .create_subscription(subscription(&block, user))
            .await
            .expect("the user subscribes");
    }

    store
        .delete_subscription(&block.id, &leaver.id)
        .await
        .expect("the user unsubscribes");

    let missing = store
        .get_subscription(&block.id, &leaver.id)
        .await
        .expect_err("the subscription is gone");
    assert!(
        matches!(missing, StoreError::NotFound(_)),
        "got {missing:?}"
    );
    assert_eq!(
        missing.to_string(),
        format!(
            "{{subscription BlockID={} UserID={}}} not found",
            block.id, leaver.id
        ),
        "the error names the subscription"
    );
    let again = store
        .delete_subscription(&block.id, &leaver.id)
        .await
        .expect_err("there is nothing to unsubscribe from a second time");
    assert!(matches!(again, StoreError::NotFound(_)), "got {again:?}");
    assert_eq!(
        again.to_string(),
        missing.to_string(),
        "the second unsubscription fails as the lookup does"
    );

    assert!(
        subscribed_blocks(store, leaver).await.is_empty(),
        "the user has no subscription left"
    );
    let subscribers = store
        .get_subscribers_for_block(&block.id)
        .await
        .expect("the subscribers of the block are listed");
    let user_ids: Vec<&str> = subscribers
        .iter()
        .map(|subscriber| subscriber.user_id.as_str())
        .collect();
    assert_eq!(
        user_ids,
        [stayer.id.as_str()],
        "the block lists the subscriber who stays, and only them"
    );
    let count = store
        .get_subscribers_count_for_block(&block.id)
        .await
        .expect("the subscribers are counted");
    assert_eq!(count, 1, "the count is without the one who left");

    test.teardown().await;
}

/// Subscribing again after unsubscribing starts a new subscription: nothing of the old
/// one is left to keep, so the creation time and the block type are the new ones.
#[tokio::test]
async fn subscribing_again_after_unsubscribing_starts_a_new_subscription() {
    let test = common::setup().await;
    let store = &test.store;

    let user = first_user(store).await;
    let block = first_block(store, &user.id).await;
    let sub = subscription(&block, &user);

    let first = store
        .create_subscription(sub.clone())
        .await
        .expect("the user subscribes");
    tokio::time::sleep(Duration::from_millis(5)).await;
    store
        .delete_subscription(&block.id, &user.id)
        .await
        .expect("the user unsubscribes");

    let second = store
        .create_subscription(Subscription {
            block_type: BlockType::Attachment,
            ..sub
        })
        .await
        .expect("the user subscribes again");
    let stored = store
        .get_subscription(&block.id, &user.id)
        .await
        .expect("the new subscription is found");
    assert_eq!(stored, second, "stored as the second subscription returned");
    assert!(
        stored.created_at > first.created_at,
        "the creation time is the new one: {} after {}",
        stored.created_at,
        first.created_at
    );
    assert_eq!(
        stored.notified_at, stored.created_at,
        "a new subscription counts as notified when it was made"
    );
    assert_eq!(
        stored.block_type,
        BlockType::Attachment,
        "the block type is the new one"
    );
    assert_eq!(
        subscribed_blocks(store, &user).await,
        std::slice::from_ref(&block.id),
        "one subscription"
    );

    test.teardown().await;
}

/// A subscription requires an existing block and an existing user. If either is missing,
/// creating it returns not-found on both SQLite and PostgreSQL and stores nothing.
#[tokio::test]
async fn a_subscription_to_no_block_or_by_no_user_is_refused() {
    let test = common::setup().await;
    let store = &test.store;

    let user = first_user(store).await;
    let block = first_block(store, &user.id).await;
    let (no_block, nobody) = (new_id(), new_id());

    for (block_id, user_id, what) in [
        (&no_block, &user.id, "no such block"),
        (&block.id, &nobody, "no such user"),
        (&no_block, &nobody, "neither"),
    ] {
        let error = store
            .create_subscription(Subscription {
                block_id: block_id.clone(),
                user_id: user_id.clone(),
                ..subscription(&block, &user)
            })
            .await
            .expect_err("a subscription that refers to nothing is refused");
        assert!(
            matches!(error, StoreError::NotFound(_)),
            "{what}: got {error:?}"
        );

        let missing = store
            .get_subscription(block_id, user_id)
            .await
            .expect_err("the refused subscription is not stored");
        assert!(missing.is_not_found(), "{what}: got {missing:?}");
    }
    assert!(
        subscribed_blocks(store, &user).await.is_empty(),
        "the user is subscribed to nothing"
    );
    let count = store
        .get_subscribers_count_for_block(&block.id)
        .await
        .expect("the subscribers are counted");
    assert_eq!(count, 0, "the block has no subscriber");

    test.teardown().await;
}

/// The identifiers of the subscribers of a block, sorted.
async fn block_subscribers(store: &Store, block_id: &str) -> Vec<String> {
    let mut subscribers: Vec<String> = store
        .get_subscribers_for_block(block_id)
        .await
        .expect("the subscribers are listed")
        .into_iter()
        .map(|subscriber| subscriber.user_id)
        .collect();
    subscribers.sort();
    subscribers
}

/// Marks a user row as erased for tests that verify erased subscribers do not return.
async fn mark_erased(test: &common::TestStore, user_id: &str) {
    test.system()
        .execute_unchecked(&format!(
            "UPDATE users SET deleted_at = 1, erase_started_at = 1, erased_at = 1 WHERE id = '{user_id}'"
        ))
        .await
        .expect("the account is erased");
}

/// The subscriptions to a block go into the trash with the block, alone or below what
/// is deleted, and come back with it as they were: but that of a subscriber whose
/// account was erased meanwhile. Subscriptions to other blocks stay where they are.
#[tokio::test]
async fn deleting_a_block_takes_the_subscriptions_to_it_into_the_trash_and_back() {
    let test = common::setup().await;
    let store = &test.store;

    let users = create_test_users(store, 3).await;
    let (author, reader, erased) = (&users[0], &users[1], &users[2]);
    let card = first_block(store, &author.id).await;
    let mut below = vec![Block {
        parent_id: card.id.clone(),
        block_type: BlockType::Attachment,
        ..card_on(&card.container_id, &author.id)
    }];
    common::insert_blocks(store, &mut below, &author.id).await;
    let paragraph = &below[0];
    let mut others = vec![card_on(&card.container_id, &author.id)];
    common::insert_blocks(store, &mut others, &author.id).await;
    let other_card = &others[0];

    for user in [author, reader, erased] {
        for block in [&card, paragraph, other_card] {
            store
                .create_subscription(subscription(block, user))
                .await
                .expect("the user subscribes");
        }
    }
    let before = store
        .get_subscriptions(&reader.id)
        .await
        .expect("the subscriptions are listed");

    // The card goes, and the text on it with it.
    common::delete_blocks(store, std::slice::from_ref(&card), &author.id).await;
    for user in [author, reader] {
        assert_eq!(
            subscribed_blocks(store, user).await,
            std::slice::from_ref(&other_card.id),
            "only the subscription to the block that exists is listed"
        );
    }
    for gone in [&card, paragraph] {
        assert_eq!(
            store
                .get_subscribers_count_for_block(&gone.id)
                .await
                .expect("the subscribers are counted"),
            0,
            "a deleted block has no subscribers"
        );
    }
    mark_erased(&test, &erased.id).await;

    let restored = store
        .undelete_block(&card.id, &author.id)
        .await
        .expect("the card is restored");
    assert_eq!(restored.id, card.id, "the restored card");
    let on_the_board = store
        .get_blocks(&BlockFilter::container(&card.container_id))
        .await
        .expect("the blocks of the board are listed");
    assert!(
        common::contains_block_with_id(&on_the_board, &paragraph.id),
        "the text came back with its card: {on_the_board:?}"
    );
    let mut all = vec![card.id.clone(), paragraph.id.clone(), other_card.id.clone()];
    all.sort();
    for user in [author, reader] {
        assert_eq!(
            subscribed_blocks(store, user).await,
            all,
            "the restored blocks come back with their subscriptions"
        );
    }
    assert_eq!(
        store
            .get_subscriptions(&reader.id)
            .await
            .expect("the subscriptions are listed"),
        before,
        "as they were made, and last notified"
    );
    let mut expected = vec![author.id.clone(), reader.id.clone()];
    expected.sort();
    for back in [&card, paragraph] {
        assert_eq!(
            block_subscribers(store, &back.id).await,
            expected,
            "but whose account was erased"
        );
    }

    // Deleted again and gone for good, they go with it.
    common::delete_blocks(store, std::slice::from_ref(&card), &author.id).await;
    store
        .purge_trash(i64::MAX)
        .await
        .expect("the trash is emptied");
    assert_eq!(
        subscribed_blocks(store, reader).await,
        std::slice::from_ref(&other_card.id),
        "only the subscription to the block that exists is left"
    );

    test.teardown().await;
}

/// The subscriptions to the blocks of a board go into the trash with the board, and
/// come back when it is restored with its blocks. Subscriptions to the blocks of other
/// boards stay.
#[tokio::test]
async fn deleting_a_board_takes_the_subscriptions_to_its_blocks_into_the_trash_and_back() {
    let test = common::setup().await;
    let store = &test.store;

    let users = create_test_users(store, 2).await;
    let (author, reader) = (&users[0], &users[1]);
    // Each test block is on a board of its own.
    let blocks = create_test_blocks(store, &author.id, 2).await;
    let (doomed, elsewhere) = (&blocks[0], &blocks[1]);
    for user in [author, reader] {
        for block in [doomed, elsewhere] {
            store
                .create_subscription(subscription(block, user))
                .await
                .expect("the user subscribes");
        }
    }

    store
        .delete_container(&doomed.container_id, &author.id)
        .await
        .expect("the board is deleted");
    for user in [author, reader] {
        assert_eq!(
            subscribed_blocks(store, user).await,
            std::slice::from_ref(&elsewhere.id),
            "only the subscription on the other board is listed"
        );
    }

    store
        .undelete_container(&doomed.container_id, &author.id)
        .await
        .expect("the board is restored");
    let back = store
        .get_blocks(&BlockFilter::container(&doomed.container_id))
        .await
        .expect("the blocks of the restored board are listed");
    assert_eq!(
        common::sorted_ids(&back),
        std::slice::from_ref(&doomed.id),
        "the block came back with its board"
    );
    let mut both = vec![doomed.id.clone(), elsewhere.id.clone()];
    both.sort();
    for user in [author, reader] {
        assert_eq!(
            subscribed_blocks(store, user).await,
            both,
            "the restored board's blocks come back with their subscriptions"
        );
    }

    test.teardown().await;
}

/// Subscribers come back ordered by the time they were last notified, and a
/// subscription that was ended is not among them.
#[tokio::test]
async fn subscribers_are_ordered_by_notification_time() {
    let test = common::setup().await;
    let store = &test.store;

    let users = create_test_users(store, 3).await;
    let block = first_block(store, &users[0].id).await;
    for user in users.iter().rev() {
        store
            .create_subscription(subscription(&block, user))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    let subscribers = store.get_subscribers_for_block(&block.id).await.unwrap();
    let ids: Vec<&str> = subscribers
        .iter()
        .map(|subscriber| subscriber.user_id.as_str())
        .collect();
    let expected: Vec<&str> = users.iter().rev().map(|user| user.id.as_str()).collect();
    assert_eq!(ids, expected, "earliest notified first");
    assert!(
        subscribers
            .iter()
            .all(|subscriber| subscriber.notified_at != 0),
        "got {subscribers:?}"
    );

    store
        .delete_subscription(&block.id, &users[1].id)
        .await
        .unwrap();
    assert_eq!(
        store
            .get_subscribers_count_for_block(&block.id)
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        store
            .get_subscribers_for_block(&block.id)
            .await
            .unwrap()
            .len(),
        2
    );

    test.teardown().await;
}

/// Removing a member ends that user's subscriptions to the board's blocks, and nobody
/// else's and nothing on another board.
#[tokio::test]
async fn delete_member_ends_the_subscriptions_of_that_user_to_the_blocks_of_that_board() {
    let test = common::setup().await;
    let store = &test.store;
    let users = create_test_users(store, 2).await;
    let (leaver, other) = (&users[0], &users[1]);
    // Each test block is on a board of its own; put two more on the first one's board.
    let blocks = create_test_blocks(store, &leaver.id, 2).await;
    let (on_board, elsewhere) = (&blocks[0], &blocks[1]);
    let board_id = on_board.container_id.clone();
    let mut more = vec![
        card_on(&board_id, &leaver.id),
        card_on(&board_id, &leaver.id),
    ];
    common::insert_blocks(store, &mut more, &leaver.id).await;
    let (ended_before, also_on_board) = (&more[0], &more[1]);
    for user in [leaver, other] {
        let member = ContainerMember {
            container_id: board_id.clone(),
            user_id: user.id.clone(),
            role: fvoci_model::ContainerRole::Editor,
            synthetic: false,
            inherited_role: None,
        };
        store
            .save_member(&member)
            .await
            .expect("the user becomes a member");
        for block in [on_board, elsewhere, ended_before, also_on_board] {
            store
                .create_subscription(subscription(block, user))
                .await
                .expect("the user subscribes");
        }
    }
    store
        .delete_subscription(&ended_before.id, &leaver.id)
        .await
        .expect("the user unsubscribes from one block");

    let sorted = |blocks: &[&Block]| {
        let mut ids: Vec<String> = blocks.iter().map(|block| block.id.clone()).collect();
        ids.sort();
        ids
    };
    assert_eq!(
        subscribed_blocks(store, leaver).await,
        sorted(&[on_board, elsewhere, also_on_board]),
        "before leaving"
    );

    // Removing a user who is no member ends nothing.
    store
        .delete_member(&elsewhere.container_id, &leaver.id)
        .await
        .expect("removing a user who is no member succeeds");
    assert_eq!(
        subscribed_blocks(store, leaver).await,
        sorted(&[on_board, elsewhere, also_on_board]),
        "no membership, nothing ended"
    );

    store
        .delete_member(&board_id, &leaver.id)
        .await
        .expect("the member is removed");

    assert_eq!(
        subscribed_blocks(store, leaver).await,
        sorted(&[elsewhere]),
        "only the subscription on another board is left"
    );
    assert_eq!(
        subscribed_blocks(store, other).await,
        sorted(&[on_board, elsewhere, ended_before, also_on_board]),
        "the subscriptions of a user who stays"
    );

    // Subscribing again starts a new subscription, as after unsubscribing.
    store
        .create_subscription(subscription(on_board, leaver))
        .await
        .expect("the user subscribes again");
    assert_eq!(
        subscribed_blocks(store, leaver).await,
        sorted(&[on_board, elsewhere]),
        "subscribed again"
    );

    test.teardown().await;
}

/// A member who is removed with the check for the last administrator loses their
/// subscriptions to the board's blocks as with `delete_member`; a removal that is
/// refused, or of a user who is no member, ends nothing.
#[tokio::test]
async fn remove_member_ends_subscriptions_only_when_it_removes() {
    let test = common::setup().await;
    let store = &test.store;
    let users = create_test_users(store, 3).await;
    let (admin, editor, stranger) = (&users[0], &users[1], &users[2]);
    // Each test block is on a board of its own.
    let blocks = create_test_blocks(store, &admin.id, 2).await;
    let (on_board, elsewhere) = (&blocks[0], &blocks[1]);
    let board_id = &on_board.container_id;
    for (user, role) in [
        (admin, fvoci_model::ContainerRole::Admin),
        (editor, fvoci_model::ContainerRole::Editor),
    ] {
        let member = ContainerMember {
            container_id: board_id.clone(),
            user_id: user.id.clone(),
            role,
            synthetic: false,
            inherited_role: None,
        };
        store
            .save_member(&member)
            .await
            .expect("the user becomes a member");
    }
    for user in [admin, editor, stranger] {
        for block in [on_board, elsewhere] {
            store
                .create_subscription(subscription(block, user))
                .await
                .expect("the user subscribes");
        }
    }
    let both = common::sorted_ids(&blocks);

    let refused = store
        .remove_member(board_id, &admin.id)
        .await
        .expect("the removal is answered");
    assert_eq!(
        refused,
        MemberChange::LastAdmin,
        "the only administrator stays"
    );
    assert_eq!(
        subscribed_blocks(store, admin).await,
        both,
        "a refused removal ends no subscription"
    );

    let no_member = store
        .remove_member(board_id, &stranger.id)
        .await
        .expect("the removal is answered");
    assert_eq!(no_member, MemberChange::NotAMember, "never a member");
    assert_eq!(
        subscribed_blocks(store, stranger).await,
        both,
        "removing a user who is no member ends no subscription"
    );

    let removed = store
        .remove_member(board_id, &editor.id)
        .await
        .expect("the removal is answered");
    assert_eq!(removed, MemberChange::Done, "the editor is removed");
    assert_eq!(
        subscribed_blocks(store, editor).await,
        std::slice::from_ref(&elsewhere.id),
        "only the subscription on another board is left"
    );
    assert_eq!(
        subscribed_blocks(store, admin).await,
        both,
        "the subscriptions of the member who stays"
    );

    test.teardown().await;
}
