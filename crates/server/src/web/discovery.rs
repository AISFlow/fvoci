//! Search-engine directives (`/robots.txt`, `X-Robots-Tag`) and RFC 9116 security
//! disclosure (`/.well-known/security.txt`).

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::extract::{Request, State};
use axum::http::HeaderValue;
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, HeaderName, X_CONTENT_TYPE_OPTIONS};
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get};

use crate::auth::token::origin_of;
use crate::config::SecurityContact;
use crate::error::PROBLEMS_PATH;
use crate::web::WebRoot;

/// The origin-root path crawlers fetch for robots rules (RFC 9309).
const ROBOTS_TXT_PATH: &str = "/robots.txt";

/// The origin-root well-known path for security disclosure metadata (RFC 9116).
const SECURITY_TXT_PATH: &str = "/.well-known/security.txt";

/// The legacy top-level path for security.txt; always answered with 404 so the SPA
/// fallback never returns `index.html` for it.
const LEGACY_SECURITY_TXT_PATH: &str = "/security.txt";

/// Disallow all crawlers on the origin by default.
const ROBOTS_TXT_BODY: &str = "User-agent: *\nDisallow: /\n";

/// How long caches may keep `/robots.txt` and `/.well-known/security.txt`: one hour,
/// matching `/problems` and well inside the 180-day `Expires` window.
const METADATA_CACHE_CONTROL: &str = "public, max-age=3600";

/// How far ahead `Expires` is set on `/.well-known/security.txt`: 180 days (about six
/// months), comfortably inside RFC 9116 section 2.5.5's one-year recommendation.
const SECURITY_TXT_LIFETIME_SECONDS: u64 = 180 * 86_400;

/// The `X-Robots-Tag` response header name.
const X_ROBOTS_TAG: HeaderName = HeaderName::from_static("x-robots-tag");

/// Prevents search engines from indexing pages or following links out of them.
const NO_INDEX_VALUE: HeaderValue = HeaderValue::from_static("noindex, nofollow");

/// Pre-validated data for `/.well-known/security.txt` (RFC 9116).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SecurityTxt {
    contact: SecurityContact,
    /// `https://<origin>/.well-known/security.txt` when `server_url` is HTTPS; omitted
    /// on plain HTTP because RFC 9116 section 2.5.2 requires web `Canonical` URIs to
    /// begin with `https://`.
    canonical: Option<String>,
}

impl SecurityTxt {
    /// Builds the `security.txt` policy from a validated `contact` and the normalized
    /// HTTPS origin of `server_url` (`None` on plain HTTP).
    pub(crate) fn new(contact: SecurityContact, https_origin: Option<&str>) -> Self {
        let canonical = https_origin.map(|origin| format!("{origin}{SECURITY_TXT_PATH}"));
        Self { contact, canonical }
    }

    /// Renders the RFC 9116 document with `Expires` set 180 days after `now_seconds`.
    fn render(&self, now_seconds: u64) -> String {
        let expires = format_rfc3339_utc(now_seconds.saturating_add(SECURITY_TXT_LIFETIME_SECONDS));
        let mut body = format!("Contact: {}\nExpires: {expires}\n", self.contact.as_str());
        if let Some(canonical) = &self.canonical {
            // Writing to a String cannot fail.
            let _ = writeln!(body, "Canonical: {canonical}");
        }
        body
    }
}

/// Returns the normalized HTTPS origin of `server_url`, or `None` if `server_url` is
/// not an `https://` URL.
pub(crate) fn https_origin_of(server_url: &str) -> Option<String> {
    origin_of(server_url).filter(|origin| origin.starts_with("https://"))
}

/// Origin-root routes for `/robots.txt`, `/.well-known/security.txt`, and the disabled
/// legacy `/security.txt`.
///
/// Always mounted at the server's origin root regardless of `FVOCI_BASE_PATH`, because
/// RFC 9309 and RFC 9116 clients only query the origin root. When `security_contact` is
/// unset, `/.well-known/security.txt` answers 404 for every method rather than 405.
pub(crate) fn origin_routes(web: Arc<WebRoot>) -> Router {
    let security_route = if web.security_txt().is_some() {
        get(handle_security_txt)
    } else {
        any(handle_not_found)
    };
    Router::new()
        .route(ROBOTS_TXT_PATH, get(handle_robots_txt))
        .route(SECURITY_TXT_PATH, security_route)
        .route(LEGACY_SECURITY_TXT_PATH, any(handle_not_found))
        .with_state(web)
}

/// Adds `X-Robots-Tag: noindex, nofollow` to indexable pages served by the server:
/// the web app shell (`serve_index`, including `/shared/...`), HTML documents under
/// `/static/`, and `/problems` pages. Never touches `/api/...`.
///
/// Centralized here so a future "expose to search engines" setting can turn it off in
/// one place.
pub(crate) async fn send_robots_tag(
    State(base_path): State<Arc<str>>,
    request: Request,
    next: middleware::Next,
) -> Response {
    let Some(relative) = strip_base_path(request.uri().path(), &base_path) else {
        return next.run(request).await;
    };
    let is_api = relative == "/api" || relative.starts_with("/api/");
    let is_problems = relative == PROBLEMS_PATH
        || relative
            .strip_prefix(PROBLEMS_PATH)
            .is_some_and(|rest| rest.starts_with('/'));
    let mut response = next.run(request).await;
    if is_api {
        return response;
    }
    let is_html = content_type_is(&response, "text/html");
    let is_problem_page =
        is_problems && response.status().is_success() && content_type_is(&response, "text/plain");
    if is_html || is_problem_page {
        response
            .headers_mut()
            .entry(X_ROBOTS_TAG)
            .or_insert(NO_INDEX_VALUE);
    }
    response
}

async fn handle_robots_txt() -> Response {
    plain_text(ROBOTS_TXT_BODY)
}

async fn handle_security_txt(State(web): State<Arc<WebRoot>>) -> Response {
    let Some(security_txt) = web.security_txt() else {
        return super::not_found();
    };
    plain_text(security_txt.render(unix_now_seconds()))
}

async fn handle_not_found() -> Response {
    super::not_found()
}

fn plain_text(body: impl IntoResponse) -> Response {
    (
        [
            (
                CONTENT_TYPE,
                HeaderValue::from_static("text/plain; charset=utf-8"),
            ),
            (X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")),
            (
                CACHE_CONTROL,
                HeaderValue::from_static(METADATA_CACHE_CONTROL),
            ),
        ],
        body,
    )
        .into_response()
}

/// Strips `base_path` from `path` on a segment boundary, returning `None` if `path`
/// lies outside `base_path`.
fn strip_base_path<'a>(path: &'a str, base_path: &str) -> Option<&'a str> {
    if base_path.is_empty() {
        return Some(path);
    }
    let rest = path.strip_prefix(base_path)?;
    if rest.is_empty() || rest.starts_with('/') {
        Some(rest)
    } else {
        None
    }
}

/// Returns `true` if `response` carries a `Content-Type` whose media type (before any
/// `;` parameters) equals `expected`, ASCII case-insensitively.
fn content_type_is(response: &Response, expected: &str) -> bool {
    response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|media| media.trim().eq_ignore_ascii_case(expected))
}

fn unix_now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// Formats whole seconds since the Unix epoch as RFC 3339 UTC (`YYYY-MM-DDTHH:MM:SSZ`).
///
/// Uses Howard Hinnant's `civil_from_days` algorithm.
fn format_rfc3339_utc(seconds: u64) -> String {
    let days = i64::try_from(seconds / 86_400).unwrap_or(i64::MAX);
    let of_day = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = of_day / 3600;
    let minute = of_day % 3600 / 60;
    let second = of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Converts days since 1970-01-01 to `(year, month, day)` in the proleptic Gregorian
/// calendar (Howard Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days.saturating_add(719_468);
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = u32::try_from(doy - (153 * mp + 2) / 5 + 1).unwrap_or(1);
    let month = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(1);
    let year = if month <= 2 { year + 1 } else { year };
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_utc_formats_epoch_leap_day_and_year_end() {
        assert_eq!(format_rfc3339_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_rfc3339_utc(1_709_208_000), "2024-02-29T12:00:00Z");
        assert_eq!(format_rfc3339_utc(1_735_689_599), "2024-12-31T23:59:59Z");
        assert_eq!(format_rfc3339_utc(1_794_287_739), "2026-11-10T05:15:39Z");
    }

    #[test]
    fn security_txt_renders_canonical_only_for_https_origins() {
        let contact = SecurityContact::parse("mailto:security@example.com")
            .expect("valid")
            .expect("present");
        // 2026-11-10T05:15:39Z + 180 days = 2027-05-09T05:15:39Z.
        let now = 1_794_287_739;

        let https_subpath = SecurityTxt::new(
            contact.clone(),
            https_origin_of("https://Example.COM:443/boards").as_deref(),
        );
        assert_eq!(
            https_subpath.render(now),
            "Contact: mailto:security@example.com\n\
             Expires: 2027-05-09T05:15:39Z\n\
             Canonical: https://example.com/.well-known/security.txt\n"
        );

        let https_port = SecurityTxt::new(
            contact.clone(),
            https_origin_of("https://example.org:8443/boards").as_deref(),
        );
        assert_eq!(
            https_port.render(now),
            "Contact: mailto:security@example.com\n\
             Expires: 2027-05-09T05:15:39Z\n\
             Canonical: https://example.org:8443/.well-known/security.txt\n"
        );

        let http = SecurityTxt::new(
            contact,
            https_origin_of("http://localhost:8000/boards").as_deref(),
        );
        assert_eq!(
            http.render(now),
            "Contact: mailto:security@example.com\n\
             Expires: 2027-05-09T05:15:39Z\n"
        );
    }

    #[test]
    fn strip_base_path_respects_segment_boundaries() {
        assert_eq!(strip_base_path("/api/v1/ping", ""), Some("/api/v1/ping"));
        assert_eq!(strip_base_path("/boards", "/boards"), Some(""));
        assert_eq!(strip_base_path("/boards/", "/boards"), Some("/"));
        assert_eq!(
            strip_base_path("/boards/problems", "/boards"),
            Some("/problems")
        );
        assert_eq!(strip_base_path("/boardsx/problems", "/boards"), None);
        assert_eq!(strip_base_path("/robots.txt", "/boards"), None);
    }
}
