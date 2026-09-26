use std::net::SocketAddr;

use axum::extract::rejection::JsonRejection;
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use uuid::Uuid;

use crate::api::dto::{
    CreateWorkspaceBody, DeleteWorkspaceBody, MemberResponse, MemberRoleBody, MembersResponse,
    OkResponse, PatchWorkspaceBody, WorkspaceListItemResponse, WorkspaceListResponse,
    WorkspaceMetaResponse,
};
use crate::auth::session::SessionUser;
use crate::db::workspace::{WorkspaceDbError, WorkspaceRole};
use crate::error::{AppError, ProblemCode};
use crate::http::guard::check_origin;
use crate::http::rate_limit::peer_ip;
use crate::http::state::AppState;
use crate::validate::{normalize_slug, validate_given_name};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/me/workspaces", get(list_my_workspaces))
        .route("/api/v1/me/personal-workspace", post(personal_workspace))
        .route("/api/v1/workspaces", post(create_workspace))
        .route(
            "/api/v1/workspaces/{workspace_id}",
            get(get_workspace)
                .patch(patch_workspace)
                .delete(delete_workspace),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/members",
            get(list_members),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/members/{user_id}",
            patch(patch_member).delete(remove_member),
        )
}

async fn list_my_workspaces(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<WorkspaceListResponse>, AppError> {
    let (_user, user_id, _) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Session,
        None,
    )
    .await?;
    let listed = crate::db::workspace::list_workspaces_for_user(&state.auth.db.pool, user_id)
        .await
        .map_err(internal)?;
    let items = listed
        .into_iter()
        .map(|w| WorkspaceListItemResponse {
            id: w.id.to_string(),
            name: w.name,
            slug: w.slug,
            role: w.role.as_str().to_string(),
            kind: w.kind,
            document_count: w.document_count,
            assigned_count: w.assigned_count,
        })
        .collect();
    Ok(Json(WorkspaceListResponse { items }))
}

async fn get_workspace(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
) -> Result<Json<WorkspaceMetaResponse>, AppError> {
    let (_user, user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Any,
        Some(workspace_id),
    )
    .await?;
    let result = crate::db::workspace::get_workspace_meta(
        &state.auth.db.pool,
        workspace_id,
        user_id,
        session_id,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(meta) => Ok(Json(meta_response(meta))),
        Err(err) => Err(map_workspace_error(err, false)),
    }
}

async fn list_members(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
) -> Result<Json<MembersResponse>, AppError> {
    let (_user, user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::WorkspaceManage),
        Some(workspace_id),
    )
    .await?;
    let result =
        crate::db::workspace::list_members(&state.auth.db.pool, workspace_id, user_id, session_id)
            .await
            .map_err(internal)?;
    match result {
        Ok(members) => Ok(Json(MembersResponse {
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
        Err(err) => Err(map_workspace_error(err, false)),
    }
}

async fn patch_workspace(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
    body: Result<Json<PatchWorkspaceBody>, JsonRejection>,
) -> Result<Json<WorkspaceMetaResponse>, AppError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    if body.name.is_none() {
        return Err(AppError::from_code(ProblemCode::InvalidInput));
    }
    let name = body.name.as_ref().unwrap();
    validate_given_name(name.trim())?;
    let (_user, user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::WorkspaceManage),
        Some(workspace_id),
    )
    .await?;
    let ip = peer_ip(peer.ip());
    let result = crate::db::workspace::update_workspace_meta(
        &state.auth.db.pool,
        workspace_id,
        user_id,
        session_id,
        name.trim(),
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(meta) => Ok(Json(meta_response(meta))),
        Err(err) => Err(map_workspace_error(err, false)),
    }
}

async fn create_workspace(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    body: Result<Json<CreateWorkspaceBody>, JsonRejection>,
) -> Result<Response, AppError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    validate_given_name(body.name.trim())?;
    let slug = normalize_slug(&body.slug)?;
    let (_user, user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Session,
        None,
    )
    .await?;
    let ip = peer_ip(peer.ip());
    let result = crate::db::workspace::create_workspace_as_instance_admin(
        &state.auth.db.pool,
        &state.auth.db.license,
        user_id,
        session_id,
        body.name.trim(),
        &slug,
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(meta) => Ok((StatusCode::CREATED, Json(meta_response(meta))).into_response()),
        Err(WorkspaceDbError::Forbidden) => {
            Err(AppError::from_code(ProblemCode::InsufficientPermissions))
        }
        Err(WorkspaceDbError::SlugTaken) => Err(AppError::from_code(ProblemCode::SlugTaken)),
        Err(err) => Err(map_workspace_error(err, true)),
    }
}

async fn personal_workspace(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<WorkspaceMetaResponse>, AppError> {
    check_origin(&headers, &state.public_origin)?;
    let (_user, user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Session,
        None,
    )
    .await?;
    let ip = peer_ip(peer.ip());
    let result = crate::db::workspace::ensure_personal_workspace(
        &state.auth.db.pool,
        &state.auth.db.license,
        user_id,
        session_id,
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(meta) => Ok(Json(meta_response(meta))),
        Err(err) => Err(map_workspace_error(err, false)),
    }
}

async fn patch_member(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, target_user_id)): Path<(Uuid, Uuid)>,
    body: Result<Json<MemberRoleBody>, JsonRejection>,
) -> Result<Json<MemberResponse>, AppError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    let next_role = WorkspaceRole::parse(&body.role)
        .ok_or_else(|| AppError::from_code(ProblemCode::InvalidInput))?;
    let (user, _user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Session,
        None,
    )
    .await?;
    let actor_user_id = parse_user_id(&user.user_id)?;
    let ip = peer_ip(peer.ip());
    let result = crate::db::workspace::set_member_role(
        &state.auth.db.pool,
        &state.auth.db.license,
        workspace_id,
        actor_user_id,
        session_id,
        target_user_id,
        next_role,
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(member) => Ok(Json(MemberResponse {
            user_id: member.user_id.to_string(),
            email: member.email,
            given_name: member.given_name,
            family_name: member.family_name,
            role: member.role.as_str().to_string(),
        })),
        Err(err) => Err(map_workspace_error(err, false)),
    }
}

async fn remove_member(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, target_user_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<OkResponse>, AppError> {
    check_origin(&headers, &state.public_origin)?;
    let (user, _user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Session,
        None,
    )
    .await?;
    let actor_user_id = parse_user_id(&user.user_id)?;
    let ip = peer_ip(peer.ip());
    let result = crate::db::workspace::remove_member(
        &state.auth.db.pool,
        workspace_id,
        actor_user_id,
        session_id,
        target_user_id,
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(()) => Ok(Json(OkResponse { ok: true })),
        Err(err) => Err(map_workspace_error(err, false)),
    }
}

async fn delete_workspace(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
    body: Result<Json<DeleteWorkspaceBody>, JsonRejection>,
) -> Result<Json<OkResponse>, AppError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    let confirm_slug = normalize_slug(&body.confirm_slug)?;
    let (_user, user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::WorkspaceManage),
        Some(workspace_id),
    )
    .await?;
    let ip = peer_ip(peer.ip());
    let result = crate::db::workspace::trash_workspace(
        &state.auth.db.pool,
        workspace_id,
        user_id,
        session_id,
        &confirm_slug,
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(()) => Ok(Json(OkResponse { ok: true })),
        Err(WorkspaceDbError::Forbidden) => {
            Err(AppError::from_code(ProblemCode::InsufficientPermissions))
        }
        Err(err) => Err(map_workspace_error(err, false)),
    }
}

fn meta_response(meta: crate::db::workspace::WorkspaceMeta) -> WorkspaceMetaResponse {
    WorkspaceMetaResponse {
        id: meta.id.to_string(),
        name: meta.name,
        slug: meta.slug,
    }
}

fn map_workspace_error(err: WorkspaceDbError, create_route: bool) -> AppError {
    match err {
        WorkspaceDbError::NotFound => AppError::from_code(ProblemCode::NotFound),
        WorkspaceDbError::Forbidden if create_route => {
            AppError::from_code(ProblemCode::InsufficientPermissions)
        }
        WorkspaceDbError::Forbidden => AppError::from_code(ProblemCode::NotFound),
        WorkspaceDbError::PersonalImmutable => {
            AppError::from_code(ProblemCode::PersonalWorkspaceImmutable)
        }
        WorkspaceDbError::LastOwner => AppError::from_code(ProblemCode::WorkspaceLastOwnerRequired),
        WorkspaceDbError::SelfChange => {
            AppError::from_code(ProblemCode::WorkspaceMemberSelfChangeForbidden)
        }
        WorkspaceDbError::RoleCap => AppError::from_code(ProblemCode::CannotManageRoleAboveOwn),
        WorkspaceDbError::SlugTaken => AppError::from_code(ProblemCode::SlugTaken),
        WorkspaceDbError::LastProjectLead => AppError::from_code(ProblemCode::Conflict),
        WorkspaceDbError::SeatLimit => AppError::from_code(ProblemCode::LimitSeats),
        WorkspaceDbError::GuestLimit => AppError::from_code(ProblemCode::LimitGuests),
        WorkspaceDbError::InvalidInput => AppError::from_code(ProblemCode::InvalidInput),
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
