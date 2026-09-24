#![cfg(feature = "db-tests")]

mod support;

use std::net::SocketAddr;
use std::panic::{resume_unwind, AssertUnwindSafe};
use std::time::Duration;

use crate::support::{
    auth_and_join, complete_sync_handshake, connect_member, engine_fixture, setup_wiki_doc,
    sync_update_frame, test_collab_config, wait_for_ws_close_code, TestDb, TestRun,
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

async fn http_create_project(addr: SocketAddr, session_token: &str, workspace_id: Uuid) -> Uuid {
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
    Uuid::parse_str(body["rootDocumentId"].as_str().unwrap()).unwrap()
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
async fn document_move_into_project_closes_collab_room() {
    run_collab_test("document_move_into_project_closes_collab_room", |run| {
        Box::pin(async move {
            let wiki = setup_wiki_doc(&run.harness).await;
            let app_url = run.harness.app_url.clone();
            let addr = run.spawn_router(&app_url, collab_config_fast_acl()).await;
            let routing_key = routing_key(wiki.session.workspace_id, wiki.document_id);
            let project_root =
                http_create_project(addr, &wiki.session.session_token, wiki.session.workspace_id)
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

            let within = Duration::from_millis(ACL_POLL_MS + 400);
            wait_for_ws_close_code(&mut writer, 1008, within, false, Some("permission revoked"))
                .await;
        })
    })
    .await;
}
