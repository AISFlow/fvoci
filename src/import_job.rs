use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;

use document_extract_client::limits::Limits;
use document_extract_client::outcome::ExtractStatus;
use document_extract_client::process::{extract_killable, ExtractRequest};
use sqlx::PgPool;
use tokio::sync::{Mutex, Notify};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::warn;
use uuid::Uuid;

use crate::db::attachment_extract::default_extract_limits;
use crate::db::import_jobs::{
    save_import_progress, update_import_job_status, ImportJobRefs, ImportSource, ImportStatus,
};
use crate::documents::convert::ConvertClient;
use crate::documents::import_body::{apply_imported_markdown, create_imported_wiki_document};
use crate::documents::import_zip::{title_from_file_name, unzip_bounded, ZipImportError};

#[derive(Debug, Clone)]
pub struct ImportJobSettings {
    pub convert: ConvertClient,
    pub extractor_bin: Option<PathBuf>,
    pub extract_limits: Limits,
}

impl ImportJobSettings {
    pub fn from_env(convert: ConvertClient) -> Self {
        let extractor_bin = std::env::var("FVOCI_EXTRACTOR_BIN")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .map(PathBuf::from);
        Self {
            convert,
            extractor_bin,
            extract_limits: default_extract_limits(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ImportWorkItem {
    pub workspace_id: Uuid,
    pub job_id: Uuid,
    pub actor_user_id: Uuid,
    pub session_id: Uuid,
    pub source: ImportSource,
    pub file_bytes: Vec<u8>,
    pub file_name: Option<String>,
}

#[derive(Clone, Default)]
pub struct ImportQueue {
    inner: Arc<Mutex<VecDeque<ImportWorkItem>>>,
    wake: Arc<Notify>,
}

impl ImportQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn enqueue(&self, item: ImportWorkItem) {
        self.inner.lock().await.push_back(item);
        self.wake.notify_one();
    }

    async fn pop(&self) -> Option<ImportWorkItem> {
        self.inner.lock().await.pop_front()
    }

    pub fn wake(&self) -> Arc<Notify> {
        self.wake.clone()
    }
}

pub struct ImportJobHandle {
    cancel: CancellationToken,
    join: JoinHandle<()>,
}

impl ImportJobHandle {
    pub fn request_shutdown(&self) {
        self.cancel.cancel();
    }

    pub async fn join(self) -> Result<(), String> {
        self.join
            .await
            .map_err(|err| format!("import job task join failed: {err}"))?;
        Ok(())
    }
}

pub fn spawn_import_job(
    pool: PgPool,
    settings: ImportJobSettings,
    queue: ImportQueue,
) -> ImportJobHandle {
    let cancel = CancellationToken::new();
    let child_cancel = cancel.child_token();
    let wake = queue.wake();
    let join = tokio::spawn(async move {
        import_loop(pool, settings, queue, wake, child_cancel).await;
    });
    ImportJobHandle { cancel, join }
}

async fn import_loop(
    pool: PgPool,
    settings: ImportJobSettings,
    queue: ImportQueue,
    wake: Arc<Notify>,
    cancel: CancellationToken,
) {
    while !cancel.is_cancelled() {
        if let Some(item) = queue.pop().await {
            if let Err(err) = process_import_item(&pool, &settings, item).await {
                warn!("import job failed: {err}");
            }
            continue;
        }
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = wake.notified() => {},
        }
    }
}

async fn process_import_item(
    pool: &PgPool,
    settings: &ImportJobSettings,
    item: ImportWorkItem,
) -> Result<(), String> {
    let result = match item.source {
        ImportSource::OfficeFile => run_office_import(pool, settings, &item).await,
        ImportSource::NotionZip => {
            run_notion_zip_import(
                pool,
                &settings.convert,
                item.workspace_id,
                item.actor_user_id,
                item.session_id,
                &item.file_bytes,
            )
            .await
        }
        ImportSource::MarkdownZip => Err("markdown zip is synchronous".into()),
    };
    match result {
        Ok(created) => {
            let refs = ImportJobRefs {
                document_ids: created,
                task_ids: vec![],
                stored_keys: vec![],
            };
            let _ = save_import_progress(pool, item.workspace_id, item.job_id, &refs)
                .await
                .map_err(|e| e.to_string())?;
            update_import_job_status(
                pool,
                item.workspace_id,
                item.job_id,
                ImportStatus::Completed,
            )
            .await
            .map_err(|e| e.to_string())?;
            Ok(())
        }
        Err(err) => {
            let _ = update_import_job_status(
                pool,
                item.workspace_id,
                item.job_id,
                ImportStatus::Failed,
            )
            .await;
            Err(err)
        }
    }
}

pub async fn run_markdown_zip_import(
    pool: &PgPool,
    convert: &ConvertClient,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    zip_bytes: &[u8],
) -> Result<Vec<Uuid>, String> {
    let entries = match unzip_bounded(zip_bytes) {
        Ok(v) => v,
        Err(ZipImportError::Empty) => return Err("zip required".into()),
        Err(_) => return Err("import failed".into()),
    };
    let mut created = Vec::new();
    for entry in entries {
        if !entry.name.to_lowercase().ends_with(".md") {
            continue;
        }
        let markdown = String::from_utf8(entry.data).map_err(|_| "import failed".to_string())?;
        let title = title_from_file_name(&entry.name);
        let doc_id = create_imported_wiki_document(
            pool,
            workspace_id,
            actor_user_id,
            session_id,
            &title,
            None,
            None,
        )
        .await
        .map_err(|e| format!("{e}"))?;
        apply_imported_markdown(
            pool,
            convert,
            workspace_id,
            actor_user_id,
            session_id,
            doc_id,
            &markdown,
            None,
        )
        .await
        .map_err(|e| format!("{e}"))?;
        created.push(doc_id);
    }
    Ok(created)
}

async fn run_office_import(
    pool: &PgPool,
    settings: &ImportJobSettings,
    item: &ImportWorkItem,
) -> Result<Vec<Uuid>, String> {
    let name = item.file_name.clone().unwrap_or_else(|| "file".to_string());
    let md = office_file_to_markdown(settings, &item.file_bytes, &name)?;
    let title = title_from_file_name(&name);
    let doc_id = create_imported_wiki_document(
        pool,
        item.workspace_id,
        item.actor_user_id,
        item.session_id,
        &title,
        None,
        None,
    )
    .await
    .map_err(|e| format!("{e}"))?;
    apply_imported_markdown(
        pool,
        &settings.convert,
        item.workspace_id,
        item.actor_user_id,
        item.session_id,
        doc_id,
        &md,
        None,
    )
    .await
    .map_err(|e| format!("{e}"))?;
    Ok(vec![doc_id])
}

fn office_file_to_markdown(
    settings: &ImportJobSettings,
    bytes: &[u8],
    file_name: &str,
) -> Result<String, String> {
    let lower = file_name.to_lowercase();
    if lower.ends_with(".hwp") || lower.ends_with(".hwpx") {
        let bin = settings
            .extractor_bin
            .as_ref()
            .ok_or_else(|| "office import unavailable".to_string())?;
        let report = extract_killable(ExtractRequest {
            bytes: bytes.to_vec(),
            name: file_name.to_string(),
            limits: settings.extract_limits,
            extractor_bin: bin.clone(),
            test_hang_ms: None,
        });
        return match report.outcome {
            ExtractStatus::Ok { text, .. } | ExtractStatus::Partial { text, .. } => Ok(text),
            _ => Err("import failed".into()),
        };
    }
    if lower.ends_with(".md") || lower.ends_with(".markdown") || lower.ends_with(".txt") {
        return String::from_utf8(bytes.to_vec()).map_err(|_| "import failed".into());
    }
    Err("office import unavailable".into())
}

async fn run_notion_zip_import(
    pool: &PgPool,
    convert: &ConvertClient,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    zip_bytes: &[u8],
) -> Result<Vec<Uuid>, String> {
    let entries = unzip_bounded(zip_bytes).map_err(|_| "import failed".to_string())?;
    if entries
        .iter()
        .any(|e| e.name.to_lowercase().ends_with(".html"))
    {
        return Err("notion html export unsupported".into());
    }
    let mut created = Vec::new();
    let mut path_to_id = std::collections::HashMap::new();
    let mut pages: Vec<(String, Option<String>, String, String)> = Vec::new();
    for entry in entries {
        if entry.name.to_lowercase().ends_with(".md") {
            let parent = parent_path(&entry.name);
            let title = title_from_file_name(&entry.name);
            let markdown =
                String::from_utf8(entry.data).map_err(|_| "import failed".to_string())?;
            pages.push((entry.name.clone(), parent, title, markdown));
        }
    }
    pages.sort_by_key(|(path, _, _, _)| path.split('/').count());
    for (path, parent_path, title, markdown) in pages {
        let parent_id = parent_path
            .as_ref()
            .and_then(|p| path_to_id.get(p).copied());
        let doc_id = create_imported_wiki_document(
            pool,
            workspace_id,
            actor_user_id,
            session_id,
            &title,
            parent_id,
            None,
        )
        .await
        .map_err(|e| format!("{e}"))?;
        apply_imported_markdown(
            pool,
            convert,
            workspace_id,
            actor_user_id,
            session_id,
            doc_id,
            &markdown,
            None,
        )
        .await
        .map_err(|e| format!("{e}"))?;
        path_to_id.insert(path, doc_id);
        created.push(doc_id);
    }
    Ok(created)
}

fn parent_path(path: &str) -> Option<String> {
    let normalized = path.replace('\\', "/");
    if !normalized.contains('/') {
        return None;
    }
    let dir = normalized.rsplit_once('/').map(|(d, _)| d.to_string())?;
    if dir.is_empty() {
        None
    } else {
        Some(dir)
    }
}
