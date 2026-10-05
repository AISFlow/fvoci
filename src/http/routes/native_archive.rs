use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Path, RawQuery, Request, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use axum_extra::extract::CookieJar;
use serde::de::DeserializeOwned;
use serde_json::json;
use std::sync::{Arc, LazyLock};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    api::native_archive::*,
    db::native_archive::{self as db, NativeDbError},
    error::{AppError, ProblemCode},
    http::{
        guard::{check_origin, reject_bearer},
        state::AppState,
    },
    native_archive::{self as native, ArchiveError},
};

const BODY_CAP: usize = native::MAX_BYTES / 3 * 4 + 64 * 1024;
static ADMISSION: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(1)));

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/native-archive",
            get(export_native),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/native-archive/preflight",
            post(preflight).layer(DefaultBodyLimit::max(BODY_CAP)),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/native-archive/restore",
            post(restore).layer(DefaultBodyLimit::max(BODY_CAP)),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/native-archive/jobs/{job_id}",
            get(status),
        )
}

async fn authenticate(
    state: &AppState,
    headers: &HeaderMap,
    jar: &CookieJar,
    workspace: Uuid,
) -> Result<(Uuid, Uuid), AppError> {
    reject_bearer(headers)?;
    let auth = crate::http::authz::require_request_auth(
        state,
        headers,
        jar,
        crate::http::authz::Access::Session,
        Some(workspace),
    )
    .await?;
    Ok((auth.user_id, auth.credential_id))
}
async fn admission(state: &AppState, actor: Uuid) -> Result<OwnedSemaphorePermit, AppError> {
    state
        .rate_limiter
        .allow_window(
            &format!("native-archive:{actor}"),
            5,
            std::time::Duration::from_secs(15 * 60),
        )
        .await
        .map_err(AppError::rate_limited)?;
    ADMISSION
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::upload_capacity_exceeded(1))
}
async fn read_body<T: DeserializeOwned>(request: Request) -> Result<T, AppError> {
    if request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        != Some("application/json")
    {
        return Err(AppError::from_code(ProblemCode::UnsupportedMediaType));
    }
    let bytes = axum::body::to_bytes(request.into_body(), BODY_CAP)
        .await
        .map_err(|_| AppError::problem(StatusCode::PAYLOAD_TOO_LARGE, ProblemCode::InvalidInput))?;
    serde_json::from_slice(&bytes).map_err(|_| AppError::from_code(ProblemCode::InvalidInput))
}

fn archive_error(error: ArchiveError) -> AppError {
    // Only a bounded static reason is logged (never archive/child text); the
    // client receives the generic code (Unsupported also names the model).
    tracing::warn!(target: "native_archive", reason = native::log_reason(&error), "native archive refused");
    let (status, code, diagnostic) = match error {
        ArchiveError::Unsupported(detail) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "native_archive_unsupported",
            Some(detail),
        ),
        ArchiveError::Invalid(_) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "native_archive_invalid",
            None,
        ),
        ArchiveError::Limit => (StatusCode::PAYLOAD_TOO_LARGE, "native_archive_limit", None),
        ArchiveError::Worker => (
            StatusCode::SERVICE_UNAVAILABLE,
            "native_archive_worker_unavailable",
            None,
        ),
        ArchiveError::Cancelled => (
            StatusCode::REQUEST_TIMEOUT,
            "native_archive_cancelled",
            None,
        ),
    };
    let mut result = AppError::problem(status, ProblemCode::ImportFailed);
    result.params = Some(json!({"code":code,"diagnostic":diagnostic}));
    result
}
fn db_error(error: NativeDbError) -> AppError {
    match error {
        NativeDbError::Forbidden => AppError::from_code(ProblemCode::InsufficientPermissions),
        NativeDbError::Conflict | NativeDbError::Fenced => {
            AppError::from_code(ProblemCode::Conflict)
        }
        NativeDbError::Archive(error) => archive_error(error),
        NativeDbError::Sql(error) => {
            if error
                .as_database_error()
                .is_some_and(|e| e.is_unique_violation())
            {
                AppError::from_code(ProblemCode::Conflict)
            } else {
                tracing::error!(error=%error,"native_archive.db_failed");
                AppError::internal()
            }
        }
    }
}
fn config(state: &AppState) -> Result<crate::collab::CollabConfig, AppError> {
    if let Some(hub) = &state.collab {
        let mut cfg = crate::collab::CollabConfig::from_env()
            .ok_or_else(|| archive_error(ArchiveError::Worker))?;
        cfg.engine_bin = hub.engine_bin();
        cfg.limits = hub.limits();
        Ok(cfg)
    } else {
        crate::collab::CollabConfig::from_env().ok_or_else(|| archive_error(ArchiveError::Worker))
    }
}

/// `zoteroConnector` is the only key and may repeat (distinct UUIDs, at most
/// MAX_ZOTERO_CONNECTORS); anything else is refused before any read.
fn zotero_selection(raw: Option<String>) -> Result<Vec<Uuid>, AppError> {
    let refused = || archive_error(ArchiveError::Invalid("zotero connector selection".into()));
    let mut ids = Vec::new();
    for (key, value) in url::form_urlencoded::parse(raw.unwrap_or_default().as_bytes()) {
        let id = Uuid::parse_str(&value).map_err(|_| refused())?;
        if key != "zoteroConnector"
            || ids.contains(&id)
            || ids.len() == native::MAX_ZOTERO_CONNECTORS
        {
            return Err(refused());
        }
        ids.push(id);
    }
    Ok(ids)
}

async fn export_native(
    State(state): State<AppState>,
    Path((workspace, project)): Path<(Uuid, Uuid)>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Response, AppError> {
    let (actor, session) = authenticate(&state, &headers, &jar, workspace).await?;
    let connectors = zotero_selection(raw)?;
    let permit = admission(&state, actor).await?;
    let cancel = CancellationToken::new();
    let _cancel_on_drop = cancel.clone().drop_guard();
    let selection = db::Selection {
        project,
        zotero_connectors: &connectors,
    };
    let mut capture =
        db::capture_backend(&state.auth.db.pool, workspace, actor, session, &selection)
            .await
            .map_err(db_error)?;
    let mut total = capture
        .archive
        .entries
        .values()
        .map(|s| s.len() / 4 * 3)
        .sum::<usize>();
    for file in &capture.archive.graph.attachments {
        let key = capture
            .file_keys
            .get(&file.id)
            .ok_or_else(|| archive_error(ArchiveError::Invalid("file key".into())))?;
        total = total
            .checked_add(file.size_bytes as usize)
            .ok_or_else(|| archive_error(ArchiveError::Limit))?;
        if total > native::MAX_BYTES {
            return Err(archive_error(ArchiveError::Limit));
        }
        let bytes = native::read_file(&state.storage, key, file.size_bytes)
            .await
            .map_err(archive_error)?;
        db::recheck_file_backend(
            &state.auth.db.pool,
            workspace,
            actor,
            session,
            &capture.archive.graph,
            file,
            key,
        )
        .await
        .map_err(db_error)?;
        capture
            .archive
            .entries
            .insert(file.payload_entry.clone(), native::encode(&bytes));
    }
    let archive = native::validate_native(capture.archive, config(&state)?, &cancel)
        .await
        .map_err(archive_error)?;
    let delivery_graph = Arc::new(archive.graph.clone());
    let delivery_keys = Arc::new(capture.file_keys);
    let input = serde_json::to_vec(&archive).map_err(|_| AppError::internal())?;
    drop(archive);
    let helper = std::env::current_exe().map_err(|_| archive_error(ArchiveError::Worker))?;
    let bytes = native::decode(
        &native::container(&helper, input, true, &cancel)
            .await
            .map_err(archive_error)?,
    )
    .map_err(archive_error)?;
    db::recheck_delivery_backend(
        &state.auth.db.pool,
        workspace,
        actor,
        session,
        &delivery_graph,
        &delivery_keys,
    )
    .await
    .map_err(db_error)?;
    // Current credential and private-resource authorization on every bounded
    // delivery chunk. Bytes already in flight cannot be recalled.
    let pool = state.auth.db.pool.clone();
    let stream = futures_util::stream::try_unfold(
        (bytes, 0usize, permit),
        move |(bytes, offset, permit)| {
            let pool = pool.clone();
            let graph = delivery_graph.clone();
            let keys = delivery_keys.clone();
            async move {
                if offset == bytes.len() {
                    return Ok::<_, std::io::Error>(None);
                }
                db::recheck_delivery_backend(&pool, workspace, actor, session, &graph, &keys)
                    .await
                    .map_err(|_| std::io::Error::other("archive delivery authorization"))?;
                let end = (offset + 64 * 1024).min(bytes.len());
                let chunk = bytes::Bytes::copy_from_slice(&bytes[offset..end]);
                Ok(Some((chunk, (bytes, end, permit))))
            }
        },
    );
    Ok((
        [
            (header::CONTENT_TYPE, "application/zip"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"fvoci-native-project.zip\"",
            ),
            (header::CACHE_CONTROL, "no-store"),
        ],
        Body::from_stream(stream),
    )
        .into_response())
}

async fn preflight(
    State(state): State<AppState>,
    Path(workspace): Path<Uuid>,
    headers: HeaderMap,
    jar: CookieJar,
    request: Request,
) -> Result<Json<NativePreflightOutput>, AppError> {
    check_origin(&headers, &state.public_origin)?;
    let (actor, session) = authenticate(&state, &headers, &jar, workspace).await?;
    let _permit = admission(&state, actor).await?;
    db::preflight_destination_backend(&state.auth.db.pool, workspace, actor, session)
        .await
        .map_err(db_error)?;
    let body: NativePreflightBody = read_body(request).await?;
    let bytes = native::decode(&body.archive_base64).map_err(archive_error)?;
    let hash = native::digest(&bytes);
    let cancel = CancellationToken::new();
    let _guard = cancel.clone().drop_guard();
    let helper = std::env::current_exe().map_err(|_| archive_error(ArchiveError::Worker))?;
    let archive = native::parse(&helper, bytes, &cancel)
        .await
        .map_err(archive_error)?;
    let archive = native::validate_native(archive, config(&state)?, &cancel)
        .await
        .map_err(archive_error)?;
    db::preflight_destination_backend(&state.auth.db.pool, workspace, actor, session)
        .await
        .map_err(db_error)?;
    let graph = &archive.graph;
    Ok(Json(NativePreflightOutput {
        archive_hash: hash,
        source_workspace_id: graph.source_workspace_id,
        destination_workspace_id: workspace,
        destination_actor_id: actor,
        project_id: graph.project.id,
        project_name: graph.project.name.clone(),
        document_count: graph.documents.len(),
        task_count: graph.tasks.len(),
        attachment_count: graph.attachments.len(),
        revision_count: graph.revisions.len(),
        complete: true,
        diagnostics: Vec::new(),
        preserved_content_ids: true,
        requires_collision_free_installation: true,
    }))
}

async fn restore(
    State(state): State<AppState>,
    Path(workspace): Path<Uuid>,
    headers: HeaderMap,
    jar: CookieJar,
    request: Request,
) -> Result<Response, AppError> {
    check_origin(&headers, &state.public_origin)?;
    let (actor, session) = authenticate(&state, &headers, &jar, workspace).await?;
    let _permit = admission(&state, actor).await?;
    db::authorize_destination_backend(&state.auth.db.pool, workspace, actor, session)
        .await
        .map_err(db_error)?;
    let body: NativeRestoreBody = read_body(request).await?;
    if !body.confirm || body.destination_actor_id != actor {
        return Err(AppError::from_code(ProblemCode::ConfirmInvalid));
    }
    if state.import_wake.is_none() {
        return Err(archive_error(ArchiveError::Worker));
    }
    let bytes = native::decode(&body.archive_base64).map_err(archive_error)?;
    if native::digest(&bytes) != body.archive_hash {
        return Err(archive_error(ArchiveError::Invalid(
            "confirmed hash".into(),
        )));
    }
    let cancel = CancellationToken::new();
    let _guard = cancel.clone().drop_guard();
    let helper = std::env::current_exe().map_err(|_| archive_error(ArchiveError::Worker))?;
    let archive = native::parse(&helper, bytes.clone(), &cancel)
        .await
        .map_err(archive_error)?;
    native::validate_native(archive, config(&state)?, &cancel)
        .await
        .map_err(archive_error)?;
    // A replay may target a nonempty workspace after the original job committed;
    // queue_restore reauthorizes the actor and checks the durable command first.
    let id = db::queue_restore_backend(
        &state.auth.db.pool,
        workspace,
        actor,
        session,
        body.request_id,
        &body.archive_hash,
        &bytes,
    )
    .await
    .map_err(db_error)?;
    state.import_wake.as_ref().expect("checked").notify_one();
    let output = db::status_backend(&state.auth.db.pool, workspace, actor, session, id)
        .await
        .map_err(db_error)?;
    if output.status == "failed" && output.diagnostic.as_deref() == Some("conflict") {
        return Err(AppError::from_code(ProblemCode::Conflict));
    }
    Ok((StatusCode::ACCEPTED, Json(output)).into_response())
}
async fn status(
    State(state): State<AppState>,
    Path((workspace, id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<NativeJobOutput>, AppError> {
    let (actor, session) = authenticate(&state, &headers, &jar, workspace).await?;
    let output = db::status_backend(&state.auth.db.pool, workspace, actor, session, id)
        .await
        .map_err(db_error)?;
    if output.status == "failed" && output.diagnostic.as_deref() == Some("conflict") {
        return Err(AppError::from_code(ProblemCode::Conflict));
    }
    Ok(Json(output))
}
