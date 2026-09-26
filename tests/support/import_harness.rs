//! Shared fixture of the import suites: an app router with a signed-in
//! workspace owner, the app-role pool, local storage and an import runner
//! that tests drive deterministically.
#![allow(dead_code)]

use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use fvoci_server::attachments::ObjectStorage;
use fvoci_server::auth::password::Keyring;
use fvoci_server::auth::AuthService;
use fvoci_server::db::{pool, Db};
use fvoci_server::documents::convert::ConvertClient;
use fvoci_server::http::rate_limit::RateLimiter;
use fvoci_server::http::state::AppState;
use fvoci_server::import_job::{
    run_next_import, spawn_import_job, ImportJobHandle, ImportJobSettings,
};
use serde_json::{json, Value};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;
use uuid::Uuid;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use crate::project_harness::{self, admin_pool, json_request, TestDb};

pub const PEPPER: &str =
    r#"{"test":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#;

pub fn convert_client() -> ConvertClient {
    ConvertClient::from_env().expect(
        "FVOCI_DOCUMENT_CONVERT_BIN is required; run scripts/prepare-document-convert.sh first",
    )
}

pub fn zip_bytes(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut zip = ZipWriter::new(std::io::Cursor::new(&mut buf));
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        for (name, data) in files {
            zip.start_file(*name, options).expect("zip entry");
            zip.write_all(data).expect("zip write");
        }
        zip.finish().expect("zip finish");
    }
    buf
}

pub fn markdown_zip_bytes() -> Vec<u8> {
    zip_bytes(&[("notes/hello.md", b"# Imported note\n\nFrom zip.")])
}

/// App plus the pieces needed to drive the async runner deterministically.
pub struct Fixture {
    pub app: axum::Router,
    pub cookie: String,
    pub user_id: Uuid,
    pub workspace_id: Uuid,
    pub pool: PgPool,
    pub admin: PgPool,
    pub settings: ImportJobSettings,
    pub storage: ObjectStorage,
    pub runner: Option<ImportJobHandle>,
    pub storage_root_dir: std::path::PathBuf,
}

impl Fixture {
    pub fn storage_root(&self) -> std::path::PathBuf {
        self.storage_root_dir.clone()
    }

    pub async fn stop_runner(&mut self) {
        if let Some(runner) = self.runner.take() {
            runner.request_shutdown();
            runner.join().await.expect("runner join");
        }
    }

    pub async fn run_next(&self) -> bool {
        run_next_import(
            &self.pool,
            &self.settings,
            &self.storage,
            &CancellationToken::new(),
        )
        .await
        .expect("run next import")
    }

    pub async fn job_status(&self, cookie: &str, job_id: &str) -> Value {
        let (status, body) = json_request(
            self.app.clone(),
            "GET",
            &format!("/api/v1/import/{job_id}?workspaceId={}", self.workspace_id),
            None,
            Some(cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body
    }

    pub async fn import(&self, cookie: &str, body: Value) -> (StatusCode, Value) {
        json_request(
            self.app.clone(),
            "POST",
            "/api/v1/import",
            Some(body),
            Some(cookie),
        )
        .await
    }

    pub async fn document_count(&self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM fvoci.documents WHERE workspace_id = $1")
            .bind(self.workspace_id)
            .fetch_one(&self.admin)
            .await
            .unwrap()
    }
}

pub async fn fixture(harness: &TestDb) -> Fixture {
    fixture_with_runner(harness, false).await
}

pub async fn fixture_with_runner(harness: &TestDb, spawn_runner: bool) -> Fixture {
    let convert = convert_client();
    let settings = ImportJobSettings {
        poll_interval: Duration::from_millis(200),
        // The office child is the server binary, not this test binary.
        office_helper: Some(std::path::PathBuf::from(env!("CARGO_BIN_EXE_fvoci-server"))),
        ..ImportJobSettings::from_env(convert.clone())
    };
    let pool = pool::connect_app(&harness.app_url).await.expect("app pool");
    let storage_root = std::env::temp_dir().join(format!("fvoci-import-export-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&storage_root).expect("storage root");
    let storage = ObjectStorage::local(storage_root.clone());
    let runner =
        spawn_runner.then(|| spawn_import_job(pool.clone(), settings.clone(), storage.clone()));
    let wake = runner
        .as_ref()
        .map(|handle| handle.wake.clone())
        .unwrap_or_else(|| Arc::new(tokio::sync::Notify::new()));
    let state = AppState {
        auth: Arc::new(AuthService {
            db: Db::new(pool.clone()),
            password_keys: Keyring::parse(PEPPER, "test").expect("pepper"),
        }),
        branding_name: "FVOCI".to_string(),
        public_origin: "http://localhost".to_string(),
        cookie_secure: false,
        rate_limiter: RateLimiter::new(),
        storage: storage.clone(),
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
        mailer: Arc::new(fvoci_server::mail::Mailer::disabled()),
        document_convert: Some(convert),
        import_wake: Some(wake),
        import_extractor_available: false,
        quota: Default::default(),
    };
    let app = fvoci_server::http::router(state, None);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/setup")
                .header("content-type", "application/json")
                .header("origin", "http://localhost")
                .extension(axum::extract::ConnectInfo(project_harness::test_peer()))
                .body(Body::from(
                    json!({
                        "email": "owner@example.com",
                        "password": "supersecret1",
                        "givenName": "Owner",
                        "workspaceSlug": "acme",
                        "workspaceName": "Acme"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .expect("setup");
    let cookie = response
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .expect("set-cookie")
        .split(';')
        .next()
        .unwrap()
        .split('=')
        .nth(1)
        .unwrap()
        .to_string();
    let admin = admin_pool(harness).await;
    let (workspace_id,): (Uuid,) =
        sqlx::query_as("SELECT id FROM fvoci.workspaces WHERE slug = 'acme'")
            .fetch_one(&admin)
            .await
            .unwrap();
    let (user_id,): (Uuid,) = sqlx::query_as("SELECT id FROM fvoci.users LIMIT 1")
        .fetch_one(&admin)
        .await
        .unwrap();
    Fixture {
        app,
        cookie,
        user_id,
        workspace_id,
        pool,
        admin,
        settings,
        storage,
        runner,
        storage_root_dir: storage_root,
    }
}

pub async fn raw_request(
    app: axum::Router,
    method: &str,
    path: &str,
    cookie: Option<&str>,
    content_type: Option<&str>,
    body: Body,
) -> (StatusCode, Vec<u8>, axum::http::HeaderMap) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("origin", "http://localhost");
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", format!("fvoci_session={cookie}"));
    }
    if let Some(content_type) = content_type {
        builder = builder.header("content-type", content_type);
    }
    let mut request = builder.body(body).unwrap();
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(project_harness::test_peer()));
    let response = app.oneshot(request).await.expect("response");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap_or_default()
        .to_vec();
    (status, bytes, headers)
}

pub async fn job_row(admin: &PgPool, job_id: Uuid) -> (String, bool, i16, Value) {
    sqlx::query_as(
        "SELECT status, payload IS NOT NULL, attempts, created_refs FROM fvoci.import_jobs WHERE id = $1",
    )
    .bind(job_id)
    .fetch_one(admin)
    .await
    .unwrap()
}

pub async fn document_exists(admin: &PgPool, id: &str) -> bool {
    let id: Uuid = id.parse().unwrap();
    sqlx::query_scalar::<_, i64>("SELECT count(*) FROM fvoci.documents WHERE id = $1")
        .bind(id)
        .fetch_one(admin)
        .await
        .unwrap()
        == 1
}
