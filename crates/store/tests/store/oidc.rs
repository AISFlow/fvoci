//! Signing in through a provider: two first sign-ins at once of an account already
//! known to the issuer.

use std::time::Duration;

use fvoci_model::{Session, new_id};
use fvoci_store::{
    DbType, Identity, LinkRefusal, LinkingSession, OidcAccount, OidcRefusal, Removal, SessionTaken,
    Store, StoredProvider, Tenant, Unlinked,
};
use sqlx::Connection as _;

use crate::common;

/// The provider of the sign-ins of these tests, and who it vouches for.
const ISSUER: &str = "https://idp.example.com";
const SUBJECT: &str = "existing-identity";

/// One sign-in through the provider ending in the account: what `App::let_in` stores.
fn join_task(
    store: Store,
    provider_id: String,
    user_id: String,
    password_hash: String,
) -> tokio::task::JoinHandle<Result<Result<bool, OidcRefusal>, fvoci_store::StoreError>> {
    tokio::spawn(async move {
        let mut session = Session {
            id: new_id(),
            created_at: 1,
            expires_at: i64::MAX,
            ..Session::default()
        };
        let account = OidcAccount::Known {
            user_id: &user_id,
            password_hash: &password_hash,
            identity: Identity {
                issuer: ISSUER,
                subject: SUBJECT,
            },
            proved_email: None,
        };
        store
            .finish_oidc_sign_in(&provider_id, 1, account, &mut session, &new_id())
            .await
    })
}

/// An account the provider's identity names, its password hash, and the provider of
/// the organization: the identifier of each.
async fn known_account_and_provider(test: &Store) -> (String, String, String) {
    let account = common::user(&test.scoped(Tenant::Unset), "existing-identity").await;
    test.system()
        .execute_unchecked(&format!(
            "UPDATE users SET email_verified_at = created_at WHERE id = '{account}'"
        ))
        .await
        .expect("fixture account has verified address");
    let password_hash = test
        .get_user_by_id(&account)
        .await
        .expect("the account")
        .password_hash;
    test.add_identity(
        &account,
        Identity {
            issuer: ISSUER,
            subject: SUBJECT,
        },
        "",
    )
    .await
    .expect("the account's identity");
    let provider = test
        .put_oidc_provider(&StoredProvider {
            id: String::new(),
            org_id: String::new(),
            issuer: ISSUER.to_owned(),
            client_id: "fvoci".to_owned(),
            sealed_client_secret: String::new(),
            auto_join: true,
            enabled: true,
            created_at: 0,
            updated_at: 0,
            sso_required: false,
            revision: 0,
        })
        .await
        .expect("a provider")
        .id;
    (account, password_hash, provider)
}

/// The end of a sign-in (`Store::finish_oidc_sign_in`) when an identity names an
/// existing account that is not yet in the organization. Two returns may both get that
/// answer. A PostgreSQL SHARE table lock lets their membership reads finish while both
/// inserts wait, making the order independent of sleeps or the speed of the machine.
#[tokio::test]
async fn concurrent_oidc_joins_of_one_account_both_succeed() {
    let test = common::setup().await;
    let (account, password_hash, provider) = known_account_and_provider(&test).await;
    let join = || {
        join_task(
            test.store.clone(),
            provider.clone(),
            account.clone(),
            password_hash.clone(),
        )
    };
    assert!(
        !test
            .is_organization_member(&account)
            .await
            .expect("membership read"),
        "the account has not joined this organization",
    );

    let tasks = if common::db_type() == DbType::Postgres {
        let mut connection = common::connect_past_the_store(&test.database_connection()).await;
        let mut held = connection.begin().await.expect("a test transaction");
        sqlx::query("LOCK TABLE organization_members IN SHARE MODE")
            .execute(&mut *held)
            .await
            .expect("membership inserts wait");
        let tasks = (join(), join());
        let waiting = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let count: i64 = sqlx::query_scalar(
                    "SELECT count(*) FROM pg_locks WHERE relation = 'organization_members'::regclass \
                     AND mode = 'RowExclusiveLock' AND NOT granted",
                ).fetch_one(&mut *held).await.expect("the waiting inserts are observed");
                if count >= 2 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await;
        held.commit()
            .await
            .expect("both membership inserts may proceed");
        connection
            .close()
            .await
            .expect("the test connection closes");
        assert!(
            waiting.is_ok(),
            "both callbacks reached their first membership insert"
        );
        tasks
    } else {
        (join(), join())
    };
    let (first, second) = tokio::join!(tasks.0, tasks.1);
    let first = first.expect("the first callback task completes");
    let second = second.expect("the second callback task completes");
    // In the organization: the account is a member of its own space besides.
    let rows = test
        .system()
        .texts_unchecked(&format!(
            "SELECT 'in' FROM organization_members WHERE org_id = '{}'",
            test.org_id
        ))
        .await
        .expect("memberships counted")
        .len();
    let sessions = test.count_rows("sessions").await.expect("sessions counted");
    test.teardown().await;

    assert!(
        matches!(first, Ok(Ok(_))) && matches!(second, Ok(Ok(_))),
        "both valid callbacks must succeed: first={first:?}, second={second:?}"
    );
    assert_ne!(
        first.expect("the first result"),
        second.expect("the second result"),
        "exactly one call newly joins the account"
    );
    assert_eq!(rows, 1, "one organization membership");
    assert_eq!(sessions, 2, "a session for each sign-in");
}

/// A change of the provider decided from what was read is stored only if the provider
/// is still as it was read: another change of the same provider meanwhile (its secret,
/// within the same millisecond as often as not) makes it read again. Without one, it is
/// stored.
#[tokio::test]
async fn a_change_of_the_provider_made_from_an_old_reading_is_not_stored() {
    let test = common::setup().await;
    let provider = |client_id: &str, secret: &str| StoredProvider {
        id: String::new(),
        org_id: String::new(),
        issuer: "https://idp.example.com".to_owned(),
        client_id: client_id.to_owned(),
        sealed_client_secret: secret.to_owned(),
        auto_join: true,
        enabled: true,
        created_at: 0,
        updated_at: 0,
        sso_required: false,
        revision: 0,
    };
    let fvoci_store::ProviderPut {
        provider: none_read,
        ..
    } = test
        .put_oidc_provider_as_read(&provider("first", ""), None, &new_id(), Duration::ZERO)
        .await
        .expect("stored")
        .expect("none was there, as read");
    // Read by one request; changed by another meanwhile, within the same millisecond
    // as often as not: the time of the change is later all the same.
    let later = test
        .put_oidc_provider(&provider("first", "a secret"))
        .await
        .expect("changed");
    assert_eq!(later.id, none_read.id, "the same provider");
    assert!(
        later.updated_at > none_read.updated_at,
        "{} after {}",
        later.updated_at,
        none_read.updated_at
    );
    let stale = test
        .put_oidc_provider_as_read(
            &provider("first", ""),
            Some(&none_read),
            &new_id(),
            Duration::ZERO,
        )
        .await
        .expect("asked");
    let stored = test.oidc_provider().await.expect("read").expect("stored");
    let fresh = test
        .put_oidc_provider_as_read(
            &provider("third", ""),
            Some(&later),
            &new_id(),
            Duration::ZERO,
        )
        .await
        .expect("asked");
    let none_stale = test
        .put_oidc_provider_as_read(&provider("fourth", ""), None, &new_id(), Duration::ZERO)
        .await
        .expect("asked");
    test.teardown().await;
    assert!(stale.is_none(), "not stored from an old reading: {stale:?}");
    assert_eq!(
        (
            stored.client_id.as_str(),
            stored.sealed_client_secret.as_str()
        ),
        ("first", "a secret"),
        "the change meanwhile stays"
    );
    assert!(fresh.is_some(), "stored from the reading of what is there");
    assert!(none_stale.is_none(), "one was there, not none");
}

/// A provider removed after a change of it was read does not come back: the change,
/// decided from the reading before the removal, is not stored; nor when the provider was
/// set again meanwhile, at whatever time, as it is another. (That a removal and a
/// change are one after the other, not interleaved, is the lock's; this test runs them
/// one after the other.)
#[tokio::test]
async fn a_removed_provider_is_not_brought_back_by_a_change_read_before() {
    let test = common::setup().await;
    let provider = StoredProvider {
        id: String::new(),
        org_id: String::new(),
        issuer: "https://idp.example.com".to_owned(),
        client_id: "client".to_owned(),
        sealed_client_secret: "sealed".to_owned(),
        auto_join: true,
        enabled: false,
        created_at: 0,
        updated_at: 0,
        sso_required: false,
        revision: 0,
    };
    let read = test.put_oidc_provider(&provider).await.expect("stored");
    assert!(matches!(
        test.delete_oidc_provider(&new_id()).await.expect("removed"),
        Removal::Removed { ref provider, .. } if provider == &read
    ));
    let change = test
        .put_oidc_provider_as_read(&provider, Some(&read), &new_id(), Duration::ZERO)
        .await
        .expect("asked");
    let after = test.oidc_provider().await.expect("read");
    // Set again, by another request; the reading of the one removed has, at worst,
    // the time of the new one (the same millisecond, or the clock went back).
    let again = test.put_oidc_provider(&provider).await.expect("set again");
    let of_the_removed = StoredProvider {
        updated_at: again.updated_at,
        ..read.clone()
    };
    let change_again = test
        .put_oidc_provider_as_read(&provider, Some(&of_the_removed), &new_id(), Duration::ZERO)
        .await
        .expect("asked");
    test.teardown().await;
    assert!(change.is_none(), "{change:?}");
    assert!(after.is_none(), "{after:?}");
    assert_ne!(again.id, read.id, "another provider");
    assert!(change_again.is_none(), "{change_again:?}");
}

fn provider_that(sso_required: Enforcement) -> StoredProvider {
    StoredProvider {
        id: String::new(),
        org_id: String::new(),
        issuer: "https://idp.example.com".to_owned(),
        client_id: "client".to_owned(),
        sealed_client_secret: String::new(),
        auto_join: true,
        enabled: true,
        created_at: 0,
        updated_at: 0,
        sso_required: sso_required == Enforcement::On,
        revision: 0,
    }
}

/// Whether the organization takes only sessions made through its provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Enforcement {
    On,
    Off,
}

/// The provider an organization takes no other session than stays: its removal is
/// refused, until the organization is released, which leaves the provider as it was
/// but for that, under a later time.
#[tokio::test]
async fn an_enforced_provider_is_not_removed_until_it_is_released() {
    let test = common::setup().await;
    assert_eq!(
        test.delete_oidc_provider(&new_id()).await.expect("asked"),
        Removal::NoneThere
    );
    assert_eq!(test.release_sso_by_operator().await.expect("asked"), None);
    let enforced = test
        .put_oidc_provider(&provider_that(Enforcement::On))
        .await
        .expect("stored");
    assert!(enforced.sso_required);

    let refused = test.delete_oidc_provider(&new_id()).await.expect("asked");
    let still = test.oidc_provider().await.expect("read");
    let released = test.release_sso_by_operator().await.expect("released");
    let again = test.release_sso_by_operator().await.expect("asked");
    let removed = test.delete_oidc_provider(&new_id()).await.expect("asked");
    test.teardown().await;

    assert_eq!(refused, Removal::Enforced);
    assert_eq!(still.as_ref(), Some(&enforced), "as it was");
    let released = released.expect("it was enforced");
    assert!(
        matches!(&removed, Removal::Removed { provider, .. } if provider == &released),
        "the provider as released"
    );
    assert!(!released.sso_required);
    assert!(
        released.updated_at > enforced.updated_at,
        "another state of it"
    );
    assert_eq!(
        StoredProvider {
            sso_required: true,
            updated_at: enforced.updated_at,
            ..released
        },
        enforced,
        "nothing else of it changed"
    );
    assert_eq!(again, None, "nothing left to release");
}

/// What a session must be made through to open an organization is answered to who has
/// a place in it, by the organization or by a workspace of it, and to nobody else; and
/// only while the organization takes no other session.
#[tokio::test]
async fn the_required_provider_is_told_to_who_has_a_place_in_the_organization() {
    let test = common::setup().await;
    let nobodys = test.scoped(Tenant::Unset);
    let member = common::user(&test, "member").await;
    let guest = common::user(&nobodys, "guest").await;
    let stranger = common::user(&nobodys, "stranger").await;
    test.add_workspace_member(
        &test.workspace_id,
        &guest,
        fvoci_model::WorkspaceRole::Guest,
    )
    .await
    .expect("a guest of the workspace, no member of the organization");
    let required = async |user_id: &str| {
        test.required_provider_for(user_id, &test.org_id)
            .await
            .expect("asked")
    };
    let entered = async |user_id: &str| {
        test.resolve_workspace(user_id, &test.workspace_id)
            .await
            .expect("asked")
            .map(|access| match access.takes {
                SessionTaken::Any => None,
                SessionTaken::Through(provider) => Some(provider),
                SessionTaken::WithPassword => panic!("an organization takes no password alone"),
            })
    };
    let listed = async |user_id: &str, through: Option<&str>| {
        let making = through.map_or_else(fvoci_store::Making::with_password, |provider| {
            fvoci_store::Making::through(provider)
        });
        test.workspaces_of_user(user_id, making)
            .await
            .expect("listed")
            .iter()
            .map(|workspace| workspace.sso_required)
            .collect::<Vec<_>>()
    };

    let open = test
        .put_oidc_provider(&provider_that(Enforcement::Off))
        .await
        .expect("stored");
    let while_open = (
        required(&member).await,
        entered(&member).await,
        listed(&member, None).await,
    );
    let provider = test
        .put_oidc_provider(&provider_that(Enforcement::On))
        .await
        .expect("stored");
    let told = [
        required(&member).await,
        required(&guest).await,
        required(&stranger).await,
    ];
    let at_the_door = [
        entered(&member).await,
        entered(&guest).await,
        entered(&stranger).await,
    ];
    let lists = [
        listed(&member, None).await,
        listed(&member, Some(&provider.id)).await,
        listed(&member, Some(&new_id())).await,
        listed(&guest, None).await,
        listed(&stranger, None).await,
    ];
    let elsewhere = test
        .required_provider_for(&member, &new_id())
        .await
        .expect("asked");
    test.teardown().await;

    assert_eq!(open.id, provider.id, "the same provider");
    // The user's own space is listed last, and asks for no provider ever.
    assert_eq!(while_open, (None, Some(None), vec![false, false]));
    let ours = Some(provider.id.clone());
    assert_eq!(told, [ours.clone(), ours.clone(), None]);
    assert_eq!(at_the_door, [Some(ours.clone()), Some(ours), None]);
    assert_eq!(
        lists,
        [
            vec![true, false],
            vec![false, false],
            vec![true, false],
            vec![true, false],
            vec![false]
        ]
    );
    assert_eq!(elsewhere, None, "an organization that is not there");
}

/// Taking no other session is switched on, from a reading of the provider, while the
/// provider is removed: whichever comes first, the organization is never left taking
/// only the sessions of a provider that is gone. Either the removal was refused and the
/// provider is there, enforcing; or the provider is gone and the change, read before,
/// stored nothing.
#[tokio::test]
async fn enforcing_and_removing_at_once_leave_no_organization_without_its_provider() {
    let test = common::setup().await;
    for round in 0..12 {
        let read = test
            .put_oidc_provider(&provider_that(Enforcement::Off))
            .await
            .expect("stored");
        let enforcing = {
            let store = test.store.clone();
            let read = read.clone();
            tokio::spawn(async move {
                store
                    .put_oidc_provider_as_read(
                        &provider_that(Enforcement::On),
                        Some(&read),
                        &new_id(),
                        Duration::ZERO,
                    )
                    .await
            })
        };
        let removing = {
            let store = test.store.clone();
            tokio::spawn(async move { store.delete_oidc_provider(&new_id()).await })
        };
        let enforced = enforcing.await.expect("joined").expect("asked");
        let removal = removing.await.expect("joined").expect("asked");
        let left = test.oidc_provider().await.expect("read");
        match removal {
            Removal::Enforced => {
                assert!(enforced.is_some(), "round {round}: switched on first");
                assert!(
                    left.is_some_and(|left| left.sso_required),
                    "round {round}: the provider is there, and holds"
                );
            }
            Removal::Removed { .. } => {
                assert_eq!(enforced, None, "round {round}: nothing to switch it on for");
                assert_eq!(left, None, "round {round}: removed");
            }
            Removal::NoneThere => panic!("round {round}: there was a provider"),
        }
        // For the next round.
        test.release_sso_by_operator().await.expect("asked");
        test.delete_oidc_provider(&new_id()).await.expect("asked");
    }
    test.teardown().await;
}

/// A session of `user_id` under the token digest `digest`, made at 1,000 and ending at
/// 2,000.
async fn session_of(test: &common::TestStore, user_id: &str, digest: &str) -> Session {
    let session = Session {
        id: new_id(),
        user_id: user_id.to_owned(),
        created_at: 1_000,
        expires_at: 2_000,
        ..Session::default()
    };
    let password_hash = test
        .get_user_by_id(user_id)
        .await
        .expect("the account")
        .password_hash;
    assert!(
        test.create_session(&session, digest, &password_hash)
            .await
            .expect("stored"),
        "the session is made"
    );
    session
}

/// A sign-in to link ties the identity to the account of the session it was started
/// from, if that session came back under its token and is still good, and the provider
/// is still the one, switched on. An identity that is another account's is refused,
/// and one that is the account's already is left as it is.
#[tokio::test]
async fn a_link_ties_an_identity_to_the_account_of_a_session_that_is_still_good() {
    let test = common::setup().await;
    let user = common::user(&test, "linker").await;
    let other = common::user(&test, "other").await;
    let provider = test
        .put_oidc_provider(&provider_that(Enforcement::Off))
        .await
        .expect("stored");
    let session = session_of(&test, &user, "digest").await;
    let identity = Identity {
        issuer: "https://idp.example.com",
        subject: "linker-there",
    };
    let link = |token_digest: &'static str, now: i64, idle_since: i64| LinkingSession {
        session_id: &session.id,
        token_digest,
        now,
        idle_since,
    };
    let attempt = async |provider_id: &str, link: LinkingSession<'_>, identity: Identity<'_>| {
        test.finish_oidc_link(provider_id, link, identity, "Linker@Example.com")
            .await
            .expect("asked")
    };

    let refused = [
        attempt(&provider.id, link("another digest", 1_500, 0), identity).await,
        attempt(&provider.id, link("digest", 2_000, 0), identity).await,
        attempt(&provider.id, link("digest", 1_500, 1_000), identity).await,
        attempt(&new_id(), link("digest", 1_500, 0), identity).await,
    ];
    let before = test.identities_of_user(&user).await.expect("read");
    let first = attempt(&provider.id, link("digest", 1_500, 0), identity).await;
    let again = attempt(&provider.id, link("digest", 1_500, 0), identity).await;
    test.add_identity(
        &other,
        Identity {
            issuer: "https://idp.example.com",
            subject: "other-there",
        },
        "",
    )
    .await
    .expect("the other account's identity");
    let of_another = attempt(
        &provider.id,
        link("digest", 1_500, 0),
        Identity {
            issuer: "https://idp.example.com",
            subject: "other-there",
        },
    )
    .await;
    let after = test.identities_of_user(&user).await.expect("read");
    test.teardown().await;

    assert_eq!(
        refused,
        [
            Err(LinkRefusal::SessionEnded),
            Err(LinkRefusal::SessionEnded),
            Err(LinkRefusal::SessionEnded),
            Err(LinkRefusal::ProviderGone),
        ],
        "another token, past its end, idle too long, another provider"
    );
    assert_eq!(before, [], "nothing tied by what was refused");
    assert_eq!(
        first.map(|linked| (linked.user_id, linked.newly)),
        Ok((user.clone(), true))
    );
    assert_eq!(
        again.map(|linked| (linked.user_id, linked.newly)),
        Ok((user, false))
    );
    assert_eq!(of_another, Err(LinkRefusal::IdentityTaken));
    assert_eq!(
        after
            .iter()
            .map(|identity| (identity.subject.as_str(), identity.email.as_str()))
            .collect::<Vec<_>>(),
        [("linker-there", "Linker@Example.com")]
    );
}

/// An identity is tied to the account whose own address the provider vouches for, in
/// whatever letters, and to no other: not by another address, another account's, or
/// none. One that is the account's already is that still, whatever address comes.
#[tokio::test]
async fn a_link_ties_only_the_identity_of_the_accounts_own_address() {
    let test = common::setup().await;
    let user = common::user(&test, "linker").await;
    common::user(&test, "other").await;
    let provider = test
        .put_oidc_provider(&provider_that(Enforcement::Off))
        .await
        .expect("stored");
    let session = session_of(&test, &user, "digest").await;
    let attempt = async |address: &str| {
        let link = LinkingSession {
            session_id: &session.id,
            token_digest: "digest",
            now: 1_500,
            idle_since: 0,
        };
        let identity = Identity {
            issuer: "https://idp.example.com",
            subject: "linker-there",
        };
        test.finish_oidc_link(&provider.id, link, identity, address)
            .await
            .expect("asked")
            .map(|linked| linked.newly)
    };

    for address in ["somebody@elsewhere.example", "other@example.com", ""] {
        assert_eq!(
            attempt(address).await,
            Err(LinkRefusal::AnotherAddress),
            "{address:?}"
        );
    }
    assert_eq!(test.identities_of_user(&user).await.expect("read"), []);
    assert_eq!(attempt("LINKER@example.com").await, Ok(true));
    assert_eq!(
        attempt("somebody@elsewhere.example").await,
        Ok(false),
        "the account's already"
    );
    assert_eq!(test.identities_of_user(&user).await.expect("read").len(), 1);
    test.teardown().await;
}

/// An account without a password keeps its last identity, also when two of them are
/// removed at once: one removal sees what the other left. An account with a password
/// loses any, and an identity that is not the account's is not removed.
#[tokio::test]
async fn the_last_identity_of_an_account_without_a_password_is_not_removed() {
    let test = common::setup().await;
    let user = common::user(&test, "without-password").await;
    let other = common::user(&test, "other").await;
    let first = Identity {
        issuer: "https://one.example.com",
        subject: "there",
    };
    let second = Identity {
        issuer: "https://two.example.com",
        subject: "there",
    };
    let mut rounds = Vec::new();
    for _ in 0..8 {
        for identity in [first, second] {
            // Tied anew, or still there from the round before.
            match test.add_identity(&user, identity, "").await {
                Ok(()) | Err(fvoci_store::StoreError::Conflict(_)) => {}
                Err(error) => panic!("an identity is tied: {error}"),
            }
        }
        let removing = |identity: Identity<'static>| {
            let store = test.store.clone();
            let user = user.clone();
            tokio::spawn(async move { store.unlink_identity(&user, "", identity).await })
        };
        let (one, two) = (removing(first), removing(second));
        let mut outcomes = [
            one.await.expect("joined").expect("asked"),
            two.await.expect("joined").expect("asked"),
        ];
        outcomes.sort_by_key(|outcome| !matches!(outcome, Unlinked::Removed(_)));
        let left = test.identities_of_user(&user).await.expect("read").len();
        rounds.push((outcomes, left));
    }
    let not_theirs = test
        .unlink_identity(&other, "", first)
        .await
        .expect("asked");
    let changed = test
        .unlink_identity(&user, "another hash", first)
        .await
        .expect("asked");
    test.teardown().await;

    for (round, (outcomes, left)) in rounds.into_iter().enumerate() {
        assert!(
            matches!(outcomes, [Unlinked::Removed(_), Unlinked::LastWayIn]) && left == 1,
            "round {round}: one goes and the last stays"
        );
    }
    assert_eq!(not_theirs, Unlinked::NotTied);
    assert_eq!(changed, Unlinked::AccountChanged);
}

/// Two accounts link one identity at once: it is tied to one of them, the other is
/// told it is taken, and neither transaction is left half done. A provider that is
/// switched off ties nothing.
#[tokio::test]
async fn one_identity_linked_by_two_accounts_at_once_is_tied_to_one() {
    let test = common::setup().await;
    let provider = test
        .put_oidc_provider(&provider_that(Enforcement::Off))
        .await
        .expect("stored");
    let mut rounds = Vec::new();
    for round in 0..8 {
        let subject = format!("contested-{round}");
        let mut linking = Vec::new();
        let mut users = Vec::new();
        for name in ["first", "second"] {
            let user = common::user(&test, &format!("{name}-{round}")).await;
            let digest = format!("digest-{name}-{round}");
            let session = session_of(&test, &user, &digest).await;
            let (store, provider_id, subject) =
                (test.store.clone(), provider.id.clone(), subject.clone());
            // Each as the provider would know it: by the address of its own account.
            let address = format!("{name}-{round}@example.com");
            users.push(user);
            linking.push(tokio::spawn(async move {
                let link = LinkingSession {
                    session_id: &session.id,
                    token_digest: &digest,
                    now: 1_500,
                    idle_since: 0,
                };
                let identity = Identity {
                    issuer: "https://idp.example.com",
                    subject: &subject,
                };
                store
                    .finish_oidc_link(&provider_id, link, identity, &address)
                    .await
            }));
        }
        let mut outcomes = Vec::new();
        for link in linking {
            outcomes.push(link.await.expect("joined").expect("asked"));
        }
        let mut tied = Vec::new();
        for user in &users {
            tied.push(test.identities_of_user(user).await.expect("read").len());
        }
        rounds.push((outcomes, tied, users));
    }
    let mut off = provider_that(Enforcement::Off);
    off.enabled = false;
    test.put_oidc_provider(&off).await.expect("switched off");
    let user = common::user(&test, "late").await;
    let session = session_of(&test, &user, "late digest").await;
    let switched_off = test
        .finish_oidc_link(
            &provider.id,
            LinkingSession {
                session_id: &session.id,
                token_digest: "late digest",
                now: 1_500,
                idle_since: 0,
            },
            Identity {
                issuer: "https://idp.example.com",
                subject: "late",
            },
            "late@example.com",
        )
        .await
        .expect("asked");
    test.teardown().await;

    for (round, (outcomes, tied, users)) in rounds.into_iter().enumerate() {
        let winners: Vec<&String> = outcomes
            .iter()
            .filter_map(|outcome| outcome.as_ref().ok())
            .map(|linked| &linked.user_id)
            .collect();
        assert_eq!(
            winners.len(),
            1,
            "round {round}: one of the two: {outcomes:?}"
        );
        assert!(
            outcomes.contains(&Err(LinkRefusal::IdentityTaken)),
            "round {round}: the other is told it is taken: {outcomes:?}"
        );
        let expected: Vec<usize> = users
            .iter()
            .map(|user| usize::from(winners[0] == user))
            .collect();
        assert_eq!(tied, expected, "round {round}: tied to the one alone");
    }
    assert_eq!(switched_off, Err(LinkRefusal::ProviderGone));
}

/// A first sign-in at an address that is an account's own by the time it is stored (a
/// registration at it was confirmed while the person was at the provider) makes no
/// second account; a registration that no link of the organization made is not the
/// provider's to remove.
#[tokio::test]
async fn a_first_sign_in_makes_no_second_account_and_removes_no_registration_of_its_own() {
    let test = common::setup().await;
    let (_, _, provider) = known_account_and_provider(&test).await;
    let newcomer = |username: &str| fvoci_model::User {
        id: new_id(),
        username: username.to_owned(),
        ..fvoci_model::User::default()
    };
    let first_sign_in = async |user: &mut fvoci_model::User, subject: &str, email: &str| {
        let mut session = Session {
            id: new_id(),
            created_at: 1,
            expires_at: i64::MAX,
            ..Session::default()
        };
        let account = OidcAccount::New {
            user,
            identity: Identity {
                issuer: ISSUER,
                subject,
            },
            provider_email: email,
        };
        test.finish_oidc_sign_in(&provider, 1, account, &mut session, &new_id())
            .await
    };

    // The account of `known_account_and_provider` has this address as its own.
    let refused = first_sign_in(
        &mut newcomer("newcomer"),
        "a-newcomer",
        "Existing-Identity@example.com",
    )
    .await;
    assert!(
        matches!(
            refused,
            Err(fvoci_store::StoreError::Conflict(fvoci_store::Taken::Email))
        ),
        "the address is an account's: {refused:?}"
    );
    assert!(
        test.get_user_by_username("newcomer")
            .await
            .is_err_and(|error| error.is_not_found()),
        "no account was made"
    );

    // A registration of its own waits at another address: no link of this
    // organization made it, and the provider's word does not reach it. The newcomer
    // is let in beside it, and it holds its name.
    let now = 1_000;
    let waiting = fvoci_store::Signup {
        id: new_id(),
        username: "founder".to_owned(),
        email: "newcomer@example.com".to_owned(),
        custom_name: String::new(),
        password_hash: "hash".to_owned(),
        language: String::new(),
        start: fvoci_store::SignupStart::Organization("Theirs".to_owned()),
        browser_hash: String::new(),
        created_at: now,
        expires_at: i64::MAX,
    };
    test.scoped(Tenant::Unset)
        .create_signup(&waiting, None, now)
        .await
        .expect("it waits");
    let taken = first_sign_in(&mut newcomer("Founder"), "a-newcomer", "x@example.com").await;
    assert!(
        matches!(
            taken,
            Err(fvoci_store::StoreError::Conflict(
                fvoci_store::Taken::Username
            ))
        ),
        "its name is held: {taken:?}"
    );
    let mut user = newcomer("newcomer");
    let joined = first_sign_in(&mut user, "a-newcomer", "NEWCOMER@example.com").await;
    assert_eq!(joined.expect("stored"), Ok(true), "let in");
    assert!(
        test.live_signup(fvoci_store::SignupLookup::Username("founder"), now)
            .await
            .expect("read")
            .is_some(),
        "the registration of its own stays"
    );
    test.teardown().await;
}
