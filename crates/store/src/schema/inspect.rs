//! Reading a database's migration record and role privileges before running or skipping
//! migrations.

use super::{A_SCHEMA_TABLE, MIGRATIONS_TABLE, Step, migrator, pre_baseline};
use crate::Store;
use crate::db::{Dialect, Scope};
use crate::error::StoreError;

/// Where a database's schema stands against this build's migrations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SchemaState {
    /// Every migration of this build is recorded as applied, with this build's text,
    /// and no other: the database is opened without a statement that changes it.
    Current,
    /// Some of this build's migrations are not recorded yet (none, if the database is
    /// empty); what is recorded is this build's.
    Behind,
    /// The record is the one the last server before the baseline wrote: every migration
    /// of its chain, each with its checksum and applied, and no change of the data that
    /// follows them left to do. The schema is the baseline's already; only the record is
    /// to be replaced ([`super::re_anchor`]).
    BeforeBaseline,
    /// The record is not this build's: why, as the migrator would say it.
    Foreign(String),
}

/// A row of the record of the migrations.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Recorded {
    pub(super) version: i64,
    /// The SHA-384 of the migration's file.
    pub(super) checksum: Vec<u8>,
    /// Whether it was applied: `false` for one that failed.
    pub(super) applied: bool,
}

/// Reads where the database's schema stands, from the record of the migrations.
///
/// # Errors
///
/// Returns [`StoreError::Schema`] if the record is there but the role may not read it
/// (it was not granted the tables), and [`StoreError::Database`] if it cannot be read.
pub(crate) async fn state(store: &Store, steps: &[Step]) -> Result<SchemaState, StoreError> {
    let migrator = migrator(store.dialect(), steps);
    let known: Vec<(i64, &[u8])> = migrator
        .iter()
        .map(|migration| (migration.version, migration.checksum.as_ref()))
        .collect();
    store
        .with_conn(async |scope| {
            if !has_record(scope).await? {
                return Ok(SchemaState::Behind);
            }
            let recorded = record(scope).await?;
            if let Some(state) = before_baseline(scope, &recorded).await? {
                return Ok(state);
            }
            for Recorded {
                version,
                checksum,
                applied,
            } in &recorded
            {
                let reason = match known.iter().find(|(known, _)| known == version) {
                    None => format!(
                        "the database holds schema migration {version}, which this server \
                         does not know: it was last opened by a newer server"
                    ),
                    Some((_, known)) if *known != checksum.as_slice() => format!(
                        "schema migration {version} of the database differs from this \
                         server's: the database was created by a different build"
                    ),
                    Some(_) if !applied => {
                        format!("schema migration {version} failed when it was applied")
                    }
                    Some(_) => continue,
                };
                return Ok(SchemaState::Foreign(reason));
            }
            Ok(if recorded.len() == known.len() {
                SchemaState::Current
            } else {
                SchemaState::Behind
            })
        })
        .await
}

/// The record of the migrations, oldest first.
pub(super) async fn record(scope: &mut Scope<'_>) -> Result<Vec<Recorded>, StoreError> {
    let sql = scope.sql("SELECT version, checksum, success FROM _sqlx_migrations ORDER BY version");
    scope
        .fetch_all(sql)
        .await?
        .iter()
        .map(|row| {
            Ok(Recorded {
                version: row.int(0)?,
                checksum: row.bytes(1)?,
                applied: row.bool(2)?,
            })
        })
        .collect()
}

/// Where a record that the servers before the baseline wrote stands: [`None`] for a
/// record that is not one of theirs, whose migration 1 is not the first file of their
/// chain. Theirs is [`SchemaState::BeforeBaseline`] only if it is the whole record of
/// the last of them and that server did what it does to the data after its migrations;
/// anything less is refused, with what to do, because the baseline holds the schema of
/// the end of the chain alone.
pub(super) async fn before_baseline(
    scope: &mut Scope<'_>,
    recorded: &[Recorded],
) -> Result<Option<SchemaState>, StoreError> {
    let chain = pre_baseline::record(scope.dialect());
    let (Some(first), Some(chain_first), Some(chain_last)) =
        (recorded.first(), chain.first(), chain.last())
    else {
        return Ok(None);
    };
    if (first.version, hexadecimal(&first.checksum).as_str()) != *chain_first {
        return Ok(None);
    }
    let last = chain_last.0;
    if let Some(gap) = gap_from_the_chain(recorded, chain) {
        return Ok(Some(SchemaState::Foreign(format!(
            "the database's record of schema migrations is the one of a server before the \
             baseline, but {gap}: open it with the last server before the baseline first \
             (tag pre-baseline), which brings it to migration {last}, then with this one"
        ))));
    }
    let pending = pending_data_steps(scope).await?;
    if !pending.is_empty() {
        return Ok(Some(SchemaState::Foreign(format!(
            "the database's schema is the one before the baseline, but the changes of the \
             data that follow its migrations are not done ({}): open it with the last server \
             before the baseline first (tag pre-baseline), then with this one",
            pending.join(", ")
        ))));
    }
    Ok(Some(SchemaState::BeforeBaseline))
}

/// How a record of the servers before the baseline falls short of the whole chain of
/// the last of them, in the words of the refusal; [`None`] if it is that chain.
fn gap_from_the_chain(recorded: &[Recorded], chain: &[(i64, &str)]) -> Option<String> {
    for (index, (version, checksum)) in chain.iter().enumerate() {
        let Some(row) = recorded.get(index) else {
            let reached = recorded.last().map_or(0, |row| row.version);
            return Some(format!("it ends at migration {reached}"));
        };
        if row.version != *version {
            return Some(format!(
                "it holds migration {} where that server has migration {version}",
                row.version
            ));
        }
        if hexadecimal(&row.checksum) != *checksum {
            return Some(format!(
                "its migration {version} differs from that server's"
            ));
        }
        if !row.applied {
            return Some(format!(
                "its migration {version} failed when it was applied"
            ));
        }
    }
    recorded.get(chain.len()).map(|row| {
        format!(
            "it holds migration {}, which that server does not know",
            row.version
        )
    })
}

/// The changes of the data that a migration before the baseline left to the server and
/// that are not done yet, by name.
async fn pending_data_steps(scope: &mut Scope<'_>) -> Result<Vec<String>, StoreError> {
    let sql = scope.sql("SELECT name FROM data_steps ORDER BY name");
    scope
        .fetch_all(sql)
        .await?
        .iter()
        .map(|row| row.text(0))
        .collect()
}

/// Bytes in lowercase hexadecimal, as the record of the servers before the baseline is
/// written.
pub(super) fn hexadecimal(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut text, byte| {
            use std::fmt::Write as _;
            write!(text, "{byte:02x}").expect("a String takes what is written to it");
            text
        })
}

/// Whether the record of the migrations is in the schema the role works in. On
/// PostgreSQL it is found whatever the role was granted, and a record the role may not
/// read is an error that says what to grant.
async fn has_record(scope: &mut Scope<'_>) -> Result<bool, StoreError> {
    if scope.dialect() == Dialect::Sqlite {
        return Ok(table_names(scope)
            .await?
            .iter()
            .any(|table| table == MIGRATIONS_TABLE));
    }
    let sql = scope.sql(
        "SELECT record IS NOT NULL, COALESCE(has_table_privilege(record, 'SELECT'), false), \
           current_user::text, COALESCE(current_schema()::text, '') \
         FROM (SELECT to_regclass(quote_ident(current_schema()) || '._sqlx_migrations') \
           AS record) AS found",
    );
    let Some(row) = scope.fetch_optional(sql).await? else {
        return Ok(false);
    };
    let (found, readable) = (row.bool(0)?, row.bool(1)?);
    if found && !readable {
        return Err(StoreError::Schema(format!(
            "the role {} may not read the tables of the schema {}: grant it SELECT, INSERT, \
             UPDATE and DELETE on them, and USAGE and SELECT on the sequences, also by default \
             for the tables the role that migrates makes later",
            row.text(2)?,
            row.text(3)?
        )));
    }
    Ok(found)
}

/// Whether the role the store connects as may change the schema: on PostgreSQL, the
/// role that owns the record of the migrations (and so the tables the migrations made),
/// a superuser, or, on an empty schema, a role that may make tables in it. A member of
/// the owner's role is not taken for it: what it made would be its own. SQLite has no
/// roles.
///
/// # Errors
///
/// Returns [`StoreError::Database`] if the catalog cannot be read.
pub(crate) async fn may_change(store: &Store) -> Result<bool, StoreError> {
    if store.dialect() == Dialect::Sqlite {
        return Ok(true);
    }
    store
        .with_conn(async |scope| {
            let sql = scope.sql(
                "SELECT COALESCE(( \
                   SELECT CASE WHEN record IS NULL \
                     THEN has_schema_privilege(current_schema(), 'CREATE') \
                     ELSE (SELECT rolsuper FROM pg_roles WHERE rolname = current_user) \
                       OR (SELECT relowner FROM pg_class WHERE oid = record) \
                         = (SELECT oid FROM pg_roles WHERE rolname = current_user) END \
                   FROM (SELECT to_regclass(quote_ident(current_schema()) \
                     || '._sqlx_migrations') AS record) AS found), false)",
            );
            match scope.fetch_optional(sql).await? {
                Some(row) => row.bool(0),
                None => Ok(false),
            }
        })
        .await
}

/// The role and the schema the store works as and in, for what it says: on SQLite,
/// none.
pub(crate) async fn whereabouts(store: &Store) -> Result<(String, String), StoreError> {
    if store.dialect() == Dialect::Sqlite {
        return Ok((String::new(), String::new()));
    }
    store
        .with_conn(async |scope| {
            let sql =
                scope.sql("SELECT current_user::text, COALESCE(current_schema()::text, '(none)')");
            match scope.fetch_optional(sql).await? {
                Some(row) => Ok((row.text(0)?, row.text(1)?)),
                None => Ok((String::new(), String::new())),
            }
        })
        .await
}

/// The tables of the schema the role may not read and write all of (of the record of
/// the migrations, read): a role that does not own them, on a schema that is up to
/// date, would otherwise start and fail its requests (the default privileges cover only
/// the tables made after them).
///
/// # Errors
///
/// Returns [`StoreError::Database`] if the catalog cannot be read.
pub(crate) async fn ungranted_tables(store: &Store) -> Result<Vec<String>, StoreError> {
    if store.dialect() == Dialect::Sqlite {
        return Ok(Vec::new());
    }
    store
        .with_conn(async |scope| {
            let sql = scope.sql(
                "SELECT c.relname::text FROM pg_class c \
                 WHERE c.relnamespace = (SELECT oid FROM pg_namespace \
                   WHERE nspname = current_schema()) \
                 AND c.relkind IN ('r', 'p') \
                 AND NOT (has_table_privilege(c.oid, 'SELECT') \
                   AND (c.relname = '_sqlx_migrations' \
                     OR (has_table_privilege(c.oid, 'INSERT') \
                       AND has_table_privilege(c.oid, 'UPDATE') \
                       AND has_table_privilege(c.oid, 'DELETE')))) \
                 ORDER BY c.relname",
            );
            scope
                .fetch_all(sql)
                .await?
                .iter()
                .map(|row| row.text(0))
                .collect()
        })
        .await
}

/// Refuses a database that has this server's tables but no record of the migrations
/// that made them, which the migrator would try to create again.
pub(crate) async fn refuse_unrecorded_schemas(scope: &mut Scope<'_>) -> Result<(), StoreError> {
    let tables = table_names(scope).await?;
    let has = |name: &str| tables.iter().any(|table| table == name);
    if has(A_SCHEMA_TABLE) && !has(MIGRATIONS_TABLE) {
        return Err(StoreError::Schema(format!(
            "the database has a {A_SCHEMA_TABLE} table but no record of schema migrations \
             ({MIGRATIONS_TABLE}); start with an empty database"
        )));
    }
    Ok(())
}

/// The names of the tables in the database (on PostgreSQL: in the current schema).
async fn table_names(scope: &mut Scope<'_>) -> Result<Vec<String>, StoreError> {
    let sql = match scope.dialect() {
        Dialect::Sqlite => scope.sql("SELECT name FROM sqlite_master WHERE type = 'table'"),
        Dialect::Postgres => scope.sql(
            "SELECT table_name::text FROM information_schema.tables \
             WHERE table_schema = current_schema()",
        ),
    };
    scope
        .fetch_all(sql)
        .await?
        .iter()
        .map(|row| row.text(0))
        .collect()
}
