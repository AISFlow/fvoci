//! Swagger UI and the OpenAPI document (source `platform/api-docs.ts`).
//!
//! Every route is session-only like the source guard
//! (`access: { auth: "session", body: "none" }`): anonymous requests get 401
//! `authentication_required`, API tokens 404 `not_found`. The JSON is the
//! checked-in `apps/web/openapi.json`, which `api::openapi::tests` pins to the
//! live `spec_json()` export, so the served document cannot drift from the
//! generator. Swagger UI is the vendored `swagger-ui-dist` in `api_docs_assets/`
//! (no CDN, no Node at runtime); the page's inline blocks carry a per-request
//! CSP nonce like the source's `HTMLRewriter` pass.

use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use axum_extra::extract::CookieJar;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use rand::RngCore;

use crate::error::AppError;
use crate::http::authz::{require_request_auth, Access};
use crate::http::security_headers::nonce_policy;
use crate::http::state::AppState;

pub const OPENAPI_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/apps/web/openapi.json"
));

pub const SWAGGER_UI_VERSION: &str = "5.33.0";
pub const SWAGGER_UI_BUNDLE: &[u8] = include_bytes!("../api_docs_assets/swagger-ui-bundle.js");
pub const SWAGGER_UI_CSS: &[u8] = include_bytes!("../api_docs_assets/swagger-ui.css");

/// The asset URLs carry the vendored version, so a year-long private cache
/// never pins an old bundle after an upgrade.
const ASSET_CACHE_CONTROL: &str = "private, max-age=31536000, immutable";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/docs", get(get_page))
        .route("/api/docs/json", get(get_json))
        .route("/api/docs/static/swagger-ui-bundle.js", get(get_bundle))
        .route("/api/docs/static/swagger-ui.css", get(get_css))
}

async fn require_session(
    state: &AppState,
    headers: &HeaderMap,
    jar: &CookieJar,
) -> Result<(), AppError> {
    require_request_auth(state, headers, jar, Access::Session, None)
        .await
        .map(|_| ())
}

/// Source `SwaggerUIRender` output (title/description `FVOCI API`, dark-mode
/// block, local bundle and theme) with `nonce` on both inline blocks.
pub fn page_html(nonce: &str) -> String {
    let v = SWAGGER_UI_VERSION;
    format!(
        r##"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>FVOCI API</title>
    <meta name="description" content="FVOCI API" />
    <meta name="og:description" content="FVOCI API" />
    <style nonce="{nonce}">
@media (prefers-color-scheme: dark) {{
    body {{
        background-color: #222;
    }}
    .swagger-ui {{
        filter: invert(92%) hue-rotate(180deg);
    }}

    .swagger-ui .microlight {{
        filter: invert(100%) hue-rotate(180deg);
    }}
}}
</style>
    <link rel="stylesheet" href="/api/docs/static/swagger-ui.css?v={v}" />
</head>
<body>
    <div id="swagger-ui"></div>
    <script src="/api/docs/static/swagger-ui-bundle.js?v={v}" crossorigin></script>
    <script nonce="{nonce}">
        window.onload = () => {{
            window.ui = SwaggerUIBundle({{"dom_id":"#swagger-ui","url":"/api/docs/json"}});
        }};
    </script>
</body>
</html>"##
    )
}

async fn get_page(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Response, AppError> {
    require_session(&state, &headers, &jar).await?;
    let mut bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut bytes);
    let nonce = STANDARD.encode(bytes);
    let csp = HeaderValue::from_str(&nonce_policy(&state.public_origin, &nonce))
        .map_err(|_| AppError::internal())?;
    Ok((
        StatusCode::OK,
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            ),
            // The nonce is single-use: a stored copy must not be replayed.
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
            (header::CONTENT_SECURITY_POLICY, csp),
        ],
        page_html(&nonce),
    )
        .into_response())
}

async fn get_json(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Response, AppError> {
    require_session(&state, &headers, &jar).await?;
    Ok(([(header::CONTENT_TYPE, "application/json")], OPENAPI_JSON).into_response())
}

async fn get_bundle(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Response, AppError> {
    require_session(&state, &headers, &jar).await?;
    Ok(asset("text/javascript; charset=utf-8", SWAGGER_UI_BUNDLE))
}

async fn get_css(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Response, AppError> {
    require_session(&state, &headers, &jar).await?;
    Ok(asset("text/css; charset=utf-8", SWAGGER_UI_CSS))
}

fn asset(content_type: &'static str, body: &'static [u8]) -> Response {
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, ASSET_CACHE_CONTROL),
        ],
        body,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    #[test]
    fn vendored_swagger_ui_matches_pinned_release() {
        // swagger-ui-dist 5.33.0 (api_docs_assets/README.md).
        assert_eq!(
            hex::encode(Sha256::digest(SWAGGER_UI_BUNDLE)),
            "62df541529080464a7660adc793eab7128c6193ce3be24ddc1e0e0a4a63edc2f"
        );
        assert_eq!(
            hex::encode(Sha256::digest(SWAGGER_UI_CSS)),
            "1ac324f7dcd27e4b9386b4bd6421271ec147e922a22c05ba24b11515e9aa6321"
        );
    }

    #[test]
    fn page_inline_blocks_all_carry_the_nonce() {
        let html = page_html("TESTNONCE");
        let inline = |tag: &str| {
            let open = format!("<{tag}");
            html.match_indices(&open)
                .map(|(i, _)| &html[i..i + html[i..].find('>').unwrap()])
                .filter(|t| !t.contains("src="))
                .collect::<Vec<_>>()
        };
        for tag in ["script", "style"] {
            let blocks = inline(tag);
            assert_eq!(blocks.len(), 1, "{tag}");
            assert!(blocks[0].contains(r#"nonce="TESTNONCE""#), "{tag}");
        }
        assert!(!html.contains("https://"), "no CDN reference");
        let csp = nonce_policy("https://fvoci.example", "TESTNONCE");
        assert!(csp.contains("script-src 'self' 'wasm-unsafe-eval' 'nonce-TESTNONCE';"));
        assert!(csp.contains("style-src 'self' 'nonce-TESTNONCE';"));
        assert!(!csp.contains("unsafe-inline"));
    }
}
