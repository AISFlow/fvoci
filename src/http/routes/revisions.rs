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
use crate::auth::scopes::ApiTokenScope;
use crate::auth::session::SessionUser;
use crate::collab::revision::{capture_revision_offline, prepare_revision_text};
use crate::collab::room::RoomKey;
use crate::collab::room::{CapturedRevision, RevisionCaptureError, RevisionRestoreError};
use crate::db::revisions::{
    authorize_revision_target, create_manual_revision, decode_revision_cursor,
    get_revision as get_revision_for, list_revisions as list_revisions_for,
    load_persisted_target_source, resolve_restore, CreateRevisionInput, RevisionDbError,
    RevisionDetail, RevisionMeta, RevisionTarget,
};
use crate::error::{AppError, ProblemCode, SESSION_COOKIE};
use crate::http::authz::{require_request_auth, Access};
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
        .route(
            "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/revisions",
            get(list_task_revisions).post(create_task_revision),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/revisions/{revision_id}",
            get(get_task_revision),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/revisions/{revision_id}/restore",
            post(restore_task_revision),
        )
}

fn room_key(workspace_id: Uuid, target: RevisionTarget) -> RoomKey {
    match target {
        RevisionTarget::Document(id) => RoomKey::document(workspace_id, id),
        RevisionTarget::Task(id) => RoomKey::task(workspace_id, id),
    }
}

async fn create_task_revision(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, task_id)): Path<(Uuid, Uuid)>,
) -> Result<Response, RevisionApiError> {
    create_target_revision(
        state,
        peer,
        headers,
        jar,
        workspace_id,
        RevisionTarget::Task(task_id),
    )
    .await
}

async fn list_task_revisions(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, task_id)): Path<(Uuid, Uuid)>,
    query: Result<Query<RevisionListQuery>, QueryRejection>,
) -> Result<Json<RevisionListResponse>, RevisionApiError> {
    list_target_revisions(
        state,
        headers,
        jar,
        workspace_id,
        RevisionTarget::Task(task_id),
        query,
    )
    .await
}

async fn get_task_revision(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, task_id, revision_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<Json<RevisionDetailResponse>, RevisionApiError> {
    get_target_revision(
        state,
        headers,
        jar,
        workspace_id,
        RevisionTarget::Task(task_id),
        revision_id,
    )
    .await
}

async fn restore_task_revision(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, task_id, revision_id)): Path<(Uuid, Uuid, Uuid)>,
    body: Bytes,
) -> Result<Json<RevisionRestoreResponse>, RevisionApiError> {
    restore_target_revision(
        state,
        peer,
        headers,
        jar,
        workspace_id,
        RevisionTarget::Task(task_id),
        revision_id,
        body,
    )
    .await
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
    create_target_revision(
        state,
        peer,
        headers,
        jar,
        workspace_id,
        RevisionTarget::Document(document_id),
    )
    .await
}

async fn create_target_revision(
    state: AppState,
    peer: SocketAddr,
    headers: HeaderMap,
    jar: CookieJar,
    workspace_id: Uuid,
    target: RevisionTarget,
) -> Result<Response, RevisionApiError> {
    check_origin(&headers, &state.public_origin)?;
    let (user_id, credential_id) =
        revision_credential(&state, &headers, &jar, workspace_id, target, true).await?;
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
    match authorize_revision_target(
        &state.auth.db.pool,
        workspace_id,
        user_id,
        credential_id,
        target,
        true,
    )
    .await
    .map_err(internal)?
    {
        Ok(()) => {}
        Err(err) => return Err(map_revision_error(err)),
    }
    let captured = capture_for_create(&state, workspace_id, user_id, credential_id, target).await?;
    let text = prepare_revision_text(&captured.content_json).map_err(|_| collab_unavailable())?;
    let result = create_manual_revision(
        &state.auth.db.pool,
        workspace_id,
        user_id,
        credential_id,
        target,
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
    list_target_revisions(
        state,
        headers,
        jar,
        workspace_id,
        RevisionTarget::Document(document_id),
        query,
    )
    .await
}

async fn list_target_revisions(
    state: AppState,
    headers: HeaderMap,
    jar: CookieJar,
    workspace_id: Uuid,
    target: RevisionTarget,
    query: Result<Query<RevisionListQuery>, QueryRejection>,
) -> Result<Json<RevisionListResponse>, RevisionApiError> {
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
    let (user_id, credential_id) =
        revision_credential(&state, &headers, &jar, workspace_id, target, false).await?;
    let result = list_revisions_for(
        &state.auth.db.pool,
        workspace_id,
        user_id,
        credential_id,
        target,
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
    get_target_revision(
        state,
        headers,
        jar,
        workspace_id,
        RevisionTarget::Document(document_id),
        revision_id,
    )
    .await
}

async fn get_target_revision(
    state: AppState,
    headers: HeaderMap,
    jar: CookieJar,
    workspace_id: Uuid,
    target: RevisionTarget,
    revision_id: Uuid,
) -> Result<Json<RevisionDetailResponse>, RevisionApiError> {
    let (user_id, credential_id) =
        revision_credential(&state, &headers, &jar, workspace_id, target, false).await?;
    let result = get_revision_for(
        &state.auth.db.pool,
        workspace_id,
        user_id,
        credential_id,
        target,
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
    restore_target_revision(
        state,
        peer,
        headers,
        jar,
        workspace_id,
        RevisionTarget::Document(document_id),
        revision_id,
        body,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn restore_target_revision(
    state: AppState,
    peer: SocketAddr,
    headers: HeaderMap,
    jar: CookieJar,
    workspace_id: Uuid,
    target: RevisionTarget,
    revision_id: Uuid,
    body: Bytes,
) -> Result<Json<RevisionRestoreResponse>, RevisionApiError> {
    check_origin(&headers, &state.public_origin)?;
    let _restore_body: RevisionRestoreBody = if body.is_empty() {
        RevisionRestoreBody {
            correlation_id: None,
        }
    } else {
        serde_json::from_slice(&body).map_err(|_| AppError::from_code(ProblemCode::InvalidInput))?
    };
    let (user_id, credential_id) =
        revision_credential(&state, &headers, &jar, workspace_id, target, true).await?;
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
    let snap = resolve_restore(
        &state.auth.db.pool,
        workspace_id,
        user_id,
        credential_id,
        target,
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
    let restore =
        hub.restore_revision(room_key(workspace_id, target), user_id, credential_id, snap);
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
    target: RevisionTarget,
) -> Result<CapturedRevision, RevisionApiError> {
    if let Some(hub) = state.collab.as_ref() {
        if let Some(live) = hub
            .capture_if_live(room_key(workspace_id, target), user_id, session_id)
            .await
        {
            return live.map_err(|_| collab_unavailable());
        }
    }
    let persisted = load_persisted_target_source(
        &state.auth.db.pool,
        workspace_id,
        user_id,
        session_id,
        target,
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
        RevisionDbError::TaskArchived => AppError::from_code(ProblemCode::TaskArchived).into(),
        RevisionDbError::ProjectArchived => {
            AppError::from_code(ProblemCode::ProjectArchived).into()
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

/// Task revisions accept the source's tasks.read/write PAT scopes. Document
/// revision routes retain their existing cookie-only contract.
async fn revision_credential(
    state: &AppState,
    headers: &HeaderMap,
    jar: &CookieJar,
    workspace_id: Uuid,
    target: RevisionTarget,
    write: bool,
) -> Result<(Uuid, Uuid), AppError> {
    match target {
        RevisionTarget::Task(_) => {
            let scope = if write {
                ApiTokenScope::TasksWrite
            } else {
                ApiTokenScope::TasksRead
            };
            let auth = require_request_auth(
                state,
                headers,
                jar,
                Access::Scope(scope),
                Some(workspace_id),
            )
            .await?;
            Ok((auth.user_id, auth.credential_id))
        }
        RevisionTarget::Document(_) => {
            reject_bearer(headers)?;
            let (user, session_id) = require_session(state, jar).await?;
            Ok((parse_user_id(&user.user_id)?, session_id))
        }
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
