use std::net::SocketAddr;

use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::api::dto::{
    AddProjectMemberBody, CloneProjectBody, CreateProjectBody, MemberResponse, OkResponse,
    PatchProjectBody, ProjectListItemOutput, ProjectListResponse, ProjectMembersResponse,
    ProjectOutput, WorkflowOutput, WorkflowStatusOutput,
};
use crate::auth::session::SessionUser;
use crate::db::projects::{
    add_project_member, clone_project, create_project_backend as create_project, get_project,
    get_project_workflow, list_deleted_projects, list_project_members, list_projects,
    remove_project_member, restore_project, set_project_archived, trash_project, update_project,
    update_project_member_role, CloneProjectInput, CreateProjectInput, ProjectDbError,
    UpdateProjectInput,
};
use crate::error::{AppError, ProblemCode};
use crate::http::guard::check_origin;
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
            get(get_project_route)
                .patch(patch_project_route)
                .delete(delete_project_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/archive",
            post(archive_project_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/unarchive",
            post(unarchive_project_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/restore",
            post(restore_project_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/clone",
            post(clone_project_route),
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
    let (user, _user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::ProjectsManage),
        Some(workspace_id),
    )
    .await?;
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

async fn clone_project_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id)): Path<(Uuid, Uuid)>,
    body: Result<Json<CloneProjectBody>, JsonRejection>,
) -> Result<Response, AppError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    let key = normalize_project_key(&body.key).map_err(map_key_error)?;
    if !name_is_valid(&body.name) {
        return Err(AppError::from_code(ProblemCode::InvalidInput));
    }
    if let Some(visibility) = body.visibility.as_deref() {
        if visibility != "private" && visibility != "workspace" {
            return Err(AppError::from_code(ProblemCode::InvalidInput));
        }
    }
    if let Some(Some(description)) = body.description.as_ref() {
        if !description_is_valid(Some(description)) {
            return Err(AppError::from_code(ProblemCode::InvalidInput));
        }
    }
    if let Some(Some(icon)) = body.icon.as_ref() {
        if !icon_is_valid(Some(icon)) {
            return Err(AppError::from_code(ProblemCode::InvalidInput));
        }
    }
    let (user, _user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::ProjectsManage),
        Some(workspace_id),
    )
    .await?;
    let actor_user_id = parse_user_id(&user.user_id)?;
    let ip = peer_ip(peer.ip());
    let result = clone_project(
        state
            .auth
            .db
            .pool
            .postgres("src/http/routes/projects.rs")
            .map_err(internal)?,
        workspace_id,
        project_id,
        actor_user_id,
        session_id,
        CloneProjectInput {
            key: &key,
            name: &body.name,
            visibility: body.visibility.as_deref(),
            description: body.description.as_ref().map(|value| value.as_deref()),
            icon: body.icon.as_ref().map(|value| value.as_deref()),
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectListQuery {
    deleted: Option<String>,
}

async fn list_projects_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
    query: Result<Query<ProjectListQuery>, QueryRejection>,
) -> Result<Json<ProjectListResponse>, AppError> {
    let Query(query) = query.map_err(AppError::from)?;
    let deleted = match query.deleted.as_deref() {
        None | Some("false") => false,
        Some("true") => true,
        Some(_) => return Err(AppError::from_code(ProblemCode::InvalidInput)),
    };
    let auth = crate::http::authz::require_request_auth(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::ProjectsRead),
        Some(workspace_id),
    )
    .await?;
    let actor_user_id = auth.user_id;
    let session_id = auth.credential_id;
    let expose_document_count = auth.token_scopes.as_deref().is_none_or(|scopes| {
        crate::auth::scopes::grants_api_token_scope(
            scopes,
            crate::auth::scopes::ApiTokenScope::DocumentsRead,
        )
    });
    if deleted {
        // Source `listDeletedProjects`: task counts are zero and rows are not editable.
        // Document counts are unavailable for deleted projects.
        let result = list_deleted_projects(
            state
                .auth
                .db
                .pool
                .postgres("src/http/routes/projects.rs")
                .map_err(internal)?,
            workspace_id,
            actor_user_id,
            session_id,
        )
        .await
        .map_err(internal)?;
        return match result {
            Ok(rows) => Ok(Json(ProjectListResponse {
                items: rows
                    .into_iter()
                    .map(|project| ProjectListItemOutput {
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
                        document_count: None,
                        task_count: 0,
                        open_task_count: 0,
                        can_edit: false,
                        can_manage: false,
                    })
                    .collect(),
            })),
            Err(err) => Err(map_project_error(err)),
        };
    }
    let result = list_projects(
        state
            .auth
            .db
            .pool
            .postgres("src/http/routes/projects.rs")
            .map_err(internal)?,
        workspace_id,
        actor_user_id,
        session_id,
    )
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
                    document_count: expose_document_count.then_some(item.document_count),
                    task_count: item.task_count,
                    open_task_count: item.open_task_count,
                    can_edit: item.can_edit,
                    can_manage: item.can_manage,
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
    let (user, _user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::ProjectsRead),
        Some(workspace_id),
    )
    .await?;
    let actor_user_id = parse_user_id(&user.user_id)?;
    let result = get_project(
        state
            .auth
            .db
            .pool
            .postgres("src/http/routes/projects.rs")
            .map_err(internal)?,
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
    let (user, _user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::ProjectsManage),
        Some(workspace_id),
    )
    .await?;
    let actor_user_id = parse_user_id(&user.user_id)?;
    let ip = peer_ip(peer.ip());
    let description = body.description.as_ref().map(|value| value.as_deref());
    let icon = body.icon.as_ref().map(|value| value.as_deref());
    let result = update_project(
        state
            .auth
            .db
            .pool
            .postgres("src/http/routes/projects.rs")
            .map_err(internal)?,
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

async fn delete_project_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<OkResponse>, AppError> {
    check_origin(&headers, &state.public_origin)?;
    let (user, _user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::ProjectsManage),
        Some(workspace_id),
    )
    .await?;
    let actor_user_id = parse_user_id(&user.user_id)?;
    let ip = peer_ip(peer.ip());
    let result = trash_project(
        state
            .auth
            .db
            .pool
            .postgres("src/http/routes/projects.rs")
            .map_err(internal)?,
        workspace_id,
        project_id,
        actor_user_id,
        session_id,
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(()) => Ok(Json(OkResponse { ok: true })),
        Err(err) => Err(map_project_error(err)),
    }
}

async fn archive_project_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<OkResponse>, AppError> {
    set_archived_route(&state, peer, &headers, &jar, workspace_id, project_id, true).await
}

async fn unarchive_project_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<OkResponse>, AppError> {
    set_archived_route(
        &state,
        peer,
        &headers,
        &jar,
        workspace_id,
        project_id,
        false,
    )
    .await
}

async fn set_archived_route(
    state: &AppState,
    peer: SocketAddr,
    headers: &HeaderMap,
    jar: &CookieJar,
    workspace_id: Uuid,
    project_id: Uuid,
    archived: bool,
) -> Result<Json<OkResponse>, AppError> {
    check_origin(headers, &state.public_origin)?;
    let (user, _user_id, session_id) = require_session(
        state,
        headers,
        jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::ProjectsManage),
        Some(workspace_id),
    )
    .await?;
    let actor_user_id = parse_user_id(&user.user_id)?;
    let ip = peer_ip(peer.ip());
    let result = set_project_archived(
        state
            .auth
            .db
            .pool
            .postgres("src/http/routes/projects.rs")
            .map_err(internal)?,
        workspace_id,
        project_id,
        actor_user_id,
        session_id,
        archived,
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(()) => Ok(Json(OkResponse { ok: true })),
        Err(err) => Err(map_project_error(err)),
    }
}

async fn restore_project_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<ProjectOutput>, AppError> {
    check_origin(&headers, &state.public_origin)?;
    let (user, _user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::ProjectsManage),
        Some(workspace_id),
    )
    .await?;
    let actor_user_id = parse_user_id(&user.user_id)?;
    let ip = peer_ip(peer.ip());
    let result = restore_project(
        state
            .auth
            .db
            .pool
            .postgres("src/http/routes/projects.rs")
            .map_err(internal)?,
        workspace_id,
        project_id,
        actor_user_id,
        session_id,
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
    let (user, _user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::ProjectsRead),
        Some(workspace_id),
    )
    .await?;
    let actor_user_id = parse_user_id(&user.user_id)?;
    let result = list_project_members(
        state
            .auth
            .db
            .pool
            .postgres("src/http/routes/projects.rs")
            .map_err(internal)?,
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
    check_origin(&headers, &state.public_origin)?;
    let role = ProjectMemberRole::parse(&body.role)
        .ok_or_else(|| AppError::from_code(ProblemCode::InvalidInput))?;
    let (user, _user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::ProjectsManage),
        Some(workspace_id),
    )
    .await?;
    let actor_user_id = parse_user_id(&user.user_id)?;
    let ip = peer_ip(peer.ip());
    let result = add_project_member(
        state
            .auth
            .db
            .pool
            .postgres("src/http/routes/projects.rs")
            .map_err(internal)?,
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
    check_origin(&headers, &state.public_origin)?;
    let role = ProjectMemberRole::parse(&body.role)
        .ok_or_else(|| AppError::from_code(ProblemCode::InvalidInput))?;
    let (user, _user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::ProjectsManage),
        Some(workspace_id),
    )
    .await?;
    let actor_user_id = parse_user_id(&user.user_id)?;
    let ip = peer_ip(peer.ip());
    let result = update_project_member_role(
        state
            .auth
            .db
            .pool
            .postgres("src/http/routes/projects.rs")
            .map_err(internal)?,
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
    check_origin(&headers, &state.public_origin)?;
    let (user, _user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::ProjectsManage),
        Some(workspace_id),
    )
    .await?;
    let actor_user_id = parse_user_id(&user.user_id)?;
    let ip = peer_ip(peer.ip());
    let result = remove_project_member(
        state
            .auth
            .db
            .pool
            .postgres("src/http/routes/projects.rs")
            .map_err(internal)?,
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
    let (user, _user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::TasksRead),
        Some(workspace_id),
    )
    .await?;
    let actor_user_id = parse_user_id(&user.user_id)?;
    let result = get_project_workflow(
        state
            .auth
            .db
            .pool
            .postgres("src/http/routes/projects.rs")
            .map_err(internal)?,
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
        ProjectDbError::WorkflowHasNoStatuses => AppError {
            status: StatusCode::BAD_REQUEST,
            code: ProblemCode::InvalidInput,
            source: None,
            params: Some(json!({"code":"workflow_has_no_statuses"})),
            retry_after: None,
        },
        ProjectDbError::InvalidMoveAnchors => AppError::from_code(ProblemCode::InvalidInput),
        ProjectDbError::AssigneeIsNotAMember => {
            AppError::from_code(ProblemCode::AssigneeIsNotAMember)
        }
        ProjectDbError::LabelNotFound
        | ProjectDbError::MilestoneNotFound
        | ProjectDbError::DependencyNotFound => AppError::from_code(ProblemCode::NotFound),
        ProjectDbError::DependencyCycle
        | ProjectDbError::DependencyContradiction
        | ProjectDbError::TaskCannotBlockItself => AppError::from_code(ProblemCode::InvalidInput),
        ProjectDbError::InvalidInput => AppError::from_code(ProblemCode::InvalidInput),
        ProjectDbError::OpenTimeEntryExists
        | ProjectDbError::StatusHasTasks
        | ProjectDbError::WorkflowStatusLimit => AppError::from_code(ProblemCode::Conflict),
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

#[cfg(test)]
mod selected_project_create_http_tests {
    use super::*;
    use crate::db::attachment_preview::tests::Fixture;
    use serde_json::Value;
    use std::sync::Arc;
    use tower::ServiceExt;
    fn state(f: &Fixture) -> AppState {
        AppState {
            auth:Arc::new(crate::auth::AuthService{db:crate::db::Db::from_backend(f.backend.clone()),password_keys:crate::auth::password::Keyring::parse(r#"{"test":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#,"test").unwrap()}),
            branding_name:"FVOCI".into(),public_origin:"http://localhost".into(),cookie_secure:false,rate_limiter:crate::http::rate_limit::RateLimiter::new(),storage:crate::attachments::ObjectStorage::local(f.root.join("project-create-http-storage")),
            upload:crate::attachments::UploadLimits{part_size_bytes:24,max_file_size_bytes:1024,create_rate_per_5min:20,part_put_slots:crate::attachments::PartPutSlots::new(2)},
            collab:None,meili:None,search_embedder:None,markdown:None,import_wake:None,import_extractor_available:false,preview_extract:None,quota:Default::default(),mailer:Arc::new(crate::mail::Mailer::disabled()),streams:AppState::fresh_streams(),
        }
    }

    async fn request(
        app: Router,
        method: &str,
        path: &str,
        cookie: Option<&str>,
        bearer: Option<&str>,
        origin: &str,
        body: Value,
    ) -> (StatusCode, Value) {
        let mut builder = axum::http::Request::builder()
            .method(method)
            .uri(path)
            .header("origin", origin)
            .header("content-type", "application/json");
        if let Some(token) = cookie {
            builder = builder.header("cookie", format!("fvoci_session={token}"));
        }
        if let Some(token) = bearer {
            builder = builder.header("authorization", format!("Bearer {token}"));
        }
        let mut request = builder
            .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        request.extensions_mut().insert(ConnectInfo(
            "127.0.0.1:12345".parse::<SocketAddr>().unwrap(),
        ));
        let response = app.oneshot(request).await.unwrap();
        let code = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 16384)
            .await
            .unwrap();
        (code, serde_json::from_slice(&bytes).unwrap())
    }
    async fn session(f: &Fixture, user: Uuid) -> (Uuid, String) {
        let token = crate::auth::token::new_token();
        let id = Uuid::now_v7();
        let mut tx = f.backend.begin_write().await.unwrap();
        tx.operation()
            .create_session(
                id,
                user,
                &token.hash,
                chrono::DateTime::from_timestamp_micros(
                    chrono::Utc::now().timestamp_micros() + 86_400_000_000,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();
        (id, token.token)
    }
    #[tokio::test]
    async fn wiki_aux_project_create_http_normal_submit_picker_strict_scopes_auth_and_tenant() {
        let f = Fixture::new().await;
        let (credential, cookie) = session(&f, f.user).await;
        let app = router()
            .merge(crate::http::routes::task_body::router())
            .with_state(state(&f));
        let path = format!("/api/v1/workspaces/{}/projects", f.workspace);
        let picker = format!(
            "/api/v1/workspaces/{}/documents/{}/task-projects",
            f.workspace, f.document
        );
        let body = json!({"key":"NＯRMAL","name":"  실제 프로젝트 中 😀  ","visibility":"private"});
        let call = |app: Router, body: Value| {
            let path = path.clone();
            let cookie = cookie.clone();
            async move {
                request(
                    app,
                    "POST",
                    &path,
                    Some(&cookie),
                    None,
                    "http://localhost",
                    body,
                )
                .await
            }
        };
        assert_eq!(
            request(
                app.clone(),
                "POST",
                &path,
                None,
                None,
                "http://localhost",
                body.clone()
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            request(
                app.clone(),
                "POST",
                &path,
                Some(&cookie),
                None,
                "http://elsewhere",
                body.clone()
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
        for invalid in [
            json!({"key":"WIKI","name":"Test","visibility":"private"}),
            json!({"key":" normal ","name":"Test","visibility":"private"}),
            json!({"key":"VALID","name":"Test","visibility":"bad"}),
            json!({"key":"VALID","name":"Test","visibility":"private","unknown":true}),
            json!({"key":"VALID","name":"Test","visibility":"private","leadUserId":null}),
        ] {
            assert_eq!(call(app.clone(), invalid).await.0, StatusCode::BAD_REQUEST);
        }
        let pat = crate::auth::token::new_token();
        let pat_id = Uuid::now_v7();
        sqlx::query("INSERT INTO api_tokens(id,workspace_id,user_id,token_hash,name,scopes) VALUES(?1,?2,?3,?4,'Project auth','[\"documents.read\"]')")
            .bind(pat_id.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).bind(&pat.hash).execute(&f.pool).await.unwrap();
        assert_eq!(
            request(
                app.clone(),
                "POST",
                &path,
                None,
                Some(&pat.token),
                "http://localhost",
                body.clone()
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        // First-party credentials take precedence over the insufficient bearer.
        let (code, created) = request(
            app.clone(),
            "POST",
            &path,
            Some(&cookie),
            Some(&pat.token),
            "http://localhost",
            body.clone(),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED, "{created}");
        let project = Uuid::parse_str(created["id"].as_str().unwrap()).unwrap();
        let root = Uuid::parse_str(created["rootDocumentId"].as_str().unwrap()).unwrap();
        assert_eq!(created["key"], "NORMAL");
        assert_eq!(created["name"], "실제 프로젝트 中 😀");
        assert_eq!(created["visibility"], "private");
        assert_eq!(created["createdBy"], f.user.to_string());
        let roles: Vec<(Vec<u8>, String)> =
            sqlx::query_as("SELECT user_id,role FROM project_members WHERE project_id=?1")
                .bind(project.as_bytes().as_slice())
                .fetch_all(&f.pool)
                .await
                .unwrap();
        assert_eq!(roles, vec![(f.user.as_bytes().to_vec(), "lead".into())]);
        let (code, items) = request(
            app.clone(),
            "GET",
            &picker,
            Some(&cookie),
            None,
            "http://localhost",
            Value::Null,
        )
        .await;
        assert_eq!(code, StatusCode::OK, "{items}");
        assert_eq!(items["items"].as_array().unwrap().len(), 1);
        assert_eq!(items["items"][0]["id"], project.to_string());
        assert_eq!(items["items"][0]["name"], created["name"]);
        assert_eq!(items["items"][0]["key"], "NORMAL");
        assert_eq!(items["canCreateProject"], true);
        let root_row: (Vec<u8>, i64, String) =
            sqlx::query_as("SELECT project_id,number,content_json FROM documents WHERE id=?1")
                .bind(root.as_bytes().as_slice())
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert_eq!(root_row.0, project.as_bytes());
        assert_eq!(root_row.1, 1);
        assert_eq!(
            serde_json::from_str::<Value>(&root_row.2).unwrap(),
            crate::db::documents::empty_document_json()
        );
        assert_eq!(
            call(app.clone(), body.clone()).await.0,
            StatusCode::CONFLICT
        );
        let other = Uuid::now_v7();
        sqlx::query("INSERT INTO users(id,email,given_name) VALUES(?1,?2,'Other')")
            .bind(other.as_bytes().as_slice())
            .bind(format!("{other}@example.test"))
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO memberships(workspace_id,user_id,role) VALUES(?1,?2,'member')")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(other.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let (_, other_cookie) = session(&f, other).await;
        let (code, other_items) = request(
            app.clone(),
            "GET",
            &picker,
            Some(&other_cookie),
            None,
            "http://localhost",
            Value::Null,
        )
        .await;
        assert_eq!(code, StatusCode::OK, "{other_items}");
        assert_eq!(other_items["items"], json!([]));
        sqlx::query("UPDATE api_tokens SET scopes='[\"projects.manage\"]' WHERE id=?1")
            .bind(pat_id.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let (_, before): (i64, i64) = sqlx::query_as(
            "SELECT (SELECT count(*) FROM projects),(SELECT count(*) FROM audit_log)",
        )
        .fetch_one(&f.pool)
        .await
        .unwrap();
        let (code, pat_created) = request(
            app.clone(),
            "POST",
            &path,
            None,
            Some(&pat.token),
            "http://localhost",
            json!({"key":"TOKEN","name":"Token","visibility":"workspace"}),
        )
        .await;
        assert_eq!(code, StatusCode::CREATED, "{pat_created}");
        let foreign = format!("/api/v1/workspaces/{}/projects", Uuid::now_v7());
        assert_eq!(
            request(
                app.clone(),
                "POST",
                &foreign,
                None,
                Some(&pat.token),
                "http://localhost",
                body.clone()
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        sqlx::query("UPDATE sessions SET revoked_at=1 WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            call(
                app.clone(),
                json!({"key":"REVOKED","name":"No","visibility":"private"})
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
        // Cookie revocation cannot fall through to the still valid bearer.
        assert_eq!(
            request(
                app,
                "POST",
                &path,
                Some(&cookie),
                Some(&pat.token),
                "http://localhost",
                json!({"key":"FALLTHROUGH","name":"No","visibility":"private"})
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
        let (count, audits): (i64, i64) = sqlx::query_as(
            "SELECT (SELECT count(*) FROM projects),(SELECT count(*) FROM audit_log)",
        )
        .fetch_one(&f.pool)
        .await
        .unwrap();
        assert_eq!(count, 2);
        assert_eq!(audits, before + 1);
        f.close().await;
    }
}
