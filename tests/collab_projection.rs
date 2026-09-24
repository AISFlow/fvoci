#![cfg(feature = "db-tests")]

mod support;

use std::net::SocketAddr;
use std::time::Duration;

use futures_util::SinkExt;
use fvoci_server::collab::room::{
    arm_force_primary_apply_fail, arm_force_primary_load_fail, disarm_force_primary_apply_fail,
    disarm_force_primary_load_fail,
};
use fvoci_server::collab::wire::{CollabKind, CollabRoomName, DocumentMessage, WireFrame};
use fvoci_server::db::collab::load_collab_document;
use fvoci_server::db::documents::empty_document_json;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use support::{
    auth_and_join, collab_app_state, connect_member, delete_only_base_update,
    delete_only_json_after, delete_only_json_before, delete_only_update, engine_fixture,
    expectations, get_document_body, recv_document_frame, setup_wiki_doc, spawn_server,
    stateless_frame, sync_update_frame, test_collab_config, wait_for_sync_applied, TestDb,
    WikiDocFixture,
};
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

const TEST_TIMEOUT: Duration = Duration::from_secs(30);

async fn run_test<Fut>(name: &str, case: Fut)
where
    Fut: std::future::Future<Output = ()>,
{
    tokio::time::timeout(TEST_TIMEOUT, case)
        .await
        .unwrap_or_else(|_| panic!("{name} hung (>{TEST_TIMEOUT:?}) including cleanup"));
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
    run_test("collab_edit_get_body_reflects_projection", async {
        let harness = TestDb::bootstrap().await;
        let wiki = setup_wiki_doc(&harness).await;
        let app = fvoci_server::http::router(
            collab_app_state(&harness.app_url, test_collab_config(4, 30_000)).await,
            None,
        );
        let addr = spawn_server(app).await;
        send_collab_updates(addr, &wiki, &[&delete_only_base_update()]).await;

        let body = get_document_body(
            addr,
            &wiki.session.session_token,
            wiki.session.workspace_id,
            wiki.document_id,
        )
        .await;
        assert_eq!(body["contentJson"], delete_only_json_before());
        assert_eq!(body["version"], 1);

        let admin = PgPoolOptions::new()
            .max_connections(2)
            .connect(&harness.admin_url)
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
        harness.cleanup().await;
    })
    .await;
}

#[tokio::test]
async fn collab_korean_edit_get_body_matches_fixture_oracle() {
    run_test(
        "collab_korean_edit_get_body_matches_fixture_oracle",
        async {
            let harness = TestDb::bootstrap().await;
            let wiki = setup_wiki_doc(&harness).await;
            let app = fvoci_server::http::router(
                collab_app_state(&harness.app_url, test_collab_config(4, 30_000)).await,
                None,
            );
            let addr = spawn_server(app).await;
            let base = engine_fixture("korean_emoji_base.v1");
            let mid = engine_fixture("korean_emoji_mid_edit.v1");
            let delete = engine_fixture("korean_emoji_delete.v1");
            send_collab_updates(addr, &wiki, &[&base, &mid, &delete]).await;

            let body = get_document_body(
                addr,
                &wiki.session.session_token,
                wiki.session.workspace_id,
                wiki.document_id,
            )
            .await;
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
                            .connect(&harness.admin_url)
                            .await
                            .unwrap(),
                    )
                    .await
                    .unwrap();
            assert_eq!(text, "가중🚀마바사");
            assert!(!chosung.is_empty());
            harness.cleanup().await;
        },
    )
    .await;
}

#[tokio::test]
async fn collab_delete_only_get_body_updates_json() {
    run_test("collab_delete_only_get_body_updates_json", async {
        let harness = TestDb::bootstrap().await;
        let wiki = setup_wiki_doc(&harness).await;
        let app = fvoci_server::http::router(
            collab_app_state(&harness.app_url, test_collab_config(4, 30_000)).await,
            None,
        );
        let addr = spawn_server(app).await;
        let base = engine_fixture("delete_only_base.v1");
        let delete_only = engine_fixture("delete_only.v1");
        send_collab_updates(addr, &wiki, &[&base, &delete_only]).await;

        let body = get_document_body(
            addr,
            &wiki.session.session_token,
            wiki.session.workspace_id,
            wiki.document_id,
        )
        .await;
        assert_eq!(
            body["contentJson"],
            expectations()["delete_only"]["prosemirror_json_after"]
        );

        let admin = PgPoolOptions::new()
            .max_connections(2)
            .connect(&harness.admin_url)
            .await
            .unwrap();
        assert_eq!(
            document_updated_event_count(&admin, wiki.document_id).await,
            2
        );
        harness.cleanup().await;
    })
    .await;
}

#[tokio::test]
async fn collab_seed_join_does_not_overwrite_paragraph() {
    run_test("collab_seed_join_does_not_overwrite_paragraph", async {
        let harness = TestDb::bootstrap().await;
        let wiki = setup_wiki_doc(&harness).await;
        let app = fvoci_server::http::router(
            collab_app_state(&harness.app_url, test_collab_config(4, 30_000)).await,
            None,
        );
        let addr = spawn_server(app).await;
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
            .connect(&harness.admin_url)
            .await
            .unwrap();
        assert_eq!(
            document_updated_event_count(&admin, wiki.document_id).await,
            0
        );
        harness.cleanup().await;
    })
    .await;
}

#[tokio::test]
async fn collab_primary_unhealthy_skips_projection_until_catch_up() {
    run_test(
        "collab_primary_unhealthy_skips_projection_until_catch_up",
        async {
            let harness = TestDb::bootstrap().await;
            let wiki = setup_wiki_doc(&harness).await;
            let app = fvoci_server::http::router(
                collab_app_state(&harness.app_url, test_collab_config(4, 30_000)).await,
                None,
            );
            let addr = spawn_server(app).await;
            let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
            let update = delete_only_base_update();

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

            let body_after = get_document_body(
                addr,
                &wiki.session.session_token,
                wiki.session.workspace_id,
                wiki.document_id,
            )
            .await;
            assert_eq!(body_after["contentJson"], delete_only_json_before());
            harness.cleanup().await;
        },
    )
    .await;
}

#[tokio::test]
async fn collab_derived_event_failure_preserves_binary_then_retries() {
    run_test(
        "collab_derived_event_failure_preserves_binary_then_retries",
        async {
            let harness = TestDb::bootstrap().await;
            let wiki = setup_wiki_doc(&harness).await;
            let admin = PgPoolOptions::new()
                .max_connections(2)
                .connect(&harness.admin_url)
                .await
                .unwrap();
            install_derived_document_updated_fail_trigger(&admin, "test_projection_event_fail")
                .await;

            let app = fvoci_server::http::router(
                collab_app_state(&harness.app_url, test_collab_config(4, 30_000)).await,
                None,
            );
            let addr = spawn_server(app).await;
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

            let body_retry = get_document_body(
                addr,
                &wiki.session.session_token,
                wiki.session.workspace_id,
                wiki.document_id,
            )
            .await;
            assert_eq!(body_retry["contentJson"], delete_only_json_after());
            assert!(document_updated_event_count(&admin, wiki.document_id).await >= 1);
            harness.cleanup().await;
        },
    )
    .await;
}

#[tokio::test]
async fn collab_catch_up_after_server_restart_without_retransmit() {
    run_test(
        "collab_catch_up_after_server_restart_without_retransmit",
        async {
            let harness = TestDb::bootstrap().await;
            let wiki = setup_wiki_doc(&harness).await;
            let cfg = test_collab_config(4, 30_000);
            let app = fvoci_server::http::router(
                collab_app_state(&harness.app_url, cfg.clone()).await,
                None,
            );
            let addr = spawn_server(app).await;
            send_collab_updates(addr, &wiki, &[&delete_only_base_update()]).await;

            let body_before = get_document_body(
                addr,
                &wiki.session.session_token,
                wiki.session.workspace_id,
                wiki.document_id,
            )
            .await;
            assert_eq!(body_before["contentJson"], delete_only_json_before());

            let app2 =
                fvoci_server::http::router(collab_app_state(&harness.app_url, cfg).await, None);
            let addr2 = spawn_server(app2).await;
            let body_after = get_document_body(
                addr2,
                &wiki.session.session_token,
                wiki.session.workspace_id,
                wiki.document_id,
            )
            .await;
            assert_eq!(body_before["contentJson"], body_after["contentJson"]);
            assert_eq!(body_after["version"], 1);
            harness.cleanup().await;
        },
    )
    .await;
}

#[tokio::test]
async fn collab_archived_document_skips_new_projection() {
    run_test("collab_archived_document_skips_new_projection", async {
        let harness = TestDb::bootstrap().await;
        let wiki = setup_wiki_doc(&harness).await;
        let app = fvoci_server::http::router(
            collab_app_state(&harness.app_url, test_collab_config(4, 30_000)).await,
            None,
        );
        let addr = spawn_server(app).await;
        send_collab_updates(addr, &wiki, &[&delete_only_base_update()]).await;

        let projected = get_document_body(
            addr,
            &wiki.session.session_token,
            wiki.session.workspace_id,
            wiki.document_id,
        )
        .await;
        let admin = PgPoolOptions::new()
            .max_connections(2)
            .connect(&harness.admin_url)
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
        harness.cleanup().await;
    })
    .await;
}

#[tokio::test]
async fn collab_manual_persist_fails_when_derived_event_insert_blocked() {
    run_test(
        "collab_manual_persist_fails_when_derived_event_insert_blocked",
        async {
            let harness = TestDb::bootstrap().await;
            let wiki = setup_wiki_doc(&harness).await;
            let admin = PgPoolOptions::new()
                .max_connections(2)
                .connect(&harness.admin_url)
                .await
                .unwrap();
            install_derived_document_updated_fail_trigger(
                &admin,
                "test_manual_persist_derive_fail",
            )
            .await;

            let app = fvoci_server::http::router(
                collab_app_state(&harness.app_url, test_collab_config(4, 30_000)).await,
                None,
            );
            let addr = spawn_server(app).await;
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

            let body_after_edit = get_document_body(
                addr,
                &wiki.session.session_token,
                wiki.session.workspace_id,
                wiki.document_id,
            )
            .await;
            assert_eq!(body_after_edit["contentJson"], empty_document_json());

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
            harness.cleanup().await;
        },
    )
    .await;
}
