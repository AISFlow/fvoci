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

use crate::api::dto::CreateTaskBody;
use crate::api::task_timer::{
    LegacyReleaseBody, LegacyReleaseOutput, OwnerTimerState, StudyPlanTaskBody,
    StudyPlanTaskOutput, TaskEstimate, TaskEstimateCommandBody, TaskTimerState, TimeCorrectionBody,
    TimerCleanupBody, TimerCommandBody, TimerCommandOutput, TimerContextQuery, TimerHistory,
    TimerHistoryQuery, TimerManualBody, TimerRecordOutput, TimerSummary, TimerSummaryQuery,
};
use crate::api::tasks_dto::{TaskProjectOutput, TaskProjectPickerResponse};
use crate::auth::scopes::{grants_api_token_scope, ApiTokenScope};
use crate::db::task_origins::{origin_request_hash, task_projects, TASK_ORIGIN_ANCHOR_MAX_CHARS};
use crate::db::task_timer::{self, TimerDbError};
use crate::db::tasks::CreateTaskInput;
use crate::error::{AppError, ProblemCode};
use crate::http::routes::task_body::{map_origin_error, normalized_task_input};
use crate::http::routes::tasks::{internal, map_task_db_error, TaskApiError};
use crate::http::{
    authz::{require_request_auth, Access},
    guard::check_origin,
    state::AppState,
};
use crate::tasks::{priority_is_valid, task_type_is_valid, title_is_valid};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/timer",
            get(state).post(command),
        )
        .route("/api/v1/me/task-timer", get(owner))
        .route(
            "/api/v1/workspaces/{workspace_id}/documents/{document_id}/study-plan/task",
            get(plan_targets).post(create_plan_task),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/timer/estimate",
            post(set_estimate),
        )
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

async fn plan_targets(
    State(app): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace, document)): Path<(Uuid, Uuid)>,
    query: Result<Query<TimerContextQuery>, QueryRejection>,
) -> Result<Json<TaskProjectPickerResponse>, TaskApiError> {
    let auth = require_request_auth(
        &app,
        &headers,
        &jar,
        Access::Scope(ApiTokenScope::DocumentsRead),
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
    let picker = task_projects(
        app.auth.db.pool.postgres("task timer").map_err(internal)?,
        workspace,
        auth.user_id,
        auth.credential_id,
        document,
    )
    .await
    .map_err(internal)?
    .map_err(map_origin_error)?;
    Ok(Json(TaskProjectPickerResponse {
        items: picker
            .items
            .into_iter()
            .map(|item| TaskProjectOutput {
                id: item.id.to_string(),
                name: item.name,
                key: item.key,
                visibility: item.visibility,
            })
            .collect(),
        suggested_id: picker.suggested_id.map(|id| id.to_string()),
        can_create_project: picker.can_create_project,
    }))
}

async fn create_plan_task(
    State(app): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace, document)): Path<(Uuid, Uuid)>,
    body: Result<Json<StudyPlanTaskBody>, JsonRejection>,
) -> Result<Json<StudyPlanTaskOutput>, TaskApiError> {
    check_origin(&headers, &app.public_origin)?;
    let Json(body) = body.map_err(AppError::from)?;
    let task: CreateTaskBody = serde_json::from_value(body.task.clone())
        .map_err(|_| AppError::from_code(ProblemCode::InvalidInput))?;
    if !title_is_valid(&task.title)
        || !task_type_is_valid(&task.task_type)
        || !priority_is_valid(&task.priority)
        || body
            .anchor
            .as_deref()
            .is_some_and(|anchor| anchor.chars().count() > TASK_ORIGIN_ANCHOR_MAX_CHARS)
    {
        return Err(AppError::from_code(ProblemCode::InvalidInput).into());
    }
    let auth = require_request_auth(
        &app,
        &headers,
        &jar,
        Access::Scope(ApiTokenScope::DocumentsWrite),
        Some(workspace),
    )
    .await?;
    if auth
        .token_scopes
        .as_deref()
        .is_some_and(|scopes| !grants_api_token_scope(scopes, ApiTokenScope::TasksWrite))
    {
        return Err(AppError::from_code(ProblemCode::NotFound).into());
    }
    // Only this new operation adds the minute binding. The existing ordinary
    // normalizer and source-origin hashes keep their exact old contract.
    let semantic = serde_json::json!({"operation":"study-plan-task-v1",
        "task":normalized_task_input(&task), "selfAssign":body.self_assign,
        "estimateMinutes":body.minutes});
    let digest = origin_request_hash(
        auth.user_id,
        body.project_id,
        body.anchor.as_deref(),
        &semantic,
    );
    task_timer::create_plan_task(
        app.auth.db.pool.postgres("task timer").map_err(internal)?,
        workspace,
        document,
        auth.user_id,
        auth.credential_id,
        &body,
        CreateTaskInput {
            title: &task.title,
            task_type: &task.task_type,
            priority: &task.priority,
            status_id: task.status_id,
            start_date: task.start_date,
            due_date: task.due_date,
            parent_id: task.parent_id,
            milestone_id: task.milestone_id,
            recurrence: task.recurrence.clone(),
        },
        &digest,
    )
    .await
    .map_err(internal)?
    .map(Json)
    .map_err(error)
}

async fn set_estimate(
    State(app): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace, task)): Path<(Uuid, Uuid)>,
    body: Result<Json<TaskEstimateCommandBody>, JsonRejection>,
) -> Result<Json<TaskEstimate>, TaskApiError> {
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
    task_timer::set_estimate(
        app.auth.db.pool.postgres("task timer").map_err(internal)?,
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

fn error(err: TimerDbError) -> TaskApiError {
    match err {
        TimerDbError::Project(err) => map_task_db_error(err),
        TimerDbError::Origin(err) => map_origin_error(err),
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
        app.auth.db.pool.postgres("task timer").map_err(internal)?,
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
        app.auth.db.pool.postgres("task timer").map_err(internal)?,
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
    task_timer::owner_state(
        app.auth.db.pool.postgres("task timer").map_err(internal)?,
        auth.user_id,
        auth.credential_id,
    )
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
    task_timer::cleanup(
        app.auth.db.pool.postgres("task timer").map_err(internal)?,
        auth.user_id,
        auth.credential_id,
        &body,
    )
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
        app.auth.db.pool.postgres("task timer").map_err(internal)?,
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
        app.auth.db.pool.postgres("task timer").map_err(internal)?,
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
        app.auth.db.pool.postgres("task timer").map_err(internal)?,
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
        app.auth.db.pool.postgres("task timer").map_err(internal)?,
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
    task_timer::release_legacy(
        app.auth.db.pool.postgres("task timer").map_err(internal)?,
        auth.user_id,
        auth.credential_id,
        &body,
    )
    .await
    .map_err(internal)?
    .map(Json)
    .map_err(error)
}
