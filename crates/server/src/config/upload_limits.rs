//! Validation of upload, archive and document resource settings.

use std::num::NonZeroU64;

use super::{Configuration, MAX_UPLOAD_PART_BYTES, MIN_UPLOAD_PART_BYTES};
use anyhow::bail;

/// The largest file and the largest archive an upload takes, in bytes.
pub(super) struct Largest {
    pub(super) file: NonZeroU64,
    pub(super) import: NonZeroU64,
}

impl Configuration {
    /// Checks the limits on uploads: none turns a limit off, and they fit together.
    /// Returns the largest file and archive an upload takes.
    pub(super) fn check_upload_limits(&self) -> anyhow::Result<Largest> {
        // An S3 multipart upload takes no smaller part but the last, and none larger.
        if !(MIN_UPLOAD_PART_BYTES..=MAX_UPLOAD_PART_BYTES).contains(&self.upload_part_bytes) {
            bail!(
                "upload_part_bytes must be {MIN_UPLOAD_PART_BYTES} (5 MiB) to \
                 {MAX_UPLOAD_PART_BYTES} (5 GiB)"
            );
        }
        // A file comes in parts, and none is larger than the parts hold.
        let largest = self
            .upload_part_bytes
            .saturating_mul(fvoci_model::MAX_UPLOAD_PARTS.unsigned_abs());
        let Some(file) = u64::try_from(self.max_file_bytes)
            .ok()
            .and_then(NonZeroU64::new)
            .filter(|size| size.get() <= largest)
        else {
            bail!(
                "max_file_bytes must be 1 to {largest} bytes (upload_part_bytes times 10,000 \
                 parts), not {}",
                self.max_file_bytes
            );
        };
        // Less would refuse the largest file with 429 every time, as if it might fit later.
        let max_file_bytes = file.get();
        if self.max_pending_upload_bytes < max_file_bytes {
            bail!(
                "max_pending_upload_bytes must be at least max_file_bytes ({max_file_bytes}), \
                 not {}",
                self.max_pending_upload_bytes
            );
        }
        if self.max_org_storage_bytes < max_file_bytes {
            bail!(
                "max_org_storage_bytes must be at least max_file_bytes ({max_file_bytes}), \
                 not {}",
                self.max_org_storage_bytes
            );
        }
        // An archive comes as an upload in parts too.
        let Some(import) =
            NonZeroU64::new(self.max_import_bytes).filter(|size| size.get() <= largest)
        else {
            bail!(
                "max_import_bytes must be 1 to {largest} bytes (upload_part_bytes times \
                 10,000 parts), not {}",
                self.max_import_bytes
            );
        };
        if self.max_pending_upload_bytes < self.max_import_bytes {
            bail!(
                "max_pending_upload_bytes must be at least max_import_bytes ({}), not {}",
                self.max_import_bytes,
                self.max_pending_upload_bytes
            );
        }
        // Zero would keep not even the text a comment was posted with.
        if self.max_comment_versions == 0 {
            bail!("max_comment_versions must be at least 1");
        }
        // Zero would keep not even the bytes a file has.
        if self.max_file_versions == 0 {
            bail!("max_file_versions must be at least 1");
        }
        // Zero would refuse every export or import.
        if self.max_concurrent_exports == 0 {
            bail!("max_concurrent_exports must be at least 1");
        }
        if self.max_concurrent_imports == 0 {
            bail!("max_concurrent_imports must be at least 1");
        }
        // Zero would take no reader of a public link at all: sharing is turned off for that.
        if self.public_link_connections_per_container == 0 {
            bail!("public_link_connections_per_container must be at least 1");
        }
        if self.public_link_connections_per_address == 0 {
            bail!("public_link_connections_per_address must be at least 1");
        }
        // Zero would give up every download before its first piece.
        if self.export_stall_seconds == 0 {
            bail!("export_stall_seconds must be at least 1");
        }
        self.collab.check()?;
        Ok(Largest { file, import })
    }
}
