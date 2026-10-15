//! Supplemental native after-validation race, distinct from normal-binary UI18.
//! Uses the existing test-only barrier; no production switch or HTTP test route.
use super::{
    arm_append_revoke_barrier, auth_and_join, connect_member, disarm_append_revoke_barrier,
    engine_expectations, engine_fixture, expect_persisted, room_key, start_test_server,
    stateless_frame, sync_step1_frame, sync_update_frame, test_collab_config,
    tiptap_xml_pending_u1, try_connect_member, DocumentMessage, HelperChildCapacityHold,
    SyncMessage, SyncStep, TestServer, WireFrame, FIXTURE_PASSWORD, PEPPER, PUBLIC_ORIGIN,
};
use futures_util::{FutureExt, SinkExt, StreamExt};
use fvoci_server::auth::{password::Keyring, AuthService};
use fvoci_server::collab::{config::FamilyRoomTimings, CollabHub};
use fvoci_server::db::{backend::Backend, e2e_fixture, migrate, pool, Db};
use fvoci_server::http::{rate_limit::RateLimiter, state::AppState};
use reqwest::{Method, Response, StatusCode};
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc, time::Duration};
use uuid::Uuid;

enum Readback {
    Sqlite(sqlx::SqlitePool),
    Remote(Backend),
}

impl Readback {
    async fn snapshot(&self, workspace: Uuid, document: Uuid) -> Value {
        match self {
            Self::Sqlite(observer) => {
                // A separate read-only pool and one fresh read transaction.
                // Fixed target-specific queries only; no arbitrary SQL interface.
                let mut tx = observer.begin().await.unwrap();
                let state: (Vec<u8>, i64, i64, i64) = sqlx::query_as(
                    "SELECT state,writer_generation,snapshot_cutoff_seq,tail_seq FROM document_states WHERE workspace_id=?1 AND document_id=?2",
                ).bind(workspace.as_bytes().as_slice()).bind(document.as_bytes().as_slice()).fetch_one(&mut *tx).await.unwrap();
                let body: (String, String, i64) = sqlx::query_as(
                    "SELECT content_json,text,version FROM documents WHERE workspace_id=?1 AND id=?2",
                ).bind(workspace.as_bytes().as_slice()).bind(document.as_bytes().as_slice()).fetch_one(&mut *tx).await.unwrap();
                let updates: Vec<(i64, Vec<u8>, Vec<u8>)> = sqlx::query_as(
                    "SELECT seq,op_id,payload FROM document_collab_updates WHERE workspace_id=?1 AND document_id=?2 ORDER BY seq LIMIT 10001",
                ).bind(workspace.as_bytes().as_slice()).bind(document.as_bytes().as_slice()).fetch_all(&mut *tx).await.unwrap();
                let receipts: Vec<(i64, Vec<u8>, i64, Vec<u8>, Vec<u8>)> = sqlx::query_as(
                    "SELECT seq,op_id,payload_len,payload_sha256,actor_user_id FROM document_collab_op_receipts WHERE workspace_id=?1 AND document_id=?2 ORDER BY seq,op_id LIMIT 10001",
                ).bind(workspace.as_bytes().as_slice()).bind(document.as_bytes().as_slice()).fetch_all(&mut *tx).await.unwrap();
                let revisions: Vec<(Vec<u8>, Vec<u8>, String, String, String, i64)> = sqlx::query_as(
                    "SELECT id,y_snapshot,content_json,text,reason,created_at FROM revisions WHERE workspace_id=?1 AND target_id=?2 AND target_kind='document' ORDER BY created_at,id LIMIT 10001",
                ).bind(workspace.as_bytes().as_slice()).bind(document.as_bytes().as_slice()).fetch_all(&mut *tx).await.unwrap();
                let events: Vec<(Vec<u8>, i64, String, String, String)> = sqlx::query_as(
                    "SELECT id,seq,verb,payload,channel FROM events WHERE workspace_id=?1 AND target_id=?2 ORDER BY seq LIMIT 10001",
                ).bind(workspace.as_bytes().as_slice()).bind(document.as_bytes().as_slice()).fetch_all(&mut *tx).await.unwrap();
                tx.rollback().await.unwrap();
                for count in [updates.len(), receipts.len(), revisions.len(), events.len()] {
                    assert!(count <= 10000, "native no-effect observation was truncated");
                }
                json!({"state":state,"body":body,"updates":updates,"receipts":receipts,"revisions":revisions,"events":events})
            }
            Self::Remote(observer) => {
                // Reuse the maintained fresh current-SDK read-only observations.
                let observed = e2e_fixture::observe_native(observer, workspace, &[document])
                    .await
                    .unwrap_or_else(|_| panic!("remote native readback failed"));
                let row = &observed["rows"][document.to_string()];
                assert!(
                    row.is_object(),
                    "missing remote native document observation"
                );
                let mut native = row.as_object().unwrap().clone();
                // Lease renewal is independent of the rejected mutation's effects.
                assert!(native.remove("roomFences").is_some());
                let baseline = e2e_fixture::capture_baseline(observer)
                    .await
                    .unwrap_or_else(|_| panic!("remote event readback failed"));
                let allocations = baseline["allocations"]["events"]
                    .as_object()
                    .expect("missing native event allocations");
                let hashes = baseline["fingerprints"]["events"]
                    .as_object()
                    .expect("missing native event fingerprints");
                let workspace = hex::encode(workspace.as_bytes());
                let events = allocations
                    .iter()
                    .filter(|(_, refs)| {
                        refs["workspaces"]
                            .as_array()
                            .expect("missing native event scope")
                            .contains(&Value::String(workspace.clone()))
                    })
                    .map(|(hash, _)| {
                        (
                            hash.clone(),
                            hashes
                                .get(hash)
                                .expect("missing native event row hash")
                                .clone(),
                        )
                    })
                    .collect::<serde_json::Map<_, _>>();
                json!({"native":native,"events":events})
            }
        }
    }

    async fn close(&self) -> bool {
        match self {
            Self::Sqlite(pool) => {
                pool.close().await;
                true
            }
            Self::Remote(backend) => backend.close().await.is_ok(),
        }
    }
}

async fn api(
    server: &TestServer,
    method: Method,
    path: &str,
    body: Option<Value>,
    cookie: Option<&str>,
) -> Response {
    let mut request = fvoci_server::http_client::client()
        .request(method, format!("http://{}{}", server.addr, path))
        .header("origin", PUBLIC_ORIGIN);
    if let Some(cookie) = cookie {
        request = request.header("cookie", format!("fvoci_session={cookie}"));
    }
    if let Some(body) = body {
        request = request.json(&body);
    }
    request
        .send()
        .await
        .unwrap_or_else(|_| panic!("auth native HTTP request failed"))
}

fn cookie(response: &Response) -> String {
    response
        .headers()
        .get("set-cookie")
        .expect("missing normal auth cookie")
        .to_str()
        .expect("invalid normal auth cookie")
        .split(';')
        .next()
        .and_then(|value| value.strip_prefix("fvoci_session="))
        .expect("missing normal session cookie")
        .to_owned()
}

async fn app(backend: Backend, directory: &std::path::Path) -> TestServer {
    let config = test_collab_config(4, 30_000);
    let hold = HelperChildCapacityHold::reserve(1, &config).await;
    let hub = CollabHub::new_backend(
        config,
        backend.clone(),
        Some(FamilyRoomTimings::new(30_000, 5_000).unwrap()),
    )
    .unwrap();
    let state = AppState {
        realtime_mode: fvoci_server::config::RealtimeMode::On,
        native_engine: None,
        auth: Arc::new(AuthService {
            db: Db::from_backend(backend),
            password_keys: Keyring::parse(PEPPER, "test").unwrap(),
        }),
        branding_name: "FVOCI".into(),
        public_origin: PUBLIC_ORIGIN.into(),
        cookie_secure: false,
        rate_limiter: RateLimiter::new(),
        storage: fvoci_server::attachments::LocalStorage::new(directory.join("storage")).into(),
        upload: fvoci_server::attachments::UploadLimits {
            part_size_bytes: fvoci_server::config::DEFAULT_UPLOAD_PART_SIZE_BYTES,
            max_file_size_bytes: fvoci_server::config::DEFAULT_UPLOAD_MAX_FILE_SIZE_BYTES,
            create_rate_per_5min: fvoci_server::config::DEFAULT_UPLOAD_CREATE_RATE_PER_5MIN,
            part_put_slots: fvoci_server::attachments::PartPutSlots::new(
                fvoci_server::config::DEFAULT_UPLOAD_MAX_CONCURRENT_PARTS,
            ),
        },
        collab: Some(Arc::new(hub)),
        meili: None,
        search_embedder: None,
        markdown: None,
        import_wake: None,
        import_extractor_available: false,
        preview_extract: None,
        quota: Default::default(),
        streams: AppState::fresh_streams(),
        mailer: Arc::new(fvoci_server::mail::Mailer::disabled()),
    };
    start_test_server(state, Some(hold)).await
}

async fn reader_sync_fence(
    reader: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    routing: &str,
    reject_updates: bool,
) -> Vec<u8> {
    reader
        .send(super::Message::Binary(
            sync_step1_frame(routing, &[0, 0]).into(),
        ))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(frame) = reader.next().await {
            match frame.expect("native reader fence frame") {
                super::Message::Binary(bytes) => {
                    let WireFrame::Document {
                        routing_key,
                        message,
                        ..
                    } = fvoci_server::collab::wire::decode(&bytes)
                        .expect("valid native reader fence codec")
                    else {
                        panic!("unexpected native reader fence frame");
                    };
                    assert_eq!(routing_key, routing, "reader fence changed document");
                    match message {
                        DocumentMessage::Sync(SyncMessage {
                            step: SyncStep::Update,
                            ..
                        }) if reject_updates => {
                            panic!("rejected update reached the healthy reader before its fence")
                        }
                        DocumentMessage::Sync(SyncMessage {
                            step: SyncStep::Step2,
                            y_protocol,
                        }) => return y_protocol,
                        _ => {}
                    }
                }
                super::Message::Ping(_) | super::Message::Pong(_) => {}
                super::Message::Close(_) => panic!("healthy reader closed before its fence"),
                _ => panic!("unexpected native reader websocket frame"),
            }
        }
        panic!("missing native reader Step2 fence");
    })
    .await
    .expect("native reader Step2 fence did not arrive")
}

async fn case(server: &TestServer, readback: &Readback) {
    let initial = api(server, Method::GET, "/api/v1/setup", None, None).await;
    assert_eq!(initial.status(), StatusCode::OK);
    assert_eq!(
        initial.json::<Value>().await.unwrap(),
        json!({"branding":{"name":"FVOCI"},"mailEnabled":false,"needed":true,"realtimeMode":"on"}),
        "requires this unit's fresh migrated allocation"
    );
    let setup = api(
        server,
        Method::POST,
        "/api/v1/setup",
        Some(json!({
            "email":"auth-family@example.invalid","password":FIXTURE_PASSWORD,"givenName":"Auth",
            "workspaceSlug":"auth-family","workspaceName":"Auth family"
        })),
        None,
    )
    .await;
    assert_eq!(setup.status(), StatusCode::CREATED);
    let ending = cookie(&setup);
    let setup = setup.json::<Value>().await.unwrap();
    let workspace = Uuid::parse_str(setup["workspaceId"].as_str().unwrap()).unwrap();
    let login = api(
        server,
        Method::POST,
        "/api/v1/auth/login",
        Some(json!({"email":"auth-family@example.invalid","password":FIXTURE_PASSWORD})),
        None,
    )
    .await;
    assert_eq!(login.status(), StatusCode::OK);
    let retained = cookie(&login);
    assert!(
        ending != retained,
        "normal logins must yield distinct cookies"
    );
    let ending_me = api(server, Method::GET, "/api/v1/auth/me", None, Some(&ending))
        .await
        .json::<Value>()
        .await
        .unwrap();
    let retained_me = api(
        server,
        Method::GET,
        "/api/v1/auth/me",
        None,
        Some(&retained),
    )
    .await
    .json::<Value>()
    .await
    .unwrap();
    assert_eq!(ending_me["userId"], retained_me["userId"]);
    assert_ne!(ending_me["sessionId"], retained_me["sessionId"]);
    let created = api(
        server,
        Method::POST,
        &format!("/api/v1/workspaces/{workspace}/documents"),
        Some(json!({"commandId":Uuid::now_v7(),"title":"auth family race","parentId":null})),
        Some(&retained),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let document = Uuid::parse_str(
        created.json::<Value>().await.unwrap()["id"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    let routing = room_key(workspace, document);
    let mut reader = connect_member(server.addr, &retained).await;
    auth_and_join(&mut reader, &routing, 361).await;
    let mut writer = connect_member(server.addr, &ending).await;
    auth_and_join(&mut writer, &routing, 362).await;
    // Authenticated alone sends no client Step1. Drain startup traffic through
    // our first Step2 so no old response can satisfy the post-rejection fence.
    let initial_reader_state = reader_sync_fence(&mut reader, &routing, false).await;
    let (reached, release) = arm_append_revoke_barrier(document).await;
    let rejected = tiptap_xml_pending_u1();
    writer
        .send(super::Message::Binary(
            sync_update_frame(&routing, &rejected).into(),
        ))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), reached)
        .await
        .expect("after-validation barrier not reached")
        .unwrap();
    let before = readback.snapshot(workspace, document).await;
    let logout = api(
        server,
        Method::POST,
        "/api/v1/auth/logout",
        None,
        Some(&ending),
    )
    .await;
    assert_eq!(logout.status(), StatusCode::NO_CONTENT);
    release.send(()).expect("release after real logout commit");
    disarm_append_revoke_barrier(document).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(frame) = writer.next().await {
            match frame.expect("native writer frame") {
                super::Message::Close(Some(close)) => {
                    assert_eq!(u16::from(close.code), 1008);
                    return;
                }
                super::Message::Close(None) => panic!("missing actual server close code"),
                _ => {}
            }
        }
        panic!("missing actual server revocation close");
    })
    .await
    .expect("revoked native writer did not close");
    assert_eq!(
        readback.snapshot(workspace, document).await,
        before,
        "rejected validated update changed native/body/revision/event state"
    );
    assert_eq!(
        api(server, Method::GET, "/api/v1/auth/me", None, Some(&ending))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    match try_connect_member(server.addr, &ending).await {
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
            assert_eq!(response.status().as_u16(), 401)
        }
        _ => panic!("revoked cookie did not refuse the fresh handshake"),
    }
    assert_eq!(
        api(
            server,
            Method::GET,
            "/api/v1/auth/me",
            None,
            Some(&retained)
        )
        .await
        .status(),
        StatusCode::OK
    );
    let rejected_reader_state = reader_sync_fence(&mut reader, &routing, true).await;
    assert_eq!(
        rejected_reader_state, initial_reader_state,
        "rejected update changed the native state exposed to the healthy reader"
    );
    // A distinct complete fixture keeps a leaked rejected payload from being
    // mistaken for the subsequent authorized update or its fresh readback.
    let healthy = engine_fixture("marks_link_bold.v1");
    let healthy_body = engine_expectations()["marks_link_bold"]["prosemirror_json"].clone();
    assert_ne!(healthy, rejected);
    reader
        .send(super::Message::Binary(
            sync_update_frame(&routing, &healthy).into(),
        ))
        .await
        .unwrap();
    assert!(
        super::wait_for_sync_applied(&mut reader, Duration::from_secs(5)).await,
        "retained session cannot apply a healthy update"
    );
    let request = Uuid::now_v7();
    reader
        .send(super::Message::Binary(
            stateless_frame(&routing, &format!("persist:{request}")).into(),
        ))
        .await
        .unwrap();
    expect_persisted(
        &mut reader,
        request,
        Duration::from_secs(5),
        "retained session after logout refusal",
    )
    .await;
    let body = api(
        server,
        Method::GET,
        &format!("/api/v1/workspaces/{workspace}/documents/{document}/body"),
        None,
        Some(&retained),
    )
    .await;
    assert_eq!(body.status(), StatusCode::OK);
    assert_eq!(
        body.json::<Value>().await.unwrap()["contentJson"],
        healthy_body
    );
    assert_ne!(
        readback.snapshot(workspace, document).await,
        before,
        "healthy retained save had no durable effect"
    );
    reader.close(None).await.unwrap();
}

async fn run(backend: Backend, readback: Readback, directory: PathBuf) {
    let server = app(backend.clone(), &directory).await;
    let addr = server.addr;
    let result = std::panic::AssertUnwindSafe(case(&server, &readback))
        .catch_unwind()
        .await;
    server.shutdown().await;
    let closed = tokio::net::TcpStream::connect(addr).await.is_err();
    let observer_closed = readback.close().await;
    let app_closed = backend.close().await.is_ok();
    eprintln!(
        "AUTH_FAMILY_CLOSURE listener={closed} observer={observer_closed} backend={app_closed}"
    );
    if let Err(original) = result {
        if !(closed && observer_closed && app_closed) {
            eprintln!("AUTH_FAMILY_CLEANUP_FAILED");
        }
        std::panic::resume_unwind(original);
    }
    assert!(
        closed && observer_closed && app_closed,
        "auth family resource closure failed"
    );
}

#[tokio::test]
async fn sqlite_logout_after_validation_refuses_without_effects() {
    let directory = std::env::temp_dir().join(format!("fvoci-auth-family-{}", Uuid::now_v7()));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("app.sqlite");
    migrate::run_sqlite_migrations(&path).await.unwrap();
    let admission = migrate::SqliteAdmission::server(&path).unwrap();
    let backend = Backend::Sqlite(pool::connect_sqlite_app(&path, 4).await.unwrap());
    let observer = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            sqlx::sqlite::SqliteConnectOptions::new()
                .filename(&path)
                .read_only(true),
        )
        .await
        .unwrap();
    run(backend, Readback::Sqlite(observer), directory.clone()).await;
    drop(admission);
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
#[ignore = "requires controller-owned local sqld/TLS allocation and explicit exact selection; never hosted evidence"]
async fn local_libsql_logout_after_validation_refuses_without_effects() {
    let args: Vec<String> = std::env::args().collect();
    assert!(args.iter().any(|arg| arg == "--ignored") && args.iter().any(|arg| arg == "--exact") &&
        args.iter().any(|arg| arg == "auth_session_family::local_libsql_logout_after_validation_refuses_without_effects"),
        "requires exact manual test selection");
    assert_eq!(
        std::env::var("FVOCI_SELECTED_EXECUTION_MODE").as_deref(),
        Ok("orca-local"),
        "requires the controller's local allocation; environment selection is not a grant"
    );
    // This manual exact test is its own process; hide credentials before loading
    // them without changing the process-wide setting of ordinary parallel tests.
    collab_engine::process::make_process_non_dumpable().expect("native auth credential boundary");
    let settings = fvoci_server::config::DatabaseSettings::from_env()
        .unwrap_or_else(|_| panic!("missing local remote settings"));
    assert!(
        matches!(
            settings,
            fvoci_server::config::DatabaseSettings::LibsqlRemote { .. }
        ),
        "requires the actual remote backend"
    );
    let backend = migrate::connect_remote_app(&settings, 4)
        .await
        .unwrap_or_else(|_| panic!("local remote app admission failed"));
    let observer = migrate::connect_remote_app(&settings, 1)
        .await
        .unwrap_or_else(|_| panic!("local remote observer admission failed"));
    let directory =
        std::env::temp_dir().join(format!("fvoci-auth-local-remote-{}", Uuid::now_v7()));
    std::fs::create_dir(&directory).unwrap();
    run(backend, Readback::Remote(observer), directory.clone()).await;
    std::fs::remove_dir_all(directory).unwrap();
}
