use std::path::Path;

use collab_engine::b64;
use collab_engine::limits::{Limits, MAX_OUTPUT_BYTES};
use collab_engine::outcome::EngineStatus;
use collab_engine::process::{EngineSession, SpawnRequest};
use collab_engine::protocol::Request;

const ADMISSION_TIMEOUT_MS: u64 = 4_000;

/// Stricter wall clock for pre-commit admission; product recovery limits stay unchanged.
pub fn admission_limits(limits: Limits) -> Limits {
    Limits {
        timeout_ms: ADMISSION_TIMEOUT_MS.min(limits.timeout_ms),
        ..limits
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundleValidation {
    Ok,
    Rejected,
    EngineUnavailable,
}

fn spawn_validator(engine_bin: &Path, limits: Limits) -> Option<EngineSession> {
    EngineSession::spawn(SpawnRequest {
        engine_bin: engine_bin.to_path_buf(),
        limits,
        test_hang_ms: None,
        test_exit_after_read: None,
        test_close_stdout_hang_ms: None,
        test_exit_after_write: None,
    })
    .ok()
}

/// Validate the exact durable recovery bundle: committed snapshot + ordered tail + candidate.
/// After a successful Load, Snapshot must fit the output cap (merged state may exceed per-blob).
pub fn validate_recovery_bundle_blocking(
    engine_bin: &Path,
    product_limits: Limits,
    snapshot: &[u8],
    committed_tail: &[Vec<u8>],
    candidate: &[u8],
) -> BundleValidation {
    if candidate.is_empty() {
        return BundleValidation::Rejected;
    }
    let limits = admission_limits(product_limits);
    let mut session = match spawn_validator(engine_bin, limits) {
        Some(s) => s,
        None => return BundleValidation::EngineUnavailable,
    };
    let mut tail = committed_tail.to_vec();
    tail.push(candidate.to_vec());
    let load = session.call(&Request::Load {
        snapshot_b64: Some(snapshot.to_vec()),
        tail_b64: tail,
        encoding: 1,
    });
    if !matches!(load.outcome, EngineStatus::Ok { applied: true, .. }) {
        session.kill_and_reap();
        return BundleValidation::Rejected;
    }
    let snap = session.call(&Request::Snapshot);
    session.kill_and_reap();
    match snap.outcome {
        EngineStatus::Ok {
            update_b64: Some(bytes_b64),
            ..
        } => match b64::decode(&bytes_b64) {
            Ok(bytes) if bytes.len() as u64 <= MAX_OUTPUT_BYTES => BundleValidation::Ok,
            _ => BundleValidation::Rejected,
        },
        _ => BundleValidation::Rejected,
    }
}

pub async fn validate_recovery_bundle(
    engine_bin: std::path::PathBuf,
    product_limits: Limits,
    snapshot: Vec<u8>,
    committed_tail: Vec<Vec<u8>>,
    candidate: Vec<u8>,
) -> BundleValidation {
    tokio::task::spawn_blocking(move || {
        validate_recovery_bundle_blocking(
            &engine_bin,
            product_limits,
            &snapshot,
            &committed_tail,
            &candidate,
        )
    })
    .await
    .unwrap_or(BundleValidation::EngineUnavailable)
}

/// Compaction candidate: load snapshot only in a fresh child under admission limits.
/// Uses an explicit snapshot-only boundary (no synthetic tail row).
pub fn validate_snapshot_only_blocking(
    engine_bin: &Path,
    product_limits: Limits,
    snapshot: &[u8],
) -> bool {
    if snapshot.is_empty() {
        return false;
    }
    let limits = admission_limits(product_limits);
    let mut session = match spawn_validator(engine_bin, limits) {
        Some(s) => s,
        None => return false,
    };
    let load = session.call(&Request::Load {
        snapshot_b64: Some(snapshot.to_vec()),
        tail_b64: Vec::new(),
        encoding: 1,
    });
    if !matches!(load.outcome, EngineStatus::Ok { applied: true, .. }) {
        session.kill_and_reap();
        return false;
    }
    let snap = session.call(&Request::Snapshot);
    session.kill_and_reap();
    match snap.outcome {
        EngineStatus::Ok {
            update_b64: Some(bytes_b64),
            ..
        } => matches!(
            b64::decode(&bytes_b64),
            Ok(bytes) if bytes.len() as u64 <= MAX_OUTPUT_BYTES
        ),
        _ => false,
    }
}

pub async fn validate_snapshot_only(
    engine_bin: std::path::PathBuf,
    product_limits: Limits,
    snapshot: Vec<u8>,
) -> bool {
    tokio::task::spawn_blocking(move || {
        validate_snapshot_only_blocking(&engine_bin, product_limits, &snapshot)
    })
    .await
    .unwrap_or(false)
}
