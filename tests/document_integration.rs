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

#[path = "support/collab_projection.rs"]
mod selected_room_support;

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

// Trusted synthetic preparation, then the real selected app transaction checks
// the same ON body scope/credential/project rules on PG and SQLite.
async fn selected_body_scope_authorization_controls(
    backend: &fvoci_server::db::backend::Backend,
    pg_admin_url: &str,
    target: (Uuid, Uuid),
    actor: Uuid,
    credential: Uuid,
) {
    use fvoci_server::db::backend::Backend;
    use fvoci_server::db::document_ops::{authorize_document_backend, DocumentScope};
    use fvoci_server::db::documents::DocumentDbError;
    use fvoci_server::projects::ProjectPermission;
    let (workspace, wiki) = target;
    let project = Uuid::now_v7();
    let document = Uuid::now_v7();
    let pg = match backend {
        Backend::Postgres(_) => Some(
            PgPoolOptions::new()
                .max_connections(1)
                .connect(pg_admin_url)
                .await
                .unwrap(),
        ),
        _ => None,
    };
    match backend {
        Backend::Postgres(_) => {
            let mut tx = pg.as_ref().unwrap().begin().await.unwrap();
            sqlx::query("INSERT INTO fvoci.projects(id,workspace_id,key,name,visibility,created_by) VALUES($1,$2,'BODY','Body scope','workspace',$3)")
                .bind(project).bind(workspace).bind(actor).execute(&mut *tx).await.unwrap();
            sqlx::query("INSERT INTO fvoci.documents(id,workspace_id,project_id,title,path,sort_key,number,status,schema_version,content_json,created_by) VALUES($1,$2,$3,'Body scope',$4,'a0',1,'draft',2,$5,$6)")
                .bind(document).bind(workspace).bind(project).bind(document.simple().to_string()).bind(fvoci_server::db::documents::empty_document_json()).bind(actor).execute(&mut *tx).await.unwrap();
            tx.commit().await.unwrap();
        }
        Backend::Sqlite(pool) => {
            let mut tx = pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
            sqlx::query("INSERT INTO projects(id,workspace_id,key,name,visibility,created_by) VALUES(?1,?2,'BODY','Body scope','workspace',?3)")
                .bind(project.as_bytes().as_slice()).bind(workspace.as_bytes().as_slice()).bind(actor.as_bytes().as_slice()).execute(&mut *tx).await.unwrap();
            sqlx::query("INSERT INTO documents(id,workspace_id,project_id,title,path,sort_key,number,status,schema_version,content_json,created_by) VALUES(?1,?2,?3,'Body scope',?4,'a0',1,'draft',2,?5,?6)")
                .bind(document.as_bytes().as_slice()).bind(workspace.as_bytes().as_slice()).bind(project.as_bytes().as_slice()).bind(document.simple().to_string()).bind(fvoci_server::db::documents::empty_document_json().to_string()).bind(actor.as_bytes().as_slice()).execute(&mut *tx).await.unwrap();
            tx.commit().await.unwrap();
        }
        Backend::LibsqlRemote(_) => {
            unreachable!("actual remote primary proof remains separately required")
        }
    }
    let read = |tenant, session, scope, target, min| {
        authorize_document_backend(backend, tenant, actor, session, scope, target, min)
    };
    let meta = read(
        workspace,
        credential,
        DocumentScope::Wiki,
        wiki,
        ProjectPermission::Edit,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(meta.id, wiki);
    assert_eq!(meta.display_id, None);
    assert!(matches!(
        read(
            workspace,
            Uuid::now_v7(),
            DocumentScope::Wiki,
            wiki,
            ProjectPermission::Edit
        )
        .await
        .unwrap(),
        Err(DocumentDbError::Forbidden)
    ));
    for (tenant, scope, target) in [
        (Uuid::now_v7(), DocumentScope::Wiki, wiki),
        (workspace, DocumentScope::Project(project), wiki),
        (workspace, DocumentScope::Wiki, document),
        (workspace, DocumentScope::Project(Uuid::now_v7()), document),
    ] {
        assert!(matches!(
            read(tenant, credential, scope, target, ProjectPermission::View)
                .await
                .unwrap(),
            Err(DocumentDbError::NotFound)
        ));
    }
    for (phase, view, edit) in [
        ("active", true, true),
        ("archived", true, false),
        ("private", false, false),
        ("grant", true, true),
        ("deleted", false, false),
    ] {
        match backend {
            Backend::Postgres(_) => {
                let mut tx = pg.as_ref().unwrap().begin().await.unwrap();
                match phase {
                    "archived" => {
                        sqlx::query("UPDATE fvoci.projects SET status='archived' WHERE workspace_id=$1 AND id=$2").bind(workspace).bind(project).execute(&mut *tx).await.unwrap();
                    }
                    "private" => {
                        sqlx::query("UPDATE fvoci.projects SET status='active',visibility='private' WHERE workspace_id=$1 AND id=$2").bind(workspace).bind(project).execute(&mut *tx).await.unwrap();
                    }
                    "grant" => {
                        sqlx::query("INSERT INTO fvoci.project_members(workspace_id,project_id,user_id,role) VALUES($1,$2,$3,'lead')").bind(workspace).bind(project).bind(actor).execute(&mut *tx).await.unwrap();
                    }
                    "deleted" => {
                        sqlx::query("UPDATE fvoci.projects SET deleted_at=now() WHERE workspace_id=$1 AND id=$2").bind(workspace).bind(project).execute(&mut *tx).await.unwrap();
                    }
                    _ => {}
                }
                tx.commit().await.unwrap();
            }
            Backend::Sqlite(pool) => {
                let mut tx = pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
                match phase {
                    "archived" => {
                        sqlx::query(
                            "UPDATE projects SET status='archived' WHERE workspace_id=?1 AND id=?2",
                        )
                        .bind(workspace.as_bytes().as_slice())
                        .bind(project.as_bytes().as_slice())
                        .execute(&mut *tx)
                        .await
                        .unwrap();
                    }
                    "private" => {
                        sqlx::query("UPDATE projects SET status='active',visibility='private' WHERE workspace_id=?1 AND id=?2").bind(workspace.as_bytes().as_slice()).bind(project.as_bytes().as_slice()).execute(&mut *tx).await.unwrap();
                    }
                    "grant" => {
                        sqlx::query("INSERT INTO project_members(workspace_id,project_id,user_id,role) VALUES(?1,?2,?3,'lead')").bind(workspace.as_bytes().as_slice()).bind(project.as_bytes().as_slice()).bind(actor.as_bytes().as_slice()).execute(&mut *tx).await.unwrap();
                    }
                    "deleted" => {
                        sqlx::query(
                            "UPDATE projects SET deleted_at=1 WHERE workspace_id=?1 AND id=?2",
                        )
                        .bind(workspace.as_bytes().as_slice())
                        .bind(project.as_bytes().as_slice())
                        .execute(&mut *tx)
                        .await
                        .unwrap();
                    }
                    _ => {}
                }
                tx.commit().await.unwrap();
            }
            Backend::LibsqlRemote(_) => unreachable!(),
        }
        for (min, allowed) in [
            (ProjectPermission::View, view),
            (ProjectPermission::Edit, edit),
        ] {
            let result = read(
                workspace,
                credential,
                DocumentScope::Project(project),
                document,
                min,
            )
            .await
            .unwrap();
            if allowed {
                let meta = result.unwrap();
                assert_eq!(meta.id, document);
                assert_eq!(meta.project_id, Some(project));
                assert_eq!(meta.display_id.as_deref(), Some("BODY-1"));
            } else {
                assert!(
                    matches!(result, Err(DocumentDbError::NotFound)),
                    "{phase} {min:?}"
                );
            }
        }
    }
    if let Some(pg) = pg {
        pg.close().await;
    }
    eprintln!("selected_body_scope_authorization backend={} current_credential=true tenant_scope=true wiki_project_affiliation=true archived_edit_denied=true private_grant_current=true live_project_required=true",backend.kind());
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
    selected_backend_wiki_fixture(false, false, false, false).await;
}

#[tokio::test]
async fn selected_backend_native_append_fresh_child_readback() {
    selected_backend_wiki_fixture(true, false, false, false).await;
}

#[tokio::test]
async fn selected_backend_workspace_current_membership_race() {
    selected_backend_wiki_fixture(false, true, false, false).await;
}

#[tokio::test]
async fn selected_backend_native_compaction_receipt_readback() {
    selected_backend_wiki_fixture(true, false, true, false).await;
}

/// Real socket/actor/native helper receipt. The normal process + actual Vue
/// browser tracer is still a separate required acceptance, not this test.
#[tokio::test]
async fn selected_backend_room_transport_persist_revision_readback() {
    selected_backend_wiki_fixture(false, false, false, true).await;
}

/// Actual SQLite writer/fence controls for cached native consumers and cleanup.
/// PostgreSQL keeps its session/advisory-lock path and unchanged regression suite.
#[tokio::test]
async fn selected_family_cached_native_fence_and_blocked_release_controls() {
    use fvoci_server::collab::room::{
        arm_native_consumer_barrier, RoomKey, MANUAL_REVISION_BEFORE_WRITE,
        NATIVE_CAPTURE_FINAL_PROOF, NATIVE_PROJECT_FINAL_PROOF,
    };
    use fvoci_server::db::backend::Backend;
    use selected_room_support as room;
    let directory =
        std::env::temp_dir().join(format!("fvoci-family-consumer-fence-{}", Uuid::now_v7()));
    std::fs::create_dir(&directory).unwrap();
    let database = directory.join("app.db");
    migrate::run_sqlite_migrations(&database).await.unwrap();
    let admission = migrate::SqliteAdmission::server(&database).unwrap();
    let sqlite = pool::connect_sqlite_app(&database, 4).await.unwrap();
    let backend = Backend::Sqlite(sqlite.clone());
    let mut config = room::test_collab_config(4, 30_000);
    config.rpc_timeout_ms = 500;
    let hub = Arc::new(
        fvoci_server::collab::CollabHub::new_backend(
            config,
            backend.clone(),
            Some(fvoci_server::collab::config::FamilyRoomTimings::new(6_000, 1_000).unwrap()),
        )
        .unwrap(),
    );
    let mut state = app_state_backend(backend.clone()).await;
    let storage = state.storage.clone();
    state.collab = Some(hub.clone());
    let app = document_app(state);
    let server = room::spawn_server(app.clone(), hub.clone()).await;
    let (status, setup, cookie, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/setup",
        Some(
            json!({"email":"admin@example.com","password":"supersecret1","givenName":"Admin",
            "workspaceSlug":"acme","workspaceName":"Acme"}),
        ),
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let cookie = extract_session_cookie(cookie.as_ref().unwrap());
    let workspace = Uuid::parse_str(setup["workspaceId"].as_str().unwrap()).unwrap();
    let path = format!("/api/v1/workspaces/{workspace}/documents");
    let live =
        fvoci_server::db::identity::find_live_session_backend(&backend, &hash_token(&cookie))
            .await
            .unwrap()
            .unwrap();
    for committed in [false, true] {
        let (status, created, _, _) = json_request(app.clone(), "POST", &path,
            Some(json!({"commandId":Uuid::now_v7(),"parentId":null,"title":format!("Unknown startup {committed}")})), Some(&cookie), &[]).await;
        assert_eq!(status, StatusCode::CREATED);
        let document = Uuid::parse_str(created["id"].as_str().unwrap()).unwrap();
        let key = RoomKey(
            workspace,
            document,
            fvoci_server::collab::wire::CollabKind::Document,
        );
        let (reached, proceed) =
            fvoci_server::db::collab::arm_family_room_start_reply_fault(document, committed).await;
        let pending = tokio::spawn({
            let hub = hub.clone();
            async move { hub.project_live(key, live.user_id, live.session_id).await }
        });
        tokio::time::timeout(Duration::from_secs(5), reached)
            .await
            .unwrap()
            .unwrap();
        let owned: Option<(Vec<u8>,i64)> = sqlx::query_as("SELECT owner_token,fence FROM collab_room_fences WHERE workspace_id=?1 AND document_id=?2")
            .bind(workspace.as_bytes().as_slice()).bind(document.as_bytes().as_slice()).fetch_optional(&sqlite).await.unwrap();
        assert_eq!(
            owned.is_some(),
            committed,
            "observe actual COMMIT versus actual rollback"
        );
        // Current membership can disappear while an actual committed reply is
        // lost. Cleanup remains restricted to the retained owner token and
        // cannot turn this startup into a live room under stale authority.
        sqlx::query("DELETE FROM memberships WHERE workspace_id=?1 AND user_id=?2")
            .bind(workspace.as_bytes().as_slice())
            .bind(live.user_id.as_bytes().as_slice())
            .execute(&sqlite)
            .await
            .unwrap();
        proceed.send(()).unwrap();
        assert!(pending.await.unwrap().is_err());
        let after: Option<(Vec<u8>,i64,bool)> = sqlx::query_as("SELECT owner_token,fence,expires_at<=unixepoch()*1000000+CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000 FROM collab_room_fences WHERE workspace_id=?1 AND document_id=?2")
            .bind(workspace.as_bytes().as_slice()).bind(document.as_bytes().as_slice()).fetch_optional(&sqlite).await.unwrap();
        match (owned, after) {
            (Some((owner, fence)), Some((after_owner, after_fence, expired))) => {
                assert_eq!((after_owner, after_fence), (owner, fence));
                assert!(
                    expired,
                    "cleanup must expire exactly the retained committed owner"
                );
            }
            (None, None) => (),
            other => panic!("startup reconciliation changed identity {other:?}"),
        }
        assert!(!hub.room_occupies_slot(key).await);
        assert_eq!(hub.available_room_slots(), 4);
        assert_eq!(
            hub.pending_family_start_count(),
            0,
            "known owner is retired only after confirmed reconciliation"
        );
        sqlx::query("INSERT INTO memberships(workspace_id,user_id,role) VALUES(?1,?2,'owner')")
            .bind(workspace.as_bytes().as_slice())
            .bind(live.user_id.as_bytes().as_slice())
            .execute(&sqlite)
            .await
            .unwrap();
    }
    for (index, point) in [
        NATIVE_CAPTURE_FINAL_PROOF,
        NATIVE_PROJECT_FINAL_PROOF,
        MANUAL_REVISION_BEFORE_WRITE,
    ]
    .into_iter()
    .enumerate()
    {
        let (status, created, _, _) = json_request(app.clone(), "POST", &path,
            Some(json!({"commandId":Uuid::now_v7(),"parentId":null,"title":format!("Fence {index}")})), Some(&cookie), &[]).await;
        assert_eq!(status, StatusCode::CREATED);
        let document = Uuid::parse_str(created["id"].as_str().unwrap()).unwrap();
        let key = RoomKey(
            workspace,
            document,
            fvoci_server::collab::wire::CollabKind::Document,
        );
        let routing = format!("{workspace}:document:{document}");
        let mut socket = room::connect_member(server.addr, &cookie).await;
        room::auth_and_join(&mut socket, &routing, 1800 + index as u32).await;
        room::complete_sync_handshake(&mut socket, &routing).await;
        let (reached, proceed) = arm_native_consumer_barrier(document, point).await;
        let pending = tokio::spawn({
            let hub = hub.clone();
            let app = app.clone();
            let path = path.clone();
            let cookie = cookie.clone();
            async move {
                match point {
                    NATIVE_CAPTURE_FINAL_PROOF => assert!(
                        hub.capture_if_live(key, live.user_id, live.session_id)
                            .await
                            .unwrap()
                            .is_err(),
                        "expired cached capture cannot return success"
                    ),
                    NATIVE_PROJECT_FINAL_PROOF => assert!(
                        hub.project_live(key, live.user_id, live.session_id)
                            .await
                            .is_err(),
                        "expired cached projection cannot return success"
                    ),
                    _ => {
                        let (status, body, _, _) = json_request(
                            app,
                            "POST",
                            &format!("{path}/{document}/revisions"),
                            None,
                            Some(&cookie),
                            &[],
                        )
                        .await;
                        assert_eq!(
                            status,
                            StatusCode::NOT_FOUND,
                            "manual write must reject captured old room proof: {body}"
                        );
                    }
                }
            }
        });
        tokio::time::timeout(Duration::from_secs(5), reached)
            .await
            .unwrap()
            .unwrap();
        // Actual committed replacement, with a different opaque owner and
        // higher global fence. The old actor is suspended across native work.
        let replacement = Uuid::now_v7();
        let before = selected_family_fence_snapshot(&backend, workspace, document).await;
        sqlx::query("UPDATE collab_room_fences SET owner_token=?3,fence=fence+1,expires_at=9223372036854775807 WHERE workspace_id=?1 AND document_id=?2")
            .bind(workspace.as_bytes().as_slice()).bind(document.as_bytes().as_slice()).bind(replacement.as_bytes().as_slice())
            .execute(&sqlite).await.unwrap();
        proceed.send(()).unwrap();
        pending.await.unwrap();
        room::wait_for_ws_close_code(
            &mut socket,
            1013,
            Duration::from_secs(5),
            true,
            Some("try again later"),
        )
        .await;
        drop(socket);
        let after = selected_family_fence_snapshot(&backend, workspace, document).await;
        assert_eq!(after.0, replacement.as_bytes());
        assert_eq!(after.1, before.1 + 1);
        assert_eq!(
            (after.2, after.3),
            (before.2, before.3),
            "old cached consumer cannot advance native head"
        );
        let revisions: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM revisions WHERE workspace_id=?1 AND target_id=?2",
        )
        .bind(workspace.as_bytes().as_slice())
        .bind(document.as_bytes().as_slice())
        .fetch_one(&sqlite)
        .await
        .unwrap();
        assert_eq!(
            revisions, 0,
            "stale manual/session capture cannot insert a revision"
        );
    }
    let (status, created, _, _) = json_request(
        app.clone(),
        "POST",
        &path,
        Some(json!({"commandId":Uuid::now_v7(),"parentId":null,"title":"Blocked release"})),
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let document = Uuid::parse_str(created["id"].as_str().unwrap()).unwrap();
    let routing = format!("{workspace}:document:{document}");
    let mut socket = room::connect_member(server.addr, &cookie).await;
    room::auth_and_join(&mut socket, &routing, 1803).await;
    room::complete_sync_handshake(&mut socket, &routing).await;
    let mut blocker = sqlite.begin_with("BEGIN IMMEDIATE").await.unwrap();
    let (fk,): (i64,) = sqlx::query_as("PRAGMA foreign_keys")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    assert_eq!(fk, 1);
    // The real writer reservation blocks both renewal and release. Transport
    // must close while the blocker remains held, before cleanup can finish.
    room::wait_for_ws_close_code(
        &mut socket,
        1013,
        Duration::from_secs(5),
        true,
        Some("try again later"),
    )
    .await;
    drop(socket);
    let shutdown = tokio::time::timeout(Duration::from_secs(5), hub.shutdown())
        .await
        .unwrap();
    assert!(
        !shutdown.is_clean(),
        "unconfirmed blocked cleanup cannot report a clean hub"
    );
    assert!(shutdown.actor_failures >= 1);
    blocker.rollback().await.unwrap();
    // The canceled local transaction is never accepted as a cleanup receipt;
    // actual subsequent reservation/FK checks establish usable local pool.
    let mut after = sqlite.begin_with("BEGIN IMMEDIATE").await.unwrap();
    let (fk,): (i64,) = sqlx::query_as("PRAGMA foreign_keys")
        .fetch_one(&mut *after)
        .await
        .unwrap();
    assert_eq!(fk, 1);
    after.rollback().await.unwrap();
    server.shutdown().await.unwrap();
    let root = match &storage {
        fvoci_server::attachments::ObjectStorage::Local(local) => local.root().to_path_buf(),
        _ => unreachable!(),
    };
    drop(app);
    drop(storage);
    std::fs::remove_dir_all(root).unwrap();
    drop(admission);
    std::fs::remove_dir_all(directory).unwrap();
    eprintln!("selected_family_cached_consumers capture_stale=true project_stale=true manual_same_tx_proof=true socket_cancel_before_blocked_release=true cleanup_unknown_not_clean=true");
}

struct SelectedFamilyRoomFixture {
    directory: std::path::PathBuf,
    admission: migrate::SqliteAdmission,
    sqlite: sqlx::SqlitePool,
    backend: fvoci_server::db::backend::Backend,
    hub: Arc<fvoci_server::collab::CollabHub>,
    app: axum::Router,
    server: selected_room_support::TestServer,
    storage: fvoci_server::attachments::ObjectStorage,
    live: fvoci_server::db::identity::LiveSession,
    cookie: String,
    workspace: Uuid,
}
impl SelectedFamilyRoomFixture {
    async fn new(config: fvoci_server::collab::config::CollabConfig) -> Self {
        let directory =
            std::env::temp_dir().join(format!("fvoci-family-fence-oracle-{}", Uuid::now_v7()));
        std::fs::create_dir(&directory).unwrap();
        let database = directory.join("app.db");
        migrate::run_sqlite_migrations(&database).await.unwrap();
        let admission = migrate::SqliteAdmission::server(&database).unwrap();
        let sqlite = pool::connect_sqlite_app(&database, 4).await.unwrap();
        let backend = fvoci_server::db::backend::Backend::Sqlite(sqlite.clone());
        let hub = Arc::new(
            fvoci_server::collab::CollabHub::new_backend(
                config,
                backend.clone(),
                Some(fvoci_server::collab::config::FamilyRoomTimings::new(30_000, 5_000).unwrap()),
            )
            .unwrap(),
        );
        let mut state = app_state_backend(backend.clone()).await;
        let storage = state.storage.clone();
        state.collab = Some(hub.clone());
        let app = document_app(state);
        let server = selected_room_support::spawn_server(app.clone(), hub.clone()).await;
        let (status,setup,cookie,_) = json_request(app.clone(),"POST","/api/v1/setup",Some(json!({
            "email":"admin@example.com","password":"supersecret1","givenName":"Admin","workspaceSlug":"acme","workspaceName":"Acme"
        })),None,&[]).await;
        assert_eq!(status, StatusCode::CREATED);
        let cookie = extract_session_cookie(cookie.as_ref().unwrap());
        let workspace = Uuid::parse_str(setup["workspaceId"].as_str().unwrap()).unwrap();
        let live =
            fvoci_server::db::identity::find_live_session_backend(&backend, &hash_token(&cookie))
                .await
                .unwrap()
                .unwrap();
        Self {
            directory,
            admission,
            sqlite,
            backend,
            hub,
            app,
            server,
            storage,
            live,
            cookie,
            workspace,
        }
    }
    fn path(&self) -> String {
        format!("/api/v1/workspaces/{}/documents", self.workspace)
    }
    async fn create(&self, title: &str) -> Uuid {
        let (status, created, _, _) = json_request(
            self.app.clone(),
            "POST",
            &self.path(),
            Some(json!({
                "commandId":Uuid::now_v7(),"parentId":null,"title":title
            })),
            Some(&self.cookie),
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        Uuid::parse_str(created["id"].as_str().unwrap()).unwrap()
    }
    async fn finish(self, clean: bool) {
        let status = self.hub.shutdown().await;
        assert_eq!(
            status.is_clean(),
            clean,
            "actual fixture shutdown {status:?}"
        );
        let port = self.server.addr;
        self.server.shutdown().await.unwrap();
        self.sqlite.close().await;
        let root = match &self.storage {
            fvoci_server::attachments::ObjectStorage::Local(local) => local.root().to_path_buf(),
            _ => unreachable!(),
        };
        drop(self.app);
        drop(self.storage);
        drop(self.backend);
        drop(self.sqlite);
        drop(self.admission);
        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(self.directory).unwrap();
        assert!(
            tokio::net::TcpStream::connect(port).await.is_err(),
            "owned fixture port closed"
        );
        eprintln!("selected_family_fence_fixture_cleanup port={port} clean={clean} actual_pool_closed=true");
    }
}

#[tokio::test]
async fn selected_family_startup_uncertainty_bounds_admission() {
    use fvoci_server::collab::room::RoomKey;
    let mut config = selected_room_support::test_collab_config(2, 30_000);
    config.rpc_timeout_ms = 500;
    config.memory_budget_bytes = collab_engine::limits::room_memory_reservation_bytes(2);
    let fixture = SelectedFamilyRoomFixture::new(config.clone()).await;
    let mut retained = Vec::new();
    for committed in [true, false] {
        let document = fixture.create(&format!("Deadline {committed}")).await;
        let key = RoomKey(
            fixture.workspace,
            document,
            fvoci_server::collab::wire::CollabKind::Document,
        );
        let (reached, proceed) =
            fvoci_server::db::collab::arm_family_room_start_reply_fault(document, committed).await;
        let pending = tokio::spawn({
            let hub = fixture.hub.clone();
            let actor = fixture.live.user_id;
            let credential = fixture.live.session_id;
            async move { hub.project_live(key, actor, credential).await }
        });
        tokio::time::timeout(Duration::from_secs(5), reached)
            .await
            .unwrap()
            .unwrap();
        let actual: Option<Vec<u8>> = sqlx::query_scalar(
            "SELECT owner_token FROM collab_room_fences WHERE workspace_id=?1 AND document_id=?2",
        )
        .bind(fixture.workspace.as_bytes().as_slice())
        .bind(document.as_bytes().as_slice())
        .fetch_optional(&fixture.sqlite)
        .await
        .unwrap();
        assert_eq!(
            actual.is_some(),
            committed,
            "actual committed versus rollback reply-loss boundary"
        );
        assert!(tokio::time::timeout(Duration::from_secs(5), pending)
            .await
            .unwrap()
            .unwrap()
            .is_err());
        assert!(
            proceed.send(()).is_err(),
            "deadline canceled reply receiver; never invent a received outcome"
        );
        let owner = fixture
            .hub
            .unresolved_family_owner(key)
            .expect("exact unresolved owner retained");
        if let Some(actual) = actual {
            assert_eq!(Uuid::from_slice(&actual).unwrap(), owner);
        }
        retained.push((key, owner));
        for _ in 0..12 {
            assert!(fixture
                .hub
                .project_live(key, fixture.live.user_id, fixture.live.session_id)
                .await
                .is_err());
            assert_eq!(fixture.hub.unresolved_family_owner(key), Some(owner));
        }
        assert_eq!(fixture.hub.pending_family_start_count(), retained.len());
        assert_eq!(
            fvoci_server::collab::hub::room_start_count(document).await,
            1,
            "repeated same key cannot allocate another owner"
        );
        assert!(!fixture.hub.room_occupies_slot(key).await);
        assert_eq!(fixture.hub.available_room_slots(), 2);
        assert!(
            fixture.hub.can_reserve_room_memory(),
            "failed startup released actual memory reservation"
        );
    }
    let document = fixture.create("Global unresolved capacity").await;
    let key = RoomKey(
        fixture.workspace,
        document,
        fvoci_server::collab::wire::CollabKind::Document,
    );
    for _ in 0..12 {
        assert!(fixture
            .hub
            .project_live(key, fixture.live.user_id, fixture.live.session_id)
            .await
            .is_err());
    }
    assert_eq!(fixture.hub.pending_family_start_count(), 2);
    assert_eq!(
        fvoci_server::collab::hub::room_start_count(document).await,
        0,
        "unresolved room union consumes global capacity"
    );
    for (key, owner) in retained {
        assert_eq!(fixture.hub.unresolved_family_owner(key), Some(owner));
    }
    fixture.finish(false).await;
    // A real closed SQLx pool causes failure before BEGIN/business SQL; an
    // observer connection proves no lease row, never a fabricated commit loss.
    let fixture = SelectedFamilyRoomFixture::new(config).await;
    let document = fixture.create("Before BEGIN failure").await;
    let key = RoomKey(
        fixture.workspace,
        document,
        fvoci_server::collab::wire::CollabKind::Document,
    );
    let (reached, proceed) = fvoci_server::collab::hub::arm_hub_join_barrier(
        document,
        fvoci_server::collab::hub::HUB_FAMILY_START_BEFORE_BEGIN,
    )
    .await;
    let pending = tokio::spawn({
        let hub = fixture.hub.clone();
        let actor = fixture.live.user_id;
        let credential = fixture.live.session_id;
        async move { hub.project_live(key, actor, credential).await }
    });
    tokio::time::timeout(Duration::from_secs(5), reached)
        .await
        .unwrap()
        .unwrap();
    fixture.sqlite.close().await;
    proceed.send(()).unwrap();
    assert!(pending.await.unwrap().is_err());
    let owner = fixture.hub.unresolved_family_owner(key).unwrap();
    for _ in 0..12 {
        assert!(fixture
            .hub
            .project_live(key, fixture.live.user_id, fixture.live.session_id)
            .await
            .is_err());
        assert_eq!(fixture.hub.unresolved_family_owner(key), Some(owner));
    }
    assert_eq!(fixture.hub.pending_family_start_count(), 1);
    assert_eq!(
        fvoci_server::collab::hub::room_start_count(document).await,
        1
    );
    assert_eq!(fixture.hub.available_room_slots(), 2);
    assert!(fixture.hub.can_reserve_room_memory());
    let observer = pool::connect_sqlite_app(&fixture.directory.join("app.db"), 1)
        .await
        .unwrap();
    let absent: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM collab_room_fences WHERE workspace_id=?1 AND document_id=?2",
    )
    .bind(fixture.workspace.as_bytes().as_slice())
    .bind(document.as_bytes().as_slice())
    .fetch_one(&observer)
    .await
    .unwrap();
    assert_eq!(absent, 0);
    observer.close().await;
    fixture.finish(false).await;
}

#[tokio::test]
async fn selected_family_forward_noop_rechecks_native_proof() {
    use fvoci_server::collab::room::{arm_native_consumer_barrier, NATIVE_FORWARD_FINAL_PROOF};
    let fixture =
        SelectedFamilyRoomFixture::new(selected_room_support::test_collab_config(3, 30_000)).await;
    for (index, control) in ["owner", "generation", "tail"].into_iter().enumerate() {
        let document = fixture.create(&format!("Empty forward {control}")).await;
        let routing = format!("{}:document:{document}", fixture.workspace);
        let mut socket =
            selected_room_support::connect_member(fixture.server.addr, &fixture.cookie).await;
        selected_room_support::auth_and_join(&mut socket, &routing, 2000 + index as u32).await;
        selected_room_support::complete_sync_handshake(&mut socket, &routing).await;
        let before =
            selected_family_fence_snapshot(&fixture.backend, fixture.workspace, document).await;
        let body_path = format!("{}/{document}/body", fixture.path());
        let body = json!({"contentJson":{"type":"doc","content":[]}});
        let (status, current, _, _) = json_request(
            fixture.app.clone(),
            "PUT",
            &body_path,
            Some(body.clone()),
            Some(&fixture.cookie),
            &[("origin", "http://localhost")],
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "actual current ON HTTP no-op: {current}"
        );
        assert_eq!(current["id"], document.to_string());
        let (reached, proceed) =
            arm_native_consumer_barrier(document, NATIVE_FORWARD_FINAL_PROOF).await;
        let pending = tokio::spawn({
            let app = fixture.app.clone();
            let cookie = fixture.cookie.clone();
            async move {
                json_request(
                    app,
                    "PUT",
                    &body_path,
                    Some(body),
                    Some(&cookie),
                    &[("origin", "http://localhost")],
                )
                .await
            }
        });
        tokio::time::timeout(Duration::from_secs(5), reached)
            .await
            .unwrap()
            .unwrap();
        match control {
            "owner" => {
                sqlx::query("UPDATE collab_room_fences SET owner_token=?3,fence=fence+1,expires_at=9223372036854775807 WHERE workspace_id=?1 AND document_id=?2")
                .bind(fixture.workspace.as_bytes().as_slice()).bind(document.as_bytes().as_slice()).bind(Uuid::now_v7().as_bytes().as_slice()).execute(&fixture.sqlite).await.unwrap();
            }
            "generation" => {
                sqlx::query("UPDATE document_states SET writer_generation=writer_generation+1 WHERE workspace_id=?1 AND document_id=?2")
                .bind(fixture.workspace.as_bytes().as_slice()).bind(document.as_bytes().as_slice()).execute(&fixture.sqlite).await.unwrap();
            }
            "tail" => {
                use fvoci_server::db::collab::{
                    AppendCollabInput, AppendCollabResult, FamilyRoomFence,
                };
                let fence = FamilyRoomFence {
                    workspace_id: fixture.workspace,
                    document_id: document,
                    owner_token: Uuid::from_slice(&before.0).unwrap(),
                    fence: before.1,
                };
                let committed = fvoci_server::db::collab::append_family_document_room_update(
                    &fixture.backend,
                    fence,
                    AppendCollabInput {
                        workspace_id: fixture.workspace,
                        actor_user_id: fixture.live.user_id,
                        session_id: fixture.live.session_id,
                        document_id: document,
                        writer_generation: before.2,
                        expected_tail_seq: before.3,
                        op_id: Uuid::now_v7(),
                        payload: &[0, 0],
                        client_ip: None,
                    },
                )
                .await
                .unwrap()
                .unwrap();
                assert_eq!(
                    committed,
                    AppendCollabResult::Committed { seq: before.3 + 1 }
                );
            }
            _ => unreachable!(),
        }
        proceed.send(()).unwrap();
        let (status, response, _, _) = pending.await.unwrap();
        assert_eq!(
            status,
            StatusCode::SERVICE_UNAVAILABLE,
            "actual HTTP native no-op cannot succeed with old {control}: {response}"
        );
        selected_room_support::wait_for_ws_close_code(
            &mut socket,
            1013,
            Duration::from_secs(5),
            true,
            Some("try again later"),
        )
        .await;
        drop(socket);
        let after =
            selected_family_fence_snapshot(&fixture.backend, fixture.workspace, document).await;
        assert_eq!(
            after.3,
            before.3 + i64::from(control == "tail"),
            "no-op refusal adds no native operation"
        );
        let count:i64=sqlx::query_scalar("SELECT count(*) FROM document_collab_op_receipts WHERE workspace_id=?1 AND document_id=?2")
            .bind(fixture.workspace.as_bytes().as_slice()).bind(document.as_bytes().as_slice()).fetch_one(&fixture.sqlite).await.unwrap();
        assert_eq!(count, i64::from(control == "tail"));
        assert_eq!(
            selected_compaction_effect_counts(&fixture.backend, fixture.workspace, document).await,
            (0, 0)
        );
    }
    fixture.finish(true).await;
}

/// Test oracle uses the official isolated native engine and explicitly reaps it.
async fn selected_native_history(
    engine: &std::path::Path,
    snapshot: Vec<u8>,
    tail: Vec<Vec<u8>>,
) -> (Value, Vec<u8>) {
    let bin = engine.to_path_buf();
    tokio::task::spawn_blocking(move || {
        use collab_engine::outcome::EngineStatus;
        use collab_engine::process::{ChildSlotKind, EngineSession, SpawnRequest};
        use collab_engine::protocol::Request;
        let mut child = EngineSession::spawn(SpawnRequest {
            engine_bin: bin.clone(),
            limits: collab_engine::Limits::default(),
            slot_kind: ChildSlotKind::Primary,
            slot_wait: None,
            test_hang_ms: None,
            test_exit_after_read: None,
            test_close_stdout_hang_ms: None,
            test_exit_after_write: None,
        })
        .unwrap();
        let pid = child.pid().unwrap();
        assert_eq!(
            std::fs::read_link(format!("/proc/{pid}/exe")).unwrap(),
            std::fs::canonicalize(&bin).unwrap()
        );
        assert!(child
            .call(&Request::Load {
                snapshot_b64: Some(snapshot),
                tail_b64: tail,
                encoding: 1
            })
            .outcome
            .is_applied_ok());
        let content = match child.call(&Request::Project { encoding: 1 }).outcome {
            EngineStatus::Ok {
                content_json: Some(value),
                ..
            } => value,
            other => panic!("independent native projection {other:?}"),
        };
        let history = match child.call(&Request::RevisionSnapshot).outcome {
            EngineStatus::Ok {
                update_b64: Some(bytes),
                ..
            } => collab_engine::b64::decode(&bytes).unwrap(),
            other => panic!("independent native history {other:?}"),
        };
        child.kill_and_reap();
        assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
        eprintln!("selected_room_history_oracle_child pid={pid} reaped=true");
        (content, history)
    })
    .await
    .unwrap()
}

async fn selected_room_queued_delivery_controls(
    app: &axum::Router,
    addr: std::net::SocketAddr,
    backend: &fvoci_server::db::backend::Backend,
    workspace: Uuid,
    document: Uuid,
    pg_admin_url: &str,
) {
    use futures_util::{SinkExt, StreamExt};
    use fvoci_server::collab::wire::{DocumentMessage, SyncMessage, SyncStep, WireFrame};
    use fvoci_server::db::backend::Backend;
    use selected_room_support as room;
    use tokio_tungstenite::tungstenite::Message;
    for (index, control) in ["member", "session", "fence"].into_iter().enumerate() {
        if control == "fence" && matches!(backend, Backend::Postgres(_)) {
            // PG is fenced by its dedicated advisory guard, not a family row.
            continue;
        }
        let (status, _, cookie, _) = json_request(
            app.clone(),
            "POST",
            "/api/v1/auth/login",
            Some(json!({"email":"admin@example.com","password":"supersecret1"})),
            None,
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let cookie = extract_session_cookie(cookie.as_ref().unwrap());
        let live =
            fvoci_server::db::identity::find_live_session_backend(backend, &hash_token(&cookie))
                .await
                .unwrap()
                .unwrap();
        let routing = format!("{workspace}:document:{document}");
        let mut client = room::connect_member(addr, &cookie).await;
        room::auth_and_join(&mut client, &routing, 1900 + index as u32).await;
        room::complete_sync_handshake(&mut client, &routing).await;
        let (reached, proceed) =
            fvoci_server::db::collab_delivery::arm_delivery_read_barrier(live.session_id);
        client
            .send(Message::Binary(
                room::sync_step1_frame(&routing, &[0]).into(),
            ))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), reached)
            .await
            .unwrap()
            .unwrap();
        match backend {
            Backend::Postgres(_) => {
                // Fixture operator writes are separate from the restricted app
                // role performing queued-frame delivery authorization.
                let operator = PgPoolOptions::new()
                    .max_connections(1)
                    .connect(pg_admin_url)
                    .await
                    .unwrap();
                match control {
                    "member" => {
                        sqlx::query(
                            "DELETE FROM fvoci.memberships WHERE workspace_id=$1 AND user_id=$2",
                        )
                        .bind(workspace)
                        .bind(live.user_id)
                        .execute(&operator)
                        .await
                        .unwrap();
                    }
                    "session" => {
                        sqlx::query(
                            "UPDATE fvoci.sessions SET revoked_at=clock_timestamp() WHERE id=$1",
                        )
                        .bind(live.session_id)
                        .execute(&operator)
                        .await
                        .unwrap();
                    }
                    _ => unreachable!(),
                }
                operator.close().await;
            }
            Backend::Sqlite(pool) => match control {
                "member" => {
                    sqlx::query("DELETE FROM memberships WHERE workspace_id=?1 AND user_id=?2")
                        .bind(workspace.as_bytes().as_slice())
                        .bind(live.user_id.as_bytes().as_slice())
                        .execute(pool)
                        .await
                        .unwrap();
                }
                "session" => {
                    sqlx::query("UPDATE sessions SET revoked_at=unixepoch()*1000000 WHERE id=?1")
                        .bind(live.session_id.as_bytes().as_slice())
                        .execute(pool)
                        .await
                        .unwrap();
                }
                "fence" => {
                    sqlx::query("UPDATE collab_room_fences SET owner_token=?3,fence=fence+1,expires_at=9223372036854775807 WHERE workspace_id=?1 AND document_id=?2").bind(workspace.as_bytes().as_slice()).bind(document.as_bytes().as_slice()).bind(Uuid::now_v7().as_bytes().as_slice()).execute(pool).await.unwrap();
                }
                _ => unreachable!(),
            },
            _ => unreachable!(),
        }
        proceed.send(()).unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let message = tokio::time::timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
                client.next(),
            )
            .await
            .unwrap()
            .expect("revocation close")
            .unwrap();
            match message {
                Message::Close(Some(frame)) => {
                    assert_eq!(u16::from(frame.code), 1008);
                    assert_eq!(frame.reason, "permission revoked");
                    break;
                }
                Message::Binary(bytes) => {
                    assert!(
                        !matches!(
                            fvoci_server::collab::wire::decode(&bytes),
                            Ok(WireFrame::Document {
                                message: DocumentMessage::Sync(SyncMessage {
                                    step: SyncStep::Step2 | SyncStep::Update,
                                    ..
                                }),
                                ..
                            })
                        ),
                        "queued native data must not escape after committed {control} revoke"
                    );
                }
                Message::Close(None) => panic!("revocation close must include code"),
                _ => (),
            }
        }
        drop(client);
        if control == "member" {
            match backend {
                Backend::Postgres(_) => {
                    let operator = PgPoolOptions::new()
                        .max_connections(1)
                        .connect(pg_admin_url)
                        .await
                        .unwrap();
                    sqlx::query("INSERT INTO fvoci.memberships(workspace_id,user_id,role) VALUES($1,$2,'owner')").bind(workspace).bind(live.user_id).execute(&operator).await.unwrap();
                    operator.close().await;
                }
                Backend::Sqlite(pool) => {
                    sqlx::query(
                        "INSERT INTO memberships(workspace_id,user_id,role) VALUES(?1,?2,'owner')",
                    )
                    .bind(workspace.as_bytes().as_slice())
                    .bind(live.user_id.as_bytes().as_slice())
                    .execute(pool)
                    .await
                    .unwrap();
                }
                _ => unreachable!(),
            }
        }
        eprintln!("selected_room_queued_delivery backend={} control={control} denied_before_native_data=true", backend.kind());
    }
}

async fn selected_socket_native_history(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    routing: &str,
    engine: &std::path::Path,
) -> (Value, Vec<u8>) {
    use futures_util::SinkExt;
    use fvoci_server::collab::wire::{DocumentMessage, SyncMessage, SyncStep, WireFrame};
    socket
        .send(tokio_tungstenite::tungstenite::Message::Binary(
            selected_room_support::sync_step1_frame(routing, &[0]).into(),
        ))
        .await
        .unwrap();
    for _ in 0..16 {
        if let WireFrame::Document {
            message:
                DocumentMessage::Sync(SyncMessage {
                    step: SyncStep::Step2,
                    y_protocol,
                }),
            ..
        } = selected_room_support::recv_document_frame(socket, 1)
            .await
            .expect("fresh Hub client frame")
        {
            let (step, native) =
                fvoci_server::collab::y_sync::parse_sync_payload(&y_protocol, 4 * 1024 * 1024)
                    .unwrap();
            assert_eq!(step, SyncStep::Step2);
            return selected_native_history(engine, native, vec![]).await;
        }
    }
    panic!("new Hub/client must receive canonical native Step2");
}

async fn selected_room_durable_ack(
    backend: &fvoci_server::db::backend::Backend,
    target: (Uuid, Uuid),
    credential: (Uuid, Uuid),
    engine: &std::path::Path,
    payloads: &[&[u8]],
    head: (i64, i64),
    expected_history: &[u8],
) {
    let (workspace, document) = target;
    let (actor, session) = credential;
    let (cutoff, generation) = head;
    use fvoci_server::db::backend::Backend;
    use fvoci_server::db::collab::{load_collab_readonly_kind_backend, CollabKind};
    use sha2::{Digest, Sha256};
    let native = load_collab_readonly_kind_backend(
        backend,
        CollabKind::Document,
        workspace,
        actor,
        session,
        document,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        native.snapshot_cutoff_seq, cutoff,
        "ACK requires durable native compaction"
    );
    assert_eq!(native.tail_seq, cutoff);
    assert!(
        native.tail.is_empty(),
        "ACK cannot leave uncompacted native tail"
    );
    assert_eq!(
        native.writer_generation, generation,
        "native generation follows real writer activation"
    );
    let (_, history) = selected_native_history(engine, native.snapshot, vec![]).await;
    assert_eq!(
        history, expected_history,
        "durable canonical state retains original native identity/delete set"
    );
    let receipts: Vec<(Uuid, i64, i64, Vec<u8>, Uuid)> = match backend {
        Backend::Postgres(pool) => {
            let mut tx = pool.begin().await.unwrap();
            fvoci_server::db::context::set_tenant(&mut tx, workspace)
                .await
                .unwrap();
            let rows = sqlx::query_as("SELECT op_id,seq,payload_len,payload_sha256,actor_user_id FROM fvoci.document_collab_op_receipts WHERE workspace_id=$1 AND document_id=$2 ORDER BY seq")
                .bind(workspace).bind(document).fetch_all(&mut *tx).await.unwrap();
            tx.commit().await.unwrap();
            rows
        }
        Backend::Sqlite(pool) => {
            type FamilyReceiptRow = (Vec<u8>, i64, i64, Vec<u8>, Vec<u8>);
            let rows: Vec<FamilyReceiptRow> = sqlx::query_as("SELECT op_id,seq,payload_len,payload_sha256,actor_user_id FROM document_collab_op_receipts WHERE workspace_id=?1 AND document_id=?2 ORDER BY seq")
                .bind(workspace.as_bytes().as_slice()).bind(document.as_bytes().as_slice()).fetch_all(pool).await.unwrap();
            rows.into_iter()
                .map(|(id, seq, len, hash, actor)| {
                    (
                        Uuid::from_slice(&id).unwrap(),
                        seq,
                        len,
                        hash,
                        Uuid::from_slice(&actor).unwrap(),
                    )
                })
                .collect()
        }
        _ => unreachable!(),
    };
    assert_eq!(receipts.len(), payloads.len());
    let mut ids = std::collections::HashSet::new();
    for (index, (op, seq, length, hash, receipt_actor)) in receipts.iter().enumerate() {
        assert!(
            ids.insert(*op),
            "one stable native command per logical edit"
        );
        assert_eq!(*seq, index as i64 + 1);
        assert_eq!(*length, payloads[index].len() as i64);
        assert_eq!(*hash, Sha256::digest(payloads[index]).to_vec());
        assert_eq!(*receipt_actor, actor);
    }
    assert_eq!(
        selected_compaction_effect_counts(backend, workspace, document).await,
        (cutoff, cutoff),
        "ACK requires committed event and audit effects in same durable compaction"
    );
}

async fn selected_room_transport_flow(
    app: axum::Router,
    hub: Arc<fvoci_server::collab::CollabHub>,
    backend: &fvoci_server::db::backend::Backend,
    route: (&str, &str, &str),
    cookie: &str,
    native: (&std::path::Path, &Value, &[u8]),
    databases: (&str, &str, &std::path::Path),
) {
    let (path, document, workspace) = route;
    let (engine, content, update) = native;
    let (pg_url, pg_admin_url, sqlite_path) = databases;
    use futures_util::SinkExt;
    use fvoci_server::collab::wire::{DocumentMessage, SyncMessage, SyncStep, WireFrame};
    use selected_room_support as room;
    use tokio_tungstenite::tungstenite::Message;
    let workspace_id = Uuid::parse_str(workspace).unwrap();
    let document_id = Uuid::parse_str(document).unwrap();
    let live = fvoci_server::db::identity::find_live_session_backend(backend, &hash_token(cookie))
        .await
        .unwrap()
        .unwrap();
    let expected_before = selected_native_history(engine, update.to_vec(), vec![]).await;
    assert_eq!(&expected_before.0, content);
    let deletion = room::delete_only_update();
    let expected_after =
        selected_native_history(engine, update.to_vec(), vec![deletion.clone()]).await;
    let after_content = room::delete_only_json_after();
    assert_eq!(expected_after.0, after_content);
    assert_ne!(
        expected_before.1, expected_after.1,
        "delete set changes history despite unchanged state vector"
    );
    assert_eq!(
        room::engine_fixture("sv_before_delete.bin"),
        room::engine_fixture("sv_after_delete.bin")
    );
    let server = room::spawn_server(app.clone(), hub.clone()).await;
    let addr = server.addr;
    let routing = format!("{workspace}:document:{document}");
    let mut writer = room::connect_member(addr, cookie).await;
    room::auth_and_join(&mut writer, &routing, 1701).await;
    room::complete_sync_handshake(&mut writer, &routing).await;
    writer
        .send(Message::Binary(
            room::sync_update_frame(&routing, update).into(),
        ))
        .await
        .unwrap();
    assert!(
        room::wait_for_sync_applied(&mut writer, Duration::from_secs(5)).await,
        "{} actual actor must acknowledge native commit",
        backend.kind()
    );
    let request = Uuid::now_v7();
    writer
        .send(Message::Binary(
            room::stateless_frame(&routing, &format!("persist:{request}")).into(),
        ))
        .await
        .unwrap();
    assert!(
        room::wait_for_stateless_exact(
            &mut writer,
            &format!("persisted:{request}"),
            Duration::from_secs(5)
        )
        .await,
        "{} exact persist request ACK is required",
        backend.kind()
    );
    selected_room_durable_ack(
        backend,
        (workspace_id, document_id),
        (live.user_id, live.session_id),
        engine,
        &[update],
        (1, 1),
        &expected_before.1,
    )
    .await;
    let (status, body, _, _) = json_request(
        app.clone(),
        "GET",
        &format!("{path}/{document}/body"),
        None,
        Some(cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(&body["contentJson"], content);
    let revisions = format!("{path}/{document}/revisions");
    let (status, created, _, _) =
        json_request(app.clone(), "POST", &revisions, None, Some(cookie), &[]).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{} actual live manual revision {created}",
        backend.kind()
    );
    let revision = created["id"].as_str().unwrap();
    writer
        .send(Message::Binary(
            room::sync_update_frame(&routing, &deletion).into(),
        ))
        .await
        .unwrap();
    assert!(room::wait_for_sync_applied(&mut writer, Duration::from_secs(5)).await);
    let delete_request = Uuid::now_v7();
    writer
        .send(Message::Binary(
            room::stateless_frame(&routing, &format!("persist:{delete_request}")).into(),
        ))
        .await
        .unwrap();
    assert!(
        room::wait_for_stateless_exact(
            &mut writer,
            &format!("persisted:{delete_request}"),
            Duration::from_secs(5)
        )
        .await
    );
    selected_room_durable_ack(
        backend,
        (workspace_id, document_id),
        (live.user_id, live.session_id),
        engine,
        &[update, &deletion],
        (2, 1),
        &expected_after.1,
    )
    .await;

    let (status, login, session, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/auth/login",
        Some(json!({"email":"admin@example.com","password":"supersecret1"})),
        None,
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "fresh actor login: {login}");
    let fresh_cookie = extract_session_cookie(session.as_ref().unwrap());
    assert_ne!(fresh_cookie, cookie);
    let mut fresh = room::connect_member(addr, &fresh_cookie).await;
    room::auth_and_join(&mut fresh, &routing, 1702).await;
    fresh
        .send(Message::Binary(
            room::sync_step1_frame(&routing, &[0]).into(),
        ))
        .await
        .unwrap();
    let mut native = None;
    for _ in 0..16 {
        match room::recv_document_frame(&mut fresh, 1)
            .await
            .expect("fresh socket frame")
        {
            WireFrame::Document {
                message:
                    DocumentMessage::Sync(SyncMessage {
                        step: SyncStep::Step2,
                        y_protocol,
                    }),
                ..
            } => {
                let (step, bytes) =
                    fvoci_server::collab::y_sync::parse_sync_payload(&y_protocol, 4 * 1024 * 1024)
                        .unwrap();
                assert_eq!(step, SyncStep::Step2);
                native = Some(bytes);
                break;
            }
            WireFrame::Document {
                message: DocumentMessage::SyncStatus { applied: false },
                ..
            } => panic!("fresh client native read refused"),
            _ => {}
        }
    }
    let native = native.expect("fresh client must receive native Step2 within bounded handshake");
    let bin = engine.to_path_buf();
    let (projected, fresh_history) = tokio::task::spawn_blocking(move || {
        use collab_engine::outcome::EngineStatus;
        use collab_engine::process::{ChildSlotKind, EngineSession, SpawnRequest};
        use collab_engine::protocol::Request;
        let mut child = EngineSession::spawn(SpawnRequest {
            engine_bin: bin.clone(),
            limits: collab_engine::Limits::default(),
            slot_kind: ChildSlotKind::Primary,
            slot_wait: None,
            test_hang_ms: None,
            test_exit_after_read: None,
            test_close_stdout_hang_ms: None,
            test_exit_after_write: None,
        })
        .unwrap();
        let pid = child.pid().unwrap();
        assert_eq!(
            std::fs::read_link(format!("/proc/{pid}/exe")).unwrap(),
            std::fs::canonicalize(&bin).unwrap()
        );
        assert!(child
            .call(&Request::Load {
                snapshot_b64: Some(native),
                tail_b64: vec![],
                encoding: 1
            })
            .outcome
            .is_applied_ok());
        let projected = match child.call(&Request::Project { encoding: 1 }).outcome {
            EngineStatus::Ok {
                content_json: Some(content),
                ..
            } => content,
            outcome => panic!("fresh transport native projection {outcome:?}"),
        };
        let history = match child.call(&Request::RevisionSnapshot).outcome {
            EngineStatus::Ok {
                update_b64: Some(bytes),
                ..
            } => collab_engine::b64::decode(&bytes).unwrap(),
            other => panic!("fresh transport native history {other:?}"),
        };
        child.kill_and_reap();
        assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
        eprintln!("selected_room_fresh_client_child pid={pid} reaped=true");
        (projected, history)
    })
    .await
    .unwrap();
    assert_eq!(projected, after_content);
    assert_eq!(
        fresh_history, expected_after.1,
        "fresh transport preserves native IDs and delete set"
    );
    let (status, meta, _, _) = json_request(
        app.clone(),
        "GET",
        &format!("{path}/{document}"),
        None,
        Some(&fresh_cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(meta["id"], document);
    let (status, detail, _, _) = json_request(
        app.clone(),
        "GET",
        &format!("{revisions}/{revision}"),
        None,
        Some(&fresh_cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["id"], revision);
    assert_eq!(detail["targetId"], document);
    assert_eq!(detail["targetKind"], "document");
    assert_eq!(detail["reason"], "manual");
    assert_eq!(&detail["contentJson"], content);
    assert_eq!(
        collab_engine::b64::decode(detail["ySnapshot"].as_str().unwrap()).unwrap(),
        expected_before.1,
        "manual revision retains independent pre-deletion native history"
    );
    let (status, body, _, _) = json_request(
        app.clone(),
        "GET",
        &format!("{path}/{document}/body"),
        None,
        Some(&fresh_cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["contentJson"], after_content);
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
    assert_eq!(status, StatusCode::NOT_FOUND);
    fresh.close(None).await.unwrap();
    writer.close(None).await.unwrap();
    drop(fresh);
    drop(writer);
    // Probe drains real socket lease drops before observing no remaining clients.
    let key = fvoci_server::collab::room::RoomKey(
        Uuid::parse_str(workspace).unwrap(),
        Uuid::parse_str(document).unwrap(),
        fvoci_server::collab::wire::CollabKind::Document,
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if hub.probe_actor(key).await.connections == 0 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "actual socket lease cleanup deadline"
        );
        tokio::task::yield_now().await;
    }
    // Last disconnect must produce real automatic session history; the manual
    // snapshot differs, so semantic dedupe cannot hide a missing insertion.
    let session_revision = loop {
        let (status, list, _, _) = json_request(
            app.clone(),
            "GET",
            &revisions,
            None,
            Some(&fresh_cookie),
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        if let Some(row) = list["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["reason"] == "session")
        {
            assert_eq!(row["createdBy"], Value::Null);
            break row["id"].as_str().unwrap().to_owned();
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "actual last-disconnect session revision deadline"
        );
        tokio::task::yield_now().await;
    };
    let (status, session_detail, _, _) = json_request(
        app.clone(),
        "GET",
        &format!("{revisions}/{session_revision}"),
        None,
        Some(&fresh_cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(session_detail["contentJson"], after_content);
    assert_eq!(
        collab_engine::b64::decode(session_detail["ySnapshot"].as_str().unwrap()).unwrap(),
        expected_after.1
    );
    let status = hub.shutdown().await;
    assert!(
        status.is_clean(),
        "{} actual room cleanup {status:?}",
        backend.kind()
    );
    server.shutdown().await.unwrap();
    // A fresh pool and hub must reload committed native history, not the old
    // actor cache. This remains an integration server, not normal main/Vue.
    let restarted_backend = match backend {
        fvoci_server::db::backend::Backend::Postgres(_) => {
            fvoci_server::db::backend::Backend::Postgres(pool::connect_app(pg_url).await.unwrap())
        }
        fvoci_server::db::backend::Backend::Sqlite(_) => {
            fvoci_server::db::backend::Backend::Sqlite(
                pool::connect_sqlite_app(sqlite_path, 4).await.unwrap(),
            )
        }
        _ => unreachable!(),
    };
    let restarted_hub = Arc::new(
        fvoci_server::collab::CollabHub::new_backend(
            room::test_collab_config(2, 30_000),
            restarted_backend.clone(),
            Some(fvoci_server::collab::config::FamilyRoomTimings::new(30_000, 5_000).unwrap()),
        )
        .unwrap(),
    );
    let mut state = app_state_backend(restarted_backend.clone()).await;
    let restarted_storage = state.storage.clone();
    state.collab = Some(restarted_hub.clone());
    let restarted_app = document_app(state);
    let restarted_server = room::spawn_server(restarted_app.clone(), restarted_hub.clone()).await;
    let mut restarted_client = room::connect_member(restarted_server.addr, &fresh_cookie).await;
    room::auth_and_join(&mut restarted_client, &routing, 1703).await;
    room::complete_sync_handshake(&mut restarted_client, &routing).await;
    let restarted_native =
        selected_socket_native_history(&mut restarted_client, &routing, engine).await;
    assert_eq!(restarted_native.0, after_content);
    assert_eq!(
        restarted_native.1, expected_after.1,
        "new Hub/client native wire retains original history"
    );
    // Real fresh actor has loaded canonical state; the independent official
    // helper proves unchanged history after restart and no native tail loss.
    selected_room_durable_ack(
        &restarted_backend,
        (workspace_id, document_id),
        (live.user_id, live.session_id),
        engine,
        &[update, &deletion],
        (2, 2),
        &expected_after.1,
    )
    .await;
    for (id, expected) in [
        (revision, &expected_before.1),
        (session_revision.as_str(), &expected_after.1),
    ] {
        let (status, detail, _, _) = json_request(
            restarted_app.clone(),
            "GET",
            &format!("{revisions}/{id}"),
            None,
            Some(&fresh_cookie),
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(detail["id"], id);
        assert_eq!(
            collab_engine::b64::decode(detail["ySnapshot"].as_str().unwrap()).unwrap(),
            *expected
        );
    }
    restarted_client.close(None).await.unwrap();
    drop(restarted_client);
    selected_room_queued_delivery_controls(
        &restarted_app,
        restarted_server.addr,
        &restarted_backend,
        workspace_id,
        document_id,
        pg_admin_url,
    )
    .await;
    let restarted_port = restarted_server.addr;
    assert!(restarted_hub.shutdown().await.is_clean());
    restarted_server.shutdown().await.unwrap();
    let restarted_root = match &restarted_storage {
        fvoci_server::attachments::ObjectStorage::Local(local) => local.root().to_path_buf(),
        _ => unreachable!(),
    };
    drop(restarted_app);
    drop(restarted_storage);
    std::fs::remove_dir_all(restarted_root).unwrap();
    eprintln!("selected_room_restart backend={} port={restarted_port} native_history=true session_revision={session_revision} actual_new_hub=true", backend.kind());
    eprintln!("selected_room_transport backend={} actual_socket=true persist_ack={} manual_revision={} fresh_cookie=true fresh_native_body=true room_cleanup=true port={addr}",backend.kind(),request,revision);
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
            // This fixture observer needs audit SELECT visibility; actual
            // compaction still runs under ordinary restricted app authority.
            let previous = fvoci_server::db::context::set_system(&mut tx).await.unwrap();
            assert_ne!(previous, "on", "ordinary app pool must not leak system context");
            let counts = sqlx::query_as("SELECT (SELECT count(*) FROM fvoci.events WHERE workspace_id=$1 AND target_id=$2 AND verb='document.collab_snapshot_compacted'),(SELECT count(*) FROM fvoci.audit_log WHERE workspace_id=$1 AND target_id=$2 AND verb='document.collab_snapshot_compacted')")
                .bind(workspace).bind(document).fetch_one(&mut *tx).await.unwrap();
            fvoci_server::db::context::restore_system(&mut tx, &previous).await.unwrap();
            let restored: Option<String> = sqlx::query_scalar("SELECT current_setting('app.system_ctx', true)")
                .fetch_one(&mut *tx).await.unwrap();
            assert_eq!(restored.unwrap_or_default(), previous);
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
    with_room: bool,
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
    let native_fixture = if with_native || with_room {
        let engine_bin = std::path::PathBuf::from(
            std::env::var_os("FVOCI_COLLAB_ENGINE")
                .expect("native fixture requires freshly built FVOCI_COLLAB_ENGINE"),
        );
        let content = if with_room {
            selected_room_support::delete_only_json_before()
        } else {
            json!({"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"Selected native durable text"}]}]})
        };
        let seed = fvoci_server::collab::seed::SeedEngine::new(
            engine_bin.clone(),
            collab_engine::limits::Limits::default(),
        );
        let update = if with_room {
            selected_room_support::delete_only_base_update()
        } else {
            seed.tiptap_to_yjs_update(&content).await.unwrap()
        };
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
        let mut state = app_state_backend(backend.clone()).await;
        let room_hub = if with_room {
            let config = selected_room_support::test_collab_config(2, 30_000);
            let timings =
                fvoci_server::collab::config::FamilyRoomTimings::new(30_000, 5_000).unwrap();
            let hub = Arc::new(
                fvoci_server::collab::CollabHub::new_backend(
                    config,
                    backend.clone(),
                    Some(timings),
                )
                .unwrap(),
            );
            state.collab = Some(hub.clone());
            Some(hub)
        } else {
            None
        };
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
        let scope_actor = fvoci_server::db::identity::find_live_session_backend(
            &backend,
            &hash_token(&fresh_cookie),
        )
        .await
        .unwrap()
        .unwrap();
        selected_body_scope_authorization_controls(
            &backend,
            &pg.admin_url,
            (
                Uuid::parse_str(workspace).unwrap(),
                Uuid::parse_str(document).unwrap(),
            ),
            scope_actor.user_id,
            scope_actor.session_id,
        )
        .await;
        if let Some(hub) = room_hub {
            let (engine, content, update) = native_fixture.as_ref().unwrap();
            selected_room_transport_flow(
                app.clone(),
                hub,
                &backend,
                (&path, document, workspace),
                &fresh_cookie,
                (engine, content, update),
                (&pg.app_url, &pg.admin_url, &sqlite_path),
            )
            .await;
            drop(app);
            let root = match &storage {
                fvoci_server::attachments::ObjectStorage::Local(local) => {
                    local.root().to_path_buf()
                }
                _ => unreachable!(),
            };
            drop(storage);
            backend.close().await.unwrap();
            std::fs::remove_dir_all(root).unwrap();
            continue;
        }
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
            let (fresh, native_compaction_snapshot) = tokio::task::spawn_blocking(move || {
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
                // Compaction stores a full native update; a manual revision's
                // state-vector/delete-set snapshot is separate history data.
                let full_snapshot =
                    with_compaction.then(|| match child.call(&Request::Snapshot).outcome {
                        EngineStatus::Ok {
                            update_b64: Some(bytes),
                            ..
                        } => collab_engine::b64::decode(&bytes).unwrap(),
                        other => panic!("fresh native full snapshot: {other:?}"),
                    });
                child.kill_and_reap();
                assert!(
                    !proc_path.exists(),
                    "owned fresh native child must be reaped"
                );
                eprintln!(
                    "native_fresh_child backend={backend_kind} pid={pid} reaped=true executable={}",
                    engine.display()
                );
                (projected, full_snapshot)
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
                let full_snapshot = native_compaction_snapshot.as_ref().unwrap();
                let digest = Sha256::digest(payload).to_vec();
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
                    new_snapshot: full_snapshot,
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
                assert_eq!(compacted.snapshot, *full_snapshot);
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
