//! An account's change of its address: it waits for one address at a time, is made
//! by its link once and within its time, only while the account is in use and the
//! address is nobody's, and takes the account's reset credentials with it.

use fvoci_model::{User, new_id};
use fvoci_store::{
    EMAIL_CHANGE_LIFETIME_MILLIS, EMAIL_CHANGE_LINK_LIFETIME_MILLIS, SIGNUP_LIFETIME_MILLIS,
    Signup, SignupStart, Store, StoreError, Taken,
};

use crate::common;

const T0: i64 = 1_000_000;
const HASH: &str = "hash";

async fn account(store: &Store, name: &str) -> User {
    let mut user = User {
        id: new_id(),
        username: name.to_owned(),
        email: format!("{name}@old.example.test"),
        password_hash: HASH.to_owned(),
        ..User::default()
    };
    store.create_user(&mut user).await.expect("an account");
    user
}

async fn email_of(store: &Store, user_id: &str) -> String {
    store
        .get_user_by_id(user_id)
        .await
        .expect("the account")
        .email
}

/// Has the account wait for `email` and gives the change a link under `digest`.
async fn waiting(store: &Store, user: &User, email: &str, digest: &str, now: i64) {
    assert!(
        store
            .ask_email_change(&user.id, HASH, email, now)
            .await
            .expect("asked"),
        "the account waits for {email}"
    );
    assert!(
        store
            .renew_email_change_link(&user.id, email, digest, now)
            .await
            .expect("a link"),
        "the change has a link"
    );
}

#[tokio::test]
async fn a_change_is_made_by_its_link_once_and_takes_the_reset_credentials() {
    let test = common::setup().await;
    let store = &test.store;
    let ada = account(store, "ada").await;
    assert!(
        !store
            .ask_email_change(&ada.id, "another hash", "new@example.test", T0)
            .await
            .expect("asked"),
        "not under a password that is the account's no more"
    );
    assert_eq!(
        store.email_change_of(&ada.id, T0).await.expect("read"),
        None
    );

    assert!(
        store
            .create_password_reset(&ada.id, &ada.email, "reset", T0)
            .await
            .expect("a reset credential")
    );
    waiting(store, &ada, "New@example.test", "link", T0).await;
    assert_eq!(
        store.email_change_of(&ada.id, T0 + 1).await.expect("read"),
        Some("New@example.test".to_owned())
    );
    assert_eq!(email_of(store, &ada.id).await, ada.email, "not changed yet");
    assert!(
        !store
            .email_is_held("new@example.test", &new_id(), T0)
            .await
            .expect("asked"),
        "an address that waits is nobody's"
    );
    assert_eq!(
        store
            .confirm_email_change("another link", "back 1", T0 + 1)
            .await
            .expect("asked"),
        None
    );

    let changed = store
        .confirm_email_change("link", "back 2", T0 + 1)
        .await
        .expect("confirmed")
        .expect("the change is made");
    assert_eq!(
        (
            changed.user_id.as_str(),
            changed.username.as_str(),
            changed.old_email.as_str()
        ),
        (ada.id.as_str(), "ada", ada.email.as_str())
    );
    assert_eq!(email_of(store, &ada.id).await, "New@example.test");
    assert_eq!(
        store.email_change_of(&ada.id, T0 + 2).await.expect("read"),
        None
    );
    assert_eq!(
        store
            .confirm_email_change("link", "back 3", T0 + 2)
            .await
            .expect("asked"),
        None,
        "a link is used once"
    );
    assert_eq!(
        store
            .consume_password_reset("reset", "set by the link", T0 + 2)
            .await
            .expect("asked"),
        None,
        "the reset credential mailed to the address before is gone"
    );
    // And none is made for a mail to the address before, whatever its letter case.
    for (email, stored) in [(ada.email.as_str(), false), ("NEW@EXAMPLE.TEST", true)] {
        assert_eq!(
            store
                .create_password_reset(&ada.id, email, &format!("for {email}"), T0 + 3)
                .await
                .expect("asked"),
            stored,
            "a reset credential mailed to {email}"
        );
    }
    test.teardown().await;
}

#[tokio::test]
async fn an_account_waits_for_one_address_within_its_time() {
    let test = common::setup().await;
    let store = &test.store;
    let ada = account(store, "ada").await;

    // A new asking replaces the one before, and its link.
    waiting(store, &ada, "first@example.test", "first", T0).await;
    waiting(store, &ada, "second@example.test", "second", T0 + 1).await;
    assert_eq!(test.count_rows("email_changes").await.expect("counted"), 1);
    assert_eq!(
        store
            .confirm_email_change("first", "back 4", T0 + 2)
            .await
            .expect("asked"),
        None
    );
    // A link is not given to a change the account no longer waits for.
    assert!(
        !store
            .renew_email_change_link(&ada.id, "first@example.test", "late", T0 + 2)
            .await
            .expect("asked")
    );
    // A newer link ends the one before it.
    assert!(
        store
            .renew_email_change_link(&ada.id, "second@example.test", "newer", T0 + 2)
            .await
            .expect("asked")
    );
    for (digest, stands) in [("second", false), ("newer", true)] {
        assert_eq!(
            store
                .email_change_link_stands(&ada.id, digest, T0 + 3)
                .await
                .expect("asked"),
            stands,
            "{digest}"
        );
    }
    // The link ends a day after its mail; the change, seven days after it was asked.
    let link_ended = T0 + 2 + EMAIL_CHANGE_LINK_LIFETIME_MILLIS;
    assert_eq!(
        store
            .confirm_email_change("newer", "back 5", link_ended)
            .await
            .expect("asked"),
        None
    );
    assert_eq!(
        store
            .email_change_of(&ada.id, link_ended)
            .await
            .expect("read"),
        Some("second@example.test".to_owned()),
        "the account waits still, for a new mail"
    );
    let ended = T0 + 1 + EMAIL_CHANGE_LIFETIME_MILLIS;
    assert_eq!(
        store.email_change_of(&ada.id, ended).await.expect("read"),
        None
    );
    assert!(
        !store
            .renew_email_change_link(&ada.id, "second@example.test", "too late", ended)
            .await
            .expect("asked")
    );
    store.remove_ended_signups(ended).await.expect("cleaned up");
    assert_eq!(test.count_rows("email_changes").await.expect("counted"), 0);

    // Taken back, the link confirms nothing.
    waiting(store, &ada, "third@example.test", "third", ended).await;
    assert!(
        store
            .cancel_email_change(&ada.id)
            .await
            .expect("taken back")
    );
    assert!(!store.cancel_email_change(&ada.id).await.expect("asked"));
    assert_eq!(
        store
            .confirm_email_change("third", "back 6", ended + 1)
            .await
            .expect("asked"),
        None
    );
    assert_eq!(email_of(store, &ada.id).await, ada.email);
    test.teardown().await;
}

#[tokio::test]
async fn an_address_that_is_somebodys_is_not_taken_by_a_link() {
    let test = common::setup().await;
    let store = &test.store;
    let (ada, bob) = (account(store, "ada").await, account(store, "bob").await);

    // Another account's, in any letter case.
    let bobs = bob.email.to_uppercase();
    assert!(
        store
            .email_is_held(&bobs, &ada.id, T0)
            .await
            .expect("asked")
    );
    assert!(
        !store
            .email_is_held(&bobs, &bob.id, T0)
            .await
            .expect("asked"),
        "an account's own address is not held from it"
    );
    waiting(store, &ada, &bobs, "to bob's", T0).await;
    assert_eq!(
        store
            .confirm_email_change("to bob's", "back 7", T0 + 1)
            .await
            .expect("asked"),
        None
    );
    assert_eq!(email_of(store, &ada.id).await, ada.email);
    assert_eq!(email_of(store, &bob.id).await, bob.email);

    // A registration's that has not ended.
    let signup = Signup {
        id: new_id(),
        username: "cy".to_owned(),
        email: "Cy@example.test".to_owned(),
        custom_name: String::new(),
        password_hash: HASH.to_owned(),
        language: String::new(),
        start: SignupStart::Organization("Ours".to_owned()),
        browser_hash: String::new(),
        created_at: T0,
        expires_at: T0 + SIGNUP_LIFETIME_MILLIS,
    };
    store
        .create_signup(&signup, None, T0)
        .await
        .expect("a registration");
    assert!(
        store
            .email_is_held("cy@example.test", &ada.id, T0 + 1)
            .await
            .expect("asked")
    );
    waiting(store, &ada, "cy@example.test", "to cy's", T0 + 1).await;
    assert_eq!(
        store
            .confirm_email_change("to cy's", "back 8", T0 + 2)
            .await
            .expect("asked"),
        None
    );
    assert_eq!(email_of(store, &ada.id).await, ada.email);

    // Without mail the change is made at once, and says what is in its way.
    for email in [bobs.as_str(), "cy@example.test"] {
        let refused = store.change_email_now(&ada.id, HASH, email, T0 + 3).await;
        assert!(
            matches!(refused, Err(StoreError::Conflict(Taken::Email))),
            "{email}: {refused:?}"
        );
    }
    assert_eq!(
        store
            .change_email_now(&ada.id, "another hash", "free@example.test", T0 + 3)
            .await
            .expect("asked"),
        None,
        "not under a password that is the account's no more"
    );
    // Its own address in another letter case is in nobody's way.
    for email in [ada.email.to_uppercase().as_str(), "free@example.test"] {
        let before = email_of(store, &ada.id).await;
        let changed = store
            .change_email_now(&ada.id, HASH, email, T0 + 3)
            .await
            .expect("changed")
            .expect("the account is in use");
        assert_eq!(changed.old_email, before);
        assert_eq!(email_of(store, &ada.id).await, email);
    }
    test.teardown().await;
}

/// A link to an address somebody has changes nothing, itself included: the change
/// waits on with it, and once nobody has the address the link makes it. A text that is
/// no link changes nothing either.
#[tokio::test]
async fn a_link_to_an_address_somebody_has_stays_for_when_it_is_free() {
    let test = common::setup().await;
    let store = &test.store;
    let (ada, bob) = (account(store, "ada").await, account(store, "bob").await);
    waiting(store, &ada, &bob.email, "to bob's", T0).await;
    for _ in 0..2 {
        assert_eq!(
            store
                .confirm_email_change("to bob's", "back 9", T0 + 1)
                .await
                .expect("asked"),
            None
        );
        assert_eq!(
            store
                .email_change_of(&ada.id, T0 + 1)
                .await
                .expect("read")
                .as_deref(),
            Some(bob.email.as_str()),
            "the change waits as it did"
        );
    }
    assert_eq!(
        store
            .confirm_email_change("no link", "back 10", T0 + 1)
            .await
            .expect("asked"),
        None
    );
    store
        .change_email_now(&bob.id, HASH, "bob@new.example.test", T0 + 2)
        .await
        .expect("changed")
        .expect("the account is in use");
    let changed = store
        .confirm_email_change("to bob's", "back 11", T0 + 3)
        .await
        .expect("asked")
        .expect("the link stands, and nobody has the address");
    assert_eq!(changed.old_email, ada.email);
    assert_eq!(email_of(store, &ada.id).await, bob.email);
    assert_eq!(
        store.email_change_of(&ada.id, T0 + 3).await.expect("read"),
        None
    );
    test.teardown().await;
}

#[tokio::test]
async fn a_change_that_waits_goes_with_the_password_and_with_the_account() {
    let test = common::setup().await;
    let store = &test.store;
    let ada = account(store, "ada").await;
    let changes = async || test.count_rows("email_changes").await.expect("counted");

    waiting(store, &ada, "one@example.test", "one", T0).await;
    assert!(
        store
            .change_user_password(&ada.id, HASH, "second", &new_id(), "no digest")
            .await
            .expect("the password is changed")
            .is_some()
    );
    assert_eq!(changes().await, 0, "with a change of the password");
    assert_eq!(
        store
            .confirm_email_change("one", "back 12", T0 + 1)
            .await
            .expect("asked"),
        None
    );

    assert!(
        store
            .ask_email_change(&ada.id, "second", "two@example.test", T0)
            .await
            .expect("asked")
    );
    assert!(
        store
            .create_password_reset(&ada.id, &ada.email, "reset", T0)
            .await
            .expect("a reset credential")
    );
    assert!(
        store
            .consume_password_reset("reset", "third", T0 + 1)
            .await
            .expect("consumed")
            .is_some()
    );
    assert_eq!(changes().await, 0, "with a reset of the password");

    assert!(
        store
            .ask_email_change(&ada.id, "third", "three@example.test", T0)
            .await
            .expect("asked")
    );
    let _ = store
        .reset_user_password("ada", "fourth")
        .await
        .expect("the operator sets a password");
    assert_eq!(changes().await, 0, "with a password the operator sets");

    // An account that is not in use neither waits nor is given a link, nor changed.
    assert!(
        store
            .ask_email_change(&ada.id, "fourth", "four@example.test", T0)
            .await
            .expect("asked")
            && store
                .renew_email_change_link(&ada.id, "four@example.test", "four", T0)
                .await
                .expect("a link")
    );
    test.execute_unchecked(&format!(
        "UPDATE users SET deleted_at = {T0} WHERE id = '{}'",
        ada.id
    ))
    .await
    .expect("put out of use");
    assert!(
        !store
            .ask_email_change(&ada.id, "fourth", "five@example.test", T0 + 1)
            .await
            .expect("asked")
    );
    assert!(
        !store
            .renew_email_change_link(&ada.id, "four@example.test", "newer", T0 + 1)
            .await
            .expect("asked")
    );
    assert_eq!(
        store
            .confirm_email_change("four", "back 13", T0 + 1)
            .await
            .expect("asked"),
        None
    );
    assert_eq!(
        store
            .change_email_now(&ada.id, "fourth", "five@example.test", T0 + 1)
            .await
            .expect("asked"),
        None
    );
    test.teardown().await;
}
