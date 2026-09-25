use std::net::SocketAddr;

use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch as patch_method, post};
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::api::dto::{
    ActivityActorOutput, ActivityChangeOutput, ActivityCommentParentOutput, ActivityItemOutput,
    ActivityListResponse, CreateLabelBody, CreateMilestoneBody, CreateTaskBody,
    CreateTaskDependencyBody, LabelListResponse, LabelOutput, MilestoneListResponse,
    MilestoneOutput, MoveTaskBody, OkResponse, PatchLabelBody, PatchMilestoneBody, PatchTaskBody,
    TaskChildOutput, TaskChildProgressOutput, TaskDependencyListResponse, TaskDependencyOutput,
    TaskListItemOutput, TaskListResponse, TaskMetaOutput, TaskOutput, TaskParentOutput,
    TaskStatusCountOutput,
};
use crate::auth::session::SessionUser;
use crate::db::labels::{
    create_label, list_project_labels, list_workspace_labels, purge_label, update_label,
};
use crate::db::milestones::{
    create_milestone, list_project_milestones, purge_milestone, update_milestone,
};
use crate::db::projects::ProjectDbError;
use crate::db::task_activity::{
    list_task_activity, TaskActivityDbError, TaskActivityListPage, TaskActivityOutputItem,
};
use crate::db::tasks::{
    add_task_dependency, create_task, get_task, list_project_dependencies, list_project_tasks,
    move_task, patch_task_meta, remove_task_dependency, restore_task, trash_task, CreateTaskInput,
};
use crate::error::{AppError, ProblemCode};
use crate::http::guard::check_origin;
use crate::http::rate_limit::peer_ip;
use crate::http::routes::comments::comment_to_output;
use crate::http::routes::projects::map_project_error;
use crate::http::state::AppState;
use crate::tasks::activity::{encode_activity_cursor, ActivityFilter, ActivityListQuery};
use crate::tasks::dependency::DependencyType;
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

#[derive(Debug, Deserialize)]
pub struct TaskActivityQueryParams {
    pub filter: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<i32>,
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
            "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/activity",
            get(list_task_activity_route),
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
        .route(
            "/api/v1/workspaces/{workspace_id}/labels",
            get(list_workspace_labels_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/labels",
            get(list_project_labels_route).post(create_label_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/labels/{label_id}",
            patch_method(update_label_route).delete(delete_label_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/milestones",
            get(list_project_milestones_route).post(create_milestone_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/milestones/{milestone_id}",
            patch_method(update_milestone_route).delete(delete_milestone_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/dependencies",
            get(list_project_dependencies_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/dependencies",
            post(add_dependency_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/dependencies/{blocked_id}",
            axum::routing::delete(remove_dependency_route),
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
    let (user, _user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::TasksWrite),
        Some(workspace_id),
    )
    .await?;
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
        activity_channel(&headers),
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
    let (user, _user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::TasksRead),
        Some(workspace_id),
    )
    .await?;
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
            assignee_ids: uuid_strings(&task.assignee_ids),
            label_ids: uuid_strings(&task.label_ids),
            dependencies: task
                .dependencies
                .into_iter()
                .map(dependency_output)
                .collect(),
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
    check_origin(&headers, &state.public_origin)?;
    let input = parse_patch_body(&body)?;
    let (user, _user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::TasksWrite),
        Some(workspace_id),
    )
    .await?;
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
        activity_channel(&headers),
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
    check_origin(&headers, &state.public_origin)?;
    if body.before_id.is_some() && body.after_id.is_some() {
        return Err(AppError::from_code(ProblemCode::InvalidInput).into());
    }
    let (user, _user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::TasksWrite),
        Some(workspace_id),
    )
    .await?;
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
        activity_channel(&headers),
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
    check_origin(&headers, &state.public_origin)?;
    let (user, _user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::TasksWrite),
        Some(workspace_id),
    )
    .await?;
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
    check_origin(&headers, &state.public_origin)?;
    let (user, _user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::TasksWrite),
        Some(workspace_id),
    )
    .await?;
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

async fn list_workspace_labels_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
) -> Result<Json<LabelListResponse>, TaskApiError> {
    let (user, _user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::TasksRead),
        Some(workspace_id),
    )
    .await?;
    let actor_user_id = parse_user_id(&user.user_id)?;
    let result =
        list_workspace_labels(&state.auth.db.pool, workspace_id, actor_user_id, session_id)
            .await
            .map_err(internal)?;
    match result {
        Ok(labels) => Ok(Json(LabelListResponse {
            items: labels.into_iter().map(label_output).collect(),
        })),
        Err(err) => Err(map_task_db_error(err)),
    }
}

async fn list_project_labels_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<LabelListResponse>, TaskApiError> {
    let (user, _user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::TasksRead),
        Some(workspace_id),
    )
    .await?;
    let actor_user_id = parse_user_id(&user.user_id)?;
    let result = list_project_labels(
        &state.auth.db.pool,
        workspace_id,
        project_id,
        actor_user_id,
        session_id,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(labels) => Ok(Json(LabelListResponse {
            items: labels.into_iter().map(label_output).collect(),
        })),
        Err(err) => Err(map_task_db_error(err)),
    }
}

async fn create_label_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id)): Path<(Uuid, Uuid)>,
    body: Result<Json<CreateLabelBody>, JsonRejection>,
) -> Result<Response, TaskApiError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    let (user, _user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::TasksWrite),
        Some(workspace_id),
    )
    .await?;
    let actor_user_id = parse_user_id(&user.user_id)?;
    let result = create_label(
        &state.auth.db.pool,
        workspace_id,
        project_id,
        actor_user_id,
        session_id,
        &body.name,
        &body.color,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(label) => Ok((StatusCode::CREATED, Json(label_output(label))).into_response()),
        Err(err) => Err(map_task_db_error(err)),
    }
}

async fn update_label_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id, label_id)): Path<(Uuid, Uuid, Uuid)>,
    body: Result<Json<PatchLabelBody>, JsonRejection>,
) -> Result<Json<OkResponse>, TaskApiError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    let (user, _user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::TasksWrite),
        Some(workspace_id),
    )
    .await?;
    let actor_user_id = parse_user_id(&user.user_id)?;
    let result = update_label(
        &state.auth.db.pool,
        workspace_id,
        project_id,
        label_id,
        actor_user_id,
        session_id,
        body.name.as_deref(),
        body.color.as_deref(),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(()) => Ok(Json(OkResponse { ok: true })),
        Err(err) => Err(map_task_db_error(err)),
    }
}

async fn delete_label_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id, label_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<Json<OkResponse>, TaskApiError> {
    check_origin(&headers, &state.public_origin)?;
    let (user, _user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::TasksWrite),
        Some(workspace_id),
    )
    .await?;
    let actor_user_id = parse_user_id(&user.user_id)?;
    let result = purge_label(
        &state.auth.db.pool,
        workspace_id,
        project_id,
        label_id,
        actor_user_id,
        session_id,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(()) => Ok(Json(OkResponse { ok: true })),
        Err(err) => Err(map_task_db_error(err)),
    }
}

async fn list_project_milestones_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<MilestoneListResponse>, TaskApiError> {
    let (user, _user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::TasksRead),
        Some(workspace_id),
    )
    .await?;
    let actor_user_id = parse_user_id(&user.user_id)?;
    let result = list_project_milestones(
        &state.auth.db.pool,
        workspace_id,
        project_id,
        actor_user_id,
        session_id,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(rows) => Ok(Json(MilestoneListResponse {
            items: rows.into_iter().map(milestone_output).collect(),
        })),
        Err(err) => Err(map_task_db_error(err)),
    }
}

async fn create_milestone_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id)): Path<(Uuid, Uuid)>,
    body: Result<Json<CreateMilestoneBody>, JsonRejection>,
) -> Result<Response, TaskApiError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    let (user, _user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::TasksWrite),
        Some(workspace_id),
    )
    .await?;
    let actor_user_id = parse_user_id(&user.user_id)?;
    let result = create_milestone(
        &state.auth.db.pool,
        workspace_id,
        project_id,
        actor_user_id,
        session_id,
        &body.name,
        body.due_date.flatten(),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(row) => Ok((StatusCode::CREATED, Json(milestone_output(row))).into_response()),
        Err(err) => Err(map_task_db_error(err)),
    }
}

async fn update_milestone_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id, milestone_id)): Path<(Uuid, Uuid, Uuid)>,
    body: Result<Json<PatchMilestoneBody>, JsonRejection>,
) -> Result<Json<OkResponse>, TaskApiError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    let (user, _user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::TasksWrite),
        Some(workspace_id),
    )
    .await?;
    let actor_user_id = parse_user_id(&user.user_id)?;
    let result = update_milestone(
        &state.auth.db.pool,
        workspace_id,
        project_id,
        milestone_id,
        actor_user_id,
        session_id,
        body.name.as_deref(),
        body.due_date,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(()) => Ok(Json(OkResponse { ok: true })),
        Err(err) => Err(map_task_db_error(err)),
    }
}

async fn delete_milestone_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id, milestone_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<Json<OkResponse>, TaskApiError> {
    check_origin(&headers, &state.public_origin)?;
    let (user, _user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::TasksWrite),
        Some(workspace_id),
    )
    .await?;
    let actor_user_id = parse_user_id(&user.user_id)?;
    let result = purge_milestone(
        &state.auth.db.pool,
        workspace_id,
        project_id,
        milestone_id,
        actor_user_id,
        session_id,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(()) => Ok(Json(OkResponse { ok: true })),
        Err(err) => Err(map_task_db_error(err)),
    }
}

async fn list_project_dependencies_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<TaskDependencyListResponse>, TaskApiError> {
    let (user, _user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::TasksRead),
        Some(workspace_id),
    )
    .await?;
    let actor_user_id = parse_user_id(&user.user_id)?;
    let result = list_project_dependencies(
        &state.auth.db.pool,
        workspace_id,
        project_id,
        actor_user_id,
        session_id,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(items) => Ok(Json(TaskDependencyListResponse {
            items: items.into_iter().map(dependency_output).collect(),
        })),
        Err(err) => Err(map_task_db_error(err)),
    }
}

async fn add_dependency_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, task_id)): Path<(Uuid, Uuid)>,
    body: Result<Json<CreateTaskDependencyBody>, JsonRejection>,
) -> Result<Json<OkResponse>, TaskApiError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    if body.lag_days.is_some_and(|lag| lag < 0) {
        return Err(AppError::from_code(ProblemCode::InvalidInput).into());
    }
    let requested_type = match body.dependency_type.as_deref() {
        None => None,
        Some(value) => Some(
            DependencyType::parse(value)
                .ok_or_else(|| AppError::from_code(ProblemCode::InvalidInput))?,
        ),
    };
    let (user, _user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::TasksWrite),
        Some(workspace_id),
    )
    .await?;
    let actor_user_id = parse_user_id(&user.user_id)?;
    let result = add_task_dependency(
        &state.auth.db.pool,
        workspace_id,
        actor_user_id,
        session_id,
        task_id,
        body.blocked_id,
        requested_type,
        body.lag_days,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(()) => Ok(Json(OkResponse { ok: true })),
        Err(err) => Err(map_task_db_error(err)),
    }
}

async fn remove_dependency_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, task_id, blocked_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<Json<OkResponse>, TaskApiError> {
    check_origin(&headers, &state.public_origin)?;
    let (user, _user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::TasksWrite),
        Some(workspace_id),
    )
    .await?;
    let actor_user_id = parse_user_id(&user.user_id)?;
    let result = remove_task_dependency(
        &state.auth.db.pool,
        workspace_id,
        actor_user_id,
        session_id,
        task_id,
        blocked_id,
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
        && body.assignee_ids.is_none()
        && body.label_ids.is_none()
        && body.milestone_id.is_none()
    {
        return Err(AppError::from_code(ProblemCode::InvalidInput).into());
    }
    if body.assignee_ids.as_ref().is_some_and(|ids| ids.len() > 50)
        || body.label_ids.as_ref().is_some_and(|ids| ids.len() > 50)
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
        assignee_ids: body.assignee_ids.clone(),
        label_ids: body.label_ids.clone(),
        milestone_id: FieldUpdate::from_optional(body.milestone_id),
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
        ProjectDbError::DependencyCycle => TaskApiError::Coded {
            status: StatusCode::BAD_REQUEST,
            code: "dependency_cycle",
            title: "dependency cycle".to_string(),
        },
        ProjectDbError::DependencyContradiction => TaskApiError::Coded {
            status: StatusCode::BAD_REQUEST,
            code: "dependency_contradiction",
            title: "dependency contradiction".to_string(),
        },
        ProjectDbError::TaskCannotBlockItself => TaskApiError::Coded {
            status: StatusCode::BAD_REQUEST,
            code: "task_cannot_block_itself",
            title: "task cannot block itself".to_string(),
        },
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
    let (user, _user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::TasksRead),
        Some(workspace_id),
    )
    .await?;
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
                    meta: task_meta_output(task.meta),
                    assignee_ids: uuid_strings(&task.assignee_ids),
                    label_ids: uuid_strings(&task.label_ids),
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

pub(crate) fn task_meta_output(task: crate::db::tasks::TaskMetaRow) -> TaskMetaOutput {
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

fn uuid_strings(ids: &[Uuid]) -> Vec<String> {
    ids.iter().map(ToString::to_string).collect()
}

fn label_output(label: crate::db::labels::LabelRow) -> LabelOutput {
    LabelOutput {
        id: label.id.to_string(),
        project_id: label.project_id.to_string(),
        name: label.name,
        color: label.color,
    }
}

fn milestone_output(row: crate::db::milestones::MilestoneRow) -> MilestoneOutput {
    MilestoneOutput {
        id: row.id.to_string(),
        project_id: row.project_id.to_string(),
        name: row.name,
        due_date: row.due_date,
        sort_key: row.sort_key,
    }
}

fn dependency_output(edge: crate::db::tasks::TaskDependencyEdge) -> TaskDependencyOutput {
    TaskDependencyOutput {
        blocker_id: edge.blocker_id.to_string(),
        blocked_id: edge.blocked_id.to_string(),
        dependency_type: edge.dependency_type,
        lag_days: edge.lag_days,
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

fn activity_channel(headers: &HeaderMap) -> &'static str {
    if headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("Bearer "))
    {
        "api"
    } else {
        "web"
    }
}

async fn list_task_activity_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, task_id)): Path<(Uuid, Uuid)>,
    query: Result<Query<TaskActivityQueryParams>, QueryRejection>,
) -> Result<Json<ActivityListResponse>, TaskApiError> {
    let Query(query) = query.map_err(AppError::from)?;
    let Some(filter) = ActivityFilter::parse(query.filter.as_deref().unwrap_or("all")) else {
        return Err(AppError::from_code(ProblemCode::InvalidInput).into());
    };
    let (user, _user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::TasksRead),
        Some(workspace_id),
    )
    .await?;
    let actor_user_id = parse_user_id(&user.user_id)?;
    let result = list_task_activity(
        &state.auth.db.pool,
        workspace_id,
        actor_user_id,
        session_id,
        task_id,
        ActivityListQuery {
            filter,
            limit: query.limit.unwrap_or(50),
            cursor: query.cursor,
        },
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(page) => activity_page_output(page, actor_user_id).map(Json),
        Err(TaskActivityDbError::InvalidInput) => {
            Err(AppError::from_code(ProblemCode::InvalidInput).into())
        }
        Err(TaskActivityDbError::InvalidCursor) => Err(TaskApiError::Coded {
            status: StatusCode::BAD_REQUEST,
            code: "invalid_cursor",
            title: "invalid cursor".to_string(),
        }),
        Err(TaskActivityDbError::NotFound) => {
            Err(AppError::from_code(ProblemCode::NotFound).into())
        }
    }
}

/// Response size budget, as in the source: envelope headroom plus each
/// serialized item and a separator. The first item is always kept whole.
const ACTIVITY_RESPONSE_BUDGET_BYTES: usize = 1_048_576;
const ACTIVITY_ENVELOPE_BYTES: usize = 2048;

fn activity_page_output(
    page: TaskActivityListPage,
    viewer_id: Uuid,
) -> Result<ActivityListResponse, TaskApiError> {
    let total = page.items.len();
    let mut items = Vec::with_capacity(total);
    let mut last_position = None;
    let mut bytes = ACTIVITY_ENVELOPE_BYTES;
    for item in page.items {
        let position = item.position();
        let output = activity_item_output(item, viewer_id);
        let item_bytes = serde_json::to_vec(&output)
            .map_err(|err| {
                tracing::error!("activity item serialization failed: {err}");
                AppError::internal()
            })?
            .len()
            + 1;
        if !items.is_empty() && bytes + item_bytes > ACTIVITY_RESPONSE_BUDGET_BYTES {
            break;
        }
        bytes += item_bytes;
        items.push(output);
        last_position = Some(position);
    }
    let next_cursor = if page.has_more || items.len() < total {
        last_position.map(|(id, created_at, item_type)| {
            encode_activity_cursor(id, created_at, item_type, &page.scope)
        })
    } else {
        None
    };
    Ok(ActivityListResponse { items, next_cursor })
}

fn activity_item_output(item: TaskActivityOutputItem, viewer_id: Uuid) -> ActivityItemOutput {
    match item {
        TaskActivityOutputItem::Change(change) => ActivityItemOutput::Change {
            id: change.id,
            created_at: change.created_at,
            actor: change.actor.map(activity_actor_output),
            channel: change.channel,
            kind: change.kind,
            changes: change
                .changes
                .into_iter()
                .filter_map(|mut value| {
                    let field = value.get("field")?.as_str()?.to_string();
                    let from = value.get_mut("from").map(serde_json::Value::take);
                    let to = value.get_mut("to").map(serde_json::Value::take);
                    Some(ActivityChangeOutput {
                        field,
                        from: from.unwrap_or_default(),
                        to: to.unwrap_or_default(),
                    })
                })
                .collect(),
        },
        TaskActivityOutputItem::Comment(comment) => ActivityItemOutput::Comment {
            id: comment.comment.id,
            created_at: comment.comment.created_at,
            actor: comment.actor.map(activity_actor_output),
            comment: Box::new(comment_to_output(&comment.comment, viewer_id)),
            parent: comment.parent.map(|parent| ActivityCommentParentOutput {
                id: parent.id,
                body: parent.body,
                actor: parent.actor.map(activity_actor_output),
            }),
        },
    }
}

fn activity_actor_output(
    actor: crate::db::task_activity::ActivityActorOutput,
) -> ActivityActorOutput {
    ActivityActorOutput {
        id: actor.id,
        name: actor.name,
    }
}

async fn require_session(
    state: &AppState,
    headers: &HeaderMap,
    jar: &CookieJar,
    access: crate::http::authz::Access,
    workspace_id: Option<Uuid>,
) -> Result<(SessionUser, Uuid, Uuid), AppError> {
    let auth =
        crate::http::authz::require_request_auth(state, headers, jar, access, workspace_id).await?;
    Ok((auth.user, auth.user_id, auth.credential_id))
}

fn parse_user_id(value: &str) -> Result<Uuid, AppError> {
    Uuid::parse_str(value).map_err(|_| AppError::from_code(ProblemCode::AuthenticationRequired))
}

fn internal(err: sqlx::Error) -> AppError {
    tracing::error!("database error: {}", err);
    AppError::internal()
}
