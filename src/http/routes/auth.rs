use std::net::SocketAddr;

use axum::extract::rejection::JsonRejection;
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use serde_json::Value;
use uuid::Uuid;

use crate::api::dto::{
    LoginBody, LoginResponse, OkResponse, PasswordResetBody, PasswordResetConfirmBody,
    SessionUserOutput,
};
use crate::auth::password::hash_password;
use crate::auth::session::SessionUser;
use crate::auth::token::new_token;
use crate::db::identity::FamilyNamePatch;
use crate::error::{AppError, ProblemCode, SESSION_COOKIE};
use crate::http::cookie::{clear_session_cookie, set_session_cookie};
use crate::http::guard::check_origin;
use crate::http::json_input::parse_patch_me;
use crate::http::rate_limit::peer_ip;
use crate::http::state::AppState;
use crate::mail::{equalize_magic_response_timing, MAGIC_PER_EMAIL, MAGIC_PER_IP};
use crate::validate::{
    normalize_email, validate_family_name, validate_given_name, validate_locale,
    validate_text_scale, validate_week_starts_on,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/auth/login", post(login))
        .route("/api/v1/auth/logout", post(logout))
        .route("/api/v1/auth/me", get(me).patch(patch_me))
        .route("/api/v1/auth/password-reset", post(request_password_reset))
        .route(
            "/api/v1/auth/password-reset/confirm",
            post(confirm_password_reset),
        )
}

async fn login(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Result<Json<LoginBody>, JsonRejection>,
) -> Result<Response, AppError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    let ip = peer_ip(peer.ip());
    if let Err(retry_after) = state
        .rate_limiter
        .allow(&format!("login:ip:{ip}"), 30)
        .await
    {
        return Err(AppError::rate_limited(retry_after));
    }
    if body.password.is_empty() {
        return Err(AppError::from_code(ProblemCode::InvalidInput));
    }
    let email = normalize_email(&body.email)?;
    if let Err(retry_after) = state
        .rate_limiter
        .allow(&format!("login:email:{ip}:{email}"), 10)
        .await
    {
        return Err(AppError::rate_limited(retry_after));
    }
    let result = state
        .auth
        .login(&email, &body.password)
        .await
        .map_err(internal)?;
    if result.is_none() {
        return Err(AppError::from_code(ProblemCode::InvalidEmailOrPassword));
    }
    let (user_id, token) = result.unwrap();
    let cookie = set_session_cookie(state.cookie_secure, &token);
    let mut response = (
        StatusCode::OK,
        Json(LoginResponse {
            user_id: user_id.to_string(),
        }),
    )
        .into_response();
    response
        .headers_mut()
        .append(axum::http::header::SET_COOKIE, cookie.parse().unwrap());
    Ok(response)
}

async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Response, AppError> {
    check_origin(&headers, &state.public_origin)?;
    let token = jar.get(SESSION_COOKIE).map(|c| c.value().to_string());
    let actor = if let Some(token) = &token {
        state
            .auth
            .session_user(token)
            .await
            .map_err(internal)?
            .and_then(|u| Uuid::parse_str(&u.user_id).ok())
    } else {
        None
    };
    if let Some(token) = token {
        state.auth.logout(&token, actor).await.map_err(internal)?;
    }
    let cookie = clear_session_cookie(state.cookie_secure);
    let mut response = StatusCode::NO_CONTENT.into_response();
    response
        .headers_mut()
        .append(axum::http::header::SET_COOKIE, cookie.parse().unwrap());
    Ok(response)
}

async fn me(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Json<SessionUserOutput>, AppError> {
    let user = require_session(&state, &headers, &jar).await?;
    Ok(Json(SessionUserOutput::from(user)))
}

async fn patch_me(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    body: Result<Json<Value>, JsonRejection>,
) -> Result<Json<SessionUserOutput>, AppError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    let token = jar
        .get(SESSION_COOKIE)
        .map(|c| c.value().to_string())
        .ok_or_else(|| AppError::from_code(ProblemCode::AuthenticationRequired))?;
    require_session(&state, &headers, &jar).await?;

    let patch = parse_patch_me(body)?;
    validate_given_name(&patch.given_name)?;
    if let Some(locale) = &patch.locale {
        validate_locale(locale)?;
    }
    if let Some(scale) = patch.text_scale {
        validate_text_scale(scale)?;
    }
    if let Some(week) = patch.week_starts_on {
        validate_week_starts_on(week)?;
    }
    if let Some(tz) = &patch.timezone {
        if tz.trim().is_empty() {
            return Err(AppError::from_code(ProblemCode::InvalidInput));
        }
    }

    let family_name = match patch.family_name {
        FamilyNamePatch::Preserve => FamilyNamePatch::Preserve,
        FamilyNamePatch::Clear => FamilyNamePatch::Clear,
        FamilyNamePatch::Set(value) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                FamilyNamePatch::Clear
            } else {
                validate_family_name(trimmed)?;
                FamilyNamePatch::Set(trimmed.to_string())
            }
        }
    };

    let patch = crate::db::identity::ProfilePatch {
        given_name: patch.given_name.trim().to_string(),
        family_name,
        locale: patch.locale,
        timezone: patch.timezone,
        week_starts_on: patch.week_starts_on,
        text_scale: patch.text_scale,
    };

    let updated = state
        .auth
        .patch_profile(&token, patch)
        .await
        .map_err(internal)?;
    match updated {
        Ok(user) => Ok(Json(SessionUserOutput::from(user))),
        Err(()) => Err(AppError::from_code(ProblemCode::AuthenticationRequired)),
    }
}

async fn require_session(
    state: &AppState,
    headers: &HeaderMap,
    jar: &CookieJar,
) -> Result<SessionUser, AppError> {
    let auth = crate::http::authz::require_request_auth(
        state,
        headers,
        jar,
        crate::http::authz::Access::Session,
        None,
    )
    .await?;
    Ok(auth.user)
}

async fn request_password_reset(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Result<Json<PasswordResetBody>, JsonRejection>,
) -> Result<(StatusCode, Json<OkResponse>), AppError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    let ip = peer_ip(peer.ip());
    if let Err(retry_after) = state
        .rate_limiter
        .allow(&format!("magic-ip:{ip}"), MAGIC_PER_IP)
        .await
    {
        return Err(AppError::rate_limited(retry_after));
    }
    let email = normalize_email(&body.email)?;
    if let Err(retry_after) = state
        .rate_limiter
        .allow(&format!("magic-email:{ip}:{email}"), MAGIC_PER_EMAIL)
        .await
    {
        return Err(AppError::rate_limited(retry_after));
    }
    let started = std::time::Instant::now();
    if let Some((user_id, generation, suspended_at)) =
        crate::db::identity::find_reset_user_by_email(&state.auth.db.pool, &email)
            .await
            .map_err(internal)?
    {
        if suspended_at.is_none() {
            let issued = new_token();
            let expires_at = crate::db::magic::magic_expires_at(chrono::Utc::now());
            crate::db::magic::issue_password_reset_token(
                &state.auth.db.pool,
                user_id,
                generation,
                &issued.hash,
                expires_at,
            )
            .await
            .map_err(internal)?;
            let origin = state.public_origin.trim_end_matches('/');
            let url = format!("{origin}/reset-password?token={}", issued.token);
            state.mailer.send_detached(
                email,
                crate::mail::templates::RESET_SUBJECT.to_string(),
                crate::mail::templates::reset_text(&url),
            );
        }
    }
    equalize_magic_response_timing(started).await;
    Ok((StatusCode::ACCEPTED, Json(OkResponse { ok: true })))
}

async fn confirm_password_reset(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Result<Json<PasswordResetConfirmBody>, JsonRejection>,
) -> Result<Json<OkResponse>, AppError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    let ip = peer_ip(peer.ip());
    if let Err(retry_after) = state
        .rate_limiter
        .allow(&format!("magic-ip:{ip}"), MAGIC_PER_IP)
        .await
    {
        return Err(AppError::rate_limited(retry_after));
    }
    crate::validate::validate_password_setting(&state.auth.db.pool, &body.new_password).await?;
    let payload = crate::db::magic::consume_magic_token(&state.auth.db.pool, &body.token)
        .await
        .map_err(internal)?;
    let Some(payload) = payload else {
        return Err(AppError::from_code(ProblemCode::MagicInvalid));
    };
    let password_hash = hash_password(&body.new_password, &state.auth.password_keys)
        .await
        .map_err(|err| {
            tracing::error!(error = %err, "password hash failed");
            AppError::internal()
        })?;
    let ok =
        crate::db::magic::complete_password_reset(&state.auth.db.pool, &payload, &password_hash)
            .await
            .map_err(internal)?;
    if !ok {
        return Err(AppError::from_code(ProblemCode::MagicInvalid));
    }
    Ok(Json(OkResponse { ok: true }))
}

fn internal(err: sqlx::Error) -> AppError {
    tracing::error!("database error: {}", sanitize_db_error(&err));
    AppError::internal()
}

fn sanitize_db_error(err: &sqlx::Error) -> String {
    match err {
        sqlx::Error::Database(db) => db.message().to_string(),
        _ => "database operation failed".to_string(),
    }
}
