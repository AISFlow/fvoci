use axum::extract::{DefaultBodyLimit, Path, Query, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use serde::Deserialize;
use uuid::Uuid;

use crate::api::dto::ImportJobResponse;
use crate::collab::seed::SeedEngine;
use crate::db::import_jobs::{
    create_async_import_job, create_sync_import_job, get_import_job, ImportDbError, ImportSource,
    NewAsyncImport, IMPORT_HTTP_MAX_BYTES,
};
use crate::error::{AppError, ProblemCode};
use crate::http::guard::{check_origin, reject_bearer};
use crate::http::state::AppState;
use crate::import_job::{office_format_supported, run_markdown_zip_import, SyncImportError};

/// JSON body cap: a 64 MiB file as base64 (4/3) plus room for the other fields.
pub const IMPORT_BODY_MAX_BYTES: usize = IMPORT_HTTP_MAX_BYTES / 3 * 4 + 64 * 1024;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/import",
            post(start_import).layer(DefaultBodyLimit::max(IMPORT_BODY_MAX_BYTES)),
        )
        .route("/api/v1/import/{import_job_id}", get(get_import_status))
}

/// Source `importHttpInput` (`z.strictObject`).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StartImportBody {
    workspace_id: Uuid,
    source: String,
    zip_base64: Option<String>,
    file_name: Option<String>,
    project_id: Option<Uuid>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportStatusQuery {
    workspace_id: Uuid,
}

fn invalid_input() -> AppError {
    AppError::from_code(ProblemCode::InvalidInput)
}

fn import_failed() -> AppError {
    AppError::from_code(ProblemCode::ImportFailed)
}

/// Source `parseHttpBody(…, "import")`: runs only after authentication, so an
/// anonymous caller never makes the server buffer or parse an upload.
async fn read_import_body(request: Request) -> Result<StartImportBody, AppError> {
    let content_type = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .map(|v| v.trim().to_ascii_lowercase());
    if content_type.as_deref() != Some("application/json") {
        return Err(AppError::from_code(ProblemCode::UnsupportedMediaType));
    }
    let bytes = axum::body::to_bytes(request.into_body(), IMPORT_BODY_MAX_BYTES)
        .await
        .map_err(|_| AppError::problem(StatusCode::PAYLOAD_TOO_LARGE, ProblemCode::InvalidInput))?;
    let body: StartImportBody = serde_json::from_slice(&bytes).map_err(|_| invalid_input())?;
    if let Some(name) = &body.file_name {
        let len = name.chars().count();
        if !(1..=255).contains(&len) {
            return Err(AppError::with_source(
                ProblemCode::InvalidInput,
                "/fileName",
            ));
        }
    }
    if body.zip_base64.as_deref() == Some("") {
        return Err(AppError::with_source(
            ProblemCode::InvalidInput,
            "/zipBase64",
        ));
    }
    Ok(body)
}

async fn start_import(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    request: Request,
) -> Result<Response, AppError> {
    reject_bearer(&headers)?;
    check_origin(&headers, &state.public_origin)?;
    let auth = crate::http::authz::require_request_auth(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Session,
        None,
    )
    .await?;
    let (user_id, session_id) = (auth.user_id, auth.credential_id);
    let body = read_import_body(request).await?;
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
        .ok_or_else(|| AppError::with_source(ProblemCode::InvalidInput, "/source"))?;
    let file_bytes = match &body.zip_base64 {
        Some(b64) => B64
            .decode(b64.as_bytes())
            .map_err(|_| AppError::with_source(ProblemCode::InvalidInput, "/zipBase64"))?,
        None => Vec::new(),
    };
    drop(body.zip_base64);
    if file_bytes.len() > IMPORT_HTTP_MAX_BYTES {
        return Err(AppError::problem(
            StatusCode::PAYLOAD_TOO_LARGE,
            ProblemCode::InvalidInput,
        ));
    }
    let pool = &state.auth.db.pool;

    if source == ImportSource::MarkdownZip {
        let seed = match state.collab.as_ref() {
            Some(hub) => Some(SeedEngine::from_hub(hub)),
            None => SeedEngine::from_env(),
        };
        let Some(seed) = seed else {
            tracing::error!("import.failed reason=collab_engine_unset");
            return Err(import_failed());
        };
        let Some(markdown_helper) = state.markdown.as_ref() else {
            tracing::error!("import.failed reason=markdown_helper_unset");
            return Err(import_failed());
        };
        let job = create_sync_import_job(pool, body.workspace_id, user_id, session_id)
            .await
            .map_err(internal)?
            .map_err(map_import_error)?;
        let created = match run_markdown_zip_import(
            pool,
            &seed,
            markdown_helper,
            body.workspace_id,
            job.id,
            user_id,
            session_id,
            file_bytes,
        )
        .await
        {
            Ok(ids) => ids,
            Err(SyncImportError::Failed(detail)) => {
                tracing::error!(
                    import_job_id = %job.id,
                    error_hash = %hash(&detail),
                    "import.failed"
                );
                return Err(import_failed());
            }
            Err(SyncImportError::NotFound) => {
                return Err(AppError::from_code(ProblemCode::NotFound))
            }
            Err(SyncImportError::Db(err)) => return Err(internal(err)),
        };
        return Ok(created_response(
            job.id,
            body.workspace_id,
            source,
            "completed",
            &created,
        ));
    }

    // Source `startAsyncImport`: an instance without a runner fails before any
    // membership lookup; the format and file checks come before the row.
    let Some(wake) = state.import_wake.as_ref() else {
        tracing::error!(
            source = source.as_str(),
            "import.failed reason=source_unavailable"
        );
        return Err(import_failed());
    };
    if file_bytes.is_empty() {
        return Err(import_failed());
    }
    if source == ImportSource::OfficeFile
        && !office_format_supported(
            body.file_name.as_deref().unwrap_or(""),
            state.import_extractor_available,
        )
    {
        return Err(import_failed());
    }
    let job = create_async_import_job(
        pool,
        body.workspace_id,
        user_id,
        session_id,
        source,
        NewAsyncImport {
            file_name: body.file_name.as_deref(),
            project_id: body.project_id,
            payload: &file_bytes,
        },
    )
    .await
    .map_err(internal)?
    .map_err(map_import_error)?;
    wake.notify_one();
    Ok(created_response(
        job.id,
        body.workspace_id,
        source,
        "running",
        &[],
    ))
}

fn created_response(
    id: Uuid,
    workspace_id: Uuid,
    source: ImportSource,
    status: &str,
    created: &[Uuid],
) -> Response {
    (
        StatusCode::CREATED,
        Json(ImportJobResponse {
            id: id.to_string(),
            workspace_id: workspace_id.to_string(),
            source: source.as_str().to_string(),
            status: status.to_string(),
            created_document_ids: created.iter().map(Uuid::to_string).collect(),
        }),
    )
        .into_response()
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
    let auth = crate::http::authz::require_request_auth(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Session,
        None,
    )
    .await?;
    let job = get_import_job(
        &state.auth.db.pool,
        query.workspace_id,
        auth.user_id,
        auth.credential_id,
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
            .map(Uuid::to_string)
            .collect(),
    }))
}

fn map_import_error(err: ImportDbError) -> AppError {
    match err {
        ImportDbError::NotFound | ImportDbError::Forbidden => {
            AppError::from_code(ProblemCode::NotFound)
        }
    }
}

fn hash(detail: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(detail.as_bytes()))
}

fn internal(err: sqlx::Error) -> AppError {
    tracing::error!("database error: {}", err);
    AppError::internal()
}
