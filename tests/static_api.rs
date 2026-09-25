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
        },
        collab: None,
        meili: None,
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
