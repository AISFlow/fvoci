use std::future::Future;
use std::path::{Component, Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll};

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use tower::{Service, ServiceExt};
use tower_http::services::ServeFile;

use crate::error::{AppError, ProblemCode};

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
    let root = root
        .canonicalize()
        .expect("static root must exist when serving assets");
    let index = resolve_static_index(&root).expect("static index must exist when serving assets");
    axum::Router::new().fallback_service(StaticFallback { root, index })
}

#[derive(Clone)]
struct StaticFallback {
    root: PathBuf,
    index: PathBuf,
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
        Box::pin(async move { Ok(serve_static(req, root, index).await) })
    }
}

async fn serve_static(req: Request<Body>, root: PathBuf, index: PathBuf) -> Response {
    let path = req.uri().path();
    if path.starts_with(crate::error::API_PREFIX) {
        return unknown_api_fallback(req).await;
    }
    if !is_safe_static_path(path) {
        return StatusCode::NOT_FOUND.into_response();
    }

    if let Some(file) = resolve_static_file(&root, path) {
        return match ServeFile::new(file).oneshot(req).await {
            Ok(response) => response.map(Body::new),
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
        Ok(response) => response.map(Body::new),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
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
