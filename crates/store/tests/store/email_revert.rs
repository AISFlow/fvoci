//! The link mailed to the address an account had, which gives it back: made by a
//! confirmed change, used once, ended by nothing but its use, its time, a link before
//! it that is used, and the erasing of the account; while it stands its address is the
//! account's; and using it ends the account's sessions and locks its password.

use std::time::Duration;

use fvoci_model::{Session, User, new_id};
use fvoci_store::{
    CarryOver, ConfirmedPassword, DeletionAsking, EMAIL_REVERT_LIFETIME_MILLIS, EmailReverted,
    Erasing, ErasureStart, HeldSignIn, Identity, LinkingSession, SIGNUP_LIFETIME_MILLIS, Signup,
    SignupStart, Store, StoreError, StoredProvider, Taken, Unlinked,
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

async fn user(store: &Store, user_id: &str) -> User {
    store.get_user_by_id(user_id).await.expect("the account")
}

/// Moves the account to `email` by a confirmed change at `now`, whose link back to the
/// address before is stored under `back`.
async fn moved(store: &Store, user_id: &str, email: &str, back: &str, now: i64) {
    let link = format!("to {email}");
    assert!(
        store
            .ask_email_change(user_id, HASH, email, now)
            .await
            .expect("asked"),
        "the account waits for {email}"
    );
    assert!(
        store
            .renew_email_change_link(user_id, email, &link, now)
            .await
            .expect("a link"),
        "the change has a link"
    );
    assert!(
        store
            .confirm_email_change(&link, back, now)
            .await
            .expect("confirmed")
            .is_some(),
        "the account has {email}"
    );
}

/// A session of the account made with its password.
async fn session(store: &Store, user_id: &str, digest: &str) -> Session {
    let session = Session {
        id: new_id(),
        user_id: user_id.to_owned(),
        created_at: T0,
        expires_at: T0 * 1_000,
        ..Session::default()
    };
    assert!(
        store
            .create_session(&session, digest, HASH)
            .await
            .expect("stored"),
        "a session is made with the password"
    );
    session
}

async fn revert(store: &Store, back: &str, now: i64) -> Option<EmailReverted> {
    store
        .revert_email_change(back, now)
        .await
        .expect("used")
        .map(|revert| revert.reverted)
}

/// A confirmed change leaves one link back to the address before, under its digest.
/// The link gives that address back once, and with it every session ends, the
/// password is locked, and the change that waited and the reset links go.
#[tokio::test]
async fn a_link_gives_the_address_before_back_once_and_ends_what_was_the_accounts() {
    let test = common::setup().await;
    let store = &test.store;
    account(store, "first").await;
    let ada = account(store, "ada").await;
    let kept = session(store, &ada.id, "ada's session").await;
    moved(store, &ada.id, "ada@new.example.test", "back", T0).await;
    assert_eq!(test.count_rows("email_reverts").await.expect("counted"), 1);
    // An account that had no address leaves no link.
    let mut bo = account(store, "bo").await;
    test.execute_unchecked(&format!(
        "UPDATE users SET email = '' WHERE id = '{}'",
        bo.id
    ))
    .await
    .expect("no address");
    bo.email.clear();
    moved(store, &bo.id, "bo@new.example.test", "bo's back", T0).await;
    assert_eq!(test.count_rows("email_reverts").await.expect("counted"), 1);

    // What waited and was mailed under the address that is the account's now.
    assert!(
        store
            .create_password_reset(&ada.id, "ada@new.example.test", "reset", T0 + 1)
            .await
            .expect("a reset credential")
    );
    assert!(
        store
            .ask_email_change(&ada.id, HASH, "ada@third.example.test", T0 + 1)
            .await
            .expect("asked")
    );

    assert_eq!(revert(store, "no such link", T0 + 2).await, None);
    let reverted = revert(store, "back", T0 + 2).await;
    assert_eq!(
        reverted,
        Some(EmailReverted {
            user_id: ada.id.clone(),
            username: ada.username.clone(),
            before: "ada@new.example.test".to_owned(),
            restored: Some(ada.email.clone()),
            unlinked_identities: 0,
        })
    );
    let now = user(store, &ada.id).await;
    assert_eq!(now.email, ada.email, "the address before is the account's");
    assert_eq!(
        now.password_locked_at,
        T0 + 2,
        "the password is locked as it was used"
    );
    assert_eq!(now.password_hash, HASH, "and kept");
    assert!(
        store.get_session("ada's session", T0 + 3, 0).await.is_err(),
        "the session {} ended",
        kept.id
    );
    assert_eq!(
        store.email_change_of(&ada.id, T0 + 3).await.expect("read"),
        None,
        "the change that waited ended"
    );
    assert_eq!(
        store
            .consume_password_reset("reset", "a hash", T0 + 3)
            .await
            .expect("tried"),
        None,
        "the reset link ended"
    );
    assert_eq!(revert(store, "back", T0 + 3).await, None, "used once");
    assert_eq!(test.count_rows("email_reverts").await.expect("counted"), 0);
    test.teardown().await;
}

/// Of the links of an account, the one used ends those made after it, also at the same
/// moment, and leaves those made before: whoever moved the address on cannot end the
/// link to the address before theirs by using their own.
#[tokio::test]
async fn a_link_ends_the_links_made_after_it_and_leaves_those_before() {
    let test = common::setup().await;
    let store = &test.store;
    account(store, "first").await;
    let ada = account(store, "ada").await;
    // A, then B, C and D, all at one moment.
    moved(store, &ada.id, "b@example.test", "to a", T0).await;
    moved(store, &ada.id, "c@example.test", "to b", T0).await;
    moved(store, &ada.id, "d@example.test", "to c", T0).await;

    let to_b = revert(store, "to b", T0 + 1).await.expect("used");
    assert_eq!(
        (to_b.before.as_str(), to_b.restored.as_deref()),
        ("d@example.test", Some("b@example.test"))
    );
    assert_eq!(revert(store, "to c", T0 + 1).await, None, "made after it");
    let to_a = revert(store, "to a", T0 + 2).await.expect("made before it");
    assert_eq!(to_a.restored.as_deref(), Some(ada.email.as_str()));
    assert_eq!(user(store, &ada.id).await.email, ada.email);

    // The first link used first ends all the others.
    let cy = account(store, "cy").await;
    moved(store, &cy.id, "cy-b@example.test", "cy to a", T0).await;
    moved(store, &cy.id, "cy-c@example.test", "cy to b", T0 + 1).await;
    assert!(revert(store, "cy to a", T0 + 2).await.is_some());
    assert_eq!(revert(store, "cy to b", T0 + 2).await, None);
    assert_eq!(user(store, &cy.id).await.email, cy.email);
    test.teardown().await;
}

/// Nothing the account does ends a link: not a new password, a reset, a change asked and
/// taken back, another change, nor the asking to delete the account. Its time does, and
/// the erasing of the account.
#[tokio::test]
async fn only_its_time_and_the_erasing_of_the_account_end_a_link() {
    let test = common::setup().await;
    let store = &test.store;
    account(store, "first").await;
    let ada = account(store, "ada").await;
    let kept = session(store, &ada.id, "ada's session").await;
    moved(store, &ada.id, "ada@new.example.test", "back", T0).await;
    assert!(
        store
            .change_user_password(&ada.id, HASH, "changed", &kept.id, "ada's next")
            .await
            .expect("changed")
            .is_some()
    );
    assert!(
        store
            .create_password_reset(&ada.id, "ada@new.example.test", "reset", T0 + 1)
            .await
            .expect("stored")
    );
    assert!(
        store
            .consume_password_reset("reset", HASH, T0 + 1)
            .await
            .expect("reset")
            .is_some()
    );
    assert!(
        store
            .ask_email_change(&ada.id, HASH, "ada@third.example.test", T0 + 2)
            .await
            .expect("asked")
    );
    assert!(store.cancel_email_change(&ada.id).await.expect("cancelled"));
    moved(
        store,
        &ada.id,
        "ada@fourth.example.test",
        "back again",
        T0 + 3,
    )
    .await;
    let asked = store
        .request_account_deletion(&ada.id, HASH, T0 + 4, T0 + 4 + 1_000)
        .await
        .expect("asked");
    assert!(matches!(asked, DeletionAsking::Asked(..)), "{asked:?}");

    let ended = T0 + EMAIL_REVERT_LIFETIME_MILLIS;
    let reverted = revert(store, "back", ended - 1)
        .await
        .expect("while the account waits to be deleted");
    assert_eq!(reverted.restored, Some(ada.email.clone()));
    assert_eq!(revert(store, "back again", ended).await, None, "made after");

    // Its time.
    let bo = account(store, "bo").await;
    moved(store, &bo.id, "bo@new.example.test", "bo's back", T0).await;
    assert_eq!(revert(store, "bo's back", ended).await, None);
    store.remove_ended_signups(ended).await.expect("cleaned");
    assert_eq!(test.count_rows("email_reverts").await.expect("counted"), 0);

    // The erasing of the account.
    let cy = account(store, "cy").await;
    moved(store, &cy.id, "cy@new.example.test", "cy's back", T0).await;
    assert!(matches!(
        store
            .begin_account_erasure(&cy.id, T0 + 1, Erasing::AtOnce)
            .await
            .expect("begun"),
        ErasureStart::Begun(_)
    ));
    assert_eq!(test.count_rows("email_reverts").await.expect("counted"), 0);
    assert_eq!(revert(store, "cy's back", T0 + 2).await, None);
    test.teardown().await;
}

/// While a link stands, its address is the account's to have back: no other account's
/// change, registration or new account takes it. The account itself may have it again.
#[tokio::test]
async fn while_a_link_stands_nobody_else_takes_its_address() {
    let test = common::setup().await;
    let store = &test.store;
    account(store, "first").await;
    let ada = account(store, "ada").await;
    let bob = account(store, "bob").await;
    // A new account is made at the time it is: so is the link here.
    let t0: i64 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("after the epoch")
        .as_millis()
        .try_into()
        .expect("milliseconds");
    moved(store, &ada.id, "ada@new.example.test", "back", t0).await;
    let held = ada.email.to_uppercase();
    assert!(
        store
            .email_is_held(&held, &bob.id, t0 + 1)
            .await
            .expect("asked")
    );
    assert!(
        !store
            .email_is_held(&held, &ada.id, t0 + 1)
            .await
            .expect("asked"),
        "the account's own link holds it for the account"
    );
    assert!(
        store
            .email_is_held_for_revert(&held, t0 + 1)
            .await
            .expect("asked")
    );

    assert!(
        store
            .ask_email_change(&bob.id, HASH, &held, t0 + 1)
            .await
            .expect("asked")
    );
    assert!(
        store
            .renew_email_change_link(&bob.id, &held, "bob's", t0 + 1)
            .await
            .expect("a link")
    );
    assert_eq!(
        store
            .confirm_email_change("bob's", "bob's back", t0 + 1)
            .await
            .expect("asked"),
        None,
        "another account's change does not take it"
    );
    let signup = Signup {
        id: new_id(),
        username: "cy".to_owned(),
        email: held.clone(),
        custom_name: String::new(),
        password_hash: HASH.to_owned(),
        language: String::new(),
        start: SignupStart::Alone,
        browser_hash: String::new(),
        created_at: t0 + 1,
        expires_at: t0 + 1 + SIGNUP_LIFETIME_MILLIS,
    };
    assert!(matches!(
        store.create_signup(&signup, None, t0 + 1).await,
        Err(StoreError::Conflict(Taken::Email))
    ));
    let mut dee = User {
        id: new_id(),
        username: "dee".to_owned(),
        email: held.clone(),
        password_hash: HASH.to_owned(),
        ..User::default()
    };
    assert!(matches!(
        store.create_user(&mut dee).await,
        Err(StoreError::Conflict(Taken::Email))
    ));

    // The account's own change back to it is no other account's.
    moved(store, &ada.id, &ada.email, "back to b", t0 + 2).await;
    assert_eq!(user(store, &ada.id).await.email, ada.email);
    let ended = t0 + EMAIL_REVERT_LIFETIME_MILLIS;
    assert!(
        !store
            .email_is_held_for_revert("ada@new.example.test", ended + 2)
            .await
            .expect("asked"),
        "a link that ended holds nothing"
    );
    test.teardown().await;
}

/// An address somebody else has by then (as data from before, or a race the claim on
/// the names did not see) is not given back; instead the account's email address is
/// cleared so the address set after the change cannot recover it, and any pending
/// change, sessions, and reset links go too — even when other accounts already have an
/// empty address.
#[tokio::test]
async fn an_address_somebody_else_has_clears_the_accounts_email_and_the_rest_is_done() {
    let test = common::setup().await;
    let store = &test.store;
    account(store, "first").await;
    let ada = account(store, "ada").await;
    let bob = account(store, "bob").await;
    let empty = account(store, "empty").await;
    test.execute_unchecked(&format!(
        "UPDATE users SET email = '' WHERE id = '{}'",
        empty.id
    ))
    .await
    .expect("another account with no email");
    session(store, &ada.id, "ada's session").await;
    moved(store, &ada.id, "ada@new.example.test", "back", T0).await;
    assert!(
        store
            .ask_email_change(&ada.id, HASH, "ada@third.example.test", T0 + 1)
            .await
            .expect("asked"),
        "pending change is stored"
    );
    assert!(
        store
            .create_password_reset(&ada.id, "ada@new.example.test", "reset", T0 + 1)
            .await
            .expect("a reset credential"),
        "reset credential is stored"
    );
    test.execute_unchecked(&format!(
        "UPDATE users SET email = '{}' WHERE id = '{}'",
        ada.email, bob.id
    ))
    .await
    .expect("planted");

    let reverted = revert(store, "back", T0 + 2).await.expect("used");
    assert_eq!(reverted.restored, None);
    let now = user(store, &ada.id).await;
    assert_eq!(now.email, "", "the account's email address is cleared");
    assert!(
        store
            .account_by_email("ada@new.example.test")
            .await
            .is_err(),
        "the previous address no longer finds the account"
    );
    assert_eq!(
        store.email_change_of(&ada.id, T0 + 3).await.expect("read"),
        None,
        "any pending change is discarded"
    );
    assert_eq!(
        store
            .consume_password_reset("reset", "a hash", T0 + 3)
            .await
            .expect("tried"),
        None,
        "reset links end"
    );
    assert!(now.password_is_locked());
    assert!(store.get_session("ada's session", T0 + 3, 0).await.is_err());
    test.teardown().await;
}

/// Reverting removes SSO identities linked strictly after the revert link's `created_at`
/// (`created_at > link.created_at`) and keeps those linked at or before it
/// (`created_at <= link.created_at`).
#[tokio::test]
async fn identities_linked_after_the_revert_link_are_removed_and_earlier_ones_stay() {
    let test = common::setup().await;
    let store = &test.store;
    account(store, "first").await;
    let ada = account(store, "ada").await;
    let bob = account(store, "bob").await;
    let issuer = "https://idp.example.test";
    let identity = |subject| Identity { issuer, subject };

    store
        .add_identity_at(&ada.id, identity("before"), &ada.email, T0 - 1)
        .await
        .expect("before");
    store
        .add_identity_at(&ada.id, identity("at-boundary"), &ada.email, T0)
        .await
        .expect("at boundary");
    moved(store, &ada.id, "ada@new.example.test", "back", T0).await;
    store
        .add_identity_at(&ada.id, identity("after"), "ada@new.example.test", T0 + 1)
        .await
        .expect("after");
    store
        .add_identity_at(&bob.id, identity("bobs"), &bob.email, T0 + 1)
        .await
        .expect("bob's");

    let reverted = revert(store, "back", T0 + 2).await.expect("reverted");
    assert_eq!(
        reverted.unlinked_identities, 1,
        "one later identity removed"
    );

    let left: Vec<String> = store
        .identities_of_user(&ada.id)
        .await
        .expect("read")
        .into_iter()
        .map(|tied| tied.subject)
        .collect();
    assert_eq!(
        left,
        ["before", "at-boundary"],
        "identities linked at or before link.created_at stay; strictly later ones go"
    );
    assert_eq!(
        store.identities_of_user(&bob.id).await.expect("read").len(),
        1,
        "another account's identities are untouched"
    );
    test.teardown().await;
}

fn oidc_provider(issuer: &str) -> StoredProvider {
    StoredProvider {
        id: String::new(),
        org_id: String::new(),
        issuer: issuer.to_owned(),
        client_id: "fvoci".to_owned(),
        sealed_client_secret: String::new(),
        auto_join: true,
        enabled: true,
        created_at: 0,
        updated_at: 0,
        sso_required: false,
        revision: 0,
    }
}

/// Identities linked through `finish_oidc_link` after an email change in the very same
/// millisecond — even after rapid changes that bumped `email_reverts.created_at` — still
/// receive `created_at > link.created_at` and are removed when the link is used.
#[tokio::test]
async fn a_same_millisecond_oidc_link_after_a_change_is_removed_on_revert() {
    let test = common::setup().await;
    let store = &test.store;
    account(store, "first").await;
    let issuer = "https://idp.example.test";
    let provider = store
        .put_oidc_provider(&oidc_provider(issuer))
        .await
        .expect("provider");
    let cy = account(store, "cy").await;
    let cy_session = session(store, &cy.id, "cy's session").await;
    let link_at = |now: i64| LinkingSession {
        session_id: &cy_session.id,
        token_digest: "cy's session",
        now,
        idle_since: 0,
    };
    let identity = |subject| Identity { issuer, subject };
    assert!(
        store
            .finish_oidc_link(&provider.id, link_at(T0), identity("cy-owner"), &cy.email)
            .await
            .expect("linked")
            .is_ok(),
        "owner identity linked before change"
    );
    moved(store, &cy.id, "cy-b@example.test", "cy-to-a", T0).await;
    moved(store, &cy.id, "cy-c@example.test", "cy-to-b", T0).await;
    assert!(
        store
            .finish_oidc_link(
                &provider.id,
                link_at(T0),
                identity("cy-attacker"),
                "cy-c@example.test",
            )
            .await
            .expect("linked")
            .is_ok(),
        "attacker identity linked after change"
    );
    // Reverting `cy-to-b` (which bumped to T0 + 1) still removes `cy-attacker`.
    let reverted = revert(store, "cy-to-b", T0 + 5).await.expect("reverted");
    assert_eq!(reverted.unlinked_identities, 1, "attacker identity removed");
    let left: Vec<String> = store
        .identities_of_user(&cy.id)
        .await
        .expect("read")
        .into_iter()
        .map(|tied| tied.subject)
        .collect();
    assert_eq!(left, ["cy-owner"]);
    test.teardown().await;
}

/// Even when an email change carries an earlier timestamp than an identity already tied
/// to the account (lock contention or clock step-back), `email_revert::insert` clamps
/// the revert link's `created_at` to at least that identity's `created_at` so pre-existing
/// identities stay and any identity linked afterward is still bumped past it and removed.
#[tokio::test]
async fn an_identity_linked_before_a_stale_change_timestamp_stays_and_a_later_one_goes() {
    let test = common::setup().await;
    let store = &test.store;
    account(store, "first").await;
    let issuer = "https://idp.example.test";
    let provider = store
        .put_oidc_provider(&oidc_provider(issuer))
        .await
        .expect("provider");
    let ada = account(store, "ada").await;
    let ada_session = session(store, &ada.id, "ada's session").await;
    let identity = |subject| Identity { issuer, subject };
    // Pre-existing identity stamped ahead of the change's `now` (`T0 + 5` > `T0`).
    store
        .add_identity_at(&ada.id, identity("owner-ahead"), &ada.email, T0 + 5)
        .await
        .expect("pre-existing identity");
    moved(store, &ada.id, "ada@new.example.test", "back", T0).await;
    let link = LinkingSession {
        session_id: &ada_session.id,
        token_digest: "ada's session",
        now: T0,
        idle_since: 0,
    };
    assert!(
        store
            .finish_oidc_link(
                &provider.id,
                link,
                identity("attacker-after"),
                "ada@new.example.test",
            )
            .await
            .expect("linked")
            .is_ok(),
        "identity linked after change"
    );

    let reverted = revert(store, "back", T0 + 10).await.expect("reverted");
    assert_eq!(reverted.unlinked_identities, 1, "only later identity goes");
    let left: Vec<String> = store
        .identities_of_user(&ada.id)
        .await
        .expect("read")
        .into_iter()
        .map(|tied| tied.subject)
        .collect();
    assert_eq!(left, ["owner-ahead"]);
    test.teardown().await;
}

/// An identity tied via `finish_oidc_carry_over` (`tie_unless_taken`) at the same
/// millisecond as an email change is bumped past the revert link and removed on revert,
/// while the former identity tied earlier stays.
#[tokio::test]
async fn a_same_millisecond_carried_over_identity_after_a_change_is_removed_on_revert() {
    let test = common::setup().await;
    let store = &test.store;
    account(store, "first").await;
    let old_issuer = "https://old-idp.example.test";
    let new_issuer = "https://new-idp.example.test";
    let actor = new_id();
    let fvoci_store::ProviderPut { provider: old, .. } = store
        .put_oidc_provider_as_read(&oidc_provider(old_issuer), None, &actor, Duration::ZERO)
        .await
        .expect("asked")
        .expect("stored");
    let fvoci_store::ProviderPut { provider: new, .. } = store
        .put_oidc_provider_as_read(
            &oidc_provider(new_issuer),
            Some(&old),
            &actor,
            Duration::from_hours(24),
        )
        .await
        .expect("asked")
        .expect("stored");
    let ada = account(store, "ada").await;
    let former = Identity {
        issuer: old_issuer,
        subject: "ada-old",
    };
    let now = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("after epoch")
            .as_millis(),
    )
    .expect("millis");
    store
        .add_identity_at(&ada.id, former, &ada.email, now - 10)
        .await
        .expect("former identity");
    moved(store, &ada.id, "ada@new.example.test", "back", now).await;

    let held = HeldSignIn {
        held_hash: "held-carry-digest".to_owned(),
        org_id: new.org_id.clone(),
        provider_id: new.id.clone(),
        revision: new.revision,
        issuer: new_issuer.to_owned(),
        subject: "ada-carried".to_owned(),
        email: "ada@new.example.test".to_owned(),
        email_unverified: false,
        name: "Ada".to_owned(),
        preferred_username: "ada".to_owned(),
        created_at: now,
        expires_at: now + 60_000,
    };
    assert!(
        store.hold_oidc_sign_in(&held).await.expect("hold"),
        "sign-in held"
    );
    let mut carry_session = Session {
        id: new_id(),
        created_at: now,
        expires_at: now + 60_000,
        ..Session::default()
    };
    let carry = CarryOver {
        provider_id: &new.id,
        held_hash: &held.held_hash,
        former,
        now,
    };
    assert!(
        store
            .finish_oidc_carry_over(carry, &mut carry_session, "carried-session")
            .await
            .expect("carry over")
            .is_ok(),
        "carried over at the same millisecond"
    );

    let reverted = revert(store, "back", now + 1).await.expect("reverted");
    assert_eq!(
        reverted.unlinked_identities, 1,
        "carried-over identity removed"
    );
    let left: Vec<String> = store
        .identities_of_user(&ada.id)
        .await
        .expect("read")
        .into_iter()
        .map(|tied| tied.subject)
        .collect();
    assert_eq!(left, ["ada-old"]);
    test.teardown().await;
}

/// A locked password proves nothing to the store either, for the requests that checked
/// it before it was locked: no session, new password, change of address, asking to be
/// deleted or removed identity comes of them. A reset link, or the operator, sets a
/// new one, which is taken.
#[tokio::test]
async fn a_locked_password_proves_nothing_until_a_new_one_is_set() {
    let test = common::setup().await;
    let store = &test.store;
    account(store, "first").await;
    let ada = account(store, "ada").await;
    moved(store, &ada.id, "ada@new.example.test", "back", T0).await;
    let late = session(store, &ada.id, "checked before").await;
    revert(store, "back", T0 + 1).await.expect("used");

    let another = Session {
        id: new_id(),
        user_id: ada.id.clone(),
        created_at: T0,
        expires_at: T0 * 1_000,
        ..Session::default()
    };
    assert!(
        !store
            .create_session(&another, "a login", HASH)
            .await
            .expect("asked"),
        "no session"
    );
    let confirmed = ConfirmedPassword {
        session_id: &late.id,
        user_id: &ada.id,
        seen_password_hash: HASH,
    };
    assert!(
        !store
            .confirm_session_password(confirmed, "confirmed", T0 + 2)
            .await
            .expect("asked")
    );
    assert!(
        store
            .change_user_password(&ada.id, HASH, "theirs", &late.id, "theirs")
            .await
            .expect("asked")
            .is_none(),
        "no new password"
    );
    assert!(
        !store
            .ask_email_change(&ada.id, HASH, "theirs@example.test", T0 + 2)
            .await
            .expect("asked"),
        "no change of address"
    );
    assert_eq!(
        store
            .request_account_deletion(&ada.id, HASH, T0 + 2, T0 + 3)
            .await
            .expect("asked"),
        DeletionAsking::Changed
    );
    let identity = Identity {
        issuer: "https://provider.example.test",
        subject: "ada",
    };
    assert_eq!(
        store
            .unlink_identity(&ada.id, HASH, identity)
            .await
            .expect("asked"),
        Unlinked::AccountChanged
    );

    // A reset link sets a new password, which is taken.
    assert!(
        store
            .create_password_reset(&ada.id, &ada.email, "reset", T0 + 3)
            .await
            .expect("stored")
    );
    assert!(
        store
            .consume_password_reset("reset", "new hash", T0 + 3)
            .await
            .expect("reset")
            .is_some()
    );
    assert!(!user(store, &ada.id).await.password_is_locked());
    assert!(
        store
            .create_session(&another, "a login", "new hash")
            .await
            .expect("asked")
    );
    // So does the operator.
    let bo = account(store, "bo").await;
    moved(store, &bo.id, "bo@new.example.test", "bo's back", T0).await;
    revert(store, "bo's back", T0 + 1).await.expect("used");
    assert!(user(store, &bo.id).await.password_is_locked());
    let _ = store
        .reset_user_password(&bo.username, "the operator's")
        .await
        .expect("set");
    assert!(!user(store, &bo.id).await.password_is_locked());
    test.teardown().await;
}

/// Of two uses of one link at once, one gives the address back.
#[tokio::test]
async fn of_two_uses_at_once_one_is_taken() {
    let test = common::setup().await;
    let store = &test.store;
    account(store, "first").await;
    let ada = account(store, "ada").await;
    moved(store, &ada.id, "ada@new.example.test", "back", T0).await;
    let (one, two) = tokio::join!(revert(store, "back", T0 + 1), revert(store, "back", T0 + 1));
    assert_eq!(
        [one.is_some(), two.is_some()]
            .iter()
            .filter(|used| **used)
            .count(),
        1,
        "{one:?} {two:?}"
    );
    test.teardown().await;
}
