use std::env;
use std::path::PathBuf;

use collab_engine::limits::Limits;
use sqlx::PgPool;

/// Default hub room slots. Fits stock PostgreSQL (`max_connections` 100) with the derived app pool
/// and reserve; the verified 64-room capacity probe sets `FVOCI_COLLAB_MAX_ROOMS=64` and needs a
/// higher `max_connections` (compose uses 150).
pub const DEFAULT_MAX_ROOMS: usize = 30;
pub const MAX_MAX_ROOMS: usize = 512;
/// Default `connect_app` pool size when collab is disabled.
pub const APP_POOL_MAX_CONNECTIONS: u32 = 10;
/// Headroom for migrations, admin tooling, and non-app sessions on the same instance.
pub const PG_CONNECTION_RESERVE: u32 = 10;

/// App pool connections for concurrent per-room auth/append transactions at steady state.
pub fn derive_app_pool_max_connections(max_rooms: usize) -> u32 {
    max_rooms.clamp(16, MAX_MAX_ROOMS) as u32
}
/// Default aggregate helper RSS budget (2 GiB). Tune with `FVOCI_COLLAB_MEMORY_BUDGET`.
pub const DEFAULT_MEMORY_BUDGET_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Primary helper headroom for offline revision capture while all room slots are live.
pub const OFFLINE_REVISION_PRIMARY_HEADROOM: usize = 4;

/// Process-wide primary helper cap: live rooms plus offline revision capture headroom.
pub fn derive_primary_child_concurrency(max_rooms: usize) -> usize {
    max_rooms.saturating_add(OFFLINE_REVISION_PRIMARY_HEADROOM)
}

/// Default `FVOCI_COLLAB_MAX_CHILDREN` when unset: primary cap plus validator pool headroom.
pub fn derive_max_child_concurrency(max_rooms: usize) -> usize {
    derive_primary_child_concurrency(max_rooms) + derive_validator_child_concurrency(max_rooms)
}

/// Ephemeral validator pool size (compaction and fallback admission paths).
pub fn derive_validator_child_concurrency(max_rooms: usize) -> usize {
    (max_rooms / 2).max(4)
}

/// PostgreSQL connections required for collab at steady state: room guards plus app pool and reserve.
pub fn collab_pg_connections_required(max_rooms: usize) -> u64 {
    max_rooms as u64
        + u64::from(derive_app_pool_max_connections(max_rooms))
        + u64::from(PG_CONNECTION_RESERVE)
}

pub fn collab_fits_postgres_max_connections(max_rooms: usize, pg_max_connections: i64) -> bool {
    collab_pg_connections_required(max_rooms) <= pg_max_connections as u64
}

pub async fn assert_collab_fits_postgres(pool: &PgPool, max_rooms: usize) -> Result<(), String> {
    let pg_max: i64 = sqlx::query_scalar("SELECT current_setting('max_connections')::bigint")
        .fetch_one(pool)
        .await
        .map_err(|err| format!("read PostgreSQL max_connections: {err}"))?;
    if collab_fits_postgres_max_connections(max_rooms, pg_max) {
        Ok(())
    } else {
        Err(format!(
            "FVOCI_COLLAB_MAX_ROOMS={max_rooms} needs at least {} PostgreSQL max_connections (rooms + app pool {} + reserve {}); server has {}",
            collab_pg_connections_required(max_rooms),
            derive_app_pool_max_connections(max_rooms),
            PG_CONNECTION_RESERVE,
            pg_max
        ))
    }
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
        let primary_cap = derive_primary_child_concurrency(self.max_rooms);
        let validator_cap = derive_validator_child_concurrency(self.max_rooms)
            .min(self.max_child_concurrency.saturating_sub(primary_cap));
        collab_engine::process::set_max_child_concurrency(primary_cap);
        collab_engine::process::set_max_validator_child_concurrency(validator_cap.max(4));
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
        assert_eq!(derive_validator_child_concurrency(4), 4);
        assert_eq!(derive_primary_child_concurrency(4), 8);
        assert_eq!(derive_max_child_concurrency(4), 12);
        assert_eq!(derive_max_child_concurrency(16), 28);
        assert_eq!(derive_max_child_concurrency(64), 100);
    }

    #[test]
    fn derive_app_pool_tracks_max_rooms() {
        assert_eq!(derive_app_pool_max_connections(4), 16);
        assert_eq!(derive_app_pool_max_connections(64), 64);
    }

    #[test]
    fn default_max_rooms_fits_stock_postgres() {
        assert!(collab_fits_postgres_max_connections(DEFAULT_MAX_ROOMS, 100));
        assert!(!collab_fits_postgres_max_connections(64, 100));
        assert!(collab_fits_postgres_max_connections(64, 150));
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
