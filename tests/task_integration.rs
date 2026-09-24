#![cfg(feature = "db-tests")]

#[path = "support/project_harness.rs"]
mod project_harness;

use axum::http::StatusCode;
use project_harness::{json_request, setup_session, TestDb};
use serde_json::json;

#[tokio::test]
async fn create_and_read_task_with_counts() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;

    let (status, project) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects"),
        Some(json!({"key":"LAB","name":"Lab","visibility":"workspace"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let project_id = project["id"].as_str().unwrap();

    let (status, task) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({"title":"첫 일"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(task["number"], 2);

    let task_id = task["id"].as_str().unwrap();
    let (status, detail) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["canEdit"], true);

    harness.cleanup().await;
}
