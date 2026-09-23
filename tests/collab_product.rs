#![cfg(feature = "db-tests")]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use chrono::{Duration as ChronoDuration, Utc};
use futures_util::{SinkExt, StreamExt};
use fvoci_server::auth::password::Keyring;
use fvoci_server::auth::token::new_token;
use fvoci_server::auth::AuthService;
use fvoci_server::collab::config::CollabConfig;
use fvoci_server::collab::guard::RoomGuard;
use fvoci_server::collab::hub::RoomLifecyclePhase;
use fvoci_server::collab::room::{
    arm_spawn_room_block, disarm_spawn_room_block, AuthenticatedConnection, CollabSession,
    JoinError, RoomJoin,
};
use fvoci_server::collab::wire::{
    encode, AuthMessage, CollabKind, CollabRoomName, DocumentMessage, SyncMessage, SyncStep,
    WireFrame,
};
use fvoci_server::collab::y_sync::encode_sync_payload;
use fvoci_server::collab::CollabHub;
use fvoci_server::db::collab::load_collab_document;
use fvoci_server::db::documents::CreateDocumentInput;
use fvoci_server::db::workspace;
use fvoci_server::db::{documents, migrate, pool, Db};
use fvoci_server::http::rate_limit::RateLimiter;
use fvoci_server::http::state::AppState;
use rand::RngCore;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

const PEPPER: &str =
    r#"{"test":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#;
const PUBLIC_ORIGIN: &str = "http://localhost";
const LIFECYCLE_TEST_TIMEOUT: Duration = Duration::from_secs(30);

async fn run_lifecycle_test<Fut>(name: &str, case: Fut)
where
    Fut: std::future::Future<Output = ()>,
{
    tokio::time::timeout(LIFECYCLE_TEST_TIMEOUT, case)
        .await
        .unwrap_or_else(|_| panic!("{name} hung (>{LIFECYCLE_TEST_TIMEOUT:?}) including cleanup"));
}

struct TestDb {
    admin_url: String,
    app_url: String,
    db_name: String,
    role_name: String,
}

struct SessionFixture {
    pool: PgPool,
    user_id: Uuid,
    session_id: Uuid,
    workspace_id: Uuid,
    session_token: String,
}

struct WikiDocFixture {
    session: SessionFixture,
    document_id: Uuid,
}

impl WikiDocFixture {
    fn clone_fixture(&self) -> Self {
        Self {
            session: SessionFixture {
                pool: self.session.pool.clone(),
                user_id: self.session.user_id,
                session_id: self.session.session_id,
                workspace_id: self.session.workspace_id,
                session_token: self.session.session_token.clone(),
            },
            document_id: self.document_id,
        }
    }
}

fn engine_bin() -> PathBuf {
    fvoci_server::collab::config::require_collab_engine_for_tests()
}

fn server_db_url(url: &str) -> String {
    let parsed = url::Url::parse(url).expect("database url");
    let mut server = parsed;
    server.set_path("");
    server.to_string().trim_end_matches('/').to_string()
}

fn join_db_url(server_url: &str, db_name: &str) -> String {
    let mut parsed = url::Url::parse(server_url).expect("server url");
    parsed.set_path(&format!("/{}", db_name));
    parsed.to_string()
}

async fn apply_grants(pool: &PgPool, role_name: &str) {
    let quoted_role = format!("\"{}\"", role_name);
    let grants =
        include_str!("../scripts/grant-app-role.sql").replace(":\"app_role\"", &quoted_role);
    for statement in grants.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        sqlx::query(statement).execute(pool).await.expect("grant");
    }
}

impl TestDb {
    async fn bootstrap() -> Self {
        let admin_base = std::env::var("TEST_DATABASE_URL")
            .or_else(|_| std::env::var("FVOCI_TEST_DATABASE_URL"))
            .expect("TEST_DATABASE_URL missing; native helper tests require real PostgreSQL");

        let db_name = format!("fvoci_collab_prod_{}", Uuid::now_v7().simple());
        let role_name = format!("fvoci_app_{}", db_name.replace('-', "_"));
        let mut password_bytes = [0u8; 24];
        rand::rng().fill_bytes(&mut password_bytes);
        let role_password = hex::encode(password_bytes);
        let server_url = server_db_url(&admin_base);

        let admin_pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(&server_url)
            .await
            .expect("connect admin");
        sqlx::query(&format!("CREATE DATABASE \"{}\"", db_name))
            .execute(&admin_pool)
            .await
            .expect("create database");
        admin_pool.close().await;

        let admin_url = join_db_url(&server_url, &db_name);
        migrate::run_migrations(&admin_url).await.expect("migrate");

        let migration_pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(&admin_url)
            .await
            .expect("connect migration db");
        sqlx::query(&format!(
            "CREATE ROLE \"{}\" LOGIN PASSWORD '{}' NOSUPERUSER NOBYPASSRLS",
            role_name, role_password
        ))
        .execute(&migration_pool)
        .await
        .expect("create role");
        apply_grants(&migration_pool, &role_name).await;
        migration_pool.close().await;

        let mut app = url::Url::parse(&admin_url).expect("database url");
        app.set_username(&role_name).ok();
        app.set_password(Some(&role_password)).ok();

        Self {
            admin_url,
            app_url: app.to_string(),
            db_name,
            role_name,
        }
    }

    async fn cleanup(self) {
        let server_url = server_db_url(&self.admin_url);
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(&server_url)
            .await
            .ok();
        if let Some(pool) = pool {
            let _ = sqlx::query(&format!(
                "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '{}'",
                self.db_name
            ))
            .execute(&pool)
            .await;
            let _ = sqlx::query(&format!("DROP DATABASE IF EXISTS \"{}\"", self.db_name))
                .execute(&pool)
                .await;
            let _ = sqlx::query(&format!("DROP ROLE IF EXISTS \"{}\"", self.role_name))
                .execute(&pool)
                .await;
            pool.close().await;
        }
    }
}

async fn setup_owner_session(harness: &TestDb) -> SessionFixture {
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let user_id = Uuid::now_v7();
    let workspace_id = Uuid::now_v7();
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
    .bind(format!("owner-{user_id}@example.com"))
    .bind(&hash)
    .bind("Owner")
    .execute(&admin)
    .await
    .unwrap();
    sqlx::query("INSERT INTO fvoci.workspaces (id, slug, name) VALUES ($1, $2, $3)")
        .bind(workspace_id)
        .bind(workspace::personal_workspace_slug(user_id))
        .bind("Collab Product WS")
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO fvoci.memberships (workspace_id, user_id, role) VALUES ($1, $2, 'owner')",
    )
    .bind(workspace_id)
    .bind(user_id)
    .execute(&admin)
    .await
    .unwrap();
    admin.close().await;

    let pool = pool::connect_app(&harness.app_url).await.unwrap();
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

fn test_collab_config(max_rooms: usize, idle_evict_ms: u64) -> CollabConfig {
    CollabConfig {
        engine_bin: engine_bin(),
        limits: collab_engine::Limits::for_tests(),
        max_rooms,
        max_connections_per_room: 16,
        max_queued_room_ops: 128,
        max_pending_bytes_per_connection: 4 * 1024 * 1024,
        idle_evict_ms,
        revoke_poll_ms: 5_000,
        client_id_ttl_ms: 60_000,
    }
}

async fn setup_wiki_doc(harness: &TestDb) -> WikiDocFixture {
    let session = setup_owner_session(harness).await;
    let created = documents::create_wiki_document(
        &session.pool,
        session.workspace_id,
        session.user_id,
        session.session_id,
        CreateDocumentInput {
            parent_id: None,
            title: "Collab product doc",
            icon: None,
        },
        None,
    )
    .await
    .expect("create doc")
    .expect("created");
    WikiDocFixture {
        session,
        document_id: created.id,
    }
}

async fn setup_wiki_doc_batch(harness: &TestDb, count: usize) -> Vec<WikiDocFixture> {
    let session = setup_owner_session(harness).await;
    let mut docs = Vec::with_capacity(count);
    let titles = (0..count)
        .map(|index| format!("Collab lifecycle doc {index}"))
        .collect::<Vec<_>>();
    for title in titles {
        let created = documents::create_wiki_document(
            &session.pool,
            session.workspace_id,
            session.user_id,
            session.session_id,
            CreateDocumentInput {
                parent_id: None,
                title: &title,
                icon: None,
            },
            None,
        )
        .await
        .expect("create doc")
        .expect("created");
        docs.push(WikiDocFixture {
            session: SessionFixture {
                pool: session.pool.clone(),
                user_id: session.user_id,
                session_id: session.session_id,
                workspace_id: session.workspace_id,
                session_token: session.session_token.clone(),
            },
            document_id: created.id,
        });
    }
    docs
}

async fn hub_join_document(
    hub: &CollabHub,
    wiki: &WikiDocFixture,
    document_id: Uuid,
    client_id: u32,
) -> Result<Uuid, JoinError> {
    let conn_id = Uuid::now_v7();
    let (events_tx, mut events_rx) = mpsc::channel(8);
    tokio::spawn(async move { while events_rx.recv().await.is_some() {} });
    let routing_key = room_key(wiki.session.workspace_id, document_id);
    let join = RoomJoin {
        conn: AuthenticatedConnection {
            conn_id,
            session: CollabSession {
                session_id: wiki.session.session_id,
                user_id: wiki.session.user_id,
                given_name: "Owner".into(),
                family_name: None,
            },
            client_id,
            read_only: false,
            routing_key,
        },
        events: events_tx,
    };
    hub.join_room((wiki.session.workspace_id, document_id), join)
        .await?;
    Ok(conn_id)
}

async fn hub_join(
    hub: &CollabHub,
    wiki: &WikiDocFixture,
    client_id: u32,
) -> Result<Uuid, JoinError> {
    hub_join_document(hub, wiki, wiki.document_id, client_id).await
}

async fn wait_for_booting(hub: &CollabHub, key: (Uuid, Uuid)) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if hub.room_lifecycle_phase(key).await == RoomLifecyclePhase::Booting {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("room never entered Booting");
}

async fn collab_app_state(app_url: &str, with_collab: bool) -> AppState {
    let pool = pool::connect_app(app_url).await.expect("app pool");
    let collab = if with_collab {
        let cfg = CollabConfig {
            engine_bin: engine_bin(),
            limits: collab_engine::Limits::for_tests(),
            max_rooms: 4,
            max_connections_per_room: 16,
            max_queued_room_ops: 128,
            max_pending_bytes_per_connection: 4 * 1024 * 1024,
            idle_evict_ms: 30_000,
            revoke_poll_ms: 5_000,
            client_id_ttl_ms: 60_000,
        };
        Some(Arc::new(CollabHub::new(cfg, pool.clone())))
    } else {
        None
    };
    AppState {
        auth: Arc::new(AuthService {
            db: Db::new(pool),
            password_keys: Keyring::parse(PEPPER, "test").expect("pepper"),
        }),
        branding_name: "FVOCI".to_string(),
        public_origin: PUBLIC_ORIGIN.to_string(),
        cookie_secure: false,
        rate_limiter: RateLimiter::new(),
        collab,
    }
}

async fn spawn_server(app: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    addr
}

fn room_key(workspace_id: Uuid, document_id: Uuid) -> String {
    CollabRoomName {
        workspace_id,
        kind: CollabKind::Document,
        resource_id: document_id,
    }
    .routing_key()
}

fn sample_hi_update() -> Vec<u8> {
    hex::decode("0101e8eda5a2070004010b70726f73656d6972726f7202686900").expect("fixture")
}

fn sync_update_frame(routing_key: &str, update: &[u8]) -> Vec<u8> {
    encode(&WireFrame::Document {
        routing_key: routing_key.to_string(),
        room: CollabRoomName::parse(routing_key),
        message: DocumentMessage::Sync(fvoci_server::collab::wire::SyncMessage {
            step: SyncStep::Update,
            y_protocol: encode_sync_payload(SyncStep::Update, update),
        }),
    })
    .expect("encode update")
}

async fn connect_member(
    addr: SocketAddr,
    session_token: &str,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let mut request = format!("ws://{addr}/collab").into_client_request().unwrap();
    request
        .headers_mut()
        .insert("origin", PUBLIC_ORIGIN.parse().unwrap());
    request.headers_mut().insert(
        "cookie",
        format!("fvoci_session={session_token}").parse().unwrap(),
    );
    tokio_tungstenite::connect_async(request)
        .await
        .expect("connect")
        .0
}

async fn auth_and_join(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    routing_key: &str,
    client_id: u32,
) {
    ws.send(Message::Binary(
        auth_token_frame(routing_key, client_id).into(),
    ))
    .await
    .unwrap();
    let msg = tokio::time::timeout(Duration::from_secs(10), ws.next())
        .await
        .expect("timeout")
        .expect("stream")
        .expect("frame");
    let frame = fvoci_server::collab::wire::decode(&msg.into_data()).expect("decode");
    assert!(matches!(
        frame,
        WireFrame::Document {
            message: DocumentMessage::Auth(AuthMessage::Authenticated { .. }),
            ..
        }
    ));
}

fn auth_token_frame(routing_key: &str, client_id: u32) -> Vec<u8> {
    encode(&WireFrame::Document {
        routing_key: routing_key.to_string(),
        room: CollabRoomName::parse(routing_key),
        message: DocumentMessage::Auth(AuthMessage::Token {
            token: client_id.to_string(),
            provider_version: Some("4.6.0".into()),
        }),
    })
    .expect("encode auth")
}

#[tokio::test]
async fn collab_requires_native_helper_env() {
    let path = std::env::var("FVOCI_COLLAB_ENGINE")
        .expect("FVOCI_COLLAB_ENGINE must be set for collab product tests");
    let path = PathBuf::from(path.trim());
    assert!(
        path.is_file(),
        "FVOCI_COLLAB_ENGINE must point at built collab-engine binary"
    );
}

#[tokio::test]
async fn collab_unavailable_without_helper_config() {
    let harness = TestDb::bootstrap().await;
    let app = fvoci_server::http::router(collab_app_state(&harness.app_url, false).await, None);
    let addr = spawn_server(app).await;
    let response = reqwest::Client::new()
        .get(format!("http://{addr}/collab"))
        .header("origin", PUBLIC_ORIGIN)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 503);
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_rejects_missing_origin_on_upgrade() {
    let harness = TestDb::bootstrap().await;
    let app = fvoci_server::http::router(collab_app_state(&harness.app_url, true).await, None);
    let addr = spawn_server(app).await;
    let request = format!("ws://{addr}/collab").into_client_request().unwrap();
    let err = tokio_tungstenite::connect_async(request).await.unwrap_err();
    assert!(
        err.to_string().contains("403") || err.to_string().contains("Forbidden"),
        "expected origin rejection, got {err}"
    );
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_auth_handshake_succeeds_for_member() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    let app = fvoci_server::http::router(collab_app_state(&harness.app_url, true).await, None);
    let addr = spawn_server(app).await;
    let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
    let client_id = 42_424_242u32;

    let mut request = format!("ws://{addr}/collab").into_client_request().unwrap();
    request
        .headers_mut()
        .insert("origin", PUBLIC_ORIGIN.parse().unwrap());
    request.headers_mut().insert(
        "cookie",
        format!("fvoci_session={}", wiki.session.session_token)
            .parse()
            .unwrap(),
    );

    let (mut ws, _) = tokio_tungstenite::connect_async(request)
        .await
        .expect("websocket connect");
    ws.send(Message::Binary(
        auth_token_frame(&routing_key, client_id).into(),
    ))
    .await
    .unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(10), ws.next())
        .await
        .expect("timeout")
        .expect("stream")
        .expect("frame");
    let bytes = msg.into_data();
    let frame = fvoci_server::collab::wire::decode(&bytes).expect("decode");
    match frame {
        WireFrame::Document {
            message: DocumentMessage::Auth(AuthMessage::Authenticated { scope }),
            ..
        } => assert!(scope.contains("read")),
        other => panic!("expected authenticated, got {other:?}"),
    }
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_nonmember_is_denied() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    let outsider = setup_owner_session(&harness).await;
    let app = fvoci_server::http::router(collab_app_state(&harness.app_url, true).await, None);
    let addr = spawn_server(app).await;
    let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
    let mut ws = connect_member(addr, &outsider.session_token).await;
    ws.send(Message::Binary(auth_token_frame(&routing_key, 1).into()))
        .await
        .unwrap();
    let msg = tokio::time::timeout(Duration::from_secs(10), ws.next())
        .await
        .expect("timeout")
        .expect("stream")
        .expect("frame");
    let frame = fvoci_server::collab::wire::decode(&msg.into_data()).expect("decode");
    assert!(matches!(
        frame,
        WireFrame::Document {
            message: DocumentMessage::Auth(AuthMessage::PermissionDenied { .. }),
            ..
        }
    ));
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_two_clients_update_persists_and_broadcasts() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    let app = fvoci_server::http::router(collab_app_state(&harness.app_url, true).await, None);
    let addr = spawn_server(app).await;
    let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
    let update = sample_hi_update();

    let mut writer = connect_member(addr, &wiki.session.session_token).await;
    auth_and_join(&mut writer, &routing_key, 11).await;
    let mut reader = connect_member(addr, &wiki.session.session_token).await;
    auth_and_join(&mut reader, &routing_key, 22).await;

    writer
        .send(Message::Binary(
            sync_update_frame(&routing_key, &update).into(),
        ))
        .await
        .unwrap();

    let mut saw_applied = false;
    for _ in 0..8 {
        let msg = tokio::time::timeout(Duration::from_secs(10), writer.next())
            .await
            .expect("timeout");
        if msg.is_none() {
            break;
        }
        let frame =
            fvoci_server::collab::wire::decode(&msg.unwrap().unwrap().into_data()).expect("decode");
        if matches!(
            frame,
            WireFrame::Document {
                message: DocumentMessage::SyncStatus { applied: true },
                ..
            }
        ) {
            saw_applied = true;
            break;
        }
    }
    assert!(saw_applied, "writer should receive applied sync status");

    let mut saw_broadcast = false;
    for _ in 0..8 {
        let msg = tokio::time::timeout(Duration::from_secs(10), reader.next())
            .await
            .expect("timeout");
        if msg.is_none() {
            break;
        }
        let frame =
            fvoci_server::collab::wire::decode(&msg.unwrap().unwrap().into_data()).expect("decode");
        if matches!(
            frame,
            WireFrame::Document {
                message: DocumentMessage::Sync(SyncMessage {
                    step: SyncStep::Update,
                    ..
                }),
                ..
            }
        ) {
            saw_broadcast = true;
            break;
        }
    }
    assert!(saw_broadcast, "reader should receive broadcast update");

    let load = load_collab_document(
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
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_concurrent_first_joins_both_succeed() {
    run_lifecycle_test("collab_concurrent_first_joins_both_succeed", async {
        let harness = TestDb::bootstrap().await;
        let wiki = setup_wiki_doc(&harness).await;
        let hub = Arc::new(CollabHub::new(
            test_collab_config(4, 30_000),
            wiki.session.pool.clone(),
        ));
        let key = (wiki.session.workspace_id, wiki.document_id);
        let slots_before = hub.available_room_slots();
        let barrier = Arc::new(tokio::sync::Barrier::new(2));

        let hub_a = hub.clone();
        let hub_b = hub.clone();
        let wiki_a = wiki.clone_fixture();
        let wiki_b = wiki.clone_fixture();
        let barrier_a = barrier.clone();
        let barrier_b = barrier.clone();
        let (first, second) = tokio::time::timeout(Duration::from_secs(15), async {
            tokio::join!(
                async move {
                    barrier_a.wait().await;
                    hub_join(&hub_a, &wiki_a, 1).await
                },
                async move {
                    barrier_b.wait().await;
                    hub_join(&hub_b, &wiki_b, 2).await
                }
            )
        })
        .await
        .expect("concurrent first join hung waiting on room lifecycle notify");

        assert!(first.is_ok(), "first join failed: {:?}", first.err());
        assert!(second.is_ok(), "second join failed: {:?}", second.err());
        assert_eq!(hub.available_room_slots(), slots_before - 1);
        assert!(hub.room_occupies_slot(key).await);
        hub.shutdown().await;
        assert_eq!(hub.available_room_slots(), slots_before);
        harness.cleanup().await;
    })
    .await;
}

#[tokio::test]
async fn collab_lifecycle_failed_start_reuses_slot() {
    run_lifecycle_test("collab_lifecycle_failed_start_reuses_slot", async {
        let harness = TestDb::bootstrap().await;
        let wiki = setup_wiki_doc(&harness).await;
        let held = RoomGuard::try_acquire(&wiki.session.pool, wiki.document_id)
            .await
            .expect("db")
            .expect("room lock should be free");
        let hub = CollabHub::new(test_collab_config(4, 30_000), wiki.session.pool.clone());
        assert_eq!(hub.available_room_slots(), 4);
        let failed = hub_join(&hub, &wiki, 1).await;
        assert_eq!(failed, Err(JoinError::WriterStale));
        assert_eq!(hub.available_room_slots(), 4);
        held.release().await;
        assert!(hub_join(&hub, &wiki, 2).await.is_ok());
        assert_eq!(hub.available_room_slots(), 3);
        hub.shutdown().await;
        assert_eq!(hub.available_room_slots(), 4);
        harness.cleanup().await;
    })
    .await;
}

#[tokio::test]
async fn collab_lifecycle_cancelled_start_releases_slot() {
    run_lifecycle_test("collab_lifecycle_cancelled_start_releases_slot", async {
        let harness = TestDb::bootstrap().await;
        let wiki = setup_wiki_doc(&harness).await;
        let hub = Arc::new(CollabHub::new(
            test_collab_config(4, 30_000),
            wiki.session.pool.clone(),
        ));
        let join_task = tokio::spawn({
            let hub = hub.clone();
            let wiki = wiki.clone_fixture();
            async move { hub_join(&hub, &wiki, 1).await }
        });
        hub.shutdown().await;
        let result = join_task.await.expect("join task");
        assert!(
            result.is_err(),
            "join during shutdown should fail, got conn_id {:?}",
            result.ok()
        );
        assert_eq!(hub.available_room_slots(), 4);
        harness.cleanup().await;
    })
    .await;
}

#[tokio::test]
async fn collab_lifecycle_denied_joins_do_not_reserve_slots() {
    run_lifecycle_test(
        "collab_lifecycle_denied_joins_do_not_reserve_slots",
        async {
            let harness = TestDb::bootstrap().await;
            let wiki = setup_wiki_doc(&harness).await;
            let outsider = setup_owner_session(&harness).await;
            let hub = CollabHub::new(test_collab_config(4, 30_000), wiki.session.pool.clone());
            assert_eq!(hub.available_room_slots(), 4);

            for index in 0..4 {
                let fake_doc = Uuid::now_v7();
                let denied = hub_join_document(&hub, &wiki, fake_doc, index).await;
                assert_eq!(denied, Err(JoinError::AdmissionDenied));
            }
            let outsider_denied = hub_join_document(
                &hub,
                &WikiDocFixture {
                    session: outsider,
                    document_id: wiki.document_id,
                },
                wiki.document_id,
                9,
            )
            .await;
            assert_eq!(outsider_denied, Err(JoinError::AdmissionDenied));
            assert_eq!(hub.available_room_slots(), 4);
            assert!(hub_join(&hub, &wiki, 1).await.is_ok());
            assert_eq!(hub.available_room_slots(), 3);
            hub.shutdown().await;
            harness.cleanup().await;
        },
    )
    .await;
}

#[tokio::test]
async fn collab_lifecycle_starting_gate_blocks_second_creator() {
    run_lifecycle_test(
        "collab_lifecycle_starting_gate_blocks_second_creator",
        async {
            let harness = TestDb::bootstrap().await;
            let wiki = setup_wiki_doc(&harness).await;
            let release = arm_spawn_room_block().await;
            let hub = Arc::new(CollabHub::new(
                test_collab_config(4, 30_000),
                wiki.session.pool.clone(),
            ));
            let key = (wiki.session.workspace_id, wiki.document_id);
            let slots_before = hub.available_room_slots();

            let join_a = tokio::spawn({
                let hub = hub.clone();
                let wiki = wiki.clone_fixture();
                async move { hub_join(&hub, &wiki, 1).await }
            });
            wait_for_booting(&hub, key).await;
            assert_eq!(hub.available_room_slots(), slots_before - 1);

            let join_b = tokio::spawn({
                let hub = hub.clone();
                let wiki = wiki.clone_fixture();
                async move { hub_join(&hub, &wiki, 2).await }
            });
            tokio::time::sleep(Duration::from_millis(100)).await;
            assert_eq!(
                hub.room_lifecycle_phase(key).await,
                RoomLifecyclePhase::Booting
            );
            assert_eq!(hub.available_room_slots(), slots_before - 1);

            let _ = release.send(());
            assert!(join_a.await.expect("join a task").is_ok());
            assert!(join_b.await.expect("join b task").is_ok());
            assert_eq!(hub.available_room_slots(), slots_before - 1);
            disarm_spawn_room_block().await;
            hub.shutdown().await;
            harness.cleanup().await;
        },
    )
    .await;
}

#[tokio::test]
async fn collab_lifecycle_aborted_booting_creator_releases_slot() {
    run_lifecycle_test(
        "collab_lifecycle_aborted_booting_creator_releases_slot",
        async {
            let harness = TestDb::bootstrap().await;
            let wiki = setup_wiki_doc(&harness).await;
            let release = arm_spawn_room_block().await;
            let hub = Arc::new(CollabHub::new(
                test_collab_config(4, 30_000),
                wiki.session.pool.clone(),
            ));
            let key = (wiki.session.workspace_id, wiki.document_id);
            let join_task = tokio::spawn({
                let hub = hub.clone();
                let wiki = wiki.clone_fixture();
                async move { hub_join(&hub, &wiki, 1).await }
            });
            wait_for_booting(&hub, key).await;
            assert_eq!(hub.available_room_slots(), 3);
            hub.shutdown().await;
            assert_eq!(hub.available_room_slots(), 4);
            let _ = release.send(());
            let result = tokio::time::timeout(Duration::from_secs(5), join_task)
                .await
                .expect("aborted booting join hung")
                .expect("join task");
            assert!(result.is_err(), "aborted booting join should fail");
            disarm_spawn_room_block().await;
            harness.cleanup().await;
        },
    )
    .await;
}

#[tokio::test]
async fn collab_lifecycle_max_rooms_then_reuse_after_leave() {
    run_lifecycle_test("collab_lifecycle_max_rooms_then_reuse_after_leave", async {
        let harness = TestDb::bootstrap().await;
        let docs = setup_wiki_doc_batch(&harness, 5).await;
        let hub = CollabHub::new(test_collab_config(4, 30_000), docs[0].session.pool.clone());

        let mut conn_ids = Vec::new();
        for doc in docs.iter().take(4) {
            conn_ids.push(hub_join(&hub, doc, 1).await.expect("join room"));
        }
        assert_eq!(hub.available_room_slots(), 0);
        let fifth = hub_join(&hub, &docs[4], 1).await;
        assert_eq!(fifth, Err(JoinError::RoomFull));

        let key = (docs[0].session.workspace_id, docs[0].document_id);
        hub.leave_room(key, conn_ids[0]).await;
        hub.shutdown().await;
        assert_eq!(hub.available_room_slots(), 4);

        let hub = CollabHub::new(test_collab_config(4, 30_000), docs[0].session.pool.clone());
        assert!(hub_join(&hub, &docs[4], 2).await.is_ok());
        hub.shutdown().await;
        harness.cleanup().await;
    })
    .await;
}

#[tokio::test]
async fn collab_lifecycle_idle_eviction_allows_rejoin() {
    run_lifecycle_test("collab_lifecycle_idle_eviction_allows_rejoin", async {
        let harness = TestDb::bootstrap().await;
        let wiki = setup_wiki_doc(&harness).await;
        let hub = CollabHub::new(test_collab_config(4, 200), wiki.session.pool.clone());
        let key = (wiki.session.workspace_id, wiki.document_id);
        let conn_id = hub_join(&hub, &wiki, 1).await.expect("join");
        hub.leave_room(key, conn_id).await;
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert!(!hub.room_occupies_slot(key).await);
        assert_eq!(hub.available_room_slots(), 4);
        assert!(hub_join(&hub, &wiki, 2).await.is_ok());
        assert_eq!(hub.available_room_slots(), 3);
        hub.shutdown().await;
        harness.cleanup().await;
    })
    .await;
}

#[tokio::test]
async fn collab_archived_document_rejects_mutation() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    sqlx::query("UPDATE fvoci.documents SET status = 'archived' WHERE id = $1")
        .bind(wiki.document_id)
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;

    let app = fvoci_server::http::router(collab_app_state(&harness.app_url, true).await, None);
    let addr = spawn_server(app).await;
    let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
    let mut ws = connect_member(addr, &wiki.session.session_token).await;
    auth_and_join(&mut ws, &routing_key, 5).await;

    ws.send(Message::Binary(
        sync_update_frame(&routing_key, &sample_hi_update()).into(),
    ))
    .await
    .unwrap();
    let msg = tokio::time::timeout(Duration::from_secs(10), ws.next())
        .await
        .expect("timeout")
        .expect("stream")
        .expect("frame");
    let frame = fvoci_server::collab::wire::decode(&msg.into_data()).expect("decode");
    assert!(matches!(
        frame,
        WireFrame::Document {
            message: DocumentMessage::SyncStatus { applied: false },
            ..
        }
    ));
    harness.cleanup().await;
}
