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
use crate::error::{AppError, ProblemCode};
use crate::http::authz::{require_request_auth, Access, RequestAuth};
use crate::http::rate_limit::peer_ip;
use crate::http::routes::projects::map_project_error;
use crate::http::state::AppState;
use crate::search::meili::ParentKinds;
use crate::search::query::{
    query_global_search, query_workspace_search, GlobalSearchRequest, SearchQueryError,
    SearchResultItem, SearchTypeFilter, WorkspaceSearchRequest,
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GlobalSearchQuery {
    pub q: String,
    pub r#type: Option<String>,
    pub tag: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<String>,
    /// Source `searchQuery.mode`. Global search stays lexical (hybrid is ignored).
    pub mode: Option<String>,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/search", get(global_search))
        .route(
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

async fn global_search(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    query: Result<Query<GlobalSearchQuery>, QueryRejection>,
) -> Result<Json<SearchListResponse>, SearchApiError> {
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
    if query.tag.is_some() {
        return Err(AppError::from_code(ProblemCode::InvalidInput).into());
    }
    let tag: Option<Uuid> = None;
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

    let auth = require_request_auth(&state, &headers, &jar, Access::Any, None).await?;
    let (actor_user_id, session_id) = (auth.user_id, auth.credential_id);
    let ip = peer_ip(peer.ip());
    if let Err(retry_after) = state
        .rate_limiter
        .allow_window(&format!("search-ip:{ip}"), SEARCH_IP_LIMIT, SEARCH_WINDOW)
        .await
    {
        return Err(AppError::rate_limited(retry_after).into());
    }
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
    let result = query_global_search(
        &state.auth.db.pool,
        GlobalSearchRequest {
            actor_user_id,
            session_id,
            q: &query.q,
            r#type,
            tag,
            cursor: query.cursor.as_deref(),
            limit,
            meili,
            allowed_kinds: allowed_parent_kinds(&auth),
            token_workspace_id: auth.token_workspace_id,
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
        Err(SearchQueryError::NotFound) => Err(AppError::from_code(ProblemCode::NotFound).into()),
        Err(SearchQueryError::InvalidCursor) => Err(invalid_cursor()),
        Err(SearchQueryError::MeiliUnavailable) => Err(search_unavailable()),
    }
}

async fn workspace_search(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
    query: Result<Query<WorkspaceSearchQuery>, QueryRejection>,
) -> Result<Json<SearchListResponse>, SearchApiError> {
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
    // Tags are not ported yet; refuse the filter instead of silently ignoring it.
    if query.tag.is_some() {
        return Err(AppError::from_code(ProblemCode::InvalidInput).into());
    }
    let tag: Option<Uuid> = None;
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

    // Source order: authenticate, then the IP limit, then the user limit.
    let auth =
        require_request_auth(&state, &headers, &jar, Access::Any, Some(workspace_id)).await?;
    let (actor_user_id, session_id) = (auth.user_id, auth.credential_id);
    let ip = peer_ip(peer.ip());
    if let Err(retry_after) = state
        .rate_limiter
        .allow_window(&format!("search-ip:{ip}"), SEARCH_IP_LIMIT, SEARCH_WINDOW)
        .await
    {
        return Err(AppError::rate_limited(retry_after).into());
    }
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
            hybrid: query.mode.as_deref() == Some("hybrid"),
            embedder: state.search_embedder.as_ref(),
            allowed_kinds: allowed_parent_kinds(&auth),
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

/// Source `allowedContentKinds`: a PAT reads mixed content only through its scoped domains.
fn allowed_parent_kinds(auth: &RequestAuth) -> Option<ParentKinds> {
    let kinds = crate::http::routes::notifications::allowed_content_kinds(auth)?;
    Some(ParentKinds {
        document: kinds.contains(&crate::db::notifications::ContentKind::Document),
        task: kinds.contains(&crate::db::notifications::ContentKind::Task),
    })
}

fn internal(err: sqlx::Error) -> AppError {
    tracing::error!("database error: {}", err);
    AppError::internal()
}
