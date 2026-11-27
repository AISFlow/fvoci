//! The wall between organizations as the store's operations meet it: what one
//! organization does not read or change of another's, what is asked of every
//! organization, and the organizations a database holds. What PostgreSQL's row-level
//! security keeps apart whatever a query asks for; SQLite has one organization and no
//! such wall.

use fvoci_model::{
    Container, ContainerKind, ContainerMember, ContainerVisibility, PublicLinkChange, Upload,
    Workspace, WorkspaceRole, is_id, new_id,
};
use fvoci_store::{
    DbType, SYSTEM_ORG_ID, SYSTEM_WORKSPACE_ID, SharedContainer, Store, StoreError, Tenant,
};

use crate::common;

/// Stores an open board of the workspace by the user through `store` and returns its
/// identifier.
pub(crate) async fn board(store: &Store, workspace_id: &str, user_id: &str, title: &str) -> String {
    let mut board = Container {
        id: new_id(),
        workspace_id: workspace_id.to_owned(),
        visibility: ContainerVisibility::Open,
        title: title.to_owned(),
        ..Container::default()
    };
    store
        .insert_container(&mut board, user_id)
        .await
        .expect("the board is stored");
    board.id
}

/// What two organizations store in one table stays apart, whatever the statement asks
/// for: each reads its own rows, a handle that names no organization reads none and
/// stores none, the system's handle reads all, and nobody stores a row as another's.
#[tokio::test]
async fn the_boards_of_two_organizations_are_out_of_each_others_reach() {
    if common::db_type() != DbType::Postgres {
        return;
    }
    let test = common::setup().await;
    let unset = test.scoped(Tenant::Unset);
    let (other_org, their_workspace) = common::organization(&unset, "the other").await;
    let ours = &test.store;
    let theirs = test.for_org(&other_org).expect("an identifier");

    let user_id = common::user(ours, "anybody").await;
    let workspace_id = common::workspace(ours).await;
    let our_board = board(ours, &workspace_id, &user_id, "ours").await;
    let their_board = board(&theirs, &their_workspace, &user_id, "theirs").await;
    board(&theirs, &their_workspace, &user_id, "theirs too").await;

    let count = async |store: &Store| {
        store
            .count_rows("containers")
            .await
            .expect("the boards are counted")
    };
    assert_eq!(count(ours).await, 1, "ours");
    assert_eq!(count(&theirs).await, 2, "theirs");
    assert_eq!(count(&unset).await, 0, "no organization named");
    assert_eq!(count(&test.system()).await, 3, "the system's handle");

    // By its identifier too: what is another's is not there.
    assert_eq!(
        ours.get_container(&our_board).await.expect("ours").title,
        "ours"
    );
    let missing = ours
        .get_container(&their_board)
        .await
        .expect_err("their board is out of reach");
    assert!(missing.is_not_found(), "got {missing:?}");
    // Nor is it changed or deleted: the statement finds no row.
    let changed = ours
        .execute_unchecked(&format!(
            "UPDATE containers SET title = 'taken' WHERE id = '{their_board}'"
        ))
        .await
        .expect("the statement runs");
    assert_eq!(changed, 0, "an update of their board");
    let deleted = ours
        .execute_unchecked("DELETE FROM containers")
        .await
        .expect("the statement runs");
    assert_eq!(deleted, 1, "a delete of everything deletes ours");
    assert_eq!(count(&theirs).await, 2, "and leaves theirs");

    // A row is stored as the organization's own or not at all.
    // In their workspace: a board is of its workspace's organization, which the schema holds it
    // to apart from the wall.
    let insert = |org: &str| {
        format!(
            "INSERT INTO containers (id, org_id, workspace_id, visibility, title, description, icon, \
             show_description, is_template, properties, card_properties, created_at, updated_at) \
             VALUES ('{}', {org}, '{their_workspace}', 'open', '', '', '', false, false, '{{}}', '[]', 0, 0)",
            new_id()
        )
    };
    let as_theirs = ours
        .execute_unchecked(&insert(&format!("'{other_org}'")))
        .await
        .expect_err("a row of another organization is refused");
    assert!(
        as_theirs.to_string().contains("row-level security"),
        "{as_theirs}"
    );
    let as_nobody = unset
        .execute_unchecked(&insert("DEFAULT"))
        .await
        .expect_err("a row of no organization is refused");
    // The policy refuses it before the column does.
    assert!(
        as_nobody.to_string().contains("row-level security"),
        "{as_nobody}"
    );
    // The system's handle stores for whom it says, and for nobody if it does not say.
    test.system()
        .execute_unchecked(&insert(&format!("'{other_org}'")))
        .await
        .expect("the system stores a row of an organization it names");
    let unnamed = test
        .system()
        .execute_unchecked(&insert("DEFAULT"))
        .await
        .expect_err("the system names no organization by itself");
    assert!(unnamed.to_string().contains("org_id"), "{unnamed}");
    assert_eq!(count(&theirs).await, 3, "theirs, with the system's row");

    test.teardown().await;
}

/// The organization a transaction reaches is the transaction's and no longer: a
/// connection that goes back to the pool carries nothing of it to the next operation,
/// whichever handle that is of.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nothing_of_an_organization_stays_on_a_pooled_connection() {
    if common::db_type() != DbType::Postgres {
        return;
    }
    let test = common::setup().await;
    let unset = test.scoped(Tenant::Unset);
    let (other_org, their_workspace) = common::organization(&unset, "the other").await;
    let theirs = test.for_org(&other_org).expect("an identifier");
    let user_id = common::user(&test, "anybody").await;
    let workspace_id = common::workspace(&test).await;
    board(&test, &workspace_id, &user_id, "ours").await;
    board(&theirs, &their_workspace, &user_id, "theirs").await;
    board(&theirs, &their_workspace, &user_id, "theirs too").await;

    // Many more operations than the pool has connections, of every handle at once:
    // every connection serves each of them in turn.
    let mut reads = tokio::task::JoinSet::new();
    for round in 0..40 {
        for (store, expected) in [
            (test.store.clone(), 1),
            (theirs.clone(), 2),
            (unset.clone(), 0),
            (test.system(), 3),
        ] {
            reads.spawn(async move {
                let counted = store.count_rows("containers").await.expect("counted");
                (round, store.tenant(), counted, expected)
            });
        }
    }
    while let Some(read) = reads.join_next().await {
        let (round, tenant, counted, expected) = read.expect("the read ends");
        assert_eq!(counted, expected, "round {round}, {tenant:?}");
    }

    // And the connections themselves, looked at past the store's transactions, which
    // would set the organization anew: none carries one, or the system's reach.
    let left = test.settings_left_on_the_connections().await;
    assert!(!left.is_empty(), "the pool has connections to look at");
    for (org, system) in &left {
        assert_eq!(org, "", "an organization stayed on a connection: {left:?}");
        assert!(
            system.is_empty() || system == "off",
            "the system's reach stayed on a connection: {left:?}"
        );
    }

    test.teardown().await;
}

/// A workspace that is not in use is nobody's to work in, a member's neither: the
/// system's workspace, a workspace in the trash, and the workspaces of an organization
/// that was deleted are not found and not listed.
#[tokio::test]
async fn a_member_does_not_find_a_workspace_that_is_not_in_use() {
    let test = common::setup().await;
    let unset = test.scoped(Tenant::Unset);
    let user_id = common::user(&test, "member").await;
    let second_workspace = common::workspace(&test).await;
    test.add_workspace_member(&second_workspace, &user_id, WorkspaceRole::Member)
        .await
        .expect("the user is let into the second workspace");
    test.for_org(SYSTEM_ORG_ID)
        .expect("an identifier")
        .add_workspace_member(SYSTEM_WORKSPACE_ID, &user_id, WorkspaceRole::Member)
        .await
        .expect("a membership of the system's workspace is stored");

    let found = async |workspace_id: &str| {
        unset
            .resolve_workspace(&user_id, workspace_id)
            .await
            .expect("the workspace is resolved")
            .is_some()
    };
    let listed = async || {
        unset
            .workspaces_of_user(&user_id, fvoci_store::Making::with_password())
            .await
            .expect("the workspaces are listed")
            .into_iter()
            .map(|workspace| workspace.id)
            .collect::<Vec<String>>()
    };
    assert!(!found(SYSTEM_WORKSPACE_ID).await, "the system's workspace");
    let own = common::own_workspace(&test, &user_id).await;
    assert_eq!(
        listed().await,
        [
            test.workspace_id.clone(),
            second_workspace.clone(),
            own.clone()
        ],
        "the two of the organization and the user's own last, and not the system's"
    );

    let system = test.system();
    let trashed = system
        .execute_unchecked(&format!(
            "UPDATE workspaces SET deleted_at = 5 WHERE id = '{second_workspace}'"
        ))
        .await
        .expect("the workspace is put in the trash");
    assert_eq!(trashed, 1, "the second workspace");
    assert!(!found(&second_workspace).await, "a workspace in the trash");
    assert!(found(&test.workspace_id).await, "the other is still there");
    assert_eq!(
        listed().await,
        [test.workspace_id.clone(), own.clone()],
        "without the one in the trash"
    );

    let deleted = system
        .execute_unchecked(&format!(
            "UPDATE organizations SET deleted_at = 5 WHERE id = '{}'",
            test.org_id
        ))
        .await
        .expect("the organization is deleted");
    assert_eq!(deleted, 1, "the organization");
    assert!(
        !found(&test.workspace_id).await,
        "of a deleted organization"
    );
    assert_eq!(
        listed().await,
        std::slice::from_ref(&own),
        "nothing of it is listed: the user's own space is another organization"
    );

    test.teardown().await;
}

/// The schema makes the system organization and its workspace under the identifiers
/// the server knows, and nothing lists them among an organization's own.
#[tokio::test]
async fn the_schema_makes_the_system_organization_and_its_workspace() {
    let test = common::setup().await;
    assert!(
        is_id(SYSTEM_ORG_ID) && is_id(SYSTEM_WORKSPACE_ID),
        "identifiers"
    );
    let system_org = test.for_org(SYSTEM_ORG_ID).expect("an identifier");
    let workspaces = system_org.get_workspaces().await.expect("the workspaces");
    let ids: Vec<&str> = workspaces
        .iter()
        .map(|workspace| workspace.id.as_str())
        .collect();
    assert_eq!(ids, [SYSTEM_WORKSPACE_ID], "the system organization's one");

    let ours = test.get_workspaces().await.expect("the workspaces");
    assert!(
        ours.iter()
            .all(|workspace| workspace.id != SYSTEM_WORKSPACE_ID),
        "it is not ours: {ours:?}"
    );
    let missing = test
        .get_workspace(SYSTEM_WORKSPACE_ID)
        .await
        .expect_err("nor is it found by its identifier");
    assert!(missing.is_not_found(), "got {missing:?}");
    // It is not the organization the server works in, old as it is.
    let found = test
        .scoped(Tenant::Unset)
        .instance_organization()
        .await
        .expect("the organization")
        .expect("an existing organization");
    assert_eq!(found, test.org_id);

    test.teardown().await;
}

/// An SQLite database holds one organization besides the system's: a second is refused
/// by the database itself.
#[tokio::test]
async fn an_sqlite_database_holds_one_organization() {
    if common::db_type() != DbType::Sqlite {
        return;
    }
    let test = common::setup().await;
    let second = Workspace {
        id: new_id(),
        ..Workspace::default()
    };
    let refused = test
        .scoped(Tenant::Unset)
        .create_organization("a second", &second)
        .await
        .expect_err("a second organization is refused");
    assert!(
        matches!(&refused, StoreError::Database(error)
            if error.to_string().contains("holds one organization")),
        "got {refused:?}"
    );
    let missing = test
        .get_workspace(&second.id)
        .await
        .expect_err("nothing of it is stored");
    assert!(missing.is_not_found(), "got {missing:?}");
    let refused = common::try_execute_raw(
        &test.database_connection(),
        "UPDATE organizations SET is_system = NOT is_system",
    )
    .await
    .expect_err("an organization stays what it is");
    assert!(refused.to_string().contains("the system"), "{refused}");

    test.teardown().await;
}

/// An upload session of `size` bytes that the user opens through `store`: an archive to
/// import into a workspace of the store's organization, which is what an upload names.
async fn open_upload(store: &Store, workspace_id: &str, user_id: &str, size: i64) {
    let upload = Upload {
        id: new_id(),
        user_id: user_id.to_owned(),
        purpose: fvoci_model::UploadPurpose::Import,
        workspace_id: workspace_id.to_owned(),
        name: "upload.bin".to_owned(),
        size,
        part_size: size,
        created_at: 1,
        expires_at: i64::MAX,
        ..Upload::default()
    };
    store
        .create_upload(&upload)
        .await
        .expect("an upload is opened");
}

/// The limits on open uploads are a user's and the server's, not an organization's:
/// what they count is the same through the handle of any organization, with the
/// sessions of all of them in it.
#[tokio::test]
async fn the_open_uploads_are_counted_over_every_organization() {
    let test = common::setup().await;
    let user_id = common::user(&test, "uploader").await;
    let other_user = common::user(&test, "another.uploader").await;
    open_upload(&test, &test.workspace_id, &user_id, 10).await;
    open_upload(&test, &test.workspace_id, &other_user, 100).await;
    let mut handles = vec![test.store.clone(), test.scoped(Tenant::Unset)];
    let mut expected = (1, 10, 110);
    if common::db_type() == DbType::Postgres {
        let (other_org, their_workspace) =
            common::organization(&test.scoped(Tenant::Unset), "the other").await;
        let theirs = test.for_org(&other_org).expect("an identifier");
        open_upload(&theirs, &their_workspace, &user_id, 1000).await;
        handles.push(theirs);
        expected = (2, 1010, 1110);
    }
    for store in &handles {
        let usage = store
            .upload_usage(&user_id, 5)
            .await
            .expect("the usage is read");
        assert_eq!(
            (usage.user_sessions, usage.user_bytes, usage.server_bytes),
            expected,
            "through {:?}",
            store.tenant()
        );
    }
    test.teardown().await;
}

/// The organization of a shared board is told to whoever holds its read token, and to
/// nobody else; and nothing is shared of the system organization, or of a workspace
/// that was deleted.
#[tokio::test]
async fn the_organization_of_a_shared_board_is_for_the_holder_of_its_token() {
    let test = common::setup().await;
    let unset = test.scoped(Tenant::Unset);
    let user_id = common::user(&test, "sharer").await;
    let workspace_id = common::workspace(&test).await;
    let shared = board(&test, &workspace_id, &user_id, "shared").await;
    let not_shared = board(&test, &workspace_id, &user_id, "not shared").await;
    test.set_public_link(&shared, PublicLinkChange::On, "the-token", &user_id)
        .await
        .expect("the board is shared");
    test.set_public_link(&not_shared, PublicLinkChange::On, "another-token", &user_id)
        .await
        .expect("the other board is shared");
    test.set_public_link(&not_shared, PublicLinkChange::Off, "", &user_id)
        .await
        .expect("and shared no more");
    let system_org = test.for_org(SYSTEM_ORG_ID).expect("an identifier");
    let template = board(&system_org, SYSTEM_WORKSPACE_ID, &user_id, "a template").await;
    system_org
        .set_public_link(&template, PublicLinkChange::On, "template-token", &user_id)
        .await
        .expect("the template is shared");

    let org_of = async |board_id: &str, token: &str| {
        unset
            .shared_container(board_id, token)
            .await
            .expect("the shared board is looked up")
            .map(|shared| shared.org_id)
    };
    assert_eq!(
        org_of(&shared, "the-token").await,
        Some(test.org_id.clone())
    );
    // A wiki is shared as a board is, and said to be one: its token reads its pages.
    let mut wiki = Container {
        id: new_id(),
        workspace_id: workspace_id.clone(),
        kind: ContainerKind::Wiki,
        title: "a wiki".to_owned(),
        ..Container::default()
    };
    test.insert_container(&mut wiki, &user_id)
        .await
        .expect("the wiki is stored");
    test.set_public_link(&wiki.id, PublicLinkChange::On, "the-wiki-token", &user_id)
        .await
        .expect("the wiki is shared");
    assert_eq!(
        unset
            .shared_container(&wiki.id, "the-wiki-token")
            .await
            .expect("the shared wiki is looked up"),
        Some(SharedContainer {
            org_id: test.org_id.clone(),
            kind: ContainerKind::Wiki,
        })
    );
    assert_eq!(
        org_of(&shared, "the-wiki-token").await,
        None,
        "a wiki's token opens no board"
    );
    for (board_id, token, why) in [
        (shared.as_str(), "the-toke", "a part of the token"),
        (shared.as_str(), "", "no token"),
        (
            shared.as_str(),
            "another-token",
            "the token of another board",
        ),
        (
            not_shared.as_str(),
            "",
            "a board that is shared no more has no token",
        ),
        (not_shared.as_str(), "another-token", "nor its old one"),
        (
            template.as_str(),
            "template-token",
            "the system organization's",
        ),
        ("no identifier", "the-token", "no board"),
    ] {
        assert_eq!(org_of(board_id, token).await, None, "{why}");
    }

    // A workspace in the trash shares nothing.
    let trashed = test
        .system()
        .execute_unchecked(&format!(
            "UPDATE workspaces SET deleted_at = 5 WHERE id = '{workspace_id}'"
        ))
        .await
        .expect("the workspace is put in the trash");
    assert_eq!(trashed, 1, "the workspace");
    assert_eq!(
        org_of(&shared, "the-token").await,
        None,
        "a deleted workspace"
    );

    test.teardown().await;
}

/// An open board is open to who works in its workspace: a member of it who is no
/// guest. To a guest of the workspace and to somebody who is not in it, the board is
/// found neither among the boards of its workspace nor by a search, until they are on it.
#[tokio::test]
async fn an_open_board_is_found_by_who_works_in_its_workspace() {
    use fvoci_model::ContainerSearchField::Title;
    use fvoci_model::PublicContainers;

    let test = common::setup().await;
    let (member, guest, outsider) = (
        common::user(&test, "member").await,
        common::user(&test, "guest").await,
        common::user(&test, "outsider").await,
    );
    let workspace_id = common::workspace_of(&test, &[&member]).await;
    test.add_workspace_member(&workspace_id, &guest, WorkspaceRole::Guest)
        .await
        .expect("the guest is let into the workspace");
    let board_id = board(&test, &workspace_id, &member, "open to the workspace").await;

    let found_by = async |user_id: &str| {
        let (open, boards) = (
            PublicContainers::Include,
            fvoci_model::ContainerKinds::BOARDS,
        );
        let of_the_workspace = test
            .get_containers_for_user_and_workspace(user_id, &workspace_id, open, boards)
            .await
            .expect("the boards of the workspace are read");
        let searched = test
            .search_containers_for_user("open", Title, user_id, open, boards)
            .await
            .expect("the boards are searched");
        (of_the_workspace.len(), searched.len())
    };
    assert_eq!(
        found_by(&member).await,
        (1, 1),
        "who works in the workspace"
    );
    assert_eq!(found_by(&guest).await, (0, 0), "a guest of the workspace");
    assert_eq!(found_by(&outsider).await, (0, 0), "somebody not in it");

    // On the board, a guest finds it as any member of a board does.
    test.save_member(&ContainerMember {
        container_id: board_id,
        user_id: guest.clone(),
        role: fvoci_model::ContainerRole::Viewer,
        synthetic: false,
        inherited_role: None,
    })
    .await
    .expect("the guest is put on the board");
    assert_eq!(
        found_by(&guest).await,
        (1, 1),
        "a guest who is on the board"
    );
    test.teardown().await;
}

/// Whether an organization is the instance's own is asked in the transaction of what
/// depends on it, past the wall on PostgreSQL for that one statement: the oldest
/// organization is, a later one is not, and afterwards the transaction reaches its own
/// organization alone again.
#[tokio::test]
async fn the_instances_organization_is_asked_behind_a_wall_closed_again() {
    let test = common::setup().await;
    let own = test
        .instances_organization_and_reach()
        .await
        .expect("asked");
    // The system's organization, also on SQLite, which holds one organization besides.
    let systems = test
        .for_org(SYSTEM_ORG_ID)
        .expect("organization")
        .instances_organization_and_reach()
        .await
        .expect("asked");
    let later = if common::db_type() == DbType::Postgres {
        let (org, _) = common::organization(&test.system(), "Later").await;
        let later = test.for_org(&org).expect("organization");
        Some(
            later
                .instances_organization_and_reach()
                .await
                .expect("asked"),
        )
    } else {
        None
    };
    test.teardown().await;

    assert!(own.0, "the oldest organization is the instance's");
    assert!(!systems.0, "the system's organization is not");
    if let Some(later) = later {
        assert_eq!(
            later,
            (false, 1),
            "a later one is not, and sees itself alone"
        );
        assert_eq!(own.1, 1, "nor does the instance's see the others");
    }
}

/// A read past the wall puts the reach back as it found it: a transaction whose wall
/// was open before the read (a registration's, for the link it was made by) has it
/// open after, and one whose wall was closed has it closed (the test above).
#[tokio::test]
async fn a_read_past_the_wall_leaves_an_open_wall_open() {
    if common::db_type() != DbType::Postgres {
        return;
    }
    let test = common::setup().await;
    let reach = test
        .reach_after_asking_with_the_wall_open()
        .await
        .expect("asked");
    test.teardown().await;
    assert_eq!(reach, "on", "the wall the transaction opened is still open");
}

/// How many organizations an account made is asked by a transaction of an
/// organization, which the read opens the wall of. A handle that names none is
/// refused, on either database: it is not answered that the account made none.
#[tokio::test]
async fn the_organizations_an_account_made_are_not_counted_without_an_organization() {
    let test = common::setup().await;
    let user = common::user(&test, "maker").await;
    test.execute_unchecked(&format!(
        "UPDATE organizations SET created_by = '{user}' WHERE id = '{}'",
        test.org_id
    ))
    .await
    .expect("the organization is the user's making");
    let counted = test.organizations_made_by(&user).await;
    let of_none = [
        test.scoped(Tenant::Unset)
            .organizations_made_by(&user)
            .await,
        test.system().organizations_made_by(&user).await,
    ];
    test.teardown().await;
    assert_eq!(
        counted.expect("counted"),
        1,
        "in an organization's transaction"
    );
    for refused in of_none {
        assert!(
            matches!(refused, Err(StoreError::NoOrganization)),
            "no organization, no count: {refused:?}"
        );
    }
}
