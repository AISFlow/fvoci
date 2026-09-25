use std::net::SocketAddr;

use axum::extract::rejection::JsonRejection;
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use uuid::Uuid;

use crate::api::dto::{
    CreateDocumentBody, DocumentMetaResponse, MoveDocumentBody, PatchDocumentBody,
    RequiredNullable, TreeNodeResponse, TreeResponse,
};
use crate::auth::session::SessionUser;
use crate::db::documents::{CreateDocumentInput, UpdateDocumentMetaInput};
use crate::db::project_documents::{
    create_project_document, get_project_document, list_project_document_tree,
    move_project_document, update_project_document_meta,
};
use crate::documents::export::ExportFormat;
use crate::error::{AppError, ProblemCode};
use crate::http::guard::check_origin;
use crate::http::rate_limit::peer_ip;
use crate::http::routes::documents::{
    export_document, map_document_error, meta_response, DocumentApiError,
};
use crate::http::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents",
            get(list_tree).post(create_document),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}",
            get(get_document).patch(patch_document),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/move",
            post(move_document),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/md",
            get(export_markdown),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/pdf",
            get(export_pdf),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/docx",
            get(export_docx),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/pptx",
            get(export_pptx),
        )
}

macro_rules! project_export_handler {
    ($name:ident, $format:expr) => {
        async fn $name(
            State(state): State<AppState>,
            headers: HeaderMap,
            jar: CookieJar,
            Path((workspace_id, project_id, document_id)): Path<(Uuid, Uuid, Uuid)>,
        ) -> Result<Response, DocumentApiError> {
            export_document(
                &state,
                &headers,
                &jar,
                workspace_id,
                Some(project_id),
                document_id,
                $format,
            )
            .await
        }
    };
}

project_export_handler!(export_markdown, ExportFormat::Markdown);
project_export_handler!(export_pdf, ExportFormat::Pdf);
project_export_handler!(export_docx, ExportFormat::Docx);
project_export_handler!(export_pptx, ExportFormat::Pptx);

async fn list_tree(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<TreeResponse>, DocumentApiError> {
    let (_user, user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::DocumentsRead),
        Some(workspace_id),
    )
    .await?;
    let result = list_project_document_tree(
        &state.auth.db.pool,
        workspace_id,
        project_id,
        user_id,
        session_id,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(nodes) => Ok(Json(TreeResponse {
            items: nodes
                .into_iter()
                .map(|node| TreeNodeResponse {
                    id: node.id.to_string(),
                    workspace_id: node.workspace_id.to_string(),
                    parent_id: node.parent_id.map(|id| id.to_string()),
                    project_id: node.project_id.map(|id| id.to_string()),
                    title: node.title,
                    icon: node.icon,
                    path: node.path,
                    sort_key: node.sort_key,
                    number: node.number,
                    status: node.status,
                })
                .collect(),
        })),
        Err(err) => Err(map_document_error(err)),
    }
}

async fn create_document(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id)): Path<(Uuid, Uuid)>,
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
    if parent_id.is_none() {
        return Err(AppError::from_code(ProblemCode::InvalidInput).into());
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
    let result = create_project_document(
        &state.auth.db.pool,
        workspace_id,
        project_id,
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
    Path((workspace_id, project_id, document_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<Json<DocumentMetaResponse>, DocumentApiError> {
    let (_user, user_id, session_id) = require_session(
        &state,
        &headers,
        &jar,
        crate::http::authz::Access::Scope(crate::auth::scopes::ApiTokenScope::DocumentsRead),
        Some(workspace_id),
    )
    .await?;
    let result = get_project_document(
        &state.auth.db.pool,
        workspace_id,
        project_id,
        document_id,
        user_id,
        session_id,
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(meta) => Ok(Json(meta_response(&meta, true))),
        Err(err) => Err(map_document_error(err)),
    }
}

async fn patch_document(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id, document_id)): Path<(Uuid, Uuid, Uuid)>,
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
    let result = update_project_document_meta(
        &state.auth.db.pool,
        workspace_id,
        project_id,
        document_id,
        user_id,
        session_id,
        UpdateDocumentMetaInput {
            title: body.title.as_deref().map(str::trim),
            icon: body.icon.as_ref().map(|value| value.as_deref()),
            status: body.status.as_deref(),
        },
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(meta) => Ok(Json(meta_response(&meta, true))),
        Err(err) => Err(map_document_error(err)),
    }
}

async fn move_document(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id, document_id)): Path<(Uuid, Uuid, Uuid)>,
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
    let result = move_project_document(
        &state.auth.db.pool,
        workspace_id,
        project_id,
        document_id,
        user_id,
        session_id,
        body.new_parent_id,
        Some(&ip),
    )
    .await
    .map_err(internal)?;
    match result {
        Ok(meta) => Ok(Json(meta_response(&meta, true))),
        Err(err) => Err(map_document_error(err)),
    }
}

async fn require_session(
    state: &AppState,
    headers: &HeaderMap,
    jar: &CookieJar,
    access: crate::http::authz::Access,
    workspace_id: Option<Uuid>,
) -> Result<(SessionUser, Uuid, Uuid), DocumentApiError> {
    let auth =
        crate::http::authz::require_request_auth(state, headers, jar, access, workspace_id).await?;
    Ok((auth.user, auth.user_id, auth.credential_id))
}

fn internal(err: sqlx::Error) -> DocumentApiError {
    tracing::error!("database error: {}", err);
    AppError::internal().into()
}
