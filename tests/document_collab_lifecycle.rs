#![cfg(feature = "db-tests")]

mod support;

use std::net::SocketAddr;
use std::panic::{resume_unwind, AssertUnwindSafe};
use std::time::Duration;

use crate::support::{
    auth_and_join, complete_sync_handshake, connect_member, engine_fixture, setup_wiki_doc,
    sync_update_frame, test_collab_config, wait_for_sync_applied, wait_for_ws_close_code, TestDb,
    TestRun,
};
use futures_util::future::BoxFuture;
use futures_util::{FutureExt, SinkExt, StreamExt};
use fvoci_server::collab::config::CollabConfig;
use fvoci_server::collab::room::{
    arm_append_in_tx_reject_barrier, disarm_append_in_tx_reject_barrier,
};
use fvoci_server::collab::wire::{
    CollabKind, CollabRoomName, DocumentMessage, SyncMessage, SyncStep, WireFrame,
};
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

const COLLAB_TEST_TIMEOUT: Duration = Duration::from_secs(30);
const ACL_POLL_MS: u64 = 500;

fn routing_key(workspace_id: Uuid, document_id: Uuid) -> String {
    CollabRoomName {
        workspace_id,
        kind: CollabKind::Document,
        resource_id: document_id,
    }
    .routing_key()
}

fn sample_update() -> Vec<u8> {
    engine_fixture("utf8_korean.v1")
}

fn collab_config_fast_acl() -> CollabConfig {
    let mut cfg = test_collab_config(4, 30_000);
    cfg.revoke_poll_ms = ACL_POLL_MS;
    cfg
}

fn collab_config_slow_acl() -> CollabConfig {
    let mut cfg = test_collab_config(4, 30_000);
    cfg.revoke_poll_ms = 60_000;
    cfg
}

async fn wait_for_trash_append_rejection(
    writer: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    peer: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    within: Duration,
) {
    let deadline = tokio::time::Instant::now() + within;
    let mut saw_applied_false = false; // diagnostic only; delivery auth may drop it after trash
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let slice = remaining.min(Duration::from_millis(100));
        tokio::select! {
            biased;
            msg = tokio::time::timeout(slice, writer.next()) => {
                match msg {
                    Ok(Some(Ok(Message::Close(Some(frame))))) => {
                        assert_eq!(frame.code, 1008u16.into());
                        // The explicit rejection close is the writer-visible outcome; the
                        // durable outcome (no new row) is asserted by the caller. A
                        // SyncStatus ack after trash may be withheld by delivery auth.
                        assert!(
                            matches!(frame.reason.as_str(), "update rejected" | "permission revoked"),
                            "unexpected close reason {:?}",
                            frame.reason.as_str()
                        );
                        return;
                    }
                    Ok(Some(Ok(Message::Binary(bytes)))) => {
                        match fvoci_server::collab::wire::decode(&bytes) {
                            Ok(WireFrame::Document {
                                message: DocumentMessage::SyncStatus { applied: false },
                                ..
                            }) => saw_applied_false = true,
                            Ok(WireFrame::Document {
                                message: DocumentMessage::SyncStatus { applied: true },
                                ..
                            }) => panic!("append after trash must never be acknowledged as applied"),
                            _ => {}
                        }
                    }
                    Ok(Some(Ok(_))) | Ok(None) | Ok(Some(Err(_))) | Err(_) => {}
                }
            }
            msg = tokio::time::timeout(slice, peer.next()) => {
                if let Ok(Some(Ok(Message::Binary(bytes)))) = msg {
                    if matches!(
                        fvoci_server::collab::wire::decode(&bytes),
                        Ok(WireFrame::Document {
                            message: DocumentMessage::Sync(SyncMessage {
                                step: SyncStep::Update,
                                ..
                            }),
                            ..
                        })
                    ) {
                        panic!("trashed append rejection must not broadcast Sync Update");
                    }
                }
            }
        }
    }
    panic!(
        "timed out waiting for trash append rejection close; saw_applied_false={saw_applied_false}"
    );
}

async fn run_collab_test<F>(name: &str, case: F)
where
    F: for<'a> FnOnce(&'a mut TestRun) -> BoxFuture<'a, ()>,
{
    let mut run = TestRun::new(TestDb::bootstrap().await);
    let case_fut = case(&mut run);
    let case_outcome = tokio::time::timeout(
        COLLAB_TEST_TIMEOUT,
        AssertUnwindSafe(case_fut).catch_unwind(),
    )
    .await;
    let cleanup_outcome = run.finish().await;
    match (case_outcome, cleanup_outcome) {
        (Ok(Ok(())), Ok(())) => {}
        (Ok(Ok(())), Err(cleanup_err)) => {
            panic!("{name} cleanup failed after success: {cleanup_err}");
        }
        (Ok(Err(panic_payload)), cleanup) => {
            if let Err(cleanup_err) = cleanup {
                eprintln!("{name} cleanup also failed: {cleanup_err}");
            }
            resume_unwind(panic_payload);
        }
        (Err(_elapsed), Ok(())) => {
            panic!("{name} hung (>{COLLAB_TEST_TIMEOUT:?}); cleanup completed");
        }
        (Err(_elapsed), Err(cleanup_err)) => {
            panic!("{name} hung (>{COLLAB_TEST_TIMEOUT:?}); cleanup error: {cleanup_err}");
        }
    }
}

async fn http_trash(addr: SocketAddr, session_token: &str, workspace_id: Uuid, document_id: Uuid) {
    let client = reqwest::Client::new();
    let url =
        format!("http://{addr}/api/v1/workspaces/{workspace_id}/documents/{document_id}/trash");
    let resp = client
        .post(url)
        .header("cookie", format!("fvoci_session={session_token}"))
        .send()
        .await
        .expect("trash request");
    assert_eq!(
        resp.status(),
        200,
        "trash: {}",
        resp.text().await.unwrap_or_default()
    );
}

async fn http_move(
    addr: SocketAddr,
    session_token: &str,
    workspace_id: Uuid,
    document_id: Uuid,
    new_parent_id: Uuid,
) {
    let client = reqwest::Client::new();
    let url =
        format!("http://{addr}/api/v1/workspaces/{workspace_id}/documents/{document_id}/move");
    let resp = client
        .post(url)
        .header("cookie", format!("fvoci_session={session_token}"))
        .json(&json!({"newParentId": new_parent_id.to_string()}))
        .send()
        .await
        .expect("move request");
    assert_eq!(
        resp.status(),
        200,
        "move: {}",
        resp.text().await.unwrap_or_default()
    );
}

async fn http_create_project(
    addr: SocketAddr,
    session_token: &str,
    workspace_id: Uuid,
) -> (Uuid, Uuid) {
    let client = reqwest::Client::new();
    let url = format!("http://{addr}/api/v1/workspaces/{workspace_id}/projects");
    let resp = client
        .post(url)
        .header("cookie", format!("fvoci_session={session_token}"))
        .json(&json!({"key": "LAB", "name": "Lab", "visibility": "workspace"}))
        .send()
        .await
        .expect("create project");
    assert_eq!(resp.status(), 201);
    let body: serde_json::Value = resp.json().await.expect("project json");
    (
        Uuid::parse_str(body["id"].as_str().unwrap()).unwrap(),
        Uuid::parse_str(body["rootDocumentId"].as_str().unwrap()).unwrap(),
    )
}

#[tokio::test]
async fn document_trash_rejects_collab_append_after_commit() {
    run_collab_test("document_trash_rejects_collab_append_after_commit", |run| {
        Box::pin(async move {
            let wiki = setup_wiki_doc(&run.harness).await;
            let app_url = run.harness.app_url.clone();
            let addr = run.spawn_router(&app_url, collab_config_slow_acl()).await;
            let routing_key = routing_key(wiki.session.workspace_id, wiki.document_id);

            let admin = PgPoolOptions::new()
                .max_connections(2)
                .connect(&run.harness.admin_url)
                .await
                .unwrap();
            let updates_before: (i64,) = sqlx::query_as(
                "SELECT count(*) FROM fvoci.document_collab_updates WHERE document_id = $1",
            )
            .bind(wiki.document_id)
            .fetch_one(&admin)
            .await
            .unwrap();

            let mut writer = connect_member(addr, &wiki.session.session_token).await;
            auth_and_join(&mut writer, &routing_key, 101).await;
            complete_sync_handshake(&mut writer, &routing_key).await;

            let mut peer = connect_member(addr, &wiki.session.session_token).await;
            auth_and_join(&mut peer, &routing_key, 102).await;
            complete_sync_handshake(&mut peer, &routing_key).await;

            let update = sample_update();
            let (reached_rx, proceed_tx) = arm_append_in_tx_reject_barrier(wiki.document_id).await;
            writer
                .send(Message::Binary(
                    sync_update_frame(&routing_key, &update).into(),
                ))
                .await
                .unwrap();
            tokio::time::timeout(Duration::from_secs(5), reached_rx)
                .await
                .expect("append in-tx barrier must be reached after admission")
                .expect("barrier signal");

            http_trash(
                addr,
                &wiki.session.session_token,
                wiki.session.workspace_id,
                wiki.document_id,
            )
            .await;

            proceed_tx.send(()).expect("release append in-tx barrier");
            disarm_append_in_tx_reject_barrier(wiki.document_id).await;

            wait_for_trash_append_rejection(&mut writer, &mut peer, Duration::from_secs(5)).await;

            let updates_after: (i64,) = sqlx::query_as(
                "SELECT count(*) FROM fvoci.document_collab_updates WHERE document_id = $1",
            )
            .bind(wiki.document_id)
            .fetch_one(&admin)
            .await
            .unwrap();
            assert_eq!(
                updates_before.0, updates_after.0,
                "trashed append race must not persist collab updates"
            );
            admin.close().await;
        })
    })
    .await;
}

#[tokio::test]
async fn document_trash_closes_two_collab_sockets_within_acl_poll() {
    run_collab_test(
        "document_trash_closes_two_collab_sockets_within_acl_poll",
        |run| {
            Box::pin(async move {
                let wiki = setup_wiki_doc(&run.harness).await;
                let app_url = run.harness.app_url.clone();
                let addr = run.spawn_router(&app_url, collab_config_fast_acl()).await;
                let routing_key = routing_key(wiki.session.workspace_id, wiki.document_id);

                let mut peer_a = connect_member(addr, &wiki.session.session_token).await;
                auth_and_join(&mut peer_a, &routing_key, 201).await;
                complete_sync_handshake(&mut peer_a, &routing_key).await;

                let mut peer_b = connect_member(addr, &wiki.session.session_token).await;
                auth_and_join(&mut peer_b, &routing_key, 202).await;
                complete_sync_handshake(&mut peer_b, &routing_key).await;

                http_trash(
                    addr,
                    &wiki.session.session_token,
                    wiki.session.workspace_id,
                    wiki.document_id,
                )
                .await;

                let within = Duration::from_millis(ACL_POLL_MS + 400);
                wait_for_ws_close_code(
                    &mut peer_a,
                    1008,
                    within,
                    false,
                    Some("permission revoked"),
                )
                .await;
                wait_for_ws_close_code(
                    &mut peer_b,
                    1008,
                    within,
                    false,
                    Some("permission revoked"),
                )
                .await;
            })
        },
    )
    .await;
}

#[tokio::test]
async fn document_move_into_project_keeps_room_until_project_access_is_revoked() {
    run_collab_test(
        "document_move_into_project_keeps_room_until_project_access_is_revoked",
        |run| {
            Box::pin(async move {
                let wiki = setup_wiki_doc(&run.harness).await;
                let app_url = run.harness.app_url.clone();
                let addr = run.spawn_router(&app_url, collab_config_fast_acl()).await;
                let routing_key = routing_key(wiki.session.workspace_id, wiki.document_id);
                let (project_id, project_root) = http_create_project(
                    addr,
                    &wiki.session.session_token,
                    wiki.session.workspace_id,
                )
                .await;

                let mut writer = connect_member(addr, &wiki.session.session_token).await;
                auth_and_join(&mut writer, &routing_key, 301).await;
                complete_sync_handshake(&mut writer, &routing_key).await;

                http_move(
                    addr,
                    &wiki.session.session_token,
                    wiki.session.workspace_id,
                    wiki.document_id,
                    project_root,
                )
                .await;

                // Project documents share the collab room under project
                // permission: the writer keeps editing after the ACL poll.
                tokio::time::sleep(Duration::from_millis(ACL_POLL_MS + 400)).await;
                writer
                    .send(Message::Binary(
                        sync_update_frame(&routing_key, &engine_fixture("utf8_korean.v1")).into(),
                    ))
                    .await
                    .unwrap();
                assert!(
                    wait_for_sync_applied(&mut writer, Duration::from_secs(5)).await,
                    "edit must apply after the move into a project the writer can edit"
                );

                // Losing project access (private, no grant) revokes the room.
                let admin = PgPoolOptions::new()
                    .max_connections(1)
                    .connect(&run.harness.admin_url)
                    .await
                    .unwrap();
                // Another member becomes lead so the private-lead invariant holds.
                let other = Uuid::now_v7();
                sqlx::query("INSERT INTO fvoci.users (id, email, given_name) VALUES ($1, $2, 'Lead')")
                    .bind(other)
                    .bind(format!("lead-{other}@example.com"))
                    .execute(&admin)
                    .await
                    .unwrap();
                sqlx::query(
                    "INSERT INTO fvoci.memberships (workspace_id, user_id, role) VALUES ($1, $2, 'member')",
                )
                .bind(wiki.session.workspace_id)
                .bind(other)
                .execute(&admin)
                .await
                .unwrap();
                sqlx::query(
                    "INSERT INTO fvoci.project_members (id, workspace_id, project_id, user_id, role) VALUES (gen_random_uuid(), $1, $2, $3, 'lead')",
                )
                .bind(wiki.session.workspace_id)
                .bind(project_id)
                .bind(other)
                .execute(&admin)
                .await
                .unwrap();
                sqlx::query("UPDATE fvoci.projects SET visibility = 'private' WHERE id = $1")
                    .bind(project_id)
                    .execute(&admin)
                    .await
                    .unwrap();
                sqlx::query(
                    "DELETE FROM fvoci.project_members WHERE project_id = $1 AND user_id = $2",
                )
                .bind(project_id)
                .bind(wiki.session.user_id)
                .execute(&admin)
                .await
                .unwrap();
                admin.close().await;
                let within = Duration::from_millis(ACL_POLL_MS + 400);
                wait_for_ws_close_code(
                    &mut writer,
                    1008,
                    within,
                    false,
                    Some("permission revoked"),
                )
                .await;
            })
        },
    )
    .await;
}

fn project_revisions_url(workspace_id: Uuid, project_id: &str, document_id: &str) -> String {
    format!(
        "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/revisions"
    )
}

async fn collab_http_json(
    addr: std::net::SocketAddr,
    method: reqwest::Method,
    path: &str,
    token: &str,
    body: Option<serde_json::Value>,
) -> (reqwest::StatusCode, serde_json::Value) {
    let mut req = reqwest::Client::new()
        .request(method, format!("http://{addr}{path}"))
        .header("origin", crate::support::PUBLIC_ORIGIN)
        .header("cookie", format!("fvoci_session={token}"));
    if let Some(body) = body {
        req = req.json(&body);
    }
    let response = req.send().await.expect("http");
    let status = response.status();
    (
        status,
        response.json().await.unwrap_or(serde_json::Value::Null),
    )
}

async fn collab_apply_and_persist(
    addr: std::net::SocketAddr,
    token: &str,
    key: &str,
    client_id: u32,
    update: &[u8],
) {
    let mut ws = crate::support::connect_member(addr, token).await;
    crate::support::auth_and_join(&mut ws, key, client_id).await;
    crate::support::complete_sync_handshake(&mut ws, key).await;
    ws.send(Message::Binary(
        crate::support::sync_update_frame(key, update).into(),
    ))
    .await
    .unwrap();
    assert!(
        crate::support::wait_for_sync_applied(&mut ws, Duration::from_secs(8)).await,
        "update must apply"
    );
    let request_id = Uuid::now_v7();
    ws.send(Message::Binary(
        crate::support::stateless_frame(key, &format!("persist:{request_id}")).into(),
    ))
    .await
    .unwrap();
    assert!(
        crate::support::wait_for_stateless_exact(
            &mut ws,
            &format!("persisted:{request_id}"),
            Duration::from_secs(8)
        )
        .await,
        "persist ack"
    );
    let _ = ws.close(None).await;
}

/// Durable collab write sequence of a document (advances on every appended update).
async fn document_tail_seq(pool: &sqlx::PgPool, workspace_id: Uuid, document_id: Uuid) -> i64 {
    let mut tx = pool.begin().await.unwrap();
    fvoci_server::db::context::set_tenant(&mut tx, workspace_id)
        .await
        .unwrap();
    let seq = sqlx::query_scalar(
        "SELECT tail_seq FROM fvoci.document_states WHERE workspace_id = $1 AND document_id = $2",
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    seq
}

/// Project document revision create captures real collab state and restore
/// goes through the room actor as a durable forward update; an archived
/// project refuses restore without appending.
#[tokio::test]
async fn project_document_revision_create_and_forward_restore() {
    tokio::time::timeout(Duration::from_secs(60), async {
        let mut run = crate::support::TestRun::new(crate::support::TestDb::bootstrap().await);
        let owner = crate::support::setup_owner_session(&run.harness).await;
        let project = fvoci_server::db::projects::create_project(
            &owner.pool,
            owner.workspace_id,
            owner.user_id,
            owner.session_id,
            fvoci_server::db::projects::CreateProjectInput {
                key: "PREV",
                name: "Project revisions",
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
        let document_id = project.root_document_id.expect("project root document");
        let workspace_id = owner.workspace_id;
        let (state, hub) = crate::support::collab_app_state(
            &run.harness.app_url,
            crate::support::test_collab_config(4, 60_000),
        )
        .await;
        let addr = run.spawn_router_state(state, hub).await;
        let key = routing_key(workspace_id, document_id);
        let token = owner.session_token.clone();
        collab_apply_and_persist(
            addr,
            &token,
            &key,
            1,
            &crate::support::engine_fixture("structured.v1"),
        )
        .await;

        let base = project_revisions_url(
            workspace_id,
            &project.id.to_string(),
            &document_id.to_string(),
        );
        let wiki = format!("/api/v1/workspaces/{workspace_id}/documents/{document_id}/revisions");
        let (status, _) = collab_http_json(addr, reqwest::Method::POST, &wiki, &token, None).await;
        assert_eq!(status, reqwest::StatusCode::NOT_FOUND);

        let (status, created) =
            collab_http_json(addr, reqwest::Method::POST, &base, &token, None).await;
        assert_eq!(status, reqwest::StatusCode::CREATED, "{created}");
        let revision_id = created["id"].as_str().unwrap().to_string();
        let (status, original) = collab_http_json(
            addr,
            reqwest::Method::GET,
            &format!("{base}/{revision_id}"),
            &token,
            None,
        )
        .await;
        assert_eq!(status, reqwest::StatusCode::OK, "{original}");

        collab_apply_and_persist(
            addr,
            &token,
            &key,
            3,
            &crate::support::engine_fixture("followup_edit.v1"),
        )
        .await;
        let (status, edited) =
            collab_http_json(addr, reqwest::Method::POST, &base, &token, None).await;
        assert_eq!(status, reqwest::StatusCode::CREATED, "{edited}");
        let (_, edited) = collab_http_json(
            addr,
            reqwest::Method::GET,
            &format!("{base}/{}", edited["id"].as_str().unwrap()),
            &token,
            None,
        )
        .await;
        assert_ne!(edited["contentJson"], original["contentJson"]);

        let before = document_tail_seq(&owner.pool, workspace_id, document_id).await;
        let (preview_status, preview) = collab_http_json(addr, reqwest::Method::GET,
            &format!("{base}/{revision_id}/restore-preview"), &token, None).await;
        assert_eq!(preview_status, reqwest::StatusCode::OK, "{preview}");
        let restore_body = serde_json::json!({"correlationId": Uuid::now_v7(), "expectedTailSeq": preview["currentTailSeq"]});
        let (status, restored) = collab_http_json(
            addr,
            reqwest::Method::POST,
            &format!("{base}/{revision_id}/restore"),
            &token,
            Some(restore_body.clone()),
        )
        .await;
        assert_eq!(status, reqwest::StatusCode::OK, "{restored}");
        assert_eq!(restored["restored"], true);
        let after = document_tail_seq(&owner.pool, workspace_id, document_id).await;
        assert!(
            after > before,
            "restore must append a durable forward update"
        );

        let (status, current) =
            collab_http_json(addr, reqwest::Method::POST, &base, &token, None).await;
        assert_eq!(status, reqwest::StatusCode::CREATED, "{current}");
        let (_, current) = collab_http_json(
            addr,
            reqwest::Method::GET,
            &format!("{base}/{}", current["id"].as_str().unwrap()),
            &token,
            None,
        )
        .await;
        assert_eq!(current["contentJson"], original["contentJson"]);

        let admin = sqlx::PgPool::connect(&run.harness.admin_url).await.unwrap();
        sqlx::query(
            "UPDATE fvoci.projects SET status = 'archived' WHERE workspace_id = $1 AND id = $2",
        )
        .bind(workspace_id)
        .bind(project.id)
        .execute(&admin)
        .await
        .unwrap();
        admin.close().await;
        let before = document_tail_seq(&owner.pool, workspace_id, document_id).await;
        let (status, body) = collab_http_json(
            addr,
            reqwest::Method::POST,
            &format!("{base}/{revision_id}/restore"),
            &token,
            Some(restore_body.clone()),
        )
        .await;
        assert_eq!(status, reqwest::StatusCode::CONFLICT, "{body}");
        assert_eq!(body["code"], "project_archived");
        assert_eq!(
            document_tail_seq(&owner.pool, workspace_id, document_id).await,
            before
        );
        run.finish().await.expect("cleanup");
    })
    .await
    .expect("project_document_revision_create_and_forward_restore hung");
}
