//! Source `apps/server/src/domains/stars/routes.ts`: stars and recent items.
//! Auth `any`: sessions see everything; API tokens are narrowed to the content
//! kinds their read scopes grant (source `allowedContentKinds`).

use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get};
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use chrono::SecondsFormat;
use serde::Deserialize;
use uuid::Uuid;

use crate::api::dto::{
    OkResponse, RecentItemOutput, RecentListResponse, StarCreateBody, StarItemOutput,
    StarListResponse,
};
use crate::db::stars::{
    add_star, kind_str, list_recent, list_stars, remove_star, RecentItem, StarDbError, StarItem,
    StarTarget, RECENT_DEFAULT_LIMIT, RECENT_MAX_LIMIT,
};
use crate::error::{AppError, ProblemCode};
use crate::http::authz::{require_request_auth, Access};
use crate::http::guard::check_origin;
use crate::http::routes::notifications::allowed_content_kinds;
use crate::http::state::AppState;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecentQuery {
    pub limit: Option<String>,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/workspaces/{workspace_id}/stars",
            get(list_stars_route).post(create_star_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/stars/{id}",
            delete(remove_star_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/recent",
            get(list_recent_route),
        )
}

fn iso(at: chrono::DateTime<chrono::Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn star_output(item: StarItem) -> StarItemOutput {
    StarItemOutput {
        id: item.id.to_string(),
        r#type: kind_str(item.kind).to_string(),
        target_id: item.target_id.to_string(),
        title: item.title,
        project_id: item.project_id.map(|id| id.to_string()),
        number: item.number,
        created_at: iso(item.created_at),
    }
}

fn recent_output(item: RecentItem) -> RecentItemOutput {
    RecentItemOutput {
        r#type: kind_str(item.kind).to_string(),
        id: item.id.to_string(),
        title: item.title,
        project_id: item.project_id.map(|id| id.to_string()),
        number: item.number,
        updated_at: iso(item.updated_at),
    }
}

/// Non-member, dead credential and missing target all read as 404 (the Rust
/// API hides existence; the source documents 404 for these routes).
fn map_star_error(err: StarDbError) -> AppError {
    match err {
        StarDbError::NotFound | StarDbError::Forbidden => {
            AppError::from_code(ProblemCode::NotFound)
        }
    }
}

fn internal(err: sqlx::Error) -> AppError {
    tracing::error!("database error: {}", err);
    AppError::internal()
}

/// Source `z.coerce.number().int().min(1).max(50).default(20)`.
fn parse_recent_limit(raw: Option<&str>) -> Result<i64, AppError> {
    let Some(raw) = raw else {
        return Ok(RECENT_DEFAULT_LIMIT);
    };
    let value: f64 = raw
        .trim()
        .parse()
        .map_err(|_| AppError::from_code(ProblemCode::InvalidInput))?;
    if raw.trim().is_empty()
        || !value.is_finite()
        || value.fract() != 0.0
        || !(1.0..=RECENT_MAX_LIMIT as f64).contains(&value)
    {
        return Err(AppError::from_code(ProblemCode::InvalidInput));
    }
    Ok(value as i64)
}

/// Hyphenated form only, like the source `uuid` primitive.
pub(crate) fn parse_body_uuid(value: &str) -> Option<Uuid> {
    if value.len() != 36 {
        return None;
    }
    Uuid::parse_str(value).ok()
}

fn parse_star_target(body: &StarCreateBody) -> Result<StarTarget, AppError> {
    let id = parse_body_uuid(&body.id)
        .ok_or_else(|| AppError::with_source(ProblemCode::InvalidInput, "/id"))?;
    match body.r#type.as_str() {
        "document" => Ok(StarTarget::Document(id)),
        "task" => Ok(StarTarget::Task(id)),
        _ => Err(AppError::with_source(ProblemCode::InvalidInput, "/type")),
    }
}

async fn list_stars_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
) -> Result<Json<StarListResponse>, AppError> {
    let auth =
        require_request_auth(&state, &headers, &jar, Access::Any, Some(workspace_id)).await?;
    let kinds = allowed_content_kinds(&auth);
    let items = list_stars(
        &state.auth.db.pool,
        workspace_id,
        auth.user_id,
        auth.credential_id,
        kinds.as_deref(),
    )
    .await
    .map_err(internal)?
    .map_err(map_star_error)?;
    Ok(Json(StarListResponse {
        items: items.into_iter().map(star_output).collect(),
    }))
}

async fn create_star_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
    body: Result<Json<StarCreateBody>, JsonRejection>,
) -> Result<Response, AppError> {
    check_origin(&headers, &state.public_origin)?;
    let auth =
        require_request_auth(&state, &headers, &jar, Access::Any, Some(workspace_id)).await?;
    let Json(body) = body.map_err(AppError::from)?;
    let target = parse_star_target(&body)?;
    let kinds = allowed_content_kinds(&auth);
    let item = add_star(
        &state.auth.db.pool,
        workspace_id,
        auth.user_id,
        auth.credential_id,
        target,
        kinds.as_deref(),
    )
    .await
    .map_err(internal)?
    .map_err(map_star_error)?;
    Ok((StatusCode::CREATED, Json(star_output(item))).into_response())
}

async fn remove_star_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, star_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<OkResponse>, AppError> {
    check_origin(&headers, &state.public_origin)?;
    let auth =
        require_request_auth(&state, &headers, &jar, Access::Any, Some(workspace_id)).await?;
    let kinds = allowed_content_kinds(&auth);
    remove_star(
        &state.auth.db.pool,
        workspace_id,
        auth.user_id,
        auth.credential_id,
        star_id,
        kinds.as_deref(),
    )
    .await
    .map_err(internal)?
    .map_err(map_star_error)?;
    Ok(Json(OkResponse { ok: true }))
}

async fn list_recent_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
    query: Result<Query<RecentQuery>, QueryRejection>,
) -> Result<Json<RecentListResponse>, AppError> {
    let auth =
        require_request_auth(&state, &headers, &jar, Access::Any, Some(workspace_id)).await?;
    let Query(query) = query.map_err(AppError::from)?;
    let limit = parse_recent_limit(query.limit.as_deref())?;
    let kinds = allowed_content_kinds(&auth);
    let items = list_recent(
        &state.auth.db.pool,
        workspace_id,
        auth.user_id,
        auth.credential_id,
        limit,
        kinds.as_deref(),
    )
    .await
    .map_err(internal)?
    .map_err(map_star_error)?;
    Ok(Json(RecentListResponse {
        items: items.into_iter().map(recent_output).collect(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_limit_follows_source_coercion() {
        assert_eq!(parse_recent_limit(None).unwrap(), 20);
        assert_eq!(parse_recent_limit(Some("50")).unwrap(), 50);
        assert_eq!(parse_recent_limit(Some("1")).unwrap(), 1);
        assert!(parse_recent_limit(Some("0")).is_err());
        assert!(parse_recent_limit(Some("51")).is_err());
        assert!(parse_recent_limit(Some("2.5")).is_err());
        assert!(parse_recent_limit(Some("abc")).is_err());
    }
}
