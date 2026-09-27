#![cfg(feature = "db-tests")]
#![allow(dead_code)]

#[path = "support/project_harness.rs"]
mod project_harness;

use axum::http::StatusCode;
use chrono::{Duration as ChronoDuration, Utc};
use futures_util::StreamExt;
use project_harness::{
    add_workspace_user, admin_pool, app_pool, create_project, http_request, insert_minimal_project,
    insert_project_document, insert_stored_attachment, json_request, setup_session, TestDb,
};
use serde_json::json;
use uuid::Uuid;

fn item_for<'a>(body: &'a serde_json::Value, slug: &str) -> &'a serde_json::Value {
    body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["slug"] == slug)
        .unwrap_or_else(|| panic!("missing workspace {slug} in {body}"))
}

async fn create_wiki(
    app: axum::Router,
    cookie: &str,
    workspace_id: Uuid,
    title: &str,
) -> serde_json::Value {
    let (status, body) = json_request(
        app,
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"title": title, "parentId": null})),
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body:?}");
    body
}

#[tokio::test]
async fn workspace_card_counts_respect_visibility() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let guest = add_workspace_user(&admin, workspace_id, "guest", "guest").await;

    create_wiki(app.clone(), &cookie, workspace_id, "위키 문서").await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let hid = create_project(app.clone(), &cookie, workspace_id, "HID", "private").await;
    let lab_id = lab["id"].as_str().unwrap();
    let (status, task) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{lab_id}/tasks"),
        Some(json!({"title": "열린 일"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{task:?}");
    let task_id = task["id"].as_str().unwrap();
    let (status, patched) = json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}"),
        Some(json!({"assigneeIds": [owner_id.to_string()]})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{patched:?}");

    let (status, listed) = json_request(
        app.clone(),
        "GET",
        "/api/v1/me/workspaces",
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{listed:?}");
    let owner_card = item_for(&listed, "acme");
    assert!(
        owner_card["documentCount"].as_i64().unwrap() >= 3,
        "owner sees wiki + two project root documents: {owner_card}"
    );
    assert_eq!(owner_card["assignedCount"], 1);

    let (status, guest_listed) = json_request(
        app.clone(),
        "GET",
        "/api/v1/me/workspaces",
        None,
        Some(&guest.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{guest_listed:?}");
    let guest_card = item_for(&guest_listed, "acme");
    assert_eq!(
        guest_card["documentCount"], 0,
        "guest excludes wiki and non-member projects: {guest_card}"
    );
    assert_eq!(guest_card["assignedCount"], 0);
    let _ = hid;

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn trash_workspace_is_owner_only_and_emits_deleted_event() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, workspace_id, "member", "member").await;

    let (status, created) = json_request(
        app.clone(),
        "POST",
        "/api/v1/workspaces",
        Some(json!({"name": "Beta 팀", "slug": "beta-team"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created:?}");
    let beta_id = Uuid::parse_str(created["id"].as_str().unwrap()).unwrap();

    let (status, invite) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{beta_id}/invitations"),
        Some(json!({"email": "pending-lifecycle@example.com", "role": "member"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{invite:?}");

    let (status, token_body) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{beta_id}/api-tokens"),
        Some(json!({"name": "lifecycle", "scopes": ["workspace.manage"]})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{token_body:?}");
    let secret = token_body["token"].as_str().unwrap().to_string();

    let (status, forbidden) = json_request(
        app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{workspace_id}"),
        Some(json!({"confirmSlug": "acme"})),
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{forbidden:?}");
    assert_eq!(forbidden["code"], "insufficient_permissions");

    let (status, mismatch) = json_request(
        app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{beta_id}"),
        Some(json!({"confirmSlug": "wrong-slug"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{mismatch:?}");
    assert_eq!(mismatch["code"], "invalid_input");

    let (status, deleted) = json_request(
        app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{beta_id}"),
        Some(json!({"confirmSlug": "beta-team"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{deleted:?}");
    assert_eq!(deleted["ok"], true);

    let (status, listed) = json_request(
        app.clone(),
        "GET",
        "/api/v1/me/workspaces",
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        listed["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["slug"] != "beta-team"),
        "{listed}"
    );

    let (status, missing) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{beta_id}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{missing:?}");

    let pending: i64 =
        sqlx::query_scalar("SELECT count(*) FROM fvoci.invitations WHERE workspace_id = $1")
            .bind(beta_id)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(pending, 0);

    let tokens: i64 =
        sqlx::query_scalar("SELECT count(*) FROM fvoci.api_tokens WHERE workspace_id = $1")
            .bind(beta_id)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(tokens, 0);

    let memberships: i64 =
        sqlx::query_scalar("SELECT count(*) FROM fvoci.memberships WHERE workspace_id = $1")
            .bind(beta_id)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(memberships, 0);

    let deleted_at: Option<chrono::DateTime<Utc>> =
        sqlx::query_scalar("SELECT deleted_at FROM fvoci.workspaces WHERE id = $1")
            .bind(beta_id)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert!(deleted_at.is_some());

    let verbs: Vec<(String,)> = sqlx::query_as(
        "SELECT verb FROM fvoci.events WHERE workspace_id = $1 AND verb = 'workspace.deleted'",
    )
    .bind(beta_id)
    .fetch_all(&admin)
    .await
    .unwrap();
    assert_eq!(verbs.len(), 1);

    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM fvoci.audit_log WHERE workspace_id = $1 AND verb = 'workspace.deleted'",
    )
    .bind(beta_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(audits, 1);

    let auth = format!("Bearer {secret}");
    let (status, stale, _) = http_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{beta_id}"),
        None,
        None,
        None,
        &[("authorization", auth.as_str())],
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{stale:?}");

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn trash_rejects_personal_workspace_and_accepts_workspace_manage_pat() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _owner_id, workspace_id) = setup_session(&harness).await;

    let (status, personal) = json_request(
        app.clone(),
        "POST",
        "/api/v1/me/personal-workspace",
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{personal:?}");
    let personal_id = personal["id"].as_str().unwrap();
    let personal_slug = personal["slug"].as_str().unwrap();

    let (status, blocked) = json_request(
        app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{personal_id}"),
        Some(json!({"confirmSlug": personal_slug})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{blocked:?}");
    assert_eq!(blocked["code"], "personal_workspace_is_immutable");

    let (status, token_body) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/api-tokens"),
        Some(json!({"name": "docs-only", "scopes": ["documents.read"]})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{token_body:?}");
    let docs_secret = token_body["token"].as_str().unwrap();
    let docs_auth = format!("Bearer {docs_secret}");
    let (status, denied, _) = http_request(
        app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{workspace_id}"),
        Some(json!({"confirmSlug": "acme"}).to_string().into_bytes()),
        Some("application/json"),
        None,
        &[("authorization", docs_auth.as_str())],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{denied:?}");

    let (status, manage_body) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/api-tokens"),
        Some(json!({"name": "manager", "scopes": ["workspace.manage"]})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{manage_body:?}");
    let manage_secret = manage_body["token"].as_str().unwrap();
    let manage_auth = format!("Bearer {manage_secret}");

    let (status, created) = json_request(
        app.clone(),
        "POST",
        "/api/v1/workspaces",
        Some(json!({"name": "Gamma", "slug": "gamma-team"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created:?}");
    let gamma_id = created["id"].as_str().unwrap();

    let (status, token_gamma) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{gamma_id}/api-tokens"),
        Some(json!({"name": "gamma-manage", "scopes": ["workspace.manage"]})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{token_gamma:?}");
    let gamma_secret = token_gamma["token"].as_str().unwrap();
    let gamma_auth = format!("Bearer {gamma_secret}");
    let (status, trashed, _) = http_request(
        app,
        "DELETE",
        &format!("/api/v1/workspaces/{gamma_id}"),
        Some(
            json!({"confirmSlug": "gamma-team"})
                .to_string()
                .into_bytes(),
        ),
        Some("application/json"),
        None,
        &[("authorization", gamma_auth.as_str())],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{trashed:?}");
    let _ = manage_auth;

    harness.cleanup().await;
}

#[tokio::test]
async fn sweep_purges_expired_team_and_personal_immediately() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, _workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let pool = app_pool(&harness).await;

    let (status, created) = json_request(
        app.clone(),
        "POST",
        "/api/v1/workspaces",
        Some(json!({"name": "Doomed", "slug": "doomed-team"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created:?}");
    let doomed_id = Uuid::parse_str(created["id"].as_str().unwrap()).unwrap();
    let wiki = create_wiki(app.clone(), &cookie, doomed_id, "첨부 부모").await;
    let document_id = Uuid::parse_str(wiki["id"].as_str().unwrap()).unwrap();
    let attachment_id = insert_stored_attachment(&admin, doomed_id, document_id, owner_id).await;

    let (status, _) = json_request(
        app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{doomed_id}"),
        Some(json!({"confirmSlug": "doomed-team"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let now = Utc::now();
    let fresh = fvoci_server::db::workspace::sweep_deleted_workspaces(&pool, now)
        .await
        .expect("fresh sweep");
    assert!(
        fresh.iter().all(|row| !row.purged),
        "team workspaces inside the 30-day grace window must not purge: {fresh:?}"
    );
    let still: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM fvoci.workspaces WHERE id = $1")
        .bind(doomed_id)
        .fetch_optional(&admin)
        .await
        .unwrap();
    assert!(still.is_some());

    sqlx::query("UPDATE fvoci.workspaces SET deleted_at = $2 WHERE id = $1")
        .bind(doomed_id)
        .bind(now - ChronoDuration::days(31))
        .execute(&admin)
        .await
        .unwrap();
    let expired = fvoci_server::db::workspace::sweep_deleted_workspaces(&pool, now)
        .await
        .expect("expired sweep");
    assert!(expired.iter().any(|row| row.purged));
    let gone: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM fvoci.workspaces WHERE id = $1")
        .bind(doomed_id)
        .fetch_optional(&admin)
        .await
        .unwrap();
    assert!(gone.is_none());
    let attachments: i64 =
        sqlx::query_scalar("SELECT count(*) FROM fvoci.attachments WHERE id = $1")
            .bind(attachment_id)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(attachments, 0);

    let (status, personal) = json_request(
        app,
        "POST",
        "/api/v1/me/personal-workspace",
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{personal:?}");
    let personal_id = Uuid::parse_str(personal["id"].as_str().unwrap()).unwrap();
    sqlx::query("UPDATE fvoci.workspaces SET deleted_at = now() WHERE id = $1")
        .bind(personal_id)
        .execute(&admin)
        .await
        .unwrap();
    let personal_sweep = fvoci_server::db::workspace::sweep_deleted_workspaces(&pool, now)
        .await
        .expect("personal sweep");
    assert!(personal_sweep.iter().any(|row| row.purged));
    let personal_gone: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM fvoci.workspaces WHERE id = $1")
            .bind(personal_id)
            .fetch_optional(&admin)
            .await
            .unwrap();
    assert!(personal_gone.is_none());

    admin.close().await;
    pool.close().await;
    harness.cleanup().await;
}

fn write_local_attachment_payload(storage_root: &std::path::Path, key: &str, payload: &[u8]) {
    let dir = storage_root.join("objects").join(key);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("payload"), payload).unwrap();
}

async fn setup_session_with_storage(
    harness: &TestDb,
) -> (axum::Router, String, Uuid, Uuid, std::path::PathBuf) {
    use axum::body::Body;
    use axum::http::Request;
    use fvoci_server::attachments::LocalStorage;
    use tower::ServiceExt;

    let storage_root = std::env::temp_dir().join(format!("fvoci-ws-export-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&storage_root).unwrap();
    let mut state = project_harness::app_state(&harness.app_url).await;
    state.storage = LocalStorage::new(storage_root.clone()).into();
    let app = fvoci_server::http::router(state, None);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/setup")
                .header("content-type", "application/json")
                .header("origin", "http://localhost")
                .extension(axum::extract::ConnectInfo(project_harness::test_peer()))
                .body(Body::from(
                    json!({
                        "email": "owner@example.com",
                        "password": "supersecret1",
                        "givenName": "Owner",
                        "workspaceSlug": "acme",
                        "workspaceName": "Acme"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .expect("setup");
    let cookie_hdr = response
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .expect("set-cookie")
        .to_string();
    let cookie = cookie_hdr
        .split(';')
        .next()
        .unwrap()
        .split('=')
        .nth(1)
        .unwrap()
        .to_string();
    let admin = admin_pool(harness).await;
    let user_id: (Uuid,) = sqlx::query_as("SELECT id FROM fvoci.users LIMIT 1")
        .fetch_one(&admin)
        .await
        .unwrap();
    let workspace_id: (Uuid,) =
        sqlx::query_as("SELECT id FROM fvoci.workspaces WHERE slug = 'acme'")
            .fetch_one(&admin)
            .await
            .unwrap();
    admin.close().await;
    (app, cookie, user_id.0, workspace_id.0, storage_root)
}

async fn bytes_request(
    app: axum::Router,
    method: &str,
    path: &str,
    cookie: Option<&str>,
    extra_headers: &[(&str, &str)],
) -> (StatusCode, Vec<u8>, axum::http::HeaderMap) {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("origin", "http://localhost");
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", format!("fvoci_session={}", cookie));
    }
    for (name, value) in extra_headers {
        builder = builder.header(*name, *value);
    }
    let request = builder.body(Body::empty()).unwrap();
    let mut request = request;
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
    (status, bytes, headers)
}

fn external_zip_check(bytes: &[u8]) {
    let path = std::env::temp_dir().join(format!("fvoci-ws-export-{}.zip", Uuid::now_v7()));
    std::fs::write(&path, bytes).unwrap();
    let output = std::process::Command::new("python3")
        .args(["-m", "zipfile", "-t"])
        .arg(&path)
        .output()
        .expect("python3 is required for the external zip check");
    let _ = std::fs::remove_file(&path);
    assert!(
        output.status.success(),
        "python3 -m zipfile -t failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn read_zip_entry(bytes: &[u8], want: &str) -> Option<Vec<u8>> {
    let u16_at = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]) as usize;
    let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
    let eocd = bytes.len().checked_sub(22).filter(|_| bytes.len() >= 22)?;
    if u32_at(eocd) != 0x0605_4b50 {
        return None;
    }
    let count = u16_at(eocd + 10);
    let mut at = u32_at(eocd + 16);
    for _ in 0..count {
        if u32_at(at) != 0x0201_4b50 {
            return None;
        }
        let size = u32_at(at + 24);
        let name_len = u16_at(at + 28);
        let local = u32_at(at + 42);
        let name = String::from_utf8(bytes[at + 46..at + 46 + name_len].to_vec()).unwrap();
        if name == want {
            let data_at = local + 30 + u16_at(local + 26) + u16_at(local + 28);
            return Some(bytes[data_at..data_at + size].to_vec());
        }
        at += 46 + name_len + u16_at(at + 30) + u16_at(at + 32);
    }
    None
}

fn read_zip_names(bytes: &[u8]) -> Vec<String> {
    let mut names = Vec::new();
    let mut offset = 0usize;
    while offset + 4 <= bytes.len() {
        let sig = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
        if sig == 0x0201_4b50 {
            break;
        }
        if sig != 0x0403_4b50 {
            offset += 1;
            continue;
        }
        if offset + 30 > bytes.len() {
            break;
        }
        let name_len =
            u16::from_le_bytes(bytes[offset + 26..offset + 28].try_into().unwrap()) as usize;
        let extra_len =
            u16::from_le_bytes(bytes[offset + 28..offset + 30].try_into().unwrap()) as usize;
        let name_start = offset + 30;
        let name_end = name_start + name_len;
        if name_end > bytes.len() {
            break;
        }
        names.push(String::from_utf8_lossy(&bytes[name_start..name_end]).into_owned());
        let data_len =
            u32::from_le_bytes(bytes[offset + 18..offset + 22].try_into().unwrap()) as usize;
        offset = name_end + extra_len + data_len;
    }
    names
}

#[tokio::test]
async fn workspace_zip_export_requires_manage_and_streams_zip() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, workspace_id, "member", "member").await;

    let wiki = create_wiki(app.clone(), &cookie, workspace_id, "보낼 위키").await;
    let document_id = Uuid::parse_str(wiki["id"].as_str().unwrap()).unwrap();
    let _attachment_id =
        insert_stored_attachment(&admin, workspace_id, document_id, owner_id).await;

    let path = format!("/api/v1/workspaces/{workspace_id}/export");
    let (status, _, _) = bytes_request(app.clone(), "GET", &path, Some(&member.cookie), &[]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, body, headers) =
        bytes_request(app.clone(), "GET", &path, Some(&cookie), &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get("content-type").unwrap(), "application/zip");
    assert_eq!(
        headers.get("content-disposition").unwrap(),
        "attachment; filename=\"fvoci-workspace.zip\""
    );
    external_zip_check(&body);
    let names = read_zip_names(&body);
    assert!(names.iter().any(|n| n == "workspace.json"));
    assert!(names.iter().any(|n| n == "documents.json"));
    let docs: serde_json::Value =
        serde_json::from_slice(&read_zip_entry(&body, "documents.json").expect("documents.json"))
            .expect("documents json");
    assert!(
        docs.as_array()
            .unwrap()
            .iter()
            .any(|d| d["title"] == "보낼 위키"),
        "{docs}"
    );
    let ws_start = body
        .windows(14)
        .position(|w| w == b"workspace.json")
        .expect("workspace.json entry");
    let json_start = body[ws_start..]
        .iter()
        .position(|b| *b == b'{')
        .map(|i| ws_start + i)
        .expect("json brace");
    let json_end = body[json_start..]
        .iter()
        .position(|b| *b == b'}')
        .map(|i| json_start + i + 1)
        .expect("json end");
    let workspace_json: serde_json::Value =
        serde_json::from_slice(&body[json_start..json_end]).expect("workspace.json");
    assert_eq!(workspace_json["slug"], "acme");
    assert!(workspace_json.get("excludedPrivateProjectCount").is_some());
    let _ = document_id;

    let (status, token_body) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/api-tokens"),
        Some(json!({"name": "export", "scopes": ["workspace.manage"]})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let secret = token_body["token"].as_str().unwrap();
    let (status, _, _) = bytes_request(
        app.clone(),
        "GET",
        &path,
        None,
        &[("authorization", &format!("Bearer {secret}"))],
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn workspace_zip_omits_private_project_without_membership() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let private_id = Uuid::now_v7();
    let private_doc = Uuid::now_v7();
    insert_minimal_project(&admin, workspace_id, private_id, "SEC", owner_id, "private").await;
    insert_project_document(&admin, workspace_id, private_id, private_doc, owner_id, 1).await;

    let path = format!("/api/v1/workspaces/{workspace_id}/export");
    let (status, body, _) = bytes_request(app, "GET", &path, Some(&cookie), &[]).await;
    assert_eq!(status, StatusCode::OK);
    external_zip_check(&body);
    let workspace_json: serde_json::Value =
        serde_json::from_slice(&read_zip_entry(&body, "workspace.json").expect("workspace.json"))
            .unwrap();
    assert_eq!(workspace_json["excludedPrivateProjectCount"], 1);
    let docs: serde_json::Value =
        serde_json::from_slice(&read_zip_entry(&body, "documents.json").unwrap()).unwrap();
    let ids: Vec<String> = docs
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d["id"].as_str().map(str::to_string))
        .collect();
    assert!(
        !ids.contains(&private_doc.to_string()),
        "private project doc must not appear: {ids:?}"
    );

    admin.close().await;
    harness.cleanup().await;
}

struct StoredAttachmentInsert<'a> {
    storage_root: &'a std::path::Path,
    admin: &'a sqlx::PgPool,
    workspace_id: Uuid,
    document_id: Uuid,
    uploader_id: Uuid,
    name: &'a str,
    scan_status: &'a str,
    payload: &'a [u8],
}

async fn insert_attachment_with_payload(insert: StoredAttachmentInsert<'_>) -> Uuid {
    let StoredAttachmentInsert {
        storage_root,
        admin,
        workspace_id,
        document_id,
        uploader_id,
        name,
        scan_status,
        payload,
    } = insert;
    let id = Uuid::now_v7();
    let key = Uuid::now_v7().to_string();
    write_local_attachment_payload(storage_root, &key, payload);
    sqlx::query(
        r#"
        INSERT INTO fvoci.attachments (
            id, workspace_id, document_id, uploader_id, status, name, mime, reserved_size_bytes,
            size_bytes, storage_key, scan_status, completed_at
        ) VALUES ($1, $2, $3, $4, 'stored', $5, 'application/octet-stream', $6, $6, $7, $8, now())
        "#,
    )
    .bind(id)
    .bind(workspace_id)
    .bind(document_id)
    .bind(uploader_id)
    .bind(name)
    .bind(payload.len() as i64)
    .bind(key)
    .bind(scan_status)
    .execute(admin)
    .await
    .expect("insert attachment");
    id
}

#[tokio::test]
async fn workspace_zip_skips_infected_attachment_bytes() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id, storage_root) =
        setup_session_with_storage(&harness).await;
    let admin = admin_pool(&harness).await;
    let wiki = create_wiki(app.clone(), &cookie, workspace_id, "첨부 검사").await;
    let document_id = Uuid::parse_str(wiki["id"].as_str().unwrap()).unwrap();
    let infected_id = insert_attachment_with_payload(StoredAttachmentInsert {
        storage_root: &storage_root,
        admin: &admin,
        workspace_id,
        document_id,
        uploader_id: owner_id,
        name: "bad.bin",
        scan_status: "infected",
        payload: b"bad",
    })
    .await;

    let path = format!("/api/v1/workspaces/{workspace_id}/export");
    let (status, body, _) = bytes_request(app, "GET", &path, Some(&cookie), &[]).await;
    assert_eq!(status, StatusCode::OK);
    external_zip_check(&body);
    let meta: serde_json::Value =
        serde_json::from_slice(&read_zip_entry(&body, "attachments.json").unwrap()).unwrap();
    let ids: Vec<String> = meta
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["id"].as_str().unwrap().to_string())
        .collect();
    assert!(
        ids.iter().any(|id| id == &infected_id.to_string()),
        "{meta}"
    );
    let names = read_zip_names(&body);
    assert!(
        !names.iter().any(|n| n.contains(&infected_id.to_string())),
        "infected payload must not be packed: {names:?}"
    );

    admin.close().await;
    harness.cleanup().await;
}

const EXPORT_WITNESS_MARKER: &str = "EXPORT_WITNESS_MARKER_A";
const EXPORT_WITNESS_SECRET: &str = "EXPORT_WITNESS_SECRET_B";

async fn patch_document_export_fields(
    admin: &sqlx::PgPool,
    document_id: Uuid,
    sort_key: &str,
    text: &str,
) {
    sqlx::query("UPDATE fvoci.documents SET sort_key = $1, text = $2 WHERE id = $3")
        .bind(sort_key)
        .bind(text)
        .bind(document_id)
        .execute(admin)
        .await
        .expect("patch document export fields");
}

async fn stream_export_aborts_after_witness<F, Fut>(
    app: axum::Router,
    cookie: &str,
    path: &str,
    witness: &str,
    secret: &str,
    revoke: F,
) -> (bool, Vec<u8>)
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    let request = Request::builder()
        .method("GET")
        .uri(path)
        .header("origin", "http://localhost")
        .header("cookie", format!("fvoci_session={cookie}"))
        .body(Body::empty())
        .unwrap();
    let mut request = request;
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(project_harness::test_peer()));
    let response = app.oneshot(request).await.expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.into_body().into_data_stream();
    let mut buf = Vec::new();
    let mut revoked = false;
    let mut saw_error = false;
    let mut revoke = Some(revoke);
    let witness_bytes = witness.as_bytes();
    let secret_bytes = secret.as_bytes();
    while let Some(frame) = stream.next().await {
        match frame {
            Ok(bytes) => {
                buf.extend_from_slice(&bytes);
                if !revoked && buf.windows(witness_bytes.len()).any(|w| w == witness_bytes) {
                    if let Some(revoke_fn) = revoke.take() {
                        revoke_fn().await;
                    }
                    revoked = true;
                }
            }
            Err(_) => {
                saw_error = true;
                break;
            }
        }
    }
    assert!(revoked, "witness marker must appear before stream ends");
    assert!(
        !buf.windows(secret_bytes.len()).any(|w| w == secret_bytes),
        "revoked content must not be delivered"
    );
    (saw_error, buf)
}

#[tokio::test]
async fn workspace_zip_aborts_when_project_access_revoked_mid_stream() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let project = create_project(app.clone(), &cookie, workspace_id, "PRV", "private").await;
    let project_id = Uuid::parse_str(project["id"].as_str().unwrap()).unwrap();
    let deputy = add_workspace_user(&admin, workspace_id, "member", "deputy-lead").await;
    sqlx::query(
        "INSERT INTO fvoci.project_members (id, workspace_id, project_id, user_id, role) VALUES ($1, $2, $3, $4, 'lead')",
    )
    .bind(Uuid::now_v7())
    .bind(workspace_id)
    .bind(project_id)
    .bind(deputy.user_id)
    .execute(&admin)
    .await
    .expect("second project lead");
    let doc_a = Uuid::now_v7();
    let doc_b = Uuid::now_v7();
    insert_project_document(&admin, workspace_id, project_id, doc_a, owner_id, 2).await;
    insert_project_document(&admin, workspace_id, project_id, doc_b, owner_id, 3).await;
    patch_document_export_fields(&admin, doc_a, "A", EXPORT_WITNESS_MARKER).await;
    patch_document_export_fields(&admin, doc_b, "B", EXPORT_WITNESS_SECRET).await;

    let path = format!("/api/v1/workspaces/{workspace_id}/export");
    let (saw_error, _) = stream_export_aborts_after_witness(
        app,
        &cookie,
        &path,
        EXPORT_WITNESS_MARKER,
        EXPORT_WITNESS_SECRET,
        || async {
            sqlx::query(
                "DELETE FROM fvoci.project_members WHERE workspace_id = $1 AND project_id = $2 AND user_id = $3",
            )
            .bind(workspace_id)
            .bind(project_id)
            .bind(owner_id)
            .execute(&admin)
            .await
            .expect("revoke project membership");
        },
    )
    .await;
    assert!(
        saw_error,
        "export must fail after project access is revoked mid-stream"
    );

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn workspace_zip_aborts_when_wiki_document_revoked_mid_stream() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let wiki_a = create_wiki(app.clone(), &cookie, workspace_id, "witness-a").await;
    let wiki_b = create_wiki(app.clone(), &cookie, workspace_id, "witness-b").await;
    let doc_a = Uuid::parse_str(wiki_a["id"].as_str().unwrap()).unwrap();
    let doc_b = Uuid::parse_str(wiki_b["id"].as_str().unwrap()).unwrap();
    patch_document_export_fields(&admin, doc_a, "A", EXPORT_WITNESS_MARKER).await;
    patch_document_export_fields(&admin, doc_b, "B", EXPORT_WITNESS_SECRET).await;

    let path = format!("/api/v1/workspaces/{workspace_id}/export");
    let (saw_error, _) = stream_export_aborts_after_witness(
        app,
        &cookie,
        &path,
        EXPORT_WITNESS_MARKER,
        EXPORT_WITNESS_SECRET,
        || async {
            sqlx::query(
                "UPDATE fvoci.documents SET deleted_at = now() WHERE workspace_id = $1 AND id = $2",
            )
            .bind(workspace_id)
            .bind(doc_b)
            .execute(&admin)
            .await
            .expect("soft-delete wiki document");
        },
    )
    .await;
    assert!(
        saw_error,
        "export must fail after wiki document is revoked mid-stream"
    );

    admin.close().await;
    harness.cleanup().await;
}

async fn insert_document_comment(
    admin: &sqlx::PgPool,
    workspace_id: Uuid,
    document_id: Uuid,
    author: Uuid,
    body: &str,
) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO fvoci.comments (id, workspace_id, document_id, created_by, body, created_at, updated_at)
        VALUES ($1, $2, $3, $4, $5, now(), now())
        "#,
    )
    .bind(id)
    .bind(workspace_id)
    .bind(document_id)
    .bind(author)
    .bind(body)
    .execute(admin)
    .await
    .expect("insert document comment");
    id
}

async fn insert_task_comment(
    admin: &sqlx::PgPool,
    workspace_id: Uuid,
    task_id: Uuid,
    author: Uuid,
    body: &str,
) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO fvoci.comments (id, workspace_id, task_id, created_by, body, created_at, updated_at)
        VALUES ($1, $2, $3, $4, $5, now(), now())
        "#,
    )
    .bind(id)
    .bind(workspace_id)
    .bind(task_id)
    .bind(author)
    .bind(body)
    .execute(admin)
    .await
    .expect("insert task comment");
    id
}

#[tokio::test]
async fn workspace_zip_export_includes_task_and_project_document_comments() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let project = create_project(app.clone(), &cookie, workspace_id, "CMT", "workspace").await;
    let project_id = Uuid::parse_str(project["id"].as_str().unwrap()).unwrap();
    let project_doc = Uuid::now_v7();
    insert_project_document(&admin, workspace_id, project_id, project_doc, owner_id, 2).await;
    let (status, task) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({"title": "댓글 일"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{task:?}");
    let task_id = Uuid::parse_str(task["id"].as_str().unwrap()).unwrap();
    let wiki = create_wiki(app.clone(), &cookie, workspace_id, "위키 댓글").await;
    let wiki_id = Uuid::parse_str(wiki["id"].as_str().unwrap()).unwrap();
    let doc_comment_id = insert_document_comment(
        &admin,
        workspace_id,
        project_doc,
        owner_id,
        "프로젝트 문서 댓글",
    )
    .await;
    let wiki_comment_id =
        insert_document_comment(&admin, workspace_id, wiki_id, owner_id, "위키 문서 댓글").await;
    let task_comment_id =
        insert_task_comment(&admin, workspace_id, task_id, owner_id, "태스크 댓글").await;

    let path = format!("/api/v1/workspaces/{workspace_id}/export");
    let (status, body, _) = bytes_request(app, "GET", &path, Some(&cookie), &[]).await;
    assert_eq!(status, StatusCode::OK);
    external_zip_check(&body);
    let comments: serde_json::Value =
        serde_json::from_slice(&read_zip_entry(&body, "comments.json").unwrap()).unwrap();
    let ids: Vec<String> = comments
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap().to_string())
        .collect();
    assert!(ids.contains(&doc_comment_id.to_string()));
    assert!(ids.contains(&wiki_comment_id.to_string()));
    assert!(ids.contains(&task_comment_id.to_string()));
    let bodies: Vec<&str> = comments
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["body"].as_str().unwrap())
        .collect();
    assert!(bodies.contains(&"프로젝트 문서 댓글"));
    assert!(bodies.contains(&"위키 문서 댓글"));
    assert!(bodies.contains(&"태스크 댓글"));

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn workspace_zip_export_negative_auth_and_limits() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let guest = add_workspace_user(&admin, workspace_id, "guest", "zip-guest").await;
    let path = format!("/api/v1/workspaces/{workspace_id}/export");

    let (status, _, _) = bytes_request(app.clone(), "GET", &path, None, &[]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _, _) = bytes_request(app.clone(), "GET", &path, Some(&guest.cookie), &[]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, token_body) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/api-tokens"),
        Some(json!({"name": "read-only", "scopes": ["documents.read"]})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let read_secret = token_body["token"].as_str().unwrap();
    let (status, _, _) = bytes_request(
        app.clone(),
        "GET",
        &path,
        None,
        &[("authorization", &format!("Bearer {read_secret}"))],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let unknown = Uuid::now_v7();
    let (status, _, _) = bytes_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{unknown}/export"),
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, created) = json_request(
        app.clone(),
        "POST",
        "/api/v1/workspaces",
        Some(json!({"name": "Trash Me", "slug": "trash-me"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let trashed_id = Uuid::parse_str(created["id"].as_str().unwrap()).unwrap();
    let (status, _) = json_request(
        app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{trashed_id}"),
        Some(json!({"confirmSlug": "trash-me"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = bytes_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{trashed_id}/export"),
        Some(&cookie),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/admin/legal",
        Some(json!({
            "kind": "terms",
            "title": "약관",
            "bodyMarkdown": "# 약관",
            "required": true,
            "effectiveAt": "2026-10-01T00:00:00Z"
        })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, body) = json_request(app.clone(), "GET", &path, None, Some(&cookie)).await;
    assert_eq!(status, StatusCode::PRECONDITION_REQUIRED);
    assert_eq!(body["code"], "consent_required");

    let (status, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/auth/consents",
        Some(json!({"items": [{"kind": "terms", "version": 1}]})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn workspace_zip_export_rate_limited_after_five_exports() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _owner_id, workspace_id) = setup_session(&harness).await;
    let path = format!("/api/v1/workspaces/{workspace_id}/export");

    let (status, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/admin/legal",
        Some(json!({
            "kind": "terms",
            "title": "약관",
            "bodyMarkdown": "# 약관",
            "required": true,
            "effectiveAt": "2026-10-01T00:00:00Z"
        })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, body) = json_request(app.clone(), "GET", &path, None, Some(&cookie)).await;
    assert_eq!(status, StatusCode::PRECONDITION_REQUIRED);
    assert_eq!(body["code"], "consent_required");
    let (status, _) = json_request(
        app.clone(),
        "POST",
        "/api/v1/auth/consents",
        Some(json!({"items": [{"kind": "terms", "version": 1}]})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    for i in 0..5 {
        let (status, _, _) = bytes_request(app.clone(), "GET", &path, Some(&cookie), &[]).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "export {i} should succeed within rate window"
        );
    }
    let (status, _, _) = bytes_request(app, "GET", &path, Some(&cookie), &[]).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);

    harness.cleanup().await;
}

#[tokio::test]
async fn workspace_zip_export_releases_inflight_after_snapshot_db_error() {
    use fvoci_server::db::workspace_export::{
        arm_workspace_export_snapshot_db_error, disarm_workspace_export_snapshot_db_error,
    };

    let harness = TestDb::bootstrap().await;
    let (app, cookie, _owner_id, workspace_id) = setup_session(&harness).await;
    let path = format!("/api/v1/workspaces/{workspace_id}/export");
    arm_workspace_export_snapshot_db_error(workspace_id);
    let (status, _, _) = bytes_request(app.clone(), "GET", &path, Some(&cookie), &[]).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    disarm_workspace_export_snapshot_db_error(workspace_id);
    let (status, body, _) = bytes_request(app, "GET", &path, Some(&cookie), &[]).await;
    assert_eq!(status, StatusCode::OK);
    external_zip_check(&body);

    harness.cleanup().await;
}

#[tokio::test]
async fn workspace_zip_export_concurrent_inflight_returns_429() {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    let harness = TestDb::bootstrap().await;
    let (app, cookie, _owner_id, workspace_id) = setup_session(&harness).await;
    let path = format!("/api/v1/workspaces/{workspace_id}/export");
    let request = Request::builder()
        .method("GET")
        .uri(&path)
        .header("origin", "http://localhost")
        .header("cookie", format!("fvoci_session={cookie}"))
        .body(Body::empty())
        .unwrap();
    let mut request = request;
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(project_harness::test_peer()));
    let response = app.clone().oneshot(request).await.expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.into_body().into_data_stream();
    let (status, _, _) = bytes_request(app, "GET", &path, Some(&cookie), &[]).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    while stream.next().await.is_some() {}

    harness.cleanup().await;
}

#[tokio::test]
async fn workspace_zip_skips_missing_storage_object() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let wiki = create_wiki(app.clone(), &cookie, workspace_id, "missing blob").await;
    let document_id = Uuid::parse_str(wiki["id"].as_str().unwrap()).unwrap();
    let missing_id = Uuid::now_v7();
    let key = Uuid::now_v7().to_string();
    sqlx::query(
        r#"
        INSERT INTO fvoci.attachments (
            id, workspace_id, document_id, uploader_id, status, name, mime, reserved_size_bytes,
            size_bytes, storage_key, scan_status, completed_at
        ) VALUES ($1, $2, $3, $4, 'stored', 'ghost.bin', 'application/octet-stream', 4, 4, $5, 'clean', now())
        "#,
    )
    .bind(missing_id)
    .bind(workspace_id)
    .bind(document_id)
    .bind(owner_id)
    .bind(key)
    .execute(&admin)
    .await
    .expect("insert missing attachment");

    let path = format!("/api/v1/workspaces/{workspace_id}/export");
    let (status, body, _) = bytes_request(app, "GET", &path, Some(&cookie), &[]).await;
    assert_eq!(status, StatusCode::OK);
    external_zip_check(&body);
    let names = read_zip_names(&body);
    assert!(
        !names.iter().any(|n| n.contains(&missing_id.to_string())),
        "missing object must not be packed: {names:?}"
    );

    admin.close().await;
    harness.cleanup().await;
}

const ATTACH_EXPORT_SECRET: &str = "ATTACH_EXPORT_SECRET_PAYLOAD";
const PROJECT_DOC_ATTACHMENT_PAYLOAD: &[u8] = b"project-doc-attachment-payload";

#[tokio::test]
async fn workspace_zip_includes_project_document_attachment_bytes() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id, storage_root) =
        setup_session_with_storage(&harness).await;
    let admin = admin_pool(&harness).await;
    let project = create_project(app.clone(), &cookie, workspace_id, "PAD", "workspace").await;
    let project_id = Uuid::parse_str(project["id"].as_str().unwrap()).unwrap();
    let project_doc = Uuid::now_v7();
    insert_project_document(&admin, workspace_id, project_id, project_doc, owner_id, 2).await;
    let attachment_id = insert_attachment_with_payload(StoredAttachmentInsert {
        storage_root: &storage_root,
        admin: &admin,
        workspace_id,
        document_id: project_doc,
        uploader_id: owner_id,
        name: "proj-doc.bin",
        scan_status: "clean",
        payload: PROJECT_DOC_ATTACHMENT_PAYLOAD,
    })
    .await;

    let path = format!("/api/v1/workspaces/{workspace_id}/export");
    let (status, body, _) = bytes_request(app, "GET", &path, Some(&cookie), &[]).await;
    assert_eq!(status, StatusCode::OK);
    external_zip_check(&body);
    let names = read_zip_names(&body);
    assert!(
        names.iter().any(|n| n.contains(&attachment_id.to_string())),
        "project document attachment bytes must be packed: {names:?}"
    );
    let packed = names
        .iter()
        .find(|n| n.contains(&attachment_id.to_string()))
        .expect("attachment entry");
    let bytes = read_zip_entry(&body, packed).expect("attachment payload");
    assert_eq!(bytes, PROJECT_DOC_ATTACHMENT_PAYLOAD);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn workspace_zip_aborts_when_attachment_revoked_after_storage_open() {
    use fvoci_server::db::workspace_export::arm_attachment_payload_barrier;

    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id, storage_root) =
        setup_session_with_storage(&harness).await;
    let admin = admin_pool(&harness).await;
    let wiki = create_wiki(app.clone(), &cookie, workspace_id, "attach parent").await;
    let document_id = Uuid::parse_str(wiki["id"].as_str().unwrap()).unwrap();
    let attachment_id = insert_attachment_with_payload(StoredAttachmentInsert {
        storage_root: &storage_root,
        admin: &admin,
        workspace_id,
        document_id,
        uploader_id: owner_id,
        name: "secret.bin",
        scan_status: "clean",
        payload: ATTACH_EXPORT_SECRET.as_bytes(),
    })
    .await;
    let (mut reached_rx, proceed_tx) = arm_attachment_payload_barrier(attachment_id);
    let mut proceed_tx = Some(proceed_tx);
    let path = format!("/api/v1/workspaces/{workspace_id}/export");

    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    let request = Request::builder()
        .method("GET")
        .uri(&path)
        .header("origin", "http://localhost")
        .header("cookie", format!("fvoci_session={cookie}"))
        .body(Body::empty())
        .unwrap();
    let mut request = request;
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(project_harness::test_peer()));
    let response = app.oneshot(request).await.expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.into_body().into_data_stream();
    let mut buf = Vec::new();
    let mut saw_error = false;
    let mut barrier_done = false;
    loop {
        let frame = if barrier_done {
            stream.next().await
        } else {
            tokio::select! {
                _ = &mut reached_rx => {
                    sqlx::query(
                        "UPDATE fvoci.documents SET deleted_at = now() WHERE workspace_id = $1 AND id = $2",
                    )
                    .bind(workspace_id)
                    .bind(document_id)
                    .execute(&admin)
                    .await
                    .expect("soft-delete wiki attachment parent");
                    if let Some(tx) = proceed_tx.take() {
                        tx.send(()).expect("release attachment barrier");
                    }
                    barrier_done = true;
                    continue;
                }
                frame = stream.next() => frame,
            }
        };
        match frame {
            Some(Ok(bytes)) => buf.extend_from_slice(&bytes),
            Some(Err(_)) => {
                saw_error = true;
                break;
            }
            None => break,
        }
    }
    assert!(
        barrier_done,
        "export must reach post-open attachment barrier"
    );
    assert!(
        saw_error,
        "export must fail after post-open attachment recheck"
    );
    assert!(
        !buf.windows(ATTACH_EXPORT_SECRET.len())
            .any(|w| w == ATTACH_EXPORT_SECRET.as_bytes()),
        "secret attachment bytes must not be delivered"
    );

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn workspace_zip_client_abort_does_not_deliver_trailing_secret() {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    let harness = TestDb::bootstrap().await;
    let (app, cookie, _owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let wiki_a = create_wiki(app.clone(), &cookie, workspace_id, "abort-a").await;
    let wiki_b = create_wiki(app.clone(), &cookie, workspace_id, "abort-b").await;
    let doc_a = Uuid::parse_str(wiki_a["id"].as_str().unwrap()).unwrap();
    let doc_b = Uuid::parse_str(wiki_b["id"].as_str().unwrap()).unwrap();
    patch_document_export_fields(&admin, doc_a, "A", EXPORT_WITNESS_MARKER).await;
    patch_document_export_fields(&admin, doc_b, "B", EXPORT_WITNESS_SECRET).await;

    let path = format!("/api/v1/workspaces/{workspace_id}/export");
    let request = Request::builder()
        .method("GET")
        .uri(&path)
        .header("origin", "http://localhost")
        .header("cookie", format!("fvoci_session={cookie}"))
        .body(Body::empty())
        .unwrap();
    let mut request = request;
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(project_harness::test_peer()));
    let response = app.oneshot(request).await.expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.into_body().into_data_stream();
    let mut buf = Vec::new();
    let witness = EXPORT_WITNESS_MARKER.as_bytes();
    while let Some(frame) = stream.next().await {
        let bytes = frame.expect("frame");
        buf.extend_from_slice(&bytes);
        if buf.windows(witness.len()).any(|w| w == witness) {
            break;
        }
    }
    assert!(
        !buf.windows(EXPORT_WITNESS_SECRET.len())
            .any(|w| w == EXPORT_WITNESS_SECRET.as_bytes()),
        "aborted client must not receive trailing document secret"
    );

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn member_self_remove_stays_forbidden() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let (status, body) = json_request(
        app,
        "DELETE",
        &format!("/api/v1/workspaces/{workspace_id}/members/{owner_id}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body:?}");
    assert_eq!(body["code"], "workspace_member_self_change_forbidden");
    harness.cleanup().await;
}
