//! In-process preview worker (source BullMQ `thumbnail` queue). Same shape as
//! the HWP extract job: poll a DB lease, do the work outside any transaction,
//! publish under the lease. Decoding happens only in the rlimited child
//! ([`super::preview::run_preview_helper`]).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};
use uuid::Uuid;

use super::preview::{preview_mime_supported, run_preview_helper, PreviewError, PreviewLimits};
use super::ObjectStorage;
use crate::db::attachment_preview::{
    claim_preview, fail_preview, journal_preview_key, load_preview_input, publish_preview,
    release_preview, PreviewClaim,
};

#[derive(Debug, Clone)]
pub struct PreviewJobSettings {
    /// The preview child; production uses this binary itself.
    pub helper: PathBuf,
    pub limits: PreviewLimits,
    pub poll_interval: Duration,
}

impl PreviewJobSettings {
    pub fn new(helper: PathBuf) -> Self {
        Self {
            helper,
            limits: PreviewLimits::default(),
            poll_interval: Duration::from_secs(5),
        }
    }
}

pub struct PreviewJobHandle {
    cancel: CancellationToken,
    join: JoinHandle<()>,
    pub wake: Arc<Notify>,
}

impl PreviewJobHandle {
    pub fn request_shutdown(&self) {
        self.cancel.cancel();
    }

    pub async fn join(self) -> Result<(), String> {
        self.join
            .await
            .map_err(|err| format!("attachment preview task join failed: {err}"))
    }
}

pub fn spawn_preview_job(
    settings: PreviewJobSettings,
    pool: PgPool,
    storage: ObjectStorage,
) -> PreviewJobHandle {
    let cancel = CancellationToken::new();
    let wake = Arc::new(Notify::new());
    let join = tokio::spawn(run_preview_loop(
        settings,
        pool,
        storage,
        cancel.child_token(),
        wake.clone(),
    ));
    PreviewJobHandle { cancel, join, wake }
}

async fn run_preview_loop(
    settings: PreviewJobSettings,
    pool: PgPool,
    storage: ObjectStorage,
    cancel: CancellationToken,
    wake: Arc<Notify>,
) {
    while !cancel.is_cancelled() {
        let worked = match process_one_preview(&settings, &pool, &storage, &cancel).await {
            Ok(worked) => worked,
            Err(err) => {
                warn!(error = %err, "attachment preview cycle failed");
                false
            }
        };
        if cancel.is_cancelled() {
            break;
        }
        if worked {
            continue;
        }
        tokio::select! {
            () = cancel.cancelled() => break,
            () = wake.notified() => {},
            () = tokio::time::sleep(settings.poll_interval) => {},
        }
    }
}

/// Claims and processes at most one preview. `Ok(true)` when a claim was
/// taken (so the loop polls again at once).
pub async fn process_one_preview(
    settings: &PreviewJobSettings,
    pool: &PgPool,
    storage: &ObjectStorage,
    cancel: &CancellationToken,
) -> Result<bool, String> {
    let Some(claim) = claim_preview(pool)
        .await
        .map_err(|e| format!("claim failed: {e}"))?
    else {
        return Ok(false);
    };
    debug!(attachment_id = %claim.attachment_id, attempt = claim.attempt, "claimed preview lease");
    let Some(input) = load_preview_input(pool, &claim)
        .await
        .map_err(|e| format!("load failed: {e}"))?
    else {
        return Ok(true);
    };
    if !preview_mime_supported(&input.mime) {
        fail(pool, &claim, "unsupported format").await?;
        return Ok(true);
    }
    // Source: the row's size (already checked against the upload limit and
    // the stored object) caps the read below the static byte limit.
    let cap = settings
        .limits
        .input_bytes
        .min(input.size_bytes.max(0) as u64);
    let source = match read_bounded(storage, &input.storage_key, cap).await {
        Ok(Some(source)) => source,
        Ok(None) => {
            fail(pool, &claim, "stored object exceeds its row size").await?;
            return Ok(true);
        }
        Err(err) => return Err(err),
    };
    let rendered = tokio::select! {
        () = cancel.cancelled() => {
            let _ = release_preview(pool, &claim).await;
            return Ok(true);
        }
        rendered = run_preview_helper(&settings.helper, source, &settings.limits) => rendered,
    };
    let preview = match rendered {
        Ok(preview) => preview,
        Err(PreviewError::Rejected(msg)) | Err(PreviewError::ResourceLimit(msg)) => {
            fail(pool, &claim, &msg).await?;
            return Ok(true);
        }
        // Retryable: the lease expires and the claim counts the attempt.
        Err(PreviewError::Worker(msg)) => return Err(msg),
    };
    let key = Uuid::now_v7().to_string();
    let journal_id = journal_preview_key(pool, &claim, &key)
        .await
        .map_err(|e| format!("journal failed: {e}"))?;
    let bytes = preview.webp.len() as u64;
    storage
        .put_bytes(&key, preview.webp)
        .await
        .map_err(|e| format!("preview store failed: {e}"))?;
    let published = publish_preview(
        pool,
        &claim,
        journal_id,
        &key,
        preview.width,
        preview.height,
        bytes,
    )
    .await
    .map_err(|e| format!("publish failed: {e}"))?;
    if !published {
        warn!(attachment_id = %claim.attachment_id, "preview lease lost; key left for reclaim");
    }
    Ok(true)
}

async fn fail(pool: &PgPool, claim: &PreviewClaim, reason: &str) -> Result<(), String> {
    warn!(attachment_id = %claim.attachment_id, reason, "attachment preview failed");
    fail_preview(pool, claim)
        .await
        .map(|_| ())
        .map_err(|e| format!("fail update failed: {e}"))
}

/// `Ok(None)` when the object is larger than `cap` (a row/object mismatch).
async fn read_bounded(
    storage: &ObjectStorage,
    key: &str,
    cap: u64,
) -> Result<Option<Vec<u8>>, String> {
    let size = storage
        .head(key)
        .await
        .map_err(|e| format!("storage head failed: {e}"))?
        .ok_or_else(|| format!("storage object missing for key {key}"))?;
    if size > cap {
        return Ok(None);
    }
    if size == 0 {
        return Ok(Some(Vec::new()));
    }
    storage
        .read_range(key, 0, size - 1)
        .await
        .map(Some)
        .map_err(|e| format!("storage read failed: {e}"))
}
