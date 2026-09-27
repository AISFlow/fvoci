#![cfg(feature = "db-tests")]
#![allow(dead_code)]

//! Attachments on every source parent (wiki document, project document,
//! task) with the parent-permission model, delete + object journal, the HWP
//! edit-copy pair, API-token target scopes and the readers that used to
//! assume a document parent (share, extract claim, search index).
//! Everything runs as the non-superuser app role against real PostgreSQL.

#[path = "support/project_harness.rs"]
mod project_harness;

#[path = "support/license.rs"]
mod license_fixture;

use axum::http::StatusCode;
use project_harness::{
    add_workspace_user, admin_pool, app_state, create_project, http_request, json_request,
    setup_session, TestDb,
};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

const HWPX_NAME: &str = "report.hwpx";

struct Ctx {
    harness: TestDb,
    app: axum::Router,
    cookie: String,
    owner: Uuid,
    ws: Uuid,
    admin: PgPool,
}

async fn ctx() -> Ctx {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner, ws) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    Ctx {
        harness,
        app,
        cookie,
        owner,
        ws,
        admin,
    }
}

impl Ctx {
    async fn done(self) {
        self.admin.close().await;
        self.harness.cleanup().await;
    }
}

async fn create_task(app: &axum::Router, cookie: &str, ws: Uuid, project_id: &str) -> String {
    let (status, body) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/projects/{project_id}/tasks"),
        Some(json!({"title": "Task"})),
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["id"].as_str().unwrap().to_string()
}

async fn create_wiki_document(app: &axum::Router, cookie: &str, ws: Uuid) -> String {
    let (status, body) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/documents"),
        Some(json!({"parentId": null, "title": "Doc"})),
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["id"].as_str().unwrap().to_string()
}

async fn create_project_document(
    app: &axum::Router,
    cookie: &str,
    ws: Uuid,
    project: &Value,
) -> String {
    let project_id = project["id"].as_str().unwrap();
    let (status, body) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/projects/{project_id}/documents"),
        Some(json!({"parentId": project["rootDocumentId"], "title": "Spec"})),
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["id"].as_str().unwrap().to_string()
}

/// Create → PUT every part → complete. Returns the final status and body so
/// callers can assert refusals at the create step.
async fn upload(
    app: &axum::Router,
    cookie: &str,
    ws: Uuid,
    create_path: &str,
    name: &str,
    bytes: &[u8],
) -> (StatusCode, Value) {
    let (status, created) = json_request(
        app.clone(),
        "POST",
        create_path,
        Some(json!({"name": name, "sizeBytes": bytes.len()})),
        Some(cookie),
    )
    .await;
    if status != StatusCode::CREATED {
        return (status, created);
    }
    let attachment_id = created["attachmentId"].as_str().unwrap().to_string();
    let part_size = created["partSizeBytes"].as_u64().unwrap() as usize;
    let mut parts = Vec::new();
    for part in created["parts"].as_array().unwrap() {
        let number = part["partNumber"].as_u64().unwrap() as usize;
        let start = (number - 1) * part_size;
        let end = (start + part_size).min(bytes.len());
        let (status, body, headers) = http_request(
            app.clone(),
            "PUT",
            part["url"].as_str().unwrap(),
            Some(bytes[start..end].to_vec()),
            Some("application/octet-stream"),
            Some(cookie),
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::OK, "part: {body}");
        let etag = headers.get("etag").unwrap().to_str().unwrap().to_string();
        parts.push(json!({"partNumber": number, "etag": etag}));
    }
    json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/attachments/{attachment_id}/complete"),
        Some(json!({"parts": parts})),
        Some(cookie),
    )
    .await
}

fn task_upload_path(ws: Uuid, task_id: &str) -> String {
    format!("/api/v1/workspaces/{ws}/tasks/{task_id}/uploads")
}

async fn add_project_member(admin: &PgPool, ws: Uuid, project_id: &str, user: Uuid, role: &str) {
    sqlx::query(
        "INSERT INTO fvoci.project_members (id, workspace_id, project_id, user_id, role) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(Uuid::now_v7())
    .bind(ws)
    .bind(Uuid::parse_str(project_id).unwrap())
    .bind(user)
    .bind(role)
    .execute(admin)
    .await
    .unwrap();
}

async fn create_api_token(app: &axum::Router, cookie: &str, ws: Uuid, scopes: &[&str]) -> String {
    let (status, body) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{ws}/api-tokens"),
        Some(json!({"name": format!("t-{}", Uuid::now_v7().simple()), "scopes": scopes})),
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["token"].as_str().unwrap().to_string()
}

async fn bearer(app: &axum::Router, method: &str, path: &str, secret: &str) -> StatusCode {
    let auth = format!("Bearer {secret}");
    let (status, _, _) = http_request(
        app.clone(),
        method,
        path,
        None,
        None,
        None,
        &[("authorization", auth.as_str())],
    )
    .await;
    status
}

async fn journal_rows(admin: &PgPool, attachment_id: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM fvoci.attachment_object_cleanups WHERE attachment_id = $1",
    )
    .bind(Uuid::parse_str(attachment_id).unwrap())
    .fetch_one(admin)
    .await
    .unwrap()
}

async fn storage_key(admin: &PgPool, attachment_id: &str) -> String {
    sqlx::query_scalar("SELECT storage_key FROM fvoci.attachments WHERE id = $1")
        .bind(Uuid::parse_str(attachment_id).unwrap())
        .fetch_one(admin)
        .await
        .unwrap()
}

#[tokio::test]
async fn task_attachment_round_trip_list_meta_download_and_event() {
    let c = ctx().await;
    let project = create_project(c.app.clone(), &c.cookie, c.ws, "ATT", "workspace").await;
    let project_id = project["id"].as_str().unwrap();
    let task_id = create_task(&c.app, &c.cookie, c.ws, project_id).await;
    let bytes = b"task attachment bytes".to_vec();
    let (status, stored) = upload(
        &c.app,
        &c.cookie,
        c.ws,
        &task_upload_path(c.ws, &task_id),
        "notes.txt",
        &bytes,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{stored}");
    let att_id = stored["id"].as_str().unwrap().to_string();
    assert_eq!(stored["sizeBytes"], bytes.len());

    let (status, list) = json_request(
        c.app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{}/tasks/{task_id}/attachments", c.ws),
        None,
        Some(&c.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items = list["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], att_id);

    let (status, meta) = json_request(
        c.app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{}/attachments/{att_id}", c.ws),
        None,
        Some(&c.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{meta}");
    let (status, _, headers) = http_request(
        c.app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{}/attachments/{att_id}/download", c.ws),
        None,
        None,
        Some(&c.cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers.get("content-length").unwrap().to_str().unwrap(),
        bytes.len().to_string()
    );

    let (task_col, doc_col): (Option<Uuid>, Option<Uuid>) =
        sqlx::query_as("SELECT task_id, document_id FROM fvoci.attachments WHERE id = $1")
            .bind(Uuid::parse_str(&att_id).unwrap())
            .fetch_one(&c.admin)
            .await
            .unwrap();
    assert_eq!(task_col.unwrap().to_string(), task_id);
    assert!(doc_col.is_none());
    let payload: Value = sqlx::query_scalar(
        "SELECT payload FROM fvoci.events WHERE verb = 'attachment.completed' AND target_id = $1",
    )
    .bind(Uuid::parse_str(&att_id).unwrap())
    .fetch_one(&c.admin)
    .await
    .unwrap();
    assert_eq!(payload["taskId"], task_id);
    assert!(payload["documentId"].is_null());
    c.done().await;
}

#[tokio::test]
async fn task_attachment_follows_project_permission() {
    let c = ctx().await;
    let project = create_project(c.app.clone(), &c.cookie, c.ws, "PRV", "private").await;
    let project_id = project["id"].as_str().unwrap();
    let task_id = create_task(&c.app, &c.cookie, c.ws, project_id).await;
    let (status, stored) = upload(
        &c.app,
        &c.cookie,
        c.ws,
        &task_upload_path(c.ws, &task_id),
        "a.txt",
        b"abc",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{stored}");
    let att_id = stored["id"].as_str().unwrap().to_string();
    let list_path = format!("/api/v1/workspaces/{}/tasks/{task_id}/attachments", c.ws);
    let meta_path = format!("/api/v1/workspaces/{}/attachments/{att_id}", c.ws);

    // A workspace member outside the private project sees nothing.
    let outsider = add_workspace_user(&c.admin, c.ws, "member", "outsider").await;
    for path in [&list_path, &meta_path] {
        let (status, _) =
            json_request(c.app.clone(), "GET", path, None, Some(&outsider.cookie)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
    }
    let (status, _) = upload(
        &c.app,
        &outsider.cookie,
        c.ws,
        &task_upload_path(c.ws, &task_id),
        "b.txt",
        b"abc",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // A project viewer reads but cannot upload or delete.
    let viewer = add_workspace_user(&c.admin, c.ws, "member", "viewer").await;
    add_project_member(&c.admin, c.ws, project_id, viewer.user_id, "viewer").await;
    let (status, list) =
        json_request(c.app.clone(), "GET", &list_path, None, Some(&viewer.cookie)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    let (status, _) = upload(
        &c.app,
        &viewer.cookie,
        c.ws,
        &task_upload_path(c.ws, &task_id),
        "c.txt",
        b"abc",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = json_request(
        c.app.clone(),
        "DELETE",
        &meta_path,
        None,
        Some(&viewer.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // A guest who is a project member uses the project role.
    let guest = add_workspace_user(&c.admin, c.ws, "guest", "guest").await;
    add_project_member(&c.admin, c.ws, project_id, guest.user_id, "member").await;
    let (status, body) = upload(
        &c.app,
        &guest.cookie,
        c.ws,
        &task_upload_path(c.ws, &task_id),
        "g.txt",
        b"guest",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // A different uploader cannot continue someone else's upload.
    let member = add_workspace_user(&c.admin, c.ws, "member", "editor").await;
    add_project_member(&c.admin, c.ws, project_id, member.user_id, "member").await;
    let (status, created) = json_request(
        c.app.clone(),
        "POST",
        &task_upload_path(c.ws, &task_id),
        Some(json!({"name": "x.bin", "sizeBytes": 3})),
        Some(&c.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, body, _) = http_request(
        c.app.clone(),
        "PUT",
        created["parts"][0]["url"].as_str().unwrap(),
        Some(b"xyz".to_vec()),
        Some("application/octet-stream"),
        Some(&member.cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], "only_the_uploader_may_continue_this_upload");
    c.done().await;
}

#[tokio::test]
async fn archived_and_trashed_parents_refuse_writes() {
    let c = ctx().await;
    let project = create_project(c.app.clone(), &c.cookie, c.ws, "ARC", "workspace").await;
    let project_id = project["id"].as_str().unwrap();
    let task_id = create_task(&c.app, &c.cookie, c.ws, project_id).await;
    let (status, stored) = upload(
        &c.app,
        &c.cookie,
        c.ws,
        &task_upload_path(c.ws, &task_id),
        "a.txt",
        b"abc",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let att_id = stored["id"].as_str().unwrap().to_string();
    let task_uuid = Uuid::parse_str(&task_id).unwrap();

    sqlx::query("UPDATE fvoci.tasks SET archived_at = now() WHERE id = $1")
        .bind(task_uuid)
        .execute(&c.admin)
        .await
        .unwrap();
    let (status, body) = upload(
        &c.app,
        &c.cookie,
        c.ws,
        &task_upload_path(c.ws, &task_id),
        "b.txt",
        b"abc",
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "task_archived");
    // Stored attachments on an archived task cannot be deleted; reads stay.
    let (status, body) = json_request(
        c.app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{}/attachments/{att_id}", c.ws),
        None,
        Some(&c.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "task_archived");
    let (status, _) = json_request(
        c.app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{}/attachments/{att_id}", c.ws),
        None,
        Some(&c.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    sqlx::query("UPDATE fvoci.tasks SET archived_at = NULL WHERE id = $1")
        .bind(task_uuid)
        .execute(&c.admin)
        .await
        .unwrap();
    sqlx::query("UPDATE fvoci.projects SET status = 'archived' WHERE id = $1")
        .bind(Uuid::parse_str(project_id).unwrap())
        .execute(&c.admin)
        .await
        .unwrap();
    let (status, body) = upload(
        &c.app,
        &c.cookie,
        c.ws,
        &task_upload_path(c.ws, &task_id),
        "c.txt",
        b"abc",
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "project_archived");

    sqlx::query("UPDATE fvoci.projects SET status = 'active' WHERE id = $1")
        .bind(Uuid::parse_str(project_id).unwrap())
        .execute(&c.admin)
        .await
        .unwrap();
    sqlx::query("UPDATE fvoci.tasks SET deleted_at = now() WHERE id = $1")
        .bind(task_uuid)
        .execute(&c.admin)
        .await
        .unwrap();
    let (status, _) = upload(
        &c.app,
        &c.cookie,
        c.ws,
        &task_upload_path(c.ws, &task_id),
        "d.txt",
        b"abc",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = json_request(
        c.app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{}/tasks/{task_id}/attachments", c.ws),
        None,
        Some(&c.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    c.done().await;
}

#[tokio::test]
async fn project_document_uploads_only_through_their_own_route() {
    let c = ctx().await;
    let project = create_project(c.app.clone(), &c.cookie, c.ws, "DOC", "workspace").await;
    let project_id = project["id"].as_str().unwrap();
    let other = create_project(c.app.clone(), &c.cookie, c.ws, "OTH", "workspace").await;
    let other_id = other["id"].as_str().unwrap();
    let doc_id = create_project_document(&c.app, &c.cookie, c.ws, &project).await;
    let wiki_id = create_wiki_document(&c.app, &c.cookie, c.ws).await;

    let project_path = format!(
        "/api/v1/workspaces/{}/projects/{project_id}/documents/{doc_id}/uploads",
        c.ws
    );
    let (status, stored) = upload(&c.app, &c.cookie, c.ws, &project_path, "p.txt", b"p").await;
    assert_eq!(status, StatusCode::OK, "{stored}");

    // Wrong affiliation: wiki route for a project doc, another project, and
    // the project route for a wiki doc.
    for path in [
        format!("/api/v1/workspaces/{}/documents/{doc_id}/uploads", c.ws),
        format!(
            "/api/v1/workspaces/{}/projects/{other_id}/documents/{doc_id}/uploads",
            c.ws
        ),
        format!(
            "/api/v1/workspaces/{}/projects/{project_id}/documents/{wiki_id}/uploads",
            c.ws
        ),
    ] {
        let (status, _) = upload(&c.app, &c.cookie, c.ws, &path, "x.txt", b"x").await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
    }
    let (status, _) = upload(
        &c.app,
        &c.cookie,
        c.ws,
        &format!("/api/v1/workspaces/{}/documents/{wiki_id}/uploads", c.ws),
        "w.txt",
        b"w",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    c.done().await;
}

#[tokio::test]
async fn delete_needs_edit_for_own_and_manage_for_others_and_reclaims_objects() {
    let c = ctx().await;
    let project = create_project(c.app.clone(), &c.cookie, c.ws, "DEL", "workspace").await;
    let project_id = project["id"].as_str().unwrap();
    let task_id = create_task(&c.app, &c.cookie, c.ws, project_id).await;
    let member = add_workspace_user(&c.admin, c.ws, "member", "member").await;

    let (_, owner_att) = upload(
        &c.app,
        &c.cookie,
        c.ws,
        &task_upload_path(c.ws, &task_id),
        "owner.txt",
        b"owner",
    )
    .await;
    let owner_att = owner_att["id"].as_str().unwrap().to_string();
    let (status, member_att) = upload(
        &c.app,
        &member.cookie,
        c.ws,
        &task_upload_path(c.ws, &task_id),
        "member.txt",
        b"member",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{member_att}");
    let member_att = member_att["id"].as_str().unwrap().to_string();

    // Member (edit) cannot delete the owner's attachment.
    let (status, _) = json_request(
        c.app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{}/attachments/{owner_att}", c.ws),
        None,
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Member deletes their own; owner (manage) deletes someone else's.
    let key = storage_key(&c.admin, &member_att).await;
    let (status, body) = json_request(
        c.app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{}/attachments/{member_att}", c.ws),
        None,
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ok"], true);
    assert_eq!(
        journal_rows(&c.admin, &member_att).await,
        0,
        "reclaimed inline"
    );
    let state = app_state(&c.harness.app_url).await;
    assert_eq!(state.storage.head(&key).await.unwrap(), None);

    let (status, _) = json_request(
        c.app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{}/attachments/{owner_att}", c.ws),
        None,
        Some(&c.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = json_request(
        c.app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{}/attachments/{owner_att}", c.ws),
        None,
        Some(&c.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "second delete");

    let deleted: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM fvoci.events WHERE verb = 'attachment.deleted' AND workspace_id = $1",
    )
    .bind(c.ws)
    .fetch_one(&c.admin)
    .await
    .unwrap();
    assert_eq!(deleted, 2);
    let audited: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM fvoci.audit_log WHERE verb = 'attachment.deleted' AND workspace_id = $1",
    )
    .bind(c.ws)
    .fetch_one(&c.admin)
    .await
    .unwrap();
    assert_eq!(audited, 2);
    let (_, list) = json_request(
        c.app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{}/tasks/{task_id}/attachments", c.ws),
        None,
        Some(&c.cookie),
    )
    .await;
    assert!(list["items"].as_array().unwrap().is_empty());
    c.done().await;
}

#[tokio::test]
async fn delete_event_failure_rolls_back_row_and_journal() {
    let c = ctx().await;
    let doc_id = create_wiki_document(&c.app, &c.cookie, c.ws).await;
    let (_, stored) = upload(
        &c.app,
        &c.cookie,
        c.ws,
        &format!("/api/v1/workspaces/{}/documents/{doc_id}/uploads", c.ws),
        "keep.txt",
        b"keep",
    )
    .await;
    let att_id = stored["id"].as_str().unwrap().to_string();
    project_harness::install_insert_fail_trigger(&c.admin, "audit_log", "att_del_fail").await;
    let (status, _) = json_request(
        c.app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{}/attachments/{att_id}", c.ws),
        None,
        Some(&c.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    let still: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.attachments WHERE id = $1")
        .bind(Uuid::parse_str(&att_id).unwrap())
        .fetch_one(&c.admin)
        .await
        .unwrap();
    assert_eq!(still, 1);
    assert_eq!(journal_rows(&c.admin, &att_id).await, 0);
    project_harness::drop_insert_fail_trigger(&c.admin, "audit_log", "att_del_fail").await;
    let (status, _, _) = http_request(
        c.app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{}/attachments/{att_id}/download", c.ws),
        None,
        None,
        Some(&c.cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "object kept");
    c.done().await;
}

#[tokio::test]
async fn delete_during_upload_blocks_later_complete_and_reclaims_parts() {
    let c = ctx().await;
    let doc_id = create_wiki_document(&c.app, &c.cookie, c.ws).await;
    let (status, created) = json_request(
        c.app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{}/documents/{doc_id}/uploads", c.ws),
        Some(json!({"name": "half.bin", "sizeBytes": 4})),
        Some(&c.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let att_id = created["attachmentId"].as_str().unwrap().to_string();
    let (status, _, headers) = http_request(
        c.app.clone(),
        "PUT",
        created["parts"][0]["url"].as_str().unwrap(),
        Some(b"half".to_vec()),
        Some("application/octet-stream"),
        Some(&c.cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let etag = headers.get("etag").unwrap().to_str().unwrap().to_string();
    let (status, _) = json_request(
        c.app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{}/attachments/{att_id}", c.ws),
        None,
        Some(&c.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(journal_rows(&c.admin, &att_id).await, 0);
    let (status, _) = json_request(
        c.app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{}/attachments/{att_id}/complete", c.ws),
        Some(json!({"parts": [{"partNumber": 1, "etag": etag}]})),
        Some(&c.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    c.done().await;
}

#[tokio::test]
async fn object_journal_waits_for_an_in_flight_complete() {
    let c = ctx().await;
    let doc_id = create_wiki_document(&c.app, &c.cookie, c.ws).await;
    let (_, stored) = upload(
        &c.app,
        &c.cookie,
        c.ws,
        &format!("/api/v1/workspaces/{}/documents/{doc_id}/uploads", c.ws),
        "busy.txt",
        b"busy",
    )
    .await;
    let att_id = Uuid::parse_str(stored["id"].as_str().unwrap()).unwrap();
    let key = storage_key(&c.admin, &att_id.to_string()).await;
    // Hold the upload session lock the way an in-flight complete does.
    let holder = admin_pool(&c.harness).await;
    let mut conn = holder.acquire().await.unwrap();
    sqlx::query("SELECT pg_advisory_lock($1, $2)")
        .bind(fvoci_server::attachments::ATTACHMENT_LOCK_NAMESPACE)
        .bind(fvoci_server::db::context::lock_key_from_uuid(att_id))
        .execute(&mut *conn)
        .await
        .unwrap();
    let (status, _) = json_request(
        c.app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{}/attachments/{att_id}", c.ws),
        None,
        Some(&c.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        journal_rows(&c.admin, &att_id.to_string()).await,
        1,
        "busy row kept"
    );
    let state = app_state(&c.harness.app_url).await;
    let pool = &state.auth.db.pool;
    // Rows rescheduled a minute out; make them due and release the holder.
    sqlx::query("SELECT pg_advisory_unlock($1, $2)")
        .bind(fvoci_server::attachments::ATTACHMENT_LOCK_NAMESPACE)
        .bind(fvoci_server::db::context::lock_key_from_uuid(att_id))
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    sqlx::query("UPDATE fvoci.attachment_object_cleanups SET due_at = now() - interval '1 second'")
        .execute(&c.admin)
        .await
        .unwrap();
    let stats =
        fvoci_server::db::attachments::reclaim_attachment_objects(pool, &state.storage, None, 10)
            .await
            .unwrap();
    assert_eq!(stats.reclaimed, 1, "{stats:?}");
    assert_eq!(journal_rows(&c.admin, &att_id.to_string()).await, 0);
    assert_eq!(state.storage.head(&key).await.unwrap(), None);
    holder.close().await;
    c.done().await;
}

#[tokio::test]
async fn hwp_edit_context_and_edit_copy_on_the_same_parent() {
    let c = ctx().await;
    let project = create_project(c.app.clone(), &c.cookie, c.ws, "HWP", "workspace").await;
    let project_id = project["id"].as_str().unwrap();
    let task_id = create_task(&c.app, &c.cookie, c.ws, project_id).await;
    let (_, hwp) = upload(
        &c.app,
        &c.cookie,
        c.ws,
        &task_upload_path(c.ws, &task_id),
        HWPX_NAME,
        b"PK\x03\x04not really a zip",
    )
    .await;
    let hwp_id = hwp["id"].as_str().unwrap().to_string();
    let (_, txt) = upload(
        &c.app,
        &c.cookie,
        c.ws,
        &task_upload_path(c.ws, &task_id),
        "plain.txt",
        b"plain",
    )
    .await;
    let txt_id = txt["id"].as_str().unwrap().to_string();

    let ctx_path = |id: &str| format!("/api/v1/workspaces/{}/attachments/{id}/edit-context", c.ws);
    let (status, body) = json_request(
        c.app.clone(),
        "GET",
        &ctx_path(&hwp_id),
        None,
        Some(&c.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["editable"], true);
    assert_eq!(body["sourceAttachmentId"], hwp_id);
    let (_, body) = json_request(
        c.app.clone(),
        "GET",
        &ctx_path(&txt_id),
        None,
        Some(&c.cookie),
    )
    .await;
    assert_eq!(body["editable"], false);

    let viewer = add_workspace_user(&c.admin, c.ws, "guest", "viewer").await;
    add_project_member(&c.admin, c.ws, project_id, viewer.user_id, "viewer").await;
    let (status, body) = json_request(
        c.app.clone(),
        "GET",
        &ctx_path(&hwp_id),
        None,
        Some(&viewer.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["editable"], false);

    let copy_path = |id: &str| format!("/api/v1/workspaces/{}/attachments/{id}/edit-copy", c.ws);
    let (status, stored) = upload(
        &c.app,
        &c.cookie,
        c.ws,
        &copy_path(&hwp_id),
        "report (edited).hwpx",
        b"PK\x03\x04edited",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{stored}");
    let copy_task: Option<Uuid> =
        sqlx::query_scalar("SELECT task_id FROM fvoci.attachments WHERE id = $1")
            .bind(Uuid::parse_str(stored["id"].as_str().unwrap()).unwrap())
            .fetch_one(&c.admin)
            .await
            .unwrap();
    assert_eq!(copy_task.unwrap().to_string(), task_id);

    let (status, body) = upload(&c.app, &c.cookie, c.ws, &copy_path(&txt_id), "x.txt", b"x").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let (status, _) = upload(
        &c.app,
        &viewer.cookie,
        c.ws,
        &copy_path(&hwp_id),
        "v.hwpx",
        b"PK",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    c.done().await;
}

#[tokio::test]
async fn api_tokens_need_the_parent_domain_scope() {
    let c = ctx().await;
    let project = create_project(c.app.clone(), &c.cookie, c.ws, "TOK", "workspace").await;
    let project_id = project["id"].as_str().unwrap();
    let task_id = create_task(&c.app, &c.cookie, c.ws, project_id).await;
    let doc_id = create_wiki_document(&c.app, &c.cookie, c.ws).await;
    let (_, task_att) = upload(
        &c.app,
        &c.cookie,
        c.ws,
        &task_upload_path(c.ws, &task_id),
        "t.txt",
        b"t",
    )
    .await;
    let task_att = task_att["id"].as_str().unwrap().to_string();
    let (_, doc_att) = upload(
        &c.app,
        &c.cookie,
        c.ws,
        &format!("/api/v1/workspaces/{}/documents/{doc_id}/uploads", c.ws),
        "d.txt",
        b"d",
    )
    .await;
    let doc_att = doc_att["id"].as_str().unwrap().to_string();

    let docs_only = create_api_token(&c.app, &c.cookie, c.ws, &["documents.read"]).await;
    let tasks_only = create_api_token(&c.app, &c.cookie, c.ws, &["tasks.read"]).await;
    let meta = |id: &str| format!("/api/v1/workspaces/{}/attachments/{id}", c.ws);
    assert_eq!(
        bearer(&c.app, "GET", &meta(&doc_att), &docs_only).await,
        StatusCode::OK
    );
    assert_eq!(
        bearer(&c.app, "GET", &meta(&task_att), &docs_only).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        bearer(&c.app, "GET", &meta(&task_att), &tasks_only).await,
        StatusCode::OK
    );
    assert_eq!(
        bearer(&c.app, "GET", &meta(&doc_att), &tasks_only).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        bearer(
            &c.app,
            "GET",
            &format!("/api/v1/workspaces/{}/tasks/{task_id}/attachments", c.ws),
            &tasks_only
        )
        .await,
        StatusCode::OK
    );
    // Read scopes cannot delete.
    assert_eq!(
        bearer(&c.app, "DELETE", &meta(&task_att), &tasks_only).await,
        StatusCode::NOT_FOUND
    );
    let tasks_write = create_api_token(&c.app, &c.cookie, c.ws, &["tasks.write"]).await;
    assert_eq!(
        bearer(&c.app, "DELETE", &meta(&doc_att), &tasks_write).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        bearer(&c.app, "DELETE", &meta(&task_att), &tasks_write).await,
        StatusCode::OK
    );
    c.done().await;
}

#[tokio::test]
async fn task_attachments_stay_out_of_shares_and_reach_extract_and_search() {
    let c = ctx().await;
    let project = create_project(c.app.clone(), &c.cookie, c.ws, "SRC", "workspace").await;
    let project_id = project["id"].as_str().unwrap();
    let task_id = create_task(&c.app, &c.cookie, c.ws, project_id).await;
    let (_, hwp) = upload(
        &c.app,
        &c.cookie,
        c.ws,
        &task_upload_path(c.ws, &task_id),
        HWPX_NAME,
        b"PK\x03\x04hwpx",
    )
    .await;
    let hwp_id = Uuid::parse_str(hwp["id"].as_str().unwrap()).unwrap();

    // A document share never resolves a task attachment (no 500 on the NULL
    // document parent).
    let doc_id = create_wiki_document(&c.app, &c.cookie, c.ws).await;
    let (status, body) = json_request(
        c.app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{}/documents/{doc_id}/share-links", c.ws),
        Some(json!({})),
        Some(&c.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let token = body["url"]
        .as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_string();
    let (status, _) = json_request(
        c.app.clone(),
        "GET",
        &format!("/api/v1/share/{token}/attachments/{hwp_id}"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // The HWP extract claim takes a task-parented attachment.
    let state = app_state(&c.harness.app_url).await;
    let pool = &state.auth.db.pool;
    let claim = fvoci_server::db::attachment_extract::claim_extract(pool)
        .await
        .unwrap()
        .expect("task attachment claimed");
    assert_eq!(claim.attachment_id, hwp_id);

    // The search index source lists it with its task parent.
    let rows = fvoci_server::db::search_index::list_sources(
        pool,
        c.ws,
        None,
        100,
        &fvoci_server::db::search_index::SourceScope {
            project_id: None,
            document_id: None,
            subtree: false,
            task_id: Some(Uuid::parse_str(&task_id).unwrap()),
        },
    )
    .await
    .unwrap();
    let att_rows: Vec<_> = rows
        .iter()
        .filter(|r| r.attachment_id == Some(hwp_id))
        .collect();
    assert_eq!(att_rows.len(), 1, "{rows:?}");
    assert_eq!(
        att_rows[0].task_id.map(|id| id.to_string()),
        Some(task_id.clone())
    );
    let _ = c.owner;
    c.done().await;
}

// ---------------------------------------------------------------------------
// Storage quota (source requireStorageReservation)

fn quota_app(
    c: &Ctx,
    state: fvoci_server::http::state::AppState,
    storage: i64,
    upload: i64,
) -> axum::Router {
    let mut state = state;
    state.quota = fvoci_server::db::quota::StorageQuota::fixed(
        fvoci_server::db::quota::QuotaLimit::Bytes(storage),
        fvoci_server::db::quota::QuotaLimit::Bytes(upload),
    );
    let _ = c;
    fvoci_server::http::router(state, None)
}

async fn create_only(
    app: &axum::Router,
    cookie: &str,
    path: &str,
    size: usize,
) -> (StatusCode, Value) {
    json_request(
        app.clone(),
        "POST",
        path,
        Some(json!({"name": "q.bin", "sizeBytes": size})),
        Some(cookie),
    )
    .await
}

#[tokio::test]
async fn upload_and_storage_limits_count_every_row_of_the_workspace() {
    let c = ctx().await;
    let app = quota_app(&c, app_state(&c.harness.app_url).await, 30, 12);
    let doc_id = create_wiki_document(&c.app, &c.cookie, c.ws).await;
    let project = create_project(c.app.clone(), &c.cookie, c.ws, "QTA", "workspace").await;
    let task_id = create_task(&c.app, &c.cookie, c.ws, project["id"].as_str().unwrap()).await;
    let doc_path = format!("/api/v1/workspaces/{}/documents/{doc_id}/uploads", c.ws);
    let task_path = task_upload_path(c.ws, &task_id);

    let (status, body) = create_only(&app, &c.cookie, &doc_path, 13).await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{body}");
    assert_eq!(body["code"], "limit.upload");

    // 12 stored on the document + 12 still uploading on the task = 24.
    let (status, stored) = upload(&app, &c.cookie, c.ws, &doc_path, "a.bin", &[1u8; 12]).await;
    assert_eq!(status, StatusCode::OK, "{stored}");
    let (status, _) = create_only(&app, &c.cookie, &task_path, 12).await;
    assert_eq!(status, StatusCode::CREATED);
    // 24 + 7 > 30: refused across parents; 6 fits exactly.
    let (status, body) = create_only(&app, &c.cookie, &task_path, 7).await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{body}");
    assert_eq!(body["code"], "limit.storage");
    let (status, _) = create_only(&app, &c.cookie, &task_path, 6).await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = create_only(&app, &c.cookie, &task_path, 1).await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED);

    // Deleting the stored attachment frees its reservation.
    let (status, _) = json_request(
        app.clone(),
        "DELETE",
        &format!(
            "/api/v1/workspaces/{}/attachments/{}",
            c.ws,
            stored["id"].as_str().unwrap()
        ),
        None,
        Some(&c.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = create_only(&app, &c.cookie, &task_path, 12).await;
    assert_eq!(status, StatusCode::CREATED);

    // Full again (12 + 12 + 6 = 30).
    let (status, _) = create_only(&app, &c.cookie, &doc_path, 1).await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED);
    // Permission is checked before the quota: an outsider learns nothing.
    let outsider = add_workspace_user(&c.admin, c.ws, "guest", "outsider").await;
    let (status, _) = create_only(&app, &outsider.cookie, &doc_path, 1).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    c.done().await;
}

#[tokio::test]
async fn signed_license_upload_and_storage_limits_reach_reservation() {
    let c = ctx().await;
    let mut state = app_state(&c.harness.app_url).await;
    let license =
        license_fixture::signed_license_with_limits(json!({"storageBytes": 30, "uploadBytes": 12}));
    state.auth = std::sync::Arc::new(fvoci_server::auth::AuthService {
        db: fvoci_server::db::Db::with_license(state.auth.db.pool.clone(), license.clone()),
        password_keys: state.auth.password_keys.clone(),
    });
    state.quota = fvoci_server::db::quota::StorageQuota::from_license(license);
    let app = fvoci_server::http::router(state, None);
    let doc_id = create_wiki_document(&c.app, &c.cookie, c.ws).await;
    let path = format!("/api/v1/workspaces/{}/documents/{doc_id}/uploads", c.ws);

    let (status, body) = create_only(&app, &c.cookie, &path, 13).await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{body}");
    assert_eq!(body["code"], "limit.upload");
    for _ in 0..2 {
        let (status, body) = create_only(&app, &c.cookie, &path, 12).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }
    let (status, body) = create_only(&app, &c.cookie, &path, 7).await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{body}");
    assert_eq!(body["code"], "limit.storage");
    let (status, body) = create_only(&app, &c.cookie, &path, 6).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    c.done().await;
}

#[tokio::test]
async fn concurrent_reservations_never_exceed_the_storage_limit() {
    let c = ctx().await;
    let app = quota_app(&c, app_state(&c.harness.app_url).await, 100, 100);
    let doc_id = create_wiki_document(&c.app, &c.cookie, c.ws).await;
    let project = create_project(c.app.clone(), &c.cookie, c.ws, "CON", "workspace").await;
    let task_id = create_task(&c.app, &c.cookie, c.ws, project["id"].as_str().unwrap()).await;
    let doc_path = format!("/api/v1/workspaces/{}/documents/{doc_id}/uploads", c.ws);
    let task_path = task_upload_path(c.ws, &task_id);
    // Separate users so the per-user create rate limit is not the gate.
    let mut users = Vec::new();
    for i in 0..10 {
        users.push(add_workspace_user(&c.admin, c.ws, "member", &format!("u{i}")).await);
    }
    let mut handles = Vec::new();
    for (i, user) in users.iter().enumerate() {
        let app = app.clone();
        let cookie = user.cookie.clone();
        let path = if i % 2 == 0 {
            doc_path.clone()
        } else {
            task_path.clone()
        };
        handles.push(tokio::spawn(async move {
            create_only(&app, &cookie, &path, 30).await.0
        }));
    }
    let mut created = 0;
    let mut refused = 0;
    for handle in handles {
        match handle.await.unwrap() {
            StatusCode::CREATED => created += 1,
            StatusCode::PAYMENT_REQUIRED => refused += 1,
            other => panic!("unexpected {other}"),
        }
    }
    assert_eq!((created, refused), (3, 7));
    let reserved: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(reserved_size_bytes), 0)::bigint FROM fvoci.attachments WHERE workspace_id = $1",
    )
    .bind(c.ws)
    .fetch_one(&c.admin)
    .await
    .unwrap();
    assert_eq!(reserved, 90);
    c.done().await;
}
