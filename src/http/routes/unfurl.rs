//! `GET /api/v1/workspaces/{workspace_id}/unfurl` (source
//! `apps/server/src/domains/search/unfurl.ts`).
//!
//! Auth is a live session or PAT (`Access::Any`). Membership is checked in a
//! short transaction before the outbound GET and again after it, so a
//! revocation that finishes during the fetch is not delivered. Rate limits
//! match the source: 60/min per IP and 30/min per user.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::rejection::QueryRejection;
use axum::extract::{ConnectInfo, Extension, Path, Query, State};
use axum::http::HeaderMap;
use axum::routing::get;
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use serde::Deserialize;
use uuid::Uuid;

use crate::error::{AppError, ProblemCode};
use crate::http::authz::{require_request_auth, Access};
use crate::http::rate_limit::peer_ip;
use crate::http::state::AppState;
use crate::integrations::ai::is_member;
use crate::integrations::outbound::{OutboundRejected, URL_MAX};
use crate::integrations::unfurl::{
    fetch_unfurl, oembed_iframe, validate_unfurl_query, UnfurlResult, UNFURL_TIMEOUT,
};
use crate::integrations::Integrations;

const UNFURL_IP_LIMIT: u32 = 60;
const UNFURL_USER_LIMIT: u32 = 30;
const UNFURL_WINDOW: Duration = Duration::from_secs(60);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UnfurlQuery {
    pub url: String,
}

pub fn router(integrations: Arc<Integrations>) -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/workspaces/{workspace_id}/unfurl",
            get(unfurl_route),
        )
        .layer(Extension(integrations))
}

fn map_reject(err: OutboundRejected) -> AppError {
    match err {
        OutboundRejected::Url | OutboundRejected::Address | OutboundRejected::Redirect => {
            AppError::from_code(ProblemCode::InvalidInput)
        }
        OutboundRejected::Resolve => AppError::from_code(ProblemCode::InvalidInput),
    }
}

async fn require_member(
    state: &AppState,
    workspace_id: Uuid,
    user_id: Uuid,
    session_id: Uuid,
) -> Result<(), AppError> {
    match is_member(
        state
            .auth
            .db
            .pool
            .postgres("src/http/routes/unfurl.rs")
            .map_err(crate::http::routes::tasks::internal)?,
        workspace_id,
        user_id,
        session_id,
    )
    .await
    {
        Ok(true) => Ok(()),
        Ok(false) => Err(AppError::from_code(ProblemCode::NotFound)),
        Err(err) => {
            tracing::error!("unfurl membership: {err}");
            Err(AppError::internal())
        }
    }
}

async fn unfurl_route(
    State(state): State<AppState>,
    Extension(integrations): Extension<Arc<Integrations>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
    query: Result<Query<UnfurlQuery>, QueryRejection>,
) -> Result<Json<UnfurlResult>, AppError> {
    let Query(query) = query.map_err(AppError::from)?;
    if query.url.len() > URL_MAX {
        return Err(AppError::from_code(ProblemCode::InvalidInput));
    }
    validate_unfurl_query(&query.url).map_err(map_reject)?;

    let auth =
        require_request_auth(&state, &headers, &jar, Access::Any, Some(workspace_id)).await?;
    let ip = peer_ip(peer.ip());
    if let Err(retry_after) = state
        .rate_limiter
        .allow_window(&format!("unfurl-ip:{ip}"), UNFURL_IP_LIMIT, UNFURL_WINDOW)
        .await
    {
        return Err(AppError::rate_limited(retry_after));
    }
    if let Err(retry_after) = state
        .rate_limiter
        .allow_window(
            &format!("unfurl-user:{}", auth.user_id),
            UNFURL_USER_LIMIT,
            UNFURL_WINDOW,
        )
        .await
    {
        return Err(AppError::rate_limited(retry_after));
    }

    require_member(&state, workspace_id, auth.user_id, auth.credential_id).await?;

    let outbound = integrations.outbound.without_allow_list();
    let mut result = fetch_unfurl(&outbound, &query.url, UNFURL_TIMEOUT)
        .await
        .map_err(map_reject)?;

    require_member(&state, workspace_id, auth.user_id, auth.credential_id).await?;

    let hosts = crate::settings::current_values(
        state
            .auth
            .db
            .pool
            .postgres("src/http/routes/unfurl.rs")
            .map_err(crate::http::routes::tasks::internal)?,
        &state.branding_name,
    )
    .await
    .map_err(|err| {
        tracing::error!("unfurl settings: {err}");
        AppError::internal()
    })?
    .embed
    .hosts;
    result.html = oembed_iframe(&query.url, &hosts);
    Ok(Json(result))
}
