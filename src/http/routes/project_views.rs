//! Source `apps/server/src/domains/tasks/task-config.ts` `routes.views.*`:
//! per-user saved views of a project's task list (`tasks.*` scopes).

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch};
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use serde_json::Value;
use uuid::Uuid;

use crate::api::collections_dto::{ProjectViewListResponse, ProjectViewOutput};
use crate::api::dto::OkResponse;
use crate::auth::scopes::ApiTokenScope;
use crate::collections::{parse_project_view_create, parse_project_view_patch};
use crate::db::project_views::{
    create_view, delete_view, list_views, update_view, ProjectViewRow, ViewDbError,
};
use crate::error::{AppError, ProblemCode};
use crate::http::guard::check_origin;
use crate::http::routes::collections::{
    actor, internal, invalid, json_body, version_conflict, ApiError,
};
use crate::http::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/views",
            get(list_route).post(create_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/views/{view_id}",
            patch(update_route).delete(delete_route),
        )
}

fn map_view_error(err: ViewDbError) -> ApiError {
    match err {
        ViewDbError::NotFound => AppError::from_code(ProblemCode::NotFound).into(),
        ViewDbError::InvalidInput => invalid(),
        ViewDbError::VersionConflict => version_conflict(),
    }
}

fn output(row: ProjectViewRow) -> ProjectViewOutput {
    ProjectViewOutput {
        id: row.id.to_string(),
        project_id: row.project_id.to_string(),
        name: row.name,
        r#type: row.view_type,
        config: row.config,
    }
}

async fn list_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<ProjectViewListResponse>, ApiError> {
    let actor = actor(
        &state,
        &headers,
        &jar,
        ApiTokenScope::TasksRead,
        workspace_id,
        None,
    )
    .await?;
    let rows = list_views(&state.auth.db.pool, workspace_id, &actor, project_id)
        .await
        .map_err(internal)?
        .map_err(map_view_error)?;
    Ok(Json(ProjectViewListResponse {
        items: rows.into_iter().map(output).collect(),
    }))
}

async fn create_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id)): Path<(Uuid, Uuid)>,
    body: Result<Json<Value>, JsonRejection>,
) -> Result<Response, ApiError> {
    check_origin(&headers, &state.public_origin)?;
    let actor = actor(
        &state,
        &headers,
        &jar,
        ApiTokenScope::TasksWrite,
        workspace_id,
        None,
    )
    .await?;
    let input = parse_project_view_create(&json_body(body)?).map_err(|_| invalid())?;
    let row = create_view(
        &state.auth.db.pool,
        workspace_id,
        &actor,
        project_id,
        &input,
    )
    .await
    .map_err(internal)?
    .map_err(map_view_error)?;
    Ok((StatusCode::CREATED, Json(output(row))).into_response())
}

async fn update_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, view_id)): Path<(Uuid, Uuid)>,
    body: Result<Json<Value>, JsonRejection>,
) -> Result<Json<OkResponse>, ApiError> {
    check_origin(&headers, &state.public_origin)?;
    let actor = actor(
        &state,
        &headers,
        &jar,
        ApiTokenScope::TasksWrite,
        workspace_id,
        None,
    )
    .await?;
    let input = parse_project_view_patch(&json_body(body)?).map_err(|_| invalid())?;
    update_view(&state.auth.db.pool, workspace_id, &actor, view_id, &input)
        .await
        .map_err(internal)?
        .map_err(map_view_error)?;
    Ok(Json(OkResponse { ok: true }))
}

async fn delete_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, view_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<OkResponse>, ApiError> {
    check_origin(&headers, &state.public_origin)?;
    let actor = actor(
        &state,
        &headers,
        &jar,
        ApiTokenScope::TasksWrite,
        workspace_id,
        None,
    )
    .await?;
    delete_view(&state.auth.db.pool, workspace_id, &actor, view_id)
        .await
        .map_err(internal)?
        .map_err(map_view_error)?;
    Ok(Json(OkResponse { ok: true }))
}
