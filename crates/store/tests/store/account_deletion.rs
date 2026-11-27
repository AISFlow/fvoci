//! An account its person asks to delete: who may ask, what the wait leaves as it is,
//! and taking the asking back.

use fvoci_model::{Leaving, OrgRole, Session, new_id};
use fvoci_store::{DbType, DeletionAsking, Store, StoreError};

use crate::common::{self, TestStore};

/// A time in milliseconds that the tests count from.
const T0: i64 = 1_800_000_000_000;

/// The wait the tests ask for.
const WAIT: i64 = 30 * 24 * 60 * 60 * 1000;

/// The password hash of a user made by [`common::user`]: none.
const NO_HASH: &str = "";

/// A session of the user that was made at [`T0`].
fn session_of(user_id: &str) -> Session {
    Session {
        id: new_id(),
        user_id: user_id.to_owned(),
        created_at: T0,
        last_used_at: T0,
        expires_at: T0 + 2 * WAIT,
        user_agent: "a browser".to_owned(),
        ..Session::default()
    }
}

/// Stores a session of the user under the digest; says whether it was stored.
async fn open(store: &Store, user_id: &str, digest: &str) -> bool {
    store
        .create_session(&session_of(user_id), digest, NO_HASH)
        .await
        .expect("the session is asked for")
}

/// Whether the digest finds a session that opens the account's things.
async fn opens(store: &Store, digest: &str) -> bool {
    match store.get_session(digest, T0 + 1, T0 - 1).await {
        Ok(_) => true,
        Err(StoreError::NoRows) => false,
        Err(other) => panic!("looking a session up failed: {other:?}"),
    }
}

/// Until when the account of the digest's session waits, if it does.
async fn waits(store: &Store, digest: &str) -> Option<Leaving> {
    store
        .leaving_session(digest, T0 + 1, T0 - 1)
        .await
        .expect("the session is looked for")
        .map(|(_, leaving)| leaving)
}

async fn ask(store: &Store, user_id: &str) -> DeletionAsking {
    store
        .request_account_deletion(user_id, NO_HASH, T0, T0 + WAIT)
        .await
        .expect("the deletion is asked")
}

/// Gives a member of the handle's organization the role in it, past the routes.
async fn set_org_role(store: &Store, user_id: &str, role: OrgRole) {
    store
        .execute_unchecked(&format!(
            "UPDATE organization_members SET role = '{}' WHERE user_id = '{user_id}'",
            role.as_str()
        ))
        .await
        .expect("the role is set");
}

/// The first user of the instance, who administers it and the test's organization,
/// and a second administrator of both, so that neither is the last.
async fn two_administrators(test: &TestStore) -> (String, String) {
    let first = common::user(test, "first").await;
    let second = common::user(test, "second").await;
    set_org_role(test, &second, OrgRole::Admin).await;
    test.execute_unchecked(&format!(
        "UPDATE users SET is_admin = true WHERE id = '{second}'"
    ))
    .await
    .expect("the second administers the instance");
    (first, second)
}

fn names(organizations: &[fvoci_model::OrganizationName]) -> Vec<&str> {
    organizations
        .iter()
        .map(|organization| organization.id.as_str())
        .collect()
}

#[tokio::test]
async fn an_account_asked_to_be_deleted_waits_out_of_use_and_comes_back_as_it_was() {
    let test = common::setup().await;
    common::user(&test, "first").await;
    let member = common::user(&test, "member").await;
    assert!(open(&test, &member, "before").await);

    let asked = ask(&test, &member).await;

    let leaving = Leaving {
        requested_at: T0,
        delete_after: T0 + WAIT,
    };
    assert!(matches!(asked, DeletionAsking::Asked(got, _) if got == leaving));
    assert!(!opens(&test, "before").await, "its sessions are ended");
    assert_eq!(waits(&test, "before").await, None, "and gone");
    assert!(
        test.get_user_by_id(&member).await.is_err(),
        "the account is out of use"
    );
    assert_eq!(
        test.account_by_id(&member).await.expect("found").username,
        "member",
        "and still one to sign in to"
    );
    assert!(
        test.get_workspace_member(&test.workspace_id, &member)
            .await
            .expect("read")
            .is_some(),
        "its memberships stay"
    );

    // Signing in meanwhile makes a session that opens the way back only.
    assert!(open(&test, &member, "meanwhile").await);
    assert!(!opens(&test, "meanwhile").await);
    assert_eq!(waits(&test, "meanwhile").await, Some(leaving));
    assert_eq!(ask(&test, &member).await, DeletionAsking::Changed);

    // The wait run out, the way back is open until the erasing begins.
    let late = T0 + WAIT + 1;
    assert_eq!(
        test.leaving_session("meanwhile", late, T0 - 1)
            .await
            .expect("looked for")
            .map(|(_, leaving)| leaving),
        Some(leaving)
    );
    assert!(
        test.cancel_account_deletion(&member, late)
            .await
            .expect("taken back")
    );

    assert!(opens(&test, "meanwhile").await, "the session opens again");
    assert_eq!(waits(&test, "meanwhile").await, None);
    let user = test.get_user_by_id(&member).await.expect("in use again");
    assert_eq!((user.username.as_str(), user.deleted_at), ("member", None));
    assert!(
        !test
            .cancel_account_deletion(&member, late + 1)
            .await
            .expect("asked"),
        "an account in use has nothing to take back"
    );
    test.teardown().await;
}

#[tokio::test]
async fn the_only_administrator_of_an_organization_with_other_people_stays() {
    let test = common::setup().await;
    let (first, second) = two_administrators(&test).await;
    let member = common::user(&test, "member").await;

    // Of two administrators one goes; the other is then the only one in use.
    assert!(matches!(
        ask(&test, &first).await,
        DeletionAsking::Asked(..)
    ));
    let DeletionAsking::Refused(standing) = ask(&test, &second).await else {
        panic!("the last administrator went");
    };
    assert!(standing.last_instance_admin, "of the instance too");
    assert_eq!(names(&standing.sole_admin_of), [test.org_id.as_str()]);
    assert_eq!(standing.alone_in, []);
    assert!(
        test.get_user_by_id(&second).await.is_ok(),
        "refused, the account stays in use"
    );

    // The first comes back: the second may go, and a member always may.
    assert!(
        test.cancel_account_deletion(&first, T0 + 1)
            .await
            .expect("back")
    );
    assert!(matches!(
        ask(&test, &second).await,
        DeletionAsking::Asked(..)
    ));
    assert!(matches!(
        ask(&test, &member).await,
        DeletionAsking::Asked(..)
    ));
    let DeletionAsking::Refused(standing) = ask(&test, &first).await else {
        panic!("the last administrator went");
    };
    assert_eq!(
        names(&standing.sole_admin_of),
        [test.org_id.as_str()],
        "who waits to be deleted is still in the organization"
    );
    test.teardown().await;
}

#[tokio::test]
async fn the_instances_last_administrator_stays_also_alone_in_an_organization() {
    let test = common::setup().await;
    let founder = common::user(&test, "founder").await;

    let standing = test
        .account_deletion_standing(&founder)
        .await
        .expect("read");

    assert!(standing.last_instance_admin);
    // The instance's own organization goes with no account, whoever is in it.
    assert_eq!(names(&standing.sole_admin_of), [test.org_id.as_str()]);
    assert_eq!(standing.alone_in, [], "it is not one that goes");
    assert_eq!(
        ask(&test, &founder).await,
        DeletionAsking::Refused(standing)
    );
    test.teardown().await;
}

/// An organization that has nobody but its administrator does not hold them back, and
/// one of several organizations that has other people does, named alone. SQLite holds
/// one organization.
#[tokio::test]
async fn each_organization_of_an_account_is_asked_for_itself() {
    if common::db_type() != DbType::Postgres {
        return;
    }
    let test = common::setup().await;
    let (_, person) = two_administrators(&test).await;
    let system = test.system();
    let (alone, _) = common::organization(&system, "Alone").await;
    let (shared, shared_workspace) = common::organization(&system, "Shared").await;
    let (others, _) = common::organization(&system, "Somebody else's").await;
    for org_id in [&alone, &shared] {
        test.for_org(org_id)
            .expect("an organization")
            .add_organization_member(&person, OrgRole::Admin)
            .await
            .expect("its administrator");
    }
    common::user(
        &test.for_org(&others).expect("an organization"),
        "elsewhere",
    )
    .await;
    // A guest of a workspace is somebody too, though no member of the organization.
    let guest = common::user(&system, "guest").await;
    test.for_org(&shared)
        .expect("an organization")
        .add_workspace_member(&shared_workspace, &guest, fvoci_model::WorkspaceRole::Guest)
        .await
        .expect("a guest");

    let DeletionAsking::Refused(standing) = ask(&test, &person).await else {
        panic!("the only administrator of an organization with a guest went");
    };
    assert!(!standing.last_instance_admin);
    assert_eq!(names(&standing.sole_admin_of), [shared.as_str()]);
    assert_eq!(names(&standing.alone_in), [alone.as_str()]);

    // The guest gone, both organizations have nobody else.
    system
        .execute_unchecked(&format!(
            "DELETE FROM workspace_members WHERE user_id = '{guest}'"
        ))
        .await
        .expect("the guest leaves");
    let counted = test
        .leaving_organization(standing.alone_in[0].clone())
        .await
        .expect("counted")
        .expect("the organization is still there");
    assert_eq!(
        (
            counted.name.as_str(),
            counted.boards,
            counted.wikis,
            counted.files
        ),
        ("Alone", Some(0), Some(0), Some(0))
    );
    assert!(matches!(
        ask(&test, &person).await,
        DeletionAsking::Asked(..)
    ));
    test.teardown().await;
}

/// Reading the counts of an organization that was already removed (e.g., an own space
/// erased concurrently while a waiting account checks its deletion screen) returns
/// `Ok(None)` instead of `StoreError::NotFound`.
#[tokio::test]
async fn leaving_organization_on_a_gone_organization_does_not_fail_not_found() {
    let test = common::setup().await;
    let gone_id = new_id();
    let gone = test.for_org(&gone_id).expect("an identifier");
    let result = gone
        .leaving_organization(fvoci_model::OrganizationName {
            id: gone_id,
            name: String::new(),
        })
        .await
        .expect("a gone organization is Ok(None), not an error");
    assert_eq!(result, None);
    test.teardown().await;
}

#[tokio::test]
async fn an_account_that_changed_since_it_was_checked_is_not_put_out_of_use() {
    let test = common::setup().await;
    common::user(&test, "first").await;
    let member = common::user(&test, "member").await;

    let asked = test
        .request_account_deletion(
            &member,
            "a hash seen before the password was set anew",
            T0,
            T0 + WAIT,
        )
        .await
        .expect("asked");

    assert_eq!(asked, DeletionAsking::Changed);
    assert!(test.get_user_by_id(&member).await.is_ok());
    test.teardown().await;
}

/// Only an account that waits is taken back: not one whose erasing began, and not one
/// that was put out of use without a wait.
#[tokio::test]
async fn nothing_is_taken_back_once_the_erasing_began() {
    let test = common::setup().await;
    common::user(&test, "first").await;
    let erasing = common::user(&test, "erasing").await;
    let deactivated = common::user(&test, "deactivated").await;
    assert!(matches!(
        ask(&test, &erasing).await,
        DeletionAsking::Asked(..)
    ));
    assert!(open(&test, &erasing, "meanwhile").await);
    for statement in [
        format!("UPDATE users SET erase_started_at = {T0} WHERE id = '{erasing}'"),
        format!("UPDATE users SET deleted_at = {T0} WHERE id = '{deactivated}'"),
    ] {
        test.execute_unchecked(&statement).await.expect("set");
    }

    for user_id in [&erasing, &deactivated] {
        assert!(
            !test
                .cancel_account_deletion(user_id, T0 + 1)
                .await
                .expect("asked"),
            "{user_id} is not taken back"
        );
        assert!(
            test.account_by_id(user_id).await.is_err(),
            "{user_id} is nobody to sign in to"
        );
        assert!(
            !open(&test, user_id, &format!("after {user_id}")).await,
            "no session is made for {user_id}"
        );
    }
    assert_eq!(waits(&test, "meanwhile").await, None);
    test.teardown().await;
}

/// Asking to delete an account, and taking that back, changes what the links of the
/// account's own space open, and nothing of any other organization: the revision of
/// the cached checks of the space moves on, and that of the organization the account
/// works in stays, so that what was checked there is not thrown away for it.
#[tokio::test]
async fn a_deletion_asked_and_taken_back_moves_the_revision_of_the_space_alone() {
    let test = common::setup().await;
    // The first user administers the instance, and stays.
    common::user(&test, "founder").await;
    let leaver = common::user(&test, "leaver").await;
    let space = test
        .account_deletion_standing(&leaver)
        .await
        .expect("read")
        .own_space
        .expect("the account has a space");
    let of_the_space = test.for_org(&space.id).expect("an organization");
    let before = (
        test.visibility_version().await,
        of_the_space.visibility_version().await,
    );

    assert!(matches!(
        ask(&test, &leaver).await,
        DeletionAsking::Asked(..)
    ));
    let asked = (
        test.visibility_version().await,
        of_the_space.visibility_version().await,
    );
    assert!(
        test.cancel_account_deletion(&leaver, T0 + 1)
            .await
            .expect("taken back"),
        "the asking is taken back"
    );
    let back = (
        test.visibility_version().await,
        of_the_space.visibility_version().await,
    );
    test.teardown().await;

    assert_ne!(asked.1, before.1, "asking moves the space's revision");
    assert_ne!(back.1, asked.1, "and so does taking it back");
    assert_eq!(asked.0, before.0, "the organization's stays as it is asked");
    assert_eq!(back.0, before.0, "and as it is taken back");
}
