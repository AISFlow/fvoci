#![cfg(feature = "db-tests")]

mod support;

use std::net::SocketAddr;
use std::panic::{resume_unwind, AssertUnwindSafe};
use std::time::Duration;

use futures_util::future::BoxFuture;
use futures_util::{FutureExt, SinkExt};
use fvoci_server::collab::room::{
    arm_append_projection_barrier, arm_force_primary_apply_fail, arm_force_primary_load_fail,
    disarm_append_projection_barrier, disarm_force_primary_apply_fail,
    disarm_force_primary_load_fail,
};
use fvoci_server::collab::wire::{CollabKind, CollabRoomName, DocumentMessage, WireFrame};
use fvoci_server::db::collab::claim_writer_and_load;
use fvoci_server::db::documents::empty_document_json;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use support::{
    auth_and_join, complete_sync_handshake, connect_member, delete_only_base_update,
    delete_only_json_after, delete_only_json_before, delete_only_update, engine_fixture,
    expectations, get_document_body, join_denied, persist_barrier, recv_document_frame,
    setup_wiki_doc, stateless_frame, sync_step1_frame, sync_update_frame, test_collab_config,
    tiny_output_project_collab_config, wait_for_committed_update_then_close,
    wait_for_stateless_exact, wait_for_sync_applied, wait_for_sync_update, wait_for_ws_close_code,
    wait_for_writer_close_without_peer_update, TestDb, TestRun, WikiDocFixture,
};
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

const TEST_TIMEOUT: Duration = Duration::from_secs(30);

async fn run_test<F>(name: &str, case: F)
where
    F: for<'a> FnOnce(&'a mut TestRun) -> BoxFuture<'a, ()>,
{
    let mut run = TestRun::new(TestDb::bootstrap().await);
    let case_fut = case(&mut run);
    let case_outcome =
        tokio::time::timeout(TEST_TIMEOUT, AssertUnwindSafe(case_fut).catch_unwind()).await;
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
            panic!("{name} case hung (>{TEST_TIMEOUT:?}); cleanup completed");
        }
        (Err(_elapsed), Err(cleanup_err)) => {
            panic!("{name} hung (>{TEST_TIMEOUT:?}); cleanup error: {cleanup_err}");
        }
    }
}

async fn install_derived_document_updated_fail_trigger(admin: &PgPool, fn_name: &str) {
    sqlx::query(&format!(
        r#"
        CREATE OR REPLACE FUNCTION fvoci.{fn_name}()
        RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN
            IF NEW.verb = 'document.updated'
               AND NEW.channel = 'system'
               AND NEW.actor_user_id IS NULL THEN
                RAISE EXCEPTION 'derived document.updated blocked';
            END IF;
            RETURN NEW;
        END;
        $$;
        "#
    ))
    .execute(admin)
    .await
    .unwrap();
    sqlx::query(&format!(
        r#"
        CREATE TRIGGER fvoci_{fn_name}
        BEFORE INSERT ON fvoci.events
        FOR EACH ROW EXECUTE FUNCTION fvoci.{fn_name}()
        "#
    ))
    .execute(admin)
    .await
    .unwrap();
}

async fn event_count(admin: &PgPool, document_id: Uuid, verb: &str) -> i64 {
    let (count,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM fvoci.events WHERE target_id = $1 AND verb = $2")
            .bind(document_id)
            .bind(verb)
            .fetch_one(admin)
            .await
            .unwrap();
    count
}

async fn document_updated_event_count(admin: &PgPool, document_id: Uuid) -> i64 {
    event_count(admin, document_id, "document.updated").await
}

fn room_key(workspace_id: Uuid, document_id: Uuid) -> String {
    CollabRoomName {
        workspace_id,
        kind: CollabKind::Document,
        resource_id: document_id,
    }
    .routing_key()
}

async fn await_persisted_get_body(
    addr: SocketAddr,
    wiki: &WikiDocFixture,
    client_id: u32,
) -> serde_json::Value {
    let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
    let request_id = Uuid::now_v7();
    persist_barrier(
        addr,
        &wiki.session.session_token,
        &routing_key,
        client_id,
        request_id,
    )
    .await;
    get_document_body(
        addr,
        &wiki.session.session_token,
        wiki.session.workspace_id,
        wiki.document_id,
    )
    .await
}

async fn send_collab_updates(addr: SocketAddr, wiki: &WikiDocFixture, payloads: &[&[u8]]) {
    let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
    let mut writer = connect_member(addr, &wiki.session.session_token).await;
    auth_and_join(&mut writer, &routing_key, 71).await;
    for payload in payloads {
        writer
            .send(Message::Binary(
                sync_update_frame(&routing_key, payload).into(),
            ))
            .await
            .unwrap();
        assert!(
            wait_for_sync_applied(&mut writer, Duration::from_secs(5)).await,
            "collab update must ack applied:true"
        );
    }
}

#[tokio::test]
async fn collab_edit_get_body_reflects_projection() {
    run_test("collab_edit_get_body_reflects_projection", |run| Box::pin(async {
        let app_url = run.harness.app_url.clone();
        let admin_url = run.harness.admin_url.clone();
        let wiki = setup_wiki_doc(&run.harness).await;
        let addr = run
            .spawn_router(&app_url, test_collab_config(4, 30_000))
            .await;
        send_collab_updates(addr, &wiki, &[&delete_only_base_update()]).await;

        let body = await_persisted_get_body(addr, &wiki, 72).await;
        assert_eq!(body["contentJson"], delete_only_json_before());
        assert_eq!(body["version"], 1);

        let admin = PgPoolOptions::new()
            .max_connections(2)
            .connect(&admin_url)
            .await
            .unwrap();
        assert_eq!(
            document_updated_event_count(&admin, wiki.document_id).await,
            1
        );
        let (channel,): (String,) = sqlx::query_as(
            "SELECT channel FROM fvoci.events WHERE target_id = $1 AND verb = 'document.updated' LIMIT 1",
        )
        .bind(wiki.document_id)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(channel, "system");
    }))
    .await;
}

#[tokio::test]
async fn collab_korean_edit_get_body_matches_fixture_oracle() {
    run_test(
        "collab_korean_edit_get_body_matches_fixture_oracle",
        |run| {
            Box::pin(async {
                let app_url = run.harness.app_url.clone();
                let admin_url = run.harness.admin_url.clone();
                let wiki = setup_wiki_doc(&run.harness).await;
                let addr = run
                    .spawn_router(&app_url, test_collab_config(4, 30_000))
                    .await;
                let base = engine_fixture("korean_emoji_base.v1");
                let mid = engine_fixture("korean_emoji_mid_edit.v1");
                let delete = engine_fixture("korean_emoji_delete.v1");
                send_collab_updates(addr, &wiki, &[&base, &mid, &delete]).await;

                let body = await_persisted_get_body(addr, &wiki, 73).await;
                assert_eq!(
                    body["contentJson"],
                    expectations()["korean_emoji"]["prosemirror_json"]
                );
                let (text, chosung): (String, String) =
                    sqlx::query_as("SELECT text, chosung FROM fvoci.documents WHERE id = $1")
                        .bind(wiki.document_id)
                        .fetch_one(
                            &PgPoolOptions::new()
                                .max_connections(2)
                                .connect(&admin_url)
                                .await
                                .unwrap(),
                        )
                        .await
                        .unwrap();
                assert_eq!(text, "가중🚀마바사");
                assert!(!chosung.is_empty());
            })
        },
    )
    .await;
}

#[tokio::test]
async fn collab_delete_only_get_body_updates_json() {
    run_test("collab_delete_only_get_body_updates_json", |run| {
        Box::pin(async {
            let app_url = run.harness.app_url.clone();
            let admin_url = run.harness.admin_url.clone();
            let wiki = setup_wiki_doc(&run.harness).await;
            let addr = run
                .spawn_router(&app_url, test_collab_config(4, 30_000))
                .await;
            let base = engine_fixture("delete_only_base.v1");
            let delete_only = engine_fixture("delete_only.v1");
            send_collab_updates(addr, &wiki, &[&base, &delete_only]).await;

            let body = await_persisted_get_body(addr, &wiki, 74).await;
            assert_eq!(
                body["contentJson"],
                expectations()["delete_only"]["prosemirror_json_after"]
            );

            let admin = PgPoolOptions::new()
                .max_connections(2)
                .connect(&admin_url)
                .await
                .unwrap();
            assert_eq!(
                document_updated_event_count(&admin, wiki.document_id).await,
                2
            );
        })
    })
    .await;
}

#[tokio::test]
async fn collab_seed_join_does_not_overwrite_paragraph() {
    run_test("collab_seed_join_does_not_overwrite_paragraph", |run| {
        Box::pin(async {
            let app_url = run.harness.app_url.clone();
            let admin_url = run.harness.admin_url.clone();
            let wiki = setup_wiki_doc(&run.harness).await;
            let addr = run
                .spawn_router(&app_url, test_collab_config(4, 30_000))
                .await;
            let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
            let mut writer = connect_member(addr, &wiki.session.session_token).await;
            auth_and_join(&mut writer, &routing_key, 80).await;

            let body = get_document_body(
                addr,
                &wiki.session.session_token,
                wiki.session.workspace_id,
                wiki.document_id,
            )
            .await;
            assert_eq!(body["contentJson"], empty_document_json());
            assert_eq!(body["version"], 1);

            let admin = PgPoolOptions::new()
                .max_connections(2)
                .connect(&admin_url)
                .await
                .unwrap();
            assert_eq!(
                document_updated_event_count(&admin, wiki.document_id).await,
                0
            );
        })
    })
    .await;
}

#[tokio::test]
async fn collab_primary_unhealthy_skips_projection_until_catch_up() {
    run_test(
        "collab_primary_unhealthy_skips_projection_until_catch_up",
        |run| {
            Box::pin(async {
                let app_url = run.harness.app_url.clone();
                let wiki = setup_wiki_doc(&run.harness).await;
                let addr = run
                    .spawn_router(&app_url, test_collab_config(4, 30_000))
                    .await;
                let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
                let update = delete_only_base_update();

                let mut reader = connect_member(addr, &wiki.session.session_token).await;
                auth_and_join(&mut reader, &routing_key, 90).await;
                let mut writer = connect_member(addr, &wiki.session.session_token).await;
                auth_and_join(&mut writer, &routing_key, 91).await;
                arm_force_primary_apply_fail(wiki.document_id).await;
                arm_force_primary_load_fail(wiki.document_id).await;
                writer
                    .send(Message::Binary(
                        sync_update_frame(&routing_key, &update).into(),
                    ))
                    .await
                    .unwrap();
                assert!(
                    wait_for_sync_applied(&mut writer, Duration::from_secs(5)).await,
                    "durable commit must still ack when primary reload fails"
                );
                wait_for_ws_close_code(
                    &mut writer,
                    1011,
                    Duration::from_secs(5),
                    true,
                    Some("primary engine unhealthy"),
                )
                .await;
                wait_for_ws_close_code(
                    &mut reader,
                    1011,
                    Duration::from_secs(5),
                    true,
                    Some("primary engine unhealthy"),
                )
                .await;

                let body_before = get_document_body(
                    addr,
                    &wiki.session.session_token,
                    wiki.session.workspace_id,
                    wiki.document_id,
                )
                .await;
                assert_eq!(body_before["contentJson"], empty_document_json());

                disarm_force_primary_load_fail(wiki.document_id).await;
                disarm_force_primary_apply_fail(wiki.document_id).await;

                let mut recovery = connect_member(addr, &wiki.session.session_token).await;
                auth_and_join(&mut recovery, &routing_key, 92).await;

                let body_after = await_persisted_get_body(addr, &wiki, 96).await;
                assert_eq!(body_after["contentJson"], delete_only_json_before());
            })
        },
    )
    .await;
}

#[tokio::test]
async fn collab_derived_event_failure_preserves_binary_then_retries() {
    run_test(
        "collab_derived_event_failure_preserves_binary_then_retries",
        |run| {
            Box::pin(async {
                let app_url = run.harness.app_url.clone();
                let admin_url = run.harness.admin_url.clone();
                let wiki = setup_wiki_doc(&run.harness).await;
                let admin = PgPoolOptions::new()
                    .max_connections(2)
                    .connect(&admin_url)
                    .await
                    .unwrap();
                install_derived_document_updated_fail_trigger(&admin, "test_projection_event_fail")
                    .await;

                let addr = run
                    .spawn_router(&app_url, test_collab_config(4, 30_000))
                    .await;
                let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
                let update = delete_only_base_update();

                let mut writer = connect_member(addr, &wiki.session.session_token).await;
                auth_and_join(&mut writer, &routing_key, 93).await;
                writer
                    .send(Message::Binary(
                        sync_update_frame(&routing_key, &update).into(),
                    ))
                    .await
                    .unwrap();
                assert!(wait_for_sync_applied(&mut writer, Duration::from_secs(5)).await);

                let mut second = connect_member(addr, &wiki.session.session_token).await;
                auth_and_join(&mut second, &routing_key, 98).await;
                complete_sync_handshake(&mut second, &routing_key).await;

                let body_stale = get_document_body(
                    addr,
                    &wiki.session.session_token,
                    wiki.session.workspace_id,
                    wiki.document_id,
                )
                .await;
                assert_eq!(body_stale["contentJson"], empty_document_json());
                assert_eq!(
                    document_updated_event_count(&admin, wiki.document_id).await,
                    0
                );
                assert_eq!(
                    event_count(&admin, wiki.document_id, "document.collab_update_appended",).await,
                    1,
                    "append audit event must commit while derived document.updated is blocked"
                );

                let load = fvoci_server::db::collab::load_collab_document(
                    &wiki.session.pool,
                    wiki.session.workspace_id,
                    wiki.session.user_id,
                    wiki.session.session_id,
                    wiki.document_id,
                )
                .await
                .unwrap()
                .unwrap();
                assert_eq!(load.tail.len(), 1);
                assert_eq!(load.tail[0].payload, update);

                sqlx::query("DROP TRIGGER fvoci_test_projection_event_fail ON fvoci.events")
                    .execute(&admin)
                    .await
                    .unwrap();

                let follow_up = delete_only_update();
                writer
                    .send(Message::Binary(
                        sync_update_frame(&routing_key, &follow_up).into(),
                    ))
                    .await
                    .unwrap();
                assert!(wait_for_sync_applied(&mut writer, Duration::from_secs(5)).await);

                let body_retry = await_persisted_get_body(addr, &wiki, 97).await;
                assert_eq!(body_retry["contentJson"], delete_only_json_after());
                assert!(document_updated_event_count(&admin, wiki.document_id).await >= 1);
            })
        },
    )
    .await;
}

#[tokio::test]
async fn collab_catch_up_after_server_restart_without_retransmit() {
    run_test(
        "collab_catch_up_after_server_restart_without_retransmit",
        |run| {
            Box::pin(async {
                let app_url = run.harness.app_url.clone();
                let admin_url = run.harness.admin_url.clone();
                let wiki = setup_wiki_doc(&run.harness).await;
                let admin = PgPoolOptions::new()
                    .max_connections(2)
                    .connect(&admin_url)
                    .await
                    .unwrap();
                install_derived_document_updated_fail_trigger(&admin, "test_restart_catchup").await;

                let cfg = test_collab_config(4, 30_000);
                let addr = run.spawn_router(&app_url, cfg.clone()).await;
                send_collab_updates(addr, &wiki, &[&delete_only_base_update()]).await;

                let stale = get_document_body(
                    addr,
                    &wiki.session.session_token,
                    wiki.session.workspace_id,
                    wiki.document_id,
                )
                .await;
                assert_eq!(stale["contentJson"], empty_document_json());
                assert_eq!(
                    document_updated_event_count(&admin, wiki.document_id).await,
                    0
                );

                sqlx::query("DROP TRIGGER fvoci_test_restart_catchup ON fvoci.events")
                    .execute(&admin)
                    .await
                    .unwrap();

                run.shutdown_last_server()
                    .await
                    .expect("restart test must shut down first server");
                let addr2 = run.spawn_router(&app_url, cfg).await;
                let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
                let mut writer = connect_member(addr2, &wiki.session.session_token).await;
                auth_and_join(&mut writer, &routing_key, 75).await;

                let body_after = get_document_body(
                    addr2,
                    &wiki.session.session_token,
                    wiki.session.workspace_id,
                    wiki.document_id,
                )
                .await;
                assert_eq!(body_after["contentJson"], delete_only_json_before());
                assert_eq!(body_after["version"], 1);
                assert_eq!(
                    document_updated_event_count(&admin, wiki.document_id).await,
                    1
                );
            })
        },
    )
    .await;
}

#[tokio::test]
async fn collab_archived_document_skips_new_projection() {
    run_test("collab_archived_document_skips_new_projection", |run| {
        Box::pin(async {
            let app_url = run.harness.app_url.clone();
            let admin_url = run.harness.admin_url.clone();
            let wiki = setup_wiki_doc(&run.harness).await;
            let addr = run
                .spawn_router(&app_url, test_collab_config(4, 30_000))
                .await;
            send_collab_updates(addr, &wiki, &[&delete_only_base_update()]).await;

            let projected = await_persisted_get_body(addr, &wiki, 76).await;
            let admin = PgPoolOptions::new()
                .max_connections(2)
                .connect(&admin_url)
                .await
                .unwrap();
            let events_before = document_updated_event_count(&admin, wiki.document_id).await;

            sqlx::query("UPDATE fvoci.documents SET status = 'archived' WHERE id = $1")
                .bind(wiki.document_id)
                .execute(&admin)
                .await
                .unwrap();

            let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
            let mut writer = connect_member(addr, &wiki.session.session_token).await;
            auth_and_join(&mut writer, &routing_key, 94).await;
            writer
                .send(Message::Binary(
                    sync_update_frame(&routing_key, &delete_only_update()).into(),
                ))
                .await
                .unwrap();
            let mut saw_reject = false;
            for _ in 0..8 {
                if let Some(WireFrame::Document {
                    message: DocumentMessage::SyncStatus { applied: false },
                    ..
                }) = recv_document_frame(&mut writer, 1).await
                {
                    saw_reject = true;
                    break;
                }
            }
            assert!(saw_reject, "archived wiki doc must reject collab mutation");

            let body = get_document_body(
                addr,
                &wiki.session.session_token,
                wiki.session.workspace_id,
                wiki.document_id,
            )
            .await;
            assert_eq!(body["contentJson"], projected["contentJson"]);
            assert_eq!(
                document_updated_event_count(&admin, wiki.document_id).await,
                events_before
            );
        })
    })
    .await;
}

#[tokio::test]
async fn collab_operational_project_failure_recovers_primary() {
    run_test(
        "collab_operational_project_failure_recovers_primary",
        |run| {
            Box::pin(async {
                let app_url = run.harness.app_url.clone();
                let admin_url = run.harness.admin_url.clone();
                let wiki = setup_wiki_doc(&run.harness).await;
                let addr = run
                    .spawn_router(&app_url, test_collab_config(4, 30_000))
                    .await;
                let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
                let update = engine_fixture("map_child.v1");

                let mut writer = connect_member(addr, &wiki.session.session_token).await;
                auth_and_join(&mut writer, &routing_key, 83).await;
                writer
                    .send(Message::Binary(
                        sync_update_frame(&routing_key, &update).into(),
                    ))
                    .await
                    .unwrap();
                assert!(
                    wait_for_sync_applied(&mut writer, Duration::from_secs(5)).await,
                    "durable append must ack when project is operationally malformed"
                );

                let load = fvoci_server::db::collab::load_collab_document(
                    &wiki.session.pool,
                    wiki.session.workspace_id,
                    wiki.session.user_id,
                    wiki.session.session_id,
                    wiki.document_id,
                )
                .await
                .unwrap()
                .unwrap();
                assert_eq!(load.tail.len(), 1);
                assert_eq!(load.tail[0].payload, update);

                let mut second = connect_member(addr, &wiki.session.session_token).await;
                auth_and_join(&mut second, &routing_key, 84).await;
                complete_sync_handshake(&mut second, &routing_key).await;

                let request_id = Uuid::now_v7();
                second
                    .send(Message::Binary(
                        stateless_frame(&routing_key, &format!("persist:{request_id}")).into(),
                    ))
                    .await
                    .unwrap();
                assert!(
                    wait_for_stateless_exact(
                        &mut second,
                        &format!("persist-failed:{request_id}"),
                        Duration::from_secs(5),
                    )
                    .await,
                    "operational project failure must yield persist-failed"
                );

                let body = get_document_body(
                    addr,
                    &wiki.session.session_token,
                    wiki.session.workspace_id,
                    wiki.document_id,
                )
                .await;
                assert_eq!(body["contentJson"], empty_document_json());

                let admin = PgPoolOptions::new()
                    .max_connections(2)
                    .connect(&admin_url)
                    .await
                    .unwrap();
                assert_eq!(
                    document_updated_event_count(&admin, wiki.document_id).await,
                    0
                );

                let follow_up = delete_only_update();
                second
                    .send(Message::Binary(
                        sync_update_frame(&routing_key, &follow_up).into(),
                    ))
                    .await
                    .unwrap();
                assert!(
                    wait_for_sync_applied(&mut second, Duration::from_secs(5)).await,
                    "follow-up edit must ack after operational project recovery"
                );
            })
        },
    )
    .await;
}

#[tokio::test]
async fn collab_projection_recovery_failure_closes_room() {
    run_test("collab_projection_recovery_failure_closes_room", |run| {
        Box::pin(async {
            let app_url = run.harness.app_url.clone();
            let wiki = setup_wiki_doc(&run.harness).await;
            let addr = run
                .spawn_router(&app_url, tiny_output_project_collab_config(4, 30_000))
                .await;
            let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
            let update = delete_only_base_update();

            let mut writer = connect_member(addr, &wiki.session.session_token).await;
            auth_and_join(&mut writer, &routing_key, 85).await;
            let mut peer = connect_member(addr, &wiki.session.session_token).await;
            auth_and_join(&mut peer, &routing_key, 88).await;
            arm_force_primary_load_fail(wiki.document_id).await;
            writer
                .send(Message::Binary(
                    sync_update_frame(&routing_key, &update).into(),
                ))
                .await
                .unwrap();
            assert!(
                wait_for_sync_applied(&mut writer, Duration::from_secs(5)).await,
                "durable append must ack before room closes when recovery reload fails"
            );
            wait_for_ws_close_code(
                &mut writer,
                1011,
                Duration::from_secs(5),
                true,
                Some("primary engine unhealthy"),
            )
            .await;
            wait_for_committed_update_then_close(
                &mut peer,
                1011,
                Duration::from_secs(5),
                Some("primary engine unhealthy"),
            )
            .await;

            let mut denied = connect_member(addr, &wiki.session.session_token).await;
            join_denied(&mut denied, &routing_key, 86).await;

            disarm_force_primary_load_fail(wiki.document_id).await;

            let mut recovery = connect_member(addr, &wiki.session.session_token).await;
            auth_and_join(&mut recovery, &routing_key, 87).await;
            complete_sync_handshake(&mut recovery, &routing_key).await;

            let request_id = Uuid::now_v7();
            recovery
                .send(Message::Binary(
                    stateless_frame(&routing_key, &format!("persist:{request_id}")).into(),
                ))
                .await
                .unwrap();
            assert!(
                wait_for_stateless_exact(
                    &mut recovery,
                    &format!("persisted:{request_id}"),
                    Duration::from_secs(5),
                )
                .await,
                "persist must succeed after recovery reload is restored"
            );

            let follow_up = delete_only_update();
            recovery
                .send(Message::Binary(
                    sync_update_frame(&routing_key, &follow_up).into(),
                ))
                .await
                .unwrap();
            assert!(
                wait_for_sync_applied(&mut recovery, Duration::from_secs(5)).await,
                "follow-up edit must ack after projection recovery failure heals"
            );
        })
    })
    .await;
}

#[tokio::test]
async fn collab_post_commit_stale_writer_acks_before_close() {
    run_test("collab_post_commit_stale_writer_acks_before_close", |run| {
        Box::pin(async {
            let app_url = run.harness.app_url.clone();
            let wiki = setup_wiki_doc(&run.harness).await;
            let addr = run
                .spawn_router(&app_url, test_collab_config(4, 30_000))
                .await;
            let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
            let update = delete_only_base_update();

            let mut writer = connect_member(addr, &wiki.session.session_token).await;
            auth_and_join(&mut writer, &routing_key, 89).await;
            let mut peer = connect_member(addr, &wiki.session.session_token).await;
            auth_and_join(&mut peer, &routing_key, 99).await;
            let (reached_rx, proceed_tx) = arm_append_projection_barrier(wiki.document_id).await;
            writer
                .send(Message::Binary(
                    sync_update_frame(&routing_key, &update).into(),
                ))
                .await
                .unwrap();
            tokio::time::timeout(Duration::from_secs(5), reached_rx)
                .await
                .expect("append projection barrier must be reached after commit")
                .expect("barrier signal");
            claim_writer_and_load(
                &wiki.session.pool,
                wiki.session.workspace_id,
                wiki.session.user_id,
                wiki.session.session_id,
                wiki.document_id,
            )
            .await
            .unwrap()
            .unwrap();
            proceed_tx.send(()).expect("release projection barrier");
            disarm_append_projection_barrier(wiki.document_id).await;
            assert!(
                wait_for_sync_applied(&mut writer, Duration::from_secs(5)).await,
                "committed append must ack before post-commit stale-writer close"
            );
            wait_for_ws_close_code(
                &mut writer,
                1008,
                Duration::from_secs(5),
                true,
                Some("writer stale"),
            )
            .await;
            wait_for_committed_update_then_close(
                &mut peer,
                1008,
                Duration::from_secs(5),
                Some("writer stale"),
            )
            .await;
        })
    })
    .await;
}

#[tokio::test]
async fn collab_deterministic_project_failure_recovers_primary() {
    run_test(
        "collab_deterministic_project_failure_recovers_primary",
        |run| {
            Box::pin(async {
                let app_url = run.harness.app_url.clone();
                let wiki = setup_wiki_doc(&run.harness).await;
                let addr = run
                    .spawn_router(&app_url, tiny_output_project_collab_config(4, 30_000))
                    .await;
                let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
                let update = delete_only_base_update();

                let mut writer = connect_member(addr, &wiki.session.session_token).await;
                auth_and_join(&mut writer, &routing_key, 81).await;
                writer
                    .send(Message::Binary(
                        sync_update_frame(&routing_key, &update).into(),
                    ))
                    .await
                    .unwrap();
                assert!(
                    wait_for_sync_applied(&mut writer, Duration::from_secs(5)).await,
                    "durable append must still ack when project is deterministically skipped"
                );

                let mut second = connect_member(addr, &wiki.session.session_token).await;
                auth_and_join(&mut second, &routing_key, 82).await;
                complete_sync_handshake(&mut second, &routing_key).await;

                let request_id = Uuid::now_v7();
                second
                    .send(Message::Binary(
                        stateless_frame(&routing_key, &format!("persist:{request_id}")).into(),
                    ))
                    .await
                    .unwrap();
                assert!(
                    wait_for_stateless_exact(
                        &mut second,
                        &format!("persisted:{request_id}"),
                        Duration::from_secs(5),
                    )
                    .await,
                    "persist must succeed after deterministic project skip"
                );

                let body = get_document_body(
                    addr,
                    &wiki.session.session_token,
                    wiki.session.workspace_id,
                    wiki.document_id,
                )
                .await;
                assert_eq!(body["contentJson"], empty_document_json());

                let follow_up = delete_only_update();
                second
                    .send(Message::Binary(
                        sync_update_frame(&routing_key, &follow_up).into(),
                    ))
                    .await
                    .unwrap();
                assert!(
                    wait_for_sync_applied(&mut second, Duration::from_secs(5)).await,
                    "follow-up edit must ack after primary recovery"
                );
            })
        },
    )
    .await;
}

#[tokio::test]
async fn collab_unloaded_primary_precommit_update_closes_without_broadcast() {
    run_test(
        "collab_unloaded_primary_precommit_update_closes_without_broadcast",
        |run| {
            Box::pin(async {
                let app_url = run.harness.app_url.clone();
                let wiki = setup_wiki_doc(&run.harness).await;
                let addr = run
                    .spawn_router(&app_url, tiny_output_project_collab_config(4, 30_000))
                    .await;
                let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
                let initial = delete_only_base_update();

                let mut writer = connect_member(addr, &wiki.session.session_token).await;
                auth_and_join(&mut writer, &routing_key, 101).await;
                let mut peer = connect_member(addr, &wiki.session.session_token).await;
                auth_and_join(&mut peer, &routing_key, 102).await;
                writer
                    .send(Message::Binary(
                        sync_update_frame(&routing_key, &initial).into(),
                    ))
                    .await
                    .unwrap();
                assert!(
                    wait_for_sync_applied(&mut writer, Duration::from_secs(5)).await,
                    "initial committed content must ack before persist recovery"
                );
                assert!(
                    wait_for_sync_update(&mut peer, Duration::from_secs(5)).await,
                    "peer must receive the committed initial Sync Update"
                );

                let barrier_id = Uuid::now_v7();
                writer
                    .send(Message::Binary(
                        stateless_frame(&routing_key, &format!("persist:{barrier_id}")).into(),
                    ))
                    .await
                    .unwrap();
                assert!(
                    wait_for_stateless_exact(
                        &mut writer,
                        &format!("persisted:{barrier_id}"),
                        Duration::from_secs(5),
                    )
                    .await,
                    "initial persist must finish after committed content"
                );

                arm_force_primary_load_fail(wiki.document_id).await;
                let request_id = Uuid::now_v7();
                writer
                    .send(Message::Binary(
                        stateless_frame(&routing_key, &format!("persist:{request_id}")).into(),
                    ))
                    .await
                    .unwrap();
                assert!(
                    wait_for_stateless_exact(
                        &mut writer,
                        &format!("persisted:{request_id}"),
                        Duration::from_secs(5),
                    )
                    .await,
                    "tiny-output persist must succeed while leaving primary unloaded"
                );

                let load_after_persist = fvoci_server::db::collab::load_collab_document(
                    &wiki.session.pool,
                    wiki.session.workspace_id,
                    wiki.session.user_id,
                    wiki.session.session_id,
                    wiki.document_id,
                )
                .await
                .unwrap()
                .unwrap();
                let persist_snapshot = load_after_persist.snapshot.clone();
                let persist_tail_seq = load_after_persist.tail_seq;
                let persist_tail: Vec<Vec<u8>> = load_after_persist
                    .tail
                    .iter()
                    .map(|row| row.payload.clone())
                    .collect();
                assert!(
                    persist_tail_seq >= 1,
                    "persist must compact real committed content, tail_seq={persist_tail_seq}"
                );

                writer
                    .send(Message::Binary(
                        sync_update_frame(&routing_key, &delete_only_update()).into(),
                    ))
                    .await
                    .unwrap();
                wait_for_writer_close_without_peer_update(
                    &mut writer,
                    &mut peer,
                    1011,
                    Duration::from_secs(5),
                    Some("engine unavailable"),
                )
                .await;
                peer.send(Message::Binary(
                    sync_step1_frame(&routing_key, &[0, 0]).into(),
                ))
                .await
                .unwrap();
                wait_for_ws_close_code(
                    &mut peer,
                    1011,
                    Duration::from_secs(5),
                    true,
                    Some("engine unavailable"),
                )
                .await;

                let load_after_close = fvoci_server::db::collab::load_collab_document(
                    &wiki.session.pool,
                    wiki.session.workspace_id,
                    wiki.session.user_id,
                    wiki.session.session_id,
                    wiki.document_id,
                )
                .await
                .unwrap()
                .unwrap();
                assert_eq!(load_after_close.snapshot, persist_snapshot);
                assert_eq!(load_after_close.tail_seq, persist_tail_seq);
                assert_eq!(
                    load_after_close
                        .tail
                        .iter()
                        .map(|row| row.payload.clone())
                        .collect::<Vec<_>>(),
                    persist_tail,
                    "pre-commit 1011 must not append the rejected update"
                );

                disarm_force_primary_load_fail(wiki.document_id).await;

                let mut recovery = connect_member(addr, &wiki.session.session_token).await;
                auth_and_join(&mut recovery, &routing_key, 103).await;
                complete_sync_handshake(&mut recovery, &routing_key).await;
                recovery
                    .send(Message::Binary(
                        sync_update_frame(&routing_key, &delete_only_update()).into(),
                    ))
                    .await
                    .unwrap();
                assert!(
                    wait_for_sync_applied(&mut recovery, Duration::from_secs(5)).await,
                    "follow-up edit must ack after unloaded primary is restored"
                );
            })
        },
    )
    .await;
}

#[tokio::test]
async fn collab_manual_persist_fails_when_derived_event_insert_blocked() {
    run_test(
        "collab_manual_persist_fails_when_derived_event_insert_blocked",
        |run| {
            Box::pin(async {
                let app_url = run.harness.app_url.clone();
                let admin_url = run.harness.admin_url.clone();
                let wiki = setup_wiki_doc(&run.harness).await;
                let admin = PgPoolOptions::new()
                    .max_connections(2)
                    .connect(&admin_url)
                    .await
                    .unwrap();
                install_derived_document_updated_fail_trigger(
                    &admin,
                    "test_manual_persist_derive_fail",
                )
                .await;

                let addr = run
                    .spawn_router(&app_url, test_collab_config(4, 30_000))
                    .await;
                let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
                let request_id = Uuid::now_v7();

                let mut writer = connect_member(addr, &wiki.session.session_token).await;
                auth_and_join(&mut writer, &routing_key, 95).await;
                writer
                    .send(Message::Binary(
                        sync_update_frame(&routing_key, &delete_only_base_update()).into(),
                    ))
                    .await
                    .unwrap();
                assert!(wait_for_sync_applied(&mut writer, Duration::from_secs(5)).await);

                writer
                    .send(Message::Binary(
                        stateless_frame(&routing_key, &format!("persist:{request_id}")).into(),
                    ))
                    .await
                    .unwrap();

                let mut saw_persist_failed = false;
                for _ in 0..12 {
                    if let Some(WireFrame::Document {
                        message: DocumentMessage::Stateless(body),
                        ..
                    }) = recv_document_frame(&mut writer, 1).await
                    {
                        if body == format!("persist-failed:{request_id}") {
                            saw_persist_failed = true;
                            break;
                        }
                    }
                }
                assert!(
                    saw_persist_failed,
                    "manual persist must fail when derived event insert is blocked"
                );

                let body_after_edit = get_document_body(
                    addr,
                    &wiki.session.session_token,
                    wiki.session.workspace_id,
                    wiki.document_id,
                )
                .await;
                assert_eq!(body_after_edit["contentJson"], empty_document_json());

                sqlx::query("DROP TRIGGER fvoci_test_manual_persist_derive_fail ON fvoci.events")
                    .execute(&admin)
                    .await
                    .unwrap();

                let retry_id = Uuid::now_v7();
                writer
                    .send(Message::Binary(
                        stateless_frame(&routing_key, &format!("persist:{retry_id}")).into(),
                    ))
                    .await
                    .unwrap();
                let mut saw_persisted = false;
                for _ in 0..12 {
                    if let Some(WireFrame::Document {
                        message: DocumentMessage::Stateless(body),
                        ..
                    }) = recv_document_frame(&mut writer, 1).await
                    {
                        if body == format!("persisted:{retry_id}") {
                            saw_persisted = true;
                            break;
                        }
                    }
                }
                assert!(
                    saw_persisted,
                    "manual persist must succeed after derive repair"
                );

                let body_after_retry = get_document_body(
                    addr,
                    &wiki.session.session_token,
                    wiki.session.workspace_id,
                    wiki.document_id,
                )
                .await;
                assert_eq!(body_after_retry["contentJson"], delete_only_json_before());
            })
        },
    )
    .await;
}

#[tokio::test]
async fn collab_test_run_finishes_cleanup_on_deliberate_panic() {
    let harness = TestDb::bootstrap().await;
    let db_name = harness.db_name().to_string();
    let role_name = harness.role_name().to_string();
    let admin_url = harness.admin_url.clone();
    let app_url = harness.app_url.clone();

    let mut run = TestRun::new(harness);
    let addr = run
        .spawn_router(&app_url, test_collab_config(2, 30_000))
        .await;
    assert!(
        tokio::net::TcpStream::connect(addr).await.is_ok(),
        "server must accept connections before panic"
    );

    let case_outcome = AssertUnwindSafe(async {
        panic!("deliberate collab projection cleanup regression panic");
    })
    .catch_unwind()
    .await;
    assert!(
        case_outcome.is_err(),
        "case must panic for cleanup regression"
    );

    run.finish()
        .await
        .expect("finish must shut down server and drop database after panic");

    assert!(
        !TestDb::database_exists(&admin_url, &db_name)
            .await
            .expect("database existence probe"),
        "database must be dropped after panic cleanup"
    );
    assert!(
        !TestDb::role_exists(&admin_url, &role_name)
            .await
            .expect("role existence probe"),
        "role must be dropped after panic cleanup"
    );
    assert!(
        tokio::net::TcpStream::connect(addr).await.is_err(),
        "server task must join and stop accepting connections after hub shutdown"
    );
}
