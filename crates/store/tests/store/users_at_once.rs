//! What two writes of a user's account at once leave: the password and the bag of client state.

use std::sync::Arc;

use fvoci_model::ClientStatePatch;
use fvoci_store::StoreError;
use tokio::sync::Barrier;
use tokio::task::JoinSet;

use crate::common;
use crate::users::{has_session, open_session_under};

/// A password is changed from the hash the old password was checked against. If the
/// stored hash is another one by then, nothing is changed: not the hash, not when the
/// user was last changed, not a session, and no session answers to the new digest.
#[tokio::test]
async fn change_user_password_from_a_stale_hash_changes_nothing() {
    let test = common::setup().await;
    let store = &test.store;
    let alice = common::user(store, "alice").await;
    let _ = store
        .reset_user_password("alice", "current-hash")
        .await
        .expect("the password is set");
    let here = open_session_under(store, &alice, "alice-here", "current-hash").await;
    open_session_under(store, &alice, "alice-phone", "current-hash").await;
    let before = store
        .get_user_by_id(&alice)
        .await
        .expect("the user is read");

    for stale in ["stale-hash", "", "CURRENT-HASH"] {
        let changed = store
            .change_user_password(&alice, stale, "new-hash", &here, "alice-here-anew")
            .await
            .expect("a change from another hash is answered");
        assert!(changed.is_none(), "{stale:?} is not the stored hash");

        let after = store
            .get_user_by_id(&alice)
            .await
            .expect("the user is read");
        assert_eq!(
            after, before,
            "{stale:?}: the user is as it was, hash included"
        );
        for digest in ["alice-here", "alice-phone"] {
            assert!(
                has_session(store, digest).await,
                "{stale:?}: the session {digest} is where it was"
            );
        }
        assert!(
            !has_session(store, "alice-here-anew").await,
            "{stale:?}: no session answers to the digest of the refused change"
        );
    }

    test.teardown().await;
}

/// Two changes of one password that start from the same hash: one is made, and the
/// other is told that the password is another one by now. The hash, and the session
/// that is left, are those of the change that was made.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn of_two_password_changes_from_the_same_hash_one_is_made() {
    const ROUNDS: usize = 40;

    let test = common::setup().await;
    let store = &test.store;
    let alice = common::user(store, "alice").await;

    let mut seen = String::new();
    for round in 0..ROUNDS {
        let start = Arc::new(Barrier::new(2));
        let mut racers = JoinSet::new();
        for device in ["laptop", "phone"] {
            let digest = format!("{device}-{round}");
            let session_id = open_session_under(store, &alice, &digest, &seen).await;
            let (store, start) = (store.clone(), Arc::clone(&start));
            let (alice, seen) = (alice.clone(), seen.clone());
            racers.spawn(async move {
                let new_hash = format!("hash-{digest}");
                let new_digest = format!("{digest}-anew");
                start.wait().await;
                let changed = store
                    .change_user_password(&alice, &seen, &new_hash, &session_id, &new_digest)
                    .await
                    .expect("a change of the password is answered")
                    .is_some();
                (changed, digest, new_hash, new_digest)
            });
        }
        let outcomes = racers.join_all().await;

        let made = outcomes.iter().filter(|(changed, ..)| *changed).count();
        assert_eq!(
            made, 1,
            "round {round}: exactly one change is made: {outcomes:?}"
        );
        let stored = store
            .get_user_by_id(&alice)
            .await
            .expect("the user is read");
        for (changed, digest, new_hash, new_digest) in &outcomes {
            assert_eq!(
                stored.password_hash == *new_hash,
                *changed,
                "round {round}: the stored hash {} is that of the change that was made, \
                 {digest}: {changed}",
                stored.password_hash
            );
            assert_eq!(
                has_session(store, new_digest).await,
                *changed,
                "round {round}: only the session of the change that was made is left, under \
                 its new digest: {digest}: {changed}"
            );
            assert!(
                !has_session(store, digest).await,
                "round {round}: no session is found under the old digest {digest}"
            );
        }
        seen = stored.password_hash;
    }

    test.teardown().await;
}

/// Many changes at once that each add entries under names of their own never leave the
/// bag with more entries than it holds: the room that is left is given out once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn changes_at_once_never_overfill_the_bag_of_client_state() {
    const ROUNDS: usize = 12;
    /// How many changes add an entry at once.
    const ADDERS: usize = 12;
    /// How many entries fit when they start.
    const ROOM: usize = 5;

    let test = common::setup().await;
    let store = &test.store;
    let limit = fvoci_model::CLIENT_STATE_MAX_ENTRIES;
    let entries = |names: Vec<String>| ClientStatePatch {
        updated_fields: names
            .into_iter()
            .map(|name| (name, "v".to_owned()))
            .collect(),
        ..ClientStatePatch::default()
    };

    for round in 0..ROUNDS {
        let user_id = common::user(store, &format!("collector-{round}")).await;
        let before = (0..limit - ROOM)
            .map(|index| format!("before-{index:03}"))
            .collect();
        store
            .patch_client_state(&user_id, &entries(before))
            .await
            .expect("the bag takes what leaves room");

        let start = Arc::new(Barrier::new(ADDERS));
        let mut adders = JoinSet::new();
        for adder in 0..ADDERS {
            let (store, start) = (store.clone(), Arc::clone(&start));
            let user_id = user_id.clone();
            let patch = entries(vec![format!("added-{adder:02}")]);
            adders.spawn(async move {
                start.wait().await;
                store.patch_client_state(&user_id, &patch).await
            });
        }
        let outcomes = adders.join_all().await;

        let mut added = 0;
        for outcome in &outcomes {
            match outcome {
                Ok(bag) => {
                    added += 1;
                    assert!(
                        bag.len() <= limit,
                        "round {round}: a change answered a bag of {} entries",
                        bag.len()
                    );
                }
                Err(StoreError::TooManyClientStateEntries { limit: said }) => {
                    assert_eq!(*said, limit, "round {round}: the limit the refusal names");
                }
                Err(other) => panic!("round {round}: a change failed otherwise: {other:?}"),
            }
        }
        let stored = store
            .get_client_state(&user_id)
            .await
            .expect("the bag is read");
        assert!(
            stored.len() <= limit,
            "round {round}: the bag holds {} entries, more than {limit}",
            stored.len()
        );
        assert_eq!(
            stored.len(),
            limit - ROOM + added,
            "round {round}: what was there and what the {added} changes that succeeded added"
        );
        assert_eq!(
            added, ROOM,
            "round {round}: the room that was left is used, and no more"
        );
    }

    test.teardown().await;
}
