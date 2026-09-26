#![allow(dead_code)]

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
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
    encode, AuthMessage, CollabRoomName, ConnectionMessage, DocumentMessage, SyncMessage, SyncStep,
    WireFrame,
};
use fvoci_server::collab::y_sync::{encode_sync_payload, parse_sync_payload};
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
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
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
    fvoci_server::db::migrate::apply_app_role_grants(pool, role_name)
        .await
        .expect("grant");
}

/// Route server `tracing` output (e.g. the underlying error behind a 1011
/// "collab unavailable" join) into the libtest-captured output of the test that
/// produced it. Default `warn`; override with `RUST_LOG`.
pub fn init_test_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_test_writer()
        .try_init();
}

impl TestDb {
    pub async fn bootstrap() -> Self {
        init_test_tracing();
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

    pub fn db_name(&self) -> &str {
        &self.db_name
    }

    pub fn role_name(&self) -> &str {
        &self.role_name
    }

    pub async fn database_exists(admin_url: &str, db_name: &str) -> Result<bool, String> {
        let server_url = server_db_url(admin_url);
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(&server_url)
            .await
            .map_err(|e| format!("connect admin to probe database: {e}"))?;
        let (exists,): (bool,) =
            sqlx::query_as("SELECT EXISTS(SELECT 1 FROM pg_database WHERE datname = $1)")
                .bind(db_name)
                .fetch_one(&pool)
                .await
                .map_err(|e| format!("probe database {db_name}: {e}"))?;
        pool.close().await;
        Ok(exists)
    }

    pub async fn role_exists(admin_url: &str, role_name: &str) -> Result<bool, String> {
        let server_url = server_db_url(admin_url);
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(&server_url)
            .await
            .map_err(|e| format!("connect admin to probe role: {e}"))?;
        let (exists,): (bool,) =
            sqlx::query_as("SELECT EXISTS(SELECT 1 FROM pg_roles WHERE rolname = $1)")
                .bind(role_name)
                .fetch_one(&pool)
                .await
                .map_err(|e| format!("probe role {role_name}: {e}"))?;
        pool.close().await;
        Ok(exists)
    }

    pub async fn cleanup(self) -> Result<(), String> {
        let server_url = server_db_url(&self.admin_url);
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(&server_url)
            .await
            .map_err(|e| format!("connect admin for cleanup: {e}"))?;
        let mut errors = Vec::new();
        if let Err(error) = sqlx::query(&format!(
            "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '{}'",
            self.db_name
        ))
        .execute(&pool)
        .await
        {
            errors.push(format!("terminate backends for {}: {error}", self.db_name));
        }
        if let Err(error) = sqlx::query(&format!("DROP DATABASE IF EXISTS \"{}\"", self.db_name))
            .execute(&pool)
            .await
        {
            errors.push(format!("drop database {}: {error}", self.db_name));
        }
        if let Err(error) = sqlx::query(&format!("DROP ROLE IF EXISTS \"{}\"", self.role_name))
            .execute(&pool)
            .await
        {
            errors.push(format!("drop role {}: {error}", self.role_name));
        }
        pool.close().await;
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
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

/// Per-fixture symlink to the real collab-engine binary. Unlinking breaks fresh
/// validator spawns for that path while an already-running primary child survives.
pub struct OwnedEngineSymlink {
    link_path: PathBuf,
    real_bin: PathBuf,
}

impl OwnedEngineSymlink {
    pub fn new() -> Self {
        let real_bin = fvoci_server::collab::config::require_collab_engine_for_tests();
        let link_path = std::env::temp_dir().join(format!(
            "fvoci-collab-engine-{}.link",
            Uuid::now_v7().simple()
        ));
        std::os::unix::fs::symlink(&real_bin, &link_path).unwrap_or_else(|e| {
            panic!(
                "symlink {} -> {}: {e}",
                link_path.display(),
                real_bin.display()
            )
        });
        Self {
            link_path,
            real_bin,
        }
    }

    pub fn path(&self) -> PathBuf {
        self.link_path.clone()
    }

    pub fn break_spawn(&self) {
        if std::fs::symlink_metadata(&self.link_path).is_ok() {
            std::fs::remove_file(&self.link_path).unwrap_or_else(|e| {
                panic!(
                    "unlink owned engine symlink {}: {e}",
                    self.link_path.display()
                )
            });
        }
        assert!(
            std::fs::symlink_metadata(&self.link_path).is_err(),
            "owned engine symlink must be absent after break_spawn: {}",
            self.link_path.display()
        );
    }

    pub fn restore(&self) {
        if std::fs::symlink_metadata(&self.link_path).is_ok() {
            std::fs::remove_file(&self.link_path).unwrap_or_else(|e| {
                panic!(
                    "clear stale owned engine symlink {}: {e}",
                    self.link_path.display()
                )
            });
        }
        std::os::unix::fs::symlink(&self.real_bin, &self.link_path).unwrap_or_else(|e| {
            panic!(
                "restore engine symlink {} -> {}: {e}",
                self.link_path.display(),
                self.real_bin.display()
            )
        });
    }
}

impl Drop for OwnedEngineSymlink {
    fn drop(&mut self) {
        if std::fs::symlink_metadata(&self.link_path).is_ok() {
            std::fs::remove_file(&self.link_path).unwrap_or_else(|e| {
                panic!(
                    "drop owned engine symlink {}: {e}",
                    self.link_path.display()
                )
            });
        }
    }
}

pub fn test_collab_config_with_engine(
    max_rooms: usize,
    idle_evict_ms: u64,
    engine_bin: impl AsRef<Path>,
) -> CollabConfig {
    let mut cfg = test_collab_config(max_rooms, idle_evict_ms);
    cfg.engine_bin = engine_bin.as_ref().to_path_buf();
    cfg
}

pub fn test_collab_config(max_rooms: usize, idle_evict_ms: u64) -> CollabConfig {
    CollabConfig {
        engine_bin: fvoci_server::collab::config::require_collab_engine_for_tests(),
        limits: collab_engine::Limits::for_tests(),
        max_rooms,
        max_child_concurrency: fvoci_server::collab::config::derive_max_child_concurrency(
            max_rooms,
        ),
        memory_budget_bytes: fvoci_server::collab::config::DEFAULT_MEMORY_BUDGET_BYTES,
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
        rpc_timeout_ms: 5_000,
    }
}

pub async fn setup_wiki_doc_batch(harness: &TestDb, count: usize) -> Vec<WikiDocFixture> {
    let session = setup_owner_session(harness).await;
    let mut docs = Vec::with_capacity(count);
    for index in 0..count {
        let title = format!("Collab capacity doc {index}");
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
    let max_rooms = cfg.max_rooms;
    collab_app_state_with_pool(
        app_url,
        cfg,
        fvoci_server::collab::config::derive_app_pool_max_connections(max_rooms),
    )
    .await
}

pub async fn collab_app_state_with_pool(
    app_url: &str,
    cfg: CollabConfig,
    max_connections: u32,
) -> (AppState, Arc<CollabHub>) {
    let pool = PgPoolOptions::new()
        .max_connections(max_connections)
        .connect(app_url)
        .await
        .expect("app pool");
    let hub = Arc::new(CollabHub::new(cfg, pool));
    let storage_root =
        std::env::temp_dir().join(format!("fvoci-collab-proj-store-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&storage_root).expect("storage root");
    let state = AppState {
        auth: Arc::new(AuthService {
            db: Db::new(hub.pool().clone()),
            password_keys: Keyring::parse(PEPPER, "test").expect("pepper"),
        }),
        branding_name: "FVOCI".to_string(),
        public_origin: PUBLIC_ORIGIN.to_string(),
        cookie_secure: false,
        rate_limiter: RateLimiter::new(),
        storage: fvoci_server::attachments::LocalStorage::new(storage_root).into(),
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
        document_convert: None,
        markdown: Some(
            fvoci_server::documents::markdown_helper::MarkdownHelper::new(env!(
                "CARGO_BIN_EXE_fvoci-server"
            )),
        ),
        import_wake: None,
        import_extractor_available: false,
        quota: Default::default(),
        mailer: std::sync::Arc::new(fvoci_server::mail::Mailer::disabled()),
    };
    (state, hub)
}

pub struct TestServer {
    pub addr: SocketAddr,
    hub: Arc<CollabHub>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    join: Option<tokio::task::JoinHandle<()>>,
    /// Released only after `hub.shutdown()` has reaped this server's helpers.
    _helper_slot: tokio::sync::OwnedSemaphorePermit,
}

/// Concurrent collab test servers per test process.
///
/// The collab-engine primary child cap is process-wide and last-write-wins:
/// every `CollabHub::new` sets it to `max_rooms + OFFLINE_REVISION_PRIMARY_HEADROOM`
/// (6 for the smallest `max_rooms` = 2 used here). Production runs one hub per
/// process, whose room slots stay under that cap. libtest runs many tests, each
/// with its own hub, in one process; without this gate, ungated parallel tests
/// exceed the shared cap and a helper spawn fails with `ResourceLimit` ("live
/// primary collab children at cap N"), which surfaces as a 1011 "collab
/// unavailable" close before auth. Each test server keeps at most one live room
/// plus one offline revision-capture helper, so 3 servers x 2 children fit in 6.
const MAX_CONCURRENT_TEST_SERVERS: usize = 3;

static TEST_SERVER_SLOTS: std::sync::LazyLock<Arc<tokio::sync::Semaphore>> =
    std::sync::LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_TEST_SERVERS)));

/// One concurrent-server slot (see `MAX_CONCURRENT_TEST_SERVERS`). Suites that
/// build `CollabHub`s directly instead of through `spawn_server` hold one permit
/// per live hub and drop it only after `hub.shutdown()` has reaped its children.
pub async fn acquire_test_server_slot() -> tokio::sync::OwnedSemaphorePermit {
    TEST_SERVER_SLOTS
        .clone()
        .acquire_owned()
        .await
        .expect("test server slot")
}

impl TestServer {
    pub fn hub(&self) -> Arc<CollabHub> {
        self.hub.clone()
    }

    pub async fn shutdown(mut self) -> Result<(), String> {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        let server_result = match self.join.take() {
            Some(join) => join
                .await
                .map_err(|error| format!("test server task failed: {error}")),
            None => Ok(()),
        };
        self.hub.shutdown().await;
        self.hub.pool().close().await;
        server_result
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

    pub fn hub(&self) -> Arc<CollabHub> {
        self.servers
            .last()
            .expect("spawn_router before hub()")
            .hub()
    }

    pub async fn spawn_router(&mut self, app_url: &str, cfg: CollabConfig) -> SocketAddr {
        let (state, hub) = collab_app_state(app_url, cfg).await;
        self.spawn_router_state(state, hub).await
    }

    pub async fn spawn_router_with_pool(
        &mut self,
        app_url: &str,
        cfg: CollabConfig,
        max_connections: u32,
    ) -> SocketAddr {
        let (state, hub) = collab_app_state_with_pool(app_url, cfg, max_connections).await;
        self.spawn_router_state(state, hub).await
    }

    pub async fn spawn_router_state(&mut self, state: AppState, hub: Arc<CollabHub>) -> SocketAddr {
        let server = spawn_server(fvoci_server::http::router(state, None), hub).await;
        let addr = server.addr;
        self.servers.push(server);
        addr
    }

    pub async fn shutdown_last_server(&mut self) -> Result<(), String> {
        if let Some(server) = self.servers.pop() {
            server.shutdown().await?;
        }
        Ok(())
    }

    pub async fn finish(mut self) -> Result<(), String> {
        let mut errors = Vec::new();
        while let Some(server) = self.servers.pop() {
            if let Err(error) = server.shutdown().await {
                errors.push(error);
            }
        }
        if let Err(error) = self.harness.cleanup().await {
            errors.push(error);
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }
}

pub async fn spawn_server(app: Router, hub: Arc<CollabHub>) -> TestServer {
    let helper_slot = TEST_SERVER_SLOTS
        .clone()
        .acquire_owned()
        .await
        .expect("test server slot");
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
        _helper_slot: helper_slot,
    }
}

pub fn delete_only_base_update() -> Vec<u8> {
    engine_fixture("delete_only_base.v1")
}

/// Crafted updateV1 tail from collab-engine `classify_decode` coverage: a huge varint
/// length makes `Update::decode_v1` return `ReadError::NotEnoughMemory` → `LimitKind::Memory`.
pub fn huge_varint_memory_candidate() -> Vec<u8> {
    vec![0xff, 0xff, 0xff, 0xff, 0x0f]
}

/// Corrupts `utf8_korean.v1` like `invalid_utf8_is_malformed_result_and_child_is_recycled`.
pub fn invalid_utf8_update_candidate() -> Vec<u8> {
    let mut bytes = engine_fixture("utf8_korean.v1");
    let marker = [0xEC, 0x95, 0x88];
    let pos = bytes
        .windows(3)
        .position(|w| w == marker)
        .expect("안녕 utf8 marker in utf8_korean.v1 fixture");
    bytes[pos] = 0xFF;
    bytes
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
    let Message::Binary(bytes) = msg else {
        panic!("expected binary auth reply, got {msg:?}");
    };
    let frame = fvoci_server::collab::wire::decode(&bytes).expect("decode");
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
        match tokio::time::timeout(remaining.min(Duration::from_millis(200)), ws.next()).await {
            Err(_) => continue,
            Ok(None) => panic!("bare TCP EOF while waiting for stateless {expected}"),
            Ok(Some(Err(err))) => {
                panic!("websocket error while waiting for stateless {expected}: {err}")
            }
            Ok(Some(Ok(Message::Close(frame)))) => {
                panic!("CloseFrame while waiting for stateless {expected}: {frame:?}")
            }
            Ok(Some(Ok(Message::Binary(bytes)))) => {
                match fvoci_server::collab::wire::decode(&bytes) {
                    Ok(WireFrame::Document {
                        message: DocumentMessage::Stateless(body),
                        ..
                    }) if body == expected => return true,
                    Ok(WireFrame::Document {
                        message: DocumentMessage::Stateless(body),
                        ..
                    }) if body.starts_with("persisted:") || body.starts_with("persist-failed:") => {
                        panic!("unexpected persist stateless {body}, expected {expected}");
                    }
                    Ok(WireFrame::Document {
                        message: DocumentMessage::Close { reason },
                        ..
                    }) => {
                        panic!("document Close while waiting for stateless {expected}: {reason:?}")
                    }
                    Ok(_) | Err(_) => {}
                }
            }
            Ok(Some(Ok(_))) => {}
        }
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

pub async fn wait_for_sync_update(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    within: Duration,
) -> bool {
    let deadline = tokio::time::Instant::now() + within;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining.min(Duration::from_millis(200)), ws.next()).await {
            Err(_) => continue,
            Ok(None) => panic!("bare TCP EOF while waiting for Sync Update"),
            Ok(Some(Err(err))) => panic!("websocket error while waiting for Sync Update: {err}"),
            Ok(Some(Ok(Message::Close(frame)))) => {
                panic!("CloseFrame while waiting for Sync Update: {frame:?}")
            }
            Ok(Some(Ok(Message::Binary(bytes)))) => {
                if matches!(
                    fvoci_server::collab::wire::decode(&bytes),
                    Ok(WireFrame::Document {
                        message: DocumentMessage::Sync(SyncMessage {
                            step: SyncStep::Update,
                            ..
                        }),
                        ..
                    })
                ) {
                    return true;
                }
            }
            Ok(Some(Ok(_))) => {}
        }
    }
    false
}

pub fn ws_close_code(frame: &CloseFrame) -> u16 {
    u16::from(frame.code)
}

fn close_reason_str(frame: &CloseFrame) -> &str {
    frame.reason.as_str()
}

pub async fn wait_for_ws_close_code(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    expected: u16,
    within: Duration,
    reject_sync_update: bool,
    expected_reason: Option<&str>,
) {
    let deadline = tokio::time::Instant::now() + within;
    let mut saw_applied_false = false;
    let mut saw_applied_true = false;
    let mut last_frame = String::from("none");
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining.min(Duration::from_millis(100)), ws.next()).await {
            Ok(Some(Ok(Message::Close(Some(frame))))) => {
                let code = ws_close_code(&frame);
                assert_eq!(
                    code, expected,
                    "CloseFrame {code} ({:?}), expected {expected}; reason {:?}",
                    frame.code, frame.reason
                );
                if let Some(expected_reason) = expected_reason {
                    assert_eq!(
                        close_reason_str(&frame),
                        expected_reason,
                        "CloseFrame {expected} reason mismatch"
                    );
                }
                assert!(
                    !saw_applied_false,
                    "must not receive applied:false before CloseFrame {expected}"
                );
                return;
            }
            Ok(Some(Ok(Message::Close(None)))) => {
                panic!("Close without code, expected CloseFrame {expected}");
            }
            Ok(None) => panic!("bare TCP EOF, expected CloseFrame {expected}"),
            Ok(Some(Err(err))) => panic!("websocket error before CloseFrame {expected}: {err}"),
            Ok(Some(Ok(Message::Binary(bytes)))) => {
                last_frame = format!("{:?}", fvoci_server::collab::wire::decode(&bytes));
                if let Ok(WireFrame::Document {
                    message: DocumentMessage::SyncStatus { applied: false },
                    ..
                }) = fvoci_server::collab::wire::decode(&bytes)
                {
                    saw_applied_false = true;
                }
                if let Ok(WireFrame::Document {
                    message: DocumentMessage::SyncStatus { applied: true },
                    ..
                }) = fvoci_server::collab::wire::decode(&bytes)
                {
                    saw_applied_true = true;
                }
                if reject_sync_update
                    && matches!(
                        fvoci_server::collab::wire::decode(&bytes),
                        Ok(WireFrame::Document {
                            message: DocumentMessage::Sync(SyncMessage {
                                step: SyncStep::Update,
                                ..
                            }),
                            ..
                        })
                    )
                {
                    panic!("rejected update must not broadcast Sync Update before close");
                }
            }
            Ok(Some(Ok(other))) => {
                last_frame = format!("non-binary {other:?}");
            }
            Err(_) => {}
        }
    }
    panic!(
        "timed out waiting for CloseFrame {expected}; saw_applied_false={saw_applied_false}; saw_applied_true={saw_applied_true}; last={last_frame}"
    );
}

pub async fn wait_for_writer_close_without_peer_update(
    writer: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    peer: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    expected: u16,
    within: Duration,
    expected_reason: Option<&str>,
) {
    let deadline = tokio::time::Instant::now() + within;
    let mut saw_applied_false = false;
    let mut last_frame = String::from("none");
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let slice = remaining.min(Duration::from_millis(100));
        tokio::select! {
            biased;
            msg = tokio::time::timeout(slice, writer.next()) => {
                match msg {
                    Ok(Some(Ok(Message::Close(Some(frame))))) => {
                        let code = ws_close_code(&frame);
                        assert_eq!(
                            code, expected,
                            "CloseFrame {code} ({:?}), expected {expected}; reason {:?}",
                            frame.code, frame.reason
                        );
                        if let Some(expected_reason) = expected_reason {
                            assert_eq!(
                                close_reason_str(&frame),
                                expected_reason,
                                "CloseFrame {expected} reason mismatch"
                            );
                        }
                        assert!(
                            !saw_applied_false,
                            "must not receive applied:false before CloseFrame {expected}"
                        );
                        return;
                    }
                    Ok(Some(Ok(Message::Close(None)))) => {
                        panic!("Close without code, expected CloseFrame {expected}");
                    }
                    Ok(None) => panic!("bare TCP EOF, expected CloseFrame {expected}"),
                    Ok(Some(Err(err))) => {
                        panic!("websocket error before CloseFrame {expected}: {err}")
                    }
                    Ok(Some(Ok(Message::Binary(bytes)))) => {
                        last_frame = format!("{:?}", fvoci_server::collab::wire::decode(&bytes));
                        if let Ok(WireFrame::Document {
                            message: DocumentMessage::SyncStatus { applied: false },
                            ..
                        }) = fvoci_server::collab::wire::decode(&bytes)
                        {
                            saw_applied_false = true;
                        }
                        if matches!(
                            fvoci_server::collab::wire::decode(&bytes),
                            Ok(WireFrame::Document {
                                message: DocumentMessage::Sync(SyncMessage {
                                    step: SyncStep::Update,
                                    ..
                                }),
                                ..
                            })
                        ) {
                            panic!("rejected update must not broadcast Sync Update before close");
                        }
                    }
                    Ok(Some(Ok(_))) | Err(_) => {}
                }
            }
            msg = tokio::time::timeout(slice, peer.next()) => {
                match msg {
                    Ok(Some(Ok(Message::Binary(bytes)))) => {
                        if let Ok(WireFrame::Document {
                            message: DocumentMessage::SyncStatus { applied: false },
                            ..
                        }) = fvoci_server::collab::wire::decode(&bytes)
                        {
                            panic!("peer must not receive applied:false for a pre-commit engine close");
                        }
                        if matches!(
                            fvoci_server::collab::wire::decode(&bytes),
                            Ok(WireFrame::Document {
                                message: DocumentMessage::Sync(SyncMessage {
                                    step: SyncStep::Update,
                                    ..
                                }),
                                ..
                            })
                        ) {
                            panic!("peer must not receive Sync Update for a pre-commit rejected edit");
                        }
                    }
                    Ok(Some(Ok(Message::Close(frame)))) => {
                        panic!("peer closed before explicit Step1: {frame:?}")
                    }
                    Ok(None) => panic!("peer TCP EOF before explicit Step1"),
                    Ok(Some(Err(err))) => {
                        panic!("peer websocket error before explicit Step1: {err}")
                    }
                    Ok(Some(Ok(_))) | Err(_) => {}
                }
            }
        }
    }
    panic!(
        "timed out waiting for writer CloseFrame {expected}; saw_applied_false={saw_applied_false}; last={last_frame}"
    );
}

pub async fn wait_for_committed_update_then_close(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    expected_routing_key: &str,
    expected_payload: &[u8],
    expected: u16,
    within: Duration,
    expected_reason: Option<&str>,
) {
    let deadline = tokio::time::Instant::now() + within;
    let mut saw_applied_false = false;
    let mut saw_update = false;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining.min(Duration::from_millis(100)), ws.next()).await {
            Ok(Some(Ok(Message::Close(Some(frame))))) => {
                let code = ws_close_code(&frame);
                assert_eq!(
                    code, expected,
                    "CloseFrame {code} ({:?}), expected {expected}; reason {:?}",
                    frame.code, frame.reason
                );
                if let Some(expected_reason) = expected_reason {
                    assert_eq!(
                        close_reason_str(&frame),
                        expected_reason,
                        "CloseFrame {expected} reason mismatch"
                    );
                }
                assert!(
                    !saw_applied_false,
                    "must not receive applied:false before CloseFrame {expected}"
                );
                assert!(
                    saw_update,
                    "committed update must broadcast Sync Update before CloseFrame {expected}"
                );
                return;
            }
            Ok(Some(Ok(Message::Close(None)))) => {
                panic!("Close without code, expected CloseFrame {expected} after Sync Update");
            }
            Ok(None) => panic!("bare TCP EOF, expected CloseFrame {expected} after Sync Update"),
            Ok(Some(Err(err))) => {
                panic!("websocket error before CloseFrame {expected} after Sync Update: {err}")
            }
            Ok(Some(Ok(Message::Binary(bytes)))) => {
                if let Ok(WireFrame::Document {
                    message: DocumentMessage::SyncStatus { applied: false },
                    ..
                }) = fvoci_server::collab::wire::decode(&bytes)
                {
                    saw_applied_false = true;
                }
                if let Ok(WireFrame::Document {
                    routing_key,
                    message:
                        DocumentMessage::Sync(SyncMessage {
                            step: SyncStep::Update,
                            y_protocol,
                        }),
                    ..
                }) = fvoci_server::collab::wire::decode(&bytes)
                {
                    assert_eq!(
                        routing_key, expected_routing_key,
                        "committed Sync Update routing_key must match the room"
                    );
                    let (step, payload) = parse_sync_payload(
                        &y_protocol,
                        fvoci_server::collab::wire::Limits::DEFAULT.max_binary_payload_bytes,
                    )
                    .expect("committed Sync Update y_protocol must parse");
                    assert_eq!(step, SyncStep::Update);
                    assert_eq!(
                        payload.as_slice(),
                        expected_payload,
                        "committed Sync Update payload must match fixture"
                    );
                    saw_update = true;
                }
            }
            Ok(Some(Ok(_))) | Err(_) => {}
        }
    }
    panic!(
        "timed out waiting for CloseFrame {expected} after committed Update; saw_update={saw_update}; saw_applied_false={saw_applied_false}"
    );
}

pub async fn join_unavailable(
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
    let msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("timeout")
        .expect("stream")
        .expect("frame");
    assert!(
        matches!(&msg, Message::Close(Some(frame)) if u16::from(frame.code) == 1011),
        "unavailable room must close 1011 without reporting an authentication failure, got {msg:?}"
    );
}

pub async fn assert_peer_still_connected(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    within: Duration,
) {
    ws.send(Message::Binary(
        encode(&WireFrame::Connection(ConnectionMessage::Ping))
            .expect("ping frame")
            .into(),
    ))
    .await
    .unwrap();
    let deadline = tokio::time::Instant::now() + within;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining.min(Duration::from_millis(100)), ws.next()).await {
            Ok(Some(Ok(Message::Close(_)))) => {
                panic!("peer closed immediately after join recovery failure denied a new joiner");
            }
            Ok(Some(Ok(Message::Binary(bytes)))) => {
                if matches!(
                    fvoci_server::collab::wire::decode(&bytes),
                    Ok(WireFrame::Connection(ConnectionMessage::Pong))
                ) {
                    return;
                }
            }
            Ok(None) => panic!("bare TCP EOF while peer should remain connected"),
            Ok(Some(Err(err))) => {
                panic!("websocket error while peer should remain connected: {err}")
            }
            Ok(Some(Ok(_))) | Err(_) => {}
        }
    }
    panic!("timed out waiting for peer pong while connection should remain open");
}

async fn peer_step2_excludes_rejected_candidate(
    peer: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    expected_routing_key: &str,
    rejected_candidate: &[u8],
    within: Duration,
) {
    peer.send(Message::Binary(
        sync_step1_frame(expected_routing_key, &[0, 0]).into(),
    ))
    .await
    .unwrap();
    let deadline = tokio::time::Instant::now() + within;
    let mut saw_step2 = false;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining.min(Duration::from_millis(100)), peer.next()).await {
            Ok(Some(Ok(Message::Binary(bytes)))) => {
                if let Ok(WireFrame::Document {
                    routing_key,
                    message:
                        DocumentMessage::Sync(SyncMessage {
                            step: SyncStep::Step2,
                            y_protocol,
                        }),
                    ..
                }) = fvoci_server::collab::wire::decode(&bytes)
                {
                    assert_eq!(
                        routing_key, expected_routing_key,
                        "peer Step2 routing_key must match the room"
                    );
                    let (step, payload) = parse_sync_payload(
                        &y_protocol,
                        fvoci_server::collab::wire::Limits::DEFAULT.max_binary_payload_bytes,
                    )
                    .expect("peer Step2 y_protocol must parse");
                    assert_eq!(step, SyncStep::Step2);
                    if !rejected_candidate.is_empty() {
                        assert!(
                            !payload
                                .windows(rejected_candidate.len())
                                .any(|w| w == rejected_candidate),
                            "peer Step2 must not include rejected candidate bytes"
                        );
                    }
                    saw_step2 = true;
                    break;
                }
                if matches!(
                    fvoci_server::collab::wire::decode(&bytes),
                    Ok(WireFrame::Document {
                        message: DocumentMessage::Sync(SyncMessage {
                            step: SyncStep::Update,
                            ..
                        }),
                        ..
                    })
                ) {
                    panic!("rejected update must not broadcast Sync Update to peer");
                }
            }
            Ok(Some(Ok(Message::Close(frame)))) => {
                panic!("peer closed during post-rejection Step1 barrier: {frame:?}");
            }
            Ok(None) => panic!("peer TCP EOF during post-rejection Step1 barrier"),
            Ok(Some(Err(err))) => {
                panic!("peer websocket error during post-rejection Step1 barrier: {err}");
            }
            Ok(Some(Ok(_))) | Err(_) => {}
        }
    }
    assert!(
        saw_step2,
        "peer must receive Step2 after policy rejection proving committed state excludes candidate"
    );
}

pub async fn wait_for_policy_rejection_close(
    writer: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    peer: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    expected_routing_key: &str,
    rejected_candidate: &[u8],
    within: Duration,
) {
    let deadline = tokio::time::Instant::now() + within;
    let mut saw_applied_false = false;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let slice = remaining.min(Duration::from_millis(100));
        tokio::select! {
            biased;
            msg = tokio::time::timeout(slice, writer.next()) => {
                match msg {
                    Ok(Some(Ok(Message::Close(Some(frame))))) => {
                        assert_eq!(
                            ws_close_code(&frame),
                            1008,
                            "policy rejection must CloseFrame 1008, got {:?}",
                            frame.code
                        );
                        assert_eq!(
                            close_reason_str(&frame),
                            "update rejected",
                            "policy rejection reason mismatch"
                        );
                        assert!(
                            saw_applied_false,
                            "policy rejection must send applied:false before CloseFrame 1008"
                        );
                        peer_step2_excludes_rejected_candidate(
                            peer,
                            expected_routing_key,
                            rejected_candidate,
                            deadline.saturating_duration_since(tokio::time::Instant::now()),
                        )
                        .await;
                        return;
                    }
                    Ok(Some(Ok(Message::Close(None)))) => {
                        panic!("writer Close without code before policy CloseFrame 1008");
                    }
                    Ok(Some(Ok(Message::Binary(bytes)))) => {
                        if let Ok(WireFrame::Document {
                            routing_key,
                            message: DocumentMessage::SyncStatus { applied: false },
                            ..
                        }) = fvoci_server::collab::wire::decode(&bytes)
                        {
                            assert_eq!(
                                routing_key,
                                expected_routing_key,
                                "applied:false routing_key must match the room"
                            );
                            saw_applied_false = true;
                        }
                        if matches!(
                            fvoci_server::collab::wire::decode(&bytes),
                            Ok(WireFrame::Document {
                                message: DocumentMessage::Sync(SyncMessage {
                                    step: SyncStep::Update,
                                    ..
                                }),
                                ..
                            })
                        ) {
                            panic!("rejected update must not broadcast Sync Update to writer");
                        }
                    }
                    Ok(None) => panic!("bare TCP EOF before policy CloseFrame 1008"),
                    Ok(Some(Err(err))) => panic!("websocket error before policy CloseFrame 1008: {err}"),
                    Ok(Some(Ok(_))) | Err(_) => {}
                }
            }
            msg = tokio::time::timeout(slice, peer.next()) => {
                match msg {
                    Ok(Some(Ok(Message::Binary(bytes)))) => {
                        if matches!(
                            fvoci_server::collab::wire::decode(&bytes),
                            Ok(WireFrame::Document {
                                message: DocumentMessage::Sync(SyncMessage {
                                    step: SyncStep::Update,
                                    ..
                                }),
                                ..
                            })
                        ) {
                            panic!("rejected update must not broadcast Sync Update to peer");
                        }
                    }
                    Ok(Some(Ok(Message::Close(frame)))) => {
                        panic!("peer closed before writer policy CloseFrame 1008: {frame:?}");
                    }
                    Ok(None) => panic!("peer TCP EOF before writer policy CloseFrame 1008"),
                    Ok(Some(Err(err))) => {
                        panic!("peer websocket error before writer policy CloseFrame 1008: {err}");
                    }
                    Ok(Some(Ok(_))) | Err(_) => {}
                }
            }
        }
    }
    panic!("timed out waiting for policy CloseFrame 1008; saw_applied_false={saw_applied_false}");
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
