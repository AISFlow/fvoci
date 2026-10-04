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
    app_state_backend(fvoci_server::db::backend::Backend::Postgres(pool)).await
}

async fn app_state_backend(backend: fvoci_server::db::backend::Backend) -> AppState {
    let storage_root = std::env::temp_dir().join(format!("fvoci-doc-test-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&storage_root).expect("storage root");
    AppState {
        auth: Arc::new(AuthService {
            db: Db::from_backend(backend),
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
        Some(json!({"commandId": uuid::Uuid::now_v7(), "parentId": null, "title": "  Root  "})),
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
        Some(json!({"commandId": uuid::Uuid::now_v7(), "parentId": root_id, "title": "Child", "icon": "📄"})),
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
        Some(json!({"commandId": uuid::Uuid::now_v7(), "parentId": null, "title": "Second root"})),
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
        Some(json!({"commandId": uuid::Uuid::now_v7(), "parentId": null, "title": "Hidden"})),
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
        Some(json!({"commandId": uuid::Uuid::now_v7(), "parentId": null, "title": "Guest write"})),
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
        Some(json!({"commandId": uuid::Uuid::now_v7(), "parentId": null, "title": "Member doc"})),
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
        Some(
            json!({"commandId": uuid::Uuid::now_v7(), "parentId": null, "title": "Admin override"}),
        ),
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
        Some(json!({"commandId": uuid::Uuid::now_v7(), "parentId": foreign_parent, "title": "Cross"})),
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
        Some(
            json!({"commandId": uuid::Uuid::now_v7(), "parentId": affiliated, "title": "Mismatch"}),
        ),
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
            Some(json!({"commandId": uuid::Uuid::now_v7(), "parentId": parent, "title": format!("D{depth}")})),
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
        Some(json!({"commandId": uuid::Uuid::now_v7(), "parentId": last_id, "title": "Too deep"})),
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
        Some(json!({"commandId": uuid::Uuid::now_v7(), "parentId": null, "title": "Null regression"})),
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
        Some(json!({"commandId": uuid::Uuid::now_v7(), "parentId": null, "title": "Nope"})),
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
        Some(json!({"commandId": uuid::Uuid::now_v7(), "parentId": Uuid::now_v7(), "title": "Missing parent"})),
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
        Some(json!({"commandId": uuid::Uuid::now_v7(), "title": "Missing parentId"})),
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
        Some(json!({"commandId": uuid::Uuid::now_v7(), "parentId": null, "title": ""})),
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
        Some(json!({"commandId": uuid::Uuid::now_v7(), "parentId": null, "title": "X"})),
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
        Some(json!({"commandId": uuid::Uuid::now_v7(), "parentId": null, "title": "Iconed", "icon": "📄"})),
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
        Some(json!({"commandId": uuid::Uuid::now_v7(), "parentId": null, "title": "Sibling of corrupt"})),
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
                Some(json!({"commandId": uuid::Uuid::now_v7(), "parentId": null, "title": "After product demote"})),
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
                Some(json!({"commandId": uuid::Uuid::now_v7(), "parentId": null, "title": "Before product remove"})),
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
        Some(json!({"commandId": uuid::Uuid::now_v7(), "parentId": null, "title": "After demote"})),
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
                Some(json!({"commandId": uuid::Uuid::now_v7(), "parentId": null, "title": "After suspend"})),
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
        Some(json!({"commandId": uuid::Uuid::now_v7(), "parentId": null, "title": "After remove"})),
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
                Some(
                    json!({"commandId": uuid::Uuid::now_v7(), "parentId": null, "title": "Raced"}),
                ),
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
        Some(
            json!({"commandId": uuid::Uuid::now_v7(), "parentId": null, "title": "Blocked event"}),
        ),
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
        Some(
            json!({"commandId": uuid::Uuid::now_v7(), "parentId": null, "title": "Blocked audit"}),
        ),
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
    let command_id = Uuid::now_v7();
    let body = match parent_id {
        Some(parent) => json!({"commandId": command_id, "parentId": parent, "title": title}),
        None => json!({"commandId": command_id, "parentId": null, "title": title}),
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

#[tokio::test]
async fn wiki_create_command_replays_original_result_and_rejects_hash_actor_purge() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner, ws) = setup_session(&harness).await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let path = format!("/api/v1/workspaces/{ws}/documents");
    let command = Uuid::now_v7();
    let input =
        json!({"commandId":command,"parentId":null,"title":"한글 日本語 中文 🙂 가","icon":null});
    let (first, second) = tokio::join!(
        json_request(
            app.clone(),
            "POST",
            &path,
            Some(input.clone()),
            Some(&cookie),
            &[]
        ),
        json_request(
            app.clone(),
            "POST",
            &path,
            Some(input.clone()),
            Some(&cookie),
            &[]
        ),
    );
    assert_eq!(first.0, StatusCode::CREATED, "{}", first.1);
    assert_eq!(second.0, StatusCode::CREATED, "{}", second.1);
    assert_eq!(
        first.1, second.1,
        "same command returns the original response"
    );
    let original = first.1;
    let id = Uuid::parse_str(original["id"].as_str().unwrap()).unwrap();
    let inventory: (i64, i32, i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM fvoci.documents WHERE workspace_id=$1), next_document_number, (SELECT count(*) FROM fvoci.events WHERE workspace_id=$1 AND verb='document.created'), (SELECT count(*) FROM fvoci.audit_log WHERE workspace_id=$1 AND verb='document.created'), (SELECT count(*) FROM fvoci.wiki_create_commands WHERE workspace_id=$1) FROM fvoci.workspaces WHERE id=$1"
    ).bind(ws).fetch_one(&admin).await.unwrap();
    assert_eq!(inventory, (1, 1, 1, 1, 1));
    let restricted = pool::connect_app(&harness.app_url).await.unwrap();
    migrate::assert_app_role(&restricted)
        .await
        .expect("actual restricted application role");
    let receipt_rls: (bool, bool) = sqlx::query_as(
        "SELECT relrowsecurity, relforcerowsecurity FROM pg_class WHERE oid='fvoci.wiki_create_commands'::regclass"
    ).fetch_one(&restricted).await.unwrap();
    assert_eq!(receipt_rls, (true, true));
    let mut scoped = restricted.begin().await.unwrap();
    fvoci_server::db::context::set_tenant(&mut scoped, Uuid::now_v7())
        .await
        .unwrap();
    let hidden: i64 =
        sqlx::query_scalar("SELECT count(*) FROM fvoci.wiki_create_commands WHERE workspace_id=$1")
            .bind(ws)
            .fetch_one(&mut *scoped)
            .await
            .unwrap();
    assert_eq!(
        hidden, 0,
        "actual RLS hides receipts under the wrong tenant"
    );
    fvoci_server::db::context::set_tenant(&mut scoped, ws)
        .await
        .unwrap();
    let visible: i64 =
        sqlx::query_scalar("SELECT count(*) FROM fvoci.wiki_create_commands WHERE workspace_id=$1")
            .bind(ws)
            .fetch_one(&mut *scoped)
            .await
            .unwrap();
    assert_eq!(visible, 1);
    scoped.rollback().await.unwrap();
    restricted.close().await;

    let mut changed = input.clone();
    changed["title"] = json!("different");
    let mismatch = json_request(
        app.clone(),
        "POST",
        &path,
        Some(changed),
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(mismatch.0, StatusCode::CONFLICT);
    assert_eq!(mismatch.1["code"], "request_mismatch");
    let (other, other_cookie) =
        create_second_user_session(&harness, "other@example.com", "Other").await;
    sqlx::query(
        "INSERT INTO fvoci.memberships (workspace_id,user_id,role) VALUES ($1,$2,'member')",
    )
    .bind(ws)
    .bind(other)
    .execute(&admin)
    .await
    .unwrap();
    let wrong_actor = json_request(
        app.clone(),
        "POST",
        &path,
        Some(input.clone()),
        Some(&other_cookie),
        &[],
    )
    .await;
    assert_eq!(
        wrong_actor.0,
        StatusCode::CONFLICT,
        "another actor cannot use the original command"
    );

    sqlx::query("UPDATE fvoci.documents SET title='Later title' WHERE id=$1")
        .bind(id)
        .execute(&admin)
        .await
        .unwrap();
    let replay = json_request(
        app.clone(),
        "POST",
        &path,
        Some(input.clone()),
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(replay.0, StatusCode::CREATED);
    assert_eq!(
        replay.1, original,
        "readback changes cannot change the receipt echo"
    );
    sqlx::query("UPDATE fvoci.documents SET deleted_at=now() WHERE id=$1")
        .bind(id)
        .execute(&admin)
        .await
        .unwrap();
    let trashed = json_request(
        app.clone(),
        "POST",
        &path,
        Some(input.clone()),
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(
        trashed.0,
        StatusCode::NOT_FOUND,
        "a receipt cannot authorize a trashed target"
    );
    sqlx::query("UPDATE fvoci.documents SET deleted_at=NULL WHERE id=$1")
        .bind(id)
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query("UPDATE fvoci.memberships SET role='guest' WHERE workspace_id=$1 AND user_id=$2")
        .bind(ws)
        .bind(owner)
        .execute(&admin)
        .await
        .unwrap();
    assert_eq!(
        json_request(
            app.clone(),
            "POST",
            &path,
            Some(input.clone()),
            Some(&cookie),
            &[]
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    sqlx::query("UPDATE fvoci.memberships SET role='owner' WHERE workspace_id=$1 AND user_id=$2")
        .bind(ws)
        .bind(owner)
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query("DELETE FROM fvoci.documents WHERE id=$1")
        .bind(id)
        .execute(&admin)
        .await
        .unwrap();
    let purged = json_request(
        app.clone(),
        "POST",
        &path,
        Some(input.clone()),
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(purged.0, StatusCode::NOT_FOUND);
    let retired: (i64, i32, i64, Option<Uuid>) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM fvoci.documents WHERE workspace_id=$1), next_document_number, (SELECT count(*) FROM fvoci.wiki_create_commands WHERE workspace_id=$1), (SELECT document_id FROM fvoci.wiki_create_commands WHERE workspace_id=$1 AND command_id=$2) FROM fvoci.workspaces WHERE id=$1"
    ).bind(ws).bind(command).fetch_one(&admin).await.unwrap();
    assert_eq!(retired, (0, 1, 1, None));
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn wiki_create_command_requires_identity_and_receipt_failure_rolls_back_everything() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, ws) = setup_session(&harness).await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let path = format!("/api/v1/workspaces/{ws}/documents");
    let missing = json_request(
        app.clone(),
        "POST",
        &path,
        Some(json!({"parentId":null,"title":"missing command"})),
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(missing.0, StatusCode::BAD_REQUEST);
    install_insert_fail_trigger(&admin, "wiki_create_commands", "reject_wiki_receipt").await;
    let input = json!({"commandId":Uuid::now_v7(),"parentId":null,"title":"rolled back"});
    let failed = json_request(
        app.clone(),
        "POST",
        &path,
        Some(input.clone()),
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(failed.0, StatusCode::INTERNAL_SERVER_ERROR);
    let inventory: (i64, i32, i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM fvoci.documents WHERE workspace_id=$1), next_document_number, (SELECT count(*) FROM fvoci.events WHERE workspace_id=$1 AND verb='document.created'), (SELECT count(*) FROM fvoci.audit_log WHERE workspace_id=$1 AND verb='document.created'), (SELECT count(*) FROM fvoci.wiki_create_commands WHERE workspace_id=$1) FROM fvoci.workspaces WHERE id=$1"
    ).bind(ws).fetch_one(&admin).await.unwrap();
    assert_eq!(inventory, (0, 0, 0, 0, 0));
    sqlx::query("DROP TRIGGER fvoci_reject_wiki_receipt ON fvoci.wiki_create_commands")
        .execute(&admin)
        .await
        .unwrap();
    let success = json_request(app, "POST", &path, Some(input), Some(&cookie), &[]).await;
    assert_eq!(success.0, StatusCode::CREATED);
    assert_eq!(success.1["number"], 1);
    admin.close().await;
    harness.cleanup().await;
}

/// Early executable common fixture. Native editor/ACK/revision/browser proof
/// is a separate required tracer; this does not claim that acceptance.
// Synthetic data setup only. The product login/session/permission readers below
// run through the real selected app connection; this is not a port of user/admin
// mutation APIs or a claim that SQLite has PostgreSQL's app-role boundary.
async fn selected_fixture_actor(
    backend: &fvoci_server::db::backend::Backend,
    harness: &TestDb,
    app: &axum::Router,
    workspace: Uuid,
    role: &str,
    label: &str,
) -> (fvoci_server::db::identity::LiveSession, String) {
    use fvoci_server::db::backend::Backend;
    let user = Uuid::now_v7();
    let email = format!("{label}@example.com");
    let password = fvoci_server::auth::password::hash_password(
        "supersecret1",
        &Keyring::parse(PEPPER, "test").unwrap(),
    )
    .await
    .unwrap();
    match backend {
        Backend::Postgres(_) => {
            let admin = PgPoolOptions::new()
                .max_connections(1)
                .connect(&harness.admin_url)
                .await
                .unwrap();
            sqlx::query(
                "INSERT INTO fvoci.users(id,email,password_hash,given_name) VALUES($1,$2,$3,$4)",
            )
            .bind(user)
            .bind(&email)
            .bind(&password)
            .bind(label)
            .execute(&admin)
            .await
            .unwrap();
            admin.close().await;
        }
        Backend::Sqlite(pool) => {
            let mut tx = pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
            sqlx::query("INSERT INTO users(id,email,password_hash,given_name) VALUES(?1,?2,?3,?4)")
                .bind(user.as_bytes().to_vec())
                .bind(&email)
                .bind(&password)
                .bind(label)
                .execute(&mut *tx)
                .await
                .unwrap();
            tx.commit().await.unwrap();
        }
        Backend::LibsqlRemote(_) => unreachable!("actual remote primary is separately required"),
    }
    selected_fixture_membership(backend, harness, workspace, user, Some(role)).await;
    let (status, body, cookie, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/auth/login",
        Some(json!({"email":email,"password":"supersecret1"})),
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "actual {label} login: {body}");
    let cookie = extract_session_cookie(cookie.as_ref().unwrap());
    let live = fvoci_server::db::identity::find_live_session_backend(backend, &hash_token(&cookie))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(live.user_id, user);
    (live, cookie)
}

async fn selected_fixture_membership(
    backend: &fvoci_server::db::backend::Backend,
    harness: &TestDb,
    workspace: Uuid,
    user: Uuid,
    role: Option<&str>,
) {
    use fvoci_server::db::backend::Backend;
    match backend {
        Backend::Postgres(_) => {
            let admin = PgPoolOptions::new()
                .max_connections(1)
                .connect(&harness.admin_url)
                .await
                .unwrap();
            let mut tx = admin.begin().await.unwrap();
            match role {
                Some(role) => {
                    sqlx::query("INSERT INTO fvoci.memberships(workspace_id,user_id,role) VALUES($1,$2,$3) ON CONFLICT(workspace_id,user_id) DO UPDATE SET role=EXCLUDED.role").bind(workspace).bind(user).bind(role).execute(&mut *tx).await.unwrap();
                }
                None => {
                    assert_eq!(
                        sqlx::query(
                            "DELETE FROM fvoci.memberships WHERE workspace_id=$1 AND user_id=$2"
                        )
                        .bind(workspace)
                        .bind(user)
                        .execute(&mut *tx)
                        .await
                        .unwrap()
                        .rows_affected(),
                        1
                    );
                }
            }
            tx.commit().await.unwrap();
            admin.close().await;
        }
        Backend::Sqlite(pool) => {
            let mut tx = pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
            match role {
                Some(role) => {
                    sqlx::query("INSERT INTO memberships(workspace_id,user_id,role) VALUES(?1,?2,?3) ON CONFLICT(workspace_id,user_id) DO UPDATE SET role=excluded.role").bind(workspace.as_bytes().to_vec()).bind(user.as_bytes().to_vec()).bind(role).execute(&mut *tx).await.unwrap();
                }
                None => {
                    assert_eq!(
                        sqlx::query("DELETE FROM memberships WHERE workspace_id=?1 AND user_id=?2")
                            .bind(workspace.as_bytes().to_vec())
                            .bind(user.as_bytes().to_vec())
                            .execute(&mut *tx)
                            .await
                            .unwrap()
                            .rows_affected(),
                        1
                    );
                }
            }
            tx.commit().await.unwrap();
        }
        Backend::LibsqlRemote(_) => unreachable!("actual remote primary is separately required"),
    }
}

async fn selected_fixture_view_grant(
    backend: &fvoci_server::db::backend::Backend,
    harness: &TestDb,
    workspace: Uuid,
    user: Uuid,
    document: Uuid,
) {
    use fvoci_server::db::backend::Backend;
    let group = Uuid::now_v7();
    let grant = Uuid::now_v7();
    match backend {
        Backend::Postgres(_) => {
            let admin = PgPoolOptions::new()
                .max_connections(1)
                .connect(&harness.admin_url)
                .await
                .unwrap();
            let mut tx = admin.begin().await.unwrap();
            sqlx::query("INSERT INTO fvoci.groups(id,workspace_id,name) VALUES($1,$2,'View only')")
                .bind(group)
                .bind(workspace)
                .execute(&mut *tx)
                .await
                .unwrap();
            sqlx::query(
                "INSERT INTO fvoci.group_members(workspace_id,group_id,user_id) VALUES($1,$2,$3)",
            )
            .bind(workspace)
            .bind(group)
            .bind(user)
            .execute(&mut *tx)
            .await
            .unwrap();
            sqlx::query("INSERT INTO fvoci.document_members(id,workspace_id,document_id,group_id,role) VALUES($1,$2,$3,$4,'viewer')").bind(grant).bind(workspace).bind(document).bind(group).execute(&mut *tx).await.unwrap();
            tx.commit().await.unwrap();
            admin.close().await;
        }
        Backend::Sqlite(pool) => {
            let mut tx = pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
            sqlx::query("INSERT INTO groups(id,workspace_id,name) VALUES(?1,?2,'View only')")
                .bind(group.as_bytes().to_vec())
                .bind(workspace.as_bytes().to_vec())
                .execute(&mut *tx)
                .await
                .unwrap();
            sqlx::query(
                "INSERT INTO group_members(workspace_id,group_id,user_id) VALUES(?1,?2,?3)",
            )
            .bind(workspace.as_bytes().to_vec())
            .bind(group.as_bytes().to_vec())
            .bind(user.as_bytes().to_vec())
            .execute(&mut *tx)
            .await
            .unwrap();
            sqlx::query("INSERT INTO document_members(id,workspace_id,document_id,group_id,role) VALUES(?1,?2,?3,?4,'viewer')").bind(grant.as_bytes().to_vec()).bind(workspace.as_bytes().to_vec()).bind(document.as_bytes().to_vec()).bind(group.as_bytes().to_vec()).execute(&mut *tx).await.unwrap();
            tx.commit().await.unwrap();
        }
        Backend::LibsqlRemote(_) => unreachable!("actual remote primary is separately required"),
    }
}

async fn selected_family_fence_snapshot(
    backend: &fvoci_server::db::backend::Backend,
    workspace: Uuid,
    document: Uuid,
) -> (Vec<u8>, i64, i64, i64) {
    let fvoci_server::db::backend::Backend::Sqlite(pool) = backend else {
        panic!("actual local family control")
    };
    sqlx::query_as("SELECT f.owner_token,f.fence,s.writer_generation,s.tail_seq FROM collab_room_fences f JOIN document_states s ON s.workspace_id=f.workspace_id AND s.document_id=f.document_id WHERE f.workspace_id=?1 AND f.document_id=?2")
        .bind(workspace.as_bytes().to_vec()).bind(document.as_bytes().to_vec()).fetch_one(pool).await.unwrap()
}

async fn selected_workspace_race_controls(
    backend: &fvoci_server::db::backend::Backend,
    harness: &TestDb,
    app: &axum::Router,
    workspace: Uuid,
) -> Vec<String> {
    let (actor, cookie) =
        selected_fixture_actor(backend, harness, app, workspace, "member", "race-member").await;
    let mut failures = Vec::new();
    for new_role in [Some("guest"), None] {
        selected_fixture_membership(backend, harness, workspace, actor.user_id, Some("member"))
            .await;
        let (reached, proceed) =
            fvoci_server::db::workspace::arm_workspace_card_barrier(actor.user_id).await;
        let pending = tokio::spawn({
            let app = app.clone();
            let cookie = cookie.clone();
            async move {
                json_request(
                    app,
                    "GET",
                    "/api/v1/me/workspaces",
                    None,
                    Some(&cookie),
                    &[],
                )
                .await
            }
        });
        reached.await.unwrap();
        // Real second writer commits BEFORE the card transaction begins.
        selected_fixture_membership(backend, harness, workspace, actor.user_id, new_role).await;
        proceed.send(()).unwrap();
        let (status, listed, _, _) = pending.await.unwrap();
        assert_eq!(status, StatusCode::OK, "race self-list: {listed}");
        let id = workspace.to_string();
        let card = listed["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == id);
        let valid = match new_role {
            None => card.is_none(),
            Some("guest") => card.is_some_and(|card| {
                card["role"] == "guest" && card["documentCount"] == 0 && card["assignedCount"] == 0
            }),
            _ => unreachable!(),
        };
        eprintln!("workspace_race backend={} committed_role={new_role:?} authority_current={valid} response={listed}",backend.kind());
        if !valid {
            failures.push(format!(
                "{} committed {new_role:?} returned stale card {listed}",
                backend.kind()
            ));
        }
        let (status, fresh, _, _) = json_request(
            app.clone(),
            "GET",
            "/api/v1/me/workspaces",
            None,
            Some(&cookie),
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let card = fresh["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == id);
        match new_role {
            None => assert!(card.is_none()),
            Some("guest") => {
                let card = card.unwrap();
                assert_eq!(card["role"], "guest");
                assert_eq!(card["documentCount"], 0);
            }
            _ => unreachable!(),
        }
    }
    failures
}

#[tokio::test]
async fn selected_backend_setup_cookie_wiki_command_readback() {
    selected_backend_wiki_fixture(false, false, false).await;
}

#[tokio::test]
async fn selected_backend_native_append_fresh_child_readback() {
    selected_backend_wiki_fixture(true, false, false).await;
}

#[tokio::test]
async fn selected_backend_workspace_current_membership_race() {
    selected_backend_wiki_fixture(false, true, false).await;
}

#[tokio::test]
async fn selected_backend_native_compaction_receipt_readback() {
    selected_backend_wiki_fixture(true, false, true).await;
}

async fn selected_compaction_effect_counts(
    backend: &fvoci_server::db::backend::Backend,
    workspace: Uuid,
    document: Uuid,
) -> (i64, i64) {
    use fvoci_server::db::backend::Backend;
    match backend {
        Backend::Postgres(pool) => {
            let mut tx = pool.begin().await.unwrap();
            fvoci_server::db::context::set_tenant(&mut tx, workspace).await.unwrap();
            let counts = sqlx::query_as("SELECT (SELECT count(*) FROM fvoci.events WHERE workspace_id=$1 AND target_id=$2 AND verb='document.collab_snapshot_compacted'),(SELECT count(*) FROM fvoci.audit_log WHERE workspace_id=$1 AND target_id=$2 AND verb='document.collab_snapshot_compacted')")
                .bind(workspace).bind(document).fetch_one(&mut *tx).await.unwrap();
            tx.commit().await.unwrap();
            counts
        }
        Backend::Sqlite(pool) => sqlx::query_as("SELECT (SELECT count(*) FROM events WHERE workspace_id=?1 AND target_id=?2 AND verb='document.collab_snapshot_compacted'),(SELECT count(*) FROM audit_log WHERE workspace_id=?1 AND target_id=?2 AND verb='document.collab_snapshot_compacted')")
            .bind(workspace.as_bytes().as_slice()).bind(document.as_bytes().as_slice()).fetch_one(pool).await.unwrap(),
        Backend::LibsqlRemote(_) => unreachable!("actual remote is a separate proof"),
    }
}

async fn selected_backend_wiki_fixture(
    with_native: bool,
    membership_races: bool,
    with_compaction: bool,
) {
    use fvoci_server::db::backend::Backend;
    async fn claim_native(
        backend: &Backend,
        workspace: Uuid,
        actor: Uuid,
        credential: Uuid,
        document: Uuid,
        owner: Uuid,
    ) -> Result<
        Result<
            (
                fvoci_server::db::collab::ClaimWriterResult,
                Option<fvoci_server::db::collab::FamilyRoomFence>,
            ),
            fvoci_server::db::collab::CollabDbError,
        >,
        sqlx::Error,
    > {
        use fvoci_server::db::collab::{
            claim_family_document_room, claim_writer_and_load_kind_backend, CollabKind,
        };
        match backend {
            Backend::Postgres(_) => claim_writer_and_load_kind_backend(
                backend,
                CollabKind::Document,
                workspace,
                actor,
                credential,
                document,
            )
            .await
            .map(|result| result.map(|native| (native, None))),
            _ => claim_family_document_room(
                backend,
                workspace,
                actor,
                credential,
                document,
                owner,
                std::time::Duration::from_secs(30),
            )
            .await
            .map(|result| result.map(|claimed| (claimed.native, Some(claimed.fence)))),
        }
    }
    async fn append_native(
        backend: &Backend,
        fence: Option<fvoci_server::db::collab::FamilyRoomFence>,
        input: fvoci_server::db::collab::AppendCollabInput<'_>,
    ) -> Result<
        Result<
            fvoci_server::db::collab::AppendCollabResult,
            fvoci_server::db::collab::CollabDbError,
        >,
        sqlx::Error,
    > {
        use fvoci_server::db::collab::{append_collab_update, append_family_document_room_update};
        match backend {
            Backend::Postgres(pool) => append_collab_update(pool, input).await,
            _ => {
                append_family_document_room_update(
                    backend,
                    fence.expect("family claim must own a fence"),
                    input,
                )
                .await
            }
        }
    }
    let native_fixture = if with_native {
        let engine_bin = std::path::PathBuf::from(
            std::env::var_os("FVOCI_COLLAB_ENGINE")
                .expect("native fixture requires freshly built FVOCI_COLLAB_ENGINE"),
        );
        let content = json!({"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"Selected native durable text"}]}]});
        let seed = fvoci_server::collab::seed::SeedEngine::new(
            engine_bin.clone(),
            collab_engine::limits::Limits::default(),
        );
        let update = seed.tiptap_to_yjs_update(&content).await.unwrap();
        Some((engine_bin, content, update))
    } else {
        None
    };
    let pg = TestDb::bootstrap().await;
    let pg_backend = Backend::Postgres(pool::connect_app(&pg.app_url).await.unwrap());
    let directory = std::env::temp_dir().join(format!("fvoci-selected-backend-{}", Uuid::now_v7()));
    std::fs::create_dir(&directory).unwrap();
    let sqlite_path = directory.join("app.db");
    migrate::run_sqlite_migrations(&sqlite_path).await.unwrap();
    let sqlite_admission = migrate::SqliteAdmission::server(&sqlite_path).unwrap();
    // The actual operator path refuses a live selected-backend server.
    assert!(migrate::run_sqlite_migrations(&sqlite_path).await.is_err());
    let sqlite_backend = Backend::Sqlite(pool::connect_sqlite_app(&sqlite_path, 4).await.unwrap());
    let capability = migrate::assert_sqlite_schema_current(&sqlite_backend)
        .await
        .unwrap();
    assert_eq!(capability.lineage, migrate::SQLITE_LINEAGE);
    assert_eq!(capability.applied_steps, 3);
    let command = Uuid::now_v7();
    let room_owner = Uuid::now_v7();
    let mut membership_failures = Vec::new();
    for backend in [pg_backend, sqlite_backend] {
        let mut pending_activation_revoke = None;
        match &backend {
            Backend::Postgres(pool) => {
                let role: (String,bool,bool,bool,Option<String>) = sqlx::query_as(
                    "SELECT current_user::text,rolsuper,rolbypassrls,rolcanlogin,current_setting('app.tenant_id',true) FROM pg_catalog.pg_roles WHERE rolname=current_user"
                ).fetch_one(pool).await.unwrap();
                assert_eq!(role.0, pg.role_name);
                assert!(
                    !role.1 && !role.2 && role.3,
                    "actual app LOGIN role must be restricted"
                );
                assert!(role.4.as_deref().unwrap_or("").is_empty());
                eprintln!("selected_backend_actual_role name={} superuser={} bypassrls={} login={} tenant={:?}",role.0,role.1,role.2,role.3,role.4);
            }
            Backend::Sqlite(pool) => {
                let engine: (String,String,i64) = sqlx::query_as("SELECT sqlite_version(),sqlite_source_id(),(SELECT foreign_keys FROM pragma_foreign_keys)").fetch_one(pool).await.unwrap();
                assert_eq!(engine.0, fvoci_server::db::pool::SQLITE_VERSION);
                assert_eq!(engine.1, fvoci_server::db::pool::SQLITE_SOURCE_ID);
                assert_eq!(engine.2, 1);
                eprintln!(
                    "selected_backend_actual_sqlite version={} source={} fk={}",
                    engine.0, engine.1, engine.2
                );
            }
            Backend::LibsqlRemote(_) => unreachable!("remote primary is a separate actual proof"),
        }
        let state = app_state_backend(backend.clone()).await;
        let storage = state.storage.clone();
        let app = document_app(state);
        let (status, instance, _, _) =
            json_request(app.clone(), "GET", "/api/v1/instance", None, None, &[]).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{} instance: {instance}",
            backend.kind()
        );
        let (status, setup, cookie, _) = json_request(
            app.clone(),
            "POST",
            "/api/v1/setup",
            Some(json!({
                "email":"admin@example.com", "password":"supersecret1", "givenName":"Admin",
                "workspaceSlug":"acme", "workspaceName":"Acme"
            })),
            None,
            &[],
        )
        .await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "{} setup: {setup}",
            backend.kind()
        );
        let cookie = extract_session_cookie(cookie.as_ref().unwrap());
        let workspace = setup["workspaceId"].as_str().unwrap();
        let path = format!("/api/v1/workspaces/{workspace}/documents");
        let input = json!({"commandId":command,"parentId":null,"title":"Same selected fixture"});
        let (status, created, _, _) = json_request(
            app.clone(),
            "POST",
            &path,
            Some(input.clone()),
            Some(&cookie),
            &[],
        )
        .await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "{} create: {created}",
            backend.kind()
        );
        let (status, replayed, _, _) =
            json_request(app.clone(), "POST", &path, Some(input), Some(&cookie), &[]).await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "{} replay: {replayed}",
            backend.kind()
        );
        assert_eq!(replayed, created);
        let (status, _, _, _) = json_request(
            app.clone(),
            "POST",
            &path,
            Some(json!({"commandId":command,"parentId":null,"title":"Changed hash"})),
            Some(&cookie),
            &[],
        )
        .await;
        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "{} changed command",
            backend.kind()
        );
        let (status, login, fresh_cookie, _) = json_request(
            app.clone(),
            "POST",
            "/api/v1/auth/login",
            Some(json!({"email":"admin@example.com","password":"supersecret1"})),
            None,
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{} login: {login}", backend.kind());
        let fresh_cookie = extract_session_cookie(fresh_cookie.as_ref().unwrap());
        assert_ne!(fresh_cookie, cookie);
        let (status, me, _, _) = json_request(
            app.clone(),
            "GET",
            "/api/v1/auth/me",
            None,
            Some(&fresh_cookie),
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{} me: {me}", backend.kind());
        // The current Vue session loads this route before opening the wiki.
        let (status, workspaces, _, _) = json_request(
            app.clone(),
            "GET",
            "/api/v1/me/workspaces",
            None,
            Some(&fresh_cookie),
            &[],
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{} workspace session: {workspaces}",
            backend.kind()
        );
        let card = workspaces["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == workspace)
            .unwrap();
        assert_eq!(card["documentCount"], 1);
        assert_eq!(card["assignedCount"], 0);
        assert_eq!(card["role"], "owner");
        let (status, metadata, _, _) = json_request(
            app.clone(),
            "GET",
            &format!("/api/v1/workspaces/{workspace}"),
            None,
            Some(&fresh_cookie),
            &[],
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{} workspace metadata: {metadata}",
            backend.kind()
        );
        assert_eq!(metadata["id"], workspace);
        let (status, _, _, _) = json_request(
            app.clone(),
            "GET",
            &format!("/api/v1/workspaces/{}", Uuid::now_v7()),
            None,
            Some(&fresh_cookie),
            &[],
        )
        .await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "workspace membership is current and scoped"
        );
        let (status, _, _, _) =
            json_request(app.clone(), "GET", "/api/v1/me/workspaces", None, None, &[]).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "workspace session remains mandatory"
        );
        let document = created["id"].as_str().unwrap();
        let (status, read, _, _) = json_request(
            app.clone(),
            "GET",
            &format!("{path}/{document}"),
            None,
            Some(&fresh_cookie),
            &[],
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{} readback: {read}",
            backend.kind()
        );
        assert_eq!(read["id"], created["id"]);
        assert_eq!(read["title"], created["title"]);
        assert!(read.get("schemaVersion").is_some());
        assert_eq!(read["schemaVersion"], created["schemaVersion"]);
        let (status, body, _, _) = json_request(
            app.clone(),
            "GET",
            &format!("{path}/{document}/body"),
            None,
            Some(&fresh_cookie),
            &[],
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{} body readback: {body}",
            backend.kind()
        );
        assert!(body.get("contentJson").is_some());
        assert_eq!(
            body["contentJson"],
            fvoci_server::db::documents::empty_document_json()
        );
        assert_eq!(body["version"], created["version"]);
        // Exercise the existing native authorization/empty-only seed through
        // the real cookie identity and restricted selected-backend connection.
        // Room transport, persist ACK and revisions remain separate acceptance.
        use fvoci_server::db::collab::{
            load_collab_readonly_kind_backend, resolve_collab_admission_kind_backend,
            CollabDbError, CollabKind,
        };
        let live = fvoci_server::db::identity::find_live_session_backend(
            &backend,
            &hash_token(&fresh_cookie),
        )
        .await
        .unwrap()
        .unwrap();
        let workspace_id = Uuid::parse_str(workspace).unwrap();
        let document_id = Uuid::parse_str(document).unwrap();
        let admission = resolve_collab_admission_kind_backend(
            &backend,
            CollabKind::Document,
            workspace_id,
            live.user_id,
            live.session_id,
            document_id,
        )
        .await
        .unwrap()
        .unwrap();
        assert!(!admission.read_only);
        assert!(!admission.archived);
        let seeded = load_collab_readonly_kind_backend(
            &backend,
            CollabKind::Document,
            workspace_id,
            live.user_id,
            live.session_id,
            document_id,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(seeded.snapshot, [0, 0], "canonical empty native V1 state");
        assert!(seeded.tail.is_empty());
        assert_eq!(seeded.writer_generation, 0);
        assert_eq!(seeded.snapshot_cutoff_seq, 0);
        assert_eq!(seeded.tail_seq, 0);
        let (claimed, room_fence) = claim_native(
            &backend,
            workspace_id,
            live.user_id,
            live.session_id,
            document_id,
            room_owner,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(claimed.writer_generation, 1);
        assert_eq!(claimed.load.snapshot, seeded.snapshot);
        let reread = load_collab_readonly_kind_backend(
            &backend,
            CollabKind::Document,
            workspace_id,
            live.user_id,
            live.session_id,
            document_id,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(reread.writer_generation, 1, "read cannot claim a writer");
        assert_eq!(reread.snapshot, seeded.snapshot);
        for (tenant, actor, session, expected) in [
            (
                Uuid::now_v7(),
                live.user_id,
                live.session_id,
                CollabDbError::NotFound,
            ),
            (
                workspace_id,
                live.user_id,
                Uuid::now_v7(),
                CollabDbError::Forbidden,
            ),
            (
                workspace_id,
                Uuid::now_v7(),
                live.session_id,
                CollabDbError::Forbidden,
            ),
        ] {
            assert_eq!(
                claim_native(&backend, tenant, actor, session, document_id, room_owner)
                    .await
                    .unwrap()
                    .unwrap_err(),
                expected
            );
        }
        let unchanged = load_collab_readonly_kind_backend(
            &backend,
            CollabKind::Document,
            workspace_id,
            live.user_id,
            live.session_id,
            document_id,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            unchanged.writer_generation, 1,
            "refusal cannot bump generation"
        );
        if let Some((engine_bin, content, payload)) = &native_fixture {
            use fvoci_server::db::collab::{AppendCollabInput, AppendCollabResult};
            let operation = command;
            let input = |actor, credential, generation, op_id| AppendCollabInput {
                workspace_id,
                actor_user_id: actor,
                session_id: credential,
                document_id,
                writer_generation: generation,
                expected_tail_seq: 0,
                op_id,
                payload: payload.as_slice(),
                client_ip: None,
            };
            assert_eq!(
                append_native(
                    &backend,
                    room_fence,
                    input(live.user_id, live.session_id, 1, operation)
                )
                .await
                .unwrap()
                .unwrap(),
                AppendCollabResult::Committed { seq: 1 }
            );
            // Lost-response retry retains the native operation and exact bytes.
            assert_eq!(
                append_native(
                    &backend,
                    room_fence,
                    input(live.user_id, live.session_id, 1, operation)
                )
                .await
                .unwrap()
                .unwrap(),
                AppendCollabResult::DuplicateAck { seq: 1 }
            );
            let mut changed = payload.clone();
            changed.push(0);
            assert_eq!(
                append_native(
                    &backend,
                    room_fence,
                    AppendCollabInput {
                        payload: &changed,
                        ..input(live.user_id, live.session_id, 1, operation)
                    }
                )
                .await
                .unwrap()
                .unwrap_err(),
                CollabDbError::OpIdConflict
            );
            assert_eq!(
                append_native(
                    &backend,
                    room_fence,
                    input(Uuid::now_v7(), live.session_id, 1, operation)
                )
                .await
                .unwrap()
                .unwrap_err(),
                CollabDbError::Forbidden
            );
            assert_eq!(
                append_native(
                    &backend,
                    room_fence,
                    input(live.user_id, Uuid::now_v7(), 1, operation)
                )
                .await
                .unwrap()
                .unwrap_err(),
                CollabDbError::Forbidden
            );
            assert_eq!(
                append_native(
                    &backend,
                    room_fence,
                    input(live.user_id, live.session_id, 0, operation)
                )
                .await
                .unwrap()
                .unwrap_err(),
                CollabDbError::StaleWriter
            );
            let persisted = load_collab_readonly_kind_backend(
                &backend,
                CollabKind::Document,
                workspace_id,
                live.user_id,
                live.session_id,
                document_id,
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(persisted.tail_seq, 1);
            assert_eq!(
                persisted.tail.len(),
                1,
                "refused/replayed operations cannot add native updates"
            );
            assert_eq!(persisted.tail[0].op_id, operation);
            assert_eq!(persisted.tail[0].payload, *payload);
            let (engine, snapshot, tail) = (
                engine_bin.clone(),
                persisted.snapshot,
                persisted.tail.into_iter().map(|row| row.payload).collect(),
            );
            let backend_kind = backend.kind();
            let fresh = tokio::task::spawn_blocking(move || {
                use collab_engine::outcome::EngineStatus;
                use collab_engine::process::{ChildSlotKind, EngineSession, SpawnRequest};
                use collab_engine::protocol::Request;
                let mut child = EngineSession::spawn(SpawnRequest {
                    engine_bin: engine.clone(),
                    limits: collab_engine::limits::Limits::default(),
                    slot_kind: ChildSlotKind::Primary,
                    slot_wait: None,
                    test_hang_ms: None,
                    test_exit_after_read: None,
                    test_close_stdout_hang_ms: None,
                    test_exit_after_write: None,
                })
                .unwrap();
                let pid = child.pid().unwrap();
                let proc_path = std::path::PathBuf::from(format!("/proc/{pid}"));
                assert_eq!(
                    std::fs::read_link(proc_path.join("exe")).unwrap(),
                    std::fs::canonicalize(&engine).unwrap()
                );
                assert!(child
                    .call(&Request::Load {
                        snapshot_b64: Some(snapshot),
                        tail_b64: tail,
                        encoding: 1
                    })
                    .outcome
                    .is_applied_ok());
                let projected = match child.call(&Request::Project { encoding: 1 }).outcome {
                    EngineStatus::Ok {
                        content_json: Some(content),
                        ..
                    } => content,
                    other => panic!("fresh native projection: {other:?}"),
                };
                child.kill_and_reap();
                assert!(
                    !proc_path.exists(),
                    "owned fresh native child must be reaped"
                );
                eprintln!(
                    "native_fresh_child backend={backend_kind} pid={pid} reaped=true executable={}",
                    engine.display()
                );
                projected
            })
            .await
            .unwrap();
            assert_eq!(
                fresh, *content,
                "new isolated native client loads committed state"
            );
            use fvoci_server::db::collab::{
                project_derived_body_kind_backend, ProjectDerivedBodyInput,
                ProjectDerivedBodyResult,
            };
            let project_input = |generation, tail, session| {
                ProjectDerivedBodyInput::new(
                    workspace_id,
                    live.user_id,
                    session,
                    document_id,
                    generation,
                    tail,
                    fvoci_server::collab::derived_body::prepare_derived_body(fresh.clone())
                        .unwrap(),
                )
            };
            for (generation, tail, session, expected) in [
                (0, 1, live.session_id, CollabDbError::StaleWriter),
                (1, 0, live.session_id, CollabDbError::StaleCutoff),
                (1, 1, Uuid::now_v7(), CollabDbError::Forbidden),
            ] {
                assert_eq!(
                    project_derived_body_kind_backend(
                        &backend,
                        CollabKind::Document,
                        project_input(generation, tail, session),
                        room_fence
                    )
                    .await
                    .unwrap()
                    .unwrap_err(),
                    expected
                );
            }
            assert_eq!(
                project_derived_body_kind_backend(
                    &backend,
                    CollabKind::Document,
                    project_input(1, 1, live.session_id),
                    room_fence
                )
                .await
                .unwrap()
                .unwrap(),
                ProjectDerivedBodyResult::Updated
            );
            assert_eq!(
                project_derived_body_kind_backend(
                    &backend,
                    CollabKind::Document,
                    project_input(1, 1, live.session_id),
                    room_fence
                )
                .await
                .unwrap()
                .unwrap(),
                ProjectDerivedBodyResult::Unchanged
            );
            let (status, projected_body, _, _) = json_request(
                app.clone(),
                "GET",
                &format!("{path}/{document}/body"),
                None,
                Some(&fresh_cookie),
                &[],
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(projected_body["contentJson"], fresh);
            assert_eq!(
                projected_body["version"], created["version"],
                "derived native body does not advance metadata version"
            );
            let revisions_path = format!("{path}/{document}/revisions");
            let (status, manual, _, _) = json_request(
                app.clone(),
                "POST",
                &revisions_path,
                None,
                Some(&fresh_cookie),
                &[("origin", "http://localhost")],
            )
            .await;
            assert_eq!(
                status,
                StatusCode::CREATED,
                "{} manual: {manual}",
                backend.kind()
            );
            let revision = manual["id"].as_str().unwrap();
            let (status, repeated_manual, _, _) = json_request(
                app.clone(),
                "POST",
                &revisions_path,
                None,
                Some(&fresh_cookie),
                &[("origin", "http://localhost")],
            )
            .await;
            assert_eq!(status, StatusCode::CREATED);
            assert_eq!(
                repeated_manual, manual,
                "unchanged manual capture deduplicates"
            );
            let (status, login, readback_cookie, _) = json_request(
                app.clone(),
                "POST",
                "/api/v1/auth/login",
                Some(json!({"email":"admin@example.com","password":"supersecret1"})),
                None,
                &[],
            )
            .await;
            assert_eq!(status, StatusCode::OK, "fresh post-write login: {login}");
            let readback_cookie = extract_session_cookie(readback_cookie.as_ref().unwrap());
            assert_ne!(readback_cookie, fresh_cookie);
            let readback_identity = fvoci_server::db::identity::find_live_session_backend(
                &backend,
                &hash_token(&readback_cookie),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(readback_identity.user_id, live.user_id);
            assert_ne!(readback_identity.session_id, live.session_id);
            let (status, fresh_body, _, _) = json_request(
                app.clone(),
                "GET",
                &format!("{path}/{document}/body"),
                None,
                Some(&readback_cookie),
                &[],
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(fresh_body["contentJson"], fresh);
            let (status, list, _, _) = json_request(
                app.clone(),
                "GET",
                &revisions_path,
                None,
                Some(&readback_cookie),
                &[],
            )
            .await;
            assert_eq!(
                status,
                StatusCode::OK,
                "{} revision list: {list}",
                backend.kind()
            );
            assert_eq!(list["items"].as_array().unwrap().len(), 1);
            assert_eq!(list["items"][0]["id"], manual["id"]);
            assert_eq!(list["items"][0]["createdBy"], me["userId"]);
            assert_eq!(list["items"][0]["reason"], "manual");
            let (status, detail, _, _) = json_request(
                app.clone(),
                "GET",
                &format!("{revisions_path}/{revision}"),
                None,
                Some(&readback_cookie),
                &[],
            )
            .await;
            assert_eq!(
                status,
                StatusCode::OK,
                "{} revision detail: {detail}",
                backend.kind()
            );
            assert_eq!(detail["id"], manual["id"]);
            assert_eq!(detail["targetId"], created["id"]);
            assert_eq!(detail["contentJson"], fresh);
            let saved_snapshot =
                collab_engine::b64::decode(detail["ySnapshot"].as_str().unwrap()).unwrap();
            assert!(!saved_snapshot.is_empty());
            let current_native = load_collab_readonly_kind_backend(
                &backend,
                CollabKind::Document,
                workspace_id,
                readback_identity.user_id,
                readback_identity.session_id,
                document_id,
            )
            .await
            .unwrap()
            .unwrap();
            let before_compaction = with_compaction.then(|| current_native.clone());
            let engine = engine_bin.clone();
            let captured = tokio::task::spawn_blocking(move || {
                fvoci_server::collab::revision::capture_revision_offline(
                    engine,
                    collab_engine::limits::Limits::default(),
                    current_native.snapshot,
                    current_native
                        .tail
                        .into_iter()
                        .map(|row| row.payload)
                        .collect(),
                )
            })
            .await
            .unwrap()
            .unwrap();
            assert_eq!(
                saved_snapshot, captured.y_snapshot,
                "manual history snapshot equals freshly captured durable native source"
            );
            if with_compaction {
                use fvoci_server::db::collab::{
                    compact_collab_snapshot_kind_backend, verify_collab_operation_kind_backend,
                    CompactCollabInput, VerifyCollabInput,
                };
                use sha2::{Digest, Sha256};
                let digest = Sha256::digest(&payload).to_vec();
                let verify = |scope, credential, op, stored_actor| VerifyCollabInput {
                    workspace_id: scope,
                    actor_user_id: live.user_id,
                    session_id: credential,
                    document_id,
                    op_id: op,
                    expected_payload_len: payload.len() as i64,
                    expected_payload_sha256: &digest,
                    expected_actor_user_id: stored_actor,
                };
                let receipt = verify_collab_operation_kind_backend(
                    &backend,
                    CollabKind::Document,
                    verify(workspace_id, live.session_id, operation, live.user_id),
                )
                .await
                .unwrap()
                .unwrap();
                assert_eq!(receipt.seq, 1);
                let wrong_digest = [0; 32];
                let changed_verify = VerifyCollabInput {
                    expected_payload_sha256: &wrong_digest,
                    ..verify(workspace_id, live.session_id, operation, live.user_id)
                };
                assert_eq!(
                    verify_collab_operation_kind_backend(
                        &backend,
                        CollabKind::Document,
                        changed_verify
                    )
                    .await
                    .unwrap()
                    .unwrap_err(),
                    CollabDbError::OpIdConflict
                );
                assert_eq!(
                    verify_collab_operation_kind_backend(
                        &backend,
                        CollabKind::Document,
                        verify(workspace_id, live.session_id, operation, Uuid::now_v7())
                    )
                    .await
                    .unwrap()
                    .unwrap_err(),
                    CollabDbError::OpIdConflict
                );
                assert_eq!(
                    verify_collab_operation_kind_backend(
                        &backend,
                        CollabKind::Document,
                        verify(workspace_id, Uuid::now_v7(), operation, live.user_id)
                    )
                    .await
                    .unwrap()
                    .unwrap_err(),
                    CollabDbError::Forbidden
                );
                assert_eq!(
                    verify_collab_operation_kind_backend(
                        &backend,
                        CollabKind::Document,
                        verify(workspace_id, live.session_id, Uuid::now_v7(), live.user_id)
                    )
                    .await
                    .unwrap()
                    .unwrap_err(),
                    CollabDbError::NotFound
                );
                assert_eq!(
                    verify_collab_operation_kind_backend(
                        &backend,
                        CollabKind::Document,
                        verify(Uuid::now_v7(), live.session_id, operation, live.user_id)
                    )
                    .await
                    .unwrap()
                    .unwrap_err(),
                    CollabDbError::NotFound
                );
                let compact = |credential, generation, cutoff, tail| CompactCollabInput {
                    workspace_id,
                    actor_user_id: live.user_id,
                    session_id: credential,
                    document_id,
                    writer_generation: generation,
                    cutoff_seq: cutoff,
                    expected_tail_seq: tail,
                    new_snapshot: &captured.y_snapshot,
                    client_ip: None,
                };
                assert_eq!(
                    selected_compaction_effect_counts(&backend, workspace_id, document_id).await,
                    (0, 0)
                );
                for (credential, generation, cutoff, tail, error) in [
                    (live.session_id, 0, 1, 1, CollabDbError::StaleWriter),
                    (live.session_id, 1, 1, 0, CollabDbError::StaleCutoff),
                    (live.session_id, 1, 2, 1, CollabDbError::InvalidCutoff),
                    (live.session_id, 1, 0, 1, CollabDbError::InvalidCutoff),
                    (Uuid::now_v7(), 1, 1, 1, CollabDbError::Forbidden),
                ] {
                    assert_eq!(
                        compact_collab_snapshot_kind_backend(
                            &backend,
                            CollabKind::Document,
                            compact(credential, generation, cutoff, tail),
                            room_fence
                        )
                        .await
                        .unwrap()
                        .unwrap_err(),
                        error
                    );
                }
                if room_fence.is_some() {
                    assert_eq!(
                        compact_collab_snapshot_kind_backend(
                            &backend,
                            CollabKind::Document,
                            compact(live.session_id, 1, 1, 1),
                            None
                        )
                        .await
                        .unwrap()
                        .unwrap_err(),
                        CollabDbError::StaleWriter
                    );
                }
                assert_eq!(
                    selected_compaction_effect_counts(&backend, workspace_id, document_id).await,
                    (0, 0)
                );
                let before = load_collab_readonly_kind_backend(
                    &backend,
                    CollabKind::Document,
                    workspace_id,
                    live.user_id,
                    live.session_id,
                    document_id,
                )
                .await
                .unwrap()
                .unwrap();
                assert_eq!(
                    Some(&before),
                    before_compaction.as_ref(),
                    "all refused compactions leave canonical native source unchanged"
                );
                let compacted = compact_collab_snapshot_kind_backend(
                    &backend,
                    CollabKind::Document,
                    compact(live.session_id, 1, 1, 1),
                    room_fence,
                )
                .await
                .unwrap()
                .unwrap();
                assert_eq!(compacted.snapshot, captured.y_snapshot);
                assert!(compacted.tail.is_empty());
                assert_eq!(
                    (
                        compacted.writer_generation,
                        compacted.snapshot_cutoff_seq,
                        compacted.tail_seq
                    ),
                    (1, 1, 1)
                );
                assert_eq!(
                    selected_compaction_effect_counts(&backend, workspace_id, document_id).await,
                    (1, 1)
                );
                let repeated = compact_collab_snapshot_kind_backend(
                    &backend,
                    CollabKind::Document,
                    compact(live.session_id, 1, 1, 1),
                    room_fence,
                )
                .await
                .unwrap()
                .unwrap();
                assert_eq!(
                    repeated, compacted,
                    "identical repeat leaves durable native source unchanged"
                );
                assert_eq!(
                    selected_compaction_effect_counts(&backend, workspace_id, document_id).await,
                    (1, 1),
                    "repeat emits no new event/audit"
                );
                let after_receipt = verify_collab_operation_kind_backend(
                    &backend,
                    CollabKind::Document,
                    verify(workspace_id, live.session_id, operation, live.user_id),
                )
                .await
                .unwrap()
                .unwrap();
                assert_eq!(
                    after_receipt, receipt,
                    "tail compaction preserves exact immutable operation receipt"
                );
                let fresh = load_collab_readonly_kind_backend(
                    &backend,
                    CollabKind::Document,
                    workspace_id,
                    readback_identity.user_id,
                    readback_identity.session_id,
                    document_id,
                )
                .await
                .unwrap()
                .unwrap();
                assert_eq!(
                    fresh, compacted,
                    "fresh cookie actor sees exact committed snapshot/head"
                );
                let engine = engine_bin.clone();
                let compacted_body = tokio::task::spawn_blocking(move || {
                    fvoci_server::collab::revision::capture_revision_offline(
                        engine,
                        collab_engine::limits::Limits::default(),
                        fresh.snapshot,
                        Vec::new(),
                    )
                })
                .await
                .unwrap()
                .unwrap();
                assert_eq!(compacted_body.y_snapshot, captured.y_snapshot);
                assert_eq!(
                    compacted_body.content_json, detail["contentJson"],
                    "compacted canonical native snapshot preserves current body/history IDs"
                );
                eprintln!("native_compaction backend={} cutoff=1 tail=1 generation=1 updates=0 receipts_preserved=true event=1 audit=1 repeat_no_effect=true",backend.kind());
            }
            let (status, _, _, _) = json_request(
                app.clone(),
                "GET",
                &format!(
                    "/api/v1/workspaces/{}/documents/{document}/revisions/{revision}",
                    Uuid::now_v7()
                ),
                None,
                Some(&readback_cookie),
                &[],
            )
            .await;
            assert_eq!(
                status,
                StatusCode::NOT_FOUND,
                "wrong tenant cannot read revision"
            );
            let (viewer, _viewer_cookie) =
                selected_fixture_actor(&backend, &pg, &app, workspace_id, "guest", "native-viewer")
                    .await;
            selected_fixture_view_grant(&backend, &pg, workspace_id, viewer.user_id, document_id)
                .await;
            let view_load = load_collab_readonly_kind_backend(
                &backend,
                CollabKind::Document,
                workspace_id,
                viewer.user_id,
                viewer.session_id,
                document_id,
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(
                view_load.tail_seq, 1,
                "real View-only actor reads durable native history"
            );
            assert_eq!(
                claim_native(
                    &backend,
                    workspace_id,
                    viewer.user_id,
                    viewer.session_id,
                    document_id,
                    Uuid::now_v7()
                )
                .await
                .unwrap()
                .unwrap_err(),
                CollabDbError::Forbidden,
                "real View-only actor cannot claim a writer"
            );
            if let Some(fence) = room_fence {
                use fvoci_server::db::collab::{
                    claim_family_document_room, release_family_document_room,
                    renew_family_document_room,
                };
                let retry = claim_family_document_room(
                    &backend,
                    workspace_id,
                    live.user_id,
                    live.session_id,
                    document_id,
                    room_owner,
                    std::time::Duration::from_secs(30),
                )
                .await
                .unwrap()
                .unwrap();
                assert_eq!(retry.fence, fence);
                assert_eq!(
                    retry.native.writer_generation, 1,
                    "same live owner retry cannot replace generation"
                );
                let other_owner = Uuid::now_v7();
                assert_eq!(
                    claim_family_document_room(
                        &backend,
                        workspace_id,
                        live.user_id,
                        live.session_id,
                        document_id,
                        other_owner,
                        std::time::Duration::from_secs(30)
                    )
                    .await
                    .unwrap()
                    .unwrap_err(),
                    CollabDbError::StaleWriter
                );
                assert!(renew_family_document_room(
                    &backend,
                    fence,
                    std::time::Duration::from_secs(30)
                )
                .await
                .unwrap());
                assert!(release_family_document_room(&backend, fence).await.unwrap());
                assert_eq!(
                    project_derived_body_kind_backend(
                        &backend,
                        CollabKind::Document,
                        project_input(1, 1, live.session_id),
                        Some(fence)
                    )
                    .await
                    .unwrap()
                    .unwrap_err(),
                    CollabDbError::StaleWriter,
                    "expired room cannot project even an unchanged native body"
                );
                assert_eq!(
                    append_native(
                        &backend,
                        Some(fence),
                        input(live.user_id, live.session_id, 1, operation)
                    )
                    .await
                    .unwrap()
                    .unwrap_err(),
                    CollabDbError::StaleWriter,
                    "expired ownership cannot replay a receipt"
                );
                let replacement = claim_family_document_room(
                    &backend,
                    workspace_id,
                    live.user_id,
                    live.session_id,
                    document_id,
                    other_owner,
                    std::time::Duration::from_secs(30),
                )
                .await
                .unwrap()
                .unwrap();
                assert_ne!(replacement.fence, fence);
                assert_eq!(replacement.native.writer_generation, 2);
                assert_eq!(
                    project_derived_body_kind_backend(
                        &backend,
                        CollabKind::Document,
                        project_input(2, 1, live.session_id),
                        Some(fence)
                    )
                    .await
                    .unwrap()
                    .unwrap_err(),
                    CollabDbError::StaleWriter,
                    "old owner cannot project against replacement generation"
                );
                assert_eq!(
                    project_derived_body_kind_backend(
                        &backend,
                        CollabKind::Document,
                        project_input(2, 1, live.session_id),
                        Some(replacement.fence),
                    )
                    .await
                    .unwrap()
                    .unwrap(),
                    ProjectDerivedBodyResult::Unchanged,
                    "current replacement owner can project the unchanged native head"
                );
                assert!(!renew_family_document_room(
                    &backend,
                    fence,
                    std::time::Duration::from_secs(30)
                )
                .await
                .unwrap());
                assert!(
                    !release_family_document_room(&backend, fence).await.unwrap(),
                    "old owner cannot release replacement"
                );
                assert_eq!(
                    append_native(
                        &backend,
                        Some(fence),
                        input(live.user_id, live.session_id, 2, operation)
                    )
                    .await
                    .unwrap()
                    .unwrap_err(),
                    CollabDbError::StaleWriter
                );
                assert_eq!(
                    append_native(
                        &backend,
                        Some(replacement.fence),
                        input(live.user_id, live.session_id, 2, operation)
                    )
                    .await
                    .unwrap()
                    .unwrap(),
                    AppendCollabResult::DuplicateAck { seq: 1 }
                );
                assert!(release_family_document_room(&backend, replacement.fence)
                    .await
                    .unwrap());
                use fvoci_server::db::collab::{
                    acquire_family_document_room, activate_family_document_writer,
                };
                let view_reader = acquire_family_document_room(
                    &backend,
                    workspace_id,
                    viewer.user_id,
                    viewer.session_id,
                    document_id,
                    Uuid::now_v7(),
                    std::time::Duration::from_secs(30),
                )
                .await
                .unwrap()
                .unwrap();
                assert_eq!(
                    view_reader.native.writer_generation, 2,
                    "actual View-only reader must not need Edit or advance generation"
                );
                let protected =
                    selected_family_fence_snapshot(&backend, workspace_id, document_id).await;
                assert_eq!(
                    activate_family_document_writer(
                        &backend,
                        view_reader.fence,
                        viewer.user_id,
                        viewer.session_id,
                        Uuid::now_v7()
                    )
                    .await
                    .unwrap()
                    .unwrap_err(),
                    CollabDbError::Forbidden,
                    "actual View-only reader cannot activate a writer"
                );
                assert_eq!(
                    selected_family_fence_snapshot(&backend, workspace_id, document_id).await,
                    protected,
                    "refused activation preserves owner/fence/generation/tail"
                );
                assert!(release_family_document_room(&backend, view_reader.fence)
                    .await
                    .unwrap());
                let reader = acquire_family_document_room(
                    &backend,
                    workspace_id,
                    live.user_id,
                    live.session_id,
                    document_id,
                    Uuid::now_v7(),
                    std::time::Duration::from_secs(30),
                )
                .await
                .unwrap()
                .unwrap();
                assert_eq!(
                    reader.native.writer_generation, 2,
                    "reader room ownership does not claim native writer generation"
                );
                let writer_owner = Uuid::now_v7();
                let writer = activate_family_document_writer(
                    &backend,
                    reader.fence,
                    live.user_id,
                    live.session_id,
                    writer_owner,
                )
                .await
                .unwrap()
                .unwrap();
                assert_eq!(writer.native.writer_generation, 3);
                assert_eq!(writer.native.load.tail_seq, 1);
                assert_ne!(
                    writer.fence, reader.fence,
                    "stable writer activation changes the opaque owner proof"
                );
                let replay = activate_family_document_writer(
                    &backend,
                    reader.fence,
                    live.user_id,
                    live.session_id,
                    writer_owner,
                )
                .await
                .unwrap()
                .unwrap();
                assert_eq!(
                    replay.native.writer_generation, 3,
                    "same activation token reconciles without a second generation bump"
                );
                assert_eq!(replay.fence, writer.fence);
                let protected =
                    selected_family_fence_snapshot(&backend, workspace_id, document_id).await;
                // Keep a real second owner so this synthetic downgrade does
                // not manufacture an ownerless workspace invariant violation.
                let _backup_owner = selected_fixture_actor(
                    &backend,
                    &pg,
                    &app,
                    workspace_id,
                    "owner",
                    "backup-owner",
                )
                .await;
                selected_fixture_membership(
                    &backend,
                    &pg,
                    workspace_id,
                    live.user_id,
                    Some("guest"),
                )
                .await;
                assert_eq!(
                    activate_family_document_writer(
                        &backend,
                        reader.fence,
                        live.user_id,
                        live.session_id,
                        writer_owner
                    )
                    .await
                    .unwrap()
                    .unwrap_err(),
                    CollabDbError::Forbidden,
                    "committed owner downgrade precedes known-token activation replay"
                );
                assert_eq!(
                    selected_family_fence_snapshot(&backend, workspace_id, document_id).await,
                    protected
                );
                selected_fixture_membership(
                    &backend,
                    &pg,
                    workspace_id,
                    live.user_id,
                    Some("owner"),
                )
                .await;
                assert_eq!(
                    activate_family_document_writer(
                        &backend,
                        reader.fence,
                        live.user_id,
                        Uuid::now_v7(),
                        writer_owner
                    )
                    .await
                    .unwrap()
                    .unwrap_err(),
                    CollabDbError::Forbidden,
                    "current credential precedes stable activation replay"
                );
                assert_eq!(
                    activate_family_document_writer(
                        &backend,
                        reader.fence,
                        live.user_id,
                        live.session_id,
                        Uuid::now_v7()
                    )
                    .await
                    .unwrap()
                    .unwrap_err(),
                    CollabDbError::StaleWriter,
                    "a different activation cannot steal current ownership"
                );
                assert!(!renew_family_document_room(
                    &backend,
                    reader.fence,
                    std::time::Duration::from_secs(30)
                )
                .await
                .unwrap());
                assert!(!release_family_document_room(&backend, reader.fence)
                    .await
                    .unwrap());
                assert_eq!(
                    project_derived_body_kind_backend(
                        &backend,
                        CollabKind::Document,
                        project_input(3, 1, live.session_id),
                        Some(reader.fence)
                    )
                    .await
                    .unwrap()
                    .unwrap_err(),
                    CollabDbError::StaleWriter
                );
                assert_eq!(
                    project_derived_body_kind_backend(
                        &backend,
                        CollabKind::Document,
                        project_input(3, 1, live.session_id),
                        Some(writer.fence)
                    )
                    .await
                    .unwrap()
                    .unwrap(),
                    ProjectDerivedBodyResult::Unchanged
                );
                pending_activation_revoke = Some((reader.fence, writer_owner, writer.fence));
            }
        }
        let (status, _, _, _) = json_request(
            app.clone(),
            "GET",
            &format!(
                "/api/v1/workspaces/{}/documents/{document}/body",
                Uuid::now_v7()
            ),
            None,
            Some(&fresh_cookie),
            &[],
        )
        .await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "{} wrong-tenant body",
            backend.kind()
        );
        let (status, _, _, _) = json_request(
            app.clone(),
            "GET",
            &format!("{path}/{document}/body"),
            None,
            None,
            &[],
        )
        .await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "{} anonymous body",
            backend.kind()
        );
        if membership_races {
            membership_failures
                .extend(selected_workspace_race_controls(&backend, &pg, &app, workspace_id).await);
        }
        if let Some((original, writer_owner, current)) = pending_activation_revoke {
            let protected =
                selected_family_fence_snapshot(&backend, workspace_id, document_id).await;
            let Backend::Sqlite(pool) = &backend else {
                unreachable!()
            };
            let mut tx = pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
            assert_eq!(sqlx::query("UPDATE sessions SET revoked_at=(unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000) WHERE id=?1 AND user_id=?2 AND revoked_at IS NULL").bind(live.session_id.as_bytes().to_vec()).bind(live.user_id.as_bytes().to_vec()).execute(&mut *tx).await.unwrap().rows_affected(),1);
            tx.commit().await.unwrap();
            assert!(
                fvoci_server::db::identity::find_live_session_backend(
                    &backend,
                    &hash_token(&fresh_cookie)
                )
                .await
                .unwrap()
                .is_none(),
                "actual live credential was durably revoked"
            );
            assert_eq!(
                fvoci_server::db::collab::activate_family_document_writer(
                    &backend,
                    original,
                    live.user_id,
                    live.session_id,
                    writer_owner
                )
                .await
                .unwrap()
                .unwrap_err(),
                CollabDbError::Forbidden,
                "committed credential revoke precedes known-token activation replay"
            );
            assert_eq!(
                selected_family_fence_snapshot(&backend, workspace_id, document_id).await,
                protected
            );
            assert!(
                fvoci_server::db::collab::release_family_document_room(&backend, current)
                    .await
                    .unwrap()
            );
        }
        drop(app);
        let storage_root = match &storage {
            fvoci_server::attachments::ObjectStorage::Local(local) => local.root().to_path_buf(),
            _ => unreachable!("owned local fixture storage"),
        };
        drop(storage);
        backend.close().await.unwrap();
        std::fs::remove_dir_all(storage_root).unwrap();
    }
    drop(sqlite_admission);
    // Idempotent restart reads all real step receipts and definitions.
    migrate::run_sqlite_migrations(&sqlite_path).await.unwrap();
    pg.cleanup().await;
    std::fs::remove_dir_all(directory).unwrap();
    assert!(
        membership_failures.is_empty(),
        "current authority violations after committed changes: {membership_failures:#?}"
    );
}

struct MigrationCommitPause(Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>);
impl MigrationCommitPause {
    fn release(&self) {
        *self.0 .0.lock().unwrap() = true;
        self.0 .1.notify_all();
    }
}
impl Drop for MigrationCommitPause {
    fn drop(&mut self) {
        self.release();
    }
}
async fn pause_actual_sqlite_commit(
    pool: &sqlx::SqlitePool,
) -> (MigrationCommitPause, tokio::sync::oneshot::Receiver<()>) {
    let pause = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let callback_pause = pause.clone();
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let mut entered_tx = Some(entered_tx);
    let mut conn = pool.acquire().await.unwrap();
    {
        let mut native = conn.lock_handle().await.unwrap();
        // Supported SQLx hook blocks its actual SQLite worker at COMMIT,
        // after real DDL/marker operations, without faking a driver result.
        native.set_commit_hook(move || {
            if let Some(entered) = entered_tx.take() {
                let _ = entered.send(());
            }
            let mut released = callback_pause.0.lock().unwrap();
            while !*released {
                released = callback_pause.1.wait(released).unwrap();
            }
            true // SQLx maps true to SQLite's zero/allow-commit callback result
        });
    }
    drop(conn);
    (MigrationCommitPause(pause), entered_rx)
}

#[tokio::test]
async fn sqlite_migration_cancelled_commit_retains_admission_until_drain() {
    tokio::task::LocalSet::new()
        .run_until(async {
            use migrate::{SqliteAdmission, SqliteMigrationDrain, SqliteMigrationTestControl};
            let directory =
                std::env::temp_dir().join(format!("fvoci-migration-cancel-{}", Uuid::now_v7()));
            std::fs::create_dir(&directory).unwrap();
            // Old c961 lifecycle control on the same real SQLite/COMMIT barrier.
            // It decisively admits a server while its original worker is still paused.
            let old_path = directory.join("legacy.db");
            let (connected_tx, connected_rx) = tokio::sync::oneshot::channel();
            let (proceed_tx, proceed_rx) = tokio::sync::oneshot::channel();
            let old_request_path = old_path.clone();
            let old_request = tokio::task::spawn_local(async move {
                migrate::run_sqlite_migrations_legacy_control(
                    &old_request_path,
                    SqliteMigrationTestControl {
                        connected: connected_tx,
                        proceed: proceed_rx,
                        cleanup_started: None,
                    },
                )
                .await
            });
            let old_pool = connected_rx.await.unwrap();
            let (old_pause, old_entered) = pause_actual_sqlite_commit(&old_pool).await;
            proceed_tx.send(()).unwrap();
            old_entered.await.unwrap();
            old_request.abort();
            assert!(old_request.await.unwrap_err().is_cancelled());
            let wrongly_admitted = SqliteAdmission::server(&old_path)
                .expect("old request-owned guard releases before the paused worker drains");
            drop(wrongly_admitted);
            old_pause.release();
            // The old request has no cleanup owner. The control itself must
            // obtain its actual worker-shutdown receipt before closing the pool.
            old_pool.acquire().await.unwrap().close().await.unwrap();
            old_pool.close().await;
            assert!(old_pool.is_closed());
            assert_eq!(old_pool.size(), 0);
            drop(old_pause);

            // New finite cleanup owner survives both request and request-runtime Drop.
            let new_path = directory.join("owned.db");
            let (connected_tx, connected_rx) = tokio::sync::oneshot::channel();
            let (proceed_tx, proceed_rx) = tokio::sync::oneshot::channel();
            let run = migrate::start_sqlite_migration_controlled(
                &new_path,
                SqliteMigrationTestControl {
                    connected: connected_tx,
                    proceed: proceed_rx,
                    cleanup_started: None,
                },
            )
            .unwrap();
            let observer = run.observer();
            let (drop_runtime_tx, drop_runtime_rx) = tokio::sync::oneshot::channel();
            let caller = std::thread::spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                runtime.spawn(run.wait());
                runtime.block_on(async {
                    drop_runtime_rx.await.unwrap();
                });
                drop(runtime); // aborts/drops the waiting request, never the cleanup owner's runtime
            });
            let new_pool = connected_rx.await.unwrap();
            let (new_pause, new_entered) = pause_actual_sqlite_commit(&new_pool).await;
            proceed_tx.send(()).unwrap();
            new_entered.await.unwrap();
            drop_runtime_tx.send(()).unwrap();
            caller.join().unwrap();
            assert!(
                SqliteAdmission::server(&new_path).is_err(),
                "server refused while cancelled COMMIT is unsettled"
            );
            assert!(
                migrate::run_sqlite_migrations(&new_path).await.is_err(),
                "second migrator refused while cleanup owns admission"
            );
            new_pause.release();
            assert_eq!(observer.wait().await.unwrap(), SqliteMigrationDrain::Closed);
            assert!(new_pool.is_closed());
            assert_eq!(new_pool.size(), 0);
            let after_drain = SqliteAdmission::server(&new_path)
                .expect("admission available only after confirmed drain");
            drop(after_drain);
            drop(new_pause);

            for path in [old_path, new_path] {
                let db = pool::connect_sqlite_app(&path, 1).await.unwrap();
                let prefix: Vec<i64> =
                    sqlx::query_scalar("SELECT version FROM schema_migrations ORDER BY version")
                        .fetch_all(&db)
                        .await
                        .unwrap();
                assert_eq!(
            prefix,
            vec![1],
            "paused COMMIT settled one whole DDL/marker step, with no next step after cancellation"
        );
                db.close().await;
                migrate::run_sqlite_migrations(&path).await.unwrap();
                let db = pool::connect_sqlite_app(&path, 1).await.unwrap();
                let backend = fvoci_server::db::backend::Backend::Sqlite(db);
                assert_eq!(
                    migrate::assert_sqlite_schema_current(&backend)
                        .await
                        .unwrap()
                        .applied_steps,
                    3
                );
                backend.close().await.unwrap();
            }
            // Cancellation during preparation proceeds to a close that is physically
            // blocked by this real leased connection. Aborting the waiting request
            // during that close must not release admission either.
            let close_path = directory.join("closing.db");
            migrate::run_sqlite_migrations(&close_path).await.unwrap();
            let db = pool::connect_sqlite_app(&close_path, 1).await.unwrap();
            let before: Vec<(i64, String, i64)> = sqlx::query_as(
                "SELECT version,sql_sha256,applied_at FROM schema_migrations ORDER BY version",
            )
            .fetch_all(&db)
            .await
            .unwrap();
            db.close().await;
            let (connected_tx, connected_rx) = tokio::sync::oneshot::channel();
            let (_proceed_tx, proceed_rx) = tokio::sync::oneshot::channel();
            let (cleanup_started_tx, cleanup_started_rx) = tokio::sync::oneshot::channel();
            let run = migrate::start_sqlite_migration_controlled(
                &close_path,
                SqliteMigrationTestControl {
                    connected: connected_tx,
                    proceed: proceed_rx,
                    cleanup_started: Some(cleanup_started_tx),
                },
            )
            .unwrap();
            let close_observer = run.observer();
            let closing_pool = connected_rx.await.unwrap();
            let held_connection = closing_pool.acquire().await.unwrap();
            run.cancel();
            let request = tokio::spawn(run.wait());
            cleanup_started_rx.await.unwrap();
            request.abort();
            assert!(request.await.unwrap_err().is_cancelled());
            assert!(
                SqliteAdmission::server(&close_path).is_err(),
                "caller cancellation cannot release blocked close admission"
            );
            drop(held_connection);
            assert_eq!(
                close_observer.wait().await.unwrap(),
                SqliteMigrationDrain::Closed
            );
            assert_eq!(closing_pool.size(), 0);
            let db = pool::connect_sqlite_app(&close_path, 1).await.unwrap();
            let after: Vec<(i64, String, i64)> = sqlx::query_as(
                "SELECT version,sql_sha256,applied_at FROM schema_migrations ORDER BY version",
            )
            .fetch_all(&db)
            .await
            .unwrap();
            assert_eq!(
                after, before,
                "cancelled preparation leaves the existing complete prefix unchanged"
            );
            db.close().await;
            migrate::run_sqlite_migrations(&close_path).await.unwrap();
            // Pool-level shutdown hides retirement errors. An owner that never
            // received the original worker's close result must quarantine, even if
            // acquiring a replacement succeeds and that replacement closes cleanly.
            let replaced_path = directory.join("replaced.db");
            let (connected_tx, connected_rx) = tokio::sync::oneshot::channel();
            let (_proceed_tx, proceed_rx) = tokio::sync::oneshot::channel();
            let run = migrate::start_sqlite_migration_controlled(
                &replaced_path,
                SqliteMigrationTestControl {
                    connected: connected_tx,
                    proceed: proceed_rx,
                    cleanup_started: None,
                },
            )
            .unwrap();
            let observer = run.observer();
            let replaced_pool = connected_rx.await.unwrap();
            // This real close receipt belongs to the test, not the migration owner.
            // The owner's later acquire must not bless this replacement as its drain.
            replaced_pool
                .acquire()
                .await
                .unwrap()
                .close()
                .await
                .unwrap();
            run.cancel();
            let error = run.wait().await.unwrap_err();
            assert!(error.to_string().contains("cleanup is unconfirmed"));
            assert_eq!(
                observer.wait().await.unwrap(),
                SqliteMigrationDrain::Quarantined
            );
            assert!(SqliteAdmission::server(&replaced_path).is_err());
            assert_eq!(replaced_pool.size(), 0);
            // Only the fail-closed inode lock remains until this test process exits;
            // both real worker connections were explicitly closed above/by the owner.
            std::fs::remove_dir_all(directory).unwrap();
        })
        .await;
}
