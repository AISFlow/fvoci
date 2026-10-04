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
    claim_preview_backend, fail_preview_backend, journal_preview_key_backend,
    load_preview_input_backend, publish_preview_backend_with_cancel, release_preview_backend,
    PreviewClaim,
};
use crate::db::backend::Backend;

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
    spawn_preview_job_backend(settings, Backend::Postgres(pool), storage)
}

/// Poll the selected backend with the same child and storage boundaries.
pub fn spawn_preview_job_backend(
    settings: PreviewJobSettings,
    backend: Backend,
    storage: ObjectStorage,
) -> PreviewJobHandle {
    let cancel = CancellationToken::new();
    let wake = Arc::new(Notify::new());
    let join = tokio::spawn(run_preview_loop(
        settings,
        backend,
        storage,
        cancel.child_token(),
        wake.clone(),
    ));
    PreviewJobHandle { cancel, join, wake }
}

async fn run_preview_loop(
    settings: PreviewJobSettings,
    backend: Backend,
    storage: ObjectStorage,
    cancel: CancellationToken,
    wake: Arc<Notify>,
) {
    while !cancel.is_cancelled() {
        let worked = match process_one_preview_backend(&settings, &backend, &storage, &cancel).await
        {
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
    process_one_preview_backend(settings, &Backend::Postgres(pool.clone()), storage, cancel).await
}

/// Claims and processes one actual selected-backend preview; no DB transaction
/// survives an input read, native child, or storage write.
pub async fn process_one_preview_backend(
    settings: &PreviewJobSettings,
    backend: &Backend,
    storage: &ObjectStorage,
    cancel: &CancellationToken,
) -> Result<bool, String> {
    if cancel.is_cancelled() {
        return Ok(false);
    }
    let Some(claim) = claim_preview_backend(backend)
        .await
        .map_err(|e| format!("claim failed: {e}"))?
    else {
        return Ok(false);
    };
    debug!(attachment_id = %claim.attachment_id, attempt = claim.attempt, "claimed preview lease");
    let Some(input) = load_preview_input_backend(backend, &claim)
        .await
        .map_err(|e| format!("load failed: {e}"))?
    else {
        return Ok(true);
    };
    if !preview_mime_supported(&input.mime) {
        fail(backend, &claim, "unsupported format").await?;
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
            fail(backend, &claim, "stored object exceeds its row size").await?;
            return Ok(true);
        }
        Err(err) => return Err(err),
    };
    if cancel.is_cancelled() {
        let _ = release_preview_backend(backend, &claim).await;
        return Ok(true);
    }
    #[cfg(all(test, feature = "db-tests"))]
    tests::pause(claim.attachment_id, "child").await;
    let rendered = tokio::select! {
        () = cancel.cancelled() => {
            let _ = release_preview_backend(backend, &claim).await;
            return Ok(true);
        }
        rendered = run_preview_helper(&settings.helper, source, &settings.limits) => rendered,
    };
    let preview = match rendered {
        Ok(preview) => preview,
        Err(PreviewError::Rejected(msg)) | Err(PreviewError::ResourceLimit(msg)) => {
            fail(backend, &claim, &msg).await?;
            return Ok(true);
        }
        // Retryable: the lease expires and the claim counts the attempt.
        Err(PreviewError::Worker(msg)) => return Err(msg),
    };
    let key = Uuid::now_v7().to_string();
    let Some(journal_id) = journal_preview_key_backend(backend, &claim, &key)
        .await
        .map_err(|e| format!("journal failed: {e}"))?
    else {
        return Ok(true);
    };
    #[cfg(all(test, feature = "db-tests"))]
    tests::pause(claim.attachment_id, "journal").await;
    if cancel.is_cancelled() {
        let _ = release_preview_backend(backend, &claim).await;
        return Ok(true);
    }
    let bytes = preview.webp.len() as u64;
    tokio::select! {
        () = cancel.cancelled() => {
            let _ = release_preview_backend(backend, &claim).await;
            return Ok(true);
        }
        stored = storage.put_bytes(&key, preview.webp) => {
            stored.map_err(|e| format!("preview store failed: {e}"))?;
        }
    }
    if cancel.is_cancelled() {
        let _ = release_preview_backend(backend, &claim).await;
        return Ok(true);
    }
    #[cfg(all(test, feature = "db-tests"))]
    tests::pause(claim.attachment_id, "stored").await;
    let published = publish_preview_backend_with_cancel(
        backend,
        &claim,
        journal_id,
        &key,
        preview.width,
        preview.height,
        bytes,
        Some(cancel),
    )
    .await
    .map_err(|e| format!("publish failed: {e}"))?;
    if !published {
        if cancel.is_cancelled() {
            let _ = release_preview_backend(backend, &claim).await;
        }
        warn!(attachment_id = %claim.attachment_id, "preview lease lost; key left for reclaim");
    }
    Ok(true)
}

async fn fail(backend: &Backend, claim: &PreviewClaim, reason: &str) -> Result<(), String> {
    warn!(attachment_id = %claim.attachment_id, reason, "attachment preview failed");
    fail_preview_backend(backend, claim)
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

#[cfg(all(test, feature = "db-tests"))]
mod tests {
    use super::*;
    use crate::db::attachment_preview::tests::Fixture;
    use sha2::{Digest, Sha256};
    use std::io::Cursor;
    use std::sync::{LazyLock, Mutex};

    #[derive(Clone)]
    struct Gate {
        id: Uuid,
        stage: &'static str,
        reached: Arc<Notify>,
        release: Arc<Notify>,
    }
    static GATE: LazyLock<Mutex<Option<Gate>>> = LazyLock::new(|| Mutex::new(None));
    pub(super) async fn pause(id: Uuid, stage: &str) {
        let gate = GATE
            .lock()
            .unwrap()
            .clone()
            .filter(|g| g.id == id && g.stage == stage);
        if let Some(g) = gate {
            g.reached.notify_one();
            g.release.notified().await;
        }
    }
    fn arm(id: Uuid, stage: &'static str) -> Gate {
        let gate = Gate {
            id,
            stage,
            reached: Arc::new(Notify::new()),
            release: Arc::new(Notify::new()),
        };
        assert!(GATE.lock().unwrap().replace(gate.clone()).is_none());
        gate
    }
    fn disarm(gate: &Gate) {
        GATE.lock().unwrap().take();
        gate.release.notify_one();
    }
    fn settings() -> PreviewJobSettings {
        let path = std::env::var_os("FVOCI_PREVIEW_TEST_HELPER")
            .expect("root allocation must supply freshly built own image-helper binary");
        let path = PathBuf::from(path);
        assert!(path.is_absolute() && path.is_file());
        PreviewJobSettings::new(path)
    }
    fn png() -> Vec<u8> {
        let img =
            image::RgbaImage::from_fn(40, 20, |x, y| image::Rgba([x as u8, y as u8, 90, 255]));
        let mut out = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }
    async fn input(f: &Fixture) -> (Uuid, ObjectStorage) {
        let bytes = png();
        let (id, key) = f.attachment(bytes.len() as i64, "image/png").await;
        let storage = ObjectStorage::local(f.root.join("objects"));
        storage.put_bytes(&key, bytes).await.unwrap();
        (id, storage)
    }
    async fn run(
        settings: PreviewJobSettings,
        backend: Backend,
        storage: ObjectStorage,
        cancel: CancellationToken,
    ) -> Result<bool, String> {
        process_one_preview_backend(&settings, &backend, &storage, &cancel).await
    }

    #[tokio::test]
    async fn preview_selected_png_child_storage_publish_fresh_connection() {
        let f = Fixture::new().await;
        let (id, storage) = input(&f).await;
        assert!(process_one_preview_backend(
            &settings(),
            &f.backend,
            &storage,
            &CancellationToken::new()
        )
        .await
        .unwrap());
        assert!(!process_one_preview_backend(
            &settings(),
            &f.backend,
            &storage,
            &CancellationToken::new()
        )
        .await
        .unwrap());
        assert!(f.journals().await.is_empty());
        // Close the actual app pool before independent fresh readback.
        f.pool.close().await;
        let fresh = crate::db::pool::connect_sqlite_app(&f.path, 1)
            .await
            .unwrap();
        let (status, variants): (String, String) =
            sqlx::query_as("SELECT preview_status,variants FROM attachments WHERE id=?1")
                .bind(id.as_bytes().as_slice())
                .fetch_one(&fresh)
                .await
                .unwrap();
        assert_eq!(status, "ok");
        let variants: serde_json::Value = serde_json::from_str(&variants).unwrap();
        let v = crate::db::attachments::preview_variant_of(&variants).unwrap();
        assert_eq!((v.width, v.height), (40, 20));
        assert!(v.bytes > 0);
        assert_eq!(storage.head(&v.key).await.unwrap(), Some(v.bytes as u64));
        let stored = storage
            .read_range(&v.key, 0, v.bytes as u64 - 1)
            .await
            .unwrap();
        assert_eq!(
            crate::attachments::sniff_mime_from_bytes(&stored),
            "image/webp"
        );
        let img = image::load_from_memory(&stored).unwrap().into_rgba8();
        assert_eq!(img, image::load_from_memory(&png()).unwrap().into_rgba8());
        let etag = super::super::preview::preview_etag(&v.key, v.bytes, v.width, v.height);
        let expected = hex::encode(Sha256::digest(format!(
            "{}|{}|{}|{}",
            v.key, v.bytes, v.width, v.height
        )));
        assert_eq!(etag, format!("\"{}\"", &expected[..16]));
        println!(
            "S31 fresh-readback mime=image/webp bytes={} sha256={} etag={} dimensions={}x{}",
            stored.len(),
            hex::encode(Sha256::digest(&stored)),
            etag,
            v.width,
            v.height
        );
        fresh.close().await;
        f.close().await;
    }

    #[tokio::test]
    async fn preview_selected_storage_failure_preserves_actual_key_journal() {
        let f = Fixture::new().await;
        let (id, storage) = input(&f).await;
        let gate = arm(id, "journal");
        let task = tokio::spawn(run(
            settings(),
            f.backend.clone(),
            storage.clone(),
            CancellationToken::new(),
        ));
        gate.reached.notified().await;
        let journal = f.journals().await;
        assert_eq!(journal.len(), 1);
        let key = &journal[0].1;
        // Real local storage failure: occupy that actual future object's directory
        // with a file after input read/render and after journaling.
        let blocked = f.root.join("objects").join("objects").join(key);
        std::fs::write(&blocked, b"storage failure").unwrap();
        disarm(&gate);
        let result = task.await.unwrap();
        assert!(result.unwrap_err().contains("preview store failed"));
        assert_eq!(journal, f.journals().await);
        assert_eq!(f.row(id).await.0, "pending");
        f.close().await;
    }

    #[tokio::test]
    async fn preview_selected_cancellation_after_journal_releases_claim_keeps_pointer() {
        let f = Fixture::new().await;
        let (id, storage) = input(&f).await;
        let gate = arm(id, "journal");
        let cancel = CancellationToken::new();
        let task = tokio::spawn(run(
            settings(),
            f.backend.clone(),
            storage.clone(),
            cancel.clone(),
        ));
        gate.reached.notified().await;
        let journal = f.journals().await;
        assert_eq!(journal.len(), 1);
        cancel.cancel();
        disarm(&gate);
        assert!(task.await.unwrap().unwrap());
        let row = f.row(id).await;
        assert_eq!(row.0, "pending");
        assert_eq!(row.2, 0);
        assert!(row.3.is_none());
        assert_eq!(journal, f.journals().await);
        assert!(storage.head(&journal[0].1).await.unwrap().is_none());
        // Current positive can retry while old pointer survives.
        assert!(process_one_preview_backend(
            &settings(),
            &f.backend,
            &storage,
            &CancellationToken::new()
        )
        .await
        .unwrap());
        assert_eq!(f.row(id).await.0, "ok");
        assert_eq!(journal, f.journals().await);
        f.close().await;
    }

    #[tokio::test]
    async fn preview_selected_parent_deleted_after_storage_never_publishes() {
        let f = Fixture::new().await;
        let (id, storage) = input(&f).await;
        let gate = arm(id, "stored");
        let task = tokio::spawn(run(
            settings(),
            f.backend.clone(),
            storage.clone(),
            CancellationToken::new(),
        ));
        gate.reached.notified().await;
        let journal = f.journals().await;
        assert_eq!(journal.len(), 1);
        assert!(storage.head(&journal[0].1).await.unwrap().unwrap() > 0);
        sqlx::query("UPDATE documents SET deleted_at=1 WHERE id=?1")
            .bind(f.document.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        disarm(&gate);
        assert!(task.await.unwrap().unwrap());
        assert_eq!(f.row(id).await.0, "pending");
        assert_eq!(journal, f.journals().await);
        f.close().await;
    }

    #[tokio::test]
    async fn preview_selected_stale_consumer_after_storage_keeps_its_pointer() {
        let f = Fixture::new().await;
        let (id, storage) = input(&f).await;
        let gate = arm(id, "stored");
        let task = tokio::spawn(run(
            settings(),
            f.backend.clone(),
            storage.clone(),
            CancellationToken::new(),
        ));
        gate.reached.notified().await;
        let journal = f.journals().await;
        f.expire(id).await;
        let newer = crate::db::attachment_preview::claim_preview_backend(&f.backend)
            .await
            .unwrap()
            .unwrap();
        disarm(&gate);
        assert!(task.await.unwrap().unwrap());
        assert_eq!(f.row(id).await.0, "pending");
        assert_eq!(
            f.row(id).await.3,
            Some(newer.lease_token.as_bytes().to_vec())
        );
        assert_eq!(journal, f.journals().await);
        assert!(storage.head(&journal[0].1).await.unwrap().is_some());
        f.close().await;
    }

    #[tokio::test]
    async fn preview_selected_hostile_bytes_row_size_mime_and_child_limits_preserved() {
        let f = Fixture::new().await;
        let storage = ObjectStorage::local(f.root.join("objects"));
        for (bytes, mime, size) in [
            (b"bad PNG".to_vec(), "image/png", 7),
            (png(), "image/png", 1),
            (png(), "image/svg+xml", 300),
        ] {
            let (id, key) = f.attachment(size, mime).await;
            storage.put_bytes(&key, bytes).await.unwrap();
            assert!(process_one_preview_backend(
                &settings(),
                &f.backend,
                &storage,
                &CancellationToken::new()
            )
            .await
            .unwrap());
            assert_eq!(f.row(id).await.0, "failed");
            assert!(f.journals().await.is_empty());
        }
        let (id, storage) = input(&f).await;
        let mut tight = settings();
        tight.limits.input_pixels = 1;
        assert!(process_one_preview_backend(
            &tight,
            &f.backend,
            &storage,
            &CancellationToken::new()
        )
        .await
        .unwrap());
        assert_eq!(f.row(id).await.0, "failed");
        assert!(f.journals().await.is_empty());
        let source = png();
        let mut memory = PreviewLimits::default();
        memory.child_address_space = 32 * 1024 * 1024;
        assert!(matches!(
            run_preview_helper(&settings().helper, source.clone(), &memory).await,
            Err(PreviewError::ResourceLimit(_))
        ));
        let mut time = PreviewLimits::default();
        time.timeout = Duration::from_nanos(1);
        assert!(matches!(
            run_preview_helper(&settings().helper, source, &time).await,
            Err(PreviewError::ResourceLimit(_))
        ));
        f.close().await;
    }
}
