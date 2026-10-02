//! Timer endpoints use the same auth, origin and problem boundary as tasks.
use axum::{
    extract::{rejection::JsonRejection, Path, State},
    http::HeaderMap,
    routing::{get, post},
    Json, Router,
};
use axum_extra::extract::CookieJar;
use uuid::Uuid;

use crate::api::task_timer::{
    OwnerTimerState, TaskTimerState, TimerCleanupBody, TimerCommandBody, TimerCommandOutput,
};
use crate::auth::scopes::ApiTokenScope;
use crate::db::task_timer::{self, TimerDbError};
use crate::error::{AppError, ProblemCode};
use crate::http::routes::tasks::{internal, map_task_db_error, TaskApiError};
use crate::http::{
    authz::{require_request_auth, Access},
    guard::check_origin,
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/timer",
            get(state).post(command),
        )
        .route("/api/v1/me/task-timer", get(owner))
        .route("/api/v1/me/task-timer/stop", post(cleanup))
}

fn error(err: TimerDbError) -> TaskApiError {
    match err {
        TimerDbError::Project(err) => map_task_db_error(err),
        TimerDbError::InvalidInput => AppError::from_code(ProblemCode::InvalidInput).into(),
        TimerDbError::Conflict(reason) => {
            let mut err = AppError::from_code(ProblemCode::Conflict);
            err.params = Some(serde_json::json!({"code":reason}));
            err.into()
        }
    }
}

async fn state(
    State(app): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace, task)): Path<(Uuid, Uuid)>,
) -> Result<Json<TaskTimerState>, TaskApiError> {
    let auth = require_request_auth(
        &app,
        &headers,
        &jar,
        Access::Scope(ApiTokenScope::TasksRead),
        Some(workspace),
    )
    .await?;
    task_timer::task_state(
        &app.auth.db.pool,
        workspace,
        task,
        auth.user_id,
        auth.credential_id,
    )
    .await
    .map_err(internal)?
    .map(Json)
    .map_err(error)
}

async fn command(
    State(app): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace, task)): Path<(Uuid, Uuid)>,
    body: Result<Json<TimerCommandBody>, JsonRejection>,
) -> Result<Json<TimerCommandOutput>, TaskApiError> {
    check_origin(&headers, &app.public_origin)?;
    let Json(body) = body.map_err(AppError::from)?;
    let auth = require_request_auth(
        &app,
        &headers,
        &jar,
        Access::Scope(ApiTokenScope::TasksWrite),
        Some(workspace),
    )
    .await?;
    task_timer::command(
        &app.auth.db.pool,
        workspace,
        task,
        auth.user_id,
        auth.credential_id,
        &body,
    )
    .await
    .map_err(internal)?
    .map(Json)
    .map_err(error)
}

async fn owner(
    State(app): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<OwnerTimerState>, TaskApiError> {
    let auth = require_request_auth(&app, &headers, &jar, Access::Session, None).await?;
    task_timer::owner_state(&app.auth.db.pool, auth.user_id, auth.credential_id)
        .await
        .map_err(internal)?
        .map(Json)
        .map_err(error)
}

async fn cleanup(
    State(app): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    body: Result<Json<TimerCleanupBody>, JsonRejection>,
) -> Result<Json<TimerCommandOutput>, TaskApiError> {
    check_origin(&headers, &app.public_origin)?;
    let Json(body) = body.map_err(AppError::from)?;
    let auth = require_request_auth(&app, &headers, &jar, Access::Session, None).await?;
    task_timer::cleanup(&app.auth.db.pool, auth.user_id, auth.credential_id, &body)
        .await
        .map_err(internal)?
        .map(Json)
        .map_err(error)
}
