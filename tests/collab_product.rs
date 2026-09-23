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
    arm_force_primary_apply_fail, arm_spawn_room_block, disarm_force_primary_apply_fail,
    disarm_spawn_room_block, AuthenticatedConnection, CollabSession, JoinError, RoomJoin,
};
use fvoci_server::collab::wire::{
    encode, AuthMessage, CollabKind, CollabRoomName, DocumentMessage, SyncMessage, SyncStep,
    WireFrame,
};
use fvoci_server::collab::y_sync::{encode_sync_payload, parse_sync_payload};
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
const OP_CAP_TEST_TIMEOUT: Duration = Duration::from_secs(45);

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

async fn hub_join_readonly(
    hub: &CollabHub,
    wiki: &WikiDocFixture,
    client_id: u32,
) -> Result<Uuid, JoinError> {
    let conn_id = Uuid::now_v7();
    let (events_tx, mut events_rx) = mpsc::channel(8);
    tokio::spawn(async move { while events_rx.recv().await.is_some() {} });
    let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
    let join = RoomJoin {
        conn: AuthenticatedConnection {
            conn_id,
            session: CollabSession {
                session_id: wiki.session.session_id,
                user_id: wiki.session.user_id,
                given_name: "Reader".into(),
                family_name: None,
            },
            client_id,
            read_only: true,
            routing_key,
        },
        events: events_tx,
    };
    hub.join_room((wiki.session.workspace_id, wiki.document_id), join)
        .await?;
    Ok(conn_id)
}

async fn wait_for_booting(hub: &CollabHub, key: (Uuid, Uuid)) {
    wait_for_phase(hub, key, RoomLifecyclePhase::Booting).await;
}

async fn wait_for_phase(hub: &CollabHub, key: (Uuid, Uuid), expected: RoomLifecyclePhase) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if hub.room_lifecycle_phase(key).await == expected {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("room did not reach expected lifecycle phase");
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

fn engine_fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("crates/collab-engine/fixtures")
            .join(name),
    )
    .unwrap_or_else(|e| panic!("read fixture {name}: {e}"))
}

fn sync_step1_frame(routing_key: &str, state_vector: &[u8]) -> Vec<u8> {
    encode(&WireFrame::Document {
        routing_key: routing_key.to_string(),
        room: CollabRoomName::parse(routing_key),
        message: DocumentMessage::Sync(SyncMessage {
            step: SyncStep::Step1,
            y_protocol: encode_sync_payload(SyncStep::Step1, state_vector),
        }),
    })
    .expect("encode step1")
}

fn stateless_frame(routing_key: &str, payload: &str) -> Vec<u8> {
    encode(&WireFrame::Document {
        routing_key: routing_key.to_string(),
        room: CollabRoomName::parse(routing_key),
        message: DocumentMessage::Stateless(payload.to_string()),
    })
    .expect("encode stateless")
}

async fn recv_document_frame(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    attempts: usize,
) -> Option<WireFrame> {
    for _ in 0..attempts {
        let msg = tokio::time::timeout(Duration::from_secs(10), ws.next())
            .await
            .ok()
            .flatten()?;
        if let Ok(Message::Binary(bytes)) = msg {
            return fvoci_server::collab::wire::decode(&bytes).ok();
        }
    }
    None
}

async fn recv_document_frame_within(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    within: Duration,
) -> Option<WireFrame> {
    let msg = tokio::time::timeout(within, ws.next())
        .await
        .ok()
        .flatten()?;
    if let Ok(Message::Binary(bytes)) = msg {
        return fvoci_server::collab::wire::decode(&bytes).ok();
    }
    None
}

async fn wait_for_sync_applied(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    within: Duration,
) -> bool {
    let deadline = tokio::time::Instant::now() + within;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match recv_document_frame_within(ws, remaining.min(Duration::from_millis(200))).await {
            Some(WireFrame::Document {
                message: DocumentMessage::SyncStatus { applied: true },
                ..
            }) => return true,
            Some(WireFrame::Document {
                message: DocumentMessage::SyncStatus { applied: false },
                ..
            }) => return false,
            Some(WireFrame::Document {
                message: DocumentMessage::Close { .. },
                ..
            }) => return false,
            _ => {}
        }
    }
    false
}

async fn wait_for_stateless_prefix(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    prefix: &str,
    within: Duration,
) -> bool {
    let deadline = tokio::time::Instant::now() + within;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if let Some(WireFrame::Document {
            message: DocumentMessage::Stateless(body),
            ..
        }) = recv_document_frame_within(ws, remaining.min(Duration::from_millis(200))).await
        {
            if body.starts_with(prefix) {
                return true;
            }
        }
    }
    false
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
            let release = arm_spawn_room_block(wiki.document_id).await;
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
            tokio::time::timeout(Duration::from_secs(5), async {
                while hub.room_waiter_count(key).await == 0 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("second creator never reached the shared slot");
            assert_eq!(
                hub.room_lifecycle_phase(key).await,
                RoomLifecyclePhase::Booting
            );
            assert_eq!(hub.available_room_slots(), slots_before - 1);

            let _ = release.send(());
            assert!(join_a.await.expect("join a task").is_ok());
            assert!(join_b.await.expect("join b task").is_ok());
            assert_eq!(hub.available_room_slots(), slots_before - 1);
            disarm_spawn_room_block(wiki.document_id).await;
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
            let release = arm_spawn_room_block(wiki.document_id).await;
            let hub = Arc::new(CollabHub::new(
                test_collab_config(4, 200),
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
            join_task.abort();
            assert!(join_task.await.unwrap_err().is_cancelled());
            release
                .send(())
                .expect("hub still owns startup after caller cancellation");
            // The abandoned caller cannot strand Booting. The same live hub evicts
            // its zero-client room and can subsequently acquire the database guard.
            wait_for_phase(&hub, key, RoomLifecyclePhase::Absent).await;
            assert_eq!(hub.available_room_slots(), 4);
            assert!(hub_join(&hub, &wiki, 2).await.is_ok());
            disarm_spawn_room_block(wiki.document_id).await;
            hub.shutdown().await;
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
        let hub = CollabHub::new(test_collab_config(4, 200), docs[0].session.pool.clone());

        let mut conn_ids = Vec::new();
        for doc in docs.iter().take(4) {
            conn_ids.push(hub_join(&hub, doc, 1).await.expect("join room"));
        }
        assert_eq!(hub.available_room_slots(), 0);
        let fifth = hub_join(&hub, &docs[4], 1).await;
        assert_eq!(fifth, Err(JoinError::RoomFull));

        let key = (docs[0].session.workspace_id, docs[0].document_id);
        hub.leave_room(key, conn_ids[0]).await;
        wait_for_phase(&hub, key, RoomLifecyclePhase::Absent).await;
        assert_eq!(hub.available_room_slots(), 1);
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
        wait_for_phase(&hub, key, RoomLifecyclePhase::Absent).await;
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

#[tokio::test]
async fn collab_reconnect_step1_includes_server_state_vector() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    let app = fvoci_server::http::router(collab_app_state(&harness.app_url, true).await, None);
    let addr = spawn_server(app).await;
    let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
    let update = sample_hi_update();

    let mut writer = connect_member(addr, &wiki.session.session_token).await;
    auth_and_join(&mut writer, &routing_key, 11).await;
    writer
        .send(Message::Binary(sync_update_frame(&routing_key, &update).into()))
        .await
        .unwrap();
    for _ in 0..8 {
        if let Some(WireFrame::Document {
            message: DocumentMessage::SyncStatus { applied: true },
            ..
        }) = recv_document_frame(&mut writer, 1).await
        {
            break;
        }
    }

    writer
        .send(Message::Binary(sync_step1_frame(&routing_key, &[0, 0]).into()))
        .await
        .unwrap();

    let mut saw_step2 = false;
    let mut saw_server_step1 = false;
    for _ in 0..12 {
        let frame = recv_document_frame(&mut writer, 1).await;
        match frame {
            Some(WireFrame::Document {
                message: DocumentMessage::Sync(SyncMessage { step, y_protocol }),
                ..
            }) => match step {
                SyncStep::Step2 if !saw_step2 => saw_step2 = true,
                SyncStep::Step1 if saw_step2 => {
                    let (_, sv) = parse_sync_payload(&y_protocol, 4 * 1024 * 1024).unwrap();
                    assert!(!sv.is_empty(), "server Step1 must carry a state vector");
                    saw_server_step1 = true;
                    break;
                }
                _ => {}
            },
            _ => {}
        }
    }
    assert!(saw_step2, "client should receive Step2");
    assert!(saw_server_step1, "client should receive server Step1 after Step2");
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_empty_byte_update_is_rejected() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    let app = fvoci_server::http::router(collab_app_state(&harness.app_url, true).await, None);
    let addr = spawn_server(app).await;
    let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);

    let mut writer = connect_member(addr, &wiki.session.session_token).await;
    auth_and_join(&mut writer, &routing_key, 32).await;
    writer
        .send(Message::Binary(sync_update_frame(&routing_key, &[]).into()))
        .await
        .unwrap();

    let mut saw_rejected = false;
    for _ in 0..6 {
        if let Some(WireFrame::Document {
            message: DocumentMessage::SyncStatus { applied: false },
            ..
        }) = recv_document_frame(&mut writer, 1).await
        {
            saw_rejected = true;
            break;
        }
    }
    assert!(saw_rejected, "byte-empty update must be rejected as malformed");

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
    assert!(load.tail.is_empty());
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_canonical_noop_update_is_not_stored() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    let app = fvoci_server::http::router(collab_app_state(&harness.app_url, true).await, None);
    let addr = spawn_server(app).await;
    let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);

    let mut writer = connect_member(addr, &wiki.session.session_token).await;
    auth_and_join(&mut writer, &routing_key, 31).await;
    writer
        .send(Message::Binary(sync_update_frame(&routing_key, &[0, 0]).into()))
        .await
        .unwrap();

    let mut saw_applied = false;
    for _ in 0..6 {
        if let Some(WireFrame::Document {
            message: DocumentMessage::SyncStatus { applied: true },
            ..
        }) = recv_document_frame(&mut writer, 1).await
        {
            saw_applied = true;
            break;
        }
    }
    assert!(saw_applied, "noop update should ack without rejection");

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
    assert!(load.tail.is_empty(), "noop update must not create a tail row");
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_persist_barrier_and_id_correlation() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    let app = fvoci_server::http::router(collab_app_state(&harness.app_url, true).await, None);
    let addr = spawn_server(app).await;
    let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
    let update = sample_hi_update();
    let request_id = Uuid::now_v7();

    let mut writer = connect_member(addr, &wiki.session.session_token).await;
    auth_and_join(&mut writer, &routing_key, 41).await;
    writer
        .send(Message::Binary(sync_update_frame(&routing_key, &update).into()))
        .await
        .unwrap();
    for _ in 0..8 {
        if let Some(WireFrame::Document {
            message: DocumentMessage::SyncStatus { applied: true },
            ..
        }) = recv_document_frame(&mut writer, 1).await
        {
            break;
        }
    }

    writer
        .send(Message::Binary(
            stateless_frame(&routing_key, &format!("persist:{request_id}")).into(),
        ))
        .await
        .unwrap();

    let mut saw_persisted = false;
    for _ in 0..8 {
        if let Some(WireFrame::Document {
            message: DocumentMessage::Stateless(body),
            ..
        }) = recv_document_frame(&mut writer, 1).await
        {
            if body == format!("persisted:{request_id}") {
                saw_persisted = true;
                break;
            }
        }
    }
    assert!(saw_persisted, "persist reply must echo request id");

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
    assert_eq!(load.snapshot_cutoff_seq, load.tail_seq);
    assert!(load.tail.is_empty(), "persist should compact tail");
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_primary_recycles_after_op_cap_then_edits_persist() {
    tokio::time::timeout(OP_CAP_TEST_TIMEOUT, async {
        let harness = TestDb::bootstrap().await;
        let wiki = setup_wiki_doc(&harness).await;
        let app =
            fvoci_server::http::router(collab_app_state(&harness.app_url, true).await, None);
        let addr = spawn_server(app).await;
        let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
        let update = sample_hi_update();
        let request_id = Uuid::now_v7();

        let mut writer = connect_member(addr, &wiki.session.session_token).await;
        auth_and_join(&mut writer, &routing_key, 51).await;

        // Each Step1 costs one Sync + one Inspect on the primary child (256-op cap).
        for round in 0..128 {
            writer
                .send(Message::Binary(sync_step1_frame(&routing_key, &[0, 0]).into()))
                .await
                .unwrap();
            let mut saw_step2 = false;
            let mut saw_server_step1 = false;
            let round_deadline = tokio::time::Instant::now() + Duration::from_millis(800);
            while tokio::time::Instant::now() < round_deadline
                && !(saw_step2 && saw_server_step1)
            {
                let remaining = round_deadline.saturating_duration_since(tokio::time::Instant::now());
                match recv_document_frame_within(
                    &mut writer,
                    remaining.min(Duration::from_millis(100)),
                )
                .await
                {
                    Some(WireFrame::Document {
                        message: DocumentMessage::Sync(SyncMessage {
                            step: SyncStep::Step2,
                            ..
                        }),
                        ..
                    }) => saw_step2 = true,
                    Some(WireFrame::Document {
                        message: DocumentMessage::Sync(SyncMessage {
                            step: SyncStep::Step1,
                            ..
                        }),
                        ..
                    }) if saw_step2 => saw_server_step1 = true,
                    _ => {}
                }
            }
            assert!(
                saw_step2 && saw_server_step1,
                "round {round}: expected Step2 then server Step1"
            );
        }

        writer
            .send(Message::Binary(sync_update_frame(&routing_key, &update).into()))
            .await
            .unwrap();
        assert!(
            wait_for_sync_applied(&mut writer, Duration::from_secs(5)).await,
            "distinct edit must apply after primary recycle"
        );

        writer
            .send(Message::Binary(
                stateless_frame(&routing_key, &format!("persist:{request_id}")).into(),
            ))
            .await
            .unwrap();
        assert!(
            wait_for_stateless_prefix(&mut writer, &format!("persisted:{request_id}"), Duration::from_secs(5)).await,
            "persist must succeed after op-cap recycle"
        );

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
        assert_eq!(load.tail_seq, 1);
        assert_eq!(load.snapshot_cutoff_seq, load.tail_seq);
        assert!(load.tail.is_empty(), "persist compacts accepted tail");
        assert_ne!(load.snapshot, vec![0, 0], "snapshot must hold the edit");
        harness.cleanup().await;
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "collab_primary_recycles_after_op_cap_then_edits_persist hung (>{OP_CAP_TEST_TIMEOUT:?}) including cleanup"
        )
    });
}

#[tokio::test]
async fn collab_readonly_first_then_writer_edits() {
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

    let mut reader = connect_member(addr, &wiki.session.session_token).await;
    auth_and_join(&mut reader, &routing_key, 61).await;

    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    sqlx::query("UPDATE fvoci.documents SET status = 'published' WHERE id = $1")
        .bind(wiki.document_id)
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;

    let mut writer = connect_member(addr, &wiki.session.session_token).await;
    auth_and_join(&mut writer, &routing_key, 62).await;

    let update = sample_hi_update();
    writer
        .send(Message::Binary(sync_update_frame(&routing_key, &update).into()))
        .await
        .unwrap();

    let mut writer_applied = false;
    for _ in 0..8 {
        if let Some(WireFrame::Document {
            message: DocumentMessage::SyncStatus { applied: true },
            ..
        }) = recv_document_frame(&mut writer, 1).await
        {
            writer_applied = true;
            break;
        }
    }
    assert!(writer_applied, "writer should apply after readonly-first room");

    let mut reader_saw = false;
    for _ in 0..8 {
        if let Some(WireFrame::Document {
            message: DocumentMessage::Sync(SyncMessage {
                step: SyncStep::Update,
                ..
            }),
            ..
        }) = recv_document_frame(&mut reader, 1).await
        {
            reader_saw = true;
            break;
        }
    }
    assert!(reader_saw, "reader should receive broadcast after writer edit");
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_delete_only_round_trip_persists() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    let app = fvoci_server::http::router(collab_app_state(&harness.app_url, true).await, None);
    let addr = spawn_server(app).await;
    let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
    let base = engine_fixture("delete_only_base.v1");
    let delete_only = engine_fixture("delete_only.v1");
    let request_id = Uuid::now_v7();

    let mut writer = connect_member(addr, &wiki.session.session_token).await;
    auth_and_join(&mut writer, &routing_key, 71).await;

    for payload in [&base, &delete_only] {
        writer
            .send(Message::Binary(sync_update_frame(&routing_key, payload).into()))
            .await
            .unwrap();
        for _ in 0..8 {
            if let Some(WireFrame::Document {
                message: DocumentMessage::SyncStatus { applied: true },
                ..
            }) = recv_document_frame(&mut writer, 1).await
            {
                break;
            }
        }
    }

    writer
        .send(Message::Binary(
            stateless_frame(&routing_key, &format!("persist:{request_id}")).into(),
        ))
        .await
        .unwrap();
    for _ in 0..8 {
        if let Some(WireFrame::Document {
            message: DocumentMessage::Stateless(body),
            ..
        }) = recv_document_frame(&mut writer, 1).await
        {
            if body.starts_with("persisted:") {
                break;
            }
        }
    }

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
    assert!(load.tail.is_empty());
    assert!(
        !load.snapshot.is_empty() && load.snapshot != [0, 0],
        "delete-only state should be snapshotted"
    );
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_two_readonly_joins_then_writer_edits() {
    run_lifecycle_test("collab_two_readonly_joins_then_writer_edits", async {
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

        let hub = CollabHub::new(test_collab_config(4, 30_000), wiki.session.pool.clone());
        let key = (wiki.session.workspace_id, wiki.document_id);
        assert!(hub_join_readonly(&hub, &wiki, 81).await.is_ok());
        assert!(hub_join_readonly(&hub, &wiki, 82).await.is_ok());

        let admin = PgPoolOptions::new()
            .max_connections(1)
            .connect(&harness.admin_url)
            .await
            .unwrap();
        sqlx::query("UPDATE fvoci.documents SET status = 'published' WHERE id = $1")
            .bind(wiki.document_id)
            .execute(&admin)
            .await
            .unwrap();
        admin.close().await;

        let conn_id = hub_join(&hub, &wiki, 83).await.expect("writer join");
        let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
        hub.send_frame(
            key,
            conn_id,
            sync_update_frame(&routing_key, &sample_hi_update()),
        )
        .await;

        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let mut tail_len = 0usize;
        while tokio::time::Instant::now() < deadline {
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
            tail_len = load.tail.len();
            if tail_len == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(tail_len, 1, "writer update must persist after two readonly joins");
        hub.shutdown().await;
        harness.cleanup().await;
    })
    .await;
}

#[tokio::test]
async fn collab_committed_update_survives_primary_apply_fail_reload() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    let app = fvoci_server::http::router(collab_app_state(&harness.app_url, true).await, None);
    let addr = spawn_server(app).await;
    let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
    let update = sample_hi_update();

    let mut writer = connect_member(addr, &wiki.session.session_token).await;
    auth_and_join(&mut writer, &routing_key, 91).await;
    arm_force_primary_apply_fail();
    writer
        .send(Message::Binary(sync_update_frame(&routing_key, &update).into()))
        .await
        .unwrap();
    assert!(
        wait_for_sync_applied(&mut writer, Duration::from_secs(5)).await,
        "durable commit must not be rejected when primary apply fails"
    );
    disarm_force_primary_apply_fail();

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
async fn collab_lifecycle_foreign_leave_does_not_evict_member() {
    run_lifecycle_test("collab_lifecycle_foreign_leave_does_not_evict_member", async {
        let harness = TestDb::bootstrap().await;
        let wiki = setup_wiki_doc(&harness).await;
        let hub = CollabHub::new(test_collab_config(4, 200), wiki.session.pool.clone());
        let key = (wiki.session.workspace_id, wiki.document_id);
        let conn_id = hub_join(&hub, &wiki, 1).await.expect("join");
        assert_eq!(hub.room_lifecycle_phase(key).await, RoomLifecyclePhase::Live);

        hub.leave_room(key, Uuid::now_v7()).await;
        assert_eq!(hub.room_lifecycle_phase(key).await, RoomLifecyclePhase::Live);

        hub.leave_room(key, conn_id).await;
        wait_for_phase(&hub, key, RoomLifecyclePhase::Absent).await;
        hub.shutdown().await;
        harness.cleanup().await;
    })
    .await;
}

#[tokio::test]
async fn collab_lifecycle_shutdown_during_booting_reclaims_slot() {
    run_lifecycle_test("collab_lifecycle_shutdown_during_booting_reclaims_slot", async {
        let harness = TestDb::bootstrap().await;
        let wiki = setup_wiki_doc(&harness).await;
        let release = arm_spawn_room_block(wiki.document_id).await;
        let hub = Arc::new(CollabHub::new(
            test_collab_config(4, 30_000),
            wiki.session.pool.clone(),
        ));
        let key = (wiki.session.workspace_id, wiki.document_id);
        let slots_before = hub.available_room_slots();

        let join_task = tokio::spawn({
            let hub = hub.clone();
            let wiki = wiki.clone_fixture();
            async move { hub_join(&hub, &wiki, 1).await }
        });
        wait_for_booting(&hub, key).await;
        assert_eq!(hub.available_room_slots(), slots_before - 1);

        let shutdown_task = tokio::spawn({
            let hub = hub.clone();
            async move { hub.shutdown().await }
        });
        for _ in 0..32 {
            tokio::task::yield_now().await;
        }

        release.send(()).expect("release startup gate");
        let join_result = join_task.await.expect("join task");
        assert!(join_result.is_err(), "join during shutdown must fail");
        shutdown_task.await.expect("shutdown task");
        assert_eq!(hub.available_room_slots(), slots_before);
        assert_eq!(hub.room_lifecycle_phase(key).await, RoomLifecyclePhase::Absent);
        disarm_spawn_room_block(wiki.document_id).await;
        harness.cleanup().await;
    })
    .await;
}
