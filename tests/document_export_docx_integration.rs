#![cfg(feature = "db-tests")]
#![allow(dead_code)]

//! `GET …/documents/{id}/{docx,pptx,md}` (wiki and project) are served by the
//! Rust writers in the `--internal-markdown` child; there is no Node convert
//! helper any more.

#[path = "support/project_harness.rs"]
mod project_harness;

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use project_harness::{admin_pool, create_project, json_request, setup_session, test_peer, TestDb};
use serde_json::json;
use tower::ServiceExt;

const DOCX: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";
const PPTX: &str = "application/vnd.openxmlformats-officedocument.presentationml.presentation";

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
/// state through the collab hub, which this app state does not configure.
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

fn assert_file_headers(headers: &HeaderMap, content_type: &str, filename_star: &str) {
    assert_eq!(headers["content-type"], content_type);
    let disposition = headers["content-disposition"].to_str().unwrap();
    assert!(disposition.contains(filename_star), "{disposition}");
    assert_eq!(headers["cache-control"], "private, no-store");
    assert_eq!(headers["x-content-type-options"], "nosniff");
}

/// PPTX: the product's OOXML importer reads the slide back (title, heading,
/// list item, table cell).
fn assert_pptx(status: StatusCode, headers: &HeaderMap, bytes: &[u8], title: &str, file: &str) {
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(bytes));
    assert_file_headers(headers, PPTX, file);
    let text = match fvoci_server::documents::office::extract_office(
        bytes,
        fvoci_server::documents::office::OfficeKind::Pptx,
        fvoci_server::documents::office::OfficeMode::Markdown,
        1 << 20,
    ) {
        fvoci_server::documents::office::OfficeOutcome::Ok { text, .. } => text,
        other => panic!("{other:?}"),
    };
    assert!(text.starts_with(&format!("**{title}**")), "{text}");
    for want in ["**안건**", "**예산**", "| 항목 |"] {
        assert!(text.contains(want), "{want}: {text}");
    }
}

/// Markdown: source `documentMarkdown` (`# title`, then `tiptapDocToMd`).
fn assert_md(status: StatusCode, headers: &HeaderMap, bytes: &[u8], title: &str, file: &str) {
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(bytes));
    assert_file_headers(headers, "text/markdown; charset=utf-8", file);
    assert_eq!(
        String::from_utf8(bytes.to_vec()).unwrap(),
        format!("# {title}\n\n## 안건\n\n- **예산**\n\n| 항목 |\n| --- |\n")
    );
}

#[tokio::test]
async fn wiki_and_project_office_and_markdown_exports_in_rust() {
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
    let (status, headers, bytes) = get_raw(&app, &format!("{base}/pptx"), &cookie).await;
    assert_pptx(
        status,
        &headers,
        &bytes,
        "회의록",
        "filename*=UTF-8''%ED%9A%8C%EC%9D%98%EB%A1%9D.pptx",
    );
    let (status, headers, bytes) = get_raw(&app, &format!("{base}/md"), &cookie).await;
    assert_md(
        status,
        &headers,
        &bytes,
        "회의록",
        "filename*=UTF-8''%ED%9A%8C%EC%9D%98%EB%A1%9D.md",
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
    store_body(&harness, pdoc_id).await;
    let (status, headers, bytes) = get_raw(&app, &format!("{pbase}/docx"), &cookie).await;
    assert_docx(status, &headers, &bytes, "filename*=UTF-8''Spec.docx");
    let (status, headers, bytes) = get_raw(&app, &format!("{pbase}/pptx"), &cookie).await;
    assert_pptx(
        status,
        &headers,
        &bytes,
        "Spec",
        "filename*=UTF-8''Spec.pptx",
    );
    let (status, headers, bytes) = get_raw(&app, &format!("{pbase}/md"), &cookie).await;
    assert_md(status, &headers, &bytes, "Spec", "filename*=UTF-8''Spec.md");

    // The workspace route does not serve project documents; no session, no file.
    for format in ["docx", "pptx", "md"] {
        let (status, _, _) = get_raw(
            &app,
            &format!("/api/v1/workspaces/{workspace_id}/documents/{pdoc_id}/{format}"),
            &cookie,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{format}");
        let (status, _, _) = get_raw(&app, &format!("{base}/{format}"), "not-a-session").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{format}");
    }
    harness.cleanup().await;
}
