//! The versions of a file (`file_versions`): the bytes it had before it was written
//! anew, kept to be read and gone back to.
//!
//! A file's record names its newest version: where its bytes are, their size, and the
//! number of the version, from 1. A file that was never written anew has no row here:
//! its record is its only version. The write that gives it a second one puts down a
//! row for the first too, and from then on the file has a row per version kept, the
//! newest among them. The store keeps the newest ([`Store::set_file_versions_kept`]);
//! the row of an older one goes, and its object is doomed in the same transaction.
//!
//! A new version is written against the one it was made from: a write that names
//! another version than the file's newest changes nothing
//! ([`VersionAdded::Stale`], [`VersionRestored::Stale`]). Of two writes made from the
//! same version, the board's lock lets one in first, and the other finds a newer
//! version.
//!
//! The rows have no key to the record. A board in the trash keeps its file records
//! with it (`deleted_containers.files`) and the rows stay here meanwhile; they go when
//! the record goes for good ([`forget`]): with its board out of the trash, or as a file
//! that no block shows any more.

use std::sync::atomic::Ordering;

use crate::Store;
use crate::containers;
use crate::db::{Dialect, Id, OptId, Row, Scope, SqlFragment};
use crate::error::StoreError;
use crate::files::check_new_path;
use crate::loose_objects::{self, LooseState};

/// How many versions of a file the store keeps unless told otherwise.
pub const DEFAULT_FILE_VERSIONS_KEPT: u32 = 50;

/// How many records one statement of [`forget`] names.
const FORGET_BATCH: usize = 300;

/// A version of a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeptFileVersion {
    /// Its number among the file's, from 1; versions no longer kept leave the lowest
    /// numbers out.
    pub version: i64,
    /// Where its bytes are in the file storage.
    pub path: String,
    /// Their size.
    pub size: i64,
    /// When it was written, in milliseconds since the epoch.
    pub created_at: i64,
    /// Who wrote it; empty where nobody is known to have.
    pub created_by: String,
    /// The version whose bytes it took, if it was made by going back to one.
    pub restored_from: Option<i64>,
    /// Whether it is the file's newest: what the file is read as.
    pub is_current: bool,
}

/// Why an uploaded file cannot become a version of another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnusableUpload {
    /// The board has no file of that name.
    Missing,
    /// It is the file it was to become a version of.
    SameFile,
    /// Its name ends in another extension than the file's.
    OtherExtension,
    /// A block shows it, or a kept version of a body or a comment does.
    Shown,
    /// It has versions of its own.
    HasVersions,
}

/// What became of an uploaded file that was to become a file's new version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionAdded {
    /// It is the file's newest version, and no file of its own any more.
    Written(KeptFileVersion),
    /// The file's newest version is `current`, not the one the write was made from.
    /// Nothing changed: the upload is the file it was.
    Stale {
        /// The number of the file's newest version.
        current: i64,
    },
    /// The upload cannot become a version. Nothing changed.
    Unusable(UnusableUpload),
}

/// What became of a copy of an older version's bytes that was to become the file's
/// new version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionRestored {
    /// The copy is the file's newest version.
    Written(KeptFileVersion),
    /// The file's newest version is `current`, not the one the write was made from.
    /// Nothing changed, and the copy is still marked as being written.
    Stale {
        /// The number of the file's newest version.
        current: i64,
    },
    /// The copy was given up while it was written: nothing changed, and it is doomed.
    GivenUp,
}

/// An older version's bytes, copied to a place marked as being written
/// ([`Store::mark_writing`]), to become a file's new version.
#[derive(Debug, Clone, Copy)]
pub struct RestoredCopy<'a> {
    /// The version the bytes were copied from.
    pub from: i64,
    /// Where the copy is.
    pub path: &'a str,
    /// Who goes back to the version.
    pub by: &'a str,
}

impl Store {
    /// Sets how many versions of a file are kept, the newest. At least one. Counts for
    /// the versions written after.
    pub fn set_file_versions_kept(&self, kept: u32) {
        self.inner
            .file_versions_kept
            .store(kept.max(1), Ordering::Relaxed);
    }

    fn file_versions_kept(&self) -> i64 {
        i64::from(self.inner.file_versions_kept.load(Ordering::Relaxed))
    }

    /// The versions kept of a file of a board, the newest first.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if the board has no file of that stored name,
    /// and [`StoreError::Database`] if a query fails.
    pub async fn file_versions(
        &self,
        container_id: &str,
        stored_name: &str,
    ) -> Result<Vec<KeptFileVersion>, StoreError> {
        self.with_conn(async |scope| {
            let file = Head::read(scope, container_id, stored_name)
                .await?
                .ok_or_else(no_file)?;
            versions_of(scope, &file).await
        })
        .await
    }

    /// Makes the uploaded file `upload` of a board the new version of its file
    /// `stored_name`, written by `user_id` from the version `base`: the upload's
    /// object becomes the version's, and its record goes, so that it is no file of its
    /// own any more. Versions past those kept go, the oldest first, and their objects
    /// are doomed.
    ///
    /// The upload is one that nothing shows yet and that has the file's extension. The
    /// file keeps the name it was uploaded with.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if the board is not there or has no file of
    /// the stored name, and [`StoreError::Database`] if a statement fails; nothing
    /// changed then.
    pub async fn add_file_version(
        &self,
        container_id: &str,
        stored_name: &str,
        base: i64,
        upload: &str,
        user_id: &str,
    ) -> Result<VersionAdded, StoreError> {
        let kept = self.file_versions_kept();
        self.transaction(async |scope| {
            let (file, source) = hold(scope, container_id, stored_name, Some(upload)).await?;
            if file.version != base {
                return Ok(VersionAdded::Stale {
                    current: file.version,
                });
            }
            if upload == stored_name {
                return Ok(VersionAdded::Unusable(UnusableUpload::SameFile));
            }
            let Some(source) = source else {
                return Ok(VersionAdded::Unusable(UnusableUpload::Missing));
            };
            if let Some(why) = unusable(scope, container_id, upload, &file, &source).await? {
                return Ok(VersionAdded::Unusable(why));
            }
            // The upload's record goes, and its object is the version's: one record
            // names the place again once the file's does.
            let mut sql = scope.sql("DELETE FROM files WHERE id = ");
            sql.bind(Id(&source.id));
            scope.execute(sql).await?;
            let bytes = NewBytes {
                path: &source.path,
                size: source.size,
                by: user_id,
                restored_from: None,
            };
            write(scope, &file, bytes, kept)
                .await
                .map(VersionAdded::Written)
        })
        .await
    }

    /// Makes a copy of the bytes of an older version of a file the file's new version,
    /// written from the version `base`, and takes the copy's mark of writing away in
    /// the same transaction. Versions past those kept go, the oldest first, and their
    /// objects are doomed.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::InvalidPath`] for a place spelled otherwise than the one
    /// way, [`StoreError::NotFound`] if the board is not there, has no file of the
    /// stored name, or the file has the version copied no more, and
    /// [`StoreError::Database`] if a statement fails. Nothing changed then, and the
    /// copy is still marked as being written.
    pub async fn restore_file_version(
        &self,
        container_id: &str,
        stored_name: &str,
        base: i64,
        copy: RestoredCopy<'_>,
    ) -> Result<VersionRestored, StoreError> {
        check_new_path(copy.path)?;
        let kept = self.file_versions_kept();
        self.transaction(async |scope| {
            let (file, _) = hold(scope, container_id, stored_name, None).await?;
            if file.version != base {
                return Ok(VersionRestored::Stale {
                    current: file.version,
                });
            }
            let source = versions_of(scope, &file)
                .await?
                .into_iter()
                .find(|kept| kept.version == copy.from)
                .ok_or_else(|| StoreError::not_found("version"))?;
            let mut sql = scope.sql("DELETE FROM loose_objects WHERE path = ");
            sql.bind(copy.path)
                .push_bind(" AND state = ", LooseState::Writing.as_str());
            if scope.execute(sql).await? == 0 {
                // Doomed while it was copied, and perhaps removed since.
                loose_objects::doom(scope, &[(copy.path.to_owned(), file.org_id.clone())]).await?;
                return Ok(VersionRestored::GivenUp);
            }
            let bytes = NewBytes {
                path: copy.path,
                size: source.size,
                by: copy.by,
                restored_from: Some(copy.from),
            };
            write(scope, &file, bytes, kept)
                .await
                .map(VersionRestored::Written)
        })
        .await
    }
}

/// The record of a file, as far as its versions go.
#[derive(Debug, Clone)]
struct Head {
    id: String,
    version: i64,
    path: String,
    size: i64,
    created_at: i64,
    created_by: String,
    extension: String,
    /// The organization of the record, where the rows this handle writes must name it
    /// ([`loose_objects::org_column`]).
    org_id: Option<String>,
}

impl Head {
    const COLUMNS: &'static str =
        "name, id, version, path, size, created_at, created_by, extension";
    /// Where the organization's column is read, when it is: after [`Self::COLUMNS`].
    const ORG_COLUMN: usize = 8;

    async fn read(
        scope: &mut Scope<'_>,
        container_id: &str,
        stored_name: &str,
    ) -> Result<Option<Self>, StoreError> {
        // Nothing is held: a reader takes the record as it is.
        let sql = Self::select(scope, container_id, &[stored_name]);
        match scope.fetch_optional(sql).await? {
            Some(row) => Ok(Some(Self::from_row(&row, scope)?.1)),
            None => Ok(None),
        }
    }

    fn select(scope: &Scope<'_>, container_id: &str, names: &[&str]) -> crate::db::Sql {
        let mut head = SqlFragment::of(&["SELECT ", Self::COLUMNS]);
        if let Some(org) = loose_objects::org_column(scope) {
            head.push(", ").push(org);
        }
        head.push(" FROM files WHERE container_id = ");
        let mut sql = scope.sql(&head);
        sql.bind(Id(container_id))
            .push(" AND name IN (")
            .bind_list(names.iter().copied())
            .push(")");
        sql
    }

    /// The stored name of the row's record, and the record.
    fn from_row(row: &Row, scope: &Scope<'_>) -> Result<(String, Self), StoreError> {
        let org_id = loose_objects::org_column(scope)
            .map(|_| row.id(Self::ORG_COLUMN))
            .transpose()?;
        Ok((
            row.text(0)?,
            Self {
                id: row.id(1)?,
                version: row.int(2)?,
                path: row.text(3)?,
                size: row.int(4)?,
                created_at: row.int(5)?,
                created_by: row.id(6)?,
                extension: row.text(7)?,
                org_id,
            },
        ))
    }
}

fn no_file() -> StoreError {
    StoreError::not_found("file")
}

/// Holds a file of a board for a write of its versions, and with it the upload that is
/// to become one, if any: the board first, as every write of what is on it does, so
/// that the writes of a file's versions queue up and what is read here stays so until
/// the transaction ends; then the records, in the order of their identifiers, which
/// the collection of the files no block shows skips while they are held
/// ([`Store::collect_unreferenced_files`]).
async fn hold(
    scope: &mut Scope<'_>,
    container_id: &str,
    stored_name: &str,
    upload: Option<&str>,
) -> Result<(Head, Option<Head>), StoreError> {
    if !containers::lock_container(scope, container_id).await? {
        return Err(no_file());
    }
    let names: Vec<&str> = std::iter::once(stored_name).chain(upload).collect();
    let mut sql = Head::select(scope, container_id, &names);
    sql.push(" ORDER BY id");
    if scope.dialect() == Dialect::Postgres {
        sql.push(" FOR UPDATE");
    }
    let (mut file, mut source) = (None, None);
    for row in scope.fetch_all(sql).await? {
        let (name, head) = Head::from_row(&row, scope)?;
        if name == stored_name {
            file = Some(head);
        } else {
            source = Some(head);
        }
    }
    Ok((file.ok_or_else(no_file)?, source))
}

/// Why the upload `source` cannot become a version of `file`, if it cannot.
async fn unusable(
    scope: &mut Scope<'_>,
    container_id: &str,
    upload: &str,
    file: &Head,
    source: &Head,
) -> Result<Option<UnusableUpload>, StoreError> {
    if source.extension != file.extension {
        return Ok(Some(UnusableUpload::OtherExtension));
    }
    let mut sql = scope.sql("SELECT 1 FROM file_refs WHERE container_id = ");
    sql.bind(Id(container_id))
        .push_bind(" AND name = ", upload)
        .push(" LIMIT 1");
    if scope.fetch_optional(sql).await?.is_some() {
        return Ok(Some(UnusableUpload::Shown));
    }
    if source.version != 1 || has_row(scope, &source.id, None).await? {
        return Ok(Some(UnusableUpload::HasVersions));
    }
    Ok(None)
}

/// Whether a file has a row of `version`, or of any version for `None`.
async fn has_row(
    scope: &mut Scope<'_>,
    file_id: &str,
    version: Option<i64>,
) -> Result<bool, StoreError> {
    let mut sql = scope.sql("SELECT 1 FROM file_versions WHERE file_id = ");
    sql.bind(Id(file_id));
    if let Some(version) = version {
        sql.push_bind(" AND version = ", version);
    }
    sql.push(" LIMIT 1");
    Ok(scope.fetch_optional(sql).await?.is_some())
}

/// The versions kept of a file, the newest first: its rows, or its record alone while
/// it has none.
async fn versions_of(
    scope: &mut Scope<'_>,
    file: &Head,
) -> Result<Vec<KeptFileVersion>, StoreError> {
    let mut sql = scope.sql(
        "SELECT version, path, size, created_at, created_by, restored_from \
         FROM file_versions WHERE file_id = ",
    );
    sql.bind(Id(&file.id)).push(" ORDER BY version DESC");
    let rows = scope.fetch_all(sql).await?;
    if rows.is_empty() {
        return Ok(vec![KeptFileVersion {
            version: file.version,
            path: file.path.clone(),
            size: file.size,
            created_at: file.created_at,
            created_by: file.created_by.clone(),
            restored_from: None,
            is_current: true,
        }]);
    }
    // The newest row is the file's version, whatever the record said when it was
    // read: a version written since is in the rows.
    let newest = rows.first().map(|row| row.int(0)).transpose()?;
    rows.iter()
        .map(|row| {
            let version = row.int(0)?;
            Ok(KeptFileVersion {
                version,
                path: row.text(1)?,
                size: row.int(2)?,
                created_at: row.int(3)?,
                created_by: row.id(4)?,
                restored_from: row.opt_int(5)?,
                is_current: Some(version) == newest,
            })
        })
        .collect()
}

/// The bytes of a new version.
#[derive(Debug, Clone, Copy)]
struct NewBytes<'a> {
    path: &'a str,
    size: i64,
    by: &'a str,
    restored_from: Option<i64>,
}

/// One row of `file_versions` to put down.
#[derive(Debug, Clone, Copy)]
struct NewRow<'a> {
    version: i64,
    path: &'a str,
    size: i64,
    created_at: i64,
    created_by: &'a str,
    restored_from: Option<i64>,
}

async fn insert_row(scope: &mut Scope<'_>, file: &Head, row: NewRow<'_>) -> Result<(), StoreError> {
    let mut sql = scope.sql("INSERT INTO file_versions (file_id, version, path, size, created_at, created_by, restored_from");
    sql.push(if file.org_id.is_some() {
        ", org_id) VALUES ("
    } else {
        ") VALUES ("
    })
    .bind(Id(&file.id))
    .push_bind(", ", row.version)
    .push_bind(", ", row.path)
    .push_bind(", ", row.size)
    .push_bind(", ", row.created_at)
    .push_bind(", ", OptId(row.created_by));
    match row.restored_from {
        Some(from) => sql.push_bind(", ", from),
        None => sql.push(", NULL"),
    };
    if let Some(org_id) = &file.org_id {
        sql.push_bind(", ", Id(org_id));
    }
    sql.push(")");
    scope.execute(sql).await?;
    Ok(())
}

/// Writes `bytes` as the new version of a held file ([`hold`]) and forgets the
/// versions past the `kept` newest.
async fn write(
    scope: &mut Scope<'_>,
    file: &Head,
    bytes: NewBytes<'_>,
    kept: i64,
) -> Result<KeptFileVersion, StoreError> {
    // The bytes the record names are a version from now on: its row, if this is the
    // first write of the file.
    if !has_row(scope, &file.id, Some(file.version)).await? {
        let first = NewRow {
            version: file.version,
            path: &file.path,
            size: file.size,
            created_at: file.created_at,
            created_by: &file.created_by,
            restored_from: None,
        };
        insert_row(scope, file, first).await?;
    }
    let version = file.version + 1;
    let created_at = scope.now_millis();
    let row = NewRow {
        version,
        path: bytes.path,
        size: bytes.size,
        created_at,
        created_by: bytes.by,
        restored_from: bytes.restored_from,
    };
    insert_row(scope, file, row).await?;
    let mut sql = scope.sql("UPDATE files SET version = ");
    sql.bind(version)
        .push_bind(", path = ", bytes.path)
        .push_bind(", size = ", bytes.size)
        .push_bind(" WHERE id = ", Id(&file.id))
        .push_bind(" AND version = ", file.version);
    if scope.execute(sql).await? != 1 {
        // The record is held, and was read at that version.
        return Err(StoreError::Corrupt {
            column: "version",
            reason: format!("the record of file {} changed while it was held", file.id),
        });
    }
    let mut sql = scope.sql("DELETE FROM file_versions WHERE file_id = ");
    sql.bind(Id(&file.id))
        .push_bind(" AND version <= ", version.saturating_sub(kept))
        .push(" RETURNING path");
    let doomed: Vec<(String, Option<String>)> = scope
        .fetch_all(sql)
        .await?
        .iter()
        .map(|row| Ok((row.text(0)?, file.org_id.clone())))
        .collect::<Result<_, StoreError>>()?;
    loose_objects::doom(scope, &doomed).await?;
    Ok(KeptFileVersion {
        version,
        path: bytes.path.to_owned(),
        size: bytes.size,
        created_at,
        created_by: bytes.by.to_owned(),
        restored_from: bytes.restored_from,
        is_current: true,
    })
}

/// Forgets the versions of files whose records go for good, and dooms their objects,
/// in the transaction that removes the records: nothing else says where the bytes of
/// an older version are.
pub(crate) async fn forget(scope: &mut Scope<'_>, file_ids: &[&str]) -> Result<(), StoreError> {
    let org = loose_objects::org_column(scope);
    for batch in file_ids.chunks(FORGET_BATCH) {
        let mut sql = scope.sql("DELETE FROM file_versions WHERE file_id IN (");
        sql.bind_list(batch.iter().map(|id| Id(id)))
            .push(") RETURNING path");
        if let Some(column) = org {
            sql.push([", ", column]);
        }
        let doomed: Vec<(String, Option<String>)> = scope
            .fetch_all(sql)
            .await?
            .iter()
            .map(|row| Ok((row.text(0)?, org.map(|_| row.id(1)).transpose()?)))
            .collect::<Result<_, StoreError>>()?;
        loose_objects::doom(scope, &doomed).await?;
    }
    Ok(())
}

/// The SQL condition that the user `users.id` uploaded a file of the board `b`, or
/// wrote a version of one: `OR EXISTS (…)`, for who is named as an author
/// (`crate::directory`).
pub(crate) fn or_wrote_file_of_container(scope: &Scope<'_>) -> SqlFragment {
    let mut frag =
        SqlFragment::of(&[" OR EXISTS (SELECT 1 FROM files f WHERE f.container_id = b.id"]);
    frag.push_fragment(&scope.same_organization(&[("f", "b")]))
        .push(
            " AND (f.created_by = users.id OR EXISTS (SELECT 1 FROM file_versions fv \
             WHERE fv.file_id = f.id AND fv.created_by = users.id",
        )
        .push_fragment(&scope.same_organization(&[("fv", "f")]))
        .push(")))");
    frag
}
