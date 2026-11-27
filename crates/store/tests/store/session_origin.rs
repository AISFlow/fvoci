//! Where a session made through a provider came from, and the endings that follow it:
//! the provider removed, its issuer changed, the identity unlinked.

use std::time::Duration;

use fvoci_model::{LoginOrigin, Session, User, new_id};
use fvoci_store::{Identity, OidcAccount, OidcRefusal, Removal, Store, StoredProvider, Unlinked};

use crate::common::{self};

/// The time the sessions are made at, in milliseconds.
const T0: i64 = 1_800_000_000_000;

/// The password hash of the tests' people: one they can unlink an identity with.
const HASH: &str = "a password hash";

/// Stores a person with a password and answers their identifier.
async fn person(store: &Store, username: &str) -> String {
    let mut user = User {
        id: new_id(),
        username: username.to_owned(),
        email: format!("{username}@example.com"),
        password_hash: HASH.to_owned(),
        ..User::default()
    };
    store.create_user(&mut user).await.expect("stored");
    user.id
}

fn provider(issuer: &str) -> StoredProvider {
    StoredProvider {
        id: String::new(),
        org_id: String::new(),
        issuer: issuer.to_owned(),
        client_id: "client".to_owned(),
        sealed_client_secret: String::new(),
        auto_join: true,
        enabled: true,
        sso_required: false,
        revision: 0,
        created_at: 0,
        updated_at: 0,
    }
}

/// Stores a session of the user, made through `login` or with a password, and answers
/// the digest it is found by.
async fn open(store: &Store, user_id: &str, login: Option<LoginOrigin>) -> String {
    let digest = new_id();
    let session = Session {
        id: new_id(),
        user_id: user_id.to_owned(),
        created_at: T0,
        last_used_at: T0,
        expires_at: T0 + 1000,
        user_agent: "a browser".to_owned(),
        login,
        ..Session::default()
    };
    assert!(
        store
            .create_session(&session, &digest, HASH)
            .await
            .expect("stored"),
        "the user's password is as read"
    );
    digest
}

async fn alive(store: &Store, digest: &str) -> bool {
    store.get_session(digest, T0, 0).await.is_ok()
}

fn origin(provider: &StoredProvider) -> LoginOrigin {
    LoginOrigin {
        provider_id: provider.id.clone(),
        org_id: provider.org_id.clone(),
        issuer: provider.issuer.clone(),
        subject: Some("subject".to_owned()),
        revision: Some(provider.revision),
    }
}

/// A session reads back with the origin it was stored with, and one of a password
/// with none.
#[tokio::test]
async fn a_session_keeps_its_origin() {
    let test = common::setup().await;
    let user = person(&test, "someone").await;
    let fvoci_store::ProviderPut {
        provider: stored, ..
    } = test
        .put_oidc_provider_as_read(
            &provider("https://one.example"),
            None,
            &user,
            Duration::ZERO,
        )
        .await
        .expect("stored")
        .expect("new");
    let through = open(&test, &user, Some(origin(&stored))).await;
    let password = open(&test, &user, None).await;
    let read = test.get_session(&through, T0, 0).await.expect("read");
    let read_password = test.get_session(&password, T0, 0).await.expect("read");
    test.teardown().await;
    assert_eq!(read.login, Some(origin(&stored)));
    assert_eq!(read_password.login, None);
}

/// Removing the provider ends the sessions made through it, of every person; a new
/// issuer ends those of the issuer before; a change that keeps the issuer, and
/// sessions of a password or of another organization's provider of the same issuer,
/// stay.
#[tokio::test]
async fn the_providers_sessions_end_with_its_removal_or_its_issuer() {
    let test = common::setup().await;
    let first = person(&test, "first").await;
    let second = person(&test, "second").await;
    let fvoci_store::ProviderPut { provider: one, .. } = test
        .put_oidc_provider_as_read(
            &provider("https://one.example"),
            None,
            &first,
            Duration::ZERO,
        )
        .await
        .expect("stored")
        .expect("new");
    let mut elsewhere = origin(&one);
    elsewhere.org_id = new_id();
    let firsts = open(&test, &first, Some(origin(&one))).await;
    let seconds = open(&test, &second, Some(origin(&one))).await;
    let password = open(&test, &first, None).await;
    let of_another_organization = open(&test, &first, Some(elsewhere)).await;

    let mut same_issuer = provider("https://one.example");
    same_issuer.auto_join = false;
    let fvoci_store::ProviderPut { provider: kept, .. } = test
        .put_oidc_provider_as_read(&same_issuer, Some(&one), &first, Duration::ZERO)
        .await
        .expect("stored")
        .expect("as read");
    let after_the_same_issuer = (alive(&test, &firsts).await, alive(&test, &seconds).await);
    let fvoci_store::ProviderPut { provider: two, .. } = test
        .put_oidc_provider_as_read(
            &provider("https://two.example"),
            Some(&kept),
            &first,
            Duration::ZERO,
        )
        .await
        .expect("stored")
        .expect("as read");
    let after_another_issuer = (alive(&test, &firsts).await, alive(&test, &seconds).await);
    let of_two = open(&test, &second, Some(origin(&two))).await;
    let removal = test.delete_oidc_provider(&first).await.expect("removed");
    let of_two_after = alive(&test, &of_two).await;
    let others = (
        alive(&test, &password).await,
        alive(&test, &of_another_organization).await,
    );
    test.teardown().await;

    assert_eq!(after_the_same_issuer, (true, true), "the issuer stayed");
    assert_eq!(after_another_issuer, (false, false), "everybody's ended");
    assert!(
        matches!(&removal, Removal::Removed { provider, .. } if provider.issuer == "https://two.example"),
        "{removal:?}"
    );
    assert!(!of_two_after, "ended with the removal");
    assert_eq!(
        others,
        (true, true),
        "a password's and another organization's"
    );
}

/// Unlinking an identity ends the user's sessions made through a provider of its
/// issuer, and only those.
#[tokio::test]
async fn unlinking_ends_the_users_sessions_of_that_issuer() {
    let test = common::setup().await;
    let user = person(&test, "linked").await;
    let other = person(&test, "other").await;
    let fvoci_store::ProviderPut {
        provider: stored, ..
    } = test
        .put_oidc_provider_as_read(
            &provider("https://one.example"),
            None,
            &user,
            Duration::ZERO,
        )
        .await
        .expect("stored")
        .expect("new");
    test.add_identity(
        &user,
        Identity {
            issuer: "https://one.example",
            subject: "subject",
        },
        "",
    )
    .await
    .expect("tied");
    let mut of_another_issuer = origin(&stored);
    of_another_issuer.issuer = "https://two.example".to_owned();
    let through = open(&test, &user, Some(origin(&stored))).await;
    let password = open(&test, &user, None).await;
    let another_issuer = open(&test, &user, Some(of_another_issuer)).await;
    let someone_elses = open(&test, &other, Some(origin(&stored))).await;
    let unlinked = test
        .unlink_identity(
            &user,
            HASH,
            Identity {
                issuer: "https://one.example",
                subject: "subject",
            },
        )
        .await
        .expect("asked");
    let alive_after = [
        alive(&test, &through).await,
        alive(&test, &password).await,
        alive(&test, &another_issuer).await,
        alive(&test, &someone_elses).await,
    ];
    test.teardown().await;
    assert!(matches!(unlinked, Unlinked::Removed(_)));
    assert_eq!(
        alive_after,
        [false, true, true, true],
        "only the user's session of that issuer ends"
    );
}

/// Unlinking one identity of an issuer ends the user's sessions made for it, and those
/// of the issuer that know no subject (an older server made them), but not those made
/// for another identity of the same issuer.
#[tokio::test]
async fn unlinking_ends_the_sessions_of_that_identity_only() {
    let test = common::setup().await;
    let user = person(&test, "linked twice").await;
    let fvoci_store::ProviderPut {
        provider: stored, ..
    } = test
        .put_oidc_provider_as_read(
            &provider("https://one.example"),
            None,
            &user,
            Duration::ZERO,
        )
        .await
        .expect("stored")
        .expect("new");
    for subject in ["first", "second"] {
        test.add_identity(
            &user,
            Identity {
                issuer: "https://one.example",
                subject,
            },
            "",
        )
        .await
        .expect("tied");
    }
    let made_for = |subject: Option<&str>| LoginOrigin {
        subject: subject.map(str::to_owned),
        ..origin(&stored)
    };
    let first = open(&test, &user, Some(made_for(Some("first")))).await;
    let second = open(&test, &user, Some(made_for(Some("second")))).await;
    let of_before = open(&test, &user, Some(made_for(None))).await;
    let unlinked = test
        .unlink_identity(
            &user,
            HASH,
            Identity {
                issuer: "https://one.example",
                subject: "first",
            },
        )
        .await
        .expect("asked");
    let alive_after = [
        alive(&test, &first).await,
        alive(&test, &second).await,
        alive(&test, &of_before).await,
    ];
    let read = test.get_session(&second, T0, 0).await.expect("read");
    test.teardown().await;
    assert!(matches!(unlinked, Unlinked::Removed(_)));
    assert_eq!(
        alive_after,
        [false, true, false],
        "the first identity's, and the one that knows no subject, end"
    );
    assert_eq!(read.login, Some(made_for(Some("second"))));
}

/// A sign-in through the provider that ends after the identity was unlinked makes no
/// session: the unlinking ended those there were, and the account no longer has the
/// identity the provider vouched for.
#[tokio::test]
async fn a_sign_in_that_ends_after_the_unlinking_is_refused() {
    let test = common::setup().await;
    let user = person(&test, "linked").await;
    let fvoci_store::ProviderPut {
        provider: stored, ..
    } = test
        .put_oidc_provider_as_read(
            &provider("https://one.example"),
            None,
            &user,
            Duration::ZERO,
        )
        .await
        .expect("stored")
        .expect("new");
    let identity = Identity {
        issuer: "https://one.example",
        subject: "subject",
    };
    test.add_identity(&user, identity, "").await.expect("tied");
    assert!(matches!(
        test.unlink_identity(&user, HASH, identity)
            .await
            .expect("asked"),
        Unlinked::Removed(_)
    ));
    let mut session = Session {
        id: new_id(),
        created_at: T0,
        expires_at: T0 + 1000,
        ..Session::default()
    };
    let finished = test
        .finish_oidc_sign_in(
            &stored.id,
            stored.revision,
            OidcAccount::Known {
                user_id: &user,
                password_hash: HASH,
                identity,
                proved_email: None,
            },
            &mut session,
            "digest",
        )
        .await
        .expect("asked");
    let made = alive(&test, "digest").await;
    test.teardown().await;
    assert_eq!(finished, Err(OidcRefusal::AccountChanged));
    assert!(!made, "no session");
}

/// The revision of a provider's sign-in: raised by a change of the issuer, the client,
/// the sealed secret or the switch, and by nothing else, the operator's release among
/// it; a sign-in through it keeps the revision it was made at, and one begun at another
/// revision makes no session.
#[tokio::test]
async fn a_change_of_what_signing_in_takes_raises_the_revision() {
    let test = common::setup().await;
    let user = person(&test, "admin").await;
    let fvoci_store::ProviderPut {
        provider: first, ..
    } = test
        .put_oidc_provider_as_read(
            &provider("https://one.example"),
            None,
            &user,
            Duration::ZERO,
        )
        .await
        .expect("stored")
        .expect("new");
    let mut next = first.clone();
    let mut revisions = vec![first.revision];
    let mut read = first;
    for change in [
        |p: &mut StoredProvider| p.auto_join = false,
        |p: &mut StoredProvider| p.sealed_client_secret = "another sealed secret".to_owned(),
        |p: &mut StoredProvider| p.sso_required = true,
        |p: &mut StoredProvider| p.sso_required = false,
        |p: &mut StoredProvider| p.enabled = false,
        |p: &mut StoredProvider| p.enabled = true,
        |p: &mut StoredProvider| p.client_id = "another client".to_owned(),
        |p: &mut StoredProvider| p.issuer = "https://two.example".to_owned(),
    ] {
        change(&mut next);
        read = test
            .put_oidc_provider_as_read(&next, Some(&read), &user, Duration::ZERO)
            .await
            .expect("stored")
            .expect("as read")
            .provider;
        revisions.push(read.revision);
    }
    next.sso_required = true;
    let fvoci_store::ProviderPut {
        provider: enforced, ..
    } = test
        .put_oidc_provider_as_read(&next, Some(&read), &user, Duration::ZERO)
        .await
        .expect("stored")
        .expect("as read");
    let released = test
        .release_sso_by_operator()
        .await
        .expect("asked")
        .expect("it was enforced");
    revisions.extend([enforced.revision, released.revision]);
    read = released;
    let identity = Identity {
        issuer: "https://two.example",
        subject: "subject",
    };
    test.add_identity(&user, identity, "").await.expect("tied");
    let sign_in = async |revision: i64, digest: &str| {
        let mut session = Session {
            id: new_id(),
            created_at: T0,
            expires_at: T0 + 1000,
            ..Session::default()
        };
        let account = OidcAccount::Known {
            user_id: &user,
            password_hash: HASH,
            identity,
            proved_email: None,
        };
        test.finish_oidc_sign_in(&read.id, revision, account, &mut session, digest)
            .await
            .expect("asked")
    };
    // Begun at the revision before, it ends at none: the secret it was made with is
    // not the one stored.
    let stale = sign_in(read.revision - 1, "stale").await;
    let finished = sign_in(read.revision, "digest").await;
    let stale_made = alive(&test, "stale").await;
    let made = test.get_session("digest", T0, 0).await.expect("made");
    test.teardown().await;

    assert_eq!(revisions, [1, 1, 2, 2, 2, 3, 4, 5, 6, 6, 6]);
    assert_eq!(stale, Err(OidcRefusal::ProviderGone));
    assert!(!stale_made, "no session of the revision before");
    assert!(finished.is_ok(), "{finished:?}");
    assert_eq!(
        made.login,
        Some(LoginOrigin {
            subject: Some("subject".to_owned()),
            ..origin(&read)
        })
    );
    assert_eq!(made.login.and_then(|login| login.revision), Some(6));
}
