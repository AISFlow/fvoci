//! A registration that replaces the one of its browser, and an acceptance that does,
//! each hold two links, here of two organizations: whichever goes first holds both
//! before the other holds any (`crates/store/src/link_locks.rs`), by the turn of
//! several links, as the rows of the two organizations would not keep them apart. On
//! PostgreSQL the test stops the registration once it holds its first link, starts the
//! acceptance, and lets the registration go once the acceptance waits for it; on
//! SQLite, which holds one organization and whose writers take turns, the two run one
//! after the other, in one organization.

use fvoci_model::ContainerRole;
use fvoci_model::new_id;
use fvoci_store::{
    Acceptor, DbType, NewLink, SIGNUP_LIFETIME_MILLIS, Signup, SignupLookup, SignupStart, Store,
    Tenant,
};

use crate::common::{self, TestStore};
use crate::invitations::{LATER, MakerFound, NOW, link, newcomer};
use crate::lock_gate::{Gate, ended};

/// When the registration again and the acceptance happen: after the registration that
/// ends first has ended.
const AFTER: i64 = NOW + 10;

/// A registration of `username` from the browser whose marker has the digest
/// `browser`, waiting until `expires_at`.
fn registration(username: &str, browser: &str, expires_at: i64) -> Signup {
    Signup {
        id: new_id(),
        username: username.to_owned(),
        email: format!("{username}@example.test"),
        custom_name: String::new(),
        password_hash: "hash".to_owned(),
        language: String::new(),
        start: SignupStart::Organization("Ours".to_owned()),
        browser_hash: browser.to_owned(),
        created_at: NOW,
        expires_at,
    }
}

/// Registers by the link of `token_hash`, as the server does: its use is taken, and
/// the registration's start is the link.
async fn register_by(store: &Store, token_hash: &str, signup: &Signup) {
    let maker = MakerFound::of(store, token_hash, NOW)
        .await
        .expect("the maker is found");
    let reserved = store
        .reserve_invitation(token_hash, signup, None, NOW, maker.inviter())
        .await
        .expect("registered");
    assert!(reserved.accepted().is_some(), "the link lets them register");
}

/// How often the link of `token_hash` of the organization of `store` counts as used.
async fn uses_of(store: &Store, token_hash: &str) -> i64 {
    store
        .unended_invitation(token_hash, AFTER)
        .await
        .expect("the link is read")
        .expect("the link")
        .uses
}

/// The organization the second link is of: on PostgreSQL another, and on SQLite, which
/// holds one, the test's.
async fn other_organization(test: &TestStore) -> (Store, String) {
    if common::db_type() != DbType::Postgres {
        return (test.store.clone(), test.org_id.clone());
    }
    let (org_id, _) = common::organization(&test.scoped(Tenant::Unset), "Other").await;
    (test.for_org(&org_id).expect("an identifier"), org_id)
}

/// The registration from the browser `one` replaces its own, made by the second link,
/// after the clean-up before it removes one by the first link that ended; the
/// acceptance of the second link from the browser `two` replaces that browser's
/// registration, made by the first link. Each holds one of the links and then the
/// other, in the other order.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_registration_again_and_an_acceptance_again_take_turns() {
    let test = common::setup().await;
    let admin = common::user(&test, "admin").await;
    link(&test, &admin, "first", None, None).await;
    let (theirs, their_org) = other_organization(&test).await;
    let their_admin = common::user(&theirs, "their-admin").await;
    let their_workspace = common::workspace(&theirs).await;
    theirs
        .create_invitation(&NewLink {
            workspace_id: &their_workspace,
            guest_of: None::<(&str, ContainerRole)>,
            token_hash: "second",
            created_by: &their_admin,
            now: NOW,
            expires_at: LATER + SIGNUP_LIFETIME_MILLIS,
            max_uses: None,
            mailed: None,
        })
        .await
        .expect("the second link is stored");
    let waiting = NOW + SIGNUP_LIFETIME_MILLIS;
    register_by(&test, "first", &registration("ended", "gone", NOW + 1)).await;
    register_by(&test, "first", &registration("tess", "two", waiting)).await;
    register_by(&theirs, "second", &registration("olga", "one", waiting)).await;
    let again = registration("olga", "one", AFTER + SIGNUP_LIFETIME_MILLIS);
    let maker = MakerFound::of(&theirs, "second", AFTER)
        .await
        .expect("the maker is found");
    let mut account = newcomer("tess");

    if common::db_type() == DbType::Postgres {
        let mut gate = Gate::before(&test, "DELETE", "signups", "OLD.browser_hash = 'gone'").await;
        let acceptor = gate.second_store_of(&test, &their_org).await;
        let registrar = test.store.clone();
        let renewed = again.clone();
        let registering =
            tokio::spawn(
                async move { registrar.create_signup(&renewed, Some("one"), AFTER).await },
            );
        let holding = gate.stopped(&registering).await;
        let accepting = tokio::spawn(async move {
            let replacing = Acceptor::Replacing {
                user: &mut account,
                browser: "two",
            };
            acceptor
                .accept_invitation("second", replacing, AFTER, maker.inviter())
                .await
        });
        gate.waits_for(holding, &accepting).await;
        gate.open().await;
        ended(registering).await.expect("registered again");
        let accepted = ended(accepting).await.expect("accepted");
        assert!(accepted.accepted().is_some(), "let in");
    } else {
        test.create_signup(&again, Some("one"), AFTER)
            .await
            .expect("registered again");
        let replacing = Acceptor::Replacing {
            user: &mut account,
            browser: "two",
        };
        let accepted = theirs
            .accept_invitation("second", replacing, AFTER, maker.inviter())
            .await
            .expect("accepted");
        assert!(accepted.accepted().is_some(), "let in");
    }
    assert_eq!(
        uses_of(&test, "first").await,
        0,
        "both its registrations went"
    );
    assert_eq!(
        uses_of(&theirs, "second").await,
        1,
        "its registration went, and its acceptance counts"
    );
    let olga = test
        .live_signup(SignupLookup::Username("olga"), AFTER)
        .await
        .expect("read");
    assert_eq!(
        olga.map(|signup| signup.id),
        Some(again.id),
        "the registration again waits"
    );
    let waiting_tess = test
        .live_signup(SignupLookup::Username("tess"), AFTER)
        .await
        .expect("read");
    assert_eq!(waiting_tess, None, "replaced by the account");
    test.teardown().await;
}

/// An acceptance of a link for members from a browser whose registration the same link
/// made replaces that registration: it holds the link, and then the link of what it
/// replaces, after that link's organization's row. Revoking the link holds that row and
/// then the link. The acceptance holds the row first: the revocation waits for it. The
/// acceptance is stopped after the link, where it holds the workspace's boards in the
/// trash before its maker's membership, by a gate that holds them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_acceptance_again_and_a_revocation_of_its_link_take_turns() {
    let test = common::setup().await;
    let admin = common::user(&test, "admin").await;
    let members = link(&test, &admin, "members", None, None).await;
    let boards = common::create_test_boards(&test, &test.workspace_id, &admin, 1).await;
    test.delete_container(&boards[0].id, &admin)
        .await
        .expect("a board in the trash");
    let waiting = NOW + SIGNUP_LIFETIME_MILLIS;
    register_by(&test, "members", &registration("sam", "mine", waiting)).await;
    let maker = MakerFound::of(&test, "members", AFTER)
        .await
        .expect("the maker is found");
    let mut account = newcomer("sam");

    if common::db_type() == DbType::Postgres {
        let trashed = format!(
            "SELECT id FROM deleted_containers WHERE workspace_id = '{}' FOR UPDATE",
            test.workspace_id
        );
        let mut gate = Gate::holding(&test, &trashed).await;
        let revoker = gate.second_store(&test).await;
        let acceptor = test.store.clone();
        let accepting = tokio::spawn(async move {
            let replacing = Acceptor::Replacing {
                user: &mut account,
                browser: "mine",
            };
            acceptor
                .accept_invitation("members", replacing, AFTER, maker.inviter())
                .await
        });
        let holding = gate.stopped(&accepting).await;
        let workspace_id = test.workspace_id.clone();
        let revoking = tokio::spawn(async move {
            revoker
                .revoke_invitation(&workspace_id, &members.id, None, AFTER)
                .await
        });
        gate.waits_for(holding, &revoking).await;
        gate.open().await;
        let accepted = ended(accepting).await.expect("accepted");
        assert!(
            accepted.accepted().is_some(),
            "let in before the revocation"
        );
        assert!(ended(revoking).await.expect("revoked"), "and revoked after");
    } else {
        let replacing = Acceptor::Replacing {
            user: &mut account,
            browser: "mine",
        };
        let accepted = test
            .accept_invitation("members", replacing, AFTER, maker.inviter())
            .await
            .expect("accepted");
        assert!(accepted.accepted().is_some(), "let in");
        let revoked = test.revoke_invitation(&test.workspace_id, &members.id, None, AFTER);
        assert!(revoked.await.expect("revoked"));
    }
    let sam = test
        .live_signup(SignupLookup::Username("sam"), AFTER)
        .await
        .expect("read");
    assert_eq!(sam, None, "replaced by the account");
    test.teardown().await;
}
