//! Serving the web app: the files under `static/` and the `index.html` shell.
//!
//! Every path that is neither an API route nor a static file gets `index.html`, so that
//! the web app's client-side routes can be opened directly. Cache headers are set so that
//! the server can sit behind a CDN.

mod discovery;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::Router;
use axum::body::Body;
use axum::extract::{OriginalUri, State};
use axum::http::header::{
    ACCESS_CONTROL_ALLOW_ORIGIN, ALLOW, CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE,
    HeaderName, REFERRER_POLICY, STRICT_TRANSPORT_SECURITY, X_CONTENT_TYPE_OPTIONS,
    X_FRAME_OPTIONS,
};
use axum::http::{HeaderValue, Method, Request, StatusCode, Uri};
use axum::middleware;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use serde::Deserialize;
use tower::ServiceExt as _;
use tower_http::services::ServeDir;

pub(crate) use discovery::{origin_routes, send_robots_tag};

use crate::config::SecurityContact;
use crate::error::AppError;
use discovery::{SecurityTxt, https_origin_of};

/// The body of the answer to a path that names nothing.
const NOT_FOUND_BODY: &str = "not found\n";

/// The placeholder in `index.html` that is replaced with the base URL.
const BASE_URL_PLACEHOLDER: &str = "{{.BaseURL}}";

/// The policy of every HTML document under `static/`.
///
/// The one such document is the web app's file viewer frame. The app frames it with
/// `sandbox="allow-scripts"`; this sandboxes it the same way when it is opened on its
/// own, so that it never runs with the app's origin.
const STATIC_DOCUMENT_POLICY: &str = "sandbox allow-scripts";

/// The policy of `index.html` if even the strict policy without hashes could not be made
/// a header (it always can): nothing loads, so the page fails where it is seen.
const INDEX_DOCUMENT_POLICY: &str = "default-src 'none'; frame-ancestors 'none'; base-uri 'none'";

/// Baseline browser capability lockdown sent on every response.
const PERMISSIONS_POLICY_VALUE: &str =
    "camera=(), microphone=(), geolocation=(), payment=(), usb=()";

/// Build-generated inline script hash manifest (`dist/csp.json`).
#[derive(Deserialize)]
struct CspManifest {
    #[serde(rename = "script-src")]
    script_src: Vec<String>,
}

/// One-year `Strict-Transport-Security` policy sent when `server_url` uses HTTPS.
const HSTS_POLICY: &str = "max-age=31536000; includeSubDomains";

/// Whether responses carry `Strict-Transport-Security` (enabled when `server_url` starts
/// with `https://`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransportSecurity {
    /// `server_url` uses `http://`: do not send `Strict-Transport-Security`.
    PlainHttp,
    /// `server_url` uses `https://`: send `Strict-Transport-Security`.
    Https,
}

/// The web app directory (`web_app_path`) and how it is served.
#[derive(Debug, Clone)]
pub struct WebRoot {
    root: PathBuf,
    base_url: String,
    https_origin: Option<String>,
    security_txt: Option<SecurityTxt>,
    logged_missing_csp: Arc<AtomicBool>,
}

impl WebRoot {
    /// Creates the web root for the `web_app_path` and `server_url` settings.
    pub fn new(web_app_path: impl AsRef<Path>, server_url: &str) -> Self {
        Self {
            root: web_app_path.as_ref().to_path_buf(),
            base_url: base_url(server_url),
            https_origin: https_origin_of(server_url),
            security_txt: None,
            logged_missing_csp: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Configures the `/.well-known/security.txt` response for this web root.
    #[must_use]
    pub(crate) fn with_security_contact(mut self, contact: Option<SecurityContact>) -> Self {
        self.security_txt =
            contact.map(|contact| SecurityTxt::new(contact, self.https_origin.as_deref()));
        self
    }

    /// The `/.well-known/security.txt` policy, or `None` when `security_contact` is unset.
    pub(crate) fn security_txt(&self) -> Option<&SecurityTxt> {
        self.security_txt.as_ref()
    }

    /// The path the web app is served under: the path part of `server_url`.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Whether `server_url` uses `https://` and responses should carry HSTS.
    pub(crate) fn transport_security(&self) -> TransportSecurity {
        if self.https_origin.is_some() {
            TransportSecurity::Https
        } else {
            TransportSecurity::PlainHttp
        }
    }

    /// Serves the file `path` names under `static/`.
    ///
    /// `path` is the request path after `/static/`. Responses carry `Cache-Control`:
    /// a file whose name contains a content hash may be cached forever, any other file
    /// must be revalidated. A `.zst`, `.br` or `.gz` file next to the requested one is
    /// served instead when the client accepts that encoding (the one it prefers; zstd,
    /// then brotli, then gzip on a tie).
    ///
    /// Any origin may read these files: the file viewer's frame has an opaque origin and
    /// loads the app's scripts, style sheets and fonts across origins, and they are the
    /// public build, sent without credentials. An HTML document is sandboxed (it is the
    /// viewer's frame); whether it is one is read from the type the file service gave
    /// it, not from the requested name, which may be percent-encoded.
    pub async fn serve_static(&self, path: &str, request: Request<Body>) -> Response {
        let Ok(uri) = format!("/{path}").parse::<Uri>() else {
            return not_found();
        };
        let (mut parts, body) = request.into_parts();
        parts.uri = uri;

        let files = ServeDir::new(self.root.join("static"))
            .append_index_html_on_directories(false)
            .precompressed_zstd()
            .precompressed_br()
            .precompressed_gzip();
        let mut response = match files.oneshot(Request::from_parts(parts, body)).await {
            Ok(response) => response.map(Body::new),
            Err(never) => match never {},
        };
        if response.status() == StatusCode::NOT_FOUND {
            return not_found();
        }

        let file_name = path.rsplit('/').next().unwrap_or(path);
        let headers = response.headers_mut();
        headers.insert(ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
        let active_document = headers
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(is_sandboxed_static_mime);
        if active_document {
            headers.insert(
                CONTENT_SECURITY_POLICY,
                HeaderValue::from_static(STATIC_DOCUMENT_POLICY),
            );
        }

        let cacheable = matches!(
            response.status(),
            StatusCode::OK | StatusCode::PARTIAL_CONTENT | StatusCode::NOT_MODIFIED
        );
        if cacheable {
            let policy = if has_content_hash(file_name) {
                "public, max-age=31536000, immutable"
            } else {
                "no-cache"
            };
            response
                .headers_mut()
                .insert(CACHE_CONTROL, HeaderValue::from_static(policy));
        }
        response
    }

    /// Serves `index.html` with the base URL filled in.
    ///
    /// The file and `csp.json` are read on every request, so a new build of the web app
    /// is picked up without a restart, and the response must be revalidated by caches.
    pub async fn serve_index(&self) -> Response {
        match tokio::fs::read_to_string(self.root.join("index.html")).await {
            Ok(template) => {
                let csp = self.index_csp_header().await;
                (
                    [
                        (
                            CONTENT_TYPE,
                            HeaderValue::from_static("text/html; charset=utf-8"),
                        ),
                        (CACHE_CONTROL, HeaderValue::from_static("no-cache")),
                        (X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")),
                        (X_FRAME_OPTIONS, HeaderValue::from_static("DENY")),
                        (CONTENT_SECURITY_POLICY, csp),
                    ],
                    template.replace(BASE_URL_PLACEHOLDER, &self.base_url),
                )
                    .into_response()
            }
            Err(error) => {
                tracing::error!(%error, root = %self.root.display(), "unable to serve the index.html file");
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            }
        }
    }

    async fn index_csp_header(&self) -> HeaderValue {
        let csp_path = self.root.join("csp.json");
        match tokio::fs::read_to_string(&csp_path).await {
            Ok(raw) => {
                if let Some(policy) = serde_json::from_str::<CspManifest>(&raw)
                    .ok()
                    .and_then(|manifest| strict_index_csp(&manifest.script_src))
                {
                    self.logged_missing_csp.store(false, Ordering::Relaxed);
                    policy
                } else {
                    if !self.logged_missing_csp.swap(true, Ordering::Relaxed) {
                        tracing::error!(
                            path = %csp_path.display(),
                            "csp.json is malformed; the index is served with no inline script allowed"
                        );
                    }
                    index_csp_without_hashes()
                }
            }
            Err(error) => {
                if !self.logged_missing_csp.swap(true, Ordering::Relaxed) {
                    tracing::error!(
                        %error,
                        path = %csp_path.display(),
                        "csp.json could not be read; the index is served with no inline script allowed"
                    );
                }
                index_csp_without_hashes()
            }
        }
    }
}

/// Whether `content_type` names an active document type under `/static/` that must carry
/// `Content-Security-Policy: sandbox allow-scripts` when opened directly.
fn is_sandboxed_static_mime(content_type: &str) -> bool {
    let media_type = content_type.split(';').next().map_or("", str::trim);
    media_type.eq_ignore_ascii_case("text/html")
        || media_type.eq_ignore_ascii_case("image/svg+xml")
        || media_type.eq_ignore_ascii_case("application/xhtml+xml")
        || media_type.eq_ignore_ascii_case("text/xml")
        || media_type.eq_ignore_ascii_case("application/xml")
}

/// Validates a `'sha256-...'` source expression from `csp.json`.
fn is_valid_csp_hash_source(token: &str) -> bool {
    let Some(digest) = token
        .strip_prefix("'sha256-")
        .and_then(|rest| rest.strip_suffix('\''))
    else {
        return false;
    };
    !digest.is_empty()
        && digest.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'=' | b'-' | b'_')
        })
}

/// The policy of `index.html` when `csp.json` is not there or not as the build writes
/// it: the strict policy with no hash, closed rather than open. The inline start script
/// is refused and the app does not start, which is seen at once, where a policy without
/// `script-src` would let any injected script run unnoticed.
fn index_csp_without_hashes() -> HeaderValue {
    strict_index_csp(&[]).unwrap_or_else(|| HeaderValue::from_static(INDEX_DOCUMENT_POLICY))
}

/// Builds the strict production Content-Security-Policy matching the E2E/preview server.
fn strict_index_csp(script_hashes: &[String]) -> Option<HeaderValue> {
    let mut script_src = String::from("script-src 'self'");
    for hash in script_hashes {
        if !is_valid_csp_hash_source(hash) {
            return None;
        }
        script_src.push(' ');
        script_src.push_str(hash);
    }
    let policy = [
        "default-src 'self'",
        script_src.as_str(),
        "style-src 'self' 'unsafe-inline'",
        "img-src 'self' blob:",
        "font-src 'self'",
        "connect-src 'self'",
        "object-src 'none'",
        "base-uri 'none'",
        "form-action 'self'",
        "frame-ancestors 'none'",
    ]
    .join("; ");
    HeaderValue::from_str(&policy).ok()
}

/// Applies baseline security headers (`Referrer-Policy`, `X-Content-Type-Options`,
/// `Permissions-Policy`, `Cross-Origin-Opener-Policy`, `Cross-Origin-Resource-Policy` on
/// non-`/static/` routes, and `Strict-Transport-Security` when `server_url` uses HTTPS)
/// to every response without duplicating headers set by inner handlers.
pub(crate) async fn send_security_headers(
    State(transport): State<TransportSecurity>,
    request: Request<Body>,
    next: middleware::Next,
) -> Response {
    let path = request.uri().path();
    let is_static = path == "/static" || path.starts_with("/static/");
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers
        .entry(REFERRER_POLICY)
        .or_insert(HeaderValue::from_static("no-referrer"));
    headers
        .entry(X_CONTENT_TYPE_OPTIONS)
        .or_insert(HeaderValue::from_static("nosniff"));
    headers
        .entry(HeaderName::from_static("permissions-policy"))
        .or_insert(HeaderValue::from_static(PERMISSIONS_POLICY_VALUE));
    headers
        .entry(HeaderName::from_static("cross-origin-opener-policy"))
        .or_insert(HeaderValue::from_static("same-origin"));
    if !is_static {
        headers
            .entry(HeaderName::from_static("cross-origin-resource-policy"))
            .or_insert(HeaderValue::from_static("same-origin"));
    }
    if transport == TransportSecurity::Https {
        headers
            .entry(STRICT_TRANSPORT_SECURITY)
            .or_insert(HeaderValue::from_static(HSTS_POLICY));
    }
    response
}

/// The routes of the web app, served from `web`: its files under `/static/`, and its page
/// for every path that no other route takes ([`handle_fallback`]). The fallback is this
/// router's: merged into one that has its own, it would panic.
pub(crate) fn app_routes() -> Router<Arc<WebRoot>> {
    Router::new()
        .route("/static/{*path}", get(handle_static))
        .fallback(handle_fallback)
}

/// Serves a static file under `/static/`.
async fn handle_static(State(web): State<Arc<WebRoot>>, request: Request<Body>) -> Response {
    // The path as the client sent it, still percent-encoded: the file service decodes
    // it, once.
    let path = request
        .uri()
        .path()
        .strip_prefix("/static/")
        .unwrap_or_default()
        .to_owned();
    web.serve_static(&path, request).await
}

/// The page that starts the web app, for its root.
pub(crate) async fn handle_root(State(web): State<Arc<WebRoot>>) -> Response {
    web.serve_index().await
}

/// Everything that is neither an API route nor a static file is a route of the web app:
/// the page that starts it is the answer to a `GET`.
///
/// A path that ends in a slash is redirected to the one without, so that every screen
/// has one address. The root is the exception; with a base path it is the base path
/// itself, with or without the slash.
async fn handle_fallback(
    State(web): State<Arc<WebRoot>>,
    method: Method,
    uri: Uri,
    OriginalUri(original): OriginalUri,
) -> Response {
    let path = uri.path();
    if path == "/api" || path.starts_with("/api/") {
        return AppError::not_found("route").into_response();
    }
    if path == "/static" || path.starts_with("/static/") {
        return not_found().into_response();
    }
    if method != Method::GET && method != Method::HEAD {
        let mut response = AppError::method_not_allowed().into_response();
        response
            .headers_mut()
            .insert(ALLOW, HeaderValue::from_static("GET, HEAD"));
        return response;
    }
    if path.len() > 1 && path.ends_with('/') {
        return match without_trailing_slash(&original) {
            Some(location) => Redirect::permanent(&location).into_response(),
            None => not_found().into_response(),
        };
    }
    web.serve_index().await
}

/// The address a path with a trailing slash is redirected to: the same path without
/// the slash, with its query.
///
/// `None` for a path with an empty segment or a backslash, such as `//host/` and
/// `/\host/`: without its last slash it would read as an address on another host.
fn without_trailing_slash(uri: &Uri) -> Option<String> {
    let path = uri.path().trim_end_matches('/');
    // Browsers read a backslash as a slash, so `/\host` is `//host` to them.
    if path.is_empty() || path.contains("//") || path.contains('\\') {
        return None;
    }
    Some(match uri.query() {
        Some(query) => format!("{path}?{query}"),
        None => path.to_owned(),
    })
}

/// The plain-text 404 response for an unmatched path.
pub fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        [
            (
                CONTENT_TYPE,
                HeaderValue::from_static("text/plain; charset=utf-8"),
            ),
            (X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")),
        ],
        NOT_FOUND_BODY,
    )
        .into_response()
}

/// Returns the path part of the `server_url` URL, which is the base URL of the web app.
///
/// The path is taken as it is: a trailing slash stays, and a URL without a path gives the
/// empty string. A `server_url` that is not a URL also gives the empty string.
pub fn base_url(server_url: &str) -> String {
    let Some((_, rest)) = server_url.split_once("://") else {
        return String::new();
    };
    let end = rest.find(['?', '#']).unwrap_or(rest.len());
    match rest[..end].find('/') {
        Some(start) => rest[start..end].to_owned(),
        None => String::new(),
    }
}

/// Returns `true` if a file name carries a hash of the file's content, as build tools
/// produce (`main-0123456789abcdef.js`), so the file never changes under that name.
fn has_content_hash(file_name: &str) -> bool {
    file_name.split(['-', '.', '_']).any(|part| {
        part.len() >= 8
            && part.bytes().all(|byte| byte.is_ascii_hexdigit())
            && part.bytes().any(|byte| byte.is_ascii_digit())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Extracts the base path from `server_url`, falling back to `/` for invalid URLs.
    #[test]
    fn base_url_is_the_path_of_the_server_url() {
        let cases = [
            ("https://some-fake-server.com/fake-url", "/fake-url"),
            ("https://localhost:8080/fake-url", "/fake-url"),
            // "should return '/' as base url is not good!"
            ("https://localhost:8080/#!@$@#@", "/"),
        ];
        for (server_url, want) in cases {
            assert_eq!(base_url(server_url), want, "server_url {server_url}");
        }
    }

    #[test]
    fn base_url_edge_cases() {
        assert_eq!(base_url("http://localhost:8000"), "");
        assert_eq!(base_url("http://localhost:8000/"), "/");
        assert_eq!(base_url("http://host/boards/sub?x=1"), "/boards/sub");
        assert_eq!(base_url("not a url"), "");
    }

    #[test]
    fn content_hash_detection() {
        for hashed in [
            "main-0123456789abcdef.js",
            "app-1a2b3c4d5e6f7a8b_bg.wasm",
            "style.9f86d081884c7d65.css",
        ] {
            assert!(has_content_hash(hashed), "{hashed} has a content hash");
        }
        for plain in [
            "main.js",
            "favicon.svg",
            "Metropolis-Regular.woff2",
            "feedface.png",
            "a-1234567.js",
        ] {
            assert!(!has_content_hash(plain), "{plain} has no content hash");
        }
    }
}
