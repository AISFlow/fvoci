#![cfg(feature = "db-tests")]
#![allow(dead_code)]

#[path = "support/project_harness.rs"]
mod project_harness;

/// Renumbering a wiki subtree must move each number into the project namespace
/// atomically, even when another wiki row already uses the allocated number.
#[tokio::test]
async fn wiki_subtree_move_renumbers_affiliation_atomically() {
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    let harness = project_harness::TestDb::bootstrap().await;
    let (app, cookie, owner, ws) = project_harness::setup_session(&harness).await;
    let admin = project_harness::admin_pool(&harness).await;
    let pool = project_harness::app_pool(&harness).await;
    let (superuser, bypass): (bool, bool) =
        sqlx::query_as("SELECT rolsuper, rolbypassrls FROM pg_roles WHERE rolname = current_user")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!superuser && !bypass);
    let project =
        project_harness::create_project(app.clone(), &cookie, ws, "MOVE", "private").await;
    let mut docs = Vec::new();
    for parent in [None, Some(0), Some(1), None] {
        let (status, body) = project_harness::json_request(
            app.clone(),
            "POST",
            &format!("/api/v1/workspaces/{ws}/documents"),
            Some(serde_json::json!({"commandId": uuid::Uuid::now_v7(), "title": "Wiki move", "parentId": parent.map(|i| &docs[i])})),
            Some(&cookie),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::CREATED, "{body}");
        docs.push(body["id"].as_str().unwrap().to_string());
    }
    let project_id: Uuid = project["id"].as_str().unwrap().parse().unwrap();
    let root: Uuid = project["rootDocumentId"].as_str().unwrap().parse().unwrap();
    let move_path = format!("/api/v1/workspaces/{ws}/documents/{}/move", docs[0]);
    let destination = serde_json::json!({"newParentId": root});
    // Denied destination access must not consume numbers or partially move rows.
    let denied = project_harness::add_workspace_user(&admin, ws, "member", "move-denied").await;
    let before: i32 = sqlx::query_scalar("SELECT next_number FROM fvoci.projects WHERE id = $1")
        .bind(project_id)
        .fetch_one(&admin)
        .await
        .unwrap();
    let (status, _) = project_harness::json_request(
        app.clone(),
        "POST",
        &move_path,
        Some(destination.clone()),
        Some(&denied.cookie),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::NOT_FOUND);
    let after: i32 = sqlx::query_scalar("SELECT next_number FROM fvoci.projects WHERE id = $1")
        .bind(project_id)
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(before, after);
    // Failure after renumber/path writes must roll back the entire affiliation
    // and allocator transition, not strand a partially moved subtree.
    project_harness::install_insert_fail_trigger(&admin, "events", "move_event_failure").await;
    let (status, _) = project_harness::json_request(
        app.clone(),
        "POST",
        &move_path,
        Some(destination.clone()),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::INTERNAL_SERVER_ERROR);
    project_harness::drop_insert_fail_trigger(&admin, "events", "move_event_failure").await;
    let original: Vec<(Option<Uuid>, i32)> = sqlx::query_as(
        "SELECT project_id, number FROM fvoci.documents WHERE id=ANY($1) ORDER BY number",
    )
    .bind(
        docs[..3]
            .iter()
            .map(|id| id.parse::<Uuid>().unwrap())
            .collect::<Vec<_>>(),
    )
    .fetch_all(&admin)
    .await
    .unwrap();
    assert_eq!(original, vec![(None, 1), (None, 2), (None, 3)]);
    let after_failure: i32 =
        sqlx::query_scalar("SELECT next_number FROM fvoci.projects WHERE id=$1")
            .bind(project_id)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(before, after_failure);
    let (status, body) = project_harness::json_request(
        app.clone(),
        "POST",
        &move_path,
        Some(destination),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "{body}");
    assert_eq!(body["projectId"], project_id.to_string());
    type MovedDocumentRow = (Uuid, Option<Uuid>, Option<Uuid>, i32, String);
    let moved: Vec<MovedDocumentRow> = sqlx::query_as(
        "SELECT id, project_id, parent_id, number, path FROM fvoci.documents WHERE workspace_id=$1 AND id=ANY($2) ORDER BY number"
    ).bind(ws).bind(docs[..3].iter().map(|id| id.parse::<Uuid>().unwrap()).collect::<Vec<_>>())
        .fetch_all(&admin).await.unwrap();
    assert_eq!(moved.len(), 3);
    assert_eq!(moved.iter().map(|r| r.3).collect::<Vec<_>>(), vec![2, 3, 4]);
    assert!(moved.iter().all(|r| r.1 == Some(project_id)));
    let by_id = |id: &str| moved.iter().find(|r| r.0.to_string() == id).unwrap();
    let moved_root = by_id(&docs[0]);
    let moved_child = by_id(&docs[1]);
    let moved_grandchild = by_id(&docs[2]);
    assert_eq!(moved_root.2, Some(root));
    assert_eq!(moved_child.2, Some(moved_root.0));
    assert_eq!(moved_grandchild.2, Some(moved_child.0));
    assert!(moved_child.4.starts_with(&(moved_root.4.clone() + ".")));
    assert!(moved_grandchild
        .4
        .starts_with(&(moved_child.4.clone() + ".")));
    let unaffected: (Option<Uuid>, i32) =
        sqlx::query_as("SELECT project_id, number FROM fvoci.documents WHERE id=$1")
            .bind(docs[3].parse::<Uuid>().unwrap())
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(unaffected, (None, 4));
    let next: i32 = sqlx::query_scalar("SELECT next_number FROM fvoci.projects WHERE id=$1")
        .bind(project_id)
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(next, 5);
    let events: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.events WHERE workspace_id=$1 AND verb='document.moved' AND payload->>'documentId'=$2")
        .bind(ws).bind(&docs[0]).fetch_one(&admin).await.unwrap();
    assert_eq!(events, 1);
    assert_ne!(owner, denied.user_id);
    pool.close().await;
    admin.close().await;
    harness.cleanup().await;
}

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use fvoci_server::db::collab::{resolve_collab_admission, CollabDbError};
use fvoci_server::db::context;
use project_harness::{
    add_workspace_user, admin_pool, app_pool, count_rows, create_project, drop_insert_fail_trigger,
    hold_membership_user_lock, http_request, insert_stored_attachment, install_insert_fail_trigger,
    json_request, json_request_with_headers, session_id_for_user, setup_session,
    wait_for_advisory_blocked_by, wait_for_blocked_by_holder, wait_for_blocked_query_count,
    wait_for_user_for_update_blocked, TestDb,
};
use serde_json::{json, Value};
use sqlx::Acquire;
use uuid::Uuid;

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
        Some(json!({"commandId": uuid::Uuid::now_v7(), "parentId": root_id, "title":"Mismatch"})),
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
    let remove_path = format!("/api/v1/workspaces/{workspace_id}/members/{}", lead.user_id);

    let patch_app = app.clone();
    let patch_cookie = ws_admin.cookie.clone();
    let patch_path = project_path.clone();
    let remove_app = app.clone();
    let remove_cookie = owner_cookie.clone();
    let remove_member_path = remove_path.clone();

    let (patch, remove, barrier) = if patch_first {
        let mut barrier = admin.begin().await.unwrap();
        hold_membership_user_lock(&mut barrier, lead.user_id).await;
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
        wait_for_advisory_blocked_by(&admin, blocker_pid).await;
        (patch, remove, barrier)
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
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            let remaining: (i64,) = sqlx::query_as(
                "SELECT count(*) FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2",
            )
            .bind(workspace_id)
            .bind(lead.user_id)
            .fetch_one(&admin)
            .await
            .unwrap();
            if remaining.0 == 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
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
        (patch, remove, barrier)
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

    let visibility: (String,) =
        sqlx::query_as("SELECT visibility FROM fvoci.projects WHERE workspace_id = $1 AND id = $2")
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
        assert!(
            lead_count.0 >= 1,
            "private project must retain at least one lead"
        );
        assert_eq!(
            member_count.0, 1,
            "blocked remove must keep workspace membership"
        );
    } else {
        assert_eq!(visibility.0, "workspace");
        assert_eq!(
            lead_count.0, 0,
            "removed lead must drop project lead membership"
        );
        assert_eq!(
            member_count.0, 0,
            "successful remove must drop workspace membership"
        );
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
        Some(json!({"commandId": uuid::Uuid::now_v7(), "parentId": null, "title": "Wiki root"})),
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

    // A project filter keeps a project-prefix hit only in that project.
    let (status, lookup) = json_request(
        app.clone(),
        "GET",
        &format!(
            "/api/v1/workspaces/{workspace_id}/lookup/LAB-{task_number}?projectId={project_id}"
        ),
        None,
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items = lookup["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{lookup}");
    assert_eq!(items[0]["kind"], "task");
    let ops = create_project(app.clone(), &owner_cookie, workspace_id, "OPS", "workspace").await;
    let other_project_id = ops["id"].as_str().unwrap();
    let (status, lookup) = json_request(
        app.clone(),
        "GET",
        &format!(
            "/api/v1/workspaces/{workspace_id}/lookup/LAB-{task_number}?projectId={other_project_id}"
        ),
        None,
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(lookup["items"].as_array().unwrap().is_empty(), "{lookup}");

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

#[tokio::test]
async fn lookup_revoked_session_returns_authentication_required() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &owner_cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let (status, task) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({"title":"Lookup target"})),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let task_number = task["number"].as_i64().unwrap();
    let owner_id: (Uuid,) = sqlx::query_as("SELECT id FROM fvoci.users LIMIT 1")
        .fetch_one(&admin)
        .await
        .unwrap();
    sqlx::query("UPDATE fvoci.sessions SET revoked_at = now() WHERE user_id = $1")
        .bind(owner_id.0)
        .execute(&admin)
        .await
        .unwrap();
    let (status, body) = json_request(
        app,
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/lookup/LAB-{task_number}"),
        None,
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], "authentication_required");
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn lookup_suspended_user_returns_authentication_required() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let (status, task) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({"title":"Suspended lookup"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let task_number = task["number"].as_i64().unwrap();
    sqlx::query("UPDATE fvoci.users SET suspended_at = now() WHERE id = $1")
        .bind(owner_id)
        .execute(&admin)
        .await
        .unwrap();
    let (status, body) = json_request(
        app,
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/lookup/LAB-{task_number}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], "authentication_required");
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn lookup_deactivated_user_returns_authentication_required() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let (status, task) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({"title":"Deactivated lookup"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let task_number = task["number"].as_i64().unwrap();
    sqlx::query("UPDATE fvoci.users SET deleted_at = now() WHERE id = $1")
        .bind(owner_id)
        .execute(&admin)
        .await
        .unwrap();
    let (status, body) = json_request(
        app,
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/lookup/LAB-{task_number}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], "authentication_required");
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn lookup_removed_member_and_private_project_share_empty_items() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, workspace_id, "member", "member").await;
    let hid = create_project(app.clone(), &member.cookie, workspace_id, "HID", "private").await;
    let project_id = hid["id"].as_str().unwrap();
    let (status, task) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({"title":"Hidden task"})),
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let task_number = task["number"].as_i64().unwrap();
    let root_number = 1i64;

    let empty_items = json!({"items": []});

    let (status, invisible_task_body) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/lookup/HID-{task_number}"),
        None,
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(invisible_task_body, empty_items);

    let (status, nonexistent_body) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/lookup/HID-99999"),
        None,
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(nonexistent_body, invisible_task_body);

    let outsider = add_workspace_user(&admin, workspace_id, "guest", "outsider").await;
    sqlx::query("DELETE FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2")
        .bind(workspace_id)
        .bind(outsider.user_id)
        .execute(&admin)
        .await
        .unwrap();
    let (status, member_body) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/lookup/HID-{task_number}"),
        None,
        Some(&outsider.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(member_body["code"], "not_found");

    let (status, root_body) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/lookup/HID-{root_number}"),
        None,
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(root_body, invisible_task_body);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn lookup_rate_limit_returns_retry_after() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let (status, task) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({"title":"Rate limit probe"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let task_number = task["number"].as_i64().unwrap();
    let path = format!("/api/v1/workspaces/{workspace_id}/lookup/LAB-{task_number}");
    for _ in 0..60 {
        let (status, _) = json_request(app.clone(), "GET", &path, None, Some(&cookie)).await;
        assert_eq!(status, StatusCode::OK);
    }
    let (status, body, headers) =
        json_request_with_headers(app, "GET", &path, None, Some(&cookie)).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body["code"], "rate_limit_exceeded");
    assert!(body["params"]["retryAfter"].is_number());
    let retry_after = headers.get("retry-after").unwrap().to_str().unwrap();
    assert_eq!(
        body["params"]["retryAfter"].as_u64().unwrap().to_string(),
        retry_after
    );
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn private_project_cross_path_authorization_links() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let pool = app_pool(&harness).await;

    let lead = add_workspace_user(&admin, workspace_id, "member", "lead").await;
    let hid = create_project(app.clone(), &lead.cookie, workspace_id, "HID", "private").await;
    let project_id = hid["id"].as_str().unwrap();
    let root_id = hid["rootDocumentId"].as_str().unwrap();
    let root_uuid = Uuid::parse_str(root_id).unwrap();

    let (status, task) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({"title":"Private task"})),
        Some(&lead.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let task_id = task["id"].as_str().unwrap();
    let task_number = task["number"].as_i64().unwrap();

    let lab_member = add_workspace_user(&admin, workspace_id, "member", "labonly").await;
    create_project(
        app.clone(),
        &lab_member.cookie,
        workspace_id,
        "LAB",
        "workspace",
    )
    .await;
    let guest = add_workspace_user(&admin, workspace_id, "guest", "guest").await;

    let other_workspace = Uuid::now_v7();
    sqlx::query("INSERT INTO fvoci.workspaces (id, slug, name) VALUES ($1, 'other', 'Other')")
        .bind(other_workspace)
        .execute(&admin)
        .await
        .unwrap();
    let other_ws_user = add_workspace_user(&admin, other_workspace, "member", "foreign").await;

    let attachment_id =
        insert_stored_attachment(&admin, workspace_id, root_uuid, lead.user_id).await;
    let lead_session = session_id_for_user(&admin, lead.user_id).await;
    let empty_lookup = json!({"items": []});

    struct PrivateProjectFixture<'a> {
        workspace_id: Uuid,
        project_id: &'a str,
        root_id: &'a str,
        task_id: &'a str,
        task_number: i64,
        attachment_id: Uuid,
        empty_lookup: &'a Value,
    }

    let fixture = PrivateProjectFixture {
        workspace_id,
        project_id,
        root_id,
        task_id,
        task_number,
        attachment_id,
        empty_lookup: &empty_lookup,
    };

    async fn assert_member_denied(
        app: &axum::Router,
        cookie: &str,
        fx: &PrivateProjectFixture<'_>,
    ) {
        let doc_base = format!(
            "/api/v1/workspaces/{}/documents/{}",
            fx.workspace_id, fx.root_id
        );
        for path in [
            doc_base.clone(),
            format!("{doc_base}/ancestors"),
            format!("{doc_base}/body"),
        ] {
            let (status, _) = json_request(app.clone(), "GET", &path, None, Some(cookie)).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "GET {path}");
        }

        let (status, tree) = json_request(
            app.clone(),
            "GET",
            &format!("/api/v1/workspaces/{}/tree", fx.workspace_id),
            None,
            Some(cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(!tree["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["id"].as_str() == Some(fx.root_id)));

        let (status, _) = json_request(
            app.clone(),
            "POST",
            &format!("{doc_base}/uploads"),
            Some(json!({"name":"x.bin","sizeBytes":1})),
            Some(cookie),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let att_base = format!(
            "/api/v1/workspaces/{}/attachments/{}",
            fx.workspace_id, fx.attachment_id
        );
        let fake_attachment = Uuid::now_v7();
        let (status, _) = json_request(
            app.clone(),
            "POST",
            &format!(
                "/api/v1/workspaces/{}/attachments/{}/complete",
                fx.workspace_id, fake_attachment
            ),
            Some(json!({"parts": [{"partNumber": 1, "etag": "x"}]})),
            Some(cookie),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = json_request(app.clone(), "GET", &att_base, None, Some(cookie)).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let download = format!("{att_base}/download");
        let (status, _, _) = http_request(
            app.clone(),
            "HEAD",
            &download,
            None,
            None,
            Some(cookie),
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _, _) = http_request(
            app.clone(),
            "GET",
            &download,
            None,
            None,
            Some(cookie),
            &[("range", "bytes=0-0")],
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let (status, _) = json_request(
            app.clone(),
            "GET",
            &format!(
                "/api/v1/workspaces/{}/tasks/{}",
                fx.workspace_id, fx.task_id
            ),
            None,
            Some(cookie),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = json_request(
            app.clone(),
            "GET",
            &format!(
                "/api/v1/workspaces/{}/projects/{}/tasks",
                fx.workspace_id, fx.project_id
            ),
            None,
            Some(cookie),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let (status, projects) = json_request(
            app.clone(),
            "GET",
            &format!("/api/v1/workspaces/{}/projects", fx.workspace_id),
            None,
            Some(cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(!projects["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["id"].as_str() == Some(fx.project_id)));

        let (status, lookup) = json_request(
            app.clone(),
            "GET",
            &format!(
                "/api/v1/workspaces/{}/lookup/HID-{}",
                fx.workspace_id, fx.task_number
            ),
            None,
            Some(cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(lookup, *fx.empty_lookup);
    }

    async fn assert_non_member_denied(
        app: &axum::Router,
        cookie: &str,
        fx: &PrivateProjectFixture<'_>,
    ) {
        let get_paths = [
            format!("/api/v1/workspaces/{}/tree", fx.workspace_id),
            format!(
                "/api/v1/workspaces/{}/documents/{}",
                fx.workspace_id, fx.root_id
            ),
            format!(
                "/api/v1/workspaces/{}/documents/{}/ancestors",
                fx.workspace_id, fx.root_id
            ),
            format!(
                "/api/v1/workspaces/{}/documents/{}/body",
                fx.workspace_id, fx.root_id
            ),
            format!(
                "/api/v1/workspaces/{}/attachments/{}",
                fx.workspace_id, fx.attachment_id
            ),
            format!(
                "/api/v1/workspaces/{}/attachments/{}/download",
                fx.workspace_id, fx.attachment_id
            ),
            format!(
                "/api/v1/workspaces/{}/tasks/{}",
                fx.workspace_id, fx.task_id
            ),
            format!("/api/v1/workspaces/{}/projects", fx.workspace_id),
            format!(
                "/api/v1/workspaces/{}/lookup/HID-{}",
                fx.workspace_id, fx.task_number
            ),
        ];
        for path in get_paths {
            let (status, body) = json_request(app.clone(), "GET", &path, None, Some(cookie)).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "GET {path}: {body:?}");
        }

        let doc_uploads = format!(
            "/api/v1/workspaces/{}/documents/{}/uploads",
            fx.workspace_id, fx.root_id
        );
        let (status, body) = json_request(
            app.clone(),
            "POST",
            &doc_uploads,
            Some(json!({"name":"x.bin","sizeBytes":1})),
            Some(cookie),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "POST {doc_uploads}: {body:?}"
        );

        let fake_attachment = Uuid::now_v7();
        let complete = format!(
            "/api/v1/workspaces/{}/attachments/{}/complete",
            fx.workspace_id, fake_attachment
        );
        let (status, body) = json_request(
            app.clone(),
            "POST",
            &complete,
            Some(json!({"parts": [{"partNumber": 1, "etag": "x"}]})),
            Some(cookie),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "POST {complete}: {body:?}");
    }

    async fn assert_unauthenticated(app: &axum::Router, cookie: &str, workspace_id: Uuid) {
        let (status, body) = json_request(
            app.clone(),
            "GET",
            &format!("/api/v1/workspaces/{workspace_id}/projects"),
            None,
            Some(cookie),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["code"], "authentication_required");
    }

    for cookie in [&owner_cookie, &lab_member.cookie, &guest.cookie] {
        assert_member_denied(&app, cookie, &fixture).await;
    }

    assert_non_member_denied(&app, &other_ws_user.cookie, &fixture).await;

    let outsider = add_workspace_user(&admin, workspace_id, "guest", "outsider").await;
    sqlx::query("DELETE FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2")
        .bind(workspace_id)
        .bind(outsider.user_id)
        .execute(&admin)
        .await
        .unwrap();
    assert_non_member_denied(&app, &outsider.cookie, &fixture).await;

    // Project documents join collab under project permission: the private
    // project's lead is admitted, the workspace owner without a grant is not.
    let admission =
        resolve_collab_admission(&pool, workspace_id, lead.user_id, lead_session, root_uuid).await;
    assert!(
        matches!(admission, Ok(Ok(ref a)) if !a.read_only),
        "collab admission for lead: {admission:?}"
    );
    let owner_session = session_id_for_user(&admin, owner_id).await;
    let admission =
        resolve_collab_admission(&pool, workspace_id, owner_id, owner_session, root_uuid).await;
    assert!(
        matches!(admission, Ok(Err(CollabDbError::NotFound))),
        "collab admission for owner: {admission:?}"
    );

    let (status, detail) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}"),
        None,
        Some(&lead.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["id"], task_id);

    let (status, list) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        None,
        Some(&lead.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["items"].as_array().unwrap().len(), 1);

    let (status, projects) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects"),
        None,
        Some(&lead.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(projects["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["id"].as_str() == Some(project_id)));

    let (status, lookup) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/lookup/HID-{task_number}"),
        None,
        Some(&lead.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(lookup["items"].as_array().unwrap().len(), 1);

    let doc_base = format!("/api/v1/workspaces/{workspace_id}/documents/{root_id}");
    for path in [
        doc_base.clone(),
        format!("{doc_base}/ancestors"),
        format!("{doc_base}/body"),
    ] {
        let (status, _) = json_request(app.clone(), "GET", &path, None, Some(&lead.cookie)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "lead GET {path}");
    }

    sqlx::query("UPDATE fvoci.sessions SET revoked_at = now() WHERE user_id = $1")
        .bind(lead.user_id)
        .execute(&admin)
        .await
        .unwrap();
    assert_unauthenticated(&app, &lead.cookie, workspace_id).await;
    let (status, body) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/lookup/HID-{task_number}"),
        None,
        Some(&lead.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], "authentication_required");

    pool.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn clone_project_copies_configuration_not_tasks_or_members() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _user_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;

    let source = create_project(app.clone(), &cookie, workspace_id, "SRC", "workspace").await;
    let source_id = source["id"].as_str().unwrap();

    let (status, label) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{source_id}/labels"),
        Some(json!({"name": "bug", "color": "red"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{label:?}");

    let (status, milestone) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{source_id}/milestones"),
        Some(json!({"name": "M1"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{milestone:?}");

    let (status, clone) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{source_id}/clone"),
        Some(json!({"key": "CPY", "name": "Copy"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{clone:?}");
    let clone_id = clone["id"].as_str().unwrap();

    let events: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.events WHERE workspace_id = $1 AND verb = 'project.created' AND payload->>'sourceProjectId' = $2",
    )
    .bind(workspace_id)
    .bind(source_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(events.0, 1);

    let clone_tasks: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.tasks WHERE workspace_id = $1 AND project_id = $2",
    )
    .bind(workspace_id)
    .bind(Uuid::parse_str(clone_id).unwrap())
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(clone_tasks.0, 0);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn project_documents_tree_create_and_move_emit_events() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _user_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;

    let project = create_project(app.clone(), &cookie, workspace_id, "DOC", "workspace").await;
    let project_id = project["id"].as_str().unwrap();
    let root_id = project["rootDocumentId"].as_str().unwrap();

    let (status, tree) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{tree:?}");
    assert_eq!(tree["items"].as_array().unwrap().len(), 1);

    let (status, created) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents"),
        Some(json!({"parentId": root_id, "title": "Spec"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created:?}");
    let child_id = created["id"].as_str().unwrap();

    let (status, child) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents"),
        Some(json!({"parentId": child_id, "title": "Nested"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{child:?}");
    let nested_id = child["id"].as_str().unwrap();

    let (status, moved) = json_request(
        app.clone(),
        "POST",
        &format!(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{nested_id}/move"
        ),
        Some(json!({"newParentId": root_id})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{moved:?}");

    let moved_events: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.events WHERE workspace_id = $1 AND verb = 'document.moved'",
    )
    .bind(workspace_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert!(moved_events.0 >= 1);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn project_document_move_rejects_descendant_cycle() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _user_id, workspace_id) = setup_session(&harness).await;

    let project = create_project(app.clone(), &cookie, workspace_id, "CYC", "workspace").await;
    let project_id = project["id"].as_str().unwrap();
    let root_id = project["rootDocumentId"].as_str().unwrap();

    let (status, parent) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents"),
        Some(json!({"parentId": root_id, "title": "Parent"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{parent:?}");
    let parent_id = parent["id"].as_str().unwrap();

    let (status, child) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents"),
        Some(json!({"parentId": parent_id, "title": "Child"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{child:?}");
    let child_id = child["id"].as_str().unwrap();

    let (status, body) = json_request(
        app.clone(),
        "POST",
        &format!(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{parent_id}/move"
        ),
        Some(json!({"newParentId": child_id})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body:?}");
    assert_eq!(body["code"], "document_cycle");

    harness.cleanup().await;
}

fn project_revisions_url(workspace_id: Uuid, project_id: &str, document_id: &str) -> String {
    format!(
        "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/revisions"
    )
}

async fn insert_document_revision(
    admin: &sqlx::PgPool,
    workspace_id: Uuid,
    document_id: &str,
) -> Uuid {
    let id = Uuid::now_v7();
    let expected: Value = serde_json::from_str(include_str!(
        "../crates/collab-engine/fixtures/expectations.json"
    ))
    .unwrap();
    let body = fvoci_server::collab::derived_body::prepare_derived_body(
        expected["empty_doc"]["prosemirror_json"].clone(),
    )
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO fvoci.revisions (
            id, workspace_id, target_kind, target_id, y_snapshot, encoding, content_json, text, reason
        ) VALUES ($1, $2, 'document', $3, $4, 1, $5, $6, 'manual')
        "#,
    )
    .bind(id)
    .bind(workspace_id)
    .bind(Uuid::parse_str(document_id).unwrap())
    .bind(include_bytes!("../crates/collab-engine/fixtures/revision_snapshot.bin").as_slice())
    .bind(body.content_json())
    .bind(body.text())
    .execute(admin)
    .await
    .expect("insert revision");
    id
}

async fn project_restore_witness(admin: &sqlx::PgPool, workspace_id: Uuid, doc_id: &str) -> Value {
    sqlx::query_scalar(
        r#"
        SELECT jsonb_build_object(
            'body', (SELECT jsonb_build_object('content', content_json, 'text', text,
                'chosung', chosung, 'updatedAt', updated_at)
                FROM fvoci.documents WHERE workspace_id=$1 AND id=$2),
            'state', (SELECT to_jsonb(s) FROM fvoci.document_states s
                WHERE workspace_id=$1 AND document_id=$2),
            'updates', (SELECT coalesce(jsonb_agg(to_jsonb(u) ORDER BY seq), '[]'::jsonb)
                FROM fvoci.document_collab_updates u WHERE workspace_id=$1 AND document_id=$2),
            'receipts', (SELECT coalesce(jsonb_agg(to_jsonb(r) ORDER BY op_id), '[]'::jsonb)
                FROM fvoci.document_collab_op_receipts r WHERE workspace_id=$1 AND document_id=$2),
            'history', (SELECT coalesce(jsonb_agg(to_jsonb(h) ORDER BY id), '[]'::jsonb)
                FROM fvoci.revisions h WHERE workspace_id=$1 AND target_kind='document' AND target_id=$2),
            'events', (SELECT coalesce(jsonb_agg(to_jsonb(e) ORDER BY id), '[]'::jsonb)
                FROM fvoci.events e WHERE workspace_id=$1 AND target_id=$2),
            'audit', (SELECT coalesce(jsonb_agg(to_jsonb(a) ORDER BY id), '[]'::jsonb)
                FROM fvoci.audit_log a WHERE workspace_id=$1 AND target_id=$2))
        "#,
    )
    .bind(workspace_id)
    .bind(Uuid::parse_str(doc_id).unwrap())
    .fetch_one(admin)
    .await
    .unwrap()
}

/// Project document revisions use the owning project's permission on the
/// project path: members and viewers read, only editors write, the wiki path
/// and a mismatched project path refuse, and an archived project refuses
/// writes with `project_archived`.
#[tokio::test]
async fn project_document_revisions_follow_project_permission() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, _owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lead = add_workspace_user(&admin, workspace_id, "member", "lead").await;
    let viewer = add_workspace_user(&admin, workspace_id, "member", "viewer").await;
    let outsider = add_workspace_user(&admin, workspace_id, "member", "outsider").await;

    let project = create_project(app.clone(), &lead.cookie, workspace_id, "REV", "private").await;
    let project_id = project["id"].as_str().unwrap().to_string();
    let root_id = project["rootDocumentId"].as_str().unwrap().to_string();
    let other = create_project(app.clone(), &lead.cookie, workspace_id, "OTH", "private").await;
    let other_id = other["id"].as_str().unwrap().to_string();
    let (status, body) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/members"),
        Some(json!({"userId": viewer.user_id.to_string(), "role": "viewer"})),
        Some(&lead.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body:?}");
    let (status, created) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents"),
        Some(json!({"parentId": root_id, "title": "Spec"})),
        Some(&lead.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created:?}");
    let doc_id = created["id"].as_str().unwrap().to_string();

    // This pinned Yjs Snapshot belongs to revision_before.v1, whose shared
    // array has no ProseMirror content. The native revision suite consumes
    // this exact pair and checks the independent empty_doc projection.
    let rev = insert_document_revision(&admin, workspace_id, &doc_id).await;
    let sibling_rev = insert_document_revision(&admin, workspace_id, &root_id).await;
    // Retain the Snapshot's client-41 history, with no tail. Its empty
    // ProseMirror projection matches the newly created document body.
    // State must exist so missing state cannot mask a broken restore guard.
    sqlx::query(
        "INSERT INTO fvoci.document_states (workspace_id, document_id, state, encoding) VALUES ($1, $2, $3, 1)",
    )
    .bind(workspace_id)
    .bind(Uuid::parse_str(&doc_id).unwrap())
    .bind(include_bytes!("../crates/collab-engine/fixtures/revision_before.v1").as_slice())
    .execute(&admin)
    .await
    .unwrap();
    let expected: Value = serde_json::from_str(include_str!(
        "../crates/collab-engine/fixtures/expectations.json"
    ))
    .unwrap();
    let before_restore = project_restore_witness(&admin, workspace_id, &doc_id).await;
    // These PostgreSQL CI shards do not build a worker. The restricted app
    // role's successful source resolution is the positive control here; the
    // revision integration suite consumes this exact Snapshot in a native child
    // and also executes actual native HTTP restore.
    let restore_pool = project_harness::app_pool(&harness).await;
    let session_id = project_harness::session_id_for_user(&admin, lead.user_id).await;
    let source = fvoci_server::db::revisions::resolve_restore(
        &restore_pool,
        workspace_id,
        lead.user_id,
        session_id,
        fvoci_server::db::revisions::RevisionScope::project_document(
            Uuid::parse_str(&project_id).unwrap(),
            Uuid::parse_str(&doc_id).unwrap(),
        ),
        rev,
        None,
    )
    .await
    .unwrap()
    .expect("authorized source resolves with real state and snapshot");
    assert_eq!(
        source,
        include_bytes!("../crates/collab-engine/fixtures/revision_snapshot.bin")
    );
    let base = project_revisions_url(workspace_id, &project_id, &doc_id);
    // A well-formed request must reach server authorization. An empty body is
    // invalid before authorization and would not exercise these 404/409 gates.
    let restore_body = json!({"correlationId": Uuid::now_v7(), "expectedTailSeq": "0"});
    let (status, body) = json_request(
        app.clone(),
        "POST",
        &format!("{base}/{rev}/restore"),
        Some(json!({})),
        Some(&lead.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body:?}");
    assert_eq!(body["code"], "invalid_input");

    for cookie in [&lead.cookie, &viewer.cookie] {
        let (status, list) = json_request(app.clone(), "GET", &base, None, Some(cookie)).await;
        assert_eq!(status, StatusCode::OK, "{list:?}");
        let ids: Vec<&str> = list["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, vec![rev.to_string().as_str()]);
        let (status, detail) = json_request(
            app.clone(),
            "GET",
            &format!("{base}/{rev}"),
            None,
            Some(cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{detail:?}");
        assert_eq!(detail["targetId"], doc_id.as_str());
        assert_eq!(
            detail["contentJson"],
            expected["empty_doc"]["prosemirror_json"]
        );
    }

    // A revision of another document is not reachable through this document.
    let (status, _) = json_request(
        app.clone(),
        "GET",
        &format!("{base}/{sibling_rev}"),
        None,
        Some(&lead.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, body) = json_request(
        app.clone(),
        "POST",
        &format!("{base}/{sibling_rev}/restore"),
        Some(restore_body.clone()),
        Some(&lead.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body:?}");

    // Viewers cannot write; restore is refused before any collab work.
    let (status, _) = json_request(app.clone(), "POST", &base, None, Some(&viewer.cookie)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("{base}/{rev}/restore"),
        Some(restore_body.clone()),
        Some(&viewer.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Private project: workspace owner without membership and other members see nothing.
    for cookie in [&owner_cookie, &outsider.cookie] {
        let (status, _) = json_request(app.clone(), "GET", &base, None, Some(cookie)).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = json_request(
            app.clone(),
            "GET",
            &format!("{base}/{rev}"),
            None,
            Some(cookie),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, body) = json_request(
            app.clone(),
            "POST",
            &format!("{base}/{rev}/restore"),
            Some(restore_body.clone()),
            Some(cookie),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body:?}");
    }

    // The wiki path and a mismatched project path do not expose project documents.
    for path in [
        format!("/api/v1/workspaces/{workspace_id}/documents/{doc_id}/revisions"),
        project_revisions_url(workspace_id, &other_id, &doc_id),
    ] {
        let (status, _) = json_request(app.clone(), "GET", &path, None, Some(&lead.cookie)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
        let (status, _) = json_request(
            app.clone(),
            "GET",
            &format!("{path}/{rev}"),
            None,
            Some(&lead.cookie),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
        let (status, _) = json_request(app.clone(), "POST", &path, None, Some(&lead.cookie)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
        let (status, _) = json_request(
            app.clone(),
            "POST",
            &format!("{path}/{rev}/restore"),
            Some(restore_body.clone()),
            Some(&lead.cookie),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
    }

    // Removing the viewer from the project revokes revision access.
    let (status, _) = json_request(
        app.clone(),
        "DELETE",
        &format!(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/members/{}",
            viewer.user_id
        ),
        None,
        Some(&lead.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = json_request(app.clone(), "GET", &base, None, Some(&viewer.cookie)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) = json_request(
        app.clone(),
        "POST",
        &format!("{base}/{rev}/restore"),
        Some(restore_body.clone()),
        Some(&viewer.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body:?}");

    // Archived project: reads stay, writes are refused with project_archived.
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/archive"),
        None,
        Some(&lead.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = json_request(app.clone(), "GET", &base, None, Some(&lead.cookie)).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = json_request(app.clone(), "POST", &base, None, Some(&lead.cookie)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body:?}");
    assert_eq!(body["code"], "project_archived");
    let (status, body) = json_request(
        app.clone(),
        "POST",
        &format!("{base}/{rev}/restore"),
        Some(restore_body.clone()),
        Some(&lead.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body:?}");
    assert_eq!(body["code"], "project_archived");
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/unarchive"),
        None,
        Some(&lead.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    assert_eq!(
        project_restore_witness(&admin, workspace_id, &doc_id).await,
        before_restore,
        "source resolution and malformed/scope/permission/archive denials cannot change document effects",
    );

    // A trashed project document has no revision routes.
    let (status, body) = json_request(
        app.clone(),
        "POST",
        &format!(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{doc_id}/trash"
        ),
        None,
        Some(&lead.cookie),
    )
    .await;
    assert!(status.is_success(), "{status} {body:?}");
    let (status, _) = json_request(app.clone(), "GET", &base, None, Some(&lead.cookie)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let trashed = project_restore_witness(&admin, workspace_id, &doc_id).await;
    let (status, body) = json_request(
        app.clone(),
        "POST",
        &format!("{base}/{rev}/restore"),
        Some(restore_body),
        Some(&lead.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body:?}");
    assert_eq!(
        project_restore_witness(&admin, workspace_id, &doc_id).await,
        trashed
    );

    restore_pool.close().await;
    admin.close().await;
    harness.cleanup().await;
}

// ---------------------------------------------------------------------------
// Project reads take no row lock on the project
// ---------------------------------------------------------------------------

/// Project row locks other transactions hold while a read runs: a collab append
/// or admission (`FOR SHARE`) and a project mutation (`FOR NO KEY UPDATE`).
const PROJECT_ROW_HOLDERS: [&str; 2] = ["FOR SHARE", "FOR NO KEY UPDATE"];

/// Project `RDL` (private), a lead, a workspace guest who is only a project
/// viewer, one task with a comment, and the project root document with a
/// comment.
struct ProjectReadFixture {
    workspace_id: Uuid,
    project_id: String,
    root_id: String,
    task_id: String,
    viewer: project_harness::TestUser,
}

async fn project_read_fixture(app: &axum::Router, admin: &sqlx::PgPool) -> ProjectReadFixture {
    let workspace_id: Uuid =
        sqlx::query_scalar("SELECT id FROM fvoci.workspaces WHERE slug = 'acme'")
            .fetch_one(admin)
            .await
            .unwrap();
    let lead = add_workspace_user(admin, workspace_id, "member", "lead").await;
    let viewer = add_workspace_user(admin, workspace_id, "guest", "viewer").await;
    let project = create_project(app.clone(), &lead.cookie, workspace_id, "RDL", "private").await;
    let project_id = project["id"].as_str().unwrap().to_string();
    let root_id = project["rootDocumentId"].as_str().unwrap().to_string();
    let ws = format!("/api/v1/workspaces/{workspace_id}");
    let (status, body) = json_request(
        app.clone(),
        "POST",
        &format!("{ws}/projects/{project_id}/members"),
        Some(json!({"userId": viewer.user_id.to_string(), "role": "viewer"})),
        Some(&lead.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body:?}");
    let (status, task) = json_request(
        app.clone(),
        "POST",
        &format!("{ws}/projects/{project_id}/tasks"),
        Some(json!({"title": "Read barrier task"})),
        Some(&lead.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{task:?}");
    let task_id = task["id"].as_str().unwrap().to_string();
    for path in [
        format!("{ws}/tasks/{task_id}/comments"),
        format!("{ws}/projects/{project_id}/documents/{root_id}/comments"),
    ] {
        let (status, body) = json_request(
            app.clone(),
            "POST",
            &path,
            Some(json!({"body": "hello"})),
            Some(&lead.cookie),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "POST {path}: {body:?}");
    }
    ProjectReadFixture {
        workspace_id,
        project_id,
        root_id,
        task_id,
        viewer,
    }
}

/// GETs every path as the viewer while an admin transaction holds the project
/// row with each of [`PROJECT_ROW_HOLDERS`], and returns the `(holder, path)`
/// pairs that did not answer within 2 s. Each holder is released before the
/// next request, so a request that waited finishes and frees its connection.
async fn reads_waiting_on_project_row(
    app: &axum::Router,
    admin: &sqlx::PgPool,
    fx: &ProjectReadFixture,
    paths: &[String],
) -> Vec<String> {
    let mut waited = Vec::new();
    for holder_lock in PROJECT_ROW_HOLDERS {
        for path in paths {
            let mut holder = admin.begin().await.unwrap();
            sqlx::query(&format!(
                "SELECT id FROM fvoci.projects WHERE workspace_id = $1 AND id = $2::uuid {holder_lock}"
            ))
            .bind(fx.workspace_id)
            .bind(&fx.project_id)
            .fetch_one(&mut *holder)
            .await
            .unwrap();
            let response = tokio::time::timeout(
                Duration::from_secs(2),
                json_request(app.clone(), "GET", path, None, Some(&fx.viewer.cookie)),
            )
            .await;
            holder.rollback().await.unwrap();
            match response {
                Ok((status, body)) => {
                    assert_eq!(status, StatusCode::OK, "GET {path}: {body:?}")
                }
                Err(_) => waited.push(format!("{holder_lock}: GET {path}")),
            }
        }
    }
    waited
}

/// Task reads (list, detail, dependencies, activity, time entries, backlinks,
/// parent candidates) read the project row without locking it, so a view-only
/// guest cannot stall collab appends or project writes, and a collab append
/// or project mutation does not stall them.
#[tokio::test]
async fn task_reads_do_not_wait_on_project_row_locks() {
    let harness = TestDb::bootstrap().await;
    let (app, _, _, _) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let fx = project_read_fixture(&app, &admin).await;
    let ws = format!("/api/v1/workspaces/{}", fx.workspace_id);
    let (project_id, task_id) = (&fx.project_id, &fx.task_id);
    let paths = vec![
        format!("{ws}/projects/{project_id}/tasks"),
        format!("{ws}/tasks/{task_id}"),
        format!("/api/v1/tasks/{task_id}"),
        format!("{ws}/projects/{project_id}/dependencies"),
        format!("{ws}/tasks/{task_id}/activity"),
        format!("{ws}/tasks/{task_id}/time-entries"),
        format!("{ws}/tasks/{task_id}/backlinks"),
        format!("{ws}/projects/{project_id}/tasks/parents?childType=task"),
    ];
    let waited = reads_waiting_on_project_row(&app, &admin, &fx, &paths).await;
    assert!(
        waited.is_empty(),
        "task reads waited on a project row lock: {waited:#?}"
    );
    admin.close().await;
    harness.cleanup().await;
}

/// Project, label, milestone, group-grant, comment, project document and
/// revision reads take no project row lock either.
#[tokio::test]
async fn project_reads_do_not_wait_on_project_row_locks() {
    let harness = TestDb::bootstrap().await;
    let (app, _, _, _) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let fx = project_read_fixture(&app, &admin).await;
    let ws = format!("/api/v1/workspaces/{}", fx.workspace_id);
    let (project_id, task_id, root_id) = (&fx.project_id, &fx.task_id, &fx.root_id);
    let doc = format!("{ws}/projects/{project_id}/documents/{root_id}");
    let paths = vec![
        format!("{ws}/projects"),
        format!("{ws}/projects/{project_id}"),
        format!("{ws}/projects/{project_id}/members"),
        format!("{ws}/projects/{project_id}/workflow"),
        format!("{ws}/projects/{project_id}/groups"),
        format!("{ws}/projects/{project_id}/labels"),
        format!("{ws}/projects/{project_id}/milestones"),
        format!("{ws}/tasks/{task_id}/comments"),
        format!("{ws}/tasks/{task_id}/revisions"),
        format!("{ws}/projects/{project_id}/documents"),
        doc.clone(),
        format!("{doc}/comments"),
        format!("{doc}/children"),
        format!("{doc}/ancestors"),
        format!("{doc}/backlinks"),
        format!("{doc}/revisions"),
        format!("/api/v1/documents/{root_id}"),
    ];
    let waited = reads_waiting_on_project_row(&app, &admin, &fx, &paths).await;
    assert!(
        waited.is_empty(),
        "project reads waited on a project row lock: {waited:#?}"
    );
    admin.close().await;
    harness.cleanup().await;
}

/// A task list read that is paused after its permission check holds no
/// transaction id, so a slow list never holds back the cluster-wide
/// `pg_snapshot_xmin` gate that SSE and outbox consumers wait on.
#[tokio::test]
async fn paused_task_list_read_holds_no_transaction_id() {
    let harness = TestDb::bootstrap().await;
    let (app, _, _, _) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let fx = project_read_fixture(&app, &admin).await;
    let path = format!(
        "/api/v1/workspaces/{}/projects/{}/tasks",
        fx.workspace_id, fx.project_id
    );

    // The list reads task_labels for its items after the permission check.
    let mut pause = admin.begin().await.unwrap();
    sqlx::query("LOCK TABLE fvoci.task_labels IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *pause)
        .await
        .unwrap();
    let pause_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *pause)
        .await
        .unwrap();
    let app_bg = app.clone();
    let cookie = fx.viewer.cookie.clone();
    let list =
        tokio::spawn(async move { json_request(app_bg, "GET", &path, None, Some(&cookie)).await });
    let waiting = wait_for_blocked_by_holder(&admin, pause_pid, Some("%task_labels%"), 1).await;
    let backend_xid: Option<String> =
        sqlx::query_scalar("SELECT backend_xid::text FROM pg_stat_activity WHERE pid = $1")
            .bind(waiting[0])
            .fetch_one(&admin)
            .await
            .unwrap();
    pause.rollback().await.unwrap();
    let (status, body) = tokio::time::timeout(Duration::from_secs(10), list)
        .await
        .expect("list finished")
        .expect("join");
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert_eq!(body["items"].as_array().map(Vec::len), Some(1), "{body:?}");
    assert_eq!(
        backend_xid, None,
        "a paused task list read holds a transaction id"
    );
    admin.close().await;
    harness.cleanup().await;
}

/// Backlink reads scan the body of every live document and task in the
/// workspace. With a table lock held on `fvoci.tasks`, the document read
/// reaches its task body scan and the task read blocks earlier on its task
/// lookup; both end at the per-transaction 15 s statement timeout with a 500
/// instead of holding a pool connection and its snapshot for as long as they
/// wait.
#[tokio::test]
async fn backlink_scans_end_at_the_statement_timeout() {
    let harness = TestDb::bootstrap().await;
    let (app, _, _, _) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let fx = project_read_fixture(&app, &admin).await;
    let ws = format!("/api/v1/workspaces/{}", fx.workspace_id);
    let document_backlinks = format!(
        "{ws}/projects/{}/documents/{}/backlinks",
        fx.project_id, fx.root_id
    );
    let paths = [
        document_backlinks.clone(),
        format!("{ws}/tasks/{}/backlinks", fx.task_id),
    ];

    let mut pause = admin.begin().await.unwrap();
    sqlx::query("LOCK TABLE fvoci.tasks IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *pause)
        .await
        .unwrap();
    let pause_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *pause)
        .await
        .unwrap();
    let reads: Vec<_> = paths
        .iter()
        .map(|path| {
            let (app, path, cookie) = (app.clone(), path.clone(), fx.viewer.cookie.clone());
            tokio::spawn(async move { json_request(app, "GET", &path, None, Some(&cookie)).await })
        })
        .collect();
    // The document read has passed its permission check and reached the
    // task body scan.
    wait_for_blocked_by_holder(
        &admin,
        pause_pid,
        Some("%jsonb_path_exists(t.content_json%"),
        1,
    )
    .await;
    wait_for_blocked_by_holder(&admin, pause_pid, None, paths.len()).await;

    // Twice the statement timeout: a bounded read has ended well before.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut unbounded = Vec::new();
    for (path, read) in paths.iter().zip(reads) {
        match tokio::time::timeout_at(deadline, read).await {
            Ok(joined) => {
                let (status, body) = joined.expect("join");
                assert_eq!(
                    status,
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "GET {path}: {body:?}"
                );
            }
            Err(_) => unbounded.push(format!("GET {path}")),
        }
    }
    pause.rollback().await.unwrap();
    assert!(
        unbounded.is_empty(),
        "backlink scans still ran with fvoci.tasks locked for 30 s: {unbounded:#?}"
    );

    let (status, body) = json_request(
        app.clone(),
        "GET",
        &document_backlinks,
        None,
        Some(&fx.viewer.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    admin.close().await;
    harness.cleanup().await;
}

// Project document cardinality is delivered with the authorized project list,
// including root/all statuses, without querying each project's document tree.
async fn count_list(app: &axum::Router, cookie: &str, ws: Uuid) -> Vec<Value> {
    let (status, body) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{ws}/projects"),
        None,
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    body["items"].as_array().unwrap().clone()
}

fn count_item<'a>(items: &'a [Value], project: &Value) -> &'a Value {
    items
        .iter()
        .find(|item| item["id"] == project["id"])
        .expect("visible project")
}

async fn count_mutation(
    app: &axum::Router,
    cookie: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Value {
    let (status, result) = json_request(app.clone(), method, path, body, Some(cookie)).await;
    assert!(
        status == StatusCode::OK || status == StatusCode::CREATED,
        "{method} {path}: {status} {result:?}"
    );
    result
}

async fn assert_document_count(
    app: &axum::Router,
    cookie: &str,
    ws: Uuid,
    project: &Value,
    expected: i64,
) {
    let items = count_list(app, cookie, ws).await;
    assert_eq!(count_item(&items, project)["documentCount"], expected);
    let path = format!(
        "/api/v1/workspaces/{ws}/projects/{}/documents",
        project["id"].as_str().unwrap()
    );
    let (status, tree) = json_request(app.clone(), "GET", &path, None, Some(cookie)).await;
    assert_eq!(status, StatusCode::OK, "{tree:?}");
    assert_eq!(tree["items"].as_array().unwrap().len() as i64, expected);
}

#[tokio::test]
async fn project_document_count_root_statuses_lifecycle_affiliation_and_tenant() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner, ws) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let pool = app_pool(&harness).await;
    let (superuser, bypassrls): (bool, bool) =
        sqlx::query_as("SELECT rolsuper, rolbypassrls FROM pg_roles WHERE rolname = current_user")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        !superuser && !bypassrls,
        "counts must execute as the restricted app role"
    );
    let a = create_project(app.clone(), &cookie, ws, "CNT", "workspace").await;
    let b = create_project(app.clone(), &cookie, ws, "DST", "workspace").await;
    let aid = a["id"].as_str().unwrap();
    let bid = b["id"].as_str().unwrap();
    let base = format!("/api/v1/workspaces/{ws}/projects/{aid}");
    assert_document_count(&app, &cookie, ws, &a, 1).await;
    let draft = count_mutation(
        &app,
        &cookie,
        "POST",
        &format!("{base}/documents"),
        Some(json!({"parentId": a["rootDocumentId"], "title": "Draft"})),
    )
    .await;
    let published = count_mutation(
        &app,
        &cookie,
        "POST",
        &format!("{base}/documents"),
        Some(json!({"parentId": draft["id"], "title": "Published"})),
    )
    .await;
    let archived = count_mutation(
        &app,
        &cookie,
        "POST",
        &format!("{base}/documents"),
        Some(json!({"parentId": published["id"], "title": "Archived"})),
    )
    .await;
    for (doc, status) in [(&published, "published"), (&archived, "archived")] {
        count_mutation(
            &app,
            &cookie,
            "PATCH",
            &format!("{base}/documents/{}", doc["id"].as_str().unwrap()),
            Some(json!({"status": status})),
        )
        .await;
    }
    assert_document_count(&app, &cookie, ws, &a, 4).await;
    count_mutation(
        &app,
        &cookie,
        "POST",
        &format!("{base}/documents/{}/trash", draft["id"].as_str().unwrap()),
        None,
    )
    .await;
    assert_document_count(&app, &cookie, ws, &a, 1).await;
    // Restore respects the existing parent-first contract and counts each live row.
    for (doc, expected) in [(&draft, 2), (&published, 3), (&archived, 4)] {
        count_mutation(
            &app,
            &cookie,
            "POST",
            &format!("{base}/documents/{}/restore", doc["id"].as_str().unwrap()),
            None,
        )
        .await;
        assert_document_count(&app, &cookie, ws, &a, expected).await;
    }
    // Same-project reparenting changes no cardinality.
    count_mutation(
        &app,
        &cookie,
        "POST",
        &format!("{base}/documents/{}/move", archived["id"].as_str().unwrap()),
        Some(json!({"newParentId": a["rootDocumentId"]})),
    )
    .await;
    assert_document_count(&app, &cookie, ws, &a, 4).await;
    let wiki = count_mutation(
        &app,
        &cookie,
        "POST",
        &format!("/api/v1/workspaces/{ws}/documents"),
        Some(json!({"commandId": uuid::Uuid::now_v7(), "parentId": null, "title": "Wiki"})),
    )
    .await;
    assert_document_count(&app, &cookie, ws, &a, 4).await;
    assert_document_count(&app, &cookie, ws, &b, 1).await;
    count_mutation(
        &app,
        &cookie,
        "POST",
        &format!(
            "/api/v1/workspaces/{ws}/documents/{}/move",
            wiki["id"].as_str().unwrap()
        ),
        Some(json!({"newParentId": b["rootDocumentId"]})),
    )
    .await;
    assert_document_count(&app, &cookie, ws, &a, 4).await;
    assert_document_count(&app, &cookie, ws, &b, 2).await;
    // Cross-project moves are not exposed by current product routes. Seed an
    // actual committed affiliation transition to check both grouped counts.
    sqlx::query("UPDATE fvoci.documents SET project_id = $1, parent_id = $2, path = (SELECT path FROM fvoci.documents WHERE id = $2) || '.' || replace(id::text, '-', '') WHERE id = $3")
        .bind(Uuid::parse_str(bid).unwrap())
        .bind(Uuid::parse_str(b["rootDocumentId"].as_str().unwrap()).unwrap())
        .bind(Uuid::parse_str(archived["id"].as_str().unwrap()).unwrap())
        .execute(&admin)
        .await
        .unwrap();
    assert_document_count(&app, &cookie, ws, &a, 3).await;
    assert_document_count(&app, &cookie, ws, &b, 3).await;
    count_mutation(&app, &cookie, "POST", &format!("{base}/archive"), None).await;
    let items = count_list(&app, &cookie, ws).await;
    assert_eq!(count_item(&items, &a)["status"], "archived");
    assert_eq!(count_item(&items, &a)["documentCount"], 3);
    count_mutation(&app, &cookie, "POST", &format!("{base}/unarchive"), None).await;
    count_mutation(&app, &cookie, "DELETE", &base, None).await;
    assert!(!count_list(&app, &cookie, ws)
        .await
        .iter()
        .any(|item| item["id"] == a["id"]));
    let (status, deleted) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{ws}/projects?deleted=true"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{deleted:?}");
    let item = count_item(deleted["items"].as_array().unwrap(), &a);
    assert!(item.as_object().unwrap().contains_key("documentCount"));
    assert!(item["documentCount"].is_null());
    assert_eq!(item["taskCount"], 0);
    assert_eq!(item["openTaskCount"], 0);
    assert_eq!(item["canEdit"], false);
    assert_eq!(item["canManage"], false);
    count_mutation(&app, &cookie, "POST", &format!("{base}/restore"), None).await;
    assert_document_count(&app, &cookie, ws, &a, 3).await;
    // An actual visible project with no document rows has a real zero.
    let empty_id = Uuid::now_v7();
    project_harness::insert_minimal_project(&admin, ws, empty_id, "EMP", owner, "workspace").await;
    let other_ws = count_mutation(
        &app,
        &cookie,
        "POST",
        "/api/v1/workspaces",
        Some(json!({"slug": "count-other", "name": "Other"})),
    )
    .await;
    let other_ws = Uuid::parse_str(other_ws["id"].as_str().unwrap()).unwrap();
    let foreign = create_project(app.clone(), &cookie, other_ws, "CNT", "workspace").await;
    assert_document_count(&app, &cookie, other_ws, &foreign, 1).await;
    let items = count_list(&app, &cookie, ws).await;
    assert_eq!(
        items
            .iter()
            .map(|row| row["key"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["CNT", "DST", "EMP"]
    );
    assert_eq!(items[2]["documentCount"], 0);
    assert!(!items.iter().any(|item| item["id"] == foreign["id"]));
    assert_document_count(&app, &cookie, ws, &a, 3).await;
    assert_document_count(&app, &cookie, ws, &b, 3).await;
    pool.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn project_document_count_visibility_grants_revoke_and_credentials() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, ws) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lead = add_workspace_user(&admin, ws, "member", "count-lead").await;
    let member = add_workspace_user(&admin, ws, "member", "count-member").await;
    let guest = add_workspace_user(&admin, ws, "guest", "count-guest").await;
    let workspace_admin = add_workspace_user(&admin, ws, "admin", "count-admin").await;
    let public = create_project(app.clone(), &cookie, ws, "PUB", "workspace").await;
    let private = create_project(app.clone(), &lead.cookie, ws, "PVT", "private").await;
    let pid = private["id"].as_str().unwrap();
    let base = format!("/api/v1/workspaces/{ws}");
    for actor in [&cookie, &member.cookie, &workspace_admin.cookie] {
        let items = count_list(&app, actor, ws).await;
        assert_eq!(
            items.len(),
            1,
            "private counts must not grant admin visibility"
        );
        assert_eq!(count_item(&items, &public)["documentCount"], 1);
    }
    assert!(
        count_list(&app, &guest.cookie, ws).await.is_empty(),
        "public projects still need guest grants"
    );
    count_mutation(
        &app,
        &lead.cookie,
        "POST",
        &format!("{base}/projects/{pid}/members"),
        Some(json!({"userId": guest.user_id, "role": "viewer"})),
    )
    .await;
    assert_document_count(&app, &guest.cookie, ws, &private, 1).await;
    let mut groups = Vec::new();
    for name in ["Count group one", "Count group two"] {
        let group = count_mutation(
            &app,
            &cookie,
            "POST",
            &format!("{base}/groups"),
            Some(json!({"name": name})),
        )
        .await;
        let gid = group["id"].as_str().unwrap().to_string();
        for actor in [&guest, &member] {
            count_mutation(
                &app,
                &cookie,
                "POST",
                &format!("{base}/groups/{gid}/members"),
                Some(json!({"userId": actor.user_id})),
            )
            .await;
        }
        count_mutation(
            &app,
            &lead.cookie,
            "POST",
            &format!("{base}/projects/{pid}/groups"),
            Some(json!({"groupId": gid, "role": "viewer"})),
        )
        .await;
        groups.push(gid);
    }
    let items = count_list(&app, &guest.cookie, ws).await;
    assert_eq!(
        items.len(),
        1,
        "multiple direct/group grants do not multiply projects"
    );
    assert_eq!(
        items[0]["documentCount"], 1,
        "multiple grants do not multiply documents"
    );
    assert_eq!(items[0]["canEdit"], false);
    assert_eq!(items[0]["canManage"], false);
    assert_document_count(&app, &member.cookie, ws, &private, 1).await;
    count_mutation(
        &app,
        &lead.cookie,
        "DELETE",
        &format!("{base}/projects/{pid}/members/{}", guest.user_id),
        None,
    )
    .await;
    assert_document_count(&app, &guest.cookie, ws, &private, 1).await;
    count_mutation(
        &app,
        &cookie,
        "DELETE",
        &format!("{base}/groups/{}/members", groups[0]),
        Some(json!({"userId": guest.user_id})),
    )
    .await;
    assert_document_count(&app, &guest.cookie, ws, &private, 1).await;
    for gid in &groups {
        count_mutation(
            &app,
            &cookie,
            "DELETE",
            &format!("{base}/groups/{gid}/members"),
            Some(json!({"userId": member.user_id})),
        )
        .await;
    }
    assert_eq!(
        count_list(&app, &member.cookie, ws).await.len(),
        1,
        "group membership removal hides private counts"
    );
    for gid in &groups {
        count_mutation(
            &app,
            &lead.cookie,
            "DELETE",
            &format!("{base}/projects/{pid}/groups"),
            Some(json!({"groupId": gid})),
        )
        .await;
    }
    assert!(
        count_list(&app, &guest.cookie, ws).await.is_empty(),
        "committed last grant revoke applies to next snapshot"
    );
    assert_eq!(count_list(&app, &member.cookie, ws).await.len(), 1);
    count_mutation(
        &app,
        &cookie,
        "POST",
        &format!("{base}/projects/{}/members", public["id"].as_str().unwrap()),
        Some(json!({"userId": guest.user_id, "role": "viewer"})),
    )
    .await;
    assert_document_count(&app, &guest.cookie, ws, &public, 1).await;
    // Suspension, expiry and membership removal are committed before each read.
    sqlx::query("UPDATE fvoci.users SET suspended_at = now() WHERE id = $1")
        .bind(guest.user_id)
        .execute(&admin)
        .await
        .unwrap();
    let (status, _) = json_request(
        app.clone(),
        "GET",
        &format!("{base}/projects"),
        None,
        Some(&guest.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    sqlx::query("UPDATE fvoci.users SET suspended_at = NULL WHERE id = $1")
        .bind(guest.user_id)
        .execute(&admin)
        .await
        .unwrap();
    assert_document_count(&app, &guest.cookie, ws, &public, 1).await;
    sqlx::query(
        "UPDATE fvoci.sessions SET expires_at = now() - interval '1 second' WHERE user_id = $1",
    )
    .bind(guest.user_id)
    .execute(&admin)
    .await
    .unwrap();
    let (status, _) = json_request(
        app.clone(),
        "GET",
        &format!("{base}/projects"),
        None,
        Some(&guest.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    sqlx::query("DELETE FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2")
        .bind(ws)
        .bind(member.user_id)
        .execute(&admin)
        .await
        .unwrap();
    let (status, _) = json_request(
        app.clone(),
        "GET",
        &format!("{base}/projects"),
        None,
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let other = count_mutation(
        &app,
        &cookie,
        "POST",
        "/api/v1/workspaces",
        Some(json!({"slug": "counts-denied", "name": "Other"})),
    )
    .await;
    let (status, _) = json_request(
        app.clone(),
        "GET",
        &format!(
            "/api/v1/workspaces/{}/projects",
            other["id"].as_str().unwrap()
        ),
        None,
        Some(&lead.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn project_document_count_pat_scopes_workspace_binding_and_deleted() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, ws) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let project = create_project(app.clone(), &cookie, ws, "PAT", "workspace").await;
    let path = format!("/api/v1/workspaces/{ws}/projects");
    let other = count_mutation(
        &app,
        &cookie,
        "POST",
        "/api/v1/workspaces",
        Some(json!({"slug": "count-pat", "name": "Other"})),
    )
    .await;
    let mut both = String::new();
    let mut projects_only = String::new();
    for (name, scopes, allowed, expected) in [
        ("projects", vec!["projects.read"], true, Value::Null),
        (
            "both",
            vec!["projects.read", "documents.read"],
            true,
            json!(1),
        ),
        ("documents", vec!["documents.read"], false, Value::Null),
    ] {
        let token = count_mutation(
            &app,
            &cookie,
            "POST",
            &format!("/api/v1/workspaces/{ws}/api-tokens"),
            Some(json!({"name": name, "scopes": scopes})),
        )
        .await;
        let secret = token["token"].as_str().unwrap();
        let authorization = format!("Bearer {secret}");
        let (status, body, _) = http_request(
            app.clone(),
            "GET",
            &path,
            None,
            None,
            None,
            &[("authorization", &authorization)],
        )
        .await;
        if allowed {
            assert_eq!(status, StatusCode::OK, "{body:?}");
            let item = &body["items"][0];
            assert!(item.as_object().unwrap().contains_key("documentCount"));
            assert_eq!(item["documentCount"], expected);
            assert_eq!(item["taskCount"], 0);
            assert_eq!(item["openTaskCount"], 0);
            assert_eq!(item["canEdit"], true);
            assert_eq!(item["canManage"], true);
        } else {
            assert_eq!(
                status,
                StatusCode::NOT_FOUND,
                "missing ProjectsRead remains refused: {body:?}"
            );
        }
        let foreign_path = format!(
            "/api/v1/workspaces/{}/projects",
            other["id"].as_str().unwrap()
        );
        let (status, _, _) = http_request(
            app.clone(),
            "GET",
            &foreign_path,
            None,
            None,
            None,
            &[("authorization", &authorization)],
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "token workspace binding");
        if name == "both" {
            both = secret.to_string();
        }
        if name == "projects" {
            projects_only = secret.to_string();
        }
    }
    count_mutation(
        &app,
        &cookie,
        "DELETE",
        &format!("{path}/{}", project["id"].as_str().unwrap()),
        None,
    )
    .await;
    for secret in [&both, &projects_only] {
        let authorization = format!("Bearer {secret}");
        let (status, body, _) = http_request(
            app.clone(),
            "GET",
            &format!("{path}?deleted=true"),
            None,
            None,
            None,
            &[("authorization", &authorization)],
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body:?}");
        assert_eq!(body["items"].as_array().unwrap().len(), 1);
        assert!(body["items"][0]
            .as_object()
            .unwrap()
            .contains_key("documentCount"));
        assert!(body["items"][0]["documentCount"].is_null());
    }
    let authorization = format!("Bearer {both}");
    sqlx::query("DELETE FROM fvoci.api_tokens WHERE token_hash = $1")
        .bind(fvoci_server::auth::token::hash_token(&both))
        .execute(&admin)
        .await
        .unwrap();
    let (status, _, _) = http_request(
        app.clone(),
        "GET",
        &path,
        None,
        None,
        None,
        &[("authorization", &authorization)],
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn project_document_count_snapshot_keeps_grants_counts_and_permissions_consistent() {
    let harness = TestDb::bootstrap().await;
    let (app, _, _, _) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let fx = project_read_fixture(&app, &admin).await;
    let path = format!("/api/v1/workspaces/{}/projects", fx.workspace_id);
    let mut pause = admin.begin().await.unwrap();
    sqlx::query("LOCK TABLE fvoci.documents IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *pause)
        .await
        .unwrap();
    let pause_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *pause)
        .await
        .unwrap();
    let app_bg = app.clone();
    let cookie = fx.viewer.cookie.clone();
    let list =
        tokio::spawn(async move { json_request(app_bg, "GET", &path, None, Some(&cookie)).await });
    let waiting = wait_for_blocked_by_holder(&admin, pause_pid, Some("%document_count%"), 1).await;
    let xid: Option<String> =
        sqlx::query_scalar("SELECT backend_xid::text FROM pg_stat_activity WHERE pid = $1")
            .bind(waiting[0])
            .fetch_one(&admin)
            .await
            .unwrap();
    sqlx::query("DELETE FROM fvoci.project_members WHERE workspace_id = $1 AND project_id = $2::uuid AND user_id = $3")
        .bind(fx.workspace_id).bind(&fx.project_id).bind(fx.viewer.user_id).execute(&mut *pause).await.unwrap();
    sqlx::query("UPDATE fvoci.documents SET deleted_at = now() WHERE workspace_id = $1 AND project_id = $2::uuid")
        .bind(fx.workspace_id).bind(&fx.project_id).execute(&mut *pause).await.unwrap();
    pause.commit().await.unwrap();
    let (status, body) = tokio::time::timeout(Duration::from_secs(10), list)
        .await
        .expect("snapshot list finished")
        .expect("join");
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        body["items"][0]["documentCount"], 1,
        "initial live snapshot"
    );
    assert_eq!(body["items"][0]["taskCount"], 1);
    assert_eq!(body["items"][0]["canEdit"], false);
    assert_eq!(body["items"][0]["canManage"], false);
    assert_eq!(xid, None, "list read allocates no transaction id");
    assert!(
        count_list(&app, &fx.viewer.cookie, fx.workspace_id)
            .await
            .is_empty(),
        "next snapshot applies revoke"
    );
    admin.close().await;
    harness.cleanup().await;
}

/// SQLite family ports of the project routes: every operation runs through
/// the real router on a migrated database and is read back on a new
/// connection.
mod sqlite_port {
    use super::project_harness::{http_request, json_request};
    use axum::http::StatusCode;
    use fvoci_server::auth::{password::Keyring, AuthService};
    use fvoci_server::db::{backend::Backend, migrate, pool, Db};
    use fvoci_server::http::{rate_limit::RateLimiter, state::AppState};
    use serde_json::{json, Value};
    use sqlx::SqlitePool;
    use std::{path::PathBuf, sync::Arc};
    use uuid::Uuid;

    pub(super) fn blob(id: Uuid) -> Vec<u8> {
        id.as_bytes().to_vec()
    }

    pub(super) fn id_of(value: &Value) -> Uuid {
        Uuid::parse_str(value.as_str().expect("id string")).expect("uuid")
    }

    pub(super) struct Member {
        pub id: Uuid,
        pub cookie: String,
    }

    pub(super) struct Lite {
        root: PathBuf,
        pub path: PathBuf,
        pub pool: SqlitePool,
        pub app: axum::Router,
        pub ws: Uuid,
        pub owner: Member,
    }

    fn app(backend: Backend, root: &std::path::Path) -> axum::Router {
        fvoci_server::http::router(
            AppState {
                realtime_mode: fvoci_server::config::RealtimeMode::On,
                native_engine: None,
                auth: Arc::new(AuthService {
                    db: Db::from_backend(backend),
                    password_keys: Keyring::parse(
                        r#"{"test":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#,
                        "test",
                    )
                    .unwrap(),
                }),
                branding_name: "Projects".into(),
                public_origin: "http://localhost".into(),
                cookie_secure: false,
                rate_limiter: RateLimiter::new(),
                storage: fvoci_server::attachments::LocalStorage::new(root.join("storage")).into(),
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
                markdown: None,
                import_wake: None,
                import_extractor_available: false,
                preview_extract: None,
                quota: Default::default(),
                mailer: Arc::new(fvoci_server::mail::Mailer::disabled()),
                streams: AppState::fresh_streams(),
            },
            None,
        )
    }

    impl Lite {
        pub async fn new() -> Self {
            let root = std::env::temp_dir().join(format!("projects-port-{}", Uuid::now_v7()));
            std::fs::create_dir_all(&root).unwrap();
            let path = root.join("projects.sqlite");
            migrate::run_sqlite_migrations(&path).await.unwrap();
            let pool = pool::connect_sqlite_app(&path, 4).await.unwrap();
            let app = app(Backend::Sqlite(pool.clone()), &root);
            let ws = insert_workspace(&pool, "lite").await;
            let owner = insert_member(&pool, ws, "owner", "owner").await;
            Self {
                root,
                path,
                pool,
                app,
                ws,
                owner,
            }
        }

        pub async fn workspace(&self, slug: &str) -> Uuid {
            insert_workspace(&self.pool, slug).await
        }

        pub async fn member(&self, ws: Uuid, role: &str, label: &str) -> Member {
            insert_member(&self.pool, ws, role, label).await
        }

        /// A personal access token of `who` in this workspace with `scopes`.
        pub async fn token(&self, who: &Member, scopes: &str) -> String {
            let token = fvoci_server::auth::token::new_token();
            sqlx::query("INSERT INTO api_tokens(id,workspace_id,user_id,token_hash,name,scopes) VALUES(?1,?2,?3,?4,'Port',?5)")
                .bind(blob(Uuid::now_v7()))
                .bind(blob(self.ws))
                .bind(blob(who.id))
                .bind(&token.hash)
                .bind(scopes)
                .execute(&self.pool)
                .await
                .unwrap();
            token.token
        }

        pub fn url(&self, rest: &str) -> String {
            format!("/api/v1/workspaces/{}{rest}", self.ws)
        }

        pub async fn call(
            &self,
            method: &str,
            rest: &str,
            body: Option<Value>,
            who: &Member,
        ) -> (StatusCode, Value) {
            json_request(
                self.app.clone(),
                method,
                &self.url(rest),
                body,
                Some(&who.cookie),
            )
            .await
        }

        pub async fn bearer(
            &self,
            method: &str,
            rest: &str,
            body: Option<Value>,
            token: &str,
        ) -> (StatusCode, Value) {
            let header = format!("Bearer {token}");
            let (status, body, _) = http_request(
                self.app.clone(),
                method,
                &self.url(rest),
                body.map(|body| body.to_string().into_bytes()),
                Some("application/json"),
                None,
                &[("authorization", header.as_str())],
            )
            .await;
            (status, body)
        }

        /// A new connection: readback never reuses the writer's pool.
        pub async fn fresh(&self) -> SqlitePool {
            pool::connect_sqlite_app(&self.path, 1).await.unwrap()
        }

        pub async fn project(&self, key: &str, visibility: &str) -> (Uuid, Uuid) {
            let (status, body) = self
                .call(
                    "POST",
                    "/projects",
                    Some(json!({"key": key, "name": key, "visibility": visibility})),
                    &self.owner,
                )
                .await;
            assert_eq!(status, StatusCode::CREATED, "{body}");
            (id_of(&body["id"]), id_of(&body["rootDocumentId"]))
        }

        pub async fn finish(self) {
            self.pool.close().await;
            drop(self.app);
            std::fs::remove_dir_all(self.root).unwrap();
        }
    }

    async fn insert_workspace(pool: &SqlitePool, slug: &str) -> Uuid {
        let ws = Uuid::now_v7();
        sqlx::query("INSERT INTO workspaces(id,slug,name) VALUES(?1,?2,?2)")
            .bind(blob(ws))
            .bind(slug)
            .execute(pool)
            .await
            .unwrap();
        ws
    }

    async fn insert_member(pool: &SqlitePool, ws: Uuid, role: &str, label: &str) -> Member {
        let id = Uuid::now_v7();
        let token = fvoci_server::auth::token::new_token();
        sqlx::query("INSERT INTO users(id,email,given_name) VALUES(?1,?2,?3)")
            .bind(blob(id))
            .bind(format!("{label}@example.test"))
            .bind(label)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO memberships(workspace_id,user_id,role) VALUES(?1,?2,?3)")
            .bind(blob(ws))
            .bind(blob(id))
            .bind(role)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO sessions(id,user_id,token_hash,expires_at) VALUES(?1,?2,?3,(unixepoch()+3600)*1000000)")
            .bind(blob(Uuid::now_v7()))
            .bind(blob(id))
            .bind(&token.hash)
            .execute(pool)
            .await
            .unwrap();
        Member {
            id,
            cookie: token.token,
        }
    }

    pub(super) async fn scalar(pool: &SqlitePool, sql: &str, ids: &[Uuid]) -> i64 {
        let mut query = sqlx::query_scalar::<_, i64>(sql);
        for id in ids {
            query = query.bind(blob(*id));
        }
        query.fetch_one(pool).await.unwrap()
    }

    pub(super) async fn text(pool: &SqlitePool, sql: &str, ids: &[Uuid]) -> Option<String> {
        let mut query = sqlx::query_scalar::<_, Option<String>>(sql);
        for id in ids {
            query = query.bind(blob(*id));
        }
        query.fetch_one(pool).await.unwrap()
    }

    /// `events` and `audit_log` rows of `verb` in the workspace.
    pub(super) async fn journal(pool: &SqlitePool, ws: Uuid, verb: &str) -> (i64, i64) {
        sqlx::query_as(
            "SELECT (SELECT count(*) FROM events WHERE workspace_id=?1 AND verb=?2),
                    (SELECT count(*) FROM audit_log WHERE workspace_id=?1 AND verb=?2)",
        )
        .bind(blob(ws))
        .bind(verb)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    pub(super) async fn member_roles(pool: &SqlitePool, project: Uuid) -> Vec<(Uuid, String)> {
        sqlx::query_as::<_, (Vec<u8>, String)>(
            "SELECT user_id,role FROM project_members WHERE project_id=?1 AND user_id IS NOT NULL ORDER BY user_id",
        )
        .bind(blob(project))
        .fetch_all(pool)
        .await
        .unwrap()
        .into_iter()
        .map(|(id, role)| (Uuid::from_slice(&id).unwrap(), role))
        .collect()
    }
}

/// PATCH project and the member routes on SQLite: success with a new-connection
/// readback and journal rows, then every refusal leaves the rows as they were.
#[tokio::test]
async fn sqlite_project_update_and_members_keep_the_postgres_contract() {
    use axum::http::StatusCode;
    use serde_json::json;
    use sqlite_port::{journal, member_roles, scalar, text, Lite};

    let lite = Lite::new().await;
    let (project, _root) = lite.project("LITE", "workspace").await;
    let p = format!("/projects/{project}");
    let alice = lite.member(lite.ws, "member", "ba").await;
    let zed = lite.member(lite.ws, "member", "b-z").await;
    let guest = lite.member(lite.ws, "guest", "guest").await;
    let other_ws = lite.workspace("elsewhere").await;
    let stranger = lite.member(other_ws, "owner", "stranger").await;

    let (status, body) = lite
        .call(
            "PATCH",
            &p,
            Some(json!({"name": "  Renamed  ", "description": "About", "icon": "R"})),
            &lite.owner,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["name"], "Renamed");
    assert_eq!(body["description"], "About");
    let fresh = lite.fresh().await;
    let stored = text(
        &fresh,
        "SELECT name||'|'||description||'|'||icon||'|'||visibility FROM projects WHERE id=?1",
        &[project],
    )
    .await;
    assert_eq!(stored.as_deref(), Some("Renamed|About|R|workspace"));
    assert_eq!(journal(&fresh, lite.ws, "project.updated").await, (1, 1));
    fresh.close().await;

    // NUL is a 400 before any write; Manage, the tenant and the credential are
    // checked before any change.
    for name in ["Bad\u{0}name", " "] {
        let (status, _) = lite
            .call("PATCH", &p, Some(json!({"name": name})), &lite.owner)
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{name:?}");
    }
    for who in [&alice, &guest, &stranger] {
        let (status, _) = lite
            .call("PATCH", &p, Some(json!({"name": "Taken"})), who)
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
    let fresh = lite.fresh().await;
    assert_eq!(
        text(&fresh, "SELECT name FROM projects WHERE id=?1", &[project])
            .await
            .as_deref(),
        Some("Renamed")
    );
    assert_eq!(journal(&fresh, lite.ws, "project.updated").await, (1, 1));
    fresh.close().await;

    // A projects.manage token is a credential like a session.
    let token = lite.token(&lite.owner, r#"["projects.manage"]"#).await;
    let (status, body) = lite
        .bearer("PATCH", &p, Some(json!({"icon": null})), &token)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["icon"].is_null());

    // Members: add, duplicate, non-member and guest-lead refusals.
    let members = format!("{p}/members");
    for (who, role) in [(&alice, "member"), (&zed, "viewer")] {
        let (status, body) = lite
            .call(
                "POST",
                &members,
                Some(json!({"userId": who.id, "role": role})),
                &lite.owner,
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }
    for (user, role) in [
        (alice.id, "viewer"),
        (stranger.id, "member"),
        (guest.id, "lead"),
    ] {
        let (status, body) = lite
            .call(
                "POST",
                &members,
                Some(json!({"userId": user, "role": role})),
                &lite.owner,
            )
            .await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
    }
    let (status, _) = lite
        .call(
            "POST",
            &members,
            Some(json!({"userId": guest.id, "role": "viewer"})),
            &alice,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Email order is bytewise ("b-z" sorts before "ba"), as COLLATE "C".
    let (status, list) = lite.call("GET", &members, None, &zed).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let emails: Vec<&str> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["email"].as_str().unwrap())
        .collect();
    assert_eq!(
        emails,
        ["b-z@example.test", "ba@example.test", "owner@example.test"]
    );
    assert_eq!(list["items"][0]["role"], "viewer");
    let (status, _) = lite.call("GET", &members, None, &stranger).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Two concurrent adds of one user: one grant, one conflict.
    let racer = lite.member(lite.ws, "member", "racer").await;
    let add = || {
        lite.call(
            "POST",
            &members,
            Some(json!({"userId": racer.id, "role": "member"})),
            &lite.owner,
        )
    };
    let (first, second) = tokio::join!(add(), add());
    let mut statuses = [first.0, second.0];
    statuses.sort();
    assert_eq!(statuses, [StatusCode::CREATED, StatusCode::CONFLICT]);

    // Role change and the private last-lead invariant.
    let (status, _) = lite
        .call(
            "PATCH",
            &format!("{members}/{}", alice.id),
            Some(json!({"role": "lead"})),
            &lite.owner,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = lite
        .call(
            "PATCH",
            &format!("{members}/{}", guest.id),
            Some(json!({"role": "member"})),
            &lite.owner,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) = lite
        .call(
            "PATCH",
            &p,
            Some(json!({"visibility": "private"})),
            &lite.owner,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, _) = lite
        .call(
            "DELETE",
            &format!("{members}/{}", alice.id),
            None,
            &lite.owner,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    for (method, body) in [("PATCH", Some(json!({"role": "member"}))), ("DELETE", None)] {
        let (status, refused) = lite
            .call(
                method,
                &format!("{members}/{}", lite.owner.id),
                body,
                &lite.owner,
            )
            .await;
        assert_eq!(status, StatusCode::CONFLICT, "{method} {refused}");
    }
    let fresh = lite.fresh().await;
    let mut expected = vec![
        (lite.owner.id, "lead".to_string()),
        (zed.id, "viewer".to_string()),
        (racer.id, "member".to_string()),
    ];
    expected.sort();
    assert_eq!(member_roles(&fresh, project).await, expected);
    assert_eq!(
        journal(&fresh, lite.ws, "project_member.added").await,
        (3, 3)
    );
    assert_eq!(
        journal(&fresh, lite.ws, "project_member.role_changed").await,
        (1, 1)
    );
    assert_eq!(
        journal(&fresh, lite.ws, "project_member.removed").await,
        (1, 1)
    );
    assert_eq!(
        text(
            &fresh,
            "SELECT visibility FROM projects WHERE id=?1",
            &[project]
        )
        .await
        .as_deref(),
        Some("private")
    );
    fresh.close().await;

    // A lead change promotes a direct member and demotes the other leads; a
    // guest or a non-member cannot be named lead.
    let (status, _) = lite
        .call(
            "PATCH",
            &p,
            Some(json!({"leadUserId": guest.id})),
            &lite.owner,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = lite
        .call(
            "PATCH",
            &p,
            Some(json!({"leadUserId": alice.id})),
            &lite.owner,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, body) = lite
        .call(
            "PATCH",
            &p,
            Some(json!({"leadUserId": racer.id})),
            &lite.owner,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let fresh = lite.fresh().await;
    let mut expected = vec![
        (lite.owner.id, "member".to_string()),
        (zed.id, "viewer".to_string()),
        (racer.id, "lead".to_string()),
    ];
    expected.sort();
    assert_eq!(member_roles(&fresh, project).await, expected);
    fresh.close().await;

    // A workspace project without a lead cannot become private.
    let (open, _) = lite.project("OPEN", "workspace").await;
    let (status, _) = lite
        .call(
            "DELETE",
            &format!("/projects/{open}/members/{}", lite.owner.id),
            None,
            &lite.owner,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = lite
        .call(
            "PATCH",
            &format!("/projects/{open}"),
            Some(json!({"visibility": "private"})),
            &lite.owner,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let fresh = lite.fresh().await;
    assert_eq!(
        text(
            &fresh,
            "SELECT visibility FROM projects WHERE id=?1",
            &[open]
        )
        .await
        .as_deref(),
        Some("workspace")
    );
    assert_eq!(
        scalar(
            &fresh,
            "SELECT count(*) FROM project_members WHERE project_id=?1",
            &[open]
        )
        .await,
        0
    );
    fresh.close().await;
    lite.finish().await;
}

/// One clone fixture, written once per backend: a label, a milestone and a
/// task through the API, then task-collection fields, options and views as
/// rows (live and deleted, shared and private).
mod clone_fixture {
    use super::project_harness::json_request;
    use axum::http::StatusCode;
    use serde_json::{json, Value};
    use uuid::Uuid;

    pub(super) type SeedField = (
        Uuid,
        &'static str,
        &'static str,
        Option<&'static str>,
        &'static str,
        &'static str,
        bool,
    );
    pub(super) type FieldRow = (Uuid, String, String, Option<String>, String, String);
    pub(super) type OptionRow = (Uuid, String, String, String, String);
    pub(super) type ViewRow = (String, String, String, Value, Uuid);
    pub(super) type StatusRow = (Uuid, String, String, String, Option<i64>);
    pub(super) type MilestoneRow = (Uuid, String, Option<String>, String);

    /// A project's copied configuration as read back.
    pub(super) struct Copied {
        pub collection: (String, String, i64),
        pub fields: Vec<FieldRow>,
        pub options: Vec<OptionRow>,
        pub views: Vec<ViewRow>,
        pub statuses: Vec<StatusRow>,
        pub labels: Vec<(Uuid, String, String)>,
        pub milestones: Vec<MilestoneRow>,
        pub items: i64,
    }

    pub(super) struct Seeded {
        pub label: Uuid,
        pub milestone: Uuid,
        pub status: Uuid,
        pub fields: Vec<SeedField>,
        pub options: Vec<(Uuid, Uuid, &'static str, &'static str, &'static str, bool)>,
        pub views: Vec<(Uuid, &'static str, &'static str, &'static str, Value)>,
    }

    pub(super) async fn api(
        app: &axum::Router,
        cookie: &str,
        ws: Uuid,
        project: Uuid,
    ) -> (Uuid, Uuid) {
        let base = format!("/api/v1/workspaces/{ws}/projects/{project}");
        let (status, label) = json_request(
            app.clone(),
            "POST",
            &format!("{base}/labels"),
            Some(json!({"name": "bug", "color": "red"})),
            Some(cookie),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{label}");
        let (status, milestone) = json_request(
            app.clone(),
            "POST",
            &format!("{base}/milestones"),
            Some(json!({"name": "M1", "dueDate": "2031-02-03"})),
            Some(cookie),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{milestone}");
        let (status, task) = json_request(
            app.clone(),
            "POST",
            &format!("{base}/tasks"),
            Some(json!({"title": "Seed task"})),
            Some(cookie),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{task}");
        (
            Uuid::parse_str(label["id"].as_str().unwrap()).unwrap(),
            Uuid::parse_str(milestone["id"].as_str().unwrap()).unwrap(),
        )
    }

    /// Rows to write; `status` is the source's first workflow status.
    pub(super) fn plan(label: Uuid, milestone: Uuid, status: Uuid) -> Seeded {
        let (notes, stage, gone) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        let (alpha, beta, zulu, gamma) = (
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
        );
        let board = json!({
            "query": {
                "filters": {
                    "labelId": label.to_string(),
                    "milestoneId": milestone.to_string(),
                    "statusId": status.to_string(),
                    "custom": [{"fieldId": stage.to_string(), "operator": "equals", "value": alpha.to_string()}],
                },
                "sort": [{"field": notes.to_string(), "direction": "asc"}],
            },
            "groupBy": stage.to_string(),
        });
        Seeded {
            label,
            milestone,
            status,
            fields: vec![
                (
                    notes,
                    "notes",
                    "Notes",
                    Some("Free text"),
                    "text",
                    "a",
                    false,
                ),
                (stage, "stage", "Stage", None, "select", "b", false),
                (gone, "gone", "Gone", None, "select", "c", true),
            ],
            options: vec![
                (alpha, stage, "alpha", "Alpha", "a", false),
                (beta, stage, "beta", "Beta", "b", false),
                (zulu, stage, "zulu", "Zulu", "c", true),
                (gamma, gone, "gamma", "Gamma", "a", false),
            ],
            views: vec![
                (Uuid::now_v7(), "shared", "Board", "board", board),
                (Uuid::now_v7(), "shared", "All", "table", json!({})),
                (Uuid::now_v7(), "private", "Mine", "table", json!({})),
            ],
        }
    }

    /// The copied configuration of `project`, ids replaced by what they name.
    pub(super) fn normalize(copied: Copied, owner: Uuid) -> Value {
        let Copied {
            collection,
            fields,
            options,
            views,
            statuses,
            labels,
            milestones,
            items,
        } = copied;
        let mut names = std::collections::HashMap::new();
        for (id, key, ..) in &fields {
            names.insert(id.to_string(), format!("field:{key}"));
        }
        for (id, field, key, ..) in &options {
            names.insert(id.to_string(), format!("option:{field}/{key}"));
        }
        for (id, name, ..) in &statuses {
            names.insert(id.to_string(), format!("status:{name}"));
        }
        for (id, name, _) in &labels {
            names.insert(id.to_string(), format!("label:{name}"));
        }
        for (id, name, ..) in &milestones {
            names.insert(id.to_string(), format!("milestone:{name}"));
        }
        fn rename(value: &Value, names: &std::collections::HashMap<String, String>) -> Value {
            match value {
                Value::String(raw) if Uuid::parse_str(raw).is_ok() => Value::String(
                    names
                        .get(raw)
                        .cloned()
                        .unwrap_or_else(|| "unmapped".to_string()),
                ),
                Value::Array(items) => {
                    Value::Array(items.iter().map(|item| rename(item, names)).collect())
                }
                Value::Object(map) => Value::Object(
                    map.iter()
                        .map(|(key, item)| (key.clone(), rename(item, names)))
                        .collect(),
                ),
                other => other.clone(),
            }
        }
        json!({
            "collection": {"kind": collection.0, "name": collection.1, "count": collection.2},
            "fields": fields.iter().map(|(_, key, name, description, kind, sort)| json!([key, name, description, kind, sort])).collect::<Vec<_>>(),
            "options": options.iter().map(|(_, field, key, label, sort)| json!([field, key, label, sort])).collect::<Vec<_>>(),
            "views": views.iter().map(|(visibility, name, kind, config, view_owner)| json!([visibility, name, kind, rename(config, &names), *view_owner == owner])).collect::<Vec<_>>(),
            "statuses": statuses.iter().map(|(_, name, category, sort, wip)| json!([name, category, sort, wip])).collect::<Vec<_>>(),
            "labels": labels.iter().map(|(_, name, color)| json!([name, color])).collect::<Vec<_>>(),
            "milestones": milestones.iter().map(|(_, name, due, sort)| json!([name, due, sort])).collect::<Vec<_>>(),
            "items": items,
        })
    }

    /// What the clone must hold, written down independently of both readers.
    pub(super) fn assert_expected(snapshot: &Value, source_statuses: &Value) {
        assert_eq!(
            snapshot["collection"],
            json!({"kind": "task", "name": "Copy", "count": 1})
        );
        assert_eq!(
            snapshot["fields"],
            json!([
                ["notes", "Notes", "Free text", "text", "a"],
                ["stage", "Stage", null, "select", "b"],
            ])
        );
        assert_eq!(
            snapshot["options"],
            json!([
                ["stage", "alpha", "Alpha", "a"],
                ["stage", "beta", "Beta", "b"]
            ])
        );
        let views = snapshot["views"].as_array().unwrap();
        assert_eq!(views.len(), 2, "private views stay behind: {views:?}");
        assert_eq!(
            json!([views[0][0], views[0][1], views[0][2]]),
            json!(["shared", "All", "table"])
        );
        assert_eq!(
            json!([views[1][0], views[1][1], views[1][2]]),
            json!(["shared", "Board", "board"])
        );
        assert_eq!(views[0][4], json!(true));
        let board = &views[1][3];
        assert_eq!(board["groupBy"], "field:stage");
        assert_eq!(board["query"]["filters"]["labelId"], "label:bug");
        assert_eq!(board["query"]["filters"]["milestoneId"], "milestone:M1");
        assert!(board["query"]["filters"]["statusId"]
            .as_str()
            .unwrap()
            .starts_with("status:"));
        assert_eq!(
            board["query"]["filters"]["custom"][0]["fieldId"],
            "field:stage"
        );
        assert_eq!(
            board["query"]["filters"]["custom"][0]["value"],
            "option:stage/alpha"
        );
        assert_eq!(board["query"]["sort"][0]["field"], "field:notes");
        assert_eq!(snapshot["labels"], json!([["bug", "red"]]));
        assert_eq!(
            snapshot["milestones"],
            json!([["M1", "2031-02-03", snapshot["milestones"][0][2]]])
        );
        assert_eq!(snapshot["statuses"], *source_statuses);
        assert_eq!(snapshot["items"], json!(0), "tasks are not cloned");
    }
}

async fn sqlite_seed_configuration(
    lite: &sqlite_port::Lite,
    project: uuid::Uuid,
) -> clone_fixture::Seeded {
    use sqlite_port::blob;
    let (label, milestone) =
        clone_fixture::api(&lite.app, &lite.owner.cookie, lite.ws, project).await;
    let status: Vec<u8> =
        sqlx::query_scalar("SELECT id FROM statuses WHERE project_id=?1 ORDER BY sort_key LIMIT 1")
            .bind(blob(project))
            .fetch_one(&lite.pool)
            .await
            .unwrap();
    let seeded = clone_fixture::plan(label, milestone, uuid::Uuid::from_slice(&status).unwrap());
    let collection: Vec<u8> =
        sqlx::query_scalar("SELECT id FROM collections WHERE project_id=?1 AND kind='task'")
            .bind(blob(project))
            .fetch_one(&lite.pool)
            .await
            .unwrap();
    for (id, key, name, description, kind, sort, deleted) in &seeded.fields {
        sqlx::query("INSERT INTO collection_fields(id,workspace_id,collection_id,key,name,description,type,sort_key,deleted_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,CASE WHEN ?9 THEN unixepoch()*1000000 END)")
            .bind(blob(*id)).bind(blob(lite.ws)).bind(&collection).bind(key).bind(name).bind(description).bind(kind).bind(sort).bind(deleted)
            .execute(&lite.pool).await.unwrap();
    }
    for (id, field, key, label, sort, deleted) in &seeded.options {
        sqlx::query("INSERT INTO collection_options(id,workspace_id,collection_id,field_id,key,label,sort_key,deleted_at) VALUES(?1,?2,?3,?4,?5,?6,?7,CASE WHEN ?8 THEN unixepoch()*1000000 END)")
            .bind(blob(*id)).bind(blob(lite.ws)).bind(&collection).bind(blob(*field)).bind(key).bind(label).bind(sort).bind(deleted)
            .execute(&lite.pool).await.unwrap();
    }
    for (id, visibility, name, kind, config) in &seeded.views {
        sqlx::query("INSERT INTO collection_views(id,workspace_id,collection_id,owner_id,visibility,name,type,config) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)")
            .bind(blob(*id)).bind(blob(lite.ws)).bind(&collection).bind(blob(lite.owner.id)).bind(visibility).bind(name).bind(kind).bind(config.to_string())
            .execute(&lite.pool).await.unwrap();
    }
    seeded
}

async fn sqlite_configuration_snapshot(
    pool: &sqlx::SqlitePool,
    project: uuid::Uuid,
    owner: uuid::Uuid,
) -> serde_json::Value {
    use sqlite_port::blob;
    use uuid::Uuid;
    let id = |bytes: Vec<u8>| Uuid::from_slice(&bytes).unwrap();
    let collections: Vec<(Vec<u8>, String, String)> =
        sqlx::query_as("SELECT id,kind,name FROM collections WHERE project_id=?1 ORDER BY id")
            .bind(blob(project))
            .fetch_all(pool)
            .await
            .unwrap();
    let collection = collections[0].0.clone();
    type RawField = (Vec<u8>, String, String, Option<String>, String, String);
    let fields: Vec<RawField> = sqlx::query_as(
        "SELECT id,key,name,description,type,sort_key FROM collection_fields WHERE collection_id=?1 ORDER BY sort_key,id",
    )
    .bind(&collection)
    .fetch_all(pool)
    .await
    .unwrap();
    let options: Vec<(Vec<u8>, String, String, String, String)> = sqlx::query_as(
        "SELECT o.id,f.key,o.key,o.label,o.sort_key FROM collection_options o JOIN collection_fields f ON f.id=o.field_id WHERE o.collection_id=?1 ORDER BY f.key,o.sort_key",
    )
    .bind(&collection)
    .fetch_all(pool)
    .await
    .unwrap();
    let views: Vec<(String, String, String, String, Vec<u8>)> = sqlx::query_as(
        "SELECT visibility,name,type,config,owner_id FROM collection_views WHERE collection_id=?1 ORDER BY name",
    )
    .bind(&collection)
    .fetch_all(pool)
    .await
    .unwrap();
    type RawStatus = (Vec<u8>, String, String, String, Option<i64>);
    let statuses: Vec<RawStatus> = sqlx::query_as(
        "SELECT id,name,category,sort_key,wip_limit FROM statuses WHERE project_id=?1 ORDER BY sort_key",
    )
    .bind(blob(project))
    .fetch_all(pool)
    .await
    .unwrap();
    let labels: Vec<(Vec<u8>, String, String)> =
        sqlx::query_as("SELECT id,name,color FROM labels WHERE project_id=?1 ORDER BY name")
            .bind(blob(project))
            .fetch_all(pool)
            .await
            .unwrap();
    let milestones: Vec<(Vec<u8>, String, Option<String>, String)> = sqlx::query_as(
        "SELECT id,name,due_date,sort_key FROM milestones WHERE project_id=?1 ORDER BY sort_key",
    )
    .bind(blob(project))
    .fetch_all(pool)
    .await
    .unwrap();
    let items: i64 =
        sqlx::query_scalar("SELECT count(*) FROM collection_items WHERE collection_id=?1")
            .bind(&collection)
            .fetch_one(pool)
            .await
            .unwrap();
    clone_fixture::normalize(
        clone_fixture::Copied {
            collection: (
                collections[0].1.clone(),
                collections[0].2.clone(),
                collections.len() as i64,
            ),
            fields: fields
                .into_iter()
                .map(|r| (id(r.0), r.1, r.2, r.3, r.4, r.5))
                .collect(),
            options: options
                .into_iter()
                .map(|r| (id(r.0), r.1, r.2, r.3, r.4))
                .collect(),
            views: views
                .into_iter()
                .map(|r| (r.0, r.1, r.2, serde_json::from_str(&r.3).unwrap(), id(r.4)))
                .collect(),
            statuses: statuses
                .into_iter()
                .map(|r| (id(r.0), r.1, r.2, r.3, r.4))
                .collect(),
            labels: labels.into_iter().map(|r| (id(r.0), r.1, r.2)).collect(),
            milestones: milestones
                .into_iter()
                .map(|r| (id(r.0), r.1, r.2, r.3))
                .collect(),
            items,
        },
        owner,
    )
}

async fn pg_configuration_snapshot(
    pool: &sqlx::PgPool,
    project: uuid::Uuid,
    owner: uuid::Uuid,
) -> serde_json::Value {
    use uuid::Uuid;
    let collections: Vec<(Uuid, String, String)> = sqlx::query_as(
        "SELECT id,kind,name FROM fvoci.collections WHERE project_id=$1 ORDER BY id",
    )
    .bind(project)
    .fetch_all(pool)
    .await
    .unwrap();
    let collection = collections[0].0;
    let fields: Vec<clone_fixture::FieldRow> = sqlx::query_as(
        r#"SELECT id,key,name,description,type,sort_key FROM fvoci.collection_fields WHERE collection_id=$1 ORDER BY sort_key COLLATE "C",id"#,
    )
    .bind(collection)
    .fetch_all(pool)
    .await
    .unwrap();
    let options: Vec<(Uuid, String, String, String, String)> = sqlx::query_as(
        r#"SELECT o.id,f.key,o.key,o.label,o.sort_key FROM fvoci.collection_options o JOIN fvoci.collection_fields f ON f.id=o.field_id WHERE o.collection_id=$1 ORDER BY f.key COLLATE "C",o.sort_key COLLATE "C""#,
    )
    .bind(collection)
    .fetch_all(pool)
    .await
    .unwrap();
    let views: Vec<(String, String, String, serde_json::Value, Uuid)> = sqlx::query_as(
        r#"SELECT visibility,name,type,config,owner_id FROM fvoci.collection_views WHERE collection_id=$1 ORDER BY name COLLATE "C""#,
    )
    .bind(collection)
    .fetch_all(pool)
    .await
    .unwrap();
    let statuses: Vec<(Uuid, String, String, String, Option<i32>)> = sqlx::query_as(
        r#"SELECT id,name,category,sort_key,wip_limit FROM fvoci.statuses WHERE project_id=$1 ORDER BY sort_key COLLATE "C""#,
    )
    .bind(project)
    .fetch_all(pool)
    .await
    .unwrap();
    let labels: Vec<(Uuid, String, String)> = sqlx::query_as(
        r#"SELECT id,name,color FROM fvoci.labels WHERE project_id=$1 ORDER BY name COLLATE "C""#,
    )
    .bind(project)
    .fetch_all(pool)
    .await
    .unwrap();
    let milestones: Vec<(Uuid, String, Option<String>, String)> = sqlx::query_as(
        r#"SELECT id,name,due_date::text,sort_key FROM fvoci.milestones WHERE project_id=$1 ORDER BY sort_key COLLATE "C""#,
    )
    .bind(project)
    .fetch_all(pool)
    .await
    .unwrap();
    let items: i64 =
        sqlx::query_scalar("SELECT count(*) FROM fvoci.collection_items WHERE collection_id=$1")
            .bind(collection)
            .fetch_one(pool)
            .await
            .unwrap();
    clone_fixture::normalize(
        clone_fixture::Copied {
            collection: (
                collections[0].1.clone(),
                collections[0].2.clone(),
                collections.len() as i64,
            ),
            fields,
            options,
            views,
            statuses: statuses
                .into_iter()
                .map(|r| (r.0, r.1, r.2, r.3, r.4.map(i64::from)))
                .collect(),
            labels,
            milestones,
            items,
        },
        owner,
    )
}

/// Clone on SQLite: configuration, collection and root in one write, then
/// the duplicate key, the uncopyable view and the access refusals leave no
/// project, collection or journal row behind.
#[tokio::test]
async fn sqlite_clone_copies_configuration_and_refuses_without_effects() {
    use axum::http::StatusCode;
    use serde_json::json;
    use sqlite_port::{blob, id_of, journal, member_roles, scalar, Lite};

    let lite = Lite::new().await;
    let (source, _) = lite.project("SRC", "private").await;
    let seeded = sqlite_seed_configuration(&lite, source).await;
    let viewer = lite.member(lite.ws, "member", "viewer").await;
    let (status, _) = lite
        .call(
            "POST",
            &format!("/projects/{source}/members"),
            Some(json!({"userId": viewer.id, "role": "viewer"})),
            &lite.owner,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let clone_path = format!("/projects/{source}/clone");

    let (status, body) = lite
        .call(
            "POST",
            &clone_path,
            Some(json!({"key": "CPY", "name": " Copy ", "description": "Cloned"})),
            &lite.owner,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["key"], "CPY");
    assert_eq!(body["name"], "Copy");
    assert_eq!(body["visibility"], "private");
    let copy = id_of(&body["id"]);
    let root = id_of(&body["rootDocumentId"]);

    let fresh = lite.fresh().await;
    let source_snapshot = sqlite_configuration_snapshot(&fresh, source, lite.owner.id).await;
    assert_eq!(
        source_snapshot["items"],
        json!(1),
        "the seeded task is an item"
    );
    let snapshot = sqlite_configuration_snapshot(&fresh, copy, lite.owner.id).await;
    clone_fixture::assert_expected(&snapshot, &source_snapshot["statuses"]);
    assert_eq!(
        member_roles(&fresh, copy).await,
        vec![(lite.owner.id, "lead".to_string())]
    );
    assert_eq!(
        scalar(
            &fresh,
            "SELECT count(*) FROM documents WHERE id=?1 AND project_id=?2 AND parent_id IS NULL AND title='Copy'",
            &[root, copy]
        )
        .await,
        1
    );
    let created: String = sqlx::query_scalar(
        "SELECT json_extract(payload,'$.sourceProjectId') FROM events WHERE verb='project.created' AND target_id=?1",
    )
    .bind(blob(copy))
    .fetch_one(&fresh)
    .await
    .unwrap();
    assert_eq!(created, source.to_string());
    assert_eq!(seeded.views.len(), 3);
    fresh.close().await;

    // Refusals: duplicate key, a viewer, another tenant, and a shared view
    // that sorts by a deleted field (the PostgreSQL clone rejects it).
    let stranger_ws = lite.workspace("strangers").await;
    let stranger = lite.member(stranger_ws, "owner", "stranger").await;
    let gone = seeded.fields[2].0;
    sqlx::query("INSERT INTO collection_views(id,workspace_id,collection_id,owner_id,visibility,name,type,config) SELECT ?1,workspace_id,id,?2,'shared','Broken','table',?3 FROM collections WHERE project_id=?4 AND kind='task'")
        .bind(blob(uuid::Uuid::now_v7()))
        .bind(blob(lite.owner.id))
        .bind(json!({"query": {"sort": [{"field": gone.to_string(), "direction": "asc"}]}}).to_string())
        .bind(blob(source))
        .execute(&lite.pool)
        .await
        .unwrap();
    for (who, key, expected) in [
        (&lite.owner, "CPY", StatusCode::CONFLICT),
        (&viewer, "VWR", StatusCode::NOT_FOUND),
        (&stranger, "STR", StatusCode::NOT_FOUND),
        (&lite.owner, "BRK", StatusCode::BAD_REQUEST),
    ] {
        let (status, body) = lite
            .call(
                "POST",
                &clone_path,
                Some(json!({"key": key, "name": key})),
                who,
            )
            .await;
        assert_eq!(status, expected, "{key}: {body}");
    }
    let fresh = lite.fresh().await;
    assert_eq!(
        scalar(&fresh, "SELECT count(*) FROM projects", &[]).await,
        2
    );
    assert_eq!(
        scalar(&fresh, "SELECT count(*) FROM collections", &[]).await,
        2
    );
    assert_eq!(
        scalar(&fresh, "SELECT count(*) FROM documents", &[]).await,
        2
    );
    assert_eq!(journal(&fresh, lite.ws, "project.created").await, (2, 2));
    fresh.close().await;
    lite.finish().await;
}

/// The same fixture cloned on PostgreSQL and on SQLite yields the same
/// collection, fields, options, shared views and items, each read back on a
/// new connection and checked against written-down constants.
#[tokio::test]
async fn pg_and_sqlite_clone_the_same_configuration() {
    use axum::http::StatusCode;
    use project_harness::{admin_pool, create_project, json_request, setup_session, TestDb};
    use serde_json::json;
    use sqlite_port::{id_of, Lite};

    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner, ws) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let source = id_of(&create_project(app.clone(), &cookie, ws, "SRC", "workspace").await["id"]);
    let (label, milestone) = clone_fixture::api(&app, &cookie, ws, source).await;
    let status: uuid::Uuid = sqlx::query_scalar(
        r#"SELECT id FROM fvoci.statuses WHERE project_id=$1 ORDER BY sort_key COLLATE "C" LIMIT 1"#,
    )
    .bind(source)
    .fetch_one(&admin)
    .await
    .unwrap();
    let seeded = clone_fixture::plan(label, milestone, status);
    let collection: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM fvoci.collections WHERE project_id=$1 AND kind='task'")
            .bind(source)
            .fetch_one(&admin)
            .await
            .unwrap();
    for (id, key, name, description, kind, sort, deleted) in &seeded.fields {
        sqlx::query("INSERT INTO fvoci.collection_fields(id,workspace_id,collection_id,key,name,description,type,sort_key,deleted_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,CASE WHEN $9 THEN now() END)")
            .bind(id).bind(ws).bind(collection).bind(key).bind(name).bind(description).bind(kind).bind(sort).bind(deleted)
            .execute(&admin).await.unwrap();
    }
    for (id, field, key, label, sort, deleted) in &seeded.options {
        sqlx::query("INSERT INTO fvoci.collection_options(id,workspace_id,collection_id,field_id,key,label,sort_key,deleted_at) VALUES($1,$2,$3,$4,$5,$6,$7,CASE WHEN $8 THEN now() END)")
            .bind(id).bind(ws).bind(collection).bind(field).bind(key).bind(label).bind(sort).bind(deleted)
            .execute(&admin).await.unwrap();
    }
    for (id, visibility, name, kind, config) in &seeded.views {
        sqlx::query("INSERT INTO fvoci.collection_views(id,workspace_id,collection_id,owner_id,visibility,name,type,config) VALUES($1,$2,$3,$4,$5,$6,$7,$8)")
            .bind(id).bind(ws).bind(collection).bind(owner).bind(visibility).bind(name).bind(kind).bind(config)
            .execute(&admin).await.unwrap();
    }
    let (status, body) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/projects/{source}/clone"),
        Some(json!({"key": "CPY", "name": "Copy"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let pg_copy = id_of(&body["id"]);
    admin.close().await;
    let reader = admin_pool(&harness).await;
    let pg_source = pg_configuration_snapshot(&reader, source, owner).await;
    let pg = pg_configuration_snapshot(&reader, pg_copy, owner).await;
    reader.close().await;

    let lite = Lite::new().await;
    let (lite_source, _) = lite.project("SRC", "workspace").await;
    sqlite_seed_configuration(&lite, lite_source).await;
    let (status, body) = lite
        .call(
            "POST",
            &format!("/projects/{lite_source}/clone"),
            Some(json!({"key": "CPY", "name": "Copy"})),
            &lite.owner,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let fresh = lite.fresh().await;
    let sqlite_source = sqlite_configuration_snapshot(&fresh, lite_source, lite.owner.id).await;
    let sqlite = sqlite_configuration_snapshot(&fresh, id_of(&body["id"]), lite.owner.id).await;
    fresh.close().await;

    assert_eq!(pg_source, sqlite_source);
    assert_eq!(pg, sqlite);
    clone_fixture::assert_expected(&pg, &pg_source["statuses"]);
    clone_fixture::assert_expected(&sqlite, &sqlite_source["statuses"]);
    lite.finish().await;
    harness.cleanup().await;
}

async fn sqlite_project_document(
    lite: &sqlite_port::Lite,
    project: uuid::Uuid,
    parent: uuid::Uuid,
    title: &str,
) -> uuid::Uuid {
    let (status, body) = lite
        .call(
            "POST",
            &format!("/projects/{project}/documents"),
            Some(serde_json::json!({"parentId": parent, "title": title})),
            &lite.owner,
        )
        .await;
    assert_eq!(status, axum::http::StatusCode::CREATED, "{body}");
    sqlite_port::id_of(&body["id"])
}

/// Project document move on SQLite: the whole subtree moves, then cycle,
/// root, depth, cross-project, trashed-parent and access refusals keep it.
#[tokio::test]
async fn sqlite_project_document_move_keeps_the_postgres_contract() {
    use axum::http::StatusCode;
    use serde_json::json;
    use sqlite_port::{journal, scalar, text, Lite};

    let lite = Lite::new().await;
    let (project, root) = lite.project("MOVE", "private").await;
    let (other, other_root) = lite.project("ELSE", "workspace").await;
    let a = sqlite_project_document(&lite, project, root, "A").await;
    let b = sqlite_project_document(&lite, project, a, "B").await;
    let b2 = sqlite_project_document(&lite, project, b, "B2").await;
    let c = sqlite_project_document(&lite, project, root, "C").await;
    let foreign = sqlite_project_document(&lite, other, other_root, "Foreign").await;
    let viewer = lite.member(lite.ws, "member", "viewer").await;
    let (status, _) = lite
        .call(
            "POST",
            &format!("/projects/{project}/members"),
            Some(json!({"userId": viewer.id, "role": "viewer"})),
            &lite.owner,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let stranger_ws = lite.workspace("strangers").await;
    let stranger = lite.member(stranger_ws, "owner", "stranger").await;
    let mv = |doc: uuid::Uuid| format!("/projects/{project}/documents/{doc}/move");
    let label = |id: uuid::Uuid| id.simple().to_string();

    let (status, body) = lite
        .call("POST", &mv(b), Some(json!({"newParentId": c})), &lite.owner)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["parentId"], c.to_string());
    assert_eq!(body["displayId"], "MOVE-3");
    let fresh = lite.fresh().await;
    let moved = format!("{}.{}.{}", label(root), label(c), label(b));
    assert_eq!(
        text(&fresh, "SELECT path FROM documents WHERE id=?1", &[b]).await,
        Some(moved.clone())
    );
    assert_eq!(
        text(&fresh, "SELECT path FROM documents WHERE id=?1", &[b2]).await,
        Some(format!("{moved}.{}", label(b2)))
    );
    assert_eq!(
        scalar(
            &fresh,
            "SELECT count(*) FROM documents WHERE id=?1 AND parent_id=?2",
            &[b, c]
        )
        .await,
        1
    );
    assert_eq!(journal(&fresh, lite.ws, "document.moved").await, (1, 1));
    fresh.close().await;

    let mut chain = c;
    for depth in 0..17 {
        chain = sqlite_project_document(&lite, project, chain, &format!("D{depth}")).await;
    }
    for (doc, parent, who, expected, code) in [
        (
            c,
            b2,
            &lite.owner,
            StatusCode::BAD_REQUEST,
            Some("document_cycle"),
        ),
        (
            c,
            c,
            &lite.owner,
            StatusCode::BAD_REQUEST,
            Some("document_cycle"),
        ),
        (
            root,
            a,
            &lite.owner,
            StatusCode::BAD_REQUEST,
            Some("root_document_move"),
        ),
        (
            b,
            chain,
            &lite.owner,
            StatusCode::BAD_REQUEST,
            Some("tree_depth_limit"),
        ),
        (b, foreign, &lite.owner, StatusCode::NOT_FOUND, None),
        (foreign, a, &lite.owner, StatusCode::NOT_FOUND, None),
        (b, a, &viewer, StatusCode::NOT_FOUND, None),
        (b, a, &stranger, StatusCode::NOT_FOUND, None),
    ] {
        let (status, body) = lite
            .call("POST", &mv(doc), Some(json!({"newParentId": parent})), who)
            .await;
        assert_eq!(status, expected, "{body}");
        if let Some(code) = code {
            assert_eq!(body["code"], code, "{body}");
        }
    }
    let (status, _) = lite
        .call(
            "POST",
            &format!("/projects/{project}/documents/{a}/trash"),
            None,
            &lite.owner,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = lite
        .call(
            "POST",
            &mv(b2),
            Some(json!({"newParentId": a})),
            &lite.owner,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) = lite
        .call(
            "POST",
            &mv(b2),
            Some(json!({"newParentId": chain})),
            &lite.owner,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "a leaf fits at the limit: {body}");

    let fresh = lite.fresh().await;
    assert_eq!(
        text(&fresh, "SELECT path FROM documents WHERE id=?1", &[b]).await,
        Some(moved)
    );
    assert_eq!(
        scalar(
            &fresh,
            "SELECT count(*) FROM documents WHERE id=?1 AND parent_id=?2",
            &[b2, chain]
        )
        .await,
        1
    );
    assert_eq!(journal(&fresh, lite.ws, "document.moved").await, (2, 2));
    fresh.close().await;
    lite.finish().await;
}

/// Saved views on SQLite: owner-only rows validated against the project,
/// listed in the PostgreSQL name order, with the expected-config CAS.
#[tokio::test]
async fn sqlite_project_views_keep_the_postgres_contract() {
    use axum::http::StatusCode;
    use serde_json::json;
    use sqlite_port::{id_of, scalar, text, Lite};

    let lite = Lite::new().await;
    let (project, _) = lite.project("VIEW", "workspace").await;
    let (other, _) = lite.project("ELSE", "workspace").await;
    let (hidden, _) = lite.project("HIDE", "private").await;
    let (label, milestone) =
        clone_fixture::api(&lite.app, &lite.owner.cookie, lite.ws, project).await;
    let (foreign_label, _) =
        clone_fixture::api(&lite.app, &lite.owner.cookie, lite.ws, other).await;
    let status: Vec<u8> =
        sqlx::query_scalar("SELECT id FROM statuses WHERE project_id=?1 ORDER BY sort_key LIMIT 1")
            .bind(sqlite_port::blob(project))
            .fetch_one(&lite.pool)
            .await
            .unwrap();
    let status = uuid::Uuid::from_slice(&status).unwrap();
    let mate = lite.member(lite.ws, "member", "mate").await;
    let stranger_ws = lite.workspace("strangers").await;
    let stranger = lite.member(stranger_ws, "owner", "stranger").await;
    let views = format!("/projects/{project}/views");
    let filtered = json!({"filters": {"statusId": status, "labelId": label, "milestoneId": milestone, "assigneeId": mate.id}, "sort": [{"field": "priority", "direction": "desc"}]});

    let (status_code, zeta) = lite
        .call(
            "POST",
            &views,
            Some(json!({"name": "Zeta", "type": "list", "config": filtered})),
            &lite.owner,
        )
        .await;
    assert_eq!(status_code, StatusCode::CREATED, "{zeta}");
    assert_eq!(zeta["config"]["filters"]["labelId"], label.to_string());
    let zeta_id = id_of(&zeta["id"]);
    let (status_code, alpha) = lite
        .call(
            "POST",
            &views,
            Some(json!({"name": "alpha", "type": "board", "config": {}})),
            &lite.owner,
        )
        .await;
    assert_eq!(status_code, StatusCode::CREATED, "{alpha}");
    let alpha_id = id_of(&alpha["id"]);

    for config in [
        json!({"filters": {"labelId": foreign_label}}),
        json!({"filters": {"statusId": uuid::Uuid::now_v7()}}),
        json!({"filters": {"assigneeId": stranger.id}}),
        json!({"filters": {"custom": [{"fieldId": uuid::Uuid::now_v7(), "operator": "empty"}]}}),
    ] {
        let (status_code, body) = lite
            .call(
                "POST",
                &views,
                Some(json!({"name": "Bad", "type": "list", "config": config})),
                &lite.owner,
            )
            .await;
        assert_eq!(status_code, StatusCode::BAD_REQUEST, "{config}: {body}");
    }
    for (who, path) in [
        (&stranger, views.clone()),
        (&mate, format!("/projects/{hidden}/views")),
    ] {
        let (status_code, _) = lite
            .call(
                "POST",
                &path,
                Some(json!({"name": "Nope", "type": "list", "config": {}})),
                who,
            )
            .await;
        assert_eq!(status_code, StatusCode::NOT_FOUND);
        let (status_code, _) = lite.call("GET", &path, None, who).await;
        assert_eq!(status_code, StatusCode::NOT_FOUND);
    }

    // PostgreSQL orders names by its collation: "alpha" before "Zeta".
    let (status_code, list) = lite.call("GET", &views, None, &lite.owner).await;
    assert_eq!(status_code, StatusCode::OK, "{list}");
    let names: Vec<&str> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["alpha", "Zeta"]);
    let (status_code, mine) = lite.call("GET", &views, None, &mate).await;
    assert_eq!(status_code, StatusCode::OK);
    assert_eq!(mine["items"], json!([]));

    let view = |id: uuid::Uuid| format!("/views/{id}");
    let (status_code, _) = lite
        .call(
            "PATCH",
            &view(zeta_id),
            Some(json!({"name": "Theirs"})),
            &mate,
        )
        .await;
    assert_eq!(status_code, StatusCode::NOT_FOUND);
    let (status_code, body) = lite
        .call(
            "PATCH",
            &view(zeta_id),
            Some(json!({"name": "Omega", "config": {"filters": {"openOnly": true}}, "expectedConfig": filtered})),
            &lite.owner,
        )
        .await;
    assert_eq!(status_code, StatusCode::OK, "{body}");
    let (status_code, body) = lite
        .call(
            "PATCH",
            &view(zeta_id),
            Some(json!({"config": {}, "expectedConfig": filtered})),
            &lite.owner,
        )
        .await;
    assert_eq!(status_code, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "document_version_mismatch");
    let (status_code, _) = lite
        .call(
            "PATCH",
            &view(zeta_id),
            Some(json!({"config": {"filters": {"labelId": foreign_label}}, "expectedConfig": {"filters": {"openOnly": true}}})),
            &lite.owner,
        )
        .await;
    assert_eq!(status_code, StatusCode::BAD_REQUEST);

    let fresh = lite.fresh().await;
    assert_eq!(
        text(
            &fresh,
            "SELECT name||'|'||json_extract(config,'$.filters.openOnly') FROM views WHERE id=?1",
            &[zeta_id]
        )
        .await
        .as_deref(),
        Some("Omega|1")
    );
    fresh.close().await;

    for (who, expected) in [
        (&mate, StatusCode::NOT_FOUND),
        (&stranger, StatusCode::NOT_FOUND),
        (&lite.owner, StatusCode::OK),
        (&lite.owner, StatusCode::NOT_FOUND),
    ] {
        let (status_code, _) = lite.call("DELETE", &view(alpha_id), None, who).await;
        assert_eq!(status_code, expected);
    }
    let fresh = lite.fresh().await;
    assert_eq!(
        scalar(
            &fresh,
            "SELECT count(*) FROM views WHERE project_id=?1",
            &[project]
        )
        .await,
        1
    );
    fresh.close().await;
    lite.finish().await;
}
