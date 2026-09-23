#![cfg(feature = "db-tests")]

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{Duration as ChronoDuration, Utc};
use fvoci_server::auth::password::Keyring;
use fvoci_server::auth::token::hash_token;
use fvoci_server::auth::AuthService;
use fvoci_server::db::identity::{
    count_users, find_live_session, new_setup_input, setup_first_owner, SetupFirstOwnerResult,
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

struct TestDb {
    admin_url: String,
    app_url: String,
    db_name: String,
}

impl TestDb {
    async fn bootstrap() -> Self {
        let admin_base = std::env::var("TEST_DATABASE_URL")
            .or_else(|_| std::env::var("FVOCI_TEST_DATABASE_URL"))
            .expect("TEST_DATABASE_URL missing");
        if let Ok(app_url) = std::env::var("TEST_APP_DATABASE_URL") {
            let db_name = admin_base.rsplit('/').next().unwrap_or("test").to_string();
            return Self {
                admin_url: admin_base,
                app_url,
                db_name,
            };
        }

        let db_name = format!("fvoci_test_{}", Uuid::now_v7().simple());
        let role_name = format!("fvoci_app_{}", db_name.replace('-', "_"));
        let mut password_bytes = [0u8; 24];
        rand::rng().fill_bytes(&mut password_bytes);
        let role_password = hex::encode(password_bytes);
        let server_url = trim_db_url(&admin_base);

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

        let admin_url = format!("{}/{}", server_url, db_name);
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

        let quoted_role = format!("\"{}\"", role_name);
        let grants =
            include_str!("../scripts/grant-app-role.sql").replace(":\"app_role\"", &quoted_role);
        for statement in grants.split(';').map(str::trim).filter(|s| !s.is_empty()) {
            sqlx::query(statement)
                .execute(&migration_pool)
                .await
                .expect("grant");
        }
        migration_pool.close().await;

        let parsed = url::Url::parse(&admin_url).expect("database url");
        let app_url = {
            let mut app = parsed;
            app.set_username(&role_name).ok();
            app.set_password(Some(&role_password)).ok();
            app.to_string()
        };

        Self {
            admin_url,
            app_url,
            db_name,
        }
    }

    async fn cleanup(self) {
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(&trim_db_url(&self.admin_url))
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
            pool.close().await;
        }
    }
}

fn trim_db_url(url: &str) -> String {
    if let Some(idx) = url.rfind('/') {
        url[..idx].to_string()
    } else {
        url.to_string()
    }
}

async fn app_state(app_url: &str) -> AppState {
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
    }
}

async fn json_request(
    app: axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    cookie: Option<&str>,
    extra_headers: &[(&str, &str)],
) -> (StatusCode, Value, Option<String>) {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", format!("fvoci_session={}", cookie));
    }
    for (name, value) in extra_headers {
        builder = builder.header(*name, *value);
    }
    let request = if let Some(body) = body {
        builder
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    } else {
        builder.body(Body::empty()).unwrap()
    };
    let response = app.oneshot(request).await.expect("response");
    let status = response.status();
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
    (status, json, set_cookie)
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
    let app = router(app_state(&harness.app_url).await);
    let (_, _, cookie_hdr) = json_request(
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

    let (status, body, _) = json_request(
        app.clone(),
        "GET",
        "/api/v1/auth/me",
        None,
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["email"], "admin@example.com");
    assert_eq!(body["locale"], "ko");

    let (status, body, _) = json_request(
        app.clone(),
        "PATCH",
        "/api/v1/auth/me",
        Some(json!({"givenName": "Renamed", "familyName": null})),
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["givenName"], "Renamed");

    let (status, _, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/auth/logout",
        None,
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _, _) = json_request(
        app.clone(),
        "GET",
        "/api/v1/auth/me",
        None,
        Some(&cookie),
        &[],
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
    let input_a = new_setup_input(
        "a@example.com".into(),
        hash.clone(),
        "A".into(),
        None,
        "race-a".into(),
        "Race A".into(),
        token_a.hash,
        Utc::now() + ChronoDuration::days(30),
    );
    let input_b = new_setup_input(
        "b@example.com".into(),
        hash,
        "B".into(),
        None,
        "race-b".into(),
        "Race B".into(),
        token_b.hash,
        Utc::now() + ChronoDuration::days(30),
    );
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
    let (status, _, _) = json_request(
        app.clone(),
        "GET",
        "/api/v1/auth/me",
        None,
        Some(&cookie),
        &[],
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
    let input = new_setup_input(
        "owner@example.com".into(),
        hash,
        "Owner".into(),
        None,
        "owner".into(),
        "Owner".into(),
        token.hash,
        Utc::now() + ChronoDuration::days(30),
    );
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

    let (status, _, _) = json_request(
        app.clone(),
        "PATCH",
        "/api/v1/auth/me",
        Some(json!({"givenName": "Blocked"})),
        Some(&cookie),
        &[],
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

    let (status, _, _) = json_request(
        app.clone(),
        "PATCH",
        "/api/v1/auth/me",
        Some(json!({"givenName": "Blocked"})),
        Some(&cookie),
        &[],
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
    let (status, _, _) = json_request(
        app,
        "PATCH",
        "/api/v1/auth/me",
        Some(json!({"givenName": "Nope", "userId": other})),
        Some(&cookie),
        &[],
    )
    .await;
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::UNPROCESSABLE_ENTITY,
        "unexpected status {status}"
    );
    harness.cleanup().await;
}

#[tokio::test]
async fn patch_me_rejects_bearer_token_auth() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _) = setup_session(&harness).await;
    let (status, body, _) = json_request(
        app,
        "GET",
        "/api/v1/auth/me",
        None,
        Some(&cookie),
        &[("authorization", "Bearer deadbeef")],
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

    let (status, body, _) = json_request(
        app.clone(),
        "GET",
        "/api/v1/auth/me",
        None,
        Some(&cookie),
        &[],
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

    let (status, live_body, _) = json_request(
        app.clone(),
        "GET",
        "/api/v1/auth/me",
        None,
        Some(&cookie),
        &[],
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
            )
            .await
        }
    });

    wait_for_profile_patch_blocked(&admin, blocker_pid).await;
    admin_tx.commit().await.unwrap();

    let (patch_status, patch_body, _) = patch.await.unwrap();
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
    let app = router(app_state(&harness.app_url).await);
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
    )
    .await;

    let (status, body, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/auth/login",
        Some(json!({"email": "hash@example.com", "password": "supersecret1"})),
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.get("userId").is_some());

    let (status, _, _) = json_request(
        app,
        "POST",
        "/api/v1/auth/login",
        Some(json!({"email": "hash@example.com", "password": "wrong-password"})),
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

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
    assert_eq!(versions.0, 2);
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
