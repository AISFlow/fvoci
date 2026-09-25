use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use document_extract_client::limits::{Limits, DEFAULT_TIMEOUT_MS};
use document_extract_client::outcome::ExtractStatus;
use document_extract_client::process::{extract_killable_with_cancel, Cancelled, ExtractRequest};
use document_extract_client::ExtractReport;
use sqlx::PgPool;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::attachments::ObjectStorage;
use crate::db::attachment_extract::{
    claim_extract, default_extract_limits, finish_extract, load_extract_input,
    oversize_resource_limit, release_extract, FinishExtract, EXTRACT_LEASE_SECS,
    EXTRACT_RETRY_BACKOFF_MS,
};

const _: () = assert!(EXTRACT_LEASE_SECS * 1000 > DEFAULT_TIMEOUT_MS);

#[derive(Debug, Clone)]
pub struct ExtractJobSettings {
    pub extractor_bin: PathBuf,
    pub limits: Limits,
    pub poll_interval: Duration,
    pub retry_backoff: Duration,
    /// Test-only hang injection; compiled only for `extract-native-tests`.
    #[cfg(feature = "extract-native-tests")]
    pub test_hang_ms: Option<u64>,
}

fn extract_request_test_hang(settings: &ExtractJobSettings) -> Option<u64> {
    #[cfg(feature = "extract-native-tests")]
    {
        settings.test_hang_ms
    }
    #[cfg(not(feature = "extract-native-tests"))]
    {
        let _ = settings;
        None
    }
}

impl ExtractJobSettings {
    pub fn from_env() -> Result<Option<Self>, String> {
        let raw = match std::env::var("FVOCI_EXTRACTOR_BIN") {
            Ok(value) if !value.trim().is_empty() => value,
            _ => return Ok(None),
        };
        let extractor_bin = PathBuf::from(raw.trim());
        validate_extractor_bin(&extractor_bin)?;

        let poll_secs = parse_positive_u64(
            "FVOCI_EXTRACT_POLL_SECS",
            std::env::var("FVOCI_EXTRACT_POLL_SECS").ok().as_deref(),
            30,
        )?;

        let limits = default_extract_limits();
        limits
            .validate()
            .map_err(|e| format!("invalid extract limits: {e}"))?;

        Ok(Some(Self {
            extractor_bin,
            limits,
            poll_interval: Duration::from_secs(poll_secs),
            retry_backoff: Duration::from_millis(EXTRACT_RETRY_BACKOFF_MS),
            #[cfg(feature = "extract-native-tests")]
            test_hang_ms: None,
        }))
    }
}

pub fn validate_extractor_bin(path: &std::path::Path) -> Result<(), String> {
    if path.as_os_str().is_empty() {
        return Err("FVOCI_EXTRACTOR_BIN is empty".into());
    }
    if !path.is_file() {
        return Err(format!(
            "FVOCI_EXTRACTOR_BIN must point at an existing file: {}",
            path.display()
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = std::fs::metadata(path)
            .map_err(|e| format!("FVOCI_EXTRACTOR_BIN metadata read failed: {e}"))?;
        if meta.permissions().mode() & 0o111 == 0 {
            return Err(format!(
                "FVOCI_EXTRACTOR_BIN is not executable: {}",
                path.display()
            ));
        }
    }
    Ok(())
}

pub struct ExtractJobHandle {
    cancel: CancellationToken,
    join: JoinHandle<()>,
    pub wake: Arc<Notify>,
}

impl ExtractJobHandle {
    pub fn request_shutdown(&self) {
        self.cancel.cancel();
    }

    pub async fn join(self) -> Result<(), String> {
        self.join
            .await
            .map_err(|err| format!("attachment extract task join failed: {err}"))?;
        Ok(())
    }
}

pub fn spawn_extract_job(
    settings: ExtractJobSettings,
    pool: PgPool,
    storage: ObjectStorage,
) -> ExtractJobHandle {
    let cancel = CancellationToken::new();
    let wake = Arc::new(Notify::new());
    let child_cancel = cancel.child_token();
    let join = tokio::spawn(run_extract_loop(
        settings,
        pool,
        storage,
        child_cancel,
        wake.clone(),
    ));
    ExtractJobHandle { cancel, join, wake }
}

async fn run_extract_loop(
    settings: ExtractJobSettings,
    pool: PgPool,
    storage: ObjectStorage,
    cancel: CancellationToken,
    wake: Arc<Notify>,
) {
    while !cancel.is_cancelled() {
        let worked = match process_one_claim(&settings, &pool, &storage, &cancel).await {
            Ok(worked) => worked,
            Err(err) => {
                warn!(error = %err, "attachment extract claim cycle failed");
                false
            }
        };

        if cancel.is_cancelled() {
            break;
        }

        let delay = if worked {
            settings.retry_backoff
        } else {
            settings.poll_interval
        };
        tokio::select! {
            () = cancel.cancelled() => break,
            () = wake.notified() => {},
            () = tokio::time::sleep(delay) => {},
        }
    }
}

async fn process_one_claim(
    settings: &ExtractJobSettings,
    pool: &PgPool,
    storage: &ObjectStorage,
    cancel: &CancellationToken,
) -> Result<bool, String> {
    let claim = claim_extract(pool)
        .await
        .map_err(|e| format!("claim failed: {e}"))?;
    let Some(claim) = claim else {
        return Ok(false);
    };

    debug!(
        workspace_id = %claim.workspace_id,
        attachment_id = %claim.attachment_id,
        attempt = claim.attempt,
        "claimed attachment extract lease"
    );

    if cancel.is_cancelled() {
        let _ = release_extract(pool, &claim)
            .await
            .map_err(|e| format!("release after cancel failed: {e}"))?;
        return Ok(true);
    }

    let input = load_extract_input(pool, &claim)
        .await
        .map_err(|e| format!("load failed: {e}"))?;
    let Some(input) = input else {
        warn!(
            workspace_id = %claim.workspace_id,
            attachment_id = %claim.attachment_id,
            "extract load matched no leased row"
        );
        return Ok(true);
    };

    let finish = if input.size_bytes as u64 > settings.limits.max_input_bytes {
        oversize_resource_limit(input.size_bytes)
    } else {
        let bytes =
            read_extract_input(storage, &input.storage_key, settings.limits.max_input_bytes)
                .await
                .map_err(|e| format!("storage read failed: {e}"))?;
        if cancel.is_cancelled() {
            let _ = release_extract(pool, &claim)
                .await
                .map_err(|e| format!("release after cancel failed: {e}"))?;
            return Ok(true);
        }
        match run_native_extract(settings, &input.name, bytes, cancel).await? {
            Some(finish) => finish,
            None => {
                let _ = release_extract(pool, &claim)
                    .await
                    .map_err(|e| format!("release after cancel failed: {e}"))?;
                return Ok(true);
            }
        }
    };

    let applied = finish_extract(pool, &claim, &finish)
        .await
        .map_err(|e| format!("finish failed: {e}"))?;
    if !applied {
        warn!(
            workspace_id = %claim.workspace_id,
            attachment_id = %claim.attachment_id,
            "extract finish matched no leased row"
        );
    }
    Ok(true)
}

/// Reads an attachment original through the configured `ObjectStorage`
/// (local file or S3 ranged GET) into memory, refusing anything above
/// `max_bytes` before reading. The bytes go to the extractor over stdin, so
/// no temporary file is written on either driver.
pub async fn read_extract_input(
    storage: &ObjectStorage,
    storage_key: &str,
    max_bytes: u64,
) -> Result<Vec<u8>, String> {
    let size = match storage.head(storage_key).await {
        Ok(Some(size)) => size,
        Ok(None) => {
            return Err(format!("storage object missing for key {}", storage_key));
        }
        Err(err) => return Err(format!("storage head failed: {err}")),
    };
    if size > max_bytes {
        return Err(format!(
            "storage object {} bytes exceeds {}-byte extract limit",
            size, max_bytes
        ));
    }
    if size == 0 {
        return Ok(Vec::new());
    }
    storage
        .read_range(storage_key, 0, size - 1)
        .await
        .map_err(|e| format!("storage read failed: {e}"))
}

async fn run_native_extract(
    settings: &ExtractJobSettings,
    name: &str,
    bytes: Vec<u8>,
    cancel: &CancellationToken,
) -> Result<Option<FinishExtract>, String> {
    let cancel_flag = Arc::new(AtomicBool::new(false));
    let request = ExtractRequest {
        bytes,
        name: name.to_string(),
        limits: settings.limits,
        extractor_bin: settings.extractor_bin.clone(),
        test_hang_ms: extract_request_test_hang(settings),
    };
    let worker_cancel = cancel_flag.clone();
    let handle = tokio::task::spawn_blocking(move || {
        extract_killable_with_cancel(request, worker_cancel.as_ref())
    });

    let mut handle = handle;
    let join_result = tokio::select! {
        () = cancel.cancelled() => {
            cancel_flag.store(true, Ordering::Release);
            handle.await
        }
        join_result = &mut handle => join_result,
    };

    match join_result {
        Ok(Ok(report)) => Ok(Some(map_extract_report(report))),
        Ok(Err(Cancelled { .. })) => Ok(None),
        Err(err) => Err(format!("extract worker join failed: {err}")),
    }
}

fn map_extract_report(report: ExtractReport) -> FinishExtract {
    let rhwp_rev = Some(report.rhwp_rev);
    match report.outcome {
        ExtractStatus::Ok { text, warnings, .. } => {
            let (text, status, extra) = sanitize_text(text, "ok");
            FinishExtract {
                status,
                text,
                warnings: merge_warnings(warnings, extra),
                rhwp_rev,
            }
        }
        ExtractStatus::Partial { text, warnings, .. } => {
            let (text, status, extra) = sanitize_text(text, "partial");
            FinishExtract {
                status,
                text,
                warnings: merge_warnings(warnings, extra),
                rhwp_rev,
            }
        }
        ExtractStatus::Empty { warnings, .. } => FinishExtract {
            status: "empty".into(),
            text: String::new(),
            warnings,
            rhwp_rev,
        },
        ExtractStatus::Unsupported { .. } => FinishExtract {
            status: "unsupported".into(),
            text: String::new(),
            warnings: vec![],
            rhwp_rev,
        },
        ExtractStatus::Corrupt { .. } => FinishExtract {
            status: "corrupt".into(),
            text: String::new(),
            warnings: vec![],
            rhwp_rev,
        },
        ExtractStatus::ResourceLimit { .. } => FinishExtract {
            status: "resource_limit".into(),
            text: String::new(),
            warnings: vec![],
            rhwp_rev,
        },
        ExtractStatus::WorkerFailure { .. } => FinishExtract {
            status: "worker_failure".into(),
            text: String::new(),
            warnings: vec![],
            rhwp_rev,
        },
    }
}

fn sanitize_text(text: String, default_status: &str) -> (String, String, Vec<String>) {
    if !text.contains('\0') {
        return (text, default_status.to_string(), Vec::new());
    }
    let cleaned = text.replace('\0', "");
    let status = if default_status == "ok" {
        "partial".to_string()
    } else {
        default_status.to_string()
    };
    (
        cleaned,
        status,
        vec!["U+0000 stripped from extracted text".to_string()],
    )
}

fn merge_warnings(mut base: Vec<String>, extra: Vec<String>) -> Vec<String> {
    base.extend(extra);
    base.truncate(document_extract_client::limits::MAX_WARNING_ENTRIES);
    base
}

fn parse_positive_u64(name: &str, raw: Option<&str>, default: u64) -> Result<u64, String> {
    let Some(raw) = raw else {
        return Ok(default);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(format!("{name} must be a positive integer"));
    }
    let value: u64 = trimmed
        .parse()
        .map_err(|e| format!("invalid {name}: {e}"))?;
    if value == 0 {
        return Err(format!("{name} must be a positive integer"));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nul_bytes_downgrade_ok_to_partial() {
        let (text, status, warnings) = sanitize_text("a\0b".to_string(), "ok");
        assert_eq!(text, "ab");
        assert_eq!(status, "partial");
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn lease_covers_watchdog() {
        use crate::db::attachment_extract::EXTRACT_MAX_ATTEMPTS;
        use document_extract_client::limits::MAX_INPUT_BYTES;

        const _: () = assert!(EXTRACT_LEASE_SECS * 1000 > DEFAULT_TIMEOUT_MS);
        assert_eq!(EXTRACT_MAX_ATTEMPTS, 2);
        const _: () = assert!(MAX_INPUT_BYTES == 20 * 1024 * 1024);
    }
}
