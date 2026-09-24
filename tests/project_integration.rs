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
    hold_membership_user_lock, install_insert_fail_trigger, json_request, setup_session,
    wait_for_advisory_blocked_by, wait_for_blocked_query_count, wait_for_project_lock_waiters,
    wait_for_user_for_update_blocked, TestDb,
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
    let harness = TestDb::bootstrap_through(7).await;
    let admin = admin_pool(&harness).await;
    let workspace_id = Uuid::now_v7();
    let user_id = Uuid::now_v7();
    let doc_id = Uuid::now_v7();
    let path = doc_id.simple().to_string();
    sqlx::query(
        "INSERT INTO fvoci.users (id, email, given_name) VALUES ($1, $2, 'Owner')",
    )
    .bind(user_id)
    .bind(format!("owner-{user_id}@example.com"))
    .execute(&admin)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO fvoci.workspaces (id, slug, name, next_document_number, created_by)
         VALUES ($1, 'acme', 'Acme', 1, $2)",
    )
    .bind(workspace_id)
    .bind(user_id)
    .execute(&admin)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO fvoci.memberships (workspace_id, user_id, role) VALUES ($1, $2, 'owner')",
    )
    .bind(workspace_id)
    .bind(user_id)
    .execute(&admin)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO fvoci.documents (
            id, workspace_id, title, icon, path, parent_id, sort_key, project_id, number,
            status, schema_version, text, chosung, version, created_by, content_json, kind
        ) VALUES (
            $1, $2, 'Pre-upgrade doc', NULL, $3, NULL, 'V', NULL, 1,
            'draft', 2, '', '', 1, $4, '{}'::jsonb, 'wiki'
        )
        "#,
    )
    .bind(doc_id)
    .bind(workspace_id)
    .bind(path)
    .bind(user_id)
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
    let has_projects: (bool,) = sqlx::query_as(
        "SELECT EXISTS (
            SELECT 1 FROM information_schema.tables
            WHERE table_schema = 'fvoci' AND table_name = 'projects'
        )",
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert!(has_projects.0);
    let preserved: (String,) =
        sqlx::query_as("SELECT title FROM fvoci.documents WHERE id = $1 AND workspace_id = $2")
            .bind(doc_id)
            .bind(workspace_id)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(preserved.0, "Pre-upgrade doc");
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

    let (status, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}"),
        None,
        Some(&member.cookie),
    )
    .await;
    assert!(status == StatusCode::UNAUTHORIZED || status == StatusCode::NOT_FOUND);

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
    hold_membership_user_lock(&mut barrier, lead.user_id).await;
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
    let _ = wait_for_advisory_blocked_by(&admin, blocker_pid).await;
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
    hold_membership_user_lock(&mut barrier, lead.user_id).await;
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

    wait_for_advisory_blocked_by(&admin, blocker_pid).await;
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

#[tokio::test]
async fn rls_with_check_denies_cross_tenant_project_insert_for_app_role() {
    let harness = TestDb::bootstrap().await;
    let (_, cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let app_pool = app_pool(&harness).await;
    let mut conn = app_pool.acquire().await.unwrap();
    let mut tx = conn.begin().await.unwrap();
    context::set_tenant(&mut tx, workspace_id).await.unwrap();
    let foreign_workspace = Uuid::now_v7();
    let err = sqlx::query(
        r#"
        INSERT INTO fvoci.projects (
            id, workspace_id, key, name, visibility, created_by
        ) VALUES ($1, $2, 'ZZ', 'Cross tenant', 'workspace', $3)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(foreign_workspace)
    .bind(owner_id)
    .execute(&mut *tx)
    .await
    .expect_err("cross-tenant insert must fail");
    assert_eq!(
        err.as_database_error()
            .and_then(|e| e.code())
            .map(|c| c.to_string()),
        Some("42501".to_string())
    );
    tx.rollback().await.ok();
    drop(conn);
    app_pool.close().await;
    let _ = cookie;
    harness.cleanup().await;
}

#[tokio::test]
async fn concurrent_two_private_leads_workspace_remove_has_single_winner() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lead_a = add_workspace_user(&admin, workspace_id, "member", "lead-a").await;
    let lead_b = add_workspace_user(&admin, workspace_id, "member", "lead-b").await;
    let hid = create_project(app.clone(), &lead_a.cookie, workspace_id, "HID", "private").await;
    let project_id = Uuid::parse_str(hid["id"].as_str().unwrap()).unwrap();
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/members"),
        Some(json!({"userId": lead_b.user_id.to_string(), "role":"lead"})),
        Some(&lead_a.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let remove_a = tokio::spawn({
        let app = app.clone();
        let owner_cookie = owner_cookie.clone();
        let user_id = lead_a.user_id;
        async move {
            json_request(
                app,
                "DELETE",
                &format!("/api/v1/workspaces/{workspace_id}/members/{user_id}"),
                None,
                Some(&owner_cookie),
            )
            .await
        }
    });
    let remove_b = tokio::spawn({
        let app = app.clone();
        let owner_cookie = owner_cookie.clone();
        let user_id = lead_b.user_id;
        async move {
            json_request(
                app,
                "DELETE",
                &format!("/api/v1/workspaces/{workspace_id}/members/{user_id}"),
                None,
                Some(&owner_cookie),
            )
            .await
        }
    });
    let (a, b) = tokio::time::timeout(Duration::from_secs(15), async {
        tokio::join!(remove_a, remove_b)
    })
    .await
    .expect("concurrent workspace removes must finish");
    let a = a.expect("join a");
    let b = b.expect("join b");
    let outcomes = [a.0, b.0];
    assert_eq!(
        outcomes
            .iter()
            .filter(|status| **status == StatusCode::OK)
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|status| **status == StatusCode::CONFLICT)
            .count(),
        1
    );
    let lead_count: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.project_members
         WHERE workspace_id = $1 AND project_id = $2 AND role = 'lead'",
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert!(lead_count.0 >= 1);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn workspace_guest_demotion_preserves_existing_project_membership() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lead = add_workspace_user(&admin, workspace_id, "member", "lead").await;
    let joiner = add_workspace_user(&admin, workspace_id, "member", "joiner").await;
    let hid = create_project(app.clone(), &lead.cookie, workspace_id, "HID", "private").await;
    let project_id = hid["id"].as_str().unwrap();
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/members"),
        Some(json!({"userId": joiner.user_id.to_string(), "role":"viewer"})),
        Some(&lead.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, _) = json_request(
        app.clone(),
        "PATCH",
        &format!(
            "/api/v1/workspaces/{workspace_id}/members/{}",
            joiner.user_id
        ),
        Some(json!({"role":"guest"})),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}"),
        None,
        Some(&joiner.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["visibility"], "private");

    let guest_role: (String,) = sqlx::query_as(
        "SELECT role FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(workspace_id)
    .bind(joiner.user_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(guest_role.0, "guest");
    let member_rows: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.project_members
         WHERE workspace_id = $1 AND project_id = $2 AND user_id = $3",
    )
    .bind(workspace_id)
    .bind(Uuid::parse_str(project_id).unwrap())
    .bind(joiner.user_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(member_rows.0, 1);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn concurrent_patch_private_vs_workspace_remove_under_project_lock() {
    run_patch_private_vs_workspace_remove_project_lock_race(
        true,
        StatusCode::OK,
        StatusCode::CONFLICT,
    )
    .await;
}

#[tokio::test]
async fn concurrent_workspace_remove_vs_patch_private_under_project_lock() {
    run_patch_private_vs_workspace_remove_project_lock_race(
        false,
        StatusCode::CONFLICT,
        StatusCode::OK,
    )
    .await;
}

async fn run_patch_private_vs_workspace_remove_project_lock_race(
    patch_first: bool,
    expected_patch: StatusCode,
    expected_remove: StatusCode,
) {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let ws_admin = add_workspace_user(&admin, workspace_id, "admin", "wsadmin").await;
    let lead = add_workspace_user(&admin, workspace_id, "member", "lead").await;
    let lab = create_project(app.clone(), &lead.cookie, workspace_id, "LAB", "workspace").await;
    let project_id = Uuid::parse_str(lab["id"].as_str().unwrap()).unwrap();
    let project_path = format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}");
    let remove_path = format!(
        "/api/v1/workspaces/{workspace_id}/members/{}",
        lead.user_id
    );

    let mut barrier = admin.begin().await.unwrap();
    sqlx::query("SELECT id FROM fvoci.projects WHERE workspace_id = $1 AND id = $2 FOR UPDATE")
        .bind(workspace_id)
        .bind(project_id)
        .fetch_one(&mut *barrier)
        .await
        .unwrap();
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *barrier)
        .await
        .unwrap();

    let patch_app = app.clone();
    let patch_cookie = ws_admin.cookie.clone();
    let patch_path = project_path.clone();
    let remove_app = app.clone();
    let remove_cookie = owner_cookie.clone();
    let remove_member_path = remove_path.clone();

    let (patch, remove) = if patch_first {
        let patch = tokio::spawn(async move {
            json_request(
                patch_app,
                "PATCH",
                &patch_path,
                Some(json!({"visibility":"private"})),
                Some(&patch_cookie),
            )
            .await
        });
        wait_for_blocked_query_count(&admin, blocker_pid, "%fvoci.projects%", 1).await;
        let remove = tokio::spawn(async move {
            json_request(
                remove_app,
                "DELETE",
                &remove_member_path,
                None,
                Some(&remove_cookie),
            )
            .await
        });
        wait_for_project_lock_waiters(&admin, 2).await;
        (patch, remove)
    } else {
        let remove = tokio::spawn(async move {
            json_request(
                remove_app,
                "DELETE",
                &remove_member_path,
                None,
                Some(&remove_cookie),
            )
            .await
        });
        wait_for_blocked_query_count(&admin, blocker_pid, "%fvoci.projects%", 1).await;
        let patch = tokio::spawn(async move {
            json_request(
                patch_app,
                "PATCH",
                &patch_path,
                Some(json!({"visibility":"private"})),
                Some(&patch_cookie),
            )
            .await
        });
        wait_for_project_lock_waiters(&admin, 2).await;
        (patch, remove)
    };

    barrier.commit().await.unwrap();

    let (patch_status, _) = tokio::time::timeout(Duration::from_secs(10), patch)
        .await
        .expect("patch finished")
        .expect("join");
    let (remove_status, _) = tokio::time::timeout(Duration::from_secs(10), remove)
        .await
        .expect("remove finished")
        .expect("join");
    assert_eq!(patch_status, expected_patch, "patch status");
    assert_eq!(remove_status, expected_remove, "remove status");

    let visibility: (String,) = sqlx::query_as(
        "SELECT visibility FROM fvoci.projects WHERE workspace_id = $1 AND id = $2",
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    let lead_count: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.project_members
         WHERE workspace_id = $1 AND project_id = $2 AND role = 'lead'",
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    let member_count: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(workspace_id)
    .bind(lead.user_id)
    .fetch_one(&admin)
    .await
    .unwrap();

    if expected_patch == StatusCode::OK {
        assert_eq!(visibility.0, "private");
        assert!(lead_count.0 >= 1, "private project must retain at least one lead");
        assert_eq!(member_count.0, 1, "blocked remove must keep workspace membership");
    } else {
        assert_eq!(visibility.0, "workspace");
        assert_eq!(lead_count.0, 0, "removed lead must drop project lead membership");
        assert_eq!(member_count.0, 0, "successful remove must drop workspace membership");
    }

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn contract_display_id_lookup_respects_acl() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &owner_cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let (status, task) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({"title":"Lookup me"})),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let task_number = task["number"].as_i64().unwrap();
    let root_number = 1i64;

    let (status, lookup) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/lookup/LAB-{task_number}"),
        None,
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items = lookup["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["kind"], "task");
    assert_eq!(items[0]["displayId"], format!("LAB-{task_number}"));

    let (status, wiki) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"parentId": null, "title": "Wiki root"})),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(wiki["displayId"], "WIKI-1");

    let (status, lookup) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/lookup/LAB-{root_number}"),
        None,
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items = lookup["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["kind"], "document");

    let (status, lookup) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/lookup/WIKI-1"),
        None,
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(lookup["items"].as_array().unwrap().len(), 1);
    assert_eq!(lookup["items"][0]["kind"], "document");

    let (status, lookup) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/lookup/WIKI-1?projectId={project_id}"),
        None,
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(lookup["items"].as_array().unwrap().is_empty());

    let outsider = add_workspace_user(&admin, workspace_id, "guest", "outsider").await;
    sqlx::query("DELETE FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2")
        .bind(workspace_id)
        .bind(outsider.user_id)
        .execute(&admin)
        .await
        .unwrap();
    let (status, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/lookup/LAB-{task_number}"),
        None,
        Some(&outsider.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, lookup) = json_request(
        app,
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/lookup/not-a-display-id"),
        None,
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(lookup["items"].as_array().unwrap().is_empty());

    admin.close().await;
    harness.cleanup().await;
}
