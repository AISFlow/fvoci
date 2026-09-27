//! GET /api/v1/workspaces/{workspace_id}/export — streaming workspace ZIP.

use std::collections::HashSet;
use std::io;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum_extra::extract::CookieJar;
use bytes::Bytes;
use futures_util::StreamExt;
use serde::Serialize;
use uuid::Uuid;

use crate::attachments::ObjectStorage;
use crate::db::workspace::WorkspaceDbError;
use crate::db::workspace_export::{
    self, AttachmentMetaJson, CommentJson, DocumentJson, TaskJson, WorkspaceExportDbError,
    WorkspaceExportSnapshot, WorkspaceJson,
};
use crate::error::AppError;
use crate::export_zip::{zip_safe_name, ZipStream};
use crate::http::state::AppState;

use super::{internal, map_workspace_error, require_session};

const WS_EXPORT_PER_USER: u32 = 5;
const WS_EXPORT_WINDOW: Duration = Duration::from_secs(15 * 60);
const EXPORT_CHANNEL_DEPTH: usize = 4;

static EXPORT_INFLIGHT: LazyLock<Mutex<HashSet<Uuid>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

struct ExportInflightGuard {
    user_id: Uuid,
    released: bool,
}

impl ExportInflightGuard {
    fn acquire(user_id: Uuid) -> Result<Self, AppError> {
        let mut inflight = EXPORT_INFLIGHT
            .lock()
            .expect("workspace export inflight mutex poisoned");
        if !inflight.insert(user_id) {
            return Err(AppError::rate_limited(WS_EXPORT_WINDOW.as_secs() as u32));
        }
        Ok(Self {
            user_id,
            released: false,
        })
    }

    fn release(&mut self) {
        if self.released {
            return;
        }
        if let Ok(mut inflight) = EXPORT_INFLIGHT.lock() {
            inflight.remove(&self.user_id);
        }
        self.released = true;
    }
}

impl Drop for ExportInflightGuard {
    fn drop(&mut self) {
        self.release();
    }
}

#[derive(Debug)]
enum ExportFailure {
    Closed,
    Db,
    Storage,
    Zip,
    Encode,
    Auth,
    Limit,
}

type ExportSender = mpsc::Sender<Result<Bytes, io::Error>>;

use tokio::sync::mpsc;

struct ExportWriter {
    tx: ExportSender,
    zip: ZipStream,
}

struct JsonByteBudget {
    used: u64,
}

impl JsonByteBudget {
    fn charge(&mut self, bytes: usize) -> Result<(), ExportFailure> {
        self.used += bytes as u64;
        if self.used > workspace_export::EXPORT_MAX_JSON_BYTES {
            return Err(ExportFailure::Limit);
        }
        Ok(())
    }
}

impl ExportWriter {
    async fn send(&self, bytes: Bytes) -> Result<(), ExportFailure> {
        if bytes.is_empty() {
            return Ok(());
        }
        self.tx
            .send(Ok(bytes))
            .await
            .map_err(|_| ExportFailure::Closed)
    }

    async fn begin(&mut self, name: &str) -> Result<(), ExportFailure> {
        let header = self.zip.begin_entry(name).map_err(|_| ExportFailure::Zip)?;
        self.send(header).await
    }

    async fn data(&mut self, bytes: Bytes) -> Result<(), ExportFailure> {
        self.zip
            .entry_data(&bytes)
            .map_err(|_| ExportFailure::Zip)?;
        self.send(bytes).await
    }

    async fn end(&mut self) -> Result<(), ExportFailure> {
        let descriptor = self.zip.end_entry().map_err(|_| ExportFailure::Zip)?;
        self.send(descriptor).await
    }

    async fn file(&mut self, name: &str, bytes: Bytes) -> Result<(), ExportFailure> {
        let chunk = self
            .zip
            .whole_entry(name, &bytes)
            .map_err(|_| ExportFailure::Zip)?;
        self.send(chunk).await
    }
}

fn pretty_element<T: Serialize>(value: &T) -> Result<String, ExportFailure> {
    serde_json::to_string_pretty(value)
        .map(|s| format!("  {s}"))
        .map_err(|_| ExportFailure::Encode)
}

fn map_recheck(
    check: Result<Result<(), WorkspaceExportDbError>, sqlx::Error>,
) -> Result<(), ExportFailure> {
    match check {
        Ok(Ok(())) => Ok(()),
        Ok(Err(WorkspaceExportDbError::Forbidden | WorkspaceExportDbError::NotFound)) => {
            Err(ExportFailure::Auth)
        }
        Ok(Err(WorkspaceExportDbError::Truncated)) => Err(ExportFailure::Limit),
        Err(err) => {
            tracing::error!(error = %err, "workspace_export.recheck_failed");
            Err(ExportFailure::Db)
        }
    }
}

async fn ensure_still_authorized(
    pool: &sqlx::PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
    session_id: Uuid,
) -> Result<(), ExportFailure> {
    map_recheck(
        workspace_export::recheck_export_authorization(pool, workspace_id, user_id, session_id)
            .await,
    )
}

async fn ensure_attachment_delivery(
    pool: &sqlx::PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
    session_id: Uuid,
    document_id: Option<Uuid>,
    task_id: Option<Uuid>,
) -> Result<(), ExportFailure> {
    map_recheck(
        workspace_export::recheck_attachment_delivery(
            pool,
            workspace_id,
            user_id,
            session_id,
            document_id,
            task_id,
        )
        .await,
    )
}

async fn write_json_element(
    writer: &mut ExportWriter,
    budget: &mut JsonByteBudget,
    first: &mut bool,
    element: String,
) -> Result<(), ExportFailure> {
    let chunk = if *first {
        *first = false;
        format!("{element}\n")
    } else {
        format!(",{element}\n")
    };
    budget.charge(chunk.len())?;
    writer.data(Bytes::from(chunk)).await
}

async fn write_workspace_zip(
    writer: &mut ExportWriter,
    pool: &sqlx::PgPool,
    storage: &ObjectStorage,
    workspace_id: Uuid,
    user_id: Uuid,
    session_id: Uuid,
    snapshot: WorkspaceExportSnapshot,
) -> Result<(), ExportFailure> {
    let mut json_budget = JsonByteBudget { used: 0 };
    ensure_still_authorized(pool, workspace_id, user_id, session_id).await?;

    let workspace_json = serde_json::to_string_pretty(&WorkspaceJson {
        id: snapshot.workspace.id.to_string(),
        slug: snapshot.workspace.slug.clone(),
        name: snapshot.workspace.name.clone(),
        excluded_private_project_count: snapshot.workspace.excluded_private_project_count,
    })
    .map_err(|_| ExportFailure::Encode)?;
    let workspace_payload = format!("{workspace_json}\n");
    json_budget.charge(workspace_payload.len())?;
    writer
        .file("workspace.json", Bytes::from(workspace_payload))
        .await?;

    writer.begin("documents.json").await?;
    json_budget.charge(2)?;
    writer.data(Bytes::from_static(b"[\n")).await?;
    let mut first_doc = true;
    for doc in &snapshot.documents {
        map_recheck(
            workspace_export::recheck_document_delivery(
                pool,
                workspace_id,
                user_id,
                session_id,
                doc.id,
                doc.project_id,
            )
            .await,
        )?;
        let body = workspace_export::fetch_document_body(pool, workspace_id, doc.id)
            .await
            .map_err(|_| ExportFailure::Db)?;
        let Some(body) = body else {
            return Err(ExportFailure::Auth);
        };
        let element = pretty_element(&DocumentJson {
            id: doc.id.to_string(),
            parent_id: doc.parent_id.map(|id| id.to_string()),
            title: &doc.title,
            status: &doc.status,
            text: &body.text,
            content_json: &body.content_json,
        })?;
        write_json_element(writer, &mut json_budget, &mut first_doc, element).await?;
    }
    let doc_close: &[u8] = if first_doc { b"]\n" } else { b"\n]\n" };
    json_budget.charge(doc_close.len())?;
    writer.data(Bytes::copy_from_slice(doc_close)).await?;
    writer.end().await?;

    writer.begin("tasks.json").await?;
    json_budget.charge(2)?;
    writer.data(Bytes::from_static(b"[\n")).await?;
    let mut first_task = true;
    for task in &snapshot.tasks {
        map_recheck(
            workspace_export::recheck_task_delivery(
                pool,
                workspace_id,
                user_id,
                session_id,
                task.project_id,
            )
            .await,
        )?;
        let element = pretty_element(&TaskJson {
            id: task.id.to_string(),
            project_id: task.project_id.to_string(),
            title: task.title.clone(),
            status_id: task.status_id.to_string(),
            task_type: task.task_type.clone(),
            parent_id: task.parent_id.map(|id| id.to_string()),
            archived_at: task.archived_at.map(workspace_export::js_iso),
            created_at: workspace_export::js_iso(task.created_at),
        })?;
        write_json_element(writer, &mut json_budget, &mut first_task, element).await?;
    }
    let task_close: &[u8] = if first_task { b"]\n" } else { b"\n]\n" };
    json_budget.charge(task_close.len())?;
    writer.data(Bytes::copy_from_slice(task_close)).await?;
    writer.end().await?;

    ensure_still_authorized(pool, workspace_id, user_id, session_id).await?;

    writer.begin("comments.json").await?;
    json_budget.charge(2)?;
    writer.data(Bytes::from_static(b"[\n")).await?;
    let mut first_comment = true;
    for comment in &snapshot.comments {
        map_recheck(
            workspace_export::recheck_comment_delivery(
                pool,
                workspace_id,
                user_id,
                session_id,
                comment.document_id,
                comment.task_id,
            )
            .await,
        )?;
        let body = workspace_export::fetch_comment_body(pool, workspace_id, comment.id)
            .await
            .map_err(|_| ExportFailure::Db)?
            .ok_or(ExportFailure::Auth)?;
        let element = pretty_element(&CommentJson {
            id: comment.id.to_string(),
            document_id: comment.document_id.map(|id| id.to_string()),
            task_id: comment.task_id.map(|id| id.to_string()),
            body: &body,
            created_at: workspace_export::js_iso(comment.created_at),
        })?;
        write_json_element(writer, &mut json_budget, &mut first_comment, element).await?;
    }
    let comment_close: &[u8] = if first_comment { b"]\n" } else { b"\n]\n" };
    json_budget.charge(comment_close.len())?;
    writer.data(Bytes::copy_from_slice(comment_close)).await?;
    writer.end().await?;

    writer.begin("attachments.json").await?;
    json_budget.charge(2)?;
    writer.data(Bytes::from_static(b"[\n")).await?;
    let mut first_att = true;
    for row in &snapshot.attachments {
        ensure_attachment_delivery(
            pool,
            workspace_id,
            user_id,
            session_id,
            row.document_id,
            row.task_id,
        )
        .await?;
        let element = pretty_element(&AttachmentMetaJson {
            id: row.id.to_string(),
            name: &row.name,
            mime: &row.mime,
            size_bytes: row.size_bytes,
            scan_status: &row.scan_status,
        })?;
        write_json_element(writer, &mut json_budget, &mut first_att, element).await?;
    }
    let att_close: &[u8] = if first_att { b"]\n" } else { b"\n]\n" };
    json_budget.charge(att_close.len())?;
    writer.data(Bytes::copy_from_slice(att_close)).await?;
    writer.end().await?;

    for row in &snapshot.attachments {
        ensure_attachment_delivery(
            pool,
            workspace_id,
            user_id,
            session_id,
            row.document_id,
            row.task_id,
        )
        .await?;
        if row.scan_status == "infected" {
            continue;
        }
        let Some(size) = storage.head(&row.storage_key).await.map_err(|err| {
            tracing::error!(error = %err, "workspace_export.storage_head_failed");
            ExportFailure::Storage
        })?
        else {
            continue;
        };
        let name = format!("attachments/{}-{}", row.id, zip_safe_name(&row.name));
        if size == 0 {
            writer.file(&name, Bytes::new()).await?;
            continue;
        }
        let mut stream = match storage
            .open_payload_stream(&row.storage_key, 0, size - 1)
            .await
        {
            Ok(stream) => stream,
            Err(err) => match storage.head(&row.storage_key).await {
                Ok(None) => continue,
                Ok(Some(_)) => {
                    tracing::error!(error = %err, "workspace_export.storage_open_failed");
                    return Err(ExportFailure::Storage);
                }
                Err(head_err) => {
                    tracing::error!(error = %head_err, "workspace_export.storage_head_retry_failed");
                    return Err(ExportFailure::Storage);
                }
            },
        };
        workspace_export::pause_attachment_payload_barrier_if_armed(row.id).await;
        ensure_attachment_delivery(
            pool,
            workspace_id,
            user_id,
            session_id,
            row.document_id,
            row.task_id,
        )
        .await?;
        writer.begin(&name).await?;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|err| {
                tracing::error!(error = %err, "workspace_export.storage_read_failed");
                ExportFailure::Storage
            })?;
            writer.data(chunk).await?;
        }
        writer.end().await?;
    }

    ensure_still_authorized(pool, workspace_id, user_id, session_id).await?;

    let tail = std::mem::take(&mut writer.zip)
        .finish()
        .map_err(|_| ExportFailure::Zip)?;
    writer.send(tail).await
}

pub async fn workspace_export(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
) -> Result<Response, AppError> {
    let (_user, user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::WorkspaceManage),
        Some(workspace_id),
    )
    .await?;
    state
        .rate_limiter
        .allow_window(
            &format!("ws-export:{user_id}"),
            WS_EXPORT_PER_USER,
            WS_EXPORT_WINDOW,
        )
        .await
        .map_err(AppError::rate_limited)?;

    let inflight_guard = ExportInflightGuard::acquire(user_id)?;

    let pool = state.auth.db.pool.clone();
    let loaded = workspace_export::load_export_snapshot(&pool, workspace_id, user_id, session_id)
        .await
        .map_err(internal)?;
    let snapshot = match loaded {
        Ok(snapshot) => snapshot,
        Err(WorkspaceExportDbError::NotFound | WorkspaceExportDbError::Forbidden) => {
            return Err(map_workspace_error(WorkspaceDbError::Forbidden, false));
        }
        Err(WorkspaceExportDbError::Truncated) => {
            tracing::error!(workspace_id = %workspace_id, "workspace_export.truncated");
            return Err(AppError::internal());
        }
    };

    let declared: u64 = snapshot
        .attachments
        .iter()
        .filter(|row| row.scan_status != "infected")
        .map(|row| row.size_bytes.unwrap_or(0).max(0) as u64)
        .sum();
    if declared > u64::from(u32::MAX) - 64 * 1024 * 1024
        || snapshot.attachments.len() > workspace_export::EXPORT_MAX_ATTACHMENTS
    {
        tracing::error!(
            attachments = snapshot.attachments.len(),
            "workspace_export.too_large_for_zip32"
        );
        return Err(AppError::internal());
    }

    let (tx, rx) = mpsc::channel(EXPORT_CHANNEL_DEPTH);
    let storage = state.storage.clone();
    tokio::spawn(async move {
        let _inflight_guard = inflight_guard;
        let mut writer = ExportWriter {
            tx,
            zip: ZipStream::new(),
        };
        let result = write_workspace_zip(
            &mut writer,
            &pool,
            &storage,
            workspace_id,
            user_id,
            session_id,
            snapshot,
        )
        .await;
        match result {
            Ok(()) | Err(ExportFailure::Closed) => {}
            Err(failure) => {
                tracing::error!(?failure, "workspace_export.failed");
                let _ = writer
                    .tx
                    .send(Err(io::Error::other("workspace export failed")))
                    .await;
            }
        }
    });
    let body = futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    });
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/zip"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"fvoci-workspace.zip\"",
            ),
            (header::CACHE_CONTROL, "no-store"),
        ],
        Body::from_stream(body),
    )
        .into_response())
}
