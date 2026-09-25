use std::net::SocketAddr;
use std::time::Duration;

use axum::extract::rejection::JsonRejection;
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get};
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use uuid::Uuid;

use crate::api::dto::{CreateHolidayBody, HolidaysListResponse, IcsTokenResponse, OkResponse};
use crate::db::holidays::{
    add_workspace_holiday, list_workspace_holidays, remove_workspace_holiday, HolidayDbError,
};
use crate::db::ics::{read_ics_by_token, rotate_ics_token, IcsDbError};
use crate::error::{AppError, ProblemCode};
use crate::http::guard::check_origin;
use crate::http::rate_limit::peer_ip;
use crate::http::state::AppState;
use crate::ics::{caldav_multistatus, caldav_not_found};
use crate::tasks::parse_iso_date;

const ICS_RATE_WINDOW: Duration = Duration::from_secs(60);
const ICS_RATE_LIMIT: u32 = 60;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/workspaces/{workspace_id}/holidays",
            get(list_holidays_route).post(create_holiday_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/holidays/{date}",
            axum::routing::delete(remove_holiday_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/ics-token",
            axum::routing::post(create_ics_token_route),
        )
        .route("/api/v1/ics/{token}", any(ics_public_route))
}

fn map_holiday_error(err: HolidayDbError) -> AppError {
    match err {
        HolidayDbError::NotFound | HolidayDbError::Forbidden => {
            AppError::from_code(ProblemCode::NotFound)
        }
    }
}

fn map_ics_error(err: IcsDbError) -> AppError {
    match err {
        IcsDbError::NotFound | IcsDbError::Forbidden => AppError::from_code(ProblemCode::NotFound),
    }
}

async fn require_session(
    state: &AppState,
    headers: &HeaderMap,
    jar: &CookieJar,
    access: crate::http::authz::Access,
    workspace_id: Option<Uuid>,
) -> Result<(Uuid, Uuid), AppError> {
    let auth =
        crate::http::authz::require_request_auth(state, headers, jar, access, workspace_id).await?;
    Ok((auth.user_id, auth.credential_id))
}

fn internal(err: sqlx::Error) -> AppError {
    tracing::error!("database error: {}", err);
    AppError::internal()
}

async fn list_holidays_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
) -> Result<Json<HolidaysListResponse>, AppError> {
    let (actor_user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::WorkspaceManage),
        Some(workspace_id),
    )
    .await?;
    let result =
        list_workspace_holidays(&state.auth.db.pool, workspace_id, actor_user_id, session_id)
            .await
            .map_err(internal)?;
    match result {
        Ok(list) => Ok(Json(HolidaysListResponse {
            can_edit: list.can_edit,
            items: list
                .items
                .into_iter()
                .map(|date| date.format("%Y-%m-%d").to_string())
                .collect(),
        })),
        Err(err) => Err(map_holiday_error(err)),
    }
}

async fn create_holiday_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
    body: Result<Json<CreateHolidayBody>, JsonRejection>,
) -> Result<Response, AppError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    let (actor_user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::WorkspaceManage),
        Some(workspace_id),
    )
    .await?;
    let result = add_workspace_holiday(
        &state.auth.db.pool,
        workspace_id,
        actor_user_id,
        session_id,
        body.date,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(()) => Ok((StatusCode::CREATED, Json(OkResponse { ok: true })).into_response()),
        Err(err) => Err(map_holiday_error(err)),
    }
}

async fn remove_holiday_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, date)): Path<(Uuid, String)>,
) -> Result<Json<OkResponse>, AppError> {
    check_origin(&headers, &state.public_origin)?;
    let on_date =
        parse_iso_date(&date).ok_or_else(|| AppError::from_code(ProblemCode::NotFound))?;
    let (actor_user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::WorkspaceManage),
        Some(workspace_id),
    )
    .await?;
    let result = remove_workspace_holiday(
        &state.auth.db.pool,
        workspace_id,
        actor_user_id,
        session_id,
        on_date,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(()) => Ok(Json(OkResponse { ok: true })),
        Err(err) => Err(map_holiday_error(err)),
    }
}

async fn create_ics_token_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
) -> Result<Response, AppError> {
    check_origin(&headers, &state.public_origin)?;
    let (actor_user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::TasksWrite),
        Some(workspace_id),
    )
    .await?;
    let result = rotate_ics_token(
        &state.auth.db.pool,
        workspace_id,
        actor_user_id,
        session_id,
        &state.public_origin,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(url) => Ok((StatusCode::CREATED, Json(IcsTokenResponse { url })).into_response()),
        Err(err) => Err(map_ics_error(err)),
    }
}

async fn enforce_ics_limit(state: &AppState, peer: SocketAddr) -> Result<(), AppError> {
    let ip = peer_ip(peer.ip());
    state
        .rate_limiter
        .allow_window(&format!("ics-ip:{ip}"), ICS_RATE_LIMIT, ICS_RATE_WINDOW)
        .await
        .map_err(AppError::rate_limited)
}

fn calendar_headers(etag: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static("text/calendar; charset=utf-8"),
    );
    if let Ok(value) = HeaderValue::from_str(etag) {
        headers.insert(axum::http::header::ETAG, value);
    }
    headers
}

fn dav_xml_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static("application/xml; charset=utf-8"),
    );
    headers.insert("dav", HeaderValue::from_static("1, calendar-access"));
    headers
}

fn ics_href(token: &str) -> String {
    format!("/api/v1/ics/{token}")
}

async fn ics_public_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    method: Method,
    Path(token): Path<String>,
) -> Result<Response, AppError> {
    if method == Method::OPTIONS {
        enforce_ics_limit(&state, peer).await?;
        let found = read_ics_by_token(&state.auth.db.pool, &token)
            .await
            .map_err(internal)?;
        if found.is_none() {
            return Ok((
                StatusCode::NOT_FOUND,
                dav_xml_headers(),
                caldav_not_found(&ics_href(&token)),
            )
                .into_response());
        }
        let mut headers = HeaderMap::new();
        headers.insert("dav", HeaderValue::from_static("1, calendar-access"));
        headers.insert(
            axum::http::header::ALLOW,
            HeaderValue::from_static("OPTIONS, GET, HEAD, PROPFIND, REPORT"),
        );
        return Ok((StatusCode::NO_CONTENT, headers).into_response());
    }
    let kind = if method == Method::GET {
        IcsReadKind::Get
    } else if method == Method::HEAD {
        IcsReadKind::Head
    } else if method.as_str() == "PROPFIND" || method.as_str() == "REPORT" {
        IcsReadKind::Caldav
    } else {
        return Err(AppError::from_code(ProblemCode::NotFound));
    };
    ics_read(&state, peer, &token, kind).await
}

enum IcsReadKind {
    Get,
    Head,
    Caldav,
}

async fn ics_read(
    state: &AppState,
    peer: SocketAddr,
    token: &str,
    kind: IcsReadKind,
) -> Result<Response, AppError> {
    enforce_ics_limit(state, peer).await?;
    let found = read_ics_by_token(&state.auth.db.pool, token)
        .await
        .map_err(internal)?;
    let Some(found) = found else {
        return match kind {
            IcsReadKind::Caldav => Ok((
                StatusCode::NOT_FOUND,
                dav_xml_headers(),
                caldav_not_found(&ics_href(token)),
            )
                .into_response()),
            IcsReadKind::Get | IcsReadKind::Head => Err(AppError::from_code(ProblemCode::NotFound)),
        };
    };
    match kind {
        IcsReadKind::Caldav => Ok((
            StatusCode::MULTI_STATUS,
            dav_xml_headers(),
            caldav_multistatus(&ics_href(token), &found.ics, &found.etag),
        )
            .into_response()),
        IcsReadKind::Head => Ok((StatusCode::OK, calendar_headers(&found.etag)).into_response()),
        IcsReadKind::Get => {
            Ok((StatusCode::OK, calendar_headers(&found.etag), found.ics).into_response())
        }
    }
}
