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
    CreateTaskBody, MoveTaskBody, OkResponse, PatchTaskBody, TaskChildOutput,
    TaskChildProgressOutput, TaskListItemOutput, TaskListResponse, TaskMetaOutput, TaskOutput,
    TaskParentOutput, TaskStatusCountOutput,
};
use crate::auth::session::SessionUser;
use crate::db::projects::ProjectDbError;
use crate::db::tasks::{
    create_task, get_task, list_project_tasks, move_task, patch_task_meta, restore_task,
    trash_task, CreateTaskInput,
};
use crate::error::{AppError, ProblemCode, SESSION_COOKIE};
use crate::http::guard::{check_origin, reject_bearer};
use crate::http::rate_limit::peer_ip;
use crate::http::routes::projects::map_project_error;
use crate::http::state::AppState;
use crate::tasks::list_query::{parse_task_list_query, TaskListQueryError};
use crate::tasks::patch::{
    estimate_is_valid, ExpectedDatesInput, FieldUpdate, MoveTaskInput, PatchTaskMetaInput,
};
use crate::tasks::{priority_is_valid, task_type_is_valid, title_is_valid};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskListQueryParams {
    pub query: Option<String>,
    pub archived: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<i32>,
    pub from: Option<String>,
    pub to: Option<String>,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks",
            get(list_tasks).post(create_task_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/tasks/{task_id}",
            get(get_task_route).patch(patch_task_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/move",
            post(move_task_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/trash",
            post(trash_task_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/restore",
            post(restore_task_route),
        )
}

async fn create_task_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id)): Path<(Uuid, Uuid)>,
    body: Result<Json<CreateTaskBody>, JsonRejection>,
) -> Result<Response, TaskApiError> {
    let Json(body) = body.map_err(AppError::from)?;
    reject_bearer(&headers)?;
    check_origin(&headers, &state.public_origin)?;
    if !title_is_valid(&body.title) {
        return Err(AppError::from_code(ProblemCode::InvalidInput).into());
    }
    if !task_type_is_valid(&body.task_type) {
        return Err(AppError::from_code(ProblemCode::InvalidInput).into());
    }
    if !priority_is_valid(&body.priority) {
        return Err(AppError::from_code(ProblemCode::InvalidInput).into());
    }
    if body.milestone_id.is_some() {
        return Err(AppError::from_code(ProblemCode::InvalidInput).into());
    }
    let (user, session_id) = require_session(&state, &jar).await?;
    let actor_user_id = parse_user_id(&user.user_id)?;
    let ip = peer_ip(peer.ip());
    let result = create_task(
        &state.auth.db.pool,
        workspace_id,
        project_id,
        actor_user_id,
        session_id,
        CreateTaskInput {
            title: &body.title,
            task_type: &body.task_type,
            priority: &body.priority,
            status_id: body.status_id,
            start_date: body.start_date,
            due_date: body.due_date,
            parent_id: body.parent_id,
            milestone_id: None,
            recurrence: body.recurrence,
        },
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(task) => Ok((StatusCode::CREATED, Json(task_meta_output(task))).into_response()),
        Err(err) => Err(map_task_db_error(err)),
    }
}

async fn get_task_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, task_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<TaskOutput>, TaskApiError> {
    reject_bearer(&headers)?;
    let (user, session_id) = require_session(&state, &jar).await?;
    let actor_user_id = parse_user_id(&user.user_id)?;
    let result = get_task(
        &state.auth.db.pool,
        workspace_id,
        task_id,
        actor_user_id,
        session_id,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(task) => Ok(Json(TaskOutput {
            meta: task_meta_output(task.meta),
            content_json: task.content_json,
            can_edit: task.can_edit,
            assignee_ids: Vec::new(),
            label_ids: Vec::new(),
            dependencies: Vec::new(),
            child_progress: task.child_progress.map(|progress| TaskChildProgressOutput {
                done: progress.done,
                total: progress.total,
            }),
            parent: task.parent.map(|parent| TaskParentOutput {
                id: parent.id.to_string(),
                title: parent.title,
                task_type: parent.task_type,
                number: parent.number,
            }),
            children: task
                .children
                .into_iter()
                .map(|child| TaskChildOutput {
                    id: child.id.to_string(),
                    number: child.number,
                    title: child.title,
                    task_type: child.task_type,
                    status_id: child.status_id.to_string(),
                })
                .collect(),
        })),
        Err(err) => Err(map_task_db_error(err)),
    }
}

async fn patch_task_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, task_id)): Path<(Uuid, Uuid)>,
    body: Result<Json<PatchTaskBody>, JsonRejection>,
) -> Result<Json<TaskMetaOutput>, TaskApiError> {
    let Json(body) = body.map_err(AppError::from)?;
    reject_bearer(&headers)?;
    check_origin(&headers, &state.public_origin)?;
    if body.assignee_ids.is_some() || body.label_ids.is_some() || body.milestone_id.is_some() {
        return Err(AppError::from_code(ProblemCode::InvalidInput).into());
    }
    let input = parse_patch_body(&body)?;
    let (user, session_id) = require_session(&state, &jar).await?;
    let actor_user_id = parse_user_id(&user.user_id)?;
    let ip = peer_ip(peer.ip());
    let result = patch_task_meta(
        &state.auth.db.pool,
        workspace_id,
        task_id,
        actor_user_id,
        session_id,
        input,
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(task) => Ok(Json(task_meta_output(task))),
        Err(err) => Err(map_task_db_error(err)),
    }
}

async fn move_task_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, task_id)): Path<(Uuid, Uuid)>,
    body: Result<Json<MoveTaskBody>, JsonRejection>,
) -> Result<Json<TaskMetaOutput>, TaskApiError> {
    let Json(body) = body.map_err(AppError::from)?;
    reject_bearer(&headers)?;
    check_origin(&headers, &state.public_origin)?;
    if body.before_id.is_some() && body.after_id.is_some() {
        return Err(AppError::from_code(ProblemCode::InvalidInput).into());
    }
    let (user, session_id) = require_session(&state, &jar).await?;
    let actor_user_id = parse_user_id(&user.user_id)?;
    let ip = peer_ip(peer.ip());
    let result = move_task(
        &state.auth.db.pool,
        workspace_id,
        task_id,
        actor_user_id,
        session_id,
        MoveTaskInput {
            status_id: body.status_id,
            expected_status_id: body.expected_status_id,
            before_id: body.before_id,
            after_id: body.after_id,
        },
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(task) => Ok(Json(task_meta_output(task))),
        Err(err) => Err(map_task_db_error(err)),
    }
}

async fn trash_task_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, task_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<OkResponse>, TaskApiError> {
    reject_bearer(&headers)?;
    check_origin(&headers, &state.public_origin)?;
    let (user, session_id) = require_session(&state, &jar).await?;
    let actor_user_id = parse_user_id(&user.user_id)?;
    let ip = peer_ip(peer.ip());
    let result = trash_task(
        &state.auth.db.pool,
        workspace_id,
        task_id,
        actor_user_id,
        session_id,
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(()) => Ok(Json(OkResponse { ok: true })),
        Err(err) => Err(map_task_db_error(err)),
    }
}

async fn restore_task_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, task_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<OkResponse>, TaskApiError> {
    reject_bearer(&headers)?;
    check_origin(&headers, &state.public_origin)?;
    let (user, session_id) = require_session(&state, &jar).await?;
    let actor_user_id = parse_user_id(&user.user_id)?;
    let ip = peer_ip(peer.ip());
    let result = restore_task(
        &state.auth.db.pool,
        workspace_id,
        task_id,
        actor_user_id,
        session_id,
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(()) => Ok(Json(OkResponse { ok: true })),
        Err(err) => Err(map_task_db_error(err)),
    }
}

fn parse_patch_body(body: &PatchTaskBody) -> Result<PatchTaskMetaInput, TaskApiError> {
    if body.expected_dates.is_none()
        && body.task_type.is_none()
        && body.title.is_none()
        && body.priority.is_none()
        && body.status_id.is_none()
        && body.start_date.is_none()
        && body.due_date.is_none()
        && body.due_at.is_none()
        && body.estimate.is_none()
        && body.parent_id.is_none()
        && body.recurrence.is_none()
        && body.archived.is_none()
    {
        return Err(AppError::from_code(ProblemCode::InvalidInput).into());
    }
    if let Some(title) = &body.title {
        if !title_is_valid(title) {
            return Err(AppError::from_code(ProblemCode::InvalidInput).into());
        }
    }
    if let Some(task_type) = &body.task_type {
        if !task_type_is_valid(task_type) {
            return Err(AppError::from_code(ProblemCode::InvalidInput).into());
        }
    }
    if let Some(priority) = &body.priority {
        if !priority_is_valid(priority) {
            return Err(AppError::from_code(ProblemCode::InvalidInput).into());
        }
    }
    if let Some(Some(estimate)) = &body.estimate {
        if !estimate_is_valid(estimate) {
            return Err(AppError::from_code(ProblemCode::InvalidInput).into());
        }
    }
    if let Some(Some(value)) = &body.recurrence {
        if !recurrence_preset_is_valid(value) {
            return Err(TaskApiError::Coded {
                status: StatusCode::BAD_REQUEST,
                code: "invalid_recurrence_preset",
                title: "invalid recurrence preset".to_string(),
            });
        }
    }
    Ok(PatchTaskMetaInput {
        expected_dates: body
            .expected_dates
            .as_ref()
            .map(|dates| ExpectedDatesInput {
                start_date: dates.start_date,
                due_date: dates.due_date,
                due_at: dates.due_at,
            }),
        task_type: body.task_type.clone(),
        title: body.title.clone(),
        priority: body.priority.clone(),
        status_id: body.status_id,
        start_date: FieldUpdate::from_optional(body.start_date),
        due_date: FieldUpdate::from_optional(body.due_date),
        due_at: FieldUpdate::from_optional(body.due_at),
        estimate: match &body.estimate {
            None => FieldUpdate::Unchanged,
            Some(None) => FieldUpdate::Clear,
            Some(Some(value)) => FieldUpdate::Set(value.clone()),
        },
        parent_id: FieldUpdate::from_optional(body.parent_id),
        recurrence: match &body.recurrence {
            None => FieldUpdate::Unchanged,
            Some(None) => FieldUpdate::Clear,
            Some(Some(value)) => FieldUpdate::Set(value.clone()),
        },
        archived: body.archived,
    })
}

fn recurrence_preset_is_valid(value: &serde_json::Value) -> bool {
    let Some(obj) = value.as_object() else {
        return false;
    };
    if obj.len() != 1 || !obj.contains_key("kind") {
        return false;
    }
    matches!(
        obj.get("kind").and_then(serde_json::Value::as_str),
        Some("daily" | "weekly" | "monthly")
    )
}

fn map_task_db_error(err: ProjectDbError) -> TaskApiError {
    match err {
        ProjectDbError::Conflict => TaskApiError::Coded {
            status: StatusCode::CONFLICT,
            code: "task_hierarchy_violation",
            title: "task hierarchy violation".to_string(),
        },
        ProjectDbError::VersionConflict => TaskApiError::Coded {
            status: StatusCode::CONFLICT,
            code: "document_version_mismatch",
            title: "document version mismatch".to_string(),
        },
        ProjectDbError::TaskArchived => TaskApiError::Coded {
            status: StatusCode::CONFLICT,
            code: "task_archived",
            title: "task archived".to_string(),
        },
        ProjectDbError::InvalidAnchor => TaskApiError::Coded {
            status: StatusCode::BAD_REQUEST,
            code: "anchor_not_in_target_list",
            title: "anchor not in target list".to_string(),
        },
        ProjectDbError::StatusNotInWorkflow => TaskApiError::Coded {
            status: StatusCode::BAD_REQUEST,
            code: "status_not_in_project_workflow",
            title: "status not in project workflow".to_string(),
        },
        ProjectDbError::WipLimitExceeded => TaskApiError::Coded {
            status: StatusCode::CONFLICT,
            code: "wip_limit_exceeded",
            title: "wip limit exceeded".to_string(),
        },
        ProjectDbError::WorkflowHasNoStatuses => TaskApiError::Coded {
            status: StatusCode::BAD_REQUEST,
            code: "workflow_has_no_statuses",
            title: "workflow has no statuses".to_string(),
        },
        ProjectDbError::InvalidMoveAnchors => AppError::from_code(ProblemCode::InvalidInput).into(),
        other => map_project_error(other).into(),
    }
}

async fn list_tasks(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id)): Path<(Uuid, Uuid)>,
    query: Result<Query<TaskListQueryParams>, QueryRejection>,
) -> Result<Json<TaskListResponse>, TaskApiError> {
    reject_bearer(&headers)?;
    let Query(params) = query.map_err(AppError::from)?;
    let parsed = parse_task_list_query(
        params.query.as_deref(),
        params.archived.as_deref(),
        params.cursor.as_deref(),
        params.limit,
        params.from.as_deref(),
        params.to.as_deref(),
    )
    .map_err(map_task_list_query_error)?;
    let (user, session_id) = require_session(&state, &jar).await?;
    let actor_user_id = parse_user_id(&user.user_id)?;
    let result = list_project_tasks(
        &state.auth.db.pool,
        workspace_id,
        project_id,
        actor_user_id,
        session_id,
        &parsed,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(page) => Ok(Json(TaskListResponse {
            items: page
                .items
                .into_iter()
                .map(|task| TaskListItemOutput {
                    meta: task_meta_output(task),
                    assignee_ids: Vec::new(),
                    label_ids: Vec::new(),
                })
                .collect(),
            next_cursor: page.next_cursor,
            status_counts: page
                .status_counts
                .into_iter()
                .map(|(status_id, count)| TaskStatusCountOutput {
                    status_id: status_id.to_string(),
                    count,
                })
                .collect(),
        })),
        Err(err) => Err(map_task_db_error(err)),
    }
}

fn map_task_list_query_error(err: TaskListQueryError) -> TaskApiError {
    match err {
        TaskListQueryError::InvalidInput => AppError::from_code(ProblemCode::InvalidInput).into(),
        TaskListQueryError::InvalidCursor => AppError {
            status: StatusCode::BAD_REQUEST,
            code: ProblemCode::InvalidInput,
            source: None,
            params: Some(json!({"code":"invalid_cursor"})),
            retry_after: None,
        }
        .into(),
    }
}

fn task_meta_output(task: crate::db::tasks::TaskMetaRow) -> TaskMetaOutput {
    TaskMetaOutput {
        id: task.id.to_string(),
        workspace_id: task.workspace_id.to_string(),
        project_id: task.project_id.to_string(),
        number: task.number,
        title: task.title,
        task_type: task.task_type,
        priority: task.priority,
        status_id: task.status_id.to_string(),
        start_date: task.start_date,
        due_date: task.due_date,
        due_at: task.due_at,
        estimate: task.estimate,
        parent_id: task.parent_id.map(|id| id.to_string()),
        milestone_id: task.milestone_id.map(|id| id.to_string()),
        recurrence: task.recurrence,
        sort_key: task.sort_key,
        schema_version: task.schema_version,
        version: task.version,
        archived_at: task.archived_at,
        created_by: task.created_by.to_string(),
        created_at: task.created_at,
        updated_at: task.updated_at,
    }
}

enum TaskApiError {
    App(AppError),
    Coded {
        status: StatusCode,
        code: &'static str,
        title: String,
    },
}

impl From<AppError> for TaskApiError {
    fn from(value: AppError) -> Self {
        Self::App(value)
    }
}

impl IntoResponse for TaskApiError {
    fn into_response(self) -> Response {
        match self {
            Self::App(err) => err.into_response(),
            Self::Coded {
                status,
                code,
                title,
            } => {
                let body = json!({
                    "type": "about:blank",
                    "title": title,
                    "status": status.as_u16(),
                    "code": code,
                });
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
