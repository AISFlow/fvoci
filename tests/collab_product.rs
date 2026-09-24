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
use fvoci_server::collab::awareness::{decode_awareness, encode_awareness, AwarenessUpdate};
use fvoci_server::collab::config::CollabConfig;
use fvoci_server::collab::guard::RoomGuard;
use fvoci_server::collab::hub::RoomLifecyclePhase;
use fvoci_server::collab::room::{
    arm_append_in_tx_reject_barrier, arm_append_revoke_barrier, arm_force_primary_apply_fail,
    arm_force_primary_load_fail, arm_spawn_room_block, disarm_append_in_tx_reject_barrier,
    disarm_append_revoke_barrier, disarm_force_primary_apply_fail, disarm_force_primary_load_fail,
    disarm_spawn_room_block, AuthenticatedConnection, CollabSession, JoinError, RoomClientEvent,
    RoomJoin,
};
use fvoci_server::collab::transport::take_data_frame_send_budget;
use fvoci_server::collab::wire::{
    encode, AuthMessage, CollabKind, CollabRoomName, DocumentMessage, SyncMessage, SyncStep,
    WireFrame,
};
use fvoci_server::collab::y_sync::{encode_sync_payload, parse_sync_payload};
use fvoci_server::collab::CollabHub;
use fvoci_server::db::collab::{load_collab_document, resolve_collab_admission};
use fvoci_server::db::collab_delivery::{
    arm_delivery_read_barrier, arm_force_delivery_read_fail, arm_force_delivery_tx_error,
    check_delivery_admission, delivery_read_count, disarm_delivery_read_barrier,
    disarm_force_delivery_read_fail, disarm_force_delivery_tx_error, reset_delivery_read_count,
    DeliveryAdmission,
};
use fvoci_server::db::documents::{empty_document_json, CreateDocumentInput};
use fvoci_server::db::identity::revoke_session;
use fvoci_server::db::workspace;
use fvoci_server::db::{documents, migrate, pool, Db};
use fvoci_server::http::rate_limit::RateLimiter;
use fvoci_server::http::state::AppState;
use rand::RngCore;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use tokio::sync::{mpsc, watch};
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
    test_collab_config_with_revoke(max_rooms, idle_evict_ms, 5_000)
}

fn test_collab_config_with_revoke(
    max_rooms: usize,
    idle_evict_ms: u64,
    revoke_poll_ms: u64,
) -> CollabConfig {
    CollabConfig {
        engine_bin: engine_bin(),
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
        revoke_poll_ms,
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
                locale: "en".into(),
            },
            client_id,
            read_only: false,
            routing_key,
        },
        events: events_tx,
        cancel: None,
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
                locale: "en".into(),
            },
            client_id,
            read_only: true,
            routing_key,
        },
        events: events_tx,
        cancel: None,
    };
    hub.join_room((wiki.session.workspace_id, wiki.document_id), join)
        .await?;
    Ok(conn_id)
}

async fn wait_for_cancel_signal(
    mut cancel_rx: watch::Receiver<Option<fvoci_server::collab::room::ConnectionCancel>>,
    deadline: Duration,
) -> bool {
    tokio::time::timeout(deadline, async {
        while cancel_rx.changed().await.is_ok() {
            if cancel_rx.borrow().is_some() {
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or(false)
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

async fn collab_app_state_with_config(app_url: &str, cfg: CollabConfig) -> AppState {
    let pool = pool::connect_app(app_url).await.expect("app pool");
    AppState {
        auth: Arc::new(AuthService {
            db: Db::new(pool.clone()),
            password_keys: Keyring::parse(PEPPER, "test").expect("pepper"),
        }),
        branding_name: "FVOCI".to_string(),
        public_origin: PUBLIC_ORIGIN.to_string(),
        cookie_secure: false,
        rate_limiter: RateLimiter::new(),
        collab: Some(Arc::new(CollabHub::new(cfg, pool))),
    }
}

async fn collab_app_state(app_url: &str, with_collab: bool) -> AppState {
    let pool = pool::connect_app(app_url).await.expect("app pool");
    let collab = if with_collab {
        let cfg = test_collab_config(4, 30_000);
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

/// Raw Yjs update that inserts `Y.Text("hi")` at the fragment name `prosemirror`.
/// Valid for Apply/broadcast; not a Tiptap XmlFragment. Success persist/recycle
/// tests reuse pinned engine XML fixtures instead of this payload.
fn sample_hi_update() -> Vec<u8> {
    hex::decode("0101e8eda5a2070004010b70726f73656d6972726f7202686900").expect("fixture")
}

/// Pinned engine Tiptap XmlFragment update (`pending_u1.v1`: paragraph "one").
fn tiptap_xml_pending_u1() -> Vec<u8> {
    engine_fixture("pending_u1.v1")
}

/// Independent sequential follow-up (`pending_u2.v1`: paragraph "two한글").
fn tiptap_xml_pending_u2() -> Vec<u8> {
    engine_fixture("pending_u2.v1")
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

#[derive(Debug)]
enum PersistOutcome {
    Persisted,
    Failed(String),
    Closed(Option<String>),
    Timeout,
}

async fn wait_for_persist_outcome(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    request_id: Uuid,
    within: Duration,
) -> PersistOutcome {
    let persisted = format!("persisted:{request_id}");
    let failed = format!("persist-failed:{request_id}");
    let deadline = tokio::time::Instant::now() + within;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match recv_document_frame_within(ws, remaining.min(Duration::from_millis(200))).await {
            Some(WireFrame::Document {
                message: DocumentMessage::Stateless(body),
                ..
            }) => {
                if body == persisted {
                    return PersistOutcome::Persisted;
                }
                if body == failed || body.starts_with("persist-failed:") {
                    return PersistOutcome::Failed(body);
                }
            }
            Some(WireFrame::Document {
                message: DocumentMessage::Close { reason },
                ..
            }) => return PersistOutcome::Closed(reason),
            _ => {}
        }
    }
    PersistOutcome::Timeout
}

async fn expect_persisted(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    request_id: Uuid,
    within: Duration,
    context: &str,
) {
    match wait_for_persist_outcome(ws, request_id, within).await {
        PersistOutcome::Persisted => {}
        PersistOutcome::Failed(body) => panic!(
            "{context}: expected persisted:{request_id}, got {body} (fail promptly, do not wait out the barrier)"
        ),
        PersistOutcome::Closed(reason) => panic!(
            "{context}: unexpected Close before persist ack for {request_id} ({reason:?})"
        ),
        PersistOutcome::Timeout => panic!(
            "{context}: timed out waiting for persisted:{request_id} ({within:?})"
        ),
    }
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

async fn connect_member(
    addr: SocketAddr,
    session_token: &str,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    tokio_tungstenite::connect_async(collab_ws_request(addr, session_token))
        .await
        .expect("connect")
        .0
}

async fn try_connect_member(
    addr: SocketAddr,
    session_token: &str,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    tokio_tungstenite::tungstenite::Error,
> {
    tokio_tungstenite::connect_async(collab_ws_request(addr, session_token))
        .await
        .map(|(stream, _)| stream)
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
        .send(Message::Binary(
            sync_update_frame(&routing_key, &update).into(),
        ))
        .await
        .unwrap();
    assert!(
        wait_for_sync_applied(&mut writer, Duration::from_secs(5)).await,
        "initial update must ack applied:true before reconnect Step1"
    );

    writer
        .send(Message::Binary(
            sync_step1_frame(&routing_key, &[0, 0]).into(),
        ))
        .await
        .unwrap();

    let mut saw_step2 = false;
    let mut saw_server_step1 = false;
    for _ in 0..12 {
        let frame = recv_document_frame(&mut writer, 1).await;
        if let Some(WireFrame::Document {
            message: DocumentMessage::Sync(SyncMessage { step, y_protocol }),
            ..
        }) = frame
        {
            match step {
                SyncStep::Step2 if !saw_step2 => saw_step2 = true,
                SyncStep::Step1 if saw_step2 => {
                    let (_, sv) = parse_sync_payload(&y_protocol, 4 * 1024 * 1024).unwrap();
                    assert!(!sv.is_empty(), "server Step1 must carry a state vector");
                    saw_server_step1 = true;
                    break;
                }
                _ => {}
            }
        }
    }
    assert!(saw_step2, "client should receive Step2");
    assert!(
        saw_server_step1,
        "client should receive server Step1 after Step2"
    );
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
    assert!(
        saw_rejected,
        "byte-empty update must be rejected as malformed"
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
        .send(Message::Binary(
            sync_update_frame(&routing_key, &[0, 0]).into(),
        ))
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
    assert!(
        load.tail.is_empty(),
        "noop update must not create a tail row"
    );
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_persist_barrier_and_id_correlation() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    let app = fvoci_server::http::router(collab_app_state(&harness.app_url, true).await, None);
    let addr = spawn_server(app).await;
    let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
    let first = tiptap_xml_pending_u1();
    let second = tiptap_xml_pending_u2();
    let first_id = Uuid::now_v7();
    let second_id = Uuid::now_v7();

    let mut writer = connect_member(addr, &wiki.session.session_token).await;
    auth_and_join(&mut writer, &routing_key, 41).await;

    writer
        .send(Message::Binary(
            sync_update_frame(&routing_key, &first).into(),
        ))
        .await
        .unwrap();
    assert!(
        wait_for_sync_applied(&mut writer, Duration::from_secs(5)).await,
        "first sequential Tiptap XmlFragment edit must apply"
    );
    writer
        .send(Message::Binary(
            stateless_frame(&routing_key, &format!("persist:{first_id}")).into(),
        ))
        .await
        .unwrap();
    expect_persisted(
        &mut writer,
        first_id,
        Duration::from_secs(5),
        "first persist barrier",
    )
    .await;

    writer
        .send(Message::Binary(
            sync_update_frame(&routing_key, &second).into(),
        ))
        .await
        .unwrap();
    assert!(
        wait_for_sync_applied(&mut writer, Duration::from_secs(5)).await,
        "second independent sequential Tiptap edit must apply after commit"
    );
    writer
        .send(Message::Binary(
            stateless_frame(&routing_key, &format!("persist:{second_id}")).into(),
        ))
        .await
        .unwrap();
    expect_persisted(
        &mut writer,
        second_id,
        Duration::from_secs(5),
        "second persist barrier",
    )
    .await;

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
    assert_ne!(
        load.snapshot,
        vec![0, 0],
        "snapshot must hold sequential Tiptap XmlFragment edits"
    );
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
        let update = tiptap_xml_pending_u1();
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
        expect_persisted(
            &mut writer,
            request_id,
            Duration::from_secs(5),
            "persist must succeed after op-cap recycle",
        )
        .await;

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
        .send(Message::Binary(
            sync_update_frame(&routing_key, &update).into(),
        ))
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
    assert!(
        writer_applied,
        "writer should apply after readonly-first room"
    );

    let mut reader_saw = false;
    for _ in 0..8 {
        if let Some(WireFrame::Document {
            message:
                DocumentMessage::Sync(SyncMessage {
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
    assert!(
        reader_saw,
        "reader should receive broadcast after writer edit"
    );
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
            .send(Message::Binary(
                sync_update_frame(&routing_key, payload).into(),
            ))
            .await
            .unwrap();
        assert!(
            wait_for_sync_applied(&mut writer, Duration::from_secs(5)).await,
            "each delete-only round-trip update must ack applied:true"
        );
    }

    writer
        .send(Message::Binary(
            stateless_frame(&routing_key, &format!("persist:{request_id}")).into(),
        ))
        .await
        .unwrap();
    expect_persisted(
        &mut writer,
        request_id,
        Duration::from_secs(5),
        "delete-only persist must echo persisted:<id>",
    )
    .await;

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

async fn add_session_for_user(
    pool: &PgPool,
    user_id: Uuid,
) -> fvoci_server::auth::token::SessionToken {
    let token = new_token();
    let session_id = Uuid::now_v7();
    let expires = Utc::now() + ChronoDuration::days(30);
    let mut tx = pool.begin().await.unwrap();
    fvoci_server::db::identity::create_session(&mut tx, session_id, user_id, &token.hash, expires)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    token
}

async fn setup_second_member(harness: &TestDb, wiki: &WikiDocFixture) -> SessionFixture {
    let admin = PgPoolOptions::new()
        .max_connections(1)
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
    .bind(format!("member-{user_id}@example.com"))
    .bind(&hash)
    .bind("Peer")
    .execute(&admin)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO fvoci.memberships (workspace_id, user_id, role) VALUES ($1, $2, 'member')",
    )
    .bind(wiki.session.workspace_id)
    .bind(user_id)
    .execute(&admin)
    .await
    .unwrap();
    admin.close().await;

    let token = new_token();
    let session_id = Uuid::now_v7();
    let expires = Utc::now() + ChronoDuration::days(30);
    let mut tx = wiki.session.pool.begin().await.unwrap();
    fvoci_server::db::identity::create_session(&mut tx, session_id, user_id, &token.hash, expires)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    SessionFixture {
        pool: wiki.session.pool.clone(),
        user_id,
        session_id,
        workspace_id: wiki.session.workspace_id,
        session_token: token.token,
    }
}

fn collab_session_from(fixture: &SessionFixture, given_name: &str) -> CollabSession {
    CollabSession {
        session_id: fixture.session_id,
        user_id: fixture.user_id,
        given_name: given_name.into(),
        family_name: None,
        locale: "en".into(),
    }
}

#[tokio::test]
async fn collab_append_revoke_barrier_rejects_writer_not_room() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    let app = fvoci_server::http::router(collab_app_state(&harness.app_url, true).await, None);
    let addr = spawn_server(app).await;
    let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
    let update = sample_hi_update();
    let writer_token = add_session_for_user(&wiki.session.pool, wiki.session.user_id).await;

    let mut reader = connect_member(addr, &wiki.session.session_token).await;
    auth_and_join(&mut reader, &routing_key, 210).await;
    let mut writer = connect_member(addr, &writer_token.token).await;
    auth_and_join(&mut writer, &routing_key, 211).await;

    let (reached_rx, proceed_tx) = arm_append_revoke_barrier(wiki.document_id).await;
    writer
        .send(Message::Binary(
            sync_update_frame(&routing_key, &update).into(),
        ))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), reached_rx)
        .await
        .expect("append barrier must be reached after validation")
        .expect("barrier signal");
    revoke_session(
        &wiki.session.pool,
        &writer_token.hash,
        Some(wiki.session.user_id),
    )
    .await
    .expect("revoke writer session");
    proceed_tx.send(()).expect("release append barrier");
    disarm_append_revoke_barrier(wiki.document_id).await;

    assert!(
        wait_for_ws_close(&mut writer, Duration::from_secs(3)).await,
        "revoked writer must be closed after definite append rejection"
    );
    reader
        .send(Message::Binary(
            sync_step1_frame(&routing_key, &[0, 0]).into(),
        ))
        .await
        .unwrap();
    let mut reader_still_live = false;
    let mut saw_rejected_update = false;
    for _ in 0..12 {
        match recv_document_frame(&mut reader, 1).await {
            Some(WireFrame::Document {
                message:
                    DocumentMessage::Sync(SyncMessage {
                        step: SyncStep::Update,
                        ..
                    }),
                ..
            }) => {
                saw_rejected_update = true;
            }
            Some(WireFrame::Document {
                message:
                    DocumentMessage::Sync(SyncMessage {
                        step: SyncStep::Step2,
                        ..
                    }),
                ..
            }) => {
                reader_still_live = true;
                break;
            }
            Some(_) => {}
            None => break,
        }
    }
    assert!(
        !saw_rejected_update,
        "rejected writer update must not be delivered to a healthy peer"
    );
    assert!(
        reader_still_live,
        "reader must remain live after writer-only rejection"
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
    assert!(
        load.tail.is_empty(),
        "rejected writer update must not enter durable save"
    );

    let fresh_writer_token = add_session_for_user(&wiki.session.pool, wiki.session.user_id).await;
    let mut fresh_writer = connect_member(addr, &fresh_writer_token.token).await;
    auth_and_join(&mut fresh_writer, &routing_key, 212).await;
    fresh_writer
        .send(Message::Binary(
            sync_update_frame(&routing_key, &update).into(),
        ))
        .await
        .unwrap();
    let mut reader_saw_followup = false;
    for _ in 0..12 {
        if let Some(WireFrame::Document {
            message:
                DocumentMessage::Sync(SyncMessage {
                    step: SyncStep::Update,
                    ..
                }),
            ..
        }) = recv_document_frame(&mut reader, 1).await
        {
            reader_saw_followup = true;
            break;
        }
    }
    assert!(
        reader_saw_followup,
        "reader must receive a later update from a live writer after barrier rejection"
    );
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_append_in_tx_reject_barrier_rejects_writer_not_room() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    let app = fvoci_server::http::router(collab_app_state(&harness.app_url, true).await, None);
    let addr = spawn_server(app).await;
    let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
    let update = sample_hi_update();
    let writer_token = add_session_for_user(&wiki.session.pool, wiki.session.user_id).await;

    let mut reader = connect_member(addr, &wiki.session.session_token).await;
    auth_and_join(&mut reader, &routing_key, 220).await;
    let mut writer = connect_member(addr, &writer_token.token).await;
    auth_and_join(&mut writer, &routing_key, 221).await;

    let (reached_rx, proceed_tx) = arm_append_in_tx_reject_barrier(wiki.document_id).await;
    writer
        .send(Message::Binary(
            sync_update_frame(&routing_key, &update).into(),
        ))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), reached_rx)
        .await
        .expect("in-tx barrier must be reached after the locking precheck")
        .expect("barrier signal");
    revoke_session(
        &wiki.session.pool,
        &writer_token.hash,
        Some(wiki.session.user_id),
    )
    .await
    .expect("revoke writer session");
    proceed_tx.send(()).expect("release in-tx barrier");
    disarm_append_in_tx_reject_barrier(wiki.document_id).await;

    assert!(
        wait_for_ws_close(&mut writer, Duration::from_secs(3)).await,
        "writer revoked inside append must close after Forbidden"
    );
    reader
        .send(Message::Binary(
            sync_step1_frame(&routing_key, &[0, 0]).into(),
        ))
        .await
        .unwrap();
    let mut reader_still_live = false;
    let mut saw_rejected_update = false;
    for _ in 0..12 {
        match recv_document_frame(&mut reader, 1).await {
            Some(WireFrame::Document {
                message:
                    DocumentMessage::Sync(SyncMessage {
                        step: SyncStep::Update,
                        ..
                    }),
                ..
            }) => {
                saw_rejected_update = true;
            }
            Some(WireFrame::Document {
                message:
                    DocumentMessage::Sync(SyncMessage {
                        step: SyncStep::Step2,
                        ..
                    }),
                ..
            }) => {
                reader_still_live = true;
                break;
            }
            Some(_) => {}
            None => break,
        }
    }
    assert!(
        !saw_rejected_update,
        "in-tx rejected update must not enter a healthy client"
    );
    assert!(
        reader_still_live,
        "reader must remain live after in-tx writer rejection"
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
    assert!(
        load.tail.is_empty(),
        "in-tx rejected update must not enter durable save"
    );
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_malformed_step1_closes_offender_healthy_peer_syncs() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    let app = fvoci_server::http::router(collab_app_state(&harness.app_url, true).await, None);
    let addr = spawn_server(app).await;
    let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);

    let mut offender = connect_member(addr, &wiki.session.session_token).await;
    auth_and_join(&mut offender, &routing_key, 201).await;
    offender
        .send(Message::Binary(
            sync_step1_frame(&routing_key, &[0xff, 0xff, 0xff]).into(),
        ))
        .await
        .unwrap();
    assert!(
        wait_for_ws_close(&mut offender, Duration::from_secs(3)).await,
        "malformed Step1 must close the offending connection"
    );

    let mut healthy = connect_member(addr, &wiki.session.session_token).await;
    auth_and_join(&mut healthy, &routing_key, 202).await;
    healthy
        .send(Message::Binary(
            sync_step1_frame(&routing_key, &[0, 0]).into(),
        ))
        .await
        .unwrap();
    let mut saw_step2 = false;
    for _ in 0..12 {
        if let Some(WireFrame::Document {
            message:
                DocumentMessage::Sync(SyncMessage {
                    step: SyncStep::Step2,
                    ..
                }),
            ..
        }) = recv_document_frame(&mut healthy, 1).await
        {
            saw_step2 = true;
            break;
        }
    }
    assert!(
        saw_step2,
        "healthy peer must still receive Step2 after malformed Step1 from another member"
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
        assert_eq!(
            tail_len, 1,
            "writer update must persist after two readonly joins"
        );
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

    let mut reader = connect_member(addr, &wiki.session.session_token).await;
    auth_and_join(&mut reader, &routing_key, 92).await;
    let mut writer = connect_member(addr, &wiki.session.session_token).await;
    auth_and_join(&mut writer, &routing_key, 91).await;
    arm_force_primary_apply_fail(wiki.document_id).await;
    writer
        .send(Message::Binary(
            sync_update_frame(&routing_key, &update).into(),
        ))
        .await
        .unwrap();
    assert!(
        wait_for_sync_applied(&mut writer, Duration::from_secs(5)).await,
        "durable commit must not be rejected when primary apply fails"
    );
    let mut reader_saw_broadcast = false;
    for _ in 0..12 {
        if let Some(WireFrame::Document {
            message:
                DocumentMessage::Sync(SyncMessage {
                    step: SyncStep::Update,
                    ..
                }),
            ..
        }) = recv_document_frame(&mut reader, 1).await
        {
            reader_saw_broadcast = true;
            break;
        }
    }
    assert!(
        reader_saw_broadcast,
        "peer must receive durable broadcast after primary apply failure reload"
    );
    disarm_force_primary_apply_fail(wiki.document_id).await;

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
async fn collab_reload_failure_after_commit_preserves_durable_tail() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    let app = fvoci_server::http::router(collab_app_state(&harness.app_url, true).await, None);
    let addr = spawn_server(app).await;
    let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
    let update = sample_hi_update();
    let request_id = Uuid::now_v7();

    let mut writer = connect_member(addr, &wiki.session.session_token).await;
    auth_and_join(&mut writer, &routing_key, 92).await;
    arm_force_primary_apply_fail(wiki.document_id).await;
    arm_force_primary_load_fail(wiki.document_id).await;
    let (reached_rx, proceed_tx) = arm_delivery_read_barrier(wiki.session.session_id);
    writer
        .send(Message::Binary(
            sync_update_frame(&routing_key, &update).into(),
        ))
        .await
        .unwrap();
    writer
        .send(Message::Binary(
            stateless_frame(&routing_key, &format!("persist:{request_id}")).into(),
        ))
        .await
        .unwrap();

    tokio::time::timeout(Duration::from_secs(2), reached_rx)
        .await
        .expect(
            "writer Data delivery auth must reach barrier after durable commit (echo or Applied)",
        )
        .expect("barrier signal");
    proceed_tx.send(()).expect("release delivery read");
    disarm_delivery_read_barrier(wiki.session.session_id);

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut saw_applied = false;
    let mut saw_persisted = false;
    let mut saw_close_1011 = false;
    while tokio::time::Instant::now() < deadline && !saw_close_1011 {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining.min(Duration::from_millis(200)), writer.next()).await {
            Ok(Some(Ok(Message::Close(Some(frame))))) => {
                let code = ws_close_code(&frame);
                assert_eq!(
                    code, 1011,
                    "reload failure CloseFrame {code} ({:?}), expected 1011; reason {:?}",
                    frame.code, frame.reason
                );
                saw_close_1011 = true;
            }
            Ok(Some(Ok(Message::Close(None)))) => {
                panic!("Close without code after reload failure, expected CloseFrame 1011");
            }
            Ok(None) => {
                panic!("bare TCP EOF after reload failure, expected CloseFrame 1011");
            }
            Ok(Some(Err(err))) => {
                panic!("websocket error before CloseFrame 1011: {err}");
            }
            Ok(Some(Ok(Message::Binary(bytes)))) => {
                match fvoci_server::collab::wire::decode(&bytes) {
                    Ok(WireFrame::Document {
                        message: DocumentMessage::SyncStatus { applied: true },
                        ..
                    }) => saw_applied = true,
                    Ok(WireFrame::Document {
                        message: DocumentMessage::Stateless(body),
                        ..
                    }) if body.starts_with("persisted:") => saw_persisted = true,
                    _ => {}
                }
            }
            Ok(Some(Ok(_))) | Err(_) => {}
        }
    }
    assert!(
        saw_applied,
        "durable commit must ack Applied before 1011 when primary reload fails; saw_close_1011={saw_close_1011}"
    );
    assert!(!saw_persisted, "stale primary must not emit persisted ack");
    assert!(
        saw_close_1011,
        "primary reload failure must CloseFrame 1011 after Applied"
    );
    disarm_force_primary_load_fail(wiki.document_id).await;
    disarm_force_primary_apply_fail(wiki.document_id).await;

    let mut recovery = connect_member(addr, &wiki.session.session_token).await;
    auth_and_join(&mut recovery, &routing_key, 93).await;
    recovery
        .send(Message::Binary(
            sync_step1_frame(&routing_key, &[0, 0]).into(),
        ))
        .await
        .unwrap();
    let mut saw_step2 = false;
    for _ in 0..12 {
        if let Some(WireFrame::Document {
            message:
                DocumentMessage::Sync(SyncMessage {
                    step: SyncStep::Step2,
                    ..
                }),
            ..
        }) = recv_document_frame(&mut recovery, 1).await
        {
            saw_step2 = true;
            break;
        }
    }
    assert!(
        saw_step2,
        "fresh join after reload failure must receive Step2 with durable tail"
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
    assert_eq!(load.tail.len(), 1);
    assert_eq!(load.tail[0].payload, update);
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_lifecycle_foreign_leave_does_not_evict_member() {
    run_lifecycle_test(
        "collab_lifecycle_foreign_leave_does_not_evict_member",
        async {
            let harness = TestDb::bootstrap().await;
            let wiki = setup_wiki_doc(&harness).await;
            let hub = CollabHub::new(test_collab_config(4, 200), wiki.session.pool.clone());
            let key = (wiki.session.workspace_id, wiki.document_id);
            let conn_id = hub_join(&hub, &wiki, 1).await.expect("join");
            assert_eq!(
                hub.room_lifecycle_phase(key).await,
                RoomLifecyclePhase::Live
            );

            hub.leave_room(key, Uuid::now_v7()).await;
            assert_eq!(
                hub.room_member_count(key).await,
                1,
                "unknown leave must not remove the actual member from eviction accounting"
            );
            assert_eq!(
                hub.room_lifecycle_phase(key).await,
                RoomLifecyclePhase::Live
            );

            hub.leave_room(key, conn_id).await;
            wait_for_phase(&hub, key, RoomLifecyclePhase::Absent).await;
            hub.shutdown().await;
            harness.cleanup().await;
        },
    )
    .await;
}

#[tokio::test]
async fn collab_lifecycle_shutdown_during_booting_reclaims_slot() {
    run_lifecycle_test(
        "collab_lifecycle_shutdown_during_booting_reclaims_slot",
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
            tokio::time::timeout(Duration::from_secs(5), async {
                while !hub.is_shutting_down() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("shutdown must reach the admission barrier");

            release.send(()).expect("release startup gate");
            let join_result = join_task.await.expect("join task");
            assert!(join_result.is_err(), "join during shutdown must fail");
            shutdown_task.await.expect("shutdown task");
            assert_eq!(hub.available_room_slots(), slots_before);
            assert_eq!(
                hub.room_lifecycle_phase(key).await,
                RoomLifecyclePhase::Absent
            );
            disarm_spawn_room_block(wiki.document_id).await;
            harness.cleanup().await;
        },
    )
    .await;
}

fn awareness_live_frame(routing_key: &str, client_id: u32, clock: u64, user_id: &str) -> Vec<u8> {
    let state = serde_json::json!({
        "user": {"id": user_id, "name": "forged", "color": "#000000"}
    });
    let payload = encode_awareness(&[AwarenessUpdate {
        client_id,
        clock,
        state: Some(serde_json::to_vec(&state).unwrap()),
    }]);
    encode(&WireFrame::Document {
        routing_key: routing_key.to_string(),
        room: CollabRoomName::parse(routing_key),
        message: DocumentMessage::Awareness(payload),
    })
    .expect("awareness frame")
}

fn ws_close_code(frame: &tokio_tungstenite::tungstenite::protocol::CloseFrame) -> u16 {
    u16::from(frame.code)
}

fn binary_is_sync_update(bytes: &[u8]) -> bool {
    matches!(
        fvoci_server::collab::wire::decode(bytes),
        Ok(WireFrame::Document {
            message: DocumentMessage::Sync(SyncMessage {
                step: SyncStep::Update,
                ..
            }),
            ..
        })
    )
}

async fn wait_for_ws_close(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    within: Duration,
) -> bool {
    let deadline = tokio::time::Instant::now() + within;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining.min(Duration::from_millis(100)), ws.next()).await {
            Ok(Some(Ok(Message::Close(_)))) => return true,
            Ok(Some(Ok(Message::Binary(_)))) => {}
            Ok(Some(Ok(_))) => {}
            Ok(None) => return true,
            _ => {}
        }
    }
    false
}

/// Requires an explicit CloseFrame with `expected` code. Bare TCP EOF or a
/// Close without a code fails; a Sync Update also fails when
/// `reject_sync_update` is set.
async fn wait_for_ws_close_code(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    expected: u16,
    within: Duration,
    reject_sync_update: bool,
) {
    let deadline = tokio::time::Instant::now() + within;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining.min(Duration::from_millis(100)), ws.next()).await {
            Ok(Some(Ok(Message::Close(Some(frame))))) => {
                let code = ws_close_code(&frame);
                assert_eq!(
                    code, expected,
                    "CloseFrame code {code} ({:?}), expected {expected}; reason {:?}",
                    frame.code, frame.reason
                );
                return;
            }
            Ok(Some(Ok(Message::Close(None)))) => {
                panic!("Close without code, expected CloseFrame {expected}");
            }
            Ok(Some(Ok(Message::Binary(bytes)))) => {
                assert!(
                    !(reject_sync_update && binary_is_sync_update(&bytes)),
                    "Sync Update must not precede CloseFrame {expected}"
                );
            }
            Ok(Some(Ok(_))) => {}
            Ok(None) => {
                panic!("bare TCP EOF without CloseFrame, expected close code {expected}");
            }
            Ok(Some(Err(err))) => {
                panic!("websocket error before CloseFrame {expected}: {err}");
            }
            Err(_) => {}
        }
    }
    panic!("did not receive CloseFrame {expected} within {within:?}");
}

#[tokio::test]
async fn collab_archived_readonly_scope_allows_sync_refuses_write() {
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
    ws.send(Message::Binary(auth_token_frame(&routing_key, 90).into()))
        .await
        .unwrap();
    let frame = recv_document_frame(&mut ws, 4)
        .await
        .expect("auth response");
    match frame {
        WireFrame::Document {
            message: DocumentMessage::Auth(AuthMessage::Authenticated { scope }),
            ..
        } => assert_eq!(scope, "readonly"),
        other => panic!("expected readonly auth, got {other:?}"),
    }

    ws.send(Message::Binary(
        sync_step1_frame(&routing_key, &[0, 0]).into(),
    ))
    .await
    .unwrap();
    let mut saw_step2 = false;
    for _ in 0..8 {
        if let Some(WireFrame::Document {
            message:
                DocumentMessage::Sync(SyncMessage {
                    step: SyncStep::Step2,
                    ..
                }),
            ..
        }) = recv_document_frame(&mut ws, 1).await
        {
            saw_step2 = true;
            break;
        }
    }
    assert!(saw_step2, "readonly join must receive initial sync");

    ws.send(Message::Binary(
        sync_update_frame(&routing_key, &sample_hi_update()).into(),
    ))
    .await
    .unwrap();
    let mut saw_rejected = false;
    for _ in 0..6 {
        if let Some(WireFrame::Document {
            message: DocumentMessage::SyncStatus { applied: false },
            ..
        }) = recv_document_frame(&mut ws, 1).await
        {
            saw_rejected = true;
            break;
        }
    }
    assert!(saw_rejected, "readonly connection must refuse writes");
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_revoked_session_closes_without_post_revoke_broadcast() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    let cfg = test_collab_config_with_revoke(4, 30_000, 30_000);
    let app = fvoci_server::http::router(
        collab_app_state_with_config(&harness.app_url, cfg).await,
        None,
    );
    let addr = spawn_server(app).await;
    let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);

    let reader_token = new_token();
    let reader_session_id = Uuid::now_v7();
    let reader_expires = Utc::now() + ChronoDuration::days(30);
    let mut reader_tx = wiki.session.pool.begin().await.unwrap();
    fvoci_server::db::identity::create_session(
        &mut reader_tx,
        reader_session_id,
        wiki.session.user_id,
        &reader_token.hash,
        reader_expires,
    )
    .await
    .unwrap();
    reader_tx.commit().await.unwrap();
    let observer_token = add_session_for_user(&wiki.session.pool, wiki.session.user_id).await;

    let mut writer = connect_member(addr, &wiki.session.session_token).await;
    auth_and_join(&mut writer, &routing_key, 101).await;
    let mut observer = connect_member(addr, &observer_token.token).await;
    auth_and_join(&mut observer, &routing_key, 103).await;
    let mut reader = connect_member(addr, &reader_token.token).await;
    auth_and_join(&mut reader, &routing_key, 102).await;

    revoke_session(
        &wiki.session.pool,
        &reader_token.hash,
        Some(wiki.session.user_id),
    )
    .await
    .expect("revoke");

    writer
        .send(Message::Binary(
            sync_update_frame(&routing_key, &sample_hi_update()).into(),
        ))
        .await
        .unwrap();
    assert!(
        wait_for_sync_applied(&mut writer, Duration::from_secs(5)).await,
        "writer should still apply after peer revocation"
    );

    let mut observer_saw_update = false;
    for _ in 0..12 {
        if let Some(WireFrame::Document {
            message:
                DocumentMessage::Sync(SyncMessage {
                    step: SyncStep::Update,
                    ..
                }),
            ..
        }) = recv_document_frame(&mut observer, 1).await
        {
            observer_saw_update = true;
            break;
        }
    }
    assert!(
        observer_saw_update,
        "healthy peer must receive the live writer's update"
    );
    wait_for_ws_close_code(&mut reader, 1008, Duration::from_secs(2), true).await;
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_outbound_queue_saturation_closes_slow_peer() {
    run_lifecycle_test("collab_outbound_queue_saturation", async {
        let harness = TestDb::bootstrap().await;
        let wiki = setup_wiki_doc(&harness).await;
        let hub = CollabHub::new(test_collab_config(4, 30_000), wiki.session.pool.clone());
        let key = (wiki.session.workspace_id, wiki.document_id);
        let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
        let user_id = wiki.session.user_id.to_string();

        let (slow_tx, _slow_rx) = mpsc::channel(2);
        let (cancel_tx, cancel_rx) = watch::channel(None);
        let slow_id = Uuid::now_v7();
        hub.join_room(
            key,
            RoomJoin {
                conn: AuthenticatedConnection {
                    conn_id: slow_id,
                    session: CollabSession {
                        session_id: wiki.session.session_id,
                        user_id: wiki.session.user_id,
                        given_name: "Slow".into(),
                        family_name: None,
                        locale: "en".into(),
                    },
                    client_id: 301,
                    read_only: false,
                    routing_key: routing_key.clone(),
                },
                events: slow_tx,
                cancel: Some(cancel_tx),
            },
        )
        .await
        .expect("slow join");
        hub.send_frame(
            key,
            slow_id,
            awareness_live_frame(&routing_key, 301, 1, &user_id),
        )
        .await;

        let (writer_tx, mut writer_rx) = mpsc::channel(64);
        let writer_id = Uuid::now_v7();
        hub.join_room(
            key,
            RoomJoin {
                conn: AuthenticatedConnection {
                    conn_id: writer_id,
                    session: CollabSession {
                        session_id: wiki.session.session_id,
                        user_id: wiki.session.user_id,
                        given_name: "Writer".into(),
                        family_name: None,
                        locale: "en".into(),
                    },
                    client_id: 302,
                    read_only: false,
                    routing_key: routing_key.clone(),
                },
                events: writer_tx,
                cancel: None,
            },
        )
        .await
        .expect("writer join");
        while writer_rx.try_recv().is_ok() {}

        let mut slow_cancelled: bool = false;
        for clock in 1u64..=32 {
            if cancel_rx.borrow().is_some() {
                slow_cancelled = true;
                break;
            }
            hub.send_frame(
                key,
                writer_id,
                awareness_live_frame(&routing_key, 302, clock, &user_id),
            )
            .await;
            tokio::task::yield_now().await;
        }
        if !slow_cancelled {
            slow_cancelled = wait_for_cancel_signal(cancel_rx, Duration::from_secs(3)).await;
        }
        assert!(
            slow_cancelled,
            "saturated outbound queue must signal independent cancel within deadline"
        );

        let mut writer_saw_tombstone = false;
        let tombstone_deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while tokio::time::Instant::now() < tombstone_deadline {
            match writer_rx.try_recv() {
                Ok(RoomClientEvent::Outbound(frame)) => {
                    if let Ok(WireFrame::Document {
                        message: DocumentMessage::Awareness(payload),
                        ..
                    }) = fvoci_server::collab::wire::decode(&frame.bytes)
                    {
                        let updates = decode_awareness(&payload).expect("awareness");
                        if updates
                            .iter()
                            .any(|u| u.client_id == 301 && u.state.is_none())
                        {
                            writer_saw_tombstone = true;
                            break;
                        }
                    }
                }
                Ok(_) => {}
                Err(mpsc::error::TryRecvError::Empty) => tokio::task::yield_now().await,
                Err(mpsc::error::TryRecvError::Disconnected) => break,
            }
        }
        assert!(
            writer_saw_tombstone,
            "surviving writer must receive a generation-aware tombstone for the evicted peer"
        );

        hub.send_frame(
            key,
            writer_id,
            awareness_live_frame(&routing_key, 302, 99, &user_id),
        )
        .await;
        let writer_still_live = tokio::time::timeout(Duration::from_secs(1), async {
            while let Ok(event) = writer_rx.try_recv() {
                if matches!(event, RoomClientEvent::Outbound(_)) {
                    return true;
                }
            }
            while let Some(event) = writer_rx.recv().await {
                if matches!(event, RoomClientEvent::Outbound(_)) {
                    return true;
                }
            }
            false
        })
        .await
        .unwrap_or(false);
        assert!(
            writer_still_live,
            "evicting the slow peer must not break the writer connection"
        );

        let (late_tx, mut late_rx) = mpsc::channel(8);
        hub.join_room(
            key,
            RoomJoin {
                conn: AuthenticatedConnection {
                    conn_id: Uuid::now_v7(),
                    session: CollabSession {
                        session_id: wiki.session.session_id,
                        user_id: wiki.session.user_id,
                        given_name: "Late".into(),
                        family_name: None,
                        locale: "en".into(),
                    },
                    client_id: 303,
                    read_only: false,
                    routing_key: routing_key.clone(),
                },
                events: late_tx,
                cancel: None,
            },
        )
        .await
        .expect("late join");
        let mut late_saw_ghost = false;
        let late_deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while tokio::time::Instant::now() < late_deadline {
            match late_rx.try_recv() {
                Ok(RoomClientEvent::Outbound(frame)) => {
                    if let Ok(WireFrame::Document {
                        message: DocumentMessage::Awareness(payload),
                        ..
                    }) = fvoci_server::collab::wire::decode(&frame.bytes)
                    {
                        let updates = decode_awareness(&payload).expect("awareness");
                        if updates
                            .iter()
                            .any(|u| u.client_id == 301 && u.state.is_some())
                        {
                            late_saw_ghost = true;
                            break;
                        }
                    }
                }
                Ok(RoomClientEvent::Close { .. }) => break,
                Err(mpsc::error::TryRecvError::Empty) => tokio::task::yield_now().await,
                Err(mpsc::error::TryRecvError::Disconnected) => break,
            }
        }
        assert!(
            !late_saw_ghost,
            "late joiner must not receive ghost presence for the evicted peer"
        );
        hub.shutdown().await;
        harness.cleanup().await;
    })
    .await;
}

#[tokio::test]
async fn collab_awareness_generation_takeover_old_leave_cannot_clear() {
    run_lifecycle_test("collab_awareness_generation_takeover", async {
        let harness = TestDb::bootstrap().await;
        let wiki = setup_wiki_doc(&harness).await;
        let hub = CollabHub::new(test_collab_config(4, 30_000), wiki.session.pool.clone());
        let key = (wiki.session.workspace_id, wiki.document_id);
        let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
        let user_id = wiki.session.user_id.to_string();

        let (old_tx, mut old_rx) = mpsc::channel(8);
        let old_id = Uuid::now_v7();
        hub.join_room(
            key,
            RoomJoin {
                conn: AuthenticatedConnection {
                    conn_id: old_id,
                    session: CollabSession {
                        session_id: wiki.session.session_id,
                        user_id: wiki.session.user_id,
                        given_name: "Old".into(),
                        family_name: None,
                        locale: "en".into(),
                    },
                    client_id: 201,
                    read_only: false,
                    routing_key: routing_key.clone(),
                },
                events: old_tx,
                cancel: None,
            },
        )
        .await
        .expect("old join");
        hub.send_frame(
            key,
            old_id,
            awareness_live_frame(&routing_key, 201, 1, &user_id),
        )
        .await;
        while old_rx.try_recv().is_ok() {}

        let (new_tx, mut new_rx) = mpsc::channel(8);
        let new_id = Uuid::now_v7();
        hub.join_room(
            key,
            RoomJoin {
                conn: AuthenticatedConnection {
                    conn_id: new_id,
                    session: CollabSession {
                        session_id: wiki.session.session_id,
                        user_id: wiki.session.user_id,
                        given_name: "New".into(),
                        family_name: None,
                        locale: "en".into(),
                    },
                    client_id: 201,
                    read_only: false,
                    routing_key: routing_key.clone(),
                },
                events: new_tx,
                cancel: None,
            },
        )
        .await
        .expect("new join");
        hub.send_frame(
            key,
            new_id,
            awareness_live_frame(&routing_key, 201, 2, &user_id),
        )
        .await;
        while new_rx.try_recv().is_ok() {}

        hub.leave_room(key, old_id).await;
        let mut saw_tombstone = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while tokio::time::Instant::now() < deadline {
            if let Ok(RoomClientEvent::Outbound(frame)) = new_rx.try_recv() {
                let bytes = frame.bytes;
                if let Ok(WireFrame::Document {
                    message: DocumentMessage::Awareness(payload),
                    ..
                }) = fvoci_server::collab::wire::decode(&bytes)
                {
                    let updates = decode_awareness(&payload).expect("awareness");
                    if updates
                        .iter()
                        .any(|u| u.client_id == 201 && u.state.is_none())
                    {
                        saw_tombstone = true;
                        break;
                    }
                }
            }
            tokio::task::yield_now().await;
        }
        assert!(
            !saw_tombstone,
            "stale connection leave must not remove newer generation claim"
        );
        hub.shutdown().await;
        harness.cleanup().await;
    })
    .await;
}

#[tokio::test]
async fn collab_client_id_live_ownership_blocks_other_user() {
    run_lifecycle_test("collab_client_id_live_ownership", async {
        let harness = TestDb::bootstrap().await;
        let wiki = setup_wiki_doc(&harness).await;
        let mut cfg = test_collab_config(4, 30_000);
        cfg.client_id_ttl_ms = 5_000;
        let hub = CollabHub::new(cfg, wiki.session.pool.clone());
        let key = (wiki.session.workspace_id, wiki.document_id);
        let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
        let peer = setup_second_member(&harness, &wiki).await;

        let (owner_tx, mut owner_rx) = mpsc::channel(8);
        let owner_id = Uuid::now_v7();
        hub.join_room(
            key,
            RoomJoin {
                conn: AuthenticatedConnection {
                    conn_id: owner_id,
                    session: collab_session_from(&wiki.session, "Owner"),
                    client_id: 201,
                    read_only: false,
                    routing_key: routing_key.clone(),
                },
                events: owner_tx,
                cancel: None,
            },
        )
        .await
        .expect("owner join");
        while owner_rx.try_recv().is_ok() {}

        let (peer_tx, _peer_rx) = mpsc::channel(8);
        let live_conflict = hub
            .join_room(
                key,
                RoomJoin {
                    conn: AuthenticatedConnection {
                        conn_id: Uuid::now_v7(),
                        session: collab_session_from(&peer, "Peer"),
                        client_id: 201,
                        read_only: false,
                        routing_key: routing_key.clone(),
                    },
                    events: peer_tx,
                    cancel: None,
                },
            )
            .await;
        assert!(
            matches!(live_conflict, Err(JoinError::AdmissionDenied)),
            "a different user must not take a live connection's client id"
        );

        hub.leave_room(key, owner_id).await;
        let (peer_tx2, _peer_rx2) = mpsc::channel(8);
        let after_leave = hub
            .join_room(
                key,
                RoomJoin {
                    conn: AuthenticatedConnection {
                        conn_id: Uuid::now_v7(),
                        session: collab_session_from(&peer, "Peer"),
                        client_id: 201,
                        read_only: false,
                        routing_key: routing_key.clone(),
                    },
                    events: peer_tx2,
                    cancel: None,
                },
            )
            .await;
        assert!(
            matches!(after_leave, Err(JoinError::AdmissionDenied)),
            "a different user must not reuse a client id after the owner leaves"
        );

        let (same_tx, _same_rx) = mpsc::channel(8);
        hub.join_room(
            key,
            RoomJoin {
                conn: AuthenticatedConnection {
                    conn_id: Uuid::now_v7(),
                    session: collab_session_from(&wiki.session, "Owner"),
                    client_id: 201,
                    read_only: false,
                    routing_key,
                },
                events: same_tx,
                cancel: None,
            },
        )
        .await
        .expect("same user generation takeover must still be allowed");
        hub.shutdown().await;
        harness.cleanup().await;
    })
    .await;
}

#[tokio::test]
async fn collab_late_join_receives_peer_awareness_snapshot() {
    run_lifecycle_test("collab_late_join_awareness", async {
        let harness = TestDb::bootstrap().await;
        let wiki = setup_wiki_doc(&harness).await;
        let hub = CollabHub::new(test_collab_config(4, 30_000), wiki.session.pool.clone());
        let key = (wiki.session.workspace_id, wiki.document_id);
        let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
        let user_id = wiki.session.user_id.to_string();

        let (first_tx, mut first_rx) = mpsc::channel(8);
        let first_id = Uuid::now_v7();
        hub.join_room(
            key,
            RoomJoin {
                conn: AuthenticatedConnection {
                    conn_id: first_id,
                    session: CollabSession {
                        session_id: wiki.session.session_id,
                        user_id: wiki.session.user_id,
                        given_name: "First".into(),
                        family_name: None,
                        locale: "en".into(),
                    },
                    client_id: 101,
                    read_only: false,
                    routing_key: routing_key.clone(),
                },
                events: first_tx,
                cancel: None,
            },
        )
        .await
        .expect("first join");
        hub.send_frame(
            key,
            first_id,
            awareness_live_frame(&routing_key, 101, 1, &user_id),
        )
        .await;
        while first_rx.try_recv().is_ok() {}

        let (late_tx, mut late_rx) = mpsc::channel(8);
        hub.join_room(
            key,
            RoomJoin {
                conn: AuthenticatedConnection {
                    conn_id: Uuid::now_v7(),
                    session: CollabSession {
                        session_id: wiki.session.session_id,
                        user_id: wiki.session.user_id,
                        given_name: "Late".into(),
                        family_name: None,
                        locale: "en".into(),
                    },
                    client_id: 102,
                    read_only: false,
                    routing_key: routing_key.clone(),
                },
                events: late_tx,
                cancel: None,
            },
        )
        .await
        .expect("late join");

        let mut saw_peer = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while tokio::time::Instant::now() < deadline {
            match late_rx.try_recv() {
                Ok(RoomClientEvent::Outbound(frame)) => {
                    if let Ok(WireFrame::Document {
                        message: DocumentMessage::Awareness(payload),
                        ..
                    }) = fvoci_server::collab::wire::decode(&frame.bytes)
                    {
                        let updates = decode_awareness(&payload).expect("awareness");
                        if updates
                            .iter()
                            .any(|u| u.client_id == 101 && u.state.is_some())
                        {
                            saw_peer = true;
                            break;
                        }
                    }
                }
                Ok(RoomClientEvent::Close { .. }) => break,
                Err(mpsc::error::TryRecvError::Empty) => {
                    tokio::task::yield_now().await;
                }
                Err(mpsc::error::TryRecvError::Disconnected) => break,
            }
        }
        assert!(
            saw_peer,
            "late joiner must receive existing peer awareness snapshot on join"
        );
        hub.shutdown().await;
        harness.cleanup().await;
    })
    .await;
}

#[tokio::test]
async fn collab_idle_socket_closes_without_auth() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    let mut cfg = test_collab_config(4, 30_000);
    cfg.auth_wait_ms = 400;
    let app = fvoci_server::http::router(
        collab_app_state_with_config(&harness.app_url, cfg).await,
        None,
    );
    let addr = spawn_server(app).await;
    let mut ws = connect_member(addr, &wiki.session.session_token).await;
    assert!(
        wait_for_ws_close(&mut ws, Duration::from_millis(1_500)).await,
        "idle unauthenticated socket must close after auth_wait deadline"
    );
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_socket_cap_rejects_excess_and_releases() {
    run_lifecycle_test("collab_socket_cap", async {
        let harness = TestDb::bootstrap().await;
        let wiki = setup_wiki_doc(&harness).await;
        let mut cfg = test_collab_config(4, 30_000);
        cfg.max_collab_sockets = 1;
        let hub = CollabHub::new(cfg.clone(), wiki.session.pool.clone());
        assert_eq!(hub.available_collab_sockets(), 1);
        let held = hub
            .try_acquire_socket(wiki.session.session_id)
            .expect("first permit");
        assert_eq!(hub.available_collab_sockets(), 0);
        assert!(hub.try_acquire_socket(wiki.session.session_id).is_none());
        drop(held);
        assert_eq!(hub.available_collab_sockets(), 1);

        let app = fvoci_server::http::router(
            collab_app_state_with_config(&harness.app_url, cfg).await,
            None,
        );
        let addr = spawn_server(app).await;
        let mut ws1 = connect_member(addr, &wiki.session.session_token).await;
        assert!(
            try_connect_member(addr, &wiki.session.session_token)
                .await
                .is_err(),
            "socket cap must reject excess upgrades"
        );
        ws1.close(None).await.unwrap();
        assert!(
            try_connect_member(addr, &wiki.session.session_token)
                .await
                .is_ok(),
            "released socket permit must allow a new upgrade"
        );
        hub.shutdown().await;
        harness.cleanup().await;
    })
    .await;
}

#[tokio::test]
async fn collab_session_socket_cap_rejects_same_session_allows_other() {
    run_lifecycle_test("collab_session_socket_cap", async {
        let harness = TestDb::bootstrap().await;
        let wiki = setup_wiki_doc(&harness).await;
        let mut cfg = test_collab_config(4, 30_000);
        cfg.max_collab_sockets = 4;
        cfg.max_collab_sockets_per_session = 1;
        let other = add_session_for_user(&wiki.session.pool, wiki.session.user_id).await;
        let app = fvoci_server::http::router(
            collab_app_state_with_config(&harness.app_url, cfg).await,
            None,
        );
        let addr = spawn_server(app).await;
        let mut ws1 = connect_member(addr, &wiki.session.session_token).await;
        assert!(
            try_connect_member(addr, &wiki.session.session_token)
                .await
                .is_err(),
            "per-session socket cap must reject a second socket for the same session"
        );
        assert!(
            try_connect_member(addr, &other.token).await.is_ok(),
            "a different session must still obtain a global socket"
        );
        ws1.close(None).await.unwrap();
        harness.cleanup().await;
    })
    .await;
}

#[tokio::test]
async fn collab_pre_auth_outbound_is_bounded() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    let mut cfg = test_collab_config(4, 30_000);
    cfg.max_pre_auth_outbound_frames = 1;
    cfg.auth_wait_ms = 5_000;
    let app = fvoci_server::http::router(
        collab_app_state_with_config(&harness.app_url, cfg).await,
        None,
    );
    let addr = spawn_server(app).await;
    let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
    let mut ws = connect_member(addr, &wiki.session.session_token).await;
    let invalid_auth = encode(&WireFrame::Document {
        routing_key: routing_key.clone(),
        room: CollabRoomName::parse(&routing_key),
        message: DocumentMessage::Auth(AuthMessage::Token {
            token: "not-a-client-id".into(),
            provider_version: Some("4.6.0".into()),
        }),
    })
    .expect("encode invalid auth");
    ws.send(Message::Binary(invalid_auth.clone().into()))
        .await
        .unwrap();
    let first = recv_document_frame(&mut ws, 2).await;
    assert!(
        matches!(
            first,
            Some(WireFrame::Document {
                message: DocumentMessage::Auth(AuthMessage::PermissionDenied { .. }),
                ..
            })
        ),
        "invalid auth must yield a bounded pre-auth denial"
    );
    ws.send(Message::Binary(invalid_auth.into())).await.unwrap();
    wait_for_ws_close_code(&mut ws, 1013, Duration::from_secs(2), false).await;
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_pre_auth_exhaustion_does_not_swallow_authenticated() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    let mut cfg = test_collab_config(4, 30_000);
    cfg.max_pre_auth_outbound_frames = 1;
    cfg.auth_wait_ms = 5_000;
    let app = fvoci_server::http::router(
        collab_app_state_with_config(&harness.app_url, cfg).await,
        None,
    );
    let addr = spawn_server(app).await;
    let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
    let mut ws = connect_member(addr, &wiki.session.session_token).await;
    let invalid_auth = encode(&WireFrame::Document {
        routing_key: routing_key.clone(),
        room: CollabRoomName::parse(&routing_key),
        message: DocumentMessage::Auth(AuthMessage::Token {
            token: "not-a-client-id".into(),
            provider_version: Some("4.6.0".into()),
        }),
    })
    .expect("encode invalid auth");
    ws.send(Message::Binary(invalid_auth.into())).await.unwrap();
    let first = recv_document_frame(&mut ws, 2).await;
    assert!(
        matches!(
            first,
            Some(WireFrame::Document {
                message: DocumentMessage::Auth(AuthMessage::PermissionDenied { .. }),
                ..
            })
        ),
        "first invalid auth still receives a denial"
    );
    ws.send(Message::Binary(auth_token_frame(&routing_key, 77).into()))
        .await
        .unwrap();
    let mut saw_authenticated = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let mut closed_1013 = false;
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(100), ws.next()).await {
            Ok(Some(Ok(Message::Binary(bytes)))) => {
                if let Ok(WireFrame::Document {
                    message: DocumentMessage::Auth(AuthMessage::Authenticated { .. }),
                    ..
                }) = fvoci_server::collab::wire::decode(&bytes)
                {
                    saw_authenticated = true;
                    break;
                }
            }
            Ok(Some(Ok(Message::Close(Some(frame))))) => {
                assert_eq!(
                    ws_close_code(&frame),
                    1013,
                    "pre-auth exhaustion CloseFrame {:?}, expected 1013",
                    frame.code
                );
                closed_1013 = true;
                break;
            }
            Ok(Some(Ok(Message::Close(None)))) | Ok(None) => {
                panic!("bare TCP EOF without CloseFrame, expected close code 1013");
            }
            Ok(Some(Err(err))) => panic!("websocket error: {err}"),
            _ => {}
        }
    }
    assert!(
        !saw_authenticated,
        "Authenticated must not be delivered after pre-auth allowance is exhausted"
    );
    assert!(
        closed_1013,
        "socket must close with explicit CloseFrame 1013 instead of joining silently"
    );
    harness.cleanup().await;
}

enum AdmissionParity {
    Allowed { read_only: bool },
    Denied,
}

async fn assert_admission_parity(
    pool: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
    expect: AdmissionParity,
    label: &str,
) {
    let locking =
        resolve_collab_admission(pool, workspace_id, user_id, session_id, document_id).await;
    let delivery =
        check_delivery_admission(pool, workspace_id, user_id, session_id, document_id).await;
    match (&locking, &delivery, expect) {
        (
            Ok(Ok(admission)),
            Ok(DeliveryAdmission::Allowed { read_only }),
            AdmissionParity::Allowed {
                read_only: expect_ro,
            },
        ) => {
            assert_eq!(
                admission.read_only, *read_only,
                "{label}: locking and delivery read_only disagree"
            );
            assert_eq!(
                admission.read_only, expect_ro,
                "{label}: unexpected read_only"
            );
        }
        (Ok(Err(_)), Ok(DeliveryAdmission::Denied), AdmissionParity::Denied) => {}
        (Ok(Ok(admission)), Ok(DeliveryAdmission::Allowed { .. }), AdmissionParity::Denied) => {
            panic!(
                "{label}: expected Denied (do not widen grants); got Allowed read_only={}",
                admission.read_only
            );
        }
        (_, _, AdmissionParity::Allowed { read_only }) => panic!(
            "{label}: expected Allowed read_only={read_only}; locking={locking:?} delivery={delivery:?}"
        ),
        _ => panic!(
            "{label}: locking and delivery must agree; locking={locking:?} delivery={delivery:?}"
        ),
    }
}

#[tokio::test]
async fn collab_delivery_admission_parity_with_locking_join() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    let pool = &wiki.session.pool;
    let ws = wiki.session.workspace_id;
    let user = wiki.session.user_id;
    let session = wiki.session.session_id;
    let doc = wiki.document_id;

    assert_admission_parity(
        pool,
        ws,
        user,
        session,
        doc,
        AdmissionParity::Allowed { read_only: false },
        "live wiki member",
    )
    .await;

    revoke_session(
        pool,
        &fvoci_server::auth::token::hash_token(&wiki.session.session_token),
        Some(user),
    )
    .await
    .expect("revoke");
    assert_admission_parity(
        pool,
        ws,
        user,
        session,
        doc,
        AdmissionParity::Denied,
        "revoked session",
    )
    .await;

    let token = add_session_for_user(pool, user).await;
    let sid = sqlx::query_scalar::<_, Uuid>("SELECT id FROM fvoci.app_session_by_token_hash($1)")
        .bind(&token.hash)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_admission_parity(
        pool,
        ws,
        user,
        sid,
        doc,
        AdmissionParity::Allowed { read_only: false },
        "fresh session",
    )
    .await;

    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&harness.admin_url)
        .await
        .unwrap();

    sqlx::query("UPDATE fvoci.sessions SET expires_at = now() - interval '1 hour' WHERE id = $1")
        .bind(sid)
        .execute(&admin)
        .await
        .unwrap();
    assert_admission_parity(
        pool,
        ws,
        user,
        sid,
        doc,
        AdmissionParity::Denied,
        "expired session",
    )
    .await;
    sqlx::query("UPDATE fvoci.sessions SET expires_at = now() + interval '30 days' WHERE id = $1")
        .bind(sid)
        .execute(&admin)
        .await
        .unwrap();
    assert_admission_parity(
        pool,
        ws,
        user,
        sid,
        doc,
        AdmissionParity::Allowed { read_only: false },
        "restored session expiry",
    )
    .await;

    sqlx::query("UPDATE fvoci.users SET suspended_at = now() WHERE id = $1")
        .bind(user)
        .execute(&admin)
        .await
        .unwrap();
    assert_admission_parity(
        pool,
        ws,
        user,
        sid,
        doc,
        AdmissionParity::Denied,
        "suspended user",
    )
    .await;
    sqlx::query("UPDATE fvoci.users SET suspended_at = NULL WHERE id = $1")
        .bind(user)
        .execute(&admin)
        .await
        .unwrap();
    assert_admission_parity(
        pool,
        ws,
        user,
        sid,
        doc,
        AdmissionParity::Allowed { read_only: false },
        "restored suspension",
    )
    .await;

    sqlx::query("UPDATE fvoci.users SET deleted_at = now() WHERE id = $1")
        .bind(user)
        .execute(&admin)
        .await
        .unwrap();
    assert_admission_parity(
        pool,
        ws,
        user,
        sid,
        doc,
        AdmissionParity::Denied,
        "deleted user",
    )
    .await;
    sqlx::query("UPDATE fvoci.users SET deleted_at = NULL WHERE id = $1")
        .bind(user)
        .execute(&admin)
        .await
        .unwrap();
    assert_admission_parity(
        pool,
        ws,
        user,
        sid,
        doc,
        AdmissionParity::Allowed { read_only: false },
        "restored deleted user",
    )
    .await;

    sqlx::query(
        "UPDATE fvoci.memberships SET role = 'guest' WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(ws)
    .bind(user)
    .execute(&admin)
    .await
    .unwrap();
    assert_admission_parity(
        pool,
        ws,
        user,
        sid,
        doc,
        AdmissionParity::Denied,
        "forbidden guest membership",
    )
    .await;
    sqlx::query(
        "UPDATE fvoci.memberships SET role = 'owner' WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(ws)
    .bind(user)
    .execute(&admin)
    .await
    .unwrap();
    sqlx::query("DELETE FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2")
        .bind(ws)
        .bind(user)
        .execute(&admin)
        .await
        .unwrap();
    assert_admission_parity(
        pool,
        ws,
        user,
        sid,
        doc,
        AdmissionParity::Denied,
        "removed membership",
    )
    .await;
    sqlx::query(
        "INSERT INTO fvoci.memberships (workspace_id, user_id, role) VALUES ($1, $2, 'owner')",
    )
    .bind(ws)
    .bind(user)
    .execute(&admin)
    .await
    .unwrap();
    assert_admission_parity(
        pool,
        ws,
        user,
        sid,
        doc,
        AdmissionParity::Allowed { read_only: false },
        "restored membership",
    )
    .await;

    sqlx::query("UPDATE fvoci.workspaces SET deleted_at = now() WHERE id = $1")
        .bind(ws)
        .execute(&admin)
        .await
        .unwrap();
    assert_admission_parity(
        pool,
        ws,
        user,
        sid,
        doc,
        AdmissionParity::Denied,
        "deleted workspace",
    )
    .await;
    sqlx::query("UPDATE fvoci.workspaces SET deleted_at = NULL WHERE id = $1")
        .bind(ws)
        .execute(&admin)
        .await
        .unwrap();
    assert_admission_parity(
        pool,
        ws,
        user,
        sid,
        doc,
        AdmissionParity::Allowed { read_only: false },
        "restored workspace",
    )
    .await;

    let project_doc = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO fvoci.documents (
            id, workspace_id, title, path, sort_key, project_id, number, status,
            schema_version, content_json, created_by
        ) VALUES (
            $1, $2, 'Project collab lock', $3, 'V', $4, 1, 'draft', 2, $5, $6
        )
        "#,
    )
    .bind(project_doc)
    .bind(ws)
    .bind(project_doc.simple().to_string())
    .bind(Uuid::now_v7())
    .bind(empty_document_json())
    .bind(user)
    .execute(&admin)
    .await
    .unwrap();
    assert_admission_parity(
        pool,
        ws,
        user,
        sid,
        project_doc,
        AdmissionParity::Denied,
        "project document current locking contract",
    )
    .await;
    assert_admission_parity(
        pool,
        ws,
        user,
        sid,
        doc,
        AdmissionParity::Allowed { read_only: false },
        "wiki remains Allowed while project doc is Denied",
    )
    .await;

    sqlx::query("UPDATE fvoci.documents SET status = 'archived' WHERE id = $1")
        .bind(doc)
        .execute(&admin)
        .await
        .unwrap();
    assert_admission_parity(
        pool,
        ws,
        user,
        sid,
        doc,
        AdmissionParity::Allowed { read_only: true },
        "archived wiki",
    )
    .await;
    sqlx::query(
        "UPDATE fvoci.documents SET status = 'published', deleted_at = now() WHERE id = $1",
    )
    .bind(doc)
    .execute(&admin)
    .await
    .unwrap();
    assert_admission_parity(
        pool,
        ws,
        user,
        sid,
        doc,
        AdmissionParity::Denied,
        "soft-deleted wiki",
    )
    .await;
    admin.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_delivery_read_is_nonblocking_while_session_row_locked() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    let mut lock_tx = wiki.session.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM fvoci.sessions WHERE id = $1 FOR UPDATE")
        .bind(wiki.session.session_id)
        .fetch_one(&mut *lock_tx)
        .await
        .unwrap();
    let started = std::time::Instant::now();
    let delivery = tokio::time::timeout(
        Duration::from_millis(500),
        check_delivery_admission(
            &wiki.session.pool,
            wiki.session.workspace_id,
            wiki.session.user_id,
            wiki.session.session_id,
            wiki.document_id,
        ),
    )
    .await
    .expect("delivery read must not wait on FOR UPDATE")
    .expect("delivery query");
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "nonlocking delivery read must return while the session row is locked"
    );
    assert!(matches!(
        delivery,
        DeliveryAdmission::Allowed { read_only: false }
    ));
    drop(lock_tx);
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_delivery_read_failure_closes_1011_without_data_frame() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    let app = fvoci_server::http::router(collab_app_state(&harness.app_url, true).await, None);
    let addr = spawn_server(app).await;
    let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
    let mut writer = connect_member(addr, &wiki.session.session_token).await;
    auth_and_join(&mut writer, &routing_key, 401).await;
    let reader_token = add_session_for_user(&wiki.session.pool, wiki.session.user_id).await;
    let mut reader = connect_member(addr, &reader_token.token).await;
    auth_and_join(&mut reader, &routing_key, 402).await;
    arm_force_delivery_read_fail(wiki.document_id);
    writer
        .send(Message::Binary(
            sync_update_frame(&routing_key, &sample_hi_update()).into(),
        ))
        .await
        .unwrap();
    wait_for_ws_close_code(&mut reader, 1011, Duration::from_secs(2), true).await;
    disarm_force_delivery_read_fail(wiki.document_id);
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_control_frames_skip_delivery_admission_read() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    reset_delivery_read_count(wiki.document_id);
    let mut cfg = test_collab_config(4, 30_000);
    cfg.max_pre_auth_outbound_frames = 1;
    let app = fvoci_server::http::router(
        collab_app_state_with_config(&harness.app_url, cfg).await,
        None,
    );
    let addr = spawn_server(app).await;
    let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
    let mut ws = connect_member(addr, &wiki.session.session_token).await;
    let invalid_auth = encode(&WireFrame::Document {
        routing_key: routing_key.clone(),
        room: CollabRoomName::parse(&routing_key),
        message: DocumentMessage::Auth(AuthMessage::Token {
            token: "not-a-client-id".into(),
            provider_version: Some("4.6.0".into()),
        }),
    })
    .expect("encode invalid auth");
    ws.send(Message::Binary(invalid_auth.into())).await.unwrap();
    let _ = recv_document_frame(&mut ws, 2).await;
    assert_eq!(
        delivery_read_count(wiki.document_id),
        0,
        "pre-auth denial must not run the outbound delivery read"
    );
    harness.cleanup().await;
}

async fn wait_for_close_without_sync_update(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    expected: u16,
    within: Duration,
) -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + within;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining.min(Duration::from_millis(50)), ws.next()).await {
            Ok(Some(Ok(Message::Close(Some(frame))))) => {
                let code = ws_close_code(&frame);
                if code == expected {
                    return Ok(());
                }
                return Err(format!(
                    "close code {code} ({:?}), expected {expected}",
                    frame.code
                ));
            }
            Ok(Some(Ok(Message::Close(None)))) => {
                return Err(format!("Close without code, expected {expected}"));
            }
            Ok(None) => {
                return Err(format!(
                    "bare TCP EOF without CloseFrame, expected close code {expected}"
                ));
            }
            Ok(Some(Ok(Message::Binary(bytes)))) => {
                if binary_is_sync_update(&bytes) {
                    return Err("sync update delivered".into());
                }
            }
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(err))) => return Err(format!("websocket error: {err}")),
            _ => {}
        }
    }
    Err(format!("socket did not close with {expected}"))
}

async fn pooled_tenant_setting(pool: &PgPool) -> String {
    sqlx::query_scalar::<_, Option<String>>(
        "SELECT NULLIF(current_setting('app.tenant_id', true), '')",
    )
    .fetch_one(pool)
    .await
    .ok()
    .flatten()
    .unwrap_or_default()
}

#[tokio::test]
async fn collab_delivery_auth_and_send_share_one_dequeue_deadline() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    let mut cfg = test_collab_config(4, 30_000);
    cfg.outbound_send_deadline_ms = 200;
    let app = fvoci_server::http::router(
        collab_app_state_with_config(&harness.app_url, cfg).await,
        None,
    );
    let addr = spawn_server(app).await;
    let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
    let mut reader = connect_member(addr, &wiki.session.session_token).await;
    auth_and_join(&mut reader, &routing_key, 501).await;
    let writer_token = add_session_for_user(&wiki.session.pool, wiki.session.user_id).await;
    let mut writer = connect_member(addr, &writer_token.token).await;
    auth_and_join(&mut writer, &routing_key, 502).await;

    let (reached_rx, proceed_tx) = arm_delivery_read_barrier(wiki.session.session_id);
    writer
        .send(Message::Binary(
            sync_update_frame(&routing_key, &sample_hi_update()).into(),
        ))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), reached_rx)
        .await
        .expect("delivery read must reach the dequeue barrier")
        .expect("barrier signal");
    tokio::time::sleep(Duration::from_millis(50)).await;
    proceed_tx.send(()).expect("release delivery read");
    disarm_delivery_read_barrier(wiki.session.session_id);

    let mut saw_update = false;
    let wait_until = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < wait_until {
        match recv_document_frame_within(&mut reader, Duration::from_millis(100)).await {
            Some(WireFrame::Document {
                message:
                    DocumentMessage::Sync(SyncMessage {
                        step: SyncStep::Update,
                        ..
                    }),
                ..
            }) => {
                saw_update = true;
                break;
            }
            Some(_) => {}
            None => {}
        }
    }
    assert!(
        saw_update,
        "reader must still receive the update after a shared leftover send"
    );
    let budget = take_data_frame_send_budget(wiki.session.session_id)
        .expect("transport must record the Data-frame budget");
    assert_eq!(budget.total, Duration::from_millis(200));
    assert!(
        budget.auth_remaining <= budget.total,
        "auth remaining is taken from the dequeue deadline"
    );
    assert!(
        budget.send_remaining <= budget.auth_remaining,
        "send must use leftover time, not a second full budget"
    );
    assert!(
        budget.send_remaining <= Duration::from_millis(180),
        "holding auth for 50ms must shrink send leftover below a fresh 200ms budget, got {:?}",
        budget.send_remaining
    );
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_delivery_auth_timeout_closes_1011_without_data_frame() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    let mut cfg = test_collab_config(4, 30_000);
    cfg.outbound_send_deadline_ms = 100;
    let app = fvoci_server::http::router(
        collab_app_state_with_config(&harness.app_url, cfg).await,
        None,
    );
    let addr = spawn_server(app).await;
    let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
    let mut reader = connect_member(addr, &wiki.session.session_token).await;
    auth_and_join(&mut reader, &routing_key, 511).await;
    let writer_token = add_session_for_user(&wiki.session.pool, wiki.session.user_id).await;
    let mut writer = connect_member(addr, &writer_token.token).await;
    auth_and_join(&mut writer, &routing_key, 512).await;

    let (reached_rx, proceed_tx) = arm_delivery_read_barrier(wiki.session.session_id);
    writer
        .send(Message::Binary(
            sync_update_frame(&routing_key, &sample_hi_update()).into(),
        ))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), reached_rx)
        .await
        .expect("delivery read must reach the dequeue barrier")
        .expect("barrier signal");
    wait_for_close_without_sync_update(&mut reader, 1011, Duration::from_millis(400))
        .await
        .expect("auth timeout must close 1011 without delivering the Data frame");
    drop(proceed_tx);
    disarm_delivery_read_barrier(wiki.session.session_id);
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_delivery_auth_cancel_closes_without_waiting_full_deadline() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    let mut cfg = test_collab_config_with_revoke(4, 30_000, 50);
    cfg.outbound_send_deadline_ms = 5_000;
    let app = fvoci_server::http::router(
        collab_app_state_with_config(&harness.app_url, cfg).await,
        None,
    );
    let addr = spawn_server(app).await;
    let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
    let mut reader = connect_member(addr, &wiki.session.session_token).await;
    auth_and_join(&mut reader, &routing_key, 521).await;
    let writer_token = add_session_for_user(&wiki.session.pool, wiki.session.user_id).await;
    let mut writer = connect_member(addr, &writer_token.token).await;
    auth_and_join(&mut writer, &routing_key, 522).await;

    let (reached_rx, proceed_tx) = arm_delivery_read_barrier(wiki.session.session_id);
    writer
        .send(Message::Binary(
            sync_update_frame(&routing_key, &sample_hi_update()).into(),
        ))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), reached_rx)
        .await
        .expect("delivery read must reach the dequeue barrier")
        .expect("barrier signal");
    revoke_session(
        &wiki.session.pool,
        &fvoci_server::auth::token::hash_token(&wiki.session.session_token),
        Some(wiki.session.user_id),
    )
    .await
    .expect("revoke reader");
    let started = std::time::Instant::now();
    wait_for_close_without_sync_update(&mut reader, 1008, Duration::from_millis(800))
        .await
        .expect("cancel must close 1008 without delivering the Data frame");
    assert!(
        started.elapsed() < Duration::from_millis(1500),
        "cancel must not run the full outbound deadline, elapsed {:?}",
        started.elapsed()
    );
    drop(proceed_tx);
    disarm_delivery_read_barrier(wiki.session.session_id);
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_delivery_cancel_and_error_reset_pool_tenant_context() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&harness.app_url)
        .await
        .unwrap();
    let ws = wiki.session.workspace_id;
    let user = wiki.session.user_id;
    let session = wiki.session.session_id;
    let doc = wiki.document_id;

    let (reached_rx, proceed_tx) = arm_delivery_read_barrier(session);
    let handle = tokio::spawn({
        let pool = pool.clone();
        async move { check_delivery_admission(&pool, ws, user, session, doc).await }
    });
    tokio::time::timeout(Duration::from_secs(2), reached_rx)
        .await
        .expect("in-tx tenant must be set before the cancel barrier")
        .expect("barrier signal");
    handle.abort();
    let _ = handle.await;
    drop(proceed_tx);
    disarm_delivery_read_barrier(session);
    let tenant = tokio::time::timeout(Duration::from_secs(2), pooled_tenant_setting(&pool))
        .await
        .expect("pool connection must be reusable after cancel")
        .trim()
        .to_string();
    assert!(
        tenant.is_empty(),
        "cancelled delivery tx must not leak app.tenant_id onto the pooled connection, got {tenant:?}"
    );
    let after_cancel = tokio::time::timeout(
        Duration::from_secs(2),
        check_delivery_admission(&pool, ws, user, session, doc),
    )
    .await
    .expect("admission after cancel must not hang on a dirty connection")
    .expect("admission query");
    assert!(matches!(
        after_cancel,
        DeliveryAdmission::Allowed { read_only: false }
    ));

    arm_force_delivery_tx_error(session);
    let failed = check_delivery_admission(&pool, ws, user, session, doc).await;
    disarm_force_delivery_tx_error(session);
    assert!(
        failed.is_err(),
        "forced in-tx error must surface as sqlx::Error"
    );
    let tenant = tokio::time::timeout(Duration::from_secs(2), pooled_tenant_setting(&pool))
        .await
        .expect("pool connection must be reusable after in-tx error")
        .trim()
        .to_string();
    assert!(
        tenant.is_empty(),
        "failed delivery tx must not leak app.tenant_id onto the pooled connection, got {tenant:?}"
    );
    let after_error = check_delivery_admission(&pool, ws, user, session, doc)
        .await
        .expect("admission after error");
    assert!(matches!(
        after_error,
        DeliveryAdmission::Allowed { read_only: false }
    ));
    pool.close().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_authenticated_peer_fanout_records_observed_delivery() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    let mut cfg = test_collab_config(4, 30_000);
    cfg.max_collab_sockets = 16;
    cfg.max_collab_sockets_per_session = 4;
    cfg.max_connections_per_room = 16;
    let app = fvoci_server::http::router(
        collab_app_state_with_config(&harness.app_url, cfg).await,
        None,
    );
    let addr = spawn_server(app).await;
    let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);

    let mut writer = connect_member(addr, &wiki.session.session_token).await;
    auth_and_join(&mut writer, &routing_key, 600).await;

    const PEER_COUNT: usize = 5;
    let mut readers = Vec::with_capacity(PEER_COUNT);
    for index in 0..PEER_COUNT {
        let token = add_session_for_user(&wiki.session.pool, wiki.session.user_id).await;
        let mut reader = connect_member(addr, &token.token).await;
        auth_and_join(&mut reader, &routing_key, 601 + index as u32).await;
        readers.push(reader);
    }

    let drain_until = tokio::time::Instant::now() + Duration::from_millis(250);
    while tokio::time::Instant::now() < drain_until {
        for reader in &mut readers {
            let _ = tokio::time::timeout(Duration::from_millis(20), reader.next()).await;
        }
        let _ = tokio::time::timeout(Duration::from_millis(20), writer.next()).await;
    }

    reset_delivery_read_count(wiki.document_id);
    let overall_started = std::time::Instant::now();
    writer
        .send(Message::Binary(
            sync_update_frame(&routing_key, &sample_hi_update()).into(),
        ))
        .await
        .unwrap();

    let mut latencies_ms = vec![None; PEER_COUNT];
    let wait_until = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < wait_until && latencies_ms.iter().any(|v| v.is_none()) {
        for (index, reader) in readers.iter_mut().enumerate() {
            if latencies_ms[index].is_some() {
                continue;
            }
            match tokio::time::timeout(Duration::from_millis(50), reader.next()).await {
                Ok(Some(Ok(Message::Close(Some(frame))))) => {
                    panic!(
                        "peer {index} unexpected CloseFrame {} ({:?}); 1011/disconnect not allowed on this bounded fan-out",
                        u16::from(frame.code),
                        frame.code
                    );
                }
                Ok(Some(Ok(Message::Close(None)))) | Ok(None) => {
                    panic!("peer {index} disconnected without CloseFrame during bounded fan-out");
                }
                Ok(Some(Ok(Message::Binary(bytes)))) => {
                    if binary_is_sync_update(&bytes) {
                        latencies_ms[index] = Some(overall_started.elapsed().as_millis());
                    }
                }
                Ok(Some(Err(err))) => panic!("peer {index} websocket error: {err}"),
                _ => {}
            }
        }
    }
    let duration = overall_started.elapsed();
    assert!(
        latencies_ms.iter().all(|v| v.is_some()),
        "all {PEER_COUNT} distinct authenticated peers must receive the update; latencies_ms={latencies_ms:?}"
    );
    let observed_reads = delivery_read_count(wiki.document_id);
    // Writer echo is also Data (broadcast_update to every connection). No-cache
    // therefore has at least one delivery read per reader plus the writer.
    assert!(
        observed_reads > PEER_COUNT,
        "current ACL/no-cache must run a delivery read per peer plus writer echo, got {observed_reads}"
    );

    let query_started = std::time::Instant::now();
    let admission = check_delivery_admission(
        &wiki.session.pool,
        wiki.session.workspace_id,
        wiki.session.user_id,
        wiki.session.session_id,
        wiki.document_id,
    )
    .await
    .expect("admission query");
    let query_ms = query_started.elapsed().as_millis();
    assert!(matches!(
        admission,
        DeliveryAdmission::Allowed { read_only: false }
    ));

    let http_started = std::time::Instant::now();
    let http = reqwest::Client::new()
        .get(format!("http://{addr}/collab"))
        .header("origin", PUBLIC_ORIGIN)
        .send()
        .await
        .expect("http /collab");
    let http_ms = http_started.elapsed().as_millis();
    assert_eq!(
        http.status().as_u16(),
        426,
        "HTTP /collab must still answer during bounded fan-out"
    );

    let observed: Vec<u128> = latencies_ms.into_iter().map(|v| v.unwrap()).collect();
    eprintln!(
        "collab fan-out observation: peers={PEER_COUNT} updates=1 duration_ms={} rate_updates_per_s={:.3} delivery_latencies_ms={observed:?} delivery_reads={observed_reads} admission_query_ms={query_ms} http_collab_ms={http_ms} unexpected_1011=0 disconnects=0",
        duration.as_millis(),
        1000.0 / duration.as_millis().max(1) as f64,
    );
    harness.cleanup().await;
}
