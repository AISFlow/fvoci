#![cfg(feature = "db-tests")]
#![allow(dead_code)]

#[path = "support/project_harness.rs"]
mod project_harness;

use std::io::Write;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use fvoci_server::auth::password::Keyring;
use fvoci_server::auth::AuthService;
use fvoci_server::db::{pool, Db};
use fvoci_server::documents::convert::ConvertClient;
use fvoci_server::http::rate_limit::RateLimiter;
use fvoci_server::http::state::AppState;
use fvoci_server::import_job::{spawn_import_job, ImportJobHandle, ImportJobSettings, ImportQueue};
use project_harness::{json_request, setup_session, TestDb};
use serde_json::json;
use tower::ServiceExt;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

const PEPPER: &str =
    r#"{"test":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#;

fn convert_client() -> ConvertClient {
    ConvertClient::from_env().expect(
        "FVOCI_DOCUMENT_CONVERT_BIN is required; run scripts/prepare-document-convert.sh first",
    )
}

fn markdown_zip_bytes() -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut zip = ZipWriter::new(std::io::Cursor::new(&mut buf));
        let options = SimpleFileOptions::default();
        zip.start_file("notes/hello.md", options)
            .expect("zip entry");
        zip.write_all(b"# Imported note\n\nFrom zip.")
            .expect("zip write");
        zip.finish().expect("zip finish");
    }
    buf
}

async fn import_export_state(app_url: &str) -> (AppState, ImportJobHandle) {
    let convert = convert_client();
    let import_settings = ImportJobSettings::from_env(convert.clone());
    let import_queue = ImportQueue::new();
    let pool = pool::connect_app(app_url).await.expect("app pool");
    let import_job = spawn_import_job(pool.clone(), import_settings.clone(), import_queue.clone());
    let storage_root =
        std::env::temp_dir().join(format!("fvoci-import-export-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&storage_root).expect("storage root");
    let state = AppState {
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
        meili: None,
        document_convert: Some(convert),
        import_settings: Some(import_settings),
        import_queue,
    };
    (state, import_job)
}

async fn setup_import_export(harness: &TestDb) -> (axum::Router, String, uuid::Uuid) {
    let (state, _import_job) = import_export_state(&harness.app_url).await;
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
    let cookie_hdr = response
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .expect("set-cookie")
        .to_string();
    let cookie = cookie_hdr
        .split(';')
        .next()
        .unwrap_or("")
        .split('=')
        .nth(1)
        .unwrap_or("")
        .to_string();
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let ws: (uuid::Uuid,) = sqlx::query_as("SELECT id FROM fvoci.workspaces WHERE slug = 'acme'")
        .fetch_one(&admin)
        .await
        .unwrap();
    admin.close().await;
    (app, cookie, ws.0)
}

async fn raw_get(
    app: axum::Router,
    path: &str,
    cookie: Option<&str>,
    auth: Option<&str>,
) -> (StatusCode, Vec<u8>, axum::http::HeaderMap) {
    let mut builder = Request::builder()
        .method("GET")
        .uri(path)
        .header("origin", "http://localhost");
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", format!("fvoci_session={}", cookie));
    }
    if let Some(auth) = auth {
        builder = builder.header("authorization", auth);
    }
    let mut request = builder.body(Body::empty()).unwrap();
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

#[tokio::test]
async fn markdown_zip_import_creates_documents_with_body() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, workspace_id) = setup_import_export(&harness).await;
    let zip_b64 = B64.encode(markdown_zip_bytes());

    let (status, body) = json_request(
        app.clone(),
        "POST",
        "/api/v1/import",
        Some(json!({
            "workspaceId": workspace_id,
            "source": "markdown-zip",
            "zipBase64": zip_b64
        })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "import failed: {body}");
    assert_eq!(body["status"], "completed");
    let doc_ids = body["createdDocumentIds"].as_array().expect("doc ids");
    assert_eq!(doc_ids.len(), 1);
    let doc_id = doc_ids[0].as_str().unwrap();

    let (status, body) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{doc_id}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["title"], "hello");

    let (status, body) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{doc_id}/body"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let content = body["contentJson"]["content"].as_array().expect("content");
    assert!(!content.is_empty());

    harness.cleanup().await;
}

#[tokio::test]
async fn export_markdown_returns_attachment_bytes() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, workspace_id) = setup_import_export(&harness).await;
    let zip_b64 = B64.encode(markdown_zip_bytes());
    let (status, import_body) = json_request(
        app.clone(),
        "POST",
        "/api/v1/import",
        Some(json!({
            "workspaceId": workspace_id,
            "source": "markdown-zip",
            "zipBase64": zip_b64
        })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let doc_id = import_body["createdDocumentIds"][0].as_str().unwrap();

    let (status, bytes, headers) = raw_get(
        app.clone(),
        &format!("/api/v1/workspaces/{workspace_id}/documents/{doc_id}/md"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let content_type = headers.get("content-type").and_then(|v| v.to_str().ok());
    assert!(
        content_type.is_some_and(|v| v.starts_with("text/markdown")),
        "unexpected content-type: {content_type:?}"
    );
    let text = String::from_utf8(bytes).expect("utf8");
    assert!(text.contains("Imported note"));

    harness.cleanup().await;
}

#[tokio::test]
async fn import_rejects_bearer_token() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let (status, created) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/api-tokens"),
        Some(json!({
            "name": "import",
            "scopes": ["documents.read"]
        })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let secret = created["token"].as_str().unwrap();

    let (status, body, _) = project_harness::http_request(
        app,
        "POST",
        "/api/v1/import",
        Some(
            json!({
                "workspaceId": workspace_id,
                "source": "markdown-zip",
                "zipBase64": B64.encode(markdown_zip_bytes())
            })
            .to_string()
            .into_bytes(),
        ),
        Some("application/json"),
        None,
        &[("authorization", &format!("Bearer {secret}"))],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");

    harness.cleanup().await;
}
