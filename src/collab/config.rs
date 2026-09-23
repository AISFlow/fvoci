use std::env;
use std::path::PathBuf;

use collab_engine::limits::Limits;

/// Product collab runtime configuration. Enabled only when `FVOCI_COLLAB_ENGINE`
/// points at a built `collab-engine` helper binary.
#[derive(Debug, Clone)]
pub struct CollabConfig {
    pub engine_bin: PathBuf,
    pub limits: Limits,
    pub max_rooms: usize,
    pub max_connections_per_room: usize,
    pub max_queued_room_ops: usize,
    pub max_pending_bytes_per_connection: usize,
    pub idle_evict_ms: u64,
    pub revoke_poll_ms: u64,
    pub client_id_ttl_ms: u64,
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
            .unwrap_or(4);
        let max_connections_per_room = env::var("FVOCI_COLLAB_MAX_CONNECTIONS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(32);
        let max_queued_room_ops = env::var("FVOCI_COLLAB_MAX_QUEUE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(256);
        let max_pending_bytes_per_connection = env::var("FVOCI_COLLAB_MAX_PENDING_BYTES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(4 * 1024 * 1024);
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
        Some(Self {
            engine_bin,
            limits: Limits::default(),
            max_rooms: max_rooms.clamp(1, 4),
            max_connections_per_room: max_connections_per_room.max(1),
            max_queued_room_ops: max_queued_room_ops.max(16),
            max_pending_bytes_per_connection: max_pending_bytes_per_connection.max(64 * 1024),
            idle_evict_ms: idle_evict_ms.max(1_000),
            revoke_poll_ms: revoke_poll_ms.max(500),
            client_id_ttl_ms: client_id_ttl_ms.max(5_000),
        })
    }

    pub fn is_available(&self) -> bool {
        self.engine_bin.is_file()
    }
}

pub fn collab_engine_path_for_tests() -> Option<PathBuf> {
    if let Ok(path) = env::var("FVOCI_COLLAB_ENGINE") {
        let p = PathBuf::from(path.trim());
        if p.is_file() {
            return Some(p);
        }
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
