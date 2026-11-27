//! The local-disk file backend.

use std::io::SeekFrom;
use std::ops::Range;
use std::path::{Component, Path, PathBuf};

use futures_util::{StreamExt as _, TryStreamExt as _};
use tokio::fs;
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncSeekExt as _, AsyncWriteExt as _};
use tokio_util::io::ReaderStream;

use super::{FileStream, ObjectListing, StorageError, StoredObject};
use crate::metrics::Metrics;

/// Files kept in a directory on the local disk, the `file_storage_path` setting.
///
/// A stored path such as `<organization>/<board>/<secret>` ([`fvoci_model::FileKey`]) is
/// that relative path under the directory, the same key as in a bucket.
#[derive(Debug, Clone)]
pub struct LocalFiles {
    root: PathBuf,
    /// What the operations are measured into ([`super::FileBackend::measured`]).
    pub(super) metrics: Metrics,
}

/// The directory under the root, in which files are written before they are renamed
/// into their place. No stored path begins with a dot.
pub const WRITING_DIR: &str = ".tmp/writing";

/// Writes everything `reader` yields to a new file at `path`, and returns the number of
/// bytes written.
async fn write_file(
    path: &Path,
    reader: &mut (dyn AsyncRead + Send + Unpin),
) -> std::io::Result<u64> {
    let mut file = fs::File::create(path).await?;
    let written = tokio::io::copy(reader, &mut file).await?;
    file.flush().await?;
    // On the disk before the rename makes it the file: after a power loss the name does
    // not point at nothing.
    file.sync_all().await?;
    Ok(written)
}

impl LocalFiles {
    /// Creates a backend rooted at `directory`. The directory is created on first write.
    pub fn new(directory: impl AsRef<Path>) -> Self {
        Self {
            root: directory.as_ref().to_path_buf(),
            metrics: Metrics::none(),
        }
    }

    /// The same backend, measuring its operations into `metrics`.
    pub(super) fn measured(self, metrics: Metrics) -> Self {
        Self { metrics, ..self }
    }

    /// Resolves a stored path to a path under the root.
    ///
    /// Only a normalized relative path is accepted ([`fvoci_model::is_storage_path`]).
    /// Absolute paths and `..` components are refused so no stored path can reach outside
    /// the root, and alternate spellings (`a//b`, `./a`) are refused so no two strings name
    /// the same file.
    fn resolve(&self, path: &str) -> Result<PathBuf, StorageError> {
        let relative = Path::new(path);
        let plain = fvoci_model::is_storage_path(path)
            && relative
                .components()
                .all(|component| matches!(component, Component::Normal(_)));
        if plain {
            Ok(self.root.join(relative))
        } else {
            Err(StorageError::InvalidPath(path.to_owned()))
        }
    }

    /// Writes `bytes` at `path` as every file is written, reads them back and removes
    /// them: whether the directory is one (or can be made) and the server may write,
    /// read and remove files in it. Returns what was read.
    pub(super) async fn check(
        &self,
        path: &str,
        mut bytes: &[u8],
    ) -> Result<Vec<u8>, StorageError> {
        match fs::metadata(&self.root).await {
            Ok(metadata) if !metadata.is_dir() => {
                return Err(StorageError::Config(
                    "file_storage_path is not a directory".to_owned(),
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        self.write(path, &mut bytes).await?;
        let read = fs::read(self.resolve(path)?).await;
        // Removed also when it could not be read.
        let removed = self.delete(path).await;
        let read = read?;
        removed?;
        Ok(read)
    }

    pub(super) async fn write(
        &self,
        path: &str,
        reader: &mut (dyn AsyncRead + Send + Unpin),
    ) -> Result<u64, StorageError> {
        let target = self.resolve(path)?;
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).await?;
        }
        // The file is written under the root and renamed into its place, which on one
        // file system is atomic: a reader never finds half a file, and a write that fails
        // leaves nothing at the path. What a crash leaves is in WRITING_DIR, which the
        // server empties when it starts.
        let writing = self.root.join(WRITING_DIR);
        fs::create_dir_all(&writing).await?;
        let partial = writing.join(crate::auth::secret::new_secret());
        let written = match write_file(&partial, reader).await {
            Ok(written) => fs::rename(&partial, &target).await.map(|()| written),
            Err(error) => Err(error),
        };
        if written.is_err() {
            match fs::remove_file(&partial).await {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => tracing::warn!(%error, "a partial file could not be removed"),
            }
        }
        Ok(written?)
    }

    pub(super) async fn size(&self, path: &str) -> Result<Option<u64>, StorageError> {
        match fs::metadata(self.resolve(path)?).await {
            Ok(metadata) if metadata.is_file() => Ok(Some(metadata.len())),
            Ok(_) => Ok(None),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub(super) async fn read(
        &self,
        path: &str,
        range: Option<Range<u64>>,
    ) -> Result<FileStream, StorageError> {
        let mut file = fs::File::open(self.resolve(path)?)
            .await
            .map_err(|error| not_found_or_io(error, path))?;
        match range {
            None => Ok(ReaderStream::new(file).map_err(StorageError::from).boxed()),
            Some(range) => {
                file.seek(SeekFrom::Start(range.start)).await?;
                let length = range.end.saturating_sub(range.start);
                Ok(ReaderStream::new(file.take(length))
                    .map_err(StorageError::from)
                    .boxed())
            }
        }
    }

    pub(super) async fn copy(&self, from: &str, to: &str) -> Result<(), StorageError> {
        let (source, target) = (self.resolve(from)?, self.resolve(to)?);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::copy(source, target)
            .await
            .map_err(|error| not_found_or_io(error, from))?;
        Ok(())
    }

    pub(super) async fn delete(&self, path: &str) -> Result<(), StorageError> {
        match fs::remove_file(self.resolve(path)?).await {
            Ok(()) => Ok(()),
            // What is gone is deleted.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(StorageError::Io(error)),
        }
    }

    pub(super) fn list(&self) -> ObjectListing {
        let walk = Walk {
            root: self.root.clone(),
            directories: vec![String::new()],
            entries: None,
        };
        futures_util::stream::unfold(walk, |mut walk| async move {
            let next = walk.next().await.transpose()?;
            Some((next, walk))
        })
        .boxed()
    }

    pub(super) async fn rename(&self, from: &str, to: &str) -> Result<(), StorageError> {
        let (source, target) = (self.resolve(from)?, self.resolve(to)?);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::rename(source, target)
            .await
            .map_err(|error| not_found_or_io(error, from))
    }
}

/// A walk through the files under the root, a directory at a time: the directories
/// still to read, by their path under the root, and the one being read.
struct Walk {
    root: PathBuf,
    directories: Vec<String>,
    entries: Option<(String, fs::ReadDir)>,
}

impl Walk {
    /// The next file, or `None` once every directory was read.
    async fn next(&mut self) -> Result<Option<StoredObject>, StorageError> {
        loop {
            let Some((directory, entries)) = &mut self.entries else {
                let Some(directory) = self.directories.pop() else {
                    return Ok(None);
                };
                let entries = match fs::read_dir(self.root.join(&directory)).await {
                    Ok(entries) => entries,
                    // The root before the first write, or a directory gone meanwhile.
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    // A directory under the root the server may not read (a volume's
                    // `lost+found`) holds none of its files.
                    Err(error) if !directory.is_empty() => {
                        tracing::warn!(%error, directory, "a directory under the files could not be read");
                        continue;
                    }
                    Err(error) => return Err(error.into()),
                };
                self.entries = Some((directory, entries));
                continue;
            };
            let Some(entry) = entries.next_entry().await? else {
                self.entries = None;
                continue;
            };
            // A name that is no text names no stored file: no record can name it.
            let Ok(name) = entry.file_name().into_string() else {
                tracing::warn!(directory, "a file whose name is no text is left out");
                continue;
            };
            let path = if directory.is_empty() {
                name.clone()
            } else {
                format!("{directory}/{name}")
            };
            // An entry removed since it was read, as by the housekeeping, is gone.
            let kind = match entry.file_type().await {
                Ok(kind) => kind,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            if kind.is_dir() {
                // The files being written and the temporary files are no stored files.
                if !(directory.is_empty() && name.starts_with('.')) {
                    self.directories.push(path);
                }
            } else if kind.is_file() {
                let modified = match entry.metadata().await {
                    Ok(metadata) => metadata.modified()?,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(error.into()),
                };
                let since_epoch = modified
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default();
                return Ok(Some(StoredObject {
                    path,
                    modified_at: i64::try_from(since_epoch.as_millis()).unwrap_or(i64::MAX),
                }));
            }
        }
    }
}

fn not_found_or_io(error: std::io::Error, path: &str) -> StorageError {
    if error.kind() == std::io::ErrorKind::NotFound {
        StorageError::NotFound(path.to_owned())
    } else {
        StorageError::Io(error)
    }
}
