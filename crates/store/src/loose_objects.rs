//! Objects of the file storage that may have no record: those being written, whose
//! record is not there yet, and those whose record went, which are to be removed.
//!
//! A file's object is in the file storage and its record in the database, and no
//! transaction holds both. So an object is listed here before it is written, and the
//! transaction that records it takes it off; and the transaction that removes a record
//! lists its object as doomed. The server's housekeeping removes the doomed objects,
//! trying again after a failure, and dooms the objects whose writing has gone on for
//! longer than any write takes. Whatever stops in between, the list says what is left
//! to do.

use fvoci_model::{FileKey, FileRecord};

use crate::SYSTEM_ORG_ID;
use crate::Store;
use crate::db::{Dialect, Id, OptId, Row, Scope};
use crate::error::StoreError;
use crate::files;

/// What a listed object is waiting for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LooseState {
    /// It is being written, and its record is not there yet.
    Writing,
    /// Its record went, or it never had one: it is to be removed.
    Doomed,
}

impl LooseState {
    /// The value of the `state` column.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Writing => "writing",
            Self::Doomed => "doomed",
        }
    }

    fn parse(text: &str) -> Result<Self, StoreError> {
        match text {
            "writing" => Ok(Self::Writing),
            "doomed" => Ok(Self::Doomed),
            other => Err(StoreError::Corrupt {
                column: "state",
                reason: format!("unknown state {other:?}"),
            }),
        }
    }
}

/// An object of the file storage that is listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LooseObject {
    /// Its place in the file storage.
    pub path: String,
    /// What it is waiting for.
    pub state: LooseState,
    /// When it was listed, in milliseconds since the Unix epoch.
    pub created_at: i64,
    /// When the housekeeping next looks at it.
    pub retry_at: i64,
    /// How many of its removals failed.
    pub attempts: i64,
    /// When its last removal was tried; 0 before the first.
    pub last_attempt_at: i64,
    /// The kind of the error of its last removal; empty before the first failure.
    pub last_error: String,
}

/// How many objects are listed: places, of whichever organizations listed them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LooseCounts {
    /// Being written.
    pub writing: i64,
    /// Doomed: their records went, and they are to be removed.
    pub doomed: i64,
    /// The doomed ones whose removal failed again when it was tried again (twice or
    /// more), by the kind of the error of the last failure, in the order of the kinds.
    pub failing: Vec<(String, i64)>,
}

const LOOSE_COLUMNS: &str =
    "path, state, created_at, retry_at, attempts, last_attempt_at, last_error";

impl Store {
    /// Lists the object about to be written at `path`, with its record not there yet.
    /// From `doom_at` on, the housekeeping takes the object for one whose writing
    /// stopped, and dooms it.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::InvalidPath`] for a place spelled otherwise than the one way,
    /// and [`StoreError::Database`] if the statement fails, as it does when the
    /// organization lists the path already. Another organization's row of the same path
    /// is no hindrance (PostgreSQL; SQLite holds one organization).
    pub async fn mark_writing(&self, path: &str, doom_at: i64) -> Result<(), StoreError> {
        if !fvoci_model::is_storage_path(path) {
            return Err(StoreError::InvalidPath(path.to_owned()));
        }
        self.with_write_conn(async |scope| {
            let mut sql =
                scope.sql("INSERT INTO loose_objects (path, state, created_at, retry_at) VALUES (");
            sql.bind(path)
                .push_bind(", ", LooseState::Writing.as_str())
                .push_bind(", ", scope.now_millis())
                .push_bind(", ", doom_at)
                .push(")");
            scope.execute(sql).await?;
            Ok(())
        })
        .await
    }

    /// Records a file whose object was written at the place it names, taking the mark
    /// of [`Store::mark_writing`] away, in one transaction. Returns whether it was
    /// recorded: if the place was no longer marked, as when the housekeeping doomed it
    /// and may have removed what was there then, nothing is recorded and the object is
    /// doomed (again), for the housekeeping to remove.
    ///
    /// # Errors
    ///
    /// The errors of [`Store::save_file_record`]; nothing is recorded or doomed then.
    pub async fn record_written_file(&self, file: &FileRecord) -> Result<bool, StoreError> {
        files::check_new_path(&file.path)?;
        self.transaction(async |scope| {
            if record_written(scope, file).await? {
                return Ok(true);
            }
            doom(scope, &[(file.path.clone(), None)]).await?;
            Ok(false)
        })
        .await
    }

    /// Dooms the object being written at `path`, for the writer that gave it up, to be
    /// removed from `remove_at` on. Returns whether it was being written: if it was not,
    /// it was recorded or doomed already, and is none of the writer's.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the statement fails.
    pub async fn doom_writing(&self, path: &str, remove_at: i64) -> Result<bool, StoreError> {
        self.with_write_conn(async |scope| {
            let mut sql = scope.sql("UPDATE loose_objects SET state = ");
            sql.bind(LooseState::Doomed.as_str())
                .push_bind(", retry_at = ", remove_at)
                .push_bind(" WHERE path = ", path)
                .push_bind(" AND state = ", LooseState::Writing.as_str());
            Ok(scope.execute(sql).await? > 0)
        })
        .await
    }

    /// Dooms the objects whose writing was to be over by `now`, and returns how many.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the statement fails.
    pub async fn doom_stale_writing(&self, now: i64) -> Result<u64, StoreError> {
        self.with_write_conn(async |scope| {
            let mut sql = scope.sql("UPDATE loose_objects SET state = ");
            sql.bind(LooseState::Doomed.as_str())
                .push_bind(" WHERE state = ", LooseState::Writing.as_str())
                .push_bind(" AND retry_at <= ", now);
            scope.execute(sql).await
        })
        .await
    }

    /// Returns up to `limit` doomed objects whose removal is due at `now`, those due
    /// the longest first.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn due_doomed_objects(
        &self,
        now: i64,
        limit: i64,
    ) -> Result<Vec<LooseObject>, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = scope.sql([
                "SELECT ",
                LOOSE_COLUMNS,
                " FROM loose_objects WHERE state = ",
            ]);
            sql.bind(LooseState::Doomed.as_str())
                .push_bind(" AND retry_at <= ", now)
                .push_bind(" ORDER BY retry_at, path LIMIT ", limit);
            scope
                .fetch_all(sql)
                .await?
                .iter()
                .map(loose_object_from_row)
                .collect()
        })
        .await
    }

    /// Returns every listed object, by path.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn loose_objects(&self) -> Result<Vec<LooseObject>, StoreError> {
        self.with_conn(async |scope| {
            let sql = scope.sql([
                "SELECT ",
                LOOSE_COLUMNS,
                " FROM loose_objects ORDER BY path",
            ]);
            scope
                .fetch_all(sql)
                .await?
                .iter()
                .map(loose_object_from_row)
                .collect()
        })
        .await
    }

    /// Takes a doomed object off the list once it is removed, or found gone, by a run
    /// that read it due at `due_by`. One that is not listed, not doomed, or doomed to
    /// be removed after `due_by` (doomed again meanwhile, as by a writer that wrote at
    /// last) is left as it is.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the statement fails.
    pub async fn forget_doomed_object(&self, path: &str, due_by: i64) -> Result<(), StoreError> {
        self.with_write_conn(async |scope| {
            let mut sql = scope.sql("DELETE FROM loose_objects WHERE path = ");
            sql.bind(path)
                .push_bind(" AND state = ", LooseState::Doomed.as_str())
                .push_bind(" AND retry_at <= ", due_by);
            scope.execute(sql).await?;
            Ok(())
        })
        .await
    }

    /// Notes a failed removal of a doomed object at `now`, with the kind of its error,
    /// and when to try again.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the statement fails.
    pub async fn doomed_object_failed(
        &self,
        path: &str,
        now: i64,
        error_kind: &str,
        retry_at: i64,
    ) -> Result<(), StoreError> {
        self.with_write_conn(async |scope| {
            let mut sql = scope.sql("UPDATE loose_objects SET attempts = attempts + 1, ");
            sql.push_bind("last_attempt_at = ", now)
                .push_bind(", last_error = ", error_kind)
                .push_bind(", retry_at = ", retry_at)
                .push_bind(" WHERE path = ", path)
                .push_bind(" AND state = ", LooseState::Doomed.as_str());
            scope.execute(sql).await?;
            Ok(())
        })
        .await
    }

    /// Dooms objects of the file storage that nothing names, due at once, and returns
    /// how many it listed: one listed already is left as it is. By a handle of an
    /// organization each is the organization's; by one that reaches every organization
    /// (PostgreSQL), each is the organization's whose key it is ([`FileKey`]), and the
    /// system organization's when it is no key of an organization there is.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::InvalidPath`] for a place spelled otherwise than the one way,
    /// or by a handle of an organization for the key of another (nothing is doomed then),
    /// and [`StoreError::Database`] if a statement fails.
    pub async fn doom_unrecorded(&self, paths: &[&str]) -> Result<u64, StoreError> {
        let org = self.tenant().org_id();
        let another_orgs = |path: &str| {
            org.as_deref()
                .is_some_and(|org| FileKey::parse(path).is_some_and(|key| key.org_id != org))
        };
        if let Some(path) = paths
            .iter()
            .find(|path| !fvoci_model::is_storage_path(path) || another_orgs(path))
        {
            return Err(StoreError::InvalidPath((*path).to_owned()));
        }
        self.transaction(async |scope| {
            let names_org = scope.dialect() == Dialect::Postgres && !scope.has_org();
            let key = match scope.dialect() {
                Dialect::Sqlite => "path",
                Dialect::Postgres => "org_id, path",
            };
            let now = scope.now_millis();
            let mut listed = 0;
            for path in paths {
                let mut sql =
                    scope.sql("INSERT INTO loose_objects (path, state, created_at, retry_at");
                sql.push(if names_org { ", org_id" } else { "" })
                    .push_bind(") SELECT ", *path)
                    .push_bind(", ", LooseState::Doomed.as_str())
                    .push_bind(", ", now)
                    .push_bind(", ", now);
                if names_org {
                    let org = OptId(FileKey::parse(path).map_or("", |key| key.org_id));
                    sql.push_bind(", COALESCE((SELECT id FROM organizations WHERE id = ", org)
                        .push_bind("), ", Id(SYSTEM_ORG_ID))
                        .push(")");
                }
                // SQLite reads an upsert after a SELECT only behind a WHERE.
                sql.push([" WHERE true ON CONFLICT (", key, ") DO NOTHING"]);
                listed += scope.execute(sql).await?;
            }
            Ok(listed)
        })
        .await
    }

    /// Counts the listed objects.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn loose_object_counts(&self) -> Result<LooseCounts, StoreError> {
        self.with_conn(async |scope| {
            let sql = scope.sql(
                "SELECT state, CASE WHEN attempts > 1 THEN last_error ELSE '' END, \
                 CAST(COUNT(DISTINCT path) AS BIGINT) FROM loose_objects GROUP BY 1, 2 \
                 ORDER BY 1, 2",
            );
            let mut counts = LooseCounts::default();
            for row in scope.fetch_all(sql).await? {
                let state = LooseState::parse(&row.text(0)?)?;
                let error = row.text(1)?;
                let count = row.int(2)?;
                match state {
                    LooseState::Writing => counts.writing += count,
                    LooseState::Doomed => {
                        counts.doomed += count;
                        if !error.is_empty() {
                            counts.failing.push((error, count));
                        }
                    }
                }
            }
            Ok(counts)
        })
        .await
    }
}

/// How many objects one statement of [`doom`] lists.
const DOOMED_PER_STATEMENT: usize = 500;

/// Records a file whose object was written at the place it names and takes the mark of
/// [`Store::mark_writing`] away, in the caller's transaction. Returns whether the place
/// was still marked; if it was not, nothing is recorded.
pub(crate) async fn record_written(
    scope: &mut Scope<'_>,
    file: &FileRecord,
) -> Result<bool, StoreError> {
    let mut sql = scope.sql("DELETE FROM loose_objects WHERE path = ");
    sql.bind(&file.path)
        .push_bind(" AND state = ", LooseState::Writing.as_str());
    if scope.execute(sql).await? == 0 {
        return Ok(false);
    }
    files::save_file_record(scope, file).await?;
    Ok(true)
}

/// Dooms the objects of records that are being removed, in the caller's transaction,
/// a few hundred to a statement. Each comes with the organization of its record, which
/// a statement of a handle that reaches every organization names (PostgreSQL); one of
/// an organization lists them as its own.
pub(crate) async fn doom(
    scope: &mut Scope<'_>,
    objects: &[(String, Option<String>)],
) -> Result<(), StoreError> {
    let now = scope.now_millis();
    let names_org = scope.dialect() == Dialect::Postgres && !scope.has_org();
    // A statement takes a row once: two records of one place (which only a wrong call
    // of the store makes) are one object to doom.
    let objects: Vec<&(String, Option<String>)> = objects
        .iter()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    for chunk in objects.chunks(DOOMED_PER_STATEMENT) {
        let mut sql = scope.sql("INSERT INTO loose_objects (path, state, created_at, retry_at");
        sql.push(if names_org {
            ", org_id) VALUES "
        } else {
            ") VALUES "
        });
        for (index, &(path, org_id)) in chunk.iter().enumerate() {
            sql.push_bind(if index == 0 { "(" } else { ", (" }, path.as_str())
                .push_bind(", ", LooseState::Doomed.as_str())
                .push_bind(", ", now)
                .push_bind(", ", now);
            if names_org {
                // None is refused by the column: a row is some organization's.
                sql.push_bind(", ", OptId(org_id.as_deref().unwrap_or_default()));
            }
            sql.push(")");
        }
        // A mark of writing of the same place, which no record should outlive, gives
        // way.
        // The key: on PostgreSQL a place is listed once by an organization.
        let key = match scope.dialect() {
            Dialect::Sqlite => "path",
            Dialect::Postgres => "org_id, path",
        };
        sql.push_bind(
            [" ON CONFLICT (", key, ") DO UPDATE SET state = "],
            LooseState::Doomed.as_str(),
        )
        .push(", retry_at = excluded.retry_at");
        scope.execute(sql).await?;
    }
    Ok(())
}

/// The organization of the rows a statement returns, where they carry it: the column
/// to read after the others, on PostgreSQL by a handle that reaches every organization.
pub(crate) fn org_column(scope: &Scope<'_>) -> Option<&'static str> {
    (scope.dialect() == Dialect::Postgres && !scope.has_org()).then_some("org_id")
}

fn loose_object_from_row(row: &Row) -> Result<LooseObject, StoreError> {
    Ok(LooseObject {
        path: row.text(0)?,
        state: LooseState::parse(&row.text(1)?)?,
        created_at: row.int(2)?,
        retry_at: row.int(3)?,
        attempts: row.int(4)?,
        last_attempt_at: row.int(5)?,
        last_error: row.text(6)?,
    })
}
