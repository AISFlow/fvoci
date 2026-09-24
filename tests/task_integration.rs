#![cfg(feature = "db-tests")]
#![allow(dead_code)]

#[path = "support/project_harness.rs"]
mod project_harness;

use std::collections::HashSet;
use std::time::Duration;

use axum::http::StatusCode;
use project_harness::{
    add_workspace_user, admin_pool, count_rows, create_project, drop_insert_fail_trigger,
    insert_minimal_project, insert_project_document, install_insert_fail_trigger, json_request,
    setup_session, wait_for_query_blocked_by, wait_for_user_for_update_blocked, TestDb,
};
use serde_json::json;
use url::form_urlencoded;
use uuid::Uuid;

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

    let (status, _) = json_request(
        app,
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{lab_id}/tasks"),
        Some(json!({"title":"Bad status", "statusId": foreign_status})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
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
async fn task_create_rejects_unsupported_milestone_and_recurrence_blob() {
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
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(problem["code"], "invalid_input");

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

    for body in [
        json!({"assigneeIds": []}),
        json!({"labelIds": []}),
        json!({"milestoneId": Uuid::now_v7().to_string()}),
    ] {
        let (status, problem) = patch_task(app.clone(), workspace_id, task_id, body, &cookie).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(problem["code"], "invalid_input");
    }

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
    assert_eq!(problem["code"], "conflict");
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
