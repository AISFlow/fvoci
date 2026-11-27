//! Search-engine directives (`/robots.txt`, `X-Robots-Tag`) and RFC 9116 security
//! disclosure (`/.well-known/security.txt`), with and without a base path.

use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::{Method, StatusCode};

use crate::common::{Setup, TestServer};

/// Response parts inspected across crawler and well-known routes.
#[derive(Debug)]
struct Answer {
    status: StatusCode,
    content_type: String,
    cache_control: String,
    x_content_type_options: String,
    x_robots_tag: Option<String>,
    allow: String,
    body: String,
}

fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("HTTP client")
    })
}

async fn ask(origin: &str, method: Method, path: &str) -> Answer {
    let response = http_client()
        .request(method, format!("{origin}{path}"))
        .send()
        .await
        .expect("server answers");
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    };
    Answer {
        status: response.status(),
        content_type: header("content-type").unwrap_or_default(),
        cache_control: header("cache-control").unwrap_or_default(),
        x_content_type_options: header("x-content-type-options").unwrap_or_default(),
        x_robots_tag: header("x-robots-tag"),
        allow: header("allow").unwrap_or_default(),
        body: response.text().await.expect("response body"),
    }
}

fn origin_of(th: &TestServer, base: &str) -> String {
    th.base_url
        .strip_suffix(base)
        .expect("base_url ends with base_path")
        .to_owned()
}

fn write_static_html(th: &TestServer) {
    let path = Path::new(&th.config.web_app_path)
        .join("static")
        .join("viewer-frame.html");
    std::fs::write(path, "<!doctype html><title>frame</title>").expect("write static html");
}

/// Parses an RFC 3339 UTC timestamp (`YYYY-MM-DDTHH:MM:SSZ`) into Unix seconds.
fn parse_rfc3339_utc(text: &str) -> Option<u64> {
    if !text.is_ascii() {
        return None;
    }
    let bytes = text.as_bytes();
    if bytes.len() != 20
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
        || bytes[19] != b'Z'
    {
        return None;
    }
    let digits = |range: std::ops::Range<usize>| {
        let slice = &text[range];
        slice
            .bytes()
            .all(|byte| byte.is_ascii_digit())
            .then_some(slice)
    };
    let year: i64 = digits(0..4)?.parse().ok()?;
    let month: u32 = digits(5..7)?.parse().ok()?;
    let day: u32 = digits(8..10)?.parse().ok()?;
    let hour: u64 = digits(11..13)?.parse().ok()?;
    let minute: u64 = digits(14..16)?.parse().ok()?;
    let second: u64 = digits(17..19)?.parse().ok()?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    // Howard Hinnant's `days_from_civil`.
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(month);
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let days = u64::try_from(days).ok()?;
    Some(days * 86_400 + hour * 3600 + minute * 60 + second)
}

fn unix_now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_secs()
}

/// Checks `GET`/`HEAD`/`POST` on `/robots.txt` at the origin root.
async fn check_robots_txt(origin: &str) {
    let robots = ask(origin, Method::GET, "/robots.txt").await;
    assert_eq!(robots.status, StatusCode::OK, "GET /robots.txt");
    assert_eq!(
        robots.content_type, "text/plain; charset=utf-8",
        "GET /robots.txt content-type"
    );
    assert_eq!(
        robots.cache_control, "public, max-age=3600",
        "GET /robots.txt cache-control"
    );
    assert_eq!(
        robots.x_content_type_options, "nosniff",
        "GET /robots.txt nosniff"
    );
    assert_eq!(robots.x_robots_tag, None, "/robots.txt has no X-Robots-Tag");
    assert_eq!(
        robots.body, "User-agent: *\nDisallow: /\n",
        "GET /robots.txt body"
    );

    let head = ask(origin, Method::HEAD, "/robots.txt").await;
    assert_eq!(head.status, StatusCode::OK, "HEAD /robots.txt");
    assert_eq!(
        head.content_type, "text/plain; charset=utf-8",
        "HEAD /robots.txt content-type"
    );
    assert_eq!(
        head.cache_control, "public, max-age=3600",
        "HEAD /robots.txt cache-control"
    );
    assert_eq!(
        head.x_content_type_options, "nosniff",
        "HEAD /robots.txt nosniff"
    );
    assert_eq!(
        head.x_robots_tag, None,
        "HEAD /robots.txt has no X-Robots-Tag"
    );
    assert!(head.body.is_empty(), "HEAD /robots.txt body is empty");

    let post = ask(origin, Method::POST, "/robots.txt").await;
    assert_eq!(
        post.status,
        StatusCode::METHOD_NOT_ALLOWED,
        "POST /robots.txt"
    );
    assert!(
        post.allow.split(',').any(|name| name.trim() == "GET"),
        "Allow lists GET: {:?}",
        post.allow
    );
}

/// Checks `/robots.txt` and `X-Robots-Tag` across pages, static HTML, `/problems`, and
/// `/api/...` on a server mounted under `base` (empty or `/boards`).
async fn check_robots_and_headers(origin: &str, base: &str) {
    check_robots_txt(origin).await;

    // Indexable pages must carry `X-Robots-Tag: noindex, nofollow`.
    let mut indexed_pages = vec![
        format!("{base}/"),
        format!("{base}/login"),
        format!("{base}/shared/board-123?r=read-token"),
        format!("{base}/static/viewer-frame.html"),
        format!("{base}/static/viewer%2Dframe.html"),
        format!("{base}/problems"),
        format!("{base}/problems/not-found"),
    ];
    if !base.is_empty() {
        indexed_pages.push(base.to_owned());
    }
    for path in indexed_pages {
        let answer = ask(origin, Method::GET, &path).await;
        assert_eq!(answer.status, StatusCode::OK, "GET {path}");
        assert_eq!(
            answer.x_robots_tag.as_deref(),
            Some("noindex, nofollow"),
            "GET {path} must send X-Robots-Tag"
        );
        let head = ask(origin, Method::HEAD, &path).await;
        assert_eq!(head.status, StatusCode::OK, "HEAD {path}");
        assert_eq!(
            head.x_robots_tag.as_deref(),
            Some("noindex, nofollow"),
            "HEAD {path} must send X-Robots-Tag"
        );
        assert!(head.body.is_empty(), "HEAD {path} body is empty");
    }

    // API routes, non-HTML static assets, and error JSON responses must not carry
    // `X-Robots-Tag`.
    for (path, expected_status) in [
        (format!("{base}/api/v1/ping"), StatusCode::OK),
        (format!("{base}/api/v1/client-config"), StatusCode::OK),
        (
            format!("{base}/api/v1/no-such-route"),
            StatusCode::NOT_FOUND,
        ),
        (format!("{base}/api"), StatusCode::NOT_FOUND),
        (format!("{base}/api/v2/boards"), StatusCode::NOT_FOUND),
        (format!("{base}/static/main.js"), StatusCode::OK),
        (
            format!("{base}/problems/no-such-kind"),
            StatusCode::NOT_FOUND,
        ),
    ] {
        let answer = ask(origin, Method::GET, &path).await;
        assert_eq!(answer.status, expected_status, "GET {path}");
        assert_eq!(
            answer.x_robots_tag, None,
            "GET {path} must not send X-Robots-Tag"
        );
    }
}

/// Checks that disabled `/.well-known/security.txt` and legacy `/security.txt` return
/// 404 across GET, HEAD, and POST (never 405 or `index.html`).
async fn check_security_txt_disabled(origin: &str) {
    for path in ["/.well-known/security.txt", "/security.txt"] {
        for method in [Method::GET, Method::HEAD, Method::POST] {
            let answer = ask(origin, method.clone(), path).await;
            assert_eq!(answer.status, StatusCode::NOT_FOUND, "{method} {path}");
            if method != Method::HEAD {
                assert_eq!(
                    answer.body, "not found\n",
                    "{method} {path} is not index.html"
                );
            }
            assert_eq!(answer.x_robots_tag, None, "{method} {path}");
        }
    }
}

#[tokio::test]
async fn robots_and_x_robots_tag_without_a_base_path() {
    let th = Setup::native().start().await;
    write_static_html(&th);
    let origin = origin_of(&th, "");
    check_robots_and_headers(&origin, "").await;
    check_security_txt_disabled(&origin).await;
    th.teardown().await;
}

#[tokio::test]
async fn robots_and_x_robots_tag_under_a_base_path() {
    let th = Setup::native()
        .base_path("/boards")
        .configure(|config| {
            config.server_url = "https://example.com/boards".to_owned();
        })
        .start()
        .await;
    write_static_html(&th);
    let origin = origin_of(&th, "/boards");
    check_robots_and_headers(&origin, "/boards").await;
    check_security_txt_disabled(&origin).await;
    th.teardown().await;
}

/// Checks that legacy `/security.txt` is always 404, never served or redirected.
async fn check_legacy_security_txt_not_found(origin: &str) {
    for method in [Method::GET, Method::POST] {
        let legacy = ask(origin, method.clone(), "/security.txt").await;
        assert_eq!(
            legacy.status,
            StatusCode::NOT_FOUND,
            "{method} legacy /security.txt status"
        );
        assert_eq!(
            legacy.body, "not found\n",
            "{method} legacy /security.txt body"
        );
    }
}

/// Checks `HEAD` (200 with empty body) and `POST` (405) on an enabled `security.txt`.
async fn check_security_txt_methods(origin: &str) {
    let head = ask(origin, Method::HEAD, "/.well-known/security.txt").await;
    assert_eq!(
        head.status,
        StatusCode::OK,
        "HEAD /.well-known/security.txt"
    );
    assert_eq!(
        head.content_type, "text/plain; charset=utf-8",
        "HEAD security.txt content-type"
    );
    assert_eq!(
        head.cache_control, "public, max-age=3600",
        "HEAD security.txt cache-control"
    );
    assert!(head.body.is_empty(), "HEAD security.txt body is empty");

    let post = ask(origin, Method::POST, "/.well-known/security.txt").await;
    assert_eq!(
        post.status,
        StatusCode::METHOD_NOT_ALLOWED,
        "POST /.well-known/security.txt"
    );
    assert!(
        post.allow.split(',').any(|name| name.trim() == "GET"),
        "Allow lists GET: {:?}",
        post.allow
    );
}

#[tokio::test]
async fn security_txt_conforms_to_rfc9116_with_https_origin_and_base_path() {
    let before = unix_now_seconds();
    let th = Setup::native()
        .base_path("/boards")
        .configure(|config| {
            config.server_url = "https://example.com/boards".to_owned();
            config.security_contact = "mailto:security@example.com".to_owned();
        })
        .start()
        .await;
    let origin = origin_of(&th, "/boards");

    let answer = ask(&origin, Method::GET, "/.well-known/security.txt").await;
    let after = unix_now_seconds();
    assert_eq!(
        answer.status,
        StatusCode::OK,
        "GET /.well-known/security.txt"
    );
    assert_eq!(
        answer.content_type, "text/plain; charset=utf-8",
        "security.txt content-type"
    );
    assert_eq!(
        answer.cache_control, "public, max-age=3600",
        "security.txt cache-control"
    );
    assert_eq!(
        answer.x_content_type_options, "nosniff",
        "security.txt nosniff"
    );
    assert_eq!(
        answer.x_robots_tag, None,
        "security.txt has no X-Robots-Tag"
    );

    let lines: Vec<&str> = answer.body.lines().collect();
    assert_eq!(
        lines.len(),
        3,
        "Contact, Expires, Canonical: {:?}",
        answer.body
    );
    assert_eq!(
        lines[0], "Contact: mailto:security@example.com",
        "Contact line"
    );
    let expires_text = lines[1]
        .strip_prefix("Expires: ")
        .expect("Expires field present");
    let expires = parse_rfc3339_utc(expires_text)
        .unwrap_or_else(|| panic!("valid RFC 3339 UTC timestamp: {expires_text:?}"));
    let offset_min = 180 * 86_400;
    assert!(
        expires >= before + offset_min && expires <= after + offset_min,
        "Expires {expires_text} is 180 days ahead of request time"
    );
    assert!(
        expires - before < 365 * 86_400,
        "Expires stays under RFC 9116's one-year recommendation"
    );
    assert_eq!(
        lines[2], "Canonical: https://example.com/.well-known/security.txt",
        "Canonical uses origin root without the /boards base path"
    );

    check_security_txt_methods(&origin).await;
    check_legacy_security_txt_not_found(&origin).await;
    th.teardown().await;
}

#[tokio::test]
async fn security_txt_omits_canonical_on_plain_http_and_refuses_legacy_path() {
    let th = Setup::native()
        .configure(|config| {
            config.server_url = "http://localhost:8000".to_owned();
            config.security_contact = "https://example.org/security".to_owned();
        })
        .start()
        .await;
    let origin = origin_of(&th, "");

    let answer = ask(&origin, Method::GET, "/.well-known/security.txt").await;
    assert_eq!(
        answer.status,
        StatusCode::OK,
        "GET /.well-known/security.txt"
    );
    assert_eq!(
        answer.content_type, "text/plain; charset=utf-8",
        "security.txt content-type"
    );
    assert_eq!(
        answer.cache_control, "public, max-age=3600",
        "security.txt cache-control"
    );
    let lines: Vec<&str> = answer.body.lines().collect();
    assert_eq!(
        lines.len(),
        2,
        "Contact and Expires only on HTTP: {:?}",
        answer.body
    );
    assert_eq!(
        lines[0], "Contact: https://example.org/security",
        "Contact line"
    );
    assert!(
        lines[1]
            .strip_prefix("Expires: ")
            .and_then(parse_rfc3339_utc)
            .is_some(),
        "valid RFC 3339 UTC Expires: {:?}",
        lines[1]
    );
    assert!(
        !answer.body.contains("Canonical:"),
        "plain HTTP omits Canonical (RFC 9116 requires https://)"
    );

    check_legacy_security_txt_not_found(&origin).await;
    th.teardown().await;
}

#[tokio::test]
async fn invalid_security_contact_refuses_to_start() {
    let error = Setup::native()
        .configure(|config| {
            config.security_contact = "http://example.com/security".to_owned();
        })
        .try_start()
        .await
        .err()
        .expect("non-HTTPS web URI must prevent startup");
    assert!(
        error.to_string().contains("security_contact"),
        "startup error names security_contact: {error:#}"
    );
}
