//! Exporting boards and wikis to the native `.fvoci` archive format of version 1.
//!
//! An archive is a ZIP holding `version.json` and, per container, a directory with a
//! `container.jsonl` and the container's files. Each line of `container.jsonl` is a
//! container, a block, a member, or a course schedule as the contract has them
//! (`contracts/openapi.json`), bodies included; the files are those the blocks show, those
//! of the images and file blocks of bodies among them. A container's line also carries
//! `fileNames`: the name each of its files was uploaded with, by its stored name, which a
//! file block shows; and `missingFiles`: the stored names of the files the blocks show that
//! were not there to be written, which have no entry. An import gives the files it stores
//! those names, and stores none of the missing ones.

use std::collections::{BTreeMap, HashSet};
use std::io::{BufWriter, Write as _};
use std::path::Path;

use futures_util::StreamExt as _;
use fvoci_model::{ArchiveHeader, Container, ContainerKind};
use serde::Serialize;
use tokio::fs;
use zip::write::{SimpleFileOptions, ZipWriter};

use super::App;
use super::container_copies::counted_as;
use super::doomed::Doomed;
use crate::auth::now_millis;
use crate::auth::secret::new_secret;
use crate::error::AppError;
use crate::metrics::ContainerWork;

/// The archive format version this server writes. An archive's lines carry when each
/// container and block was made, and an import does not read it back: what it brings is
/// made at the import, as the store makes every new row. An import that kept those times
/// would read archives otherwise than this version promises, and takes a new version.
pub(super) const ARCHIVE_VERSION: i64 = 1;

/// A container's line of an archive: the container, and the name each of its files was
/// uploaded with, by its stored name (`fileNames`, left out when there is none).
#[derive(Serialize)]
struct ArchivedContainer<'a> {
    #[serde(flatten)]
    container: &'a Container,
    #[serde(rename = "fileNames", skip_serializing_if = "BTreeMap::is_empty")]
    file_names: BTreeMap<String, String>,
    /// The stored names of the files the blocks show that the export did not find.
    #[serde(rename = "missingFiles", skip_serializing_if = "Vec::is_empty")]
    missing_files: Vec<String>,
}

/// A course's shared schedule line in `container.jsonl`. Personal `unit_progress` rows
/// (`completed_at`, personal `planned_date`, `last_position`) are never exported.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ArchivedCourseSchedule<'a> {
    container_id: &'a str,
    start_date: &'a str,
    target_date: &'a str,
    study_days: u8,
}

/// The directory of the temporary directory exports are written in.
pub(crate) const EXPORTS_DIR: &str = "exports";

/// How much of an entry is gathered before it is handed to the archive writer.
const WRITE_CHUNK_BYTES: usize = 1024 * 1024;

/// An archive written to a temporary file, ready to be sent. The file is removed when
/// this is dropped.
#[derive(Debug)]
pub struct ExportedArchive {
    /// The archive, read from its start.
    pub file: fs::File,
    /// Its size in bytes.
    pub size: u64,
    /// Where the file is; held only to remove it. After `file`, so that the file is
    /// closed before it is removed.
    _path: Doomed,
}

impl ExportedArchive {
    /// An archive in the file at `path`, open from its start, of `size` bytes.
    pub(crate) fn new(file: fs::File, size: u64, path: Doomed) -> Self {
        Self {
            file,
            size,
            _path: path,
        }
    }

    /// Closes the file and removes it, and waits for that.
    pub(crate) async fn remove(self) {
        let Self {
            file, _path: path, ..
        } = self;
        drop(file);
        path.remove().await;
    }
}

impl App {
    /// Builds an archive of the given boards in a temporary file
    /// and returns it open.
    ///
    /// Each board is written with its blocks and members; a file a block shows is added
    /// if it exists, and named in the board's `missingFiles` if it does not.
    /// The file stays in its directory while it is sent and is removed with the
    /// [`ExportedArchive`] (sent, given up, or the client gone); what a stopped server
    /// leaves is removed when it starts again.
    ///
    /// # Errors
    ///
    /// Returns a not-found error if a board does not exist, or a server error if the
    /// archive cannot be written.
    pub async fn export_archive(
        &self,
        workspace_id: &str,
        container_ids: &[String],
    ) -> Result<ExportedArchive, AppError> {
        let mut containers = Vec::with_capacity(container_ids.len());
        for id in container_ids {
            self.write_container_bodies(id).await?;
            containers.push(self.store().get_container(id).await.map_err(|error| {
                AppError::from(error).context(format_args!("could not fetch board {id}"))
            })?);
        }

        let directory = self.config().tmp_dir().join(EXPORTS_DIR);
        fs::create_dir_all(&directory)
            .await
            .map_err(|error| io_error(&error))?;
        // Removed however the export ends, also when its request is dropped meanwhile: the
        // file stays while it is sent, and goes with the archive.
        let path = Doomed::file(directory.join(format!("{}.fvoci", new_secret())));
        self.write_archive(path.path(), workspace_id, &containers)
            .await?;
        let file = fs::File::open(path.path())
            .await
            .map_err(|error| io_error(&error))?;
        let size = file
            .metadata()
            .await
            .map_err(|error| io_error(&error))?
            .len();
        // Counted once the archive is there to be sent, whatever becomes of its download.
        for container in &containers {
            self.metrics()
                .container_copied(ContainerWork::Export, counted_as(container.kind));
        }
        Ok(ExportedArchive::new(file, size, path))
    }

    /// Writes the archive of `containers` to a new file at `path`.
    async fn write_archive(
        &self,
        path: &Path,
        workspace_id: &str,
        containers: &[Container],
    ) -> Result<(), AppError> {
        let file = fs::File::create(path)
            .await
            .map_err(|error| io_error(&error))?
            .into_std()
            .await;
        let mut writer = ArchiveWriter::new(file);
        writer.start_entry("version.json").await?;
        let header = ArchiveHeader {
            version: ARCHIVE_VERSION,
            date: now_millis(),
        };
        let bytes = serde_json::to_vec(&header).map_err(|error| {
            AppError::internal(format_args!("cannot write archive header: {error}"))
        })?;
        writer.write(&bytes).await?;
        for container in containers {
            self.write_container(&mut writer, container, workspace_id)
                .await?;
        }
        writer.finish().await
    }

    /// Writes one container's directory: its files, then its `container.jsonl`, which names
    /// the files that were not there to be written.
    async fn write_container(
        &self,
        writer: &mut ArchiveWriter,
        container: &Container,
        workspace_id: &str,
    ) -> Result<(), AppError> {
        let blocks = self.blocks_for_container(&container.id).await?;
        // Two blocks may name the same file: it is written once, as an entry of an
        // archive has a name of its own.
        let mut files = Vec::new();
        let mut seen = HashSet::new();
        for block in &blocks {
            for filename in block.shown_files() {
                // An attachment block names its file as a client wrote it. A name that cannot
                // be a stored name would be read back as another entry of the archive,
                // or clash with one.
                if is_stored_name(&filename) && seen.insert(filename.clone()) {
                    files.push(filename);
                }
            }
        }
        let mut written = Vec::new();
        let mut missing_files = Vec::new();
        for filename in files {
            if self.write_file(writer, &container.id, &filename).await? {
                written.push(filename);
            } else {
                missing_files.push(filename);
            }
        }
        if !missing_files.is_empty() {
            tracing::warn!(
                workspace_id,
                container_id = container.id,
                missing = ?missing_files,
                "files the blocks show are missing from the export"
            );
        }
        let file_names = self.file_names(&container.id, &written).await?;
        let archived = ArchivedContainer {
            container,
            file_names,
            missing_files,
        };
        writer
            .start_entry(&format!("{}/container.jsonl", container.id))
            .await?;
        writer.write_line("container", &archived).await?;
        for block in &blocks {
            writer.write_line("block", block).await?;
        }
        if container.kind == ContainerKind::Course
            && let Some(schedule) = self.store().get_course_schedule(&container.id).await?
        {
            let line = ArchivedCourseSchedule {
                container_id: &container.id,
                start_date: &schedule.start_date,
                target_date: &schedule.target_date,
                study_days: schedule.study_days,
            };
            writer.write_line("courseSchedule", &line).await?;
        }

        for member in self.present_members_for_container(&container.id).await? {
            writer.write_line("containerMember", &member).await?;
        }
        Ok(())
    }

    /// The name each of `files` of the board was uploaded with, by its stored name; a file
    /// without a record is left out. Loaded in a single query (`1.2.5`).
    async fn file_names(
        &self,
        container_id: &str,
        files: &[String],
    ) -> Result<BTreeMap<String, String>, AppError> {
        if files.is_empty() {
            return Ok(BTreeMap::new());
        }
        let wanted: HashSet<&str> = files.iter().map(String::as_str).collect();
        let records = self.store().get_files_of_container(container_id).await?;
        let mut names = BTreeMap::new();
        for record in records {
            if wanted.contains(record.stored_name.as_str()) {
                names.insert(record.stored_name, record.name);
            }
        }
        Ok(names)
    }

    /// Writes a board file as its own archive entry, and returns whether it did: a file
    /// without a record or without its object gets no entry.
    async fn write_file(
        &self,
        writer: &mut ArchiveWriter,
        container_id: &str,
        filename: &str,
    ) -> Result<bool, AppError> {
        let mut stream = match self.open_stored_file(container_id, filename).await {
            Ok(Some(stream)) => stream,
            // Gone between the lookup and the read, as when it is collected meanwhile.
            Ok(None) => return Ok(false),
            Err(error) if error.is_not_found() => return Ok(false),
            Err(error) => return Err(error),
        };
        writer
            .start_entry(&format!("{container_id}/{filename}"))
            .await?;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk
                .map_err(|error| AppError::internal(format_args!("cannot read file: {error}")))?;
            writer.write(&chunk).await?;
        }
        Ok(true)
    }
}

/// The ZIP writer of an archive.
type Zip = ZipWriter<BufWriter<std::fs::File>>;

/// Writes an archive to a file. The writing and the compression are done off the
/// runtime's workers, a chunk at a time.
struct ArchiveWriter {
    /// The writer, away while a chunk is written; none after a failure. Boxed: it is
    /// large, and held across every wait of the export.
    zip: Option<Box<Zip>>,
    /// What is gathered for the current entry and not yet written.
    pending: Vec<u8>,
}

impl ArchiveWriter {
    fn new(file: std::fs::File) -> Self {
        Self {
            zip: Some(Box::new(ZipWriter::new(BufWriter::new(file)))),
            pending: Vec::new(),
        }
    }

    /// Starts a new entry, compressed with DEFLATE.
    async fn start_entry(&mut self, name: &str) -> Result<(), AppError> {
        self.flush().await?;
        let name = name.to_owned();
        self.blocking(move |zip| {
            let options =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            zip.start_file(name, options).map_err(|error| {
                AppError::internal(format_args!("cannot create archive entry: {error}"))
            })
        })
        .await
    }

    /// Writes bytes to the current entry.
    async fn write(&mut self, bytes: &[u8]) -> Result<(), AppError> {
        self.pending.extend_from_slice(bytes);
        if self.pending.len() >= WRITE_CHUNK_BYTES {
            self.flush().await?;
        }
        Ok(())
    }

    /// Writes one JSONL line wrapping `value` under `line_type`, serialized in a single
    /// pass without materializing an intermediate `serde_json::Value`.
    async fn write_line(
        &mut self,
        line_type: &str,
        value: &impl Serialize,
    ) -> Result<(), AppError> {
        #[derive(Serialize)]
        struct BorrowedArchiveLine<'a, T: Serialize + ?Sized> {
            #[serde(rename = "type")]
            line_type: &'a str,
            data: &'a T,
        }
        let line = BorrowedArchiveLine {
            line_type,
            data: value,
        };
        let mut bytes = serde_json::to_vec(&line).map_err(|error| {
            AppError::internal(format_args!("cannot encode archive line: {error}"))
        })?;
        bytes.push(b'\n');
        self.write(&bytes).await
    }

    /// Writes what is gathered.
    async fn flush(&mut self) -> Result<(), AppError> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let bytes = std::mem::take(&mut self.pending);
        self.blocking(move |zip| {
            zip.write_all(&bytes).map_err(|error| {
                AppError::internal(format_args!("cannot write archive entry: {error}"))
            })
        })
        .await
    }

    /// Ends the archive: its directory, and what is buffered.
    async fn finish(mut self) -> Result<(), AppError> {
        self.flush().await?;
        let zip = self
            .zip
            .take()
            .ok_or_else(|| AppError::internal("the archive writer failed before"))?;
        tokio::task::spawn_blocking(move || -> zip::result::ZipResult<()> {
            (*zip).finish()?.flush()?;
            Ok(())
        })
        .await
        .map_err(|error| AppError::internal(format_args!("archive task failed: {error}")))?
        .map_err(|error| AppError::internal(format_args!("cannot finish archive: {error}")))
    }

    /// Runs `work` on the writer off the runtime's workers.
    async fn blocking(
        &mut self,
        work: impl FnOnce(&mut Zip) -> Result<(), AppError> + Send + 'static,
    ) -> Result<(), AppError> {
        let mut zip = self
            .zip
            .take()
            .ok_or_else(|| AppError::internal("the archive writer failed before"))?;
        let zip = tokio::task::spawn_blocking(move || {
            // A writer that failed is dropped here, off the runtime's workers: dropping
            // one ends the archive, which writes.
            work(&mut zip).map(|()| zip)
        })
        .await
        .map_err(|error| AppError::internal(format_args!("archive task failed: {error}")))??;
        self.zip = Some(zip);
        Ok(())
    }
}

impl Drop for ArchiveWriter {
    /// A writer left unfinished, as when reading a file failed, ends its archive when it
    /// is dropped, which writes: that is done off the runtime's workers.
    fn drop(&mut self) {
        if let Some(zip) = self.zip.take() {
            drop(tokio::task::spawn_blocking(move || drop(zip)));
        }
    }
}

/// A temporary file that cannot be written or read: the server's failure.
fn io_error(error: &std::io::Error) -> AppError {
    AppError::internal(format_args!("cannot write the archive: {error}"))
}

/// Whether a name a block shows can be the stored name of a file, and so an entry of the
/// container's directory: one path segment, and not a name the archive itself uses.
fn is_stored_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains(['/', '\\'])
        && !matches!(name, "." | ".." | "container.jsonl" | "version.json")
}
