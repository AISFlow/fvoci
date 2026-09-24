#![cfg(feature = "db-tests")]

#[allow(dead_code)]
mod support;

use std::panic::{resume_unwind, AssertUnwindSafe};
use std::sync::Arc;
use std::time::Duration;

use futures_util::future::BoxFuture;
use futures_util::FutureExt;
use fvoci_server::collab::hub::{
    arm_hub_join_barrier, disarm_hub_join_barrier, CollabHub, IdleEvictDecision,
    RoomLifecyclePhase, HUB_JOIN_BARRIER_AFTER_ACTOR_REPLY, HUB_JOIN_BARRIER_BEFORE_ACTOR_JOIN,
};
use fvoci_server::collab::room::{
    arm_join_barrier, arm_join_reply_barrier, disarm_join_barrier, AuthenticatedConnection,
    CollabSession, ConnectionLease, JoinError, RoomJoin,
};
use fvoci_server::collab::wire::{CollabKind, CollabRoomName};
use support::{setup_wiki_doc, test_collab_config, TestDb, TestRun};
use tokio::sync::mpsc;
use uuid::Uuid;

const TEST_TIMEOUT: Duration = Duration::from_secs(30);

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
