#![cfg(feature = "db-tests")]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use chrono::{Duration as ChronoDuration, Utc};
use futures_util::stream;
use futures_util::StreamExt;
use fvoci_server::auth::password::Keyring;
use fvoci_server::auth::token::hash_token;
use fvoci_server::auth::AuthService;
use fvoci_server::db::attachments::{
    authorize_upload_part, commit_upload_part, test_barrier, AttachmentDbError,
};
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
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
    0x42, 0x60, 0x82,
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
    fvoci_server::db::migrate::apply_app_role_grants(pool, role_name)
        .await
        .expect("grant");
}

async fn app_state(app_url: &str) -> AppState {
    let storage_root = std::env::temp_dir().join(format!("fvoci-att-store-{}", Uuid::now_v7()));
    app_state_with_storage(app_url, storage_root).await
}

async fn app_state_with_storage(app_url: &str, storage_root: PathBuf) -> AppState {
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
        storage: fvoci_server::attachments::LocalStorage::new(storage_root),
        upload: fvoci_server::attachments::UploadLimits {
            part_size_bytes: fvoci_server::config::DEFAULT_UPLOAD_PART_SIZE_BYTES,
            max_file_size_bytes: fvoci_server::config::DEFAULT_UPLOAD_MAX_FILE_SIZE_BYTES,
            create_rate_per_5min: fvoci_server::config::DEFAULT_UPLOAD_CREATE_RATE_PER_5MIN,
        },
        collab: None,
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
    sqlx::query("INSERT INTO fvoci.memberships (workspace_id, user_id, role) VALUES ($1, $2, $3)")
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

async fn session_id_for_cookie(harness: &TestDb, cookie: &str) -> Uuid {
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let id: Uuid = sqlx::query_scalar("SELECT id FROM fvoci.sessions WHERE token_hash = $1")
        .bind(hash_token(cookie))
        .fetch_one(&admin)
        .await
        .unwrap();
    admin.close().await;
    id
}

async fn install_insert_fail_trigger(admin: &PgPool, target: &str, fn_name: &str) {
    sqlx::query(&format!(
        r#"
        CREATE OR REPLACE FUNCTION fvoci.{fn_name}()
        RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN
            RAISE EXCEPTION 'insert blocked on {target}';
        END;
        $$;
        "#,
    ))
    .execute(admin)
    .await
    .unwrap();
    sqlx::query(&format!(
        r#"
        CREATE TRIGGER fvoci_{fn_name}
        BEFORE INSERT ON fvoci.{target}
        FOR EACH ROW EXECUTE FUNCTION fvoci.{fn_name}()
        "#,
    ))
    .execute(admin)
    .await
    .unwrap();
}

async fn begin_upload(
    app: &axum::Router,
    cookie: &str,
    workspace_id: Uuid,
    document_id: &str,
    name: &str,
    bytes: &[u8],
) -> (String, String, String) {
    let (status, created, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/uploads"),
        Some(json!({ "name": name, "sizeBytes": bytes.len() })),
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let attachment_id = created["attachmentId"].as_str().unwrap().to_string();
    let part_url = created["parts"][0]["url"].as_str().unwrap().to_string();
    (attachment_id, part_url, name.to_string())
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
    assert!(meta["createdAt"].is_string());
    assert_eq!(meta["id"], uploaded.attachment_id);

    let (status, replay, _) = json_request(
        app.clone(),
        "POST",
        &format!(
            "/api/v1/workspaces/{workspace_id}/attachments/{}/complete",
            uploaded.attachment_id
        ),
        Some(json!({ "parts": [{ "partNumber": 1, "etag": uploaded.etag }] })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay["id"], uploaded.attachment_id);
    assert_eq!(replay["preview"], Value::Null);

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
    assert!(headers
        .get("content-disposition")
        .unwrap()
        .to_str()
        .unwrap()
        .contains("pixel.png"));
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let events: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.events WHERE verb = 'attachment.completed' AND target_id = $1",
    )
    .bind(Uuid::parse_str(&uploaded.attachment_id).unwrap())
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(events.0, 1, "successful complete replays the event once");
    admin.close().await;
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
        &[("range", "bytes=20-")],
    )
    .await;
    assert_eq!(status, StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(
        headers.get("content-range").unwrap().to_str().unwrap(),
        "bytes */10"
    );
    assert_eq!(
        headers.get("content-type").unwrap().to_str().unwrap(),
        "application/problem+json"
    );
    let problem: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(problem["code"], "range_not_satisfiable");
    assert_eq!(
        headers
            .get("x-content-type-options")
            .unwrap()
            .to_str()
            .unwrap(),
        "nosniff"
    );

    let (status, _, headers) = request(
        app.clone(),
        "HEAD",
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
    assert_eq!(
        headers.get("content-range").unwrap().to_str().unwrap(),
        "bytes */10"
    );

    let (status, _, headers) = request(
        app.clone(),
        "GET",
        &format!(
            "/api/v1/workspaces/{workspace_id}/attachments/{}/download",
            uploaded.attachment_id
        ),
        None,
        None,
        Some(&cookie),
        &[("range", "bytes=0-1,5-9")],
    )
    .await;
    assert_eq!(status, StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(
        headers.get("content-range").unwrap().to_str().unwrap(),
        "bytes */10"
    );
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
    let row: (String, String) =
        sqlx::query_as("SELECT extract_status, extract_text FROM fvoci.attachments WHERE id = $1")
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
    let (app, cookie, _owner, workspace_id) = setup_session(&harness).await;
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

    let (status, _, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/auth/logout",
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/complete"),
        Some(json!({ "parts": [{ "partNumber": 1, "etag": etag }] })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
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

#[tokio::test]
async fn attachment_app_role_rls_two_tenant_isolation() {
    let harness = TestDb::bootstrap().await;
    let (_app, _cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let other_ws = Uuid::now_v7();
    sqlx::query("INSERT INTO fvoci.workspaces (id, slug, name) VALUES ($1, 'tenant-b', 'B')")
        .bind(other_ws)
        .execute(&admin)
        .await
        .unwrap();
    let foreign_doc = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO fvoci.documents (
            id, workspace_id, title, path, sort_key, number, status, schema_version,
            content_json, created_by
        ) VALUES (
            $1, $2, 'Secret', $3, 'V', 1, 'draft', 2, '{"type":"doc"}'::jsonb, $4
        )
        "#,
    )
    .bind(foreign_doc)
    .bind(other_ws)
    .bind(foreign_doc.simple().to_string())
    .bind(owner_id)
    .execute(&admin)
    .await
    .unwrap();
    let foreign_att = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO fvoci.attachments (
            id, workspace_id, document_id, uploader_id, status, name, reserved_size_bytes,
            size_bytes, storage_key, completed_at
        ) VALUES ($1, $2, $3, $4, 'stored', 'secret.bin', 5, 5, $5, now())
        "#,
    )
    .bind(foreign_att)
    .bind(other_ws)
    .bind(foreign_doc)
    .bind(owner_id)
    .bind(Uuid::now_v7().to_string())
    .execute(&admin)
    .await
    .unwrap();

    let app_pool = pool::connect_app(&harness.app_url).await.unwrap();
    let mut tx = app_pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
        .bind(workspace_id.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let hidden: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM fvoci.attachments WHERE id = $1")
        .bind(foreign_att)
        .fetch_optional(&mut *tx)
        .await
        .unwrap();
    assert!(hidden.is_none());
    let foreign_insert = sqlx::query(
        r#"
        INSERT INTO fvoci.attachments (
            id, workspace_id, document_id, uploader_id, status, name, reserved_size_bytes, storage_key
        ) VALUES ($1, $2, $3, $4, 'uploading', 'leak', 1, $5)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(other_ws)
    .bind(foreign_doc)
    .bind(owner_id)
    .bind(Uuid::now_v7().to_string())
    .execute(&mut *tx)
    .await;
    let err = foreign_insert.expect_err("foreign tenant insert must be denied");
    assert_eq!(
        err.as_database_error()
            .and_then(|e| e.code())
            .map(|c| c.to_string()),
        Some("42501".to_string())
    );
    tx.rollback().await.unwrap();
    app_pool.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn put_stream_revocation_blocks_part_publication() {
    let harness = TestDb::bootstrap().await;
    let storage_root = std::env::temp_dir().join(format!("fvoci-att-stage-{}", Uuid::now_v7()));
    let state = app_state_with_storage(&harness.app_url, storage_root).await;
    let app = app_router(state.clone());
    let (_, _, cookie_hdr) = json_request(
        app.clone(),
        "POST",
        "/api/v1/setup",
        Some(json!({
            "email": "stage@example.com",
            "password": "supersecret1",
            "givenName": "Stage",
            "workspaceSlug": "stage",
            "workspaceName": "Stage"
        })),
        None,
    )
    .await;
    let cookie = extract_session_cookie(cookie_hdr.as_ref().unwrap());
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let (owner_id, workspace_id): (Uuid, Uuid) = sqlx::query_as(
        "SELECT u.id, w.id FROM fvoci.users u JOIN fvoci.workspaces w ON w.slug = 'stage'",
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    admin.close().await;
    let document_id = create_document(&app, &cookie, workspace_id).await;
    let payload = b"stage-me";
    let (attachment_id, _part_url, _) = begin_upload(
        &app,
        &cookie,
        workspace_id,
        &document_id,
        "stage.bin",
        payload,
    )
    .await;
    let attachment_uuid = Uuid::parse_str(&attachment_id).unwrap();
    let session_id = session_id_for_cookie(&harness, &cookie).await;
    let pool = pool::connect_app(&harness.app_url).await.unwrap();
    let storage_key = authorize_upload_part(
        &pool,
        workspace_id,
        attachment_uuid,
        owner_id,
        session_id,
        1,
    )
    .await
    .unwrap()
    .unwrap()
    .0;
    let mut staged = state
        .storage
        .stage_part_stream(
            &storage_key,
            1,
            stream::iter(vec![Ok::<bytes::Bytes, std::io::Error>(
                bytes::Bytes::from_static(payload),
            )]),
            payload.len() as u64,
        )
        .await
        .unwrap();
    let (status, _, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/auth/logout",
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let denied = commit_upload_part(
        &pool,
        &state.storage,
        workspace_id,
        attachment_uuid,
        owner_id,
        session_id,
        1,
        &mut staged,
    )
    .await
    .unwrap()
    .unwrap_err();
    assert!(matches!(denied, AttachmentDbError::Forbidden));
    let parts = state.storage.list_parts(&storage_key).await.unwrap();
    assert!(parts.is_empty());
    pool.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn mid_assembly_revocation_denies_stored_publication() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _owner, workspace_id) = setup_session(&harness).await;
    let document_id = create_document(&app, &cookie, workspace_id).await;
    let payload = b"assembly";
    let (attachment_id, part_url, _) = begin_upload(
        &app,
        &cookie,
        workspace_id,
        &document_id,
        "asm.bin",
        payload,
    )
    .await;
    let (status, _, headers) = request(
        app.clone(),
        "PUT",
        &part_url,
        Some(payload.to_vec()),
        Some("application/octet-stream"),
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let etag = headers.get("etag").unwrap().to_str().unwrap();
    let attachment_uuid = Uuid::parse_str(&attachment_id).unwrap();
    let mut barrier = test_barrier::arm_pre_mark_stored(attachment_uuid);
    let complete = tokio::spawn({
        let app = app.clone();
        let cookie = cookie.clone();
        let attachment_id = attachment_id.clone();
        let etag = etag.to_string();
        async move {
            json_request(
                app,
                "POST",
                &format!("/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/complete"),
                Some(json!({ "parts": [{ "partNumber": 1, "etag": etag }] })),
                Some(&cookie),
            )
            .await
        }
    });
    tokio::time::timeout(Duration::from_secs(30), barrier.wait_entered())
        .await
        .expect("complete should reach pre-mark barrier")
        .expect("barrier entered");
    let (logout_status, _, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/auth/logout",
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(logout_status, StatusCode::NO_CONTENT);
    barrier.proceed();
    let (status, _, _) = complete.await.unwrap();
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "revoked session must be denied at final publication recheck"
    );
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let row: (String,) = sqlx::query_as("SELECT status FROM fvoci.attachments WHERE id = $1")
        .bind(attachment_uuid)
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_ne!(row.0, "stored");
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn attachment_event_and_audit_failures_roll_back_and_retry() {
    let harness = TestDb::bootstrap().await;
    let storage_root = std::env::temp_dir().join(format!("fvoci-att-rb-{}", Uuid::now_v7()));
    let state = app_state_with_storage(&harness.app_url, storage_root).await;
    let app = app_router(state.clone());
    let (_, _, cookie_hdr) = json_request(
        app.clone(),
        "POST",
        "/api/v1/setup",
        Some(json!({
            "email": "rollback@example.com",
            "password": "supersecret1",
            "givenName": "Rollback",
            "workspaceSlug": "rollback",
            "workspaceName": "Rollback"
        })),
        None,
    )
    .await;
    let cookie = extract_session_cookie(cookie_hdr.as_ref().unwrap());
    let admin_ws = PgPoolOptions::new()
        .max_connections(1)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let workspace_id: Uuid =
        sqlx::query_scalar("SELECT id FROM fvoci.workspaces WHERE slug = 'rollback'")
            .fetch_one(&admin_ws)
            .await
            .unwrap();
    admin_ws.close().await;
    let document_id = create_document(&app, &cookie, workspace_id).await;
    let payload = b"rollback";
    let (attachment_id, part_url, _) =
        begin_upload(&app, &cookie, workspace_id, &document_id, "rb.bin", payload).await;
    let (status, _, headers) = request(
        app.clone(),
        "PUT",
        &part_url,
        Some(payload.to_vec()),
        Some("application/octet-stream"),
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let etag = headers.get("etag").unwrap().to_str().unwrap();
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    install_insert_fail_trigger(&admin, "events", "test_att_event_fail").await;
    let (status, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/complete"),
        Some(json!({ "parts": [{ "partNumber": 1, "etag": etag }] })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    let row: (String,) = sqlx::query_as("SELECT status FROM fvoci.attachments WHERE id = $1")
        .bind(Uuid::parse_str(&attachment_id).unwrap())
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_ne!(row.0, "stored");
    let storage_key: (String,) =
        sqlx::query_as("SELECT storage_key FROM fvoci.attachments WHERE id = $1")
            .bind(Uuid::parse_str(&attachment_id).unwrap())
            .fetch_one(&admin)
            .await
            .unwrap();
    let parts = state.storage.list_parts(&storage_key.0).await.unwrap();
    assert_eq!(parts.len(), 1);
    sqlx::query("DROP TRIGGER fvoci_test_att_event_fail ON fvoci.events")
        .execute(&admin)
        .await
        .unwrap();

    let (status, body, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/complete"),
        Some(json!({ "parts": [{ "partNumber": 1, "etag": "not-the-etag" }] })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "wrong etag: {:?}", body);
    assert_eq!(body["code"], "submitted_parts_do_not_match_uploaded_parts");

    install_insert_fail_trigger(&admin, "audit_log", "test_att_audit_fail").await;
    let (status, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/complete"),
        Some(json!({ "parts": [{ "partNumber": 1, "etag": etag }] })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    let row: (String,) = sqlx::query_as("SELECT status FROM fvoci.attachments WHERE id = $1")
        .bind(Uuid::parse_str(&attachment_id).unwrap())
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_ne!(row.0, "stored");
    sqlx::query("DROP TRIGGER fvoci_test_att_audit_fail ON fvoci.audit_log")
        .execute(&admin)
        .await
        .unwrap();

    let (status, completed, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/complete"),
        Some(json!({ "parts": [{ "partNumber": 1, "etag": etag }] })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "retry complete: {:?}", completed);
    assert_eq!(completed["sizeBytes"], payload.len());
    assert_eq!(completed["scanStatus"], "skipped");
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn concurrent_duplicate_complete_is_idempotent() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _owner, workspace_id) = setup_session(&harness).await;
    let document_id = create_document(&app, &cookie, workspace_id).await;
    let payload = b"concurrent";
    let (attachment_id, part_url, _) = begin_upload(
        &app,
        &cookie,
        workspace_id,
        &document_id,
        "dup.bin",
        payload,
    )
    .await;
    let (status, _, headers) = request(
        app.clone(),
        "PUT",
        &part_url,
        Some(payload.to_vec()),
        Some("application/octet-stream"),
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let etag = headers.get("etag").unwrap().to_str().unwrap().to_string();
    let body = json!({ "parts": [{ "partNumber": 1, "etag": etag }] });
    let first = tokio::spawn({
        let app = app.clone();
        let cookie = cookie.clone();
        let attachment_id = attachment_id.clone();
        let body = body.clone();
        async move {
            json_request(
                app,
                "POST",
                &format!("/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/complete"),
                Some(body),
                Some(&cookie),
            )
            .await
        }
    });
    let second = tokio::spawn({
        let app = app.clone();
        let cookie = cookie.clone();
        let attachment_id = attachment_id.clone();
        let body = body.clone();
        async move {
            json_request(
                app,
                "POST",
                &format!("/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/complete"),
                Some(body),
                Some(&cookie),
            )
            .await
        }
    });
    let (s1, _, _) = first.await.unwrap();
    let (s2, _, _) = second.await.unwrap();
    assert_eq!(s1, StatusCode::OK);
    assert_eq!(s2, StatusCode::OK);
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let stored: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.attachments WHERE id = $1 AND status = 'stored'",
    )
    .bind(Uuid::parse_str(&attachment_id).unwrap())
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(stored.0, 1);
    let events: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.events WHERE verb = 'attachment.completed' AND target_id = $1",
    )
    .bind(Uuid::parse_str(&attachment_id).unwrap())
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(events.0, 1);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn aborted_complete_releases_lock_for_retry_on_same_pool() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _owner, workspace_id) = setup_session(&harness).await;
    let document_id = create_document(&app, &cookie, workspace_id).await;
    let payload = b"cancel-retry";
    let (attachment_id, part_url, _) = begin_upload(
        &app,
        &cookie,
        workspace_id,
        &document_id,
        "cancel.bin",
        payload,
    )
    .await;
    let (status, _, headers) = request(
        app.clone(),
        "PUT",
        &part_url,
        Some(payload.to_vec()),
        Some("application/octet-stream"),
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let etag = headers.get("etag").unwrap().to_str().unwrap().to_string();
    let attachment_uuid = Uuid::parse_str(&attachment_id).unwrap();
    let mut barrier = test_barrier::arm_pre_mark_stored(attachment_uuid);
    let aborted = tokio::spawn({
        let app = app.clone();
        let cookie = cookie.clone();
        let attachment_id = attachment_id.clone();
        let etag = etag.clone();
        async move {
            json_request(
                app,
                "POST",
                &format!("/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/complete"),
                Some(json!({ "parts": [{ "partNumber": 1, "etag": etag }] })),
                Some(&cookie),
            )
            .await
        }
    });
    tokio::time::timeout(Duration::from_secs(30), barrier.wait_entered())
        .await
        .expect("complete should reach pre-mark barrier")
        .expect("barrier entered");
    aborted.abort();
    let _ = aborted.await;
    test_barrier::disarm_pre_mark_stored(attachment_uuid);
    let (status, body, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/complete"),
        Some(json!({ "parts": [{ "partNumber": 1, "etag": "wrong-etag" }] })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "wrong etag: {:?}", body);
    assert_eq!(body["code"], "submitted_parts_do_not_match_uploaded_parts");
    let (status, completed, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/complete"),
        Some(json!({ "parts": [{ "partNumber": 1, "etag": etag }] })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "retry complete: {:?}", completed);
    assert_eq!(completed["sizeBytes"], payload.len());
    harness.cleanup().await;
}

#[tokio::test]
async fn fresh_migration_006_adds_attachments_table() {
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let versions: (i64,) = sqlx::query_as("SELECT count(*) FROM fvoci.schema_migrations")
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(
        versions.0,
        i64::from(fvoci_server::db::migrate::migration_count())
    );
    let has_attachments: (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM information_schema.tables WHERE table_schema = 'fvoci' AND table_name = 'attachments')",
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert!(has_attachments.0);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn migration_005_upgrades_to_006_attachments() {
    let admin_base = std::env::var("TEST_DATABASE_URL")
        .or_else(|_| std::env::var("FVOCI_TEST_DATABASE_URL"))
        .expect("TEST_DATABASE_URL missing");
    let db_name = format!("fvoci_att_upg_{}", Uuid::now_v7().simple());
    let server_url = server_db_url(&admin_base);
    let admin_pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&server_url)
        .await
        .unwrap();
    sqlx::query(&format!("CREATE DATABASE \"{db_name}\""))
        .execute(&admin_pool)
        .await
        .unwrap();
    admin_pool.close().await;

    let admin_url = join_db_url(&server_url, &db_name);
    let migration_pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&admin_url)
        .await
        .unwrap();
    for sql in [
        include_str!("../migrations/001_schema.sql"),
        include_str!("../migrations/002_functions.sql"),
        include_str!("../migrations/003_workspace.sql"),
        include_str!("../migrations/004_documents.sql"),
        include_str!("../migrations/005_collab_updates.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&migration_pool).await.unwrap();
    }
    sqlx::query("INSERT INTO fvoci.schema_migrations (version) VALUES (1), (2), (3), (4), (5)")
        .execute(&migration_pool)
        .await
        .unwrap();
    let has_attachments: (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM information_schema.tables WHERE table_schema = 'fvoci' AND table_name = 'attachments')",
    )
    .fetch_one(&migration_pool)
    .await
    .unwrap();
    assert!(!has_attachments.0);
    migration_pool.close().await;

    migrate::run_migrations(&admin_url).await.unwrap();
    let migration_pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&admin_url)
        .await
        .unwrap();
    let versions: (i64,) = sqlx::query_as("SELECT count(*) FROM fvoci.schema_migrations")
        .fetch_one(&migration_pool)
        .await
        .unwrap();
    assert_eq!(
        versions.0,
        i64::from(fvoci_server::db::migrate::migration_count())
    );
    let has_attachments: (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM information_schema.tables WHERE table_schema = 'fvoci' AND table_name = 'attachments')",
    )
    .fetch_one(&migration_pool)
    .await
    .unwrap();
    assert!(has_attachments.0);
    migration_pool.close().await;

    let server_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&server_url)
        .await
        .unwrap();
    let _ = sqlx::query(&format!(
        "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '{}'",
        db_name
    ))
    .execute(&server_pool)
    .await;
    let _ = sqlx::query(&format!("DROP DATABASE IF EXISTS \"{}\"", db_name))
        .execute(&server_pool)
        .await;
    server_pool.close().await;
}

#[tokio::test]
async fn stored_original_survives_service_recreation() {
    let harness = TestDb::bootstrap().await;
    let storage_root = std::env::temp_dir().join(format!("fvoci-att-persist-{}", Uuid::now_v7()));
    let state = app_state_with_storage(&harness.app_url, storage_root.clone()).await;
    let app = app_router(state);
    let (_, _, cookie_hdr) = json_request(
        app.clone(),
        "POST",
        "/api/v1/setup",
        Some(json!({
            "email": "persist@example.com",
            "password": "supersecret1",
            "givenName": "Persist",
            "workspaceSlug": "persist",
            "workspaceName": "Persist"
        })),
        None,
    )
    .await;
    let cookie = extract_session_cookie(cookie_hdr.as_ref().unwrap());
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let workspace_id: Uuid =
        sqlx::query_scalar("SELECT id FROM fvoci.workspaces WHERE slug = 'persist'")
            .fetch_one(&admin)
            .await
            .unwrap();
    admin.close().await;
    let document_id = create_document(&app, &cookie, workspace_id).await;
    let payload = b"persist-bytes";
    let uploaded = upload_bytes(
        &app,
        &cookie,
        workspace_id,
        &document_id,
        "persist.bin",
        payload,
        None,
    )
    .await;

    let app2 = app_router(app_state_with_storage(&harness.app_url, storage_root).await);
    let (status, body, _) = request(
        app2,
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
    assert_eq!(body, payload);
    harness.cleanup().await;
}

#[tokio::test]
async fn strict_dto_unknown_null_filename_and_preview_query() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _owner, workspace_id) = setup_session(&harness).await;
    let document_id = create_document(&app, &cookie, workspace_id).await;
    let create_path = format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/uploads");

    let (status, body, _) = json_request(
        app.clone(),
        "POST",
        &create_path,
        Some(json!({ "name": "a.bin", "sizeBytes": 1, "extra": true })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "invalid_input");

    let (status, body, _) = json_request(
        app.clone(),
        "POST",
        &create_path,
        Some(json!({ "name": "a.bin", "sizeBytes": 1, "declaredMime": null })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "invalid_input");

    let too_long = "한".repeat(256);
    let (status, body, _) = json_request(
        app.clone(),
        "POST",
        &create_path,
        Some(json!({ "name": too_long, "sizeBytes": 1 })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "invalid_input");

    let name = "첨부😀.png";
    let uploaded = upload_bytes(
        &app,
        &cookie,
        workspace_id,
        &document_id,
        name,
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
    assert_eq!(meta["name"], name);
    assert_eq!(meta["preview"], Value::Null);
    assert!(meta["createdAt"].is_string());

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
    let disposition = headers
        .get("content-disposition")
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(
        disposition,
        fvoci_server::attachments::content_disposition_attachment(name)
    );

    let (status, body, _) = json_request(
        app.clone(),
        "POST",
        &format!(
            "/api/v1/workspaces/{workspace_id}/attachments/{}/complete",
            uploaded.attachment_id
        ),
        Some(json!({ "parts": [{ "partNumber": 1, "etag": uploaded.etag }], "extra": true })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "invalid_input");

    let (status, body, _) = json_request(
        app.clone(),
        "POST",
        &format!(
            "/api/v1/workspaces/{workspace_id}/attachments/{}/complete",
            uploaded.attachment_id
        ),
        Some(json!({ "parts": [{ "partNumber": 0, "etag": uploaded.etag }] })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "invalid_input");

    let download = format!(
        "/api/v1/workspaces/{workspace_id}/attachments/{}/download",
        uploaded.attachment_id
    );
    let (status, _, _) = request(
        app.clone(),
        "GET",
        &format!("{download}?variant=preview"),
        None,
        None,
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, body, _) = request(
        app.clone(),
        "GET",
        &format!("{download}?variant=thumb"),
        None,
        None,
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let json: Value = serde_json::from_slice(&body).unwrap_or(json!({}));
    assert_eq!(json["code"], "invalid_input");

    let (status, body, _) = request(
        app.clone(),
        "GET",
        &format!("{download}?foo=1"),
        None,
        None,
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let json: Value = serde_json::from_slice(&body).unwrap_or(json!({}));
    assert_eq!(json["code"], "invalid_input");
    harness.cleanup().await;
}

#[tokio::test]
async fn product_role_and_member_revoke_deny_upload() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, _owner, workspace_id) = setup_session(&harness).await;
    let document_id = create_document(&app, &owner_cookie, workspace_id).await;
    let (member_id, member_cookie) =
        create_user_with_role(&harness, "writer@example.com", workspace_id, "member").await;
    let payload = b"member-bytes";
    let (attachment_id, part_url, _) = begin_upload(
        &app,
        &member_cookie,
        workspace_id,
        &document_id,
        "member.bin",
        payload,
    )
    .await;
    let (status, _, headers) = request(
        app.clone(),
        "PUT",
        &part_url,
        Some(payload.to_vec()),
        Some("application/octet-stream"),
        Some(&member_cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let etag = headers.get("etag").unwrap().to_str().unwrap().to_string();

    let (status, body, _) = json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{workspace_id}/members/{member_id}"),
        Some(json!({ "role": "guest" })),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "demote: {:?}", body);
    assert_eq!(body["role"], "guest");

    let (status, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/complete"),
        Some(json!({ "parts": [{ "partNumber": 1, "etag": etag }] })),
        Some(&member_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (removed_id, removed_cookie) =
        create_user_with_role(&harness, "removed@example.com", workspace_id, "member").await;
    let (attachment_id, part_url, _) = begin_upload(
        &app,
        &removed_cookie,
        workspace_id,
        &document_id,
        "removed.bin",
        payload,
    )
    .await;
    let (status, _, headers) = request(
        app.clone(),
        "PUT",
        &part_url,
        Some(payload.to_vec()),
        Some("application/octet-stream"),
        Some(&removed_cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let etag = headers.get("etag").unwrap().to_str().unwrap().to_string();
    let (status, _, _) = request(
        app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{workspace_id}/members/{removed_id}"),
        None,
        Some("application/json"),
        Some(&owner_cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/complete"),
        Some(json!({ "parts": [{ "partNumber": 1, "etag": etag }] })),
        Some(&removed_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let missing_doc = Uuid::now_v7();
    let (status, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{missing_doc}/uploads"),
        Some(json!({ "name": "gone.bin", "sizeBytes": 4 })),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    harness.cleanup().await;
}

#[tokio::test]
async fn aborted_put_removes_writing_and_keeps_valid_part() {
    let harness = TestDb::bootstrap().await;
    let storage_root = std::env::temp_dir().join(format!("fvoci-att-abort-{}", Uuid::now_v7()));
    let state = app_state_with_storage(&harness.app_url, storage_root.clone()).await;
    let app = app_router(state);
    let (_, _, cookie_hdr) = json_request(
        app.clone(),
        "POST",
        "/api/v1/setup",
        Some(json!({
            "email": "abort@example.com",
            "password": "supersecret1",
            "givenName": "Abort",
            "workspaceSlug": "abort",
            "workspaceName": "Abort"
        })),
        None,
    )
    .await;
    let cookie = extract_session_cookie(cookie_hdr.as_ref().unwrap());
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let workspace_id: Uuid =
        sqlx::query_scalar("SELECT id FROM fvoci.workspaces WHERE slug = 'abort'")
            .fetch_one(&admin)
            .await
            .unwrap();
    admin.close().await;
    let document_id = create_document(&app, &cookie, workspace_id).await;
    let keep = b"keep-part";
    let (attachment_id, part_url, _) =
        begin_upload(&app, &cookie, workspace_id, &document_id, "abort.bin", keep).await;
    let (status, _, _) = request(
        app.clone(),
        "PUT",
        &part_url,
        Some(keep.to_vec()),
        Some("application/octet-stream"),
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let hanging = stream::iter(vec![Ok::<bytes::Bytes, std::io::Error>(
        bytes::Bytes::from_static(b"xx"),
    )])
    .chain(futures_util::stream::pending());
    let builder = Request::builder()
        .method("PUT")
        .uri(part_url.clone())
        .header("cookie", format!("fvoci_session={cookie}"))
        .header("content-type", "application/octet-stream");
    let mut request = builder.body(Body::from_stream(hanging)).unwrap();
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(test_peer()));
    let aborted = tokio::spawn(app.clone().oneshot(request));
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if writing_temps(&storage_root).await > 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("in-flight PUT should create a .writing file");
    aborted.abort();
    let _ = aborted.await;
    assert_eq!(
        writing_temps(&storage_root).await,
        0,
        "aborted PUT must remove staged .writing"
    );

    let (status, resume, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/upload"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(resume["uploadedParts"].as_array().unwrap().len(), 1);
    assert_eq!(resume["attachmentId"], attachment_id);
    harness.cleanup().await;
}

#[tokio::test]
async fn aborted_put_before_publish_removes_staged_and_keeps_part() {
    let harness = TestDb::bootstrap().await;
    let storage_root = std::env::temp_dir().join(format!("fvoci-att-prepub-{}", Uuid::now_v7()));
    let state = app_state_with_storage(&harness.app_url, storage_root.clone()).await;
    let app = app_router(state);
    let (_, _, cookie_hdr) = json_request(
        app.clone(),
        "POST",
        "/api/v1/setup",
        Some(json!({
            "email": "prepub@example.com",
            "password": "supersecret1",
            "givenName": "Prepub",
            "workspaceSlug": "prepub",
            "workspaceName": "Prepub"
        })),
        None,
    )
    .await;
    let cookie = extract_session_cookie(cookie_hdr.as_ref().unwrap());
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let workspace_id: Uuid =
        sqlx::query_scalar("SELECT id FROM fvoci.workspaces WHERE slug = 'prepub'")
            .fetch_one(&admin)
            .await
            .unwrap();
    admin.close().await;
    let document_id = create_document(&app, &cookie, workspace_id).await;
    let keep = b"keep-part";
    let (attachment_id, part_url, _) = begin_upload(
        &app,
        &cookie,
        workspace_id,
        &document_id,
        "prepub.bin",
        keep,
    )
    .await;
    let (status, _, headers) = request(
        app.clone(),
        "PUT",
        &part_url,
        Some(keep.to_vec()),
        Some("application/octet-stream"),
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let original_etag = headers.get("etag").unwrap().to_str().unwrap().to_string();
    let attachment_uuid = Uuid::parse_str(&attachment_id).unwrap();
    let mut barrier = test_barrier::arm_pre_publish(attachment_uuid);
    let aborted = tokio::spawn({
        let app = app.clone();
        let cookie = cookie.clone();
        let part_url = part_url.clone();
        async move {
            request(
                app,
                "PUT",
                &part_url,
                Some(b"overwrite".to_vec()),
                Some("application/octet-stream"),
                Some(&cookie),
                &[],
            )
            .await
        }
    });
    tokio::time::timeout(Duration::from_secs(30), barrier.wait_entered())
        .await
        .expect("PUT should reach pre-publish barrier")
        .expect("barrier entered");
    aborted.abort();
    let _ = aborted.await;
    test_barrier::disarm_pre_publish(attachment_uuid);
    assert_eq!(
        writing_temps(&storage_root).await,
        0,
        "cancelled staged part must not leave .writing"
    );
    let (status, resume, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/upload"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(resume["uploadedParts"][0]["etag"], original_etag);
    harness.cleanup().await;
}

async fn writing_temps(root: &Path) -> usize {
    let mut count = 0usize;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(mut entries) = tokio::fs::read_dir(&dir).await else {
            continue;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(".writing"))
            {
                count += 1;
            }
        }
    }
    count
}
