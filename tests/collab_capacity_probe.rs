//! Heavy collab capacity probe. Run only via `scripts/collab-capacity-probe.sh`.
#![cfg(feature = "db-tests")]

use std::sync::Arc;
use std::time::{Duration, Instant};

use collab_engine::process::{
    max_child_concurrency, raise_nofile_to_hard_limit, sum_live_children_rss_bytes,
};
use futures_util::{SinkExt, StreamExt};
use fvoci_server::collab::config::derive_max_child_concurrency;
use fvoci_server::collab::hub::CollabHub;
use fvoci_server::collab::wire::{
    encode, AuthMessage, CollabKind, CollabRoomName, DocumentMessage, WireFrame,
};
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::Message;

mod support;

use support::{
    complete_sync_handshake, connect_member, engine_fixture, invalid_utf8_update_candidate,
    setup_wiki_doc_batch, sync_update_frame, test_collab_config, wait_for_sync_applied,
    wait_for_sync_update, TestDb, TestRun,
};

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

fn probe_rooms() -> usize {
    std::env::var("COLLAB_PROBE_ROOMS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(200)
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

fn probe_config(max_rooms: usize, peers: usize) -> fvoci_server::collab::config::CollabConfig {
    let mut cfg = test_collab_config(max_rooms, 3_000);
    cfg.max_child_concurrency = derive_max_child_concurrency(max_rooms);
    cfg.memory_budget_bytes = 8 * 1024 * 1024 * 1024;
    // One workspace/session backs all probe documents; parallel opens need headroom.
    cfg.max_collab_sockets = max_rooms * peers + 64;
    cfg.max_collab_sockets_per_session = max_rooms * peers + 64;
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

struct RoomPeers {
    routing_key: String,
    writers: Vec<Ws>,
    readers: Vec<Ws>,
}

fn probe_open_failure_diag(hub: &Arc<CollabHub>, room_index: usize, reason: &str) {
    eprintln!(
        "probe open failure room_index={} reason={} child_cap={} child_rss={} room_slots={} collab_sockets={} memory_budget_gib={}",
        room_index,
        reason,
        max_child_concurrency(),
        sum_live_children_rss_bytes(),
        hub.available_room_slots(),
        hub.available_collab_sockets(),
        hub.config().memory_budget_bytes / (1024 * 1024 * 1024)
    );
}

async fn open_room_peers(
    addr: std::net::SocketAddr,
    token: &str,
    routing_key: &str,
    peers: usize,
    room_index: usize,
    hub: Arc<CollabHub>,
) -> RoomPeers {
    let mut writers = Vec::with_capacity(peers);
    let mut readers = Vec::with_capacity(peers);
    let base_id = (room_index as u32 + 1) * 100;
    for peer in 0..peers {
        let client_id = base_id + peer as u32;
        let mut ws = connect_member(addr, token).await;
        let mut last_err = String::from("no attempts");
        let authed = {
            let mut ok = false;
            for attempt in 0..12 {
                match probe_auth_and_join(&mut ws, routing_key, client_id).await {
                    Ok(()) => {
                        ok = true;
                        break;
                    }
                    Err(reason) => {
                        last_err = reason;
                        ws = connect_member(addr, token).await;
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
            complete_sync_handshake(&mut ws, routing_key).await;
        }
        if peer == 0 {
            writers.push(ws);
        } else {
            readers.push(ws);
        }
    }
    // With peers=2: writer + one reader for broadcast measurement.
    if peers == 1 {
        readers.push(writers.pop().expect("writer"));
    }
    RoomPeers {
        routing_key: routing_key.to_string(),
        writers,
        readers,
    }
}

async fn room_still_edits(room: &mut RoomPeers, edit: &[u8], within: Duration) -> bool {
    let writer = room.writers.first_mut().expect("writer");
    let reader = room.readers.first_mut().expect("reader");
    writer
        .send(Message::Binary(
            sync_update_frame(&room.routing_key, edit).into(),
        ))
        .await
        .unwrap();
    wait_for_sync_applied(writer, within).await && wait_for_sync_update(reader, within).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 32)]
async fn collab_capacity_probe() {
    raise_nofile_to_hard_limit();
    let max_rooms = probe_rooms();
    let peers = probe_peers();
    let duration = probe_duration();
    let open_concurrency = probe_open_concurrency();

    let harness = TestDb::bootstrap().await;
    let mut run = TestRun::new(harness);
    let cfg = probe_config(max_rooms, peers);
    let app_url = run.harness.app_url.clone();
    let db_pool_size = (max_rooms as u32).clamp(128, 256);
    let addr = run
        .spawn_router_with_pool(&app_url, cfg.clone(), db_pool_size)
        .await;
    let hub = run.hub();

    eprintln!(
        "probe config: rooms={} peers={} duration_s={} open_concurrency={} max_child_concurrency={} memory_budget_gib={} db_pool={}",
        max_rooms,
        peers,
        duration.as_secs(),
        open_concurrency,
        derive_max_child_concurrency(max_rooms),
        cfg.memory_budget_bytes / (1024 * 1024 * 1024),
        db_pool_size
    );

    let docs_started = Instant::now();
    let docs = setup_wiki_doc_batch(&run.harness, max_rooms + 1).await;
    eprintln!(
        "probe: created {} documents in {:?}",
        docs.len(),
        docs_started.elapsed()
    );

    let open_started = Instant::now();
    let mut rooms: Vec<RoomPeers> = Vec::with_capacity(max_rooms);
    let open_slots = Arc::new(tokio::sync::Semaphore::new(open_concurrency));
    for (index, doc) in docs[..max_rooms].iter().enumerate() {
        let routing_key = room_key(doc.session.workspace_id, doc.document_id);
        let token = doc.session.session_token.clone();
        let permit = open_slots
            .clone()
            .acquire_owned()
            .await
            .expect("open semaphore");
        let room_index = index;
        let hub_for_open = hub.clone();
        let room = tokio::spawn(async move {
            let peers =
                open_room_peers(addr, &token, &routing_key, peers, room_index, hub_for_open).await;
            drop(permit);
            peers
        })
        .await
        .expect("open room task");
        rooms.push(room);
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
        rooms.len(),
        peers,
        open_started.elapsed(),
        sum_live_children_rss_bytes()
    );

    let overflow = &docs[max_rooms];
    let overflow_key = room_key(overflow.session.workspace_id, overflow.document_id);
    let mut overflow_ws = connect_member(addr, &overflow.session.session_token).await;
    assert!(
        try_auth_expect_1013(&mut overflow_ws, &overflow_key, 88_888).await,
        "room {} must close with retryable 1013",
        max_rooms + 1
    );

    let edit_a = engine_fixture("pending_u1.v1");
    let edit_b = engine_fixture("pending_u2.v1");
    let hostile = invalid_utf8_update_candidate();
    tokio::time::sleep(Duration::from_secs(2)).await;
    let mut latencies_ms = Vec::new();
    let load_started = Instant::now();
    let mut edits = 0u64;
    let mut tick = 0u64;
    let latency_sample_indices: Vec<usize> = [0, 10, 25, 50, 75, 100, 125, 150, 175, 199]
        .into_iter()
        .map(|i| i.min(max_rooms - 1))
        .collect();
    while load_started.elapsed() < duration {
        let tick_started = Instant::now();
        let edit = if tick.is_multiple_of(2) {
            &edit_a
        } else {
            &edit_b
        };
        let mut sample_send_at = Vec::new();
        for (index, room) in rooms.iter_mut().enumerate() {
                let writer = room.writers.first_mut().expect("writer");
                let sent_at = Instant::now();
                if writer
                    .send(Message::Binary(
                        sync_update_frame(&room.routing_key, edit).into(),
                    ))
                    .await
                    .is_err()
                {
                    eprintln!("probe: writer send failed room_index={index}");
                    continue;
                }
                if latency_sample_indices.contains(&index) {
                    sample_send_at.push((index, sent_at));
                }
                edits += 1;
        }
        for (index, sent_at) in sample_send_at {
            let reader = rooms[index].readers.first_mut().expect("reader");
            if wait_for_sync_update(reader, Duration::from_millis(800)).await {
                latencies_ms.push(sent_at.elapsed().as_millis() as u64);
            }
        }
        tick += 1;
        let elapsed = tick_started.elapsed();
        if elapsed < Duration::from_secs(1) {
            tokio::time::sleep(Duration::from_secs(1) - elapsed).await;
        }
    }

    let hostile_started = Instant::now();
    let victim = &mut rooms[0];
    victim.writers[0]
        .send(Message::Binary(
            sync_update_frame(&victim.routing_key, &hostile).into(),
        ))
        .await
        .unwrap();
    let _ = wait_for_sync_applied(&mut victim.writers[0], Duration::from_secs(3)).await;
    let sample_indices = [25, 50, 100, 150, 199].map(|i| i.min(max_rooms - 1));
    let mut healthy = 0usize;
    for index in sample_indices {
        if room_still_edits(&mut rooms[index], &edit_a, Duration::from_secs(3)).await {
            healthy += 1;
        }
    }
    assert!(
        healthy >= sample_indices.len() - 1,
        "hostile update must not kill unrelated rooms; healthy={healthy}/{}",
        sample_indices.len()
    );
    eprintln!(
        "probe: hostile room isolated in {:?}; {}/{} sample rooms still editing",
        hostile_started.elapsed(),
        healthy,
        sample_indices.len()
    );

    // Free one slot: drop all peers on room 0 and wait for idle eviction.
    let reuse_doc = &docs[max_rooms];
    let reuse_key = room_key(reuse_doc.session.workspace_id, reuse_doc.document_id);
    rooms.remove(0);
    assert!(
        wait_for_room_slot(&hub, Duration::from_secs(30)).await,
        "hub must free a room slot after leave/eviction"
    );
    let mut reuse_ws = connect_member(addr, &reuse_doc.session.session_token).await;
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

    latencies_ms.sort_unstable();
    let (threads, fds) = proc_threads_fds();
    let child_rss = sum_live_children_rss_bytes();
    eprintln!(
        "PROBE_SUMMARY rooms={} peers={} duration_s={} edits={} child_rss_bytes={} p95_apply_broadcast_ms={} p50_apply_broadcast_ms={} server_threads={} server_fds={} open_ms={} load_ms={}",
        max_rooms,
        peers,
        duration.as_secs(),
        edits,
        child_rss,
        percentile(&latencies_ms, 95),
        percentile(&latencies_ms, 50),
        threads,
        fds,
        open_started.elapsed().as_millis(),
        load_started.elapsed().as_millis()
    );

    run.finish().await.expect("probe cleanup");
}
