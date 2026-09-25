#![cfg(feature = "db-tests")]

use std::sync::Arc;
use std::time::Duration;

use axum::http::{HeaderMap, Request, StatusCode};
use bytes::Bytes;
use chrono::Utc;
use futures_util::stream;
use fvoci_server::attachments::{gc_stale_uploads, ObjectStorage, S3Storage, StorageError};
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
        },
        collab: None,
        meili: None,
        mailer: Arc::new(fvoci_server::mail::Mailer::disabled()),
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
    let app =
        app_router(app_state_with_part_size(&harness.app_url, storage, part_size_bytes).await);
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
    let message = err.to_string();
    assert!(message.contains("EntityTooSmall"), "{message}");
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
