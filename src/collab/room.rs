use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use collab_engine::b64;
use collab_engine::outcome::EngineStatus;
use collab_engine::protocol::Request;
use sqlx::postgres::PgPool;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use crate::collab::awareness::{decode_awareness, AwarenessRegistry};
use crate::collab::config::CollabConfig;
use crate::collab::engine_bridge::{BridgeError, EngineBridge, fresh_validate_snapshot};
use crate::collab::wire::{encode, AuthMessage, DocumentMessage, SyncStep, WireFrame};
use crate::collab::y_sync::{encode_sync_payload, is_empty_update, parse_sync_payload};
use crate::db::collab::{
    append_collab_update, claim_writer_and_load, compact_collab_snapshot,
    load_collab_readonly, resolve_collab_admission, AppendCollabInput, CollabDbError,
    CompactCollabInput, COLLAB_ROOM_SESSION_LOCK_NAMESPACE,
};
use crate::db::context::lock_key_from_uuid;
use crate::db::identity::LiveSession;

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
    pub async fn join(
        &self,
        join: RoomJoin,
    ) -> Result<(), JoinError> {
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
        let _ = self
            .tx
            .send(RoomCommand::Frame { conn_id, bytes })
            .await;
    }

    pub async fn shutdown(&self) {
        let _ = self.tx.send(RoomCommand::Shutdown).await;
    }
}

struct ConnectionState {
    session: CollabSession,
    client_id: u32,
    read_only: bool,
    routing_key: String,
    events: mpsc::Sender<RoomClientEvent>,
    conn_generation: u64,
    pending_bytes: usize,
}

struct PersistBarrier {
    request_id: Uuid,
    conn_id: Uuid,
    prefix_seq: u64,
}

struct RoomActor {
    workspace_id: Uuid,
    document_id: Uuid,
    config: CollabConfig,
    pool: PgPool,
    engine: EngineBridge,
    writer_generation: Option<i64>,
    tail_seq: i64,
    snapshot_cutoff_seq: i64,
    connections: HashMap<Uuid, ConnectionState>,
    awareness: AwarenessRegistry,
    fifo_seq: u64,
    persist_failed: bool,
    persist_pinned: bool,
    pending_persist: VecDeque<PersistBarrier>,
    client_id_owner: HashMap<u32, (Uuid, Instant)>,
    guard_conn: Option<sqlx::pool::PoolConnection<sqlx::Postgres>>,
    shutting_down: bool,
}

pub async fn spawn_room(
    workspace_id: Uuid,
    document_id: Uuid,
    config: CollabConfig,
    pool: PgPool,
) -> Result<(RoomHandle, oneshot::Receiver<()>), JoinError> {
    let engine = EngineBridge::spawn(config.engine_bin.clone(), config.limits)
        .map_err(|_| JoinError::EngineUnavailable)?;
    let guard_conn = acquire_room_guard(&pool, document_id)
        .await
        .map_err(|_| JoinError::DbError)?;
    let (tx, rx) = mpsc::channel(config.max_queued_room_ops);
    let (finished_tx, finished_rx) = oneshot::channel();
    let actor = RoomActor {
        workspace_id,
        document_id,
        config,
        pool,
        engine,
        writer_generation: None,
        tail_seq: 0,
        snapshot_cutoff_seq: 0,
        connections: HashMap::new(),
        awareness: AwarenessRegistry::new(),
        fifo_seq: 0,
        persist_failed: false,
        persist_pinned: false,
        pending_persist: VecDeque::new(),
        client_id_owner: HashMap::new(),
        guard_conn: Some(guard_conn),
        shutting_down: false,
    };
    tokio::spawn(async move {
        actor.run(rx).await;
        let _ = finished_tx.send(());
    });
    Ok((RoomHandle { tx }, finished_rx))
}

async fn acquire_room_guard(
    pool: &PgPool,
    document_id: Uuid,
) -> Result<sqlx::pool::PoolConnection<sqlx::Postgres>, sqlx::Error> {
    let mut conn = pool.acquire().await?;
    sqlx::query("SELECT pg_advisory_lock($1, $2)")
        .bind(COLLAB_ROOM_SESSION_LOCK_NAMESPACE)
        .bind(lock_key_from_uuid(document_id))
        .execute(&mut *conn)
        .await?;
    Ok(conn)
}

async fn release_room_guard(
    conn: Option<sqlx::pool::PoolConnection<sqlx::Postgres>>,
    document_id: Uuid,
) {
    if let Some(mut conn) = conn {
        let _ = sqlx::query("SELECT pg_advisory_unlock($1, $2)")
            .bind(COLLAB_ROOM_SESSION_LOCK_NAMESPACE)
            .bind(lock_key_from_uuid(document_id))
            .execute(&mut *conn)
            .await;
    }
}

impl RoomActor {
    async fn run(mut self, mut rx: mpsc::Receiver<RoomCommand>) {
        while let Some(cmd) = rx.recv().await {
            match cmd {
                RoomCommand::Join(join, reply) => {
                    let result = self.handle_join(join).await;
                    let _ = reply.send(result);
                }
                RoomCommand::Leave(conn_id) => {
                    self.handle_leave(conn_id);
                }
                RoomCommand::Frame { conn_id, bytes } => {
                    self.handle_frame(conn_id, bytes).await;
                }
                RoomCommand::Shutdown => {
                    self.shutting_down = true;
                    break;
                }
            }
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
        let _ = self.engine.kill_and_reap().await;
        release_room_guard(self.guard_conn.take(), self.document_id).await;
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
            self.tail_seq = claim.load.tail_seq;
            self.snapshot_cutoff_seq = claim.load.snapshot_cutoff_seq;
            self.load_engine(&claim.load.snapshot, &claim.load.tail).await?;
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
            self.tail_seq = load.tail_seq;
            self.snapshot_cutoff_seq = load.snapshot_cutoff_seq;
            self.load_engine(&load.snapshot, &load.tail).await?;
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
            },
        );
        Ok(())
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
        if let Some(conn) = self.connections.remove(&conn_id) {
            if let Some(encoded) = self
                .awareness
                .remove_client(conn.client_id, conn.conn_generation)
            {
                self.broadcast_awareness(&encoded);
            }
            self.client_id_owner.remove(&conn.client_id);
        }
    }

    async fn handle_frame(&mut self, conn_id: Uuid, bytes: Vec<u8>) {
        let Some(conn) = self.connections.get_mut(&conn_id) else {
            return;
        };
        if conn.pending_bytes + bytes.len() > self.config.max_pending_bytes_per_connection {
            let _ = conn
                .events
                .send(RoomClientEvent::Close {
                    code: 1009,
                    reason: "pending bytes exceeded".into(),
                })
                .await;
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
                let _ = events
                    .send(RoomClientEvent::Close {
                        code: 1003,
                        reason: "invalid frame".into(),
                    })
                    .await;
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
                    let _ = events
                        .send(RoomClientEvent::Close {
                            code: 1008,
                            reason: "invalid room".into(),
                        })
                        .await;
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
                self.handle_sync(
                    conn_id,
                    events,
                    routing_key,
                    read_only,
                    sync,
                )
                .await;
            }
            DocumentMessage::Awareness(payload) => {
                self.handle_awareness(
                    conn_id,
                    client_id,
                    session,
                    conn_generation,
                    routing_key,
                    payload,
                )
                .await;
            }
            DocumentMessage::QueryAwareness => {
                let encoded = self.awareness.encode_all();
                if !encoded.is_empty() {
                    self.send_document(
                        events,
                        routing_key,
                        DocumentMessage::Awareness(encoded),
                    )
                    .await;
                }
            }
            DocumentMessage::Stateless(payload) => {
                self.handle_stateless(conn_id, events, routing_key, payload)
                    .await;
            }
            DocumentMessage::Close { .. } => {
                let _ = events
                    .send(RoomClientEvent::Close {
                        code: 1000,
                        reason: "client close".into(),
                    })
                    .await;
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
            let scope = "read-write";
            self.send_document(
                events,
                routing_key,
                DocumentMessage::Auth(AuthMessage::Authenticated {
                    scope: scope.into(),
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
                let report = match self.engine.call(Request::Sync {
                    state_vector_b64: payload,
                    encoding: 1,
                }).await {
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
                let report = match self.engine.call(Request::Apply {
                    update_b64: payload.clone(),
                    encoding: 1,
                }).await {
                    Ok(report) => report,
                    Err(BridgeError::Dead) => {
                        self.send_sync_status(events, routing_key, false).await;
                        return;
                    }
                };
                if !report.outcome.is_applied_ok() {
                    self.reload_engine_from_db().await;
                    self.send_sync_status(events, routing_key, false).await;
                    return;
                }
                let snapshot_report = match self.engine.call(Request::Snapshot).await {
                    Ok(report) => report,
                    Err(BridgeError::Dead) => {
                        self.reload_engine_from_db().await;
                        self.send_sync_status(events, routing_key, false).await;
                        return;
                    }
                };
                let proposed = match snapshot_report.outcome {
                    EngineStatus::Ok {
                        update_b64: Some(bytes_b64),
                        ..
                    } => match b64::decode(&bytes_b64) {
                        Ok(bytes) => bytes,
                        Err(_) => {
                            self.reload_engine_from_db().await;
                            self.send_sync_status(events, routing_key, false).await;
                            return;
                        }
                    },
                    _ => {
                        self.reload_engine_from_db().await;
                        self.send_sync_status(events, routing_key, false).await;
                        return;
                    }
                };
                if !fresh_validate_snapshot(
                    self.engine.engine_bin().to_path_buf(),
                    self.engine.limits(),
                    proposed,
                )
                .await
                {
                    self.reload_engine_from_db().await;
                    self.send_sync_status(events, routing_key, false).await;
                    return;
                }
                self.fifo_seq += 1;
                let op_prefix = self.fifo_seq;
                if let Some(writer_generation) = self.writer_generation {
                    let op_id = Uuid::now_v7();
                    let append = append_collab_update(
                        &self.pool,
                        AppendCollabInput {
                            workspace_id: self.workspace_id,
                            actor_user_id: self.connections.get(&conn_id).map(|c| c.session.user_id).unwrap_or_default(),
                            session_id: self.connections.get(&conn_id).map(|c| c.session.session_id).unwrap_or_default(),
                            document_id: self.document_id,
                            writer_generation,
                            op_id,
                            payload: &payload,
                            client_ip: None,
                        },
                    )
                    .await;
                    match append {
                        Ok(Ok(result)) => {
                            self.tail_seq = match result {
                                crate::db::collab::AppendCollabResult::Committed { seq } => seq,
                                crate::db::collab::AppendCollabResult::DuplicateAck { seq } => seq,
                            };
                            self.broadcast_update(routing_key, &sync.y_protocol);
                            self.send_sync_status(events, routing_key, true).await;
                            self.resolve_persist_barriers(op_prefix).await;
                        }
                        _ => {
                            self.reload_engine_from_db().await;
                            self.send_sync_status(events, routing_key, false).await;
                        }
                    }
                } else {
                    self.send_sync_status(events, routing_key, false).await;
                }
            }
        }
    }

    async fn handle_awareness(
        &mut self,
        _conn_id: Uuid,
        client_id: u32,
        session: &CollabSession,
        conn_generation: u64,
        _routing_key: &str,
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
                self.pending_persist.push_back(PersistBarrier {
                    request_id: id,
                    conn_id,
                    prefix_seq: prefix,
                });
                self.flush_persist_barriers(events, routing_key).await;
            }
        }
    }

    async fn flush_persist_barriers(
        &mut self,
        events: &mpsc::Sender<RoomClientEvent>,
        routing_key: &str,
    ) {
        while let Some(front) = self.pending_persist.front() {
            if front.prefix_seq > self.fifo_seq {
                break;
            }
            let barrier = self.pending_persist.pop_front().unwrap();
            let result = self.run_persist(barrier.request_id).await;
            if self.connections.contains_key(&barrier.conn_id) {
                self.send_stateless(events, routing_key, result).await;
            }
        }
    }

    async fn resolve_persist_barriers(&mut self, _current_seq: u64) {
        let ready = self
            .pending_persist
            .iter()
            .filter(|b| b.prefix_seq <= self.fifo_seq)
            .map(|b| (b.conn_id, b.request_id))
            .collect::<Vec<_>>();
        for (conn_id, request_id) in ready {
            self.pending_persist.retain(|b| b.request_id != request_id);
            let result = self.run_persist(request_id).await;
            if let Some(conn) = self.connections.get(&conn_id) {
                self.send_stateless(&conn.events, &conn.routing_key, result)
                    .await;
            }
        }
    }

    async fn run_persist(&mut self, request_id: Uuid) -> String {
        if self.persist_failed {
            return format!("persist-failed:{request_id}");
        }
        let snapshot_report = match self.engine.call(Request::Snapshot).await {
            Ok(report) => report,
            Err(BridgeError::Dead) => {
                self.persist_failed = true;
                self.persist_pinned = true;
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
                    self.persist_pinned = true;
                    return format!("persist-failed:{request_id}");
                }
            },
            _ => {
                self.persist_failed = true;
                self.persist_pinned = true;
                return format!("persist-failed:{request_id}");
            }
        };
        if !fresh_validate_snapshot(
            self.engine.engine_bin().to_path_buf(),
            self.engine.limits(),
            snapshot.clone(),
        )
        .await
        {
            self.persist_failed = true;
            self.persist_pinned = true;
            return format!("persist-failed:{request_id}");
        }
        if let Some(writer_generation) = self.writer_generation {
            let cutoff = self.tail_seq;
            let compact = compact_collab_snapshot(
                &self.pool,
                CompactCollabInput {
                    workspace_id: self.workspace_id,
                    actor_user_id: self
                        .connections
                        .values()
                        .next()
                        .map(|c| c.session.user_id)
                        .unwrap_or_default(),
                    session_id: self
                        .connections
                        .values()
                        .next()
                        .map(|c| c.session.session_id)
                        .unwrap_or_default(),
                    document_id: self.document_id,
                    writer_generation,
                    cutoff_seq: cutoff,
                    expected_tail_seq: self.tail_seq,
                    new_snapshot: &snapshot,
                    client_ip: None,
                },
            )
            .await;
            match compact {
                Ok(Ok(load)) => {
                    self.snapshot_cutoff_seq = load.snapshot_cutoff_seq;
                    self.tail_seq = load.tail_seq;
                    format!("persisted:{request_id}")
                }
                _ => {
                    self.persist_failed = true;
                    self.persist_pinned = true;
                    format!("persist-failed:{request_id}")
                }
            }
        } else {
            format!("persist-failed:{request_id}")
        }
    }

    async fn load_engine(
        &mut self,
        snapshot: &[u8],
        tail: &[crate::db::collab::CollabUpdateRow],
    ) -> Result<(), JoinError> {
        let tail_b64 = tail.iter().map(|row| row.payload.clone()).collect();
        let report = self
            .engine
            .call(Request::Load {
                snapshot_b64: Some(snapshot.to_vec()),
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

    async fn reload_engine_from_db(&mut self) {
        let _ = self.engine.kill_and_reap().await;
        if self.writer_generation.is_some() {
            if let Ok(Ok(claim)) = claim_writer_and_load(
                &self.pool,
                self.workspace_id,
                self.connections
                    .values()
                    .next()
                    .map(|c| c.session.user_id)
                    .unwrap_or_default(),
                self.connections
                    .values()
                    .next()
                    .map(|c| c.session.session_id)
                    .unwrap_or_default(),
                self.document_id,
            )
            .await
            {
                self.writer_generation = Some(claim.writer_generation);
                self.tail_seq = claim.load.tail_seq;
                self.snapshot_cutoff_seq = claim.load.snapshot_cutoff_seq;
                let _ = self.load_engine(&claim.load.snapshot, &claim.load.tail).await;
            }
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
        for (id, conn) in &self.connections {
            let _ = id;
            let _ = conn.events.try_send(RoomClientEvent::Outbound(frame.clone()));
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
        self.send_document(
            events,
            routing_key,
            DocumentMessage::SyncStatus { applied },
        )
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
