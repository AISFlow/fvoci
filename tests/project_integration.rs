#![cfg(feature = "db-tests")]
#![allow(dead_code)]

#[path = "support/project_harness.rs"]
mod project_harness;

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use fvoci_server::db::{context, migrate};
use project_harness::{
    add_workspace_user, admin_pool, app_pool, count_rows, create_project, drop_insert_fail_trigger,
    install_insert_fail_trigger, json_request, setup_session, wait_for_user_for_update_blocked,
    TestDb,
};
use serde_json::json;
use sqlx::Acquire;
use uuid::Uuid;

#[tokio::test]
async fn migration_008_projects_schema_exists() {
    let harness = TestDb::bootstrap().await;
    let admin = admin_pool(&harness).await;
    let version: Option<i32> =
        sqlx::query_scalar("SELECT version FROM fvoci.schema_migrations WHERE version = 8")
            .fetch_optional(&admin)
            .await
            .unwrap();
    assert_eq!(version, Some(8));
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn migration_007_upgrades_to_008_projects() {
    let harness = TestDb::bootstrap().await;
    let admin = admin_pool(&harness).await;
    sqlx::query("DELETE FROM fvoci.schema_migrations WHERE version = 8")
        .execute(&admin)
        .await
        .unwrap();
    migrate::run_migrations(&harness.admin_url)
        .await
        .expect("upgrade to 008");
    let version: Option<i32> =
        sqlx::query_scalar("SELECT version FROM fvoci.schema_migrations WHERE version = 8")
            .fetch_optional(&admin)
            .await
            .unwrap();
    assert_eq!(version, Some(8));
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn contract_create_workspace_visible_project() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let events_before = count_rows(&admin, "events").await;

    let body = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    assert!(body["rootDocumentId"].is_string());
    assert!(body["description"].is_null());
    assert!(body["icon"].is_null());
    assert_eq!(body["status"], "active");

    let events_after = count_rows(&admin, "events").await;
    assert_eq!(events_after, events_before + 1);
    let verb: (String,) =
        sqlx::query_as("SELECT verb FROM fvoci.events ORDER BY created_at DESC LIMIT 1")
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(verb.0, "project.created");
    let audit: (String,) =
        sqlx::query_as("SELECT verb FROM fvoci.audit_log ORDER BY created_at DESC LIMIT 1")
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(audit.0, "project.created");

    let project_id = body["id"].as_str().unwrap();
    let (status, workflow) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/workflow"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(workflow["statuses"].as_array().unwrap().len(), 6);

    let (status, list) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["items"][0]["taskCount"], 0);
    assert_eq!(list["items"][0]["openTaskCount"], 0);

    let root_id = body["rootDocumentId"].as_str().unwrap();
    let (status, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{root_id}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn contract_guest_cannot_create_project() {
    let harness = TestDb::bootstrap().await;
    let (app, _, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let guest = add_workspace_user(&admin, workspace_id, "guest", "guest").await;
    let events_before = count_rows(&admin, "events").await;

    let (status, body) = json_request(
        app,
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects"),
        Some(json!({"key":"LAB","name":"Lab","visibility":"workspace"})),
        Some(&guest.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");
    assert_eq!(count_rows(&admin, "events").await, events_before);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn contract_duplicate_reserved_and_strict_create_input() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;

    let (status, body) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects"),
        Some(json!({"key":"LAB","name":"Dup","visibility":"workspace"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "conflict");

    for key in ["WIKI", "OPS-5", "lab"] {
        let (status, _) = json_request(
            app.clone(),
            "POST",
            &format!("/api/v1/workspaces/{workspace_id}/projects"),
            Some(json!({"key": key, "name":"X","visibility":"workspace"})),
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "key {key}");
    }

    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects"),
        Some(json!({"key":"AB","name":"NFKC","visibility":"workspace"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, _) = json_request(
        app,
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects"),
        Some(json!({"key":"OPS","name":"Extra","visibility":"workspace","foo":1})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    harness.cleanup().await;
}

#[tokio::test]
async fn contract_private_membership_and_admin_visibility() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, workspace_id, "member", "member").await;

    let hid = create_project(app.clone(), &member.cookie, workspace_id, "HID", "private").await;
    let project_id = hid["id"].as_str().unwrap();

    let (status, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}"),
        None,
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, list) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects"),
        None,
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(list["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|p| p["key"] != "HID"));

    let (status, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/workflow"),
        None,
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/members"),
        Some(json!({"userId": owner_id.to_string(), "role":"viewer"})),
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, body) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}"),
        None,
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["visibility"], "private");

    let (status, _) = json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}"),
        Some(json!({"name":"Nope"})),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/members/{owner_id}"),
        Some(json!({"role":"lead"})),
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = json_request(
        app.clone(),
        "PATCH",
        &format!(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/members/{}",
            member.user_id
        ),
        Some(json!({"role":"member"})),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = json_request(
        app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/members/{owner_id}"),
        None,
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn contract_workspace_visible_owner_manage_without_membership() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, workspace_id, "member", "member").await;
    let guest = add_workspace_user(&admin, workspace_id, "guest", "guest").await;

    let lab = create_project(
        app.clone(),
        &member.cookie,
        workspace_id,
        "LAB",
        "workspace",
    )
    .await;
    let project_id = lab["id"].as_str().unwrap();

    let (status, body) = json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}"),
        Some(json!({"name":"Renamed"})),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["name"], "Renamed");

    let (status, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}"),
        None,
        Some(&guest.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn contract_patch_nulls_and_omitted_fields() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let (status, created) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects"),
        Some(json!({"key":"LAB","name":"Lab","visibility":"workspace","description":"keep","icon":"📁"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let project_id = created["id"].as_str().unwrap();

    let (status, patched) = json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}"),
        Some(json!({"description": null, "icon": null})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(patched["description"].is_null());
    assert!(patched["icon"].is_null());

    let (status, kept) = json_request(
        app,
        "PATCH",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}"),
        Some(json!({"name":"Still Lab"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(kept["description"].is_null());
    assert!(kept["icon"].is_null());

    harness.cleanup().await;
}

#[tokio::test]
async fn contract_wiki_affiliation_rejects_project_root_parent() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let project = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let root_id = project["rootDocumentId"].as_str().unwrap();

    let (status, body) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"parentId": root_id, "title":"Mismatch"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "document_affiliation_mismatch");

    harness.cleanup().await;
}

#[tokio::test]
async fn contract_revoked_session_and_removed_member() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, workspace_id, "member", "member").await;
    let hid = create_project(app.clone(), &member.cookie, workspace_id, "HID", "private").await;
    let project_id = hid["id"].as_str().unwrap();

    sqlx::query("UPDATE fvoci.sessions SET revoked_at = now() WHERE user_id = $1")
        .bind(member.user_id)
        .execute(&admin)
        .await
        .unwrap();
    let (status, _) = json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}"),
        Some(json!({"name":"X"})),
        Some(&member.cookie),
    )
    .await;
    assert!(status == StatusCode::UNAUTHORIZED || status == StatusCode::NOT_FOUND);

    sqlx::query("DELETE FROM fvoci.project_members WHERE workspace_id = $1 AND user_id = $2")
        .bind(workspace_id)
        .bind(member.user_id)
        .execute(&admin)
        .await
        .unwrap();
    let (status, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}"),
        None,
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = json_request(
        app,
        "DELETE",
        &format!(
            "/api/v1/workspaces/{workspace_id}/members/{}",
            member.user_id
        ),
        None,
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn project_create_event_failure_rolls_back_all_state() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    install_insert_fail_trigger(&admin, "events", "test_project_event_fail").await;
    let projects_before = count_rows(&admin, "projects").await;
    let docs_before = count_rows(&admin, "documents").await;
    let statuses_before = count_rows(&admin, "statuses").await;

    let (status, _) = json_request(
        app,
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects"),
        Some(json!({"key":"LAB","name":"Lab","visibility":"workspace"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(count_rows(&admin, "projects").await, projects_before);
    assert_eq!(count_rows(&admin, "documents").await, docs_before);
    assert_eq!(count_rows(&admin, "statuses").await, statuses_before);

    drop_insert_fail_trigger(&admin, "events", "test_project_event_fail").await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn workspace_remove_last_private_lead_blocked() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lead = add_workspace_user(&admin, workspace_id, "member", "lead").await;
    create_project(app.clone(), &lead.cookie, workspace_id, "HID", "private").await;

    let (status, _) = json_request(
        app,
        "DELETE",
        &format!("/api/v1/workspaces/{workspace_id}/members/{}", lead.user_id),
        None,
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    let member_count: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(workspace_id)
    .bind(lead.user_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(member_count.0, 1);
    let _ = owner_id;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn patch_visibility_private_with_zero_leads_rejected() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, workspace_id, "member", "member").await;
    let lab = create_project(
        app.clone(),
        &member.cookie,
        workspace_id,
        "LAB",
        "workspace",
    )
    .await;
    let project_id = lab["id"].as_str().unwrap();

    sqlx::query("DELETE FROM fvoci.project_members WHERE workspace_id = $1 AND project_id = $2")
        .bind(workspace_id)
        .bind(Uuid::parse_str(project_id).unwrap())
        .execute(&admin)
        .await
        .unwrap();

    let (status, _) = json_request(
        app,
        "PATCH",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}"),
        Some(json!({"visibility":"private"})),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let _ = owner_id;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn rls_denies_cross_tenant_project_access_for_app_role() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    create_project(app, &cookie, workspace_id, "LAB", "workspace").await;
    let app_pool = app_pool(&harness).await;
    let mut conn = app_pool.acquire().await.unwrap();
    let mut tx = conn.begin().await.unwrap();
    context::set_tenant(&mut tx, workspace_id).await.unwrap();
    let visible: (i64,) =
        sqlx::query_as("SELECT count(*) FROM fvoci.projects WHERE workspace_id = $1")
            .bind(workspace_id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(visible.0, 1);
    let other = Uuid::now_v7();
    context::set_tenant(&mut tx, other).await.unwrap();
    let hidden: (i64,) = sqlx::query_as("SELECT count(*) FROM fvoci.projects")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(hidden.0, 0);
    tx.rollback().await.ok();
    drop(conn);
    app_pool.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn wiki_tree_and_attachment_deny_project_root() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let project = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let root_id = project["rootDocumentId"].as_str().unwrap();

    let (status, tree) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/tree"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(tree["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|n| n.get("projectId").map(|v| v.is_null()).unwrap_or(true)));

    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{root_id}/uploads"),
        Some(json!({"name":"x.bin","sizeBytes":1})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let admin = admin_pool(&harness).await;
    let private = create_project(app.clone(), &cookie, workspace_id, "HID", "private").await;
    let private_root = private["rootDocumentId"].as_str().unwrap();
    let (status, tree) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/tree"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!tree["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|n| n["id"].as_str() == Some(private_root)));
    let _ = owner_id;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn session_revoke_barrier_blocks_project_patch() {
    let harness = TestDb::bootstrap().await;
    let (app, _, _owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, workspace_id, "member", "member").await;
    let lab = create_project(
        app.clone(),
        &member.cookie,
        workspace_id,
        "LAB",
        "workspace",
    )
    .await;
    let project_id = lab["id"].as_str().unwrap();
    let events_before = count_rows(&admin, "events").await;

    let mut barrier = admin.begin().await.unwrap();
    sqlx::query("SELECT id FROM fvoci.users WHERE id = $1 FOR UPDATE")
        .bind(member.user_id)
        .fetch_one(&mut *barrier)
        .await
        .unwrap();
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *barrier)
        .await
        .unwrap();

    let app_bg = app.clone();
    let cookie = member.cookie.clone();
    let patch_path = format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}");
    let patch_task = tokio::spawn(async move {
        json_request(
            app_bg,
            "PATCH",
            &patch_path,
            Some(json!({"name":"Blocked"})),
            Some(&cookie),
        )
        .await
    });

    wait_for_user_for_update_blocked(&admin, blocker_pid).await;
    sqlx::query("UPDATE fvoci.sessions SET revoked_at = now() WHERE user_id = $1")
        .bind(member.user_id)
        .execute(&mut *barrier)
        .await
        .unwrap();
    barrier.commit().await.unwrap();

    let (status, _) = tokio::time::timeout(Duration::from_secs(10), patch_task)
        .await
        .expect("patch finished")
        .expect("join");
    assert!(status == StatusCode::UNAUTHORIZED || status == StatusCode::NOT_FOUND);
    assert_eq!(count_rows(&admin, "events").await, events_before);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn app_pool_does_not_reuse_tenant_context_after_rollback() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    create_project(app, &cookie, workspace_id, "LAB", "workspace").await;
    let admin = admin_pool(&harness).await;
    let admin_count: (i64,) =
        sqlx::query_as("SELECT count(*) FROM fvoci.projects WHERE workspace_id = $1")
            .bind(workspace_id)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(admin_count.0, 1);

    let pool = app_pool(&harness).await;
    let mut conn = pool.acquire().await.unwrap();
    let mut tx = conn.begin().await.unwrap();
    context::set_tenant(&mut tx, workspace_id).await.unwrap();
    let visible: (i64,) =
        sqlx::query_as("SELECT count(*) FROM fvoci.projects WHERE workspace_id = $1")
            .bind(workspace_id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(visible.0, 1);
    tx.rollback().await.unwrap();
    drop(conn);

    let mut conn = pool.acquire().await.unwrap();
    let mut tx = conn.begin().await.unwrap();
    let hidden: (i64,) = sqlx::query_as("SELECT count(*) FROM fvoci.projects")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(hidden.0, 0);
    tx.rollback().await.ok();
    drop(conn);
    pool.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn concurrent_same_key_project_create_has_single_winner() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let projects_before = count_rows(&admin, "projects").await;
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let body = json!({"key":"RACE","name":"Race","visibility":"workspace"});

    let first = {
        let app = app.clone();
        let cookie = cookie.clone();
        let barrier = barrier.clone();
        let body = body.clone();
        tokio::spawn(async move {
            barrier.wait().await;
            json_request(
                app,
                "POST",
                &format!("/api/v1/workspaces/{workspace_id}/projects"),
                Some(body),
                Some(&cookie),
            )
            .await
        })
    };
    let second = tokio::spawn(async move {
        barrier.wait().await;
        json_request(
            app,
            "POST",
            &format!("/api/v1/workspaces/{workspace_id}/projects"),
            Some(body),
            Some(&cookie),
        )
        .await
    });

    let (status_a, _) = first.await.expect("first join");
    let (status_b, _) = second.await.expect("second join");
    let statuses = [status_a, status_b];
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == StatusCode::CREATED)
            .count(),
        1
    );
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == StatusCode::CONFLICT)
            .count(),
        1
    );
    assert_eq!(count_rows(&admin, "projects").await, projects_before + 1);
    let key_count: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.projects WHERE workspace_id = $1 AND key = 'RACE'",
    )
    .bind(workspace_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(key_count.0, 1);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn project_create_audit_failure_rolls_back_all_state() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    install_insert_fail_trigger(&admin, "audit_log", "test_project_audit_fail").await;
    let projects_before = count_rows(&admin, "projects").await;
    let docs_before = count_rows(&admin, "documents").await;
    let events_before = count_rows(&admin, "events").await;

    let (status, _) = json_request(
        app,
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects"),
        Some(json!({"key":"AUD","name":"Audit fail","visibility":"workspace"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(count_rows(&admin, "projects").await, projects_before);
    assert_eq!(count_rows(&admin, "documents").await, docs_before);
    assert_eq!(count_rows(&admin, "events").await, events_before);

    drop_insert_fail_trigger(&admin, "audit_log", "test_project_audit_fail").await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn concurrent_workspace_remove_vs_private_lead_under_user_lock() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lead = add_workspace_user(&admin, workspace_id, "member", "lead").await;
    create_project(app.clone(), &lead.cookie, workspace_id, "HID", "private").await;

    let mut barrier = admin.begin().await.unwrap();
    sqlx::query("SELECT id FROM fvoci.users WHERE id = $1 FOR UPDATE")
        .bind(lead.user_id)
        .fetch_one(&mut *barrier)
        .await
        .unwrap();
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *barrier)
        .await
        .unwrap();

    let remove = tokio::spawn({
        let app = app.clone();
        let owner_cookie = owner_cookie.clone();
        let lead_id = lead.user_id;
        async move {
            json_request(
                app,
                "DELETE",
                &format!("/api/v1/workspaces/{workspace_id}/members/{lead_id}"),
                None,
                Some(&owner_cookie),
            )
            .await
        }
    });
    let _ = wait_for_user_for_update_blocked(&admin, blocker_pid).await;
    barrier.commit().await.unwrap();

    let (status, _) = tokio::time::timeout(Duration::from_secs(10), remove)
        .await
        .expect("remove finished")
        .expect("join");
    assert_eq!(status, StatusCode::CONFLICT);
    let member_count: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(workspace_id)
    .bind(lead.user_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(member_count.0, 1);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn concurrent_member_add_vs_workspace_remove_under_user_lock() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lead = add_workspace_user(&admin, workspace_id, "member", "lead").await;
    let hid = create_project(app.clone(), &lead.cookie, workspace_id, "HID", "private").await;
    let project_id = hid["id"].as_str().unwrap();

    let mut barrier = admin.begin().await.unwrap();
    sqlx::query("SELECT id FROM fvoci.users WHERE id = $1 FOR UPDATE")
        .bind(lead.user_id)
        .fetch_one(&mut *barrier)
        .await
        .unwrap();
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *barrier)
        .await
        .unwrap();

    let remove = tokio::spawn({
        let app = app.clone();
        let owner_cookie = owner_cookie.clone();
        let lead_id = lead.user_id;
        async move {
            json_request(
                app,
                "DELETE",
                &format!("/api/v1/workspaces/{workspace_id}/members/{lead_id}"),
                None,
                Some(&owner_cookie),
            )
            .await
        }
    });
    let add = tokio::spawn({
        let app = app.clone();
        let lead_cookie = lead.cookie.clone();
        let owner_id = owner_id.to_string();
        let project_id = project_id.to_string();
        async move {
            json_request(
                app,
                "POST",
                &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/members"),
                Some(json!({"userId": owner_id, "role":"viewer"})),
                Some(&lead_cookie),
            )
            .await
        }
    });

    wait_for_user_for_update_blocked(&admin, blocker_pid).await;
    barrier.commit().await.unwrap();

    let (remove_status, _) = tokio::time::timeout(Duration::from_secs(10), remove)
        .await
        .expect("remove finished")
        .expect("join");
    let (add_status, _) = tokio::time::timeout(Duration::from_secs(10), add)
        .await
        .expect("add finished")
        .expect("join");
    assert_eq!(remove_status, StatusCode::CONFLICT);
    assert_eq!(add_status, StatusCode::CREATED);
    let member_count: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(workspace_id)
    .bind(lead.user_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(member_count.0, 1);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn patch_lead_user_id_rejects_workspace_guest() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let guest = add_workspace_user(&admin, workspace_id, "guest", "guest").await;
    let member = add_workspace_user(&admin, workspace_id, "member", "member").await;
    let lab = create_project(
        app.clone(),
        &member.cookie,
        workspace_id,
        "LAB",
        "workspace",
    )
    .await;
    let project_id = lab["id"].as_str().unwrap();

    json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/members"),
        Some(json!({"userId": guest.user_id.to_string(), "role":"viewer"})),
        Some(&member.cookie),
    )
    .await;

    let (status, body) = json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}"),
        Some(json!({"leadUserId": guest.user_id.to_string()})),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "conflict");

    let (status, body) = json_request(
        app,
        "PATCH",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}"),
        Some(json!({"leadUserId": owner_id.to_string()})),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "conflict");

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn deleted_workspace_returns_not_found_on_nested_project_routes() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let (status, task) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({"title":"Live"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let task_id = task["id"].as_str().unwrap();

    sqlx::query("UPDATE fvoci.workspaces SET deleted_at = now() WHERE id = $1")
        .bind(workspace_id)
        .execute(&admin)
        .await
        .unwrap();

    for (method, path, body) in [
        (
            "GET",
            format!("/api/v1/workspaces/{workspace_id}/projects"),
            None,
        ),
        (
            "GET",
            format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}"),
            None,
        ),
        (
            "GET",
            format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/workflow"),
            None,
        ),
        (
            "GET",
            format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
            None,
        ),
        (
            "GET",
            format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}"),
            None,
        ),
        (
            "POST",
            format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
            Some(json!({"title":"After delete"})),
        ),
    ] {
        let (status, body_json) =
            json_request(app.clone(), method, &path, body.clone(), Some(&cookie)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {path}");
        assert_eq!(body_json["code"], "not_found", "{method} {path}");
    }

    admin.close().await;
    harness.cleanup().await;
}
