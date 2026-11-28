//! Fresh databases for tests, on SQLite or PostgreSQL. Behind the `test-util` feature.
//!
//! Tests run against SQLite by default. Set `FVOCI_STORE_TEST_DB_TYPE=postgres` and
//! `FVOCI_STORE_TEST_CONN_STRING` to a PostgreSQL URL to run them against PostgreSQL;
//! every [`TestDatabase`] is then a schema of its own in that database.
//!
//! The URL must name a role that is no superuser, is not exempt from row-level
//! security (`NOBYPASSRLS`) and may make schemas in the database (`CREATE`): the
//! policies of the schema pass a role that is either, and tests run as one would show
//! nothing of them. A test database is not made for such a role. The few tests of
//! what the server makes of such a role need the URL of a superuser of the same
//! database besides, in `FVOCI_STORE_TEST_SUPERUSER_CONN_STRING`.

use std::path::{Path, PathBuf};

use fvoci_model::new_id;
use sqlx::Connection as _;
use sqlx::postgres::PgConnection;

use crate::error::StoreError;
use crate::{DbType, Store};

/// The environment variable that selects the database kind for tests.
pub const DB_TYPE_ENV: &str = "FVOCI_STORE_TEST_DB_TYPE";

/// The environment variable that holds the PostgreSQL URL for tests.
pub const CONN_STRING_ENV: &str = "FVOCI_STORE_TEST_CONN_STRING";

/// The environment variable that holds the PostgreSQL URL of a superuser, for the tests
/// of what the server makes of a role that the wall between organizations does not
/// hold. `cargo xtask test` sets it to the superuser of its container.
pub const SUPERUSER_CONN_STRING_ENV: &str = "FVOCI_STORE_TEST_SUPERUSER_CONN_STRING";

/// Which database the tests run against, as the environment says ([`DB_TYPE_ENV`]).
///
/// # Panics
///
/// Panics if the variable names no kind of database.
pub fn db_type_of_the_tests() -> DbType {
    match std::env::var(DB_TYPE_ENV) {
        Ok(kind) if !kind.trim().is_empty() => kind
            .trim()
            .parse()
            .unwrap_or_else(|error| panic!("{DB_TYPE_ENV}: {error}")),
        _ => DbType::Sqlite,
    }
}

/// An empty database that belongs to one test.
#[derive(Debug)]
pub struct TestDatabase {
    db_type: DbType,
    database_connection: String,
    /// The `database_connection` of the role that owns the tables, where that is
    /// another role.
    owner_database_connection: Option<String>,
    cleanup: Cleanup,
}

/// Who owns the tables of a database made by [`TestDatabase::create_owned_apart`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableOwner {
    /// The superuser of [`SUPERUSER_CONN_STRING_ENV`], as the administrator of a
    /// database container is.
    Superuser,
    /// A role made for the one test database that is no superuser and has no
    /// `BYPASSRLS`, as the master user of a managed database is: the policies, which
    /// are forced, hold it.
    Role,
}

/// As whom a test database is made and connected to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    /// The role of [`CONN_STRING_ENV`], which the policies hold.
    HeldByTheWall,
    /// The role of [`SUPERUSER_CONN_STRING_ENV`], which they do not.
    Superuser,
    /// A role made for the one test database, that is no superuser and has
    /// `BYPASSRLS`: the policies do not hold it either.
    BypassRls,
}

#[derive(Debug)]
enum Cleanup {
    SqliteFile(PathBuf),
    PostgresSchema {
        url: String,
        schema: String,
        /// The role that was made with the schema and goes with it.
        role: Option<String>,
    },
}

impl TestDatabase {
    /// Creates an empty database: a new SQLite file under `directory`, or a new schema in
    /// the PostgreSQL database that the environment names.
    ///
    /// # Panics
    ///
    /// Panics if the environment asks for PostgreSQL without giving a URL, or the
    /// database cannot be created. This is test support; a broken setup should fail the
    /// test loudly.
    pub async fn create(directory: &Path) -> Self {
        Self::create_as(directory, Role::HeldByTheWall).await
    }

    /// Creates an empty database as [`TestDatabase::create`] does, but on PostgreSQL as
    /// a superuser, whom no policy holds: for the tests of what the server makes of
    /// such a role, and for nothing else. On SQLite it is a database like any other.
    ///
    /// # Panics
    ///
    /// Panics as [`TestDatabase::create`] does, and if the environment asks for
    /// PostgreSQL without giving the URL of a superuser
    /// ([`SUPERUSER_CONN_STRING_ENV`]).
    pub async fn create_as_superuser(directory: &Path) -> Self {
        Self::create_as(directory, Role::Superuser).await
    }

    /// Creates an empty database as [`TestDatabase::create`] does, but on PostgreSQL as
    /// a role of its own that is no superuser and has `BYPASSRLS`, the other kind of
    /// role that no policy holds. The role is made by the superuser of
    /// [`SUPERUSER_CONN_STRING_ENV`] and removed with the database.
    ///
    /// # Panics
    ///
    /// Panics as [`TestDatabase::create_as_superuser`] does.
    pub async fn create_as_role_with_bypassrls(directory: &Path) -> Self {
        Self::create_as(directory, Role::BypassRls).await
    }

    async fn create_as(directory: &Path, role: Role) -> Self {
        // An identifier has hyphens; a file name takes them, a schema name does not.
        let name = new_id().replace('-', "_");
        let db_type = db_type_of_the_tests();
        match db_type {
            DbType::Sqlite => {
                std::fs::create_dir_all(directory)
                    .unwrap_or_else(|error| panic!("creating {}: {error}", directory.display()));
                let path = directory.join(format!("{name}.db"));
                Self {
                    db_type,
                    database_connection: format!("{}?busy_timeout_ms=5000", path.display()),
                    owner_database_connection: None,
                    cleanup: Cleanup::SqliteFile(path),
                }
            }
            DbType::Postgres => {
                let variable = match role {
                    Role::HeldByTheWall => CONN_STRING_ENV,
                    Role::Superuser | Role::BypassRls => SUPERUSER_CONN_STRING_ENV,
                };
                let url = std::env::var(variable).unwrap_or_else(|_| {
                    panic!("{variable} must hold a PostgreSQL URL for PostgreSQL test runs")
                });
                let schema = format!("t_{name}");
                let mut connection = connect(&url).await;
                let past_the_wall = role_is_past_the_wall(&mut connection).await;
                match role {
                    Role::HeldByTheWall => assert!(
                        !past_the_wall,
                        "{CONN_STRING_ENV} names a superuser or a role with BYPASSRLS. The \
                         tests need a role that is no superuser, has NOBYPASSRLS and has \
                         CREATE on the database; `cargo xtask test` makes one in its container"
                    ),
                    Role::Superuser | Role::BypassRls => assert!(
                        past_the_wall,
                        "{SUPERUSER_CONN_STRING_ENV} names a role the policies hold; the \
                         tests that use it need a superuser"
                    ),
                }
                let (own_role, connect_as) = if role == Role::BypassRls {
                    // The role passes every policy of the cluster: its password is its
                    // own, made here and kept nowhere but in the test's `database_connection`.
                    let own_role = format!("r_{name}");
                    let password = new_id();
                    run(
                        &mut connection,
                        format!(
                            "CREATE ROLE {own_role} LOGIN PASSWORD '{password}' \
                             NOSUPERUSER BYPASSRLS; \
                             CREATE SCHEMA {schema} AUTHORIZATION {own_role}"
                        ),
                    )
                    .await;
                    let connect_as = url_as_role(&url, &own_role, &password);
                    let mut as_the_role = connect(&connect_as).await;
                    let (superuser, bypasses): (bool, bool) = sqlx::query_as(
                        "SELECT rolsuper, rolbypassrls FROM pg_roles WHERE rolname = current_user",
                    )
                    .fetch_one(&mut as_the_role)
                    .await
                    .unwrap_or_else(|error| panic!("reading the role that was made: {error}"));
                    assert!(
                        !superuser && bypasses,
                        "the role that was made is no superuser and has BYPASSRLS"
                    );
                    (Some(own_role), connect_as)
                } else {
                    run(&mut connection, format!("CREATE SCHEMA {schema}")).await;
                    (None, url.clone())
                };
                let separator = if connect_as.contains('?') { '&' } else { '?' };
                Self {
                    db_type,
                    database_connection: format!("{connect_as}{separator}search_path={schema}"),
                    owner_database_connection: None,
                    cleanup: Cleanup::PostgresSchema {
                        url,
                        schema,
                        role: own_role,
                    },
                }
            }
        }
    }

    /// Creates an empty database whose tables are not the tests' role's: on PostgreSQL a
    /// schema that `owner` owns, in which the role of [`CONN_STRING_ENV`] may only use
    /// the tables (`SELECT`, `INSERT`, `UPDATE`, `DELETE`, and the sequences), as a server
    /// is installed apart from the administrator who migrates.
    /// [`TestDatabase::database_connection`] connects as the tests' role and
    /// [`TestDatabase::owner_database_connection`] as the owner. On SQLite, which has no
    /// roles, it is a database like any other.
    ///
    /// # Panics
    ///
    /// Panics as [`TestDatabase::create_as_superuser`] does.
    pub async fn create_owned_apart(directory: &Path, owner: TableOwner) -> Self {
        if db_type_of_the_tests() == DbType::Sqlite {
            return Self::create(directory).await;
        }
        let url_of = |variable: &str| {
            std::env::var(variable).unwrap_or_else(|_| {
                panic!("{variable} must hold a PostgreSQL URL for PostgreSQL test runs")
            })
        };
        let tests_url = url_of(CONN_STRING_ENV);
        let superuser_url = url_of(SUPERUSER_CONN_STRING_ENV);
        let name = new_id().replace('-', "_");
        let schema = format!("t_{name}");
        let tests_role = role_of(&tests_url).await;
        assert!(
            !role_is_past_the_wall(&mut connect(&tests_url).await).await,
            "{CONN_STRING_ENV} names a superuser or a role with BYPASSRLS"
        );
        let mut connection = connect(&superuser_url).await;
        let (owner_role, owner_url, made) = match owner {
            TableOwner::Superuser => {
                let role = role_of(&superuser_url).await;
                (role, superuser_url.clone(), None)
            }
            TableOwner::Role => {
                let role = format!("o_{name}");
                let password = new_id();
                run(
                    &mut connection,
                    format!(
                        "CREATE ROLE {} LOGIN PASSWORD '{password}' NOSUPERUSER \
                         NOBYPASSRLS NOCREATEDB NOCREATEROLE",
                        quoted(&role)
                    ),
                )
                .await;
                let url = url_as_role(&superuser_url, &role, &password);
                (role.clone(), url, Some(role))
            }
        };
        assert_ne!(owner_role, tests_role, "the tables' owner is another role");
        let (owner_role, tests_role) = (quoted(&owner_role), quoted(&tests_role));
        run(
            &mut connection,
            format!(
                "CREATE SCHEMA {schema} AUTHORIZATION {owner_role}; \
                 GRANT USAGE ON SCHEMA {schema} TO {tests_role}; \
                 ALTER DEFAULT PRIVILEGES FOR ROLE {owner_role} IN SCHEMA {schema} \
                   GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO {tests_role}; \
                 ALTER DEFAULT PRIVILEGES FOR ROLE {owner_role} IN SCHEMA {schema} \
                   GRANT USAGE, SELECT ON SEQUENCES TO {tests_role}"
            ),
        )
        .await;
        let in_schema = |url: &str| {
            let separator = if url.contains('?') { '&' } else { '?' };
            format!("{url}{separator}search_path={schema}")
        };
        Self {
            db_type: DbType::Postgres,
            database_connection: in_schema(&tests_url),
            owner_database_connection: Some(in_schema(&owner_url)),
            cleanup: Cleanup::PostgresSchema {
                url: superuser_url,
                schema,
                role: made,
            },
        }
    }

    /// The `database_connection` that connects as the role that owns the tables: that of
    /// a database made by [`TestDatabase::create_owned_apart`], else
    /// [`TestDatabase::database_connection`].
    pub fn owner_database_connection(&self) -> &str {
        self.owner_database_connection
            .as_deref()
            .unwrap_or(&self.database_connection)
    }

    /// The kind of database this is.
    pub fn db_type(&self) -> DbType {
        self.db_type
    }

    /// The `database_connection` that connects to this database.
    pub fn database_connection(&self) -> &str {
        &self.database_connection
    }

    /// The path of the database file, if this is an SQLite database.
    pub fn sqlite_path(&self) -> Option<&Path> {
        match &self.cleanup {
            Cleanup::SqliteFile(path) => Some(path),
            Cleanup::PostgresSchema { .. } => None,
        }
    }

    /// Removes the database. Close every store connected to it first.
    ///
    /// # Panics
    ///
    /// Panics if a PostgreSQL schema cannot be dropped.
    pub async fn remove(self) {
        match self.cleanup {
            Cleanup::SqliteFile(path) => {
                // The log and its index are gone with the last connection that closed;
                // a store that was not closed leaves them, and they go with the file.
                for suffix in ["", "-wal", "-shm"] {
                    let mut name = path.as_os_str().to_owned();
                    name.push(suffix);
                    let _ = std::fs::remove_file(name);
                }
            }
            Cleanup::PostgresSchema { url, schema, role } => {
                let mut connection = connect(&url).await;
                run(&mut connection, format!("DROP SCHEMA {schema} CASCADE")).await;
                if let Some(role) = role {
                    run(&mut connection, format!("DROP ROLE {role}")).await;
                }
            }
        }
    }
}

/// The URL with a role that a test database made for itself and the role's password,
/// in place of those it names.
///
/// # Panics
///
/// Panics if the URL names no role: one of a superuser does.
fn url_as_role(url: &str, role: &str, password: &str) -> String {
    let (scheme, rest) = url
        .split_once("://")
        .unwrap_or_else(|| panic!("{SUPERUSER_CONN_STRING_ENV} is no URL"));
    // The role and its password end at the last `@` before the path or the query.
    let authority_end = rest.find(['/', '?']).unwrap_or(rest.len());
    let at = rest[..authority_end]
        .rfind('@')
        .unwrap_or_else(|| panic!("{SUPERUSER_CONN_STRING_ENV} names no role"));
    format!("{scheme}://{role}:{password}@{}", &rest[at + 1..])
}

/// The role a URL connects as.
async fn role_of(url: &str) -> String {
    sqlx::query_scalar::<_, String>("SELECT current_user::text")
        .fetch_one(&mut connect(url).await)
        .await
        .unwrap_or_else(|error| panic!("reading the role of a test URL: {error}"))
}

/// A role's name as an identifier of a statement.
fn quoted(role: &str) -> String {
    format!("\"{}\"", role.replace('"', "\"\""))
}

async fn connect(url: &str) -> PgConnection {
    PgConnection::connect(url)
        .await
        .unwrap_or_else(|error| panic!("connecting to the test PostgreSQL server: {error}"))
}

/// Runs one statement.
async fn run(connection: &mut PgConnection, statement: String) {
    // The statement is built from a generated identifier, not from input.
    sqlx::raw_sql(sqlx::AssertSqlSafe(statement))
        .execute(connection)
        .await
        .unwrap_or_else(|error| panic!("preparing the test database: {error}"));
}

/// Whether the connection's role passes the row-level security policies: a superuser,
/// or a role with `BYPASSRLS`.
async fn role_is_past_the_wall(connection: &mut PgConnection) -> bool {
    sqlx::query_scalar("SELECT rolsuper OR rolbypassrls FROM pg_roles WHERE rolname = current_user")
        .fetch_one(connection)
        .await
        .unwrap_or_else(|error| panic!("reading the role of the test connection: {error}"))
}

/// Another writer of an SQLite file, as another process with the file open would be:
/// it holds the file for writing, with a change not yet committed, until it lets go.
#[derive(Debug)]
pub struct OtherWriter(sqlx::sqlite::SqliteConnection);

impl OtherWriter {
    /// Opens the file and takes it for writing, as a commit of the rollback journal
    /// holds it: no other connection writes meanwhile.
    ///
    /// # Panics
    ///
    /// Panics if the file does not open or is held by another writer.
    pub async fn hold(database: &Path) -> Self {
        let mut connection =
            sqlx::sqlite::SqliteConnection::connect(&database.display().to_string())
                .await
                .unwrap_or_else(|error| panic!("opening {}: {error}", database.display()));
        sqlx::raw_sql("BEGIN EXCLUSIVE; UPDATE workspaces SET name = name")
            .execute(&mut connection)
            .await
            .unwrap_or_else(|error| panic!("holding {}: {error}", database.display()));
        Self(connection)
    }

    /// Commits and closes.
    ///
    /// # Panics
    ///
    /// Panics if the commit fails.
    pub async fn let_go(mut self) {
        sqlx::raw_sql("COMMIT")
            .execute(&mut self.0)
            .await
            .unwrap_or_else(|error| panic!("committing: {error}"));
        drop(self.0.close().await);
    }
}

/// Removes the record of migration `version` from the schema `database_connection`
/// connects to, as its owner: the schema then reads as behind this build while its
/// tables are unchanged, for the tests of a migration asked for while a server serves it.
///
/// # Panics
///
/// Panics if the database cannot be reached or the record cannot be removed.
pub async fn forget_migration(db_type: DbType, database_connection: &str, version: i64) {
    let store = Store::open(db_type, database_connection)
        .await
        .unwrap_or_else(|error| panic!("connecting to the test database: {error}"));
    store
        .with_write_conn(async |scope| {
            let mut sql = scope.sql("DELETE FROM _sqlx_migrations WHERE version = ");
            sql.bind(version);
            scope.execute(sql).await
        })
        .await
        .unwrap_or_else(|error| panic!("removing the record of migration {version}: {error}"));
    store.close().await;
}

/// The tables that are no organization's; every other table of the schema is behind
/// the wall between organizations.
pub const GLOBAL_TABLES: [&str; 14] = crate::schema::GLOBAL_TABLES;

/// The tables that hold rows of organizations, and the organizations themselves, each
/// behind the wall on PostgreSQL.
pub const ORG_TABLES: [&str; 54] = crate::schema::ORG_TABLES;

/// Takes away again, as the owner of the schema `database_connection` connects to, what
/// the migrations after the baseline made, newest first: the schema is the baseline's.
/// Their record is left as it is.
///
/// # Panics
///
/// Panics if the database cannot be reached or a statement fails.
pub async fn undo_the_migrations_after_the_baseline(db_type: DbType, database_connection: &str) {
    let store = Store::open(db_type, database_connection)
        .await
        .unwrap_or_else(|error| panic!("connecting to the test database: {error}"));
    store
        .with_write_conn(undo_after_the_baseline)
        .await
        .unwrap_or_else(|error| panic!("undoing the migrations after the baseline: {error}"));
    store.close().await;
}

/// The statements of [`crate::schema::UNDONE_FOR_TESTS`], in their order.
async fn undo_after_the_baseline(scope: &mut crate::db::Scope<'_>) -> Result<(), StoreError> {
    for (_, statements) in crate::schema::UNDONE_FOR_TESTS {
        for statement in statements {
            scope.execute(scope.sql(*statement)).await?;
        }
    }
    Ok(())
}

/// Replaces the record of the migrations of the schema `database_connection` connects
/// to, as its owner, with the record the last server before the baseline wrote, every
/// migration of its chain applied, and takes away what the migrations after the
/// baseline made ([`undo_the_migrations_after_the_baseline`]): the database as that
/// server left it, whose schema is the baseline's.
///
/// # Panics
///
/// Panics if the database cannot be reached or the record cannot be written.
pub async fn write_the_record_before_the_baseline(db_type: DbType, database_connection: &str) {
    let store = Store::open(db_type, database_connection)
        .await
        .unwrap_or_else(|error| panic!("connecting to the test database: {error}"));
    let record = crate::schema::record_before_the_baseline(store.dialect());
    store
        .with_write_conn(async |scope| {
            undo_after_the_baseline(scope).await?;
            scope
                .execute(scope.sql("DELETE FROM _sqlx_migrations"))
                .await?;
            for (version, checksum) in record {
                let bytes = (0..checksum.len())
                    .step_by(2)
                    .map(|at| u8::from_str_radix(&checksum[at..at + 2], 16))
                    .collect::<Result<Vec<u8>, _>>()
                    .unwrap_or_else(|error| panic!("the checksum of {version}: {error}"));
                let mut sql = scope.sql(
                    "INSERT INTO _sqlx_migrations \
                     (version, description, success, checksum, execution_time) VALUES (",
                );
                sql.bind(*version)
                    .push(", 'before the baseline', TRUE, ")
                    .bind(crate::db::Param::Bytes(bytes))
                    .push(", 0)");
                scope.execute(sql).await?;
            }
            Ok(())
        })
        .await
        .unwrap_or_else(|error| panic!("writing the record before the baseline: {error}"));
    store.close().await;
}

impl Store {
    /// A handle to the same database whose writes of blocks note nothing to tell
    /// anybody, and make nobody watch anything, not even their writer: for a fixture
    /// that sets the scene of a test, whose writes are nobody's words.
    #[must_use]
    pub fn telling_nobody(&self) -> Self {
        Self {
            noting: crate::db::Noting::Nobody,
            ..self.clone()
        }
    }

    /// What the idle connections of a PostgreSQL store carry of an organization, read
    /// past the store's own transactions: for each connection the settings `fvoci.org`
    /// and `fvoci.system` as it has them, empty for none. Empty on SQLite.
    ///
    /// # Panics
    ///
    /// Panics if a connection cannot be taken or read.
    pub async fn settings_left_on_the_connections(&self) -> Vec<(String, String)> {
        let crate::Pool::Postgres(pool) = &self.inner.pool else {
            return Vec::new();
        };
        // All of them at once, so that each is looked at.
        let mut connections = Vec::new();
        for _ in 0..pool.size() {
            let connection = pool
                .acquire()
                .await
                .unwrap_or_else(|error| panic!("taking a connection: {error}"));
            connections.push(connection);
        }
        let mut settings = Vec::new();
        for connection in &mut connections {
            let left: (String, String) = sqlx::query_as(
                "SELECT COALESCE(current_setting('fvoci.org', true), ''), \
                 COALESCE(current_setting('fvoci.system', true), '')",
            )
            .fetch_one(&mut **connection)
            .await
            .unwrap_or_else(|error| panic!("reading the settings of a connection: {error}"));
            settings.push(left);
        }
        settings
    }

    /// Whether the handle's organization is the instance's own, asked in a transaction
    /// as a change of a role asks it, and how many organizations that transaction
    /// reaches afterwards: for the test that the wall is closed again after it.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a statement fails.
    pub async fn instances_organization_and_reach(&self) -> Result<(bool, i64), StoreError> {
        self.transaction(async |scope| {
            let answer = crate::directory::is_the_instances_organization(scope).await?;
            let sql = scope.sql("SELECT count(*) FROM organizations");
            let reached = scope.fetch_optional(sql).await?;
            Ok((answer, reached.map_or(Ok(0), |row| row.int(0))?))
        })
        .await
    }

    /// What the reach of a transaction of this handle is after it asked, with its wall
    /// open already, whether its organization is the instance's own: for the test that
    /// a read past the wall leaves the reach as it found it. PostgreSQL only, which
    /// has the wall.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a statement fails.
    pub async fn reach_after_asking_with_the_wall_open(&self) -> Result<String, StoreError> {
        self.transaction(async |scope| {
            let wall = crate::directory::open_wall(scope).await?;
            crate::directory::is_the_instances_organization(scope).await?;
            let sql = scope.sql("SELECT current_setting('fvoci.system', true)");
            let reach: String = scope.first(sql).await?.unwrap_or_default();
            crate::directory::close_wall(scope, wall).await?;
            Ok(reach)
        })
        .await
    }

    /// How many organizations the account made, asked in a transaction of this
    /// handle as the start of an organization asks it: for the test of what a handle
    /// that names no organization is answered.
    ///
    /// # Errors
    ///
    /// As `directory::organizations_made_by`.
    pub async fn organizations_made_by(&self, user_id: &str) -> Result<i64, StoreError> {
        self.transaction(async |scope| {
            crate::directory::organizations_made_by(scope, user_id).await
        })
        .await
    }

    /// How many rows of a table this handle reaches, with no condition: for the tests
    /// of the wall between organizations.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the statement fails.
    pub async fn count_rows(&self, table: &str) -> Result<i64, StoreError> {
        self.with_conn(async |scope| {
            let sql = scope.sql_unchecked(&format!("SELECT count(*) FROM {table}"));
            Ok(scope.first(sql).await?.unwrap_or(0))
        })
        .await
    }

    /// Runs one query as this handle does its own, and returns the first column of
    /// every row, which is text (`CAST(… AS TEXT)` for one that is not): for the tests
    /// of what is in a table, such as that no token is. Nothing checks the text.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn texts_unchecked(&self, query: &str) -> Result<Vec<String>, StoreError> {
        self.with_conn(async |scope| {
            let sql = scope.sql_unchecked(query);
            scope.all(sql).await
        })
        .await
    }

    /// Runs one statement as this handle does its own, and returns how many rows it
    /// changed: for the tests of what the database refuses whatever the server asks.
    /// Nothing checks the text.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the statement fails.
    pub async fn execute_unchecked(&self, statement: &str) -> Result<u64, StoreError> {
        self.with_write_conn(async |scope| {
            let sql = scope.sql_unchecked(statement);
            scope.execute(sql).await
        })
        .await
    }
}
