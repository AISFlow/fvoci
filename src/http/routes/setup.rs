use std::net::SocketAddr;

use axum::extract::rejection::JsonRejection;
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde::Serialize;

use crate::auth::service::SetupError;
use crate::error::{AppError, ProblemCode};
use crate::http::cookie::set_session_cookie;
use crate::http::guard::check_origin;
use crate::http::rate_limit::peer_ip;
use crate::http::state::AppState;
use crate::validate::{normalize_email, normalize_slug, validate_family_name, validate_given_name};

pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/setup", get(setup_status).post(setup_run))
}

#[derive(Serialize)]
struct SetupStatusResponse {
    needed: bool,
    branding: Branding,
}

#[derive(Serialize)]
struct Branding {
    name: String,
}

async fn setup_status(
    State(state): State<AppState>,
) -> Result<Json<SetupStatusResponse>, AppError> {
    let needed = state.auth.setup_needed().await.map_err(internal)?;
    Ok(Json(SetupStatusResponse {
        needed,
        branding: Branding {
            name: state.branding_name.clone(),
        },
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SetupBody {
    email: String,
    password: String,
    given_name: String,
    family_name: Option<String>,
    workspace_slug: String,
    workspace_name: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SetupResponse {
    user_id: String,
    workspace_id: String,
}

async fn setup_run(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Result<Json<SetupBody>, JsonRejection>,
) -> Result<Response, AppError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    let ip = peer_ip(peer.ip());
    if !state
        .rate_limiter
        .allow(&format!("setup:ip:{ip}"), 10)
        .await
    {
        return Err(AppError::from_code(ProblemCode::RateLimited));
    }

    if body.password.len() < 10 {
        return Err(AppError::problem(
            StatusCode::BAD_REQUEST,
            ProblemCode::PasswordInvalid,
        ));
    }

    let email = normalize_email(&body.email)?;
    let workspace_slug = normalize_slug(&body.workspace_slug)?;
    validate_given_name(&body.given_name)?;
    if let Some(family_name) = &body.family_name {
        validate_family_name(family_name)?;
    }
    let workspace_name = body.workspace_name.trim();
    if workspace_name.is_empty() || workspace_name.len() > 100 {
        return Err(AppError::problem(
            StatusCode::BAD_REQUEST,
            ProblemCode::InvalidInput,
        ));
    }

    let result = state
        .auth
        .setup_instance(
            email,
            body.password,
            body.given_name.trim().to_string(),
            body.family_name.map(|s| s.trim().to_string()),
            workspace_slug,
            workspace_name.to_string(),
        )
        .await
        .map_err(internal)?;

    match result {
        Ok((user_id, workspace_id, token)) => {
            let cookie = set_session_cookie(state.cookie_secure, &token);
            let mut response = (
                StatusCode::CREATED,
                Json(SetupResponse {
                    user_id: user_id.to_string(),
                    workspace_id: workspace_id.to_string(),
                }),
            )
                .into_response();
            response
                .headers_mut()
                .append(axum::http::header::SET_COOKIE, cookie.parse().unwrap());
            Ok(response)
        }
        Err(SetupError::Closed) => Err(AppError::from_code(
            ProblemCode::InstanceSetupAlreadyCompleted,
        )),
        Err(SetupError::SlugTaken) => Err(AppError::from_code(ProblemCode::SlugTaken)),
    }
}

fn internal(err: sqlx::Error) -> AppError {
    tracing::error!("database error: {}", sanitize_db_error(&err));
    AppError::problem(StatusCode::INTERNAL_SERVER_ERROR, ProblemCode::InvalidInput)
}

fn sanitize_db_error(err: &sqlx::Error) -> String {
    match err {
        sqlx::Error::Database(db) => db.message().to_string(),
        _ => "database operation failed".to_string(),
    }
}
