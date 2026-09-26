#![cfg(feature = "db-tests")]
#![allow(dead_code)]

#[path = "support/project_harness.rs"]
mod project_harness;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use project_harness::{
    add_workspace_user, admin_pool, app_pool, create_project, insert_minimal_project, json_request,
    setup_session, test_peer, TestDb,
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

async fn call_bearer(
    app: axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    secret: &str,
) -> (StatusCode, Value) {
    let builder = Request::builder()
        .method(method)
        .uri(path)
        .header("origin", "http://localhost")
        .header("authorization", format!("Bearer {secret}"));
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

async fn create_pat(
    app: &axum::Router,
    cookie: &str,
    ws: Uuid,
    name: &str,
    scopes: &[&str],
) -> (String, String) {
    let (status, body) = call(
        app,
        "POST",
        &format!("/api/v1/workspaces/{ws}/api-tokens"),
        Some(json!({ "name": name, "scopes": scopes })),
        cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    (
        body["id"].as_str().expect("token id").to_string(),
        body["token"].as_str().expect("token secret").to_string(),
    )
}

fn listed_kinds(items: &Value) -> Vec<&str> {
    items
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["kind"].as_str().unwrap())
        .collect()
}

async fn workspace_counts(admin: &PgPool, ws: Uuid) -> (i64, i64) {
    let documents: i64 =
        sqlx::query_scalar("SELECT count(*) FROM fvoci.documents WHERE workspace_id = $1")
            .bind(ws)
            .fetch_one(admin)
            .await
            .unwrap();
    let tasks: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.tasks WHERE workspace_id = $1")
        .bind(ws)
        .fetch_one(admin)
        .await
        .unwrap();
    (documents, tasks)
}

async fn add_project_member(
    app: &axum::Router,
    cookie: &str,
    ws: Uuid,
    project_id: &str,
    user_id: Uuid,
    role: &str,
) {
    let (status, body) = call(
        app,
        "POST",
        &format!("/api/v1/workspaces/{ws}/projects/{project_id}/members"),
        Some(json!({ "userId": user_id, "role": role })),
        cookie,
    )
    .await;
    assert!(status.is_success(), "{status} {body}");
}

#[tokio::test]
async fn templates_list_create_apply_and_guest_denied() {
    let db = TestDb::bootstrap().await;
    let (app, owner_cookie, _owner, ws) = setup_session(&db).await;
    let project = create_project(app.clone(), &owner_cookie, ws, "PRJ", "workspace").await;
    let project_id = project["id"].as_str().unwrap();

    let base = format!("/api/v1/workspaces/{ws}/templates");
    let (status, listed) = call(&app, "GET", &base, None, &owner_cookie).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(listed["items"].as_array().unwrap().len(), 0);

    let (status, doc_tpl) = call(
        &app,
        "POST",
        &base,
        Some(json!({
            "kind": "document",
            "title": "Doc tpl",
            "payload": { "title": "Doc tpl" }
        })),
        &owner_cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{doc_tpl}");
    let doc_tpl_id = doc_tpl["id"].as_str().unwrap();

    let (status, applied) = call(
        &app,
        "POST",
        &format!("{base}/{doc_tpl_id}/apply"),
        Some(json!({})),
        &owner_cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{applied}");
    assert_eq!(applied["kind"], "document");
    assert!(applied["displayId"].as_str().unwrap().starts_with("WIKI-"));

    let (status, task_tpl) = call(
        &app,
        "POST",
        &base,
        Some(json!({
            "kind": "task",
            "title": "Task tpl",
            "payload": { "title": "Task tpl" }
        })),
        &owner_cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{task_tpl}");
    let task_tpl_id = task_tpl["id"].as_str().unwrap();

    let (status, applied_task) = call(
        &app,
        "POST",
        &format!("{base}/{task_tpl_id}/apply"),
        Some(json!({ "projectId": project_id })),
        &owner_cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{applied_task}");
    assert_eq!(applied_task["kind"], "task");
    assert!(applied_task["displayId"]
        .as_str()
        .unwrap()
        .starts_with("PRJ-"));

    let admin = admin_pool(&db).await;
    let guest = add_workspace_user(&admin, ws, "guest", "guest").await;
    admin.close().await;
    let (status, _) = call(&app, "GET", &base, None, &guest.cookie).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    db.cleanup().await;
}

#[tokio::test]
async fn templates_rls_denies_cross_tenant_reads() {
    let db = TestDb::bootstrap().await;
    let (app, cookie, _user, ws) = setup_session(&db).await;
    let base = format!("/api/v1/workspaces/{ws}/templates");
    let (status, created) = call(
        &app,
        "POST",
        &base,
        Some(json!({
            "kind": "document",
            "title": "Secret",
            "payload": { "title": "Secret" }
        })),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let tpl_id = created["id"].as_str().unwrap();

    let app_pool = app_pool(&db).await;
    let tpl_uuid = Uuid::parse_str(tpl_id).unwrap();
    let mut tx = app_pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
        .bind(ws.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM fvoci.templates WHERE workspace_id = $1")
            .bind(ws)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(count, 1);
    tx.commit().await.unwrap();

    let other = Uuid::now_v7();
    let mut tx = app_pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
        .bind(other.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let denied: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.templates WHERE id = $1")
        .bind(tpl_uuid)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(denied, 0);
    tx.rollback().await.unwrap();
    app_pool.close().await;

    db.cleanup().await;
}

#[tokio::test]
async fn templates_pat_kind_scopes_and_revoked_token() {
    let db = TestDb::bootstrap().await;
    let (app, owner_cookie, _owner, ws) = setup_session(&db).await;
    let project = create_project(app.clone(), &owner_cookie, ws, "PAT", "workspace").await;
    let project_id = project["id"].as_str().unwrap();
    let base = format!("/api/v1/workspaces/{ws}/templates");

    let (status, doc_tpl) = call(
        &app,
        "POST",
        &base,
        Some(json!({
            "kind": "document",
            "title": "Doc",
            "payload": { "title": "Doc" }
        })),
        &owner_cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{doc_tpl}");
    let doc_tpl_id = doc_tpl["id"].as_str().unwrap();

    let (status, task_tpl) = call(
        &app,
        "POST",
        &base,
        Some(json!({
            "kind": "task",
            "title": "Task",
            "payload": { "title": "Task" }
        })),
        &owner_cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{task_tpl}");
    let task_tpl_id = task_tpl["id"].as_str().unwrap();

    let (_id, doc_read) =
        create_pat(&app, &owner_cookie, ws, "doc-read", &["documents.read"]).await;
    let (status, listed) = call_bearer(app.clone(), "GET", &base, None, &doc_read).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(listed_kinds(&listed["items"]), ["document"]);
    let (status, denied) = call_bearer(
        app.clone(),
        "POST",
        &base,
        Some(json!({
            "kind": "document",
            "title": "Nope",
            "payload": { "title": "Nope" }
        })),
        &doc_read,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{denied}");
    let (status, denied) = call_bearer(
        app.clone(),
        "POST",
        &format!("{base}/{doc_tpl_id}/apply"),
        Some(json!({})),
        &doc_read,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{denied}");

    let (_id, task_read) = create_pat(&app, &owner_cookie, ws, "task-read", &["tasks.read"]).await;
    let (status, listed) = call_bearer(app.clone(), "GET", &base, None, &task_read).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(listed_kinds(&listed["items"]), ["task"]);

    let (_id, doc_write) =
        create_pat(&app, &owner_cookie, ws, "doc-write", &["documents.write"]).await;
    let (status, listed) = call_bearer(app.clone(), "GET", &base, None, &doc_write).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(listed_kinds(&listed["items"]), ["document"]);
    let (status, created) = call_bearer(
        app.clone(),
        "POST",
        &base,
        Some(json!({
            "kind": "document",
            "title": "Pat doc",
            "payload": { "title": "Pat doc" }
        })),
        &doc_write,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let (status, denied) = call_bearer(
        app.clone(),
        "POST",
        &base,
        Some(json!({
            "kind": "task",
            "title": "Pat task",
            "payload": { "title": "Pat task" }
        })),
        &doc_write,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{denied}");
    let (status, applied) = call_bearer(
        app.clone(),
        "POST",
        &format!("{base}/{doc_tpl_id}/apply"),
        Some(json!({})),
        &doc_write,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{applied}");
    assert_eq!(applied["kind"], "document");
    let (status, denied) = call_bearer(
        app.clone(),
        "POST",
        &format!("{base}/{task_tpl_id}/apply"),
        Some(json!({ "projectId": project_id })),
        &doc_write,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{denied}");

    let (_id, task_write) =
        create_pat(&app, &owner_cookie, ws, "task-write", &["tasks.write"]).await;
    let (status, listed) = call_bearer(app.clone(), "GET", &base, None, &task_write).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(listed_kinds(&listed["items"]), ["task"]);
    let (status, applied) = call_bearer(
        app.clone(),
        "POST",
        &format!("{base}/{task_tpl_id}/apply"),
        Some(json!({ "projectId": project_id })),
        &task_write,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{applied}");
    assert_eq!(applied["kind"], "task");
    let (status, denied) = call_bearer(
        app.clone(),
        "POST",
        &format!("{base}/{doc_tpl_id}/apply"),
        Some(json!({})),
        &task_write,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{denied}");

    let (token_id, revoked_secret) = create_pat(
        &app,
        &owner_cookie,
        ws,
        "revoked",
        &["documents.write", "tasks.write"],
    )
    .await;
    let (status, listed) = call_bearer(app.clone(), "GET", &base, None, &revoked_secret).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let (status, revoked, ..) = project_harness::json_request_with_headers(
        app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{ws}/api-tokens/{token_id}"),
        None,
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{revoked}");
    let (status, denied) = call_bearer(app.clone(), "GET", &base, None, &revoked_secret).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{denied}");
    assert_eq!(denied["code"], "authentication_required");
    let (status, denied) = call_bearer(
        app.clone(),
        "POST",
        &format!("{base}/{doc_tpl_id}/apply"),
        Some(json!({})),
        &revoked_secret,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{denied}");

    db.cleanup().await;
}

#[tokio::test]
async fn templates_guest_apply_archive_and_foreign_targets() {
    let db = TestDb::bootstrap().await;
    let (app, owner_cookie, owner_id, ws) = setup_session(&db).await;
    let allowed = create_project(app.clone(), &owner_cookie, ws, "OKP", "workspace").await;
    let denied = create_project(app.clone(), &owner_cookie, ws, "NOPE", "private").await;
    let allowed_id = allowed["id"].as_str().unwrap().to_string();
    let denied_id = denied["id"].as_str().unwrap().to_string();
    let base = format!("/api/v1/workspaces/{ws}/templates");

    let (status, doc_tpl) = call(
        &app,
        "POST",
        &base,
        Some(json!({
            "kind": "document",
            "title": "Guest doc",
            "payload": { "title": "Guest doc" }
        })),
        &owner_cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{doc_tpl}");
    let doc_tpl_id = doc_tpl["id"].as_str().unwrap().to_string();

    let (status, task_tpl) = call(
        &app,
        "POST",
        &base,
        Some(json!({
            "kind": "task",
            "title": "Guest task",
            "payload": { "title": "Guest task" }
        })),
        &owner_cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{task_tpl}");
    let task_tpl_id = task_tpl["id"].as_str().unwrap().to_string();

    let admin = admin_pool(&db).await;
    let guest = add_workspace_user(&admin, ws, "guest", "guest").await;
    add_project_member(
        &app,
        &owner_cookie,
        ws,
        &allowed_id,
        guest.user_id,
        "member",
    )
    .await;

    let (status, _) = call(&app, "GET", &base, None, &guest.cookie).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, created) = call(
        &app,
        "POST",
        &base,
        Some(json!({
            "kind": "document",
            "title": "Guest create",
            "payload": { "title": "Guest create" }
        })),
        &guest.cookie,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{created}");

    let (status, denied_wiki) = call(
        &app,
        "POST",
        &format!("{base}/{doc_tpl_id}/apply"),
        Some(json!({})),
        &guest.cookie,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{denied_wiki}");
    let (status, denied_other) = call(
        &app,
        "POST",
        &format!("{base}/{task_tpl_id}/apply"),
        Some(json!({ "projectId": denied_id })),
        &guest.cookie,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{denied_other}");

    let (status, applied_doc) = call(
        &app,
        "POST",
        &format!("{base}/{doc_tpl_id}/apply"),
        Some(json!({ "projectId": allowed_id })),
        &guest.cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{applied_doc}");
    assert_eq!(applied_doc["kind"], "document");
    assert!(applied_doc["displayId"]
        .as_str()
        .unwrap()
        .starts_with("OKP-"));

    let (status, applied_task) = call(
        &app,
        "POST",
        &format!("{base}/{task_tpl_id}/apply"),
        Some(json!({ "projectId": allowed_id })),
        &guest.cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{applied_task}");
    assert_eq!(applied_task["kind"], "task");
    assert!(applied_task["displayId"]
        .as_str()
        .unwrap()
        .starts_with("OKP-"));

    let (docs_before, tasks_before) = workspace_counts(&admin, ws).await;

    let (status, wiki_doc) = call(
        &app,
        "POST",
        &format!("{base}/{doc_tpl_id}/apply"),
        Some(json!({})),
        &owner_cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{wiki_doc}");
    let wiki_parent = wiki_doc["id"].as_str().unwrap();

    let (status, foreign_parent) = call(
        &app,
        "POST",
        &format!("{base}/{doc_tpl_id}/apply"),
        Some(json!({
            "projectId": allowed_id,
            "parentId": wiki_parent
        })),
        &owner_cookie,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{foreign_parent}");

    let other_ws = Uuid::now_v7();
    let other_project = Uuid::now_v7();
    sqlx::query("INSERT INTO fvoci.workspaces (id, slug, name) VALUES ($1, $2, 'Other')")
        .bind(other_ws)
        .bind(format!("other-{}", &other_ws.simple().to_string()[20..]))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO fvoci.memberships (workspace_id, user_id, role) VALUES ($1, $2, 'owner')",
    )
    .bind(other_ws)
    .bind(owner_id)
    .execute(&admin)
    .await
    .unwrap();
    insert_minimal_project(
        &admin,
        other_ws,
        other_project,
        "FRN",
        owner_id,
        "workspace",
    )
    .await;

    let (status, foreign_project) = call(
        &app,
        "POST",
        &format!("{base}/{task_tpl_id}/apply"),
        Some(json!({ "projectId": other_project })),
        &owner_cookie,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{foreign_project}");

    let (status, archived) = call(
        &app,
        "POST",
        &format!("/api/v1/workspaces/{ws}/projects/{denied_id}/archive"),
        Some(json!({})),
        &owner_cookie,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{archived}");
    let (status, archived_apply) = call(
        &app,
        "POST",
        &format!("{base}/{task_tpl_id}/apply"),
        Some(json!({ "projectId": denied_id })),
        &owner_cookie,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{archived_apply}");
    assert_eq!(archived_apply["code"], "project_archived");

    let (docs_after, tasks_after) = workspace_counts(&admin, ws).await;
    assert_eq!(
        docs_after,
        docs_before + 1,
        "only the wiki apply created a document"
    );
    assert_eq!(
        tasks_after, tasks_before,
        "failed applies must not create tasks"
    );

    admin.close().await;
    db.cleanup().await;
}
