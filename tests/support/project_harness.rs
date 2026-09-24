use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
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

pub fn test_peer() -> std::net::SocketAddr {
    std::net::SocketAddr::from(([203, 0, 113, 10], 42424))
}

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
        let db_name = format!("fvoci_test_{}", Uuid::now_v7().simple());
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

async fn app_state(app_url: &str) -> AppState {
    let pool = pool::connect_app(app_url).await.expect("app pool");
    let storage_root = std::env::temp_dir().join(format!("fvoci-proj-test-{}", Uuid::now_v7()));
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

pub async fn json_request(
    app: axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    cookie: Option<&str>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("origin", "http://localhost");
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", format!("fvoci_session={}", cookie));
    }
    let request = if let Some(body) = body {
        builder
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    } else {
        builder.body(Body::empty()).unwrap()
    };
    let mut request = request;
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(test_peer()));
    let response = app.oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap_or_default();
    let json = if bytes.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&bytes).unwrap_or(json!({}))
    };
    (status, json)
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

pub async fn setup_session(harness: &TestDb) -> (axum::Router, String, Uuid, Uuid) {
    let app = app_router(app_state(&harness.app_url).await);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/setup")
                .header("content-type", "application/json")
                .header("origin", "http://localhost")
                .extension(axum::extract::ConnectInfo(test_peer()))
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
    let cookie = extract_session_cookie(&cookie_hdr);
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let user_id: (Uuid,) = sqlx::query_as("SELECT id FROM fvoci.users LIMIT 1")
        .fetch_one(&admin)
        .await
        .unwrap();
    let workspace_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM fvoci.workspaces WHERE slug = 'acme'")
            .fetch_one(&admin)
            .await
            .unwrap();
    admin.close().await;
    (app, cookie, user_id.0, workspace_id.0)
}

pub async fn admin_pool(harness: &TestDb) -> PgPool {
    PgPoolOptions::new()
        .max_connections(5)
        .connect(&harness.admin_url)
        .await
        .expect("admin pool")
}

pub struct TestUser {
    pub user_id: Uuid,
    pub cookie: String,
}

pub async fn add_workspace_user(
    admin: &PgPool,
    workspace_id: Uuid,
    role: &str,
    label: &str,
) -> TestUser {
    let user_id = Uuid::now_v7();
    let email = format!("{label}-{user_id}@example.com");
    sqlx::query("INSERT INTO fvoci.users (id, email, given_name) VALUES ($1, $2, $3)")
        .bind(user_id)
        .bind(&email)
        .bind(label)
        .execute(admin)
        .await
        .expect("insert user");
    sqlx::query("INSERT INTO fvoci.memberships (workspace_id, user_id, role) VALUES ($1, $2, $3)")
        .bind(workspace_id)
        .bind(user_id)
        .bind(role)
        .execute(admin)
        .await
        .expect("insert membership");
    let token = fvoci_server::auth::token::new_token();
    sqlx::query(
        "INSERT INTO fvoci.sessions (id, user_id, token_hash, expires_at) VALUES ($1, $2, $3, now() + interval '1 hour')",
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(&token.hash)
    .execute(admin)
    .await
    .expect("insert session");
    TestUser {
        user_id,
        cookie: token.token,
    }
}

pub async fn insert_minimal_project(
    admin: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    key: &str,
    created_by: Uuid,
    visibility: &str,
) {
    sqlx::query(
        r#"
        INSERT INTO fvoci.projects (
            id, workspace_id, key, name, visibility, status, next_number, created_by
        ) VALUES ($1, $2, $3, $4, $5, 'active', 1, $6)
        "#,
    )
    .bind(project_id)
    .bind(workspace_id)
    .bind(key)
    .bind(key)
    .bind(visibility)
    .bind(created_by)
    .execute(admin)
    .await
    .expect("insert project");
}

pub async fn insert_project_document(
    admin: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    document_id: Uuid,
    created_by: Uuid,
    number: i32,
) {
    sqlx::query(
        r#"
        INSERT INTO fvoci.documents (
            id, workspace_id, title, path, parent_id, sort_key, project_id, number, status,
            schema_version, content_json, created_by
        ) VALUES (
            $1, $2, 'Project doc', $3, NULL, 'V', $4, $5, 'published', 2, '{"type":"doc","content":[{"type":"paragraph"}]}'::jsonb, $6
        )
        "#,
    )
    .bind(document_id)
    .bind(workspace_id)
    .bind(document_id.simple().to_string())
    .bind(project_id)
    .bind(number)
    .bind(created_by)
    .execute(admin)
    .await
    .expect("insert project document");
}

pub async fn count_rows(admin: &PgPool, table: &str) -> i64 {
    let sql = format!("SELECT count(*) FROM fvoci.{table}");
    sqlx::query_scalar::<_, i64>(&sql)
        .fetch_one(admin)
        .await
        .unwrap_or(0)
}

pub async fn install_insert_fail_trigger(admin: &PgPool, target: &str, fn_name: &str) {
    sqlx::query(&format!(
        r#"
        CREATE OR REPLACE FUNCTION fvoci.{fn_name}()
        RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN
            RAISE EXCEPTION 'insert blocked on {target}';
        END;
        $$;
        "#
    ))
    .execute(admin)
    .await
    .unwrap();
    sqlx::query(&format!(
        r#"
        CREATE TRIGGER fvoci_{fn_name}
        BEFORE INSERT ON fvoci.{target}
        FOR EACH ROW EXECUTE FUNCTION fvoci.{fn_name}()
        "#
    ))
    .execute(admin)
    .await
    .unwrap();
}

pub async fn drop_insert_fail_trigger(admin: &PgPool, target: &str, fn_name: &str) {
    let _ = sqlx::query(&format!(
        "DROP TRIGGER IF EXISTS fvoci_{fn_name} ON fvoci.{target}"
    ))
    .execute(admin)
    .await;
    let _ = sqlx::query(&format!("DROP FUNCTION IF EXISTS fvoci.{fn_name}()"))
        .execute(admin)
        .await;
}

pub async fn wait_for_user_for_update_blocked(admin: &PgPool, blocker_pid: i32) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        let blocked: Option<i32> = sqlx::query_scalar(
            r#"
            SELECT activity.pid
            FROM pg_stat_activity AS activity
            WHERE activity.wait_event_type = 'Lock'
              AND activity.state = 'active'
              AND activity.query ILIKE '%fvoci.users%'
              AND activity.query ILIKE '%FOR UPDATE%'
              AND $1 = ANY(pg_blocking_pids(activity.pid))
            LIMIT 1
            "#,
        )
        .bind(blocker_pid)
        .fetch_optional(admin)
        .await
        .unwrap();
        if blocked.is_some() {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("expected FOR UPDATE block on users row");
}

pub async fn wait_for_query_blocked_by(admin: &PgPool, blocker_pid: i32, query_like: &str) -> i32 {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        let blocked: Option<i32> = sqlx::query_scalar(
            r#"
            SELECT activity.pid
            FROM pg_stat_activity AS activity
            WHERE activity.wait_event_type = 'Lock'
              AND activity.state = 'active'
              AND activity.query ILIKE $2
              AND $1 = ANY(pg_blocking_pids(activity.pid))
            LIMIT 1
            "#,
        )
        .bind(blocker_pid)
        .bind(query_like)
        .fetch_optional(admin)
        .await
        .unwrap();
        if let Some(pid) = blocked {
            return pid;
        }
        tokio::task::yield_now().await;
    }
    panic!("expected blocked query matching {query_like}");
}

pub async fn wait_for_advisory_blocked_by(admin: &PgPool, blocker_pid: i32) -> i32 {
    wait_for_query_blocked_by(admin, blocker_pid, "%pg_advisory_xact_lock%").await
}

pub async fn create_project(
    app: axum::Router,
    cookie: &str,
    workspace_id: Uuid,
    key: &str,
    visibility: &str,
) -> Value {
    let (status, body) = json_request(
        app,
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects"),
        Some(json!({"key": key, "name": key, "visibility": visibility})),
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body:?}");
    body
}

pub async fn app_pool(harness: &TestDb) -> PgPool {
    pool::connect_app(&harness.app_url).await.expect("app pool")
}
