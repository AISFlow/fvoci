use std::net::SocketAddr;

use axum::extract::rejection::JsonRejection;
use axum::extract::{ConnectInfo, Path, State};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use uuid::Uuid;

use crate::api::dto::{
    InvitationAcceptBody, InvitationCreateBody, InvitationCreateResponse, InvitationLegalDocument,
    InvitationPublicResponse,
};
use crate::auth::session::SessionUser;
use crate::auth::token::hash_token;
use crate::db::invitations::InvitationDbError;
use crate::db::workspace::WorkspaceRole;
use crate::error::{AppError, ProblemCode};
use crate::http::guard::check_origin;
use crate::http::rate_limit::peer_ip;
use crate::http::state::AppState;
use crate::validate::{normalize_email, validate_family_name, validate_given_name};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/workspaces/{workspace_id}/invitations",
            post(create_invitation),
        )
        .route("/api/v1/invitations/{token}", get(get_invitation))
        .route(
            "/api/v1/invitations/{token}/accept",
            post(accept_invitation),
        )
}

async fn create_invitation(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
    body: Result<Json<InvitationCreateBody>, JsonRejection>,
) -> Result<(axum::http::StatusCode, Json<InvitationCreateResponse>), AppError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    let email = normalize_email(&body.email)?;
    let role = WorkspaceRole::parse(&body.role)
        .ok_or_else(|| AppError::from_code(ProblemCode::InvalidInput))?;
    let (_user, user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::WorkspaceManage),
        Some(workspace_id),
    )
    .await?;
    if let Err(retry_after) = state
        .rate_limiter
        .allow(&format!("invite-create:{user_id}"), 30)
        .await
    {
        return Err(AppError::rate_limited(retry_after));
    }
    let ip = peer_ip(peer.ip());
    let result = crate::db::invitations::create_invitation(
        &state.auth.db.pool,
        workspace_id,
        user_id,
        session_id,
        &email,
        role,
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(created) => {
            let origin = state.public_origin.trim_end_matches('/');
            let accept_url = format!("{origin}/invite/{}", created.accept_path_token);
            let mail_delayed = match state.mailer.send_invite(&email, &accept_url).await {
                Ok(()) => None,
                Err(_) => {
                    tracing::warn!(message = "invitation: invite mail", "mail.send_failed");
                    Some(true)
                }
            };
            Ok((
                axum::http::StatusCode::CREATED,
                Json(InvitationCreateResponse {
                    accept_url,
                    mail_delayed,
                }),
            ))
        }
        Err(err) => Err(map_create_error(err)),
    }
}

async fn get_invitation(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path(token): Path<String>,
) -> Result<Json<InvitationPublicResponse>, AppError> {
    if token.is_empty() {
        return Err(AppError::from_code(
            ProblemCode::InvitationNotFoundOrExpired,
        ));
    }
    let ip = peer_ip(peer.ip());
    if let Err(retry_after) = state
        .rate_limiter
        .allow(&format!("invite-read:{ip}"), 60)
        .await
    {
        return Err(AppError::rate_limited(retry_after));
    }
    let result = crate::db::invitations::get_invitation_public(&state.auth.db.pool, &token)
        .await
        .map_err(internal)?;
    match result {
        Ok(preview) => Ok(Json(InvitationPublicResponse {
            workspace_name: preview.workspace_name,
            email_masked: preview.email_masked,
            role: preview.role.as_str().to_string(),
            required_legal: preview
                .required_legal
                .into_iter()
                .map(|(kind, version, title)| InvitationLegalDocument {
                    kind,
                    version,
                    title,
                })
                .collect(),
        })),
        Err(InvitationDbError::Expired | InvitationDbError::AlreadyAccepted) => Err(
            AppError::from_code(ProblemCode::InvitationNotFoundOrExpired),
        ),
        Err(err) => Err(map_preview_error(err)),
    }
}

async fn accept_invitation(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path(token): Path<String>,
    body: Result<Json<InvitationAcceptBody>, JsonRejection>,
) -> Result<Response, AppError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    if token.is_empty() {
        return Err(AppError::from_code(ProblemCode::NotFound));
    }
    let ip = peer_ip(peer.ip());
    let hash_prefix: String = hash_token(&token).chars().take(8).collect();
    if let Err(retry_after) = state
        .rate_limiter
        .allow(&format!("invite-accept:{ip}:{hash_prefix}"), 30)
        .await
    {
        return Err(AppError::rate_limited(retry_after));
    }
    if let Some(password) = body.password.as_deref() {
        crate::validate::validate_password_setting(&state.auth.db.pool, password).await?;
    }
    let settings = crate::settings::current_values(&state.auth.db.pool, &state.branding_name)
        .await
        .map_err(internal)?;
    let consents: Vec<(String, i32)> = body
        .consents
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|item| (item.kind.clone(), item.version))
        .collect();
    let email = match body.email.as_deref() {
        Some(value) => Some(normalize_email(value)?),
        None => None,
    };
    if let Some(given_name) = body.given_name.as_deref() {
        validate_given_name(given_name)?;
    }
    if let Some(family_name) = body.family_name.as_deref() {
        validate_family_name(family_name)?;
    }
    let result = crate::db::invitations::accept_invitation(
        &state.auth.db.pool,
        &state.auth.password_keys,
        &token,
        crate::db::invitations::AcceptInvitationRequest {
            email: email.as_deref(),
            given_name: body.given_name.as_deref().map(str::trim),
            family_name: body.family_name.as_deref(),
            password: body.password.as_deref(),
            client_ip: Some(&ip),
            consents: &consents,
            defaults: &settings.defaults_user,
        },
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(issued) => Ok(crate::http::routes::auth::issued_response(
            state.cookie_secure,
            issued,
        )),
        Err(err) => Err(map_accept_error(err)),
    }
}

fn map_create_error(err: InvitationDbError) -> AppError {
    match err {
        InvitationDbError::NotFound => AppError::from_code(ProblemCode::NotFound),
        InvitationDbError::Forbidden => AppError::from_code(ProblemCode::InsufficientPermissions),
        InvitationDbError::PersonalImmutable => {
            AppError::from_code(ProblemCode::PersonalWorkspaceImmutable)
        }
        InvitationDbError::RoleCap => {
            AppError::from_code(ProblemCode::CannotInviteARoleAboveYourOwn)
        }
        other => map_accept_error(other),
    }
}

fn map_preview_error(err: InvitationDbError) -> AppError {
    match err {
        InvitationDbError::Expired
        | InvitationDbError::AlreadyAccepted
        | InvitationDbError::NotFound => {
            AppError::from_code(ProblemCode::InvitationNotFoundOrExpired)
        }
        _ => AppError::from_code(ProblemCode::InvitationNotFoundOrExpired),
    }
}

fn map_accept_error(err: InvitationDbError) -> AppError {
    match err {
        InvitationDbError::NotFound => AppError::from_code(ProblemCode::NotFound),
        InvitationDbError::Expired => AppError::from_code(ProblemCode::Expired),
        InvitationDbError::AlreadyAccepted => AppError::from_code(ProblemCode::AlreadyAccepted),
        InvitationDbError::Unauthorized
        | InvitationDbError::Forbidden
        | InvitationDbError::AlreadyLinked => {
            AppError::from_code(ProblemCode::CannotAcceptInvitation)
        }
        InvitationDbError::ConsentRequired => AppError::from_code(ProblemCode::ConsentRequired),
        InvitationDbError::SeatLimit => AppError::from_code(ProblemCode::LimitSeats),
        InvitationDbError::GuestLimit => AppError::from_code(ProblemCode::LimitGuests),
        InvitationDbError::PersonalImmutable | InvitationDbError::RoleCap => {
            AppError::from_code(ProblemCode::NotFound)
        }
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
