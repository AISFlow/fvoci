#![cfg(feature = "db-tests")]
//! Instance administration against a real PostgreSQL with the non-superuser
//! app role: admin-only routes, last-admin protection, the 428 consent gate,
//! consent records, typed instance settings, branding assets and audit paging.
//! Legal publishing renders markdown through the document convert helper
//! (FVOCI_DOCUMENT_CONVERT_BIN, see scripts/prepare-document-convert.sh).

use std::sync::Arc;

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use chrono::{Duration as ChronoDuration, Utc};
use fvoci_server::auth::password::Keyring;
use fvoci_server::auth::AuthService;
use fvoci_server::db::{migrate, pool, Db};
use fvoci_server::documents::convert::ConvertClient;
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
        migrate::apply_app_role_grants(&migration_pool, &role_name)
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
        if let Ok(pool) = PgPoolOptions::new()
            .max_connections(1)
            .connect(&server_url)
            .await
        {
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
    let mut server = url::Url::parse(url).expect("database url");
    server.set_path("");
    server.to_string().trim_end_matches('/').to_string()
}

fn join_db_url(server_url: &str, db_name: &str) -> String {
    let mut parsed = url::Url::parse(server_url).expect("server url");
    parsed.set_path(&format!("/{}", db_name));
    parsed.to_string()
}

fn convert_client() -> ConvertClient {
    ConvertClient::from_env().expect(
        "FVOCI_DOCUMENT_CONVERT_BIN is required; run scripts/prepare-document-convert.sh first",
    )
}

struct Harness {
    db: TestDb,
    app: axum::Router,
    storage_root: std::path::PathBuf,
    admin_cookie: String,
    admin_id: Uuid,
}

async fn app_state(app_url: &str, storage_root: &std::path::Path) -> AppState {
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
        storage: fvoci_server::attachments::LocalStorage::new(storage_root.to_path_buf()).into(),
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
        document_convert: Some(convert_client()),
        import_wake: None,
        import_extractor_available: false,
        quota: Default::default(),
        mailer: Arc::new(fvoci_server::mail::Mailer::disabled()),
    }
}

struct Reply {
    status: StatusCode,
    json: Value,
    bytes: Vec<u8>,
    headers: HeaderMap,
}

async fn send(
    app: &axum::Router,
    method: &str,
    path: &str,
    body: Option<(&str, Vec<u8>)>,
    cookie: Option<&str>,
    extra_headers: &[(&str, &str)],
) -> Reply {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", format!("fvoci_session={cookie}"));
    }
    for (name, value) in extra_headers {
        builder = builder.header(*name, *value);
    }
    let mut request = match body {
        Some((content_type, bytes)) => builder
            .header("content-type", content_type)
            .header("content-length", bytes.len().to_string())
            .body(Body::from(bytes))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(test_peer()));
    let response = app.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap_or_default()
        .to_vec();
    let json = serde_json::from_slice(&bytes).unwrap_or(json!({}));
    Reply {
        status,
        json,
        bytes,
        headers,
    }
}

async fn get(app: &axum::Router, path: &str, cookie: Option<&str>) -> Reply {
    send(app, "GET", path, None, cookie, &[]).await
}

async fn with_json(
    app: &axum::Router,
    method: &str,
    path: &str,
    body: Value,
    cookie: Option<&str>,
) -> Reply {
    send(
        app,
        method,
        path,
        Some(("application/json", body.to_string().into_bytes())),
        cookie,
        &[],
    )
    .await
}

fn session_cookie(headers: &HeaderMap) -> String {
    let raw = headers
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .expect("set-cookie");
    raw.split(';')
        .next()
        .unwrap()
        .split('=')
        .nth(1)
        .unwrap()
        .to_string()
}

async fn harness() -> Harness {
    let db = TestDb::bootstrap().await;
    let storage_root = std::env::temp_dir().join(format!("fvoci-admin-test-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&storage_root).expect("storage root");
    let app = router(app_state(&db.app_url, &storage_root).await, None);
    let setup = with_json(
        &app,
        "POST",
        "/api/v1/setup",
        json!({
            "email": "admin@example.com",
            "password": "supersecret1",
            "givenName": "Admin",
            "workspaceSlug": "acme",
            "workspaceName": "Acme"
        }),
        None,
    )
    .await;
    assert_eq!(setup.status, StatusCode::CREATED, "{}", setup.json);
    let admin_cookie = session_cookie(&setup.headers);
    let admin_id = Uuid::parse_str(setup.json["userId"].as_str().unwrap()).unwrap();
    Harness {
        db,
        app,
        storage_root,
        admin_cookie,
        admin_id,
    }
}

impl Harness {
    async fn finish(self) {
        let _ = std::fs::remove_dir_all(&self.storage_root);
        self.db.cleanup().await;
    }

    /// A plain live user (not an admin) with a session; optionally an acme member.
    async fn user(&self, email: &str, member_role: Option<&str>) -> (Uuid, String) {
        let admin = self.db.admin().await;
        let user_id = Uuid::now_v7();
        let hash = fvoci_server::auth::password::hash_password(
            "supersecret1",
            &Keyring::parse(PEPPER, "test").unwrap(),
        )
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO fvoci.users (id, email, password_hash, given_name) VALUES ($1, $2, $3, 'U')",
        )
        .bind(user_id)
        .bind(email)
        .bind(&hash)
        .execute(&admin)
        .await
        .unwrap();
        if let Some(role) = member_role {
            sqlx::query(
                "INSERT INTO fvoci.memberships (workspace_id, user_id, role)
                 SELECT id, $1, $2 FROM fvoci.workspaces WHERE slug = 'acme'",
            )
            .bind(user_id)
            .bind(role)
            .execute(&admin)
            .await
            .unwrap();
        }
        admin.close().await;
        let pool = pool::connect_app(&self.db.app_url).await.unwrap();
        let token = fvoci_server::auth::token::new_token();
        let mut tx = pool.begin().await.unwrap();
        fvoci_server::db::identity::create_session(
            &mut tx,
            Uuid::now_v7(),
            user_id,
            &token.hash,
            Utc::now() + ChronoDuration::days(30),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        pool.close().await;
        (user_id, token.token)
    }

    async fn acme_id(&self) -> Uuid {
        let admin = self.db.admin().await;
        let id = sqlx::query_scalar("SELECT id FROM fvoci.workspaces WHERE slug = 'acme'")
            .fetch_one(&admin)
            .await
            .unwrap();
        admin.close().await;
        id
    }

    async fn audit_verbs(&self) -> Vec<String> {
        let admin = self.db.admin().await;
        let verbs = sqlx::query_scalar("SELECT verb FROM fvoci.audit_log ORDER BY created_at, id")
            .fetch_all(&admin)
            .await
            .unwrap();
        admin.close().await;
        verbs
    }

    async fn publish(&self, kind: &str, required: bool) -> Reply {
        with_json(
            &self.app,
            "POST",
            "/api/v1/admin/legal",
            json!({
                "kind": kind,
                "title": format!("{kind} 약관"),
                "bodyMarkdown": "# 제1조\n\n**목적** <script>alert(1)</script> 본문",
                "required": required,
                "effectiveAt": "2026-10-01T00:00:00Z"
            }),
            Some(&self.admin_cookie),
        )
        .await
    }
}

const ADMIN_GETS: &[&str] = &[
    "/api/v1/admin/audit",
    "/api/v1/admin/system",
    "/api/v1/admin/users",
    "/api/v1/admin/workspaces",
    "/api/v1/admin/instance-settings",
];

#[tokio::test]
async fn admin_routes_are_hidden_from_non_admins_tokens_and_anonymous() {
    let h = harness().await;
    let (member_id, member) = h.user("member@example.com", Some("owner")).await;

    for path in ADMIN_GETS {
        let anon = get(&h.app, path, None).await;
        assert_eq!(anon.status, StatusCode::UNAUTHORIZED, "{path}");
        let denied = get(&h.app, path, Some(&member)).await;
        assert_eq!(denied.status, StatusCode::NOT_FOUND, "{path}");
        assert_eq!(denied.json["code"], "not_found", "{path}");
        let ok = get(&h.app, path, Some(&h.admin_cookie)).await;
        assert_eq!(ok.status, StatusCode::OK, "{path}: {}", ok.json);
    }
    let writes: Vec<(&str, &str, Value)> = vec![
        (
            "PATCH",
            "/api/v1/admin/users",
            json!({"userId": member_id, "instanceAdmin": true}),
        ),
        (
            "PATCH",
            "/api/v1/admin/instance-admins",
            json!({"userId": member_id, "value": true}),
        ),
        (
            "PATCH",
            "/api/v1/admin/instance-settings",
            json!({"share": {"enabled": false, "defaultExpiresDays": 1, "maxExpiresDays": 2}}),
        ),
        (
            "POST",
            "/api/v1/admin/legal",
            json!({"kind": "terms", "title": "t", "bodyMarkdown": "b", "required": true,
                   "effectiveAt": "2026-10-01T00:00:00Z"}),
        ),
    ];
    for (method, path, body) in &writes {
        let denied = with_json(&h.app, method, path, body.clone(), Some(&member)).await;
        assert_eq!(denied.status, StatusCode::NOT_FOUND, "{method} {path}");
    }
    let png = tiny_png();
    let upload = send(
        &h.app,
        "POST",
        "/api/v1/admin/branding/assets/logo",
        Some(("application/octet-stream", png)),
        Some(&member),
        &[],
    )
    .await;
    assert_eq!(upload.status, StatusCode::NOT_FOUND);
    let remove = send(
        &h.app,
        "DELETE",
        "/api/v1/admin/branding/assets/logo",
        None,
        Some(&member),
        &[],
    )
    .await;
    assert_eq!(remove.status, StatusCode::NOT_FOUND);

    // A workspace-manage API token of the instance admin is still not a session.
    let acme = h.acme_id().await;
    let token = with_json(
        &h.app,
        "POST",
        &format!("/api/v1/workspaces/{acme}/api-tokens"),
        json!({"name": "ops", "scopes": ["workspace.manage"]}),
        Some(&h.admin_cookie),
    )
    .await;
    assert_eq!(token.status, StatusCode::CREATED, "{}", token.json);
    let bearer = format!("Bearer {}", token.json["token"].as_str().unwrap());
    for path in ADMIN_GETS {
        let reply = send(
            &h.app,
            "GET",
            path,
            None,
            None,
            &[("authorization", &bearer)],
        )
        .await;
        assert_eq!(reply.status, StatusCode::NOT_FOUND, "{path}");
    }

    // Nothing was written by the denied calls.
    let admin = h.db.admin().await;
    let settings_rows: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.instance_settings")
        .fetch_one(&admin)
        .await
        .unwrap();
    let legal_rows: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.legal_documents")
        .fetch_one(&admin)
        .await
        .unwrap();
    let flag: bool = sqlx::query_scalar("SELECT is_instance_admin FROM fvoci.users WHERE id = $1")
        .bind(member_id)
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!((settings_rows, legal_rows, flag), (0, 0, false));
    admin.close().await;

    // Directory counts and user list shape.
    let system = get(&h.app, "/api/v1/admin/system", Some(&h.admin_cookie)).await;
    assert_eq!(system.json["users"], 2);
    assert_eq!(system.json["workspaces"], 1);
    let users = get(&h.app, "/api/v1/admin/users", Some(&h.admin_cookie)).await;
    let items = users.json["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["email"], "admin@example.com");
    assert_eq!(items[0]["instanceAdmin"], true);
    assert!(items[0]["eraseAt"].is_null());
    let workspaces = get(&h.app, "/api/v1/admin/workspaces", Some(&h.admin_cookie)).await;
    assert_eq!(workspaces.json["items"][0]["slug"], "acme");
    h.finish().await;
}

#[tokio::test]
async fn last_admin_protection_self_suspension_and_suspension_revokes_access() {
    let h = harness().await;
    let admin_id = h.admin_id;
    let patch = |body: Value| {
        with_json(
            &h.app,
            "PATCH",
            "/api/v1/admin/users",
            body,
            Some(&h.admin_cookie),
        )
    };

    let demote_self = patch(json!({"userId": admin_id, "instanceAdmin": false})).await;
    assert_eq!(demote_self.status, StatusCode::CONFLICT);
    assert_eq!(demote_self.json["code"], "last_instance_admin");
    let suspend_self = patch(json!({"userId": admin_id, "suspended": true})).await;
    assert_eq!(suspend_self.json["code"], "last_instance_admin");
    let via_flag = with_json(
        &h.app,
        "PATCH",
        "/api/v1/admin/instance-admins",
        json!({"userId": admin_id, "value": false}),
        Some(&h.admin_cookie),
    )
    .await;
    assert_eq!(via_flag.status, StatusCode::CONFLICT);
    assert_eq!(via_flag.json["code"], "last_instance_admin");
    let empty = patch(json!({"userId": admin_id})).await;
    assert_eq!(empty.status, StatusCode::BAD_REQUEST);
    let null_flag = patch(json!({"userId": admin_id, "suspended": null})).await;
    assert_eq!(null_flag.status, StatusCode::BAD_REQUEST);
    let missing = patch(json!({"userId": Uuid::now_v7(), "suspended": true})).await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);

    let (second_id, second) = h.user("second@example.com", Some("member")).await;
    let promote = with_json(
        &h.app,
        "PATCH",
        "/api/v1/admin/instance-admins",
        json!({"userId": second_id, "value": true}),
        Some(&h.admin_cookie),
    )
    .await;
    assert_eq!(promote.status, StatusCode::OK, "{}", promote.json);
    assert_eq!(
        get(&h.app, "/api/v1/admin/system", Some(&second))
            .await
            .status,
        StatusCode::OK
    );

    // Two admins: suspending yourself is still refused, with its own code.
    let suspend_self = patch(json!({"userId": admin_id, "suspended": true})).await;
    assert_eq!(suspend_self.status, StatusCode::CONFLICT);
    assert_eq!(suspend_self.json["code"], "self_suspension");

    // The second admin's API token and session die with the suspension.
    let acme = h.acme_id().await;
    let admin_pool = h.db.admin().await;
    sqlx::query("UPDATE fvoci.memberships SET role = 'admin' WHERE user_id = $1")
        .bind(second_id)
        .execute(&admin_pool)
        .await
        .unwrap();
    let token = with_json(
        &h.app,
        "POST",
        &format!("/api/v1/workspaces/{acme}/api-tokens"),
        json!({"name": "second", "scopes": ["workspace.manage"]}),
        Some(&second),
    )
    .await;
    assert_eq!(token.status, StatusCode::CREATED, "{}", token.json);
    let suspended = patch(json!({"userId": second_id, "suspended": true})).await;
    assert_eq!(suspended.status, StatusCode::OK, "{}", suspended.json);
    assert!(suspended.json["suspendedAt"].is_string());
    assert_eq!(
        get(&h.app, "/api/v1/auth/me", Some(&second)).await.status,
        StatusCode::UNAUTHORIZED
    );
    let live_tokens: i64 =
        sqlx::query_scalar("SELECT count(*) FROM fvoci.api_tokens WHERE user_id = $1")
            .bind(second_id)
            .fetch_one(&admin_pool)
            .await
            .unwrap();
    assert_eq!(live_tokens, 0);

    // The suspended admin no longer counts: demoting the remaining one fails.
    let demote_self = patch(json!({"userId": admin_id, "instanceAdmin": false})).await;
    assert_eq!(demote_self.json["code"], "last_instance_admin");
    let restored = patch(json!({"userId": second_id, "suspended": false})).await;
    assert_eq!(restored.status, StatusCode::OK);
    assert!(restored.json["suspendedAt"].is_null());
    let demote_self = patch(json!({"userId": admin_id, "instanceAdmin": false})).await;
    assert_eq!(demote_self.status, StatusCode::OK, "{}", demote_self.json);
    // The demoted actor loses the console immediately.
    assert_eq!(
        get(&h.app, "/api/v1/admin/users", Some(&h.admin_cookie))
            .await
            .status,
        StatusCode::NOT_FOUND
    );

    let verbs = h.audit_verbs().await;
    let count = |v: &str| verbs.iter().filter(|x| x.as_str() == v).count();
    assert_eq!(count("admin.instance_admin_set"), 2);
    assert_eq!(count("admin.user_suspended_set"), 2);
    let payload: Value = sqlx::query_scalar(
        "SELECT payload FROM fvoci.audit_log WHERE verb = 'admin.user_suspended_set' ORDER BY created_at LIMIT 1",
    )
    .fetch_one(&admin_pool)
    .await
    .unwrap();
    assert_eq!(
        payload,
        json!({"targetId": second_id.to_string(), "suspended": true})
    );
    let events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM fvoci.events WHERE verb LIKE 'admin.%' AND workspace_id IS NULL",
    )
    .fetch_one(&admin_pool)
    .await
    .unwrap();
    assert_eq!(events, 4);
    admin_pool.close().await;
    h.finish().await;
}

#[tokio::test]
async fn definer_floor_refuses_non_admin_callers_and_an_adminless_instance() {
    let h = harness().await;
    let (member_id, _) = h.user("member@example.com", None).await;
    let app = pool::connect_app(&h.db.app_url).await.unwrap();

    // The app role cannot write the flag columns directly.
    let direct = sqlx::query("UPDATE fvoci.users SET is_instance_admin = true WHERE id = $1")
        .bind(member_id)
        .execute(&app)
        .await;
    assert!(direct.is_err());

    // Without an admin self-context the definer refuses.
    let mut tx = app.begin().await.unwrap();
    let refused =
        sqlx::query_scalar::<_, i32>("SELECT fvoci.app_admin_set_instance_admin($1, true)")
            .bind(member_id)
            .fetch_one(&mut *tx)
            .await;
    let err = refused.unwrap_err();
    assert_eq!(
        err.as_database_error().unwrap().code().as_deref(),
        Some("42501")
    );
    tx.rollback().await.unwrap();

    let mut tx = app.begin().await.unwrap();
    fvoci_server::db::context::set_self_user(&mut tx, member_id)
        .await
        .unwrap();
    let refused = sqlx::query_scalar::<_, i32>("SELECT fvoci.app_admin_set_suspended($1, true)")
        .bind(h.admin_id)
        .fetch_one(&mut *tx)
        .await;
    assert_eq!(
        refused
            .unwrap_err()
            .as_database_error()
            .unwrap()
            .code()
            .as_deref(),
        Some("42501")
    );
    tx.rollback().await.unwrap();

    // Even an admin context cannot remove the last live admin.
    let mut tx = app.begin().await.unwrap();
    fvoci_server::db::context::set_self_user(&mut tx, h.admin_id)
        .await
        .unwrap();
    let refused =
        sqlx::query_scalar::<_, i32>("SELECT fvoci.app_admin_set_instance_admin($1, false)")
            .bind(h.admin_id)
            .fetch_one(&mut *tx)
            .await;
    assert_eq!(
        refused
            .unwrap_err()
            .as_database_error()
            .unwrap()
            .code()
            .as_deref(),
        Some("23514")
    );
    tx.rollback().await.unwrap();

    // The consent-gate definer answers only a boolean for a token hash.
    let pending: bool = sqlx::query_scalar("SELECT fvoci.app_session_consent_pending('nope')")
        .fetch_one(&app)
        .await
        .unwrap();
    assert!(!pending);
    app.close().await;
    h.finish().await;
}

#[tokio::test]
async fn concurrent_mutual_demotion_leaves_exactly_one_admin() {
    let h = harness().await;
    let (second_id, second) = h.user("second@example.com", None).await;
    let promote = with_json(
        &h.app,
        "PATCH",
        "/api/v1/admin/users",
        json!({"userId": second_id, "instanceAdmin": true}),
        Some(&h.admin_cookie),
    )
    .await;
    assert_eq!(promote.status, StatusCode::OK);
    for _ in 0..5 {
        let a = with_json(
            &h.app,
            "PATCH",
            "/api/v1/admin/users",
            json!({"userId": second_id, "instanceAdmin": false}),
            Some(&h.admin_cookie),
        );
        let b = with_json(
            &h.app,
            "PATCH",
            "/api/v1/admin/users",
            json!({"userId": h.admin_id, "instanceAdmin": false}),
            Some(&second),
        );
        let (a, b) = tokio::join!(a, b);
        let statuses = [a.status, b.status];
        assert_eq!(
            statuses.iter().filter(|s| **s == StatusCode::OK).count(),
            1,
            "{} / {}",
            a.json,
            b.json
        );
        let admin = h.db.admin().await;
        let admins: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM fvoci.users WHERE is_instance_admin AND deleted_at IS NULL AND suspended_at IS NULL",
        )
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(admins, 1);
        // Restore both as admins for the next round (owner connection).
        sqlx::query("UPDATE fvoci.users SET is_instance_admin = true")
            .execute(&admin)
            .await
            .unwrap();
        admin.close().await;
    }
    h.finish().await;
}

#[tokio::test]
async fn audit_log_pages_with_a_stable_keyset_cursor() {
    let h = harness().await;
    let admin = h.db.admin().await;
    // Ten rows sharing one timestamp plus the setup row: ties break on id.
    let at = Utc::now() - ChronoDuration::minutes(5);
    for i in 0..10 {
        sqlx::query(
            "INSERT INTO fvoci.audit_log (id, verb, payload, ip, created_at) VALUES ($1, 'test.row', $2, '198.51.100.7', $3)",
        )
        .bind(Uuid::now_v7())
        .bind(json!({"i": i}))
        .bind(at)
        .execute(&admin)
        .await
        .unwrap();
    }
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.audit_log")
        .fetch_one(&admin)
        .await
        .unwrap();
    admin.close().await;

    let mut seen = Vec::new();
    let mut cursor: Option<String> = None;
    let mut pages = 0;
    loop {
        let path = match &cursor {
            Some(c) => format!("/api/v1/admin/audit?limit=3&cursor={c}"),
            None => "/api/v1/admin/audit?limit=3".to_string(),
        };
        let page = get(&h.app, &path, Some(&h.admin_cookie)).await;
        assert_eq!(page.status, StatusCode::OK, "{}", page.json);
        for item in page.json["items"].as_array().unwrap() {
            seen.push((
                item["createdAt"].as_str().unwrap().to_string(),
                item["id"].as_str().unwrap().to_string(),
            ));
        }
        pages += 1;
        match page.json["nextCursor"].as_str() {
            Some(next) => cursor = Some(next.to_string()),
            None => break,
        }
    }
    assert_eq!(seen.len() as i64, total);
    assert_eq!(pages, (total as usize).div_ceil(3));
    let mut unique = seen.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), seen.len(), "no row repeated");
    let first = get(
        &h.app,
        "/api/v1/admin/audit?limit=100",
        Some(&h.admin_cookie),
    )
    .await;
    let items = first.json["items"].as_array().unwrap();
    assert_eq!(items[0]["verb"], "instance.setup");
    assert_eq!(items[1]["ip"], "198.51.100.7");
    assert!(items[1]["payload"]["i"].as_i64().is_some());

    for bad in [
        "/api/v1/admin/audit?limit=0",
        "/api/v1/admin/audit?limit=101",
        "/api/v1/admin/audit?limit=2.5",
        "/api/v1/admin/audit?cursor=not-a-cursor",
        "/api/v1/admin/audit?verb=x",
    ] {
        let reply = get(&h.app, bad, Some(&h.admin_cookie)).await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{bad}");
    }
    h.finish().await;
}

#[tokio::test]
async fn instance_settings_validate_persist_audit_and_project_publicly() {
    let h = harness().await;
    let patch = |body: Value| {
        with_json(
            &h.app,
            "PATCH",
            "/api/v1/admin/instance-settings",
            body,
            Some(&h.admin_cookie),
        )
    };
    let initial = get(
        &h.app,
        "/api/v1/admin/instance-settings",
        Some(&h.admin_cookie),
    )
    .await;
    assert_eq!(initial.json["values"]["share"]["enabled"], true);
    assert_eq!(initial.json["values"]["branding"]["name"], "FVOCI");
    assert_eq!(initial.json["overridden"], json!([]));
    assert_eq!(
        initial.json["eeFeatures"],
        json!(["audit", "branding", "workspaceSso"])
    );
    let revision0 = initial.json["version"].as_i64().unwrap();

    let invalid = [
        json!({}),
        json!({"nope": null}),
        json!({"share": {"enabled": true, "defaultExpiresDays": 30, "maxExpiresDays": 7}}),
        json!({"share": {"enabled": true, "defaultExpiresDays": 3}}),
        json!({"branding": {"name": "X", "smtpFromDisplay": null, "loginBrandText": null,
               "logo": {"key": Uuid::now_v7(), "sha256": "a".repeat(64), "mime": "image/png"}}}),
        json!({"branding": {"name": "X\nBcc: evil", "smtpFromDisplay": null, "loginBrandText": null}}),
        json!({"auth": {"passwordMinLength": 9}}),
        json!({"operator": {"businessName": null, "representative": null, "registrationNumber": null,
               "mailOrderNumber": null, "address": null, "phone": null, "supportEmail": null,
               "businessInfoUrl": "javascript:alert(1)", "hostingProvider": null}}),
        json!({"i18n": {"overrides": {"mail.magic.link.text": "no url"}}}),
        json!([1]),
    ];
    for body in invalid {
        let reply = patch(body.clone()).await;
        assert_eq!(
            reply.status,
            StatusCode::BAD_REQUEST,
            "{body}: {}",
            reply.json
        );
    }

    let share = json!({"enabled": false, "defaultExpiresDays": 3, "maxExpiresDays": 30});
    let updated = patch(json!({"share": share, "branding": {"name": "  사내 위키 ", "smtpFromDisplay": null, "loginBrandText": "환영"}})).await;
    assert_eq!(updated.status, StatusCode::OK, "{}", updated.json);
    assert_eq!(updated.json["values"]["share"], share);
    assert_eq!(updated.json["values"]["branding"]["name"], "사내 위키");
    assert_eq!(updated.json["overridden"], json!(["branding", "share"]));
    let revision1 = updated.json["version"].as_i64().unwrap();
    assert!(revision1 > revision0);

    // Read API used by other features.
    let app_pool = pool::connect_app(&h.db.app_url).await.unwrap();
    let policy = fvoci_server::settings::share_policy(&app_pool)
        .await
        .unwrap();
    assert!(!policy.enabled);
    assert_eq!(
        (policy.default_expires_days, policy.max_expires_days),
        (3, 30)
    );

    // Same value again: no row change, no audit.
    let same = patch(json!({"share": share})).await;
    assert_eq!(same.json["version"].as_i64().unwrap(), revision1);
    let admin = h.db.admin().await;
    let audits: Vec<Value> = sqlx::query_scalar(
        "SELECT payload FROM fvoci.audit_log WHERE verb = 'instance_settings.updated' ORDER BY created_at",
    )
    .fetch_all(&admin)
    .await
    .unwrap();
    assert_eq!(audits.len(), 1);
    // JSON object keys arrive unordered (serde_json map); compare as a set.
    let mut keys: Vec<&str> = audits[0]["keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| k.as_str().unwrap())
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, ["branding", "share"]);
    let fields = audits[0]["fields"].as_array().unwrap();
    assert!(fields.contains(&json!("share.enabled")));
    assert!(fields.contains(&json!("branding.name")));
    // Key paths only, never the values.
    assert!(!audits[0].to_string().contains("사내 위키"));

    // Public projection: public keys only, branding without storage identity.
    let public = get(&h.app, "/api/v1/instance", None).await;
    assert_eq!(public.status, StatusCode::OK);
    let values = public.json["values"].as_object().unwrap();
    for hidden in ["auth", "embed", "i18n", "security"] {
        assert!(!values.contains_key(hidden), "{hidden} leaked");
    }
    assert_eq!(values["share"], share);
    assert_eq!(values["branding"]["name"], "사내 위키");
    assert!(values["branding"].get("smtpFromDisplay").is_none());
    assert!(values["webPushPublicKey"].is_null());
    let etag = public
        .headers
        .get("etag")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert_eq!(
        public.headers.get("cache-control").unwrap(),
        "public, max-age=60"
    );
    let cached = send(
        &h.app,
        "GET",
        "/api/v1/instance",
        None,
        None,
        &[("if-none-match", &format!("W/{etag}"))],
    )
    .await;
    assert_eq!(cached.status, StatusCode::NOT_MODIFIED);
    let setup_status = get(&h.app, "/api/v1/setup", None).await;
    assert_eq!(setup_status.json["branding"]["name"], "사내 위키");

    // Reset deletes the row and falls back to the default.
    let reset = patch(json!({"share": null})).await;
    assert_eq!(reset.json["values"]["share"]["enabled"], true);
    assert_eq!(reset.json["overridden"], json!(["branding"]));
    let noop_reset = patch(json!({"share": null})).await;
    assert_eq!(noop_reset.json["version"], reset.json["version"]);

    // A stored row the app did not write (tampered) falls back, not 500.
    sqlx::query("INSERT INTO fvoci.instance_settings (key, value) VALUES ('auth', '{\"passwordMinLength\": 3}')")
        .execute(&admin)
        .await
        .unwrap();
    let tolerant = get(
        &h.app,
        "/api/v1/admin/instance-settings",
        Some(&h.admin_cookie),
    )
    .await;
    assert_eq!(tolerant.json["values"]["auth"]["passwordMinLength"], 10);

    // The password setting raises the floor on reset/invite/setup paths.
    let raised = patch(json!({"auth": {"passwordMinLength": 14}})).await;
    assert_eq!(raised.status, StatusCode::OK, "{}", raised.json);
    let reset_confirm = with_json(
        &h.app,
        "POST",
        "/api/v1/auth/password-reset/confirm",
        json!({"token": "x".repeat(43), "newPassword": "thirteen-char"}),
        None,
    )
    .await;
    assert_eq!(
        reset_confirm.json["code"], "password_invalid",
        "{}",
        reset_confirm.json
    );

    // Settings writes are atomic with their audit row.
    sqlx::query(
        r#"CREATE FUNCTION fvoci.test_block_audit() RETURNS trigger LANGUAGE plpgsql AS $$
           BEGIN RAISE EXCEPTION 'audit blocked'; END; $$"#,
    )
    .execute(&admin)
    .await
    .unwrap();
    sqlx::query("CREATE TRIGGER test_block_audit BEFORE INSERT ON fvoci.audit_log FOR EACH ROW EXECUTE FUNCTION fvoci.test_block_audit()")
        .execute(&admin)
        .await
        .unwrap();
    let failed = patch(json!({"features": {"ai": true}})).await;
    assert_eq!(failed.status, StatusCode::INTERNAL_SERVER_ERROR);
    let features_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM fvoci.instance_settings WHERE key = 'features'")
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(features_rows, 0);
    sqlx::query("DROP TRIGGER test_block_audit ON fvoci.audit_log")
        .execute(&admin)
        .await
        .unwrap();

    // Restart-required keys report a pending restart after a change.
    let ai = patch(json!({"features": {"ai": true}})).await;
    assert_eq!(ai.json["restartRequired"], json!(["features"]));

    // The app role cannot write settings outside the admin transaction.
    let direct =
        sqlx::query("INSERT INTO fvoci.instance_settings (key, value) VALUES ('embed', '{}')")
            .execute(&app_pool)
            .await;
    assert!(direct.is_err());
    admin.close().await;
    app_pool.close().await;
    h.finish().await;
}

fn tiny_png() -> Vec<u8> {
    // 1x1 RGBA PNG.
    let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    png.extend_from_slice(&[
        0, 0, 0, 13, b'I', b'H', b'D', b'R', 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0,
    ]);
    png.extend_from_slice(&[0x1f, 0x15, 0xc4, 0x89]);
    png.extend_from_slice(&[0, 0, 0, 0, b'I', b'E', b'N', b'D', 0xae, 0x42, 0x60, 0x82]);
    png
}

#[tokio::test]
async fn branding_assets_upload_serve_replace_and_remove() {
    let h = harness().await;
    let upload = |bytes: Vec<u8>, content_type: &'static str| {
        let app = h.app.clone();
        let cookie = h.admin_cookie.clone();
        async move {
            send(
                &app,
                "POST",
                "/api/v1/admin/branding/assets/logo",
                Some((content_type, bytes)),
                Some(&cookie),
                &[],
            )
            .await
        }
    };
    let svg = upload(
        b"<svg xmlns='http://www.w3.org/2000/svg'><script>1</script></svg>".to_vec(),
        "application/octet-stream",
    )
    .await;
    assert_eq!(svg.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(svg.json["code"], "unsupported_branding_asset_type");
    let wrong_type = upload(tiny_png(), "image/png").await;
    assert_eq!(wrong_type.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(wrong_type.json["code"], "unsupported_media_type");
    let empty = upload(Vec::new(), "application/octet-stream").await;
    assert_eq!(empty.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        empty.json["code"],
        "raw_application_octet_stream_body_required"
    );
    let mut big = tiny_png();
    big.resize(512 * 1024 + 1, 0);
    let too_big = upload(big, "application/octet-stream").await;
    assert_eq!(too_big.status, StatusCode::PAYLOAD_TOO_LARGE);
    let bad_kind = send(
        &h.app,
        "POST",
        "/api/v1/admin/branding/assets/banner",
        Some(("application/octet-stream", tiny_png())),
        Some(&h.admin_cookie),
        &[],
    )
    .await;
    assert_eq!(bad_kind.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        get(&h.app, "/api/v1/branding/logo", None).await.status,
        StatusCode::NOT_FOUND
    );

    let first = upload(tiny_png(), "application/octet-stream").await;
    assert_eq!(first.status, StatusCode::OK, "{}", first.json);
    let logo = first.json["values"]["branding"]["logo"].clone();
    assert_eq!(logo["mime"], "image/png");
    let first_key = logo["key"].as_str().unwrap().to_string();

    let served = get(&h.app, "/api/v1/branding/logo", None).await;
    assert_eq!(served.status, StatusCode::OK);
    assert_eq!(served.bytes, tiny_png());
    assert_eq!(served.headers.get("content-type").unwrap(), "image/png");
    assert_eq!(
        served.headers.get("x-content-type-options").unwrap(),
        "nosniff"
    );
    assert_eq!(
        served.headers.get("content-security-policy").unwrap(),
        "sandbox"
    );
    let etag = served
        .headers
        .get("etag")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let cached = send(
        &h.app,
        "GET",
        "/api/v1/branding/logo",
        None,
        None,
        &[("if-none-match", &etag)],
    )
    .await;
    assert_eq!(cached.status, StatusCode::NOT_MODIFIED);
    let public = get(&h.app, "/api/v1/instance", None).await;
    let href = public.json["values"]["branding"]["logo"].as_str().unwrap();
    assert!(href.starts_with("/api/v1/branding/logo?v="));
    assert!(!public
        .bytes
        .windows(first_key.len())
        .any(|w| w == first_key.as_bytes()));

    // Replacing deletes the previous object after commit.
    let mut second_png = tiny_png();
    second_png.extend_from_slice(b"trailer");
    let second = upload(second_png.clone(), "application/octet-stream").await;
    assert_eq!(second.status, StatusCode::OK, "{}", second.json);
    let second_key = second.json["values"]["branding"]["logo"]["key"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(first_key, second_key);
    assert!(!h.storage_root.join("objects").join(&first_key).exists());
    assert_eq!(
        get(&h.app, "/api/v1/branding/logo", None).await.bytes,
        second_png
    );

    // A settings row whose digest no longer matches the object is not served.
    let admin = h.db.admin().await;
    sqlx::query(
        "UPDATE fvoci.instance_settings SET value = jsonb_set(value, '{logo,sha256}', to_jsonb(repeat('0', 64))) WHERE key = 'branding'",
    )
    .execute(&admin)
    .await
    .unwrap();
    assert_eq!(
        get(&h.app, "/api/v1/branding/logo", None).await.status,
        StatusCode::NOT_FOUND
    );
    admin.close().await;

    let removed = send(
        &h.app,
        "DELETE",
        "/api/v1/admin/branding/assets/logo",
        None,
        Some(&h.admin_cookie),
        &[],
    )
    .await;
    assert_eq!(removed.status, StatusCode::OK, "{}", removed.json);
    assert!(removed.json["values"]["branding"]["logo"].is_null());
    assert!(!h.storage_root.join("objects").join(&second_key).exists());
    let again = send(
        &h.app,
        "DELETE",
        "/api/v1/admin/branding/assets/logo",
        None,
        Some(&h.admin_cookie),
        &[],
    )
    .await;
    assert_eq!(again.status, StatusCode::NOT_FOUND);
    // A later name-only PATCH keeps asset leaves intact.
    let fav = send(
        &h.app,
        "POST",
        "/api/v1/admin/branding/assets/favicon",
        Some(("application/octet-stream", tiny_png())),
        Some(&h.admin_cookie),
        &[],
    )
    .await;
    assert_eq!(fav.status, StatusCode::OK);
    let renamed = with_json(
        &h.app,
        "PATCH",
        "/api/v1/admin/instance-settings",
        json!({"branding": {"name": "Renamed", "smtpFromDisplay": null, "loginBrandText": null}}),
        Some(&h.admin_cookie),
    )
    .await;
    assert_eq!(
        renamed.json["values"]["branding"]["favicon"]["mime"],
        "image/png"
    );
    let verbs = h.audit_verbs().await;
    assert_eq!(
        verbs
            .iter()
            .filter(|v| *v == "instance_settings.updated")
            .count(),
        5
    );
    h.finish().await;
}

#[tokio::test]
async fn legal_publish_consent_gate_and_consent_records() {
    let h = harness().await;
    let (member_id, member) = h.user("member@example.com", Some("member")).await;

    // Validation and versioning.
    for body in [
        json!({"kind": "Terms", "title": "t", "bodyMarkdown": "b", "required": true, "effectiveAt": "2026-10-01T00:00:00Z"}),
        json!({"kind": "terms", "title": " ", "bodyMarkdown": "b", "required": true, "effectiveAt": "2026-10-01T00:00:00Z"}),
        json!({"kind": "terms", "title": "t", "bodyMarkdown": "", "required": true, "effectiveAt": "2026-10-01T00:00:00Z"}),
        json!({"kind": "terms", "title": "t", "bodyMarkdown": "b", "required": true, "effectiveAt": "2026-10-01T09:00:00+09:00"}),
        json!({"kind": "terms", "title": "t", "bodyMarkdown": "b", "required": true, "effectiveAt": "2026-10-01T00:00:00Z", "extra": 1}),
    ] {
        let reply = with_json(
            &h.app,
            "POST",
            "/api/v1/admin/legal",
            body.clone(),
            Some(&h.admin_cookie),
        )
        .await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{body}");
    }
    // Optional documents never gate.
    let privacy = h.publish("privacy", false).await;
    assert_eq!(privacy.status, StatusCode::CREATED, "{}", privacy.json);
    assert_eq!(
        get(&h.app, "/api/v1/auth/me", Some(&member)).await.status,
        StatusCode::OK
    );

    let terms = h.publish("terms", true).await;
    assert_eq!(terms.status, StatusCode::CREATED, "{}", terms.json);
    assert_eq!(terms.json["version"], 1);
    let html = terms.json["bodyHtml"].as_str().unwrap();
    assert!(html.contains("<strong>목적</strong>"), "{html}");
    assert!(!html.contains("<script"), "{html}");

    // Public reads.
    let latest = get(&h.app, "/api/v1/legal/terms", None).await;
    assert_eq!(latest.json["version"], 1);
    assert_eq!(
        get(&h.app, "/api/v1/legal/terms?version=2", None)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&h.app, "/api/v1/legal/terms?version=0", None)
            .await
            .status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        get(&h.app, "/api/v1/legal/Bad_Kind", None).await.status,
        StatusCode::BAD_REQUEST
    );

    // The gate: every authenticated cookie request except the allowlist.
    for path in [
        "/api/v1/auth/me",
        "/api/v1/workspaces",
        "/api/v1/instance",
        "/api/v1/admin/system",
        // The collaboration upgrade is gated too; the gate answers before the
        // handler, so no collab runtime is needed here.
        "/collab",
    ] {
        let gated = get(&h.app, path, Some(&member)).await;
        assert_eq!(gated.status, StatusCode::PRECONDITION_REQUIRED, "{path}");
        assert_eq!(gated.json["code"], "consent_required", "{path}");
    }
    // The admin who published is gated too until they accept.
    assert_eq!(
        get(&h.app, "/api/v1/admin/users", Some(&h.admin_cookie))
            .await
            .status,
        StatusCode::PRECONDITION_REQUIRED
    );
    // Anonymous requests are not gated.
    assert_eq!(
        get(&h.app, "/api/v1/instance", None).await.status,
        StatusCode::OK
    );
    let pending = get(&h.app, "/api/v1/auth/consents/pending", Some(&member)).await;
    assert_eq!(pending.status, StatusCode::OK);
    let pending_items = pending.json["pending"].as_array().unwrap();
    assert_eq!(pending_items.len(), 1);
    assert_eq!(pending_items[0]["kind"], "terms");
    assert!(pending_items[0]["bodyHtml"].is_string());
    assert_eq!(
        get(&h.app, "/api/v1/legal/terms/versions", Some(&member))
            .await
            .status,
        StatusCode::OK
    );

    // A stale or unknown item is skipped, so the gate stays.
    let stale = with_json(
        &h.app,
        "POST",
        "/api/v1/auth/consents",
        json!({"items": [{"kind": "terms", "version": 9}]}),
        Some(&member),
    )
    .await;
    assert_eq!(stale.status, StatusCode::OK);
    assert_eq!(
        get(&h.app, "/api/v1/auth/me", Some(&member)).await.status,
        StatusCode::PRECONDITION_REQUIRED
    );
    let empty = with_json(
        &h.app,
        "POST",
        "/api/v1/auth/consents",
        json!({"items": []}),
        Some(&member),
    )
    .await;
    assert_eq!(empty.status, StatusCode::BAD_REQUEST);
    let accept = with_json(
        &h.app,
        "POST",
        "/api/v1/auth/consents",
        json!({"items": [{"kind": "terms", "version": 1, "extra": true}]}),
        Some(&member),
    )
    .await;
    assert_eq!(accept.status, StatusCode::OK, "{}", accept.json);
    assert_eq!(
        get(&h.app, "/api/v1/auth/me", Some(&member)).await.status,
        StatusCode::OK
    );
    // Re-submitting is idempotent.
    let again = with_json(
        &h.app,
        "POST",
        "/api/v1/auth/consents",
        json!({"items": [{"kind": "terms", "version": 1}]}),
        Some(&member),
    )
    .await;
    assert_eq!(again.status, StatusCode::OK);

    // A new required version gates again.
    assert_eq!(
        h.publish("terms", true).await.status,
        StatusCode::PRECONDITION_REQUIRED
    );
    let accept_admin = with_json(
        &h.app,
        "POST",
        "/api/v1/auth/consents",
        json!({"items": [{"kind": "terms", "version": 1}]}),
        Some(&h.admin_cookie),
    )
    .await;
    assert_eq!(accept_admin.status, StatusCode::OK);
    let v2 = h.publish("terms", true).await;
    assert_eq!(v2.status, StatusCode::CREATED);
    assert_eq!(v2.json["version"], 2);
    assert_eq!(
        get(&h.app, "/api/v1/auth/me", Some(&member)).await.status,
        StatusCode::PRECONDITION_REQUIRED
    );
    let versions = get(&h.app, "/api/v1/legal/terms/versions", None).await;
    assert_eq!(versions.json["versions"].as_array().unwrap().len(), 2);
    assert_eq!(versions.json["versions"][0]["version"], 2);
    assert!(versions.json["versions"][0].get("bodyHtml").is_none());

    // API tokens are not gated (source: session cookies only).
    let with_consent = with_json(
        &h.app,
        "POST",
        "/api/v1/auth/consents",
        json!({"items": [{"kind": "terms", "version": 2}]}),
        Some(&h.admin_cookie),
    )
    .await;
    assert_eq!(with_consent.status, StatusCode::OK);
    let acme = h.acme_id().await;
    let admin_pool = h.db.admin().await;
    sqlx::query("UPDATE fvoci.memberships SET role = 'admin' WHERE user_id = $1")
        .bind(member_id)
        .execute(&admin_pool)
        .await
        .unwrap();
    let accept_v2 = with_json(
        &h.app,
        "POST",
        "/api/v1/auth/consents",
        json!({"items": [{"kind": "terms", "version": 2}]}),
        Some(&member),
    )
    .await;
    assert_eq!(accept_v2.status, StatusCode::OK);
    let token = with_json(
        &h.app,
        "POST",
        &format!("/api/v1/workspaces/{acme}/api-tokens"),
        json!({"name": "bot", "scopes": ["workspace.manage"]}),
        Some(&member),
    )
    .await;
    assert_eq!(token.status, StatusCode::CREATED, "{}", token.json);
    let v3 = h.publish("terms", true).await;
    assert_eq!(v3.status, StatusCode::CREATED);
    let bearer = format!("Bearer {}", token.json["token"].as_str().unwrap());
    let via_token = send(
        &h.app,
        "GET",
        &format!("/api/v1/workspaces/{acme}/api-tokens"),
        None,
        None,
        &[("authorization", &bearer)],
    )
    .await;
    assert_eq!(via_token.status, StatusCode::OK, "{}", via_token.json);

    // Workspace consents: workspace admins only; members are listed with their rows.
    let admin_accept = with_json(
        &h.app,
        "POST",
        "/api/v1/auth/consents",
        json!({"items": [{"kind": "terms", "version": 3}]}),
        Some(&h.admin_cookie),
    )
    .await;
    assert_eq!(admin_accept.status, StatusCode::OK);
    let listed = get(
        &h.app,
        &format!("/api/v1/workspaces/{acme}/consents"),
        Some(&h.admin_cookie),
    )
    .await;
    assert_eq!(listed.status, StatusCode::OK, "{}", listed.json);
    let members = listed.json["members"].as_array().unwrap();
    let member_row = members
        .iter()
        .find(|m| m["userId"] == member_id.to_string())
        .expect("member listed");
    assert_eq!(member_row["consents"].as_array().unwrap().len(), 2);
    sqlx::query("UPDATE fvoci.memberships SET role = 'member' WHERE user_id = $1")
        .bind(member_id)
        .execute(&admin_pool)
        .await
        .unwrap();
    let member_accept = with_json(
        &h.app,
        "POST",
        "/api/v1/auth/consents",
        json!({"items": [{"kind": "terms", "version": 3}]}),
        Some(&member),
    )
    .await;
    assert_eq!(member_accept.status, StatusCode::OK);
    let denied = get(
        &h.app,
        &format!("/api/v1/workspaces/{acme}/consents"),
        Some(&member),
    )
    .await;
    assert_eq!(denied.status, StatusCode::NOT_FOUND);

    // Evidence rows: channel, ip; consent.recorded only when nothing is pending.
    let rows: Vec<(String, i32, String, Option<String>)> = sqlx::query_as(
        "SELECT kind, version, channel, host(ip) FROM fvoci.user_consents WHERE user_id = $1 ORDER BY version",
    )
    .bind(member_id)
    .fetch_all(&admin_pool)
    .await
    .unwrap();
    assert_eq!(
        rows,
        vec![
            (
                "terms".into(),
                1,
                "gate".into(),
                Some("203.0.113.10".into())
            ),
            (
                "terms".into(),
                2,
                "gate".into(),
                Some("203.0.113.10".into())
            ),
            (
                "terms".into(),
                3,
                "gate".into(),
                Some("203.0.113.10".into())
            ),
        ]
    );
    let recorded: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM fvoci.audit_log WHERE verb = 'consent.recorded' AND actor_user_id = $1",
    )
    .bind(member_id)
    .fetch_one(&admin_pool)
    .await
    .unwrap();
    assert_eq!(
        recorded, 4,
        "one per completed acceptance (v1 twice, v2, v3)"
    );
    let published: i64 =
        sqlx::query_scalar("SELECT count(*) FROM fvoci.audit_log WHERE verb = 'legal.published'")
            .fetch_one(&admin_pool)
            .await
            .unwrap();
    assert_eq!(published, 4);
    // Consents are append-only evidence for the app role.
    let app_pool = pool::connect_app(&h.db.app_url).await.unwrap();
    assert!(sqlx::query("DELETE FROM fvoci.user_consents")
        .execute(&app_pool)
        .await
        .is_err());
    assert!(sqlx::query("UPDATE fvoci.legal_documents SET title = 'x'")
        .execute(&app_pool)
        .await
        .is_err());
    app_pool.close().await;
    admin_pool.close().await;
    h.finish().await;
}

#[tokio::test]
async fn invitation_acceptance_requires_and_records_required_legal_consent() {
    let h = harness().await;
    assert_eq!(h.publish("terms", true).await.status, StatusCode::CREATED);
    let admin_accept = with_json(
        &h.app,
        "POST",
        "/api/v1/auth/consents",
        json!({"items": [{"kind": "terms", "version": 1}]}),
        Some(&h.admin_cookie),
    )
    .await;
    assert_eq!(admin_accept.status, StatusCode::OK);
    let acme = h.acme_id().await;
    let defaults = with_json(
        &h.app,
        "PATCH",
        "/api/v1/admin/instance-settings",
        json!({"defaults.user": {"locale": "ko", "timezone": "UTC", "weekStartsOn": 0, "textScale": 18}}),
        Some(&h.admin_cookie),
    )
    .await;
    assert_eq!(defaults.status, StatusCode::OK, "{}", defaults.json);
    let invite = with_json(
        &h.app,
        "POST",
        &format!("/api/v1/workspaces/{acme}/invitations"),
        json!({"email": "new@example.com", "role": "member"}),
        Some(&h.admin_cookie),
    )
    .await;
    assert_eq!(invite.status, StatusCode::CREATED, "{}", invite.json);
    let url = invite.json["acceptUrl"].as_str().unwrap();
    let token = url.rsplit('/').next().unwrap().to_string();
    let preview = get(&h.app, &format!("/api/v1/invitations/{token}"), None).await;
    assert_eq!(
        preview.json["requiredLegal"],
        json!([{"kind": "terms", "version": 1, "title": "terms 약관"}])
    );

    let body = |consents: Value| json!({"email": "new@example.com", "givenName": "New", "password": "supersecret1", "consents": consents});
    let missing = with_json(
        &h.app,
        "POST",
        &format!("/api/v1/invitations/{token}/accept"),
        body(json!([])),
        None,
    )
    .await;
    assert_eq!(missing.status, StatusCode::PRECONDITION_REQUIRED);
    let stale = with_json(
        &h.app,
        "POST",
        &format!("/api/v1/invitations/{token}/accept"),
        body(json!([{"kind": "terms", "version": 2}])),
        None,
    )
    .await;
    assert_eq!(stale.status, StatusCode::PRECONDITION_REQUIRED);
    let accepted = with_json(
        &h.app,
        "POST",
        &format!("/api/v1/invitations/{token}/accept"),
        body(json!([{"kind": "terms", "version": 1}])),
        None,
    )
    .await;
    assert_eq!(accepted.status, StatusCode::OK, "{}", accepted.json);
    let new_cookie = session_cookie(&accepted.headers);
    let me = get(&h.app, "/api/v1/auth/me", Some(&new_cookie)).await;
    assert_eq!(me.status, StatusCode::OK, "{}", me.json);
    let admin = h.db.admin().await;
    let (channel, tz, scale): (String, String, i16) = sqlx::query_as(
        "SELECT c.channel, u.timezone, u.text_scale FROM fvoci.user_consents c JOIN fvoci.users u ON u.id = c.user_id WHERE u.email = 'new@example.com'",
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(
        (channel.as_str(), tz.as_str(), scale),
        ("signup", "UTC", 18)
    );
    admin.close().await;
    h.finish().await;
}

/// Waits until some backend is blocked on a lock while touching `fvoci.users`.
async fn wait_for_users_lock_waiter(admin: &PgPool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let waiting: i64 = sqlx::query_scalar(
            r#"
            SELECT count(*) FROM pg_stat_activity
            WHERE datname = current_database()
              AND wait_event_type = 'Lock'
              AND state = 'active'
              AND query ILIKE '%fvoci.users%'
            "#,
        )
        .fetch_one(admin)
        .await
        .unwrap();
        if waiting > 0 {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no lock waiter appeared"
        );
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn demotion_committed_while_a_settings_write_waits_is_seen_in_its_transaction() {
    let h = harness().await;
    let (second_id, _) = h.user("second@example.com", None).await;
    let promote = with_json(
        &h.app,
        "PATCH",
        "/api/v1/admin/users",
        json!({"userId": second_id, "instanceAdmin": true}),
        Some(&h.admin_cookie),
    )
    .await;
    assert_eq!(promote.status, StatusCode::OK);

    // Barrier: hold the actor's user row, start the write, demote, release.
    let admin = h.db.admin().await;
    let mut holder = admin.begin().await.unwrap();
    sqlx::query("SELECT id FROM fvoci.users WHERE id = $1 FOR UPDATE")
        .bind(h.admin_id)
        .execute(&mut *holder)
        .await
        .unwrap();
    let app = h.app.clone();
    let cookie = h.admin_cookie.clone();
    let write = tokio::spawn(async move {
        with_json(
            &app,
            "PATCH",
            "/api/v1/admin/instance-settings",
            json!({"share": {"enabled": false, "defaultExpiresDays": 1, "maxExpiresDays": 1}}),
            Some(&cookie),
        )
        .await
    });
    wait_for_users_lock_waiter(&admin).await;
    sqlx::query("UPDATE fvoci.users SET is_instance_admin = false WHERE id = $1")
        .bind(h.admin_id)
        .execute(&mut *holder)
        .await
        .unwrap();
    holder.commit().await.unwrap();
    let reply = write.await.unwrap();
    assert_eq!(reply.status, StatusCode::NOT_FOUND, "{}", reply.json);
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.instance_settings")
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(rows, 0);
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM fvoci.audit_log WHERE verb = 'instance_settings.updated'",
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(audits, 0);
    admin.close().await;
    h.finish().await;
}

#[tokio::test]
async fn share_policy_setting_governs_share_links() {
    let h = harness().await;
    let ws = h.acme_id().await;
    let doc = with_json(
        &h.app,
        "POST",
        &format!("/api/v1/workspaces/{ws}/documents"),
        json!({"parentId": null, "title": "공유 문서"}),
        Some(&h.admin_cookie),
    )
    .await;
    assert_eq!(doc.status, StatusCode::CREATED, "{}", doc.json);
    let doc_id = doc.json["id"].as_str().unwrap().to_string();
    let links = format!("/api/v1/workspaces/{ws}/documents/{doc_id}/share-links");
    let created = with_json(&h.app, "POST", &links, json!({}), Some(&h.admin_cookie)).await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.json);
    let url = created.json["url"].as_str().unwrap();
    let token = url.rsplit('/').next().unwrap().to_string();
    let public = format!("/api/v1/share/{token}");
    assert_eq!(get(&h.app, &public, None).await.status, StatusCode::OK);

    let patch = |body: Value| {
        with_json(
            &h.app,
            "PATCH",
            "/api/v1/admin/instance-settings",
            body,
            Some(&h.admin_cookie),
        )
    };
    // Disabled: every public route is 404 and new links are refused.
    let off =
        patch(json!({"share": {"enabled": false, "defaultExpiresDays": 7, "maxExpiresDays": 365}}))
            .await;
    assert_eq!(off.status, StatusCode::OK, "{}", off.json);
    assert_eq!(
        get(&h.app, &public, None).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&h.app, &format!("{public}/tree"), None).await.status,
        StatusCode::NOT_FOUND
    );
    let refused = with_json(&h.app, "POST", &links, json!({}), Some(&h.admin_cookie)).await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{}", refused.json);

    // Re-enabled with a narrower window: the default and the maximum apply.
    let narrow =
        patch(json!({"share": {"enabled": true, "defaultExpiresDays": 3, "maxExpiresDays": 30}}))
            .await;
    assert_eq!(narrow.status, StatusCode::OK, "{}", narrow.json);
    assert_eq!(get(&h.app, &public, None).await.status, StatusCode::OK);
    let too_long = with_json(
        &h.app,
        "POST",
        &links,
        json!({"expiresInDays": 31}),
        Some(&h.admin_cookie),
    )
    .await;
    assert_eq!(
        too_long.status,
        StatusCode::BAD_REQUEST,
        "{}",
        too_long.json
    );
    let defaulted = with_json(&h.app, "POST", &links, json!({}), Some(&h.admin_cookie)).await;
    assert_eq!(defaulted.status, StatusCode::CREATED, "{}", defaulted.json);
    let expires =
        chrono::DateTime::parse_from_rfc3339(defaulted.json["expiresAt"].as_str().unwrap())
            .unwrap()
            .with_timezone(&chrono::Utc);
    let days = (expires - chrono::Utc::now()).num_hours();
    assert!((71..=72).contains(&days), "default 3 days, got {days} h");
    h.finish().await;
}

async fn erase(h: &Harness, path: &str, user_id: Uuid, cookie: Option<&str>) -> Reply {
    with_json(
        &h.app,
        "POST",
        &format!("/api/v1/admin/users/{path}"),
        json!({ "userId": user_id }),
        cookie,
    )
    .await
}

async fn count_events(admin: &PgPool, verb: &str, target: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM fvoci.events WHERE verb = $1 AND target_id = $2")
        .bind(verb)
        .bind(target)
        .fetch_one(admin)
        .await
        .unwrap()
}

/// Source `scheduleUserErasure` / `cancelUserErasure({ actorAdminId })` and
/// admin-directory.test.ts: admin-only (404 otherwise), no cancel token in
/// the response, replay keeps the deadline, owner / last-admin rules, and a
/// cancel that clears the pending erasure without reviving credentials.
#[tokio::test]
async fn admin_erase_and_cancel_follow_the_source_rules() {
    let h = harness().await;
    let admin = h.db.admin().await;
    let acme = h.acme_id().await;
    let (victim_id, victim) = h.user("victim@example.com", Some("member")).await;
    let (member_id, member) = h.user("member@example.com", Some("member")).await;
    sqlx::query(
        r#"
        INSERT INTO fvoci.invitations (id, workspace_id, email, role, token_hash, invited_by, expires_at)
        VALUES ($1, $2, 'pending@example.com', 'member', 'victim-invite', $3, now() + interval '1 day')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(acme)
    .bind(victim_id)
    .execute(&admin)
    .await
    .unwrap();

    // Hidden from anonymous callers and non-admins; strict body.
    assert_eq!(
        erase(&h, "erase", victim_id, None).await.status,
        StatusCode::UNAUTHORIZED
    );
    for path in ["erase", "cancel-erase"] {
        let reply = erase(&h, path, victim_id, Some(&member)).await;
        assert_eq!(
            reply.status,
            StatusCode::NOT_FOUND,
            "{path}: {}",
            reply.json
        );
    }
    let extra = with_json(
        &h.app,
        "POST",
        "/api/v1/admin/users/erase",
        json!({ "userId": victim_id, "cancelToken": "x" }),
        Some(&h.admin_cookie),
    )
    .await;
    assert_eq!(extra.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        erase(&h, "erase", Uuid::now_v7(), Some(&h.admin_cookie))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        erase(&h, "cancel-erase", victim_id, Some(&h.admin_cookie))
            .await
            .status,
        StatusCode::NOT_FOUND,
        "nothing pending yet"
    );
    assert_eq!(count_events(&admin, "user.withdrawn", victim_id).await, 0);

    // Schedule: the admin sees the deadline, never the token.
    let before = Utc::now();
    let scheduled = erase(&h, "erase", victim_id, Some(&h.admin_cookie)).await;
    assert_eq!(scheduled.status, StatusCode::OK, "{}", scheduled.json);
    assert_eq!(scheduled.json["ok"], true);
    assert_eq!(scheduled.json["mailSent"], false);
    assert!(scheduled.json.get("cancelToken").is_none());
    let erase_at =
        chrono::DateTime::parse_from_rfc3339(scheduled.json["eraseAt"].as_str().unwrap())
            .unwrap()
            .with_timezone(&Utc);
    assert!(erase_at >= before + ChronoDuration::days(14));
    assert!(erase_at <= Utc::now() + ChronoDuration::days(14));
    // Credentials revoked and sent invitations removed in the same transaction.
    assert_eq!(
        get(&h.app, "/api/v1/auth/me", Some(&victim)).await.status,
        StatusCode::UNAUTHORIZED
    );
    let invites: i64 =
        sqlx::query_scalar("SELECT count(*) FROM fvoci.invitations WHERE invited_by = $1")
            .bind(victim_id)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(invites, 0);
    let (hash, deleted): (Option<String>, Option<chrono::DateTime<Utc>>) = sqlx::query_as(
        "SELECT withdraw_cancel_token_hash, deleted_at FROM fvoci.users WHERE id = $1",
    )
    .bind(victim_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert!(hash.is_some() && deleted.is_some());
    let (actor, channel): (Option<Uuid>, String) = sqlx::query_as(
        "SELECT actor_user_id, channel FROM fvoci.events WHERE verb = 'user.withdrawn' AND target_id = $1",
    )
    .bind(victim_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!((actor, channel.as_str()), (Some(h.admin_id), "web"));
    let audit_actor: Option<Uuid> = sqlx::query_scalar(
        "SELECT actor_user_id FROM fvoci.audit_log WHERE verb = 'user.withdrawn' AND target_id = $1",
    )
    .bind(victim_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(audit_actor, Some(h.admin_id));

    // The directory shows the pending erasure with the same deadline.
    let users = get(&h.app, "/api/v1/admin/users", Some(&h.admin_cookie)).await;
    let row = users.json["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == victim_id.to_string())
        .expect("victim row")
        .clone();
    assert!(row["deletedAt"].is_string());
    assert_eq!(row["eraseAt"], scheduled.json["eraseAt"]);

    // Replay: same deadline, no new token, no second event.
    let replay = erase(&h, "erase", victim_id, Some(&h.admin_cookie)).await;
    assert_eq!(replay.status, StatusCode::OK, "{}", replay.json);
    assert_eq!(replay.json["eraseAt"], scheduled.json["eraseAt"]);
    assert_eq!(replay.json["mailSent"], false);
    let hash_after: Option<String> =
        sqlx::query_scalar("SELECT withdraw_cancel_token_hash FROM fvoci.users WHERE id = $1")
            .bind(victim_id)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(hash_after, hash);
    assert_eq!(count_events(&admin, "user.withdrawn", victim_id).await, 1);

    // Cancel: restores the row and drops the one-time cancel hash; the old
    // session stays revoked. A replay finds nothing pending.
    let cancelled = erase(&h, "cancel-erase", victim_id, Some(&h.admin_cookie)).await;
    assert_eq!(cancelled.status, StatusCode::OK, "{}", cancelled.json);
    assert_eq!(cancelled.json, json!({ "ok": true }));
    let (hash, deleted): (Option<String>, Option<chrono::DateTime<Utc>>) = sqlx::query_as(
        "SELECT withdraw_cancel_token_hash, deleted_at FROM fvoci.users WHERE id = $1",
    )
    .bind(victim_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert!(hash.is_none() && deleted.is_none());
    assert_eq!(
        get(&h.app, "/api/v1/auth/me", Some(&victim)).await.status,
        StatusCode::UNAUTHORIZED
    );
    let replay = erase(&h, "cancel-erase", victim_id, Some(&h.admin_cookie)).await;
    assert_eq!(replay.status, StatusCode::NOT_FOUND);
    let (actor,): (Option<Uuid>,) = sqlx::query_as(
        "SELECT actor_user_id FROM fvoci.events WHERE verb = 'user.withdraw_cancelled' AND target_id = $1",
    )
    .bind(victim_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(actor, Some(h.admin_id));
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM fvoci.audit_log WHERE verb = 'user.withdraw_cancelled' AND target_id = $1",
    )
    .bind(victim_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(audits, 1);

    // Past the 14-day grace period the cancel is a 409 conflict.
    let again = erase(&h, "erase", victim_id, Some(&h.admin_cookie)).await;
    assert_eq!(again.status, StatusCode::OK, "{}", again.json);
    sqlx::query(
        "UPDATE fvoci.users SET deleted_at = now() - interval '14 days' - interval '1 minute' WHERE id = $1",
    )
    .bind(victim_id)
    .execute(&admin)
    .await
    .unwrap();
    let late = erase(&h, "cancel-erase", victim_id, Some(&h.admin_cookie)).await;
    assert_eq!(late.status, StatusCode::CONFLICT, "{}", late.json);
    assert_eq!(late.json["code"], "conflict");
    // Anonymized rows are gone for both operations.
    sqlx::query("UPDATE fvoci.users SET anonymized_at = now() WHERE id = $1")
        .bind(victim_id)
        .execute(&admin)
        .await
        .unwrap();
    for path in ["erase", "cancel-erase"] {
        let reply = erase(&h, path, victim_id, Some(&h.admin_cookie)).await;
        assert_eq!(reply.status, StatusCode::NOT_FOUND, "{path}");
    }

    // A team owner must transfer ownership first.
    sqlx::query("UPDATE fvoci.memberships SET role = 'owner' WHERE user_id = $1")
        .bind(member_id)
        .execute(&admin)
        .await
        .unwrap();
    let owner = erase(&h, "erase", member_id, Some(&h.admin_cookie)).await;
    assert_eq!(owner.status, StatusCode::CONFLICT);
    assert_eq!(owner.json["code"], "owner_transfer_required");

    // The last live instance admin cannot be scheduled (here: itself, once it
    // no longer owns the team workspace).
    sqlx::query("UPDATE fvoci.memberships SET role = 'admin' WHERE user_id = $1")
        .bind(h.admin_id)
        .execute(&admin)
        .await
        .unwrap();
    let last = erase(&h, "erase", h.admin_id, Some(&h.admin_cookie)).await;
    assert_eq!(last.status, StatusCode::CONFLICT, "{}", last.json);
    assert_eq!(last.json["code"], "last_instance_admin");
    assert_eq!(count_events(&admin, "user.withdrawn", h.admin_id).await, 0);
    admin.close().await;
    h.finish().await;
}

/// The admin cancel definer is its own narrow path: it refuses callers
/// without a live admin self context and rechecks the grace period with the
/// database clock; the user's cancel-hash definer still requires the hash.
#[tokio::test]
async fn admin_restore_definer_requires_a_live_admin_and_an_open_grace_period() {
    let h = harness().await;
    let admin = h.db.admin().await;
    let (victim_id, _) = h.user("victim@example.com", None).await;
    let (member_id, _) = h.user("member@example.com", None).await;
    sqlx::query(
        "UPDATE fvoci.users SET deleted_at = now() - interval '1 day', withdraw_cancel_token_hash = $2 WHERE id = $1",
    )
    .bind(victim_id)
    .bind("a".repeat(64))
    .execute(&admin)
    .await
    .unwrap();
    let app = pool::connect_app(&h.db.app_url).await.unwrap();

    let code = |err: sqlx::Error| {
        err.as_database_error()
            .unwrap()
            .code()
            .map(|c| c.to_string())
    };
    let mut tx = app.begin().await.unwrap();
    let refused =
        sqlx::query_scalar::<_, bool>("SELECT fvoci.app_admin_user_restore_withdrawn($1)")
            .bind(victim_id)
            .fetch_one(&mut *tx)
            .await;
    assert_eq!(code(refused.unwrap_err()).as_deref(), Some("42501"));
    tx.rollback().await.unwrap();

    let mut tx = app.begin().await.unwrap();
    fvoci_server::db::context::set_self_user(&mut tx, member_id)
        .await
        .unwrap();
    let refused =
        sqlx::query_scalar::<_, bool>("SELECT fvoci.app_admin_user_restore_withdrawn($1)")
            .bind(victim_id)
            .fetch_one(&mut *tx)
            .await;
    assert_eq!(code(refused.unwrap_err()).as_deref(), Some("42501"));
    tx.rollback().await.unwrap();

    // The user's own definer still needs the matching hash.
    let restored: bool = sqlx::query_scalar("SELECT fvoci.app_user_restore_withdrawn($1, $2)")
        .bind(victim_id)
        .bind("b".repeat(64))
        .fetch_one(&app)
        .await
        .unwrap();
    assert!(!restored);

    // Past the deadline even an admin context restores nothing.
    sqlx::query(
        "UPDATE fvoci.users SET deleted_at = now() - interval '14 days 1 second' WHERE id = $1",
    )
    .bind(victim_id)
    .execute(&admin)
    .await
    .unwrap();
    let mut tx = app.begin().await.unwrap();
    fvoci_server::db::context::set_self_user(&mut tx, h.admin_id)
        .await
        .unwrap();
    let restored: bool = sqlx::query_scalar("SELECT fvoci.app_admin_user_restore_withdrawn($1)")
        .bind(victim_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert!(!restored);
    tx.rollback().await.unwrap();

    sqlx::query("UPDATE fvoci.users SET deleted_at = now() - interval '13 days' WHERE id = $1")
        .bind(victim_id)
        .execute(&admin)
        .await
        .unwrap();
    let mut tx = app.begin().await.unwrap();
    fvoci_server::db::context::set_self_user(&mut tx, h.admin_id)
        .await
        .unwrap();
    let restored: bool = sqlx::query_scalar("SELECT fvoci.app_admin_user_restore_withdrawn($1)")
        .bind(victim_id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert!(restored);
    tx.commit().await.unwrap();
    let (hash, deleted): (Option<String>, Option<chrono::DateTime<Utc>>) = sqlx::query_as(
        "SELECT withdraw_cancel_token_hash, deleted_at FROM fvoci.users WHERE id = $1",
    )
    .bind(victim_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert!(hash.is_none() && deleted.is_none());
    app.close().await;
    admin.close().await;
    h.finish().await;
}

/// The user's token cancel and the admin cancel take the same locks: with
/// the target row held, both queue; after release exactly one restores the
/// account and the other finds nothing pending. One cancel event.
#[tokio::test]
async fn user_and_admin_cancel_race_restores_once() {
    let h = harness().await;
    let admin = h.db.admin().await;
    let (victim_id, victim) = h.user("victim@example.com", None).await;
    let withdrawn = with_json(
        &h.app,
        "POST",
        "/api/v1/auth/withdraw",
        json!({ "currentPassword": "supersecret1" }),
        Some(&victim),
    )
    .await;
    assert_eq!(withdrawn.status, StatusCode::OK, "{}", withdrawn.json);
    let token = withdrawn.json["cancelToken"].as_str().unwrap().to_string();

    let mut holder = admin.begin().await.unwrap();
    sqlx::query("SELECT id FROM fvoci.users WHERE id = $1 FOR UPDATE")
        .bind(victim_id)
        .execute(&mut *holder)
        .await
        .unwrap();
    let app = h.app.clone();
    let user_cancel = tokio::spawn(async move {
        with_json(
            &app,
            "POST",
            "/api/v1/auth/cancel-withdraw",
            json!({ "token": token }),
            None,
        )
        .await
    });
    wait_for_users_lock_waiter(&admin).await;
    let app = h.app.clone();
    let cookie = h.admin_cookie.clone();
    let admin_cancel = tokio::spawn(async move {
        with_json(
            &app,
            "POST",
            "/api/v1/admin/users/cancel-erase",
            json!({ "userId": victim_id }),
            Some(&cookie),
        )
        .await
    });
    // The admin request queues behind the admission lock the user cancel holds.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let waiting: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity WHERE datname = current_database() AND wait_event_type = 'Lock' AND state = 'active'",
        )
        .fetch_one(&admin)
        .await
        .unwrap();
        if waiting >= 2 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "admin cancel never queued"
        );
        tokio::task::yield_now().await;
    }
    holder.commit().await.unwrap();
    let (user_reply, admin_reply) = (user_cancel.await.unwrap(), admin_cancel.await.unwrap());
    let statuses = [user_reply.status, admin_reply.status];
    assert_eq!(
        statuses.iter().filter(|s| **s == StatusCode::OK).count(),
        1,
        "{} / {}",
        user_reply.json,
        admin_reply.json
    );
    assert!(statuses.contains(&StatusCode::NOT_FOUND));
    assert_eq!(
        count_events(&admin, "user.withdraw_cancelled", victim_id).await,
        1
    );
    let deleted: Option<chrono::DateTime<Utc>> =
        sqlx::query_scalar("SELECT deleted_at FROM fvoci.users WHERE id = $1")
            .bind(victim_id)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert!(deleted.is_none());
    admin.close().await;
    h.finish().await;
}

/// Two admins scheduling each other's erasure at once: the admin check runs
/// under the locks, so the second sees its actor withdrawn and gets 404. One
/// live admin always remains.
#[tokio::test]
async fn concurrent_mutual_erasure_leaves_one_live_admin() {
    let h = harness().await;
    let (second_id, second) = h.user("second@example.com", None).await;
    let promote = with_json(
        &h.app,
        "PATCH",
        "/api/v1/admin/users",
        json!({"userId": second_id, "instanceAdmin": true}),
        Some(&h.admin_cookie),
    )
    .await;
    assert_eq!(promote.status, StatusCode::OK);
    // The setup admin owns the team workspace; hand it to someone else so
    // both erasures are allowed on their own.
    h.user("owner@example.com", Some("owner")).await;
    let admin = h.db.admin().await;
    sqlx::query("UPDATE fvoci.memberships SET role = 'admin' WHERE user_id = $1")
        .bind(h.admin_id)
        .execute(&admin)
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        erase(&h, "erase", second_id, Some(&h.admin_cookie)),
        erase(&h, "erase", h.admin_id, Some(&second)),
    );
    let statuses = [a.status, b.status];
    assert_eq!(
        statuses.iter().filter(|s| **s == StatusCode::OK).count(),
        1,
        "{} / {}",
        a.json,
        b.json
    );
    assert!(
        statuses.contains(&StatusCode::NOT_FOUND),
        "{} / {}",
        a.json,
        b.json
    );
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM fvoci.users WHERE is_instance_admin AND deleted_at IS NULL AND suspended_at IS NULL",
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(live, 1);
    admin.close().await;
    h.finish().await;
}

/// `fvoci-migrate --verify-storage` also covers branding assets: each one the
/// instance settings reference must exist with its recorded SHA-256.
#[tokio::test]
async fn verify_storage_covers_branding_assets() {
    let h = harness().await;
    let storage: fvoci_server::attachments::ObjectStorage =
        fvoci_server::attachments::LocalStorage::new(h.storage_root.clone()).into();
    let pool = pool::connect_app(&h.db.app_url).await.unwrap();
    let report = fvoci_server::attachments::verify_stored_objects(&pool, &storage)
        .await
        .unwrap();
    assert_eq!(report.branding_checked, 0);
    assert!(report.is_complete());

    let mut keys = Vec::new();
    for kind in ["logo", "favicon"] {
        let reply = send(
            &h.app,
            "POST",
            &format!("/api/v1/admin/branding/assets/{kind}"),
            Some(("application/octet-stream", tiny_png())),
            Some(&h.admin_cookie),
            &[],
        )
        .await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.json);
        keys.push(
            reply.json["values"]["branding"][kind]["key"]
                .as_str()
                .unwrap()
                .to_string(),
        );
    }
    let report = fvoci_server::attachments::verify_stored_objects(&pool, &storage)
        .await
        .unwrap();
    assert_eq!(report.branding_checked, 2);
    assert!(report.is_complete(), "{report:?}");
    let printed = serde_json::to_value(&report).unwrap();
    assert_eq!(printed["brandingChecked"], 2);
    assert_eq!(printed["brandingMissing"], json!([]));

    // Same length, different bytes: the digest catches it.
    let mut other = tiny_png();
    let last = other.len() - 1;
    other[last] ^= 0xff;
    storage.delete_object(&keys[0]).await.unwrap();
    storage.put_bytes(&keys[0], other).await.unwrap();
    storage.delete_object(&keys[1]).await.unwrap();
    let report = fvoci_server::attachments::verify_stored_objects(&pool, &storage)
        .await
        .unwrap();
    assert_eq!(report.branding_mismatch, vec!["logo"]);
    assert_eq!(report.branding_missing, vec!["favicon"]);
    assert!(!report.is_complete());
    pool.close().await;
    h.finish().await;
}
