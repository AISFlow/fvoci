use std::net::SocketAddr;

use axum::body::Body;
use axum::extract::rejection::{JsonRejection, QueryRejection};
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
use uuid::Uuid;

use crate::api::dto::{
    AttachmentCompletePartBody, AttachmentDownloadQuery, AttachmentOutput,
    AttachmentPartUrlResponse, AttachmentUploadedPartResponse, CompleteAttachmentUploadBody,
    CreateAttachmentUploadBody, CreateAttachmentUploadResponse, PutAttachmentPartResponse,
    ResumeAttachmentUploadResponse,
};
use crate::attachments::StorageError;
use crate::attachments::{content_disposition_attachment, parse_range, ParsedRange};
use crate::auth::session::SessionUser;
use crate::db::attachments::{
    authorize_upload_part, commit_upload_part, complete_upload, create_upload, get_attachment_meta,
    open_download, resume_upload, AttachmentDbError, AttachmentRow, CreateUploadInput,
};
use crate::error::{AppError, ProblemCode};
use crate::http::guard::check_origin;
use crate::http::rate_limit::peer_ip;
use crate::http::state::AppState;
use crate::validate::utf16_len;

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

fn attachment_output(att: &AttachmentRow) -> AttachmentOutput {
    AttachmentOutput {
        id: att.id.to_string(),
        name: att.name.clone(),
        mime: att.mime.clone(),
        size_bytes: att.size_bytes,
        image: att.image,
        scan_status: att.scan_status.clone(),
        created_at: att.created_at,
        completed_at: att.completed_at,
        preview: None,
    }
}

fn validate_create_upload(body: &CreateAttachmentUploadBody) -> Result<(), AppError> {
    if body.name.is_empty() || utf16_len(&body.name) > 255 || body.size_bytes <= 0 {
        return Err(AppError::from_code(ProblemCode::InvalidInput));
    }
    if let Some(mime) = &body.declared_mime {
        if utf16_len(mime) > 255 {
            return Err(AppError::from_code(ProblemCode::InvalidInput));
        }
    }
    Ok(())
}

fn validate_complete_parts(parts: &[AttachmentCompletePartBody]) -> Result<(), AppError> {
    if parts.is_empty() || parts.len() > 10_000 {
        return Err(AppError::from_code(ProblemCode::InvalidInput));
    }
    for part in parts {
        if !(1..=10_000).contains(&part.part_number) {
            return Err(AppError::from_code(ProblemCode::InvalidInput));
        }
        if part.etag.is_empty() || utf16_len(&part.etag) > 128 {
            return Err(AppError::from_code(ProblemCode::InvalidInput));
        }
    }
    Ok(())
}

fn original_download_or_error(query: &AttachmentDownloadQuery) -> Result<(), AppError> {
    match query.variant.as_deref() {
        None => Ok(()),
        Some("preview") => Err(AppError::from_code(ProblemCode::NotFound)),
        Some(_) => Err(AppError::from_code(ProblemCode::InvalidInput)),
    }
}

fn part_url(workspace_id: Uuid, attachment_id: Uuid, part_number: i32) -> String {
    format!("/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/parts/{part_number}")
}

async fn create_upload_session(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, document_id)): Path<(Uuid, Uuid)>,
    body: Result<Json<CreateAttachmentUploadBody>, JsonRejection>,
) -> Result<Response, AppError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    validate_create_upload(&body)?;
    let (_user, user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::DocumentsWrite),
        Some(workspace_id),
    )
    .await?;
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
            name: body.name,
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
                .map(|part_number| AttachmentPartUrlResponse {
                    part_number,
                    url: part_url(workspace_id, att.id, part_number),
                })
                .collect::<Vec<_>>();
            Ok((
                StatusCode::CREATED,
                Json(CreateAttachmentUploadResponse {
                    attachment_id: att.id.to_string(),
                    part_size_bytes: meta.part_size_bytes,
                    parts,
                }),
            )
                .into_response())
        }
        Err(AttachmentDbError::TooLarge) => Err(AppError::from_code(
            ProblemCode::FileExceedsUploadMaxFileSizeMb,
        )),
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
    check_origin(&headers, &state.public_origin)?;
    if !(1..=10_000).contains(&part_number) {
        return Err(AppError::from_code(ProblemCode::InvalidInput));
    }
    let (_user, user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Any,
        Some(workspace_id),
    )
    .await?;
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
    let (storage_key, max_bytes, upload_ref) = match auth {
        Ok(v) => v,
        Err(AttachmentDbError::UploadForbidden) => {
            return Err(AppError::from_code(
                ProblemCode::OnlyTheUploaderMayContinueThisUpload,
            ));
        }
        Err(AttachmentDbError::UploadState) => {
            return Err(AppError::from_code(
                ProblemCode::UploadIsNotInTheRequiredState,
            ));
        }
        Err(AttachmentDbError::PartTooLarge) => {
            return Err(AppError::from_code(
                ProblemCode::PartExceedsUploadPartSizeMb,
            ));
        }
        Err(AttachmentDbError::Forbidden) | Err(AttachmentDbError::NotFound) => {
            return Err(AppError::from_code(ProblemCode::NotFound));
        }
        Err(AttachmentDbError::InvalidInput) => {
            return Err(AppError::from_code(ProblemCode::InvalidInput));
        }
        Err(_) => return Err(AppError::internal()),
    };
    // Held until the part is committed; refused before the body is read.
    let Some(_slot) = state.upload.part_put_slots.try_acquire(user_id) else {
        return Err(AppError::upload_capacity_exceeded(
            PART_SLOT_RETRY_AFTER_SECS,
        ));
    };
    let declared_len = declared_body_length(&headers, &body)?;
    let stream = body.into_data_stream();
    // Bounds how long a paced body can hold its slot, on every driver.
    let body_deadline = state
        .upload
        .part_put_slots
        .body_deadline(declared_len.unwrap_or(max_bytes));
    let mut staged = tokio::time::timeout(
        body_deadline,
        state.storage.stage_part_stream(
            &storage_key,
            upload_ref.as_deref(),
            part_number,
            stream,
            declared_len,
            max_bytes,
        ),
    )
    .await
    .map_err(|_| AppError::from_code(ProblemCode::InvalidInput))?
    .map_err(map_storage_error)?;
    #[cfg(feature = "db-tests")]
    crate::db::attachments::test_barrier::wait_pre_publish_barrier(attachment_id).await;
    let part = commit_upload_part(
        &state.auth.db.pool,
        &state.storage,
        workspace_id,
        attachment_id,
        user_id,
        session_id,
        part_number,
        &mut staged,
    )
    .await
    .map_err(internal)?;
    let part = match part {
        Ok(part) => part,
        Err(AttachmentDbError::UploadForbidden) => {
            return Err(AppError::from_code(
                ProblemCode::OnlyTheUploaderMayContinueThisUpload,
            ));
        }
        Err(AttachmentDbError::UploadState) => {
            return Err(AppError::from_code(
                ProblemCode::UploadIsNotInTheRequiredState,
            ));
        }
        Err(AttachmentDbError::PartTooLarge) => {
            return Err(AppError::from_code(
                ProblemCode::PartExceedsUploadPartSizeMb,
            ));
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
        Json(PutAttachmentPartResponse { etag: part.etag }),
    )
        .into_response())
}

async fn resume_upload_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, attachment_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<ResumeAttachmentUploadResponse>, AppError> {
    let (_user, user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Any,
        Some(workspace_id),
    )
    .await?;
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
                .map(|part_number| AttachmentPartUrlResponse {
                    part_number,
                    url: part_url(workspace_id, att.id, part_number),
                })
                .collect::<Vec<_>>();
            let uploaded_parts = uploaded
                .into_iter()
                .map(|(part_number, etag)| AttachmentUploadedPartResponse { part_number, etag })
                .collect::<Vec<_>>();
            Ok(Json(ResumeAttachmentUploadResponse {
                attachment_id: att.id.to_string(),
                part_size_bytes: meta.part_size_bytes,
                uploaded_parts,
                parts,
            }))
        }
        Err(AttachmentDbError::UploadForbidden) => Err(AppError::from_code(
            ProblemCode::OnlyTheUploaderMayContinueThisUpload,
        )),
        Err(AttachmentDbError::UploadState) => Err(AppError::from_code(
            ProblemCode::UploadIsNotInTheRequiredState,
        )),
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
    body: Result<Json<CompleteAttachmentUploadBody>, JsonRejection>,
) -> Result<Json<AttachmentOutput>, AppError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    validate_complete_parts(&body.parts)?;
    let (_user, user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Any,
        Some(workspace_id),
    )
    .await?;
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
        Err(AttachmentDbError::UploadForbidden) => Err(AppError::from_code(
            ProblemCode::OnlyTheUploaderMayContinueThisUpload,
        )),
        Err(AttachmentDbError::UploadState) => Err(AppError::from_code(
            ProblemCode::UploadIsNotInTheRequiredState,
        )),
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
) -> Result<Json<AttachmentOutput>, AppError> {
    let (_user, user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Any,
        Some(workspace_id),
    )
    .await?;
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
    query: Result<Query<AttachmentDownloadQuery>, QueryRejection>,
) -> Result<Response, AppError> {
    let Query(query) = query.map_err(AppError::from)?;
    serve_download(
        &state,
        &headers,
        &jar,
        workspace_id,
        attachment_id,
        &query,
        false,
    )
    .await
}

async fn head_download(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, attachment_id)): Path<(Uuid, Uuid)>,
    query: Result<Query<AttachmentDownloadQuery>, QueryRejection>,
) -> Result<Response, AppError> {
    let Query(query) = query.map_err(AppError::from)?;
    serve_download(
        &state,
        &headers,
        &jar,
        workspace_id,
        attachment_id,
        &query,
        true,
    )
    .await
}

async fn serve_download(
    state: &AppState,
    headers: &HeaderMap,
    jar: &CookieJar,
    workspace_id: Uuid,
    attachment_id: Uuid,
    query: &AttachmentDownloadQuery,
    head_only: bool,
) -> Result<Response, AppError> {
    original_download_or_error(query)?;
    let (_user, user_id, session_id) = require_session(
        state,
        headers,
        jar,
        crate::http::authz::Access::Session,
        None,
    )
    .await?;
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
    response_headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
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
    response_headers.insert(CONTENT_SECURITY_POLICY, HeaderValue::from_static("sandbox"));

    match parsed {
        ParsedRange::Invalid => {
            let mut resp = AppError::from_code(ProblemCode::RangeNotSatisfiable).into_response();
            let headers = resp.headers_mut();
            headers.insert(
                CONTENT_RANGE,
                HeaderValue::from_str(&format!("bytes */{}", size))
                    .map_err(|_| AppError::internal())?,
            );
            for (name, value) in response_headers.iter() {
                if name != CONTENT_TYPE {
                    headers.insert(name, value.clone());
                }
            }
            Ok(resp)
        }
        ParsedRange::Full => {
            response_headers.insert(
                CONTENT_LENGTH,
                HeaderValue::from_str(&size.to_string()).map_err(|_| AppError::internal())?,
            );
            if head_only {
                return Ok((StatusCode::OK, response_headers).into_response());
            }
            let file = state
                .storage
                .open_payload_stream(&att.storage_key, 0, (size as u64).saturating_sub(1))
                .await
                .map_err(|_| AppError::internal())?;
            Ok((StatusCode::OK, response_headers, Body::from_stream(file)).into_response())
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
            let stream = state
                .storage
                .open_payload_stream(&att.storage_key, start, end)
                .await
                .map_err(|_| AppError::internal())?;
            Ok((
                StatusCode::PARTIAL_CONTENT,
                response_headers,
                Body::from_stream(stream),
            )
                .into_response())
        }
    }
}

const PART_SLOT_RETRY_AFTER_SECS: u32 = 2;

/// The part's declared length: `Content-Length`, or the exact size the body
/// already knows (e.g. a buffered body). A malformed header is rejected; no
/// declared length at all (chunked) yields `None`, which the S3 driver refuses.
fn declared_body_length(headers: &HeaderMap, body: &Body) -> Result<Option<u64>, AppError> {
    match headers.get(CONTENT_LENGTH) {
        Some(value) => value
            .to_str()
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .map(Some)
            .ok_or_else(|| AppError::from_code(ProblemCode::InvalidInput)),
        None => Ok(axum::body::HttpBody::size_hint(body).exact()),
    }
}

fn map_storage_error(err: StorageError) -> AppError {
    match err {
        StorageError::PartTooLarge => AppError::from_code(ProblemCode::PartExceedsUploadPartSizeMb),
        StorageError::UploadGone | StorageError::InvalidKey => {
            AppError::from_code(ProblemCode::UploadIsNotInTheRequiredState)
        }
        StorageError::EtagMismatch | StorageError::PartTooSmall => {
            AppError::from_code(ProblemCode::SubmittedPartsDoNotMatchUploadedParts)
        }
        // A body that broke off mid-stream is the client's failure: answer
        // 400 (usually unseen) instead of logging a server error.
        StorageError::LengthRequired | StorageError::LengthMismatch | StorageError::ClientBody => {
            AppError::from_code(ProblemCode::InvalidInput)
        }
        _ => AppError::internal(),
    }
}

async fn require_session(
    state: &AppState,
    headers: &HeaderMap,
    jar: &CookieJar,
    access: crate::http::authz::Access,
    workspace_id: Option<Uuid>,
) -> Result<(SessionUser, Uuid, Uuid), AppError> {
    let auth =
        crate::http::authz::require_request_auth(state, headers, jar, access, workspace_id).await?;
    Ok((auth.user, auth.user_id, auth.credential_id))
}

fn internal(err: sqlx::Error) -> AppError {
    tracing::error!("database error: {}", err);
    AppError::internal()
}
