use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use fvoci_server::auth::password::Keyring;
use fvoci_server::auth::AuthService;
use fvoci_server::db::Db;
use fvoci_server::http::rate_limit::RateLimiter;
use fvoci_server::http::static_assets::{
    is_safe_static_path, resolve_static_index, static_router, validate_static_root,
};
use fvoci_server::http::{router, state::AppState};
use tower::ServiceExt;

const PEPPER: &str =
    r#"{"test":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#;

async fn app_state() -> AppState {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_lazy("postgres://postgres:postgres@127.0.0.1:1/none")
        .expect("lazy pool");
    let storage_root =
        std::env::temp_dir().join(format!("fvoci-static-test-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&storage_root).expect("storage root");
    AppState {
        realtime_mode: fvoci_server::config::RealtimeMode::On,
        native_engine: None,
        auth: Arc::new(AuthService {
            db: Db::new(pool),
            password_keys: Keyring::parse(PEPPER, "test").expect("pepper"),
        }),
        branding_name: "FVOCI".to_string(),
        public_origin: "http://localhost".to_string(),
        cookie_secure: false,
        rate_limiter: RateLimiter::new(),
        storage: fvoci_server::attachments::LocalStorage::new(storage_root).into(),
        upload: fvoci_server::attachments::UploadLimits {
            part_size_bytes: fvoci_server::config::DEFAULT_UPLOAD_PART_SIZE_BYTES,
            max_file_size_bytes: fvoci_server::config::DEFAULT_UPLOAD_MAX_FILE_SIZE_BYTES,
            create_rate_per_5min: fvoci_server::config::DEFAULT_UPLOAD_CREATE_RATE_PER_5MIN,
            part_put_slots: fvoci_server::attachments::PartPutSlots::new(
                fvoci_server::config::DEFAULT_UPLOAD_MAX_CONCURRENT_PARTS,
            ),
        },
        collab: None,
        meili: None,
        search_embedder: None,
        markdown: Some(
            fvoci_server::documents::markdown_helper::MarkdownHelper::new(env!(
                "CARGO_BIN_EXE_fvoci-server"
            )),
        ),
        import_wake: None,
        import_extractor_available: false,
        preview_extract: None,
        quota: Default::default(),
        streams: fvoci_server::http::state::AppState::fresh_streams(),
        mailer: std::sync::Arc::new(fvoci_server::mail::Mailer::disabled()),
    }
}

#[tokio::test]
async fn unknown_api_route_returns_problem_not_html() {
    let app = router(app_state().await, None);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/unknown-endpoint")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json.get("code").and_then(|v| v.as_str()), Some("not_found"));
}

#[tokio::test]
async fn static_root_rejects_traversal_paths() {
    assert!(!is_safe_static_path("/../secret"));
    assert!(!is_safe_static_path("/.env"));
    assert!(is_safe_static_path("/assets/app.js"));
}

#[tokio::test]
async fn static_router_serves_index_and_asset() {
    let dir = std::env::temp_dir().join(format!("fvoci-static-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).expect("tmpdir");
    std::fs::write(dir.join("index.html"), "<html>ok</html>").unwrap();
    std::fs::write(dir.join("assets.txt"), "asset").unwrap();

    let app: Router = static_router(dir.clone());
    let response = app
        .clone()
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");

    for uri in ["/index.html", "/w/acme/settings"] {
        let response = app
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["cache-control"], "no-store");
    }

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/assets.txt")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/missing-asset.js")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/assets/missing")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(unix)]
#[tokio::test]
async fn static_router_rejects_outside_root_symlink_and_index_escape() {
    let outside = std::env::temp_dir().join(format!("fvoci-outside-{}", uuid::Uuid::now_v7()));
    let dir = std::env::temp_dir().join(format!("fvoci-static-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&outside).expect("outside dir");
    std::fs::create_dir_all(&dir).expect("tmpdir");
    std::fs::create_dir_all(dir.join("assets")).expect("assets dir");
    std::fs::write(outside.join("secret.txt"), "secret").unwrap();
    std::fs::write(dir.join("index.html"), "<html>ok</html>").unwrap();
    std::os::unix::fs::symlink(outside.join("secret.txt"), dir.join("assets/escape.txt")).unwrap();

    let app: Router = static_router(dir.clone());
    let response = app
        .oneshot(
            Request::builder()
                .uri("/assets/escape.txt")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let outside_index = outside.join("outside.html");
    std::fs::write(&outside_index, "<html>outside</html>").unwrap();
    let index_link = dir.join("index.html");
    std::fs::remove_file(&index_link).unwrap();
    std::os::unix::fs::symlink(&outside_index, &index_link).unwrap();
    assert!(validate_static_root(&dir).is_err());
    assert!(resolve_static_index(&dir.canonicalize().unwrap()).is_err());

    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(outside);
}

#[tokio::test]
async fn merged_router_returns_json_for_unknown_api_and_serves_static() {
    let dir = std::env::temp_dir().join(format!("fvoci-app-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).expect("tmpdir");
    std::fs::write(dir.join("index.html"), "<html>ok</html>").unwrap();

    let app = router(app_state().await, Some(dir.clone()));

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/unknown-endpoint")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json.get("code").and_then(|v| v.as_str()), Some("not_found"));

    let response = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn static_shell_and_assets_send_no_referrer() {
    let dir = std::env::temp_dir().join(format!("fvoci-static-ref-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).expect("tmpdir");
    std::fs::write(dir.join("index.html"), "<html>ok</html>").unwrap();
    std::fs::write(dir.join("assets.txt"), "asset").unwrap();
    let app: Router = router(app_state().await, Some(dir.clone()));
    // The SPA shell for a share URL carries the token in the path.
    for (uri, status) in [
        ("/s/some-share-token", StatusCode::OK),
        ("/", StatusCode::OK),
        ("/assets.txt", StatusCode::OK),
        ("/missing-asset.js", StatusCode::NOT_FOUND),
    ] {
        let response = app
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), status, "{uri}");
        assert_eq!(
            response.headers()["referrer-policy"],
            "no-referrer",
            "{uri}"
        );
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// Source `http-security.ts` (nosecone defaults + application CSP) on the SPA
/// shell, an asset, an API problem response and the 404 fallback.
#[tokio::test]
async fn global_security_headers_on_shell_assets_and_api() {
    let dir = std::env::temp_dir().join(format!("fvoci-static-sec-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).expect("tmpdir");
    // An inline theme script is allowed by its build-time hash only.
    std::fs::write(
        dir.join("index.html"),
        "<html><head><script>document.documentElement.dataset.t='1'</script><script type=\"module\" src=\"/assets/a.js\"></script></head></html>",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    std::fs::write(dir.join("assets/a.js"), "export{}").unwrap();
    let app: Router = router(app_state().await, Some(dir.clone()));
    let inline_hash = {
        use base64::Engine;
        use sha2::Digest;
        base64::engine::general_purpose::STANDARD.encode(sha2::Sha256::digest(
            b"document.documentElement.dataset.t='1'",
        ))
    };
    for (uri, status) in [
        ("/", StatusCode::OK),
        ("/w/some/wiki", StatusCode::OK),
        ("/assets/a.js", StatusCode::OK),
        ("/assets/missing.js", StatusCode::NOT_FOUND),
        ("/api/v1/unknown-endpoint", StatusCode::NOT_FOUND),
    ] {
        let response = app
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), status, "{uri}");
        let h = response.headers();
        let csp = h["content-security-policy"].to_str().unwrap();
        assert_eq!(
            csp,
            format!(
                "default-src 'self'; base-uri 'self'; font-src 'self' data:; form-action 'self'; \
                 frame-ancestors 'self'; img-src 'self' data: blob:; object-src 'none'; \
                 script-src 'self' 'wasm-unsafe-eval' 'sha256-{inline_hash}'; script-src-attr 'none'; \
                 style-src 'self'; connect-src 'self'; \
                 frame-src 'self' blob: https://www.youtube.com https://player.vimeo.com https://www.figma.com;"
            ),
            "{uri}"
        );
        assert_eq!(h["referrer-policy"], "no-referrer", "{uri}");
        assert_eq!(h["x-content-type-options"], "nosniff", "{uri}");
        assert_eq!(h["x-frame-options"], "SAMEORIGIN", "{uri}");
        assert_eq!(h["cross-origin-opener-policy"], "same-origin", "{uri}");
        assert_eq!(h["cross-origin-resource-policy"], "same-origin", "{uri}");
        assert_eq!(h["origin-agent-cluster"], "?1", "{uri}");
        assert_eq!(h["x-dns-prefetch-control"], "off", "{uri}");
        assert_eq!(h["x-download-options"], "noopen", "{uri}");
        assert_eq!(h["x-permitted-cross-domain-policies"], "none", "{uri}");
        assert_eq!(h["x-xss-protection"], "0", "{uri}");
        assert!(h["permissions-policy"]
            .to_str()
            .unwrap()
            .contains("camera=()"));
        // The public origin of this state is http: no HSTS, no upgrade.
        assert!(h.get("strict-transport-security").is_none(), "{uri}");
        assert!(h.get("cross-origin-embedder-policy").is_none(), "{uri}");
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn https_origin_adds_hsts_and_upgrade_insecure_requests() {
    let mut state = app_state().await;
    state.public_origin = "https://fvoci.example".to_string();
    let response = router(state, None)
        .oneshot(
            Request::builder()
                .uri("/api/v1/unknown-endpoint")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let h = response.headers();
    assert_eq!(
        h["strict-transport-security"],
        "max-age=31536000; includeSubDomains"
    );
    assert!(h["content-security-policy"]
        .to_str()
        .unwrap()
        .ends_with("; upgrade-insecure-requests;"));
}

/// `HEAD /s/{token}` answers like the GET shell (source server.ts `/s/:token`
/// headers): private no-store, noindex, no-referrer and the HTML content type
/// with an empty body. It omits Content-Length (RFC 9110 9.3.2) because the
/// GET length depends on the share's head tags. Other paths keep the generic
/// static HEAD handling.
#[tokio::test]
async fn share_shell_head_matches_get_headers_with_empty_body() {
    let dir = std::env::temp_dir().join(format!("fvoci-static-head-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).expect("tmpdir");
    std::fs::write(dir.join("index.html"), "<html>ok</html>").unwrap();
    std::fs::write(dir.join("assets.txt"), "asset").unwrap();
    let app: Router = router(app_state().await, Some(dir.clone()));
    let send = |method: &'static str, uri: &'static str| {
        let app = app.clone();
        async move {
            let response = app
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(uri)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let status = response.status();
            let headers = response.headers().clone();
            let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            (status, headers, bytes)
        }
    };
    for uri in ["/s/some-share-token", "/s/some-share-token/"] {
        let (status, get_headers, get_body) = send("GET", uri).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        let (status, h, body) = send("HEAD", uri).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert!(body.is_empty(), "{uri}");
        assert_eq!(h["content-type"], "text/html; charset=utf-8", "{uri}");
        assert_eq!(h["cache-control"], "private, no-store", "{uri}");
        assert_eq!(h["x-robots-tag"], "noindex", "{uri}");
        assert_eq!(h["referrer-policy"], "no-referrer", "{uri}");
        assert!(h.get("content-length").is_none(), "{uri}");
        assert!(!get_body.is_empty(), "{uri}");
        for name in [
            "content-type",
            "cache-control",
            "x-robots-tag",
            "referrer-policy",
        ] {
            assert_eq!(h[name], get_headers[name], "{uri} {name}");
        }
    }
    // Non-share paths keep the generic static HEAD: no noindex, own caching.
    for (uri, cache) in [
        ("/", Some("no-store")),
        ("/w/some/wiki", Some("no-store")),
        ("/s/some-share-token/attachments/x", Some("no-store")),
        ("/assets.txt", None),
    ] {
        let (status, h, body) = send("HEAD", uri).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert!(body.is_empty(), "{uri}");
        assert!(h.get("x-robots-tag").is_none(), "{uri}");
        assert_eq!(
            h.get("cache-control").map(|v| v.to_str().unwrap()),
            cache,
            "{uri}"
        );
    }
    let (status, _, body) = send("HEAD", "/missing-asset.js").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

/// On the wire hyper sends no body and no invented `content-length: 0` for the
/// share shell HEAD.
#[tokio::test]
async fn share_shell_head_on_the_wire_has_no_body_or_length() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let dir = std::env::temp_dir().join(format!("fvoci-static-wire-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).expect("tmpdir");
    std::fs::write(dir.join("index.html"), "<html>ok</html>").unwrap();
    let app: Router = router(app_state().await, Some(dir.clone()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(
            b"HEAD /s/some-share-token HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.unwrap();
    server.abort();
    let raw = String::from_utf8(raw).unwrap().to_ascii_lowercase();
    let (head, body) = raw.split_once("\r\n\r\n").expect("header end");
    assert!(head.starts_with("http/1.1 200"), "{head}");
    assert!(
        head.contains("\r\ncache-control: private, no-store"),
        "{head}"
    );
    assert!(head.contains("\r\nx-robots-tag: noindex"), "{head}");
    assert!(
        head.contains("\r\ncontent-type: text/html; charset=utf-8"),
        "{head}"
    );
    assert!(!head.contains("content-length"), "{head}");
    assert!(!head.contains("transfer-encoding"), "{head}");
    assert!(body.is_empty(), "{body}");
    let _ = std::fs::remove_dir_all(dir);
}

/// Source `server.ts` `ROBOTS_TXT` / `publicText`: exact body, public text
/// headers, the global security headers, served ahead of the static fallback
/// (a stray `robots.txt` in the web build cannot replace it). `/s/` stays
/// crawlable for share-card unfurl bots.
#[tokio::test]
async fn robots_txt_is_the_source_policy_with_public_text_headers() {
    let dir = std::env::temp_dir().join(format!("fvoci-static-robots-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).expect("tmpdir");
    std::fs::write(dir.join("index.html"), "<html></html>").unwrap();
    std::fs::write(dir.join("robots.txt"), "User-agent: *\nDisallow: /\n").unwrap();
    for static_dir in [None, Some(dir.clone())] {
        let app: Router = router(app_state().await, static_dir.clone());
        for method in ["GET", "HEAD"] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri("/robots.txt")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let ctx = format!("{method} static={}", static_dir.is_some());
            assert_eq!(response.status(), StatusCode::OK, "{ctx}");
            let h = response.headers().clone();
            assert_eq!(h["content-type"], "text/plain; charset=utf-8", "{ctx}");
            assert_eq!(h["cache-control"], "public, max-age=3600", "{ctx}");
            assert_eq!(h["x-content-type-options"], "nosniff", "{ctx}");
            assert_eq!(h["referrer-policy"], "no-referrer", "{ctx}");
            assert!(h.contains_key("content-security-policy"), "{ctx}");
            let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            if method == "HEAD" {
                assert!(bytes.is_empty(), "{ctx}");
                continue;
            }
            assert_eq!(
                &bytes[..],
                b"User-agent: *\nDisallow: /api/\nDisallow: /w/\nAllow: /legal/\n",
                "{ctx}"
            );
            assert!(!std::str::from_utf8(&bytes).unwrap().contains("/s/"));
        }
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// Source `apiDocs` guard `access: { auth: "session" }`: no cookie and no
/// bearer is 401 `authentication_required` as problem JSON on every docs path
/// (page, JSON, every asset) with the global CSP, and nothing of the page or
/// spec leaks, also when the SPA fallback is mounted.
#[tokio::test]
async fn api_docs_require_a_session() {
    let dir = std::env::temp_dir().join(format!("fvoci-static-docs-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).expect("tmpdir");
    std::fs::write(dir.join("index.html"), "<html></html>").unwrap();
    for static_dir in [None, Some(dir.clone())] {
        let app: Router = router(app_state().await, static_dir.clone());
        for uri in [
            "/api/docs",
            "/api/docs/json",
            "/api/docs/static/fvoci-swagger-initializer.js",
            "/api/docs/static/fvoci-swagger-theme.css",
            "/api/docs/static/swagger-ui-bundle.js",
            "/api/docs/static/swagger-ui.css",
            "/api/docs/static/swagger-ui-bundle.js.LICENSE.txt",
        ] {
            let response = app
                .clone()
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{uri}");
            let h = response.headers().clone();
            assert!(h["content-type"]
                .to_str()
                .unwrap()
                .starts_with("application/problem+json"));
            assert!(h["content-security-policy"]
                .to_str()
                .unwrap()
                .starts_with("default-src 'self';"));
            let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(json["code"], "authentication_required", "{uri}");
        }
    }
    let _ = std::fs::remove_dir_all(dir);
}

async fn probe_body(
    app: &Router,
    request: Request<Body>,
) -> (StatusCode, axum::http::HeaderMap, String) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, headers, String::from_utf8(bytes.to_vec()).unwrap())
}

fn probe_request(uri: &str, peer: Option<[u8; 4]>) -> Request<Body> {
    let mut request = Request::builder()
        .uri(uri)
        // A session cookie must not pull probes into the consent gate.
        .header("cookie", "fvoci_session=synthProbeCookie")
        .body(Body::empty())
        .unwrap();
    if let Some(ip) = peer {
        request
            .extensions_mut()
            .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                ip, 40000,
            ))));
    }
    request
}

/// `app_state` whose unreachable pool gives up quickly, so DB-backed
/// routes fail fast instead of waiting out the default acquire timeout.
async fn probe_state() -> AppState {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_millis(300))
        .connect_lazy("postgres://postgres:postgres@127.0.0.1:1/none")
        .expect("lazy pool");
    AppState {
        realtime_mode: fvoci_server::config::RealtimeMode::On,
        native_engine: None,
        auth: Arc::new(AuthService {
            db: Db::new(pool),
            password_keys: Keyring::parse(PEPPER, "test").expect("pepper"),
        }),
        ..app_state().await
    }
}

async fn probe_router(allow: &str) -> Router {
    use fvoci_server::http::probes::{MetricsAllowList, Observability, ObservabilitySettings};
    fvoci_server::http::router_with_observability(
        probe_state().await,
        None,
        Arc::new(fvoci_server::integrations::Integrations::disabled()),
        Arc::new(fvoci_server::identity::Identity::disabled(
            "http://localhost",
        )),
        Arc::new(Observability::new(ObservabilitySettings {
            allow: MetricsAllowList::parse(Some(allow)).unwrap(),
            outbox_consumers: Vec::new(),
            refresh_interval: std::time::Duration::ZERO,
        })),
    )
}

/// Source `/health`: always `{"ok":true}`, no session lookup.
#[tokio::test]
async fn health_probe_is_ok_without_database_or_session() {
    let app = probe_router("").await;
    let (status, headers, body) = probe_body(&app, probe_request("/health", None)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers["content-type"]
        .to_str()
        .unwrap()
        .starts_with("application/json"));
    assert_eq!(body, r#"{"ok":true}"#);
}

/// Source `/ready` with PostgreSQL down: 503 and the failing check.
#[tokio::test]
async fn ready_probe_reports_database_down() {
    let app = probe_router("").await;
    let started = std::time::Instant::now();
    let (status, _, body) = probe_body(&app, probe_request("/ready", None)).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        json,
        serde_json::json!({"ok": false, "checks": {"pg": false}})
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(4),
        "{:?}",
        started.elapsed()
    );
    assert!(
        !body.contains("127.0.0.1:1") && !body.contains("postgres"),
        "{body}"
    );
}

/// Source `metricsAllowed`: unset, a peer outside the list, or no socket
/// peer at all get the generic 404 problem, never the metrics.
#[tokio::test]
async fn metrics_probe_is_hidden_outside_allow_list() {
    for (allow, peer) in [
        ("", Some([127, 0, 0, 1])),
        ("10.0.0.0/8", Some([127, 0, 0, 1])),
        ("127.0.0.1/32", None),
    ] {
        let app = probe_router(allow).await;
        let mut request = probe_request("/metrics", peer);
        request
            .headers_mut()
            .insert("x-forwarded-for", "10.1.1.1".parse().unwrap());
        let (status, _, body) = probe_body(&app, request).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{allow} {peer:?}");
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(json["code"], "not_found");
        assert!(!body.contains("fvoci_"), "{body}");
    }
}

/// Non-GET/HEAD methods on the probes get the same generic 404 as a denied
/// peer, never a 405 that would announce the route.
#[tokio::test]
async fn probe_other_methods_get_generic_not_found() {
    let app = probe_router("127.0.0.1/32").await;
    let (_, _, denied) = probe_body(&probe_router("").await, probe_request("/metrics", None)).await;
    for (method, uri) in [
        ("POST", "/metrics"),
        ("DELETE", "/metrics"),
        ("POST", "/health"),
        ("PUT", "/ready"),
    ] {
        let mut request = probe_request(uri, Some([127, 0, 0, 1]));
        *request.method_mut() = method.parse().unwrap();
        let (status, headers, body) = probe_body(&app, request).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {uri}");
        assert!(headers.get("allow").is_none(), "{method} {uri}");
        assert_eq!(body, denied, "{method} {uri}");
    }
}

/// An allowed peer gets the Prometheus/OpenMetrics text with the source
/// metric names; HTTP labels carry route templates, not concrete paths.
#[tokio::test]
async fn metrics_probe_exports_text_format_for_allowed_peer() {
    let app = probe_router("172.30.0.0/24,127.0.0.1/32").await;
    let (status, _, _) = probe_body(&app, probe_request("/health", None)).await;
    assert_eq!(status, StatusCode::OK);
    let secret_path = "/api/v1/share/synthProbeShareTok";
    probe_body(&app, probe_request(secret_path, None)).await;
    let (status, headers, body) =
        probe_body(&app, probe_request("/metrics", Some([127, 0, 0, 1]))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        headers["content-type"],
        "application/openmetrics-text; version=1.0.0; charset=utf-8"
    );
    for name in [
        "# TYPE fvoci_http_request_duration_seconds histogram",
        "# TYPE fvoci_outbox_lag_seconds gauge",
        "# TYPE fvoci_outbox_xmin_stall_seconds gauge",
        "# TYPE fvoci_task_stream_subscribers gauge",
        "# TYPE fvoci_db_pool_connections gauge",
        "# TYPE fvoci_db_metrics_refresh_failures counter",
        "# TYPE fvoci_db_metrics_last_success_timestamp_seconds gauge",
        "# TYPE fvoci_process_resident_memory_bytes gauge",
        "# TYPE fvoci_collab_helper_resident_memory_bytes gauge",
        "# TYPE fvoci_collab_helper_memory_budget_bytes gauge",
        "fvoci_db_pool_max_connections 1",
        "fvoci_task_stream_subscribers 0",
        "fvoci_collab_helper_resident_memory_bytes 0",
    ] {
        assert!(body.contains(name), "{name} missing:\n{body}");
    }
    // PostgreSQL is unreachable: the outbox gauges are unknown, not a
    // healthy 0, and the failed refresh is counted.
    for (name, value) in [
        ("fvoci_outbox_lag_seconds", "NaN"),
        ("fvoci_outbox_xmin_stall_seconds", "NaN"),
        ("fvoci_db_metrics_refresh_failures_total", "1"),
        ("fvoci_db_metrics_last_success_timestamp_seconds", "0.0"),
        // No collaboration hub in this router.
        ("fvoci_collab_helper_memory_budget_bytes", "NaN"),
    ] {
        assert_eq!(metric_sample(&body, name), value, "{name}\n{body}");
    }
    let rss: f64 = metric_sample(&body, "fvoci_process_resident_memory_bytes")
        .parse()
        .unwrap();
    assert!(rss > 1_000_000.0, "{rss}");
    assert!(
        body.contains(
            r#"fvoci_http_request_duration_seconds_count{method="GET",route="/health",status="200"} 1"#
        ),
        "{body}"
    );
    assert!(body.contains(r#"route="/api/v1/share/{token}""#), "{body}");
    assert!(!body.contains("synthProbeShareTok"), "{body}");
    assert!(body.ends_with("# EOF\n"), "{body}");
    // IPv4-mapped dual-stack peer inside the list.
    let mut request = probe_request("/metrics", None);
    request.extensions_mut().insert(axum::extract::ConnectInfo(
        "[::ffff:172.30.0.7]:40000"
            .parse::<std::net::SocketAddr>()
            .unwrap(),
    ));
    let (status, _, _) = probe_body(&app, request).await;
    assert_eq!(status, StatusCode::OK);
}

fn metric_sample<'a>(body: &'a str, name: &str) -> &'a str {
    body.lines()
        .find_map(|line| line.strip_prefix(name)?.strip_prefix(' '))
        .unwrap_or_else(|| panic!("{name} missing:\n{body}"))
}

/// Over real TCP the allowlist sees the socket peer (127.0.0.1): listed it
/// scrapes, unlisted it gets the 404 even when `X-Forwarded-For` names an
/// allowed address, and a listed peer is not refused by a foreign XFF.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn metrics_allow_list_uses_tcp_peer_not_forwarded_for() {
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    for (allow, forwarded, expected) in [
        ("127.0.0.1/32", None, 200),
        ("127.0.0.1/32", Some("203.0.113.9"), 200),
        ("10.0.0.0/8", Some("10.1.1.1"), 404),
        ("10.0.0.0/8", Some("10.1.1.1, 127.0.0.1"), 404),
        ("", Some("127.0.0.1"), 404),
    ] {
        let (addr, task) = serve_on_loopback(probe_router(allow).await).await;
        let mut request = client.get(format!("http://{addr}/metrics"));
        if let Some(xff) = forwarded {
            request = request
                .header("x-forwarded-for", xff)
                .header("x-real-ip", xff)
                .header("forwarded", format!("for={xff}"));
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        let body = response.text().await.unwrap();
        assert_eq!(status, expected, "{allow} {forwarded:?}: {body}");
        assert_eq!(body.contains("fvoci_"), expected == 200, "{body}");
        task.abort();
    }
}

/// Source `SHELL_EXCLUDED_PREFIXES`: probe sub-paths never get the SPA shell.
#[tokio::test]
async fn probe_subpaths_do_not_serve_spa_shell() {
    let dir = std::env::temp_dir().join(format!("fvoci-probe-shell-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("index.html"), "<html>shell</html>").unwrap();
    let app = router(app_state().await, Some(dir.clone()));
    let (status, _, body) = probe_body(&app, probe_request("/health", None)).await;
    assert_eq!((status, body.as_str()), (StatusCode::OK, r#"{"ok":true}"#));
    for uri in ["/health/x", "/ready/", "/metrics/extra"] {
        let (status, _, body) = probe_body(&app, probe_request(uri, None)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
        assert!(!body.contains("shell"), "{uri}: {body}");
    }
    let (status, _, body) = probe_body(&app, probe_request("/projects", None)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("shell"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Serves `router` on an ephemeral loopback port.
async fn serve_on_loopback(app: Router) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    (addr, task)
}

/// `fvoci-server healthcheck` with only `FVOCI_BIND`: no database URL,
/// keys or config, so exit codes prove it branches before server startup.
fn healthcheck(bind: Option<&str>, mode: Option<&str>) -> std::process::Output {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_fvoci-server"));
    command.env_clear().arg("healthcheck");
    if let Some(mode) = mode {
        command.arg(mode);
    }
    if let Some(bind) = bind {
        command.env("FVOCI_BIND", bind);
    }
    command.output().expect("run healthcheck")
}

async fn healthcheck_async(
    bind: Option<String>,
    mode: Option<&'static str>,
) -> std::process::Output {
    tokio::task::spawn_blocking(move || healthcheck(bind.as_deref(), mode))
        .await
        .unwrap()
}

/// Source `cli.ts` healthcheck: 0 only for a 2xx `/ready`, 1 for 503, no
/// listener, an unusable address or an unknown/split-role mode.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn healthcheck_cli_exit_codes() {
    let ok = Router::new().route(
        "/ready",
        axum::routing::get(|| async { axum::Json(serde_json::json!({"ok": true})) }),
    );
    let (ok_addr, ok_task) = serve_on_loopback(ok).await;
    let out = healthcheck_async(Some(ok_addr.to_string()), None).await;
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let wildcard = format!("0.0.0.0:{}", ok_addr.port());
    let out = healthcheck_async(Some(wildcard), None).await;
    assert_eq!(out.status.code(), Some(0), "{out:?}");

    // The real router with PostgreSQL down answers 503.
    let (down_addr, down_task) = serve_on_loopback(router(probe_state().await, None)).await;
    let out = healthcheck_async(Some(down_addr.to_string()), None).await;
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("503"),
        "{out:?}"
    );

    ok_task.abort();
    down_task.abort();
    let _ = ok_task.await;
    let out = healthcheck_async(Some(ok_addr.to_string()), None).await;
    assert_eq!(out.status.code(), Some(1), "{out:?}");

    for (bind, mode) in [
        (None, None),
        (Some("127.0.0.1:0".to_string()), None),
        (Some("garbage".to_string()), None),
        (Some(down_addr.to_string()), Some("worker")),
        (Some(down_addr.to_string()), Some("compact")),
        (Some(down_addr.to_string()), Some("thumbnail")),
        (Some(down_addr.to_string()), Some("bogus")),
    ] {
        let out = healthcheck_async(bind.clone(), mode).await;
        assert_eq!(out.status.code(), Some(1), "{bind:?} {mode:?} {out:?}");
        assert!(out.stdout.is_empty(), "{out:?}");
    }
}
