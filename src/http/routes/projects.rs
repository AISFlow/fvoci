use std::net::SocketAddr;

use axum::extract::rejection::JsonRejection;
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch};
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use serde_json::json;
use uuid::Uuid;

use crate::api::dto::{
    AddProjectMemberBody, CreateProjectBody, MemberResponse, OkResponse, PatchProjectBody,
    ProjectListItemOutput, ProjectListResponse, ProjectMembersResponse, ProjectOutput,
    WorkflowOutput, WorkflowStatusOutput,
};
use crate::auth::session::SessionUser;
use crate::db::projects::{
    add_project_member, create_project, get_project, get_project_workflow, list_project_members,
    list_projects, remove_project_member, update_project, update_project_member_role,
    CreateProjectInput, ProjectDbError, UpdateProjectInput,
};
use crate::error::{AppError, ProblemCode, SESSION_COOKIE};
use crate::http::guard::{check_origin, reject_bearer};
use crate::http::rate_limit::peer_ip;
use crate::http::state::AppState;
use crate::projects::{
    description_is_valid, icon_is_valid, name_is_valid, normalize_project_key, ProjectKeyError,
    ProjectMemberRole,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/workspaces/{workspace_id}/projects",
            get(list_projects_route).post(create_project_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}",
            get(get_project_route).patch(patch_project_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/members",
            get(list_members).post(add_member),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/members/{user_id}",
            patch(patch_member).delete(delete_member),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/workflow",
            get(get_workflow),
        )
}

async fn create_project_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
    body: Result<Json<CreateProjectBody>, JsonRejection>,
) -> Result<Response, AppError> {
    let Json(body) = body.map_err(AppError::from)?;
    reject_bearer(&headers)?;
    check_origin(&headers, &state.public_origin)?;
    let key = normalize_project_key(&body.key).map_err(map_key_error)?;
    if !name_is_valid(&body.name) {
        return Err(AppError::from_code(ProblemCode::InvalidInput));
    }
    if body.visibility != "private" && body.visibility != "workspace" {
        return Err(AppError::from_code(ProblemCode::InvalidInput));
    }
    if !description_is_valid(body.description.as_deref()) || !icon_is_valid(body.icon.as_deref()) {
        return Err(AppError::from_code(ProblemCode::InvalidInput));
    }
    let (user, session_id) = require_session(&state, &jar).await?;
    let actor_user_id = parse_user_id(&user.user_id)?;
    let ip = peer_ip(peer.ip());
    let result = create_project(
        &state.auth.db.pool,
        workspace_id,
        actor_user_id,
        session_id,
        CreateProjectInput {
            key: &key,
            name: &body.name,
            visibility: &body.visibility,
            description: body.description.as_deref(),
            icon: body.icon.as_deref(),
            lead_user_id: body.lead_user_id,
        },
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(project) => {
            Ok((StatusCode::CREATED, Json(project_output(project, false))).into_response())
        }
        Err(err) => Err(map_project_error(err)),
    }
}

async fn list_projects_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
) -> Result<Json<ProjectListResponse>, AppError> {
    reject_bearer(&headers)?;
    let (user, session_id) = require_session(&state, &jar).await?;
    let actor_user_id = parse_user_id(&user.user_id)?;
    let result = list_projects(&state.auth.db.pool, workspace_id, actor_user_id, session_id)
        .await
        .map_err(internal)?;
    match result {
        Ok(items) => Ok(Json(ProjectListResponse {
            items: items
                .into_iter()
                .map(|item| ProjectListItemOutput {
                    id: item.project.id.to_string(),
                    workspace_id: item.project.workspace_id.to_string(),
                    key: item.project.key,
                    name: item.project.name,
                    description: item.project.description,
                    icon: item.project.icon,
                    visibility: item.project.visibility,
                    root_document_id: item.project.root_document_id.map(|id| id.to_string()),
                    status: item.project.status,
                    created_by: item.project.created_by.to_string(),
                    created_at: item.project.created_at,
                    updated_at: item.project.updated_at,
                    task_count: item.task_count,
                    open_task_count: item.open_task_count,
                })
                .collect(),
        })),
        Err(err) => Err(map_project_error(err)),
    }
}

async fn get_project_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<ProjectOutput>, AppError> {
    reject_bearer(&headers)?;
    let (user, session_id) = require_session(&state, &jar).await?;
    let actor_user_id = parse_user_id(&user.user_id)?;
    let result = get_project(
        &state.auth.db.pool,
        workspace_id,
        project_id,
        actor_user_id,
        session_id,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(project) => Ok(Json(project_output(project, false))),
        Err(err) => Err(map_project_error(err)),
    }
}

async fn patch_project_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id)): Path<(Uuid, Uuid)>,
    body: Result<Json<PatchProjectBody>, JsonRejection>,
) -> Result<Json<ProjectOutput>, AppError> {
    let Json(body) = body.map_err(AppError::from)?;
    reject_bearer(&headers)?;
    check_origin(&headers, &state.public_origin)?;
    if let Some(name) = body.name.as_deref() {
        if !name_is_valid(name) {
            return Err(AppError::from_code(ProblemCode::InvalidInput));
        }
    }
    if let Some(visibility) = body.visibility.as_deref() {
        if visibility != "private" && visibility != "workspace" {
            return Err(AppError::from_code(ProblemCode::InvalidInput));
        }
    }
    if let Some(description) = body.description.as_ref().and_then(|value| value.as_deref()) {
        if !description_is_valid(Some(description)) {
            return Err(AppError::from_code(ProblemCode::InvalidInput));
        }
    }
    if let Some(icon) = body.icon.as_ref().and_then(|value| value.as_deref()) {
        if !icon_is_valid(Some(icon)) {
            return Err(AppError::from_code(ProblemCode::InvalidInput));
        }
    }
    let (user, session_id) = require_session(&state, &jar).await?;
    let actor_user_id = parse_user_id(&user.user_id)?;
    let ip = peer_ip(peer.ip());
    let description = body.description.as_ref().map(|value| value.as_deref());
    let icon = body.icon.as_ref().map(|value| value.as_deref());
    let result = update_project(
        &state.auth.db.pool,
        workspace_id,
        project_id,
        actor_user_id,
        session_id,
        UpdateProjectInput {
            name: body.name.as_deref(),
            visibility: body.visibility.as_deref(),
            description,
            icon,
            lead_user_id: body.lead_user_id,
        },
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(project) => Ok(Json(project_output(project, false))),
        Err(err) => Err(map_project_error(err)),
    }
}

async fn list_members(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<ProjectMembersResponse>, AppError> {
    reject_bearer(&headers)?;
    let (user, session_id) = require_session(&state, &jar).await?;
    let actor_user_id = parse_user_id(&user.user_id)?;
    let result = list_project_members(
        &state.auth.db.pool,
        workspace_id,
        project_id,
        actor_user_id,
        session_id,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(members) => Ok(Json(ProjectMembersResponse {
            items: members
                .into_iter()
                .map(|member| MemberResponse {
                    user_id: member.user_id.to_string(),
                    email: member.email,
                    given_name: member.given_name,
                    family_name: member.family_name,
                    role: member.role.as_str().to_string(),
                })
                .collect(),
        })),
        Err(err) => Err(map_project_error(err)),
    }
}

async fn add_member(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id)): Path<(Uuid, Uuid)>,
    body: Result<Json<AddProjectMemberBody>, JsonRejection>,
) -> Result<Response, AppError> {
    let Json(body) = body.map_err(AppError::from)?;
    reject_bearer(&headers)?;
    check_origin(&headers, &state.public_origin)?;
    let role = ProjectMemberRole::parse(&body.role)
        .ok_or_else(|| AppError::from_code(ProblemCode::InvalidInput))?;
    let (user, session_id) = require_session(&state, &jar).await?;
    let actor_user_id = parse_user_id(&user.user_id)?;
    let ip = peer_ip(peer.ip());
    let result = add_project_member(
        &state.auth.db.pool,
        workspace_id,
        project_id,
        actor_user_id,
        session_id,
        body.user_id,
        role,
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(()) => Ok((StatusCode::CREATED, Json(OkResponse { ok: true })).into_response()),
        Err(err) => Err(map_project_error(err)),
    }
}

async fn patch_member(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id, target_user_id)): Path<(Uuid, Uuid, Uuid)>,
    body: Result<Json<crate::api::dto::MemberRoleBody>, JsonRejection>,
) -> Result<Json<OkResponse>, AppError> {
    let Json(body) = body.map_err(AppError::from)?;
    reject_bearer(&headers)?;
    check_origin(&headers, &state.public_origin)?;
    let role = ProjectMemberRole::parse(&body.role)
        .ok_or_else(|| AppError::from_code(ProblemCode::InvalidInput))?;
    let (user, session_id) = require_session(&state, &jar).await?;
    let actor_user_id = parse_user_id(&user.user_id)?;
    let ip = peer_ip(peer.ip());
    let result = update_project_member_role(
        &state.auth.db.pool,
        workspace_id,
        project_id,
        actor_user_id,
        session_id,
        target_user_id,
        role,
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(()) => Ok(Json(OkResponse { ok: true })),
        Err(err) => Err(map_project_error(err)),
    }
}

async fn delete_member(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id, target_user_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<Json<OkResponse>, AppError> {
    reject_bearer(&headers)?;
    check_origin(&headers, &state.public_origin)?;
    let (user, session_id) = require_session(&state, &jar).await?;
    let actor_user_id = parse_user_id(&user.user_id)?;
    let ip = peer_ip(peer.ip());
    let result = remove_project_member(
        &state.auth.db.pool,
        workspace_id,
        project_id,
        actor_user_id,
        session_id,
        target_user_id,
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(()) => Ok(Json(OkResponse { ok: true })),
        Err(err) => Err(map_project_error(err)),
    }
}

async fn get_workflow(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<WorkflowOutput>, AppError> {
    reject_bearer(&headers)?;
    let (user, session_id) = require_session(&state, &jar).await?;
    let actor_user_id = parse_user_id(&user.user_id)?;
    let result = get_project_workflow(
        &state.auth.db.pool,
        workspace_id,
        project_id,
        actor_user_id,
        session_id,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(workflow) => Ok(Json(WorkflowOutput {
            id: workflow.id.to_string(),
            project_id: workflow.project_id.to_string(),
            statuses: workflow
                .statuses
                .into_iter()
                .map(|status| WorkflowStatusOutput {
                    id: status.id.to_string(),
                    workflow_id: workflow.id.to_string(),
                    name: status.name,
                    category: status.category,
                    sort_key: status.sort_key,
                    wip_limit: status.wip_limit,
                })
                .collect(),
        })),
        Err(err) => Err(map_project_error(err)),
    }
}

fn project_output(project: crate::db::projects::ProjectRow, include_counts: bool) -> ProjectOutput {
    let _ = include_counts;
    ProjectOutput {
        id: project.id.to_string(),
        workspace_id: project.workspace_id.to_string(),
        key: project.key,
        name: project.name,
        description: project.description,
        icon: project.icon,
        visibility: project.visibility,
        root_document_id: project.root_document_id.map(|id| id.to_string()),
        status: project.status,
        created_by: project.created_by.to_string(),
        created_at: project.created_at,
        updated_at: project.updated_at,
    }
}

fn map_key_error(err: ProjectKeyError) -> AppError {
    match err {
        ProjectKeyError::Reserved => AppError {
            status: StatusCode::BAD_REQUEST,
            code: ProblemCode::InvalidInput,
            source: None,
            params: Some(json!({"code":"project.key.reserved"})),
            retry_after: None,
        },
        ProjectKeyError::InvalidPattern => AppError::from_code(ProblemCode::InvalidInput),
    }
}

pub fn map_project_error(err: ProjectDbError) -> AppError {
    match err {
        ProjectDbError::NotFound | ProjectDbError::Forbidden => {
            AppError::from_code(ProblemCode::NotFound)
        }
        ProjectDbError::Conflict | ProjectDbError::LastLead | ProjectDbError::GuestLead => {
            AppError::from_code(ProblemCode::Conflict)
        }
        ProjectDbError::LeadNotMember => AppError::from_code(ProblemCode::Conflict),
        ProjectDbError::Archived => AppError::from_code(ProblemCode::ProjectArchived),
        ProjectDbError::VersionConflict => AppError {
            status: StatusCode::CONFLICT,
            code: ProblemCode::Conflict,
            source: None,
            params: Some(json!({"code":"document_version_mismatch"})),
            retry_after: None,
        },
        ProjectDbError::TaskArchived | ProjectDbError::InvalidAnchor => {
            AppError::from_code(ProblemCode::Conflict)
        }
        ProjectDbError::StatusNotInWorkflow => AppError {
            status: StatusCode::BAD_REQUEST,
            code: ProblemCode::InvalidInput,
            source: None,
            params: Some(json!({"code":"status_not_in_project_workflow"})),
            retry_after: None,
        },
        ProjectDbError::WipLimitExceeded => AppError {
            status: StatusCode::CONFLICT,
            code: ProblemCode::Conflict,
            source: None,
            params: Some(json!({"code":"wip_limit_exceeded"})),
            retry_after: None,
        },
        ProjectDbError::InvalidMoveAnchors => AppError::from_code(ProblemCode::InvalidInput),
        ProjectDbError::InvalidCursor => AppError {
            status: StatusCode::BAD_REQUEST,
            code: ProblemCode::InvalidInput,
            source: None,
            params: Some(json!({"code":"invalid_cursor"})),
            retry_after: None,
        },
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
