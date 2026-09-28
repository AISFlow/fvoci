//! Swagger UI and the OpenAPI document (source `platform/api-docs.ts`).
//!
//! Two responsibilities, both session-only like the source guard
//! (`access: { auth: "session", body: "none" }`): anonymous requests get 401
//! `authentication_required`, API tokens 404 `not_found`, and the global
//! consent gate and security headers apply as to every API route.
//!
//! - [`ui_router`]: the Swagger UI page and its same-origin assets, all fixed
//!   bytes embedded from `api_docs_assets/`: the vendored `swagger-ui-dist`
//!   (no CDN, no Node at runtime) plus FVOCI's static page, initializer and
//!   dark-mode theme. The page has no inline blocks, so it runs under the
//!   application CSP unchanged.
//! - `/api/docs/json`: the checked-in `apps/web/openapi.json`, which
//!   `api::openapi::tests` pins to the live `spec_json()` export, so the
//!   served document cannot drift from the generator.

use axum::extract::State;
use axum::http::{header, HeaderMap};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use axum_extra::extract::CookieJar;

use crate::error::AppError;
use crate::http::authz::{require_request_auth, Access};
use crate::http::state::AppState;

pub const OPENAPI_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/apps/web/openapi.json"
));

pub const SWAGGER_UI_VERSION: &str = "5.33.0";
pub const SWAGGER_UI_BUNDLE: &[u8] = include_bytes!("../api_docs_assets/swagger-ui-bundle.js");
pub const SWAGGER_UI_CSS: &[u8] = include_bytes!("../api_docs_assets/swagger-ui.css");
/// Third-party notices the bundle's `/*! For license information … */` banner
/// points at (relative to the bundle URL).
pub const SWAGGER_UI_BUNDLE_LICENSE: &[u8] =
    include_bytes!("../api_docs_assets/swagger-ui-bundle.js.LICENSE.txt");

/// Source `SwaggerUIRender` output (title/description `FVOCI API`, local
/// bundle and theme) with the inline dark-mode block and initializer moved to
/// the two files below.
pub const PAGE_HTML: &[u8] = include_bytes!("../api_docs_assets/fvoci-api-docs.html");
pub const INITIALIZER_JS: &[u8] = include_bytes!("../api_docs_assets/fvoci-swagger-initializer.js");
pub const THEME_CSS: &[u8] = include_bytes!("../api_docs_assets/fvoci-swagger-theme.css");

/// The vendored URLs carry `?v=SWAGGER_UI_VERSION`, so a year-long private
/// cache never pins an old bundle after an upgrade.
const VENDORED_CACHE_CONTROL: &str = "private, max-age=31536000, immutable";
/// FVOCI's own page assets have unversioned URLs; they are revalidated (a few
/// hundred bytes) so an edit takes effect on the next load.
const REVALIDATE_CACHE_CONTROL: &str = "private, no-cache";
const NO_STORE: &str = "no-store";

/// One fixed response of the docs UI.
struct Asset {
    path: &'static str,
    content_type: &'static str,
    cache_control: &'static str,
    body: &'static [u8],
}

const UI_ASSETS: [Asset; 6] = [
    Asset {
        path: "/api/docs",
        content_type: "text/html; charset=utf-8",
        cache_control: NO_STORE,
        body: PAGE_HTML,
    },
    Asset {
        path: "/api/docs/static/fvoci-swagger-initializer.js",
        content_type: "text/javascript; charset=utf-8",
        cache_control: REVALIDATE_CACHE_CONTROL,
        body: INITIALIZER_JS,
    },
    Asset {
        path: "/api/docs/static/fvoci-swagger-theme.css",
        content_type: "text/css; charset=utf-8",
        cache_control: REVALIDATE_CACHE_CONTROL,
        body: THEME_CSS,
    },
    Asset {
        path: "/api/docs/static/swagger-ui-bundle.js",
        content_type: "text/javascript; charset=utf-8",
        cache_control: VENDORED_CACHE_CONTROL,
        body: SWAGGER_UI_BUNDLE,
    },
    Asset {
        path: "/api/docs/static/swagger-ui.css",
        content_type: "text/css; charset=utf-8",
        cache_control: VENDORED_CACHE_CONTROL,
        body: SWAGGER_UI_CSS,
    },
    Asset {
        path: "/api/docs/static/swagger-ui-bundle.js.LICENSE.txt",
        content_type: "text/plain; charset=utf-8",
        cache_control: VENDORED_CACHE_CONTROL,
        body: SWAGGER_UI_BUNDLE_LICENSE,
    },
];

pub fn router() -> Router<AppState> {
    ui_router().route("/api/docs/json", get(get_openapi_json))
}

/// The Swagger UI page and its assets.
fn ui_router() -> Router<AppState> {
    UI_ASSETS.iter().fold(Router::new(), |router, asset| {
        router.route(
            asset.path,
            get(
                move |State(state): State<AppState>, headers: HeaderMap, jar: CookieJar| async move {
                    require_session(&state, &headers, &jar).await?;
                    Ok::<_, AppError>(fixed(asset.content_type, asset.cache_control, asset.body))
                },
            ),
        )
    })
}

async fn get_openapi_json(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> Result<Response, AppError> {
    require_session(&state, &headers, &jar).await?;
    Ok(fixed("application/json", NO_STORE, OPENAPI_JSON.as_bytes()))
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

fn fixed(content_type: &'static str, cache_control: &'static str, body: &'static [u8]) -> Response {
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, cache_control),
        ],
        body,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::security_headers::inline_hashes;
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
        assert_eq!(
            hex::encode(Sha256::digest(SWAGGER_UI_BUNDLE_LICENSE)),
            "63818894e4b04cd0e3180d9cb20761e227a939121e7484f8e1d528227c756f89"
        );
    }

    /// The page runs under the application CSP (`script-src 'self'`,
    /// `script-src-attr 'none'`, `style-src 'self'`): it may only reference
    /// same-origin files served by [`ui_router`], and must hold no inline
    /// script, style block, style attribute or event handler.
    #[test]
    fn page_is_static_and_references_only_served_assets() {
        let html = std::str::from_utf8(PAGE_HTML).unwrap();
        assert!(inline_hashes(html, "script").is_empty());
        assert!(inline_hashes(html, "style").is_empty());
        assert!(!html.contains("<style"));
        assert!(!html.contains("style="));
        let handler = regex::Regex::new(r"(?i)\son[a-z]+\s*=").unwrap();
        assert!(!handler.is_match(html), "no event handler attribute");
        assert!(!html.contains("//"), "no absolute or protocol-relative URL");

        let refs = regex::Regex::new(r#"(?:src|href)="([^"]+)""#).unwrap();
        let referenced = refs
            .captures_iter(html)
            .map(|c| c[1].to_string())
            .collect::<Vec<_>>();
        assert_eq!(referenced.len(), 4, "{referenced:?}");
        for url in &referenced {
            let (path, query) = url.split_once('?').unwrap_or((url, ""));
            let asset = UI_ASSETS
                .iter()
                .find(|a| a.path == path)
                .unwrap_or_else(|| panic!("{url} is not served"));
            // A long-cached URL must change when its bytes can change.
            if asset.cache_control == VENDORED_CACHE_CONTROL {
                assert_eq!(query, format!("v={SWAGGER_UI_VERSION}"), "{url}");
            } else {
                assert_eq!(asset.cache_control, REVALIDATE_CACHE_CONTROL, "{url}");
            }
        }

        let init = std::str::from_utf8(INITIALIZER_JS).unwrap();
        assert!(init.contains(r#"url: "/api/docs/json""#));
        assert!(std::str::from_utf8(THEME_CSS)
            .unwrap()
            .contains("prefers-color-scheme: dark"));
    }
}
