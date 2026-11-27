//! Bringing a database to this server's schema, and refusing one it cannot use.
//!
//! The schema is a list of migrations per database kind, compiled into the binary and
//! applied in order by sqlx's migrator, which records them in `_sqlx_migrations`. It
//! checks the checksum of every applied migration, so an edited history is noticed; it
//! refuses a database that holds a migration this binary does not know; and on
//! PostgreSQL it takes an advisory lock, so two servers starting on one empty database
//! do not both create it. Migrations only go forward.
//!
//! The two files of a migration describe one change and differ in the column types,
//! in PostgreSQL's wall and in SQLite's triggers only. PostgreSQL keeps the rows of the
//! organizations apart with row-level security, which SQLite does not have: every table
//! of an organization gets its policy there, and SQLite holds one organization by its
//! triggers instead.
//!
//! The first migration is the baseline: the schema the chain of migrations of the
//! servers before it built. A database such a server brought to the end of its chain
//! has that schema already, and only its record is replaced ([`re_anchor`]).

mod inspect;
mod pre_baseline;
mod steps;

use sqlx::SqlSafeStr as _;
use sqlx::migrate::{MigrateError, Migration, MigrationType, Migrator};

pub(crate) use self::inspect::{
    SchemaState, may_change, refuse_unrecorded_schemas, state, ungranted_tables, whereabouts,
};
pub(crate) use self::steps::{STEPS, Step};
use crate::db::{Dialect, Param};
use crate::error::StoreError;
use crate::{Pool, Store};

/// What [`Store::migrate`] would do to a database's schema, as [`Store::check_schema`]
/// finds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaCheck {
    /// Nothing: the schema is this server's.
    Current,
    /// Apply the migrations of this server that the database does not have yet: all of
    /// them, on an empty database.
    Behind,
    /// Replace the record of the migrations, the one the last server before the baseline
    /// wrote, with the baseline's; the schema is not changed.
    BeforeBaseline,
}

/// The tables that are no organization's: the accounts and what is true of the
/// installation. Every other table of the schema is one of [`ORG_TABLES`].
#[cfg(any(test, feature = "test-util"))]
pub(crate) const GLOBAL_TABLES: [&str; 14] = [
    "_sqlx_migrations",
    "api_tokens",
    "data_steps",
    "email_changes",
    "email_reverts",
    "instance",
    "password_resets",
    "search_index_state",
    "sessions",
    "signups",
    "user_client_state",
    "user_identities",
    "user_settings",
    "users",
];

/// The tables that hold rows of organizations, and the organizations themselves. On
/// PostgreSQL each is behind the wall: row-level security enabled and forced, with the
/// policy `org_wall`. A test of the store reads the catalog to see that every table of
/// the schema is in this list or in [`GLOBAL_TABLES`], and that it is so.
#[cfg(any(test, feature = "test-util"))]
pub(crate) const ORG_TABLES: [&str; 51] = [
    "api_token_workspaces",
    "audit_log",
    "blocks",
    "body_docs",
    "body_links",
    "body_updates",
    "body_version_editors",
    "body_versions",
    "card_assignees",
    "card_due_notices",
    "categories",
    "category_containers",
    "comment_versions",
    "container_members",
    "container_teams",
    "containers",
    "course_exams",
    "course_meetings",
    "course_schedules",
    "deleted_blocks",
    "deleted_containers",
    "diary_days",
    "exam_scores",
    "file_refs",
    "file_versions",
    "files",
    "invitations",
    "loose_objects",
    "meeting_attendance",
    "notification_outbox",
    "notifications",
    "oidc_former_providers",
    "oidc_held_sign_ins",
    "oidc_login_flows",
    "oidc_pending_providers",
    "oidc_providers",
    "organization_members",
    "organizations",
    "public_links",
    "search_outbox",
    "subscriptions",
    "team_members",
    "teams",
    "time_entries",
    "unit_notes",
    "unit_progress",
    "unit_reviews",
    "upload_parts",
    "uploads",
    "workspace_members",
    "workspaces",
];

/// The table in which sqlx's migrator records what it applied.
const MIGRATIONS_TABLE: &str = "_sqlx_migrations";

/// A table of this server's schema, by which a database that has tables but no
/// migration record is told from an empty one.
const A_SCHEMA_TABLE: &str = "blocks";

/// Applies the migrations the database does not have yet.
///
/// # Errors
///
/// Returns [`StoreError::Schema`] with the reason if the database holds this server's
/// tables without their migration record, was migrated by a newer server, or has a
/// history that differs from this binary's; and
/// [`StoreError::Database`] if a statement fails.
pub(crate) async fn migrate(store: &Store, steps: &[Step]) -> Result<(), StoreError> {
    store.with_conn(inspect::refuse_unrecorded_schemas).await?;

    let migrator = migrator(store.dialect(), steps);
    let result = match &store.inner.pool {
        Pool::Sqlite(pool) => migrator.run(pool).await,
        Pool::Postgres(pool) => migrator.run(pool).await,
    };
    result.map_err(|error| match error {
        MigrateError::Execute(error) | MigrateError::ExecuteMigration(error, _) => {
            StoreError::Database(error)
        }
        MigrateError::VersionMissing(version) => StoreError::Schema(format!(
            "the database holds schema migration {version}, which this server does not \
             know: it was last opened by a newer server"
        )),
        MigrateError::VersionMismatch(version) => StoreError::Schema(format!(
            "schema migration {version} of the database differs from this server's: the \
             database was created by a different build"
        )),
        other => StoreError::Schema(other.to_string()),
    })
}

/// Replaces the record of a database that the last server before the baseline brought
/// up to date ([`SchemaState::BeforeBaseline`]) with the record of the baseline. Its
/// schema is the baseline's already: no statement changes it, and a server that serves
/// the database meanwhile goes on serving it. Of two that do it at once, the second
/// finds it done: the record is locked (PostgreSQL) or the database is (SQLite's
/// `BEGIN IMMEDIATE`), and read again under the lock.
///
/// # Errors
///
/// Returns [`StoreError::Schema`] with the reason if the record, read again, is not the
/// one before the baseline, and [`StoreError::Database`] if a statement fails; the
/// record is left as it was then.
pub(crate) async fn re_anchor(store: &Store, steps: &[Step]) -> Result<(), StoreError> {
    let migrator = migrator(store.dialect(), steps);
    let Some(baseline) = migrator.iter().next() else {
        return Err(StoreError::Schema(
            "this server has no migration".to_owned(),
        ));
    };
    let anchored = [inspect::Recorded {
        version: baseline.version,
        checksum: baseline.checksum.to_vec(),
        applied: true,
    }];
    store
        .system()
        .transaction(async |scope| {
            if scope.dialect() == Dialect::Postgres {
                // A long transaction that reads the record (a dump) holds the lock off:
                // the opening fails after a while instead of waiting for it.
                let wait = scope.sql(RE_ANCHOR_LOCK_TIMEOUT);
                scope.execute(wait).await?;
                let lock = scope.sql("LOCK TABLE _sqlx_migrations IN ACCESS EXCLUSIVE MODE");
                scope.execute(lock).await?;
            }
            let recorded = inspect::record(scope).await?;
            if recorded == anchored {
                return Ok(());
            }
            match inspect::before_baseline(scope, &recorded).await? {
                Some(SchemaState::BeforeBaseline) => {}
                Some(SchemaState::Foreign(reason)) => return Err(StoreError::Schema(reason)),
                Some(SchemaState::Current | SchemaState::Behind) | None => {
                    return Err(StoreError::Schema(
                        "the record of the schema migrations changed while it was to be \
                         replaced by the baseline's: open the database again"
                            .to_owned(),
                    ));
                }
            }
            scope
                .execute(scope.sql("DELETE FROM _sqlx_migrations"))
                .await?;
            let mut sql = scope.sql(
                "INSERT INTO _sqlx_migrations \
                 (version, description, success, checksum, execution_time) VALUES (",
            );
            sql.bind(baseline.version)
                .push(", ")
                .bind(baseline.description.as_ref())
                .push(", TRUE, ")
                .bind(Param::Bytes(baseline.checksum.to_vec()))
                .push(", 0)");
            scope.execute(sql).await?;
            tracing::info!(
                replaced = recorded.len(),
                "the record of the schema migrations before the baseline was replaced by \
                 the baseline's"
            );
            Ok(())
        })
        .await
}

/// How long the replacement of the record waits for the lock of the record on
/// PostgreSQL, in its transaction alone.
const RE_ANCHOR_LOCK_TIMEOUT: &str = "SET LOCAL lock_timeout = '10s'";

/// The record a database holds when the last server before the baseline brought it up
/// to date, oldest first: each version with the SHA-384 of its file, in lowercase
/// hexadecimal.
#[cfg(any(test, feature = "test-util"))]
pub(crate) fn record_before_the_baseline(dialect: Dialect) -> &'static [(i64, &'static str)] {
    pre_baseline::record(dialect)
}

/// The migrator of the migrations `steps`.
fn migrator(dialect: Dialect, steps: &[Step]) -> Migrator {
    let migrations = steps
        .iter()
        .map(|step| {
            let sql = match dialect {
                Dialect::Sqlite => step.sqlite,
                Dialect::Postgres => step.postgres,
            };
            Migration::new(
                step.version,
                step.description.into(),
                MigrationType::Simple,
                sql.into_sql_str(),
                false,
            )
        })
        .collect();
    Migrator::with_migrations(migrations)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a PostgreSQL migration after the wall begins with if it reads or changes
    /// rows of an organization's table: the migrator's transaction names no
    /// organization, so without it the statements find no row, and say nothing.
    const SYSTEM_SCOPE: &str = "SELECT set_config('fvoci.system', 'on', true);";

    /// The lines of a migration without their comments, the blank ones left out. (No
    /// string in the schema holds `--`.)
    fn statement_lines(sql: &str) -> impl Iterator<Item = &str> {
        sql.lines()
            .map(|line| line.split("--").next().unwrap_or_default().trim())
            .filter(|line| !line.is_empty())
    }

    /// The words of a migration: its runs of letters, digits and underscores, in lower
    /// case, comments left out.
    fn words(sql: &str) -> Vec<String> {
        statement_lines(sql)
            .flat_map(|line| line.split(|c: char| !c.is_ascii_alphanumeric() && c != '_'))
            .filter(|word| !word.is_empty())
            .map(str::to_ascii_lowercase)
            .collect()
    }

    /// The tables of organizations whose rows a migration may read or change: every
    /// one it names anywhere, if it has a statement that reads or changes rows at all.
    /// More than it must be: a migration that changes the rows of one table and the
    /// shape of another names both. A line too many at its head costs nothing; a
    /// statement that finds no row costs the data.
    fn walled_tables(sql: &str) -> Vec<String> {
        let words = words(sql);
        let touches_rows = words
            .iter()
            .any(|word| ["select", "insert", "update", "delete"].contains(&word.as_str()));
        if !touches_rows {
            return Vec::new();
        }
        let mut walled: Vec<String> = words
            .into_iter()
            .filter(|word| ORG_TABLES.contains(&word.as_str()))
            .collect();
        walled.sort_unstable();
        walled.dedup();
        walled
    }

    fn begins_in_the_system_scope(sql: &str) -> bool {
        statement_lines(sql).next() == Some(SYSTEM_SCOPE)
    }

    /// A PostgreSQL migration that reads or changes rows of an organization's table
    /// begins by reaching the rows of all: forced on the owner,
    /// the policies would otherwise leave its `UPDATE` and `DELETE` without a row and
    /// its `INSERT ... SELECT` without a source, with no error.
    #[test]
    fn a_migration_that_touches_an_organizations_rows_begins_in_the_system_scope() {
        for step in &STEPS {
            let walled = walled_tables(step.postgres);
            assert!(
                walled.is_empty() || begins_in_the_system_scope(step.postgres),
                "migration {} ({}) reads or changes rows of {walled:?}; its PostgreSQL file \
                 must begin with the line: {SYSTEM_SCOPE}",
                step.version,
                step.description
            );
        }
    }

    /// The check sees what it is there to see: statements of every kind that touch a
    /// table of an organization, also in a subquery, and not those that touch none or
    /// only change the shape of a table.
    #[test]
    fn the_check_of_later_migrations_sees_what_it_is_there_to_see() {
        for touching in [
            "UPDATE organizations SET name = 'ours'",
            "INSERT INTO data_steps (name) SELECT b.title FROM users AS u, containers AS b;",
            "DELETE FROM sessions USING containers WHERE sessions.user_id = containers.created_by",
            "ALTER TABLE containers ADD COLUMN color TEXT; UPDATE users SET email = ''",
            // More than it must be, and meant: `DELETE` is a word here, not a statement.
            "CREATE TABLE notes (org_id UUID REFERENCES organizations (id) ON DELETE CASCADE)",
            "UPDATE containers SET title = ''",
            "DELETE FROM deleted_blocks WHERE deleted_at < 5;",
            "INSERT INTO file_refs (block_id) VALUES ('x')",
            "INSERT INTO data_steps (name) SELECT title FROM containers;",
            "update users set display_name = (select title from containers limit 1)",
            "CREATE TABLE copies AS SELECT b.id FROM blocks AS b JOIN workspaces t ON true",
            "-- a comment\nUPDATE users SET email = '' WHERE id IN (SELECT user_id FROM container_members)",
        ] {
            assert!(!walled_tables(touching).is_empty(), "{touching}");
            assert!(!begins_in_the_system_scope(touching), "{touching}");
        }
        for not_touching in [
            "ALTER TABLE containers ADD COLUMN color TEXT NOT NULL DEFAULT '';",
            "CREATE INDEX blocks_title_idx ON blocks (title);",
            "CREATE TABLE notes (id UUID PRIMARY KEY, container_id UUID REFERENCES containers (id));",
            "INSERT INTO data_steps (name) VALUES ('later');",
            "UPDATE users SET display_name = '' -- not from containers",
            "UPDATE users SET updated_at = (EXTRACT(EPOCH FROM now()) * 1000)::BIGINT;",
            "DELETE FROM sessions WHERE user_id IN (SELECT id FROM (SELECT id FROM users) AS u)",
        ] {
            assert_eq!(
                walled_tables(not_touching),
                Vec::<String>::new(),
                "{not_touching}"
            );
        }
        let scoped = format!("-- why\n{SYSTEM_SCOPE}\nUPDATE containers SET title = '';");
        assert!(begins_in_the_system_scope(&scoped), "comments come first");
        assert!(
            !begins_in_the_system_scope(&format!(
                "UPDATE containers SET title = '';\n{SYSTEM_SCOPE}"
            )),
            "the first statement, not a later one"
        );
    }

    /// The record before the baseline is never taken for the baseline's: its first
    /// migration is another file. Its versions go up, as the migrator wrote them.
    #[test]
    fn the_record_before_the_baseline_is_not_the_baselines() {
        for dialect in [Dialect::Sqlite, Dialect::Postgres] {
            let record = pre_baseline::record(dialect);
            let baseline = migrator(dialect, &STEPS);
            let Some(first) = baseline.iter().next() else {
                panic!("{dialect:?}: no baseline");
            };
            assert_eq!(record.first().map(|row| row.0), Some(first.version));
            assert_ne!(
                record.first().map(|row| row.1.to_owned()),
                Some(inspect::hexadecimal(&first.checksum)),
                "{dialect:?}"
            );
            assert!(
                record.windows(2).all(|pair| pair[0].0 < pair[1].0),
                "{dialect:?}: the versions go up"
            );
        }
    }

    /// No table is both an organization's and nobody's.
    #[test]
    fn the_two_lists_of_tables_share_no_table() {
        for table in ORG_TABLES {
            assert!(!GLOBAL_TABLES.contains(&table), "{table} is in both lists");
        }
    }

    /// The baseline, and a migration after it as a later server has one.
    fn baseline_and_one_more() -> [Step; 2] {
        let [baseline] = &STEPS;
        [
            Step {
                version: baseline.version,
                description: baseline.description,
                sqlite: baseline.sqlite,
                postgres: baseline.postgres,
            },
            Step {
                version: 2,
                description: "after the baseline",
                sqlite: "CREATE TABLE after_the_baseline (id INTEGER PRIMARY KEY);",
                postgres: "CREATE TABLE after_the_baseline (id BIGINT PRIMARY KEY);",
            },
        ]
    }

    /// The bytes of lowercase hexadecimal, as the record before the baseline has them.
    fn bytes_of(hexadecimal: &str) -> Vec<u8> {
        (0..hexadecimal.len())
            .step_by(2)
            .map(|at| u8::from_str_radix(&hexadecimal[at..at + 2], 16).expect("hexadecimal"))
            .collect()
    }

    /// A database of the baseline whose record is the one the last server before the
    /// baseline left, as a database that server brought up to date is.
    async fn before_the_baseline() -> Store {
        let store = Store::open(crate::DbType::Sqlite, ":memory:")
            .await
            .expect("the database opens");
        store.migrate().await.expect("the baseline is applied");
        store
            .system()
            .transaction(async |scope| {
                scope
                    .execute(scope.sql("DELETE FROM _sqlx_migrations"))
                    .await?;
                for (version, checksum) in pre_baseline::record(Dialect::Sqlite) {
                    let mut sql = scope.sql(
                        "INSERT INTO _sqlx_migrations \
                         (version, description, success, checksum, execution_time) VALUES (",
                    );
                    sql.bind(*version)
                        .push(", 'before the baseline', TRUE, ")
                        .bind(Param::Bytes(bytes_of(checksum)))
                        .push(", 0)");
                    scope.execute(sql).await?;
                }
                Ok(())
            })
            .await
            .expect("the record before the baseline is written");
        store
    }

    /// The versions the record of the database holds, oldest first.
    async fn recorded_versions(store: &Store) -> Vec<i64> {
        store
            .system()
            .transaction(async |scope| inspect::record(scope).await)
            .await
            .expect("the record is read")
            .iter()
            .map(|recorded| recorded.version)
            .collect()
    }

    /// Once a server has migrations after the baseline, one migration of a database
    /// before the baseline replaces its record and then applies them: neither a
    /// server's start nor `--migrate` leaves it behind until the next.
    #[tokio::test]
    async fn a_database_before_the_baseline_takes_the_migrations_after_it_at_once() {
        let steps = baseline_and_one_more();
        let store = before_the_baseline().await;
        assert_eq!(
            store.check_schema().await.expect("the record is read"),
            SchemaCheck::BeforeBaseline
        );
        store.migrate_with(&steps).await.expect("migrated");
        assert_eq!(recorded_versions(&store).await, [1, 2]);
        assert_eq!(
            state(&store, &steps).await.expect("the record is read"),
            SchemaState::Current
        );
        store
            .system()
            .transaction(async |scope| {
                scope
                    .execute(scope.sql("INSERT INTO after_the_baseline (id) VALUES (1)"))
                    .await
            })
            .await
            .expect("the table of the migration after the baseline is there");
        store.close().await;
    }

    /// The replacement of the record alone, which `--migrate` makes while a server
    /// serves the database, leaves the migrations after the baseline to be applied, as
    /// it says: those change tables, and are applied under the server's lock.
    #[tokio::test]
    async fn the_record_replaced_alone_leaves_the_migrations_after_the_baseline() {
        let steps = baseline_and_one_more();
        let store = before_the_baseline().await;
        assert_eq!(
            store.re_anchor_with(&steps).await.expect("replaced"),
            SchemaCheck::Behind
        );
        assert_eq!(recorded_versions(&store).await, [1]);
        assert_eq!(
            store.re_anchor_with(&STEPS).await.expect("read again"),
            SchemaCheck::Current,
            "this server's: nothing is left"
        );
        store.close().await;
    }
}
