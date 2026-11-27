//! A database the last server before the baseline brought up to date: its schema is
//! the baseline's, and its record of the migrations is replaced by the baseline's when
//! it is opened, with nothing else changed; any other record of those servers is
//! refused.
//!
//! The files of `before_baseline/` were read from such databases, brought up to date by
//! the chain of migrations: the record of each kind (`record-*.tsv`: the version, the
//! checksum and whether it applied, of every migration), and the catalog of each,
//! written as [`catalog`] writes one (`catalog-*.txt`).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use fvoci_store::{DbType, Store, StoreError};
use sha2::{Digest as _, Sha384};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{ConnectOptions as _, Connection as _, Row as _};

use crate::common;

/// The folder of the files read from the databases before the baseline.
fn folder() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("store")
        .join("before_baseline")
}

/// The name a file of the folder has for the kind of database.
fn kind_name(db_type: DbType) -> &'static str {
    match db_type {
        DbType::Sqlite => "sqlite",
        DbType::Postgres => "postgres",
    }
}

/// One line of the record of the migrations: the version, the checksum in lowercase
/// hexadecimal, and whether it applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Recorded {
    pub(crate) version: i64,
    pub(crate) checksum: String,
    pub(crate) success: bool,
}

/// The record a database of the kind held before the baseline, as it was read.
pub(crate) fn record_before_the_baseline(db_type: DbType) -> Vec<Recorded> {
    let path = folder().join(format!("record-{}.tsv", kind_name(db_type)));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
    text.lines()
        .map(|line| {
            let [version, checksum, success] = line.split('\t').collect::<Vec<_>>()[..] else {
                panic!(
                    "{}: `{line}` is no `version checksum success`",
                    path.display()
                );
            };
            Recorded {
                version: version.parse().expect("a version is a number"),
                checksum: checksum.to_owned(),
                success: success.parse().expect("success is `true` or `false`"),
            }
        })
        .collect()
}

/// The record of the migrations of a test's database, oldest first.
pub(crate) async fn record(database_connection: &str) -> Vec<Recorded> {
    let sql = "SELECT version, checksum, success FROM _sqlx_migrations ORDER BY version";
    let rows = match common::db_type() {
        DbType::Sqlite => {
            let mut conn = sqlite(database_connection).await;
            let rows = sqlx::query(sql)
                .fetch_all(&mut conn)
                .await
                .expect("the record is read");
            conn.close().await.expect("the connection closes");
            rows.iter()
                .map(|row| (row.get(0), row.get::<Vec<u8>, _>(1), row.get(2)))
                .collect::<Vec<(i64, Vec<u8>, bool)>>()
        }
        DbType::Postgres => {
            let mut conn = common::connect_past_the_store(database_connection).await;
            let rows = sqlx::query(sql)
                .fetch_all(&mut conn)
                .await
                .expect("the record is read");
            conn.close().await.expect("the connection closes");
            rows.iter()
                .map(|row| (row.get(0), row.get::<Vec<u8>, _>(1), row.get(2)))
                .collect()
        }
    };
    rows.into_iter()
        .map(|(version, checksum, success)| Recorded {
            version,
            checksum: checksum
                .iter()
                .fold(String::new(), |mut hexadecimal, byte| {
                    write!(hexadecimal, "{byte:02x}").expect("a string takes it");
                    hexadecimal
                }),
            success,
        })
        .collect()
}

async fn sqlite(path: &str) -> sqlx::SqliteConnection {
    SqliteConnectOptions::new()
        .filename(path)
        .connect()
        .await
        .expect("the test database file opens")
}

/// The catalog of a test's database as text: every object of the schema with its
/// definition, and every row of every table but the record of the migrations. Two
/// databases whose catalogs are the same text have one schema, and the same rows.
///
/// On SQLite it is the definitions the database keeps (`sqlite_master`) without their
/// comments and the spaces between their words. On PostgreSQL it is read from the
/// catalog: the tables and whether row-level security is on and forced, the columns in
/// their order (not their numbers, which columns dropped earlier leave gaps in) with
/// their types, defaults and whether they take NULL, every constraint, index, trigger,
/// function, policy, type and comment, with the schema's name taken out.
pub(crate) async fn catalog(database_connection: &str) -> String {
    match common::db_type() {
        DbType::Sqlite => sqlite_catalog(database_connection).await,
        DbType::Postgres => postgres_catalog(database_connection).await,
    }
}

async fn sqlite_catalog(path: &str) -> String {
    let mut conn = sqlite(path).await;
    let mut text = String::from("# objects\n");
    let objects: Vec<(String, String, String, Option<String>)> =
        sqlx::query_as("SELECT type, name, tbl_name, sql FROM sqlite_master ORDER BY type, name")
            .fetch_all(&mut conn)
            .await
            .expect("the definitions are read");
    for (kind, name, table, sql) in &objects {
        let sql = sql.as_deref().map(normalized_sql).unwrap_or_default();
        writeln!(text, "{kind} {name} on {table}: {sql}").expect("a string takes what is written");
    }
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&mut conn)
        .await
        .expect("the version of the file is read");
    writeln!(text, "# user_version {version}\n# rows").expect("a string takes what is written");
    let tables: Vec<String> = objects
        .iter()
        .filter(|(kind, name, _, _)| kind == "table" && !is_record_or_internal(name))
        .map(|(_, name, _, _)| name.clone())
        .collect();
    for table in tables {
        let mut columns: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT name FROM pragma_table_info('{table}')"
        )))
        .fetch_all(&mut conn)
        .await
        .expect("the columns are read");
        columns.sort();
        let quoted: Vec<String> = columns
            .iter()
            .map(|column| format!("'{column}=' || quote(\"{column}\")"))
            .collect();
        let rows: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT {} FROM \"{table}\" ORDER BY 1",
            quoted.join(" || ' ' || ")
        )))
        .fetch_all(&mut conn)
        .await
        .expect("the rows are read");
        for row in rows {
            writeln!(text, "{table}: {row}").expect("a string takes what is written");
        }
    }
    conn.close().await.expect("the connection closes");
    text
}

/// Whether a table is the record of the migrations or one SQLite keeps for itself.
fn is_record_or_internal(table: &str) -> bool {
    table == "_sqlx_migrations" || table.starts_with("sqlite_")
}

/// A definition without its comments, its words apart by one space and none next to
/// a parenthesis or a comma, and its names without quotes: as written by hand, and as
/// SQLite writes it again where a table or a column was renamed or added to.
fn normalized_sql(sql: &str) -> String {
    let code: Vec<&str> = sql
        .lines()
        .map(|line| line.split_once("--").map_or(line, |(code, _)| code))
        .collect();
    let joined = code.join(" ").replace('"', "");
    let spaced: Vec<&str> = joined.split_whitespace().collect();
    let mut text = spaced.join(" ");
    for mark in ["(", ")", ","] {
        text = text
            .replace(&format!(" {mark}"), mark)
            .replace(&format!("{mark} "), mark);
    }
    text
}

/// What the catalog of a PostgreSQL schema says, section by section: each a query
/// whose rows are lines of text.
const POSTGRES_SECTIONS: [(&str, &str); 9] = [
    (
        "tables",
        "SELECT c.relname || ' ' || c.relkind::text || ' ' || c.relpersistence::text \
           || ' rls=' || c.relrowsecurity || ' forced=' || c.relforcerowsecurity \
           || ' acl=' || COALESCE(c.relacl::text, '') \
         FROM pg_class c \
         WHERE c.relnamespace = current_schema()::regnamespace AND c.relkind NOT IN ('i', 'I') \
         ORDER BY c.relname",
    ),
    (
        "columns",
        "SELECT c.relname || '.' || a.attname || ' ' || format_type(a.atttypid, a.atttypmod) \
           || CASE WHEN a.attnotnull THEN ' not null' ELSE '' END \
           || CASE WHEN a.attidentity <> '' THEN ' identity ' || a.attidentity::text ELSE '' END \
           || CASE WHEN a.attgenerated <> '' THEN ' generated ' || a.attgenerated::text ELSE '' END \
           || COALESCE(' default ' || pg_get_expr(d.adbin, d.adrelid), '') \
           || CASE WHEN a.attcollation <> t.typcollation \
                THEN ' collate ' || a.attcollation::regcollation::text ELSE '' END \
         FROM pg_attribute a \
         JOIN pg_class c ON c.oid = a.attrelid \
         JOIN pg_type t ON t.oid = a.atttypid \
         LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum \
         WHERE c.relnamespace = current_schema()::regnamespace AND c.relkind = 'r' \
           AND a.attnum > 0 AND NOT a.attisdropped \
         ORDER BY c.relname, a.attnum",
    ),
    (
        "constraints",
        "SELECT c.relname || ' ' || con.conname || ' ' || con.contype::text || ' ' \
           || pg_get_constraintdef(con.oid) \
           || CASE WHEN con.condeferrable THEN ' deferrable' ELSE '' END \
           || CASE WHEN con.convalidated THEN '' ELSE ' not valid' END \
         FROM pg_constraint con JOIN pg_class c ON c.oid = con.conrelid \
         WHERE con.connamespace = current_schema()::regnamespace \
         ORDER BY c.relname, con.conname",
    ),
    (
        "indexes",
        "SELECT indexdef FROM pg_indexes WHERE schemaname = current_schema() \
         ORDER BY tablename, indexname",
    ),
    (
        "triggers",
        "SELECT pg_get_triggerdef(t.oid) || ' enabled=' || t.tgenabled::text \
         FROM pg_trigger t JOIN pg_class c ON c.oid = t.tgrelid \
         WHERE c.relnamespace = current_schema()::regnamespace AND NOT t.tgisinternal \
         ORDER BY c.relname, t.tgname",
    ),
    (
        "functions",
        "SELECT p.proname || '(' || pg_get_function_identity_arguments(p.oid) || ') ' \
           || pg_get_function_result(p.oid) || ' ' || l.lanname || ' ' || p.provolatile::text \
           || ' secdef=' || p.prosecdef \
           || ' config=' || COALESCE(array_to_string(p.proconfig, ','), '') \
           || ' ' || replace(p.prosrc, E'\\n', E'\\\\n') \
         FROM pg_proc p JOIN pg_language l ON l.oid = p.prolang \
         WHERE p.pronamespace = current_schema()::regnamespace \
         ORDER BY 1",
    ),
    (
        "policies",
        "SELECT tablename || ' ' || policyname || ' ' || permissive || ' ' \
           || array_to_string(roles, ',') || ' ' || cmd \
           || ' using ' || COALESCE(qual, '') || ' check ' || COALESCE(with_check, '') \
         FROM pg_policies WHERE schemaname = current_schema() \
         ORDER BY tablename, policyname",
    ),
    (
        "types",
        "SELECT t.typname || ' ' || t.typtype::text FROM pg_type t \
         WHERE t.typnamespace = current_schema()::regnamespace AND t.typtype IN ('d', 'e', 'r', 'm') \
         ORDER BY 1",
    ),
    (
        "comments",
        "SELECT c.relname || COALESCE('.' || a.attname, '') || ' ' || d.description \
         FROM pg_description d \
         JOIN pg_class c ON c.oid = d.objoid AND d.classoid = 'pg_class'::regclass \
         LEFT JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum = d.objsubid \
           AND d.objsubid > 0 \
         WHERE c.relnamespace = current_schema()::regnamespace \
         ORDER BY 1",
    ),
];

async fn postgres_catalog(database_connection: &str) -> String {
    let mut conn = common::connect_past_the_store(database_connection).await;
    let schema: String = sqlx::query_scalar("SELECT current_schema()::text")
        .fetch_one(&mut conn)
        .await
        .expect("the schema is named");
    let mut text = String::new();
    for (section, query) in POSTGRES_SECTIONS {
        writeln!(text, "# {section}").expect("a string takes what is written");
        let lines: Vec<String> = sqlx::query_scalar(query)
            .fetch_all(&mut conn)
            .await
            .unwrap_or_else(|error| panic!("the {section} are read: {error}"));
        for line in lines {
            text.push_str(&line.replace(&format!("{schema}."), ""));
            text.push('\n');
        }
    }
    text.push_str("# rows\n");
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT relname::text FROM pg_class \
         WHERE relnamespace = current_schema()::regnamespace AND relkind = 'r' \
           AND relname <> '_sqlx_migrations' \
         ORDER BY relname",
    )
    .fetch_all(&mut conn)
    .await
    .expect("the tables are read");
    for table in tables {
        let rows: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT to_jsonb(t)::text FROM {table} AS t ORDER BY 1"
        )))
        .fetch_all(&mut conn)
        .await
        .expect("the rows are read");
        for row in rows {
            writeln!(text, "{table}: {row}").expect("a string takes what is written");
        }
    }
    conn.close().await.expect("the connection closes");
    text
}

/// The catalog of a database of the kind before the baseline, as it was read.
fn catalog_before_the_baseline(db_type: DbType) -> String {
    let path = folder().join(format!("catalog-{}.txt", kind_name(db_type)));
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()))
}

/// Fails, with a copy of the catalog that differs beside the tests' files for `diff`,
/// unless `catalog` is the one of the databases before the baseline.
fn assert_catalog_is_the_one_before_the_baseline(catalog: &str) {
    let db_type = common::db_type();
    let expected = catalog_before_the_baseline(db_type);
    if catalog == expected {
        return;
    }
    let copy = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("catalog-{}.txt", kind_name(db_type)));
    std::fs::write(&copy, catalog).expect("the catalog that differs is written");
    panic!(
        "the schema is not the one before the baseline: diff {} {}",
        folder()
            .join(format!("catalog-{}.txt", kind_name(db_type)))
            .display(),
        copy.display()
    );
}

/// A database of its own brought up to date by a store that is closed again: what
/// reaches it, and the database, to remove when the test is done.
async fn brought_up_to_date() -> (String, fvoci_store::testing::TestDatabase) {
    let database = common::empty_database().await;
    let connection = common::raw_connection(&database);
    Store::connect(common::db_type(), &connection)
        .await
        .expect("the empty database is brought up to date")
        .close()
        .await;
    (connection, database)
}

/// A database brought up to date has the schema, and the rows, of one the last server
/// before the baseline brought up to date.
#[tokio::test]
async fn a_database_brought_up_to_date_has_the_schema_before_the_baseline() {
    let (connection, database) = brought_up_to_date().await;
    assert_catalog_is_the_one_before_the_baseline(&catalog(&connection).await);
    database.remove().await;
}

/// A database brought up to date holds the record of the baseline alone: migration 1,
/// with the checksum of its file.
#[tokio::test]
async fn a_database_brought_up_to_date_holds_the_record_of_the_baseline() {
    let (connection, database) = brought_up_to_date().await;
    assert_eq!(record(&connection).await, [baseline_record()]);
    database.remove().await;
}

/// The record of the baseline: migration 1, with the SHA-384 of the file of the kind.
fn baseline_record() -> Recorded {
    let file: &[u8] = match common::db_type() {
        DbType::Sqlite => include_bytes!("../../schema/sqlite/0001_baseline.sql"),
        DbType::Postgres => include_bytes!("../../schema/postgres/0001_baseline.sql"),
    };
    Recorded {
        version: 1,
        checksum: Sha384::digest(file)
            .iter()
            .fold(String::new(), |mut hexadecimal, byte| {
                write!(hexadecimal, "{byte:02x}").expect("a string takes it");
                hexadecimal
            }),
        success: true,
    }
}

/// Writes `rows` as the whole record of the migrations of the database, as the server
/// that brought it up to date would have.
async fn write_record(connection: &str, rows: &[Recorded]) {
    let values: Vec<String> = rows
        .iter()
        .map(|row| {
            let checksum = match common::db_type() {
                DbType::Sqlite => format!("X'{}'", row.checksum),
                DbType::Postgres => format!("decode('{}', 'hex')", row.checksum),
            };
            format!(
                "({}, 'migration {}', {}, {checksum}, 0)",
                row.version, row.version, row.success
            )
        })
        .collect();
    common::execute_raw(
        connection,
        &format!(
            "DELETE FROM _sqlx_migrations; INSERT INTO _sqlx_migrations \
             (version, description, success, checksum, execution_time) VALUES {};",
            values.join(", ")
        ),
    )
    .await;
}

/// A database brought up to date by the baseline, with the record the last server
/// before it wrote: as that server left it.
async fn left_before_the_baseline() -> (String, fvoci_store::testing::TestDatabase) {
    let (connection, database) = brought_up_to_date().await;
    write_record(&connection, &record_before_the_baseline(common::db_type())).await;
    (connection, database)
}

/// Opening a database before the baseline replaces its record with the baseline's, and
/// nothing else: the schema and every row are as they were.
#[tokio::test]
async fn opening_a_database_before_the_baseline_replaces_its_record_alone() {
    let (connection, database) = left_before_the_baseline().await;
    Store::connect(common::db_type(), &connection)
        .await
        .expect("the database before the baseline opens")
        .close()
        .await;
    assert_eq!(record(&connection).await, [baseline_record()]);
    assert_catalog_is_the_one_before_the_baseline(&catalog(&connection).await);
    database.remove().await;
}

/// What the organizations hold is kept: a board made before the record is replaced is
/// there after it.
#[tokio::test]
async fn the_rows_of_a_database_before_the_baseline_are_kept() {
    let test = common::setup().await;
    let user_id = common::user(&test, "kept").await;
    let boards = common::create_test_boards(&test, &test.workspace_id, &user_id, 1).await;
    let connection = test.database_connection();
    write_record(&connection, &record_before_the_baseline(common::db_type())).await;
    let store = test.reconnect().await;
    let board = store
        .get_container(&boards[0].id)
        .await
        .expect("the board is there");
    assert_eq!(board.title, boards[0].title);
    assert_eq!(record(&connection).await, [baseline_record()]);
    store.close().await;
    test.teardown().await;
}

/// A database opened again after its record was replaced is opened as it is: its record
/// is not written a second time.
#[tokio::test]
async fn a_replaced_record_is_not_replaced_again() {
    let (connection, database) = left_before_the_baseline().await;
    Store::connect(common::db_type(), &connection)
        .await
        .expect("the database before the baseline opens")
        .close()
        .await;
    common::execute_raw(
        &connection,
        "UPDATE _sqlx_migrations SET description = 'replaced once'",
    )
    .await;
    let store = Store::connect(common::db_type(), &connection)
        .await
        .expect("it opens again");
    assert_eq!(
        store
            .texts_unchecked("SELECT description FROM _sqlx_migrations")
            .await
            .expect("the record is read"),
        ["replaced once"]
    );
    store.close().await;
    database.remove().await;
}

/// Two servers that open a database before the baseline at once both open it, and the
/// record is replaced once: the second finds it done under the lock.
#[tokio::test]
async fn two_openings_at_once_replace_the_record_once() {
    let (connection, database) = left_before_the_baseline().await;
    let (first, second) = tokio::join!(
        Store::connect(common::db_type(), &connection),
        Store::connect(common::db_type(), &connection),
    );
    first.expect("the first opens it").close().await;
    second.expect("the second opens it").close().await;
    assert_eq!(record(&connection).await, [baseline_record()]);
    database.remove().await;
}

/// A record of the servers before the baseline that is not the whole of the last one's,
/// or a database whose changes of the data are not done, is refused with what to do,
/// and left as it was.
#[tokio::test]
async fn any_other_record_before_the_baseline_is_refused() {
    let chain = record_before_the_baseline(common::db_type());
    let last = chain.last().expect("the chain has migrations").version;
    let mut short = chain.clone();
    short.pop();
    let mut changed = chain.clone();
    changed[40].checksum = "00".repeat(48);
    let mut failed = chain.clone();
    failed[40].success = false;
    let mut longer = chain.clone();
    longer.push(Recorded {
        version: last + 1,
        checksum: "00".repeat(48),
        success: true,
    });
    let failed_version = chain[40].version;
    let cases = [
        (short, None, "it ends at migration".to_owned()),
        (
            changed,
            None,
            format!("its migration {failed_version} differs"),
        ),
        (
            failed,
            None,
            format!("its migration {failed_version} failed"),
        ),
        (longer, None, format!("migration {}, which", last + 1)),
        (
            chain.clone(),
            Some("INSERT INTO data_steps (name) VALUES ('date_values')"),
            "are not done (date_values)".to_owned(),
        ),
    ];
    for (rows, more, said) in cases {
        let (connection, database) = brought_up_to_date().await;
        write_record(&connection, &rows).await;
        if let Some(sql) = more {
            common::execute_raw(&connection, sql).await;
        }
        let refused = Store::connect(common::db_type(), &connection)
            .await
            .expect_err("the record is refused");
        assert!(
            matches!(&refused, StoreError::Schema(reason)
                if reason.contains(&said) && reason.contains("before the baseline")),
            "{said}: {refused}"
        );
        assert_eq!(record(&connection).await, rows, "{said}: the record stays");
        database.remove().await;
    }
}
