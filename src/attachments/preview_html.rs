//! On-demand text for `GET …/attachments/{id}/preview-html` (source
//! `previewHtmlOf`). The extract job's stored text is the preview; only when
//! it is still empty (job pending or failed) does the request parse the
//! original itself, through the same isolated children as the job, under the
//! source's request-time bounds: 20 MiB input and a 10 s parse. Nothing here
//! writes extract state or holds a database transaction.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use crate::attachments::extract_job::{read_extract_input, run_extractor};
use crate::attachments::{ExtractJobSettings, ObjectStorage};
use crate::db::attachment_extract::{default_extract_limits, EXTRACT_RETRY_BACKOFF_MS};
use crate::documents::office::OfficeLimits;

/// Source `OFFICE_PREVIEW_MAX_BYTES`: the original is read into memory.
pub const PREVIEW_MAX_INPUT_BYTES: u64 = 20 * 1024 * 1024;
/// Source `PREVIEW_PARSE_TIMEOUT_MS`: a person is waiting on the response.
pub const PREVIEW_PARSE_TIMEOUT: Duration = Duration::from_secs(10);
/// Parses in flight per process; a request past it is answered 429 at once.
pub const PREVIEW_PARSE_CONCURRENCY: usize = 2;
/// `Retry-After` for a refused parse.
pub const PREVIEW_BUSY_RETRY_AFTER_SECS: u32 = 2;

/// The request-time extractor and its admission. Clones share the permits.
#[derive(Clone)]
pub struct PreviewExtractor {
    settings: Arc<ExtractJobSettings>,
    permits: Arc<Semaphore>,
    deadline: Duration,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PreviewParse {
    Text(String),
    /// Too large, unsupported, corrupt, over a limit or timed out.
    Unavailable,
    /// Every parse slot is taken.
    Busy,
}

impl PreviewExtractor {
    /// The helpers the extract job resolved (`FVOCI_EXTRACTOR_BIN`, this
    /// executable as the office child), with the preview bounds.
    pub fn from_extract_settings(settings: &ExtractJobSettings) -> Self {
        Self::new(
            settings.extractor_bin.clone(),
            settings.office_helper.clone(),
        )
    }

    pub fn new(extractor_bin: Option<PathBuf>, office_helper: Option<PathBuf>) -> Self {
        Self::with_bounds(
            extractor_bin,
            office_helper,
            PREVIEW_PARSE_TIMEOUT,
            PREVIEW_PARSE_CONCURRENCY,
        )
    }

    /// `timeout` only ever tightens the job's limits.
    pub fn with_bounds(
        extractor_bin: Option<PathBuf>,
        office_helper: Option<PathBuf>,
        timeout: Duration,
        concurrency: usize,
    ) -> Self {
        let timeout = timeout
            .min(PREVIEW_PARSE_TIMEOUT)
            .max(Duration::from_secs(1));
        let mut limits = default_extract_limits();
        limits.max_input_bytes = limits.max_input_bytes.min(PREVIEW_MAX_INPUT_BYTES);
        limits.timeout_ms = limits.timeout_ms.min(timeout.as_millis() as u64);
        let mut office_limits = OfficeLimits::attachment();
        office_limits.max_input_bytes = office_limits.max_input_bytes.min(PREVIEW_MAX_INPUT_BYTES);
        office_limits.timeout = office_limits.timeout.min(timeout);
        let settings = ExtractJobSettings {
            extractor_bin,
            limits,
            office_helper,
            office_limits,
            poll_interval: Duration::from_secs(1),
            retry_backoff: Duration::from_millis(EXTRACT_RETRY_BACKOFF_MS),
            #[cfg(feature = "extract-native-tests")]
            test_hang_ms: None,
        };
        Self {
            settings: Arc::new(settings),
            permits: Arc::new(Semaphore::new(concurrency.max(1))),
            // The child has its own watchdog; this also bounds the storage
            // read and the wait for the native helper's single child slot.
            deadline: timeout * 2,
        }
    }

    fn max_input_bytes(&self) -> u64 {
        self.settings
            .limits
            .max_input_bytes
            .min(self.settings.office_limits.max_input_bytes)
    }

    /// Parses `storage_key` in a detached task that owns the permit, so a
    /// dropped request or the deadline cancels the child (killed and reaped)
    /// before the slot frees.
    pub async fn parse(
        &self,
        storage: &ObjectStorage,
        storage_key: &str,
        name: &str,
        mime: &str,
        size_bytes: Option<i64>,
    ) -> PreviewParse {
        let max = self.max_input_bytes();
        match size_bytes {
            Some(size) if size >= 0 && size as u64 <= max => {}
            _ => return PreviewParse::Unavailable,
        }
        let Ok(permit) = self.permits.clone().try_acquire_owned() else {
            return PreviewParse::Busy;
        };
        let cancel = CancellationToken::new();
        let _cancel_on_drop = cancel.clone().drop_guard();
        let settings = self.settings.clone();
        let storage = storage.clone();
        let (storage_key, name, mime) =
            (storage_key.to_string(), name.to_string(), mime.to_string());
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move {
            let _permit = permit;
            let bytes = tokio::select! {
                () = task_cancel.cancelled() => return None,
                read = read_extract_input(&storage, &storage_key, max) => read.ok()?,
            };
            match run_extractor(&settings, &name, &mime, bytes, &task_cancel).await {
                Ok(Some(finish)) if matches!(finish.status.as_str(), "ok" | "partial") => {
                    Some(finish.text).filter(|text| !text.is_empty())
                }
                Ok(_) => None,
                Err(err) => {
                    tracing::warn!(error = %err, "attachment preview parse failed");
                    None
                }
            }
        });
        match tokio::time::timeout(self.deadline, task).await {
            Ok(Ok(Some(text))) => PreviewParse::Text(text),
            Ok(_) => PreviewParse::Unavailable,
            Err(_) => {
                cancel.cancel();
                PreviewParse::Unavailable
            }
        }
    }
}
