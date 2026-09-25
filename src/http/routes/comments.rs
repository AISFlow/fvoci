use std::net::SocketAddr;

use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::api::dto::{
    CommentListResponse, CommentOutput, CommentReactionBody, CommentReactionSummary,
    CreateCommentBody, OkResponse, PatchCommentBody,
};
use crate::auth::scopes::{grants_api_token_scope, ApiTokenScope};
use crate::db::comments::{
    comment_output, comment_write_kind, create_document_comment, create_project_document_comment,
    create_task_comment, list_document_comments, list_project_document_comments,
    list_task_comments, purge_comment, resolve_comment, set_comment_reaction, unresolve_comment,
    update_comment, CommentDbError, CommentListQuery, CommentWriteKind, CreateCommentInput,
    PatchCommentInput, ReactionInput,
};
use crate::error::{AppError, ProblemCode};
use crate::http::authz::{self, Access, RequestAuth};
use crate::http::guard::check_origin;
use crate::http::rate_limit::peer_ip;
use crate::http::state::AppState;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommentListQueryParams {
    pub limit: Option<i32>,
    pub cursor: Option<String>,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/workspaces/{workspace_id}/documents/{document_id}/comments",
            get(list_document_comments_route).post(create_document_comment_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/comments",
            get(list_project_document_comments_route).post(create_project_document_comment_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/comments",
            get(list_task_comments_route).post(create_task_comment_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/comments/{comment_id}",
            axum::routing::patch(patch_comment_route).delete(delete_comment_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/comments/{comment_id}/resolve",
            post(resolve_comment_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/comments/{comment_id}/unresolve",
            post(unresolve_comment_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/comments/{comment_id}/reactions",
            post(react_comment_route),
        )
}

fn comment_to_output(comment: &crate::db::comments::CommentRow, viewer_id: Uuid) -> CommentOutput {
    let (reactions, other_reaction_count) = comment_output(comment, viewer_id);
    CommentOutput {
        id: comment.id,
        workspace_id: comment.workspace_id,
        document_id: comment.document_id,
        task_id: comment.task_id,
        parent_id: comment.parent_id,
        created_by: comment.created_by,
        body: comment.body.clone(),
        resolved_at: comment.resolved_at,
        reactions: reactions
            .into_iter()
            .map(|(emoji, summary)| {
                (
                    emoji,
                    CommentReactionSummary {
                        count: summary.count,
                        reacted_by_me: summary.reacted_by_me,
                    },
                )
            })
            .collect(),
        other_reaction_count,
        created_at: comment.created_at,
        updated_at: comment.updated_at,
    }
}

fn validate_create_body(
    body: &CreateCommentBody,
) -> Result<(Vec<Uuid>, Vec<Uuid>), CommentApiError> {
    Ok((
        body.mentioned_user_ids.clone().unwrap_or_default(),
        body.mentioned_group_ids.clone().unwrap_or_default(),
    ))
}

fn create_input<'a>(
    body: &'a str,
    parent_id: Option<Uuid>,
    mentioned_user_ids: &'a [Uuid],
    mentioned_group_ids: &'a [Uuid],
) -> CreateCommentInput<'a> {
    CreateCommentInput {
        body,
        parent_id,
        mentioned_user_ids,
        mentioned_group_ids,
    }
}

async fn list_document_comments_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, document_id)): Path<(Uuid, Uuid)>,
    query: Result<Query<CommentListQueryParams>, QueryRejection>,
) -> Result<Json<CommentListResponse>, CommentApiError> {
    let Query(query) = query.map_err(AppError::from)?;
    let auth = require_comment_auth(
        &state,
        &headers,
        &jar,
        Access::Scope(ApiTokenScope::DocumentsRead),
        Some(workspace_id),
    )
    .await?;
    let page = list_document_comments(
        &state.auth.db.pool,
        workspace_id,
        auth.user_id,
        auth.credential_id,
        document_id,
        CommentListQuery {
            limit: query.limit.unwrap_or(50),
            cursor: query.cursor,
        },
    )
    .await
    .map_err(internal)?;
    match page {
        Ok(page) => Ok(Json(CommentListResponse {
            items: page
                .items
                .iter()
                .map(|row| comment_to_output(row, auth.user_id))
                .collect(),
            next_cursor: page.next_cursor,
        })),
        Err(err) => Err(map_comment_error(err)),
    }
}

async fn list_project_document_comments_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id, document_id)): Path<(Uuid, Uuid, Uuid)>,
    query: Result<Query<CommentListQueryParams>, QueryRejection>,
) -> Result<Json<CommentListResponse>, CommentApiError> {
    let Query(query) = query.map_err(AppError::from)?;
    let auth = require_comment_auth(
        &state,
        &headers,
        &jar,
        Access::Scope(ApiTokenScope::DocumentsRead),
        Some(workspace_id),
    )
    .await?;
    let page = list_project_document_comments(
        &state.auth.db.pool,
        workspace_id,
        auth.user_id,
        auth.credential_id,
        project_id,
        document_id,
        CommentListQuery {
            limit: query.limit.unwrap_or(50),
            cursor: query.cursor,
        },
    )
    .await
    .map_err(internal)?;
    match page {
        Ok(page) => Ok(Json(CommentListResponse {
            items: page
                .items
                .iter()
                .map(|row| comment_to_output(row, auth.user_id))
                .collect(),
            next_cursor: page.next_cursor,
        })),
        Err(err) => Err(map_comment_error(err)),
    }
}

async fn list_task_comments_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, task_id)): Path<(Uuid, Uuid)>,
    query: Result<Query<CommentListQueryParams>, QueryRejection>,
) -> Result<Json<CommentListResponse>, CommentApiError> {
    let Query(query) = query.map_err(AppError::from)?;
    let auth = require_comment_auth(
        &state,
        &headers,
        &jar,
        Access::Scope(ApiTokenScope::TasksRead),
        Some(workspace_id),
    )
    .await?;
    let page = list_task_comments(
        &state.auth.db.pool,
        workspace_id,
        auth.user_id,
        auth.credential_id,
        task_id,
        CommentListQuery {
            limit: query.limit.unwrap_or(50),
            cursor: query.cursor,
        },
    )
    .await
    .map_err(internal)?;
    match page {
        Ok(page) => Ok(Json(CommentListResponse {
            items: page
                .items
                .iter()
                .map(|row| comment_to_output(row, auth.user_id))
                .collect(),
            next_cursor: page.next_cursor,
        })),
        Err(err) => Err(map_comment_error(err)),
    }
}

async fn create_document_comment_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, document_id)): Path<(Uuid, Uuid)>,
    body: Result<Json<CreateCommentBody>, JsonRejection>,
) -> Result<Response, CommentApiError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    let (mentioned_user_ids, mentioned_group_ids) = validate_create_body(&body)?;
    let auth = require_comment_auth(
        &state,
        &headers,
        &jar,
        Access::Scope(ApiTokenScope::DocumentsWrite),
        Some(workspace_id),
    )
    .await?;
    let ip = peer_ip(peer.ip());
    if let Err(retry_after) = state
        .rate_limiter
        .allow(&format!("comment-create:{}", auth.user_id), 60)
        .await
    {
        return Err(AppError::rate_limited(retry_after).into());
    }
    let created = create_document_comment(
        &state.auth.db.pool,
        workspace_id,
        auth.user_id,
        auth.credential_id,
        document_id,
        create_input(
            &body.body,
            body.parent_id,
            &mentioned_user_ids,
            &mentioned_group_ids,
        ),
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match created {
        Ok(row) => Ok((
            StatusCode::CREATED,
            Json(comment_to_output(&row, auth.user_id)),
        )
            .into_response()),
        Err(err) => Err(map_comment_error(err)),
    }
}

async fn create_project_document_comment_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id, document_id)): Path<(Uuid, Uuid, Uuid)>,
    body: Result<Json<CreateCommentBody>, JsonRejection>,
) -> Result<Response, CommentApiError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    let (mentioned_user_ids, mentioned_group_ids) = validate_create_body(&body)?;
    let auth = require_comment_auth(
        &state,
        &headers,
        &jar,
        Access::Scope(ApiTokenScope::DocumentsWrite),
        Some(workspace_id),
    )
    .await?;
    let ip = peer_ip(peer.ip());
    if let Err(retry_after) = state
        .rate_limiter
        .allow(&format!("comment-create:{}", auth.user_id), 60)
        .await
    {
        return Err(AppError::rate_limited(retry_after).into());
    }
    let created = create_project_document_comment(
        &state.auth.db.pool,
        workspace_id,
        auth.user_id,
        auth.credential_id,
        (project_id, document_id),
        create_input(
            &body.body,
            body.parent_id,
            &mentioned_user_ids,
            &mentioned_group_ids,
        ),
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match created {
        Ok(row) => Ok((
            StatusCode::CREATED,
            Json(comment_to_output(&row, auth.user_id)),
        )
            .into_response()),
        Err(err) => Err(map_comment_error(err)),
    }
}

async fn create_task_comment_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, task_id)): Path<(Uuid, Uuid)>,
    body: Result<Json<CreateCommentBody>, JsonRejection>,
) -> Result<Response, CommentApiError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    let (mentioned_user_ids, mentioned_group_ids) = validate_create_body(&body)?;
    let auth = require_comment_auth(
        &state,
        &headers,
        &jar,
        Access::Scope(ApiTokenScope::TasksWrite),
        Some(workspace_id),
    )
    .await?;
    let ip = peer_ip(peer.ip());
    if let Err(retry_after) = state
        .rate_limiter
        .allow(&format!("comment-create:{}", auth.user_id), 60)
        .await
    {
        return Err(AppError::rate_limited(retry_after).into());
    }
    let created = create_task_comment(
        &state.auth.db.pool,
        workspace_id,
        auth.user_id,
        auth.credential_id,
        task_id,
        create_input(
            &body.body,
            body.parent_id,
            &mentioned_user_ids,
            &mentioned_group_ids,
        ),
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match created {
        Ok(row) => Ok((
            StatusCode::CREATED,
            Json(comment_to_output(&row, auth.user_id)),
        )
            .into_response()),
        Err(err) => Err(map_comment_error(err)),
    }
}

async fn patch_comment_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, comment_id)): Path<(Uuid, Uuid)>,
    body: Result<Json<PatchCommentBody>, JsonRejection>,
) -> Result<Json<CommentOutput>, CommentApiError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    if body.body.is_none() {
        return Err(CommentApiError::App(AppError::from_code(
            ProblemCode::InvalidInput,
        )));
    }
    let auth = require_mutation_auth(&state, &headers, &jar, workspace_id, comment_id).await?;
    let ip = peer_ip(peer.ip());
    let updated = update_comment(
        &state.auth.db.pool,
        workspace_id,
        auth.user_id,
        auth.credential_id,
        comment_id,
        PatchCommentInput {
            body: body.body.as_deref(),
        },
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match updated {
        Ok(row) => Ok(Json(comment_to_output(&row, auth.user_id))),
        Err(err) => Err(map_comment_error(err)),
    }
}

async fn delete_comment_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, comment_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<OkResponse>, CommentApiError> {
    check_origin(&headers, &state.public_origin)?;
    let auth = require_mutation_auth(&state, &headers, &jar, workspace_id, comment_id).await?;
    let ip = peer_ip(peer.ip());
    let result = purge_comment(
        &state.auth.db.pool,
        workspace_id,
        auth.user_id,
        auth.credential_id,
        comment_id,
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(()) => Ok(Json(OkResponse { ok: true })),
        Err(err) => Err(map_comment_error(err)),
    }
}

async fn resolve_comment_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, comment_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<CommentOutput>, CommentApiError> {
    check_origin(&headers, &state.public_origin)?;
    let auth = require_mutation_auth(&state, &headers, &jar, workspace_id, comment_id).await?;
    let ip = peer_ip(peer.ip());
    let updated = resolve_comment(
        &state.auth.db.pool,
        workspace_id,
        auth.user_id,
        auth.credential_id,
        comment_id,
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match updated {
        Ok(row) => Ok(Json(comment_to_output(&row, auth.user_id))),
        Err(err) => Err(map_comment_error(err)),
    }
}

async fn unresolve_comment_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, comment_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<CommentOutput>, CommentApiError> {
    check_origin(&headers, &state.public_origin)?;
    let auth = require_mutation_auth(&state, &headers, &jar, workspace_id, comment_id).await?;
    let ip = peer_ip(peer.ip());
    let updated = unresolve_comment(
        &state.auth.db.pool,
        workspace_id,
        auth.user_id,
        auth.credential_id,
        comment_id,
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match updated {
        Ok(row) => Ok(Json(comment_to_output(&row, auth.user_id))),
        Err(err) => Err(map_comment_error(err)),
    }
}

async fn react_comment_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, comment_id)): Path<(Uuid, Uuid)>,
    body: Result<Json<CommentReactionBody>, JsonRejection>,
) -> Result<Json<CommentOutput>, CommentApiError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    let auth = require_mutation_auth(&state, &headers, &jar, workspace_id, comment_id).await?;
    let ip = peer_ip(peer.ip());
    let updated = set_comment_reaction(
        &state.auth.db.pool,
        workspace_id,
        auth.user_id,
        auth.credential_id,
        comment_id,
        ReactionInput {
            emoji: &body.emoji,
            on: body.on,
        },
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match updated {
        Ok(row) => Ok(Json(comment_to_output(&row, auth.user_id))),
        Err(err) => Err(map_comment_error(err)),
    }
}

enum CommentApiError {
    App(AppError),
    Coded {
        status: StatusCode,
        code: &'static str,
        title: String,
        params: Option<serde_json::Value>,
    },
}

impl From<AppError> for CommentApiError {
    fn from(value: AppError) -> Self {
        Self::App(value)
    }
}

impl IntoResponse for CommentApiError {
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

fn map_comment_error(err: CommentDbError) -> CommentApiError {
    match err {
        CommentDbError::NotFound => CommentApiError::Coded {
            status: StatusCode::NOT_FOUND,
            code: "not_found",
            title: "not found".to_string(),
            params: None,
        },
        CommentDbError::InvalidInput => AppError::from_code(ProblemCode::InvalidInput).into(),
        CommentDbError::InvalidCursor => CommentApiError::Coded {
            status: StatusCode::BAD_REQUEST,
            code: "invalid_cursor",
            title: "invalid cursor".to_string(),
            params: Some(json!({"code": "invalid_cursor"})),
        },
        CommentDbError::Conflict => CommentApiError::Coded {
            status: StatusCode::CONFLICT,
            code: "conflict",
            title: "conflict".to_string(),
            params: None,
        },
        CommentDbError::ProjectArchived => AppError::from_code(ProblemCode::ProjectArchived).into(),
        CommentDbError::TaskArchived => CommentApiError::Coded {
            status: StatusCode::CONFLICT,
            code: "task_archived",
            title: "task archived".to_string(),
            params: None,
        },
    }
}

async fn require_comment_auth(
    state: &AppState,
    headers: &HeaderMap,
    jar: &CookieJar,
    access: Access,
    workspace_id: Option<Uuid>,
) -> Result<RequestAuth, CommentApiError> {
    Ok(authz::require_request_auth(state, headers, jar, access, workspace_id).await?)
}

async fn require_mutation_auth(
    state: &AppState,
    headers: &HeaderMap,
    jar: &CookieJar,
    workspace_id: Uuid,
    comment_id: Uuid,
) -> Result<RequestAuth, CommentApiError> {
    let auth = require_comment_auth(state, headers, jar, Access::Any, Some(workspace_id)).await?;
    if auth.token_scopes.is_some() {
        let kind = comment_write_kind(
            &state.auth.db.pool,
            workspace_id,
            auth.user_id,
            auth.credential_id,
            comment_id,
        )
        .await
        .map_err(internal)?;
        let kind = kind.map_err(map_comment_error)?;
        let required = match kind {
            CommentWriteKind::Document => ApiTokenScope::DocumentsWrite,
            CommentWriteKind::Task => ApiTokenScope::TasksWrite,
        };
        let scopes = auth.token_scopes.as_deref().unwrap_or(&[]);
        if !grants_api_token_scope(scopes, required) {
            return Err(map_comment_error(CommentDbError::NotFound));
        }
    }
    Ok(auth)
}

fn internal(err: sqlx::Error) -> AppError {
    tracing::error!("database error: {}", err);
    AppError::internal()
}
