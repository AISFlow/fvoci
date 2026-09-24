#![cfg(feature = "db-tests")]

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use chrono::{Duration as ChronoDuration, Utc};
use fvoci_server::auth::password::Keyring;
use fvoci_server::auth::token::hash_token;
use fvoci_server::auth::AuthService;
use fvoci_server::db::identity::{
    count_users, find_live_session, new_setup_input, setup_first_owner, SetupFirstOwnerResult,
    SetupSessionParams,
};
use fvoci_server::db::{migrate, pool, Db};
use fvoci_server::http::rate_limit::RateLimiter;
use fvoci_server::http::{router, state::AppState};
use rand::RngCore;
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

const PEPPER: &str =
    r#"{"test":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#;

fn test_peer() -> std::net::SocketAddr {
    std::net::SocketAddr::from(([203, 0, 113, 10], 42424))
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

        fvoci_server::db::migrate::apply_app_role_grants(&migration_pool, &role_name)
            .await
            .expect("grant");
        migration_pool.close().await;

        let mut app = url::Url::parse(&admin_url).expect("database url");
        app.set_username(&role_name).ok();
        app.set_password(Some(&role_password)).ok();
        let app_url = app.to_string();

        Self {
            admin_url,
            app_url,
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

async fn app_state(app_url: &str) -> AppState {
    let pool = pool::connect_app(app_url).await.expect("app pool");
    let storage_root = std::env::temp_dir().join(format!("fvoci-db-test-{}", Uuid::now_v7()));
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

async fn json_request(
    app: axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    cookie: Option<&str>,
    extra_headers: &[(&str, &str)],
    peer: Option<std::net::SocketAddr>,
) -> (StatusCode, Value, Option<String>, HeaderMap) {
    let peer = peer.unwrap_or_else(test_peer);
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", format!("fvoci_session={}", cookie));
    }
    for (name, value) in extra_headers {
        builder = builder.header(*name, *value);
    }
    let mut request = if let Some(body) = body {
        builder
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    } else {
        builder.body(Body::empty()).unwrap()
    };
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(peer));
    let response = app.oneshot(request).await.expect("response");
    let status = response.status();
    let headers = response.headers().clone();
    let set_cookie = response
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap_or_default();
    let json = if bytes.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&bytes).unwrap_or(json!({}))
    };
    (status, json, set_cookie, headers)
}

async fn wait_for_profile_patch_blocked(admin: &PgPool, blocker_pid: i32) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let blocked: Option<i32> = sqlx::query_scalar(
            r"
            SELECT activity.pid
            FROM pg_stat_activity AS activity
            WHERE activity.wait_event_type = 'Lock'
              AND activity.state = 'active'
              AND activity.query ILIKE '%fvoci.users%'
              AND activity.query ILIKE '%FOR UPDATE%'
              AND $1 = ANY(pg_blocking_pids(activity.pid))
            LIMIT 1
            ",
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
    panic!("PATCH profile FOR UPDATE did not block on admin suspension lock");
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

async fn setup_session(harness: &TestDb) -> (axum::Router, String, Uuid) {
    let app = router(app_state(&harness.app_url).await, None);
    let (_, _, cookie_hdr, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/setup",
        Some(json!({
            "email": "admin@example.com",
            "password": "supersecret1",
            "givenName": "Admin",
            "workspaceSlug": "acme",
            "workspaceName": "Acme"
        })),
        None,
        &[],
        None,
    )
    .await;
    let cookie = extract_session_cookie(cookie_hdr.as_ref().unwrap());
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let user_id: (Uuid,) = sqlx::query_as("SELECT id FROM fvoci.users LIMIT 1")
        .fetch_one(&admin)
        .await
        .unwrap();
    admin.close().await;
    (app, cookie, user_id.0)
}

async fn reapply_app_grants(admin_url: &str, role_name: &str) {
    let migration_pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(admin_url)
        .await
        .expect("connect for grants");
    fvoci_server::db::migrate::apply_app_role_grants(&migration_pool, role_name)
        .await
        .expect("grant");
    migration_pool.close().await;
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

#[tokio::test]
async fn db_tests_require_database_url() {
    std::env::var("TEST_DATABASE_URL")
        .or_else(|_| std::env::var("FVOCI_TEST_DATABASE_URL"))
        .expect("TEST_DATABASE_URL missing");
}

#[tokio::test]
async fn setup_login_me_patch_logout_flow() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _) = setup_session(&harness).await;

    let (status, body, _, _) = json_request(
        app.clone(),
        "GET",
        "/api/v1/auth/me",
        None,
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["email"], "admin@example.com");
    assert_eq!(body["locale"], "ko");

    let (status, body, _, _) = json_request(
        app.clone(),
        "PATCH",
        "/api/v1/auth/me",
        Some(json!({"givenName": "Renamed", "familyName": null})),
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["givenName"], "Renamed");

    let (status, _, _, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/auth/logout",
        None,
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _, _, _) = json_request(
        app.clone(),
        "GET",
        "/api/v1/auth/me",
        None,
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    harness.cleanup().await;
}

#[tokio::test]
async fn concurrent_setup_has_single_winner() {
    let harness = TestDb::bootstrap().await;
    let pool_a = pool::connect_app(&harness.app_url).await.unwrap();
    let pool_b = pool::connect_app(&harness.app_url).await.unwrap();
    let hash = fvoci_server::auth::password::hash_password(
        "supersecret1",
        &Keyring::parse(PEPPER, "test").unwrap(),
    )
    .await
    .unwrap();
    let token_a = fvoci_server::auth::token::new_token();
    let token_b = fvoci_server::auth::token::new_token();
    let input_a = new_setup_input(SetupSessionParams {
        email: "a@example.com".into(),
        password_hash: hash.clone(),
        given_name: "A".into(),
        family_name: None,
        workspace_slug: "race-a".into(),
        workspace_name: "Race A".into(),
        token_hash: token_a.hash,
        expires_at: Utc::now() + ChronoDuration::days(30),
        client_ip: None,
    });
    let input_b = new_setup_input(SetupSessionParams {
        email: "b@example.com".into(),
        password_hash: hash,
        given_name: "B".into(),
        family_name: None,
        workspace_slug: "race-b".into(),
        workspace_name: "Race B".into(),
        token_hash: token_b.hash,
        expires_at: Utc::now() + ChronoDuration::days(30),
        client_ip: None,
    });
    let (a, b) = tokio::join!(
        setup_first_owner(&pool_a, input_a),
        setup_first_owner(&pool_b, input_b)
    );
    let outcomes = [a.unwrap(), b.unwrap()];
    assert_eq!(
        outcomes
            .iter()
            .filter(|r| matches!(r, SetupFirstOwnerResult::Created))
            .count(),
        1
    );
    assert_eq!(count_users(&pool_a).await.unwrap(), 1);
    pool_a.close().await;
    pool_b.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn revoked_and_expired_sessions_are_rejected() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _) = setup_session(&harness).await;
    let token_hash = hash_token(&cookie);
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    sqlx::query("UPDATE fvoci.sessions SET revoked_at = now() WHERE token_hash = $1")
        .bind(&token_hash)
        .execute(&admin)
        .await
        .unwrap();
    let (status, _, _, _) = json_request(
        app.clone(),
        "GET",
        "/api/v1/auth/me",
        None,
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let token = fvoci_server::auth::token::new_token();
    let expires = Utc::now() - ChronoDuration::hours(1);
    let user_id: (Uuid,) = sqlx::query_as("SELECT id FROM fvoci.users LIMIT 1")
        .fetch_one(&admin)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO fvoci.sessions (id, user_id, token_hash, expires_at) VALUES ($1, $2, $3, $4)",
    )
    .bind(Uuid::now_v7())
    .bind(user_id.0)
    .bind(&token.hash)
    .bind(expires)
    .execute(&admin)
    .await
    .unwrap();
    assert!(find_live_session(&admin, &token.hash)
        .await
        .unwrap()
        .is_none());
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn audit_insert_failure_rolls_back_setup() {
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    install_insert_fail_trigger(&admin, "audit_log", "test_audit_fail").await;
    let pool = pool::connect_app(&harness.app_url).await.unwrap();
    let hash = fvoci_server::auth::password::hash_password(
        "supersecret1",
        &Keyring::parse(PEPPER, "test").unwrap(),
    )
    .await
    .unwrap();
    let token = fvoci_server::auth::token::new_token();
    let input = new_setup_input(SetupSessionParams {
        email: "owner@example.com".into(),
        password_hash: hash,
        given_name: "Owner".into(),
        family_name: None,
        workspace_slug: "owner".into(),
        workspace_name: "Owner".into(),
        token_hash: token.hash,
        expires_at: Utc::now() + ChronoDuration::days(30),
        client_ip: None,
    });
    let result = setup_first_owner(&pool, input).await;
    assert!(result.is_err());
    assert_eq!(count_users(&pool).await.unwrap(), 0);
    pool.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn profile_event_failure_rolls_back_profile_update() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, user_id) = setup_session(&harness).await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    install_insert_fail_trigger(&admin, "events", "test_event_fail").await;

    let (status, _, _, _) = json_request(
        app.clone(),
        "PATCH",
        "/api/v1/auth/me",
        Some(json!({"givenName": "Blocked"})),
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);

    let name: (String,) = sqlx::query_as("SELECT given_name FROM fvoci.users WHERE id = $1")
        .bind(user_id)
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(name.0, "Admin");

    let events: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.events WHERE verb = 'user.name_updated' AND actor_user_id = $1",
    )
    .bind(user_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(events.0, 0);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn profile_audit_failure_rolls_back_profile_update() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, user_id) = setup_session(&harness).await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    install_insert_fail_trigger(&admin, "audit_log", "test_profile_audit_fail").await;

    let (status, _, _, _) = json_request(
        app.clone(),
        "PATCH",
        "/api/v1/auth/me",
        Some(json!({"givenName": "Blocked"})),
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);

    let name: (String,) = sqlx::query_as("SELECT given_name FROM fvoci.users WHERE id = $1")
        .bind(user_id)
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(name.0, "Admin");

    let events: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.events WHERE verb = 'user.name_updated' AND actor_user_id = $1",
    )
    .bind(user_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(events.0, 0);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn patch_me_rejects_foreign_user_id_in_body() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _) = setup_session(&harness).await;
    let other = Uuid::now_v7().to_string();
    let (status, _, _, _) = json_request(
        app,
        "PATCH",
        "/api/v1/auth/me",
        Some(json!({"givenName": "Nope", "userId": other})),
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    harness.cleanup().await;
}

#[tokio::test]
async fn patch_me_rejects_bearer_token_auth() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _) = setup_session(&harness).await;
    let (status, body, _, _) = json_request(
        app,
        "GET",
        "/api/v1/auth/me",
        None,
        Some(&cookie),
        &[("authorization", "Bearer deadbeef")],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");
    harness.cleanup().await;
}

#[tokio::test]
async fn patch_profile_waits_on_suspend_lock_then_returns_unauthorized() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, user_id) = setup_session(&harness).await;
    let admin = PgPoolOptions::new()
        .max_connections(3)
        .connect(&harness.admin_url)
        .await
        .unwrap();

    let (status, body, _, _) = json_request(
        app.clone(),
        "GET",
        "/api/v1/auth/me",
        None,
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["givenName"], "Admin");

    let mut admin_tx = admin.begin().await.unwrap();
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *admin_tx)
        .await
        .unwrap();
    sqlx::query("UPDATE fvoci.users SET suspended_at = now() WHERE id = $1")
        .bind(user_id)
        .execute(&mut *admin_tx)
        .await
        .unwrap();

    let (status, live_body, _, _) = json_request(
        app.clone(),
        "GET",
        "/api/v1/auth/me",
        None,
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(live_body["givenName"], "Admin");

    let patch = tokio::spawn({
        let app = app.clone();
        let cookie = cookie.clone();
        async move {
            json_request(
                app,
                "PATCH",
                "/api/v1/auth/me",
                Some(json!({"givenName": "Race"})),
                Some(&cookie),
                &[],
                None,
            )
            .await
        }
    });

    wait_for_profile_patch_blocked(&admin, blocker_pid).await;
    admin_tx.commit().await.unwrap();

    let (patch_status, patch_body, _, _) = patch.await.unwrap();
    assert_eq!(patch_status, StatusCode::UNAUTHORIZED);
    assert_eq!(patch_body["code"], "authentication_required");

    let given_name: (String,) = sqlx::query_as("SELECT given_name FROM fvoci.users WHERE id = $1")
        .bind(user_id)
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(given_name.0, "Admin");

    let events: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.events WHERE verb = 'user.name_updated' AND actor_user_id = $1",
    )
    .bind(user_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(events.0, 0);

    let audits: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.audit_log WHERE verb = 'user.name_updated' AND actor_user_id = $1",
    )
    .bind(user_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(audits.0, 0);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn stored_password_hash_verifies_on_login() {
    let harness = TestDb::bootstrap().await;
    let app = router(app_state(&harness.app_url).await, None);
    json_request(
        app.clone(),
        "POST",
        "/api/v1/setup",
        Some(json!({
            "email": "hash@example.com",
            "password": "supersecret1",
            "givenName": "Hasher",
            "workspaceSlug": "hashco",
            "workspaceName": "Hash Co"
        })),
        None,
        &[],
        None,
    )
    .await;

    let (status, body, _, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/auth/login",
        Some(json!({"email": "hash@example.com", "password": "supersecret1"})),
        None,
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.get("userId").is_some());

    let (status, _, _, _) = json_request(
        app,
        "POST",
        "/api/v1/auth/login",
        Some(json!({"email": "hash@example.com", "password": "wrong-password"})),
        None,
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    harness.cleanup().await;
}

#[tokio::test]
async fn concurrent_migrations_wait_then_initialize_once() {
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    // This is a UUID database owned exclusively by this test, never a supplied DB.
    sqlx::query("DROP SCHEMA fvoci CASCADE")
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(
        "DROP FUNCTION IF EXISTS public.app_tenant_id(), public.app_system_ctx_on(), public.app_self_user_id(), public.app_invitation_token_hash()",
    )
    .execute(&admin)
    .await
    .unwrap();
    let mut blocker = admin.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(847291003552)")
        .execute(&mut *blocker)
        .await
        .unwrap();
    let release_after_waiters = async {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let waiting: i64 = sqlx::query_scalar(
                    "SELECT count(*) FROM pg_locks WHERE locktype = 'advisory'
                     AND NOT granted AND database =
                     (SELECT oid FROM pg_database WHERE datname = current_database())",
                )
                .fetch_one(&admin)
                .await
                .unwrap();
                if waiting == 2 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("both migrations must reach the held lock");
        blocker.commit().await.unwrap();
    };
    let (first, second, ()) = tokio::time::timeout(Duration::from_secs(20), async {
        tokio::join!(
            migrate::run_migrations(&harness.admin_url),
            migrate::run_migrations(&harness.admin_url),
            release_after_waiters,
        )
    })
    .await
    .expect("both migrations must complete");
    first.unwrap();
    second.unwrap();
    let versions: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.schema_migrations")
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(
        versions,
        i64::from(fvoci_server::db::migrate::latest_migration_version())
    );
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn versioned_migrations_are_idempotent_on_rerun() {
    let harness = TestDb::bootstrap().await;
    migrate::run_migrations(&harness.admin_url)
        .await
        .expect("second migrate");
    migrate::run_migrations(&harness.admin_url)
        .await
        .expect("third migrate");
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let versions: (i64,) = sqlx::query_as("SELECT count(*) FROM fvoci.schema_migrations")
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(
        versions.0,
        i64::from(fvoci_server::db::migrate::latest_migration_version())
    );
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn assert_app_role_rejects_migration_owner_connection() {
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let app = pool::connect_app(&harness.app_url).await.unwrap();
    assert!(
        migrate::assert_app_role(&admin).await.is_err(),
        "migration owner must be rejected at runtime"
    );
    migrate::assert_app_role(&app)
        .await
        .expect("app role should pass guard");
    admin.close().await;
    app.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn patch_family_name_omitted_preserves_existing_value() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, user_id) = setup_session(&harness).await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();

    sqlx::query("UPDATE fvoci.users SET family_name = 'Kim' WHERE id = $1")
        .bind(user_id)
        .execute(&admin)
        .await
        .unwrap();

    let (status, body, _, _) = json_request(
        app.clone(),
        "PATCH",
        "/api/v1/auth/me",
        Some(json!({"givenName": "Renamed"})),
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["givenName"], "Renamed");
    assert_eq!(body["familyName"], "Kim");

    let (status, body, _, _) = json_request(
        app,
        "PATCH",
        "/api/v1/auth/me",
        Some(json!({"givenName": "Renamed", "familyName": null})),
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["familyName"].is_null());

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn patch_rejects_null_name_and_malformed_json_as_problem() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _) = setup_session(&harness).await;
    let (status, body, _, _) = json_request(
        app.clone(),
        "PATCH",
        "/api/v1/auth/me",
        Some(json!({"givenName": null})),
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "invalid_input");
    assert_eq!(body["source"], "/givenName");
    for content_type in [Some("application/json"), None] {
        let mut request = Request::builder()
            .method("PATCH")
            .uri("/api/v1/auth/me")
            .header("cookie", format!("fvoci_session={cookie}"));
        if let Some(value) = content_type {
            request = request.header("content-type", value);
        }
        let response = app
            .clone()
            .oneshot(request.body(Body::from("{")).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response.headers()["content-type"],
            "application/problem+json"
        );
        let bytes = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        let problem: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(problem["code"], "invalid_input");
    }
    harness.cleanup().await;
}

#[tokio::test]
async fn setup_rejects_unknown_fields() {
    let harness = TestDb::bootstrap().await;
    let app = router(app_state(&harness.app_url).await, None);
    let (status, _, _, _) = json_request(
        app,
        "POST",
        "/api/v1/setup",
        Some(json!({
            "email": "x@example.com",
            "password": "supersecret1",
            "givenName": "X",
            "workspaceSlug": "xco",
            "workspaceName": "X Co",
            "extra": true
        })),
        None,
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    harness.cleanup().await;
}

#[tokio::test]
async fn patch_profile_waits_on_session_revoke_lock_then_returns_unauthorized() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, user_id) = setup_session(&harness).await;
    let token_hash = hash_token(&cookie);
    let admin = PgPoolOptions::new()
        .max_connections(3)
        .connect(&harness.admin_url)
        .await
        .unwrap();

    let mut admin_tx = admin.begin().await.unwrap();
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *admin_tx)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM fvoci.sessions WHERE user_id = $1 AND token_hash = $2 FOR UPDATE")
        .bind(user_id)
        .bind(&token_hash)
        .execute(&mut *admin_tx)
        .await
        .unwrap();

    let patch = tokio::spawn({
        let app = app.clone();
        let cookie = cookie.clone();
        async move {
            json_request(
                app,
                "PATCH",
                "/api/v1/auth/me",
                Some(json!({"givenName": "Race"})),
                Some(&cookie),
                &[],
                None,
            )
            .await
        }
    });

    wait_for_profile_patch_blocked(&admin, blocker_pid).await;
    sqlx::query("UPDATE fvoci.sessions SET revoked_at = now() WHERE token_hash = $1")
        .bind(&token_hash)
        .execute(&mut *admin_tx)
        .await
        .unwrap();
    admin_tx.commit().await.unwrap();

    let (patch_status, patch_body, _, _) = patch.await.unwrap();
    assert_eq!(patch_status, StatusCode::UNAUTHORIZED);
    assert_eq!(patch_body["code"], "authentication_required");

    let given_name: (String,) = sqlx::query_as("SELECT given_name FROM fvoci.users WHERE id = $1")
        .bind(user_id)
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(given_name.0, "Admin");

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn rate_limit_uses_socket_ip_not_forwarded_for() {
    let harness = TestDb::bootstrap().await;
    let app = router(app_state(&harness.app_url).await, None);
    let peer_a = std::net::SocketAddr::from(([203, 0, 113, 1], 42424));
    let peer_b = std::net::SocketAddr::from(([203, 0, 113, 2], 42424));

    for _ in 0..10 {
        let (status, _, _, _) = json_request(
            app.clone(),
            "POST",
            "/api/v1/auth/login",
            Some(json!({"email": "a@example.com", "password": "wrong-password"})),
            None,
            &[("x-forwarded-for", "10.0.0.99")],
            Some(peer_a),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    let (status, _, _, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/auth/login",
        Some(json!({"email": "a@example.com", "password": "wrong-password"})),
        None,
        &[("x-forwarded-for", "10.0.0.100")],
        Some(peer_a),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);

    let (status, _, _, _) = json_request(
        app,
        "POST",
        "/api/v1/auth/login",
        Some(json!({"email": "a@example.com", "password": "wrong-password"})),
        None,
        &[("x-forwarded-for", "10.0.0.99")],
        Some(peer_b),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    harness.cleanup().await;
}

#[tokio::test]
async fn setup_rejects_short_password_by_utf16_length() {
    let harness = TestDb::bootstrap().await;
    let app = router(app_state(&harness.app_url).await, None);
    let (status, body, _, _) = json_request(
        app,
        "POST",
        "/api/v1/setup",
        Some(json!({
            "email": "short@example.com",
            "password": "가나다라",
            "givenName": "Short",
            "workspaceSlug": "short",
            "workspaceName": "Short"
        })),
        None,
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "password_invalid");
    harness.cleanup().await;
}

#[tokio::test]
async fn login_rate_limit_returns_contract_fields() {
    let harness = TestDb::bootstrap().await;
    let app = router(app_state(&harness.app_url).await, None);
    let peer = std::net::SocketAddr::from(([203, 0, 113, 99], 42424));
    for _ in 0..10 {
        let (status, _, _, _) = json_request(
            app.clone(),
            "POST",
            "/api/v1/auth/login",
            Some(json!({"email": "victim@example.com", "password": "wrong-password"})),
            None,
            &[],
            Some(peer),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    let (status, body, _, headers) = json_request(
        app,
        "POST",
        "/api/v1/auth/login",
        Some(json!({"email": "victim@example.com", "password": "wrong-password"})),
        None,
        &[],
        Some(peer),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body["code"], "rate_limit_exceeded");
    assert!(body["params"]["retryAfter"].is_number());
    let retry_after = headers.get("retry-after").unwrap().to_str().unwrap();
    assert_eq!(
        body["params"]["retryAfter"].as_u64().unwrap().to_string(),
        retry_after
    );
    harness.cleanup().await;
}

#[tokio::test]
async fn origin_mismatch_returns_forbidden_problem() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _) = setup_session(&harness).await;
    let (status, body, _, _) = json_request(
        app,
        "POST",
        "/api/v1/auth/logout",
        None,
        Some(&cookie),
        &[("origin", "http://evil.example.com")],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "origin_mismatch");
    harness.cleanup().await;
}

#[tokio::test]
async fn setup_records_client_ip_in_audit_log() {
    let harness = TestDb::bootstrap().await;
    let app = router(app_state(&harness.app_url).await, None);
    let peer = std::net::SocketAddr::from(([203, 0, 113, 50], 42424));
    let (_, _, _, _) = json_request(
        app,
        "POST",
        "/api/v1/setup",
        Some(json!({
            "email": "audit@example.com",
            "password": "supersecret1",
            "givenName": "Audit",
            "workspaceSlug": "auditco",
            "workspaceName": "Audit Co"
        })),
        None,
        &[],
        Some(peer),
    )
    .await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let ip: (Option<String>,) = sqlx::query_as(
        "SELECT host(ip) FROM fvoci.audit_log WHERE verb = 'instance.setup' LIMIT 1",
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(ip.0.as_deref(), Some("203.0.113.50"));
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn app_role_cannot_read_secret_columns() {
    let harness = TestDb::bootstrap().await;
    let app = pool::connect_app(&harness.app_url).await.unwrap();
    let denied_password =
        sqlx::query_scalar::<_, String>("SELECT password_hash FROM fvoci.users LIMIT 1")
            .fetch_optional(&app)
            .await;
    assert!(denied_password.is_err());
    let denied_token =
        sqlx::query_scalar::<_, String>("SELECT token_hash FROM fvoci.sessions LIMIT 1")
            .fetch_optional(&app)
            .await;
    assert!(denied_token.is_err());
    let max_version =
        sqlx::query_scalar::<_, i32>("SELECT max(version) FROM fvoci.schema_migrations")
            .fetch_one(&app)
            .await
            .expect("app role may read migration version");
    assert_eq!(max_version, migrate::latest_migration_version());
    for sql in [
        "INSERT INTO fvoci.schema_migrations (version) VALUES (999)",
        "UPDATE fvoci.schema_migrations SET version = version",
        "DELETE FROM fvoci.schema_migrations",
    ] {
        let error = sqlx::query(sql).execute(&app).await.expect_err(sql);
        let code = error
            .as_database_error()
            .and_then(|db| db.code())
            .map(|code| code.to_string());
        assert_eq!(code.as_deref(), Some("42501"), "{sql}: {error}");
    }
    app.close().await;
    harness.cleanup().await;
}

async fn create_second_user_session(
    harness: &TestDb,
    email: &str,
    given_name: &str,
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
    .bind(given_name)
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

#[tokio::test]
async fn logout_unknown_cookie_returns_no_content_without_event() {
    let harness = TestDb::bootstrap().await;
    let app = router(app_state(&harness.app_url).await, None);
    let (status, _, _, _) = json_request(
        app,
        "POST",
        "/api/v1/auth/logout",
        None,
        Some("not-a-real-session-token"),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let events: (i64,) =
        sqlx::query_as("SELECT count(*) FROM fvoci.events WHERE verb = 'auth.logout'")
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(events.0, 0);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn workspace_list_get_patch_and_create_flow() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _) = setup_session(&harness).await;

    let (status, body, _, _) = json_request(
        app.clone(),
        "GET",
        "/api/v1/me/workspaces",
        None,
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    let workspace_id = body["items"][0]["id"].as_str().unwrap();
    assert_eq!(body["items"][0]["role"], "owner");
    // Interim constant until documents/tasks slices exist (not computed aggregates).
    assert_eq!(body["items"][0]["documentCount"], 0);

    let (status, body, _, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{}", workspace_id),
        None,
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["slug"], "acme");

    let (status, body, _, _) = json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{}", workspace_id),
        Some(json!({"name": "Acme Renamed"})),
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["name"], "Acme Renamed");

    let (status, _, _, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/workspaces",
        Some(json!({"name": "Beta", "slug": "beta-ws"})),
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, body, _, _) = json_request(
        app,
        "GET",
        "/api/v1/me/workspaces",
        None,
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["items"].as_array().unwrap().len(), 2);

    harness.cleanup().await;
}

#[tokio::test]
async fn workspace_nonmember_and_cross_tenant_access_are_denied() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _) = setup_session(&harness).await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let owner_workspace: (Uuid,) =
        sqlx::query_as("SELECT id FROM fvoci.workspaces WHERE slug = 'acme'")
            .fetch_one(&admin)
            .await
            .unwrap();
    let other_workspace = Uuid::now_v7();
    sqlx::query("INSERT INTO fvoci.workspaces (id, slug, name) VALUES ($1, 'other', 'Other')")
        .bind(other_workspace)
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;

    let (status, body, _, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{}", other_workspace),
        None,
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");

    let (status, _, _, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{}", owner_workspace.0),
        None,
        None,
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let app_pool = pool::connect_app(&harness.app_url).await.unwrap();
    let leaked = fvoci_server::db::workspace::tenant_context_probe(
        &app_pool,
        owner_workspace.0,
        other_workspace,
    )
    .await
    .unwrap();
    assert!(leaked.is_none());
    app_pool.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn workspace_role_matrix() {
    let harness = TestDb::bootstrap().await;
    let (app, admin_cookie, owner_id) = setup_session(&harness).await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let workspace_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM fvoci.workspaces WHERE slug = 'acme'")
            .fetch_one(&admin)
            .await
            .unwrap();
    let (actor_id, actor_cookie) =
        create_second_user_session(&harness, "member@example.com", "Member").await;
    let (target_id, _) = create_second_user_session(&harness, "peer@example.com", "Peer").await;
    let app_pool = pool::connect_app(&harness.app_url).await.unwrap();
    for (user_id, role) in [
        (actor_id, fvoci_server::db::workspace::WorkspaceRole::Member),
        (
            target_id,
            fvoci_server::db::workspace::WorkspaceRole::Member,
        ),
    ] {
        fvoci_server::db::workspace::add_membership_for_test(
            &app_pool,
            workspace_id.0,
            user_id,
            role,
        )
        .await
        .unwrap();
    }
    app_pool.close().await;
    admin.close().await;

    let (status, body, _, _) = json_request(
        app.clone(),
        "PATCH",
        &format!(
            "/api/v1/workspaces/{}/members/{}",
            workspace_id.0, target_id
        ),
        Some(json!({"role": "admin"})),
        Some(&actor_cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");

    let (status, _, _, _) = json_request(
        app.clone(),
        "PATCH",
        &format!(
            "/api/v1/workspaces/{}/members/{}",
            workspace_id.0, target_id
        ),
        Some(json!({"role": "admin"})),
        Some(&admin_cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _, _, _) = json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{}", workspace_id.0),
        Some(json!({"name": "Denied"})),
        Some(&actor_cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _, _, _) = json_request(
        app.clone(),
        "PATCH",
        &format!(
            "/api/v1/workspaces/{}/members/{}",
            workspace_id.0, target_id
        ),
        Some(json!({"role": "owner"})),
        Some(&admin_cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _, _, _) = json_request(
        app.clone(),
        "DELETE",
        &format!(
            "/api/v1/workspaces/{}/members/{}",
            workspace_id.0, target_id
        ),
        None,
        Some(&admin_cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body, _, _) = json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{}/members/{}", workspace_id.0, owner_id),
        Some(json!({"role": "member"})),
        Some(&admin_cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "workspace_member_self_change_forbidden");

    harness.cleanup().await;
}

#[tokio::test]
async fn workspace_member_event_failure_rolls_back_role_change() {
    let harness = TestDb::bootstrap().await;
    let (app, admin_cookie, _) = setup_session(&harness).await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let workspace_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM fvoci.workspaces WHERE slug = 'acme'")
            .fetch_one(&admin)
            .await
            .unwrap();
    let (member_id, _) = create_second_user_session(&harness, "evt@example.com", "Evt").await;
    fvoci_server::db::workspace::add_membership_for_test(
        &pool::connect_app(&harness.app_url).await.unwrap(),
        workspace_id.0,
        member_id,
        fvoci_server::db::workspace::WorkspaceRole::Member,
    )
    .await
    .unwrap();
    install_insert_fail_trigger(&admin, "events", "test_ws_event_fail").await;

    let (status, _, _, _) = json_request(
        app.clone(),
        "PATCH",
        &format!(
            "/api/v1/workspaces/{}/members/{}",
            workspace_id.0, member_id
        ),
        Some(json!({"role": "admin"})),
        Some(&admin_cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);

    let role: (String,) = sqlx::query_as(
        "SELECT role FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(workspace_id.0)
    .bind(member_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(role.0, "member");

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn personal_workspace_is_immutable_and_idempotent() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, user_id) = setup_session(&harness).await;

    let (status, body, _, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/me/personal-workspace",
        None,
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let personal_id = body["id"].as_str().unwrap();
    assert_eq!(body["name"], "Personal");
    assert!(body["slug"].as_str().unwrap().starts_with("u-"));

    let (status, body, _, _) = json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{}", personal_id),
        Some(json!({"name": "Denied"})),
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "personal_workspace_is_immutable");

    let (status, body, _, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/me/personal-workspace",
        None,
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["id"], personal_id);

    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let pointer: (Option<Uuid>,) =
        sqlx::query_as("SELECT personal_workspace_id FROM fvoci.users WHERE id = $1")
            .bind(user_id)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(pointer.0.unwrap().to_string(), personal_id);
    let events: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.events WHERE verb = 'workspace.personal_created' AND actor_user_id = $1",
    )
    .bind(user_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(events.0, 1);
    let (status, _, _, _) = json_request(
        app,
        "POST",
        "/api/v1/me/personal-workspace",
        None,
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let events: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.events WHERE verb = 'workspace.personal_created' AND actor_user_id = $1",
    )
    .bind(user_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(events.0, 1);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn personal_workspace_creation_writes_event_and_audit() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, user_id) = setup_session(&harness).await;
    let peer = std::net::SocketAddr::from(([203, 0, 113, 70], 42424));
    let (status, body, _, _) = json_request(
        app,
        "POST",
        "/api/v1/me/personal-workspace",
        None,
        Some(&cookie),
        &[],
        Some(peer),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let personal_id = Uuid::parse_str(body["id"].as_str().unwrap()).unwrap();
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let events: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.events WHERE verb = 'workspace.personal_created' AND workspace_id = $1",
    )
    .bind(personal_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(events.0, 1);
    let audits: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.audit_log WHERE verb = 'workspace.personal_created' AND actor_user_id = $1",
    )
    .bind(user_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(audits.0, 1);
    let ip: (Option<String>,) = sqlx::query_as(
        "SELECT host(ip) FROM fvoci.audit_log WHERE verb = 'workspace.personal_created' AND actor_user_id = $1",
    )
    .bind(user_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(ip.0.as_deref(), Some("203.0.113.70"));
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn personal_workspace_event_failure_rolls_back_creation() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, user_id) = setup_session(&harness).await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    install_insert_fail_trigger(&admin, "events", "test_personal_event_fail").await;
    let (status, _, _, _) = json_request(
        app,
        "POST",
        "/api/v1/me/personal-workspace",
        None,
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    let pointer: (Option<Uuid>,) =
        sqlx::query_as("SELECT personal_workspace_id FROM fvoci.users WHERE id = $1")
            .bind(user_id)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert!(pointer.0.is_none());
    let workspaces: (i64,) =
        sqlx::query_as("SELECT count(*) FROM fvoci.workspaces WHERE kind = 'personal'")
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(workspaces.0, 0);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn personal_workspace_audit_failure_rolls_back_creation() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, user_id) = setup_session(&harness).await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    install_insert_fail_trigger(&admin, "audit_log", "test_personal_audit_fail").await;
    let (status, _, _, _) = json_request(
        app,
        "POST",
        "/api/v1/me/personal-workspace",
        None,
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    let pointer: (Option<Uuid>,) =
        sqlx::query_as("SELECT personal_workspace_id FROM fvoci.users WHERE id = $1")
            .bind(user_id)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert!(pointer.0.is_none());
    let events: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.events WHERE verb = 'workspace.personal_created'",
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(events.0, 0);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn workspace_member_change_waits_on_suspend_lock() {
    let harness = TestDb::bootstrap().await;
    let (app, admin_cookie, admin_user_id) = setup_session(&harness).await;
    let admin = PgPoolOptions::new()
        .max_connections(3)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let workspace_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM fvoci.workspaces WHERE slug = 'acme'")
            .fetch_one(&admin)
            .await
            .unwrap();
    let (member_id, _) = create_second_user_session(&harness, "race@example.com", "Race").await;
    fvoci_server::db::workspace::add_membership_for_test(
        &pool::connect_app(&harness.app_url).await.unwrap(),
        workspace_id.0,
        member_id,
        fvoci_server::db::workspace::WorkspaceRole::Member,
    )
    .await
    .unwrap();

    let mut admin_tx = admin.begin().await.unwrap();
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *admin_tx)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM fvoci.users WHERE id = $1 FOR UPDATE")
        .bind(admin_user_id)
        .execute(&mut *admin_tx)
        .await
        .unwrap();

    let patch = tokio::spawn({
        let app = app.clone();
        let admin_cookie = admin_cookie.clone();
        let workspace_id = workspace_id.0;
        async move {
            json_request(
                app,
                "PATCH",
                &format!("/api/v1/workspaces/{}/members/{}", workspace_id, member_id),
                Some(json!({"role": "admin"})),
                Some(&admin_cookie),
                &[],
                None,
            )
            .await
        }
    });

    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let blocked: Option<i32> = sqlx::query_scalar(
            r"
            SELECT activity.pid
            FROM pg_stat_activity AS activity
            WHERE activity.wait_event_type = 'Lock'
              AND activity.state = 'active'
              AND activity.query ILIKE '%fvoci.users%'
              AND activity.query ILIKE '%FOR UPDATE%'
              AND $1 = ANY(pg_blocking_pids(activity.pid))
            LIMIT 1
            ",
        )
        .bind(blocker_pid)
        .fetch_optional(&admin)
        .await
        .unwrap();
        if blocked.is_some() {
            break;
        }
        tokio::task::yield_now().await;
    }
    sqlx::query("UPDATE fvoci.users SET suspended_at = now() WHERE id = $1")
        .bind(admin_user_id)
        .execute(&mut *admin_tx)
        .await
        .unwrap();
    admin_tx.commit().await.unwrap();

    let (status, body, _, _) = patch.await.unwrap();
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");

    let role: (String,) = sqlx::query_as(
        "SELECT role FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(workspace_id.0)
    .bind(member_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(role.0, "member");
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn pool_connection_tenant_context_resets_across_commit_and_rollback() {
    let harness = TestDb::bootstrap().await;
    let (app, _, _) = setup_session(&harness).await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let ws_a: (Uuid,) = sqlx::query_as("SELECT id FROM fvoci.workspaces WHERE slug = 'acme'")
        .fetch_one(&admin)
        .await
        .unwrap();
    let ws_b = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO fvoci.workspaces (id, slug, name) VALUES ($1, 'tenant-b', 'Tenant B')",
    )
    .bind(ws_b)
    .execute(&admin)
    .await
    .unwrap();
    admin.close().await;
    drop(app);

    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&harness.app_url)
        .await
        .unwrap();

    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
        .bind(ws_a.0.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let visible_a: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM fvoci.workspaces WHERE id = $1")
            .bind(ws_a.0)
            .fetch_optional(&mut *tx)
            .await
            .unwrap();
    assert!(visible_a.is_some());
    tx.commit().await.unwrap();

    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
        .bind(ws_b.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let hidden_a: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM fvoci.workspaces WHERE id = $1")
        .bind(ws_a.0)
        .fetch_optional(&mut *tx)
        .await
        .unwrap();
    assert!(hidden_a.is_none());
    tx.rollback().await.unwrap();

    let mut tx = pool.begin().await.unwrap();
    let anonymous: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM fvoci.workspaces WHERE id = $1")
            .bind(ws_a.0)
            .fetch_optional(&mut *tx)
            .await
            .unwrap();
    assert!(anonymous.is_none());
    tx.rollback().await.unwrap();
    pool.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn logout_revoked_token_does_not_emit_duplicate_event() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _) = setup_session(&harness).await;
    let (status, _, _, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/auth/logout",
        None,
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _, _, _) = json_request(
        app,
        "POST",
        "/api/v1/auth/logout",
        None,
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let events: (i64,) =
        sqlx::query_as("SELECT count(*) FROM fvoci.events WHERE verb = 'auth.logout'")
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(events.0, 1);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn migration_001_002_database_upgrades_to_003() {
    let harness = TestDb::bootstrap().await;
    let (_, _, owner_id) = setup_session(&harness).await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    sqlx::query("DROP POLICY IF EXISTS memberships_select_self ON fvoci.memberships")
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query("ALTER TABLE fvoci.users DROP CONSTRAINT IF EXISTS users_personal_workspace_fk")
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query("DROP INDEX IF EXISTS fvoci.users_personal_workspace_id_unique")
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query("DROP FUNCTION IF EXISTS public.app_self_user_id()")
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query("DELETE FROM fvoci.schema_migrations WHERE version = 3")
        .execute(&admin)
        .await
        .unwrap();
    migrate::run_migrations(&harness.admin_url)
        .await
        .expect("upgrade to 003");
    let has_fn: (bool,) =
        sqlx::query_as("SELECT EXISTS (SELECT 1 FROM pg_proc WHERE proname = 'app_self_user_id')")
            .fetch_one(&admin)
            .await
            .unwrap();
    assert!(has_fn.0);
    let has_fk: (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'users_personal_workspace_fk')",
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert!(has_fk.0);
    let has_idx: (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM pg_indexes WHERE indexname = 'users_personal_workspace_id_unique')",
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert!(has_idx.0);
    let versions: (i64,) = sqlx::query_as("SELECT count(*) FROM fvoci.schema_migrations")
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(
        versions.0,
        i64::from(fvoci_server::db::migrate::latest_migration_version())
    );
    reapply_app_grants(&harness.admin_url, &harness.role_name).await;
    let app_pool = pool::connect_app(&harness.app_url).await.unwrap();
    let mut tx = app_pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.self_user_id', $1, true)")
        .bind(owner_id.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let visible: Vec<(Uuid,)> = sqlx::query_as("SELECT user_id FROM fvoci.memberships")
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    assert!(!visible.is_empty());
    assert!(visible.iter().all(|(user_id,)| *user_id == owner_id));
    tx.rollback().await.unwrap();
    app_pool.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn app_role_cannot_mutate_foreign_tenant_rows() {
    let harness = TestDb::bootstrap().await;
    let _ = setup_session(&harness).await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let ws_a: (Uuid,) = sqlx::query_as("SELECT id FROM fvoci.workspaces WHERE slug = 'acme'")
        .fetch_one(&admin)
        .await
        .unwrap();
    let ws_b = Uuid::now_v7();
    sqlx::query("INSERT INTO fvoci.workspaces (id, slug, name) VALUES ($1, 'tenant-b', 'B')")
        .bind(ws_b)
        .execute(&admin)
        .await
        .unwrap();

    let app_pool = pool::connect_app(&harness.app_url).await.unwrap();
    let mut tx = app_pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
        .bind(ws_a.0.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let foreign_insert = sqlx::query(
        "INSERT INTO fvoci.memberships (workspace_id, user_id, role) VALUES ($1, $2, 'guest')",
    )
    .bind(ws_b)
    .bind(Uuid::now_v7())
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

    let mut tx = app_pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
        .bind(ws_a.0.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let denied_update = sqlx::query("UPDATE fvoci.workspaces SET name = 'hacked' WHERE id = $1")
        .bind(ws_b)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(denied_update.rows_affected(), 0);
    tx.rollback().await.unwrap();
    let name: (String,) = sqlx::query_as("SELECT name FROM fvoci.workspaces WHERE id = $1")
        .bind(ws_b)
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(name.0, "B");
    app_pool.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn two_workspaces_keep_membership_lists_isolated() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _) = setup_session(&harness).await;
    let (status, _, _, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/workspaces",
        Some(json!({"name": "Beta", "slug": "beta-iso"})),
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (other_id, other_cookie) =
        create_second_user_session(&harness, "other@example.com", "Other").await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let beta: (Uuid,) = sqlx::query_as("SELECT id FROM fvoci.workspaces WHERE slug = 'beta-iso'")
        .fetch_one(&admin)
        .await
        .unwrap();
    fvoci_server::db::workspace::add_membership_for_test(
        &pool::connect_app(&harness.app_url).await.unwrap(),
        beta.0,
        other_id,
        fvoci_server::db::workspace::WorkspaceRole::Guest,
    )
    .await
    .unwrap();
    admin.close().await;

    let (status, body, _, _) = json_request(
        app.clone(),
        "GET",
        "/api/v1/me/workspaces",
        None,
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["items"].as_array().unwrap().len(), 2);

    let (status, body, _, _) = json_request(
        app.clone(),
        "GET",
        "/api/v1/me/workspaces",
        None,
        Some(&other_cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    assert_eq!(body["items"][0]["slug"], "beta-iso");
    assert_eq!(body["items"][0]["role"], "guest");

    let (status, _, _, _) = json_request(
        app,
        "GET",
        &format!("/api/v1/workspaces/{}", beta.0),
        None,
        Some(&other_cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    harness.cleanup().await;
}

#[tokio::test]
async fn concurrent_admin_member_mutations_complete_without_deadlock() {
    let harness = TestDb::bootstrap().await;
    let (app, admin_cookie, _) = setup_session(&harness).await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let workspace_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM fvoci.workspaces WHERE slug = 'acme'")
            .fetch_one(&admin)
            .await
            .unwrap();
    let (member_a, _) = create_second_user_session(&harness, "ma@example.com", "A").await;
    let (member_b, _) = create_second_user_session(&harness, "mb@example.com", "B").await;
    let app_pool = pool::connect_app(&harness.app_url).await.unwrap();
    for (id, role) in [
        (member_a, fvoci_server::db::workspace::WorkspaceRole::Member),
        (member_b, fvoci_server::db::workspace::WorkspaceRole::Admin),
    ] {
        fvoci_server::db::workspace::add_membership_for_test(&app_pool, workspace_id.0, id, role)
            .await
            .unwrap();
    }
    app_pool.close().await;
    admin.close().await;

    let demote = tokio::spawn({
        let app = app.clone();
        let admin_cookie = admin_cookie.clone();
        let workspace_id = workspace_id.0;
        async move {
            json_request(
                app,
                "PATCH",
                &format!("/api/v1/workspaces/{}/members/{}", workspace_id, member_a),
                Some(json!({"role": "guest"})),
                Some(&admin_cookie),
                &[],
                None,
            )
            .await
        }
    });
    let remove = tokio::spawn({
        let app = app.clone();
        let admin_cookie = admin_cookie.clone();
        let workspace_id = workspace_id.0;
        async move {
            json_request(
                app,
                "DELETE",
                &format!("/api/v1/workspaces/{}/members/{}", workspace_id, member_b),
                None,
                Some(&admin_cookie),
                &[],
                None,
            )
            .await
        }
    });
    let (demote, remove) = tokio::time::timeout(Duration::from_secs(15), async {
        tokio::join!(demote, remove)
    })
    .await
    .expect("concurrent member mutations must not deadlock");
    let (demote_status, _, _, _) = demote.unwrap();
    let (remove_status, _, _, _) = remove.unwrap();
    assert_eq!(demote_status, StatusCode::OK);
    assert_eq!(remove_status, StatusCode::OK);
    harness.cleanup().await;
}

#[tokio::test]
async fn workspace_member_remove_waits_on_session_revoke_lock() {
    let harness = TestDb::bootstrap().await;
    let (app, admin_cookie, admin_user_id) = setup_session(&harness).await;
    let token_hash = hash_token(&admin_cookie);
    let admin = PgPoolOptions::new()
        .max_connections(3)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let workspace_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM fvoci.workspaces WHERE slug = 'acme'")
            .fetch_one(&admin)
            .await
            .unwrap();
    let (member_id, _) = create_second_user_session(&harness, "rm@example.com", "Rm").await;
    fvoci_server::db::workspace::add_membership_for_test(
        &pool::connect_app(&harness.app_url).await.unwrap(),
        workspace_id.0,
        member_id,
        fvoci_server::db::workspace::WorkspaceRole::Member,
    )
    .await
    .unwrap();

    let mut admin_tx = admin.begin().await.unwrap();
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *admin_tx)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM fvoci.sessions WHERE user_id = $1 AND token_hash = $2 FOR UPDATE")
        .bind(admin_user_id)
        .bind(&token_hash)
        .execute(&mut *admin_tx)
        .await
        .unwrap();

    let delete_req = tokio::spawn({
        let app = app.clone();
        let admin_cookie = admin_cookie.clone();
        let workspace_id = workspace_id.0;
        async move {
            json_request(
                app,
                "DELETE",
                &format!("/api/v1/workspaces/{}/members/{}", workspace_id, member_id),
                None,
                Some(&admin_cookie),
                &[],
                None,
            )
            .await
        }
    });

    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let blocked: Option<i32> = sqlx::query_scalar(
            r"
            SELECT activity.pid
            FROM pg_stat_activity AS activity
            WHERE activity.wait_event_type = 'Lock'
              AND activity.state = 'active'
              AND activity.query ILIKE '%fvoci.sessions%'
              AND activity.query ILIKE '%FOR UPDATE%'
              AND $1 = ANY(pg_blocking_pids(activity.pid))
            LIMIT 1
            ",
        )
        .bind(blocker_pid)
        .fetch_optional(&admin)
        .await
        .unwrap();
        if blocked.is_some() {
            break;
        }
        tokio::task::yield_now().await;
    }
    sqlx::query("UPDATE fvoci.sessions SET revoked_at = now() WHERE token_hash = $1")
        .bind(&token_hash)
        .execute(&mut *admin_tx)
        .await
        .unwrap();
    admin_tx.commit().await.unwrap();

    let (status, body, _, _) = delete_req.await.unwrap();
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");

    let members: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(workspace_id.0)
    .bind(member_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(members.0, 1);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn last_owner_concurrent_demotion_has_single_winner() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_a_cookie, owner_a_id) = setup_session(&harness).await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let workspace_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM fvoci.workspaces WHERE slug = 'acme'")
            .fetch_one(&admin)
            .await
            .unwrap();
    let (owner_b_id, owner_b_cookie) =
        create_second_user_session(&harness, "ownerb@example.com", "OwnerB").await;
    fvoci_server::db::workspace::add_membership_for_test(
        &pool::connect_app(&harness.app_url).await.unwrap(),
        workspace_id.0,
        owner_b_id,
        fvoci_server::db::workspace::WorkspaceRole::Owner,
    )
    .await
    .unwrap();
    admin.close().await;

    let demote_b = tokio::spawn({
        let app = app.clone();
        let cookie = owner_a_cookie.clone();
        let ws = workspace_id.0;
        async move {
            json_request(
                app,
                "PATCH",
                &format!("/api/v1/workspaces/{}/members/{}", ws, owner_b_id),
                Some(json!({"role": "member"})),
                Some(&cookie),
                &[],
                None,
            )
            .await
        }
    });
    let demote_a = tokio::spawn({
        let app = app.clone();
        let ws = workspace_id.0;
        async move {
            json_request(
                app,
                "PATCH",
                &format!("/api/v1/workspaces/{}/members/{}", ws, owner_a_id),
                Some(json!({"role": "member"})),
                Some(&owner_b_cookie),
                &[],
                None,
            )
            .await
        }
    });
    let (a, b) = tokio::time::timeout(Duration::from_secs(15), async {
        tokio::join!(demote_b, demote_a)
    })
    .await
    .expect("concurrent owner demotions must not deadlock");
    let results = [a.unwrap(), b.unwrap()];
    assert_eq!(
        results
            .iter()
            .filter(|(status, _, _, _)| *status == StatusCode::OK)
            .count(),
        1
    );
    let (loser_status, loser_body, _, _) = results
        .iter()
        .find(|(status, _, _, _)| *status != StatusCode::OK)
        .expect("one loser");
    assert_eq!(*loser_status, StatusCode::NOT_FOUND);
    assert_eq!(loser_body["code"], "not_found");
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let owners: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.memberships WHERE workspace_id = $1 AND role = 'owner'",
    )
    .bind(workspace_id.0)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(owners.0, 1);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn last_owner_concurrent_removal_has_single_winner() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_a_cookie, owner_a_id) = setup_session(&harness).await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let workspace_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM fvoci.workspaces WHERE slug = 'acme'")
            .fetch_one(&admin)
            .await
            .unwrap();
    let (owner_b_id, owner_b_cookie) =
        create_second_user_session(&harness, "remb@example.com", "RemB").await;
    fvoci_server::db::workspace::add_membership_for_test(
        &pool::connect_app(&harness.app_url).await.unwrap(),
        workspace_id.0,
        owner_b_id,
        fvoci_server::db::workspace::WorkspaceRole::Owner,
    )
    .await
    .unwrap();
    admin.close().await;

    let remove_b = tokio::spawn({
        let app = app.clone();
        let cookie = owner_a_cookie.clone();
        let ws = workspace_id.0;
        async move {
            json_request(
                app,
                "DELETE",
                &format!("/api/v1/workspaces/{}/members/{}", ws, owner_b_id),
                None,
                Some(&cookie),
                &[],
                None,
            )
            .await
        }
    });
    let remove_a = tokio::spawn({
        let app = app.clone();
        let ws = workspace_id.0;
        async move {
            json_request(
                app,
                "DELETE",
                &format!("/api/v1/workspaces/{}/members/{}", ws, owner_a_id),
                None,
                Some(&owner_b_cookie),
                &[],
                None,
            )
            .await
        }
    });
    let (a, b) = tokio::time::timeout(Duration::from_secs(15), async {
        tokio::join!(remove_b, remove_a)
    })
    .await
    .expect("concurrent owner removals must not deadlock");
    let results = [a.unwrap(), b.unwrap()];
    assert_eq!(
        results
            .iter()
            .filter(|(status, _, _, _)| *status == StatusCode::OK)
            .count(),
        1
    );
    let (loser_status, loser_body, _, _) = results
        .iter()
        .find(|(status, _, _, _)| *status != StatusCode::OK)
        .expect("one loser");
    assert_eq!(*loser_status, StatusCode::NOT_FOUND);
    assert_eq!(loser_body["code"], "not_found");
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let owners: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.memberships WHERE workspace_id = $1 AND role = 'owner'",
    )
    .bind(workspace_id.0)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(owners.0, 1);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn pool_tenant_context_does_not_leak_after_failed_statement() {
    let harness = TestDb::bootstrap().await;
    let (app, _, _) = setup_session(&harness).await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let ws_b = Uuid::now_v7();
    sqlx::query("INSERT INTO fvoci.workspaces (id, slug, name) VALUES ($1, 'leak-b', 'Leak B')")
        .bind(ws_b)
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
    drop(app);

    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&harness.app_url)
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
        .bind(ws_b.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let bad = sqlx::query("SELECT 1 / 0").execute(&mut *tx).await;
    assert!(bad.is_err());
    tx.rollback().await.unwrap();

    let mut tx = pool.begin().await.unwrap();
    let leaked: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM fvoci.workspaces WHERE id = $1")
        .bind(ws_b)
        .fetch_optional(&mut *tx)
        .await
        .unwrap();
    assert!(leaked.is_none());
    tx.rollback().await.unwrap();
    pool.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn non_instance_admin_cannot_create_workspace() {
    let harness = TestDb::bootstrap().await;
    let _ = setup_session(&harness).await;
    let (member_id, member_cookie) =
        create_second_user_session(&harness, "plain@example.com", "Plain").await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    sqlx::query("UPDATE fvoci.users SET is_instance_admin = false WHERE id = $1")
        .bind(member_id)
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;

    let app = router(app_state(&harness.app_url).await, None);
    let (status, body, _, _) = json_request(
        app,
        "POST",
        "/api/v1/workspaces",
        Some(json!({"name": "Nope", "slug": "nope-ws"})),
        Some(&member_cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "insufficient_permissions");
    harness.cleanup().await;
}

#[tokio::test]
async fn workspace_name_patch_writes_event_and_audit() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, user_id) = setup_session(&harness).await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let workspace_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM fvoci.workspaces WHERE slug = 'acme'")
            .fetch_one(&admin)
            .await
            .unwrap();
    let peer = std::net::SocketAddr::from(([203, 0, 113, 60], 42424));
    let (status, body, _, _) = json_request(
        app,
        "PATCH",
        &format!("/api/v1/workspaces/{}", workspace_id.0),
        Some(json!({"name": "Renamed Co"})),
        Some(&cookie),
        &[],
        Some(peer),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["name"], "Renamed Co");
    let events: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.events WHERE verb = 'workspace.name_updated' AND workspace_id = $1",
    )
    .bind(workspace_id.0)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(events.0, 1);
    let audits: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.audit_log WHERE verb = 'workspace.name_updated' AND actor_user_id = $1",
    )
    .bind(user_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(audits.0, 1);
    let payload: (serde_json::Value,) = sqlx::query_as(
        "SELECT payload FROM fvoci.events WHERE verb = 'workspace.name_updated' AND workspace_id = $1",
    )
    .bind(workspace_id.0)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(payload.0["name"], "Renamed Co");
    assert_eq!(payload.0["fromName"], "Acme");
    let ip: (Option<String>,) = sqlx::query_as(
        "SELECT host(ip) FROM fvoci.audit_log WHERE verb = 'workspace.name_updated' AND actor_user_id = $1",
    )
    .bind(user_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(ip.0.as_deref(), Some("203.0.113.60"));
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn workspace_name_event_failure_rolls_back_update() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _) = setup_session(&harness).await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let workspace_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM fvoci.workspaces WHERE slug = 'acme'")
            .fetch_one(&admin)
            .await
            .unwrap();
    install_insert_fail_trigger(&admin, "events", "test_ws_name_event_fail").await;
    let (status, _, _, _) = json_request(
        app,
        "PATCH",
        &format!("/api/v1/workspaces/{}", workspace_id.0),
        Some(json!({"name": "Blocked"})),
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    let name: (String,) = sqlx::query_as("SELECT name FROM fvoci.workspaces WHERE id = $1")
        .bind(workspace_id.0)
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(name.0, "Acme");
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn workspace_name_audit_failure_rolls_back_update() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _) = setup_session(&harness).await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let workspace_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM fvoci.workspaces WHERE slug = 'acme'")
            .fetch_one(&admin)
            .await
            .unwrap();
    install_insert_fail_trigger(&admin, "audit_log", "test_ws_name_audit_fail").await;
    let (status, _, _, _) = json_request(
        app,
        "PATCH",
        &format!("/api/v1/workspaces/{}", workspace_id.0),
        Some(json!({"name": "Blocked"})),
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    let name: (String,) = sqlx::query_as("SELECT name FROM fvoci.workspaces WHERE id = $1")
        .bind(workspace_id.0)
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(name.0, "Acme");
    let events: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.events WHERE verb = 'workspace.name_updated' AND workspace_id = $1",
    )
    .bind(workspace_id.0)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(events.0, 0);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn workspace_member_audit_failure_rolls_back_role_change() {
    let harness = TestDb::bootstrap().await;
    let (app, admin_cookie, _) = setup_session(&harness).await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let workspace_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM fvoci.workspaces WHERE slug = 'acme'")
            .fetch_one(&admin)
            .await
            .unwrap();
    let (member_id, _) = create_second_user_session(&harness, "aud@example.com", "Aud").await;
    fvoci_server::db::workspace::add_membership_for_test(
        &pool::connect_app(&harness.app_url).await.unwrap(),
        workspace_id.0,
        member_id,
        fvoci_server::db::workspace::WorkspaceRole::Member,
    )
    .await
    .unwrap();
    install_insert_fail_trigger(&admin, "audit_log", "test_ws_member_audit_fail").await;
    let (status, _, _, _) = json_request(
        app,
        "PATCH",
        &format!(
            "/api/v1/workspaces/{}/members/{}",
            workspace_id.0, member_id
        ),
        Some(json!({"role": "admin"})),
        Some(&admin_cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    let role: (String,) = sqlx::query_as(
        "SELECT role FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(workspace_id.0)
    .bind(member_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(role.0, "member");
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn workspace_delete_member_writes_event_and_audit() {
    let harness = TestDb::bootstrap().await;
    let (app, admin_cookie, owner_id) = setup_session(&harness).await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let workspace_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM fvoci.workspaces WHERE slug = 'acme'")
            .fetch_one(&admin)
            .await
            .unwrap();
    let (member_id, _) = create_second_user_session(&harness, "del@example.com", "Del").await;
    fvoci_server::db::workspace::add_membership_for_test(
        &pool::connect_app(&harness.app_url).await.unwrap(),
        workspace_id.0,
        member_id,
        fvoci_server::db::workspace::WorkspaceRole::Member,
    )
    .await
    .unwrap();
    let (status, _, _, _) = json_request(
        app,
        "DELETE",
        &format!(
            "/api/v1/workspaces/{}/members/{}",
            workspace_id.0, member_id
        ),
        None,
        Some(&admin_cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let members: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(workspace_id.0)
    .bind(member_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(members.0, 0);
    let events: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.events WHERE verb = 'workspace_member.removed' AND workspace_id = $1",
    )
    .bind(workspace_id.0)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(events.0, 1);
    let audits: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.audit_log WHERE verb = 'workspace_member.removed' AND actor_user_id = $1",
    )
    .bind(owner_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(audits.0, 1);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn workspace_delete_member_event_failure_rolls_back_removal() {
    let harness = TestDb::bootstrap().await;
    let (app, admin_cookie, _) = setup_session(&harness).await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let workspace_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM fvoci.workspaces WHERE slug = 'acme'")
            .fetch_one(&admin)
            .await
            .unwrap();
    let (member_id, _) = create_second_user_session(&harness, "del2@example.com", "Del2").await;
    fvoci_server::db::workspace::add_membership_for_test(
        &pool::connect_app(&harness.app_url).await.unwrap(),
        workspace_id.0,
        member_id,
        fvoci_server::db::workspace::WorkspaceRole::Member,
    )
    .await
    .unwrap();
    install_insert_fail_trigger(&admin, "events", "test_ws_remove_event_fail").await;
    let (status, _, _, _) = json_request(
        app,
        "DELETE",
        &format!(
            "/api/v1/workspaces/{}/members/{}",
            workspace_id.0, member_id
        ),
        None,
        Some(&admin_cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    let members: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(workspace_id.0)
    .bind(member_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(members.0, 1);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn workspace_admin_cannot_demote_owner_returns_forbidden() {
    let harness = TestDb::bootstrap().await;
    let (app, _, owner_id) = setup_session(&harness).await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let workspace_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM fvoci.workspaces WHERE slug = 'acme'")
            .fetch_one(&admin)
            .await
            .unwrap();
    let (admin_id, admin_cookie) =
        create_second_user_session(&harness, "wsadmin@example.com", "WsAdmin").await;
    fvoci_server::db::workspace::add_membership_for_test(
        &pool::connect_app(&harness.app_url).await.unwrap(),
        workspace_id.0,
        admin_id,
        fvoci_server::db::workspace::WorkspaceRole::Admin,
    )
    .await
    .unwrap();
    admin.close().await;
    let (status, body, _, _) = json_request(
        app,
        "PATCH",
        &format!("/api/v1/workspaces/{}/members/{}", workspace_id.0, owner_id),
        Some(json!({"role": "member"})),
        Some(&admin_cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "cannot_manage_a_role_above_your_own");
    harness.cleanup().await;
}

#[tokio::test]
async fn deleted_target_member_patch_returns_not_found_before_mutation() {
    let harness = TestDb::bootstrap().await;
    let (app, admin_cookie, _) = setup_session(&harness).await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let workspace_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM fvoci.workspaces WHERE slug = 'acme'")
            .fetch_one(&admin)
            .await
            .unwrap();
    let (member_id, _) = create_second_user_session(&harness, "gone@example.com", "Gone").await;
    fvoci_server::db::workspace::add_membership_for_test(
        &pool::connect_app(&harness.app_url).await.unwrap(),
        workspace_id.0,
        member_id,
        fvoci_server::db::workspace::WorkspaceRole::Member,
    )
    .await
    .unwrap();
    sqlx::query("UPDATE fvoci.users SET deleted_at = now() WHERE id = $1")
        .bind(member_id)
        .execute(&admin)
        .await
        .unwrap();
    let (status, body, _, _) = json_request(
        app,
        "PATCH",
        &format!(
            "/api/v1/workspaces/{}/members/{}",
            workspace_id.0, member_id
        ),
        Some(json!({"role": "admin"})),
        Some(&admin_cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");
    let role: (String,) = sqlx::query_as(
        "SELECT role FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(workspace_id.0)
    .bind(member_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(role.0, "member");
    let events: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.events WHERE verb = 'workspace_member.role_changed' AND workspace_id = $1",
    )
    .bind(workspace_id.0)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(events.0, 0);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn nonmember_personal_workspace_member_patch_returns_not_found() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, owner_id) = setup_session(&harness).await;
    let (_, stranger_cookie) =
        create_second_user_session(&harness, "stranger@example.com", "Stranger").await;
    let (status, body, _, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/me/personal-workspace",
        None,
        Some(&owner_cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let personal_id = body["id"].as_str().unwrap();
    let (status, body, _, _) = json_request(
        app,
        "PATCH",
        &format!("/api/v1/workspaces/{}/members/{}", personal_id, owner_id),
        Some(json!({"role": "guest"})),
        Some(&stranger_cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");
    harness.cleanup().await;
}

#[tokio::test]
async fn personal_workspace_origin_mismatch_returns_forbidden() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _) = setup_session(&harness).await;
    let (status, body, _, _) = json_request(
        app,
        "POST",
        "/api/v1/me/personal-workspace",
        None,
        Some(&cookie),
        &[("origin", "http://evil.example.com")],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "origin_mismatch");
    harness.cleanup().await;
}

#[tokio::test]
async fn workspace_mutating_routes_reject_origin_mismatch() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id) = setup_session(&harness).await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let workspace_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM fvoci.workspaces WHERE slug = 'acme'")
            .fetch_one(&admin)
            .await
            .unwrap();
    let (member_id, _) = create_second_user_session(&harness, "ori@example.com", "Ori").await;
    fvoci_server::db::workspace::add_membership_for_test(
        &pool::connect_app(&harness.app_url).await.unwrap(),
        workspace_id.0,
        member_id,
        fvoci_server::db::workspace::WorkspaceRole::Member,
    )
    .await
    .unwrap();
    admin.close().await;
    let evil = &[("origin", "http://evil.example.com")];
    let (status, body, _, _) = json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{}", workspace_id.0),
        Some(json!({"name": "Evil"})),
        Some(&cookie),
        evil,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "origin_mismatch");
    let (status, body, _, _) = json_request(
        app.clone(),
        "PATCH",
        &format!(
            "/api/v1/workspaces/{}/members/{}",
            workspace_id.0, member_id
        ),
        Some(json!({"role": "admin"})),
        Some(&cookie),
        evil,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "origin_mismatch");
    let (status, body, _, _) = json_request(
        app.clone(),
        "DELETE",
        &format!(
            "/api/v1/workspaces/{}/members/{}",
            workspace_id.0, member_id
        ),
        None,
        Some(&cookie),
        evil,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "origin_mismatch");
    let (status, body, _, _) = json_request(
        app,
        "POST",
        "/api/v1/workspaces",
        Some(json!({"name": "Evil", "slug": "evil-ws"})),
        Some(&cookie),
        evil,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "origin_mismatch");
    let _ = owner_id;
    harness.cleanup().await;
}

#[tokio::test]
async fn pool_self_user_and_system_ctx_reset_after_commit_rollback_and_error() {
    let harness = TestDb::bootstrap().await;
    let (app, _, user_id) = setup_session(&harness).await;
    drop(app);
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&harness.app_url)
        .await
        .unwrap();

    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.self_user_id', $1, true)")
        .bind(user_id.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('app.system_ctx', 'on', true)")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.self_user_id', $1, true)")
        .bind(user_id.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.rollback().await.unwrap();

    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.self_user_id', $1, true)")
        .bind(user_id.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('app.system_ctx', 'on', true)")
        .execute(&mut *tx)
        .await
        .unwrap();
    let bad = sqlx::query("SELECT 1 / 0").execute(&mut *tx).await;
    assert!(bad.is_err());
    tx.rollback().await.unwrap();

    let self_user: Option<String> =
        sqlx::query_scalar("SELECT current_setting('app.self_user_id', true)")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(self_user.is_none() || self_user.as_deref() == Some(""));
    let system_ctx: Option<String> =
        sqlx::query_scalar("SELECT current_setting('app.system_ctx', true)")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(system_ctx.is_none() || system_ctx.as_deref() == Some(""));
    pool.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn fresh_migration_003_adds_personal_workspace_constraints() {
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let has_fk: (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'users_personal_workspace_fk')",
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert!(has_fk.0);
    let has_idx: (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM pg_indexes WHERE indexname = 'users_personal_workspace_id_unique')",
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert!(has_idx.0);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn create_workspace_duplicate_slug_returns_conflict() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _) = setup_session(&harness).await;
    let (status, body, _, _) = json_request(
        app,
        "POST",
        "/api/v1/workspaces",
        Some(json!({"name": "Dup", "slug": "acme"})),
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "slug_taken");
    harness.cleanup().await;
}

#[tokio::test]
async fn create_workspace_fullwidth_slug_folds_to_existing_slug_returns_conflict() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _) = setup_session(&harness).await;
    let (status, body, _, _) = json_request(
        app,
        "POST",
        "/api/v1/workspaces",
        Some(json!({"name": "Folded", "slug": "ａｃｍｅ"})),
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "slug_taken");
    harness.cleanup().await;
}

#[tokio::test]
async fn personal_workspace_member_ops_return_personal_immutable() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, user_id) = setup_session(&harness).await;
    let (status, body, _, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/me/personal-workspace",
        None,
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let personal_id = body["id"].as_str().unwrap();
    let other_id = Uuid::now_v7();
    for (method, path, body) in [
        (
            "PATCH",
            format!("/api/v1/workspaces/{}/members/{}", personal_id, user_id),
            Some(json!({"role": "guest"})),
        ),
        (
            "DELETE",
            format!("/api/v1/workspaces/{}/members/{}", personal_id, user_id),
            None,
        ),
        (
            "PATCH",
            format!("/api/v1/workspaces/{}/members/{}", personal_id, other_id),
            Some(json!({"role": "guest"})),
        ),
        (
            "DELETE",
            format!("/api/v1/workspaces/{}/members/{}", personal_id, other_id),
            None,
        ),
    ] {
        let (status, body, _, _) =
            json_request(app.clone(), method, &path, body, Some(&cookie), &[], None).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["code"], "personal_workspace_is_immutable");
    }
    harness.cleanup().await;
}

#[tokio::test]
async fn memberships_select_self_policy_isolates_users() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie_a, user_a) = setup_session(&harness).await;
    let (user_b, cookie_b) =
        create_second_user_session(&harness, "selfb@example.com", "SelfB").await;
    let (status, _, _, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/workspaces",
        Some(json!({"name": "Beta", "slug": "self-beta"})),
        Some(&cookie_a),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let beta_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM fvoci.workspaces WHERE slug = 'self-beta'")
            .fetch_one(&admin)
            .await
            .unwrap();
    fvoci_server::db::workspace::add_membership_for_test(
        &pool::connect_app(&harness.app_url).await.unwrap(),
        beta_id.0,
        user_b,
        fvoci_server::db::workspace::WorkspaceRole::Guest,
    )
    .await
    .unwrap();
    admin.close().await;

    let app_pool = pool::connect_app(&harness.app_url).await.unwrap();
    let mut tx = app_pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.self_user_id', $1, true)")
        .bind(user_a.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let rows_a: Vec<(Uuid,)> = sqlx::query_as("SELECT user_id FROM fvoci.memberships")
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    assert!(rows_a.iter().all(|(id,)| *id == user_a));
    tx.rollback().await.unwrap();

    let mut tx = app_pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.self_user_id', $1, true)")
        .bind(user_b.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let rows_b: Vec<(Uuid,)> = sqlx::query_as("SELECT user_id FROM fvoci.memberships")
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    assert_eq!(rows_b.len(), 1);
    assert_eq!(rows_b[0].0, user_b);
    tx.rollback().await.unwrap();
    app_pool.close().await;
    let _ = cookie_b;
    harness.cleanup().await;
}

#[tokio::test]
async fn workspace_create_event_failure_rolls_back_workspace() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, user_id) = setup_session(&harness).await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    install_insert_fail_trigger(&admin, "events", "test_ws_create_event_fail").await;
    let before: (i64,) = sqlx::query_as("SELECT count(*) FROM fvoci.workspaces")
        .fetch_one(&admin)
        .await
        .unwrap();
    let (status, _, _, _) = json_request(
        app,
        "POST",
        "/api/v1/workspaces",
        Some(json!({"name": "Blocked", "slug": "blocked-ws"})),
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    let after: (i64,) = sqlx::query_as("SELECT count(*) FROM fvoci.workspaces")
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(before.0, after.0);
    let memberships: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.memberships WHERE user_id = $1 AND workspace_id IN (SELECT id FROM fvoci.workspaces WHERE slug = 'blocked-ws')",
    )
    .bind(user_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(memberships.0, 0);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn workspace_create_audit_failure_rolls_back_workspace() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _) = setup_session(&harness).await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    install_insert_fail_trigger(&admin, "audit_log", "test_ws_create_audit_fail").await;
    let (status, _, _, _) = json_request(
        app,
        "POST",
        "/api/v1/workspaces",
        Some(json!({"name": "Blocked", "slug": "blocked-audit"})),
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    let exists: (i64,) =
        sqlx::query_as("SELECT count(*) FROM fvoci.workspaces WHERE slug = 'blocked-audit'")
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(exists.0, 0);
    let events: (i64,) =
        sqlx::query_as("SELECT count(*) FROM fvoci.events WHERE verb = 'workspace.created'")
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(events.0, 0);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn workspace_delete_member_audit_failure_rolls_back_removal() {
    let harness = TestDb::bootstrap().await;
    let (app, admin_cookie, _) = setup_session(&harness).await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let workspace_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM fvoci.workspaces WHERE slug = 'acme'")
            .fetch_one(&admin)
            .await
            .unwrap();
    let (member_id, _) = create_second_user_session(&harness, "audrm@example.com", "AudRm").await;
    fvoci_server::db::workspace::add_membership_for_test(
        &pool::connect_app(&harness.app_url).await.unwrap(),
        workspace_id.0,
        member_id,
        fvoci_server::db::workspace::WorkspaceRole::Member,
    )
    .await
    .unwrap();
    install_insert_fail_trigger(&admin, "audit_log", "test_ws_remove_audit_fail").await;
    let (status, _, _, _) = json_request(
        app,
        "DELETE",
        &format!(
            "/api/v1/workspaces/{}/members/{}",
            workspace_id.0, member_id
        ),
        None,
        Some(&admin_cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    let members: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(workspace_id.0)
    .bind(member_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(members.0, 1);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn pool_context_resets_when_transaction_is_dropped_without_rollback() {
    let harness = TestDb::bootstrap().await;
    let (app, _, user_id) = setup_session(&harness).await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let ws_a: (Uuid,) = sqlx::query_as("SELECT id FROM fvoci.workspaces WHERE slug = 'acme'")
        .fetch_one(&admin)
        .await
        .unwrap();
    admin.close().await;
    drop(app);

    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&harness.app_url)
        .await
        .unwrap();
    {
        let mut tx = pool.begin().await.unwrap();
        sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
            .bind(ws_a.0.to_string())
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("SELECT set_config('app.self_user_id', $1, true)")
            .bind(user_id.to_string())
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("SELECT set_config('app.system_ctx', 'on', true)")
            .execute(&mut *tx)
            .await
            .unwrap();
        drop(tx);
    }
    let tenant: Option<String> =
        sqlx::query_scalar("SELECT current_setting('app.tenant_id', true)")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(tenant.is_none() || tenant.as_deref() == Some(""));
    let self_user: Option<String> =
        sqlx::query_scalar("SELECT current_setting('app.self_user_id', true)")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(self_user.is_none() || self_user.as_deref() == Some(""));
    let system_ctx: Option<String> =
        sqlx::query_scalar("SELECT current_setting('app.system_ctx', true)")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(system_ctx.is_none() || system_ctx.as_deref() == Some(""));
    pool.close().await;
    harness.cleanup().await;
}

struct UngrantedDb {
    admin_url: String,
    role_name: String,
    app_url: String,
}

async fn migrated_db_without_grants() -> UngrantedDb {
    let admin_base = std::env::var("TEST_DATABASE_URL")
        .or_else(|_| std::env::var("FVOCI_TEST_DATABASE_URL"))
        .expect("TEST_DATABASE_URL missing");
    let db_name = format!("fvoci_test_{}", Uuid::now_v7().simple());
    let role_name = format!("fvoci_app_{db_name}");
    let mut password_bytes = [0u8; 24];
    rand::rng().fill_bytes(&mut password_bytes);
    let role_password = hex::encode(password_bytes);
    let server_url = server_db_url(&admin_base);
    let admin_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&server_url)
        .await
        .expect("connect admin");
    sqlx::query(&format!("CREATE DATABASE \"{db_name}\""))
        .execute(&admin_pool)
        .await
        .expect("create database");
    admin_pool.close().await;
    let admin_url = join_db_url(&server_url, &db_name);
    migrate::run_migrations(&admin_url).await.expect("migrate");
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin_url)
        .await
        .expect("connect migrated db");
    sqlx::query(&format!(
        "CREATE ROLE \"{role_name}\" LOGIN PASSWORD '{role_password}' NOSUPERUSER NOBYPASSRLS"
    ))
    .execute(&admin)
    .await
    .expect("create role");
    admin.close().await;
    let mut app = url::Url::parse(&admin_url).expect("database url");
    app.set_username(&role_name).ok();
    app.set_password(Some(&role_password)).ok();
    UngrantedDb {
        admin_url,
        role_name,
        app_url: app.to_string(),
    }
}

async fn drop_ungranted(db: UngrantedDb) {
    let server_url = server_db_url(&db.admin_url);
    let db_name = url::Url::parse(&db.admin_url)
        .expect("url")
        .path()
        .trim_start_matches('/')
        .to_string();
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&server_url)
        .await
        .expect("connect server");
    sqlx::query(&format!(
        "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '{db_name}'"
    ))
    .execute(&pool)
    .await
    .ok();
    sqlx::query(&format!("DROP DATABASE IF EXISTS \"{db_name}\""))
        .execute(&pool)
        .await
        .expect("drop database");
    sqlx::query(&format!("DROP ROLE IF EXISTS \"{}\"", db.role_name))
        .execute(&pool)
        .await
        .expect("drop role");
    pool.close().await;
}

async fn role_has_any_fvoci_privilege(admin: &PgPool, role: &str) -> Vec<String> {
    sqlx::query_scalar(
        r#"
        SELECT 'schema usage' WHERE has_schema_privilege($1, 'fvoci', 'USAGE')
        UNION ALL
        SELECT format('%s on %s', p.privilege, c.relname)
        FROM pg_class c
        INNER JOIN pg_namespace n ON n.oid = c.relnamespace
        CROSS JOIN (VALUES ('SELECT'), ('INSERT'), ('UPDATE'), ('DELETE')) AS p(privilege)
        WHERE n.nspname = 'fvoci' AND c.relkind IN ('r', 'p')
          AND has_table_privilege($1, c.oid, p.privilege)
        UNION ALL
        SELECT format('execute %s', p.oid::regprocedure)
        FROM pg_proc p
        INNER JOIN pg_namespace n ON n.oid = p.pronamespace
        WHERE n.nspname IN ('fvoci', 'public') AND p.prosecdef
          AND has_function_privilege($1, p.oid, 'EXECUTE')
        "#,
    )
    .bind(role)
    .fetch_all(admin)
    .await
    .expect("privilege probe")
}

async fn assert_forbidden_app_access_denied(app_url: &str) {
    let app = PgPoolOptions::new()
        .max_connections(1)
        .connect(app_url)
        .await
        .expect("connect app role");
    for (label, sql) in [
        (
            "password hash",
            "SELECT password_hash FROM fvoci.users LIMIT 1",
        ),
        (
            "session token",
            "SELECT token_hash FROM fvoci.sessions LIMIT 1",
        ),
        (
            "migrations insert",
            "INSERT INTO fvoci.schema_migrations (version) VALUES (999)",
        ),
        (
            "migrations update",
            "UPDATE fvoci.schema_migrations SET version = version",
        ),
        ("migrations delete", "DELETE FROM fvoci.schema_migrations"),
        ("audit update", "UPDATE fvoci.audit_log SET verb = verb"),
        ("audit delete", "DELETE FROM fvoci.audit_log"),
        ("event update", "UPDATE fvoci.events SET verb = verb"),
        (
            "receipt update",
            "UPDATE fvoci.document_collab_op_receipts SET op_id = op_id",
        ),
        (
            "receipt delete",
            "DELETE FROM fvoci.document_collab_op_receipts",
        ),
        (
            "instance admin escalation",
            "UPDATE fvoci.users SET is_instance_admin = true",
        ),
        (
            "password overwrite",
            "UPDATE fvoci.users SET password_hash = 'x'",
        ),
        ("email overwrite", "UPDATE fvoci.users SET email = email"),
        (
            "auth generation reset",
            "UPDATE fvoci.users SET auth_generation = auth_generation",
        ),
        ("user delete", "DELETE FROM fvoci.users"),
        (
            "session token overwrite",
            "UPDATE fvoci.sessions SET token_hash = 'x'",
        ),
        (
            "session reassignment",
            "UPDATE fvoci.sessions SET user_id = user_id",
        ),
        ("event delete", "DELETE FROM fvoci.events"),
    ] {
        let error = sqlx::query(sql)
            .execute(&app)
            .await
            .expect_err(&format!("{label} must be denied to the app role"));
        let code = error
            .as_database_error()
            .and_then(|db| db.code())
            .map(|code| code.to_string());
        assert_eq!(code.as_deref(), Some("42501"), "{label}: {error}");
    }
    app.close().await;
}

#[tokio::test]
async fn definer_functions_are_not_public_between_migrate_and_grant() {
    let db = migrated_db_without_grants().await;
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&db.admin_url)
        .await
        .unwrap();
    let public_definers: Vec<String> = sqlx::query_scalar(
        r#"
        SELECT p.oid::regprocedure::text
        FROM pg_proc p
        INNER JOIN pg_namespace n ON n.oid = p.pronamespace
        WHERE n.nspname IN ('fvoci', 'public') AND p.prosecdef
          AND has_function_privilege('public', p.oid, 'EXECUTE')
        "#,
    )
    .fetch_all(&admin)
    .await
    .unwrap();
    assert!(
        public_definers.is_empty(),
        "PUBLIC can execute {public_definers:?}"
    );
    let leaked = role_has_any_fvoci_privilege(&admin, &db.role_name).await;
    assert!(leaked.is_empty(), "ungranted role already has {leaked:?}");
    admin.close().await;
    drop_ungranted(db).await;
}

#[tokio::test]
async fn failed_grant_leaves_no_partial_privileges_via_migrate_binary() {
    let db = migrated_db_without_grants().await;
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&db.admin_url)
        .await
        .unwrap();
    // The last statements of grant-app-role.sql reference this function, so the
    // broad table grant near the top has already executed when the script fails.
    sqlx::query("DROP FUNCTION fvoci.app_claim_attachment_extract()")
        .execute(&admin)
        .await
        .unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_fvoci-migrate"))
        .arg("--grant-app-role")
        .arg(&db.role_name)
        .env("DATABASE_URL", &db.admin_url)
        .env_remove("FVOCI_MIGRATION_URL")
        .output()
        .expect("run fvoci-migrate");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "grant must fail: {stderr}");
    assert!(stderr.contains("no privileges were committed"), "{stderr}");
    let leaked = role_has_any_fvoci_privilege(&admin, &db.role_name).await;
    assert!(leaked.is_empty(), "failed grant left {leaked:?}");
    admin.close().await;
    drop_ungranted(db).await;
}

#[tokio::test]
async fn failed_grant_via_library_rolls_back_then_rerun_restores_narrow_grants() {
    let db = migrated_db_without_grants().await;
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&db.admin_url)
        .await
        .unwrap();
    sqlx::query("ALTER FUNCTION fvoci.app_claim_attachment_extract() RENAME TO app_claim_hidden")
        .execute(&admin)
        .await
        .unwrap();
    let error = migrate::apply_app_role_grants(&admin, &db.role_name)
        .await
        .expect_err("grant must fail while the function is missing");
    assert!(
        error.to_string().contains("no privileges were committed"),
        "{error}"
    );
    assert!(role_has_any_fvoci_privilege(&admin, &db.role_name)
        .await
        .is_empty());

    sqlx::query("ALTER FUNCTION fvoci.app_claim_hidden() RENAME TO app_claim_attachment_extract")
        .execute(&admin)
        .await
        .unwrap();
    migrate::grant_app_role(&db.admin_url, &db.role_name)
        .await
        .expect("grant");
    migrate::grant_app_role(&db.admin_url, &db.role_name)
        .await
        .expect("grant rerun is idempotent");
    assert_forbidden_app_access_denied(&db.app_url).await;

    let app_pool = pool::connect_app(&db.app_url).await.expect("app pool");
    migrate::assert_app_role(&app_pool)
        .await
        .expect("app role checks");
    assert_eq!(count_users(&app_pool).await.expect("count users"), 0);
    app_pool.close().await;
    admin.close().await;
    drop_ungranted(db).await;
}

#[tokio::test]
async fn grant_refuses_owner_superuser_bypassrls_and_missing_roles() {
    let db = migrated_db_without_grants().await;
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&db.admin_url)
        .await
        .unwrap();
    let schema_owner: String = sqlx::query_scalar(
        "SELECT pg_get_userbyid(nspowner)::text FROM pg_namespace WHERE nspname = 'fvoci'",
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    let suffix = Uuid::now_v7().simple().to_string();
    let inherits_owner = format!("fvoci_inherit_{suffix}");
    let owns_table = format!("fvoci_owner_{suffix}");
    let bypass = format!("fvoci_bypass_{suffix}");
    let superuser = format!("fvoci_super_{suffix}");
    for statement in [
        format!("CREATE ROLE \"{inherits_owner}\" NOSUPERUSER NOBYPASSRLS"),
        format!("GRANT \"{schema_owner}\" TO \"{inherits_owner}\""),
        format!("CREATE ROLE \"{owns_table}\" NOSUPERUSER NOBYPASSRLS"),
        "CREATE TABLE fvoci.grant_owner_probe (id int)".to_string(),
        format!("ALTER TABLE fvoci.grant_owner_probe OWNER TO \"{owns_table}\""),
        format!("CREATE ROLE \"{bypass}\" NOSUPERUSER BYPASSRLS"),
        format!("CREATE ROLE \"{superuser}\" SUPERUSER"),
    ] {
        sqlx::query(&statement)
            .execute(&admin)
            .await
            .expect(&statement);
    }
    for role in [
        schema_owner.as_str(),
        inherits_owner.as_str(),
        owns_table.as_str(),
        bypass.as_str(),
        superuser.as_str(),
        "fvoci_missing_role_for_grant_test",
    ] {
        let error = migrate::grant_app_role(&db.admin_url, role)
            .await
            .expect_err("grant must be refused");
        assert!(error.to_string().contains("refused"), "{role}: {error}");
    }
    let owner_can_read: bool =
        sqlx::query_scalar("SELECT has_table_privilege($1, 'fvoci.users', 'SELECT')")
            .bind(&schema_owner)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert!(
        owner_can_read,
        "refused grants must not touch the owner ACL"
    );
    sqlx::query("DROP TABLE fvoci.grant_owner_probe")
        .execute(&admin)
        .await
        .unwrap();
    for role in [&inherits_owner, &owns_table, &bypass, &superuser] {
        sqlx::query(&format!("DROP ROLE \"{role}\""))
            .execute(&admin)
            .await
            .unwrap();
    }
    admin.close().await;
    drop_ungranted(db).await;
}

fn server_process_env(app_url: &str, storage_root: &std::path::Path) -> std::process::Command {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_fvoci-server"));
    command
        .env("DATABASE_APP_URL", app_url)
        .env_remove("DATABASE_URL")
        .env_remove("FVOCI_MIGRATION_URL")
        .env("PASSWORD_PEPPER_KEYS", PEPPER)
        .env("PASSWORD_PEPPER_ACTIVE_KEY_ID", "test")
        .env("FVOCI_BIND", "127.0.0.1:0")
        .env("FVOCI_PUBLIC_ORIGIN", "http://localhost")
        .env("FVOCI_COOKIE_SECURE", "0")
        .env(
            "FVOCI_STORAGE_DIR",
            storage_root.to_string_lossy().to_string(),
        )
        .env("FVOCI_SHUTDOWN_DEADLINE_MS", "5000")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    command
}

fn assert_schema_gate_process_failure(output: &std::process::Output, expected_phrases: &[&str]) {
    assert!(
        !output.status.success(),
        "server must exit nonzero: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    for phrase in expected_phrases {
        assert!(
            combined.contains(phrase),
            "expected {phrase:?} in operator message, got: {combined}"
        );
    }
}

#[tokio::test]
async fn server_exits_when_schema_is_behind() {
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    sqlx::query("DELETE FROM fvoci.schema_migrations WHERE version = 8")
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;

    let storage_root = std::env::temp_dir().join(format!("fvoci-schema-gate-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&storage_root).expect("storage root");
    let output = server_process_env(&harness.app_url, &storage_root)
        .output()
        .expect("spawn fvoci-server");
    assert_schema_gate_process_failure(
        &output,
        &[
            "behind compiled version",
            migrate::SCHEMA_GATE_OPERATOR_HINT,
        ],
    );
    let _ = std::fs::remove_dir_all(storage_root);
    harness.cleanup().await;
}

#[tokio::test]
async fn server_exits_when_schema_is_newer_than_binary() {
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    sqlx::query("INSERT INTO fvoci.schema_migrations (version) VALUES (999)")
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;

    let storage_root = std::env::temp_dir().join(format!("fvoci-schema-gate-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&storage_root).expect("storage root");
    let output = server_process_env(&harness.app_url, &storage_root)
        .output()
        .expect("spawn fvoci-server");
    assert_schema_gate_process_failure(
        &output,
        &["newer than this binary", "deploy a matching fvoci-server"],
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        !combined.contains("fvoci-migrate"),
        "newer schema must not tell the operator to migrate: {combined}"
    );
    let _ = std::fs::remove_dir_all(storage_root);
    harness.cleanup().await;
}

#[tokio::test]
async fn server_exits_when_no_migrations_applied() {
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    sqlx::query("DELETE FROM fvoci.schema_migrations")
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;

    let storage_root = std::env::temp_dir().join(format!("fvoci-schema-gate-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&storage_root).expect("storage root");
    let output = server_process_env(&harness.app_url, &storage_root)
        .output()
        .expect("spawn fvoci-server");
    assert_schema_gate_process_failure(
        &output,
        &["no applied migrations", migrate::SCHEMA_GATE_OPERATOR_HINT],
    );
    let _ = std::fs::remove_dir_all(storage_root);
    harness.cleanup().await;
}

#[tokio::test]
async fn server_exits_on_unmigrated_database() {
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
        .max_connections(1)
        .connect(&server_url)
        .await
        .expect("connect admin");
    sqlx::query(&format!("CREATE DATABASE \"{db_name}\""))
        .execute(&admin_pool)
        .await
        .expect("create database");
    admin_pool.close().await;
    let admin_url = join_db_url(&server_url, &db_name);
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin_url)
        .await
        .expect("connect fresh db");
    sqlx::query(&format!(
        "CREATE ROLE \"{role_name}\" LOGIN PASSWORD '{role_password}' NOSUPERUSER NOBYPASSRLS"
    ))
    .execute(&admin)
    .await
    .expect("create role");
    admin.close().await;
    let mut app = url::Url::parse(&admin_url).expect("database url");
    app.set_username(&role_name).ok();
    app.set_password(Some(&role_password)).ok();
    let app_url = app.to_string();

    let storage_root = std::env::temp_dir().join(format!("fvoci-schema-gate-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&storage_root).expect("storage root");
    let output = server_process_env(&app_url, &storage_root)
        .output()
        .expect("spawn fvoci-server");
    assert_schema_gate_process_failure(
        &output,
        &[
            "cannot read fvoci.schema_migrations",
            migrate::SCHEMA_GATE_OPERATOR_HINT,
        ],
    );
    let _ = std::fs::remove_dir_all(storage_root);

    let cleanup_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&server_url)
        .await
        .expect("connect server");
    sqlx::query(&format!(
        "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '{db_name}'"
    ))
    .execute(&cleanup_pool)
    .await
    .ok();
    sqlx::query(&format!("DROP DATABASE IF EXISTS \"{db_name}\""))
        .execute(&cleanup_pool)
        .await
        .expect("drop database");
    sqlx::query(&format!("DROP ROLE IF EXISTS \"{role_name}\""))
        .execute(&cleanup_pool)
        .await
        .expect("drop role");
    cleanup_pool.close().await;
}

#[tokio::test]
async fn server_exits_when_app_grants_are_missing() {
    let db = migrated_db_without_grants().await;
    let storage_root = std::env::temp_dir().join(format!("fvoci-schema-gate-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&storage_root).expect("storage root");
    let output = server_process_env(&db.app_url, &storage_root)
        .output()
        .expect("spawn fvoci-server");
    assert_schema_gate_process_failure(
        &output,
        &[
            "cannot read fvoci.schema_migrations",
            migrate::SCHEMA_GATE_OPERATOR_HINT,
        ],
    );
    let _ = std::fs::remove_dir_all(storage_root);
    drop_ungranted(db).await;
}
