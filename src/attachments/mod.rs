mod disposition;
mod local;
mod mime;
mod range;

pub use disposition::content_disposition_attachment;
pub use local::{LocalStorage, PartInfo, StagedPart, StorageError};
pub use mime::{is_image_mime, sniff_mime_from_bytes};
pub use range::{parse_range, ParsedRange};

use std::fmt;

#[derive(Clone)]
pub struct UploadLimits {
    pub part_size_bytes: i64,
    pub max_file_size_bytes: i64,
    pub create_rate_per_5min: u32,
}

impl fmt::Debug for UploadLimits {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UploadLimits")
            .field("part_size_bytes", &self.part_size_bytes)
            .field("max_file_size_bytes", &self.max_file_size_bytes)
            .field("create_rate_per_5min", &self.create_rate_per_5min)
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
