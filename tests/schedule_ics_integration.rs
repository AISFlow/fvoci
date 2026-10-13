#![cfg(feature = "db-tests")]
#![allow(dead_code)]

#[path = "support/integrations_family.rs"]
mod integrations_family;

#[tokio::test]
async fn sqlite_holidays_and_ics_http_rotation_expiry_and_tenant_refusal() {
    let f = integrations_family::Fixture::new().await;
    let app = f.app(std::sync::Arc::new(
        fvoci_server::integrations::Integrations::disabled(),
    ));
    let base = format!("/api/v1/workspaces/{}", f.workspace);
    let (status, list) = json_request(
        app.clone(),
        "GET",
        &format!("{base}/holidays"),
        None,
        Some(&f.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["canEdit"], true);
    for _ in 0..2 {
        let (status, _) = json_request(
            app.clone(),
            "POST",
            &format!("{base}/holidays"),
            Some(json!({"date":"2026-12-25"})),
            Some(&f.token),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
    }
    let fresh = f.fresh().await;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM workspace_holidays")
        .fetch_one(&fresh)
        .await
        .unwrap();
    assert_eq!(count, 1);
    let (status, _) = json_request(
        app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{}/holidays/2026-12-25", Uuid::now_v7()),
        None,
        Some(&f.token),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = json_request(
        app.clone(),
        "DELETE",
        &format!("{base}/holidays/2026-12-25"),
        None,
        Some(&f.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, first) = json_request(
        app.clone(),
        "POST",
        &format!("{base}/ics-token"),
        None,
        Some(&f.token),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let first_path = url::Url::parse(first["url"].as_str().unwrap())
        .unwrap()
        .path()
        .to_string();
    let (status, second) = json_request(
        app.clone(),
        "POST",
        &format!("{base}/ics-token"),
        None,
        Some(&f.token),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let path = url::Url::parse(second["url"].as_str().unwrap())
        .unwrap()
        .path()
        .to_string();
    let (project, backlog, _) = f.seed_project().await;
    let task = Uuid::now_v7();
    sqlx::query("INSERT INTO tasks(id,workspace_id,project_id,number,title,status_id,sort_key,schema_version,content_json,created_by,due_date) VALUES(?1,?2,?3,1,'Assigned calendar task',?4,'a0',1,'{\"type\":\"doc\",\"content\":[]}',?5,'2026-12-25')").bind(task.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(project.as_bytes().as_slice()).bind(backlog.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).execute(&fresh).await.unwrap();
    sqlx::query("INSERT INTO task_assignees(workspace_id,task_id,user_id) VALUES(?1,?2,?3)")
        .bind(f.workspace.as_bytes().as_slice())
        .bind(task.as_bytes().as_slice())
        .bind(f.user.as_bytes().as_slice())
        .execute(&fresh)
        .await
        .unwrap();
    let (status, _, calendar) = raw_request(app.clone(), "GET", &path, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(String::from_utf8(calendar)
        .unwrap()
        .contains("Assigned calendar task"));
    assert_eq!(
        raw_request(app.clone(), "GET", &first_path, None).await.0,
        StatusCode::NOT_FOUND
    );
    for (method, expected) in [
        ("GET", StatusCode::OK),
        ("HEAD", StatusCode::OK),
        ("OPTIONS", StatusCode::NO_CONTENT),
        ("PROPFIND", StatusCode::MULTI_STATUS),
        ("REPORT", StatusCode::MULTI_STATUS),
    ] {
        assert_eq!(
            raw_request(app.clone(), method, &path, None).await.0,
            expected
        );
    }
    sqlx::query("UPDATE ics_tokens SET expires_at=unixepoch()*1000000-1")
        .execute(&fresh)
        .await
        .unwrap();
    assert_eq!(
        raw_request(app.clone(), "GET", &path, None).await.0,
        StatusCode::NOT_FOUND
    );
    fresh.close().await;
    f.cleanup().await;
}

/// Same-workspace refusal: a member reads the holidays but only an admin
/// writes them (PG `mutate_holiday`).
#[tokio::test]
async fn sqlite_holiday_writes_refuse_same_workspace_member() {
    let f = integrations_family::Fixture::new().await;
    let app = f.app(std::sync::Arc::new(
        fvoci_server::integrations::Integrations::disabled(),
    ));
    let path = format!("/api/v1/workspaces/{}/holidays", f.workspace);
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &path,
        Some(json!({"date":"2026-12-25"})),
        Some(&f.token),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (_, member) = f.member("member").await;
    let (status, list) = json_request(app.clone(), "GET", &path, None, Some(&member)).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["canEdit"], false);
    let fresh = f.fresh().await;
    let counts = "SELECT (SELECT group_concat(date) FROM workspace_holidays),(SELECT count(*) FROM events),(SELECT count(*) FROM audit_log)";
    let before: (String, i64, i64) = sqlx::query_as(counts).fetch_one(&fresh).await.unwrap();
    assert_eq!(before.0, "2026-12-25");
    let (status, body) = json_request(
        app.clone(),
        "POST",
        &path,
        Some(json!({"date":"2026-12-31"})),
        Some(&member),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "not_found");
    let (status, body) = json_request(
        app.clone(),
        "DELETE",
        &format!("{path}/2026-12-25"),
        None,
        Some(&member),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "not_found");
    let after: (String, i64, i64) = sqlx::query_as(counts).fetch_one(&fresh).await.unwrap();
    assert_eq!(before, after);
    fresh.close().await;
    f.cleanup().await;
}

#[path = "support/project_harness.rs"]
mod project_harness;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use project_harness::{
    add_workspace_user, admin_pool, create_project, http_request, json_request, setup_session,
    TestDb,
};
use serde_json::{json, Value};
use tower::ServiceExt;
use uuid::Uuid;

async fn create_task(
    app: axum::Router,
    cookie: &str,
    workspace_id: Uuid,
    project_id: &str,
    title: &str,
    dates: Value,
) -> Value {
    let mut body = dates;
    body["title"] = json!(title);
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

async fn raw_request(
    app: axum::Router,
    method: &str,
    path: &str,
    cookie: Option<&str>,
) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("origin", "http://localhost");
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", format!("fvoci_session={cookie}"));
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
async fn holidays_crud_permissions_and_dependency_lag() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, workspace_id, "member", "hol-member").await;
    let guest = add_workspace_user(&admin, workspace_id, "guest", "hol-guest").await;

    let (status, listed) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/holidays"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(listed["canEdit"], true);
    assert_eq!(listed["items"], json!([]));

    let (status, created) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/holidays"),
        Some(json!({"date": "2031-04-14"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["ok"], true);

    let (status, again) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/holidays"),
        Some(json!({"date": "2031-04-14"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{again}");

    let (status, bad) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/holidays"),
        Some(json!({"date": "2031-4-14"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");

    let (status, member_list) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/holidays"),
        None,
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{member_list}");
    assert_eq!(member_list["canEdit"], false);
    assert_eq!(member_list["items"], json!(["2031-04-14"]));

    let (status, guest_list) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/holidays"),
        None,
        Some(&guest.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{guest_list}");
    assert_eq!(guest_list["canEdit"], false);

    let (status, member_write) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/holidays"),
        Some(json!({"date": "2031-05-01"})),
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{member_write}");

    let lab = create_project(app.clone(), &cookie, workspace_id, "HOL", "workspace").await;
    let project_id = lab["id"].as_str().unwrap();
    let blocker = create_task(
        app.clone(),
        &cookie,
        workspace_id,
        project_id,
        "선행",
        json!({"dueDate": "2031-04-11"}),
    )
    .await;
    let blocked = create_task(
        app.clone(),
        &cookie,
        workspace_id,
        project_id,
        "후행",
        json!({"startDate": "2031-04-14"}),
    )
    .await;
    let (status, dep) = json_request(
        app.clone(),
        "POST",
        &format!(
            "/api/v1/workspaces/{workspace_id}/tasks/{}/dependencies",
            blocker["id"].as_str().unwrap()
        ),
        Some(json!({"blockedId": blocked["id"], "type": "FS", "lagDays": 1})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{dep}");
    assert_eq!(dep["code"], "dependency_contradiction");

    let (status, removed) = json_request(
        app.clone(),
        "DELETE",
        &format!("/api/v1/workspaces/{workspace_id}/holidays/2031-04-14"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{removed}");

    let (status, dep_ok) = json_request(
        app.clone(),
        "POST",
        &format!(
            "/api/v1/workspaces/{workspace_id}/tasks/{}/dependencies",
            blocker["id"].as_str().unwrap()
        ),
        Some(json!({"blockedId": blocked["id"], "type": "FS", "lagDays": 1})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{dep_ok}");

    admin.close().await;
    harness.cleanup().await;
}

/// PG twin of `sqlite_holiday_writes_refuse_same_workspace_member`.
#[tokio::test]
async fn holiday_writes_refuse_same_workspace_member() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let path = format!("/api/v1/workspaces/{workspace_id}/holidays");
    let (status, _) = json_request(
        app.clone(),
        "POST",
        &path,
        Some(json!({"date": "2026-12-25"})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let member = add_workspace_user(&admin, workspace_id, "member", "hol-writer").await;
    let (status, list) = json_request(app.clone(), "GET", &path, None, Some(&member.cookie)).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["canEdit"], false);
    let counts = "SELECT (SELECT string_agg(date::text, ',' ORDER BY date) FROM fvoci.workspace_holidays), (SELECT count(*) FROM fvoci.events), (SELECT count(*) FROM fvoci.audit_log)";
    let before: (String, i64, i64) = sqlx::query_as(counts).fetch_one(&admin).await.unwrap();
    assert_eq!(before.0, "2026-12-25");
    let (status, body) = json_request(
        app.clone(),
        "POST",
        &path,
        Some(json!({"date": "2026-12-31"})),
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "not_found");
    let (status, body) = json_request(
        app.clone(),
        "DELETE",
        &format!("{path}/2026-12-25"),
        None,
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "not_found");
    let after: (String, i64, i64) = sqlx::query_as(counts).fetch_one(&admin).await.unwrap();
    assert_eq!(before, after);
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn ics_feed_token_visibility_caldav_and_pat_scopes() {
    let harness = TestDb::bootstrap().await;
    let (app, cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let member = add_workspace_user(&admin, workspace_id, "member", "ics-member").await;

    let flags: Vec<(String, bool, bool)> = sqlx::query_as(
        r#"
        SELECT c.relname, c.relrowsecurity, c.relforcerowsecurity
        FROM pg_class c
        INNER JOIN pg_namespace n ON n.oid = c.relnamespace
        WHERE n.nspname = 'fvoci'
          AND c.relname IN ('workspace_holidays', 'ics_tokens')
        ORDER BY c.relname
        "#,
    )
    .fetch_all(&admin)
    .await
    .unwrap();
    assert_eq!(flags.len(), 2);
    for (name, rls, force) in &flags {
        assert!(rls, "{name} missing RLS");
        assert!(force, "{name} missing FORCE RLS");
    }

    let public_proj = create_project(app.clone(), &cookie, workspace_id, "PUB", "workspace").await;
    let private_proj = create_project(app.clone(), &cookie, workspace_id, "SEC", "private").await;
    let visible = create_task(
        app.clone(),
        &cookie,
        workspace_id,
        public_proj["id"].as_str().unwrap(),
        "실험, 공개",
        json!({"dueDate": "2026-08-26"}),
    )
    .await;
    let secret = create_task(
        app.clone(),
        &cookie,
        workspace_id,
        private_proj["id"].as_str().unwrap(),
        "비밀",
        json!({"dueDate": "2026-08-27"}),
    )
    .await;
    let _undated = create_task(
        app.clone(),
        &cookie,
        workspace_id,
        public_proj["id"].as_str().unwrap(),
        "날짜없음",
        json!({}),
    )
    .await;
    let unassigned = create_task(
        app.clone(),
        &cookie,
        workspace_id,
        public_proj["id"].as_str().unwrap(),
        "담당없음",
        json!({"dueDate": "2026-08-28"}),
    )
    .await;
    // The feed holds the owner's assigned tasks (source listIcsFeed). The member
    // is also assigned to the private task, which they still cannot see.
    for (task, assignees) in [
        (&visible, json!([owner_id, member.user_id])),
        (&secret, json!([owner_id, member.user_id])),
    ] {
        let (status, body) = json_request(
            app.clone(),
            "PATCH",
            &format!(
                "/api/v1/workspaces/{workspace_id}/tasks/{}",
                task["id"].as_str().unwrap()
            ),
            Some(json!({"assigneeIds": assignees})),
            Some(&cookie),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }

    let (status, token_body) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/ics-token"),
        None,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{token_body}");
    let owner_url = token_body["url"].as_str().unwrap().to_string();
    assert!(owner_url.starts_with("http://localhost/api/v1/ics/"));
    let owner_path = owner_url.strip_prefix("http://localhost").unwrap();

    let stored: (String,) =
        sqlx::query_as("SELECT token_hash FROM fvoci.ics_tokens WHERE user_id = $1")
            .bind(owner_id)
            .fetch_one(&admin)
            .await
            .unwrap();
    let raw = owner_path.rsplit('/').next().unwrap();
    assert_ne!(stored.0, raw);
    assert_eq!(stored.0.len(), 64);
    assert!(!stored.0.contains(raw));

    let (status, headers, body) = raw_request(app.clone(), "GET", owner_path, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or(""),
        "text/calendar; charset=utf-8"
    );
    assert!(headers.get("etag").is_some());
    let ics = String::from_utf8(body).unwrap();
    assert!(ics.contains("BEGIN:VCALENDAR"));
    assert!(ics.contains(&format!("UID:{}@fvoci", visible["id"].as_str().unwrap())));
    assert!(ics.contains("SUMMARY:실험\\, 공개"));
    assert!(ics.contains("DTSTART;VALUE=DATE:20260826"));
    assert!(ics.contains(&format!("UID:{}@fvoci", secret["id"].as_str().unwrap())));
    assert!(!ics.contains("날짜없음"));
    assert!(
        !ics.contains(&format!("UID:{}@fvoci", unassigned["id"].as_str().unwrap())),
        "a visible task not assigned to the feed owner is not exported"
    );

    let (status, _, empty) = raw_request(app.clone(), "HEAD", owner_path, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(empty.is_empty());

    let (status, headers, _) = raw_request(app.clone(), "OPTIONS", owner_path, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        headers
            .get("dav")
            .and_then(|v| v.to_str().ok())
            .unwrap_or(""),
        "1, calendar-access"
    );

    let (status, headers, caldav) = raw_request(app.clone(), "PROPFIND", owner_path, None).await;
    assert_eq!(status, StatusCode::MULTI_STATUS);
    assert!(headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .starts_with("application/xml"));
    let xml = String::from_utf8(caldav).unwrap();
    assert!(xml.contains("calendar-data"));
    assert!(xml.contains("실험\\, 공개") || xml.contains("실험, 공개"));

    let (status, member_token) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/ics-token"),
        None,
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{member_token}");
    let member_path = member_token["url"]
        .as_str()
        .unwrap()
        .strip_prefix("http://localhost")
        .unwrap()
        .to_string();
    let (status, _, member_ics) = raw_request(app.clone(), "GET", &member_path, None).await;
    assert_eq!(status, StatusCode::OK);
    let member_ics = String::from_utf8(member_ics).unwrap();
    assert!(member_ics.contains(&format!("UID:{}@fvoci", visible["id"].as_str().unwrap())));
    assert!(!member_ics.contains(&format!("UID:{}@fvoci", secret["id"].as_str().unwrap())));

    let (status, _, _) =
        raw_request(app.clone(), "GET", "/api/v1/ics/not-a-real-token", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, write_pat) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/api-tokens"),
        Some(json!({"name": "ics-write", "scopes": ["tasks.write"]})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{write_pat}");
    let write_token = write_pat["token"].as_str().unwrap();
    let (status, rotate) = bearer(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/ics-token"),
        None,
        write_token,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{rotate}");
    let rotated_path = rotate["url"]
        .as_str()
        .unwrap()
        .strip_prefix("http://localhost")
        .unwrap()
        .to_string();
    let (status, _, _) = raw_request(app.clone(), "GET", owner_path, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, manage_pat) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/api-tokens"),
        Some(json!({"name": "hol-manage", "scopes": ["workspace.manage"]})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{manage_pat}");
    let manage_token = manage_pat["token"].as_str().unwrap();
    let (status, holidays) = bearer(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/holidays"),
        None,
        manage_token,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{holidays}");

    let (status, read_pat) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/api-tokens"),
        Some(json!({"name": "read-only", "scopes": ["tasks.read"]})),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{read_pat}");
    let read_token = read_pat["token"].as_str().unwrap();
    let (status, denied) = bearer(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/ics-token"),
        None,
        read_token,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{denied}");
    let (status, denied_holidays) = bearer(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/holidays"),
        None,
        read_token,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{denied_holidays}");

    sqlx::query("UPDATE fvoci.ics_tokens SET expires_at = now() - interval '1 second'")
        .execute(&admin)
        .await
        .unwrap();
    let (status, _, _) = raw_request(app.clone(), "GET", &rotated_path, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    admin.close().await;
    harness.cleanup().await;
}
