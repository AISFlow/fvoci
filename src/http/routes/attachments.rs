use std::net::SocketAddr;

use axum::body::Body;
use axum::extract::rejection::JsonRejection;
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::header::{
    ACCEPT_RANGES, CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_RANGE,
    CONTENT_SECURITY_POLICY, CONTENT_TYPE, RANGE,
};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use bytes::Bytes;
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::attachments::{content_disposition_attachment, parse_range, ParsedRange};
use crate::attachments::StorageError;
use crate::auth::session::SessionUser;
use crate::db::attachments::{
    authorize_upload_part, commit_upload_part, complete_upload, create_upload, get_attachment_meta,
    open_download, resume_upload, AttachmentDbError, AttachmentRow, CreateUploadInput,
};
use crate::error::{AppError, ProblemCode, SESSION_COOKIE};
use crate::http::guard::{check_origin, reject_bearer};
use crate::http::rate_limit::peer_ip;
use crate::http::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/workspaces/{workspace_id}/documents/{document_id}/uploads",
            post(create_upload_session),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/parts/{part_number}",
            put(put_upload_part),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/upload",
            get(resume_upload_session),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/complete",
            post(complete_upload_session),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}",
            get(get_attachment),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/download",
            get(download_attachment).head(head_download),
        )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateUploadBody {
    name: String,
    size_bytes: i64,
    #[serde(default)]
    declared_mime: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PartRef {
    part_number: i32,
    etag: String,
}

#[derive(Deserialize)]
struct CompleteUploadBody {
    parts: Vec<PartRef>,
}

#[derive(Deserialize)]
struct DownloadQuery {
    variant: Option<String>,
}

fn attachment_output(att: &AttachmentRow) -> Value {
    json!({
        "id": att.id.to_string(),
        "name": att.name,
        "mime": att.mime,
        "sizeBytes": att.size_bytes,
        "image": att.image,
        "scanStatus": att.scan_status,
        "createdAt": att.created_at.to_rfc3339(),
        "completedAt": att.completed_at.map(|t| t.to_rfc3339()),
        "preview": null,
    })
}

fn part_url(workspace_id: Uuid, attachment_id: Uuid, part_number: i32) -> String {
    format!(
        "/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/parts/{part_number}"
    )
}

async fn create_upload_session(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, document_id)): Path<(Uuid, Uuid)>,
    body: Result<Json<CreateUploadBody>, JsonRejection>,
) -> Result<Response, AppError> {
    let Json(body) = body?;
    reject_bearer(&headers)?;
    check_origin(&headers, &state.public_origin)?;
    let (user, session_id) = require_session(&state, &jar).await?;
    let user_id = parse_user_id(&user.user_id)?;
    let name = body.name.trim();
    if name.is_empty() || name.len() > 255 || body.size_bytes <= 0 {
        return Err(AppError::from_code(ProblemCode::InvalidInput));
    }
    if let Some(mime) = &body.declared_mime {
        if mime.len() > 255 {
            return Err(AppError::from_code(ProblemCode::InvalidInput));
        }
    }
    let rate_key = format!("upload_create:{}", user_id);
    if let Err(retry_after) = state
        .rate_limiter
        .allow(&rate_key, state.upload.create_rate_per_5min)
        .await
    {
        return Err(AppError::rate_limited(retry_after));
    }
    let ip = peer_ip(peer.ip());
    let result = create_upload(
        &state.auth.db.pool,
        &state.storage,
        &state.upload,
        workspace_id,
        document_id,
        user_id,
        session_id,
        CreateUploadInput {
            name: name.to_string(),
            size_bytes: body.size_bytes,
            declared_mime: body.declared_mime,
        },
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok((att, meta)) => {
            let parts = (1..=meta.part_count)
                .map(|part_number| {
                    json!({
                        "partNumber": part_number,
                        "url": part_url(workspace_id, att.id, part_number),
                    })
                })
                .collect::<Vec<_>>();
            Ok((
                StatusCode::CREATED,
                Json(json!({
                    "attachmentId": att.id.to_string(),
                    "partSizeBytes": meta.part_size_bytes,
                    "parts": parts,
                })),
            )
                .into_response())
        }
        Err(AttachmentDbError::TooLarge) => {
            Err(AppError::from_code(ProblemCode::FileExceedsUploadMaxFileSizeMb))
        }
        Err(AttachmentDbError::Forbidden) | Err(AttachmentDbError::NotFound) => {
            Err(AppError::from_code(ProblemCode::NotFound))
        }
        Err(AttachmentDbError::InvalidInput) => Err(AppError::from_code(ProblemCode::InvalidInput)),
        Err(_) => Err(AppError::internal()),
    }
}

async fn put_upload_part(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, attachment_id, part_number)): Path<(Uuid, Uuid, i32)>,
    body: Body,
) -> Result<Response, AppError> {
    reject_bearer(&headers)?;
    check_origin(&headers, &state.public_origin)?;
    if part_number < 1 || part_number > 10_000 {
        return Err(AppError::from_code(ProblemCode::InvalidInput));
    }
    let (user, session_id) = require_session(&state, &jar).await?;
    let user_id = parse_user_id(&user.user_id)?;
    let auth = authorize_upload_part(
        &state.auth.db.pool,
        workspace_id,
        attachment_id,
        user_id,
        session_id,
        part_number,
    )
    .await
    .map_err(internal)?;
    let (storage_key, max_bytes) = match auth {
        Ok(v) => v,
        Err(AttachmentDbError::UploadForbidden) => {
            return Err(AppError::from_code(ProblemCode::OnlyTheUploaderMayContinueThisUpload));
        }
        Err(AttachmentDbError::UploadState) => {
            return Err(AppError::from_code(ProblemCode::UploadIsNotInTheRequiredState));
        }
        Err(AttachmentDbError::PartTooLarge) => {
            return Err(AppError::from_code(ProblemCode::PartExceedsUploadPartSizeMb));
        }
        Err(AttachmentDbError::Forbidden) | Err(AttachmentDbError::NotFound) => {
            return Err(AppError::from_code(ProblemCode::NotFound));
        }
        Err(AttachmentDbError::InvalidInput) => {
            return Err(AppError::from_code(ProblemCode::InvalidInput));
        }
        Err(_) => return Err(AppError::internal()),
    };
    let stream = body.into_data_stream().map(|r| r.map_err(|e| e));
    let staged = state
        .storage
        .stage_part_stream(&storage_key, part_number, stream, max_bytes)
        .await
        .map_err(map_storage_error)?;
    let part = commit_upload_part(
        &state.auth.db.pool,
        &state.storage,
        workspace_id,
        attachment_id,
        user_id,
        session_id,
        part_number,
        &staged,
    )
    .await
    .map_err(internal)?;
    let part = match part {
        Ok(part) => part,
        Err(AttachmentDbError::UploadForbidden) => {
            return Err(AppError::from_code(ProblemCode::OnlyTheUploaderMayContinueThisUpload));
        }
        Err(AttachmentDbError::UploadState) => {
            return Err(AppError::from_code(ProblemCode::UploadIsNotInTheRequiredState));
        }
        Err(AttachmentDbError::PartTooLarge) => {
            return Err(AppError::from_code(ProblemCode::PartExceedsUploadPartSizeMb));
        }
        Err(AttachmentDbError::Forbidden) | Err(AttachmentDbError::NotFound) => {
            return Err(AppError::from_code(ProblemCode::NotFound));
        }
        Err(AttachmentDbError::InvalidInput) => {
            return Err(AppError::from_code(ProblemCode::InvalidInput));
        }
        Err(_) => return Err(AppError::internal()),
    };
    let _ip = peer_ip(peer.ip());
    Ok((
        StatusCode::OK,
        [(axum::http::header::ETAG, part.etag.clone())],
        Json(json!({ "etag": part.etag })),
    )
        .into_response())
}

async fn resume_upload_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, attachment_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Value>, AppError> {
    reject_bearer(&headers)?;
    let (user, session_id) = require_session(&state, &jar).await?;
    let user_id = parse_user_id(&user.user_id)?;
    let result = resume_upload(
        &state.auth.db.pool,
        &state.storage,
        workspace_id,
        attachment_id,
        user_id,
        session_id,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok((att, meta, uploaded, remaining)) => {
            let parts = remaining
                .into_iter()
                .map(|part_number| {
                    json!({
                        "partNumber": part_number,
                        "url": part_url(workspace_id, att.id, part_number),
                    })
                })
                .collect::<Vec<_>>();
            let uploaded_parts = uploaded
                .into_iter()
                .map(|(part_number, etag)| json!({ "partNumber": part_number, "etag": etag }))
                .collect::<Vec<_>>();
            Ok(Json(json!({
                "attachmentId": att.id.to_string(),
                "partSizeBytes": meta.part_size_bytes,
                "uploadedParts": uploaded_parts,
                "parts": parts,
            })))
        }
        Err(AttachmentDbError::UploadForbidden) => {
            Err(AppError::from_code(ProblemCode::OnlyTheUploaderMayContinueThisUpload))
        }
        Err(AttachmentDbError::UploadState) => {
            Err(AppError::from_code(ProblemCode::UploadIsNotInTheRequiredState))
        }
        Err(AttachmentDbError::Forbidden) | Err(AttachmentDbError::NotFound) => {
            Err(AppError::from_code(ProblemCode::NotFound))
        }
        Err(_) => Err(AppError::internal()),
    }
}

async fn complete_upload_session(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, attachment_id)): Path<(Uuid, Uuid)>,
    body: Result<Json<CompleteUploadBody>, JsonRejection>,
) -> Result<Json<Value>, AppError> {
    let Json(body) = body?;
    reject_bearer(&headers)?;
    check_origin(&headers, &state.public_origin)?;
    if body.parts.is_empty() || body.parts.len() > 10_000 {
        return Err(AppError::from_code(ProblemCode::InvalidInput));
    }
    let (user, session_id) = require_session(&state, &jar).await?;
    let user_id = parse_user_id(&user.user_id)?;
    let parts = body
        .parts
        .into_iter()
        .map(|p| (p.part_number, p.etag))
        .collect::<Vec<_>>();
    let ip = peer_ip(peer.ip());
    let result = complete_upload(
        &state.auth.db.pool,
        &state.storage,
        workspace_id,
        attachment_id,
        user_id,
        session_id,
        parts,
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(att) => Ok(Json(attachment_output(&att))),
        Err(AttachmentDbError::UploadForbidden) => {
            Err(AppError::from_code(ProblemCode::OnlyTheUploaderMayContinueThisUpload))
        }
        Err(AttachmentDbError::UploadState) => {
            Err(AppError::from_code(ProblemCode::UploadIsNotInTheRequiredState))
        }
        Err(AttachmentDbError::EtagMismatch) => Err(AppError::from_code(
            ProblemCode::SubmittedPartsDoNotMatchUploadedParts,
        )),
        Err(AttachmentDbError::Forbidden) | Err(AttachmentDbError::NotFound) => {
            Err(AppError::from_code(ProblemCode::NotFound))
        }
        Err(AttachmentDbError::InvalidInput) => Err(AppError::from_code(ProblemCode::InvalidInput)),
        Err(_) => Err(AppError::internal()),
    }
}

async fn get_attachment(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, attachment_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Value>, AppError> {
    reject_bearer(&headers)?;
    let (user, session_id) = require_session(&state, &jar).await?;
    let user_id = parse_user_id(&user.user_id)?;
    let result = get_attachment_meta(
        &state.auth.db.pool,
        workspace_id,
        attachment_id,
        user_id,
        session_id,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(att) => Ok(Json(attachment_output(&att))),
        Err(AttachmentDbError::Forbidden) | Err(AttachmentDbError::NotFound) => {
            Err(AppError::from_code(ProblemCode::NotFound))
        }
        Err(_) => Err(AppError::internal()),
    }
}

async fn download_attachment(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, attachment_id)): Path<(Uuid, Uuid)>,
    Query(query): Query<DownloadQuery>,
) -> Result<Response, AppError> {
    serve_download(&state, &headers, &jar, workspace_id, attachment_id, &query, false).await
}

async fn head_download(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, attachment_id)): Path<(Uuid, Uuid)>,
    Query(query): Query<DownloadQuery>,
) -> Result<Response, AppError> {
    serve_download(&state, &headers, &jar, workspace_id, attachment_id, &query, true).await
}

async fn serve_download(
    state: &AppState,
    headers: &HeaderMap,
    jar: &CookieJar,
    workspace_id: Uuid,
    attachment_id: Uuid,
    query: &DownloadQuery,
    head_only: bool,
) -> Result<Response, AppError> {
    reject_bearer(headers)?;
    if query.variant.as_deref() == Some("preview") {
        return Err(AppError::from_code(ProblemCode::NotFound));
    }
    if query.variant.is_some() {
        return Err(AppError::from_code(ProblemCode::InvalidInput));
    }
    let (user, session_id) = require_session(state, jar).await?;
    let user_id = parse_user_id(&user.user_id)?;
    let result = open_download(
        &state.auth.db.pool,
        workspace_id,
        attachment_id,
        user_id,
        session_id,
    )
    .await
    .map_err(internal)?;
    let att = match result {
        Ok(att) => att,
        Err(AttachmentDbError::Infected) => {
            return Err(AppError::from_code(ProblemCode::AttachmentFailedVirusScan));
        }
        Err(AttachmentDbError::Forbidden) | Err(AttachmentDbError::NotFound) => {
            return Err(AppError::from_code(ProblemCode::NotFound));
        }
        Err(_) => return Err(AppError::internal()),
    };
    let size = att.size_bytes.ok_or_else(AppError::internal)?;
    let range_header = headers.get(RANGE).and_then(|v| v.to_str().ok());
    let parsed = parse_range(range_header, size as u64);
    let mut response_headers = HeaderMap::new();
    response_headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"));
    response_headers.insert(
        CONTENT_DISPOSITION,
        HeaderValue::from_str(&content_disposition_attachment(&att.name))
            .map_err(|_| AppError::internal())?,
    );
    response_headers.insert(ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    response_headers.insert(CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    response_headers.insert(
        axum::http::header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response_headers.insert(
        CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("sandbox"),
    );

    match parsed {
        ParsedRange::Invalid => {
            response_headers.insert(
                CONTENT_RANGE,
                HeaderValue::from_str(&format!("bytes */{}", size))
                    .map_err(|_| AppError::internal())?,
            );
            return Err(AppError::from_code(ProblemCode::RangeNotSatisfiable));
        }
        ParsedRange::Full => {
            response_headers.insert(
                CONTENT_LENGTH,
                HeaderValue::from_str(&size.to_string()).map_err(|_| AppError::internal())?,
            );
            if head_only {
                return Ok((StatusCode::OK, response_headers).into_response());
            }
            let bytes = state
                .storage
                .read_range(&att.storage_key, 0, (size - 1) as u64)
                .await
                .map_err(|_| AppError::internal())?;
            return Ok((StatusCode::OK, response_headers, Bytes::from(bytes)).into_response());
        }
        ParsedRange::Bytes { start, end } => {
            let len = end - start + 1;
            response_headers.insert(
                CONTENT_RANGE,
                HeaderValue::from_str(&format!("bytes {}-{}/{}", start, end, size))
                    .map_err(|_| AppError::internal())?,
            );
            response_headers.insert(
                CONTENT_LENGTH,
                HeaderValue::from_str(&len.to_string()).map_err(|_| AppError::internal())?,
            );
            if head_only {
                return Ok((StatusCode::PARTIAL_CONTENT, response_headers).into_response());
            }
            let bytes = state
                .storage
                .read_range(&att.storage_key, start, end)
                .await
                .map_err(|_| AppError::internal())?;
            return Ok((
                StatusCode::PARTIAL_CONTENT,
                response_headers,
                Bytes::from(bytes),
            )
                .into_response());
        }
    }
}

fn map_storage_error(err: StorageError) -> AppError {
    match err {
        StorageError::PartTooLarge => AppError::from_code(ProblemCode::PartExceedsUploadPartSizeMb),
        StorageError::UploadGone | StorageError::InvalidKey => {
            AppError::from_code(ProblemCode::UploadIsNotInTheRequiredState)
        }
        StorageError::EtagMismatch => {
            AppError::from_code(ProblemCode::SubmittedPartsDoNotMatchUploadedParts)
        }
        _ => AppError::internal(),
    }
}

async fn require_session(state: &AppState, jar: &CookieJar) -> Result<(SessionUser, Uuid), AppError> {
    let token = jar
        .get(SESSION_COOKIE)
        .map(|c| c.value().to_string())
        .ok_or_else(|| AppError::from_code(ProblemCode::AuthenticationRequired))?;
    let user = state
        .auth
        .session_user(&token)
        .await
        .map_err(internal)?
        .ok_or_else(|| AppError::from_code(ProblemCode::AuthenticationRequired))?;
    let session_id = Uuid::parse_str(&user.session_id)
        .map_err(|_| AppError::from_code(ProblemCode::AuthenticationRequired))?;
    Ok((user, session_id))
}

fn parse_user_id(value: &str) -> Result<Uuid, AppError> {
    Uuid::parse_str(value).map_err(|_| AppError::from_code(ProblemCode::AuthenticationRequired))
}

fn internal(err: sqlx::Error) -> AppError {
    tracing::error!("database error: {}", err);
    AppError::internal()
}
