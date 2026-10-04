#![cfg(feature = "db-tests")]
#![allow(dead_code)]

#[path = "support/project_harness.rs"]
mod project_harness;

use axum::http::StatusCode;
use fvoci_server::db::outbox::{ensure_consumer, lease_consumer, read_events, release_consumer};
use fvoci_server::notifications::{process_notification_event, NOTIFICATIONS_CONSUMER};
use project_harness::{
    add_workspace_user, admin_pool, app_pool, close_pool, create_project, http_request,
    json_request, setup_session, TestDb,
};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

/// `pg_stat_activity` row reported when the snapshot xmin fails to settle:
/// (datname, pid, backend_xid, backend_xmin).
type XidHolder = (Option<String>, i32, Option<String>, Option<String>);

/// Wait (bounded, read-only) until the cluster-wide snapshot xmin passes
/// `horizon`, i.e. every transaction up to it has ended. A transaction in
/// another test's database can hold xmin back; on timeout, report the holders.
async fn wait_xmin_past(pool: &PgPool, horizon: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        let settled: bool =
            sqlx::query_scalar("SELECT pg_snapshot_xmin(pg_current_snapshot()) > $1::xid8")
                .bind(horizon)
                .fetch_one(pool)
                .await
                .expect("snapshot xmin");
        if settled {
            return;
        }
        if std::time::Instant::now() >= deadline {
            let xmin: String =
                sqlx::query_scalar("SELECT pg_snapshot_xmin(pg_current_snapshot())::text")
                    .fetch_one(pool)
                    .await
                    .expect("snapshot xmin");
            let holders: Vec<XidHolder> = sqlx::query_as(
                "SELECT datname::text, pid, backend_xid::text, backend_xmin::text \
                     FROM pg_stat_activity \
                     WHERE backend_xid IS NOT NULL OR backend_xmin IS NOT NULL \
                     ORDER BY age(COALESCE(backend_xid, backend_xmin)) DESC LIMIT 5",
            )
            .fetch_all(pool)
            .await
            .expect("xmin holders");
            panic!("events never settled: xmin {xmin} <= {horizon}; oldest holders {holders:?}");
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

async fn drain_notifications(pool: &PgPool) {
    // The relay reads only settled events (xact < snapshot xmin), and xmin is
    // cluster-wide: a transaction in another test's database can hold it below
    // an event this test just committed. Wait until every transaction older
    // than now has ended, then drain.
    let horizon: String = sqlx::query_scalar("SELECT pg_current_xact_id()::text")
        .fetch_one(pool)
        .await
        .expect("current xid");
    wait_xmin_past(pool, &horizon).await;
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
        Some(json!({"commandId": uuid::Uuid::now_v7(), "parentId": null, "title": "멘션 문서"})),
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

    // Replay (what --recover-outbox does on restore: the cursor is rebased
    // before events already delivered) must not duplicate notifications.
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.notifications")
        .fetch_one(&admin)
        .await
        .expect("count before replay");
    sqlx::query(
        "UPDATE fvoci.outbox_consumers SET last_xact = '0'::xid8, last_seq = 0 WHERE consumer = 'notifications'",
    )
    .execute(&admin)
    .await
    .expect("rebase cursor");
    drain_notifications(&app_db).await;
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.notifications")
        .fetch_one(&admin)
        .await
        .expect("count after replay");
    assert_eq!(
        after, before,
        "replayed events must not duplicate notifications"
    );

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

#[tokio::test]
async fn group_mention_on_project_document_respects_access_at_delivery() {
    let harness = TestDb::bootstrap().await;
    let (app, owner_cookie, _, workspace_id) = setup_session(&harness).await;
    let admin = admin_pool(&harness).await;
    let app_db = app_pool(&harness).await;
    let member = add_workspace_user(&admin, workspace_id, "member", "viewer").await;
    let outsider = add_workspace_user(&admin, workspace_id, "member", "outsider").await;

    let (status, group) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/groups"),
        Some(json!({"name": "랩팀"})),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{group:?}");
    let group_id = group["id"].as_str().unwrap();
    for user_id in [member.user_id, outsider.user_id] {
        let (status, added) = json_request(
            app.clone(),
            "POST",
            &format!("/api/v1/workspaces/{workspace_id}/groups/{group_id}/members"),
            Some(json!({"userId": user_id.to_string()})),
            Some(&owner_cookie),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{added:?}");
    }

    let hid = create_project(app.clone(), &owner_cookie, workspace_id, "HID", "private").await;
    let project_id = hid["id"].as_str().unwrap();
    let document_id = hid["rootDocumentId"].as_str().expect("root document");
    let (status, granted) = json_request(
        app.clone(),
        "POST",
        &format!("/api/v1/workspaces/{workspace_id}/projects/{project_id}/members"),
        Some(json!({"userId": member.user_id.to_string(), "role": "member"})),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{granted:?}");

    let (status, comment) = json_request(
        app.clone(),
        "POST",
        &format!(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/comments"
        ),
        Some(json!({
            "body": "@랩팀",
            "mentionedGroupIds": [group_id]
        })),
        Some(&owner_cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{comment:?}");
    drain_notifications(&app_db).await;

    let (status, member_listed) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/notifications"),
        None,
        Some(&member.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{member_listed:?}");
    assert!(
        member_listed["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["verb"] == "comment.created"),
        "{member_listed:?}"
    );

    let (status, outsider_listed) = json_request(
        app.clone(),
        "GET",
        &format!("/api/v1/workspaces/{workspace_id}/notifications"),
        None,
        Some(&outsider.cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{outsider_listed:?}");
    assert!(
        outsider_listed["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["verb"] != "comment.created"),
        "{outsider_listed:?}"
    );

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
    // Precondition: 018 records the newest settled event (xact < xmin). xmin is
    // cluster-wide, so another test's open transaction can keep this event
    // unsettled; wait until it is settled before upgrading.
    wait_xmin_past(&admin, &last.0).await;
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

const SEEDED_CONSUMERS: [&str; 5] = ["github", "mail", "notifications", "push", "webhooks"];

/// Cursors of the consumers that 018/020/027/040 seed, by name.
async fn seeded_cursors(admin: &PgPool) -> Vec<(String, String, i64)> {
    sqlx::query_as(
        "SELECT consumer, last_xact::text, last_seq FROM fvoci.outbox_consumers \
         WHERE consumer = ANY($1) ORDER BY consumer",
    )
    .bind(&SEEDED_CONSUMERS[..])
    .fetch_all(admin)
    .await
    .expect("seeded cursors")
}

fn no_cursors() -> Vec<(String, String, i64)> {
    Vec::new()
}

fn cursors_at(xact: &str, seq: i64) -> Vec<(String, String, i64)> {
    SEEDED_CONSUMERS
        .iter()
        .map(|name| (name.to_string(), xact.to_string(), seq))
        .collect()
}

/// A connection to another database on the same cluster, in a transaction
/// with an assigned xid: it holds the cluster-wide xmin at or below that xid
/// until rolled back. It touches no tables.
async fn hold_xid_in_other_database(harness: &TestDb) -> (sqlx::PgConnection, String) {
    use sqlx::Connection;
    let mut server = url::Url::parse(&harness.admin_url).expect("admin url");
    server.set_path("/postgres");
    let mut conn = sqlx::PgConnection::connect(server.as_str())
        .await
        .expect("holder connection");
    sqlx::query("BEGIN")
        .execute(&mut conn)
        .await
        .expect("begin");
    let xid: String = sqlx::query_scalar("SELECT pg_current_xact_id()::text")
        .fetch_one(&mut conn)
        .await
        .expect("holder xid");
    (conn, xid)
}

async fn release_xid(mut conn: sqlx::PgConnection) {
    use sqlx::Connection;
    sqlx::query("ROLLBACK")
        .execute(&mut conn)
        .await
        .expect("rollback holder");
    conn.close().await.expect("close holder");
}

async fn insert_upgrade_workspace(admin: &PgPool) -> Uuid {
    let mut tx = admin.begin().await.expect("tx");
    sqlx::query("SELECT set_config('app.system_ctx', 'on', true)")
        .execute(&mut *tx)
        .await
        .expect("system ctx");
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO fvoci.workspaces (id, slug, name) VALUES (gen_random_uuid(), 'upg', 'Upgrade') RETURNING id",
    )
    .fetch_one(&mut *tx)
    .await
    .expect("workspace");
    tx.commit().await.expect("commit");
    id
}

/// Commits one event created `age_days` ago; returns (id, xact, seq).
async fn commit_event(admin: &PgPool, workspace_id: Uuid, age_days: i32) -> (Uuid, String, i64) {
    let mut tx = admin.begin().await.expect("tx");
    sqlx::query("SELECT set_config('app.system_ctx', 'on', true)")
        .execute(&mut *tx)
        .await
        .expect("system ctx");
    let row: (Uuid, String, i64) = sqlx::query_as(
        "INSERT INTO fvoci.events (id, workspace_id, verb, target_type, target_id, payload, created_at) \
         VALUES (gen_random_uuid(), $1, 'task.created', 'task', gen_random_uuid(), '{}'::jsonb, \
                 now() - make_interval(days => $2)) \
         RETURNING id, xact::text, seq",
    )
    .bind(workspace_id)
    .bind(age_days)
    .fetch_one(&mut *tx)
    .await
    .expect("event");
    tx.commit().await.expect("commit");
    row
}

async fn read_ids(admin: &PgPool, consumer: &str) -> Vec<Uuid> {
    read_events(admin, consumer, 100)
        .await
        .expect("read events")
        .into_iter()
        .map(|event| event.id)
        .collect()
}

/// 018/020/027/040 seed nothing while another database's transaction holds the
/// cluster xmin below the pre-upgrade events. 041 refuses to guess while that
/// holder may be same-database work, then seeds every missing cursor at the
/// newest event once it has settled: no history is replayed, new events flow.
#[tokio::test]
async fn upgrade_repairs_cursors_missed_while_cluster_xmin_lagged() {
    let harness = TestDb::bootstrap_through(17).await;
    let admin = admin_pool(&harness).await;
    let workspace_id = insert_upgrade_workspace(&admin).await;
    let (holder, holder_xid) = hold_xid_in_other_database(&harness).await;
    let (_, last_xact, last_seq) = commit_event(&admin, workspace_id, 0).await;
    let lagging: bool = sqlx::query_scalar(
        "SELECT $1::xid8 < $2::xid8 AND pg_snapshot_xmin(pg_current_snapshot()) <= $1::xid8",
    )
    .bind(&holder_xid)
    .bind(&last_xact)
    .fetch_one(&admin)
    .await
    .expect("xmin precondition");
    assert!(
        lagging,
        "holder {holder_xid} must keep xmin below event {last_xact}"
    );

    fvoci_server::db::migrate::run_migrations_through(&harness.admin_url, 40)
        .await
        .expect("migrate through 040");
    assert_eq!(
        seeded_cursors(&admin).await,
        no_cursors(),
        "018/020/027/040 seed nothing while xmin lags"
    );

    let err = fvoci_server::db::migrate::run_migrations(&harness.admin_url)
        .await
        .expect_err("041 must refuse an unsettled tail");
    let db_err = err.as_database_error().expect("database error");
    assert_eq!(db_err.code().as_deref(), Some("55000"), "{db_err}");
    assert!(db_err.message().contains("is not settled"), "{db_err}");
    assert!(
        fvoci_server::db::migrate::assert_schema_current(&admin)
            .await
            .is_err(),
        "the schema gate keeps the server off"
    );
    assert_eq!(
        seeded_cursors(&admin).await,
        no_cursors(),
        "nothing seeded on failure"
    );

    release_xid(holder).await;
    wait_xmin_past(&admin, &last_xact).await;
    fvoci_server::db::migrate::run_migrations(&harness.admin_url)
        .await
        .expect("rerun migrate after the holder ended");
    fvoci_server::db::migrate::assert_schema_current(&admin)
        .await
        .expect("schema current");
    assert_eq!(
        seeded_cursors(&admin).await,
        cursors_at(&last_xact, last_seq)
    );

    for consumer in SEEDED_CONSUMERS {
        assert!(
            read_ids(&admin, consumer).await.is_empty(),
            "{consumer} replays history"
        );
    }
    let (fresh, fresh_xact, _) = commit_event(&admin, workspace_id, 0).await;
    wait_xmin_past(&admin, &fresh_xact).await;
    for consumer in SEEDED_CONSUMERS {
        assert_eq!(read_ids(&admin, consumer).await, [fresh], "{consumer}");
    }
    admin.close().await;
    harness.cleanup().await;
}

/// Existing cursors are never moved, and an unsettled tail does not block an
/// upgrade that has nothing to repair.
#[tokio::test]
async fn upgrade_keeps_existing_cursors_and_repairs_only_missing_ones() {
    let harness = TestDb::bootstrap_through(40).await;
    let admin = admin_pool(&harness).await;
    let workspace_id = insert_upgrade_workspace(&admin).await;
    for consumer in SEEDED_CONSUMERS {
        sqlx::query("INSERT INTO fvoci.outbox_consumers (consumer) VALUES ($1)")
            .bind(consumer)
            .execute(&admin)
            .await
            .expect("existing cursor");
    }
    let (holder, _) = hold_xid_in_other_database(&harness).await;
    commit_event(&admin, workspace_id, 0).await;
    fvoci_server::db::migrate::run_migrations(&harness.admin_url)
        .await
        .expect("nothing missing: no guard");
    assert_eq!(seeded_cursors(&admin).await, cursors_at("0", 0));
    release_xid(holder).await;

    // Partial: one cursor already exists, the others were missed.
    let harness2 = TestDb::bootstrap_through(40).await;
    let admin2 = admin_pool(&harness2).await;
    let workspace_id = insert_upgrade_workspace(&admin2).await;
    sqlx::query("INSERT INTO fvoci.outbox_consumers (consumer) VALUES ('notifications')")
        .execute(&admin2)
        .await
        .expect("existing notifications cursor");
    let (_, last_xact, last_seq) = commit_event(&admin2, workspace_id, 0).await;
    wait_xmin_past(&admin2, &last_xact).await;
    fvoci_server::db::migrate::run_migrations(&harness2.admin_url)
        .await
        .expect("repair missing cursors");
    let expected: Vec<_> = cursors_at(&last_xact, last_seq)
        .into_iter()
        .map(|(name, xact, seq)| {
            if name == "notifications" {
                (name, "0".to_string(), 0)
            } else {
                (name, xact, seq)
            }
        })
        .collect();
    assert_eq!(seeded_cursors(&admin2).await, expected);
    admin.close().await;
    admin2.close().await;
    harness.cleanup().await;
    harness2.cleanup().await;
}

/// A fresh install has no events: 041 seeds nothing even while xmin lags, and
/// the first `ensure_consumer` starts at the beginning as before.
#[tokio::test]
async fn fresh_install_seeds_no_cursors() {
    let harness = TestDb::bootstrap_through(40).await;
    let admin = admin_pool(&harness).await;
    let (holder, _) = hold_xid_in_other_database(&harness).await;
    fvoci_server::db::migrate::run_migrations(&harness.admin_url)
        .await
        .expect("fresh migrate");
    release_xid(holder).await;
    assert_eq!(seeded_cursors(&admin).await, no_cursors());
    ensure_consumer(&admin, NOTIFICATIONS_CONSUMER)
        .await
        .expect("ensure");
    let cursor: (String, i64) = sqlx::query_as(
        "SELECT last_xact::text, last_seq FROM fvoci.outbox_consumers WHERE consumer = 'notifications'",
    )
    .fetch_one(&admin)
    .await
    .expect("cursor");
    assert_eq!(cursor, ("0".to_string(), 0));
    admin.close().await;
    harness.cleanup().await;
}

/// A pre-041 dump restored into another cluster carries xids past this
/// cluster's xmax, so 018/020/027/040 seeded nothing. 041 seeds at the restored
/// tail; the relay stays fail-closed until --recover-outbox rebases these rows
/// like any other, which replays only its window and then new events.
#[tokio::test]
async fn restored_epoch_seeds_cursors_that_recovery_rebases_within_its_window() {
    use fvoci_server::db::outbox::is_outbox_xid_epoch_mismatch;
    use fvoci_server::db::outbox_recover::{recover_outbox, RecoverOutboxOptions};

    let harness = TestDb::bootstrap_through(40).await;
    let admin = admin_pool(&harness).await;
    let workspace_id = insert_upgrade_workspace(&admin).await;
    let (outside, _, _) = commit_event(&admin, workspace_id, 60).await;
    let (inside, _, inside_seq) = commit_event(&admin, workspace_id, 1).await;
    // Old-cluster xids, past this cluster's xmax (as after a logical restore).
    sqlx::query(
        "UPDATE fvoci.events SET xact = CASE WHEN id = $1 THEN '100000000000'::xid8 \
         ELSE '100000000001'::xid8 END",
    )
    .bind(outside)
    .execute(&admin)
    .await
    .expect("restored xids");
    assert_eq!(seeded_cursors(&admin).await, no_cursors());

    fvoci_server::db::migrate::run_migrations(&harness.admin_url)
        .await
        .expect("041 seeds a restored tail");
    assert_eq!(
        seeded_cursors(&admin).await,
        cursors_at("100000000001", inside_seq)
    );
    for consumer in SEEDED_CONSUMERS {
        let err = read_events(&admin, consumer, 100)
            .await
            .expect_err("relay stays fail-closed before recovery");
        assert!(is_outbox_xid_epoch_mismatch(&err), "{consumer}: {err}");
    }

    let bounds: (chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>) =
        sqlx::query_as("SELECT now() - interval '2 days', now()")
            .fetch_one(&admin)
            .await
            .expect("recovery bounds");
    let db_name: String = sqlx::query_scalar("SELECT current_database()::text")
        .fetch_one(&admin)
        .await
        .expect("db name");
    close_pool(admin).await;
    wait_for_no_client_backends(&harness, &db_name).await;
    let report = recover_outbox(
        &harness.admin_url,
        RecoverOutboxOptions {
            since: bounds
                .0
                .to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
            snapshot_at: bounds
                .1
                .to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
            apply: true,
            reason: Some("test restore of a pre-041 dump".into()),
            acknowledge_external_replay: true,
        },
    )
    .await
    .expect("recover");
    assert_eq!(report.consumers_rebased, 5, "{report:?}");
    assert_eq!((report.eligible, report.excluded), (1, 1), "{report:?}");

    let admin = admin_pool(&harness).await;
    let recovery_xid: String =
        sqlx::query_scalar("SELECT xact::text FROM fvoci.events WHERE id = $1")
            .bind(inside)
            .fetch_one(&admin)
            .await
            .expect("recovered xact");
    assert_eq!(seeded_cursors(&admin).await, cursors_at(&recovery_xid, 0));
    let (fresh, fresh_xact, _) = commit_event(&admin, workspace_id, 0).await;
    wait_xmin_past(&admin, &fresh_xact).await;
    for consumer in SEEDED_CONSUMERS {
        assert_eq!(
            read_ids(&admin, consumer).await,
            [inside, fresh],
            "{consumer}: window replay then new events, never the excluded history"
        );
    }
    admin.close().await;
    harness.cleanup().await;
}

/// `pg_stat_activity` row reported when client backends outlive the wait:
/// (pid, state, application_name, client, backend_start, state_change, query).
type RemainingBackend = (
    i32,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

async fn wait_for_no_client_backends(harness: &TestDb, db_name: &str) {
    let mut server = url::Url::parse(&harness.admin_url).expect("admin url");
    server.set_path("/postgres");
    let observer = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(server.as_str())
        .await
        .expect("observer");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        let n: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity WHERE datname = $1 AND backend_type = 'client backend'",
        )
        .bind(db_name)
        .fetch_one(&observer)
        .await
        .expect("backends");
        if n == 0 {
            break;
        }
        if std::time::Instant::now() >= deadline {
            let rows: Vec<RemainingBackend> = sqlx::query_as(
                "SELECT pid, state, application_name, client_addr::text || ':' || client_port::text, \
                        backend_start::text, state_change::text, left(query, 200) \
                 FROM pg_stat_activity WHERE datname = $1 AND backend_type = 'client backend'",
            )
            .bind(db_name)
            .fetch_all(&observer)
            .await
            .expect("backend rows");
            panic!("{n} client backends remain: {rows:?}");
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    observer.close().await;
}

/// sqlx 0.8.6 `Pool::close` returns before a connection whose return to the
/// pool is under way; that connection then goes idle and stays open while the
/// pool lives. `close_pool` waits it out, so recovery's no-other-sessions
/// precondition holds without depending on when the pool is dropped.
#[tokio::test]
async fn close_pool_closes_a_connection_returned_during_close() {
    let harness = TestDb::bootstrap_through(1).await;
    let admin = admin_pool(&harness).await;
    let db_name: String = sqlx::query_scalar("SELECT current_database()::text")
        .fetch_one(&admin)
        .await
        .expect("db name");
    drop(admin.acquire().await.expect("acquire"));
    // Let the return task pass its closed check and start the on-release ping.
    tokio::task::yield_now().await;
    let lingering = admin.clone();
    close_pool(admin).await;
    assert_eq!(lingering.size(), 0);
    wait_for_no_client_backends(&harness, &db_name).await;
    drop(lingering);
    harness.cleanup().await;
}
