//! Personal API tokens: stored by their digest, opening the workspaces they name while
//! their owner works in them, and ended with what ends the owner's sessions.

use fvoci_model::{ApiTokenScope, LoginOrigin, OrgRole, Session, Workspace, WorkspaceRole, new_id};
use fvoci_store::{
    DbType, GrantedWorkspace, Identity, Issuance, Store, TokenToIssue, Unlinked, WorkspaceTrashing,
};

use crate::common::{self, TestStore};
use crate::provider_tokens::{provider, put, session};

/// A time in milliseconds the tests count from, before any token ends.
const T0: i64 = 1_800_000_000_000;

/// When the tokens of the tests end.
const ENDS: i64 = T0 + 90 * 24 * 60 * 60 * 1000;

/// A session of `user_id` from [`T0`] to [`ENDS`], stored: its identifier.
async fn session_of(store: &Store, user_id: &str) -> String {
    let session = Session {
        id: new_id(),
        user_id: user_id.to_owned(),
        created_at: T0,
        last_used_at: T0,
        expires_at: ENDS,
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

/// Stores a write token of `user_id` under `digest` that reaches `workspaces` of the
/// handle's organization, made by a new session of theirs.
async fn issue(store: &Store, user_id: &str, digest: &str, workspaces: &[&str]) -> Issuance {
    let session_id = session_of(store, user_id).await;
    issue_by(store, user_id, &session_id, digest, workspaces).await
}

/// As [`issue`], made by the session `session_id`.
async fn issue_by(
    store: &Store,
    user_id: &str,
    session_id: &str,
    digest: &str,
    workspaces: &[&str],
) -> Issuance {
    let org_id = store
        .tenant()
        .org_id()
        .expect("a handle of an organization");
    let granted: Vec<GrantedWorkspace<'_>> = workspaces
        .iter()
        .map(|workspace_id| GrantedWorkspace {
            workspace_id,
            org_id: &org_id,
        })
        .collect();
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
        .expect("the token is stored")
}

/// The identifier of a token stored as [`issue`] stores it.
async fn issued(store: &Store, user_id: &str, digest: &str, workspaces: &[&str]) -> String {
    match issue(store, user_id, digest, workspaces).await {
        Issuance::Issued(token) => token.id,
        refused => panic!("{refused:?}"),
    }
}

/// Whether the token stored under `digest` opens anything at [`T0`].
async fn is_live(store: &Store, digest: &str) -> bool {
    store
        .live_api_token(digest, T0 + 1)
        .await
        .expect("looked up")
        .is_some()
}

/// Whether the token reaches the workspace, asked of the handle's organization.
async fn reaches(store: &Store, token_id: &str, workspace_id: &str) -> bool {
    store
        .api_token_reach(token_id, workspace_id)
        .await
        .expect("asked")
        .is_some()
}

/// An administrator of the test's organization.
async fn org_admin(test: &TestStore, username: &str) -> String {
    let user_id = common::user(test, username).await;
    for statement in [
        format!(
            "UPDATE organization_members SET role = '{}' WHERE user_id = '{user_id}'",
            OrgRole::Admin.as_str()
        ),
        format!("UPDATE users SET is_admin = TRUE WHERE id = '{user_id}'"),
    ] {
        test.execute_unchecked(&statement)
            .await
            .expect("the user administers the organization");
    }
    user_id
}

#[tokio::test]
async fn a_token_opens_the_workspaces_it_names_until_it_is_taken_back() {
    let test = common::setup().await;
    let store = &test.store;
    let alice = common::user(store, "alice").await;
    let named = common::workspace_of(store, &[&alice]).await;
    let other = common::workspace_of(store, &[&alice]).await;
    let token_id = issued(store, &alice, "digest-alice", &[&named]).await;

    let live = store
        .live_api_token("digest-alice", T0 + 1)
        .await
        .expect("looked up")
        .expect("the token is live");
    assert_eq!(
        (live.id.as_str(), live.user_id.as_str()),
        (token_id.as_str(), alice.as_str())
    );
    assert_eq!(live.scope, ApiTokenScope::Write);
    assert!(
        reaches(store, &token_id, &named).await,
        "the workspace it names"
    );
    assert!(
        !reaches(store, &token_id, &other).await,
        "not one it does not name"
    );
    assert!(
        store
            .live_api_token("digest-alice", ENDS)
            .await
            .expect("looked up")
            .is_none(),
        "past its end it opens nothing"
    );
    let listed = store.api_tokens_of_user(&alice).await.expect("listed");
    assert_eq!(listed.len(), 1);
    assert_eq!(
        listed[0]
            .workspaces
            .iter()
            .map(|workspace| workspace.workspace_id.as_str())
            .collect::<Vec<_>>(),
        [named.as_str()]
    );

    let bob = common::user(store, "bob").await;
    assert!(
        !store
            .revoke_api_token(&bob, &token_id, T0 + 2)
            .await
            .expect("asked"),
        "nobody takes back another's token"
    );
    assert!(
        store
            .revoke_api_token(&alice, &token_id, T0 + 2)
            .await
            .expect("taken back")
    );
    assert!(
        !is_live(store, "digest-alice").await,
        "taken back, it opens nothing"
    );
    assert!(
        store
            .api_tokens_of_user(&alice)
            .await
            .expect("listed")
            .is_empty(),
        "and is not listed"
    );

    test.teardown().await;
}

#[tokio::test]
async fn a_token_is_given_no_workspace_its_maker_does_not_work_in() {
    let test = common::setup().await;
    let store = &test.store;
    let alice = common::user(store, "alice").await;
    let theirs = common::workspace_of(store, &[&alice]).await;
    let not_theirs = common::workspace(store).await;
    let as_guest = common::workspace(store).await;
    store
        .add_workspace_member(&as_guest, &alice, WorkspaceRole::Guest)
        .await
        .expect("a guest of it");

    for (refused, digest) in [(&not_theirs, "digest-none"), (&as_guest, "digest-guest")] {
        let issuance = issue(store, &alice, digest, &[&theirs, refused]).await;
        assert_eq!(issuance, Issuance::NoLongerWorksIn(refused.clone()));
        assert!(!is_live(store, digest).await, "nothing of the token stays");
    }
    let listed = store.api_tokens_of_user(&alice).await.expect("listed");
    assert_eq!(listed, [], "no token was made");

    test.teardown().await;
}

#[tokio::test]
async fn what_ends_the_sessions_of_an_account_ends_its_tokens() {
    let test = common::setup().await;
    let store = &test.store;
    let alice = common::user(store, "alice").await;
    let bob = common::user(store, "bob").await;
    let workspace = common::workspace_of(store, &[&alice, &bob]).await;
    issued(store, &alice, "digest-alice", &[&workspace]).await;
    issued(store, &bob, "digest-bob", &[&workspace]).await;
    let kept = Session {
        id: new_id(),
        user_id: alice.clone(),
        created_at: T0,
        last_used_at: T0,
        expires_at: ENDS,
        ..Session::default()
    };
    assert!(
        store
            .create_session(&kept, "alice-here", "")
            .await
            .expect("the session is stored")
    );

    let set = store
        .change_user_password(&alice, "", "new-hash", &kept.id, "alice-here-anew")
        .await
        .expect("the password is set");
    assert!(set.is_some(), "the hash was the one seen");
    assert!(
        !is_live(store, "digest-alice").await,
        "a password set ends the tokens, the session it was set through stays"
    );
    assert!(is_live(store, "digest-bob").await, "another's stay");

    store
        .request_account_deletion(&bob, "", T0 + 1, T0 + 2)
        .await
        .expect("asked");
    assert!(
        !is_live(store, "digest-bob").await,
        "an account put out of use"
    );

    test.teardown().await;
}

#[tokio::test]
async fn a_token_stops_reaching_a_workspace_its_owner_no_longer_works_in() {
    let test = common::setup().await;
    let store = &test.store;
    let admin = org_admin(&test, "admin").await;
    let alice = common::user(store, "alice").await;
    let left = common::workspace_of(store, &[&admin, &alice]).await;
    let guest_of = common::workspace_of(store, &[&admin, &alice]).await;
    let trashed = Workspace {
        id: new_id(),
        title: "trashed".to_owned(),
        ..Workspace::default()
    };
    store
        .create_named_workspace(&trashed, &admin)
        .await
        .expect("the workspace is made");
    store
        .add_workspace_member(&trashed.id, &alice, WorkspaceRole::Member)
        .await
        .expect("a member of it");
    let kept = common::workspace_of(store, &[&alice]).await;
    let token_id = issued(
        store,
        &alice,
        "digest",
        &[&left, &guest_of, &trashed.id, &kept],
    )
    .await;

    store
        .remove_workspace_member(&left, &alice, &admin)
        .await
        .expect("removed");
    store
        .change_workspace_role(
            &guest_of,
            &alice,
            (WorkspaceRole::Member, WorkspaceRole::Guest),
            &admin,
        )
        .await
        .expect("made a guest");
    assert_eq!(
        store
            .trash_workspace(&trashed.id, &admin)
            .await
            .expect("trashed"),
        WorkspaceTrashing::Done
    );

    for gone in [&left, &guest_of, &trashed.id] {
        assert!(!reaches(store, &token_id, gone).await, "{gone}");
    }
    assert!(
        reaches(store, &token_id, &kept).await,
        "the one still worked in"
    );
    assert!(is_live(store, "digest").await, "the token itself stays");

    test.teardown().await;
}

#[tokio::test]
async fn a_token_made_through_a_provider_ends_with_an_identity_unlinked() {
    let test = common::setup().await;
    let store = &test.store;
    let alice = common::user(store, "alice").await;
    let workspace = common::workspace_of(store, &[&alice]).await;
    let of_two = put(store, &provider("https://two.example.com"), None, &alice).await;
    let login = LoginOrigin {
        provider_id: of_two.id.clone(),
        org_id: of_two.org_id.clone(),
        issuer: of_two.issuer.clone(),
        subject: Some("alice".to_owned()),
        revision: Some(of_two.revision),
    };
    let through = session(store, &alice, Some(login)).await;
    for digest in ["digest-sso", "digest-other"] {
        let Issuance::Issued(_) = issue_by(store, &alice, &through, digest, &[&workspace]).await
        else {
            panic!("the token is stored");
        };
    }
    // As if made through the identity of another issuer, as the provider of another
    // organization of the account's would make one (SQLite holds one organization).
    store
        .execute_unchecked(
            "UPDATE api_tokens SET origin_issuer = 'https://one.example.com' \
             WHERE token_hash = 'digest-other'",
        )
        .await
        .expect("written");
    issued(store, &alice, "digest-plain", &[&workspace]).await;
    let live = store
        .live_api_token("digest-sso", T0 + 1)
        .await
        .expect("looked up")
        .expect("live");
    assert_eq!(live.origin_provider_id.as_deref(), Some(of_two.id.as_str()));

    let (kept, unlinked) = (
        Identity {
            issuer: "https://one.example.com",
            subject: "alice",
        },
        Identity {
            issuer: "https://two.example.com",
            subject: "alice",
        },
    );
    for identity in [kept, unlinked] {
        store
            .add_identity(&alice, identity, "")
            .await
            .expect("an identity is tied");
    }
    let outcome = store
        .unlink_identity(&alice, "", unlinked)
        .await
        .expect("asked");
    assert!(matches!(outcome, Unlinked::Removed(_)), "{outcome:?}");
    assert!(
        !is_live(store, "digest-sso").await,
        "made through the identity unlinked"
    );
    assert!(
        is_live(store, "digest-other").await,
        "made through another identity"
    );
    assert!(
        is_live(store, "digest-plain").await,
        "made with the password"
    );

    test.teardown().await;
}

#[tokio::test]
async fn an_administrator_sees_and_takes_back_the_tokens_of_their_organization() {
    let test = common::setup().await;
    let store = &test.store;
    let admin = org_admin(&test, "admin").await;
    let alice = common::user(store, "alice").await;
    let workspace = common::workspace_of(store, &[&alice]).await;
    let token_id = issued(store, &alice, "digest", &[&workspace]).await;
    let listed = store
        .api_tokens_of_organization(T0 + 1)
        .await
        .expect("listed");
    assert_eq!(
        listed
            .iter()
            .map(|token| token.id.as_str())
            .collect::<Vec<_>>(),
        [token_id.as_str()]
    );
    // An SQLite database holds one organization: the wall is PostgreSQL's to keep.
    if common::db_type() == DbType::Postgres {
        let system = test.system();
        let (elsewhere, _) = common::organization(&system, "elsewhere").await;
        let elsewhere = system.for_org(&elsewhere).expect("an identifier");
        assert!(
            elsewhere
                .api_tokens_of_organization(T0 + 1)
                .await
                .expect("listed")
                .is_empty(),
            "another organization sees none of them"
        );
        assert!(
            !elsewhere
                .revoke_api_token_of_organization(&admin, &token_id, T0 + 2)
                .await
                .expect("asked"),
            "nor takes one back"
        );
        assert!(!reaches(&elsewhere, &token_id, &workspace).await);
    }
    assert!(
        store
            .revoke_api_token_of_organization(&admin, &token_id, T0 + 2)
            .await
            .expect("taken back")
    );
    assert!(
        !is_live(store, "digest").await,
        "reaching nothing else, it is taken back"
    );

    test.teardown().await;
}

#[tokio::test]
async fn an_administrator_takes_a_token_out_of_their_organization_alone() {
    // An SQLite database holds one organization.
    if common::db_type() != DbType::Postgres {
        return;
    }
    let test = common::setup().await;
    let store = &test.store;
    let admin = org_admin(&test, "admin").await;
    let alice = common::user(store, "alice").await;
    let here = common::workspace_of(store, &[&alice]).await;
    let system = test.system();
    let (elsewhere, _) = common::organization(&system, "elsewhere").await;
    let elsewhere = system.for_org(&elsewhere).expect("an identifier");
    let there = common::workspace_of(&elsewhere, &[&alice]).await;
    let token_id = issued(store, &alice, "digest", &[&here]).await;
    // The same token is given a workspace of the other organization as well.
    let org_there = elsewhere.tenant().org_id().expect("an organization");
    elsewhere
        .execute_unchecked(&format!(
            "INSERT INTO api_token_workspaces (org_id, token_id, workspace_id) \
             VALUES ('{org_there}', '{token_id}', '{there}')"
        ))
        .await
        .expect("the token reaches the other workspace");

    assert!(
        store
            .revoke_api_token_of_organization(&admin, &token_id, T0 + 2)
            .await
            .expect("taken out")
    );
    assert!(!reaches(store, &token_id, &here).await, "out of this one");
    assert!(
        reaches(&elsewhere, &token_id, &there).await,
        "the other organization's stays"
    );
    assert!(is_live(store, "digest").await, "and the token with it");
    assert!(
        store
            .api_tokens_of_organization(T0 + 3)
            .await
            .expect("listed")
            .is_empty(),
        "the administrator lists it no more"
    );
    assert!(
        !store
            .revoke_api_token_of_organization(&admin, &token_id, T0 + 3)
            .await
            .expect("asked"),
        "nor takes it out again"
    );

    test.teardown().await;
}

#[tokio::test]
async fn a_token_is_made_by_a_session_in_use_and_an_account_holds_so_many() {
    let test = common::setup().await;
    let store = &test.store;
    let alice = common::user(store, "alice").await;
    let workspace = common::workspace_of(store, &[&alice]).await;
    let session_id = session_of(store, &alice).await;
    let ended = session_of(store, &alice).await;
    store
        .execute_unchecked(&format!("DELETE FROM sessions WHERE id = '{ended}'"))
        .await
        .expect("the session ends");
    assert_eq!(
        issue_by(store, &alice, &ended, "digest-ended", &[&workspace]).await,
        Issuance::SessionEnded,
        "a session that ended makes no token"
    );
    assert!(!is_live(store, "digest-ended").await);

    let mut first = None;
    for made in 0..fvoci_model::API_TOKEN_MAX_LIVE {
        let digest = format!("digest-{made}");
        let Issuance::Issued(token) =
            issue_by(store, &alice, &session_id, &digest, &[&workspace]).await
        else {
            panic!("token {made} is made");
        };
        first.get_or_insert(token.id);
    }
    assert_eq!(
        issue_by(store, &alice, &session_id, "digest-more", &[&workspace]).await,
        Issuance::TooMany
    );
    let first = first.expect("a token was made");
    assert!(
        store
            .revoke_api_token(&alice, &first, T0 + 1)
            .await
            .expect("taken back")
    );
    assert!(
        matches!(
            issue_by(store, &alice, &session_id, "digest-more", &[&workspace]).await,
            Issuance::Issued(_)
        ),
        "one taken back makes room"
    );

    test.teardown().await;
}
