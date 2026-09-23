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

use crate::api::dto::{LoginBody, LoginResponse, SessionUserOutput};
use crate::auth::session::SessionUser;
use crate::db::identity::FamilyNamePatch;
use crate::error::{AppError, ProblemCode, SESSION_COOKIE};
use crate::http::cookie::{clear_session_cookie, set_session_cookie};
use crate::http::guard::{check_origin, reject_bearer};
use crate::http::json_input::parse_patch_me;
use crate::http::rate_limit::peer_ip;
use crate::http::state::AppState;
use crate::validate::{
    normalize_email, validate_family_name, validate_given_name, validate_locale,
    validate_text_scale, validate_week_starts_on,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/auth/login", post(login))
        .route("/api/v1/auth/logout", post(logout))
        .route("/api/v1/auth/me", get(me).patch(patch_me))
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
    reject_bearer(&headers)?;
    let user = require_session(&state, &jar).await?;
    Ok(Json(SessionUserOutput::from(user)))
}

async fn patch_me(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    body: Result<Json<Value>, JsonRejection>,
) -> Result<Json<SessionUserOutput>, AppError> {
    let Json(body) = body.map_err(AppError::from)?;
    reject_bearer(&headers)?;
    check_origin(&headers, &state.public_origin)?;
    let token = jar
        .get(SESSION_COOKIE)
        .map(|c| c.value().to_string())
        .ok_or_else(|| AppError::from_code(ProblemCode::AuthenticationRequired))?;
    require_session(&state, &jar).await?;

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

async fn require_session(state: &AppState, jar: &CookieJar) -> Result<SessionUser, AppError> {
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
    Ok(user)
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
