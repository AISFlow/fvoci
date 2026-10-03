//! Synthetic external GETs, real Rust HTTP/auth/native and restricted-role PG.
use super::*;
use fvoci_server::db::context::set_tenant;
use fvoci_server::integrations::zotero::fixtures::{Upstream, KEY};
use fvoci_server::integrations::Integrations;
use sqlx::{Connection, PgConnection};
use std::sync::{atomic::Ordering, Arc};

struct OwnedServer {
    inner: support::TestServer,
    storage: std::path::PathBuf,
}
impl std::ops::Deref for OwnedServer {
    type Target = support::TestServer;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}
impl OwnedServer {
    async fn shutdown(self) -> Result<(), String> {
        self.inner.shutdown().await?;
        tokio::fs::remove_dir_all(self.storage)
            .await
            .map_err(|e| e.to_string())
    }
}

async fn boot() -> (TestDb, Upstream, OwnedServer, SessionFixture) {
    boot_with_search(None).await
}
async fn boot_with_search(
    meili: Option<fvoci_server::search::meili::MeiliConfig>,
) -> (TestDb, Upstream, OwnedServer, SessionFixture) {
    let db = TestDb::bootstrap().await;
    let upstream = Upstream::start().await;
    let owner = setup_owner_session(&db).await;
    let mut tx = owner.pool.begin().await.unwrap();
    set_tenant(&mut tx, owner.workspace_id).await.unwrap();
    sqlx::query("UPDATE fvoci.workspaces SET slug=$2 WHERE id=$1")
        .bind(owner.workspace_id)
        .bind(format!(
            "zt-{}",
            &owner.workspace_id.simple().to_string()[..24]
        ))
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let (mut state, hub) = collab_app_state(&db.app_url, test_collab_config(2, 60000)).await;
    state.meili = meili;
    let storage = match &state.storage {
        fvoci_server::attachments::ObjectStorage::Local(local) => local.root().to_path_buf(),
        _ => panic!("owned local fixture storage required"),
    };
    let mut integrations = Integrations::disabled();
    integrations.encryption_keys = Some(Arc::new(Keyring::parse(PEPPER, "test").unwrap()));
    integrations.zotero = upstream.reader();
    let inner = support::spawn_server(
        fvoci_server::http::router_with_integrations(state, None, Arc::new(integrations)),
        hub,
    )
    .await;
    let server = OwnedServer { inner, storage };
    let (status, value) = session_call(
        server.addr,
        Method::POST,
        "/api/v1/me/personal-workspace",
        &owner.session_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    let owner = SessionFixture {
        workspace_id: Uuid::parse_str(value["id"].as_str().unwrap()).unwrap(),
        ..owner
    };
    (db, upstream, server, owner)
}
fn root(owner: &SessionFixture) -> String {
    format!("/api/v1/workspaces/{}/zotero", owner.workspace_id)
}

#[tokio::test]
async fn zotero_literal_upstream_failure_matrix_is_durable_and_bounded() {
    run_test(
        "zotero_literal_upstream_failure_matrix_is_durable_and_bounded",
        async {
            let (db, upstream, server, owner) = boot().await;
            let id = connect(server.addr, &owner, "user").await;
            let (status, initial) = sync(server.addr, &owner, id).await;
            assert_eq!(status, StatusCode::OK, "{initial}");
            let reference = initial["references"][0]["id"].clone();
            for (mode, expected, calls) in [
                (4, StatusCode::SERVICE_UNAVAILABLE, 1),
                (10, StatusCode::SERVICE_UNAVAILABLE, 1),
                (11, StatusCode::SERVICE_UNAVAILABLE, 1),
                (12, StatusCode::SERVICE_UNAVAILABLE, 1),
                (3, StatusCode::SERVICE_UNAVAILABLE, 1),
                (8, StatusCode::BAD_REQUEST, 1),
                (5, StatusCode::CONFLICT, 4),
                (9, StatusCode::BAD_REQUEST, 7),
            ] {
                // Each literal response is a separate synthetic time window.
                // Expire only this fixture's already-tested deadline; credential
                // replacement must preserve it in the product.
                let admin = admin_pool(&db).await;
                sqlx::query("UPDATE fvoci.zotero_connectors SET retry_at=now()-interval '1 second' WHERE id=$1")
                    .bind(id).execute(&admin).await.unwrap();
                admin.close().await;
                assert_eq!(connect(server.addr, &owner, "user").await, id);
                upstream.model.mode.store(mode, Ordering::SeqCst);
                let before = upstream.model.log.lock().unwrap().len();
                let (status, value) = sync(server.addr, &owner, id).await;
                assert_eq!(status, expected, "fixture mode {mode}: {value}");
                assert!(!value.to_string().contains(KEY));
                assert_eq!(
                    upstream.model.log.lock().unwrap().len() - before,
                    calls,
                    "independent request budget for mode {mode}"
                );
                let current = fresh(server.addr, &owner, id).await;
                assert_eq!(current["connector"]["completedVersion"], "12");
                assert_eq!(current["references"][0]["id"], reference);
                assert_eq!(
                    current["connector"]["state"],
                    if mode == 3 { "denied" } else { "connected" }
                );
                if [4, 10, 11, 12].contains(&mode) {
                    assert!(current["connector"]["retryAt"].is_string());
                    let before = upstream.model.log.lock().unwrap().len();
                    let (status, _) = sync(server.addr, &owner, id).await;
                    assert_eq!(status, StatusCode::CONFLICT);
                    assert_eq!(upstream.model.log.lock().unwrap().len(), before);
                }
            }
            // Both ordinary and foreign Link headers are navigation hints only:
            // the legitimate fixed-key flow must still succeed without a fetch.
            assert_eq!(connect(server.addr, &owner, "user").await, id);
            upstream.model.mode.store(7, Ordering::SeqCst);
            let (status, value) = sync(server.addr, &owner, id).await;
            assert_eq!(status, StatusCode::OK, "{value}");
            assert!(upstream
                .model
                .log
                .lock()
                .unwrap()
                .iter()
                .all(|(_, url)| !url.contains("example.invalid")));
            upstream.model.mode.store(13, Ordering::SeqCst);
            let (status, value) = sync(server.addr, &owner, id).await;
            assert_eq!(status, StatusCode::OK, "{value}");
            assert_eq!(value["references"][0]["availability"], "excluded");
            assert_eq!(value["references"][0]["remoteVersion"], "13");
            assert_eq!(value["references"][0]["id"], reference);
            cleanup(db, upstream, server, owner).await;
        },
    )
    .await;
}
async fn connect(addr: SocketAddr, owner: &SessionFixture, kind: &str) -> Uuid {
    let(status,value)=session_call(addr,Method::POST,&root(owner),&owner.session_token,Some(json!({"libraryType":kind,"remoteLibraryId":"42","libraryUrl":format!("https://www.zotero.org/{}/42",if kind=="user"{"users"}else{"groups"}),"apiKey":KEY}))).await;
    assert_eq!(status, StatusCode::CREATED, "{value}");
    assert!(!value.to_string().contains(KEY));
    Uuid::parse_str(value["id"].as_str().unwrap()).unwrap()
}
async fn sync(addr: SocketAddr, owner: &SessionFixture, id: Uuid) -> (StatusCode, Value) {
    session_call(
        addr,
        Method::POST,
        &format!("{}/libraries/{id}/sync", root(owner)),
        &owner.session_token,
        None,
    )
    .await
}
async fn fresh(addr: SocketAddr, owner: &SessionFixture, id: Uuid) -> Value {
    // session_call constructs a genuinely new reqwest Client each time.
    let (status, value) = session_call(
        addr,
        Method::GET,
        &format!("{}/libraries/{id}", root(owner)),
        &owner.session_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    value
}
async fn observer(db: &TestDb, owner: &SessionFixture) -> PgConnection {
    let mut conn = PgConnection::connect(&db.app_url).await.unwrap();
    let flags: (bool, bool) =
        sqlx::query_as("SELECT rolsuper,rolbypassrls FROM pg_roles WHERE rolname=current_user")
            .fetch_one(&mut conn)
            .await
            .unwrap();
    assert_eq!(flags, (false, false));
    let forced: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='fvoci' AND c.relname IN ('zotero_connectors','zotero_credentials','zotero_references','zotero_collections','zotero_memberships','zotero_links') AND c.relrowsecurity AND c.relforcerowsecurity")
        .fetch_one(&mut conn).await.unwrap();
    assert_eq!(
        forced, 6,
        "all actual Zotero tables force row-level security"
    );
    sqlx::query("BEGIN").execute(&mut conn).await.unwrap();
    sqlx::query(
        "SELECT set_config('app.tenant_id',$1,true),set_config('app.self_user_id',$2,true)",
    )
    .bind(owner.workspace_id.to_string())
    .bind(owner.user_id.to_string())
    .execute(&mut conn)
    .await
    .unwrap();
    conn
}
async fn cleanup(db: TestDb, upstream: Upstream, server: OwnedServer, owner: SessionFixture) {
    owner.pool.close().await;
    server.shutdown().await.unwrap();
    upstream.shutdown().await;
    db.cleanup().await.unwrap();
}

fn assert_source_body(body: &Value, block: &str, text: &str) {
    assert_eq!(body["type"], "doc");
    let paragraphs = body["content"].as_array().expect("document paragraphs");
    assert_eq!(paragraphs.len(), 1);
    assert_eq!(paragraphs[0]["type"], "paragraph");
    assert_eq!(paragraphs[0]["attrs"]["id"], block);
    let runs = paragraphs[0]["content"].as_array().expect("paragraph text");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["type"], "text");
    assert_eq!(runs[0]["text"], text);
    assert!(runs[0]
        .get("marks")
        .is_none_or(|marks| marks.as_array().is_some_and(Vec::is_empty)));
}

/// Reconstruct a stored revision in an isolated existing engine, using the
/// complete persisted native source. Never restore or write the live document.
async fn project_source_revision(
    source: &fvoci_server::db::revisions::PersistedCollabSource,
    revision: Vec<u8>,
) -> Value {
    let config = test_collab_config(2, 60000);
    let snapshot = source.snapshot.clone();
    let tail = source.tail.clone();
    tokio::task::spawn_blocking(move || {
        use collab_engine::outcome::EngineStatus;
        use collab_engine::process::{ChildSlotKind, EngineSession, SpawnRequest};
        use collab_engine::protocol::Request;
        let mut engine = EngineSession::spawn(SpawnRequest {
            engine_bin: config.engine_bin,
            limits: config.limits,
            slot_kind: ChildSlotKind::Primary,
            slot_wait: None,
            test_hang_ms: None,
            test_exit_after_read: None,
            test_close_stdout_hang_ms: None,
            test_exit_after_write: None,
        })
        .unwrap();
        assert!(engine
            .call(&Request::Load {
                snapshot_b64: Some(snapshot),
                tail_b64: tail,
                encoding: 1,
            })
            .outcome
            .is_applied_ok());
        let update = match engine
            .call(&Request::RestoreFromSnapshot {
                snap_b64: revision,
                encoding: 1,
            })
            .outcome
        {
            EngineStatus::Ok {
                update_b64: Some(bytes),
                ..
            } => collab_engine::b64::decode(&bytes).unwrap(),
            other => panic!("persisted revision reconstruction failed: {other:?}"),
        };
        assert!(engine
            .call(&Request::Apply {
                update_b64: update,
                encoding: 1
            })
            .outcome
            .is_applied_ok());
        match engine.call(&Request::Project { encoding: 1 }).outcome {
            EngineStatus::Ok {
                content_json: Some(body),
                ..
            } => body,
            other => panic!("persisted revision projection failed: {other:?}"),
        }
    })
    .await
    .unwrap()
}

/// Real source preparation for W7's canonical archive consumer. This does not
/// implement or stand in for export, destination publish or reconciliation.
#[tokio::test]
async fn zotero_historical_source_has_one_reference_authored_revisions_and_task_origin() {
    run_test(
        "zotero_historical_source_has_one_reference_authored_revisions_and_task_origin",
        async {
            let (db, upstream, server, owner) = boot().await;
            upstream.model.mode.store(22, Ordering::SeqCst);
            // Mode22's literal one-item source is independent of the other
            // fixture's pagination expansion and rejects unrequested keys.
            upstream.model.many.store(1, Ordering::SeqCst);
            let negative = reqwest::Client::new()
                .get(format!("http://{}/users/42/items?itemKey=EFGH4567", upstream.addr))
                .send().await.unwrap();
            assert_eq!(negative.status(), StatusCode::BAD_REQUEST);
            let import_start = upstream.model.log.lock().unwrap().len();
            let id = connect(server.addr, &owner, "user").await;
            let (status, imported) = sync(server.addr, &owner, id).await;
            assert_eq!(status, StatusCode::OK, "{imported}");
            assert_eq!(imported["connector"]["completedVersion"], "99");
            assert_eq!(imported["connector"]["generation"], "1");
            assert_eq!(imported["connector"]["progressVersion"], Value::Null);
            assert_eq!(imported["connector"]["committedPages"], 0);
            assert_eq!(imported["connector"]["reconciliationRequired"], false);
            assert_eq!(imported["references"].as_array().unwrap().len(), 1);
            let reference = &imported["references"][0];
            assert_eq!(reference["itemKey"], "ABCD2345");
            assert_eq!(reference["remoteVersion"], "7");
            assert_eq!(reference["availability"], "available");
            assert_eq!(reference["bibliography"]["title"], "합성 연구 자료 🙂");
            assert_eq!(reference["bibliography"]["fields"]["ISBN"], "9780000000000");
            assert_eq!(reference["collectionKeys"], json!(["BCDE3456", "CDEF4567"]));
            assert_eq!(imported["collections"].as_array().unwrap().len(), 2);
            assert_eq!(imported["collections"][0]["key"], "BCDE3456");
            assert_eq!(imported["collections"][0]["parentKey"], Value::Null);
            assert_eq!(imported["collections"][1]["key"], "CDEF4567");
            assert_eq!(imported["collections"][1]["parentKey"], "BCDE3456");
            for collection in imported["collections"].as_array().unwrap() {
                assert_eq!(collection["availability"], "available");
            }
            let document = Uuid::parse_str(reference["id"].as_str().unwrap()).unwrap();
            let expected_version = reference["localVersion"].clone();

            let request_id = Uuid::now_v7();
            let capture = json!({"requestId":request_id,"intent":"task","title":"Compare my authored evidence"});
            let (status, authored) = session_call(server.addr, Method::POST,
                &format!("/api/v1/workspaces/{}/personal-input", owner.workspace_id),
                &owner.session_token, Some(capture.clone())).await;
            assert_eq!(status, StatusCode::CREATED, "{authored}");
            let task = Uuid::parse_str(authored["taskId"].as_str().unwrap()).unwrap();
            let origin = Uuid::parse_str(authored["documentId"].as_str().unwrap()).unwrap();
            let project = Uuid::parse_str(authored["projectId"].as_str().unwrap()).unwrap();
            assert_eq!(authored["replayed"], false);
            let origin_body = json!({"type":"doc","content":[{"type":"paragraph","attrs":{"id":"authored-commentary"},"content":[{"type":"text","text":"내가 쓴 의견과 비교 기록"}]}]});
            let (status, value) = session_call(server.addr, Method::PUT,
                &document_api(owner.workspace_id, origin, "/body"), &owner.session_token,
                Some(json!({"contentJson":origin_body}))).await;
            assert_eq!(status, StatusCode::OK, "{value}");
            let (status, value) = session_call(server.addr, Method::PATCH,
                &document_api(owner.workspace_id, document, ""), &owner.session_token,
                Some(json!({"title":"Authored astronomy 개인 의견"}))).await;
            assert_eq!(status, StatusCode::OK, "{value}");
            let first = json!({"type":"doc","content":[{"type":"paragraph","attrs":{"id":"owned-commentary"},"content":[{"type":"text","text":"My authored telescope notes. 개인 의견은 유지됩니다."}]}]});
            let second = json!({"type":"doc","content":[{"type":"paragraph","attrs":{"id":"owned-commentary"},"content":[{"type":"text","text":"My revised telescope comparison. 복원 후에도 유지됩니다."}]}]});
            let mut revisions = Vec::new();
            for body in [first, second] {
                let (status, value) = session_call(server.addr, Method::PUT,
                    &document_api(owner.workspace_id, document, "/body"), &owner.session_token,
                    Some(json!({"contentJson":body}))).await;
                assert_eq!(status, StatusCode::OK, "{value}");
                let (status, revision) = session_call(server.addr, Method::POST,
                    &document_api(owner.workspace_id, document, "/revisions"), &owner.session_token, None).await;
                assert_eq!(status, StatusCode::CREATED, "{revision}");
                revisions.push(Uuid::parse_str(revision["id"].as_str().unwrap()).unwrap());
            }
            assert_ne!(revisions[0], revisions[1]);
            // Independently load the actual app-role persisted native source,
            // rather than capturing the live room or reseeding its JSON cache.
            let native_pool = PgPoolOptions::new().max_connections(1).connect(&db.app_url).await.unwrap();
            let mut persisted_reference = None;
            for (target, block, text) in [
                (document, "owned-commentary", "My revised telescope comparison. 복원 후에도 유지됩니다."),
                (origin, "authored-commentary", "내가 쓴 의견과 비교 기록"),
            ] {
                let source = fvoci_server::db::revisions::load_persisted_target_source(
                    &native_pool, owner.workspace_id, owner.user_id, owner.session_id,
                    fvoci_server::db::revisions::RevisionTarget::Document(target),
                ).await.unwrap().unwrap();
                let config = test_collab_config(2, 60000);
                let (snapshot, tail) = (source.snapshot.clone(), source.tail.clone());
                let projected = tokio::task::spawn_blocking(move || {
                    fvoci_server::collab::revision::project_persisted_offline(
                        config.engine_bin, config.limits, snapshot, tail,
                    ).unwrap()
                }).await.unwrap();
                assert_source_body(&projected, block, text);
                if target == document { persisted_reference = Some(source); }
            }
            let (status, value) = session_call(server.addr, Method::POST,
                &format!("{}/references/{document}/links", root(&owner)), &owner.session_token,
                Some(json!({"documentId":null,"taskId":task,"anchor":null,"expectedVersion":expected_version}))).await;
            assert_eq!(status, StatusCode::OK, "{value}");

            // Each session_call uses a genuinely new HTTP client. Read the
            // first immutable revision after the later body/link writes.
            for (revision, text) in revisions.iter().zip([
                "My authored telescope notes. 개인 의견은 유지됩니다.",
                "My revised telescope comparison. 복원 후에도 유지됩니다.",
            ]) {
                let (status, value) = session_call(server.addr, Method::GET,
                    &document_api(owner.workspace_id, document, &format!("/revisions/{revision}")),
                    &owner.session_token, None).await;
                assert_eq!(status, StatusCode::OK, "{value}");
                assert_eq!(value["id"], revision.to_string());
                assert_eq!(value["targetKind"], "document");
                assert_eq!(value["targetId"], document.to_string());
                assert_eq!(value["reason"], "manual");
                assert_source_body(&value["contentJson"], "owned-commentary", text);
                assert!(!value["ySnapshot"].as_str().unwrap().is_empty());
                let stored_revision = fvoci_server::db::revisions::get_revision(
                    &native_pool, owner.workspace_id, owner.user_id, owner.session_id,
                    fvoci_server::db::revisions::RevisionTarget::Document(document), *revision,
                ).await.unwrap().unwrap();
                assert_eq!(stored_revision.content_json, value["contentJson"]);
                assert_eq!(collab_engine::b64::encode(&stored_revision.y_snapshot), value["ySnapshot"]);
                let projected = project_source_revision(
                    persisted_reference.as_ref().unwrap(), stored_revision.y_snapshot,
                ).await;
                assert_source_body(&projected, "owned-commentary", text);
            }
            native_pool.close().await;
            let (status, meta) = session_call(server.addr, Method::GET,
                &document_api(owner.workspace_id, document, ""), &owner.session_token, None).await;
            assert_eq!(status, StatusCode::OK, "{meta}");
            assert_eq!(meta["title"], "Authored astronomy 개인 의견");
            let (status, body) = session_call(server.addr, Method::GET,
                &document_api(owner.workspace_id, document, "/body"), &owner.session_token, None).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_source_body(&body["contentJson"], "owned-commentary", "My revised telescope comparison. 복원 후에도 유지됩니다.");
            let (status, body) = session_call(server.addr, Method::GET,
                &document_api(owner.workspace_id, origin, "/body"), &owner.session_token, None).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_source_body(&body["contentJson"], "authored-commentary", "내가 쓴 의견과 비교 기록");
            let (status, origins) = session_call(server.addr, Method::GET,
                &task_path(&owner, task, "/origin"), &owner.session_token, None).await;
            assert_eq!(status, StatusCode::OK, "{origins}");
            assert_eq!(origins["items"].as_array().unwrap().len(), 1);
            assert_eq!(origins["items"][0]["taskId"], task.to_string());
            assert_eq!(origins["items"][0]["documentId"], origin.to_string());
            assert_eq!(origins["items"][0]["taskTitle"], "Compare my authored evidence");
            let current = fresh(server.addr, &owner, id).await;
            assert_eq!(current["connector"]["completedVersion"], "99");
            assert_eq!(current["references"].as_array().unwrap().len(), 1);
            assert_eq!(current["references"][0]["id"], document.to_string());
            assert_eq!(current["references"][0]["availability"], "available");
            assert_eq!(current["collections"].as_array().unwrap().len(), 2);
            for collection in current["collections"].as_array().unwrap() {
                assert_eq!(collection["availability"], "available");
            }
            assert_eq!(current["references"][0]["bibliography"]["title"], "합성 연구 자료 🙂");
            assert_eq!(current["references"][0]["links"][0]["taskId"], task.to_string());
            assert_eq!(current["references"][0]["links"].as_array().unwrap().len(), 1);

            let mut conn = observer(&db, &owner).await;
            let input: fvoci_server::api::personal_input_dto::PersonalInputBody = serde_json::from_value(capture).unwrap();
            let receipt_hash = fvoci_server::db::personal_input::request_hash(owner.workspace_id, owner.user_id, &input);
            let normalized = json!({"title":"Compare my authored evidence","type":"task","priority":"medium","statusId":null,"startDate":null,"dueDate":null,"parentId":null,"milestoneId":null,"recurrence":null,"selfAssign":true});
            let origin_hash = fvoci_server::db::task_origins::origin_request_hash(owner.user_id, project, None, &normalized);
            let receipts: Vec<Value> = sqlx::query_scalar("SELECT jsonb_build_object('workspaceId',workspace_id,'actor',actor_user_id,'requestId',request_id,'hash',request_hash,'intent',intent,'documentId',document_id,'taskId',task_id,'projectId',project_id) FROM fvoci.personal_input_commands WHERE workspace_id=$1 AND actor_user_id=$2 AND request_id=$3")
                .bind(owner.workspace_id).bind(owner.user_id).bind(request_id).fetch_all(&mut conn).await.unwrap();
            assert_eq!(receipts, vec![json!({"workspaceId":owner.workspace_id,"actor":owner.user_id,"requestId":request_id,"hash":receipt_hash,"intent":"task","documentId":origin,"taskId":task,"projectId":project})]);
            let origin_rows: Vec<Value> = sqlx::query_scalar("SELECT jsonb_build_object('workspaceId',workspace_id,'taskId',task_id,'documentId',document_id,'requestId',request_id,'hash',request_hash,'anchor',anchor) FROM fvoci.task_origins WHERE workspace_id=$1 AND task_id=$2")
                .bind(owner.workspace_id).bind(task).fetch_all(&mut conn).await.unwrap();
            assert_eq!(origin_rows, vec![json!({"workspaceId":owner.workspace_id,"taskId":task,"documentId":origin,"requestId":request_id,"hash":origin_hash,"anchor":null})]);
            let stored: Value = sqlx::query_scalar("SELECT jsonb_build_object('documentId',r.document_id,'wiki',d.project_id IS NULL,'completedVersion',c.completed_version::text,'progressVersion',c.progress_version::text,'hasReadLease',c.sync_id IS NOT NULL,'credentialRows',(SELECT count(*) FROM fvoci.zotero_credentials k WHERE k.connector_id=c.id),'references',(SELECT count(*) FROM fvoci.zotero_references x WHERE x.connector_id=c.id),'collections',(SELECT count(*) FROM fvoci.zotero_collections x WHERE x.connector_id=c.id),'memberships',(SELECT count(*) FROM fvoci.zotero_memberships x WHERE x.connector_id=c.id),'links',(SELECT count(*) FROM fvoci.zotero_links x WHERE x.connector_id=c.id)) FROM fvoci.zotero_references r JOIN fvoci.documents d ON d.workspace_id=r.workspace_id AND d.id=r.document_id JOIN fvoci.zotero_connectors c ON c.id=r.connector_id WHERE r.id=$1")
                .bind(document).fetch_one(&mut conn).await.unwrap();
            assert_eq!(stored, json!({"documentId":document,"wiki":true,"completedVersion":"99","progressVersion":null,"hasReadLease":false,"credentialRows":1,"references":1,"collections":2,"memberships":2,"links":1}));
            sqlx::query("ROLLBACK").execute(&mut conn).await.unwrap();
            conn.close().await.unwrap();
            {
                let requests = upstream.model.log.lock().unwrap();
                assert!(requests[import_start..].iter().all(|(method, uri)| method == "GET" && uri.starts_with("/users/42/") && !uri.contains("EFGH4567") && !uri.contains(KEY)));
            }
            cleanup(db, upstream, server, owner).await;
        },
    ).await;
}

#[tokio::test]
async fn zotero_readonly_identity_links_private_deletion_and_new_client() {
    run_test("zotero_readonly_identity_links_private_deletion_and_new_client",async{
        let(db,upstream,server,owner)=boot().await;let id=connect(server.addr,&owner,"user").await;
        let(status,value)=sync(server.addr,&owner,id).await;assert_eq!(status,StatusCode::OK,"{value}");
        assert_eq!(value["connector"]["completedVersion"],"12");assert_eq!(value["references"].as_array().unwrap().len(),2);
        let(reference_id,version)={let r=&value["references"][0];assert_eq!(r["itemKey"],"ABCD2345");assert_eq!(r["bibliography"]["title"],"합성 연구 자료 🙂");assert_eq!(r["collectionKeys"],json!(["BCDE3456","CDEF4567"]));assert_eq!(r["bibliography"]["creators"][1]["name"],"Synthetic Research Group");(r["id"].as_str().unwrap().to_owned(),r["localVersion"].as_str().unwrap().to_owned())};
        let mut conn=observer(&db,&owner).await;
        let row:(Uuid,String,i64)=sqlx::query_as("SELECT r.document_id,r.bibliography->>'title',c.completed_version FROM fvoci.zotero_references r JOIN fvoci.zotero_connectors c ON c.id=r.connector_id WHERE r.item_key='ABCD2345' AND r.connector_id=$1").bind(id).fetch_one(&mut conn).await.unwrap();
        assert_eq!(row,(Uuid::parse_str(&reference_id).unwrap(),"합성 연구 자료 🙂".to_owned(),12));
        let sealed:String=sqlx::query_scalar("SELECT sealed_key FROM fvoci.zotero_credentials WHERE connector_id=$1").bind(id).fetch_one(&mut conn).await.unwrap();assert!(!sealed.contains(KEY));assert!(sealed.starts_with("enc:v2:test:"));drop(conn);
        let(status,note)=session_call(server.addr,Method::POST,&format!("/api/v1/workspaces/{}/personal-input",owner.workspace_id),&owner.session_token,Some(json!({"requestId":Uuid::now_v7(),"intent":"task","title":"Read and compare evidence"}))).await;assert_eq!(status,StatusCode::CREATED,"{note}");
        let task=note["taskId"].as_str().unwrap();let authored=note["documentId"].as_str().unwrap();
        let body=json!({"taskId":task,"documentId":null,"anchor":null,"expectedVersion":version});
        let path=format!("{}/references/{reference_id}/links",root(&owner));
        for _ in 0..2{let(status,value)=session_call(server.addr,Method::POST,&path,&owner.session_token,Some(body.clone())).await;assert_eq!(status,StatusCode::OK,"{value}");assert!(!value.to_string().contains(KEY));}
        let new=fresh(server.addr,&owner,id).await;assert_eq!(new["references"][0]["links"].as_array().unwrap().len(),1);assert_eq!(new["references"][0]["links"][0]["taskId"],task);
        let group=connect(server.addr,&owner,"group").await;let(status,group_data)=sync(server.addr,&owner,group).await;assert_eq!(status,StatusCode::OK,"{group_data}");assert_ne!(group_data["references"][0]["id"],reference_id);
        let mut conn=observer(&db,&owner).await;let grants:i64=sqlx::query_scalar("SELECT count(*) FROM fvoci.memberships WHERE workspace_id=$1").bind(owner.workspace_id).fetch_one(&mut conn).await.unwrap();assert_eq!(grants,1);drop(conn);
        let outsider=create_user_session(&db,owner.workspace_id,WorkspaceRole::Member).await;
        for endpoint in [format!("{}/libraries/{id}",root(&owner)),root(&owner)]{let(status,value)=session_call(server.addr,Method::GET,&endpoint,&outsider.session_token,None).await;assert_eq!(status,StatusCode::NOT_FOUND,"{value}");assert!(!value.to_string().contains("합성"));}
        let(status,value)=session_call(server.addr,Method::POST,&path,&outsider.session_token,Some(body)).await;assert_eq!(status,StatusCode::NOT_FOUND,"{value}");
        let mut hidden=observer(&db,&owner).await;sqlx::query("SELECT set_config('app.self_user_id',$1,true)").bind(outsider.user_id.to_string()).execute(&mut hidden).await.unwrap();let count:i64=sqlx::query_scalar("SELECT count(*) FROM fvoci.zotero_references").fetch_one(&mut hidden).await.unwrap();assert_eq!(count,0);drop(hidden);
        upstream.model.mode.store(2,Ordering::SeqCst);let(status,value)=sync(server.addr,&owner,id).await;assert_eq!(status,StatusCode::OK,"{value}");assert_eq!(value["connector"]["completedVersion"],"13");
        assert_eq!(value["references"][0]["id"],reference_id);assert_eq!(value["references"][0]["availability"],"deleted");assert_eq!(value["references"][0]["links"][0]["taskId"],task);
        let(status,note_after)=session_call(server.addr,Method::GET,&format!("/api/v1/workspaces/{}/documents/{authored}",owner.workspace_id),&owner.session_token,None).await;assert_eq!(status,StatusCode::OK,"{note_after}");assert_eq!(note_after["title"],"Read and compare evidence");
        let(status,value)=session_call(server.addr,Method::DELETE,&format!("{}/libraries/{id}",root(&owner)),&owner.session_token,None).await;assert_eq!(status,StatusCode::OK);assert_eq!(value["state"],"disconnected");
        assert_eq!(fresh(server.addr,&owner,id).await["references"][0]["id"],reference_id);
        assert!(upstream.model.log.lock().unwrap().iter().all(|(method,uri)|method=="GET"&&!uri.contains(KEY)&&!uri.contains("/file")&&!uri.contains("/children")));
        outsider.pool.close().await;cleanup(db,upstream,server,owner).await;
    }).await;
}

#[tokio::test]
async fn zotero_partial_page_and_final_sql_rollback_keep_cold_checkpoint() {
    run_test("zotero_partial_page_and_final_sql_rollback_keep_cold_checkpoint",async{
        let(db,upstream,server,owner)=boot().await;upstream.model.many.store(1,Ordering::SeqCst);upstream.model.mode.store(1,Ordering::SeqCst);
        let id=connect(server.addr,&owner,"user").await;let(status,value)=sync(server.addr,&owner,id).await;assert_eq!(status,StatusCode::SERVICE_UNAVAILABLE,"{value}");
        let partial=fresh(server.addr,&owner,id).await;assert_eq!(partial["references"].as_array().unwrap().len(),25);assert_eq!(partial["connector"]["completedVersion"],"0");assert_eq!(partial["connector"]["progressVersion"],"12");assert_eq!(partial["connector"]["committedPages"],1);
        let ids:Vec<_>=partial["references"].as_array().unwrap().iter().map(|r|(r["itemKey"].clone(),r["id"].clone())).collect();
        let calls=upstream.model.log.lock().unwrap().len();
        let(status,_)=sync(server.addr,&owner,id).await;assert_eq!(status,StatusCode::CONFLICT);
        assert_eq!(upstream.model.log.lock().unwrap().len(),calls,"persisted retry deadline prevents any outbound request");
        let admin=admin_pool(&db).await;
        // Move only the synthetic fixture deadline; never sleep, retry a failed
        // assertion, or alter the product backoff policy.
        sqlx::query("UPDATE fvoci.zotero_connectors SET retry_at=now()-interval '1 second' WHERE id=$1").bind(id).execute(&admin).await.unwrap();
        sqlx::raw_sql("CREATE FUNCTION fvoci.zotero_test_final_fail() RETURNS trigger LANGUAGE plpgsql AS $$BEGIN IF NEW.completed_version>OLD.completed_version THEN RAISE EXCEPTION 'synthetic final checkpoint failure'; END IF; RETURN NEW; END$$; CREATE TRIGGER zotero_test_final_fail BEFORE UPDATE ON fvoci.zotero_connectors FOR EACH ROW EXECUTE FUNCTION fvoci.zotero_test_final_fail();").execute(&admin).await.unwrap();
        upstream.model.mode.store(0,Ordering::SeqCst);let(status,_)=sync(server.addr,&owner,id).await;assert_eq!(status,StatusCode::SERVICE_UNAVAILABLE);
        let rolled=fresh(server.addr,&owner,id).await;assert_eq!(rolled["connector"]["completedVersion"],"0");assert_eq!(rolled["references"].as_array().unwrap().len(),25);
        sqlx::raw_sql("DROP TRIGGER zotero_test_final_fail ON fvoci.zotero_connectors; DROP FUNCTION fvoci.zotero_test_final_fail();").execute(&admin).await.unwrap();admin.close().await;
        let(status,value)=sync(server.addr,&owner,id).await;assert_eq!(status,StatusCode::OK,"{value}");assert_eq!(value["references"].as_array().unwrap().len(),27);assert_eq!(value["connector"]["completedVersion"],"12");
        for(key,id) in ids{assert!(value["references"].as_array().unwrap().iter().any(|r|r["itemKey"]==key&&r["id"]==id));}
        cleanup(db,upstream,server,owner).await;
    }).await;
}

#[tokio::test]
async fn zotero_generation_retirement_barrier_rejects_old_response() {
    run_test(
        "zotero_generation_retirement_barrier_rejects_old_response",
        async {
            let (db, upstream, server, owner) = boot().await;
            let id = connect(server.addr, &owner, "user").await;
            upstream.model.mode.store(6, Ordering::SeqCst);
            let path = format!("{}/libraries/{id}/sync", root(&owner));
            let cookie = owner.session_token.clone();
            let addr = server.addr;
            let sync = tokio::spawn(async move {
                session_call(addr, Method::POST, &path, &cookie, None).await
            });
            tokio::time::timeout(Duration::from_secs(5), upstream.model.entered.notified())
                .await
                .unwrap();
            let requests = upstream.model.log.lock().unwrap().len();
            let (overlap, _) = self::sync(server.addr, &owner, id).await;
            assert_eq!(overlap, StatusCode::CONFLICT);
            assert_eq!(upstream.model.log.lock().unwrap().len(), requests);
            let (status, value) = session_call(
                addr,
                Method::DELETE,
                &format!("{}/libraries/{id}", root(&owner)),
                &owner.session_token,
                None,
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{value}");
            assert_eq!(value["generation"], "2");
            // The disconnect COMMIT is observed through a new client before release.
            let retired = fresh(server.addr, &owner, id).await;
            assert_eq!(retired["connector"]["state"], "disconnected");
            // ABA: same connector and connected state, a newer sealed key generation.
            assert_eq!(connect(server.addr, &owner, "user").await, id);
            let replaced = fresh(server.addr, &owner, id).await;
            assert_eq!(replaced["connector"]["generation"], "3");
            assert_eq!(replaced["connector"]["state"], "connected");
            upstream.model.release.notify_one();
            let (status, value) = sync.await.unwrap();
            assert_eq!(status, StatusCode::CONFLICT, "{value}");
            let after = fresh(server.addr, &owner, id).await;
            assert_eq!(after["connector"]["completedVersion"], "0");
            assert!(after["references"].as_array().unwrap().is_empty());
            upstream.model.mode.store(0, Ordering::SeqCst);
            let (status, value) = self::sync(server.addr, &owner, id).await;
            assert_eq!(status, StatusCode::OK, "{value}");
            cleanup(db, upstream, server, owner).await;
        },
    )
    .await;
}

async fn search_path(owner: &SessionFixture, query: &str, global: bool) -> String {
    let mut url = reqwest::Url::parse("http://127.0.0.1").unwrap();
    url.set_path(&if global {
        "/api/v1/search".to_owned()
    } else {
        format!("/api/v1/workspaces/{}/search", owner.workspace_id)
    });
    url.query_pairs_mut()
        .append_pair("q", query)
        .append_pair("type", "document");
    format!("{}?{}", url.path(), url.query().unwrap())
}

#[tokio::test]
async fn zotero_real_search_keeps_private_bibliography_out_of_pat_results_and_snippets() {
    run_test("zotero_real_search_keeps_private_bibliography_out_of_pat_results_and_snippets",async{
        use fvoci_server::search::{index::{rebuild_search_index,process_search_index_event},meili::MeiliConfig};
        let meili=MeiliConfig::new(std::env::var("FVOCI_MEILI_URL").expect("isolated real Meili required"),std::env::var("FVOCI_MEILI_KEY").expect("fixture Meili key required"),format!("w6_{}",Uuid::now_v7().simple()));
        let(db,upstream,server,owner)=boot_with_search(Some(meili.clone())).await;
        let id=connect(server.addr,&owner,"user").await;
        let(status,initial)=sync(server.addr,&owner,id).await;assert_eq!(status,StatusCode::OK,"{initial}");
        let document=Uuid::parse_str(initial["references"][0]["id"].as_str().unwrap()).unwrap();
        let path=document_api(owner.workspace_id,document,"");
        let(status,value)=session_call(server.addr,Method::PATCH,&path,&owner.session_token,Some(json!({"title":"Authored astronomy 개인 의견"}))).await;
        assert_eq!(status,StatusCode::OK,"{value}");
        let authored=json!({"type":"doc","content":[{"type":"paragraph","attrs":{"id":"owned-commentary"},"content":[{"type":"text","text":"My authored telescope notes. 개인 의견은 유지됩니다."}]}]});
        let(status,value)=session_call(server.addr,Method::PUT,&document_api(owner.workspace_id,document,"/body"),&owner.session_token,Some(json!({"contentJson":authored}))).await;
        assert_eq!(status,StatusCode::OK,"{value}");
        rebuild_search_index(&owner.pool,&meili,Some(owner.workspace_id)).await.unwrap();
        let pat=insert_pat(&db,owner.workspace_id,owner.user_id,&["documents.read","tasks.read"]).await;
        // Expected strings are literal upstream values, never adapter-derived.
        for query in ["합성 연구 자료","9780000000000","Synthetic Press"] {
            for global in [false,true] {
                let path=search_path(&owner,query,global).await;
                let(status,value)=session_call(server.addr,Method::GET,&path,&owner.session_token,None).await;
                assert_eq!(status,StatusCode::OK,"{query}: {value}");
                assert!(value["items"].as_array().unwrap().iter().any(|item|item["id"]==document.to_string()),"cookie bibliography result: {query}: {value}");
                let(status,value)=call(server.addr,Method::GET,&path,Cred::Bearer(&pat),None).await;
                assert_eq!(status,StatusCode::OK,"{value}");
                assert_eq!(value["items"],json!([]),"PAT result-existence leak: {query}: {value}");
                assert!(!value.to_string().contains("합성 연구 자료"));
            }
        }
        for query in ["astronmy","telescop","ㄱㅇ ㅇㄱ"] {
            let(status,value)=call(server.addr,Method::GET,&search_path(&owner,query,false).await,Cred::Bearer(&pat),None).await;
            assert_eq!(status,StatusCode::OK,"{value}");
            assert!(value["items"].as_array().unwrap().iter().any(|item|item["id"]==document.to_string()),"authored typo/stem/chosung lost: {query}: {value}");
            for secret in ["합성 연구 자료","Synthetic Press","9780000000000",KEY] {assert!(!value.to_string().contains(secret),"PAT snippet leak: {secret}: {value}");}
        }
        let outsider=create_user_session(&db,owner.workspace_id,WorkspaceRole::Member).await;
        let(status,value)=session_call(server.addr,Method::GET,&search_path(&owner,"합성",true).await,&outsider.session_token,None).await;
        assert_eq!(status,StatusCode::OK,"{value}");assert_eq!(value["items"],json!([]));
        let(status,value)=session_call(server.addr,Method::GET,&search_path(&owner,"astronomy",false).await,&outsider.session_token,None).await;
        assert_eq!(status,StatusCode::NOT_FOUND,"inconsistent personal membership cannot grant ownership: {value}");
        let admin=admin_pool(&db).await;
        let before:i64=sqlx::query_scalar("SELECT COALESCE(max(seq),0) FROM fvoci.events").fetch_one(&admin).await.unwrap();
        upstream.model.mode.store(14,Ordering::SeqCst);
        let(status,updated)=sync(server.addr,&owner,id).await;assert_eq!(status,StatusCode::OK,"{updated}");
        assert_eq!(updated["references"][0]["id"],document.to_string());assert_eq!(updated["references"][0]["bibliography"]["title"],"Revised synthetic bibliography");
        let event_id:Uuid=sqlx::query_scalar("SELECT id FROM fvoci.events WHERE seq>$1 AND workspace_id=$2 AND target_id=$3 AND verb='document.updated' ORDER BY seq DESC LIMIT 1").bind(before).bind(owner.workspace_id).bind(document).fetch_one(&admin).await.unwrap();
        admin.close().await;
        let event=fvoci_server::db::outbox::fetch_event_by_id(&owner.pool,event_id).await.unwrap().unwrap();
        assert!(!event.payload.to_string().contains(KEY));
        process_search_index_event(&owner.pool,&meili,&event).await.unwrap();
        let(status,value)=session_call(server.addr,Method::GET,&search_path(&owner,"Revised synthetic bibliography",false).await,&owner.session_token,None).await;
        assert_eq!(status,StatusCode::OK,"{value}");assert!(value["items"].as_array().unwrap().iter().any(|item|item["id"]==document.to_string()));
        let(status,value)=session_call(server.addr,Method::GET,&document_api(owner.workspace_id,document,"/body"),&owner.session_token,None).await;
        assert_eq!(status,StatusCode::OK,"{value}");assert_eq!(value["contentJson"],authored,"metadata refresh preserves actual native authored body");
        let(status,value)=call(server.addr,Method::GET,&search_path(&owner,"astronmy",false).await,Cred::Bearer(&pat),None).await;
        assert_eq!(status,StatusCode::OK,"{value}");assert!(value["items"].as_array().unwrap().iter().any(|item|item["id"]==document.to_string()));assert!(!value.to_string().contains("Revised synthetic bibliography"));
        let response=reqwest::Client::new().delete(format!("{}/indexes/{}",meili.url,meili.index_uid)).bearer_auth(meili.api_key()).send().await.unwrap();assert!(response.status().is_success());
        let task:Value=response.json().await.unwrap();fvoci_server::search::meili::wait_meili_tasks(&meili,&[task["taskUid"].as_u64().unwrap()]).await.unwrap();
        outsider.pool.close().await;cleanup(db,upstream,server,owner).await;
    }).await;
}

#[tokio::test]
async fn zotero_inflight_commit_rechecks_current_session_and_owner_permission() {
    run_test("zotero_inflight_commit_rechecks_current_session_and_owner_permission",async{
        for revoke_session in [true,false] {
            let(db,upstream,server,owner)=boot().await;
            let id=connect(server.addr,&owner,"user").await;
            upstream.model.mode.store(6,Ordering::SeqCst);
            let addr=server.addr;let cookie=owner.session_token.clone();let path=format!("{}/libraries/{id}/sync",root(&owner));
            let pending=tokio::spawn(async move{session_call(addr,Method::POST,&path,&cookie,None).await});
            tokio::time::timeout(Duration::from_secs(5),upstream.model.entered.notified()).await.unwrap();
            let admin=admin_pool(&db).await;
            if revoke_session {sqlx::query("UPDATE fvoci.sessions SET revoked_at=now() WHERE id=$1").bind(owner.session_id).execute(&admin).await.unwrap();}
            else {sqlx::query("DELETE FROM fvoci.memberships WHERE workspace_id=$1 AND user_id=$2").bind(owner.workspace_id).bind(owner.user_id).execute(&admin).await.unwrap();}
            admin.close().await;
            upstream.model.release.notify_one();
            let(status,value)=pending.await.unwrap();assert!(matches!(status,StatusCode::FORBIDDEN|StatusCode::NOT_FOUND|StatusCode::UNAUTHORIZED),"retired current authority: {status}: {value}");
            let mut conn=observer(&db,&owner).await;
            let state:(i64,i64)=sqlx::query_as("SELECT completed_version,(SELECT count(*) FROM fvoci.zotero_references WHERE connector_id=$1) FROM fvoci.zotero_connectors WHERE id=$1").bind(id).fetch_one(&mut conn).await.unwrap();assert_eq!(state,(0,0));drop(conn);
            cleanup(db,upstream,server,owner).await;
        }
    }).await;
}

#[tokio::test]
async fn zotero_versions_above_browser_integer_precision_remain_exact_strings() {
    run_test(
        "zotero_versions_above_browser_integer_precision_remain_exact_strings",
        async {
            let (db, upstream, server, owner) = boot().await;
            upstream.model.mode.store(15, Ordering::SeqCst);
            let id = connect(server.addr, &owner, "user").await;
            let (status, value) = sync(server.addr, &owner, id).await;
            assert_eq!(status, StatusCode::OK, "{value}");
            assert_eq!(value["connector"]["completedVersion"], "9007199254740993");
            assert_eq!(
                fresh(server.addr, &owner, id).await["connector"]["completedVersion"],
                "9007199254740993"
            );
            let mut conn = observer(&db, &owner).await;
            let version: i64 = sqlx::query_scalar(
                "SELECT completed_version FROM fvoci.zotero_connectors WHERE id=$1",
            )
            .bind(id)
            .fetch_one(&mut conn)
            .await
            .unwrap();
            assert_eq!(version, 9007199254740993);
            drop(conn);
            cleanup(db, upstream, server, owner).await;
        },
    )
    .await;
}

#[tokio::test]
async fn zotero_imported_title_and_escaped_secret_fail_before_metadata_commit() {
    run_test("zotero_imported_title_and_escaped_secret_fail_before_metadata_commit",async{
        let(db,upstream,server,owner)=boot().await;let id=connect(server.addr,&owner,"user").await;
        let(status,initial)=sync(server.addr,&owner,id).await;assert_eq!(status,StatusCode::OK);
        let document=Uuid::parse_str(initial["references"][0]["id"].as_str().unwrap()).unwrap();
        // Authored text is allowed to contain this synthetic literal. The
        // credential policy applies only to imported upstream metadata.
        let content=json!({"type":"doc","content":[{"type":"paragraph","attrs":{"id":"literal-authored-text"},"content":[{"type":"text","text":KEY}]}]});
        let(status,_)=session_call(server.addr,Method::PUT,&document_api(owner.workspace_id,document,"/body"),&owner.session_token,Some(json!({"contentJson":content}))).await;assert_eq!(status,StatusCode::OK);
        let mut observed=Vec::new();
        for mode in [16,17,18,19,20] {
            assert_eq!(connect(server.addr,&owner,"user").await,id);
            upstream.model.mode.store(mode,Ordering::SeqCst);
            let(status,_)=sync(server.addr,&owner,id).await;
            let current=fresh(server.addr,&owner,id).await;
            observed.push((mode,status,current["connector"]["completedVersion"]=="12",current["references"][0]["bibliography"]["title"]=="합성 연구 자료 🙂",!current["references"][0]["bibliography"].to_string().contains(KEY)));
        }
        let(status,body)=session_call(server.addr,Method::GET,&document_api(owner.workspace_id,document,"/body"),&owner.session_token,None).await;
        let body_preserved=status==StatusCode::OK&&body["contentJson"]==content;
        upstream.model.mode.store(21,Ordering::SeqCst);
        assert_eq!(connect(server.addr,&owner,"user").await,id);
        let(absent_status,absent)=sync(server.addr,&owner,id).await;
        let absent_allowed=absent_status==StatusCode::OK&&absent["references"][0]["bibliography"]["title"]=="";
        cleanup(db,upstream,server,owner).await;
        assert_eq!(observed,[16,17,18,19,20].into_iter().map(|mode|(mode,StatusCode::BAD_REQUEST,true,true,true)).collect::<Vec<_>>());
        assert!(body_preserved);
        assert!(absent_allowed);
    }).await;
}

#[tokio::test]
async fn zotero_learned_backoff_survives_key_replacement_disconnect_and_other_library() {
    run_test(
        "zotero_learned_backoff_survives_key_replacement_disconnect_and_other_library",
        async {
            let (db, upstream, server, owner) = boot().await;
            let id = connect(server.addr, &owner, "user").await;
            upstream.model.mode.store(4, Ordering::SeqCst);
            let (status, _) = sync(server.addr, &owner, id).await;
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
            let deadline = fresh(server.addr, &owner, id).await["connector"]["retryAt"].clone();
            assert!(deadline.is_string());
            assert_eq!(connect(server.addr, &owner, "user").await, id);
            let reconnect_keeps =
                fresh(server.addr, &owner, id).await["connector"]["retryAt"] == deadline;
            let (status, disconnected) = session_call(
                server.addr,
                Method::DELETE,
                &format!("{}/libraries/{id}", root(&owner)),
                &owner.session_token,
                None,
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            let disconnect_keeps = disconnected["retryAt"] == deadline;
            let group = connect(server.addr, &owner, "group").await;
            let group_deadline =
                fresh(server.addr, &owner, group).await["connector"]["retryAt"] == deadline;
            upstream.model.mode.store(0, Ordering::SeqCst);
            let before = upstream.model.log.lock().unwrap().len();
            let (status, _) = sync(server.addr, &owner, group).await;
            let no_early_get = upstream.model.log.lock().unwrap().len() == before;
            cleanup(db, upstream, server, owner).await;
            assert_eq!(
                (
                    reconnect_keeps,
                    disconnect_keeps,
                    group_deadline,
                    status,
                    no_early_get
                ),
                (true, true, true, StatusCode::CONFLICT, true)
            );
        },
    )
    .await;
}

#[tokio::test]
async fn zotero_same_actor_fixed_host_allows_one_active_library_read_cycle() {
    run_test(
        "zotero_same_actor_fixed_host_allows_one_active_library_read_cycle",
        async {
            let (db, upstream, server, owner) = boot().await;
            let primary = connect(server.addr, &owner, "user").await;
            let secondary = connect(server.addr, &owner, "group").await;
            upstream.model.mode.store(6, Ordering::SeqCst);
            let addr = server.addr;
            let cookie = owner.session_token.clone();
            let path = format!("{}/libraries/{primary}/sync", root(&owner));
            let held = tokio::spawn(async move {
                session_call(addr, Method::POST, &path, &cookie, None).await
            });
            tokio::time::timeout(Duration::from_secs(5), upstream.model.entered.notified())
                .await
                .unwrap();
            let before = upstream.model.log.lock().unwrap().len();
            // Observe whether an overlapping start enters the held upstream. Both
            // requests release before assertions and cleanup in either outcome.
            let overlap =
                tokio::time::timeout(Duration::from_secs(1), sync(addr, &owner, secondary))
                    .await
                    .ok()
                    .map(|(status, _)| status);
            let no_extra_get = upstream.model.log.lock().unwrap().len() == before;
            upstream.model.release.notify_waiters();
            let (status, _) = tokio::time::timeout(Duration::from_secs(5), held)
                .await
                .unwrap()
                .unwrap();
            cleanup(db, upstream, server, owner).await;
            assert_eq!(status, StatusCode::OK, "valid-before-retirement control");
            assert_eq!((overlap, no_extra_get), (Some(StatusCode::CONFLICT), true));
        },
    )
    .await;
}

#[tokio::test]
async fn zotero_retiring_credentials_keeps_host_lease_until_held_get_finishes() {
    run_test(
        "zotero_retiring_credentials_keeps_host_lease_until_held_get_finishes",
        async {
            let mut observed = Vec::new();
            for replace in [false, true] {
                for other_library in [false, true] {
                    let (db, upstream, server, owner) = boot().await;
                    let primary = connect(server.addr, &owner, "user").await;
                    let secondary = connect(server.addr, &owner, "group").await;
                    upstream.model.mode.store(6, Ordering::SeqCst);
                    let addr = server.addr;
                    let cookie = owner.session_token.clone();
                    let path = format!("{}/libraries/{primary}/sync", root(&owner));
                    let held = tokio::spawn(async move {
                        session_call(addr, Method::POST, &path, &cookie, None).await
                    });
                    tokio::time::timeout(Duration::from_secs(5), upstream.model.entered.notified())
                        .await
                        .unwrap();
                    if replace {
                        assert_eq!(connect(addr, &owner, "user").await, primary);
                    } else {
                        let (status, _) = session_call(
                            addr,
                            Method::DELETE,
                            &format!("{}/libraries/{primary}", root(&owner)),
                            &owner.session_token,
                            None,
                        )
                        .await;
                        assert_eq!(status, StatusCode::OK);
                        if !other_library {
                            assert_eq!(connect(addr, &owner, "user").await, primary);
                        }
                    }
                    // Retirement is committed and visible before the attempted
                    // new read. The old upstream GET is still held at this point.
                    assert_eq!(
                        fresh(addr, &owner, primary).await["connector"]["completedVersion"],
                        "0"
                    );
                    let target = if other_library { secondary } else { primary };
                    let before = upstream.model.log.lock().unwrap().len();
                    let cookie = owner.session_token.clone();
                    let path = format!("{}/libraries/{target}/sync", root(&owner));
                    let mut attempt = tokio::spawn(async move {
                        session_call(addr, Method::POST, &path, &cookie, None).await
                    });
                    let early_status = tokio::time::timeout(Duration::from_secs(1), &mut attempt)
                        .await
                        .ok()
                        .map(|result| result.unwrap().0);
                    let no_early_get = upstream.model.log.lock().unwrap().len() == before;
                    upstream.model.release.notify_waiters();
                    let retired_status = tokio::time::timeout(Duration::from_secs(5), held)
                        .await
                        .unwrap()
                        .unwrap()
                        .0;
                    if early_status.is_none() {
                        // A failing admission may have left a second GET held.
                        // Release and join it before any assertion or cleanup.
                        tokio::time::timeout(Duration::from_secs(5), attempt)
                            .await
                            .unwrap()
                            .unwrap();
                    }
                    upstream.model.mode.store(0, Ordering::SeqCst);
                    let (after_release, _) = sync(addr, &owner, target).await;
                    cleanup(db, upstream, server, owner).await;
                    observed.push((early_status, no_early_get, retired_status, after_release));
                }
            }
            assert_eq!(
                observed,
                vec![
                    (
                        Some(StatusCode::CONFLICT),
                        true,
                        StatusCode::CONFLICT,
                        StatusCode::OK
                    );
                    4
                ]
            );
        },
    )
    .await;
}

#[tokio::test]
async fn zotero_retained_collection_union_admission_keeps_cold_library_readable() {
    run_test("zotero_retained_collection_union_admission_keeps_cold_library_readable",async{
        let(db,upstream,server,owner)=boot().await;let id=connect(server.addr,&owner,"user").await;
        let alphabet=b"23456789ABCDEFGHIJKLMNPQRSTUVWXYZ";let base=alphabet.len();
        let keys:Vec<String>=(0..2000).map(|i|format!("JKLM2{}{}{}",alphabet[(i/base/base)%base] as char,alphabet[(i/base)%base] as char,alphabet[i%base] as char)).collect();
        let mut conn=observer(&db,&owner).await;
        sqlx::query("INSERT INTO fvoci.zotero_collections(workspace_id,owner_user_id,connector_id,collection_key,remote_version,name,availability) SELECT $1,$2,$3,key,1,'Historical','deleted' FROM unnest($4::text[]) AS key").bind(owner.workspace_id).bind(owner.user_id).bind(id).bind(keys).execute(&mut conn).await.unwrap();
        sqlx::query("COMMIT").execute(&mut conn).await.unwrap();conn.close().await.unwrap();
        assert_eq!(fresh(server.addr,&owner,id).await["collections"].as_array().unwrap().len(),2000);
        let(status,_)=sync(server.addr,&owner,id).await;
        let mut conn=observer(&db,&owner).await;
        let state:(i64,i64,i64)=sqlx::query_as("SELECT completed_version,(SELECT count(*) FROM fvoci.zotero_collections WHERE connector_id=$1),(SELECT count(*) FROM fvoci.zotero_references WHERE connector_id=$1) FROM fvoci.zotero_connectors WHERE id=$1").bind(id).fetch_one(&mut conn).await.unwrap();drop(conn);
        let(read_status,_)=session_call(server.addr,Method::GET,&format!("{}/libraries/{id}",root(&owner)),&owner.session_token,None).await;
        cleanup(db,upstream,server,owner).await;
        assert_eq!((status,state,read_status),(StatusCode::BAD_REQUEST,(0,2000,0),StatusCode::OK));
    }).await;
}
