use std::net::SocketAddr;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::rejection::QueryRejection;
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::api::dto::{
    RevisionCreateResponse, RevisionDetailResponse, RevisionListResponse, RevisionMetaResponse,
    RevisionRestoreBody, RevisionRestoreResponse,
};
use crate::auth::session::SessionUser;
use crate::collab::revision::{capture_revision_offline, prepare_revision_text};
use crate::collab::room::{CapturedRevision, RevisionCaptureError, RevisionRestoreError};
use crate::db::revisions::{
    create_manual_document_revision, decode_revision_cursor, get_document_revision,
    list_document_revisions, load_persisted_collab_source, resolve_document_restore,
    CreateRevisionInput, RevisionDbError, RevisionDetail, RevisionMeta,
};
use crate::error::{AppError, ProblemCode, SESSION_COOKIE};
use crate::http::guard::{check_origin, reject_bearer};
use crate::http::rate_limit::{peer_ip, REVISION_WRITE_LIMIT, REVISION_WRITE_WINDOW};
use crate::http::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/workspaces/{workspace_id}/documents/{document_id}/revisions",
            get(list_revisions).post(create_revision),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/documents/{document_id}/revisions/{revision_id}",
            get(get_revision),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/documents/{document_id}/revisions/{revision_id}/restore",
            post(restore_revision),
        )
}

#[derive(Deserialize)]
struct RevisionListQuery {
    limit: Option<i64>,
    cursor: Option<String>,
}

enum RevisionApiError {
    App(AppError),
    Coded {
        status: StatusCode,
        code: &'static str,
        title: String,
        params: Option<Value>,
    },
}

impl From<AppError> for RevisionApiError {
    fn from(value: AppError) -> Self {
        Self::App(value)
    }
}

impl IntoResponse for RevisionApiError {
    fn into_response(self) -> Response {
        match self {
            Self::App(err) => err.into_response(),
            Self::Coded {
                status,
                code,
                title,
                params,
            } => {
                let mut body = json!({
                    "type": "about:blank",
                    "title": title,
                    "status": status.as_u16(),
                    "code": code,
                });
                if let Some(params) = params {
                    body["params"] = params;
                }
                let mut headers = HeaderMap::new();
                headers.insert(
                    axum::http::header::CONTENT_TYPE,
                    HeaderValue::from_static("application/problem+json"),
                );
                (status, headers, Json(body)).into_response()
            }
        }
    }
}

async fn create_revision(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, document_id)): Path<(Uuid, Uuid)>,
) -> Result<Response, RevisionApiError> {
    reject_bearer(&headers)?;
    check_origin(&headers, &state.public_origin)?;
    let (user, session_id) = require_session(&state, &jar).await?;
    let user_id = parse_user_id(&user.user_id)?;
    let _ = peer_ip(peer.ip());
    if let Err(retry_after) = state
        .rate_limiter
        .allow_window(
            &format!("revision-write:{user_id}"),
            REVISION_WRITE_LIMIT,
            REVISION_WRITE_WINDOW,
        )
        .await
    {
        return Err(AppError::rate_limited(retry_after).into());
    }
    let captured =
        capture_for_create(&state, workspace_id, user_id, session_id, document_id).await?;
    let text = prepare_revision_text(&captured.content_json).map_err(|_| collab_unavailable())?;
    let result = create_manual_document_revision(
        &state.auth.db.pool,
        workspace_id,
        user_id,
        session_id,
        document_id,
        CreateRevisionInput {
            y_snapshot: captured.y_snapshot,
            content_json: captured.content_json,
            text,
            reason: "manual".into(),
        },
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(id) => Ok((
            StatusCode::CREATED,
            Json(RevisionCreateResponse { id: id.to_string() }),
        )
            .into_response()),
        Err(err) => Err(map_revision_error(err)),
    }
}

async fn list_revisions(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, document_id)): Path<(Uuid, Uuid)>,
    query: Result<Query<RevisionListQuery>, QueryRejection>,
) -> Result<Json<RevisionListResponse>, RevisionApiError> {
    reject_bearer(&headers)?;
    let Query(query) = query.map_err(AppError::from)?;
    let limit = query.limit.unwrap_or(50);
    if !(1..=100).contains(&limit) {
        return Err(AppError::from_code(ProblemCode::InvalidInput).into());
    }
    let before = match query.cursor.as_deref() {
        None => None,
        Some(raw) => Some(
            decode_revision_cursor(raw)
                .ok_or_else(|| AppError::from_code(ProblemCode::InvalidInput))?,
        ),
    };
    let (user, session_id) = require_session(&state, &jar).await?;
    let user_id = parse_user_id(&user.user_id)?;
    let result = list_document_revisions(
        &state.auth.db.pool,
        workspace_id,
        user_id,
        session_id,
        document_id,
        limit,
        before,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(page) => Ok(Json(RevisionListResponse {
            items: page.items.into_iter().map(meta_response).collect(),
            next_cursor: page.next_cursor,
        })),
        Err(err) => Err(map_revision_error(err)),
    }
}

async fn get_revision(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, document_id, revision_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<Json<RevisionDetailResponse>, RevisionApiError> {
    reject_bearer(&headers)?;
    let (user, session_id) = require_session(&state, &jar).await?;
    let user_id = parse_user_id(&user.user_id)?;
    let result = get_document_revision(
        &state.auth.db.pool,
        workspace_id,
        user_id,
        session_id,
        document_id,
        revision_id,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(detail) => Ok(Json(detail_response(detail))),
        Err(err) => Err(map_revision_error(err)),
    }
}

async fn restore_revision(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, document_id, revision_id)): Path<(Uuid, Uuid, Uuid)>,
    body: Bytes,
) -> Result<Json<RevisionRestoreResponse>, RevisionApiError> {
    reject_bearer(&headers)?;
    check_origin(&headers, &state.public_origin)?;
    let _restore_body: RevisionRestoreBody = if body.is_empty() {
        RevisionRestoreBody {
            correlation_id: None,
        }
    } else {
        serde_json::from_slice(&body).map_err(|_| AppError::from_code(ProblemCode::InvalidInput))?
    };
    let (user, session_id) = require_session(&state, &jar).await?;
    let user_id = parse_user_id(&user.user_id)?;
    let _ = peer_ip(peer.ip());
    if let Err(retry_after) = state
        .rate_limiter
        .allow_window(
            &format!("revision-write:{user_id}"),
            REVISION_WRITE_LIMIT,
            REVISION_WRITE_WINDOW,
        )
        .await
    {
        return Err(AppError::rate_limited(retry_after).into());
    }
    let snap = resolve_document_restore(
        &state.auth.db.pool,
        workspace_id,
        user_id,
        session_id,
        document_id,
        revision_id,
        None,
    )
    .await
    .map_err(internal)?;
    let snap = match snap {
        Ok(bytes) => bytes,
        Err(err) => return Err(map_revision_error(err)),
    };
    let Some(hub) = state.collab.clone() else {
        return Err(collab_unavailable());
    };
    let timeout = hub.rpc_timeout();
    let restore = hub.restore_revision((workspace_id, document_id), user_id, session_id, snap);
    match tokio::time::timeout(timeout.max(Duration::from_millis(1)), restore).await {
        Ok(Ok(())) => Ok(Json(RevisionRestoreResponse { restored: true })),
        Ok(Err(RevisionRestoreError::Rejected)) => {
            Err(AppError::from_code(ProblemCode::RestoreRejected).into())
        }
        Ok(Err(RevisionRestoreError::Unavailable)) => Err(collab_unavailable()),
        Err(_) => Err(AppError::from_code(ProblemCode::CollabTimeoutRetry).into()),
    }
}

async fn capture_for_create(
    state: &AppState,
    workspace_id: Uuid,
    user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
) -> Result<CapturedRevision, RevisionApiError> {
    if let Some(hub) = state.collab.as_ref() {
        if let Some(live) = hub.capture_if_live((workspace_id, document_id)).await {
            return live.map_err(|_| collab_unavailable());
        }
    }
    let persisted = load_persisted_collab_source(
        &state.auth.db.pool,
        workspace_id,
        user_id,
        session_id,
        document_id,
    )
    .await
    .map_err(internal)?;
    let persisted = match persisted {
        Ok(source) => source,
        Err(err) => return Err(map_revision_error(err)),
    };
    let (engine_bin, limits) = match state.collab.as_ref() {
        Some(hub) => (hub.engine_bin(), hub.limits()),
        None => {
            let cfg = crate::collab::CollabConfig::from_env().ok_or_else(collab_unavailable)?;
            (cfg.engine_bin, cfg.limits)
        }
    };
    tokio::task::spawn_blocking(move || {
        capture_revision_offline(engine_bin, limits, persisted.snapshot, persisted.tail)
    })
    .await
    .map_err(|_| collab_unavailable())?
    .map_err(|err| match err {
        RevisionCaptureError::Unavailable => collab_unavailable(),
    })
}

fn meta_response(meta: RevisionMeta) -> RevisionMetaResponse {
    RevisionMetaResponse {
        id: meta.id.to_string(),
        target_kind: meta.target_kind,
        target_id: meta.target_id.to_string(),
        reason: meta.reason,
        created_by: meta.created_by.map(|id| id.to_string()),
        created_at: meta.created_at,
    }
}

fn detail_response(detail: RevisionDetail) -> RevisionDetailResponse {
    RevisionDetailResponse {
        id: detail.meta.id.to_string(),
        target_kind: detail.meta.target_kind,
        target_id: detail.meta.target_id.to_string(),
        reason: detail.meta.reason,
        created_by: detail.meta.created_by.map(|id| id.to_string()),
        created_at: detail.meta.created_at,
        content_json: detail.content_json,
        y_snapshot: collab_engine::b64::encode(&detail.y_snapshot),
    }
}

fn map_revision_error(err: RevisionDbError) -> RevisionApiError {
    match err {
        RevisionDbError::NotFound | RevisionDbError::Forbidden => {
            AppError::from_code(ProblemCode::NotFound).into()
        }
    }
}

fn collab_unavailable() -> RevisionApiError {
    RevisionApiError::Coded {
        status: StatusCode::SERVICE_UNAVAILABLE,
        code: "collab_unavailable",
        title: "collab unavailable".into(),
        params: None,
    }
}

async fn require_session(
    state: &AppState,
    jar: &CookieJar,
) -> Result<(SessionUser, Uuid), AppError> {
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
