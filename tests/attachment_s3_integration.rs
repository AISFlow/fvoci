#![cfg(feature = "db-tests")]

#[path = "support/office_fixtures.rs"]
mod office_fixtures;

use std::sync::Arc;
use std::time::Duration;

use axum::http::{HeaderMap, Request, StatusCode};
use bytes::Bytes;
use chrono::Utc;
use futures_util::stream;
use fvoci_server::attachments::{ObjectStorage, S3Storage, StorageError};
use fvoci_server::auth::password::Keyring;
use fvoci_server::auth::AuthService;
use fvoci_server::config::S3Settings;
use fvoci_server::db::{migrate, pool, Db};
use fvoci_server::http::rate_limit::RateLimiter;
use fvoci_server::http::state::AppState;
use rand::RngCore;
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;
use uuid::Uuid;

const PEPPER: &str =
    r#"{"test":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#;

const PNG_BYTES: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
    0x42, 0x60, 0x82,
];

fn test_peer() -> std::net::SocketAddr {
    std::net::SocketAddr::from(([203, 0, 113, 11], 42426))
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

        let db_name = format!("fvoci_s3_{}", Uuid::now_v7().simple());
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
        fvoci_server::db::migrate::apply_app_role_grants(&migration_pool, &role_name)
            .await
            .expect("grant");
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
    parsed.set_path(&format!("/{db_name}"));
    parsed.to_string()
}

fn nonempty_env(name: &str) -> String {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| panic!("{name} missing — run via scripts/start-test-minio.sh"))
}

fn s3_settings() -> S3Settings {
    S3Settings {
        endpoint: nonempty_env("S3_ENDPOINT"),
        public_endpoint: None,
        region: nonempty_env("S3_REGION"),
        bucket: nonempty_env("S3_BUCKET"),
        access_key_id: nonempty_env("S3_ACCESS_KEY_ID"),
        secret_access_key: nonempty_env("S3_SECRET_ACCESS_KEY"),
        force_path_style: std::env::var("S3_FORCE_PATH_STYLE")
            .map(|v| v.trim() != "0")
            .unwrap_or(true),
    }
}

async fn s3_backend() -> ObjectStorage {
    let s3 = S3Storage::new(s3_settings()).expect("s3 settings");
    s3.ensure_bucket().await.expect("ensure bucket");
    ObjectStorage::from(s3)
}

async fn app_state_with_part_size(
    app_url: &str,
    storage: ObjectStorage,
    part_size_bytes: i64,
) -> AppState {
    let pool = pool::connect_app(app_url).await.expect("app pool");
    AppState {
        auth: Arc::new(AuthService {
            db: Db::new(pool),
            password_keys: Keyring::parse(PEPPER, "test").expect("pepper"),
        }),
        branding_name: "FVOCI".to_string(),
        public_origin: "http://localhost".to_string(),
        cookie_secure: false,
        rate_limiter: RateLimiter::new(),
        storage,
        upload: fvoci_server::attachments::UploadLimits {
            part_size_bytes,
            max_file_size_bytes: fvoci_server::config::DEFAULT_UPLOAD_MAX_FILE_SIZE_BYTES,
            create_rate_per_5min: fvoci_server::config::DEFAULT_UPLOAD_CREATE_RATE_PER_5MIN,
            part_put_slots: fvoci_server::attachments::PartPutSlots::new(
                fvoci_server::config::DEFAULT_UPLOAD_MAX_CONCURRENT_PARTS,
            ),
        },
        collab: None,
        meili: None,
        search_embedder: None,
        streams: fvoci_server::http::state::AppState::fresh_streams(),
        mailer: Arc::new(fvoci_server::mail::Mailer::disabled()),
        markdown: Some(
            fvoci_server::documents::markdown_helper::MarkdownHelper::new(env!(
                "CARGO_BIN_EXE_fvoci-server"
            )),
        ),
        import_wake: None,
        import_extractor_available: false,
        preview_extract: None,
        quota: Default::default(),
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
) -> (StatusCode, Vec<u8>, HeaderMap) {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", format!("fvoci_session={cookie}"));
    }
    if let Some(ct) = content_type {
        builder = builder.header("content-type", ct);
    }
    let mut request = builder
        .body(axum::body::Body::from(body.unwrap_or_default()))
        .unwrap();
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(test_peer()));
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, bytes.to_vec(), headers)
}

async fn json_request(
    app: axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    cookie: Option<&str>,
) -> (StatusCode, Value, Option<HeaderMap>) {
    let encoded = body.map(|v| serde_json::to_vec(&v).unwrap());
    let (status, bytes, headers) =
        request(app, method, path, encoded, Some("application/json"), cookie).await;
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, value, Some(headers))
}

fn extract_session_cookie(headers: &HeaderMap) -> String {
    headers
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with("fvoci_session="))
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .split('=')
        .nth(1)
        .unwrap_or("")
        .to_string()
}

async fn setup_session(harness: &TestDb, storage: ObjectStorage) -> (axum::Router, String, Uuid) {
    setup_session_with_part_size(
        harness,
        storage,
        fvoci_server::config::DEFAULT_UPLOAD_PART_SIZE_BYTES,
    )
    .await
}

async fn setup_session_with_part_size(
    harness: &TestDb,
    storage: ObjectStorage,
    part_size_bytes: i64,
) -> (axum::Router, String, Uuid) {
    let state = app_state_with_part_size(&harness.app_url, storage, part_size_bytes).await;
    setup_session_with_state(harness, state).await
}

async fn setup_session_with_state(
    harness: &TestDb,
    state: AppState,
) -> (axum::Router, String, Uuid) {
    let app = app_router(state);
    let (_, _, cookie_hdr) = json_request(
        app.clone(),
        "POST",
        "/api/v1/setup",
        Some(json!({
            "email": "s3owner@example.com",
            "password": "supersecret1",
            "givenName": "Owner",
            "workspaceSlug": "s3ws",
            "workspaceName": "S3"
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
    let workspace_id: Uuid =
        sqlx::query_scalar("SELECT id FROM fvoci.workspaces WHERE slug = 's3ws'")
            .fetch_one(&admin)
            .await
            .unwrap();
    admin.close().await;
    (app, cookie, workspace_id)
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

#[tokio::test]
async fn s3_multipart_complete_download_delete_and_abort() {
    let storage = s3_backend().await;
    let key = Uuid::now_v7().to_string();
    let upload_id = storage
        .create_multipart(&key)
        .await
        .expect("create multipart")
        .expect("s3 upload id");
    let body = b"hello-s3-part";
    let mut staged = storage
        .stage_part_stream(
            &key,
            Some(&upload_id),
            1,
            stream::iter(vec![Ok::<Bytes, std::io::Error>(Bytes::from_static(body))]),
            Some(body.len() as u64),
            body.len() as u64,
        )
        .await
        .expect("stage");
    let part = storage
        .publish_staged_part(&key, 1, &mut staged)
        .await
        .expect("publish");
    let size = storage
        .assemble_multipart(
            &key,
            Some(&upload_id),
            &[(part.part_number, part.etag.clone())],
        )
        .await
        .expect("complete");
    assert_eq!(size, body.len() as u64);
    assert_eq!(storage.head(&key).await.unwrap(), Some(body.len() as u64));
    let got = storage
        .read_range(&key, 0, (body.len() as u64) - 1)
        .await
        .unwrap();
    assert_eq!(got, body);
    storage.delete_object(&key).await.unwrap();
    assert_eq!(storage.head(&key).await.unwrap(), None);

    let gone_key = Uuid::now_v7().to_string();
    let upload_id = storage
        .create_multipart(&gone_key)
        .await
        .unwrap()
        .expect("upload id");
    storage
        .abort_multipart(&gone_key, Some(&upload_id))
        .await
        .unwrap();
    let err = storage
        .list_parts(&gone_key, Some(&upload_id))
        .await
        .unwrap_err();
    assert!(matches!(err, StorageError::UploadGone));
}

#[tokio::test]
async fn s3_complete_is_idempotent_when_object_already_published() {
    let storage = s3_backend().await;
    let key = Uuid::now_v7().to_string();
    let upload_id = storage
        .create_multipart(&key)
        .await
        .unwrap()
        .expect("upload id");
    let body = b"idempotent-complete";
    let mut staged = storage
        .stage_part_stream(
            &key,
            Some(&upload_id),
            1,
            stream::iter(vec![Ok::<Bytes, std::io::Error>(Bytes::from_static(body))]),
            Some(body.len() as u64),
            body.len() as u64,
        )
        .await
        .unwrap();
    let part = storage
        .publish_staged_part(&key, 1, &mut staged)
        .await
        .unwrap();
    let first = storage
        .assemble_multipart(&key, Some(&upload_id), &[(1, part.etag.clone())])
        .await
        .unwrap();
    let second = storage
        .assemble_multipart(&key, Some(&upload_id), &[(1, part.etag)])
        .await
        .unwrap();
    assert_eq!(first, second);
    assert_eq!(first, body.len() as u64);
    storage.delete_object(&key).await.unwrap();
}

#[tokio::test]
async fn s3_http_upload_download_and_stale_gc() {
    let harness = TestDb::bootstrap().await;
    let storage = s3_backend().await;
    let gc_storage = storage.clone();
    let (app, cookie, workspace_id) = setup_session(&harness, storage).await;
    let document_id = create_document(&app, &cookie, workspace_id).await;

    let (status, created, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/uploads"),
        Some(json!({ "name": "pixel.png", "sizeBytes": PNG_BYTES.len(), "declaredMime": "image/png" })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create: {created:?}");
    let attachment_id = created["attachmentId"].as_str().unwrap().to_string();
    let part_url = created["parts"][0]["url"].as_str().unwrap();
    let (status, _, headers) = request(
        app.clone(),
        "PUT",
        part_url,
        Some(PNG_BYTES.to_vec()),
        Some("application/octet-stream"),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let etag = headers
        .get("etag")
        .or_else(|| headers.get("ETag"))
        .and_then(|v| v.to_str().ok())
        .expect("etag")
        .to_string();
    let (status, completed, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/complete"),
        Some(json!({ "parts": [{ "partNumber": 1, "etag": etag }] })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "complete: {completed:?}");

    let (status, bytes, _) = request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/download"),
        None,
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, PNG_BYTES);

    let stale_payload = b"s3-stale";
    let (status, created, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/uploads"),
        Some(json!({ "name": "stale.bin", "sizeBytes": stale_payload.len() })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let stale_id = created["attachmentId"].as_str().unwrap().to_string();
    let stale_url = created["parts"][0]["url"].as_str().unwrap();
    let (status, _, _) = request(
        app.clone(),
        "PUT",
        stale_url,
        Some(stale_payload.to_vec()),
        Some("application/octet-stream"),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let stale_uuid = Uuid::parse_str(&stale_id).unwrap();
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE fvoci.attachments SET created_at = NOW() - INTERVAL '25 hours' WHERE id = $1",
    )
    .bind(stale_uuid)
    .execute(&admin)
    .await
    .unwrap();
    let storage_key: String =
        sqlx::query_scalar("SELECT storage_key FROM fvoci.attachments WHERE id = $1")
            .bind(stale_uuid)
            .fetch_one(&admin)
            .await
            .unwrap();
    // A second handle on the same key (e.g. a create whose upload id never
    // reached the row) must be aborted too.
    let orphan = gc_storage
        .create_multipart(&storage_key)
        .await
        .unwrap()
        .expect("orphan upload id");
    assert_eq!(
        gc_storage
            .list_multipart_uploads(&storage_key)
            .await
            .unwrap()
            .len(),
        2
    );

    // Stale rows in another workspace are swept too (RLS is per tenant), and a
    // fresh upload is left alone.
    let (status, body, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/workspaces",
        Some(json!({ "name": "Other", "slug": "s3other" })),
        Some(&cookie),
    )
    .await;
    assert!(status.is_success(), "create workspace: {status} {body:?}");
    let other_ws: Uuid =
        sqlx::query_scalar("SELECT id FROM fvoci.workspaces WHERE slug = 's3other'")
            .fetch_one(&admin)
            .await
            .unwrap();
    let other_doc = create_document(&app, &cookie, other_ws).await;
    let mut other_ids = Vec::new();
    for name in ["other-stale.bin", "fresh.bin"] {
        let (status, created, _) = json_request(
            app.clone(),
            "POST",
            &format!("/api/v1/workspaces/{other_ws}/documents/{other_doc}/uploads"),
            Some(json!({ "name": name, "sizeBytes": 3 })),
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{created:?}");
        other_ids.push(Uuid::parse_str(created["attachmentId"].as_str().unwrap()).unwrap());
    }
    sqlx::query(
        "UPDATE fvoci.attachments SET created_at = NOW() - INTERVAL '25 hours' WHERE id = $1",
    )
    .bind(other_ids[0])
    .execute(&admin)
    .await
    .unwrap();
    let cutoff = Utc::now() - chrono::Duration::hours(24);

    let pool = pool::connect_app(&harness.app_url).await.unwrap();
    // Each run is bounded: the listing stops at the batch limit across
    // workspaces instead of loading every stale row.
    let one = fvoci_server::db::attachments::list_stale_uploading(&pool, cutoff, None, 1)
        .await
        .unwrap();
    assert_eq!(one.len(), 1);
    let all = fvoci_server::db::attachments::list_stale_uploading(&pool, cutoff, None, 10)
        .await
        .unwrap();
    let mut seen: Vec<Uuid> = all.iter().map(|row| row.workspace_id).collect();
    seen.dedup();
    assert_eq!(all.len(), 2);
    assert_eq!(seen.len(), 2, "stale rows from both workspaces: {all:?}");
    let purged = gc_stale_uploads(&pool, &gc_storage, cutoff)
        .await
        .expect("gc");
    assert_eq!(purged, 2);
    let remaining: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM fvoci.attachments WHERE id = ANY($1) ORDER BY id")
            .bind(vec![stale_uuid, other_ids[0], other_ids[1]])
            .fetch_all(&admin)
            .await
            .unwrap();
    assert_eq!(remaining, vec![other_ids[1]]);
    assert_eq!(gc_storage.head(&storage_key).await.unwrap(), None);
    let leftover = gc_storage
        .list_multipart_uploads(&storage_key)
        .await
        .unwrap();
    assert!(leftover.is_empty(), "orphan {orphan} left: {leftover:?}");
    let stored: i64 =
        sqlx::query_scalar("SELECT count(*) FROM fvoci.attachments WHERE status = 'stored'")
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(stored, 1, "stored attachment must survive the sweep");
    // Idempotent: nothing left to purge.
    let again = gc_stale_uploads(&pool, &gc_storage, cutoff)
        .await
        .expect("gc again");
    assert_eq!(again, 0);
    pool.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn s3_gc_does_not_delete_object_while_complete_holds_lock() {
    let harness = TestDb::bootstrap().await;
    let storage = s3_backend().await;
    let gc_storage = storage.clone();
    let (app, cookie, workspace_id) = setup_session(&harness, storage).await;
    let document_id = create_document(&app, &cookie, workspace_id).await;
    let payload = b"lock-race";
    let (status, created, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/uploads"),
        Some(json!({ "name": "lock.bin", "sizeBytes": payload.len() })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let attachment_id = created["attachmentId"].as_str().unwrap().to_string();
    let part_url = created["parts"][0]["url"].as_str().unwrap();
    let (status, _, headers) = request(
        app.clone(),
        "PUT",
        part_url,
        Some(payload.to_vec()),
        Some("application/octet-stream"),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let etag = headers.get("etag").unwrap().to_str().unwrap().to_string();
    let attachment_uuid = Uuid::parse_str(&attachment_id).unwrap();
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE fvoci.attachments SET created_at = NOW() - INTERVAL '25 hours' WHERE id = $1",
    )
    .bind(attachment_uuid)
    .execute(&admin)
    .await
    .unwrap();
    let mut barrier =
        fvoci_server::db::attachments::test_barrier::arm_pre_mark_stored(attachment_uuid);
    let complete = tokio::spawn({
        let app = app.clone();
        let cookie = cookie.clone();
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
    let pool = pool::connect_app(&harness.app_url).await.unwrap();
    let during = gc_stale_uploads(&pool, &gc_storage, Utc::now())
        .await
        .expect("gc during lock");
    assert_eq!(during, 0);
    barrier.proceed();
    let (status, _, _) = complete.await.unwrap();
    assert_eq!(status, StatusCode::OK);
    let stored: String = sqlx::query_scalar("SELECT status FROM fvoci.attachments WHERE id = $1")
        .bind(attachment_uuid)
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(stored, "stored");
    // Once complete released the lock the row is stored: a later sweep skips
    // it and the published object survives.
    let after = gc_stale_uploads(&pool, &gc_storage, Utc::now())
        .await
        .expect("gc after complete");
    assert_eq!(after, 0);
    let storage_key: String =
        sqlx::query_scalar("SELECT storage_key FROM fvoci.attachments WHERE id = $1")
            .bind(attachment_uuid)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(
        gc_storage.head(&storage_key).await.unwrap(),
        Some(payload.len() as u64)
    );
    pool.close().await;
    admin.close().await;
    harness.cleanup().await;
}

const MIB: usize = 1024 * 1024;

fn patterned(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
        .collect()
}

async fn stage_and_publish(
    storage: &ObjectStorage,
    key: &str,
    upload_id: &str,
    part_number: i32,
    body: &[u8],
) -> fvoci_server::attachments::PartInfo {
    let mut staged = storage
        .stage_part_stream(
            key,
            Some(upload_id),
            part_number,
            stream::iter(vec![Ok::<Bytes, std::io::Error>(Bytes::copy_from_slice(
                body,
            ))]),
            Some(body.len() as u64),
            body.len() as u64,
        )
        .await
        .expect("stage part");
    storage
        .publish_staged_part(key, part_number, &mut staged)
        .await
        .expect("publish part")
}

#[tokio::test]
async fn s3_two_part_upload_out_of_order_complete_and_ranged_read() {
    let storage = s3_backend().await;
    let key = Uuid::now_v7().to_string();
    let upload_id = storage.create_multipart(&key).await.unwrap().unwrap();
    let first = patterned(5 * MIB, 1);
    let second = patterned(4096 + 7, 2);
    // Upload the final part first: order must not matter.
    let p2 = stage_and_publish(&storage, &key, &upload_id, 2, &second).await;
    let p1 = stage_and_publish(&storage, &key, &upload_id, 1, &first).await;
    let listed = storage.list_parts(&key, Some(&upload_id)).await.unwrap();
    assert_eq!(
        listed
            .iter()
            .map(|p| (p.part_number, p.size_bytes))
            .collect::<Vec<_>>(),
        vec![(1, first.len() as u64), (2, second.len() as u64)]
    );
    assert_eq!(listed[0].etag, p1.etag);
    assert_eq!(
        storage.list_multipart_uploads(&key).await.unwrap(),
        vec![Some(upload_id.clone())]
    );
    // Not visible before complete: publish is atomic.
    assert_eq!(storage.head(&key).await.unwrap(), None);
    let size = storage
        .assemble_multipart(
            &key,
            Some(&upload_id),
            &[(2, p2.etag.clone()), (1, format!("\"{}\"", p1.etag))],
        )
        .await
        .unwrap();
    let total = (first.len() + second.len()) as u64;
    assert_eq!(size, total);
    assert!(storage
        .list_multipart_uploads(&key)
        .await
        .unwrap()
        .is_empty());
    let boundary = first.len() as u64;
    let across = storage
        .read_range(&key, boundary - 3, boundary + 3)
        .await
        .unwrap();
    assert_eq!(&across[..3], &first[first.len() - 3..]);
    assert_eq!(&across[3..], &second[..4]);
    let whole = storage.read_range(&key, 0, total - 1).await.unwrap();
    assert_eq!(whole.len() as u64, total);
    assert_eq!(&whole[..first.len()], &first[..]);
    assert_eq!(
        storage.sniff_mime(&key).await.unwrap(),
        "application/octet-stream"
    );
    storage.delete_object(&key).await.unwrap();
    assert_eq!(storage.head(&key).await.unwrap(), None);
    // Deleting a missing object is idempotent.
    storage.delete_object(&key).await.unwrap();
}

#[tokio::test]
async fn s3_error_paths_map_to_storage_errors() {
    let storage = s3_backend().await;

    // Wrong ETag: rejected without publishing; the upload stays usable.
    let key = Uuid::now_v7().to_string();
    let upload_id = storage.create_multipart(&key).await.unwrap().unwrap();
    let part = stage_and_publish(&storage, &key, &upload_id, 1, b"etag-check").await;
    let err = storage
        .assemble_multipart(
            &key,
            Some(&upload_id),
            &[(1, "0123456789abcdef0123456789abcdef".into())],
        )
        .await
        .unwrap_err();
    assert!(matches!(err, StorageError::EtagMismatch), "{err:?}");
    assert_eq!(storage.head(&key).await.unwrap(), None);
    // A part number that was never uploaded is also a mismatch.
    let err = storage
        .assemble_multipart(&key, Some(&upload_id), &[(2, part.etag.clone())])
        .await
        .unwrap_err();
    assert!(matches!(err, StorageError::EtagMismatch), "{err:?}");
    storage
        .assemble_multipart(&key, Some(&upload_id), &[(1, part.etag)])
        .await
        .unwrap();
    storage.delete_object(&key).await.unwrap();

    // Non-final parts under the S3 minimum are refused by the server; the
    // error carries the S3 code, not a success.
    let key = Uuid::now_v7().to_string();
    let upload_id = storage.create_multipart(&key).await.unwrap().unwrap();
    let a = stage_and_publish(&storage, &key, &upload_id, 1, b"small-1").await;
    let b = stage_and_publish(&storage, &key, &upload_id, 2, b"small-2").await;
    let err = storage
        .assemble_multipart(&key, Some(&upload_id), &[(1, a.etag), (2, b.etag)])
        .await
        .unwrap_err();
    // EntityTooSmall is a client part-list problem (4xx), not an I/O error.
    assert!(matches!(err, StorageError::PartTooSmall), "{err:?}");
    assert_eq!(storage.head(&key).await.unwrap(), None);
    storage
        .abort_multipart(&key, Some(&upload_id))
        .await
        .unwrap();

    // Abort: later part uploads, listing and completion all see a gone upload,
    // and a second abort is a no-op.
    let key = Uuid::now_v7().to_string();
    let upload_id = storage.create_multipart(&key).await.unwrap().unwrap();
    let part = stage_and_publish(&storage, &key, &upload_id, 1, b"aborted").await;
    storage
        .abort_multipart(&key, Some(&upload_id))
        .await
        .unwrap();
    storage
        .abort_multipart(&key, Some(&upload_id))
        .await
        .unwrap();
    let err = storage
        .stage_part_stream(
            &key,
            Some(&upload_id),
            2,
            stream::iter(vec![Ok::<Bytes, std::io::Error>(Bytes::from_static(
                b"late",
            ))]),
            Some(4),
            4,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, StorageError::UploadGone), "{err:?}");
    assert!(matches!(
        storage
            .list_parts(&key, Some(&upload_id))
            .await
            .unwrap_err(),
        StorageError::UploadGone
    ));
    assert!(matches!(
        storage
            .assemble_multipart(&key, Some(&upload_id), &[(1, part.etag)])
            .await
            .unwrap_err(),
        StorageError::UploadGone
    ));
    assert!(storage
        .list_multipart_uploads(&key)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(storage.head(&key).await.unwrap(), None);

    // Oversized part: refused before anything reaches S3.
    let key = Uuid::now_v7().to_string();
    let upload_id = storage.create_multipart(&key).await.unwrap().unwrap();
    let err = storage
        .stage_part_stream(
            &key,
            Some(&upload_id),
            1,
            stream::iter(vec![
                Ok::<Bytes, std::io::Error>(Bytes::from_static(b"12345")),
                Ok(Bytes::from_static(b"6789")),
            ]),
            Some(9),
            8,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, StorageError::PartTooLarge), "{err:?}");
    assert!(storage
        .list_parts(&key, Some(&upload_id))
        .await
        .unwrap()
        .is_empty());
    // Missing upload reference is a gone upload, not a request.
    assert!(matches!(
        storage.list_parts(&key, None).await.unwrap_err(),
        StorageError::UploadGone
    ));
    storage
        .abort_multipart(&key, Some(&upload_id))
        .await
        .unwrap();

    // Reads of a missing object.
    let missing = Uuid::now_v7().to_string();
    assert_eq!(storage.head(&missing).await.unwrap(), None);
    match storage.read_range(&missing, 0, 1).await.unwrap_err() {
        StorageError::Io(err) => assert_eq!(err.kind(), std::io::ErrorKind::NotFound),
        other => panic!("unexpected {other:?}"),
    }
    // Keys are validated before any request is signed.
    assert!(matches!(
        storage.head("../etc/passwd").await.unwrap_err(),
        StorageError::InvalidKey
    ));
}

#[tokio::test]
async fn s3_errors_never_expose_credentials() {
    let good = s3_settings();
    let mut bad = good.clone();
    bad.secret_access_key = format!("{}-wrong", good.secret_access_key);
    let storage = S3Storage::new(bad.clone()).unwrap();
    let err = storage.head_bucket().await.unwrap_err().to_string();
    assert!(err.contains("403"), "{err}");
    let key = Uuid::now_v7().to_string();
    let err2 = storage
        .create_multipart(&key)
        .await
        .unwrap_err()
        .to_string();
    assert!(err2.contains("SignatureDoesNotMatch"), "{err2}");
    for message in [&err, &err2] {
        assert!(!message.contains(&bad.secret_access_key), "{message}");
        assert!(!message.contains(&good.access_key_id), "{message}");
        assert!(!message.contains("X-Amz-"), "{message}");
    }

    // Transport failure: the signed URL (key id + signature) is stripped.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let closed = listener.local_addr().unwrap();
    drop(listener);
    let mut unreachable = good.clone();
    unreachable.endpoint = format!("http://{closed}");
    let storage = S3Storage::new(unreachable).unwrap();
    let err = storage.head(&key).await.unwrap_err().to_string();
    assert!(!err.contains(&good.access_key_id), "{err}");
    assert!(!err.contains("X-Amz-"), "{err}");
    assert!(!format!("{storage:?}").contains(&good.access_key_id));
}

async fn put_part(
    app: &axum::Router,
    cookie: &str,
    url: &str,
    body: &[u8],
) -> (StatusCode, String) {
    let (status, _, headers) = request(
        app.clone(),
        "PUT",
        url,
        Some(body.to_vec()),
        Some("application/octet-stream"),
        Some(cookie),
    )
    .await;
    let etag = headers
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    (status, etag)
}

#[tokio::test]
async fn s3_http_two_part_upload_resume_and_range_download() {
    let harness = TestDb::bootstrap().await;
    let storage = s3_backend().await;
    let part_size = 5 * MIB;
    let (app, cookie, workspace_id) =
        setup_session_with_part_size(&harness, storage.clone(), part_size as i64).await;
    let document_id = create_document(&app, &cookie, workspace_id).await;
    let mut payload = PNG_BYTES.to_vec();
    payload.extend(patterned(part_size + 1000 - PNG_BYTES.len(), 9));
    let (status, created, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/uploads"),
        Some(json!({ "name": "big.png", "sizeBytes": payload.len(), "declaredMime": "image/png" })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create: {created:?}");
    let attachment_id = created["attachmentId"].as_str().unwrap().to_string();
    let parts = created["parts"].as_array().unwrap();
    assert_eq!(parts.len(), 2);
    let (status, etag2) = put_part(
        &app,
        &cookie,
        parts[1]["url"].as_str().unwrap(),
        &payload[part_size..],
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Resume lists what S3 holds for this upload.
    let (status, resume, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/upload"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "resume: {resume:?}");
    assert_eq!(resume["uploadedParts"].as_array().unwrap().len(), 1);
    assert_eq!(resume["uploadedParts"][0]["partNumber"], 2);

    // An oversized part is refused by the API before reaching S3.
    let (status, _) = put_part(
        &app,
        &cookie,
        parts[0]["url"].as_str().unwrap(),
        &patterned(part_size + 1, 3),
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);

    let (status, etag1) = put_part(
        &app,
        &cookie,
        parts[0]["url"].as_str().unwrap(),
        &payload[..part_size],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, completed, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/complete"),
        Some(json!({ "parts": [
            { "partNumber": 1, "etag": etag1 },
            { "partNumber": 2, "etag": etag2 }
        ] })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "complete: {completed:?}");
    assert_eq!(completed["mime"], "image/png");

    let (status, bytes, _) = request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/download"),
        None,
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, payload);

    let start = part_size - 10;
    let end = part_size + 10;
    let mut req = Request::builder()
        .method("GET")
        .uri(format!(
            "/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/download"
        ))
        .header("cookie", format!("fvoci_session={cookie}"))
        .header("range", format!("bytes={start}-{end}"))
        .body(axum::body::Body::empty())
        .unwrap();
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(test_peer()));
    let response = app.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        response.headers()["content-range"],
        format!("bytes {start}-{end}/{}", payload.len()).as_str()
    );
    let ranged = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&ranged[..], &payload[start..=end]);
    harness.cleanup().await;
}

/// One bounded maintenance upload-GC pass; returns the number of rows purged.
async fn gc_stale_uploads(
    pool: &sqlx::PgPool,
    storage: &ObjectStorage,
    cutoff: chrono::DateTime<Utc>,
) -> Result<u32, sqlx::Error> {
    fvoci_server::jobs::run_stale_upload_gc(
        pool,
        storage,
        cutoff,
        None,
        fvoci_server::jobs::UPLOAD_GC_BATCH,
        &tokio_util::sync::CancellationToken::new(),
    )
    .await
    .map(|stats| stats.purged)
}

/// A part body that records whether the server ever polled it.
fn tracked_body(chunks: Vec<Vec<u8>>) -> (axum::body::Body, Arc<std::sync::atomic::AtomicBool>) {
    let polled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = polled.clone();
    let mut chunks = chunks.into_iter();
    let body = stream::poll_fn(move |_| {
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
        std::task::Poll::Ready(
            chunks
                .next()
                .map(|chunk| Ok::<Bytes, std::io::Error>(Bytes::from(chunk))),
        )
    });
    (axum::body::Body::from_stream(body), polled)
}

async fn put_raw(
    app: &axum::Router,
    cookie: &str,
    url: &str,
    content_length: Option<u64>,
    body: axum::body::Body,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method("PUT")
        .uri(url)
        .header("cookie", format!("fvoci_session={cookie}"))
        .header("content-type", "application/octet-stream");
    if let Some(len) = content_length {
        builder = builder.header("content-length", len);
    }
    let mut req = builder.body(body).unwrap();
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(test_peer()));
    let response = app.clone().oneshot(req).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn upload_ref_of(harness: &TestDb, attachment_id: &str) -> (String, Option<String>) {
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let row: (String, Option<String>) = sqlx::query_as(
        "SELECT storage_key, upload_meta->>'upload_ref' FROM fvoci.attachments WHERE id = $1",
    )
    .bind(Uuid::parse_str(attachment_id).unwrap())
    .fetch_one(&admin)
    .await
    .unwrap();
    admin.close().await;
    row
}

/// Review B1: proxied S3 parts stream with their declared length. An
/// oversized or undeclared length is refused before the body is read, and a
/// body that disagrees with its length never becomes an S3 part.
#[tokio::test]
async fn s3_part_put_streams_with_a_bounded_declared_length() {
    let harness = TestDb::bootstrap().await;
    let storage = s3_backend().await;
    let (app, cookie, workspace_id) = setup_session(&harness, storage.clone()).await;
    let document_id = create_document(&app, &cookie, workspace_id).await;
    let payload = patterned(1000, 5);
    let (status, created, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/uploads"),
        Some(json!({ "name": "len.bin", "sizeBytes": payload.len() })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created:?}");
    let attachment_id = created["attachmentId"].as_str().unwrap().to_string();
    let part_url = created["parts"][0]["url"].as_str().unwrap().to_string();
    let (key, upload_id) = upload_ref_of(&harness, &attachment_id).await;
    let upload_id = upload_id.expect("upload id persisted");

    // Declared length above the part maximum: 413 without reading a byte.
    let (body, polled) = tracked_body(vec![vec![0u8; 5000]]);
    let (status, problem) = put_raw(&app, &cookie, &part_url, Some(5000), body).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{problem:?}");
    assert!(!polled.load(std::sync::atomic::Ordering::SeqCst));

    // No declared length (chunked): refused before reading, nothing buffered.
    let (body, polled) = tracked_body(vec![payload.clone()]);
    let (status, problem) = put_raw(&app, &cookie, &part_url, None, body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{problem:?}");
    assert_eq!(problem["code"], "invalid_input", "{problem:?}");
    assert!(!polled.load(std::sync::atomic::Ordering::SeqCst));

    // Short and long bodies against a valid declared length fail the S3
    // request, so no part is stored.
    let (body, _) = tracked_body(vec![payload[..500].to_vec()]);
    let (status, problem) = put_raw(&app, &cookie, &part_url, Some(1000), body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "short: {problem:?}");
    let (body, _) = tracked_body(vec![payload.clone(), vec![1u8; 500]]);
    let (status, problem) = put_raw(&app, &cookie, &part_url, Some(1000), body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "long: {problem:?}");
    // Review D3: the client's body breaking off mid-stream (disconnect) is a
    // 400 client error, not a 500, and stores nothing.
    let broken = axum::body::Body::from_stream(stream::iter(vec![
        Ok::<Bytes, std::io::Error>(Bytes::copy_from_slice(&payload[..100])),
        Err(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "client went away",
        )),
    ]));
    let (status, problem) = put_raw(&app, &cookie, &part_url, Some(1000), broken).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "disconnect: {problem:?}");
    assert_eq!(problem["code"], "invalid_input", "{problem:?}");
    let parts = storage.list_parts(&key, Some(&upload_id)).await.unwrap();
    assert!(
        parts.is_empty(),
        "rejected bodies must not be parts: {parts:?}"
    );

    // The exact declared length streams through in chunks and completes.
    let (body, _) = tracked_body(payload.chunks(256).map(<[u8]>::to_vec).collect());
    let (status, problem) = put_raw(&app, &cookie, &part_url, Some(1000), body).await;
    assert_eq!(status, StatusCode::OK, "{problem:?}");
    let parts = storage.list_parts(&key, Some(&upload_id)).await.unwrap();
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0].size_bytes, 1000);
    let (status, completed, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/complete"),
        Some(json!({ "parts": [{ "partNumber": 1, "etag": parts[0].etag }] })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "complete: {completed:?}");
    assert_eq!(
        storage.read_range(&key, 0, 999).await.unwrap(),
        payload,
        "streamed part must round-trip"
    );
    harness.cleanup().await;
}

/// Review N4: a non-final part under the S3 minimum makes complete answer a
/// 4xx parts problem and leaves the upload resumable, not a 500.
#[tokio::test]
async fn s3_complete_with_too_small_part_is_a_client_error() {
    let harness = TestDb::bootstrap().await;
    let storage = s3_backend().await;
    let part_size = 5 * MIB;
    let (app, cookie, workspace_id) =
        setup_session_with_part_size(&harness, storage.clone(), part_size as i64).await;
    let document_id = create_document(&app, &cookie, workspace_id).await;
    let (status, created, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/uploads"),
        Some(json!({ "name": "small.bin", "sizeBytes": part_size + 1000 })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created:?}");
    let attachment_id = created["attachmentId"].as_str().unwrap().to_string();
    let parts = created["parts"].as_array().unwrap();
    // Part 1 is below its maximum (allowed per request) but below 5 MiB.
    let (status, etag1) = put_part(
        &app,
        &cookie,
        parts[0]["url"].as_str().unwrap(),
        &patterned(1000, 1),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, etag2) = put_part(
        &app,
        &cookie,
        parts[1]["url"].as_str().unwrap(),
        &patterned(1000, 2),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, problem, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/complete"),
        Some(json!({ "parts": [
            { "partNumber": 1, "etag": etag1 },
            { "partNumber": 2, "etag": etag2 }
        ] })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{problem:?}");
    assert_eq!(
        problem["code"], "submitted_parts_do_not_match_uploaded_parts",
        "{problem:?}"
    );
    let (status, resume, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/upload"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "still resumable: {resume:?}");
    assert_eq!(resume["uploadedParts"].as_array().unwrap().len(), 2);
    harness.cleanup().await;
}

/// Review N1, pinned semantics: with S3 the part bytes reach the multipart
/// upload before `commit_upload_part` rechecks the session (as with the
/// source's presigned PUTs). A PUT whose session is revoked mid-request gets
/// a 4xx and the part is listed by S3, but it is never published: complete
/// with the revoked session is refused and no object exists.
#[tokio::test]
async fn s3_revocation_during_part_put_is_refused_and_never_published() {
    use fvoci_server::db::attachments::test_barrier;

    let harness = TestDb::bootstrap().await;
    let storage = s3_backend().await;
    let (app, cookie, workspace_id) = setup_session(&harness, storage.clone()).await;
    let document_id = create_document(&app, &cookie, workspace_id).await;
    let payload = b"revoked-mid-put".to_vec();
    let (status, created, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/uploads"),
        Some(json!({ "name": "revoke.bin", "sizeBytes": payload.len() })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created:?}");
    let attachment_id = created["attachmentId"].as_str().unwrap().to_string();
    let part_url = created["parts"][0]["url"].as_str().unwrap().to_string();
    let (key, upload_id) = upload_ref_of(&harness, &attachment_id).await;
    let upload_id = upload_id.expect("upload id persisted");

    let mut barrier = test_barrier::arm_pre_publish(Uuid::parse_str(&attachment_id).unwrap());
    let put = tokio::spawn({
        let app = app.clone();
        let cookie = cookie.clone();
        let payload = payload.clone();
        async move { put_part(&app, &cookie, &part_url, &payload).await }
    });
    tokio::time::timeout(Duration::from_secs(30), barrier.wait_entered())
        .await
        .expect("PUT should reach the pre-publish barrier")
        .expect("barrier entered");
    let (status, _, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/auth/logout",
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    barrier.proceed();
    let (status, _) = put.await.unwrap();
    assert!(status.is_client_error(), "revoked PUT answered {status}");

    let listed = storage.list_parts(&key, Some(&upload_id)).await.unwrap();
    assert_eq!(listed.len(), 1, "S3 holds the part bytes: {listed:?}");
    let (status, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/complete"),
        Some(json!({ "parts": [{ "partNumber": 1, "etag": listed[0].etag }] })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(storage.head(&key).await.unwrap(), None, "never published");
    harness.cleanup().await;
}

/// Workspace purge with S3: every key's open multipart uploads (including an
/// orphan whose id never reached the row) are aborted and objects deleted
/// before the DB rows go. Wrong credentials or a missing bucket keep every
/// row for the next sweep; an object that is already gone counts as deleted.
#[tokio::test]
async fn s3_workspace_purge_cleans_storage_before_rows() {
    let harness = TestDb::bootstrap().await;
    let storage = s3_backend().await;
    let (app, cookie, _) = setup_session(&harness, storage.clone()).await;
    let (status, body, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/workspaces",
        Some(json!({ "name": "Purge", "slug": "s3purge" })),
        Some(&cookie),
    )
    .await;
    assert!(status.is_success(), "create workspace: {status} {body:?}");
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let ws: Uuid = sqlx::query_scalar("SELECT id FROM fvoci.workspaces WHERE slug = 's3purge'")
        .fetch_one(&admin)
        .await
        .unwrap();
    let document_id = create_document(&app, &cookie, ws).await;

    // One stored attachment.
    let (status, created, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/documents/{document_id}/uploads"),
        Some(json!({ "name": "kept.png", "sizeBytes": PNG_BYTES.len(), "declaredMime": "image/png" })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created:?}");
    let stored_id = created["attachmentId"].as_str().unwrap().to_string();
    let (status, etag) = put_part(
        &app,
        &cookie,
        created["parts"][0]["url"].as_str().unwrap(),
        PNG_BYTES,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, completed, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/attachments/{stored_id}/complete"),
        Some(json!({ "parts": [{ "partNumber": 1, "etag": etag }] })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{completed:?}");
    let (stored_key, _) = upload_ref_of(&harness, &stored_id).await;

    // One in-flight upload with a part, plus an orphan upload on its key.
    let (status, created, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/documents/{document_id}/uploads"),
        Some(json!({ "name": "inflight.bin", "sizeBytes": 7 })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created:?}");
    let inflight_id = created["attachmentId"].as_str().unwrap().to_string();
    let (status, _) = put_part(
        &app,
        &cookie,
        created["parts"][0]["url"].as_str().unwrap(),
        b"inflite",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (inflight_key, _) = upload_ref_of(&harness, &inflight_id).await;
    storage
        .create_multipart(&inflight_key)
        .await
        .unwrap()
        .expect("orphan upload");
    assert_eq!(
        storage
            .list_multipart_uploads(&inflight_key)
            .await
            .unwrap()
            .len(),
        2
    );

    let (status, body, _) = json_request(
        app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{ws}"),
        Some(json!({ "confirmSlug": "s3purge" })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    sqlx::query(
        "UPDATE fvoci.workspaces SET deleted_at = now() - interval '31 days' WHERE id = $1",
    )
    .bind(ws)
    .execute(&admin)
    .await
    .unwrap();

    let pool = pool::connect_app(&harness.app_url).await.unwrap();
    let rows_left = |admin: &sqlx::PgPool| {
        let admin = admin.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM fvoci.attachments WHERE workspace_id = $1",
            )
            .bind(ws)
            .fetch_one(&admin)
            .await
            .unwrap()
        }
    };
    let cancel = tokio_util::sync::CancellationToken::new();

    // Wrong secret (403) and a missing bucket (404 NoSuchBucket) are
    // failures: storage and rows are both kept.
    let mut bad_secret = s3_settings();
    bad_secret.secret_access_key = "wrong-secret-for-purge".into();
    let mut missing_bucket = s3_settings();
    missing_bucket.bucket = format!("fvoci-missing-{}", Uuid::now_v7().simple());
    for settings in [bad_secret, missing_bucket] {
        let broken = ObjectStorage::from(S3Storage::new(settings).unwrap());
        let stats = fvoci_server::jobs::run_workspace_purge(&pool, &broken, Utc::now(), &cancel)
            .await
            .unwrap();
        assert_eq!(stats.purged, 0, "{stats:?}");
        assert!(stats.storage_failed > 0, "{stats:?}");
        assert_eq!(rows_left(&admin).await, 2);
        assert!(storage.head(&stored_key).await.unwrap().is_some());
        assert_eq!(
            storage
                .list_multipart_uploads(&inflight_key)
                .await
                .unwrap()
                .len(),
            2
        );
    }

    // A crash after an earlier storage delete: the object is already gone.
    storage.delete_object(&stored_key).await.unwrap();
    let stats = fvoci_server::jobs::run_workspace_purge(&pool, &storage, Utc::now(), &cancel)
        .await
        .unwrap();
    assert_eq!(stats.purged, 1, "{stats:?}");
    assert_eq!(stats.storage_failed, 0, "{stats:?}");
    assert_eq!(rows_left(&admin).await, 0);
    assert_eq!(storage.head(&stored_key).await.unwrap(), None);
    assert_eq!(storage.head(&inflight_key).await.unwrap(), None);
    assert!(storage
        .list_multipart_uploads(&inflight_key)
        .await
        .unwrap()
        .is_empty());
    let gone: Option<Uuid> = sqlx::query_scalar("SELECT id FROM fvoci.workspaces WHERE id = $1")
        .bind(ws)
        .fetch_optional(&admin)
        .await
        .unwrap();
    assert!(gone.is_none());
    let again = fvoci_server::jobs::run_workspace_purge(&pool, &storage, Utc::now(), &cancel)
        .await
        .unwrap();
    assert_eq!(again.purged, 0);
    pool.close().await;
    admin.close().await;
    harness.cleanup().await;
}

/// Native extraction reads the original through `ObjectStorage`: for S3 that
/// is a ranged GET of the whole object (across a part boundary), with the
/// size limit enforced from HeadObject before any byte is read.
#[tokio::test]
async fn s3_extract_input_reads_original_through_object_storage() {
    let storage = s3_backend().await;
    let key = Uuid::now_v7().to_string();
    let upload_id = storage.create_multipart(&key).await.unwrap().unwrap();
    let first = patterned(5 * MIB, 3);
    let second = patterned(4096, 4);
    let a = stage_and_publish(&storage, &key, &upload_id, 1, &first).await;
    let b = stage_and_publish(&storage, &key, &upload_id, 2, &second).await;
    storage
        .assemble_multipart(&key, Some(&upload_id), &[(1, a.etag), (2, b.etag)])
        .await
        .unwrap();
    let mut expected = first;
    expected.extend(&second);
    let read = fvoci_server::attachments::read_extract_input(&storage, &key, 64 * MIB as u64)
        .await
        .unwrap();
    assert_eq!(read, expected);
    let too_big =
        fvoci_server::attachments::read_extract_input(&storage, &key, expected.len() as u64 - 1)
            .await
            .unwrap_err();
    assert!(too_big.contains("exceeds"), "{too_big}");
    storage.delete_object(&key).await.unwrap();
    let missing = fvoci_server::attachments::read_extract_input(&storage, &key, 64 * MIB as u64)
        .await
        .unwrap_err();
    assert!(missing.contains("missing"), "{missing}");
}

/// Post-restore check (`fvoci-migrate --verify-storage`): every stored
/// attachment must exist in the bucket with its recorded size. A missing
/// object is reported; a storage error is a failure, never "missing".
#[tokio::test]
async fn s3_verify_stored_objects_reports_missing_and_fails_on_storage_errors() {
    let harness = TestDb::bootstrap().await;
    let storage = s3_backend().await;
    let (app, cookie, workspace_id) = setup_session(&harness, storage.clone()).await;
    let document_id = create_document(&app, &cookie, workspace_id).await;
    let (status, created, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/uploads"),
        Some(json!({ "name": "verify.png", "sizeBytes": PNG_BYTES.len(), "declaredMime": "image/png" })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created:?}");
    let attachment_id = created["attachmentId"].as_str().unwrap().to_string();
    let (status, etag) = put_part(
        &app,
        &cookie,
        created["parts"][0]["url"].as_str().unwrap(),
        PNG_BYTES,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, completed, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/complete"),
        Some(json!({ "parts": [{ "partNumber": 1, "etag": etag }] })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{completed:?}");
    let (key, _) = upload_ref_of(&harness, &attachment_id).await;

    let pool = pool::connect_app(&harness.app_url).await.unwrap();
    let report = fvoci_server::attachments::verify_stored_objects(&pool, &storage)
        .await
        .unwrap();
    assert_eq!(report.checked, 1);
    assert!(report.is_complete(), "{report:?}");

    let mut bad_secret = s3_settings();
    bad_secret.secret_access_key = "wrong-secret-for-verify".into();
    let broken = ObjectStorage::from(S3Storage::new(bad_secret).unwrap());
    assert!(
        fvoci_server::attachments::verify_stored_objects(&pool, &broken)
            .await
            .is_err(),
        "a 403 must fail the check, not report the object missing"
    );

    storage.delete_object(&key).await.unwrap();
    let report = fvoci_server::attachments::verify_stored_objects(&pool, &storage)
        .await
        .unwrap();
    assert_eq!(
        report.missing,
        vec![Uuid::parse_str(&attachment_id).unwrap()]
    );
    assert!(!report.is_complete());
    pool.close().await;
    harness.cleanup().await;
}

/// Review D2: part PUTs in flight are bounded per process. With every slot
/// taken, a PUT is refused with 503 + Retry-After before its body is read,
/// and succeeds once a slot frees up.
#[tokio::test]
async fn s3_part_put_slots_bound_concurrent_uploads() {
    use fvoci_server::db::attachments::test_barrier;

    let harness = TestDb::bootstrap().await;
    let storage = s3_backend().await;
    let mut state = app_state_with_part_size(
        &harness.app_url,
        storage.clone(),
        fvoci_server::config::DEFAULT_UPLOAD_PART_SIZE_BYTES,
    )
    .await;
    state.upload.part_put_slots = fvoci_server::attachments::PartPutSlots::new(1);
    let (app, cookie, workspace_id) = setup_session_with_state(&harness, state).await;
    let document_id = create_document(&app, &cookie, workspace_id).await;
    let mut urls = Vec::new();
    let mut ids = Vec::new();
    for name in ["slot-a.bin", "slot-b.bin"] {
        let (status, created, _) = json_request(
            app.clone(),
            "POST",
            &format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/uploads"),
            Some(json!({ "name": name, "sizeBytes": 4 })),
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{created:?}");
        ids.push(created["attachmentId"].as_str().unwrap().to_string());
        urls.push(created["parts"][0]["url"].as_str().unwrap().to_string());
    }

    // The first PUT holds the only slot while parked before publish.
    let mut barrier = test_barrier::arm_pre_publish(Uuid::parse_str(&ids[0]).unwrap());
    let first = tokio::spawn({
        let app = app.clone();
        let cookie = cookie.clone();
        let url = urls[0].clone();
        async move { put_part(&app, &cookie, &url, b"aaaa").await }
    });
    tokio::time::timeout(Duration::from_secs(30), barrier.wait_entered())
        .await
        .expect("first PUT should reach the pre-publish barrier")
        .expect("barrier entered");

    let (body, polled) = tracked_body(vec![b"bbbb".to_vec()]);
    let mut req = Request::builder()
        .method("PUT")
        .uri(&urls[1])
        .header("cookie", format!("fvoci_session={cookie}"))
        .header("content-type", "application/octet-stream")
        .header("content-length", 4)
        .body(body)
        .unwrap();
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(test_peer()));
    let response = app.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()["retry-after"], "2");
    let problem: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(problem["code"], "upload_capacity_exceeded", "{problem:?}");
    assert!(!polled.load(std::sync::atomic::Ordering::SeqCst));

    barrier.proceed();
    let (status, _) = first.await.unwrap();
    assert_eq!(status, StatusCode::OK);
    // The slot is released with the finished request.
    let (status, _) = put_part(&app, &cookie, &urls[1], b"bbbb").await;
    assert_eq!(status, StatusCode::OK);
    harness.cleanup().await;
}

#[tokio::test]
async fn s3_image_preview_is_stored_served_and_reclaimed() {
    let harness = TestDb::bootstrap().await;
    let storage = s3_backend().await;
    let state = app_state_with_part_size(
        &harness.app_url,
        storage.clone(),
        fvoci_server::config::DEFAULT_UPLOAD_PART_SIZE_BYTES,
    )
    .await;
    let pool = state.auth.db.pool.clone();
    let (app, cookie, workspace_id) = setup_session_with_state(&harness, state).await;
    let document_id = create_document(&app, &cookie, workspace_id).await;

    let img = image::RgbaImage::from_fn(2000, 1000, |x, y| {
        image::Rgba([(x % 251) as u8, (y % 241) as u8, 7, 255])
    });
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let (status, created, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/uploads"),
        Some(json!({"name": "s3.png", "sizeBytes": png.len()})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let attachment_id = created["attachmentId"].as_str().unwrap().to_string();
    let (status, _, headers) = request(
        app.clone(),
        "PUT",
        created["parts"][0]["url"].as_str().unwrap(),
        Some(png.clone()),
        Some("application/octet-stream"),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let etag = headers["etag"].to_str().unwrap().to_string();
    let (status, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/complete"),
        Some(json!({"parts": [{"partNumber": 1, "etag": etag}]})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let settings = fvoci_server::attachments::PreviewJobSettings::new(std::path::PathBuf::from(
        env!("CARGO_BIN_EXE_fvoci-server"),
    ));
    let worked = fvoci_server::attachments::process_one_preview(
        &settings,
        &pool,
        &storage,
        &tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(worked);
    let (status, meta, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(meta["preview"], json!({"width": 1600, "height": 800}));
    let (status, body, headers) = request(
        app.clone(),
        "GET",
        &format!(
            "/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/download?variant=preview"
        ),
        None,
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["content-type"], "image/webp");
    let decoded = image::load_from_memory_with_format(&body, image::ImageFormat::WebP).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (1600, 800));

    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let (original, preview): (String, String) = sqlx::query_as(
        "SELECT storage_key, variants -> 'preview' ->> 'key' FROM fvoci.attachments WHERE id = $1",
    )
    .bind(Uuid::parse_str(&attachment_id).unwrap())
    .fetch_one(&admin)
    .await
    .unwrap();
    admin.close().await;
    let (status, _, _) = json_request(
        app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(storage.head(&original).await.unwrap(), None);
    assert_eq!(storage.head(&preview).await.unwrap(), None);
    harness.cleanup().await;
}

/// `preview-html` for an office file the extract job has not reached reads
/// the original back from S3 and parses it in the isolated office child;
/// nothing is written to the extract columns.
#[tokio::test]
async fn s3_preview_html_parses_a_not_yet_extracted_office_file_on_demand() {
    let harness = TestDb::bootstrap().await;
    let mut state = app_state_with_part_size(
        &harness.app_url,
        s3_backend().await,
        fvoci_server::config::DEFAULT_UPLOAD_PART_SIZE_BYTES,
    )
    .await;
    state.preview_extract = Some(fvoci_server::attachments::PreviewExtractor::new(
        None,
        Some(std::path::PathBuf::from(env!("CARGO_BIN_EXE_fvoci-server"))),
    ));
    let (app, cookie, workspace_id) = setup_session_with_state(&harness, state).await;
    let document_id = create_document(&app, &cookie, workspace_id).await;
    let docx = office_fixtures::docx("S3 회의록", &["<b>S3 즉석 본문</b> & 끝"]);

    let (status, created, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/uploads"),
        Some(json!({ "name": "memo.docx", "sizeBytes": docx.len() })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create: {created:?}");
    let attachment_id = created["attachmentId"].as_str().unwrap().to_string();
    let part_url = created["parts"][0]["url"].as_str().unwrap();
    let (status, _, headers) = request(
        app.clone(),
        "PUT",
        part_url,
        Some(docx.clone()),
        Some("application/octet-stream"),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let etag = headers
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .expect("etag")
        .to_string();
    let (status, completed, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/complete"),
        Some(json!({ "parts": [{ "partNumber": 1, "etag": etag }] })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "complete: {completed:?}");

    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let extract_state = |admin: sqlx::PgPool| {
        let id = Uuid::parse_str(&attachment_id).unwrap();
        async move {
            sqlx::query_scalar::<_, String>(
                "SELECT row(extract_text, extract_status, extract_attempts, extract_warnings,
                            extract_lease_token, extract_lease_expires_at, extract_rhwp_rev)::text
                 FROM fvoci.attachments WHERE id = $1",
            )
            .bind(id)
            .fetch_one(&admin)
            .await
            .unwrap()
        }
    };
    let before = extract_state(admin.clone()).await;
    assert!(before.starts_with("(\"\",pending,0,"), "{before}");

    let (status, body, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/preview-html"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let html = body["html"].as_str().unwrap();
    assert!(html.starts_with("<pre>S3 회의록"), "{html}");
    assert!(
        html.contains("&lt;b&gt;S3 즉석 본문&lt;/b&gt; &amp; 끝"),
        "{html}"
    );
    assert_eq!(extract_state(admin.clone()).await, before);
    admin.close().await;
    harness.cleanup().await;
}

// ------------------------------------------------ transfer modes (#149 A/B)
//
// The same scenarios run in `proxy` (bytes through the API) and `presigned`
// (browser <-> MinIO with signed URLs) modes. A plain reqwest client with no
// cookie store and no redirect following plays the browser, so every storage
// request it makes carries only what the signed URL grants.

use fvoci_server::attachments::{PresignTtls, TransferMode};

/// Browsers reach MinIO under another host name than the server's internal
/// `S3_ENDPOINT`, as with a dedicated `files.example.com`: the signed `Host`
/// must be the public one.
fn public_endpoint() -> String {
    let mut url = url::Url::parse(&nonempty_env("S3_ENDPOINT")).unwrap();
    url.set_host(Some("localhost")).unwrap();
    url.as_str().trim_end_matches('/').to_string()
}

fn presign_s3_settings() -> S3Settings {
    S3Settings {
        public_endpoint: Some(public_endpoint()),
        ..s3_settings()
    }
}

async fn presign_backend(ttls: PresignTtls) -> ObjectStorage {
    let s3 = S3Storage::new(presign_s3_settings())
        .expect("s3 settings")
        .with_presign_ttls(ttls);
    s3.ensure_bucket().await.expect("ensure bucket");
    ObjectStorage::from(s3)
}

fn browser() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .unwrap()
}

fn mode_str(mode: TransferMode) -> &'static str {
    mode.as_str()
}

async fn patch_transfer(app: &axum::Router, cookie: &str, value: Value) -> (StatusCode, Value) {
    let (status, body, _) = json_request(
        app.clone(),
        "PATCH",
        "/api/v1/admin/instance-settings",
        Some(json!({ "attachmentTransfer": value })),
        Some(cookie),
    )
    .await;
    (status, body)
}

async fn admin_transfer_status(app: &axum::Router, cookie: &str) -> Value {
    let (status, body, _) = json_request(
        app.clone(),
        "GET",
        "/api/v1/admin/instance-settings",
        None,
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    body
}

async fn create_upload_session(
    app: &axum::Router,
    cookie: &str,
    workspace_id: Uuid,
    document_id: &str,
    name: &str,
    size: usize,
) -> Value {
    let (status, created, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/uploads"),
        Some(json!({ "name": name, "sizeBytes": size })),
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create: {created:?}");
    created
}

/// PUTs one part through the session's own path: the API with the session
/// cookie for `proxy`, the signed storage URL with nothing else for
/// `presigned` (no cookie, no `Authorization`, no `Content-Type`).
async fn put_via(
    app: &axum::Router,
    cookie: &str,
    mode: TransferMode,
    url: &str,
    body: &[u8],
) -> (StatusCode, String) {
    match mode {
        TransferMode::Proxy => put_part(app, cookie, url, body).await,
        TransferMode::Presigned => {
            let res = browser().put(url).body(body.to_vec()).send().await.unwrap();
            let status = StatusCode::from_u16(res.status().as_u16()).unwrap();
            let etag = res
                .headers()
                .get("etag")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string();
            (status, etag)
        }
    }
}

async fn complete_parts(
    app: &axum::Router,
    cookie: &str,
    workspace_id: Uuid,
    attachment_id: &str,
    parts: &[(i32, &str)],
) -> (StatusCode, Value) {
    let parts: Vec<Value> = parts
        .iter()
        .map(|(n, etag)| json!({ "partNumber": n, "etag": etag }))
        .collect();
    let (status, body, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/complete"),
        Some(json!({ "parts": parts })),
        Some(cookie),
    )
    .await;
    (status, body)
}

async fn resume_session(
    app: &axum::Router,
    cookie: &str,
    workspace_id: Uuid,
    attachment_id: &str,
) -> (StatusCode, Value) {
    let (status, body, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/upload"),
        None,
        Some(cookie),
    )
    .await;
    (status, body)
}

async fn download_request(
    app: &axum::Router,
    cookie: &str,
    method: &str,
    path: &str,
    range: Option<&str>,
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("cookie", format!("fvoci_session={cookie}"));
    if let Some(range) = range {
        builder = builder.header("range", range);
    }
    let mut req = builder.body(axum::body::Body::empty()).unwrap();
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(test_peer()));
    let response = app.clone().oneshot(req).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, headers, bytes.to_vec())
}

/// Original bytes (optionally a range) through the mode's own path: the API
/// streams them for `proxy`; for `presigned` the API answers a `302` that the
/// browser follows to storage, forwarding `Range`.
async fn fetch_original(
    app: &axum::Router,
    cookie: &str,
    mode: TransferMode,
    path: &str,
    range: Option<&str>,
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let (status, headers, body) = download_request(app, cookie, "GET", path, range).await;
    match mode {
        TransferMode::Proxy => {
            assert!(status.is_success(), "{status}");
            (status, headers, body)
        }
        TransferMode::Presigned => {
            assert_eq!(
                status,
                StatusCode::FOUND,
                "{:?}",
                String::from_utf8_lossy(&body)
            );
            assert_eq!(headers["cache-control"], "no-store");
            let location = headers["location"].to_str().unwrap().to_string();
            assert!(
                location.starts_with(&format!("{}/", public_endpoint())),
                "{location}"
            );
            let mut req = browser().get(&location);
            if let Some(range) = range {
                req = req.header("range", range);
            }
            let res = req.send().await.unwrap();
            let status = StatusCode::from_u16(res.status().as_u16()).unwrap();
            let mut out = HeaderMap::new();
            for (name, value) in res.headers() {
                out.insert(
                    axum::http::HeaderName::from_bytes(name.as_str().as_bytes()).unwrap(),
                    axum::http::HeaderValue::from_bytes(value.as_bytes()).unwrap(),
                );
            }
            (status, out, res.bytes().await.unwrap().to_vec())
        }
    }
}

async fn attachment_status(harness: &TestDb, attachment_id: &str) -> Option<String> {
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM fvoci.attachments WHERE id = $1")
            .bind(Uuid::parse_str(attachment_id).unwrap())
            .fetch_optional(&admin)
            .await
            .unwrap();
    admin.close().await;
    status
}

#[tokio::test]
async fn transfer_modes_upload_and_download_through_their_own_paths() {
    let harness = TestDb::bootstrap().await;
    let storage = presign_backend(PresignTtls::default()).await;
    let part_size = 5 * MIB;
    let (app, cookie, workspace_id) =
        setup_session_with_part_size(&harness, storage.clone(), part_size as i64).await;
    let document_id = create_document(&app, &cookie, workspace_id).await;
    let status = admin_transfer_status(&app, &cookie).await;
    assert_eq!(
        status["attachmentTransfer"],
        json!({"effective": "proxy", "source": "default", "presignedAvailable": true,
               "unavailableReason": null, "blocked": false})
    );
    assert_eq!(
        status["values"]["attachmentTransfer"],
        json!({"mode": "proxy"})
    );
    let (status, share, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/share-links"),
        Some(json!({})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{share:?}");
    let share_token = share["url"]
        .as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_string();

    for mode in [TransferMode::Proxy, TransferMode::Presigned] {
        let (status, body) = patch_transfer(&app, &cookie, json!({"mode": mode_str(mode)})).await;
        assert_eq!(status, StatusCode::OK, "{body:?}");
        assert_eq!(body["attachmentTransfer"]["effective"], mode_str(mode));
        assert_eq!(body["attachmentTransfer"]["source"], "stored");

        let name = format!("보고서 {} v1.bin", mode_str(mode));
        let payload = patterned(part_size + 1000, 7);
        let created = create_upload_session(
            &app,
            &cookie,
            workspace_id,
            &document_id,
            &name,
            payload.len(),
        )
        .await;
        assert_eq!(created["transfer"], mode_str(mode));
        let parts = created["parts"].as_array().unwrap().clone();
        assert_eq!(parts.len(), 2);
        match mode {
            TransferMode::Proxy => {
                assert_eq!(created["partUrlsExpireAt"], Value::Null);
                assert!(parts[0]["url"].as_str().unwrap().starts_with("/api/v1/"));
            }
            TransferMode::Presigned => {
                let expires: chrono::DateTime<Utc> =
                    serde_json::from_value(created["partUrlsExpireAt"].clone()).unwrap();
                let left = (expires - Utc::now()).num_seconds();
                assert!((890..=900).contains(&left), "{left}");
                for part in &parts {
                    let url = part["url"].as_str().unwrap();
                    assert!(url.starts_with(&format!("{}/", public_endpoint())), "{url}");
                    assert!(
                        url.contains("X-Amz-SignedHeaders=content-length%3Bhost"),
                        "{url}"
                    );
                }
            }
        }
        let attachment_id = created["attachmentId"].as_str().unwrap().to_string();
        let (s1, etag1) = put_via(
            &app,
            &cookie,
            mode,
            parts[0]["url"].as_str().unwrap(),
            &payload[..part_size],
        )
        .await;
        let (s2, etag2) = put_via(
            &app,
            &cookie,
            mode,
            parts[1]["url"].as_str().unwrap(),
            &payload[part_size..],
        )
        .await;
        assert_eq!((s1, s2), (StatusCode::OK, StatusCode::OK), "{mode:?}");
        assert!(!etag1.is_empty() && !etag2.is_empty(), "ETag exposed");
        let (status, completed) = complete_parts(
            &app,
            &cookie,
            workspace_id,
            &attachment_id,
            &[(1, &etag1), (2, &etag2)],
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{mode:?} complete: {completed:?}");

        let path =
            format!("/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/download");
        let (status, headers, bytes) = fetch_original(&app, &cookie, mode, &path, None).await;
        assert_eq!(status, StatusCode::OK, "{mode:?}");
        assert_eq!(bytes, payload, "{mode:?}");
        assert_eq!(headers["content-type"], "application/octet-stream");
        assert_eq!(
            headers["content-disposition"].to_str().unwrap(),
            fvoci_server::attachments::content_disposition_attachment(&name)
        );
        assert_eq!(headers["cache-control"], "private, no-store");

        let (start, end) = (part_size - 10, part_size + 10);
        let range = format!("bytes={start}-{end}");
        let (status, headers, bytes) =
            fetch_original(&app, &cookie, mode, &path, Some(&range)).await;
        assert_eq!(status, StatusCode::PARTIAL_CONTENT, "{mode:?}");
        assert_eq!(
            headers["content-range"],
            format!("bytes {start}-{end}/{}", payload.len()).as_str()
        );
        assert_eq!(&bytes[..], &payload[start..=end]);

        // An unsatisfiable range and HEAD are answered by the API itself.
        let (status, headers, _) =
            download_request(&app, &cookie, "GET", &path, Some("bytes=99999999-")).await;
        assert_eq!(status, StatusCode::RANGE_NOT_SATISFIABLE, "{mode:?}");
        assert!(headers.get("location").is_none());
        let (status, headers, body) = download_request(&app, &cookie, "HEAD", &path, None).await;
        assert_eq!(status, StatusCode::OK, "{mode:?}");
        assert!(body.is_empty());
        assert_eq!(
            headers["content-length"],
            payload.len().to_string().as_str()
        );
        assert_eq!(headers["content-security-policy"], "sandbox");
        // Share-link downloads always stream through the API.
        let shared = format!("/api/v1/share/{share_token}/attachments/{attachment_id}/download");
        let (status, headers, bytes) = download_request(&app, &cookie, "GET", &shared, None).await;
        assert_eq!(status, StatusCode::OK, "{mode:?}");
        assert!(headers.get("location").is_none());
        assert_eq!(bytes, payload, "{mode:?}");
    }

    // Both kinds of stored object pass the storage verification.
    let pool = pool::connect_app(&harness.app_url).await.unwrap();
    let report = fvoci_server::attachments::verify_stored_objects(&pool, &storage)
        .await
        .unwrap();
    assert_eq!(report.checked, 2);
    assert!(report.is_complete(), "{report:?}");
    pool.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn transfer_mode_switch_keeps_each_session_on_its_bound_path() {
    let harness = TestDb::bootstrap().await;
    let storage = presign_backend(PresignTtls::default()).await;
    let (app, cookie, workspace_id) = setup_session(&harness, storage.clone()).await;
    let document_id = create_document(&app, &cookie, workspace_id).await;
    let payload = patterned(3000, 3);

    // Session A starts in proxy mode, then the admin switches to presigned.
    let a = create_upload_session(
        &app,
        &cookie,
        workspace_id,
        &document_id,
        "a.bin",
        payload.len(),
    )
    .await;
    assert_eq!(a["transfer"], "proxy");
    let (status, _) = patch_transfer(&app, &cookie, json!({"mode": "presigned"})).await;
    assert_eq!(status, StatusCode::OK);
    let b = create_upload_session(
        &app,
        &cookie,
        workspace_id,
        &document_id,
        "b.bin",
        payload.len(),
    )
    .await;
    assert_eq!(b["transfer"], "presigned");

    // A keeps the API path after the switch.
    let (status, etag_a) = put_via(
        &app,
        &cookie,
        TransferMode::Proxy,
        a["parts"][0]["url"].as_str().unwrap(),
        &payload,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let a_id = a["attachmentId"].as_str().unwrap();
    let (status, body) = complete_parts(&app, &cookie, workspace_id, a_id, &[(1, &etag_a)]).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");

    // B never takes a part through the API: no second path for its bytes.
    let b_id = b["attachmentId"].as_str().unwrap().to_string();
    let proxy_path = format!("/api/v1/workspaces/{workspace_id}/attachments/{b_id}/parts/1");
    let (status, problem, _) = json_request(
        app.clone(),
        "PUT",
        &proxy_path,
        Some(json!("x")),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{problem:?}");
    assert_eq!(problem["code"], "upload_is_not_in_the_required_state");
    let (key_b, upload_b) = upload_ref_of(&harness, &b_id).await;
    assert!(
        storage
            .list_parts(&key_b, upload_b.as_deref())
            .await
            .unwrap()
            .is_empty(),
        "the refused PUT reached no storage"
    );

    // Switching back does not move B either: resume still signs storage URLs.
    let (status, _) = patch_transfer(&app, &cookie, json!({"mode": "proxy"})).await;
    assert_eq!(status, StatusCode::OK);
    let (status, resumed) = resume_session(&app, &cookie, workspace_id, &b_id).await;
    assert_eq!(status, StatusCode::OK, "{resumed:?}");
    assert_eq!(resumed["transfer"], "presigned");
    let url = resumed["parts"][0]["url"].as_str().unwrap();
    assert!(url.starts_with(&public_endpoint()), "{url}");
    let (status, etag_b) = put_via(&app, &cookie, TransferMode::Presigned, url, &payload).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = complete_parts(&app, &cookie, workspace_id, &b_id, &[(1, &etag_b)]).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");

    // New sessions follow the current mode; the switch also changes how
    // stored originals are served, whatever session uploaded them.
    let c = create_upload_session(&app, &cookie, workspace_id, &document_id, "c.bin", 10).await;
    assert_eq!(c["transfer"], "proxy");
    let path = format!("/api/v1/workspaces/{workspace_id}/attachments/{b_id}/download");
    let (status, _, bytes) = download_request(&app, &cookie, "GET", &path, None).await;
    assert_eq!((status, bytes), (StatusCode::OK, payload.clone()));
    harness.cleanup().await;
}

#[tokio::test]
async fn presigned_part_urls_expire_and_resume_issues_fresh_ones() {
    let harness = TestDb::bootstrap().await;
    let ttl = Duration::from_secs(3);
    let storage = presign_backend(PresignTtls {
        part: ttl,
        download: Duration::from_secs(60),
    })
    .await;
    let part_size = 5 * MIB;
    let (app, cookie, workspace_id) =
        setup_session_with_part_size(&harness, storage.clone(), part_size as i64).await;
    let document_id = create_document(&app, &cookie, workspace_id).await;
    let payload = patterned(part_size + 500, 11);

    for mode in [TransferMode::Proxy, TransferMode::Presigned] {
        let (status, _) = patch_transfer(&app, &cookie, json!({"mode": mode_str(mode)})).await;
        assert_eq!(status, StatusCode::OK);
        let created = create_upload_session(
            &app,
            &cookie,
            workspace_id,
            &document_id,
            "exp.bin",
            payload.len(),
        )
        .await;
        let id = created["attachmentId"].as_str().unwrap().to_string();
        let (status, etag1) = put_via(
            &app,
            &cookie,
            mode,
            created["parts"][0]["url"].as_str().unwrap(),
            &payload[..part_size],
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{mode:?}");
        tokio::time::sleep(ttl + Duration::from_secs(2)).await;
        let late = created["parts"][1]["url"].as_str().unwrap();
        let (status, _) = put_via(&app, &cookie, mode, late, &payload[part_size..]).await;
        match mode {
            // API paths do not expire; the session itself is checked instead.
            TransferMode::Proxy => {
                assert_eq!(status, StatusCode::OK);
                let (status, resumed) = resume_session(&app, &cookie, workspace_id, &id).await;
                assert_eq!(status, StatusCode::OK);
                assert_eq!(resumed["partUrlsExpireAt"], Value::Null);
                assert_eq!(resumed["uploadedParts"].as_array().unwrap().len(), 2);
                let etag2 = resumed["uploadedParts"][1]["etag"]
                    .as_str()
                    .unwrap()
                    .to_string();
                let (status, body) = complete_parts(
                    &app,
                    &cookie,
                    workspace_id,
                    &id,
                    &[(1, &etag1), (2, &etag2)],
                )
                .await;
                assert_eq!(status, StatusCode::OK, "{body:?}");
            }
            TransferMode::Presigned => {
                assert_eq!(status, StatusCode::FORBIDDEN, "expired URL must be refused");
                let (status, resumed) = resume_session(&app, &cookie, workspace_id, &id).await;
                assert_eq!(status, StatusCode::OK, "{resumed:?}");
                assert_eq!(resumed["uploadedParts"].as_array().unwrap().len(), 1);
                let fresh = resumed["parts"].as_array().unwrap();
                assert_eq!(fresh.len(), 1);
                assert_eq!(fresh[0]["partNumber"], 2);
                assert_ne!(fresh[0]["url"].as_str().unwrap(), late);
                let expires: chrono::DateTime<Utc> =
                    serde_json::from_value(resumed["partUrlsExpireAt"].clone()).unwrap();
                assert!(expires > Utc::now());
                let (status, etag2) = put_via(
                    &app,
                    &cookie,
                    mode,
                    fresh[0]["url"].as_str().unwrap(),
                    &payload[part_size..],
                )
                .await;
                assert_eq!(status, StatusCode::OK);
                let (status, body) = complete_parts(
                    &app,
                    &cookie,
                    workspace_id,
                    &id,
                    &[(1, &etag1), (2, &etag2)],
                )
                .await;
                assert_eq!(status, StatusCode::OK, "{body:?}");
            }
        }
        let path = format!("/api/v1/workspaces/{workspace_id}/attachments/{id}/download");
        let (_, _, bytes) = fetch_original(&app, &cookie, mode, &path, None).await;
        assert_eq!(bytes, payload, "{mode:?}");
    }
    harness.cleanup().await;
}

/// All parts reach storage, then the session is revoked while complete is
/// between assembly and marking the row stored: the final re-check refuses
/// it in both modes and nothing becomes a stored attachment.
#[tokio::test]
async fn complete_after_revocation_is_refused_in_both_modes() {
    use fvoci_server::db::attachments::test_barrier;

    for mode in [TransferMode::Proxy, TransferMode::Presigned] {
        let harness = TestDb::bootstrap().await;
        let storage = presign_backend(PresignTtls::default()).await;
        let (app, cookie, workspace_id) = setup_session(&harness, storage.clone()).await;
        let document_id = create_document(&app, &cookie, workspace_id).await;
        let (status, _) = patch_transfer(&app, &cookie, json!({"mode": mode_str(mode)})).await;
        assert_eq!(status, StatusCode::OK);
        let payload = patterned(2048, 1);
        let created = create_upload_session(
            &app,
            &cookie,
            workspace_id,
            &document_id,
            "r.bin",
            payload.len(),
        )
        .await;
        let id = created["attachmentId"].as_str().unwrap().to_string();
        let (status, etag) = put_via(
            &app,
            &cookie,
            mode,
            created["parts"][0]["url"].as_str().unwrap(),
            &payload,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{mode:?}");

        let mut barrier = test_barrier::arm_pre_mark_stored(Uuid::parse_str(&id).unwrap());
        let complete = tokio::spawn({
            let (app, cookie, id, etag) = (app.clone(), cookie.clone(), id.clone(), etag.clone());
            async move { complete_parts(&app, &cookie, workspace_id, &id, &[(1, &etag)]).await }
        });
        tokio::time::timeout(Duration::from_secs(30), barrier.wait_entered())
            .await
            .expect("complete should reach the pre-mark-stored barrier")
            .expect("barrier entered");
        let (status, _, _) = json_request(
            app.clone(),
            "POST",
            "/api/v1/auth/logout",
            None,
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        barrier.proceed();
        let (status, body) = complete.await.unwrap();
        assert_eq!(status, StatusCode::NOT_FOUND, "{mode:?}: {body:?}");
        assert_ne!(
            attachment_status(&harness, &id).await.as_deref(),
            Some("stored")
        );

        // The revoked session can neither resume (re-issue URLs) nor retry.
        let (status, _) = resume_session(&app, &cookie, workspace_id, &id).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _) = complete_parts(&app, &cookie, workspace_id, &id, &[(1, &etag)]).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        // The abandoned upload is reclaimed with its assembled object.
        let (key, _) = upload_ref_of(&harness, &id).await;
        let pool = pool::connect_app(&harness.app_url).await.unwrap();
        let purged = gc_stale_uploads(&pool, &storage, Utc::now() + chrono::Duration::hours(1))
            .await
            .unwrap();
        assert_eq!(purged, 1, "{mode:?}");
        assert_eq!(storage.head(&key).await.unwrap(), None, "{mode:?}");
        assert_eq!(attachment_status(&harness, &id).await, None);
        pool.close().await;
        harness.cleanup().await;
    }
}

#[tokio::test]
async fn mismatched_parts_are_refused_before_anything_is_published() {
    let harness = TestDb::bootstrap().await;
    let storage = presign_backend(PresignTtls::default()).await;
    let part_size = 5 * MIB;
    let (app, cookie, workspace_id) =
        setup_session_with_part_size(&harness, storage.clone(), part_size as i64).await;
    let document_id = create_document(&app, &cookie, workspace_id).await;
    let payload = patterned(part_size + 1000, 21);

    // Proxy: the API measures every part and S3 checks the named ETags.
    let (status, _) = patch_transfer(&app, &cookie, json!({"mode": "proxy"})).await;
    assert_eq!(status, StatusCode::OK);
    let created = create_upload_session(
        &app,
        &cookie,
        workspace_id,
        &document_id,
        "p.bin",
        payload.len(),
    )
    .await;
    let id = created["attachmentId"].as_str().unwrap().to_string();
    let (status, _) = put_via(
        &app,
        &cookie,
        TransferMode::Proxy,
        created["parts"][1]["url"].as_str().unwrap(),
        &payload[part_size - 1..],
    )
    .await;
    assert_eq!(
        status,
        StatusCode::PAYLOAD_TOO_LARGE,
        "a longer part is refused"
    );
    let mut etags = Vec::new();
    for (i, range) in [(0, 0..part_size), (1, part_size..payload.len())] {
        let (status, etag) = put_via(
            &app,
            &cookie,
            TransferMode::Proxy,
            created["parts"][i]["url"].as_str().unwrap(),
            &payload[range],
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        etags.push(etag);
    }
    let (status, problem) = complete_parts(
        &app,
        &cookie,
        workspace_id,
        &id,
        &[(1, &etags[0]), (2, "bogus")],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{problem:?}");
    assert_eq!(
        problem["code"],
        "submitted_parts_do_not_match_uploaded_parts"
    );
    let (key, _) = upload_ref_of(&harness, &id).await;
    assert_eq!(storage.head(&key).await.unwrap(), None);

    // Presigned: the server never saw the bytes, so storage must list exactly
    // the expected parts before `CompleteMultipartUpload` runs.
    let (status, _) = patch_transfer(&app, &cookie, json!({"mode": "presigned"})).await;
    assert_eq!(status, StatusCode::OK);
    let created = create_upload_session(
        &app,
        &cookie,
        workspace_id,
        &document_id,
        "s.bin",
        payload.len(),
    )
    .await;
    let id = created["attachmentId"].as_str().unwrap().to_string();
    let (key, upload_id) = upload_ref_of(&harness, &id).await;
    let upload_id = upload_id.unwrap();
    let url2 = created["parts"][1]["url"].as_str().unwrap().to_string();
    // The signed content-length refuses a body of another length.
    for body in [&payload[part_size + 1..], &payload[part_size - 1..]] {
        let (status, _) = put_via(&app, &cookie, TransferMode::Presigned, &url2, body).await;
        assert!(
            status.is_client_error(),
            "{} bytes answered {status}",
            body.len()
        );
    }
    assert!(storage
        .list_parts(&key, Some(&upload_id))
        .await
        .unwrap()
        .is_empty());
    let (status, etag1) = put_via(
        &app,
        &cookie,
        TransferMode::Presigned,
        created["parts"][0]["url"].as_str().unwrap(),
        &payload[..part_size],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // A part of the wrong length that did reach the upload anyway (storage
    // that ignores the signed length) is refused at complete.
    let short = stage_and_publish(&storage, &key, &upload_id, 2, &payload[part_size + 1..]).await;
    let (status, problem) = complete_parts(
        &app,
        &cookie,
        workspace_id,
        &id,
        &[(1, &etag1), (2, &short.etag)],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{problem:?}");
    assert_eq!(
        problem["code"],
        "submitted_parts_do_not_match_uploaded_parts"
    );
    assert_eq!(
        attachment_status(&harness, &id).await.as_deref(),
        Some("uploading")
    );
    assert_eq!(storage.head(&key).await.unwrap(), None, "nothing published");

    // Resume treats the wrong-length part as missing and signs a new URL.
    let (status, resumed) = resume_session(&app, &cookie, workspace_id, &id).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(resumed["uploadedParts"].as_array().unwrap().len(), 1);
    assert_eq!(resumed["parts"][0]["partNumber"], 2);
    let (status, etag2) = put_via(
        &app,
        &cookie,
        TransferMode::Presigned,
        resumed["parts"][0]["url"].as_str().unwrap(),
        &payload[part_size..],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    for bad in [
        vec![(1, etag1.as_str()), (2, "\"0000\"")],
        vec![(1, etag2.as_str()), (2, etag1.as_str())],
        vec![(1, etag1.as_str()), (1, etag1.as_str())],
    ] {
        let (status, problem) = complete_parts(&app, &cookie, workspace_id, &id, &bad).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad:?}: {problem:?}");
        assert_eq!(storage.head(&key).await.unwrap(), None, "{bad:?}");
    }
    let (status, problem) = complete_parts(&app, &cookie, workspace_id, &id, &[(1, &etag1)]).await;
    assert_eq!(
        (status, &problem["code"]),
        (StatusCode::BAD_REQUEST, &json!("invalid_input"))
    );
    let (status, body) = complete_parts(
        &app,
        &cookie,
        workspace_id,
        &id,
        &[(1, &etag1), (2, &etag2)],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");

    // Leftover part URLs cannot touch the published object: UploadPart only
    // writes into its (now completed) multipart upload, which S3 no longer
    // knows, and only the server can complete an upload.
    let (status, _) = put_via(
        &app,
        &cookie,
        TransferMode::Presigned,
        created["parts"][0]["url"].as_str().unwrap(),
        &patterned(part_size, 99),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "NoSuchUpload");
    let stored = storage
        .read_range(&key, 0, payload.len() as u64 - 1)
        .await
        .unwrap();
    assert_eq!(stored, payload);
    harness.cleanup().await;
}

#[tokio::test]
async fn presigned_mode_needs_presign_capable_storage() {
    let harness = TestDb::bootstrap().await;
    let capable = presign_backend(PresignTtls::default()).await;
    let (app, cookie, workspace_id) = setup_session(&harness, capable.clone()).await;
    let document_id = create_document(&app, &cookie, workspace_id).await;
    let (status, _) = patch_transfer(&app, &cookie, json!({"mode": "presigned"})).await;
    assert_eq!(status, StatusCode::OK);
    let bound = create_upload_session(&app, &cookie, workspace_id, &document_id, "b.bin", 64).await;
    assert_eq!(bound["transfer"], "presigned");
    let bound_id = bound["attachmentId"].as_str().unwrap().to_string();

    // The same database behind servers whose storage cannot presign.
    let local_root = std::env::temp_dir().join(format!("fvoci-transfer-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&local_root).unwrap();
    for (storage, reason) in [
        (s3_backend().await, "public_endpoint_missing"),
        (ObjectStorage::local(local_root.clone()), "storage_local"),
    ] {
        let local = storage.presign_unavailable()
            == Some(fvoci_server::attachments::TransferUnavailable::StorageLocal);
        let app = app_router(
            app_state_with_part_size(
                &harness.app_url,
                storage,
                fvoci_server::config::DEFAULT_UPLOAD_PART_SIZE_BYTES,
            )
            .await,
        );
        // The stored `presigned` stays, blocked: proxy applies and says why.
        let status = admin_transfer_status(&app, &cookie).await;
        assert_eq!(
            status["attachmentTransfer"],
            json!({"effective": "proxy", "source": "stored", "presignedAvailable": false,
                   "unavailableReason": reason, "blocked": true}),
            "{reason}"
        );
        assert_eq!(status["values"]["attachmentTransfer"]["mode"], "presigned");
        if !local {
            let created =
                create_upload_session(&app, &cookie, workspace_id, &document_id, "p.bin", 64).await;
            assert_eq!(created["transfer"], "proxy", "{reason}");
            // A session bound to presigned is never moved to the proxy path.
            let (status, problem) = resume_session(&app, &cookie, workspace_id, &bound_id).await;
            assert_eq!(status, StatusCode::CONFLICT, "{problem:?}");
            assert_eq!(problem["code"], "attachment_transfer_unavailable");
        }
        // Explicitly choosing presigned here is refused.
        let (status, problem) = patch_transfer(&app, &cookie, json!({"mode": "presigned"})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{reason}: {problem:?}");
        assert_eq!(problem["code"], "attachment_transfer_unavailable");
    }
    // Reset through the existing mechanism deletes the row.
    let (status, body) = patch_transfer(&app, &cookie, Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["attachmentTransfer"]["source"], "default");
    assert_eq!(body["attachmentTransfer"]["effective"], "proxy");
    assert!(!body["overridden"]
        .as_array()
        .unwrap()
        .contains(&json!("attachmentTransfer")));
    let _ = std::fs::remove_dir_all(&local_root);
    harness.cleanup().await;
}

/// A real `fvoci-server` process on this suite's database and MinIO: the
/// environment variables, restart persistence and startup refusals can only
/// be seen on a process, and its output is the log the operator gets.
struct TransferServer {
    child: std::process::Child,
    base: String,
    logs: Arc<std::sync::Mutex<Vec<String>>>,
}

impl Drop for TransferServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn transfer_server_command(harness: &TestDb, env: &[(&str, &str)]) -> std::process::Command {
    let s3 = s3_settings();
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_fvoci-server"));
    command
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("DATABASE_APP_URL", &harness.app_url)
        .env("PASSWORD_PEPPER_KEYS", PEPPER)
        .env("PASSWORD_PEPPER_ACTIVE_KEY_ID", "test")
        .env("FVOCI_BIND", "127.0.0.1:0")
        .env("FVOCI_PUBLIC_ORIGIN", "http://127.0.0.1:0")
        .env("FVOCI_COOKIE_SECURE", "0")
        .env("FVOCI_SHUTDOWN_DEADLINE_MS", "5000")
        .env("STORAGE_DRIVER", "s3")
        .env("S3_ENDPOINT", &s3.endpoint)
        .env("S3_REGION", &s3.region)
        .env("S3_BUCKET", &s3.bucket)
        .env("S3_ACCESS_KEY_ID", &s3.access_key_id)
        .env("S3_SECRET_ACCESS_KEY", &s3.secret_access_key)
        .env(
            "S3_FORCE_PATH_STYLE",
            if s3.force_path_style { "1" } else { "0" },
        )
        .env("S3_PUBLIC_ENDPOINT", public_endpoint())
        // Everything this crate and its HTTP stack log at debug.
        .env("RUST_LOG", "debug")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    for (name, value) in env {
        if value.is_empty() {
            command.env_remove(name);
        } else {
            command.env(name, value);
        }
    }
    command
}

fn spawn_transfer_server(mut command: std::process::Command) -> TransferServer {
    use std::io::{BufRead, BufReader};
    let mut child = command.spawn().expect("spawn fvoci-server");
    let logs = Arc::new(std::sync::Mutex::new(Vec::new()));
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    for stream in [
        Box::new(child.stdout.take().unwrap()) as Box<dyn std::io::Read + Send>,
        Box::new(child.stderr.take().unwrap()),
    ] {
        let (logs, tx) = (logs.clone(), tx.clone());
        std::thread::spawn(move || {
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                logs.lock().unwrap().push(line.clone());
                let _ = tx.send(line);
            }
        });
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let mut base = None;
    while base.is_none() && std::time::Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(line) => {
                if let Some(rest) = line.split("fvoci-server listening on ").nth(1) {
                    base = Some(rest.trim().trim_end_matches('/').to_string());
                }
            }
            Err(_) if child.try_wait().ok().flatten().is_some() => break,
            Err(_) => {}
        }
    }
    let mut server = TransferServer {
        child,
        base: String::new(),
        logs,
    };
    server.base =
        base.unwrap_or_else(|| panic!("server did not start: {:?}", server.logs.lock().unwrap()));
    server
}

/// Runs a server that must refuse to start and returns its output.
fn refused_startup(mut command: std::process::Command) -> String {
    let mut child = command.spawn().expect("spawn fvoci-server");
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while child.try_wait().unwrap().is_none() {
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let output = child.wait_with_output().unwrap();
            panic!(
                "server kept running: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

struct Api {
    base: String,
    cookie: String,
}

impl Api {
    async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> (u16, Value, reqwest::header::HeaderMap) {
        let mut req = browser()
            .request(method, format!("{}{path}", self.base))
            .header("cookie", format!("fvoci_session={}", self.cookie));
        if let Some(body) = body {
            req = req.json(&body);
        }
        let res = req.send().await.unwrap();
        let status = res.status().as_u16();
        let headers = res.headers().clone();
        let body = res.json::<Value>().await.unwrap_or(Value::Null);
        (status, body, headers)
    }

    async fn transfer(&self) -> Value {
        let (status, body, _) = self
            .call(
                reqwest::Method::GET,
                "/api/v1/admin/instance-settings",
                None,
            )
            .await;
        assert_eq!(status, 200, "{body:?}");
        body
    }
}

#[tokio::test]
async fn transfer_mode_env_lock_restart_and_startup_refusals() {
    let harness = TestDb::bootstrap().await;
    // Startup refuses every setting that could never take effect, naming it.
    for (env, needle) in [
        (
            vec![("FVOCI_ATTACHMENT_TRANSFER_MODE", "direct")],
            "FVOCI_ATTACHMENT_TRANSFER_MODE",
        ),
        (
            vec![
                ("FVOCI_ATTACHMENT_TRANSFER_MODE", "presigned"),
                ("S3_PUBLIC_ENDPOINT", ""),
            ],
            "requires S3_PUBLIC_ENDPOINT",
        ),
        (
            vec![
                ("FVOCI_ATTACHMENT_TRANSFER_MODE", "presigned"),
                ("STORAGE_DRIVER", "local"),
                ("FVOCI_STORAGE_DIR", "/nonexistent-fvoci-storage"),
            ],
            "requires STORAGE_DRIVER=s3",
        ),
        (
            vec![("FVOCI_PUBLIC_ORIGIN", "http://localhost:0")],
            "S3_PUBLIC_ENDPOINT must use a host other than",
        ),
        (
            vec![("FVOCI_PUBLIC_ORIGIN", "https://127.0.0.1:0")],
            "S3_PUBLIC_ENDPOINT must be https",
        ),
        (
            vec![("FVOCI_ATTACHMENT_PRESIGN_PART_TTL_SECS", "3601")],
            "FVOCI_ATTACHMENT_PRESIGN_PART_TTL_SECS",
        ),
        (
            vec![("FVOCI_ATTACHMENT_PRESIGN_DOWNLOAD_TTL_SECS", "4")],
            "FVOCI_ATTACHMENT_PRESIGN_DOWNLOAD_TTL_SECS",
        ),
    ] {
        let output = refused_startup(transfer_server_command(&harness, &env));
        assert!(output.contains(needle), "{env:?}: {output}");
    }

    let server = spawn_transfer_server(transfer_server_command(&harness, &[]));
    let setup = browser()
        .post(format!("{}/api/v1/setup", server.base))
        .json(&json!({
            "email": "s3owner@example.com", "password": "supersecret1", "givenName": "Owner",
            "workspaceSlug": "s3ws", "workspaceName": "S3"
        }))
        .send()
        .await
        .unwrap();
    assert!(setup.status().is_success(), "{}", setup.status());
    let mut cookie_headers = HeaderMap::new();
    for value in setup.headers().get_all("set-cookie") {
        cookie_headers.append("set-cookie", value.to_str().unwrap().parse().unwrap());
    }
    let cookie = extract_session_cookie(&cookie_headers);
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let workspace_id: Uuid =
        sqlx::query_scalar("SELECT id FROM fvoci.workspaces WHERE slug = 's3ws'")
            .fetch_one(&admin)
            .await
            .unwrap();
    admin.close().await;
    let api = Api {
        base: server.base.clone(),
        cookie: cookie.clone(),
    };
    let (status, doc, _) = api
        .call(
            reqwest::Method::POST,
            &format!("/api/v1/workspaces/{workspace_id}/documents"),
            Some(json!({"parentId": null, "title": "Doc"})),
        )
        .await;
    assert_eq!(status, 201, "{doc:?}");
    let document_id = doc["id"].as_str().unwrap().to_string();
    let uploads = format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/uploads");

    // Admin chooses presigned; a whole upload and download goes through MinIO.
    let (status, body, _) = api
        .call(
            reqwest::Method::PATCH,
            "/api/v1/admin/instance-settings",
            Some(json!({"attachmentTransfer": {"mode": "presigned"}})),
        )
        .await;
    assert_eq!(status, 200, "{body:?}");
    assert_eq!(body["attachmentTransfer"]["effective"], "presigned");
    let payload = patterned(4096, 17);
    let (status, created, _) = api
        .call(
            reqwest::Method::POST,
            &uploads,
            Some(json!({"name": "log.bin", "sizeBytes": payload.len()})),
        )
        .await;
    assert_eq!(status, 201, "{created:?}");
    assert_eq!(created["transfer"], "presigned");
    let id = created["attachmentId"].as_str().unwrap().to_string();
    let put = browser()
        .put(created["parts"][0]["url"].as_str().unwrap())
        .body(payload.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(put.status().as_u16(), 200);
    let etag = put.headers()["etag"].to_str().unwrap().to_string();
    let (status, body, _) = api
        .call(
            reqwest::Method::POST,
            &format!("/api/v1/workspaces/{workspace_id}/attachments/{id}/complete"),
            Some(json!({"parts": [{"partNumber": 1, "etag": etag}]})),
        )
        .await;
    assert_eq!(status, 200, "{body:?}");
    let (status, _, headers) = api
        .call(
            reqwest::Method::GET,
            &format!("/api/v1/workspaces/{workspace_id}/attachments/{id}/download"),
            None,
        )
        .await;
    assert_eq!(status, 302);
    let location = headers["location"].to_str().unwrap().to_string();
    // The storage origin is allowed by the page CSP.
    let csp = headers["content-security-policy"]
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        csp.contains(&format!("connect-src 'self' {};", public_endpoint())),
        "{csp}"
    );
    let got = browser().get(&location).send().await.unwrap();
    assert_eq!(got.bytes().await.unwrap().to_vec(), payload);
    let mut logs = server.logs.lock().unwrap().clone();
    drop(server);

    // Restarted with the variable set: the environment wins over the stored
    // row, and admin writes cannot change the locked leaf.
    let server = spawn_transfer_server(transfer_server_command(
        &harness,
        &[("FVOCI_ATTACHMENT_TRANSFER_MODE", "proxy")],
    ));
    let api = Api {
        base: server.base.clone(),
        cookie: cookie.clone(),
    };
    let body = api.transfer().await;
    assert!(body["envApplied"]
        .as_array()
        .unwrap()
        .contains(&json!("attachmentTransfer.mode")));
    assert_eq!(body["values"]["attachmentTransfer"]["mode"], "proxy");
    assert_eq!(body["attachmentTransfer"]["effective"], "proxy");
    assert_eq!(body["attachmentTransfer"]["source"], "env");
    let (status, body, _) = api
        .call(
            reqwest::Method::PATCH,
            "/api/v1/admin/instance-settings",
            Some(json!({"attachmentTransfer": {"mode": "presigned"}})),
        )
        .await;
    assert_eq!(status, 200, "{body:?}");
    assert_eq!(body["attachmentTransfer"]["effective"], "proxy");
    let (status, created, _) = api
        .call(
            reqwest::Method::POST,
            &uploads,
            Some(json!({"name": "env.bin", "sizeBytes": 10})),
        )
        .await;
    assert_eq!(status, 201);
    assert_eq!(created["transfer"], "proxy");
    logs.extend(server.logs.lock().unwrap().clone());
    drop(server);

    // Without the variable the admin's stored value is back after restart.
    let server = spawn_transfer_server(transfer_server_command(&harness, &[]));
    let api = Api {
        base: server.base.clone(),
        cookie: cookie.clone(),
    };
    let body = api.transfer().await;
    assert_eq!(body["attachmentTransfer"]["effective"], "presigned");
    assert_eq!(body["attachmentTransfer"]["source"], "stored");
    logs.extend(server.logs.lock().unwrap().clone());
    drop(server);

    // Restarted without the public endpoint the stored value cannot apply:
    // startup warns, the admin sees why, uploads use the proxy.
    let server = spawn_transfer_server(transfer_server_command(
        &harness,
        &[("S3_PUBLIC_ENDPOINT", "")],
    ));
    let api = Api {
        base: server.base.clone(),
        cookie: cookie.clone(),
    };
    let body = api.transfer().await;
    assert_eq!(
        body["attachmentTransfer"],
        json!({"effective": "proxy", "source": "stored", "presignedAvailable": false,
               "unavailableReason": "public_endpoint_missing", "blocked": true})
    );
    let (status, created, headers) = api
        .call(
            reqwest::Method::POST,
            &uploads,
            Some(json!({"name": "blocked.bin", "sizeBytes": 10})),
        )
        .await;
    assert_eq!(status, 201);
    assert_eq!(created["transfer"], "proxy");
    let csp = headers["content-security-policy"].to_str().unwrap();
    assert!(csp.contains("connect-src 'self';"), "{csp}");
    let server_logs = server.logs.lock().unwrap().clone();
    assert!(
        server_logs
            .iter()
            .any(|l| l.contains("attachment.transfer_mode_unavailable")
                && l.contains("public_endpoint_missing")),
        "{server_logs:?}"
    );
    logs.extend(server_logs);
    drop(server);

    // No signed URL, signature or credential reached any log line.
    let s3 = s3_settings();
    for line in &logs {
        for secret in [
            "X-Amz-Signature",
            "X-Amz-Credential",
            "x-amz-signature",
            s3.access_key_id.as_str(),
            s3.secret_access_key.as_str(),
        ] {
            assert!(!line.contains(secret), "log leaks {secret}: {line}");
        }
    }
    assert!(
        logs.len() > 20,
        "the servers logged at debug: {}",
        logs.len()
    );
    harness.cleanup().await;
}
