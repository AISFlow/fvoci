//! Source `apps/server/src/domains/documents/tags.ts`: the workspace tag pool
//! and tag assignment on wiki and project documents (`documents.*` scopes).

use std::net::SocketAddr;

use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch};
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use serde::Deserialize;
use uuid::Uuid;

use crate::api::collections_dto::{
    DocumentTagAssignBody, DocumentTagCreateBody, DocumentTagListResponse, DocumentTagOutput,
    DocumentTagPatchBody, DocumentTagPoolItemOutput, DocumentTagPoolListResponse,
};
use crate::api::dto::OkResponse;
use crate::auth::scopes::ApiTokenScope;
use crate::collections::{iso_millis, parse_name};
use crate::db::document_tags::{
    assign_tag, create_tag, delete_tag, list_document_tags, list_tags, unassign_tag, update_tag,
    Affiliation, TagDbError, TagRow, TAG_POOL_LIMIT_DEFAULT, TAG_POOL_LIMIT_MAX, TAG_QUERY_MAX,
};
use crate::db::labels::label_color_is_valid;
use crate::error::{AppError, ProblemCode};
use crate::http::guard::check_origin;
use crate::http::routes::collections::{actor, internal, invalid, ApiError};
use crate::http::routes::stars::parse_body_uuid;
use crate::http::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/workspaces/{workspace_id}/document-tags",
            get(list_pool).post(create_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/document-tags/{tag_id}",
            patch(update_route).delete(delete_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/documents/{document_id}/tags",
            get(wiki_list).post(wiki_assign),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/documents/{document_id}/tags/{tag_id}",
            delete(wiki_unassign),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/tags",
            get(project_list).post(project_assign),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/tags/{tag_id}",
            delete(project_unassign),
        )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PoolQuery {
    pub q: Option<String>,
    pub limit: Option<String>,
}

fn map_tag_error(err: TagDbError) -> ApiError {
    match err {
        TagDbError::NotFound => AppError::from_code(ProblemCode::NotFound).into(),
        TagDbError::Forbidden => AppError::from_code(ProblemCode::InsufficientPermissions).into(),
        TagDbError::Conflict => AppError::from_code(ProblemCode::Conflict).into(),
        TagDbError::ProjectArchived => AppError::from_code(ProblemCode::ProjectArchived).into(),
    }
}

fn tag_output(tag: TagRow) -> DocumentTagOutput {
    DocumentTagOutput {
        id: tag.id.to_string(),
        workspace_id: tag.workspace_id.to_string(),
        name: tag.name,
        color: tag.color,
        created_at: iso_millis(tag.created_at),
        updated_at: iso_millis(tag.updated_at),
    }
}

fn trimmed_name(raw: &str) -> Result<String, ApiError> {
    parse_name(&serde_json::Value::String(raw.to_string())).map_err(|_| invalid())
}

/// Source `z.coerce.number().int().min(1).max(100).default(50)`.
fn parse_limit(raw: Option<&str>) -> Result<i64, ApiError> {
    let Some(raw) = raw else {
        return Ok(TAG_POOL_LIMIT_DEFAULT);
    };
    let value: f64 = raw.trim().parse().map_err(|_| invalid())?;
    if raw.trim().is_empty()
        || !value.is_finite()
        || value.fract() != 0.0
        || !(1.0..=TAG_POOL_LIMIT_MAX as f64).contains(&value)
    {
        return Err(invalid());
    }
    Ok(value as i64)
}

async fn list_pool(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
    query: Result<Query<PoolQuery>, QueryRejection>,
) -> Result<Json<DocumentTagPoolListResponse>, ApiError> {
    let actor = actor(
        &state,
        &headers,
        &jar,
        ApiTokenScope::DocumentsRead,
        workspace_id,
        None,
    )
    .await?;
    let Query(query) = query.map_err(AppError::from)?;
    let q = query.q.as_deref().map(str::trim);
    if q.is_some_and(|q| q.encode_utf16().count() > TAG_QUERY_MAX) {
        return Err(invalid());
    }
    let limit = parse_limit(query.limit.as_deref())?;
    let pool = list_tags(&state.auth.db.pool, workspace_id, &actor, q, limit)
        .await
        .map_err(internal)?
        .map_err(map_tag_error)?;
    Ok(Json(DocumentTagPoolListResponse {
        can_create: pool.can_create,
        can_manage: pool.can_manage,
        items: pool
            .items
            .into_iter()
            .map(|tag| DocumentTagPoolItemOutput {
                id: tag.id.to_string(),
                workspace_id: tag.workspace_id.to_string(),
                name: tag.name,
                color: tag.color,
                created_at: iso_millis(tag.created_at),
                updated_at: iso_millis(tag.updated_at),
                assignment_count: tag.assignment_count,
            })
            .collect(),
    }))
}

async fn create_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
    body: Result<Json<DocumentTagCreateBody>, JsonRejection>,
) -> Result<Response, ApiError> {
    check_origin(&headers, &state.public_origin)?;
    let actor = actor(
        &state,
        &headers,
        &jar,
        ApiTokenScope::DocumentsWrite,
        workspace_id,
        Some(peer),
    )
    .await?;
    let Json(body) = body.map_err(AppError::from)?;
    let name = trimmed_name(&body.name)?;
    let color = body.color.unwrap_or_else(|| "gray".to_string());
    if !label_color_is_valid(&color) {
        return Err(invalid());
    }
    let tag = create_tag(&state.auth.db.pool, workspace_id, &actor, &name, &color)
        .await
        .map_err(internal)?
        .map_err(map_tag_error)?;
    Ok((StatusCode::CREATED, Json(tag_output(tag))).into_response())
}

async fn update_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, tag_id)): Path<(Uuid, Uuid)>,
    body: Result<Json<DocumentTagPatchBody>, JsonRejection>,
) -> Result<Json<DocumentTagOutput>, ApiError> {
    check_origin(&headers, &state.public_origin)?;
    let actor = actor(
        &state,
        &headers,
        &jar,
        ApiTokenScope::DocumentsWrite,
        workspace_id,
        Some(peer),
    )
    .await?;
    let Json(body) = body.map_err(AppError::from)?;
    if body.name.is_none() && body.color.is_none() {
        return Err(invalid());
    }
    let name = body.name.as_deref().map(trimmed_name).transpose()?;
    if body
        .color
        .as_deref()
        .is_some_and(|c| !label_color_is_valid(c))
    {
        return Err(invalid());
    }
    let tag = update_tag(
        &state.auth.db.pool,
        workspace_id,
        &actor,
        tag_id,
        name.as_deref(),
        body.color.as_deref(),
    )
    .await
    .map_err(internal)?
    .map_err(map_tag_error)?;
    Ok(Json(tag_output(tag)))
}

async fn delete_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, tag_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<OkResponse>, ApiError> {
    check_origin(&headers, &state.public_origin)?;
    let actor = actor(
        &state,
        &headers,
        &jar,
        ApiTokenScope::DocumentsWrite,
        workspace_id,
        Some(peer),
    )
    .await?;
    delete_tag(&state.auth.db.pool, workspace_id, &actor, tag_id)
        .await
        .map_err(internal)?
        .map_err(map_tag_error)?;
    Ok(Json(OkResponse { ok: true }))
}

async fn list_for(
    state: &AppState,
    headers: &HeaderMap,
    jar: &CookieJar,
    workspace_id: Uuid,
    document_id: Uuid,
    affiliation: Affiliation,
) -> Result<Json<DocumentTagListResponse>, ApiError> {
    let actor = actor(
        state,
        headers,
        jar,
        ApiTokenScope::DocumentsRead,
        workspace_id,
        None,
    )
    .await?;
    let tags = list_document_tags(
        &state.auth.db.pool,
        workspace_id,
        &actor,
        document_id,
        affiliation,
    )
    .await
    .map_err(internal)?
    .map_err(map_tag_error)?;
    Ok(Json(DocumentTagListResponse {
        items: tags.into_iter().map(tag_output).collect(),
    }))
}

async fn assign_for(
    state: &AppState,
    headers: &HeaderMap,
    jar: &CookieJar,
    workspace_id: Uuid,
    document_id: Uuid,
    affiliation: Affiliation,
    body: Result<Json<DocumentTagAssignBody>, JsonRejection>,
) -> Result<Json<DocumentTagOutput>, ApiError> {
    check_origin(headers, &state.public_origin)?;
    let actor = actor(
        state,
        headers,
        jar,
        ApiTokenScope::DocumentsWrite,
        workspace_id,
        None,
    )
    .await?;
    let Json(body) = body.map_err(AppError::from)?;
    let tag_id = parse_body_uuid(&body.tag_id).ok_or_else(invalid)?;
    let tag = assign_tag(
        &state.auth.db.pool,
        workspace_id,
        &actor,
        document_id,
        affiliation,
        tag_id,
    )
    .await
    .map_err(internal)?
    .map_err(map_tag_error)?;
    Ok(Json(tag_output(tag)))
}

async fn unassign_for(
    state: &AppState,
    headers: &HeaderMap,
    jar: &CookieJar,
    workspace_id: Uuid,
    document_id: Uuid,
    affiliation: Affiliation,
    tag_id: Uuid,
) -> Result<Json<OkResponse>, ApiError> {
    check_origin(headers, &state.public_origin)?;
    let actor = actor(
        state,
        headers,
        jar,
        ApiTokenScope::DocumentsWrite,
        workspace_id,
        None,
    )
    .await?;
    unassign_tag(
        &state.auth.db.pool,
        workspace_id,
        &actor,
        document_id,
        affiliation,
        tag_id,
    )
    .await
    .map_err(internal)?
    .map_err(map_tag_error)?;
    Ok(Json(OkResponse { ok: true }))
}

async fn wiki_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, document_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<DocumentTagListResponse>, ApiError> {
    list_for(
        &state,
        &headers,
        &jar,
        workspace_id,
        document_id,
        Affiliation::Wiki,
    )
    .await
}

async fn wiki_assign(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, document_id)): Path<(Uuid, Uuid)>,
    body: Result<Json<DocumentTagAssignBody>, JsonRejection>,
) -> Result<Json<DocumentTagOutput>, ApiError> {
    assign_for(
        &state,
        &headers,
        &jar,
        workspace_id,
        document_id,
        Affiliation::Wiki,
        body,
    )
    .await
}

async fn wiki_unassign(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, document_id, tag_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<Json<OkResponse>, ApiError> {
    unassign_for(
        &state,
        &headers,
        &jar,
        workspace_id,
        document_id,
        Affiliation::Wiki,
        tag_id,
    )
    .await
}

async fn project_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id, document_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<Json<DocumentTagListResponse>, ApiError> {
    list_for(
        &state,
        &headers,
        &jar,
        workspace_id,
        document_id,
        Affiliation::Project(project_id),
    )
    .await
}

async fn project_assign(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id, document_id)): Path<(Uuid, Uuid, Uuid)>,
    body: Result<Json<DocumentTagAssignBody>, JsonRejection>,
) -> Result<Json<DocumentTagOutput>, ApiError> {
    assign_for(
        &state,
        &headers,
        &jar,
        workspace_id,
        document_id,
        Affiliation::Project(project_id),
        body,
    )
    .await
}

async fn project_unassign(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id, document_id, tag_id)): Path<(Uuid, Uuid, Uuid, Uuid)>,
) -> Result<Json<OkResponse>, ApiError> {
    unassign_for(
        &state,
        &headers,
        &jar,
        workspace_id,
        document_id,
        Affiliation::Project(project_id),
        tag_id,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_limit_follows_source_coercion() {
        assert_eq!(parse_limit(None).unwrap(), 50);
        assert_eq!(parse_limit(Some("100")).unwrap(), 100);
        assert!(parse_limit(Some("0")).is_err());
        assert!(parse_limit(Some("101")).is_err());
        assert!(parse_limit(Some("1.5")).is_err());
    }
}
