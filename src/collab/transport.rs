use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::auth::token::hash_token;
use crate::collab::hub::CollabHub;
use crate::collab::origin::{validate_collab_origin, CollabOriginError};
use crate::collab::room::{
    parse_client_id, AuthenticatedConnection, CollabSession, JoinError, RoomClientEvent, RoomJoin,
};
use crate::collab::wire::{AuthMessage, CollabKind, CollabRoomName, DocumentMessage, WireFrame};
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
    ws.on_upgrade(move |socket| handle_socket(socket, hub, session, peer))
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

async fn handle_socket(
    socket: WebSocket,
    hub: Arc<CollabHub>,
    live: CollabSession,
    _peer: SocketAddr,
) {
    let (mut sender, mut receiver) = socket.split();
    let conn_id = Uuid::now_v7();
    let (events_tx, mut events_rx) = mpsc::channel(64);
    let mut joined_room: Option<(crate::collab::room::RoomKey, String, bool)> = None;

    loop {
        tokio::select! {
            outbound = events_rx.recv() => {
                match outbound {
                    Some(RoomClientEvent::Outbound(bytes)) => {
                        if sender.send(Message::Binary(bytes.into())).await.is_err() {
                            break;
                        }
                    }
                    Some(RoomClientEvent::Close { code, reason }) => {
                        let _ = sender.send(Message::Close(Some(axum::extract::ws::CloseFrame {
                            code,
                            reason: reason.into(),
                        }))).await;
                        break;
                    }
                    None => break,
                }
            }
            inbound = receiver.next() => {
                match inbound {
                    Some(Ok(Message::Binary(bytes))) => {
                        if let Some((key, routing_key, authenticated)) = &joined_room {
                            if *authenticated {
                                hub.send_frame(*key, conn_id, bytes.to_vec()).await;
                            } else {
                                if try_authenticate(
                                    &hub,
                                    conn_id,
                                    &live,
                                    &routing_key,
                                    &bytes,
                                    &events_tx,
                                ).await {
                                    joined_room = Some((key.clone(), routing_key.clone(), true));
                                }
                            }
                        } else if let Some((key, routing, auth_ok)) = first_room_from_frame(&bytes, &live, conn_id, &hub, &events_tx).await {
                            joined_room = Some((key, routing, auth_ok));
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        let _ = sender.send(Message::Pong(payload)).await;
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }
    if let Some((key, _, _)) = joined_room {
        hub.leave_room(key, conn_id).await;
    }
}

async fn first_room_from_frame(
    bytes: &[u8],
    live: &CollabSession,
    conn_id: Uuid,
    hub: &Arc<CollabHub>,
    events: &mpsc::Sender<RoomClientEvent>,
) -> Option<(crate::collab::room::RoomKey, String, bool)> {
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
        send_auth_denied(events, &routing_key, "unsupported kind").await;
        return None;
    }
    let key = (room_name.workspace_id, room_name.resource_id);
    let auth_ok = if matches!(message, DocumentMessage::Auth(AuthMessage::Token { .. })) {
        try_authenticate(hub, conn_id, live, &routing_key, bytes, events).await
    } else {
        false
    };
    Some((key, routing_key, auth_ok))
}

async fn try_authenticate(
    hub: &Arc<CollabHub>,
    conn_id: Uuid,
    live: &CollabSession,
    routing_key: &str,
    bytes: &[u8],
    events: &mpsc::Sender<RoomClientEvent>,
) -> bool {
    let frame = crate::collab::wire::decode(bytes).ok();
    let token = match frame {
        Some(WireFrame::Document {
            message: DocumentMessage::Auth(AuthMessage::Token { token, .. }),
            ..
        }) => token,
        _ => return false,
    };
    let client_id = parse_client_id(&token);
    if client_id.is_none() {
        send_auth_denied(events, routing_key, "unauthorized").await;
        return false;
    }
    let room = CollabRoomName::parse(routing_key.split('\0').next().unwrap_or(routing_key));
    let room = match room {
        Some(r) if r.kind == CollabKind::Document => r,
        _ => {
            send_auth_denied(events, routing_key, "not found").await;
            return false;
        }
    };
    let join = RoomJoin {
        conn: AuthenticatedConnection {
            conn_id,
            session: live.clone(), // CollabSession is Clone
            client_id: client_id.unwrap(),
            read_only: false,
            routing_key: routing_key.to_string(),
        },
        events: events.clone(),
    };
    match hub
        .join_room((room.workspace_id, room.resource_id), join)
        .await
    {
        Ok(()) => {
            let scope = "read-write";
            send_auth_ok(events, routing_key, scope).await;
            true
        }
        Err(JoinError::AdmissionDenied) => {
            send_auth_denied(events, routing_key, "not found").await;
            false
        }
        Err(JoinError::UnsupportedKind) => {
            send_auth_denied(events, routing_key, "unsupported kind").await;
            false
        }
        Err(_) => {
            send_auth_denied(events, routing_key, "unauthorized").await;
            false
        }
    }
}

async fn send_auth_ok(events: &mpsc::Sender<RoomClientEvent>, routing_key: &str, scope: &str) {
    let frame = crate::collab::wire::encode(&WireFrame::Document {
        routing_key: routing_key.to_string(),
        room: None,
        message: DocumentMessage::Auth(AuthMessage::Authenticated {
            scope: scope.into(),
        }),
    })
    .unwrap_or_default();
    let _ = events.send(RoomClientEvent::Outbound(frame)).await;
}

async fn send_auth_denied(events: &mpsc::Sender<RoomClientEvent>, routing_key: &str, reason: &str) {
    let frame = crate::collab::wire::encode(&WireFrame::Document {
        routing_key: routing_key.to_string(),
        room: None,
        message: DocumentMessage::Auth(AuthMessage::PermissionDenied {
            reason: reason.into(),
        }),
    })
    .unwrap_or_default();
    let _ = events.send(RoomClientEvent::Outbound(frame)).await;
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
