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
    read_extract_input, spawn_extract_job, spawn_extract_job_with_embedder, validate_extractor_bin,
    ExtractJobHandle, ExtractJobSettings,
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

/// Admission for part PUTs in flight. Each proxied part holds an inbound
/// connection and, with S3, an outbound one for as long as the client paces
/// its body. A process-wide pool bounds the total, a per-user share keeps one
/// user from exhausting that pool for every tenant, and every admitted body
/// has a deadline so a slot is always released. A refused PUT is answered
/// before its body is read.
#[derive(Clone)]
pub struct PartPutSlots {
    semaphore: std::sync::Arc<tokio::sync::Semaphore>,
    max: u32,
    per_user_max: u32,
    per_user: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<uuid::Uuid, u32>>>,
    body_base: std::time::Duration,
    body_min_bytes_per_sec: u64,
    body_margin: std::time::Duration,
}

/// One admitted part PUT; releases its global and per-user share on drop.
pub struct PartPutSlot {
    _permit: tokio::sync::OwnedSemaphorePermit,
    user_id: uuid::Uuid,
    per_user: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<uuid::Uuid, u32>>>,
}

impl Drop for PartPutSlot {
    fn drop(&mut self) {
        let mut map = self
            .per_user
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(count) = map.get_mut(&self.user_id) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                map.remove(&self.user_id);
            }
        }
    }
}

/// Fixed allowance for a part body on top of its length-scaled share.
pub const PART_BODY_BASE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(60);
/// Slowest accepted average inbound rate for a whole part body.
pub const PART_BODY_MIN_BYTES_PER_SEC: u64 = 64 * 1024;
/// Slack over a storage driver's own deadline at the same rate (the S3
/// response-header timeout plus scheduling margin), so a stall on the storage
/// side ends as that driver's logged server error rather than a client 400.
pub const PART_BODY_OUTER_MARGIN: std::time::Duration = std::time::Duration::from_secs(35);

impl PartPutSlots {
    pub fn new(max: u32) -> Self {
        Self::with_per_user(
            max,
            crate::config::DEFAULT_UPLOAD_MAX_CONCURRENT_PARTS_PER_USER,
        )
    }

    /// `per_user_max` is clamped to `max`.
    pub fn with_per_user(max: u32, per_user_max: u32) -> Self {
        Self {
            semaphore: std::sync::Arc::new(tokio::sync::Semaphore::new(max as usize)),
            max,
            per_user_max: per_user_max.clamp(1, max.max(1)),
            per_user: Default::default(),
            body_base: PART_BODY_BASE_DEADLINE,
            body_min_bytes_per_sec: PART_BODY_MIN_BYTES_PER_SEC,
            body_margin: PART_BODY_OUTER_MARGIN,
        }
    }

    /// Exact body deadline (no storage-driver margin), for tests.
    pub fn with_body_deadline(mut self, base: std::time::Duration, min_bytes_per_sec: u64) -> Self {
        self.body_base = base;
        self.body_min_bytes_per_sec = min_bytes_per_sec.max(1);
        self.body_margin = std::time::Duration::ZERO;
        self
    }

    pub fn max(&self) -> u32 {
        self.max
    }

    pub fn per_user_max(&self) -> u32 {
        self.per_user_max
    }

    /// Deadline for receiving and staging a part body of `len` bytes.
    pub fn body_deadline(&self, len: u64) -> std::time::Duration {
        self.body_base
            + self.body_margin
            + std::time::Duration::from_secs_f64(len as f64 / self.body_min_bytes_per_sec as f64)
    }

    /// A slot held until the returned guard drops, or `None` when the user's
    /// share or the process-wide pool is used up.
    pub fn try_acquire(&self, user_id: uuid::Uuid) -> Option<PartPutSlot> {
        let mut map = self
            .per_user
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let held = map.get(&user_id).copied().unwrap_or(0);
        if held >= self.per_user_max {
            return None;
        }
        let permit = self.semaphore.clone().try_acquire_owned().ok()?;
        map.insert(user_id, held + 1);
        Some(PartPutSlot {
            _permit: permit,
            user_id,
            per_user: self.per_user.clone(),
        })
    }
}

impl fmt::Debug for UploadLimits {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UploadLimits")
            .field("part_size_bytes", &self.part_size_bytes)
            .field("max_file_size_bytes", &self.max_file_size_bytes)
            .field("create_rate_per_5min", &self.create_rate_per_5min)
            .field("max_concurrent_part_puts", &self.part_put_slots.max())
            .field(
                "max_concurrent_part_puts_per_user",
                &self.part_put_slots.per_user_max(),
            )
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

#[cfg(test)]
mod part_slot_tests {
    use super::PartPutSlots;
    use uuid::Uuid;

    #[test]
    fn one_user_cannot_take_the_whole_pool() {
        let slots = PartPutSlots::with_per_user(3, 2);
        let (a, b) = (Uuid::now_v7(), Uuid::now_v7());
        let a1 = slots.try_acquire(a).expect("a1");
        let _a2 = slots.try_acquire(a).expect("a2");
        assert!(slots.try_acquire(a).is_none(), "a is at its share");
        let b1 = slots.try_acquire(b).expect("b still admitted");
        assert!(slots.try_acquire(b).is_none(), "the pool is full");
        drop(a1);
        let _a3 = slots.try_acquire(a).expect("a's share came back");
        drop(b1);
        let _b2 = slots.try_acquire(b).expect("b's share came back");
    }

    #[test]
    fn per_user_share_is_clamped_to_the_pool_and_deadline_scales() {
        let slots = PartPutSlots::with_per_user(2, 10);
        assert_eq!(slots.per_user_max(), 2);
        let slots = slots.with_body_deadline(std::time::Duration::from_secs(1), 1024);
        assert_eq!(slots.body_deadline(2048), std::time::Duration::from_secs(3));
        // The default leaves room for the S3 driver's own deadline to fire
        // first at the same rate.
        let defaults = PartPutSlots::new(4);
        assert_eq!(
            defaults.body_deadline(64 * 1024),
            super::PART_BODY_BASE_DEADLINE
                + super::PART_BODY_OUTER_MARGIN
                + std::time::Duration::from_secs(1)
        );
    }
}
