#![cfg(feature = "db-tests")]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::Mutex;
use std::time::Duration;

use axum::Router;
use chrono::{Duration as ChronoDuration, Utc};
use collab_engine::limits::room_memory_reservation_bytes;
use collab_engine::outcome::EngineStatus;
use collab_engine::process::{EngineSession, SpawnRequest};
use collab_engine::protocol::Request;
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
    disarm_spawn_room_block, AuthenticatedConnection, CollabSession, ConnectionLease, JoinError,
    RoomClientEvent, RoomJoin,
};
use fvoci_server::collab::transport::take_data_frame_send_budget;
use fvoci_server::collab::wire::{
    encode, AuthMessage, CollabKind, CollabRoomName, DocumentMessage, SyncMessage, SyncStep,
    WireFrame,
};
use fvoci_server::collab::y_sync::{encode_sync_payload, parse_sync_payload};
use fvoci_server::collab::CollabHub;
use fvoci_server::db::collab::{
    arm_force_estimate_fail, disarm_force_estimate_fail, estimate_persisted_collab_bytes,
    load_collab_document, resolve_collab_admission, COLLAB_ROOM_SESSION_LOCK_NAMESPACE,
};
use fvoci_server::db::collab_delivery::{
    arm_delivery_read_barrier, arm_force_delivery_read_fail, arm_force_delivery_tx_error,
    check_delivery_admission, delivery_read_count, disarm_delivery_read_barrier,
    disarm_force_delivery_read_fail, disarm_force_delivery_tx_error, reset_delivery_read_count,
    DeliveryAdmission,
};
use fvoci_server::db::context::lock_key_from_uuid;
use fvoci_server::db::documents::{empty_document_json, CreateDocumentInput};
use fvoci_server::db::identity::revoke_session;
use fvoci_server::db::workspace;
use fvoci_server::db::{documents, migrate, pool, Db};
use fvoci_server::http::rate_limit::RateLimiter;
use fvoci_server::http::state::AppState;
use rand::RngCore;
use serde_json::Value;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use tokio::sync::{mpsc, watch, OwnedSemaphorePermit, Semaphore};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

const PEPPER: &str =
    r#"{"test":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#;
const PUBLIC_ORIGIN: &str = "http://localhost";
const LIFECYCLE_TEST_TIMEOUT: Duration = Duration::from_secs(30);
const OP_CAP_TEST_TIMEOUT: Duration = Duration::from_secs(45);
const FIXTURE_PASSWORD: &str = "supersecret1";

static FIXTURE_PASSWORD_HASH: tokio::sync::OnceCell<String> = tokio::sync::OnceCell::const_new();

/// One Argon2 hash per test binary. Password verification is not under test here.
async fn fixture_password_hash() -> &'static str {
    FIXTURE_PASSWORD_HASH
        .get_or_init(|| async {
            fvoci_server::auth::password::hash_password(
                FIXTURE_PASSWORD,
                &Keyring::parse(PEPPER, "test").unwrap(),
            )
            .await
            .expect("fixture password hash")
        })
        .await
}

/// Parallel tests in one binary must not storm past the process-wide live-helper cap.
/// Reserve one slot per hub/server (four for `collab_lifecycle_max_rooms_then_reuse_after_leave`).
static HELPER_CHILD_CAPACITY: LazyLock<Mutex<(usize, Arc<Semaphore>)>> =
    LazyLock::new(|| Mutex::new((0, Arc::new(Semaphore::new(1)))));

fn helper_capacity_semaphore(config: &CollabConfig) -> Arc<Semaphore> {
    let cap = config.max_rooms.max(1);
    let mut guard = HELPER_CHILD_CAPACITY.lock().expect("helper capacity");
    if guard.0 != cap {
        guard.0 = cap;
        guard.1 = Arc::new(Semaphore::new(cap));
        config.apply_runtime_limits();
    }
    guard.1.clone()
}

struct HelperChildCapacityHold {
    #[allow(dead_code)]
    permits: Vec<OwnedSemaphorePermit>,
}

impl HelperChildCapacityHold {
    async fn reserve(room_slots: usize, config: &CollabConfig) -> Self {
        let semaphore = helper_capacity_semaphore(config);
        let room_slots = room_slots.min(config.max_rooms);
        let mut permits = Vec::with_capacity(room_slots);
        for _ in 0..room_slots {
            permits.push(
                semaphore
                    .clone()
                    .acquire_owned()
                    .await
                    .expect("helper child capacity"),
            );
        }
        Self { permits }
    }
}

async fn new_test_collab_hub(
    config: CollabConfig,
    pool: PgPool,
    reserved_helpers: usize,
) -> (CollabHub, HelperChildCapacityHold) {
    let capacity = HelperChildCapacityHold::reserve(reserved_helpers, &config).await;
    (CollabHub::new(config, pool), capacity)
}

async fn new_test_collab_hub_arc(
    config: CollabConfig,
    pool: PgPool,
    reserved_helpers: usize,
) -> (Arc<CollabHub>, HelperChildCapacityHold) {
    let (hub, capacity) = new_test_collab_hub(config, pool, reserved_helpers).await;
    (Arc::new(hub), capacity)
}

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
    fvoci_server::db::migrate::apply_app_role_grants(pool, role_name)
        .await
        .expect("grant");
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
    let hash = fixture_password_hash().await;
    sqlx::query(
        "INSERT INTO fvoci.users (id, email, password_hash, given_name) VALUES ($1, $2, $3, $4)",
    )
    .bind(user_id)
    .bind(format!("owner-{user_id}@example.com"))
    .bind(hash)
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
        revoke_poll_ms,
        client_id_ttl_ms: 60_000,
        rpc_timeout_ms: 5_000,
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

struct DirectHubLeases(Vec<ConnectionLease>);

impl DirectHubLeases {
    fn new() -> Self {
        Self(Vec::new())
    }

    fn retain(&mut self, lease: ConnectionLease) {
        self.0.push(lease);
    }
}

async fn hub_join_document(
    hub: &CollabHub,
    wiki: &WikiDocFixture,
    document_id: Uuid,
    client_id: u32,
) -> Result<(Uuid, fvoci_server::collab::room::ConnectionLease), JoinError> {
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
    let lease = hub
        .join_room((wiki.session.workspace_id, document_id), join)
        .await?;
    Ok((conn_id, lease))
}

async fn hub_join(
    leases: &mut DirectHubLeases,
    hub: &CollabHub,
    wiki: &WikiDocFixture,
    client_id: u32,
) -> Result<Uuid, JoinError> {
    let (conn_id, lease) = hub_join_document(hub, wiki, wiki.document_id, client_id).await?;
    leases.retain(lease);
    Ok(conn_id)
}

async fn hub_join_readonly(
    leases: &mut DirectHubLeases,
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
    let lease = hub
        .join_room((wiki.session.workspace_id, wiki.document_id), join)
        .await?;
    leases.retain(lease);
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
    let storage_root =
        std::env::temp_dir().join(format!("fvoci-collab-product-store-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&storage_root).expect("storage root");
    AppState {
        auth: Arc::new(AuthService {
            db: Db::new(pool.clone()),
            password_keys: Keyring::parse(PEPPER, "test").expect("pepper"),
        }),
        branding_name: "FVOCI".to_string(),
        public_origin: PUBLIC_ORIGIN.to_string(),
        cookie_secure: false,
        rate_limiter: RateLimiter::new(),
        storage: fvoci_server::attachments::LocalStorage::new(storage_root),
        upload: fvoci_server::attachments::UploadLimits {
            part_size_bytes: fvoci_server::config::DEFAULT_UPLOAD_PART_SIZE_BYTES,
            max_file_size_bytes: fvoci_server::config::DEFAULT_UPLOAD_MAX_FILE_SIZE_BYTES,
            create_rate_per_5min: fvoci_server::config::DEFAULT_UPLOAD_CREATE_RATE_PER_5MIN,
        },
        collab: Some(Arc::new(CollabHub::new(cfg, pool))),
        meili: None,
        document_convert: None,
        import_settings: None,
        import_queue: fvoci_server::import_job::ImportQueue::new(),
        mailer: std::sync::Arc::new(fvoci_server::mail::Mailer::disabled()),
    }
}

async fn collab_app_state(
    app_url: &str,
    with_collab: bool,
) -> (AppState, Option<HelperChildCapacityHold>) {
    let pool = pool::connect_app(app_url).await.expect("app pool");
    let (collab, helper_capacity) = if with_collab {
        let cfg = test_collab_config(4, 30_000);
        let capacity = HelperChildCapacityHold::reserve(1, &cfg).await;
        (
            Some(Arc::new(CollabHub::new(cfg, pool.clone()))),
            Some(capacity),
        )
    } else {
        (None, None)
    };
    let storage_root =
        std::env::temp_dir().join(format!("fvoci-collab-product-store-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&storage_root).expect("storage root");
    let state = AppState {
        auth: Arc::new(AuthService {
            db: Db::new(pool),
            password_keys: Keyring::parse(PEPPER, "test").expect("pepper"),
        }),
        branding_name: "FVOCI".to_string(),
        public_origin: PUBLIC_ORIGIN.to_string(),
        cookie_secure: false,
        rate_limiter: RateLimiter::new(),
        storage: fvoci_server::attachments::LocalStorage::new(storage_root),
        upload: fvoci_server::attachments::UploadLimits {
            part_size_bytes: fvoci_server::config::DEFAULT_UPLOAD_PART_SIZE_BYTES,
            max_file_size_bytes: fvoci_server::config::DEFAULT_UPLOAD_MAX_FILE_SIZE_BYTES,
            create_rate_per_5min: fvoci_server::config::DEFAULT_UPLOAD_CREATE_RATE_PER_5MIN,
        },
        collab,
        meili: None,
        document_convert: None,
        import_settings: None,
        import_queue: fvoci_server::import_job::ImportQueue::new(),
    }
        mailer: std::sync::Arc::new(fvoci_server::mail::Mailer::disabled()),
    };
    (state, helper_capacity)
}

struct TestServer {
    addr: SocketAddr,
    collab: Option<Arc<CollabHub>>,
    #[allow(dead_code)]
    helper_capacity: Option<HelperChildCapacityHold>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    join: Option<tokio::task::JoinHandle<()>>,
}

impl TestServer {
    async fn start(
        app: Router,
        collab: Option<Arc<CollabHub>>,
        helper_capacity: Option<HelperChildCapacityHold>,
    ) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        let join = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(async {
                shutdown_rx.await.ok();
            })
            .await
            .expect("serve collab_product test server");
        });
        Self {
            addr,
            collab,
            helper_capacity,
            shutdown: Some(shutdown_tx),
            join: Some(join),
        }
    }

    async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(join) = self.join.take() {
            let _ = join.await;
        }
        if let Some(hub) = self.collab.take() {
            hub.shutdown().await;
        }
    }
}

async fn start_test_server(
    state: AppState,
    helper_capacity: Option<HelperChildCapacityHold>,
) -> TestServer {
    let collab = state.collab.clone();
    let app = fvoci_server::http::router(state, None);
    TestServer::start(app, collab, helper_capacity).await
}

async fn start_product_test_server(app_url: &str, with_collab: bool) -> TestServer {
    let (state, helper_capacity) = collab_app_state(app_url, with_collab).await;
    start_test_server(state, helper_capacity).await
}

async fn start_configured_test_server(app_url: &str, cfg: CollabConfig) -> TestServer {
    let helper_capacity = HelperChildCapacityHold::reserve(1, &cfg).await;
    let state = collab_app_state_with_config(app_url, cfg).await;
    start_test_server(state, Some(helper_capacity)).await
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

fn engine_expectations() -> Value {
    serde_json::from_str(include_str!(
        "../crates/collab-engine/fixtures/expectations.json"
    ))
    .expect("pinned engine expectations.json")
}

fn pending_tiptap_u1_json() -> Value {
    engine_expectations()["pending"]["prosemirror_json_u1"].clone()
}

fn pending_tiptap_both_json() -> Value {
    engine_expectations()["pending"]["prosemirror_json_both"].clone()
}

fn project_snapshot_json(snapshot: &[u8]) -> Value {
    let mut session = EngineSession::spawn(SpawnRequest {
        engine_bin: engine_bin(),
        limits: collab_engine::Limits::for_tests(),
        slot_kind: collab_engine::process::ChildSlotKind::Primary,
        slot_wait: None,
        test_hang_ms: None,
        test_exit_after_read: None,
        test_close_stdout_hang_ms: None,
        test_exit_after_write: None,
    })
    .expect("spawn helper to restore snapshot");
    let load = session.call(&Request::Load {
        snapshot_b64: Some(snapshot.to_vec()),
        tail_b64: Vec::new(),
        encoding: 1,
    });
    match load.outcome {
        EngineStatus::Ok { applied: true, .. } => {}
        other => panic!("snapshot load must apply, got {other:?}"),
    }
    match session.call(&Request::Project { encoding: 1 }).outcome {
        EngineStatus::Ok {
            content_json: Some(json),
            ..
        } => json,
        other => panic!("snapshot Project must return Tiptap JSON, got {other:?}"),
    }
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
    Eof,
    WsError(String),
    UnexpectedStateless(String),
    InvalidFrame(String),
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
        match tokio::time::timeout(remaining.min(Duration::from_millis(200)), ws.next()).await {
            Err(_) => continue,
            Ok(None) => return PersistOutcome::Eof,
            Ok(Some(Err(err))) => return PersistOutcome::WsError(err.to_string()),
            Ok(Some(Ok(Message::Close(frame)))) => {
                return PersistOutcome::Closed(frame.map(|close| close.reason.to_string()));
            }
            Ok(Some(Ok(Message::Binary(bytes)))) => {
                match fvoci_server::collab::wire::decode(&bytes) {
                    Ok(WireFrame::Document {
                        message: DocumentMessage::Stateless(body),
                        ..
                    }) => {
                        if body == persisted {
                            return PersistOutcome::Persisted;
                        }
                        if body == failed {
                            return PersistOutcome::Failed(body);
                        }
                        if body.starts_with("persisted:") || body.starts_with("persist-failed:") {
                            return PersistOutcome::UnexpectedStateless(body);
                        }
                    }
                    Ok(WireFrame::Document {
                        message: DocumentMessage::Close { reason },
                        ..
                    }) => return PersistOutcome::Closed(reason),
                    Ok(_) => {}
                    Err(err) => return PersistOutcome::InvalidFrame(err.to_string()),
                }
            }
            Ok(Some(Ok(_))) => {}
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
        PersistOutcome::Eof => panic!(
            "{context}: WebSocket EOF before persist ack for {request_id}"
        ),
        PersistOutcome::WsError(err) => panic!(
            "{context}: WebSocket error before persist ack for {request_id}: {err}"
        ),
        PersistOutcome::UnexpectedStateless(body) => panic!(
            "{context}: persist reply {body} is not exact persisted:{request_id}"
        ),
        PersistOutcome::InvalidFrame(err) => panic!(
            "{context}: undecodable frame before persist ack for {request_id}: {err}"
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
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining.min(Duration::from_millis(200)), ws.next()).await {
            Err(_) => continue,
            Ok(None) => panic!("websocket closed before auth"),
            Ok(Some(Err(err))) => panic!("websocket error before auth: {err}"),
            Ok(Some(Ok(Message::Binary(bytes)))) => {
                let frame = fvoci_server::collab::wire::decode(&bytes).expect("decode");
                assert!(
                    matches!(
                        frame,
                        WireFrame::Document {
                            message: DocumentMessage::Auth(AuthMessage::Authenticated { .. }),
                            ..
                        }
                    ),
                    "expected Authenticated auth frame, got {frame:?}"
                );
                return;
            }
            Ok(Some(Ok(Message::Ping(_)))) | Ok(Some(Ok(Message::Pong(_)))) => {}
            Ok(Some(Ok(Message::Close(_)))) => panic!("websocket closed before auth"),
            Ok(Some(Ok(_))) => {}
        }
    }
    panic!("timed out waiting for auth response");
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
    let server = start_product_test_server(&harness.app_url, false).await;
    let addr = server.addr;
    let response = reqwest::Client::new()
        .get(format!("http://{addr}/collab"))
        .header("origin", PUBLIC_ORIGIN)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 503);
    server.shutdown().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_rejects_missing_origin_on_upgrade() {
    let harness = TestDb::bootstrap().await;
    let server = start_product_test_server(&harness.app_url, true).await;
    let addr = server.addr;
    let request = format!("ws://{addr}/collab").into_client_request().unwrap();
    let err = tokio_tungstenite::connect_async(request).await.unwrap_err();
    assert!(
        err.to_string().contains("403") || err.to_string().contains("Forbidden"),
        "expected origin rejection, got {err}"
    );
    server.shutdown().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_auth_handshake_succeeds_for_member() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    let server = start_product_test_server(&harness.app_url, true).await;
    let addr = server.addr;
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
    server.shutdown().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_nonmember_is_denied() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    let outsider = setup_owner_session(&harness).await;
    let server = start_product_test_server(&harness.app_url, true).await;
    let addr = server.addr;
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
    server.shutdown().await;
    harness.cleanup().await;
}

async fn wait_for_capacity_retry_close_without_auth_denied(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    within: Duration,
) {
    let deadline = tokio::time::Instant::now() + within;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining.min(Duration::from_millis(100)), ws.next()).await {
            Ok(Some(Ok(Message::Close(Some(frame))))) => {
                let code = ws_close_code(&frame);
                assert_eq!(
                    code, 1013,
                    "capacity refusal CloseFrame {code} ({:?}), expected 1013; reason {:?}",
                    frame.code, frame.reason
                );
                return;
            }
            Ok(Some(Ok(Message::Close(None)))) => {
                panic!("Close without code, expected CloseFrame 1013 try again later");
            }
            Ok(Some(Ok(Message::Binary(bytes)))) => {
                if let Ok(WireFrame::Document {
                    message: DocumentMessage::Auth(AuthMessage::PermissionDenied { reason }),
                    ..
                }) = fvoci_server::collab::wire::decode(&bytes)
                {
                    panic!(
                        "capacity refusal must not send PermissionDenied ({reason}); expected Close 1013"
                    );
                }
                if let Ok(WireFrame::Document {
                    message: DocumentMessage::Auth(AuthMessage::Authenticated { scope }),
                    ..
                }) = fvoci_server::collab::wire::decode(&bytes)
                {
                    panic!("capacity refusal must not authenticate (scope={scope})");
                }
            }
            Ok(Some(Ok(_))) => {}
            Ok(None) => {
                panic!("bare TCP EOF without CloseFrame, expected close code 1013");
            }
            Ok(Some(Err(err))) => {
                panic!("websocket error before CloseFrame 1013: {err}");
            }
            Err(_) => {}
        }
    }
    panic!("did not receive CloseFrame 1013 within {within:?}");
}

async fn wait_for_unavailable_close_without_auth_denied(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    within: Duration,
) {
    let deadline = tokio::time::Instant::now() + within;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining.min(Duration::from_millis(100)), ws.next()).await {
            Ok(Some(Ok(Message::Close(Some(frame))))) => {
                let code = ws_close_code(&frame);
                assert_eq!(
                    code, 1011,
                    "operational join failure CloseFrame {code} ({:?}), expected 1011; reason {:?}",
                    frame.code, frame.reason
                );
                assert!(
                    frame.reason.to_string().contains("collab unavailable"),
                    "Close 1011 reason must be collab unavailable, got {:?}",
                    frame.reason
                );
                return;
            }
            Ok(Some(Ok(Message::Close(None)))) => {
                panic!("Close without code, expected CloseFrame 1011 collab unavailable");
            }
            Ok(Some(Ok(Message::Binary(bytes)))) => {
                if let Ok(WireFrame::Document {
                    message: DocumentMessage::Auth(AuthMessage::PermissionDenied { reason }),
                    ..
                }) = fvoci_server::collab::wire::decode(&bytes)
                {
                    panic!(
                        "operational join failure must not send PermissionDenied ({reason}); expected Close 1011"
                    );
                }
                if let Ok(WireFrame::Document {
                    message: DocumentMessage::Auth(AuthMessage::Authenticated { scope }),
                    ..
                }) = fvoci_server::collab::wire::decode(&bytes)
                {
                    panic!("operational join failure must not authenticate (scope={scope})");
                }
            }
            Ok(Some(Ok(_))) => {}
            Ok(None) => {
                panic!("bare TCP EOF without CloseFrame, expected close code 1011");
            }
            Ok(Some(Err(err))) => {
                panic!("websocket error before CloseFrame 1011: {err}");
            }
            Err(_) => {}
        }
    }
    panic!("did not receive CloseFrame 1011 within {within:?}");
}

#[tokio::test]
async fn collab_ws_writer_stale_lock_closes_1011_then_join_after_release() {
    run_lifecycle_test(
        "collab_ws_writer_stale_lock_closes_1011_then_join_after_release",
        async {
            let harness = TestDb::bootstrap().await;
            let wiki = setup_wiki_doc(&harness).await;
            let held = RoomGuard::try_acquire(&wiki.session.pool, wiki.document_id)
                .await
                .expect("db")
                .expect("room lock should be free");
            let server = start_product_test_server(&harness.app_url, true).await;
            let addr = server.addr;
            let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
            let mut ws = connect_member(addr, &wiki.session.session_token).await;
            ws.send(Message::Binary(auth_token_frame(&routing_key, 31).into()))
                .await
                .unwrap();
            wait_for_unavailable_close_without_auth_denied(&mut ws, Duration::from_secs(5)).await;

            held.release().await;
            let mut recovered = connect_member(addr, &wiki.session.session_token).await;
            auth_and_join(&mut recovered, &routing_key, 32).await;
            server.shutdown().await;
            harness.cleanup().await;
        },
    )
    .await;
}

#[tokio::test]
async fn collab_ws_pending_room_writer_stale_closes_1011() {
    run_lifecycle_test("collab_ws_pending_room_writer_stale_closes_1011", async {
        let harness = TestDb::bootstrap().await;
        let wiki = setup_wiki_doc(&harness).await;
        let held = RoomGuard::try_acquire(&wiki.session.pool, wiki.document_id)
            .await
            .expect("db")
            .expect("room lock should be free");
        let server = start_product_test_server(&harness.app_url, true).await;
        let addr = server.addr;
        let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
        let mut ws = connect_member(addr, &wiki.session.session_token).await;
        ws.send(Message::Binary(
            sync_step1_frame(&routing_key, &[0, 0]).into(),
        ))
        .await
        .unwrap();
        ws.send(Message::Binary(auth_token_frame(&routing_key, 33).into()))
            .await
            .unwrap();
        wait_for_unavailable_close_without_auth_denied(&mut ws, Duration::from_secs(5)).await;
        held.release().await;
        server.shutdown().await;
        harness.cleanup().await;
    })
    .await;
}

#[tokio::test]
async fn collab_ws_missing_helper_closes_1011_then_valid_join() {
    run_lifecycle_test(
        "collab_ws_missing_helper_closes_1011_then_valid_join",
        async {
            let harness = TestDb::bootstrap().await;
            let wiki = setup_wiki_doc(&harness).await;
            let other = setup_wiki_doc(&harness).await;
            let missing =
                std::env::temp_dir().join(format!("fvoci-f7-missing-engine-{}", Uuid::now_v7()));
            let mut cfg = test_collab_config(4, 30_000);
            cfg.engine_bin = missing;
            let server = start_configured_test_server(&harness.app_url, cfg).await;
            let addr = server.addr;
            let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
            let mut ws = connect_member(addr, &wiki.session.session_token).await;
            ws.send(Message::Binary(auth_token_frame(&routing_key, 41).into()))
                .await
                .unwrap();
            wait_for_unavailable_close_without_auth_denied(&mut ws, Duration::from_secs(5)).await;

            let healthy_server = start_product_test_server(&harness.app_url, true).await;
            let healthy_addr = healthy_server.addr;
            let other_key = room_key(other.session.workspace_id, other.document_id);
            let mut recovered = connect_member(healthy_addr, &other.session.session_token).await;
            auth_and_join(&mut recovered, &other_key, 42).await;
            server.shutdown().await;
            healthy_server.shutdown().await;
            harness.cleanup().await;
        },
    )
    .await;
}

#[tokio::test]
async fn collab_ws_room_full_closes_1013_without_auth_denied() {
    run_lifecycle_test(
        "collab_ws_room_full_closes_1013_without_auth_denied",
        async {
            let harness = TestDb::bootstrap().await;
            let docs = setup_wiki_doc_batch(&harness, 2).await;
            let mut cfg = test_collab_config(1, 30_000);
            cfg.max_collab_sockets = 8;
            let server = start_configured_test_server(&harness.app_url, cfg).await;
            let addr = server.addr;
            let first_key = room_key(docs[0].session.workspace_id, docs[0].document_id);
            let mut first = connect_member(addr, &docs[0].session.session_token).await;
            auth_and_join(&mut first, &first_key, 51).await;
            let second_key = room_key(docs[1].session.workspace_id, docs[1].document_id);
            let mut second = connect_member(addr, &docs[1].session.session_token).await;
            second
                .send(Message::Binary(auth_token_frame(&second_key, 52).into()))
                .await
                .unwrap();
            wait_for_capacity_retry_close_without_auth_denied(&mut second, Duration::from_secs(5))
                .await;
            drop(first);
            server.shutdown().await;
            harness.cleanup().await;
        },
    )
    .await;
}

#[tokio::test]
async fn collab_ws_true_access_denial_stays_auth_frame() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    let outsider = setup_owner_session(&harness).await;
    let server = start_product_test_server(&harness.app_url, true).await;
    let addr = server.addr;
    let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
    let mut ws = connect_member(addr, &outsider.session_token).await;
    ws.send(Message::Binary(auth_token_frame(&routing_key, 7).into()))
        .await
        .unwrap();
    let frame = recv_document_frame(&mut ws, 4).await.expect("denial frame");
    match frame {
        WireFrame::Document {
            message: DocumentMessage::Auth(AuthMessage::PermissionDenied { reason }),
            ..
        } => assert_eq!(reason, "not found"),
        other => panic!("true access denial must be PermissionDenied, got {other:?}"),
    }
    match tokio::time::timeout(Duration::from_millis(200), ws.next()).await {
        Ok(Some(Ok(Message::Close(Some(frame))))) => {
            panic!(
                "true access denial must not Close {}, expected the socket to stay for retry",
                ws_close_code(&frame)
            );
        }
        Ok(Some(Ok(Message::Close(None)))) | Ok(None) => {
            panic!("true access denial must not close the socket");
        }
        _ => {}
    }
    server.shutdown().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_ws_unsupported_kind_stays_auth_frame() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    let server = start_product_test_server(&harness.app_url, true).await;
    let addr = server.addr;
    let routing_key = CollabRoomName {
        workspace_id: wiki.session.workspace_id,
        kind: CollabKind::Task,
        resource_id: wiki.document_id,
    }
    .routing_key();
    let mut ws = connect_member(addr, &wiki.session.session_token).await;
    ws.send(Message::Binary(auth_token_frame(&routing_key, 8).into()))
        .await
        .unwrap();
    let frame = recv_document_frame(&mut ws, 4)
        .await
        .expect("unsupported kind frame");
    match frame {
        WireFrame::Document {
            message: DocumentMessage::Auth(AuthMessage::PermissionDenied { reason }),
            ..
        } => assert_eq!(reason, "unsupported kind"),
        other => panic!("unsupported kind must be PermissionDenied, got {other:?}"),
    }
    server.shutdown().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_two_clients_update_persists_and_broadcasts() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    let server = start_product_test_server(&harness.app_url, true).await;
    let addr = server.addr;
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
    server.shutdown().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_concurrent_first_joins_both_succeed() {
    run_lifecycle_test("collab_concurrent_first_joins_both_succeed", async {
        let harness = TestDb::bootstrap().await;
        let wiki = setup_wiki_doc(&harness).await;
        let (hub, _helper_capacity) =
            new_test_collab_hub_arc(test_collab_config(4, 30_000), wiki.session.pool.clone(), 1)
                .await;
        let key = (wiki.session.workspace_id, wiki.document_id);
        let slots_before = hub.available_room_slots();
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let mut leases = DirectHubLeases::new();

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
                    hub_join_document(&hub_a, &wiki_a, wiki_a.document_id, 1).await
                },
                async move {
                    barrier_b.wait().await;
                    hub_join_document(&hub_b, &wiki_b, wiki_b.document_id, 2).await
                }
            )
        })
        .await
        .expect("concurrent first join hung waiting on room lifecycle notify");

        assert!(first.is_ok(), "first join failed: {:?}", first.err());
        assert!(second.is_ok(), "second join failed: {:?}", second.err());
        leases.retain(first.unwrap().1);
        leases.retain(second.unwrap().1);
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
        let (hub, _helper_capacity) =
            new_test_collab_hub(test_collab_config(4, 30_000), wiki.session.pool.clone(), 1).await;
        let mut leases = DirectHubLeases::new();
        assert_eq!(hub.available_room_slots(), 4);
        let failed = hub_join(&mut leases, &hub, &wiki, 1).await;
        assert!(matches!(failed, Err(JoinError::WriterStale)));
        assert_eq!(hub.available_room_slots(), 4);
        held.release().await;
        assert!(hub_join(&mut leases, &hub, &wiki, 2).await.is_ok());
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
        let (hub, _helper_capacity) =
            new_test_collab_hub_arc(test_collab_config(4, 30_000), wiki.session.pool.clone(), 1)
                .await;
        let join_task = tokio::spawn({
            let hub = hub.clone();
            let wiki = wiki.clone_fixture();
            async move { hub_join_document(&hub, &wiki, wiki.document_id, 1).await }
        });
        hub.shutdown().await;
        let result = join_task.await.expect("join task").map(|(id, _)| id);
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
            let (hub, _helper_capacity) =
                new_test_collab_hub(test_collab_config(4, 30_000), wiki.session.pool.clone(), 1)
                    .await;
            let mut leases = DirectHubLeases::new();
            assert_eq!(hub.available_room_slots(), 4);

            for index in 0..4 {
                let fake_doc = Uuid::now_v7();
                let denied = hub_join_document(&hub, &wiki, fake_doc, index).await;
                assert!(matches!(denied, Err(JoinError::AdmissionDenied)));
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
            assert!(matches!(outsider_denied, Err(JoinError::AdmissionDenied)));
            assert_eq!(hub.available_room_slots(), 4);
            assert!(hub_join(&mut leases, &hub, &wiki, 1).await.is_ok());
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
            let (hub, _helper_capacity) = new_test_collab_hub_arc(
                test_collab_config(4, 30_000),
                wiki.session.pool.clone(),
                1,
            )
            .await;
            let key = (wiki.session.workspace_id, wiki.document_id);
            let slots_before = hub.available_room_slots();
            let mut leases = DirectHubLeases::new();

            let join_a = tokio::spawn({
                let hub = hub.clone();
                let wiki = wiki.clone_fixture();
                async move { hub_join_document(&hub, &wiki, wiki.document_id, 1).await }
            });
            wait_for_booting(&hub, key).await;
            assert_eq!(hub.available_room_slots(), slots_before - 1);

            let join_b = tokio::spawn({
                let hub = hub.clone();
                let wiki = wiki.clone_fixture();
                async move { hub_join_document(&hub, &wiki, wiki.document_id, 2).await }
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
            let (conn_a, lease_a) = join_a.await.expect("join a task").expect("join a");
            leases.retain(lease_a);
            let (conn_b, lease_b) = join_b.await.expect("join b task").expect("join b");
            leases.retain(lease_b);
            assert_ne!(conn_a, conn_b);
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
            let (hub, _helper_capacity) =
                new_test_collab_hub_arc(test_collab_config(4, 200), wiki.session.pool.clone(), 1)
                    .await;
            let key = (wiki.session.workspace_id, wiki.document_id);
            let join_task = tokio::spawn({
                let hub = hub.clone();
                let wiki = wiki.clone_fixture();
                async move { hub_join_document(&hub, &wiki, wiki.document_id, 1).await }
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
            let mut leases = DirectHubLeases::new();
            assert!(hub_join(&mut leases, &hub, &wiki, 2).await.is_ok());
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
        let (hub, _helper_capacity) =
            new_test_collab_hub(test_collab_config(4, 200), docs[0].session.pool.clone(), 4).await;
        let mut leases = DirectHubLeases::new();

        let mut conn_ids = Vec::new();
        for doc in docs.iter().take(4) {
            conn_ids.push(
                hub_join(&mut leases, &hub, doc, 1)
                    .await
                    .expect("join room"),
            );
        }
        assert_eq!(hub.available_room_slots(), 0);
        let fifth = hub_join(&mut leases, &hub, &docs[4], 1).await;
        assert!(matches!(fifth, Err(JoinError::RoomFull)));

        let key = (docs[0].session.workspace_id, docs[0].document_id);
        hub.leave_room(key, conn_ids[0]).await;
        wait_for_phase(&hub, key, RoomLifecyclePhase::Absent).await;
        assert_eq!(hub.available_room_slots(), 1);
        assert!(hub_join(&mut leases, &hub, &docs[4], 2).await.is_ok());
        hub.shutdown().await;
        harness.cleanup().await;
    })
    .await;
}

#[tokio::test]
async fn collab_memory_budget_refusal_frees_room_slot() {
    run_lifecycle_test("collab_memory_budget_refusal_frees_room_slot", async {
        let harness = TestDb::bootstrap().await;
        let wiki = setup_wiki_doc(&harness).await;
        let mut cfg = test_collab_config(4, 200);
        cfg.memory_budget_bytes = 1;
        let (hub, _helper_capacity) = new_test_collab_hub(cfg, wiki.session.pool.clone(), 1).await;
        let mut leases = DirectHubLeases::new();
        let denied = hub_join(&mut leases, &hub, &wiki, 1).await;
        assert!(matches!(denied, Err(JoinError::CapacityRetry)));
        assert_eq!(hub.available_room_slots(), 4);
        hub.shutdown().await;
        harness.cleanup().await;
    })
    .await;
}

struct GuestCollabSession {
    session_token: String,
    group_id: Uuid,
}

async fn admin_seed_document_state_bytes(
    admin_url: &str,
    workspace_id: Uuid,
    document_id: Uuid,
    state_len: usize,
) {
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(admin_url)
        .await
        .unwrap();
    let state = vec![0xABu8; state_len];
    sqlx::query(
        r#"
        INSERT INTO fvoci.document_states (
            workspace_id, document_id, state, writer_generation, snapshot_cutoff_seq, tail_seq
        ) VALUES ($1, $2, $3, 1, 0, 0)
        ON CONFLICT (workspace_id, document_id) DO UPDATE
        SET state = EXCLUDED.state
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .bind(state)
    .execute(&admin)
    .await
    .unwrap();
    admin.close().await;
}

async fn setup_guest_with_wiki_group_edit(
    harness: &TestDb,
    wiki: &WikiDocFixture,
) -> GuestCollabSession {
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    let guest_id = Uuid::now_v7();
    sqlx::query("INSERT INTO fvoci.users (id, email, given_name) VALUES ($1, $2, $3)")
        .bind(guest_id)
        .bind(format!("guest-{guest_id}@example.com"))
        .bind("Guest")
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO fvoci.memberships (workspace_id, user_id, role) VALUES ($1, $2, 'guest')",
    )
    .bind(wiki.session.workspace_id)
    .bind(guest_id)
    .execute(&admin)
    .await
    .unwrap();
    let group_id = Uuid::now_v7();
    sqlx::query("INSERT INTO fvoci.groups (id, workspace_id, name) VALUES ($1, $2, $3)")
        .bind(group_id)
        .bind(wiki.session.workspace_id)
        .bind("wiki-editors")
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO fvoci.group_members (workspace_id, group_id, user_id) VALUES ($1, $2, $3)",
    )
    .bind(wiki.session.workspace_id)
    .bind(group_id)
    .bind(guest_id)
    .execute(&admin)
    .await
    .unwrap();
    let member_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO fvoci.document_members (id, workspace_id, document_id, group_id, role)
        VALUES ($1, $2, $3, $4, 'member')
        "#,
    )
    .bind(member_id)
    .bind(wiki.session.workspace_id)
    .bind(wiki.document_id)
    .bind(group_id)
    .execute(&admin)
    .await
    .unwrap();
    let token = new_token();
    let session_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO fvoci.sessions (id, user_id, token_hash, expires_at) VALUES ($1, $2, $3, now() + interval '1 hour')",
    )
    .bind(session_id)
    .bind(guest_id)
    .bind(&token.hash)
    .execute(&admin)
    .await
    .unwrap();
    admin.close().await;
    GuestCollabSession {
        session_token: token.token,
        group_id,
    }
}

fn huge_varint_memory_candidate() -> Vec<u8> {
    vec![0xff, 0xff, 0xff, 0xff, 0x0f]
}

#[tokio::test]
async fn collab_guest_group_grant_can_append_and_revoke_rejects() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    let guest = setup_guest_with_wiki_group_edit(&harness, &wiki).await;
    let server = start_product_test_server(&harness.app_url, true).await;
    let addr = server.addr;
    let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
    let update = sample_hi_update();

    let mut writer = connect_member(addr, &guest.session_token).await;
    auth_and_join(&mut writer, &routing_key, 701).await;
    writer
        .send(Message::Binary(
            sync_update_frame(&routing_key, &update).into(),
        ))
        .await
        .unwrap();
    assert!(
        wait_for_sync_applied(&mut writer, Duration::from_secs(5)).await,
        "guest with group edit grant must append through collab"
    );

    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&harness.admin_url)
        .await
        .unwrap();
    sqlx::query(
        "DELETE FROM fvoci.document_members WHERE workspace_id = $1 AND document_id = $2 AND group_id = $3",
    )
    .bind(wiki.session.workspace_id)
    .bind(wiki.document_id)
    .bind(guest.group_id)
    .execute(&admin)
    .await
    .unwrap();
    admin.close().await;

    writer
        .send(Message::Binary(
            sync_update_frame(&routing_key, &update).into(),
        ))
        .await
        .unwrap();
    wait_for_ws_close_code(&mut writer, 1008, Duration::from_secs(5), true).await;

    server.shutdown().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_persisted_estimate_nonzero_under_rls() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    admin_seed_document_state_bytes(
        &harness.admin_url,
        wiki.session.workspace_id,
        wiki.document_id,
        4096,
    )
    .await;
    let mut conn = wiki.session.pool.acquire().await.unwrap();
    let estimate =
        estimate_persisted_collab_bytes(&mut conn, wiki.session.workspace_id, wiki.document_id)
            .await
            .expect("estimate");
    assert!(
        estimate >= 4096,
        "RLS-aware estimate must include seeded snapshot bytes, got {estimate}"
    );
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_memory_budget_uses_persisted_factor_not_floor_only() {
    run_lifecycle_test(
        "collab_memory_budget_uses_persisted_factor_not_floor_only",
        async {
            let harness = TestDb::bootstrap().await;
            let wiki = setup_wiki_doc(&harness).await;
            let persisted = 2 * 1024 * 1024;
            admin_seed_document_state_bytes(
                &harness.admin_url,
                wiki.session.workspace_id,
                wiki.document_id,
                persisted,
            )
            .await;
            let reservation = room_memory_reservation_bytes(persisted as u64);
            assert!(
                reservation > collab_engine::limits::MIN_ROOM_MEMORY_RESERVATION_BYTES,
                "2 MiB persisted must exceed the 16 MiB floor via the 14× factor"
            );
            let mut tight_cfg = test_collab_config(4, 200);
            tight_cfg.memory_budget_bytes = 20 * 1024 * 1024;
            let tight_budget = tight_cfg.memory_budget_bytes;
            let (hub, _helper_capacity) =
                new_test_collab_hub(tight_cfg, wiki.session.pool.clone(), 1).await;
            let mut leases = DirectHubLeases::new();
            let denied = hub_join(&mut leases, &hub, &wiki, 1).await;
            assert!(matches!(denied, Err(JoinError::CapacityRetry)));
            assert_eq!(hub.available_room_slots(), 4);
            assert!(
            collab_engine::limits::MIN_ROOM_MEMORY_RESERVATION_BYTES < tight_budget,
            "20 MiB budget would admit the 16 MiB floor alone; denial must come from 14× persisted"
        );
            hub.shutdown().await;
            harness.cleanup().await;
        },
    )
    .await;
}

#[tokio::test]
async fn collab_estimate_fail_frees_room_slot() {
    run_lifecycle_test("collab_estimate_fail_frees_room_slot", async {
        let harness = TestDb::bootstrap().await;
        let wiki = setup_wiki_doc(&harness).await;
        let (hub, _helper_capacity) =
            new_test_collab_hub(test_collab_config(4, 200), wiki.session.pool.clone(), 1).await;
        arm_force_estimate_fail();
        let mut leases = DirectHubLeases::new();
        let denied = hub_join(&mut leases, &hub, &wiki, 1).await;
        disarm_force_estimate_fail();
        assert!(matches!(denied, Err(JoinError::DbError)));
        assert_eq!(hub.available_room_slots(), 4);
        hub.shutdown().await;
        harness.cleanup().await;
    })
    .await;
}

#[tokio::test]
async fn collab_user_reject_budget_survives_reconnect() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    let server = start_product_test_server(&harness.app_url, true).await;
    let addr = server.addr;
    let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
    let hostile = invalid_utf8_update_candidate();

    for client_id in 1..=8u32 {
        let mut writer = connect_member(addr, &wiki.session.session_token).await;
        auth_and_join(&mut writer, &routing_key, client_id).await;
        writer
            .send(Message::Binary(
                sync_update_frame(&routing_key, &hostile).into(),
            ))
            .await
            .unwrap();
        wait_for_ws_close_code(&mut writer, 1008, Duration::from_secs(5), true).await;
    }

    // Budget exhausted: even a valid update from the same user is refused
    // (without the budget it would apply), across a fresh connection.
    let mut writer = connect_member(addr, &wiki.session.session_token).await;
    auth_and_join(&mut writer, &routing_key, 709).await;
    writer
        .send(Message::Binary(
            sync_update_frame(&routing_key, &sample_hi_update()).into(),
        ))
        .await
        .unwrap();
    wait_for_ws_close_code(&mut writer, 1008, Duration::from_secs(5), false).await;

    // Another user in the same room keeps writing.
    let other = setup_second_member(&harness, &wiki).await;
    let mut peer = connect_member(addr, &other.session_token).await;
    auth_and_join(&mut peer, &routing_key, 710).await;
    peer.send(Message::Binary(
        sync_update_frame(&routing_key, &sample_hi_update()).into(),
    ))
    .await
    .unwrap();
    assert!(
        wait_for_sync_applied(&mut peer, Duration::from_secs(5)).await,
        "a different user in the same room is not blocked by another user's budget"
    );

    server.shutdown().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_primary_huge_varint_memory_rejected_1008() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    let server = start_product_test_server(&harness.app_url, true).await;
    let addr = server.addr;
    let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
    let poison = huge_varint_memory_candidate();
    let valid = sample_hi_update();

    let mut writer = connect_member(addr, &wiki.session.session_token).await;
    auth_and_join(&mut writer, &routing_key, 801).await;
    let mut peer = connect_member(addr, &wiki.session.session_token).await;
    auth_and_join(&mut peer, &routing_key, 802).await;

    let load_before = load_collab_document(
        &wiki.session.pool,
        wiki.session.workspace_id,
        wiki.session.user_id,
        wiki.session.session_id,
        wiki.document_id,
    )
    .await
    .unwrap()
    .unwrap();

    writer
        .send(Message::Binary(
            sync_update_frame(&routing_key, &poison).into(),
        ))
        .await
        .unwrap();
    wait_for_ws_close_code(&mut writer, 1008, Duration::from_secs(5), true).await;

    let load_after = load_collab_document(
        &wiki.session.pool,
        wiki.session.workspace_id,
        wiki.session.user_id,
        wiki.session.session_id,
        wiki.document_id,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(load_after.snapshot, load_before.snapshot);
    assert_eq!(load_after.tail_seq, load_before.tail_seq);

    let mut recovery = connect_member(addr, &wiki.session.session_token).await;
    auth_and_join(&mut recovery, &routing_key, 803).await;
    recovery
        .send(Message::Binary(
            sync_update_frame(&routing_key, &valid).into(),
        ))
        .await
        .unwrap();
    assert!(
        wait_for_sync_applied(&mut recovery, Duration::from_secs(5)).await,
        "room must keep editing after primary reload rejects huge-varint candidate"
    );

    server.shutdown().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_lifecycle_idle_eviction_allows_rejoin() {
    run_lifecycle_test("collab_lifecycle_idle_eviction_allows_rejoin", async {
        let harness = TestDb::bootstrap().await;
        let wiki = setup_wiki_doc(&harness).await;
        let (hub, _helper_capacity) =
            new_test_collab_hub(test_collab_config(4, 200), wiki.session.pool.clone(), 1).await;
        let key = (wiki.session.workspace_id, wiki.document_id);
        let mut leases = DirectHubLeases::new();
        let conn_id = hub_join(&mut leases, &hub, &wiki, 1).await.expect("join");
        hub.leave_room(key, conn_id).await;
        wait_for_phase(&hub, key, RoomLifecyclePhase::Absent).await;
        assert!(!hub.room_occupies_slot(key).await);
        assert_eq!(hub.available_room_slots(), 4);
        assert!(hub_join(&mut leases, &hub, &wiki, 2).await.is_ok());
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

    let server = start_product_test_server(&harness.app_url, true).await;
    let addr = server.addr;
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
    server.shutdown().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_reconnect_step1_includes_server_state_vector() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    let server = start_product_test_server(&harness.app_url, true).await;
    let addr = server.addr;
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
    server.shutdown().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_empty_byte_update_is_rejected() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    let server = start_product_test_server(&harness.app_url, true).await;
    let addr = server.addr;
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
    server.shutdown().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_canonical_noop_update_is_not_stored() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    let server = start_product_test_server(&harness.app_url, true).await;
    let addr = server.addr;
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
    server.shutdown().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_persist_barrier_and_id_correlation() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    let server = start_product_test_server(&harness.app_url, true).await;
    let addr = server.addr;
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

    let after_first = load_collab_document(
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
        after_first.tail.is_empty(),
        "first persist should compact tail"
    );
    assert_eq!(
        project_snapshot_json(&after_first.snapshot),
        pending_tiptap_u1_json(),
        "first persist snapshot must project pending u1 text one"
    );

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
    assert_eq!(
        project_snapshot_json(&load.snapshot),
        pending_tiptap_both_json(),
        "compacted snapshot must project pending u1 one and u2 two한글"
    );
    server.shutdown().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_primary_recycles_after_op_cap_then_edits_persist() {
    tokio::time::timeout(OP_CAP_TEST_TIMEOUT, async {
        let harness = TestDb::bootstrap().await;
        let wiki = setup_wiki_doc(&harness).await;
        let server = start_product_test_server(&harness.app_url, true).await;
        let addr = server.addr;
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
        assert_eq!(
            project_snapshot_json(&load.snapshot),
            pending_tiptap_u1_json(),
            "recycled persist snapshot must project pending u1 text one"
        );
    server.shutdown().await;
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

    let server = start_product_test_server(&harness.app_url, true).await;
    let addr = server.addr;
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
    server.shutdown().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_delete_only_round_trip_persists() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    let server = start_product_test_server(&harness.app_url, true).await;
    let addr = server.addr;
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
    server.shutdown().await;
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
    let hash = fixture_password_hash().await;
    sqlx::query(
        "INSERT INTO fvoci.users (id, email, password_hash, given_name) VALUES ($1, $2, $3, $4)",
    )
    .bind(user_id)
    .bind(format!("member-{user_id}@example.com"))
    .bind(hash)
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
    let server = start_product_test_server(&harness.app_url, true).await;
    let addr = server.addr;
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
    server.shutdown().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_append_in_tx_reject_barrier_rejects_writer_not_room() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    let server = start_product_test_server(&harness.app_url, true).await;
    let addr = server.addr;
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
    server.shutdown().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_malformed_step1_closes_offender_healthy_peer_syncs() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    let server = start_product_test_server(&harness.app_url, true).await;
    let addr = server.addr;
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
    server.shutdown().await;
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

        let (hub, _helper_capacity) =
            new_test_collab_hub(test_collab_config(4, 30_000), wiki.session.pool.clone(), 1).await;
        let key = (wiki.session.workspace_id, wiki.document_id);
        let mut leases = DirectHubLeases::new();
        assert!(hub_join_readonly(&mut leases, &hub, &wiki, 81)
            .await
            .is_ok());
        assert!(hub_join_readonly(&mut leases, &hub, &wiki, 82)
            .await
            .is_ok());

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

        let conn_id = hub_join(&mut leases, &hub, &wiki, 83)
            .await
            .expect("writer join");
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
    let server = start_product_test_server(&harness.app_url, true).await;
    let addr = server.addr;
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
    server.shutdown().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_reload_failure_after_commit_preserves_durable_tail() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    let server = start_product_test_server(&harness.app_url, true).await;
    let addr = server.addr;
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
    server.shutdown().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_lifecycle_foreign_leave_does_not_evict_member() {
    run_lifecycle_test(
        "collab_lifecycle_foreign_leave_does_not_evict_member",
        async {
            let harness = TestDb::bootstrap().await;
            let wiki = setup_wiki_doc(&harness).await;
            let (hub, _helper_capacity) =
                new_test_collab_hub(test_collab_config(4, 200), wiki.session.pool.clone(), 1).await;
            let key = (wiki.session.workspace_id, wiki.document_id);
            let mut leases = DirectHubLeases::new();
            let conn_id = hub_join(&mut leases, &hub, &wiki, 1).await.expect("join");
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
            let (hub, _helper_capacity) = new_test_collab_hub_arc(
                test_collab_config(4, 30_000),
                wiki.session.pool.clone(),
                1,
            )
            .await;
            let key = (wiki.session.workspace_id, wiki.document_id);
            let slots_before = hub.available_room_slots();

            let join_task = tokio::spawn({
                let hub = hub.clone();
                let wiki = wiki.clone_fixture();
                async move { hub_join_document(&hub, &wiki, wiki.document_id, 1).await }
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

    let server = start_product_test_server(&harness.app_url, true).await;
    let addr = server.addr;
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
    server.shutdown().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_revoked_session_closes_without_post_revoke_broadcast() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    let cfg = test_collab_config_with_revoke(4, 30_000, 30_000);
    let server = start_configured_test_server(&harness.app_url, cfg).await;
    let addr = server.addr;
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
    server.shutdown().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_outbound_queue_saturation_closes_slow_peer() {
    run_lifecycle_test("collab_outbound_queue_saturation", async {
        let harness = TestDb::bootstrap().await;
        let wiki = setup_wiki_doc(&harness).await;
        let (hub, _helper_capacity) =
            new_test_collab_hub(test_collab_config(4, 30_000), wiki.session.pool.clone(), 1).await;
        let key = (wiki.session.workspace_id, wiki.document_id);
        let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
        let user_id = wiki.session.user_id.to_string();

        let (slow_tx, _slow_rx) = mpsc::channel(2);
        let (cancel_tx, cancel_rx) = watch::channel(None);
        let slow_id = Uuid::now_v7();
        let _lease = hub
            .join_room(
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
        let _lease = hub
            .join_room(
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
        let _lease = hub
            .join_room(
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
        let (hub, _helper_capacity) =
            new_test_collab_hub(test_collab_config(4, 30_000), wiki.session.pool.clone(), 1).await;
        let key = (wiki.session.workspace_id, wiki.document_id);
        let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
        let user_id = wiki.session.user_id.to_string();

        let (old_tx, mut old_rx) = mpsc::channel(8);
        let old_id = Uuid::now_v7();
        let _lease = hub
            .join_room(
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
        let _lease = hub
            .join_room(
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
        let (hub, _helper_capacity) = new_test_collab_hub(cfg, wiki.session.pool.clone(), 1).await;
        let key = (wiki.session.workspace_id, wiki.document_id);
        let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
        let peer = setup_second_member(&harness, &wiki).await;

        let (owner_tx, mut owner_rx) = mpsc::channel(8);
        let owner_id = Uuid::now_v7();
        let _lease = hub
            .join_room(
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
        let _lease = hub
            .join_room(
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
        let (hub, _helper_capacity) =
            new_test_collab_hub(test_collab_config(4, 30_000), wiki.session.pool.clone(), 1).await;
        let key = (wiki.session.workspace_id, wiki.document_id);
        let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
        let user_id = wiki.session.user_id.to_string();

        let (first_tx, mut first_rx) = mpsc::channel(8);
        let first_id = Uuid::now_v7();
        let _lease = hub
            .join_room(
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
        let _lease = hub
            .join_room(
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
    let server = start_configured_test_server(&harness.app_url, cfg).await;
    let addr = server.addr;
    let mut ws = connect_member(addr, &wiki.session.session_token).await;
    assert!(
        wait_for_ws_close(&mut ws, Duration::from_millis(1_500)).await,
        "idle unauthenticated socket must close after auth_wait deadline"
    );
    server.shutdown().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_socket_cap_rejects_excess_and_releases() {
    run_lifecycle_test("collab_socket_cap", async {
        let harness = TestDb::bootstrap().await;
        let wiki = setup_wiki_doc(&harness).await;
        let mut cfg = test_collab_config(4, 30_000);
        cfg.max_collab_sockets = 1;
        let (hub, _hub_capacity) =
            new_test_collab_hub(cfg.clone(), wiki.session.pool.clone(), 1).await;
        assert_eq!(hub.available_collab_sockets(), 1);
        let held = hub
            .try_acquire_socket(wiki.session.session_id)
            .expect("first permit");
        assert_eq!(hub.available_collab_sockets(), 0);
        assert!(hub.try_acquire_socket(wiki.session.session_id).is_none());
        drop(held);
        assert_eq!(hub.available_collab_sockets(), 1);

        let server = start_configured_test_server(&harness.app_url, cfg).await;
        let addr = server.addr;
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
        server.shutdown().await;
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
        let server = start_configured_test_server(&harness.app_url, cfg).await;
        let addr = server.addr;
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
        server.shutdown().await;
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
    let server = start_configured_test_server(&harness.app_url, cfg).await;
    let addr = server.addr;
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
    server.shutdown().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_pre_auth_exhaustion_does_not_swallow_authenticated() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    let mut cfg = test_collab_config(4, 30_000);
    cfg.max_pre_auth_outbound_frames = 1;
    cfg.auth_wait_ms = 5_000;
    let server = start_configured_test_server(&harness.app_url, cfg).await;
    let addr = server.addr;
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
    server.shutdown().await;
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
    let project_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO fvoci.projects (
            id, workspace_id, key, name, visibility, status, next_number, created_by
        ) VALUES ($1, $2, 'COL', 'Project collab lock', 'private', 'active', 1, $3)
        "#,
    )
    .bind(project_id)
    .bind(ws)
    .bind(user)
    .execute(&admin)
    .await
    .unwrap();
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
    .bind(project_id)
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
    let server = start_product_test_server(&harness.app_url, true).await;
    let addr = server.addr;
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
    server.shutdown().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_control_frames_skip_delivery_admission_read() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    reset_delivery_read_count(wiki.document_id);
    let mut cfg = test_collab_config(4, 30_000);
    cfg.max_pre_auth_outbound_frames = 1;
    let server = start_configured_test_server(&harness.app_url, cfg).await;
    let addr = server.addr;
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
    server.shutdown().await;
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
    let server = start_configured_test_server(&harness.app_url, cfg).await;
    let addr = server.addr;
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
    server.shutdown().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_delivery_auth_timeout_closes_1011_without_data_frame() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    let mut cfg = test_collab_config(4, 30_000);
    cfg.outbound_send_deadline_ms = 100;
    let server = start_configured_test_server(&harness.app_url, cfg).await;
    let addr = server.addr;
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
    server.shutdown().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_delivery_auth_cancel_closes_without_waiting_full_deadline() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    let mut cfg = test_collab_config_with_revoke(4, 30_000, 50);
    cfg.outbound_send_deadline_ms = 5_000;
    let server = start_configured_test_server(&harness.app_url, cfg).await;
    let addr = server.addr;
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
    server.shutdown().await;
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
    // Force the ACL-sweep call to run first. It must neither consume nor wait
    // on the outbound transport barrier, otherwise cancellation tests can
    // accidentally stall the very actor that must revoke the socket.
    let sweep = tokio::time::timeout(
        Duration::from_secs(2),
        check_delivery_admission(&pool, ws, user, session, doc),
    )
    .await
    .expect("ACL sweep must not consume the outbound barrier")
    .expect("ACL sweep query");
    assert!(matches!(
        sweep,
        DeliveryAdmission::Allowed { read_only: false }
    ));
    let handle = tokio::spawn({
        let pool = pool.clone();
        async move {
            fvoci_server::db::collab_delivery::authorize_outbound_delivery(
                &pool, ws, user, session, doc,
            )
            .await
        }
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
    let server = start_configured_test_server(&harness.app_url, cfg).await;
    let addr = server.addr;
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
    server.shutdown().await;
    harness.cleanup().await;
}

fn invalid_utf8_update_candidate() -> Vec<u8> {
    let mut bytes = engine_fixture("utf8_korean.v1");
    let marker = [0xEC, 0x95, 0x88];
    let pos = bytes
        .windows(3)
        .position(|w| w == marker)
        .expect("안녕 utf8 marker in utf8_korean.v1 fixture");
    bytes[pos] = 0xFF;
    bytes
}

async fn room_fence_backend_pid(pool: &PgPool, document_id: Uuid) -> Option<i32> {
    let lock_key = lock_key_from_uuid(document_id);
    sqlx::query_scalar(
        "SELECT l.pid FROM pg_locks l
         WHERE l.locktype = 'advisory'
           AND l.classid = $1
           AND l.objid = $2
           AND l.granted = true
         LIMIT 1",
    )
    .bind(COLLAB_ROOM_SESSION_LOCK_NAMESPACE)
    .bind(lock_key)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
}

#[tokio::test]
async fn collab_reject_reload_failure_closes_room_without_serving_rejected_to_peer() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    let server = start_product_test_server(&harness.app_url, true).await;
    let addr = server.addr;
    let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
    let good = sample_hi_update();
    let hostile = invalid_utf8_update_candidate();

    let mut writer = connect_member(addr, &wiki.session.session_token).await;
    auth_and_join(&mut writer, &routing_key, 501).await;
    writer
        .send(Message::Binary(
            sync_update_frame(&routing_key, &good).into(),
        ))
        .await
        .unwrap();
    assert!(
        wait_for_sync_applied(&mut writer, Duration::from_secs(3)).await,
        "baseline edit must apply"
    );

    let mut reader = connect_member(addr, &wiki.session.session_token).await;
    auth_and_join(&mut reader, &routing_key, 502).await;

    arm_force_primary_load_fail(wiki.document_id).await;
    writer
        .send(Message::Binary(
            sync_update_frame(&routing_key, &hostile).into(),
        ))
        .await
        .unwrap();
    wait_for_ws_close_code(&mut writer, 1013, Duration::from_secs(5), false).await;
    disarm_force_primary_load_fail(wiki.document_id).await;

    let mut recovery = connect_member(addr, &wiki.session.session_token).await;
    auth_and_join(&mut recovery, &routing_key, 503).await;
    recovery
        .send(Message::Binary(
            sync_step1_frame(&routing_key, &[0, 0]).into(),
        ))
        .await
        .unwrap();
    let mut step2_payload = None;
    for _ in 0..12 {
        if let Some(WireFrame::Document {
            message:
                DocumentMessage::Sync(SyncMessage {
                    step: SyncStep::Step2,
                    y_protocol,
                    ..
                }),
            ..
        }) = recv_document_frame(&mut recovery, 1).await
        {
            step2_payload = Some(
                parse_sync_payload(&y_protocol, 4 * 1024 * 1024)
                    .expect("step2")
                    .1,
            );
            break;
        }
    }
    let step2 = step2_payload.expect("peer must receive SyncStep2 after room reload failure close");
    assert_ne!(
        step2, hostile,
        "peer SyncStep2 must not contain the rejected hostile candidate"
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
    assert_eq!(load.tail[0].payload, good);

    let _ = reader.close(None).await;
    server.shutdown().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_room_fence_connection_loss_closes_room_and_recovers() {
    let harness = TestDb::bootstrap().await;
    let wiki = setup_wiki_doc(&harness).await;
    let server = start_product_test_server(&harness.app_url, true).await;
    let addr = server.addr;
    let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);

    let mut writer = connect_member(addr, &wiki.session.session_token).await;
    auth_and_join(&mut writer, &routing_key, 601).await;
    let fence_pid = room_fence_backend_pid(&wiki.session.pool, wiki.document_id)
        .await
        .expect("live room must hold session advisory lock on dedicated connection");
    sqlx::query("SELECT pg_terminate_backend($1)")
        .bind(fence_pid)
        .execute(&wiki.session.pool)
        .await
        .expect("terminate room fence backend");

    writer
        .send(Message::Binary(
            sync_update_frame(&routing_key, &sample_hi_update()).into(),
        ))
        .await
        .unwrap();
    wait_for_ws_close_code(&mut writer, 1013, Duration::from_secs(5), false).await;

    let mut recovery = connect_member(addr, &wiki.session.session_token).await;
    auth_and_join(&mut recovery, &routing_key, 602).await;
    recovery
        .send(Message::Binary(
            sync_update_frame(&routing_key, &sample_hi_update()).into(),
        ))
        .await
        .unwrap();
    assert!(
        wait_for_sync_applied(&mut recovery, Duration::from_secs(5)).await,
        "room must recover edits after fence connection loss and client reconnect"
    );

    server.shutdown().await;
    harness.cleanup().await;
}

#[tokio::test]
async fn collab_cold_reload_fragmented_document_fits_rlimits() {
    let mut session = EngineSession::spawn(SpawnRequest {
        engine_bin: engine_bin(),
        limits: collab_engine::Limits::for_tests(),
        slot_kind: collab_engine::process::ChildSlotKind::Primary,
        slot_wait: None,
        test_hang_ms: None,
        test_exit_after_read: None,
        test_close_stdout_hang_ms: None,
        test_exit_after_write: None,
    })
    .expect("spawn primary for fragmented cold reload");
    let mut tail = Vec::new();
    for i in 0..48usize {
        let update = engine_fixture(&format!("pending_u{}.v1", (i % 2) + 1));
        let apply = session.call(&Request::Apply {
            update_b64: update.clone(),
            encoding: 1,
        });
        assert!(
            apply.outcome.is_applied_ok(),
            "fragmented apply {i} must succeed before cold reload, got {:?}",
            apply.outcome
        );
        tail.push(update);
    }
    let snap = session.call(&Request::Snapshot);
    let snapshot = match snap.outcome {
        EngineStatus::Ok {
            update_b64: Some(bytes_b64),
            ..
        } => collab_engine::b64::decode(&bytes_b64).expect("snapshot bytes"),
        other => panic!("snapshot before cold reload failed: {other:?}"),
    };
    session.kill_and_reap();
    let mut cold = EngineSession::spawn(SpawnRequest {
        engine_bin: engine_bin(),
        limits: collab_engine::Limits::for_tests(),
        slot_kind: collab_engine::process::ChildSlotKind::Primary,
        slot_wait: None,
        test_hang_ms: None,
        test_exit_after_read: None,
        test_close_stdout_hang_ms: None,
        test_exit_after_write: None,
    })
    .expect("spawn fresh primary for cold reload");
    let load = cold.call(&Request::Load {
        snapshot_b64: Some(snapshot),
        tail_b64: tail,
        encoding: 1,
    });
    assert!(
        load.outcome.is_applied_ok(),
        "cold reload of fragmented document must fit helper rlimits, got {:?}",
        load.outcome
    );
}
