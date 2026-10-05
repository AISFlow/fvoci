//! Import runs (source `packages/core/src/import.ts`, `packages/jobs/src/import-job.ts`).
//!
//! markdown-zip runs inside the request. office-file and notion-zip rows are
//! durable in `fvoci.import_jobs`: one worker loop per process claims a row
//! with a lease (concurrency 1, as the source worker), recovers what a dead
//! previous run created before retrying (source `startImportRun`), and every
//! created document is recorded under the lease in its own transaction. The
//! daily maintenance sweep fails expired leases and compensates their rows
//! (source `sweepOrphanImports`). Shutdown stops between items, compensates
//! and marks the row failed, like the source abort path.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::db::backend::Backend;
use document_extract_client::limits::Limits;
use document_extract_client::outcome::ExtractStatus;
use document_extract_client::process::{extract_killable_with_cancel, ExtractRequest};
use sqlx::PgPool;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::attachments::{sniff_mime_from_bytes, ObjectStorage};
use crate::collab::seed::SeedEngine;
use crate::db::attachment_extract::default_extract_limits;
use crate::db::context::defer_import_events;
use crate::db::documents::{
    ImportDocumentOwner, ImportDocumentPublication, ImportFence, ImportNativeSeed,
    ImportPublicationError,
};
use crate::db::import_jobs::{
    claim_expired_import_job, fail_stale_sync_import_jobs, finish_sync_import_job,
    purge_imported_document, purge_imported_task, ImportClaim, ImportJobRefs, ImportSource,
    ImportStatus, IMPORT_MAX_ATTEMPTS, IMPORT_SWEEP_MAX,
};
use crate::db::quota::StorageQuota;
use crate::db::tasks::CreateTaskInput;
use crate::documents::import_body::{
    apply_imported_markdown, create_fenced_wiki_document, create_imported_wiki_document,
    ImportBodyError,
};
use crate::documents::import_zip::{title_from_file_name, unzip_bounded, ZipEntry};
use crate::documents::markdown_helper::MarkdownHelper;
use crate::documents::office::{
    run_office_helper, OfficeCancelled, OfficeKind, OfficeLimits, OfficeMode, OfficeOutcome,
};
use crate::export_zip::zip_safe_name;

const DEFAULT_POLL: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct ImportJobSettings {
    pub extractor_bin: Option<PathBuf>,
    pub extract_limits: Limits,
    /// This binary, run as the `--internal-office-extract` child.
    pub office_helper: Option<PathBuf>,
    /// This binary, run as the `--internal-markdown` child.
    pub markdown: Option<MarkdownHelper>,
    /// `collab-engine` child that turns imported Tiptap JSON into the Yjs seed.
    pub seed: Option<SeedEngine>,
    pub office_limits: OfficeLimits,
    /// Storage quota imported Notion assets reserve against (source
    /// `requireStorageReservation`; reads the live signed limit).
    pub quota: StorageQuota,
    pub poll_interval: Duration,
}

impl ImportJobSettings {
    pub fn from_env() -> Self {
        Self::from_env_with_license(Arc::new(crate::license::absent()))
    }

    pub fn from_env_with_license(license: Arc<crate::license::Entitlements>) -> Self {
        let extractor_bin = std::env::var("FVOCI_EXTRACTOR_BIN")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .map(PathBuf::from);
        let poll_interval = std::env::var("FVOCI_IMPORT_POLL_SECS")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|v| *v > 0)
            .map(Duration::from_secs)
            .unwrap_or(DEFAULT_POLL);
        Self {
            extractor_bin,
            extract_limits: default_extract_limits(),
            office_helper: std::env::current_exe().ok(),
            markdown: MarkdownHelper::current_exe().ok(),
            seed: SeedEngine::from_env(),
            office_limits: OfficeLimits::import(),
            quota: StorageQuota::from_license(license),
            poll_interval,
        }
    }
}

/// Office formats this server can turn into Markdown (source import accept
/// list: PDF, DOCX, PPTX, XLSX, ODT, ODP, ODS, HWP, HWPX; Markdown and text
/// are also taken as-is). Anything else is rejected before a job is created
/// instead of failing later in the worker.
pub fn office_format_supported(file_name: &str, extractor_available: bool) -> bool {
    let lower = file_name.to_lowercase();
    if lower.ends_with(".hwp") || lower.ends_with(".hwpx") {
        return extractor_available;
    }
    OfficeKind::from_name(&lower).is_some()
        || lower.ends_with(".md")
        || lower.ends_with(".markdown")
        || lower.ends_with(".txt")
}

pub struct ImportJobHandle {
    cancel: CancellationToken,
    join: JoinHandle<()>,
    pub wake: Arc<Notify>,
}

impl ImportJobHandle {
    pub fn request_shutdown(&self) {
        self.cancel.cancel();
    }

    pub async fn join(self) -> Result<(), String> {
        self.join
            .await
            .map_err(|err| format!("import job task join failed: {err}"))
    }
}

pub fn spawn_import_job(
    pool: PgPool,
    settings: ImportJobSettings,
    storage: ObjectStorage,
) -> ImportJobHandle {
    spawn_import_job_backend(Backend::Postgres(pool), settings, storage)
}

pub fn spawn_import_job_backend(
    backend: Backend,
    settings: ImportJobSettings,
    storage: ObjectStorage,
) -> ImportJobHandle {
    let cancel = CancellationToken::new();
    let wake = Arc::new(Notify::new());
    let join = tokio::spawn(import_loop(
        backend,
        settings,
        storage,
        cancel.child_token(),
        wake.clone(),
    ));
    ImportJobHandle { cancel, join, wake }
}

async fn import_loop(
    backend: Backend,
    settings: ImportJobSettings,
    storage: ObjectStorage,
    cancel: CancellationToken,
    wake: Arc<Notify>,
) {
    while !cancel.is_cancelled() {
        let worked = match run_next_import_backend(&backend, &settings, &storage, &cancel).await {
            Ok(worked) => worked,
            Err(err) if import_database_error_stops_scheduler(&backend, &err) => {
                warn!(error = %err, "import.database_outcome_unconfirmed_stopped");
                break;
            }
            Err(err) => {
                warn!(error = %err, "import.claim_failed");
                false
            }
        };
        if worked || cancel.is_cancelled() {
            continue;
        }
        tokio::select! {
            () = cancel.cancelled() => break,
            () = wake.notified() => {},
            () = tokio::time::sleep(settings.poll_interval) => {},
        }
    }
}

/// Claims and runs one async job. `Ok(false)` = nothing to claim.
pub async fn run_next_import(
    pool: &PgPool,
    settings: &ImportJobSettings,
    storage: &ObjectStorage,
    cancel: &CancellationToken,
) -> Result<bool, sqlx::Error> {
    run_next_import_backend(&Backend::Postgres(pool.clone()), settings, storage, cancel).await
}

pub async fn run_next_import_backend(
    backend: &Backend,
    settings: &ImportJobSettings,
    storage: &ObjectStorage,
    cancel: &CancellationToken,
) -> Result<bool, sqlx::Error> {
    if cancel.is_cancelled() {
        return Ok(false);
    }
    let Some(claim) = crate::db::import_jobs::claim_next_import_job_backend(backend).await? else {
        return Ok(false);
    };
    info!(workspace_id = %claim.workspace_id, import_job_id = %claim.job_id, attempt = claim.attempt, source = claim.source.as_str(), "import.claimed");
    run_claimed(backend, settings, storage, cancel, &claim).await?;
    Ok(true)
}

/// Source `importErrorHash`: logs carry a hash, never file contents.
fn error_hash(detail: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(detail.as_bytes()))
}

#[derive(Debug)]
enum RunError {
    /// The lease is no longer ours; the sweep owns the row and its refs.
    Fenced,
    /// Shutdown between items.
    Aborted,
    /// Capacity pressure (no collab seed slot / engine did not start):
    /// retried while attempts remain.
    Transient(String),
    /// A prior run's refs still name resources that could not be removed.
    CleanupIncomplete,
    Db(sqlx::Error),
    Denied,
    Failed(String),
}

impl From<ImportBodyError> for RunError {
    fn from(err: ImportBodyError) -> Self {
        match err {
            ImportBodyError::Fenced => RunError::Fenced,
            ImportBodyError::Unavailable => RunError::Transient(err.to_string()),
            other => RunError::Failed(other.to_string()),
        }
    }
}

fn db_failed(err: sqlx::Error) -> RunError {
    RunError::Db(err)
}

async fn run_claimed(
    pool: &Backend,
    settings: &ImportJobSettings,
    storage: &ObjectStorage,
    cancel: &CancellationToken,
    claim: &ImportClaim,
) -> Result<(), sqlx::Error> {
    if claim.source == ImportSource::NativeArchive {
        run_native_claimed(pool, settings, storage, cancel, claim).await?;
        return Ok(());
    }
    let mut created = ImportJobRefs::default();
    let result = run_claimed_inner(pool, settings, storage, cancel, claim, &mut created).await;
    match result {
        Ok(()) => match crate::db::import_jobs::finish_import_job_backend(
            pool,
            claim,
            ImportStatus::Completed,
        )
        .await
        {
            Ok(true) => info!(
                workspace_id = %claim.workspace_id,
                import_job_id = %claim.job_id,
                documents = created.document_ids.len(),
                tasks = created.task_ids.len(),
                objects = created.stored_keys.len(),
                "import.completed"
            ),
            Ok(false) => warn!(
                workspace_id = %claim.workspace_id,
                import_job_id = %claim.job_id,
                "import.completed_after_fence_lost"
            ),
            Err(err) => return Err(err),
        },
        Err(RunError::Db(error))
            if is_import_finish_unknown(&error) || matches!(pool, Backend::LibsqlRemote(_)) =>
        {
            return Err(error)
        }
        Err(RunError::Fenced) => warn!(
            workspace_id = %claim.workspace_id,
            import_job_id = %claim.job_id,
            "import.failed reason=fenced"
        ),
        Err(err) => {
            let undo = compensate_claimed_import_backend(pool, storage, claim, &created).await?;
            if undo.failed > 0 {
                error!(
                    workspace_id = %claim.workspace_id,
                    import_job_id = %claim.job_id,
                    failed = undo.failed,
                    "import.compensate_failed"
                );
            }
            if matches!(err, RunError::Transient(_) | RunError::CleanupIncomplete)
                && claim.attempt < IMPORT_MAX_ATTEMPTS
            {
                let clear_refs = undo.failed == 0 && !matches!(err, RunError::CleanupIncomplete);
                match crate::db::import_jobs::release_import_job_for_retry_backend(
                    pool, claim, clear_refs,
                )
                .await
                {
                    Ok(true) => warn!(
                        workspace_id = %claim.workspace_id,
                        import_job_id = %claim.job_id,
                        attempt = claim.attempt,
                        reason = match &err {
                            RunError::Transient(detail) => error_hash(detail),
                            RunError::CleanupIncomplete => "compensate_incomplete".to_string(),
                            _ => unreachable!(),
                        },
                        "import.retry_scheduled"
                    ),
                    Ok(false) => {
                        warn!(import_job_id = %claim.job_id, "import.retry_after_fence_lost")
                    }
                    Err(e) => return Err(e),
                }
                return Ok(());
            }
            let reason = match &err {
                RunError::Aborted => "shutdown".to_string(),
                RunError::Failed(detail) | RunError::Transient(detail) => error_hash(detail),
                RunError::CleanupIncomplete => "compensate_incomplete".to_string(),
                RunError::Fenced => unreachable!(),
                RunError::Db(error) => error_hash(&error.to_string()),
                RunError::Denied => error_hash("authorization revoked"),
            };
            match crate::db::import_jobs::finish_import_job_backend(
                pool,
                claim,
                ImportStatus::Failed,
            )
            .await
            {
                Ok(true) => {}
                Ok(false) => warn!(import_job_id = %claim.job_id, "import.fail_after_fence_lost"),
                Err(e) => return Err(e),
            }
            warn!(
                workspace_id = %claim.workspace_id,
                import_job_id = %claim.job_id,
                reason,
                "import.failed"
            );
        }
    }
    Ok(())
}

async fn run_claimed_inner(
    pool: &Backend,
    settings: &ImportJobSettings,
    storage: &ObjectStorage,
    cancel: &CancellationToken,
    claim: &ImportClaim,
    created: &mut ImportJobRefs,
) -> Result<(), RunError> {
    // Source `startImportRun`: undo what a dead previous run left, then clear refs.
    if !claim.prior_refs.is_empty() {
        let undo = compensate_claimed_import_backend(pool, storage, claim, &claim.prior_refs)
            .await
            .map_err(db_failed)?;
        warn!(
            workspace_id = %claim.workspace_id,
            import_job_id = %claim.job_id,
            documents = claim.prior_refs.document_ids.len(),
            tasks = claim.prior_refs.task_ids.len(),
            objects = claim.prior_refs.stored_keys.len(),
            skipped = undo.skipped,
            failed = undo.failed,
            "import.restarted"
        );
        if undo.failed > 0 {
            return Err(RunError::CleanupIncomplete);
        }
        if !crate::db::import_jobs::reset_import_refs_backend(pool, claim)
            .await
            .map_err(db_failed)?
        {
            return Err(RunError::Fenced);
        }
    }
    if cancel.is_cancelled() {
        return Err(RunError::Aborted);
    }
    let Some(payload) = crate::db::import_jobs::load_import_payload_backend(pool, claim)
        .await
        .map_err(db_failed)?
    else {
        return Err(RunError::Fenced);
    };
    match claim.source {
        ImportSource::NativeArchive => Err(RunError::Failed(
            "native archive requires atomic runner".into(),
        )),
        ImportSource::OfficeFile => {
            run_office_import(pool, settings, cancel, claim, payload, created).await
        }
        // Source `deferEvents`: events of every row this run creates are
        // parked until the `completed` transition publishes them.
        ImportSource::NotionZip => {
            defer_import_events(
                claim.job_id,
                run_notion_import(pool, settings, storage, cancel, claim, payload, created),
            )
            .await
        }
        ImportSource::MarkdownZip => Err(RunError::Failed("markdown-zip is synchronous".into())),
    }
}

/// Native publication has no partially created graph to compensate. Storage
/// keys remain in the durable cleanup journal until the single publication
/// transaction hands them off. In particular a lost commit response must
/// never purge keys from a successfully committed graph.
async fn run_native_claimed(
    pool: &Backend,
    settings: &ImportJobSettings,
    storage: &ObjectStorage,
    cancel: &CancellationToken,
    claim: &ImportClaim,
) -> Result<(), sqlx::Error> {
    use crate::db::native_archive::{self as db, NativeDbError};
    use crate::native_archive::{self as native, ArchiveError};
    let run = async {
        if cancel.is_cancelled() {
            return Err(NativeDbError::Archive(ArchiveError::Cancelled));
        }
        db::preflight_destination_backend(
            pool,
            claim.workspace_id,
            claim.created_by,
            claim.session_id,
        )
        .await?;
        let payload = crate::db::import_jobs::load_import_payload_backend(pool, claim)
            .await?
            .ok_or(NativeDbError::Fenced)?;
        let helper = settings
            .office_helper
            .as_ref()
            .ok_or(ArchiveError::Worker)?;
        // Bind publication to the same original bytes before parse owns them.
        let expected_archive_hash = native::digest(&payload);
        let archive = native::parse(helper, payload, cancel).await?;
        let cfg = crate::collab::CollabConfig::from_env().ok_or(ArchiveError::Worker)?;
        let archive = native::validate_native(archive, cfg, cancel).await?;
        if !crate::db::import_jobs::extend_import_lease_backend(pool, claim).await? {
            return Err(NativeDbError::Fenced);
        }
        let mut keys = std::collections::BTreeMap::new();
        for file in &archive.graph.attachments {
            if cancel.is_cancelled() {
                return Err(NativeDbError::Archive(ArchiveError::Cancelled));
            }
            // Every attempt has new keys; stale prior keys stay journaled. The
            // object stores accept only bare UUID keys, like ordinary uploads.
            let key = Uuid::now_v7().to_string();
            db::stage_key_backend(pool, claim, file.id, &key, cancel).await?;
            let bytes = archive.bytes(&file.payload_entry)?;
            let expected = native::digest(&bytes);
            storage
                .put_bytes(&key, bytes)
                .await
                .map_err(|_| ArchiveError::Invalid("storage write".into()))?;
            let readback = native::read_file(storage, &key, file.size_bytes).await?;
            if native::digest(&readback) != expected {
                return Err(NativeDbError::Archive(ArchiveError::Invalid(
                    "storage readback hash".into(),
                )));
            }
            keys.insert(file.id, key);
        }
        if cancel.is_cancelled() {
            return Err(NativeDbError::Archive(ArchiveError::Cancelled));
        }
        db::publish_backend(
            pool,
            claim,
            &archive,
            &keys,
            &settings.quota,
            &expected_archive_hash,
            cancel,
        )
        .await
    };
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(300), run).await;
    let diagnostic = match outcome {
        Ok(Ok(())) => {
            info!(import_job_id=%claim.job_id,"native_archive.completed");
            return Ok(());
        }
        Ok(Err(NativeDbError::Fenced)) => {
            warn!(import_job_id=%claim.job_id,"native_archive.fenced");
            return Ok(());
        }
        Ok(Err(NativeDbError::Forbidden)) => "authorization_revoked",
        Ok(Err(NativeDbError::Conflict)) => "conflict",
        Ok(Err(NativeDbError::Archive(ArchiveError::Unsupported(_)))) => {
            "unsupported_native_archive"
        }
        Ok(Err(NativeDbError::Archive(ArchiveError::Cancelled))) => "cancelled",
        Ok(Err(NativeDbError::Archive(ref error))) => {
            warn!(import_job_id=%claim.job_id, reason=native::log_reason(error), "native_archive.invalid");
            "invalid_or_incomplete_archive"
        }
        Ok(Err(NativeDbError::Sql(error)))
            if is_import_finish_unknown(&error) || matches!(pool, Backend::LibsqlRemote(_)) =>
        {
            return Err(error)
        }
        Ok(Err(NativeDbError::Sql(ref error)))
            if error
                .as_database_error()
                .is_some_and(|e| e.is_unique_violation()) =>
        {
            "conflict"
        }
        Ok(Err(NativeDbError::Sql(error))) => {
            error!(error=%error,import_job_id=%claim.job_id,"native_archive.db_failed");
            "database_failure"
        }
        Err(_) if matches!(pool, Backend::LibsqlRemote(_)) => {
            return Err(sqlx::Error::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "native import timeout; remote original-stream settlement unknown",
            )))
        }
        Err(_) => "native_archive_timeout",
    };
    let transition = db::fail_native_backend(pool, claim, diagnostic).await;
    // A test-local response boundary control exercises propagation and loop
    // stopping after a real known local transition. It is not a DB COMMIT fault
    // or proof of remote settlement; the actual producer fault oracle is pending.
    #[cfg(test)]
    let transition = if IMPORT_NATIVE_FAILED_RESPONSE_LOSS
        .try_with(|control| {
            if control.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) >= 1 {
                control.1.cancel();
            }
            control.2.notify_one();
        })
        .is_ok()
    {
        transition.and_then(|_| {
            Err(sqlx::Error::AnyDriverError(Box::new(
                crate::db::backend::CommitUnknown {
                    source: sqlx::Error::Protocol(
                        "native failed-transition response-loss control".into(),
                    ),
                },
            )))
        })
    } else {
        transition
    };
    match transition {
        Err(error) if import_database_error_stops_scheduler(pool, &error) => return Err(error),
        Ok(true) => warn!(import_job_id=%claim.job_id,diagnostic,"native_archive.failed"),
        Ok(false) => {
            warn!(import_job_id=%claim.job_id,"native_archive.failure_after_fence_or_commit")
        }
        Err(error) => {
            error!(error=%error,import_job_id=%claim.job_id,"native_archive.fail_transition")
        }
    }
    Ok(())
}

async fn run_office_import(
    pool: &Backend,
    settings: &ImportJobSettings,
    cancel: &CancellationToken,
    claim: &ImportClaim,
    payload: Vec<u8>,
    created: &mut ImportJobRefs,
) -> Result<(), RunError> {
    let name = claim
        .file_name
        .clone()
        .unwrap_or_else(|| "file".to_string());
    let markdown = office_file_to_markdown(settings, payload, &name, cancel).await?;
    if cancel.is_cancelled() {
        return Err(RunError::Aborted);
    }
    // A long parse must not let the sweep reclaim the row under us.
    if !crate::db::import_jobs::extend_import_lease_backend(pool, claim)
        .await
        .map_err(db_failed)?
    {
        return Err(RunError::Fenced);
    }
    let title = title_from_file_name(&name);
    publish_imported_markdown_backend(
        pool,
        settings.seed.as_ref(),
        settings.markdown.as_ref(),
        ImportMarkdownInput {
            owner: ImportDocumentOwner::Async(claim),
            title: &title,
            parent: None,
            markdown: &markdown,
            apply_body: !markdown.trim().is_empty(),
        },
        cancel,
        &mut created.document_ids,
    )
    .await?;
    Ok(())
}

async fn office_file_to_markdown(
    settings: &ImportJobSettings,
    bytes: Vec<u8>,
    file_name: &str,
    cancel: &CancellationToken,
) -> Result<String, RunError> {
    let lower = file_name.to_lowercase();
    if lower.ends_with(".hwp") || lower.ends_with(".hwpx") {
        let bin = settings
            .extractor_bin
            .clone()
            .ok_or_else(|| RunError::Failed("office import unavailable".into()))?;
        let request = ExtractRequest {
            bytes,
            name: file_name.to_string(),
            limits: settings.extract_limits,
            extractor_bin: bin,
            test_hang_ms: None,
        };
        // The extractor client blocks on its child; keep it off the runtime.
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let worker_flag = cancel_flag.clone();
        let mut handle = tokio::task::spawn_blocking(move || {
            extract_killable_with_cancel(request, worker_flag.as_ref())
        });
        let joined = tokio::select! {
            () = cancel.cancelled() => {
                cancel_flag.store(true, Ordering::Release);
                handle.await
            }
            joined = &mut handle => joined,
        };
        return match joined {
            Ok(Ok(report)) => match report.outcome {
                ExtractStatus::Ok { text, .. } | ExtractStatus::Partial { text, .. } => Ok(text),
                _ => Err(RunError::Failed("office extract failed".into())),
            },
            Ok(Err(_cancelled)) => Err(RunError::Aborted),
            Err(err) => Err(RunError::Failed(format!(
                "extract worker join failed: {err}"
            ))),
        };
    }
    if let Some(kind) = OfficeKind::from_name(&lower) {
        let helper = settings
            .office_helper
            .as_deref()
            .ok_or_else(|| RunError::Failed("office import unavailable".into()))?;
        // Source `officeFileToMarkdownIsolated`: anything but a parsed
        // document fails the job; the reason is logged only as a hash.
        return match run_office_helper(
            helper,
            bytes,
            kind,
            OfficeMode::Markdown,
            &settings.office_limits,
            cancel,
        )
        .await
        {
            Ok(OfficeOutcome::Ok { text, .. }) => Ok(text),
            Ok(OfficeOutcome::Empty) => Ok(String::new()),
            Ok(other) => Err(RunError::Failed(format!("office import {other:?}"))),
            Err(OfficeCancelled) => Err(RunError::Aborted),
        };
    }
    if lower.ends_with(".md") || lower.ends_with(".markdown") || lower.ends_with(".txt") {
        return Ok(String::from_utf8_lossy(&bytes).into_owned());
    }
    Err(RunError::Failed("office import unavailable".into()))
}

async fn unzip_off_runtime(bytes: Vec<u8>) -> Result<Vec<ZipEntry>, String> {
    tokio::task::spawn_blocking(move || unzip_bounded(&bytes))
        .await
        .map_err(|err| format!("unzip join failed: {err}"))?
        .map_err(|err| err.to_string())
}

async fn run_notion_import(
    pool: &Backend,
    settings: &ImportJobSettings,
    storage: &ObjectStorage,
    cancel: &CancellationToken,
    claim: &ImportClaim,
    payload: Vec<u8>,
    created: &mut ImportJobRefs,
) -> Result<(), RunError> {
    let entries = unzip_off_runtime(payload).await.map_err(RunError::Failed)?;
    let export = parse_notion_export(entries).map_err(RunError::Failed)?;
    let mut doc_by_path: HashMap<String, Uuid> = HashMap::new();
    for page in export.pages {
        if cancel.is_cancelled() {
            return Err(RunError::Aborted);
        }
        let parent_id = page
            .parent_path
            .as_ref()
            .and_then(|path| doc_by_path.get(path).copied());
        let document_id = publish_imported_markdown_backend(
            pool,
            settings.seed.as_ref(),
            settings.markdown.as_ref(),
            ImportMarkdownInput {
                owner: ImportDocumentOwner::Async(claim),
                title: &page.title,
                parent: parent_id,
                markdown: &page.markdown,
                apply_body: !page.markdown.is_empty(),
            },
            cancel,
            &mut created.document_ids,
        )
        .await?;
        doc_by_path.insert(page.path, document_id);
    }
    // Source: an asset belongs to the page whose folder holds it; a root
    // asset goes to the first created page; without pages it is dropped.
    for asset in export.assets {
        if cancel.is_cancelled() {
            return Err(RunError::Aborted);
        }
        let document_id = match &asset.parent_path {
            None => created.document_ids.first().copied(),
            Some(path) => doc_by_path.get(path).copied(),
        };
        let Some(document_id) = document_id else {
            continue;
        };
        store_imported_asset(
            pool,
            settings,
            storage,
            claim,
            document_id,
            asset,
            created,
            cancel,
        )
        .await?;
    }
    if let Some(project_id) = claim.project_id {
        import_notion_databases(pool, cancel, claim, project_id, export.databases, created).await?;
    }
    Ok(())
}

#[cfg(test)]
tokio::task_local! {
    // Test-only pause after the actual put ACK, before the actual finalizer.
    static IMPORT_ASSET_AFTER_PUT_CONTROL: Arc<(Notify, Notify)>;
}

/// Source `storeImportedAsset`: reserve (row + key ref, fenced), write the
/// object, then mark it stored with the sniffed MIME.
#[allow(clippy::too_many_arguments)]
async fn store_imported_asset(
    pool: &Backend,
    settings: &ImportJobSettings,
    storage: &ObjectStorage,
    claim: &ImportClaim,
    document_id: Uuid,
    asset: NotionAsset,
    created: &mut ImportJobRefs,
    cancel: &CancellationToken,
) -> Result<(), RunError> {
    // Attachment rows reserve at least one byte; an empty file has nothing
    // to store.
    if asset.data.is_empty() {
        return Ok(());
    }
    if cancel.is_cancelled() {
        return Err(RunError::Aborted);
    }
    let name: String = zip_safe_name(&asset.name).chars().take(255).collect();
    let size = asset.data.len() as i64;
    let reserved = crate::db::attachments::create_import_attachment_backend(
        pool,
        &settings.quota,
        claim,
        document_id,
        &name,
        size,
        cancel,
    )
    .await
    .map_err(db_failed)?;
    let (attachment_id, storage_key) = reserved.map_err(RunError::from)?;
    created.stored_keys.push(storage_key.clone());
    let mime = sniff_mime_from_bytes(&asset.data);
    storage
        .put_bytes(&storage_key, asset.data)
        .await
        .map_err(|err| RunError::Failed(format!("import attachment put: {err}")))?;
    #[cfg(test)]
    if let Ok(pause) = IMPORT_ASSET_AFTER_PUT_CONTROL.try_with(Arc::clone) {
        pause.0.notify_one();
        pause.1.notified().await;
    }
    if cancel.is_cancelled() {
        return Err(RunError::Aborted);
    }
    let stored = crate::db::attachments::mark_import_attachment_stored_backend(
        pool,
        storage,
        &settings.quota,
        claim,
        document_id,
        attachment_id,
        &storage_key,
        &name,
        &mime,
        size,
        cancel,
    )
    .await
    .map_err(db_failed)?;
    stored.map_err(RunError::from)?;
    Ok(())
}

/// Source `IMPORT_TASK_TITLE_MAX` (task title contract, 500).
const IMPORT_TASK_TITLE_MAX: usize = 500;

/// Source `IMPORT_HEADER_PATTERNS` (`@fvoci/i18n`), case-insensitive.
fn header_column(header: &[String], needles: &[&str]) -> Option<usize> {
    header.iter().position(|h| {
        let h = h.trim().to_lowercase();
        needles.iter().any(|n| h.contains(n))
    })
}

/// First `YYYY-MM-DD` in a cell (source `ISO_DATE`); an impossible calendar
/// date is dropped rather than failing the import.
fn first_iso_date(cell: &str) -> Option<chrono::NaiveDate> {
    let bytes = cell.as_bytes();
    (0..bytes.len().saturating_sub(9)).find_map(|at| {
        let window = cell.get(at..at + 10)?;
        let b = window.as_bytes();
        let shape = b[4] == b'-'
            && b[7] == b'-'
            && b.iter()
                .enumerate()
                .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit());
        if !shape {
            return None;
        }
        chrono::NaiveDate::parse_from_str(window, "%Y-%m-%d").ok()
    })
}

/// Source `matchMember`: email, `family+given` or `given family`.
fn match_member(members: &[crate::db::workspace::MemberRow], cell: &str) -> Option<Uuid> {
    if cell.is_empty() {
        return None;
    }
    let needle = cell.to_lowercase();
    members
        .iter()
        .find(|m| {
            let family = m.family_name.clone().unwrap_or_default();
            m.email.to_lowercase() == needle
                || format!("{family}{}", m.given_name).to_lowercase() == needle
                || format!("{} {family}", m.given_name).trim().to_lowercase() == needle
        })
        .map(|m| m.user_id)
}

/// Source `importNotionDatabases`: each CSV row (after the header) becomes a
/// task in `project_id`; the first column is the title, and status / assignee
/// / due columns are matched by header name.
async fn import_notion_databases(
    pool: &Backend,
    cancel: &CancellationToken,
    claim: &ImportClaim,
    project_id: Uuid,
    databases: Vec<NotionDatabase>,
    created: &mut ImportJobRefs,
) -> Result<(), RunError> {
    if databases.iter().all(|db| db.rows.len() < 2) {
        return Ok(());
    }
    let statuses = crate::db::tasks::project_status_names_backend(pool, claim, project_id)
        .await
        .map_err(db_failed)?
        .map_err(|e| RunError::Failed(format!("import statuses: {e:?}")))?;
    let members = match crate::db::workspace::list_members_backend(
        pool,
        claim.workspace_id,
        claim.created_by,
        claim.session_id,
    )
    .await
    .map_err(db_failed)?
    {
        Ok(members) => members,
        Err(err) => return Err(RunError::Failed(format!("import members: {err:?}"))),
    };
    for database in databases {
        let Some(header) = database.rows.first() else {
            continue;
        };
        let status_col = header_column(header, &["status", "상태"]);
        let assignee_col = header_column(header, &["assign", "owner", "담당"]);
        let due_col = header_column(header, &["due", "date", "기한", "마감"]);
        for row in database.rows.iter().skip(1) {
            if cancel.is_cancelled() {
                return Err(RunError::Aborted);
            }
            let title: String = row
                .first()
                .map(|c| c.trim())
                .unwrap_or("")
                .chars()
                .take(IMPORT_TASK_TITLE_MAX)
                .collect();
            let title = title.trim();
            if title.is_empty() {
                continue;
            }
            let cell =
                |col: Option<usize>| col.and_then(|c| row.get(c)).map(|c| c.trim()).unwrap_or("");
            let status_name = cell(status_col).to_lowercase();
            let status_id = statuses
                .iter()
                .find(|(_, name)| name.to_lowercase() == status_name)
                .map(|(id, _)| *id);
            let due_date = first_iso_date(cell(due_col));
            let assignee = match_member(&members, cell(assignee_col));
            let created_task = crate::db::tasks::create_import_task_backend(
                pool,
                crate::db::tasks::ImportTaskRequest {
                    claim,
                    project_id,
                    input: CreateTaskInput {
                        title,
                        task_type: "task",
                        priority: "none",
                        status_id,
                        start_date: None,
                        due_date,
                        parent_id: None,
                        milestone_id: None,
                        recurrence: None,
                    },
                    assignee,
                },
                cancel,
            )
            .await
            .map_err(db_failed)?;
            match created_task {
                Ok(Some(task_id)) => created.task_ids.push(task_id),
                Ok(None) => return Err(RunError::Fenced),
                Err(err) => return Err(RunError::Failed(format!("import task: {err:?}"))),
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotionPage {
    pub path: String,
    pub parent_path: Option<String>,
    pub title: String,
    pub markdown: String,
}

fn has_suffix_ci(name: &str, suffix: &str) -> bool {
    name.len() >= suffix.len()
        && name.is_char_boundary(name.len() - suffix.len())
        && name[name.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
}

fn strip_suffix_ci<'a>(name: &'a str, suffix: &str) -> &'a str {
    if has_suffix_ci(name, suffix) {
        &name[..name.len() - suffix.len()]
    } else {
        name
    }
}

fn parent_dir(path: &str) -> &str {
    path.rfind('/').map(|at| &path[..at]).unwrap_or("")
}

/// Source `notionTitle`: Notion appends ` <8..32 hex id>` to titles.
pub fn notion_title(base: &str) -> String {
    let hex = base
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_hexdigit())
        .count();
    let stripped = if (8..=32).contains(&hex) {
        let head = &base[..base.len() - hex];
        let separators = head
            .chars()
            .rev()
            .take_while(|c| c.is_whitespace() || *c == '_' || *c == '-')
            .map(char::len_utf8)
            .sum::<usize>();
        if separators > 0 {
            head[..head.len() - separators].trim()
        } else {
            base.trim()
        }
    } else {
        base.trim()
    };
    if stripped.is_empty() {
        title_from_file_name(base)
    } else {
        stripped.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotionDatabase {
    pub title: String,
    pub rows: Vec<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotionAsset {
    pub parent_path: Option<String>,
    pub name: String,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NotionExport {
    pub pages: Vec<NotionPage>,
    pub databases: Vec<NotionDatabase>,
    pub assets: Vec<NotionAsset>,
}

/// Source `IMPORT_CSV_MAX_ROWS`: rows parsed per CSV, header included.
pub const IMPORT_CSV_MAX_ROWS: usize = 10_000;

/// Source `parseNotionCsv`: RFC 4180 quoting, BOM stripped, at most
/// [`IMPORT_CSV_MAX_ROWS`] rows (the rest is not parsed), blank rows dropped.
pub fn parse_notion_csv(text: &str) -> Vec<Vec<String>> {
    let src = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    let mut chars = src.chars().peekable();
    let mut stopped = false;
    while let Some(ch) = chars.next() {
        if quoted {
            if ch != '"' {
                cell.push(ch);
            } else if chars.peek() == Some(&'"') {
                cell.push('"');
                chars.next();
            } else {
                quoted = false;
            }
            continue;
        }
        match ch {
            '"' => quoted = true,
            ',' => row.push(std::mem::take(&mut cell)),
            '\n' | '\r' => {
                if ch == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                row.push(std::mem::take(&mut cell));
                rows.push(std::mem::take(&mut row));
                if rows.len() >= IMPORT_CSV_MAX_ROWS {
                    stopped = true;
                    break;
                }
            }
            _ => cell.push(ch),
        }
    }
    if !stopped && (!cell.is_empty() || !row.is_empty()) && rows.len() < IMPORT_CSV_MAX_ROWS {
        row.push(cell);
        rows.push(row);
    }
    rows.retain(|r| r.iter().any(|c| !c.trim().is_empty()));
    rows
}

/// Source `parseNotionExport`: `.md` entries are pages (a page's parent is
/// the page whose folder, the same path without `.md`, contains it; parents
/// sort before children), `.csv` entries are databases, everything else is
/// an asset of the page whose folder holds it. HTML exports are refused.
pub fn parse_notion_export(entries: Vec<ZipEntry>) -> Result<NotionExport, String> {
    if entries
        .iter()
        .any(|e| has_suffix_ci(&e.name, ".html") || has_suffix_ci(&e.name, ".htm"))
    {
        return Err("notion html export unsupported".into());
    }
    let folder_to_page: HashMap<String, String> = entries
        .iter()
        .filter(|e| has_suffix_ci(&e.name, ".md"))
        .map(|e| (strip_suffix_ci(&e.name, ".md").to_string(), e.name.clone()))
        .collect();
    let owner_of = |path: &str| -> Option<String> {
        let mut dir = parent_dir(path);
        while !dir.is_empty() {
            if let Some(owner) = folder_to_page.get(dir) {
                return Some(owner.clone());
            }
            dir = parent_dir(dir);
        }
        None
    };
    let mut export = NotionExport::default();
    for entry in entries {
        let base = entry
            .name
            .rsplit('/')
            .next()
            .unwrap_or(&entry.name)
            .to_string();
        if has_suffix_ci(&entry.name, ".md") {
            export.pages.push(NotionPage {
                parent_path: owner_of(&entry.name),
                title: notion_title(strip_suffix_ci(&base, ".md")),
                markdown: String::from_utf8_lossy(&entry.data).into_owned(),
                path: entry.name,
            });
        } else if has_suffix_ci(&entry.name, ".csv") {
            export.databases.push(NotionDatabase {
                title: notion_title(strip_suffix_ci(&base, ".csv")),
                rows: parse_notion_csv(&String::from_utf8_lossy(&entry.data)),
            });
        } else {
            export.assets.push(NotionAsset {
                parent_path: owner_of(&entry.name),
                name: base,
                data: entry.data,
            });
        }
    }
    export
        .pages
        .sort_by_key(|page| page.path.split('/').count());
    Ok(export)
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CompensateOutcome {
    pub failed: u32,
    pub skipped: u32,
}

/// Source `compensateImport`: tasks, then documents newest first (children
/// reference parents), then stored objects when every row purge succeeded.
/// A row that is already gone counts as skipped, not failed.
pub async fn compensate_import(
    pool: &PgPool,
    storage: &ObjectStorage,
    workspace_id: Uuid,
    refs: &ImportJobRefs,
) -> CompensateOutcome {
    let mut out = CompensateOutcome::default();
    let mut keys = refs.stored_keys.clone();
    for task_id in refs.task_ids.iter().rev() {
        match purge_imported_task(pool, workspace_id, *task_id).await {
            Ok(Some(attachment_keys)) => keys.extend(attachment_keys),
            Ok(None) => out.skipped += 1,
            Err(err) => {
                out.failed += 1;
                warn!(
                    workspace_id = %workspace_id,
                    task_id = %task_id,
                    error = %err,
                    "import.compensate_task_failed"
                );
            }
        }
    }
    for document_id in refs.document_ids.iter().rev() {
        match purge_imported_document(pool, workspace_id, *document_id).await {
            Ok(Some(attachment_keys)) => keys.extend(attachment_keys),
            Ok(None) => out.skipped += 1,
            Err(err) => {
                out.failed += 1;
                warn!(
                    workspace_id = %workspace_id,
                    document_id = %document_id,
                    error = %err,
                    "import.compensate_document_failed"
                );
            }
        }
    }
    if out.failed == 0 {
        for key in keys {
            // Also aborts an open multipart upload, which could otherwise
            // publish an object after its row is gone.
            if let Err(err) = storage.purge_key(&key).await {
                out.failed += 1;
                warn!(error = %err, "import.compensate_object_failed");
            }
        }
    }
    out
}

/// Source `sweepOrphanImports`, run by the daily maintenance sweep, plus
/// the markdown-zip rows a cancelled request or a crash left `pending`
/// (see [`fail_stale_sync_import_jobs`]). Returns the rows it failed. The
/// two parts are independent: a failed stale-row pass is logged and retried
/// by the next sweep, and never skips the lease-expired compensation.
pub async fn sweep_orphan_imports(
    pool: &PgPool,
    storage: &ObjectStorage,
    cancel: &CancellationToken,
) -> Result<u32, sqlx::Error> {
    let mut swept = 0;
    if !cancel.is_cancelled() {
        match fail_stale_sync_import_jobs(pool).await {
            Ok(stale) => {
                if stale > 0 {
                    warn!(jobs = stale, "import.sync_stale_failed");
                }
                swept += u32::try_from(stale).unwrap_or(u32::MAX);
            }
            Err(err) => warn!(error = %err, "import.sync_stale_sweep_failed"),
        }
    }
    for _ in 0..IMPORT_SWEEP_MAX {
        if cancel.is_cancelled() {
            break;
        }
        let Some(job) = claim_expired_import_job(pool).await? else {
            break;
        };
        swept += 1;
        let undo = compensate_import(pool, storage, job.workspace_id, &job.created_refs).await;
        warn!(
            workspace_id = %job.workspace_id,
            import_job_id = %job.job_id,
            documents = job.created_refs.document_ids.len(),
            skipped = undo.skipped,
            "import.swept"
        );
        if undo.failed > 0 {
            error!(
                workspace_id = %job.workspace_id,
                import_job_id = %job.job_id,
                failed = undo.failed,
                "import.compensate_failed"
            );
        }
    }
    Ok(swept)
}

#[derive(Debug)]
pub enum SyncImportError {
    /// Membership or session gone: masked as 404 like the rest of the API.
    NotFound,
    /// Source `ImportFailedError` (400 `import_failed`).
    Failed(String),
    Db(sqlx::Error),
}

/// Source `importMarkdownZip` after the job row exists: every `.md` entry
/// becomes a root document. When the run returns an error the row is marked
/// failed; documents already created stay (the source does not compensate
/// this path). The run is the request future: if it is dropped (client
/// disconnect, shutdown deadline) or the process dies, the row stays
/// `pending` until the daily sweep fails it after
/// [`SYNC_IMPORT_STALE_SECS`](crate::db::import_jobs::SYNC_IMPORT_STALE_SECS).
#[allow(clippy::too_many_arguments)]
pub async fn run_markdown_zip_import(
    pool: &PgPool,
    seed: &SeedEngine,
    markdown_helper: &MarkdownHelper,
    workspace_id: Uuid,
    job_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    zip_bytes: Vec<u8>,
) -> Result<Vec<Uuid>, SyncImportError> {
    let result = markdown_zip_documents(
        pool,
        seed,
        markdown_helper,
        workspace_id,
        actor_user_id,
        session_id,
        zip_bytes,
    )
    .await;
    let status = if result.is_ok() {
        ImportStatus::Completed
    } else {
        ImportStatus::Failed
    };
    let moved = finish_sync_import_job(pool, workspace_id, job_id, status)
        .await
        .map_err(SyncImportError::Db)?;
    if !moved {
        return Err(SyncImportError::Db(sqlx::Error::Protocol(format!(
            "import job {job_id} was not pending when finishing"
        ))));
    }
    result
}

async fn markdown_zip_documents(
    pool: &PgPool,
    seed: &SeedEngine,
    markdown_helper: &MarkdownHelper,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    zip_bytes: Vec<u8>,
) -> Result<Vec<Uuid>, SyncImportError> {
    if zip_bytes.is_empty() {
        return Err(SyncImportError::Failed("zip required".into()));
    }
    let entries = unzip_off_runtime(zip_bytes)
        .await
        .map_err(SyncImportError::Failed)?;
    let mut created = Vec::new();
    for entry in entries {
        if !has_suffix_ci(&entry.name, ".md") {
            continue;
        }
        let markdown = String::from_utf8_lossy(&entry.data).into_owned();
        let map = |err: ImportBodyError| match err {
            ImportBodyError::NotFound | ImportBodyError::Forbidden => SyncImportError::NotFound,
            other => SyncImportError::Failed(other.to_string()),
        };
        let document_id = create_imported_wiki_document(
            pool,
            workspace_id,
            actor_user_id,
            session_id,
            &title_from_file_name(&entry.name),
            None,
        )
        .await
        .map_err(map)?;
        apply_imported_markdown(
            pool,
            seed,
            markdown_helper,
            workspace_id,
            actor_user_id,
            session_id,
            document_id,
            &markdown,
        )
        .await
        .map_err(map)?;
        created.push(document_id);
    }
    Ok(created)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, data: &str) -> ZipEntry {
        ZipEntry {
            name: name.to_string(),
            data: data.as_bytes().to_vec(),
        }
    }

    #[test]
    fn notion_titles_drop_page_ids() {
        assert_eq!(
            notion_title("Roadmap 0123456789abcdef0123456789abcdef"),
            "Roadmap"
        );
        assert_eq!(notion_title("회의록_89ab01cd"), "회의록");
        assert_eq!(notion_title("Plain title"), "Plain title");
        // Too long to be an id suffix: kept as-is.
        assert_eq!(
            notion_title("x 0123456789abcdef0123456789abcdef01"),
            "x 0123456789abcdef0123456789abcdef01"
        );
        assert_eq!(notion_title("0123456789abcdef"), "0123456789abcdef");
    }

    #[test]
    fn notion_hierarchy_comes_from_folders() {
        let pages = parse_notion_export(vec![
            entry(
                "Root 0123456789abcdef/Child aaaaaaaa/Grand bbbbbbbb.md",
                "# g",
            ),
            entry("Root 0123456789abcdef.md", "# r"),
            entry("Root 0123456789abcdef/Child aaaaaaaa.md", "# c"),
            entry("Root 0123456789abcdef/image.png", "png"),
            entry("Other.md", "# o"),
        ])
        .unwrap()
        .pages;
        let by_title: HashMap<_, _> = pages.iter().map(|p| (p.title.as_str(), p)).collect();
        assert_eq!(by_title["Root"].parent_path, None);
        assert_eq!(by_title["Other"].parent_path, None);
        assert_eq!(
            by_title["Child"].parent_path.as_deref(),
            Some("Root 0123456789abcdef.md")
        );
        assert_eq!(
            by_title["Grand"].parent_path.as_deref(),
            Some("Root 0123456789abcdef/Child aaaaaaaa.md")
        );
        let order: Vec<_> = pages.iter().map(|p| p.title.as_str()).collect();
        assert!(order.iter().position(|t| *t == "Root") < order.iter().position(|t| *t == "Child"));
        assert!(
            order.iter().position(|t| *t == "Child") < order.iter().position(|t| *t == "Grand")
        );
    }

    #[test]
    fn notion_html_export_is_rejected() {
        assert!(parse_notion_export(vec![entry("Page.html", "<p>")]).is_err());
    }

    #[test]
    fn office_formats_are_explicit() {
        assert!(office_format_supported("a.MD", false));
        assert!(office_format_supported("a.txt", false));
        assert!(!office_format_supported("a.hwp", false));
        assert!(office_format_supported("a.hwpx", true));
        for ext in ["pdf", "docx", "pptx", "xlsx", "odt", "odp", "ods", "PDF"] {
            assert!(
                office_format_supported(&format!("보고서.{ext}"), false),
                "{ext}"
            );
        }
        assert!(!office_format_supported("a.doc", true));
        assert!(!office_format_supported("a.epub", true));
        assert!(!office_format_supported("docx", true));
    }

    #[test]
    fn notion_csv_follows_source_quoting_and_limits() {
        let rows = parse_notion_csv(
            "\u{feff}Name,Status,Due\r\n\"A, \"\"quoted\"\"\",Done,2026-01-02\n\n,,\nB\nlast,x",
        );
        assert_eq!(
            rows,
            vec![
                vec!["Name", "Status", "Due"],
                vec!["A, \"quoted\"", "Done", "2026-01-02"],
                vec!["B"],
                vec!["last", "x"],
            ]
            .into_iter()
            .map(|r| r.into_iter().map(String::from).collect::<Vec<_>>())
            .collect::<Vec<_>>()
        );
        let many = "x\n".repeat(IMPORT_CSV_MAX_ROWS + 50);
        assert_eq!(parse_notion_csv(&many).len(), IMPORT_CSV_MAX_ROWS);
        // A quoted newline stays inside the cell.
        assert_eq!(parse_notion_csv("\"a\nb\",c")[0][0], "a\nb");
    }

    #[test]
    fn notion_export_splits_pages_databases_and_assets() {
        let export = parse_notion_export(vec![
            entry("Root 0123456789abcdef.md", "# r"),
            entry("Root 0123456789abcdef/image.png", "png"),
            entry("Root 0123456789abcdef/Tasks 89abcdef01.csv", "Name\nOne"),
            entry("loose.pdf", "%PDF"),
        ])
        .unwrap();
        assert_eq!(export.pages.len(), 1);
        assert_eq!(export.databases[0].title, "Tasks");
        assert_eq!(export.databases[0].rows.len(), 2);
        let by_name: HashMap<_, _> = export.assets.iter().map(|a| (a.name.as_str(), a)).collect();
        assert_eq!(
            by_name["image.png"].parent_path.as_deref(),
            Some("Root 0123456789abcdef.md")
        );
        assert_eq!(by_name["loose.pdf"].parent_path, None);
    }

    #[test]
    fn header_matching_dates_and_members() {
        let header: Vec<String> = ["이름", "진행 상태", "담당자", "마감일"]
            .into_iter()
            .map(String::from)
            .collect();
        assert_eq!(header_column(&header, &["status", "상태"]), Some(1));
        assert_eq!(
            header_column(&header, &["assign", "owner", "담당"]),
            Some(2)
        );
        assert_eq!(
            header_column(&header, &["due", "date", "기한", "마감"]),
            Some(3)
        );
        assert_eq!(
            first_iso_date("March 2026-03-05 (Thu)"),
            chrono::NaiveDate::from_ymd_opt(2026, 3, 5)
        );
        assert_eq!(first_iso_date("2026-13-45"), None);
        assert_eq!(first_iso_date("none"), None);
        let id = Uuid::now_v7();
        let members = vec![crate::db::workspace::MemberRow {
            user_id: id,
            email: "Kim@Example.com".into(),
            given_name: "민수".into(),
            family_name: Some("김".into()),
            role: crate::db::workspace::WorkspaceRole::Member,
        }];
        assert_eq!(match_member(&members, "kim@example.com"), Some(id));
        assert_eq!(match_member(&members, "김민수"), Some(id));
        assert_eq!(match_member(&members, "민수 김"), Some(id));
        assert_eq!(match_member(&members, "someone"), None);
        assert_eq!(match_member(&members, ""), None);
    }
}

impl From<crate::db::attachments::ImportAttachmentError> for RunError {
    fn from(error: crate::db::attachments::ImportAttachmentError) -> Self {
        use crate::db::attachments::ImportAttachmentError;
        match error {
            ImportAttachmentError::Fenced => Self::Fenced,
            ImportAttachmentError::Cancelled => Self::Aborted,
            ImportAttachmentError::Attachment(error) => {
                Self::Failed(format!("import attachment: {error:?}"))
            }
        }
    }
}
impl From<ImportPublicationError> for RunError {
    fn from(error: ImportPublicationError) -> Self {
        use crate::db::collab::CollabDbError;
        use crate::db::documents::DocumentDbError;
        match error {
            ImportPublicationError::Fenced
            | ImportPublicationError::Native(CollabDbError::StaleWriter) => Self::Fenced,
            ImportPublicationError::Cancelled => Self::Aborted,
            ImportPublicationError::Document(
                DocumentDbError::NotFound | DocumentDbError::Forbidden,
            )
            | ImportPublicationError::Native(CollabDbError::NotFound | CollabDbError::Forbidden) => {
                Self::Denied
            }
            ImportPublicationError::Sql(error) => Self::Db(error),
            other => Self::Failed(other.to_string()),
        }
    }
}
struct ImportMarkdownInput<'a> {
    owner: ImportDocumentOwner<'a>,
    title: &'a str,
    parent: Option<Uuid>,
    markdown: &'a str,
    apply_body: bool,
}
async fn publish_imported_markdown_backend(
    backend: &Backend,
    seed: Option<&SeedEngine>,
    helper: Option<&MarkdownHelper>,
    input: ImportMarkdownInput<'_>,
    cancel: &CancellationToken,
    progress: &mut Vec<Uuid>,
) -> Result<Uuid, RunError> {
    if cancel.is_cancelled() {
        return Err(RunError::Aborted);
    }
    let workspace = input.owner.workspace();
    let actor = input.owner.actor();
    let credential = input.owner.credential();
    if let Backend::Postgres(pool) = backend {
        let document = match input.owner {
            ImportDocumentOwner::Async(claim) => {
                let fence = ImportFence {
                    job_id: claim.job_id,
                    lease_token: claim.lease_token,
                };
                create_fenced_wiki_document(
                    pool,
                    workspace,
                    actor,
                    credential,
                    input.title,
                    input.parent,
                    fence,
                )
                .await?
            }
            ImportDocumentOwner::Sync { .. } => {
                create_imported_wiki_document(
                    pool,
                    workspace,
                    actor,
                    credential,
                    input.title,
                    input.parent,
                )
                .await?
            }
        };
        progress.push(document);
        if input.apply_body {
            let seed = seed.ok_or_else(|| RunError::Failed("collab engine unavailable".into()))?;
            let helper =
                helper.ok_or_else(|| RunError::Failed("markdown helper unavailable".into()))?;
            apply_imported_markdown(
                pool,
                seed,
                helper,
                workspace,
                actor,
                credential,
                document,
                input.markdown,
            )
            .await?;
        }
        return Ok(document);
    }
    let seed = seed.ok_or_else(|| RunError::Failed("collab engine unavailable".into()))?;
    let helper = helper.ok_or_else(|| RunError::Failed("markdown helper unavailable".into()))?;
    if input.markdown.len() > crate::collab::derived_body::DOCUMENT_MAX_BODY_BYTES {
        return Err(RunError::Failed("document too large".into()));
    }
    let content = helper
        .md_to_tiptap(input.markdown)
        .await
        .map_err(|e| RunError::Failed(e.to_string()))?;
    if cancel.is_cancelled() {
        return Err(RunError::Aborted);
    }
    let prepared = crate::collab::derived_body::prepare_derived_body(content.clone())
        .map_err(|e| RunError::Failed(format!("import body: {e:?}")))?;
    let update = seed
        .tiptap_to_yjs_update(&content)
        .await
        .map_err(|e| match e {
            crate::collab::seed::SeedError::Unavailable => RunError::Transient(e.to_string()),
            other => RunError::Failed(other.to_string()),
        })?;
    if cancel.is_cancelled() {
        return Err(RunError::Aborted);
    }
    // Select one operation ID before the canonical publisher's actual writer;
    // finish uncertainty returns without generating another ID or retrying.
    let op_id = Uuid::now_v7();
    let document = crate::db::documents::publish_import_document_backend(
        backend,
        ImportDocumentPublication {
            owner: input.owner,
            title: input.title,
            parent_id: input.parent,
            native: Some(ImportNativeSeed {
                op_id,
                update: &update,
                prepared,
            }),
        },
        cancel,
    )
    .await
    .map_err(RunError::from)?;
    progress.push(document);
    Ok(document)
}

fn is_import_finish_unknown(error: &sqlx::Error) -> bool {
    crate::db::backend::is_rollback_cleanup_unknown(error)
        || matches!(error,sqlx::Error::AnyDriverError(source) if source.is::<crate::db::backend::CommitUnknown>() || source.is::<crate::db::backend::CommitCleanupUnknown>())
}

/// Daily scheduler recognition: preserve/downcast the original typed error;
/// remote failures conservatively stop before any other stream/write/purge.
/// Recognition does not establish settlement or authorize reconciliation.
pub fn import_database_error_stops_scheduler(backend: &Backend, error: &sqlx::Error) -> bool {
    matches!(backend, Backend::LibsqlRemote(_)) || is_import_finish_unknown(error)
}

async fn compensate_claimed_import_backend(
    backend: &Backend,
    storage: &ObjectStorage,
    claim: &ImportClaim,
    refs: &ImportJobRefs,
) -> Result<CompensateOutcome, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return Ok(compensate_import(pool, storage, claim.workspace_id, refs).await);
    }
    // Shutdown must stop publication, while owned cleanup is explicitly
    // awaited with its own non-cancelled token as in the original abort path.
    match crate::db::import_jobs::compensate_family_import(
        backend,
        storage,
        crate::db::import_jobs::ImportCleanupOwner::Runner(claim),
        refs,
        &CancellationToken::new(),
    )
    .await
    {
        Ok(outcome) => Ok(outcome),
        Err(error)
            if is_import_finish_unknown(&error) || matches!(backend, Backend::LibsqlRemote(_)) =>
        {
            Err(error)
        }
        Err(error) => {
            warn!(error=%error,import_job_id=%claim.job_id,"import.compensate_failed");
            Ok(CompensateOutcome {
                failed: 1,
                skipped: 0,
            })
        }
    }
}

/// PG compatibility entry. Selected-family scheduling must supply its actual
/// Daily proof to the claimed entry below; metadata preflight is insufficient.
pub async fn sweep_orphan_imports_backend(
    backend: &Backend,
    storage: &ObjectStorage,
    cancel: &CancellationToken,
) -> Result<u32, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return sweep_orphan_imports(pool, storage, cancel).await;
    }
    Err(sqlx::Error::Protocol(
        "selected import sweep requires current Daily maintenance proof".into(),
    ))
}

/// Actual selected Daily consumer. Candidate reads do not authorize effects:
/// each prep and cleanup unit checks/renews the proof in the mutation writer.
pub async fn sweep_orphan_imports_with_maintenance_claim_backend(
    backend: &Backend,
    storage: &ObjectStorage,
    cancel: &CancellationToken,
    proof: &crate::db::maintenance_claim::FamilyMaintenanceProof,
    policy: crate::db::maintenance_claim::FamilyMaintenanceLeasePolicy,
) -> Result<u32, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return sweep_orphan_imports(pool, storage, cancel).await;
    }
    if cancel.is_cancelled() {
        return Ok(0);
    }
    let context = crate::db::import_jobs::ImportMaintenanceContext { proof, policy };
    let mut swept = 0;
    match crate::db::import_jobs::fail_stale_sync_imports_with_maintenance_backend(
        backend, context, cancel,
    )
    .await
    {
        Ok(stale) => swept += u32::try_from(stale).unwrap_or(u32::MAX),
        Err(error)
            if is_import_finish_unknown(&error) || matches!(backend, Backend::LibsqlRemote(_)) =>
        {
            return Err(error)
        }
        Err(error) => warn!(error=%error,"import.sync_stale_sweep_failed"),
    }
    if cancel.is_cancelled() {
        return Ok(swept);
    }
    let candidates =
        crate::db::import_jobs::import_cleanup_candidates_backend(backend, context, cancel).await?;
    for job in candidates {
        if cancel.is_cancelled() {
            break;
        }
        // FAILED prep is durable before external I/O. Commit uncertainty returns
        // immediately; neither cleanup nor a fresh observer follows it.
        if !crate::db::import_jobs::prepare_import_cleanup_backend(backend, &job, context, cancel)
            .await?
        {
            continue;
        }
        swept += 1;
        match crate::db::import_jobs::cleanup_failed_import_backend(
            backend, storage, &job, context, cancel,
        )
        .await
        {
            Ok(undo) => {
                warn!(workspace_id=%job.workspace_id,import_job_id=%job.job_id,skipped=undo.skipped,"import.swept")
            }
            Err(error)
                if is_import_finish_unknown(&error)
                    || matches!(backend, Backend::LibsqlRemote(_)) =>
            {
                return Err(error)
            }
            Err(error) => error!(error=%error,import_job_id=%job.job_id,"import.compensate_failed"),
        }
    }
    Ok(swept)
}

#[allow(clippy::too_many_arguments)] // Retains the existing request-driven public input contract.
pub async fn run_markdown_zip_import_backend(
    backend: &Backend,
    seed: &SeedEngine,
    helper: &MarkdownHelper,
    workspace: Uuid,
    job: Uuid,
    actor: Uuid,
    credential: Uuid,
    zip_bytes: Vec<u8>,
) -> Result<Vec<Uuid>, SyncImportError> {
    if let Backend::Postgres(pool) = backend {
        return run_markdown_zip_import(
            pool, seed, helper, workspace, job, actor, credential, zip_bytes,
        )
        .await;
    }
    let cancel = CancellationToken::new();
    let result = async {
        if zip_bytes.is_empty() {
            return Err(SyncImportError::Failed("zip required".into()));
        }
        let entries = unzip_off_runtime(zip_bytes)
            .await
            .map_err(SyncImportError::Failed)?;
        let mut created = Vec::new();
        for entry in entries {
            if !has_suffix_ci(&entry.name, ".md") {
                continue;
            }
            let markdown = String::from_utf8_lossy(&entry.data).into_owned();
            let title = title_from_file_name(&entry.name);
            publish_imported_markdown_backend(
                backend,
                Some(seed),
                Some(helper),
                ImportMarkdownInput {
                    owner: ImportDocumentOwner::Sync {
                        workspace,
                        job,
                        actor,
                        credential,
                    },
                    title: &title,
                    parent: None,
                    markdown: &markdown,
                    apply_body: true,
                },
                &cancel,
                &mut created,
            )
            .await
            .map_err(|e| match e {
                RunError::Db(error) => SyncImportError::Db(error),
                RunError::Denied => SyncImportError::NotFound,
                other => SyncImportError::Failed(format!("{other:?}")),
            })?;
        }
        Ok(created)
    }
    .await;
    if matches!(&result,Err(SyncImportError::Db(error)) if is_import_finish_unknown(error) || matches!(backend, Backend::LibsqlRemote(_)))
    {
        return result;
    }
    let status = if result.is_ok() {
        ImportStatus::Completed
    } else {
        ImportStatus::Failed
    };
    if !crate::db::import_jobs::finish_sync_import_job_backend(
        backend, workspace, job, actor, credential, status,
    )
    .await
    .map_err(SyncImportError::Db)?
    {
        return Err(SyncImportError::NotFound);
    }
    result
}

#[cfg(test)]
tokio::task_local! {
    static IMPORT_NATIVE_FAILED_RESPONSE_LOSS: Arc<(
        std::sync::atomic::AtomicUsize, CancellationToken, Arc<Notify>
    )>;
}

#[cfg(test)]
mod native_failure_propagation_tests {
    use super::*;
    use crate::db::attachment_preview::tests::Fixture;

    async fn native_job(f: &Fixture, session: Uuid) -> Uuid {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO import_jobs(id,workspace_id,created_by,session_id,source,status,payload,native_request_id,native_archive_hash) VALUES(?1,?2,?3,?4,'native-archive','running',?5,?6,?7)")
            .bind(id.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).bind(session.as_bytes().as_slice()).bind(b"literal response-boundary control archive".as_slice()).bind(Uuid::now_v7().as_bytes().as_slice()).bind("0".repeat(64)).execute(&f.pool).await.unwrap();
        id
    }

    #[tokio::test]
    async fn import_selected_actual_asset_reserve_finalize_rollback_control_and_healthy_retry() {
        use crate::db::attachments::{ImportAttachmentError, ImportAttachmentRollbackReason};
        fn stopped<'a>(
            backend: &Backend,
            error: &'a RunError,
        ) -> &'a ImportAttachmentRollbackReason {
            let RunError::Db(error) = error else {
                panic!("actual asset caller must preserve SQLx stop mapping");
            };
            assert!(import_database_error_stops_scheduler(backend, error));
            let sqlx::Error::AnyDriverError(source) = error else {
                panic!("shared typed rollback error required");
            };
            let receipt = source
                .downcast_ref::<crate::db::backend::RollbackCleanupUnknown>()
                .unwrap();
            assert!(
                matches!(&receipt.cleanup, sqlx::Error::Protocol(message) if message == "synthetic import cleanup fault after actual awaited rollback; propagation only")
            );
            receipt
                .original
                .as_ref()
                .unwrap()
                .downcast_ref::<ImportAttachmentRollbackReason>()
                .unwrap()
        }
        let f = Fixture::new().await;
        let credential = Uuid::now_v7();
        let token = crate::auth::token::new_token();
        let mut tx = f.backend.begin_write().await.unwrap();
        tx.operation()
            .create_session(
                credential,
                f.user,
                &token.hash,
                chrono::DateTime::from_timestamp_micros(
                    chrono::Utc::now().timestamp_micros() + 86_400_000_000,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let mut jobs = Vec::new();
        for name in ["asset.zip", "next.zip"] {
            jobs.push(
                crate::db::import_jobs::create_async_import_job_backend(
                    &f.backend,
                    f.workspace,
                    f.user,
                    credential,
                    ImportSource::NotionZip,
                    crate::db::import_jobs::NewAsyncImport {
                        file_name: Some(name),
                        project_id: None,
                        payload: b"retained queued source",
                    },
                )
                .await
                .unwrap()
                .unwrap(),
            );
        }
        sqlx::query("UPDATE import_jobs SET created_at=1 WHERE id=?1")
            .bind(jobs[0].id.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let claim = crate::db::import_jobs::claim_next_import_job_backend(&f.backend)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(claim.job_id, jobs[0].id);
        let mut tx = f.backend.begin_write().await.unwrap();
        tx.operation().set_tenant(f.workspace).await.unwrap();
        assert!(tx
            .operation()
            .append_import_ref(
                f.workspace,
                ImportFence {
                    job_id: claim.job_id,
                    lease_token: claim.lease_token
                },
                crate::db::import_jobs::ImportRefKind::Document,
                &f.document.to_string()
            )
            .await
            .unwrap());
        tx.commit().await.unwrap();
        let root = f.root.join("actual-asset-caller");
        let storage = ObjectStorage::local(root.clone());
        let settings = ImportJobSettings::from_env();
        let cancel = CancellationToken::new();
        let literal = b"literal actual asset caller bytes";
        let asset = |name: &str| NotionAsset {
            parent_path: None,
            name: name.into(),
            data: literal.to_vec(),
        };
        sqlx::query("UPDATE sessions SET revoked_at=1 WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        crate::db::attachments::import_rollback_test_hooks::arm((f.document, true));
        let mut reserve_progress = ImportJobRefs::default();
        let error = store_imported_asset(
            &f.backend,
            &settings,
            &storage,
            &claim,
            f.document,
            asset("reserve.txt"),
            &mut reserve_progress,
            &cancel,
        )
        .await
        .unwrap_err();
        assert!(matches!(
            stopped(&f.backend, &error),
            ImportAttachmentRollbackReason::Domain(ImportAttachmentError::Attachment(
                crate::db::attachments::AttachmentDbError::Forbidden
            ))
        ));
        assert!(reserve_progress.is_empty());
        assert!(
            !root.exists(),
            "typed reserve stop must precede physical put"
        );
        let counts: (i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM attachments),(SELECT count(*) FROM events),(SELECT count(*) FROM import_deferred_events)").fetch_one(&f.pool).await.unwrap();
        assert_eq!(counts, (0, 0, 0));
        let known = store_imported_asset(
            &f.backend,
            &settings,
            &storage,
            &claim,
            f.document,
            asset("reserve.txt"),
            &mut reserve_progress,
            &cancel,
        )
        .await
        .unwrap_err();
        assert!(matches!(known, RunError::Failed(reason) if reason.contains("Forbidden")));
        assert!(reserve_progress.is_empty());
        assert!(!root.exists());
        sqlx::query("UPDATE sessions SET revoked_at=NULL WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let mut healthy_progress = ImportJobRefs::default();
        store_imported_asset(
            &f.backend,
            &settings,
            &storage,
            &claim,
            f.document,
            asset("healthy.txt"),
            &mut healthy_progress,
            &cancel,
        )
        .await
        .unwrap();
        assert_eq!(healthy_progress.stored_keys.len(), 1);
        assert_eq!(
            std::fs::read(
                root.join("objects")
                    .join(&healthy_progress.stored_keys[0])
                    .join("payload")
            )
            .unwrap(),
            literal
        );
        let healthy: (String, String) =
            sqlx::query_as("SELECT status,name FROM attachments WHERE storage_key=?1")
                .bind(&healthy_progress.stored_keys[0])
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert_eq!(healthy, ("stored".into(), "healthy.txt".into()));
        let pause = Arc::new((Notify::new(), Notify::new()));
        let mut pending = tokio::spawn(IMPORT_ASSET_AFTER_PUT_CONTROL.scope(pause.clone(), {
            let backend = f.backend.clone();
            let storage = storage.clone();
            let settings = settings.clone();
            let claim = claim.clone();
            let cancel = cancel.clone();
            let document = f.document;
            let value = asset("finalize.txt");
            async move {
                let mut progress = ImportJobRefs::default();
                let result = store_imported_asset(
                    &backend,
                    &settings,
                    &storage,
                    &claim,
                    document,
                    value,
                    &mut progress,
                    &cancel,
                )
                .await;
                (result, progress)
            }
        }));
        tokio::select! {
            ()=pause.0.notified()=>{},
            result=&mut pending=>panic!("actual put must reach finalizer control: {result:?}"),
        }
        let row: (Vec<u8>, String, String) = sqlx::query_as("SELECT id,storage_key,status FROM attachments WHERE workspace_id=?1 AND document_id=?2 AND status='uploading'")
            .bind(f.workspace.as_bytes().as_slice()).bind(f.document.as_bytes().as_slice()).fetch_one(&f.pool).await.unwrap();
        let attachment = Uuid::from_slice(&row.0).unwrap();
        assert_eq!(row.2, "uploading");
        assert_eq!(
            std::fs::read(root.join("objects").join(&row.1).join("payload")).unwrap(),
            literal
        );
        crate::db::attachments::import_rollback_test_hooks::arm((attachment, false));
        sqlx::query("UPDATE sessions SET revoked_at=1 WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        pause.1.notify_one();
        let (result, progress) = pending.await.unwrap();
        let error = result.unwrap_err();
        assert!(matches!(
            stopped(&f.backend, &error),
            ImportAttachmentRollbackReason::Domain(ImportAttachmentError::Attachment(
                crate::db::attachments::AttachmentDbError::Forbidden
            ))
        ));
        assert_eq!(progress.stored_keys, vec![row.1.clone()]);
        let state: (String, i64, Option<Vec<u8>>, String, i64) = sqlx::query_as("SELECT status,attempts,lease_token,created_refs,(SELECT count(*) FROM import_deferred_events) FROM import_jobs WHERE id=?1")
            .bind(claim.job_id.as_bytes().as_slice()).fetch_one(&f.pool).await.unwrap();
        assert_eq!(
            (&state.0, state.1, state.2.as_ref(), state.4),
            (
                &"running".to_owned(),
                1,
                Some(&claim.lease_token.as_bytes().to_vec()),
                1
            )
        );
        let refs: ImportJobRefs = serde_json::from_str(&state.3).unwrap();
        assert_eq!(refs.document_ids, vec![f.document]);
        assert_eq!(
            refs.stored_keys,
            vec![healthy_progress.stored_keys[0].clone(), row.1.clone()]
        );
        let effects: (i64, i64, String, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM documents),(SELECT count(*) FROM events),(SELECT status FROM attachments WHERE id=?1),(SELECT attempts FROM import_jobs WHERE id=?2)")
            .bind(attachment.as_bytes().as_slice()).bind(jobs[1].id.as_bytes().as_slice()).fetch_one(&f.pool).await.unwrap();
        assert_eq!(effects, (1, 0, "uploading".into(), 0));
        assert_eq!(
            std::fs::read(root.join("objects").join(&row.1).join("payload")).unwrap(),
            literal
        );
        let mime = sniff_mime_from_bytes(literal);
        let known = crate::db::attachments::mark_import_attachment_stored_backend(
            &f.backend,
            &storage,
            &settings.quota,
            &claim,
            f.document,
            attachment,
            &row.1,
            "finalize.txt",
            &mime,
            literal.len() as i64,
            &cancel,
        )
        .await
        .unwrap();
        assert!(matches!(
            known,
            Err(ImportAttachmentError::Attachment(
                crate::db::attachments::AttachmentDbError::Forbidden
            ))
        ));
        sqlx::query("UPDATE sessions SET revoked_at=NULL WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        crate::db::attachments::mark_import_attachment_stored_backend(
            &f.backend,
            &storage,
            &settings.quota,
            &claim,
            f.document,
            attachment,
            &row.1,
            "finalize.txt",
            &mime,
            literal.len() as i64,
            &cancel,
        )
        .await
        .unwrap()
        .unwrap();
        let repaired: (String, String, i64) = sqlx::query_as("SELECT status,storage_key,(SELECT count(*) FROM import_deferred_events) FROM attachments WHERE id=?1")
            .bind(attachment.as_bytes().as_slice()).fetch_one(&f.pool).await.unwrap();
        assert_eq!(repaired, ("stored".into(), row.1.clone(), 2));
        assert_eq!(
            std::fs::read(root.join("objects").join(&row.1).join("payload")).unwrap(),
            literal
        );
        assert!(!cancel.is_cancelled());
        f.close().await;
    }

    #[tokio::test]
    async fn import_selected_actual_rollback_control_stops_loop_before_compensation_and_next_claim()
    {
        let f = Fixture::new().await;
        let literal = b"physical referenced import bytes";
        let (_, key) = f.attachment(literal.len() as i64, "text/plain").await;
        let root = f.root.join("rollback-control");
        let storage = ObjectStorage::local(root.clone());
        storage.put_bytes(&key, literal.to_vec()).await.unwrap();
        let credential = Uuid::now_v7();
        let token = crate::auth::token::new_token();
        let mut tx = f.backend.begin_write().await.unwrap();
        tx.operation()
            .create_session(
                credential,
                f.user,
                &token.hash,
                chrono::DateTime::from_timestamp_micros(
                    chrono::Utc::now().timestamp_micros() + 86_400_000_000,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let mut jobs = Vec::new();
        for name in ["first.txt", "next.txt"] {
            jobs.push(
                crate::db::import_jobs::create_async_import_job_backend(
                    &f.backend,
                    f.workspace,
                    f.user,
                    credential,
                    ImportSource::OfficeFile,
                    crate::db::import_jobs::NewAsyncImport {
                        file_name: Some(name),
                        project_id: None,
                        payload: b"literal queued content",
                    },
                )
                .await
                .unwrap()
                .unwrap(),
            );
        }
        // Real prior durable key from a previous attempt; the physical object
        // is still referenced by an attachment, so compensation must refuse it.
        let prior = ImportJobRefs {
            stored_keys: vec![key.clone()],
            ..Default::default()
        };
        sqlx::query("UPDATE import_jobs SET created_refs=?2,created_at=?3 WHERE id=?1")
            .bind(jobs[0].id.as_bytes().as_slice())
            .bind(serde_json::to_string(&prior).unwrap())
            .bind(1_i64)
            .execute(&f.pool)
            .await
            .unwrap();
        let mut settings = ImportJobSettings::from_env();
        settings.office_helper = None;
        settings.markdown = None;
        settings.seed = None;
        let cancel = CancellationToken::new();
        let wake = Arc::new(Notify::new());
        crate::db::import_jobs::IMPORT_ROLLBACK_AFTER_ACK_CONTROL.scope(true, async {
            let claim = crate::db::import_jobs::claim_next_import_job_backend(&f.backend).await.unwrap().unwrap();
            assert_eq!(claim.job_id, jobs[0].id);
            let error = run_claimed(&f.backend, &settings, &storage, &cancel, &claim).await.unwrap_err();
            assert!(import_database_error_stops_scheduler(&f.backend, &error));
            let sqlx::Error::AnyDriverError(source) = &error else { panic!("typed rollback error required"); };
            let stopped = source.downcast_ref::<crate::db::backend::RollbackCleanupUnknown>().unwrap();
            let original = stopped.original.as_ref().unwrap().downcast_ref::<sqlx::Error>().unwrap();
            assert!(matches!(original, sqlx::Error::Protocol(message) if message.contains("cleanup key still referenced")));
            assert!(matches!(&stopped.cleanup, sqlx::Error::Io(error) if error.kind()==std::io::ErrorKind::ConnectionAborted));
            // Make the same real job claimable again without replacing its refs,
            // then exercise the actual scheduler loop's stop branch.
            sqlx::query("UPDATE import_jobs SET lease_until=1 WHERE id=?1").bind(jobs[0].id.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
            tokio::time::timeout(Duration::from_secs(5), import_loop(f.backend.clone(), settings.clone(), storage.clone(), cancel.clone(), wake.clone())).await.expect("unconfirmed rollback must stop the actual loop");
        }).await;
        assert!(!cancel.is_cancelled());
        let first: (String, i64, String, Option<Vec<u8>>) = sqlx::query_as(
            "SELECT status,attempts,created_refs,lease_token FROM import_jobs WHERE id=?1",
        )
        .bind(jobs[0].id.as_bytes().as_slice())
        .fetch_one(&f.pool)
        .await
        .unwrap();
        assert_eq!((&first.0, first.1), (&"running".to_owned(), 2));
        assert_eq!(
            serde_json::from_str::<ImportJobRefs>(&first.2).unwrap(),
            prior
        );
        assert!(first.3.is_some());
        let next: (String, i64) =
            sqlx::query_as("SELECT status,attempts FROM import_jobs WHERE id=?1")
                .bind(jobs[1].id.as_bytes().as_slice())
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert_eq!(next, ("running".into(), 0));
        let effects: (i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM documents),(SELECT count(*) FROM attachments),(SELECT count(*) FROM events)").fetch_one(&f.pool).await.unwrap();
        assert_eq!(effects, (1, 1, 0));
        assert_eq!(
            std::fs::read(root.join("objects").join(&key).join("payload")).unwrap(),
            literal
        );
        // A known acknowledged refusal still follows the bounded retry policy
        // while another attempt remains. It cannot discard these current refs
        // or the referenced literal object merely to manufacture progress.
        sqlx::query("UPDATE import_jobs SET lease_until=1,attempts=0 WHERE id=?1")
            .bind(jobs[0].id.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(
            run_next_import_backend(&f.backend, &settings, &storage, &cancel)
                .await
                .unwrap()
        );
        let retry: (String, i64, String, Option<Vec<u8>>) = sqlx::query_as(
            "SELECT status,attempts,created_refs,lease_token FROM import_jobs WHERE id=?1",
        )
        .bind(jobs[0].id.as_bytes().as_slice())
        .fetch_one(&f.pool)
        .await
        .unwrap();
        assert_eq!((&retry.0, retry.1), (&"running".to_owned(), 1));
        assert!(retry.3.is_none());
        assert_eq!(
            serde_json::from_str::<ImportJobRefs>(&retry.2).unwrap(),
            prior
        );
        assert_eq!(
            std::fs::read(root.join("objects").join(&key).join("payload")).unwrap(),
            literal
        );
        // Exercise the final allowed attempt: an acknowledged incomplete
        // compensation retries below that bound, and fails at the bound.
        // Keep the same refs/object and let the actual next job follow it.
        sqlx::query("UPDATE import_jobs SET lease_until=1,attempts=?2 WHERE id=?1")
            .bind(jobs[0].id.as_bytes().as_slice())
            .bind(IMPORT_MAX_ATTEMPTS - 1)
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(
            run_next_import_backend(&f.backend, &settings, &storage, &cancel)
                .await
                .unwrap()
        );
        let failed: String = sqlx::query_scalar("SELECT status FROM import_jobs WHERE id=?1")
            .bind(jobs[0].id.as_bytes().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(failed, "failed");
        let final_attempt: i64 = sqlx::query_scalar("SELECT attempts FROM import_jobs WHERE id=?1")
            .bind(jobs[0].id.as_bytes().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(final_attempt, i64::from(IMPORT_MAX_ATTEMPTS));
        assert!(
            run_next_import_backend(&f.backend, &settings, &storage, &cancel)
                .await
                .unwrap()
        );
        let next_attempt: i64 = sqlx::query_scalar("SELECT attempts FROM import_jobs WHERE id=?1")
            .bind(jobs[1].id.as_bytes().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(next_attempt, 1);
        assert_eq!(
            std::fs::read(root.join("objects").join(&key).join("payload")).unwrap(),
            literal
        );
        f.close().await;
    }

    #[tokio::test]
    async fn import_selected_native_failure_response_control_propagates_and_stops_before_next_claim(
    ) {
        let f = Fixture::new().await;
        let credential = Uuid::now_v7();
        let token = crate::auth::token::new_token();
        let mut tx = f.backend.begin_write().await.unwrap();
        tx.operation()
            .create_session(
                credential,
                f.user,
                &token.hash,
                chrono::DateTime::from_timestamp_micros(
                    chrono::Utc::now().timestamp_micros() + 86_400_000_000,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();
        // The full fixture has a live document: native preflight therefore
        // returns the actual empty-destination Conflict before parser I/O.
        let first = native_job(&f, credential).await;
        let second = native_job(&f, credential).await;
        let healthy = native_job(&f, credential).await;
        let office = crate::db::import_jobs::create_async_import_job_backend(
            &f.backend,
            f.workspace,
            f.user,
            credential,
            ImportSource::OfficeFile,
            crate::db::import_jobs::NewAsyncImport {
                file_name: Some("healthy.txt"),
                project_id: None,
                payload: b"literal healthy queued content",
            },
        )
        .await
        .unwrap()
        .unwrap();
        let mut settings = ImportJobSettings::from_env();
        settings.office_helper = None;
        settings.markdown = None;
        settings.seed = None;
        let storage = ObjectStorage::local(f.root.join("native-response-control"));
        let cancel = CancellationToken::new();
        let wake = Arc::new(Notify::new());
        let control = || {
            Arc::new((
                std::sync::atomic::AtomicUsize::new(0),
                cancel.clone(),
                wake.clone(),
            ))
        };
        let error = IMPORT_NATIVE_FAILED_RESPONSE_LOSS
            .scope(
                control(),
                run_next_import_backend(&f.backend, &settings, &storage, &cancel),
            )
            .await
            .unwrap_err();
        let sqlx::Error::AnyDriverError(error) = error else {
            panic!("original typed uncertainty must propagate")
        };
        let unknown = error
            .downcast_ref::<crate::db::backend::CommitUnknown>()
            .expect("original typed CommitUnknown");
        assert!(unknown
            .source
            .to_string()
            .contains("native failed-transition response-loss control"));
        IMPORT_NATIVE_FAILED_RESPONSE_LOSS
            .scope(
                control(),
                import_loop(
                    f.backend.clone(),
                    settings.clone(),
                    storage.clone(),
                    cancel.clone(),
                    wake.clone(),
                ),
            )
            .await;
        assert!(
            !cancel.is_cancelled(),
            "stop must come from uncertain outcome, not shutdown"
        );
        let rows: Vec<(Vec<u8>, String, i64)> =
            sqlx::query_as("SELECT id,status,attempts FROM import_jobs ORDER BY created_at,id")
                .fetch_all(&f.pool)
                .await
                .unwrap();
        assert_eq!(
            rows,
            vec![
                (first.as_bytes().to_vec(), "failed".into(), 1),
                (second.as_bytes().to_vec(), "failed".into(), 1),
                (healthy.as_bytes().to_vec(), "running".into(), 0),
                (office.id.as_bytes().to_vec(), "running".into(), 0)
            ]
        );
        let events: i64 = sqlx::query_scalar("SELECT count(*) FROM events")
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(events, 0);
        assert_eq!(
            std::fs::read_dir(&f.root)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name() == "native-response-control")
                .count(),
            0,
            "no storage I/O/purge occurred"
        );
        // Positive control: without response loss the same known destination
        // failure retains ordinary policy and the next real queued claim works.
        assert!(
            run_next_import_backend(&f.backend, &settings, &storage, &cancel)
                .await
                .unwrap()
        );
        let next = crate::db::import_jobs::claim_next_import_job_backend(&f.backend)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(next.job_id, office.id);
        assert_eq!(next.source, ImportSource::OfficeFile);
        assert_eq!(next.attempt, 1);
        f.close().await;
    }
}
