//! Timer endpoints use the same auth, origin and problem boundary as tasks.
use axum::{
    extract::{
        rejection::{JsonRejection, QueryRejection},
        Path, Query, State,
    },
    http::HeaderMap,
    routing::{get, post},
    Json, Router,
};
use axum_extra::extract::CookieJar;
use uuid::Uuid;

use crate::api::task_timer::{
    LegacyReleaseBody, LegacyReleaseOutput, OwnerTimerState, TaskTimerState, TimeCorrectionBody,
    TimerCleanupBody, TimerCommandBody, TimerCommandOutput, TimerContextQuery, TimerHistory,
    TimerHistoryQuery, TimerManualBody, TimerRecordOutput, TimerSummary, TimerSummaryQuery,
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
        .route("/api/v1/me/task-timer/legacy-release", post(release_legacy))
        .route(
            "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/timer/history",
            get(history).post(create_manual),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/timer/summary",
            get(summary),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/timer/records/{record_id}/correct",
            post(correct),
        )
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
    query: Result<Query<TimerContextQuery>, QueryRejection>,
) -> Result<Json<TaskTimerState>, TaskApiError> {
    let auth = require_request_auth(
        &app,
        &headers,
        &jar,
        Access::Scope(ApiTokenScope::TasksRead),
        Some(workspace),
    )
    .await?;
    let Query(query) = query.map_err(|_| AppError::from_code(ProblemCode::InvalidInput))?;
    captured_context(
        query.expected_actor_id,
        query.expected_session_id,
        auth.user_id,
        auth.credential_id,
    )?;
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
    query: Result<Query<TimerContextQuery>, QueryRejection>,
) -> Result<Json<OwnerTimerState>, TaskApiError> {
    let auth = require_request_auth(&app, &headers, &jar, Access::Session, None).await?;
    let Query(query) = query.map_err(|_| AppError::from_code(ProblemCode::InvalidInput))?;
    captured_context(
        query.expected_actor_id,
        query.expected_session_id,
        auth.user_id,
        auth.credential_id,
    )?;
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

pub(crate) fn captured_context(
    actor: Option<Uuid>,
    session: Option<Uuid>,
    actual_actor: Uuid,
    actual_session: Uuid,
) -> Result<(), TaskApiError> {
    if actor.is_some_and(|v| v != actual_actor) || session.is_some_and(|v| v != actual_session) {
        return Err(error(TimerDbError::Conflict("timer_context_changed")));
    }
    Ok(())
}
async fn history(
    State(app): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace, task)): Path<(Uuid, Uuid)>,
    query: Result<Query<TimerHistoryQuery>, QueryRejection>,
) -> Result<Json<TimerHistory>, TaskApiError> {
    let Query(query) = query.map_err(|_| AppError::from_code(ProblemCode::InvalidInput))?;
    let auth = require_request_auth(
        &app,
        &headers,
        &jar,
        Access::Scope(ApiTokenScope::TasksRead),
        Some(workspace),
    )
    .await?;
    captured_context(
        query.expected_actor_id,
        query.expected_session_id,
        auth.user_id,
        auth.credential_id,
    )?;
    task_timer::history(
        &app.auth.db.pool,
        workspace,
        task,
        auth.user_id,
        auth.credential_id,
        &query,
    )
    .await
    .map_err(internal)?
    .map(Json)
    .map_err(error)
}
async fn summary(
    State(app): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace, task)): Path<(Uuid, Uuid)>,
    query: Result<Query<TimerSummaryQuery>, QueryRejection>,
) -> Result<Json<TimerSummary>, TaskApiError> {
    let Query(query) = query.map_err(|_| AppError::from_code(ProblemCode::InvalidInput))?;
    let auth = require_request_auth(
        &app,
        &headers,
        &jar,
        Access::Scope(ApiTokenScope::TasksRead),
        Some(workspace),
    )
    .await?;
    captured_context(
        query.expected_actor_id,
        query.expected_session_id,
        auth.user_id,
        auth.credential_id,
    )?;
    task_timer::summary(
        &app.auth.db.pool,
        workspace,
        task,
        auth.user_id,
        auth.credential_id,
        &query,
    )
    .await
    .map_err(internal)?
    .map(Json)
    .map_err(error)
}
async fn create_manual(
    State(app): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace, task)): Path<(Uuid, Uuid)>,
    body: Result<Json<TimerManualBody>, JsonRejection>,
) -> Result<Json<TimerRecordOutput>, TaskApiError> {
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
    task_timer::create_manual(
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
async fn correct(
    State(app): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace, task, id)): Path<(Uuid, Uuid, Uuid)>,
    body: Result<Json<TimeCorrectionBody>, JsonRejection>,
) -> Result<Json<TimerRecordOutput>, TaskApiError> {
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
    task_timer::correct(
        &app.auth.db.pool,
        workspace,
        task,
        id,
        auth.user_id,
        auth.credential_id,
        &body,
    )
    .await
    .map_err(internal)?
    .map(Json)
    .map_err(error)
}
async fn release_legacy(
    State(app): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    body: Result<Json<LegacyReleaseBody>, JsonRejection>,
) -> Result<Json<LegacyReleaseOutput>, TaskApiError> {
    check_origin(&headers, &app.public_origin)?;
    let Json(body) = body.map_err(AppError::from)?;
    let auth = require_request_auth(&app, &headers, &jar, Access::Session, None).await?;
    task_timer::release_legacy(&app.auth.db.pool, auth.user_id, auth.credential_id, &body)
        .await
        .map_err(internal)?
        .map(Json)
        .map_err(error)
}
