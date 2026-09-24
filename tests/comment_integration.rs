#![cfg(feature = "db-tests")]
#![allow(dead_code)]

#[path = "support/project_harness.rs"]
mod project_harness;

use axum::http::StatusCode;
use project_harness::{
    add_workspace_user, admin_pool, count_rows, create_project, drop_insert_fail_trigger,
    install_insert_fail_trigger, json_request, setup_session, TestDb,
};
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
async fn wiki_document_comment_create_list_resolve_and_react() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let (status, doc) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"parentId": null, "title": "댓글 문서"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let document_id = doc["id"].as_str().unwrap();

    let (status, created) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/comments"),
        Some(json!({"body": "첫 댓글"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let comment_id = created["id"].as_str().unwrap();
    assert_eq!(created["body"], "첫 댓글");

    let (status, listed) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/comments"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["items"].as_array().unwrap().len(), 1);

    let (status, resolved) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/comments/{comment_id}/resolve"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(resolved["resolvedAt"].is_string());

    let (status, reacted) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/comments/{comment_id}/reactions"),
        Some(json!({"emoji": "👍", "on": true})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(reacted["reactions"]["👍"]["count"], 1);
    assert_eq!(reacted["reactions"]["👍"]["reactedByMe"], true);

    harness.cleanup().await;
}

#[tokio::test]
async fn task_comment_create_and_reply_inherits_target() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let (status, task) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({"title": "태스크"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let task_id = task["id"].as_str().unwrap();

    let (status, root) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/comments"),
        Some(json!({"body": "루트"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let root_id = root["id"].as_str().unwrap();

    let (status, reply) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/comments"),
        Some(json!({"body": "답글", "parentId": root_id})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(reply["parentId"], root_id);
    assert_eq!(reply["taskId"], task_id);

    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!(
            "/api/v1/workspaces/{workspace_id}/comments/{}/resolve",
            reply["id"].as_str().unwrap()
        ),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    harness.cleanup().await;
}

#[tokio::test]
async fn guest_and_non_member_get_masked_not_found() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let guest = add_workspace_user(&admin, workspace_id, "guest", "guest").await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let (status, task) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({"title": "비공개"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let task_id = task["id"].as_str().unwrap();

    let (_, guest_post) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/comments"),
        Some(json!({"body": "게스트"})),
        Some(&guest.cookie),
    )
    .await;
    let (status, missing) = json_request(
        app.clone(),
        "POST",
        &format!(
            "/api/v1/workspaces/{workspace_id}/tasks/{}/comments",
            Uuid::now_v7()
        ),
        Some(json!({"body": "없음"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(guest_post, missing);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn private_project_viewer_can_list_but_not_create_task_comment() {
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
        Some(json!({"userId": owner_id.to_string(), "role": "viewer"})),
        Some(&member.cookie),
    )
    .await;
    let (status, task) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({"title": "Lead task"})),
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let task_id = task["id"].as_str().unwrap();
    json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/comments"),
        Some(json!({"body": "멤버 댓글"})),
        Some(&member.cookie),
    )
    .await;

    let (status, list) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/comments"),
        None,
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["items"].as_array().unwrap().len(), 1);

    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/comments"),
        Some(json!({"body": "뷰어는 불가"})),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn mentioned_group_ids_returns_400() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let (status, task) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({"title": "T"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let task_id = task["id"].as_str().unwrap();
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/comments"),
        Some(json!({"body": "그룹", "mentionedGroupIds": [Uuid::now_v7().to_string()]})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    harness.cleanup().await;
}

#[tokio::test]
async fn comment_create_audit_failure_rolls_back_event() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let (status, doc) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"parentId": null, "title": "감사"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let document_id = doc["id"].as_str().unwrap();
    install_insert_fail_trigger(&admin, "audit_log", "test_comment_audit_fail").await;
    let events_before = count_rows(&admin, "events").await;
    let comments_before = count_rows(&admin, "comments").await;
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/comments"),
        Some(json!({"body": "실패"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(count_rows(&admin, "events").await, events_before);
    assert_eq!(count_rows(&admin, "comments").await, comments_before);
    drop_insert_fail_trigger(&admin, "audit_log", "test_comment_audit_fail").await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn concurrent_reaction_toggles_leave_single_membership() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, user_id, workspace_id) = setup_session(&harness).await;
    let (status, doc) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"parentId": null, "title": "반응"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let document_id = doc["id"].as_str().unwrap();
    let (status, created) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/comments"),
        Some(json!({"body": "좋아요"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let comment_id = created["id"].as_str().unwrap();
    let path = format!("/api/v1/workspaces/{workspace_id}/comments/{comment_id}/reactions");
    let body = json!({"emoji": "👍", "on": true});
    let (on_a, on_b) = tokio::join!(
        json_request(
            app.clone(),
            "POST",
            &path,
            Some(body.clone()),
            Some(&cookie)
        ),
        json_request(app.clone(), "POST", &path, Some(body), Some(&cookie)),
    );
    assert_eq!(on_a.0, StatusCode::OK);
    assert_eq!(on_b.0, StatusCode::OK);
    assert_eq!(on_a.1["reactions"]["👍"]["count"], 1);
    assert_eq!(on_b.1["reactions"]["👍"]["count"], 1);

    let (status, off) = json_request(
        app.clone(),
        "POST",
        &path,
        Some(json!({"emoji": "👍", "on": false})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(off["reactions"].as_object().unwrap().get("👍"), None);

    let admin = admin_pool(&harness).await;
    let reactions: (serde_json::Value,) =
        sqlx::query_as("SELECT reactions FROM fvoci.comments WHERE id = $1")
            .bind(Uuid::parse_str(comment_id).unwrap())
            .fetch_one(&admin)
            .await
            .unwrap();
    let ids = reactions
        .0
        .get("👍")
        .and_then(|v| v.as_array())
        .map(|arr| arr.len())
        .unwrap_or(0);
    assert_eq!(ids, 0);
    let _ = user_id;
    admin.close().await;
    harness.cleanup().await;
}
