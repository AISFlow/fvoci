//! Records about uploaded files.
//!
//! A file is one board's. It is stored and asked for under a name the server made up,
//! and the record ties that name to the board, to the place in the file storage and to
//! the name the file was uploaded with.

use fvoci_model::FileRecord;

use crate::Store;
use crate::db::{Dialect, Id, OptId, Row, Scope, SqlFragment};
use crate::error::StoreError;

pub(crate) const FILE_COLUMNS: &str =
    "id, container_id, name, original_name, extension, size, path, created_at, version, created_by";

/// How many columns [`FILE_COLUMNS`] names: the index of a column read after them.
pub(crate) const FILE_COLUMN_COUNT: usize = 10;

impl Store {
    /// Returns the record of the file a board has under a stored name.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if the board has no file of that name. A file of
    /// another board is not found.
    pub async fn get_file_record(
        &self,
        container_id: &str,
        stored_name: &str,
    ) -> Result<FileRecord, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = scope.sql(["SELECT ", FILE_COLUMNS, " FROM files WHERE container_id = "]);
            sql.bind(Id(container_id))
                .push_bind(" AND name = ", stored_name);
            let row = scope
                .fetch_optional(sql)
                .await?
                .ok_or_else(|| StoreError::not_found("file"))?;
            file_from_row(&row)
        })
        .await
    }

    /// Returns every file record of a container, oldest first, in a single query.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn get_files_of_container(
        &self,
        container_id: &str,
    ) -> Result<Vec<FileRecord>, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = scope.sql(["SELECT ", FILE_COLUMNS, " FROM files WHERE container_id = "]);
            sql.bind(Id(container_id)).push(" ORDER BY created_at, id");
            scope
                .fetch_all(sql)
                .await?
                .iter()
                .map(file_from_row)
                .collect()
        })
        .await
    }

    /// Returns the organization of a board or wiki, which the keys of its files begin
    /// with ([`fvoci_model::FileKey`]): that of its workspace.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if the handle reaches no such container, and
    /// [`StoreError::Database`] if the query fails.
    pub async fn org_of_container(&self, container_id: &str) -> Result<String, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = scope.sql(
                "SELECT w.org_id FROM containers AS c JOIN workspaces AS w \
                 ON w.id = c.workspace_id WHERE c.id = ",
            );
            sql.bind(Id(container_id));
            let row = scope
                .fetch_optional(sql)
                .await?
                .ok_or_else(|| StoreError::not_found("board"))?;
            row.id(0)
        })
        .await
    }

    /// Stores a record about a file.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::InvalidId`] if the record's or the board's identifier is not
    /// one, [`StoreError::InvalidPath`] for a place spelled otherwise than the one way,
    /// [`StoreError::NotFound`] if the board does not exist, and
    /// [`StoreError::Database`] if the board has a file of that stored name already.
    pub async fn save_file_record(&self, file: &FileRecord) -> Result<(), StoreError> {
        check_new_path(&file.path)?;
        self.with_write_conn(async |scope| save_file_record(scope, file).await)
            .await
    }

    /// Returns the total bytes stored or held open for the handle's organization across
    /// live `files`, archived `file_versions`, and open `uploads`.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names no organization, and
    /// [`StoreError::Database`] if the query fails.
    pub async fn organization_storage_bytes(&self) -> Result<i64, StoreError> {
        self.with_conn(async |scope| {
            let org = scope.org()?;
            let sql = match scope.dialect() {
                Dialect::Postgres => scope.sql(
                    "SELECT CAST(\
                     COALESCE((SELECT SUM(size) FROM files), 0) + \
                     COALESCE((SELECT SUM(size) FROM file_versions), 0) + \
                     COALESCE((SELECT SUM(size) FROM uploads WHERE closed_at = 0), 0) \
                     AS BIGINT)",
                ),
                Dialect::Sqlite => {
                    let mut sql = scope.sql(
                        "SELECT \
                         COALESCE((SELECT SUM(f.size) FROM files AS f \
                           JOIN containers AS c ON c.id = f.container_id \
                           JOIN workspaces AS w ON w.id = c.workspace_id \
                           WHERE w.org_id = ",
                    );
                    sql.bind(org.clone())
                        .push_bind(
                            "), 0) + \
                             COALESCE((SELECT SUM(fv.size) FROM file_versions AS fv \
                               JOIN files AS f ON f.id = fv.file_id \
                               JOIN containers AS c ON c.id = f.container_id \
                               JOIN workspaces AS w ON w.id = c.workspace_id \
                               WHERE w.org_id = ",
                            org.clone(),
                        )
                        .push_bind(
                            "), 0) + \
                             COALESCE((SELECT SUM(u.size) FROM uploads AS u \
                               WHERE u.closed_at = 0 AND (\
                                 u.container_id IN (SELECT c.id FROM containers AS c \
                                   JOIN workspaces AS w ON w.id = c.workspace_id \
                                   WHERE w.org_id = ",
                            org.clone(),
                        )
                        .push_bind(
                            ") OR u.container_id IN (SELECT w.id FROM workspaces AS w \
                               WHERE w.org_id = ",
                            org,
                        )
                        .push("))), 0)");
                    sql
                }
            };
            match scope.fetch_optional(sql).await? {
                Some(row) => row.int(0),
                None => Ok(0),
            }
        })
        .await
    }
}

impl Store {
    /// Returns up to `limit` places that file records name, live or kept with a board in
    /// the trash, after `after` in their order: a page of every record the handle
    /// reaches, for the comparison of the records with the file storage.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn recorded_paths_after(
        &self,
        after: &str,
        limit: i64,
    ) -> Result<Vec<String>, StoreError> {
        self.with_conn(async |scope| {
            let (trashed_path, trashed_files) = match scope.dialect() {
                Dialect::Sqlite => ("json_extract(f.value, '$.path')", "json_each(d.files) AS f"),
                Dialect::Postgres => (
                    "f.value ->> 'path'",
                    "json_array_elements(d.files) AS f(value)",
                ),
            };
            // Each side takes its own page first, so that the live records are read a
            // page at a time by their index; the trash has none to read by.
            let mut sql =
                scope.sql("SELECT path FROM (SELECT * FROM (SELECT path FROM files WHERE path > ");
            sql.bind(after)
                .push_bind(" ORDER BY path LIMIT ", limit)
                .push([
                    ") AS live UNION SELECT * FROM (SELECT ",
                    trashed_path,
                    " AS path FROM deleted_containers AS d, ",
                    trashed_files,
                    " WHERE ",
                    trashed_path,
                    " > ",
                ])
                .bind(after)
                .push_bind(" ORDER BY 1 LIMIT ", limit)
                // The bytes of the versions before a file's newest, which no record names.
                .push(") AS trashed UNION SELECT * FROM (SELECT path FROM file_versions WHERE path > ")
                .bind(after)
                .push_bind(" ORDER BY path LIMIT ", limit)
                .push_bind(") AS versions) AS recorded ORDER BY path LIMIT ", limit);
            scope
                .fetch_all(sql)
                .await?
                .iter()
                .map(|row| row.text(0))
                .collect()
        })
        .await
    }
}

/// Refuses a place of the file storage that a new record, mark or session would name
/// otherwise than the one way ([`fvoci_model::is_storage_path`]); the empty place of a
/// record or session that names none passes. A record the store held already, as one
/// that comes back from the trash, is taken as it was.
pub(crate) fn check_new_path(path: &str) -> Result<(), StoreError> {
    if path.is_empty() || fvoci_model::is_storage_path(path) {
        Ok(())
    } else {
        Err(StoreError::InvalidPath(path.to_owned()))
    }
}

/// The places that rows name otherwise than the one way: those that a rough condition
/// finds, checked with [`fvoci_model::is_storage_path`].
pub(crate) async fn other_spellings(scope: &mut Scope<'_>) -> Result<Vec<String>, StoreError> {
    let (trashed_path, trashed_files, bs_fn, bs_arg) = match scope.dialect() {
        Dialect::Sqlite => (
            "json_extract(f.value, '$.path')",
            "json_each(d.files) AS f",
            "instr(",
            ", char(92)) > 0",
        ),
        Dialect::Postgres => (
            "f.value ->> 'path'",
            "json_array_elements(d.files) AS f(value)",
            "strpos(",
            ", chr(92)) > 0",
        ),
    };
    let rough = |p: &'static str| -> SqlFragment {
        SqlFragment::of(&[
            p,
            " <> '' AND (",
            p,
            " LIKE '%//%' OR ",
            p,
            " LIKE '/%' OR ",
            p,
            " LIKE '%/' OR ",
            p,
            " LIKE '.%' OR ",
            p,
            " LIKE '%/.%' OR ",
            bs_fn,
            p,
            bs_arg,
            ")",
        ])
    };
    let rough_path = rough("path");
    let rough_trashed = rough(trashed_path);
    let mut statement = SqlFragment::of(&["SELECT path FROM files WHERE "]);
    statement
        .push_fragment(&rough_path)
        .push(" UNION SELECT path FROM uploads WHERE ")
        .push_fragment(&rough_path)
        .push(" UNION SELECT path FROM loose_objects WHERE ")
        .push_fragment(&rough_path)
        .push(" UNION SELECT path FROM file_versions WHERE ")
        .push_fragment(&rough_path)
        .push(" UNION SELECT ")
        .push(trashed_path)
        .push(" FROM deleted_containers AS d, ")
        .push(trashed_files)
        .push(" WHERE ")
        .push_fragment(&rough_trashed);
    let sql = scope.sql(&statement);
    let mut found = Vec::new();
    for row in scope.fetch_all(sql).await? {
        let path = row.text(0)?;
        if !fvoci_model::is_storage_path(&path) {
            found.push(path);
        }
    }
    Ok(found)
}

pub(crate) async fn save_file_record(
    scope: &mut Scope<'_>,
    file: &FileRecord,
) -> Result<(), StoreError> {
    // A new file is shown by no block until one is written that shows it.
    let mut sql = scope.sql([
        "INSERT INTO files (",
        FILE_COLUMNS,
        ", unreferenced_at) VALUES (",
    ]);
    sql.bind(Id(&file.id))
        .push_bind(", ", Id(&file.container_id))
        .push_bind(", ", &file.stored_name)
        .push_bind(", ", &file.name)
        .push_bind(", ", &file.extension)
        .push_bind(", ", file.size)
        .push_bind(", ", &file.path)
        .push_bind(", ", file.created_at)
        .push_bind(", ", file.version)
        .push_bind(", ", OptId(&file.created_by))
        .push_bind(", ", file.created_at)
        .push(")");
    scope
        .execute(sql)
        .await
        .map_err(|error| error.missing_reference("board"))?;
    Ok(())
}

/// The records of every file of a board, oldest first, held on PostgreSQL until the
/// transaction ends: the board's deletion takes them to the trash, and a collection of
/// the files no block shows skips them meanwhile (PostgreSQL); or it took one before,
/// and that one is not taken to the trash.
pub(crate) async fn files_of_container(
    scope: &mut Scope<'_>,
    container_id: &str,
) -> Result<Vec<FileRecord>, StoreError> {
    let mut sql = scope.sql(["SELECT ", FILE_COLUMNS, " FROM files WHERE container_id = "]);
    sql.bind(Id(container_id)).push(" ORDER BY created_at, id");
    if scope.dialect() == Dialect::Postgres {
        sql.push(" FOR UPDATE");
    }
    scope
        .fetch_all(sql)
        .await?
        .iter()
        .map(file_from_row)
        .collect()
}

pub(crate) fn file_from_row(row: &Row) -> Result<FileRecord, StoreError> {
    Ok(FileRecord {
        id: row.id(0)?,
        container_id: row.id(1)?,
        stored_name: row.text(2)?,
        name: row.text(3)?,
        extension: row.text(4)?,
        size: row.int(5)?,
        path: row.text(6)?,
        created_at: row.int(7)?,
        version: row.int(8)?,
        created_by: row.id(9)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_count_of_the_columns_is_the_columns() {
        assert_eq!(
            FILE_COLUMNS.split(',').count(),
            FILE_COLUMN_COUNT,
            "{FILE_COLUMNS}"
        );
    }
}
