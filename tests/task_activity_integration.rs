#![cfg(feature = "db-tests")]
#![allow(dead_code)]

#[path = "support/project_harness.rs"]
mod project_harness;

use std::collections::HashSet;

use axum::http::StatusCode;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use project_harness::{
    add_workspace_user, admin_pool, app_pool, create_project, json_request, setup_session, TestDb,
};
use serde_json::{json, Value};
use sqlx::PgPool;
use url::form_urlencoded;
use uuid::Uuid;

async fn create_task_with(
    app: axum::Router,
    cookie: &str,
    workspace_id: Uuid,
    project_id: &str,
    body: Value,
) -> Value {
    let (status, task) = json_request(
        app,
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(body),
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    task
}

async fn create_task(
    app: axum::Router,
    cookie: &str,
    workspace_id: Uuid,
    project_id: &str,
    title: &str,
) -> Value {
    create_task_with(
        app,
        cookie,
        workspace_id,
        project_id,
        json!({"title": title, "priority": "medium"}),
    )
    .await
}

async fn patch_task(
    app: axum::Router,
    cookie: &str,
    workspace_id: Uuid,
    task_id: &str,
    body: Value,
) {
    let (status, out) = json_request(
        app,
        "PATCH",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}"),
        Some(body),
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{out}");
}

async fn activity(
    app: axum::Router,
    cookie: &str,
    workspace_id: Uuid,
    task_id: &str,
    query: &str,
) -> (StatusCode, Value) {
    json_request(
        app,
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/activity{query}"),
        None,
        Some(cookie),
    )
    .await
}

fn encode_query(raw: &str) -> String {
    form_urlencoded::byte_serialize(raw.as_bytes()).collect()
}

fn change_for<'a>(items: &'a [Value], field: &str) -> &'a Value {
    items
        .iter()
        .filter(|item| item["type"] == "change")
        .flat_map(|item| item["changes"].as_array().unwrap().iter())
        .find(|change| change["field"] == field)
        .unwrap_or_else(|| panic!("no {field} change in {items:?}"))
}

async fn workflow_status(
    app: axum::Router,
    cookie: &str,
    workspace_id: Uuid,
    project_id: &str,
    category: &str,
) -> (String, String) {
    let (status, wf) = json_request(
        app,
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/workflow"),
        None,
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{wf}");
    let found = wf["statuses"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["category"] == category)
        .expect("status category");
    (
        found["id"].as_str().unwrap().to_string(),
        found["name"].as_str().unwrap().to_string(),
    )
}

async fn collect_all_pages(
    app: axum::Router,
    cookie: &str,
    workspace_id: Uuid,
    task_id: &str,
    filter: &str,
    limit: usize,
) -> Vec<Value> {
    let mut out = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..200 {
        let query = match &cursor {
            Some(cursor) => format!(
                "?filter={filter}&limit={limit}&cursor={}",
                encode_query(cursor)
            ),
            None => format!("?filter={filter}&limit={limit}"),
        };
        let (status, page) = activity(app.clone(), cookie, workspace_id, task_id, &query).await;
        assert_eq!(status, StatusCode::OK, "{page}");
        out.extend(page["items"].as_array().unwrap().iter().cloned());
        match page["nextCursor"].as_str() {
            Some(next) => cursor = Some(next.to_string()),
            None => return out,
        }
    }
    panic!("activity pagination did not terminate");
}

fn item_keys(items: &[Value]) -> Vec<(String, String)> {
    items
        .iter()
        .map(|item| {
            (
                item["type"].as_str().unwrap().to_string(),
                item["id"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

#[tokio::test]
async fn task_activity_records_changes_and_lists_union_feed() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let project = create_project(app.clone(), &cookie, workspace_id, "ACT", "workspace").await;
    let project_id = project["id"].as_str().unwrap();
    let task = create_task(app.clone(), &cookie, workspace_id, project_id, "Initial").await;
    let task_id = task["id"].as_str().unwrap();

    patch_task(
        app.clone(),
        &cookie,
        workspace_id,
        task_id,
        json!({"title": "Changed", "priority": "high", "dueAt": "2026-10-01T09:30:00Z"}),
    )
    .await;
    // A PATCH that changes nothing leaves no activity row.
    patch_task(
        app.clone(),
        &cookie,
        workspace_id,
        task_id,
        json!({"title": "Changed"}),
    )
    .await;

    let (status, page) = activity(app.clone(), &cookie, workspace_id, task_id, "").await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let items = page["items"].as_array().expect("items");
    assert_eq!(items.len(), 2, "{page}");
    assert_eq!(items[0]["type"], "change");
    assert_eq!(items[0]["kind"], "changed");
    assert_eq!(items[0]["channel"], "web");
    assert_eq!(items[0]["actor"]["id"], owner_id.to_string());
    assert_eq!(items[0]["actor"]["name"], "Owner");
    // Wire contract: camelCase like every other field (source taskActivityItemOutput).
    assert!(items[0]["createdAt"].as_str().is_some(), "{page}");
    assert!(items[0].get("created_at").is_none(), "{page}");
    assert_eq!(items[1]["kind"], "created");
    assert_eq!(items[1]["changes"], json!([]));
    let title = change_for(items, "title");
    assert_eq!(title["from"], "Initial");
    assert_eq!(title["to"], "Changed");
    assert_eq!(change_for(items, "priority")["to"], "high");
    // Source stores dueAt with toISOString(): millisecond precision and a Z suffix.
    assert_eq!(change_for(items, "dueAt")["to"], "2026-10-01T09:30:00.000Z");

    let (status, comment) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/comments"),
        Some(json!({"body": "검증 댓글"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{comment}");
    let (status, reply) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/comments"),
        Some(json!({"body": "답글", "parentId": comment["id"]})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{reply}");

    let (status, all) = activity(app.clone(), &cookie, workspace_id, task_id, "?filter=all").await;
    assert_eq!(status, StatusCode::OK, "{all}");
    let items = all["items"].as_array().unwrap();
    assert_eq!(items.len(), 4, "{all}");
    assert_eq!(items[0]["type"], "comment");
    assert_eq!(items[0]["id"], reply["id"]);
    assert!(items[0]["createdAt"].as_str().is_some());
    assert_eq!(items[0]["comment"]["body"], "답글");
    assert_eq!(items[0]["comment"]["createdBy"], owner_id.to_string());
    assert!(items[0]["comment"]["reactions"].is_object());
    assert_eq!(items[0]["parent"]["id"], comment["id"]);
    assert_eq!(items[0]["parent"]["body"], "검증 댓글");
    assert_eq!(items[0]["parent"]["actor"]["name"], "Owner");
    assert!(items[1]["parent"].is_null());

    let (status, changes_only) = activity(
        app.clone(),
        &cookie,
        workspace_id,
        task_id,
        "?filter=changes",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changes_only}");
    assert_eq!(changes_only["items"].as_array().unwrap().len(), 2);
    assert!(changes_only["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|item| item["type"] == "change"));
    let (status, comments_only) = activity(
        app.clone(),
        &cookie,
        workspace_id,
        task_id,
        "?filter=comments",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{comments_only}");
    assert_eq!(comments_only["items"].as_array().unwrap().len(), 2);

    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM fvoci.task_activity WHERE workspace_id = $1 AND task_id = $2",
    )
    .bind(workspace_id)
    .bind(Uuid::parse_str(task_id).unwrap())
    .fetch_one(&admin)
    .await
    .expect("activity rows");
    assert_eq!(count, 2);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_activity_rejects_bad_filter_and_cursors_with_invalid_cursor() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let project = create_project(app.clone(), &cookie, workspace_id, "CUR", "workspace").await;
    let project_id = project["id"].as_str().unwrap();
    let task = create_task(app.clone(), &cookie, workspace_id, project_id, "Cursor").await;
    let task_id = task["id"].as_str().unwrap();
    for body in ["one", "two"] {
        let (status, _) = json_request(
            app.clone(),
            "POST",
            &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/comments"),
            Some(json!({"body": body})),
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
    }

    let (status, body) =
        activity(app.clone(), &cookie, workspace_id, task_id, "?filter=bogus").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "invalid_input");
    for limit in ["0", "101"] {
        let (status, _) = activity(
            app.clone(),
            &cookie,
            workspace_id,
            task_id,
            &format!("?limit={limit}"),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    let (status, page) = activity(
        app.clone(),
        &cookie,
        workspace_id,
        task_id,
        "?filter=comments&limit=1",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let cursor = page["nextCursor"].as_str().expect("second comment remains");

    // A cursor minted for another filter is rejected.
    let (status, body) = activity(
        app.clone(),
        &cookie,
        workspace_id,
        task_id,
        &format!("?filter=changes&cursor={}", encode_query(cursor)),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "invalid_cursor");

    // Right scope, but `at` is not a timestamp: 400, never a 500.
    let mut payload: Value =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(cursor).unwrap()).unwrap();
    payload["at"] = json!("not-a-date");
    let forged = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
    let (status, body) = activity(
        app.clone(),
        &cookie,
        workspace_id,
        task_id,
        &format!("?filter=comments&cursor={}", encode_query(&forged)),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "invalid_cursor");

    let (status, body) = activity(
        app.clone(),
        &cookie,
        workspace_id,
        task_id,
        "?filter=comments&cursor=%25%25garbage",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "invalid_cursor");

    let (status, page2) = activity(
        app.clone(),
        &cookie,
        workspace_id,
        task_id,
        &format!("?filter=comments&limit=1&cursor={}", encode_query(cursor)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page2}");
    assert_eq!(page2["items"][0]["comment"]["body"], "one");
    assert!(page2["nextCursor"].is_null());
    harness.cleanup().await;
}

#[tokio::test]
async fn task_activity_hides_task_from_non_viewers_and_trash() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let private = create_project(app.clone(), &cookie, workspace_id, "HID", "private").await;
    let private_task = create_task(
        app.clone(),
        &cookie,
        workspace_id,
        private["id"].as_str().unwrap(),
        "Secret",
    )
    .await;
    let private_task_id = private_task["id"].as_str().unwrap();
    let open = create_project(app.clone(), &cookie, workspace_id, "OPN", "workspace").await;
    let open_task = create_task(
        app.clone(),
        &cookie,
        workspace_id,
        open["id"].as_str().unwrap(),
        "Open",
    )
    .await;
    let open_task_id = open_task["id"].as_str().unwrap();

    let member = add_workspace_user(&admin, workspace_id, "member", "member").await;
    let guest = add_workspace_user(&admin, workspace_id, "guest", "guest").await;

    // Non-member of a private project and a guest without a grant: 404, no oracle.
    let (status, _) = activity(
        app.clone(),
        &member.cookie,
        workspace_id,
        private_task_id,
        "",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = activity(app.clone(), &guest.cookie, workspace_id, open_task_id, "").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, page) =
        activity(app.clone(), &member.cookie, workspace_id, open_task_id, "").await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let (status, _) = activity(
        app.clone(),
        &cookie,
        workspace_id,
        &Uuid::now_v7().to_string(),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{private_task_id}/trash"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = activity(app.clone(), &cookie, workspace_id, private_task_id, "").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_activity_group_only_viewer_sees_feed_and_parent_titles() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let project = create_project(app.clone(), &cookie, workspace_id, "GRP", "private").await;
    let project_id = project["id"].as_str().unwrap();
    let parent_a = create_task_with(
        app.clone(),
        &cookie,
        workspace_id,
        project_id,
        json!({"title": "Parent A", "type": "task"}),
    )
    .await;
    let parent_b = create_task_with(
        app.clone(),
        &cookie,
        workspace_id,
        project_id,
        json!({"title": "Parent B", "type": "task"}),
    )
    .await;
    let child = create_task_with(
        app.clone(),
        &cookie,
        workspace_id,
        project_id,
        json!({"title": "Child", "type": "subtask", "parentId": parent_a["id"]}),
    )
    .await;
    let child_id = child["id"].as_str().unwrap();
    patch_task(
        app.clone(),
        &cookie,
        workspace_id,
        child_id,
        json!({"parentId": parent_b["id"]}),
    )
    .await;

    let viewer = add_workspace_user(&admin, workspace_id, "member", "viewer").await;
    let (status, _) = activity(app.clone(), &viewer.cookie, workspace_id, child_id, "").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "no grant yet");

    let (status, group) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/groups"),
        Some(json!({"name": "viewers"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{group}");
    let group_id = group["id"].as_str().unwrap();
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/groups/{group_id}/members"),
        Some(json!({"userId": viewer.user_id.to_string()})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/groups"),
        Some(json!({"groupId": group_id, "role": "viewer"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, page) = activity(app.clone(), &viewer.cookie, workspace_id, child_id, "").await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let parent = change_for(page["items"].as_array().unwrap(), "parentId");
    assert_eq!(parent["from"]["id"], parent_a["id"]);
    assert_eq!(parent["from"]["label"], "Parent A", "{page}");
    assert_eq!(parent["to"]["label"], "Parent B", "{page}");

    // A trashed parent is not disclosed.
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!(
            "/api/v1/workspaces/{workspace_id}/tasks/{}/trash",
            parent_a["id"].as_str().unwrap()
        ),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, page) = activity(app.clone(), &viewer.cookie, workspace_id, child_id, "").await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let parent = change_for(page["items"].as_array().unwrap(), "parentId");
    assert!(parent["from"]["label"].is_null(), "{page}");
    assert_eq!(parent["to"]["label"], "Parent B");
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_activity_records_move_status_and_recurrence_spawn() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let project = create_project(app.clone(), &cookie, workspace_id, "MOV", "workspace").await;
    let project_id = project["id"].as_str().unwrap();
    let (backlog_id, backlog_name) =
        workflow_status(app.clone(), &cookie, workspace_id, project_id, "backlog").await;
    let (done_id, done_name) =
        workflow_status(app.clone(), &cookie, workspace_id, project_id, "done").await;
    let task = create_task_with(
        app.clone(),
        &cookie,
        workspace_id,
        project_id,
        json!({"title": "Repeat", "recurrence": {"kind": "daily"}, "dueDate": "2026-01-01"}),
    )
    .await;
    let task_id = task["id"].as_str().unwrap();
    assert_eq!(task["statusId"], backlog_id);

    let (status, moved) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/move"),
        Some(json!({"statusId": done_id})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{moved}");

    let (status, page) = activity(app.clone(), &cookie, workspace_id, task_id, "").await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "{page}");
    let status_change = change_for(items, "statusId");
    assert_eq!(
        status_change["from"],
        json!({"id": backlog_id, "label": backlog_name})
    );
    assert_eq!(
        status_change["to"],
        json!({"id": done_id, "label": done_name})
    );
    let recurrence = change_for(items, "recurrence");
    assert_eq!(recurrence["from"], "daily");
    assert!(recurrence["to"].is_null());

    let spawned_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM fvoci.tasks WHERE workspace_id = $1 AND project_id = $2::uuid AND id <> $3::uuid",
    )
    .bind(workspace_id)
    .bind(project_id)
    .bind(task_id)
    .fetch_one(&admin)
    .await
    .expect("spawned occurrence");
    let (status, spawned) = activity(
        app.clone(),
        &cookie,
        workspace_id,
        &spawned_id.to_string(),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{spawned}");
    let spawned_items = spawned["items"].as_array().unwrap();
    assert_eq!(spawned_items.len(), 1, "{spawned}");
    assert_eq!(spawned_items[0]["kind"], "created");
    assert_eq!(spawned_items[0]["channel"], "system");

    // A status change through PATCH is recorded the same way.
    patch_task(
        app.clone(),
        &cookie,
        workspace_id,
        task_id,
        json!({"statusId": backlog_id}),
    )
    .await;
    let (_, page) = activity(app.clone(), &cookie, workspace_id, task_id, "?limit=1").await;
    let latest = page["items"][0]["changes"].as_array().unwrap();
    assert_eq!(latest.len(), 1, "{page}");
    assert_eq!(latest[0]["field"], "statusId");
    assert_eq!(latest[0]["to"]["id"], backlog_id);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_activity_records_assignee_and_label_diffs() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let project = create_project(app.clone(), &cookie, workspace_id, "ASG", "workspace").await;
    let project_id = project["id"].as_str().unwrap();
    let member = add_workspace_user(&admin, workspace_id, "member", "mina").await;
    let mut label_ids = Vec::new();
    for name in ["bug", "ui"] {
        let (status, label) = json_request(
            app.clone(),
            "POST",
            &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/labels"),
            Some(json!({"name": name, "color": "red"})),
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{label}");
        label_ids.push(label["id"].as_str().unwrap().to_string());
    }
    let task = create_task(app.clone(), &cookie, workspace_id, project_id, "People").await;
    let task_id = task["id"].as_str().unwrap();

    patch_task(
        app.clone(),
        &cookie,
        workspace_id,
        task_id,
        json!({
            "assigneeIds": [owner_id.to_string(), member.user_id.to_string()],
            "labelIds": [label_ids[0]],
        }),
    )
    .await;
    patch_task(
        app.clone(),
        &cookie,
        workspace_id,
        task_id,
        json!({
            "assigneeIds": [member.user_id.to_string()],
            "labelIds": [label_ids[0], label_ids[1]],
        }),
    )
    .await;
    // Same sets in another order: identity-based diff records nothing.
    patch_task(
        app.clone(),
        &cookie,
        workspace_id,
        task_id,
        json!({"labelIds": [label_ids[1], label_ids[0]]}),
    )
    .await;

    let (status, page) = activity(app.clone(), &cookie, workspace_id, task_id, "").await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), 3, "{page}");
    let latest = &items[0..1];
    let assignees = change_for(latest, "assigneeIds");
    assert_eq!(assignees["from"]["totalCount"], 2);
    assert_eq!(assignees["to"]["totalCount"], 1);
    assert_eq!(
        assignees["to"]["items"],
        json!([{"id": member.user_id.to_string(), "label": "mina"}])
    );
    let from_labels: HashSet<String> = assignees["from"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["label"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(from_labels, HashSet::from(["Owner".into(), "mina".into()]));
    let labels = change_for(latest, "labelIds");
    assert_eq!(labels["from"]["totalCount"], 1);
    assert_eq!(labels["from"]["items"][0]["label"], "bug");
    assert_eq!(labels["to"]["totalCount"], 2);
    let first = change_for(&items[1..2], "assigneeIds");
    assert_eq!(first["from"], json!({"items": [], "totalCount": 0}));
    admin.close().await;
    harness.cleanup().await;
}

async fn insert_activity_row(admin: &PgPool, workspace_id: Uuid, task_id: Uuid, at: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO fvoci.task_activity (id, workspace_id, task_id, channel, kind, changes, created_at)
        VALUES ($1, $2, $3, 'api', 'changed', '[{"field":"title","from":"a","to":"b"}]', $4::timestamptz)
        "#,
    )
    .bind(id)
    .bind(workspace_id)
    .bind(task_id)
    .bind(at)
    .execute(admin)
    .await
    .expect("insert activity");
    id
}

async fn insert_comment_row(
    admin: &PgPool,
    workspace_id: Uuid,
    task_id: Uuid,
    author: Uuid,
    body: &str,
    at: &str,
) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO fvoci.comments (id, workspace_id, task_id, created_by, body, created_at, updated_at)
        VALUES ($1, $2, $3, $4, $5, $6::timestamptz, $6::timestamptz)
        "#,
    )
    .bind(id)
    .bind(workspace_id)
    .bind(task_id)
    .bind(author)
    .bind(body)
    .bind(at)
    .execute(admin)
    .await
    .expect("insert comment");
    id
}

#[tokio::test]
async fn task_activity_keyset_pages_mixed_rows_with_equal_timestamps() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let project = create_project(app.clone(), &cookie, workspace_id, "KEY", "workspace").await;
    let project_id = project["id"].as_str().unwrap();
    let task = create_task(app.clone(), &cookie, workspace_id, project_id, "Keyset").await;
    let task_id = task["id"].as_str().unwrap();
    let task_uuid = Uuid::parse_str(task_id).unwrap();

    // Several rows share each timestamp, across both tables.
    for second in 0..4 {
        let at = format!("2030-01-01T00:00:0{second}.123456Z");
        for _ in 0..2 {
            insert_activity_row(&admin, workspace_id, task_uuid, &at).await;
            insert_comment_row(&admin, workspace_id, task_uuid, owner_id, "c", &at).await;
        }
    }
    let (status, full) = activity(app.clone(), &cookie, workspace_id, task_id, "?limit=100").await;
    assert_eq!(status, StatusCode::OK, "{full}");
    let full = full["items"].as_array().unwrap().clone();
    assert_eq!(full.len(), 17, "16 inserted rows plus the created row");

    for limit in [1, 2, 3, 5] {
        let paged =
            collect_all_pages(app.clone(), &cookie, workspace_id, task_id, "all", limit).await;
        assert_eq!(item_keys(&paged), item_keys(&full), "limit {limit}");
    }
    let keys = item_keys(&full);
    let unique: HashSet<_> = keys.iter().cloned().collect();
    assert_eq!(unique.len(), keys.len());
    // Newest first, ties broken by id then type (descending).
    for pair in full.windows(2) {
        let a = (
            pair[0]["createdAt"].as_str().unwrap(),
            pair[0]["id"].as_str().unwrap(),
            pair[0]["type"].as_str().unwrap(),
        );
        let b = (
            pair[1]["createdAt"].as_str().unwrap(),
            pair[1]["id"].as_str().unwrap(),
            pair[1]["type"].as_str().unwrap(),
        );
        assert!(a > b, "{a:?} before {b:?}");
    }

    let comments =
        collect_all_pages(app.clone(), &cookie, workspace_id, task_id, "comments", 3).await;
    assert_eq!(comments.len(), 8);
    assert!(comments.iter().all(|item| item["type"] == "comment"));
    let changes =
        collect_all_pages(app.clone(), &cookie, workspace_id, task_id, "changes", 3).await;
    assert_eq!(changes.len(), 9);
    assert!(changes.iter().all(|item| item["type"] == "change"));
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_activity_splits_pages_at_the_response_byte_budget() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let project = create_project(app.clone(), &cookie, workspace_id, "BIG", "workspace").await;
    let project_id = project["id"].as_str().unwrap();
    let task = create_task(app.clone(), &cookie, workspace_id, project_id, "Big").await;
    let task_id = task["id"].as_str().unwrap();
    let task_uuid = Uuid::parse_str(task_id).unwrap();
    // 8000 three-byte characters: about 24 KB of JSON per comment.
    let body = "가".repeat(8000);
    for n in 0..60 {
        let at = format!("2030-01-01T00:{:02}:00Z", n);
        insert_comment_row(&admin, workspace_id, task_uuid, owner_id, &body, &at).await;
    }

    let (status, page) = activity(
        app.clone(),
        &cookie,
        workspace_id,
        task_id,
        "?filter=comments&limit=100",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let first = page["items"].as_array().unwrap().len();
    assert!((30..60).contains(&first), "budget cut at {first}");
    assert!(serde_json::to_vec(&page).unwrap().len() <= 1_048_576);
    assert!(page["nextCursor"].as_str().is_some());

    let all = collect_all_pages(app.clone(), &cookie, workspace_id, task_id, "comments", 100).await;
    assert_eq!(all.len(), 60);
    let unique: HashSet<_> = item_keys(&all).into_iter().collect();
    assert_eq!(unique.len(), 60);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_activity_is_append_only_for_the_app_role() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let project = create_project(app.clone(), &cookie, workspace_id, "APP", "workspace").await;
    let project_id = project["id"].as_str().unwrap();
    let task = create_task(app.clone(), &cookie, workspace_id, project_id, "Log").await;
    let task_uuid = Uuid::parse_str(task["id"].as_str().unwrap()).unwrap();
    let pool = app_pool(&harness).await;

    let tenant_tx = || async {
        let mut tx = pool.begin().await.unwrap();
        fvoci_server::db::context::set_tenant(&mut tx, workspace_id)
            .await
            .unwrap();
        tx
    };

    let mut tx = tenant_tx().await;
    let visible: i64 =
        sqlx::query_scalar("SELECT count(*) FROM fvoci.task_activity WHERE task_id = $1")
            .bind(task_uuid)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(visible, 1);
    sqlx::query(
        "INSERT INTO fvoci.task_activity (id, workspace_id, task_id, channel, kind, changes) VALUES ($1, $2, $3, 'api', 'created', '[]')",
    )
    .bind(Uuid::now_v7())
    .bind(workspace_id)
    .bind(task_uuid)
    .execute(&mut *tx)
    .await
    .expect("app role may append");
    tx.rollback().await.unwrap();

    for statement in [
        "UPDATE fvoci.task_activity SET channel = 'system' WHERE task_id = $1",
        "DELETE FROM fvoci.task_activity WHERE task_id = $1",
    ] {
        let mut tx = tenant_tx().await;
        let err = sqlx::query(statement)
            .bind(task_uuid)
            .execute(&mut *tx)
            .await
            .expect_err(statement);
        let code = err
            .as_database_error()
            .and_then(|db| db.code())
            .map(|code| code.to_string());
        assert_eq!(code.as_deref(), Some("42501"), "{statement}: {err}");
        tx.rollback().await.unwrap();
    }

    // Another tenant can neither read nor append.
    let mut tx = pool.begin().await.unwrap();
    fvoci_server::db::context::set_tenant(&mut tx, Uuid::now_v7())
        .await
        .unwrap();
    let foreign: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.task_activity")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(foreign, 0);
    let insert = sqlx::query(
        "INSERT INTO fvoci.task_activity (id, workspace_id, task_id, channel, kind, changes) VALUES ($1, $2, $3, 'api', 'created', '[]')",
    )
    .bind(Uuid::now_v7())
    .bind(workspace_id)
    .bind(task_uuid)
    .execute(&mut *tx)
    .await;
    assert!(insert.is_err(), "cross-tenant append must fail");
    tx.rollback().await.unwrap();
    pool.close().await;
    harness.cleanup().await;
}
