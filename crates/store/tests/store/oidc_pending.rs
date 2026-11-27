//! The pending settings of a provider in the store: saved only while the organization
//! takes no password and the provider is as read; a trial that ends well makes them
//! the provider's, at a new revision, in its own transaction, and one of settings
//! that were saved anew, thrown away or replaced makes nothing. A trial holds the
//! provider before the account, as a token made through the provider does.

use std::time::Duration;

use fvoci_model::{AuditAction, LoginOrigin, OrgRole, Session, User, new_id};
use fvoci_store::{
    AuditPlace, DbType, Identity, Issuance, OidcRefusal, Paging, PendingProvider, PendingSettings,
    ProviderPut, Store, StoredProvider, Trial, Tried,
};

use crate::common;
use crate::lock_gate::{Gate, ended};
use crate::provider_tokens;

const ISSUER: &str = "https://issuer.example.test";

/// The password hash of the person of these tests.
const HASH: &str = "a password hash";

/// Whether the organization takes a password beside the provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Passwords {
    Taken,
    Refused,
}

fn provider(passwords: Passwords) -> StoredProvider {
    StoredProvider {
        id: String::new(),
        org_id: String::new(),
        issuer: ISSUER.to_owned(),
        client_id: "client".to_owned(),
        sealed_client_secret: "the sealed secret".to_owned(),
        auto_join: true,
        enabled: true,
        sso_required: passwords == Passwords::Refused,
        revision: 0,
        created_at: 0,
        updated_at: 0,
    }
}

fn pending(secret: &str) -> PendingSettings {
    PendingSettings {
        issuer: ISSUER.to_owned(),
        client_id: "client".to_owned(),
        sealed_client_secret: secret.to_owned(),
    }
}

/// Saves `secret` aside against the provider as `read`, as `user_id`.
async fn set_aside(
    store: &Store,
    secret: &str,
    read: &StoredProvider,
    user_id: &str,
) -> Option<PendingProvider> {
    store
        .put_oidc_pending_provider(&pending(secret), read, read, user_id)
        .await
        .expect("asked")
        .map(|(_, pending)| pending)
}

/// A person whose identity at the issuer is tied, and their identifier.
async fn person(store: &Store) -> String {
    let mut user = User {
        id: new_id(),
        username: "admin".to_owned(),
        email: "admin@example.com".to_owned(),
        password_hash: HASH.to_owned(),
        ..User::default()
    };
    store.create_user(&mut user).await.expect("stored");
    store
        .execute_unchecked(&format!(
            "UPDATE organization_members SET role = '{}' WHERE user_id = '{}'",
            OrgRole::Admin.as_str(),
            user.id
        ))
        .await
        .expect("an administrator of the organization");
    store
        .add_identity(&user.id, identity(), "")
        .await
        .expect("tied");
    user.id
}

fn identity() -> Identity<'static> {
    Identity {
        issuer: ISSUER,
        subject: "subject",
    }
}

/// Ends a trial of the pending settings saved at `pending_at` as `user_id`, with the
/// session under `digest`.
async fn trial(
    store: &Store,
    provider_id: &str,
    pending_at: i64,
    user_id: &str,
    digest: &str,
) -> Result<fvoci_store::Applied, OidcRefusal> {
    let mut session = Session {
        id: new_id(),
        created_at: 1,
        expires_at: i64::MAX,
        ..Session::default()
    };
    let account = Tried::Known {
        user_id,
        password_hash: HASH,
        identity: identity(),
    };
    store
        .finish_oidc_trial(
            Trial {
                provider_id,
                pending_at,
                trier_id: user_id,
                keep_former: Duration::ZERO,
            },
            account,
            &mut session,
            digest,
        )
        .await
        .expect("asked")
}

async fn actions(store: &Store) -> Vec<AuditAction> {
    store
        .audit_records(
            AuditPlace::Organization,
            Paging {
                page: 0,
                per_page: std::num::NonZeroU32::new(100).expect("a page size"),
            },
        )
        .await
        .expect("records")
        .into_iter()
        .map(|record| record.action)
        .collect()
}

/// Saved while the organization takes a password, nothing is set aside; saved while
/// it takes none, they are; a trial of the settings saved before the last is refused,
/// one of the last makes them the provider's at the next revision, and a session of
/// it, and nothing is left aside.
#[tokio::test]
async fn a_trial_of_the_last_settings_saved_applies_them() {
    let test = common::setup().await;
    let user = person(&test).await;
    let fvoci_store::ProviderPut { provider: open, .. } = test
        .put_oidc_provider_as_read(&provider(Passwords::Taken), None, &user, Duration::ZERO)
        .await
        .expect("stored")
        .expect("new");
    let not_enforced = set_aside(&test, "another", &open, &user).await;
    let fvoci_store::ProviderPut {
        provider: enforced, ..
    } = test
        .put_oidc_provider_as_read(
            &provider(Passwords::Refused),
            Some(&open),
            &user,
            Duration::ZERO,
        )
        .await
        .expect("stored")
        .expect("as read");
    let first = set_aside(&test, "first", &enforced, &user)
        .await
        .expect("set aside");
    let second = set_aside(&test, "second", &enforced, &user)
        .await
        .expect("set aside");
    let stale = trial(&test, &enforced.id, first.created_at, &user, "stale").await;
    let applied = trial(&test, &enforced.id, second.created_at, &user, "applied").await;
    let left = test.oidc_pending_provider().await.expect("read");
    let made = test.get_session("applied", 1, 0).await.expect("made");
    let stale_made = test.get_session("stale", 1, 0).await.is_ok();
    let recorded = actions(&test).await;
    test.teardown().await;

    assert_eq!(
        not_enforced, None,
        "nothing aside while a password is taken"
    );
    assert!(second.created_at > first.created_at);
    assert_eq!(stale.err(), Some(OidcRefusal::ProviderGone));
    assert!(!stale_made);
    let applied = applied.expect("applied");
    assert_eq!(applied.provider.sealed_client_secret, "second");
    assert_eq!(
        applied.provider.id, enforced.id,
        "the same issuer and client"
    );
    assert_eq!(applied.provider.revision, enforced.revision + 1);
    assert!(applied.provider.sso_required);
    assert_eq!(left, None);
    let login = made.login.expect("made through the provider");
    assert_eq!(login.revision, Some(applied.provider.revision));
    assert_eq!(
        recorded[..3],
        [
            AuditAction::OidcProviderPendingApplied,
            AuditAction::OidcProviderPendingSaved,
            AuditAction::OidcProviderPendingSaved,
        ]
    );
}

/// Pending settings go with a provider released and given another secret at once,
/// and with one thrown away: a trial of them makes nothing after either.
#[tokio::test]
async fn a_trial_of_settings_thrown_away_makes_nothing() {
    let test = common::setup().await;
    let user = person(&test).await;
    let fvoci_store::ProviderPut {
        provider: enforced, ..
    } = test
        .put_oidc_provider_as_read(&provider(Passwords::Refused), None, &user, Duration::ZERO)
        .await
        .expect("stored")
        .expect("new");
    let thrown = set_aside(&test, "one", &enforced, &user)
        .await
        .expect("set aside");
    let discarded = test
        .discard_oidc_pending_provider(&user)
        .await
        .expect("asked");
    let again = test
        .discard_oidc_pending_provider(&user)
        .await
        .expect("asked");
    let after_discard = trial(&test, &enforced.id, thrown.created_at, &user, "one").await;
    let replaced = set_aside(&test, "two", &enforced, &user)
        .await
        .expect("set aside");
    let mut released = provider(Passwords::Taken);
    released.sealed_client_secret = "three".to_owned();
    let _ = test
        .put_oidc_provider_as_read(&released, Some(&enforced), &user, Duration::ZERO)
        .await
        .expect("stored")
        .expect("as read");
    let after_change = trial(&test, &enforced.id, replaced.created_at, &user, "two").await;
    let left = test.oidc_pending_provider().await.expect("read");
    let recorded = actions(&test).await;
    test.teardown().await;

    assert!(discarded && !again, "thrown away once");
    assert_eq!(after_discard.err(), Some(OidcRefusal::ProviderGone));
    assert_eq!(after_change.err(), Some(OidcRefusal::ProviderGone));
    assert_eq!(left, None, "went with the change of the secret");
    assert!(recorded.contains(&AuditAction::OidcProviderPendingDiscarded));
    assert_eq!(
        recorded
            .iter()
            .filter(|action| **action == AuditAction::OidcProviderPendingDiscarded)
            .count(),
        2,
        "thrown away, and gone with the change"
    );
}

/// A trial ends only in the account of the administrator who started it, while they
/// administer the organization: one that ends in another account, or after they were
/// made a member, changes nothing.
#[tokio::test]
async fn a_trial_ends_only_in_its_administrators_account() {
    let test = common::setup().await;
    let user = person(&test).await;
    let fvoci_store::ProviderPut {
        provider: enforced, ..
    } = test
        .put_oidc_provider_as_read(&provider(Passwords::Refused), None, &user, Duration::ZERO)
        .await
        .expect("stored")
        .expect("new");
    let pending = set_aside(&test, "next", &enforced, &user)
        .await
        .expect("set aside");
    let started_by_another = new_id();
    let mut session = Session {
        id: new_id(),
        created_at: 1,
        expires_at: i64::MAX,
        ..Session::default()
    };
    let account = Tried::Known {
        user_id: &user,
        password_hash: HASH,
        identity: identity(),
    };
    let another = test
        .finish_oidc_trial(
            Trial {
                provider_id: &enforced.id,
                pending_at: pending.created_at,
                trier_id: &started_by_another,
                keep_former: Duration::ZERO,
            },
            account,
            &mut session,
            "another",
        )
        .await
        .expect("asked");
    test.execute_unchecked(&format!(
        "UPDATE organization_members SET role = 'member' WHERE user_id = '{user}'"
    ))
    .await
    .expect("made a member");
    let no_longer = trial(&test, &enforced.id, pending.created_at, &user, "member").await;
    let left = test.oidc_pending_provider().await.expect("read");
    let read = test.oidc_provider().await.expect("read");
    let made = [
        test.get_session("another", 1, 0).await.is_ok(),
        test.get_session("member", 1, 0).await.is_ok(),
    ];
    test.teardown().await;

    assert_eq!(another.err(), Some(OidcRefusal::NotTheTrier));
    assert_eq!(no_longer.err(), Some(OidcRefusal::NotTheTrier));
    assert_eq!(left, Some(pending), "still aside");
    assert_eq!(read, Some(enforced), "the provider as it was");
    assert_eq!(made, [false, false], "no session");
}

/// A trial does not end in an account that waits to be deleted, which changes nothing
/// of its organization; back in use, it does.
#[tokio::test]
async fn a_trial_does_not_end_in_an_account_that_waits_to_be_deleted() {
    let test = common::setup().await;
    let user = person(&test).await;
    let fvoci_store::ProviderPut {
        provider: enforced, ..
    } = test
        .put_oidc_provider_as_read(&provider(Passwords::Refused), None, &user, Duration::ZERO)
        .await
        .expect("stored")
        .expect("new");
    let pending = set_aside(&test, "next", &enforced, &user)
        .await
        .expect("set aside");
    test.execute_unchecked(&format!(
        "UPDATE users SET deleted_at = 5, delete_after = 6 WHERE id = '{user}'"
    ))
    .await
    .expect("the account waits");

    let waiting = trial(&test, &enforced.id, pending.created_at, &user, "waiting").await;
    let left = test.oidc_pending_provider().await.expect("read");
    let read = test.oidc_provider().await.expect("read");
    assert!(
        test.cancel_account_deletion(&user, 7)
            .await
            .expect("taken back")
    );
    let back = trial(&test, &enforced.id, pending.created_at, &user, "back").await;
    test.teardown().await;

    assert_eq!(waiting.err(), Some(OidcRefusal::NotTheTrier));
    assert_eq!(left, Some(pending), "still aside");
    assert_eq!(read, Some(enforced), "the provider as it was");
    assert!(back.is_ok(), "{back:?}");
}

/// A trial stopped once it holds its administrator's account, and a token made
/// meanwhile by a session of the same account made through the provider: the token
/// waits for the provider, which the trial holds before the account, and is refused
/// once the trial has made the settings the provider's. Held the other way round, each
/// would wait for the other until PostgreSQL ended one. PostgreSQL only: on SQLite
/// writers take turns.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_token_made_during_a_trial_waits_for_the_provider() {
    let test = common::setup().await;
    if common::db_type() != DbType::Postgres {
        test.teardown().await;
        return;
    }
    let user = person(&test).await;
    let workspace = common::workspace_of(&test, &[&user]).await;
    let ProviderPut {
        provider: enforced, ..
    } = test
        .put_oidc_provider_as_read(&provider(Passwords::Refused), None, &user, Duration::ZERO)
        .await
        .expect("stored")
        .expect("new");
    let pending = set_aside(&test, "next", &enforced, &user)
        .await
        .expect("set aside");
    let login = LoginOrigin {
        provider_id: enforced.id.clone(),
        org_id: enforced.org_id.clone(),
        issuer: enforced.issuer.clone(),
        subject: Some(identity().subject.to_owned()),
        revision: Some(enforced.revision),
    };
    let through = Session {
        id: new_id(),
        user_id: user.clone(),
        created_at: 1,
        last_used_at: 1,
        expires_at: i64::MAX,
        login: Some(login),
        ..Session::default()
    };
    assert!(
        test.create_session(&through, &new_id(), HASH)
            .await
            .expect("asked"),
        "the session is stored"
    );
    let session_id = through.id;

    let mut gate = Gate::before(&test, "DELETE", "oidc_pending_providers", "true").await;
    let second = gate.second_store(&test).await;
    let trying = {
        let store = test.store.clone();
        let (provider_id, user) = (enforced.id.clone(), user.clone());
        tokio::spawn(async move {
            trial(&store, &provider_id, pending.created_at, &user, "tried").await
        })
    };
    let holder = gate.stopped(&trying).await;
    let making = {
        let (user, workspace) = (user.clone(), workspace.clone());
        tokio::spawn(async move {
            provider_tokens::issue(&second, &user, &session_id, "during", &workspace).await
        })
    };
    gate.waits_for(holder, &making).await;
    gate.open().await;
    let tried = ended(trying).await;
    let made = ended(making).await;

    let applied = tried.expect("the trial applies the settings");
    assert_eq!(applied.provider.revision, enforced.revision + 1);
    assert_eq!(
        made,
        Issuance::ProviderChanged,
        "the session proves the provider as it was before the trial"
    );
    test.teardown().await;
}
