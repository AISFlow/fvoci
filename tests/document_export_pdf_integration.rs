#![cfg(feature = "db-tests")]
#![allow(dead_code)]

//! `GET …/documents/{id}/pdf` (wiki and project) is served by the Rust
//! writer: the app state here has no Node convert helper at all.

#[path = "support/project_harness.rs"]
mod project_harness;

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use project_harness::{
    admin_pool, create_project, json_request, setup_session, setup_session_with_markdown,
    test_peer, TestDb,
};
use serde_json::json;
use tower::ServiceExt;

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
        {"type": "heading", "attrs": {"level": 2}, "content": [{"type": "text", "text": "안건 🎉"}]},
        {"type": "bulletList", "content": [{"type": "listItem", "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": "예산", "marks": [{"type": "bold"}]}]}]}]},
        {"type": "paragraph", "content": [{"type": "text", "text": "링크", "marks": [
            {"type": "link", "attrs": {"href": "https://example.com/회의"}}]}]},
        {"type": "table", "content": [{"type": "tableRow", "content": [
            {"type": "tableHeader", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "항목"}]}]}]}]}
    ]})
}

/// Stored bodies are written directly: `PUT …/body` also seeds the Yjs
/// state through the collab hub, which this app state does not configure.
async fn store_body(harness: &TestDb, id: &str, body: serde_json::Value) {
    let admin = admin_pool(harness).await;
    sqlx::query("UPDATE fvoci.documents SET content_json = $1 WHERE id = $2::uuid")
        .bind(body)
        .bind(id)
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}

fn assert_pdf(
    status: StatusCode,
    headers: &HeaderMap,
    bytes: &[u8],
    filename_star: &str,
    title: &str,
) {
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(bytes));
    assert_eq!(headers["content-type"], "application/pdf");
    let disposition = headers["content-disposition"].to_str().unwrap();
    assert!(disposition.starts_with("attachment;"), "{disposition}");
    assert!(disposition.contains(filename_star), "{disposition}");
    assert_eq!(headers["cache-control"], "private, no-store");
    assert_eq!(headers["x-content-type-options"], "nosniff");
    let pdf = pdf_extract::Document::load_mem(bytes).expect("lopdf reads the export");
    assert_eq!(pdf.get_pages().len(), 1);
    let text: String = pdf_extract::extract_text_from_mem(bytes)
        .unwrap()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    for want in [title, "안건🎉", "예산", "링크", "항목"] {
        assert!(text.contains(want), "{want}: {text}");
    }
    // The link keeps a URI annotation (percent-encoded, 7-bit).
    let raw = String::from_utf8_lossy(bytes);
    assert!(
        raw.contains("https://example.com/%ED%9A%8C%EC%9D%98"),
        "link annotation"
    );
}

#[tokio::test]
async fn wiki_and_project_pdf_without_the_node_helper() {
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
    store_body(&harness, doc_id, body()).await;

    let (status, headers, bytes) = get_raw(&app, &format!("{base}/pdf"), &cookie).await;
    assert_pdf(
        status,
        &headers,
        &bytes,
        "filename*=UTF-8''%ED%9A%8C%EC%9D%98%EB%A1%9D.pdf",
        "회의록",
    );

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
    store_body(&harness, pdoc_id, body()).await;
    let (status, headers, bytes) = get_raw(&app, &format!("{pbase}/pdf"), &cookie).await;
    assert_pdf(
        status,
        &headers,
        &bytes,
        "filename*=UTF-8''Spec.pdf",
        "Spec",
    );

    // Authorization before conversion: the workspace route does not serve
    // project documents; no session, no file.
    let (status, _, _) = get_raw(
        &app,
        &format!("/api/v1/workspaces/{workspace_id}/documents/{pdoc_id}/pdf"),
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = get_raw(&app, &format!("{base}/pdf"), "not-a-session").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // A stored body that is not a Tiptap doc is 400 (source `invalid_input`).
    store_body(&harness, doc_id, json!({"type": "paragraph"})).await;
    let (status, _, _) = get_raw(&app, &format!("{base}/pdf"), &cookie).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    harness.cleanup().await;
}

/// Without the export child (it could not be located at startup) the route
/// is a logged 500 after authorization, like the DOCX export.
#[tokio::test]
async fn pdf_without_the_export_child_is_a_server_error() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _owner, workspace_id) = setup_session_with_markdown(&harness, None).await;
    let (status, doc) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"parentId": null, "title": "없음"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{doc}");
    let base = format!(
        "/api/v1/workspaces/{workspace_id}/documents/{}",
        doc["id"].as_str().unwrap()
    );
    let (status, _, _) = get_raw(&app, &format!("{base}/pdf"), "not-a-session").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _, _) = get_raw(&app, &format!("{base}/pdf"), &cookie).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    harness.cleanup().await;
}
