#![cfg(feature = "db-tests")]
#![allow(dead_code)]

#[path = "support/project_harness.rs"]
mod project_harness;

use axum::http::StatusCode;
use project_harness::{
    add_workspace_user, admin_pool, app_pool, create_project, http_request, json_request,
    setup_session, TestDb,
};
use serde_json::{json, Value};
use uuid::Uuid;

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

#[tokio::test]
async fn groups_crud_membership_project_grant_and_user_id_members() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, workspace_id, "member", "grp-member").await;
    let project = create_project(app.clone(), &cookie, workspace_id, "GRP", "private").await;
    let project_id = project["id"].as_str().unwrap();
    let events_before: (i64,) = sqlx::query_as("SELECT count(*) FROM fvoci.events")
        .fetch_one(&admin)
        .await
        .unwrap();

    let (status, created) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/groups"),
        Some(json!({"name": "랩팀"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created:?}");
    assert_eq!(created["name"], "랩팀");
    let group_id = created["id"].as_str().unwrap().to_string();

    let events_after: (i64,) = sqlx::query_as("SELECT count(*) FROM fvoci.events")
        .fetch_one(&admin)
        .await
        .unwrap();
    assert_eq!(
        events_after.0, events_before.0,
        "source has no group events"
    );

    let (status, listed) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/groups"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(listed["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["id"] == group_id));

    let (status, member_create) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/groups"),
        Some(json!({"name": "멤버불가"})),
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{member_create:?}");

    let (status, add) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/groups/{group_id}/members"),
        Some(json!({"userId": member.user_id.to_string()})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{add:?}");

    let (status, members) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/groups/{group_id}/members"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(members["items"][0]["userId"], member.user_id.to_string());

    let (status, grant) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/groups"),
        Some(json!({"groupId": group_id, "role": "viewer"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{grant:?}");

    let (status, grants) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/groups"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(grants["items"][0]["groupId"], group_id);
    assert_eq!(grants["items"][0]["role"], "viewer");

    let (status, project_get) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}"),
        None,
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{project_get:?}");

    let (status, project_members) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/members"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let user_ids: Vec<&str> = project_members["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| item["userId"].as_str())
        .collect();
    assert!(user_ids.iter().all(|id| Uuid::parse_str(id).is_ok()));
    assert!(!user_ids.contains(&member.user_id.to_string().as_str()));
    assert!(user_ids.contains(&owner_id.to_string().as_str()));

    let (status, ungrant) = json_request(
        app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/groups"),
        Some(json!({"groupId": group_id})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{ungrant:?}");

    let (status, after) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}"),
        None,
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{after:?}");

    let (status, removed) = json_request(
        app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{workspace_id}/groups/{group_id}/members"),
        Some(json!({"userId": member.user_id.to_string()})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{removed:?}");

    let (status, deleted) = json_request(
        app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{workspace_id}/groups/{group_id}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{deleted:?}");

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn groups_missing_unauth_and_extra_keys() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;

    let (status, missing) = json_request(
        app.clone(),
        "GET",
        &format!(
            "/api/v1/workspaces/{workspace_id}/groups/00000000-0000-7000-8000-000000000001/members"
        ),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{missing:?}");

    let (status, unauth) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/groups"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{unauth:?}");

    let (status, bad) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/groups"),
        Some(json!({"name": "오염", "evil": 1})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{bad:?}");

    harness.cleanup().await;
}

#[tokio::test]
async fn wiki_document_group_grants_and_project_document_404() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let project = create_project(app.clone(), &cookie, workspace_id, "GRP", "private").await;
    let project_id = project["id"].as_str().unwrap();
    let root_id = project["rootDocumentId"].as_str().unwrap();

    let (status, wiki) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"parentId": null, "title": "위키"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{wiki:?}");
    let wiki_id = wiki["id"].as_str().unwrap();

    let (status, created) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/groups"),
        Some(json!({"name": "위키뷰어"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created:?}");
    let group_id = created["id"].as_str().unwrap();

    let guest = add_workspace_user(&admin, workspace_id, "guest", "grp-guest").await;
    let (status, before) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{wiki_id}"),
        None,
        Some(&guest.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{before:?}");

    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/groups/{group_id}/members"),
        Some(json!({"userId": guest.user_id.to_string()})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, grant) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{wiki_id}/groups"),
        Some(json!({"groupId": group_id, "role": "viewer"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{grant:?}");

    let (status, grants) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{wiki_id}/groups"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(grants["items"][0]["groupId"], group_id);
    assert_eq!(grants["items"][0]["role"], "viewer");

    let (status, opened) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{wiki_id}"),
        None,
        Some(&guest.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{opened:?}");

    let (status, guest_list) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{wiki_id}/groups"),
        None,
        Some(&guest.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{guest_list:?}");

    let (status, comments) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{wiki_id}/comments"),
        None,
        Some(&guest.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{comments:?}");

    let (status, write_comment) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{wiki_id}/comments"),
        Some(json!({"body": "불가"})),
        Some(&guest.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{write_comment:?}");

    let (status, revisions) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{wiki_id}/revisions"),
        None,
        Some(&guest.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{revisions:?}");

    let (status, unauth) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{wiki_id}/groups"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{unauth:?}");

    let outsider = add_workspace_user(&admin, workspace_id, "guest", "outsider-tmp").await;
    sqlx::query("DELETE FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2")
        .bind(workspace_id)
        .bind(outsider.user_id)
        .execute(&admin)
        .await
        .unwrap();
    let (status, outsider_get) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{wiki_id}/groups"),
        None,
        Some(&outsider.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{outsider_get:?}");

    let (status, project_grant) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{root_id}/groups"),
        Some(json!({"groupId": group_id, "role": "viewer"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{project_grant:?}");

    let (status, project_list) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{root_id}/groups"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{project_list:?}");

    let (status, project_groups) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/groups"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(project_groups["items"].as_array().unwrap().len(), 0);

    let (status, revoked) = json_request(
        app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{wiki_id}/groups"),
        Some(json!({"groupId": group_id})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{revoked:?}");

    let (status, closed) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{wiki_id}"),
        None,
        Some(&guest.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{closed:?}");

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn workspace_leave_and_group_delete_cascade_grants() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, workspace_id, "member", "cascade").await;
    let (status, created) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/groups"),
        Some(json!({"name": "캐스케이드"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let group_id = Uuid::parse_str(created["id"].as_str().unwrap()).unwrap();
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/groups/{group_id}/members"),
        Some(json!({"userId": member.user_id.to_string()})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let project = create_project(app.clone(), &cookie, workspace_id, "CAS", "private").await;
    let project_id = Uuid::parse_str(project["id"].as_str().unwrap()).unwrap();
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/groups"),
        Some(json!({"groupId": group_id.to_string(), "role": "viewer"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, _) = json_request(
        app.clone(),
        "DELETE",
        &format!(
            "/api/v1/workspaces/{workspace_id}/members/{}",
            member.user_id
        ),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let remaining: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.group_members WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(workspace_id)
    .bind(member.user_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(remaining.0, 0);

    let (status, _) = json_request(
        app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{workspace_id}/groups/{group_id}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let grants: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.project_members WHERE workspace_id = $1 AND group_id = $2",
    )
    .bind(workspace_id)
    .bind(group_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(grants.0, 0);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn last_private_lead_group_delete_conflicts() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let project = create_project(app.clone(), &cookie, workspace_id, "LEAD", "private").await;
    let project_id = project["id"].as_str().unwrap();
    let (status, created) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/groups"),
        Some(json!({"name": "책임그룹"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let group_id = created["id"].as_str().unwrap();
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/groups"),
        Some(json!({"groupId": group_id, "role": "lead"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, _) = json_request(
        app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/members/{owner_id}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = json_request(
        app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{workspace_id}/groups/{group_id}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body:?}");

    harness.cleanup().await;
}

#[tokio::test]
async fn groups_rls_force_and_app_role_grants() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let (status, created) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/groups"),
        Some(json!({"name": "격리"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created:?}");

    let flags: Vec<(String, bool, bool)> = sqlx::query_as(
        r#"
        SELECT c.relname, c.relrowsecurity, c.relforcerowsecurity
        FROM pg_class c
        JOIN pg_namespace n ON n.oid = c.relnamespace
        WHERE n.nspname = 'fvoci'
          AND c.relname IN ('groups', 'group_members', 'document_members')
        ORDER BY c.relname
        "#,
    )
    .fetch_all(&admin)
    .await
    .unwrap();
    assert_eq!(flags.len(), 3);
    for (name, rls, force) in &flags {
        assert!(rls, "{name} missing RLS");
        assert!(force, "{name} missing FORCE RLS");
    }

    let app_db = app_pool(&harness).await;
    let visible: (i64,) = sqlx::query_as("SELECT count(*) FROM fvoci.groups")
        .fetch_one(&app_db)
        .await
        .unwrap();
    assert_eq!(visible.0, 0);

    admin.close().await;
    app_db.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn group_pat_scopes_match_source() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let project = create_project(app.clone(), &cookie, workspace_id, "PAT", "workspace").await;
    let project_id = project["id"].as_str().unwrap();

    let (status, manage) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/api-tokens"),
        Some(json!({"name": "ws", "scopes": ["workspace.manage"]})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{manage:?}");
    let manage_token = manage["token"].as_str().unwrap();

    let (status, created) = bearer(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/groups"),
        Some(json!({"name": "토큰그룹"})),
        manage_token,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created:?}");
    let group_id = created["id"].as_str().unwrap();

    let (status, projects_only) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/api-tokens"),
        Some(json!({"name": "pr", "scopes": ["projects.read"]})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let projects_token = projects_only["token"].as_str().unwrap();
    let (status, denied) = bearer(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/groups"),
        Some(json!({"name": "거부"})),
        projects_token,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{denied:?}");

    let (status, listed) = bearer(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/groups"),
        None,
        projects_token,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{listed:?}");

    let (status, grant_denied) = bearer(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/groups"),
        Some(json!({"groupId": group_id, "role": "viewer"})),
        projects_token,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{grant_denied:?}");

    let (status, wiki) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"parentId": null, "title": "토큰위키"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let wiki_id = wiki["id"].as_str().unwrap();

    let (status, docs) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/api-tokens"),
        Some(json!({"name": "docs", "scopes": ["documents.write"]})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let docs_token = docs["token"].as_str().unwrap();
    let (status, session_only) = bearer(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{wiki_id}/groups"),
        Some(json!({"groupId": group_id, "role": "viewer"})),
        docs_token,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{session_only:?}");

    let (status, list_ok) = bearer(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{wiki_id}/groups"),
        None,
        docs_token,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{list_ok:?}");

    harness.cleanup().await;
}
