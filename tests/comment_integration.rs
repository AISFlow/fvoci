#![cfg(feature = "db-tests")]
#![allow(dead_code)]

#[path = "support/project_harness.rs"]
mod project_harness;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use project_harness::{
    add_workspace_user, admin_pool, count_rows, create_project, drop_insert_fail_trigger,
    http_request, install_insert_fail_trigger, json_request, setup_session, test_peer, TestDb,
};
use serde_json::{json, Value};
use tower::ServiceExt;
use uuid::Uuid;

async fn json_request_origin(
    app: axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    cookie: Option<&str>,
    origin: &str,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("origin", origin);
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", format!("fvoci_session={cookie}"));
    }
    let request = if let Some(body) = body {
        builder
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    } else {
        builder.body(Body::empty()).unwrap()
    };
    let mut request = request;
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(test_peer()));
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
async fn mentioned_group_ids_expand_into_event() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, workspace_id, "member", "groupmate").await;
    let (status, group) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/groups"),
        Some(json!({"name": "랩팀"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{group:?}");
    let group_id = group["id"].as_str().unwrap();
    let (status, added) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/groups/{group_id}/members"),
        Some(json!({"userId": member.user_id.to_string()})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{added:?}");

    let (status, doc) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"parentId": null, "title": "그룹 멘션"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let document_id = doc["id"].as_str().unwrap();
    let (status, created) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/comments"),
        Some(json!({
            "body": "@랩팀",
            "mentionedGroupIds": [group_id]
        })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created:?}");

    let payload: Value = sqlx::query_scalar(
        "SELECT payload FROM fvoci.events WHERE verb = 'comment.created' ORDER BY created_at DESC LIMIT 1",
    )
    .fetch_one(&admin)
    .await
    .expect("comment.created payload");
    let mentioned_users = payload["mentionedUserIds"]
        .as_array()
        .expect("mentionedUserIds");
    assert!(
        mentioned_users
            .iter()
            .any(|id| id.as_str() == Some(&member.user_id.to_string())),
        "{payload:?}"
    );
    let mentioned_groups = payload["mentionedGroupIds"]
        .as_array()
        .expect("mentionedGroupIds");
    assert!(
        mentioned_groups
            .iter()
            .any(|id| id.as_str() == Some(group_id)),
        "{payload:?}"
    );

    let (status, unknown) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/comments"),
        Some(json!({
            "body": "없는 그룹",
            "mentionedGroupIds": [Uuid::now_v7().to_string()]
        })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{unknown:?}");

    admin.close().await;
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

#[tokio::test]
async fn wiki_comment_delete_is_author_or_manage() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, _, workspace_id) = setup_session(&harness).await;
    let admin_db = admin_pool(&harness).await;
    let member = add_workspace_user(&admin_db, workspace_id, "member", "member").await;
    let admin = add_workspace_user(&admin_db, workspace_id, "admin", "admin").await;
    let (status, doc) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"parentId": null, "title": "권한"})),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let document_id = doc["id"].as_str().unwrap();
    let comments = format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/comments");

    let (status, owner_comment) = json_request(
        app.clone(),
        "POST",
        &comments,
        Some(json!({"body": "소유자"})),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let owner_comment_id = owner_comment["id"].as_str().unwrap();

    let (status, member_comment) = json_request(
        app.clone(),
        "POST",
        &comments,
        Some(json!({"body": "멤버"})),
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let member_comment_id = member_comment["id"].as_str().unwrap();

    let (status, extra_member) = json_request(
        app.clone(),
        "POST",
        &comments,
        Some(json!({"body": "멤버 본인 삭제"})),
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, _) = json_request(
        app.clone(),
        "DELETE",
        &format!(
            "/api/v1/workspaces/{workspace_id}/comments/{}",
            extra_member["id"].as_str().unwrap()
        ),
        None,
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, denied) = json_request(
        app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{workspace_id}/comments/{owner_comment_id}"),
        None,
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(denied["code"], "not_found");

    let (status, patched) = json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{workspace_id}/comments/{owner_comment_id}"),
        Some(json!({"body": "멤버가 수정"})),
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(patched["body"], "멤버가 수정");

    let (status, _) = json_request(
        app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{workspace_id}/comments/{member_comment_id}"),
        None,
        Some(&admin.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, owner_again) = json_request(
        app.clone(),
        "POST",
        &comments,
        Some(json!({"body": "소유자가 지움"})),
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = json_request(
        app.clone(),
        "DELETE",
        &format!(
            "/api/v1/workspaces/{workspace_id}/comments/{}",
            owner_again["id"].as_str().unwrap()
        ),
        None,
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    admin_db.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn comment_body_limit_counts_utf16_code_units() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let (status, doc) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"parentId": null, "title": "길이"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let document_id = doc["id"].as_str().unwrap();
    let path = format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/comments");

    let (status, _) = json_request(
        app.clone(),
        "POST",
        &path,
        Some(json!({"body": "가".repeat(8000)})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, over_korean) = json_request(
        app.clone(),
        "POST",
        &path,
        Some(json!({"body": "가".repeat(8001)})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(over_korean["code"], "invalid_input");

    let (status, _) = json_request(
        app.clone(),
        "POST",
        &path,
        Some(json!({"body": "👍".repeat(4000)})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, over_emoji) = json_request(
        app.clone(),
        "POST",
        &path,
        Some(json!({"body": "👍".repeat(4001)})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(over_emoji["code"], "invalid_input");

    harness.cleanup().await;
}

#[tokio::test]
async fn resolve_and_unresolve_reject_cross_origin() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let (status, doc) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"parentId": null, "title": "출처"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let document_id = doc["id"].as_str().unwrap();
    let (status, created) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/comments"),
        Some(json!({"body": "원본"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let comment_id = created["id"].as_str().unwrap();
    let resolve = format!("/api/v1/workspaces/{workspace_id}/comments/{comment_id}/resolve");
    let unresolve = format!("/api/v1/workspaces/{workspace_id}/comments/{comment_id}/unresolve");

    let (status, evil) = json_request_origin(
        app.clone(),
        "POST",
        &resolve,
        None,
        Some(&cookie),
        "https://evil.example",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(evil["code"], "origin_mismatch");

    let (status, _) = json_request(app.clone(), "POST", &resolve, None, Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK);

    let (status, evil_unresolve) = json_request_origin(
        app.clone(),
        "POST",
        &unresolve,
        None,
        Some(&cookie),
        "https://evil.example",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(evil_unresolve["code"], "origin_mismatch");

    harness.cleanup().await;
}

#[tokio::test]
async fn project_document_comments_use_project_permission() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let document_id = lab["rootDocumentId"]
        .as_str()
        .expect("project root document");
    let comments = format!(
        "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/comments"
    );

    let (status, listed) = json_request(app.clone(), "GET", &comments, None, Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK, "{listed:?}");
    assert_eq!(listed["items"].as_array().unwrap().len(), 0);

    let (status, created) = json_request(
        app.clone(),
        "POST",
        &comments,
        Some(json!({"body": "프로젝트 문서"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created:?}");
    assert_eq!(created["body"], "프로젝트 문서");
    assert_eq!(created["documentId"], document_id);

    let (status, wiki) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/comments"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(wiki["code"], "not_found");

    let hid = create_project(app.clone(), &cookie, workspace_id, "HID", "private").await;
    let private_project_id = hid["id"].as_str().unwrap();
    let private_document_id = hid["rootDocumentId"].as_str().expect("private root");
    let private_comments = format!(
        "/api/v1/workspaces/{workspace_id}/projects/{private_project_id}/documents/{private_document_id}/comments"
    );
    let outsider = add_workspace_user(&admin, workspace_id, "member", "outsider").await;
    let (status, denied) = json_request(
        app.clone(),
        "GET",
        &private_comments,
        None,
        Some(&outsider.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{denied:?}");

    json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{private_project_id}/members"),
        Some(json!({"userId": outsider.user_id.to_string(), "role": "viewer"})),
        Some(&cookie),
    )
    .await;
    let (status, viewer_list) = json_request(
        app.clone(),
        "GET",
        &private_comments,
        None,
        Some(&outsider.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{viewer_list:?}");
    let (status, viewer_create) = json_request(
        app.clone(),
        "POST",
        &private_comments,
        Some(json!({"body": "뷰어는 불가"})),
        Some(&outsider.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{viewer_create:?}");

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn comment_pat_scopes_match_parent_kind() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let (status, doc) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"parentId": null, "title": "PAT 문서"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let document_id = doc["id"].as_str().unwrap();
    let comments = format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/comments");

    let (status, read_token) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/api-tokens"),
        Some(json!({"name": "doc-read", "scopes": ["documents.read"]})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{read_token:?}");
    let read_secret = read_token["token"].as_str().unwrap().to_string();
    let read_auth = format!("Bearer {read_secret}");
    let (status, listed, _) = http_request(
        app.clone(),
        "GET",
        &comments,
        None,
        None,
        None,
        &[("authorization", read_auth.as_str())],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{listed:?}");
    let (status, denied, _) = http_request(
        app.clone(),
        "POST",
        &comments,
        Some(json!({"body": "읽기만"}).to_string().into_bytes()),
        Some("application/json"),
        None,
        &[("authorization", read_auth.as_str())],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{denied:?}");

    let (status, write_token) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/api-tokens"),
        Some(json!({"name": "doc-write", "scopes": ["documents.write"]})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{write_token:?}");
    let write_secret = write_token["token"].as_str().unwrap().to_string();
    let write_auth = format!("Bearer {write_secret}");
    let (status, created, _) = http_request(
        app.clone(),
        "POST",
        &comments,
        Some(json!({"body": "쓰기"}).to_string().into_bytes()),
        Some("application/json"),
        None,
        &[("authorization", write_auth.as_str())],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created:?}");

    let lab = create_project(app.clone(), &cookie, workspace_id, "PAT", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let (status, task) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({"title": "PAT 태스크"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let task_id = task["id"].as_str().unwrap();
    let task_comments = format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/comments");
    let (status, task_denied, _) = http_request(
        app.clone(),
        "POST",
        &task_comments,
        Some(json!({"body": "문서 토큰"}).to_string().into_bytes()),
        Some("application/json"),
        None,
        &[("authorization", write_auth.as_str())],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{task_denied:?}");

    let (status, task_token) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/api-tokens"),
        Some(json!({"name": "task-write", "scopes": ["tasks.write"]})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{task_token:?}");
    let task_secret = task_token["token"].as_str().unwrap().to_string();
    let task_auth = format!("Bearer {task_secret}");
    let (status, task_created, _) = http_request(
        app.clone(),
        "POST",
        &task_comments,
        Some(json!({"body": "태스크"}).to_string().into_bytes()),
        Some("application/json"),
        None,
        &[("authorization", task_auth.as_str())],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{task_created:?}");

    harness.cleanup().await;
}

#[tokio::test]
async fn archived_parent_comment_writes_return_conflict() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let (status, task) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({"title": "보관"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let task_id = task["id"].as_str().unwrap();
    let comments = format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/comments");

    let (status, created) = json_request(
        app.clone(),
        "POST",
        &comments,
        Some(json!({"body": "보관 전"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, _) = json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}"),
        Some(json!({"archived": true})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, archived_task) = json_request(
        app.clone(),
        "POST",
        &comments,
        Some(json!({"body": "보관된 태스크"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(archived_task["code"], "task_archived");

    sqlx::query("UPDATE fvoci.tasks SET archived_at = NULL WHERE id = $1")
        .bind(Uuid::parse_str(task_id).unwrap())
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query("UPDATE fvoci.projects SET status = 'archived' WHERE id = $1")
        .bind(Uuid::parse_str(project_id).unwrap())
        .execute(&admin)
        .await
        .unwrap();

    let (status, listed) = json_request(app.clone(), "GET", &comments, None, Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["items"].as_array().unwrap().len(), 1);

    let (status, archived_project) = json_request(
        app.clone(),
        "POST",
        &comments,
        Some(json!({"body": "보관된 프로젝트"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(archived_project["code"], "project_archived");

    let _ = created;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn comment_create_unknown_fields_and_empty_group_ids() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let (status, doc) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"parentId": null, "title": "필드"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let document_id = doc["id"].as_str().unwrap();
    let path = format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/comments");

    let (status, extra) = json_request(
        app.clone(),
        "POST",
        &path,
        Some(json!({"body": "a", "extra": 1})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(extra["code"], "invalid_input");

    let (status, empty_groups) = json_request(
        app.clone(),
        "POST",
        &path,
        Some(json!({"body": "빈 그룹", "mentionedGroupIds": []})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(empty_groups["body"], "빈 그룹");

    let (status, limit_zero) = json_request(
        app.clone(),
        "GET",
        &format!("{path}?limit=0"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(limit_zero["code"], "invalid_input");

    let (status, limit_high) = json_request(
        app.clone(),
        "GET",
        &format!("{path}?limit=101"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(limit_high["code"], "invalid_input");

    harness.cleanup().await;
}
