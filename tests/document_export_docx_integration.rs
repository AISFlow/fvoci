#![cfg(feature = "db-tests")]
#![allow(dead_code)]

//! `GET …/documents/{id}/docx` (wiki and project) is served by the Rust
//! writer: the app state here has no Node convert helper at all.

#[path = "support/project_harness.rs"]
mod project_harness;

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use project_harness::{admin_pool, create_project, json_request, setup_session, test_peer, TestDb};
use serde_json::json;
use tower::ServiceExt;

const DOCX: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";

async fn get_raw(app: &axum::Router, path: &str, cookie: &str) -> (StatusCode, HeaderMap, Vec<u8>) {
    let request = Request::builder()
        .method("GET")
        .uri(path)
        .header("origin", "http://localhost")
        .header("cookie", format!("fvoci_session={cookie}"))
        .extension(axum::extract::ConnectInfo(test_peer()))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec();
    (status, headers, bytes)
}

fn body() -> serde_json::Value {
    json!({"type": "doc", "content": [
        {"type": "heading", "attrs": {"level": 2}, "content": [{"type": "text", "text": "안건"}]},
        {"type": "bulletList", "content": [{"type": "listItem", "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": "예산", "marks": [{"type": "bold"}]}]}]}]},
        {"type": "table", "content": [{"type": "tableRow", "content": [
            {"type": "tableHeader", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "항목"}]}]}]}]}
    ]})
}

/// Stored bodies are written directly: `PUT …/body` also seeds the Yjs
/// state, which still goes through the Node helper this app does not have.
async fn store_body(harness: &TestDb, id: &str) {
    let admin = admin_pool(harness).await;
    sqlx::query("UPDATE fvoci.documents SET content_json = $1 WHERE id = $2::uuid")
        .bind(body())
        .bind(id)
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}

fn assert_docx(status: StatusCode, headers: &HeaderMap, bytes: &[u8], filename_star: &str) {
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(bytes));
    assert_eq!(headers["content-type"], DOCX);
    let disposition = headers["content-disposition"].to_str().unwrap();
    assert!(disposition.contains(filename_star), "{disposition}");
    assert_eq!(headers["cache-control"], "private, no-store");
    assert_eq!(headers["x-content-type-options"], "nosniff");
    let docx = docx_rs::read_docx(bytes).expect("docx-rs reads the export");
    // Title paragraph + heading + list item + table.
    assert_eq!(docx.document.children.len(), 4);
}

#[tokio::test]
async fn wiki_and_project_docx_without_the_node_helper() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _owner, workspace_id) = setup_session(&harness).await;

    let (status, doc) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"parentId": null, "title": "회의록"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{doc}");
    let doc_id = doc["id"].as_str().unwrap();
    let base = format!("/api/v1/workspaces/{workspace_id}/documents/{doc_id}");
    store_body(&harness, doc_id).await;

    let (status, headers, bytes) = get_raw(&app, &format!("{base}/docx"), &cookie).await;
    assert_docx(
        status,
        &headers,
        &bytes,
        "filename*=UTF-8''%ED%9A%8C%EC%9D%98%EB%A1%9D.docx",
    );
    // PDF still needs the Node helper, which this state does not have.
    let (status, _, _) = get_raw(&app, &format!("{base}/pdf"), &cookie).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);

    let project = create_project(app.clone(), &cookie, workspace_id, "DOC", "workspace").await;
    let project_id = project["id"].as_str().unwrap();
    let root_id = project["rootDocumentId"].as_str().unwrap();
    let (status, pdoc) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents"),
        Some(json!({"title": "Spec", "parentId": root_id})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{pdoc}");
    let pdoc_id = pdoc["id"].as_str().unwrap();
    let pbase =
        format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{pdoc_id}");
    store_body(&harness, pdoc_id).await;
    let (status, headers, bytes) = get_raw(&app, &format!("{pbase}/docx"), &cookie).await;
    assert_docx(status, &headers, &bytes, "filename*=UTF-8''Spec.docx");

    // The workspace route does not serve project documents; no session, no file.
    let (status, _, _) = get_raw(
        &app,
        &format!("/api/v1/workspaces/{workspace_id}/documents/{pdoc_id}/docx"),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = get_raw(&app, &format!("{base}/docx"), "not-a-session").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    harness.cleanup().await;
}
