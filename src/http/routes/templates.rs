//! Source `apps/server/src/domains/documents/templates.ts`.

use std::net::SocketAddr;

use axum::extract::rejection::JsonRejection;
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use uuid::Uuid;

use crate::api::dto::{
    TemplateApplyBody, TemplateApplyOutput, TemplateCreateBody, TemplateListResponse,
    TemplateOutput,
};
use crate::auth::scopes::{grants_api_token_scope, ApiTokenScope};
use crate::collections::iso_millis;
use crate::db::collections::Actor;
use crate::db::templates::{
    apply_template, create_template, list_templates, ApplyTemplateInput, TemplateDbError,
    TemplateKind, TemplateRow,
};
use crate::error::{AppError, ProblemCode};
use crate::http::authz::{require_request_auth, Access, RequestAuth};
use crate::http::guard::check_origin;
use crate::http::rate_limit::peer_ip;
use crate::http::routes::collections::{internal, invalid, ApiError};
use crate::http::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/workspaces/{workspace_id}/templates",
            get(list_route).post(create_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/templates/{template_id}/apply",
            post(apply_route),
        )
}

fn allowed_kinds(scopes: Option<&[ApiTokenScope]>, write: bool) -> Vec<TemplateKind> {
    if scopes.is_none() {
        return vec![TemplateKind::Document, TemplateKind::Task];
    }
    let scopes = scopes.unwrap();
    let mut kinds = Vec::new();
    let doc = if write {
        ApiTokenScope::DocumentsWrite
    } else {
        ApiTokenScope::DocumentsRead
    };
    let task = if write {
        ApiTokenScope::TasksWrite
    } else {
        ApiTokenScope::TasksRead
    };
    if grants_api_token_scope(scopes, doc) {
        kinds.push(TemplateKind::Document);
    }
    if grants_api_token_scope(scopes, task) {
        kinds.push(TemplateKind::Task);
    }
    kinds
}

fn map_error(err: TemplateDbError) -> ApiError {
    match err {
        TemplateDbError::NotFound => AppError::from_code(ProblemCode::NotFound).into(),
        TemplateDbError::Forbidden => AppError::from_code(ProblemCode::InsufficientPermissions).into(),
        TemplateDbError::InvalidInput => AppError::from_code(ProblemCode::InvalidInput).into(),
        TemplateDbError::ProjectArchived => AppError::from_code(ProblemCode::ProjectArchived).into(),
    }
}

fn to_output(row: TemplateRow) -> TemplateOutput {
    TemplateOutput {
        id: row.id.to_string(),
        workspace_id: row.workspace_id.to_string(),
        kind: row.kind.as_str().to_string(),
        title: row.title,
        payload: row.payload,
        created_by: row.created_by.to_string(),
        created_at: iso_millis(row.created_at),
    }
}

async fn auth_actor(
    state: &AppState,
    headers: &HeaderMap,
    jar: &CookieJar,
    workspace_id: Uuid,
    peer: Option<SocketAddr>,
) -> Result<(RequestAuth, Actor), ApiError> {
    let auth = require_request_auth(state, headers, jar, Access::Any, Some(workspace_id)).await?;
    let actor = Actor {
        user_id: auth.user_id,
        credential_id: auth.credential_id,
        client_ip: peer.map(|p| p.ip()),
    };
    Ok((auth, actor))
}

fn require_kind_scope(auth: &RequestAuth, kind: TemplateKind, write: bool) -> Result<(), ApiError> {
    let Some(scopes) = auth.token_scopes.as_deref() else {
        return Ok(());
    };
    let required = match (kind, write) {
        (TemplateKind::Document, true) => ApiTokenScope::DocumentsWrite,
        (TemplateKind::Document, false) => ApiTokenScope::DocumentsRead,
        (TemplateKind::Task, true) => ApiTokenScope::TasksWrite,
        (TemplateKind::Task, false) => ApiTokenScope::TasksRead,
    };
    if !grants_api_token_scope(scopes, required) {
        return Err(AppError::from_code(ProblemCode::NotFound).into());
    }
    Ok(())
}

async fn list_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
) -> Result<Json<TemplateListResponse>, ApiError> {
    let (auth, actor) = auth_actor(&state, &headers, &jar, workspace_id, None).await?;
    let kinds = allowed_kinds(auth.token_scopes.as_deref(), false);
    let rows = list_templates(&state.auth.db.pool, workspace_id, &actor, &kinds)
        .await
        .map_err(internal)?
        .map_err(map_error)?;
    Ok(Json(TemplateListResponse {
        items: rows.into_iter().map(to_output).collect(),
    }))
}

async fn create_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
    body: Result<Json<TemplateCreateBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    check_origin(&headers, &state.public_origin)?;
    let (auth, actor) = auth_actor(&state, &headers, &jar, workspace_id, Some(peer)).await?;
    let Json(body) = body.map_err(AppError::from)?;
    let kind = TemplateKind::parse(&body.kind).ok_or_else(|| invalid())?;
    require_kind_scope(&auth, kind, true)?;
    let row = create_template(
        &state.auth.db.pool,
        workspace_id,
        &actor,
        kind,
        &body.title,
        body.payload,
    )
    .await
    .map_err(internal)?
    .map_err(map_error)?;
    Ok((StatusCode::CREATED, Json(to_output(row))).into_response())
}

async fn apply_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, template_id)): Path<(Uuid, Uuid)>,
    body: Result<Json<TemplateApplyBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    check_origin(&headers, &state.public_origin)?;
    let (auth, actor) = auth_actor(&state, &headers, &jar, workspace_id, Some(peer)).await?;
    let Json(body) = body.map_err(AppError::from)?;
    let kinds = allowed_kinds(auth.token_scopes.as_deref(), true);
    let channel = if auth.token_scopes.is_some() { "api" } else { "web" };
    let ip = peer_ip(peer.ip());
    let applied = apply_template(
        &state.auth.db.pool,
        workspace_id,
        &actor,
        auth.credential_id,
        template_id,
        ApplyTemplateInput {
            project_id: body.project_id,
            parent_id: body.parent_id,
        },
        &kinds,
        channel,
        Some(&ip),
    )
    .await
    .map_err(internal)?
    .map_err(map_error)?;
    Ok((
        StatusCode::CREATED,
        Json(TemplateApplyOutput {
            kind: applied.kind.as_str().to_string(),
            id: applied.id.to_string(),
            display_id: applied.display_id,
        }),
    )
        .into_response())
}
