use std::net::SocketAddr;

use axum::extract::rejection::QueryRejection;
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::HeaderMap;
use axum::routing::get;
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use serde::Deserialize;
use uuid::Uuid;

use crate::api::dto::{LookupItemOutput, LookupListResponse};
use crate::auth::session::SessionUser;
use crate::db::lookup::{lookup_display_id, LookupDbError};
use crate::error::{AppError, ProblemCode, SESSION_COOKIE};
use crate::http::guard::reject_bearer;
use crate::http::rate_limit::peer_ip;
use crate::http::routes::projects::map_project_error;
use crate::http::state::AppState;

const LOOKUP_IP_LIMIT: u32 = 120;
const LOOKUP_USER_LIMIT: u32 = 60;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LookupQuery {
    pub project_id: Option<Uuid>,
}

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/api/v1/workspaces/{workspace_id}/lookup/{display_id}",
        get(lookup_display_id_route),
    )
}

async fn lookup_display_id_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, display_id)): Path<(Uuid, String)>,
    query: Result<Query<LookupQuery>, QueryRejection>,
) -> Result<Json<LookupListResponse>, AppError> {
    reject_bearer(&headers)?;
    if display_id.trim().is_empty() || display_id.chars().count() > 64 {
        return Err(AppError::from_code(ProblemCode::InvalidInput));
    }
    let Query(query) = query.map_err(AppError::from)?;
    let ip = peer_ip(peer.ip());
    if let Err(retry_after) = state
        .rate_limiter
        .allow(&format!("lookup:ip:{ip}"), LOOKUP_IP_LIMIT)
        .await
    {
        return Err(AppError::rate_limited(retry_after));
    }
    let (user, session_id) = require_session(&state, &jar).await?;
    let actor_user_id = parse_user_id(&user.user_id)?;
    if let Err(retry_after) = state
        .rate_limiter
        .allow(&format!("lookup:user:{actor_user_id}"), LOOKUP_USER_LIMIT)
        .await
    {
        return Err(AppError::rate_limited(retry_after));
    }
    let result = lookup_display_id(
        &state.auth.db.pool,
        workspace_id,
        actor_user_id,
        session_id,
        &display_id,
        query.project_id,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(items) => Ok(Json(LookupListResponse {
            items: items
                .into_iter()
                .map(|item| LookupItemOutput {
                    kind: item.kind,
                    id: item.id.to_string(),
                    display_id: item.display_id,
                    title: item.title,
                    project_id: item.project_id.map(|id| id.to_string()),
                })
                .collect(),
        })),
        Err(LookupDbError::NotFound) | Err(LookupDbError::Forbidden) => Err(map_project_error(
            crate::db::projects::ProjectDbError::NotFound,
        )),
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
