use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use uuid::Uuid;

use crate::api::dto::{
    CreateGroupBody, GroupListResponse, GroupMemberBody, GroupMemberListResponse,
    GroupMemberOutput, GroupOutput, OkResponse, ProjectGroupGrantBody,
    ProjectGroupGrantListResponse, ProjectGroupGrantOutput, ProjectGroupRevokeBody,
};
use crate::auth::session::SessionUser;
use crate::db::groups::{
    add_group_member, add_group_to_document, add_group_to_project, create_group,
    list_document_group_grants, list_group_members, list_groups, list_project_group_grants,
    purge_group, remove_group_from_document, remove_group_from_project, remove_group_member,
    GroupDbError, GroupRow,
};
use crate::error::{AppError, ProblemCode};
use crate::http::guard::check_origin;
use crate::http::state::AppState;
use crate::projects::ProjectMemberRole;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/workspaces/{workspace_id}/groups",
            get(list_groups_route).post(create_group_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/groups/{group_id}",
            axum::routing::delete(purge_group_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/groups/{group_id}/members",
            get(list_group_members_route)
                .post(add_group_member_route)
                .delete(remove_group_member_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/groups",
            get(list_project_groups_route)
                .post(add_project_group_route)
                .delete(remove_project_group_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/documents/{document_id}/groups",
            get(list_document_groups_route)
                .post(add_document_group_route)
                .delete(remove_document_group_route),
        )
}

fn group_output(row: GroupRow) -> GroupOutput {
    GroupOutput {
        id: row.id.to_string(),
        workspace_id: row.workspace_id.to_string(),
        name: row.name,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

fn map_group_error(err: GroupDbError) -> AppError {
    match err {
        GroupDbError::NotFound | GroupDbError::Forbidden => {
            AppError::from_code(ProblemCode::NotFound)
        }
        GroupDbError::Conflict | GroupDbError::LastLead => {
            AppError::from_code(ProblemCode::Conflict)
        }
        GroupDbError::InvalidInput => AppError::from_code(ProblemCode::InvalidInput),
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

fn internal(err: sqlx::Error) -> AppError {
    tracing::error!("database error: {}", err);
    AppError::internal()
}

async fn list_groups_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
) -> Result<Json<GroupListResponse>, AppError> {
    let (user, _user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::WorkspaceManage),
        Some(workspace_id),
    )
    .await?;
    let actor_user_id = Uuid::parse_str(&user.user_id)
        .map_err(|_| AppError::from_code(ProblemCode::AuthenticationRequired))?;
    let result = list_groups(&state.auth.db.pool, workspace_id, actor_user_id, session_id)
        .await
        .map_err(internal)?;
    match result {
        Ok(items) => Ok(Json(GroupListResponse {
            items: items.into_iter().map(group_output).collect(),
        })),
        Err(err) => Err(map_group_error(err)),
    }
}

async fn create_group_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
    body: Result<Json<CreateGroupBody>, JsonRejection>,
) -> Result<Response, AppError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    let (user, _user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::WorkspaceManage),
        Some(workspace_id),
    )
    .await?;
    let actor_user_id = Uuid::parse_str(&user.user_id)
        .map_err(|_| AppError::from_code(ProblemCode::AuthenticationRequired))?;
    let result = create_group(
        &state.auth.db.pool,
        workspace_id,
        actor_user_id,
        session_id,
        &body.name,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(created) => Ok((StatusCode::CREATED, Json(group_output(created))).into_response()),
        Err(err) => Err(map_group_error(err)),
    }
}

async fn purge_group_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, group_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<OkResponse>, AppError> {
    check_origin(&headers, &state.public_origin)?;
    let (user, _user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::WorkspaceManage),
        Some(workspace_id),
    )
    .await?;
    let actor_user_id = Uuid::parse_str(&user.user_id)
        .map_err(|_| AppError::from_code(ProblemCode::AuthenticationRequired))?;
    let result = purge_group(
        &state.auth.db.pool,
        workspace_id,
        actor_user_id,
        session_id,
        group_id,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(()) => Ok(Json(OkResponse { ok: true })),
        Err(err) => Err(map_group_error(err)),
    }
}

async fn list_group_members_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, group_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<GroupMemberListResponse>, AppError> {
    let (user, _user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::WorkspaceManage),
        Some(workspace_id),
    )
    .await?;
    let actor_user_id = Uuid::parse_str(&user.user_id)
        .map_err(|_| AppError::from_code(ProblemCode::AuthenticationRequired))?;
    let result = list_group_members(
        &state.auth.db.pool,
        workspace_id,
        actor_user_id,
        session_id,
        group_id,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(items) => Ok(Json(GroupMemberListResponse {
            items: items
                .into_iter()
                .map(|member| GroupMemberOutput {
                    user_id: member.user_id.to_string(),
                })
                .collect(),
        })),
        Err(err) => Err(map_group_error(err)),
    }
}

async fn add_group_member_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, group_id)): Path<(Uuid, Uuid)>,
    body: Result<Json<GroupMemberBody>, JsonRejection>,
) -> Result<Response, AppError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    let (user, _user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::WorkspaceManage),
        Some(workspace_id),
    )
    .await?;
    let actor_user_id = Uuid::parse_str(&user.user_id)
        .map_err(|_| AppError::from_code(ProblemCode::AuthenticationRequired))?;
    let result = add_group_member(
        &state.auth.db.pool,
        workspace_id,
        actor_user_id,
        session_id,
        group_id,
        body.user_id,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(()) => Ok((StatusCode::CREATED, Json(OkResponse { ok: true })).into_response()),
        Err(err) => Err(map_group_error(err)),
    }
}

async fn remove_group_member_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, group_id)): Path<(Uuid, Uuid)>,
    body: Result<Json<GroupMemberBody>, JsonRejection>,
) -> Result<Json<OkResponse>, AppError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    let (user, _user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::WorkspaceManage),
        Some(workspace_id),
    )
    .await?;
    let actor_user_id = Uuid::parse_str(&user.user_id)
        .map_err(|_| AppError::from_code(ProblemCode::AuthenticationRequired))?;
    let result = remove_group_member(
        &state.auth.db.pool,
        workspace_id,
        actor_user_id,
        session_id,
        group_id,
        body.user_id,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(()) => Ok(Json(OkResponse { ok: true })),
        Err(err) => Err(map_group_error(err)),
    }
}

async fn list_project_groups_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<ProjectGroupGrantListResponse>, AppError> {
    let (user, _user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::ProjectsRead),
        Some(workspace_id),
    )
    .await?;
    let actor_user_id = Uuid::parse_str(&user.user_id)
        .map_err(|_| AppError::from_code(ProblemCode::AuthenticationRequired))?;
    let result = list_project_group_grants(
        &state.auth.db.pool,
        workspace_id,
        actor_user_id,
        session_id,
        project_id,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(items) => Ok(Json(ProjectGroupGrantListResponse {
            items: items
                .into_iter()
                .map(|grant| ProjectGroupGrantOutput {
                    group_id: grant.group_id.to_string(),
                    role: grant.role.as_str().to_string(),
                })
                .collect(),
        })),
        Err(err) => Err(map_group_error(err)),
    }
}

async fn add_project_group_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id)): Path<(Uuid, Uuid)>,
    body: Result<Json<ProjectGroupGrantBody>, JsonRejection>,
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
    let actor_user_id = Uuid::parse_str(&user.user_id)
        .map_err(|_| AppError::from_code(ProblemCode::AuthenticationRequired))?;
    let result = add_group_to_project(
        &state.auth.db.pool,
        workspace_id,
        actor_user_id,
        session_id,
        project_id,
        body.group_id,
        role,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(()) => Ok((StatusCode::CREATED, Json(OkResponse { ok: true })).into_response()),
        Err(err) => Err(map_group_error(err)),
    }
}

async fn remove_project_group_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id)): Path<(Uuid, Uuid)>,
    body: Result<Json<ProjectGroupRevokeBody>, JsonRejection>,
) -> Result<Json<OkResponse>, AppError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    let (user, _user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::ProjectsManage),
        Some(workspace_id),
    )
    .await?;
    let actor_user_id = Uuid::parse_str(&user.user_id)
        .map_err(|_| AppError::from_code(ProblemCode::AuthenticationRequired))?;
    let result = remove_group_from_project(
        &state.auth.db.pool,
        workspace_id,
        actor_user_id,
        session_id,
        project_id,
        body.group_id,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(()) => Ok(Json(OkResponse { ok: true })),
        Err(err) => Err(map_group_error(err)),
    }
}

async fn list_document_groups_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, document_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<ProjectGroupGrantListResponse>, AppError> {
    let (user, _user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::DocumentsRead),
        Some(workspace_id),
    )
    .await?;
    let actor_user_id = Uuid::parse_str(&user.user_id)
        .map_err(|_| AppError::from_code(ProblemCode::AuthenticationRequired))?;
    let result = list_document_group_grants(
        &state.auth.db.pool,
        workspace_id,
        actor_user_id,
        session_id,
        document_id,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(items) => Ok(Json(ProjectGroupGrantListResponse {
            items: items
                .into_iter()
                .map(|grant| ProjectGroupGrantOutput {
                    group_id: grant.group_id.to_string(),
                    role: grant.role.as_str().to_string(),
                })
                .collect(),
        })),
        Err(err) => Err(map_group_error(err)),
    }
}

async fn add_document_group_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, document_id)): Path<(Uuid, Uuid)>,
    body: Result<Json<ProjectGroupGrantBody>, JsonRejection>,
) -> Result<Response, AppError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    let role = ProjectMemberRole::parse(&body.role)
        .ok_or_else(|| AppError::from_code(ProblemCode::InvalidInput))?;
    let (user, _user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Session,
        Some(workspace_id),
    )
    .await?;
    let actor_user_id = Uuid::parse_str(&user.user_id)
        .map_err(|_| AppError::from_code(ProblemCode::AuthenticationRequired))?;
    let result = add_group_to_document(
        &state.auth.db.pool,
        workspace_id,
        actor_user_id,
        session_id,
        document_id,
        body.group_id,
        role,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(()) => Ok((StatusCode::CREATED, Json(OkResponse { ok: true })).into_response()),
        Err(err) => Err(map_group_error(err)),
    }
}

async fn remove_document_group_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, document_id)): Path<(Uuid, Uuid)>,
    body: Result<Json<ProjectGroupRevokeBody>, JsonRejection>,
) -> Result<Json<OkResponse>, AppError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    let (user, _user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Session,
        Some(workspace_id),
    )
    .await?;
    let actor_user_id = Uuid::parse_str(&user.user_id)
        .map_err(|_| AppError::from_code(ProblemCode::AuthenticationRequired))?;
    let result = remove_group_from_document(
        &state.auth.db.pool,
        workspace_id,
        actor_user_id,
        session_id,
        document_id,
        body.group_id,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(()) => Ok(Json(OkResponse { ok: true })),
        Err(err) => Err(map_group_error(err)),
    }
}
