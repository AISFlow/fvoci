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
use crate::db::attachments::{create_import_attachment, mark_import_attachment_stored};
use crate::db::context::defer_import_events;
use crate::db::documents::ImportFence;
use crate::db::import_jobs::{
    claim_expired_import_job, claim_next_import_job, extend_import_lease, finish_import_job,
    finish_sync_import_job, load_import_payload, purge_imported_document, purge_imported_task,
    reset_import_refs, ImportClaim, ImportJobRefs, ImportSource, ImportStatus, IMPORT_SWEEP_MAX,
};
use crate::db::quota::StorageQuota;
use crate::db::tasks::{create_import_task, project_status_names, CreateTaskInput};
use crate::db::workspace::list_members;
use crate::documents::convert::ConvertClient;
use crate::documents::import_body::{
    apply_imported_markdown, create_fenced_wiki_document, create_imported_wiki_document,
    ImportBodyError,
};
use crate::documents::import_zip::{title_from_file_name, unzip_bounded, zip_safe_name, ZipEntry};
use crate::documents::markdown_helper::MarkdownHelper;
use crate::documents::office::{
    run_office_helper, OfficeCancelled, OfficeKind, OfficeLimits, OfficeMode, OfficeOutcome,
};

const DEFAULT_POLL: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct ImportJobSettings {
    pub convert: ConvertClient,
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
    /// `requireStorageReservation`; unlimited until the license port).
    pub quota: StorageQuota,
    pub poll_interval: Duration,
}

impl ImportJobSettings {
    fn seed_engine(&self) -> Result<&SeedEngine, RunError> {
        self.seed
            .as_ref()
            .ok_or_else(|| RunError::Failed("collab engine unavailable".into()))
    }

    fn markdown_helper(&self) -> Result<&MarkdownHelper, RunError> {
        self.markdown
            .as_ref()
            .ok_or_else(|| RunError::Failed("markdown helper unavailable".into()))
    }

    pub fn from_env(convert: ConvertClient) -> Self {
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
            convert,
            extractor_bin,
            extract_limits: default_extract_limits(),
            office_helper: std::env::current_exe().ok(),
            markdown: MarkdownHelper::current_exe().ok(),
            seed: SeedEngine::from_env(),
            office_limits: OfficeLimits::import(),
            quota: StorageQuota::default(),
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
    let cancel = CancellationToken::new();
    let wake = Arc::new(Notify::new());
    let join = tokio::spawn(import_loop(
        pool,
        settings,
        storage,
        cancel.child_token(),
        wake.clone(),
    ));
    ImportJobHandle { cancel, join, wake }
}

async fn import_loop(
    pool: PgPool,
    settings: ImportJobSettings,
    storage: ObjectStorage,
    cancel: CancellationToken,
    wake: Arc<Notify>,
) {
    while !cancel.is_cancelled() {
        let worked = match run_next_import(&pool, &settings, &storage, &cancel).await {
            Ok(worked) => worked,
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
    let Some(claim) = claim_next_import_job(pool).await? else {
        return Ok(false);
    };
    info!(
        workspace_id = %claim.workspace_id,
        import_job_id = %claim.job_id,
        attempt = claim.attempt,
        source = claim.source.as_str(),
        "import.claimed"
    );
    run_claimed(pool, settings, storage, cancel, &claim).await;
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
    Failed(String),
}

impl From<ImportBodyError> for RunError {
    fn from(err: ImportBodyError) -> Self {
        match err {
            ImportBodyError::Fenced => RunError::Fenced,
            other => RunError::Failed(other.to_string()),
        }
    }
}

fn db_failed(err: sqlx::Error) -> RunError {
    RunError::Failed(err.to_string())
}

async fn run_claimed(
    pool: &PgPool,
    settings: &ImportJobSettings,
    storage: &ObjectStorage,
    cancel: &CancellationToken,
    claim: &ImportClaim,
) {
    let mut created = ImportJobRefs::default();
    let result = run_claimed_inner(pool, settings, storage, cancel, claim, &mut created).await;
    match result {
        Ok(()) => match finish_import_job(pool, claim, ImportStatus::Completed).await {
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
            Err(err) => error!(error = %err, import_job_id = %claim.job_id, "import.finish_failed"),
        },
        Err(RunError::Fenced) => warn!(
            workspace_id = %claim.workspace_id,
            import_job_id = %claim.job_id,
            "import.failed reason=fenced"
        ),
        Err(err) => {
            let undo = compensate_import(pool, storage, claim.workspace_id, &created).await;
            if undo.failed > 0 {
                error!(
                    workspace_id = %claim.workspace_id,
                    import_job_id = %claim.job_id,
                    failed = undo.failed,
                    "import.compensate_failed"
                );
            }
            let reason = match &err {
                RunError::Aborted => "shutdown".to_string(),
                RunError::Failed(detail) => error_hash(detail),
                RunError::Fenced => unreachable!(),
            };
            match finish_import_job(pool, claim, ImportStatus::Failed).await {
                Ok(true) => {}
                Ok(false) => warn!(import_job_id = %claim.job_id, "import.fail_after_fence_lost"),
                Err(e) => error!(error = %e, import_job_id = %claim.job_id, "import.finish_failed"),
            }
            warn!(
                workspace_id = %claim.workspace_id,
                import_job_id = %claim.job_id,
                reason,
                "import.failed"
            );
        }
    }
}

async fn run_claimed_inner(
    pool: &PgPool,
    settings: &ImportJobSettings,
    storage: &ObjectStorage,
    cancel: &CancellationToken,
    claim: &ImportClaim,
    created: &mut ImportJobRefs,
) -> Result<(), RunError> {
    // Source `startImportRun`: undo what a dead previous run left, then clear refs.
    if !claim.prior_refs.is_empty() {
        let undo = compensate_import(pool, storage, claim.workspace_id, &claim.prior_refs).await;
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
        if !reset_import_refs(pool, claim).await.map_err(db_failed)? {
            return Err(RunError::Fenced);
        }
    }
    if cancel.is_cancelled() {
        return Err(RunError::Aborted);
    }
    let Some(payload) = load_import_payload(pool, claim).await.map_err(db_failed)? else {
        return Err(RunError::Fenced);
    };
    let fence = ImportFence {
        job_id: claim.job_id,
        lease_token: claim.lease_token,
    };
    match claim.source {
        ImportSource::OfficeFile => {
            run_office_import(pool, settings, cancel, claim, fence, payload, created).await
        }
        // Source `deferEvents`: events of every row this run creates are
        // parked until the `completed` transition publishes them.
        ImportSource::NotionZip => {
            defer_import_events(
                claim.job_id,
                run_notion_import(
                    pool, settings, storage, cancel, claim, fence, payload, created,
                ),
            )
            .await
        }
        ImportSource::MarkdownZip => Err(RunError::Failed("markdown-zip is synchronous".into())),
    }
}

async fn run_office_import(
    pool: &PgPool,
    settings: &ImportJobSettings,
    cancel: &CancellationToken,
    claim: &ImportClaim,
    fence: ImportFence,
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
    if !extend_import_lease(pool, claim).await.map_err(db_failed)? {
        return Err(RunError::Fenced);
    }
    let document_id = create_fenced_wiki_document(
        pool,
        claim.workspace_id,
        claim.created_by,
        claim.session_id,
        &title_from_file_name(&name),
        None,
        fence,
    )
    .await?;
    created.document_ids.push(document_id);
    if markdown.trim().is_empty() {
        return Ok(());
    }
    apply_imported_markdown(
        pool,
        settings.seed_engine()?,
        settings.markdown_helper()?,
        claim.workspace_id,
        claim.created_by,
        claim.session_id,
        document_id,
        &markdown,
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

#[allow(clippy::too_many_arguments)]
async fn run_notion_import(
    pool: &PgPool,
    settings: &ImportJobSettings,
    storage: &ObjectStorage,
    cancel: &CancellationToken,
    claim: &ImportClaim,
    fence: ImportFence,
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
        let document_id = create_fenced_wiki_document(
            pool,
            claim.workspace_id,
            claim.created_by,
            claim.session_id,
            &page.title,
            parent_id,
            fence,
        )
        .await?;
        created.document_ids.push(document_id);
        if !page.markdown.is_empty() {
            apply_imported_markdown(
                pool,
                settings.seed_engine()?,
                settings.markdown_helper()?,
                claim.workspace_id,
                claim.created_by,
                claim.session_id,
                document_id,
                &page.markdown,
            )
            .await?;
        }
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
            fence,
            document_id,
            asset,
            created,
        )
        .await?;
    }
    if let Some(project_id) = claim.project_id {
        import_notion_databases(
            pool,
            cancel,
            claim,
            fence,
            project_id,
            export.databases,
            created,
        )
        .await?;
    }
    Ok(())
}

/// Source `storeImportedAsset`: reserve (row + key ref, fenced), write the
/// object, then mark it stored with the sniffed MIME.
#[allow(clippy::too_many_arguments)]
async fn store_imported_asset(
    pool: &PgPool,
    settings: &ImportJobSettings,
    storage: &ObjectStorage,
    claim: &ImportClaim,
    fence: ImportFence,
    document_id: Uuid,
    asset: NotionAsset,
    created: &mut ImportJobRefs,
) -> Result<(), RunError> {
    // Attachment rows reserve at least one byte; an empty file has nothing
    // to store.
    if asset.data.is_empty() {
        return Ok(());
    }
    let name: String = zip_safe_name(&asset.name).chars().take(255).collect();
    let size = asset.data.len() as i64;
    let reserved = create_import_attachment(
        pool,
        &settings.quota,
        claim.workspace_id,
        claim.created_by,
        claim.session_id,
        document_id,
        &name,
        size,
        fence,
    )
    .await
    .map_err(db_failed)?;
    let (attachment_id, storage_key) = match reserved {
        Ok(Some(reserved)) => reserved,
        Ok(None) => return Err(RunError::Fenced),
        Err(err) => return Err(RunError::Failed(format!("import attachment: {err:?}"))),
    };
    created.stored_keys.push(storage_key.clone());
    let mime = sniff_mime_from_bytes(&asset.data);
    storage
        .put_bytes(&storage_key, asset.data)
        .await
        .map_err(|err| RunError::Failed(format!("import attachment put: {err}")))?;
    let stored = mark_import_attachment_stored(
        pool,
        claim.workspace_id,
        claim.created_by,
        attachment_id,
        &name,
        &mime,
        size,
        fence,
    )
    .await
    .map_err(db_failed)?;
    if !stored {
        return Err(RunError::Failed(format!(
            "import attachment {attachment_id}: finalize lost"
        )));
    }
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
    pool: &PgPool,
    cancel: &CancellationToken,
    claim: &ImportClaim,
    fence: ImportFence,
    project_id: Uuid,
    databases: Vec<NotionDatabase>,
    created: &mut ImportJobRefs,
) -> Result<(), RunError> {
    if databases.iter().all(|db| db.rows.len() < 2) {
        return Ok(());
    }
    let statuses = project_status_names(pool, claim.workspace_id, project_id)
        .await
        .map_err(db_failed)?;
    let members = match list_members(pool, claim.workspace_id, claim.created_by, claim.session_id)
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
            let created_task = create_import_task(
                pool,
                claim.workspace_id,
                project_id,
                claim.created_by,
                claim.session_id,
                CreateTaskInput {
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
                fence,
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
/// reference parents), then stored objects.
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
    for key in keys {
        // Also aborts an open multipart upload, which could otherwise
        // publish an object after its row is gone.
        if let Err(err) = storage.purge_key(&key).await {
            out.failed += 1;
            warn!(error = %err, "import.compensate_object_failed");
        }
    }
    out
}

/// Source `sweepOrphanImports`, run by the daily maintenance sweep.
pub async fn sweep_orphan_imports(
    pool: &PgPool,
    storage: &ObjectStorage,
    cancel: &CancellationToken,
) -> Result<u32, sqlx::Error> {
    let mut swept = 0;
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
/// becomes a root document. On failure the row is marked failed; documents
/// already created stay (the source does not compensate this path).
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
