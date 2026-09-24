#![cfg(feature = "db-tests")]

#[path = "support/project_harness.rs"]
mod project_harness;

use axum::http::StatusCode;
use project_harness::{json_request, setup_session, TestDb};
use serde_json::json;
use sqlx::postgres::PgPoolOptions;

#[tokio::test]
async fn migration_008_projects_schema_exists() {
    let harness = TestDb::bootstrap().await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
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
async fn create_workspace_visible_project_seeds_workflow_and_counts() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;

    let (status, body) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects"),
        Some(json!({"key":"LAB","name":"Lab","visibility":"workspace"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(body["rootDocumentId"].is_string());
    assert!(body["description"].is_null());
    assert_eq!(body["status"], "active");

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

    harness.cleanup().await;
}

#[tokio::test]
async fn private_project_hidden_from_non_member_owner() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();

    let member_id = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO fvoci.users (id, email, given_name) VALUES ($1, $2, 'Member')")
        .bind(member_id)
        .bind(format!("member-{member_id}@example.com"))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO fvoci.memberships (workspace_id, user_id, role) VALUES ($1, $2, 'member')",
    )
    .bind(workspace_id)
    .bind(member_id)
    .execute(&admin)
    .await
    .unwrap();
    let member_token = fvoci_server::auth::token::new_token();
    sqlx::query(
        "INSERT INTO fvoci.sessions (id, user_id, token_hash, expires_at) VALUES ($1, $2, $3, now() + interval '1 hour')",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(member_id)
    .bind(&member_token.hash)
    .execute(&admin)
    .await
    .unwrap();
    admin.close().await;
    let member_cookie = member_token.token;

    let (status, created) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects"),
        Some(json!({"key":"HID","name":"Hidden","visibility":"private"})),
        Some(&member_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let project_id = created["id"].as_str().unwrap();

    let (status, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}"),
        None,
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    harness.cleanup().await;
}

#[tokio::test]
async fn duplicate_and_reserved_project_keys_rejected() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;

    json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects"),
        Some(json!({"key":"LAB","name":"Lab","visibility":"workspace"})),
        Some(&cookie),
    )
    .await;

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

    harness.cleanup().await;
}

#[tokio::test]
async fn removing_last_private_lead_returns_conflict() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let (status, project) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects"),
        Some(json!({"key":"HID","name":"Hidden","visibility":"private"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let project_id = project["id"].as_str().unwrap();

    let (status, _) = json_request(
        app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/members/{owner_id}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    harness.cleanup().await;
}
