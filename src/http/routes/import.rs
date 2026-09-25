use std::net::SocketAddr;

use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use serde::Deserialize;
use uuid::Uuid;

use crate::api::dto::ImportJobResponse;
use crate::auth::session::SessionUser;
use crate::db::import_jobs::{
    create_import_job, get_import_job, update_import_job_status, ImportDbError, ImportSource,
    ImportStatus, IMPORT_HTTP_MAX_BYTES,
};
use crate::error::{AppError, ProblemCode};
use crate::http::guard::{check_origin, reject_bearer};
use crate::http::rate_limit::peer_ip;
use crate::http::state::AppState;
use crate::import_job::{run_markdown_zip_import, ImportWorkItem};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/import", post(start_import))
        .route("/api/v1/import/{import_job_id}", get(get_import_status))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartImportBody {
    workspace_id: Uuid,
    source: String,
    zip_base64: Option<String>,
    file_name: Option<String>,
    #[allow(dead_code)]
    project_id: Option<Uuid>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportStatusQuery {
    workspace_id: Uuid,
}

async fn start_import(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    body: Json<StartImportBody>,
) -> Result<Response, AppError> {
    reject_bearer(&headers)?;
    check_origin(&headers, &state.public_origin)?;
    let (_user, user_id, session_id) = require_session(&state, &headers, &jar).await?;
    let _ = peer_ip(peer.ip());
    if let Err(retry_after) = state
        .rate_limiter
        .allow_window(
            &format!("import-user:{user_id}"),
            5,
            std::time::Duration::from_secs(15 * 60),
        )
        .await
    {
        return Err(AppError::rate_limited(retry_after));
    }
    let source = ImportSource::parse(&body.source)
        .ok_or_else(|| AppError::from_code(ProblemCode::InvalidInput))?;
    let file_bytes = match &body.zip_base64 {
        Some(b64) => B64
            .decode(b64.trim())
            .map_err(|_| AppError::from_code(ProblemCode::InvalidInput))?,
        None => Vec::new(),
    };
    if file_bytes.len() > IMPORT_HTTP_MAX_BYTES {
        return Err(AppError::from_code(ProblemCode::InvalidInput));
    }
    let convert = state
        .document_convert
        .as_ref()
        .ok_or_else(|| AppError::from_code(ProblemCode::InternalError))?;
    let initial_status = if source == ImportSource::MarkdownZip {
        ImportStatus::Pending
    } else {
        ImportStatus::Running
    };
    let job = create_import_job(
        &state.auth.db.pool,
        body.workspace_id,
        user_id,
        session_id,
        source,
        initial_status,
    )
    .await
    .map_err(internal)?
    .map_err(map_import_error)?;
    let created_ids = if source == ImportSource::MarkdownZip {
        let ids = run_markdown_zip_import(
            &state.auth.db.pool,
            convert,
            body.workspace_id,
            user_id,
            session_id,
            &file_bytes,
        )
        .await
        .map_err(|_| AppError::from_code(ProblemCode::InvalidInput))?;
        let _ = update_import_job_status(
            &state.auth.db.pool,
            body.workspace_id,
            job.id,
            ImportStatus::Completed,
        )
        .await
        .map_err(internal)?;
        ids
    } else {
        if file_bytes.is_empty() {
            let _ = update_import_job_status(
                &state.auth.db.pool,
                body.workspace_id,
                job.id,
                ImportStatus::Failed,
            )
            .await;
            return Err(AppError::from_code(ProblemCode::InvalidInput));
        }
        state
            .import_queue
            .enqueue(ImportWorkItem {
                workspace_id: body.workspace_id,
                job_id: job.id,
                actor_user_id: user_id,
                session_id,
                source,
                file_bytes,
                file_name: body.file_name.clone(),
            })
            .await;
        Vec::new()
    };
    let response = ImportJobResponse {
        id: job.id.to_string(),
        workspace_id: body.workspace_id.to_string(),
        source: body.source.clone(),
        status: if source == ImportSource::MarkdownZip {
            "completed".to_string()
        } else {
            "running".to_string()
        },
        created_document_ids: created_ids.iter().map(|id| id.to_string()).collect(),
    };
    Ok((StatusCode::CREATED, Json(response)).into_response())
}

async fn get_import_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(import_job_id): Path<Uuid>,
    Query(query): Query<ImportStatusQuery>,
) -> Result<Json<ImportJobResponse>, AppError> {
    reject_bearer(&headers)?;
    check_origin(&headers, &state.public_origin)?;
    let (_user, user_id, session_id) = require_session(&state, &headers, &jar).await?;
    let job = get_import_job(
        &state.auth.db.pool,
        query.workspace_id,
        user_id,
        session_id,
        import_job_id,
    )
    .await
    .map_err(internal)?
    .map_err(map_import_error)?;
    Ok(Json(ImportJobResponse {
        id: job.id.to_string(),
        workspace_id: job.workspace_id.to_string(),
        source: job.source.as_str().to_string(),
        status: job.status.as_str().to_string(),
        created_document_ids: job
            .created_refs
            .document_ids
            .iter()
            .map(|id| id.to_string())
            .collect(),
    }))
}

fn map_import_error(err: ImportDbError) -> AppError {
    match err {
        ImportDbError::NotFound | ImportDbError::Forbidden => {
            AppError::from_code(ProblemCode::NotFound)
        }
        ImportDbError::InvalidInput => AppError::from_code(ProblemCode::InvalidInput),
    }
}

async fn require_session(
    state: &AppState,
    headers: &HeaderMap,
    jar: &CookieJar,
) -> Result<(SessionUser, Uuid, Uuid), AppError> {
    let auth = crate::http::authz::require_request_auth(
        state,
        headers,
        jar,
        crate::http::authz::Access::Session,
        None,
    )
    .await?;
    Ok((auth.user, auth.user_id, auth.credential_id))
}

fn internal(err: sqlx::Error) -> AppError {
    tracing::error!("database error: {}", err);
    AppError::internal()
}
