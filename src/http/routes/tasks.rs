use std::net::SocketAddr;

use axum::extract::rejection::JsonRejection;
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use serde_json::json;
use uuid::Uuid;

use crate::api::dto::{CreateTaskBody, TaskListResponse, TaskMetaOutput, TaskOutput};
use crate::auth::session::SessionUser;
use crate::db::projects::ProjectDbError;
use crate::db::tasks::{create_task, get_task, list_project_tasks, CreateTaskInput};
use crate::error::{AppError, ProblemCode, SESSION_COOKIE};
use crate::http::guard::{check_origin, reject_bearer};
use crate::http::rate_limit::peer_ip;
use crate::http::routes::projects::map_project_error;
use crate::http::state::AppState;
use crate::tasks::{priority_is_valid, task_type_is_valid, title_is_valid};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks",
            get(list_tasks).post(create_task_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/tasks/{task_id}",
            get(get_task_route),
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
            milestone_id: body.milestone_id,
            recurrence: body.recurrence,
        },
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(task) => Ok((
            StatusCode::CREATED,
            Json(task_meta_output(task)),
        )
            .into_response()),
        Err(ProjectDbError::Conflict) => Err(TaskApiError::Coded {
            status: StatusCode::CONFLICT,
            code: "task_hierarchy_violation",
            title: "task hierarchy violation".to_string(),
        }),
        Err(err) => Err(map_project_error(err).into()),
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
            children: Vec::new(),
            parent: None,
        })),
        Err(err) => Err(map_project_error(err).into()),
    }
}

async fn list_tasks(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<TaskListResponse>, TaskApiError> {
    reject_bearer(&headers)?;
    let (user, session_id) = require_session(&state, &jar).await?;
    let actor_user_id = parse_user_id(&user.user_id)?;
    let result = list_project_tasks(
        &state.auth.db.pool,
        workspace_id,
        project_id,
        actor_user_id,
        session_id,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(tasks) => Ok(Json(TaskListResponse {
            items: tasks.into_iter().map(task_meta_output).collect(),
        })),
        Err(err) => Err(map_project_error(err).into()),
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
