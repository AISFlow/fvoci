use std::net::SocketAddr;

use axum::extract::rejection::JsonRejection;
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use serde::de::Deserializer;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::api::dto::{
    AncestorResponse, AncestorsResponse, BodyResponse, DocumentMetaResponse,
    PatchDocumentBody, TreeNodeResponse, TreeResponse,
};
use crate::auth::session::SessionUser;
use crate::db::documents::{
    create_wiki_document, get_wiki_document, list_wiki_ancestors, list_wiki_tree,
    update_wiki_document_meta, CreateDocumentInput, DocumentDbError, DocumentMeta,
    UpdateDocumentMetaInput, MAX_TREE_DEPTH,
};
use crate::error::{AppError, ProblemCode, SESSION_COOKIE};
use crate::http::guard::{check_origin, reject_bearer};
use crate::http::rate_limit::peer_ip;
use crate::http::state::AppState;

/// Wiki resource routes using current session and transactional document authorization.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/workspaces/{workspace_id}/documents",
            post(create_document),
        )
        .route("/api/v1/workspaces/{workspace_id}/tree", get(list_tree))
        .route(
            "/api/v1/workspaces/{workspace_id}/documents/{document_id}",
            get(get_document).patch(patch_document),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/documents/{document_id}/ancestors",
            get(get_ancestors),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/documents/{document_id}/body",
            get(get_body),
        )
}

/// Source `parentId` is `uuid.nullable()`: present and null is allowed, omitted is not.
/// `#[serde(default)]` plus a third Missing variant is required; a wrapper around
/// `Option` would treat omitted fields as null because serde's missing-field path
/// visits `none`.
#[derive(Debug, Default, PartialEq, Eq)]
enum RequiredNullable<T> {
    #[default]
    Missing,
    Null,
    Value(T),
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for RequiredNullable<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match Option::<T>::deserialize(deserializer)? {
            None => Self::Null,
            Some(value) => Self::Value(value),
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateDocumentBody {
    #[serde(default)]
    parent_id: RequiredNullable<Uuid>,
    title: String,
    icon: Option<Option<String>>,
}

#[derive(Deserialize)]
struct TreeQuery {
    tag: Option<String>,
}

#[derive(Deserialize)]
struct BodyQuery {
    format: Option<String>,
}

enum DocumentApiError {
    App(AppError),
    Coded {
        status: StatusCode,
        code: &'static str,
        title: String,
        params: Option<Value>,
    },
}

impl From<AppError> for DocumentApiError {
    fn from(value: AppError) -> Self {
        Self::App(value)
    }
}

impl IntoResponse for DocumentApiError {
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

async fn create_document(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
    body: Result<Json<CreateDocumentBody>, JsonRejection>,
) -> Result<Response, DocumentApiError> {
    let Json(body) = body.map_err(AppError::from)?;
    reject_bearer(&headers)?;
    check_origin(&headers, &state.public_origin)?;
    let title = body.title.trim();
    if !crate::db::documents::title_is_valid(title) {
        return Err(AppError::from_code(ProblemCode::InvalidInput).into());
    }
    if let Some(Some(icon)) = body.icon.as_ref() {
        if !crate::db::documents::icon_is_valid(icon) {
            return Err(AppError::from_code(ProblemCode::InvalidInput).into());
        }
    }
    let parent_id = match body.parent_id {
        RequiredNullable::Missing => {
            return Err(AppError::from_code(ProblemCode::InvalidInput).into());
        }
        RequiredNullable::Null => None,
        RequiredNullable::Value(id) => Some(id),
    };
    let (user, session_id) = require_session(&state, &jar).await?;
    let user_id = parse_user_id(&user.user_id)?;
    let ip = peer_ip(peer.ip());
    let result = create_wiki_document(
        &state.auth.db.pool,
        workspace_id,
        user_id,
        session_id,
        CreateDocumentInput {
            parent_id,
            title,
            icon: body.icon.as_ref().map(|icon| icon.as_deref()),
        },
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(meta) => Ok((StatusCode::CREATED, Json(meta_response(&meta, true))).into_response()),
        Err(err) => Err(map_document_error(err)),
    }
}

async fn get_document(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, document_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<DocumentMetaResponse>, DocumentApiError> {
    reject_bearer(&headers)?;
    let (user, session_id) = require_session(&state, &jar).await?;
    let user_id = parse_user_id(&user.user_id)?;
    let result = get_wiki_document(
        &state.auth.db.pool,
        workspace_id,
        user_id,
        session_id,
        document_id,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(meta) => Ok(Json(meta_response(&meta, false))),
        Err(err) => Err(map_document_error(err)),
    }
}

async fn patch_document(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, document_id)): Path<(Uuid, Uuid)>,
    body: Result<Json<PatchDocumentBody>, JsonRejection>,
) -> Result<Json<DocumentMetaResponse>, DocumentApiError> {
    let Json(body) = body.map_err(AppError::from)?;
    reject_bearer(&headers)?;
    check_origin(&headers, &state.public_origin)?;
    if let Some(title) = body.title.as_ref() {
        if !crate::db::documents::title_is_valid(title) {
            return Err(AppError::from_code(ProblemCode::InvalidInput).into());
        }
    }
    if let Some(Some(icon)) = body.icon.as_ref() {
        if !crate::db::documents::icon_is_valid(icon) {
            return Err(AppError::from_code(ProblemCode::InvalidInput).into());
        }
    }
    if let Some(status) = body.status.as_ref() {
        if !crate::db::documents::status_is_valid(status) {
            return Err(AppError::from_code(ProblemCode::InvalidInput).into());
        }
    }
    let (user, session_id) = require_session(&state, &jar).await?;
    let user_id = parse_user_id(&user.user_id)?;
    let ip = peer_ip(peer.ip());
    let title = body.title.as_deref().map(str::trim);
    let result = update_wiki_document_meta(
        &state.auth.db.pool,
        workspace_id,
        user_id,
        session_id,
        document_id,
        UpdateDocumentMetaInput {
            title,
            icon: body.icon.as_ref().map(|icon| icon.as_deref()),
            status: body.status.as_deref(),
        },
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(meta) => Ok(Json(meta_response(&meta, false))),
        Err(err) => Err(map_document_error(err)),
    }
}

async fn list_tree(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
    Query(query): Query<TreeQuery>,
) -> Result<Json<TreeResponse>, DocumentApiError> {
    reject_bearer(&headers)?;
    if query.tag.is_some() {
        return Err(AppError::from_code(ProblemCode::InvalidInput).into());
    }
    let (user, session_id) = require_session(&state, &jar).await?;
    let user_id = parse_user_id(&user.user_id)?;
    let result = list_wiki_tree(&state.auth.db.pool, workspace_id, user_id, session_id)
        .await
        .map_err(internal)?;
    match result {
        Ok(items) => Ok(Json(TreeResponse {
            items: items
                .into_iter()
                .map(|n| TreeNodeResponse {
                    id: n.id.to_string(),
                    workspace_id: n.workspace_id.to_string(),
                    parent_id: n.parent_id.map(|id| id.to_string()),
                    project_id: n.project_id.map(|id| id.to_string()),
                    title: n.title,
                    icon: n.icon,
                    path: n.path,
                    sort_key: n.sort_key,
                    number: n.number,
                    status: n.status,
                })
                .collect(),
        })),
        Err(err) => Err(map_document_error(err)),
    }
}

async fn get_ancestors(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, document_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<AncestorsResponse>, DocumentApiError> {
    reject_bearer(&headers)?;
    let (user, session_id) = require_session(&state, &jar).await?;
    let user_id = parse_user_id(&user.user_id)?;
    let result = list_wiki_ancestors(
        &state.auth.db.pool,
        workspace_id,
        user_id,
        session_id,
        document_id,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(items) => Ok(Json(AncestorsResponse {
            items: items
                .into_iter()
                .map(|n| AncestorResponse {
                    id: n.id.to_string(),
                    title: n.title,
                    icon: n.icon,
                    path: n.path,
                    project_id: n.project_id.map(|id| id.to_string()),
                    number: n.number,
                })
                .collect(),
        })),
        Err(err) => Err(map_document_error(err)),
    }
}

async fn get_body(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, document_id)): Path<(Uuid, Uuid)>,
    Query(query): Query<BodyQuery>,
) -> Result<Json<BodyResponse>, DocumentApiError> {
    reject_bearer(&headers)?;
    if query.format.is_some() {
        return Err(AppError::from_code(ProblemCode::InvalidInput).into());
    }
    let (user, session_id) = require_session(&state, &jar).await?;
    let user_id = parse_user_id(&user.user_id)?;
    let result = get_wiki_document(
        &state.auth.db.pool,
        workspace_id,
        user_id,
        session_id,
        document_id,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(meta) => Ok(Json(BodyResponse {
            content_json: meta.content_json,
            version: meta.version,
        })),
        Err(err) => Err(map_document_error(err)),
    }
}

fn meta_response(meta: &DocumentMeta, include_display_id: bool) -> DocumentMetaResponse {
    DocumentMetaResponse {
        id: meta.id.to_string(),
        workspace_id: meta.workspace_id.to_string(),
        title: meta.title.clone(),
        number: meta.number,
        icon: meta.icon.clone(),
        path: meta.path.clone(),
        parent_id: meta.parent_id.map(|id| id.to_string()),
        sort_key: meta.sort_key.clone(),
        project_id: meta.project_id.map(|id| id.to_string()),
        status: meta.status.clone(),
        schema_version: meta.schema_version,
        version: meta.version,
        created_by: meta.created_by.to_string(),
        created_at: meta.created_at,
        updated_at: meta.updated_at,
        display_id: if include_display_id {
            meta.display_id.clone()
        } else {
            None
        },
    }
}

fn map_document_error(err: DocumentDbError) -> DocumentApiError {
    match err {
        DocumentDbError::NotFound | DocumentDbError::Forbidden => {
            AppError::from_code(ProblemCode::NotFound).into()
        }
        DocumentDbError::AffiliationMismatch => DocumentApiError::Coded {
            status: StatusCode::BAD_REQUEST,
            code: "document_affiliation_mismatch",
            title: "document affiliation mismatch".to_string(),
            params: None,
        },
        DocumentDbError::DepthLimit => DocumentApiError::Coded {
            status: StatusCode::BAD_REQUEST,
            code: "tree_depth_limit",
            title: format!("tree depth would exceed limit ({MAX_TREE_DEPTH})"),
            params: Some(json!({ "limit": MAX_TREE_DEPTH })),
        },
        DocumentDbError::InvalidSortKey => AppError::internal().into(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_parent_id_is_required_nullable() {
        let omitted: CreateDocumentBody = serde_json::from_str(r#"{"title":"X"}"#).unwrap();
        assert_eq!(omitted.parent_id, RequiredNullable::Missing);
        let null_parent: CreateDocumentBody =
            serde_json::from_str(r#"{"parentId":null,"title":"X"}"#).unwrap();
        assert_eq!(null_parent.parent_id, RequiredNullable::Null);
        let with_parent: CreateDocumentBody = serde_json::from_str(
            r#"{"parentId":"11111111-1111-4111-8111-111111111111","title":"X"}"#,
        )
        .unwrap();
        assert_eq!(
            with_parent.parent_id,
            RequiredNullable::Value(
                Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap()
            )
        );
    }

    #[test]
    fn patch_icon_distinguishes_omitted_null_and_value() {
        let omitted: PatchDocumentBody = serde_json::from_str(r#"{"title":"T"}"#).unwrap();
        assert_eq!(omitted.icon, None);
        let clear: PatchDocumentBody = serde_json::from_str(r#"{"icon":null}"#).unwrap();
        assert_eq!(clear.icon, Some(None));
        let set: PatchDocumentBody = serde_json::from_str(r#"{"icon":"📄"}"#).unwrap();
        assert_eq!(set.icon, Some(Some("📄".to_string())));
    }

    #[test]
    fn patch_document_body_deserializes_double_option_icon() {
        let body: PatchDocumentBody =
            serde_json::from_str(r#"{"icon":"📄","status":"draft"}"#).unwrap();
        assert_eq!(body.icon, Some(Some("📄".to_string())));
        assert_eq!(body.status.as_deref(), Some("draft"));
    }
}
