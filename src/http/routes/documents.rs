use std::net::SocketAddr;

use axum::extract::rejection::JsonRejection;
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::api::dto::{
    AncestorResponse, AncestorsResponse, BodyResponse, CreateDocumentBody, DocumentMetaResponse,
    MoveDocumentBody, OkResponse, PatchDocumentBody, RequiredNullable, SortDocumentBody,
    TrashItemResponse, TrashListResponse, TreeNodeResponse, TreeResponse,
};
use crate::auth::session::SessionUser;
use crate::db::documents::{
    create_wiki_document, get_wiki_document, list_trashed_wiki_documents, list_wiki_ancestors,
    list_wiki_tree, move_wiki_document, reorder_wiki_document, restore_wiki_document,
    trash_wiki_document, update_wiki_document_meta, CreateDocumentInput, DocumentDbError,
    DocumentMeta, TrashChildrenMode, UpdateDocumentMetaInput, MAX_TREE_DEPTH,
};
use crate::documents::export::{
    export_filename, render_document_export, ExportFormat, ExportRenderError,
};
use crate::error::{AppError, ProblemCode};
use crate::http::guard::check_origin;
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
            get(get_document)
                .patch(patch_document)
                .delete(trash_document),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/documents/{document_id}/ancestors",
            get(get_ancestors),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/documents/{document_id}/move",
            post(move_document),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/documents/{document_id}/sort",
            post(sort_document),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/documents/{document_id}/trash",
            post(trash_document),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/documents/{document_id}/restore",
            post(restore_document),
        )
        .route("/api/v1/workspaces/{workspace_id}/trash", get(list_trash))
        .route(
            "/api/v1/workspaces/{workspace_id}/documents/{document_id}/body",
            get(get_body),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/documents/{document_id}/md",
            get(export_markdown),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/documents/{document_id}/pdf",
            get(export_pdf),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/documents/{document_id}/docx",
            get(export_docx),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/documents/{document_id}/pptx",
            get(export_pptx),
        )
}

#[derive(Deserialize)]
struct TreeQuery {
    tag: Option<String>,
}

#[derive(Deserialize)]
struct BodyQuery {
    format: Option<String>,
}

#[derive(Deserialize)]
struct TrashQuery {
    children: Option<String>,
}

pub(crate) enum DocumentApiError {
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
    let (_user, user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::DocumentsWrite),
        Some(workspace_id),
    )
    .await?;
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
    let (_user, user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::DocumentsRead),
        Some(workspace_id),
    )
    .await?;
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
    let (_user, user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::DocumentsWrite),
        Some(workspace_id),
    )
    .await?;
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
    if query.tag.is_some() {
        return Err(AppError::from_code(ProblemCode::InvalidInput).into());
    }
    let (_user, user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::DocumentsRead),
        Some(workspace_id),
    )
    .await?;
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
    let (_user, user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::DocumentsRead),
        Some(workspace_id),
    )
    .await?;
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

async fn move_document(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, document_id)): Path<(Uuid, Uuid)>,
    body: Result<Json<MoveDocumentBody>, JsonRejection>,
) -> Result<Json<DocumentMetaResponse>, DocumentApiError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    let (_user, user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::DocumentsWrite),
        Some(workspace_id),
    )
    .await?;
    let ip = peer_ip(peer.ip());
    let result = move_wiki_document(
        &state.auth.db.pool,
        workspace_id,
        user_id,
        session_id,
        document_id,
        body.new_parent_id,
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(meta) => Ok(Json(meta_response(&meta, false))),
        Err(err) => Err(map_document_error(err)),
    }
}

async fn sort_document(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, document_id)): Path<(Uuid, Uuid)>,
    body: Result<Json<SortDocumentBody>, JsonRejection>,
) -> Result<Json<DocumentMetaResponse>, DocumentApiError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    let (_user, user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::DocumentsWrite),
        Some(workspace_id),
    )
    .await?;
    let ip = peer_ip(peer.ip());
    let result = reorder_wiki_document(
        &state.auth.db.pool,
        workspace_id,
        user_id,
        session_id,
        document_id,
        body.after_id,
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(meta) => Ok(Json(meta_response(&meta, false))),
        Err(err) => Err(map_document_error(err)),
    }
}

async fn trash_document(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, document_id)): Path<(Uuid, Uuid)>,
    Query(query): Query<TrashQuery>,
) -> Result<Json<OkResponse>, DocumentApiError> {
    check_origin(&headers, &state.public_origin)?;
    let children = parse_trash_children(query.children.as_deref())?;
    let (_user, user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::DocumentsWrite),
        Some(workspace_id),
    )
    .await?;
    let ip = peer_ip(peer.ip());
    let result = trash_wiki_document(
        &state.auth.db.pool,
        workspace_id,
        user_id,
        session_id,
        document_id,
        children,
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(()) => Ok(Json(OkResponse { ok: true })),
        Err(err) => Err(map_document_error(err)),
    }
}

async fn restore_document(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, document_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<OkResponse>, DocumentApiError> {
    check_origin(&headers, &state.public_origin)?;
    let (_user, user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::DocumentsWrite),
        Some(workspace_id),
    )
    .await?;
    let ip = peer_ip(peer.ip());
    let result = restore_wiki_document(
        &state.auth.db.pool,
        workspace_id,
        user_id,
        session_id,
        document_id,
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(()) => Ok(Json(OkResponse { ok: true })),
        Err(err) => Err(map_document_error(err)),
    }
}

async fn list_trash(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
) -> Result<Json<TrashListResponse>, DocumentApiError> {
    let (_user, user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::DocumentsRead),
        Some(workspace_id),
    )
    .await?;
    let result =
        list_trashed_wiki_documents(&state.auth.db.pool, workspace_id, user_id, session_id)
            .await
            .map_err(internal)?;
    match result {
        Ok(items) => Ok(Json(TrashListResponse {
            items: items
                .into_iter()
                .map(|item| TrashItemResponse {
                    id: item.id.to_string(),
                    title: item.title,
                    deleted_at: item.deleted_at,
                    project_id: item.project_id.map(|id| id.to_string()),
                })
                .collect(),
        })),
        Err(err) => Err(map_document_error(err)),
    }
}

async fn export_markdown(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, document_id)): Path<(Uuid, Uuid)>,
) -> Result<Response, DocumentApiError> {
    export_document(
        &state,
        peer,
        &headers,
        &jar,
        workspace_id,
        document_id,
        ExportFormat::Markdown,
    )
    .await
}

async fn export_pdf(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, document_id)): Path<(Uuid, Uuid)>,
) -> Result<Response, DocumentApiError> {
    export_document(
        &state,
        peer,
        &headers,
        &jar,
        workspace_id,
        document_id,
        ExportFormat::Pdf,
    )
    .await
}

async fn export_docx(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, document_id)): Path<(Uuid, Uuid)>,
) -> Result<Response, DocumentApiError> {
    export_document(
        &state,
        peer,
        &headers,
        &jar,
        workspace_id,
        document_id,
        ExportFormat::Docx,
    )
    .await
}

async fn export_pptx(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, document_id)): Path<(Uuid, Uuid)>,
) -> Result<Response, DocumentApiError> {
    export_document(
        &state,
        peer,
        &headers,
        &jar,
        workspace_id,
        document_id,
        ExportFormat::Pptx,
    )
    .await
}

async fn export_document(
    state: &AppState,
    peer: SocketAddr,
    headers: &HeaderMap,
    jar: &CookieJar,
    workspace_id: Uuid,
    document_id: Uuid,
    format: ExportFormat,
) -> Result<Response, DocumentApiError> {
    let (_user, user_id, session_id) = require_session(
        state,
        headers,
        jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::DocumentsRead),
        Some(workspace_id),
    )
    .await?;
    let _ = peer_ip(peer.ip());
    if let Err(retry_after) = state
        .rate_limiter
        .allow_window(
            &format!("doc-export-user:{user_id}"),
            10,
            std::time::Duration::from_secs(60),
        )
        .await
    {
        return Err(AppError::rate_limited(retry_after).into());
    }
    let Some(convert) = state.document_convert.as_ref() else {
        return Err(AppError::internal().into());
    };
    let result = get_wiki_document(
        &state.auth.db.pool,
        workspace_id,
        user_id,
        session_id,
        document_id,
    )
    .await
    .map_err(internal)?;
    let meta = match result {
        Ok(meta) => meta,
        Err(err) => return Err(map_document_error(err)),
    };
    let rendered = match render_document_export(convert, format, &meta.title, &meta.content_json) {
        Ok(v) => v,
        Err(ExportRenderError::InvalidInput) => {
            return Err(AppError::from_code(ProblemCode::InvalidInput).into());
        }
        Err(ExportRenderError::TooLarge) => {
            return Err(DocumentApiError::Coded {
                status: StatusCode::PAYLOAD_TOO_LARGE,
                code: "document_body_exceeds_document_max_body_bytes",
                title: "document body exceeds document max body bytes".to_string(),
                params: None,
            });
        }
        Err(ExportRenderError::Unavailable | ExportRenderError::Failed) => {
            return Err(AppError::internal().into());
        }
    };
    let filename = export_filename(&meta.title, &rendered.ext);
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_str(&rendered.content_type).map_err(|_| AppError::internal())?,
    );
    headers.insert(
        axum::http::header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
            .map_err(|_| AppError::internal())?,
    );
    headers.insert(
        axum::http::header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    Ok((StatusCode::OK, headers, rendered.bytes).into_response())
}

async fn get_body(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, document_id)): Path<(Uuid, Uuid)>,
    Query(query): Query<BodyQuery>,
) -> Result<Json<BodyResponse>, DocumentApiError> {
    if query.format.is_some() {
        return Err(AppError::from_code(ProblemCode::InvalidInput).into());
    }
    let (_user, user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::DocumentsRead),
        Some(workspace_id),
    )
    .await?;
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

pub(crate) fn meta_response(meta: &DocumentMeta, include_display_id: bool) -> DocumentMetaResponse {
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

fn parse_trash_children(value: Option<&str>) -> Result<TrashChildrenMode, DocumentApiError> {
    match value {
        None => Ok(TrashChildrenMode::Trash),
        Some("trash") => Ok(TrashChildrenMode::Trash),
        Some("reparent") => Ok(TrashChildrenMode::Reparent),
        Some(_) => Err(AppError::from_code(ProblemCode::InvalidInput).into()),
    }
}

pub(crate) fn map_document_error(err: DocumentDbError) -> DocumentApiError {
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
        DocumentDbError::Cycle => DocumentApiError::Coded {
            status: StatusCode::BAD_REQUEST,
            code: "document_cycle",
            title:
                "move would create a cycle (new parent is the document itself or its descendant)"
                    .to_string(),
            params: None,
        },
        DocumentDbError::TrashedParent => DocumentApiError::Coded {
            status: StatusCode::CONFLICT,
            code: "restore_rejected",
            title: "restore rejected".to_string(),
            params: None,
        },
        DocumentDbError::RootDocumentTrash => DocumentApiError::Coded {
            status: StatusCode::BAD_REQUEST,
            code: "root_document_trash",
            title: "cannot trash a project's root document (delete the project instead)"
                .to_string(),
            params: None,
        },
        DocumentDbError::RootDocumentMove => DocumentApiError::Coded {
            status: StatusCode::BAD_REQUEST,
            code: "root_document_move",
            title: "cannot move a project's root document (not supported in v1)".to_string(),
            params: None,
        },
        DocumentDbError::InvalidSortKey => AppError::internal().into(),
    }
}

async fn require_session(
    state: &AppState,
    headers: &HeaderMap,
    jar: &CookieJar,
    access: crate::http::authz::Access,
    workspace_id: Option<Uuid>,
) -> Result<(SessionUser, Uuid, Uuid), AppError> {
    let auth =
        crate::http::authz::require_request_auth(state, headers, jar, access, workspace_id).await?;
    Ok((auth.user, auth.user_id, auth.credential_id))
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
    fn patch_title_and_status_are_optional_but_not_nullable() {
        let omitted: PatchDocumentBody = serde_json::from_str("{}").unwrap();
        assert!(omitted.title.is_none() && omitted.status.is_none());
        for field in ["title", "status"] {
            let body = serde_json::json!({field: null});
            assert!(serde_json::from_value::<PatchDocumentBody>(body).is_err());
        }
    }
}
