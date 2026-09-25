mod backend;
mod disposition;
mod extract_job;
mod local;
mod mime;
mod range;
mod s3;
mod verify;

pub use backend::{ObjectBody, ObjectStorage};
pub use extract_job::{
    read_extract_input, spawn_extract_job, validate_extractor_bin, ExtractJobHandle,
    ExtractJobSettings,
};

pub use disposition::content_disposition_attachment;
pub use local::{LocalStorage, PartInfo, StagedPart, StorageError};
pub use mime::{is_image_mime, sniff_mime_from_bytes};
pub use range::{parse_range, ParsedRange};
pub use s3::{S3Storage, UploadTimeouts, MINIO_TEST_IMAGE};
pub use verify::{verify_stored_objects, StorageVerifyReport};

use std::fmt;

#[derive(Clone)]
pub struct UploadLimits {
    pub part_size_bytes: i64,
    pub max_file_size_bytes: i64,
    pub create_rate_per_5min: u32,
    pub part_put_slots: PartPutSlots,
}

/// Per-process bound on part PUTs in flight. Each proxied part holds an
/// inbound connection and, with S3, an outbound one for as long as the
/// client paces its body; when every slot is taken the PUT is refused
/// before its body is read.
#[derive(Clone)]
pub struct PartPutSlots {
    semaphore: std::sync::Arc<tokio::sync::Semaphore>,
    max: u32,
}

impl PartPutSlots {
    pub fn new(max: u32) -> Self {
        Self {
            semaphore: std::sync::Arc::new(tokio::sync::Semaphore::new(max as usize)),
            max,
        }
    }

    pub fn max(&self) -> u32 {
        self.max
    }

    /// A slot held until the returned permit drops, or `None` when all are
    /// in use.
    pub fn try_acquire(&self) -> Option<tokio::sync::OwnedSemaphorePermit> {
        self.semaphore.clone().try_acquire_owned().ok()
    }
}

impl fmt::Debug for UploadLimits {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UploadLimits")
            .field("part_size_bytes", &self.part_size_bytes)
            .field("max_file_size_bytes", &self.max_file_size_bytes)
            .field("create_rate_per_5min", &self.create_rate_per_5min)
            .field("max_concurrent_part_puts", &self.part_put_slots.max())
            .finish()
    }
}

pub const MAX_PART_COUNT: i32 = 10_000;
pub const ATTACHMENT_LOCK_NAMESPACE: i32 = 1_907_001;
pub const STORAGE_LOCK_NAMESPACE: i32 = 1_907_002;

pub fn is_hwp_attachment(name: &str, mime: &str) -> bool {
    mime.to_ascii_lowercase().starts_with("application/x-hwp")
        || name.to_ascii_lowercase().ends_with(".hwp")
        || name.to_ascii_lowercase().ends_with(".hwpx")
}

pub fn initial_extract_status(name: &str, mime: &str) -> &'static str {
    if is_hwp_attachment(name, mime) {
        "pending"
    } else {
        "skipped"
    }
}
