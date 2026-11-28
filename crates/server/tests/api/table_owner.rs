//! A server that connects to PostgreSQL as a role that does not own the tables: their
//! owner, the database's administrator, brings the schema up to date with
//! `fvoci-server --migrate` (`fvoci_server::app::migrate_database`), and the server only
//! uses them. It serves; it does not start on a schema that is behind it, or on one
//! whose record is the one before the baseline, and says what to run; the migration
//! takes the lock of the server, as a start does, but the replacing of the record does
//! not: it leaves the schema as it is, while a server serves it, and the migrations
//! after the baseline wait for the lock.

use std::path::PathBuf;

use fvoci_model::{Card, Container, ContainerVisibility};
use fvoci_server::Server;
use fvoci_server::config::Configuration;
use fvoci_store::testing::{
    TableOwner, TestDatabase, db_type_of_the_tests, forget_migration,
    write_the_record_before_the_baseline,
};
use fvoci_store::{DbType, SchemaCheck, Store};

use crate::boards::new_board;
use crate::common::{Setup, TEST_WORKSPACE_ID, TEST_WS};
use crate::content_search::new_card;

/// A board, and a card with a body, through the API of a server whose role does not own
/// the tables, whoever owns them.
#[tokio::test]
async fn a_server_serves_tables_its_role_does_not_own() {
    for owner in [TableOwner::Superuser, TableOwner::Role] {
        let th = Setup::one_user()
            .connecting_as_a_role_that_does_not_own_the_tables(owner)
            .start()
            .await;
        let board: Container = th
            .client
            .create_board(&new_board(
                TEST_WORKSPACE_ID,
                ContainerVisibility::Open,
                "Apart",
            ))
            .await
            .created_json();
        let card = new_card(
            &th.client,
            &board.id,
            "Owned apart",
            "written by the server",
        )
        .await;
        let read: Card = th
            .client
            .get(&format!("{TEST_WS}/cards/{}", card.id))
            .await
            .ok_json();
        assert_eq!(read.title, "Owned apart", "{owner:?}");
        th.teardown().await;
    }
}

/// A server whose role does not own the tables does not start on a schema that is
/// behind it, or before the baseline, and names the migration to run; the owner's
/// migration takes the lock of the server, and the server then starts. While it serves,
/// a migration of a schema that is up to date does nothing, the record before the
/// baseline is replaced and what is after the baseline refused, and a schema behind is
/// refused. Whoever owns the tables.
#[tokio::test]
async fn a_server_waits_for_the_owner_to_migrate_a_schema_behind_it() {
    if db_type_of_the_tests() != DbType::Postgres {
        return;
    }
    for owner in [TableOwner::Superuser, TableOwner::Role] {
        the_owner_migrates_while_no_server_serves(owner).await;
    }
}

async fn the_owner_migrates_while_no_server_serves(owner: TableOwner) {
    let directory = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("api-tests")
        .join(fvoci_model::new_id());
    let database = TestDatabase::create_owned_apart(&directory, owner).await;
    let config = |database_connection: &str| Configuration {
        database_kind: database.db_type().as_str().to_owned(),
        database_connection: database_connection.to_owned(),
        file_storage_path: directory.join("uploads").display().to_string(),
        skip_template_init: true,
        server_lock_wait_seconds: 0,
        ..Configuration::default()
    };

    let Err(refused) = Server::new(config(database.database_connection()), "").await else {
        panic!("the server started on a schema behind it");
    };
    let refusal = format!("{refused:#}");
    assert!(
        refusal.contains("fvoci-server --migrate"),
        "{owner:?}: {refusal}"
    );

    fvoci_server::app::migrate_database(&config(database.owner_database_connection()))
        .await
        .expect("the owner brings the schema up to date");

    // As the last server before the baseline left it: refused to the server's role too.
    write_the_record_before_the_baseline(database.db_type(), database.owner_database_connection())
        .await;
    let Err(refused) = Server::new(config(database.database_connection()), "").await else {
        panic!("the server started on a record before the baseline");
    };
    let refusal = format!("{refused:#}");
    assert!(
        refusal.contains("fvoci-server --migrate") && refusal.contains("before the baseline"),
        "{owner:?}: {refusal}"
    );
    fvoci_server::app::check_database(&config(database.owner_database_connection()))
        .await
        .expect("the owner may replace it, the check says, changing nothing");
    fvoci_server::app::migrate_database(&config(database.owner_database_connection()))
        .await
        .expect("the owner replaces the record");
    let server = Server::new(config(database.database_connection()), "")
        .await
        .expect("then the server starts");

    // Up to date, a migration leaves the schema alone while the server serves it, as a
    // second `up` of the compose file asks for one.
    fvoci_server::app::migrate_database(&config(database.owner_database_connection()))
        .await
        .expect("nothing to do while the server serves the database");
    // Before the baseline, the record is replaced without the lock; the migrations
    // after the baseline, which change the schema, wait for the lock the server holds,
    // and give up.
    write_the_record_before_the_baseline(database.db_type(), database.owner_database_connection())
        .await;
    the_migration_waits_for_the_server(&config(database.owner_database_connection()), owner).await;
    let store = Store::open(database.db_type(), database.owner_database_connection())
        .await
        .expect("the owner connects");
    assert_eq!(
        store.check_schema().await.expect("checked"),
        SchemaCheck::Behind,
        "{owner:?}: the record is the baseline's, the migrations after it to apply"
    );
    store.close().await;
    // Behind, it waits for the lock the server holds, and gives up.
    forget_migration(database.db_type(), database.owner_database_connection(), 1).await;
    the_migration_waits_for_the_server(&config(database.owner_database_connection()), owner).await;
    server.shutdown().await;
    database.remove().await;
}

/// The owner's migration of a schema behind waits for the lock of the server that
/// serves the database, and gives up, saying what to do.
async fn the_migration_waits_for_the_server(config: &Configuration, owner: TableOwner) {
    let busy = fvoci_server::app::migrate_database(config)
        .await
        .expect_err("no migration while a server serves the database");
    let said = format!("{busy:#}");
    assert!(
        said.contains("another process is the server") && said.contains("stop it"),
        "{owner:?}: {said}"
    );
}
