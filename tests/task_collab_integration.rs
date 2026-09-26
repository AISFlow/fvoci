//! Task collaboration rooms (`{ws}:task:{id}`), task revisions, the task block
//! patch and task origins against a real PostgreSQL and the collab-engine child.
#![cfg(feature = "db-tests")]

#[allow(dead_code)]
mod support;

use std::net::SocketAddr;
use std::time::Duration;

use chrono::{Duration as ChronoDuration, Utc};
use futures_util::{SinkExt, StreamExt};
use fvoci_server::auth::password::Keyring;
use fvoci_server::auth::token::new_token;
use fvoci_server::collab::seed::SeedEngine;
use fvoci_server::collab::wire::{
    AuthMessage, CollabKind, CollabRoomName, DocumentMessage, WireFrame,
};
use fvoci_server::db::documents::CreateDocumentInput;
use fvoci_server::db::pool;
use fvoci_server::db::workspace::{self, WorkspaceRole};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use support::{
    auth_and_join, collab_app_state, complete_sync_handshake, connect_member, engine_fixture,
    setup_owner_session, stateless_frame, sync_update_frame, test_collab_config,
    wait_for_stateless_exact, wait_for_sync_applied, wait_for_sync_update, wait_for_ws_close_code,
    SessionFixture, TestDb, TestRun, PEPPER, PUBLIC_ORIGIN,
};
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

const TEST_TIMEOUT: Duration = Duration::from_secs(90);

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn run_test<F>(name: &str, case: F)
where
    F: std::future::Future<Output = ()>,
{
    tokio::time::timeout(TEST_TIMEOUT, case)
        .await
        .unwrap_or_else(|_| panic!("{name} hung (>{TEST_TIMEOUT:?})"));
}

fn task_key(workspace_id: Uuid, task_id: Uuid) -> String {
    CollabRoomName {
        workspace_id,
        kind: CollabKind::Task,
        resource_id: task_id,
    }
    .routing_key()
}

enum Cred<'a> {
    Session(&'a str),
    Bearer(&'a str),
}

async fn call(
    addr: SocketAddr,
    method: Method,
    path: &str,
    cred: Cred<'_>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let client = reqwest::Client::new();
    let mut req = client
        .request(method, format!("http://{addr}{path}"))
        .header("origin", PUBLIC_ORIGIN);
    req = match cred {
        Cred::Session(token) => req.header("cookie", format!("fvoci_session={token}")),
        Cred::Bearer(token) => req.header("authorization", format!("Bearer {token}")),
    };
    if let Some(body) = body {
        req = req.json(&body);
    }
    let response = req.send().await.expect("http");
    let status = response.status();
    let value = response.json().await.unwrap_or(Value::Null);
    (status, value)
}

async fn session_call(
    addr: SocketAddr,
    method: Method,
    path: &str,
    token: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    call(addr, method, path, Cred::Session(token), body).await
}

async fn admin_pool(harness: &TestDb) -> PgPool {
    PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap()
}

async fn admin_exec(harness: &TestDb, sql: &str, id: Uuid) {
    let admin = admin_pool(harness).await;
    sqlx::query(sql).bind(id).execute(&admin).await.unwrap();
    admin.close().await;
}

async fn create_user_session(
    harness: &TestDb,
    workspace_id: Uuid,
    role: WorkspaceRole,
) -> SessionFixture {
    let admin = admin_pool(harness).await;
    let user_id = Uuid::now_v7();
    let hash = fvoci_server::auth::password::hash_password(
        "supersecret1",
        &Keyring::parse(PEPPER, "test").unwrap(),
    )
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO fvoci.users (id, email, password_hash, given_name) VALUES ($1, $2, $3, $4)",
    )
    .bind(user_id)
    .bind(format!("u-{user_id}@example.com"))
    .bind(&hash)
    .bind("User")
    .execute(&admin)
    .await
    .unwrap();
    admin.close().await;
    let pool = pool::connect_app(&harness.app_url).await.unwrap();
    workspace::add_membership_for_test(&pool, workspace_id, user_id, role)
        .await
        .unwrap();
    let token = new_token();
    let session_id = Uuid::now_v7();
    let expires = Utc::now() + ChronoDuration::days(30);
    let mut tx = pool.begin().await.unwrap();
    fvoci_server::db::identity::create_session(&mut tx, session_id, user_id, &token.hash, expires)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    SessionFixture {
        pool,
        user_id,
        session_id,
        workspace_id,
        session_token: token.token,
    }
}

async fn add_project_member(
    harness: &TestDb,
    workspace_id: Uuid,
    project_id: Uuid,
    user_id: Uuid,
    role: &str,
) {
    let admin = admin_pool(harness).await;
    sqlx::query(
        "INSERT INTO fvoci.project_members (id, workspace_id, project_id, user_id, role) VALUES ($5, $1, $2, $3, $4)",
    )
    .bind(workspace_id)
    .bind(project_id)
    .bind(user_id)
    .bind(role)
    .bind(Uuid::now_v7())
    .execute(&admin)
    .await
    .unwrap();
    admin.close().await;
}

async fn insert_pat(
    harness: &TestDb,
    workspace_id: Uuid,
    user_id: Uuid,
    scopes: &[&str],
) -> String {
    let admin = admin_pool(harness).await;
    let token = new_token();
    let scopes: Vec<String> = scopes.iter().map(|s| s.to_string()).collect();
    sqlx::query(
        "INSERT INTO fvoci.api_tokens (id, workspace_id, user_id, token_hash, name, scopes) VALUES ($1, $2, $3, $4, 'pat', $5)",
    )
    .bind(Uuid::now_v7())
    .bind(workspace_id)
    .bind(user_id)
    .bind(&token.hash)
    .bind(&scopes)
    .execute(&admin)
    .await
    .unwrap();
    admin.close().await;
    token.token
}

async fn create_project(addr: SocketAddr, owner: &SessionFixture, key: &str) -> Uuid {
    create_named_project(addr, owner, key, "private").await
}

async fn create_named_project(
    addr: SocketAddr,
    owner: &SessionFixture,
    key: &str,
    visibility: &str,
) -> Uuid {
    let (status, body) = session_call(
        addr,
        Method::POST,
        &format!("/api/v1/workspaces/{}/projects", owner.workspace_id),
        &owner.session_token,
        Some(json!({"key": key, "name": key, "visibility": visibility})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    Uuid::parse_str(body["id"].as_str().expect("project id")).unwrap()
}

fn document_api(workspace_id: Uuid, document_id: Uuid, suffix: &str) -> String {
    format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}{suffix}")
}

async fn create_wiki(owner: &SessionFixture, title: &str) -> Uuid {
    fvoci_server::db::documents::create_wiki_document(
        &owner.pool,
        owner.workspace_id,
        owner.user_id,
        owner.session_id,
        CreateDocumentInput {
            parent_id: None,
            title,
            icon: None,
        },
        None,
    )
    .await
    .unwrap()
    .unwrap()
    .id
}

async fn create_origin_task(
    addr: SocketAddr,
    token: &str,
    workspace_id: Uuid,
    document_id: Uuid,
    project_id: Uuid,
    title: &str,
) -> Uuid {
    let (status, body) = session_call(
        addr,
        Method::POST,
        &document_api(workspace_id, document_id, "/tasks"),
        token,
        Some(json!({
            "projectId": project_id,
            "requestId": Uuid::now_v7(),
            "task": {"title": title}
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    Uuid::parse_str(body["taskId"].as_str().expect("task id")).unwrap()
}

async fn grant_wiki_group_view(
    harness: &TestDb,
    workspace_id: Uuid,
    document_id: Uuid,
    user_id: Uuid,
) {
    let admin = admin_pool(harness).await;
    let group_id = Uuid::now_v7();
    sqlx::query("INSERT INTO fvoci.groups (id, workspace_id, name) VALUES ($1, $2, 'origin-view')")
        .bind(group_id)
        .bind(workspace_id)
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO fvoci.group_members (workspace_id, group_id, user_id) VALUES ($1, $2, $3)",
    )
    .bind(workspace_id)
    .bind(group_id)
    .bind(user_id)
    .execute(&admin)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO fvoci.document_members (id, workspace_id, document_id, group_id, role) VALUES ($1, $2, $3, $4, 'viewer')",
    )
    .bind(Uuid::now_v7())
    .bind(workspace_id)
    .bind(document_id)
    .bind(group_id)
    .execute(&admin)
    .await
    .unwrap();
    admin.close().await;
}

async fn create_task(addr: SocketAddr, owner: &SessionFixture, project_id: Uuid) -> Uuid {
    let (status, body) = session_call(
        addr,
        Method::POST,
        &format!(
            "/api/v1/workspaces/{}/projects/{project_id}/tasks",
            owner.workspace_id
        ),
        &owner.session_token,
        Some(json!({"title": "협업 태스크"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    Uuid::parse_str(body["id"].as_str().expect("task id")).unwrap()
}

fn task_path(s: &SessionFixture, task_id: Uuid, extra: &str) -> String {
    format!(
        "/api/v1/workspaces/{}/tasks/{task_id}{extra}",
        s.workspace_id
    )
}

fn para(id: &str, text: &str) -> Value {
    json!({"type":"paragraph","attrs":{"id":id},"content":[{"type":"text","text":text}]})
}

fn auth_frame(routing_key: &str, client_id: u32) -> Vec<u8> {
    fvoci_server::collab::wire::encode(&WireFrame::Document {
        routing_key: routing_key.to_string(),
        room: CollabRoomName::parse(routing_key),
        message: DocumentMessage::Auth(AuthMessage::Token {
            token: client_id.to_string(),
            provider_version: Some("4.6.0".into()),
        }),
    })
    .expect("encode auth")
}

/// Sends the auth token and returns `Ok(scope)` or `Err(denied reason)`.
async fn authenticate(ws: &mut Ws, routing_key: &str, client_id: u32) -> Result<String, String> {
    ws.send(Message::Binary(auth_frame(routing_key, client_id).into()))
        .await
        .unwrap();
    let msg = tokio::time::timeout(Duration::from_secs(10), ws.next())
        .await
        .expect("auth reply timeout")
        .expect("stream")
        .expect("frame");
    let Message::Binary(bytes) = msg else {
        panic!("expected binary auth reply, got {msg:?}");
    };
    match fvoci_server::collab::wire::decode(&bytes).expect("decode") {
        WireFrame::Document {
            message: DocumentMessage::Auth(AuthMessage::Authenticated { scope }),
            ..
        } => Ok(scope),
        WireFrame::Document {
            message: DocumentMessage::Auth(AuthMessage::PermissionDenied { reason }),
            ..
        } => Err(reason),
        other => panic!("unexpected auth reply {other:?}"),
    }
}

async fn admission(
    addr: SocketAddr,
    token: &str,
    key: &str,
    client_id: u32,
) -> Result<String, String> {
    let mut ws = connect_member(addr, token).await;
    let result = authenticate(&mut ws, key, client_id).await;
    let _ = ws.close(None).await;
    result
}

async fn apply_and_persist(
    addr: SocketAddr,
    token: &str,
    key: &str,
    client_id: u32,
    update: &[u8],
) {
    let mut ws = connect_member(addr, token).await;
    auth_and_join(&mut ws, key, client_id).await;
    complete_sync_handshake(&mut ws, key).await;
    ws.send(Message::Binary(sync_update_frame(key, update).into()))
        .await
        .unwrap();
    assert!(
        wait_for_sync_applied(&mut ws, Duration::from_secs(8)).await,
        "update must apply"
    );
    let request_id = Uuid::now_v7();
    ws.send(Message::Binary(
        stateless_frame(key, &format!("persist:{request_id}")).into(),
    ))
    .await
    .unwrap();
    assert!(
        wait_for_stateless_exact(
            &mut ws,
            &format!("persisted:{request_id}"),
            Duration::from_secs(8)
        )
        .await,
        "persist ack"
    );
    let _ = ws.close(None).await;
}

async fn get_task_json(addr: SocketAddr, s: &SessionFixture, task_id: Uuid) -> Value {
    let (status, body) = session_call(
        addr,
        Method::GET,
        &task_path(s, task_id, ""),
        &s.session_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

/// The derived projection is written by the room after the durable append;
/// bounded read-only wait for the row to reflect `needle`.
async fn wait_task_content_contains(
    pool: &PgPool,
    workspace_id: Uuid,
    task_id: Uuid,
    needle: &str,
) -> (Value, String, String) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        let row: (Value, String, String) = sqlx::query_as(
            "SELECT content_json, text, chosung FROM fvoci.tasks WHERE workspace_id = $1 AND id = $2",
        )
        .bind(workspace_id)
        .bind(task_id)
        .fetch_one(pool)
        .await
        .unwrap();
        if row.0.to_string().contains(needle) {
            return row;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("task projection never contained {needle}: {}", row.0);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

struct TaskFixture {
    owner: SessionFixture,
    project_id: Uuid,
    task_id: Uuid,
}

async fn setup_task(run: &mut TestRun, revoke_poll_ms: u64) -> (SocketAddr, TaskFixture) {
    let owner = setup_owner_session(&run.harness).await;
    let mut cfg = test_collab_config(8, 60_000);
    cfg.revoke_poll_ms = revoke_poll_ms;
    let (state, hub) = collab_app_state(&run.harness.app_url, cfg).await;
    let addr = run.spawn_router_state(state, hub).await;
    let project_id = create_project(addr, &owner, "TCOL").await;
    let task_id = create_task(addr, &owner, project_id).await;
    (
        addr,
        TaskFixture {
            owner,
            project_id,
            task_id,
        },
    )
}

async fn seed_update(run: &TestRun, content: Value) -> Vec<u8> {
    SeedEngine::from_hub(&run.hub())
        .tiptap_to_yjs_update(&content)
        .await
        .expect("seed update")
}

#[tokio::test]
async fn task_room_admission_matrix() {
    run_test("task_room_admission_matrix", async {
        let mut run = TestRun::new(TestDb::bootstrap().await);
        let (addr, f) = setup_task(&mut run, 5_000).await;
        let ws_id = f.owner.workspace_id;
        let key = task_key(ws_id, f.task_id);

        let editor = create_user_session(&run.harness, ws_id, WorkspaceRole::Member).await;
        add_project_member(&run.harness, ws_id, f.project_id, editor.user_id, "member").await;
        let viewer = create_user_session(&run.harness, ws_id, WorkspaceRole::Member).await;
        add_project_member(&run.harness, ws_id, f.project_id, viewer.user_id, "viewer").await;
        let guest_viewer = create_user_session(&run.harness, ws_id, WorkspaceRole::Guest).await;
        add_project_member(
            &run.harness,
            ws_id,
            f.project_id,
            guest_viewer.user_id,
            "viewer",
        )
        .await;
        let guest = create_user_session(&run.harness, ws_id, WorkspaceRole::Guest).await;
        let outsider = create_user_session(&run.harness, ws_id, WorkspaceRole::Member).await;

        assert_eq!(
            admission(addr, &f.owner.session_token, &key, 1).await,
            Ok("read-write".into())
        );
        assert_eq!(
            admission(addr, &editor.session_token, &key, 2).await,
            Ok("read-write".into())
        );
        assert_eq!(
            admission(addr, &viewer.session_token, &key, 3).await,
            Ok("readonly".into())
        );
        assert_eq!(
            admission(addr, &guest_viewer.session_token, &key, 4).await,
            Ok("readonly".into())
        );
        assert_eq!(
            admission(addr, &guest.session_token, &key, 5).await,
            Err("not found".into())
        );
        assert_eq!(
            admission(addr, &outsider.session_token, &key, 6).await,
            Err("not found".into())
        );
        // A task id under the document kind, and an unknown task id, are not found.
        let doc_kind_key = CollabRoomName {
            workspace_id: ws_id,
            kind: CollabKind::Document,
            resource_id: f.task_id,
        }
        .routing_key();
        assert_eq!(
            admission(addr, &f.owner.session_token, &doc_kind_key, 7).await,
            Err("not found".into())
        );
        assert_eq!(
            admission(
                addr,
                &f.owner.session_token,
                &task_key(ws_id, Uuid::now_v7()),
                8
            )
            .await,
            Err("not found".into())
        );

        // Viewer's writes are refused by the room: no tail row appears.
        let mut ws = connect_member(addr, &viewer.session_token).await;
        assert_eq!(authenticate(&mut ws, &key, 9).await, Ok("readonly".into()));
        complete_sync_handshake(&mut ws, &key).await;
        ws.send(Message::Binary(
            sync_update_frame(&key, &engine_fixture("structured.v1")).into(),
        ))
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let _ = ws.close(None).await;
        let admin = admin_pool(&run.harness).await;
        let tail: i64 =
            sqlx::query_scalar("SELECT count(*) FROM fvoci.task_collab_updates WHERE task_id = $1")
                .bind(f.task_id)
                .fetch_one(&admin)
                .await
                .unwrap();
        assert_eq!(tail, 0, "readonly member must not append");

        // Archived task → readonly for an editor; archived project → readonly; trashed → not found.
        sqlx::query("UPDATE fvoci.tasks SET archived_at = now() WHERE id = $1")
            .bind(f.task_id)
            .execute(&admin)
            .await
            .unwrap();
        assert_eq!(
            admission(addr, &editor.session_token, &key, 10).await,
            Ok("readonly".into())
        );
        sqlx::query("UPDATE fvoci.tasks SET archived_at = NULL WHERE id = $1")
            .bind(f.task_id)
            .execute(&admin)
            .await
            .unwrap();
        sqlx::query("UPDATE fvoci.projects SET status = 'archived' WHERE id = $1")
            .bind(f.project_id)
            .execute(&admin)
            .await
            .unwrap();
        assert_eq!(
            admission(addr, &f.owner.session_token, &key, 11).await,
            Ok("readonly".into())
        );
        sqlx::query("UPDATE fvoci.projects SET status = 'active' WHERE id = $1")
            .bind(f.project_id)
            .execute(&admin)
            .await
            .unwrap();
        sqlx::query("UPDATE fvoci.tasks SET deleted_at = now() WHERE id = $1")
            .bind(f.task_id)
            .execute(&admin)
            .await
            .unwrap();
        assert_eq!(
            admission(addr, &f.owner.session_token, &key, 12).await,
            Err("not found".into())
        );
        admin.close().await;
        run.finish().await.expect("cleanup");
    })
    .await;
}

#[tokio::test]
async fn task_room_revocation_mid_session_closes_writer() {
    run_test("task_room_revocation_mid_session_closes_writer", async {
        let mut run = TestRun::new(TestDb::bootstrap().await);
        let (addr, f) = setup_task(&mut run, 150).await;
        let ws_id = f.owner.workspace_id;
        let key = task_key(ws_id, f.task_id);
        let editor = create_user_session(&run.harness, ws_id, WorkspaceRole::Member).await;
        add_project_member(&run.harness, ws_id, f.project_id, editor.user_id, "member").await;
        let editor2 = create_user_session(&run.harness, ws_id, WorkspaceRole::Member).await;
        add_project_member(&run.harness, ws_id, f.project_id, editor2.user_id, "member").await;

        // Membership removed while joined → 1008 on the poll tick.
        let mut ws = connect_member(addr, &editor.session_token).await;
        assert_eq!(
            authenticate(&mut ws, &key, 21).await,
            Ok("read-write".into())
        );
        complete_sync_handshake(&mut ws, &key).await;
        admin_exec(
            &run.harness,
            "DELETE FROM fvoci.project_members WHERE user_id = $1",
            editor.user_id,
        )
        .await;
        wait_for_ws_close_code(
            &mut ws,
            1008,
            Duration::from_secs(5),
            false,
            Some("permission revoked"),
        )
        .await;

        // A read-write socket is closed when the task becomes archived (readonly now).
        let mut ws2 = connect_member(addr, &editor2.session_token).await;
        assert_eq!(
            authenticate(&mut ws2, &key, 22).await,
            Ok("read-write".into())
        );
        complete_sync_handshake(&mut ws2, &key).await;
        admin_exec(
            &run.harness,
            "UPDATE fvoci.tasks SET archived_at = now() WHERE id = $1",
            f.task_id,
        )
        .await;
        wait_for_ws_close_code(
            &mut ws2,
            1008,
            Duration::from_secs(5),
            false,
            Some("permission revoked"),
        )
        .await;

        // Archiving the containing project after a writer joined also
        // downgrades admission; the live writer must be closed on the poll.
        admin_exec(
            &run.harness,
            "UPDATE fvoci.tasks SET archived_at = NULL WHERE id = $1",
            f.task_id,
        )
        .await;
        let mut ws_project = connect_member(addr, &editor2.session_token).await;
        assert_eq!(
            authenticate(&mut ws_project, &key, 24).await,
            Ok("read-write".into())
        );
        complete_sync_handshake(&mut ws_project, &key).await;
        admin_exec(
            &run.harness,
            "UPDATE fvoci.projects SET status = 'archived' WHERE id = $1",
            f.project_id,
        )
        .await;
        wait_for_ws_close_code(
            &mut ws_project,
            1008,
            Duration::from_secs(5),
            false,
            Some("permission revoked"),
        )
        .await;

        // Trashing the task closes a readonly socket too.
        let mut ws3 = connect_member(addr, &f.owner.session_token).await;
        assert_eq!(
            authenticate(&mut ws3, &key, 23).await,
            Ok("readonly".into())
        );
        complete_sync_handshake(&mut ws3, &key).await;
        admin_exec(
            &run.harness,
            "UPDATE fvoci.tasks SET deleted_at = now() WHERE id = $1",
            f.task_id,
        )
        .await;
        wait_for_ws_close_code(
            &mut ws3,
            1008,
            Duration::from_secs(5),
            false,
            Some("permission revoked"),
        )
        .await;
        run.finish().await.expect("cleanup");
    })
    .await;
}

#[tokio::test]
async fn task_body_persists_projects_and_survives_restart() {
    run_test("task_body_persists_projects_and_survives_restart", async {
        let mut run = TestRun::new(TestDb::bootstrap().await);
        let (addr, f) = setup_task(&mut run, 5_000).await;
        let ws_id = f.owner.workspace_id;
        let key = task_key(ws_id, f.task_id);
        let update = seed_update(&run, json!({"type":"doc","content":[para("t-1", "태스크 본문 한글")]})).await;
        apply_and_persist(addr, &f.owner.session_token, &key, 31, &update).await;

        let admin = admin_pool(&run.harness).await;
        let (content, text, chosung) = wait_task_content_contains(&admin, ws_id, f.task_id, "태스크 본문 한글").await;
        assert_eq!(content["content"][0]["attrs"]["id"], "t-1");
        assert!(text.contains("태스크 본문 한글"), "{text}");
        assert!(chosung.contains("ㅌㅅㅋ"), "{chosung}");
        let states: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.task_states WHERE task_id = $1")
            .bind(f.task_id).fetch_one(&admin).await.unwrap();
        assert_eq!(states, 1);
        let docs_rows: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.document_states WHERE document_id = $1")
            .bind(f.task_id).fetch_one(&admin).await.unwrap();
        assert_eq!(docs_rows, 0, "task state must not land in document tables");
        let appended: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM fvoci.events WHERE target_id = $1 AND verb = 'task.collab_update_appended' AND target_type = 'task'",
        ).bind(f.task_id).fetch_one(&admin).await.unwrap();
        assert!(appended >= 1);
        let updated: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM fvoci.events WHERE target_id = $1 AND verb = 'task.updated' AND channel = 'system' AND payload->>'collab' = 'true' AND payload->>'taskId' = $1::text",
        ).bind(f.task_id).fetch_one(&admin).await.unwrap();
        assert!(updated >= 1, "derived projection must record task.updated collab");
        let body = get_task_json(addr, &f.owner, f.task_id).await;
        assert!(body["contentJson"].to_string().contains("태스크 본문 한글"));

        // Restart: a new server restores the room from task_states + tail.
        run.shutdown_last_server().await.expect("shutdown");
        let (state, hub) = collab_app_state(&run.harness.app_url, test_collab_config(8, 60_000)).await;
        let addr = run.spawn_router_state(state, hub).await;
        let body = get_task_json(addr, &f.owner, f.task_id).await;
        assert!(body["contentJson"].to_string().contains("태스크 본문 한글"));
        let mut observer = connect_member(addr, &f.owner.session_token).await;
        auth_and_join(&mut observer, &key, 32).await;
        complete_sync_handshake(&mut observer, &key).await;
        let second = seed_update(&run, json!({"type":"doc","content":[para("t-2", "재시작 후 편집")]})).await;
        apply_and_persist(addr, &f.owner.session_token, &key, 33, &second).await;
        assert!(wait_for_sync_update(&mut observer, Duration::from_secs(8)).await);
        let (content, _, _) = wait_task_content_contains(&admin, ws_id, f.task_id, "재시작 후 편집").await;
        let joined = content.to_string();
        assert!(joined.contains("태스크 본문 한글") && joined.contains("재시작 후 편집"), "{joined}");
        let _ = observer.close(None).await;
        admin.close().await;
        run.finish().await.expect("cleanup");
    })
    .await;
}

#[tokio::test]
async fn task_revisions_create_list_get_restore_through_room() {
    run_test("task_revisions_create_list_get_restore_through_room", async {
        let mut run = TestRun::new(TestDb::bootstrap().await);
        let (addr, f) = setup_task(&mut run, 5_000).await;
        let ws_id = f.owner.workspace_id;
        let key = task_key(ws_id, f.task_id);
        let s = &f.owner;
        let viewer = create_user_session(&run.harness, ws_id, WorkspaceRole::Member).await;
        add_project_member(&run.harness, ws_id, f.project_id, viewer.user_id, "viewer").await;

        // No collab state yet → create is 404 like the source (not a collab task).
        let (status, _) = session_call(addr, Method::POST, &task_path(s, f.task_id, "/revisions"), &s.session_token, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let first = seed_update(&run, json!({"type":"doc","content":[para("r-1", "첫 버전")]})).await;
        apply_and_persist(addr, &s.session_token, &key, 41, &first).await;
        let (status, created) = session_call(addr, Method::POST, &task_path(s, f.task_id, "/revisions"), &s.session_token, None).await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        let revision_id = created["id"].as_str().expect("id").to_string();
        // Unchanged content dedupes to the same revision.
        let (status, again) = session_call(addr, Method::POST, &task_path(s, f.task_id, "/revisions"), &s.session_token, None).await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(again["id"], revision_id.as_str());

        let (status, list) = session_call(addr, Method::GET, &task_path(s, f.task_id, "/revisions?limit=20"), &viewer.session_token, None).await;
        assert_eq!(status, StatusCode::OK, "{list}");
        assert_eq!(list["items"].as_array().unwrap().len(), 1);
        assert_eq!(list["items"][0]["targetKind"], "task");
        assert_eq!(list["items"][0]["reason"], "manual");
        let (status, detail) = session_call(addr, Method::GET, &task_path(s, f.task_id, &format!("/revisions/{revision_id}")), &viewer.session_token, None).await;
        assert_eq!(status, StatusCode::OK, "{detail}");
        assert!(detail["contentJson"].to_string().contains("첫 버전"));
        assert!(detail["ySnapshot"].as_str().is_some());

        // Viewer cannot create or restore; a document path cannot read a task revision.
        let (status, _) = session_call(addr, Method::POST, &task_path(s, f.task_id, "/revisions"), &viewer.session_token, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = session_call(addr, Method::POST, &task_path(s, f.task_id, &format!("/revisions/{revision_id}/restore")), &viewer.session_token, Some(json!({}))).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = session_call(
            addr,
            Method::GET,
            &format!("/api/v1/workspaces/{ws_id}/documents/{}/revisions/{revision_id}", f.task_id),
            &s.session_token,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // Replace the body, then restore the first revision through the live room.
        let mut observer = connect_member(addr, &s.session_token).await;
        auth_and_join(&mut observer, &key, 42).await;
        complete_sync_handshake(&mut observer, &key).await;
        let (status, _) = session_call(
            addr,
            Method::PATCH,
            &task_path(s, f.task_id, "/blocks/r-1"),
            &s.session_token,
            Some(json!({"type":"paragraph","content":[{"type":"text","text":"둘째 버전"}]})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(wait_for_sync_update(&mut observer, Duration::from_secs(8)).await);
        let admin = admin_pool(&run.harness).await;
        wait_task_content_contains(&admin, ws_id, f.task_id, "둘째 버전").await;
        let (status, restored) = session_call(
            addr,
            Method::POST,
            &task_path(s, f.task_id, &format!("/revisions/{revision_id}/restore")),
            &s.session_token,
            Some(json!({"correlationId": Uuid::now_v7()})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{restored}");
        assert_eq!(restored["restored"], true);
        assert!(wait_for_sync_update(&mut observer, Duration::from_secs(8)).await, "restore must broadcast");
        let (content, _, _) = wait_task_content_contains(&admin, ws_id, f.task_id, "첫 버전").await;
        assert!(!content.to_string().contains("둘째 버전"), "{content}");
        let requested: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM fvoci.events WHERE target_id = $1 AND verb = 'task.updated' AND payload->>'restoreRequested' = $2",
        ).bind(f.task_id).bind(&revision_id).fetch_one(&admin).await.unwrap();
        assert_eq!(requested, 1);

        // Task revision PAT access follows tasks.read/write; document
        // revision routes remain session-only and rooms still need a cookie.
        let reader = insert_pat(&run.harness, ws_id, s.user_id, &["tasks.read"]).await;
        let writer = insert_pat(&run.harness, ws_id, s.user_id, &["tasks.write"]).await;
        let wrong = insert_pat(&run.harness, ws_id, s.user_id, &["documents.read"]).await;
        let (status, list) = call(addr, Method::GET, &task_path(s, f.task_id, "/revisions"), Cred::Bearer(&reader), None).await;
        assert_eq!(status, StatusCode::OK, "{list}");
        assert_eq!(list["items"].as_array().unwrap().len(), 1);
        let (status, detail) = call(addr, Method::GET, &task_path(s, f.task_id, &format!("/revisions/{revision_id}")), Cred::Bearer(&reader), None).await;
        assert_eq!(status, StatusCode::OK, "{detail}");
        assert_eq!(detail["id"], revision_id.as_str());
        let (status, _) = call(addr, Method::POST, &task_path(s, f.task_id, "/revisions"), Cred::Bearer(&reader), None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = call(addr, Method::POST, &task_path(s, f.task_id, &format!("/revisions/{revision_id}/restore")), Cred::Bearer(&reader), Some(json!({}))).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = call(addr, Method::GET, &task_path(s, f.task_id, "/revisions"), Cred::Bearer(&wrong), None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = call(addr, Method::GET,
            &format!("/api/v1/workspaces/{ws_id}/documents/{}/revisions", f.task_id),
            Cred::Bearer(&writer), None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let viewer_writer = insert_pat(&run.harness, ws_id, viewer.user_id, &["tasks.write"]).await;
        let (status, _) = call(addr, Method::POST, &task_path(s, f.task_id, "/revisions"), Cred::Bearer(&viewer_writer), None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "PAT scope cannot replace project Edit");

        let (status, _) = session_call(addr, Method::PATCH, &task_path(s, f.task_id, "/blocks/r-1"), &s.session_token,
            Some(json!({"type":"paragraph","content":[{"type":"text","text":"PAT version"}]}))).await;
        assert_eq!(status, StatusCode::OK);
        let (status, pat_created) = call(addr, Method::POST, &task_path(s, f.task_id, "/revisions"), Cred::Bearer(&writer), None).await;
        assert_eq!(status, StatusCode::CREATED, "{pat_created}");
        assert_ne!(pat_created["id"], revision_id.as_str());
        let (status, restored) = call(addr, Method::POST, &task_path(s, f.task_id, &format!("/revisions/{revision_id}/restore")), Cred::Bearer(&writer), Some(json!({}))).await;
        assert_eq!(status, StatusCode::OK, "{restored}");
        assert_eq!(restored["restored"], true);
        let (content, _, _) = wait_task_content_contains(&admin, ws_id, f.task_id, "첫 버전").await;
        assert!(!content.to_string().contains("PAT version"));
        sqlx::query("DELETE FROM fvoci.api_tokens WHERE token_hash = $1")
            .bind(fvoci_server::auth::token::hash_token(&writer))
            .execute(&admin).await.unwrap();
        let (status, _) = call(addr, Method::POST, &task_path(s, f.task_id, "/revisions"), Cred::Bearer(&writer), None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "revoked PAT cannot create");

        // Archived task: list still works, create/restore are 409 task_archived.
        sqlx::query("UPDATE fvoci.tasks SET archived_at = now() WHERE id = $1").bind(f.task_id).execute(&admin).await.unwrap();
        let (status, _) = session_call(addr, Method::GET, &task_path(s, f.task_id, "/revisions"), &s.session_token, None).await;
        assert_eq!(status, StatusCode::OK);
        let (status, problem) = session_call(addr, Method::POST, &task_path(s, f.task_id, "/revisions"), &s.session_token, None).await;
        assert_eq!(status, StatusCode::CONFLICT, "{problem}");
        assert_eq!(problem["code"], "task_archived");
        let (status, problem) = session_call(addr, Method::POST, &task_path(s, f.task_id, &format!("/revisions/{revision_id}/restore")), &s.session_token, Some(json!({}))).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(problem["code"], "task_archived");
        sqlx::query("UPDATE fvoci.tasks SET archived_at = NULL WHERE id = $1").bind(f.task_id).execute(&admin).await.unwrap();
        sqlx::query("UPDATE fvoci.projects SET status = 'archived' WHERE id = $1").bind(f.project_id).execute(&admin).await.unwrap();
        let (status, problem) = session_call(addr, Method::POST, &task_path(s, f.task_id, "/revisions"), &s.session_token, None).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(problem["code"], "project_archived");
        sqlx::query("UPDATE fvoci.projects SET status = 'active' WHERE id = $1").bind(f.project_id).execute(&admin).await.unwrap();
        // Trashed task: everything is 404.
        sqlx::query("UPDATE fvoci.tasks SET deleted_at = now() WHERE id = $1").bind(f.task_id).execute(&admin).await.unwrap();
        let (status, _) = session_call(addr, Method::GET, &task_path(s, f.task_id, "/revisions"), &s.session_token, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let _ = observer.close(None).await;
        admin.close().await;
        run.finish().await.expect("cleanup");
    })
    .await;
}

#[tokio::test]
async fn task_block_patch_authz_and_conflicts() {
    run_test("task_block_patch_authz_and_conflicts", async {
        let mut run = TestRun::new(TestDb::bootstrap().await);
        let (addr, f) = setup_task(&mut run, 5_000).await;
        let ws_id = f.owner.workspace_id;
        let key = task_key(ws_id, f.task_id);
        let s = &f.owner;
        let update = seed_update(&run, json!({"type":"doc","content":[para("b-1", "첫 문단"), para("b-2", "둘째 문단")]})).await;
        apply_and_persist(addr, &s.session_token, &key, 51, &update).await;

        let (status, meta) = session_call(
            addr,
            Method::PATCH,
            &task_path(s, f.task_id, "/blocks/b-2"),
            &s.session_token,
            Some(json!({"type":"heading","attrs":{"level":2},"content":[{"type":"text","text":"바뀐 제목"}]})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{meta}");
        assert_eq!(meta["id"], f.task_id.to_string());
        let admin = admin_pool(&run.harness).await;
        let (content, _, _) = wait_task_content_contains(&admin, ws_id, f.task_id, "바뀐 제목").await;
        assert_eq!(content["content"][0]["attrs"]["id"], "b-1");
        assert_eq!(content["content"][1]["type"], "heading");
        assert_eq!(content["content"][1]["attrs"]["id"], "b-2");
        let body = get_task_json(addr, s, f.task_id).await;
        assert!(body["contentJson"].to_string().contains("바뀐 제목"));

        let (status, _) = session_call(addr, Method::PATCH, &task_path(s, f.task_id, "/blocks/nope"), &s.session_token, Some(json!({"type":"paragraph"}))).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, problem) = session_call(addr, Method::PATCH, &task_path(s, f.task_id, "/blocks/b-1"), &s.session_token, Some(json!({"type":"paragraph","attrs":{"id":"other"}}))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(problem["code"], "invalid_document_body");

        // PAT: tasks.write may patch, tasks.read may not.
        let writer = insert_pat(&run.harness, ws_id, s.user_id, &["tasks.write"]).await;
        let reader = insert_pat(&run.harness, ws_id, s.user_id, &["tasks.read"]).await;
        let (status, body) = call(addr, Method::PATCH, &task_path(s, f.task_id, "/blocks/b-1"), Cred::Bearer(&writer), Some(json!({"type":"paragraph","content":[{"type":"text","text":"토큰 편집"}]}))).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        wait_task_content_contains(&admin, ws_id, f.task_id, "토큰 편집").await;
        let (status, _) = call(addr, Method::PATCH, &task_path(s, f.task_id, "/blocks/b-1"), Cred::Bearer(&reader), Some(json!({"type":"paragraph"}))).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // Viewer 404, archived task/project 409.
        let viewer = create_user_session(&run.harness, ws_id, WorkspaceRole::Member).await;
        add_project_member(&run.harness, ws_id, f.project_id, viewer.user_id, "viewer").await;
        let (status, _) = session_call(addr, Method::PATCH, &task_path(s, f.task_id, "/blocks/b-1"), &viewer.session_token, Some(json!({"type":"paragraph"}))).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        sqlx::query("UPDATE fvoci.tasks SET archived_at = now() WHERE id = $1").bind(f.task_id).execute(&admin).await.unwrap();
        let (status, problem) = session_call(addr, Method::PATCH, &task_path(s, f.task_id, "/blocks/b-1"), &s.session_token, Some(json!({"type":"paragraph"}))).await;
        assert_eq!(status, StatusCode::CONFLICT, "{problem}");
        assert_eq!(problem["code"], "task_archived");
        sqlx::query("UPDATE fvoci.tasks SET archived_at = NULL WHERE id = $1").bind(f.task_id).execute(&admin).await.unwrap();
        sqlx::query("UPDATE fvoci.projects SET status = 'archived' WHERE id = $1").bind(f.project_id).execute(&admin).await.unwrap();
        let (status, problem) = session_call(addr, Method::PATCH, &task_path(s, f.task_id, "/blocks/b-1"), &s.session_token, Some(json!({"type":"paragraph"}))).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(problem["code"], "project_archived");
        sqlx::query("UPDATE fvoci.projects SET status = 'active' WHERE id = $1").bind(f.project_id).execute(&admin).await.unwrap();

        // A stale tail precondition is a 409 from the room (after the route's retries
        // it only surfaces under sustained concurrent edits; exercise the room directly).
        let hub = run.hub();
        let room = fvoci_server::collab::room::RoomKey::task(ws_id, f.task_id);
        let live = hub.project_live(room, s.user_id, s.session_id).await.expect("live");
        let seed = seed_update(&run, json!({"type":"doc","content":[para("b-1", "경합")]})).await;
        let stale = hub
            .replace_body(room, s.user_id, s.session_id, seed, Some(live.tail_seq - 1))
            .await;
        assert!(matches!(stale, Err(fvoci_server::collab::room::BodyWriteError::Conflict)), "{stale:?}");
        admin.close().await;
        run.finish().await.expect("cleanup");
    })
    .await;
}

#[tokio::test]
async fn task_origin_create_replay_and_get_authz() {
    run_test("task_origin_create_replay_and_get_authz", async {
        let mut run = TestRun::new(TestDb::bootstrap().await);
        let (addr, f) = setup_task(&mut run, 5_000).await;
        let ws_id = f.owner.workspace_id;
        let s = &f.owner;
        let wiki = fvoci_server::db::documents::create_wiki_document(
            &s.pool,
            ws_id,
            s.user_id,
            s.session_id,
            CreateDocumentInput { parent_id: None, title: "원본 문서", icon: None },
            None,
        )
        .await
        .unwrap()
        .unwrap()
        .id;
        let path = format!("/api/v1/workspaces/{ws_id}/documents/{wiki}/tasks");
        let request_id = Uuid::now_v7();
        let body = json!({"projectId": f.project_id, "requestId": request_id, "anchor": "blk-9", "task": {"title": "문서에서 만든 태스크"}});
        let (status, created) = session_call(addr, Method::POST, &path, &s.session_token, Some(body.clone())).await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        let task_id = created["taskId"].as_str().unwrap().to_string();
        let (status, replay) = session_call(addr, Method::POST, &path, &s.session_token, Some(body)).await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(replay["taskId"], task_id.as_str());
        let (status, problem) = session_call(
            addr,
            Method::POST,
            &path,
            &s.session_token,
            Some(json!({"projectId": f.project_id, "requestId": request_id, "anchor": "blk-9", "task": {"title": "다른 제목"}})),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(problem["code"], "document_version_mismatch");
        let (status, _) = session_call(addr, Method::POST, &path, &s.session_token, Some(json!({"projectId": f.project_id, "requestId": Uuid::now_v7(), "anchor": "x".repeat(201), "task": {"title": "t"}}))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let admin = admin_pool(&run.harness).await;
        let origins: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.task_origins WHERE document_id = $1")
            .bind(wiki).fetch_one(&admin).await.unwrap();
        assert_eq!(origins, 1);

        let task_uuid = Uuid::parse_str(&task_id).unwrap();
        let (status, origin) = session_call(addr, Method::GET, &task_path(s, task_uuid, "/origin"), &s.session_token, None).await;
        assert_eq!(status, StatusCode::OK, "{origin}");
        assert_eq!(origin["count"], 1);
        assert_eq!(origin["items"][0]["documentId"], wiki.to_string());
        assert_eq!(origin["items"][0]["documentTitle"], "원본 문서");
        assert_eq!(origin["items"][0]["taskTitle"], "문서에서 만든 태스크");
        assert_eq!(origin["items"][0]["anchor"], "blk-9");
        assert!(origin["items"][0]["documentDisplayId"].as_str().unwrap().starts_with("WIKI-"));
        assert!(origin["nextCursor"].is_null());

        // Viewer of the project: can read the origin, cannot create into it.
        let viewer = create_user_session(&run.harness, ws_id, WorkspaceRole::Member).await;
        add_project_member(&run.harness, ws_id, f.project_id, viewer.user_id, "viewer").await;
        let (status, _) = session_call(addr, Method::POST, &path, &viewer.session_token, Some(json!({"projectId": f.project_id, "requestId": Uuid::now_v7(), "task": {"title": "t"}}))).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, origin) = session_call(addr, Method::GET, &task_path(s, task_uuid, "/origin"), &viewer.session_token, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(origin["count"], 1);

        // Guest with project access but no view on the wiki document: origin hidden (200, empty).
        let guest = create_user_session(&run.harness, ws_id, WorkspaceRole::Guest).await;
        add_project_member(&run.harness, ws_id, f.project_id, guest.user_id, "member").await;
        let (status, origin) = session_call(addr, Method::GET, &task_path(s, task_uuid, "/origin"), &guest.session_token, None).await;
        assert_eq!(status, StatusCode::OK, "{origin}");
        assert_eq!(origin["count"], 0);
        assert_eq!(origin["items"].as_array().unwrap().len(), 0);
        // …and cannot create from a document it cannot view.
        let (status, _) = session_call(addr, Method::POST, &path, &guest.session_token, Some(json!({"projectId": f.project_id, "requestId": Uuid::now_v7(), "task": {"title": "t"}}))).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // Outsider: task not visible → 404.
        let outsider = create_user_session(&run.harness, ws_id, WorkspaceRole::Member).await;
        let (status, _) = session_call(addr, Method::GET, &task_path(s, task_uuid, "/origin"), &outsider.session_token, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // PAT needs both scopes.
        let docs_only = insert_pat(&run.harness, ws_id, s.user_id, &["documents.read"]).await;
        let both = insert_pat(&run.harness, ws_id, s.user_id, &["documents.read", "tasks.read"]).await;
        let (status, _) = call(addr, Method::GET, &task_path(s, task_uuid, "/origin"), Cred::Bearer(&docs_only), None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, origin) = call(addr, Method::GET, &task_path(s, task_uuid, "/origin"), Cred::Bearer(&both), None).await;
        assert_eq!(status, StatusCode::OK, "{origin}");
        assert_eq!(origin["count"], 1);
        let write_docs_only = insert_pat(&run.harness, ws_id, s.user_id, &["documents.write"]).await;
        let (status, _) = call(addr, Method::POST, &path, Cred::Bearer(&write_docs_only), Some(json!({"projectId": f.project_id, "requestId": Uuid::now_v7(), "task": {"title": "t"}}))).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // Trashed source document hides the origin; the task still reads.
        sqlx::query("UPDATE fvoci.documents SET deleted_at = now() WHERE id = $1").bind(wiki).execute(&admin).await.unwrap();
        let (status, origin) = session_call(addr, Method::GET, &task_path(s, task_uuid, "/origin"), &s.session_token, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(origin["count"], 0);
        admin.close().await;
        run.finish().await.expect("cleanup");
    })
    .await;
}

#[tokio::test]
async fn task_origin_replay_rechecks_edit_and_concurrent_request_is_single_create() {
    run_test("task_origin_replay_rechecks_edit_and_concurrent_request_is_single_create", async {
        let mut run = TestRun::new(TestDb::bootstrap().await);
        let (addr, f) = setup_task(&mut run, 5_000).await;
        let ws_id = f.owner.workspace_id;
        let editor = create_user_session(&run.harness, ws_id, WorkspaceRole::Member).await;
        add_project_member(&run.harness, ws_id, f.project_id, editor.user_id, "member").await;
        let wiki = fvoci_server::db::documents::create_wiki_document(
            &f.owner.pool,
            ws_id,
            f.owner.user_id,
            f.owner.session_id,
            CreateDocumentInput { parent_id: None, title: "Origin source", icon: None },
            None,
        )
        .await
        .unwrap()
        .unwrap()
        .id;
        let path = format!("/api/v1/workspaces/{ws_id}/documents/{wiki}/tasks");
        let body = json!({"projectId": f.project_id, "requestId": Uuid::now_v7(), "task": {"title": "Replay target"}});
        let (status, created) = session_call(addr, Method::POST, &path, &editor.session_token, Some(body.clone())).await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        let first_id = created["taskId"].as_str().unwrap();
        let admin = admin_pool(&run.harness).await;
        sqlx::query("UPDATE fvoci.project_members SET role = 'viewer' WHERE workspace_id = $1 AND project_id = $2 AND user_id = $3")
            .bind(ws_id).bind(f.project_id).bind(editor.user_id).execute(&admin).await.unwrap();
        let (status, _) = session_call(addr, Method::POST, &path, &editor.session_token, Some(body.clone())).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "replay needs current Edit");
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.task_origins WHERE workspace_id = $1 AND document_id = $2")
            .bind(ws_id).bind(wiki).fetch_one(&admin).await.unwrap();
        assert_eq!(count, 1);
        let tasks_after_denial: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.tasks WHERE workspace_id = $1 AND project_id = $2 AND title = 'Replay target'")
            .bind(ws_id).bind(f.project_id).fetch_one(&admin).await.unwrap();
        assert_eq!(tasks_after_denial, 1);
        sqlx::query("UPDATE fvoci.project_members SET role = 'member' WHERE workspace_id = $1 AND project_id = $2 AND user_id = $3")
            .bind(ws_id).bind(f.project_id).bind(editor.user_id).execute(&admin).await.unwrap();
        let (status, replay) = session_call(addr, Method::POST, &path, &editor.session_token, Some(body)).await;
        assert_eq!(status, StatusCode::CREATED, "{replay}");
        assert_eq!(replay["taskId"], first_id);

        let concurrent = json!({"projectId": f.project_id, "requestId": Uuid::now_v7(), "task": {"title": "Concurrent target"}});
        let (a, b) = tokio::join!(
            session_call(addr, Method::POST, &path, &editor.session_token, Some(concurrent.clone())),
            session_call(addr, Method::POST, &path, &editor.session_token, Some(concurrent)),
        );
        assert_eq!(a.0, StatusCode::CREATED, "{}", a.1);
        assert_eq!(b.0, StatusCode::CREATED, "{}", b.1);
        assert_eq!(a.1["taskId"], b.1["taskId"]);
        let origins: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.task_origins WHERE workspace_id = $1 AND document_id = $2")
            .bind(ws_id).bind(wiki).fetch_one(&admin).await.unwrap();
        assert_eq!(origins, 2);
        let tasks: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.tasks WHERE workspace_id = $1 AND project_id = $2 AND title = 'Concurrent target'")
            .bind(ws_id).bind(f.project_id).fetch_one(&admin).await.unwrap();
        assert_eq!(tasks, 1);
        admin.close().await;
        run.finish().await.expect("cleanup");
    }).await;
}

#[tokio::test]
async fn document_origin_picker_filters_edit_archive_guest_and_pat_scope() {
    run_test(
        "document_origin_picker_filters_edit_archive_guest_and_pat_scope",
        async {
            let mut run = TestRun::new(TestDb::bootstrap().await);
            let (addr, f) = setup_task(&mut run, 5_000).await;
            let ws_id = f.owner.workspace_id;
            let s = &f.owner;
            let wiki = create_wiki(s, "피커 문서").await;
            let admin = admin_pool(&run.harness).await;
            let root: Uuid =
                sqlx::query_scalar("SELECT root_document_id FROM fvoci.projects WHERE id = $1")
                    .bind(f.project_id)
                    .fetch_one(&admin)
                    .await
                    .unwrap();
            let project_doc = fvoci_server::db::project_documents::create_project_document(
                &s.pool,
                ws_id,
                f.project_id,
                s.user_id,
                s.session_id,
                CreateDocumentInput {
                    parent_id: Some(root),
                    title: "프로젝트 출처",
                    icon: None,
                },
                None,
            )
            .await
            .unwrap()
            .unwrap()
            .id;
            let visible = create_named_project(addr, s, "VIS", "workspace").await;
            let archived = create_named_project(addr, s, "ARC", "workspace").await;
            sqlx::query("UPDATE fvoci.projects SET status = 'archived' WHERE id = $1")
                .bind(archived)
                .execute(&admin)
                .await
                .unwrap();

            let picker_path = document_api(ws_id, wiki, "/task-projects");
            let (status, picker) =
                session_call(addr, Method::GET, &picker_path, &s.session_token, None).await;
            assert_eq!(status, StatusCode::OK, "{picker}");
            let ids: Vec<String> = picker["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|item| item["id"].as_str().unwrap().to_string())
                .collect();
            assert!(ids.contains(&f.project_id.to_string()), "{picker}");
            assert!(ids.contains(&visible.to_string()), "{picker}");
            assert!(
                !ids.contains(&archived.to_string()),
                "archived omitted: {picker}"
            );
            assert_eq!(picker["canCreateProject"], true);
            assert_eq!(picker["suggestedId"].as_str(), Some(ids[0].as_str()));

            let (status, from_project) = session_call(
                addr,
                Method::GET,
                &document_api(ws_id, project_doc, "/task-projects"),
                &s.session_token,
                None,
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{from_project}");
            assert_eq!(from_project["suggestedId"], f.project_id.to_string());

            let viewer = create_user_session(&run.harness, ws_id, WorkspaceRole::Member).await;
            add_project_member(&run.harness, ws_id, f.project_id, viewer.user_id, "viewer").await;
            let (status, viewer_picker) =
                session_call(addr, Method::GET, &picker_path, &viewer.session_token, None).await;
            assert_eq!(status, StatusCode::OK, "{viewer_picker}");
            let viewer_ids: Vec<String> = viewer_picker["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|item| item["id"].as_str().unwrap().to_string())
                .collect();
            assert!(
                !viewer_ids.contains(&f.project_id.to_string()),
                "private viewer is not Edit: {viewer_picker}"
            );
            assert!(viewer_ids.contains(&visible.to_string()), "{viewer_picker}");
            assert_eq!(viewer_picker["canCreateProject"], true);

            let guest = create_user_session(&run.harness, ws_id, WorkspaceRole::Guest).await;
            let (status, _) =
                session_call(addr, Method::GET, &picker_path, &guest.session_token, None).await;
            assert_eq!(status, StatusCode::NOT_FOUND);
            grant_wiki_group_view(&run.harness, ws_id, wiki, guest.user_id).await;
            let (status, guest_picker) =
                session_call(addr, Method::GET, &picker_path, &guest.session_token, None).await;
            assert_eq!(status, StatusCode::OK, "{guest_picker}");
            assert_eq!(guest_picker["canCreateProject"], false);
            assert_eq!(guest_picker["items"].as_array().unwrap().len(), 0);

            let docs_only = insert_pat(&run.harness, ws_id, s.user_id, &["documents.read"]).await;
            let tasks_only = insert_pat(&run.harness, ws_id, s.user_id, &["tasks.read"]).await;
            let (status, pat_picker) = call(
                addr,
                Method::GET,
                &picker_path,
                Cred::Bearer(&docs_only),
                None,
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{pat_picker}");
            assert!(!pat_picker["items"].as_array().unwrap().is_empty());
            let (status, _) = call(
                addr,
                Method::GET,
                &picker_path,
                Cred::Bearer(&tasks_only),
                None,
            )
            .await;
            assert_eq!(status, StatusCode::NOT_FOUND);

            let (status, _) = session_call(
                addr,
                Method::GET,
                &document_api(ws_id, Uuid::now_v7(), "/task-projects"),
                &s.session_token,
                None,
            )
            .await;
            assert_eq!(status, StatusCode::NOT_FOUND);

            admin.close().await;
            run.finish().await.expect("cleanup");
        },
    )
    .await;
}

#[tokio::test]
async fn document_origin_list_pages_authorized_count_and_current_authz() {
    run_test("document_origin_list_pages_authorized_count_and_current_authz", async {
        let mut run = TestRun::new(TestDb::bootstrap().await);
        let (addr, f) = setup_task(&mut run, 5_000).await;
        let ws_id = f.owner.workspace_id;
        let s = &f.owner;
        let wiki = create_wiki(s, "목록 문서").await;
        let visible = create_named_project(addr, s, "VISL", "workspace").await;
        let extra = create_named_project(addr, s, "VIS2", "workspace").await;
        let hidden_task = create_origin_task(
            addr,
            &s.session_token,
            ws_id,
            wiki,
            f.project_id,
            "비공개 연결",
        )
        .await;
        let first_visible = create_origin_task(
            addr,
            &s.session_token,
            ws_id,
            wiki,
            visible,
            "공개 연결 1",
        )
        .await;
        let second_visible = create_origin_task(
            addr,
            &s.session_token,
            ws_id,
            wiki,
            extra,
            "공개 연결 2",
        )
        .await;

        let list_path = document_api(ws_id, wiki, "/task-origins");
        let (status, all) = session_call(addr, Method::GET, &list_path, &s.session_token, None).await;
        assert_eq!(status, StatusCode::OK, "{all}");
        assert_eq!(all["count"], 3);
        assert_eq!(all["items"].as_array().unwrap().len(), 3);
        assert!(all["nextCursor"].is_null());

        let (status, page) = session_call(
            addr,
            Method::GET,
            &format!("{list_path}?limit=1"),
            &s.session_token,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{page}");
        assert_eq!(page["count"], 3, "count is authorized total, not page length");
        assert_eq!(page["items"].as_array().unwrap().len(), 1);
        let cursor = page["nextCursor"].as_str().expect("next cursor");
        let (status, rest) = session_call(
            addr,
            Method::GET,
            &format!("{list_path}?limit=1&after={cursor}"),
            &s.session_token,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{rest}");
        assert_eq!(rest["count"], 3);
        assert_eq!(rest["items"].as_array().unwrap().len(), 1);
        assert_ne!(rest["items"][0]["taskId"], page["items"][0]["taskId"]);

        let member = create_user_session(&run.harness, ws_id, WorkspaceRole::Member).await;
        let (status, member_list) =
            session_call(addr, Method::GET, &list_path, &member.session_token, None).await;
        assert_eq!(status, StatusCode::OK, "{member_list}");
        assert_eq!(member_list["count"], 2, "private origin omitted from count");
        let member_ids: Vec<String> = member_list["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["taskId"].as_str().unwrap().to_string())
            .collect();
        assert!(!member_ids.contains(&hidden_task.to_string()), "{member_list}");
        assert!(member_ids.contains(&first_visible.to_string()), "{member_list}");
        assert!(member_ids.contains(&second_visible.to_string()), "{member_list}");

        let (status, _) = session_call(
            addr,
            Method::GET,
            &format!("{list_path}?limit=0"),
            &s.session_token,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, _) = session_call(
            addr,
            Method::GET,
            &format!("{list_path}?limit=101"),
            &s.session_token,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let docs_only = insert_pat(&run.harness, ws_id, s.user_id, &["documents.read"]).await;
        let tasks_only = insert_pat(&run.harness, ws_id, s.user_id, &["tasks.read"]).await;
        let both = insert_pat(
            &run.harness,
            ws_id,
            s.user_id,
            &["documents.read", "tasks.read"],
        )
        .await;
        let (status, _) = call(addr, Method::GET, &list_path, Cred::Bearer(&docs_only), None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = call(addr, Method::GET, &list_path, Cred::Bearer(&tasks_only), None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, pat_list) = call(addr, Method::GET, &list_path, Cred::Bearer(&both), None).await;
        assert_eq!(status, StatusCode::OK, "{pat_list}");
        assert_eq!(pat_list["count"], 3);

        let admin = admin_pool(&run.harness).await;
        let token_id: Uuid = sqlx::query_scalar(
            "SELECT id FROM fvoci.api_tokens WHERE user_id = $1 AND 'tasks.read' = ANY(scopes) AND 'documents.read' = ANY(scopes)",
        )
        .bind(s.user_id)
        .fetch_one(&admin)
        .await
        .unwrap();
        admin.close().await;
        fvoci_server::db::api_tokens::revoke_api_token(
            &s.pool,
            ws_id,
            s.user_id,
            s.session_id,
            token_id,
            None,
        )
        .await
        .unwrap()
        .unwrap();
        let (status, _) = call(addr, Method::GET, &list_path, Cred::Bearer(&both), None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        let (status, _) = session_call(
            addr,
            Method::POST,
            &document_api(ws_id, wiki, "/trash"),
            &s.session_token,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = session_call(addr, Method::GET, &list_path, &s.session_token, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = session_call(
            addr,
            Method::POST,
            &document_api(ws_id, wiki, "/restore"),
            &s.session_token,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, restored_doc) =
            session_call(addr, Method::GET, &list_path, &s.session_token, None).await;
        assert_eq!(status, StatusCode::OK, "{restored_doc}");
        assert_eq!(restored_doc["count"], 3);

        let (status, _) = session_call(
            addr,
            Method::POST,
            &task_path(s, first_visible, "/trash"),
            &s.session_token,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, after_trash) =
            session_call(addr, Method::GET, &list_path, &s.session_token, None).await;
        assert_eq!(status, StatusCode::OK, "{after_trash}");
        assert_eq!(after_trash["count"], 2);
        let (status, _) = session_call(
            addr,
            Method::POST,
            &task_path(s, first_visible, "/restore"),
            &s.session_token,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, after_restore) =
            session_call(addr, Method::GET, &list_path, &s.session_token, None).await;
        assert_eq!(status, StatusCode::OK, "{after_restore}");
        assert_eq!(after_restore["count"], 3);

        let admin = admin_pool(&run.harness).await;
        sqlx::query("UPDATE fvoci.projects SET visibility = 'private' WHERE id = $1")
            .bind(visible)
            .execute(&admin)
            .await
            .unwrap();
        admin.close().await;
        let (status, revoked) =
            session_call(addr, Method::GET, &list_path, &member.session_token, None).await;
        assert_eq!(status, StatusCode::OK, "{revoked}");
        assert_eq!(revoked["count"], 1, "revoked project View drops the origin");

        run.finish().await.expect("cleanup");
    })
    .await;
}
