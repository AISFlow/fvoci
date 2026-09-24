#![cfg(feature = "db-tests")]

#[allow(dead_code)]
mod support;

use std::panic::{resume_unwind, AssertUnwindSafe};
use std::sync::Arc;
use std::time::Duration;

use futures_util::future::BoxFuture;
use futures_util::FutureExt;
use fvoci_server::collab::hub::{
    arm_hub_join_barrier, arm_reclaim_barrier, disarm_hub_join_barrier, disarm_reclaim_barrier,
    room_start_count, CollabHub, IdleEvictDecision, IdleEvictionHold, RoomLifecyclePhase,
    HUB_JOIN_BARRIER_AFTER_ACTOR_REPLY, HUB_JOIN_BARRIER_AFTER_SLOT_READY,
    HUB_JOIN_BARRIER_BEFORE_ACTOR_JOIN,
};
use fvoci_server::collab::room::{
    arm_actor_panic_after_join_barrier, arm_actor_panic_on_next_frame, arm_engine_stop_witness,
    arm_join_barrier, arm_join_channel_admission_witness, arm_join_reply_barrier,
    arm_teardown_barrier, disarm_actor_panic_after_join_barrier, disarm_actor_panic_on_next_frame,
    disarm_engine_stop_witness, disarm_join_barrier, disarm_join_channel_admission_witness,
    disarm_teardown_barrier, join_delivery_attempt_count, AuthenticatedConnection, CollabSession,
    ConnectionLease, JoinError, RoomClientEvent, RoomJoin,
};
use fvoci_server::collab::wire::{CollabKind, CollabRoomName, DocumentMessage, WireFrame};
use fvoci_server::db::collab::COLLAB_ROOM_SESSION_LOCK_NAMESPACE;
use fvoci_server::db::context::lock_key_from_uuid;
use sqlx::postgres::PgPoolOptions;
use support::{setup_wiki_doc, sync_update_frame, test_collab_config, TestDb, TestRun};
use tokio::sync::mpsc;
use uuid::Uuid;

const TEST_TIMEOUT: Duration = Duration::from_secs(30);
const SAMPLE_HI_UPDATE: &str = "0101e8eda5a2070004010b70726f73656d6972726f7202686900";

struct LifecycleRun {
    inner: TestRun,
    hubs: Vec<Arc<CollabHub>>,
    leases: Vec<ConnectionLease>,
}

impl LifecycleRun {
    fn new(harness: TestDb) -> Self {
        Self {
            inner: TestRun::new(harness),
            hubs: Vec::new(),
            leases: Vec::new(),
        }
    }

    fn register_hub(&mut self, hub: Arc<CollabHub>) -> Arc<CollabHub> {
        self.hubs.push(hub.clone());
        hub
    }

    fn retain_lease(&mut self, lease: ConnectionLease) {
        self.leases.push(lease);
    }

    async fn finish(self) -> Result<(), String> {
        let mut errors = Vec::new();
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

async fn run_lifecycle_test<F>(name: &str, case: F)
where
    F: for<'a> FnOnce(&'a mut LifecycleRun) -> BoxFuture<'a, ()>,
{
    let mut run = LifecycleRun::new(TestDb::bootstrap().await);
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

async fn setup_second_doc(
    _run: &LifecycleRun,
    wiki: &support::WikiDocFixture,
) -> support::WikiDocFixture {
    use fvoci_server::db::documents::CreateDocumentInput;
    let created = fvoci_server::db::documents::create_wiki_document(
        &wiki.session.pool,
        wiki.session.workspace_id,
        wiki.session.user_id,
        wiki.session.session_id,
        CreateDocumentInput {
            parent_id: None,
            title: "Lifecycle doc B",
            icon: None,
        },
        None,
    )
    .await
    .expect("create second doc")
    .expect("created");
    support::WikiDocFixture {
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

fn clone_wiki(wiki: &support::WikiDocFixture) -> support::WikiDocFixture {
    support::WikiDocFixture {
        session: support::SessionFixture {
            pool: wiki.session.pool.clone(),
            user_id: wiki.session.user_id,
            session_id: wiki.session.session_id,
            workspace_id: wiki.session.workspace_id,
            session_token: wiki.session.session_token.clone(),
        },
        document_id: wiki.document_id,
    }
}

async fn hub_join_with_lease(
    hub: &CollabHub,
    wiki: &support::WikiDocFixture,
    client_id: u32,
) -> Result<(Uuid, ConnectionLease), JoinError> {
    hub_join_with_id(hub, wiki, client_id, Uuid::now_v7()).await
}

async fn hub_join_with_id(
    hub: &CollabHub,
    wiki: &support::WikiDocFixture,
    client_id: u32,
    conn_id: Uuid,
) -> Result<(Uuid, ConnectionLease), JoinError> {
    let (events_tx, mut events_rx) = mpsc::channel(8);
    tokio::spawn(async move { while events_rx.recv().await.is_some() {} });
    let routing_key = CollabRoomName {
        workspace_id: wiki.session.workspace_id,
        kind: CollabKind::Document,
        resource_id: wiki.document_id,
    }
    .routing_key();
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
        .join_room(room_key(wiki.session.workspace_id, wiki.document_id), join)
        .await?;
    Ok((conn_id, lease))
}

async fn hub_join(
    run: &mut LifecycleRun,
    hub: &CollabHub,
    wiki: &support::WikiDocFixture,
    client_id: u32,
) -> Result<Uuid, JoinError> {
    let (id, lease) = hub_join_with_lease(hub, wiki, client_id).await?;
    run.retain_lease(lease);
    Ok(id)
}

async fn wait_for_joining_count(hub: &CollabHub, key: (Uuid, Uuid), expected: usize) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if hub.room_joining_count(key).await == expected {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("room joining count did not reach expected value");
}

async fn wait_for_probe_connections(hub: &CollabHub, key: (Uuid, Uuid), expected: usize) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if hub.probe_actor(key).await.connections == expected {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("actor connection count did not reach expected value");
}

async fn wait_for_probe_awareness_clients(hub: &CollabHub, key: (Uuid, Uuid), expected: usize) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if hub.probe_actor(key).await.awareness_clients == expected {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("actor awareness registry count did not reach expected value");
}

async fn wait_for_member_count(hub: &CollabHub, key: (Uuid, Uuid), expected: usize) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if hub.room_member_count(key).await == expected {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("room member count did not reach expected value");
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
    .unwrap_or_else(|_| panic!("room phase {expected:?} not observed"));
}

async fn admin_pool(admin_url: &str) -> sqlx::PgPool {
    PgPoolOptions::new()
        .max_connections(4)
        .connect(admin_url)
        .await
        .expect("admin pool")
}

async fn tail_seq(admin: &sqlx::PgPool, document_id: Uuid) -> i64 {
    sqlx::query_scalar("SELECT tail_seq FROM fvoci.document_states WHERE document_id = $1")
        .bind(document_id)
        .fetch_optional(admin)
        .await
        .expect("tail_seq")
        .unwrap_or(0)
}

async fn tail_row_count(admin: &sqlx::PgPool, document_id: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM fvoci.document_collab_updates WHERE document_id = $1")
        .bind(document_id)
        .fetch_one(admin)
        .await
        .expect("tail rows")
}

async fn room_guard_held(admin: &sqlx::PgPool, document_id: Uuid) -> bool {
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

async fn wait_until_guard(admin: &sqlx::PgPool, document_id: Uuid, held: bool) {
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

async fn hub_join_with_events(
    hub: &CollabHub,
    wiki: &support::WikiDocFixture,
    client_id: u32,
) -> Result<(Uuid, ConnectionLease, mpsc::Receiver<RoomClientEvent>), JoinError> {
    let conn_id = Uuid::now_v7();
    let (events_tx, events_rx) = mpsc::channel(8);
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
    Ok((conn_id, lease, events_rx))
}

async fn wait_for_close(events_rx: &mut mpsc::Receiver<RoomClientEvent>, code: u16) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(event) = events_rx.recv().await {
            match event {
                RoomClientEvent::Close { code: got, .. } if got == code => return,
                RoomClientEvent::Outbound(frame) => {
                    if let Ok(WireFrame::Document {
                        message: DocumentMessage::Auth(_)
                            | DocumentMessage::SyncStatus { applied: true },
                        ..
                    }) = fvoci_server::collab::wire::decode(&frame.bytes)
                    {
                        panic!("unexpected auth or success ack during panic teardown");
                    }
                }
                _ => {}
            }
        }
        panic!("events channel closed without Close {code}");
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for Close {code}"));
}

async fn wait_for_sync_status_applied(events_rx: &mut mpsc::Receiver<RoomClientEvent>) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(event) = events_rx.recv().await {
            let RoomClientEvent::Outbound(frame) = event else {
                continue;
            };
            if let Ok(WireFrame::Document {
                message: DocumentMessage::SyncStatus { applied: true },
                ..
            }) = fvoci_server::collab::wire::decode(&frame.bytes)
            {
                return;
            }
        }
        panic!("events channel closed without SyncStatus applied:true");
    })
    .await
    .expect("committed update must ack with SyncStatus applied:true");
}

async fn assert_no_sync_status_applied(events_rx: &mut mpsc::Receiver<RoomClientEvent>) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(event) = events_rx.recv().await {
            if let RoomClientEvent::Outbound(frame) = event {
                if let Ok(WireFrame::Document {
                    message: DocumentMessage::SyncStatus { applied: true },
                    ..
                }) = fvoci_server::collab::wire::decode(&frame.bytes)
                {
                    panic!("uncommitted candidate must not receive SyncStatus applied:true");
                }
            }
        }
    })
    .await
    .expect("actor teardown must close the event channel");
}

#[tokio::test]
async fn collab_lifecycle_blocked_join_does_not_hold_phase() {
    run_lifecycle_test("collab_lifecycle_blocked_join_does_not_hold_phase", |run| {
        Box::pin(async {
            let wiki = setup_wiki_doc(&run.inner.harness).await;
            let doc_b = setup_second_doc(run, &wiki).await;
            let hub = run.register_hub(Arc::new(CollabHub::new(
                test_collab_config(4, 200),
                wiki.session.pool.clone(),
            )));
            let key_a = room_key(wiki.session.workspace_id, wiki.document_id);
            let key_b = room_key(doc_b.session.workspace_id, doc_b.document_id);

            let member_a = hub_join(run, &hub, &wiki, 1).await.expect("first join");
            assert_eq!(hub.room_member_count(key_a).await, 1);

            let (reached_rx, proceed_tx) = arm_join_barrier(wiki.document_id).await;
            let join_b = tokio::spawn({
                let hub = hub.clone();
                let wiki = clone_wiki(&wiki);
                async move { hub_join_with_lease(&hub, &wiki, 2).await }
            });
            tokio::time::timeout(Duration::from_secs(5), reached_rx)
                .await
                .expect("join barrier must be reached inside actor handle_join")
                .expect("barrier signal");

            assert_eq!(
                hub.room_lifecycle_phase(key_a).await,
                RoomLifecyclePhase::Live,
                "phase lock must not be held across blocked actor join"
            );
            assert_eq!(hub.room_joining_count(key_a).await, 1);

            hub.send_frame(key_a, member_a, vec![0x01, 0x02]).await;
            hub.leave_room(key_a, Uuid::now_v7()).await;
            assert_eq!(
                hub.room_member_count(key_a).await,
                1,
                "foreign leave must not remove the live member"
            );

            let member_b_room = hub_join(run, &hub, &doc_b, 10)
                .await
                .expect("other room join");
            hub.leave_room(key_b, member_b_room).await;
            wait_for_member_count(&hub, key_b, 0).await;
            hub.force_room_idle_eligible(key_b).await;
            assert!(hub.execute_idle_evict_if_eligible(key_b).await);
            assert_eq!(
                hub.room_lifecycle_phase(key_b).await,
                RoomLifecyclePhase::Absent
            );

            proceed_tx.send(()).expect("release join barrier");
            let (member_b, lease_b) = join_b.await.expect("join task").expect("second join");
            run.retain_lease(lease_b);
            assert_ne!(member_b, member_a);
            disarm_join_barrier(wiki.document_id).await;
        })
    })
    .await;
}

#[tokio::test]
async fn collab_lifecycle_aborted_join_clears_actor_ownership() {
    run_lifecycle_test(
        "collab_lifecycle_aborted_join_clears_actor_ownership",
        |run| {
            Box::pin(async {
                let wiki = setup_wiki_doc(&run.inner.harness).await;
                let hub = run.register_hub(Arc::new(CollabHub::new(
                    test_collab_config(4, 200),
                    wiki.session.pool.clone(),
                )));
                let key = room_key(wiki.session.workspace_id, wiki.document_id);
                const ABORTED_CLIENT_ID: u32 = 42;

                let member = hub_join(run, &hub, &wiki, 1).await.expect("seed member");
                assert_eq!(hub.room_member_count(key).await, 1);
                assert_eq!(hub.probe_actor(key).await.connections, 1);

                let (reached_rx, proceed_tx) = arm_join_barrier(wiki.document_id).await;
                let join_task = tokio::spawn({
                    let hub = hub.clone();
                    let wiki = clone_wiki(&wiki);
                    async move {
                        hub_join_with_lease(&hub, &wiki, ABORTED_CLIENT_ID)
                            .await
                            .map(|(id, _)| id)
                    }
                });
                tokio::time::timeout(Duration::from_secs(5), reached_rx)
                    .await
                    .expect("join barrier must be reached")
                    .expect("barrier signal");
                wait_for_joining_count(&hub, key, 1).await;

                join_task.abort();
                assert!(join_task.await.unwrap_err().is_cancelled());
                wait_for_joining_count(&hub, key, 0).await;

                proceed_tx.send(()).expect("release stranded barrier");
                wait_for_probe_connections(&hub, key, 1).await;
                assert_eq!(hub.room_member_count(key).await, 1);

                assert!(
                    hub_join(run, &hub, &wiki, ABORTED_CLIENT_ID).await.is_ok(),
                    "aborted join must release actor client-id capacity"
                );
                assert_eq!(hub.room_member_count(key).await, 2);
                assert_eq!(hub.probe_actor(key).await.connections, 2);

                disarm_join_barrier(wiki.document_id).await;
                hub.leave_room(key, member).await;
            })
        },
    )
    .await;
}

#[tokio::test]
async fn collab_lifecycle_abort_after_actor_reply_clears_connection() {
    run_lifecycle_test(
        "collab_lifecycle_abort_after_actor_reply_clears_connection",
        |run| {
            Box::pin(async {
                let wiki = setup_wiki_doc(&run.inner.harness).await;
                let hub = run.register_hub(Arc::new(CollabHub::new(
                    test_collab_config(4, 200),
                    wiki.session.pool.clone(),
                )));
                let key = room_key(wiki.session.workspace_id, wiki.document_id);
                const ABORTED_CLIENT_ID: u32 = 51;

                let member = hub_join(run, &hub, &wiki, 1).await.expect("seed member");
                assert_eq!(hub.room_member_count(key).await, 1);

                let (reached_rx, proceed_tx) =
                    arm_hub_join_barrier(wiki.document_id, HUB_JOIN_BARRIER_AFTER_ACTOR_REPLY)
                        .await;
                let join_task = tokio::spawn({
                    let hub = hub.clone();
                    let wiki = clone_wiki(&wiki);
                    async move { hub_join_with_lease(&hub, &wiki, ABORTED_CLIENT_ID).await }
                });
                tokio::time::timeout(Duration::from_secs(5), reached_rx)
                    .await
                    .expect("hub barrier after actor reply must be reached")
                    .expect("barrier signal");
                wait_for_member_count(&hub, key, 2).await;
                wait_for_probe_connections(&hub, key, 2).await;
                let probe_at_barrier = hub.probe_actor(key).await;
                assert_eq!(
                    probe_at_barrier.connections, 2,
                    "actor must admit the join before the caller receives the lease"
                );
                assert_eq!(
                    probe_at_barrier.awareness_clients, 2,
                    "join claims must register in awareness before caller receives lease"
                );

                join_task.abort();
                assert!(join_task.await.unwrap_err().is_cancelled());
                let _ = proceed_tx.send(());
                disarm_hub_join_barrier(wiki.document_id, HUB_JOIN_BARRIER_AFTER_ACTOR_REPLY).await;

                wait_for_joining_count(&hub, key, 0).await;
                wait_for_probe_connections(&hub, key, 1).await;
                wait_for_member_count(&hub, key, 1).await;
                let probe_after_abort = hub.probe_actor(key).await;
                assert_eq!(probe_after_abort.connections, 1);
                assert_eq!(
                    probe_after_abort.awareness_clients, 2,
                    "claim-only registry entries can outlive active connections after late lease drop"
                );

                assert!(
                    hub_join(run, &hub, &wiki, ABORTED_CLIENT_ID).await.is_ok(),
                    "post-reply abort must release actor connection and allow client-id reuse"
                );
                assert_eq!(hub.room_member_count(key).await, 2);
                wait_for_probe_connections(&hub, key, 2).await;
                wait_for_probe_awareness_clients(&hub, key, 2).await;
                hub.leave_room(key, member).await;
            })
        },
    )
    .await;
}

#[tokio::test]
async fn collab_lifecycle_drop_lease_without_leave_clears_connection() {
    run_lifecycle_test(
        "collab_lifecycle_drop_lease_without_leave_clears_connection",
        |run| {
            Box::pin(async {
                let wiki = setup_wiki_doc(&run.inner.harness).await;
                let hub = run.register_hub(Arc::new(CollabHub::new(
                    test_collab_config(4, 200),
                    wiki.session.pool.clone(),
                )));
                let key = room_key(wiki.session.workspace_id, wiki.document_id);

                let (_conn_id, lease) = hub_join_with_lease(&hub, &wiki, 1).await.expect("join");
                assert_eq!(hub.probe_actor(key).await.connections, 1);
                drop(lease);
                wait_for_probe_connections(&hub, key, 0).await;
                wait_for_member_count(&hub, key, 0).await;

                hub.force_room_idle_eligible(key).await;
                assert!(hub.execute_idle_evict_if_eligible(key).await);
                assert_eq!(
                    hub.room_lifecycle_phase(key).await,
                    RoomLifecyclePhase::Absent
                );
            })
        },
    )
    .await;
}

#[tokio::test]
async fn collab_lifecycle_joining_blocks_idle_eviction_then_rejoin_safe() {
    run_lifecycle_test(
        "collab_lifecycle_joining_blocks_idle_eviction_then_rejoin_safe",
        |run| {
            Box::pin(async {
                let wiki = setup_wiki_doc(&run.inner.harness).await;
                let hub = run.register_hub(Arc::new(CollabHub::new(
                    test_collab_config(4, 200),
                    wiki.session.pool.clone(),
                )));
                let key = room_key(wiki.session.workspace_id, wiki.document_id);

                let member = hub_join(run, &hub, &wiki, 1).await.expect("seed join");
                hub.leave_room(key, member).await;
                wait_for_member_count(&hub, key, 0).await;
                hub.force_room_idle_eligible(key).await;
                assert_eq!(
                    hub.idle_evict_decision(key).await,
                    IdleEvictDecision::WouldEvict
                );

                let (reached_rx, proceed_tx) = arm_join_barrier(wiki.document_id).await;
                let join_task = tokio::spawn({
                    let hub = hub.clone();
                    let wiki = clone_wiki(&wiki);
                    async move { hub_join_with_lease(&hub, &wiki, 2).await.map(|(id, _)| id) }
                });
                tokio::time::timeout(Duration::from_secs(5), reached_rx)
                    .await
                    .expect("join barrier must be reached")
                    .expect("barrier signal");
                wait_for_joining_count(&hub, key, 1).await;
                assert_eq!(
                    hub.idle_evict_decision(key).await,
                    IdleEvictDecision::DeferredJoining
                );
                assert!(
                    !hub.execute_idle_evict_if_eligible(key).await,
                    "idle eviction must not run while join is pending"
                );

                join_task.abort();
                let _ = join_task.await;
                wait_for_joining_count(&hub, key, 0).await;
                proceed_tx.send(()).expect("release join barrier");
                wait_for_probe_connections(&hub, key, 0).await;
                disarm_join_barrier(wiki.document_id).await;

                hub.force_room_idle_eligible(key).await;
                assert_eq!(
                    hub.idle_evict_decision(key).await,
                    IdleEvictDecision::WouldEvict
                );
                assert!(hub.execute_idle_evict_if_eligible(key).await);
                assert!(!hub.room_occupies_slot(key).await);
                assert_eq!(hub.available_room_slots(), 4);
                assert!(hub_join(run, &hub, &wiki, 3).await.is_ok());
                assert_eq!(hub.available_room_slots(), 3);
            })
        },
    )
    .await;
}

#[tokio::test]
async fn collab_lifecycle_joining_lease_blocks_eviction_while_paused_before_actor() {
    run_lifecycle_test(
        "collab_lifecycle_joining_lease_blocks_eviction_while_paused_before_actor",
        |run| {
            Box::pin(async {
                let wiki = setup_wiki_doc(&run.inner.harness).await;
                let hub = run.register_hub(Arc::new(CollabHub::new(
                    test_collab_config(4, 50),
                    wiki.session.pool.clone(),
                )));
                let key = room_key(wiki.session.workspace_id, wiki.document_id);

                hub.force_room_idle_eligible(key).await;
                let (reached_rx, proceed_tx) =
                    arm_hub_join_barrier(wiki.document_id, HUB_JOIN_BARRIER_BEFORE_ACTOR_JOIN)
                        .await;
                let join_task = tokio::spawn({
                    let hub = hub.clone();
                    let wiki = clone_wiki(&wiki);
                    async move { hub_join_with_lease(&hub, &wiki, 1).await.map(|(id, _)| id) }
                });
                tokio::time::timeout(Duration::from_secs(5), reached_rx)
                    .await
                    .expect("hub barrier before actor join must be reached")
                    .expect("barrier signal");
                hub.force_room_idle_eligible(key).await;
                assert_eq!(hub.room_joining_count(key).await, 1);
                assert_eq!(
                    hub.room_lifecycle_phase(key).await,
                    RoomLifecyclePhase::Live
                );
                assert_eq!(
                    hub.idle_evict_decision(key).await,
                    IdleEvictDecision::DeferredJoining
                );
                assert!(
                    !hub.execute_idle_evict_if_eligible(key).await,
                    "joining lease must block eviction before actor join"
                );

                join_task.abort();
                let _ = join_task.await;
                let _ = proceed_tx.send(());
                disarm_hub_join_barrier(wiki.document_id, HUB_JOIN_BARRIER_BEFORE_ACTOR_JOIN).await;
                wait_for_joining_count(&hub, key, 0).await;
            })
        },
    )
    .await;
}

#[tokio::test]
async fn collab_lifecycle_abort_queued_reply_reclaims_lease() {
    run_lifecycle_test(
        "collab_lifecycle_abort_queued_reply_reclaims_lease",
        |run| {
            Box::pin(async {
                let wiki = setup_wiki_doc(&run.inner.harness).await;
                let hub = run.register_hub(Arc::new(CollabHub::new(
                    test_collab_config(4, 200),
                    wiki.session.pool.clone(),
                )));
                let key = room_key(wiki.session.workspace_id, wiki.document_id);
                hub_join(run, &hub, &wiki, 1).await.expect("seed member");
                let conn_id = Uuid::now_v7();
                let (reached, proceed) = arm_join_reply_barrier(conn_id).await;
                let task = tokio::spawn({
                    let hub = hub.clone();
                    let wiki = clone_wiki(&wiki);
                    async move { hub_join_with_id(&hub, &wiki, 52, conn_id).await }
                });
                tokio::time::timeout(Duration::from_secs(5), reached)
                    .await
                    .expect("caller paused before polling reply")
                    .expect("pause signal");
                // Probe is FIFO after Join: the actor has sent the lease into the
                // oneshot, while its receiver is deliberately still unpolled.
                assert_eq!(hub.probe_actor(key).await.connections, 2);
                assert_eq!(
                    hub.room_lifecycle_phase(key).await,
                    RoomLifecyclePhase::Live
                );
                assert_eq!(hub.room_joining_count(key).await, 1);
                task.abort();
                assert!(task.await.unwrap_err().is_cancelled());
                drop(proceed);
                wait_for_probe_connections(&hub, key, 1).await;
                assert_eq!(
                    hub.room_lifecycle_phase(key).await,
                    RoomLifecyclePhase::Live
                );
                assert_eq!(hub.room_joining_count(key).await, 0);
                hub_join(run, &hub, &wiki, 52)
                    .await
                    .expect("join after queued lease drop");
                assert_eq!(hub.probe_actor(key).await.connections, 2);
            })
        },
    )
    .await;
}

#[tokio::test]
async fn collab_lifecycle_actor_panic_teardown_closes_peer_and_releases_resources() {
    run_lifecycle_test(
        "collab_lifecycle_actor_panic_teardown_closes_peer_and_releases_resources",
        |run| {
            Box::pin(async {
                let wiki = setup_wiki_doc(&run.inner.harness).await;
                let hub = run.register_hub(Arc::new(CollabHub::new(
                    test_collab_config(4, 200),
                    wiki.session.pool.clone(),
                )));
                let key = room_key(wiki.session.workspace_id, wiki.document_id);
                let admin = admin_pool(&run.inner.harness.admin_url).await;
                let engine_stop_witness = arm_engine_stop_witness(wiki.document_id).await;

                let (conn_id, lease, mut events_rx) =
                    hub_join_with_events(&hub, &wiki, 1).await.expect("join");
                run.retain_lease(lease);
                wait_until_guard(&admin, wiki.document_id, true).await;
                assert!(
                    hub.probe_actor(key).await.connections == 1,
                    "room must own a live bridge before panic"
                );

                arm_actor_panic_on_next_frame(wiki.document_id).await;
                let frame = sync_update_frame(
                    &routing_key(wiki.session.workspace_id, wiki.document_id),
                    &sample_hi_update(),
                );
                hub.send_frame(key, conn_id, frame).await;
                wait_for_close(&mut events_rx, 1011).await;
                assert!(
                    tokio::time::timeout(Duration::from_secs(5), engine_stop_witness)
                        .await
                        .expect("this room's engine bridge must stop before teardown finishes")
                        .expect("engine stop witness"),
                    "this bridge must successfully stop and join"
                );
                wait_for_member_count(&hub, key, 0).await;
                wait_until_guard(&admin, wiki.document_id, false).await;
                disarm_engine_stop_witness(wiki.document_id).await;
                disarm_actor_panic_on_next_frame(wiki.document_id).await;
            })
        },
    )
    .await;
}

#[tokio::test]
async fn collab_lifecycle_actor_panic_queued_join_gets_engine_unavailable() {
    run_lifecycle_test(
        "collab_lifecycle_actor_panic_queued_join_gets_engine_unavailable",
        |run| {
            Box::pin(async {
                let wiki = setup_wiki_doc(&run.inner.harness).await;
                let hub = run.register_hub(Arc::new(CollabHub::new(
                    test_collab_config(4, 200),
                    wiki.session.pool.clone(),
                )));
                let key = room_key(wiki.session.workspace_id, wiki.document_id);

                let (_member_a, lease_a, mut events_a) =
                    hub_join_with_events(&hub, &wiki, 1).await.expect("seed join");
                run.retain_lease(lease_a);

                let (reached_rx, proceed_tx) = arm_join_barrier(wiki.document_id).await;
                let join_b = tokio::spawn({
                    let hub = hub.clone();
                    let wiki = clone_wiki(&wiki);
                    async move { hub_join_with_lease(&hub, &wiki, 2).await }
                });
                tokio::time::timeout(Duration::from_secs(5), reached_rx)
                    .await
                    .expect("join barrier must be reached")
                    .expect("barrier signal");

                let conn_c = Uuid::now_v7();
                let admission_witness = arm_join_channel_admission_witness(conn_c).await;
                let join_c = tokio::spawn({
                    let hub = hub.clone();
                    let wiki = clone_wiki(&wiki);
                    async move { hub_join_with_id(&hub, &wiki, 3, conn_c).await }
                });
                tokio::time::timeout(Duration::from_secs(5), admission_witness)
                    .await
                    .expect("queued join must be admitted to the actor mailbox")
                    .expect("join channel admission witness");

                arm_actor_panic_after_join_barrier(wiki.document_id).await;
                proceed_tx.send(()).expect("release join barrier into panic");

                wait_for_close(&mut events_a, 1011).await;

                let queued = join_c.await.expect("queued join task");
                assert!(
                    matches!(queued, Err(JoinError::EngineUnavailable)),
                    "queued join during panic teardown must get explicit EngineUnavailable, got {queued:?}"
                );

                let blocked = join_b.await.expect("blocked join task");
                assert!(
                    matches!(blocked, Err(JoinError::EngineUnavailable)),
                    "in-flight join must not succeed after actor panic, got {blocked:?}"
                );

                wait_for_member_count(&hub, key, 0).await;
                disarm_join_barrier(wiki.document_id).await;
                disarm_join_channel_admission_witness(conn_c).await;
                disarm_actor_panic_after_join_barrier(wiki.document_id).await;
            })
        },
    )
    .await;
}

#[tokio::test]
async fn collab_lifecycle_actor_panic_guard_held_until_teardown_barrier() {
    run_lifecycle_test(
        "collab_lifecycle_actor_panic_guard_held_until_teardown_barrier",
        |run| {
            Box::pin(async {
                let wiki = setup_wiki_doc(&run.inner.harness).await;
                let hub = run.register_hub(Arc::new(CollabHub::new(
                    test_collab_config(4, 200),
                    wiki.session.pool.clone(),
                )));
                let key = room_key(wiki.session.workspace_id, wiki.document_id);
                let admin = admin_pool(&run.inner.harness.admin_url).await;
                let engine_stop_witness = arm_engine_stop_witness(wiki.document_id).await;

                let (conn_id, lease, mut events_rx) =
                    hub_join_with_events(&hub, &wiki, 1).await.expect("join");
                run.retain_lease(lease);
                wait_until_guard(&admin, wiki.document_id, true).await;

                let (reached_rx, proceed_tx) = arm_teardown_barrier(wiki.document_id).await;
                arm_actor_panic_on_next_frame(wiki.document_id).await;
                let frame = sync_update_frame(
                    &routing_key(wiki.session.workspace_id, wiki.document_id),
                    &sample_hi_update(),
                );
                hub.send_frame(key, conn_id, frame).await;
                wait_for_close(&mut events_rx, 1011).await;

                assert!(
                    tokio::time::timeout(Duration::from_secs(5), engine_stop_witness)
                        .await
                        .expect("this room's engine bridge must stop before guard release")
                        .expect("engine stop witness"),
                    "this bridge must successfully stop and join"
                );
                tokio::time::timeout(Duration::from_secs(5), reached_rx)
                    .await
                    .expect("teardown barrier must be reached after engine stop")
                    .expect("barrier signal");
                assert!(
                    room_guard_held(&admin, wiki.document_id).await,
                    "guard must stay held until teardown proceeds past engine stop"
                );

                proceed_tx.send(()).expect("release teardown barrier");
                wait_until_guard(&admin, wiki.document_id, false).await;
                wait_for_member_count(&hub, key, 0).await;
                disarm_teardown_barrier(wiki.document_id).await;
                disarm_engine_stop_witness(wiki.document_id).await;
                disarm_actor_panic_on_next_frame(wiki.document_id).await;
            })
        },
    )
    .await;
}

#[tokio::test]
async fn collab_lifecycle_actor_panic_preserves_committed_state_without_uncommitted_append() {
    run_lifecycle_test(
        "collab_lifecycle_actor_panic_preserves_committed_state_without_uncommitted_append",
        |run| {
            Box::pin(async {
                let wiki = setup_wiki_doc(&run.inner.harness).await;
                let hub = run.register_hub(Arc::new(CollabHub::new(
                    test_collab_config(4, 200),
                    wiki.session.pool.clone(),
                )));
                let key = room_key(wiki.session.workspace_id, wiki.document_id);
                let admin = admin_pool(&run.inner.harness.admin_url).await;

                let (conn_id, lease, mut events_rx) =
                    hub_join_with_events(&hub, &wiki, 1).await.expect("join");
                run.retain_lease(lease);
                let committed_frame = sync_update_frame(
                    &routing_key(wiki.session.workspace_id, wiki.document_id),
                    &sample_hi_update(),
                );
                hub.send_frame(key, conn_id, committed_frame).await;
                wait_for_sync_status_applied(&mut events_rx).await;
                let committed_tail = tail_seq(&admin, wiki.document_id).await;
                let committed_rows = tail_row_count(&admin, wiki.document_id).await;
                assert!(
                    committed_tail >= 1,
                    "fixture update must commit to durable tail"
                );

                let distinct_candidate = sync_update_frame(
                    &routing_key(wiki.session.workspace_id, wiki.document_id),
                    &support::delete_only_update(),
                );
                arm_actor_panic_on_next_frame(wiki.document_id).await;
                hub.send_frame(key, conn_id, distinct_candidate).await;
                wait_for_close(&mut events_rx, 1011).await;
                assert_no_sync_status_applied(&mut events_rx).await;
                assert_eq!(
                    tail_seq(&admin, wiki.document_id).await,
                    committed_tail,
                    "panic must not advance durable tail_seq"
                );
                assert_eq!(
                    tail_row_count(&admin, wiki.document_id).await,
                    committed_rows,
                    "panic must not append a durable tail row"
                );
                wait_for_member_count(&hub, key, 0).await;
                wait_until_guard(&admin, wiki.document_id, false).await;
                disarm_actor_panic_on_next_frame(wiki.document_id).await;
            })
        },
    )
    .await;
}

#[tokio::test]
async fn collab_lifecycle_panic_rejoin_restores_committed_bytes_and_allows_edit() {
    run_lifecycle_test(
        "collab_lifecycle_panic_rejoin_restores_committed_bytes_and_allows_edit",
        |run| {
            Box::pin(async {
                let wiki = setup_wiki_doc(&run.inner.harness).await;
                let hub = run.register_hub(Arc::new(CollabHub::new(
                    test_collab_config(4, 200),
                    wiki.session.pool.clone(),
                )));
                let key = room_key(wiki.session.workspace_id, wiki.document_id);
                let admin = admin_pool(&run.inner.harness.admin_url).await;
                let _idle_hold = IdleEvictionHold::arm(wiki.document_id);

                let (conn_id, lease, mut events_rx) =
                    hub_join_with_events(&hub, &wiki, 1).await.expect("join");
                run.retain_lease(lease);
                assert_eq!(room_start_count(wiki.document_id).await, 1);
                let committed_frame = sync_update_frame(
                    &routing_key(wiki.session.workspace_id, wiki.document_id),
                    &sample_hi_update(),
                );
                hub.send_frame(key, conn_id, committed_frame).await;
                wait_for_sync_status_applied(&mut events_rx).await;
                let committed_tail = tail_seq(&admin, wiki.document_id).await;
                let committed_rows = tail_row_count(&admin, wiki.document_id).await;
                assert!(committed_tail >= 1, "fixture update must commit");

                arm_actor_panic_on_next_frame(wiki.document_id).await;
                hub.send_frame(
                    key,
                    conn_id,
                    sync_update_frame(
                        &routing_key(wiki.session.workspace_id, wiki.document_id),
                        &support::delete_only_update(),
                    ),
                )
                .await;
                wait_for_close(&mut events_rx, 1011).await;
                wait_until_guard(&admin, wiki.document_id, false).await;
                disarm_actor_panic_on_next_frame(wiki.document_id).await;

                let (rejoin_id, rejoin_lease, mut rejoin_events) =
                    hub_join_with_events(&hub, &wiki, 2).await.expect("rejoin");
                run.retain_lease(rejoin_lease);
                assert_eq!(
                    room_start_count(wiki.document_id).await,
                    2,
                    "dead slot must start exactly one successor actor"
                );
                assert_eq!(
                    tail_seq(&admin, wiki.document_id).await,
                    committed_tail,
                    "rejoin must restore the previously committed tail"
                );
                assert_eq!(
                    tail_row_count(&admin, wiki.document_id).await,
                    committed_rows
                );
                assert_eq!(
                    hub.room_lifecycle_phase(key).await,
                    RoomLifecyclePhase::Live
                );

                hub.send_frame(
                    key,
                    rejoin_id,
                    sync_update_frame(
                        &routing_key(wiki.session.workspace_id, wiki.document_id),
                        &support::delete_only_update(),
                    ),
                )
                .await;
                wait_for_sync_status_applied(&mut rejoin_events).await;
                assert!(
                    tail_seq(&admin, wiki.document_id).await > committed_tail,
                    "successor actor must persist a further edit"
                );
            })
        },
    )
    .await;
}

#[tokio::test]
async fn collab_lifecycle_old_guard_held_until_teardown_then_next_owner() {
    run_lifecycle_test(
        "collab_lifecycle_old_guard_held_until_teardown_then_next_owner",
        |run| {
            Box::pin(async {
                let wiki = setup_wiki_doc(&run.inner.harness).await;
                let hub = run.register_hub(Arc::new(CollabHub::new(
                    test_collab_config(4, 200),
                    wiki.session.pool.clone(),
                )));
                let key = room_key(wiki.session.workspace_id, wiki.document_id);
                let admin = admin_pool(&run.inner.harness.admin_url).await;
                let _idle_hold = IdleEvictionHold::arm(wiki.document_id);

                let (conn_id, lease, mut events_rx) =
                    hub_join_with_events(&hub, &wiki, 1).await.expect("join");
                run.retain_lease(lease);
                wait_until_guard(&admin, wiki.document_id, true).await;
                let slots_before = hub.available_room_slots();

                let (reached_rx, proceed_tx) = arm_teardown_barrier(wiki.document_id).await;
                arm_actor_panic_on_next_frame(wiki.document_id).await;
                hub.send_frame(
                    key,
                    conn_id,
                    sync_update_frame(
                        &routing_key(wiki.session.workspace_id, wiki.document_id),
                        &sample_hi_update(),
                    ),
                )
                .await;
                wait_for_close(&mut events_rx, 1011).await;
                tokio::time::timeout(Duration::from_secs(5), reached_rx)
                    .await
                    .expect("teardown barrier")
                    .expect("barrier signal");
                assert!(room_guard_held(&admin, wiki.document_id).await);
                assert_eq!(room_start_count(wiki.document_id).await, 1);

                let rejoin = tokio::spawn({
                    let hub = hub.clone();
                    let wiki = clone_wiki(&wiki);
                    async move { hub_join_with_lease(&hub, &wiki, 2).await }
                });
                wait_for_phase(&hub, key, RoomLifecyclePhase::Closing).await;
                assert_eq!(
                    room_start_count(wiki.document_id).await,
                    1,
                    "successor must wait for the old guard"
                );
                assert!(
                    room_guard_held(&admin, wiki.document_id).await,
                    "old owner keeps the advisory lock until teardown proceeds"
                );
                assert_eq!(hub.available_room_slots(), slots_before);

                proceed_tx.send(()).expect("release teardown");
                let (_, lease) = tokio::time::timeout(Duration::from_secs(5), rejoin)
                    .await
                    .expect("rejoin after old owner")
                    .expect("rejoin task")
                    .expect("next owner join");
                run.retain_lease(lease);
                assert_eq!(room_start_count(wiki.document_id).await, 2);
                wait_until_guard(&admin, wiki.document_id, true).await;
                disarm_actor_panic_on_next_frame(wiki.document_id).await;
                disarm_teardown_barrier(wiki.document_id).await;
            })
        },
    )
    .await;
}

#[tokio::test]
async fn collab_lifecycle_concurrent_first_rejoin_starts_one_actor() {
    run_lifecycle_test(
        "collab_lifecycle_concurrent_first_rejoin_starts_one_actor",
        |run| {
            Box::pin(async {
                let wiki = setup_wiki_doc(&run.inner.harness).await;
                let hub = run.register_hub(Arc::new(CollabHub::new(
                    test_collab_config(4, 200),
                    wiki.session.pool.clone(),
                )));
                let key = room_key(wiki.session.workspace_id, wiki.document_id);
                let _idle_hold = IdleEvictionHold::arm(wiki.document_id);

                let (conn_id, lease, mut events_rx) =
                    hub_join_with_events(&hub, &wiki, 1).await.expect("join");
                run.retain_lease(lease);
                arm_actor_panic_on_next_frame(wiki.document_id).await;
                hub.send_frame(
                    key,
                    conn_id,
                    sync_update_frame(
                        &routing_key(wiki.session.workspace_id, wiki.document_id),
                        &sample_hi_update(),
                    ),
                )
                .await;
                wait_for_close(&mut events_rx, 1011).await;
                wait_for_member_count(&hub, key, 0).await;
                disarm_actor_panic_on_next_frame(wiki.document_id).await;

                let left = tokio::spawn({
                    let hub = hub.clone();
                    let wiki = clone_wiki(&wiki);
                    async move { hub_join_with_lease(&hub, &wiki, 3).await }
                });
                let right = tokio::spawn({
                    let hub = hub.clone();
                    let wiki = clone_wiki(&wiki);
                    async move { hub_join_with_lease(&hub, &wiki, 4).await }
                });
                let (left, right) = tokio::join!(left, right);
                let (_, left_lease) = left.expect("left task").expect("left rejoin");
                let (_, right_lease) = right.expect("right task").expect("right rejoin");
                run.retain_lease(left_lease);
                run.retain_lease(right_lease);
                assert_eq!(
                    room_start_count(wiki.document_id).await,
                    2,
                    "concurrent first rejoin must start one successor actor"
                );
                wait_for_probe_connections(&hub, key, 2).await;
            })
        },
    )
    .await;
}

#[tokio::test]
async fn collab_lifecycle_cancel_joiner_cleanup_continues() {
    run_lifecycle_test("collab_lifecycle_cancel_joiner_cleanup_continues", |run| {
        Box::pin(async {
            let wiki = setup_wiki_doc(&run.inner.harness).await;
            let hub = run.register_hub(Arc::new(CollabHub::new(
                test_collab_config(4, 200),
                wiki.session.pool.clone(),
            )));
            let key = room_key(wiki.session.workspace_id, wiki.document_id);
            let admin = admin_pool(&run.inner.harness.admin_url).await;
            let _idle_hold = IdleEvictionHold::arm(wiki.document_id);

            let (conn_id, lease, mut events_rx) =
                hub_join_with_events(&hub, &wiki, 1).await.expect("join");
            run.retain_lease(lease);
            let slots_held = hub.available_room_slots();
            let (teardown_reached, teardown_proceed) =
                arm_teardown_barrier(wiki.document_id).await;
            arm_actor_panic_on_next_frame(wiki.document_id).await;
            hub.send_frame(
                key,
                conn_id,
                sync_update_frame(
                    &routing_key(wiki.session.workspace_id, wiki.document_id),
                    &sample_hi_update(),
                ),
            )
            .await;
            wait_for_close(&mut events_rx, 1011).await;
            tokio::time::timeout(Duration::from_secs(5), teardown_reached)
                .await
                .expect("teardown reached")
                .expect("teardown signal");

            let (reclaim_reached, reclaim_proceed) =
                arm_reclaim_barrier(wiki.document_id).await;
            let join_task = tokio::spawn({
                let hub = hub.clone();
                let wiki = clone_wiki(&wiki);
                async move { hub_join_with_lease(&hub, &wiki, 5).await }
            });
            tokio::time::timeout(Duration::from_secs(5), reclaim_reached)
                .await
                .expect("reclaim must start on the hub task")
                .expect("reclaim signal");
            wait_for_phase(&hub, key, RoomLifecyclePhase::Closing).await;

            join_task.abort();
            assert!(join_task.await.unwrap_err().is_cancelled());
            assert_eq!(
                hub.room_lifecycle_phase(key).await,
                RoomLifecyclePhase::Closing,
                "cancelling the joiner must not strand or steal Closing"
            );
            assert_eq!(hub.available_room_slots(), slots_held);

            reclaim_proceed.send(()).expect("release reclaim");
            assert!(
                room_guard_held(&admin, wiki.document_id).await,
                "cleanup must still wait for finished/helper/guard"
            );

            teardown_proceed.send(()).expect("release teardown");
            wait_until_guard(&admin, wiki.document_id, false).await;
            wait_for_phase(&hub, key, RoomLifecyclePhase::Absent).await;
            assert_eq!(
                hub.available_room_slots(),
                slots_held + 1,
                "reclaim must drop the permit after cancelled join"
            );

            let (_, lease) = hub_join_with_lease(&hub, &wiki, 6)
                .await
                .expect("join after cancelled reclaim");
            run.retain_lease(lease);
            disarm_actor_panic_on_next_frame(wiki.document_id).await;
            disarm_teardown_barrier(wiki.document_id).await;
            disarm_reclaim_barrier(wiki.document_id).await;
        })
    })
    .await;
}

#[tokio::test]
async fn collab_lifecycle_closing_between_slot_return_and_phase_lock_retries() {
    run_lifecycle_test(
        "collab_lifecycle_closing_between_slot_return_and_phase_lock_retries",
        |run| {
            Box::pin(async {
                let wiki = setup_wiki_doc(&run.inner.harness).await;
                let hub = run.register_hub(Arc::new(CollabHub::new(
                    test_collab_config(4, 200),
                    wiki.session.pool.clone(),
                )));
                let key = room_key(wiki.session.workspace_id, wiki.document_id);
                let _idle_hold = IdleEvictionHold::arm(wiki.document_id);

                let (conn_id, lease, mut events_rx) =
                    hub_join_with_events(&hub, &wiki, 1).await.expect("join");
                run.retain_lease(lease);
                let (teardown_reached, teardown_proceed) =
                    arm_teardown_barrier(wiki.document_id).await;
                arm_actor_panic_on_next_frame(wiki.document_id).await;
                hub.send_frame(
                    key,
                    conn_id,
                    sync_update_frame(
                        &routing_key(wiki.session.workspace_id, wiki.document_id),
                        &sample_hi_update(),
                    ),
                )
                .await;
                wait_for_close(&mut events_rx, 1011).await;
                tokio::time::timeout(Duration::from_secs(5), teardown_reached)
                    .await
                    .expect("teardown reached")
                    .expect("teardown signal");

                let (slot_ready_rx, slot_ready_tx) =
                    arm_hub_join_barrier(wiki.document_id, HUB_JOIN_BARRIER_AFTER_SLOT_READY).await;
                let rejoin = tokio::spawn({
                    let hub = hub.clone();
                    let wiki = clone_wiki(&wiki);
                    async move { hub_join_with_lease(&hub, &wiki, 7).await }
                });
                tokio::time::timeout(Duration::from_secs(5), slot_ready_rx)
                    .await
                    .expect("slot-ready barrier")
                    .expect("barrier signal");
                assert_eq!(
                    hub.room_lifecycle_phase(key).await,
                    RoomLifecyclePhase::Live
                );

                let evict = tokio::spawn({
                    let hub = hub.clone();
                    async move { hub.execute_idle_evict_if_eligible(key).await }
                });
                wait_for_phase(&hub, key, RoomLifecyclePhase::Closing).await;
                slot_ready_tx.send(()).expect("release slot-ready");
                teardown_proceed.send(()).expect("release teardown");

                let evicted = evict.await.expect("evict task");
                assert!(evicted, "idle owner must win Live to Closing in the window");
                let (_, lease) = tokio::time::timeout(Duration::from_secs(5), rejoin)
                    .await
                    .expect("retry after Closing race")
                    .expect("rejoin task")
                    .expect("bounded retry must succeed on the next owner");
                run.retain_lease(lease);
                assert_eq!(room_start_count(wiki.document_id).await, 2);
                disarm_hub_join_barrier(wiki.document_id, HUB_JOIN_BARRIER_AFTER_SLOT_READY).await;
                disarm_actor_panic_on_next_frame(wiki.document_id).await;
                disarm_teardown_barrier(wiki.document_id).await;
            })
        },
    )
    .await;
}

#[tokio::test]
async fn collab_lifecycle_queue_full_is_not_stale_reclamation() {
    run_lifecycle_test(
        "collab_lifecycle_queue_full_is_not_stale_reclamation",
        |run| {
            Box::pin(async {
                let wiki = setup_wiki_doc(&run.inner.harness).await;
                let mut cfg = test_collab_config(4, 200);
                cfg.max_queued_room_ops = 1;
                let hub =
                    run.register_hub(Arc::new(CollabHub::new(cfg, wiki.session.pool.clone())));
                let key = room_key(wiki.session.workspace_id, wiki.document_id);

                hub_join(run, &hub, &wiki, 1).await.expect("seed");
                let (reached_rx, proceed_tx) = arm_join_barrier(wiki.document_id).await;
                let blocked = tokio::spawn({
                    let hub = hub.clone();
                    let wiki = clone_wiki(&wiki);
                    async move { hub_join_with_lease(&hub, &wiki, 8).await }
                });
                tokio::time::timeout(Duration::from_secs(5), reached_rx)
                    .await
                    .expect("join barrier")
                    .expect("barrier signal");

                let conn_c = Uuid::now_v7();
                let admitted_c = arm_join_channel_admission_witness(conn_c).await;
                let queued = tokio::spawn({
                    let hub = hub.clone();
                    let wiki = clone_wiki(&wiki);
                    async move { hub_join_with_id(&hub, &wiki, 9, conn_c).await }
                });
                tokio::time::timeout(Duration::from_secs(5), admitted_c)
                    .await
                    .expect("queued join must occupy the mailbox")
                    .expect("admission witness");

                let overflow = hub_join_with_lease(&hub, &wiki, 10).await;
                assert!(
                    matches!(overflow, Err(JoinError::RoomFull)),
                    "QueueFull must surface as backpressure, got {overflow:?}"
                );
                assert_eq!(
                    hub.room_lifecycle_phase(key).await,
                    RoomLifecyclePhase::Live,
                    "a full mailbox is not a dead slot"
                );
                assert_eq!(room_start_count(wiki.document_id).await, 1);

                proceed_tx.send(()).expect("release join barrier");
                let (_, blocked_lease) =
                    blocked.await.expect("blocked task").expect("blocked join");
                let (_, queued_lease) = queued.await.expect("queued task").expect("queued join");
                run.retain_lease(blocked_lease);
                run.retain_lease(queued_lease);
                disarm_join_channel_admission_witness(conn_c).await;
                disarm_join_barrier(wiki.document_id).await;
            })
        },
    )
    .await;
}

#[tokio::test]
async fn collab_lifecycle_noreply_is_not_retried_for_same_conn() {
    run_lifecycle_test(
        "collab_lifecycle_noreply_is_not_retried_for_same_conn",
        |run| {
            Box::pin(async {
                let wiki = setup_wiki_doc(&run.inner.harness).await;
                let hub = run.register_hub(Arc::new(CollabHub::new(
                    test_collab_config(4, 200),
                    wiki.session.pool.clone(),
                )));
                let key = room_key(wiki.session.workspace_id, wiki.document_id);
                let _idle_hold = IdleEvictionHold::arm(wiki.document_id);

                let (_member, lease, mut events_rx) =
                    hub_join_with_events(&hub, &wiki, 1).await.expect("seed");
                run.retain_lease(lease);

                let conn_b = Uuid::now_v7();
                let (reached_rx, proceed_tx) = arm_join_barrier(wiki.document_id).await;
                let join_b = tokio::spawn({
                    let hub = hub.clone();
                    let wiki = clone_wiki(&wiki);
                    async move { hub_join_with_id(&hub, &wiki, 11, conn_b).await }
                });
                tokio::time::timeout(Duration::from_secs(5), reached_rx)
                    .await
                    .expect("join barrier")
                    .expect("barrier signal");

                arm_actor_panic_after_join_barrier(wiki.document_id).await;
                proceed_tx.send(()).expect("panic after enqueue");
                wait_for_close(&mut events_rx, 1011).await;

                let blocked = join_b.await.expect("in-flight join task");
                assert!(
                    matches!(blocked, Err(JoinError::EngineUnavailable)),
                    "NoReply must fail without inventing success, got {blocked:?}"
                );
                assert_eq!(
                    join_delivery_attempt_count(conn_b).await,
                    1,
                    "NoReply must never retry the same conn_id"
                );
                assert_eq!(
                    room_start_count(wiki.document_id).await,
                    1,
                    "NoReply itself must not spawn a successor"
                );

                let (_, lease) = hub_join_with_lease(&hub, &wiki, 12)
                    .await
                    .expect("fresh conn after NoReply");
                run.retain_lease(lease);
                assert_eq!(join_delivery_attempt_count(conn_b).await, 1);
                assert_eq!(room_start_count(wiki.document_id).await, 2);
                wait_for_probe_connections(&hub, key, 1).await;
                disarm_join_barrier(wiki.document_id).await;
                disarm_actor_panic_after_join_barrier(wiki.document_id).await;
            })
        },
    )
    .await;
}

#[tokio::test]
async fn collab_lifecycle_shutdown_waits_for_reclaim() {
    run_lifecycle_test("collab_lifecycle_shutdown_waits_for_reclaim", |run| {
        Box::pin(async {
            let wiki = setup_wiki_doc(&run.inner.harness).await;
            let hub = run.register_hub(Arc::new(CollabHub::new(
                test_collab_config(4, 200),
                wiki.session.pool.clone(),
            )));
            let key = room_key(wiki.session.workspace_id, wiki.document_id);
            let admin = admin_pool(&run.inner.harness.admin_url).await;
            let _idle_hold = IdleEvictionHold::arm(wiki.document_id);

            let (conn_id, lease, mut events_rx) =
                hub_join_with_events(&hub, &wiki, 1).await.expect("join");
            run.retain_lease(lease);
            let slots_held = hub.available_room_slots();
            let (teardown_reached, teardown_proceed) = arm_teardown_barrier(wiki.document_id).await;
            arm_actor_panic_on_next_frame(wiki.document_id).await;
            hub.send_frame(
                key,
                conn_id,
                sync_update_frame(
                    &routing_key(wiki.session.workspace_id, wiki.document_id),
                    &sample_hi_update(),
                ),
            )
            .await;
            wait_for_close(&mut events_rx, 1011).await;
            tokio::time::timeout(Duration::from_secs(5), teardown_reached)
                .await
                .expect("teardown reached")
                .expect("teardown signal");

            let (reclaim_reached, reclaim_proceed) = arm_reclaim_barrier(wiki.document_id).await;
            let rejoin = tokio::spawn({
                let hub = hub.clone();
                let wiki = clone_wiki(&wiki);
                async move { hub_join_with_lease(&hub, &wiki, 13).await }
            });
            tokio::time::timeout(Duration::from_secs(5), reclaim_reached)
                .await
                .expect("reclaim started")
                .expect("reclaim signal");
            wait_for_phase(&hub, key, RoomLifecyclePhase::Closing).await;

            let drain_witness = hub.arm_shutdown_drain_witness().await;
            let shutdown_task = tokio::spawn({
                let hub = hub.clone();
                async move { hub.shutdown().await }
            });
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    if hub.is_shutting_down() {
                        return;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("shutdown must begin while reclaim owns the slot");
            tokio::time::timeout(Duration::from_secs(5), drain_witness)
                .await
                .expect("shutdown must reach the reclaim drain")
                .expect("drain witness");
            assert!(
                !shutdown_task.is_finished(),
                "shutdown must wait for the hub-owned reclaim task"
            );
            assert_eq!(
                hub.room_lifecycle_phase(key).await,
                RoomLifecyclePhase::Closing
            );
            assert!(room_guard_held(&admin, wiki.document_id).await);
            assert_eq!(hub.available_room_slots(), slots_held);

            reclaim_proceed.send(()).expect("release reclaim pause");
            assert!(
                !shutdown_task.is_finished(),
                "shutdown must still wait for finished/helper/guard"
            );
            assert!(room_guard_held(&admin, wiki.document_id).await);

            teardown_proceed.send(()).expect("release teardown");
            let status = shutdown_task.await.expect("shutdown task");
            assert!(
                !status.is_clean(),
                "panic reclaim during shutdown must report abnormal actor completion: {status:?}"
            );
            assert_eq!(
                status.actor_failures,
                1,
                "panic reclaim must be counted exactly once: {status:?}"
            );
            assert_eq!(
                hub.available_room_slots(),
                slots_held + 1,
                "reclaim must release the permit before shutdown returns ({status:?})"
            );
            wait_until_guard(&admin, wiki.document_id, false).await;
            let rejoin_result = rejoin.await.expect("rejoin task");
            assert!(
                matches!(rejoin_result, Err(JoinError::EngineUnavailable)),
                "in-flight rejoin must fail once shutdown owns admission, got {rejoin_result:?}"
            );
            disarm_actor_panic_on_next_frame(wiki.document_id).await;
            disarm_teardown_barrier(wiki.document_id).await;
            disarm_reclaim_barrier(wiki.document_id).await;
            hub.disarm_shutdown_drain_witness().await;
        })
    })
    .await;
}

#[tokio::test]
async fn collab_lifecycle_idle_timer_reclaims_dead_slot_after_hold_release() {
    run_lifecycle_test(
        "collab_lifecycle_idle_timer_reclaims_dead_slot_after_hold_release",
        |run| {
            Box::pin(async {
                let wiki = setup_wiki_doc(&run.inner.harness).await;
                let hub = run.register_hub(Arc::new(CollabHub::new(
                    test_collab_config(4, 200),
                    wiki.session.pool.clone(),
                )));
                let key = room_key(wiki.session.workspace_id, wiki.document_id);
                let admin = admin_pool(&run.inner.harness.admin_url).await;
                let idle_hold = IdleEvictionHold::arm(wiki.document_id);

                let (conn_id, lease, mut events_rx) =
                    hub_join_with_events(&hub, &wiki, 1).await.expect("join");
                run.retain_lease(lease);
                let slots_held = hub.available_room_slots();
                assert_eq!(room_start_count(wiki.document_id).await, 1);

                let (teardown_reached, teardown_proceed) =
                    arm_teardown_barrier(wiki.document_id).await;
                arm_actor_panic_on_next_frame(wiki.document_id).await;
                hub.send_frame(
                    key,
                    conn_id,
                    sync_update_frame(
                        &routing_key(wiki.session.workspace_id, wiki.document_id),
                        &sample_hi_update(),
                    ),
                )
                .await;
                wait_for_close(&mut events_rx, 1011).await;
                tokio::time::timeout(Duration::from_secs(5), teardown_reached)
                    .await
                    .expect("teardown reached")
                    .expect("teardown signal");
                teardown_proceed.send(()).expect("release teardown");
                wait_until_guard(&admin, wiki.document_id, false).await;
                assert_eq!(
                    hub.room_lifecycle_phase(key).await,
                    RoomLifecyclePhase::Live,
                    "dead actor stays Live until an owner reclaims it"
                );

                drop(idle_hold);
                tokio::time::timeout(Duration::from_secs(5), async {
                    loop {
                        if hub.room_lifecycle_phase(key).await == RoomLifecyclePhase::Absent {
                            return;
                        }
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .expect("idle timer must reclaim the closed Live");
                assert_eq!(
                    hub.available_room_slots(),
                    slots_held + 1,
                    "timer reclaim must return the room permit"
                );
                assert_eq!(
                    room_start_count(wiki.document_id).await,
                    1,
                    "timer reclaim must not start a successor actor"
                );

                let (_, lease) = hub_join_with_lease(&hub, &wiki, 14)
                    .await
                    .expect("rejoin after timer reclaim");
                run.retain_lease(lease);
                assert_eq!(
                    room_start_count(wiki.document_id).await,
                    2,
                    "successor actor must start after timer reclaim"
                );
                disarm_actor_panic_on_next_frame(wiki.document_id).await;
                disarm_teardown_barrier(wiki.document_id).await;
            })
        },
    )
    .await;
}
