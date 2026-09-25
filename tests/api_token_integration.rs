#![cfg(feature = "db-tests")]

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use chrono::{Duration as ChronoDuration, Utc};
use fvoci_server::auth::password::Keyring;
use fvoci_server::auth::token::hash_token;
use fvoci_server::auth::AuthService;
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

    async fn admin(&self) -> PgPool {
        PgPoolOptions::new()
            .max_connections(4)
            .connect(&self.admin_url)
            .await
            .unwrap()
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
    let storage_root = std::env::temp_dir().join(format!("fvoci-pat-test-{}", Uuid::now_v7()));
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
        storage: fvoci_server::attachments::LocalStorage::new(storage_root).into(),
        upload: fvoci_server::attachments::UploadLimits {
            part_size_bytes: fvoci_server::config::DEFAULT_UPLOAD_PART_SIZE_BYTES,
            max_file_size_bytes: fvoci_server::config::DEFAULT_UPLOAD_MAX_FILE_SIZE_BYTES,
            create_rate_per_5min: fvoci_server::config::DEFAULT_UPLOAD_CREATE_RATE_PER_5MIN,
        },
        collab: None,
        meili: None,
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
        builder = builder.header("cookie", format!("fvoci_session={cookie}"));
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
    let admin = harness.admin().await;
    let user_id: (Uuid,) = sqlx::query_as("SELECT id FROM fvoci.users LIMIT 1")
        .fetch_one(&admin)
        .await
        .unwrap();
    admin.close().await;
    (app, cookie, user_id.0)
}

async fn acme_id(admin: &PgPool) -> Uuid {
    sqlx::query_scalar("SELECT id FROM fvoci.workspaces WHERE slug = 'acme'")
        .fetch_one(admin)
        .await
        .unwrap()
}

async fn create_second_user_session(
    harness: &TestDb,
    email: &str,
    given_name: &str,
) -> (Uuid, String) {
    let admin = harness.admin().await;
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

async fn add_membership(admin: &PgPool, workspace_id: Uuid, user_id: Uuid, role: &str) {
    sqlx::query("INSERT INTO fvoci.memberships (workspace_id, user_id, role) VALUES ($1, $2, $3)")
        .bind(workspace_id)
        .bind(user_id)
        .bind(role)
        .execute(admin)
        .await
        .unwrap();
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

async fn create_token(
    app: axum::Router,
    cookie: &str,
    workspace_id: Uuid,
    name: &str,
    scopes: &[&str],
    extra: Value,
) -> (StatusCode, Value) {
    let mut body = json!({
        "name": name,
        "scopes": scopes,
    });
    if let Some(obj) = extra.as_object() {
        for (key, value) in obj {
            body[key] = value.clone();
        }
    }
    let (status, json, _, _) = json_request(
        app,
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/api-tokens"),
        Some(body),
        Some(cookie),
        &[],
        None,
    )
    .await;
    (status, json)
}

async fn bearer_get(app: axum::Router, path: &str, secret: &str) -> (StatusCode, Value) {
    let auth = format!("Bearer {secret}");
    let (status, json, _, _) = json_request(
        app,
        "GET",
        path,
        None,
        None,
        &[("authorization", auth.as_str())],
        None,
    )
    .await;
    (status, json)
}

async fn wait_for_user_for_update_blocked(admin: &PgPool, blocker_pid: i32) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
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

#[tokio::test]
async fn create_list_revoke_hides_secret_and_stores_hash() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id) = setup_session(&harness).await;
    let admin = harness.admin().await;
    let ws = acme_id(&admin).await;

    let (status, created) = create_token(
        app.clone(),
        &cookie,
        ws,
        "CI",
        &["documents.read", "documents.write"],
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let secret = created["token"].as_str().expect("secret once").to_string();
    assert!(secret.len() > 16);
    assert_eq!(created["name"], "CI");
    assert_eq!(created["userId"], owner_id.to_string());
    assert!(created["expiresAt"].is_string());
    assert!(!created.as_object().unwrap().contains_key("lastUsedAt"));

    let stored: (String, Option<chrono::DateTime<Utc>>) =
        sqlx::query_as("SELECT token_hash, last_used_at FROM fvoci.api_tokens WHERE id = $1")
            .bind(created["id"].as_str().unwrap().parse::<Uuid>().unwrap())
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(stored.0, hash_token(&secret));
    assert_ne!(stored.0, secret);
    assert!(stored.1.is_none());

    let events: (i64,) =
        sqlx::query_as("SELECT count(*) FROM fvoci.events WHERE verb = 'api_token.created'")
            .fetch_one(&admin)
            .await
            .unwrap();
    let audit: (i64,) =
        sqlx::query_as("SELECT count(*) FROM fvoci.audit_log WHERE verb = 'api_token.created'")
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(events.0, 1);
    assert_eq!(audit.0, 1);

    let (status, listed, _, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{ws}/api-tokens"),
        None,
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items = listed["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert!(items[0].get("token").is_none());
    assert_eq!(items[0]["id"], created["id"]);

    let (status, body) =
        bearer_get(app.clone(), &format!("/api/v1/workspaces/{ws}"), &secret).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["slug"], "acme");
    let last_used: Option<chrono::DateTime<Utc>> =
        sqlx::query_scalar("SELECT last_used_at FROM fvoci.api_tokens WHERE token_hash = $1")
            .bind(hash_token(&secret))
            .fetch_one(&admin)
            .await
            .unwrap();
    assert!(last_used.is_some());

    let (status, revoked, _, _) = json_request(
        app.clone(),
        "DELETE",
        &format!(
            "/api/v1/workspaces/{ws}/api-tokens/{}",
            created["id"].as_str().unwrap()
        ),
        None,
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(revoked["ok"], true);

    let (status, body) =
        bearer_get(app.clone(), &format!("/api/v1/workspaces/{ws}"), &secret).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], "authentication_required");

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn token_auth_allows_scoped_routes_and_rejects_session_only() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _) = setup_session(&harness).await;
    let admin = harness.admin().await;
    let ws = acme_id(&admin).await;

    let (status, created) = create_token(
        app.clone(),
        &cookie,
        ws,
        "docs",
        &["documents.read"],
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let secret = created["token"].as_str().unwrap().to_string();
    let auth = format!("Bearer {secret}");

    let (status, _) = bearer_get(app.clone(), &format!("/api/v1/workspaces/{ws}"), &secret).await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = bearer_get(app.clone(), "/api/v1/auth/me", &secret).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");

    let (status, body) = bearer_get(app.clone(), "/api/v1/me/workspaces", &secret).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");

    let (status, body) = bearer_get(app.clone(), "/api/v1/me/api-tokens", &secret).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");

    let (status, body, _, _) = json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{ws}"),
        Some(json!({ "name": "Nope" })),
        None,
        &[("authorization", auth.as_str())],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");

    let (status, tree) = bearer_get(
        app.clone(),
        &format!("/api/v1/workspaces/{ws}/tree"),
        &secret,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(tree["items"].as_array().is_some());
    let (status, body) = bearer_get(
        app.clone(),
        &format!("/api/v1/workspaces/{ws}/projects"),
        &secret,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");

    let other = Uuid::now_v7();
    let (status, body) =
        bearer_get(app.clone(), &format!("/api/v1/workspaces/{other}"), &secret).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn expiry_service_origin_cookie_precedence_and_rate_limit() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _) = setup_session(&harness).await;
    let admin = harness.admin().await;
    let ws = acme_id(&admin).await;

    let (status, unlimited) = create_token(
        app.clone(),
        &cookie,
        ws,
        "forever",
        &["workspace.manage"],
        json!({ "unlimited": true }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(unlimited["expiresAt"].is_null());
    let manage_secret = unlimited["token"].as_str().unwrap().to_string();
    let (status, _) = bearer_get(
        app.clone(),
        &format!("/api/v1/workspaces/{ws}"),
        &manage_secret,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, service) = create_token(
        app.clone(),
        &cookie,
        ws,
        "bot",
        &["documents.read"],
        json!({ "service": true }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(service["userId"].is_null());
    let service_secret = service["token"].as_str().unwrap().to_string();
    let (status, body) = bearer_get(
        app.clone(),
        &format!("/api/v1/workspaces/{ws}"),
        &service_secret,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], "authentication_required");

    let (status, expiring) = create_token(
        app.clone(),
        &cookie,
        ws,
        "soon",
        &["documents.read"],
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let expiring_secret = expiring["token"].as_str().unwrap().to_string();
    sqlx::query(
        "UPDATE fvoci.api_tokens SET expires_at = now() - interval '1 second' WHERE id = $1",
    )
    .bind(expiring["id"].as_str().unwrap().parse::<Uuid>().unwrap())
    .execute(&admin)
    .await
    .unwrap();
    let (status, body) = bearer_get(
        app.clone(),
        &format!("/api/v1/workspaces/{ws}"),
        &expiring_secret,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], "authentication_required");

    let (status, body, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/api-tokens"),
        Some(json!({ "name": "origin", "scopes": ["documents.read"] })),
        Some(&cookie),
        &[("origin", "http://evil.example.com")],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "origin_mismatch");

    let (status, bogus) = bearer_get(
        app.clone(),
        &format!("/api/v1/workspaces/{ws}"),
        "not-a-real-token",
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(bogus["code"], "authentication_required");

    let auth = format!("Bearer {manage_secret}");
    let (status, body, _, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{ws}"),
        None,
        Some("dead-cookie"),
        &[("authorization", auth.as_str())],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], "authentication_required");

    let mut last = StatusCode::CREATED;
    for i in 0..21 {
        let (status, _) = create_token(
            app.clone(),
            &cookie,
            ws,
            &format!("rate-{i}"),
            &["documents.read"],
            json!({}),
        )
        .await;
        last = status;
        if status == StatusCode::TOO_MANY_REQUESTS {
            break;
        }
    }
    assert_eq!(last, StatusCode::TOO_MANY_REQUESTS);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn suspended_deleted_and_removed_member_tokens_stop() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _) = setup_session(&harness).await;
    let admin = harness.admin().await;
    let ws = acme_id(&admin).await;

    let (member_id, member_cookie) =
        create_second_user_session(&harness, "admin2@example.com", "Second").await;
    add_membership(&admin, ws, member_id, "admin").await;
    let (status, created) = create_token(
        app.clone(),
        &member_cookie,
        ws,
        "member-pat",
        &["documents.read", "workspace.manage"],
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let secret = created["token"].as_str().unwrap().to_string();
    let (status, _) = bearer_get(app.clone(), &format!("/api/v1/workspaces/{ws}"), &secret).await;
    assert_eq!(status, StatusCode::OK);

    sqlx::query("UPDATE fvoci.users SET suspended_at = now() WHERE id = $1")
        .bind(member_id)
        .execute(&admin)
        .await
        .unwrap();
    let (status, body) =
        bearer_get(app.clone(), &format!("/api/v1/workspaces/{ws}"), &secret).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], "authentication_required");

    sqlx::query("UPDATE fvoci.users SET suspended_at = NULL WHERE id = $1")
        .bind(member_id)
        .execute(&admin)
        .await
        .unwrap();
    let (status, _) = bearer_get(app.clone(), &format!("/api/v1/workspaces/{ws}"), &secret).await;
    assert_eq!(status, StatusCode::OK);

    let (status, _, _, _) = json_request(
        app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{ws}/members/{member_id}"),
        None,
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let remaining: (i64,) =
        sqlx::query_as("SELECT count(*) FROM fvoci.api_tokens WHERE token_hash = $1")
            .bind(hash_token(&secret))
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(remaining.0, 0);
    let (status, body) =
        bearer_get(app.clone(), &format!("/api/v1/workspaces/{ws}"), &secret).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], "authentication_required");

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn holder_scoped_barriers_stop_token_writes() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id) = setup_session(&harness).await;
    let admin = harness.admin().await;
    let ws = acme_id(&admin).await;
    let (status, created) = create_token(
        app.clone(),
        &cookie,
        ws,
        "barrier",
        &["workspace.manage"],
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let secret = created["token"].as_str().unwrap().to_string();
    let events_before: (i64,) = sqlx::query_as("SELECT count(*) FROM fvoci.events")
        .fetch_one(&admin)
        .await
        .unwrap();

    let mut barrier = admin.begin().await.unwrap();
    sqlx::query("SELECT id FROM fvoci.users WHERE id = $1 FOR UPDATE")
        .bind(owner_id)
        .fetch_one(&mut *barrier)
        .await
        .unwrap();
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *barrier)
        .await
        .unwrap();

    let app_bg = app.clone();
    let auth = format!("Bearer {secret}");
    let patch_path = format!("/api/v1/workspaces/{ws}");
    let patch_task = tokio::spawn(async move {
        json_request(
            app_bg,
            "PATCH",
            &patch_path,
            Some(json!({ "name": "Blocked" })),
            None,
            &[("authorization", auth.as_str())],
            None,
        )
        .await
    });
    wait_for_user_for_update_blocked(&admin, blocker_pid).await;
    sqlx::query("UPDATE fvoci.users SET suspended_at = now() WHERE id = $1")
        .bind(owner_id)
        .execute(&mut *barrier)
        .await
        .unwrap();
    barrier.commit().await.unwrap();

    let (status, body, _, _) = tokio::time::timeout(Duration::from_secs(10), patch_task)
        .await
        .expect("patch finished")
        .expect("join");
    assert!(
        status == StatusCode::UNAUTHORIZED || status == StatusCode::NOT_FOUND,
        "status={status} body={body}"
    );
    let events_after: (i64,) = sqlx::query_as("SELECT count(*) FROM fvoci.events")
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(events_after.0, events_before.0);

    sqlx::query("UPDATE fvoci.users SET suspended_at = NULL WHERE id = $1")
        .bind(owner_id)
        .execute(&admin)
        .await
        .unwrap();

    let (member_id, member_cookie) =
        create_second_user_session(&harness, "gone@example.com", "Gone").await;
    add_membership(&admin, ws, member_id, "admin").await;
    let (status, member_token) = create_token(
        app.clone(),
        &member_cookie,
        ws,
        "gone-pat",
        &["workspace.manage"],
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let member_secret = member_token["token"].as_str().unwrap().to_string();

    let mut remove_barrier = admin.begin().await.unwrap();
    sqlx::query("SELECT id FROM fvoci.users WHERE id = $1 FOR UPDATE")
        .bind(member_id)
        .fetch_one(&mut *remove_barrier)
        .await
        .unwrap();
    let remove_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *remove_barrier)
        .await
        .unwrap();
    let app_rm = app.clone();
    let auth_rm = format!("Bearer {member_secret}");
    let patch_path = format!("/api/v1/workspaces/{ws}");
    let remove_patch = tokio::spawn(async move {
        json_request(
            app_rm,
            "PATCH",
            &patch_path,
            Some(json!({ "name": "Removed" })),
            None,
            &[("authorization", auth_rm.as_str())],
            None,
        )
        .await
    });
    wait_for_user_for_update_blocked(&admin, remove_pid).await;
    sqlx::query("DELETE FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2")
        .bind(ws)
        .bind(member_id)
        .execute(&mut *remove_barrier)
        .await
        .unwrap();
    remove_barrier.commit().await.unwrap();
    let (status, body, _, _) = tokio::time::timeout(Duration::from_secs(10), remove_patch)
        .await
        .expect("remove patch finished")
        .expect("join");
    assert!(
        status == StatusCode::UNAUTHORIZED || status == StatusCode::NOT_FOUND,
        "status={status} body={body}"
    );

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn create_rolls_back_when_event_or_audit_insert_fails() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _) = setup_session(&harness).await;
    let admin = harness.admin().await;
    let ws = acme_id(&admin).await;
    let before: (i64,) = sqlx::query_as("SELECT count(*) FROM fvoci.api_tokens")
        .fetch_one(&admin)
        .await
        .unwrap();

    install_insert_fail_trigger(&admin, "events", "block_pat_events").await;
    let (status, _) = create_token(
        app.clone(),
        &cookie,
        ws,
        "blocked-event",
        &["documents.read"],
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    sqlx::query("DROP TRIGGER fvoci_block_pat_events ON fvoci.events")
        .execute(&admin)
        .await
        .unwrap();

    install_insert_fail_trigger(&admin, "audit_log", "block_pat_audit").await;
    let (status, _) = create_token(
        app.clone(),
        &cookie,
        ws,
        "blocked-audit",
        &["documents.read"],
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);

    let after: (i64,) = sqlx::query_as("SELECT count(*) FROM fvoci.api_tokens")
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(after.0, before.0);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn me_routes_are_session_only_and_member_cannot_manage_tokens() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _) = setup_session(&harness).await;
    let admin = harness.admin().await;
    let ws = acme_id(&admin).await;
    let (status, _created) = create_token(
        app.clone(),
        &cookie,
        ws,
        "mine",
        &["documents.read"],
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, listed, _, _) = json_request(
        app.clone(),
        "GET",
        "/api/v1/me/api-tokens",
        None,
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["items"].as_array().unwrap().len(), 1);
    assert!(listed["items"][0].get("token").is_none());

    let (member_id, member_cookie) =
        create_second_user_session(&harness, "member@example.com", "Member").await;
    add_membership(&admin, ws, member_id, "member").await;
    let (status, body) = create_token(
        app.clone(),
        &member_cookie,
        ws,
        "nope",
        &["documents.read"],
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn project_workflow_requires_tasks_read_scope_like_the_source() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _) = setup_session(&harness).await;
    let admin = harness.admin().await;
    let ws = acme_id(&admin).await;
    let (status, project, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/projects"),
        Some(json!({"key": "WFS", "name": "Workflow scope", "visibility": "workspace"})),
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{project}");
    let project_id = project["id"].as_str().unwrap().to_string();
    let path = format!("/api/v1/workspaces/{ws}/projects/{project_id}/workflow");

    let (_, projects_only) = create_token(
        app.clone(),
        &cookie,
        ws,
        "projects",
        &["projects.read"],
        json!({}),
    )
    .await;
    let (status, _) =
        bearer_get(app.clone(), &path, projects_only["token"].as_str().unwrap()).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "workflow is a tasks.read resource"
    );

    let (_, tasks_read) = create_token(
        app.clone(),
        &cookie,
        ws,
        "tasks",
        &["tasks.read"],
        json!({}),
    )
    .await;
    let (status, body) =
        bearer_get(app.clone(), &path, tasks_read["token"].as_str().unwrap()).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    admin.close().await;
    harness.cleanup().await;
}
