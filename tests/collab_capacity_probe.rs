//! Heavy collab capacity probe. Run only via `scripts/collab-capacity-probe.sh`.
#![cfg(feature = "db-tests")]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{Duration as ChronoDuration, Utc};
use collab_engine::process::{
    max_child_concurrency, max_validator_child_concurrency, raise_nofile_to_hard_limit,
    sum_live_children_rss_bytes,
};
use futures_util::future::join_all;
use futures_util::{SinkExt, StreamExt};
use fvoci_server::auth::password::Keyring;
use fvoci_server::auth::token::new_token;
use fvoci_server::collab::config::{
    collab_pg_connections_required, derive_app_pool_max_connections, derive_max_child_concurrency,
};
use fvoci_server::collab::hub::CollabHub;
use fvoci_server::collab::wire::{
    encode, AuthMessage, CollabKind, CollabRoomName, DocumentMessage, WireFrame,
};
use sqlx::postgres::PgPoolOptions;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::Message;

mod support;

use support::{
    complete_sync_handshake, connect_member, engine_fixture, invalid_utf8_update_candidate,
    setup_owner_session, sync_update_frame, test_collab_config, wait_for_sync_applied,
    wait_for_sync_update, TestDb, TestRun, PEPPER,
};

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

fn probe_rooms() -> usize {
    std::env::var("COLLAB_PROBE_ROOMS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(64)
}

fn probe_peers() -> usize {
    std::env::var("COLLAB_PROBE_PEERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2)
        .max(1)
}

fn probe_duration() -> Duration {
    let secs = std::env::var("COLLAB_PROBE_DURATION_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(180);
    Duration::from_secs(secs.max(180))
}

fn probe_open_concurrency() -> usize {
    std::env::var("COLLAB_PROBE_OPEN_CONCURRENCY")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8)
        .max(1)
}

fn probe_auth_timeout() -> Duration {
    Duration::from_secs(180)
}

fn room_key(workspace_id: uuid::Uuid, document_id: uuid::Uuid) -> String {
    CollabRoomName {
        workspace_id,
        kind: CollabKind::Document,
        resource_id: document_id,
    }
    .routing_key()
}

fn ws_close_code(frame: &CloseFrame) -> u16 {
    u16::from(frame.code)
}

#[derive(Clone)]
struct PeerSession {
    token: String,
}

#[derive(Clone)]
struct ProbeDoc {
    workspace_id: uuid::Uuid,
    document_id: uuid::Uuid,
    peers: Vec<PeerSession>,
}

struct RoomPeers {
    routing_key: String,
    writer: Arc<tokio::sync::Mutex<Ws>>,
    reader: Arc<tokio::sync::Mutex<Ws>>,
}

fn probe_config(max_rooms: usize, peers: usize) -> fvoci_server::collab::config::CollabConfig {
    let mut cfg = test_collab_config(max_rooms, 3_000);
    cfg.max_child_concurrency = derive_max_child_concurrency(max_rooms);
    cfg.memory_budget_bytes = 8 * 1024 * 1024 * 1024;
    cfg.max_collab_sockets = max_rooms * peers + 64;
    cfg.max_collab_sockets_per_session = peers.max(4);
    cfg.max_connections_per_room = peers.max(2);
    cfg.revoke_poll_ms = 3_600_000;
    cfg.engine_bin = std::env::var("FVOCI_COLLAB_ENGINE")
        .ok()
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_file())
        .unwrap_or_else(fvoci_server::collab::config::require_collab_engine_for_tests);
    cfg
}

fn proc_threads_fds() -> (u64, usize) {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let threads = status
        .lines()
        .find_map(|line| line.strip_prefix("Threads:"))
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(0);
    let fds = std::fs::read_dir("/proc/self/fd")
        .map(|entries| entries.count())
        .unwrap_or(0);
    (threads, fds)
}

fn percentile(sorted: &[u64], pct: usize) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    sorted[(sorted.len() * pct / 100).min(sorted.len() - 1)]
}

async fn try_auth_expect_1013(ws: &mut Ws, routing_key: &str, client_id: u32) -> bool {
    let frame = encode(&WireFrame::Document {
        routing_key: routing_key.to_string(),
        room: CollabRoomName::parse(routing_key),
        message: DocumentMessage::Auth(AuthMessage::Token {
            token: client_id.to_string(),
            provider_version: Some("4.6.0".into()),
        }),
    })
    .expect("encode auth");
    ws.send(Message::Binary(frame.into())).await.unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), ws.next()).await {
            Ok(Some(Ok(Message::Close(Some(frame))))) => return ws_close_code(&frame) == 1013,
            Ok(Some(Ok(Message::Binary(bytes)))) => {
                if matches!(
                    fvoci_server::collab::wire::decode(&bytes),
                    Ok(WireFrame::Document {
                        message: DocumentMessage::Auth(AuthMessage::PermissionDenied { .. }),
                        ..
                    })
                ) {
                    return false;
                }
            }
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(_))) | Ok(None) => return false,
            Err(_) => {}
        }
    }
    false
}

async fn probe_auth_and_join(ws: &mut Ws, routing_key: &str, client_id: u32) -> Result<(), String> {
    let frame = encode(&WireFrame::Document {
        routing_key: routing_key.to_string(),
        room: CollabRoomName::parse(routing_key),
        message: DocumentMessage::Auth(AuthMessage::Token {
            token: client_id.to_string(),
            provider_version: Some("4.6.0".into()),
        }),
    })
    .expect("encode auth");
    ws.send(Message::Binary(frame.into())).await.unwrap();
    let deadline = tokio::time::Instant::now() + probe_auth_timeout();
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), ws.next()).await {
            Ok(Some(Ok(Message::Binary(bytes)))) => {
                if matches!(
                    fvoci_server::collab::wire::decode(&bytes),
                    Ok(WireFrame::Document {
                        message: DocumentMessage::Auth(AuthMessage::Authenticated { .. }),
                        ..
                    })
                ) {
                    return Ok(());
                }
            }
            Ok(Some(Ok(Message::Close(Some(frame))))) => {
                return Err(format!("close {}: {}", ws_close_code(&frame), frame.reason));
            }
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(err))) => return Err(format!("ws error: {err}")),
            Ok(None) => return Err("ws eof".into()),
            Err(_) => {}
        }
    }
    Err("auth timeout".into())
}

async fn wait_for_room_slot(hub: &Arc<CollabHub>, within: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + within;
    while tokio::time::Instant::now() < deadline {
        if hub.available_room_slots() >= 1 {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    false
}

async fn drain_ws(ws: &mut Ws, within: Duration) {
    let deadline = tokio::time::Instant::now() + within;
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(50), ws.next()).await {
            Ok(Some(Ok(_))) => continue,
            _ => break,
        }
    }
}

async fn read_close_code(ws: &mut Ws, within: Duration) -> Option<u16> {
    let deadline = tokio::time::Instant::now() + within;
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(100), ws.next()).await {
            Ok(Some(Ok(Message::Close(Some(frame))))) => return Some(ws_close_code(&frame)),
            Ok(Some(Ok(_))) => continue,
            _ => break,
        }
    }
    None
}

async fn writer_alive(ws: &mut Ws, routing_key: &str, marker: &[u8], within: Duration) -> bool {
    if ws
        .send(Message::Binary(
            sync_update_frame(routing_key, marker).into(),
        ))
        .await
        .is_err()
    {
        return false;
    }
    wait_for_sync_applied(ws, within).await
}

async fn drain_ws_locked(ws: &Arc<tokio::sync::Mutex<Ws>>, within: Duration) {
    let mut guard = ws.lock().await;
    drain_ws(&mut guard, within).await;
}

async fn writer_alive_locked(
    ws: &Arc<tokio::sync::Mutex<Ws>>,
    routing_key: &str,
    marker: &[u8],
    within: Duration,
) -> bool {
    let mut guard = ws.lock().await;
    writer_alive(&mut guard, routing_key, marker, within).await
}

fn probe_open_failure_diag(hub: &Arc<CollabHub>, room_index: usize, reason: &str) {
    eprintln!(
        "probe open failure room_index={} reason={} primary_cap={} validator_cap={} child_rss={} room_slots={} collab_sockets={} memory_budget_gib={}",
        room_index,
        reason,
        max_child_concurrency(),
        max_validator_child_concurrency(),
        sum_live_children_rss_bytes(),
        hub.available_room_slots(),
        hub.available_collab_sockets(),
        hub.config().memory_budget_bytes / (1024 * 1024 * 1024)
    );
}

async fn spawn_peer_session(
    harness: &TestDb,
    workspace_id: uuid::Uuid,
    label: &str,
) -> PeerSession {
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&harness.admin_url)
        .await
        .expect("admin pool");
    let user_id = uuid::Uuid::now_v7();
    let hash = fvoci_server::auth::password::hash_password(
        "supersecret1",
        &Keyring::parse(PEPPER, "test").unwrap(),
    )
    .await
    .expect("hash");
    sqlx::query(
        "INSERT INTO fvoci.users (id, email, password_hash, given_name) VALUES ($1, $2, $3, $4)",
    )
    .bind(user_id)
    .bind(format!("{label}-{user_id}@example.com"))
    .bind(&hash)
    .bind(label)
    .execute(&admin)
    .await
    .expect("insert user");
    sqlx::query(
        "INSERT INTO fvoci.memberships (workspace_id, user_id, role) VALUES ($1, $2, 'member')",
    )
    .bind(workspace_id)
    .bind(user_id)
    .execute(&admin)
    .await
    .expect("insert membership");
    admin.close().await;

    let app = PgPoolOptions::new()
        .max_connections(2)
        .connect(&harness.app_url)
        .await
        .expect("app pool");
    let token = new_token();
    let session_id = uuid::Uuid::now_v7();
    let expires = Utc::now() + ChronoDuration::days(30);
    let mut tx = app.begin().await.expect("begin");
    fvoci_server::db::identity::create_session(&mut tx, session_id, user_id, &token.hash, expires)
        .await
        .expect("session");
    tx.commit().await.expect("commit");
    app.close().await;
    PeerSession { token: token.token }
}

async fn setup_probe_docs(harness: &TestDb, count: usize, peers: usize) -> Vec<ProbeDoc> {
    let owner = setup_owner_session(harness).await;
    let mut docs = Vec::with_capacity(count);
    for index in 0..count {
        let title = format!("Collab capacity doc {index}");
        let created = fvoci_server::db::documents::create_wiki_document(
            &owner.pool,
            owner.workspace_id,
            owner.user_id,
            owner.session_id,
            fvoci_server::db::documents::CreateDocumentInput {
                parent_id: None,
                title: &title,
                icon: None,
            },
            None,
        )
        .await
        .expect("create doc")
        .expect("created");
        let mut peer_sessions = Vec::with_capacity(peers);
        for peer in 0..peers {
            let label = if peer == 0 {
                format!("writer-{index}")
            } else {
                format!("reader-{index}-{peer}")
            };
            peer_sessions.push(spawn_peer_session(harness, owner.workspace_id, &label).await);
        }
        docs.push(ProbeDoc {
            workspace_id: owner.workspace_id,
            document_id: created.id,
            peers: peer_sessions,
        });
    }
    docs
}

async fn open_room_peers(
    addr: std::net::SocketAddr,
    doc: &ProbeDoc,
    peers: usize,
    room_index: usize,
    hub: Arc<CollabHub>,
) -> RoomPeers {
    let routing_key = room_key(doc.workspace_id, doc.document_id);
    let mut writer_ws = None;
    let mut reader_ws = None;
    let base_id = (room_index as u32 + 1) * 100;
    for peer in 0..peers {
        let client_id = base_id + peer as u32;
        let session = &doc.peers[peer];
        let mut ws = connect_member(addr, &session.token).await;
        let mut last_err = String::from("no attempts");
        let authed = {
            let mut ok = false;
            for attempt in 0..12 {
                match probe_auth_and_join(&mut ws, &routing_key, client_id).await {
                    Ok(()) => {
                        ok = true;
                        break;
                    }
                    Err(reason) => {
                        last_err = reason;
                        ws = connect_member(addr, &session.token).await;
                        tokio::time::sleep(Duration::from_secs(1 + attempt as u64)).await;
                    }
                }
            }
            ok
        };
        if !authed {
            probe_open_failure_diag(&hub, room_index, &last_err);
        }
        assert!(
            authed,
            "probe auth failed for {routing_key} peer {client_id}: {last_err}"
        );
        if peer == 0 && peers > 1 {
            complete_sync_handshake(&mut ws, &routing_key).await;
        }
        if peer == 0 {
            writer_ws = Some(ws);
        } else {
            reader_ws = Some(ws);
        }
    }
    let writer = writer_ws.expect("writer peer");
    let reader = reader_ws.unwrap_or_else(|| {
        panic!("reader peer required when peers > 1");
    });
    RoomPeers {
        routing_key,
        writer: Arc::new(tokio::sync::Mutex::new(writer)),
        reader: Arc::new(tokio::sync::Mutex::new(reader)),
    }
}

fn marker_edit(tick: u64) -> Vec<u8> {
    if tick.is_multiple_of(2) {
        engine_fixture("pending_u1.v1")
    } else {
        engine_fixture("pending_u2.v1")
    }
}

struct SampleEvent {
    sent_at: Instant,
}

const LATENCY_WAIT: Duration = Duration::from_secs(5);

fn hostile_sample_indices(max_rooms: usize) -> Vec<usize> {
    if max_rooms >= 64 {
        return [10, 20, 30, 45, 60]
            .into_iter()
            .map(|i| i.min(max_rooms - 1))
            .collect();
    }
    let mut indices = Vec::new();
    for idx in 1..max_rooms {
        if indices.len() >= 5.min(max_rooms - 1) {
            break;
        }
        indices.push(idx);
    }
    indices
}

#[tokio::test(flavor = "multi_thread", worker_threads = 32)]
async fn collab_capacity_probe() {
    raise_nofile_to_hard_limit();
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("collab.stage=info".parse().expect("directive")),
        )
        .try_init();

    let max_rooms = probe_rooms();
    let peers = probe_peers();
    let duration = probe_duration();
    let open_concurrency = probe_open_concurrency();

    let harness = TestDb::bootstrap().await;
    let mut run = TestRun::new(harness);
    let cfg = probe_config(max_rooms, peers);
    let app_url = run.harness.app_url.clone();
    let db_pool_size = derive_app_pool_max_connections(max_rooms);
    let pg_required = collab_pg_connections_required(max_rooms) as u32 + 8;
    let pg_max = std::env::var("FVOCI_TEST_PG_MAX_CONNECTIONS")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(pg_required.max(150));
    assert!(
        pg_max >= collab_pg_connections_required(max_rooms) as u32,
        "FVOCI_TEST_PG_MAX_CONNECTIONS={pg_max} below collab requirement {}",
        collab_pg_connections_required(max_rooms)
    );
    let addr = run
        .spawn_router_with_pool(&app_url, cfg.clone(), db_pool_size)
        .await;
    let hub = run.hub();

    eprintln!(
        "probe config: rooms={} peers={} duration_s={} open_concurrency={} primary_cap={} validator_cap={} memory_budget_gib={} db_pool={}",
        max_rooms,
        peers,
        duration.as_secs(),
        open_concurrency,
        max_child_concurrency(),
        max_validator_child_concurrency(),
        cfg.memory_budget_bytes / (1024 * 1024 * 1024),
        db_pool_size
    );

    let docs_started = Instant::now();
    let docs = setup_probe_docs(&run.harness, max_rooms + 1, peers).await;
    eprintln!(
        "probe: created {} documents in {:?}",
        docs.len(),
        docs_started.elapsed()
    );

    let open_started = Instant::now();
    let mut writer_rooms: Vec<(String, Arc<tokio::sync::Mutex<Ws>>)> =
        Vec::with_capacity(max_rooms);
    let mut reader_rooms: Vec<Arc<tokio::sync::Mutex<Ws>>> = Vec::with_capacity(max_rooms);
    let open_slots = Arc::new(tokio::sync::Semaphore::new(open_concurrency));
    for (index, doc) in docs[..max_rooms].iter().enumerate() {
        let permit = open_slots
            .clone()
            .acquire_owned()
            .await
            .expect("open semaphore");
        let room_index = index;
        let hub_for_open = hub.clone();
        let doc = doc.clone();
        let room = tokio::spawn(async move {
            let peers = open_room_peers(addr, &doc, peers, room_index, hub_for_open).await;
            drop(permit);
            peers
        })
        .await
        .expect("open room task");
        writer_rooms.push((room.routing_key.clone(), room.writer));
        reader_rooms.push(room.reader);
        if (index + 1) % 25 == 0 || index + 1 == max_rooms {
            eprintln!(
                "probe: opened {}/{} rooms (child_rss={})",
                index + 1,
                max_rooms,
                sum_live_children_rss_bytes()
            );
        }
    }
    eprintln!(
        "probe: opened {} rooms × {} peers in {:?} (child_rss={})",
        max_rooms,
        peers,
        open_started.elapsed(),
        sum_live_children_rss_bytes()
    );

    for (_, ws) in &writer_rooms {
        drain_ws_locked(ws, Duration::from_millis(200)).await;
    }
    for ws in &reader_rooms {
        drain_ws_locked(ws, Duration::from_millis(200)).await;
    }

    let overflow = &docs[max_rooms];
    let overflow_key = room_key(overflow.workspace_id, overflow.document_id);
    let mut overflow_ws = connect_member(addr, &overflow.peers[0].token).await;
    assert!(
        try_auth_expect_1013(&mut overflow_ws, &overflow_key, 88_888).await,
        "room {} must close with retryable 1013",
        max_rooms + 1
    );

    let hostile = invalid_utf8_update_candidate();
    let mut latency_txs = Vec::with_capacity(max_rooms);
    let mut latency_joins = Vec::with_capacity(max_rooms);
    let latency_readers = reader_rooms.clone();
    for reader in latency_readers {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<SampleEvent>();
        latency_txs.push(tx);
        latency_joins.push(tokio::spawn(async move {
            let mut latencies_ms = Vec::new();
            let mut missed = 0usize;
            while let Some(event) = rx.recv().await {
                let mut guard = reader.lock().await;
                if wait_for_sync_update(&mut guard, LATENCY_WAIT).await {
                    latencies_ms.push(event.sent_at.elapsed().as_millis() as u64);
                } else {
                    missed += 1;
                }
            }
            (latencies_ms, missed)
        }));
    }

    let load_started = Instant::now();
    let mut edits = 0u64;
    let mut tick = 0u64;
    let mut closes_1011 = 0u64;
    while load_started.elapsed() < duration {
        let tick_started = Instant::now();
        let edit = marker_edit(tick);
        join_all(
            reader_rooms
                .iter()
                .map(|reader| drain_ws_locked(reader, Duration::from_millis(10))),
        )
        .await;
        for (index, (routing_key, writer)) in writer_rooms.iter().enumerate() {
            let mut writer = writer.lock().await;
            let sent_at = Instant::now();
            if writer
                .send(Message::Binary(
                    sync_update_frame(routing_key, &edit).into(),
                ))
                .await
                .is_err()
            {
                if let Some(code) = read_close_code(&mut writer, Duration::from_millis(200)).await {
                    if code == 1011 {
                        closes_1011 += 1;
                    }
                }
                eprintln!("probe: writer send failed room_index={index}");
                continue;
            }
            let _ = latency_txs[index].send(SampleEvent { sent_at });
            edits += 1;
        }
        tick += 1;
        let elapsed = tick_started.elapsed();
        if elapsed < Duration::from_secs(1) {
            tokio::time::sleep(Duration::from_secs(1) - elapsed).await;
        }
    }
    for tx in latency_txs {
        drop(tx);
    }
    let mut latencies_ms = Vec::new();
    let mut latency_missed = 0usize;
    for join in latency_joins {
        let (room_latencies, room_missed) = join.await.expect("room latency task");
        latencies_ms.extend(room_latencies);
        latency_missed += room_missed;
    }

    let mut lost_writers = 0usize;
    for (index, (routing_key, writer)) in writer_rooms.iter().enumerate() {
        drain_ws_locked(writer, Duration::from_millis(100)).await;
        if !writer_alive_locked(
            writer,
            routing_key,
            &marker_edit(tick),
            Duration::from_secs(3),
        )
        .await
        {
            lost_writers += 1;
            eprintln!("probe: writer lost after load room_index={index}");
        }
    }

    let achieved_rate = edits as f64 / duration.as_secs_f64() / max_rooms as f64;
    assert!(
        achieved_rate >= 0.95,
        "achieved edit rate {:.3}/s/room below 0.95 (edits={} rooms={} duration_s={})",
        achieved_rate,
        edits,
        max_rooms,
        duration.as_secs()
    );
    assert_eq!(lost_writers, 0, "writers lost under load");
    assert_eq!(closes_1011, 0, "connections closed with 1011 under load");

    let hostile_sample_indices = hostile_sample_indices(max_rooms);
    if max_rooms >= 64 {
        assert_eq!(
            hostile_sample_indices.len(),
            5,
            "merge bar expects five distinct non-victim hostile samples"
        );
    }
    let victim_doc = &docs[0];
    let victim_key = room_key(victim_doc.workspace_id, victim_doc.document_id);
    let mut pre_hostile_writer_ok = HashMap::new();
    for index in &hostile_sample_indices {
        let index = *index;
        let (routing_key, writer) = &writer_rooms[index];
        let ok = writer_alive_locked(
            writer,
            routing_key,
            &marker_edit(tick + 1),
            Duration::from_secs(3),
        )
        .await;
        pre_hostile_writer_ok.insert(index, ok);
        eprintln!("probe: pre-hostile room_index={index} writer_alive={ok}");
    }

    {
        let (victim_routing_key, victim_writer) = &writer_rooms[0];
        let mut victim_writer = victim_writer.lock().await;
        victim_writer
            .send(Message::Binary(
                sync_update_frame(victim_routing_key, &hostile).into(),
            ))
            .await
            .unwrap();
        let _ = wait_for_sync_applied(&mut victim_writer, Duration::from_secs(3)).await;
    }

    for (_, ws) in &writer_rooms {
        drain_ws_locked(ws, Duration::from_millis(200)).await;
    }
    for ws in &reader_rooms {
        drain_ws_locked(ws, Duration::from_millis(200)).await;
    }

    let hostile_started = Instant::now();
    let mut healthy = 0usize;
    for index in &hostile_sample_indices {
        let index = *index;
        let (routing_key, writer) = &writer_rooms[index];
        if writer_alive_locked(
            writer,
            routing_key,
            &marker_edit(tick + 2),
            Duration::from_secs(3),
        )
        .await
        {
            healthy += 1;
        } else {
            eprintln!(
                "probe: hostile sample room_index={index} unhealthy (pre_hostile={})",
                pre_hostile_writer_ok.get(&index).copied().unwrap_or(false)
            );
        }
    }
    let mut recovery_ws = connect_member(addr, &victim_doc.peers[0].token).await;
    assert!(
        probe_auth_and_join(&mut recovery_ws, &victim_key, 99_001)
            .await
            .is_ok(),
        "hostile victim room must accept a fresh writer connection"
    );
    assert!(
        writer_alive(
            &mut recovery_ws,
            &victim_key,
            &marker_edit(tick + 3),
            Duration::from_secs(5),
        )
        .await,
        "hostile victim room must recover and accept edits on a new connection"
    );
    assert_eq!(
        healthy,
        hostile_sample_indices.len(),
        "hostile update must not kill unrelated rooms; healthy={healthy}/{} pre_states={:?}",
        hostile_sample_indices.len(),
        pre_hostile_writer_ok
    );
    eprintln!(
        "probe: hostile room isolated in {:?}; {}/{} sample rooms still editing",
        hostile_started.elapsed(),
        healthy,
        hostile_sample_indices.len()
    );

    let reuse_doc = &docs[max_rooms];
    let reuse_key = room_key(reuse_doc.workspace_id, reuse_doc.document_id);
    drop(recovery_ws);
    if let Some((_, writer)) = writer_rooms.first() {
        let _ = writer.lock().await.close(None).await;
    }
    if let Some(reader) = reader_rooms.first() {
        let _ = reader.lock().await.close(None).await;
    }
    writer_rooms.remove(0);
    reader_rooms.remove(0);
    assert!(
        wait_for_room_slot(&hub, Duration::from_secs(60)).await,
        "hub must free a room slot after leave/eviction"
    );
    let mut reuse_ws = connect_member(addr, &reuse_doc.peers[0].token).await;
    assert!(
        probe_auth_and_join(&mut reuse_ws, &reuse_key, 77_777)
            .await
            .is_ok(),
        "slot must be reusable after leave/eviction (room {})",
        max_rooms + 1
    );
    assert_eq!(
        hub.available_room_slots(),
        0,
        "reused slot should be occupied after room {} joins",
        max_rooms + 1
    );

    assert_eq!(
        latency_missed,
        0,
        "latency sampler missed {latency_missed} ordinary load edits (expected one sample per edit per room)"
    );
    let expected_latency_samples = edits;
    assert_eq!(
        latencies_ms.len(),
        expected_latency_samples as usize,
        "latency sample count {} != edits {}",
        latencies_ms.len(),
        expected_latency_samples
    );
    latencies_ms.sort_unstable();
    let (threads, fds) = proc_threads_fds();
    let child_rss = sum_live_children_rss_bytes();
    let p50_apply_broadcast = percentile(&latencies_ms, 50);
    let p95_apply_broadcast = percentile(&latencies_ms, 95);
    let p99_apply_broadcast = percentile(&latencies_ms, 99);
    assert!(
        p95_apply_broadcast <= 300,
        "p95 apply->broadcast {} ms exceeds 300 ms bar (p50={} p99={} samples={})",
        p95_apply_broadcast,
        p50_apply_broadcast,
        p99_apply_broadcast,
        latencies_ms.len()
    );
    eprintln!(
        "PROBE_SUMMARY rooms={} peers={} duration_s={} edits={} achieved_rate_per_room={:.3} child_rss_bytes={} p50_apply_broadcast_ms={} p95_apply_broadcast_ms={} p99_apply_broadcast_ms={} latency_samples={} server_threads={} server_fds={} open_ms={} load_ms={} lost_writers={} closes_1011={} db_pool={}",
        max_rooms,
        peers,
        duration.as_secs(),
        edits,
        achieved_rate,
        child_rss,
        p50_apply_broadcast,
        p95_apply_broadcast,
        p99_apply_broadcast,
        latencies_ms.len(),
        threads,
        fds,
        open_started.elapsed().as_millis(),
        load_started.elapsed().as_millis(),
        lost_writers,
        closes_1011,
        db_pool_size
    );
    eprintln!(
        "PROBE_STAGE note=see collab.stage tracing lines in this log for validate/auth_tx/append_tx/apply/broadcast breakdown"
    );

    run.finish().await.expect("probe cleanup");
}
