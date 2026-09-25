#![cfg(feature = "db-tests")]
#![allow(dead_code)]

#[path = "support/project_harness.rs"]
mod project_harness;

use axum::http::StatusCode;
use chrono::{Duration as ChronoDuration, Utc};
use project_harness::{
    add_workspace_user, admin_pool, app_pool, create_project, http_request,
    insert_stored_attachment, json_request, setup_session, TestDb,
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
