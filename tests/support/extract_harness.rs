use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::Router;
use fvoci_server::auth::password::Keyring;
use fvoci_server::auth::AuthService;
use fvoci_server::attachments::{spawn_extract_job, ExtractJobSettings, LocalStorage, UploadLimits};
use fvoci_server::db::attachment_extract::fetch_extract_state;
use fvoci_server::db::{migrate, pool, Db};
use fvoci_server::http::rate_limit::RateLimiter;
use fvoci_server::http::state::AppState;
use rand::RngCore;
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

const PEPPER: &str =
    r#"{"test":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#;

pub struct TestDb {
    pub admin_url: String,
    pub app_url: String,
    db_name: String,
    role_name: String,
}

impl TestDb {
    pub async fn bootstrap() -> Self {
        let admin_base = std::env::var("TEST_DATABASE_URL")
            .or_else(|_| std::env::var("FVOCI_TEST_DATABASE_URL"))
            .expect("TEST_DATABASE_URL missing");

        let db_name = format!("fvoci_ext_{}", Uuid::now_v7().simple());
        let role_name = format!("fvoci_app_{}", db_name.replace('-', "_"));
        let mut password_bytes = [0u8; 24];
        rand::rng().fill_bytes(&mut password_bytes);
        let role_password = hex::encode(password_bytes);
        let server_url = server_db_url(&admin_base);

        let admin_pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(&server_url)
            .await
            .expect("connect admin");
        sqlx::query(&format!("CREATE DATABASE \"{}\"", db_name))
            .execute(&admin_pool)
            .await
            .expect("create database");
        admin_pool.close().await;

        let admin_url = join_db_url(&server_url, &db_name);
        migrate::run_migrations(&admin_url).await.expect("migrate");

        let migration_pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(&admin_url)
            .await
            .expect("connect migration db");
        sqlx::query(&format!(
            "CREATE ROLE \"{}\" LOGIN PASSWORD '{}' NOSUPERUSER NOBYPASSRLS",
            role_name, role_password
        ))
        .execute(&migration_pool)
        .await
        .expect("create role");
        apply_grants(&migration_pool, &role_name).await;
        migration_pool.close().await;

        let mut app = url::Url::parse(&admin_url).expect("database url");
        app.set_username(&role_name).ok();
        app.set_password(Some(&role_password)).ok();

        Self {
            admin_url,
            app_url: app.to_string(),
            db_name,
            role_name,
        }
    }

    pub async fn cleanup(self) {
        let server_url = server_db_url(&self.admin_url);
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(&server_url)
            .await
            .ok();
        if let Some(pool) = pool {
            let _ = sqlx::query(&format!(
                "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '{}'",
                self.db_name
            ))
            .execute(&pool)
            .await;
            let _ = sqlx::query(&format!("DROP DATABASE IF EXISTS \"{}\"", self.db_name))
                .execute(&pool)
                .await;
            let _ = sqlx::query(&format!("DROP ROLE IF EXISTS \"{}\"", self.role_name))
                .execute(&pool)
                .await;
            pool.close().await;
        }
    }
}

fn server_db_url(url: &str) -> String {
    let parsed = url::Url::parse(url).expect("database url");
    let mut server = parsed;
    server.set_path("");
    server.to_string().trim_end_matches('/').to_string()
}

fn join_db_url(server_url: &str, db_name: &str) -> String {
    let mut parsed = url::Url::parse(server_url).expect("server url");
    parsed.set_path(&format!("/{}", db_name));
    parsed.to_string()
}

async fn apply_grants(pool: &PgPool, role_name: &str) {
    let quoted_role = format!("\"{}\"", role_name);
    let grants =
        include_str!("../../scripts/grant-app-role.sql").replace(":\"app_role\"", &quoted_role);
    for statement in grants.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        sqlx::query(statement).execute(pool).await.expect("grant");
    }
}

fn test_peer() -> std::net::SocketAddr {
    std::net::SocketAddr::from(([203, 0, 113, 11], 42425))
}

pub async fn app_pool(url: &str) -> PgPool {
    pool::connect_app(url).await.expect("connect app")
}

pub async fn app_state_with_storage(app_url: &str, storage_root: PathBuf) -> AppState {
    let pool = pool::connect_app(app_url).await.expect("app pool");
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
        storage: LocalStorage::new(storage_root.clone()),
        upload: UploadLimits {
            part_size_bytes: fvoci_server::config::DEFAULT_UPLOAD_PART_SIZE_BYTES,
            max_file_size_bytes: fvoci_server::config::DEFAULT_UPLOAD_MAX_FILE_SIZE_BYTES,
            create_rate_per_5min: fvoci_server::config::DEFAULT_UPLOAD_CREATE_RATE_PER_5MIN,
        },
        collab: None,
    }
}

pub fn app_router(state: AppState) -> Router {
    fvoci_server::http::router(state, None)
}

pub async fn request(
    app: Router,
    method: &str,
    path: &str,
    body: Option<Vec<u8>>,
    content_type: Option<&str>,
    cookie: Option<&str>,
    extra_headers: &[(&str, &str)],
) -> (StatusCode, Vec<u8>, HeaderMap) {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", format!("fvoci_session={}", cookie));
    }
    for (name, value) in extra_headers {
        builder = builder.header(*name, *value);
    }
    if let Some(ct) = content_type {
        builder = builder.header("content-type", ct);
    }
    let request = builder
        .body(body.map(Body::from).unwrap_or_else(Body::empty))
        .unwrap();
    let mut request = request;
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(test_peer()));
    let response = app.oneshot(request).await.expect("response");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap_or_default();
    (status, bytes.to_vec(), headers)
}

pub async fn json_request(
    app: Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    cookie: Option<&str>,
) -> (StatusCode, Value, Option<String>) {
    let bytes = body.map(|v| v.to_string().into_bytes());
    let (status, raw, headers) = request(
        app,
        method,
        path,
        bytes,
        Some("application/json"),
        cookie,
        &[],
    )
    .await;
    let set_cookie = headers
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let json = if raw.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&raw).unwrap_or(json!({}))
    };
    (status, json, set_cookie)
}

pub fn extract_session_cookie(set_cookie: &str) -> String {
    set_cookie
        .split(';')
        .next()
        .unwrap_or("")
        .split('=')
        .nth(1)
        .unwrap_or("")
        .to_string()
}

pub async fn setup_session(harness: &TestDb) -> (Router, String, Uuid, Uuid, PathBuf) {
    let storage_root = std::env::temp_dir().join(format!("fvoci-ext-store-{}", Uuid::now_v7()));
    let app = app_router(app_state_with_storage(&harness.app_url, storage_root.clone()).await);
    let (_, _, cookie_hdr) = json_request(
        app.clone(),
        "POST",
        "/api/v1/setup",
        Some(json!({
            "email": "owner@example.com",
            "password": "supersecret1",
            "givenName": "Owner",
            "workspaceSlug": "acme",
            "workspaceName": "Acme"
        })),
        None,
    )
    .await;
    let cookie = extract_session_cookie(cookie_hdr.as_ref().unwrap());
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let ids: (Uuid, Uuid) = sqlx::query_as(
        "SELECT u.id, w.id FROM fvoci.users u CROSS JOIN fvoci.workspaces w WHERE w.slug = 'acme' LIMIT 1",
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    admin.close().await;
    (app, cookie, ids.0, ids.1, storage_root)
}

pub async fn create_document(app: &Router, cookie: &str, workspace_id: Uuid) -> String {
    let (status, body, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"parentId": null, "title": "Doc"})),
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    body["id"].as_str().unwrap().to_string()
}

pub struct UploadSession {
    pub attachment_id: String,
}

pub async fn upload_bytes(
    app: &Router,
    cookie: &str,
    workspace_id: Uuid,
    document_id: &str,
    name: &str,
    bytes: &[u8],
    declared_mime: Option<&str>,
) -> UploadSession {
    let mut body = json!({
        "name": name,
        "sizeBytes": bytes.len(),
    });
    if let Some(mime) = declared_mime {
        body["declaredMime"] = json!(mime);
    }
    let (status, created, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/uploads"),
        Some(body),
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create: {:?}", created);
    let attachment_id = created["attachmentId"].as_str().unwrap().to_string();
    let part_url = created["parts"][0]["url"].as_str().unwrap();
    let (status, _, headers) = request(
        app.clone(),
        "PUT",
        part_url,
        Some(bytes.to_vec()),
        Some("application/octet-stream"),
        Some(cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let etag = headers
        .get("etag")
        .or_else(|| headers.get("ETag"))
        .and_then(|v| v.to_str().ok())
        .expect("missing etag");
    let (status, completed, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/complete"),
        Some(json!({ "parts": [{ "partNumber": 1, "etag": etag }] })),
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "complete: {:?}", completed);
    UploadSession { attachment_id }
}

pub fn require_extractor_bin() -> PathBuf {
    let raw = std::env::var("FVOCI_EXTRACTOR_BIN").unwrap_or_else(|_| {
        panic!(
            "FVOCI_EXTRACTOR_BIN is required for extract-native-tests; build document-extract first"
        )
    });
    let path = PathBuf::from(raw.trim());
    fvoci_server::attachments::validate_extractor_bin(&path)
        .unwrap_or_else(|err| panic!("invalid FVOCI_EXTRACTOR_BIN ({}): {}", path.display(), err));
    path
}

pub fn extract_job_settings(extractor_bin: PathBuf) -> ExtractJobSettings {
    ExtractJobSettings {
        extractor_bin,
        limits: document_extract_client::Limits::for_tests(),
        poll_interval: Duration::from_millis(100),
        retry_backoff: Duration::from_millis(100),
    }
}

pub fn idle_extract_job_settings(extractor_bin: PathBuf) -> ExtractJobSettings {
    ExtractJobSettings {
        extractor_bin,
        limits: document_extract_client::Limits::for_tests(),
        poll_interval: Duration::from_secs(600),
        retry_backoff: Duration::from_secs(600),
    }
}

pub async fn wait_for_extract(
    pool: &PgPool,
    workspace_id: Uuid,
    attachment_id: Uuid,
    timeout: Duration,
) -> String {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        let state = fetch_extract_state(pool, workspace_id, attachment_id)
            .await
            .unwrap()
            .expect("attachment row");
        if state.extract_status != "pending" {
            return state.extract_text;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("extract did not finish before deadline");
}

pub async fn download_original(
    app: &Router,
    cookie: &str,
    workspace_id: Uuid,
    attachment_id: &str,
) -> Vec<u8> {
    let (status, bytes, _) = request(
        app.clone(),
        "GET",
        &format!(
            "/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/download"
        ),
        None,
        None,
        Some(cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "download failed");
    bytes
}

pub async fn spawn_extract_for_storage(
    harness: &TestDb,
    storage_root: &PathBuf,
    settings: ExtractJobSettings,
) -> fvoci_server::attachments::ExtractJobHandle {
    let pool = app_pool(&harness.app_url).await;
    spawn_extract_job(settings, pool, LocalStorage::new(storage_root.clone()))
}
