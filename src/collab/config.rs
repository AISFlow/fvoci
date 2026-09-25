use std::env;
use std::path::PathBuf;

use collab_engine::limits::Limits;

pub const DEFAULT_MAX_ROOMS: usize = 64;
pub const MAX_MAX_ROOMS: usize = 512;
/// Default aggregate helper RSS budget (2 GiB). Tune with `FVOCI_COLLAB_MEMORY_BUDGET`.
pub const DEFAULT_MEMORY_BUDGET_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Headroom above [`DEFAULT_MAX_ROOMS`] for recycle, revision restore, and derived work.
pub fn derive_max_child_concurrency(max_rooms: usize) -> usize {
    let headroom = (max_rooms / 2).max(4);
    max_rooms + headroom
}

/// Product collab runtime configuration. Enabled only when `FVOCI_COLLAB_ENGINE`
/// points at a built `collab-engine` helper binary.
#[derive(Debug, Clone)]
pub struct CollabConfig {
    pub engine_bin: PathBuf,
    pub limits: Limits,
    pub max_rooms: usize,
    pub max_child_concurrency: usize,
    pub memory_budget_bytes: u64,
    pub max_collab_sockets: usize,
    pub max_collab_sockets_per_session: usize,
    pub max_connections_per_room: usize,
    pub max_queued_room_ops: usize,
    pub max_pending_bytes_per_connection: usize,
    pub max_outbound_frames_per_connection: usize,
    pub max_outbound_bytes_per_connection: usize,
    pub outbound_send_deadline_ms: u64,
    pub max_ws_frame_bytes: usize,
    pub max_ws_message_bytes: usize,
    pub auth_wait_ms: u64,
    pub max_pre_auth_outbound_frames: u32,
    pub max_inbound_messages_per_window: u32,
    pub inbound_message_window_ms: u64,
    pub idle_evict_ms: u64,
    pub revoke_poll_ms: u64,
    pub client_id_ttl_ms: u64,
    pub rpc_timeout_ms: u64,
}

impl CollabConfig {
    pub fn from_env() -> Option<Self> {
        let raw = env::var("FVOCI_COLLAB_ENGINE").ok()?;
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return None;
        }
        let engine_bin = PathBuf::from(trimmed);
        if !engine_bin.is_file() {
            return None;
        }
        let max_rooms = env::var("FVOCI_COLLAB_MAX_ROOMS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_MAX_ROOMS)
            .clamp(1, MAX_MAX_ROOMS);
        let max_child_concurrency = env::var("FVOCI_COLLAB_MAX_CHILDREN")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(|| derive_max_child_concurrency(max_rooms))
            .max(max_rooms);
        let memory_budget_bytes = env::var("FVOCI_COLLAB_MEMORY_BUDGET")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_MEMORY_BUDGET_BYTES)
            .max(collab_engine::limits::MIN_ROOM_MEMORY_RESERVATION_BYTES);
        let max_connections_per_room = env::var("FVOCI_COLLAB_MAX_CONNECTIONS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(32);
        let max_collab_sockets = env::var("FVOCI_COLLAB_MAX_SOCKETS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(max_rooms * max_connections_per_room);
        let max_collab_sockets_per_session = env::var("FVOCI_COLLAB_MAX_SOCKETS_PER_SESSION")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(4);
        let max_queued_room_ops = env::var("FVOCI_COLLAB_MAX_QUEUE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(256);
        let max_pending_bytes_per_connection = env::var("FVOCI_COLLAB_MAX_PENDING_BYTES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(4 * 1024 * 1024);
        let max_outbound_frames_per_connection = env::var("FVOCI_COLLAB_MAX_OUTBOUND_FRAMES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(64);
        let max_outbound_bytes_per_connection = env::var("FVOCI_COLLAB_MAX_OUTBOUND_BYTES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(4 * 1024 * 1024);
        let outbound_send_deadline_ms = env::var("FVOCI_COLLAB_OUTBOUND_DEADLINE_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(5_000);
        let wire_limits = crate::collab::wire::Limits::DEFAULT;
        let max_ws_frame_bytes = env::var("FVOCI_COLLAB_MAX_WS_FRAME_BYTES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(wire_limits.max_frame_bytes);
        let max_ws_message_bytes = env::var("FVOCI_COLLAB_MAX_WS_MESSAGE_BYTES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(wire_limits.max_frame_bytes);
        let auth_wait_ms = env::var("FVOCI_COLLAB_AUTH_WAIT_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(30_000);
        let max_pre_auth_outbound_frames = env::var("FVOCI_COLLAB_MAX_PRE_AUTH_OUTBOUND_FRAMES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(2);
        let max_inbound_messages_per_window = env::var("FVOCI_COLLAB_MAX_INBOUND_MSGS_PER_WINDOW")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(256);
        let inbound_message_window_ms = env::var("FVOCI_COLLAB_INBOUND_MSG_WINDOW_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1_000);
        let idle_evict_ms = env::var("FVOCI_COLLAB_IDLE_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(30_000);
        let revoke_poll_ms = env::var("FVOCI_COLLAB_REVOKE_POLL_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(5_000);
        let client_id_ttl_ms = env::var("FVOCI_COLLAB_CLIENT_ID_TTL_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(60_000);
        let rpc_timeout_ms = env::var("COLLAB_RPC_TIMEOUT_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(5_000);
        Some(Self {
            engine_bin,
            limits: Limits::default(),
            max_rooms,
            max_child_concurrency,
            memory_budget_bytes,
            max_collab_sockets: max_collab_sockets.max(1),
            max_collab_sockets_per_session: max_collab_sockets_per_session.clamp(1, 8),
            max_connections_per_room: max_connections_per_room.max(1),
            max_queued_room_ops: max_queued_room_ops.max(16),
            max_pending_bytes_per_connection: max_pending_bytes_per_connection.max(64 * 1024),
            max_outbound_frames_per_connection: max_outbound_frames_per_connection.clamp(8, 64),
            max_outbound_bytes_per_connection: max_outbound_bytes_per_connection.max(64 * 1024),
            outbound_send_deadline_ms: outbound_send_deadline_ms.max(500),
            max_ws_frame_bytes: max_ws_frame_bytes.max(4_096),
            max_ws_message_bytes: max_ws_message_bytes.max(4_096),
            auth_wait_ms: auth_wait_ms.max(1_000),
            max_pre_auth_outbound_frames: max_pre_auth_outbound_frames.clamp(1, 8),
            max_inbound_messages_per_window: max_inbound_messages_per_window.clamp(16, 4096),
            inbound_message_window_ms: inbound_message_window_ms.max(100),
            idle_evict_ms: idle_evict_ms.max(1_000),
            revoke_poll_ms: revoke_poll_ms.max(500),
            client_id_ttl_ms: client_id_ttl_ms.max(5_000),
            rpc_timeout_ms: rpc_timeout_ms.max(1),
        })
    }

    pub fn is_available(&self) -> bool {
        self.engine_bin.is_file()
    }

    /// Apply process-wide helper limits derived from this configuration.
    pub fn apply_runtime_limits(&self) {
        collab_engine::process::set_max_child_concurrency(self.max_child_concurrency);
    }
}

pub fn collab_engine_path_for_tests() -> Option<PathBuf> {
    if let Ok(path) = env::var("FVOCI_COLLAB_ENGINE") {
        let p = PathBuf::from(path.trim());
        // An explicit selection must not silently run a different helper.
        return p.is_file().then_some(p);
    }
    [
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/debug/collab-engine"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("crates/collab-engine/target/debug/collab-engine"),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
}

pub fn require_collab_engine_for_tests() -> PathBuf {
    collab_engine_path_for_tests().expect(
        "FVOCI_COLLAB_ENGINE or target/debug/collab-engine required for collab product tests",
    )
}

#[cfg(test)]
mod config_tests {
    use super::*;

    #[test]
    fn derive_child_concurrency_includes_headroom() {
        assert_eq!(derive_max_child_concurrency(4), 8);
        assert_eq!(derive_max_child_concurrency(16), 24);
        assert_eq!(derive_max_child_concurrency(64), 96);
    }

    #[test]
    fn max_rooms_env_clamps_to_ceiling() {
        let engine = require_collab_engine_for_tests();
        let key = "FVOCI_COLLAB_MAX_ROOMS";
        let prior = env::var(key).ok();
        unsafe { env::set_var(key, "9999") };
        unsafe { env::set_var("FVOCI_COLLAB_ENGINE", engine.to_string_lossy().as_ref()) };
        let cfg = CollabConfig::from_env().expect("collab config");
        assert_eq!(cfg.max_rooms, MAX_MAX_ROOMS);
        match prior {
            Some(value) => unsafe { env::set_var(key, value) },
            None => unsafe { env::remove_var(key) },
        }
    }
}
