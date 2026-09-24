#![cfg(feature = "db-tests")]

use std::sync::Arc;

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use chrono::{Duration as ChronoDuration, Utc};
use fvoci_server::auth::password::Keyring;
use fvoci_server::auth::AuthService;
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

// PNG magic bytes + minimal payload
const PNG_BYTES: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
    0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
    0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
    0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
    0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

fn test_peer() -> std::net::SocketAddr {
    std::net::SocketAddr::from(([203, 0, 113, 11], 42425))
}

struct TestDb {
    admin_url: String,
    app_url: String,
    db_name: String,
    role_name: String,
}

impl TestDb {
    async fn bootstrap() -> Self {
        let admin_base = std::env::var("TEST_DATABASE_URL")
            .or_else(|_| std::env::var("FVOCI_TEST_DATABASE_URL"))
            .expect("TEST_DATABASE_URL missing");

        let db_name = format!("fvoci_att_{}", Uuid::now_v7().simple());
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

    async fn cleanup(self) {
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
        include_str!("../scripts/grant-app-role.sql").replace(":\"app_role\"", &quoted_role);
    for statement in grants.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        sqlx::query(statement).execute(pool).await.expect("grant");
    }
}

async fn app_state(app_url: &str) -> AppState {
    let pool = pool::connect_app(app_url).await.expect("app pool");
    let storage_root = std::env::temp_dir().join(format!("fvoci-att-store-{}", Uuid::now_v7()));
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
        storage: fvoci_server::attachments::LocalStorage::new(storage_root),
        upload: fvoci_server::attachments::UploadLimits {
            part_size_bytes: fvoci_server::config::DEFAULT_UPLOAD_PART_SIZE_BYTES,
            max_file_size_bytes: fvoci_server::config::DEFAULT_UPLOAD_MAX_FILE_SIZE_BYTES,
            create_rate_per_5min: fvoci_server::config::DEFAULT_UPLOAD_CREATE_RATE_PER_5MIN,
        },
    }
}

fn app_router(state: AppState) -> axum::Router {
    fvoci_server::http::router(state, None)
}

async fn request(
    app: axum::Router,
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

async fn json_request(
    app: axum::Router,
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

fn extract_session_cookie(set_cookie: &str) -> String {
    set_cookie
        .split(';')
        .next()
        .unwrap_or("")
        .split('=')
        .nth(1)
        .unwrap_or("")
        .to_string()
}

async fn setup_session(harness: &TestDb) -> (axum::Router, String, Uuid, Uuid) {
    let app = app_router(app_state(&harness.app_url).await);
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
    (app, cookie, ids.0, ids.1)
}

async fn create_document(app: &axum::Router, cookie: &str, workspace_id: Uuid) -> String {
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

async fn create_user_with_role(
    harness: &TestDb,
    email: &str,
    workspace_id: Uuid,
    role: &str,
) -> (Uuid, String) {
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let user_id = Uuid::now_v7();
    let hash = fvoci_server::auth::password::hash_password(
        "supersecret1",
        &Keyring::parse(PEPPER, "test").unwrap(),
    )
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO fvoci.users (id, email, password_hash, given_name) VALUES ($1, $2, $3, $4)",
    )
    .bind(user_id)
    .bind(email)
    .bind(&hash)
    .bind("User")
    .execute(&admin)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO fvoci.memberships (workspace_id, user_id, role) VALUES ($1, $2, $3)",
    )
    .bind(workspace_id)
    .bind(user_id)
    .bind(role)
    .execute(&admin)
    .await
    .unwrap();
    admin.close().await;

    let pool = pool::connect_app(&harness.app_url).await.unwrap();
    let token = fvoci_server::auth::token::new_token();
    let expires = Utc::now() + ChronoDuration::days(30);
    let mut tx = pool.begin().await.unwrap();
    fvoci_server::db::identity::create_session(
        &mut tx,
        Uuid::now_v7(),
        user_id,
        &token.hash,
        expires,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    pool.close().await;
    (user_id, token.token)
}

struct UploadSession {
    attachment_id: String,
    etag: String,
}

async fn upload_bytes(
    app: &axum::Router,
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
        .unwrap_or_else(|| {
            panic!("missing etag header");
        })
        .to_string();
    let (status, completed, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/complete"),
        Some(json!({ "parts": [{ "partNumber": 1, "etag": etag }] })),
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "complete: {:?}", completed);
    UploadSession {
        attachment_id,
        etag,
    }
}

#[tokio::test]
async fn wiki_attachment_round_trip_download_and_meta() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _owner, workspace_id) = setup_session(&harness).await;
    let document_id = create_document(&app, &cookie, workspace_id).await;
    let uploaded = upload_bytes(
        &app,
        &cookie,
        workspace_id,
        &document_id,
        "pixel.png",
        PNG_BYTES,
        Some("image/png"),
    )
    .await;

    let (status, meta, _) = json_request(
        app.clone(),
        "GET",
        &format!(
            "/api/v1/workspaces/{workspace_id}/attachments/{}",
            uploaded.attachment_id
        ),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(meta["name"], "pixel.png");
    assert_eq!(meta["sizeBytes"], PNG_BYTES.len());
    assert_eq!(meta["image"], true);
    assert_eq!(meta["scanStatus"], "skipped");
    assert_eq!(meta["preview"], Value::Null);
    assert!(meta["completedAt"].is_string());

    let (status, body, headers) = request(
        app.clone(),
        "GET",
        &format!(
            "/api/v1/workspaces/{workspace_id}/attachments/{}/download",
            uploaded.attachment_id
        ),
        None,
        None,
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, PNG_BYTES);
    assert_eq!(
        headers.get("content-type").unwrap().to_str().unwrap(),
        "application/octet-stream"
    );
    assert!(headers.get("content-disposition").unwrap().to_str().unwrap().contains("pixel.png"));
    harness.cleanup().await;
}

#[tokio::test]
async fn guest_and_other_member_access_controls() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, _owner, workspace_id) = setup_session(&harness).await;
    let document_id = create_document(&app, &owner_cookie, workspace_id).await;
    let uploaded = upload_bytes(
        &app,
        &owner_cookie,
        workspace_id,
        &document_id,
        "file.bin",
        b"hello",
        None,
    )
    .await;

    let (_guest_id, guest_cookie) =
        create_user_with_role(&harness, "guest@example.com", workspace_id, "guest").await;
    let (_member_id, member_cookie) =
        create_user_with_role(&harness, "member@example.com", workspace_id, "member").await;

    let (status, _, _) = json_request(
        app.clone(),
        "GET",
        &format!(
            "/api/v1/workspaces/{workspace_id}/attachments/{}/download",
            uploaded.attachment_id
        ),
        None,
        Some(&guest_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _, _) = request(
        app.clone(),
        "PUT",
        &format!(
            "/api/v1/workspaces/{workspace_id}/attachments/{}/parts/1",
            uploaded.attachment_id
        ),
        Some(b"hack".to_vec()),
        Some("application/octet-stream"),
        Some(&member_cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _, _) = json_request(
        app.clone(),
        "GET",
        &format!(
            "/api/v1/workspaces/{workspace_id}/attachments/{}/download",
            uploaded.attachment_id
        ),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    harness.cleanup().await;
}

#[tokio::test]
async fn download_range_206_and_invalid_416() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _owner, workspace_id) = setup_session(&harness).await;
    let document_id = create_document(&app, &cookie, workspace_id).await;
    let payload = b"0123456789";
    let uploaded = upload_bytes(
        &app,
        &cookie,
        workspace_id,
        &document_id,
        "ten.bin",
        payload,
        None,
    )
    .await;

    let (status, body, headers) = request(
        app.clone(),
        "GET",
        &format!(
            "/api/v1/workspaces/{workspace_id}/attachments/{}/download",
            uploaded.attachment_id
        ),
        None,
        None,
        Some(&cookie),
        &[("range", "bytes=2-5")],
    )
    .await;
    assert_eq!(status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(body, b"2345");
    assert_eq!(
        headers.get("content-range").unwrap().to_str().unwrap(),
        "bytes 2-5/10"
    );

    let (status, _, _) = request(
        app.clone(),
        "GET",
        &format!(
            "/api/v1/workspaces/{workspace_id}/attachments/{}/download",
            uploaded.attachment_id
        ),
        None,
        None,
        Some(&cookie),
        &[("range", "bytes=20-")],
    )
    .await;
    assert_eq!(status, StatusCode::RANGE_NOT_SATISFIABLE);
    harness.cleanup().await;
}

#[tokio::test]
async fn declared_size_mismatch_deletes_row_and_object() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _owner, workspace_id) = setup_session(&harness).await;
    let document_id = create_document(&app, &cookie, workspace_id).await;
    let (status, created, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/uploads"),
        Some(json!({ "name": "bad.bin", "sizeBytes": 10 })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let attachment_id = created["attachmentId"].as_str().unwrap();
    let part_url = created["parts"][0]["url"].as_str().unwrap();
    let (status, _, headers) = request(
        app.clone(),
        "PUT",
        part_url,
        Some(b"short".to_vec()),
        Some("application/octet-stream"),
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let etag = headers.get("etag").unwrap().to_str().unwrap();
    let (status, body, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/complete"),
        Some(json!({ "parts": [{ "partNumber": 1, "etag": etag }] })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "invalid_input");

    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let count: (i64,) =
        sqlx::query_as("SELECT count(*) FROM fvoci.attachments WHERE id = $1::uuid")
            .bind(attachment_id)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(count.0, 0);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn hwp_upload_sets_pending_extract_status() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _owner, workspace_id) = setup_session(&harness).await;
    let document_id = create_document(&app, &cookie, workspace_id).await;
    let uploaded = upload_bytes(
        &app,
        &cookie,
        workspace_id,
        &document_id,
        "report.hwp",
        b"HWP placeholder",
        Some("application/x-hwp"),
    )
    .await;

    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let row: (String, String) = sqlx::query_as(
        "SELECT extract_status, extract_text FROM fvoci.attachments WHERE id = $1",
    )
    .bind(Uuid::parse_str(&uploaded.attachment_id).unwrap())
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(row.0, "pending");
    assert_eq!(row.1, "");
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn revoked_session_cannot_complete_upload() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let document_id = create_document(&app, &cookie, workspace_id).await;
    let (status, created, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/uploads"),
        Some(json!({ "name": "revoke.bin", "sizeBytes": 5 })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let attachment_id = created["attachmentId"].as_str().unwrap();
    let part_url = created["parts"][0]["url"].as_str().unwrap();
    let (status, _, headers) = request(
        app.clone(),
        "PUT",
        part_url,
        Some(b"12345".to_vec()),
        Some("application/octet-stream"),
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let etag = headers.get("etag").unwrap().to_str().unwrap();

    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    sqlx::query("UPDATE fvoci.sessions SET revoked_at = now() WHERE user_id = $1")
        .bind(owner_id)
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;

    let (status, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/complete"),
        Some(json!({ "parts": [{ "partNumber": 1, "etag": etag }] })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    harness.cleanup().await;
}

#[tokio::test]
async fn attachment_completed_event_is_recorded() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _owner, workspace_id) = setup_session(&harness).await;
    let document_id = create_document(&app, &cookie, workspace_id).await;
    let uploaded = upload_bytes(
        &app,
        &cookie,
        workspace_id,
        &document_id,
        "evt.bin",
        b"event",
        None,
    )
    .await;

    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let count: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.events WHERE verb = 'attachment.completed' AND target_id = $1",
    )
    .bind(Uuid::parse_str(&uploaded.attachment_id).unwrap())
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(count.0, 1);
    admin.close().await;
    harness.cleanup().await;
}
