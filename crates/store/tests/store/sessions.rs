//! Sessions: found by the digest of their token, ended by disuse, by their end time, and
//! with the password they were opened under.

use std::sync::Arc;

use fvoci_model::{Session, new_id};
use fvoci_store::{Store, StoreError};
use tokio::sync::Barrier;

use crate::common;

/// A time in milliseconds that the tests count from.
const T0: i64 = 1_800_000_000_000;

/// A session of the user that was made at [`T0`] and ends, whatever its use, at `T0 +
/// 1000`.
fn session_of(user_id: &str) -> Session {
    Session {
        id: new_id(),
        user_id: user_id.to_owned(),
        created_at: T0,
        last_used_at: T0,
        expires_at: T0 + 1000,
        user_agent: "a browser".to_owned(),
        ..Session::default()
    }
}

/// The password hash of a user made by [`common::user`]: none.
const NO_HASH: &str = "";

/// Stores the session under the digest, for a user whose password hash is still
/// [`NO_HASH`].
async fn open(store: &Store, session: &Session, digest: &str) {
    let stored = store
        .create_session(session, digest, NO_HASH)
        .await
        .expect("the session is stored");
    assert!(
        stored,
        "{digest}: the password hash is the one seen, so the session is stored"
    );
}

/// Whether the store has a valid session under the digest at `now`, counting as idle
/// what was not used after `idle_since`.
async fn is_valid(store: &Store, digest: &str, now: i64, idle_since: i64) -> bool {
    match store.get_session(digest, now, idle_since).await {
        Ok(_) => true,
        Err(StoreError::NoRows) => false,
        Err(other) => panic!("looking a session up failed: {other:?}"),
    }
}

/// A session is created, read back and deleted.
#[tokio::test]
async fn create_and_get_and_delete_session() {
    let test = common::setup().await;
    let store = &test.store;
    let user_id = common::user(store, "user").await;

    // "CreateAndGetSession"
    let session = session_of(&user_id);
    open(store, &session, "digest").await;
    let got = store
        .get_session("digest", T0 + 1, T0 - 1)
        .await
        .expect("the session is found by its digest");
    assert_eq!(got, session, "as it was stored");

    // "Get nonexistent session"
    let error = store
        .get_session("another-digest", T0 + 1, T0 - 1)
        .await
        .expect_err("no session has that digest");
    assert!(matches!(error, StoreError::NoRows), "got {error:?}");
    assert!(error.is_not_found(), "which counts as not found");

    // "DeleteAndGetSession"
    let _ = store
        .delete_session(&session.id)
        .await
        .expect("the session is deleted");
    assert!(
        !is_valid(store, "digest", T0 + 1, T0 - 1).await,
        "a deleted session is gone"
    );
    let _ = store
        .delete_session(&session.id)
        .await
        .expect("deleting it again succeeds");
    let _ = store
        .delete_session(&new_id())
        .await
        .expect("as does deleting one that never was");

    test.teardown().await;
}

/// A session is stored under what the caller gives as the digest, and a digest names
/// one session.
#[tokio::test]
async fn a_session_is_found_by_its_digest_and_two_cannot_share_one() {
    let test = common::setup().await;
    let store = &test.store;
    let user_id = common::user(store, "user").await;

    open(store, &session_of(&user_id), "digest-one").await;
    let error = store
        .create_session(&session_of(&user_id), "digest-one", NO_HASH)
        .await
        .expect_err("a second session under the same digest is refused");
    assert!(matches!(error, StoreError::Database(_)), "got {error:?}");
    open(store, &session_of(&user_id), "digest-two").await;

    let same_id = session_of(&user_id);
    open(store, &same_id, "digest-three").await;
    let error = store
        .create_session(&same_id, "digest-four", NO_HASH)
        .await
        .expect_err("a session identifier is used once");
    assert!(matches!(error, StoreError::Database(_)), "got {error:?}");

    test.teardown().await;
}

/// A session that has no user, text for one or a user who does not exist is not stored:
/// nobody has the password hash that was seen. A session whose own identifier is text
/// that is none is refused as such.
#[tokio::test]
async fn a_session_needs_identifiers() {
    let test = common::setup().await;
    let store = &test.store;
    let user_id = common::user(store, "user").await;

    for user in [String::new(), "nobody".to_owned(), new_id()] {
        let digest = new_id();
        let stored = store
            .create_session(&session_of(&user), &digest, NO_HASH)
            .await
            .expect("a session of nobody is answered");
        assert!(!stored, "{user:?} is no user: the session is not stored");
        assert!(
            !is_valid(store, &digest, T0 + 1, T0 - 1).await,
            "{user:?}: nothing is found under the digest"
        );
    }

    let session = Session {
        id: "no-identifier".to_owned(),
        ..session_of(&user_id)
    };
    let digest = new_id();
    let error = store
        .create_session(&session, &digest, NO_HASH)
        .await
        .expect_err("text that is no identifier is refused");
    assert!(
        matches!(&error, StoreError::InvalidId(refused) if refused == "no-identifier"),
        "got {error:?}"
    );
    assert!(
        !is_valid(store, &digest, T0 + 1, T0 - 1).await,
        "nothing is found under the digest of the refused session"
    );

    test.teardown().await;
}

/// A session ends when it was not used after the idle limit, and at its end time
/// whatever its use. Using it moves the first and never the second.
#[tokio::test]
async fn a_session_ends_by_disuse_and_at_its_end_time() {
    let test = common::setup().await;
    let store = &test.store;
    let user_id = common::user(store, "user").await;
    let session = session_of(&user_id);
    open(store, &session, "digest").await;

    // Used at T0. With "idle since" before that, it is in use; at or after, it is idle.
    assert!(is_valid(store, "digest", T0 + 1, T0 - 1).await, "in use");
    assert!(
        !is_valid(store, "digest", T0 + 1, T0).await,
        "not used after the limit: idle"
    );
    assert!(!is_valid(store, "digest", T0 + 1, T0 + 5).await, "idle");

    // Using it moves the time it was last used.
    store
        .touch_session(&session.id, T0 + 10)
        .await
        .expect("the use is recorded");
    assert!(
        is_valid(store, "digest", T0 + 11, T0 + 5).await,
        "in use again"
    );
    let touched = store
        .get_session("digest", T0 + 11, T0 + 5)
        .await
        .expect("the session is found");
    assert_eq!(touched.last_used_at, T0 + 10, "the recorded use");
    assert_eq!(touched.created_at, T0, "the creation time stays");
    assert_eq!(touched.expires_at, T0 + 1000, "and so does the end time");

    // The end time is the end, however recently the session was used.
    store
        .touch_session(&session.id, T0 + 999)
        .await
        .expect("the use is recorded");
    assert!(
        is_valid(store, "digest", T0 + 999, T0 + 900).await,
        "the moment before the end"
    );
    assert!(
        !is_valid(store, "digest", T0 + 1000, T0 + 900).await,
        "at its end time the session has ended"
    );
    assert!(
        !is_valid(store, "digest", T0 + 5000, T0 + 900).await,
        "and after"
    );

    // Recording the use of a session that is not there changes nothing.
    store
        .touch_session(&new_id(), T0)
        .await
        .expect("touching no session succeeds");

    test.teardown().await;
}

/// A session of a user who is deactivated has ended, whatever its times; it is valid
/// again if the user is active again. Another user's session is not touched.
#[tokio::test]
async fn a_session_of_a_deactivated_user_is_not_found() {
    let test = common::setup().await;
    let user_id = common::user(&test, "leaving").await;
    let other_id = common::user(&test, "staying").await;
    for (id, digest) in [(&user_id, "digest-leaving"), (&other_id, "digest-staying")] {
        open(&test, &session_of(id), digest).await;
    }
    assert!(
        is_valid(&test, "digest-leaving", T0 + 1, T0 - 1).await,
        "valid while the user is active"
    );

    common::execute_raw(
        &test.database_connection(),
        &format!("UPDATE users SET deleted_at = {T0} WHERE id = '{user_id}'"),
    )
    .await;
    assert!(
        !is_valid(&test, "digest-leaving", T0 + 1, T0 - 1).await,
        "ended with the deactivation"
    );
    assert!(
        is_valid(&test, "digest-staying", T0 + 1, T0 - 1).await,
        "the other user's session is their own"
    );

    common::execute_raw(
        &test.database_connection(),
        &format!("UPDATE users SET deleted_at = NULL WHERE id = '{user_id}'"),
    )
    .await;
    assert!(
        is_valid(&test, "digest-leaving", T0 + 1, T0 - 1).await,
        "valid again once the user is active again"
    );

    test.teardown().await;
}

/// The clean-up deletes exactly the sessions that have ended: the idle ones and the
/// ones past their end time.
#[tokio::test]
async fn clean_up_deletes_the_sessions_that_have_ended() {
    let test = common::setup().await;
    let store = &test.store;
    let user_id = common::user(store, "user").await;

    let live = session_of(&user_id);
    let idle = Session {
        last_used_at: T0 - 500,
        created_at: T0 - 500,
        ..session_of(&user_id)
    };
    let past_its_end = Session {
        expires_at: T0 + 50,
        ..session_of(&user_id)
    };
    for (session, digest) in [(&live, "live"), (&idle, "idle"), (&past_its_end, "past")] {
        open(store, session, digest).await;
    }

    let now = T0 + 100;
    let idle_since = T0 - 100;
    let mut deleted = match store
        .clean_up_sessions(now, idle_since)
        .await
        .expect("the clean-up runs")
        .into_scope()
    {
        fvoci_store::EndedSessionScope::Sessions(ids) => ids,
        fvoci_store::EndedSessionScope::None => Vec::new(),
        other => panic!("unexpected scope: {other:?}"),
    };
    deleted.sort();
    let mut ended = [idle.id.clone(), past_its_end.id.clone()];
    ended.sort();
    assert_eq!(deleted, ended, "the idle one and the one past its end");
    assert!(
        is_valid(store, "live", now, idle_since).await,
        "the live one stays"
    );
    // Gone, not only refused: with limits that would accept them they are not found.
    assert!(
        !is_valid(store, "idle", now, T0 - 1000).await,
        "the idle one"
    );
    assert!(
        !is_valid(store, "past", T0, idle_since).await,
        "the ended one"
    );
    assert_eq!(
        store
            .clean_up_sessions(now, idle_since)
            .await
            .expect("the clean-up runs again")
            .into_scope(),
        fvoci_store::EndedSessionScope::None,
        "nothing is left to delete"
    );

    test.teardown().await;
}

/// A password set by name ends the sessions of that user, and only theirs, together
/// with the password change.
#[tokio::test]
async fn reset_user_password_deletes_the_sessions_of_that_user_only() {
    let test = common::setup().await;
    let store = &test.store;
    let alice = common::user(store, "alice").await;
    let bob = common::user(store, "bob").await;
    for (user, digest) in [(&alice, "alice-1"), (&alice, "alice-2"), (&bob, "bob-1")] {
        open(store, &session_of(user), digest).await;
    }

    let fvoci_store::PasswordSet { user_id: reset, .. } = store
        .reset_user_password("alice", "new-hash")
        .await
        .expect("the password is set");
    assert_eq!(reset, alice, "the user whose password was set");

    for digest in ["alice-1", "alice-2"] {
        assert!(
            !is_valid(store, digest, T0 + 1, T0 - 1).await,
            "{digest} ended with the old password"
        );
    }
    assert!(
        is_valid(store, "bob-1", T0 + 1, T0 - 1).await,
        "another user's session stays"
    );
    let stored = store
        .get_user_by_id(&alice)
        .await
        .expect("the user is read");
    assert_eq!(stored.password_hash, "new-hash", "the new hash is stored");

    // No such user: nothing changes.
    let error = store
        .reset_user_password("nobody", "hash")
        .await
        .expect_err("no user has that name");
    assert!(
        matches!(error, StoreError::UserNotFound(_)),
        "got {error:?}"
    );
    assert!(is_valid(store, "bob-1", T0 + 1, T0 - 1).await, "untouched");

    test.teardown().await;
}

/// A password a user changes themselves ends their other sessions and keeps the one it
/// was changed through.
#[tokio::test]
async fn change_user_password_keeps_one_session_and_ends_the_others() {
    let test = common::setup().await;
    let store = &test.store;
    let alice = common::user(store, "alice").await;
    let bob = common::user(store, "bob").await;
    let kept = session_of(&alice);
    open(store, &kept, "alice-here").await;
    for (user, digest) in [
        (&alice, "alice-phone"),
        (&alice, "alice-old"),
        (&bob, "bob"),
    ] {
        open(store, &session_of(user), digest).await;
    }

    let changed = store
        .change_user_password(&alice, NO_HASH, "new-hash", &kept.id, "alice-here-anew")
        .await
        .expect("the password is changed");
    assert!(changed.is_some(), "the hash was still the one seen");

    assert!(
        is_valid(store, "alice-here-anew", T0 + 1, T0 - 1).await,
        "the session the change came through stays, under its new token"
    );
    assert!(
        !is_valid(store, "alice-here", T0 + 1, T0 - 1).await,
        "and is no longer found under the old one"
    );
    let kept_now = store
        .get_session("alice-here-anew", T0 + 1, T0 - 1)
        .await
        .expect("the session is read");
    assert_eq!(kept_now.id, kept.id, "the same session");
    for digest in ["alice-phone", "alice-old"] {
        assert!(
            !is_valid(store, digest, T0 + 1, T0 - 1).await,
            "{digest} ended with the old password"
        );
    }
    assert!(
        is_valid(store, "bob", T0 + 1, T0 - 1).await,
        "another user's session stays"
    );
    let stored = store
        .get_user_by_id(&alice)
        .await
        .expect("the user is read");
    assert_eq!(stored.password_hash, "new-hash", "the new hash is stored");

    // A user who is not there has no hash to replace: nothing is changed, and no
    // session is touched.
    let changed = store
        .change_user_password(&new_id(), NO_HASH, "hash", &kept.id, "unused")
        .await
        .expect("a change for no user is answered");
    assert!(changed.is_none(), "no such user");
    assert!(
        is_valid(store, "alice-here-anew", T0 + 1, T0 - 1).await,
        "untouched: the session is where it was"
    );
    assert!(
        !is_valid(store, "unused", T0 + 1, T0 - 1).await,
        "and not under the token of the refused change"
    );

    test.teardown().await;
}

/// A hash is replaced by a newer one of the same password only while it is still the
/// stored one, and sessions stay.
#[tokio::test]
async fn replace_password_hash_replaces_the_hash_it_was_made_from() {
    let test = common::setup().await;
    let store = &test.store;
    let alice = common::user(store, "alice").await;
    let _ = store
        .reset_user_password("alice", "old-hash")
        .await
        .expect("a first hash");
    let stored = store
        .create_session(&session_of(&alice), "alice", "old-hash")
        .await
        .expect("the session is stored");
    assert!(stored, "the hash is the one the password was set to");

    assert!(
        store
            .replace_password_hash(&alice, "old-hash", "current-hash")
            .await
            .expect("the statement runs"),
        "the stored hash is the one named: replaced"
    );
    assert!(
        !store
            .replace_password_hash(&alice, "old-hash", "another-hash")
            .await
            .expect("the statement runs"),
        "the password hash changed in the meantime: left alone"
    );
    assert!(
        !store
            .replace_password_hash(&new_id(), "old-hash", "another-hash")
            .await
            .expect("the statement runs"),
        "no such user"
    );
    let stored = store
        .get_user_by_id(&alice)
        .await
        .expect("the user is read");
    assert_eq!(stored.password_hash, "current-hash");
    assert!(
        is_valid(store, "alice", T0 + 1, T0 - 1).await,
        "the password did not change, the session stays"
    );

    test.teardown().await;
}

/// The time a session was last used, as the store has it.
async fn last_used(store: &Store, digest: &str) -> i64 {
    store
        .get_session(digest, T0 + 1, i64::MIN)
        .await
        .expect("the session is found")
        .last_used_at
}

/// A session is stored only while the password hash of its user is the one the password
/// was checked against. With another hash nothing is stored, and the answer says so.
#[tokio::test]
async fn create_session_with_a_hash_that_is_not_the_users_stores_nothing() {
    let test = common::setup().await;
    let store = &test.store;
    let alice = common::user(store, "alice").await;
    let _ = store
        .reset_user_password("alice", "current-hash")
        .await
        .expect("the password is set");

    let session = session_of(&alice);
    for seen in ["stale-hash", NO_HASH, "CURRENT-HASH", "current-hash "] {
        let stored = store
            .create_session(&session, "digest", seen)
            .await
            .expect("a session opened with another hash is answered");
        assert!(!stored, "{seen:?} is not the user's hash");
        assert!(
            !is_valid(store, "digest", T0 + 1, T0 - 1).await,
            "{seen:?}: no session is found under the digest"
        );
    }

    // The same session under the same digest is stored with the hash that is the
    // user's: none of the refused calls left a row with its identifier or its digest.
    let stored = store
        .create_session(&session, "digest", "current-hash")
        .await
        .expect("a session opened with the user's hash is answered");
    assert!(stored, "the hash is the user's");
    let found = store
        .get_session("digest", T0 + 1, T0 - 1)
        .await
        .expect("the session is found by its digest");
    assert_eq!(found, session, "as it was given");

    test.teardown().await;
}

/// A user who is deactivated gets no new session, whatever hash was seen.
#[tokio::test]
async fn create_session_for_a_deactivated_user_stores_nothing() {
    let test = common::setup().await;
    let store = &test.store;
    let user_id = common::user(store, "leaving").await;
    common::execute_raw(
        &test.database_connection(),
        &format!("UPDATE users SET deleted_at = {T0} WHERE id = '{user_id}'"),
    )
    .await;

    let session = session_of(&user_id);
    let stored = store
        .create_session(&session, "digest", NO_HASH)
        .await
        .expect("a session of a deactivated user is answered");
    assert!(!stored, "the user is deactivated");

    // Active again, the user gets the same session: the refused call stored nothing.
    common::execute_raw(
        &test.database_connection(),
        &format!("UPDATE users SET deleted_at = NULL WHERE id = '{user_id}'"),
    )
    .await;
    assert!(
        !is_valid(store, "digest", T0 + 1, T0 - 1).await,
        "nothing waits under the digest for the user to come back"
    );
    open(store, &session, "digest").await;

    test.teardown().await;
}

/// A password is set anew while a session is opened with the old one. Whichever comes
/// first, no session that was opened with the old password is there afterwards: it was
/// refused, or the reset ended it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_opened_with_the_old_password_does_not_outlive_a_reset() {
    const ROUNDS: usize = 100;

    let test = common::setup().await;
    let store = &test.store;
    let alice = common::user(store, "alice").await;

    let mut old_hash = NO_HASH.to_owned();
    let (mut refused, mut ended) = (0, 0);
    for round in 0..ROUNDS {
        let new_hash = format!("hash-{round}");
        let digest = format!("digest-{round}");

        let start = Arc::new(Barrier::new(2));
        let opening = tokio::spawn({
            let (store, start) = (store.clone(), Arc::clone(&start));
            let (session, digest, seen) = (session_of(&alice), digest.clone(), old_hash.clone());
            async move {
                start.wait().await;
                store.create_session(&session, &digest, &seen).await
            }
        });
        let resetting = tokio::spawn({
            let (store, start) = (store.clone(), Arc::clone(&start));
            let new_hash = new_hash.clone();
            async move {
                start.wait().await;
                store.reset_user_password("alice", &new_hash).await
            }
        });
        let stored = opening
            .await
            .expect("the opening task runs to its end")
            .unwrap_or_else(|error| panic!("round {round}: opening a session: {error:?}"));
        let _ = resetting
            .await
            .expect("the resetting task runs to its end")
            .unwrap_or_else(|error| panic!("round {round}: setting the password: {error:?}"));

        assert!(
            !is_valid(store, &digest, T0 + 1, T0 - 1).await,
            "round {round}: a session opened with the old password (stored: {stored}) is \
             there after the reset"
        );
        if stored {
            ended += 1;
        } else {
            refused += 1;
        }
        old_hash = new_hash;
    }

    // No session of any round is left under any digest: a clean-up that would take
    // every session finds none.
    let left = store
        .clean_up_sessions(i64::MAX, i64::MAX)
        .await
        .expect("the clean-up runs");
    assert_eq!(
        left.into_scope(),
        fvoci_store::EndedSessionScope::None,
        "{refused} sessions were refused and {ended} ended by the reset: none is left"
    );

    test.teardown().await;
}

/// The time a session was last used only moves forward: an older use, or the same one
/// again, changes nothing.
#[tokio::test]
async fn touch_session_does_not_move_the_time_backwards() {
    let test = common::setup().await;
    let store = &test.store;
    let user_id = common::user(store, "user").await;
    let session = session_of(&user_id);
    open(store, &session, "digest").await;

    for (now, expected, what) in [
        (T0 + 50, T0 + 50, "a later use is recorded"),
        (T0 + 20, T0 + 50, "an older use changes nothing"),
        (T0 - 20, T0 + 50, "nor does one from before the session"),
        (T0 + 50, T0 + 50, "nor the same use again"),
        (T0 + 51, T0 + 51, "the next later use is recorded"),
    ] {
        store
            .touch_session(&session.id, now)
            .await
            .expect("the use is recorded");
        assert_eq!(
            last_used(store, "digest").await,
            expected,
            "touched at T0 {:+}: {what}",
            now - T0
        );
    }

    // A test may set the time to anything, also back; from there it moves forward again.
    store
        .set_session_last_used(&session.id, T0 + 5)
        .await
        .expect("the time is set");
    assert_eq!(last_used(store, "digest").await, T0 + 5, "set back");
    store
        .touch_session(&session.id, T0 + 10)
        .await
        .expect("the use is recorded");
    assert_eq!(
        last_used(store, "digest").await,
        T0 + 10,
        "forward from there"
    );

    test.teardown().await;
}

/// The clean-up says which sessions it deleted: their identifiers, each once, and no
/// session that it left. A session that ends at this very moment has ended.
#[tokio::test]
async fn clean_up_returns_the_identifiers_of_the_sessions_it_deleted() {
    let test = common::setup().await;
    let store = &test.store;
    let alice = common::user(store, "alice").await;
    let bob = common::user(store, "bob").await;
    let now = T0 + 100;
    let idle_since = T0 + 40;

    // The name of each session is its digest; `last_used_at` is set after it is stored.
    let cases = [
        ("in-use", &alice, T0 + 90, T0 + 1000, false),
        ("used-at-the-limit", &alice, idle_since, T0 + 1000, true),
        (
            "used-after-the-limit",
            &alice,
            idle_since + 1,
            T0 + 1000,
            false,
        ),
        ("ends-now", &alice, T0 + 90, now, true),
        ("ends-next", &alice, T0 + 90, now + 1, false),
        ("idle-and-ended", &alice, T0, T0 + 50, true),
        ("of-another-user-idle", &bob, T0 + 10, T0 + 1000, true),
        ("of-another-user-in-use", &bob, T0 + 90, T0 + 1000, false),
    ];
    let mut ended = Vec::new();
    for (digest, user_id, last_used_at, expires_at, has_ended) in cases {
        let session = Session {
            expires_at,
            ..session_of(user_id)
        };
        open(store, &session, digest).await;
        store
            .set_session_last_used(&session.id, last_used_at)
            .await
            .expect("the time of the last use is set");
        if has_ended {
            ended.push(session.id);
        }
    }
    ended.sort();

    let mut deleted = match store
        .clean_up_sessions(now, idle_since)
        .await
        .expect("the clean-up runs")
        .into_scope()
    {
        fvoci_store::EndedSessionScope::Sessions(ids) => ids,
        fvoci_store::EndedSessionScope::None => Vec::new(),
        other => panic!("unexpected scope: {other:?}"),
    };
    deleted.sort();
    assert_eq!(deleted, ended, "exactly the sessions that have ended");

    // What it named is gone and what it did not name is there, under limits that would
    // accept every session.
    for (digest, _, _, _, has_ended) in cases {
        assert_eq!(
            is_valid(store, digest, i64::MIN, i64::MIN).await,
            !has_ended,
            "{digest}"
        );
    }

    test.teardown().await;
}

/// The sessions counted live are those `get_session` would find: not ended by disuse or
/// at their end time, and of a user who is not deactivated.
#[tokio::test]
async fn the_live_sessions_are_those_that_would_be_found() {
    let test = common::setup().await;
    let user_id = common::user(&test, "counted").await;
    let leaving_id = common::user(&test, "leaving").await;
    assert_eq!(
        test.count_live_sessions(T0 + 1, T0 - 1)
            .await
            .expect("counted"),
        0
    );
    let used_later = session_of(&user_id);
    open(&test, &session_of(&user_id), "digest-1").await;
    open(&test, &used_later, "digest-2").await;
    open(&test, &session_of(&leaving_id), "digest-3").await;
    test.touch_session(&used_later.id, T0 + 100)
        .await
        .expect("the use is recorded");
    let live = async |now: i64, idle_since: i64| {
        test.count_live_sessions(now, idle_since)
            .await
            .expect("counted")
    };
    assert_eq!(live(T0 + 1, T0 - 1).await, 3, "all in use");
    assert_eq!(live(T0 + 101, T0 + 50).await, 1, "one used after the limit");
    assert_eq!(live(T0 + 1000, T0 - 1).await, 0, "all at their end time");
    assert_eq!(live(T0 + 999, T0 - 1).await, 3, "the moment before");
    assert_eq!(
        live(T0 + 1, T0).await,
        1,
        "used at the limit is idle, as get_session has it"
    );
    assert!(!is_valid(&test, "digest-1", T0 + 1, T0).await);
    assert!(is_valid(&test, "digest-2", T0 + 1, T0).await);
    common::execute_raw(
        &test.database_connection(),
        &format!("UPDATE users SET deleted_at = {T0} WHERE id = '{leaving_id}'"),
    )
    .await;
    assert_eq!(
        live(T0 + 1, T0 - 1).await,
        2,
        "a deactivated user's session is not live"
    );
    test.teardown().await;
}

/// The readiness check reads the sessions; on PostgreSQL it gives up by itself behind a
/// lock on the table, before the server's two seconds, and its connection comes back.
/// On SQLite there is no such lock to hold; a reader waits only for a writer, up to the
/// busy timeout, and only when the database is not in WAL mode.
#[tokio::test]
async fn a_readiness_check_behind_a_lock_on_the_sessions_gives_up_by_itself() {
    let test = common::setup().await;
    test.check_ready().await.expect("ready without a lock");
    if common::db_type() == fvoci_store::DbType::Postgres {
        let mut holder = common::connect_past_the_store(&test.database_connection()).await;
        let mut lock = sqlx::Connection::begin(&mut holder)
            .await
            .expect("a transaction of the test's own");
        sqlx::raw_sql("LOCK TABLE sessions IN ACCESS EXCLUSIVE MODE")
            .execute(&mut *lock)
            .await
            .expect("the table lock is held");
        let checked =
            tokio::time::timeout(std::time::Duration::from_secs(2), test.check_ready()).await;
        assert!(
            matches!(checked, Ok(Err(StoreError::Database(_)))),
            "the statement gave up by itself: {checked:?}"
        );
        lock.rollback().await.expect("the table lock is let go");
        sqlx::Connection::close(holder)
            .await
            .expect("the connection closes");
        test.check_ready()
            .await
            .expect("ready once the lock is gone");
    }
    test.teardown().await;
}
