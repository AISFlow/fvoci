#![cfg(feature = "db-tests")]

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
        mailer: Arc::new(fvoci_server::mail::Mailer::disabled()),
        document_convert: None,
        import_wake: None,
        import_extractor_available: false,
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
