#![cfg(feature = "db-tests")]
#![allow(dead_code)]

#[path = "support/project_harness.rs"]
mod project_harness;

use axum::http::StatusCode;
use fvoci_server::db::outbox::{ensure_consumer, lease_consumer, read_events, release_consumer};
use fvoci_server::notifications::{process_notification_event, NOTIFICATIONS_CONSUMER};
use project_harness::{
    add_workspace_user, admin_pool, app_pool, create_project, http_request, json_request,
    setup_session, TestDb,
};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

async fn drain_notifications(pool: &PgPool) {
    ensure_consumer(pool, NOTIFICATIONS_CONSUMER)
        .await
        .expect("ensure notifications consumer");
    let owner = Uuid::now_v7();
    let leased = lease_consumer(pool, NOTIFICATIONS_CONSUMER, owner, 30)
        .await
        .expect("lease");
    assert!(leased, "notifications consumer lease");
    loop {
        let events = read_events(pool, NOTIFICATIONS_CONSUMER, 100)
            .await
            .expect("read events");
        if events.is_empty() {
            break;
        }
        for event in events {
            process_notification_event(pool, owner, &event)
                .await
                .expect("deliver notification");
        }
    }
    let _ = release_consumer(pool, NOTIFICATIONS_CONSUMER, owner).await;
}

async fn bearer_get(app: axum::Router, path: &str, token: &str) -> (StatusCode, Value) {
    let auth = format!("Bearer {token}");
    let (status, body, _) = http_request(
        app,
        "GET",
        path,
        None,
        None,
        None,
        &[("authorization", auth.as_str())],
    )
    .await;
    (status, body)
}

#[tokio::test]
async fn assignment_fans_out_inbox_prefs_rls_and_pat_kinds() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, owner_id, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let app_db = app_pool(&harness).await;
    let member = add_workspace_user(&admin, workspace_id, "member", "assignee").await;
    let outsider = add_workspace_user(&admin, workspace_id, "member", "outsider").await;

    let (status, prefs) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/notification-prefs"),
        None,
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{prefs:?}");
    assert_eq!(prefs["inApp"], true);
    assert_eq!(prefs["mailImmediate"], true);
    assert_eq!(prefs["mailDigest"], false);

    let project =
        create_project(app.clone(), &owner_cookie, workspace_id, "NTF", "workspace").await;
    let project_id = project["id"].as_str().unwrap();
    let (status, created) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks"),
        Some(json!({"title": "알림 수신 확인 태스크"})),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created:?}");
    let task_id = created["id"].as_str().unwrap();
    drain_notifications(&app_db).await;

    let (status, patched) = json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{workspace_id}/tasks/{task_id}"),
        Some(json!({"assigneeIds": [member.user_id, owner_id]})),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{patched:?}");

    drain_notifications(&app_db).await;
    drain_notifications(&app_db).await;

    let (status, listed) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/notifications"),
        None,
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{listed:?}");
    let items = listed["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{listed:?}");
    assert_eq!(items[0]["verb"], "task.updated");
    assert_eq!(items[0]["targetId"], task_id);
    assert!(items[0]["readAt"].is_null());

    let (status, owner_listed) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/notifications"),
        None,
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{owner_listed:?}");
    let owner_items = owner_listed["items"].as_array().unwrap();
    assert!(
        owner_items
            .iter()
            .all(|item| item["targetId"] != task_id || item["verb"] != "task.updated"),
        "{owner_listed:?}"
    );

    let (status, outsider_listed) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/notifications"),
        None,
        Some(&outsider.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(outsider_listed["items"].as_array().unwrap().is_empty());

    let (status, unread) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/notifications/unread-count"),
        None,
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(unread["count"], 1);

    let notification_id = items[0]["id"].as_str().unwrap();
    let (status, patched_flags) = json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{workspace_id}/notifications/{notification_id}"),
        Some(json!({"read": true})),
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{patched_flags:?}");
    let (status, unread_after) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/notifications/unread-count"),
        None,
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(unread_after["count"], 0);

    let (status, read_all) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/notifications/read-all"),
        None,
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{read_all:?}");

    let (status, archived) = json_request(
        app.clone(),
        "PATCH",
        &format!("/api/v1/workspaces/{workspace_id}/notifications/{notification_id}"),
        Some(json!({"archived": true})),
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{archived:?}");
    let (status, archived_list) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/notifications?filter=archived"),
        None,
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(archived_list["items"].as_array().unwrap().len(), 1);

    let (status, bad_cursor) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/notifications?cursor=not-a-cursor"),
        None,
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{bad_cursor:?}");
    assert_eq!(bad_cursor["params"]["code"], "invalid_cursor");

    let (status, me) = json_request(
        app.clone(),
        "GET",
        "/api/v1/me/notifications?filter=archived",
        None,
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{me:?}");
    assert!(!me["items"].as_array().unwrap().is_empty());

    sqlx::query(
        r#"
        INSERT INTO fvoci.notifications (
            id, workspace_id, user_id, event_id, verb, target_type, target_id, payload
        ) VALUES ($1, $2, $3, $4, 'task.updated', 'task', $5, '{}'::jsonb)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(workspace_id)
    .bind(owner_id)
    .bind(Uuid::now_v7())
    .bind(Uuid::parse_str(task_id).unwrap())
    .execute(&admin)
    .await
    .expect("seed owner notification");

    let (status, token) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/api-tokens"),
        Some(json!({"name": "tasks", "scopes": ["tasks.read"]})),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{token:?}");
    let secret = token["token"].as_str().unwrap();
    let (status, pat_list) = bearer_get(
        app.clone(),
        &format!("/api/v1/workspaces/{workspace_id}/notifications"),
        secret,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{pat_list:?}");
    assert_eq!(pat_list["items"].as_array().unwrap().len(), 1);
    assert_eq!(pat_list["items"][0]["verb"], "task.updated");

    let (status, docs_token) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/api-tokens"),
        Some(json!({"name": "docs", "scopes": ["documents.read"]})),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{docs_token:?}");
    let (status, me_pat) = bearer_get(
        app.clone(),
        "/api/v1/me/notifications",
        docs_token["token"].as_str().unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{me_pat:?}");

    let (status, saved_prefs) = json_request(
        app.clone(),
        "PUT",
        &format!("/api/v1/workspaces/{workspace_id}/notification-prefs"),
        Some(json!({"inApp": false, "mailImmediate": true, "mailDigest": false})),
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved_prefs:?}");
    let (status, hidden) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/notifications"),
        None,
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(hidden["items"].as_array().unwrap().is_empty());

    let rls: Vec<(bool, bool)> = sqlx::query_as(
        r#"
        SELECT c.relrowsecurity, c.relforcerowsecurity
        FROM pg_class c
        INNER JOIN pg_namespace n ON n.oid = c.relnamespace
        WHERE n.nspname = 'fvoci'
          AND c.relname IN ('notifications', 'notification_prefs')
        ORDER BY c.relname
        "#,
    )
    .fetch_all(&admin)
    .await
    .expect("rls flags");
    assert_eq!(rls.len(), 2);
    assert!(rls.iter().all(|(enabled, forced)| *enabled && *forced));

    let mut tx = app_db.begin().await.expect("rls tx");
    sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
        .bind(workspace_id.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('app.self_user_id', $1, true)")
        .bind(outsider.user_id.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let seen: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.notifications")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(seen, 0, "RLS must hide another user's inbox");

    admin.close().await;
    app_db.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn comment_mention_notifies_member_not_actor() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let app_db = app_pool(&harness).await;
    let member = add_workspace_user(&admin, workspace_id, "member", "mentioned").await;

    let (status, doc) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents"),
        Some(json!({"parentId": null, "title": "멘션 문서"})),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{doc:?}");
    let document_id = doc["id"].as_str().unwrap();
    let (status, comment) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/comments"),
        Some(json!({
            "body": "멘션",
            "mentionedUserIds": [member.user_id]
        })),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{comment:?}");
    drain_notifications(&app_db).await;

    let (status, listed) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/notifications"),
        None,
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{listed:?}");
    let items = listed["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{listed:?}");
    assert_eq!(items[0]["verb"], "comment.created");

    let (status, owner_listed) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/notifications"),
        None,
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(owner_listed["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|item| item["verb"] != "comment.created"));

    admin.close().await;
    app_db.close().await;
    harness.cleanup().await;
}

/// Upgrading an existing install must not notify users about past events: the
/// notifications consumer starts after the events recorded before migration 018.
#[tokio::test]
async fn upgrade_starts_notifications_after_existing_events() {
    let harness = TestDb::bootstrap_through(17).await;
    let admin = admin_pool(&harness).await;
    let mut tx = admin.begin().await.expect("tx");
    sqlx::query("SELECT set_config('app.system_ctx', 'on', true)")
        .execute(&mut *tx)
        .await
        .expect("system ctx");
    let workspace_id: Uuid = sqlx::query_scalar(
        "INSERT INTO fvoci.workspaces (id, slug, name) VALUES (gen_random_uuid(), 'upg', 'Upgrade') RETURNING id",
    )
    .fetch_one(&mut *tx)
    .await
    .expect("workspace");
    let last: (String, i64) = sqlx::query_as(
        "INSERT INTO fvoci.events (id, workspace_id, verb, target_type, target_id, payload) \
         VALUES (gen_random_uuid(), $1, 'task.created', 'task', gen_random_uuid(), '{}'::jsonb) \
         RETURNING xact::text, seq",
    )
    .bind(workspace_id)
    .fetch_one(&mut *tx)
    .await
    .expect("historical event");
    tx.commit().await.expect("commit");
    admin.close().await;

    fvoci_server::db::migrate::run_migrations(&harness.admin_url)
        .await
        .expect("migrate to latest");
    let admin = admin_pool(&harness).await;
    let cursor: (String, i64) = sqlx::query_as(
        "SELECT last_xact::text, last_seq FROM fvoci.outbox_consumers WHERE consumer = 'notifications'",
    )
    .fetch_one(&admin)
    .await
    .expect("notifications cursor");
    assert_eq!(cursor, last, "cursor starts after the pre-upgrade events");
    admin.close().await;
    harness.cleanup().await;
}
