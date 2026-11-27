//! The record that an account was given its welcome board: made once, with the board,
//! by one of the calls that would make it at once, not where no board came of it, and
//! taken back with a board that is.

use std::sync::Arc;

use fvoci_model::{DeleteContainersAndBlocks, new_id};
use fvoci_store::{CopyChanges, CopyTitles, Store};
use tokio::sync::Barrier;
use tokio::task::JoinSet;

use crate::common;

/// The titles of copies, in English, as a caller of the store brings them.
const TITLES: CopyTitles = CopyTitles {
    copy_of: |title| format!("{title} copy"),
    new_template: "New board template",
};

/// When the account is recorded as given its welcome board, as text: none if it is not.
async fn recorded(store: &Store, user_id: &str) -> Vec<String> {
    store
        .texts_unchecked(&format!(
            "SELECT CAST(welcomed_at AS TEXT) FROM users \
             WHERE CAST(id AS TEXT) = '{user_id}' AND welcomed_at IS NOT NULL"
        ))
        .await
        .expect("the record is read")
}

/// Of the welcome boards asked for an account at once, one is made, and none after;
/// one that cannot be copied records nothing, and the account is given it the next time.
#[tokio::test]
async fn one_welcome_board_is_made_of_those_asked_for_at_once() {
    let test = common::setup().await;
    let workspace_id = common::workspace(&test).await;
    let owner = common::user(&test, "owner").await;
    let alice = common::user(&test, "alice").await;
    let bob = common::user(&test, "bob").await;
    for user_id in [&alice, &bob] {
        common::works_in(&test, &workspace_id, user_id).await;
    }
    let board = common::create_test_boards(&test, &workspace_id, &owner, 1)
        .await
        .remove(0);
    let boards_of = async |user_id: &str| {
        test.texts_unchecked(&format!(
            "SELECT CAST(id AS TEXT) FROM containers WHERE CAST(created_by AS TEXT) = '{user_id}'"
        ))
        .await
        .expect("the boards are read")
    };

    // No such board to copy: nothing is made, and nothing recorded.
    let missing = test
        .duplicate_welcome_board(&new_id(), &alice, &workspace_id, &TITLES, <_>::default(), 6)
        .await;
    assert!(missing.is_err_and(|error| error.is_not_found()));

    let barrier = Arc::new(Barrier::new(8));
    let mut asked = JoinSet::new();
    for _ in 0..8 {
        let (store, barrier) = (test.store.clone(), Arc::clone(&barrier));
        let (board_id, alice, workspace_id) =
            (board.id.clone(), alice.clone(), workspace_id.clone());
        asked.spawn(async move {
            barrier.wait().await;
            store
                .duplicate_welcome_board(
                    &board_id,
                    &alice,
                    &workspace_id,
                    &TITLES,
                    CopyChanges::default(),
                    7,
                )
                .await
                .expect("answered")
                .is_some()
        });
    }
    let made = asked.join_all().await;
    assert_eq!(made.iter().filter(|made| **made).count(), 1, "{made:?}");
    assert_eq!(boards_of(&alice).await.len(), 1, "one board");
    assert_eq!(recorded(&test, &alice).await, ["7"], "with the board");

    // Each account its own; a record made for a board that was asked for counts,
    // and is of the last one asked for.
    assert_eq!(recorded(&test, &bob).await, Vec::<String>::new());
    test.record_welcome(&bob, 8).await.unwrap();
    test.record_welcome(&bob, 9).await.unwrap();
    assert_eq!(recorded(&test, &bob).await, ["9"]);
    let again = test
        .duplicate_welcome_board(&board.id, &bob, &workspace_id, &TITLES, <_>::default(), 10)
        .await
        .expect("answered");
    assert_eq!(again, None);
    assert_eq!(boards_of(&bob).await, Vec::<String>::new());
    let nobody = test
        .duplicate_welcome_board(
            &board.id,
            &new_id(),
            &workspace_id,
            &TITLES,
            <_>::default(),
            10,
        )
        .await
        .expect("answered");
    assert_eq!(nobody, None, "nobody's account");
    test.teardown().await;
}

/// A welcome board that is taken back takes the record made with it along, and the
/// account is given its board the next time; a record made since, for another board,
/// stays, and so do the board and the record where the board cannot be deleted.
#[tokio::test]
async fn a_welcome_board_taken_back_takes_its_record_along() {
    let test = common::setup().await;
    let workspace_id = common::workspace(&test).await;
    let owner = common::user(&test, "owner").await;
    let carol = common::user(&test, "carol").await;
    let dave = common::user(&test, "dave").await;
    for user_id in [&carol, &dave] {
        common::works_in(&test, &workspace_id, user_id).await;
    }
    let template = common::create_test_boards(&test, &workspace_id, &owner, 1)
        .await
        .remove(0);
    let given = async |user_id: &str, now: i64| {
        let (copies, _members) = test
            .duplicate_welcome_board(
                &template.id,
                user_id,
                &workspace_id,
                &TITLES,
                <_>::default(),
                now,
            )
            .await
            .expect("answered")
            .expect("given");
        DeleteContainersAndBlocks::from(&copies)
    };
    let is_there = async |copies: &DeleteContainersAndBlocks| match test
        .get_container(&copies.containers[0])
        .await
    {
        Ok(_) => true,
        Err(error) if error.is_not_found() => false,
        Err(error) => panic!("the board is read: {error}"),
    };

    // Not deleted: a block that is none of the board's. Board and record stay.
    let carols = given(&carol, 7).await;
    let mut with_a_stranger = carols.clone();
    with_a_stranger.blocks.push(new_id());
    let refused = test
        .take_back_welcome_board(&with_a_stranger, &carol, 7)
        .await;
    assert!(refused.is_err_and(|error| error.is_not_found()));
    assert!(is_there(&carols).await);
    assert_eq!(recorded(&test, &carol).await, ["7"]);

    // Taken back with its record: the account is given its board the next time.
    test.take_back_welcome_board(&carols, &carol, 7)
        .await
        .expect("taken back");
    assert!(!is_there(&carols).await);
    assert_eq!(recorded(&test, &carol).await, Vec::<String>::new());
    let next = given(&carol, 8).await;
    assert!(is_there(&next).await);
    assert_eq!(recorded(&test, &carol).await, ["8"]);

    // A record made since is another board's: the board goes, and the record stays.
    let daves = given(&dave, 7).await;
    test.record_welcome(&dave, 9).await.unwrap();
    test.take_back_welcome_board(&daves, &dave, 7)
        .await
        .expect("taken back");
    assert!(!is_there(&daves).await);
    assert_eq!(recorded(&test, &dave).await, ["9"]);
    test.teardown().await;
}

/// Concurrent `change_instance_role` (when the instance's organization follows) and
/// `take_back_welcome_board` acquire `organizations` before `users`, preventing
/// PostgreSQL deadlocks.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_instance_role_change_and_take_back_welcome_board_never_deadlock() {
    let test = common::setup().await;
    let owner = crate::account_erasure::founder(&test).await;
    let alice = common::user(&test, "alice").await;
    common::works_in(&test, &test.workspace_id, &alice).await;
    let template = common::create_test_boards(&test, &test.workspace_id, &owner, 1)
        .await
        .remove(0);

    for round in 1..=8_i64 {
        let (copies, _) = test
            .duplicate_welcome_board(
                &template.id,
                &alice,
                &test.workspace_id,
                &TITLES,
                CopyChanges::default(),
                round,
            )
            .await
            .expect("duplicated")
            .expect("welcome board created");
        let delete_req = DeleteContainersAndBlocks::from(&copies);
        let barrier = Arc::new(Barrier::new(2));
        let (s1, b1, a1, o1) = (
            test.store.clone(),
            Arc::clone(&barrier),
            alice.clone(),
            owner.clone(),
        );
        let (s2, b2, a2) = (test.store.clone(), Arc::clone(&barrier), alice.clone());

        let role_task = tokio::spawn(async move {
            b1.wait().await;
            let promoted = s1
                .set_user_admin(&a1, fvoci_model::InstanceRole::Administrator, &o1)
                .await
                .expect("promote alice");
            assert!(
                matches!(promoted, fvoci_store::AdminChange::Done(_)),
                "alice promoted to instance administrator"
            );
            let demoted = s1
                .set_user_admin(&a1, fvoci_model::InstanceRole::User, &o1)
                .await
                .expect("demote alice");
            assert!(
                matches!(demoted, fvoci_store::AdminChange::Done(_)),
                "alice demoted back to regular user"
            );
        });
        let take_back_task = tokio::spawn(async move {
            b2.wait().await;
            s2.take_back_welcome_board(&delete_req, &a2, round)
                .await
                .expect("take back welcome board");
        });
        let (r1, r2) = tokio::join!(role_task, take_back_task);
        r1.expect("role task joined");
        r2.expect("take back task joined");
        assert_eq!(recorded(&test, &alice).await, Vec::<String>::new());
    }
    test.teardown().await;
}
