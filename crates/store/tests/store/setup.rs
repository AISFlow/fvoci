//! Connecting, schema migration and the databases that are refused: what
//! `Store::connect` promises. And what the schema it creates promises: the same tables
//! on both databases, with foreign keys that are enforced.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use fvoci_model::{Block, Category, CategoryType, ContainerVisibility, FileRecord, new_id};
use fvoci_store::{DbType, Store, StoreError};
use tokio::task::JoinSet;

use crate::common;

#[tokio::test]
async fn connect_creates_the_schema_once_and_reopens_it() {
    let test = common::setup().await;
    assert_eq!(test.db_type(), common::db_type());
    let version = test
        .db_version()
        .await
        .expect("the database reports a version");
    assert!(!version.is_empty(), "the version string is not empty");

    // A second store on the same database finds the schema and leaves it alone.
    let again = Store::connect(common::db_type(), &test.database_connection())
        .await
        .expect("an existing schema at the current version is accepted");
    again.close().await;

    test.teardown().await;
}

/// Connects to a database that holds what `prepare` put there and returns the refusal.
async fn refusal_of_a_database_with(prepare: &str) -> String {
    let database = common::empty_database().await;
    common::execute_raw(&common::raw_connection(&database), prepare).await;

    let error = Store::connect(database.db_type(), database.database_connection())
        .await
        .expect_err("the database is not this server's to open");
    assert!(matches!(error, StoreError::Schema(_)), "got {error:?}");
    database.remove().await;
    error.to_string()
}

#[tokio::test]
async fn tables_without_a_migration_record_are_refused() {
    let refusal =
        refusal_of_a_database_with("CREATE TABLE blocks (id VARCHAR(36) PRIMARY KEY);").await;
    assert!(
        refusal.contains("no record of schema migrations"),
        "the refusal says why: {refusal}"
    );
}

#[tokio::test]
async fn a_database_migrated_by_a_newer_server_is_refused() {
    let test = common::setup().await;
    test.store.close().await;
    // What a later version of this server leaves behind: a migration this one lacks.
    common::execute_raw(
        &test.database_connection(),
        "INSERT INTO _sqlx_migrations \
         (version, description, success, checksum, execution_time) \
         SELECT 9999, 'from the future', success, checksum, execution_time \
         FROM _sqlx_migrations WHERE version = 1",
    )
    .await;

    let error = Store::connect(common::db_type(), &test.database_connection())
        .await
        .expect_err("a database with a migration this server does not know is refused");
    assert!(
        matches!(&error, StoreError::Schema(reason) if reason.contains("newer server")),
        "got {error:?}"
    );

    test.teardown().await;
}

#[tokio::test]
async fn a_changed_migration_history_is_refused() {
    let test = common::setup().await;
    test.store.close().await;
    common::execute_raw(
        &test.database_connection(),
        "UPDATE _sqlx_migrations SET checksum = checksum || checksum WHERE version = 1",
    )
    .await;

    let error = Store::connect(common::db_type(), &test.database_connection())
        .await
        .expect_err("a database whose applied migration differs is refused");
    assert!(
        matches!(&error, StoreError::Schema(reason) if reason.contains("differs")),
        "got {error:?}"
    );

    test.teardown().await;
}

#[tokio::test]
async fn the_tables_carry_no_prefix() {
    let test = common::setup().await;
    // A statement that names the tables plainly runs, on either database.
    common::execute_raw(
        &test.database_connection(),
        "SELECT (SELECT COUNT(*) FROM blocks) + (SELECT COUNT(*) FROM containers) \
         + (SELECT COUNT(*) FROM users) + (SELECT COUNT(*) FROM sessions)",
    )
    .await;
    test.teardown().await;
}

#[tokio::test]
async fn in_memory_sqlite_databases_are_private_to_their_store() {
    let first = Store::connect(DbType::Sqlite, ":memory:?busy_timeout_ms=5000")
        .await
        .expect("an in-memory database");
    let second = Store::connect(DbType::Sqlite, ":memory:")
        .await
        .expect("another in-memory database");

    // A block of a board of a workspace, by a user, all in the first store only.
    let (org_id, _) = common::organization(&first, "").await;
    let first = first.for_org(&org_id).expect("an identifier");
    let user_id = common::user(&first, "only-in-first").await;
    let workspace_id = common::workspace(&first).await;
    let boards = common::create_test_boards(&first, &workspace_id, &user_id, 1).await;
    let mut block = Block {
        id: new_id(),
        container_id: boards[0].id.clone(),
        created_by: user_id.clone(),
        ..Block::default()
    };
    first
        .insert_block(&mut block, &user_id)
        .await
        .expect("inserted");

    assert!(
        first.get_block(&block.id).await.is_ok(),
        "the first store has the block"
    );
    let missing = second
        .get_block(&block.id)
        .await
        .expect_err("not in the second store");
    assert!(missing.is_not_found(), "got {missing:?}");
    let missing = second
        .get_user_by_id(&user_id)
        .await
        .expect_err("not in the second store");
    assert!(missing.is_not_found(), "got {missing:?}");

    first.close().await;
    second.close().await;
}

/// How many writes [`foreign_keys_are_enforced_on_the_connections_of_the_store`] sends
/// at once: several times the connections a store keeps, so that each of them is used.
const WRITES_AT_ONCE: usize = 40;

/// A file record for a board that does not exist: a row only the database can refuse,
/// since the store does not look for the board first. The store reads that refusal as
/// "the board is not found".
fn record_for_no_board(stored_name: String) -> FileRecord {
    FileRecord {
        id: new_id(),
        container_id: new_id(),
        stored_name,
        ..FileRecord::default()
    }
}

/// A row that refers to something that does not exist.
#[derive(Debug, Clone, Copy)]
enum Orphan {
    /// A file record for a board that does not exist.
    FileRecord,
    /// A category of a user who does not exist.
    Category,
}

/// The schema's foreign keys are enforced on the connections the store uses, each of
/// them. SQLite enforces them only on a connection that asks for it, so one that did
/// not would store what the others refuse.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn foreign_keys_are_enforced_on_the_connections_of_the_store() {
    let test = common::setup().await;
    let store = &test.store;
    let user_id = common::user(store, "user").await;
    let workspace_id = common::workspace(store).await;

    // Rows that refer to nothing, by a single statement and by a transaction: a file
    // record for a board that does not exist, a category of a user who does not.
    let mut writes = JoinSet::new();
    for index in 0..WRITES_AT_ONCE {
        let store = test.store.clone();
        let workspace_id = workspace_id.clone();
        writes.spawn(async move {
            if index % 2 == 0 {
                let record = record_for_no_board(format!("7file{index}.png"));
                (Orphan::FileRecord, store.save_file_record(&record).await)
            } else {
                let category = Category {
                    id: new_id(),
                    name: format!("Category {index}"),
                    user_id: new_id(),
                    workspace_id,
                    ..Category::default()
                };
                (Orphan::Category, store.create_category(&category).await)
            }
        });
    }
    let mut refused = 0;
    while let Some(write) = writes.join_next().await {
        let (orphan, outcome) = write.expect("the write ran to its end");
        let error = outcome.expect_err("a row that refers to nothing is refused");
        // The store hands on the refusal of a category as the database gave it, and
        // reads the refusal of a file record as "no such board".
        let over_the_foreign_key = match orphan {
            Orphan::FileRecord => matches!(&error, StoreError::NotFound(what) if what == "board"),
            Orphan::Category => common::refused_over_a_foreign_key(&error),
        };
        assert!(
            over_the_foreign_key,
            "{orphan:?}: refused over the foreign key: got {error:?}"
        );
        refused += 1;
    }
    assert_eq!(refused, WRITES_AT_ONCE, "every write was refused");

    // The same rows are stored once what they refer to exists.
    let boards = common::create_test_boards(store, &workspace_id, &user_id, 1).await;
    let record = FileRecord {
        container_id: boards[0].id.clone(),
        ..record_for_no_board("7file.png".to_owned())
    };
    store
        .save_file_record(&record)
        .await
        .expect("a file record for a board that exists is stored");
    let category = Category {
        id: new_id(),
        name: "Category".to_owned(),
        user_id: user_id.clone(),
        workspace_id: workspace_id.clone(),
        ..Category::default()
    };
    store
        .create_category(&category)
        .await
        .expect("a category of a user who exists is stored");
    let listed = store
        .get_user_categories(&user_id, &workspace_id)
        .await
        .expect("the categories are listed");
    assert_eq!(
        listed.len(),
        1,
        "the one category that was not refused: {listed:?}"
    );

    test.teardown().await;
}

/// An in-memory SQLite database enforces the foreign keys as a database file does.
#[tokio::test]
async fn foreign_keys_are_enforced_in_an_in_memory_sqlite_database() {
    let store = Store::connect(DbType::Sqlite, ":memory:")
        .await
        .expect("an in-memory database");

    let error = store
        .save_file_record(&record_for_no_board("7file.png".to_owned()))
        .await
        .expect_err("a file record for a board that does not exist is refused");
    assert!(
        matches!(&error, StoreError::NotFound(what) if what == "board"),
        "refused over the foreign key, which reads as no such board: got {error:?}"
    );

    store.close().await;
}

/// Whether the database refused a statement because a row would break a `CHECK` of its
/// table.
fn is_check_violation(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .is_some_and(sqlx::error::DatabaseError::is_check_violation)
}

/// Sets one column of one row past the store, and returns how many rows changed or the
/// database's refusal.
async fn set_column(
    test: &common::TestStore,
    table: &str,
    column: &str,
    value: &str,
    id: &str,
) -> Result<u64, sqlx::Error> {
    common::try_execute_raw(
        &test.database_connection(),
        &format!("UPDATE {table} SET {column} = '{value}' WHERE id = '{id}'"),
    )
    .await
}

/// The schema keeps what the model promises: a board is `open` or `private`,
/// whoever writes the row. Anything else is refused, in whatever letter case.
#[tokio::test]
async fn the_schema_refuses_a_board_visibility_that_is_none() {
    let test = common::setup().await;
    let user_id = common::user(&test, "owner").await;
    let workspace_id = common::workspace(&test).await;
    let boards = common::create_test_boards(&test, &workspace_id, &user_id, 1).await;
    let board_id = &boards[0].id;

    for vis in ["", " ", "X", "O", "P", "o", "p", "Open", "PRIVATE", "open "] {
        let error = set_column(&test, "containers", "visibility", vis, board_id)
            .await
            .expect_err(&format!("the board visibility {vis:?} is refused"));
        assert!(
            is_check_violation(&error),
            "{vis:?}: refused by the check: got {error:?}"
        );
        let board = test
            .get_container(board_id)
            .await
            .expect("the board is read");
        assert_eq!(
            board.visibility,
            ContainerVisibility::Open,
            "{vis:?}: the board is as it was"
        );
    }
    for vis in [ContainerVisibility::Private, ContainerVisibility::Open] {
        let changed = set_column(&test, "containers", "visibility", vis.as_str(), board_id)
            .await
            .expect("a board visibility that is one is stored");
        assert_eq!(changed, 1, "{vis:?}: the one row of the board");
        let board = test
            .get_container(board_id)
            .await
            .expect("the board is read");
        assert_eq!(board.visibility, vis, "as written");
    }

    // A row that is new is checked as one that is changed: the same row is stored with
    // a visibility that is one, and refused with one that is none.
    let new_board = |vis: &str| {
        format!(
            "INSERT INTO containers (id, workspace_id, visibility, title, description, icon, \
             show_description, is_template, properties, card_properties, created_at, \
             updated_at) VALUES ('{}', '{workspace_id}', '{vis}', '', '', '', FALSE, FALSE, \
             '{{}}', '[]', 1, 1)",
            new_id()
        )
    };
    let error = common::try_execute_raw(&test.database_connection(), &new_board("O"))
        .await
        .expect_err("a new board of a visibility that is none is refused");
    assert!(is_check_violation(&error), "got {error:?}");
    let stored = common::try_execute_raw(&test.database_connection(), &new_board("private"))
        .await
        .expect("a new board of a visibility that is one is stored");
    assert_eq!(stored, 1, "the one row");

    test.teardown().await;
}

/// The schema keeps what the model promises: a category is one the server made
/// (`system`) or one of the user's own (`custom`), whoever writes the row. Anything
/// else is refused, the empty type too.
#[tokio::test]
async fn the_schema_refuses_a_category_type_that_is_none() {
    let test = common::setup().await;
    let user_id = common::user(&test, "owner").await;
    let workspace_id = common::workspace(&test).await;
    let category = Category {
        id: new_id(),
        name: "Category".to_owned(),
        user_id: user_id.clone(),
        workspace_id: workspace_id.clone(),
        ..Category::default()
    };
    test.create_category(&category)
        .await
        .expect("the category is stored");

    for category_type in ["", " ", "odd", "System", "CUSTOM", "customs", "system "] {
        let error = set_column(&test, "categories", "type", category_type, &category.id)
            .await
            .expect_err(&format!("the category type {category_type:?} is refused"));
        assert!(
            is_check_violation(&error),
            "{category_type:?}: refused by the check: got {error:?}"
        );
        let stored = test
            .get_category(&category.id)
            .await
            .expect("the category is read");
        assert_eq!(
            stored.category_type,
            CategoryType::Custom,
            "{category_type:?}: the category is as it was"
        );
    }
    for category_type in [CategoryType::System, CategoryType::Custom] {
        let changed = set_column(
            &test,
            "categories",
            "type",
            category_type.as_str(),
            &category.id,
        )
        .await
        .expect("a category type that is one is stored");
        assert_eq!(changed, 1, "{category_type:?}: the one row of the category");
        let stored = test
            .get_category(&category.id)
            .await
            .expect("the category is read");
        assert_eq!(stored.category_type, category_type, "as written");
    }

    // A row that is new is checked as one that is changed: the same row is stored with
    // a type that is one, and refused with one that is none.
    let new_category = |category_type: &str| {
        format!(
            "INSERT INTO categories (id, name, user_id, workspace_id, created_at, updated_at, type) \
             VALUES ('{}', 'New', '{user_id}', '{workspace_id}', 1, 1, '{category_type}')",
            new_id()
        )
    };
    let error = common::try_execute_raw(&test.database_connection(), &new_category("odd"))
        .await
        .expect_err("a new category of a type that is none is refused");
    assert!(is_check_violation(&error), "got {error:?}");
    let stored = common::try_execute_raw(&test.database_connection(), &new_category("system"))
        .await
        .expect("a new category of a type that is one is stored");
    assert_eq!(stored, 1, "the one row");

    test.teardown().await;
}

/// The column types that only PostgreSQL has. Where its baseline names one of them,
/// SQLite's says `TEXT`.
const POSTGRES_ONLY_TYPES: [&str; 3] = ["uuid", "jsonb", "json"];

/// Reads the baseline schema of a database kind.
fn baseline_schema(directory: &str) -> String {
    schema_file(directory, "0001_baseline.sql")
}

/// Reads a migration of a database kind.
fn schema_file(directory: &str, name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("schema")
        .join(directory)
        .join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()))
}

/// The statements of a schema file: its lines without their comments, the blank ones
/// left out, each with the number it has in the file. (No string in the schema holds
/// `--`.)
fn statement_lines(schema: &str) -> Vec<(usize, &str)> {
    schema
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let statement = line
                .split_once("--")
                .map_or(line, |(statement, _comment)| statement)
                .trim_end();
            (!statement.is_empty()).then_some((index + 1, statement))
        })
        .collect()
}

/// The words of a line: its runs of letters, digits and underscores.
fn words(line: &str) -> impl Iterator<Item = &str> {
    line.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|word| !word.is_empty())
}

/// The indexes that PostgreSQL's baseline has and SQLite's does not: a key of the
/// members of a workspace with their organization, and the places of the loose objects
/// by themselves, which SQLite's key of that table is and PostgreSQL's, with the
/// organization first, is not.
const POSTGRES_ONLY_INDEXES: [&str; 2] = ["loose_objects_path_idx", "workspace_members_scope_key"];

/// What a baseline makes: each table with the names of its columns, and the names of
/// the indexes.
#[derive(Debug, Default, PartialEq, Eq)]
struct Shape {
    tables: BTreeMap<String, BTreeSet<String>>,
    indexes: BTreeSet<String>,
}

/// The tables, columns and indexes a schema file makes. A column is a line of a
/// `CREATE TABLE` that begins with a name; a line of a key or a check is none.
fn shape_of(schema: &str) -> Shape {
    let mut shape = Shape::default();
    let mut table: Option<String> = None;
    for (_, line) in statement_lines(schema) {
        if let Some(name) = line.strip_prefix("CREATE TABLE ") {
            let name = name.trim_end_matches(" (").to_owned();
            shape.tables.insert(name.clone(), BTreeSet::new());
            table = Some(name);
            continue;
        }
        if line.starts_with(')') {
            table = None;
            continue;
        }
        if let Some(index) = line
            .strip_prefix("CREATE INDEX ")
            .or_else(|| line.strip_prefix("CREATE UNIQUE INDEX "))
            .and_then(|rest| words(rest).next())
        {
            shape.indexes.insert(index.to_owned());
            continue;
        }
        if let Some(columns) = table.as_ref().and_then(|name| shape.tables.get_mut(name))
            && let Some(column) = words(line).next()
            && !["PRIMARY", "UNIQUE", "CHECK", "FOREIGN", "CONSTRAINT"].contains(&column)
        {
            columns.insert(column.to_owned());
        }
    }
    shape
}

/// The two baseline files make the same tables and indexes. Each table of PostgreSQL's
/// has the columns of SQLite's, and the organization of its rows (`org_id`) where
/// SQLite, of one organization, has none; PostgreSQL has the two indexes more of
/// [`POSTGRES_ONLY_INDEXES`]. The column types that only PostgreSQL has are `TEXT`
/// on SQLite. A table, column or index that one file has and the other lacks fails here.
#[test]
fn the_two_baselines_make_the_same_tables_columns_and_indexes() {
    let postgres = baseline_schema("postgres");
    let sqlite = baseline_schema("sqlite");
    let mut postgres_shape = shape_of(&postgres);
    let sqlite_shape = shape_of(&sqlite);
    assert!(
        postgres_shape.tables.len() > 50 && postgres_shape.indexes.len() > 100,
        "the PostgreSQL baseline is read whole: {postgres_shape:?}"
    );

    for index in POSTGRES_ONLY_INDEXES {
        assert!(
            postgres_shape.indexes.remove(index) && !sqlite_shape.indexes.contains(index),
            "{index} is PostgreSQL's alone"
        );
    }
    for (table, columns) in &mut postgres_shape.tables {
        if sqlite_shape
            .tables
            .get(table)
            .is_some_and(|sqlite_columns| !sqlite_columns.contains("org_id"))
        {
            columns.remove("org_id");
        }
    }
    assert_eq!(
        postgres_shape, sqlite_shape,
        "the baselines of the two databases make different tables, columns or indexes"
    );

    // Each of the types is in the PostgreSQL baseline, and none of them is in SQLite's.
    let postgres_types = column_types(&postgres);
    let sqlite_types = column_types(&sqlite);
    for postgres_type in POSTGRES_ONLY_TYPES {
        assert!(
            postgres_types.contains(postgres_type),
            "the PostgreSQL baseline has no {postgres_type} column: {postgres_types:?}"
        );
        assert!(
            !sqlite_types.contains(postgres_type),
            "the SQLite baseline has a column of the PostgreSQL type {postgres_type}"
        );
    }
}

/// The types of the columns of a schema file, in lowercase: the word after the name of
/// each column of a `CREATE TABLE`.
fn column_types(schema: &str) -> BTreeSet<String> {
    let mut types = BTreeSet::new();
    let mut in_table = false;
    for (_, line) in statement_lines(schema) {
        if line.starts_with("CREATE TABLE ") {
            in_table = true;
        } else if line.starts_with(')') {
            in_table = false;
        } else if in_table {
            let mut words = words(line);
            if let (Some(column), Some(kind)) = (words.next(), words.next())
                && !["PRIMARY", "UNIQUE", "CHECK", "FOREIGN", "CONSTRAINT"].contains(&column)
            {
                types.insert(kind.to_ascii_lowercase());
            }
        }
    }
    types
}

/// The shape of a schema is read as the test above needs it: the columns of a table
/// but not its keys or checks, and every index, also one written over two lines.
#[test]
fn the_shape_of_a_schema_is_its_tables_columns_and_indexes() {
    let schema = "-- A table.\n\
                  CREATE TABLE t (\n    \
                      id UUID NOT NULL, -- an identifier\n    \
                      parent_id UUID,\n    \
                      PRIMARY KEY (id),\n    \
                      CHECK (parent_id <> id)\n\
                  );\n\
                  CREATE INDEX t_parent_id_idx ON t (parent_id);\n\
                  CREATE UNIQUE INDEX t_id_key\n    ON t (id);\n";
    let names = |names: &[&str]| -> BTreeSet<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    };
    assert_eq!(
        shape_of(schema),
        Shape {
            tables: BTreeMap::from([("t".to_owned(), names(&["id", "parent_id"]))]),
            indexes: names(&["t_id_key", "t_parent_id_idx"]),
        }
    );
}
