use std::future::Future;
use std::path::{Component, Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll};

use std::net::SocketAddr;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{header, Method, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use tower::{Service, ServiceExt};
use tower_http::services::ServeFile;

use crate::error::{AppError, ProblemCode};
use crate::http::spa_head::{inject_share_og, share_shell_token, ShareOgMeta};
use crate::http::state::AppState;

pub fn validate_static_root(path: &Path) -> Result<PathBuf, String> {
    let canonical = path
        .canonicalize()
        .map_err(|e| format!("invalid FVOCI_STATIC_DIR: {e}"))?;
    if !canonical.is_dir() {
        return Err("FVOCI_STATIC_DIR must be a directory".into());
    }
    resolve_static_index(&canonical)?;
    Ok(canonical)
}

pub fn resolve_static_index(root: &Path) -> Result<PathBuf, String> {
    let index = root.join("index.html");
    let canonical = index
        .canonicalize()
        .map_err(|e| format!("missing index.html in FVOCI_STATIC_DIR: {e}"))?;
    if !canonical.is_file() {
        return Err("index.html must be a regular file".into());
    }
    let root_canonical = root
        .canonicalize()
        .map_err(|e| format!("invalid FVOCI_STATIC_DIR: {e}"))?;
    if !canonical.starts_with(&root_canonical) {
        return Err("index.html resolves outside FVOCI_STATIC_DIR".into());
    }
    Ok(canonical)
}

pub fn static_router(root: PathBuf) -> axum::Router {
    static_fallback(root, None)
}

/// `static_router` whose `/s/{token}` shell carries the share's title and
/// Open Graph tags (source server.ts SPA fallback).
pub fn static_router_with_share_head(root: PathBuf, state: AppState) -> axum::Router {
    static_fallback(root, Some(state))
}

fn static_fallback(root: PathBuf, share_head: Option<AppState>) -> axum::Router {
    let root = root
        .canonicalize()
        .expect("static root must exist when serving assets");
    let index = resolve_static_index(&root).expect("static index must exist when serving assets");
    axum::Router::new().fallback_service(StaticFallback {
        root,
        index,
        share_head,
    })
}

#[derive(Clone)]
struct StaticFallback {
    root: PathBuf,
    index: PathBuf,
    share_head: Option<AppState>,
}

impl Service<Request<Body>> for StaticFallback {
    type Response = Response;
    type Error = std::convert::Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        let root = self.root.clone();
        let index = self.index.clone();
        let share_head = self.share_head.clone();
        Box::pin(async move {
            if let Some(state) = share_head {
                if req.method() == Method::GET {
                    if let Some(token) = share_shell_token(req.uri().path()) {
                        let token = token.to_string();
                        let peer = req
                            .extensions()
                            .get::<ConnectInfo<SocketAddr>>()
                            .map(|info| info.0);
                        let mut response = share_shell(&state, peer, &token, &index).await;
                        response.headers_mut().insert(
                            header::REFERRER_POLICY,
                            header::HeaderValue::from_static("no-referrer"),
                        );
                        return Ok(response);
                    }
                }
            }
            Ok(serve_static(req, root, index).await)
        })
    }
}

/// Every shell/asset response sends `Referrer-Policy: no-referrer` (source
/// global security headers): `/s/{token}` and invite paths carry secrets.
async fn serve_static(req: Request<Body>, root: PathBuf, index: PathBuf) -> Response {
    let mut response = serve_static_file(req, root, index).await;
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        header::HeaderValue::from_static("no-referrer"),
    );
    response
}

async fn serve_static_file(req: Request<Body>, root: PathBuf, index: PathBuf) -> Response {
    let path = req.uri().path();
    if path.starts_with(crate::error::API_PREFIX) {
        return unknown_api_fallback(req).await;
    }
    if !is_safe_static_path(path) {
        return StatusCode::NOT_FOUND.into_response();
    }

    if let Some(file) = resolve_static_file(&root, path) {
        let is_index = file == index;
        return match ServeFile::new(file).oneshot(req).await {
            Ok(response) => {
                let mut response = response.map(Body::new);
                if is_index {
                    no_cache_headers(&mut response);
                }
                response
            }
            Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };
    }

    let rel = path.trim_start_matches('/');
    if rel.starts_with("assets/") {
        return StatusCode::NOT_FOUND.into_response();
    }
    if !rel.is_empty() && Path::new(rel).extension().is_some() {
        return StatusCode::NOT_FOUND.into_response();
    }

    match ServeFile::new(index).oneshot(req).await {
        Ok(response) => {
            let mut response = response.map(Body::new);
            no_cache_headers(&mut response);
            response
        }
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Source server.ts `/s/:token` shell: never cached (`private, no-store`),
/// `x-robots-tag: noindex`, and the share's head tags when the token resolves
/// while sharing is enabled and the caller is inside the share-ip limit. An
/// invalid, expired, revoked or rate-limited token gets the plain shell, so
/// the head never tells whether a token exists.
async fn share_shell(
    state: &AppState,
    peer: Option<SocketAddr>,
    token: &str,
    index: &Path,
) -> Response {
    let html = match tokio::fs::read_to_string(index).await {
        Ok(html) => html,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };
    let meta = match crate::http::routes::share::shell_head_meta(state, peer, token).await {
        Ok(meta) => meta,
        Err(err) => return err.into_response(),
    };
    let html = match meta {
        Some(meta) => inject_share_og(
            &html,
            &ShareOgMeta {
                title: &meta.title,
                excerpt: &meta.excerpt,
                url: &crate::http::routes::share::share_page_url(&state.public_origin, token),
            },
        ),
        None => html,
    };
    let mut response = Response::new(Body::from(html));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("text/html; charset=utf-8"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("private, no-store"),
    );
    headers.insert(
        header::HeaderName::from_static("x-robots-tag"),
        header::HeaderValue::from_static("noindex"),
    );
    response
}

fn resolve_static_file(root: &Path, uri_path: &str) -> Option<PathBuf> {
    let rel = uri_path.trim_start_matches('/');
    if rel.is_empty() {
        return None;
    }
    let candidate = root.join(rel);
    let canonical = candidate.canonicalize().ok()?;
    let root_canonical = root.canonicalize().ok()?;
    if !canonical.starts_with(&root_canonical) {
        return None;
    }
    canonical.is_file().then_some(canonical)
}

pub async fn unknown_api_fallback(req: Request<Body>) -> Response {
    if req.uri().path().starts_with(crate::error::API_PREFIX) {
        return AppError::from_code(ProblemCode::NotFound).into_response();
    }
    StatusCode::NOT_FOUND.into_response()
}

pub fn is_safe_static_path(uri_path: &str) -> bool {
    let path = uri_path.trim_start_matches('/');
    if path.is_empty() {
        return true;
    }
    let parsed = Path::new(path);
    for component in parsed.components() {
        match component {
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return false,
            Component::Normal(part) if part.to_string_lossy().starts_with('.') => return false,
            _ => {}
        }
    }
    true
}

pub fn no_cache_headers(response: &mut Response) {
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
}
