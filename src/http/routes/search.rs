use std::net::SocketAddr;
use std::time::Duration;

use axum::extract::rejection::QueryRejection;
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::api::dto::{SearchItemOutput, SearchListResponse, SearchSnippetPiece};
use crate::auth::session::SessionUser;
use crate::error::{AppError, ProblemCode, SESSION_COOKIE};
use crate::http::guard::reject_bearer;
use crate::http::rate_limit::peer_ip;
use crate::http::routes::projects::map_project_error;
use crate::http::state::AppState;
use crate::search::query::{
    query_workspace_search, SearchQueryError, SearchResultItem, SearchTypeFilter,
    WorkspaceSearchRequest,
};

const SEARCH_IP_LIMIT: u32 = 120;
const SEARCH_USER_LIMIT: u32 = 60;
const SEARCH_WINDOW: Duration = Duration::from_secs(60);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceSearchQuery {
    pub q: String,
    pub r#type: Option<String>,
    pub tag: Option<String>,
    pub project_id: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<String>,
    pub mode: Option<String>,
}

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/api/v1/workspaces/{workspace_id}/search",
        get(workspace_search),
    )
}

enum SearchApiError {
    App(AppError),
    Coded {
        status: StatusCode,
        code: &'static str,
        title: String,
        params: Option<serde_json::Value>,
    },
}

impl From<AppError> for SearchApiError {
    fn from(value: AppError) -> Self {
        Self::App(value)
    }
}

impl IntoResponse for SearchApiError {
    fn into_response(self) -> Response {
        match self {
            Self::App(err) => err.into_response(),
            Self::Coded {
                status,
                code,
                title,
                params,
            } => {
                let mut body = json!({
                    "type": "about:blank",
                    "title": title,
                    "status": status.as_u16(),
                    "code": code,
                });
                if let Some(params) = params {
                    body["params"] = params;
                }
                let mut headers = HeaderMap::new();
                headers.insert(
                    axum::http::header::CONTENT_TYPE,
                    HeaderValue::from_static("application/problem+json"),
                );
                (status, headers, Json(body)).into_response()
            }
        }
    }
}

fn search_unavailable() -> SearchApiError {
    SearchApiError::Coded {
        status: StatusCode::SERVICE_UNAVAILABLE,
        code: "search_unavailable",
        title: "search unavailable".into(),
        params: None,
    }
}

fn invalid_cursor() -> SearchApiError {
    AppError {
        status: StatusCode::BAD_REQUEST,
        code: ProblemCode::InvalidInput,
        source: None,
        params: Some(json!({"code":"invalid_cursor"})),
        retry_after: None,
    }
    .into()
}

async fn workspace_search(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
    query: Result<Query<WorkspaceSearchQuery>, QueryRejection>,
) -> Result<Json<SearchListResponse>, SearchApiError> {
    reject_bearer(&headers)?;
    let Query(query) = query.map_err(AppError::from)?;
    if query.q.trim().is_empty() || query.q.chars().count() > 200 {
        return Err(AppError::from_code(ProblemCode::InvalidInput).into());
    }
    if query.cursor.as_ref().is_some_and(|c| c.len() > 1024) {
        return Err(invalid_cursor());
    }
    let r#type = SearchTypeFilter::parse(query.r#type.as_deref().unwrap_or("all"))
        .ok_or_else(|| AppError::from_code(ProblemCode::InvalidInput))?;
    if let Some(mode) = query.mode.as_deref() {
        if mode != "lexical" && mode != "hybrid" {
            return Err(AppError::from_code(ProblemCode::InvalidInput).into());
        }
    }
    let project_id = match query.project_id.as_deref() {
        None => None,
        Some(raw) => {
            Some(Uuid::parse_str(raw).map_err(|_| AppError::from_code(ProblemCode::InvalidInput))?)
        }
    };
    let tag = match query.tag.as_deref() {
        None => None,
        Some(raw) => {
            Some(Uuid::parse_str(raw).map_err(|_| AppError::from_code(ProblemCode::InvalidInput))?)
        }
    };
    let limit = match query.limit.as_deref() {
        None => 20u32,
        Some(raw) => {
            let parsed: u32 = raw
                .parse()
                .map_err(|_| AppError::from_code(ProblemCode::InvalidInput))?;
            if !(1..=50).contains(&parsed) {
                return Err(AppError::from_code(ProblemCode::InvalidInput).into());
            }
            parsed
        }
    };

    let ip = peer_ip(peer.ip());
    if let Err(retry_after) = state
        .rate_limiter
        .allow_window(&format!("search-ip:{ip}"), SEARCH_IP_LIMIT, SEARCH_WINDOW)
        .await
    {
        return Err(AppError::rate_limited(retry_after).into());
    }
    let (user, session_id) = require_session(&state, &jar).await?;
    let actor_user_id = parse_user_id(&user.user_id)?;
    if let Err(retry_after) = state
        .rate_limiter
        .allow_window(
            &format!("search-user:{actor_user_id}"),
            SEARCH_USER_LIMIT,
            SEARCH_WINDOW,
        )
        .await
    {
        return Err(AppError::rate_limited(retry_after).into());
    }

    let Some(meili) = state.meili.as_ref() else {
        return Err(search_unavailable());
    };
    let result = query_workspace_search(
        &state.auth.db.pool,
        WorkspaceSearchRequest {
            workspace_id,
            actor_user_id,
            session_id,
            q: &query.q,
            r#type,
            project_id,
            tag,
            cursor: query.cursor.as_deref(),
            limit,
            meili,
        },
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(page) => Ok(Json(SearchListResponse {
            items: page.items.into_iter().map(to_output).collect(),
            next_cursor: page.next_cursor,
        })),
        Err(SearchQueryError::Forbidden) => {
            Err(AppError::from_code(ProblemCode::AuthenticationRequired).into())
        }
        Err(SearchQueryError::NotFound) => {
            Err(map_project_error(crate::db::projects::ProjectDbError::NotFound).into())
        }
        Err(SearchQueryError::InvalidCursor) => Err(invalid_cursor()),
        Err(SearchQueryError::MeiliUnavailable) => Err(search_unavailable()),
    }
}

fn to_output(item: SearchResultItem) -> SearchItemOutput {
    SearchItemOutput {
        r#type: item.r#type.as_str().to_string(),
        id: item.id.to_string(),
        title: item.title,
        display_id: item.display_id,
        extract_status: item.extract_status,
        chunk_no: item.chunk_no,
        snippet: item.snippet.map(|pieces| {
            pieces
                .into_iter()
                .map(|p| SearchSnippetPiece {
                    text: p.text,
                    r#match: p.r#match,
                })
                .collect()
        }),
        project_id: item.project_id.map(|id| id.to_string()),
        document_id: item.document_id.map(|id| id.to_string()),
        task_id: item.task_id.map(|id| id.to_string()),
        score: item.score,
        updated_at: item.updated_at.to_rfc3339(),
        workspace_id: item.workspace_id.to_string(),
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
