#![cfg(feature = "db-tests")]
#![allow(dead_code)]

#[path = "support/project_harness.rs"]
mod project_harness;

use std::time::Duration;

use axum::http::StatusCode;
use project_harness::{
    add_workspace_user, admin_pool, count_rows, create_project, drop_insert_fail_trigger,
    insert_minimal_project, insert_project_document, install_insert_fail_trigger, json_request,
    setup_session, wait_for_query_blocked_by, wait_for_user_for_update_blocked, TestDb,
};
use serde_json::json;
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
    let (app, owner_cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lead = add_workspace_user(&admin, workspace_id, "member", "lead").await;
    let other = add_workspace_user(&admin, workspace_id, "member", "other").await;
    let lab = create_project(app.clone(), &lead.cookie, workspace_id, "LAB", "workspace").await;
    let project_id = Uuid::parse_str(lab["id"].as_str().unwrap()).unwrap();
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
        let owner_cookie = owner_cookie.clone();
        async move {
            json_request(
                app,
                "PATCH",
                &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}"),
                Some(json!({"visibility":"private"})),
                Some(&owner_cookie),
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
    assert_eq!(count_rows(&admin, "events").await, events_before);
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

    let (status, page1) = json_request(
        app.clone(),
        "GET",
        &format!(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks?limit=2&query={{\"sort\":[{{\"field\":\"number\",\"direction\":\"asc\"}}]}}"
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
    let (status, page2) = json_request(
        app,
        "GET",
        &format!(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks?limit=2&query={{\"sort\":[{{\"field\":\"number\",\"direction\":\"asc\"}}]}}&cursor={cursor}"
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
