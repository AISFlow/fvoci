//! A server that connects as a role that does not own the tables: their owner (the
//! database's administrator, a superuser or not) brings the schema up to date, and the
//! server's role only uses the tables. It opens a schema that is up to date without a
//! statement that changes it, refuses one that is behind or whose record is the one
//! before the baseline, and cannot turn the wall between organizations off.

use std::path::PathBuf;

use fvoci_store::testing::{TableOwner, TestDatabase, write_the_record_before_the_baseline};
use fvoci_store::{DbType, SYSTEM_ORG_ID, Store, StoreError};

use crate::common;

async fn owned_apart(owner: TableOwner) -> TestDatabase {
    let directory = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("store-tests");
    TestDatabase::create_owned_apart(&directory, owner).await
}

/// The owner migrates, and the server's role opens and uses the tables it brought.
async fn a_role_that_does_not_own_the_tables_uses_them(owner: TableOwner) {
    let database = owned_apart(owner).await;
    let owner_store = Store::connect(database.db_type(), database.owner_database_connection())
        .await
        .expect("the owner brings the schema up to date");
    if owner == TableOwner::Role {
        assert_eq!(
            owner_store
                .role_past_the_wall()
                .await
                .expect("the role is read"),
            None,
            "an owner that is no superuser migrated under the forced policies"
        );
    }
    // The rows every database starts with went in past the wall the tables force on
    // their owner too.
    assert_eq!(
        owner_store
            .system()
            .texts_unchecked("SELECT CAST(id AS TEXT) FROM organizations WHERE is_system")
            .await
            .expect("the organizations are read"),
        [SYSTEM_ORG_ID],
        "{owner:?}: the system's organization is there"
    );
    owner_store.close().await;

    let store = Store::connect(database.db_type(), database.database_connection())
        .await
        .expect("the server's role opens the schema as it is");
    let (org_id, workspace_id) = common::organization(&store, "apart").await;
    let store = store.for_org(&org_id).expect("an identifier");
    let user_id = common::user(&store, "someone").await;
    let boards = common::create_test_boards(&store, &workspace_id, &user_id, 2).await;
    assert_eq!(boards.len(), 2, "the role writes rows");
    store.close().await;
    database.remove().await;
}

#[tokio::test]
async fn a_role_that_does_not_own_the_tables_uses_what_a_superuser_migrated() {
    a_role_that_does_not_own_the_tables_uses_them(TableOwner::Superuser).await;
}

#[tokio::test]
async fn a_role_that_does_not_own_the_tables_uses_what_an_owner_held_by_the_wall_migrated() {
    a_role_that_does_not_own_the_tables_uses_them(TableOwner::Role).await;
}

/// A record that is not this build's (a newer server's migration) is refused to a role
/// that does not own the tables with the reason, not as a schema to bring up to date.
#[tokio::test]
async fn a_role_that_does_not_own_the_tables_is_told_why_a_record_is_not_this_builds() {
    if common::db_type() != DbType::Postgres {
        return;
    }
    let database = owned_apart(TableOwner::Superuser).await;
    Store::connect(database.db_type(), database.owner_database_connection())
        .await
        .expect("the owner brings the schema up to date")
        .close()
        .await;
    common::execute_raw(
        database.owner_database_connection(),
        "INSERT INTO _sqlx_migrations (version, description, success, checksum, \
         execution_time) VALUES (999999, 'newer', true, '\\x00', 0)",
    )
    .await;
    let refused = Store::connect(database.db_type(), database.database_connection())
        .await
        .expect_err("a newer server's record is refused");
    assert!(
        matches!(&refused, StoreError::SchemaNotOwned { reason, .. }
            if reason.contains("newer server")),
        "{refused}"
    );
    database.remove().await;
}

/// A database before the baseline is refused to a role that does not own the tables:
/// its record is to be replaced, which the owner does (`fvoci-server --migrate`), and
/// it is left as it is.
#[tokio::test]
async fn a_role_that_does_not_own_the_tables_leaves_the_record_before_the_baseline_to_the_owner() {
    if common::db_type() != DbType::Postgres {
        return;
    }
    let database = owned_apart(TableOwner::Role).await;
    Store::connect(database.db_type(), database.owner_database_connection())
        .await
        .expect("the owner brings the schema up to date")
        .close()
        .await;
    write_the_record_before_the_baseline(database.db_type(), database.owner_database_connection())
        .await;
    let refused = Store::connect(database.db_type(), database.database_connection())
        .await
        .expect_err("the server's role does not replace the record");
    assert!(
        matches!(&refused, StoreError::SchemaNotOwned { reason, .. }
            if reason.contains("before the baseline")),
        "{refused}"
    );
    let owner = Store::open(database.db_type(), database.owner_database_connection())
        .await
        .expect("the owner connects");
    assert_eq!(
        owner
            .check_schema()
            .await
            .expect("the owner may replace it"),
        fvoci_store::SchemaCheck::BeforeBaseline,
        "the record is as it was"
    );
    owner
        .migrate()
        .await
        .expect("the owner replaces the record");
    owner.close().await;
    Store::connect(database.db_type(), database.database_connection())
        .await
        .expect("then the server's role opens it")
        .close()
        .await;
    database.remove().await;
}

/// An empty schema that the server's role may not make tables in is refused the same
/// way: the owner makes them.
#[tokio::test]
async fn a_role_that_may_not_make_tables_refuses_an_empty_schema() {
    if common::db_type() != DbType::Postgres {
        return;
    }
    let database = owned_apart(TableOwner::Superuser).await;
    let refused = Store::connect(database.db_type(), database.database_connection())
        .await
        .expect_err("nothing is made as the server's role");
    assert!(
        matches!(refused, StoreError::SchemaNotOwned { .. }),
        "{refused}"
    );
    database.remove().await;
}

/// A table the server's role was not granted, as one a migration made before the
/// default privileges were set, is named at the start rather than failing requests.
#[tokio::test]
async fn a_role_that_was_not_granted_a_table_is_told_which() {
    if common::db_type() != DbType::Postgres {
        return;
    }
    let database = owned_apart(TableOwner::Superuser).await;
    Store::connect(database.db_type(), database.owner_database_connection())
        .await
        .expect("the owner brings the schema up to date")
        .close()
        .await;
    common::execute_raw(
        database.owner_database_connection(),
        "DO $$ DECLARE grantee text; BEGIN \
           FOR grantee IN SELECT DISTINCT g.grantee FROM information_schema.role_table_grants g \
             WHERE g.table_schema = current_schema() AND g.table_name = 'blocks' \
             AND g.grantee <> current_user LOOP \
             EXECUTE format('REVOKE DELETE ON blocks FROM %I', grantee); \
           END LOOP; END $$",
    )
    .await;
    let refused = Store::connect(database.db_type(), database.database_connection())
        .await
        .expect_err("a table the role may not write is refused");
    let said = refused.to_string();
    assert!(said.contains("blocks") && said.contains("grant"), "{said}");
    database.remove().await;
}

/// What the owner of the tables may do to the wall, the server's role may not: a server
/// that is broken into cannot let one organization read another's rows.
#[tokio::test]
async fn a_role_that_does_not_own_the_tables_cannot_turn_the_wall_off() {
    if common::db_type() != DbType::Postgres {
        return;
    }
    let database = owned_apart(TableOwner::Superuser).await;
    let owner_store = Store::connect(database.db_type(), database.owner_database_connection())
        .await
        .expect("the owner brings the schema up to date");
    owner_store.close().await;

    for statement in [
        "ALTER TABLE blocks NO FORCE ROW LEVEL SECURITY",
        "ALTER TABLE blocks DISABLE ROW LEVEL SECURITY",
        "DROP POLICY org_wall ON blocks",
        "CREATE POLICY open_wall ON blocks USING (true)",
        "DROP TABLE blocks",
        "CREATE TABLE intruder (id integer)",
    ] {
        let refused = common::try_execute_raw(database.database_connection(), statement)
            .await
            .expect_err(statement);
        let said = refused.to_string();
        assert!(
            said.contains("must be owner") || said.contains("permission denied"),
            "{statement}: {said}"
        );
    }
    database.remove().await;
}
