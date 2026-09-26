#![cfg(feature = "db-tests")]
#![allow(dead_code)]

#[path = "support/project_harness.rs"]
mod project_harness;

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use fvoci_server::auth::token::hash_token;
use fvoci_server::db::context::{set_system, set_tenant};
use fvoci_server::db::share::hydrate_share_hits;
use project_harness::{
    add_workspace_user, admin_pool, app_pool, create_project, http_request, json_request,
    setup_session, setup_session_with_convert, TestDb,
};
use serde_json::{json, Value};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

async fn raw_get(
    app: axum::Router,
    path: &str,
    headers: &[(&str, &str)],
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let mut builder = Request::builder().method("GET").uri(path);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let mut request = builder.body(Body::empty()).unwrap();
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(project_harness::test_peer()));
    let response = app.oneshot(request).await.expect("response");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap_or_default()
        .to_vec();
    (status, headers, bytes)
}

async fn public_json(app: axum::Router, path: &str) -> (StatusCode, Value) {
    let (status, _, bytes) = raw_get(app, path, &[]).await;
    let body = serde_json::from_slice(&bytes).unwrap_or(json!({}));
    (status, body)
}

async fn bearer(
    app: axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    token: &str,
) -> (StatusCode, Value) {
    let auth = format!("Bearer {token}");
    let bytes = body.map(|value| value.to_string().into_bytes());
    let (status, json, _) = http_request(
        app,
        method,
        path,
        bytes,
        Some("application/json"),
        None,
        &[("authorization", &auth)],
    )
    .await;
    (status, json)
}

async fn create_wiki_doc(
    app: &axum::Router,
    cookie: &str,
    workspace_id: Uuid,
    parent: Option<&str>,
    title: &str,
) -> String {
    let (status, body) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"parentId": parent, "title": title})),
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["id"].as_str().unwrap().to_string()
}

async fn create_task(
    app: &axum::Router,
    cookie: &str,
    workspace_id: Uuid,
    project_id: &str,
    title: &str,
) -> String {
    let (status, body) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({"title": title})),
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["id"].as_str().unwrap().to_string()
}

async fn create_api_token(
    app: &axum::Router,
    cookie: &str,
    workspace_id: Uuid,
    scopes: &[&str],
) -> String {
    let (status, body) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/api-tokens"),
        Some(json!({"name": format!("t-{}", Uuid::now_v7().simple()), "scopes": scopes})),
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["token"].as_str().unwrap().to_string()
}

async fn share_document(
    app: &axum::Router,
    cookie: &str,
    workspace_id: Uuid,
    document_id: &str,
) -> (Value, String) {
    let (status, body) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/share-links"),
        Some(json!({})),
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let url = body["url"].as_str().unwrap();
    let token = url
        .strip_prefix("http://localhost/s/")
        .unwrap_or_else(|| panic!("share url {url}"))
        .to_string();
    (body, token)
}

async fn set_content(admin: &PgPool, document_id: &str, content: Value) {
    sqlx::query("UPDATE fvoci.documents SET content_json = $2, updated_at = now() WHERE id = $1")
        .bind(Uuid::parse_str(document_id).unwrap())
        .bind(content)
        .execute(admin)
        .await
        .expect("set content");
}

async fn insert_attachment(
    admin: &PgPool,
    workspace_id: Uuid,
    document_id: &str,
    uploader: Uuid,
    status: &str,
    scan_status: &str,
) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO fvoci.attachments (
            id, workspace_id, document_id, uploader_id, status, name, reserved_size_bytes,
            size_bytes, storage_key, scan_status, completed_at
        ) VALUES (
            $1, $2, $3, $4, $5, 'secret.txt', 4,
            CASE WHEN $5 = 'stored' THEN 4 END, $6, $7,
            CASE WHEN $5 = 'stored' THEN now() END
        )
        "#,
    )
    .bind(id)
    .bind(workspace_id)
    .bind(Uuid::parse_str(document_id).unwrap())
    .bind(uploader)
    .bind(status)
    .bind(format!("{workspace_id}/{id}"))
    .bind(scan_status)
    .execute(admin)
    .await
    .expect("insert attachment");
    id
}

async fn upload_attachment(
    app: &axum::Router,
    cookie: &str,
    workspace_id: Uuid,
    document_id: &str,
    bytes: &[u8],
) -> String {
    let (status, created) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/uploads"),
        Some(json!({"name": "공유 첨부.txt", "sizeBytes": bytes.len()})),
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let attachment_id = created["attachmentId"].as_str().unwrap().to_string();
    let part_url = created["parts"][0]["url"].as_str().unwrap().to_string();
    let (status, _, headers) = http_request(
        app.clone(),
        "PUT",
        &part_url,
        Some(bytes.to_vec()),
        Some("application/octet-stream"),
        Some(cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let etag = headers["etag"].to_str().unwrap().to_string();
    let (status, completed) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/attachments/{attachment_id}/complete"),
        Some(json!({"parts": [{"partNumber": 1, "etag": etag}]})),
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{completed}");
    attachment_id
}

fn ids(items: &Value) -> Vec<String> {
    items["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn stars_and_recent_follow_current_read_access_and_token_kinds() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, ws) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, ws, "member", "star-member").await;

    let doc = create_wiki_doc(&app, &cookie, ws, None, "별표 문서").await;
    let public_project = create_project(app.clone(), &cookie, ws, "PUB", "workspace").await;
    let private_project = create_project(app.clone(), &cookie, ws, "PRV", "private").await;
    let task = create_task(
        &app,
        &cookie,
        ws,
        public_project["id"].as_str().unwrap(),
        "공개 태스크",
    )
    .await;
    let hidden_task = create_task(
        &app,
        &cookie,
        ws,
        private_project["id"].as_str().unwrap(),
        "비공개 태스크",
    )
    .await;
    let stars = format!("/api/v1/workspaces/{ws}/stars");

    let (status, first) = json_request(
        app.clone(),
        "POST",
        &stars,
        Some(json!({"type": "document", "id": doc})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{first}");
    assert_eq!(first["type"], "document");
    assert_eq!(first["targetId"], doc.as_str());
    assert_eq!(first["title"], "별표 문서");
    assert_eq!(first["projectId"], Value::Null);
    let (status, again) = json_request(
        app.clone(),
        "POST",
        &stars,
        Some(json!({"type": "document", "id": doc})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(again["id"], first["id"], "starring twice is idempotent");
    let (status, task_star) = json_request(
        app.clone(),
        "POST",
        &stars,
        Some(json!({"type": "task", "id": task})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{task_star}");
    assert_eq!(task_star["projectId"], public_project["id"]);

    for bad in [
        json!({"type": "comment", "id": doc}),
        json!({"type": "document", "id": "not-a-uuid"}),
        json!({"type": "document", "id": doc, "extra": 1}),
        json!({"type": "document"}),
    ] {
        let (status, body) = json_request(
            app.clone(),
            "POST",
            &stars,
            Some(bad.clone()),
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad} -> {body}");
    }
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &stars,
        Some(json!({"type": "task", "id": Uuid::now_v7()})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // A member cannot star (or see) a task in a private project they are not in.
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &stars,
        Some(json!({"type": "task", "id": hidden_task})),
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, recent) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{ws}/recent?limit=50"),
        None,
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{recent}");
    let member_recent = ids(&recent);
    assert!(member_recent.contains(&doc));
    assert!(member_recent.contains(&task));
    assert!(!member_recent.contains(&hidden_task), "{recent}");

    let (status, listed) = json_request(app.clone(), "GET", &stars, None, Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        ids(&listed),
        vec![
            task_star["id"].as_str().unwrap().to_string(),
            first["id"].as_str().unwrap().to_string()
        ],
        "newest first"
    );
    // Other users never see someone else's stars.
    let (_, member_stars) =
        json_request(app.clone(), "GET", &stars, None, Some(&member.cookie)).await;
    assert_eq!(member_stars["items"], json!([]));
    let (status, _) = json_request(
        app.clone(),
        "DELETE",
        &format!("{stars}/{}", first["id"].as_str().unwrap()),
        None,
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Recent: owner sees everything, limit validation follows the source.
    let (status, recent) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{ws}/recent?limit=1"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(recent["items"].as_array().unwrap().len(), 1);
    for bad in ["?limit=0", "?limit=51", "?limit=x", "?other=1"] {
        let (status, _) = json_request(
            app.clone(),
            "GET",
            &format!("/api/v1/workspaces/{ws}/recent{bad}"),
            None,
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }

    // API tokens: tasks.read only sees task stars and cannot touch document stars.
    let tasks_token = create_api_token(&app, &cookie, ws, &["tasks.read"]).await;
    let (status, token_list) = bearer(app.clone(), "GET", &stars, None, &tasks_token).await;
    assert_eq!(status, StatusCode::OK, "{token_list}");
    assert_eq!(
        ids(&token_list),
        vec![task_star["id"].as_str().unwrap().to_string()]
    );
    let (status, _) = bearer(
        app.clone(),
        "POST",
        &stars,
        Some(json!({"type": "document", "id": doc})),
        &tasks_token,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = bearer(
        app.clone(),
        "DELETE",
        &format!("{stars}/{}", first["id"].as_str().unwrap()),
        None,
        &tasks_token,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, token_recent) = bearer(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{ws}/recent"),
        None,
        &tasks_token,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let token_items = token_recent["items"].as_array().unwrap();
    assert!(!token_items.is_empty(), "{token_recent}");
    assert!(
        token_items.iter().all(|i| i["type"] == "task"),
        "{token_recent}"
    );
    assert!(ids(&token_recent).contains(&task), "{token_recent}");
    let share_only = create_api_token(&app, &cookie, ws, &["share.manage"]).await;
    let (status, empty) = bearer(app.clone(), "GET", &stars, None, &share_only).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(empty["items"], json!([]));

    // Trashing the document hides its star and recent entry; the star row stays.
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/documents/{doc}/trash"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, listed) = json_request(app.clone(), "GET", &stars, None, Some(&cookie)).await;
    assert_eq!(
        ids(&listed),
        vec![task_star["id"].as_str().unwrap().to_string()]
    );
    let doc_star_rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM fvoci.stars WHERE document_id = $1 AND user_id = $2",
    )
    .bind(Uuid::parse_str(&doc).unwrap())
    .bind(owner_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(doc_star_rows, 1, "the star row stays while hidden");
    let (_, recent) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{ws}/recent?limit=50"),
        None,
        Some(&cookie),
    )
    .await;
    assert!(!ids(&recent).contains(&doc));
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &stars,
        Some(json!({"type": "document", "id": doc})),
        Some(&cookie),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "cannot star a trashed document"
    );

    // Archived tasks leave the list.
    sqlx::query("UPDATE fvoci.tasks SET archived_at = now() WHERE id = $1")
        .bind(Uuid::parse_str(&task).unwrap())
        .execute(&admin)
        .await
        .unwrap();
    let (_, listed) = json_request(app.clone(), "GET", &stars, None, Some(&cookie)).await;
    assert_eq!(listed["items"], json!([]));
    sqlx::query("UPDATE fvoci.tasks SET archived_at = NULL WHERE id = $1")
        .bind(Uuid::parse_str(&task).unwrap())
        .execute(&admin)
        .await
        .unwrap();

    let (status, removed) = json_request(
        app.clone(),
        "DELETE",
        &format!("{stars}/{}", task_star["id"].as_str().unwrap()),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{removed}");
    assert_eq!(removed["ok"], true);
    let (status, _) = json_request(
        app.clone(),
        "DELETE",
        &format!("{stars}/{}", task_star["id"].as_str().unwrap()),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Removed members and outsiders get 404 on every route; their stars are gone.
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &stars,
        Some(json!({"type": "task", "id": task})),
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    sqlx::query("DELETE FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2")
        .bind(ws)
        .bind(member.user_id)
        .execute(&admin)
        .await
        .unwrap();
    let (status, _) = json_request(app.clone(), "GET", &stars, None, Some(&member.cookie)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{ws}/recent"),
        None,
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.stars WHERE user_id = $1")
        .bind(member.user_id)
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(left, 0, "membership removal cascades to stars");
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn document_share_scope_rechecks_state_on_every_request() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, ws) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;

    let root = create_wiki_doc(&app, &cookie, ws, None, "공유 루트").await;
    let child = create_wiki_doc(&app, &cookie, ws, Some(&root), "자식").await;
    let grandchild = create_wiki_doc(&app, &cookie, ws, Some(&child), "손자").await;
    let sibling = create_wiki_doc(&app, &cookie, ws, None, "형제 비공개").await;
    let sibling_child = create_wiki_doc(&app, &cookie, ws, Some(&sibling), "형제 자식").await;
    set_content(
        &admin,
        &root,
        json!({"type": "doc", "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": "<script>alert(1)</script> 본문"}]},
            {"type": "paragraph", "content": [{"type": "text", "text": "링크",
                "marks": [{"type": "link", "attrs": {"href": "javascript:alert(1)"}}]}]}
        ]}),
    )
    .await;

    let (created, token) = share_document(&app, &cookie, ws, &root).await;
    assert_eq!(created["documentId"], root.as_str());
    assert_eq!(created["projectId"], Value::Null);
    let expires = chrono::DateTime::parse_from_rfc3339(created["expiresAt"].as_str().unwrap())
        .unwrap()
        .with_timezone(&chrono::Utc);
    let days = (expires - chrono::Utc::now()).num_hours();
    assert!(
        (7 * 24 - 1..=7 * 24).contains(&days),
        "default 7 days: {days}h"
    );
    assert_eq!(token.len(), 43);

    // Only the hash is stored.
    let stored: (String,) =
        sqlx::query_as("SELECT token_hash FROM fvoci.share_links WHERE id = $1")
            .bind(Uuid::parse_str(created["id"].as_str().unwrap()).unwrap())
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(stored.0, hash_token(&token));
    assert_ne!(stored.0, token);

    let base = format!("/api/v1/share/{token}");
    let (status, meta) = public_json(app.clone(), &base).await;
    assert_eq!(status, StatusCode::OK, "{meta}");
    assert_eq!(meta["title"], "공유 루트");
    assert_eq!(meta["documentId"], root.as_str());
    assert_eq!(meta["projectId"], Value::Null);

    // Body formats: sanitized HTML, fragment ETag/304, markdown.
    let (status, headers, html) = raw_get(app.clone(), &format!("{base}/body"), &[]).await;
    assert_eq!(status, StatusCode::OK);
    let html = String::from_utf8(html).unwrap();
    assert!(html.starts_with("<!DOCTYPE html>"), "{html}");
    assert!(
        html.contains("&lt;script&gt;alert(1)&lt;/script&gt; 본문"),
        "{html}"
    );
    assert!(!html.contains("<script>"), "{html}");
    assert!(!html.contains("javascript:"), "{html}");
    let csp = headers["content-security-policy"].to_str().unwrap();
    assert!(
        csp.starts_with("default-src 'none'; style-src 'nonce-"),
        "{csp}"
    );
    assert_eq!(headers["cache-control"], "private, no-store");
    assert!(csp.contains("img-src 'self' data: blob:"), "{csp}");
    assert_eq!(headers["referrer-policy"], "no-referrer");
    assert!(
        html.contains("<meta name=\"referrer\" content=\"no-referrer\"/>"),
        "{html}"
    );
    let (status, headers, fragment) =
        raw_get(app.clone(), &format!("{base}/body?format=fragment"), &[]).await;
    assert_eq!(status, StatusCode::OK);
    let fragment = String::from_utf8(fragment).unwrap();
    assert!(fragment.starts_with("<p>&lt;script&gt;"), "{fragment}");
    assert!(fragment.contains("<a>링크</a>"), "{fragment}");
    assert_eq!(
        headers["content-security-policy"],
        "default-src 'none'; sandbox"
    );
    assert_eq!(headers["referrer-policy"], "no-referrer");
    let etag = headers["etag"].to_str().unwrap().to_string();
    let (status, _, empty) = raw_get(
        app.clone(),
        &format!("{base}/body?format=fragment"),
        &[("if-none-match", &format!("W/{etag}"))],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_MODIFIED);
    assert!(empty.is_empty());
    let (status, headers, md) = raw_get(app.clone(), &format!("{base}/body?format=md"), &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["content-type"], "text/markdown; charset=utf-8");
    assert_eq!(
        headers["content-security-policy"],
        "default-src 'none'; sandbox"
    );
    let md = String::from_utf8(md).unwrap();
    assert!(md.contains("\\<script>alert(1)\\</script> 본문"), "{md}");
    for bad in ["?format=pdf", "?format=html&x=1"] {
        let (status, _, _) = raw_get(app.clone(), &format!("{base}/body{bad}"), &[]).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }

    // Tree and documents: subtree only.
    let (status, tree) = public_json(app.clone(), &format!("{base}/tree")).await;
    assert_eq!(status, StatusCode::OK);
    let mut tree_ids = ids(&tree);
    tree_ids.sort();
    let mut expected = vec![root.clone(), child.clone(), grandchild.clone()];
    expected.sort();
    assert_eq!(tree_ids, expected);
    for (doc, want) in [
        (&child, StatusCode::OK),
        (&grandchild, StatusCode::OK),
        (&sibling, StatusCode::NOT_FOUND),
        (&sibling_child, StatusCode::NOT_FOUND),
    ] {
        let (status, _, _) = raw_get(
            app.clone(),
            &format!("{base}/documents/{doc}?format=fragment"),
            &[],
        )
        .await;
        assert_eq!(status, want, "document {doc}");
    }
    let (status, _, _) = raw_get(
        app.clone(),
        &format!("{base}/documents/{}", Uuid::now_v7()),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Attachments: only stored, clean attachments of visible documents.
    let visible_att = upload_attachment(&app, &cookie, ws, &child, b"shared bytes").await;
    let sibling_att = insert_attachment(&admin, ws, &sibling, owner_id, "stored", "clean").await;
    let infected = insert_attachment(&admin, ws, &child, owner_id, "stored", "infected").await;
    let uploading = insert_attachment(&admin, ws, &child, owner_id, "uploading", "skipped").await;
    let (status, att) =
        public_json(app.clone(), &format!("{base}/attachments/{visible_att}")).await;
    assert_eq!(status, StatusCode::OK, "{att}");
    assert_eq!(att["name"], "공유 첨부.txt");
    assert_eq!(att["sizeBytes"], 12);
    let (status, headers, bytes) = raw_get(
        app.clone(),
        &format!("{base}/attachments/{visible_att}/download"),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, b"shared bytes");
    assert_eq!(headers["content-length"], "12");
    assert!(headers["content-disposition"]
        .to_str()
        .unwrap()
        .starts_with("attachment;"));
    assert_eq!(headers["content-security-policy"], "sandbox");
    assert_eq!(headers["x-content-type-options"], "nosniff");
    assert_eq!(headers["content-type"], "application/octet-stream");
    for id in [sibling_att, infected, uploading, Uuid::now_v7()] {
        let (status, _) = public_json(app.clone(), &format!("{base}/attachments/{id}")).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "meta {id}");
        let (status, _, _) = raw_get(
            app.clone(),
            &format!("{base}/attachments/{id}/download"),
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "download {id}");
    }
    let (status, _, _) = raw_get(
        app.clone(),
        &format!("{base}/attachments/{visible_att}/download?variant=preview"),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = raw_get(
        app.clone(),
        &format!("{base}/attachments/{visible_att}/download?variant=x"),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // PDF (helper-backed, see share_pdf_* tests): invalid query is 400.
    let (status, _, _) = raw_get(app.clone(), &format!("{base}/pdf?documentId=nope"), &[]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Search: validation first, then 503 without Meili (never a leak).
    for bad in ["", "?q=", "?q=a&x=1"] {
        let (status, _) = public_json(app.clone(), &format!("{base}/search{bad}")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }
    let long = "가".repeat(201);
    let (status, _) = public_json(app.clone(), &format!("{base}/search?q={long}")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, body) = public_json(app.clone(), &format!("{base}/search?q=%E3%84%B1")).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "single chosung is an empty result: {body}"
    );
    assert_eq!(body["items"], json!([]));
    let (status, body) = public_json(app.clone(), &format!("{base}/search?q=root")).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    let (status, _) = public_json(
        app.clone(),
        &format!("/api/v1/share/{}/search?q=root", "x".repeat(43)),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // PG hydrate is the boundary: sibling docs and any task are dropped for a document share.
    let pool = app_pool(&harness).await;
    let (_, rows) = hydrate_share_hits(
        &pool,
        &token,
        &[
            Uuid::parse_str(&root).unwrap(),
            Uuid::parse_str(&sibling).unwrap(),
            Uuid::parse_str(&sibling_child).unwrap(),
        ],
        &[Uuid::now_v7()],
    )
    .await
    .unwrap()
    .expect("share resolves");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, Uuid::parse_str(&root).unwrap());

    // Trashing the child (with its subtree) removes it and the grandchild.
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/documents/{child}/trash"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, tree) = public_json(app.clone(), &format!("{base}/tree")).await;
    assert_eq!(ids(&tree), vec![root.clone()]);
    let (status, _, _) = raw_get(app.clone(), &format!("{base}/documents/{grandchild}"), &[]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = public_json(app.clone(), &format!("{base}/attachments/{visible_att}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "attachment of trashed doc");

    // A live descendant under a trashed ancestor stays hidden even if the
    // ancestor alone is marked deleted.
    let g2 = create_wiki_doc(&app, &cookie, ws, Some(&root), "둘째").await;
    let g2_child = create_wiki_doc(&app, &cookie, ws, Some(&g2), "둘째의 자식").await;
    sqlx::query("UPDATE fvoci.documents SET deleted_at = now() WHERE id = $1")
        .bind(Uuid::parse_str(&g2).unwrap())
        .execute(&admin)
        .await
        .unwrap();
    let (status, _, _) = raw_get(app.clone(), &format!("{base}/documents/{g2_child}"), &[]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Trashing the root makes every public route 404.
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/documents/{root}/trash"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    for suffix in ["", "/body", "/tree", "/search?q=root", "/pdf"] {
        let (status, _, _) = raw_get(app.clone(), &format!("{base}{suffix}"), &[]).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "trashed root {suffix}");
    }
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/documents/{root}/restore"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = public_json(app.clone(), &base).await;
    assert_eq!(status, StatusCode::OK, "restored root is shared again");

    // Expiry.
    sqlx::query("UPDATE fvoci.share_links SET expires_at = now() - interval '1 second'")
        .execute(&admin)
        .await
        .unwrap();
    let (status, _) = public_json(app.clone(), &base).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "expired");
    sqlx::query("UPDATE fvoci.share_links SET expires_at = now() + interval '1 day'")
        .execute(&admin)
        .await
        .unwrap();
    let (status, _) = public_json(app.clone(), &base).await;
    assert_eq!(status, StatusCode::OK);

    // Workspace deletion.
    sqlx::query("UPDATE fvoci.workspaces SET deleted_at = now() WHERE id = $1")
        .bind(ws)
        .execute(&admin)
        .await
        .unwrap();
    let (status, _) = public_json(app.clone(), &base).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "deleted workspace");
    sqlx::query("UPDATE fvoci.workspaces SET deleted_at = NULL WHERE id = $1")
        .bind(ws)
        .execute(&admin)
        .await
        .unwrap();

    // Revocation.
    let (status, _) = json_request(
        app.clone(),
        "DELETE",
        &format!(
            "/api/v1/workspaces/{ws}/share-links/{}",
            created["id"].as_str().unwrap()
        ),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = public_json(app.clone(), &base).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "revoked");
    assert!(hydrate_share_hits(&pool, &token, &[], &[])
        .await
        .unwrap()
        .is_none());

    for bad in ["x", &"a".repeat(300)] {
        let (status, headers, _) = raw_get(app.clone(), &format!("/api/v1/share/{bad}"), &[]).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(headers["referrer-policy"], "no-referrer", "errors too");
    }
    pool.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn share_link_management_permissions_and_validation() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _owner_id, ws) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, ws, "member", "share-member").await;
    let other = add_workspace_user(&admin, ws, "member", "share-other").await;
    let guest = add_workspace_user(&admin, ws, "guest", "share-guest").await;
    let workspace_admin = add_workspace_user(&admin, ws, "admin", "share-admin").await;
    let doc = create_wiki_doc(&app, &cookie, ws, None, "관리 문서").await;
    let project = create_project(app.clone(), &cookie, ws, "SHP", "workspace").await;
    let project_id = project["id"].as_str().unwrap();
    let project_root = project["rootDocumentId"].as_str().unwrap();
    let links = format!("/api/v1/workspaces/{ws}/share-links");

    let (owner_link, _) = share_document(&app, &cookie, ws, &doc).await;
    let (member_link, _) = share_document(&app, &member.cookie, ws, &doc).await;

    // Listing: owner/admin see all, members only their own.
    let (_, all) = json_request(app.clone(), "GET", &links, None, Some(&cookie)).await;
    assert_eq!(all["items"].as_array().unwrap().len(), 2);
    let (_, by_admin) = json_request(
        app.clone(),
        "GET",
        &links,
        None,
        Some(&workspace_admin.cookie),
    )
    .await;
    assert_eq!(by_admin["items"].as_array().unwrap().len(), 2);
    let (_, own) = json_request(app.clone(), "GET", &links, None, Some(&member.cookie)).await;
    assert_eq!(
        ids(&own),
        vec![member_link["id"].as_str().unwrap().to_string()]
    );
    let (_, none) = json_request(app.clone(), "GET", &links, None, Some(&other.cookie)).await;
    assert_eq!(none["items"], json!([]));
    let (status, doc_links) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{ws}/documents/{doc}/share-links"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(doc_links["items"].as_array().unwrap().len(), 2);

    // Revoke: another plain member cannot; the creator and admins can.
    let (status, _) = json_request(
        app.clone(),
        "DELETE",
        &format!("{links}/{}", member_link["id"].as_str().unwrap()),
        None,
        Some(&other.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = json_request(
        app.clone(),
        "DELETE",
        &format!("{links}/{}", owner_link["id"].as_str().unwrap()),
        None,
        Some(&workspace_admin.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = json_request(
        app.clone(),
        "DELETE",
        &format!("{links}/{}", member_link["id"].as_str().unwrap()),
        None,
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Guests without a grant cannot create or list document links.
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/documents/{doc}/share-links"),
        Some(json!({})),
        Some(&guest.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{ws}/documents/{doc}/share-links"),
        None,
        Some(&guest.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &links,
        Some(json!({"projectId": project_id})),
        Some(&guest.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Validation.
    for bad in [
        json!({}),
        json!({"documentId": doc, "projectId": project_id}),
        json!({"documentId": doc, "expiresInDays": 0}),
        json!({"documentId": doc, "expiresInDays": 366}),
        json!({"documentId": doc, "expiresInDays": 1.5}),
        json!({"documentId": doc, "expiresInDays": null}),
        json!({"documentId": doc, "unknown": true}),
        json!({"documentId": "nope"}),
    ] {
        let (status, body) = json_request(
            app.clone(),
            "POST",
            &links,
            Some(bad.clone()),
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad} -> {body}");
    }
    let (status, custom) = json_request(
        app.clone(),
        "POST",
        &links,
        Some(json!({"documentId": doc, "expiresInDays": 365})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{custom}");

    // Affiliation: wiki paths refuse project docs and vice versa.
    let (status, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{ws}/documents/{project_root}/share-links"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/projects/{project_id}/documents/{doc}/share-links"),
        Some(json!({})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, project_doc_link) = json_request(
        app.clone(),
        "POST",
        &format!(
            "/api/v1/workspaces/{ws}/projects/{project_id}/documents/{project_root}/share-links"
        ),
        Some(json!({"expiresInDays": 3})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{project_doc_link}");
    assert_eq!(project_doc_link["documentId"], project_root);
    let project_doc_token = project_doc_link["url"]
        .as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_string();
    let (status, meta) =
        public_json(app.clone(), &format!("/api/v1/share/{project_doc_token}")).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "project document share resolves: {meta}"
    );
    assert_eq!(meta["documentId"], project_root);
    assert_eq!(meta["projectId"], Value::Null);
    let (status, tree) = public_json(
        app.clone(),
        &format!("/api/v1/share/{project_doc_token}/tree"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ids(&tree), vec![project_root.to_string()]);
    let (status, _, _) = raw_get(
        app.clone(),
        &format!("/api/v1/share/{project_doc_token}/documents/{doc}"),
        &[],
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "wiki doc outside a project doc share"
    );

    // API tokens need share.manage.
    let docs_token = create_api_token(&app, &cookie, ws, &["documents.write"]).await;
    let (status, _) = bearer(app.clone(), "GET", &links, None, &docs_token).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let share_token = create_api_token(&app, &cookie, ws, &["share.manage"]).await;
    let (status, listed) = bearer(app.clone(), "GET", &links, None, &share_token).await;
    assert_eq!(status, StatusCode::OK, "{listed}");

    // Removing the creator's membership deletes their links (source FK cascade).
    let (_, other_token) = share_document(&app, &other.cookie, ws, &doc).await;
    let (status, _) = public_json(app.clone(), &format!("/api/v1/share/{other_token}")).await;
    assert_eq!(status, StatusCode::OK);
    sqlx::query("DELETE FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2")
        .bind(ws)
        .bind(other.user_id)
        .execute(&admin)
        .await
        .unwrap();
    let (status, _) = public_json(app.clone(), &format!("/api/v1/share/{other_token}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn project_share_covers_project_subtree_and_tasks_only() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _owner_id, ws) = setup_session(&harness).await;
    let project = create_project(app.clone(), &cookie, ws, "PSH", "private").await;
    let other_project = create_project(app.clone(), &cookie, ws, "OTH", "workspace").await;
    let project_id = project["id"].as_str().unwrap();
    let root = project["rootDocumentId"].as_str().unwrap().to_string();
    let (status, child) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/projects/{project_id}/documents"),
        Some(json!({"parentId": root, "title": "프로젝트 하위"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{child}");
    let child = child["id"].as_str().unwrap().to_string();
    let wiki = create_wiki_doc(&app, &cookie, ws, None, "위키").await;
    let task = create_task(&app, &cookie, ws, project_id, "프로젝트 태스크").await;
    let foreign_task = create_task(
        &app,
        &cookie,
        ws,
        other_project["id"].as_str().unwrap(),
        "다른 태스크",
    )
    .await;

    let (status, link) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/share-links"),
        Some(json!({"projectId": project_id})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{link}");
    assert_eq!(link["projectId"], project_id);
    assert_eq!(link["documentId"], Value::Null);
    let token = link["url"]
        .as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_string();
    let base = format!("/api/v1/share/{token}");

    let (status, meta) = public_json(app.clone(), &base).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(meta["title"], "PSH");
    assert_eq!(meta["documentId"], root.as_str());
    assert_eq!(meta["projectId"], project_id);
    let (_, tree) = public_json(app.clone(), &format!("{base}/tree")).await;
    let mut got = ids(&tree);
    got.sort();
    let mut want = vec![root.clone(), child.clone()];
    want.sort();
    assert_eq!(got, want);
    let (status, _, _) = raw_get(app.clone(), &format!("{base}/documents/{wiki}"), &[]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let admin = admin_pool(&harness).await;
    sqlx::query("UPDATE fvoci.tasks SET content_json = $2 WHERE id = $1")
        .bind(Uuid::parse_str(&task).unwrap())
        .bind(json!({"type": "doc", "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": "과제 설명 본문"}]}
        ]}))
        .execute(&admin)
        .await
        .unwrap();
    let pool = app_pool(&harness).await;
    let (_, rows) = hydrate_share_hits(
        &pool,
        &token,
        &[
            Uuid::parse_str(&child).unwrap(),
            Uuid::parse_str(&wiki).unwrap(),
        ],
        &[
            Uuid::parse_str(&task).unwrap(),
            Uuid::parse_str(&foreign_task).unwrap(),
        ],
    )
    .await
    .unwrap()
    .unwrap();
    let mut got: Vec<(bool, Uuid)> = rows.iter().map(|r| (r.is_task, r.id)).collect();
    got.sort();
    let mut want = vec![
        (false, Uuid::parse_str(&child).unwrap()),
        (true, Uuid::parse_str(&task).unwrap()),
    ];
    want.sort();
    assert_eq!(got, want);
    let task_row = rows.iter().find(|r| r.is_task).unwrap();
    assert_eq!(
        task_row.body, "과제 설명 본문",
        "snippet body is the task text"
    );

    // Deleting the project ends the share.
    sqlx::query("UPDATE fvoci.projects SET deleted_at = now() WHERE id = $1")
        .bind(Uuid::parse_str(project_id).unwrap())
        .execute(&admin)
        .await
        .unwrap();
    let (status, _) = public_json(app.clone(), &base).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    pool.close().await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn share_rate_limit_is_per_ip() {
    let harness = TestDb::bootstrap().await;
    let (app, _cookie, _owner_id, _ws) = setup_session(&harness).await;
    for i in 0..60 {
        let (status, _) = public_json(app.clone(), "/api/v1/share/unknown-token").await;
        assert_eq!(status, StatusCode::NOT_FOUND, "request {i}");
    }
    let (status, headers, _) = raw_get(app.clone(), "/api/v1/share/unknown-token/tree", &[]).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert!(headers.contains_key("retry-after"));
    harness.cleanup().await;
}

#[tokio::test]
async fn app_role_rls_isolates_stars_and_share_links() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, ws_a) = setup_session(&harness).await;
    let (status, created) = json_request(
        app.clone(),
        "POST",
        "/api/v1/workspaces",
        Some(json!({"name": "Beta", "slug": "beta-ws"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let ws_b = Uuid::parse_str(created["id"].as_str().unwrap()).unwrap();
    let doc = create_wiki_doc(&app, &cookie, ws_a, None, "A 문서").await;
    let (_, token) = share_document(&app, &cookie, ws_a, &doc).await;
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws_a}/stars"),
        Some(json!({"type": "document", "id": doc})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let pool = app_pool(&harness).await;
    let role: (bool, bool) =
        sqlx::query_as("SELECT rolsuper, rolbypassrls FROM pg_roles WHERE rolname = current_user")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(role, (false, false), "tests run as the real app role");

    // Tenant B sees nothing of tenant A.
    let mut tx = pool.begin().await.unwrap();
    set_tenant(&mut tx, ws_b).await.unwrap();
    let links: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.share_links")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let stars: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.stars")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!((links, stars), (0, 0));
    let resolved: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM fvoci.app_share_link_by_token_hash($1)")
            .bind(hash_token(&token))
            .fetch_optional(&mut *tx)
            .await
            .unwrap();
    assert!(
        resolved.is_none(),
        "definer lookup needs the system context"
    );
    let insert = sqlx::query(
        "INSERT INTO fvoci.stars (id, workspace_id, user_id, document_id) VALUES ($1, $2, $3, $4)",
    )
    .bind(Uuid::now_v7())
    .bind(ws_a)
    .bind(owner_id)
    .bind(Uuid::parse_str(&doc).unwrap())
    .execute(&mut *tx)
    .await;
    assert!(insert.is_err(), "WITH CHECK blocks cross-tenant insert");
    tx.rollback().await.unwrap();

    // Tenant A sees its rows but never the token hash column.
    let mut tx = pool.begin().await.unwrap();
    set_tenant(&mut tx, ws_a).await.unwrap();
    let links: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.share_links")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(links, 1);
    let err = sqlx::query("SELECT token_hash FROM fvoci.share_links")
        .fetch_all(&mut *tx)
        .await
        .expect_err("token_hash is not readable by the app role");
    assert!(err.to_string().contains("permission denied"), "{err}");
    tx.rollback().await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    set_tenant(&mut tx, ws_a).await.unwrap();
    let err = sqlx::query("UPDATE fvoci.share_links SET expires_at = now() + interval '9 years'")
        .execute(&mut *tx)
        .await
        .expect_err("share links are not updatable");
    assert!(err.to_string().contains("permission denied"), "{err}");
    tx.rollback().await.unwrap();

    // With the system context the definer lookup resolves exactly one hash.
    let mut tx = pool.begin().await.unwrap();
    set_system(&mut tx).await.unwrap();
    let resolved: Option<(Uuid, Uuid)> =
        sqlx::query_as("SELECT id, workspace_id FROM fvoci.app_share_link_by_token_hash($1)")
            .bind(hash_token(&token))
            .fetch_optional(&mut *tx)
            .await
            .unwrap();
    assert_eq!(resolved.map(|r| r.1), Some(ws_a));
    let wrong: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM fvoci.app_share_link_by_token_hash($1)")
            .bind(hash_token("not-the-token"))
            .fetch_optional(&mut *tx)
            .await
            .unwrap();
    assert!(wrong.is_none());
    // The system-context branch is SELECT-only: no cross-tenant delete.
    let deleted = sqlx::query("DELETE FROM fvoci.share_links")
        .execute(&mut *tx)
        .await
        .unwrap()
        .rows_affected();
    assert_eq!(deleted, 0, "system context cannot delete share links");
    tx.rollback().await.unwrap();

    let admin = admin_pool(&harness).await;
    let flags: Vec<(String, bool, bool)> = sqlx::query_as(
        r#"
        SELECT c.relname, c.relrowsecurity, c.relforcerowsecurity
        FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
        WHERE n.nspname = 'fvoci' AND c.relname IN ('stars', 'share_links')
        ORDER BY c.relname
        "#,
    )
    .fetch_all(&admin)
    .await
    .unwrap();
    assert_eq!(
        flags,
        vec![
            ("share_links".to_string(), true, true),
            ("stars".to_string(), true, true)
        ]
    );
    let public_exec: bool = sqlx::query_scalar(
        "SELECT has_function_privilege('public', 'fvoci.app_share_link_by_token_hash(text)', 'EXECUTE')",
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert!(!public_exec, "definer EXECUTE is revoked from PUBLIC");
    pool.close().await;
    admin.close().await;
    harness.cleanup().await;
}

async fn star(app: &axum::Router, cookie: &str, ws: Uuid, kind: &str, id: &str) -> StatusCode {
    json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/stars"),
        Some(json!({"type": kind, "id": id})),
        Some(cookie),
    )
    .await
    .0
}

async fn star_targets(app: &axum::Router, cookie: &str, ws: Uuid) -> Vec<String> {
    let (status, body) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{ws}/stars"),
        None,
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["targetId"].as_str().unwrap().to_string())
        .collect()
}

async fn recent_ids(app: &axum::Router, cookie: &str, ws: Uuid) -> Vec<String> {
    let (status, body) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{ws}/recent?limit=50"),
        None,
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    ids(&body)
}

#[tokio::test]
async fn stars_and_recent_gate_on_current_access() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _owner_id, ws) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, ws, "member", "gate-member").await;
    let guest = add_workspace_user(&admin, ws, "guest", "gate-guest").await;

    // 1. Revoked project access hides a starred private-project task.
    let private = create_project(app.clone(), &cookie, ws, "GPR", "private").await;
    let private_id = private["id"].as_str().unwrap();
    let private_task = create_task(&app, &cookie, ws, private_id, "비공개 과제").await;
    let (status, added) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/projects/{private_id}/members"),
        Some(json!({"userId": member.user_id, "role": "member"})),
        Some(&cookie),
    )
    .await;
    assert!(status.is_success(), "{added}");
    assert_eq!(
        star(&app, &member.cookie, ws, "task", &private_task).await,
        StatusCode::CREATED
    );
    assert_eq!(
        star_targets(&app, &member.cookie, ws).await,
        vec![private_task.clone()]
    );
    assert!(recent_ids(&app, &member.cookie, ws)
        .await
        .contains(&private_task));
    let (status, removed) = json_request(
        app.clone(),
        "DELETE",
        &format!(
            "/api/v1/workspaces/{ws}/projects/{private_id}/members/{}",
            member.user_id
        ),
        None,
        Some(&cookie),
    )
    .await;
    assert!(status.is_success(), "{removed}");
    let (status, body) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{ws}/stars"),
        None,
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["items"], json!([]));
    assert!(
        !body.to_string().contains("비공개 과제"),
        "title leaked: {body}"
    );
    let recent = recent_ids(&app, &member.cookie, ws).await;
    assert!(!recent.contains(&private_task), "{recent:?}");
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.stars WHERE user_id = $1")
        .bind(member.user_id)
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(rows, 1, "row kept, only hidden");

    // 2. Guest: no wiki access without a grant, access with a group grant.
    let wiki = create_wiki_doc(&app, &cookie, ws, None, "손님 문서").await;
    assert_eq!(
        star(&app, &guest.cookie, ws, "document", &wiki).await,
        StatusCode::NOT_FOUND
    );
    assert!(!recent_ids(&app, &guest.cookie, ws).await.contains(&wiki));
    let group_id = Uuid::now_v7();
    sqlx::query("INSERT INTO fvoci.groups (id, workspace_id, name) VALUES ($1, $2, 'guests')")
        .bind(group_id)
        .bind(ws)
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO fvoci.group_members (workspace_id, group_id, user_id) VALUES ($1, $2, $3)",
    )
    .bind(ws)
    .bind(group_id)
    .bind(guest.user_id)
    .execute(&admin)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO fvoci.document_members (id, workspace_id, document_id, group_id, role)
         VALUES ($1, $2, $3, $4, 'viewer')",
    )
    .bind(Uuid::now_v7())
    .bind(ws)
    .bind(Uuid::parse_str(&wiki).unwrap())
    .bind(group_id)
    .execute(&admin)
    .await
    .unwrap();
    assert!(recent_ids(&app, &guest.cookie, ws).await.contains(&wiki));
    assert_eq!(
        star(&app, &guest.cookie, ws, "document", &wiki).await,
        StatusCode::CREATED
    );
    assert_eq!(
        star_targets(&app, &guest.cookie, ws).await,
        vec![wiki.clone()]
    );
    sqlx::query("DELETE FROM fvoci.group_members WHERE group_id = $1 AND user_id = $2")
        .bind(group_id)
        .bind(guest.user_id)
        .execute(&admin)
        .await
        .unwrap();
    assert!(star_targets(&app, &guest.cookie, ws).await.is_empty());
    assert!(!recent_ids(&app, &guest.cookie, ws).await.contains(&wiki));

    // 3. A soft-deleted project hides its documents and tasks.
    let open = create_project(app.clone(), &cookie, ws, "GOP", "workspace").await;
    let open_id = open["id"].as_str().unwrap();
    let open_root = open["rootDocumentId"].as_str().unwrap().to_string();
    let open_task = create_task(&app, &cookie, ws, open_id, "열린 과제").await;
    assert_eq!(
        star(&app, &cookie, ws, "task", &open_task).await,
        StatusCode::CREATED
    );
    assert_eq!(
        star(&app, &cookie, ws, "document", &open_root).await,
        StatusCode::CREATED
    );
    let before = recent_ids(&app, &cookie, ws).await;
    assert!(
        before.contains(&open_task) && before.contains(&open_root),
        "{before:?}"
    );
    sqlx::query("UPDATE fvoci.projects SET deleted_at = now() WHERE id = $1")
        .bind(Uuid::parse_str(open_id).unwrap())
        .execute(&admin)
        .await
        .unwrap();
    let starred = star_targets(&app, &cookie, ws).await;
    assert!(
        !starred.contains(&open_task) && !starred.contains(&open_root),
        "{starred:?}"
    );
    let after = recent_ids(&app, &cookie, ws).await;
    assert!(
        !after.contains(&open_task) && !after.contains(&open_root),
        "{after:?}"
    );

    // 4. An archived task is absent from recent.
    let live = create_project(app.clone(), &cookie, ws, "GLV", "workspace").await;
    let archived = create_task(&app, &cookie, ws, live["id"].as_str().unwrap(), "보관 과제").await;
    assert!(recent_ids(&app, &cookie, ws).await.contains(&archived));
    sqlx::query("UPDATE fvoci.tasks SET archived_at = now() WHERE id = $1")
        .bind(Uuid::parse_str(&archived).unwrap())
        .execute(&admin)
        .await
        .unwrap();
    assert!(!recent_ids(&app, &cookie, ws).await.contains(&archived));

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn share_pdf_renders_shared_scope_through_convert_helper() {
    let convert = fvoci_server::documents::convert::ConvertClient::from_env().expect(
        "FVOCI_DOCUMENT_CONVERT_BIN is required; run scripts/prepare-document-convert.sh first",
    );
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _owner_id, ws) = setup_session_with_convert(&harness, Some(convert)).await;
    let admin = admin_pool(&harness).await;
    let root = create_wiki_doc(&app, &cookie, ws, None, "공유 PDF").await;
    let child = create_wiki_doc(&app, &cookie, ws, Some(&root), "하위 PDF").await;
    let sibling = create_wiki_doc(&app, &cookie, ws, None, "형제").await;
    set_content(
        &admin,
        &root,
        json!({"type": "doc", "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": "한글 본문 PDF"}]}
        ]}),
    )
    .await;
    let (_, token) = share_document(&app, &cookie, ws, &root).await;
    let base = format!("/api/v1/share/{token}/pdf");

    let (status, headers, bytes) = raw_get(app.clone(), &base, &[]).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    assert_eq!(headers["content-type"], "application/pdf");
    assert_eq!(headers["cache-control"], "private, no-store");
    assert_eq!(headers["x-content-type-options"], "nosniff");
    assert_eq!(headers["referrer-policy"], "no-referrer");
    let disposition = headers["content-disposition"].to_str().unwrap();
    assert!(disposition.starts_with("attachment;"), "{disposition}");
    assert!(
        disposition.contains("filename*=UTF-8''%EA%B3%B5%EC%9C%A0%20PDF.pdf"),
        "{disposition}"
    );
    assert!(bytes.starts_with(b"%PDF-"), "not a PDF");

    let (status, headers, bytes) =
        raw_get(app.clone(), &format!("{base}?documentId={child}"), &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(bytes.starts_with(b"%PDF-"));
    assert!(headers["content-disposition"]
        .to_str()
        .unwrap()
        .contains("%ED%95%98%EC%9C%84%20PDF.pdf"));

    for (query, want) in [
        (format!("?documentId={sibling}"), StatusCode::NOT_FOUND),
        (
            format!("?documentId={}", Uuid::now_v7()),
            StatusCode::NOT_FOUND,
        ),
        ("?documentId=nope".to_string(), StatusCode::BAD_REQUEST),
        ("?other=1".to_string(), StatusCode::BAD_REQUEST),
    ] {
        let (status, _, _) = raw_get(app.clone(), &format!("{base}{query}"), &[]).await;
        assert_eq!(status, want, "{query}");
    }
    let (status, _, _) = raw_get(
        app.clone(),
        &format!("/api/v1/share/{}/pdf", "x".repeat(43)),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Trashed root and revocation both end PDF access.
    sqlx::query("UPDATE fvoci.documents SET deleted_at = now() WHERE id = $1")
        .bind(Uuid::parse_str(&root).unwrap())
        .execute(&admin)
        .await
        .unwrap();
    let (status, _, _) = raw_get(app.clone(), &base, &[]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    sqlx::query("UPDATE fvoci.documents SET deleted_at = NULL WHERE id = $1")
        .bind(Uuid::parse_str(&root).unwrap())
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query("DELETE FROM fvoci.share_links")
        .execute(&admin)
        .await
        .unwrap();
    let (status, _, _) = raw_get(app.clone(), &base, &[]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn share_pdf_without_convert_helper_is_a_server_error_after_scope_checks() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _owner_id, ws) = setup_session(&harness).await;
    let root = create_wiki_doc(&app, &cookie, ws, None, "헬퍼 없음").await;
    let (_, token) = share_document(&app, &cookie, ws, &root).await;
    // Scope checks still run first: an unknown token is 404, not 500.
    let (status, _, _) = raw_get(
        app.clone(),
        &format!("/api/v1/share/{}/pdf", "x".repeat(43)),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = raw_get(app.clone(), &format!("/api/v1/share/{token}/pdf"), &[]).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    harness.cleanup().await;
}

const SHELL_INDEX: &str = "<!doctype html>\n<html lang=\"ko\"><head><meta charset=\"UTF-8\" /><title>FVOCI</title><script>document.documentElement.dataset.theme = \"light\";</script></head><body><div id=\"root\"></div><script type=\"module\" src=\"/assets/index.js\"></script></body></html>";

fn body_part(html: &str) -> &str {
    &html[html.find("<body").unwrap()..]
}

fn meta_content<'a>(html: &'a str, attr: &str) -> Option<&'a str> {
    let marker = format!("{attr} content=\"");
    let start = html.find(&marker)? + marker.len();
    let end = html[start..].find('"')?;
    Some(&html[start..start + end])
}

/// Source server.ts `/s/:token` SPA fallback + spa-html.ts `injectShareOg`:
/// the shell gets the share's title and OG tags (escaped, one line, excerpt
/// cut at 200 UTF-16 units), is never cached and not indexed; invalid tokens,
/// a disabled share policy and a caller over the share-ip limit get the plain
/// shell without telling whether the token exists.
#[tokio::test]
async fn share_shell_head_carries_escaped_og_meta_only_for_live_shares() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _owner_id, ws) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let static_dir = std::env::temp_dir().join(format!("fvoci-share-shell-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&static_dir).unwrap();
    std::fs::write(static_dir.join("index.html"), SHELL_INDEX).unwrap();
    let shell_app = |dir: &std::path::Path| {
        let dir = dir.to_path_buf();
        let url = harness.app_url.clone();
        async move { fvoci_server::http::router(project_harness::app_state(&url).await, Some(dir)) }
    };
    let site = shell_app(&static_dir).await;

    let doc = create_wiki_doc(&app, &cookie, ws, None, "공유 제목").await;
    // Stored titles may carry markup-looking text, bidi controls and newlines.
    sqlx::query("UPDATE fvoci.documents SET title = $2 WHERE id = $1")
        .bind(Uuid::parse_str(&doc).unwrap())
        .bind("제목 <b>\"따옴표\"</b> & \u{202E}역순\n둘째 줄")
        .execute(&admin)
        .await
        .unwrap();
    let long = "가".repeat(168);
    set_content(
        &admin,
        &doc,
        json!({"type": "doc", "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": "<script>alert(1)</script> \"본문\""}]},
            {"type": "paragraph", "content": [{"type": "text", "text": format!("{long}😀끝")}]}
        ]}),
    )
    .await;
    let (_, token) = share_document(&app, &cookie, ws, &doc).await;

    let (status, headers, bytes) = raw_get(site.clone(), &format!("/s/{token}"), &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["content-type"], "text/html; charset=utf-8");
    assert_eq!(headers["cache-control"], "private, no-store");
    assert_eq!(headers["x-robots-tag"], "noindex");
    assert_eq!(headers["referrer-policy"], "no-referrer");
    assert!(!headers.contains_key("content-security-policy"));
    let html = String::from_utf8(bytes).unwrap();
    assert_eq!(body_part(&html), body_part(SHELL_INDEX), "body untouched");
    // Inline blocks are unchanged, so build-time CSP hashes of the shell hold.
    assert!(html.contains("<script>document.documentElement.dataset.theme = \"light\";</script>"));
    assert_eq!(
        html.matches("<script").count(),
        SHELL_INDEX.matches("<script").count()
    );
    let title = "제목 &lt;b&gt;&quot;따옴표&quot;&lt;/b&gt; &amp; 역순 둘째 줄";
    assert!(html.contains(&format!("<title>{title}</title>")), "{html}");
    assert_eq!(meta_content(&html, "property=\"og:title\""), Some(title));
    assert_eq!(
        meta_content(&html, "property=\"og:url\""),
        Some(format!("http://localhost/s/{token}").as_str())
    );
    assert_eq!(meta_content(&html, "property=\"og:type\""), Some("article"));
    assert_eq!(
        meta_content(&html, "name=\"twitter:card\""),
        Some("summary")
    );
    let description = meta_content(&html, "property=\"og:description\"").unwrap();
    let expected_start = "&lt;script&gt;alert(1)&lt;/script&gt; &quot;본문&quot; ";
    assert!(description.starts_with(expected_start), "{description}");
    // 200 UTF-16 units: 30 of the first paragraph, one joining space, 168 of
    // the long one (199); the emoji pair would cross 200 and is not split.
    let raw_description = description
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"");
    assert_eq!(raw_description.encode_utf16().count(), 199);
    assert!(raw_description.ends_with('가'));
    assert!(!raw_description.contains('\u{FFFD}'));
    assert!(!html.contains("<script>alert"));

    // Trailing slash is the same root; sub-paths are not.
    let (_, _, bytes) = raw_get(site.clone(), &format!("/s/{token}/"), &[]).await;
    assert!(String::from_utf8(bytes).unwrap().contains("og:title"));
    let (status, headers, bytes) =
        raw_get(site.clone(), &format!("/s/{token}/attachments/x"), &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!headers.contains_key("x-robots-tag"));
    assert_eq!(bytes, SHELL_INDEX.as_bytes());

    // An unknown token: the plain shell with the same private headers.
    let (status, headers, bytes) = raw_get(site.clone(), "/s/not-a-token", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["x-robots-tag"], "noindex");
    assert_eq!(headers["cache-control"], "private, no-store");
    assert_eq!(bytes, SHELL_INDEX.as_bytes());

    // A project share: project name, empty description.
    let project = create_project(app.clone(), &cookie, ws, "OGP", "workspace").await;
    let (status, link) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/share-links"),
        Some(json!({"projectId": project["id"]})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{link}");
    let project_token = link["url"]
        .as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_string();
    let (_, _, bytes) = raw_get(site.clone(), &format!("/s/{project_token}"), &[]).await;
    let html = String::from_utf8(bytes).unwrap();
    assert_eq!(meta_content(&html, "property=\"og:title\""), Some("OGP"));
    assert_eq!(meta_content(&html, "property=\"og:description\""), Some(""));

    // Revoked or expired shares stop injecting.
    sqlx::query("UPDATE fvoci.share_links SET expires_at = now() - interval '1 second' WHERE token_hash = $1")
        .bind(hash_token(&project_token))
        .execute(&admin)
        .await
        .unwrap();
    let (_, _, bytes) = raw_get(site.clone(), &format!("/s/{project_token}"), &[]).await;
    assert_eq!(bytes, SHELL_INDEX.as_bytes());

    // Sharing disabled by the instance policy: nothing is injected.
    let (status, body) = json_request(
        app.clone(),
        "PATCH",
        "/api/v1/admin/instance-settings",
        Some(json!({"share": {"enabled": false, "defaultExpiresDays": 7, "maxExpiresDays": 365}})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, headers, bytes) = raw_get(site.clone(), &format!("/s/{token}"), &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["x-robots-tag"], "noindex");
    assert_eq!(bytes, SHELL_INDEX.as_bytes());
    let (status, body) = json_request(
        app.clone(),
        "PATCH",
        "/api/v1/admin/instance-settings",
        Some(json!({"share": null})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Over the share-ip limit the shell is still served, without meta or 429.
    let limited = shell_app(&static_dir).await;
    for i in 0..60 {
        let (status, _) = public_json(limited.clone(), "/api/v1/share/unknown-token").await;
        assert_eq!(status, StatusCode::NOT_FOUND, "request {i}");
    }
    let (status, headers, bytes) = raw_get(limited.clone(), &format!("/s/{token}"), &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!headers.contains_key("retry-after"));
    assert_eq!(bytes, SHELL_INDEX.as_bytes());
    // The fresh app (own limiter) still injects after the policy reset.
    let (_, _, bytes) = raw_get(site.clone(), &format!("/s/{token}"), &[]).await;
    assert!(String::from_utf8(bytes).unwrap().contains("og:title"));

    let _ = std::fs::remove_dir_all(&static_dir);
    admin.close().await;
    harness.cleanup().await;
}
