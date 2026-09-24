use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[cfg(feature = "db-tests")]
use std::sync::LazyLock;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{mpsc, watch};
use uuid::Uuid;

use crate::auth::token::hash_token;
use crate::collab::config::CollabConfig;
use crate::collab::hub::CollabHub;
use crate::collab::origin::{validate_collab_origin, CollabOriginError};
use crate::collab::room::{
    parse_client_id, AuthenticatedConnection, CollabSession, ConnectionCancel, ConnectionLease,
    JoinError, OutboundFrame, OutboundKind, RoomClientEvent, RoomJoin,
};
use crate::collab::wire::{AuthMessage, CollabKind, CollabRoomName, DocumentMessage, WireFrame};
use crate::db::collab::resolve_collab_admission;
use crate::db::collab_delivery::{authorize_outbound_delivery, OutboundDeliveryAuth};
use crate::db::identity::find_live_session;
use crate::error::SESSION_COOKIE;
use crate::http::state::AppState;

fn is_ws_upgrade(headers: &HeaderMap) -> bool {
    let upgrade = headers
        .get("upgrade")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.eq_ignore_ascii_case("websocket"))
        .unwrap_or(false);
    let connection = headers
        .get("connection")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_ascii_lowercase().contains("upgrade"))
        .unwrap_or(false);
    upgrade && connection
}

fn collab_unavailable_response() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        axum::Json(serde_json::json!({
            "type": "about:blank",
            "title": "collaboration unavailable",
            "status": 503,
            "code": "collab_unavailable"
        })),
    )
        .into_response()
}

fn origin_rejected_response(err: CollabOriginError) -> Response {
    let code = match err {
        CollabOriginError::Missing => "origin_required",
        CollabOriginError::NonUtf8 | CollabOriginError::Multiple | CollabOriginError::Malformed => {
            "origin_invalid"
        }
        CollabOriginError::Mismatch => "origin_mismatch",
    };
    (
        StatusCode::FORBIDDEN,
        axum::Json(serde_json::json!({
            "type": "about:blank",
            "title": "origin rejected",
            "status": 403,
            "code": code
        })),
    )
        .into_response()
}

/// `/collab` entry: 503 when disabled, 426 for plain GET, 403 on upgrade without valid Origin.
pub async fn collab_entry(
    State(state): State<AppState>,
    headers: HeaderMap,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    ws: Result<WebSocketUpgrade, axum::extract::ws::rejection::WebSocketUpgradeRejection>,
) -> Response {
    if state.collab.is_none() {
        return collab_unavailable_response();
    }
    if !is_ws_upgrade(&headers) {
        return collab_get_without_upgrade().await.into_response();
    }
    if let Err(err) = validate_collab_origin(&headers, &state.public_origin) {
        return origin_rejected_response(err);
    }
    let ws = match ws {
        Ok(ws) => ws,
        Err(_) => return collab_get_without_upgrade().await.into_response(),
    };
    collab_upgrade(state, headers, ws, peer).await
}

async fn collab_upgrade(
    state: AppState,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
    peer: SocketAddr,
) -> Response {
    let hub = state.collab.clone().expect("collab checked");
    let session_token = cookie_value(&headers, SESSION_COOKIE);
    let pool = state.auth.db.pool.clone();
    let live = if let Some(token) = session_token {
        find_live_session(&pool, &hash_token(token))
            .await
            .ok()
            .flatten()
    } else {
        None
    };
    if live.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let session = CollabSession::from(live.unwrap());
    let socket_permit = hub.try_acquire_socket(session.session_id);
    if socket_permit.is_none() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let max_frame = hub.config().max_ws_frame_bytes;
    let max_message = hub.config().max_ws_message_bytes;
    ws.max_frame_size(max_frame)
        .max_message_size(max_message)
        .on_upgrade(move |socket| async move {
            let _socket_permit = socket_permit;
            handle_socket(socket, hub, session, peer).await;
        })
}

pub async fn collab_get_without_upgrade() -> impl IntoResponse {
    (
        StatusCode::UPGRADE_REQUIRED,
        axum::Json(serde_json::json!({
            "type": "about:blank",
            "title": "upgrade required",
            "status": 426,
            "code": "upgrade_required"
        })),
    )
}

struct PreAuthOutboundAllowance {
    remaining: u32,
    max_bytes: usize,
}

impl PreAuthOutboundAllowance {
    fn new(config: &CollabConfig) -> Self {
        Self {
            remaining: config.max_pre_auth_outbound_frames,
            max_bytes: config.max_ws_message_bytes,
        }
    }

    fn try_send(&mut self, events: &mpsc::Sender<RoomClientEvent>, bytes: Vec<u8>) -> bool {
        if self.remaining == 0 || bytes.len() > self.max_bytes {
            return false;
        }
        if events
            .try_send(RoomClientEvent::Outbound(OutboundFrame::unaccounted(bytes)))
            .is_ok()
        {
            self.remaining -= 1;
            true
        } else {
            false
        }
    }

    fn exhausted(&self) -> bool {
        self.remaining == 0
    }
}

#[cfg(feature = "db-tests")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DataFrameSendBudget {
    pub total: Duration,
    pub auth_remaining: Duration,
    pub send_remaining: Duration,
}

#[cfg(feature = "db-tests")]
static DATA_FRAME_SEND_BUDGETS: LazyLock<
    std::sync::Mutex<std::collections::HashMap<Uuid, DataFrameSendBudget>>,
> = LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

#[cfg(feature = "db-tests")]
pub fn take_data_frame_send_budget(session_id: Uuid) -> Option<DataFrameSendBudget> {
    DATA_FRAME_SEND_BUDGETS
        .lock()
        .ok()
        .and_then(|mut traces| traces.remove(&session_id))
}

#[cfg(feature = "db-tests")]
fn record_data_frame_send_budget(session_id: Uuid, budget: DataFrameSendBudget) {
    if let Ok(mut traces) = DATA_FRAME_SEND_BUDGETS.lock() {
        traces.insert(session_id, budget);
    }
}

fn remaining_until(deadline: tokio::time::Instant) -> Duration {
    deadline.saturating_duration_since(tokio::time::Instant::now())
}

/// Best-effort bound for writing a WebSocket Close after a Data-path failure.
/// Distinct from the shared Data auth+send budget: leftover time from an
/// already-expired dequeue deadline cannot deliver 1011. Close uses
/// `min(send_deadline, this grace)` from `Instant::now()` at the Close attempt.
const CLEANUP_CLOSE_GRACE: Duration = Duration::from_millis(250);

fn cleanup_close_deadline(send_deadline: Duration) -> tokio::time::Instant {
    tokio::time::Instant::now() + send_deadline.min(CLEANUP_CLOSE_GRACE)
}

async fn send_cleanup_close(
    sender: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    code: u16,
    reason: &str,
    send_deadline: Duration,
    max_frame_bytes: usize,
) {
    send_close_until(
        sender,
        code,
        reason,
        cleanup_close_deadline(send_deadline),
        max_frame_bytes,
    )
    .await;
}

async fn send_ws_message_until(
    sender: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    message: Message,
    deadline: tokio::time::Instant,
    max_frame_bytes: usize,
) -> bool {
    let byte_len = match &message {
        Message::Binary(bytes) => bytes.len(),
        Message::Close(frame) => frame.as_ref().map(|f| f.reason.len()).unwrap_or(0),
        Message::Ping(payload) | Message::Pong(payload) => payload.len(),
        _ => 0,
    };
    if byte_len > max_frame_bytes {
        return false;
    }
    matches!(
        tokio::time::timeout_at(deadline, sender.send(message)).await,
        Ok(Ok(()))
    )
}

async fn send_ws_message(
    sender: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    message: Message,
    send_deadline: Duration,
    max_frame_bytes: usize,
) -> bool {
    send_ws_message_until(
        sender,
        message,
        tokio::time::Instant::now() + send_deadline,
        max_frame_bytes,
    )
    .await
}

async fn send_close_until(
    sender: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    code: u16,
    reason: &str,
    deadline: tokio::time::Instant,
    max_frame_bytes: usize,
) {
    let _ = send_ws_message_until(
        sender,
        Message::Close(Some(axum::extract::ws::CloseFrame {
            code,
            reason: reason.into(),
        })),
        deadline,
        max_frame_bytes,
    )
    .await;
}

async fn send_close(
    sender: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    code: u16,
    reason: &str,
    send_deadline: Duration,
    max_frame_bytes: usize,
) {
    send_close_until(
        sender,
        code,
        reason,
        tokio::time::Instant::now() + send_deadline,
        max_frame_bytes,
    )
    .await;
}

async fn handle_socket(
    socket: WebSocket,
    hub: Arc<CollabHub>,
    live: CollabSession,
    _peer: SocketAddr,
) {
    let config = hub.config();
    let (mut sender, mut receiver) = socket.split();
    let conn_id = Uuid::now_v7();
    let (events_tx, mut events_rx) =
        mpsc::channel(config.max_outbound_frames_per_connection.max(8));
    let (cancel_tx, mut cancel_rx) = watch::channel(None::<ConnectionCancel>);
    let mut joined_room: Option<(crate::collab::room::RoomKey, String, bool, bool)> = None;
    let mut connection_lease: Option<ConnectionLease> = None;
    let send_deadline = Duration::from_millis(config.outbound_send_deadline_ms);
    let max_frame_bytes = config.max_ws_frame_bytes;
    let auth_deadline = Instant::now() + Duration::from_millis(config.auth_wait_ms);
    let mut auth_wait = Box::pin(tokio::time::sleep_until(tokio::time::Instant::from_std(
        auth_deadline,
    )));
    let mut collab_authenticated = false;
    let mut pre_auth_outbound = PreAuthOutboundAllowance::new(config);
    let mut inbound_window_start = Instant::now();
    let mut inbound_window_count = 0u32;

    loop {
        tokio::select! {
            _ = auth_wait.as_mut(), if !collab_authenticated => {
                break;
            }
            room_event = events_rx.recv() => {
                match room_event {
                    Some(RoomClientEvent::Outbound(outbound)) => {
                        if outbound.kind == OutboundKind::Data {
                            if let Some((key, _, authenticated, read_only)) = &joined_room {
                                if *authenticated {
                                    let workspace_id = key.0;
                                    let document_id = key.1;
                                    let read_only = *read_only;
                                    let deadline =
                                        tokio::time::Instant::now() + send_deadline;
                                    #[cfg(feature = "db-tests")]
                                    let auth_remaining = remaining_until(deadline);
                                    let auth = tokio::select! {
                                        biased;
                                        _ = cancel_rx.changed() => {
                                            let cancel = cancel_rx.borrow_and_update().clone();
                                            if let Some(cancel) = cancel {
                                                send_cleanup_close(
                                                    &mut sender,
                                                    cancel.code,
                                                    &cancel.reason,
                                                    send_deadline,
                                                    max_frame_bytes,
                                                )
                                                .await;
                                            }
                                            break;
                                        }
                                        auth = tokio::time::timeout_at(
                                            deadline,
                                            authorize_outbound_delivery(
                                                hub.pool(),
                                                workspace_id,
                                                live.user_id,
                                                live.session_id,
                                                document_id,
                                            ),
                                        ) => auth,
                                    };
                                    match auth {
                                        Err(_) | Ok(OutboundDeliveryAuth::DbError) => {
                                            send_cleanup_close(
                                                &mut sender,
                                                1011,
                                                "authorization unavailable",
                                                send_deadline,
                                                max_frame_bytes,
                                            )
                                            .await;
                                            break;
                                        }
                                        Ok(OutboundDeliveryAuth::Denied) => {
                                            send_cleanup_close(
                                                &mut sender,
                                                1008,
                                                "permission revoked",
                                                send_deadline,
                                                max_frame_bytes,
                                            )
                                            .await;
                                            break;
                                        }
                                        Ok(OutboundDeliveryAuth::Allowed {
                                            read_only: admission_ro,
                                        }) => {
                                            if !(read_only || !admission_ro) {
                                                send_cleanup_close(
                                                    &mut sender,
                                                    1008,
                                                    "permission revoked",
                                                    send_deadline,
                                                    max_frame_bytes,
                                                )
                                                .await;
                                                break;
                                            }
                                        }
                                    }
                                    let send_remaining = remaining_until(deadline);
                                    #[cfg(feature = "db-tests")]
                                    record_data_frame_send_budget(
                                        live.session_id,
                                        DataFrameSendBudget {
                                            total: send_deadline,
                                            auth_remaining,
                                            send_remaining,
                                        },
                                    );
                                    if send_remaining.is_zero() {
                                        send_cleanup_close(
                                            &mut sender,
                                            1011,
                                            "authorization unavailable",
                                            send_deadline,
                                            max_frame_bytes,
                                        )
                                        .await;
                                        break;
                                    }
                                    if !send_ws_message_until(
                                        &mut sender,
                                        Message::Binary(outbound.bytes.into()),
                                        deadline,
                                        max_frame_bytes,
                                    )
                                    .await
                                    {
                                        break;
                                    }
                                    continue;
                                }
                            }
                        }
                        if !send_ws_message(
                            &mut sender,
                            Message::Binary(outbound.bytes.into()),
                            send_deadline,
                            max_frame_bytes,
                        )
                        .await
                        {
                            break;
                        }
                    }
                    Some(RoomClientEvent::Close { code, reason }) => {
                        send_close(
                            &mut sender,
                            code,
                            &reason,
                            send_deadline,
                            max_frame_bytes,
                        )
                        .await;
                        break;
                    }
                    None => break,
                }
            }
            _ = cancel_rx.changed() => {
                let cancel = {
                    let guard = cancel_rx.borrow_and_update();
                    guard.clone()
                };
                if let Some(cancel) = cancel {
                    send_close(
                        &mut sender,
                        cancel.code,
                        &cancel.reason,
                        send_deadline,
                        max_frame_bytes,
                    )
                    .await;
                    break;
                }
            }
            inbound = receiver.next() => {
                match inbound {
                    Some(Ok(Message::Binary(bytes))) => {
                        if bytes.len() > config.max_ws_message_bytes {
                            break;
                        }
                        let now = Instant::now();
                        if now.duration_since(inbound_window_start)
                            > Duration::from_millis(config.inbound_message_window_ms)
                        {
                            inbound_window_start = now;
                            inbound_window_count = 0;
                        }
                        inbound_window_count += 1;
                        if inbound_window_count > config.max_inbound_messages_per_window {
                            break;
                        }
                        if let Some((key, routing_key, authenticated, _read_only)) = &joined_room {
                            if *authenticated {
                                hub.send_frame(*key, conn_id, bytes.to_vec()).await;
                            } else {
                                match try_authenticate(
                                    &hub,
                                    conn_id,
                                    &live,
                                    routing_key,
                                    &bytes,
                                    ConnectionEvents { events: &events_tx, cancel: &cancel_tx },
                                    &mut pre_auth_outbound,
                                )
                                .await
                                {
                                    AuthAttempt::Joined { read_only, lease } => {
                                        joined_room =
                                            Some((*key, routing_key.clone(), true, read_only));
                                        connection_lease = Some(lease);
                                        collab_authenticated = true;
                                    }
                                    AuthAttempt::Denied => {}
                                    AuthAttempt::Closed => {
                                        send_close(
                                            &mut sender,
                                            1013,
                                            "pre-auth outbound exhausted",
                                            send_deadline,
                                            max_frame_bytes,
                                        )
                                        .await;
                                        break;
                                    }
                                }
                            }
                        } else {
                            match first_room_from_frame(
                                &bytes,
                                &live,
                                conn_id,
                                &hub,
                                &events_tx,
                                &cancel_tx,
                                &mut pre_auth_outbound,
                            )
                            .await
                            {
                                FirstRoom::Joined {
                                    key,
                                    routing,
                                    read_only,
                                    lease,
                                } => {
                                    joined_room = Some((key, routing, true, read_only));
                                    connection_lease = Some(lease);
                                    collab_authenticated = true;
                                }
                                FirstRoom::Pending { key, routing } => {
                                    joined_room = Some((key, routing, false, false));
                                }
                                FirstRoom::None => {}
                                FirstRoom::Closed => {
                                    send_close(
                                        &mut sender,
                                        1013,
                                        "pre-auth outbound exhausted",
                                        send_deadline,
                                        max_frame_bytes,
                                    )
                                    .await;
                                    break;
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        if payload.len() > config.max_ws_frame_bytes {
                            break;
                        }
                        if !send_ws_message(
                            &mut sender,
                            Message::Pong(payload),
                            send_deadline,
                            max_frame_bytes,
                        )
                        .await
                        {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }
    if let Some((key, _, _, _)) = joined_room {
        hub.leave_room(key, conn_id).await;
    }
    drop(connection_lease);
}

async fn first_room_from_frame(
    bytes: &[u8],
    live: &CollabSession,
    conn_id: Uuid,
    hub: &Arc<CollabHub>,
    events: &mpsc::Sender<RoomClientEvent>,
    cancel: &watch::Sender<Option<ConnectionCancel>>,
    pre_auth_outbound: &mut PreAuthOutboundAllowance,
) -> FirstRoom {
    let frame = crate::collab::wire::decode(bytes).ok();
    let Some(WireFrame::Document {
        routing_key,
        room,
        message,
    }) = frame
    else {
        return FirstRoom::None;
    };
    let Some(room_name) = room else {
        return FirstRoom::None;
    };
    if room_name.kind != CollabKind::Document {
        if !send_auth_denied(pre_auth_outbound, events, &routing_key, "unsupported kind") {
            signal_pre_auth_close(cancel);
            return FirstRoom::Closed;
        }
        return FirstRoom::None;
    }
    let key = (room_name.workspace_id, room_name.resource_id);
    if matches!(message, DocumentMessage::Auth(AuthMessage::Token { .. })) {
        match try_authenticate(
            hub,
            conn_id,
            live,
            &routing_key,
            bytes,
            ConnectionEvents { events, cancel },
            pre_auth_outbound,
        )
        .await
        {
            AuthAttempt::Joined { read_only, lease } => FirstRoom::Joined {
                key,
                routing: routing_key,
                read_only,
                lease,
            },
            AuthAttempt::Denied => FirstRoom::Pending {
                key,
                routing: routing_key,
            },
            AuthAttempt::Closed => FirstRoom::Closed,
        }
    } else {
        FirstRoom::Pending {
            key,
            routing: routing_key,
        }
    }
}

enum FirstRoom {
    Joined {
        key: crate::collab::room::RoomKey,
        routing: String,
        read_only: bool,
        lease: ConnectionLease,
    },
    Pending {
        key: crate::collab::room::RoomKey,
        routing: String,
    },
    None,
    Closed,
}

enum AuthAttempt {
    Joined {
        read_only: bool,
        lease: ConnectionLease,
    },
    Denied,
    Closed,
}

fn signal_pre_auth_close(cancel: &watch::Sender<Option<ConnectionCancel>>) {
    let _ = cancel.send_replace(Some(ConnectionCancel {
        code: 1013,
        reason: "pre-auth outbound exhausted".into(),
    }));
}

struct ConnectionEvents<'a> {
    events: &'a mpsc::Sender<RoomClientEvent>,
    cancel: &'a watch::Sender<Option<ConnectionCancel>>,
}

async fn try_authenticate(
    hub: &Arc<CollabHub>,
    conn_id: Uuid,
    live: &CollabSession,
    routing_key: &str,
    bytes: &[u8],
    channels: ConnectionEvents<'_>,
    pre_auth_outbound: &mut PreAuthOutboundAllowance,
) -> AuthAttempt {
    let ConnectionEvents { events, cancel } = channels;
    let frame = crate::collab::wire::decode(bytes).ok();
    let token = match frame {
        Some(WireFrame::Document {
            message: DocumentMessage::Auth(AuthMessage::Token { token, .. }),
            ..
        }) => token,
        _ => return AuthAttempt::Denied,
    };
    let client_id = parse_client_id(&token);
    if client_id.is_none() {
        if !send_auth_denied(pre_auth_outbound, events, routing_key, "unauthorized") {
            signal_pre_auth_close(cancel);
            return AuthAttempt::Closed;
        }
        return AuthAttempt::Denied;
    }
    let room = CollabRoomName::parse(routing_key.split('\0').next().unwrap_or(routing_key));
    let room = match room {
        Some(r) if r.kind == CollabKind::Document => r,
        _ => {
            if !send_auth_denied(pre_auth_outbound, events, routing_key, "not found") {
                signal_pre_auth_close(cancel);
                return AuthAttempt::Closed;
            }
            return AuthAttempt::Denied;
        }
    };
    let admission = match resolve_collab_admission(
        hub.pool(),
        room.workspace_id,
        live.user_id,
        live.session_id,
        room.resource_id,
    )
    .await
    {
        Ok(Ok(admission)) => admission,
        _ => {
            if !send_auth_denied(pre_auth_outbound, events, routing_key, "not found") {
                signal_pre_auth_close(cancel);
                return AuthAttempt::Closed;
            }
            return AuthAttempt::Denied;
        }
    };
    let read_only = admission.read_only;
    let join = RoomJoin {
        conn: AuthenticatedConnection {
            conn_id,
            session: live.clone(),
            client_id: client_id.unwrap(),
            read_only,
            routing_key: routing_key.to_string(),
        },
        events: events.clone(),
        cancel: Some(cancel.clone()),
    };
    match hub
        .join_room((room.workspace_id, room.resource_id), join)
        .await
    {
        Ok(lease) => {
            let scope = if read_only { "readonly" } else { "read-write" };
            if send_auth_ok(pre_auth_outbound, events, routing_key, scope) {
                AuthAttempt::Joined { read_only, lease }
            } else {
                hub.leave_room((room.workspace_id, room.resource_id), conn_id)
                    .await;
                drop(lease);
                signal_pre_auth_close(cancel);
                AuthAttempt::Closed
            }
        }
        Err(JoinError::AdmissionDenied) => {
            if !send_auth_denied(pre_auth_outbound, events, routing_key, "not found") {
                signal_pre_auth_close(cancel);
                return AuthAttempt::Closed;
            }
            AuthAttempt::Denied
        }
        Err(JoinError::UnsupportedKind) => {
            if !send_auth_denied(pre_auth_outbound, events, routing_key, "unsupported kind") {
                signal_pre_auth_close(cancel);
                return AuthAttempt::Closed;
            }
            AuthAttempt::Denied
        }
        Err(_) => {
            if !send_auth_denied(pre_auth_outbound, events, routing_key, "unauthorized") {
                signal_pre_auth_close(cancel);
                return AuthAttempt::Closed;
            }
            AuthAttempt::Denied
        }
    }
}

fn send_auth_ok(
    pre_auth: &mut PreAuthOutboundAllowance,
    events: &mpsc::Sender<RoomClientEvent>,
    routing_key: &str,
    scope: &str,
) -> bool {
    let frame = crate::collab::wire::encode(&WireFrame::Document {
        routing_key: routing_key.to_string(),
        room: None,
        message: DocumentMessage::Auth(AuthMessage::Authenticated {
            scope: scope.into(),
        }),
    })
    .unwrap_or_default();
    pre_auth.try_send(events, frame)
}

fn send_auth_denied(
    pre_auth: &mut PreAuthOutboundAllowance,
    events: &mpsc::Sender<RoomClientEvent>,
    routing_key: &str,
    reason: &str,
) -> bool {
    if pre_auth.exhausted() {
        return false;
    }
    let frame = crate::collab::wire::encode(&WireFrame::Document {
        routing_key: routing_key.to_string(),
        room: None,
        message: DocumentMessage::Auth(AuthMessage::PermissionDenied {
            reason: reason.into(),
        }),
    })
    .unwrap_or_default();
    pre_auth.try_send(events, frame)
}

fn cookie_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get("cookie")
        .and_then(|v| v.to_str().ok())
        .and_then(|cookie| {
            cookie.split(';').find_map(|part| {
                let part = part.trim();
                part.strip_prefix(name)
                    .and_then(|rest| rest.strip_prefix('='))
                    .map(str::trim)
            })
        })
}
