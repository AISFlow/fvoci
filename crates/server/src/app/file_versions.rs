//! The versions kept of a board's file (`fvoci_store`'s `file_versions`), as the
//! board's readers are given them and as its editors write a new one: from an uploaded
//! file, or by going back to an older one.

use fvoci_model::{BlockPatch, BlockType, FileVersion, JsonMap};
use fvoci_store::{
    BlockFilter, KeptFileVersion, RestoredCopy, UnusableUpload, VersionAdded, VersionRestored,
};
use serde_json::Value;

use super::App;
use super::blocks::ChangeNotification;
use super::files::FILE;
use crate::error::{AppError, Reason};

/// What a request misses that asks for a version a file has not, or has no more.
const VERSION: &str = "version";

impl App {
    /// The versions kept of a file of a board, the newest first.
    ///
    /// # Errors
    ///
    /// Returns a not-found error if the board has no such file, and the store's error.
    pub async fn file_versions(
        &self,
        container_id: &str,
        file_id: &str,
    ) -> Result<Vec<FileVersion>, AppError> {
        Ok(self
            .kept_file_versions(container_id, file_id)
            .await?
            .into_iter()
            .map(described)
            .collect())
    }

    /// A version kept of a file of a board, with the place of its bytes.
    ///
    /// # Errors
    ///
    /// Returns a not-found error if the board has no such file or the file no such
    /// version (any more), and the store's error.
    pub(crate) async fn kept_file_version(
        &self,
        container_id: &str,
        file_id: &str,
        version: i64,
    ) -> Result<KeptFileVersion, AppError> {
        self.kept_file_versions(container_id, file_id)
            .await?
            .into_iter()
            .find(|kept| kept.version == version)
            .ok_or_else(|| AppError::not_found(VERSION))
    }

    /// Writes the size of a file's newest version on the attachments of the board that
    /// give the file, as `user_id`; who listens to the board is told as of any change
    /// of a block. An attachment says the size it was made with, and a card shows that
    /// one and the viewer holds the file to its limit by it.
    ///
    /// The version is written by now, so nothing here fails the request: what could
    /// not be read or changed is logged, and the attachment says an older size until
    /// the next version is written. The size is read here and not taken from the
    /// write, so of two versions written close together the later one's is what is
    /// read; the block is still patched outside the write's lock.
    async fn resize_attachments(&self, container_id: &str, file_id: &str, user_id: &str) {
        let size = match self.file_record(container_id, file_id).await {
            Ok(Some(record)) => record.size,
            // Gone meanwhile, with its board or collected: nothing gives it any more.
            Ok(None) => return,
            Err(error) => {
                tracing::warn!(%error, container_id, file_id, "a file with a new version was not read");
                return;
            }
        };
        let attachments = BlockFilter {
            block_type: Some(BlockType::Attachment),
            ..BlockFilter::container(container_id)
        };
        let blocks = match self.store().get_blocks(&attachments).await {
            Ok(blocks) => blocks,
            Err(error) => {
                tracing::warn!(%error, container_id, "the attachments of a board were not read");
                return;
            }
        };
        let stale = blocks.iter().filter(|block| {
            let fields = &block.fields;
            fields.get("fileId").and_then(Value::as_str) == Some(file_id)
                && fields.get("size").and_then(Value::as_i64) != Some(size)
        });
        for block in stale {
            let mut updated_fields = JsonMap::new();
            updated_fields.insert("size".to_owned(), Value::from(size));
            let patch = BlockPatch {
                updated_fields,
                ..BlockPatch::default()
            };
            let patched = self
                .patch_block(&block.id, &patch, user_id, ChangeNotification::default())
                .await;
            if let Err(error) = patched {
                tracing::warn!(%error, block_id = %block.id, "an attachment keeps its file's older size");
            }
        }
    }

    async fn kept_file_versions(
        &self,
        container_id: &str,
        file_id: &str,
    ) -> Result<Vec<KeptFileVersion>, AppError> {
        match self.store().file_versions(container_id, file_id).await {
            Ok(kept) => Ok(kept),
            Err(error) if error.is_not_found() => Err(AppError::not_found(FILE)),
            Err(error) => Err(error.into()),
        }
    }

    /// Makes the uploaded file `upload` of the board the new version of its file
    /// `file_id`, written by `user_id` from the version `base`. The upload is no file
    /// of its own after: its identifier names nothing.
    ///
    /// # Errors
    ///
    /// Returns a not-found error if the board has no such file; a conflict with the
    /// reason `file_version_stale` if the file's newest version is not `base` (the
    /// upload is left as it is); a bad request at `/fileId` for an upload that cannot
    /// become a version; and the store's error.
    pub async fn add_file_version(
        &self,
        container_id: &str,
        file_id: &str,
        base: i64,
        upload: &str,
        user_id: &str,
    ) -> Result<FileVersion, AppError> {
        let added = self
            .store()
            .add_file_version(container_id, file_id, base, upload, user_id)
            .await;
        match added {
            Ok(VersionAdded::Written(kept)) => {
                self.resize_attachments(container_id, file_id, user_id)
                    .await;
                Ok(described(kept))
            }
            Ok(VersionAdded::Stale { current }) => Err(stale(base, current)),
            Ok(VersionAdded::Unusable(why)) => {
                Err(AppError::bad_request(unusable(why)).with_pointer("/fileId"))
            }
            Err(error) if error.is_not_found() => Err(AppError::not_found(FILE)),
            Err(error) => Err(error.into()),
        }
    }

    /// Takes a file of the board back to its version `version`, as `user_id`, from the
    /// version `base`: the bytes of the older version are copied, and the copy is the
    /// file's new version. A file whose newest version is the one asked for is left as
    /// it is.
    ///
    /// # Errors
    ///
    /// Returns a not-found error if the board has no such file or the file no such
    /// version (any more); a conflict with the reason `file_version_stale` if the
    /// file's newest version is not `base`; and a server error if the bytes cannot be
    /// copied or the version recorded.
    pub async fn restore_file_version(
        &self,
        container_id: &str,
        file_id: &str,
        version: i64,
        base: i64,
        user_id: &str,
    ) -> Result<FileVersion, AppError> {
        let kept = self.kept_file_versions(container_id, file_id).await?;
        let current = kept
            .iter()
            .find(|kept| kept.is_current)
            .ok_or_else(|| AppError::not_found(FILE))?;
        if current.version != base {
            return Err(stale(base, current.version));
        }
        let Some(source) = kept.iter().find(|kept| kept.version == version) else {
            return Err(AppError::not_found(VERSION));
        };
        if source.is_current {
            return Ok(described(source.clone()));
        }
        let size = u64::try_from(source.size).unwrap_or(0);
        self.check_org_storage_room(size).await?;
        let path = self.new_version_place(container_id).await?;
        let written = self
            .mark_place(&path)
            .await?
            .copy_from(&source.path, size)
            .await?;
        let copy = RestoredCopy {
            from: version,
            path: written.path(),
            by: user_id,
        };
        let restored = self
            .store()
            .restore_file_version(container_id, file_id, base, copy)
            .await;
        match restored {
            Ok(VersionRestored::Written(kept)) => {
                written.settled_in_transaction();
                self.resize_attachments(container_id, file_id, user_id)
                    .await;
                Ok(described(kept))
            }
            Ok(VersionRestored::GivenUp) => {
                written.settled_in_transaction();
                Err(AppError::internal(
                    "the copy of the version was given up while it was written",
                ))
            }
            Ok(VersionRestored::Stale { current }) => {
                written.give_up().await;
                Err(stale(base, current))
            }
            Err(error) => {
                written.give_up().await;
                if error.is_not_found() {
                    Err(AppError::not_found(VERSION))
                } else {
                    Err(error.into())
                }
            }
        }
    }
}

/// A kept version as a reader is given it: without the place of its bytes.
fn described(kept: KeptFileVersion) -> FileVersion {
    FileVersion {
        version: kept.version,
        created_at: kept.created_at,
        created_by: kept.created_by,
        size: kept.size,
        is_current: kept.is_current,
        restored_from: kept.restored_from,
    }
}

/// The refusal of a write made from the version `base` of a file whose newest is
/// `current`.
fn stale(base: i64, current: i64) -> AppError {
    AppError::conflict(format_args!(
        "the file's newest version is {current}, not {base}"
    ))
    .with_reason(Reason::FileVersionStale)
}

/// What is said of an upload that cannot become a version.
fn unusable(why: UnusableUpload) -> &'static str {
    match why {
        UnusableUpload::Missing => "the board has no uploaded file of that identifier",
        UnusableUpload::SameFile => "a file is no new version of itself",
        UnusableUpload::OtherExtension => {
            "the uploaded file has another extension than the file it is to be a version of"
        }
        UnusableUpload::Shown => "the uploaded file is shown by a block",
        UnusableUpload::HasVersions => "the uploaded file has versions of its own",
    }
}
