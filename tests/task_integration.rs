#![cfg(feature = "db-tests")]
#![allow(dead_code)]

#[path = "support/project_harness.rs"]
mod project_harness;

use std::collections::HashSet;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use futures_util::StreamExt;
use fvoci_server::db::pool;
use fvoci_server::http::routes::streams::{
    reset_task_stream_task_hint_enqueue_count, task_stream_task_hint_enqueue_count,
};
use fvoci_server::streams::{
    initial_cursor, poll_access_events, poll_task_events, EventCursor, EventPage, StreamHub,
};
use project_harness::{
    add_workspace_user, admin_pool, app_state, count_rows, create_project,
    drop_insert_fail_trigger, insert_minimal_project, insert_project_document,
    install_insert_fail_trigger, json_request, session_id_for_user, setup_session, test_peer,
    wait_for_query_blocked_by, wait_for_user_for_update_blocked, TestDb,
};
use serde_json::json;
use tokio::time::timeout;
use tower::ServiceExt;
use url::form_urlencoded;
use uuid::Uuid;

/// `pg_stat_activity` row reported when the snapshot xmin fails to settle:
/// (datname, pid, backend_xid, backend_xmin).
type XidHolder = (Option<String>, i32, Option<String>, Option<String>);

async fn json_request_bearer(
    app: axum::Router,
    method: &str,
    path: &str,
    bearer: &str,
) -> (StatusCode, serde_json::Value) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("origin", "http://localhost")
        .header("authorization", format!("Bearer {bearer}"))
        .body(Body::empty())
        .unwrap();
    let mut request = request;
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(project_harness::test_peer()));
    let response = app.oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap_or_default();
    let json = if bytes.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&bytes).unwrap_or(json!({}))
    };
    (status, json)
}

async fn insert_other_workspace(admin: &sqlx::PgPool) -> Uuid {
    let id = Uuid::now_v7();
    let slug = format!("w-{}", &id.simple().to_string()[20..]);
    sqlx::query("INSERT INTO fvoci.workspaces (id, slug, name) VALUES ($1, $2, 'Other')")
        .bind(id)
        .bind(slug)
        .execute(admin)
        .await
        .unwrap();
    id
}

#[tokio::test]
async fn contract_task_create_read_and_counts() {
    let harness = TestDb::bootstrap().await;
    let (app, _cookie, _, workspace_id) = setup_session(&harness).await;
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
    let (status, workflow) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/workflow"),
        None,
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let backlog_id = workflow["statuses"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["category"] == "backlog")
        .unwrap()["id"]
        .as_str()
        .unwrap();

    let (status, task) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({"title":"첫 일"})),
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(task["type"], "task");
    assert_eq!(task["priority"], "none");
    assert!(task["parentId"].is_null());
    assert_eq!(task["number"], 2);
    assert_eq!(task["statusId"], backlog_id);

    let task_id = task["id"].as_str().unwrap();
    let (status, detail) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}"),
        None,
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["canEdit"], true);
    assert!(detail["estimate"].is_null());
    assert!(detail["recurrence"].is_null());
    assert_eq!(detail["contentJson"]["type"], "doc");
    assert!(detail["assigneeIds"].as_array().unwrap().is_empty());
    assert!(detail["labelIds"].as_array().unwrap().is_empty());
    assert!(detail["children"].as_array().unwrap().is_empty());
    assert!(detail["parent"].is_null());
    assert_eq!(detail["childProgress"], json!({"done": 0, "total": 0}));
    assert!(detail["dependencies"].as_array().unwrap().is_empty());

    let (status, list) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects"),
        None,
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["items"][0]["taskCount"], 1);
    assert_eq!(list["items"][0]["openTaskCount"], 1);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn private_project_viewer_can_read_but_not_create_task() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, workspace_id, "member", "member").await;
    let hid = create_project(app.clone(), &member.cookie, workspace_id, "HID", "private").await;
    let project_id = hid["id"].as_str().unwrap();

    json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/members"),
        Some(json!({"userId": owner_id.to_string(), "role":"viewer"})),
        Some(&member.cookie),
    )
    .await;

    let (status, task) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({"title":"Lead task"})),
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let task_id = task["id"].as_str().unwrap();

    let (status, detail) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}"),
        None,
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["canEdit"], false);

    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({"title":"Viewer cannot"})),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_counts_exclude_deleted_and_archived_rows() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = Uuid::parse_str(lab["id"].as_str().unwrap()).unwrap();
    let (status, task) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({"title":"Live"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let task_id = Uuid::parse_str(task["id"].as_str().unwrap()).unwrap();
    let status_id = Uuid::parse_str(task["statusId"].as_str().unwrap()).unwrap();

    sqlx::query(
        r#"
        INSERT INTO fvoci.tasks (
            id, workspace_id, project_id, number, title, type, priority, status_id,
            content_json, created_by, deleted_at
        ) VALUES (
            $1, $2, $3, 99, 'Trashed', 'task', 'none', $4, '{"type":"doc","content":[{"type":"paragraph"}]}'::jsonb, $5, now()
        )
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(workspace_id)
    .bind(project_id)
    .bind(status_id)
    .bind(owner_id)
    .execute(&admin)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO fvoci.tasks (
            id, workspace_id, project_id, number, title, type, priority, status_id,
            content_json, created_by, archived_at
        ) VALUES (
            $1, $2, $3, 100, 'Archived', 'task', 'none', $4, '{"type":"doc","content":[{"type":"paragraph"}]}'::jsonb, $5, now()
        )
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(workspace_id)
    .bind(project_id)
    .bind(status_id)
    .bind(owner_id)
    .execute(&admin)
    .await
    .unwrap();

    let (status, list) = json_request(
        app,
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["items"][0]["taskCount"], 1);
    assert_eq!(list["items"][0]["openTaskCount"], 1);
    let _ = task_id;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn visibility_private_blocks_non_member_task_create_after_patch() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lead = add_workspace_user(&admin, workspace_id, "member", "lead").await;
    let other = add_workspace_user(&admin, workspace_id, "member", "other").await;
    let lab = create_project(app.clone(), &lead.cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();

    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({"title":"Visible era"})),
        Some(&other.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, _) = json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}"),
        Some(json!({"visibility":"private"})),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = json_request(
        app,
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({"title":"After private"})),
        Some(&other.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_status_fk_rejects_cross_project_status() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let ops = create_project(app.clone(), &cookie, workspace_id, "OPS", "workspace").await;
    let lab_id = lab["id"].as_str().unwrap();
    let ops_id = ops["id"].as_str().unwrap();
    let (status, ops_workflow) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{ops_id}/workflow"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let foreign_status = ops_workflow["statuses"][0]["id"].as_str().unwrap();

    let (status, body) = json_request(
        app,
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{lab_id}/tasks"),
        Some(json!({"title":"Bad status", "statusId": foreign_status})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "status_not_in_project_workflow");
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn private_task_hidden_from_non_member_get() {
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
        Some(json!({"title":"Secret"})),
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let task_id = task["id"].as_str().unwrap();

    let (status, _) = json_request(
        app,
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}"),
        None,
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn session_revoke_barrier_blocks_task_create() {
    let harness = TestDb::bootstrap().await;
    let (app, _, _, workspace_id) = setup_session(&harness).await;
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
    let events_before = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM fvoci.events")
        .fetch_one(&admin)
        .await
        .unwrap();

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
    let path = format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks");
    let create_task = tokio::spawn(async move {
        json_request(
            app_bg,
            "POST",
            &path,
            Some(json!({"title":"Race"})),
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
    let (status, _) = tokio::time::timeout(Duration::from_secs(10), create_task)
        .await
        .expect("task create finished")
        .expect("join");
    assert!(status == StatusCode::UNAUTHORIZED || status == StatusCode::NOT_FOUND);
    let events_after = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM fvoci.events")
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(events_after, events_before);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn real_project_document_fixture_used_for_collab_denial() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let project_id = Uuid::now_v7();
    let doc_id = Uuid::now_v7();
    insert_minimal_project(&admin, workspace_id, project_id, "PRJ", owner_id, "private").await;
    insert_project_document(&admin, workspace_id, project_id, doc_id, owner_id, 1).await;

    let (status, _) = json_request(
        app,
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{doc_id}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_detail_returns_parent_children_and_child_progress() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();

    let (status, parent) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({"title":"Parent", "type":"task"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let parent_id = parent["id"].as_str().unwrap();

    let (status, child) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({"title":"Child", "type":"subtask", "parentId": parent_id})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let child_id = child["id"].as_str().unwrap();
    assert_eq!(child["parentId"], parent_id);

    let (status, parent_detail) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{parent_id}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        parent_detail["childProgress"],
        json!({"done": 0, "total": 1})
    );
    let children = parent_detail["children"].as_array().unwrap();
    assert_eq!(children.len(), 1);
    assert_eq!(children[0]["id"], child_id);
    assert_eq!(children[0]["title"], "Child");
    assert_eq!(children[0]["type"], "subtask");

    let (status, child_detail) = json_request(
        app,
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{child_id}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(child_detail["childProgress"].is_null());
    assert_eq!(child_detail["parent"]["id"], parent_id);
    assert_eq!(child_detail["parent"]["title"], "Parent");

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_create_recurrence_round_trips_on_get() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();

    let (status, created) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({"title":"Repeat", "recurrence": {"kind":"weekly"}})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(created["recurrence"], json!({"kind":"weekly"}));
    let task_id = created["id"].as_str().unwrap();

    let (status, detail) = json_request(
        app,
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["recurrence"], json!({"kind":"weekly"}));

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_create_rejects_explicit_null_optional_fields() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let path = format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks");

    for (field, body) in [
        ("parentId", json!({"title":"Bad", "parentId": null})),
        ("statusId", json!({"title":"Bad", "statusId": null})),
        ("startDate", json!({"title":"Bad", "startDate": null})),
        ("dueDate", json!({"title":"Bad", "dueDate": null})),
        ("milestoneId", json!({"title":"Bad", "milestoneId": null})),
        ("recurrence", json!({"title":"Bad", "recurrence": null})),
    ] {
        let (status, problem) =
            json_request(app.clone(), "POST", &path, Some(body), Some(&cookie)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "field {field}");
        assert_eq!(problem["code"], "invalid_input", "field {field}");
    }

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_create_rejects_unknown_milestone_and_recurrence_blob() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let path = format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks");

    let foreign_milestone = Uuid::now_v7();
    let (status, problem) = json_request(
        app.clone(),
        "POST",
        &path,
        Some(json!({"title":"Bad", "milestoneId": foreign_milestone.to_string()})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(problem["code"], "not_found");

    let (status, problem) = json_request(
        app,
        "POST",
        &path,
        Some(json!({"title":"Bad", "recurrence": {"kind":"yearly"}})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(problem["code"], "invalid_input");

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn concurrent_visibility_private_vs_task_create_under_project_lock() {
    let harness = TestDb::bootstrap().await;
    let (app, _owner_cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lead = add_workspace_user(&admin, workspace_id, "member", "lead").await;
    let other = add_workspace_user(&admin, workspace_id, "member", "other").await;
    let lab = create_project(app.clone(), &lead.cookie, workspace_id, "LAB", "workspace").await;
    let project_id = Uuid::parse_str(lab["id"].as_str().unwrap()).unwrap();
    let tasks_before = count_rows(&admin, "tasks").await;
    let events_before = count_rows(&admin, "events").await;

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

    let patch = tokio::spawn({
        let app = app.clone();
        let lead_cookie = lead.cookie.clone();
        async move {
            json_request(
                app,
                "PATCH",
                &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}"),
                Some(json!({"visibility":"private"})),
                Some(&lead_cookie),
            )
            .await
        }
    });
    let create = tokio::spawn({
        let app = app.clone();
        let cookie = other.cookie.clone();
        async move {
            json_request(
                app,
                "POST",
                &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
                Some(json!({"title":"Race task"})),
                Some(&cookie),
            )
            .await
        }
    });

    let _ = wait_for_query_blocked_by(&admin, blocker_pid, "%fvoci.projects%").await;
    sqlx::query(
        "UPDATE fvoci.projects SET visibility = 'private', updated_at = now() WHERE workspace_id = $1 AND id = $2",
    )
    .bind(workspace_id)
    .bind(project_id)
    .execute(&mut *barrier)
    .await
    .unwrap();
    barrier.commit().await.unwrap();

    let (patch_status, _) = tokio::time::timeout(Duration::from_secs(10), patch)
        .await
        .expect("patch finished")
        .expect("join");
    let (create_status, _) = tokio::time::timeout(Duration::from_secs(10), create)
        .await
        .expect("create finished")
        .expect("join");
    assert_eq!(patch_status, StatusCode::OK);
    assert_eq!(create_status, StatusCode::NOT_FOUND);
    assert_eq!(count_rows(&admin, "tasks").await, tasks_before);
    assert_eq!(count_rows(&admin, "events").await, events_before + 1);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_create_audit_failure_rolls_back_all_state() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    install_insert_fail_trigger(&admin, "audit_log", "test_task_audit_fail").await;
    let tasks_before = count_rows(&admin, "tasks").await;
    let events_before = count_rows(&admin, "events").await;

    let (status, _) = json_request(
        app,
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({"title":"Audit blocked"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(count_rows(&admin, "tasks").await, tasks_before);
    assert_eq!(count_rows(&admin, "events").await, events_before);

    drop_insert_fail_trigger(&admin, "audit_log", "test_task_audit_fail").await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn suspended_user_task_create_denied_after_auth() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    sqlx::query("UPDATE fvoci.users SET suspended_at = now() WHERE id = $1")
        .bind(owner_id)
        .execute(&admin)
        .await
        .unwrap();
    let (status, body) = json_request(
        app,
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({"title":"After suspend"})),
        Some(&cookie),
    )
    .await;
    assert!(status == StatusCode::UNAUTHORIZED || status == StatusCode::NOT_FOUND);
    let _ = body;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn contract_task_meta_fields_workflow_and_list_pagination() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();

    let (status, workflow) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/workflow"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let status_row = &workflow["statuses"].as_array().unwrap()[0];
    assert!(status_row.get("workflowId").is_some());
    assert!(status_row["wipLimit"].is_null());

    for title in ["One", "Two", "Three"] {
        let (status, _) = json_request(
            app.clone(),
            "POST",
            &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
            Some(json!({"title": title})),
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
    }

    let list_query = r#"{"sort":[{"field":"number","direction":"asc"}]}"#;
    let encoded_query = form_urlencoded::byte_serialize(list_query.as_bytes()).collect::<String>();

    let (status, page1) = json_request(
        app.clone(),
        "GET",
        &format!(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks?limit=2&query={encoded_query}"
        ),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page1["items"].as_array().unwrap().len(), 2);
    assert!(page1["nextCursor"].is_string());
    assert!(!page1["statusCounts"].as_array().unwrap().is_empty());
    let first = &page1["items"][0];
    assert!(first.get("sortKey").is_some());
    assert_eq!(first["schemaVersion"], 2);
    assert_eq!(first["version"], 1);
    assert!(first["assigneeIds"].as_array().unwrap().is_empty());
    assert!(first["labelIds"].as_array().unwrap().is_empty());

    let cursor = page1["nextCursor"].as_str().unwrap();
    let encoded_cursor = form_urlencoded::byte_serialize(cursor.as_bytes()).collect::<String>();
    let (status, page2) = json_request(
        app,
        "GET",
        &format!(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks?limit=2&query={encoded_query}&cursor={encoded_cursor}"
        ),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page2["items"].as_array().unwrap().len(), 1);
    assert!(page2["nextCursor"].is_null());

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn contract_task_hierarchy_rejects_invalid_parents() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let path = format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks");

    let (status, task) = json_request(
        app.clone(),
        "POST",
        &path,
        Some(json!({"title":"Parent task", "type":"task"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let task_id = task["id"].as_str().unwrap();

    let (status, story) = json_request(
        app.clone(),
        "POST",
        &path,
        Some(json!({"title":"Story", "type":"story"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let story_id = story["id"].as_str().unwrap();

    let (status, body) = json_request(
        app.clone(),
        "POST",
        &path,
        Some(json!({"title":"Bad child", "type":"task", "parentId": task_id})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "task_hierarchy_violation");

    let (status, subtask) = json_request(
        app.clone(),
        "POST",
        &path,
        Some(json!({"title":"Sub", "type":"subtask", "parentId": story_id})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let subtask_id = subtask["id"].as_str().unwrap();

    let (status, body) = json_request(
        app.clone(),
        "POST",
        &path,
        Some(json!({"title":"Nested sub", "type":"subtask", "parentId": subtask_id})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "task_hierarchy_violation");

    let (status, body) = json_request(
        app.clone(),
        "POST",
        &path,
        Some(json!({"title":"Epic child", "type":"epic", "parentId": story_id})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "task_hierarchy_violation");

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn contract_task_create_includes_bug_type() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let (status, task) = json_request(
        app,
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({"title":"Bug", "type":"bug"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(task["type"], "bug");
    admin.close().await;
    harness.cleanup().await;
}

async fn list_tasks_page(
    app: axum::Router,
    workspace_id: Uuid,
    project_id: &str,
    cookie: &str,
    query: &str,
    limit: i32,
    cursor: Option<&str>,
) -> (StatusCode, serde_json::Value) {
    let encoded_query = form_urlencoded::byte_serialize(query.as_bytes()).collect::<String>();
    let path = if let Some(cursor) = cursor {
        let encoded_cursor = form_urlencoded::byte_serialize(cursor.as_bytes()).collect::<String>();
        format!(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks?limit={limit}&query={encoded_query}&cursor={encoded_cursor}"
        )
    } else {
        format!(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks?limit={limit}&query={encoded_query}"
        )
    };
    json_request(app, "GET", &path, None, Some(cookie)).await
}

async fn walk_task_list_ids(
    app: axum::Router,
    workspace_id: Uuid,
    project_id: &str,
    cookie: &str,
    query: &str,
    limit: i32,
) -> Vec<String> {
    let mut ids = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let (status, page) = list_tasks_page(
            app.clone(),
            workspace_id,
            project_id,
            cookie,
            query,
            limit,
            cursor.as_deref(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{page:?}");
        for item in page["items"].as_array().unwrap() {
            ids.push(item["id"].as_str().unwrap().to_string());
        }
        cursor = page["nextCursor"].as_str().map(str::to_string);
        if cursor.is_none() {
            break;
        }
    }
    ids
}

#[tokio::test]
async fn task_list_pagination_walks_all_pages_without_duplicates() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    for (title, priority) in [
        ("Alpha", "high"),
        ("Beta", "high"),
        ("Gamma", "medium"),
        ("Delta", "medium"),
        ("Epsilon", "low"),
    ] {
        let (status, _) = json_request(
            app.clone(),
            "POST",
            &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
            Some(json!({"title": title, "priority": priority})),
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
    }

    for query in [
        r#"{"sort":[{"field":"number","direction":"asc"}]}"#,
        r#"{"sort":[{"field":"priority","direction":"desc"},{"field":"number","direction":"asc"}]}"#,
    ] {
        let ids =
            walk_task_list_ids(app.clone(), workspace_id, project_id, &cookie, query, 2).await;
        assert_eq!(ids.len(), 5, "query {query}");
        assert_eq!(
            ids.len(),
            ids.iter().collect::<HashSet<_>>().len(),
            "query {query}"
        );
    }

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_list_as_of_cursor_excludes_late_created_tasks() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let query = r#"{"sort":[{"field":"number","direction":"asc"}]}"#;
    for title in ["One", "Two", "Three", "Four"] {
        let (status, _) = json_request(
            app.clone(),
            "POST",
            &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
            Some(json!({"title": title})),
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
    }

    let (status, page1) = list_tasks_page(
        app.clone(),
        workspace_id,
        project_id,
        &cookie,
        query,
        2,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let page1_counts = page1["statusCounts"].clone();
    let cursor = page1["nextCursor"].as_str().unwrap();

    let (status, late) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({"title":"Late"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let late_id = late["id"].as_str().unwrap();

    let (status, page2) = list_tasks_page(
        app.clone(),
        workspace_id,
        project_id,
        &cookie,
        query,
        2,
        Some(cursor),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page2["statusCounts"], page1_counts);
    let page2_ids: Vec<&str> = page2["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect();
    assert!(!page2_ids.contains(&late_id));
    assert_eq!(page2_ids.len(), 2);
    let page1_ids: Vec<String> = page1["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap().to_string())
        .collect();
    let all_ids: Vec<String> = page1_ids
        .into_iter()
        .chain(page2_ids.iter().map(|id| id.to_string()))
        .collect();
    assert_eq!(all_ids.len(), 4);
    assert_eq!(all_ids.len(), all_ids.iter().collect::<HashSet<_>>().len());
    assert!(!all_ids.iter().any(|id| id == late_id));

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_list_status_counts_honor_active_filters() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    for (title, task_type) in [
        ("Bug one", "bug"),
        ("Bug two", "bug"),
        ("Plain task", "task"),
    ] {
        let (status, _) = json_request(
            app.clone(),
            "POST",
            &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
            Some(json!({"title": title, "type": task_type})),
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
    }

    let query = r#"{"filters":{"type":"bug"},"sort":[{"field":"number","direction":"asc"}]}"#;
    let (status, page) = list_tasks_page(
        app.clone(),
        workspace_id,
        project_id,
        &cookie,
        query,
        50,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["items"].as_array().unwrap().len(), 2);
    let total: i64 = page["statusCounts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["count"].as_i64().unwrap())
        .sum();
    assert_eq!(total, 2);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_list_rejects_unknown_query_params_and_bad_filters() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let base = format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks");

    let (status, body) = json_request(
        app.clone(),
        "GET",
        &format!("{base}?unknown=1"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "invalid_input");

    let bad_query =
        form_urlencoded::byte_serialize(r#"{"filters":{"type":"milestone"}}"#.as_bytes())
            .collect::<String>();
    let (status, body) = json_request(
        app.clone(),
        "GET",
        &format!("{base}?query={bad_query}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "invalid_input");

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_list_rejects_cursor_with_different_filters() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    for title in ["A", "B", "C"] {
        let (status, _) = json_request(
            app.clone(),
            "POST",
            &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
            Some(json!({"title": title})),
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
    }

    let query_a = r#"{"sort":[{"field":"number","direction":"asc"}]}"#;
    let (status, page1) = list_tasks_page(
        app.clone(),
        workspace_id,
        project_id,
        &cookie,
        query_a,
        1,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let cursor = page1["nextCursor"].as_str().unwrap();

    let query_b = r#"{"sort":[{"field":"number","direction":"desc"}]}"#;
    let (status, body) = list_tasks_page(
        app,
        workspace_id,
        project_id,
        &cookie,
        query_b,
        1,
        Some(cursor),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "invalid_input");
    assert_eq!(body["params"]["code"], "invalid_cursor");

    admin.close().await;
    harness.cleanup().await;
}

async fn all_task_ids_unpaginated(
    app: axum::Router,
    workspace_id: Uuid,
    project_id: &str,
    cookie: &str,
    query: &str,
) -> Vec<String> {
    let (status, page) =
        list_tasks_page(app, workspace_id, project_id, cookie, query, 100, None).await;
    assert_eq!(status, StatusCode::OK, "{page:?}");
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap().to_string())
        .collect()
}

async fn assert_pagination_walk_matches_unpaginated(
    app: axum::Router,
    workspace_id: Uuid,
    project_id: &str,
    cookie: &str,
    query: &str,
) {
    let expected =
        all_task_ids_unpaginated(app.clone(), workspace_id, project_id, cookie, query).await;
    for limit in [1, 2] {
        let walked =
            walk_task_list_ids(app.clone(), workspace_id, project_id, cookie, query, limit).await;
        assert_eq!(walked, expected, "query {query} limit {limit}");
        assert_eq!(
            walked.len(),
            walked.iter().collect::<HashSet<_>>().len(),
            "query {query} limit {limit}"
        );
    }
}

async fn create_task_with_title(
    app: axum::Router,
    cookie: &str,
    workspace_id: Uuid,
    project_id: &str,
    body: serde_json::Value,
) -> serde_json::Value {
    let (status, task) = json_request(
        app,
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(body),
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{task:?}");
    task
}

#[tokio::test]
async fn task_list_title_sort_pagination_matches_unpaginated() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    for title in ["E", "D", "C", "B", "A"] {
        create_task_with_title(
            app.clone(),
            &cookie,
            workspace_id,
            project_id,
            json!({"title": title}),
        )
        .await;
    }

    let query = r#"{"sort":[{"field":"title","direction":"asc"}]}"#;
    assert_pagination_walk_matches_unpaginated(
        app.clone(),
        workspace_id,
        project_id,
        &cookie,
        query,
    )
    .await;

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_list_priority_tie_pagination_matches_unpaginated() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    for title in ["One", "Two", "Three", "Four"] {
        create_task_with_title(
            app.clone(),
            &cookie,
            workspace_id,
            project_id,
            json!({"title": title, "priority": "high"}),
        )
        .await;
    }

    let query = r#"{"sort":[{"field":"priority","direction":"asc"}]}"#;
    assert_pagination_walk_matches_unpaginated(
        app.clone(),
        workspace_id,
        project_id,
        &cookie,
        query,
    )
    .await;

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_list_priority_rank_order_matches_source() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    for (title, priority) in [
        ("Urgent", "urgent"),
        ("High", "high"),
        ("Medium", "medium"),
        ("Low", "low"),
        ("None", "none"),
    ] {
        create_task_with_title(
            app.clone(),
            &cookie,
            workspace_id,
            project_id,
            json!({"title": title, "priority": priority}),
        )
        .await;
    }

    for query in [
        r#"{"sort":[{"field":"priority","direction":"asc"}]}"#,
        r#"{"sort":[{"field":"priority","direction":"desc"}]}"#,
    ] {
        assert_pagination_walk_matches_unpaginated(
            app.clone(),
            workspace_id,
            project_id,
            &cookie,
            query,
        )
        .await;
    }

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_list_created_desc_pagination_handles_created_at_and_id_inversion() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let mut ids = Vec::new();
    for title in ["One", "Two", "Three", "Four", "Five"] {
        let task = create_task_with_title(
            app.clone(),
            &cookie,
            workspace_id,
            project_id,
            json!({"title": title}),
        )
        .await;
        ids.push(task["id"].as_str().unwrap().to_string());
    }

    sqlx::query(
        "UPDATE fvoci.tasks SET created_at = TIMESTAMPTZ '2026-01-01 12:00:00+00' WHERE workspace_id = $1 AND id = $2::uuid",
    )
    .bind(workspace_id)
    .bind(&ids[0])
    .execute(&admin)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE fvoci.tasks SET created_at = TIMESTAMPTZ '2026-01-01 12:00:00+00' WHERE workspace_id = $1 AND id = $2::uuid",
    )
    .bind(workspace_id)
    .bind(&ids[1])
    .execute(&admin)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE fvoci.tasks SET created_at = TIMESTAMPTZ '2026-01-05 12:00:00+00' WHERE workspace_id = $1 AND id = $2::uuid",
    )
    .bind(workspace_id)
    .bind(&ids[2])
    .execute(&admin)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE fvoci.tasks SET created_at = TIMESTAMPTZ '2026-01-02 12:00:00+00' WHERE workspace_id = $1 AND id = $2::uuid",
    )
    .bind(workspace_id)
    .bind(&ids[3])
    .execute(&admin)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE fvoci.tasks SET created_at = TIMESTAMPTZ '2026-01-03 12:00:00+00' WHERE workspace_id = $1 AND id = $2::uuid",
    )
    .bind(workspace_id)
    .bind(&ids[4])
    .execute(&admin)
    .await
    .unwrap();

    let query = r#"{"sort":[{"field":"created","direction":"desc"}]}"#;
    assert_pagination_walk_matches_unpaginated(
        app.clone(),
        workspace_id,
        project_id,
        &cookie,
        query,
    )
    .await;

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_create_assigns_distinct_increasing_sort_keys_within_status() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let first = create_task_with_title(
        app.clone(),
        &cookie,
        workspace_id,
        project_id,
        json!({"title": "First"}),
    )
    .await;
    let second = create_task_with_title(
        app.clone(),
        &cookie,
        workspace_id,
        project_id,
        json!({"title": "Second"}),
    )
    .await;
    let first_key = first["sortKey"].as_str().unwrap();
    let second_key = second["sortKey"].as_str().unwrap();
    assert_ne!(first_key, second_key);
    assert!(first_key < second_key);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_list_pagination_preserves_status_counts() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    for title in ["E", "D", "C", "B", "A"] {
        create_task_with_title(
            app.clone(),
            &cookie,
            workspace_id,
            project_id,
            json!({"title": title}),
        )
        .await;
    }

    let query = r#"{"sort":[{"field":"title","direction":"asc"}]}"#;
    let (status, first_page) = list_tasks_page(
        app.clone(),
        workspace_id,
        project_id,
        &cookie,
        query,
        2,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let baseline_counts = first_page["statusCounts"].clone();
    let cursor = first_page["nextCursor"].as_str().unwrap();
    let (status, second_page) = list_tasks_page(
        app.clone(),
        workspace_id,
        project_id,
        &cookie,
        query,
        2,
        Some(cursor),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(second_page["statusCounts"], baseline_counts);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_list_rejects_malformed_cursor() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let query = r#"{"sort":[{"field":"number","direction":"asc"}]}"#;
    let (status, body) = list_tasks_page(
        app,
        workspace_id,
        project_id,
        &cookie,
        query,
        10,
        Some("not-a-cursor"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "invalid_input");
    assert_eq!(body["params"]["code"], "invalid_cursor");

    admin.close().await;
    harness.cleanup().await;
}

async fn patch_task(
    app: axum::Router,
    workspace_id: Uuid,
    task_id: &str,
    body: serde_json::Value,
    cookie: &str,
) -> (StatusCode, serde_json::Value) {
    json_request(
        app,
        "PATCH",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}"),
        Some(body),
        Some(cookie),
    )
    .await
}

#[tokio::test]
async fn task_patch_happy_path_updates_meta_and_records_event() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let task = create_task_with_title(
        app.clone(),
        &cookie,
        workspace_id,
        project_id,
        json!({"title": "Before"}),
    )
    .await;
    let task_id = task["id"].as_str().unwrap();
    let events_before = count_rows(&admin, "events").await;

    let (status, patched) = patch_task(
        app.clone(),
        workspace_id,
        task_id,
        json!({
            "title": "After",
            "priority": "high",
            "startDate": "2026-02-01",
            "dueDate": "2026-02-15",
            "estimate": "2.5"
        }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(patched["title"], "After");
    assert_eq!(patched["priority"], "high");
    assert_eq!(patched["startDate"], "2026-02-01");
    assert_eq!(patched["dueDate"], "2026-02-15");
    assert_eq!(patched["estimate"], "2.5");

    let (status, detail) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["title"], "After");
    assert_eq!(count_rows(&admin, "events").await, events_before + 1);
    assert_eq!(count_rows(&admin, "audit_log").await, events_before + 1);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_patch_sets_dates_and_estimate_fields() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let task = create_task_with_title(
        app.clone(),
        &cookie,
        workspace_id,
        project_id,
        json!({"title": "Dates"}),
    )
    .await;
    let task_id = task["id"].as_str().unwrap();

    let (status, patched) = patch_task(
        app.clone(),
        workspace_id,
        task_id,
        json!({"title": "Dates", "startDate": "2026-02-01", "dueDate": "2026-02-15"}),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{patched:?}");
    assert_eq!(patched["startDate"], "2026-02-01");
    assert_eq!(patched["dueDate"], "2026-02-15");

    let (status, patched) = patch_task(
        app.clone(),
        workspace_id,
        task_id,
        json!({"estimate": "2.5"}),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{patched:?}");
    assert_eq!(patched["estimate"], "2.5");
    let _ = patched;

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_patch_rejects_empty_body_and_unsupported_relation_fields() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let task = create_task_with_title(
        app.clone(),
        &cookie,
        workspace_id,
        project_id,
        json!({"title": "Keep"}),
    )
    .await;
    let task_id = task["id"].as_str().unwrap();

    let (status, _) = patch_task(app.clone(), workspace_id, task_id, json!({}), &cookie).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, problem) = patch_task(
        app,
        workspace_id,
        task_id,
        json!({"milestoneId": Uuid::now_v7().to_string()}),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(problem["code"], "not_found");

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_patch_expected_dates_version_conflict() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let task = create_task_with_title(
        app.clone(),
        &cookie,
        workspace_id,
        project_id,
        json!({"title": "Dates"}),
    )
    .await;
    let task_id = task["id"].as_str().unwrap();
    let events_before = count_rows(&admin, "events").await;

    let (status, problem) = patch_task(
        app,
        workspace_id,
        task_id,
        json!({
            "dueDate": "2026-03-01",
            "expectedDates": {
                "startDate": null,
                "dueDate": "2026-01-01",
                "dueAt": null
            }
        }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(problem["code"], "document_version_mismatch");
    assert_eq!(count_rows(&admin, "events").await, events_before);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_patch_archive_unarchive_and_list_archived_filter() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let task = create_task_with_title(
        app.clone(),
        &cookie,
        workspace_id,
        project_id,
        json!({"title": "Archive me"}),
    )
    .await;
    let task_id = task["id"].as_str().unwrap();

    let (status, _) = patch_task(
        app.clone(),
        workspace_id,
        task_id,
        json!({"archived": true}),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, active_list) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks?limit=50"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(active_list["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|item| item["id"].as_str() != Some(task_id)));

    let (status, archived_list) = json_request(
        app.clone(),
        "GET",
        &format!(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks?archived=true&limit=50"
        ),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(archived_list["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["id"].as_str() == Some(task_id)));

    let (status, _) = patch_task(
        app.clone(),
        workspace_id,
        task_id,
        json!({"archived": false}),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, active_again) = json_request(
        app,
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks?limit=50"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(active_again["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["id"].as_str() == Some(task_id)));

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_trash_restore_affects_list_get_and_counts() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let task = create_task_with_title(
        app.clone(),
        &cookie,
        workspace_id,
        project_id,
        json!({"title": "Disposable"}),
    )
    .await;
    let task_id = task["id"].as_str().unwrap();

    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/trash"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, list) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks?limit=50"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(list["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|item| item["id"].as_str() != Some(task_id)));

    let (status, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, projects) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(projects["items"][0]["taskCount"], 0);
    assert_eq!(projects["items"][0]["openTaskCount"], 0);

    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/restore"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, detail) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["title"], "Disposable");

    let (status, projects) = json_request(
        app,
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(projects["items"][0]["taskCount"], 1);
    assert_eq!(projects["items"][0]["openTaskCount"], 1);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_move_keeps_distinct_sort_keys_within_status() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let first = create_task_with_title(
        app.clone(),
        &cookie,
        workspace_id,
        project_id,
        json!({"title": "First"}),
    )
    .await;
    let second = create_task_with_title(
        app.clone(),
        &cookie,
        workspace_id,
        project_id,
        json!({"title": "Second"}),
    )
    .await;
    create_task_with_title(
        app.clone(),
        &cookie,
        workspace_id,
        project_id,
        json!({"title": "Third"}),
    )
    .await;
    let status_id = first["statusId"].as_str().unwrap();
    let second_id = second["id"].as_str().unwrap();
    let first_id = first["id"].as_str().unwrap();

    let (status, moved) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{first_id}/move"),
        Some(json!({
            "statusId": status_id,
            "afterId": second_id
        })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let moved_key = moved["sortKey"].as_str().unwrap();
    let first_key = first["sortKey"].as_str().unwrap();
    let second_key = second["sortKey"].as_str().unwrap();
    assert_ne!(moved_key, first_key);
    assert_ne!(moved_key, second_key);
    assert!(first_key < moved_key);
    assert!(second_key < moved_key);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_patch_denied_for_private_non_member_and_viewer() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, workspace_id, "member", "member").await;
    let hid = create_project(app.clone(), &member.cookie, workspace_id, "HID", "private").await;
    let project_id = hid["id"].as_str().unwrap();
    let task = create_task_with_title(
        app.clone(),
        &member.cookie,
        workspace_id,
        project_id,
        json!({"title": "Secret"}),
    )
    .await;
    let task_id = task["id"].as_str().unwrap();

    let (status, _) = patch_task(
        app.clone(),
        workspace_id,
        task_id,
        json!({"title": "Hacked"}),
        &owner_cookie,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let events_after_non_member = count_rows(&admin, "events").await;

    json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/members"),
        Some(json!({"userId": owner_id.to_string(), "role": "viewer"})),
        Some(&member.cookie),
    )
    .await;
    assert!(count_rows(&admin, "events").await > events_after_non_member);

    let (status, _) = patch_task(
        app,
        workspace_id,
        task_id,
        json!({"title": "Viewer edit"}),
        &owner_cookie,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        count_rows(&admin, "events").await,
        events_after_non_member + 1
    );

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_patch_session_revoke_barrier_leaves_events_unchanged() {
    let harness = TestDb::bootstrap().await;
    let (app, _, _, workspace_id) = setup_session(&harness).await;
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
    let task = create_task_with_title(
        app.clone(),
        &member.cookie,
        workspace_id,
        project_id,
        json!({"title": "Race"}),
    )
    .await;
    let task_id = task["id"].as_str().unwrap().to_string();
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
    let patch_task_id = task_id.clone();
    let patch = tokio::spawn(async move {
        patch_task(
            app_bg,
            workspace_id,
            &patch_task_id,
            json!({"title": "Blocked"}),
            &cookie,
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
    let (status, _) = tokio::time::timeout(Duration::from_secs(10), patch)
        .await
        .expect("patch finished")
        .expect("join");
    assert!(status == StatusCode::UNAUTHORIZED || status == StatusCode::NOT_FOUND);
    assert_eq!(count_rows(&admin, "events").await, events_before);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_patch_audit_failure_rolls_back_all_state() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let task = create_task_with_title(
        app.clone(),
        &cookie,
        workspace_id,
        project_id,
        json!({"title": "Before audit"}),
    )
    .await;
    let task_id = task["id"].as_str().unwrap();
    install_insert_fail_trigger(&admin, "audit_log", "test_task_patch_audit_fail").await;
    let events_before = count_rows(&admin, "events").await;

    let (status, _) = patch_task(
        app.clone(),
        workspace_id,
        task_id,
        json!({"title": "Should rollback"}),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(count_rows(&admin, "events").await, events_before);

    let (status, detail) = json_request(
        app,
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["title"], "Before audit");

    drop_insert_fail_trigger(&admin, "audit_log", "test_task_patch_audit_fail").await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_list_default_sort_uses_rank_asc() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let a = create_task_with_title(
        app.clone(),
        &cookie,
        workspace_id,
        project_id,
        json!({"title": "A"}),
    )
    .await;
    let b = create_task_with_title(
        app.clone(),
        &cookie,
        workspace_id,
        project_id,
        json!({"title": "B"}),
    )
    .await;
    let c = create_task_with_title(
        app.clone(),
        &cookie,
        workspace_id,
        project_id,
        json!({"title": "C"}),
    )
    .await;
    sqlx::query("UPDATE fvoci.tasks SET sort_key = 'z' WHERE workspace_id = $1 AND id = $2::uuid")
        .bind(workspace_id)
        .bind(a["id"].as_str().unwrap())
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query("UPDATE fvoci.tasks SET sort_key = 'm' WHERE workspace_id = $1 AND id = $2::uuid")
        .bind(workspace_id)
        .bind(b["id"].as_str().unwrap())
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query("UPDATE fvoci.tasks SET sort_key = 'a' WHERE workspace_id = $1 AND id = $2::uuid")
        .bind(workspace_id)
        .bind(c["id"].as_str().unwrap())
        .execute(&admin)
        .await
        .unwrap();

    let ids = walk_task_list_ids(app, workspace_id, project_id, &cookie, r#"{}"#, 10).await;
    assert_eq!(
        ids,
        vec![
            c["id"].as_str().unwrap().to_string(),
            b["id"].as_str().unwrap().to_string(),
            a["id"].as_str().unwrap().to_string(),
        ]
    );

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_list_due_sort_paginates_by_due_at_when_due_date_null() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let early = create_task_with_title(
        app.clone(),
        &cookie,
        workspace_id,
        project_id,
        json!({"title": "Early"}),
    )
    .await;
    let late = create_task_with_title(
        app.clone(),
        &cookie,
        workspace_id,
        project_id,
        json!({"title": "Late"}),
    )
    .await;
    sqlx::query(
        "UPDATE fvoci.tasks SET due_date = NULL, due_at = TIMESTAMPTZ '2026-04-01 12:00:00+00' WHERE workspace_id = $1 AND id = $2::uuid",
    )
    .bind(workspace_id)
    .bind(early["id"].as_str().unwrap())
    .execute(&admin)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE fvoci.tasks SET due_date = NULL, due_at = TIMESTAMPTZ '2026-04-15 12:00:00+00' WHERE workspace_id = $1 AND id = $2::uuid",
    )
    .bind(workspace_id)
    .bind(late["id"].as_str().unwrap())
    .execute(&admin)
    .await
    .unwrap();

    let query = r#"{"sort":[{"field":"due","direction":"asc"}]}"#;
    let ids = walk_task_list_ids(app, workspace_id, project_id, &cookie, query, 1).await;
    assert_eq!(ids.len(), 2);
    assert_eq!(ids[0], early["id"].as_str().unwrap());
    assert_eq!(ids[1], late["id"].as_str().unwrap());

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_list_title_filter_escapes_ilike_wildcards() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let percent = create_task_with_title(
        app.clone(),
        &cookie,
        workspace_id,
        project_id,
        json!({"title": "100% done"}),
    )
    .await;
    create_task_with_title(
        app.clone(),
        &cookie,
        workspace_id,
        project_id,
        json!({"title": "100x task"}),
    )
    .await;

    let query = r#"{"filters":{"title":"100%"}}"#;
    let ids = walk_task_list_ids(app, workspace_id, project_id, &cookie, query, 10).await;
    assert_eq!(ids, vec![percent["id"].as_str().unwrap().to_string()]);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn concurrent_visibility_private_vs_task_patch_under_project_lock() {
    let harness = TestDb::bootstrap().await;
    let (app, _owner_cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lead = add_workspace_user(&admin, workspace_id, "member", "lead").await;
    let other = add_workspace_user(&admin, workspace_id, "member", "other").await;
    let lab = create_project(app.clone(), &lead.cookie, workspace_id, "LAB", "workspace").await;
    let project_id = Uuid::parse_str(lab["id"].as_str().unwrap()).unwrap();
    let task = create_task_with_title(
        app.clone(),
        &lead.cookie,
        workspace_id,
        lab["id"].as_str().unwrap(),
        json!({"title": "Race target"}),
    )
    .await;
    let task_id = task["id"].as_str().unwrap().to_string();
    let events_before = count_rows(&admin, "events").await;

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

    let task_patch = tokio::spawn({
        let app = app.clone();
        let cookie = other.cookie.clone();
        let task_id = task_id.clone();
        async move {
            patch_task(
                app,
                workspace_id,
                &task_id,
                json!({"title": "Race edit"}),
                &cookie,
            )
            .await
        }
    });
    wait_for_query_blocked_by(&admin, blocker_pid, "%fvoci.projects%").await;
    let visibility_patch = tokio::spawn({
        let app = app.clone();
        let lead_cookie = lead.cookie.clone();
        async move {
            json_request(
                app,
                "PATCH",
                &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}"),
                Some(json!({"visibility": "private"})),
                Some(&lead_cookie),
            )
            .await
        }
    });
    sqlx::query(
        "UPDATE fvoci.projects SET visibility = 'private', updated_at = now() WHERE workspace_id = $1 AND id = $2",
    )
    .bind(workspace_id)
    .bind(project_id)
    .execute(&mut *barrier)
    .await
    .unwrap();
    barrier.commit().await.unwrap();

    let (visibility_status, _) = tokio::time::timeout(Duration::from_secs(10), visibility_patch)
        .await
        .expect("visibility patch finished")
        .expect("join");
    let (task_status, _) = tokio::time::timeout(Duration::from_secs(10), task_patch)
        .await
        .expect("task patch finished")
        .expect("join");
    assert_eq!(visibility_status, StatusCode::OK);
    assert_eq!(task_status, StatusCode::NOT_FOUND);
    assert_eq!(count_rows(&admin, "events").await, events_before + 1);

    admin.close().await;
    harness.cleanup().await;
}

async fn review_setup() -> (
    TestDb,
    axum::Router,
    String,
    Uuid,
    sqlx::PgPool,
    String,
    Uuid,
) {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap().to_string();
    let pid = Uuid::parse_str(&project_id).unwrap();
    (harness, app, cookie, workspace_id, admin, project_id, pid)
}

async fn workflow_status_ids(
    app: axum::Router,
    cookie: &str,
    ws: Uuid,
    project_id: &str,
) -> Vec<(String, String)> {
    let (_, wf) = json_request(
        app,
        "GET",
        &format!("/api/v1/workspaces/{ws}/projects/{project_id}/workflow"),
        None,
        Some(cookie),
    )
    .await;
    wf["statuses"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["id"].as_str().unwrap().to_string(),
                s["category"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

#[tokio::test]
async fn task_patch_lost_update_preserves_concurrent_priority_under_project_lock() {
    let (harness, app, _cookie, ws, admin, project_id, pid) = review_setup().await;
    let other = add_workspace_user(&admin, ws, "member", "other").await;
    let task = create_task_with_title(
        app.clone(),
        &other.cookie,
        ws,
        &project_id,
        json!({"title": "T"}),
    )
    .await;
    let task_id = task["id"].as_str().unwrap().to_string();
    let tid = Uuid::parse_str(&task_id).unwrap();
    let mut holder = admin.begin().await.unwrap();
    sqlx::query("SELECT id FROM fvoci.projects WHERE workspace_id=$1 AND id=$2 FOR UPDATE")
        .bind(ws)
        .bind(pid)
        .fetch_one(&mut *holder)
        .await
        .unwrap();
    let hpid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *holder)
        .await
        .unwrap();
    let patch = tokio::spawn({
        let app = app.clone();
        let c = other.cookie.clone();
        let t = task_id.clone();
        async move { patch_task(app, ws, &t, json!({"title": "Renamed"}), &c).await }
    });
    wait_for_query_blocked_by(&admin, hpid, "%fvoci.projects%").await;
    sqlx::query("UPDATE fvoci.tasks SET priority='high' WHERE id=$1")
        .bind(tid)
        .execute(&mut *holder)
        .await
        .unwrap();
    holder.commit().await.unwrap();
    let (st, _) = patch.await.unwrap();
    let row: (String, String) =
        sqlx::query_as("SELECT title, priority FROM fvoci.tasks WHERE id=$1")
            .bind(tid)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(st, StatusCode::OK);
    assert_eq!(row.0, "Renamed");
    assert_eq!(row.1, "high");

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_patch_rejects_concurrently_archived_task_under_project_lock() {
    let (harness, app, _cookie, ws, admin, project_id, pid) = review_setup().await;
    let other = add_workspace_user(&admin, ws, "member", "other").await;
    let task = create_task_with_title(
        app.clone(),
        &other.cookie,
        ws,
        &project_id,
        json!({"title": "T"}),
    )
    .await;
    let task_id = task["id"].as_str().unwrap().to_string();
    let tid = Uuid::parse_str(&task_id).unwrap();
    let mut holder = admin.begin().await.unwrap();
    sqlx::query("SELECT id FROM fvoci.projects WHERE workspace_id=$1 AND id=$2 FOR UPDATE")
        .bind(ws)
        .bind(pid)
        .fetch_one(&mut *holder)
        .await
        .unwrap();
    let hpid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *holder)
        .await
        .unwrap();
    let patch = tokio::spawn({
        let app = app.clone();
        let c = other.cookie.clone();
        let t = task_id.clone();
        async move { patch_task(app, ws, &t, json!({"title": "Renamed"}), &c).await }
    });
    wait_for_query_blocked_by(&admin, hpid, "%fvoci.projects%").await;
    sqlx::query("UPDATE fvoci.tasks SET priority='high', archived_at=now() WHERE id=$1")
        .bind(tid)
        .execute(&mut *holder)
        .await
        .unwrap();
    holder.commit().await.unwrap();
    let (st, body) = patch.await.unwrap();
    assert_eq!(st, StatusCode::CONFLICT);
    assert_eq!(body["code"], "task_archived");

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_patch_vs_trash_race_returns_not_found() {
    let (harness, app, _cookie, ws, admin, project_id, pid) = review_setup().await;
    let other = add_workspace_user(&admin, ws, "member", "other").await;
    let task = create_task_with_title(
        app.clone(),
        &other.cookie,
        ws,
        &project_id,
        json!({"title": "T"}),
    )
    .await;
    let task_id = task["id"].as_str().unwrap().to_string();
    let tid = Uuid::parse_str(&task_id).unwrap();
    let mut holder = admin.begin().await.unwrap();
    sqlx::query("SELECT id FROM fvoci.projects WHERE workspace_id=$1 AND id=$2 FOR UPDATE")
        .bind(ws)
        .bind(pid)
        .fetch_one(&mut *holder)
        .await
        .unwrap();
    let hpid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *holder)
        .await
        .unwrap();
    let patch = tokio::spawn({
        let app = app.clone();
        let c = other.cookie.clone();
        let t = task_id.clone();
        async move { patch_task(app, ws, &t, json!({"title": "Renamed"}), &c).await }
    });
    wait_for_query_blocked_by(&admin, hpid, "%fvoci.projects%").await;
    sqlx::query("UPDATE fvoci.tasks SET deleted_at=now() WHERE id=$1")
        .bind(tid)
        .execute(&mut *holder)
        .await
        .unwrap();
    holder.commit().await.unwrap();
    let (st, _) = patch.await.unwrap();
    assert_eq!(st, StatusCode::NOT_FOUND);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_move_stale_expected_status_returns_version_conflict() {
    let (harness, app, cookie, ws, admin, project_id, pid) = review_setup().await;
    let other = add_workspace_user(&admin, ws, "member", "other").await;
    let statuses = workflow_status_ids(app.clone(), &cookie, ws, &project_id).await;
    let task = create_task_with_title(
        app.clone(),
        &other.cookie,
        ws,
        &project_id,
        json!({"title": "T"}),
    )
    .await;
    let task_id = task["id"].as_str().unwrap().to_string();
    let tid = Uuid::parse_str(&task_id).unwrap();
    let from = task["statusId"].as_str().unwrap().to_string();
    let others: Vec<_> = statuses.iter().filter(|(id, _)| *id != from).collect();
    let (s2, s3) = (others[0].0.clone(), others[1].0.clone());
    let mut holder = admin.begin().await.unwrap();
    sqlx::query("SELECT id FROM fvoci.projects WHERE workspace_id=$1 AND id=$2 FOR UPDATE")
        .bind(ws)
        .bind(pid)
        .fetch_one(&mut *holder)
        .await
        .unwrap();
    let hpid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *holder)
        .await
        .unwrap();
    let mv = tokio::spawn({
        let app = app.clone();
        let c = other.cookie.clone();
        let t = task_id.clone();
        let from = from.clone();
        let s3 = s3.clone();
        async move {
            json_request(
                app,
                "POST",
                &format!("/api/v1/workspaces/{ws}/tasks/{t}/move"),
                Some(json!({"statusId": s3, "expectedStatusId": from})),
                Some(&c),
            )
            .await
        }
    });
    wait_for_query_blocked_by(&admin, hpid, "%fvoci.projects%").await;
    sqlx::query("UPDATE fvoci.tasks SET status_id=$2::uuid WHERE id=$1")
        .bind(tid)
        .bind(&s2)
        .execute(&mut *holder)
        .await
        .unwrap();
    holder.commit().await.unwrap();
    let (st, body) = mv.await.unwrap();
    let now: (Uuid,) = sqlx::query_as("SELECT status_id FROM fvoci.tasks WHERE id=$1")
        .bind(tid)
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(st, StatusCode::CONFLICT);
    assert_eq!(body["code"], "document_version_mismatch");
    assert_eq!(now.0.to_string(), s2);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_patch_rejects_hierarchy_cycle_via_type_change() {
    let (harness, app, cookie, ws, admin, project_id, _pid) = review_setup().await;
    let epic = create_task_with_title(
        app.clone(),
        &cookie,
        ws,
        &project_id,
        json!({"title": "E", "type": "epic"}),
    )
    .await;
    let eid = epic["id"].as_str().unwrap().to_string();
    let t = create_task_with_title(
        app.clone(),
        &cookie,
        ws,
        &project_id,
        json!({"title": "T", "type": "task", "parentId": eid}),
    )
    .await;
    let tid = t["id"].as_str().unwrap();
    let (st, body) = patch_task(
        app,
        ws,
        &eid,
        json!({"type": "subtask", "parentId": tid}),
        &cookie,
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);
    assert_eq!(body["code"], "task_hierarchy_violation");

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_patch_rejects_type_change_that_orphans_children() {
    let (harness, app, cookie, ws, admin, project_id, _pid) = review_setup().await;
    let story = create_task_with_title(
        app.clone(),
        &cookie,
        ws,
        &project_id,
        json!({"title": "S", "type": "story"}),
    )
    .await;
    let sid = story["id"].as_str().unwrap().to_string();
    create_task_with_title(
        app.clone(),
        &cookie,
        ws,
        &project_id,
        json!({"title": "sub", "type": "subtask", "parentId": sid}),
    )
    .await;
    let (st, body) = patch_task(app, ws, &sid, json!({"type": "epic"}), &cookie).await;
    assert_eq!(st, StatusCode::CONFLICT);
    assert_eq!(body["code"], "task_hierarchy_violation");

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_move_before_first_item_succeeds_without_panic() {
    let (harness, app, cookie, ws, admin, project_id, _pid) = review_setup().await;
    let a =
        create_task_with_title(app.clone(), &cookie, ws, &project_id, json!({"title": "A"})).await;
    let b =
        create_task_with_title(app.clone(), &cookie, ws, &project_id, json!({"title": "B"})).await;
    let (aid, bid, sid) = (
        a["id"].as_str().unwrap(),
        b["id"].as_str().unwrap(),
        a["statusId"].as_str().unwrap(),
    );
    let (st, body) = json_request(
        app,
        "POST",
        &format!("/api/v1/workspaces/{ws}/tasks/{bid}/move"),
        Some(json!({"statusId": sid, "beforeId": aid})),
        Some(&cookie),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        body["sortKey"].as_str().unwrap() < a["sortKey"].as_str().unwrap(),
        "moved task should sort before anchor"
    );

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_patch_rejects_invalid_recurrence_preset() {
    let (harness, app, cookie, ws, admin, project_id, _pid) = review_setup().await;
    let t =
        create_task_with_title(app.clone(), &cookie, ws, &project_id, json!({"title": "T"})).await;
    let tid = t["id"].as_str().unwrap();
    let (st, body) = patch_task(
        app,
        ws,
        tid,
        json!({"recurrence": {"kind": "hourly", "x": [1, 2, 3]}}),
        &cookie,
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "invalid_recurrence_preset");

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_move_enforces_wip_limit() {
    let (harness, app, cookie, ws, admin, project_id, _pid) = review_setup().await;
    let statuses = workflow_status_ids(app.clone(), &cookie, ws, &project_id).await;
    let a =
        create_task_with_title(app.clone(), &cookie, ws, &project_id, json!({"title": "A"})).await;
    let b =
        create_task_with_title(app.clone(), &cookie, ws, &project_id, json!({"title": "B"})).await;
    let target = statuses
        .iter()
        .find(|(id, _)| id != a["statusId"].as_str().unwrap())
        .unwrap()
        .0
        .clone();
    sqlx::query("UPDATE fvoci.statuses SET wip_limit=1 WHERE id=$1::uuid")
        .bind(&target)
        .execute(&admin)
        .await
        .unwrap();
    let mv = |id: String| {
        let app = app.clone();
        let c = cookie.clone();
        let target = target.clone();
        async move {
            json_request(
                app,
                "POST",
                &format!("/api/v1/workspaces/{ws}/tasks/{id}/move"),
                Some(json!({"statusId": target})),
                Some(&c),
            )
            .await
        }
    };
    let (s1, _) = mv(a["id"].as_str().unwrap().to_string()).await;
    let (s2, body) = mv(b["id"].as_str().unwrap().to_string()).await;
    assert_eq!(s1, StatusCode::OK);
    assert_eq!(s2, StatusCode::CONFLICT);
    assert_eq!(body["code"], "wip_limit_exceeded");

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_move_to_done_spawns_recurring_next_occurrence() {
    let (harness, app, cookie, ws, admin, project_id, pid) = review_setup().await;
    let statuses = workflow_status_ids(app.clone(), &cookie, ws, &project_id).await;
    let done = statuses
        .iter()
        .find(|(_, c)| c == "done")
        .unwrap()
        .0
        .clone();
    let t = create_task_with_title(
        app.clone(),
        &cookie,
        ws,
        &project_id,
        json!({
            "title": "R",
            "recurrence": {"kind": "daily"},
            "dueDate": "2026-01-01"
        }),
    )
    .await;
    let tid = t["id"].as_str().unwrap();
    let before = count_rows(&admin, "tasks").await;
    let (st, body) = json_request(
        app,
        "POST",
        &format!("/api/v1/workspaces/{ws}/tasks/{tid}/move"),
        Some(json!({"statusId": done})),
        Some(&cookie),
    )
    .await;
    let after: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.tasks WHERE project_id=$1 AND deleted_at IS NULL",
    )
    .bind(pid)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(st, StatusCode::OK);
    assert!(body["recurrence"].is_null());
    assert_eq!(after.0, before + 1);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_patch_unrelated_fields_do_not_rewrite_estimate_precision() {
    let (harness, app, cookie, ws, admin, project_id, _pid) = review_setup().await;
    let t =
        create_task_with_title(app.clone(), &cookie, ws, &project_id, json!({"title": "T"})).await;
    let tid = t["id"].as_str().unwrap();
    patch_task(
        app.clone(),
        ws,
        tid,
        json!({"estimate": "123456789012.123456"}),
        &cookie,
    )
    .await;
    let before: (String,) =
        sqlx::query_as("SELECT estimate::text FROM fvoci.tasks WHERE id=$1::uuid")
            .bind(tid)
            .fetch_one(&admin)
            .await
            .unwrap();
    patch_task(app, ws, tid, json!({"title": "unrelated"}), &cookie).await;
    let after: (String,) =
        sqlx::query_as("SELECT estimate::text FROM fvoci.tasks WHERE id=$1::uuid")
            .bind(tid)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(before, after);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_move_and_patch_reject_cross_project_status() {
    let (harness, app, cookie, ws, admin, project_id, _pid) = review_setup().await;
    let other = create_project(app.clone(), &cookie, ws, "OTH", "workspace").await;
    let other_status = workflow_status_ids(app.clone(), &cookie, ws, other["id"].as_str().unwrap())
        .await[0]
        .0
        .clone();
    let t =
        create_task_with_title(app.clone(), &cookie, ws, &project_id, json!({"title": "T"})).await;
    let tid = t["id"].as_str().unwrap();
    let (s1, b1) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/tasks/{tid}/move"),
        Some(json!({"statusId": other_status})),
        Some(&cookie),
    )
    .await;
    let (s2, b2) = patch_task(
        app.clone(),
        ws,
        tid,
        json!({"statusId": other_status}),
        &cookie,
    )
    .await;
    assert_eq!(s1, StatusCode::BAD_REQUEST);
    assert_eq!(b1["code"], "status_not_in_project_workflow");
    assert_eq!(s2, StatusCode::BAD_REQUEST);
    assert_eq!(b2["code"], "status_not_in_project_workflow");

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_patch_expected_dates_conflict_uses_document_version_mismatch() {
    let (harness, app, cookie, ws, admin, project_id, _pid) = review_setup().await;
    let t =
        create_task_with_title(app.clone(), &cookie, ws, &project_id, json!({"title": "T"})).await;
    let tid = t["id"].as_str().unwrap();
    let (st, body) = patch_task(
        app,
        ws,
        tid,
        json!({
            "expectedDates": {
                "startDate": "2020-01-01",
                "dueDate": null,
                "dueAt": null
            },
            "title": "x"
        }),
        &cookie,
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);
    assert_eq!(body["code"], "document_version_mismatch");

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_move_trash_restore_denied_for_private_non_member_and_viewer() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, workspace_id, "member", "member").await;
    let hid = create_project(app.clone(), &member.cookie, workspace_id, "HID", "private").await;
    let project_id = hid["id"].as_str().unwrap();
    let task = create_task_with_title(
        app.clone(),
        &member.cookie,
        workspace_id,
        project_id,
        json!({"title": "Secret"}),
    )
    .await;
    let task_id = task["id"].as_str().unwrap();

    for path in ["move", "trash"] {
        let (status, _) = match path {
            "move" => {
                json_request(
                    app.clone(),
                    "POST",
                    &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/{path}"),
                    Some(json!({"statusId": task["statusId"]})),
                    Some(&owner_cookie),
                )
                .await
            }
            _ => {
                json_request(
                    app.clone(),
                    "POST",
                    &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/{path}"),
                    None,
                    Some(&owner_cookie),
                )
                .await
            }
        };
        assert_eq!(status, StatusCode::NOT_FOUND, "non-member {path}");
    }

    json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/members"),
        Some(json!({"userId": owner_id.to_string(), "role": "viewer"})),
        Some(&member.cookie),
    )
    .await;

    let (trash_status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/trash"),
        None,
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(trash_status, StatusCode::NOT_FOUND);

    let (move_status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/move"),
        Some(json!({"statusId": task["statusId"]})),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(move_status, StatusCode::NOT_FOUND);

    let (trash_ok, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/trash"),
        None,
        Some(&member.cookie),
    )
    .await;
    assert_eq!(trash_ok, StatusCode::OK);

    let (restore_status, _) = json_request(
        app,
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/restore"),
        None,
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(restore_status, StatusCode::NOT_FOUND);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_move_to_done_spawns_recurring_occurrence_with_source_parity() {
    let (harness, app, cookie, ws, admin, project_id, pid) = review_setup().await;
    let statuses = workflow_status_ids(app.clone(), &cookie, ws, &project_id).await;
    let done = statuses
        .iter()
        .find(|(_, c)| c == "done")
        .unwrap()
        .0
        .clone();
    let t = create_task_with_title(
        app.clone(),
        &cookie,
        ws,
        &project_id,
        json!({
            "title": "R",
            "priority": "high",
            "recurrence": {"kind": "monthly"},
            "startDate": "2026-01-31",
            "dueDate": "2026-01-31"
        }),
    )
    .await;
    let tid = t["id"].as_str().unwrap();
    patch_task(app.clone(), ws, tid, json!({"estimate": "3.5"}), &cookie).await;
    let (st, body) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/tasks/{tid}/move"),
        Some(json!({"statusId": done})),
        Some(&cookie),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    type SpawnRow = (
        uuid::Uuid,
        String,
        String,
        Option<chrono::NaiveDate>,
        Option<chrono::NaiveDate>,
        Option<serde_json::Value>,
        Option<String>,
        uuid::Uuid,
    );
    let rows: Vec<SpawnRow> = sqlx::query_as(
        "SELECT t.id, t.title, t.priority, t.start_date, t.due_date, t.recurrence, t.estimate::text, t.status_id FROM fvoci.tasks t WHERE t.project_id=$1 ORDER BY number",
    )
    .bind(pid)
    .fetch_all(&admin)
    .await
    .unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows[0].5.is_none(), "original recurrence cleared");
    let next = &rows[1];
    assert_eq!(next.1, "R");
    assert_eq!(next.2, "high");
    assert_eq!(
        next.3,
        Some(chrono::NaiveDate::from_ymd_opt(2026, 3, 3).unwrap())
    );
    assert_eq!(
        next.4,
        Some(chrono::NaiveDate::from_ymd_opt(2026, 3, 3).unwrap())
    );
    assert_eq!(next.5, Some(json!({"kind": "monthly"})));
    assert!(
        next.6.is_none(),
        "estimate is not copied to spawned occurrence"
    );
    let ev: Vec<(String, serde_json::Value)> =
        sqlx::query_as("SELECT verb, payload FROM fvoci.events WHERE target_id=$1")
            .bind(next.0)
            .fetch_all(&admin)
            .await
            .unwrap();
    let au: Vec<(String,)> = sqlx::query_as("SELECT verb FROM fvoci.audit_log WHERE target_id=$1")
        .bind(next.0)
        .fetch_all(&admin)
        .await
        .unwrap();
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].0, "task.created");
    assert_eq!(ev[0].1["recurrenceOf"], json!(tid));
    assert_eq!(au.len(), 1);
    assert_eq!(au[0].0, "task.created");

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_move_to_done_recurrence_spawn_failure_rolls_back_move() {
    let (harness, app, cookie, ws, admin, project_id, pid) = review_setup().await;
    let statuses = workflow_status_ids(app.clone(), &cookie, ws, &project_id).await;
    let done = statuses
        .iter()
        .find(|(_, c)| c == "done")
        .unwrap()
        .0
        .clone();
    let t = create_task_with_title(
        app.clone(),
        &cookie,
        ws,
        &project_id,
        json!({"title": "R", "recurrence": {"kind": "daily"}}),
    )
    .await;
    let tid = t["id"].as_str().unwrap();
    let events_before = count_rows(&admin, "events").await;
    let next_before: (i32,) = sqlx::query_as("SELECT next_number FROM fvoci.projects WHERE id=$1")
        .bind(pid)
        .fetch_one(&admin)
        .await
        .unwrap();
    install_insert_fail_trigger(&admin, "tasks", "dprobe_task_insert_fail").await;
    let (st, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/tasks/{tid}/move"),
        Some(json!({"statusId": done})),
        Some(&cookie),
    )
    .await;
    drop_insert_fail_trigger(&admin, "tasks", "dprobe_task_insert_fail").await;
    let row: (String, Option<serde_json::Value>) =
        sqlx::query_as("SELECT status_id::text, recurrence FROM fvoci.tasks WHERE id=$1::uuid")
            .bind(tid)
            .fetch_one(&admin)
            .await
            .unwrap();
    let next_after: (i32,) = sqlx::query_as("SELECT next_number FROM fvoci.projects WHERE id=$1")
        .bind(pid)
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(st, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(row.0, t["statusId"].as_str().unwrap());
    assert!(row.1.is_some());
    assert_eq!(next_before, next_after);
    assert_eq!(count_rows(&admin, "events").await, events_before);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_wip_limit_concurrent_moves_allow_only_one() {
    let (harness, app, cookie, ws, admin, project_id, _pid) = review_setup().await;
    let statuses = workflow_status_ids(app.clone(), &cookie, ws, &project_id).await;
    let a =
        create_task_with_title(app.clone(), &cookie, ws, &project_id, json!({"title": "A"})).await;
    let b =
        create_task_with_title(app.clone(), &cookie, ws, &project_id, json!({"title": "B"})).await;
    let target = statuses
        .iter()
        .find(|(id, _)| id != a["statusId"].as_str().unwrap())
        .unwrap()
        .0
        .clone();
    sqlx::query("UPDATE fvoci.statuses SET wip_limit=1 WHERE id=$1::uuid")
        .bind(&target)
        .execute(&admin)
        .await
        .unwrap();
    let mv = |id: String| {
        let app = app.clone();
        let c = cookie.clone();
        let target = target.clone();
        async move {
            json_request(
                app,
                "POST",
                &format!("/api/v1/workspaces/{ws}/tasks/{id}/move"),
                Some(json!({"statusId": target})),
                Some(&c),
            )
            .await
        }
    };
    let (r1, r2) = tokio::join!(
        mv(a["id"].as_str().unwrap().to_string()),
        mv(b["id"].as_str().unwrap().to_string())
    );
    let n: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.tasks WHERE status_id=$1::uuid AND deleted_at IS NULL AND archived_at IS NULL",
    )
    .bind(&target)
    .fetch_one(&admin)
    .await
    .unwrap();
    let statuses = [r1.0, r2.0];
    let ok_count = statuses
        .iter()
        .filter(|status| **status == StatusCode::OK)
        .count();
    let conflict_count = statuses
        .iter()
        .filter(|status| **status == StatusCode::CONFLICT)
        .count();
    assert_eq!(ok_count, 1);
    assert_eq!(conflict_count, 1);
    assert_eq!(n.0, 1);

    let (winner, loser) = if r1.0 == StatusCode::OK {
        (&a, &b)
    } else {
        (&b, &a)
    };
    patch_task(
        app.clone(),
        ws,
        winner["id"].as_str().unwrap(),
        json!({"statusId": winner["statusId"]}),
        &cookie,
    )
    .await;
    let c =
        create_task_with_title(app.clone(), &cookie, ws, &project_id, json!({"title": "C"})).await;
    let (s4, _) = patch_task(
        app.clone(),
        ws,
        c["id"].as_str().unwrap(),
        json!({"statusId": target}),
        &cookie,
    )
    .await;
    let (s5, b5) = patch_task(
        app.clone(),
        ws,
        loser["id"].as_str().unwrap(),
        json!({"statusId": target}),
        &cookie,
    )
    .await;
    assert_eq!(s4, StatusCode::OK);
    assert_eq!(s5, StatusCode::CONFLICT);
    assert_eq!(b5["code"], "wip_limit_exceeded");

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_patch_status_done_with_type_change_spawns_post_patch_values() {
    let (harness, app, cookie, ws, admin, project_id, pid) = review_setup().await;
    let statuses = workflow_status_ids(app.clone(), &cookie, ws, &project_id).await;
    let done = statuses
        .iter()
        .find(|(_, c)| c == "done")
        .unwrap()
        .0
        .clone();
    let t = create_task_with_title(
        app.clone(),
        &cookie,
        ws,
        &project_id,
        json!({
            "title": "R",
            "type": "task",
            "recurrence": {"kind": "weekly"}
        }),
    )
    .await;
    let tid = t["id"].as_str().unwrap();
    let (st, body) = patch_task(
        app.clone(),
        ws,
        tid,
        json!({
            "statusId": done,
            "type": "bug",
            "recurrence": {"kind": "daily"}
        }),
        &cookie,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let rows: Vec<(String, Option<serde_json::Value>)> = sqlx::query_as(
        "SELECT type, recurrence FROM fvoci.tasks WHERE project_id=$1 ORDER BY number",
    )
    .bind(pid)
    .fetch_all(&admin)
    .await
    .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].0, "bug");
    assert_eq!(rows[1].0, "bug");
    assert_eq!(rows[1].1, Some(json!({"kind": "weekly"})));

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_concurrent_move_to_done_on_recurring_task_spawns_once() {
    let (harness, app, cookie, ws, admin, project_id, pid) = review_setup().await;
    let statuses = workflow_status_ids(app.clone(), &cookie, ws, &project_id).await;
    let done = statuses
        .iter()
        .find(|(_, c)| c == "done")
        .unwrap()
        .0
        .clone();
    let t = create_task_with_title(
        app.clone(),
        &cookie,
        ws,
        &project_id,
        json!({"title": "R", "recurrence": {"kind": "daily"}}),
    )
    .await;
    let tid = t["id"].as_str().unwrap().to_string();
    let before = count_rows(&admin, "tasks").await;
    let mv = |tid: String| {
        let app = app.clone();
        let c = cookie.clone();
        let done = done.clone();
        async move {
            json_request(
                app,
                "POST",
                &format!("/api/v1/workspaces/{ws}/tasks/{tid}/move"),
                Some(json!({"statusId": done})),
                Some(&c),
            )
            .await
        }
    };
    let (r1, r2) = tokio::join!(mv(tid.clone()), mv(tid.clone()));
    let after: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.tasks WHERE project_id=$1 AND deleted_at IS NULL",
    )
    .bind(pid)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert!(r1.0 == StatusCode::OK || r2.0 == StatusCode::OK);
    assert_eq!(after.0, before + 1, "only one recurring spawn must succeed");

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_dates_accept_only_source_iso_format() {
    let (harness, app, cookie, ws, admin, project_id, _pid) = review_setup().await;
    for bad in ["2026-1-5", "+262142-12-31", "2026-02-30"] {
        let (st, _) = json_request(
            app.clone(),
            "POST",
            &format!("/api/v1/workspaces/{ws}/projects/{project_id}/tasks"),
            Some(json!({"title": "T", "dueDate": bad})),
            Some(&cookie),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "create dueDate {bad}");
    }
    let t =
        create_task_with_title(app.clone(), &cookie, ws, &project_id, json!({"title": "T"})).await;
    let tid = t["id"].as_str().unwrap();
    for bad in ["2026-1-5", "+262142-12-31"] {
        let (st, _) = patch_task(app.clone(), ws, tid, json!({"dueDate": bad}), &cookie).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "patch dueDate {bad}");
    }
    let (st, body) = patch_task(
        app.clone(),
        ws,
        tid,
        json!({"dueDate": "9999-12-31"}),
        &cookie,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_layout_gantt_geometry_and_holiday_column() {
    let (harness, app, cookie, ws, admin, project_id, _pid) = review_setup().await;
    let (status, task_a) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/projects/{project_id}/tasks"),
        Some(json!({
            "title": "Layout A",
            "startDate": "2026-09-01",
            "dueDate": "2026-09-03"
        })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, task_b) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/projects/{project_id}/tasks"),
        Some(json!({
            "title": "Layout B",
            "startDate": "2026-09-04",
            "dueDate": "2026-09-04"
        })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let a_id = task_a["id"].as_str().unwrap();
    let b_id = task_b["id"].as_str().unwrap();
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/tasks/{a_id}/dependencies"),
        Some(json!({"blockedId": b_id, "type": "FS"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/holidays"),
        Some(json!({"date": "2026-09-02"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let q = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("year", "2026")
        .append_pair("month", "9")
        .append_pair("query", r#"{"filters":{"title":"Layout"},"sort":[]}"#)
        .finish();
    let (status, layout) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{ws}/projects/{project_id}/task-layout?{q}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(layout["truncated"], false);
    let items = layout["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(layout["pathTotal"], 1);
    let off = layout["columns"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["date"] == "2026-09-02")
        .unwrap();
    assert_eq!(off["offDuty"], true);
    assert!(layout["monthBands"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b["label"] == "9월"));
    let (status, _) = json_request(
        app.clone(),
        "GET",
        &format!(
            "/api/v1/workspaces/{ws}/projects/{project_id}/task-layout?year=2026&month=9&view=calendar"
        ),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_layout_auth_pat_session_and_query_validation() {
    let (harness, app, cookie, ws, admin, project_id, _pid) = review_setup().await;
    let layout_path =
        format!("/api/v1/workspaces/{ws}/projects/{project_id}/task-layout?year=2026&month=9");

    let (status, _) = json_request(app.clone(), "GET", &layout_path, None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, read_token) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/api-tokens"),
        Some(json!({"name": "read", "scopes": ["tasks.read"]})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let read = read_token["token"].as_str().unwrap();
    let (status, layout) = json_request_bearer(app.clone(), "GET", &layout_path, read).await;
    assert_eq!(status, StatusCode::OK);
    assert!(layout["items"].is_array());

    let (status, wrong_scope) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/api-tokens"),
        Some(json!({"name": "proj", "scopes": ["projects.read"]})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let projects_only = wrong_scope["token"].as_str().unwrap();
    let (status, _) = json_request_bearer(app.clone(), "GET", &layout_path, projects_only).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let other_ws = insert_other_workspace(&admin).await;
    let foreign_path = format!(
        "/api/v1/workspaces/{other_ws}/projects/{project_id}/task-layout?year=2026&month=9"
    );
    let (status, _) = json_request(app.clone(), "GET", &foreign_path, None, Some(&cookie)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = json_request(
        app.clone(),
        "GET",
        &format!(
            "/api/v1/workspaces/{ws}/projects/{project_id}/task-layout?year=2026&month=9&maxLanes=501"
        ),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{ws}/projects/{project_id}/task-layout?year=2026&month=13"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_layout_revoked_session_is_unauthorized() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    sqlx::query("UPDATE fvoci.sessions SET revoked_at = now() WHERE user_id = $1")
        .bind(owner_id)
        .execute(&admin)
        .await
        .unwrap();
    let (status, _) = json_request(
        app,
        "GET",
        &format!(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/task-layout?year=2026&month=9"
        ),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_layout_private_project_denies_non_member() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, workspace_id, "member", "member").await;
    let hid = create_project(app.clone(), &member.cookie, workspace_id, "HID", "private").await;
    let project_id = hid["id"].as_str().unwrap();
    let (status, _) = json_request(
        app,
        "GET",
        &format!(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/task-layout?year=2026&month=9"
        ),
        None,
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_layout_truncation_applies_after_title_filter() {
    let (harness, app, cookie, ws, admin, project_id, _pid) = review_setup().await;
    let pid = Uuid::parse_str(&project_id).unwrap();
    let (_, wf) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{ws}/projects/{project_id}/workflow"),
        None,
        Some(&cookie),
    )
    .await;
    let status_id = Uuid::parse_str(wf["statuses"][0]["id"].as_str().unwrap()).unwrap();
    let owner_id: Uuid = sqlx::query_scalar("SELECT id FROM fvoci.users LIMIT 1")
        .fetch_one(&admin)
        .await
        .unwrap();
    for number in 1..=501_i32 {
        sqlx::query(
            r#"
            INSERT INTO fvoci.tasks (
                id, workspace_id, project_id, number, title, type, priority, status_id,
                content_json, created_by, start_date, due_date
            ) VALUES (
                $1, $2, $3, $4, $5, 'task', 'none', $6,
                '{"type":"doc","content":[{"type":"paragraph"}]}'::jsonb, $7,
                '2026-09-02'::date, '2026-09-28'::date
            )
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(ws)
        .bind(pid)
        .bind(number)
        .bind(format!("bulk filler {number}"))
        .bind(status_id)
        .bind(owner_id)
        .execute(&admin)
        .await
        .unwrap();
    }
    sqlx::query(
        r#"
        INSERT INTO fvoci.tasks (
            id, workspace_id, project_id, number, title, type, priority, status_id,
            content_json, created_by, start_date, due_date
        ) VALUES (
            $1, $2, $3, 502, 'GanttNeedleUnique', 'task', 'none', $4,
            '{"type":"doc","content":[{"type":"paragraph"}]}'::jsonb, $5,
            '2026-09-05'::date, '2026-09-06'::date
        )
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(ws)
    .bind(pid)
    .bind(status_id)
    .bind(owner_id)
    .execute(&admin)
    .await
    .unwrap();

    let (status, unfiltered) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{ws}/projects/{project_id}/task-layout?year=2026&month=9"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(unfiltered["truncated"], true);
    assert_eq!(unfiltered["items"].as_array().unwrap().len(), 500);

    let q = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("year", "2026")
        .append_pair("month", "9")
        .append_pair(
            "query",
            r#"{"filters":{"title":"GanttNeedleUnique"},"sort":[]}"#,
        )
        .finish();
    let (status, filtered) = json_request(
        app,
        "GET",
        &format!("/api/v1/workspaces/{ws}/projects/{project_id}/task-layout?{q}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(filtered["truncated"], false);
    let titles = filtered["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["title"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(titles, vec!["GanttNeedleUnique"]);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_layout_year_one_month_one_and_due_at_window() {
    let (harness, app, cookie, ws, admin, project_id, _pid) = review_setup().await;
    let (status, empty) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{ws}/projects/{project_id}/task-layout?year=1&month=1"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(empty["items"].as_array().unwrap().len(), 0);

    let (status, task) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/projects/{project_id}/tasks"),
        Some(json!({"title": "DueAt only"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let task_id = task["id"].as_str().unwrap();
    let (status, _) = patch_task(
        app.clone(),
        ws,
        task_id,
        json!({"dueAt": "2026-09-15T12:00:00Z"}),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, layout) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{ws}/projects/{project_id}/task-layout?year=2026&month=9"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let item = layout["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == task_id)
        .expect("dueAt task in layout");
    let due_at = item["dueAt"].as_str().expect("dueAt in layout item");
    let parsed = chrono::DateTime::parse_from_rfc3339(due_at)
        .expect("layout dueAt is RFC3339")
        .with_timezone(&chrono::Utc);
    let expected = chrono::DateTime::parse_from_rfc3339("2026-09-15T12:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    assert_eq!(parsed, expected);
    assert_eq!(item["end"], "2026-09-15");

    let (status, _) = patch_task(
        app,
        ws,
        task_id,
        json!({"dueAt": null, "dueDate": "2026-09-16"}),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    admin.close().await;
    harness.cleanup().await;
}

async fn get_task_layout(
    app: axum::Router,
    workspace_id: Uuid,
    project_id: &str,
    query: &str,
    cookie: &str,
) -> serde_json::Value {
    let (status, layout) = json_request(
        app,
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/task-layout?{query}"),
        None,
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{layout}");
    layout
}

fn layout_item<'a>(layout: &'a serde_json::Value, task_id: &str) -> &'a serde_json::Value {
    layout["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == task_id)
        .unwrap_or_else(|| panic!("task {task_id} missing from layout {layout}"))
}

fn layout_links(layout: &serde_json::Value) -> Vec<(String, String, String, i64)> {
    let mut links: Vec<_> = layout["links"]
        .as_array()
        .unwrap_or_else(|| panic!("links missing from layout {layout}"))
        .iter()
        .map(|link| {
            (
                link["blockerId"].as_str().unwrap().to_string(),
                link["blockedId"].as_str().unwrap().to_string(),
                link["type"].as_str().unwrap().to_string(),
                link["lagDays"].as_i64().unwrap(),
            )
        })
        .collect();
    links.sort();
    links
}

#[tokio::test]
async fn task_layout_can_edit_follows_project_permission_and_archive() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let viewer = add_workspace_user(&admin, workspace_id, "member", "viewer").await;
    let editor = add_workspace_user(&admin, workspace_id, "member", "editor").await;
    let prv = create_project(app.clone(), &owner_cookie, workspace_id, "PRV", "private").await;
    let project_id = prv["id"].as_str().unwrap();
    for (user, role) in [(&viewer, "viewer"), (&editor, "member")] {
        let (status, body) = json_request(
            app.clone(),
            "POST",
            &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/members"),
            Some(json!({"userId": user.user_id.to_string(), "role": role})),
            Some(&owner_cookie),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }
    let task = create_task_with_title(
        app.clone(),
        &owner_cookie,
        workspace_id,
        project_id,
        json!({"title": "Gate", "dueDate": "2026-09-10"}),
    )
    .await;
    let task_id = task["id"].as_str().unwrap();
    let month = "year=2026&month=9";

    for (who, cookie, can_edit) in [
        ("lead", &owner_cookie, true),
        ("member", &editor.cookie, true),
        ("viewer", &viewer.cookie, false),
    ] {
        let layout = get_task_layout(app.clone(), workspace_id, project_id, month, cookie).await;
        assert_eq!(layout["canEdit"], can_edit, "{who}");
        assert_eq!(layout["items"].as_array().unwrap().len(), 1, "{who}");
    }
    // `false` matches what PATCH does for the same actor.
    let (status, _) = patch_task(
        app.clone(),
        workspace_id,
        task_id,
        json!({"dueDate": "2026-09-11"}),
        &viewer.cookie,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // The hint ignores API-token scopes: a tasks.read token of the lead sees
    // `true` although PATCH also needs tasks.write.
    let (status, token) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/api-tokens"),
        Some(json!({"name": "read", "scopes": ["tasks.read"]})),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{token}");
    let (status, layout) = json_request_bearer(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/task-layout?{month}"),
        token["token"].as_str().unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{layout}");
    assert_eq!(layout["canEdit"], true);

    // On a workspace-visibility project the workspace role decides for
    // non-guests: the workspace member who only views PRV may edit here. A
    // guest added as a project viewer may not.
    let guest = add_workspace_user(&admin, workspace_id, "guest", "guest").await;
    let wsp = create_project(app.clone(), &owner_cookie, workspace_id, "WSP", "workspace").await;
    let wsp_id = wsp["id"].as_str().unwrap();
    let (status, body) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{wsp_id}/members"),
        Some(json!({"userId": guest.user_id.to_string(), "role": "viewer"})),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    for (who, cookie, can_edit) in [
        ("workspace member", &viewer.cookie, true),
        ("guest viewer", &guest.cookie, false),
    ] {
        let layout = get_task_layout(app.clone(), workspace_id, wsp_id, month, cookie).await;
        assert_eq!(layout["canEdit"], can_edit, "{who}");
    }

    let (status, body) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/archive"),
        None,
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    for (who, cookie) in [
        ("lead", &owner_cookie),
        ("member", &editor.cookie),
        ("viewer", &viewer.cookie),
    ] {
        // Archived projects stay readable, but nobody may reschedule.
        let layout = get_task_layout(app.clone(), workspace_id, project_id, month, cookie).await;
        assert_eq!(layout["canEdit"], false, "{who}");
        assert_eq!(layout["items"].as_array().unwrap().len(), 1, "{who}");
    }
    let (status, body) = patch_task(
        app.clone(),
        workspace_id,
        task_id,
        json!({"dueDate": "2026-09-11"}),
        &owner_cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "project_archived");

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_layout_links_are_limited_to_returned_items() {
    let (harness, app, cookie, ws, admin, project_id, _pid) = review_setup().await;
    let mut ids = Vec::new();
    for (title, start, due) in [
        ("Link A", "2026-09-01", "2026-09-03"),
        ("Link B", "2026-09-08", "2026-09-09"),
        ("Other C", "2026-09-10", "2026-09-11"),
        ("Link D", "2026-11-02", "2026-11-03"),
    ] {
        let task = create_task_with_title(
            app.clone(),
            &cookie,
            ws,
            &project_id,
            json!({"title": title, "startDate": start, "dueDate": due}),
        )
        .await;
        ids.push(task["id"].as_str().unwrap().to_string());
    }
    let (a, b, c, d) = (&ids[0], &ids[1], &ids[2], &ids[3]);
    for (blocker, blocked, kind, lag) in [(a, b, "FS", 2), (a, c, "SS", 0), (b, d, "FF", 1)] {
        let (status, body) = json_request(
            app.clone(),
            "POST",
            &format!("/api/v1/workspaces/{ws}/tasks/{blocker}/dependencies"),
            Some(json!({"blockedId": blocked, "type": kind, "lagDays": lag})),
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }

    // D lies outside September, so B -> D is not returned.
    let month = get_task_layout(app.clone(), ws, &project_id, "year=2026&month=9", &cookie).await;
    let mut expected = vec![
        (a.clone(), b.clone(), "FS".to_string(), 2),
        (a.clone(), c.clone(), "SS".to_string(), 0),
    ];
    expected.sort();
    assert_eq!(layout_links(&month), expected);
    assert_eq!(month["linkTotal"], 2);

    // The title filter drops C, and with it A -> C.
    let q = form_urlencoded::Serializer::new(String::new())
        .append_pair("year", "2026")
        .append_pair("month", "9")
        .append_pair("query", r#"{"filters":{"title":"Link"},"sort":[]}"#)
        .finish();
    let filtered = get_task_layout(app.clone(), ws, &project_id, &q, &cookie).await;
    assert_eq!(filtered["items"].as_array().unwrap().len(), 2);
    assert_eq!(
        layout_links(&filtered),
        vec![(a.clone(), b.clone(), "FS".to_string(), 2)]
    );
    assert_eq!(filtered["linkTotal"], 1);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_layout_links_cap_keeps_lowest_pairs_and_reports_total() {
    let (harness, app, cookie, ws, admin, project_id, pid) = review_setup().await;
    let (_, wf) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{ws}/projects/{project_id}/workflow"),
        None,
        Some(&cookie),
    )
    .await;
    let status_id = Uuid::parse_str(wf["statuses"][0]["id"].as_str().unwrap()).unwrap();
    let owner_id: Uuid = sqlx::query_scalar("SELECT id FROM fvoci.users LIMIT 1")
        .fetch_one(&admin)
        .await
        .unwrap();
    // 65 tasks with every forward pair linked: 65 * 64 / 2 = 2080 links, over
    // the 2048 cap. Written directly: the API would take one request per link.
    sqlx::query(
        r#"
        INSERT INTO fvoci.tasks (
            id, workspace_id, project_id, number, title, type, priority, status_id,
            content_json, created_by, start_date, due_date
        )
        SELECT gen_random_uuid(), $1, $2, n, 'cap ' || n, 'task', 'none', $3,
               '{"type":"doc","content":[{"type":"paragraph"}]}'::jsonb, $4,
               '2026-09-07'::date, '2026-09-08'::date
        FROM generate_series(1, 65) AS n
        "#,
    )
    .bind(ws)
    .bind(pid)
    .bind(status_id)
    .bind(owner_id)
    .execute(&admin)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO fvoci.task_dependencies (workspace_id, blocker_id, blocked_id, type, lag_days)
        SELECT $1, a.id, b.id, 'FS', 0
        FROM fvoci.tasks a
        JOIN fvoci.tasks b ON b.project_id = a.project_id AND a.number < b.number
        WHERE a.project_id = $2
        "#,
    )
    .bind(ws)
    .bind(pid)
    .execute(&admin)
    .await
    .unwrap();
    let lowest: Vec<(Uuid, Uuid)> = sqlx::query_as(
        r#"
        SELECT blocker_id, blocked_id
        FROM fvoci.task_dependencies
        WHERE workspace_id = $1
        ORDER BY blocker_id, blocked_id
        LIMIT 2048
        "#,
    )
    .bind(ws)
    .fetch_all(&admin)
    .await
    .unwrap();

    let layout = get_task_layout(app.clone(), ws, &project_id, "year=2026&month=9", &cookie).await;
    assert_eq!(layout["items"].as_array().unwrap().len(), 65);
    assert_eq!(layout["linkTotal"], 2080);
    assert_eq!(layout["pathTotal"], 2080);
    assert_eq!(layout["paths"].as_array().unwrap().len(), 2048);
    let kept: Vec<(String, String)> = layout_links(&layout)
        .into_iter()
        .map(|(blocker, blocked, _, _)| (blocker, blocked))
        .collect();
    let mut lowest: Vec<(String, String)> = lowest
        .into_iter()
        .map(|(blocker, blocked)| (blocker.to_string(), blocked.to_string()))
        .collect();
    lowest.sort();
    assert_eq!(kept, lowest);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_layout_calendar_lists_workspace_holidays_within_scale() {
    let (harness, app, cookie, ws, admin, project_id, _pid) = review_setup().await;
    for date in [
        "2026-08-29",
        "2026-08-30",
        "2026-09-02",
        "2026-10-03",
        "2026-10-04",
    ] {
        let (status, body) = json_request(
            app.clone(),
            "POST",
            &format!("/api/v1/workspaces/{ws}/holidays"),
            Some(json!({"date": date})),
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }
    // Another tenant's holiday inside the range must not leak in.
    let other_ws = insert_other_workspace(&admin).await;
    sqlx::query(
        "INSERT INTO fvoci.workspace_holidays (workspace_id, date) VALUES ($1, '2026-09-10')",
    )
    .bind(other_ws)
    .execute(&admin)
    .await
    .unwrap();

    // Sunday weeks: the scale is 2026-08-30..=2026-10-03, both ends included.
    let sunday = get_task_layout(
        app.clone(),
        ws,
        &project_id,
        "year=2026&month=9&weekStartsOn=0",
        &cookie,
    )
    .await;
    assert_eq!(sunday["scale"]["start"], "2026-08-30");
    assert_eq!(sunday["scale"]["end"], "2026-10-03");
    assert_eq!(
        sunday["calendar"],
        json!({"weekend": [0, 6], "holidays": ["2026-08-30", "2026-09-02", "2026-10-03"]})
    );

    // Monday weeks shift the window to 2026-08-31..=2026-10-04.
    let monday = get_task_layout(
        app.clone(),
        ws,
        &project_id,
        "year=2026&month=9&weekStartsOn=1",
        &cookie,
    )
    .await;
    assert_eq!(monday["scale"]["start"], "2026-08-31");
    assert_eq!(monday["scale"]["end"], "2026-10-04");
    assert_eq!(
        monday["calendar"]["holidays"],
        json!(["2026-09-02", "2026-10-03", "2026-10-04"])
    );

    admin.close().await;
    harness.cleanup().await;
}

/// The Gantt reschedules through PATCH /tasks/{id}, sending the layout item's
/// dates back as `expectedDates`. Pins the rules it relies on: only the sent
/// date fields change, a sent `dueAt` keeps the time of day the client chose,
/// a stale snapshot is 409 and a dependency contradiction is 400, and neither
/// refusal writes anything.
#[tokio::test]
async fn task_patch_gantt_reschedule_through_expected_dates() {
    let (harness, app, cookie, ws, admin, project_id, _pid) = review_setup().await;
    let month = "year=2026&month=9";
    let expected_of = |item: &serde_json::Value| {
        json!({
            "startDate": item["startDate"],
            "dueDate": item["dueDate"],
            "dueAt": item["dueAt"],
        })
    };
    let due_only = create_task_with_title(
        app.clone(),
        &cookie,
        ws,
        &project_id,
        json!({"title": "Due only", "dueDate": "2026-09-10"}),
    )
    .await;
    let due_only = due_only["id"].as_str().unwrap().to_string();
    let timed = create_task_with_title(
        app.clone(),
        &cookie,
        ws,
        &project_id,
        json!({"title": "Timed", "startDate": "2026-09-14"}),
    )
    .await;
    let timed = timed["id"].as_str().unwrap().to_string();
    let (status, body) = patch_task(
        app.clone(),
        ws,
        &timed,
        json!({"dueAt": "2026-09-15T09:30:00Z"}),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let before = get_task_layout(app.clone(), ws, &project_id, month, &cookie).await;

    // (a) Moving a due-date-only bar by three days writes only dueDate.
    let due_item = layout_item(&before, &due_only);
    assert_eq!(due_item["inferred"], "from-due");
    let stale_due_only = expected_of(due_item);
    let (status, body) = patch_task(
        app.clone(),
        ws,
        &due_only,
        json!({"dueDate": "2026-09-13", "expectedDates": stale_due_only}),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["startDate"], serde_json::Value::Null);
    assert_eq!(body["dueDate"], "2026-09-13");
    assert_eq!(body["dueAt"], serde_json::Value::Null);

    // (b) Moving a start + dueAt bar by three days: the client shifts dueAt by
    // whole days, and the stored value keeps 09:30 UTC.
    let timed_item = layout_item(&before, &timed);
    assert_eq!(timed_item["dueAt"], "2026-09-15T09:30:00.000Z");
    let (status, body) = patch_task(
        app.clone(),
        ws,
        &timed,
        json!({
            "startDate": "2026-09-17",
            "dueAt": "2026-09-18T09:30:00.000Z",
            "expectedDates": expected_of(timed_item),
        }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["dueDate"], serde_json::Value::Null);

    let after = get_task_layout(app.clone(), ws, &project_id, month, &cookie).await;
    let due_item = layout_item(&after, &due_only);
    assert_eq!(due_item["startDate"], serde_json::Value::Null);
    assert_eq!(due_item["dueDate"], "2026-09-13");
    assert_eq!(due_item["dueAt"], serde_json::Value::Null);
    assert_eq!(due_item["start"], "2026-09-13");
    assert_eq!(due_item["end"], "2026-09-13");
    let timed_item = layout_item(&after, &timed);
    assert_eq!(timed_item["startDate"], "2026-09-17");
    assert_eq!(timed_item["dueDate"], serde_json::Value::Null);
    assert_eq!(timed_item["dueAt"], "2026-09-18T09:30:00.000Z");
    assert_eq!(timed_item["start"], "2026-09-17");
    assert_eq!(timed_item["end"], "2026-09-18");

    // (c) A stale snapshot is refused and writes nothing.
    let events = count_rows(&admin, "events").await;
    let activity = count_rows(&admin, "task_activity").await;
    let (status, body) = patch_task(
        app.clone(),
        ws,
        &due_only,
        json!({"dueDate": "2026-09-20", "expectedDates": stale_due_only}),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "document_version_mismatch");
    assert_eq!(count_rows(&admin, "events").await, events);
    assert_eq!(count_rows(&admin, "task_activity").await, activity);

    // (d) Moving the blocked task before its blocker's due date is refused
    // even with a current snapshot, and writes nothing.
    let blocker = create_task_with_title(
        app.clone(),
        &cookie,
        ws,
        &project_id,
        json!({"title": "Blocker", "startDate": "2026-09-01", "dueDate": "2026-09-10"}),
    )
    .await;
    let blocker = blocker["id"].as_str().unwrap();
    let (status, body) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/tasks/{blocker}/dependencies"),
        Some(json!({"blockedId": timed, "type": "FS"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let events = count_rows(&admin, "events").await;
    let activity = count_rows(&admin, "task_activity").await;
    let (status, body) = patch_task(
        app.clone(),
        ws,
        &timed,
        json!({
            "startDate": "2026-09-08",
            "dueAt": "2026-09-09T09:30:00.000Z",
            "expectedDates": expected_of(timed_item),
        }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "dependency_contradiction");
    assert_eq!(count_rows(&admin, "events").await, events);
    assert_eq!(count_rows(&admin, "task_activity").await, activity);
    let unchanged = get_task_layout(app.clone(), ws, &project_id, month, &cookie).await;
    let timed_item = layout_item(&unchanged, &timed);
    assert_eq!(timed_item["startDate"], "2026-09-17");
    assert_eq!(timed_item["dueAt"], "2026-09-18T09:30:00.000Z");

    admin.close().await;
    harness.cleanup().await;
}

/// PATCH compares `expectedDates.dueAt` with the stored value to the
/// millisecond. Browsers hold `dueAt` in a JS `Date`, and the layout and
/// collection rows render it with milliseconds, so a `dueAt` an API client set
/// with sub-millisecond digits must not turn every reschedule into a 409; a
/// different millisecond is still a stale snapshot and writes nothing.
#[tokio::test]
async fn task_patch_expected_due_at_compares_to_the_millisecond() {
    let (harness, app, cookie, ws, admin, project_id, _pid) = review_setup().await;
    let month = "year=2026&month=9";
    let task = create_task_with_title(
        app.clone(),
        &cookie,
        ws,
        &project_id,
        json!({"title": "Micros", "startDate": "2026-09-14"}),
    )
    .await;
    let id = task["id"].as_str().unwrap().to_string();
    let (status, body) = patch_task(
        app.clone(),
        ws,
        &id,
        json!({"dueAt": "2026-09-15T09:30:00.123456Z"}),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let expected =
        |due_at: &str| json!({"startDate": "2026-09-14", "dueDate": null, "dueAt": due_at});

    // A neighbouring millisecond is a stale snapshot and writes nothing.
    let events = count_rows(&admin, "events").await;
    let activity = count_rows(&admin, "task_activity").await;
    for stale in ["2026-09-15T09:30:00.122Z", "2026-09-15T09:30:00.124Z"] {
        let (status, body) = patch_task(
            app.clone(),
            ws,
            &id,
            json!({"title": "Stale", "expectedDates": expected(stale)}),
            &cookie,
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{stale}: {body}");
        assert_eq!(body["code"], "document_version_mismatch");
    }
    assert_eq!(count_rows(&admin, "events").await, events);
    assert_eq!(count_rows(&admin, "task_activity").await, activity);

    // The same millisecond, as a browser sends it, matches.
    let (status, body) = patch_task(
        app.clone(),
        ws,
        &id,
        json!({
            "dueAt": "2026-09-16T09:30:00.123456Z",
            "expectedDates": expected("2026-09-15T09:30:00.123Z"),
        }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // The layout keeps its millisecond form, and sending it back matches.
    let layout = get_task_layout(app.clone(), ws, &project_id, month, &cookie).await;
    let item = layout_item(&layout, &id);
    assert_eq!(item["dueAt"], "2026-09-16T09:30:00.123Z");
    let (status, body) = patch_task(
        app.clone(),
        ws,
        &id,
        json!({
            "dueAt": "2026-09-17T09:30:00.123Z",
            "expectedDates": {
                "startDate": item["startDate"],
                "dueDate": item["dueDate"],
                "dueAt": item["dueAt"],
            },
        }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let layout = get_task_layout(app.clone(), ws, &project_id, month, &cookie).await;
    assert_eq!(
        layout_item(&layout, &id)["dueAt"],
        "2026-09-17T09:30:00.123Z"
    );

    admin.close().await;
    harness.cleanup().await;
}

async fn wait_for_hub_active(hub: &StreamHub, expected: usize, within: Duration) -> bool {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if hub.active_count() == expected {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    hub.active_count() == expected
}

async fn wait_for_task_hint_enqueued(
    workspace_id: Uuid,
    project_id: Uuid,
    min: usize,
    within: Duration,
) -> bool {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if task_stream_task_hint_enqueue_count(workspace_id, project_id) >= min {
            return true;
        }
        tokio::task::yield_now().await;
    }
    task_stream_task_hint_enqueue_count(workspace_id, project_id) >= min
}

async fn setup_session_with_hub(
    harness: &TestDb,
) -> (axum::Router, String, Uuid, Uuid, std::sync::Arc<StreamHub>) {
    let state = app_state(&harness.app_url).await;
    let hub = state.streams.clone();
    let app = fvoci_server::http::router(state, None);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/setup")
                .header("content-type", "application/json")
                .header("origin", "http://localhost")
                .extension(ConnectInfo(test_peer()))
                .body(Body::from(
                    json!({
                        "email": "sf-owner@example.com",
                        "password": "supersecret1",
                        "givenName": "SF",
                        "workspaceSlug": "sf-hub",
                        "workspaceName": "SF Hub"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .expect("setup");
    assert_eq!(response.status(), StatusCode::CREATED);
    let cookie_hdr = response
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .expect("set-cookie");
    let cookie = cookie_hdr
        .split(';')
        .next()
        .unwrap_or("")
        .split('=')
        .nth(1)
        .unwrap_or("")
        .to_string();
    let admin = admin_pool(harness).await;
    let owner_id: (Uuid,) = sqlx::query_as("SELECT id FROM fvoci.users WHERE email = $1")
        .bind("sf-owner@example.com")
        .fetch_one(&admin)
        .await
        .expect("owner");
    let workspace_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM fvoci.workspaces WHERE slug = 'sf-hub'")
            .fetch_one(&admin)
            .await
            .expect("workspace");
    admin.close().await;
    (app, cookie, owner_id.0, workspace_id.0, hub)
}

/// After `event: open`, signal `open_ready` then wait on `gate` and read until disconnect.
async fn sse_collect_after_open_gate(
    app: axum::Router,
    path: String,
    cookie: String,
    gate: tokio::sync::oneshot::Receiver<()>,
    open_ready: tokio::sync::oneshot::Sender<()>,
    within: Duration,
) -> Vec<u8> {
    let (done_tx, done_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let mut request = Request::builder()
            .method("GET")
            .uri(&path)
            .header("cookie", format!("fvoci_session={}", cookie))
            .header("origin", "http://localhost")
            .body(Body::empty())
            .expect("request");
        request.extensions_mut().insert(ConnectInfo(test_peer()));
        let response = app.oneshot(request).await.expect("sse response");
        if response.status() != StatusCode::OK {
            let _ = done_tx.send(Vec::new());
            return;
        }
        let mut stream = response.into_body().into_data_stream();
        let mut buf = Vec::new();
        let needle = b"event: open";
        let mut saw_open = false;
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => {
                    buf.extend_from_slice(&bytes);
                    if !saw_open && buf.windows(needle.len()).any(|w| w == needle) {
                        saw_open = true;
                    }
                }
                Err(_) => break,
            }
            if saw_open {
                break;
            }
        }
        if saw_open {
            let _ = open_ready.send(());
            let _ = gate.await;
            while let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(bytes) => buf.extend_from_slice(&bytes),
                    Err(_) => break,
                }
            }
        }
        let _ = done_tx.send(buf);
    });
    timeout(within, done_rx)
        .await
        .ok()
        .and_then(|r| r.ok())
        .unwrap_or_default()
}

/// Hold an SSE connection open after `event: open` without consuming further body bytes.
fn sse_stall_after_open(
    app: axum::Router,
    path: String,
    cookie: String,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut request = Request::builder()
            .method("GET")
            .uri(&path)
            .header("cookie", format!("fvoci_session={}", cookie))
            .header("origin", "http://localhost")
            .body(Body::empty())
            .expect("request");
        request.extensions_mut().insert(ConnectInfo(test_peer()));
        let response = app.oneshot(request).await.expect("sse response");
        if response.status() != StatusCode::OK {
            return;
        }
        let mut stream = response.into_body().into_data_stream();
        let mut buf = Vec::new();
        let needle = b"event: open";
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => {
                    buf.extend_from_slice(&bytes);
                    if buf.windows(needle.len()).any(|w| w == needle) {
                        break;
                    }
                }
                Err(_) => return,
            }
        }
        tokio::time::sleep(Duration::from_secs(120)).await;
    })
}

async fn sse_until_disconnect(
    app: axum::Router,
    path: String,
    cookie: String,
    within: Duration,
) -> bool {
    let (done_tx, done_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let mut request = Request::builder()
            .method("GET")
            .uri(&path)
            .header("cookie", format!("fvoci_session={}", cookie))
            .header("origin", "http://localhost")
            .body(Body::empty())
            .expect("request");
        request.extensions_mut().insert(ConnectInfo(test_peer()));
        let response = app.oneshot(request).await.expect("sse response");
        if response.status() != StatusCode::OK {
            let _ = done_tx.send(false);
            return;
        }
        let mut stream = response.into_body().into_data_stream();
        while stream.next().await.is_some() {}
        let _ = done_tx.send(true);
    });
    timeout(within, done_rx)
        .await
        .ok()
        .and_then(|r| r.ok())
        .unwrap_or(false)
}

async fn sse_listen(
    app: axum::Router,
    path: String,
    cookie: String,
    open_needle: Option<Vec<u8>>,
    open_notify: Option<tokio::sync::oneshot::Sender<()>>,
    hit_needle: Option<Vec<u8>>,
    within: Duration,
) -> bool {
    let mut request = Request::builder()
        .method("GET")
        .uri(&path)
        .header("cookie", format!("fvoci_session={}", cookie))
        .header("origin", "http://localhost")
        .body(Body::empty())
        .expect("request");
    request.extensions_mut().insert(ConnectInfo(test_peer()));
    let (done_tx, done_rx) = tokio::sync::oneshot::channel();
    let (opened_tx, opened_rx) = tokio::sync::oneshot::channel();
    let open_notify = open_notify;
    let wait_for_hit = hit_needle.is_some();
    tokio::spawn(async move {
        let response = app.oneshot(request).await.expect("sse response");
        let ct = response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if response.status() != StatusCode::OK || !ct.contains("text/event-stream") {
            let _ = opened_tx.send(false);
            let _ = done_tx.send(false);
            return;
        }
        let mut open_confirmed = open_needle.is_none();
        let mut open_notify = open_notify;
        let mut opened_signal = Some(opened_tx);
        if open_confirmed {
            if let Some(tx) = opened_signal.take() {
                let _ = tx.send(true);
            }
            if let Some(notify) = open_notify.take() {
                notify.send(()).ok();
            }
        }
        let mut stream = response.into_body().into_data_stream();
        let mut buf = Vec::new();
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => {
                    buf.extend_from_slice(&bytes);
                    if !open_confirmed {
                        if let Some(needle) = open_needle.as_ref() {
                            if buf.windows(needle.len()).any(|w| w == needle.as_slice()) {
                                open_confirmed = true;
                                if let Some(tx) = opened_signal.take() {
                                    let _ = tx.send(true);
                                }
                                if let Some(notify) = open_notify.take() {
                                    notify.send(()).ok();
                                }
                            }
                        }
                    }
                    if open_confirmed {
                        if let Some(needle) = hit_needle.as_ref() {
                            if buf.windows(needle.len()).any(|w| w == needle.as_slice()) {
                                done_tx.send(true).ok();
                                return;
                            }
                        }
                    }
                }
                Err(_) => break,
            }
        }
        if !open_confirmed {
            if let Some(tx) = opened_signal.take() {
                let _ = tx.send(false);
            }
        }
        let _ = done_tx.send(false);
    });
    let opened = timeout(within, opened_rx)
        .await
        .ok()
        .and_then(|r| r.ok())
        .unwrap_or(false);
    if !opened {
        return false;
    }
    if !wait_for_hit {
        return true;
    }
    timeout(within, done_rx)
        .await
        .ok()
        .and_then(|r| r.ok())
        .unwrap_or(false)
}

#[tokio::test]
async fn task_stream_notifies_viewers_on_other_user_task_create() {
    let harness = TestDb::bootstrap().await;
    let (app, _cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let owner = add_workspace_user(&admin, workspace_id, "member", "owner2").await;
    let peer = add_workspace_user(&admin, workspace_id, "member", "peer").await;
    let lab = create_project(app.clone(), &owner.cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let path = format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/stream");
    let (open_tx, open_rx) = tokio::sync::oneshot::channel();
    let listener = tokio::spawn(sse_listen(
        app.clone(),
        path.clone(),
        owner.cookie.clone(),
        Some(b"event: open".to_vec()),
        Some(open_tx),
        Some(b"event: task".to_vec()),
        Duration::from_secs(20),
    ));
    assert!(
        timeout(Duration::from_secs(5), open_rx)
            .await
            .ok()
            .and_then(|r| r.ok())
            .is_some(),
        "task stream should emit open before mutations"
    );
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({"title": "remote"})),
        Some(&peer.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(
        listener.await.expect("listener task"),
        "project stream should receive a task invalidation frame"
    );
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_stream_notifies_other_viewer_on_task_meta_date_and_status_patch() {
    let harness = TestDb::bootstrap().await;
    let (app, _cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let owner = add_workspace_user(&admin, workspace_id, "member", "meta-owner").await;
    let peer = add_workspace_user(&admin, workspace_id, "member", "meta-peer").await;
    let lab = create_project(app.clone(), &owner.cookie, workspace_id, "MSE", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let task = create_task_with_title(
        app.clone(),
        &owner.cookie,
        workspace_id,
        project_id,
        json!({"title": "meta stream"}),
    )
    .await;
    let task_id = task["id"].as_str().unwrap();
    let statuses = workflow_status_ids(app.clone(), &owner.cookie, workspace_id, project_id).await;
    let other_status = statuses
        .iter()
        .find(|(id, _)| id != task["statusId"].as_str().unwrap())
        .unwrap()
        .0
        .clone();
    let path = format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/stream");

    for body in [
        json!({"dueDate": "2026-04-01"}),
        json!({"statusId": other_status}),
    ] {
        let (open_tx, open_rx) = tokio::sync::oneshot::channel();
        let listener = tokio::spawn(sse_listen(
            app.clone(),
            path.clone(),
            owner.cookie.clone(),
            Some(b"event: open".to_vec()),
            Some(open_tx),
            Some(b"event: task".to_vec()),
            Duration::from_secs(20),
        ));
        assert!(
            timeout(Duration::from_secs(5), open_rx)
                .await
                .ok()
                .and_then(|r| r.ok())
                .is_some(),
            "task stream should emit open before mutations"
        );
        let (status, _) = patch_task(
            app.clone(),
            workspace_id,
            task_id,
            body.clone(),
            &peer.cookie,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(
            listener.await.expect("listener task"),
            "project stream should receive a task invalidation frame for {body}"
        );
    }
    admin.close().await;
    harness.cleanup().await;
}

/// The credential a direct poll checks, as the stream producer does.
#[derive(Clone, Copy)]
struct StreamCredential {
    user_id: Uuid,
    session_id: Uuid,
}

async fn stream_credential(admin: &sqlx::PgPool, user_id: Uuid) -> StreamCredential {
    StreamCredential {
        user_id,
        session_id: session_id_for_user(admin, user_id).await,
    }
}

/// One producer poll with a live credential.
async fn poll_task_page(
    pool: &sqlx::PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    credential: StreamCredential,
    cursor: &EventCursor,
    limit: i32,
) -> EventPage {
    poll_task_events(
        pool,
        workspace_id,
        project_id,
        credential.user_id,
        credential.session_id,
        cursor,
        limit,
    )
    .await
    .expect("poll")
    .expect("live credential")
}

async fn poll_task_rows(
    pool: &sqlx::PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    credential: StreamCredential,
    cursor: &EventCursor,
    limit: i32,
) -> Vec<fvoci_server::streams::StreamEventRow> {
    poll_task_page(pool, workspace_id, project_id, credential, cursor, limit)
        .await
        .rows
}

#[tokio::test]
async fn task_meta_and_move_events_reach_only_their_project_poll() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let stranger = add_workspace_user(&admin, workspace_id, "member", "meta-stranger").await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "MPA", "private").await;
    let project_a = lab["id"].as_str().unwrap().to_string();
    let other = create_project(app.clone(), &cookie, workspace_id, "MPB", "workspace").await;
    let project_b = Uuid::parse_str(other["id"].as_str().unwrap()).unwrap();
    let project_a_id = Uuid::parse_str(&project_a).unwrap();
    let task = create_task_with_title(
        app.clone(),
        &cookie,
        workspace_id,
        &project_a,
        json!({"title": "scoped"}),
    )
    .await;
    let task_id = task["id"].as_str().unwrap();
    let current_status = task["statusId"].as_str().unwrap().to_string();
    let statuses = workflow_status_ids(app.clone(), &cookie, workspace_id, &project_a).await;
    let next_status = statuses
        .iter()
        .find(|(id, _)| *id != current_status)
        .unwrap()
        .0
        .clone();
    let app_pool = pool::connect_app(&harness.app_url).await.expect("app pool");
    let cursor = initial_cursor(&app_pool).await.expect("cursor");
    let events_before = count_rows(&admin, "events").await;

    // Denied, conflicting and no-op patches must not manufacture events.
    let (status, _) = patch_task(
        app.clone(),
        workspace_id,
        task_id,
        json!({"dueDate": "2026-05-01"}),
        &stranger.cookie,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = patch_task(
        app.clone(),
        workspace_id,
        task_id,
        json!({
            "dueDate": "2026-05-01",
            "expectedDates": {"startDate": null, "dueDate": "2026-01-01", "dueAt": null}
        }),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = patch_task(
        app.clone(),
        workspace_id,
        task_id,
        json!({"statusId": current_status}),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(count_rows(&admin, "events").await, events_before);

    let (status, _) = patch_task(
        app.clone(),
        workspace_id,
        task_id,
        json!({"dueDate": "2026-05-01"}),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = patch_task(
        app.clone(),
        workspace_id,
        task_id,
        json!({"statusId": next_status}),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = patch_task(
        app.clone(),
        workspace_id,
        task_id,
        json!({"assigneeIds": [owner_id.to_string()]}),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/move"),
        Some(json!({"statusId": current_status})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Wait for the updates to pass the xmin gate, then poll once. The cursor is
    // itself xmin-bounded, so `task.created` can land in the window when another
    // test pinned xmin as it was taken: count updates only.
    settle_committed_events(&admin).await;
    let credential = stream_credential(&admin, owner_id).await;
    let rows = poll_task_rows(
        &app_pool,
        workspace_id,
        project_a_id,
        credential,
        &cursor,
        100,
    )
    .await;
    let updates: Vec<&serde_json::Value> = rows
        .iter()
        .filter(|r| r.verb == "task.updated")
        .map(|r| &r.payload)
        .collect();
    assert_eq!(updates.len(), 4, "{rows:?}");
    for payload in &updates {
        assert_eq!(payload["taskId"], task_id);
        assert_eq!(payload["projectId"], project_a);
    }
    assert_eq!(updates[0]["dueDate"], "2026-05-01");
    assert_eq!(updates[1]["from"], current_status);
    assert_eq!(updates[1]["to"], next_status);
    assert_eq!(updates[2]["assigneeIds"], json!([owner_id.to_string()]));
    assert_eq!(updates[3]["from"], next_status);
    assert_eq!(updates[3]["to"], current_status);

    let foreign =
        poll_task_rows(&app_pool, workspace_id, project_b, credential, &cursor, 100).await;
    assert!(foreign.is_empty(), "{foreign:?}");

    app_pool.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_update_poll_waits_for_update_held_behind_xmin() {
    // An update committed above the cluster-wide xmin stays hidden until xmin
    // passes it (CI once saw `task.created` + three updates at that point).
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "XMH", "workspace").await;
    let project = lab["id"].as_str().unwrap().to_string();
    let project_id = Uuid::parse_str(&project).unwrap();
    let task = create_task_with_title(
        app.clone(),
        &cookie,
        workspace_id,
        &project,
        json!({"title": "held"}),
    )
    .await;
    let task_id = task["id"].as_str().unwrap();
    let current_status = task["statusId"].as_str().unwrap().to_string();
    let statuses = workflow_status_ids(app.clone(), &cookie, workspace_id, &project).await;
    let next_status = statuses
        .iter()
        .find(|(id, _)| *id != current_status)
        .unwrap()
        .0
        .clone();
    for body in [
        json!({"dueDate": "2026-05-01"}),
        json!({"statusId": next_status}),
        json!({"assigneeIds": [owner_id.to_string()]}),
    ] {
        let (status, _) =
            patch_task(app.clone(), workspace_id, task_id, body.clone(), &cookie).await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }

    // Settle everything committed so far, then hold an xid so the move
    // commits above xmin until the hold ends.
    settle_committed_events(&admin).await;
    let mut hold = admin.begin().await.expect("hold tx");
    sqlx::query("SELECT pg_current_xact_id()")
        .execute(&mut *hold)
        .await
        .expect("hold xid");
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/move"),
        Some(json!({"statusId": current_status})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let app_pool = pool::connect_app(&harness.app_url).await.expect("app pool");
    let cursor = fvoci_server::streams::EventCursor::default();
    let credential = stream_credential(&admin, owner_id).await;
    let pending = poll_task_rows(
        &app_pool,
        workspace_id,
        project_id,
        credential,
        &cursor,
        100,
    )
    .await;
    let verbs: Vec<&str> = pending.iter().map(|r| r.verb.as_str()).collect();
    assert_eq!(
        verbs,
        [
            "task.created",
            "task.updated",
            "task.updated",
            "task.updated"
        ],
        "move must stay hidden while the hold is open: {pending:?}"
    );

    hold.commit().await.expect("release hold");
    settle_committed_events(&admin).await;
    let rows = poll_task_rows(
        &app_pool,
        workspace_id,
        project_id,
        credential,
        &cursor,
        100,
    )
    .await;
    let updates: Vec<&serde_json::Value> = rows
        .iter()
        .filter(|r| r.verb == "task.updated")
        .map(|r| &r.payload)
        .collect();
    assert_eq!(updates.len(), 4, "{rows:?}");
    assert_eq!(updates[3]["from"], next_status);
    assert_eq!(updates[3]["to"], current_status);
    assert_eq!(updates[3]["projectId"], project);

    app_pool.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_stream_denies_non_member_like_task_list() {
    let harness = TestDb::bootstrap().await;
    let (app, _cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let owner = add_workspace_user(&admin, workspace_id, "member", "priv-owner").await;
    let outsider = add_workspace_user(&admin, workspace_id, "member", "outsider").await;
    let lab = create_project(app.clone(), &owner.cookie, workspace_id, "PRV", "private").await;
    let project_id = lab["id"].as_str().unwrap();
    let (status, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/stream"),
        None,
        Some(&outsider.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_stream_rejects_65th_concurrent_subscriber() {
    let harness = TestDb::bootstrap().await;
    let state = app_state(&harness.app_url).await;
    let hub = state.streams.clone();
    let app = fvoci_server::http::router(state, None);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/setup")
                .header("content-type", "application/json")
                .header("origin", "http://localhost")
                .extension(ConnectInfo(test_peer()))
                .body(Body::from(
                    json!({
                        "email": "cap@example.com",
                        "password": "supersecret1",
                        "givenName": "Cap",
                        "workspaceSlug": "cap",
                        "workspaceName": "Cap"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .expect("setup");
    assert_eq!(response.status(), StatusCode::CREATED);
    let cookie = response
        .headers()
        .get_all(axum::http::header::SET_COOKIE)
        .iter()
        .find_map(|v| v.to_str().ok())
        .and_then(|s| s.split(';').next())
        .and_then(|s| s.strip_prefix("fvoci_session="))
        .expect("session");
    let (status, list) = json_request(
        app.clone(),
        "GET",
        "/api/v1/me/workspaces",
        None,
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let workspace_id = Uuid::parse_str(list["items"][0]["id"].as_str().unwrap()).unwrap();
    let lab = create_project(app.clone(), cookie, workspace_id, "CAP", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let path = format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/stream");
    let holds = (0..64).map(|_| {
        tokio::spawn(sse_listen(
            app.clone(),
            path.clone(),
            cookie.to_string(),
            Some(b"event: open".to_vec()),
            None,
            None,
            Duration::from_secs(10),
        ))
    });
    for hold in holds {
        assert!(hold.await.expect("hold task"), "subscriber should open");
    }
    assert!(
        wait_for_hub_active(&hub, 64, Duration::from_secs(3)).await,
        "expected 64 active stream guards, saw {}",
        hub.active_count()
    );
    let (status, _) = json_request(app.clone(), "GET", &path, None, Some(cookie)).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    hub.begin_shutdown();
    let (status, _) = json_request(app.clone(), "GET", &path, None, Some(cookie)).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    harness.cleanup().await;
}

#[tokio::test]
async fn task_stream_emits_activity_on_comment() {
    let harness = TestDb::bootstrap().await;
    let (app, _cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let owner = add_workspace_user(&admin, workspace_id, "member", "act-owner").await;
    let peer = add_workspace_user(&admin, workspace_id, "member", "act-peer").await;
    let lab = create_project(app.clone(), &owner.cookie, workspace_id, "ACT", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let (status, task) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({"title": "comment me"})),
        Some(&owner.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let task_id = task["id"].as_str().unwrap();
    let path = format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/stream");
    // The live tail starts at the cursor taken before `event: open`; mutate only after open.
    let (open_tx, open_rx) = tokio::sync::oneshot::channel();
    let listener = tokio::spawn(sse_listen(
        app.clone(),
        path,
        owner.cookie.clone(),
        Some(b"event: open".to_vec()),
        Some(open_tx),
        Some(b"task.activity".to_vec()),
        Duration::from_secs(20),
    ));
    assert!(
        timeout(Duration::from_secs(5), open_rx)
            .await
            .ok()
            .and_then(|r| r.ok())
            .is_some(),
        "task stream should emit open before comment mutation"
    );
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/comments"),
        Some(json!({"body": "stream ping"})),
        Some(&peer.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(
        listener.await.expect("listener"),
        "task stream should map comment.created to task.activity"
    );
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn workspace_access_stream_closes_when_member_removed() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let victim = add_workspace_user(&admin, workspace_id, "member", "access-victim").await;
    let path = format!("/api/v1/workspaces/{workspace_id}/access-stream");
    let disconnect = tokio::spawn(sse_until_disconnect(
        app.clone(),
        path,
        victim.cookie.clone(),
        Duration::from_secs(30),
    ));
    tokio::time::sleep(Duration::from_millis(900)).await;
    let (status, _) = json_request(
        app.clone(),
        "DELETE",
        &format!(
            "/api/v1/workspaces/{workspace_id}/members/{}",
            victim.user_id
        ),
        None,
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        disconnect.await.expect("disconnect task"),
        "access stream should end after workspace_member.removed"
    );
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_stream_slow_reader_does_not_block_fast_reader() {
    let harness = TestDb::bootstrap().await;
    let (app, _cookie, _, workspace_id, hub) = setup_session_with_hub(&harness).await;
    let admin = admin_pool(&harness).await;
    let owner = add_workspace_user(&admin, workspace_id, "member", "sf-owner").await;
    let peer = add_workspace_user(&admin, workspace_id, "member", "sf-peer").await;
    let lab = create_project(app.clone(), &owner.cookie, workspace_id, "SF", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let path = format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/stream");
    let _stall = sse_stall_after_open(app.clone(), path.clone(), owner.cookie.clone());
    assert!(
        wait_for_hub_active(&hub, 1, Duration::from_secs(5)).await,
        "slow reader should acquire stream capacity (saw {})",
        hub.active_count()
    );
    let fast = tokio::spawn(sse_listen(
        app.clone(),
        path,
        peer.cookie.clone(),
        Some(b"event: open".to_vec()),
        None,
        Some(b"event: task".to_vec()),
        Duration::from_secs(25),
    ));
    assert!(
        wait_for_hub_active(&hub, 2, Duration::from_secs(5)).await,
        "slow reader must hold hub capacity while HTTP body is alive (saw {})",
        hub.active_count()
    );
    for i in 0..12 {
        let (status, _) = json_request(
            app.clone(),
            "POST",
            &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
            Some(json!({"title": format!("flood-{i}")})),
            Some(&peer.cookie),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
    }
    assert!(
        fast.await.expect("fast listener"),
        "fast reader should still receive task invalidations while slow reader stalls"
    );
    admin.close().await;
    harness.cleanup().await;
}

/// Wait (bounded, read-only) until every transaction started so far has ended,
/// so committed events pass the stream's `xact < snapshot xmin` filter. xmin is
/// cluster-wide; on timeout, report the oldest holders.
async fn settle_committed_events(admin: &sqlx::PgPool) {
    let horizon: String = sqlx::query_scalar("SELECT pg_current_xact_id()::text")
        .fetch_one(admin)
        .await
        .expect("current xid");
    let deadline = Instant::now() + Duration::from_secs(15);
    while !sqlx::query_scalar::<_, bool>(
        "SELECT pg_snapshot_xmin(pg_current_snapshot()) > $1::xid8",
    )
    .bind(&horizon)
    .fetch_one(admin)
    .await
    .expect("snapshot xmin")
    {
        if Instant::now() >= deadline {
            let xmin: String =
                sqlx::query_scalar("SELECT pg_snapshot_xmin(pg_current_snapshot())::text")
                    .fetch_one(admin)
                    .await
                    .expect("snapshot xmin");
            let holders: Vec<XidHolder> = sqlx::query_as(
                "SELECT datname::text, pid, backend_xid::text, backend_xmin::text \
                     FROM pg_stat_activity \
                     WHERE backend_xid IS NOT NULL OR backend_xmin IS NOT NULL \
                     ORDER BY age(COALESCE(backend_xid, backend_xmin)) DESC LIMIT 5",
            )
            .fetch_all(admin)
            .await
            .expect("xmin holders");
            panic!("events never settled: xmin {xmin} <= {horizon}; oldest holders {holders:?}");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn task_stream_poll_skips_uncommitted_events_until_commit() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LC", "workspace").await;
    let project_id = Uuid::parse_str(lab["id"].as_str().unwrap()).unwrap();
    let (status, task) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({"title": "late"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let task_id = task["id"].as_str().unwrap();
    // Precondition: the baseline cursor must sit past `task.created`. Both the
    // cursor and poll read only settled events (xact < cluster-wide xmin), so
    // an unsettled create would reappear in the poll below as a committed row.
    settle_committed_events(&admin).await;
    let app_pool = pool::connect_app(&harness.app_url).await.expect("app pool");
    let cursor = initial_cursor(&app_pool).await.expect("cursor");
    let mut tx = admin.begin().await.expect("tx");
    sqlx::query("SELECT set_config('fvoci.workspace_id', $1::text, true)")
        .bind(workspace_id)
        .execute(&mut *tx)
        .await
        .expect("tenant");
    let event_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO fvoci.events (id, workspace_id, verb, target_type, target_id, actor_user_id, payload, channel)
        VALUES ($1, $2, 'task.updated', 'task', $3::uuid, $4, $5::jsonb, 'web')
        "#,
    )
    .bind(event_id)
    .bind(workspace_id)
    .bind(Uuid::parse_str(task_id).unwrap())
    .bind(owner_id)
    .bind(json!({"taskId": task_id, "projectId": project_id.to_string()}))
    .execute(&mut *tx)
    .await
    .expect("insert uncommitted");
    let credential = stream_credential(&admin, owner_id).await;
    let pending =
        poll_task_rows(&app_pool, workspace_id, project_id, credential, &cursor, 10).await;
    assert!(
        pending.is_empty(),
        "uncommitted event must not appear in poll snapshot \
         (uncommitted {event_id}, cursor {cursor:?}): {pending:?}"
    );
    tx.commit().await.expect("commit");
    settle_committed_events(&admin).await;
    let mut seen = false;
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let rows =
            poll_task_rows(&app_pool, workspace_id, project_id, credential, &cursor, 10).await;
        if rows.iter().any(|r| r.verb == "task.updated") {
            seen = true;
            break;
        }
    }
    assert!(seen, "committed event should become visible to poll");
    admin.close().await;
    harness.cleanup().await;
}

fn cursor_of(row: &fvoci_server::streams::StreamEventRow) -> EventCursor {
    EventCursor {
        xact: row.xact.clone(),
        seq: row.seq,
    }
}

/// Workspace events after `cursor`, as the admin sees them.
async fn events_after(admin: &sqlx::PgPool, workspace_id: Uuid, cursor: &EventCursor) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM fvoci.events WHERE workspace_id = $1 AND (xact, seq) > ($2::xid8, $3)",
    )
    .bind(workspace_id)
    .bind(&cursor.xact)
    .bind(cursor.seq)
    .fetch_one(admin)
    .await
    .expect("count events")
}

/// Inserts `count` events in one committed transaction: `task.created` rows
/// of `task_project`, or, with `None`, collab updates that no stream matches.
async fn insert_workspace_events(
    admin: &sqlx::PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    count: i32,
    task_project: Option<Uuid>,
) {
    let (verb, target_type) = match task_project {
        Some(_) => ("task.created", "task"),
        None => ("document.collab_update_appended", "document"),
    };
    sqlx::query(
        r#"
        INSERT INTO fvoci.events (id, workspace_id, verb, target_type, target_id, actor_user_id, payload, channel)
        SELECT gen_random_uuid(), $1, $2, $3, gen_random_uuid(), $4,
               CASE WHEN $6::uuid IS NULL THEN '{}'::jsonb
                    ELSE jsonb_build_object('taskId', gen_random_uuid()::text, 'projectId', $6::text)
               END,
               'web'
        FROM generate_series(1, $5)
        "#,
    )
    .bind(workspace_id)
    .bind(verb)
    .bind(target_type)
    .bind(actor_user_id)
    .bind(count)
    .bind(task_project)
    .execute(admin)
    .await
    .expect("insert events");
}

/// `next` may jump to the settled horizon but never past an event whose
/// transaction is still open: polling from it after the commit returns it.
#[tokio::test]
async fn task_stream_next_cursor_keeps_an_in_flight_event() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "SAF", "workspace").await;
    let project_id = Uuid::parse_str(lab["id"].as_str().unwrap()).unwrap();
    settle_committed_events(&admin).await;
    let app_pool = pool::connect_app(&harness.app_url).await.expect("app pool");
    let credential = stream_credential(&admin, owner_id).await;
    let start = initial_cursor(&app_pool).await.expect("cursor");
    let task_id = Uuid::now_v7().to_string();
    let mut tx = admin.begin().await.expect("tx");
    sqlx::query(
        r#"
        INSERT INTO fvoci.events (id, workspace_id, verb, target_type, target_id, actor_user_id, payload, channel)
        VALUES ($1, $2, 'task.updated', 'task', gen_random_uuid(), $3, $4::jsonb, 'web')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(workspace_id)
    .bind(owner_id)
    .bind(json!({"taskId": task_id, "projectId": project_id.to_string()}))
    .execute(&mut *tx)
    .await
    .expect("insert uncommitted");
    let in_flight: String = sqlx::query_scalar("SELECT pg_current_xact_id()::text")
        .fetch_one(&mut *tx)
        .await
        .expect("event xid");
    // A later transaction commits while the event's is still open, so the
    // newest completed xid is past the open one.
    sqlx::query("SELECT pg_current_xact_id()")
        .execute(&admin)
        .await
        .expect("later xid");
    let held = poll_task_page(&app_pool, workspace_id, project_id, credential, &start, 10).await;
    assert!(held.rows.is_empty(), "{:?}", held.rows);
    assert!(
        held.next.xact.parse::<u64>().expect("xid8") <= in_flight.parse::<u64>().expect("xid8"),
        "next {:?} passed the open transaction {in_flight}",
        held.next
    );
    tx.commit().await.expect("commit");
    settle_committed_events(&admin).await;
    let page = poll_task_page(
        &app_pool,
        workspace_id,
        project_id,
        credential,
        &held.next,
        10,
    )
    .await;
    assert!(
        page.rows.iter().any(|row| row.payload["taskId"] == task_id),
        "event committed after the poll must follow its next cursor {:?}: {:?}",
        held.next,
        page.rows
    );
    app_pool.close().await;
    admin.close().await;
    harness.cleanup().await;
}

/// Polls that match nothing still advance, so no poll rescans events it has
/// already passed (the access stream matches almost nothing).
#[tokio::test]
async fn stream_polls_advance_past_non_matching_events() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "PRG", "workspace").await;
    let project_id = Uuid::parse_str(lab["id"].as_str().unwrap()).unwrap();
    settle_committed_events(&admin).await;
    let app_pool = pool::connect_app(&harness.app_url).await.expect("app pool");
    let credential = stream_credential(&admin, owner_id).await;
    let start = initial_cursor(&app_pool).await.expect("cursor");
    insert_workspace_events(&admin, workspace_id, owner_id, 2_000, None).await;
    settle_committed_events(&admin).await;
    assert_eq!(events_after(&admin, workspace_id, &start).await, 2_000);

    let page = poll_task_page(&app_pool, workspace_id, project_id, credential, &start, 50).await;
    assert!(page.rows.is_empty(), "{:?}", page.rows);
    assert_eq!(
        events_after(&admin, workspace_id, &page.next).await,
        0,
        "task stream next cursor {:?}",
        page.next
    );
    let next = poll_access_events(
        &app_pool,
        workspace_id,
        credential.user_id,
        credential.session_id,
        &start,
    )
    .await
    .expect("access poll")
    .expect("still a member");
    assert_eq!(
        events_after(&admin, workspace_id, &next).await,
        0,
        "access stream next cursor {next:?}"
    );
    app_pool.close().await;
    admin.close().await;
    harness.cleanup().await;
}

/// A full page continues from its last row, so a burst larger than the
/// limit is read in order across polls. The limit is clamped to 100: a
/// larger request must not jump past the rows it did not return.
#[tokio::test]
async fn task_stream_full_page_continues_from_its_last_row() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LIM", "workspace").await;
    let project_id = Uuid::parse_str(lab["id"].as_str().unwrap()).unwrap();
    settle_committed_events(&admin).await;
    let app_pool = pool::connect_app(&harness.app_url).await.expect("app pool");
    let credential = stream_credential(&admin, owner_id).await;
    let start = initial_cursor(&app_pool).await.expect("cursor");
    insert_workspace_events(&admin, workspace_id, owner_id, 25, Some(project_id)).await;
    settle_committed_events(&admin).await;

    let mut cursor = start;
    let mut seen = Vec::new();
    for expected in [10, 10, 5] {
        let page =
            poll_task_page(&app_pool, workspace_id, project_id, credential, &cursor, 10).await;
        assert_eq!(page.rows.len(), expected, "{:?}", page.rows);
        if expected == 10 {
            assert_eq!(
                page.next,
                cursor_of(&page.rows[9]),
                "a full page ends at its 10th row"
            );
        }
        seen.extend(page.rows.iter().map(cursor_of));
        cursor = page.next;
    }
    assert_eq!(events_after(&admin, workspace_id, &cursor).await, 0);
    let order: Vec<(u64, i64)> = seen
        .iter()
        .map(|c| (c.xact.parse().expect("xid8"), c.seq))
        .collect();
    assert!(order.windows(2).all(|w| w[0] < w[1]), "{order:?}");

    insert_workspace_events(&admin, workspace_id, owner_id, 105, Some(project_id)).await;
    settle_committed_events(&admin).await;
    let clamped = poll_task_page(
        &app_pool,
        workspace_id,
        project_id,
        credential,
        &cursor,
        1_000,
    )
    .await;
    assert_eq!(clamped.rows.len(), 100);
    assert_eq!(clamped.next, cursor_of(&clamped.rows[99]));
    let rest = poll_task_page(
        &app_pool,
        workspace_id,
        project_id,
        credential,
        &clamped.next,
        1_000,
    )
    .await;
    assert_eq!(rest.rows.len(), 5);
    app_pool.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
/// Witness: hints enqueue while the HTTP consumer is blocked after `open`, then project
/// access is revoked before consumption; authorized poll must discard the bounded queue.
async fn task_stream_enqueue_before_revoke_discards_queued_hints() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let viewer = add_workspace_user(&admin, workspace_id, "member", "revoke-viewer").await;
    let lab = create_project(app.clone(), &owner_cookie, workspace_id, "REV", "private").await;
    let project_id = Uuid::parse_str(lab["id"].as_str().unwrap()).expect("project id");
    reset_task_stream_task_hint_enqueue_count(workspace_id, project_id);
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/members"),
        Some(json!({"userId": viewer.user_id.to_string(), "role": "viewer"})),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let path = format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/stream");
    let (gate_tx, gate_rx) = tokio::sync::oneshot::channel();
    let (open_ready_tx, open_ready_rx) = tokio::sync::oneshot::channel();
    let collector = tokio::spawn(sse_collect_after_open_gate(
        app.clone(),
        path,
        viewer.cookie.clone(),
        gate_rx,
        open_ready_tx,
        Duration::from_secs(30),
    ));
    assert!(
        timeout(Duration::from_secs(15), open_ready_rx)
            .await
            .expect("open-ready timeout")
            .is_ok(),
        "collector must consume event: open before task mutations"
    );
    for i in 0..10 {
        let (status, _) = json_request(
            app.clone(),
            "POST",
            &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
            Some(json!({"title": format!("queued-{i}")})),
            Some(&owner_cookie),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        if task_stream_task_hint_enqueue_count(workspace_id, project_id) >= 1 {
            break;
        }
    }
    assert!(
        wait_for_task_hint_enqueued(workspace_id, project_id, 1, Duration::from_secs(20)).await,
        "expected at least one queued task hint before revoke (saw {})",
        task_stream_task_hint_enqueue_count(workspace_id, project_id)
    );
    let (status, _) = json_request(
        app.clone(),
        "DELETE",
        &format!(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/members/{}",
            viewer.user_id
        ),
        None,
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    gate_tx.send(()).ok();
    let body = collector.await.expect("collector");
    assert!(
        body.windows(b"event: open".len())
            .any(|w| w == b"event: open"),
        "open must be delivered before revoke witness gate"
    );
    assert!(
        !body
            .windows(b"event: task".len())
            .any(|w| w == b"event: task"),
        "enqueue-before-revoke hints must not reach the response body after authorization loss"
    );
    let _ = owner_id;
    admin.close().await;
    harness.cleanup().await;
}

/// Opens an SSE stream with one credential header and returns once the server
/// answered: `Some(reader)` for an admitted stream, where `reader` finishes when
/// the server ends the body; `None` for any other status.
async fn sse_admit(
    app: axum::Router,
    path: &str,
    credential: (&str, String),
) -> Option<tokio::task::JoinHandle<()>> {
    let mut request = Request::builder()
        .method("GET")
        .uri(path)
        .header(credential.0, credential.1)
        .header("origin", "http://localhost")
        .body(Body::empty())
        .expect("request");
    request.extensions_mut().insert(ConnectInfo(test_peer()));
    let response = app.oneshot(request).await.expect("sse response");
    if response.status() != StatusCode::OK {
        return None;
    }
    let mut body = response.into_body().into_data_stream();
    Some(tokio::spawn(
        async move { while body.next().await.is_some() {} },
    ))
}

fn session_cookie_header(cookie: &str) -> (&'static str, String) {
    ("cookie", format!("fvoci_session={cookie}"))
}

/// Inserts a committed `task.created` event for `project_id` as the admin
/// (no project lock), the shape `record_task_event_and_audit` writes.
async fn insert_task_created_event(
    admin: &sqlx::PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_user_id: Uuid,
) -> Uuid {
    let task_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO fvoci.events (id, workspace_id, verb, target_type, target_id, actor_user_id, payload, channel)
        VALUES ($1, $2, 'task.created', 'task', $3, $4, $5::jsonb, 'web')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(workspace_id)
    .bind(task_id)
    .bind(actor_user_id)
    .bind(json!({"taskId": task_id.to_string(), "projectId": project_id.to_string()}))
    .execute(admin)
    .await
    .expect("insert task event");
    task_id
}

/// Stream admission and per-hint delivery read the project without a row
/// lock, so a writer holding the project row (here: every row, since any row
/// lock needs ROW SHARE and EXCLUSIVE refuses it) cannot stall them. The table
/// lock takes no xid, so the inserted event still settles past the xmin gate.
#[tokio::test]
async fn task_stream_admits_and_delivers_while_project_rows_are_locked() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "NRL", "workspace").await;
    let project_id = Uuid::parse_str(lab["id"].as_str().unwrap()).unwrap();
    let mut hold = admin.begin().await.expect("hold tx");
    sqlx::query("LOCK TABLE fvoci.projects IN EXCLUSIVE MODE")
        .execute(&mut *hold)
        .await
        .expect("lock projects");
    let path = format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/stream");
    let (open_tx, open_rx) = tokio::sync::oneshot::channel();
    let listener = tokio::spawn(sse_listen(
        app.clone(),
        path,
        cookie.clone(),
        Some(b"event: open".to_vec()),
        Some(open_tx),
        Some(b"event: task".to_vec()),
        Duration::from_secs(20),
    ));
    assert!(
        timeout(Duration::from_secs(5), open_rx)
            .await
            .ok()
            .and_then(|r| r.ok())
            .is_some(),
        "stream admission and the open frame must not wait for a project row lock"
    );
    let task_id = insert_task_created_event(&admin, workspace_id, project_id, owner_id).await;
    assert!(
        listener.await.expect("listener"),
        "hint for {task_id} must be delivered while the project rows are locked"
    );
    hold.rollback().await.expect("release");
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn workspace_access_stream_closes_when_workspace_trashed() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, workspace_id, "member", "trash-member").await;
    let path = format!("/api/v1/workspaces/{workspace_id}/access-stream");
    let reader = sse_admit(app.clone(), &path, session_cookie_header(&member.cookie))
        .await
        .expect("member admitted to the access stream");
    let (status, body) = json_request(
        app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{workspace_id}"),
        Some(json!({"confirmSlug": "acme"})),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        timeout(Duration::from_secs(10), reader).await.is_ok(),
        "a member's access stream must end once the workspace is trashed"
    );
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn workspace_access_stream_closes_when_member_role_changed() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, workspace_id, "member", "role-member").await;
    let bystander = add_workspace_user(&admin, workspace_id, "member", "role-bystander").await;
    let path = format!("/api/v1/workspaces/{workspace_id}/access-stream");
    let reader = sse_admit(app.clone(), &path, session_cookie_header(&member.cookie))
        .await
        .expect("member admitted to the access stream");
    let bystander_reader = sse_admit(app.clone(), &path, session_cookie_header(&bystander.cookie))
        .await
        .expect("bystander admitted to the access stream");
    let (status, body) = json_request(
        app.clone(),
        "PATCH",
        &format!(
            "/api/v1/workspaces/{workspace_id}/members/{}",
            member.user_id
        ),
        Some(json!({"role": "admin"})),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        timeout(Duration::from_secs(10), reader).await.is_ok(),
        "the member's access stream must end after its role changed"
    );
    assert!(
        !bystander_reader.is_finished(),
        "another member's role change must not end this member's stream"
    );
    bystander_reader.abort();
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn workspace_access_stream_closes_when_user_suspended() {
    let harness = TestDb::bootstrap().await;
    let (app, admin_cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, workspace_id, "member", "suspend-member").await;
    let path = format!("/api/v1/workspaces/{workspace_id}/access-stream");
    let reader = sse_admit(app.clone(), &path, session_cookie_header(&member.cookie))
        .await
        .expect("member admitted to the access stream");
    let (status, body) = json_request(
        app.clone(),
        "PATCH",
        "/api/v1/admin/users",
        Some(json!({"userId": member.user_id, "suspended": true})),
        Some(&admin_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        timeout(Duration::from_secs(10), reader).await.is_ok(),
        "a suspended user's access stream must end"
    );
    admin.close().await;
    harness.cleanup().await;
}

/// api_tokens has RLS: the per-tick credential check must run under the
/// workspace tenant or a live token reads as dead (and a dead one as live).
#[tokio::test]
async fn task_stream_with_bearer_token_closes_when_token_deleted() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "BTK", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let (status, token) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/api-tokens"),
        Some(json!({"name": "stream", "scopes": ["tasks.read"]})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{token}");
    let secret = token["token"].as_str().expect("secret").to_string();
    let path = format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/stream");
    let reader = sse_admit(
        app.clone(),
        &path,
        ("authorization", format!("Bearer {secret}")),
    )
    .await
    .expect("bearer admitted to the task stream");
    // A live token keeps the stream open across several ticks.
    tokio::time::sleep(Duration::from_millis(2_000)).await;
    assert!(
        !reader.is_finished(),
        "a live token's stream must stay open"
    );
    let (status, body) = json_request(
        app.clone(),
        "DELETE",
        &format!(
            "/api/v1/workspaces/{workspace_id}/api-tokens/{}",
            token["id"].as_str().unwrap()
        ),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        timeout(Duration::from_secs(10), reader).await.is_ok(),
        "the stream must end once its bearer token is deleted"
    );
    admin.close().await;
    harness.cleanup().await;
}

async fn set_user_time_zone(admin: &sqlx::PgPool, user_id: Uuid, zone: &str) {
    sqlx::query("UPDATE fvoci.users SET timezone = $2 WHERE id = $1")
        .bind(user_id)
        .bind(zone)
        .execute(admin)
        .await
        .unwrap();
}

async fn set_task_due(
    admin: &sqlx::PgPool,
    task: &serde_json::Value,
    due_date: Option<&str>,
    due_at: Option<&str>,
) {
    sqlx::query(
        "UPDATE fvoci.tasks SET due_date = $2::date, due_at = $3::timestamptz WHERE id = $1::uuid",
    )
    .bind(task["id"].as_str().unwrap())
    .bind(due_date)
    .bind(due_at)
    .execute(admin)
    .await
    .unwrap();
}

#[tokio::test]
async fn task_list_due_sort_and_cursor_use_actor_time_zone() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    // (title, due_date, due_at): Seoul dates 01-02, 01-01, 01-02, 01-02, 01-03;
    // the UTC dates of the dueAt rows are 01-01, 01-02 and 01-02.
    let mut tasks = Vec::new();
    for (title, due_date, due_at) in [
        ("P", None, Some("2026-01-01T16:00:00Z")),
        ("Q", Some("2026-01-01"), None),
        ("R", Some("2026-01-02"), None),
        ("S", None, Some("2026-01-02T10:00:00Z")),
        ("T", None, Some("2026-01-02T16:00:00Z")),
    ] {
        let task = create_task_with_title(
            app.clone(),
            &cookie,
            workspace_id,
            project_id,
            json!({"title": title}),
        )
        .await;
        set_task_due(&admin, &task, due_date, due_at).await;
        tasks.push(task["id"].as_str().unwrap().to_string());
    }
    let [p, q, r, s, t] = <[String; 5]>::try_from(tasks).unwrap();
    set_user_time_zone(&admin, owner_id, "Asia/Seoul").await;

    let query = r#"{"sort":[{"field":"due","direction":"asc"}]}"#;
    let expected = vec![q.clone(), p.clone(), r.clone(), s.clone(), t.clone()];
    let full =
        all_task_ids_unpaginated(app.clone(), workspace_id, project_id, &cookie, query).await;
    assert_eq!(full, expected, "due sort in the actor zone, ties by id");
    assert_pagination_walk_matches_unpaginated(
        app.clone(),
        workspace_id,
        project_id,
        &cookie,
        query,
    )
    .await;
    let desc = r#"{"sort":[{"field":"due","direction":"desc"}]}"#;
    let full_desc =
        all_task_ids_unpaginated(app.clone(), workspace_id, project_id, &cookie, desc).await;
    assert_eq!(
        full_desc,
        vec![t.clone(), p.clone(), r.clone(), s.clone(), q.clone()]
    );
    assert_pagination_walk_matches_unpaginated(
        app.clone(),
        workspace_id,
        project_id,
        &cookie,
        desc,
    )
    .await;
    let filtered =
        r#"{"filters":{"dueBefore":"2026-01-02"},"sort":[{"field":"due","direction":"asc"}]}"#;
    assert_eq!(
        all_task_ids_unpaginated(app.clone(), workspace_id, project_id, &cookie, filtered).await,
        vec![q.clone(), p.clone(), r.clone(), s.clone()]
    );
    assert_pagination_walk_matches_unpaginated(
        app.clone(),
        workspace_id,
        project_id,
        &cookie,
        filtered,
    )
    .await;

    // A cursor issued under one zone does not continue under another: the
    // filter and due order it encodes would silently change meaning. The
    // anchor (Q) is date-only, so its own sort key is the same in both zones.
    let (status, page1) = list_tasks_page(
        app.clone(),
        workspace_id,
        project_id,
        &cookie,
        query,
        1,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page1["items"][0]["id"], q.as_str());
    let cursor = page1["nextCursor"].as_str().unwrap().to_string();
    let (status, same) = list_tasks_page(
        app.clone(),
        workspace_id,
        project_id,
        &cookie,
        query,
        1,
        Some(&cursor),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{same}");
    set_user_time_zone(&admin, owner_id, "America/Los_Angeles").await;
    let (status, body) = list_tasks_page(
        app.clone(),
        workspace_id,
        project_id,
        &cookie,
        query,
        1,
        Some(&cursor),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["params"]["code"], "invalid_cursor");
    // Los Angeles dates: P 01-01, Q 01-01, R 01-02, S 01-02, T 01-02.
    let la = all_task_ids_unpaginated(app.clone(), workspace_id, project_id, &cookie, query).await;
    assert_eq!(
        la,
        vec![p.clone(), q.clone(), r.clone(), s.clone(), t.clone()]
    );
    assert_pagination_walk_matches_unpaginated(
        app.clone(),
        workspace_id,
        project_id,
        &cookie,
        query,
    )
    .await;

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_list_and_layout_unknown_time_zone_falls_back_to_utc() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let evening = create_task_with_title(
        app.clone(),
        &cookie,
        workspace_id,
        project_id,
        json!({"title": "Evening"}),
    )
    .await;
    set_task_due(&admin, &evening, None, Some("2026-01-01T20:00:00Z")).await;
    let later = create_task_with_title(
        app.clone(),
        &cookie,
        workspace_id,
        project_id,
        json!({"title": "Later"}),
    )
    .await;
    set_task_due(&admin, &later, Some("2026-01-02"), None).await;
    // Stored zone names are free text; an unknown one reads as UTC.
    set_user_time_zone(&admin, owner_id, "Mars/Olympus_Mons").await;

    let query =
        r#"{"filters":{"dueBefore":"2026-01-01"},"sort":[{"field":"due","direction":"asc"}]}"#;
    let ids = all_task_ids_unpaginated(app.clone(), workspace_id, project_id, &cookie, query).await;
    assert_eq!(ids, vec![evening["id"].as_str().unwrap().to_string()]);
    let layout = form_urlencoded::Serializer::new(String::new())
        .append_pair("year", "2026")
        .append_pair("month", "1")
        .append_pair("query", query)
        .finish();
    let (status, body) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/task-layout?{layout}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let layout_ids: Vec<&str> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect();
    assert_eq!(layout_ids, vec![evening["id"].as_str().unwrap()]);

    admin.close().await;
    harness.cleanup().await;
}
