#![cfg(feature = "db-tests")]

#[allow(dead_code)]
mod support;

use std::io::{BufRead, BufReader};
use std::net::SocketAddr;
use std::panic::{resume_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::future::BoxFuture;
use futures_util::{FutureExt, SinkExt, StreamExt};
use fvoci_server::collab::hub::{CollabHub, RoomLifecyclePhase};
use fvoci_server::collab::room::{
    AuthenticatedConnection, CollabSession, ConnectionLease, JoinError, RoomJoin,
};
use fvoci_server::collab::wire::{
    AuthMessage, CollabKind, CollabRoomName, DocumentMessage, WireFrame,
};
use fvoci_server::db::collab::COLLAB_ROOM_SESSION_LOCK_NAMESPACE;
use fvoci_server::db::context::lock_key_from_uuid;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use support::{
    setup_wiki_doc, sync_update_frame, test_collab_config, TestDb, TestRun, WikiDocFixture, PEPPER,
    PUBLIC_ORIGIN,
};
use tokio::sync::mpsc as tokio_mpsc;
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

const TEST_TIMEOUT: Duration = Duration::from_secs(30);
const SAMPLE_HI_UPDATE: &str = "0101e8eda5a2070004010b70726f73656d6972726f7202686900";

struct ShutdownRun {
    inner: TestRun,
    hubs: Vec<Arc<CollabHub>>,
    leases: Vec<ConnectionLease>,
    children: Vec<OwnedChild>,
}

impl ShutdownRun {
    fn new(harness: TestDb) -> Self {
        Self {
            inner: TestRun::new(harness),
            hubs: Vec::new(),
            leases: Vec::new(),
            children: Vec::new(),
        }
    }

    fn register_hub(&mut self, hub: Arc<CollabHub>) -> Arc<CollabHub> {
        self.hubs.push(hub.clone());
        hub
    }

    fn retain_lease(&mut self, lease: ConnectionLease) {
        self.leases.push(lease);
    }

    fn retain_child(&mut self, child: OwnedChild) {
        self.children.push(child);
    }

    async fn finish(mut self) -> Result<(), String> {
        let mut errors = Vec::new();
        self.leases.clear();
        for mut child in self.children.drain(..) {
            child.kill_and_wait();
        }
        for hub in self.hubs {
            hub.shutdown().await;
        }
        if let Err(error) = self.inner.finish().await {
            errors.push(error);
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }
}

struct OwnedChild {
    child: Option<Child>,
    helper_pids: Vec<u32>,
}

impl OwnedChild {
    fn pid(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }

    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        match self.child.as_mut() {
            Some(child) => child.try_wait(),
            None => Ok(None),
        }
    }

    fn send_sigterm(&self) {
        let Some(pid) = self.pid() else {
            return;
        };
        let _ = Command::new("kill")
            .args(["-s", "TERM", &pid.to_string()])
            .status();
    }

    fn kill_and_wait(&mut self) {
        if let Some(mut child) = self.child.take() {
            match child.try_wait() {
                Ok(Some(_)) => {}
                _ => {
                    let pid = child.id();
                    let _ = Command::new("kill")
                        .args(["-s", "TERM", &pid.to_string()])
                        .status();
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
        }
        self.helper_pids
            .retain(|pid| std::path::Path::new(&format!("/proc/{pid}")).exists());
        for helper in &self.helper_pids {
            let _ = Command::new("kill")
                .args(["-s", "KILL", &helper.to_string()])
                .status();
        }
        self.helper_pids.clear();
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        self.kill_and_wait();
    }
}

async fn run_shutdown_test<F>(name: &str, case: F)
where
    F: for<'a> FnOnce(&'a mut ShutdownRun) -> BoxFuture<'a, ()>,
{
    let mut run = ShutdownRun::new(TestDb::bootstrap().await);
    let case_fut = case(&mut run);
    let case_outcome =
        tokio::time::timeout(TEST_TIMEOUT, AssertUnwindSafe(case_fut).catch_unwind()).await;
    let cleanup_outcome = run.finish().await;

    match (case_outcome, cleanup_outcome) {
        (Ok(Ok(())), Ok(())) => {}
        (Ok(Ok(())), Err(cleanup_err)) => {
            panic!("{name} cleanup failed after success: {cleanup_err}");
        }
        (Ok(Err(panic_payload)), cleanup) => {
            if let Err(cleanup_err) = cleanup {
                eprintln!("{name} cleanup also failed: {cleanup_err}");
            }
            resume_unwind(panic_payload);
        }
        (Err(_elapsed), Ok(())) => {
            panic!("{name} case hung (>{TEST_TIMEOUT:?}); cleanup completed");
        }
        (Err(_elapsed), Err(cleanup_err)) => {
            panic!("{name} hung (>{TEST_TIMEOUT:?}); cleanup error: {cleanup_err}");
        }
    }
}

fn room_key(workspace_id: Uuid, document_id: Uuid) -> (Uuid, Uuid) {
    (workspace_id, document_id)
}

fn routing_key(workspace_id: Uuid, document_id: Uuid) -> String {
    CollabRoomName {
        workspace_id,
        kind: CollabKind::Document,
        resource_id: document_id,
    }
    .routing_key()
}

fn sample_hi_update() -> Vec<u8> {
    hex::decode(SAMPLE_HI_UPDATE).expect("fixture")
}

async fn setup_second_doc(wiki: &WikiDocFixture) -> WikiDocFixture {
    use fvoci_server::db::documents::CreateDocumentInput;
    let created = fvoci_server::db::documents::create_wiki_document(
        &wiki.session.pool,
        wiki.session.workspace_id,
        wiki.session.user_id,
        wiki.session.session_id,
        CreateDocumentInput {
            parent_id: None,
            title: "Shutdown doc B",
            icon: None,
        },
        None,
    )
    .await
    .expect("create second doc")
    .expect("created");
    WikiDocFixture {
        session: support::SessionFixture {
            pool: wiki.session.pool.clone(),
            user_id: wiki.session.user_id,
            session_id: wiki.session.session_id,
            workspace_id: wiki.session.workspace_id,
            session_token: wiki.session.session_token.clone(),
        },
        document_id: created.id,
    }
}

async fn hub_join_with_lease(
    hub: &CollabHub,
    wiki: &WikiDocFixture,
    client_id: u32,
) -> Result<(Uuid, ConnectionLease), JoinError> {
    let conn_id = Uuid::now_v7();
    let (events_tx, mut events_rx) = tokio_mpsc::channel(8);
    tokio::spawn(async move { while events_rx.recv().await.is_some() {} });
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
            routing_key: routing_key(wiki.session.workspace_id, wiki.document_id),
        },
        events: events_tx,
        cancel: None,
    };
    let lease = hub
        .join_room(room_key(wiki.session.workspace_id, wiki.document_id), join)
        .await?;
    Ok((conn_id, lease))
}

async fn hub_join(
    run: &mut ShutdownRun,
    hub: &CollabHub,
    wiki: &WikiDocFixture,
    client_id: u32,
) -> Result<Uuid, JoinError> {
    let (id, lease) = hub_join_with_lease(hub, wiki, client_id).await?;
    run.retain_lease(lease);
    Ok(id)
}

async fn admin_pool(admin_url: &str) -> PgPool {
    PgPoolOptions::new()
        .max_connections(4)
        .connect(admin_url)
        .await
        .expect("admin pool")
}

async fn tail_seq(admin: &PgPool, document_id: Uuid) -> i64 {
    sqlx::query_scalar("SELECT tail_seq FROM fvoci.document_states WHERE document_id = $1")
        .bind(document_id)
        .fetch_optional(admin)
        .await
        .expect("tail_seq")
        .unwrap_or(0)
}

async fn room_guard_held(admin: &PgPool, document_id: Uuid) -> bool {
    let key = lock_key_from_uuid(document_id);
    sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM pg_locks
            WHERE locktype = 'advisory'
              AND classid = $1::oid
              AND objid = $2::oid
              AND granted
        )
        "#,
    )
    .bind(COLLAB_ROOM_SESSION_LOCK_NAMESPACE)
    .bind(key)
    .fetch_one(admin)
    .await
    .expect("pg_locks")
}

async fn wait_for_phase(hub: &CollabHub, key: (Uuid, Uuid), expected: RoomLifecyclePhase) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if hub.room_lifecycle_phase(key).await == expected {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!("room phase did not reach {expected:?}");
    });
}

async fn wait_for_document_states_blocked(admin: &PgPool, blocker_pid: i32) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let blocked: Option<i32> = sqlx::query_scalar(
            r#"
            SELECT activity.pid
            FROM pg_stat_activity AS activity
            WHERE activity.wait_event_type = 'Lock'
              AND activity.state = 'active'
              AND activity.query ILIKE '%document_states%'
              AND activity.query ILIKE '%FOR UPDATE%'
              AND $1 = ANY(pg_blocking_pids(activity.pid))
            LIMIT 1
            "#,
        )
        .bind(blocker_pid)
        .fetch_optional(admin)
        .await
        .expect("pg_stat_activity");
        if blocked.is_some() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("append did not block on document_states FOR UPDATE held by {blocker_pid}");
}

async fn wait_until_guard(admin: &PgPool, document_id: Uuid, held: bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if room_guard_held(admin, document_id).await == held {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("room guard held={held} not observed"));
}

fn process_comm(pid: u32) -> Option<String> {
    std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .ok()
        .map(|s| s.trim().to_string())
}

fn direct_children(pid: u32) -> Vec<u32> {
    let mut pids = Vec::new();
    let Ok(entries) = std::fs::read_dir(format!("/proc/{pid}/task")) else {
        return pids;
    };
    for entry in entries.flatten() {
        if let Ok(text) = std::fs::read_to_string(entry.path().join("children")) {
            for token in text.split_whitespace() {
                if let Ok(child) = token.parse::<u32>() {
                    pids.push(child);
                }
            }
        }
    }
    pids.sort_unstable();
    pids.dedup();
    pids
}

fn collab_engine_descendants(root: u32) -> Vec<u32> {
    let mut found = Vec::new();
    let mut stack = vec![root];
    let mut seen = std::collections::HashSet::new();
    while let Some(pid) = stack.pop() {
        if !seen.insert(pid) {
            continue;
        }
        for child in direct_children(pid) {
            stack.push(child);
            if process_comm(child).is_some_and(|comm| comm.starts_with("collab-engine")) {
                found.push(child);
            }
        }
    }
    found
}

fn pid_alive(pid: u32) -> bool {
    PathBuf::from(format!("/proc/{pid}")).exists()
}

fn wait_pids_exit(pids: &[u32], within: Duration) {
    let deadline = Instant::now() + within;
    loop {
        if pids.iter().all(|pid| !pid_alive(*pid)) {
            return;
        }
        if Instant::now() >= deadline {
            let live: Vec<u32> = pids.iter().copied().filter(|pid| pid_alive(*pid)).collect();
            panic!("helper pids still live after parent exit: {live:?}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn server_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fvoci-server"))
}

fn pump_lines<R: std::io::Read + Send + 'static>(
    stream: R,
    logs: Arc<Mutex<Vec<String>>>,
    tx: mpsc::Sender<String>,
) {
    std::thread::spawn(move || {
        let reader = BufReader::new(stream);
        for line in reader.lines().map_while(Result::ok) {
            if let Ok(mut held) = logs.lock() {
                held.push(line.clone());
            }
            let _ = tx.send(line);
        }
    });
}

fn spawn_server_process(
    harness: &TestDb,
    deadline_ms: u64,
) -> (OwnedChild, SocketAddr, Arc<Mutex<Vec<String>>>) {
    spawn_server_process_with_auth_wait(harness, deadline_ms, None)
}

fn spawn_server_process_with_auth_wait(
    harness: &TestDb,
    deadline_ms: u64,
    auth_wait_ms: Option<u64>,
) -> (OwnedChild, SocketAddr, Arc<Mutex<Vec<String>>>) {
    let engine = fvoci_server::collab::config::require_collab_engine_for_tests();
    let logs = Arc::new(Mutex::new(Vec::new()));
    let storage_root =
        std::env::temp_dir().join(format!("fvoci-collab-shutdown-store-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&storage_root).expect("collab shutdown storage root");
    let mut command = Command::new(server_bin());
    command
        .env("DATABASE_URL", &harness.admin_url)
        .env("DATABASE_APP_URL", &harness.app_url)
        .env("PASSWORD_PEPPER_KEYS", PEPPER)
        .env("PASSWORD_PEPPER_ACTIVE_KEY_ID", "test")
        .env("FVOCI_BIND", "127.0.0.1:0")
        .env("FVOCI_PUBLIC_ORIGIN", PUBLIC_ORIGIN)
        .env("FVOCI_COOKIE_SECURE", "0")
        .env("FVOCI_COLLAB_ENGINE", &engine)
        .env("FVOCI_STORAGE_DIR", &storage_root)
        .env("FVOCI_SHUTDOWN_DEADLINE_MS", deadline_ms.to_string())
        .env("FVOCI_COLLAB_IDLE_MS", "60000")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(auth_wait_ms) = auth_wait_ms {
        command.env("FVOCI_COLLAB_AUTH_WAIT_MS", auth_wait_ms.to_string());
    }
    let mut child = command.spawn().expect("spawn fvoci-server");
    let stderr = child.stderr.take().expect("stderr");
    let stdout = child.stdout.take().expect("stdout");
    let (tx, rx) = mpsc::channel::<String>();
    pump_lines(stderr, logs.clone(), tx.clone());
    pump_lines(stdout, logs.clone(), tx);
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut listen = None;
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(line) => {
                if let Some(rest) = line.strip_prefix("fvoci-server listening on ") {
                    listen = Some(rest.trim().to_string());
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if child.try_wait().ok().flatten().is_some() {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let addr = match listen {
        Some(url) => {
            let parsed = url::Url::parse(&url).expect("listen url");
            SocketAddr::new(
                parsed.host_str().expect("listen host").parse().expect("ip"),
                parsed.port().expect("listen port"),
            )
        }
        None => {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "fvoci-server did not print listen address; logs={:?}",
                logs.lock().unwrap()
            );
        }
    };
    (
        OwnedChild {
            child: Some(child),
            helper_pids: Vec::new(),
        },
        addr,
        logs,
    )
}

fn wait_for_exit(child: &mut OwnedChild, within: Duration) -> ExitStatus {
    let deadline = Instant::now() + within;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                child.child = None;
                return status;
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let logs_note = format!("pid={:?}", child.pid());
                    child.kill_and_wait();
                    panic!("server still running after {within:?} ({logs_note})");
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("wait for server: {error}"),
        }
    }
}

#[tokio::test]
async fn begin_shutdown_rejects_new_sockets_joins_and_frames() {
    run_shutdown_test(
        "begin_shutdown_rejects_new_sockets_joins_and_frames",
        |run| {
            Box::pin(async {
                let wiki = setup_wiki_doc(&run.inner.harness).await;
                let hub = run.register_hub(Arc::new(CollabHub::new(
                    test_collab_config(4, 30_000),
                    wiki.session.pool.clone(),
                )));
                let key = room_key(wiki.session.workspace_id, wiki.document_id);
                let conn_id = hub_join(run, &hub, &wiki, 1).await.expect("join");
                let admin = admin_pool(&run.inner.harness.admin_url).await;
                wait_until_guard(&admin, wiki.document_id, true).await;
                let before = tail_seq(&admin, wiki.document_id).await;
                let permit_before = hub
                    .try_acquire_socket(wiki.session.session_id)
                    .expect("socket before shutdown");
                drop(permit_before);

                hub.begin_shutdown();
                assert!(
                    hub.is_shutting_down(),
                    "begin_shutdown must be visible without awaiting hub.shutdown"
                );
                assert!(
                    hub.try_acquire_socket(wiki.session.session_id).is_none(),
                    "new sockets must be refused at the stop signal"
                );
                let join_after = hub_join_with_lease(&hub, &wiki, 2).await;
                assert!(
                    matches!(join_after, Err(JoinError::EngineUnavailable)),
                    "new joins must be refused at the stop signal, got {join_after:?}"
                );

                let frame = sync_update_frame(
                    &routing_key(wiki.session.workspace_id, wiki.document_id),
                    &sample_hi_update(),
                );
                hub.send_frame(key, conn_id, frame).await;
                // Probe is ordered after any frame the hub might have queued.
                assert_eq!(
                    hub.probe_actor(key).await.connections,
                    1,
                    "in-flight membership must stay until owned shutdown joins the actor"
                );
                assert_eq!(
                    tail_seq(&admin, wiki.document_id).await,
                    before,
                    "frames submitted after begin_shutdown must not persist"
                );
            })
        },
    )
    .await;
}

#[tokio::test]
async fn shutdown_closes_independent_room_while_other_blocked_on_row_lock() {
    run_shutdown_test(
        "shutdown_closes_independent_room_while_other_blocked_on_row_lock",
        |run| {
            Box::pin(async {
                let wiki = setup_wiki_doc(&run.inner.harness).await;
                let doc_b = setup_second_doc(&wiki).await;
                let hub = run.register_hub(Arc::new(CollabHub::new(
                    test_collab_config(4, 30_000),
                    wiki.session.pool.clone(),
                )));
                let key_a = room_key(wiki.session.workspace_id, wiki.document_id);
                let key_b = room_key(doc_b.session.workspace_id, doc_b.document_id);
                let conn_a = hub_join(run, &hub, &wiki, 1).await.expect("join A");
                let _conn_b = hub_join(run, &hub, &doc_b, 2).await.expect("join B");
                let admin = admin_pool(&run.inner.harness.admin_url).await;
                wait_until_guard(&admin, wiki.document_id, true).await;
                wait_until_guard(&admin, doc_b.document_id, true).await;

                let mut barrier = admin.begin().await.expect("barrier tx");
                let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                    .fetch_one(&mut *barrier)
                    .await
                    .expect("blocker pid");
                sqlx::query(
                    r#"
                    SELECT writer_generation
                    FROM fvoci.document_states
                    WHERE workspace_id = $1 AND document_id = $2
                    FOR UPDATE
                    "#,
                )
                .bind(wiki.session.workspace_id)
                .bind(wiki.document_id)
                .execute(&mut *barrier)
                .await
                .expect("hold document_states");

                let frame = sync_update_frame(
                    &routing_key(wiki.session.workspace_id, wiki.document_id),
                    &sample_hi_update(),
                );
                let send_a = tokio::spawn({
                    let hub = hub.clone();
                    async move {
                        hub.send_frame(key_a, conn_a, frame).await;
                    }
                });
                wait_for_document_states_blocked(&admin, blocker_pid).await;

                let shutdown = tokio::spawn({
                    let hub = hub.clone();
                    async move {
                        hub.shutdown().await;
                    }
                });
                wait_for_phase(&hub, key_b, RoomLifecyclePhase::Absent).await;
                wait_until_guard(&admin, doc_b.document_id, false).await;
                assert!(
                    room_guard_held(&admin, wiki.document_id).await,
                    "blocked room must keep its acquired guard until the in-flight commit finishes"
                );
                assert!(
                    !shutdown.is_finished(),
                    "shutdown must not finish while an in-flight append is still in a DB wait"
                );

                barrier.commit().await.expect("release document_states");
                tokio::time::timeout(Duration::from_secs(5), shutdown)
                    .await
                    .expect("shutdown must finish after the blocked append is allowed to complete")
                    .expect("shutdown task");
                let _ = send_a.await;
                wait_until_guard(&admin, wiki.document_id, false).await;
                assert!(
                    tail_seq(&admin, wiki.document_id).await > 0,
                    "in-flight append must commit; shutdown must not drop that tx as rollback"
                );
            })
        },
    )
    .await;
}

#[tokio::test]
async fn process_sigterm_joins_helper_and_releases_guard() {
    run_shutdown_test("process_sigterm_joins_helper_and_releases_guard", |run| {
        Box::pin(async {
            let wiki = setup_wiki_doc(&run.inner.harness).await;
            let (mut child, addr, logs) = spawn_server_process(&run.inner.harness, 30_000);
            let mut ws = support::connect_member(addr, &wiki.session.session_token).await;
            let key = routing_key(wiki.session.workspace_id, wiki.document_id);
            support::auth_and_join(&mut ws, &key, 1).await;
            support::complete_sync_handshake(&mut ws, &key).await;
            let admin = admin_pool(&run.inner.harness.admin_url).await;
            wait_until_guard(&admin, wiki.document_id, true).await;
            let server_pid = child.pid().expect("server pid");
            let helpers = collab_engine_descendants(server_pid);
            assert!(
                !helpers.is_empty(),
                "joined room must own a collab-engine child"
            );
            child.helper_pids = helpers.clone();

            child.send_sigterm();
            let status = wait_for_exit(&mut child, Duration::from_secs(10));
            assert!(
                status.success(),
                "normal SIGTERM must exit 0, got {status}; logs={:?}",
                logs.lock().unwrap()
            );
            wait_pids_exit(&helpers, Duration::from_secs(1));
            wait_until_guard(&admin, wiki.document_id, false).await;
            drop(ws);
            run.retain_child(child);
        })
    })
    .await;
}

#[tokio::test]
async fn process_shutdown_deadline_exits_nonzero() {
    run_shutdown_test("process_shutdown_deadline_exits_nonzero", |run| {
        Box::pin(async {
            let wiki = setup_wiki_doc(&run.inner.harness).await;
            let deadline_ms = 800u64;
            let (mut child, addr, logs) = spawn_server_process(&run.inner.harness, deadline_ms);
            let mut ws = support::connect_member(addr, &wiki.session.session_token).await;
            let key = routing_key(wiki.session.workspace_id, wiki.document_id);
            support::auth_and_join(&mut ws, &key, 1).await;
            support::complete_sync_handshake(&mut ws, &key).await;
            let admin = admin_pool(&run.inner.harness.admin_url).await;
            wait_until_guard(&admin, wiki.document_id, true).await;

            let mut barrier = admin.begin().await.expect("barrier tx");
            let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *barrier)
                .await
                .expect("blocker pid");
            sqlx::query(
                r#"
                SELECT writer_generation
                FROM fvoci.document_states
                WHERE workspace_id = $1 AND document_id = $2
                FOR UPDATE
                "#,
            )
            .bind(wiki.session.workspace_id)
            .bind(wiki.document_id)
            .execute(&mut *barrier)
            .await
            .expect("hold document_states");

            ws.send(Message::Binary(
                sync_update_frame(&key, &sample_hi_update()).into(),
            ))
            .await
            .expect("send update");
            wait_for_document_states_blocked(&admin, blocker_pid).await;

            let server_pid = child.pid().expect("server pid");
            let helpers = collab_engine_descendants(server_pid);
            child.helper_pids = helpers.clone();
            let signaled = Instant::now();
            child.send_sigterm();
            let status = wait_for_exit(
                &mut child,
                Duration::from_millis(deadline_ms.saturating_mul(2) + 1_000),
            );
            let elapsed = signaled.elapsed();
            assert!(
                !status.success(),
                "deadline expiry must be nonzero, got {status}; logs={:?}",
                logs.lock().unwrap()
            );
            assert!(
                elapsed < Duration::from_millis(deadline_ms + 1_500),
                "deadline must bound exit, elapsed={elapsed:?} deadline={deadline_ms}ms"
            );
            let log_text = logs.lock().unwrap().join("\n");
            assert!(
                log_text.contains("shutdown deadline exceeded")
                    || log_text.contains("server shutdown deadline exceeded"),
                "expiry must report deadline failure, logs={log_text}"
            );
            wait_pids_exit(&helpers, Duration::from_secs(1));
            barrier.rollback().await.ok();
            drop(ws);
            run.retain_child(child);
        })
    })
    .await;
}

#[tokio::test]
async fn shutdown_reports_start_task_panic_as_unclean() {
    run_shutdown_test("shutdown_reports_start_task_panic_as_unclean", |run| {
        Box::pin(async {
            let wiki = setup_wiki_doc(&run.inner.harness).await;
            let hub = run.register_hub(Arc::new(CollabHub::new(
                test_collab_config(4, 30_000),
                wiki.session.pool.clone(),
            )));
            hub.spawn_panicking_start_task_for_tests();
            let status = hub.shutdown().await;
            assert!(
                !status.is_clean(),
                "a panicking start task must not be reported as a clean shutdown: {status:?}"
            );
            assert!(
                status.start_task_failures >= 1,
                "start-task JoinHandle failure must be counted, got {status:?}"
            );
            assert!(
                !status.idle_task_failed,
                "idle join is independent of an injected start panic: {status:?}"
            );
        })
    })
    .await;
}

#[tokio::test]
async fn process_http_drain_deadline_exits_nonzero() {
    run_shutdown_test("process_http_drain_deadline_exits_nonzero", |run| {
        Box::pin(async {
            let deadline_ms = 800u64;
            let (mut child, addr, logs) = spawn_server_process(&run.inner.harness, deadline_ms);
            let mut held = std::net::TcpStream::connect(addr).expect("hold http connection");
            held.set_nodelay(true).ok();
            let request = format!(
                "POST /api/v1/setup HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: 1048576\r\n\r\n",
                addr
            );
            std::io::Write::write_all(&mut held, request.as_bytes()).expect("headers without body");

            let signaled = Instant::now();
            child.send_sigterm();
            let status = wait_for_exit(
                &mut child,
                Duration::from_millis(deadline_ms.saturating_mul(2) + 1_000),
            );
            let elapsed = signaled.elapsed();
            assert!(
                !status.success(),
                "HTTP drain deadline must be nonzero, got {status}; logs={:?}",
                logs.lock().unwrap()
            );
            assert!(
                elapsed < Duration::from_millis(deadline_ms + 1_500),
                "HTTP drain must be bound by the same deadline, elapsed={elapsed:?} deadline={deadline_ms}ms"
            );
            let log_text = logs.lock().unwrap().join("\n");
            assert!(
                log_text.contains("shutdown deadline exceeded")
                    || log_text.contains("server shutdown deadline exceeded"),
                "HTTP drain expiry must report deadline, not clean success, logs={log_text}"
            );
            assert!(
                !log_text.contains("collaboration task panicked"),
                "HTTP drain expiry is not a helper panic, logs={log_text}"
            );
            drop(held);
            run.retain_child(child);
        })
    })
    .await;
}

async fn wait_for_restart_close_without_auth_denied(
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
                let code = u16::from(frame.code);
                assert!(
                    code == 1012,
                    "pre-auth shutdown CloseFrame {code} ({:?}), expected 1012; reason {:?}",
                    frame.code,
                    frame.reason
                );
                return;
            }
            Ok(Some(Ok(Message::Close(None)))) => {
                panic!("Close without code, expected CloseFrame 1012");
            }
            Ok(Some(Ok(Message::Binary(bytes)))) => {
                if let Ok(WireFrame::Document {
                    message: DocumentMessage::Auth(AuthMessage::PermissionDenied { reason }),
                    ..
                }) = fvoci_server::collab::wire::decode(&bytes)
                {
                    panic!(
                        "pre-auth shutdown must not send PermissionDenied ({reason}); expected Close 1012"
                    );
                }
            }
            Ok(Some(Ok(_))) => {}
            Ok(None) => panic!("bare TCP EOF without CloseFrame, expected 1012"),
            Ok(Some(Err(err))) => panic!("websocket error before CloseFrame 1012: {err}"),
            Err(_) => {}
        }
    }
    panic!("did not receive CloseFrame 1012 within {within:?}");
}

#[tokio::test]
async fn process_sigterm_releases_pre_auth_socket_before_auth_wait() {
    run_shutdown_test(
        "process_sigterm_releases_pre_auth_socket_before_auth_wait",
        |run| {
            Box::pin(async {
                let wiki = setup_wiki_doc(&run.inner.harness).await;
                let deadline_ms = 800u64;
                let auth_wait_ms = 5_000u64;
                let (mut child, addr, logs) = spawn_server_process_with_auth_wait(
                    &run.inner.harness,
                    deadline_ms,
                    Some(auth_wait_ms),
                );
                let mut ws = support::connect_member(addr, &wiki.session.session_token).await;
                let signaled = Instant::now();
                child.send_sigterm();
                wait_for_restart_close_without_auth_denied(
                    &mut ws,
                    Duration::from_millis(deadline_ms),
                )
                .await;
                let status = wait_for_exit(
                    &mut child,
                    Duration::from_millis(deadline_ms.saturating_mul(2) + 1_000),
                );
                let elapsed = signaled.elapsed();
                assert!(
                    status.success(),
                    "pre-auth SIGTERM must exit 0 before auth_wait, got {status}; logs={:?}",
                    logs.lock().unwrap()
                );
                assert!(
                    elapsed < Duration::from_millis(auth_wait_ms),
                    "pre-auth socket must not hold shutdown for auth_wait, elapsed={elapsed:?} auth_wait_ms={auth_wait_ms}"
                );
                drop(ws);
                run.retain_child(child);
            })
        },
    )
    .await;
}
