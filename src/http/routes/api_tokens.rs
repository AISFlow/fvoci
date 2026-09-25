use std::net::SocketAddr;

use axum::extract::rejection::JsonRejection;
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use uuid::Uuid;

use crate::api::dto::{
    ApiTokenCreateBody, ApiTokenCreatedOutput, ApiTokenListResponse, ApiTokenOutput,
    MeApiTokenCreateBody, OkResponse,
};
use crate::auth::scopes::{parse_api_token_scope, ApiTokenScope};
use crate::db::api_tokens::{
    create_api_token, list_api_tokens, list_user_api_tokens, revoke_api_token,
    revoke_user_api_token, ApiTokenDbError, ApiTokenRecord, CreateApiTokenInput,
    API_TOKEN_CREATE_LIMIT,
};
use crate::error::{AppError, ProblemCode};
use crate::http::authz::{require_request_auth, Access};
use crate::http::guard::check_origin;
use crate::http::rate_limit::peer_ip;
use crate::http::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/workspaces/{workspace_id}/api-tokens",
            get(list_workspace_tokens).post(create_workspace_token),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/api-tokens/{id}",
            axum::routing::delete(revoke_workspace_token),
        )
        .route(
            "/api/v1/me/api-tokens",
            get(list_me_tokens).post(create_me_token),
        )
        .route(
            "/api/v1/me/api-tokens/{id}",
            axum::routing::delete(revoke_me_token),
        )
}

fn parse_scopes(values: &[String]) -> Result<Vec<ApiTokenScope>, AppError> {
    if values.is_empty() {
        return Err(AppError::from_code(ProblemCode::InvalidInput));
    }
    let mut scopes = Vec::with_capacity(values.len());
    for value in values {
        scopes.push(
            parse_api_token_scope(value)
                .ok_or_else(|| AppError::from_code(ProblemCode::InvalidInput))?,
        );
    }
    Ok(scopes)
}

fn record_output(item: &ApiTokenRecord) -> ApiTokenOutput {
    ApiTokenOutput {
        id: item.id.to_string(),
        workspace_id: item.workspace_id.to_string(),
        user_id: item.user_id.map(|id| id.to_string()),
        name: item.name.clone(),
        scopes: item.scopes.iter().map(|s| s.as_str().to_string()).collect(),
        expires_at: item.expires_at,
        created_at: item.created_at,
    }
}

fn created_output(item: &ApiTokenRecord, token: String) -> ApiTokenCreatedOutput {
    let base = record_output(item);
    ApiTokenCreatedOutput {
        id: base.id,
        workspace_id: base.workspace_id,
        user_id: base.user_id,
        name: base.name,
        scopes: base.scopes,
        expires_at: base.expires_at,
        created_at: base.created_at,
        token,
    }
}

fn map_token_error(err: ApiTokenDbError) -> AppError {
    match err {
        ApiTokenDbError::InvalidInput => AppError::from_code(ProblemCode::InvalidInput),
        ApiTokenDbError::Forbidden | ApiTokenDbError::NotFound => {
            AppError::from_code(ProblemCode::NotFound)
        }
    }
}

fn internal(err: sqlx::Error) -> AppError {
    tracing::error!("database error: {}", err);
    AppError::internal()
}

async fn list_workspace_tokens(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
) -> Result<Json<ApiTokenListResponse>, AppError> {
    let auth = require_request_auth(
        &state,
        &headers,
        &jar,
        Access::Scope(ApiTokenScope::WorkspaceManage),
        Some(workspace_id),
    )
    .await?;
    let result = list_api_tokens(
        &state.auth.db.pool,
        workspace_id,
        auth.user_id,
        auth.credential_id,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(items) => Ok(Json(ApiTokenListResponse {
            items: items.iter().map(record_output).collect(),
        })),
        Err(err) => Err(map_token_error(err)),
    }
}

async fn create_workspace_token(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
    body: Result<Json<ApiTokenCreateBody>, JsonRejection>,
) -> Result<Response, AppError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    let auth = require_request_auth(
        &state,
        &headers,
        &jar,
        Access::Scope(ApiTokenScope::WorkspaceManage),
        Some(workspace_id),
    )
    .await?;
    let scopes = parse_scopes(&body.scopes)?;
    let ip = peer_ip(peer.ip());
    if let Err(retry_after) = state
        .rate_limiter
        .allow(
            &format!("api-token-create:{}", auth.user_id),
            API_TOKEN_CREATE_LIMIT,
        )
        .await
    {
        return Err(AppError::rate_limited(retry_after));
    }
    let result = create_api_token(
        &state.auth.db.pool,
        workspace_id,
        auth.user_id,
        auth.credential_id,
        CreateApiTokenInput {
            name: &body.name,
            scopes: &scopes,
            unlimited: body.unlimited.unwrap_or(false),
            service: body.service.unwrap_or(false),
        },
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(created) => Ok((
            StatusCode::CREATED,
            Json(created_output(&created.record, created.token)),
        )
            .into_response()),
        Err(err) => Err(map_token_error(err)),
    }
}

async fn revoke_workspace_token(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, id)): Path<(Uuid, Uuid)>,
) -> Result<Json<OkResponse>, AppError> {
    check_origin(&headers, &state.public_origin)?;
    let auth = require_request_auth(
        &state,
        &headers,
        &jar,
        Access::Scope(ApiTokenScope::WorkspaceManage),
        Some(workspace_id),
    )
    .await?;
    let ip = peer_ip(peer.ip());
    let result = revoke_api_token(
        &state.auth.db.pool,
        workspace_id,
        auth.user_id,
        auth.credential_id,
        id,
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(()) => Ok(Json(OkResponse { ok: true })),
        Err(err) => Err(map_token_error(err)),
    }
}

async fn list_me_tokens(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<ApiTokenListResponse>, AppError> {
    let auth = require_request_auth(&state, &headers, &jar, Access::Session, None).await?;
    let result = list_user_api_tokens(&state.auth.db.pool, auth.user_id, auth.credential_id)
        .await
        .map_err(internal)?;
    match result {
        Ok(items) => Ok(Json(ApiTokenListResponse {
            items: items.iter().map(record_output).collect(),
        })),
        Err(err) => Err(map_token_error(err)),
    }
}

async fn create_me_token(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    body: Result<Json<MeApiTokenCreateBody>, JsonRejection>,
) -> Result<Response, AppError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    let auth = require_request_auth(&state, &headers, &jar, Access::Session, None).await?;
    let scopes = parse_scopes(&body.scopes)?;
    let ip = peer_ip(peer.ip());
    if let Err(retry_after) = state
        .rate_limiter
        .allow(
            &format!("api-token-create:{}", auth.user_id),
            API_TOKEN_CREATE_LIMIT,
        )
        .await
    {
        return Err(AppError::rate_limited(retry_after));
    }
    let result = create_api_token(
        &state.auth.db.pool,
        body.workspace_id,
        auth.user_id,
        auth.credential_id,
        CreateApiTokenInput {
            name: &body.name,
            scopes: &scopes,
            unlimited: false,
            service: false,
        },
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(created) => Ok((
            StatusCode::CREATED,
            Json(created_output(&created.record, created.token)),
        )
            .into_response()),
        Err(err) => Err(map_token_error(err)),
    }
}

async fn revoke_me_token(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(id): Path<Uuid>,
) -> Result<Json<OkResponse>, AppError> {
    check_origin(&headers, &state.public_origin)?;
    let auth = require_request_auth(&state, &headers, &jar, Access::Session, None).await?;
    let ip = peer_ip(peer.ip());
    let result = revoke_user_api_token(
        &state.auth.db.pool,
        auth.user_id,
        auth.credential_id,
        id,
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(()) => Ok(Json(OkResponse { ok: true })),
        Err(err) => Err(map_token_error(err)),
    }
}
