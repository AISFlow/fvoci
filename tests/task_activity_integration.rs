#![cfg(feature = "db-tests")]
#![allow(dead_code)]

#[path = "support/project_harness.rs"]
mod project_harness;

use axum::http::StatusCode;
use project_harness::{admin_pool, create_project, json_request, setup_session, TestDb};
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
        Some(json!({"title": title, "priority": "medium"})),
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    task
}

fn encode_query(raw: &str) -> String {
    form_urlencoded::byte_serialize(raw.as_bytes()).collect()
}

#[tokio::test]
async fn task_activity_records_changes_and_lists_union_feed() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let project = create_project(app.clone(), &cookie, workspace_id, "ACT", "workspace").await;
    let project_id = project["id"].as_str().unwrap();
    let task = create_task(app.clone(), &cookie, workspace_id, project_id, "Initial").await;
    let task_id = task["id"].as_str().unwrap();

    let (status, _) = json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}"),
        Some(json!({"title": "Changed", "priority": "high"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, page) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/activity"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let items = page["items"].as_array().expect("items");
    assert!(items.len() >= 2);
    assert_eq!(items[0]["type"], "change");
    assert_eq!(items[0]["kind"], "changed");
    let fields = items[0]["changes"]
        .as_array()
        .expect("changes")
        .iter()
        .map(|change| change["field"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(fields.contains(&"title"));
    assert!(fields.contains(&"priority"));

    let (status, comment) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/comments"),
        Some(json!({"body": "검증 댓글"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{comment}");

    let (status, all) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/activity?filter=all"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{all}");
    assert!(all["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["type"] == "comment"));

    let (status, changes_only) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/activity?filter=changes"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changes_only}");
    assert!(changes_only["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|item| item["type"] == "change"));

    let count: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.task_activity WHERE workspace_id = $1 AND task_id = $2",
    )
    .bind(workspace_id)
    .bind(Uuid::parse_str(task_id).unwrap())
    .fetch_one(&admin)
    .await
    .expect("activity rows");
    assert!(count.0 >= 2);
}

#[tokio::test]
async fn task_activity_rejects_wrong_filter_cursor_and_hidden_task() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let project = create_project(app.clone(), &cookie, workspace_id, "HID", "private").await;
    let project_id = project["id"].as_str().unwrap();
    let task = create_task(app.clone(), &cookie, workspace_id, project_id, "Secret").await;
    let task_id = task["id"].as_str().unwrap();

    let (status, comments_page) = json_request(
        app.clone(),
        "GET",
        &format!(
            "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/activity?filter=comments&limit=1"
        ),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{comments_page}");
    let cursor = comments_page["nextCursor"].as_str();
    assert!(cursor.is_some() || comments_page["items"].as_array().unwrap().is_empty());

    if let Some(cursor) = cursor {
        let encoded = encode_query(cursor);
        let (status, _) = json_request(
            app.clone(),
            "GET",
            &format!(
                "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/activity?filter=changes&cursor={encoded}"
            ),
            None,
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/trash"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/activity"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
