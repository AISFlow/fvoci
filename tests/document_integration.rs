#![cfg(feature = "db-tests")]

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use chrono::{Duration as ChronoDuration, Utc};
use fvoci_server::auth::password::Keyring;
use fvoci_server::auth::token::hash_token;
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

        apply_grants(&migration_pool, &role_name).await;
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

async fn apply_grants(pool: &PgPool, role_name: &str) {
    fvoci_server::db::migrate::apply_app_role_grants(pool, role_name)
        .await
        .expect("grant");
}

async fn app_state(app_url: &str) -> AppState {
    let pool = pool::connect_app(app_url).await.expect("app pool");
    let storage_root = std::env::temp_dir().join(format!("fvoci-doc-test-{}", Uuid::now_v7()));
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
        markdown: Some(
            fvoci_server::documents::markdown_helper::MarkdownHelper::new(env!(
                "CARGO_BIN_EXE_fvoci-server"
            )),
        ),
        import_wake: None,
        import_extractor_available: false,
        quota: Default::default(),
        mailer: std::sync::Arc::new(fvoci_server::mail::Mailer::disabled()),
    }
}

fn document_app(state: AppState) -> axum::Router {
    fvoci_server::http::router(state, None)
}

async fn json_request(
    app: axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    cookie: Option<&str>,
    extra_headers: &[(&str, &str)],
) -> (StatusCode, Value, Option<String>, HeaderMap) {
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
        .insert(axum::extract::ConnectInfo(test_peer()));
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

async fn setup_session(harness: &TestDb) -> (axum::Router, String, Uuid, Uuid) {
    let app = document_app(app_state(&harness.app_url).await);
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
    (app, cookie, ids.0, ids.1)
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

async fn wait_for_users_for_update(admin: &PgPool, blocker_pid: i32) -> i32 {
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
        if let Some(pid) = blocked {
            return pid;
        }
        tokio::task::yield_now().await;
    }
    panic!("document write FOR UPDATE did not block on shared user lock");
}

async fn wait_for_advisory_blocked_by(admin: &PgPool, blocker_pid: i32) -> i32 {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let blocked: Option<i32> = sqlx::query_scalar(
            r"
            SELECT activity.pid
            FROM pg_stat_activity AS activity
            WHERE activity.wait_event_type = 'Lock'
              AND activity.state = 'active'
              AND activity.query ILIKE '%pg_advisory_xact_lock%'
              AND $1 = ANY(pg_blocking_pids(activity.pid))
            LIMIT 1
            ",
        )
        .bind(blocker_pid)
        .fetch_optional(admin)
        .await
        .unwrap();
        if let Some(pid) = blocked {
            return pid;
        }
        tokio::task::yield_now().await;
    }
    panic!("operation did not block on shared membership advisory lock held by {blocker_pid}");
}

fn assert_iso_date(value: &Value) {
    let raw = value.as_str().expect("date string");
    chrono::DateTime::parse_from_rfc3339(raw).expect("ISO date");
}

#[tokio::test]
async fn document_create_get_tree_parent_rename_status_and_nulls() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id) = setup_session(&harness).await;

    let (status, body, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"parentId": null, "title": "  Root  "})),
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["title"], "Root");
    assert_eq!(body["icon"], Value::Null);
    assert_eq!(body["parentId"], Value::Null);
    assert_eq!(body["projectId"], Value::Null);
    assert_eq!(body["displayId"], "WIKI-1");
    assert_eq!(body["number"], 1);
    assert_eq!(body["status"], "draft");
    assert_eq!(body["schemaVersion"], 2);
    assert_eq!(body["version"], 1);
    assert_eq!(body["createdBy"], owner_id.to_string());
    assert_eq!(body["sortKey"], "V");
    assert_iso_date(&body["createdAt"]);
    assert_iso_date(&body["updatedAt"]);
    let root_id = body["id"].as_str().unwrap().to_string();
    assert_eq!(body["path"], root_id.replace('-', ""));

    let (status, body, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"parentId": root_id, "title": "Child", "icon": "📄"})),
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["displayId"], "WIKI-2");
    assert_eq!(body["parentId"], root_id);
    assert_eq!(body["icon"], "📄");
    assert_eq!(body["sortKey"], "V");
    let child_id = body["id"].as_str().unwrap().to_string();
    assert!(body["path"]
        .as_str()
        .unwrap()
        .starts_with(&format!("{}.", root_id.replace('-', ""))));

    let (status, body, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"parentId": null, "title": "Second root"})),
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["displayId"], "WIKI-3");
    assert_eq!(body["sortKey"], "W");
    let second_root_id = body["id"].as_str().unwrap().to_string();

    let (status, body, _, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{root_id}"),
        None,
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.get("displayId").is_none());
    assert_eq!(body["icon"], Value::Null);
    assert_eq!(body["parentId"], Value::Null);
    assert_eq!(body["projectId"], Value::Null);

    let (status, body, _, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/tree"),
        None,
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    let root_node = items.iter().find(|n| n["id"] == root_id).unwrap();
    let child_node = items.iter().find(|n| n["id"] == child_id).unwrap();
    let second_root = items.iter().find(|n| n["id"] == second_root_id).unwrap();
    assert_eq!(root_node["icon"], Value::Null);
    assert_eq!(root_node["parentId"], Value::Null);
    assert_eq!(root_node["sortKey"], "V");
    assert_eq!(child_node["parentId"], root_id);
    assert_eq!(second_root["sortKey"], "W");
    assert_eq!(second_root["parentId"], Value::Null);

    let (status, body, _, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{child_id}/ancestors"),
        None,
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    assert_eq!(body["items"][0]["id"], root_id);
    assert_eq!(body["items"][0]["icon"], Value::Null);
    assert_eq!(body["items"][0]["projectId"], Value::Null);

    let (status, body, _, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{root_id}/body"),
        None,
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["contentJson"],
        json!({"type":"doc","content":[{"type":"paragraph"}]})
    );
    assert_eq!(body["version"], 1);

    let (status, body, _, _) = json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{root_id}"),
        Some(json!({"title": "Renamed", "icon": null, "status": "published"})),
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["title"], "Renamed");
    assert_eq!(body["icon"], Value::Null);
    assert_eq!(body["status"], "published");
    assert_eq!(body["version"], 1);
    assert!(body.get("displayId").is_none());

    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let events: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.events WHERE verb = 'document.created' AND workspace_id = $1",
    )
    .bind(workspace_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(events.0, 3);
    let audits: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.audit_log WHERE verb IN ('document.created', 'document.updated') AND workspace_id = $1",
    )
    .bind(workspace_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(audits.0, 4);
    let next_number: (i32,) =
        sqlx::query_as("SELECT next_document_number FROM fvoci.workspaces WHERE id = $1")
            .bind(workspace_id)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(next_number.0, 3);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn guest_tree_is_empty_and_get_create_are_not_found() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, _, workspace_id) = setup_session(&harness).await;
    let (status, body, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"parentId": null, "title": "Hidden"})),
        Some(&owner_cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let doc_id = body["id"].as_str().unwrap().to_string();
    let (guest_id, guest_cookie) =
        create_second_user_session(&harness, "guest@example.com", "Guest").await;
    let app_pool = pool::connect_app(&harness.app_url).await.unwrap();
    fvoci_server::db::workspace::add_membership_for_test(
        &app_pool,
        workspace_id,
        guest_id,
        fvoci_server::db::workspace::WorkspaceRole::Guest,
    )
    .await
    .unwrap();
    app_pool.close().await;

    let (status, body, _, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/tree"),
        None,
        Some(&guest_cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["items"].as_array().unwrap().len(), 0);

    let (status, body, _, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{doc_id}"),
        None,
        Some(&guest_cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");

    let (status, body, _, _) = json_request(
        app,
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"parentId": null, "title": "Guest write"})),
        Some(&guest_cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");
    harness.cleanup().await;
}

#[tokio::test]
async fn member_can_create_and_instance_admin_without_membership_cannot() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let (member_id, member_cookie) =
        create_second_user_session(&harness, "member@example.com", "Member").await;
    let app_pool = pool::connect_app(&harness.app_url).await.unwrap();
    fvoci_server::db::workspace::add_membership_for_test(
        &app_pool,
        workspace_id,
        member_id,
        fvoci_server::db::workspace::WorkspaceRole::Member,
    )
    .await
    .unwrap();
    app_pool.close().await;

    let (status, body, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"parentId": null, "title": "Member doc"})),
        Some(&member_cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["displayId"], "WIKI-1");

    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    sqlx::query("DELETE FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2")
        .bind(workspace_id)
        .bind(owner_id)
        .execute(&admin)
        .await
        .unwrap();
    let gone: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(workspace_id)
    .bind(owner_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(gone.0, 0);
    admin.close().await;

    let (status, body, _, _) = json_request(
        app,
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"parentId": null, "title": "Admin override"})),
        Some(&owner_cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");
    harness.cleanup().await;
}

#[tokio::test]
async fn foreign_parent_affiliation_depth_and_unsupported_queries_are_rejected() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let (status, _, _, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/workspaces",
        Some(json!({"name": "Other", "slug": "other-ws"})),
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let other_ws: (Uuid,) =
        sqlx::query_as("SELECT id FROM fvoci.workspaces WHERE slug = 'other-ws'")
            .fetch_one(&admin)
            .await
            .unwrap();
    let foreign_parent = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO fvoci.documents (
            id, workspace_id, title, path, parent_id, sort_key, number, status,
            schema_version, content_json, created_by
        ) VALUES (
            $1, $2, 'Foreign', $3, NULL, 'V', 1, 'draft', 2, '{"type":"doc"}'::jsonb, $4
        )
        "#,
    )
    .bind(foreign_parent)
    .bind(other_ws.0)
    .bind(foreign_parent.simple().to_string())
    .bind(owner_id)
    .execute(&admin)
    .await
    .unwrap();

    let (status, body, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"parentId": foreign_parent, "title": "Cross"})),
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");

    let (status, body, _, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{foreign_parent}"),
        None,
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");

    let affiliated = Uuid::now_v7();
    let affiliated_project = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO fvoci.projects (
            id, workspace_id, key, name, visibility, status, next_number, created_by
        ) VALUES ($1, $2, 'PRJ', 'Projectish', 'private', 'active', 1, $3)
        "#,
    )
    .bind(affiliated_project)
    .bind(workspace_id)
    .bind(owner_id)
    .execute(&admin)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO fvoci.documents (
            id, workspace_id, title, path, parent_id, sort_key, project_id, number, status,
            schema_version, content_json, created_by
        ) VALUES (
            $1, $2, 'Projectish', $3, NULL, 'W', $4, 2, 'draft', 2, '{"type":"doc"}'::jsonb, $5
        )
        "#,
    )
    .bind(affiliated)
    .bind(workspace_id)
    .bind(affiliated.simple().to_string())
    .bind(affiliated_project)
    .bind(owner_id)
    .execute(&admin)
    .await
    .unwrap();

    let (status, body, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"parentId": affiliated, "title": "Mismatch"})),
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "document_affiliation_mismatch");

    let (status, body, _, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{affiliated}"),
        None,
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");

    let mut parent = None;
    let mut last_id = String::new();
    for depth in 1..=20 {
        let (status, body, _, _) = json_request(
            app.clone(),
            "POST",
            &format!("/api/v1/workspaces/{workspace_id}/documents"),
            Some(json!({"parentId": parent, "title": format!("D{depth}")})),
            Some(&cookie),
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "depth {depth}");
        last_id = body["id"].as_str().unwrap().to_string();
        parent = Some(last_id.clone());
    }
    let (status, body, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"parentId": last_id, "title": "Too deep"})),
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "tree_depth_limit");
    assert_eq!(body["params"]["limit"], 20);

    let (status, body, _, _) = json_request(
        app.clone(),
        "GET",
        &format!(
            "/api/v1/workspaces/{workspace_id}/tree?tag={}",
            Uuid::now_v7()
        ),
        None,
        Some(&cookie),
        &[],
    )
    .await;
    // An unknown tag narrows the tree to nothing; a malformed one is rejected.
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["items"], json!([]));

    let (status, body, _, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/tree?tag=not-a-uuid"),
        None,
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "invalid_input");

    let (status, body, _, _) = json_request(
        app.clone(),
        "GET",
        // `format=md` is supported (document_api_integration); other formats are not.
        &format!("/api/v1/workspaces/{workspace_id}/documents/{last_id}/body?format=html"),
        None,
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "invalid_input");

    let (status, _, _, _) = json_request(
        app.clone(),
        "GET",
        &format!(
            "/api/v1/workspaces/{workspace_id}/projects/{}/documents",
            Uuid::now_v7()
        ),
        None,
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn invalid_auth_and_input_are_source_errors() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;

    let (created, document, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"parentId": null, "title": "Null regression"})),
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(created, StatusCode::CREATED);
    for field in ["title", "status"] {
        let (status, body, _, _) = json_request(
            app.clone(),
            "PATCH",
            &format!(
                "/api/v1/workspaces/{workspace_id}/documents/{}",
                document["id"].as_str().unwrap()
            ),
            Some(json!({field: null})),
            Some(&cookie),
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["code"], "invalid_input");
    }

    let (status, body, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"parentId": null, "title": "Nope"})),
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], "authentication_required");

    let (status, body, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"parentId": Uuid::now_v7(), "title": "Missing parent"})),
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");

    let (status, body, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"title": "Missing parentId"})),
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "invalid_input");

    let (status, body, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"parentId": null, "title": ""})),
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "invalid_input");

    let (status, body, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"parentId": null, "title": "X"})),
        Some(&cookie),
        &[("origin", "http://evil.example.com")],
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "origin_mismatch");

    let (status, body, _, _) = json_request(
        app,
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/tree"),
        None,
        Some(&cookie),
        &[("authorization", "Bearer nope")],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["items"].as_array().is_some());
    harness.cleanup().await;
}

#[tokio::test]
async fn patch_icon_set_omit_preserves_then_null_clears() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let (status, body, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"parentId": null, "title": "Iconed", "icon": "📄"})),
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["icon"], "📄");
    let doc_id = body["id"].as_str().unwrap().to_string();

    let (status, body, _, _) = json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{doc_id}"),
        Some(json!({"title": "Still iconed"})),
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["title"], "Still iconed");
    assert_eq!(body["icon"], "📄");

    let (status, body, _, _) = json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{doc_id}"),
        Some(json!({"icon": null})),
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["icon"], Value::Null);

    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let stored: (Option<String>,) =
        sqlx::query_as("SELECT icon FROM fvoci.documents WHERE id = $1")
            .bind(Uuid::parse_str(&doc_id).unwrap())
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(stored.0, None);
    let payload: (Value,) = sqlx::query_as(
        "SELECT payload FROM fvoci.events WHERE verb = 'document.updated' AND target_id = $1 ORDER BY seq DESC LIMIT 1",
    )
    .bind(Uuid::parse_str(&doc_id).unwrap())
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(payload.0["icon"], Value::Null);
    assert!(payload.0.get("title").is_none());
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn invalid_stored_sort_key_returns_internal_error() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let bad_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO fvoci.documents (
            id, workspace_id, title, path, parent_id, sort_key, number, status,
            schema_version, content_json, created_by
        ) VALUES (
            $1, $2, 'Corrupt', $3, NULL, 'A0', 1, 'draft', 2, '{"type":"doc"}'::jsonb, $4
        )
        "#,
    )
    .bind(bad_id)
    .bind(workspace_id)
    .bind(bad_id.simple().to_string())
    .bind(owner_id)
    .execute(&admin)
    .await
    .unwrap();
    sqlx::query("UPDATE fvoci.workspaces SET next_document_number = 1 WHERE id = $1")
        .bind(workspace_id)
        .execute(&admin)
        .await
        .unwrap();

    let (status, body, _, _) = json_request(
        app,
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"parentId": null, "title": "Sibling of corrupt"})),
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body["code"], "internal_error");
    let docs: (i64,) =
        sqlx::query_as("SELECT count(*) FROM fvoci.documents WHERE workspace_id = $1 AND id <> $2")
            .bind(workspace_id)
            .bind(bad_id)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(docs.0, 0);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn product_membership_revoke_races_document_write_under_lock_barrier() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let (member_a, member_a_cookie) =
        create_second_user_session(&harness, "writer-a@example.com", "WriterA").await;
    let (member_b, member_b_cookie) =
        create_second_user_session(&harness, "writer-b@example.com", "WriterB").await;
    let app_pool = pool::connect_app(&harness.app_url).await.unwrap();
    for id in [member_a, member_b] {
        fvoci_server::db::workspace::add_membership_for_test(
            &app_pool,
            workspace_id,
            id,
            fvoci_server::db::workspace::WorkspaceRole::Member,
        )
        .await
        .unwrap();
    }
    app_pool.close().await;

    let admin = PgPoolOptions::new()
        .max_connections(4)
        .connect(&harness.admin_url)
        .await
        .unwrap();

    let mut demote_barrier = admin.begin().await.unwrap();
    let demote_blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *demote_barrier)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM fvoci.users WHERE id = $1 FOR UPDATE")
        .bind(owner_id)
        .execute(&mut *demote_barrier)
        .await
        .unwrap();
    let demote = tokio::spawn({
        let app = app.clone();
        let owner_cookie = owner_cookie.clone();
        async move {
            json_request(
                app,
                "PATCH",
                &format!("/api/v1/workspaces/{workspace_id}/members/{member_a}"),
                Some(json!({"role": "guest"})),
                Some(&owner_cookie),
                &[],
            )
            .await
        }
    });
    let demote_pid = wait_for_users_for_update(&admin, demote_blocker_pid).await;
    let create_denied = tokio::spawn({
        let app = app.clone();
        let member_a_cookie = member_a_cookie.clone();
        async move {
            json_request(
                app,
                "POST",
                &format!("/api/v1/workspaces/{workspace_id}/documents"),
                Some(json!({"parentId": null, "title": "After product demote"})),
                Some(&member_a_cookie),
                &[],
            )
            .await
        }
    });
    wait_for_advisory_blocked_by(&admin, demote_pid).await;
    demote_barrier.commit().await.unwrap();
    let (demote_status, demote_body, _, _) = demote.await.unwrap();
    assert_eq!(demote_status, StatusCode::OK);
    assert_eq!(demote_body["role"], "guest");
    let (create_status, create_body, _, _) = create_denied.await.unwrap();
    assert_eq!(create_status, StatusCode::NOT_FOUND);
    assert_eq!(create_body["code"], "not_found");
    let docs_a: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.documents WHERE workspace_id = $1 AND created_by = $2",
    )
    .bind(workspace_id)
    .bind(member_a)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(docs_a.0, 0);
    let role_a: (String,) = sqlx::query_as(
        "SELECT role FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(workspace_id)
    .bind(member_a)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(role_a.0, "guest");

    let mut write_barrier = admin.begin().await.unwrap();
    let write_blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *write_barrier)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM fvoci.users WHERE id = $1 FOR UPDATE")
        .bind(member_b)
        .execute(&mut *write_barrier)
        .await
        .unwrap();
    let create_first = tokio::spawn({
        let app = app.clone();
        let member_b_cookie = member_b_cookie.clone();
        async move {
            json_request(
                app,
                "POST",
                &format!("/api/v1/workspaces/{workspace_id}/documents"),
                Some(json!({"parentId": null, "title": "Before product remove"})),
                Some(&member_b_cookie),
                &[],
            )
            .await
        }
    });
    let create_pid = wait_for_users_for_update(&admin, write_blocker_pid).await;
    let remove = tokio::spawn({
        let app = app.clone();
        let owner_cookie = owner_cookie.clone();
        async move {
            json_request(
                app,
                "DELETE",
                &format!("/api/v1/workspaces/{workspace_id}/members/{member_b}"),
                None,
                Some(&owner_cookie),
                &[],
            )
            .await
        }
    });
    wait_for_advisory_blocked_by(&admin, create_pid).await;
    write_barrier.commit().await.unwrap();
    let (create_status, create_body, _, _) = create_first.await.unwrap();
    assert_eq!(create_status, StatusCode::CREATED);
    assert_eq!(create_body["title"], "Before product remove");
    let (remove_status, _, _, _) = remove.await.unwrap();
    assert_eq!(remove_status, StatusCode::OK);
    let docs_b: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.documents WHERE workspace_id = $1 AND created_by = $2",
    )
    .bind(workspace_id)
    .bind(member_b)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(docs_b.0, 1);
    let remaining_b: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(workspace_id)
    .bind(member_b)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(remaining_b.0, 0);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn suspend_and_guest_demotion_deny_write_after_shared_locks() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let (member_id, member_cookie) =
        create_second_user_session(&harness, "demote@example.com", "Demote").await;
    let app_pool = pool::connect_app(&harness.app_url).await.unwrap();
    fvoci_server::db::workspace::add_membership_for_test(
        &app_pool,
        workspace_id,
        member_id,
        fvoci_server::db::workspace::WorkspaceRole::Member,
    )
    .await
    .unwrap();
    app_pool.close().await;

    let admin = PgPoolOptions::new()
        .max_connections(3)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE fvoci.memberships SET role = 'guest', updated_at = now() WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(workspace_id)
    .bind(member_id)
    .execute(&admin)
    .await
    .unwrap();
    let role: (String,) = sqlx::query_as(
        "SELECT role FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(workspace_id)
    .bind(member_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(role.0, "guest");
    let (status, body, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"parentId": null, "title": "After demote"})),
        Some(&member_cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");

    let mut admin_tx = admin.begin().await.unwrap();
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *admin_tx)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM fvoci.users WHERE id = $1 FOR UPDATE")
        .bind(owner_id)
        .execute(&mut *admin_tx)
        .await
        .unwrap();
    let create = tokio::spawn({
        let app = app.clone();
        let owner_cookie = owner_cookie.clone();
        async move {
            json_request(
                app,
                "POST",
                &format!("/api/v1/workspaces/{workspace_id}/documents"),
                Some(json!({"parentId": null, "title": "After suspend"})),
                Some(&owner_cookie),
                &[],
            )
            .await
        }
    });
    wait_for_users_for_update(&admin, blocker_pid).await;
    sqlx::query("UPDATE fvoci.users SET suspended_at = now() WHERE id = $1")
        .bind(owner_id)
        .execute(&mut *admin_tx)
        .await
        .unwrap();
    let suspended: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.users WHERE id = $1 AND suspended_at IS NOT NULL",
    )
    .bind(owner_id)
    .fetch_one(&mut *admin_tx)
    .await
    .unwrap();
    assert_eq!(suspended.0, 1);
    admin_tx.commit().await.unwrap();
    let (status, body, _, _) = create.await.unwrap();
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");
    let docs: (i64,) =
        sqlx::query_as("SELECT count(*) FROM fvoci.documents WHERE workspace_id = $1")
            .bind(workspace_id)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(docs.0, 0);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn membership_removal_and_session_revoke_share_locks_before_write() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let (member_id, member_cookie) =
        create_second_user_session(&harness, "writer@example.com", "Writer").await;
    let app_pool = pool::connect_app(&harness.app_url).await.unwrap();
    fvoci_server::db::workspace::add_membership_for_test(
        &app_pool,
        workspace_id,
        member_id,
        fvoci_server::db::workspace::WorkspaceRole::Member,
    )
    .await
    .unwrap();
    app_pool.close().await;

    let admin = PgPoolOptions::new()
        .max_connections(3)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    sqlx::query("DELETE FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2")
        .bind(workspace_id)
        .bind(member_id)
        .execute(&admin)
        .await
        .unwrap();
    let remaining: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(workspace_id)
    .bind(member_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(remaining.0, 0);

    let (status, body, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"parentId": null, "title": "After remove"})),
        Some(&member_cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");

    let mut admin_tx = admin.begin().await.unwrap();
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *admin_tx)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM fvoci.users WHERE id = $1 FOR UPDATE")
        .bind(owner_id)
        .execute(&mut *admin_tx)
        .await
        .unwrap();
    let create = tokio::spawn({
        let app = app.clone();
        let owner_cookie = owner_cookie.clone();
        async move {
            json_request(
                app,
                "POST",
                &format!("/api/v1/workspaces/{workspace_id}/documents"),
                Some(json!({"parentId": null, "title": "Raced"})),
                Some(&owner_cookie),
                &[],
            )
            .await
        }
    });
    wait_for_users_for_update(&admin, blocker_pid).await;
    let token_hash = hash_token(&owner_cookie);
    sqlx::query("UPDATE fvoci.sessions SET revoked_at = now() WHERE token_hash = $1")
        .bind(&token_hash)
        .execute(&mut *admin_tx)
        .await
        .unwrap();
    let revoked: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.sessions WHERE token_hash = $1 AND revoked_at IS NOT NULL",
    )
    .bind(&token_hash)
    .fetch_one(&mut *admin_tx)
    .await
    .unwrap();
    assert_eq!(revoked.0, 1);
    admin_tx.commit().await.unwrap();

    let (status, body, _, _) = create.await.unwrap();
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");
    let docs: (i64,) =
        sqlx::query_as("SELECT count(*) FROM fvoci.documents WHERE workspace_id = $1")
            .bind(workspace_id)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(docs.0, 0);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn event_and_audit_failure_roll_back_document_and_allocated_number() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    install_insert_fail_trigger(&admin, "events", "test_doc_event_fail").await;
    let (status, _, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"parentId": null, "title": "Blocked event"})),
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    let docs: (i64,) =
        sqlx::query_as("SELECT count(*) FROM fvoci.documents WHERE workspace_id = $1")
            .bind(workspace_id)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(docs.0, 0);
    let number: (i32,) =
        sqlx::query_as("SELECT next_document_number FROM fvoci.workspaces WHERE id = $1")
            .bind(workspace_id)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(number.0, 0);
    sqlx::query("DROP TRIGGER fvoci_test_doc_event_fail ON fvoci.events")
        .execute(&admin)
        .await
        .unwrap();

    install_insert_fail_trigger(&admin, "audit_log", "test_doc_audit_fail").await;
    let (status, _, _, _) = json_request(
        app,
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"parentId": null, "title": "Blocked audit"})),
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    let docs: (i64,) =
        sqlx::query_as("SELECT count(*) FROM fvoci.documents WHERE workspace_id = $1")
            .bind(workspace_id)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(docs.0, 0);
    let number: (i32,) =
        sqlx::query_as("SELECT next_document_number FROM fvoci.workspaces WHERE id = $1")
            .bind(workspace_id)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(number.0, 0);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn app_role_rls_and_secret_grants_hold_for_new_tables() {
    let harness = TestDb::bootstrap().await;
    let (_app, _cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let other_ws = Uuid::now_v7();
    sqlx::query("INSERT INTO fvoci.workspaces (id, slug, name) VALUES ($1, 'tenant-b', 'B')")
        .bind(other_ws)
        .execute(&admin)
        .await
        .unwrap();
    let foreign_doc = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO fvoci.documents (
            id, workspace_id, title, path, sort_key, number, status, schema_version,
            content_json, created_by
        ) VALUES (
            $1, $2, 'Secret', $3, 'V', 1, 'draft', 2, '{"type":"doc"}'::jsonb, $4
        )
        "#,
    )
    .bind(foreign_doc)
    .bind(other_ws)
    .bind(foreign_doc.simple().to_string())
    .bind(owner_id)
    .execute(&admin)
    .await
    .unwrap();

    let app_pool = pool::connect_app(&harness.app_url).await.unwrap();
    let denied_password =
        sqlx::query_scalar::<_, String>("SELECT password_hash FROM fvoci.users LIMIT 1")
            .fetch_optional(&app_pool)
            .await;
    assert!(denied_password.is_err());
    let denied_token =
        sqlx::query_scalar::<_, String>("SELECT token_hash FROM fvoci.sessions LIMIT 1")
            .fetch_optional(&app_pool)
            .await;
    assert!(denied_token.is_err());
    let readable_version =
        sqlx::query_scalar::<_, i32>("SELECT version FROM fvoci.schema_migrations LIMIT 1")
            .fetch_one(&app_pool)
            .await
            .expect("app role may SELECT schema_migrations");
    assert!(readable_version >= 1);
    for sql in [
        "INSERT INTO fvoci.schema_migrations (version) VALUES (999)",
        "UPDATE fvoci.schema_migrations SET version = version",
        "DELETE FROM fvoci.schema_migrations",
    ] {
        let error = sqlx::query(sql).execute(&app_pool).await.expect_err(sql);
        assert_eq!(
            error
                .as_database_error()
                .and_then(|db| db.code())
                .map(|code| code.to_string())
                .as_deref(),
            Some("42501"),
            "{sql}: {error}"
        );
    }

    let mut tx = app_pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
        .bind(workspace_id.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let hidden: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM fvoci.documents WHERE id = $1")
        .bind(foreign_doc)
        .fetch_optional(&mut *tx)
        .await
        .unwrap();
    assert!(hidden.is_none());
    let foreign_insert = sqlx::query(
        r#"
        INSERT INTO fvoci.documents (
            id, workspace_id, title, path, sort_key, number, status, schema_version,
            content_json, created_by
        ) VALUES (
            $1, $2, 'Leak', $3, 'V', 1, 'draft', 2, '{"type":"doc"}'::jsonb, $4
        )
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(other_ws)
    .bind("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
    .bind(owner_id)
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

    let encoding_fail = sqlx::query(
        r#"
        INSERT INTO fvoci.document_states (workspace_id, document_id, state, encoding)
        VALUES ($1, $2, '\x00'::bytea, 2)
        "#,
    )
    .bind(other_ws)
    .bind(foreign_doc)
    .execute(&admin)
    .await;
    assert!(encoding_fail.is_err());

    let versions: (i64,) = sqlx::query_as("SELECT count(*) FROM fvoci.schema_migrations")
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(
        versions.0,
        fvoci_server::db::migrate::compiled_migration_count() as i64
    );
    app_pool.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn migration_001_003_upgrades_to_004_documents() {
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
        .unwrap();
    sqlx::query(&format!("CREATE DATABASE \"{db_name}\""))
        .execute(&admin_pool)
        .await
        .unwrap();
    admin_pool.close().await;

    let admin_url = join_db_url(&server_url, &db_name);
    let migration_pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&admin_url)
        .await
        .unwrap();
    for sql in [
        include_str!("../migrations/001_schema.sql"),
        include_str!("../migrations/002_functions.sql"),
        include_str!("../migrations/003_workspace.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&migration_pool).await.unwrap();
    }
    sqlx::query("INSERT INTO fvoci.schema_migrations (version) VALUES (1), (2), (3)")
        .execute(&migration_pool)
        .await
        .unwrap();
    let has_documents: (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM information_schema.tables WHERE table_schema = 'fvoci' AND table_name = 'documents')",
    )
    .fetch_one(&migration_pool)
    .await
    .unwrap();
    assert!(!has_documents.0);
    let has_number: (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM information_schema.columns WHERE table_schema = 'fvoci' AND table_name = 'workspaces' AND column_name = 'next_document_number')",
    )
    .fetch_one(&migration_pool)
    .await
    .unwrap();
    assert!(!has_number.0);
    migration_pool.close().await;

    migrate::run_migrations(&admin_url).await.unwrap();
    let migration_pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&admin_url)
        .await
        .unwrap();
    let versions: (i64,) = sqlx::query_as("SELECT count(*) FROM fvoci.schema_migrations")
        .fetch_one(&migration_pool)
        .await
        .unwrap();
    assert_eq!(
        versions.0,
        fvoci_server::db::migrate::compiled_migration_count() as i64
    );
    let has_documents: (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM information_schema.tables WHERE table_schema = 'fvoci' AND table_name = 'documents')",
    )
    .fetch_one(&migration_pool)
    .await
    .unwrap();
    assert!(has_documents.0);
    let has_states: (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM information_schema.tables WHERE table_schema = 'fvoci' AND table_name = 'document_states')",
    )
    .fetch_one(&migration_pool)
    .await
    .unwrap();
    assert!(has_states.0);
    let has_number: (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM information_schema.columns WHERE table_schema = 'fvoci' AND table_name = 'workspaces' AND column_name = 'next_document_number')",
    )
    .fetch_one(&migration_pool)
    .await
    .unwrap();
    assert!(has_number.0);
    sqlx::query(&format!(
        "CREATE ROLE \"{role_name}\" LOGIN PASSWORD '{role_password}' NOSUPERUSER NOBYPASSRLS"
    ))
    .execute(&migration_pool)
    .await
    .unwrap();
    apply_grants(&migration_pool, &role_name).await;
    let still_has_self: (bool,) =
        sqlx::query_as("SELECT EXISTS (SELECT 1 FROM pg_proc WHERE proname = 'app_self_user_id')")
            .fetch_one(&migration_pool)
            .await
            .unwrap();
    assert!(still_has_self.0);
    let user_id = Uuid::now_v7();
    let workspace_id = Uuid::now_v7();
    sqlx::query("INSERT INTO fvoci.users (id, email, given_name) VALUES ($1, $2, $3)")
        .bind(user_id)
        .bind("upgrade@example.com")
        .bind("Upgrade")
        .execute(&migration_pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO fvoci.workspaces (id, slug, name) VALUES ($1, 'upgrade', 'Upgrade')")
        .bind(workspace_id)
        .execute(&migration_pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO fvoci.memberships (workspace_id, user_id, role) VALUES ($1, $2, 'owner')",
    )
    .bind(workspace_id)
    .bind(user_id)
    .execute(&migration_pool)
    .await
    .unwrap();
    migration_pool.close().await;

    let mut app = url::Url::parse(&admin_url).unwrap();
    app.set_username(&role_name).ok();
    app.set_password(Some(&role_password)).ok();
    let app_pool = pool::connect_app(app.as_str()).await.unwrap();
    let mut tx = app_pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
        .bind(workspace_id.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let doc_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO fvoci.documents (
            id, workspace_id, title, path, sort_key, number, status, schema_version,
            content_json, created_by
        ) VALUES (
            $1, $2, 'Upgraded', $3, 'V', 1, 'draft', 2, '{"type":"doc"}'::jsonb, $4
        )
        "#,
    )
    .bind(doc_id)
    .bind(workspace_id)
    .bind(doc_id.simple().to_string())
    .bind(user_id)
    .execute(&mut *tx)
    .await
    .unwrap();
    let visible: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM fvoci.documents WHERE id = $1")
        .bind(doc_id)
        .fetch_optional(&mut *tx)
        .await
        .unwrap();
    assert_eq!(visible.map(|(id,)| id), Some(doc_id));
    let hidden_foreign: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM fvoci.documents WHERE workspace_id <> $1")
            .bind(workspace_id)
            .fetch_optional(&mut *tx)
            .await
            .unwrap();
    assert!(hidden_foreign.is_none());
    tx.commit().await.unwrap();
    app_pool.close().await;

    let cleanup = TestDb {
        admin_url,
        app_url: String::new(),
        db_name,
        role_name,
    };
    cleanup.cleanup().await;
}

async fn create_doc(
    app: &axum::Router,
    cookie: &str,
    workspace_id: Uuid,
    parent_id: Option<&str>,
    title: &str,
) -> Value {
    let body = match parent_id {
        Some(parent) => json!({"parentId": parent, "title": title}),
        None => json!({"parentId": null, "title": title}),
    };
    let (status, body, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(body),
        Some(cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    body
}

#[tokio::test]
async fn document_move_sort_trash_restore_and_trash_list() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let root = create_doc(&app, &cookie, workspace_id, None, "Root").await;
    let root_id = root["id"].as_str().unwrap();
    let child = create_doc(&app, &cookie, workspace_id, Some(root_id), "Child").await;
    let child_id = child["id"].as_str().unwrap();
    let sibling = create_doc(&app, &cookie, workspace_id, None, "Sibling").await;
    let sibling_id = sibling["id"].as_str().unwrap();

    let (status, body, _, _) = json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{root_id}"),
        Some(json!({"title": "Renamed root"})),
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["title"], "Renamed root");

    let (status, body, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{child_id}/move"),
        Some(json!({"newParentId": sibling_id})),
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["parentId"], sibling_id);

    let (status, body, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{child_id}/sort"),
        Some(json!({"afterId": null})),
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let first_sort = body["sortKey"].as_str().unwrap();

    let (status, body, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{child_id}/trash"),
        None,
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], true);

    let (status, body, _, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/tree"),
        None,
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!body["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|node| node["id"] == child_id));

    let (status, body, _, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/trash"),
        None,
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    assert_eq!(body["items"][0]["id"], child_id);
    assert_eq!(body["items"][0]["title"], "Child");

    let (status, body, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{child_id}/restore"),
        None,
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], true);

    let (status, body, _, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{child_id}"),
        None,
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["sortKey"], first_sort);

    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let moved_events: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.events WHERE verb = 'document.moved' AND workspace_id = $1",
    )
    .bind(workspace_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert!(moved_events.0 >= 2);
    let trashed_events: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.events WHERE verb = 'document.trashed' AND workspace_id = $1",
    )
    .bind(workspace_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(trashed_events.0, 1);
    let restored_events: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.events WHERE verb = 'document.restored' AND workspace_id = $1",
    )
    .bind(workspace_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(restored_events.0, 1);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn document_move_cycle_and_depth_are_rejected() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let root = create_doc(&app, &cookie, workspace_id, None, "Root").await;
    let root_id = root["id"].as_str().unwrap();
    let child = create_doc(&app, &cookie, workspace_id, Some(root_id), "Child").await;
    let child_id = child["id"].as_str().unwrap();

    let (status, body, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{root_id}/move"),
        Some(json!({"newParentId": child_id})),
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "document_cycle");

    let mut deepest_id = root_id.to_string();
    for index in 0..19 {
        let deepest = create_doc(
            &app,
            &cookie,
            workspace_id,
            Some(&deepest_id),
            &format!("Deep {index}"),
        )
        .await;
        deepest_id = deepest["id"].as_str().unwrap().to_string();
    }

    let (status, body, _, _) = json_request(
        app,
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{child_id}/move"),
        Some(json!({"newParentId": deepest_id})),
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "tree_depth_limit");
    harness.cleanup().await;
}

#[tokio::test]
async fn document_restore_rejects_trashed_parent() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let parent = create_doc(&app, &cookie, workspace_id, None, "Parent").await;
    let parent_id = parent["id"].as_str().unwrap();
    let child = create_doc(&app, &cookie, workspace_id, Some(parent_id), "Child").await;
    let child_id = child["id"].as_str().unwrap();

    let (status, _, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{parent_id}/trash"),
        None,
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{child_id}/restore"),
        None,
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "restore_rejected");
    harness.cleanup().await;
}

#[tokio::test]
async fn document_lifecycle_denies_guest_and_non_member() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, _, workspace_id) = setup_session(&harness).await;
    let doc = create_doc(&app, &owner_cookie, workspace_id, None, "Secret").await;
    let doc_id = doc["id"].as_str().unwrap();
    let (guest_id, guest_cookie) =
        create_second_user_session(&harness, "guest2@example.com", "Guest").await;
    let app_pool = pool::connect_app(&harness.app_url).await.unwrap();
    fvoci_server::db::workspace::add_membership_for_test(
        &app_pool,
        workspace_id,
        guest_id,
        fvoci_server::db::workspace::WorkspaceRole::Guest,
    )
    .await
    .unwrap();
    app_pool.close().await;

    for (method, path, body) in [
        (
            "POST",
            format!("/api/v1/workspaces/{workspace_id}/documents/{doc_id}/trash"),
            None,
        ),
        (
            "POST",
            format!("/api/v1/workspaces/{workspace_id}/documents/{doc_id}/move"),
            Some(json!({"newParentId": doc_id})),
        ),
        (
            "GET",
            format!("/api/v1/workspaces/{workspace_id}/trash"),
            None,
        ),
    ] {
        let (status, body_json, _, _) =
            json_request(app.clone(), method, &path, body, Some(&guest_cookie), &[]).await;
        if method == "GET" {
            assert_eq!(status, StatusCode::OK);
            assert_eq!(body_json["items"].as_array().unwrap().len(), 0);
        } else {
            assert_eq!(status, StatusCode::NOT_FOUND);
            assert_eq!(body_json["code"], "not_found");
        }
    }

    let (other_id, other_cookie) =
        create_second_user_session(&harness, "other@example.com", "Other").await;
    let (status, body, _, _) = json_request(
        app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{doc_id}"),
        None,
        Some(&other_cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");
    assert_ne!(other_id, guest_id);
    harness.cleanup().await;
}

#[tokio::test]
async fn document_lifecycle_denies_other_workspace_and_revoked_session() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let doc = create_doc(&app, &owner_cookie, workspace_id, None, "Locked").await;
    let doc_id = doc["id"].as_str().unwrap();

    let (foreign_id, foreign_cookie) =
        create_second_user_session(&harness, "foreign@example.com", "Foreign").await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let foreign_workspace = Uuid::now_v7();
    sqlx::query("INSERT INTO fvoci.workspaces (id, slug, name) VALUES ($1, $2, $3)")
        .bind(foreign_workspace)
        .bind(fvoci_server::db::workspace::personal_workspace_slug(
            foreign_id,
        ))
        .bind("Foreign")
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO fvoci.memberships (workspace_id, user_id, role) VALUES ($1, $2, 'owner')",
    )
    .bind(foreign_workspace)
    .bind(foreign_id)
    .execute(&admin)
    .await
    .unwrap();
    sqlx::query("UPDATE fvoci.sessions SET revoked_at = now() WHERE user_id = $1")
        .bind(owner_id)
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;

    let (status, body, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{doc_id}/trash"),
        None,
        Some(&foreign_cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");

    let (status, body, _, _) = json_request(
        app,
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{doc_id}/move"),
        Some(json!({"newParentId": doc_id})),
        Some(&owner_cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], "authentication_required");
    assert_ne!(foreign_id, owner_id);
    harness.cleanup().await;
}

async fn create_project_via_api(
    app: &axum::Router,
    cookie: &str,
    workspace_id: Uuid,
    key: &str,
    visibility: &str,
) -> Value {
    let (status, body, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects"),
        Some(json!({"key": key, "name": key, "visibility": visibility})),
        Some(cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    body
}

async fn add_workspace_member(
    harness: &TestDb,
    workspace_id: Uuid,
    email: &str,
    given_name: &str,
    role: fvoci_server::db::workspace::WorkspaceRole,
) -> (Uuid, String) {
    let (user_id, cookie) = create_second_user_session(harness, email, given_name).await;
    let pool = pool::connect_app(&harness.app_url).await.unwrap();
    fvoci_server::db::workspace::add_membership_for_test(&pool, workspace_id, user_id, role)
        .await
        .unwrap();
    pool.close().await;
    (user_id, cookie)
}

#[tokio::test]
async fn document_move_into_project_denies_unauthorized() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();

    let (lead_id, lead_cookie) = add_workspace_member(
        &harness,
        workspace_id,
        "lead@example.com",
        "Lead",
        fvoci_server::db::workspace::WorkspaceRole::Member,
    )
    .await;
    let private = create_project_via_api(&app, &lead_cookie, workspace_id, "HID", "private").await;
    let private_project_id = private["id"].as_str().unwrap();
    let private_root_id = private["rootDocumentId"].as_str().unwrap();

    let wiki = create_doc(&app, &lead_cookie, workspace_id, None, "Wiki").await;
    let wiki_id = wiki["id"].as_str().unwrap();

    let (outsider_id, outsider_cookie) = add_workspace_member(
        &harness,
        workspace_id,
        "outsider@example.com",
        "Outsider",
        fvoci_server::db::workspace::WorkspaceRole::Member,
    )
    .await;
    assert_ne!(outsider_id, lead_id);

    async fn assert_move_denied(
        admin: &PgPool,
        app: &axum::Router,
        cookie: &str,
        workspace_id: Uuid,
        document_id: &str,
        new_parent_id: &str,
    ) {
        let moved_before: (i64,) = sqlx::query_as(
            "SELECT count(*) FROM fvoci.events WHERE verb = 'document.moved' AND workspace_id = $1",
        )
        .bind(workspace_id)
        .fetch_one(admin)
        .await
        .unwrap();
        let audit_before: (i64,) = sqlx::query_as(
            "SELECT count(*) FROM fvoci.audit_log WHERE verb = 'document.moved' AND workspace_id = $1",
        )
        .bind(workspace_id)
        .fetch_one(admin)
        .await
        .unwrap();
        let project_id_before: Option<(Option<Uuid>,)> = sqlx::query_as(
            "SELECT project_id FROM fvoci.documents WHERE workspace_id = $1 AND id = $2",
        )
        .bind(workspace_id)
        .bind(Uuid::parse_str(document_id).unwrap())
        .fetch_optional(admin)
        .await
        .unwrap();

        let (status, body, _, _) = json_request(
            app.clone(),
            "POST",
            &format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/move"),
            Some(json!({"newParentId": new_parent_id})),
            Some(cookie),
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "body={body:?}");
        assert_eq!(body["code"], "not_found");

        let moved_after: (i64,) = sqlx::query_as(
            "SELECT count(*) FROM fvoci.events WHERE verb = 'document.moved' AND workspace_id = $1",
        )
        .bind(workspace_id)
        .fetch_one(admin)
        .await
        .unwrap();
        let audit_after: (i64,) = sqlx::query_as(
            "SELECT count(*) FROM fvoci.audit_log WHERE verb = 'document.moved' AND workspace_id = $1",
        )
        .bind(workspace_id)
        .fetch_one(admin)
        .await
        .unwrap();
        let project_id_after: Option<(Option<Uuid>,)> = sqlx::query_as(
            "SELECT project_id FROM fvoci.documents WHERE workspace_id = $1 AND id = $2",
        )
        .bind(workspace_id)
        .bind(Uuid::parse_str(document_id).unwrap())
        .fetch_optional(admin)
        .await
        .unwrap();

        assert_eq!(moved_before.0, moved_after.0);
        assert_eq!(audit_before.0, audit_after.0);
        assert_eq!(project_id_before, project_id_after);
    }

    assert_move_denied(
        &admin,
        &app,
        &owner_cookie,
        workspace_id,
        wiki_id,
        private_root_id,
    )
    .await;

    let (status, _, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{private_project_id}/members"),
        Some(json!({"userId": owner_id.to_string(), "role":"viewer"})),
        Some(&lead_cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_move_denied(
        &admin,
        &app,
        &owner_cookie,
        workspace_id,
        wiki_id,
        private_root_id,
    )
    .await;
    assert_move_denied(
        &admin,
        &app,
        &outsider_cookie,
        workspace_id,
        wiki_id,
        private_root_id,
    )
    .await;

    sqlx::query(
        "UPDATE fvoci.projects SET status = 'archived' WHERE workspace_id = $1 AND id = $2",
    )
    .bind(workspace_id)
    .bind(Uuid::parse_str(private_project_id).unwrap())
    .execute(&admin)
    .await
    .unwrap();
    assert_move_denied(
        &admin,
        &app,
        &lead_cookie,
        workspace_id,
        wiki_id,
        private_root_id,
    )
    .await;

    let lab = create_project_via_api(&app, &lead_cookie, workspace_id, "LAB", "workspace").await;
    let lab_root_id = lab["rootDocumentId"].as_str().unwrap();
    let movable = create_doc(&app, &lead_cookie, workspace_id, None, "Movable").await;
    let movable_id = movable["id"].as_str().unwrap();
    let (status, body, _, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{movable_id}/move"),
        Some(json!({"newParentId": lab_root_id})),
        Some(&lead_cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["projectId"], lab["id"]);

    admin.close().await;
    harness.cleanup().await;
}
