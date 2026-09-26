//! Task routes beyond the metadata CRUD in `tasks.rs` (source
//! `apps/server/src/domains/tasks/{routes,time-entries,task-config}.ts`):
//! time entries, clone, backlinks, purge, the flat `/tasks/:taskId` routes,
//! parent candidates, the workspace task/status lists and workflow statuses.

use std::net::SocketAddr;

use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use uuid::Uuid;

use crate::api::dto::{
    OkResponse, PatchTaskBody, TaskListItemOutput, TaskListResponse, TaskMetaOutput, TaskOutput,
    TaskStatusCountOutput, WorkflowStatusOutput, WorkspaceStatusOutput,
};
use crate::api::tasks_dto::{
    BacklinkFromResponse, BacklinkItemResponse, BacklinkListResponse, StatusCreateBody,
    StatusPatchBody, TaskCloneOutput, TaskParentCandidateOutput, TaskParentListResponse,
    TaskParentQueryParams, TimeEntryCreateBody, TimeEntryListResponse, TimeEntryOutput,
    TimeEntryRollupResponse, WorkspaceStatusListResponse, WorkspaceTaskListQueryParams,
};
use crate::auth::scopes::ApiTokenScope;
use crate::db::task_ops::{
    clone_task, create_time_entry, list_task_backlinks, list_task_parents, list_time_entries,
    locate_task, purge_task, CreateTimeEntryInput, TaskParentQuery, TimeEntryRow,
    TASK_PARENT_PAGE_MAX,
};
use crate::db::tasks::{get_task, list_workspace_tasks, patch_task_meta};
use crate::db::workflow_statuses::{
    create_status, delete_status, list_workspace_statuses, status_category_is_valid,
    status_name_is_valid, update_status, CreateStatusInput, StatusAnchor, StatusRow,
    UpdateStatusInput,
};
use crate::error::{AppError, ProblemCode};
use crate::http::authz::Access;
use crate::http::guard::check_origin;
use crate::http::rate_limit::peer_ip;
use crate::http::routes::tasks::{
    activity_channel, internal, map_task_db_error, map_task_list_query_error, parse_patch_body,
    parse_user_id, require_session, task_detail_output, task_meta_output, TaskApiError,
};
use crate::http::state::AppState;
use crate::tasks::list_query::parse_task_list_query;
use crate::tasks::{parse_iso_datetime, task_type_is_valid};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/workspaces/{workspace_id}/tasks",
            get(list_workspace_tasks_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/statuses",
            get(list_workspace_statuses_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks/parents",
            get(list_task_parents_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/time-entries",
            get(list_time_entries_route).post(create_time_entry_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/time-entries/rollup",
            get(time_entries_rollup_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/clone",
            post(clone_task_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/backlinks",
            get(list_task_backlinks_route),
        )
        .route(
            "/api/v1/tasks/{task_id}",
            get(get_flat_task_route)
                .patch(patch_flat_task_route)
                .delete(delete_flat_task_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/workflows/{workflow_id}/statuses",
            post(create_status_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/workflows/{workflow_id}/statuses/{status_id}",
            patch(update_status_route).delete(delete_status_route),
        )
}

// ---------------------------------------------------------------------------
// Time entries
// ---------------------------------------------------------------------------

fn time_entry_output(row: TimeEntryRow) -> TimeEntryOutput {
    TimeEntryOutput {
        id: row.id.to_string(),
        workspace_id: row.workspace_id.to_string(),
        task_id: row.task_id.to_string(),
        user_id: row.user_id.to_string(),
        started_at: row.started_at,
        ended_at: row.ended_at,
        duration_seconds: row.duration_seconds,
        note: row.note,
    }
}

/// JS `Date` keeps milliseconds; the stored range and its duration use the
/// same precision so the duration check matches the columns exactly.
fn parse_entry_time(value: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let at = parse_iso_datetime(value)?;
    let millis = at.timestamp_millis();
    chrono::DateTime::from_timestamp_millis(millis)
}

async fn list_time_entries_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, task_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<TimeEntryListResponse>, TaskApiError> {
    let (user, _, session_id) = require_session(
        &state,
        &headers,
        &jar,
        Access::Scope(ApiTokenScope::TasksRead),
        Some(workspace_id),
    )
    .await?;
    let actor = parse_user_id(&user.user_id)?;
    let result = list_time_entries(
        &state.auth.db.pool,
        workspace_id,
        task_id,
        actor,
        session_id,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(list) => Ok(Json(TimeEntryListResponse {
            can_create: list.can_create,
            items: list.items.into_iter().map(time_entry_output).collect(),
        })),
        Err(err) => Err(map_task_db_error(err)),
    }
}

async fn create_time_entry_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, task_id)): Path<(Uuid, Uuid)>,
    body: Result<Json<TimeEntryCreateBody>, JsonRejection>,
) -> Result<Response, TaskApiError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    let invalid = || TaskApiError::from(AppError::from_code(ProblemCode::InvalidInput));
    let started_at = parse_entry_time(&body.started_at).ok_or_else(invalid)?;
    let ended_at = match body.ended_at.as_deref() {
        None => None,
        Some(raw) => Some(parse_entry_time(raw).ok_or_else(invalid)?),
    };
    if ended_at.is_some_and(|ended_at| ended_at <= started_at) {
        return Err(invalid());
    }
    if body
        .note
        .as_deref()
        .is_some_and(|note| note.encode_utf16().count() > 2000)
    {
        return Err(invalid());
    }
    let (user, _, session_id) = require_session(
        &state,
        &headers,
        &jar,
        Access::Scope(ApiTokenScope::TasksWrite),
        Some(workspace_id),
    )
    .await?;
    let actor = parse_user_id(&user.user_id)?;
    let result = create_time_entry(
        &state.auth.db.pool,
        workspace_id,
        task_id,
        actor,
        session_id,
        CreateTimeEntryInput {
            started_at,
            ended_at,
            note: body.note,
        },
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(row) => Ok((StatusCode::CREATED, Json(time_entry_output(row))).into_response()),
        Err(err) => Err(map_task_db_error(err)),
    }
}

async fn time_entries_rollup_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, task_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<TimeEntryRollupResponse>, TaskApiError> {
    let (user, _, session_id) = require_session(
        &state,
        &headers,
        &jar,
        Access::Scope(ApiTokenScope::TasksRead),
        Some(workspace_id),
    )
    .await?;
    let actor = parse_user_id(&user.user_id)?;
    let result = list_time_entries(
        &state.auth.db.pool,
        workspace_id,
        task_id,
        actor,
        session_id,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(list) => Ok(Json(TimeEntryRollupResponse {
            total_seconds: list
                .items
                .iter()
                .map(|row| i64::from(row.duration_seconds.unwrap_or(0)))
                .sum(),
            open: list.items.iter().any(|row| row.ended_at.is_none()),
        })),
        Err(err) => Err(map_task_db_error(err)),
    }
}

// ---------------------------------------------------------------------------
// Clone, backlinks, purge
// ---------------------------------------------------------------------------

async fn clone_task_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, task_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<TaskCloneOutput>, TaskApiError> {
    check_origin(&headers, &state.public_origin)?;
    let (user, _, session_id) = require_session(
        &state,
        &headers,
        &jar,
        Access::Scope(ApiTokenScope::TasksWrite),
        Some(workspace_id),
    )
    .await?;
    let actor = parse_user_id(&user.user_id)?;
    let ip = peer_ip(peer.ip());
    let result = clone_task(
        &state.auth.db.pool,
        workspace_id,
        task_id,
        actor,
        session_id,
        Some(&ip),
        activity_channel(&headers),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(cloned) => Ok(Json(TaskCloneOutput {
            meta: task_meta_output(cloned.meta),
            display_id: cloned.display_id,
        })),
        Err(err) => Err(map_task_db_error(err)),
    }
}

async fn list_task_backlinks_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, task_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<BacklinkListResponse>, TaskApiError> {
    let (user, _, session_id) = require_session(
        &state,
        &headers,
        &jar,
        Access::Scope(ApiTokenScope::TasksRead),
        Some(workspace_id),
    )
    .await?;
    let actor = parse_user_id(&user.user_id)?;
    let result = list_task_backlinks(
        &state.auth.db.pool,
        workspace_id,
        task_id,
        actor,
        session_id,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(items) => Ok(Json(BacklinkListResponse {
            items: items
                .into_iter()
                .map(|item| BacklinkItemResponse {
                    id: item.id.to_string(),
                    from: BacklinkFromResponse {
                        r#type: item.kind.as_str().to_string(),
                        id: item.id.to_string(),
                        title: item.title,
                        display_id: item.display_id,
                    },
                })
                .collect(),
        })),
        Err(err) => Err(map_task_db_error(err)),
    }
}

/// `DELETE /workspaces/:ws/tasks/:id` (source `tasks.remove`): session only,
/// since API tokens may only take reversible actions.
pub(crate) async fn purge_task_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, task_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<OkResponse>, TaskApiError> {
    check_origin(&headers, &state.public_origin)?;
    let (user, _, session_id) =
        require_session(&state, &headers, &jar, Access::Session, Some(workspace_id)).await?;
    let actor = parse_user_id(&user.user_id)?;
    let ip = peer_ip(peer.ip());
    purge(&state, workspace_id, task_id, actor, session_id, &ip).await
}

async fn purge(
    state: &AppState,
    workspace_id: Uuid,
    task_id: Uuid,
    actor: Uuid,
    session_id: Uuid,
    ip: &str,
) -> Result<Json<OkResponse>, TaskApiError> {
    let result = purge_task(
        &state.auth.db.pool,
        workspace_id,
        task_id,
        actor,
        session_id,
        Some(ip),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(_) => Ok(Json(OkResponse { ok: true })),
        Err(err) => Err(map_task_db_error(err)),
    }
}

// ---------------------------------------------------------------------------
// Flat `/tasks/:taskId` (session only)
// ---------------------------------------------------------------------------

async fn flat_actor(
    state: &AppState,
    headers: &HeaderMap,
    jar: &CookieJar,
    task_id: Uuid,
) -> Result<(Uuid, Uuid, Uuid), TaskApiError> {
    let (user, _, session_id) = require_session(state, headers, jar, Access::Session, None).await?;
    let actor = parse_user_id(&user.user_id)?;
    let Some(workspace_id) = locate_task(&state.auth.db.pool, actor, task_id)
        .await
        .map_err(internal)?
    else {
        return Err(AppError::from_code(ProblemCode::NotFound).into());
    };
    Ok((workspace_id, actor, session_id))
}

async fn get_flat_task_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(task_id): Path<Uuid>,
) -> Result<Json<TaskOutput>, TaskApiError> {
    let (workspace_id, actor, session_id) = flat_actor(&state, &headers, &jar, task_id).await?;
    let result = get_task(
        &state.auth.db.pool,
        workspace_id,
        task_id,
        actor,
        session_id,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(task) => Ok(Json(task_detail_output(task))),
        Err(err) => Err(map_task_db_error(err)),
    }
}

async fn patch_flat_task_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(task_id): Path<Uuid>,
    body: Result<Json<PatchTaskBody>, JsonRejection>,
) -> Result<Json<TaskMetaOutput>, TaskApiError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    let input = parse_patch_body(&body)?;
    let (workspace_id, actor, session_id) = flat_actor(&state, &headers, &jar, task_id).await?;
    let ip = peer_ip(peer.ip());
    let result = patch_task_meta(
        &state.auth.db.pool,
        workspace_id,
        task_id,
        actor,
        session_id,
        input,
        Some(&ip),
        "web",
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(task) => Ok(Json(task_meta_output(task))),
        Err(err) => Err(map_task_db_error(err)),
    }
}

async fn delete_flat_task_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(task_id): Path<Uuid>,
) -> Result<Json<OkResponse>, TaskApiError> {
    check_origin(&headers, &state.public_origin)?;
    let (workspace_id, actor, session_id) = flat_actor(&state, &headers, &jar, task_id).await?;
    let ip = peer_ip(peer.ip());
    purge(&state, workspace_id, task_id, actor, session_id, &ip).await
}

// ---------------------------------------------------------------------------
// Parent candidates and workspace lists
// ---------------------------------------------------------------------------

fn invalid_input() -> TaskApiError {
    AppError::from_code(ProblemCode::InvalidInput).into()
}

fn parse_parent_query(params: TaskParentQueryParams) -> Result<TaskParentQuery, TaskApiError> {
    let q = params.q.unwrap_or_default().trim().to_string();
    if q.encode_utf16().count() > 200 {
        return Err(invalid_input());
    }
    let child_type = params.child_type.ok_or_else(invalid_input)?;
    if !task_type_is_valid(&child_type) {
        return Err(invalid_input());
    }
    let exclude_task_id = match params.exclude_task_id.as_deref() {
        None => None,
        Some(raw) => Some(Uuid::parse_str(raw).map_err(|_| invalid_input())?),
    };
    if params.cursor.as_deref().is_some_and(|c| c.len() > 1024) {
        return Err(invalid_input());
    }
    let limit = match params.limit.as_deref() {
        None => TASK_PARENT_PAGE_MAX,
        Some(raw) => {
            let value: f64 = raw.trim().parse().map_err(|_| invalid_input())?;
            if value.fract() != 0.0 || !(1.0..=TASK_PARENT_PAGE_MAX as f64).contains(&value) {
                return Err(invalid_input());
            }
            value as i64
        }
    };
    Ok(TaskParentQuery {
        q,
        child_type,
        exclude_task_id,
        cursor: params.cursor,
        limit,
    })
}

async fn list_task_parents_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id)): Path<(Uuid, Uuid)>,
    query: Result<Query<TaskParentQueryParams>, QueryRejection>,
) -> Result<Json<TaskParentListResponse>, TaskApiError> {
    let Query(params) = query.map_err(AppError::from)?;
    let parsed = parse_parent_query(params)?;
    let (user, _, session_id) = require_session(
        &state,
        &headers,
        &jar,
        Access::Scope(ApiTokenScope::TasksRead),
        Some(workspace_id),
    )
    .await?;
    let actor = parse_user_id(&user.user_id)?;
    let result = list_task_parents(
        &state.auth.db.pool,
        workspace_id,
        project_id,
        actor,
        session_id,
        &parsed,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(page) => Ok(Json(TaskParentListResponse {
            items: page
                .items
                .into_iter()
                .map(|item| TaskParentCandidateOutput {
                    id: item.id.to_string(),
                    title: item.title,
                    display_id: item.display_id,
                    task_type: item.task_type,
                })
                .collect(),
            next_cursor: page.next_cursor,
        })),
        Err(err) => Err(map_task_db_error(err)),
    }
}

async fn list_workspace_tasks_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
    query: Result<Query<WorkspaceTaskListQueryParams>, QueryRejection>,
) -> Result<Json<TaskListResponse>, TaskApiError> {
    let Query(params) = query.map_err(AppError::from)?;
    let parsed = parse_task_list_query(
        params.query.as_deref(),
        None,
        params.cursor.as_deref(),
        params.limit,
        None,
        None,
    )
    .map_err(map_task_list_query_error)?;
    let (user, _, session_id) = require_session(
        &state,
        &headers,
        &jar,
        Access::Scope(ApiTokenScope::TasksRead),
        Some(workspace_id),
    )
    .await?;
    let actor = parse_user_id(&user.user_id)?;
    let result = list_workspace_tasks(
        &state.auth.db.pool,
        workspace_id,
        actor,
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
                .map(|item| TaskListItemOutput {
                    meta: task_meta_output(item.meta),
                    assignee_ids: item.assignee_ids.iter().map(Uuid::to_string).collect(),
                    label_ids: item.label_ids.iter().map(Uuid::to_string).collect(),
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

async fn list_workspace_statuses_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
) -> Result<Json<WorkspaceStatusListResponse>, TaskApiError> {
    let (user, _, session_id) = require_session(
        &state,
        &headers,
        &jar,
        Access::Scope(ApiTokenScope::TasksRead),
        Some(workspace_id),
    )
    .await?;
    let actor = parse_user_id(&user.user_id)?;
    let result = list_workspace_statuses(&state.auth.db.pool, workspace_id, actor, session_id)
        .await
        .map_err(internal)?;
    match result {
        Ok(rows) => Ok(Json(WorkspaceStatusListResponse {
            items: rows
                .into_iter()
                .map(|row| WorkspaceStatusOutput {
                    id: row.id.to_string(),
                    workflow_id: row.workflow_id.to_string(),
                    project_id: row.project_id.to_string(),
                    name: row.name,
                    sort_key: row.sort_key,
                    category: row.category,
                    wip_limit: row.wip_limit,
                })
                .collect(),
        })),
        Err(err) => Err(map_task_db_error(err)),
    }
}

// ---------------------------------------------------------------------------
// Workflow statuses
// ---------------------------------------------------------------------------

fn status_output(row: StatusRow) -> WorkflowStatusOutput {
    WorkflowStatusOutput {
        id: row.id.to_string(),
        workflow_id: row.workflow_id.to_string(),
        name: row.name,
        category: row.category,
        sort_key: row.sort_key,
        wip_limit: row.wip_limit,
    }
}

/// Source `z.number().int().min(1)`; the column is a 32-bit integer.
fn parse_wip_limit(value: Option<serde_json::Number>) -> Result<Option<i32>, TaskApiError> {
    let Some(number) = value else {
        return Ok(None);
    };
    let as_float = number.as_f64().ok_or_else(invalid_input)?;
    if as_float.fract() != 0.0 || as_float < 1.0 || as_float > f64::from(i32::MAX) {
        return Err(invalid_input());
    }
    Ok(Some(as_float as i32))
}

fn parse_status_name(raw: &str) -> Result<String, TaskApiError> {
    let trimmed = raw.trim();
    if !status_name_is_valid(trimmed) {
        return Err(invalid_input());
    }
    Ok(trimmed.to_string())
}

async fn create_status_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, workflow_id)): Path<(Uuid, Uuid)>,
    body: Result<Json<StatusCreateBody>, JsonRejection>,
) -> Result<Response, TaskApiError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    let name = parse_status_name(&body.name)?;
    if !status_category_is_valid(&body.category) {
        return Err(invalid_input());
    }
    let wip_limit = parse_wip_limit(body.wip_limit.flatten())?;
    let (user, _, session_id) = require_session(
        &state,
        &headers,
        &jar,
        Access::Scope(ApiTokenScope::TasksWrite),
        Some(workspace_id),
    )
    .await?;
    let actor = parse_user_id(&user.user_id)?;
    let result = create_status(
        &state.auth.db.pool,
        workspace_id,
        workflow_id,
        actor,
        session_id,
        CreateStatusInput {
            name,
            category: body.category,
            wip_limit,
        },
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(row) => Ok((StatusCode::CREATED, Json(status_output(row))).into_response()),
        Err(err) => Err(map_task_db_error(err)),
    }
}

async fn update_status_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, workflow_id, status_id)): Path<(Uuid, Uuid, Uuid)>,
    body: Result<Json<StatusPatchBody>, JsonRejection>,
) -> Result<Json<WorkflowStatusOutput>, TaskApiError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    if body.name.is_none()
        && body.category.is_none()
        && body.wip_limit.is_none()
        && body.before_id.is_none()
        && body.after_id.is_none()
    {
        return Err(invalid_input());
    }
    let anchor = match (body.before_id, body.after_id) {
        (Some(_), Some(_)) => return Err(invalid_input()),
        (Some(id), None) => Some(StatusAnchor::Before(id)),
        (None, Some(id)) => Some(StatusAnchor::After(id)),
        (None, None) => None,
    };
    let name = body.name.as_deref().map(parse_status_name).transpose()?;
    if body
        .category
        .as_deref()
        .is_some_and(|category| !status_category_is_valid(category))
    {
        return Err(invalid_input());
    }
    let wip_limit = match body.wip_limit {
        None => None,
        Some(value) => Some(parse_wip_limit(value)?),
    };
    let (user, _, session_id) = require_session(
        &state,
        &headers,
        &jar,
        Access::Scope(ApiTokenScope::TasksWrite),
        Some(workspace_id),
    )
    .await?;
    let actor = parse_user_id(&user.user_id)?;
    let result = update_status(
        &state.auth.db.pool,
        workspace_id,
        workflow_id,
        status_id,
        actor,
        session_id,
        UpdateStatusInput {
            name,
            category: body.category,
            wip_limit,
            anchor,
        },
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(row) => Ok(Json(status_output(row))),
        Err(err) => Err(map_task_db_error(err)),
    }
}

async fn delete_status_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, workflow_id, status_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<Json<OkResponse>, TaskApiError> {
    check_origin(&headers, &state.public_origin)?;
    let (user, _, session_id) = require_session(
        &state,
        &headers,
        &jar,
        Access::Scope(ApiTokenScope::TasksWrite),
        Some(workspace_id),
    )
    .await?;
    let actor = parse_user_id(&user.user_id)?;
    let result = delete_status(
        &state.auth.db.pool,
        workspace_id,
        workflow_id,
        status_id,
        actor,
        session_id,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(()) => Ok(Json(OkResponse { ok: true })),
        Err(err) => Err(map_task_db_error(err)),
    }
}
