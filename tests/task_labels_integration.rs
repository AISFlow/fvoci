#![cfg(feature = "db-tests")]
#![allow(dead_code)]

#[allow(dead_code)]
#[path = "support/project_harness.rs"]
mod project_harness;

use axum::http::StatusCode;
use project_harness::{
    add_workspace_user, admin_pool, count_rows, create_project, json_request, setup_session, TestDb,
};
use serde_json::json;
use url::form_urlencoded;
use uuid::Uuid;

async fn create_task(
    app: axum::Router,
    cookie: &str,
    workspace_id: Uuid,
    project_id: &str,
    title: &str,
) -> serde_json::Value {
    let (status, task) = json_request(
        app,
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({"title": title})),
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    task
}

async fn create_label(
    app: axum::Router,
    cookie: &str,
    workspace_id: Uuid,
    project_id: &str,
    name: &str,
    color: &str,
) -> (StatusCode, serde_json::Value) {
    json_request(
        app,
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/labels"),
        Some(json!({"name": name, "color": color})),
        Some(cookie),
    )
    .await
}

fn encode_query(raw: &str) -> String {
    form_urlencoded::byte_serialize(raw.as_bytes()).collect()
}

#[tokio::test]
async fn labels_crud_and_task_attach_match_source_contract() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let task = create_task(app.clone(), &cookie, workspace_id, project_id, "일").await;
    let task_id = task["id"].as_str().unwrap();

    let (status, created) = create_label(
        app.clone(),
        &cookie,
        workspace_id,
        project_id,
        " 긴급 ",
        "red",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["name"], "긴급");
    assert_eq!(created["color"], "red");
    assert_eq!(created["projectId"], project_id);
    let label_id = created["id"].as_str().unwrap().to_string();

    let (status, dup) = create_label(
        app.clone(),
        &cookie,
        workspace_id,
        project_id,
        "긴급",
        "blue",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{dup}");
    assert_ne!(dup["id"], created["id"]);

    let (status, listed) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/labels"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["items"].as_array().unwrap().len(), 2);

    let (status, ws_listed) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/labels"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(ws_listed["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["id"] == label_id));

    let (status, updated) = json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/labels/{label_id}"),
        Some(json!({"name": "중요", "color": "amber"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["ok"], true);

    let events_before = count_rows(&admin, "events").await;
    let (status, patched) = json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}"),
        Some(json!({
            "assigneeIds": [owner_id.to_string()],
            "labelIds": [label_id]
        })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{patched}");
    let (status, detail) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["assigneeIds"], json!([owner_id.to_string()]));
    assert_eq!(detail["labelIds"], json!([label_id]));
    assert!(count_rows(&admin, "events").await >= events_before + 2);

    let payloads: Vec<(String, serde_json::Value)> = sqlx::query_as(
        "SELECT verb, payload FROM fvoci.events WHERE target_id = $1::uuid ORDER BY created_at",
    )
    .bind(Uuid::parse_str(task_id).unwrap())
    .fetch_all(&admin)
    .await
    .unwrap();
    let assignee_event = payloads
        .iter()
        .find(|(_, payload)| payload.get("assigneeIds").is_some())
        .unwrap();
    assert_eq!(assignee_event.0, "task.updated");
    assert_eq!(
        assignee_event.1["addedAssigneeIds"],
        json!([owner_id.to_string()])
    );
    let label_event = payloads
        .iter()
        .find(|(_, payload)| payload.get("labelIds").is_some())
        .unwrap();
    assert_eq!(label_event.0, "task.updated");
    assert_eq!(label_event.1["labelIds"], json!([label_id]));

    let (status, cleared) = json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}"),
        Some(json!({"assigneeIds": [], "labelIds": []})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{cleared}");
    let (_, detail) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}"),
        None,
        Some(&cookie),
    )
    .await;
    assert!(detail["assigneeIds"].as_array().unwrap().is_empty());
    assert!(detail["labelIds"].as_array().unwrap().is_empty());

    let (status, deleted) = json_request(
        app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/labels/{label_id}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{deleted}");
    assert_eq!(deleted["ok"], true);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn assignee_and_label_errors_and_filters() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, workspace_id, "member", "member").await;
    let outsider = Uuid::now_v7();
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let other = create_project(app.clone(), &cookie, workspace_id, "OTH", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let other_id = other["id"].as_str().unwrap();
    let task_a = create_task(app.clone(), &cookie, workspace_id, project_id, "A").await;
    let task_b = create_task(app.clone(), &cookie, workspace_id, project_id, "B").await;
    let a_id = task_a["id"].as_str().unwrap();
    let b_id = task_b["id"].as_str().unwrap();

    let (status, label) = create_label(
        app.clone(),
        &cookie,
        workspace_id,
        project_id,
        "버그",
        "red",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let label_id = label["id"].as_str().unwrap();
    let (status, other_label) =
        create_label(app.clone(), &cookie, workspace_id, other_id, "다른", "gray").await;
    assert_eq!(status, StatusCode::CREATED);
    let other_label_id = other_label["id"].as_str().unwrap();

    let (status, problem) = json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{a_id}"),
        Some(json!({"assigneeIds": [outsider.to_string()]})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{problem}");
    assert_eq!(problem["code"], "assignee_is_not_a_member");

    let (status, problem) = json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{a_id}"),
        Some(json!({"labelIds": [other_label_id]})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{problem}");
    assert_eq!(problem["code"], "not_found");

    json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{a_id}"),
        Some(json!({
            "assigneeIds": [owner_id.to_string()],
            "labelIds": [label_id]
        })),
        Some(&cookie),
    )
    .await;
    json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{b_id}"),
        Some(json!({"assigneeIds": [member.user_id.to_string()]})),
        Some(&cookie),
    )
    .await;

    let me_query = encode_query(r#"{"filters":{"assigneeId":"me"}}"#);
    let (status, mine) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks?query={me_query}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{mine}");
    let mine_ids: Vec<_> = mine["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(mine_ids, vec![a_id.to_string()]);
    assert_eq!(
        mine["items"][0]["assigneeIds"],
        json!([owner_id.to_string()])
    );
    assert_eq!(mine["items"][0]["labelIds"], json!([label_id]));

    let label_query = encode_query(&format!(r#"{{"filters":{{"labelId":"{label_id}"}}}}"#));
    let (status, labeled) = json_request(
        app.clone(),
        "GET",
        &format!(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks?query={label_query}"
        ),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{labeled}");
    assert_eq!(labeled["items"].as_array().unwrap().len(), 1);
    assert_eq!(labeled["items"][0]["id"], a_id);

    let bad_label = encode_query(&format!(
        r#"{{"filters":{{"labelId":"{}"}}}}"#,
        Uuid::now_v7()
    ));
    let (status, problem) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks?query={bad_label}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{problem}");
    assert_eq!(problem["code"], "invalid_input");

    let bad_assignee = encode_query(&format!(
        r#"{{"filters":{{"assigneeId":"{}"}}}}"#,
        Uuid::now_v7()
    ));
    let (status, problem) = json_request(
        app.clone(),
        "GET",
        &format!(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks?query={bad_assignee}"
        ),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{problem}");
    assert_eq!(problem["code"], "invalid_input");

    sqlx::query("DELETE FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2")
        .bind(workspace_id)
        .bind(member.user_id)
        .execute(&admin)
        .await
        .unwrap();
    let (_, detail) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{b_id}"),
        None,
        Some(&cookie),
    )
    .await;
    assert!(detail["assigneeIds"].as_array().unwrap().is_empty());

    json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{a_id}"),
        Some(json!({"labelIds": [label_id]})),
        Some(&cookie),
    )
    .await;
    json_request(
        app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/labels/{label_id}"),
        None,
        Some(&cookie),
    )
    .await;
    let (_, detail) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{a_id}"),
        None,
        Some(&cookie),
    )
    .await;
    assert!(detail["labelIds"].as_array().unwrap().is_empty());

    let (status, bad_color) = create_label(
        app.clone(),
        &cookie,
        workspace_id,
        project_id,
        "x",
        "not-a-color",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{bad_color}");

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn recurrence_copies_assignees_and_labels() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id) = setup_session(&harness).await;
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
    let done = workflow["statuses"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["category"] == "done")
        .unwrap()["id"]
        .as_str()
        .unwrap();
    let (status, created) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({"title": "R", "recurrence": {"kind": "daily"}})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let task_id = created["id"].as_str().unwrap();
    let (status, label) = create_label(
        app.clone(),
        &cookie,
        workspace_id,
        project_id,
        "반복",
        "teal",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let label_id = label["id"].as_str().unwrap();
    json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}"),
        Some(json!({
            "assigneeIds": [owner_id.to_string()],
            "labelIds": [label_id]
        })),
        Some(&cookie),
    )
    .await;
    let (status, moved) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/move"),
        Some(json!({"statusId": done})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{moved}");
    let ids: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM fvoci.tasks WHERE project_id = $1 AND deleted_at IS NULL ORDER BY number",
    )
    .bind(Uuid::parse_str(project_id).unwrap())
    .fetch_all(&admin)
    .await
    .unwrap();
    assert_eq!(ids.len(), 2);
    let next_id = ids[1].0;
    let (status, next) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{next_id}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{next}");
    assert_eq!(next["assigneeIds"], json!([owner_id.to_string()]));
    assert_eq!(next["labelIds"], json!([label_id]));

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn labels_force_rls_and_pat_scopes() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let flags: Vec<(String, bool, bool)> = sqlx::query_as(
        r#"
        SELECT c.relname, c.relrowsecurity, c.relforcerowsecurity
        FROM pg_class c
        INNER JOIN pg_namespace n ON n.oid = c.relnamespace
        WHERE n.nspname = 'fvoci'
          AND c.relname IN ('labels', 'task_assignees', 'task_labels')
        ORDER BY c.relname
        "#,
    )
    .fetch_all(&admin)
    .await
    .unwrap();
    assert_eq!(flags.len(), 3);
    for (name, rls, force) in &flags {
        assert!(rls, "{name} missing RLS");
        assert!(force, "{name} missing FORCE RLS");
    }

    let path = format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/labels");
    let (status, token) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/api-tokens"),
        Some(json!({"name": "read", "scopes": ["tasks.read"]})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{token}");
    let read_token = token["token"].as_str().unwrap();
    let (status, projects_token) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/api-tokens"),
        Some(json!({"name": "proj", "scopes": ["projects.read"]})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{projects_token}");
    let projects_only = projects_token["token"].as_str().unwrap();

    let request = axum::http::Request::builder()
        .method("GET")
        .uri(&path)
        .header("origin", "http://localhost")
        .header("authorization", format!("Bearer {read_token}"))
        .body(axum::body::Body::empty())
        .unwrap();
    let mut request = request;
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(project_harness::test_peer()));
    let response = tower::ServiceExt::oneshot(app.clone(), request)
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);

    let request = axum::http::Request::builder()
        .method("GET")
        .uri(&path)
        .header("origin", "http://localhost")
        .header("authorization", format!("Bearer {projects_only}"))
        .body(axum::body::Body::empty())
        .unwrap();
    let mut request = request;
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(project_harness::test_peer()));
    let response = tower::ServiceExt::oneshot(app, request)
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    admin.close().await;
    harness.cleanup().await;
}

/// Membership locks precede the project row lock on every write: an assignee PATCH
/// waits on the assignee's lock before taking the project row, so a concurrent
/// write by the assignee (membership lock, then project row) cannot deadlock.
#[tokio::test]
async fn assignee_patch_and_assignee_write_do_not_deadlock() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let b = add_workspace_user(&admin, workspace_id, "member", "bee").await;
    let project = create_project(app.clone(), &cookie, workspace_id, "DLK", "workspace").await;
    let project_id: Uuid = project["id"].as_str().unwrap().parse().unwrap();
    let (_, task) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({"title": "t"})),
        Some(&cookie),
    )
    .await;
    let task_id = task["id"].as_str().unwrap().to_string();
    let task_id_for_get = task_id.clone();

    // First step of any write by B: B's membership lock.
    let mut b_tx = admin.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock($1, $2)")
        .bind(fvoci_server::db::context::MEMBERSHIP_LOCK_NAMESPACE)
        .bind(fvoci_server::db::context::lock_key_from_uuid(b.user_id))
        .execute(&mut *b_tx)
        .await
        .unwrap();

    let patch = tokio::spawn({
        let app = app.clone();
        let cookie = cookie.clone();
        let b_id = b.user_id.to_string();
        async move {
            json_request(
                app,
                "PATCH",
                &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}"),
                Some(json!({"assigneeIds": [b_id]})),
                Some(&cookie),
            )
            .await
        }
    });

    // Read-only, bounded: the PATCH is waiting on B's advisory lock.
    let mut waiting = false;
    for _ in 0..400 {
        let n: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity \
             WHERE datname = current_database() AND wait_event_type = 'Lock' AND wait_event = 'advisory'",
        )
        .fetch_one(&admin)
        .await
        .unwrap();
        if n > 0 {
            waiting = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(
        waiting,
        "PATCH never waited on the assignee's membership lock"
    );

    // Second step of B's write: the project row lock (as lock_project takes it).
    sqlx::query(
        "SELECT 1 FROM fvoci.projects WHERE workspace_id = $1 AND id = $2 FOR NO KEY UPDATE",
    )
    .bind(workspace_id)
    .bind(project_id)
    .execute(&mut *b_tx)
    .await
    .expect("B locks the project row without a deadlock");
    b_tx.commit().await.unwrap();

    let (status, body) = patch.await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, detail) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id_for_get}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(
        detail["assigneeIds"],
        json!([b.user_id.to_string()]),
        "{detail}"
    );
    harness.cleanup().await;
}
