use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{mpsc, watch};
use uuid::Uuid;

use crate::auth::token::hash_token;
use crate::collab::hub::CollabHub;
use crate::collab::origin::{validate_collab_origin, CollabOriginError};
use crate::collab::config::CollabConfig;
use crate::collab::room::{
    parse_client_id, AuthenticatedConnection, CollabSession, ConnectionCancel, JoinError,
    OutboundFrame, RoomClientEvent, RoomJoin,
};
use crate::collab::wire::{AuthMessage, CollabKind, CollabRoomName, DocumentMessage, WireFrame};
use crate::db::collab::resolve_collab_admission;
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
    let socket_permit = hub.try_acquire_socket();
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
}

async fn send_ws_message(
    sender: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    message: Message,
    send_deadline: Duration,
    max_frame_bytes: usize,
) -> bool {
    let byte_len = match &message {
        Message::Binary(bytes) => bytes.len(),
        Message::Close(frame) => frame
            .as_ref()
            .map(|f| f.reason.len())
            .unwrap_or(0),
        Message::Ping(payload) | Message::Pong(payload) => payload.len(),
        _ => 0,
    };
    if byte_len > max_frame_bytes {
        return false;
    }
    match tokio::time::timeout(send_deadline, sender.send(message)).await {
        Ok(Ok(())) => true,
        _ => false,
    }
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
    let send_deadline = Duration::from_millis(config.outbound_send_deadline_ms);
    let max_frame_bytes = config.max_ws_frame_bytes;
    let auth_deadline =
        Instant::now() + Duration::from_millis(config.auth_wait_ms);
    let mut auth_wait = Box::pin(tokio::time::sleep_until(
        tokio::time::Instant::from_std(auth_deadline),
    ));
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
                        if let Some((key, _, authenticated, read_only)) = &joined_room {
                            if *authenticated
                                && !hub
                                    .session_still_authorized(key.0, key.1, &live, *read_only)
                                    .await
                            {
                                break;
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
                        let _ = send_ws_message(
                            &mut sender,
                            Message::Close(Some(axum::extract::ws::CloseFrame {
                                code,
                                reason: reason.into(),
                            })),
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
                    let _ = send_ws_message(
                        &mut sender,
                        Message::Close(Some(axum::extract::ws::CloseFrame {
                            code: cancel.code,
                            reason: cancel.reason.into(),
                        })),
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
                                let (authenticated, joined_read_only) = try_authenticate(
                                    &hub,
                                    conn_id,
                                    &live,
                                    routing_key,
                                    &bytes,
                                    &events_tx,
                                    &cancel_tx,
                                    &mut pre_auth_outbound,
                                )
                                .await;
                                if authenticated {
                                    joined_room =
                                        Some((*key, routing_key.clone(), true, joined_read_only));
                                    collab_authenticated = true;
                                }
                            }
                        } else if let Some((key, routing, auth_ok, read_only)) =
                            first_room_from_frame(
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
                            joined_room = Some((key, routing, auth_ok, read_only));
                            if auth_ok {
                                collab_authenticated = true;
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
}

async fn first_room_from_frame(
    bytes: &[u8],
    live: &CollabSession,
    conn_id: Uuid,
    hub: &Arc<CollabHub>,
    events: &mpsc::Sender<RoomClientEvent>,
    cancel: &watch::Sender<Option<ConnectionCancel>>,
    pre_auth_outbound: &mut PreAuthOutboundAllowance,
) -> Option<(crate::collab::room::RoomKey, String, bool, bool)> {
    let frame = crate::collab::wire::decode(bytes).ok()?;
    let WireFrame::Document {
        routing_key,
        room,
        message,
    } = frame
    else {
        return None;
    };
    let room_name = room?;
    if room_name.kind != CollabKind::Document {
        send_auth_denied(pre_auth_outbound, events, &routing_key, "unsupported kind");
        return None;
    }
    let key = (room_name.workspace_id, room_name.resource_id);
    let (auth_ok, read_only) = if matches!(message, DocumentMessage::Auth(AuthMessage::Token { .. })) {
        try_authenticate(
            hub,
            conn_id,
            live,
            &routing_key,
            bytes,
            events,
            cancel,
            pre_auth_outbound,
        )
        .await
    } else {
        (false, false)
    };
    Some((key, routing_key, auth_ok, read_only))
}

async fn try_authenticate(
    hub: &Arc<CollabHub>,
    conn_id: Uuid,
    live: &CollabSession,
    routing_key: &str,
    bytes: &[u8],
    events: &mpsc::Sender<RoomClientEvent>,
    cancel: &watch::Sender<Option<ConnectionCancel>>,
    pre_auth_outbound: &mut PreAuthOutboundAllowance,
) -> (bool, bool) {
    let frame = crate::collab::wire::decode(bytes).ok();
    let token = match frame {
        Some(WireFrame::Document {
            message: DocumentMessage::Auth(AuthMessage::Token { token, .. }),
            ..
        }) => token,
        _ => return (false, false),
    };
    let client_id = parse_client_id(&token);
    if client_id.is_none() {
        send_auth_denied(pre_auth_outbound, events, routing_key, "unauthorized");
        return (false, false);
    }
    let room = CollabRoomName::parse(routing_key.split('\0').next().unwrap_or(routing_key));
    let room = match room {
        Some(r) if r.kind == CollabKind::Document => r,
        _ => {
            send_auth_denied(pre_auth_outbound, events, routing_key, "not found");
            return (false, false);
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
            send_auth_denied(pre_auth_outbound, events, routing_key, "not found");
            return (false, false);
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
        Ok(()) => {
            let scope = if read_only { "readonly" } else { "read-write" };
            send_auth_ok(pre_auth_outbound, events, routing_key, scope);
            (true, read_only)
        }
        Err(JoinError::AdmissionDenied) => {
            send_auth_denied(pre_auth_outbound, events, routing_key, "not found");
            (false, false)
        }
        Err(JoinError::UnsupportedKind) => {
            send_auth_denied(pre_auth_outbound, events, routing_key, "unsupported kind");
            (false, false)
        }
        Err(_) => {
            send_auth_denied(pre_auth_outbound, events, routing_key, "unauthorized");
            (false, false)
        }
    }
}

fn send_auth_ok(
    pre_auth: &mut PreAuthOutboundAllowance,
    events: &mpsc::Sender<RoomClientEvent>,
    routing_key: &str,
    scope: &str,
) {
    let frame = crate::collab::wire::encode(&WireFrame::Document {
        routing_key: routing_key.to_string(),
        room: None,
        message: DocumentMessage::Auth(AuthMessage::Authenticated {
            scope: scope.into(),
        }),
    })
    .unwrap_or_default();
    let _ = pre_auth.try_send(events, frame);
}

fn send_auth_denied(
    pre_auth: &mut PreAuthOutboundAllowance,
    events: &mpsc::Sender<RoomClientEvent>,
    routing_key: &str,
    reason: &str,
) {
    let frame = crate::collab::wire::encode(&WireFrame::Document {
        routing_key: routing_key.to_string(),
        room: None,
        message: DocumentMessage::Auth(AuthMessage::PermissionDenied {
            reason: reason.into(),
        }),
    })
    .unwrap_or_default();
    let _ = pre_auth.try_send(events, frame);
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
