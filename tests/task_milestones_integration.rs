#![cfg(feature = "db-tests")]
#![allow(dead_code)]

#[allow(dead_code)]
#[path = "support/project_harness.rs"]
mod project_harness;

use axum::http::StatusCode;
use project_harness::{
    admin_pool, count_rows, create_project, json_request, setup_session, TestDb,
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

async fn create_milestone(
    app: axum::Router,
    cookie: &str,
    workspace_id: Uuid,
    project_id: &str,
    name: &str,
    due_date: Option<&str>,
) -> (StatusCode, serde_json::Value) {
    let mut body = json!({"name": name});
    if let Some(due_date) = due_date {
        body["dueDate"] = json!(due_date);
    }
    json_request(
        app,
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/milestones"),
        Some(body),
        Some(cookie),
    )
    .await
}

fn encode_query(raw: &str) -> String {
    form_urlencoded::byte_serialize(raw.as_bytes()).collect()
}

#[tokio::test]
async fn milestones_crud_and_task_attach_match_source_contract() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let task = create_task(app.clone(), &cookie, workspace_id, project_id, "일").await;
    let task_id = task["id"].as_str().unwrap();

    let events_before = count_rows(&admin, "events").await;
    let (status, created) = create_milestone(
        app.clone(),
        &cookie,
        workspace_id,
        project_id,
        " 출시 ",
        Some("2031-04-10"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["name"], "출시");
    assert_eq!(created["dueDate"], "2031-04-10");
    assert_eq!(created["projectId"], project_id);
    assert!(created.get("createdAt").is_none());
    let milestone_id = created["id"].as_str().unwrap().to_string();
    assert_eq!(count_rows(&admin, "events").await, events_before);

    let (status, listed) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/milestones"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["items"].as_array().unwrap().len(), 1);

    let (status, updated) = json_request(
        app.clone(),
        "PATCH",
        &format!(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/milestones/{milestone_id}"
        ),
        Some(json!({"name": "GA", "dueDate": null})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["ok"], true);
    assert_eq!(count_rows(&admin, "events").await, events_before);

    let (status, patched) = json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}"),
        Some(json!({"milestoneId": milestone_id})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{patched}");
    assert_eq!(patched["milestoneId"], milestone_id);
    assert!(count_rows(&admin, "events").await > events_before);
    let payloads: Vec<(String, serde_json::Value)> = sqlx::query_as(
        "SELECT verb, payload FROM fvoci.events WHERE target_id = $1::uuid ORDER BY created_at",
    )
    .bind(Uuid::parse_str(task_id).unwrap())
    .fetch_all(&admin)
    .await
    .unwrap();
    let milestone_event = payloads
        .iter()
        .find(|(_, payload)| payload.get("milestoneId").is_some())
        .unwrap();
    assert_eq!(milestone_event.0, "task.updated");
    assert_eq!(milestone_event.1["milestoneId"], json!(milestone_id));

    let (status, created_with) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({"title": "첨부", "milestoneId": milestone_id})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created_with}");
    assert_eq!(created_with["milestoneId"], milestone_id);

    let filter = encode_query(&format!(
        r#"{{"filters":{{"milestoneId":"{milestone_id}"}}}}"#
    ));
    let (status, filtered) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks?query={filter}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{filtered}");
    assert_eq!(filtered["items"].as_array().unwrap().len(), 2);

    let (status, cleared) = json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}"),
        Some(json!({"milestoneId": null})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{cleared}");
    assert_eq!(cleared["milestoneId"], serde_json::Value::Null);

    let attached_id = created_with["id"].as_str().unwrap();
    let (status, deleted) = json_request(
        app.clone(),
        "DELETE",
        &format!(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/milestones/{milestone_id}"
        ),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{deleted}");
    assert_eq!(deleted["ok"], true);
    let (status, after) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{attached_id}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(after["milestoneId"], serde_json::Value::Null);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn milestone_errors_and_invisible_filter() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let other = create_project(app.clone(), &cookie, workspace_id, "OTH", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let other_id = other["id"].as_str().unwrap();
    let task = create_task(app.clone(), &cookie, workspace_id, project_id, "A").await;
    let task_id = task["id"].as_str().unwrap();
    let (status, other_ms) =
        create_milestone(app.clone(), &cookie, workspace_id, other_id, "다른", None).await;
    assert_eq!(status, StatusCode::CREATED);
    let other_ms_id = other_ms["id"].as_str().unwrap();

    let (status, problem) = json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}"),
        Some(json!({"milestoneId": Uuid::now_v7().to_string()})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{problem}");
    assert_eq!(problem["code"], "not_found");

    let (status, problem) = json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}"),
        Some(json!({"milestoneId": other_ms_id})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{problem}");
    assert_eq!(problem["code"], "not_found");

    let (status, problem) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({"title": "Bad", "milestoneId": other_ms_id})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{problem}");
    assert_eq!(problem["code"], "not_found");

    let missing = encode_query(&format!(
        r#"{{"filters":{{"milestoneId":"{}"}}}}"#,
        Uuid::now_v7()
    ));
    let (status, problem) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks?query={missing}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{problem}");
    assert_eq!(problem["code"], "invalid_input");

    let (status, missing_ms) = json_request(
        app.clone(),
        "DELETE",
        &format!(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/milestones/{}",
            Uuid::now_v7()
        ),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{missing_ms}");
    assert_eq!(missing_ms["code"], "not_found");

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn dependencies_cycle_contradiction_and_trash() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let other = create_project(app.clone(), &cookie, workspace_id, "OTH", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let other_id = other["id"].as_str().unwrap();
    let a = create_task(app.clone(), &cookie, workspace_id, project_id, "A").await;
    let b = create_task(app.clone(), &cookie, workspace_id, project_id, "B").await;
    let c = create_task(app.clone(), &cookie, workspace_id, project_id, "C").await;
    let foreign = create_task(app.clone(), &cookie, workspace_id, other_id, "X").await;
    let a_id = a["id"].as_str().unwrap();
    let b_id = b["id"].as_str().unwrap();
    let c_id = c["id"].as_str().unwrap();
    let foreign_id = foreign["id"].as_str().unwrap();

    let events_before = count_rows(&admin, "events").await;
    let (status, added) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{a_id}/dependencies"),
        Some(json!({"blockedId": b_id})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{added}");
    assert_eq!(added["ok"], true);
    assert_eq!(count_rows(&admin, "events").await, events_before);

    let (status, listed) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/dependencies"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(listed["items"].as_array().unwrap().len(), 1);
    assert_eq!(listed["items"][0]["type"], "FS");
    assert_eq!(listed["items"][0]["lagDays"], 0);

    let (status, detail) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{a_id}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["dependencies"].as_array().unwrap().len(), 1);

    let (status, self_block) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{a_id}/dependencies"),
        Some(json!({"blockedId": a_id})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{self_block}");
    assert_eq!(self_block["code"], "task_cannot_block_itself");

    let (status, cycle) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{b_id}/dependencies"),
        Some(json!({"blockedId": a_id})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{cycle}");
    assert_eq!(cycle["code"], "dependency_cycle");

    json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{b_id}/dependencies"),
        Some(json!({"blockedId": c_id})),
        Some(&cookie),
    )
    .await;
    let (status, transitive) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{c_id}/dependencies"),
        Some(json!({"blockedId": a_id})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{transitive}");
    assert_eq!(transitive["code"], "dependency_cycle");

    let (status, cross) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{a_id}/dependencies"),
        Some(json!({"blockedId": foreign_id})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{cross}");

    json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{a_id}"),
        Some(json!({"dueDate": "2031-04-10"})),
        Some(&cookie),
    )
    .await;
    let (status, contradiction) = json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{b_id}"),
        Some(json!({"startDate": "2031-04-01"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{contradiction}");
    assert_eq!(contradiction["code"], "dependency_contradiction");

    let (status, ok_same_day) = json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{b_id}"),
        Some(json!({"startDate": "2031-04-10"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{ok_same_day}");

    let (status, missing_delete) = json_request(
        app.clone(),
        "DELETE",
        &format!(
            "/api/v1/workspaces/{workspace_id}/tasks/{a_id}/dependencies/{}",
            Uuid::now_v7()
        ),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{missing_delete}");
    assert_eq!(missing_delete["code"], "not_found");

    json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{b_id}/trash"),
        None,
        Some(&cookie),
    )
    .await;
    let remaining: (i64,) =
        sqlx::query_as("SELECT count(*) FROM fvoci.task_dependencies WHERE blocker_id = $1::uuid")
            .bind(Uuid::parse_str(a_id).unwrap())
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(remaining.0, 1);

    let (status, removed) = json_request(
        app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{a_id}/dependencies/{b_id}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{removed}");
    assert_eq!(removed["ok"], true);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn concurrent_dependency_inserts_cannot_both_form_a_cycle() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let a = create_task(app.clone(), &cookie, workspace_id, project_id, "A").await;
    let b = create_task(app.clone(), &cookie, workspace_id, project_id, "B").await;
    let a_id = a["id"].as_str().unwrap().to_string();
    let b_id = b["id"].as_str().unwrap().to_string();

    let a_path = format!("/api/v1/workspaces/{workspace_id}/tasks/{a_id}/dependencies");
    let b_path = format!("/api/v1/workspaces/{workspace_id}/tasks/{b_id}/dependencies");
    let left = json_request(
        app.clone(),
        "POST",
        &a_path,
        Some(json!({"blockedId": b_id})),
        Some(&cookie),
    );
    let right = json_request(
        app.clone(),
        "POST",
        &b_path,
        Some(json!({"blockedId": a_id})),
        Some(&cookie),
    );
    let ((status_a, body_a), (status_b, body_b)) = tokio::join!(left, right);
    let outcomes = [status_a, status_b];
    assert!(
        outcomes.contains(&StatusCode::OK) && outcomes.contains(&StatusCode::BAD_REQUEST),
        "{status_a:?} {body_a} / {status_b:?} {body_b}"
    );
    let failed = if status_a == StatusCode::BAD_REQUEST {
        &body_a
    } else {
        &body_b
    };
    assert_eq!(failed["code"], "dependency_cycle");
    let count: (i64,) = sqlx::query_as("SELECT count(*) FROM fvoci.task_dependencies")
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(count.0, 1);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn recurrence_copies_milestone_not_dependencies() {
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
    let done = workflow["statuses"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["category"] == "done")
        .unwrap()["id"]
        .as_str()
        .unwrap();
    let (status, ms) =
        create_milestone(app.clone(), &cookie, workspace_id, project_id, "회차", None).await;
    assert_eq!(status, StatusCode::CREATED);
    let milestone_id = ms["id"].as_str().unwrap();
    let blocker = create_task(app.clone(), &cookie, workspace_id, project_id, "Blocker").await;
    let (status, created) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({
            "title": "R",
            "recurrence": {"kind": "daily"},
            "milestoneId": milestone_id
        })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let task_id = created["id"].as_str().unwrap();
    json_request(
        app.clone(),
        "POST",
        &format!(
            "/api/v1/workspaces/{workspace_id}/tasks/{}/dependencies",
            blocker["id"]
        ),
        Some(json!({"blockedId": task_id})),
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
    assert_eq!(ids.len(), 3);
    let next_id = ids[2].0;
    let (status, next) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{next_id}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{next}");
    assert_eq!(next["milestoneId"], milestone_id);
    assert!(next["dependencies"].as_array().unwrap().is_empty());

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn milestones_force_rls_and_pat_scopes() {
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
          AND c.relname IN ('milestones', 'task_dependencies')
        ORDER BY c.relname
        "#,
    )
    .fetch_all(&admin)
    .await
    .unwrap();
    assert_eq!(flags.len(), 2);
    for (name, rls, force) in &flags {
        assert!(rls, "{name} missing RLS");
        assert!(force, "{name} missing FORCE RLS");
    }

    let path = format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/milestones");
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
