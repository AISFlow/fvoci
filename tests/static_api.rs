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
        document_convert: None,
        markdown: Some(
            fvoci_server::documents::markdown_helper::MarkdownHelper::new(env!(
                "CARGO_BIN_EXE_fvoci-server"
            )),
        ),
        import_wake: None,
        import_extractor_available: false,
        quota: Default::default(),
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
