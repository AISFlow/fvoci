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
use fvoci_server::collab::wire::{
    encode, AuthMessage, CollabRoomName, DocumentMessage, SyncMessage, SyncStep, WireFrame,
};
use fvoci_server::collab::y_sync::encode_sync_payload;
use fvoci_server::collab::CollabHub;
use fvoci_server::db::documents::CreateDocumentInput;
use fvoci_server::db::workspace;
use fvoci_server::db::{documents, migrate, pool, Db};
use fvoci_server::http::rate_limit::RateLimiter;
use fvoci_server::http::state::AppState;
use rand::RngCore;
use serde_json::Value;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

pub const PEPPER: &str =
    r#"{"test":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#;
pub const PUBLIC_ORIGIN: &str = "http://localhost";

pub struct TestDb {
    pub admin_url: String,
    pub app_url: String,
    db_name: String,
    role_name: String,
}

pub struct SessionFixture {
    pub pool: PgPool,
    pub user_id: Uuid,
    pub session_id: Uuid,
    pub workspace_id: Uuid,
    pub session_token: String,
}

pub struct WikiDocFixture {
    pub session: SessionFixture,
    pub document_id: Uuid,
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
        include_str!("../../scripts/grant-app-role.sql").replace(":\"app_role\"", &quoted_role);
    for statement in grants.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        sqlx::query(statement).execute(pool).await.expect("grant");
    }
}

impl TestDb {
    pub async fn bootstrap() -> Self {
        let admin_base = std::env::var("TEST_DATABASE_URL")
            .or_else(|_| std::env::var("FVOCI_TEST_DATABASE_URL"))
            .expect("TEST_DATABASE_URL missing; collab projection tests require real PostgreSQL");

        let db_name = format!("fvoci_collab_proj_{}", Uuid::now_v7().simple());
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

    pub async fn cleanup(self) {
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

pub async fn setup_owner_session(harness: &TestDb) -> SessionFixture {
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
        .bind("Projection WS")
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

pub fn tiny_output_project_collab_config(max_rooms: usize, idle_evict_ms: u64) -> CollabConfig {
    let mut cfg = test_collab_config(max_rooms, idle_evict_ms);
    cfg.limits.max_project_json_bytes = 8;
    cfg
}

pub fn test_collab_config(max_rooms: usize, idle_evict_ms: u64) -> CollabConfig {
    CollabConfig {
        engine_bin: fvoci_server::collab::config::require_collab_engine_for_tests(),
        limits: collab_engine::Limits::for_tests(),
        max_rooms,
        max_collab_sockets: max_rooms * 16,
        max_collab_sockets_per_session: 4,
        max_connections_per_room: 16,
        max_queued_room_ops: 128,
        max_pending_bytes_per_connection: 4 * 1024 * 1024,
        max_outbound_frames_per_connection: 64,
        max_outbound_bytes_per_connection: 4 * 1024 * 1024,
        outbound_send_deadline_ms: 5_000,
        max_ws_frame_bytes: fvoci_server::collab::wire::Limits::DEFAULT.max_frame_bytes,
        max_ws_message_bytes: fvoci_server::collab::wire::Limits::DEFAULT.max_frame_bytes,
        auth_wait_ms: 30_000,
        max_pre_auth_outbound_frames: 2,
        max_inbound_messages_per_window: 256,
        inbound_message_window_ms: 1_000,
        idle_evict_ms,
        revoke_poll_ms: 5_000,
        client_id_ttl_ms: 60_000,
    }
}

pub async fn setup_wiki_doc(harness: &TestDb) -> WikiDocFixture {
    let session = setup_owner_session(harness).await;
    let created = documents::create_wiki_document(
        &session.pool,
        session.workspace_id,
        session.user_id,
        session.session_id,
        CreateDocumentInput {
            parent_id: None,
            title: "Projection doc",
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

pub async fn collab_app_state(app_url: &str, cfg: CollabConfig) -> (AppState, Arc<CollabHub>) {
    let pool = pool::connect_app(app_url).await.expect("app pool");
    let hub = Arc::new(CollabHub::new(cfg, pool));
    let state = AppState {
        auth: Arc::new(AuthService {
            db: Db::new(hub.pool().clone()),
            password_keys: Keyring::parse(PEPPER, "test").expect("pepper"),
        }),
        branding_name: "FVOCI".to_string(),
        public_origin: PUBLIC_ORIGIN.to_string(),
        cookie_secure: false,
        rate_limiter: RateLimiter::new(),
        collab: Some(hub.clone()),
    };
    (state, hub)
}

pub struct TestServer {
    pub addr: SocketAddr,
    hub: Arc<CollabHub>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    join: Option<tokio::task::JoinHandle<()>>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(join) = self.join.take() {
            join.abort();
        }
    }
}

impl TestServer {
    pub async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(join) = self.join.take() {
            if let Err(error) = join.await {
                if !error.is_cancelled() {
                    panic!("test server task failed: {error}");
                }
            }
        }
        self.hub.shutdown().await;
    }
}

pub struct TestRun {
    pub harness: TestDb,
    servers: Vec<TestServer>,
}

impl TestRun {
    pub fn new(harness: TestDb) -> Self {
        Self {
            harness,
            servers: Vec::new(),
        }
    }

    pub async fn spawn_router(&mut self, app_url: &str, cfg: CollabConfig) -> SocketAddr {
        let (state, hub) = collab_app_state(app_url, cfg).await;
        self.spawn_router_state(state, hub).await
    }

    pub async fn spawn_router_state(&mut self, state: AppState, hub: Arc<CollabHub>) -> SocketAddr {
        let server = spawn_server(fvoci_server::http::router(state, None), hub).await;
        let addr = server.addr;
        self.servers.push(server);
        addr
    }

    pub async fn finish(mut self) {
        while let Some(server) = self.servers.pop() {
            server.shutdown().await;
        }
        self.harness.cleanup().await;
    }
}

pub async fn spawn_server(app: Router, hub: Arc<CollabHub>) -> TestServer {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let join = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async {
            shutdown_rx.await.ok();
        })
        .await
        .expect("serve test collab server");
    });
    TestServer {
        addr,
        hub,
        shutdown: Some(shutdown_tx),
        join: Some(join),
    }
}

pub fn delete_only_base_update() -> Vec<u8> {
    engine_fixture("delete_only_base.v1")
}

pub fn delete_only_update() -> Vec<u8> {
    engine_fixture("delete_only.v1")
}

pub fn delete_only_json_before() -> Value {
    expectations()["delete_only"]["prosemirror_json_before"].clone()
}

pub fn delete_only_json_after() -> Value {
    expectations()["delete_only"]["prosemirror_json_after"].clone()
}

pub fn engine_fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("crates/collab-engine/fixtures")
            .join(name),
    )
    .unwrap_or_else(|e| panic!("read fixture {name}: {e}"))
}

pub fn expectations() -> Value {
    serde_json::from_str(include_str!(
        "../../crates/collab-engine/fixtures/expectations.json"
    ))
    .expect("expectations.json")
}

fn collab_ws_request(
    addr: SocketAddr,
    session_token: &str,
) -> tokio_tungstenite::tungstenite::http::Request<()> {
    let mut request = format!("ws://{addr}/collab").into_client_request().unwrap();
    request
        .headers_mut()
        .insert("origin", PUBLIC_ORIGIN.parse().unwrap());
    request.headers_mut().insert(
        "cookie",
        format!("fvoci_session={session_token}").parse().unwrap(),
    );
    request
}

pub async fn connect_member(
    addr: SocketAddr,
    session_token: &str,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    tokio_tungstenite::connect_async(collab_ws_request(addr, session_token))
        .await
        .expect("connect")
        .0
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

pub async fn auth_and_join(
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

pub fn sync_step1_frame(routing_key: &str, state_vector: &[u8]) -> Vec<u8> {
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

pub async fn complete_sync_handshake(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    routing_key: &str,
) {
    ws.send(Message::Binary(
        sync_step1_frame(routing_key, &[0, 0]).into(),
    ))
    .await
    .unwrap();
    let mut saw_step2 = false;
    let mut saw_server_step1 = false;
    for _ in 0..16 {
        if let Some(WireFrame::Document {
            message: DocumentMessage::Sync(SyncMessage { step, .. }),
            ..
        }) = recv_document_frame(ws, 1).await
        {
            match step {
                SyncStep::Step2 if !saw_step2 => saw_step2 = true,
                SyncStep::Step1 if saw_step2 => {
                    saw_server_step1 = true;
                    break;
                }
                _ => {}
            }
        }
    }
    assert!(saw_step2, "client must receive Step2");
    assert!(
        saw_server_step1,
        "client must receive server Step1 after Step2"
    );
}

pub fn sync_update_frame(routing_key: &str, update: &[u8]) -> Vec<u8> {
    encode(&WireFrame::Document {
        routing_key: routing_key.to_string(),
        room: CollabRoomName::parse(routing_key),
        message: DocumentMessage::Sync(SyncMessage {
            step: SyncStep::Update,
            y_protocol: encode_sync_payload(SyncStep::Update, update),
        }),
    })
    .expect("encode update")
}

pub fn stateless_frame(routing_key: &str, payload: &str) -> Vec<u8> {
    encode(&WireFrame::Document {
        routing_key: routing_key.to_string(),
        room: CollabRoomName::parse(routing_key),
        message: DocumentMessage::Stateless(payload.to_string()),
    })
    .expect("encode stateless")
}

pub async fn recv_document_frame(
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

pub async fn wait_for_stateless_exact(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    expected: &str,
    within: Duration,
) -> bool {
    let deadline = tokio::time::Instant::now() + within;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if let Some(WireFrame::Document {
            message: DocumentMessage::Stateless(body),
            ..
        }) = recv_document_frame(ws, 1).await
        {
            if body == expected {
                return true;
            }
        }
        tokio::time::sleep(remaining.min(Duration::from_millis(20))).await;
    }
    false
}

pub async fn persist_barrier(
    addr: SocketAddr,
    session_token: &str,
    routing_key: &str,
    client_id: u32,
    request_id: Uuid,
) {
    let mut writer = connect_member(addr, session_token).await;
    auth_and_join(&mut writer, routing_key, client_id).await;
    writer
        .send(Message::Binary(
            stateless_frame(routing_key, &format!("persist:{request_id}")).into(),
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
        "persist must acknowledge with persisted:{request_id}"
    );
}

pub async fn wait_for_sync_applied(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    within: Duration,
) -> bool {
    let deadline = tokio::time::Instant::now() + within;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let msg = tokio::time::timeout(remaining.min(Duration::from_millis(200)), ws.next())
            .await
            .ok()
            .flatten();
        let Some(Ok(Message::Binary(bytes))) = msg else {
            continue;
        };
        if let Ok(WireFrame::Document {
            message: DocumentMessage::SyncStatus { applied: true },
            ..
        }) = fvoci_server::collab::wire::decode(&bytes)
        {
            return true;
        }
        if let Ok(WireFrame::Document {
            message: DocumentMessage::SyncStatus { applied: false },
            ..
        }) = fvoci_server::collab::wire::decode(&bytes)
        {
            return false;
        }
    }
    false
}

pub async fn get_document_body(
    addr: SocketAddr,
    session_token: &str,
    workspace_id: Uuid,
    document_id: Uuid,
) -> Value {
    let client = reqwest::Client::builder()
        .cookie_store(true)
        .build()
        .expect("client");
    let url =
        format!("http://{addr}/api/v1/workspaces/{workspace_id}/documents/{document_id}/body");
    let response = client
        .get(&url)
        .header("origin", PUBLIC_ORIGIN)
        .header("cookie", format!("fvoci_session={session_token}"))
        .send()
        .await
        .expect("GET body");
    assert_eq!(response.status(), 200, "GET body must succeed");
    response.json().await.expect("body json")
}
