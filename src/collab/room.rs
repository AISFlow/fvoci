use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use collab_engine::b64;
use collab_engine::outcome::EngineStatus;
use collab_engine::protocol::Request;
use sha2::{Digest, Sha256};
use sqlx::postgres::PgPool;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use crate::collab::awareness::{decode_awareness, AwarenessRegistry};
use crate::collab::config::CollabConfig;
use crate::collab::engine_bridge::{BridgeError, EngineBridge};
use crate::collab::guard::RoomGuard;
use crate::collab::validation::{
    validate_recovery_bundle, validate_snapshot_only, BundleValidation,
};
use crate::collab::wire::{encode, AuthMessage, DocumentMessage, SyncStep, WireFrame};
use crate::collab::y_sync::{encode_sync_payload, is_empty_update, parse_sync_payload};
use crate::db::collab::verify_collab_operation;
use crate::db::collab::{
    append_collab_update, claim_writer_and_load, compact_collab_snapshot, load_collab_readonly,
    resolve_collab_admission, AppendCollabInput, AppendCollabResult, CollabDbError,
    CompactCollabInput, VerifyCollabInput,
};
use crate::db::identity::LiveSession;

#[cfg(feature = "db-tests")]
static SPAWN_ROOM_BLOCKS: std::sync::LazyLock<
    tokio::sync::Mutex<HashMap<Uuid, tokio::sync::oneshot::Receiver<()>>>,
> = std::sync::LazyLock::new(|| tokio::sync::Mutex::new(HashMap::new()));

#[cfg(feature = "db-tests")]
pub async fn arm_spawn_room_block(document_id: Uuid) -> tokio::sync::oneshot::Sender<()> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    assert!(SPAWN_ROOM_BLOCKS
        .lock()
        .await
        .insert(document_id, rx)
        .is_none());
    tx
}

#[cfg(feature = "db-tests")]
pub async fn disarm_spawn_room_block(document_id: Uuid) {
    SPAWN_ROOM_BLOCKS.lock().await.remove(&document_id);
}

async fn wait_spawn_room_block(_document_id: Uuid) {
    #[cfg(feature = "db-tests")]
    {
        let gate = SPAWN_ROOM_BLOCKS.lock().await.remove(&_document_id);
        if let Some(rx) = gate {
            let _ = rx.await;
        }
    }
}

pub type RoomKey = (Uuid, Uuid);

#[derive(Debug, Clone)]
pub struct CollabSession {
    pub session_id: Uuid,
    pub user_id: Uuid,
    pub given_name: String,
    pub family_name: Option<String>,
}

impl From<LiveSession> for CollabSession {
    fn from(live: LiveSession) -> Self {
        Self {
            session_id: live.session_id,
            user_id: live.user_id,
            given_name: live.given_name,
            family_name: live.family_name,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AuthenticatedConnection {
    pub conn_id: Uuid,
    pub session: CollabSession,
    pub client_id: u32,
    pub read_only: bool,
    pub routing_key: String,
}

#[derive(Debug)]
pub enum RoomClientEvent {
    Outbound(Vec<u8>),
    Close { code: u16, reason: String },
}

pub struct RoomJoin {
    pub conn: AuthenticatedConnection,
    pub events: mpsc::Sender<RoomClientEvent>,
}

enum RoomCommand {
    Join(RoomJoin, oneshot::Sender<Result<(), JoinError>>),
    Leave(Uuid),
    Frame { conn_id: Uuid, bytes: Vec<u8> },
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JoinError {
    AdmissionDenied,
    UnsupportedKind,
    RoomFull,
    EngineUnavailable,
    WriterStale,
    DbError,
}

pub struct RoomHandle {
    tx: mpsc::Sender<RoomCommand>,
}

impl RoomHandle {
    pub async fn join(&self, join: RoomJoin) -> Result<(), JoinError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(RoomCommand::Join(join, reply_tx))
            .await
            .map_err(|_| JoinError::EngineUnavailable)?;
        reply_rx.await.map_err(|_| JoinError::EngineUnavailable)?
    }

    pub async fn leave(&self, conn_id: Uuid) {
        let _ = self.tx.send(RoomCommand::Leave(conn_id)).await;
    }

    pub async fn frame(&self, conn_id: Uuid, bytes: Vec<u8>) {
        let _ = self.tx.send(RoomCommand::Frame { conn_id, bytes }).await;
    }

    pub async fn shutdown(&self) {
        let _ = self.tx.send(RoomCommand::Shutdown).await;
    }
}

struct CommittedBundle {
    snapshot: Vec<u8>,
    tail_payloads: Vec<Vec<u8>>,
    tail_seq: i64,
    snapshot_cutoff_seq: i64,
}

struct ConnectionState {
    session: CollabSession,
    client_id: u32,
    read_only: bool,
    routing_key: String,
    events: mpsc::Sender<RoomClientEvent>,
    conn_generation: u64,
    pending_bytes: usize,
    poisoned: bool,
    pending_persist: VecDeque<PersistBarrier>,
    in_flight: bool,
}

struct PersistBarrier {
    request_id: Uuid,
    prefix_fifo: u64,
}

struct RoomActor {
    workspace_id: Uuid,
    document_id: Uuid,
    config: CollabConfig,
    pool: PgPool,
    engine: EngineBridge,
    room_guard: Option<RoomGuard>,
    writer_generation: Option<i64>,
    committed: CommittedBundle,
    connections: HashMap<Uuid, ConnectionState>,
    awareness: AwarenessRegistry,
    fifo_seq: u64,
    persist_failed: bool,
    client_id_owner: HashMap<u32, (Uuid, Instant)>,
    shutting_down: bool,
    last_acl_poll: Instant,
}

pub async fn spawn_room(
    workspace_id: Uuid,
    document_id: Uuid,
    config: CollabConfig,
    pool: PgPool,
    room_guard: RoomGuard,
) -> Result<(RoomHandle, oneshot::Receiver<()>), JoinError> {
    wait_spawn_room_block(document_id).await;
    let engine = EngineBridge::spawn(config.engine_bin.clone(), config.limits)
        .map_err(|_| JoinError::EngineUnavailable)?;
    let (tx, rx) = mpsc::channel(config.max_queued_room_ops);
    let (finished_tx, finished_rx) = oneshot::channel();
    let actor = RoomActor {
        workspace_id,
        document_id,
        config,
        pool,
        engine,
        room_guard: Some(room_guard),
        writer_generation: None,
        committed: CommittedBundle {
            snapshot: vec![0, 0],
            tail_payloads: Vec::new(),
            tail_seq: 0,
            snapshot_cutoff_seq: 0,
        },
        connections: HashMap::new(),
        awareness: AwarenessRegistry::new(),
        fifo_seq: 0,
        persist_failed: false,
        client_id_owner: HashMap::new(),
        shutting_down: false,
        last_acl_poll: Instant::now(),
    };
    tokio::spawn(async move {
        actor.run(rx).await;
        let _ = finished_tx.send(());
    });
    Ok((RoomHandle { tx }, finished_rx))
}

impl RoomActor {
    async fn run(mut self, mut rx: mpsc::Receiver<RoomCommand>) {
        while let Some(cmd) = rx.recv().await {
            match cmd {
                RoomCommand::Join(join, reply) => {
                    let result = self.handle_join(join).await;
                    let _ = reply.send(result);
                }
                RoomCommand::Leave(conn_id) => self.handle_leave(conn_id),
                RoomCommand::Frame { conn_id, bytes } => {
                    self.handle_frame(conn_id, bytes).await;
                }
                RoomCommand::Shutdown => {
                    self.shutting_down = true;
                    break;
                }
            }
            self.poll_acl().await;
            if self.connections.is_empty() && self.shutting_down {
                break;
            }
        }
        for (_, conn) in self.connections.drain() {
            let _ = conn
                .events
                .send(RoomClientEvent::Close {
                    code: 1001,
                    reason: "server shutdown".into(),
                })
                .await;
        }
        let engine = self.engine;
        let _ = engine.stop().await;
        if let Some(guard) = self.room_guard.take() {
            guard.release().await;
        }
    }

    async fn poll_acl(&mut self) {
        let interval = Duration::from_millis(self.config.revoke_poll_ms);
        if self.last_acl_poll.elapsed() < interval {
            return;
        }
        self.last_acl_poll = Instant::now();
        let mut to_close = Vec::new();
        let snapshots = self
            .connections
            .iter()
            .map(|(id, c)| (*id, c.session.clone(), c.read_only))
            .collect::<Vec<_>>();
        for (conn_id, session, read_only) in snapshots {
            if !self.session_authorized(&session, read_only).await {
                to_close.push(conn_id);
            }
        }
        for conn_id in to_close {
            self.close_connection(conn_id, 1008, "permission revoked");
        }
    }

    async fn session_authorized(&self, session: &CollabSession, read_only: bool) -> bool {
        match resolve_collab_admission(
            &self.pool,
            self.workspace_id,
            session.user_id,
            session.session_id,
            self.document_id,
        )
        .await
        {
            Ok(Ok(admission)) => {
                if read_only {
                    true
                } else {
                    !admission.read_only
                }
            }
            _ => false,
        }
    }

    fn close_connection(&mut self, conn_id: Uuid, code: u16, reason: &str) {
        if let Some(conn) = self.connections.remove(&conn_id) {
            let _ = conn.events.try_send(RoomClientEvent::Close {
                code,
                reason: reason.into(),
            });
            if let Some(encoded) = self
                .awareness
                .remove_client(conn.client_id, conn.conn_generation)
            {
                self.broadcast_awareness(&encoded);
            }
            self.client_id_owner.remove(&conn.client_id);
        }
    }

    async fn handle_join(&mut self, join: RoomJoin) -> Result<(), JoinError> {
        if self.shutting_down {
            return Err(JoinError::EngineUnavailable);
        }
        if self.connections.len() >= self.config.max_connections_per_room {
            return Err(JoinError::RoomFull);
        }
        let admission = resolve_collab_admission(
            &self.pool,
            self.workspace_id,
            join.conn.session.user_id,
            join.conn.session.session_id,
            self.document_id,
        )
        .await
        .map_err(|_| JoinError::DbError)?;
        let admission = admission.map_err(|_| JoinError::AdmissionDenied)?;
        let read_only = join.conn.read_only || admission.read_only;
        if self.writer_generation.is_none() && !read_only {
            let claim = claim_writer_and_load(
                &self.pool,
                self.workspace_id,
                join.conn.session.user_id,
                join.conn.session.session_id,
                self.document_id,
            )
            .await
            .map_err(|_| JoinError::DbError)?;
            let claim = claim.map_err(|e| match e {
                CollabDbError::StaleWriter => JoinError::WriterStale,
                _ => JoinError::AdmissionDenied,
            })?;
            self.writer_generation = Some(claim.writer_generation);
            self.set_committed_from_load(&claim.load);
            self.load_engine_primary().await?;
        } else if self.writer_generation.is_none() && read_only {
            let load = load_collab_readonly(
                &self.pool,
                self.workspace_id,
                join.conn.session.user_id,
                join.conn.session.session_id,
                self.document_id,
            )
            .await
            .map_err(|_| JoinError::DbError)?;
            let load = load.map_err(|_| JoinError::AdmissionDenied)?;
            self.set_committed_from_load(&load);
            self.load_engine_primary().await?;
        }
        if !self.reserve_client_id(join.conn.client_id, join.conn.session.user_id) {
            return Err(JoinError::AdmissionDenied);
        }
        let conn_generation = self.awareness.connection_generation();
        self.connections.insert(
            join.conn.conn_id,
            ConnectionState {
                session: join.conn.session,
                client_id: join.conn.client_id,
                read_only,
                routing_key: join.conn.routing_key.clone(),
                events: join.events,
                conn_generation,
                pending_bytes: 0,
                poisoned: false,
                pending_persist: VecDeque::new(),
                in_flight: false,
            },
        );
        Ok(())
    }

    fn set_committed_from_load(&mut self, load: &crate::db::collab::CollabLoadState) {
        self.committed.snapshot = load.snapshot.clone();
        self.committed.tail_payloads = load.tail.iter().map(|r| r.payload.clone()).collect();
        self.committed.tail_seq = load.tail_seq;
        self.committed.snapshot_cutoff_seq = load.snapshot_cutoff_seq;
    }

    fn reserve_client_id(&mut self, client_id: u32, user_id: Uuid) -> bool {
        let now = Instant::now();
        let ttl = Duration::from_millis(self.config.client_id_ttl_ms);
        self.client_id_owner
            .retain(|_, (_, at)| now.duration_since(*at) < ttl);
        if let Some((owner, at)) = self.client_id_owner.get(&client_id) {
            if *owner != user_id && now.duration_since(*at) < ttl {
                return false;
            }
        }
        self.client_id_owner.insert(client_id, (user_id, now));
        true
    }

    fn handle_leave(&mut self, conn_id: Uuid) {
        self.close_connection(conn_id, 1000, "client leave");
    }

    async fn handle_frame(&mut self, conn_id: Uuid, bytes: Vec<u8>) {
        let Some(conn) = self.connections.get_mut(&conn_id) else {
            return;
        };
        if conn.pending_bytes + bytes.len() > self.config.max_pending_bytes_per_connection {
            self.close_connection(conn_id, 1009, "pending bytes exceeded");
            return;
        }
        conn.pending_bytes += bytes.len();
        let routing_key = conn.routing_key.clone();
        let read_only = conn.read_only;
        let client_id = conn.client_id;
        let session = conn.session.clone();
        let events = conn.events.clone();
        let conn_generation = conn.conn_generation;

        let frame = match crate::collab::wire::decode(&bytes) {
            Ok(frame) => frame,
            Err(_) => {
                self.close_connection(conn_id, 1003, "invalid frame");
                return;
            }
        };

        match frame {
            WireFrame::Connection(_) => {
                self.handle_connection_ping(&events).await;
            }
            WireFrame::Document {
                routing_key: key,
                room,
                message,
            } => {
                if key != routing_key {
                    return;
                }
                if room.is_none() {
                    self.close_connection(conn_id, 1008, "invalid room");
                    return;
                }
                if !self.session_authorized(&session, read_only).await {
                    self.close_connection(conn_id, 1008, "permission revoked");
                    return;
                }
                self.handle_document_message(
                    conn_id,
                    &events,
                    &routing_key,
                    read_only,
                    client_id,
                    &session,
                    conn_generation,
                    message,
                )
                .await;
            }
        }
        if let Some(conn) = self.connections.get_mut(&conn_id) {
            conn.pending_bytes = conn.pending_bytes.saturating_sub(bytes.len());
        }
    }

    async fn handle_connection_ping(&self, events: &mpsc::Sender<RoomClientEvent>) {
        let pong = encode(&WireFrame::Connection(
            crate::collab::wire::ConnectionMessage::Pong,
        ))
        .unwrap_or_default();
        let _ = events.send(RoomClientEvent::Outbound(pong)).await;
    }

    #[allow(clippy::too_many_arguments)]
    async fn handle_document_message(
        &mut self,
        conn_id: Uuid,
        events: &mpsc::Sender<RoomClientEvent>,
        routing_key: &str,
        read_only: bool,
        client_id: u32,
        session: &CollabSession,
        conn_generation: u64,
        message: DocumentMessage,
    ) {
        match message {
            DocumentMessage::Auth(auth) => {
                self.handle_auth(events, routing_key, auth).await;
            }
            DocumentMessage::Sync(sync) => {
                self.handle_sync(conn_id, events, routing_key, read_only, sync)
                    .await;
            }
            DocumentMessage::Awareness(payload) => {
                if !self.session_authorized(session, read_only).await {
                    self.close_connection(conn_id, 1008, "permission revoked");
                    return;
                }
                self.handle_awareness(conn_id, client_id, session, conn_generation, payload);
            }
            DocumentMessage::QueryAwareness => {
                let encoded = self.awareness.encode_all();
                if !encoded.is_empty() {
                    self.send_document(events, routing_key, DocumentMessage::Awareness(encoded))
                        .await;
                }
            }
            DocumentMessage::Stateless(payload) => {
                self.handle_stateless(conn_id, events, routing_key, payload)
                    .await;
            }
            DocumentMessage::Close { .. } => {
                self.close_connection(conn_id, 1000, "client close");
            }
            _ => {}
        }
    }

    async fn handle_auth(
        &self,
        events: &mpsc::Sender<RoomClientEvent>,
        routing_key: &str,
        auth: AuthMessage,
    ) {
        if let AuthMessage::Token { .. } = auth {
            self.send_document(
                events,
                routing_key,
                DocumentMessage::Auth(AuthMessage::Authenticated {
                    scope: "read-write".into(),
                }),
            )
            .await;
        }
    }

    async fn handle_sync(
        &mut self,
        conn_id: Uuid,
        events: &mpsc::Sender<RoomClientEvent>,
        routing_key: &str,
        read_only: bool,
        sync: crate::collab::wire::SyncMessage,
    ) {
        let max_binary = crate::collab::wire::Limits::DEFAULT.max_binary_payload_bytes;
        let (step, payload) = match parse_sync_payload(&sync.y_protocol, max_binary) {
            Ok(parts) => parts,
            Err(_) => return,
        };
        match step {
            SyncStep::Step1 => {
                let report = match self
                    .engine
                    .call(Request::Sync {
                        state_vector_b64: payload,
                        encoding: 1,
                    })
                    .await
                {
                    Ok(report) => report,
                    Err(BridgeError::Dead) => return,
                };
                if let EngineStatus::Ok {
                    update_b64: Some(update),
                    ..
                } = report.outcome
                {
                    let update = b64::decode(&update).unwrap_or_default();
                    let y_protocol = encode_sync_payload(SyncStep::Step2, &update);
                    self.send_document(
                        events,
                        routing_key,
                        DocumentMessage::Sync(crate::collab::wire::SyncMessage {
                            step: SyncStep::Step2,
                            y_protocol,
                        }),
                    )
                    .await;
                }
            }
            SyncStep::Step2 | SyncStep::Update => {
                if read_only && !is_empty_update(&payload) {
                    self.send_sync_status(events, routing_key, false).await;
                    return;
                }
                if is_empty_update(&payload) {
                    self.send_sync_status(events, routing_key, true).await;
                    return;
                }
                let conn = self.connections.get_mut(&conn_id);
                if conn.map(|c| c.poisoned || c.in_flight).unwrap_or(true) {
                    self.send_sync_status(events, routing_key, false).await;
                    return;
                }
                if let Some(c) = self.connections.get_mut(&conn_id) {
                    c.in_flight = true;
                }

                let validation = validate_recovery_bundle(
                    self.engine.engine_bin().to_path_buf(),
                    self.engine.limits(),
                    self.committed.snapshot.clone(),
                    self.committed.tail_payloads.clone(),
                    payload.clone(),
                )
                .await;
                if validation != BundleValidation::Ok {
                    self.reject_candidate(conn_id, events, routing_key).await;
                    return;
                }

                let writer_generation = self.writer_generation;
                let Some(writer_generation) = writer_generation else {
                    self.reject_candidate(conn_id, events, routing_key).await;
                    return;
                };
                let (session_id, actor_user_id) = self
                    .connections
                    .get(&conn_id)
                    .map(|c| (c.session.session_id, c.session.user_id))
                    .unwrap_or_default();
                if !self
                    .session_authorized_by_ids(actor_user_id, session_id, read_only)
                    .await
                {
                    self.reject_candidate(conn_id, events, routing_key).await;
                    return;
                }

                let op_id = Uuid::now_v7();
                let expected_tail = self.committed.tail_seq;
                let digest = payload_digest(&payload);
                let append = append_collab_update(
                    &self.pool,
                    AppendCollabInput {
                        workspace_id: self.workspace_id,
                        actor_user_id,
                        session_id,
                        document_id: self.document_id,
                        writer_generation,
                        expected_tail_seq: expected_tail,
                        op_id,
                        payload: &payload,
                        client_ip: None,
                    },
                )
                .await;

                let committed = match append {
                    Ok(Ok(result)) => result,
                    Ok(Err(CollabDbError::StaleWriter)) => {
                        self.fatal_writer_stale();
                        self.reject_candidate(conn_id, events, routing_key).await;
                        return;
                    }
                    Ok(Err(_)) | Err(_) => {
                        match verify_collab_operation(
                            &self.pool,
                            VerifyCollabInput {
                                workspace_id: self.workspace_id,
                                actor_user_id,
                                session_id,
                                document_id: self.document_id,
                                op_id,
                                expected_payload_len: payload.len() as i64,
                                expected_payload_sha256: &digest,
                                expected_actor_user_id: actor_user_id,
                            },
                        )
                        .await
                        {
                            Ok(Ok(lookup)) => AppendCollabResult::DuplicateAck { seq: lookup.seq },
                            Ok(Err(CollabDbError::NotFound)) => {
                                match append_collab_update(
                                    &self.pool,
                                    AppendCollabInput {
                                        workspace_id: self.workspace_id,
                                        actor_user_id,
                                        session_id,
                                        document_id: self.document_id,
                                        writer_generation,
                                        expected_tail_seq: expected_tail,
                                        op_id,
                                        payload: &payload,
                                        client_ip: None,
                                    },
                                )
                                .await
                                {
                                    Ok(Ok(result)) => result,
                                    _ => {
                                        self.reject_candidate(conn_id, events, routing_key).await;
                                        return;
                                    }
                                }
                            }
                            _ => {
                                self.reject_candidate(conn_id, events, routing_key).await;
                                return;
                            }
                        }
                    }
                };

                let seq = match committed {
                    AppendCollabResult::Committed { seq }
                    | AppendCollabResult::DuplicateAck { seq } => seq,
                };

                self.committed.tail_payloads.push(payload.clone());
                self.committed.tail_seq = seq;
                self.fifo_seq += 1;
                let op_prefix = self.fifo_seq;

                if !self.apply_primary(&payload).await {
                    self.reload_primary_from_committed().await;
                    self.reject_candidate(conn_id, events, routing_key).await;
                    return;
                }

                self.broadcast_update(routing_key, &sync.y_protocol);
                self.send_sync_status(events, routing_key, true).await;
                if let Some(c) = self.connections.get_mut(&conn_id) {
                    c.in_flight = false;
                }
                self.flush_connection_persist(conn_id, op_prefix).await;
                self.maybe_compact().await;
            }
        }
    }

    async fn session_authorized_by_ids(
        &self,
        user_id: Uuid,
        session_id: Uuid,
        read_only: bool,
    ) -> bool {
        match resolve_collab_admission(
            &self.pool,
            self.workspace_id,
            user_id,
            session_id,
            self.document_id,
        )
        .await
        {
            Ok(Ok(admission)) => read_only || !admission.read_only,
            _ => false,
        }
    }

    async fn reject_candidate(
        &mut self,
        conn_id: Uuid,
        events: &mpsc::Sender<RoomClientEvent>,
        routing_key: &str,
    ) {
        self.reload_primary_from_committed().await;
        if let Some(c) = self.connections.get_mut(&conn_id) {
            c.in_flight = false;
            c.poisoned = true;
        }
        self.send_sync_status(events, routing_key, false).await;
        self.close_connection(conn_id, 1008, "update rejected");
    }

    fn fatal_writer_stale(&mut self) {
        for conn_id in self.connections.keys().cloned().collect::<Vec<_>>() {
            self.close_connection(conn_id, 1008, "writer stale");
        }
        self.writer_generation = None;
    }

    async fn apply_primary(&mut self, payload: &[u8]) -> bool {
        let report = match self
            .engine
            .call(Request::Apply {
                update_b64: payload.to_vec(),
                encoding: 1,
            })
            .await
        {
            Ok(report) => report,
            Err(BridgeError::Dead) => return false,
        };
        report.outcome.is_applied_ok()
    }

    async fn reload_primary_from_committed(&mut self) {
        let _ = self.engine.recycle().await;
        let _ = self.load_engine_primary().await;
    }

    async fn load_engine_primary(&mut self) -> Result<(), JoinError> {
        let tail_b64 = self.committed.tail_payloads.clone();
        let report = self
            .engine
            .call(Request::Load {
                snapshot_b64: Some(self.committed.snapshot.clone()),
                tail_b64,
                encoding: 1,
            })
            .await
            .map_err(|_| JoinError::EngineUnavailable)?;
        if report.outcome.is_applied_ok() {
            Ok(())
        } else {
            Err(JoinError::EngineUnavailable)
        }
    }

    fn handle_awareness(
        &mut self,
        conn_id: Uuid,
        client_id: u32,
        session: &CollabSession,
        conn_generation: u64,
        payload: Vec<u8>,
    ) {
        let updates = decode_awareness(&payload).unwrap_or_default();
        let display = crate::collab::awareness::display_name(
            &session.given_name,
            session.family_name.as_deref(),
        );
        let color = crate::collab::awareness::user_color(&session.user_id);
        if let Some(encoded) = self.awareness.apply_connection_updates(
            client_id,
            &updates,
            &session.user_id.to_string(),
            &display,
            &color,
            conn_generation,
        ) {
            self.broadcast_awareness(&encoded);
        }
        let _ = conn_id;
    }

    async fn handle_stateless(
        &mut self,
        conn_id: Uuid,
        events: &mpsc::Sender<RoomClientEvent>,
        routing_key: &str,
        payload: String,
    ) {
        if let Some(request_id) = payload.strip_prefix("persist:") {
            if let Ok(id) = Uuid::parse_str(request_id) {
                if self.persist_failed {
                    self.send_stateless(events, routing_key, format!("persist-failed:{id}"))
                        .await;
                    return;
                }
                let prefix = self.fifo_seq;
                if let Some(conn) = self.connections.get_mut(&conn_id) {
                    conn.pending_persist.push_back(PersistBarrier {
                        request_id: id,
                        prefix_fifo: prefix,
                    });
                }
                self.flush_connection_persist(conn_id, self.fifo_seq).await;
            }
        }
    }

    async fn flush_connection_persist(&mut self, conn_id: Uuid, current_fifo: u64) {
        let (routing_key, events, ready) = {
            let Some(conn) = self.connections.get_mut(&conn_id) else {
                return;
            };
            let ready = conn
                .pending_persist
                .iter()
                .filter(|b| b.prefix_fifo <= current_fifo)
                .map(|b| b.request_id)
                .collect::<Vec<_>>();
            (conn.routing_key.clone(), conn.events.clone(), ready)
        };
        for request_id in ready {
            if let Some(conn) = self.connections.get_mut(&conn_id) {
                conn.pending_persist.retain(|b| b.request_id != request_id);
            }
            let result = self.run_persist_for(conn_id, request_id).await;
            self.send_stateless(&events, &routing_key, result).await;
        }
    }

    fn any_pending_persist(&self) -> bool {
        self.connections
            .values()
            .any(|c| !c.pending_persist.is_empty())
    }

    async fn run_persist_for(&mut self, conn_id: Uuid, request_id: Uuid) -> String {
        if self.persist_failed {
            return format!("persist-failed:{request_id}");
        }
        let Some(conn) = self.connections.get(&conn_id) else {
            return format!("persist-failed:{request_id}");
        };
        let actor_user_id = conn.session.user_id;
        let session_id = conn.session.session_id;
        if !self
            .session_authorized_by_ids(actor_user_id, session_id, conn.read_only)
            .await
        {
            return format!("persist-failed:{request_id}");
        }

        let snapshot_report = match self.engine.call(Request::Snapshot).await {
            Ok(report) => report,
            Err(BridgeError::Dead) => {
                self.persist_failed = true;
                return format!("persist-failed:{request_id}");
            }
        };
        let snapshot = match snapshot_report.outcome {
            EngineStatus::Ok {
                update_b64: Some(bytes_b64),
                ..
            } => match b64::decode(&bytes_b64) {
                Ok(bytes) => bytes,
                Err(_) => {
                    self.persist_failed = true;
                    return format!("persist-failed:{request_id}");
                }
            },
            _ => {
                self.persist_failed = true;
                return format!("persist-failed:{request_id}");
            }
        };
        if !validate_snapshot_only(
            self.engine.engine_bin().to_path_buf(),
            self.engine.limits(),
            snapshot.clone(),
        )
        .await
        {
            self.persist_failed = true;
            return format!("persist-failed:{request_id}");
        }
        if let Some(writer_generation) = self.writer_generation {
            let cutoff = self.committed.tail_seq;
            let compact = compact_collab_snapshot(
                &self.pool,
                CompactCollabInput {
                    workspace_id: self.workspace_id,
                    actor_user_id,
                    session_id,
                    document_id: self.document_id,
                    writer_generation,
                    cutoff_seq: cutoff,
                    expected_tail_seq: cutoff,
                    new_snapshot: &snapshot,
                    client_ip: None,
                },
            )
            .await;
            match compact {
                Ok(Ok(load)) => {
                    self.set_committed_from_load(&load);
                    format!("persisted:{request_id}")
                }
                _ => {
                    self.persist_failed = true;
                    format!("persist-failed:{request_id}")
                }
            }
        } else {
            format!("persist-failed:{request_id}")
        }
    }

    async fn maybe_compact(&mut self) {
        if self.any_pending_persist() || self.persist_failed {
            return;
        }
        if self.committed.tail_payloads.len() < 32 {
            return;
        }
        let conn_id = self
            .connections
            .iter()
            .find(|(_, c)| !c.read_only && !c.pending_persist.is_empty())
            .map(|(id, _)| *id)
            .or_else(|| {
                self.connections
                    .iter()
                    .find(|(_, c)| !c.read_only)
                    .map(|(id, _)| *id)
            });
        if let Some(conn_id) = conn_id {
            let request_id = Uuid::now_v7();
            let _ = self.run_persist_for(conn_id, request_id).await;
        }
    }

    fn broadcast_update(&self, routing_key: &str, y_protocol: &[u8]) {
        let frame = encode(&WireFrame::Document {
            routing_key: routing_key.to_string(),
            room: None,
            message: DocumentMessage::Sync(crate::collab::wire::SyncMessage {
                step: SyncStep::Update,
                y_protocol: y_protocol.to_vec(),
            }),
        })
        .unwrap_or_default();
        for conn in self.connections.values() {
            let _ = conn
                .events
                .try_send(RoomClientEvent::Outbound(frame.clone()));
        }
    }

    fn broadcast_awareness(&self, encoded: &[u8]) {
        for conn in self.connections.values() {
            let frame = encode(&WireFrame::Document {
                routing_key: conn.routing_key.clone(),
                room: None,
                message: DocumentMessage::Awareness(encoded.to_vec()),
            })
            .unwrap_or_default();
            let _ = conn.events.try_send(RoomClientEvent::Outbound(frame));
        }
    }

    async fn send_document(
        &self,
        events: &mpsc::Sender<RoomClientEvent>,
        routing_key: &str,
        message: DocumentMessage,
    ) {
        if let Ok(bytes) = encode(&WireFrame::Document {
            routing_key: routing_key.to_string(),
            room: None,
            message,
        }) {
            let _ = events.send(RoomClientEvent::Outbound(bytes)).await;
        }
    }

    async fn send_sync_status(
        &self,
        events: &mpsc::Sender<RoomClientEvent>,
        routing_key: &str,
        applied: bool,
    ) {
        self.send_document(events, routing_key, DocumentMessage::SyncStatus { applied })
            .await;
    }

    async fn send_stateless(
        &self,
        events: &mpsc::Sender<RoomClientEvent>,
        routing_key: &str,
        payload: String,
    ) {
        self.send_document(events, routing_key, DocumentMessage::Stateless(payload))
            .await;
    }
}

fn payload_digest(payload: &[u8]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(payload);
    hasher.finalize().to_vec()
}

pub fn parse_client_id(token: &str) -> Option<u32> {
    if token.is_empty() || token.len() > 10 || !token.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let value = token.parse::<u64>().ok()?;
    if value > 0xFFFF_FFFF {
        return None;
    }
    Some(value as u32)
}
