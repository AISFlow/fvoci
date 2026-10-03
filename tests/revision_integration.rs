#![cfg(feature = "db-tests")]

#[allow(dead_code)]
mod support;

use std::time::Duration;

use chrono::{Duration as ChronoDuration, Utc};
use futures_util::{SinkExt, StreamExt};
use fvoci_server::auth::password::Keyring;
use fvoci_server::auth::token::new_token;
use fvoci_server::collab::config::CollabConfig;
use fvoci_server::collab::hub::CollabHub;
use fvoci_server::collab::revision::capture_revision_offline;
use fvoci_server::collab::room::CapturedRevision;
use fvoci_server::collab::room::{
    arm_append_revoke_barrier, arm_join_channel_admission_witness,
    arm_session_revision_persist_barrier, disarm_join_channel_admission_witness,
    disarm_session_revision_persist_barrier, session_revision_persist_barrier_armed,
    AuthenticatedConnection, CollabSession, RoomJoin,
};
use fvoci_server::collab::seed::SeedEngine;
use fvoci_server::collab::wire::{CollabKind, CollabRoomName};
use fvoci_server::config::RevisionSettings;
use fvoci_server::db::collab::{
    append_collab_update, claim_writer_and_load, resolve_collab_admission, AppendCollabInput,
    AppendCollabResult,
};
use fvoci_server::db::documents::CreateDocumentInput;
use fvoci_server::db::identity::revoke_session;
use fvoci_server::db::pool;
use fvoci_server::db::project_documents::create_project_document;
use fvoci_server::db::projects::{add_project_member, create_project, CreateProjectInput};
use fvoci_server::db::revisions::{load_durable_collab_for_system, RevisionTarget};
use fvoci_server::db::tasks::{create_task, CreateTaskInput};
use fvoci_server::db::workspace::{self, WorkspaceRole};
use fvoci_server::jobs::{
    run_revision_maintenance_batch, RevisionMaintenanceEngine, RevisionMaintenanceParams,
    RevisionMaintenanceResume, SCHEDULED_REVISION_TARGET_BATCH, WORKSPACE_SCAN_BATCH,
};
use fvoci_server::projects::ProjectMemberRole;
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use support::{
    auth_and_join, collab_app_state, complete_sync_handshake, connect_member, engine_fixture,
    setup_owner_session, setup_wiki_doc, setup_wiki_doc_batch, stateless_frame, sync_step1_frame,
    sync_update_frame, test_collab_config, wait_for_stateless_exact, wait_for_sync_applied,
    wait_for_sync_update, wait_for_ws_close_code, SessionFixture, TestRun, WikiDocFixture, PEPPER,
    PUBLIC_ORIGIN,
};
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const TEST_TIMEOUT: Duration = Duration::from_secs(45);

fn routing_key(workspace_id: Uuid, document_id: Uuid) -> String {
    CollabRoomName {
        workspace_id,
        kind: CollabKind::Document,
        resource_id: document_id,
    }
    .routing_key()
}

fn room_key(workspace_id: Uuid, document_id: Uuid) -> fvoci_server::collab::room::RoomKey {
    fvoci_server::collab::room::RoomKey(workspace_id, document_id, CollabKind::Document)
}

async fn hub_join_with_conn(
    hub: &CollabHub,
    workspace_id: Uuid,
    document_id: Uuid,
    session_id: Uuid,
    user_id: Uuid,
    client_id: u32,
    conn_id: Uuid,
) -> Result<fvoci_server::collab::room::ConnectionLease, fvoci_server::collab::room::JoinError> {
    let (events_tx, mut events_rx) = tokio::sync::mpsc::channel(8);
    tokio::spawn(async move { while events_rx.recv().await.is_some() {} });
    hub_join_with_events(
        hub,
        workspace_id,
        document_id,
        session_id,
        user_id,
        client_id,
        conn_id,
        events_tx,
    )
    .await
}

/// Hub join whose outbound events go to `events`; the caller decides whether they are drained.
#[allow(clippy::too_many_arguments)]
async fn hub_join_with_events(
    hub: &CollabHub,
    workspace_id: Uuid,
    document_id: Uuid,
    session_id: Uuid,
    user_id: Uuid,
    client_id: u32,
    conn_id: Uuid,
    events_tx: tokio::sync::mpsc::Sender<fvoci_server::collab::room::RoomClientEvent>,
) -> Result<fvoci_server::collab::room::ConnectionLease, fvoci_server::collab::room::JoinError> {
    let routing_key = routing_key(workspace_id, document_id);
    let join = RoomJoin {
        conn: AuthenticatedConnection {
            conn_id,
            session: CollabSession {
                session_id,
                user_id,
                given_name: "Owner".into(),
                family_name: None,
                locale: "en".into(),
            },
            client_id,
            read_only: false,
            routing_key,
        },
        events: events_tx,
        cancel: None,
    };
    hub.join_room(room_key(workspace_id, document_id), join)
        .await
}

async fn run_test<F>(name: &str, case: F)
where
    F: std::future::Future<Output = ()>,
{
    tokio::time::timeout(TEST_TIMEOUT, case)
        .await
        .unwrap_or_else(|_| panic!("{name} hung (>{TEST_TIMEOUT:?})"));
}

fn cookie(token: &str) -> String {
    format!("fvoci_session={token}")
}

async fn http_json(
    addr: std::net::SocketAddr,
    method: reqwest::Method,
    path: &str,
    token: &str,
    body: Option<Value>,
) -> (reqwest::StatusCode, Value) {
    let client = reqwest::Client::new();
    let mut req = client
        .request(method, format!("http://{addr}{path}"))
        .header("origin", PUBLIC_ORIGIN)
        .header("cookie", cookie(token));
    if let Some(body) = body {
        req = req.json(&body);
    }
    let response = req.send().await.expect("http");
    let status = response.status();
    let value = response.json().await.unwrap_or(Value::Null);
    (status, value)
}

async fn apply_and_persist(
    addr: std::net::SocketAddr,
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

async fn wait_room_empty(
    hub: &fvoci_server::collab::CollabHub,
    workspace_id: Uuid,
    document_id: Uuid,
) {
    let key = (workspace_id, document_id);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        if hub.room_member_count(key).await == 0 {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("room did not drain connections before idle evict");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn revision_read_context(
    pool: &PgPool,
    workspace_id: Uuid,
) -> sqlx::Transaction<'static, sqlx::Postgres> {
    let mut tx = fvoci_server::db::context::begin_read(pool).await.unwrap();
    fvoci_server::db::context::set_tenant(&mut tx, workspace_id)
        .await
        .unwrap();
    let witness: (String, bool, bool, bool, bool, bool, Uuid, String) = sqlx::query_as(
        "SELECT current_user::text, r.rolsuper, r.rolbypassrls, pg_get_userbyid(c.relowner) <> current_user, c.relforcerowsecurity, row_security_active(c.oid), public.app_tenant_id(), current_setting('transaction_read_only') FROM pg_roles r JOIN pg_class c ON c.oid = 'fvoci.revisions'::regclass WHERE r.rolname = current_user",
    ).fetch_one(&mut *tx).await.unwrap();
    assert!(!witness.0.is_empty());
    assert_eq!(
        (witness.1, witness.2, witness.3, witness.4, witness.5),
        (false, false, true, true, true)
    );
    assert_eq!(witness.6, workspace_id);
    assert_eq!(witness.7, "on");
    tx
}

async fn revision_fingerprint(
    pool: &PgPool,
    workspace_id: Uuid,
    revision_id: Uuid,
) -> (Vec<u8>, Value) {
    let mut tx = revision_read_context(pool, workspace_id).await;
    let result = sqlx::query_as(
        "SELECT y_snapshot, content_json FROM fvoci.revisions WHERE workspace_id = $1 AND id = $2",
    )
    .bind(workspace_id)
    .bind(revision_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    result
}

type RevisionHistoryRows = Vec<(Value, Vec<u8>)>;

async fn capture_history_contract(
    wiki: &WikiDocFixture,
) -> (RevisionHistoryRows, CapturedRevision, i64) {
    let mut tx = revision_read_context(&wiki.session.pool, wiki.session.workspace_id).await;
    let rows = sqlx::query_as("SELECT to_jsonb(r), r.y_snapshot FROM fvoci.revisions r WHERE workspace_id=$1 AND target_kind='document' AND target_id=$2 ORDER BY created_at,id")
        .bind(wiki.session.workspace_id).bind(wiki.document_id).fetch_all(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    let load = fvoci_server::db::collab::load_collab_readonly_kind(
        &wiki.session.pool,
        CollabKind::Document,
        wiki.session.workspace_id,
        wiki.session.user_id,
        wiki.session.session_id,
        wiki.document_id,
    )
    .await
    .unwrap()
    .unwrap();
    let captured = capture_revision_offline(
        fvoci_server::collab::config::require_collab_engine_for_tests(),
        collab_engine::Limits::for_tests(),
        load.snapshot,
        load.tail.iter().map(|row| row.payload.clone()).collect(),
    )
    .unwrap();
    (rows, captured, load.tail_seq)
}

async fn assert_restore_history_contract(
    wiki: &WikiDocFixture,
    before: &RevisionHistoryRows,
    base: &CapturedRevision,
    source_id: &str,
    restored_id: Option<&str>,
    actor: Uuid,
    request: &Value,
) -> RevisionHistoryRows {
    let mut tx = revision_read_context(&wiki.session.pool, wiki.session.workspace_id).await;
    let after: RevisionHistoryRows = sqlx::query_as("SELECT to_jsonb(r), r.y_snapshot FROM fvoci.revisions r WHERE workspace_id=$1 AND target_kind='document' AND target_id=$2 ORDER BY created_at,id")
        .bind(wiki.session.workspace_id).bind(wiki.document_id).fetch_all(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    for old in before {
        assert_eq!(after.iter().find(|row| row.0["id"] == old.0["id"]), Some(old),
            "every complete old row, content, IDs, source metadata and snapshot bytes remain immutable");
    }
    let manuals = after
        .iter()
        .filter(|row| row.0["reason"] == "manual")
        .collect::<Vec<_>>();
    assert_eq!(
        manuals.len(),
        1,
        "exact original manual count; automatic history is never promoted"
    );
    assert_eq!(manuals[0].0["id"], source_id);
    assert_eq!(manuals[0].0["created_by"], wiki.session.user_id.to_string());
    let restored = after
        .iter()
        .filter(|row| {
            row.0["reason"] == "restore" && !before.iter().any(|old| old.0["id"] == row.0["id"])
        })
        .collect::<Vec<_>>();
    assert_eq!(
        restored.len(),
        1,
        "exactly ONE new restore, independently of allowed automatic history"
    );
    let metadata = &restored[0].0;
    let source_at =
        chrono::DateTime::parse_from_rfc3339(manuals[0].0["created_at"].as_str().unwrap()).unwrap();
    let restored_at =
        chrono::DateTime::parse_from_rfc3339(metadata["created_at"].as_str().unwrap()).unwrap();
    assert!(
        restored_at >= source_at && restored_at <= Utc::now(),
        "new restore records an actual time after its preserved source"
    );
    if let Some(id) = restored_id {
        assert_eq!(metadata["id"], id);
    }
    assert_ne!(metadata["id"], source_id);
    assert_eq!(metadata["restored_from_id"], source_id);
    assert_eq!(metadata["created_by"], actor.to_string());
    assert_eq!(metadata["restore_correlation_id"], request["correlationId"]);
    let tail = request["expectedTailSeq"]
        .as_str()
        .unwrap()
        .parse::<i64>()
        .unwrap();
    assert_eq!(metadata["restore_base_tail_seq"], tail);
    assert_eq!(metadata["restore_committed_tail_seq"], tail + 1);
    assert_eq!(
        metadata["content_json"], manuals[0].0["content_json"],
        "restored known IDs/resources/content match the manual source"
    );
    for (row, bytes) in &after {
        if row["reason"] == "session" {
            let captured_at =
                chrono::DateTime::parse_from_rfc3339(row["created_at"].as_str().unwrap()).unwrap();
            assert!(
                captured_at >= source_at && captured_at <= Utc::now(),
                "automatic pre-restore capture retains actual system timestamp"
            );
            assert_eq!(
                row["created_by"],
                Value::Null,
                "automatic system session actor is NULL (#308)"
            );
            assert_eq!(row["workspace_id"], wiki.session.workspace_id.to_string());
            assert_eq!(row["target_kind"], "document");
            assert_eq!(row["target_id"], wiki.document_id.to_string());
            for field in [
                "restored_from_id",
                "restore_correlation_id",
                "restore_base_tail_seq",
                "restore_committed_tail_seq",
            ] {
                assert_eq!(
                    row[field],
                    Value::Null,
                    "automatic row cannot masquerade as restore provenance"
                );
            }
            assert_eq!(
                row["content_json"], base.content_json,
                "automatic row represents the actual pre-restore durable tail"
            );
            assert!(fvoci_server::collab::revision::revision_snapshots_equal_offline(
                fvoci_server::collab::config::require_collab_engine_for_tests(), collab_engine::Limits::for_tests(), bytes, &base.y_snapshot).unwrap(),
                "automatic snapshot semantically equals the independently captured pre-restore tail");
        } else {
            assert!(
                row["reason"] == "manual" || row["reason"] == "restore",
                "no unexpected history classification"
            );
        }
    }
    after
}

type RevisionPeerSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn receive_exact_peer_update(ws: &mut RevisionPeerSocket, key: &str, payload: &[u8]) {
    let expected = fvoci_server::collab::wire::decode(&sync_update_frame(key, payload)).unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(
            !remaining.is_zero(),
            "peer did not receive exact committed update"
        );
        let frame = tokio::time::timeout(remaining, ws.next())
            .await
            .expect("peer update deadline")
            .expect("peer remains connected")
            .expect("peer frame");
        if let Message::Binary(bytes) = frame {
            let decoded = fvoci_server::collab::wire::decode(&bytes).unwrap();
            if matches!(
                &decoded,
                fvoci_server::collab::wire::WireFrame::Document {
                    message: fvoci_server::collab::wire::DocumentMessage::Sync(
                        fvoci_server::collab::wire::SyncMessage {
                            step: fvoci_server::collab::wire::SyncStep::Update,
                            ..
                        }
                    ),
                    ..
                }
            ) {
                assert_eq!(
                    decoded, expected,
                    "actual routed bytes must equal this durable update, not just any Sync Update"
                );
                return;
            }
        } else if matches!(frame, Message::Close(_)) {
            panic!("peer closed before committed update");
        }
    }
}

fn revision_peer_doc(
    load: &fvoci_server::db::collab::CollabLoadState,
) -> collab_engine::process::EngineSession {
    let mut peer =
        collab_engine::process::EngineSession::spawn(collab_engine::process::SpawnRequest {
            engine_bin: fvoci_server::collab::config::require_collab_engine_for_tests(),
            limits: collab_engine::Limits::for_tests(),
            slot_kind: collab_engine::process::ChildSlotKind::Primary,
            slot_wait: None,
            test_hang_ms: None,
            test_exit_after_read: None,
            test_close_stdout_hang_ms: None,
            test_exit_after_write: None,
        })
        .expect("independent initialized peer Doc");
    assert!(peer
        .call(&collab_engine::protocol::Request::Load {
            snapshot_b64: Some(load.snapshot.clone()),
            tail_b64: load.tail.iter().map(|row| row.payload.clone()).collect(),
            encoding: 1,
        })
        .outcome
        .is_applied_ok());
    peer
}

fn revision_peer_projection(peer: &mut collab_engine::process::EngineSession) -> Value {
    match peer
        .call(&collab_engine::protocol::Request::Project { encoding: 1 })
        .outcome
    {
        collab_engine::outcome::EngineStatus::Ok {
            content_json: Some(body),
            ..
        } => body,
        other => panic!("peer projection failed: {other:?}"),
    }
}

fn revision_peer_update(
    peer: &mut collab_engine::process::EngineSession,
    request: collab_engine::protocol::Request,
) -> Vec<u8> {
    match peer.call(&request).outcome {
        collab_engine::outcome::EngineStatus::Ok {
            update_b64: Some(bytes),
            ..
        } => collab_engine::b64::decode(&bytes).unwrap(),
        other => panic!("peer edit failed: {other:?}"),
    }
}

async fn count_updates(pool: &PgPool, workspace_id: Uuid, document_id: Uuid) -> i64 {
    let mut tx = revision_read_context(pool, workspace_id).await;
    let count = sqlx::query_scalar(
        "SELECT count(*) FROM fvoci.document_collab_updates WHERE workspace_id = $1 AND document_id = $2",
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    count
}

async fn create_member_session(
    harness: &support::TestDb,
    workspace_id: Uuid,
    email: &str,
) -> SessionFixture {
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
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
    .bind(email)
    .bind(&hash)
    .bind("Member")
    .execute(&admin)
    .await
    .unwrap();
    admin.close().await;
    let pool = pool::connect_app(&harness.app_url).await.unwrap();
    workspace::add_membership_for_test(&pool, workspace_id, user_id, WorkspaceRole::Member)
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

fn revision_path(wiki: &WikiDocFixture, extra: &str) -> String {
    format!(
        "/api/v1/workspaces/{}/documents/{}/revisions{extra}",
        wiki.session.workspace_id, wiki.document_id
    )
}

/// Obtain a coherent server preview before arming any existing append barrier.
async fn preview_restore_body(
    addr: std::net::SocketAddr,
    path: &str,
    token: &str,
) -> serde_json::Value {
    let (status, preview) = http_json(
        addr,
        reqwest::Method::GET,
        &format!("{path}-preview"),
        token,
        None,
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::OK, "{preview}");
    assert!(
        preview["currentTailSeq"].is_string(),
        "opaque committed sequence {preview}"
    );
    serde_json::json!({"correlationId": Uuid::now_v7(), "expectedTailSeq": preview["currentTailSeq"]})
}

#[tokio::test]
async fn create_list_restore_with_live_room() {
    run_test("create_list_restore_with_live_room", async {
        let mut run = TestRun::new(support::TestDb::bootstrap().await);
        let wiki = setup_wiki_doc(&run.harness).await;
        let (state, hub) =
            collab_app_state(&run.harness.app_url, test_collab_config(4, 60_000)).await;
        let addr = run.spawn_router_state(state, hub).await;
        let key = routing_key(wiki.session.workspace_id, wiki.document_id);
        apply_and_persist(
            addr,
            &wiki.session.session_token,
            &key,
            1,
            &engine_fixture("structured.v1"),
        )
        .await;

        let (status, created) = http_json(
            addr,
            reqwest::Method::POST,
            &revision_path(&wiki, ""),
            &wiki.session.session_token,
            None,
        )
        .await;
        assert_eq!(status, reqwest::StatusCode::CREATED, "{created}");
        let revision_id = created["id"].as_str().expect("id").to_string();

        let (status, list) = http_json(
            addr,
            reqwest::Method::GET,
            &format!("{}?limit=20", revision_path(&wiki, "")),
            &wiki.session.session_token,
            None,
        )
        .await;
        assert_eq!(status, reqwest::StatusCode::OK, "{list}");
        assert_eq!(list["items"][0]["id"], revision_id);

        apply_and_persist(
            addr,
            &wiki.session.session_token,
            &key,
            2,
            &engine_fixture("followup_edit.v1"),
        )
        .await;

        let restore_path = revision_path(&wiki, &format!("/{revision_id}/restore"));
        let (_, source_before) = http_json(addr, reqwest::Method::GET,
            &revision_path(&wiki, &format!("/{revision_id}")), &wiki.session.session_token, None).await;
        let source_fingerprint = revision_fingerprint(&wiki.session.pool, wiki.session.workspace_id, Uuid::parse_str(&revision_id).unwrap()).await;
        let (status, other_document) = http_json(addr, reqwest::Method::POST,
            &format!("/api/v1/workspaces/{}/documents", wiki.session.workspace_id),
            &wiki.session.session_token, Some(json!({"parentId": null, "title": "other restore target"}))).await;
        assert_eq!(status, reqwest::StatusCode::CREATED, "{other_document}");
        let other_id = other_document["id"].as_str().unwrap();
        let (status, _) = http_json(addr, reqwest::Method::GET,
            &format!("/api/v1/workspaces/{}/documents/{other_id}/revisions/{revision_id}/restore-preview", wiki.session.workspace_id),
            &wiki.session.session_token, None).await;
        assert_eq!(status, reqwest::StatusCode::NOT_FOUND, "same workspace wrong document cannot preview source");
        let (status, _) = http_json(addr, reqwest::Method::POST,
            &format!("/api/v1/workspaces/{}/documents/{other_id}/revisions/{revision_id}/restore", wiki.session.workspace_id),
            &wiki.session.session_token, Some(json!({"correlationId": Uuid::now_v7(), "expectedTailSeq": "0"}))).await;
        assert_eq!(status, reqwest::StatusCode::NOT_FOUND, "wrong document cannot restore source");
        let (status, _) = http_json(addr, reqwest::Method::GET,
            &format!("/api/v1/workspaces/{}/tasks/{}/revisions/{revision_id}/restore-preview", wiki.session.workspace_id, wiki.document_id),
            &wiki.session.session_token, None).await;
        assert_eq!(status, reqwest::StatusCode::NOT_FOUND, "task/document namespaces remain distinct");
        let (status, _) = http_json(addr, reqwest::Method::GET,
            &format!("/api/v1/workspaces/{}/documents/{}/revisions/{revision_id}/restore-preview", Uuid::now_v7(), wiki.document_id),
            &wiki.session.session_token, None).await;
        assert_eq!(status, reqwest::StatusCode::NOT_FOUND, "foreign workspace cannot preview source");
        let before_updates = count_updates(&wiki.session.pool, wiki.session.workspace_id, wiki.document_id).await;
        let (missing_version_status, _) = http_json(addr, reqwest::Method::POST, &restore_path,
            &wiki.session.session_token, Some(json!({"correlationId": Uuid::now_v7()}))).await;
        assert_eq!(missing_version_status, reqwest::StatusCode::BAD_REQUEST, "restore cannot bypass preview version");
        let (wrong_source_status, _) = http_json(addr, reqwest::Method::GET,
            &revision_path(&wiki, &format!("/{}/restore-preview", Uuid::now_v7())),
            &wiki.session.session_token, None).await;
        assert_eq!(wrong_source_status, reqwest::StatusCode::NOT_FOUND);
        let (history_before, history_base, history_tail) = capture_history_contract(&wiki).await;
        let restore_body = preview_restore_body(addr, &revision_path(&wiki, &format!("/{revision_id}/restore")), &wiki.session.session_token).await;
        assert_eq!(restore_body["expectedTailSeq"], history_tail.to_string());
        assert!(history_base.content_json.to_string().contains("후속편집한글"));

        let (status, restored) = http_json(
            addr,
            reqwest::Method::POST,
            &revision_path(&wiki, &format!("/{revision_id}/restore")),
            &wiki.session.session_token,
            Some(restore_body.clone()),
        )
        .await;
        assert_eq!(status, reqwest::StatusCode::OK, "{restored}");
        assert_eq!(restored["restored"], true);
        let new_revision_id = restored["revisionId"].as_str().expect("new revision ID");
        assert_ne!(new_revision_id, revision_id, "restore records new history");
        let after_updates = count_updates(&wiki.session.pool, wiki.session.workspace_id, wiki.document_id).await;
        assert_eq!(after_updates, before_updates + 1);
        let history_first_restore = assert_restore_history_contract(&wiki, &history_before, &history_base, &revision_id,
            Some(new_revision_id), wiki.session.user_id, &restore_body).await;
        let (status, replay) = http_json(addr, reqwest::Method::POST, &restore_path,
            &wiki.session.session_token, Some(restore_body.clone())).await;
        assert_eq!(status, reqwest::StatusCode::OK, "{replay}");
        assert_eq!(replay["revisionId"], new_revision_id, "response-loss retry recovers exact committed restore");
        assert_eq!(assert_restore_history_contract(&wiki, &history_before, &history_base, &revision_id,
            Some(new_revision_id), wiki.session.user_id, &restore_body).await, history_first_restore, "same correlation retry adds ZERO rows or changes");

        assert_eq!(count_updates(&wiki.session.pool, wiki.session.workspace_id, wiki.document_id).await, after_updates);
        let mut changed_replay = restore_body.clone();
        changed_replay["expectedTailSeq"] = json!("0");
        let (status, _) = http_json(addr, reqwest::Method::POST, &restore_path,
            &wiki.session.session_token, Some(changed_replay)).await;
        assert_eq!(status, reqwest::StatusCode::CONFLICT, "correlation cannot authorize different input");
        let mut stale_preview = restore_body.clone();
        stale_preview["correlationId"] = json!(Uuid::now_v7());
        let (status, _) = http_json(addr, reqwest::Method::POST, &restore_path,
            &wiki.session.session_token, Some(stale_preview)).await;
        assert_eq!(status, reqwest::StatusCode::CONFLICT, "a committed peer/version change invalidates the preview");
        assert_eq!(count_updates(&wiki.session.pool, wiki.session.workspace_id, wiki.document_id).await, after_updates,
            "conflicts must not append");
        let (status, new_revision) = http_json(addr, reqwest::Method::GET,
            &revision_path(&wiki, &format!("/{new_revision_id}")), &wiki.session.session_token, None).await;
        assert_eq!(status, reqwest::StatusCode::OK, "{new_revision}");
        assert_eq!(new_revision["reason"], "restore");
        assert_eq!(new_revision["restoredFromId"], revision_id);
        assert_eq!(new_revision["createdBy"], wiki.session.user_id.to_string());
        let created_at = chrono::DateTime::parse_from_rfc3339(new_revision["createdAt"].as_str().unwrap()).unwrap();
        assert!(created_at <= Utc::now());
        assert!(created_at > Utc::now() - ChronoDuration::minutes(1));
        assert_eq!(new_revision["contentJson"], source_before["contentJson"], "content IDs/resources survive restore");
        let retained = revision_fingerprint(&wiki.session.pool, wiki.session.workspace_id, Uuid::parse_str(&revision_id).unwrap()).await;
        assert_eq!(retained, source_fingerprint, "old source history bytes are immutable");
        let (status, history) = http_json(addr, reqwest::Method::GET,
            &revision_path(&wiki, ""), &wiki.session.session_token, None).await;
        assert_eq!(status, reqwest::StatusCode::OK);
        assert_eq!(history["items"].as_array().unwrap().len(), history_first_restore.len(), "API and full restricted-role history agree");


        let body = support::get_document_body(
            addr,
            &wiki.session.session_token,
            wiki.session.workspace_id,
            wiki.document_id,
        )
        .await;
        let text = body["contentJson"].to_string();
        assert!(
            text.contains("안녕 본문"),
            "restore must return the captured structured body {text}"
        );
        assert!(
            !text.contains("후속편집한글"),
            "restore must not keep the follow-up edit {text}"
        );
        run.finish().await.expect("cleanup");
    })
    .await;
}

#[tokio::test]
async fn create_and_restore_without_live_room() {
    run_test("create_and_restore_without_live_room", async {
        let mut run = TestRun::new(support::TestDb::bootstrap().await);
        let wiki = setup_wiki_doc(&run.harness).await;
        let (state, hub) =
            collab_app_state(&run.harness.app_url, test_collab_config(4, 60_000)).await;
        let addr = run.spawn_router_state(state, hub.clone()).await;
        let key = routing_key(wiki.session.workspace_id, wiki.document_id);
        apply_and_persist(
            addr,
            &wiki.session.session_token,
            &key,
            1,
            &engine_fixture("structured.v1"),
        )
        .await;
        wait_room_empty(&hub, wiki.session.workspace_id, wiki.document_id).await;
        hub.force_room_idle_eligible((wiki.session.workspace_id, wiki.document_id))
            .await;
        assert!(
            hub.execute_idle_evict_if_eligible((wiki.session.workspace_id, wiki.document_id))
                .await
        );

        let (status, created) = http_json(
            addr,
            reqwest::Method::POST,
            &revision_path(&wiki, ""),
            &wiki.session.session_token,
            None,
        )
        .await;
        assert_eq!(status, reqwest::StatusCode::CREATED, "{created}");
        let revision_id = created["id"].as_str().unwrap().to_string();

        apply_and_persist(
            addr,
            &wiki.session.session_token,
            &key,
            3,
            &engine_fixture("followup_edit.v1"),
        )
        .await;
        wait_room_empty(&hub, wiki.session.workspace_id, wiki.document_id).await;
        hub.force_room_idle_eligible((wiki.session.workspace_id, wiki.document_id))
            .await;
        let _ = hub
            .execute_idle_evict_if_eligible((wiki.session.workspace_id, wiki.document_id))
            .await;

        let restore_body = preview_restore_body(
            addr,
            &revision_path(&wiki, &format!("/{revision_id}/restore")),
            &wiki.session.session_token,
        )
        .await;
        let (status, restored) = http_json(
            addr,
            reqwest::Method::POST,
            &revision_path(&wiki, &format!("/{revision_id}/restore")),
            &wiki.session.session_token,
            Some(restore_body),
        )
        .await;
        assert_eq!(status, reqwest::StatusCode::OK, "{restored}");
        run.finish().await.expect("cleanup");
    })
    .await;
}

#[tokio::test]
async fn create_revision_in_unloaded_live_room_uses_durable_state() {
    run_test(
        "create_revision_in_unloaded_live_room_uses_durable_state",
        async {
            let mut run = TestRun::new(support::TestDb::bootstrap().await);
            let wiki = setup_wiki_doc(&run.harness).await;
            let (state, hub) =
                collab_app_state(&run.harness.app_url, test_collab_config(4, 60_000)).await;
            let addr = run.spawn_router_state(state, hub.clone()).await;
            let key = routing_key(wiki.session.workspace_id, wiki.document_id);
            apply_and_persist(
                addr,
                &wiki.session.session_token,
                &key,
                1,
                &engine_fixture("structured.v1"),
            )
            .await;
            let persisted = support::get_document_body(
                addr,
                &wiki.session.session_token,
                wiki.session.workspace_id,
                wiki.document_id,
            )
            .await;
            wait_room_empty(&hub, wiki.session.workspace_id, wiki.document_id).await;
            hub.force_room_idle_eligible((wiki.session.workspace_id, wiki.document_id))
                .await;
            assert!(
                hub.execute_idle_evict_if_eligible((wiki.session.workspace_id, wiki.document_id))
                    .await
            );
            hub.ensure_live_room((wiki.session.workspace_id, wiki.document_id))
                .await
                .expect("unloaded live room");

            let (status, created) = http_json(
                addr,
                reqwest::Method::POST,
                &revision_path(&wiki, ""),
                &wiki.session.session_token,
                None,
            )
            .await;
            assert_eq!(status, reqwest::StatusCode::CREATED, "{created}");
            let revision_id = created["id"].as_str().expect("id");
            let (status, detail) = http_json(
                addr,
                reqwest::Method::GET,
                &revision_path(&wiki, &format!("/{revision_id}")),
                &wiki.session.session_token,
                None,
            )
            .await;
            assert_eq!(status, reqwest::StatusCode::OK, "{detail}");
            assert_ne!(
                detail["contentJson"],
                serde_json::json!({"type":"doc","content":[]}),
                "unloaded live capture must not snapshot an empty engine"
            );
            assert_eq!(
                detail["contentJson"], persisted["contentJson"],
                "unloaded live capture must use durable document state"
            );
            run.finish().await.expect("cleanup");
        },
    )
    .await;
}

#[tokio::test]
async fn restore_concurrent_peer_update_converges() {
    run_test("restore_concurrent_peer_update_converges", async {
        let mut run = TestRun::new(support::TestDb::bootstrap().await);
        let wiki = setup_wiki_doc(&run.harness).await;
        let (state, hub) =
            collab_app_state(&run.harness.app_url, test_collab_config(4, 60_000)).await;
        let addr = run.spawn_router_state(state, hub).await;
        let key = routing_key(wiki.session.workspace_id, wiki.document_id);
        apply_and_persist(
            addr,
            &wiki.session.session_token,
            &key,
            1,
            &engine_fixture("structured.v1"),
        )
        .await;
        let (status, created) = http_json(
            addr,
            reqwest::Method::POST,
            &revision_path(&wiki, ""),
            &wiki.session.session_token,
            None,
        )
        .await;
        assert_eq!(status, reqwest::StatusCode::CREATED, "{created}");
        let revision_id = created["id"].as_str().unwrap().to_string();
        apply_and_persist(
            addr,
            &wiki.session.session_token,
            &key,
            2,
            &engine_fixture("followup_edit.v1"),
        )
        .await;

        let mut observer = connect_member(addr, &wiki.session.session_token).await;
        auth_and_join(&mut observer, &key, 8).await;
        complete_sync_handshake(&mut observer, &key).await;
        let mut editor = connect_member(addr, &wiki.session.session_token).await;
        auth_and_join(&mut editor, &key, 9).await;
        complete_sync_handshake(&mut editor, &key).await;

        let restore_body = preview_restore_body(
            addr,
            &revision_path(&wiki, &format!("/{revision_id}/restore")),
            &wiki.session.session_token,
        )
        .await;
        let (reached, proceed) = arm_append_revoke_barrier(wiki.document_id).await;
        let restore = tokio::spawn({
            let token = wiki.session.session_token.clone();
            let path = revision_path(&wiki, &format!("/{revision_id}/restore"));
            async move {
                http_json(
                    addr,
                    reqwest::Method::POST,
                    &path,
                    &token,
                    Some(restore_body),
                )
                .await
            }
        });
        reached.await.expect("restore reached persist barrier");
        editor
            .send(Message::Binary(
                sync_update_frame(&key, &engine_fixture("korean_emoji_base.v1")).into(),
            ))
            .await
            .unwrap();
        let _ = proceed.send(());
        let (status, body) = restore.await.unwrap();
        assert_eq!(status, reqwest::StatusCode::OK, "{body}");
        assert!(
            wait_for_sync_applied(&mut editor, Duration::from_secs(8)).await,
            "concurrent peer edit must apply after restore"
        );
        assert!(
            wait_for_sync_update(&mut observer, Duration::from_secs(8)).await,
            "observer must receive the restore update"
        );
        assert!(
            wait_for_sync_update(&mut observer, Duration::from_secs(8)).await,
            "observer must receive the concurrent edit"
        );

        let request_id = Uuid::now_v7();
        editor
            .send(Message::Binary(
                stateless_frame(&key, &format!("persist:{request_id}")).into(),
            ))
            .await
            .unwrap();
        assert!(
            wait_for_stateless_exact(
                &mut editor,
                &format!("persisted:{request_id}"),
                Duration::from_secs(8)
            )
            .await,
            "persist ack after restore+concurrent edit"
        );

        let live = support::get_document_body(
            addr,
            &wiki.session.session_token,
            wiki.session.workspace_id,
            wiki.document_id,
        )
        .await;
        let text = live["contentJson"].to_string();
        assert!(
            text.contains("안녕 본문"),
            "restored structured content must remain {text}"
        );
        assert!(
            text.contains("가나다"),
            "concurrent peer edit must not be lost {text}"
        );
        assert!(
            !text.contains("후속편집한글"),
            "follow-up edit must be replaced by restore {text}"
        );

        run.shutdown_last_server().await.expect("stop");
        let (state, hub) =
            collab_app_state(&run.harness.app_url, test_collab_config(4, 60_000)).await;
        let addr = run.spawn_router_state(state, hub).await;
        let durable = support::get_document_body(
            addr,
            &wiki.session.session_token,
            wiki.session.workspace_id,
            wiki.document_id,
        )
        .await;
        assert_eq!(
            durable["contentJson"], live["contentJson"],
            "durable state after reload must keep restored content plus concurrent edit"
        );
        let mut fresh = connect_member(addr, &wiki.session.session_token).await;
        auth_and_join(&mut fresh, &key, 11).await;
        complete_sync_handshake(&mut fresh, &key).await;
        run.finish().await.expect("cleanup");
    })
    .await;
}

#[tokio::test]
async fn restore_committed_ambiguous_receipt_converges() {
    run_test("restore_committed_ambiguous_receipt_converges", async {
        let mut run = TestRun::new(support::TestDb::bootstrap().await);
        let wiki = setup_wiki_doc(&run.harness).await;
        let (state, hub) =
            collab_app_state(&run.harness.app_url, test_collab_config(4, 60_000)).await;
        let addr = run.spawn_router_state(state, hub).await;
        let key = routing_key(wiki.session.workspace_id, wiki.document_id);
        apply_and_persist(
            addr,
            &wiki.session.session_token,
            &key,
            1,
            &engine_fixture("structured.v1"),
        )
        .await;
        let (status, created) = http_json(
            addr,
            reqwest::Method::POST,
            &revision_path(&wiki, ""),
            &wiki.session.session_token,
            None,
        )
        .await;
        assert_eq!(status, reqwest::StatusCode::CREATED, "{created}");
        let revision_id = created["id"].as_str().unwrap().to_string();
        apply_and_persist(
            addr,
            &wiki.session.session_token,
            &key,
            2,
            &engine_fixture("followup_edit.v1"),
        )
        .await;

        let mut observer = connect_member(addr, &wiki.session.session_token).await;
        auth_and_join(&mut observer, &key, 8).await;
        complete_sync_handshake(&mut observer, &key).await;
        let mut editor = connect_member(addr, &wiki.session.session_token).await;
        auth_and_join(&mut editor, &key, 9).await;
        complete_sync_handshake(&mut editor, &key).await;

        let peer_initial = fvoci_server::db::collab::load_collab_readonly_kind(
            &wiki.session.pool,
            CollabKind::Document,
            wiki.session.workspace_id,
            wiki.session.user_id,
            wiki.session.session_id,
            wiki.document_id,
        )
        .await
        .unwrap()
        .unwrap();
        let (history_before, history_base, history_tail) = capture_history_contract(&wiki).await;
        let restore_body = preview_restore_body(
            addr,
            &revision_path(&wiki, &format!("/{revision_id}/restore")),
            &wiki.session.session_token,
        )
        .await;
        assert_eq!(restore_body["expectedTailSeq"], history_tail.to_string());
        assert!(history_base
            .content_json
            .to_string()
            .contains("후속편집한글"));

        let (reached, proceed) = fvoci_server::db::collab::arm_restore_committed_ambiguity(
            wiki.session.workspace_id,
            wiki.document_id,
        )
        .await;
        let restore = tokio::spawn({
            let token = wiki.session.session_token.clone();
            let path = revision_path(&wiki, &format!("/{revision_id}/restore"));
            let restore_body = restore_body.clone();
            async move {
                http_json(
                    addr,
                    reqwest::Method::POST,
                    &path,
                    &token,
                    Some(restore_body.clone()),
                )
                .await
            }
        });
        reached.await.expect("restore reached persist barrier");
        let _ = proceed.send(());
        let (status, body) = restore.await.unwrap();
        assert_eq!(status, reqwest::StatusCode::OK, "{body}");
        let durable = fvoci_server::db::collab::load_collab_readonly_kind(
            &wiki.session.pool,
            CollabKind::Document,
            wiki.session.workspace_id,
            wiki.session.user_id,
            wiki.session.session_id,
            wiki.document_id,
        )
        .await
        .unwrap()
        .unwrap();
        let restored_update = &durable
            .tail
            .iter()
            .find(|row| row.seq == peer_initial.tail_seq + 1)
            .expect("durable restore update")
            .payload;
        receive_exact_peer_update(&mut observer, &key, restored_update).await;
        receive_exact_peer_update(&mut editor, &key, restored_update).await;
        // Each independent peer starts from its pre-restore canonical state,
        // then applies the exact bytes verified on that peer's actual socket.
        let mut observer_doc = revision_peer_doc(&peer_initial);
        let mut editor_doc = revision_peer_doc(&peer_initial);
        for doc in [&mut observer_doc, &mut editor_doc] {
            assert!(doc
                .call(&collab_engine::protocol::Request::Apply {
                    update_b64: restored_update.clone(),
                    encoding: 1
                })
                .outcome
                .is_applied_ok());
            let body = revision_peer_projection(doc);
            assert_eq!(body["content"][0]["attrs"]["id"], "p-alpha-001");
            assert_eq!(
                body["content"][0]["content"][1]["marks"][0]["attrs"]["href"],
                "https://example.invalid/wiki/안녕"
            );
            assert_eq!(body["content"][0]["content"][3]["attrs"]["id"], "user-42");
            assert_eq!(body["content"][1]["attrs"]["id"], "tbl-001");
            assert!(body.to_string().contains("한글셀"));
            assert!(
                body.to_string().contains("안녕 본문"),
                "peer sees literal restored meaning: {body}"
            );
            assert!(
                !body.to_string().contains("후속편집한글"),
                "peer cannot retain replaced followup: {body}"
            );
        }
        let history_first_restore = assert_restore_history_contract(
            &wiki,
            &history_before,
            &history_base,
            &revision_id,
            body["revisionId"].as_str(),
            wiki.session.user_id,
            &restore_body,
        )
        .await;
        let (status, replay) = http_json(
            addr,
            reqwest::Method::POST,
            &revision_path(&wiki, &format!("/{revision_id}/restore")),
            &wiki.session.session_token,
            Some(restore_body.clone()),
        )
        .await;
        assert_eq!(status, reqwest::StatusCode::OK, "{replay}");
        assert_eq!(replay["revisionId"], body["revisionId"]);
        let (_, history) = http_json(
            addr,
            reqwest::Method::GET,
            &revision_path(&wiki, ""),
            &wiki.session.session_token,
            None,
        )
        .await;
        assert_eq!(
            history["items"].as_array().unwrap().len(),
            history_first_restore.len()
        );
        assert_eq!(
            assert_restore_history_contract(
                &wiki,
                &history_before,
                &history_base,
                &revision_id,
                body["revisionId"].as_str(),
                wiki.session.user_id,
                &restore_body
            )
            .await,
            history_first_restore,
            "same correlation retry adds ZERO rows or changes"
        );
        let mut edited = revision_peer_projection(&mut editor_doc);
        let text = edited["content"][0]["content"][0]["text"]
            .as_str()
            .expect("restored paragraph")
            .to_owned();
        assert_eq!(text, "안녕 본문 ");
        edited["content"][0]["content"][0]["text"] = json!(format!("{text}복원한 피어의 재편집 "));
        let seed = revision_peer_update(
            &mut editor_doc,
            collab_engine::protocol::Request::SeedFromTiptap {
                content_json: edited.to_string(),
                encoding: 1,
            },
        );
        let edit_update = revision_peer_update(
            &mut editor_doc,
            collab_engine::protocol::Request::ReplaceFromUpdate {
                update_b64: seed,
                encoding: 1,
            },
        );
        assert!(editor_doc
            .call(&collab_engine::protocol::Request::Apply {
                update_b64: edit_update.clone(),
                encoding: 1
            })
            .outcome
            .is_applied_ok());
        editor
            .send(Message::Binary(
                sync_update_frame(&key, &edit_update).into(),
            ))
            .await
            .unwrap();
        assert!(
            wait_for_sync_applied(&mut editor, Duration::from_secs(8)).await,
            "restored-peer-derived edit applies"
        );
        receive_exact_peer_update(&mut observer, &key, &edit_update).await;
        assert!(observer_doc
            .call(&collab_engine::protocol::Request::Apply {
                update_b64: edit_update,
                encoding: 1
            })
            .outcome
            .is_applied_ok());
        let editor_body = revision_peer_projection(&mut editor_doc);
        let observer_body = revision_peer_projection(&mut observer_doc);
        assert_eq!(
            observer_body, editor_body,
            "both initialized peer Docs converge after derived edit"
        );
        assert_eq!(
            editor_body["content"][0]["content"][0]["text"],
            "안녕 본문 복원한 피어의 재편집 "
        );
        drop(observer_doc);
        drop(editor_doc);
        let request_id = Uuid::now_v7();
        editor
            .send(Message::Binary(
                stateless_frame(&key, &format!("persist:{request_id}")).into(),
            ))
            .await
            .unwrap();
        assert!(
            wait_for_stateless_exact(
                &mut editor,
                &format!("persisted:{request_id}"),
                Duration::from_secs(8)
            )
            .await,
            "persist ack after restore+concurrent edit"
        );

        let live = support::get_document_body(
            addr,
            &wiki.session.session_token,
            wiki.session.workspace_id,
            wiki.document_id,
        )
        .await;
        let text = live["contentJson"].to_string();
        assert!(
            text.contains("안녕 본문"),
            "restored structured content must remain {text}"
        );
        assert!(
            text.contains("복원한 피어의 재편집"),
            "restored-peer-derived reedit must not be lost {text}"
        );
        assert!(
            !text.contains("후속편집한글"),
            "follow-up edit must be replaced by restore {text}"
        );

        assert_eq!(
            live["contentJson"], editor_body,
            "server durable projection equals both actual peer Docs"
        );

        run.shutdown_last_server().await.expect("stop");
        let (state, hub) =
            collab_app_state(&run.harness.app_url, test_collab_config(4, 60_000)).await;
        let addr = run.spawn_router_state(state, hub).await;
        let durable = support::get_document_body(
            addr,
            &wiki.session.session_token,
            wiki.session.workspace_id,
            wiki.document_id,
        )
        .await;
        assert_eq!(
            durable["contentJson"], live["contentJson"],
            "durable state after reload must keep restored content plus concurrent edit"
        );
        let mut fresh = connect_member(addr, &wiki.session.session_token).await;
        auth_and_join(&mut fresh, &key, 11).await;
        complete_sync_handshake(&mut fresh, &key).await;
        run.finish().await.expect("cleanup");
    })
    .await;
}

#[tokio::test]
async fn restore_rejects_when_permission_revoked_before_apply() {
    run_test(
        "restore_rejects_when_permission_revoked_before_apply",
        async {
            let mut run = TestRun::new(support::TestDb::bootstrap().await);
            let wiki = setup_wiki_doc(&run.harness).await;
            let member = create_member_session(
                &run.harness,
                wiki.session.workspace_id,
                &format!("rev-member-{}@example.com", Uuid::now_v7().simple()),
            )
            .await;
            let (state, hub) =
                collab_app_state(&run.harness.app_url, test_collab_config(4, 60_000)).await;
            let addr = run.spawn_router_state(state, hub).await;
            let key = routing_key(wiki.session.workspace_id, wiki.document_id);
            apply_and_persist(
                addr,
                &wiki.session.session_token,
                &key,
                1,
                &engine_fixture("structured.v1"),
            )
            .await;
            let (status, created) = http_json(
                addr,
                reqwest::Method::POST,
                &revision_path(&wiki, ""),
                &wiki.session.session_token,
                None,
            )
            .await;
            assert_eq!(status, reqwest::StatusCode::CREATED, "{created}");
            let revision_id = created["id"].as_str().unwrap().to_string();
            apply_and_persist(
                addr,
                &member.session_token,
                &key,
                4,
                &engine_fixture("followup_edit.v1"),
            )
            .await;

            let before = count_updates(
                &wiki.session.pool,
                wiki.session.workspace_id,
                wiki.document_id,
            )
            .await;
            let restore_body = preview_restore_body(
                addr,
                &revision_path(&wiki, &format!("/{revision_id}/restore")),
                &member.session_token,
            )
            .await;
            let (reached, proceed) = arm_append_revoke_barrier(wiki.document_id).await;
            let restore = tokio::spawn({
                let token = member.session_token.clone();
                let path = revision_path(&wiki, &format!("/{revision_id}/restore"));
                async move {
                    http_json(
                        addr,
                        reqwest::Method::POST,
                        &path,
                        &token,
                        Some(restore_body),
                    )
                    .await
                }
            });
            reached.await.expect("restore reached persist barrier");
            workspace::remove_member(
                &wiki.session.pool,
                wiki.session.workspace_id,
                wiki.session.user_id,
                wiki.session.session_id,
                member.user_id,
                None,
            )
            .await
            .unwrap()
            .expect("removed member");
            let _ = proceed.send(());
            let (status, body) = restore.await.unwrap();
            assert_eq!(status, reqwest::StatusCode::CONFLICT, "{body}");
            assert_eq!(body["code"], "restore_rejected");
            let after = count_updates(
                &wiki.session.pool,
                wiki.session.workspace_id,
                wiki.document_id,
            )
            .await;
            assert_eq!(after, before, "rejected restore must not append");
            run.finish().await.expect("cleanup");
        },
    )
    .await;
}

#[tokio::test]
async fn restore_committed_ambiguity_rechecks_removed_actor() {
    run_test(
        "restore_committed_ambiguity_rechecks_removed_actor",
        async {
            let mut run = TestRun::new(support::TestDb::bootstrap().await);
            let wiki = setup_wiki_doc(&run.harness).await;
            let member = create_member_session(
                &run.harness,
                wiki.session.workspace_id,
                &format!("rev-member-{}@example.com", Uuid::now_v7().simple()),
            )
            .await;
            let (state, hub) =
                collab_app_state(&run.harness.app_url, test_collab_config(4, 60_000)).await;
            let addr = run.spawn_router_state(state, hub).await;
            let key = routing_key(wiki.session.workspace_id, wiki.document_id);
            apply_and_persist(
                addr,
                &wiki.session.session_token,
                &key,
                1,
                &engine_fixture("structured.v1"),
            )
            .await;
            let (status, created) = http_json(
                addr,
                reqwest::Method::POST,
                &revision_path(&wiki, ""),
                &wiki.session.session_token,
                None,
            )
            .await;
            assert_eq!(status, reqwest::StatusCode::CREATED, "{created}");
            let revision_id = created["id"].as_str().unwrap().to_string();
            apply_and_persist(
                addr,
                &member.session_token,
                &key,
                4,
                &engine_fixture("followup_edit.v1"),
            )
            .await;

            let before = count_updates(
                &wiki.session.pool,
                wiki.session.workspace_id,
                wiki.document_id,
            )
            .await;
            let (history_before, history_base, history_tail) =
                capture_history_contract(&wiki).await;
            let restore_body = preview_restore_body(
                addr,
                &revision_path(&wiki, &format!("/{revision_id}/restore")),
                &member.session_token,
            )
            .await;
            assert_eq!(restore_body["expectedTailSeq"], history_tail.to_string());
            assert!(history_base
                .content_json
                .to_string()
                .contains("후속편집한글"));

            let (reached, proceed) = fvoci_server::db::collab::arm_restore_committed_ambiguity(
                wiki.session.workspace_id,
                wiki.document_id,
            )
            .await;
            let restore = tokio::spawn({
                let token = member.session_token.clone();
                let path = revision_path(&wiki, &format!("/{revision_id}/restore"));
                let restore_body = restore_body.clone();
                async move {
                    http_json(
                        addr,
                        reqwest::Method::POST,
                        &path,
                        &token,
                        Some(restore_body.clone()),
                    )
                    .await
                }
            });
            reached.await.expect("restore reached persist barrier");
            workspace::remove_member(
                &wiki.session.pool,
                wiki.session.workspace_id,
                wiki.session.user_id,
                wiki.session.session_id,
                member.user_id,
                None,
            )
            .await
            .unwrap()
            .expect("removed member");
            let _ = proceed.send(());
            let (status, body) = restore.await.unwrap();
            assert_eq!(
                status,
                reqwest::StatusCode::SERVICE_UNAVAILABLE,
                "receipt recovery cannot authorize a removed actor: {body}"
            );
            let after = count_updates(
                &wiki.session.pool,
                wiki.session.workspace_id,
                wiki.document_id,
            )
            .await;
            assert_eq!(
                after,
                before + 1,
                "commit precedes revocation; no claim of rollback"
            );
            let (status, _) = http_json(
                addr,
                reqwest::Method::POST,
                &revision_path(&wiki, &format!("/{revision_id}/restore")),
                &member.session_token,
                Some(restore_body.clone()),
            )
            .await;
            assert_eq!(
                status,
                reqwest::StatusCode::NOT_FOUND,
                "current route authorization precedes correlation retry"
            );
            let (_, history) = http_json(
                addr,
                reqwest::Method::GET,
                &revision_path(&wiki, ""),
                &wiki.session.session_token,
                None,
            )
            .await;
            let history_after = assert_restore_history_contract(
                &wiki,
                &history_before,
                &history_base,
                &revision_id,
                None,
                member.user_id,
                &restore_body,
            )
            .await;
            assert_eq!(
                history["items"].as_array().unwrap().len(),
                history_after.len()
            );
            let restored = history["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["reason"] == "restore")
                .unwrap();
            assert_eq!(restored["restoredFromId"], revision_id);
            assert_eq!(restored["createdBy"], member.user_id.to_string());
            let durable = support::get_document_body(
                addr,
                &wiki.session.session_token,
                wiki.session.workspace_id,
                wiki.document_id,
            )
            .await;
            assert!(durable["contentJson"].to_string().contains("안녕 본문"));
            assert!(!durable["contentJson"].to_string().contains("후속편집한글"));
            run.finish().await.expect("cleanup");
        },
    )
    .await;
}

#[tokio::test]
async fn restore_committed_ambiguity_rechecks_read_only_actor() {
    run_test(
        "restore_committed_ambiguity_rechecks_read_only_actor",
        async {
            let mut run = TestRun::new(support::TestDb::bootstrap().await);
            let owner = setup_owner_session(&run.harness).await;
            let project = create_project(
                &owner.pool,
                owner.workspace_id,
                owner.user_id,
                owner.session_id,
                CreateProjectInput {
                    key: "R1READ",
                    name: "Restore WRITE-only demotion",
                    visibility: "private",
                    description: None,
                    icon: None,
                    lead_user_id: None,
                },
                None,
            )
            .await
            .unwrap()
            .unwrap();
            let document = create_project_document(
                &owner.pool,
                owner.workspace_id,
                project.id,
                owner.user_id,
                owner.session_id,
                CreateDocumentInput {
                    parent_id: project.root_document_id,
                    title: "Restore current READ",
                    icon: None,
                },
                None,
            )
            .await
            .unwrap()
            .unwrap();
            let wiki = WikiDocFixture {
                session: owner,
                document_id: document.id,
            };
            let revision_base = format!(
                "/api/v1/workspaces/{}/projects/{}/documents/{}/revisions",
                wiki.session.workspace_id, project.id, wiki.document_id
            );
            let member = create_member_session(
                &run.harness,
                wiki.session.workspace_id,
                &format!("rev-member-{}@example.com", Uuid::now_v7().simple()),
            )
            .await;
            add_project_member(
                &wiki.session.pool,
                wiki.session.workspace_id,
                project.id,
                wiki.session.user_id,
                wiki.session.session_id,
                member.user_id,
                ProjectMemberRole::Member,
                None,
            )
            .await
            .unwrap()
            .unwrap();
            let (state, hub) =
                collab_app_state(&run.harness.app_url, test_collab_config(4, 60_000)).await;
            let addr = run.spawn_router_state(state, hub).await;
            let key = routing_key(wiki.session.workspace_id, wiki.document_id);
            apply_and_persist(
                addr,
                &wiki.session.session_token,
                &key,
                1,
                &engine_fixture("structured.v1"),
            )
            .await;
            let (status, created) = http_json(
                addr,
                reqwest::Method::POST,
                &revision_base.clone(),
                &wiki.session.session_token,
                None,
            )
            .await;
            assert_eq!(status, reqwest::StatusCode::CREATED, "{created}");
            let revision_id = created["id"].as_str().unwrap().to_string();
            apply_and_persist(
                addr,
                &member.session_token,
                &key,
                4,
                &engine_fixture("followup_edit.v1"),
            )
            .await;

            let before = count_updates(
                &wiki.session.pool,
                wiki.session.workspace_id,
                wiki.document_id,
            )
            .await;
            let (history_before, history_base, history_tail) =
                capture_history_contract(&wiki).await;
            let restore_body = preview_restore_body(
                addr,
                &format!("{revision_base}/{revision_id}/restore"),
                &member.session_token,
            )
            .await;
            assert_eq!(restore_body["expectedTailSeq"], history_tail.to_string());
            assert!(history_base
                .content_json
                .to_string()
                .contains("후속편집한글"));

            let (reached, proceed) = fvoci_server::db::collab::arm_restore_committed_ambiguity(
                wiki.session.workspace_id,
                wiki.document_id,
            )
            .await;
            let restore = tokio::spawn({
                let token = member.session_token.clone();
                let path = format!("{revision_base}/{revision_id}/restore");
                let restore_body = restore_body.clone();
                async move {
                    http_json(
                        addr,
                        reqwest::Method::POST,
                        &path,
                        &token,
                        Some(restore_body.clone()),
                    )
                    .await
                }
            });
            reached.await.expect("restore reached persist barrier");
            fvoci_server::db::projects::update_project_member_role(
                &wiki.session.pool,
                wiki.session.workspace_id,
                project.id,
                wiki.session.user_id,
                wiki.session.session_id,
                member.user_id,
                ProjectMemberRole::Viewer,
                None,
            )
            .await
            .unwrap()
            .unwrap();
            let admission = resolve_collab_admission(
                &member.pool,
                member.workspace_id,
                member.user_id,
                member.session_id,
                wiki.document_id,
            )
            .await
            .unwrap()
            .unwrap();
            assert!(
                admission.read_only,
                "actor retains authenticated READ while losing WRITE"
            );
            let (read_status, _) = http_json(
                addr,
                reqwest::Method::GET,
                &format!("{revision_base}/{revision_id}"),
                &member.session_token,
                None,
            )
            .await;
            assert_eq!(
                read_status,
                reqwest::StatusCode::OK,
                "receipt READ remains authorized"
            );
            let _ = proceed.send(());
            let (status, body) = restore.await.unwrap();
            assert_eq!(
                status,
                reqwest::StatusCode::CONFLICT,
                "verified receipt READ must not bypass current WRITE: {body}"
            );
            assert_eq!(body["code"], "restore_rejected");
            let after = count_updates(
                &wiki.session.pool,
                wiki.session.workspace_id,
                wiki.document_id,
            )
            .await;
            assert_eq!(
                after,
                before + 1,
                "commit precedes revocation; no claim of rollback"
            );
            let (status, _) = http_json(
                addr,
                reqwest::Method::POST,
                &format!("{revision_base}/{revision_id}/restore"),
                &member.session_token,
                Some(restore_body.clone()),
            )
            .await;
            assert_eq!(
                status,
                reqwest::StatusCode::NOT_FOUND,
                "current route authorization precedes correlation retry"
            );
            let (_, history) = http_json(
                addr,
                reqwest::Method::GET,
                &revision_base.clone(),
                &wiki.session.session_token,
                None,
            )
            .await;
            let history_after = assert_restore_history_contract(
                &wiki,
                &history_before,
                &history_base,
                &revision_id,
                None,
                member.user_id,
                &restore_body,
            )
            .await;
            assert_eq!(
                history["items"].as_array().unwrap().len(),
                history_after.len()
            );
            let restored = history["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["reason"] == "restore")
                .unwrap();
            assert_eq!(restored["restoredFromId"], revision_id);
            assert_eq!(restored["createdBy"], member.user_id.to_string());
            let (status, durable) = http_json(
                addr,
                reqwest::Method::GET,
                &format!(
                    "/api/v1/workspaces/{}/projects/{}/documents/{}/body",
                    wiki.session.workspace_id, project.id, wiki.document_id
                ),
                &wiki.session.session_token,
                None,
            )
            .await;
            assert_eq!(status, reqwest::StatusCode::OK);
            assert!(durable["contentJson"].to_string().contains("안녕 본문"));
            assert!(!durable["contentJson"].to_string().contains("후속편집한글"));
            run.finish().await.expect("cleanup");
        },
    )
    .await;
}

#[tokio::test]
async fn restart_after_restore_serves_restored_content() {
    run_test("restart_after_restore_serves_restored_content", async {
        let mut run = TestRun::new(support::TestDb::bootstrap().await);
        let wiki = setup_wiki_doc(&run.harness).await;
        let (state, hub) =
            collab_app_state(&run.harness.app_url, test_collab_config(4, 60_000)).await;
        let addr = run.spawn_router_state(state, hub).await;
        let key = routing_key(wiki.session.workspace_id, wiki.document_id);
        apply_and_persist(
            addr,
            &wiki.session.session_token,
            &key,
            1,
            &engine_fixture("structured.v1"),
        )
        .await;
        let (status, created) = http_json(
            addr,
            reqwest::Method::POST,
            &revision_path(&wiki, ""),
            &wiki.session.session_token,
            None,
        )
        .await;
        assert_eq!(status, reqwest::StatusCode::CREATED, "{created}");
        let revision_id = created["id"].as_str().unwrap().to_string();
        apply_and_persist(
            addr,
            &wiki.session.session_token,
            &key,
            2,
            &engine_fixture("followup_edit.v1"),
        )
        .await;
        let restore_body = preview_restore_body(
            addr,
            &revision_path(&wiki, &format!("/{revision_id}/restore")),
            &wiki.session.session_token,
        )
        .await;
        let (status, restored) = http_json(
            addr,
            reqwest::Method::POST,
            &revision_path(&wiki, &format!("/{revision_id}/restore")),
            &wiki.session.session_token,
            Some(restore_body),
        )
        .await;
        assert_eq!(status, reqwest::StatusCode::OK, "{restored}");
        let before = support::get_document_body(
            addr,
            &wiki.session.session_token,
            wiki.session.workspace_id,
            wiki.document_id,
        )
        .await;
        run.shutdown_last_server().await.expect("stop");
        let (state, hub) =
            collab_app_state(&run.harness.app_url, test_collab_config(4, 60_000)).await;
        let addr = run.spawn_router_state(state, hub).await;
        let after = support::get_document_body(
            addr,
            &wiki.session.session_token,
            wiki.session.workspace_id,
            wiki.document_id,
        )
        .await;
        assert_eq!(after["contentJson"], before["contentJson"]);
        let mut fresh = connect_member(addr, &wiki.session.session_token).await;
        auth_and_join(&mut fresh, &key, 11).await;
        complete_sync_handshake(&mut fresh, &key).await;
        run.finish().await.expect("cleanup");
    })
    .await;
}

#[tokio::test]
async fn revision_write_is_rate_limited() {
    run_test("revision_write_is_rate_limited", async {
        let mut run = TestRun::new(support::TestDb::bootstrap().await);
        let wiki = setup_wiki_doc(&run.harness).await;
        let (state, hub) =
            collab_app_state(&run.harness.app_url, test_collab_config(4, 60_000)).await;
        let addr = run.spawn_router_state(state, hub).await;
        let key = routing_key(wiki.session.workspace_id, wiki.document_id);
        apply_and_persist(
            addr,
            &wiki.session.session_token,
            &key,
            1,
            &engine_fixture("structured.v1"),
        )
        .await;
        let mut saw_429 = false;
        for _ in 0..31 {
            let (status, _) = http_json(
                addr,
                reqwest::Method::POST,
                &revision_path(&wiki, ""),
                &wiki.session.session_token,
                None,
            )
            .await;
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                saw_429 = true;
                break;
            }
            assert_eq!(status, reqwest::StatusCode::CREATED);
        }
        assert!(saw_429, "31st revision write must 429");
        run.finish().await.expect("cleanup");
    })
    .await;
}

#[tokio::test]
async fn restore_timeout_before_append_is_504_and_nothing_persisted() {
    run_test(
        "restore_timeout_before_append_is_504_and_nothing_persisted",
        async {
            let mut run = TestRun::new(support::TestDb::bootstrap().await);
            let wiki = setup_wiki_doc(&run.harness).await;
            let mut cfg = test_collab_config(4, 60_000);
            cfg.rpc_timeout_ms = 200;
            let (state, hub) = collab_app_state(&run.harness.app_url, cfg).await;
            let addr = run.spawn_router_state(state, hub).await;
            let key = routing_key(wiki.session.workspace_id, wiki.document_id);
            apply_and_persist(
                addr,
                &wiki.session.session_token,
                &key,
                1,
                &engine_fixture("structured.v1"),
            )
            .await;
            let (status, created) = http_json(
                addr,
                reqwest::Method::POST,
                &revision_path(&wiki, ""),
                &wiki.session.session_token,
                None,
            )
            .await;
            assert_eq!(status, reqwest::StatusCode::CREATED, "{created}");
            let revision_id = created["id"].as_str().unwrap().to_string();
            let before = count_updates(
                &wiki.session.pool,
                wiki.session.workspace_id,
                wiki.document_id,
            )
            .await;
            let restore_body = preview_restore_body(
                addr,
                &revision_path(&wiki, &format!("/{revision_id}/restore")),
                &wiki.session.session_token,
            )
            .await;
            let (reached, proceed) = arm_append_revoke_barrier(wiki.document_id).await;
            let restore = tokio::spawn({
                let token = wiki.session.session_token.clone();
                let path = revision_path(&wiki, &format!("/{revision_id}/restore"));
                async move {
                    http_json(
                        addr,
                        reqwest::Method::POST,
                        &path,
                        &token,
                        Some(restore_body),
                    )
                    .await
                }
            });
            reached.await.expect("restore reached barrier");
            let (status, body) = restore.await.unwrap();
            let _ = proceed.send(());
            assert_eq!(status, reqwest::StatusCode::GATEWAY_TIMEOUT, "{body}");
            assert_eq!(body["code"], "collab_timeout_retry");
            let during = count_updates(
                &wiki.session.pool,
                wiki.session.workspace_id,
                wiki.document_id,
            )
            .await;
            assert_eq!(during, before, "504 must mean no append yet");
            run.finish().await.expect("cleanup");
        },
    )
    .await;
}

async fn clear_session_revisions(harness: &support::TestDb, workspace_id: Uuid, document_id: Uuid) {
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    sqlx::query(
        r#"
        DELETE FROM fvoci.revisions
        WHERE workspace_id = $1 AND target_kind = 'document' AND target_id = $2 AND reason = 'session'
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .execute(&admin)
    .await
    .unwrap();
    admin.close().await;
}

async fn count_session_revisions(
    harness: &support::TestDb,
    workspace_id: Uuid,
    document_id: Uuid,
) -> i64 {
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar(
        r#"
        SELECT count(*)::bigint FROM fvoci.revisions
        WHERE workspace_id = $1 AND target_kind = 'document' AND target_id = $2 AND reason = 'session'
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    admin.close().await;
    count
}

async fn expected_committed_session_capture(
    pool: &PgPool,
    workspace_id: Uuid,
    document_id: Uuid,
    cfg: &CollabConfig,
) -> CapturedRevision {
    let durable =
        load_durable_collab_for_system(pool, workspace_id, RevisionTarget::Document(document_id))
            .await
            .expect("durable load")
            .expect("durable state");
    capture_revision_offline(
        cfg.engine_bin.clone(),
        cfg.limits,
        durable.snapshot,
        durable.tail,
    )
    .expect("offline capture of durable DB collab")
}

async fn wait_session_revision_count(
    harness: &support::TestDb,
    workspace_id: Uuid,
    document_id: Uuid,
    expected: i64,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(12);
    loop {
        let count = count_session_revisions(harness, workspace_id, document_id).await;
        if count == expected {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!(
                "expected {expected} session revisions, last count {count} for document {document_id}"
            );
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Bounded wait for the armed last-leave persist barrier; on timeout names the stage that stopped short.
async fn await_session_persist_barrier(
    hub: &CollabHub,
    harness: &support::TestDb,
    workspace_id: Uuid,
    document_id: Uuid,
    reached: tokio::sync::oneshot::Receiver<()>,
) {
    let Ok(reached) = tokio::time::timeout(Duration::from_secs(8), reached).await else {
        let armed = session_revision_persist_barrier_armed(document_id).await;
        let probe = tokio::time::timeout(
            Duration::from_secs(5),
            hub.probe_actor(room_key(workspace_id, document_id)),
        )
        .await;
        let count = tokio::time::timeout(
            Duration::from_secs(5),
            count_session_revisions(harness, workspace_id, document_id),
        )
        .await
        .ok();
        let stage = match (armed, &probe) {
            (false, _) => "barrier consumed without a reached signal",
            (true, Err(_)) => "room actor stalled before the persist pause (capture/recycle/head read)",
            (true, Ok(_)) if count.is_some_and(|count| count > 0) => {
                "last-leave work deduped against an existing session revision before the persist pause"
            }
            (true, Ok(_)) => {
                "last-leave work never scheduled or aborted before the persist pause (capture/compare/text)"
            }
        };
        panic!(
            "session revision persist barrier not reached: {stage}; armed={armed} probe={probe:?} session_count={count:?}"
        );
    };
    reached.expect("session revision persist barrier");
}

#[tokio::test]
async fn session_revision_on_last_disconnect_two_clients() {
    run_test("session_revision_on_last_disconnect_two_clients", async {
        let mut run = TestRun::new(support::TestDb::bootstrap().await);
        let wiki = setup_wiki_doc(&run.harness).await;
        let member = create_member_session(
            &run.harness,
            wiki.session.workspace_id,
            "rev-member@t.local",
        )
        .await;
        let (state, hub) =
            collab_app_state(&run.harness.app_url, test_collab_config(4, 60_000)).await;
        let addr = run.spawn_router_state(state, hub).await;
        let key = routing_key(wiki.session.workspace_id, wiki.document_id);
        let update = engine_fixture("structured.v1");

        let mut ws0 = connect_member(addr, &wiki.session.session_token).await;
        auth_and_join(&mut ws0, &key, 1).await;
        complete_sync_handshake(&mut ws0, &key).await;
        ws0.send(Message::Binary(sync_update_frame(&key, &update).into()))
            .await
            .unwrap();
        assert!(wait_for_sync_applied(&mut ws0, Duration::from_secs(8)).await);
        let request_id = Uuid::now_v7();
        ws0.send(Message::Binary(
            stateless_frame(&key, &format!("persist:{request_id}")).into(),
        ))
        .await
        .unwrap();
        assert!(
            wait_for_stateless_exact(
                &mut ws0,
                &format!("persisted:{request_id}"),
                Duration::from_secs(8)
            )
            .await
        );

        let mut ws1 = connect_member(addr, &member.session_token).await;
        auth_and_join(&mut ws1, &key, 2).await;
        complete_sync_handshake(&mut ws1, &key).await;

        let _ = ws0.close(None).await;
        wait_session_revision_count(&run.harness, wiki.session.workspace_id, wiki.document_id, 0)
            .await;

        let _ = ws1.close(None).await;
        wait_session_revision_count(&run.harness, wiki.session.workspace_id, wiki.document_id, 1)
            .await;

        let admin = PgPoolOptions::new()
            .max_connections(2)
            .connect(&run.harness.admin_url)
            .await
            .unwrap();
        let row: (Option<Uuid>, String) = sqlx::query_as(
            r#"
            SELECT created_by, reason FROM fvoci.revisions
            WHERE workspace_id = $1 AND target_id = $2 AND reason = 'session'
            "#,
        )
        .bind(wiki.session.workspace_id)
        .bind(wiki.document_id)
        .fetch_one(&admin)
        .await
        .unwrap();
        admin.close().await;
        assert!(row.0.is_none(), "session revision must be system-authored");
        assert_eq!(row.1, "session");

        run.finish().await.expect("cleanup");
    })
    .await;
}

#[tokio::test]
async fn session_revision_disabled_by_config() {
    run_test("session_revision_disabled_by_config", async {
        let mut run = TestRun::new(support::TestDb::bootstrap().await);
        let wiki = setup_wiki_doc(&run.harness).await;
        let mut cfg = test_collab_config(4, 60_000);
        cfg.revision_session_snapshot = false;
        let (state, hub) = collab_app_state(&run.harness.app_url, cfg).await;
        let addr = run.spawn_router_state(state, hub).await;
        let key = routing_key(wiki.session.workspace_id, wiki.document_id);
        apply_and_persist(
            addr,
            &wiki.session.session_token,
            &key,
            1,
            &engine_fixture("structured.v1"),
        )
        .await;
        assert_eq!(
            count_session_revisions(&run.harness, wiki.session.workspace_id, wiki.document_id,)
                .await,
            0
        );
        run.finish().await.expect("cleanup");
    })
    .await;
}

#[tokio::test]
async fn session_revision_dedupes_unchanged_reconnect() {
    run_test("session_revision_dedupes_unchanged_reconnect", async {
        let mut run = TestRun::new(support::TestDb::bootstrap().await);
        let wiki = setup_wiki_doc(&run.harness).await;
        let (state, hub) =
            collab_app_state(&run.harness.app_url, test_collab_config(4, 60_000)).await;
        let addr = run.spawn_router_state(state, hub).await;
        let key = routing_key(wiki.session.workspace_id, wiki.document_id);
        let update = engine_fixture("structured.v1");
        for round in 0..2 {
            apply_and_persist(addr, &wiki.session.session_token, &key, 10 + round, &update).await;
            wait_session_revision_count(
                &run.harness,
                wiki.session.workspace_id,
                wiki.document_id,
                1,
            )
            .await;
        }
        assert_eq!(
            count_session_revisions(&run.harness, wiki.session.workspace_id, wiki.document_id,)
                .await,
            1,
            "unchanged content across reconnect must not append another session row"
        );
        run.finish().await.expect("cleanup");
    })
    .await;
}

#[tokio::test]
async fn session_revision_y_snapshot_matches_durable_db_collab() {
    run_test(
        "session_revision_y_snapshot_matches_durable_db_collab",
        async {
            let mut run = TestRun::new(support::TestDb::bootstrap().await);
            let wiki = setup_wiki_doc(&run.harness).await;
            let (state, hub) =
                collab_app_state(&run.harness.app_url, test_collab_config(4, 60_000)).await;
            let addr = run.spawn_router_state(state, hub).await;
            let key = routing_key(wiki.session.workspace_id, wiki.document_id);
            apply_and_persist(
                addr,
                &wiki.session.session_token,
                &key,
                1,
                &engine_fixture("structured.v1"),
            )
            .await;
            wait_session_revision_count(
                &run.harness,
                wiki.session.workspace_id,
                wiki.document_id,
                1,
            )
            .await;

            let cfg = test_collab_config(4, 60_000);
            let expected = expected_committed_session_capture(
                &wiki.session.pool,
                wiki.session.workspace_id,
                wiki.document_id,
                &cfg,
            )
            .await;

            let admin = PgPoolOptions::new()
                .max_connections(2)
                .connect(&run.harness.admin_url)
                .await
                .unwrap();
            let row: (Vec<u8>, Value) = sqlx::query_as(
                r#"
            SELECT y_snapshot, content_json FROM fvoci.revisions
            WHERE workspace_id = $1 AND target_id = $2 AND reason = 'session'
            "#,
            )
            .bind(wiki.session.workspace_id)
            .bind(wiki.document_id)
            .fetch_one(&admin)
            .await
            .unwrap();
            admin.close().await;
            assert_eq!(
                row.0, expected.y_snapshot,
                "session y_snapshot must match durable DB collab"
            );
            assert_eq!(
                row.1, expected.content_json,
                "session content_json must match durable DB projection"
            );
            run.finish().await.expect("cleanup");
        },
    )
    .await;
}

#[tokio::test]
async fn session_revision_after_rejected_update_uses_committed_snapshot() {
    run_test(
        "session_revision_after_rejected_update_uses_committed_snapshot",
        async {
            let mut run = TestRun::new(support::TestDb::bootstrap().await);
            let wiki = setup_wiki_doc(&run.harness).await;
            let (state, hub) =
                collab_app_state(&run.harness.app_url, test_collab_config(4, 60_000)).await;
            let addr = run.spawn_router_state(state, hub).await;
            let key = routing_key(wiki.session.workspace_id, wiki.document_id);
            let valid = engine_fixture("structured.v1");
            let malformed = support::invalid_utf8_update_candidate();

            let mut writer = connect_member(addr, &wiki.session.session_token).await;
            auth_and_join(&mut writer, &key, 1).await;
            complete_sync_handshake(&mut writer, &key).await;
            writer
                .send(Message::Binary(sync_update_frame(&key, &valid).into()))
                .await
                .unwrap();
            assert!(wait_for_sync_applied(&mut writer, Duration::from_secs(8)).await);
            let request_id = Uuid::now_v7();
            writer
                .send(Message::Binary(
                    stateless_frame(&key, &format!("persist:{request_id}")).into(),
                ))
                .await
                .unwrap();
            assert!(
                wait_for_stateless_exact(
                    &mut writer,
                    &format!("persisted:{request_id}"),
                    Duration::from_secs(8)
                )
                .await
            );

            let mut peer = connect_member(addr, &wiki.session.session_token).await;
            auth_and_join(&mut peer, &key, 2).await;
            writer
                .send(Message::Binary(sync_update_frame(&key, &malformed).into()))
                .await
                .unwrap();
            support::wait_for_policy_rejection_close(
                &mut writer,
                &mut peer,
                &key,
                &malformed,
                Duration::from_secs(8),
            )
            .await;

            let _ = peer.close(None).await;
            let _ = writer.close(None).await;
            wait_session_revision_count(
                &run.harness,
                wiki.session.workspace_id,
                wiki.document_id,
                1,
            )
            .await;

            let admin = PgPoolOptions::new()
                .max_connections(2)
                .connect(&run.harness.admin_url)
                .await
                .unwrap();
            let cfg = test_collab_config(4, 60_000);
            let expected = expected_committed_session_capture(
                &wiki.session.pool,
                wiki.session.workspace_id,
                wiki.document_id,
                &cfg,
            )
            .await;
            let row: (Vec<u8>, Value) = sqlx::query_as(
                r#"
            SELECT y_snapshot, content_json FROM fvoci.revisions
            WHERE workspace_id = $1 AND target_id = $2 AND reason = 'session'
            "#,
            )
            .bind(wiki.session.workspace_id)
            .bind(wiki.document_id)
            .fetch_one(&admin)
            .await
            .unwrap();
            admin.close().await;
            assert_eq!(row.0, expected.y_snapshot);
            assert_eq!(row.1, expected.content_json);
            run.finish().await.expect("cleanup");
        },
    )
    .await;
}

#[tokio::test]
async fn session_revision_matches_durable_after_revoked_append_barrier() {
    run_test(
        "session_revision_matches_durable_after_revoked_append_barrier",
        async {
            let mut run = TestRun::new(support::TestDb::bootstrap().await);
            let wiki = setup_wiki_doc(&run.harness).await;
            let (state, hub) =
                collab_app_state(&run.harness.app_url, test_collab_config(4, 60_000)).await;
            let addr = run.spawn_router_state(state, hub).await;
            let key = routing_key(wiki.session.workspace_id, wiki.document_id);
            let first = engine_fixture("structured.v1");
            let second = engine_fixture("followup_edit.v1");

            let writer_token = {
                let token = new_token();
                let session_id = Uuid::now_v7();
                let expires = Utc::now() + ChronoDuration::days(1);
                let mut tx = wiki.session.pool.begin().await.unwrap();
                fvoci_server::db::identity::create_session(
                    &mut tx,
                    session_id,
                    wiki.session.user_id,
                    &token.hash,
                    expires,
                )
                .await
                .unwrap();
                tx.commit().await.unwrap();
                token
            };

            let mut reader = connect_member(addr, &wiki.session.session_token).await;
            auth_and_join(&mut reader, &key, 1).await;
            complete_sync_handshake(&mut reader, &key).await;
            reader
                .send(Message::Binary(sync_update_frame(&key, &first).into()))
                .await
                .unwrap();
            assert!(wait_for_sync_applied(&mut reader, Duration::from_secs(8)).await);
            let request_id = Uuid::now_v7();
            reader
                .send(Message::Binary(
                    stateless_frame(&key, &format!("persist:{request_id}")).into(),
                ))
                .await
                .unwrap();
            assert!(
                wait_for_stateless_exact(
                    &mut reader,
                    &format!("persisted:{request_id}"),
                    Duration::from_secs(8)
                )
                .await
            );

            let mut writer = connect_member(addr, &writer_token.token).await;
            auth_and_join(&mut writer, &key, 2).await;
            let (reached, proceed) = arm_append_revoke_barrier(wiki.document_id).await;
            writer
                .send(Message::Binary(sync_update_frame(&key, &second).into()))
                .await
                .unwrap();
            tokio::time::timeout(Duration::from_secs(8), reached)
                .await
                .expect("append must reach revoke barrier")
                .expect("barrier");
            revoke_session(
                &wiki.session.pool,
                &writer_token.hash,
                Some(wiki.session.user_id),
            )
            .await
            .expect("revoke writer");
            proceed.send(()).expect("release barrier");
            fvoci_server::collab::room::disarm_append_revoke_barrier(wiki.document_id).await;

            let _ = writer.close(None).await;
            let _ = reader.close(None).await;
            wait_session_revision_count(
                &run.harness,
                wiki.session.workspace_id,
                wiki.document_id,
                1,
            )
            .await;

            let cfg = test_collab_config(4, 60_000);
            let expected = expected_committed_session_capture(
                &wiki.session.pool,
                wiki.session.workspace_id,
                wiki.document_id,
                &cfg,
            )
            .await;
            let admin = PgPoolOptions::new()
                .max_connections(2)
                .connect(&run.harness.admin_url)
                .await
                .unwrap();
            let row: (Vec<u8>, Value) = sqlx::query_as(
                r#"
            SELECT y_snapshot, content_json FROM fvoci.revisions
            WHERE workspace_id = $1 AND target_id = $2 AND reason = 'session'
            "#,
            )
            .bind(wiki.session.workspace_id)
            .bind(wiki.document_id)
            .fetch_one(&admin)
            .await
            .unwrap();
            admin.close().await;
            assert_eq!(row.0, expected.y_snapshot);
            assert_eq!(row.1, expected.content_json);
            run.finish().await.expect("cleanup");
        },
    )
    .await;
}

#[tokio::test]
async fn session_revision_persists_through_immediate_reconnect() {
    run_test(
        "session_revision_persists_through_immediate_reconnect",
        async {
            let mut run = TestRun::new(support::TestDb::bootstrap().await);
            let wiki = setup_wiki_doc(&run.harness).await;
            let (state, hub) =
                collab_app_state(&run.harness.app_url, test_collab_config(4, 60_000)).await;
            let hub = hub.clone();
            let addr = run.spawn_router_state(state, hub.clone()).await;
            let key = routing_key(wiki.session.workspace_id, wiki.document_id);
            apply_and_persist(
                addr,
                &wiki.session.session_token,
                &key,
                1,
                &engine_fixture("structured.v1"),
            )
            .await;
            wait_session_revision_count(
                &run.harness,
                wiki.session.workspace_id,
                wiki.document_id,
                1,
            )
            .await;
            clear_session_revisions(&run.harness, wiki.session.workspace_id, wiki.document_id)
                .await;

            let cfg = test_collab_config(4, 60_000);
            let expected = expected_committed_session_capture(
                &wiki.session.pool,
                wiki.session.workspace_id,
                wiki.document_id,
                &cfg,
            )
            .await;

            let workspace_id = wiki.session.workspace_id;
            let document_id = wiki.document_id;
            let session_id = wiki.session.session_id;
            let user_id = wiki.session.user_id;

            let mut ws = connect_member(addr, &wiki.session.session_token).await;
            auth_and_join(&mut ws, &key, 10).await;
            let (persist_reached, persist_proceed) =
                arm_session_revision_persist_barrier(wiki.document_id).await;
            let _ = ws.close(None).await;
            await_session_persist_barrier(
                &hub,
                &run.harness,
                workspace_id,
                document_id,
                persist_reached,
            )
            .await;
            assert_eq!(
                count_session_revisions(&run.harness, wiki.session.workspace_id, wiki.document_id,)
                    .await,
                0,
                "last-leave snapshot must not be inserted before barrier release"
            );

            let rejoin_conn = Uuid::now_v7();
            let admission_witness = arm_join_channel_admission_witness(rejoin_conn).await;
            let rejoin = tokio::spawn({
                let hub = hub.clone();
                async move {
                    hub_join_with_conn(
                        &hub,
                        workspace_id,
                        document_id,
                        session_id,
                        user_id,
                        11,
                        rejoin_conn,
                    )
                    .await
                }
            });
            tokio::time::timeout(Duration::from_secs(5), admission_witness)
                .await
                .expect("rejoin join must be queued on the room mailbox")
                .expect("join channel admission witness");

            persist_proceed
                .send(())
                .expect("release session persist barrier");
            disarm_session_revision_persist_barrier(wiki.document_id).await;

            wait_session_revision_count(
                &run.harness,
                wiki.session.workspace_id,
                wiki.document_id,
                1,
            )
            .await;

            let rejoin_lease = tokio::time::timeout(Duration::from_secs(5), rejoin)
                .await
                .expect("hub rejoin must complete after persist barrier release")
                .expect("rejoin task")
                .expect("hub rejoin after leave");
            disarm_join_channel_admission_witness(rejoin_conn).await;

            let admin = PgPoolOptions::new()
                .max_connections(2)
                .connect(&run.harness.admin_url)
                .await
                .unwrap();
            let row: (Vec<u8>, Value) = sqlx::query_as(
                r#"
            SELECT y_snapshot, content_json FROM fvoci.revisions
            WHERE workspace_id = $1 AND target_id = $2 AND reason = 'session'
            "#,
            )
            .bind(wiki.session.workspace_id)
            .bind(wiki.document_id)
            .fetch_one(&admin)
            .await
            .unwrap();
            admin.close().await;
            assert_eq!(row.0, expected.y_snapshot);
            assert_eq!(row.1, expected.content_json);

            hub.leave_room(
                room_key(wiki.session.workspace_id, wiki.document_id),
                rejoin_conn,
            )
            .await;
            drop(rejoin_lease);
            tokio::time::sleep(Duration::from_millis(200)).await;
            assert_eq!(
                count_session_revisions(&run.harness, wiki.session.workspace_id, wiki.document_id,)
                    .await,
                1,
                "unchanged content after reconnect leave must dedupe, not erase prior snapshot"
            );
            run.finish().await.expect("cleanup");
        },
    )
    .await;
}

/// Transport enqueues `Leave` and then drops the lease; the actor may select the lease drop first.
/// The trailing `Leave` for the already-evicted connection must not schedule another capture.
#[tokio::test]
async fn session_revision_stale_leave_after_lease_drop_does_not_reschedule() {
    run_test(
        "session_revision_stale_leave_after_lease_drop_does_not_reschedule",
        async {
            let mut run = TestRun::new(support::TestDb::bootstrap().await);
            let wiki = setup_wiki_doc(&run.harness).await;
            let (state, hub) =
                collab_app_state(&run.harness.app_url, test_collab_config(4, 60_000)).await;
            let hub = hub.clone();
            let addr = run.spawn_router_state(state, hub.clone()).await;
            let key = routing_key(wiki.session.workspace_id, wiki.document_id);
            apply_and_persist(
                addr,
                &wiki.session.session_token,
                &key,
                1,
                &engine_fixture("structured.v1"),
            )
            .await;
            wait_session_revision_count(
                &run.harness,
                wiki.session.workspace_id,
                wiki.document_id,
                1,
            )
            .await;

            let room = room_key(wiki.session.workspace_id, wiki.document_id);
            let conn_id = Uuid::now_v7();
            let lease = hub_join_with_conn(
                &hub,
                wiki.session.workspace_id,
                wiki.document_id,
                wiki.session.session_id,
                wiki.session.user_id,
                20,
                conn_id,
            )
            .await
            .expect("hub join");
            drop(lease);
            // The first probe drains the lease drop; the second is served only after that
            // turn's session revision work has finished.
            hub.probe_actor(room).await;
            assert_eq!(hub.probe_actor(room).await.connections, 0);
            assert_eq!(
                count_session_revisions(&run.harness, wiki.session.workspace_id, wiki.document_id)
                    .await,
                1,
                "unchanged lease-drop leave must dedupe"
            );

            clear_session_revisions(&run.harness, wiki.session.workspace_id, wiki.document_id)
                .await;
            hub.leave_room(room, conn_id).await;
            assert_eq!(hub.probe_actor(room).await.connections, 0);
            assert_eq!(
                count_session_revisions(&run.harness, wiki.session.workspace_id, wiki.document_id)
                    .await,
                0,
                "stale leave for an evicted connection must not schedule a session revision"
            );
            run.finish().await.expect("cleanup");
        },
    )
    .await;
}

async fn assert_session_revision_matches(
    harness: &support::TestDb,
    workspace_id: Uuid,
    document_id: Uuid,
    expected: &CapturedRevision,
) {
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let row: (Vec<u8>, Value) = sqlx::query_as(
        r#"
        SELECT y_snapshot, content_json FROM fvoci.revisions
        WHERE workspace_id = $1 AND target_id = $2 AND reason = 'session'
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    admin.close().await;
    assert_eq!(row.0, expected.y_snapshot);
    assert_eq!(row.1, expected.content_json);
}

/// The sole connection is evicted with 1009 because a sync reply exceeds its outbound byte
/// budget. The transport then sends a stale `Leave`; the eviction itself must schedule the
/// last-disconnect session revision.
#[tokio::test]
async fn session_revision_on_last_connection_backpressure_close() {
    run_test(
        "session_revision_on_last_connection_backpressure_close",
        async {
            let mut run = TestRun::new(support::TestDb::bootstrap().await);
            let wiki = setup_wiki_doc(&run.harness).await;
            let workspace_id = wiki.session.workspace_id;
            let document_id = wiki.document_id;
            append_outside_room(&wiki.session, document_id, "structured.v1").await;
            let mut cfg = test_collab_config(4, 60_000);
            // Step2 carries the whole document (> 831 bytes); small frames still fit.
            cfg.max_outbound_bytes_per_connection = 512;
            let expected = expected_committed_session_capture(
                &wiki.session.pool,
                workspace_id,
                document_id,
                &cfg,
            )
            .await;
            let (state, hub) = collab_app_state(&run.harness.app_url, cfg).await;
            let addr = run.spawn_router_state(state, hub).await;
            let key = routing_key(workspace_id, document_id);

            let mut ws = connect_member(addr, &wiki.session.session_token).await;
            auth_and_join(&mut ws, &key, 30).await;
            ws.send(Message::Binary(sync_step1_frame(&key, &[0, 0]).into()))
                .await
                .unwrap();
            wait_for_ws_close_code(
                &mut ws,
                1009,
                Duration::from_secs(8),
                false,
                Some("outbound queue full"),
            )
            .await;

            wait_session_revision_count(&run.harness, workspace_id, document_id, 1).await;
            assert_session_revision_matches(&run.harness, workspace_id, document_id, &expected)
                .await;
            run.finish().await.expect("cleanup");
        },
    )
    .await;
}

/// Frame-permit backpressure evicts the last hub connection, then the lease drop and a stale
/// `Leave` arrive in that order. The eviction turn captures the session revision; the lease drop
/// and the stale `Leave` must not schedule another one.
#[tokio::test]
async fn session_revision_backpressure_eviction_then_lease_drop_then_stale_leave() {
    run_test(
        "session_revision_backpressure_eviction_then_lease_drop_then_stale_leave",
        async {
            let mut run = TestRun::new(support::TestDb::bootstrap().await);
            let wiki = setup_wiki_doc(&run.harness).await;
            let workspace_id = wiki.session.workspace_id;
            let document_id = wiki.document_id;
            append_outside_room(&wiki.session, document_id, "structured.v1").await;
            let mut cfg = test_collab_config(4, 60_000);
            // Step1 is answered with Step2 then Step1; the undrained second frame has no permit.
            cfg.max_outbound_frames_per_connection = 1;
            let expected = expected_committed_session_capture(
                &wiki.session.pool,
                workspace_id,
                document_id,
                &cfg,
            )
            .await;
            let (state, hub) = collab_app_state(&run.harness.app_url, cfg).await;
            let hub = hub.clone();
            let _addr = run.spawn_router_state(state, hub.clone()).await;
            let room = room_key(workspace_id, document_id);
            let key = routing_key(workspace_id, document_id);

            let conn_id = Uuid::now_v7();
            let (events_tx, mut events_rx) = tokio::sync::mpsc::channel(8);
            let lease = hub_join_with_events(
                &hub,
                workspace_id,
                document_id,
                wiki.session.session_id,
                wiki.session.user_id,
                31,
                conn_id,
                events_tx,
            )
            .await
            .expect("hub join");
            let handle = hub.ensure_live_room(room).await.expect("live room");
            handle.frame(conn_id, sync_step1_frame(&key, &[0, 0])).await;
            // Served after the frame turn, including its loop-tail session revision work.
            assert_eq!(hub.probe_actor(room).await.connections, 0);
            let mut saw_close = false;
            while let Ok(event) = events_rx.try_recv() {
                if let fvoci_server::collab::room::RoomClientEvent::Close { code, .. } = event {
                    assert_eq!(code, 1009);
                    saw_close = true;
                }
            }
            assert!(saw_close, "backpressure eviction must close with 1009");
            assert_eq!(
                count_session_revisions(&run.harness, workspace_id, document_id).await,
                1,
                "last-connection backpressure eviction must capture a session revision"
            );
            assert_session_revision_matches(&run.harness, workspace_id, document_id, &expected)
                .await;

            clear_session_revisions(&run.harness, workspace_id, document_id).await;
            drop(lease);
            // The first probe drains the lease drop; the second follows that turn's loop tail.
            hub.probe_actor(room).await;
            assert_eq!(hub.probe_actor(room).await.connections, 0);
            hub.leave_room(room, conn_id).await;
            assert_eq!(hub.probe_actor(room).await.connections, 0);
            assert_eq!(
                count_session_revisions(&run.harness, workspace_id, document_id).await,
                0,
                "lease drop and stale leave after eviction must not schedule another capture"
            );
            run.finish().await.expect("cleanup");
        },
    )
    .await;
}

#[tokio::test]
async fn session_revision_skips_when_writer_generation_stale() {
    run_test(
        "session_revision_skips_when_writer_generation_stale",
        async {
            let mut run = TestRun::new(support::TestDb::bootstrap().await);
            let wiki = setup_wiki_doc(&run.harness).await;
            let (state, hub) =
                collab_app_state(&run.harness.app_url, test_collab_config(4, 60_000)).await;
            let addr = run.spawn_router_state(state, hub).await;
            let key = routing_key(wiki.session.workspace_id, wiki.document_id);
            let update = engine_fixture("structured.v1");

            let mut ws = connect_member(addr, &wiki.session.session_token).await;
            auth_and_join(&mut ws, &key, 1).await;
            complete_sync_handshake(&mut ws, &key).await;
            ws.send(Message::Binary(sync_update_frame(&key, &update).into()))
                .await
                .unwrap();
            assert!(wait_for_sync_applied(&mut ws, Duration::from_secs(8)).await);
            let request_id = Uuid::now_v7();
            ws.send(Message::Binary(
                stateless_frame(&key, &format!("persist:{request_id}")).into(),
            ))
            .await
            .unwrap();
            assert!(
                wait_for_stateless_exact(
                    &mut ws,
                    &format!("persisted:{request_id}"),
                    Duration::from_secs(8)
                )
                .await
            );

            let admin = PgPoolOptions::new()
                .max_connections(2)
                .connect(&run.harness.admin_url)
                .await
                .unwrap();
            sqlx::query(
                r#"
            UPDATE fvoci.document_states
            SET writer_generation = writer_generation + 1
            WHERE workspace_id = $1 AND document_id = $2
            "#,
            )
            .bind(wiki.session.workspace_id)
            .bind(wiki.document_id)
            .execute(&admin)
            .await
            .unwrap();
            admin.close().await;

            let _ = ws.close(None).await;
            wait_session_revision_count(
                &run.harness,
                wiki.session.workspace_id,
                wiki.document_id,
                0,
            )
            .await;
            run.finish().await.expect("cleanup");
        },
    )
    .await;
}

#[tokio::test]
async fn manual_revision_after_stale_writer_close_uses_newer_durable_state() {
    run_test(
        "manual_revision_after_stale_writer_close_uses_newer_durable_state",
        async {
            let mut run = TestRun::new(support::TestDb::bootstrap().await);
            let wiki = setup_wiki_doc(&run.harness).await;
            let cfg = test_collab_config(4, 60_000);
            let (state, hub) = collab_app_state(&run.harness.app_url, cfg.clone()).await;
            let addr = run.spawn_router_state(state, hub.clone()).await;
            let key = routing_key(wiki.session.workspace_id, wiki.document_id);

            let mut ws = connect_member(addr, &wiki.session.session_token).await;
            auth_and_join(&mut ws, &key, 1).await;
            complete_sync_handshake(&mut ws, &key).await;
            ws.send(Message::Binary(
                sync_update_frame(&key, &engine_fixture("structured.v1")).into(),
            ))
            .await
            .unwrap();
            assert!(wait_for_sync_applied(&mut ws, Duration::from_secs(8)).await);

            // A newer durable writer takes over and commits past the room's tail.
            let claim = claim_writer_and_load(
                &wiki.session.pool,
                wiki.session.workspace_id,
                wiki.session.user_id,
                wiki.session.session_id,
                wiki.document_id,
            )
            .await
            .unwrap()
            .unwrap();
            let newer = append_collab_update(
                &wiki.session.pool,
                AppendCollabInput {
                    workspace_id: wiki.session.workspace_id,
                    actor_user_id: wiki.session.user_id,
                    session_id: wiki.session.session_id,
                    document_id: wiki.document_id,
                    writer_generation: claim.writer_generation,
                    expected_tail_seq: claim.load.tail_seq,
                    op_id: Uuid::now_v7(),
                    payload: &engine_fixture("followup_edit.v1"),
                    client_ip: None,
                },
            )
            .await
            .unwrap()
            .unwrap();
            assert!(matches!(newer, AppendCollabResult::Committed { .. }));

            // The room's next append hits the stale generation and closes every connection.
            ws.send(Message::Binary(
                sync_update_frame(&key, &engine_fixture("korean_emoji_base.v1")).into(),
            ))
            .await
            .unwrap();
            wait_for_ws_close_code(
                &mut ws,
                1008,
                Duration::from_secs(8),
                false,
                Some("writer stale"),
            )
            .await;
            wait_room_empty(&hub, wiki.session.workspace_id, wiki.document_id).await;

            let expected = expected_committed_session_capture(
                &wiki.session.pool,
                wiki.session.workspace_id,
                wiki.document_id,
                &cfg,
            )
            .await;
            let live = hub
                .capture_if_live(
                    (wiki.session.workspace_id, wiki.document_id),
                    wiki.session.user_id,
                    wiki.session.session_id,
                )
                .await
                .expect("stale-writer room stays live with no connections")
                .expect("live capture");
            assert_eq!(
                live.content_json, expected.content_json,
                "live capture after stale-writer close must use newer durable state"
            );
            assert_eq!(
                live.y_snapshot, expected.y_snapshot,
                "live capture y_snapshot must match newer durable state"
            );

            let (status, created) = http_json(
                addr,
                reqwest::Method::POST,
                &revision_path(&wiki, ""),
                &wiki.session.session_token,
                None,
            )
            .await;
            assert_eq!(status, reqwest::StatusCode::CREATED, "{created}");
            let revision_id = created["id"].as_str().expect("id");
            let (status, detail) = http_json(
                addr,
                reqwest::Method::GET,
                &revision_path(&wiki, &format!("/{revision_id}")),
                &wiki.session.session_token,
                None,
            )
            .await;
            assert_eq!(status, reqwest::StatusCode::OK, "{detail}");
            assert_eq!(
                detail["contentJson"], expected.content_json,
                "manual revision after stale-writer close must use newer durable state"
            );
            run.finish().await.expect("cleanup");
        },
    )
    .await;
}

/// Commit one durable update outside any live room (the import-job shape).
async fn append_outside_room(owner: &SessionFixture, document_id: Uuid, fixture: &str) {
    let claim = claim_writer_and_load(
        &owner.pool,
        owner.workspace_id,
        owner.user_id,
        owner.session_id,
        document_id,
    )
    .await
    .unwrap()
    .unwrap();
    let appended = append_collab_update(
        &owner.pool,
        AppendCollabInput {
            workspace_id: owner.workspace_id,
            actor_user_id: owner.user_id,
            session_id: owner.session_id,
            document_id,
            writer_generation: claim.writer_generation,
            expected_tail_seq: claim.load.tail_seq,
            op_id: Uuid::now_v7(),
            payload: &engine_fixture(fixture),
            client_ip: None,
        },
    )
    .await
    .unwrap()
    .unwrap();
    assert!(matches!(appended, AppendCollabResult::Committed { .. }));
}

#[tokio::test]
async fn manual_revision_in_viewer_only_room_uses_newer_durable_state() {
    run_test(
        "manual_revision_in_viewer_only_room_uses_newer_durable_state",
        async {
            let mut run = TestRun::new(support::TestDb::bootstrap().await);
            let owner = setup_owner_session(&run.harness).await;
            let project = create_project(
                &owner.pool,
                owner.workspace_id,
                owner.user_id,
                owner.session_id,
                CreateProjectInput {
                    key: "VIEWRO",
                    name: "Viewer-only room project",
                    visibility: "private",
                    description: None,
                    icon: None,
                    lead_user_id: None,
                },
                None,
            )
            .await
            .expect("create project")
            .expect("ok");
            let created = create_project_document(
                &owner.pool,
                owner.workspace_id,
                project.id,
                owner.user_id,
                owner.session_id,
                CreateDocumentInput {
                    parent_id: project.root_document_id,
                    title: "imported",
                    icon: None,
                },
                None,
            )
            .await
            .expect("create project doc")
            .expect("ok");
            let viewer = create_member_session(
                &run.harness,
                owner.workspace_id,
                "viewer-only-room@example.com",
            )
            .await;
            add_project_member(
                &owner.pool,
                owner.workspace_id,
                project.id,
                owner.user_id,
                owner.session_id,
                viewer.user_id,
                ProjectMemberRole::Viewer,
                None,
            )
            .await
            .expect("add viewer")
            .expect("ok");
            let admission = resolve_collab_admission(
                &viewer.pool,
                viewer.workspace_id,
                viewer.user_id,
                viewer.session_id,
                created.id,
            )
            .await
            .unwrap()
            .expect("viewer admitted");
            assert!(admission.read_only, "viewer must join read-only");

            let wiki = WikiDocFixture {
                session: owner,
                document_id: created.id,
            };
            let cfg = test_collab_config(4, 60_000);
            let (state, hub) = collab_app_state(&run.harness.app_url, cfg.clone()).await;
            let addr = run.spawn_router_state(state, hub.clone()).await;
            let key = routing_key(wiki.session.workspace_id, wiki.document_id);
            let room = (wiki.session.workspace_id, wiki.document_id);

            // The viewer opens the fresh document first: the room loads the empty
            // durable state without claiming a writer generation.
            let mut ws = connect_member(addr, &viewer.session_token).await;
            auth_and_join(&mut ws, &key, 1).await;
            complete_sync_handshake(&mut ws, &key).await;
            assert_eq!(hub.room_member_count(room).await, 1);

            // R1: the single out-of-room append (import shape), then a capture while
            // the viewer-only room stays live with no stale close.
            append_outside_room(&wiki.session, wiki.document_id, "structured.v1").await;
            let expected = expected_committed_session_capture(
                &wiki.session.pool,
                wiki.session.workspace_id,
                wiki.document_id,
                &cfg,
            )
            .await;
            assert_ne!(
                expected.content_json,
                json!({"type":"doc","content":[]}),
                "durable state must contain the out-of-room append"
            );
            let live = hub
                .capture_if_live(room, wiki.session.user_id, wiki.session.session_id)
                .await
                .expect("viewer-only room is live")
                .expect("live capture");
            assert_eq!(
                live.content_json, expected.content_json,
                "viewer-only live capture must use newer durable state"
            );
            assert_eq!(
                live.y_snapshot, expected.y_snapshot,
                "viewer-only live capture y_snapshot must match durable state"
            );

            // R2: a later out-of-room append after that capture's reload. The
            // revisions HTTP API is wiki-only, so capture through the hub path
            // that POST /revisions uses (`capture_for_create`).
            append_outside_room(&wiki.session, wiki.document_id, "followup_edit.v1").await;
            let expected = expected_committed_session_capture(
                &wiki.session.pool,
                wiki.session.workspace_id,
                wiki.document_id,
                &cfg,
            )
            .await;
            assert_ne!(
                expected.content_json, live.content_json,
                "the later append must change durable state"
            );
            let live = hub
                .capture_if_live(room, wiki.session.user_id, wiki.session.session_id)
                .await
                .expect("viewer-only room is live")
                .expect("live capture");
            assert_eq!(
                live.content_json, expected.content_json,
                "capture after a later out-of-room append must use durable state"
            );
            assert_eq!(
                live.y_snapshot, expected.y_snapshot,
                "capture y_snapshot after a later out-of-room append must match durable state"
            );
            assert_eq!(hub.room_member_count(room).await, 1, "viewer stays joined");
            run.finish().await.expect("cleanup");
        },
    )
    .await;
}

#[tokio::test]
async fn session_revision_skips_when_document_trashed_under_lock() {
    run_test(
        "session_revision_skips_when_document_trashed_under_lock",
        async {
            let mut run = TestRun::new(support::TestDb::bootstrap().await);
            let wiki = setup_wiki_doc(&run.harness).await;
            let (state, hub) =
                collab_app_state(&run.harness.app_url, test_collab_config(4, 60_000)).await;
            let addr = run.spawn_router_state(state, hub).await;
            let key = routing_key(wiki.session.workspace_id, wiki.document_id);
            let update = engine_fixture("structured.v1");

            let mut ws = connect_member(addr, &wiki.session.session_token).await;
            auth_and_join(&mut ws, &key, 1).await;
            complete_sync_handshake(&mut ws, &key).await;
            ws.send(Message::Binary(sync_update_frame(&key, &update).into()))
                .await
                .unwrap();
            assert!(wait_for_sync_applied(&mut ws, Duration::from_secs(8)).await);
            let request_id = Uuid::now_v7();
            ws.send(Message::Binary(
                stateless_frame(&key, &format!("persist:{request_id}")).into(),
            ))
            .await
            .unwrap();
            assert!(
                wait_for_stateless_exact(
                    &mut ws,
                    &format!("persisted:{request_id}"),
                    Duration::from_secs(8)
                )
                .await
            );

            let admin = PgPoolOptions::new()
                .max_connections(4)
                .connect(&run.harness.admin_url)
                .await
                .unwrap();
            let mut barrier = admin.begin().await.unwrap();
            sqlx::query(
                "SELECT id FROM fvoci.documents WHERE workspace_id = $1 AND id = $2 FOR UPDATE",
            )
            .bind(wiki.session.workspace_id)
            .bind(wiki.document_id)
            .execute(&mut *barrier)
            .await
            .unwrap();

            let close = tokio::spawn(async move {
                let _ = ws.close(None).await;
            });
            tokio::time::sleep(Duration::from_millis(150)).await;
            sqlx::query(
                r#"
            UPDATE fvoci.documents
            SET deleted_at = now()
            WHERE workspace_id = $1 AND id = $2
            "#,
            )
            .bind(wiki.session.workspace_id)
            .bind(wiki.document_id)
            .execute(&mut *barrier)
            .await
            .unwrap();
            barrier.commit().await.unwrap();
            let _ = close.await;
            tokio::time::sleep(Duration::from_millis(200)).await;

            assert_eq!(
                count_session_revisions(&run.harness, wiki.session.workspace_id, wiki.document_id,)
                    .await,
                0,
                "trashed document must not get a new session revision row"
            );
            let orphan_count: i64 = sqlx::query_scalar(
                r#"
            SELECT count(*)::bigint FROM fvoci.revisions
            WHERE workspace_id = $1 AND target_id = $2
            "#,
            )
            .bind(wiki.session.workspace_id)
            .bind(wiki.document_id)
            .fetch_one(&admin)
            .await
            .unwrap();
            assert_eq!(
                orphan_count, 0,
                "no revision rows may remain for trashed target"
            );
            admin.close().await;
            run.finish().await.expect("cleanup");
        },
    )
    .await;
}

fn revision_maintenance_params(
    cfg: &CollabConfig,
    interval_hours: u32,
) -> RevisionMaintenanceParams {
    RevisionMaintenanceParams {
        settings: RevisionSettings {
            session_snapshot_enabled: false,
            keep: 200,
            snapshot_interval_hours: interval_hours,
        },
        engine: Some(RevisionMaintenanceEngine {
            engine_bin: cfg.engine_bin.clone(),
            limits: cfg.limits,
        }),
    }
}

async fn count_scheduled_revisions(
    harness: &support::TestDb,
    workspace_id: Uuid,
    document_id: Uuid,
) -> i64 {
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar(
        r#"
        SELECT count(*)::bigint FROM fvoci.revisions
        WHERE workspace_id = $1 AND target_kind = 'document' AND target_id = $2 AND reason = 'scheduled'
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    admin.close().await;
    count
}

async fn seed_task_collab(
    run: &TestRun,
    addr: std::net::SocketAddr,
    session: &SessionFixture,
    task_id: Uuid,
    content: Value,
) {
    let key = CollabRoomName {
        workspace_id: session.workspace_id,
        kind: CollabKind::Task,
        resource_id: task_id,
    }
    .routing_key();
    let update = SeedEngine::from_hub(&run.hub())
        .tiptap_to_yjs_update(&content)
        .await
        .expect("task seed update");
    let mut ws = connect_member(addr, &session.session_token).await;
    auth_and_join(&mut ws, &key, 1).await;
    complete_sync_handshake(&mut ws, &key).await;
    ws.send(Message::Binary(sync_update_frame(&key, &update).into()))
        .await
        .unwrap();
    assert!(wait_for_sync_applied(&mut ws, Duration::from_secs(8)).await);
    let request_id = Uuid::now_v7();
    ws.send(Message::Binary(
        stateless_frame(&key, &format!("persist:{request_id}")).into(),
    ))
    .await
    .unwrap();
    assert!(
        wait_for_stateless_exact(
            &mut ws,
            &format!("persisted:{request_id}"),
            Duration::from_secs(8)
        )
        .await
    );
    let _ = ws.close(None).await;
}

async fn age_task_anchor(admin: &PgPool, workspace_id: Uuid, task_id: Uuid) {
    let old = Utc::now() - ChronoDuration::hours(30);
    sqlx::query(
        "UPDATE fvoci.task_states SET created_at = $3, updated_at = now() WHERE workspace_id = $1 AND task_id = $2",
    )
    .bind(workspace_id)
    .bind(task_id)
    .bind(old)
    .execute(admin)
    .await
    .unwrap();
}

async fn age_parent_task_only(admin: &PgPool, workspace_id: Uuid, task_id: Uuid) {
    let old = Utc::now() - ChronoDuration::hours(30);
    sqlx::query("UPDATE fvoci.tasks SET created_at = $3 WHERE workspace_id = $1 AND id = $2")
        .bind(workspace_id)
        .bind(task_id)
        .bind(old)
        .execute(admin)
        .await
        .unwrap();
}

async fn seed_collab_edit(addr: std::net::SocketAddr, wiki: &WikiDocFixture) {
    let key = routing_key(wiki.session.workspace_id, wiki.document_id);
    let update = engine_fixture("structured.v1");
    let mut ws = connect_member(addr, &wiki.session.session_token).await;
    auth_and_join(&mut ws, &key, 1).await;
    complete_sync_handshake(&mut ws, &key).await;
    ws.send(Message::Binary(sync_update_frame(&key, &update).into()))
        .await
        .unwrap();
    assert!(wait_for_sync_applied(&mut ws, Duration::from_secs(8)).await);
    let request_id = Uuid::now_v7();
    ws.send(Message::Binary(
        stateless_frame(&key, &format!("persist:{request_id}")).into(),
    ))
    .await
    .unwrap();
    assert!(
        wait_for_stateless_exact(
            &mut ws,
            &format!("persisted:{request_id}"),
            Duration::from_secs(8)
        )
        .await
    );
    let _ = ws.close(None).await;
}

async fn age_document_anchor(admin: &PgPool, workspace_id: Uuid, document_id: Uuid) {
    let old = Utc::now() - ChronoDuration::hours(30);
    sqlx::query(
        "UPDATE fvoci.document_states SET created_at = $3, updated_at = now() WHERE workspace_id = $1 AND document_id = $2",
    )
    .bind(workspace_id)
    .bind(document_id)
    .bind(old)
    .execute(admin)
    .await
    .unwrap();
}

async fn age_parent_document_only(admin: &PgPool, workspace_id: Uuid, document_id: Uuid) {
    let old = Utc::now() - ChronoDuration::hours(30);
    sqlx::query("UPDATE fvoci.documents SET created_at = $3 WHERE workspace_id = $1 AND id = $2")
        .bind(workspace_id)
        .bind(document_id)
        .bind(old)
        .execute(admin)
        .await
        .unwrap();
}

#[tokio::test]
async fn scheduled_revision_document_stale_creates_row() {
    run_test("scheduled_revision_document_stale_creates_row", async {
        let mut run = TestRun::new(support::TestDb::bootstrap().await);
        let wiki = setup_wiki_doc(&run.harness).await;
        let mut cfg = test_collab_config(4, 60_000);
        cfg.revision_session_snapshot = false;
        let (state, hub) = collab_app_state(&run.harness.app_url, cfg.clone()).await;
        let addr = run.spawn_router_state(state, hub).await;
        seed_collab_edit(addr, &wiki).await;

        let admin = PgPoolOptions::new()
            .max_connections(2)
            .connect(&run.harness.admin_url)
            .await
            .unwrap();
        age_document_anchor(&admin, wiki.session.workspace_id, wiki.document_id).await;
        admin.close().await;

        let params = revision_maintenance_params(&cfg, 24);
        let (stats, _) = run_revision_maintenance_batch(
            &wiki.session.pool,
            &params,
            RevisionMaintenanceResume::default(),
            &CancellationToken::new(),
        )
        .await
        .expect("sweep");
        assert_eq!(stats.snapshots_created, 1);
        assert_eq!(
            count_scheduled_revisions(&run.harness, wiki.session.workspace_id, wiki.document_id,)
                .await,
            1
        );
        run.finish().await.expect("cleanup");
    })
    .await;
}

#[tokio::test]
async fn scheduled_revision_skips_recent_anchor() {
    run_test("scheduled_revision_skips_recent_anchor", async {
        let mut run = TestRun::new(support::TestDb::bootstrap().await);
        let wiki = setup_wiki_doc(&run.harness).await;
        let mut cfg = test_collab_config(4, 60_000);
        cfg.revision_session_snapshot = false;
        let (state, hub) = collab_app_state(&run.harness.app_url, cfg.clone()).await;
        let addr = run.spawn_router_state(state, hub).await;
        seed_collab_edit(addr, &wiki).await;

        let admin = PgPoolOptions::new()
            .max_connections(2)
            .connect(&run.harness.admin_url)
            .await
            .unwrap();
        let captured = expected_committed_session_capture(
            &wiki.session.pool,
            wiki.session.workspace_id,
            wiki.document_id,
            &cfg,
        )
        .await;
        let revision_id = Uuid::now_v7();
        sqlx::query(
            r#"
            INSERT INTO fvoci.revisions (
                id, workspace_id, target_kind, target_id, y_snapshot, encoding,
                content_json, text, reason, created_by, created_at
            ) VALUES ($1, $2, 'document', $3, $4, 1, $5, 'recent', 'session', NULL, now() - interval '1 hour')
            "#,
        )
        .bind(revision_id)
        .bind(wiki.session.workspace_id)
        .bind(wiki.document_id)
        .bind(&captured.y_snapshot)
        .bind(&captured.content_json)
        .execute(&admin)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE fvoci.document_states SET updated_at = now() WHERE workspace_id = $1 AND document_id = $2",
        )
        .bind(wiki.session.workspace_id)
        .bind(wiki.document_id)
        .execute(&admin)
        .await
        .unwrap();
        admin.close().await;

        let params = revision_maintenance_params(&cfg, 24);
        let (stats, _) = run_revision_maintenance_batch(
            &wiki.session.pool,
            &params,
            RevisionMaintenanceResume::default(),
            &CancellationToken::new(),
        )
        .await
        .expect("sweep");
        assert_eq!(stats.snapshots_created, 0);
        assert_eq!(
            count_scheduled_revisions(
                &run.harness,
                wiki.session.workspace_id,
                wiki.document_id,
            )
            .await,
            0
        );
        run.finish().await.expect("cleanup");
    })
    .await;
}

#[tokio::test]
async fn scheduled_revision_semantic_dedup_on_second_sweep() {
    run_test("scheduled_revision_semantic_dedup_on_second_sweep", async {
        let mut run = TestRun::new(support::TestDb::bootstrap().await);
        let wiki = setup_wiki_doc(&run.harness).await;
        let mut cfg = test_collab_config(4, 60_000);
        cfg.revision_session_snapshot = false;
        let (state, hub) = collab_app_state(&run.harness.app_url, cfg.clone()).await;
        let addr = run.spawn_router_state(state, hub).await;
        seed_collab_edit(addr, &wiki).await;

        let admin = PgPoolOptions::new()
            .max_connections(2)
            .connect(&run.harness.admin_url)
            .await
            .unwrap();
        age_document_anchor(&admin, wiki.session.workspace_id, wiki.document_id).await;
        admin.close().await;

        let params = revision_maintenance_params(&cfg, 24);
        let cancel = CancellationToken::new();
        run_revision_maintenance_batch(
            &wiki.session.pool,
            &params,
            RevisionMaintenanceResume::default(),
            &cancel,
        )
        .await
        .expect("first");
        sqlx::query(
            r#"
            UPDATE fvoci.revisions
            SET created_at = $3
            WHERE workspace_id = $1 AND target_kind = 'document' AND target_id = $2 AND reason = 'scheduled'
            "#,
        )
        .bind(wiki.session.workspace_id)
        .bind(wiki.document_id)
        .bind(Utc::now() - ChronoDuration::hours(30))
        .execute(
            &PgPoolOptions::new()
                .max_connections(2)
                .connect(&run.harness.admin_url)
                .await
                .unwrap(),
        )
        .await
        .unwrap();
        let (stats, _) = run_revision_maintenance_batch(
            &wiki.session.pool,
            &params,
            RevisionMaintenanceResume::default(),
            &cancel,
        )
        .await
        .expect("second");
        assert_eq!(stats.snapshots_created, 0);
        assert_eq!(
            stats.snapshots_deduped,
            1,
            "unchanged body with stale anchor must hit semantic dedup (skipped={})",
            stats.snapshots_skipped
        );
        assert_eq!(
            count_scheduled_revisions(&run.harness, wiki.session.workspace_id, wiki.document_id,)
                .await,
            1
        );
        run.finish().await.expect("cleanup");
    })
    .await;
}

#[tokio::test]
async fn scheduled_revision_disabled_when_interval_zero() {
    run_test("scheduled_revision_disabled_when_interval_zero", async {
        let mut run = TestRun::new(support::TestDb::bootstrap().await);
        let wiki = setup_wiki_doc(&run.harness).await;
        let mut cfg = test_collab_config(4, 60_000);
        cfg.revision_session_snapshot = false;
        let (state, hub) = collab_app_state(&run.harness.app_url, cfg.clone()).await;
        let addr = run.spawn_router_state(state, hub).await;
        seed_collab_edit(addr, &wiki).await;

        let admin = PgPoolOptions::new()
            .max_connections(2)
            .connect(&run.harness.admin_url)
            .await
            .unwrap();
        age_document_anchor(&admin, wiki.session.workspace_id, wiki.document_id).await;
        admin.close().await;

        let params = revision_maintenance_params(&cfg, 0);
        let (stats, _) = run_revision_maintenance_batch(
            &wiki.session.pool,
            &params,
            RevisionMaintenanceResume::default(),
            &CancellationToken::new(),
        )
        .await
        .expect("sweep");
        assert_eq!(stats.snapshots_attempted, 0);
        assert_eq!(
            count_scheduled_revisions(&run.harness, wiki.session.workspace_id, wiki.document_id,)
                .await,
            0
        );
        run.finish().await.expect("cleanup");
    })
    .await;
}

async fn count_workspace_scheduled_revisions(harness: &support::TestDb, workspace_id: Uuid) -> i64 {
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar(
        r#"
        SELECT count(*)::bigint FROM fvoci.revisions
        WHERE workspace_id = $1 AND reason = 'scheduled'
        "#,
    )
    .bind(workspace_id)
    .fetch_one(&admin)
    .await
    .unwrap();
    admin.close().await;
    count
}

async fn copy_document_collab_state(
    admin: &PgPool,
    workspace_id: Uuid,
    from_document_id: Uuid,
    to_document_id: Uuid,
) {
    let (state, encoding, writer_generation): (Vec<u8>, i16, i64) = sqlx::query_as(
        r#"
        SELECT state, encoding, writer_generation
        FROM fvoci.document_states
        WHERE workspace_id = $1 AND document_id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(from_document_id)
    .fetch_one(admin)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO fvoci.document_states (
            workspace_id, document_id, state, encoding, writer_generation
        ) VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (workspace_id, document_id) DO UPDATE
        SET state = EXCLUDED.state,
            encoding = EXCLUDED.encoding,
            writer_generation = EXCLUDED.writer_generation,
            updated_at = now()
        "#,
    )
    .bind(workspace_id)
    .bind(to_document_id)
    .bind(&state)
    .bind(encoding)
    .bind(writer_generation)
    .execute(admin)
    .await
    .unwrap();
}

#[tokio::test]
async fn scheduled_revision_batch_continues_past_sixteen_targets() {
    run_test(
        "scheduled_revision_batch_continues_past_sixteen_targets",
        async {
            let target_count = SCHEDULED_REVISION_TARGET_BATCH + 2;
            let mut run = TestRun::new(support::TestDb::bootstrap().await);
            let docs = setup_wiki_doc_batch(&run.harness, target_count).await;
            let wiki = &docs[0];
            let mut cfg = test_collab_config(4, 60_000);
            cfg.revision_session_snapshot = false;
            let (state, hub) = collab_app_state(&run.harness.app_url, cfg.clone()).await;
            let addr = run.spawn_router_state(state, hub).await;
            seed_collab_edit(addr, wiki).await;

            let admin = PgPoolOptions::new()
                .max_connections(2)
                .connect(&run.harness.admin_url)
                .await
                .unwrap();
            for doc in docs.iter().skip(1) {
                copy_document_collab_state(
                    &admin,
                    wiki.session.workspace_id,
                    wiki.document_id,
                    doc.document_id,
                )
                .await;
                age_document_anchor(&admin, wiki.session.workspace_id, doc.document_id).await;
            }
            age_document_anchor(&admin, wiki.session.workspace_id, wiki.document_id).await;
            admin.close().await;

            let params = revision_maintenance_params(&cfg, 24);
            let cancel = CancellationToken::new();
            let (first, resume) = run_revision_maintenance_batch(
                &wiki.session.pool,
                &params,
                RevisionMaintenanceResume::default(),
                &cancel,
            )
            .await
            .expect("first batch");
            assert_eq!(
                first.snapshots_created,
                SCHEDULED_REVISION_TARGET_BATCH as u32
            );
            assert!(resume.workspace_id.is_some());
            assert!(resume.target.is_some());

            let (second, resume2) =
                run_revision_maintenance_batch(&wiki.session.pool, &params, resume, &cancel)
                    .await
                    .expect("second batch");
            assert_eq!(second.snapshots_created, 2);
            assert!(resume2.sweep_complete());
            assert_eq!(
                count_workspace_scheduled_revisions(&run.harness, wiki.session.workspace_id).await,
                target_count as i64
            );
            run.finish().await.expect("cleanup");
        },
    )
    .await;
}

#[tokio::test]
async fn scheduled_revision_task_stale_creates_row() {
    run_test("scheduled_revision_task_stale_creates_row", async {
        let mut run = TestRun::new(support::TestDb::bootstrap().await);
        let owner = setup_owner_session(&run.harness).await;
        let mut cfg = test_collab_config(4, 60_000);
        cfg.revision_session_snapshot = false;
        let (state, hub) = collab_app_state(&run.harness.app_url, cfg.clone()).await;
        let addr = run.spawn_router_state(state, hub).await;
        let project = create_project(
            &owner.pool,
            owner.workspace_id,
            owner.user_id,
            owner.session_id,
            CreateProjectInput {
                key: "SCHTASK",
                name: "Scheduled task project",
                visibility: "workspace",
                description: None,
                icon: None,
                lead_user_id: None,
            },
            None,
        )
        .await
        .expect("create project")
        .expect("ok");
        let task = create_task(
            &owner.pool,
            owner.workspace_id,
            project.id,
            owner.user_id,
            owner.session_id,
            CreateTaskInput {
                title: "scheduled snapshot task",
                task_type: "task",
                priority: "none",
                status_id: None,
                start_date: None,
                due_date: None,
                parent_id: None,
                milestone_id: None,
                recurrence: None,
            },
            None,
            "api",
        )
        .await
        .expect("create task")
        .expect("ok");
        let task_body = json!({
            "type": "doc",
            "content": [{
                "type": "paragraph",
                "attrs": {"id": "sch-t1"},
                "content": [{"type": "text", "text": "scheduled task body"}]
            }]
        });
        seed_task_collab(&run, addr, &owner, task.id, task_body.clone()).await;

        let admin = PgPoolOptions::new()
            .max_connections(2)
            .connect(&run.harness.admin_url)
            .await
            .unwrap();
        age_task_anchor(&admin, owner.workspace_id, task.id).await;
        admin.close().await;

        let params = revision_maintenance_params(&cfg, 24);
        let (stats, _) = run_revision_maintenance_batch(
            &owner.pool,
            &params,
            RevisionMaintenanceResume::default(),
            &CancellationToken::new(),
        )
        .await
        .expect("sweep");
        assert_eq!(stats.snapshots_created, 1);
        let admin = PgPoolOptions::new()
            .max_connections(2)
            .connect(&run.harness.admin_url)
            .await
            .unwrap();
        let (text, content_json): (String, Value) = sqlx::query_as(
            r#"
            SELECT text, content_json FROM fvoci.revisions
            WHERE workspace_id = $1 AND target_kind = 'task' AND target_id = $2 AND reason = 'scheduled'
            "#,
        )
        .bind(owner.workspace_id)
        .bind(task.id)
        .fetch_one(&admin)
        .await
        .unwrap();
        admin.close().await;
        assert!(text.contains("scheduled task body"), "{text}");
        assert_eq!(
            content_json["content"][0]["attrs"]["id"],
            json!("sch-t1")
        );
        run.finish().await.expect("cleanup");
    })
    .await;
}

#[tokio::test]
async fn scheduled_revision_skips_old_parent_new_document_state() {
    run_test(
        "scheduled_revision_skips_old_parent_new_document_state",
        async {
            let mut run = TestRun::new(support::TestDb::bootstrap().await);
            let wiki = setup_wiki_doc(&run.harness).await;
            let mut cfg = test_collab_config(4, 60_000);
            cfg.revision_session_snapshot = false;
            let (state, hub) = collab_app_state(&run.harness.app_url, cfg.clone()).await;
            let addr = run.spawn_router_state(state, hub).await;
            seed_collab_edit(addr, &wiki).await;

            let admin = PgPoolOptions::new()
                .max_connections(2)
                .connect(&run.harness.admin_url)
                .await
                .unwrap();
            age_parent_document_only(&admin, wiki.session.workspace_id, wiki.document_id).await;
            admin.close().await;

            let params = revision_maintenance_params(&cfg, 24);
            let (stats, _) = run_revision_maintenance_batch(
                &wiki.session.pool,
                &params,
                RevisionMaintenanceResume::default(),
                &CancellationToken::new(),
            )
            .await
            .expect("sweep");
            assert_eq!(stats.snapshots_created, 0);
            assert_eq!(
                count_scheduled_revisions(
                    &run.harness,
                    wiki.session.workspace_id,
                    wiki.document_id,
                )
                .await,
                0
            );
            run.finish().await.expect("cleanup");
        },
    )
    .await;
}

#[tokio::test]
async fn scheduled_revision_skips_old_parent_new_task_state() {
    run_test("scheduled_revision_skips_old_parent_new_task_state", async {
        let mut run = TestRun::new(support::TestDb::bootstrap().await);
        let owner = setup_owner_session(&run.harness).await;
        let mut cfg = test_collab_config(4, 60_000);
        cfg.revision_session_snapshot = false;
        let (state, hub) = collab_app_state(&run.harness.app_url, cfg.clone()).await;
        let addr = run.spawn_router_state(state, hub).await;
        let project = create_project(
            &owner.pool,
            owner.workspace_id,
            owner.user_id,
            owner.session_id,
            CreateProjectInput {
                key: "ANCHOR",
                name: "anchor project",
                visibility: "workspace",
                description: None,
                icon: None,
                lead_user_id: None,
            },
            None,
        )
        .await
        .expect("create project")
        .expect("ok");
        let task = create_task(
            &owner.pool,
            owner.workspace_id,
            project.id,
            owner.user_id,
            owner.session_id,
            CreateTaskInput {
                title: "anchor task",
                task_type: "task",
                priority: "none",
                status_id: None,
                start_date: None,
                due_date: None,
                parent_id: None,
                milestone_id: None,
                recurrence: None,
            },
            None,
            "api",
        )
        .await
        .expect("create task")
        .expect("ok");
        seed_task_collab(
            &run,
            addr,
            &owner,
            task.id,
            json!({
                "type": "doc",
                "content": [{
                    "type": "paragraph",
                    "attrs": {"id": "anc-t1"},
                    "content": [{"type": "text", "text": "fresh task state"}]
                }]
            }),
        )
        .await;

        let admin = PgPoolOptions::new()
            .max_connections(2)
            .connect(&run.harness.admin_url)
            .await
            .unwrap();
        age_parent_task_only(&admin, owner.workspace_id, task.id).await;
        admin.close().await;

        let params = revision_maintenance_params(&cfg, 24);
        let (stats, _) = run_revision_maintenance_batch(
            &owner.pool,
            &params,
            RevisionMaintenanceResume::default(),
            &CancellationToken::new(),
        )
        .await
        .expect("sweep");
        assert_eq!(stats.snapshots_created, 0);
        let scheduled: i64 = sqlx::query_scalar(
            r#"
            SELECT count(*)::bigint FROM fvoci.revisions
            WHERE workspace_id = $1 AND target_kind = 'task' AND target_id = $2 AND reason = 'scheduled'
            "#,
        )
        .bind(owner.workspace_id)
        .bind(task.id)
        .fetch_one(
            &PgPoolOptions::new()
                .max_connections(2)
                .connect(&run.harness.admin_url)
                .await
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(scheduled, 0);
        run.finish().await.expect("cleanup");
    })
    .await;
}

async fn insert_empty_live_workspaces(admin: &PgPool, count: usize) {
    for i in 0..count {
        let id = Uuid::now_v7();
        let slug = format!("ev{i:04}");
        sqlx::query("INSERT INTO fvoci.workspaces (id, slug, name) VALUES ($1, $2, 'Empty')")
            .bind(id)
            .bind(slug)
            .execute(admin)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn scheduled_revision_continues_past_empty_workspaces() {
    run_test(
        "scheduled_revision_continues_past_empty_workspaces",
        async {
            let run = TestRun::new(support::TestDb::bootstrap().await);
            let mut cfg = test_collab_config(4, 60_000);
            cfg.revision_session_snapshot = false;
            let params = revision_maintenance_params(&cfg, 24);

            let admin = PgPoolOptions::new()
                .max_connections(2)
                .connect(&run.harness.admin_url)
                .await
                .unwrap();
            let extra = WORKSPACE_SCAN_BATCH as usize + 4;
            insert_empty_live_workspaces(&admin, extra).await;
            let total: i64 = sqlx::query_scalar(
                "SELECT count(*)::bigint FROM fvoci.workspaces WHERE deleted_at IS NULL",
            )
            .fetch_one(&admin)
            .await
            .unwrap();
            admin.close().await;
            assert!(total > WORKSPACE_SCAN_BATCH);

            let pool = pool::connect_app(&run.harness.app_url).await.unwrap();
            let cancel = CancellationToken::new();
            let (first, resume) = run_revision_maintenance_batch(
                &pool,
                &params,
                RevisionMaintenanceResume::default(),
                &cancel,
            )
            .await
            .expect("first batch");
            assert_eq!(first.snapshots_created, 0);
            assert!(resume.workspace_id.is_some());
            assert!(resume.target.is_none());

            let mut resume = resume;
            let mut batches = 1u32;
            while resume.workspace_id.is_some() || resume.target.is_some() {
                assert!(batches < 32, "scheduled scan did not finish");
                let (stats, next) = run_revision_maintenance_batch(&pool, &params, resume, &cancel)
                    .await
                    .expect("continuation batch");
                assert_eq!(stats.snapshots_created, 0);
                resume = next;
                batches += 1;
            }
            assert!(batches >= 2);
            pool.close().await;
            run.finish().await.expect("cleanup");
        },
    )
    .await;
}

// ---------------------------------------------------------------------------
// Manual revision creation is fenced against credential and member revocation
// ---------------------------------------------------------------------------

async fn admin_pool_of(harness: &support::TestDb) -> PgPool {
    PgPoolOptions::new()
        .max_connections(4)
        .connect(&harness.admin_url)
        .await
        .unwrap()
}

/// Pid of the backend running a statement like `query_like` that waits on a
/// lock held by `blocker_pid`.
async fn wait_for_revision_write_blocked(
    admin: &PgPool,
    blocker_pid: i32,
    query_like: &str,
) -> i32 {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        let blocked: Option<i32> = sqlx::query_scalar(
            r#"
            SELECT activity.pid
            FROM pg_stat_activity AS activity
            WHERE activity.datname = current_database()
              AND activity.wait_event_type = 'Lock'
              AND activity.state = 'active'
              AND activity.query ILIKE $2
              AND $1 = ANY(pg_blocking_pids(activity.pid))
            LIMIT 1
            "#,
        )
        .bind(blocker_pid)
        .bind(query_like)
        .fetch_optional(admin)
        .await
        .unwrap();
        if let Some(pid) = blocked {
            return pid;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the manual revision write never waited on {query_like} held by pid {blocker_pid}");
}

async fn count_target_revisions(admin: &PgPool, workspace_id: Uuid, target_id: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM fvoci.revisions WHERE workspace_id = $1 AND target_id = $2",
    )
    .bind(workspace_id)
    .bind(target_id)
    .fetch_one(admin)
    .await
    .unwrap()
}

async fn hold_users_row(
    admin: &PgPool,
    user_id: Uuid,
) -> (sqlx::Transaction<'static, sqlx::Postgres>, i32) {
    let mut barrier = admin.begin().await.unwrap();
    sqlx::query("SELECT id FROM fvoci.users WHERE id = $1 FOR UPDATE")
        .bind(user_id)
        .fetch_one(&mut *barrier)
        .await
        .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *barrier)
        .await
        .unwrap();
    (barrier, pid)
}

/// A logout that commits while a manual revision POST waits on the actor's
/// credential row is seen by the write: 404 and no revision row.
#[tokio::test]
async fn manual_revision_refuses_session_revoked_while_waiting() {
    run_test(
        "manual_revision_refuses_session_revoked_while_waiting",
        async {
            let mut run = TestRun::new(support::TestDb::bootstrap().await);
            let wiki = setup_wiki_doc(&run.harness).await;
            let (state, hub) =
                collab_app_state(&run.harness.app_url, test_collab_config(4, 60_000)).await;
            let addr = run.spawn_router_state(state, hub).await;
            append_outside_room(&wiki.session, wiki.document_id, "structured.v1").await;
            let admin = admin_pool_of(&run.harness).await;
            let workspace_id = wiki.session.workspace_id;
            let before = count_target_revisions(&admin, workspace_id, wiki.document_id).await;

            let (mut barrier, blocker_pid) = hold_users_row(&admin, wiki.session.user_id).await;
            let path = revision_path(&wiki, "");
            let token = wiki.session.session_token.clone();
            let post = tokio::spawn(async move {
                http_json(addr, reqwest::Method::POST, &path, &token, None).await
            });
            wait_for_revision_write_blocked(&admin, blocker_pid, "%fvoci.users%FOR UPDATE%").await;
            sqlx::query("UPDATE fvoci.sessions SET revoked_at = now() WHERE id = $1")
                .bind(wiki.session.session_id)
                .execute(&mut *barrier)
                .await
                .unwrap();
            barrier.commit().await.unwrap();

            let (status, body) = tokio::time::timeout(Duration::from_secs(10), post)
                .await
                .expect("post finished")
                .expect("join");
            assert_eq!(status, reqwest::StatusCode::NOT_FOUND, "{body}");
            assert_eq!(
                count_target_revisions(&admin, workspace_id, wiki.document_id).await,
                before
            );
            admin.close().await;
            run.finish().await.expect("cleanup");
        },
    )
    .await;
}

/// Same fence for a task revision created with an API token (the token branch
/// of the credential recheck): the token owner is suspended while the POST
/// waits.
#[tokio::test]
async fn manual_task_revision_refuses_token_owner_suspended_while_waiting() {
    run_test(
        "manual_task_revision_refuses_token_owner_suspended_while_waiting",
        async {
            let mut run = TestRun::new(support::TestDb::bootstrap().await);
            let owner = setup_owner_session(&run.harness).await;
            let (state, hub) =
                collab_app_state(&run.harness.app_url, test_collab_config(4, 60_000)).await;
            let addr = run.spawn_router_state(state, hub.clone()).await;
            let project = create_project(
                &owner.pool,
                owner.workspace_id,
                owner.user_id,
                owner.session_id,
                CreateProjectInput {
                    key: "REVPAT",
                    name: "Revision token project",
                    visibility: "workspace",
                    description: None,
                    icon: None,
                    lead_user_id: None,
                },
                None,
            )
            .await
            .expect("create project")
            .expect("ok");
            let task = create_task(
                &owner.pool,
                owner.workspace_id,
                project.id,
                owner.user_id,
                owner.session_id,
                CreateTaskInput {
                    title: "token revision task",
                    task_type: "task",
                    priority: "none",
                    status_id: None,
                    start_date: None,
                    due_date: None,
                    parent_id: None,
                    milestone_id: None,
                    recurrence: None,
                },
                None,
                "api",
            )
            .await
            .expect("create task")
            .expect("ok");
            let update = SeedEngine::from_hub(&hub)
                .tiptap_to_yjs_update(&json!({
                    "type": "doc",
                    "content": [{
                        "type": "paragraph",
                        "attrs": {"id": "rev-pat-1"},
                        "content": [{"type": "text", "text": "token revision body"}]
                    }]
                }))
                .await
                .expect("task seed update");
            let claim = fvoci_server::db::collab::claim_writer_and_load_kind(
                &owner.pool,
                CollabKind::Task,
                owner.workspace_id,
                owner.user_id,
                owner.session_id,
                task.id,
            )
            .await
            .unwrap()
            .unwrap();
            let appended = fvoci_server::db::collab::append_collab_update_kind(
                &owner.pool,
                CollabKind::Task,
                AppendCollabInput {
                    workspace_id: owner.workspace_id,
                    actor_user_id: owner.user_id,
                    session_id: owner.session_id,
                    document_id: task.id,
                    writer_generation: claim.writer_generation,
                    expected_tail_seq: claim.load.tail_seq,
                    op_id: Uuid::now_v7(),
                    payload: &update,
                    client_ip: None,
                },
            )
            .await
            .unwrap()
            .unwrap();
            assert!(matches!(appended, AppendCollabResult::Committed { .. }));
            let created = fvoci_server::db::api_tokens::create_api_token(
                &owner.pool,
                owner.workspace_id,
                owner.user_id,
                owner.session_id,
                fvoci_server::db::api_tokens::CreateApiTokenInput {
                    name: "revision writer",
                    scopes: &[
                        fvoci_server::auth::scopes::ApiTokenScope::TasksRead,
                        fvoci_server::auth::scopes::ApiTokenScope::TasksWrite,
                    ],
                    unlimited: false,
                    service: false,
                },
                None,
            )
            .await
            .unwrap()
            .expect("token");
            let admin = admin_pool_of(&run.harness).await;
            let before = count_target_revisions(&admin, owner.workspace_id, task.id).await;

            let (mut barrier, blocker_pid) = hold_users_row(&admin, owner.user_id).await;
            let url = format!(
                "http://{addr}/api/v1/workspaces/{}/tasks/{}/revisions",
                owner.workspace_id, task.id
            );
            let secret = created.token.clone();
            let post = tokio::spawn(async move {
                let response = reqwest::Client::new()
                    .post(url)
                    .header("origin", PUBLIC_ORIGIN)
                    .header("authorization", format!("Bearer {secret}"))
                    .send()
                    .await
                    .expect("http");
                let status = response.status();
                (
                    status,
                    response.json::<Value>().await.unwrap_or(Value::Null),
                )
            });
            wait_for_revision_write_blocked(&admin, blocker_pid, "%fvoci.users%FOR UPDATE%").await;
            sqlx::query("UPDATE fvoci.users SET suspended_at = now() WHERE id = $1")
                .bind(owner.user_id)
                .execute(&mut *barrier)
                .await
                .unwrap();
            barrier.commit().await.unwrap();

            let (status, body) = tokio::time::timeout(Duration::from_secs(10), post)
                .await
                .expect("post finished")
                .expect("join");
            assert_eq!(status, reqwest::StatusCode::NOT_FOUND, "{body}");
            assert_eq!(
                count_target_revisions(&admin, owner.workspace_id, task.id).await,
                before
            );
            admin.close().await;
            run.finish().await.expect("cleanup");
        },
    )
    .await;
}

/// A member removal that holds the member's membership lock while a manual
/// revision POST starts is serialized before the write: the POST waits, then
/// sees the removal (404, no revision row).
#[tokio::test]
async fn manual_revision_refuses_member_removed_while_waiting() {
    run_test(
        "manual_revision_refuses_member_removed_while_waiting",
        async {
            let mut run = TestRun::new(support::TestDb::bootstrap().await);
            let wiki = setup_wiki_doc(&run.harness).await;
            let workspace_id = wiki.session.workspace_id;
            let member =
                create_member_session(&run.harness, workspace_id, "rev-member@example.com").await;
            let (state, hub) =
                collab_app_state(&run.harness.app_url, test_collab_config(4, 60_000)).await;
            let addr = run.spawn_router_state(state, hub).await;
            append_outside_room(&wiki.session, wiki.document_id, "structured.v1").await;
            let admin = admin_pool_of(&run.harness).await;
            let before = count_target_revisions(&admin, workspace_id, wiki.document_id).await;

            let mut barrier = admin.begin().await.unwrap();
            sqlx::query("SELECT pg_advisory_xact_lock($1, $2)")
                .bind(fvoci_server::db::context::MEMBERSHIP_LOCK_NAMESPACE)
                .bind(fvoci_server::db::context::lock_key_from_uuid(
                    member.user_id,
                ))
                .execute(&mut *barrier)
                .await
                .unwrap();
            let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *barrier)
                .await
                .unwrap();
            let path = revision_path(&wiki, "");
            let token = member.session_token.clone();
            let post = tokio::spawn(async move {
                http_json(addr, reqwest::Method::POST, &path, &token, None).await
            });
            wait_for_revision_write_blocked(&admin, blocker_pid, "%pg_advisory_xact_lock%").await;
            sqlx::query("DELETE FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2")
                .bind(workspace_id)
                .bind(member.user_id)
                .execute(&mut *barrier)
                .await
                .unwrap();
            barrier.commit().await.unwrap();

            let (status, body) = tokio::time::timeout(Duration::from_secs(10), post)
                .await
                .expect("post finished")
                .expect("join");
            assert_eq!(status, reqwest::StatusCode::NOT_FOUND, "{body}");
            assert_eq!(
                count_target_revisions(&admin, workspace_id, wiki.document_id).await,
                before
            );
            member.pool.close().await;
            admin.close().await;
            run.finish().await.expect("cleanup");
        },
    )
    .await;
}
