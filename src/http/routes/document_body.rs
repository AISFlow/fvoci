//! Document body, block, children, backlinks, duplicate and project ancestors
//! for wiki and project documents (source `apps/server/src/domains/documents/routes.ts`).
//!
//! External body writes (PUT body, PATCH block) never touch the stored body
//! directly: the new content is seeded into a standalone Yjs Doc by the editor
//! helper and the room actor turns it into one forward update of the live
//! fragment (source `replaceLiveCollabContent`), appended durably before it is
//! broadcast and projected into the derived body like any collaborator edit.

#![allow(clippy::too_many_arguments)]

use std::net::SocketAddr;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::rejection::JsonRejection;
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use crate::api::documents_dto::{
    BacklinkFromResponse, BacklinkItemResponse, BacklinkListResponse, BodyMdResponse,
    DocumentBodyResponse, DuplicateDocumentInput, PatchBlockInput, SaveVersionedBodyInput,
    SaveVersionedBodyResponse, VersionedBodyResponse,
};
use crate::api::dto::{AncestorResponse, AncestorsResponse, BodyResponse, DocumentMetaResponse};
use crate::api::dto::{TreeNodeResponse, TreeResponse};
use crate::auth::scopes::ApiTokenScope;
use crate::collab::derived_body::{
    prepare_derived_body, DerivedBodyError, DOCUMENT_MAX_BODY_BYTES,
};
use crate::collab::room::BodyWriteError;
use crate::collab::seed::{SeedEngine, SeedError};
use crate::db::document_ops::{
    authorize_document, authorize_document_backend, commit_duplicate, list_document_backlinks,
    list_project_ancestors, load_duplicate_sources, DocumentScope, DuplicateBody, SourceBody,
};
use crate::db::documents::{list_wiki_tree, DocumentDbError, TreeNode};
use crate::db::project_documents::list_project_document_tree;
use crate::documents::blocks::{replace_node_by_id, BlockNode};
use crate::documents::markdown_helper::{MarkdownError, MarkdownHelper};
use crate::error::{AppError, ProblemCode};
use crate::http::authz::{require_request_auth, Access, RequestAuth};
use crate::http::guard::check_origin;
use crate::http::rate_limit::{peer_ip, REVISION_WRITE_LIMIT, REVISION_WRITE_WINDOW};
use crate::http::routes::documents::{map_document_error, meta_response, DocumentApiError};
use crate::http::state::AppState;
use crate::projects::ProjectPermission;

/// Attempts of the block read-modify-write when a concurrent edit moves the tail.
const PATCH_BLOCK_ATTEMPTS: usize = 3;

const WS_DOC: &str = "/api/v1/workspaces/{workspace_id}/documents/{document_id}";
const PROJECT_DOC: &str =
    "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}";

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/workspaces/{workspace_id}/documents/from-draft",
            post(create_wiki_from_draft).layer(axum::extract::DefaultBodyLimit::max(
                DOCUMENT_MAX_BODY_BYTES + 4096,
            )),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/from-draft",
            post(create_project_from_draft).layer(axum::extract::DefaultBodyLimit::max(
                DOCUMENT_MAX_BODY_BYTES + 4096,
            )),
        )
        .route(
            &format!("{WS_DOC}/body/versioned"),
            get(get_versioned_body).put(save_versioned_body).layer(
                axum::extract::DefaultBodyLimit::max(
                    (crate::db::collab::MAX_COLLAB_UPDATE_BYTES * 4).div_ceil(3) + 4096,
                ),
            ),
        )
        .route(
            &format!("{PROJECT_DOC}/body/versioned"),
            get(get_project_versioned_body)
                .put(save_project_versioned_body)
                .layer(axum::extract::DefaultBodyLimit::max(
                    (crate::db::collab::MAX_COLLAB_UPDATE_BYTES * 4).div_ceil(3) + 4096,
                )),
        )
        .route(&format!("{WS_DOC}/body"), axum::routing::put(put_body_wiki))
        .route(
            &format!("{WS_DOC}/blocks/{{block_id}}"),
            patch(patch_block_wiki),
        )
        .route(&format!("{WS_DOC}/children"), get(children_wiki))
        .route(&format!("{WS_DOC}/backlinks"), get(backlinks_wiki))
        .route(&format!("{WS_DOC}/duplicate"), post(duplicate_wiki))
        .route(
            &format!("{PROJECT_DOC}/body"),
            axum::routing::put(put_body_project),
        )
        .route(
            &format!("{PROJECT_DOC}/blocks/{{block_id}}"),
            patch(patch_block_project),
        )
        .route(&format!("{PROJECT_DOC}/children"), get(children_project))
        .route(&format!("{PROJECT_DOC}/backlinks"), get(backlinks_project))
        .route(&format!("{PROJECT_DOC}/duplicate"), post(duplicate_project))
        .route(&format!("{PROJECT_DOC}/ancestors"), get(ancestors_project))
        .route(
            "/api/v1/documents/{document_id}",
            get(get_by_id).patch(update_by_id).delete(remove_by_id),
        )
        .route(
            "/api/v1/documents/{document_id}/duplicate",
            post(duplicate_by_id),
        )
}

#[derive(Deserialize)]
pub(crate) struct BodyQuery {
    pub(crate) format: Option<String>,
}

type WikiPath = Path<(Uuid, Uuid)>;
type ProjectPath = Path<(Uuid, Uuid, Uuid)>;

fn coded(status: StatusCode, code: &'static str, title: &str) -> DocumentApiError {
    DocumentApiError::Coded {
        status,
        code,
        title: title.to_string(),
        params: None,
    }
}

fn too_large() -> DocumentApiError {
    coded(
        StatusCode::PAYLOAD_TOO_LARGE,
        "document_body_exceeds_document_max_body_bytes",
        "document body exceeds document max body bytes",
    )
}

fn invalid_body() -> DocumentApiError {
    coded(
        StatusCode::BAD_REQUEST,
        "invalid_document_body",
        "invalid document body",
    )
}

fn collab_unavailable() -> DocumentApiError {
    coded(
        StatusCode::SERVICE_UNAVAILABLE,
        "collab_unavailable",
        "collab unavailable",
    )
}

fn internal(err: sqlx::Error) -> AppError {
    tracing::error!("database error: {}", err);
    AppError::internal()
}

fn db_result<T>(
    result: Result<Result<T, DocumentDbError>, sqlx::Error>,
) -> Result<T, DocumentApiError> {
    result.map_err(internal)?.map_err(map_document_error)
}

async fn auth(
    state: &AppState,
    headers: &HeaderMap,
    jar: &CookieJar,
    scope: ApiTokenScope,
    workspace_id: Uuid,
) -> Result<RequestAuth, AppError> {
    require_request_auth(
        state,
        headers,
        jar,
        Access::Scope(scope),
        Some(workspace_id),
    )
    .await
}

/// Source `onRevisionWriteLimit`: body and block writes share the revision-write budget.
async fn revision_write_limit(state: &AppState, user_id: Uuid) -> Result<(), AppError> {
    state
        .rate_limiter
        .allow_window(
            &format!("revision-write:{user_id}"),
            REVISION_WRITE_LIMIT,
            REVISION_WRITE_WINDOW,
        )
        .await
        .map_err(AppError::rate_limited)
}

fn markdown_helper(state: &AppState) -> Result<&MarkdownHelper, DocumentApiError> {
    state.markdown.as_ref().ok_or_else(|| {
        tracing::error!("document body markdown requested but the markdown helper is unavailable");
        AppError::internal().into()
    })
}

fn map_markdown(err: MarkdownError) -> DocumentApiError {
    match err {
        MarkdownError::InvalidInput(detail) => {
            tracing::info!(%detail, "document body markdown refused");
            invalid_body()
        }
        MarkdownError::TooLarge => too_large(),
        MarkdownError::Failed(detail) => {
            tracing::error!(error = %detail, "markdown helper failed");
            AppError::internal().into()
        }
    }
}

fn map_seed(err: SeedError) -> DocumentApiError {
    match err {
        SeedError::InvalidInput(detail) => {
            tracing::info!(%detail, "document body seed refused");
            invalid_body()
        }
        SeedError::TooLarge(_) => too_large(),
        SeedError::Unavailable => collab_unavailable(),
        SeedError::Failed(detail) => {
            tracing::error!(error = %detail, "document body seed failed");
            AppError::internal().into()
        }
    }
}

fn map_derived(err: DerivedBodyError) -> DocumentApiError {
    match err {
        DerivedBodyError::TooLarge => too_large(),
        DerivedBodyError::InvalidDocumentBody(_) => invalid_body(),
    }
}

fn map_body_write(err: BodyWriteError) -> DocumentApiError {
    match err {
        BodyWriteError::Rejected => AppError::from_code(ProblemCode::NotFound).into(),
        BodyWriteError::Unavailable => collab_unavailable(),
        BodyWriteError::Conflict => coded(
            StatusCode::CONFLICT,
            "document_version_mismatch",
            "document version mismatch",
        ),
        BodyWriteError::TooLarge => too_large(),
        BodyWriteError::Invalid => invalid_body(),
        BodyWriteError::DeriveFailed => {
            tracing::error!("document body write committed but the derived body failed");
            AppError::internal().into()
        }
    }
}

// ---------------------------------------------------------------------------
// GET body

pub(crate) async fn read_body(
    state: &AppState,
    headers: &HeaderMap,
    jar: &CookieJar,
    workspace_id: Uuid,
    scope: DocumentScope,
    document_id: Uuid,
    format: Option<&str>,
) -> Result<Json<DocumentBodyResponse>, DocumentApiError> {
    let markdown = match format {
        None => false,
        Some("md") => true,
        Some(_) => return Err(AppError::from_code(ProblemCode::InvalidInput).into()),
    };
    let auth = auth(
        state,
        headers,
        jar,
        ApiTokenScope::DocumentsRead,
        workspace_id,
    )
    .await?;
    let meta = match scope {
        DocumentScope::Wiki => db_result(
            crate::db::documents::read_wiki_document_body_backend(
                &state.auth.db.pool,
                workspace_id,
                auth.user_id,
                auth.credential_id,
                document_id,
            )
            .await,
        )?,
        DocumentScope::Project(_) => db_result(
            authorize_document_backend(
                &state.auth.db.pool,
                workspace_id,
                auth.user_id,
                auth.credential_id,
                scope,
                document_id,
                ProjectPermission::View,
            )
            .await,
        )?,
    };
    if !markdown {
        return Ok(Json(DocumentBodyResponse::Json(BodyResponse {
            content_json: meta.content_json,
            version: meta.version,
        })));
    }
    let content_md = markdown_helper(state)?
        .tiptap_to_md(&meta.content_json)
        .await
        .map_err(map_markdown)?;
    Ok(Json(DocumentBodyResponse::Markdown(BodyMdResponse {
        content_md,
        version: meta.version,
    })))
}

// ---------------------------------------------------------------------------
// PUT body

/// Source `documentBodyPutInput`: exactly one of `contentJson` | `contentMd`.
enum BodyInput {
    Json(Value),
    Markdown(String),
}

fn parse_body_input(bytes: &[u8]) -> Result<BodyInput, DocumentApiError> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|_| AppError::from_code(ProblemCode::InvalidInput))?;
    let Value::Object(mut obj) = value else {
        return Err(AppError::from_code(ProblemCode::InvalidInput).into());
    };
    if obj.len() != 1 {
        return Err(AppError::from_code(ProblemCode::InvalidInput).into());
    }
    if let Some(json) = obj.remove("contentJson") {
        return Ok(BodyInput::Json(json));
    }
    match obj.remove("contentMd") {
        Some(Value::String(md)) => Ok(BodyInput::Markdown(md)),
        _ => Err(AppError::from_code(ProblemCode::InvalidInput).into()),
    }
}

/// Source `prepareBodyUpdate`: byte cap first, then Markdown conversion or
/// Tiptap validation.
async fn resolve_body_json(state: &AppState, input: BodyInput) -> Result<Value, DocumentApiError> {
    match input {
        BodyInput::Markdown(md) => {
            if md.len() > DOCUMENT_MAX_BODY_BYTES {
                return Err(too_large());
            }
            let json = markdown_helper(state)?
                .md_to_tiptap(&md)
                .await
                .map_err(map_markdown)?;
            prepare_derived_body(json.clone()).map_err(map_derived)?;
            Ok(json)
        }
        BodyInput::Json(json) => {
            prepare_derived_body(json.clone()).map_err(map_derived)?;
            Ok(json)
        }
    }
}

async fn seed_for(state: &AppState, content_json: &Value) -> Result<Vec<u8>, DocumentApiError> {
    let Some(hub) = state.collab.as_ref() else {
        return Err(collab_unavailable());
    };
    SeedEngine::from_hub(hub)
        .tiptap_to_yjs_update(content_json)
        .await
        .map_err(map_seed)
}

async fn replace_live_body(
    state: &AppState,
    workspace_id: Uuid,
    document_id: Uuid,
    auth: &RequestAuth,
    seed: Vec<u8>,
    expected_tail_seq: Option<i64>,
) -> Result<(), DocumentApiError> {
    let Some(hub) = state.collab.clone() else {
        return Err(collab_unavailable());
    };
    let timeout = hub.rpc_timeout().max(Duration::from_millis(1));
    let write = hub.replace_body(
        (workspace_id, document_id),
        auth.user_id,
        auth.credential_id,
        seed,
        expected_tail_seq,
    );
    match tokio::time::timeout(timeout, write).await {
        Ok(result) => result.map_err(map_body_write),
        Err(_) => Err(AppError::from_code(ProblemCode::CollabTimeoutRetry).into()),
    }
}

async fn put_body(
    state: &AppState,
    headers: &HeaderMap,
    jar: &CookieJar,
    workspace_id: Uuid,
    scope: DocumentScope,
    document_id: Uuid,
    bytes: &[u8],
) -> Result<Json<DocumentMetaResponse>, DocumentApiError> {
    require_realtime_writer(state)?;
    check_origin(headers, &state.public_origin)?;
    let input = parse_body_input(bytes)?;
    let auth = auth(
        state,
        headers,
        jar,
        ApiTokenScope::DocumentsWrite,
        workspace_id,
    )
    .await?;
    revision_write_limit(state, auth.user_id).await?;
    db_result(
        authorize_document_backend(
            &state.auth.db.pool,
            workspace_id,
            auth.user_id,
            auth.credential_id,
            scope,
            document_id,
            ProjectPermission::Edit,
        )
        .await,
    )?;
    let content_json = resolve_body_json(state, input).await?;
    let seed = seed_for(state, &content_json).await?;
    replace_live_body(state, workspace_id, document_id, &auth, seed, None).await?;
    let meta = db_result(
        authorize_document_backend(
            &state.auth.db.pool,
            workspace_id,
            auth.user_id,
            auth.credential_id,
            scope,
            document_id,
            ProjectPermission::View,
        )
        .await,
    )?;
    Ok(Json(meta_response(&meta, false)))
}

async fn put_body_wiki(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, document_id)): WikiPath,
    body: Bytes,
) -> Result<Json<DocumentMetaResponse>, DocumentApiError> {
    put_body(
        &state,
        &headers,
        &jar,
        workspace_id,
        DocumentScope::Wiki,
        document_id,
        &body,
    )
    .await
}

async fn put_body_project(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id, document_id)): ProjectPath,
    body: Bytes,
) -> Result<Json<DocumentMetaResponse>, DocumentApiError> {
    put_body(
        &state,
        &headers,
        &jar,
        workspace_id,
        DocumentScope::Project(project_id),
        document_id,
        &body,
    )
    .await
}

// ---------------------------------------------------------------------------
// PATCH blocks/{blockId}

fn parse_block_input(bytes: &[u8], block_id: &str) -> Result<BlockNode, DocumentApiError> {
    let input: PatchBlockInput = serde_json::from_slice(bytes)
        .map_err(|_| AppError::from_code(ProblemCode::InvalidInput))?;
    if input.r#type.is_empty() {
        return Err(AppError::from_code(ProblemCode::InvalidInput).into());
    }
    // Source `patchCollabBlock`: an explicit `attrs.id` must name the patched block.
    if let Some(id) = input.attrs.as_ref().and_then(|attrs| attrs.get("id")) {
        if id.as_str() != Some(block_id) {
            return Err(invalid_body());
        }
    }
    Ok(BlockNode {
        r#type: input.r#type,
        attrs: input.attrs,
        content: input.content,
        marks: input.marks,
        text: input.text,
    })
}

async fn patch_block(
    state: &AppState,
    headers: &HeaderMap,
    jar: &CookieJar,
    workspace_id: Uuid,
    scope: DocumentScope,
    document_id: Uuid,
    block_id: &str,
    bytes: &[u8],
) -> Result<Json<DocumentMetaResponse>, DocumentApiError> {
    require_realtime_writer(state)?;
    check_origin(headers, &state.public_origin)?;
    let node = parse_block_input(bytes, block_id)?;
    let auth = auth(
        state,
        headers,
        jar,
        ApiTokenScope::DocumentsWrite,
        workspace_id,
    )
    .await?;
    revision_write_limit(state, auth.user_id).await?;
    db_result(
        authorize_document(
            state
                .auth
                .db
                .pool
                .postgres("src/http/routes/document_body.rs")
                .map_err(internal)?,
            workspace_id,
            auth.user_id,
            auth.credential_id,
            scope,
            document_id,
            ProjectPermission::Edit,
        )
        .await,
    )?;
    let Some(hub) = state.collab.clone() else {
        return Err(collab_unavailable());
    };
    let timeout = hub.rpc_timeout().max(Duration::from_millis(1));
    let mut attempt = 0;
    loop {
        attempt += 1;
        // Source `withLiveDoc`: read the live projection, patch it, replace it.
        // The replace carries the projected tail so a concurrent edit is never
        // overwritten; a moved tail re-reads the live document.
        let live = match tokio::time::timeout(
            timeout,
            hub.project_live(
                (workspace_id, document_id),
                auth.user_id,
                auth.credential_id,
            ),
        )
        .await
        {
            Ok(result) => result.map_err(map_body_write)?,
            Err(_) => return Err(AppError::from_code(ProblemCode::CollabTimeoutRetry).into()),
        };
        let Some(patched) = replace_node_by_id(&live.content_json, block_id, &node) else {
            return Err(AppError::from_code(ProblemCode::NotFound).into());
        };
        prepare_derived_body(patched.clone()).map_err(map_derived)?;
        let seed = seed_for(state, &patched).await?;
        match replace_live_body(
            state,
            workspace_id,
            document_id,
            &auth,
            seed,
            Some(live.tail_seq),
        )
        .await
        {
            Ok(()) => break,
            Err(DocumentApiError::Coded {
                status: StatusCode::CONFLICT,
                ..
            }) if attempt < PATCH_BLOCK_ATTEMPTS => continue,
            Err(err) => return Err(err),
        }
    }
    let meta = db_result(
        authorize_document(
            state
                .auth
                .db
                .pool
                .postgres("src/http/routes/document_body.rs")
                .map_err(internal)?,
            workspace_id,
            auth.user_id,
            auth.credential_id,
            scope,
            document_id,
            ProjectPermission::View,
        )
        .await,
    )?;
    Ok(Json(meta_response(&meta, false)))
}

async fn patch_block_wiki(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, document_id, block_id)): Path<(Uuid, Uuid, String)>,
    body: Bytes,
) -> Result<Json<DocumentMetaResponse>, DocumentApiError> {
    patch_block(
        &state,
        &headers,
        &jar,
        workspace_id,
        DocumentScope::Wiki,
        document_id,
        &block_id,
        &body,
    )
    .await
}

async fn patch_block_project(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id, document_id, block_id)): Path<(Uuid, Uuid, Uuid, String)>,
    body: Bytes,
) -> Result<Json<DocumentMetaResponse>, DocumentApiError> {
    patch_block(
        &state,
        &headers,
        &jar,
        workspace_id,
        DocumentScope::Project(project_id),
        document_id,
        &block_id,
        &body,
    )
    .await
}

// ---------------------------------------------------------------------------
// children / backlinks / ancestors

fn tree_response(nodes: Vec<TreeNode>, parent: Uuid) -> TreeResponse {
    TreeResponse {
        items: nodes
            .into_iter()
            .filter(|n| n.parent_id == Some(parent))
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
    }
}

/// Source `children`: the document must be readable, then its visible
/// children in tree order.
async fn children(
    state: &AppState,
    headers: &HeaderMap,
    jar: &CookieJar,
    workspace_id: Uuid,
    scope: DocumentScope,
    document_id: Uuid,
) -> Result<Json<TreeResponse>, DocumentApiError> {
    let auth = auth(
        state,
        headers,
        jar,
        ApiTokenScope::DocumentsRead,
        workspace_id,
    )
    .await?;
    let pool = state
        .auth
        .db
        .pool
        .postgres("src/http/routes/document_body.rs")
        .map_err(internal)?;
    db_result(
        authorize_document(
            pool,
            workspace_id,
            auth.user_id,
            auth.credential_id,
            scope,
            document_id,
            ProjectPermission::View,
        )
        .await,
    )?;
    let nodes = match scope {
        DocumentScope::Wiki => {
            db_result(list_wiki_tree(pool, workspace_id, auth.user_id, auth.credential_id).await)?
        }
        DocumentScope::Project(project_id) => db_result(
            list_project_document_tree(
                pool,
                workspace_id,
                project_id,
                auth.user_id,
                auth.credential_id,
            )
            .await,
        )?,
    };
    Ok(Json(tree_response(nodes, document_id)))
}

async fn children_wiki(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, document_id)): WikiPath,
) -> Result<Json<TreeResponse>, DocumentApiError> {
    children(
        &state,
        &headers,
        &jar,
        workspace_id,
        DocumentScope::Wiki,
        document_id,
    )
    .await
}

async fn children_project(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id, document_id)): ProjectPath,
) -> Result<Json<TreeResponse>, DocumentApiError> {
    children(
        &state,
        &headers,
        &jar,
        workspace_id,
        DocumentScope::Project(project_id),
        document_id,
    )
    .await
}

async fn backlinks(
    state: &AppState,
    headers: &HeaderMap,
    jar: &CookieJar,
    workspace_id: Uuid,
    scope: DocumentScope,
    document_id: Uuid,
) -> Result<Json<BacklinkListResponse>, DocumentApiError> {
    let auth = auth(
        state,
        headers,
        jar,
        ApiTokenScope::DocumentsRead,
        workspace_id,
    )
    .await?;
    let items = db_result(
        list_document_backlinks(
            state
                .auth
                .db
                .pool
                .postgres("src/http/routes/document_body.rs")
                .map_err(internal)?,
            workspace_id,
            auth.user_id,
            auth.credential_id,
            scope,
            document_id,
        )
        .await,
    )?;
    Ok(Json(BacklinkListResponse {
        items: items
            .into_iter()
            .map(|b| BacklinkItemResponse {
                id: b.id.to_string(),
                from: BacklinkFromResponse {
                    r#type: b.kind.as_str().to_string(),
                    id: b.id.to_string(),
                    title: b.title,
                    display_id: b.display_id,
                },
            })
            .collect(),
    }))
}

async fn backlinks_wiki(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, document_id)): WikiPath,
) -> Result<Json<BacklinkListResponse>, DocumentApiError> {
    backlinks(
        &state,
        &headers,
        &jar,
        workspace_id,
        DocumentScope::Wiki,
        document_id,
    )
    .await
}

async fn backlinks_project(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id, document_id)): ProjectPath,
) -> Result<Json<BacklinkListResponse>, DocumentApiError> {
    backlinks(
        &state,
        &headers,
        &jar,
        workspace_id,
        DocumentScope::Project(project_id),
        document_id,
    )
    .await
}

async fn ancestors_project(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id, document_id)): ProjectPath,
) -> Result<Json<AncestorsResponse>, DocumentApiError> {
    let auth = auth(
        &state,
        &headers,
        &jar,
        ApiTokenScope::DocumentsRead,
        workspace_id,
    )
    .await?;
    let items = db_result(
        list_project_ancestors(
            state
                .auth
                .db
                .pool
                .postgres("src/http/routes/document_body.rs")
                .map_err(internal)?,
            workspace_id,
            project_id,
            auth.user_id,
            auth.credential_id,
            document_id,
        )
        .await,
    )?;
    Ok(Json(AncestorsResponse {
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
    }))
}

// ---------------------------------------------------------------------------
// duplicate

fn parse_duplicate_input(bytes: &[u8]) -> Result<(Option<String>, bool), DocumentApiError> {
    let input: DuplicateDocumentInput = if bytes.is_empty() {
        DuplicateDocumentInput::default()
    } else {
        serde_json::from_slice(bytes).map_err(|_| AppError::from_code(ProblemCode::InvalidInput))?
    };
    let title = match input.title {
        None => None,
        Some(title) => {
            let trimmed = title.trim();
            if !crate::db::documents::title_is_valid(trimmed) {
                return Err(AppError::from_code(ProblemCode::InvalidInput).into());
            }
            Some(trimmed.to_string())
        }
    };
    Ok((title, input.include_children == Some(true)))
}

/// Projected JSON of each source and its independent seed (source `copyInto`).
async fn prepare_duplicate_bodies(
    state: &AppState,
    sources: &[crate::db::document_ops::DuplicateSource],
) -> Result<Vec<DuplicateBody>, DocumentApiError> {
    let engine = match state.collab.as_ref() {
        Some(hub) => Some((hub.engine_bin(), hub.limits())),
        None => crate::collab::CollabConfig::from_env().map(|cfg| (cfg.engine_bin, cfg.limits)),
    };
    let Some(seed_engine) = engine
        .clone()
        .map(|(bin, limits)| SeedEngine::new(bin, limits))
    else {
        return Err(collab_unavailable());
    };
    let mut bodies = Vec::with_capacity(sources.len());
    for source in sources {
        let content_json = match &source.body {
            SourceBody::Json(json) => json.clone(),
            SourceBody::Collab { snapshot, tail } => {
                let Some((engine_bin, limits)) = engine.clone() else {
                    return Err(collab_unavailable());
                };
                let (snapshot, tail) = (snapshot.clone(), tail.clone());
                tokio::task::spawn_blocking(move || {
                    crate::collab::revision::project_persisted_offline(
                        engine_bin, limits, snapshot, tail,
                    )
                })
                .await
                .map_err(|_| collab_unavailable())?
                .map_err(|_| collab_unavailable())?
            }
        };
        let prepared = prepare_derived_body(content_json).map_err(map_derived)?;
        let seed = seed_engine
            .tiptap_to_yjs_update(prepared.content_json())
            .await
            .map_err(map_seed)?;
        let (content_json, text, chosung) = prepared.into_parts();
        bodies.push(DuplicateBody {
            source_id: source.id,
            content_json,
            text,
            chosung,
            seed,
        });
    }
    Ok(bodies)
}

pub(crate) async fn duplicate(
    state: &AppState,
    peer: SocketAddr,
    headers: &HeaderMap,
    workspace_id: Uuid,
    scope: DocumentScope,
    document_id: Uuid,
    auth: &RequestAuth,
    bytes: &[u8],
) -> Result<Response, DocumentApiError> {
    let (title, include_children) = parse_duplicate_input(bytes)?;
    check_origin(headers, &state.public_origin)?;
    let pool = state
        .auth
        .db
        .pool
        .postgres("src/http/routes/document_body.rs")
        .map_err(internal)?;
    let sources = db_result(
        load_duplicate_sources(
            pool,
            workspace_id,
            auth.user_id,
            auth.credential_id,
            scope,
            document_id,
            include_children,
        )
        .await,
    )?;
    let bodies = prepare_duplicate_bodies(state, &sources).await?;
    let ip = peer_ip(peer.ip());
    let meta = db_result(
        commit_duplicate(
            pool,
            workspace_id,
            auth.user_id,
            auth.credential_id,
            scope,
            title.as_deref(),
            &sources,
            &bodies,
            Some(&ip),
        )
        .await,
    )?;
    Ok((StatusCode::CREATED, Json(meta_response(&meta, true))).into_response())
}

async fn duplicate_wiki(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, document_id)): WikiPath,
    body: Bytes,
) -> Result<Response, DocumentApiError> {
    let auth = auth(
        &state,
        &headers,
        &jar,
        ApiTokenScope::DocumentsWrite,
        workspace_id,
    )
    .await?;
    duplicate(
        &state,
        peer,
        &headers,
        workspace_id,
        DocumentScope::Wiki,
        document_id,
        &auth,
        &body,
    )
    .await
}

async fn duplicate_project(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id, document_id)): ProjectPath,
    body: Bytes,
) -> Result<Response, DocumentApiError> {
    let auth = auth(
        &state,
        &headers,
        &jar,
        ApiTokenScope::DocumentsWrite,
        workspace_id,
    )
    .await?;
    duplicate(
        &state,
        peer,
        &headers,
        workspace_id,
        DocumentScope::Project(project_id),
        document_id,
        &auth,
        &body,
    )
    .await
}

// ---------------------------------------------------------------------------
// Flat `/documents/{documentId}` routes (source `byId`, `updateById`,
// `removeById`, `duplicateById`): session only, the workspace and affiliation
// come from the document itself.

async fn locate(
    state: &AppState,
    headers: &HeaderMap,
    jar: &CookieJar,
    document_id: Uuid,
) -> Result<(RequestAuth, Uuid, DocumentScope), DocumentApiError> {
    let auth = require_request_auth(state, headers, jar, Access::Session, None).await?;
    let located = crate::db::document_ops::locate_document(
        state
            .auth
            .db
            .pool
            .postgres("src/http/routes/document_body.rs")
            .map_err(internal)?,
        auth.user_id,
        document_id,
    )
    .await
    .map_err(internal)?;
    let Some((workspace_id, project_id)) = located else {
        return Err(AppError::from_code(ProblemCode::NotFound).into());
    };
    let scope = match project_id {
        None => DocumentScope::Wiki,
        Some(project_id) => DocumentScope::Project(project_id),
    };
    Ok((auth, workspace_id, scope))
}

async fn get_by_id(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(document_id): Path<Uuid>,
) -> Result<Json<DocumentMetaResponse>, DocumentApiError> {
    let (auth, workspace_id, scope) = locate(&state, &headers, &jar, document_id).await?;
    let meta = db_result(
        authorize_document(
            state
                .auth
                .db
                .pool
                .postgres("src/http/routes/document_body.rs")
                .map_err(internal)?,
            workspace_id,
            auth.user_id,
            auth.credential_id,
            scope,
            document_id,
            ProjectPermission::View,
        )
        .await,
    )?;
    Ok(Json(meta_response(&meta, false)))
}

async fn update_by_id(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(document_id): Path<Uuid>,
    body: Result<Json<crate::api::dto::PatchDocumentBody>, JsonRejection>,
) -> Result<Json<DocumentMetaResponse>, DocumentApiError> {
    let Json(body) = body.map_err(AppError::from)?;
    check_origin(&headers, &state.public_origin)?;
    crate::http::routes::documents::validate_patch_body(&body)?;
    let (auth, workspace_id, scope) = locate(&state, &headers, &jar, document_id).await?;
    let ip = peer_ip(peer.ip());
    let input = crate::db::documents::UpdateDocumentMetaInput {
        title: body.title.as_deref().map(str::trim),
        icon: body.icon.as_ref().map(|icon| icon.as_deref()),
        status: body.status.as_deref(),
    };
    let pool = state
        .auth
        .db
        .pool
        .postgres("src/http/routes/document_body.rs")
        .map_err(internal)?;
    let meta = match scope {
        DocumentScope::Wiki => db_result(
            crate::db::documents::update_wiki_document_meta(
                pool,
                workspace_id,
                auth.user_id,
                auth.credential_id,
                document_id,
                input,
                Some(&ip),
            )
            .await,
        )?,
        DocumentScope::Project(project_id) => db_result(
            crate::db::project_documents::update_project_document_meta(
                pool,
                workspace_id,
                project_id,
                document_id,
                auth.user_id,
                auth.credential_id,
                input,
                Some(&ip),
            )
            .await,
        )?,
    };
    Ok(Json(meta_response(&meta, false)))
}

#[derive(Deserialize)]
struct RemoveQuery {
    children: Option<String>,
}

async fn remove_by_id(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(document_id): Path<Uuid>,
    Query(query): Query<RemoveQuery>,
) -> Result<Json<crate::api::dto::OkResponse>, DocumentApiError> {
    check_origin(&headers, &state.public_origin)?;
    let children = crate::http::routes::documents::parse_trash_children(query.children.as_deref())?;
    let (auth, workspace_id, scope) = locate(&state, &headers, &jar, document_id).await?;
    let ip = peer_ip(peer.ip());
    let pool = state
        .auth
        .db
        .pool
        .postgres("src/http/routes/document_body.rs")
        .map_err(internal)?;
    match scope {
        DocumentScope::Wiki => db_result(
            crate::db::documents::trash_wiki_document(
                pool,
                workspace_id,
                auth.user_id,
                auth.credential_id,
                document_id,
                children,
                Some(&ip),
            )
            .await,
        )?,
        DocumentScope::Project(project_id) => db_result(
            crate::db::project_documents::trash_project_document(
                pool,
                workspace_id,
                project_id,
                document_id,
                auth.user_id,
                auth.credential_id,
                children,
                Some(&ip),
            )
            .await,
        )?,
    };
    Ok(Json(crate::api::dto::OkResponse { ok: true }))
}

async fn duplicate_by_id(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(document_id): Path<Uuid>,
    body: Bytes,
) -> Result<Response, DocumentApiError> {
    let (auth, workspace_id, scope) = locate(&state, &headers, &jar, document_id).await?;
    duplicate(
        &state,
        peer,
        &headers,
        workspace_id,
        scope,
        document_id,
        &auth,
        &body,
    )
    .await
}

fn require_realtime_writer(state: &AppState) -> Result<(), DocumentApiError> {
    if state.realtime_mode != crate::config::RealtimeMode::On {
        return Err(coded(
            StatusCode::CONFLICT,
            "body_writer_mode_mismatch",
            "body writer mode mismatch",
        ));
    }
    Ok(())
}
fn require_off_writer(state: &AppState) -> Result<crate::collab::CollabConfig, DocumentApiError> {
    if state.realtime_mode != crate::config::RealtimeMode::Off {
        return Err(coded(
            StatusCode::CONFLICT,
            "body_writer_mode_mismatch",
            "body writer mode mismatch",
        ));
    }
    state.native_engine.clone().ok_or_else(collab_unavailable)
}
async fn create_wiki_from_draft(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace): Path<Uuid>,
    body: Bytes,
) -> Result<Response, DocumentApiError> {
    create_from_draft_inner(&state, peer, &headers, &jar, workspace, None, &body).await
}
async fn create_project_from_draft(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace, project)): Path<(Uuid, Uuid)>,
    body: Bytes,
) -> Result<Response, DocumentApiError> {
    create_from_draft_inner(
        &state,
        peer,
        &headers,
        &jar,
        workspace,
        Some(project),
        &body,
    )
    .await
}
async fn create_from_draft_inner(
    state: &AppState,
    peer: SocketAddr,
    headers: &HeaderMap,
    jar: &CookieJar,
    workspace: Uuid,
    destination_project: Option<Uuid>,
    body: &[u8],
) -> Result<Response, DocumentApiError> {
    let engine = require_off_writer(state)?;
    check_origin(headers, &state.public_origin)?;
    let input: crate::api::dto::OffDraftCreateBody =
        serde_json::from_slice(body).map_err(|_| invalid_body())?;
    let actor = auth(
        state,
        headers,
        jar,
        ApiTokenScope::DocumentsWrite,
        workspace,
    )
    .await?;
    if input.source_kind == "task" {
        let reader = auth(state, headers, jar, ApiTokenScope::TasksRead, workspace).await?;
        if reader.user_id != actor.user_id || reader.credential_id != actor.credential_id {
            return Err(AppError::from_code(ProblemCode::AuthenticationRequired).into());
        }
    }
    let request = parse_off_draft_request(
        workspace,
        destination_project,
        actor.user_id,
        actor.credential_id,
        input,
        Some(peer_ip(peer.ip())),
    )?;
    revision_write_limit(state, actor.user_id).await?;
    let created = crate::db::body_save::create_off_draft(
        &state.auth.db.pool,
        state.realtime_mode,
        engine,
        request,
    )
    .await
    .map_err(map_off_draft_error)?;
    Ok((
        StatusCode::CREATED,
        Json(crate::api::dto::OffDraftCreateResponse {
            document: meta_response(&created.document, true),
            command_id: created.command_id,
            tail_seq: created.tail_seq,
            revision_id: created.revision_id,
        }),
    )
        .into_response())
}
fn map_off_draft_error(error: crate::db::body_save::OffDraftCreateError) -> DocumentApiError {
    use crate::db::body_save::{BodySaveError, OffDraftCreateError};
    match error {
        OffDraftCreateError::Body(BodySaveError::RequestMismatch) => coded(
            StatusCode::CONFLICT,
            "request_mismatch",
            "creation command does not match the original request",
        ),
        OffDraftCreateError::Body(error) => map_versioned_body_error(error),
        OffDraftCreateError::Document(error) => map_document_error(error),
        OffDraftCreateError::Attachment(_) => AppError::from_code(ProblemCode::NotFound).into(),
        OffDraftCreateError::RollbackUnconfirmed { original, cleanup } => {
            tracing::warn!(%original,%cleanup,"OFF draft rollback unconfirmed");
            coded(
                StatusCode::SERVICE_UNAVAILABLE,
                "body_save_unconfirmed",
                "body save is unconfirmed",
            )
        }
    }
}

/// Dedicated creation contract; ordinary strict wiki/project create DTOs and
/// their command/hash domains are unchanged. Destination project is the route,
/// while source project belongs only to the currently authorized source scope.
fn parse_off_draft_request(
    workspace: Uuid,
    destination_project: Option<Uuid>,
    actor: Uuid,
    credential: Uuid,
    body: crate::api::dto::OffDraftCreateBody,
    client_ip: Option<String>,
) -> Result<crate::db::body_save::OffDraftCreateRequest, DocumentApiError> {
    use crate::api::dto::RequiredNullable;
    use crate::db::revisions::{RevisionScope, RevisionTarget};
    let parent = match body.parent_id {
        RequiredNullable::Missing => {
            return Err(AppError::from_code(ProblemCode::InvalidInput).into())
        }
        RequiredNullable::Null => None,
        RequiredNullable::Value(id) => Some(id),
    };
    let source = match (body.source_kind.as_str(), body.source_project_id) {
        ("document", Some(project)) if !project.is_nil() => {
            RevisionScope::project_document(project, body.source_id)
        }
        ("document", None) => RevisionTarget::Document(body.source_id).into(),
        ("task", None) => RevisionTarget::Task(body.source_id).into(),
        _ => return Err(invalid_body()),
    };
    let title = body.title.trim();
    let icon = body.icon.flatten();
    if body.command_id.is_nil()
        || body.source_id.is_nil()
        || parent.is_some_and(|id| id.is_nil())
        || destination_project.is_some_and(|id| id.is_nil())
        || !crate::db::documents::title_is_valid(title)
        || icon
            .as_ref()
            .is_some_and(|icon| !crate::db::documents::icon_is_valid(icon))
    {
        return Err(AppError::from_code(ProblemCode::InvalidInput).into());
    }
    crate::collab::derived_body::extract_stored_attachment_refs(&body.content_json).map_err(
        |error| match error {
            DerivedBodyError::TooLarge => too_large(),
            DerivedBodyError::InvalidDocumentBody(_) => invalid_body(),
        },
    )?;
    Ok(crate::db::body_save::OffDraftCreateRequest {
        workspace,
        source,
        destination_project,
        parent,
        actor,
        credential,
        command: body.command_id,
        title: title.to_string(),
        icon,
        content_json: body.content_json,
        client_ip,
    })
}

fn parse_body_tail(value: &str) -> Option<i64> {
    if value.is_empty()
        || (value.len() > 1 && value.starts_with('0'))
        || !value.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    value.parse::<i64>().ok().filter(|seq| *seq < i64::MAX)
}
fn map_versioned_body_error(error: crate::db::body_save::BodySaveError) -> DocumentApiError {
    use crate::db::body_save::BodySaveError;
    use crate::db::collab::CollabDbError;
    match error {
        BodySaveError::Conflict | BodySaveError::RequestMismatch => coded(
            StatusCode::CONFLICT,
            "document_version_mismatch",
            "document version mismatch",
        ),
        BodySaveError::Native(CollabDbError::StaleCutoff | CollabDbError::StaleWriter) => coded(
            StatusCode::CONFLICT,
            "document_version_mismatch",
            "document version mismatch",
        ),
        BodySaveError::Native(
            CollabDbError::PayloadTooLarge | CollabDbError::StateBudgetExceeded,
        ) => too_large(),
        BodySaveError::Native(_) | BodySaveError::Revision(_) => {
            AppError::from_code(ProblemCode::NotFound).into()
        }
        BodySaveError::Invalid => invalid_body(),
        BodySaveError::Unavailable | BodySaveError::Cancelled => collab_unavailable(),
        BodySaveError::CommitUnconfirmed(error) => {
            tracing::warn!(%error, settlement=?error.settlement,"OFF body finish unconfirmed");
            coded(
                StatusCode::SERVICE_UNAVAILABLE,
                "body_save_unconfirmed",
                "body save is unconfirmed",
            )
        }
        BodySaveError::RollbackUnconfirmed { original, cleanup } => {
            tracing::warn!(%original,%cleanup,"OFF body rollback unconfirmed");
            coded(
                StatusCode::SERVICE_UNAVAILABLE,
                "body_save_unconfirmed",
                "body save is unconfirmed",
            )
        }
        BodySaveError::Database(error) => {
            if matches!(&error, sqlx::Error::AnyDriverError(driver) if driver.downcast_ref::<crate::db::backend::RemoteSettlementUnconfirmed>().is_some())
            {
                coded(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "body_save_unconfirmed",
                    "body save is unconfirmed",
                )
            } else {
                internal(error).into()
            }
        }
    }
}
async fn get_versioned_body(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace, document)): WikiPath,
) -> Result<Json<VersionedBodyResponse>, DocumentApiError> {
    read_versioned_body_inner(
        state,
        headers,
        jar,
        workspace,
        crate::db::revisions::RevisionTarget::Document(document),
        None,
    )
    .await
}
async fn get_project_versioned_body(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace, project, document)): ProjectPath,
) -> Result<Json<VersionedBodyResponse>, DocumentApiError> {
    read_versioned_body_inner(
        state,
        headers,
        jar,
        workspace,
        crate::db::revisions::RevisionTarget::Document(document),
        Some(project),
    )
    .await
}
pub(crate) async fn read_versioned_body_inner(
    state: AppState,
    headers: HeaderMap,
    jar: CookieJar,
    workspace: Uuid,
    target: crate::db::revisions::RevisionTarget,
    project: Option<Uuid>,
) -> Result<Json<VersionedBodyResponse>, DocumentApiError> {
    let engine = require_off_writer(&state)?;
    let actor = auth(
        &state,
        &headers,
        &jar,
        match target {
            crate::db::revisions::RevisionTarget::Document(_) => ApiTokenScope::DocumentsRead,
            crate::db::revisions::RevisionTarget::Task(_) => ApiTokenScope::TasksRead,
        },
        workspace,
    )
    .await?;
    let source = crate::db::body_save::read_off_body(
        &state.auth.db.pool,
        state.realtime_mode,
        engine,
        workspace,
        match (target, project) {
            (crate::db::revisions::RevisionTarget::Document(document), Some(project)) => {
                crate::db::revisions::RevisionScope::project_document(project, document)
            }
            _ => target.into(),
        },
        actor.user_id,
        actor.credential_id,
    )
    .await
    .map_err(map_versioned_body_error)?;
    Ok(Json(VersionedBodyResponse {
        target_id: target.id(),
        tail_seq: source.native.tail_seq.to_string(),
        snapshot_v1: collab_engine::b64::encode(&source.native.snapshot),
        tail_v1: source
            .native
            .tail
            .iter()
            .map(|row| collab_engine::b64::encode(&row.payload))
            .collect(),
        content_json: source.content_json,
        writable: source.writable,
    }))
}
async fn save_versioned_body(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path((workspace, document)): WikiPath,
    body: Bytes,
) -> Result<Json<SaveVersionedBodyResponse>, DocumentApiError> {
    save_versioned_body_inner(
        state,
        headers,
        jar,
        peer,
        workspace,
        crate::db::revisions::RevisionTarget::Document(document),
        None,
        body,
    )
    .await
}
async fn save_project_versioned_body(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path((workspace, project, document)): ProjectPath,
    body: Bytes,
) -> Result<Json<SaveVersionedBodyResponse>, DocumentApiError> {
    save_versioned_body_inner(
        state,
        headers,
        jar,
        peer,
        workspace,
        crate::db::revisions::RevisionTarget::Document(document),
        Some(project),
        body,
    )
    .await
}
pub(crate) async fn save_versioned_body_inner(
    state: AppState,
    headers: HeaderMap,
    jar: CookieJar,
    peer: SocketAddr,
    workspace: Uuid,
    target: crate::db::revisions::RevisionTarget,
    project: Option<Uuid>,
    body: Bytes,
) -> Result<Json<SaveVersionedBodyResponse>, DocumentApiError> {
    let engine = require_off_writer(&state)?;
    check_origin(&headers, &state.public_origin)?;
    let input: SaveVersionedBodyInput =
        serde_json::from_slice(&body).map_err(|_| invalid_body())?;
    let expected = parse_body_tail(&input.expected_tail_seq).ok_or_else(invalid_body)?;
    if input.update_v1.len() > (crate::db::collab::MAX_COLLAB_UPDATE_BYTES * 4).div_ceil(3) + 4 {
        return Err(too_large());
    }
    let update = collab_engine::b64::decode(&input.update_v1).map_err(|_| invalid_body())?;
    let actor = auth(
        &state,
        &headers,
        &jar,
        match target {
            crate::db::revisions::RevisionTarget::Document(_) => ApiTokenScope::DocumentsWrite,
            crate::db::revisions::RevisionTarget::Task(_) => ApiTokenScope::TasksWrite,
        },
        workspace,
    )
    .await?;
    revision_write_limit(&state, actor.user_id).await?;
    let saved = crate::db::body_save::save_off_body(
        &state.auth.db.pool,
        state.realtime_mode,
        engine,
        crate::db::body_save::OffBodyRequest {
            workspace,
            target,
            project,
            actor: actor.user_id,
            credential: actor.credential_id,
            command: input.command_id,
            expected_tail: expected,
            update,
            client_ip: Some(peer_ip(peer.ip())),
        },
    )
    .await
    .map_err(map_versioned_body_error)?;
    Ok(Json(SaveVersionedBodyResponse {
        command_id: saved.command_id,
        target_id: saved.target_id,
        tail_seq: saved.tail_seq,
        revision_id: saved.revision_id,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dedicated_draft_route_keeps_source_destination_parent_and_current_create_limits() {
        let workspace = Uuid::now_v7();
        let actor = Uuid::now_v7();
        let credential = Uuid::now_v7();
        let source = Uuid::now_v7();
        let source_project = Uuid::now_v7();
        let destination_project = Uuid::now_v7();
        let parent = Uuid::now_v7();
        let raw = serde_json::json!({"commandId":Uuid::now_v7(),"sourceKind":"document","sourceId":source,
            "sourceProjectId":source_project,"parentId":parent,"title":" private 😀 ","contentJson":{"type":"doc","content":[]}});
        let request = parse_off_draft_request(
            workspace,
            Some(destination_project),
            actor,
            credential,
            serde_json::from_value(raw.clone()).unwrap(),
            None,
        )
        .unwrap();
        assert_eq!(request.source.project_id(), Some(source_project));
        assert_eq!(
            request.source.target(),
            crate::db::revisions::RevisionTarget::Document(source)
        );
        assert_eq!(request.destination_project, Some(destination_project));
        assert_eq!(request.parent, Some(parent));
        assert_eq!(request.title, "private 😀");
        for change in ["parent", "kind", "task_project", "nil", "title", "ref"] {
            let mut refused = raw.clone();
            match change {
                "parent" => {
                    refused.as_object_mut().unwrap().remove("parentId");
                }
                "kind" => refused["sourceKind"] = serde_json::json!("unregistered"),
                "task_project" => refused["sourceKind"] = serde_json::json!("task"),
                "nil" => refused["commandId"] = serde_json::json!(Uuid::nil()),
                "title" => refused["title"] = serde_json::json!("x".repeat(301)),
                _ => {
                    refused["contentJson"] = serde_json::json!({"type":"doc","content":[{"type":"attachment","attrs":{"id":"bad-ref"}}]})
                }
            }
            assert!(
                parse_off_draft_request(
                    workspace,
                    Some(destination_project),
                    actor,
                    credential,
                    serde_json::from_value(refused).unwrap(),
                    None
                )
                .is_err(),
                "{change} cannot become implicit create rights or content"
            );
        }
        let task = serde_json::json!({"commandId":Uuid::now_v7(),"sourceKind":"task","sourceId":source,"parentId":null,"title":"Task private copy","contentJson":{"type":"doc","content":[]}});
        let parsed = parse_off_draft_request(
            workspace,
            None,
            actor,
            credential,
            serde_json::from_value(task).unwrap(),
            None,
        )
        .unwrap();
        assert_eq!(
            parsed.source.target(),
            crate::db::revisions::RevisionTarget::Task(source)
        );
        assert_eq!(parsed.source.project_id(), None);
        assert_eq!(parsed.destination_project, None);
    }

    #[test]
    fn body_input_requires_exactly_one_field() {
        assert!(matches!(
            parse_body_input(br##"{"contentMd":"# a"}"##),
            Ok(BodyInput::Markdown(_))
        ));
        assert!(matches!(
            parse_body_input(br#"{"contentJson":{"type":"doc","content":[]}}"#),
            Ok(BodyInput::Json(_))
        ));
        for bad in [
            &br#"{}"#[..],
            br#"{"contentMd":"a","contentJson":{}}"#,
            br#"{"contentMd":1}"#,
            br#"{"other":1}"#,
            br#"[]"#,
        ] {
            assert!(parse_body_input(bad).is_err());
        }
    }

    #[test]
    fn block_input_rejects_foreign_attrs_id() {
        assert!(parse_block_input(br#"{"type":"paragraph","attrs":{"id":"b"}}"#, "b").is_ok());
        assert!(parse_block_input(br#"{"type":"paragraph","attrs":{"id":"x"}}"#, "b").is_err());
        assert!(parse_block_input(br#"{"type":""}"#, "b").is_err());
        assert!(parse_block_input(br#"{"type":"p","extra":1}"#, "b").is_err());
    }
}
