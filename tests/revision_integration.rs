#![cfg(feature = "db-tests")]

#[allow(dead_code)]
mod support;

use std::time::Duration;

use chrono::{Duration as ChronoDuration, Utc};
use futures_util::SinkExt;
use fvoci_server::auth::password::Keyring;
use fvoci_server::auth::token::new_token;
use fvoci_server::collab::room::arm_append_revoke_barrier;
use fvoci_server::collab::wire::{CollabKind, CollabRoomName};
use fvoci_server::db::pool;
use fvoci_server::db::workspace::{self, WorkspaceRole};
use serde_json::Value;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use support::{
    auth_and_join, collab_app_state, complete_sync_handshake, connect_member, engine_fixture,
    setup_wiki_doc, stateless_frame, sync_update_frame, test_collab_config,
    wait_for_stateless_exact, wait_for_sync_applied, wait_for_sync_update, SessionFixture, TestRun,
    WikiDocFixture, PEPPER, PUBLIC_ORIGIN,
};
use tokio_tungstenite::tungstenite::Message;
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

async fn count_updates(pool: &PgPool, workspace_id: Uuid, document_id: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM fvoci.document_collab_updates WHERE workspace_id = $1 AND document_id = $2",
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_one(pool)
    .await
    .unwrap()
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

        let (status, restored) = http_json(
            addr,
            reqwest::Method::POST,
            &revision_path(&wiki, &format!("/{revision_id}/restore")),
            &wiki.session.session_token,
            Some(serde_json::json!({})),
        )
        .await;
        assert_eq!(status, reqwest::StatusCode::OK, "{restored}");
        assert_eq!(restored["restored"], true);

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

        let (status, restored) = http_json(
            addr,
            reqwest::Method::POST,
            &revision_path(&wiki, &format!("/{revision_id}/restore")),
            &wiki.session.session_token,
            Some(serde_json::json!({})),
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
                    Some(serde_json::json!({})),
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
                        Some(serde_json::json!({})),
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
        let (status, restored) = http_json(
            addr,
            reqwest::Method::POST,
            &revision_path(&wiki, &format!("/{revision_id}/restore")),
            &wiki.session.session_token,
            Some(serde_json::json!({})),
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
                        Some(serde_json::json!({})),
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
        wait_session_revision_count(
            &run.harness,
            wiki.session.workspace_id,
            wiki.document_id,
            0,
        )
        .await;

        let _ = ws1.close(None).await;
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
            count_session_revisions(
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
            count_session_revisions(
                &run.harness,
                wiki.session.workspace_id,
                wiki.document_id,
            )
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
    run_test("session_revision_y_snapshot_matches_durable_db_collab", async {
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

        let durable = fvoci_server::db::revisions::load_durable_collab_for_system(
            &wiki.session.pool,
            wiki.session.workspace_id,
            fvoci_server::db::revisions::RevisionTarget::Document(wiki.document_id),
        )
        .await
        .unwrap()
        .unwrap();
        let cfg = test_collab_config(4, 60_000);
        let expected = fvoci_server::collab::revision::capture_revision_offline(
            cfg.engine_bin,
            cfg.limits,
            durable.snapshot,
            durable.tail,
        )
        .expect("offline capture of durable DB collab");

        let admin = PgPoolOptions::new()
            .max_connections(2)
            .connect(&run.harness.admin_url)
            .await
            .unwrap();
        let row: (Vec<u8>,) = sqlx::query_as(
            r#"
            SELECT y_snapshot FROM fvoci.revisions
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
            row.0,
            expected.y_snapshot,
            "session capture must match durable DB collab, not optimistic in-memory tail"
        );
        run.finish().await.expect("cleanup");
    })
    .await;
}

#[tokio::test]
async fn session_revision_after_rejected_update_uses_committed_snapshot() {
    run_test("session_revision_after_rejected_update_uses_committed_snapshot", async {
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
        let row: (String,) = sqlx::query_as(
            r#"
            SELECT text FROM fvoci.revisions
            WHERE workspace_id = $1 AND target_id = $2 AND reason = 'session'
            "#,
        )
        .bind(wiki.session.workspace_id)
        .bind(wiki.document_id)
        .fetch_one(&admin)
        .await
        .unwrap();
        admin.close().await;
        assert!(
            row.0.contains("안녕 본문"),
            "session snapshot must reflect durable committed content, not rejected apply: {}",
            row.0
        );
        run.finish().await.expect("cleanup");
    })
    .await;
}

#[tokio::test]
async fn session_revision_skips_when_writer_generation_stale() {
    run_test("session_revision_skips_when_writer_generation_stale", async {
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
    })
    .await;
}

#[tokio::test]
async fn session_revision_skips_when_document_trashed_under_lock() {
    run_test("session_revision_skips_when_document_trashed_under_lock", async {
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
            count_session_revisions(
                &run.harness,
                wiki.session.workspace_id,
                wiki.document_id,
            )
            .await,
            0,
            "trashed document must not get a new session revision row"
        );
        admin.close().await;
        run.finish().await.expect("cleanup");
    })
    .await;
}
