//! A session made through a provider makes a token only while the provider is as it
//! was when the session was made, and a change of the provider takes back the token a
//! session was making as it changed: on PostgreSQL the test stops the token before it
//! is stored, changes the provider, and lets the token go once the change waits for it.

use std::time::Duration;

use fvoci_model::{ApiTokenScope, LoginOrigin, Session, new_id};
use fvoci_store::{DbType, Issuance, ProviderPut, Store, StoredProvider, TokenToIssue};

use crate::common;
use crate::lock_gate::{Gate, ended};

/// The time the sessions and the tokens are made at, in milliseconds.
const T0: i64 = 1_800_000_000_000;

/// When the sessions and the tokens end.
const ENDS: i64 = T0 + 24 * 60 * 60 * 1000;

pub(crate) fn provider(issuer: &str) -> StoredProvider {
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

/// Stores `next` in place of `before` (or as the first provider) on `store`.
pub(crate) async fn put(
    store: &Store,
    next: &StoredProvider,
    before: Option<&StoredProvider>,
    actor: &str,
) -> StoredProvider {
    let ProviderPut { provider, .. } = store
        .put_oidc_provider_as_read(next, before, actor, Duration::ZERO)
        .await
        .expect("stored")
        .expect("as read");
    provider
}

/// Where a session made through `provider` came from, at `revision`.
fn origin(provider: &StoredProvider, revision: Option<i64>) -> LoginOrigin {
    LoginOrigin {
        provider_id: provider.id.clone(),
        org_id: provider.org_id.clone(),
        issuer: provider.issuer.clone(),
        subject: Some("subject".to_owned()),
        revision,
    }
}

/// Stores a session of `user_id`, made through `login` or with a password: its
/// identifier.
pub(crate) async fn session(store: &Store, user_id: &str, login: Option<LoginOrigin>) -> String {
    let session = Session {
        id: new_id(),
        user_id: user_id.to_owned(),
        created_at: T0,
        last_used_at: T0,
        expires_at: ENDS,
        login,
        ..Session::default()
    };
    assert!(
        store
            .create_session(&session, &new_id(), "")
            .await
            .expect("the session is stored"),
        "a new session"
    );
    session.id
}

/// Stores a token of `user_id` under `digest`, reaching `workspace_id` of the handle's
/// organization, made by the session `session_id`: with the provider the store reads
/// the session was made through.
pub(crate) async fn issue(
    store: &Store,
    user_id: &str,
    session_id: &str,
    digest: &str,
    workspace_id: &str,
) -> Issuance {
    let org_id = store
        .tenant()
        .org_id()
        .expect("a handle of an organization");
    let granted = [fvoci_store::GrantedWorkspace {
        workspace_id,
        org_id: &org_id,
    }];
    let id = new_id();
    store
        .issue_api_token(&TokenToIssue {
            id: &id,
            user_id,
            session_id,
            name: "agent",
            token_prefix: "fvoci_pat_abcdef",
            token_digest: digest,
            scope: ApiTokenScope::Write,
            workspaces: &granted,
            allow_personal_space: false,
            notify_self: true,
            created_at: T0,
            expires_at: ENDS,
        })
        .await
        .expect("asked")
}

/// Whether the token stored under `digest` opens anything just after [`T0`].
async fn is_live(store: &Store, digest: &str) -> bool {
    store
        .live_api_token(digest, T0 + 1)
        .await
        .expect("looked up")
        .is_some()
}

/// A session makes a token while its provider is there, switched on and at the
/// session's revision. A new secret, the provider switched off, or a session that
/// knows no revision makes none; a session made with a password is not asked.
#[tokio::test]
async fn a_session_makes_a_token_only_while_its_provider_is_as_it_was() {
    let test = common::setup().await;
    let alice = common::user(&test, "alice").await;
    let workspace = common::workspace_of(&test, &[&alice]).await;
    let first = put(&test, &provider("https://one.example"), None, &alice).await;
    let at_first = origin(&first, Some(first.revision));
    let of_first = session(&test, &alice, Some(at_first)).await;
    let unrevised = origin(&first, None);
    let knows_no_revision = session(&test, &alice, Some(unrevised)).await;
    let password = session(&test, &alice, None).await;

    let made = issue(&test, &alice, &of_first, "first", &workspace).await;
    assert!(matches!(made, Issuance::Issued(_)), "{made:?}");
    assert_eq!(
        test.texts_unchecked(
            "SELECT CAST(origin_provider_id AS TEXT) || ' ' || CAST(origin_provider_revision \
             AS TEXT) || ' ' || origin_issuer || ' ' || origin_subject FROM api_tokens \
             WHERE token_hash = 'first'"
        )
        .await
        .expect("read"),
        [format!(
            "{} {} https://one.example subject",
            first.id, first.revision
        )],
        "stored with the provider the session was made through, as the session keeps it"
    );
    assert_eq!(
        issue(&test, &alice, &knows_no_revision, "unrevised", &workspace).await,
        Issuance::ProviderChanged,
        "a session that knows no revision proves none"
    );

    let mut next = first.clone();
    next.sealed_client_secret = "another sealed secret".to_owned();
    let second = put(&test, &next, Some(&first), &alice).await;
    assert_eq!(
        second.id, first.id,
        "the provider stays, at another revision"
    );
    assert!(
        !is_live(&test, "first").await,
        "the token of the revision before ends"
    );
    assert_eq!(
        issue(&test, &alice, &of_first, "stale", &workspace).await,
        Issuance::ProviderChanged,
        "the session stays, and proves the revision before"
    );
    let at_second = origin(&second, Some(second.revision));
    let of_second = session(&test, &alice, Some(at_second)).await;
    let made = issue(&test, &alice, &of_second, "second", &workspace).await;
    assert!(matches!(made, Issuance::Issued(_)), "{made:?}");

    next.enabled = false;
    let off = put(&test, &next, Some(&second), &alice).await;
    assert!(!off.enabled, "switched off");
    assert_eq!(
        issue(&test, &alice, &of_second, "off", &workspace).await,
        Issuance::ProviderChanged,
        "a provider switched off proves nothing"
    );
    let made = issue(&test, &alice, &password, "password", &workspace).await;
    assert!(matches!(made, Issuance::Issued(_)), "{made:?}");
    for digest in ["unrevised", "stale", "off"] {
        assert!(!is_live(&test, digest).await, "{digest}: not stored");
    }
    test.teardown().await;
}

/// How the provider changes while a token is made.
#[derive(Debug, Clone, Copy)]
enum Change {
    /// Another secret: the provider stays, at another revision, and so do the sessions.
    Secret,
    /// Another issuer: the provider and the sessions made through it end.
    Issuer,
}

/// A token is stopped before it is stored, by a session made through the provider; the
/// provider changes meanwhile. The change waits for the token and takes it back once it
/// is stored, whichever the change. PostgreSQL only: on SQLite writers take turns.
async fn a_change_of_the_provider_takes_back_the_token_under_way(change: Change) {
    let test = common::setup().await;
    if common::db_type() != DbType::Postgres {
        test.teardown().await;
        return;
    }
    let alice = common::user(&test, "alice").await;
    let workspace = common::workspace_of(&test, &[&alice]).await;
    let first = put(&test, &provider("https://one.example"), None, &alice).await;
    let login = origin(&first, Some(first.revision));
    let session_id = session(&test, &alice, Some(login)).await;
    let mut next = first.clone();
    match change {
        Change::Secret => "another sealed secret".clone_into(&mut next.sealed_client_secret),
        Change::Issuer => "https://two.example".clone_into(&mut next.issuer),
    }

    let mut gate = Gate::before(
        &test,
        "INSERT",
        "api_tokens",
        "NEW.token_hash = 'under-way'",
    )
    .await;
    let second = gate.second_store(&test).await;
    let making = {
        let store = test.store.clone();
        let (alice, workspace) = (alice.clone(), workspace.clone());
        tokio::spawn(
            async move { issue(&store, &alice, &session_id, "under-way", &workspace).await },
        )
    };
    let holder = gate.stopped(&making).await;
    let changing = {
        let (first, alice) = (first.clone(), alice.clone());
        tokio::spawn(async move { put(&second, &next, Some(&first), &alice).await })
    };
    gate.waits_for(holder, &changing).await;
    gate.open().await;
    let made = ended(making).await;
    let changed = ended(changing).await;

    assert!(matches!(made, Issuance::Issued(_)), "{change:?}: {made:?}");
    assert!(
        changed.revision > first.revision,
        "{change:?}: another revision"
    );
    assert!(
        !is_live(&test, "under-way").await,
        "{change:?}: the token made as the provider changed is taken back"
    );
    test.teardown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_new_secret_takes_back_the_token_under_way() {
    a_change_of_the_provider_takes_back_the_token_under_way(Change::Secret).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_new_issuer_takes_back_the_token_under_way() {
    a_change_of_the_provider_takes_back_the_token_under_way(Change::Issuer).await;
}
