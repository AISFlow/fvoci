#![cfg(feature = "db-tests")]
#![allow(dead_code)]

//! Document tags, collections (fields, values, query, views) and project saved
//! views on a real PostgreSQL with the non-superuser app role (RLS forced).

#[path = "support/project_harness.rs"]
mod project_harness;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use project_harness::{
    add_workspace_user, admin_pool, app_pool, create_project, json_request, setup_session, TestDb,
};
use serde_json::{json, Value};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

async fn call(
    app: &axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    cookie: &str,
) -> (StatusCode, Value) {
    json_request(app.clone(), method, path, body, Some(cookie)).await
}

async fn raw_get(app: &axum::Router, path: &str) -> (StatusCode, String) {
    let mut request = Request::builder()
        .method("GET")
        .uri(path)
        .body(Body::empty())
        .unwrap();
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(project_harness::test_peer()));
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

async fn create_wiki_doc(app: &axum::Router, cookie: &str, ws: Uuid, title: &str) -> String {
    let (status, body) = call(
        app,
        "POST",
        &format!("/api/v1/workspaces/{ws}/documents"),
        Some(json!({"parentId": null, "title": title})),
        cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["id"].as_str().unwrap().to_string()
}

async fn create_task(
    app: &axum::Router,
    cookie: &str,
    ws: Uuid,
    project: &str,
    body: Value,
) -> String {
    let (status, created) = call(
        app,
        "POST",
        &format!("/api/v1/workspaces/{ws}/projects/{project}/tasks"),
        Some(body),
        cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    created["id"].as_str().unwrap().to_string()
}

async fn project_collection(app: &axum::Router, cookie: &str, ws: Uuid, project: &str) -> Value {
    let (status, body) = call(
        app,
        "GET",
        &format!("/api/v1/workspaces/{ws}/projects/{project}/collection"),
        None,
        cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

async fn create_field(
    app: &axum::Router,
    cookie: &str,
    ws: Uuid,
    collection: &str,
    body: Value,
) -> Value {
    let (status, field) = call(
        app,
        "POST",
        &format!("/api/v1/workspaces/{ws}/collections/{collection}/fields"),
        Some(body),
        cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{field}");
    field
}

async fn item_of_task(app: &axum::Router, cookie: &str, ws: Uuid, task: &str) -> Value {
    let (status, body) = call(
        app,
        "GET",
        &format!("/api/v1/workspaces/{ws}/tasks/{task}/collection-item"),
        None,
        cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["item"].clone()
}

async fn put_value(
    app: &axum::Router,
    cookie: &str,
    ws: Uuid,
    collection: &str,
    item: &Value,
    field: &Value,
    value: Value,
) -> (StatusCode, Value) {
    call(
        app,
        "PUT",
        &format!(
            "/api/v1/workspaces/{ws}/collections/{collection}/items/{}/values",
            item["id"].as_str().unwrap()
        ),
        Some(json!({
            "fieldId": field["id"],
            "expectedVersion": item["version"],
            "expectedFieldVersion": field["version"],
            "value": value,
        })),
        cookie,
    )
    .await
}

async fn query(
    app: &axum::Router,
    cookie: &str,
    ws: Uuid,
    collection: &str,
    body: Value,
) -> (StatusCode, Value) {
    call(
        app,
        "POST",
        &format!("/api/v1/workspaces/{ws}/collections/{collection}/query"),
        Some(body),
        cookie,
    )
    .await
}

async fn add_project_member(
    app: &axum::Router,
    cookie: &str,
    ws: Uuid,
    project: &str,
    user: Uuid,
    role: &str,
) {
    let (status, body) = call(
        app,
        "POST",
        &format!("/api/v1/workspaces/{ws}/projects/{project}/members"),
        Some(json!({"userId": user, "role": role})),
        cookie,
    )
    .await;
    assert!(status.is_success(), "{status} {body}");
}

async fn second_workspace(admin: &PgPool, owner: Uuid) -> Uuid {
    let ws = Uuid::now_v7();
    sqlx::query("INSERT INTO fvoci.workspaces (id, slug, name) VALUES ($1, $2, 'Other')")
        .bind(ws)
        .bind(format!("other-{}", &ws.simple().to_string()[20..]))
        .execute(admin)
        .await
        .expect("insert workspace");
    sqlx::query(
        "INSERT INTO fvoci.memberships (workspace_id, user_id, role) VALUES ($1, $2, 'owner')",
    )
    .bind(ws)
    .bind(owner)
    .execute(admin)
    .await
    .expect("insert membership");
    ws
}

#[tokio::test]
async fn document_tags_pool_assignment_roles_and_tree_filter() {
    let harness = TestDb::bootstrap().await;
    let (app, owner, owner_id, ws) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, ws, "member", "member").await;
    let guest = add_workspace_user(&admin, ws, "guest", "guest").await;
    let base = format!("/api/v1/workspaces/{ws}/document-tags");

    // Members create tags; names are unique case-insensitively; guests cannot.
    let (status, tag) = call(
        &app,
        "POST",
        &base,
        Some(json!({"name": "  기획  "})),
        &member.cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{tag}");
    assert_eq!(tag["name"], "기획");
    assert_eq!(tag["color"], "gray");
    let tag_id = tag["id"].as_str().unwrap().to_string();
    let (status, _) = call(
        &app,
        "POST",
        &base,
        Some(json!({"name": "Spec", "color": "blue"})),
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, body) = call(&app, "POST", &base, Some(json!({"name": "spec"})), &owner).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let (status, _) = call(
        &app,
        "POST",
        &base,
        Some(json!({"name": "x", "color": "black"})),
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = call(
        &app,
        "POST",
        &base,
        Some(json!({"name": "x", "extra": 1})),
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, body) = call(
        &app,
        "POST",
        &base,
        Some(json!({"name": "guest-tag"})),
        &guest.cookie,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    // Rename/recolor/delete are admin-only.
    let tag_path = format!("{base}/{tag_id}");
    let (status, _) = call(
        &app,
        "PATCH",
        &tag_path,
        Some(json!({"color": "red"})),
        &member.cookie,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = call(
        &app,
        "PATCH",
        &tag_path,
        Some(json!({"name": "SPEC"})),
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let (status, body) = call(
        &app,
        "PATCH",
        &tag_path,
        Some(json!({"color": "red"})),
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["color"], "red");
    let (status, _) = call(&app, "PATCH", &tag_path, Some(json!({})), &owner).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Assignment on a wiki document needs edit; listing needs view.
    let doc = create_wiki_doc(&app, &owner, ws, "Roadmap").await;
    let other_doc = create_wiki_doc(&app, &owner, ws, "Other").await;
    let doc_tags = format!("/api/v1/workspaces/{ws}/documents/{doc}/tags");
    let (status, body) = call(
        &app,
        "POST",
        &doc_tags,
        Some(json!({"tagId": tag_id})),
        &member.cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["id"], tag_id.as_str());
    let (status, _) = call(
        &app,
        "POST",
        &doc_tags,
        Some(json!({"tagId": tag_id})),
        &member.cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "assignment is idempotent");
    let (status, _) = call(
        &app,
        "POST",
        &doc_tags,
        Some(json!({"tagId": tag_id})),
        &guest.cookie,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "guest without a grant cannot see the doc"
    );
    let (status, _) = call(
        &app,
        "POST",
        &doc_tags,
        Some(json!({"tagId": Uuid::now_v7()})),
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) = call(&app, "GET", &doc_tags, None, &member.cookie).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["items"].as_array().unwrap().len(), 1);

    // A wiki document is not reachable through a project route.
    let project = create_project(app.clone(), &owner, ws, "TAGP", "workspace").await;
    let project_id = project["id"].as_str().unwrap();
    let (status, _) = call(
        &app,
        "GET",
        &format!("/api/v1/workspaces/{ws}/projects/{project_id}/documents/{doc}/tags"),
        None,
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Pool: counts, capability flags, substring filter, limit bounds.
    let (status, pool) = call(
        &app,
        "GET",
        &format!("{base}?q=%EA%B8%B0"),
        None,
        &member.cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{pool}");
    assert_eq!(pool["canCreate"], true);
    assert_eq!(pool["canManage"], false);
    assert_eq!(pool["items"].as_array().unwrap().len(), 1);
    assert_eq!(pool["items"][0]["assignmentCount"], 1);
    let (status, _) = call(&app, "GET", &format!("{base}?limit=101"), None, &owner).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Tree filter by tag.
    let (status, tree) = call(
        &app,
        "GET",
        &format!("/api/v1/workspaces/{ws}/tree?tag={tag_id}"),
        None,
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let ids: Vec<&str> = tree["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec![doc.as_str()]);
    assert!(!ids.contains(&other_doc.as_str()));

    // Tenant isolation: the tag id means nothing in another workspace.
    let other_ws = second_workspace(&admin, owner_id).await;
    let (status, _) = call(
        &app,
        "POST",
        &format!("/api/v1/workspaces/{other_ws}/documents/{doc}/tags"),
        Some(json!({"tagId": tag_id})),
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(
        &app,
        "DELETE",
        &format!("/api/v1/workspaces/{other_ws}/document-tags/{tag_id}"),
        None,
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Unassign twice → second is 404; deleting the tag cascades.
    let (status, _) = call(
        &app,
        "DELETE",
        &format!("{doc_tags}/{tag_id}"),
        None,
        &member.cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(
        &app,
        "DELETE",
        &format!("{doc_tags}/{tag_id}"),
        None,
        &member.cookie,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    call(
        &app,
        "POST",
        &doc_tags,
        Some(json!({"tagId": tag_id})),
        &owner,
    )
    .await;
    let (status, _) = call(&app, "DELETE", &tag_path, None, &member.cookie).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(&app, "DELETE", &tag_path, None, &owner).await;
    assert_eq!(status, StatusCode::OK);
    let left: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM fvoci.document_tag_assignments WHERE tag_id = $1::uuid",
    )
    .bind(&tag_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(left, 0);
    let audited: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM fvoci.audit_log WHERE verb LIKE 'document_tag.%' AND target_id = $1::uuid",
    )
    .bind(&tag_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(audited, 3, "created, updated, deleted");

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn task_collection_fields_values_and_permissions() {
    let harness = TestDb::bootstrap().await;
    let (app, owner, owner_id, ws) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let viewer = add_workspace_user(&admin, ws, "member", "viewer").await;
    let outsider = add_workspace_user(&admin, ws, "member", "outsider").await;
    let project = create_project(app.clone(), &owner, ws, "COLL", "private").await;
    let project_id = project["id"].as_str().unwrap().to_string();
    add_project_member(&app, &owner, ws, &project_id, viewer.user_id, "viewer").await;

    // Every project has a task collection; every task is already an item.
    let collection = project_collection(&app, &owner, ws, &project_id).await;
    assert_eq!(collection["kind"], "task");
    assert_eq!(collection["canEdit"], true);
    assert_eq!(collection["canManage"], true);
    let cid = collection["id"].as_str().unwrap().to_string();
    let task = create_task(&app, &owner, ws, &project_id, json!({"title": "T1"})).await;
    let item = item_of_task(&app, &owner, ws, &task).await;
    assert_eq!(item["collectionId"], cid.as_str());
    let viewer_collection = project_collection(&app, &viewer.cookie, ws, &project_id).await;
    assert_eq!(viewer_collection["canEdit"], false);
    let (status, _) = call(
        &app,
        "GET",
        &format!("/api/v1/workspaces/{ws}/projects/{project_id}/collection"),
        None,
        &outsider.cookie,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "private project hides its collection"
    );
    let (status, _) = call(
        &app,
        "POST",
        &format!("/api/v1/workspaces/{ws}/collections"),
        Some(json!({"name": "x", "kind": "task", "projectId": project_id})),
        &owner,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "task collections come with projects"
    );

    // Fields: option-only types, key rules, viewer cannot configure.
    let status_field = create_field(
        &app,
        &owner,
        ws,
        &cid,
        json!({"name": "단계", "type": "select", "options": ["기획", "개발", "완료"]}),
    )
    .await;
    assert_eq!(status_field["key"], "f_1");
    assert_eq!(status_field["options"].as_array().unwrap().len(), 3);
    let points = create_field(
        &app,
        &owner,
        ws,
        &cid,
        json!({"name": "Points", "key": "points", "type": "number"}),
    )
    .await;
    let note = create_field(
        &app,
        &owner,
        ws,
        &cid,
        json!({"name": "Note", "type": "text", "description": "memo"}),
    )
    .await;
    let owner_field = create_field(
        &app,
        &owner,
        ws,
        &cid,
        json!({"name": "Owner", "type": "user"}),
    )
    .await;
    let when = create_field(
        &app,
        &owner,
        ws,
        &cid,
        json!({"name": "When", "type": "date"}),
    )
    .await;
    let fields_path = format!("/api/v1/workspaces/{ws}/collections/{cid}/fields");
    for bad in [
        json!({"name": "x", "type": "text", "options": ["a"]}),
        json!({"name": "x", "type": "select", "key": "Bad Key"}),
        json!({"name": "x", "type": "select", "key": "points"}),
        json!({"name": "x", "type": "formula"}),
    ] {
        let (status, body) = call(&app, "POST", &fields_path, Some(bad.clone()), &owner).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad} {body}");
    }
    let (status, _) = call(
        &app,
        "POST",
        &fields_path,
        Some(json!({"name": "v", "type": "text"})),
        &viewer.cookie,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(&app, "GET", &fields_path, None, &outsider.cookie).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Field patch: CAS, stable option ids, archiving an option.
    let field_path = format!("{fields_path}/{}", status_field["id"].as_str().unwrap());
    let options = status_field["options"].as_array().unwrap().clone();
    let (status, body) = call(
        &app,
        "PATCH",
        &field_path,
        Some(json!({"expectedVersion": 9, "name": "x"})),
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "document_version_mismatch");
    let (status, _) = call(
        &app,
        "PATCH",
        &field_path,
        Some(json!({"expectedVersion": 1, "options": [{"id": options[0]["id"], "label": "a"}]})),
        &owner,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "every existing option id must be kept"
    );
    let (status, patched) = call(
        &app,
        "PATCH",
        &field_path,
        Some(json!({"expectedVersion": 1, "options": [
            {"id": options[0]["id"], "label": "기획"},
            {"label": "검토"},
            {"id": options[1]["id"], "label": "개발"},
            {"id": options[2]["id"], "label": "완료", "deleted": true},
        ]})),
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{patched}");
    assert_eq!(patched["version"], 2);
    let patched_options = patched["options"].as_array().unwrap();
    assert_eq!(patched_options[1]["label"], "검토");
    assert_eq!(
        patched_options[1]["key"], "o_4",
        "new keys never reuse live ones"
    );
    assert!(patched_options[3]["deletedAt"].is_string());

    // Values: type checks, CAS on item and field versions, option/user rules.
    let (status, _) = put_value(&app, &owner, ws, &cid, &item, &points, json!({"text": "3"})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, body) = put_value(
        &app,
        &owner,
        ws,
        &cid,
        &item,
        &points,
        json!({"number": 3.5}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["version"], 2);
    let (status, body) =
        put_value(&app, &owner, ws, &cid, &item, &points, json!({"number": 4})).await;
    assert_eq!(status, StatusCode::CONFLICT, "stale item version: {body}");
    let item = item_of_task(&app, &owner, ws, &task).await;
    let (status, _) = put_value(
        &app,
        &owner,
        ws,
        &cid,
        &item,
        &status_field,
        json!({"options": [options[0]["id"]]}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "stale field version");
    let status_field = patched;
    let two = json!({"options": [options[0]["id"], options[1]["id"]]});
    let (status, _) = put_value(&app, &owner, ws, &cid, &item, &status_field, two).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "select takes one option");
    let (status, _) = put_value(
        &app,
        &owner,
        ws,
        &cid,
        &item,
        &status_field,
        json!({"options": [options[2]["id"]]}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "archived option cannot be newly chosen"
    );
    let (status, _) = put_value(
        &app,
        &owner,
        ws,
        &cid,
        &item,
        &status_field,
        json!({"options": [Uuid::now_v7()]}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = put_value(
        &app,
        &owner,
        ws,
        &cid,
        &item,
        &owner_field,
        json!({"users": [Uuid::now_v7()]}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "user must be a member");
    let (status, _) = put_value(
        &app,
        &owner,
        ws,
        &cid,
        &item,
        &owner_field,
        json!({"users": [owner_id, viewer.user_id]}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "single user field");
    let (status, _) = put_value(
        &app,
        &owner,
        ws,
        &cid,
        &item,
        &note,
        json!({"text": "x".repeat(10_001)}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = put_value(
        &app,
        &owner,
        ws,
        &cid,
        &item,
        &when,
        json!({"date": "2026-13-01"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = put_value(
        &app,
        &viewer.cookie,
        ws,
        &cid,
        &item,
        &note,
        json!({"text": "v"}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "project viewer cannot write values"
    );
    let (status, _) = put_value(
        &app,
        &outsider.cookie,
        ws,
        &cid,
        &item,
        &note,
        json!({"text": "v"}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) = put_value(
        &app,
        &owner,
        ws,
        &cid,
        &item,
        &owner_field,
        json!({"users": [viewer.user_id]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let item = item_of_task(&app, &owner, ws, &task).await;
    let (status, _) = put_value(
        &app,
        &owner,
        ws,
        &cid,
        &item,
        &note,
        json!({"text": "메모 😀"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // The item lookup carries the item's values and the caller's edit right.
    let (_, lookup) = call(
        &app,
        "GET",
        &format!("/api/v1/workspaces/{ws}/tasks/{task}/collection-item"),
        None,
        &viewer.cookie,
    )
    .await;
    assert_eq!(
        lookup["values"][note["id"].as_str().unwrap()],
        json!({"text": "메모 😀"})
    );
    assert_eq!(lookup["canEdit"], false);
    let (_, lookup) = call(
        &app,
        "GET",
        &format!("/api/v1/workspaces/{ws}/tasks/{task}/collection-item"),
        None,
        &owner,
    )
    .await;
    assert_eq!(lookup["canEdit"], true);

    // Values are visible in the query; clearing with null removes the row.
    let (status, result) = query(&app, &viewer.cookie, ws, &cid, json!({"config": {}})).await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["canEdit"], false);
    let row = &result["items"][0];
    assert_eq!(row["canEdit"], false);
    assert!(row["displayId"].as_str().unwrap().starts_with("COLL-"));
    assert_eq!(
        row["values"][points["id"].as_str().unwrap()],
        json!({"number": 3.5})
    );
    assert_eq!(
        row["values"][note["id"].as_str().unwrap()],
        json!({"text": "메모 😀"})
    );
    assert_eq!(
        row["values"][owner_field["id"].as_str().unwrap()],
        json!({"users": [viewer.user_id]})
    );
    let item = item_of_task(&app, &owner, ws, &task).await;
    let (status, _) = put_value(&app, &owner, ws, &cid, &item, &note, Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    let (_, result) = query(&app, &owner, ws, &cid, json!({"config": {}})).await;
    assert!(result["items"][0]["values"]
        .get(note["id"].as_str().unwrap())
        .is_none());
    assert_eq!(result["canEdit"], true);

    // Archived project: reads work, writes are rejected.
    sqlx::query("UPDATE fvoci.projects SET status = 'archived' WHERE id = $1::uuid")
        .bind(&project_id)
        .execute(&admin)
        .await
        .unwrap();
    let item = item_of_task(&app, &owner, ws, &task).await;
    let (status, body) =
        put_value(&app, &owner, ws, &cid, &item, &points, json!({"number": 1})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "project_archived");
    let (status, body) = call(
        &app,
        "POST",
        &fields_path,
        Some(json!({"name": "late", "type": "text"})),
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let (_, result) = query(&app, &owner, ws, &cid, json!({"config": {}})).await;
    assert_eq!(result["canEdit"], false);

    admin.close().await;
    harness.cleanup().await;
}

/// Keyset paging over a custom number sort with many equal keys and NULLs:
/// pages concatenate to exactly the full ordered set, ties broken by id.
#[tokio::test]
async fn collection_query_paging_filters_groups_and_calendar() {
    let harness = TestDb::bootstrap().await;
    let (app, owner, _owner_id, ws) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let project = create_project(app.clone(), &owner, ws, "PAGE", "workspace").await;
    let project_id = project["id"].as_str().unwrap().to_string();
    let cid = project_collection(&app, &owner, ws, &project_id).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let points = create_field(
        &app,
        &owner,
        ws,
        &cid,
        json!({"name": "Points", "type": "number"}),
    )
    .await;
    let stage = create_field(
        &app,
        &owner,
        ws,
        &cid,
        json!({"name": "Stage", "type": "select", "options": ["A", "B"]}),
    )
    .await;
    let option_a = stage["options"][0]["id"].clone();
    let option_b = stage["options"][1]["id"].clone();
    let mut tasks = Vec::new();
    for index in 0..11 {
        let due = format!("2026-10-{:02}", 1 + index % 3);
        let task = create_task(
            &app,
            &owner,
            ws,
            &project_id,
            json!({"title": format!("task {index}"), "dueDate": due}),
        )
        .await;
        let item = item_of_task(&app, &owner, ws, &task).await;
        // 0,1,2 → 5; 3..7 → 7; 8,9 → unset; 10 → 1
        let value = match index {
            0..=2 => Some(5),
            3..=7 => Some(7),
            10 => Some(1),
            _ => None,
        };
        let mut item = item;
        if let Some(value) = value {
            let (status, body) = put_value(
                &app,
                &owner,
                ws,
                &cid,
                &item,
                &points,
                json!({"number": value}),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{body}");
            item = item_of_task(&app, &owner, ws, &task).await;
        }
        let option = if index % 2 == 0 { &option_a } else { &option_b };
        if index != 9 {
            let (status, body) = put_value(
                &app,
                &owner,
                ws,
                &cid,
                &item,
                &stage,
                json!({"options": [option]}),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{body}");
        }
        tasks.push((task, item["id"].as_str().unwrap().to_string(), value));
    }
    let config = json!({"query": {"sort": [{"field": points["id"], "direction": "desc"}]}});

    let (status, full) = query(
        &app,
        &owner,
        ws,
        &cid,
        json!({"config": config, "limit": 100}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{full}");
    assert_eq!(full["count"], 11);
    let full_ids: Vec<String> = full["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap().to_string())
        .collect();
    // Expected order: value desc, NULLs last, then id ascending within ties.
    let mut expected: Vec<(Option<i32>, String)> = tasks
        .iter()
        .map(|(_, item, value)| (*value, item.clone()))
        .collect();
    expected.sort_by(|a, b| match (a.0, b.0) {
        (Some(x), Some(y)) => y.cmp(&x).then(a.1.cmp(&b.1)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.1.cmp(&b.1),
    });
    let expected: Vec<String> = expected.into_iter().map(|(_, id)| id).collect();
    assert_eq!(full_ids, expected);

    let mut paged = Vec::new();
    let mut cursor: Option<String> = None;
    let mut first_cursor = None;
    for _ in 0..20 {
        let mut body = json!({"config": config, "limit": 2});
        if let Some(cursor) = &cursor {
            body["cursor"] = json!(cursor);
        }
        let (status, page) = query(&app, &owner, ws, &cid, body).await;
        assert_eq!(status, StatusCode::OK, "{page}");
        paged.extend(
            page["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|i| i["id"].as_str().unwrap().to_string()),
        );
        cursor = page["nextCursor"].as_str().map(str::to_string);
        if first_cursor.is_none() {
            first_cursor = cursor.clone();
        }
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(
        paged, expected,
        "keyset pages are complete and duplicate-free"
    );

    // A cursor is bound to its query: other sort, tampered, or garbage → 400.
    let first_cursor = first_cursor.unwrap();
    let (status, body) = query(
        &app,
        &owner,
        ws,
        &cid,
        json!({"config": {}, "cursor": first_cursor}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["params"]["code"], "invalid_cursor");
    let (status, _) = query(
        &app,
        &owner,
        ws,
        &cid,
        json!({"config": config, "cursor": "bm90LWpzb24"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Custom filters: equals on a number, empty, option equality.
    let (_, sevens) = query(&app, &owner, ws, &cid, json!({"config": {"query": {"filters": {"custom": [{"fieldId": points["id"], "operator": "equals", "value": 7}]}}}})).await;
    assert_eq!(sevens["count"], 5);
    let (_, empty) = query(&app, &owner, ws, &cid, json!({"config": {"query": {"filters": {"custom": [{"fieldId": points["id"], "operator": "empty"}]}}}})).await;
    assert_eq!(empty["count"], 2);
    let (_, only_b) = query(&app, &owner, ws, &cid, json!({"config": {"query": {"filters": {"custom": [{"fieldId": stage["id"], "operator": "equals", "value": option_b}]}}}})).await;
    assert_eq!(only_b["count"], 4);
    for bad in [
        json!({"config": {"query": {"filters": {"custom": [{"fieldId": points["id"], "operator": "equals", "value": "7"}]}}}}),
        json!({"config": {"query": {"filters": {"custom": [{"fieldId": stage["id"], "operator": "equals", "value": Uuid::now_v7()}]}}}}),
        json!({"config": {"query": {"filters": {"custom": [{"fieldId": Uuid::now_v7(), "operator": "empty"}]}}}}),
        json!({"config": {"query": {"sort": [{"field": stage["id"], "direction": "asc"}]}}}),
        json!({"config": {"query": {"filters": {"statusId": Uuid::now_v7()}}}}),
        json!({"config": {"groupBy": points["id"]}}),
        json!({"config": {"dateBy": "due"}, "day": "2026-10-01"}),
        json!({"config": {"dateBy": "due"}, "window": {"from": "2026-10-01", "to": "2027-12-01", "timeZone": "UTC"}}),
        json!({"config": {"dateBy": "due"}, "window": {"from": "2026-10-01", "to": "2026-11-01", "timeZone": "Mars/Base"}}),
        json!({"config": {}, "limit": 0}),
        json!({"config": {}, "sql": "1;DROP TABLE x"}),
    ] {
        let (status, body) = query(&app, &owner, ws, &cid, bad.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad} → {body}");
    }
    // Filter text is data, never SQL.
    let (status, none) = query(
        &app,
        &owner,
        ws,
        &cid,
        json!({"config": {"query": {"filters": {"title": "' OR 1=1 --"}}}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(none["count"], 0);

    // Board grouping by the select field: catalog order + "no value" bucket.
    let (status, board) = query(
        &app,
        &owner,
        ws,
        &cid,
        json!({"config": {"groupBy": stage["id"]}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{board}");
    let groups = board["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 3);
    assert_eq!(groups[0]["name"], "A");
    assert_eq!(groups[0]["count"], 6);
    assert_eq!(groups[1]["count"], 4);
    assert_eq!(groups[2]["id"], Value::Null);
    assert_eq!(groups[2]["count"], 1);
    let (_, lane) = query(
        &app,
        &owner,
        ws,
        &cid,
        json!({"config": {"groupBy": stage["id"]}, "group": option_b}),
    )
    .await;
    assert_eq!(lane["items"].as_array().unwrap().len(), 4);
    let (_, by_status) = query(
        &app,
        &owner,
        ws,
        &cid,
        json!({"config": {"groupBy": "status"}}),
    )
    .await;
    let status_total: i64 = by_status["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["count"].as_i64().unwrap())
        .sum();
    assert_eq!(status_total, 11);

    // Calendar: per-day counts over the window, ≤3 previews per day.
    let (status, month) = query(
        &app,
        &owner,
        ws,
        &cid,
        json!({"config": {"dateBy": "due"}, "window": {"from": "2026-10-01", "to": "2026-10-04", "timeZone": "Asia/Seoul"}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{month}");
    let days = month["days"].as_array().unwrap();
    assert_eq!(days.len(), 4, "three days + undated bucket");
    assert_eq!(days[0], json!({"date": "2026-10-01", "count": 4}));
    assert_eq!(days[3]["date"], Value::Null);
    let previews = month["previews"].as_array().unwrap();
    assert_eq!(previews.len(), 9);
    assert!(previews.iter().all(|p| p["canEdit"] == true));
    let (_, one_day) = query(
        &app,
        &owner,
        ws,
        &cid,
        json!({"config": {"dateBy": "due"}, "window": {"from": "2026-10-01", "to": "2026-10-04", "timeZone": "UTC"}, "day": "2026-10-02"}),
    )
    .await;
    assert_eq!(one_day["items"].as_array().unwrap().len(), 4);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn collection_views_visibility_manage_and_cas() {
    let harness = TestDb::bootstrap().await;
    let (app, owner, owner_id, ws) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, ws, "member", "member").await;
    let project = create_project(app.clone(), &owner, ws, "VIEW", "workspace").await;
    let project_id = project["id"].as_str().unwrap().to_string();
    let cid = project_collection(&app, &owner, ws, &project_id).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let when = create_field(
        &app,
        &owner,
        ws,
        &cid,
        json!({"name": "When", "type": "datetime"}),
    )
    .await;
    let views = format!("/api/v1/workspaces/{ws}/collections/{cid}/views");

    let private = json!({"name": "내 달력", "type": "calendar", "visibility": "private", "config": {"dateBy": when["id"]}});
    let (status, mine) = call(&app, "POST", &views, Some(private), &member.cookie).await;
    assert_eq!(status, StatusCode::CREATED, "{mine}");
    assert_eq!(mine["config"]["query"], json!({"filters": {}, "sort": []}));
    let shared = json!({"name": "Team", "type": "board", "visibility": "shared", "config": {"groupBy": "status"}});
    let (status, _) = call(&app, "POST", &views, Some(shared.clone()), &member.cookie).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "member (edit) cannot share");
    let (status, team) = call(&app, "POST", &views, Some(shared), &owner).await;
    assert_eq!(status, StatusCode::CREATED, "{team}");
    let (status, _) = call(&app, "POST", &views, Some(json!({"name": "bad", "type": "table", "visibility": "private", "config": {"dateBy": "start", "groupBy": Uuid::now_v7()}})), &owner).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, list) = call(&app, "GET", &views, None, &owner).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["canManage"], true);
    let names: Vec<&str> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        vec!["Team"],
        "other members' private views stay private"
    );
    let (_, list) = call(&app, "GET", &views, None, &member.cookie).await;
    assert_eq!(list["canManage"], false);
    assert_eq!(list["items"].as_array().unwrap().len(), 2);

    // CAS update; members cannot edit or delete shared views.
    let team_path = format!("{views}/{}", team["id"].as_str().unwrap());
    let patch = json!({"name": "Team 2", "type": "board", "visibility": "shared", "config": {"groupBy": "status"}, "expectedVersion": 1});
    let (status, _) = call(
        &app,
        "PATCH",
        &team_path,
        Some(patch.clone()),
        &member.cookie,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, updated) = call(&app, "PATCH", &team_path, Some(patch.clone()), &owner).await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["version"], 2);
    let (status, body) = call(&app, "PATCH", &team_path, Some(patch), &owner).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let mine_path = format!("{views}/{}", mine["id"].as_str().unwrap());
    let (status, _) = call(&app, "DELETE", &mine_path, None, &owner).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "someone else's private view is invisible"
    );
    let (status, _) = call(&app, "DELETE", &team_path, None, &member.cookie).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Removing a member hands their shared views to the remover.
    sqlx::query(
        "UPDATE fvoci.memberships SET role = 'admin' WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(ws)
    .bind(member.user_id)
    .execute(&admin)
    .await
    .unwrap();
    let (status, their) = call(
        &app,
        "POST",
        &views,
        Some(json!({"name": "Theirs", "type": "table", "visibility": "shared", "config": {}})),
        &member.cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{their}");
    let (status, body) = call(
        &app,
        "DELETE",
        &format!("/api/v1/workspaces/{ws}/members/{}", member.user_id),
        None,
        &owner,
    )
    .await;
    assert!(status.is_success(), "{status} {body}");
    let owner_now: Uuid =
        sqlx::query_scalar("SELECT owner_id FROM fvoci.collection_views WHERE id = $1::uuid")
            .bind(their["id"].as_str().unwrap())
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(owner_now, owner_id);
    let private_left: i64 =
        sqlx::query_scalar("SELECT count(*) FROM fvoci.collection_views WHERE id = $1::uuid")
            .bind(mine["id"].as_str().unwrap())
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(private_left, 0, "private views go with the membership");

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn wiki_document_collection_attach_guest_and_tenant_isolation() {
    let harness = TestDb::bootstrap().await;
    let (app, owner, owner_id, ws) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, ws, "member", "member").await;
    let guest = add_workspace_user(&admin, ws, "guest", "guest").await;
    let base = format!("/api/v1/workspaces/{ws}/collections");
    let (status, collection) = call(
        &app,
        "POST",
        &base,
        Some(json!({"name": "회의록", "kind": "document", "projectId": null})),
        &member.cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{collection}");
    let cid = collection["id"].as_str().unwrap().to_string();
    let (status, _) = call(
        &app,
        "POST",
        &base,
        Some(json!({"name": "g", "kind": "document", "projectId": null})),
        &guest.cookie,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "guest has no wiki base access"
    );

    let doc = create_wiki_doc(&app, &owner, ws, "2026-09-01 회의").await;
    let items = format!("{base}/{cid}/items");
    let (status, item) = call(
        &app,
        "POST",
        &items,
        Some(json!({"documentId": doc})),
        &member.cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{item}");
    let (status, again) = call(
        &app,
        "POST",
        &items,
        Some(json!({"documentId": doc})),
        &member.cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(again["id"], item["id"], "attach is idempotent");
    let (status, _) = call(
        &app,
        "POST",
        &items,
        Some(json!({"documentId": doc, "taskId": doc})),
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, second) = call(
        &app,
        "POST",
        &base,
        Some(json!({"name": "Other", "kind": "document", "projectId": null})),
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = call(
        &app,
        "POST",
        &format!("{base}/{}/items", second["id"].as_str().unwrap()),
        Some(json!({"documentId": doc})),
        &owner,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a document belongs to one collection"
    );
    let project = create_project(app.clone(), &owner, ws, "WIKC", "workspace").await;
    let task = create_task(
        &app,
        &owner,
        ws,
        project["id"].as_str().unwrap(),
        json!({"title": "t"}),
    )
    .await;
    let (status, _) = call(&app, "POST", &items, Some(json!({"taskId": task})), &owner).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "kind mismatch");

    let (status, lookup) = call(
        &app,
        "GET",
        &format!("/api/v1/workspaces/{ws}/documents/{doc}/collection-item"),
        None,
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(lookup["item"]["collectionId"], cid.as_str());
    let lonely = create_wiki_doc(&app, &owner, ws, "lonely").await;
    let (_, lookup) = call(
        &app,
        "GET",
        &format!("/api/v1/workspaces/{ws}/documents/{lonely}/collection-item"),
        None,
        &owner,
    )
    .await;
    assert_eq!(lookup["item"], Value::Null);
    assert_eq!(lookup["values"], json!({}));

    // Guests only see wiki collections through a document they can read.
    let (_, list) = call(&app, "GET", &base, None, &guest.cookie).await;
    assert!(list["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|c| c["id"] != cid.as_str()));
    let (status, _) = query(&app, &guest.cookie, ws, &cid, json!({"config": {}})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, list) = call(&app, "GET", &base, None, &member.cookie).await;
    assert!(list["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["id"] == cid.as_str()));
    let (status, result) = query(
        &app,
        &member.cookie,
        ws,
        &cid,
        json!({"config": {"query": {"sort": [{"field": "title", "direction": "asc"}]}}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(
        result["items"][0]["displayId"]
            .as_str()
            .unwrap()
            .split('-')
            .next(),
        Some("WIKI")
    );
    assert_eq!(result["items"][0]["canEdit"], true);
    let (status, _) = query(
        &app,
        &owner,
        ws,
        &cid,
        json!({"config": {"groupBy": "status"}}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "documents have no task status"
    );
    let (status, _) = query(
        &app,
        &owner,
        ws,
        &cid,
        json!({"config": {"query": {"filters": {"openOnly": true}}}}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "task filters are invalid on documents"
    );

    // Trashed documents drop out of the query.
    let (status, _) = call(
        &app,
        "POST",
        &format!("/api/v1/workspaces/{ws}/documents/{doc}/trash"),
        None,
        &owner,
    )
    .await;
    assert!(status.is_success());
    let (_, result) = query(&app, &owner, ws, &cid, json!({"config": {}})).await;
    assert_eq!(result["count"], 0);

    // Tenant isolation over HTTP and at the RLS layer with the app role.
    let other_ws = second_workspace(&admin, owner_id).await;
    let (status, _) = query(&app, &owner, other_ws, &cid, json!({"config": {}})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(
        &app,
        "GET",
        &format!("/api/v1/workspaces/{other_ws}/collections/{cid}/fields"),
        None,
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let app_db = app_pool(&harness).await;
    for table in [
        "collections",
        "collection_items",
        "document_tags",
        "views",
        "collection_views",
    ] {
        let mut tx = app_db.begin().await.unwrap();
        fvoci_server::db::context::set_tenant(&mut tx, other_ws)
            .await
            .unwrap();
        let visible: i64 = sqlx::query_scalar(&format!(
            "SELECT count(*) FROM fvoci.{table} WHERE workspace_id = $1"
        ))
        .bind(ws)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        assert_eq!(visible, 0, "{table} leaks across tenants");
        tx.rollback().await.unwrap();
    }
    let mut tx = app_db.begin().await.unwrap();
    fvoci_server::db::context::set_tenant(&mut tx, other_ws)
        .await
        .unwrap();
    let inserted = sqlx::query("INSERT INTO fvoci.collections (id, workspace_id, kind, name) VALUES ($1, $2, 'document', 'x')")
        .bind(Uuid::now_v7())
        .bind(ws)
        .execute(&mut *tx)
        .await;
    assert!(
        inserted.is_err(),
        "RLS WITH CHECK rejects a foreign workspace row"
    );
    tx.rollback().await.unwrap();
    let forced: bool = sqlx::query_scalar(
        "SELECT bool_and(relforcerowsecurity) FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = 'fvoci' AND relname IN ('document_tags','document_tag_assignments','collections','collection_items',\
         'collection_fields','collection_options','collection_values','collection_choices','collection_people','collection_views','views')",
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert!(forced);
    app_db.close().await;

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn project_saved_views_task_list_clone_and_ics() {
    let harness = TestDb::bootstrap().await;
    let (app, owner, _owner_id, ws) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, ws, "member", "member").await;
    let project = create_project(app.clone(), &owner, ws, "SAVE", "workspace").await;
    let project_id = project["id"].as_str().unwrap().to_string();
    let cid = project_collection(&app, &owner, ws, &project_id).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let size = create_field(
        &app,
        &owner,
        ws,
        &cid,
        json!({"name": "Size", "type": "number"}),
    )
    .await;
    let kind = create_field(
        &app,
        &owner,
        ws,
        &cid,
        json!({"name": "Kind", "type": "select", "options": ["X", "Y"]}),
    )
    .await;
    let mut ids = Vec::new();
    for (index, value) in [3, 1, 3, 2].iter().enumerate() {
        let task = create_task(
            &app,
            &owner,
            ws,
            &project_id,
            json!({"title": format!("s{index}"), "dueDate": format!("2026-11-0{}", index + 1)}),
        )
        .await;
        let item = item_of_task(&app, &owner, ws, &task).await;
        let (status, _) = put_value(
            &app,
            &owner,
            ws,
            &cid,
            &item,
            &size,
            json!({"number": value}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        ids.push(task);
    }

    // Task list: custom filter + field sort, paged with a cursor.
    let list_query = json!({"filters": {"custom": [{"fieldId": size["id"], "operator": "equals", "value": 3}]}, "sort": [{"field": size["id"], "direction": "asc"}]}).to_string();
    let encoded: String = url::form_urlencoded::byte_serialize(list_query.as_bytes()).collect();
    let tasks_path = format!("/api/v1/workspaces/{ws}/projects/{project_id}/tasks");
    let (status, page) = call(
        &app,
        "GET",
        &format!("{tasks_path}?query={encoded}&limit=1"),
        None,
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    let cursor = page["nextCursor"].as_str().unwrap().to_string();
    let (status, page2) = call(
        &app,
        "GET",
        &format!("{tasks_path}?query={encoded}&limit=1&cursor={cursor}"),
        None,
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page2}");
    let mut got = vec![
        page["items"][0]["id"].as_str().unwrap().to_string(),
        page2["items"][0]["id"].as_str().unwrap().to_string(),
    ];
    got.sort();
    let mut want = vec![ids[0].clone(), ids[2].clone()];
    want.sort();
    assert_eq!(got, want);
    assert!(page2["nextCursor"].is_null());
    let sorted_query = json!({"sort": [{"field": size["id"], "direction": "desc"}]}).to_string();
    let encoded_sort: String =
        url::form_urlencoded::byte_serialize(sorted_query.as_bytes()).collect();
    let (_, sorted) = call(
        &app,
        "GET",
        &format!("{tasks_path}?query={encoded_sort}"),
        None,
        &owner,
    )
    .await;
    let titles: Vec<&str> = sorted["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles[3], "s1");
    assert_eq!(titles[2], "s3");
    let bad: String = url::form_urlencoded::byte_serialize(
        json!({"filters": {"custom": [{"fieldId": Uuid::now_v7(), "operator": "empty"}]}})
            .to_string()
            .as_bytes(),
    )
    .collect();
    let (status, _) = call(
        &app,
        "GET",
        &format!("{tasks_path}?query={bad}"),
        None,
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let due: String =
        url::form_urlencoded::byte_serialize(br#"{"filters":{"dueBefore":"2026-11-02"}}"#)
            .collect();
    let (status, due_page) = call(
        &app,
        "GET",
        &format!("{tasks_path}?query={due}"),
        None,
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{due_page}");
    assert_eq!(due_page["items"].as_array().unwrap().len(), 2);

    // Saved views are private, validated and CAS-updated on config.
    let views = format!("/api/v1/workspaces/{ws}/projects/{project_id}/views");
    let config = json!({"filters": {"custom": [{"fieldId": kind["id"], "operator": "equals", "value": kind["options"][0]["id"]}], "openOnly": false}, "sort": []});
    let (status, view) = call(
        &app,
        "POST",
        &views,
        Some(json!({"name": "X only", "type": "list", "config": config})),
        &member.cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{view}");
    assert!(
        view["config"]["filters"].get("openOnly").is_none(),
        "stored canonical"
    );
    let (status, _) = call(&app, "POST", &views, Some(json!({"name": "bad", "type": "list", "config": {"filters": {"labelId": Uuid::now_v7()}}})), &member.cookie).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = call(
        &app,
        "POST",
        &views,
        Some(json!({"name": "bad", "type": "kanban", "config": {}})),
        &member.cookie,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (_, owner_list) = call(&app, "GET", &views, None, &owner).await;
    assert_eq!(owner_list["items"], json!([]));
    let view_path = format!(
        "/api/v1/workspaces/{ws}/views/{}",
        view["id"].as_str().unwrap()
    );
    let (status, _) = call(
        &app,
        "PATCH",
        &view_path,
        Some(json!({"name": "stolen"})),
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(&app, "DELETE", &view_path, None, &owner).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(
        &app,
        "PATCH",
        &view_path,
        Some(json!({"config": {}})),
        &member.cookie,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "config change needs expectedConfig"
    );
    let (status, body) = call(
        &app,
        "PATCH",
        &view_path,
        Some(json!({"config": {}, "expectedConfig": {"filters": {"title": "old"}}})),
        &member.cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let (status, _) = call(&app, "PATCH", &view_path, Some(json!({"name": "renamed", "config": {"sort": [{"field": "due", "direction": "asc"}]}, "expectedConfig": view["config"]})), &member.cookie).await;
    assert_eq!(status, StatusCode::OK);
    let (_, list) = call(&app, "GET", &views, None, &member.cookie).await;
    assert_eq!(list["items"][0]["name"], "renamed");
    assert_eq!(list["items"][0]["config"]["sort"][0]["field"], "due");

    // A saved calendar view adds the project's dated tasks to the ICS feed.
    let (status, token) = call(
        &app,
        "POST",
        &format!("/api/v1/workspaces/{ws}/ics-token"),
        None,
        &member.cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{token}");
    let path = token["url"]
        .as_str()
        .unwrap()
        .strip_prefix("http://localhost")
        .unwrap()
        .to_string();
    let (status, ics) = raw_get(&app, &path).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!ics.contains("SUMMARY:s0"), "member is not assigned");
    let (status, _) = call(
        &app,
        "POST",
        &views,
        Some(json!({"name": "달력", "type": "calendar", "config": {}})),
        &member.cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (_, ics) = raw_get(&app, &path).await;
    assert!(ics.contains("s0") && ics.contains("s3"), "{ics}");

    // Clone copies live fields, options and shared collection views (remapped).
    let views_path = format!("/api/v1/workspaces/{ws}/collections/{cid}/views");
    let (status, _) = call(
        &app,
        "POST",
        &views_path,
        Some(json!({"name": "Big", "type": "table", "visibility": "shared", "config": {"query": {"filters": {"custom": [{"fieldId": kind["id"], "operator": "equals", "value": kind["options"][1]["id"]}]}, "sort": [{"field": size["id"], "direction": "desc"}]}, "groupBy": kind["id"]}})),
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, clone) = call(
        &app,
        "POST",
        &format!("/api/v1/workspaces/{ws}/projects/{project_id}/clone"),
        Some(json!({"key": "SAVC", "name": "Clone"})),
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{clone}");
    let clone_cid = project_collection(&app, &owner, ws, clone["id"].as_str().unwrap()).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(clone_cid, cid);
    let (_, clone_fields) = call(
        &app,
        "GET",
        &format!("/api/v1/workspaces/{ws}/collections/{clone_cid}/fields"),
        None,
        &owner,
    )
    .await;
    let clone_fields = clone_fields["items"].as_array().unwrap().clone();
    assert_eq!(clone_fields.len(), 2);
    let clone_kind = clone_fields.iter().find(|f| f["name"] == "Kind").unwrap();
    assert_ne!(clone_kind["id"], kind["id"]);
    let (_, clone_views) = call(
        &app,
        "GET",
        &format!("/api/v1/workspaces/{ws}/collections/{clone_cid}/views"),
        None,
        &owner,
    )
    .await;
    let copied = &clone_views["items"][0];
    assert_eq!(copied["name"], "Big");
    assert_eq!(copied["config"]["groupBy"], clone_kind["id"]);
    assert_eq!(
        copied["config"]["query"]["filters"]["custom"][0]["value"],
        clone_kind["options"][1]["id"]
    );
    let (status, _) = query(
        &app,
        &owner,
        ws,
        &clone_cid,
        json!({"config": copied["config"]}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "remapped config is valid in the clone"
    );

    admin.close().await;
    harness.cleanup().await;
}

async fn seed_project_with_tasks(
    admin: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
    key: &str,
    project_deleted: bool,
) -> (Uuid, Uuid, Vec<Uuid>) {
    let project_id = Uuid::now_v7();
    let workflow_id = Uuid::now_v7();
    let status_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO fvoci.projects (id, workspace_id, key, name, visibility, created_by, deleted_at)
         VALUES ($1, $2, $3, $3, 'workspace', $4, CASE WHEN $5 THEN now() END)",
    )
    .bind(project_id)
    .bind(workspace_id)
    .bind(key)
    .bind(user_id)
    .bind(project_deleted)
    .execute(admin)
    .await
    .expect("project");
    sqlx::query("INSERT INTO fvoci.workflows (id, workspace_id, project_id) VALUES ($1, $2, $3)")
        .bind(workflow_id)
        .bind(workspace_id)
        .bind(project_id)
        .execute(admin)
        .await
        .expect("workflow");
    sqlx::query(
        "INSERT INTO fvoci.statuses (id, workspace_id, project_id, workflow_id, name, category, sort_key)
         VALUES ($1, $2, $3, $4, 'Todo', 'todo', 'V')",
    )
    .bind(status_id)
    .bind(workspace_id)
    .bind(project_id)
    .bind(workflow_id)
    .execute(admin)
    .await
    .expect("status");
    let mut tasks = Vec::new();
    // live, archived and soft-deleted tasks all get an item, as in the source backfill
    for (number, archived, deleted) in [(1, false, false), (2, true, false), (3, false, true)] {
        let task_id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO fvoci.tasks (id, workspace_id, project_id, number, title, status_id, created_by,
                content_json, archived_at, deleted_at)
             VALUES ($1, $2, $3, $4, 'Seeded', $5, $6, '{\"type\":\"doc\",\"content\":[]}'::jsonb,
                CASE WHEN $7 THEN now() END, CASE WHEN $8 THEN now() END)",
        )
        .bind(task_id)
        .bind(workspace_id)
        .bind(project_id)
        .bind(number)
        .bind(status_id)
        .bind(user_id)
        .bind(archived)
        .bind(deleted)
        .execute(admin)
        .await
        .expect("task");
        tasks.push(task_id);
    }
    (project_id, status_id, tasks)
}

/// Review B1: projects/tasks are FORCE RLS with a tenant-only policy, so the
/// 028 backfill must also work when the schema owner is not a superuser.
#[tokio::test]
async fn upgrade_to_028_backfills_as_a_non_superuser_schema_owner() {
    let db = TestDb::bootstrap_through(27).await;
    let admin = admin_pool(&db).await;
    let user_id = Uuid::now_v7();
    let workspace_id = Uuid::now_v7();
    sqlx::query("INSERT INTO fvoci.users (id, email, given_name) VALUES ($1, $2, 'Owner')")
        .bind(user_id)
        .bind(format!("owner-{}@example.com", user_id.simple()))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query("INSERT INTO fvoci.workspaces (id, slug, name) VALUES ($1, $2, 'ws')")
        .bind(workspace_id)
        .bind(format!("u{}", &user_id.simple().to_string()[..16]))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO fvoci.memberships (workspace_id, user_id, role) VALUES ($1, $2, 'owner')",
    )
    .bind(workspace_id)
    .bind(user_id)
    .execute(&admin)
    .await
    .unwrap();
    let (live_project, live_status, live_tasks) =
        seed_project_with_tasks(&admin, workspace_id, user_id, "UPA", false).await;
    let (deleted_project, _, deleted_tasks) =
        seed_project_with_tasks(&admin, workspace_id, user_id, "UPB", true).await;

    // Hand every schema object to a NOSUPERUSER NOBYPASSRLS owner and migrate as it.
    let owner = format!("fvoci_owner_{}", Uuid::now_v7().simple());
    sqlx::query(&format!(
        "CREATE ROLE \"{owner}\" LOGIN PASSWORD 'owner-pass' NOSUPERUSER NOBYPASSRLS"
    ))
    .execute(&admin)
    .await
    .unwrap();
    sqlx::query(&format!(
        r#"DO $$
        DECLARE r record;
        BEGIN
            EXECUTE format('ALTER SCHEMA fvoci OWNER TO %I', '{owner}');
            EXECUTE format('GRANT CREATE ON SCHEMA public TO %I', '{owner}');
            FOR r IN SELECT c.oid::regclass AS rel, c.relkind FROM pg_class c
                     JOIN pg_namespace n ON n.oid = c.relnamespace
                     WHERE n.nspname = 'fvoci' AND c.relkind IN ('r', 'p', 'v', 'm', 'f')
            LOOP
                EXECUTE format('ALTER TABLE %s OWNER TO %I', r.rel, '{owner}');
            END LOOP;
            FOR r IN SELECT c.oid::regclass AS rel FROM pg_class c
                     JOIN pg_namespace n ON n.oid = c.relnamespace
                     WHERE n.nspname = 'fvoci' AND c.relkind = 'S'
                       AND NOT EXISTS (SELECT 1 FROM pg_depend d WHERE d.objid = c.oid AND d.deptype IN ('a', 'i'))
            LOOP
                EXECUTE format('ALTER SEQUENCE %s OWNER TO %I', r.rel, '{owner}');
            END LOOP;
            FOR r IN SELECT p.oid::regprocedure AS fn FROM pg_proc p
                     JOIN pg_namespace n ON n.oid = p.pronamespace
                     WHERE n.nspname IN ('fvoci', 'public')
                       AND NOT EXISTS (SELECT 1 FROM pg_depend d WHERE d.objid = p.oid AND d.deptype = 'e')
            LOOP
                EXECUTE format('ALTER ROUTINE %s OWNER TO %I', r.fn, '{owner}');
            END LOOP;
            FOR r IN SELECT t.oid::regtype AS ty FROM pg_type t
                     JOIN pg_namespace n ON n.oid = t.typnamespace
                     WHERE n.nspname = 'fvoci' AND t.typtype IN ('e', 'd', 'c')
                       AND NOT EXISTS (SELECT 1 FROM pg_class c WHERE c.reltype = t.oid)
            LOOP
                EXECUTE format('ALTER TYPE %s OWNER TO %I', r.ty, '{owner}');
            END LOOP;
        END $$"#
    ))
    .execute(&admin)
    .await
    .expect("reassign schema objects");
    let mut owner_url = url::Url::parse(&db.admin_url).unwrap();
    owner_url.set_username(&owner).unwrap();
    owner_url.set_password(Some("owner-pass")).unwrap();
    fvoci_server::db::migrate::run_migrations_through(owner_url.as_str(), 28)
        .await
        .expect("migrate 28 as a non-superuser owner");

    // Exactly one task collection per project and one item per task.
    for project in [live_project, deleted_project] {
        let collections: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM fvoci.collections WHERE project_id = $1 AND kind = 'task'",
        )
        .bind(project)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(collections, 1, "project {project}");
    }
    for task in live_tasks.iter().chain(deleted_tasks.iter()) {
        let items: i64 =
            sqlx::query_scalar("SELECT count(*) FROM fvoci.collection_items WHERE task_id = $1")
                .bind(task)
                .fetch_one(&admin)
                .await
                .unwrap();
        assert_eq!(items, 1, "task {task}");
    }
    // FORCE RLS is back on both tables after the backfill.
    let forced: Vec<(String, bool)> = sqlx::query_as(
        "SELECT relname::text, relforcerowsecurity FROM pg_class
         WHERE oid IN ('fvoci.projects'::regclass, 'fvoci.tasks'::regclass) ORDER BY relname",
    )
    .fetch_all(&admin)
    .await
    .unwrap();
    assert_eq!(
        forced,
        vec![("projects".to_string(), true), ("tasks".to_string(), true)]
    );
    // A task created after the upgrade attaches to the backfilled collection.
    let new_task = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO fvoci.tasks (id, workspace_id, project_id, number, title, status_id, created_by, content_json)
         VALUES ($1, $2, $3, 4, 'After upgrade', $4, $5, '{\"type\":\"doc\",\"content\":[]}'::jsonb)",
    )
    .bind(new_task)
    .bind(workspace_id)
    .bind(live_project)
    .bind(live_status)
    .bind(user_id)
    .execute(&admin)
    .await
    .expect("task insert after upgrade");
    let attached: i64 =
        sqlx::query_scalar("SELECT count(*) FROM fvoci.collection_items WHERE task_id = $1")
            .bind(new_task)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(attached, 1);

    admin.close().await;
    let mut server_url = url::Url::parse(&db.admin_url).unwrap();
    server_url.set_path("/postgres");
    db.cleanup().await;
    let server = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(server_url.as_str())
        .await
        .unwrap();
    sqlx::query(&format!("DROP ROLE IF EXISTS \"{owner}\""))
        .execute(&server)
        .await
        .expect("drop owner role");
    server.close().await;
}

/// Titles `dueBefore` 2026-01-01 selects on one task query path.
async fn due_before_titles(
    app: &axum::Router,
    cookie: &str,
    ws: Uuid,
    project: &str,
    collection: &str,
    path: &str,
) -> Vec<String> {
    let query =
        r#"{"filters":{"dueBefore":"2026-01-01"},"sort":[{"field":"title","direction":"asc"}]}"#;
    let encoded: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
    let (status, body) = match path {
        "project" => {
            let path = format!("/api/v1/workspaces/{ws}/projects/{project}/tasks?query={encoded}");
            call(app, "GET", &path, None, cookie).await
        }
        "workspace" => {
            let path = format!("/api/v1/workspaces/{ws}/tasks?query={encoded}");
            call(app, "GET", &path, None, cookie).await
        }
        "layout" => {
            let path = format!(
                "/api/v1/workspaces/{ws}/projects/{project}/task-layout?year=2026&month=1&query={encoded}"
            );
            call(app, "GET", &path, None, cookie).await
        }
        _ => {
            let config: Value = serde_json::from_str(query).unwrap();
            let body = json!({"config": {"query": config}, "limit": 100});
            self::query(app, cookie, ws, collection, body).await
        }
    };
    assert_eq!(status, StatusCode::OK, "{path}: {body}");
    let mut titles: Vec<String> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["title"].as_str().unwrap().to_string())
        .collect();
    titles.sort();
    titles
}

#[tokio::test]
async fn due_before_uses_each_actor_time_zone_on_every_task_query_path() {
    let harness = TestDb::bootstrap().await;
    let (app, owner, owner_id, ws) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, ws, "member", "west").await;
    let project = create_project(app.clone(), &owner, ws, "TZ", "workspace").await;
    let project_id = project["id"].as_str().unwrap().to_string();
    let collection = project_collection(&app, &owner, ws, &project_id).await;
    let cid = collection["id"].as_str().unwrap().to_string();
    // Evening UTC on 2026-01-01 is 2026-01-02 in Seoul (+09:00); early UTC on
    // 2026-01-02 is still 2026-01-01 in Los Angeles (-08:00). Date-only dues
    // are calendar dates in every zone.
    for (title, due_date, due_at) in [
        ("at-evening", None, Some("2026-01-01T20:00:00Z")),
        ("at-early-next", None, Some("2026-01-02T03:00:00Z")),
        ("date-on", Some("2026-01-01"), None),
        ("date-after", Some("2026-01-02"), None),
    ] {
        let task = create_task(&app, &owner, ws, &project_id, json!({"title": title})).await;
        sqlx::query(
            "UPDATE fvoci.tasks SET start_date = NULL, due_date = $2::date, due_at = $3::timestamptz WHERE id = $1::uuid",
        )
        .bind(&task)
        .bind(due_date)
        .bind(due_at)
        .execute(&admin)
        .await
        .unwrap();
    }
    for (user, zone) in [
        (owner_id, "Asia/Seoul"),
        (member.user_id, "America/Los_Angeles"),
    ] {
        sqlx::query("UPDATE fvoci.users SET timezone = $2 WHERE id = $1")
            .bind(user)
            .bind(zone)
            .execute(&admin)
            .await
            .unwrap();
    }

    for (cookie, expected) in [
        (owner.as_str(), vec!["date-on"]),
        (
            member.cookie.as_str(),
            vec!["at-early-next", "at-evening", "date-on"],
        ),
    ] {
        for path in ["project", "workspace", "layout", "collection"] {
            let titles = due_before_titles(&app, cookie, ws, &project_id, &cid, path).await;
            assert_eq!(titles, expected, "{path}");
        }
    }

    sqlx::query("UPDATE fvoci.users SET timezone = 'UTC' WHERE id = $1")
        .bind(owner_id)
        .execute(&admin)
        .await
        .unwrap();
    for path in ["project", "workspace", "layout", "collection"] {
        let titles = due_before_titles(&app, &owner, ws, &project_id, &cid, path).await;
        assert_eq!(titles, vec!["at-evening", "date-on"], "{path}");
    }

    // An unknown stored zone reads as UTC on every path, collection included,
    // instead of failing `AT TIME ZONE` on the first timestamp due.
    sqlx::query("UPDATE fvoci.users SET timezone = 'Mars/Olympus_Mons' WHERE id = $1")
        .bind(owner_id)
        .execute(&admin)
        .await
        .unwrap();
    for path in ["collection", "project", "workspace", "layout"] {
        let titles = due_before_titles(&app, &owner, ws, &project_id, &cid, path).await;
        assert_eq!(titles, vec!["at-evening", "date-on"], "{path}");
    }
    // A calendar window keeps its own zone over the actor's fallback: Seoul
    // puts the 20:00Z due on 2026-01-02, which UTC would not.
    let (status, body) = query(
        &app,
        &owner,
        ws,
        &cid,
        json!({
            "config": {"dateBy": "due"},
            "window": {"from": "2026-01-02", "to": "2026-01-03", "timeZone": "Asia/Seoul"},
            "limit": 100,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let mut titles: Vec<&str> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["title"].as_str().unwrap())
        .collect();
    titles.sort();
    assert_eq!(titles, vec!["at-early-next", "at-evening", "date-after"]);

    admin.close().await;
    harness.cleanup().await;
}

/// Records statements the sqlx client finishes (`sqlx::query` events, one per
/// executed statement) on the thread where it is active. A thread-local
/// `set_default` dispatcher can miss events when other test threads cache the
/// callsite's interest first, so one global router feeds per-thread counters.
#[derive(Clone, Default)]
struct StatementCounter(std::rc::Rc<std::cell::RefCell<Vec<String>>>);

thread_local! {
    static ACTIVE_COUNTER: std::cell::RefCell<Option<StatementCounter>> =
        const { std::cell::RefCell::new(None) };
}

struct ActiveCounterGuard;

impl Drop for ActiveCounterGuard {
    fn drop(&mut self) {
        ACTIVE_COUNTER.with(|active| active.borrow_mut().take());
    }
}

impl StatementCounter {
    /// Installs the global router once and makes this counter the sink for
    /// statements finished on the current (`#[tokio::test]` runtime) thread.
    fn activate(&self) -> ActiveCounterGuard {
        use tracing_subscriber::layer::SubscriberExt;
        static ROUTER: std::sync::Once = std::sync::Once::new();
        ROUTER.call_once(|| {
            tracing::subscriber::set_global_default(
                tracing_subscriber::registry().with(StatementRouter),
            )
            .expect("statement router is the only global subscriber");
        });
        ACTIVE_COUNTER.with(|active| *active.borrow_mut() = Some(self.clone()));
        ActiveCounterGuard
    }

    fn take_statements(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.borrow_mut())
    }

    fn take(&self) -> usize {
        self.take_statements().len()
    }
}

struct SummaryVisitor(String);

impl tracing::field::Visit for SummaryVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if matches!(field.name(), "summary" | "db.statement") {
            self.0.push_str(&format!("{value:?} "));
        }
    }
}

struct StatementRouter;

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for StatementRouter {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        if event.metadata().target() != "sqlx::query" {
            return;
        }
        ACTIVE_COUNTER.with(|active| {
            if let Some(counter) = active.borrow().as_ref() {
                let mut visitor = SummaryVisitor(String::new());
                event.record(&mut visitor);
                counter.0.borrow_mut().push(visitor.0);
            }
        });
    }
}

/// Statements finished since the last take that belong to the request. The
/// process-wide time zone name cache (db::dashboard) fills once, on whichever
/// query comes first; it is not part of the request's own work. Neither is the
/// app pool's acquire check (db::pool): it runs only when an idle connection is
/// reused, not on a newly opened one, so how often it appears depends on the
/// pool's state, not on the request.
fn request_statements(counter: &StatementCounter) -> Vec<String> {
    use fvoci_server::db::pool::ACQUIRE_CHECK_SQL;

    counter
        .take_statements()
        .into_iter()
        .filter(|sql| !sql.contains("pg_catalog.pg_timezone_names"))
        .filter(|sql| !sql.contains(ACQUIRE_CHECK_SQL))
        .collect()
}

/// Runs the guest's wiki collection query, checks every row's canEdit against
/// the single-document lookup and returns (rows, statements the query issued).
async fn guest_query_checked(
    app: &axum::Router,
    app_db: &PgPool,
    guest: &project_harness::TestUser,
    ws: Uuid,
    cid: &str,
    counter: &StatementCounter,
) -> (usize, Vec<String>) {
    use fvoci_server::db::context::set_tenant;
    use fvoci_server::db::documents::document_permission;
    use fvoci_server::projects::ProjectPermission;

    counter.take();
    let (status, result) = query(
        app,
        &guest.cookie,
        ws,
        cid,
        json!({"config": {"query": {"sort": [{"field": "title", "direction": "asc"}]}}, "limit": 100}),
    )
    .await;
    let statements = request_statements(counter);
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["nextCursor"], Value::Null, "{result}");
    let rows = result["items"].as_array().unwrap();
    let mut tx = app_db.begin().await.unwrap();
    set_tenant(&mut tx, ws).await.unwrap();
    for row in rows {
        let doc: Uuid = row["documentId"].as_str().unwrap().parse().unwrap();
        let single = document_permission(&mut tx, ws, guest.user_id, doc, true)
            .await
            .unwrap();
        assert_eq!(
            row["canEdit"],
            single >= ProjectPermission::Edit,
            "{doc} {}",
            row["title"]
        );
    }
    tx.rollback().await.unwrap();
    (rows.len(), statements)
}

#[tokio::test]
async fn wiki_collection_can_edit_uses_one_set_based_permission_lookup() {
    use fvoci_server::db::context::set_tenant;
    use fvoci_server::db::documents::{document_permission, document_permissions};
    use fvoci_server::projects::ProjectPermission;

    let harness = TestDb::bootstrap().await;
    let (app, owner, owner_id, ws) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let app_db = app_pool(&harness).await;
    let member = add_workspace_user(&admin, ws, "member", "bulk-member").await;
    let guest = add_workspace_user(&admin, ws, "guest", "bulk-guest").await;
    let (status, group) = call(
        &app,
        "POST",
        &format!("/api/v1/workspaces/{ws}/groups"),
        Some(json!({"name": "bulk-grants"})),
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{group}");
    let group_id = group["id"].as_str().unwrap().to_string();
    let (status, _) = call(
        &app,
        "POST",
        &format!("/api/v1/workspaces/{ws}/groups/{group_id}/members"),
        Some(json!({"userId": guest.user_id.to_string()})),
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, collection) = call(
        &app,
        "POST",
        &format!("/api/v1/workspaces/{ws}/collections"),
        Some(json!({"name": "bulk", "kind": "document", "projectId": null})),
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{collection}");
    let cid = collection["id"].as_str().unwrap().to_string();
    let items = format!("/api/v1/workspaces/{ws}/collections/{cid}/items");

    let project_id = Uuid::now_v7();
    project_harness::insert_minimal_project(&admin, ws, project_id, "BULK", owner_id, "workspace")
        .await;
    let project_doc = Uuid::now_v7();
    project_harness::insert_project_document(&admin, ws, project_id, project_doc, owner_id, 1)
        .await;
    // Warm-up: the guest's first request slides the session expiry (one
    // UPDATE) so the counted queries below differ only by row count.
    let (status, _) = call(
        &app,
        "GET",
        &format!("/api/v1/workspaces/{ws}/collections"),
        None,
        &guest.cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let counter = StatementCounter::default();
    let _guard = counter.activate();
    let mut small = None;

    // 28 wiki rows: i % 4 == 0 workspace base only, 1 group member (edit),
    // 2 group viewer, 3 archived with an alternating member/lead/viewer grant.
    let mut docs: Vec<Uuid> = Vec::new();
    let mut archived: Vec<Uuid> = Vec::new();
    for i in 0..28 {
        let doc = create_wiki_doc(&app, &owner, ws, &format!("bulk {i:02}")).await;
        let grant = match i % 4 {
            0 => None,
            1 => Some("member"),
            2 => Some("viewer"),
            _ => Some(["member", "lead", "viewer"][(i / 4) % 3]),
        };
        if let Some(role) = grant {
            let (status, body) = call(
                &app,
                "POST",
                &format!("/api/v1/workspaces/{ws}/documents/{doc}/groups"),
                Some(json!({"groupId": group_id, "role": role})),
                &owner,
            )
            .await;
            assert_eq!(status, StatusCode::CREATED, "{body}");
        }
        let (status, body) = call(
            &app,
            "POST",
            &items,
            Some(json!({"documentId": doc})),
            &owner,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        let doc: Uuid = doc.parse().unwrap();
        if i % 4 == 3 {
            archived.push(doc);
        }
        docs.push(doc);
        if i == 3 {
            // Baseline: the guest sees 3 rows (i = 1, 2, 3) before the rest.
            small = Some(guest_query_checked(&app, &app_db, &guest, ws, &cid, &counter).await);
        }
    }
    sqlx::query("UPDATE fvoci.documents SET status = 'archived' WHERE id = ANY($1)")
        .bind(&archived)
        .execute(&admin)
        .await
        .unwrap();
    // Ids outside the collection that the bulk lookup must still rank as the
    // single lookup does: a trashed granted wiki doc, a project doc, a missing id.
    let trashed: Uuid = create_wiki_doc(&app, &owner, ws, "bulk trashed")
        .await
        .parse()
        .unwrap();
    let (status, body) = call(
        &app,
        "POST",
        &format!("/api/v1/workspaces/{ws}/documents/{trashed}/groups"),
        Some(json!({"groupId": group_id, "role": "member"})),
        &owner,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    sqlx::query("UPDATE fvoci.documents SET deleted_at = now() WHERE id = $1")
        .bind(trashed)
        .execute(&admin)
        .await
        .unwrap();
    let mut probe = docs.clone();
    probe.extend([trashed, project_doc, Uuid::now_v7()]);

    // (a) Direct: the bulk result equals the single lookup for every id, both
    // with and without require_live, for guest (grants) and member (base).
    for (user_id, label) in [(guest.user_id, "guest"), (member.user_id, "member")] {
        for require_live in [true, false] {
            let mut tx = app_db.begin().await.unwrap();
            set_tenant(&mut tx, ws).await.unwrap();
            counter.take();
            let bulk = document_permissions(&mut tx, ws, user_id, &probe, require_live)
                .await
                .unwrap();
            assert_eq!(
                counter.take(),
                2,
                "{label}: membership read + one set-based statement for {} ids",
                probe.len()
            );
            assert_eq!(bulk.len(), probe.len());
            for id in &probe {
                let single = document_permission(&mut tx, ws, user_id, *id, require_live)
                    .await
                    .unwrap();
                assert_eq!(bulk[id], single, "{label} {id} live={require_live}");
            }
            let per_doc = counter.take();
            assert!(
                per_doc >= 2 * probe.len(),
                "the counter sees the per-document path ({per_doc} statements)"
            );
            // Statement count is independent of the id count.
            counter.take();
            document_permissions(&mut tx, ws, user_id, &probe[..3], require_live)
                .await
                .unwrap();
            assert_eq!(counter.take(), 2, "{label}: 3 ids");
            tx.rollback().await.unwrap();
        }
    }
    let mut tx = app_db.begin().await.unwrap();
    set_tenant(&mut tx, ws).await.unwrap();
    let guest_levels = document_permissions(&mut tx, ws, guest.user_id, &probe, true)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    for (i, doc) in docs.iter().enumerate() {
        let expected = match i % 4 {
            0 => ProjectPermission::None,
            1 => ProjectPermission::Edit,
            2 => ProjectPermission::View,
            _ => [
                ProjectPermission::Edit,
                ProjectPermission::Manage,
                ProjectPermission::View,
            ][(i / 4) % 3],
        };
        assert_eq!(guest_levels[doc], expected, "guest level of row {i}");
    }
    assert_eq!(guest_levels[&trashed], ProjectPermission::None);
    assert_eq!(guest_levels[&project_doc], ProjectPermission::None);

    // (b) HTTP: every visible row's canEdit equals the single-document level,
    // and the whole guest query issues as many statements for 21 rows as for
    // 3 (the per-row permission step no longer scales with row count).
    let (small_rows, small_statements) = small.unwrap();
    assert_eq!(small_rows, 3);
    let (rows, statements) = guest_query_checked(&app, &app_db, &guest, ws, &cid, &counter).await;
    assert_eq!(rows, 21, "guest sees the granted docs only");
    assert_eq!(
        statements, small_statements,
        "guest query statements: 3 rows vs 21 rows"
    );
    let permission_step = statements
        .iter()
        .filter(|sql| sql.contains("SELECT d.id, d.project_id, d.deleted_at"))
        .count();
    assert_eq!(permission_step, 1, "{statements:#?}");
    let (status, result) = query(
        &app,
        &member.cookie,
        ws,
        &cid,
        json!({"config": {}, "limit": 100}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let rows = result["items"].as_array().unwrap();
    assert_eq!(rows.len(), 28);
    assert!(
        rows.iter().all(|row| row["canEdit"] == true),
        "member base >= edit"
    );

    drop(_guard);
    app_db.close().await;
    admin.close().await;
    harness.cleanup().await;
}

async fn create_group(
    app: &axum::Router,
    owner: &str,
    ws: Uuid,
    name: &str,
    users: &[Uuid],
) -> Uuid {
    let (status, group) = call(
        app,
        "POST",
        &format!("/api/v1/workspaces/{ws}/groups"),
        Some(json!({"name": name})),
        owner,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{group}");
    let group_id: Uuid = group["id"].as_str().unwrap().parse().unwrap();
    for user in users {
        let (status, body) = call(
            app,
            "POST",
            &format!("/api/v1/workspaces/{ws}/groups/{group_id}/members"),
            Some(json!({"userId": user.to_string()})),
            owner,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }
    group_id
}

async fn grant_project_user(admin: &PgPool, ws: Uuid, project: Uuid, user: Uuid, role: &str) {
    sqlx::query(
        "INSERT INTO fvoci.project_members (id, workspace_id, project_id, user_id, role) \
         VALUES (gen_random_uuid(), $1, $2, $3, $4)",
    )
    .bind(ws)
    .bind(project)
    .bind(user)
    .bind(role)
    .execute(admin)
    .await
    .expect("grant project user");
}

async fn grant_project_group(admin: &PgPool, ws: Uuid, project: Uuid, group: Uuid, role: &str) {
    sqlx::query(
        "INSERT INTO fvoci.project_members (id, workspace_id, project_id, group_id, role) \
         VALUES (gen_random_uuid(), $1, $2, $3, $4)",
    )
    .bind(ws)
    .bind(project)
    .bind(group)
    .bind(role)
    .execute(admin)
    .await
    .expect("grant project group");
}

/// Project ids the actor's workspace ACL (`load_search_acl`) yields, through
/// two of its consumers: the dashboard project list and the project ids of the
/// task collections in the collection list.
async fn acl_project_ids(
    app: &axum::Router,
    cookie: &str,
    ws: Uuid,
) -> (
    std::collections::BTreeSet<Uuid>,
    std::collections::BTreeSet<Uuid>,
) {
    let (status, dashboard) = call(app, "GET", "/api/v1/me/dashboard", None, cookie).await;
    assert_eq!(status, StatusCode::OK, "{dashboard}");
    let from_dashboard = dashboard["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_str().unwrap().parse().unwrap())
        .collect();
    let (status, list) = call(
        app,
        "GET",
        &format!("/api/v1/workspaces/{ws}/collections"),
        None,
        cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let from_collections = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["kind"] == "task")
        .map(|c| c["projectId"].as_str().unwrap().parse().unwrap())
        .collect();
    (from_dashboard, from_collections)
}

/// One project of the ACL matrix: visibility, direct rows, a group grant and
/// its archive/trash state.
struct MatrixProject {
    key: &'static str,
    visibility: &'static str,
    users: Vec<(Uuid, &'static str)>,
    group_role: Option<&'static str>,
    archived: bool,
    trashed: bool,
}

impl MatrixProject {
    fn new(key: &'static str, visibility: &'static str) -> Self {
        Self {
            key,
            visibility,
            users: Vec::new(),
            group_role: None,
            archived: false,
            trashed: false,
        }
    }

    fn user(mut self, user: Uuid, role: &'static str) -> Self {
        self.users.push((user, role));
        self
    }

    fn group(mut self, role: &'static str) -> Self {
        self.group_role = Some(role);
        self
    }

    fn archived(mut self) -> Self {
        self.archived = true;
        self
    }

    fn trashed(mut self) -> Self {
        self.trashed = true;
        self
    }
}

/// The set-based workspace ACL grants exactly the projects the single-project
/// check grants (GET project: `load_live_project` + `project_permission`, the
/// per-project rule the ACL used to loop over), for owner, admin, member and
/// guest actors with direct rows, group grants, private, archived and trashed
/// projects.
#[tokio::test]
async fn search_acl_matches_the_single_project_check() {
    let harness = TestDb::bootstrap().await;
    let (app, owner, owner_id, ws) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let ws_admin = add_workspace_user(&admin, ws, "admin", "acl-admin").await;
    let member = add_workspace_user(&admin, ws, "member", "acl-member").await;
    let member_group = add_workspace_user(&admin, ws, "member", "acl-member-group").await;
    let guest_direct = add_workspace_user(&admin, ws, "guest", "acl-guest-direct").await;
    let guest_group = add_workspace_user(&admin, ws, "guest", "acl-guest-group").await;
    let group = create_group(
        &app,
        &owner,
        ws,
        "acl-matrix",
        &[member_group.user_id, guest_group.user_id],
    )
    .await;

    let matrix = [
        MatrixProject::new("WSV", "workspace"),
        MatrixProject::new("WSD", "workspace").user(guest_direct.user_id, "viewer"),
        MatrixProject::new("PRV", "private").user(owner_id, "lead"),
        MatrixProject::new("PRD", "private")
            .user(member.user_id, "viewer")
            .user(guest_direct.user_id, "member"),
        MatrixProject::new("PRG", "private").group("viewer"),
        MatrixProject::new("ARC", "private")
            .user(member.user_id, "lead")
            .group("member")
            .archived(),
        MatrixProject::new("ARW", "workspace").archived(),
        MatrixProject::new("TRS", "workspace")
            .user(owner_id, "lead")
            .user(member.user_id, "viewer")
            .user(guest_direct.user_id, "viewer")
            .group("viewer")
            .trashed(),
    ];
    let mut key_of = std::collections::BTreeMap::new();
    for project in &matrix {
        let id = Uuid::now_v7();
        project_harness::insert_minimal_project(
            &admin,
            ws,
            id,
            project.key,
            owner_id,
            project.visibility,
        )
        .await;
        for (user, role) in &project.users {
            grant_project_user(&admin, ws, id, *user, role).await;
        }
        if let Some(role) = project.group_role {
            grant_project_group(&admin, ws, id, group, role).await;
        }
        if project.archived {
            sqlx::query("UPDATE fvoci.projects SET status = 'archived' WHERE id = $1")
                .bind(id)
                .execute(&admin)
                .await
                .unwrap();
        }
        if project.trashed {
            // The project's task collection stays live, so the collection
            // list shows it only if the ACL wrongly admits a trashed project.
            sqlx::query("UPDATE fvoci.projects SET deleted_at = now() WHERE id = $1")
                .bind(id)
                .execute(&admin)
                .await
                .unwrap();
        }
        key_of.insert(id, project.key);
    }

    let actors: [(&str, &str, &[&str]); 6] = [
        ("owner", owner.as_str(), &["ARW", "PRV", "WSD", "WSV"]),
        ("admin", ws_admin.cookie.as_str(), &["ARW", "WSD", "WSV"]),
        (
            "member",
            member.cookie.as_str(),
            &["ARC", "ARW", "PRD", "WSD", "WSV"],
        ),
        (
            "member in group",
            member_group.cookie.as_str(),
            &["ARC", "ARW", "PRG", "WSD", "WSV"],
        ),
        (
            "guest with rows",
            guest_direct.cookie.as_str(),
            &["PRD", "WSD"],
        ),
        (
            "guest in group",
            guest_group.cookie.as_str(),
            &["ARC", "PRG"],
        ),
    ];
    for (label, cookie, expected) in actors {
        let mut single = std::collections::BTreeSet::new();
        for id in key_of.keys() {
            let (status, body) = call(
                &app,
                "GET",
                &format!("/api/v1/workspaces/{ws}/projects/{id}"),
                None,
                cookie,
            )
            .await;
            match status {
                StatusCode::OK => {
                    single.insert(*id);
                }
                StatusCode::NOT_FOUND => {}
                other => panic!("{label} GET project {}: {other} {body}", key_of[id]),
            }
        }
        let keys = |ids: &std::collections::BTreeSet<Uuid>| {
            let mut keys: Vec<&str> = ids.iter().map(|id| key_of[id]).collect();
            keys.sort();
            keys
        };
        assert_eq!(keys(&single), expected, "{label}: single-project check");
        let (dashboard, collections) = acl_project_ids(&app, cookie, ws).await;
        assert_eq!(dashboard, single, "{label}: dashboard projects");
        assert_eq!(collections, single, "{label}: task collections");
    }

    admin.close().await;
    harness.cleanup().await;
}

/// Adds projects `range` with mixed visibility and grants: even numbers are
/// workspace-visible, odd ones private; `member` gets a direct viewer row when
/// `i % 3 == 1` and `group` a viewer grant when `i % 4 == 1`, so both see at
/// least one project (P001) in every range that starts at 0.
async fn seed_mixed_projects(
    admin: &PgPool,
    ws: Uuid,
    owner_id: Uuid,
    member: Uuid,
    group: Uuid,
    range: std::ops::Range<usize>,
) {
    for i in range {
        let id = Uuid::now_v7();
        let visibility = if i % 2 == 0 { "workspace" } else { "private" };
        project_harness::insert_minimal_project(
            admin,
            ws,
            id,
            &format!("P{i:03}"),
            owner_id,
            visibility,
        )
        .await;
        if i % 3 == 1 {
            grant_project_user(admin, ws, id, member, "viewer").await;
        }
        if i % 4 == 1 {
            grant_project_group(admin, ws, id, group, "viewer").await;
        }
    }
}

/// Statements each ACL-backed page issues, by page label.
async fn acl_page_statements(
    app: &axum::Router,
    counter: &StatementCounter,
    member: &project_harness::TestUser,
    guest: &project_harness::TestUser,
    ws: Uuid,
    cid: &str,
) -> Vec<(&'static str, Vec<String>)> {
    let pages: [(&str, &str, &str, Option<Value>, &str); 4] = [
        (
            "member dashboard",
            "GET",
            "/api/v1/me/dashboard",
            None,
            &member.cookie,
        ),
        (
            "guest dashboard",
            "GET",
            "/api/v1/me/dashboard",
            None,
            &guest.cookie,
        ),
        (
            "member collection list",
            "GET",
            &format!("/api/v1/workspaces/{ws}/collections"),
            None,
            &member.cookie,
        ),
        (
            "member collection query",
            "POST",
            &format!("/api/v1/workspaces/{ws}/collections/{cid}/query"),
            Some(json!({"config": {}, "limit": 50})),
            &member.cookie,
        ),
    ];
    let mut statements = Vec::new();
    for (label, method, path, body, cookie) in pages {
        counter.take();
        let (status, result) = call(app, method, path, body, cookie).await;
        assert_eq!(status, StatusCode::OK, "{label}: {result}");
        statements.push((label, request_statements(counter)));
    }
    statements
}

/// `load_search_acl` reads the actor's visible projects in one statement, so
/// the pages built on it issue as many statements with 30 projects as with 3
/// (it used to run two statements per project).
#[tokio::test]
async fn search_acl_statements_do_not_grow_with_project_count() {
    let harness = TestDb::bootstrap().await;
    let (app, owner, owner_id, ws) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, ws, "member", "acl-count-member").await;
    let guest = add_workspace_user(&admin, ws, "guest", "acl-count-guest").await;
    let group = create_group(&app, &owner, ws, "acl-count", &[guest.user_id]).await;
    seed_mixed_projects(&admin, ws, owner_id, member.user_id, group, 0..3).await;
    let base: Uuid = sqlx::query_scalar("SELECT id FROM fvoci.projects WHERE key = 'P000'")
        .fetch_one(&admin)
        .await
        .unwrap();
    let cid = project_collection(&app, &member.cookie, ws, &base.to_string()).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    // Warm-up: each actor's first request slides the session expiry (one
    // UPDATE), so the counted requests differ only by the project count.
    let counter = StatementCounter::default();
    acl_page_statements(&app, &counter, &member, &guest, ws, &cid).await;
    let _guard = counter.activate();
    let small = acl_page_statements(&app, &counter, &member, &guest, ws, &cid).await;
    seed_mixed_projects(&admin, ws, owner_id, member.user_id, group, 3..30).await;
    let large = acl_page_statements(&app, &counter, &member, &guest, ws, &cid).await;
    for ((label, at_3), (_, at_30)) in small.iter().zip(&large) {
        eprintln!(
            "acl statements, {label}: 3 projects {}, 30 projects {}",
            at_3.len(),
            at_30.len()
        );
    }
    for ((label, at_3), (_, at_30)) in small.iter().zip(&large) {
        assert_eq!(
            at_30.len(),
            at_3.len(),
            "{label}: 3 projects {at_3:#?}\n30 projects {at_30:#?}"
        );
    }

    drop(_guard);
    admin.close().await;
    harness.cleanup().await;
}
