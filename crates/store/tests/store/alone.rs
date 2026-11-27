//! An account that starts no organization: it has its own space and nothing besides,
//! is never the first of an instance, and starts an organization later, once.

use std::sync::Arc;

use fvoci_model::{OrgKind, OrgRole, User, WorkspaceRole, new_id};
use fvoci_store::{
    DbType, Founding, Joining, NewLink, OwnSpace, Proof, SIGNUP_LIFETIME_MILLIS, Signup,
    SignupStart, Started, StartedOrganization, Store, StoreError,
};
use tokio::sync::Barrier;

use crate::common;

fn account(username: &str) -> User {
    User {
        id: new_id(),
        username: username.to_owned(),
        email: format!("{username}@example.com"),
        password_hash: "hash".to_owned(),
        ..User::default()
    }
}

/// How many rows a table has, in every organization.
async fn rows(store: &Store, table: &str) -> usize {
    let query = format!("SELECT 'row' FROM {table}");
    let found = store.system().texts_unchecked(&query).await;
    found.expect("read").len()
}

/// The workspaces the account is listed, as the organization, its kind and the role.
async fn listed(store: &Store, user_id: &str) -> Vec<(String, OrgKind, WorkspaceRole)> {
    store
        .workspaces_of_user(user_id, fvoci_store::Making::with_password())
        .await
        .expect("listed")
        .into_iter()
        .map(|workspace| (workspace.org_id, workspace.kind, workspace.role))
        .collect()
}

/// Until an instance has its first user nobody is stored that starts no organization,
/// and nothing of the try stays. After it, such an account has its own space, is in
/// no organization, and administers nothing: not the instance, and not the
/// organization of the instance.
#[tokio::test]
async fn an_account_alone_is_never_the_first_and_is_in_no_organization() {
    let test = common::setup().await;
    let store = &test.store;
    let before: Vec<usize> = [
        rows(store, "users").await,
        rows(store, "organizations").await,
    ]
    .into();

    let refused = store.sign_up_alone(&mut account("early")).await;
    assert!(
        matches!(refused, Err(StoreError::NoFirstUser)),
        "the first account starts the organization: {refused:?}"
    );
    assert!(!store.is_bootstrapped().await.expect("read"));
    assert_eq!(
        before,
        [
            rows(store, "users").await,
            rows(store, "organizations").await
        ],
        "nothing of the try stays"
    );

    let mut first = account("first");
    store
        .sign_up(&mut first, "Ours", Founding::First)
        .await
        .expect("the first account takes the organization");
    let mut alone = account("alone");
    let own = store
        .sign_up_alone(&mut alone)
        .await
        .expect("an account that starts nothing");
    assert_eq!(
        common::own_space_shape(&test.system(), &alone.id).await,
        common::own_space()
    );
    assert_eq!(
        own.workspace_id,
        common::own_workspace(&test, &alone.id).await
    );
    assert_eq!(
        listed(store, &alone.id).await,
        [(own.org_id.clone(), OrgKind::Personal, WorkspaceRole::Admin)],
        "its own space, and no other workspace"
    );
    let stored = store.get_user_by_id(&alone.id).await.expect("the account");
    assert!(!stored.is_admin, "it administers no instance");
    assert_eq!(
        store
            .org_role_of_user(&alone.id, &test.org_id)
            .await
            .expect("read"),
        None,
        "and is not in the organization of the instance"
    );
    assert_eq!(
        test.instance_organization().await.expect("read"),
        Some(test.org_id.clone()),
        "which is the one the first account took"
    );
    test.teardown().await;
}

/// The first registration of an instance and registrations that start nothing, all
/// at once: the one that names the organization is the first whatever the order, and
/// each of the others is refused or stored as an account that administers nothing.
#[tokio::test]
async fn at_once_with_the_first_account_one_alone_is_refused_or_administers_nothing() {
    let test = common::setup().await;
    let store = &test.store;
    let barrier = Arc::new(Barrier::new(5));
    let found = async {
        barrier.wait().await;
        let mut first = account("first");
        store.sign_up(&mut first, "Ours", Founding::First).await?;
        Ok::<String, StoreError>(first.id)
    };
    let alone = async |name: &'static str| {
        barrier.wait().await;
        let mut user = account(name);
        match store.sign_up_alone(&mut user).await {
            Ok(_) => Some(user.id),
            Err(StoreError::NoFirstUser) => None,
            Err(error) => panic!("{name}: {error}"),
        }
    };
    let (first, a, b, c, d) = tokio::join!(found, alone("a"), alone("b"), alone("c"), alone("d"));
    let first = first.expect("the account that names the organization is the first");
    assert!(
        store.get_user_by_id(&first).await.expect("read").is_admin,
        "and administers the instance"
    );
    for user_id in [a, b, c, d].into_iter().flatten() {
        let stored = store.get_user_by_id(&user_id).await.expect("read");
        assert!(!stored.is_admin, "{}", stored.username);
        let spaces = listed(store, &user_id).await;
        assert_eq!(spaces.len(), 1, "{}: {spaces:?}", stored.username);
        assert_eq!(spaces[0].1, OrgKind::Personal, "{}", stored.username);
    }
    test.teardown().await;
}

/// What an account that started an organization has of it: the workspace listed
/// before its own space, as the administrator of it and of the organization; and
/// nothing of the instance.
async fn check_started(
    test: &common::TestStore,
    user_id: &str,
    started: &StartedOrganization,
    own: &OwnSpace,
) {
    let store = &test.store;
    let spaces = store
        .workspaces_of_user(user_id, fvoci_store::Making::with_password())
        .await
        .expect("listed");
    let [theirs, own_listed] = &spaces[..] else {
        panic!("the workspace of the organization and their own: {spaces:?}");
    };
    assert_eq!(
        (theirs.id.as_str(), theirs.org_id.as_str()),
        (started.workspace_id.as_str(), started.org_id.as_str()),
        "the workspace that was started"
    );
    assert_eq!(
        (theirs.kind, theirs.role, theirs.name.as_str()),
        (OrgKind::Shared, WorkspaceRole::Admin, "Theirs"),
        "of an organization of people, under its name, administered by who started it"
    );
    assert_eq!(
        own_listed.id, own.workspace_id,
        "their own space, as it was"
    );
    assert_eq!(
        store
            .org_role_of_user(user_id, &started.org_id)
            .await
            .expect("read"),
        Some(OrgRole::Admin),
        "who starts an organization administers it"
    );
    assert!(
        !store.get_user_by_id(user_id).await.expect("read").is_admin,
        "who starts an organization later administers no instance"
    );
    assert_eq!(
        test.instance_organization().await.expect("read"),
        Some(test.org_id.clone()),
        "the instance's own organization is the one it was"
    );
}

/// The most organizations an account makes, as the tests ask for it.
const LIMIT: u32 = 3;

/// An account that is there starts an organization: the organization and its first
/// workspace under the name, the account the administrator of both, its own space
/// beside them as before. The ones in use that an account made are counted, the one
/// it signed up with among them, and not one it was let into; at the limit it starts
/// no other, and one that is deleted makes room. The instance's own organization and
/// administrators are who they were.
#[tokio::test]
async fn an_account_starts_as_many_organizations_as_the_limit() {
    let test = common::setup().await;
    let store = &test.store;
    let mut first = account("first");
    store
        .sign_up(&mut first, "Ours", Founding::First)
        .await
        .expect("the first account");
    let member = common::user(store, "member").await;
    let mut alone = account("alone");
    let own = store.sign_up_alone(&mut alone).await.expect("alone");
    let organizations = rows(store, "organizations").await;

    if common::db_type() == DbType::Sqlite {
        // The one organization of an SQLite database is there: nobody starts another,
        // and nothing of the try stays.
        let refused = store.start_organization(&alone.id, "Theirs", LIMIT).await;
        assert!(
            matches!(refused, Err(StoreError::Database(_))),
            "{refused:?}"
        );
        assert_eq!(rows(store, "organizations").await, organizations);
        assert_eq!(listed(store, &alone.id).await.len(), 1);
        test.teardown().await;
        return;
    }

    let nobody = store.start_organization(&new_id(), "Nobody's", LIMIT).await;
    assert!(
        matches!(nobody, Err(StoreError::UserNotFound(_))),
        "{nobody:?}"
    );
    assert_eq!(rows(store, "organizations").await, organizations);

    // Who was let into an organization made none; who started nothing made none.
    let started = store
        .start_organization(&alone.id, "Theirs", LIMIT)
        .await
        .expect("the first of an account that made none");
    check_started(&test, &alone.id, &started, &own).await;
    for name in ["First", "Second", "Third", "Fourth"] {
        let made = store.start_organization(&member, name, LIMIT).await;
        assert_eq!(made.is_ok(), name != "Fourth", "{name}: {made:?}");
        if name == "Fourth" {
            assert!(
                matches!(made, Err(StoreError::TooManyOrganizations { limit: LIMIT })),
                "{made:?}"
            );
        }
    }
    // The one an account signed up with is one it made: two more, and no third.
    let mut theirs = Vec::new();
    for name in ["A", "B"] {
        let made = store.start_organization(&first.id, name, LIMIT).await;
        theirs.push(made.expect("room for it").org_id);
    }
    let full = store.start_organization(&first.id, "C", LIMIT).await;
    assert!(
        matches!(full, Err(StoreError::TooManyOrganizations { limit: LIMIT })),
        "{full:?}"
    );
    assert_eq!(rows(store, "organizations").await, organizations + 6);
    // One that is deleted is not counted.
    let delete = format!(
        "UPDATE organizations SET deleted_at = 1 WHERE id = '{}'",
        theirs[0]
    );
    common::execute_raw(&test.database_connection(), &delete).await;
    store
        .start_organization(&first.id, "C", LIMIT)
        .await
        .expect("room again");
    test.teardown().await;
}

/// Of several starts at once for one account no more are made than the limit leaves
/// room for; and who made an organization is forgotten with their account, while the
/// organization stays whose it is.
#[tokio::test]
async fn starts_at_once_keep_the_limit_and_a_maker_goes_with_their_account() {
    if common::db_type() != DbType::Postgres {
        return;
    }
    let test = common::setup().await;
    let store = &test.store;
    let mut first = account("first");
    store
        .sign_up(&mut first, "Ours", Founding::First)
        .await
        .expect("the first account");
    let mut alone = account("alone");
    store.sign_up_alone(&mut alone).await.expect("alone");

    let barrier = Arc::new(Barrier::new(4));
    let start = async |name: &'static str| {
        barrier.wait().await;
        store.start_organization(&alone.id, name, 2).await
    };
    let (a, b, c, d) = tokio::join!(start("A"), start("B"), start("C"), start("D"));
    let mut made = Vec::new();
    for result in [a, b, c, d] {
        match result {
            Ok(started) => made.push(started.org_id),
            Err(StoreError::TooManyOrganizations { limit: 2 }) => {}
            Err(error) => panic!("made or refused over the limit: {error}"),
        }
    }
    assert_eq!(made.len(), 2, "as many as the limit: {made:?}");
    let makers = |user_id: &str| {
        format!("SELECT CAST(id AS TEXT) FROM organizations WHERE created_by = '{user_id}'")
    };
    let system = test.system();
    let mut of_alone = system
        .texts_unchecked(&makers(&alone.id))
        .await
        .expect("read");
    of_alone.sort();
    made.sort();
    assert_eq!(of_alone, made, "the ones it made say so");
    assert_eq!(
        system
            .texts_unchecked(&makers(&first.id))
            .await
            .expect("read"),
        std::slice::from_ref(&test.org_id),
        "the first account made the one it took"
    );

    // The account goes: the organizations it made stay, and name nobody.
    for org_id in &made {
        let leave = format!(
            "DELETE FROM workspace_members WHERE org_id = '{org_id}'; \
             DELETE FROM organization_members WHERE org_id = '{org_id}'"
        );
        common::execute_raw(&test.database_connection(), &leave).await;
    }
    common::delete_user_row(&test, &alone.id).await;
    assert_eq!(
        system
            .texts_unchecked(&makers(&alone.id))
            .await
            .expect("read"),
        Vec::<String>::new()
    );
    let kept = format!(
        "SELECT CAST(id AS TEXT) FROM organizations WHERE created_by IS NULL AND id IN ('{}', '{}')",
        made[0], made[1]
    );
    assert_eq!(system.texts_unchecked(&kept).await.expect("read").len(), 2);
    test.teardown().await;
}

/// A space has its owner in it and nobody else, and no link into it, whatever the
/// server does: the schema refuses a second member of its organization or of its
/// workspace, and an invitation, on both databases.
#[tokio::test]
async fn the_schema_lets_nobody_but_its_owner_into_a_space() {
    let test = common::setup().await;
    let store = &test.store;
    let owner = common::user(store, "owner").await;
    let other = common::user(store, "other").await;
    let system = test.system();
    let space = "SELECT CAST(id AS TEXT) FROM organizations WHERE owner_id =";
    let org_id = system
        .texts_unchecked(&format!("{space} '{owner}'"))
        .await
        .expect("read")
        .remove(0);
    let workspace_id = common::own_workspace(&test, &owner).await;
    let connection = test.database_connection();
    let workspace_org = match common::db_type() {
        DbType::Postgres => ("org_id, ", format!("'{org_id}', ")),
        DbType::Sqlite => ("", String::new()),
    };
    let statements = [
        format!(
            "INSERT INTO organization_members (org_id, user_id, role, created_at) \
             VALUES ('{org_id}', '{other}', 'member', 1)"
        ),
        format!(
            "INSERT INTO workspace_members (workspace_id, {}user_id, role, created_at) \
             VALUES ('{workspace_id}', {}'{other}', 'guest', 1)",
            workspace_org.0, workspace_org.1
        ),
        format!("UPDATE organization_members SET user_id = '{other}' WHERE org_id = '{org_id}'"),
        format!(
            "UPDATE workspace_members SET user_id = '{other}' \
             WHERE workspace_id = '{workspace_id}'"
        ),
        format!(
            "INSERT INTO invitations (id, org_id, workspace_id, role, token_hash, created_by, \
             created_at, expires_at) VALUES ('{}', '{org_id}', '{workspace_id}', 'member', \
             'hash', '{owner}', 1, 2)",
            new_id()
        ),
    ];
    for statement in &statements {
        let refused = common::try_execute_raw(&connection, statement).await;
        let error = refused.expect_err(statement).to_string();
        assert!(
            error.contains("the space of an account") || error.contains("invited into the space"),
            "{statement}: {error}"
        );
    }
    assert_eq!(
        common::own_space_shape(&system, &owner).await,
        common::own_space(),
        "the space is what it was"
    );
    test.teardown().await;
}

/// No invitation names a space, by its organization, its workspace or a board of it,
/// made so or changed to: one that says an organization people work in and leads
/// into a space would let somebody in past the organization's own check.
#[tokio::test]
async fn no_invitation_names_a_space_by_any_of_its_keys() {
    let test = common::setup().await;
    let store = &test.store;
    let owner = common::user(store, "owner").await;
    let system = test.system();
    let space = "SELECT CAST(id AS TEXT) FROM organizations WHERE owner_id =";
    let org_id = system
        .texts_unchecked(&format!("{space} '{owner}'"))
        .await
        .expect("read")
        .remove(0);
    let workspace_id = common::own_workspace(&test, &owner).await;
    let connection = test.database_connection();
    let link = NewLink {
        workspace_id: &test.workspace_id,
        guest_of: None,
        token_hash: "a link of the organization",
        created_by: &owner,
        now: 0,
        expires_at: i64::MAX,
        max_uses: None,
        mailed: None,
    };
    let link = store.create_invitation(&link).await.expect("a link");
    let in_space = store.for_org(&org_id).expect("an identifier");
    let mut board = fvoci_model::Container {
        id: new_id(),
        workspace_id: workspace_id.clone(),
        ..fvoci_model::Container::default()
    };
    in_space
        .insert_container_with_admin(&mut board, &owner)
        .await
        .expect("a board of the space");
    let columns = "id, org_id, workspace_id, role, container_id, container_role, token_hash, \
                   created_by, created_at, expires_at";
    let insert = |org: &str, workspace: &str, guest_of: Option<&str>, hash: &str| {
        let (role, container) = match guest_of {
            Some(board) => ("guest", format!("'{board}', 'viewer'")),
            None => ("member", "NULL, NULL".to_owned()),
        };
        format!(
            "INSERT INTO invitations ({columns}) VALUES ('{}', '{org}', '{workspace}', \
             '{role}', {container}, '{hash}', '{owner}', 1, 2)",
            new_id()
        )
    };
    let update = |set: &str| format!("UPDATE invitations SET {set} WHERE id = '{}'", link.id);
    let (shared, shared_workspace) = (&test.org_id, &test.workspace_id);
    let board = &board.id;
    // Each statement names the space by one thing alone, so that each part of the
    // triggers refuses one of its own. Where PostgreSQL's keys of the organization
    // with the workspace, and with the board, refuse before its trigger is asked,
    // the refusal is the key's.
    let statements = [
        (
            insert(&org_id, shared_workspace, None, "by the organization"),
            false,
        ),
        (
            insert(shared, &workspace_id, None, "by the workspace"),
            true,
        ),
        (
            insert(shared, shared_workspace, Some(board), "by the board"),
            true,
        ),
        (update(&format!("org_id = '{org_id}'")), false),
        (update(&format!("workspace_id = '{workspace_id}'")), true),
        (
            update(&format!(
                "role = 'guest', container_id = '{board}', container_role = 'viewer'"
            )),
            true,
        ),
        (
            update(&format!(
                "org_id = '{org_id}', workspace_id = '{workspace_id}'"
            )),
            false,
        ),
    ];
    for (statement, by_the_key) in &statements {
        let refused = common::try_execute_raw(&connection, statement).await;
        let error = refused.expect_err(statement).to_string();
        let refusal = if *by_the_key && common::db_type() == DbType::Postgres {
            "foreign key"
        } else {
            "invited into the space"
        };
        assert!(error.contains(refusal), "{statement}: {error}");
    }
    assert_eq!(
        common::own_space_shape(&system, &owner).await,
        common::own_space(),
        "the space is what it was"
    );
    test.teardown().await;
}

/// The doors of the store that let people in say what the schema says of a space: no
/// link into it, no second member, and no administrator who joins it, from its own
/// organization's handle as from the one the other account works in.
#[tokio::test]
async fn the_doors_of_the_store_let_nobody_into_a_space() {
    let test = common::setup().await;
    let store = &test.store;
    let owner = common::user(store, "owner").await;
    let other = common::user(store, "other").await;
    let system = test.system();
    let space = "SELECT CAST(id AS TEXT) FROM organizations WHERE owner_id =";
    let org_id = system
        .texts_unchecked(&format!("{space} '{owner}'"))
        .await
        .expect("read")
        .remove(0);
    let workspace_id = common::own_workspace(&test, &owner).await;
    let in_space = store.for_org(&org_id).expect("an identifier");
    let link = NewLink {
        workspace_id: &workspace_id,
        guest_of: None,
        token_hash: "a link into a space",
        created_by: &owner,
        now: 0,
        expires_at: i64::MAX,
        max_uses: None,
        mailed: None,
    };
    let invited = in_space.create_invitation(&link).await;
    assert!(
        matches!(
            &invited,
            Err(StoreError::Database(error))
                if error.to_string().contains("invited into the space")
        ),
        "a link into a space: {invited:?}"
    );
    for role in [WorkspaceRole::Member, WorkspaceRole::Guest] {
        let added = in_space
            .add_workspace_member(&workspace_id, &other, role)
            .await;
        assert!(
            matches!(
                &added,
                Err(StoreError::Database(error))
                    if error.to_string().contains("the space of an account")
            ),
            "a second member of a space: {added:?}"
        );
    }
    let to_system = store
        .create_invitation(&NewLink {
            workspace_id: fvoci_store::SYSTEM_WORKSPACE_ID,
            token_hash: "a link into the system workspace",
            ..link
        })
        .await;
    assert!(
        matches!(to_system, Err(StoreError::NotFound { .. })),
        "the store refuses links into the system workspace before the schema: {to_system:?}"
    );
    for handle in [&in_space, store] {
        let joined = handle
            .join_workspace_as_org_admin(&workspace_id, &other)
            .await
            .expect("the joining is answered");
        assert!(
            matches!(joined, Joining::NoOrgAdmin | Joining::NotFound),
            "an administrator joins a space: {joined:?}"
        );
    }
    assert_eq!(
        common::own_space_shape(&system, &owner).await,
        common::own_space(),
        "the space is what it was"
    );
    test.teardown().await;
}

fn signup(username: &str, start: SignupStart, now: i64) -> Signup {
    Signup {
        id: new_id(),
        username: username.to_owned(),
        email: format!("{username}@example.com"),
        custom_name: String::new(),
        password_hash: "hash".to_owned(),
        language: String::new(),
        start,
        browser_hash: String::new(),
        created_at: now,
        expires_at: now + SIGNUP_LIFETIME_MILLIS,
    }
}

/// A registration that starts nothing waits for its address as one that starts an
/// organization does, and is read back as what it is. Confirmed, it is the account
/// and its own space; confirmed as the other kind it is nothing; and it goes with the
/// registrations of their own when a server of one organization has its first user.
#[tokio::test]
async fn a_registration_that_starts_nothing_waits_and_is_confirmed_as_one() {
    let test = common::setup().await;
    let store = &test.store;
    let now = 1_000_000;
    let waiting = signup("waits", SignupStart::Alone, now);
    store
        .create_signup(&waiting, None, now)
        .await
        .expect("stored");
    let read = store
        .signup_of(Proof::Operator("waits"), now)
        .await
        .expect("read")
        .expect("the registration");
    assert_eq!(read.start, SignupStart::Alone);
    let named = signup("names", SignupStart::Organization("Ours".to_owned()), now);
    store
        .create_signup(&named, None, now)
        .await
        .expect("stored");

    // Neither is confirmed as the other kind, and neither is confirmed alone before
    // the instance has its first user.
    let proof = Proof::Operator("waits");
    let as_founding = store
        .confirm_signup(&read, proof, Founding::First, now)
        .await;
    assert!(
        matches!(as_founding, Err(StoreError::NotFound(_))),
        "{as_founding:?}"
    );
    let as_alone = store
        .confirm_signup_alone(&named, Proof::Operator("names"), now)
        .await;
    assert!(
        matches!(as_alone, Err(StoreError::NotFound(_))),
        "{as_alone:?}"
    );
    let early = store.confirm_signup_alone(&read, proof, now).await;
    assert!(matches!(early, Err(StoreError::NoFirstUser)), "{early:?}");
    assert!(
        store.signup_of(proof, now).await.expect("read").is_some(),
        "the registration stays"
    );

    let first = store
        .confirm_signup(&named, Proof::Operator("names"), Founding::First, now)
        .await
        .expect("confirmed")
        .expect("the registration was there");
    assert_eq!(first.start, Started::Organization);
    let confirmed = store
        .confirm_signup_alone(&read, proof, now)
        .await
        .expect("confirmed")
        .expect("the registration was there");
    assert_eq!(confirmed.start, Started::Alone);
    assert_eq!(confirmed.user.id, waiting.id, "under its identifier");
    assert_eq!(confirmed.container_id, None);
    assert_eq!(
        confirmed.workspace_id,
        common::own_workspace(&test, &waiting.id).await
    );
    assert_eq!(
        listed(store, &waiting.id).await,
        [(
            confirmed.org_id.clone(),
            OrgKind::Personal,
            WorkspaceRole::Admin
        )]
    );
    let again = store.confirm_signup_alone(&read, proof, now).await;
    assert_eq!(again.expect("asked"), None, "once");

    // On a server of one organization the registrations of their own go once it has
    // its first user: the ones that start nothing with the ones that name one.
    for (name, start) in [
        ("later", SignupStart::Alone),
        ("another", SignupStart::Organization("Theirs".to_owned())),
    ] {
        let waiting = signup(name, start, now);
        store
            .create_signup(&waiting, None, now)
            .await
            .expect("stored");
    }
    assert_eq!(store.remove_founding_signups().await.expect("removed"), 2);
    test.teardown().await;
}
