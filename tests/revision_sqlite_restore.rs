#![cfg(feature = "db-tests")]

#[path = "support/libsql_finish_fixture.rs"]
#[allow(dead_code)]
mod libsql_fixture;
#[allow(dead_code)]
mod support;

use futures_util::{FutureExt, SinkExt, StreamExt};
use fvoci_server::{
    auth::{password::Keyring, AuthService},
    collab::{
        config::FamilyRoomTimings,
        hub::CollabHub,
        wire::{CollabKind, CollabRoomName},
    },
    db::{
        backend::{Backend, RemoteDatabase},
        migrate, pool, Db,
    },
    http::{rate_limit::RateLimiter, state::AppState},
};
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc, time::Duration};
use support::{
    auth_and_join, complete_sync_handshake, connect_member, engine_fixture, stateless_frame,
    sync_update_frame, test_collab_config, wait_for_stateless_exact, wait_for_sync_applied, PEPPER,
    PUBLIC_ORIGIN,
};
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

struct Fixture {
    root: PathBuf,
    backend: Backend,
    remote: Option<libsql_fixture::LibsqlFinishFixture>,
}
impl Fixture {
    async fn new(remote: bool) -> Self {
        let scratch = PathBuf::from(
            std::env::var_os("FVOCI_RESTORE_TEST_ROOT").expect("owned restore scratch required"),
        );
        let root = scratch.join(Uuid::now_v7().to_string());
        std::fs::create_dir(&root).unwrap();
        let (backend, driver) = if remote {
            let sqld = PathBuf::from(
                std::env::var_os("FVOCI_TEST_SQLD").expect("official local sqld required"),
            );
            let driver = libsql_fixture::LibsqlFinishFixture::start(&sqld, &root.join("sqld"))
                .await
                .unwrap();
            let database = driver.database().await.unwrap();
            let connection = database.connect().unwrap();
            // Test-owned empty protocol fixture, exact compiled migrations. The
            // product schema gate below verifies its complete structural digest.
            for step in migrate::compiled_sqlite_steps() {
                let tx = connection
                    .transaction_with_behavior(libsql::TransactionBehavior::Immediate)
                    .await
                    .unwrap();
                tx.execute_batch(step.sql).await.unwrap();
                tx.execute("INSERT INTO schema_migrations(version,lineage,sql_sha256,applied_at) VALUES(?1,?2,?3,unixepoch()*1000000)",
                    libsql::params![i64::from(step.version), migrate::SQLITE_LINEAGE, step.sha256]).await.unwrap();
                tx.commit().await.unwrap();
            }
            drop(connection);
            (
                Backend::LibsqlRemote(Arc::new(RemoteDatabase::from_test_driver(
                    database,
                    std::num::NonZeroU32::new(4).unwrap(),
                ))),
                Some(driver),
            )
        } else {
            let file = root.join("app.sqlite");
            migrate::run_sqlite_migrations(&file).await.unwrap();
            (
                Backend::Sqlite(pool::connect_sqlite_app(&file, 4).await.unwrap()),
                None,
            )
        };
        migrate::assert_sqlite_schema_current(&backend)
            .await
            .unwrap();
        Self {
            root,
            backend,
            remote: driver,
        }
    }
    async fn reopen(&mut self) {
        self.backend = if let Some(driver) = &self.remote {
            Backend::LibsqlRemote(Arc::new(RemoteDatabase::from_test_driver(
                driver.database().await.unwrap(),
                std::num::NonZeroU32::new(4).unwrap(),
            )))
        } else {
            Backend::Sqlite(
                pool::connect_sqlite_app(&self.root.join("app.sqlite"), 4)
                    .await
                    .unwrap(),
            )
        };
    }
    async fn sql(&self, query: &str) -> Value {
        let raw: String = match &self.backend {
            Backend::Sqlite(pool) => sqlx::query_scalar(query).fetch_one(pool).await.unwrap(),
            _ => {
                let database = self.remote.as_ref().unwrap().database().await.unwrap();
                let conn = database.connect().unwrap();
                let mut rows = conn.query(query, ()).await.unwrap();
                rows.next().await.unwrap().unwrap().get(0).unwrap()
            }
        };
        serde_json::from_str(&raw).unwrap()
    }
    async fn execute(&self, query: &str) {
        match &self.backend {
            Backend::Sqlite(pool) => {
                sqlx::query(query).execute(pool).await.unwrap();
            }
            _ => {
                self.remote
                    .as_ref()
                    .unwrap()
                    .database()
                    .await
                    .unwrap()
                    .connect()
                    .unwrap()
                    .execute_batch(query)
                    .await
                    .unwrap();
            }
        }
    }
    async fn finish(self) {
        self.backend.close().await.unwrap();
        if let Some(driver) = self.remote {
            let retired = driver.finish().await.unwrap();
            assert!(
                retired.sqld_reaped
                    && retired.proxy_joined
                    && retired.upstream_closed
                    && retired.proxy_closed
            );
        }
        std::fs::remove_dir_all(self.root).unwrap();
    }
}

async fn server(f: &Fixture) -> support::TestServer {
    let hub = Arc::new(
        CollabHub::new_backend(
            test_collab_config(4, 60_000),
            f.backend.clone(),
            Some(FamilyRoomTimings::new(60_000, 5_000).unwrap()),
        )
        .unwrap(),
    );
    let state = AppState {
        realtime_mode: fvoci_server::config::RealtimeMode::On,
        native_engine: None,
        auth: Arc::new(AuthService {
            db: Db::from_backend(f.backend.clone()),
            password_keys: Keyring::parse(PEPPER, "test").unwrap(),
        }),
        branding_name: "FVOCI".into(),
        public_origin: PUBLIC_ORIGIN.into(),
        cookie_secure: false,
        rate_limiter: RateLimiter::new(),
        storage: fvoci_server::attachments::LocalStorage::new(f.root.join("storage")).into(),
        upload: fvoci_server::attachments::UploadLimits {
            part_size_bytes: fvoci_server::config::DEFAULT_UPLOAD_PART_SIZE_BYTES,
            max_file_size_bytes: fvoci_server::config::DEFAULT_UPLOAD_MAX_FILE_SIZE_BYTES,
            create_rate_per_5min: fvoci_server::config::DEFAULT_UPLOAD_CREATE_RATE_PER_5MIN,
            part_put_slots: fvoci_server::attachments::PartPutSlots::new(
                fvoci_server::config::DEFAULT_UPLOAD_MAX_CONCURRENT_PARTS,
            ),
        },
        collab: Some(hub.clone()),
        meili: None,
        search_embedder: None,
        markdown: Some(
            fvoci_server::documents::markdown_helper::MarkdownHelper::new(env!(
                "CARGO_BIN_EXE_fvoci-server"
            )),
        ),
        import_wake: None,
        import_extractor_available: false,
        preview_extract: None,
        quota: Default::default(),
        streams: AppState::fresh_streams(),
        mailer: Arc::new(fvoci_server::mail::Mailer::disabled()),
    };
    support::spawn_server(fvoci_server::http::router(state, None), hub).await
}
async fn request(
    addr: std::net::SocketAddr,
    method: reqwest::Method,
    path: &str,
    token: &str,
    body: Option<Value>,
) -> (reqwest::StatusCode, Value) {
    let mut req = reqwest::Client::new()
        .request(method, format!("http://{addr}{path}"))
        .header("origin", PUBLIC_ORIGIN)
        .header("cookie", format!("fvoci_session={token}"));
    if let Some(body) = body {
        req = req.json(&body);
    }
    let response = req.send().await.unwrap();
    let status = response.status();
    (status, response.json().await.unwrap_or(Value::Null))
}
async fn setup(addr: std::net::SocketAddr, with_document: bool) -> (String, String, String) {
    let response = reqwest::Client::new().post(format!("http://{addr}/api/v1/setup"))
        .header("origin", PUBLIC_ORIGIN).json(&json!({"email":"restore@example.invalid","password":"supersecret1","givenName":"Restore","workspaceSlug":"restore","workspaceName":"Restore"}))
        .send().await.unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::CREATED);
    let cookie = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|h| h.to_str().ok())
        .find_map(|s| {
            s.strip_prefix("fvoci_session=")
                .map(|s| s.split(';').next().unwrap().to_string())
        })
        .unwrap();
    let setup: Value = response.json().await.unwrap();
    let workspace = setup["workspaceId"].as_str().unwrap().to_string();
    if !with_document {
        return (cookie, workspace, String::new());
    }
    let (status, document) = request(
        addr,
        reqwest::Method::POST,
        &format!("/api/v1/workspaces/{workspace}/documents"),
        &cookie,
        Some(
            json!({"commandId":Uuid::now_v7(),"title":"Restore retained history","parentId":null}),
        ),
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::CREATED, "{document}");
    (
        cookie,
        workspace,
        document["id"].as_str().unwrap().to_string(),
    )
}
async fn apply(addr: std::net::SocketAddr, token: &str, key: &str, update: &[u8], id: u32) {
    let mut ws = connect_member(addr, token).await;
    auth_and_join(&mut ws, key, id).await;
    complete_sync_handshake(&mut ws, key).await;
    ws.send(Message::Binary(sync_update_frame(key, update).into()))
        .await
        .unwrap();
    assert!(wait_for_sync_applied(&mut ws, Duration::from_secs(8)).await);
    let receipt = Uuid::now_v7();
    ws.send(Message::Binary(
        stateless_frame(key, &format!("persist:{receipt}")).into(),
    ))
    .await
    .unwrap();
    assert!(
        wait_for_stateless_exact(
            &mut ws,
            &format!("persisted:{receipt}"),
            Duration::from_secs(8)
        )
        .await
    );
    ws.close(None).await.unwrap();
}
async fn witness(f: &Fixture) -> Value {
    f.sql("SELECT json_object('body',(SELECT content_json FROM documents WHERE title='Restore retained history'),'states',(SELECT json_group_array(json_object('tail',tail_seq,'snapshot',hex(state),'generation',writer_generation)) FROM document_states),'updates',(SELECT json_group_array(json_object('seq',seq,'op',hex(op_id),'payload',hex(payload))) FROM document_collab_updates),'receipts',(SELECT count(*) FROM document_collab_op_receipts),'history',(SELECT json_group_array(json_object('id',hex(id),'snapshot',hex(y_snapshot),'source',hex(restored_from_id),'correlation',hex(restore_correlation_id),'base',restore_base_tail_seq,'committed',restore_committed_tail_seq,'reason',reason)) FROM revisions),'events',(SELECT count(*) FROM events),'audit',(SELECT count(*) FROM audit_log))").await
}

#[derive(Clone, Copy)]
enum Case {
    Commit,
    Ambiguous,
    Revoke,
    Rollback,
}
async fn contract(remote: bool, case: Case) {
    let mut f = Fixture::new(remote).await;
    let first = server(&f).await;
    let addr = first.addr;
    let outcome = std::panic::AssertUnwindSafe(async {
        let (token, workspace, document) = setup(addr, true).await;
        let key = CollabRoomName { workspace_id: Uuid::parse_str(&workspace).unwrap(), kind: CollabKind::Document, resource_id: Uuid::parse_str(&document).unwrap() }.routing_key();
        let base = format!("/api/v1/workspaces/{workspace}/documents/{document}");
        let mut peer = connect_member(addr, &token).await;
        auth_and_join(&mut peer, &key, 50).await;
        complete_sync_handshake(&mut peer, &key).await;
        apply(addr, &token, &key, &engine_fixture("structured.v1"), 1).await;
        let (status, source) = request(addr, reqwest::Method::POST, &format!("{base}/revisions"), &token, None).await;
        assert_eq!(status, reqwest::StatusCode::CREATED, "{source}");
        let revision = source["id"].as_str().unwrap();
        let path = format!("{base}/revisions/{revision}/restore");
        apply(addr, &token, &key, &engine_fixture("followup_edit.v1"), 2).await;
        let (status, preview) = request(addr, reqwest::Method::GET, &format!("{path}-preview"), &token, None).await;
        assert_eq!(status, reqwest::StatusCode::OK, "{preview}");
        assert!(preview["source"]["contentJson"].to_string().contains("안녕 본문"));
        assert!(preview["currentContentJson"].to_string().contains("후속편집한글"));
        let tail: i64 = preview["currentTailSeq"].as_str().unwrap().parse().unwrap();
        let input = json!({"correlationId":Uuid::now_v7(),"expectedTailSeq":tail.to_string()});
        let concurrent_peer_edit = engine_fixture("text.v1");
        let before_cas = witness(&f).await;
        let (bad, _) = request(addr, reqwest::Method::POST, &path, &token, Some(json!({"correlationId":Uuid::now_v7(),"expectedTailSeq":(tail-1).to_string()}))).await;
        assert_eq!(bad, reqwest::StatusCode::CONFLICT);
        let before = witness(&f).await;
        assert_eq!(before, before_cas, "stale-tail CAS has no persistent effects");
        let (status, restored) = match case {
            Case::Ambiguous => {
                let (reached, proceed) = fvoci_server::db::collab::arm_restore_committed_ambiguity(Uuid::parse_str(&workspace).unwrap(), Uuid::parse_str(&document).unwrap()).await;
                let work = tokio::spawn({ let path=path.clone(); let token=token.clone(); let input=input.clone(); async move { request(addr, reqwest::Method::POST, &path, &token, Some(input)).await }});
                reached.await.unwrap();
                let committed = witness(&f).await;
                assert_eq!(committed["states"][0]["tail"], tail+1);
                assert_eq!(committed["receipts"].as_i64().unwrap(), before["receipts"].as_i64().unwrap()+1);
                proceed.send(()).unwrap();
                work.await.unwrap()
            }
            Case::Revoke => {
                let (reached, proceed) = fvoci_server::collab::room::arm_append_revoke_barrier(Uuid::parse_str(&document).unwrap()).await;
                let work = tokio::spawn({ let path=path.clone(); let token=token.clone(); let input=input.clone(); async move { request(addr, reqwest::Method::POST, &path, &token, Some(input)).await }});
                reached.await.unwrap();
                f.execute("DELETE FROM memberships").await;
                let revoked = witness(&f).await;
                proceed.send(()).unwrap();
                let (status, body) = work.await.unwrap();
                assert_eq!(status, reqwest::StatusCode::UNPROCESSABLE_ENTITY, "{body}");
                assert_eq!(body["code"], "restore_rejected");
                assert_eq!(witness(&f).await, revoked, "revocation before apply leaves every restore effect absent");
                return None;
            }
            Case::Rollback => {
                f.execute("CREATE TRIGGER restore_body_failure BEFORE UPDATE OF content_json ON documents WHEN NEW.content_json NOT LIKE '%후속편집한글%' BEGIN SELECT RAISE(ABORT,'fixture body refusal'); END").await;
                let (status, failure) = request(addr, reqwest::Method::POST, &path, &token, Some(input.clone())).await;
                assert_eq!(status, reqwest::StatusCode::SERVICE_UNAVAILABLE, "{failure}");
                assert_eq!(witness(&f).await, before, "late body failure rolls back append, receipt, revision, events and audit");
                f.execute("DROP TRIGGER restore_body_failure").await;
                return None;
            }
            Case::Commit => request(addr, reqwest::Method::POST, &path, &token, Some(input.clone())).await,
        };
        assert_eq!(status, reqwest::StatusCode::OK, "{restored}");
        assert_ne!(restored["revisionId"], source["id"]);
        let after = witness(&f).await;
        assert_eq!(after["states"][0]["tail"], tail+1, "matching restore ACK is exactly expected+1");
        assert_eq!(after["receipts"].as_i64().unwrap(), before["receipts"].as_i64().unwrap()+1);
        assert_eq!(after["events"].as_i64().unwrap(), before["events"].as_i64().unwrap()+3);
        assert_eq!(after["audit"].as_i64().unwrap(), before["audit"].as_i64().unwrap()+1);
        let rows = after["history"].as_array().unwrap();
        for original in before["history"].as_array().unwrap() {
            assert_eq!(rows.iter().find(|r| r["id"] == original["id"]).unwrap(), original, "restore preserves previous revision bytes and metadata");
        }
        let restore = rows.iter().filter(|r| r["reason"] == "restore").collect::<Vec<_>>();
        assert_eq!(restore.len(), 1);
        assert_eq!(restore[0]["base"], tail);
        assert_eq!(restore[0]["committed"], tail+1);
        assert_eq!(restore[0]["id"], Uuid::parse_str(restored["revisionId"].as_str().unwrap()).unwrap().simple().to_string().to_uppercase());
        let (status, replay) = request(addr, reqwest::Method::POST, &path, &token, Some(input.clone())).await;
        assert_eq!(status, reqwest::StatusCode::OK, "{replay}");
        assert_eq!(replay, restored);
        assert_eq!(witness(&f).await, after, "same correlation replays without any second mutation");
        let (status, conflict) = request(addr, reqwest::Method::POST, &path, &token, Some(json!({"expectedTailSeq":(tail+1).to_string(),"correlationId":input["correlationId"]}))).await;
        assert_eq!(status, reqwest::StatusCode::CONFLICT, "{conflict}");
        let body = support::get_document_body(addr, &token, Uuid::parse_str(&workspace).unwrap(), Uuid::parse_str(&document).unwrap()).await;
        assert!(body["contentJson"].to_string().contains("안녕 본문"));
        assert!(!body["contentJson"].to_string().contains("후속편집한글"));
        let expected_payload = hex::decode(after["updates"].as_array().unwrap().last().unwrap()["payload"].as_str().unwrap()).unwrap();
        // Read the actual peer frame and compare the bytes with the new durable
        // tail, rather than accepting an unrelated update left in the socket.
        let got = tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                if let Some(Ok(Message::Binary(bytes))) = peer.next().await {
                    if let Ok(fvoci_server::collab::wire::WireFrame::Document { message: fvoci_server::collab::wire::DocumentMessage::Sync(sync), .. }) = fvoci_server::collab::wire::decode(&bytes) {
                        let (_, payload) = fvoci_server::collab::y_sync::parse_sync_payload(&sync.y_protocol, 8 * 1024 * 1024).unwrap();
                        if payload == expected_payload { break payload; }
                    }
                } else { panic!("peer closed before committed restore"); }
            }
        }).await.unwrap();
        assert_eq!(got, expected_payload);
        // This independent Yjs update was authored before restore. Persisting
        // it afterwards must merge into the restored document, not overwrite it.
        apply(addr, &token, &key, &concurrent_peer_edit, 3).await;
        let body = support::get_document_body(addr, &token, Uuid::parse_str(&workspace).unwrap(), Uuid::parse_str(&document).unwrap()).await;
        assert!(body["contentJson"].to_string().contains("안녕 본문"));
        assert!(body["contentJson"].to_string().contains("가나다"));
        peer.close(None).await.unwrap();
        Some((token, base, body["contentJson"].clone()))
    }).catch_unwind().await;
    first.shutdown().await.unwrap();
    let restart = if let Ok(Some((token, base, expected))) = &outcome {
        f.reopen().await;
        let restarted = server(&f).await;
        let result = std::panic::AssertUnwindSafe(async {
            let (status, read) = request(
                restarted.addr,
                reqwest::Method::GET,
                &format!("{base}/body"),
                token,
                None,
            )
            .await;
            assert_eq!(status, reqwest::StatusCode::OK, "{read}");
            assert_eq!(
                read["contentJson"], *expected,
                "new client after restart sees committed restore"
            );
        })
        .catch_unwind()
        .await;
        restarted.shutdown().await.unwrap();
        result
    } else {
        Ok(())
    };
    f.finish().await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
    if let Err(panic) = restart {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test]
async fn sqlite_on_restore_commit_replay_peer_and_restart() {
    contract(false, Case::Commit).await;
}
#[tokio::test]
async fn sqlite_on_restore_committed_ambiguity() {
    contract(false, Case::Ambiguous).await;
}
#[tokio::test]
async fn sqlite_on_restore_revoked_before_apply() {
    contract(false, Case::Revoke).await;
}
#[tokio::test]
async fn sqlite_on_restore_late_failure_rolls_back() {
    contract(false, Case::Rollback).await;
}
#[tokio::test]
async fn libsql_on_restore_commit_replay_peer_and_restart() {
    contract(true, Case::Commit).await;
}
#[tokio::test]
async fn libsql_on_restore_committed_ambiguity() {
    contract(true, Case::Ambiguous).await;
}
#[tokio::test]
async fn libsql_on_restore_revoked_before_apply() {
    contract(true, Case::Revoke).await;
}
#[tokio::test]
async fn libsql_on_restore_late_failure_rolls_back() {
    contract(true, Case::Rollback).await;
}

#[tokio::test]
async fn sqlite_native_archive_separate_database_preserves_restored_history() {
    use fvoci_server::db::{
        import_jobs::claim_next_import_job_backend,
        native_archive::{
            capture_backend, publish_backend, queue_restore_backend, stage_key_backend, Selection,
        },
    };
    use fvoci_server::native_archive::{self as native, validate_native};
    use std::collections::BTreeMap;
    use tokio_util::sync::CancellationToken;
    let source = Fixture::new(false).await;
    let mut destination = Fixture::new(false).await;
    assert_ne!(source.root, destination.root);
    let src = server(&source).await;
    let dst = server(&destination).await;
    let result = std::panic::AssertUnwindSafe(async {
        let (token, workspace, _) = setup(src.addr, true).await;
        let (dest_token, dest_workspace, _) = setup(dst.addr, false).await;
        let (_, actor) = request(src.addr, reqwest::Method::GET, "/api/v1/auth/me", &token, None).await;
        let actor_id = Uuid::parse_str(actor["userId"].as_str().unwrap()).unwrap();
        let credential = Uuid::parse_str(actor["sessionId"].as_str().unwrap()).unwrap();
        let (_, dest_actor) = request(dst.addr, reqwest::Method::GET, "/api/v1/auth/me", &dest_token, None).await;
        let dest_actor_id = Uuid::parse_str(dest_actor["userId"].as_str().unwrap()).unwrap();
        let dest_credential = Uuid::parse_str(dest_actor["sessionId"].as_str().unwrap()).unwrap();
        let (status, project) = request(src.addr, reqwest::Method::POST, &format!("/api/v1/workspaces/{workspace}/projects"), &token,
            Some(json!({"key":"ARC","name":"복원할 프로젝트","visibility":"private"}))).await;
        assert_eq!(status, reqwest::StatusCode::CREATED, "{project}");
        let project_id = Uuid::parse_str(project["id"].as_str().unwrap()).unwrap();
        let project_base = format!("/api/v1/workspaces/{workspace}/projects/{project_id}");
        let mut documents = Vec::new();
        for title in ["아카이브 본문", "아카이브 참조"] {
            let (status, row) = request(src.addr, reqwest::Method::POST, &format!("{project_base}/documents"), &token,
                Some(json!({"title":title,"parentId":project["rootDocumentId"]}))).await;
            assert_eq!(status, reqwest::StatusCode::CREATED, "{row}");
            documents.push(row);
        }
        let doc = Uuid::parse_str(documents[0]["id"].as_str().unwrap()).unwrap();
        let reference = Uuid::parse_str(documents[1]["id"].as_str().unwrap()).unwrap();
        let attachment = Uuid::now_v7();
        let file_key = Uuid::now_v7().to_string();
        let file_bytes = "보존할 첨부 bytes\n\0🧪".as_bytes().to_vec();
        let storage: fvoci_server::attachments::ObjectStorage = fvoci_server::attachments::LocalStorage::new(source.root.join("storage")).into();
        storage.put_bytes(&file_key, file_bytes.clone()).await.unwrap();
        let Backend::Sqlite(source_pool) = &source.backend else { unreachable!() };
        sqlx::query("INSERT INTO attachments(id,workspace_id,document_id,uploader_id,status,name,mime,size_bytes,reserved_size_bytes,storage_key,completed_at) VALUES(?1,?2,?3,?4,'stored','자료.txt','text/plain',?5,?5,?6,unixepoch()*1000000)")
            .bind(attachment.as_bytes().as_slice()).bind(Uuid::parse_str(&workspace).unwrap().as_bytes().as_slice()).bind(doc.as_bytes().as_slice()).bind(actor_id.as_bytes().as_slice())
            .bind(i64::try_from(file_bytes.len()).unwrap()).bind(&file_key).execute(source_pool).await.unwrap();
        let content = json!({"type":"doc","content":[
            {"type":"paragraph","attrs":{"id":"retained-archive-block"},"content":[{"type":"text","text":"반출 전에 복원한 본문 🧪"}]},
            {"type":"paragraph","attrs":{"id":"retained-reference"},"content":[{"type":"mention","attrs":{"entity":"document","id":reference,"label":"아카이브 참조"}}]},
            {"type":"attachment","attrs":{"id":attachment,"name":"자료.txt","mime":"text/plain","size":file_bytes.len()}}
        ]});
        let body_path = format!("{project_base}/documents/{doc}");
        let (status, write) = request(src.addr, reqwest::Method::PUT, &format!("{body_path}/body"), &token, Some(json!({"contentJson":content}))).await;
        assert_eq!(status, reqwest::StatusCode::OK, "{write}");
        let (status, revision) = request(src.addr, reqwest::Method::POST, &format!("{body_path}/revisions"), &token, None).await;
        assert_eq!(status, reqwest::StatusCode::CREATED, "{revision}");
        let revision_id = revision["id"].as_str().unwrap();
        let (status, write) = request(src.addr, reqwest::Method::PUT, &format!("{body_path}/body"), &token,
            Some(json!({"contentJson":{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"교체될 본문"}]}]}}))).await;
        assert_eq!(status, reqwest::StatusCode::OK, "{write}");
        let restore_path = format!("{body_path}/revisions/{revision_id}/restore");
        let (status, preview) = request(src.addr, reqwest::Method::GET, &format!("{restore_path}-preview"), &token, None).await;
        assert_eq!(status, reqwest::StatusCode::OK, "{preview}");
        let (status, restored) = request(src.addr, reqwest::Method::POST, &restore_path, &token,
            Some(json!({"expectedTailSeq":preview["currentTailSeq"],"correlationId":Uuid::now_v7()}))).await;
        assert_eq!(status, reqwest::StatusCode::OK, "{restored}");
        let selection = Selection {project:project_id,zotero_connectors:&[]};
        let mut capture = capture_backend(&source.backend, Uuid::parse_str(&workspace).unwrap(), actor_id, credential, &selection).await.unwrap();
        assert_eq!(capture.archive.graph.attachments.len(), 1);
        assert_eq!(capture.file_keys.get(&attachment), Some(&file_key));
        let payload_entry = capture.archive.graph.attachments[0].payload_entry.clone();
        capture.archive.entries.insert(payload_entry.clone(), native::encode(&native::read_file(&storage, &file_key, file_bytes.len() as i64).await.unwrap()));
        let cancel = CancellationToken::new();
        let archive = validate_native(capture.archive, test_collab_config(4,60_000), &cancel).await.unwrap();
        let restore_uuid = Uuid::parse_str(restored["revisionId"].as_str().unwrap()).unwrap();
        assert!(archive.graph.revisions.iter().any(|r|r.id==restore_uuid));
        let mut missing = archive.clone(); missing.entries.remove(&payload_entry);
        assert!(missing.validate().is_err(), "missing file refuses before publication");
        let mut tampered = archive.clone(); tampered.entries.insert(payload_entry.clone(), native::encode(b"tampered"));
        assert!(tampered.validate().is_err(), "attachment length tampering refuses");
        // Existing native container reader/writer: this is the real product
        // executable, rather than the integration test runner's current_exe.
        let helper = PathBuf::from(env!("CARGO_BIN_EXE_fvoci-server"));
        let packed = native::container(&helper, serde_json::to_vec(&archive).unwrap(), true, &cancel).await.unwrap();
        let packed = native::decode(&packed).unwrap();
        let parsed = native::parse(&helper, packed.clone(), &cancel).await.unwrap();
        let parsed = validate_native(parsed, test_collab_config(4,60_000), &cancel).await.unwrap();
        let hash = native::digest(&packed);
        let dest_workspace_id = Uuid::parse_str(&dest_workspace).unwrap();
        let job = queue_restore_backend(&destination.backend, dest_workspace_id, dest_actor_id, dest_credential, Uuid::now_v7(), &hash, &packed).await.unwrap();
        let claim = claim_next_import_job_backend(&destination.backend).await.unwrap().unwrap();
        assert_eq!(claim.job_id, job);
        let dest_storage: fvoci_server::attachments::ObjectStorage = fvoci_server::attachments::LocalStorage::new(destination.root.join("storage")).into();
        let staged = Uuid::now_v7().to_string();
        stage_key_backend(&destination.backend, &claim, attachment, &staged, &cancel).await.unwrap();
        dest_storage.put_bytes(&staged, parsed.bytes(&payload_entry).unwrap()).await.unwrap();
        let keys = BTreeMap::from([(attachment,staged.clone())]);
        publish_backend(&destination.backend, &claim, &parsed, &keys, &fvoci_server::db::quota::StorageQuota::Unlimited, &hash, &cancel).await.unwrap();
        let dest_base = format!("/api/v1/workspaces/{dest_workspace}/projects/{project_id}/documents/{doc}");
        let (status, body) = request(dst.addr, reqwest::Method::GET, &format!("{dest_base}/body"), &dest_token, None).await;
        assert_eq!(status, reqwest::StatusCode::OK, "{body}");
        assert_eq!(body["contentJson"], content);
        let (status, history) = request(dst.addr, reqwest::Method::GET, &format!("{dest_base}/revisions"), &dest_token, None).await;
        assert_eq!(status, reqwest::StatusCode::OK, "{history}");
        assert_eq!(history["items"].as_array().unwrap().len(), archive.graph.revisions.iter().filter(|r|r.target_id==doc).count());
        let (status, detail) = request(dst.addr, reqwest::Method::GET, &format!("{dest_base}/revisions/{restore_uuid}"), &dest_token, None).await;
        assert_eq!(status, reqwest::StatusCode::OK, "{detail}");
        assert_eq!(detail["restoredFromId"], revision["id"]);
        assert_eq!(detail["contentJson"], content);
        assert_eq!(native::read_file(&dest_storage, &staged, file_bytes.len() as i64).await.unwrap(), file_bytes);
        let (status, _) = request(dst.addr, reqwest::Method::GET, &format!("{dest_base}/body"), "", None).await;
        assert_eq!(status, reqwest::StatusCode::UNAUTHORIZED);
        let outsider = Uuid::now_v7();
        let outsider_token = fvoci_server::auth::token::new_token();
        let Backend::Sqlite(dest_pool) = &destination.backend else { unreachable!() };
        sqlx::query("INSERT INTO users(id,email,given_name) VALUES(?1,'outsider@example.invalid','Outsider')")
            .bind(outsider.as_bytes().as_slice()).execute(dest_pool).await.unwrap();
        sqlx::query("INSERT INTO memberships(workspace_id,user_id,role) VALUES(?1,?2,'member')")
            .bind(dest_workspace_id.as_bytes().as_slice()).bind(outsider.as_bytes().as_slice()).execute(dest_pool).await.unwrap();
        sqlx::query("INSERT INTO sessions(id,user_id,token_hash,expires_at) VALUES(?1,?2,?3,unixepoch()*1000000+3600000000)")
            .bind(Uuid::now_v7().as_bytes().as_slice()).bind(outsider.as_bytes().as_slice()).bind(&outsider_token.hash).execute(dest_pool).await.unwrap();
        let (status, _) = request(dst.addr, reqwest::Method::GET, &format!("{dest_base}/body"), &outsider_token.token, None).await;
        assert_eq!(status, reqwest::StatusCode::NOT_FOUND, "ordinary workspace member cannot read the restored private project");
        (dest_token, dest_base, content, attachment, staged, file_bytes)
    }).catch_unwind().await;
    src.shutdown().await.unwrap();
    dst.shutdown().await.unwrap();
    let restarted_result = if let Ok((token, base, expected, attachment, key, bytes)) = &result {
        destination.reopen().await;
        let restarted = server(&destination).await;
        let readback = std::panic::AssertUnwindSafe(async {
            let (status, body) = request(
                restarted.addr,
                reqwest::Method::GET,
                &format!("{base}/body"),
                token,
                None,
            )
            .await;
            assert_eq!(status, reqwest::StatusCode::OK, "{body}");
            assert_eq!(body["contentJson"], *expected);
            let storage: fvoci_server::attachments::ObjectStorage =
                fvoci_server::attachments::LocalStorage::new(destination.root.join("storage"))
                    .into();
            assert_eq!(
                native::read_file(&storage, key, bytes.len() as i64)
                    .await
                    .unwrap(),
                *bytes
            );
            let (status, detail) = request(
                restarted.addr,
                reqwest::Method::GET,
                &format!(
                    "/api/v1/workspaces/{}/attachments/{attachment}",
                    base.split('/').nth(4).unwrap()
                ),
                token,
                None,
            )
            .await;
            assert_eq!(status, reqwest::StatusCode::OK, "{detail}");
        })
        .catch_unwind()
        .await;
        restarted.shutdown().await.unwrap();
        readback
    } else {
        Ok(())
    };
    source.finish().await;
    destination.finish().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
    if let Err(panic) = restarted_result {
        std::panic::resume_unwind(panic);
    }
}
