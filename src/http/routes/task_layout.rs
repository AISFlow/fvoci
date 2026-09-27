use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::routing::get;
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use uuid::Uuid;

use crate::auth::scopes::ApiTokenScope;
use crate::db::task_layout::get_project_task_layout;
use crate::error::{AppError, ProblemCode};
use crate::gantt::GanttLayoutOutput;
use crate::http::routes::tasks::{
    internal, map_task_db_error, map_task_list_query_error, require_session,
    TaskApiError,
};
use crate::http::state::AppState;
use crate::tasks::layout_query::{
    layout_list_query, parse_task_layout_query, TaskLayoutQueryError,
};

const LAYOUT_QUERY_KEYS: &[&str] = &[
    "year",
    "month",
    "weekStartsOn",
    "zoom",
    "pxPerDay",
    "laneHeight",
    "pack",
    "maxLanes",
    "query",
];

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/api/v1/workspaces/{workspace_id}/projects/{project_id}/task-layout",
        get(get_task_layout_route),
    )
}

fn map_layout_query_error(_: TaskLayoutQueryError) -> AppError {
    AppError::from_code(ProblemCode::InvalidInput)
}

async fn get_task_layout_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id)): Path<(Uuid, Uuid)>,
    Query(raw): Query<HashMap<String, String>>,
) -> Result<Json<GanttLayoutOutput>, TaskApiError> {
    if raw.keys().any(|k| !LAYOUT_QUERY_KEYS.contains(&k.as_str())) {
        return Err(AppError::from_code(ProblemCode::InvalidInput).into());
    }
    let parsed_layout = parse_task_layout_query(&raw).map_err(map_layout_query_error)?;
    let range = crate::gantt::month_range(
        parsed_layout.year,
        parsed_layout.month,
        parsed_layout.week_starts_on,
    )
    .ok_or_else(|| AppError::from_code(ProblemCode::InvalidInput))?;
    let from = if range.0.as_str() < "0001-01-01" {
        "0001-01-01"
    } else {
        range.0.as_str()
    };
    let list_query =
        layout_list_query(&parsed_layout, from, &range.1).map_err(map_task_list_query_error)?;
    let (_user, actor_user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(ApiTokenScope::TasksRead),
        Some(workspace_id),
    )
    .await?;
    let result = get_project_task_layout(
        &state.auth.db.pool,
        workspace_id,
        project_id,
        actor_user_id,
        session_id,
        &parsed_layout,
        &list_query,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(out) => Ok(Json(out)),
        Err(err) => Err(map_task_db_error(err)),
    }
}
