#![cfg(feature = "db-tests")]
#![allow(dead_code)]

//! Task time entries, clone, backlinks, purge, flat `/tasks/:id` routes,
//! parent candidates, workspace task/status lists and workflow statuses
//! against real PostgreSQL through the non-superuser app role.

#[path = "support/project_harness.rs"]
mod project_harness;

use std::time::Duration;

use axum::http::StatusCode;
use project_harness::{
    add_workspace_user, admin_pool, count_rows, create_project, hold_membership_user_lock,
    http_request, json_request, setup_session, wait_for_advisory_blocked_by,
    wait_for_user_for_update_blocked, TestDb,
};
use serde_json::{json, Value};
use url::form_urlencoded;
use uuid::Uuid;

async fn create_task(
    app: axum::Router,
    cookie: &str,
    workspace_id: Uuid,
    project_id: &str,
    body: Value,
) -> Value {
    let (status, task) = json_request(
        app,
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(body),
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    task
}

/// An API token for `user_id` in `workspace_id`. Only workspace admins may
/// create tokens through the API, so non-admin holders are a direct fixture.
async fn api_token(
    admin: &sqlx::PgPool,
    user_id: Uuid,
    workspace_id: Uuid,
    scopes: &[&str],
) -> String {
    let token = fvoci_server::auth::token::new_token();
    let scopes: Vec<String> = scopes.iter().map(|s| s.to_string()).collect();
    sqlx::query(
        "INSERT INTO fvoci.api_tokens (id, workspace_id, user_id, token_hash, name, scopes) VALUES ($1, $2, $3, $4, 'pat', $5)",
    )
    .bind(Uuid::now_v7())
    .bind(workspace_id)
    .bind(user_id)
    .bind(&token.hash)
    .bind(&scopes)
    .execute(admin)
    .await
    .expect("insert api token");
    token.token
}

async fn bearer_request(
    app: axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    token: &str,
) -> (StatusCode, Value) {
    let auth = format!("Bearer {token}");
    let (status, json, _) = http_request(
        app,
        method,
        path,
        body.map(|b| b.to_string().into_bytes()),
        Some("application/json"),
        None,
        &[("authorization", auth.as_str())],
    )
    .await;
    (status, json)
}

async fn add_project_member(
    app: axum::Router,
    cookie: &str,
    workspace_id: Uuid,
    project_id: &str,
    user_id: Uuid,
    role: &str,
) {
    let (status, body) = json_request(
        app,
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/members"),
        Some(json!({"userId": user_id.to_string(), "role": role})),
        Some(cookie),
    )
    .await;
    assert!(status.is_success(), "{status} {body}");
}

async fn workflow(app: axum::Router, cookie: &str, workspace_id: Uuid, project_id: &str) -> Value {
    let (status, workflow) = json_request(
        app,
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/workflow"),
        None,
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    workflow
}

async fn insert_workspace(admin: &sqlx::PgPool) -> Uuid {
    let id = Uuid::now_v7();
    let slug = format!("w-{}", &id.simple().to_string()[20..]);
    sqlx::query("INSERT INTO fvoci.workspaces (id, slug, name) VALUES ($1, $2, 'Other')")
        .bind(id)
        .bind(slug)
        .execute(admin)
        .await
        .unwrap();
    id
}

async fn wait_for_project_row_waiters(admin: &sqlx::PgPool, expected: i64) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        let waiting: i64 = sqlx::query_scalar(
            r#"
            SELECT count(*)
            FROM pg_stat_activity
            WHERE datname = current_database()
              AND wait_event_type = 'Lock'
              AND state = 'active'
              AND query ILIKE '%FROM fvoci.projects%FOR NO KEY UPDATE%'
            "#,
        )
        .fetch_one(admin)
        .await
        .unwrap();
        if waiting >= expected {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("expected {expected} project row lock waiters");
}

fn encode(raw: &str) -> String {
    form_urlencoded::byte_serialize(raw.as_bytes()).collect()
}

async fn assert_rls_forced(admin: &sqlx::PgPool, table: &str) {
    let (rls, force): (bool, bool) = sqlx::query_as(
        r#"
        SELECT c.relrowsecurity, c.relforcerowsecurity
        FROM pg_class c
        JOIN pg_namespace n ON n.oid = c.relnamespace
        WHERE n.nspname = 'fvoci' AND c.relname = $1
        "#,
    )
    .bind(table)
    .fetch_one(admin)
    .await
    .unwrap();
    assert!(rls && force, "{table} RLS {rls} FORCE {force}");
}

// ---------------------------------------------------------------------------
// Time entries
// ---------------------------------------------------------------------------

#[tokio::test]
async fn time_entries_contract_permissions_and_open_entry_rule() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, _owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lead = add_workspace_user(&admin, workspace_id, "member", "lead").await;
    let viewer = add_workspace_user(&admin, workspace_id, "member", "viewer").await;
    let outsider = add_workspace_user(&admin, workspace_id, "member", "outsider").await;
    assert_rls_forced(&admin, "time_entries").await;

    let project = create_project(app.clone(), &lead.cookie, workspace_id, "TIME", "private").await;
    let project_id = project["id"].as_str().unwrap();
    add_project_member(
        app.clone(),
        &lead.cookie,
        workspace_id,
        project_id,
        viewer.user_id,
        "viewer",
    )
    .await;
    let task = create_task(
        app.clone(),
        &lead.cookie,
        workspace_id,
        project_id,
        json!({"title": "Track me"}),
    )
    .await;
    let task_id = task["id"].as_str().unwrap();
    let base = format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/time-entries");

    let (status, created) = json_request(
        app.clone(),
        "POST",
        &base,
        Some(json!({
            "startedAt": "2026-09-01T09:00:00.000Z",
            "endedAt": "2026-09-01T09:01:30.900Z",
            "note": "pairing"
        })),
        Some(&lead.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["durationSeconds"], 90);
    assert_eq!(created["note"], "pairing");
    assert_eq!(created["userId"], lead.user_id.to_string());
    assert_eq!(created["taskId"], task_id);
    assert_eq!(created["workspaceId"], workspace_id.to_string());

    let (status, open) = json_request(
        app.clone(),
        "POST",
        &base,
        Some(json!({"startedAt": "2026-09-02T09:00:00Z"})),
        Some(&lead.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{open}");
    assert!(open["endedAt"].is_null());
    assert!(open["durationSeconds"].is_null());
    assert!(open["note"].is_null());

    // One open entry per actor in the workspace.
    let (status, conflict) = json_request(
        app.clone(),
        "POST",
        &base,
        Some(json!({"startedAt": "2026-09-03T09:00:00Z"})),
        Some(&lead.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(conflict["code"], "open_time_entry_exists");

    for bad in [
        json!({"startedAt": "2026-09-01T09:00:00Z", "endedAt": "2026-09-01T09:00:00Z"}),
        json!({"startedAt": "2026-09-01T09:00:00Z", "endedAt": "2026-09-01T09:00:00.500Z"}),
        json!({"startedAt": "2026-09-01T09:00:00Z", "endedAt": "2026-09-01T08:00:00Z"}),
        json!({"startedAt": "2026-09-01T09:00:00+09:00"}),
        json!({"startedAt": "2026-09-01"}),
        json!({"startedAt": "2026-09-01T09:00:00Z", "note": "x".repeat(2001)}),
        json!({"startedAt": "2026-09-01T09:00:00Z", "endedAt": null}),
        json!({"startedAt": "2026-09-01T09:00:00Z", "extra": 1}),
        json!({}),
    ] {
        let (status, body) = json_request(
            app.clone(),
            "POST",
            &base,
            Some(bad.clone()),
            Some(&viewer.cookie),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad} {body}");
    }

    let (status, list) = json_request(app.clone(), "GET", &base, None, Some(&lead.cookie)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["canCreate"], true);
    let items = list["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    // Newest start first.
    assert_eq!(items[0]["id"], open["id"]);
    assert_eq!(items[1]["id"], created["id"]);

    let (status, rollup) = json_request(
        app.clone(),
        "GET",
        &format!("{base}/rollup"),
        None,
        Some(&lead.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rollup, json!({"totalSeconds": 90, "open": true}));

    // A viewer reads but cannot create; the source masks it as not found.
    let (status, list) = json_request(app.clone(), "GET", &base, None, Some(&viewer.cookie)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["canCreate"], false);
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &base,
        Some(json!({"startedAt": "2026-09-01T10:00:00Z", "endedAt": "2026-09-01T11:00:00Z"})),
        Some(&viewer.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Not a project member of the private project: hidden.
    for path in [base.clone(), format!("{base}/rollup")] {
        let (status, _) =
            json_request(app.clone(), "GET", &path, None, Some(&outsider.cookie)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
    }
    // The workspace owner is not a member of the private project either.
    let (status, _) = json_request(app.clone(), "GET", &base, None, Some(&owner_cookie)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Archived task: read-only.
    let (status, _) = json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}"),
        Some(json!({"archived": true})),
        Some(&lead.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, list) = json_request(app.clone(), "GET", &base, None, Some(&lead.cookie)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["canCreate"], false);
    let (status, body) = json_request(
        app.clone(),
        "POST",
        &base,
        Some(json!({"startedAt": "2026-09-05T10:00:00Z", "endedAt": "2026-09-05T11:00:00Z"})),
        Some(&lead.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "task_archived");

    // The app role sees no row without a tenant context.
    let app_pool = project_harness::app_pool(&harness).await;
    let visible: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.time_entries")
        .fetch_one(&app_pool)
        .await
        .unwrap();
    assert_eq!(visible, 0);
    assert_eq!(count_rows(&admin, "time_entries").await, 2);
    app_pool.close().await;

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn time_entry_api_token_scopes() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let project = create_project(app.clone(), &cookie, workspace_id, "PAT", "workspace").await;
    let project_id = project["id"].as_str().unwrap();
    let task = create_task(
        app.clone(),
        &cookie,
        workspace_id,
        project_id,
        json!({"title": "Token"}),
    )
    .await;
    let task_id = task["id"].as_str().unwrap();
    let base = format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/time-entries");
    let read = api_token(&admin, owner_id, workspace_id, &["tasks.read"]).await;
    let write = api_token(&admin, owner_id, workspace_id, &["tasks.write"]).await;
    let projects = api_token(&admin, owner_id, workspace_id, &["projects.read"]).await;
    let entry = json!({"startedAt": "2026-09-01T09:00:00Z", "endedAt": "2026-09-01T10:00:00Z"});

    let (status, _) = bearer_request(app.clone(), "POST", &base, Some(entry.clone()), &read).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = bearer_request(app.clone(), "GET", &base, None, &projects).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) = bearer_request(app.clone(), "POST", &base, Some(entry), &write).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (status, list) = bearer_request(app.clone(), "GET", &base, None, &read).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    let (status, rollup) =
        bearer_request(app.clone(), "GET", &format!("{base}/rollup"), None, &read).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rollup, json!({"totalSeconds": 3600, "open": false}));

    admin.close().await;
    harness.cleanup().await;
}

/// The session is revoked after HTTP authentication but before the write
/// transaction rechecks it: no entry is written.
#[tokio::test]
async fn time_entry_create_rechecks_a_revoked_session() {
    let harness = TestDb::bootstrap().await;
    let (app, _, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, workspace_id, "member", "member").await;
    let project = create_project(
        app.clone(),
        &member.cookie,
        workspace_id,
        "RACE",
        "workspace",
    )
    .await;
    let project_id = project["id"].as_str().unwrap();
    let task = create_task(
        app.clone(),
        &member.cookie,
        workspace_id,
        project_id,
        json!({"title": "Race"}),
    )
    .await;
    let task_id = task["id"].as_str().unwrap().to_string();

    let mut barrier = admin.begin().await.unwrap();
    sqlx::query("SELECT id FROM fvoci.users WHERE id = $1 FOR UPDATE")
        .bind(member.user_id)
        .fetch_one(&mut *barrier)
        .await
        .unwrap();
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *barrier)
        .await
        .unwrap();
    let request = tokio::spawn({
        let app = app.clone();
        let cookie = member.cookie.clone();
        async move {
            json_request(
                app,
                "POST",
                &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/time-entries"),
                Some(json!({"startedAt": "2026-09-01T09:00:00Z"})),
                Some(&cookie),
            )
            .await
        }
    });
    wait_for_user_for_update_blocked(&admin, blocker_pid).await;
    sqlx::query("UPDATE fvoci.sessions SET revoked_at = now() WHERE user_id = $1")
        .bind(member.user_id)
        .execute(&mut *barrier)
        .await
        .unwrap();
    barrier.commit().await.unwrap();
    let (status, _) = tokio::time::timeout(Duration::from_secs(10), request)
        .await
        .expect("request finished")
        .expect("join");
    assert!(
        status == StatusCode::UNAUTHORIZED || status == StatusCode::NOT_FOUND,
        "{status}"
    );
    assert_eq!(count_rows(&admin, "time_entries").await, 0);

    admin.close().await;
    harness.cleanup().await;
}

// ---------------------------------------------------------------------------
// Clone
// ---------------------------------------------------------------------------

#[tokio::test]
async fn clone_copies_metadata_assignees_and_labels_in_the_same_project() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let project = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let project_id = project["id"].as_str().unwrap();
    let workflow = workflow(app.clone(), &cookie, workspace_id, project_id).await;
    let todo = workflow["statuses"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["category"] == "todo")
        .unwrap()["id"]
        .clone();
    let (status, label) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/labels"),
        Some(json!({"name": "ui", "color": "blue"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{label}");
    let epic = create_task(
        app.clone(),
        &cookie,
        workspace_id,
        project_id,
        json!({"title": "Epic", "type": "epic"}),
    )
    .await;
    let source = create_task(
        app.clone(),
        &cookie,
        workspace_id,
        project_id,
        json!({
            "title": "Fix login",
            "type": "bug",
            "priority": "high",
            "statusId": todo,
            "startDate": "2026-09-01",
            "dueDate": "2026-09-03",
            "parentId": epic["id"],
        }),
    )
    .await;
    let source_id = source["id"].as_str().unwrap();
    let (status, _) = json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{source_id}"),
        Some(json!({
            "assigneeIds": [owner_id.to_string()],
            "labelIds": [label["id"]],
            "estimate": "3",
        })),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let events_before = count_rows(&admin, "events").await;

    let (status, copy) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{source_id}/clone"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{copy}");
    assert_eq!(copy["title"], "Fix login 복사");
    assert_eq!(copy["type"], "bug");
    assert_eq!(copy["priority"], "high");
    assert_eq!(copy["statusId"], todo);
    assert_eq!(copy["startDate"], "2026-09-01");
    assert_eq!(copy["dueDate"], "2026-09-03");
    assert_eq!(copy["parentId"], epic["id"]);
    assert_eq!(copy["projectId"], project_id);
    assert!(copy["estimate"].is_null());
    assert!(copy["recurrence"].is_null());
    // Number 1 is the project's root document.
    assert_eq!(copy["number"], 4);
    assert_eq!(copy["displayId"], "LAB-4");
    assert_ne!(copy["id"], source["id"]);

    let copy_id = copy["id"].as_str().unwrap();
    let (status, detail) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{copy_id}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["assigneeIds"], json!([owner_id.to_string()]));
    assert_eq!(detail["labelIds"], json!([label["id"]]));
    assert!(count_rows(&admin, "events").await > events_before);
    let created_events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM fvoci.events WHERE verb = 'task.created' AND target_id = $1",
    )
    .bind(Uuid::parse_str(copy_id).unwrap())
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(created_events, 1);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn clone_needs_create_rights_a_writable_project_and_task() {
    let harness = TestDb::bootstrap().await;
    let (app, _owner_cookie, _owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lead = add_workspace_user(&admin, workspace_id, "member", "lead").await;
    let viewer = add_workspace_user(&admin, workspace_id, "member", "viewer").await;
    let guest = add_workspace_user(&admin, workspace_id, "guest", "guest").await;
    let project = create_project(app.clone(), &lead.cookie, workspace_id, "CLN", "private").await;
    let project_id = project["id"].as_str().unwrap();
    add_project_member(
        app.clone(),
        &lead.cookie,
        workspace_id,
        project_id,
        viewer.user_id,
        "viewer",
    )
    .await;
    let task = create_task(
        app.clone(),
        &lead.cookie,
        workspace_id,
        project_id,
        json!({"title": "Original"}),
    )
    .await;
    let task_id = task["id"].as_str().unwrap();
    let clone_path = format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/clone");
    let tasks_before = count_rows(&admin, "tasks").await;

    // A read grant is not enough to create the copy.
    let (status, _) =
        json_request(app.clone(), "POST", &clone_path, None, Some(&viewer.cookie)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) =
        json_request(app.clone(), "POST", &clone_path, None, Some(&guest.cookie)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // A tasks.read token of the lead is refused before any write.
    let read = api_token(&admin, lead.user_id, workspace_id, &["tasks.read"]).await;
    let (status, _) = bearer_request(app.clone(), "POST", &clone_path, None, &read).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(count_rows(&admin, "tasks").await, tasks_before);

    let write = api_token(&admin, lead.user_id, workspace_id, &["tasks.write"]).await;
    let (status, copy) = bearer_request(app.clone(), "POST", &clone_path, None, &write).await;
    assert_eq!(status, StatusCode::OK, "{copy}");
    let channel: String = sqlx::query_scalar(
        "SELECT channel FROM fvoci.task_activity WHERE task_id = $1 AND kind = 'created'",
    )
    .bind(Uuid::parse_str(copy["id"].as_str().unwrap()).unwrap())
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(channel, "api");

    let (status, _) = json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}"),
        Some(json!({"archived": true})),
        Some(&lead.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) =
        json_request(app.clone(), "POST", &clone_path, None, Some(&lead.cookie)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "task_archived");

    sqlx::query("UPDATE fvoci.projects SET status = 'archived' WHERE id = $1")
        .bind(Uuid::parse_str(project_id).unwrap())
        .execute(&admin)
        .await
        .unwrap();
    let tasks_before = count_rows(&admin, "tasks").await;
    let (status, body) = json_request(
        app.clone(),
        "POST",
        &format!(
            "/api/v1/workspaces/{workspace_id}/tasks/{}/clone",
            copy["id"].as_str().unwrap()
        ),
        None,
        Some(&lead.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "project_archived");
    assert_eq!(count_rows(&admin, "tasks").await, tasks_before);
    admin.close().await;
    harness.cleanup().await;
}

/// The actor loses the project grant while the clone waits on its
/// membership lock; the transaction's recheck refuses the copy.
#[tokio::test]
async fn clone_rechecks_a_grant_revoked_after_http_authentication() {
    let harness = TestDb::bootstrap().await;
    let (app, _owner_cookie, _owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lead = add_workspace_user(&admin, workspace_id, "member", "lead").await;
    let editor = add_workspace_user(&admin, workspace_id, "member", "editor").await;
    let project = create_project(app.clone(), &lead.cookie, workspace_id, "REV", "private").await;
    let project_id = project["id"].as_str().unwrap();
    add_project_member(
        app.clone(),
        &lead.cookie,
        workspace_id,
        project_id,
        editor.user_id,
        "member",
    )
    .await;
    let task = create_task(
        app.clone(),
        &lead.cookie,
        workspace_id,
        project_id,
        json!({"title": "Secret"}),
    )
    .await;
    let task_id = task["id"].as_str().unwrap().to_string();
    let tasks_before = count_rows(&admin, "tasks").await;

    let mut barrier = admin.begin().await.unwrap();
    hold_membership_user_lock(&mut barrier, editor.user_id).await;
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *barrier)
        .await
        .unwrap();
    let request = tokio::spawn({
        let app = app.clone();
        let cookie = editor.cookie.clone();
        async move {
            json_request(
                app,
                "POST",
                &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/clone"),
                None,
                Some(&cookie),
            )
            .await
        }
    });
    wait_for_advisory_blocked_by(&admin, blocker_pid).await;
    sqlx::query("DELETE FROM fvoci.project_members WHERE user_id = $1")
        .bind(editor.user_id)
        .execute(&mut *barrier)
        .await
        .unwrap();
    barrier.commit().await.unwrap();
    let (status, _) = tokio::time::timeout(Duration::from_secs(10), request)
        .await
        .expect("clone finished")
        .expect("join");
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(count_rows(&admin, "tasks").await, tasks_before);

    admin.close().await;
    harness.cleanup().await;
}

// ---------------------------------------------------------------------------
// Backlinks
// ---------------------------------------------------------------------------

fn mention_body(task_id: &str) -> Value {
    json!({
        "type": "doc",
        "content": [{
            "type": "paragraph",
            "content": [{"type": "mention", "attrs": {"entity": "task", "id": task_id.to_uppercase()}}]
        }]
    })
}

#[tokio::test]
async fn backlinks_list_only_live_items_the_actor_can_view() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, workspace_id, "member", "member").await;
    let lab = create_project(app.clone(), &cookie, workspace_id, "LAB", "workspace").await;
    let lab_id = lab["id"].as_str().unwrap();
    let hidden = create_project(app.clone(), &cookie, workspace_id, "HID", "private").await;
    let hidden_id = hidden["id"].as_str().unwrap();
    let target = create_task(
        app.clone(),
        &cookie,
        workspace_id,
        lab_id,
        json!({"title": "Target"}),
    )
    .await;
    let target_id = target["id"].as_str().unwrap();
    let referrer = create_task(
        app.clone(),
        &cookie,
        workspace_id,
        lab_id,
        json!({"title": "Referrer"}),
    )
    .await;
    let private_referrer = create_task(
        app.clone(),
        &cookie,
        workspace_id,
        hidden_id,
        json!({"title": "Private referrer"}),
    )
    .await;
    let trashed_referrer = create_task(
        app.clone(),
        &cookie,
        workspace_id,
        lab_id,
        json!({"title": "Trashed referrer"}),
    )
    .await;
    let (status, wiki) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"parentId": null, "title": "Wiki page"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{wiki}");
    let body = mention_body(target_id);
    for id in [
        referrer["id"].as_str().unwrap(),
        private_referrer["id"].as_str().unwrap(),
        trashed_referrer["id"].as_str().unwrap(),
        target_id,
    ] {
        sqlx::query("UPDATE fvoci.tasks SET content_json = $2 WHERE id = $1")
            .bind(Uuid::parse_str(id).unwrap())
            .bind(&body)
            .execute(&admin)
            .await
            .unwrap();
    }
    sqlx::query("UPDATE fvoci.documents SET content_json = $2 WHERE id = $1")
        .bind(Uuid::parse_str(wiki["id"].as_str().unwrap()).unwrap())
        .bind(&body)
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query("UPDATE fvoci.tasks SET deleted_at = now() WHERE id = $1")
        .bind(Uuid::parse_str(trashed_referrer["id"].as_str().unwrap()).unwrap())
        .execute(&admin)
        .await
        .unwrap();

    let path = format!("/api/v1/workspaces/{workspace_id}/tasks/{target_id}/backlinks");
    let (status, owner_view) = json_request(app.clone(), "GET", &path, None, Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK);
    let mut kinds: Vec<(String, String)> = owner_view["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            assert_eq!(item["id"], item["from"]["id"]);
            (
                item["from"]["type"].as_str().unwrap().to_string(),
                item["from"]["title"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    kinds.sort();
    assert_eq!(
        kinds,
        vec![
            ("document".to_string(), "Wiki page".to_string()),
            ("task".to_string(), "Private referrer".to_string()),
            ("task".to_string(), "Referrer".to_string()),
        ]
    );
    let wiki_item = owner_view["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["from"]["type"] == "document")
        .unwrap();
    assert_eq!(wiki_item["from"]["displayId"], "WIKI-1");

    // The member cannot see the private project's referrer.
    let (status, member_view) =
        json_request(app.clone(), "GET", &path, None, Some(&member.cookie)).await;
    assert_eq!(status, StatusCode::OK);
    let titles: Vec<&str> = member_view["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["from"]["title"].as_str().unwrap())
        .collect();
    assert!(!titles.contains(&"Private referrer"), "{titles:?}");
    assert_eq!(titles.len(), 2);
    let referrer_item = member_view["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["from"]["title"] == "Referrer")
        .unwrap();
    assert_eq!(referrer_item["from"]["displayId"], "LAB-3");

    // A task in a hidden project is itself hidden.
    let hidden_path = format!(
        "/api/v1/workspaces/{workspace_id}/tasks/{}/backlinks",
        private_referrer["id"].as_str().unwrap()
    );
    let (status, _) =
        json_request(app.clone(), "GET", &hidden_path, None, Some(&member.cookie)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    admin.close().await;
    harness.cleanup().await;
}

// ---------------------------------------------------------------------------
// Purge and flat routes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn purge_deletes_the_task_detaches_children_and_journals_attachments() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let viewer = add_workspace_user(&admin, workspace_id, "member", "viewer").await;
    let project = create_project(app.clone(), &cookie, workspace_id, "PRG", "private").await;
    let project_id = project["id"].as_str().unwrap();
    add_project_member(
        app.clone(),
        &cookie,
        workspace_id,
        project_id,
        viewer.user_id,
        "viewer",
    )
    .await;
    let parent = create_task(
        app.clone(),
        &cookie,
        workspace_id,
        project_id,
        json!({"title": "Parent", "type": "task"}),
    )
    .await;
    let parent_id = parent["id"].as_str().unwrap();
    let child = create_task(
        app.clone(),
        &cookie,
        workspace_id,
        project_id,
        json!({"title": "Child", "type": "subtask", "parentId": parent_id}),
    )
    .await;
    let child_id = child["id"].as_str().unwrap();
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{parent_id}/time-entries"),
        Some(json!({"startedAt": "2026-09-01T09:00:00Z", "endedAt": "2026-09-01T10:00:00Z"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let attachment_id = Uuid::now_v7();
    let storage_key = format!("objects/{}", Uuid::now_v7());
    sqlx::query(
        r#"
        INSERT INTO fvoci.attachments (
            id, workspace_id, task_id, uploader_id, status, name, reserved_size_bytes,
            size_bytes, storage_key, completed_at
        ) VALUES ($1, $2, $3, $4, 'stored', 'probe.bin', 4, 4, $5, now())
        "#,
    )
    .bind(attachment_id)
    .bind(workspace_id)
    .bind(Uuid::parse_str(parent_id).unwrap())
    .bind(owner_id)
    .bind(&storage_key)
    .execute(&admin)
    .await
    .unwrap();

    let path = format!("/api/v1/workspaces/{workspace_id}/tasks/{parent_id}");
    // Irreversible: never through an API token, even with tasks.write.
    let write = api_token(&admin, owner_id, workspace_id, &["tasks.write"]).await;
    let (status, _) = bearer_request(app.clone(), "DELETE", &path, None, &write).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = json_request(app.clone(), "DELETE", &path, None, Some(&viewer.cookie)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(count_rows(&admin, "tasks").await, 2);

    let (status, body) = json_request(app.clone(), "DELETE", &path, None, Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, json!({"ok": true}));
    assert_eq!(count_rows(&admin, "tasks").await, 1);
    assert_eq!(count_rows(&admin, "time_entries").await, 0);
    assert_eq!(count_rows(&admin, "attachments").await, 0);
    let journaled: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM fvoci.attachment_object_cleanups WHERE storage_key = $1",
    )
    .bind(&storage_key)
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(journaled, 1);
    let (child_parent, child_type): (Option<Uuid>, String) =
        sqlx::query_as("SELECT parent_id, type FROM fvoci.tasks WHERE id = $1")
            .bind(Uuid::parse_str(child_id).unwrap())
            .fetch_one(&admin)
            .await
            .unwrap();
    assert!(child_parent.is_none());
    assert_eq!(child_type, "task");
    let deleted_events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM fvoci.events WHERE verb = 'task.deleted' AND target_id = $1",
    )
    .bind(Uuid::parse_str(parent_id).unwrap())
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(deleted_events, 1);
    let child_changes: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM fvoci.task_activity WHERE task_id = $1 AND kind = 'changed'",
    )
    .bind(Uuid::parse_str(child_id).unwrap())
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(child_changes, 1);

    let (status, _) = json_request(app.clone(), "DELETE", &path, None, Some(&cookie)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // A trashed task can be purged too (source `requireTaskForWrite`).
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{child_id}/trash"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = json_request(
        app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{child_id}"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(count_rows(&admin, "tasks").await, 0);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn purge_rechecks_a_session_revoked_after_http_authentication() {
    let harness = TestDb::bootstrap().await;
    let (app, _, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, workspace_id, "member", "member").await;
    let project = create_project(
        app.clone(),
        &member.cookie,
        workspace_id,
        "PRV",
        "workspace",
    )
    .await;
    let project_id = project["id"].as_str().unwrap();
    let task = create_task(
        app.clone(),
        &member.cookie,
        workspace_id,
        project_id,
        json!({"title": "Keep"}),
    )
    .await;
    let task_id = task["id"].as_str().unwrap().to_string();

    let mut barrier = admin.begin().await.unwrap();
    sqlx::query("SELECT id FROM fvoci.users WHERE id = $1 FOR UPDATE")
        .bind(member.user_id)
        .fetch_one(&mut *barrier)
        .await
        .unwrap();
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *barrier)
        .await
        .unwrap();
    let request = tokio::spawn({
        let app = app.clone();
        let cookie = member.cookie.clone();
        async move {
            json_request(
                app,
                "DELETE",
                &format!("/api/v1/tasks/{task_id}"),
                None,
                Some(&cookie),
            )
            .await
        }
    });
    wait_for_user_for_update_blocked(&admin, blocker_pid).await;
    sqlx::query("UPDATE fvoci.sessions SET revoked_at = now() WHERE user_id = $1")
        .bind(member.user_id)
        .execute(&mut *barrier)
        .await
        .unwrap();
    barrier.commit().await.unwrap();
    let (status, _) = tokio::time::timeout(Duration::from_secs(10), request)
        .await
        .expect("purge finished")
        .expect("join");
    assert!(
        status == StatusCode::UNAUTHORIZED || status == StatusCode::NOT_FOUND,
        "{status}"
    );
    assert_eq!(count_rows(&admin, "tasks").await, 1);

    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn flat_task_routes_locate_the_workspace_for_sessions_only() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, workspace_id, "member", "member").await;
    // A user of another workspace only.
    let other_workspace = insert_workspace(&admin).await;
    let stranger = add_workspace_user(&admin, other_workspace, "owner", "stranger").await;

    let project = create_project(app.clone(), &cookie, workspace_id, "FLT", "workspace").await;
    let project_id = project["id"].as_str().unwrap();
    let hidden = create_project(app.clone(), &cookie, workspace_id, "FHD", "private").await;
    let task = create_task(
        app.clone(),
        &cookie,
        workspace_id,
        project_id,
        json!({"title": "Flat"}),
    )
    .await;
    let task_id = task["id"].as_str().unwrap();
    let hidden_task = create_task(
        app.clone(),
        &cookie,
        workspace_id,
        hidden["id"].as_str().unwrap(),
        json!({"title": "Hidden"}),
    )
    .await;
    let flat = format!("/api/v1/tasks/{task_id}");

    let (status, scoped) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}"),
        None,
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, detail) =
        json_request(app.clone(), "GET", &flat, None, Some(&member.cookie)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail, scoped);

    let (status, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/tasks/{}", hidden_task["id"].as_str().unwrap()),
        None,
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = json_request(app.clone(), "GET", &flat, None, Some(&stranger.cookie)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/tasks/{}", Uuid::now_v7()),
        None,
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = json_request(app.clone(), "GET", &flat, None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Session only: even a full-scope token is refused.
    let token = api_token(
        &admin,
        owner_id,
        workspace_id,
        &["tasks.read", "tasks.write"],
    )
    .await;
    for method in ["GET", "PATCH", "DELETE"] {
        let body = (method == "PATCH").then(|| json!({"title": "Nope"}));
        let (status, _) = bearer_request(app.clone(), method, &flat, body, &token).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method}");
    }

    let (status, patched) = json_request(
        app.clone(),
        "PATCH",
        &flat,
        Some(json!({"title": "Flat renamed"})),
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{patched}");
    assert_eq!(patched["title"], "Flat renamed");
    let (status, _) = json_request(
        app.clone(),
        "PATCH",
        &flat,
        Some(json!({"title": ""})),
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) =
        json_request(app.clone(), "DELETE", &flat, None, Some(&stranger.cookie)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) =
        json_request(app.clone(), "DELETE", &flat, None, Some(&member.cookie)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, _) = json_request(app.clone(), "GET", &flat, None, Some(&member.cookie)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    admin.close().await;
    harness.cleanup().await;
}

// ---------------------------------------------------------------------------
// Parent candidates
// ---------------------------------------------------------------------------

#[tokio::test]
async fn parent_candidates_follow_the_hierarchy_and_page() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let outsider = add_workspace_user(&admin, workspace_id, "member", "outsider").await;
    let project = create_project(app.clone(), &cookie, workspace_id, "PAR", "private").await;
    let project_id = project["id"].as_str().unwrap();
    let epic_a = create_task(
        app.clone(),
        &cookie,
        workspace_id,
        project_id,
        json!({"title": "Alpha epic", "type": "epic"}),
    )
    .await;
    let epic_b = create_task(
        app.clone(),
        &cookie,
        workspace_id,
        project_id,
        json!({"title": "Beta epic", "type": "epic"}),
    )
    .await;
    let story = create_task(
        app.clone(),
        &cookie,
        workspace_id,
        project_id,
        json!({"title": "Alpha story", "type": "story"}),
    )
    .await;
    let archived_epic = create_task(
        app.clone(),
        &cookie,
        workspace_id,
        project_id,
        json!({"title": "Old epic", "type": "epic"}),
    )
    .await;
    let (status, _) = json_request(
        app.clone(),
        "PATCH",
        &format!(
            "/api/v1/workspaces/{workspace_id}/tasks/{}",
            archived_epic["id"].as_str().unwrap()
        ),
        Some(json!({"archived": true})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let base = format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks/parents");
    let ids = |body: &Value| -> Vec<String> {
        body["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["id"].as_str().unwrap().to_string())
            .collect()
    };

    let (status, body) = json_request(
        app.clone(),
        "GET",
        &format!("{base}?childType=task"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // Most recently updated first; archived epics are not candidates.
    assert_eq!(
        ids(&body),
        vec![
            epic_b["id"].as_str().unwrap().to_string(),
            epic_a["id"].as_str().unwrap().to_string()
        ]
    );
    assert_eq!(body["items"][0]["displayId"], "PAR-3");
    assert_eq!(body["items"][0]["type"], "epic");
    assert!(body["nextCursor"].is_null());

    let (status, body) = json_request(
        app.clone(),
        "GET",
        &format!("{base}?childType=subtask&q={}", encode(" alpha ")),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ids(&body), vec![story["id"].as_str().unwrap().to_string()]);

    let (status, body) = json_request(
        app.clone(),
        "GET",
        &format!("{base}?childType=epic"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["items"].as_array().unwrap().is_empty());

    // Display ids are an exact lookup in this project only.
    let (status, body) = json_request(
        app.clone(),
        "GET",
        &format!("{base}?childType=bug&q=par-2"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ids(&body), vec![epic_a["id"].as_str().unwrap().to_string()]);
    let (status, body) = json_request(
        app.clone(),
        "GET",
        &format!(
            "{base}?childType=bug&q=PAR-2&excludeTaskId={}",
            epic_a["id"].as_str().unwrap()
        ),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["items"].as_array().unwrap().is_empty());
    let (status, body) = json_request(
        app.clone(),
        "GET",
        &format!("{base}?childType=bug&q=OTHER-2"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["items"].as_array().unwrap().is_empty());

    // Paging with a scope-bound cursor.
    let (status, first) = json_request(
        app.clone(),
        "GET",
        &format!("{base}?childType=task&limit=1"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        ids(&first),
        vec![epic_b["id"].as_str().unwrap().to_string()]
    );
    let cursor = first["nextCursor"].as_str().unwrap().to_string();
    let (status, second) = json_request(
        app.clone(),
        "GET",
        &format!("{base}?childType=task&limit=1&cursor={}", encode(&cursor)),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        ids(&second),
        vec![epic_a["id"].as_str().unwrap().to_string()]
    );
    assert!(second["nextCursor"].is_null());
    let (status, _) = json_request(
        app.clone(),
        "GET",
        &format!("{base}?childType=task&q=beta&cursor={}", encode(&cursor)),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    for bad in [
        "".to_string(),
        "?childType=chore".to_string(),
        "?childType=task&limit=21".to_string(),
        "?childType=task&limit=0".to_string(),
        "?childType=task&excludeTaskId=nope".to_string(),
        format!("?childType=task&q={}", "a".repeat(201)),
        "?childType=task&unknown=1".to_string(),
    ] {
        let (status, _) = json_request(
            app.clone(),
            "GET",
            &format!("{base}{bad}"),
            None,
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }

    let (status, _) = json_request(
        app.clone(),
        "GET",
        &format!("{base}?childType=task"),
        None,
        Some(&outsider.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    sqlx::query("UPDATE fvoci.projects SET status = 'archived' WHERE id = $1")
        .bind(Uuid::parse_str(project_id).unwrap())
        .execute(&admin)
        .await
        .unwrap();
    let (status, body) = json_request(
        app.clone(),
        "GET",
        &format!("{base}?childType=task"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "project_archived");

    admin.close().await;
    harness.cleanup().await;
}

// ---------------------------------------------------------------------------
// Workspace task and status lists
// ---------------------------------------------------------------------------

#[tokio::test]
async fn workspace_task_and_status_lists_follow_project_visibility() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, workspace_id, "member", "member").await;
    let guest = add_workspace_user(&admin, workspace_id, "guest", "guest").await;
    let outsider_workspace = insert_workspace(&admin).await;
    let stranger = add_workspace_user(&admin, outsider_workspace, "owner", "stranger").await;

    let open = create_project(app.clone(), &cookie, workspace_id, "OPN", "workspace").await;
    let open_id = open["id"].as_str().unwrap();
    let private = create_project(app.clone(), &cookie, workspace_id, "PRI", "private").await;
    let private_id = private["id"].as_str().unwrap();
    let granted = create_project(app.clone(), &cookie, workspace_id, "GRT", "private").await;
    let granted_id = granted["id"].as_str().unwrap();
    add_project_member(
        app.clone(),
        &cookie,
        workspace_id,
        granted_id,
        guest.user_id,
        "viewer",
    )
    .await;
    let mine = create_task(
        app.clone(),
        &cookie,
        workspace_id,
        open_id,
        json!({"title": "Mine"}),
    )
    .await;
    let (status, _) = json_request(
        app.clone(),
        "PATCH",
        &format!(
            "/api/v1/workspaces/{workspace_id}/tasks/{}",
            mine["id"].as_str().unwrap()
        ),
        Some(json!({"assigneeIds": [member.user_id.to_string()]})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    create_task(
        app.clone(),
        &cookie,
        workspace_id,
        open_id,
        json!({"title": "Open other"}),
    )
    .await;
    create_task(
        app.clone(),
        &cookie,
        workspace_id,
        private_id,
        json!({"title": "Private"}),
    )
    .await;
    create_task(
        app.clone(),
        &cookie,
        workspace_id,
        granted_id,
        json!({"title": "Granted"}),
    )
    .await;

    let tasks_path = format!("/api/v1/workspaces/{workspace_id}/tasks");
    let titles = |body: &Value| -> Vec<String> {
        let mut titles: Vec<String> = body["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["title"].as_str().unwrap().to_string())
            .collect();
        titles.sort();
        titles
    };
    let (status, owner_list) =
        json_request(app.clone(), "GET", &tasks_path, None, Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK, "{owner_list}");
    assert_eq!(
        titles(&owner_list),
        vec!["Granted", "Mine", "Open other", "Private"]
    );
    let (status, member_list) =
        json_request(app.clone(), "GET", &tasks_path, None, Some(&member.cookie)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(titles(&member_list), vec!["Mine", "Open other"]);
    let counted: i64 = member_list["statusCounts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["count"].as_i64().unwrap())
        .sum();
    assert_eq!(counted, 2);
    let (status, guest_list) =
        json_request(app.clone(), "GET", &tasks_path, None, Some(&guest.cookie)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(titles(&guest_list), vec!["Granted"]);
    let (status, _) = json_request(
        app.clone(),
        "GET",
        &tasks_path,
        None,
        Some(&stranger.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let query = encode(r#"{"filters":{"assigneeId":"me"},"sort":[]}"#);
    let (status, assigned) = json_request(
        app.clone(),
        "GET",
        &format!("{tasks_path}?query={query}"),
        None,
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{assigned}");
    assert_eq!(titles(&assigned), vec!["Mine"]);
    assert_eq!(
        assigned["items"][0]["assigneeIds"],
        json!([member.user_id.to_string()])
    );

    // Keyset paging; a cursor is bound to its filters.
    let (status, first) = json_request(
        app.clone(),
        "GET",
        &format!("{tasks_path}?limit=1"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let mut seen = titles(&first);
    let mut cursor = first["nextCursor"].as_str().map(str::to_string);
    while let Some(next) = cursor {
        let (status, page) = json_request(
            app.clone(),
            "GET",
            &format!("{tasks_path}?limit=1&cursor={}", encode(&next)),
            None,
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{page}");
        seen.extend(titles(&page));
        cursor = page["nextCursor"].as_str().map(str::to_string);
    }
    seen.sort();
    assert_eq!(seen, vec!["Granted", "Mine", "Open other", "Private"]);
    let first_cursor = first["nextCursor"].as_str().unwrap();
    let (status, body) = json_request(
        app.clone(),
        "GET",
        &format!(
            "{tasks_path}?limit=1&query={query}&cursor={}",
            encode(first_cursor)
        ),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    for bad in [
        "?limit=0",
        "?limit=101",
        "?archived=true",
        "?from=2026-01-01&to=2026-01-02",
    ] {
        let (status, _) = json_request(
            app.clone(),
            "GET",
            &format!("{tasks_path}{bad}"),
            None,
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }

    let read = api_token(&admin, member.user_id, workspace_id, &["tasks.read"]).await;
    let (status, pat_list) = bearer_request(app.clone(), "GET", &tasks_path, None, &read).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(titles(&pat_list), vec!["Mine", "Open other"]);
    let projects_only = api_token(&admin, member.user_id, workspace_id, &["projects.read"]).await;
    let (status, _) = bearer_request(app.clone(), "GET", &tasks_path, None, &projects_only).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Statuses: six seeded per project, only for viewable projects.
    let statuses_path = format!("/api/v1/workspaces/{workspace_id}/statuses");
    let projects_of = |body: &Value| -> std::collections::BTreeMap<String, usize> {
        let mut map = std::collections::BTreeMap::new();
        for item in body["items"].as_array().unwrap() {
            *map.entry(item["projectId"].as_str().unwrap().to_string())
                .or_insert(0) += 1;
        }
        map
    };
    let (status, owner_statuses) =
        json_request(app.clone(), "GET", &statuses_path, None, Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(projects_of(&owner_statuses).len(), 3);
    let (status, member_statuses) = json_request(
        app.clone(),
        "GET",
        &statuses_path,
        None,
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let member_projects = projects_of(&member_statuses);
    assert_eq!(member_projects.len(), 1);
    assert_eq!(member_projects.get(open_id), Some(&6));
    let first_status = &member_statuses["items"][0];
    for key in [
        "id",
        "workflowId",
        "projectId",
        "name",
        "sortKey",
        "category",
    ] {
        assert!(first_status[key].is_string(), "{key}");
    }
    assert!(first_status["wipLimit"].is_null());
    let (status, guest_statuses) = json_request(
        app.clone(),
        "GET",
        &statuses_path,
        None,
        Some(&guest.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        projects_of(&guest_statuses)
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        vec![granted_id.to_string()]
    );
    let (status, _) = json_request(
        app.clone(),
        "GET",
        &statuses_path,
        None,
        Some(&stranger.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    admin.close().await;
    harness.cleanup().await;
}

// ---------------------------------------------------------------------------
// Workflow statuses
// ---------------------------------------------------------------------------

#[tokio::test]
async fn workflow_status_writes_need_manage_and_keep_order_and_usage_rules() {
    let harness = TestDb::bootstrap().await;
    let (app, _owner_cookie, _owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lead = add_workspace_user(&admin, workspace_id, "member", "lead").await;
    let editor = add_workspace_user(&admin, workspace_id, "member", "editor").await;
    // On a workspace-visible project only workspace admins manage; a private
    // project's lead manages and its members edit.
    let project = create_project(app.clone(), &lead.cookie, workspace_id, "WFL", "private").await;
    let project_id = project["id"].as_str().unwrap();
    add_project_member(
        app.clone(),
        &lead.cookie,
        workspace_id,
        project_id,
        editor.user_id,
        "member",
    )
    .await;
    let wf = workflow(app.clone(), &lead.cookie, workspace_id, project_id).await;
    let workflow_id = wf["id"].as_str().unwrap();
    let statuses = wf["statuses"].as_array().unwrap().clone();
    let base = format!("/api/v1/workspaces/{workspace_id}/workflows/{workflow_id}/statuses");

    // A project member edits tasks but does not manage the workflow.
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &base,
        Some(json!({"name": "QA", "category": "in_progress"})),
        Some(&editor.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, qa) = json_request(
        app.clone(),
        "POST",
        &base,
        Some(json!({"name": "  QA  ", "category": "in_progress", "wipLimit": 3})),
        Some(&lead.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{qa}");
    assert_eq!(qa["name"], "QA");
    assert_eq!(qa["wipLimit"], 3);
    assert_eq!(qa["workflowId"], workflow_id);
    let last_key = statuses.last().unwrap()["sortKey"].as_str().unwrap();
    assert!(qa["sortKey"].as_str().unwrap() > last_key);

    for bad in [
        json!({"name": "", "category": "todo"}),
        json!({"name": "x".repeat(101), "category": "todo"}),
        json!({"name": "X", "category": "doing"}),
        json!({"name": "X", "category": "todo", "wipLimit": 0}),
        json!({"name": "X", "category": "todo", "wipLimit": 1.5}),
        json!({"name": "X", "category": "todo", "extra": true}),
    ] {
        let (status, _) = json_request(
            app.clone(),
            "POST",
            &base,
            Some(bad.clone()),
            Some(&lead.cookie),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }

    let qa_id = qa["id"].as_str().unwrap();
    let first_id = statuses[0]["id"].as_str().unwrap();
    let second_id = statuses[1]["id"].as_str().unwrap();
    let (status, moved) = json_request(
        app.clone(),
        "PATCH",
        &format!("{base}/{qa_id}"),
        Some(json!({"name": "Review 2", "wipLimit": null, "beforeId": second_id})),
        Some(&lead.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{moved}");
    assert_eq!(moved["name"], "Review 2");
    assert!(moved["wipLimit"].is_null());
    let order: Vec<String> = workflow(app.clone(), &lead.cookie, workspace_id, project_id).await
        ["statuses"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(&order[..3], &[first_id, qa_id, second_id]);

    for bad in [
        json!({}),
        json!({"beforeId": first_id, "afterId": second_id}),
        json!({"category": "doing"}),
        json!({"name": "  "}),
    ] {
        let (status, _) = json_request(
            app.clone(),
            "PATCH",
            &format!("{base}/{qa_id}"),
            Some(bad.clone()),
            Some(&lead.cookie),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }
    let (status, body) = json_request(
        app.clone(),
        "PATCH",
        &format!("{base}/{qa_id}"),
        Some(json!({"afterId": Uuid::now_v7().to_string()})),
        Some(&lead.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "anchor_not_in_target_list");

    // A status of another project's workflow is not in this workflow.
    let other = create_project(app.clone(), &lead.cookie, workspace_id, "OTH", "workspace").await;
    let other_wf = workflow(
        app.clone(),
        &lead.cookie,
        workspace_id,
        other["id"].as_str().unwrap(),
    )
    .await;
    let foreign = other_wf["statuses"][0]["id"].as_str().unwrap();
    let (status, _) = json_request(
        app.clone(),
        "PATCH",
        &format!("{base}/{foreign}"),
        Some(json!({"name": "Hijack"})),
        Some(&lead.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // In-use statuses stay; a trashed task still counts.
    let task = create_task(
        app.clone(),
        &lead.cookie,
        workspace_id,
        project_id,
        json!({"title": "Uses QA", "statusId": qa_id}),
    )
    .await;
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &format!(
            "/api/v1/workspaces/{workspace_id}/tasks/{}/trash",
            task["id"].as_str().unwrap()
        ),
        None,
        Some(&lead.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = json_request(
        app.clone(),
        "DELETE",
        &format!("{base}/{qa_id}"),
        None,
        Some(&lead.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "status_has_tasks");
    let (status, _) = json_request(
        app.clone(),
        "DELETE",
        &format!(
            "/api/v1/workspaces/{workspace_id}/tasks/{}",
            task["id"].as_str().unwrap()
        ),
        None,
        Some(&lead.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = json_request(
        app.clone(),
        "DELETE",
        &format!("{base}/{qa_id}"),
        None,
        Some(&editor.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) = json_request(
        app.clone(),
        "DELETE",
        &format!("{base}/{qa_id}"),
        None,
        Some(&lead.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Token scopes: tasks.write manages, tasks.read does not.
    let read = api_token(&admin, lead.user_id, workspace_id, &["tasks.read"]).await;
    let write = api_token(&admin, lead.user_id, workspace_id, &["tasks.write"]).await;
    let (status, _) = bearer_request(
        app.clone(),
        "POST",
        &base,
        Some(json!({"name": "Via read", "category": "todo"})),
        &read,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = bearer_request(
        app.clone(),
        "POST",
        &base,
        Some(json!({"name": "Via write", "category": "todo"})),
        &write,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // Archived project: read-only.
    sqlx::query("UPDATE fvoci.projects SET status = 'archived' WHERE id = $1")
        .bind(Uuid::parse_str(project_id).unwrap())
        .execute(&admin)
        .await
        .unwrap();
    let (status, body) = json_request(
        app.clone(),
        "POST",
        &base,
        Some(json!({"name": "Late", "category": "todo"})),
        Some(&lead.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "project_archived");

    admin.close().await;
    harness.cleanup().await;
}

/// Two creations race for the last of `MAX_WORKFLOW_STATUSES` slots: the
/// workflow row lock lets exactly one through.
#[tokio::test]
async fn workflow_status_limit_holds_under_concurrent_creation() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let co_admin = add_workspace_user(&admin, workspace_id, "admin", "coadmin").await;
    let project = create_project(app.clone(), &cookie, workspace_id, "LIM", "workspace").await;
    let project_id = Uuid::parse_str(project["id"].as_str().unwrap()).unwrap();
    let wf = workflow(app.clone(), &cookie, workspace_id, &project_id.to_string()).await;
    let workflow_id = Uuid::parse_str(wf["id"].as_str().unwrap()).unwrap();
    let seeded = wf["statuses"].as_array().unwrap().len() as i64;
    for i in 0..(199 - seeded) {
        sqlx::query(
            r#"
            INSERT INTO fvoci.statuses (id, workspace_id, project_id, workflow_id, name, category, sort_key)
            VALUES ($1, $2, $3, $4, $5, 'todo', $6)
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(workspace_id)
        .bind(project_id)
        .bind(workflow_id)
        .bind(format!("S{i}"))
        .bind(format!("b{i:04}"))
        .execute(&admin)
        .await
        .unwrap();
    }
    let base = format!("/api/v1/workspaces/{workspace_id}/workflows/{workflow_id}/statuses");

    // Hold the project row so both requests queue behind it, then release.
    let mut barrier = admin.begin().await.unwrap();
    sqlx::query("SELECT id FROM fvoci.projects WHERE id = $1 FOR UPDATE")
        .bind(project_id)
        .fetch_one(&mut *barrier)
        .await
        .unwrap();
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *barrier)
        .await
        .unwrap();
    // Different actors, so neither waits on the other's membership lock.
    let spawn = |name: &'static str, cookie: String| {
        let app = app.clone();
        let base = base.clone();
        tokio::spawn(async move {
            json_request(
                app,
                "POST",
                &base,
                Some(json!({"name": name, "category": "todo"})),
                Some(&cookie),
            )
            .await
        })
    };
    let a = spawn("Last A", cookie.clone());
    let b = spawn("Last B", co_admin.cookie.clone());
    // The second waiter queues behind the first one's tuple lock, so count
    // lock waiters on the project row rather than waiters of the barrier.
    wait_for_project_row_waiters(&admin, 2).await;
    let _ = blocker_pid;
    barrier.commit().await.unwrap();
    let (status_a, body_a) = tokio::time::timeout(Duration::from_secs(10), a)
        .await
        .expect("a finished")
        .expect("join");
    let (status_b, body_b) = tokio::time::timeout(Duration::from_secs(10), b)
        .await
        .expect("b finished")
        .expect("join");
    let mut statuses = vec![status_a, status_b];
    statuses.sort();
    assert_eq!(
        statuses,
        vec![StatusCode::CREATED, StatusCode::CONFLICT],
        "{body_a} {body_b}"
    );
    let limited = if status_a == StatusCode::CONFLICT {
        body_a
    } else {
        body_b
    };
    assert_eq!(limited["code"], "workflow_status_limit");
    let total: i64 =
        sqlx::query_scalar("SELECT count(*) FROM fvoci.statuses WHERE workflow_id = $1")
            .bind(workflow_id)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert_eq!(total, 200);

    admin.close().await;
    harness.cleanup().await;
}

/// The lead is demoted while the status write waits on its membership lock;
/// the transaction's manage recheck refuses it.
#[tokio::test]
async fn workflow_status_write_rechecks_a_demoted_lead() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let lead = add_workspace_user(&admin, workspace_id, "member", "lead").await;
    let project = create_project(app.clone(), &cookie, workspace_id, "DEM", "private").await;
    let project_id = project["id"].as_str().unwrap();
    add_project_member(
        app.clone(),
        &cookie,
        workspace_id,
        project_id,
        lead.user_id,
        "lead",
    )
    .await;
    let wf = workflow(app.clone(), &lead.cookie, workspace_id, project_id).await;
    let workflow_id = wf["id"].as_str().unwrap().to_string();
    let statuses_before = count_rows(&admin, "statuses").await;

    let mut barrier = admin.begin().await.unwrap();
    hold_membership_user_lock(&mut barrier, lead.user_id).await;
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *barrier)
        .await
        .unwrap();
    let request = tokio::spawn({
        let app = app.clone();
        let cookie = lead.cookie.clone();
        async move {
            json_request(
                app,
                "POST",
                &format!("/api/v1/workspaces/{workspace_id}/workflows/{workflow_id}/statuses"),
                Some(json!({"name": "Race", "category": "todo"})),
                Some(&cookie),
            )
            .await
        }
    });
    wait_for_advisory_blocked_by(&admin, blocker_pid).await;
    sqlx::query("UPDATE fvoci.project_members SET role = 'member' WHERE user_id = $1")
        .bind(lead.user_id)
        .execute(&mut *barrier)
        .await
        .unwrap();
    barrier.commit().await.unwrap();
    let (status, _) = tokio::time::timeout(Duration::from_secs(10), request)
        .await
        .expect("request finished")
        .expect("join");
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(count_rows(&admin, "statuses").await, statuses_before);

    admin.close().await;
    harness.cleanup().await;
}
