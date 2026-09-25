//! Heavy collab capacity probe. Run only via `scripts/collab-capacity-probe.sh`.
#![cfg(feature = "db-tests")]

use std::sync::Arc;
use std::time::{Duration, Instant};

use collab_engine::process::sum_live_children_rss_bytes;
use fvoci_server::collab::config::derive_max_child_concurrency;
use fvoci_server::collab::room::{ConnectionLease, JoinError, RoomJoin};
use fvoci_server::collab::CollabHub;
use fvoci_server::db::pool;
use tokio::sync::mpsc;
use uuid::Uuid;

mod support;

use fvoci_server::collab::wire::{CollabKind, CollabRoomName};
use support::{engine_fixture, setup_wiki_doc, sync_update_frame, test_collab_config, TestDb};

fn room_key(workspace_id: Uuid, document_id: Uuid) -> String {
    CollabRoomName {
        workspace_id,
        kind: CollabKind::Document,
        resource_id: document_id,
    }
    .routing_key()
}

fn probe_rooms() -> usize {
    std::env::var("COLLAB_PROBE_ROOMS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20)
}

fn probe_duration() -> Duration {
    let secs = std::env::var("COLLAB_PROBE_DURATION_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(180);
    Duration::from_secs(secs.max(60))
}

#[derive(Debug)]
struct OpenRoom {
    conn_id: Uuid,
    #[allow(dead_code)]
    lease: ConnectionLease,
    workspace_id: Uuid,
    document_id: Uuid,
}

async fn hub_join_room(
    hub: &CollabHub,
    wiki: &support::WikiDocFixture,
    client_id: u32,
) -> Result<OpenRoom, JoinError> {
    let conn_id = Uuid::now_v7();
    let (events_tx, mut events_rx) = mpsc::channel(8);
    tokio::spawn(async move { while events_rx.recv().await.is_some() {} });
    let routing_key = room_key(wiki.session.workspace_id, wiki.document_id);
    let join = RoomJoin {
        conn: fvoci_server::collab::room::AuthenticatedConnection {
            conn_id,
            session: fvoci_server::collab::room::CollabSession {
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
        .join_room((wiki.session.workspace_id, wiki.document_id), join)
        .await?;
    Ok(OpenRoom {
        conn_id,
        lease,
        workspace_id: wiki.session.workspace_id,
        document_id: wiki.document_id,
    })
}

#[tokio::test]
async fn collab_capacity_probe() {
    let max_rooms = probe_rooms();
    let duration = probe_duration();
    let harness = TestDb::bootstrap().await;
    let mut cfg = test_collab_config(max_rooms, 120_000);
    cfg.max_child_concurrency = derive_max_child_concurrency(max_rooms);
    cfg.memory_budget_bytes = 8 * 1024 * 1024 * 1024;
    let pool = pool::connect_app(&harness.app_url).await.expect("pool");
    let hub = Arc::new(CollabHub::new(cfg, pool));
    let mut open_rooms = Vec::with_capacity(max_rooms);
    let edit = engine_fixture("pending_u1.v1");

    let open_started = Instant::now();
    for index in 0..max_rooms {
        let wiki = setup_wiki_doc(&harness).await;
        let room = hub_join_room(&hub, &wiki, (index + 1) as u32)
            .await
            .expect("open room");
        open_rooms.push(room);
    }
    eprintln!(
        "probe: opened {} rooms in {:?}",
        max_rooms,
        open_started.elapsed()
    );

    let overflow_doc = setup_wiki_doc(&harness).await;
    let overflow = hub_join_room(&hub, &overflow_doc, 99_999).await;
    assert!(
        matches!(overflow, Err(JoinError::RoomFull)),
        "room {} must refuse with RoomFull, got {:?}",
        max_rooms + 1,
        overflow
    );

    let load_started = Instant::now();
    let mut edits = 0u64;
    let mut latencies_ms = Vec::new();
    while load_started.elapsed() < duration {
        for room in open_rooms.iter() {
            let t0 = Instant::now();
            let routing = room_key(room.workspace_id, room.document_id);
            let frame = sync_update_frame(&routing, &edit);
            hub.send_frame((room.workspace_id, room.document_id), room.conn_id, frame)
                .await;
            latencies_ms.push(t0.elapsed().as_millis() as u64);
            edits += 1;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    latencies_ms.sort_unstable();
    let p95 = if latencies_ms.is_empty() {
        0
    } else {
        latencies_ms[(latencies_ms.len() * 95 / 100).min(latencies_ms.len() - 1)]
    };
    let child_rss = sum_live_children_rss_bytes();
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let threads = status
        .lines()
        .find_map(|line| line.strip_prefix("Threads:"))
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(0);
    let fds = std::fs::read_dir("/proc/self/fd")
        .map(|entries| entries.count())
        .unwrap_or(0);

    eprintln!(
        "probe metrics: child_rss_bytes={} edits={} p95_hub_send_ms={} threads={} fds={} duration_s={}",
        child_rss,
        edits,
        p95,
        threads,
        fds,
        duration.as_secs()
    );

    hub.shutdown().await;
    let _ = harness.cleanup().await;
}
