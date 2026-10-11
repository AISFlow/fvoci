use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use fvoci_server::auth::password::Keyring;
use fvoci_server::auth::AuthService;
use fvoci_server::db::{context, pool, Db};
use fvoci_server::http::rate_limit::RateLimiter;
use fvoci_server::http::state::AppState;
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

#[path = "test_db.rs"]
mod test_db;

pub use test_db::TestDb;

impl TestDb {
    pub async fn bootstrap() -> Self {
        Self::create("fvoci_test_").await
    }

    pub async fn cleanup(self) {
        let _ = self.drop_owned().await;
    }
}

pub async fn app_state(app_url: &str) -> AppState {
    let pool = pool::connect_app(app_url).await.expect("app pool");
    let storage_root = std::env::temp_dir().join(format!("fvoci-proj-test-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&storage_root).expect("storage root");
    AppState {
        realtime_mode: fvoci_server::config::RealtimeMode::On,
        native_engine: None,
        auth: Arc::new(AuthService {
            db: Db::new(pool),
            password_keys: Keyring::parse(PEPPER, "test").expect("pepper"),
        }),
        branding_name: "FVOCI".to_string(),
        public_origin: "http://localhost".to_string(),
        cookie_secure: false,
        rate_limiter: RateLimiter::new(),
        storage: fvoci_server::attachments::LocalStorage::new(storage_root).into(),
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
        search_embedder: None,
        markdown: Some(
            fvoci_server::documents::markdown_helper::MarkdownHelper::new(env!(
                "CARGO_BIN_EXE_fvoci-server"
            )),
        ),
        import_wake: None,
        import_extractor_available: false,
        preview_extract: None,
        quota: Default::default(),
        streams: fvoci_server::http::state::AppState::fresh_streams(),
        mailer: std::sync::Arc::new(fvoci_server::mail::Mailer::disabled()),
    }
}

fn app_router(state: AppState) -> axum::Router {
    fvoci_server::http::router(state, None)
}

pub async fn http_request(
    app: axum::Router,
    method: &str,
    path: &str,
    body: Option<Vec<u8>>,
    content_type: Option<&str>,
    cookie: Option<&str>,
    extra_headers: &[(&str, &str)],
) -> (StatusCode, Value, axum::http::HeaderMap) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("origin", "http://localhost");
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", format!("fvoci_session={}", cookie));
    }
    for (name, value) in extra_headers {
        builder = builder.header(*name, *value);
    }
    let request = if let Some(body) = body {
        let mut builder = builder;
        if let Some(content_type) = content_type {
            builder = builder.header("content-type", content_type);
        }
        builder.body(Body::from(body)).unwrap()
    } else {
        builder.body(Body::empty()).unwrap()
    };
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
    let json = if bytes.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&bytes).unwrap_or(json!({}))
    };
    (status, json, headers)
}

pub async fn json_request(
    app: axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    cookie: Option<&str>,
) -> (StatusCode, Value) {
    let (status, json, _) = json_request_with_headers(app, method, path, body, cookie).await;
    (status, json)
}

pub async fn json_request_with_headers(
    app: axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    cookie: Option<&str>,
) -> (StatusCode, Value, axum::http::HeaderMap) {
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
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap_or_default();
    let json = if bytes.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&bytes).unwrap_or(json!({}))
    };
    (status, json, headers)
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
    setup_session_with(harness, |_| {}).await
}

/// `setup_session` with another `--internal-markdown` child (`None`: unavailable).
pub async fn setup_session_with_markdown(
    harness: &TestDb,
    markdown: Option<fvoci_server::documents::markdown_helper::MarkdownHelper>,
) -> (axum::Router, String, Uuid, Uuid) {
    setup_session_with(harness, |state| state.markdown = markdown).await
}

async fn setup_session_with(
    harness: &TestDb,
    configure: impl FnOnce(&mut AppState),
) -> (axum::Router, String, Uuid, Uuid) {
    let mut state = app_state(&harness.app_url).await;
    configure(&mut state);
    let app = app_router(state);
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

/// Close `pool` and wait (bounded) until none of its connections is open.
/// sqlx 0.8.6 `Pool::close` can return while a connection is still being
/// returned (its on-release ping): closing an idle connection releases an
/// extra semaphore permit, so close's wait for all permits passes early. That
/// connection then goes idle in the closed pool and stays open while any clone
/// of the pool lives; each further `close` sweeps whatever went idle since.
/// Guarded by `notification_integration.rs`
/// `close_pool_closes_a_connection_returned_during_close`; go back to a single
/// `pool.close()` once that test passes with `close_pool` replaced by it.
pub async fn close_pool(pool: PgPool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        pool.close().await;
        if pool.size() == 0 {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{} pool connections still open after close",
            pool.size()
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
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
            WHERE activity.datname = current_database()
              AND activity.wait_event_type = 'Lock'
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
    wait_for_blocked_query_count(admin, blocker_pid, query_like, 1)
        .await
        .into_iter()
        .next()
        .expect("expected one blocked query")
}

pub async fn wait_for_blocked_query_count(
    admin: &PgPool,
    blocker_pid: i32,
    query_like: &str,
    expected: usize,
) -> Vec<i32> {
    wait_for_blocked_by_holder(admin, blocker_pid, Some(query_like), expected).await
}

pub async fn wait_for_blocked_by_holder(
    admin: &PgPool,
    blocker_pid: i32,
    query_like: Option<&str>,
    expected: usize,
) -> Vec<i32> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        let blocked: Vec<i32> = if let Some(query_like) = query_like {
            sqlx::query_scalar(
                r#"
                SELECT activity.pid
                FROM pg_stat_activity AS activity
                WHERE activity.datname = current_database()
                  AND activity.wait_event_type = 'Lock'
                  AND activity.state = 'active'
                  AND activity.query ILIKE $2
                  AND $1 = ANY(pg_blocking_pids(activity.pid))
                ORDER BY activity.pid
                "#,
            )
            .bind(blocker_pid)
            .bind(query_like)
            .fetch_all(admin)
            .await
            .unwrap()
        } else {
            sqlx::query_scalar(
                r#"
                SELECT activity.pid
                FROM pg_stat_activity AS activity
                WHERE activity.datname = current_database()
                  AND activity.wait_event_type = 'Lock'
                  AND activity.state = 'active'
                  AND $1 = ANY(pg_blocking_pids(activity.pid))
                ORDER BY activity.pid
                "#,
            )
            .bind(blocker_pid)
            .fetch_all(admin)
            .await
            .unwrap()
        };
        if blocked.len() >= expected {
            return blocked;
        }
        tokio::task::yield_now().await;
    }
    let detail = query_like.unwrap_or("any query");
    panic!("expected at least {expected} blocked queries matching {detail}");
}

pub async fn wait_for_active_query_count(admin: &PgPool, query_like: &str, expected: usize) {
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(admin)
        .await
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        let active: i64 = sqlx::query_scalar(
            r#"
            SELECT count(*)
            FROM pg_stat_activity AS activity
            WHERE activity.datname = current_database()
              AND activity.pid <> $1
              AND activity.state = 'active'
              AND activity.query ILIKE $2
            "#,
        )
        .bind(blocker_pid)
        .bind(query_like)
        .fetch_one(admin)
        .await
        .unwrap();
        if active >= expected as i64 {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("expected at least {expected} active queries matching {query_like}");
}

pub async fn wait_for_project_lock_waiters(admin: &PgPool, blocker_pid: i32, expected: usize) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        let waiting: i64 = sqlx::query_scalar(
            r#"
            SELECT count(*)
            FROM pg_stat_activity AS activity
            WHERE activity.datname = current_database()
              AND activity.wait_event_type = 'Lock'
              AND activity.state = 'active'
              AND activity.query ILIKE '%fvoci.projects%'
              AND $1 = ANY(pg_blocking_pids(activity.pid))
            "#,
        )
        .bind(blocker_pid)
        .fetch_one(admin)
        .await
        .unwrap();
        if waiting >= expected as i64 {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("expected at least {expected} active project-lock waiters blocked by pid {blocker_pid}");
}

pub async fn wait_for_advisory_blocked_by(admin: &PgPool, blocker_pid: i32) -> i32 {
    wait_for_query_blocked_by(admin, blocker_pid, "%pg_advisory_xact_lock%").await
}

pub async fn hold_membership_user_lock(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
) {
    sqlx::query("SELECT pg_advisory_xact_lock($1, $2)")
        .bind(context::MEMBERSHIP_LOCK_NAMESPACE)
        .bind(context::lock_key_from_uuid(user_id))
        .execute(&mut **tx)
        .await
        .unwrap();
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

pub async fn session_id_for_user(admin: &PgPool, user_id: Uuid) -> Uuid {
    sqlx::query_scalar(
        "SELECT id FROM fvoci.sessions WHERE user_id = $1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(user_id)
    .fetch_one(admin)
    .await
    .expect("session id")
}

pub async fn insert_stored_attachment(
    admin: &PgPool,
    workspace_id: Uuid,
    document_id: Uuid,
    uploader_id: Uuid,
) -> Uuid {
    let attachment_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO fvoci.attachments (
            id, workspace_id, document_id, uploader_id, status, name, reserved_size_bytes,
            size_bytes, storage_key, completed_at
        ) VALUES ($1, $2, $3, $4, 'stored', 'probe.bin', 4, 4, $5, now())
        "#,
    )
    .bind(attachment_id)
    .bind(workspace_id)
    .bind(document_id)
    .bind(uploader_id)
    .bind(Uuid::now_v7().to_string())
    .execute(admin)
    .await
    .expect("insert attachment");
    attachment_id
}
