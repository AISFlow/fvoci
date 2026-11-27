//! The setting `org_creation` and the check, at the start, of the role the server
//! connects to PostgreSQL as: a server that takes in several organizations does not
//! start as a role that the wall between them does not hold.

use fvoci_model::ContainerVisibility;
use fvoci_server::Server;
use fvoci_server::config::Configuration;
use fvoci_store::testing::{TestDatabase, db_type_of_the_tests};
use fvoci_store::{DbType, Store};

use crate::common::ROOT_WS;
use crate::common::{ROOT_WORKSPACE_ID, Setup};

fn open(config: &mut Configuration) {
    "open".clone_into(&mut config.org_creation);
}

/// Why a server with this setup does not start.
async fn refusal(setup: Setup) -> String {
    match setup.try_start().await {
        Ok(server) => {
            server.teardown().await;
            panic!("the server started");
        }
        Err(error) => format!("{error:#}"),
    }
}

/// With `org_creation` set to `open` the server does not start as a superuser, whom no
/// policy holds, and says why; as a role the policies hold, it starts.
#[tokio::test]
async fn a_server_for_several_organizations_does_not_start_past_the_wall() {
    if db_type_of_the_tests() != DbType::Postgres {
        return;
    }
    let refusal = refusal(Setup::native().connecting_as_superuser().configure(open)).await;
    assert!(
        refusal.contains("superuser") && refusal.contains("org_creation"),
        "the refusal says why: {refusal}"
    );

    let ts = Setup::native().configure(open).start().await;
    ts.teardown().await;
}

/// A start that is refused over the role leaves the database as it was: nothing is
/// made as the role that was refused, which would own it.
#[tokio::test]
async fn a_start_refused_over_the_role_makes_nothing_in_the_database() {
    if db_type_of_the_tests() != DbType::Postgres {
        return;
    }
    let directory = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("api-tests")
        .join(fvoci_model::new_id());
    let database = TestDatabase::create_as_superuser(&directory).await;
    let config = Configuration {
        database_kind: database.db_type().as_str().to_owned(),
        database_connection: database.database_connection().to_owned(),
        org_creation: "open".to_owned(),
        file_storage_path: directory.join("uploads").display().to_string(),
        skip_template_init: true,
        ..Configuration::default()
    };
    let refused = Server::new(config, "")
        .await
        .expect_err("the server does not start as a superuser");
    assert!(format!("{refused:#}").contains("superuser"), "{refused:#}");

    let store = Store::open(database.db_type(), database.database_connection())
        .await
        .expect("the database is reached");
    let no_table = store
        .count_rows("_sqlx_migrations")
        .await
        .expect_err("no migration ran");
    assert!(
        no_table.to_string().contains("_sqlx_migrations"),
        "the table is not there: {no_table}"
    );
    store.close().await;
    database.remove().await;
}

/// A role that is no superuser and has `BYPASSRLS` is past the wall like a superuser:
/// with `org_creation` set to `open` the server does not start as one either, and
/// makes nothing in the database.
#[tokio::test]
async fn a_server_for_several_organizations_does_not_start_as_a_role_with_bypassrls() {
    if db_type_of_the_tests() != DbType::Postgres {
        return;
    }
    let directory = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("api-tests")
        .join(fvoci_model::new_id());
    // The database is made for a role that is no superuser and has BYPASSRLS, which
    // the making of it checks.
    let database = TestDatabase::create_as_role_with_bypassrls(&directory).await;
    let config = |org_creation: &str| Configuration {
        database_kind: database.db_type().as_str().to_owned(),
        database_connection: database.database_connection().to_owned(),
        org_creation: org_creation.to_owned(),
        file_storage_path: directory.join("uploads").display().to_string(),
        skip_template_init: true,
        ..Configuration::default()
    };
    let refused = Server::new(config("open"), "")
        .await
        .expect_err("the server does not start as a role with BYPASSRLS");
    let refusal = format!("{refused:#}");
    assert!(
        refusal.contains("BYPASSRLS") && refusal.contains("org_creation"),
        "the refusal says why: {refusal}"
    );

    let store = Store::open(database.db_type(), database.database_connection())
        .await
        .expect("the database is reached");
    let no_table = store
        .count_rows("_sqlx_migrations")
        .await
        .expect_err("no migration ran");
    assert!(
        no_table.to_string().contains("_sqlx_migrations"),
        "the table is not there: {no_table}"
    );
    assert!(
        store
            .role_past_the_wall()
            .await
            .expect("the role is read")
            .is_some(),
        "the store names the role as one past the wall"
    );
    store.close().await;
    database.remove().await;
}

/// With the one organization of the default setting the server starts as a superuser
/// too, as a database container sets it up, and works: the wall is not there, the
/// server's own checks are.
#[tokio::test]
async fn a_server_of_one_organization_starts_past_the_wall_and_works() {
    if db_type_of_the_tests() != DbType::Postgres {
        return;
    }
    let ts = Setup::native()
        .connecting_as_superuser()
        .start()
        .await
        .init_basic()
        .await;
    let private = ts
        .create_board(ROOT_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    ts.client
        .get(&format!("{ROOT_WS}/containers/{}", private.id))
        .await
        .check_ok();
    // The second user is no member of the board: the server says so, wall or no wall.
    ts.client2
        .get(&format!("{ROOT_WS}/containers/{}", private.id))
        .await
        .check_not_found();
    ts.teardown().await;
}

/// `open` needs PostgreSQL: an SQLite database holds one organization. Any other word
/// than the two is refused on either database.
#[tokio::test]
async fn org_creation_is_one_of_two_words_and_open_needs_postgresql() {
    if db_type_of_the_tests() == DbType::Sqlite {
        let refusal = refusal(Setup::native().configure(open)).await;
        assert!(
            refusal.contains("org_creation") && refusal.contains("postgres"),
            "the refusal says why: {refusal}"
        );
    }
    let refusal = refusal(Setup::native().configure(|config| {
        "everybody".clone_into(&mut config.org_creation);
    }))
    .await;
    assert!(
        refusal.contains("org_creation") && refusal.contains("first_user_only"),
        "the refusal names the words: {refusal}"
    );
}

/// A server that starts on an empty database makes no organization: the first user who
/// signs up founds it, with one workspace named as it is, which they administer and
/// work in. A second start of the server on the same database finds that organization,
/// and makes none.
#[tokio::test]
async fn the_first_signup_makes_the_organization_and_a_second_start_finds_it() {
    let empty = Setup::native().on_an_empty_database().start().await;
    assert_eq!(
        empty
            .app
            .store()
            .instance_organization()
            .await
            .expect("the directory is read"),
        None,
        "a start makes no organization"
    );
    empty.teardown().await;

    let ts = Setup::one_user().on_an_empty_database().start().await;
    let workspaces: Vec<fvoci_model::UserWorkspace> =
        ts.client.get("/users/me/workspaces").await.ok_json();
    assert_eq!(
        workspaces
            .iter()
            .map(|workspace| workspace.kind)
            .collect::<Vec<_>>(),
        [fvoci_model::OrgKind::Shared, fvoci_model::OrgKind::Personal],
        "one workspace, and the one of the user's own space: {workspaces:?}"
    );
    assert_eq!(workspaces[0].role, fvoci_model::WorkspaceRole::Admin);
    let workspace: fvoci_model::Workspace = ts
        .client
        .get(&format!("/w/{}", workspaces[0].id))
        .await
        .ok_json();
    assert_eq!(
        workspace.title,
        crate::common::ORG_NAME,
        "named as the organization is"
    );
    // And it is there to work in.
    let board = ts
        .create_board(&workspace.id, ContainerVisibility::Private)
        .await;
    let organizations = |ts: &crate::common::TestServer| {
        let system = ts.app.store().system();
        async move {
            system
                .count_rows("organizations")
                .await
                .expect("the organizations are counted")
        }
    };
    let before = organizations(&ts).await;

    let ts = ts.killed_and_started_again(async |_| {}).await;
    assert_eq!(
        ts.app
            .store()
            .instance_organization()
            .await
            .expect("the directory is read"),
        Some(workspaces[0].org_id.clone()),
        "the second start finds the organization the first sign-up made"
    );
    assert_eq!(organizations(&ts).await, before, "and makes none");
    let again: Vec<fvoci_model::UserWorkspace> =
        ts.client.get("/users/me/workspaces").await.ok_json();
    assert_eq!(again, workspaces, "the same workspace, as it was");
    let boards: Vec<fvoci_model::Container> = ts
        .client
        .get(&format!("/w/{}/boards", workspace.id))
        .await
        .ok_json();
    assert_eq!(boards.len(), 1, "{boards:?}");
    assert_eq!(boards[0].id, board.id, "with what was made in it");
    ts.teardown().await;
}

/// The server's housekeeping reaches every organization: what is in the trash of an
/// organization the server does not work in is purged like its own. (As the one
/// organization the routes work in, a purge would not see it.)
#[tokio::test]
async fn the_housekeeping_reaches_every_organization() {
    if db_type_of_the_tests() != DbType::Postgres {
        return;
    }
    let ts = Setup::native().start().await.init_basic().await;
    let workspace = fvoci_model::Workspace {
        id: fvoci_model::new_id(),
        ..fvoci_model::Workspace::default()
    };
    let org_id = ts
        .app
        .store()
        .create_organization("another", &workspace)
        .await
        .expect("a second organization is made");
    let theirs = ts.app.store().for_org(&org_id).expect("an identifier");
    let user_id = ts.client.me().await.id;
    let mut board = fvoci_model::Container {
        id: fvoci_model::new_id(),
        workspace_id: workspace.id.clone(),
        visibility: ContainerVisibility::Open,
        ..fvoci_model::Container::default()
    };
    theirs
        .insert_container(&mut board, &user_id)
        .await
        .expect("their board is stored");
    let mut card = fvoci_model::Block {
        id: fvoci_model::new_id(),
        container_id: board.id.clone(),
        block_type: fvoci_model::BlockType::Card,
        ..fvoci_model::Block::default()
    };
    theirs
        .insert_block(&mut card, &user_id)
        .await
        .expect("their card is stored");
    theirs
        .delete_block(&card.id, &user_id)
        .await
        .expect("their card goes to the trash");
    // Long ago: past any time the trash keeps what is deleted.
    let aged = theirs
        .execute_unchecked("UPDATE deleted_blocks SET deleted_at = 1")
        .await
        .expect("the deletion is made an old one");
    assert_eq!(aged, 1, "their card in the trash");

    fvoci_server::keep_house(&ts.app).await;

    let gone = theirs
        .get_deleted_block(&card.id)
        .await
        .expect_err("their card is purged");
    assert!(gone.is_not_found(), "got {gone:?}");
    ts.teardown().await;
}
