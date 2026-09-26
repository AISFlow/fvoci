#![cfg(feature = "db-tests")]

use std::sync::Arc;

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
            .max_connections(2)
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
    let storage_root = std::env::temp_dir().join(format!("fvoci-invite-test-{}", Uuid::now_v7()));
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
            part_put_slots: fvoci_server::attachments::PartPutSlots::new(
                fvoci_server::config::DEFAULT_UPLOAD_MAX_CONCURRENT_PARTS,
            ),
        },
        collab: None,
        meili: None,
        search_embedder: None,
        document_convert: None,
        import_wake: None,
        import_extractor_available: false,
        quota: Default::default(),
        mailer: std::sync::Arc::new(fvoci_server::mail::Mailer::disabled()),
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

fn invite_token(accept_url: &str) -> String {
    accept_url
        .split("/invite/")
        .nth(1)
        .expect("acceptUrl token")
        .to_string()
}

async fn create_invite(
    app: axum::Router,
    cookie: &str,
    workspace_id: Uuid,
    email: &str,
    role: &str,
) -> (StatusCode, Value) {
    let (status, body, _, _) = json_request(
        app,
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/invitations"),
        Some(json!({ "email": email, "role": role })),
        Some(cookie),
        &[],
        None,
    )
    .await;
    (status, body)
}

#[tokio::test]
async fn members_list_and_denial_matrix() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id) = setup_session(&harness).await;
    let admin = harness.admin().await;
    let ws = acme_id(&admin).await;

    let (status, body, _, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{ws}/members"),
        None,
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["userId"], owner_id.to_string());
    assert_eq!(items[0]["role"], "owner");
    assert_eq!(items[0]["email"], "admin@example.com");

    let (status, body, _, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{ws}/members"),
        None,
        None,
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], "authentication_required");

    let (member_id, member_cookie) =
        create_second_user_session(&harness, "member@example.com", "Member").await;
    add_membership(&admin, ws, member_id, "member").await;
    let (status, body, _, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{ws}/members"),
        None,
        Some(&member_cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["items"].as_array().unwrap().len(), 2);

    let (guest_id, guest_cookie) =
        create_second_user_session(&harness, "guest@example.com", "Guest").await;
    add_membership(&admin, ws, guest_id, "guest").await;
    let (status, body, _, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{ws}/members"),
        None,
        Some(&guest_cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");

    let (_other_id, other_cookie) =
        create_second_user_session(&harness, "other@example.com", "Other").await;
    let (status, body, _, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{ws}/members"),
        None,
        Some(&other_cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");

    sqlx::query("UPDATE fvoci.sessions SET revoked_at = now() WHERE user_id = $1")
        .bind(owner_id)
        .execute(&admin)
        .await
        .unwrap();
    let (status, body, _, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{ws}/members"),
        None,
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], "authentication_required");

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn create_invitation_enforces_roles_and_hashes_token() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id) = setup_session(&harness).await;
    let admin = harness.admin().await;
    let ws = acme_id(&admin).await;

    let (status, body) = create_invite(app.clone(), &cookie, ws, "new@example.com", "member").await;
    assert_eq!(status, StatusCode::CREATED);
    let accept_url = body["acceptUrl"].as_str().unwrap();
    assert!(accept_url.starts_with("http://localhost/invite/"));
    let token = invite_token(accept_url);
    let stored: (String, String) =
        sqlx::query_as("SELECT token_hash, email FROM fvoci.invitations")
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(stored.0, hash_token(&token));
    assert_eq!(stored.1, "new@example.com");
    let leaked: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.invitations WHERE token_hash = $1 OR email = $1",
    )
    .bind(&token)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(leaked.0, 0);

    let (status, body) =
        create_invite(app.clone(), &cookie, ws, "Admin@example.com", "owner").await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let (admin_id, admin_cookie) =
        create_second_user_session(&harness, "ws-admin@example.com", "WsAdmin").await;
    add_membership(&admin, ws, admin_id, "admin").await;
    let (status, body) = create_invite(
        app.clone(),
        &admin_cookie,
        ws,
        "via-admin@example.com",
        "member",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (status, body) = create_invite(
        app.clone(),
        &admin_cookie,
        ws,
        "too-high@example.com",
        "owner",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "cannot_invite_a_role_above_your_own");

    let (member_id, member_cookie) =
        create_second_user_session(&harness, "plain-member@example.com", "Plain").await;
    add_membership(&admin, ws, member_id, "member").await;
    let (status, body) = create_invite(
        app.clone(),
        &member_cookie,
        ws,
        "nope@example.com",
        "member",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "insufficient_permissions");

    let (guest_id, guest_cookie) =
        create_second_user_session(&harness, "invite-guest@example.com", "G").await;
    add_membership(&admin, ws, guest_id, "guest").await;
    let (status, _) =
        create_invite(app.clone(), &guest_cookie, ws, "nope2@example.com", "guest").await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (_other_id, other_cookie) =
        create_second_user_session(&harness, "outsider@example.com", "Out").await;
    let (status, body) =
        create_invite(app.clone(), &other_cookie, ws, "x@example.com", "member").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");

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
    assert_eq!(status, StatusCode::OK, "{body}");
    let personal_id = body["id"].as_str().unwrap();
    let (status, body, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{personal_id}/invitations"),
        Some(json!({ "email": "p@example.com", "role": "member" })),
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "personal_workspace_is_immutable");

    sqlx::query("UPDATE fvoci.sessions SET revoked_at = now() WHERE user_id = $1")
        .bind(owner_id)
        .execute(&admin)
        .await
        .unwrap();
    let (status, body) =
        create_invite(app.clone(), &cookie, ws, "late@example.com", "member").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], "authentication_required");

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn accept_invitation_happy_path_and_token_misuse() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _) = setup_session(&harness).await;
    let admin = harness.admin().await;
    let ws = acme_id(&admin).await;

    let (status, body) =
        create_invite(app.clone(), &cookie, ws, "join@example.com", "member").await;
    assert_eq!(status, StatusCode::CREATED);
    let token = invite_token(body["acceptUrl"].as_str().unwrap());

    let (status, body, _, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/invitations/{token}"),
        None,
        None,
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["workspaceName"], "Acme");
    assert_eq!(body["emailMasked"], "j***@example.com");
    assert_eq!(body["role"], "member");
    assert_eq!(body["requiredLegal"].as_array().unwrap().len(), 0);

    let (status, body, _, _) = json_request(
        app.clone(),
        "GET",
        "/api/v1/invitations/not-a-real-token",
        None,
        None,
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "invitation_not_found_or_expired");

    let (status, body, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/invitations/{token}/accept"),
        Some(json!({
            "email": "other@example.com",
            "givenName": "Wrong",
            "password": "supersecret1"
        })),
        None,
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], "cannot_accept_invitation");

    let (status, body, set_cookie, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/invitations/{token}/accept"),
        Some(json!({
            "email": "join@example.com",
            "givenName": "Join",
            "familyName": "Kim",
            "password": "supersecret1"
        })),
        None,
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["userId"].as_str().is_some());
    let joined_cookie = extract_session_cookie(set_cookie.as_ref().unwrap());
    let (status, body, _, _) = json_request(
        app.clone(),
        "GET",
        "/api/v1/me/workspaces",
        None,
        Some(&joined_cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["slug"] == "acme" && item["role"] == "member"));

    let (status, body, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/invitations/{token}/accept"),
        Some(json!({
            "email": "join@example.com",
            "givenName": "Join",
            "password": "supersecret1"
        })),
        None,
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::GONE);
    assert_eq!(body["code"], "already_accepted");

    let (status, body) =
        create_invite(app.clone(), &cookie, ws, "expire@example.com", "guest").await;
    assert_eq!(status, StatusCode::CREATED);
    let expire_token = invite_token(body["acceptUrl"].as_str().unwrap());
    let app_pool = pool::connect_app(&harness.app_url).await.unwrap();
    fvoci_server::db::invitations::expire_invitation_for_test(
        &app_pool,
        ws,
        &hash_token(&expire_token),
    )
    .await
    .unwrap();
    let (status, body, _, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/invitations/{expire_token}"),
        None,
        None,
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "invitation_not_found_or_expired");
    let (status, body, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/invitations/{expire_token}/accept"),
        Some(json!({
            "email": "expire@example.com",
            "givenName": "Expire",
            "password": "supersecret1"
        })),
        None,
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::GONE);
    assert_eq!(body["code"], "expired");

    let (status, body) =
        create_invite(app.clone(), &cookie, ws, "revoked@example.com", "member").await;
    assert_eq!(status, StatusCode::CREATED);
    let revoked_token = invite_token(body["acceptUrl"].as_str().unwrap());
    fvoci_server::db::invitations::revoke_pending_invitation_for_test(
        &app_pool,
        ws,
        &hash_token(&revoked_token),
    )
    .await
    .unwrap();
    app_pool.close().await;
    let (status, body, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/invitations/{revoked_token}/accept"),
        Some(json!({
            "email": "revoked@example.com",
            "givenName": "Revoked",
            "password": "supersecret1"
        })),
        None,
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");

    let (existing_id, _) =
        create_second_user_session(&harness, "existing@example.com", "Existing").await;
    let (status, body) =
        create_invite(app.clone(), &cookie, ws, "existing@example.com", "admin").await;
    assert_eq!(status, StatusCode::CREATED);
    let existing_token = invite_token(body["acceptUrl"].as_str().unwrap());
    let (status, body, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/invitations/{existing_token}/accept"),
        Some(json!({
            "email": "existing@example.com",
            "password": "wrong-password-x"
        })),
        None,
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], "cannot_accept_invitation");

    sqlx::query("UPDATE fvoci.users SET suspended_at = now() WHERE id = $1")
        .bind(existing_id)
        .execute(&admin)
        .await
        .unwrap();
    let (status, body, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/invitations/{existing_token}/accept"),
        Some(json!({
            "email": "existing@example.com",
            "password": "supersecret1"
        })),
        None,
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], "cannot_accept_invitation");

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn concurrent_double_accept_creates_one_membership() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _) = setup_session(&harness).await;
    let admin = harness.admin().await;
    let ws = acme_id(&admin).await;
    let (status, body) =
        create_invite(app.clone(), &cookie, ws, "race@example.com", "member").await;
    assert_eq!(status, StatusCode::CREATED);
    let token = invite_token(body["acceptUrl"].as_str().unwrap());
    let path = format!("/api/v1/invitations/{token}/accept");
    let payload = json!({
        "email": "race@example.com",
        "givenName": "Race",
        "password": "supersecret1"
    });
    let (a, b) = tokio::join!(
        json_request(
            app.clone(),
            "POST",
            &path,
            Some(payload.clone()),
            None,
            &[],
            None,
        ),
        json_request(
            app.clone(),
            "POST",
            &path,
            Some(payload),
            None,
            &[],
            Some(std::net::SocketAddr::from(([203, 0, 113, 11], 42424))),
        )
    );
    let statuses = [a.0, b.0];
    assert!(
        statuses.contains(&StatusCode::OK) && statuses.contains(&StatusCode::GONE),
        "expected one 200 and one 410, got {statuses:?} bodies {:?} {:?}",
        a.1,
        b.1
    );
    let gone = if a.0 == StatusCode::GONE { &a.1 } else { &b.1 };
    assert_eq!(gone["code"], "already_accepted");
    let members: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.memberships WHERE workspace_id = $1 AND role = 'member'",
    )
    .bind(ws)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(members.0, 1);
    let users: (i64,) =
        sqlx::query_as("SELECT count(*) FROM fvoci.users WHERE email = 'race@example.com'")
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(users.0, 1);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn accept_rolls_back_when_event_or_audit_insert_fails() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _) = setup_session(&harness).await;
    let admin = harness.admin().await;
    let ws = acme_id(&admin).await;

    let (status, body) =
        create_invite(app.clone(), &cookie, ws, "event@example.com", "member").await;
    assert_eq!(status, StatusCode::CREATED);
    let token = invite_token(body["acceptUrl"].as_str().unwrap());
    install_insert_fail_trigger(&admin, "events", "block_invite_events").await;
    let (status, _, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/invitations/{token}/accept"),
        Some(json!({
            "email": "event@example.com",
            "givenName": "Event",
            "password": "supersecret1"
        })),
        None,
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    let accepted: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.invitations WHERE email = 'event@example.com' AND accepted_at IS NOT NULL",
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(accepted.0, 0);
    let extra_users: (i64,) =
        sqlx::query_as("SELECT count(*) FROM fvoci.users WHERE email = 'event@example.com'")
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(extra_users.0, 0);
    sqlx::query("DROP TRIGGER fvoci_block_invite_events ON fvoci.events")
        .execute(&admin)
        .await
        .unwrap();

    let (status, body) =
        create_invite(app.clone(), &cookie, ws, "audit@example.com", "member").await;
    assert_eq!(status, StatusCode::CREATED);
    let token = invite_token(body["acceptUrl"].as_str().unwrap());
    install_insert_fail_trigger(&admin, "audit_log", "block_invite_audit").await;
    let (status, _, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/invitations/{token}/accept"),
        Some(json!({
            "email": "audit@example.com",
            "givenName": "Audit",
            "password": "supersecret1"
        })),
        None,
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    let accepted: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.invitations WHERE email = 'audit@example.com' AND accepted_at IS NOT NULL",
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(accepted.0, 0);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn accept_enforces_instance_seat_quota_but_allows_guests() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _) = setup_session(&harness).await;
    let admin = harness.admin().await;
    let ws = acme_id(&admin).await;
    for i in 0..9 {
        sqlx::query(
            "INSERT INTO fvoci.users (id, email, given_name, is_instance_admin) VALUES ($1, $2, $3, true)",
        )
        .bind(Uuid::now_v7())
        .bind(format!("seat{i}@example.com"))
        .bind("Seat")
        .execute(&admin)
        .await
        .unwrap();
    }

    let (status, body) =
        create_invite(app.clone(), &cookie, ws, "eleventh@example.com", "member").await;
    assert_eq!(status, StatusCode::CREATED);
    let token = invite_token(body["acceptUrl"].as_str().unwrap());
    let (status, body, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/invitations/{token}/accept"),
        Some(json!({
            "email": "eleventh@example.com",
            "givenName": "Eleven",
            "password": "supersecret1"
        })),
        None,
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{body}");
    assert_eq!(body["code"], "limit.seats");

    let (status, body) =
        create_invite(app.clone(), &cookie, ws, "guest-ok@example.com", "guest").await;
    assert_eq!(status, StatusCode::CREATED);
    let token = invite_token(body["acceptUrl"].as_str().unwrap());
    let (status, body, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/invitations/{token}/accept"),
        Some(json!({
            "email": "guest-ok@example.com",
            "givenName": "GuestOk",
            "password": "supersecret1"
        })),
        None,
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn demote_or_remove_inviter_drops_pending_tokens_and_workspace_delete_blocks_accept() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _) = setup_session(&harness).await;
    let admin = harness.admin().await;
    let ws = acme_id(&admin).await;

    let (admin_id, admin_cookie) =
        create_second_user_session(&harness, "demote-admin@example.com", "Demote").await;
    add_membership(&admin, ws, admin_id, "admin").await;
    let (status, body) = create_invite(
        app.clone(),
        &admin_cookie,
        ws,
        "stale@example.com",
        "member",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let stale_token = invite_token(body["acceptUrl"].as_str().unwrap());
    let (status, _, _, _) = json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{ws}/members/{admin_id}"),
        Some(json!({ "role": "member" })),
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body, _, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/invitations/{stale_token}"),
        None,
        None,
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "invitation_not_found_or_expired");

    let (status, body) =
        create_invite(app.clone(), &cookie, ws, "deleted-ws@example.com", "member").await;
    assert_eq!(status, StatusCode::CREATED);
    let delete_token = invite_token(body["acceptUrl"].as_str().unwrap());
    sqlx::query("UPDATE fvoci.workspaces SET deleted_at = now() WHERE id = $1")
        .bind(ws)
        .execute(&admin)
        .await
        .unwrap();
    let (status, body, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/invitations/{delete_token}/accept"),
        Some(json!({
            "email": "deleted-ws@example.com",
            "givenName": "Gone",
            "password": "supersecret1"
        })),
        None,
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn create_invitation_rolls_back_when_event_insert_fails() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _) = setup_session(&harness).await;
    let admin = harness.admin().await;
    let ws = acme_id(&admin).await;
    install_insert_fail_trigger(&admin, "events", "block_create_invite_events").await;
    let (status, _, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/invitations"),
        Some(json!({ "email": "blocked@example.com", "role": "member" })),
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    let rows: (i64,) = sqlx::query_as("SELECT count(*) FROM fvoci.invitations")
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(rows.0, 0);
    admin.close().await;
    harness.cleanup().await;
}

async fn seed_extra_instance_admins(admin: &PgPool, count: usize) {
    for i in 0..count {
        sqlx::query(
            "INSERT INTO fvoci.users (id, email, given_name, is_instance_admin) VALUES ($1, $2, $3, true)",
        )
        .bind(Uuid::now_v7())
        .bind(format!("seat-extra-{i}@example.com"))
        .bind("Seat")
        .execute(admin)
        .await
        .unwrap();
    }
}

async fn insert_workspace_guest(admin: &PgPool, workspace_id: Uuid, email: &str) -> Uuid {
    let user_id = Uuid::now_v7();
    sqlx::query("INSERT INTO fvoci.users (id, email, given_name) VALUES ($1, $2, $3)")
        .bind(user_id)
        .bind(email)
        .bind("Guest")
        .execute(admin)
        .await
        .unwrap();
    add_membership(admin, workspace_id, user_id, "guest").await;
    user_id
}

async fn billable_user_count(admin: &PgPool) -> i32 {
    let mut tx = admin.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.system_ctx', 'on', true)")
        .execute(&mut *tx)
        .await
        .unwrap();
    let count: i32 = sqlx::query_scalar("SELECT fvoci.app_quota_billable_users(NULL::uuid)")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    count
}

#[tokio::test]
async fn guest_promotion_is_rejected_at_instance_seat_limit() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _) = setup_session(&harness).await;
    let admin = harness.admin().await;
    let ws = acme_id(&admin).await;
    seed_extra_instance_admins(&admin, 9).await;
    let guest_id = insert_workspace_guest(&admin, ws, "promote-guest@example.com").await;
    assert_eq!(billable_user_count(&admin).await, 10);

    let (status, body, _, _) = json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{ws}/members/{guest_id}"),
        Some(json!({ "role": "member" })),
        Some(&cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{body}");
    assert_eq!(body["code"], "limit.seats");
    assert_eq!(billable_user_count(&admin).await, 10);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn concurrent_guest_promotions_have_single_winner_for_last_seat() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _) = setup_session(&harness).await;
    let admin = harness.admin().await;
    let ws = acme_id(&admin).await;
    seed_extra_instance_admins(&admin, 8).await;
    let first = insert_workspace_guest(&admin, ws, "race-a@example.com").await;
    let second = insert_workspace_guest(&admin, ws, "race-b@example.com").await;
    assert_eq!(billable_user_count(&admin).await, 9);

    let left_path = format!("/api/v1/workspaces/{ws}/members/{first}");
    let right_path = format!("/api/v1/workspaces/{ws}/members/{second}");
    let (left, right) = tokio::join!(
        json_request(
            app.clone(),
            "PATCH",
            &left_path,
            Some(json!({ "role": "member" })),
            Some(&cookie),
            &[],
            None,
        ),
        json_request(
            app,
            "PATCH",
            &right_path,
            Some(json!({ "role": "member" })),
            Some(&cookie),
            &[],
            None,
        ),
    );
    let statuses = [left.0, right.0];
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == StatusCode::OK)
            .count(),
        1,
        "{left:?} {right:?}"
    );
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == StatusCode::PAYMENT_REQUIRED)
            .count(),
        1,
        "{left:?} {right:?}"
    );
    let loser = if left.0 == StatusCode::PAYMENT_REQUIRED {
        &left.1
    } else {
        &right.1
    };
    assert_eq!(loser["code"], "limit.seats");
    assert_eq!(billable_user_count(&admin).await, 10);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn personal_workspace_creation_is_rejected_at_instance_seat_limit() {
    let harness = TestDb::bootstrap().await;
    let (app, _, _) = setup_session(&harness).await;
    let admin = harness.admin().await;
    seed_extra_instance_admins(&admin, 9).await;
    assert_eq!(billable_user_count(&admin).await, 10);
    let (_, guest_cookie) =
        create_second_user_session(&harness, "personal-guest@example.com", "Personal").await;

    let (status, body, _, _) = json_request(
        app,
        "POST",
        "/api/v1/me/personal-workspace",
        None,
        Some(&guest_cookie),
        &[],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{body}");
    assert_eq!(body["code"], "limit.seats");
    assert_eq!(billable_user_count(&admin).await, 10);

    admin.close().await;
    harness.cleanup().await;
}
