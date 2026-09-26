use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, Command, Stdio};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::Router;
use fvoci_server::attachments::{
    spawn_extract_job, ExtractJobSettings, LocalStorage, UploadLimits,
};
use fvoci_server::auth::password::Keyring;
use fvoci_server::auth::AuthService;
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
    fvoci_server::db::migrate::apply_app_role_grants(pool, role_name)
        .await
        .expect("grant");
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
        storage: LocalStorage::new(storage_root.clone()).into(),
        upload: UploadLimits {
            part_size_bytes: fvoci_server::config::DEFAULT_UPLOAD_PART_SIZE_BYTES,
            max_file_size_bytes: fvoci_server::config::DEFAULT_UPLOAD_MAX_FILE_SIZE_BYTES,
            create_rate_per_5min: fvoci_server::config::DEFAULT_UPLOAD_CREATE_RATE_PER_5MIN,
            part_put_slots: fvoci_server::attachments::PartPutSlots::new(
                fvoci_server::config::DEFAULT_UPLOAD_MAX_CONCURRENT_PARTS,
            ),
        },
        collab: None,
        meili: None,
        document_convert: None,
        markdown: Some(
            fvoci_server::documents::markdown_helper::MarkdownHelper::new(env!(
                "CARGO_BIN_EXE_fvoci-server"
            )),
        ),
        import_wake: None,
        import_extractor_available: false,
        quota: Default::default(),
        search_embedder: None,
        mailer: std::sync::Arc::new(fvoci_server::mail::Mailer::disabled()),
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

fn extract_job_settings_with(
    extractor_bin: PathBuf,
    poll_interval: Duration,
    retry_backoff: Duration,
    test_hang_ms: Option<u64>,
) -> ExtractJobSettings {
    ExtractJobSettings {
        extractor_bin: Some(extractor_bin),
        limits: document_extract_client::Limits::for_tests(),
        office_helper: Some(server_bin()),
        office_limits: fvoci_server::documents::office::OfficeLimits::attachment(),
        poll_interval,
        retry_backoff,
        test_hang_ms,
    }
}

fn base_extract_job_settings(extractor_bin: PathBuf) -> ExtractJobSettings {
    extract_job_settings_with(
        extractor_bin,
        Duration::from_millis(100),
        Duration::from_millis(100),
        None,
    )
}

pub fn extract_job_settings(extractor_bin: PathBuf) -> ExtractJobSettings {
    base_extract_job_settings(extractor_bin)
}

pub fn idle_extract_job_settings(extractor_bin: PathBuf) -> ExtractJobSettings {
    extract_job_settings_with(
        extractor_bin,
        Duration::from_secs(600),
        Duration::from_secs(600),
        None,
    )
}

pub fn server_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fvoci-server"))
}

pub fn extract_job_driver_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_extract-job-driver"))
}

pub fn server_env_for_harness(
    harness: &TestDb,
    storage_root: &Path,
    extractor_bin: Option<&Path>,
) -> Vec<(String, String)> {
    let mut env = vec![
        ("DATABASE_APP_URL".into(), harness.app_url.clone()),
        ("PASSWORD_PEPPER_KEYS".into(), PEPPER.to_string()),
        ("PASSWORD_PEPPER_ACTIVE_KEY_ID".into(), "test".to_string()),
        ("FVOCI_BIND".into(), "127.0.0.1:0".to_string()),
        ("FVOCI_PUBLIC_ORIGIN".into(), "http://localhost".to_string()),
        ("FVOCI_COOKIE_SECURE".into(), "0".to_string()),
        (
            "FVOCI_STORAGE_DIR".into(),
            storage_root.to_string_lossy().to_string(),
        ),
        ("FVOCI_SHUTDOWN_DEADLINE_MS".into(), "5000".to_string()),
    ];
    if let Some(bin) = extractor_bin {
        env.push((
            "FVOCI_EXTRACTOR_BIN".into(),
            bin.to_string_lossy().to_string(),
        ));
    }
    env
}

pub struct TempStorageGuard {
    path: PathBuf,
}

impl TempStorageGuard {
    pub fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!("fvoci-ext-{label}-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&path).expect("storage root");
        Self { path }
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }
}

impl Drop for TempStorageGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

pub struct ServerProcessGuard {
    child: Option<Child>,
}

impl ServerProcessGuard {
    pub fn child_mut(&mut self) -> &mut Child {
        self.child.as_mut().expect("server child")
    }
}

impl Drop for ServerProcessGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn spawn_stderr_reader(stderr: ChildStderr) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let reader = BufReader::new(stderr);
        for line in reader.lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}

fn parse_listen_addr(line: &str) -> Option<String> {
    const PREFIX: &str = "fvoci-server listening on http://";
    line.strip_prefix(PREFIX).map(str::to_string)
}

pub fn wait_for_server_listen_addr(
    stderr_lines: &mpsc::Receiver<String>,
    deadline: std::time::Instant,
) -> String {
    while std::time::Instant::now() < deadline {
        while let Ok(line) = stderr_lines.try_recv() {
            if let Some(addr) = parse_listen_addr(&line) {
                return addr;
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("timed out waiting for fvoci-server listen address on stderr");
}

fn http_get(base_url: &str, path: &str) -> Result<(u16, String), String> {
    let parsed = url::Url::parse(base_url).map_err(|e| format!("invalid base url: {e}"))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| "missing host in base url".to_string())?;
    let port = parsed.port_or_known_default().unwrap_or(80);
    let mut stream =
        TcpStream::connect((host, port)).map_err(|e| format!("tcp connect failed: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|e| format!("read timeout failed: {e}"))?;
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .map_err(|e| format!("write timeout failed: {e}"))?;
    let request = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .map_err(|e| format!("write request failed: {e}"))?;
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|e| format!("read response failed: {e}"))?;
    let status = response
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    let body = response.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    Ok((status, body))
}

pub fn http_get_setup_status(base_url: &str) -> (u16, String) {
    http_get(base_url, "/api/v1/setup").expect("setup status request")
}

pub fn wait_for_server_setup_status(base_url: &str, deadline: std::time::Instant) {
    while std::time::Instant::now() < deadline {
        match http_get(base_url, "/api/v1/setup") {
            Ok((status, body)) if (200..300).contains(&status) && body.contains("branding") => {
                return;
            }
            Ok(_) | Err(_) => {}
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("timed out waiting for /api/v1/setup from {base_url}");
}

pub fn wait_for_server_ready(stderr_lines: &mpsc::Receiver<String>) -> String {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let addr = wait_for_server_listen_addr(stderr_lines, deadline);
    let base_url = format!("http://{addr}");
    wait_for_server_setup_status(&base_url, deadline);
    base_url
}

pub fn spawn_server_process_guarded(
    harness: &TestDb,
    storage_root: &Path,
    extractor_bin: Option<&Path>,
) -> (ServerProcessGuard, mpsc::Receiver<String>) {
    let mut command = Command::new(server_bin());
    // CI configures the helper for the parent test binary. Start each server
    // with only the extractor settings explicitly requested by this fixture.
    command.env_remove("FVOCI_EXTRACTOR_BIN");
    command.env_remove("FVOCI_EXTRACT_POLL_SECS");
    command.env_remove("DATABASE_URL");
    command.env_remove("FVOCI_MIGRATION_URL");
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in server_env_for_harness(harness, storage_root, extractor_bin) {
        command.env(key, value);
    }
    let mut child = command.spawn().expect("spawn fvoci-server");
    let stderr = child.stderr.take().expect("server stderr");
    let stderr_lines = spawn_stderr_reader(stderr);
    (ServerProcessGuard { child: Some(child) }, stderr_lines)
}

pub fn wait_for_server_exit(
    child: &mut Child,
    stderr_lines: &mpsc::Receiver<String>,
    deadline: std::time::Instant,
) -> (std::process::ExitStatus, String) {
    let mut stderr = String::new();
    let push_line = |stderr: &mut String, line: String| {
        if !stderr.is_empty() {
            stderr.push('\n');
        }
        stderr.push_str(&line);
    };
    while std::time::Instant::now() < deadline {
        while let Ok(line) = stderr_lines.try_recv() {
            push_line(&mut stderr, line);
        }
        if let Some(status) = child.try_wait().expect("wait") {
            // The child's last lines can still be in the pipe after it has been
            // reaped; read until the reader thread hits EOF and drops its sender.
            loop {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                match stderr_lines.recv_timeout(remaining) {
                    Ok(line) => push_line(&mut stderr, line),
                    Err(mpsc::RecvTimeoutError::Disconnected) => return (status, stderr),
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        panic!("fvoci-server stderr did not close before deadline; stderr={stderr}")
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    let _ = child.kill();
    let _ = child.wait();
    panic!("fvoci-server did not exit before deadline; stderr={stderr}");
}

pub fn run_extract_job_driver(
    harness: &TestDb,
    storage_root: &Path,
    extractor_bin: &Path,
    mode: &str,
    workspace_id: Uuid,
    attachment_id: Uuid,
) -> std::process::Output {
    Command::new(extract_job_driver_bin())
        .env("DATABASE_APP_URL", &harness.app_url)
        .env("FVOCI_STORAGE_DIR", storage_root)
        .env("FVOCI_EXTRACTOR_BIN", extractor_bin)
        .env("EXTRACT_JOB_DRIVER_MODE", mode)
        .env("EXTRACT_JOB_WORKSPACE_ID", workspace_id.to_string())
        .env("EXTRACT_JOB_ATTACHMENT_ID", attachment_id.to_string())
        .output()
        .expect("run extract-job-driver")
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
        &format!("/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/download"),
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
    storage_root: &Path,
    settings: ExtractJobSettings,
) -> fvoci_server::attachments::ExtractJobHandle {
    let pool = app_pool(&harness.app_url).await;
    spawn_extract_job(
        settings,
        pool,
        fvoci_server::attachments::ObjectStorage::local(storage_root.to_path_buf()),
    )
}
