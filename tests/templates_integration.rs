#![cfg(feature = "db-tests")]

#[path = "support/project_harness.rs"]
mod project_harness;

use axum::http::StatusCode;
use project_harness::{
    add_workspace_user, admin_pool, app_pool, create_project, json_request, setup_session, TestDb,
};
use serde_json::{json, Value};
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
    assert!(applied_task["displayId"].as_str().unwrap().starts_with("PRJ-"));

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
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.templates WHERE workspace_id = $1")
        .bind(ws)
        .fetch_one(&app_pool)
        .await
        .unwrap();
    assert_eq!(count, 1);

    let other = Uuid::now_v7();
    sqlx::query("SELECT set_config('app.tenant_id', $1::text, true)")
        .bind(other.to_string())
        .execute(&app_pool)
        .await
        .unwrap();
    let denied: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.templates WHERE id = $1::uuid")
        .bind(Uuid::parse_str(tpl_id).unwrap())
        .fetch_one(&app_pool)
        .await
        .unwrap();
    assert_eq!(denied, 0);
    app_pool.close().await;

    db.cleanup().await;
}
