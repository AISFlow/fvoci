//! Source `apps/server/src/domains/share/routes.ts`.
//!
//! Member routes (`share.manage` for API tokens): list/create/revoke share links
//! and the document-scoped list/create on wiki and project document paths.
//! Public routes: `/share/{token}/...`, rate limited per client IP
//! (source `share-ip`, 60/min). Every public request re-resolves the token.

use std::collections::HashSet;
use std::net::SocketAddr;
use std::time::Duration;

use axum::body::Body;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::header::{
    CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_SECURITY_POLICY, CONTENT_TYPE,
    ETAG, IF_NONE_MATCH,
};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get};
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::SecondsFormat;
use rand::RngCore;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::api::dto::{
    AttachmentDownloadQuery, AttachmentOutput, AttachmentPreviewResponse,
    DocumentShareLinkCreateBody, OkResponse, SearchItemOutput, SearchListResponse,
    SearchSnippetPiece, ShareCreateBody, ShareLinkCreatedOutput, ShareLinkListResponse,
    ShareLinkOutput, SharePublicMetaOutput, TreeNodeResponse, TreeResponse,
};
use crate::attachments::content_disposition_attachment;
use crate::auth::scopes::ApiTokenScope;
use crate::db::share::{
    create_share_link, hydrate_share_hits, list_share_links, revoke_share_link, share_attachment,
    share_document, share_public_meta, share_search_scope, share_tree, DocumentAffiliation,
    ShareDbError, ShareLinkRecord, ShareSearchRow, ShareTarget,
};
use crate::documents::export::{
    export_filename, render_document_export, ExportFormat, ExportRenderError,
};
use crate::error::{AppError, ProblemCode};
use crate::http::authz::{require_request_auth, Access};
use crate::http::guard::check_origin;
use crate::http::rate_limit::peer_ip;
use crate::http::routes::stars::parse_body_uuid;
use crate::http::state::AppState;
use crate::search::meili::{
    search_meili, MeiliHit, MeiliSearchInput, MeiliSearchScope, SearchSourceKind,
};
use crate::search::query::{highlight_snippet, parse_snippet, CHOSUNG_MIN_LENGTH};
use crate::search::text::{is_chosung_query, stem_text};
use crate::share_render::{is_tiptap_doc, tiptap_doc_to_md, tiptap_doc_to_safe_html};
use crate::validate::utf16_len;

const SHARE_IP_LIMIT: u32 = 60;
const FRAGMENT_CSP: &str = "default-src 'none'; sandbox";
const SHARE_IP_WINDOW: Duration = Duration::from_secs(60);
const SHARE_SEARCH_LIMIT: usize = 50;
const SHARE_MEILI_PAGE: u32 = 50;
const SHARE_MEILI_PAGES: usize = 8;
const SHARE_DOC_ID_CHUNK: usize = 200;
const SHARE_SEARCH_DEADLINE: Duration = Duration::from_millis(2_000);

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/workspaces/{workspace_id}/share-links",
            get(list_links_route).post(create_link_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/share-links/{id}",
            delete(revoke_link_route),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/documents/{id}/share-links",
            get(list_wiki_document_links).post(create_wiki_document_link),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{id}/share-links",
            get(list_project_document_links).post(create_project_document_link),
        )
        .merge(public_router())
}

/// Public routes carry `Referrer-Policy: no-referrer` on every response
/// (including errors) so the token in the URL never leaks via Referer.
fn public_router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/share/{token}", get(public_meta_route))
        .route("/api/v1/share/{token}/body", get(public_body_route))
        .route("/api/v1/share/{token}/tree", get(public_tree_route))
        .route("/api/v1/share/{token}/pdf", get(public_pdf_route))
        .route("/api/v1/share/{token}/search", get(public_search_route))
        .route(
            "/api/v1/share/{token}/documents/{document_id}",
            get(public_document_route),
        )
        .route(
            "/api/v1/share/{token}/attachments/{attachment_id}",
            get(public_attachment_route),
        )
        .route(
            "/api/v1/share/{token}/attachments/{attachment_id}/download",
            get(public_download_route),
        )
        .layer(axum::middleware::map_response(no_referrer))
}

async fn no_referrer(mut response: Response) -> Response {
    response.headers_mut().insert(
        axum::http::header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

fn iso(at: chrono::DateTime<chrono::Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn link_output(item: ShareLinkRecord) -> ShareLinkOutput {
    ShareLinkOutput {
        id: item.id.to_string(),
        workspace_id: item.workspace_id.to_string(),
        document_id: item.document_id.map(|id| id.to_string()),
        project_id: item.project_id.map(|id| id.to_string()),
        expires_at: iso(item.expires_at),
        created_at: iso(item.created_at),
    }
}

/// Source `humanPaths.share(token)` joined onto the public URL.
pub fn share_page_url(public_origin: &str, token: &str) -> String {
    format!("{}/s/{token}", public_origin.trim_end_matches('/'))
}

fn not_found() -> AppError {
    AppError::from_code(ProblemCode::NotFound)
}

fn map_share_error(err: ShareDbError) -> AppError {
    match err {
        ShareDbError::NotFound | ShareDbError::Forbidden => not_found(),
        ShareDbError::InvalidInput => AppError::from_code(ProblemCode::InvalidInput),
    }
}

/// Source share public routes: 404 for every route while the instance share
/// policy is disabled (checked before anything else, like `onBeforeHandle`).
async fn ensure_sharing_enabled(state: &AppState) -> Result<(), AppError> {
    let policy = crate::settings::share_policy(&state.auth.db.pool)
        .await
        .map_err(internal)?;
    if policy.enabled {
        Ok(())
    } else {
        Err(not_found())
    }
}

fn internal(err: sqlx::Error) -> AppError {
    tracing::error!("database error: {}", err);
    AppError::internal()
}

/// Source `z.number().int().min(1).max(365).optional()`.
fn parse_expires_days(value: Option<f64>) -> Result<Option<i64>, AppError> {
    match value {
        None => Ok(None),
        Some(v) if v.is_finite() && v.fract() == 0.0 && (1.0..=365.0).contains(&v) => {
            Ok(Some(v as i64))
        }
        Some(_) => Err(AppError::with_source(
            ProblemCode::InvalidInput,
            "/expiresInDays",
        )),
    }
}

async fn require_share_manage(
    state: &AppState,
    headers: &HeaderMap,
    jar: &CookieJar,
    workspace_id: Uuid,
) -> Result<(Uuid, Uuid), AppError> {
    let auth = require_request_auth(
        state,
        headers,
        jar,
        Access::Scope(ApiTokenScope::ShareManage),
        Some(workspace_id),
    )
    .await?;
    Ok((auth.user_id, auth.credential_id))
}

async fn list_links_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
) -> Result<Json<ShareLinkListResponse>, AppError> {
    let (user_id, credential_id) =
        require_share_manage(&state, &headers, &jar, workspace_id).await?;
    let items = list_share_links(
        &state.auth.db.pool,
        workspace_id,
        user_id,
        credential_id,
        None,
    )
    .await
    .map_err(internal)?
    .map_err(map_share_error)?;
    Ok(Json(ShareLinkListResponse {
        items: items.into_iter().map(link_output).collect(),
    }))
}

async fn create_link(
    state: &AppState,
    workspace_id: Uuid,
    user_id: Uuid,
    credential_id: Uuid,
    target: ShareTarget,
    expires_in_days: Option<i64>,
    affiliation: Option<DocumentAffiliation>,
) -> Result<Response, AppError> {
    let policy = crate::settings::share_policy(&state.auth.db.pool)
        .await
        .map_err(internal)?;
    let created = create_share_link(
        &state.auth.db.pool,
        workspace_id,
        user_id,
        credential_id,
        target,
        expires_in_days,
        &policy,
        affiliation,
    )
    .await
    .map_err(internal)?
    .map_err(map_share_error)?;
    let url = share_page_url(&state.public_origin, &created.token);
    let record = link_output(created.record);
    Ok((
        StatusCode::CREATED,
        Json(ShareLinkCreatedOutput {
            id: record.id,
            workspace_id: record.workspace_id,
            document_id: record.document_id,
            project_id: record.project_id,
            expires_at: record.expires_at,
            created_at: record.created_at,
            url,
        }),
    )
        .into_response())
}

async fn create_link_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path(workspace_id): Path<Uuid>,
    body: Result<Json<ShareCreateBody>, JsonRejection>,
) -> Result<Response, AppError> {
    check_origin(&headers, &state.public_origin)?;
    let (user_id, credential_id) =
        require_share_manage(&state, &headers, &jar, workspace_id).await?;
    let Json(body) = body.map_err(AppError::from)?;
    let target = match (body.document_id.as_deref(), body.project_id.as_deref()) {
        (Some(document_id), None) => ShareTarget::Document(
            parse_body_uuid(document_id)
                .ok_or_else(|| AppError::with_source(ProblemCode::InvalidInput, "/documentId"))?,
        ),
        (None, Some(project_id)) => ShareTarget::Project(
            parse_body_uuid(project_id)
                .ok_or_else(|| AppError::with_source(ProblemCode::InvalidInput, "/projectId"))?,
        ),
        _ => return Err(AppError::from_code(ProblemCode::InvalidInput)),
    };
    let days = parse_expires_days(body.expires_in_days)?;
    create_link(
        &state,
        workspace_id,
        user_id,
        credential_id,
        target,
        days,
        None,
    )
    .await
}

async fn revoke_link_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, share_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<OkResponse>, AppError> {
    check_origin(&headers, &state.public_origin)?;
    let (user_id, credential_id) =
        require_share_manage(&state, &headers, &jar, workspace_id).await?;
    revoke_share_link(
        &state.auth.db.pool,
        workspace_id,
        user_id,
        credential_id,
        share_id,
    )
    .await
    .map_err(internal)?
    .map_err(map_share_error)?;
    Ok(Json(OkResponse { ok: true }))
}

async fn list_document_links(
    state: &AppState,
    headers: &HeaderMap,
    jar: &CookieJar,
    workspace_id: Uuid,
    document_id: Uuid,
    affiliation: DocumentAffiliation,
) -> Result<Json<ShareLinkListResponse>, AppError> {
    let (user_id, credential_id) = require_share_manage(state, headers, jar, workspace_id).await?;
    let items = list_share_links(
        &state.auth.db.pool,
        workspace_id,
        user_id,
        credential_id,
        Some((document_id, affiliation)),
    )
    .await
    .map_err(internal)?
    .map_err(map_share_error)?;
    Ok(Json(ShareLinkListResponse {
        items: items.into_iter().map(link_output).collect(),
    }))
}

async fn create_document_link(
    state: &AppState,
    headers: &HeaderMap,
    jar: &CookieJar,
    workspace_id: Uuid,
    document_id: Uuid,
    affiliation: DocumentAffiliation,
    body: Result<Json<DocumentShareLinkCreateBody>, JsonRejection>,
) -> Result<Response, AppError> {
    check_origin(headers, &state.public_origin)?;
    let (user_id, credential_id) = require_share_manage(state, headers, jar, workspace_id).await?;
    let Json(body) = body.map_err(AppError::from)?;
    let days = parse_expires_days(body.expires_in_days)?;
    create_link(
        state,
        workspace_id,
        user_id,
        credential_id,
        ShareTarget::Document(document_id),
        days,
        Some(affiliation),
    )
    .await
}

async fn list_wiki_document_links(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, document_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<ShareLinkListResponse>, AppError> {
    list_document_links(
        &state,
        &headers,
        &jar,
        workspace_id,
        document_id,
        DocumentAffiliation::Workspace,
    )
    .await
}

async fn create_wiki_document_link(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, document_id)): Path<(Uuid, Uuid)>,
    body: Result<Json<DocumentShareLinkCreateBody>, JsonRejection>,
) -> Result<Response, AppError> {
    create_document_link(
        &state,
        &headers,
        &jar,
        workspace_id,
        document_id,
        DocumentAffiliation::Workspace,
        body,
    )
    .await
}

async fn list_project_document_links(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id, document_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<Json<ShareLinkListResponse>, AppError> {
    list_document_links(
        &state,
        &headers,
        &jar,
        workspace_id,
        document_id,
        DocumentAffiliation::Project(project_id),
    )
    .await
}

async fn create_project_document_link(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Path((workspace_id, project_id, document_id)): Path<(Uuid, Uuid, Uuid)>,
    body: Result<Json<DocumentShareLinkCreateBody>, JsonRejection>,
) -> Result<Response, AppError> {
    create_document_link(
        &state,
        &headers,
        &jar,
        workspace_id,
        document_id,
        DocumentAffiliation::Project(project_id),
        body,
    )
    .await
}

// ---------------------------------------------------------------------------
// Public routes
// ---------------------------------------------------------------------------

async fn enforce_share_limit(state: &AppState, peer: SocketAddr) -> Result<(), AppError> {
    let ip = peer_ip(peer.ip());
    state
        .rate_limiter
        .allow_window(&format!("share-ip:{ip}"), SHARE_IP_LIMIT, SHARE_IP_WINDOW)
        .await
        .map_err(AppError::rate_limited)
}

/// Source server.ts `shareOgMeta` behind `onShareLimit` for the `/s/{token}`
/// shell: `None` while sharing is disabled, over the share-ip limit (the
/// shell is still served, without a 429), or for a token that does not
/// resolve. Other failures are errors, as in the source.
pub(crate) async fn shell_head_meta(
    state: &AppState,
    peer: Option<SocketAddr>,
    token: &str,
) -> Result<Option<crate::db::share::SharePublicMeta>, AppError> {
    let Some(peer) = peer else {
        return Ok(None);
    };
    if enforce_share_limit(state, peer).await.is_err() {
        return Ok(None);
    }
    let policy = crate::settings::share_policy(&state.auth.db.pool)
        .await
        .map_err(internal)?;
    if !policy.enabled {
        return Ok(None);
    }
    share_public_meta(&state.auth.db.pool, token, true)
        .await
        .map_err(internal)
}

async fn public_meta_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path(token): Path<String>,
) -> Result<Json<SharePublicMetaOutput>, AppError> {
    ensure_sharing_enabled(&state).await?;
    enforce_share_limit(&state, peer).await?;
    let meta = share_public_meta(&state.auth.db.pool, &token, false)
        .await
        .map_err(internal)?
        .ok_or_else(not_found)?;
    Ok(Json(SharePublicMetaOutput {
        title: meta.title,
        document_id: meta.document_id.map(|id| id.to_string()),
        project_id: meta.project_id.map(|id| id.to_string()),
        expires_at: iso(meta.expires_at),
    }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BodyFormat {
    Html,
    Fragment,
    Markdown,
}

impl BodyFormat {
    fn as_str(self) -> &'static str {
        match self {
            Self::Html => "html",
            Self::Fragment => "fragment",
            Self::Markdown => "md",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShareBodyQuery {
    pub format: Option<String>,
}

fn parse_format(query: &ShareBodyQuery) -> Result<BodyFormat, AppError> {
    match query.format.as_deref() {
        None | Some("html") => Ok(BodyFormat::Html),
        Some("fragment") => Ok(BodyFormat::Fragment),
        Some("md") => Ok(BodyFormat::Markdown),
        Some(_) => Err(AppError::from_code(ProblemCode::InvalidInput)),
    }
}

/// Source `strongEtag(updatedAt.toISOString(), format)`: first 16 hex of
/// sha256("<iso>|<format>").
fn strong_etag(updated_at_iso: &str, format: BodyFormat) -> String {
    let digest = Sha256::digest(format!("{updated_at_iso}|{}", format.as_str()).as_bytes());
    format!("\"{}\"", &hex::encode(digest)[..16])
}

/// Source `ifNoneMatches`: weak comparison, `*` matches.
pub(crate) fn if_none_matches(header: &str, tag: &str) -> bool {
    if header.trim() == "*" {
        return true;
    }
    let weak = |value: &str| value.strip_prefix("W/").unwrap_or(value).to_string();
    let wanted = weak(tag);
    header.split(',').any(|token| weak(token.trim()) == wanted)
}

fn escape_html(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            _ => out.push(c),
        }
    }
    out
}

/// Source `wrapShareHtml` (non-print). The inline style is allowed by a
/// per-response nonce; nothing else may load or run.
fn wrap_share_html(title: &str, inner: &str, nonce: &str) -> String {
    format!(
        "<!DOCTYPE html>\n<html lang=\"ko\">\n<head>\n<meta charset=\"utf-8\"/>\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"/>\n<meta name=\"referrer\" content=\"no-referrer\"/>\n<title>{}</title>\n<style nonce=\"{}\">\nbody{{font-family:\"Noto Sans KR\",\"Noto Sans KR\",system-ui,\"Noto Sans KR\",sans-serif;word-break:keep-all;}}\n\n</style>\n</head>\n<body>\n{inner}\n</body>\n</html>",
        escape_html(title),
        escape_html(nonce),
    )
}

async fn document_body_response(
    state: &AppState,
    headers: &HeaderMap,
    token: &str,
    document_id: Option<Uuid>,
    format: BodyFormat,
) -> Result<Response, AppError> {
    let doc = share_document(&state.auth.db.pool, token, document_id)
        .await
        .map_err(internal)?
        .ok_or_else(not_found)?;
    let tag = strong_etag(&iso(doc.updated_at), format);
    if format != BodyFormat::Html {
        if let Some(header) = headers.get(IF_NONE_MATCH).and_then(|v| v.to_str().ok()) {
            if if_none_matches(header, &tag) {
                let mut out = HeaderMap::new();
                out.insert(
                    ETAG,
                    HeaderValue::from_str(&tag).map_err(|_| AppError::internal())?,
                );
                out.insert(CACHE_CONTROL, HeaderValue::from_static("private, no-cache"));
                return Ok((StatusCode::NOT_MODIFIED, out).into_response());
            }
        }
    }
    let valid = is_tiptap_doc(&doc.content_json);
    let mut out = HeaderMap::new();
    let body = match format {
        BodyFormat::Markdown => {
            out.insert(
                CONTENT_TYPE,
                HeaderValue::from_static("text/markdown; charset=utf-8"),
            );
            if valid {
                tiptap_doc_to_md(&doc.content_json)
            } else {
                String::new()
            }
        }
        BodyFormat::Fragment | BodyFormat::Html => {
            out.insert(
                CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            );
            let inner = if valid {
                tiptap_doc_to_safe_html(&doc.content_json)
            } else {
                String::new()
            };
            if format == BodyFormat::Html {
                let mut raw = [0u8; 16];
                rand::rng().fill_bytes(&mut raw);
                let nonce = URL_SAFE_NO_PAD.encode(raw);
                let csp = format!(
                    "default-src 'none'; style-src 'nonce-{nonce}'; img-src 'self' data: blob:; base-uri 'none'; form-action 'none'; frame-ancestors 'none'"
                );
                out.insert(
                    CONTENT_SECURITY_POLICY,
                    HeaderValue::from_str(&csp).map_err(|_| AppError::internal())?,
                );
                wrap_share_html(&doc.title, &inner, &nonce)
            } else {
                inner
            }
        }
    };
    if format == BodyFormat::Html {
        out.insert(CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    } else {
        // Fragment and markdown are data for the SPA, never a document: a
        // direct navigation gets no script, no subresources and an opaque origin.
        out.insert(
            CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(FRAGMENT_CSP),
        );
        out.insert(CACHE_CONTROL, HeaderValue::from_static("private, no-cache"));
        out.insert(
            ETAG,
            HeaderValue::from_str(&tag).map_err(|_| AppError::internal())?,
        );
    }
    out.insert(
        axum::http::header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    Ok((StatusCode::OK, out, body).into_response())
}

async fn public_body_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path(token): Path<String>,
    query: Result<Query<ShareBodyQuery>, QueryRejection>,
) -> Result<Response, AppError> {
    ensure_sharing_enabled(&state).await?;
    let Query(query) = query.map_err(AppError::from)?;
    let format = parse_format(&query)?;
    enforce_share_limit(&state, peer).await?;
    document_body_response(&state, &headers, &token, None, format).await
}

async fn public_document_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path((token, document_id)): Path<(String, Uuid)>,
    query: Result<Query<ShareBodyQuery>, QueryRejection>,
) -> Result<Response, AppError> {
    ensure_sharing_enabled(&state).await?;
    let Query(query) = query.map_err(AppError::from)?;
    let format = parse_format(&query)?;
    enforce_share_limit(&state, peer).await?;
    document_body_response(&state, &headers, &token, Some(document_id), format).await
}

async fn public_tree_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path(token): Path<String>,
) -> Result<Json<TreeResponse>, AppError> {
    ensure_sharing_enabled(&state).await?;
    enforce_share_limit(&state, peer).await?;
    let nodes = share_tree(&state.auth.db.pool, &token)
        .await
        .map_err(internal)?
        .ok_or_else(not_found)?;
    Ok(Json(TreeResponse {
        items: nodes
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
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharePdfQuery {
    pub document_id: Option<String>,
}

/// Source share `pdf`: the root (or `documentId` inside the visible subtree)
/// rendered by the document convert helper, `${title}.pdf` as an attachment.
/// Same share-ip limit and per-request scope checks as the body route. Without
/// a configured helper the route fails like the member export route (500, logged).
async fn public_pdf_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path(token): Path<String>,
    query: Result<Query<SharePdfQuery>, QueryRejection>,
) -> Result<Response, AppError> {
    ensure_sharing_enabled(&state).await?;
    let Query(query) = query.map_err(AppError::from)?;
    let document_id = match query.document_id.as_deref() {
        None => None,
        Some(raw) => Some(
            parse_body_uuid(raw).ok_or_else(|| AppError::from_code(ProblemCode::InvalidInput))?,
        ),
    };
    enforce_share_limit(&state, peer).await?;
    let doc = share_document(&state.auth.db.pool, &token, document_id)
        .await
        .map_err(internal)?
        .ok_or_else(not_found)?;
    let Some(convert) = state.document_convert.as_ref() else {
        tracing::error!("share pdf requested but FVOCI_DOCUMENT_CONVERT_BIN is unset");
        return Err(AppError::internal());
    };
    let rendered = match render_document_export(
        &convert.for_public(),
        ExportFormat::Pdf,
        &doc.title,
        &doc.content_json,
    )
    .await
    {
        Ok(v) => v,
        Err(ExportRenderError::InvalidInput) => {
            return Err(AppError::from_code(ProblemCode::InvalidInput));
        }
        Err(ExportRenderError::TooLarge) => {
            return Ok(problem_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                "document_body_exceeds_document_max_body_bytes",
                "document body exceeds document max body bytes",
            ));
        }
        Err(ExportRenderError::Busy) => {
            // Anonymous PDFs never wait for a helper; members and import
            // jobs keep their own permits.
            let mut response = problem_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "share_pdf_busy",
                "share pdf busy, retry",
            );
            response.headers_mut().insert(
                axum::http::header::RETRY_AFTER,
                HeaderValue::from_static("5"),
            );
            return Ok(response);
        }
        Err(ExportRenderError::Failed) => return Err(AppError::internal()),
    };
    let filename = export_filename(&doc.title, &rendered.ext);
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    headers.insert(
        CONTENT_DISPOSITION,
        HeaderValue::from_str(&content_disposition_attachment(&filename))
            .map_err(|_| AppError::internal())?,
    );
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/pdf"));
    headers.insert(CONTENT_SECURITY_POLICY, HeaderValue::from_static("sandbox"));
    Ok((StatusCode::OK, headers, rendered.bytes).into_response())
}

fn problem_response(status: StatusCode, code: &str, title: &str) -> Response {
    let body = json!({
        "type": "about:blank",
        "title": title,
        "status": status.as_u16(),
        "code": code,
    });
    let mut headers = HeaderMap::new();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/problem+json"),
    );
    (status, headers, Json(body)).into_response()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShareSearchQuery {
    pub q: Option<String>,
}

fn search_unavailable() -> Response {
    problem_response(
        StatusCode::SERVICE_UNAVAILABLE,
        "search_unavailable",
        "search unavailable",
    )
}

async fn collect_share_hits(
    meili: &crate::search::meili::MeiliConfig,
    q: &str,
    stem: &str,
    scopes: Vec<MeiliSearchScope>,
    document_share: bool,
) -> Result<Vec<MeiliHit>, crate::search::meili::MeiliError> {
    if document_share {
        let mut hits = Vec::new();
        let mut offset = 0u32;
        for _ in 0..SHARE_MEILI_PAGES {
            let page = search_meili(
                meili,
                &MeiliSearchInput {
                    q: q.to_string(),
                    stem: stem.to_string(),
                    scopes: scopes.clone(),
                    kind: Some(SearchSourceKind::Document),
                    limit: SHARE_MEILI_PAGE,
                    offset,
                },
            )
            .await?;
            let empty = page.hits.is_empty();
            hits.extend(page.hits);
            match page.next_offset {
                Some(next) if !empty && hits.len() < SHARE_SEARCH_LIMIT => offset = next,
                _ => break,
            }
        }
        return Ok(hits);
    }
    let docs_input = MeiliSearchInput {
        q: q.to_string(),
        stem: stem.to_string(),
        scopes: scopes.clone(),
        kind: Some(SearchSourceKind::Document),
        limit: SHARE_SEARCH_LIMIT as u32,
        offset: 0,
    };
    let tasks_input = MeiliSearchInput {
        kind: Some(SearchSourceKind::Task),
        ..docs_input.clone()
    };
    let (docs, tasks) = tokio::try_join!(
        search_meili(meili, &docs_input),
        search_meili(meili, &tasks_input)
    )?;
    let mut hits: Vec<MeiliHit> = docs.hits.into_iter().chain(tasks.hits).collect();
    hits.sort_by(|a, b| b.score.total_cmp(&a.score));
    Ok(hits)
}

fn share_snippet(
    title: &str,
    body: &str,
    q: &str,
    chosung: bool,
) -> Option<Vec<SearchSnippetPiece>> {
    if chosung {
        return None;
    }
    let html = highlight_snippet(body, q).or_else(|| highlight_snippet(title, q))?;
    let pieces = parse_snippet(&html);
    let joined: String = pieces.iter().map(|p| p.text.as_str()).collect();
    if joined == title {
        return None;
    }
    Some(
        pieces
            .into_iter()
            .map(|p| SearchSnippetPiece {
                text: p.text,
                r#match: p.r#match,
            })
            .collect(),
    )
}

async fn public_search_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path(token): Path<String>,
    query: Result<Query<ShareSearchQuery>, QueryRejection>,
) -> Result<Response, AppError> {
    ensure_sharing_enabled(&state).await?;
    let Query(query) = query.map_err(AppError::from)?;
    let raw_q = query
        .q
        .ok_or_else(|| AppError::from_code(ProblemCode::InvalidInput))?;
    let len = utf16_len(&raw_q);
    if len == 0 || len > 200 {
        return Err(AppError::from_code(ProblemCode::InvalidInput));
    }
    enforce_share_limit(&state, peer).await?;
    let pool = &state.auth.db.pool;
    let scope = share_search_scope(pool, &token)
        .await
        .map_err(internal)?
        .ok_or_else(not_found)?;
    let workspace_id = scope.share.workspace_id;
    let empty = || -> Response {
        Json(SearchListResponse {
            items: Vec::new(),
            next_cursor: None,
        })
        .into_response()
    };
    let q: String = raw_q.trim().chars().take(200).collect();
    if q.is_empty() {
        return Ok(empty());
    }
    let chosung = is_chosung_query(&q);
    if chosung && q.chars().filter(|c| !c.is_whitespace()).count() < CHOSUNG_MIN_LENGTH {
        return Ok(empty());
    }
    let document_share = scope.share.document_id.is_some();
    let scopes: Vec<MeiliSearchScope> = if document_share {
        scope
            .visible
            .chunks(SHARE_DOC_ID_CHUNK)
            .map(|chunk| MeiliSearchScope {
                workspace_id: workspace_id.to_string(),
                project_ids: Vec::new(),
                include_wiki: false,
                wiki_document_ids: chunk.iter().map(ToString::to_string).collect(),
            })
            .collect()
    } else {
        match scope.share.project_id {
            Some(project_id) => vec![MeiliSearchScope {
                workspace_id: workspace_id.to_string(),
                project_ids: vec![project_id.to_string()],
                include_wiki: false,
                wiki_document_ids: Vec::new(),
            }],
            None => Vec::new(),
        }
    };
    if scopes.is_empty() {
        return Ok(empty());
    }
    let Some(meili) = state.meili.as_ref() else {
        return Ok(search_unavailable());
    };
    let stem = if chosung {
        String::new()
    } else {
        stem_text(&q)
    };
    let hits = match tokio::time::timeout(
        SHARE_SEARCH_DEADLINE,
        collect_share_hits(meili, &q, &stem, scopes, document_share),
    )
    .await
    {
        Ok(Ok(hits)) => hits,
        Ok(Err(err)) => {
            tracing::warn!("share search meili failed: {err:?}");
            return Ok(search_unavailable());
        }
        Err(_) => {
            tracing::warn!("share search timed out");
            return Ok(search_unavailable());
        }
    };
    // The index filter is only recall; PG decides what is visible.
    let mut seen = HashSet::new();
    let mut ordered: Vec<(bool, Uuid, f64)> = Vec::new();
    for hit in &hits {
        let is_task = match hit.kind {
            SearchSourceKind::Document => false,
            SearchSourceKind::Task => true,
            _ => continue,
        };
        if hit.workspace_id != workspace_id.to_string() {
            continue;
        }
        let Ok(id) = Uuid::parse_str(&hit.resource_id) else {
            continue;
        };
        if seen.insert((is_task, id)) {
            ordered.push((is_task, id, hit.score));
        }
    }
    let document_ids: Vec<Uuid> = ordered.iter().filter(|h| !h.0).map(|h| h.1).collect();
    let task_ids: Vec<Uuid> = ordered.iter().filter(|h| h.0).map(|h| h.1).collect();
    let (current, rows) = hydrate_share_hits(pool, &token, &document_ids, &task_ids)
        .await
        .map_err(internal)?
        .ok_or_else(not_found)?;
    let by_key: std::collections::HashMap<(bool, Uuid), ShareSearchRow> =
        rows.into_iter().map(|r| ((r.is_task, r.id), r)).collect();
    let mut items = Vec::new();
    for (is_task, id, score) in ordered {
        if items.len() >= SHARE_SEARCH_LIMIT {
            break;
        }
        let Some(row) = by_key.get(&(is_task, id)) else {
            continue;
        };
        items.push(SearchItemOutput {
            r#type: if is_task { "task" } else { "document" }.to_string(),
            id: row.id.to_string(),
            title: row.title.clone(),
            display_id: None,
            extract_status: None,
            chunk_no: None,
            snippet: share_snippet(&row.title, &row.body, &q, chosung),
            project_id: row.project_id.map(|id| id.to_string()),
            document_id: (!is_task).then(|| row.id.to_string()),
            task_id: is_task.then(|| row.id.to_string()),
            score,
            updated_at: iso(row.updated_at),
            workspace_id: current.workspace_id.to_string(),
        });
    }
    Ok(Json(SearchListResponse {
        items,
        next_cursor: None,
    })
    .into_response())
}

async fn public_attachment_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path((token, attachment_id)): Path<(String, Uuid)>,
) -> Result<Json<AttachmentOutput>, AppError> {
    ensure_sharing_enabled(&state).await?;
    enforce_share_limit(&state, peer).await?;
    let att = share_attachment(&state.auth.db.pool, &token, attachment_id)
        .await
        .map_err(internal)?
        .ok_or_else(not_found)?;
    Ok(Json(AttachmentOutput {
        id: att.id.to_string(),
        name: att.name,
        mime: att.mime,
        size_bytes: att.size_bytes,
        image: att.image,
        scan_status: att.scan_status,
        created_at: att.created_at,
        completed_at: att.completed_at,
        preview: crate::db::attachments::preview_variant_of(&att.variants).map(|p| {
            AttachmentPreviewResponse {
                width: p.width as i32,
                height: p.height as i32,
            }
        }),
    }))
}

/// Source share download: full body only, `application/octet-stream`,
/// `attachment` disposition, sandbox CSP; `variant=preview` serves the
/// published WebP preview (no Range, like the source share route).
async fn public_download_route(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request_headers: HeaderMap,
    Path((token, attachment_id)): Path<(String, Uuid)>,
    query: Result<Query<AttachmentDownloadQuery>, QueryRejection>,
) -> Result<Response, AppError> {
    ensure_sharing_enabled(&state).await?;
    let Query(query) = query.map_err(AppError::from)?;
    let preview = match query.variant.as_deref() {
        None => false,
        Some("preview") => true,
        Some(_) => return Err(AppError::from_code(ProblemCode::InvalidInput)),
    };
    enforce_share_limit(&state, peer).await?;
    let att = share_attachment(&state.auth.db.pool, &token, attachment_id)
        .await
        .map_err(internal)?
        .ok_or_else(not_found)?;
    if preview {
        let variant =
            crate::db::attachments::preview_variant_of(&att.variants).ok_or_else(not_found)?;
        return crate::http::routes::attachments::serve_preview(
            &state,
            &request_headers,
            &variant,
            false,
            false,
        )
        .await;
    }
    let size = att.size_bytes.ok_or_else(AppError::internal)?;
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(CONTENT_SECURITY_POLICY, HeaderValue::from_static("sandbox"));
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    headers.insert(
        CONTENT_DISPOSITION,
        HeaderValue::from_str(&content_disposition_attachment(&att.name))
            .map_err(|_| AppError::internal())?,
    );
    headers.insert(
        CONTENT_LENGTH,
        HeaderValue::from_str(&size.to_string()).map_err(|_| AppError::internal())?,
    );
    if size == 0 {
        return Ok((StatusCode::OK, headers, Body::empty()).into_response());
    }
    let stream = state
        .storage
        .open_payload_stream(&att.storage_key, 0, (size as u64).saturating_sub(1))
        .await
        .map_err(|_| AppError::internal())?;
    Ok((StatusCode::OK, headers, Body::from_stream(stream)).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn etag_matches_source_formula_and_weak_compare() {
        let tag = strong_etag("2026-09-26T00:00:00.000Z", BodyFormat::Fragment);
        let digest = Sha256::digest(b"2026-09-26T00:00:00.000Z|fragment");
        assert_eq!(tag, format!("\"{}\"", &hex::encode(digest)[..16]));
        assert!(if_none_matches(&format!("W/{tag}"), &tag));
        assert!(if_none_matches(&format!("\"x\", {tag}"), &tag));
        assert!(if_none_matches("*", &tag));
        assert!(!if_none_matches("\"other\"", &tag));
    }

    #[test]
    fn wrapper_escapes_title() {
        let html = wrap_share_html("<a>&\"", "<p>x</p>", "n1");
        assert!(html.contains("<title>&lt;a&gt;&amp;&quot;</title>"));
        assert!(html.contains("<style nonce=\"n1\">"));
        assert!(html.contains("<body>\n<p>x</p>\n</body>"));
    }

    #[test]
    fn expires_days_is_integer_in_range() {
        assert_eq!(parse_expires_days(None).unwrap(), None);
        assert_eq!(parse_expires_days(Some(7.0)).unwrap(), Some(7));
        assert!(parse_expires_days(Some(0.0)).is_err());
        assert!(parse_expires_days(Some(366.0)).is_err());
        assert!(parse_expires_days(Some(1.5)).is_err());
    }

    #[test]
    fn share_url_uses_human_path() {
        assert_eq!(
            share_page_url("https://x.example/", "tok"),
            "https://x.example/s/tok"
        );
    }
}
