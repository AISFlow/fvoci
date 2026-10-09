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
    RevisionRestoreBody, RevisionRestorePreviewResponse, RevisionRestoreResponse,
};
use crate::auth::scopes::ApiTokenScope;
use crate::auth::session::SessionUser;
use crate::collab::revision::{capture_revision_offline, prepare_revision_text};
use crate::collab::room::RoomKey;
use crate::collab::room::{
    BodyWriteError, CapturedRevision, RevisionCaptureError, RevisionRestoreError,
};
use crate::db::revisions::{
    authorize_revision_target, authorize_revision_target_backend,
    create_manual_revision_with_room_proof, decode_revision_cursor,
    get_revision as get_revision_for, get_revision_backend as get_selected_revision_for,
    list_revisions_backend as list_revisions_for, load_persisted_target_source_backend,
    resolve_restore, CreateRevisionInput, RestoreRevisionInput, RevisionDbError, RevisionDetail,
    RevisionMeta, RevisionScope, RevisionTarget,
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
            "/api/v1/workspaces/{workspace_id}/documents/{document_id}/revisions/{revision_id}/restore-preview",
            get(preview_restore_revision),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/documents/{document_id}/revisions/{revision_id}/restore",
            post(restore_revision),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/revisions",
            get(list_project_document_revisions).post(create_project_document_revision),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/revisions/{revision_id}",
            get(get_project_document_revision),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/revisions/{revision_id}/restore-preview",
            get(preview_restore_project_document_revision),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/revisions/{revision_id}/restore",
            post(restore_project_document_revision),
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
            "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/revisions/{revision_id}/restore-preview",
            get(preview_restore_task_revision),
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
        RevisionTarget::Task(task_id).into(),
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
        RevisionTarget::Task(task_id).into(),
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
        RevisionTarget::Task(task_id).into(),
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
        RevisionTarget::Task(task_id).into(),
        revision_id,
        body,
    )
    .await
}

async fn create_project_document_revision(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id, document_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<Response, RevisionApiError> {
    create_target_revision(
        state,
        peer,
        headers,
        jar,
        workspace_id,
        RevisionScope::project_document(project_id, document_id),
    )
    .await
}

async fn list_project_document_revisions(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id, document_id)): Path<(Uuid, Uuid, Uuid)>,
    query: Result<Query<RevisionListQuery>, QueryRejection>,
) -> Result<Json<RevisionListResponse>, RevisionApiError> {
    list_target_revisions(
        state,
        headers,
        jar,
        workspace_id,
        RevisionScope::project_document(project_id, document_id),
        query,
    )
    .await
}

async fn get_project_document_revision(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id, document_id, revision_id)): Path<(Uuid, Uuid, Uuid, Uuid)>,
) -> Result<Json<RevisionDetailResponse>, RevisionApiError> {
    get_target_revision(
        state,
        headers,
        jar,
        workspace_id,
        RevisionScope::project_document(project_id, document_id),
        revision_id,
    )
    .await
}

async fn restore_project_document_revision(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id, document_id, revision_id)): Path<(Uuid, Uuid, Uuid, Uuid)>,
    body: Bytes,
) -> Result<Json<RevisionRestoreResponse>, RevisionApiError> {
    restore_target_revision(
        state,
        peer,
        headers,
        jar,
        workspace_id,
        RevisionScope::project_document(project_id, document_id),
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
        RevisionTarget::Document(document_id).into(),
    )
    .await
}

async fn create_target_revision(
    state: AppState,
    peer: SocketAddr,
    headers: HeaderMap,
    jar: CookieJar,
    workspace_id: Uuid,
    scope: RevisionScope,
) -> Result<Response, RevisionApiError> {
    check_origin(&headers, &state.public_origin)?;
    let (user_id, credential_id) =
        revision_credential(&state, &headers, &jar, workspace_id, scope.target(), true).await?;
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
    if state.realtime_mode == crate::config::RealtimeMode::Off {
        let id = crate::db::body_save::create_off_revision(
            &state.auth.db.pool,
            state.realtime_mode,
            state.native_engine.clone().ok_or_else(collab_unavailable)?,
            workspace_id,
            scope,
            user_id,
            credential_id,
        )
        .await
        .map_err(map_off_revision_error)?;
        return Ok((
            StatusCode::CREATED,
            Json(RevisionCreateResponse { id: id.to_string() }),
        )
            .into_response());
    }
    match authorize_revision_target_backend(
        &state.auth.db.pool,
        workspace_id,
        user_id,
        credential_id,
        scope,
        true,
    )
    .await
    .map_err(internal)?
    {
        Ok(()) => {}
        Err(err) => return Err(map_revision_error(err)),
    }
    let (captured, room_proof) =
        capture_for_create(&state, workspace_id, user_id, credential_id, scope).await?;
    #[cfg(feature = "db-tests")]
    crate::collab::room::pause_native_consumer_barrier(
        scope.target().id(),
        crate::collab::room::MANUAL_REVISION_BEFORE_WRITE,
    )
    .await;
    let text = prepare_revision_text(&captured.content_json).map_err(|_| collab_unavailable())?;
    let result = create_manual_revision_with_room_proof(
        &state.auth.db.pool,
        workspace_id,
        user_id,
        credential_id,
        scope,
        CreateRevisionInput {
            y_snapshot: captured.y_snapshot,
            content_json: captured.content_json,
            text,
            reason: "manual".into(),
        },
        room_proof,
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
        RevisionTarget::Document(document_id).into(),
        query,
    )
    .await
}

async fn list_target_revisions(
    state: AppState,
    headers: HeaderMap,
    jar: CookieJar,
    workspace_id: Uuid,
    scope: RevisionScope,
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
        revision_credential(&state, &headers, &jar, workspace_id, scope.target(), false).await?;
    let result = list_revisions_for(
        &state.auth.db.pool,
        workspace_id,
        user_id,
        credential_id,
        scope,
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
        RevisionTarget::Document(document_id).into(),
        revision_id,
    )
    .await
}

async fn get_target_revision(
    state: AppState,
    headers: HeaderMap,
    jar: CookieJar,
    workspace_id: Uuid,
    scope: RevisionScope,
    revision_id: Uuid,
) -> Result<Json<RevisionDetailResponse>, RevisionApiError> {
    let (user_id, credential_id) =
        revision_credential(&state, &headers, &jar, workspace_id, scope.target(), false).await?;
    let result = get_selected_revision_for(
        &state.auth.db.pool,
        workspace_id,
        user_id,
        credential_id,
        scope,
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
        RevisionTarget::Document(document_id).into(),
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
    scope: RevisionScope,
    revision_id: Uuid,
    body: Bytes,
) -> Result<Json<RevisionRestoreResponse>, RevisionApiError> {
    check_origin(&headers, &state.public_origin)?;
    let restore_body: RevisionRestoreBody = serde_json::from_slice(&body)
        .map_err(|_| AppError::from_code(ProblemCode::InvalidInput))?;
    let expected_tail_seq = parse_restore_tail(&restore_body.expected_tail_seq)
        .ok_or_else(|| AppError::from_code(ProblemCode::InvalidInput))?;
    let (user_id, credential_id) =
        revision_credential(&state, &headers, &jar, workspace_id, scope.target(), true).await?;
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
    if state.realtime_mode == crate::config::RealtimeMode::Off {
        let saved = crate::db::body_save::restore_off_body(
            &state.auth.db.pool,
            state.realtime_mode,
            state.native_engine.clone().ok_or_else(collab_unavailable)?,
            crate::db::body_save::OffBodyRequest {
                workspace: workspace_id,
                target: scope.target(),
                project: scope.project_id(),
                actor: user_id,
                credential: credential_id,
                command: restore_body.correlation_id,
                expected_tail: expected_tail_seq,
                update: Vec::new(),
                client_ip: Some(peer_ip(peer.ip())),
            },
            revision_id,
        )
        .await
        .map_err(map_off_revision_error)?;
        return Ok(Json(RevisionRestoreResponse {
            restored: true,
            revision_id: saved.revision_id.to_string(),
        }));
    }
    let snap = resolve_restore(
        state
            .auth
            .db
            .pool
            .postgres("src/http/routes/revisions.rs")
            .map_err(internal)?,
        workspace_id,
        user_id,
        credential_id,
        scope,
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
    let restore = hub.restore_revision(
        room_key(workspace_id, scope.target()),
        user_id,
        credential_id,
        snap,
        RestoreRevisionInput {
            scope,
            source_revision_id: revision_id,
            correlation_id: restore_body.correlation_id,
            expected_tail_seq,
        },
    );
    match tokio::time::timeout(timeout.max(Duration::from_millis(1)), restore).await {
        Ok(Ok(revision_id)) => Ok(Json(RevisionRestoreResponse {
            restored: true,
            revision_id: revision_id.to_string(),
        })),
        Ok(Err(RevisionRestoreError::Rejected)) => {
            Err(AppError::from_code(ProblemCode::RestoreRejected).into())
        }
        Ok(Err(RevisionRestoreError::Conflict)) => Err(restore_conflict()),
        Ok(Err(RevisionRestoreError::Unavailable)) => Err(collab_unavailable()),
        Err(_) => Err(AppError::from_code(ProblemCode::CollabTimeoutRetry).into()),
    }
}

fn parse_restore_tail(raw: &str) -> Option<i64> {
    if raw.is_empty()
        || raw.len() > 19
        || !raw.bytes().all(|byte| byte.is_ascii_digit())
        || (raw.len() > 1 && raw.starts_with('0'))
    {
        return None;
    }
    raw.parse::<i64>().ok().filter(|tail| *tail >= 0)
}

fn restore_conflict() -> RevisionApiError {
    RevisionApiError::Coded {
        status: StatusCode::CONFLICT,
        code: "revision_restore_conflict",
        title: "Document changed since the restore preview; review a fresh preview".into(),
        params: None,
    }
}

async fn preview_restore_revision(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, document_id, revision_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<Json<RevisionRestorePreviewResponse>, RevisionApiError> {
    preview_restore_target(
        state,
        headers,
        jar,
        workspace_id,
        RevisionTarget::Document(document_id).into(),
        revision_id,
    )
    .await
}
async fn preview_restore_project_document_revision(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id, document_id, revision_id)): Path<(Uuid, Uuid, Uuid, Uuid)>,
) -> Result<Json<RevisionRestorePreviewResponse>, RevisionApiError> {
    preview_restore_target(
        state,
        headers,
        jar,
        workspace_id,
        RevisionScope::project_document(project_id, document_id),
        revision_id,
    )
    .await
}
async fn preview_restore_task_revision(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, task_id, revision_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<Json<RevisionRestorePreviewResponse>, RevisionApiError> {
    preview_restore_target(
        state,
        headers,
        jar,
        workspace_id,
        RevisionTarget::Task(task_id).into(),
        revision_id,
    )
    .await
}
async fn preview_restore_target(
    state: AppState,
    headers: HeaderMap,
    jar: CookieJar,
    workspace_id: Uuid,
    scope: RevisionScope,
    revision_id: Uuid,
) -> Result<Json<RevisionRestorePreviewResponse>, RevisionApiError> {
    let (user_id, credential_id) =
        revision_credential(&state, &headers, &jar, workspace_id, scope.target(), true).await?;
    if state.realtime_mode == crate::config::RealtimeMode::Off {
        let preview = crate::db::body_save::preview_off_restore(
            &state.auth.db.pool,
            state.realtime_mode,
            state.native_engine.clone().ok_or_else(collab_unavailable)?,
            workspace_id,
            scope,
            user_id,
            credential_id,
            revision_id,
        )
        .await
        .map_err(map_off_revision_error)?;
        return Ok(Json(RevisionRestorePreviewResponse {
            source: detail_response(preview.source),
            current_content_json: preview.current_content_json,
            current_tail_seq: preview.current_tail.to_string(),
        }));
    }
    // The immutable source must belong to this exact route target. A preview
    // is for an editable restore; viewing history alone does not permit it.
    resolve_restore(
        state
            .auth
            .db
            .pool
            .postgres("src/http/routes/revisions.rs")
            .map_err(internal)?,
        workspace_id,
        user_id,
        credential_id,
        scope,
        revision_id,
        None,
    )
    .await
    .map_err(internal)?
    .map_err(map_revision_error)?;
    let source = get_revision_for(
        state
            .auth
            .db
            .pool
            .postgres("src/http/routes/revisions.rs")
            .map_err(internal)?,
        workspace_id,
        user_id,
        credential_id,
        scope,
        revision_id,
    )
    .await
    .map_err(internal)?
    .map_err(map_revision_error)?;
    let hub = state.collab.as_ref().ok_or_else(collab_unavailable)?;
    let current = tokio::time::timeout(
        hub.rpc_timeout().max(Duration::from_millis(1)),
        hub.project_live(
            room_key(workspace_id, scope.target()),
            user_id,
            credential_id,
        ),
    )
    .await
    .map_err(|_| AppError::from_code(ProblemCode::CollabTimeoutRetry))?
    .map_err(|err| match err {
        BodyWriteError::Rejected => map_revision_error(RevisionDbError::Forbidden),
        _ => collab_unavailable(),
    })?;
    // Check route affiliation again after room startup/project work.
    authorize_revision_target(
        state
            .auth
            .db
            .pool
            .postgres("src/http/routes/revisions.rs")
            .map_err(internal)?,
        workspace_id,
        user_id,
        credential_id,
        scope,
        true,
    )
    .await
    .map_err(internal)?
    .map_err(map_revision_error)?;
    Ok(Json(RevisionRestorePreviewResponse {
        source: detail_response(source),
        current_content_json: current.content_json,
        current_tail_seq: current.tail_seq.to_string(),
    }))
}

async fn capture_for_create(
    state: &AppState,
    workspace_id: Uuid,
    user_id: Uuid,
    session_id: Uuid,
    scope: RevisionScope,
) -> Result<
    (
        CapturedRevision,
        Option<crate::db::collab::FamilyNativeConsumerProof>,
    ),
    RevisionApiError,
> {
    if let Some(hub) = state.collab.as_ref() {
        if let Some(live) = hub
            .capture_if_live_guarded(room_key(workspace_id, scope.target()), user_id, session_id)
            .await
        {
            return live
                .map(|result| (result.captured, result.proof))
                .map_err(|_| collab_unavailable());
        }
    }
    let persisted = load_persisted_target_source_backend(
        &state.auth.db.pool,
        workspace_id,
        user_id,
        session_id,
        scope,
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
    .map(|captured| (captured, None))
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
        restored_from_id: meta.restored_from_id.map(|id| id.to_string()),
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
        restored_from_id: detail.meta.restored_from_id.map(|id| id.to_string()),
        content_json: detail.content_json,
        y_snapshot: collab_engine::b64::encode(&detail.y_snapshot),
    }
}

fn map_revision_error(err: RevisionDbError) -> RevisionApiError {
    match err {
        RevisionDbError::NotFound
        | RevisionDbError::Forbidden
        | RevisionDbError::StaleRevisionHead => AppError::from_code(ProblemCode::NotFound).into(),
        RevisionDbError::RestoreConflict => restore_conflict(),
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

fn map_off_revision_error(error: crate::db::body_save::BodySaveError) -> RevisionApiError {
    use crate::db::body_save::BodySaveError;
    use crate::db::collab::CollabDbError;
    let unconfirmed = || RevisionApiError::Coded {
        status: StatusCode::SERVICE_UNAVAILABLE,
        code: "body_save_unconfirmed",
        title: "history write is unconfirmed".into(),
        params: None,
    };
    match error {
        BodySaveError::Conflict
        | BodySaveError::RequestMismatch
        | BodySaveError::Native(CollabDbError::StaleWriter | CollabDbError::StaleCutoff) => {
            restore_conflict()
        }
        BodySaveError::Revision(error) => map_revision_error(error),
        BodySaveError::Native(
            CollabDbError::PayloadTooLarge | CollabDbError::StateBudgetExceeded,
        ) => AppError::from_code(ProblemCode::InvalidInput).into(),
        BodySaveError::Native(_) => AppError::from_code(ProblemCode::NotFound).into(),
        BodySaveError::Invalid => AppError::from_code(ProblemCode::InvalidInput).into(),
        BodySaveError::Unavailable | BodySaveError::Cancelled => collab_unavailable(),
        BodySaveError::CommitUnconfirmed(error) => {
            tracing::warn!(%error, settlement=?error.settlement,"OFF history finish unconfirmed");
            unconfirmed()
        }
        BodySaveError::RollbackUnconfirmed { original, cleanup } => {
            tracing::warn!(%original,%cleanup,"OFF history rollback unconfirmed");
            unconfirmed()
        }
        BodySaveError::Database(error) => {
            if matches!(&error, sqlx::Error::AnyDriverError(driver) if driver.downcast_ref::<crate::db::backend::RemoteSettlementUnconfirmed>().is_some())
            {
                unconfirmed()
            } else {
                internal(error).into()
            }
        }
    }
}

/// Task revisions accept the source's tasks.read/write PAT scopes. Wiki and
/// project document revision routes retain their existing cookie-only contract.
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

#[cfg(test)]
mod restore_tail_tests {
    use super::parse_restore_tail;
    #[test]
    fn opaque_tail_preserves_integer_precision_and_refuses_noncanonical_input() {
        assert_eq!(parse_restore_tail("0"), Some(0));
        assert_eq!(
            parse_restore_tail("9007199254740993"),
            Some(9_007_199_254_740_993)
        );
        assert_eq!(parse_restore_tail("9223372036854775807"), Some(i64::MAX));
        for input in [
            "",
            "01",
            "-1",
            "+1",
            "1.0",
            "1e2",
            " 1",
            "1 ",
            "9223372036854775808",
        ] {
            assert_eq!(parse_restore_tail(input), None, "{input}");
        }
    }
}
