use std::path::Path;
use std::time::Duration;

use collab_engine::b64;
use collab_engine::limits::{Limits, MAX_OUTPUT_BYTES};
use collab_engine::outcome::{EngineStatus, LimitKind};
use collab_engine::process::{ChildSlotKind, EngineSession, SpawnPhaseTimings, SpawnRequest};
use collab_engine::protocol::Request;
use std::time::Instant;

const ADMISSION_TIMEOUT_MS: u64 = 4_000;
const VALIDATOR_SLOT_WAIT: Duration = Duration::from_millis(ADMISSION_TIMEOUT_MS);

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
    /// Validator child pool saturated after bounded wait; retryable capacity pressure.
    CapacityPressure,
    EngineUnavailable,
}

fn admission_resource_limit(outcome: LimitKind) -> BundleValidation {
    match outcome {
        LimitKind::Input
        | LimitKind::Output
        | LimitKind::Frame
        | LimitKind::Memory
        | LimitKind::Stack => BundleValidation::Rejected,
        LimitKind::Time | LimitKind::Ops => BundleValidation::EngineUnavailable,
    }
}

pub(crate) fn classify_admission_load(outcome: &EngineStatus) -> Result<(), BundleValidation> {
    match outcome {
        EngineStatus::Ok { applied: true, .. } => Ok(()),
        EngineStatus::Ok { applied: false, .. } => Err(BundleValidation::Rejected),
        EngineStatus::Malformed { .. } | EngineStatus::Unsupported { .. } => {
            Err(BundleValidation::Rejected)
        }
        EngineStatus::ResourceLimit { kind, .. } => Err(admission_resource_limit(*kind)),
        EngineStatus::WorkerFailure { .. } => Err(BundleValidation::EngineUnavailable),
    }
}

pub(crate) fn classify_admission_snapshot(
    outcome: EngineStatus,
    decoded: Result<Vec<u8>, String>,
) -> BundleValidation {
    match outcome {
        EngineStatus::Ok {
            update_b64: Some(_),
            ..
        } => match decoded {
            Ok(bytes) if bytes.len() as u64 <= MAX_OUTPUT_BYTES => BundleValidation::Ok,
            Ok(_) => BundleValidation::Rejected,
            Err(_) => BundleValidation::EngineUnavailable,
        },
        EngineStatus::Ok {
            update_b64: None, ..
        } => BundleValidation::EngineUnavailable,
        other => classify_admission_load(&other)
            .err()
            .unwrap_or(BundleValidation::EngineUnavailable),
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ValidateStageTimings {
    pub load_us: u64,
}

fn spawn_validator(
    engine_bin: &Path,
    limits: Limits,
) -> Result<(EngineSession, SpawnPhaseTimings), BundleValidation> {
    match EngineSession::spawn_with_timings(SpawnRequest {
        engine_bin: engine_bin.to_path_buf(),
        limits,
        slot_kind: ChildSlotKind::Validator,
        slot_wait: Some(VALIDATOR_SLOT_WAIT),
        test_hang_ms: None,
        test_exit_after_read: None,
        test_close_stdout_hang_ms: None,
        test_exit_after_write: None,
    }) {
        Ok(pair) => Ok(pair),
        Err(report)
            if matches!(
                report.outcome,
                EngineStatus::ResourceLimit {
                    kind: LimitKind::Ops,
                    ..
                }
            ) =>
        {
            Err(BundleValidation::CapacityPressure)
        }
        Err(_) => Err(BundleValidation::EngineUnavailable),
    }
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
    validate_recovery_bundle_blocking_with_timings(
        engine_bin,
        product_limits,
        snapshot,
        committed_tail,
        candidate,
    )
    .0
}

pub fn validate_recovery_bundle_blocking_with_timings(
    engine_bin: &Path,
    product_limits: Limits,
    snapshot: &[u8],
    committed_tail: &[Vec<u8>],
    candidate: &[u8],
) -> (BundleValidation, ValidateStageTimings) {
    if candidate.is_empty() {
        return (BundleValidation::Rejected, ValidateStageTimings::default());
    }
    let limits = admission_limits(product_limits);
    let (mut session, _spawn_timings) = match spawn_validator(engine_bin, limits) {
        Ok(pair) => pair,
        Err(outcome) => return (outcome, ValidateStageTimings::default()),
    };
    let mut tail = committed_tail.to_vec();
    tail.push(candidate.to_vec());
    let load_started = Instant::now();
    let load = session.call(&Request::Load {
        snapshot_b64: Some(snapshot.to_vec()),
        tail_b64: tail,
        encoding: 1,
    });
    let load_us = load_started.elapsed().as_micros() as u64;
    if let Err(outcome) = classify_admission_load(&load.outcome) {
        session.kill_and_reap();
        return (outcome, ValidateStageTimings { load_us });
    }
    let snapshot_started = Instant::now();
    let snap = session.call(&Request::Snapshot);
    let _snapshot_us = snapshot_started.elapsed().as_micros() as u64;
    session.kill_and_reap();
    let outcome = match &snap.outcome {
        EngineStatus::Ok {
            update_b64: Some(bytes_b64),
            ..
        } => classify_admission_snapshot(snap.outcome.clone(), b64::decode(bytes_b64)),
        other => classify_admission_snapshot(other.clone(), Ok(Vec::new())),
    };
    (outcome, ValidateStageTimings { load_us })
}

pub async fn validate_recovery_bundle(
    engine_bin: std::path::PathBuf,
    product_limits: Limits,
    snapshot: Vec<u8>,
    committed_tail: Vec<Vec<u8>>,
    candidate: Vec<u8>,
) -> BundleValidation {
    validate_recovery_bundle_with_timings(
        engine_bin,
        product_limits,
        snapshot,
        committed_tail,
        candidate,
    )
    .await
    .0
}

pub async fn validate_recovery_bundle_with_timings(
    engine_bin: std::path::PathBuf,
    product_limits: Limits,
    snapshot: Vec<u8>,
    committed_tail: Vec<Vec<u8>>,
    candidate: Vec<u8>,
) -> (BundleValidation, ValidateStageTimings) {
    tokio::task::spawn_blocking(move || {
        validate_recovery_bundle_blocking_with_timings(
            &engine_bin,
            product_limits,
            &snapshot,
            &committed_tail,
            &candidate,
        )
    })
    .await
    .unwrap_or((
        BundleValidation::EngineUnavailable,
        ValidateStageTimings::default(),
    ))
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
    let (mut session, _) = match spawn_validator(engine_bin, limits) {
        Ok(pair) => pair,
        Err(_) => return false,
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use collab_engine::limits::Limits;
    use collab_engine::outcome::{EngineStatus, LimitKind, UnsupportedReason, WorkerFailureReason};

    use super::{
        classify_admission_load, classify_admission_snapshot, validate_recovery_bundle_blocking,
        BundleValidation,
    };

    #[test]
    fn classify_load_malformed_and_unsupported_are_rejected() {
        assert_eq!(
            classify_admission_load(&EngineStatus::Malformed {
                detail: "bad".into()
            }),
            Err(BundleValidation::Rejected)
        );
        assert_eq!(
            classify_admission_load(&EngineStatus::Unsupported {
                reason: UnsupportedReason::UnknownOp,
                detail: "bad".into()
            }),
            Err(BundleValidation::Rejected)
        );
    }

    #[test]
    fn classify_load_all_limit_kinds() {
        let rejected = [
            LimitKind::Input,
            LimitKind::Output,
            LimitKind::Frame,
            LimitKind::Memory,
            LimitKind::Stack,
        ];
        for kind in rejected {
            assert_eq!(
                classify_admission_load(&EngineStatus::ResourceLimit {
                    kind,
                    detail: "limit".into()
                }),
                Err(BundleValidation::Rejected),
                "{kind:?} must be policy rejection"
            );
        }
        let unavailable = [LimitKind::Time, LimitKind::Ops];
        for kind in unavailable {
            assert_eq!(
                classify_admission_load(&EngineStatus::ResourceLimit {
                    kind,
                    detail: "limit".into()
                }),
                Err(BundleValidation::EngineUnavailable),
                "{kind:?} must stay operational"
            );
        }
    }

    #[test]
    fn classify_load_worker_failure_is_unavailable() {
        assert_eq!(
            classify_admission_load(&EngineStatus::WorkerFailure {
                reason: WorkerFailureReason::ChildCrash,
                detail: "boom".into()
            }),
            Err(BundleValidation::EngineUnavailable)
        );
    }

    #[test]
    fn classify_snapshot_invalid_helper_output_is_unavailable() {
        assert_eq!(
            classify_admission_snapshot(
                EngineStatus::Ok {
                    applied: false,
                    pending: false,
                    durable: false,
                    skip_gc: true,
                    offset_kind: "utf16".into(),
                    encoding: 1,
                    fragment: "prosemirror".into(),
                    update_b64: None,
                    state_vector_b64: None,
                    xml_string: None,
                    xml_len: None,
                    content_json: None,
                    yrs: None,
                },
                Ok(Vec::new())
            ),
            BundleValidation::EngineUnavailable
        );
        assert_eq!(
            classify_admission_snapshot(
                EngineStatus::Ok {
                    applied: false,
                    pending: false,
                    durable: false,
                    skip_gc: true,
                    offset_kind: "utf16".into(),
                    encoding: 1,
                    fragment: "prosemirror".into(),
                    update_b64: Some("!!!".into()),
                    state_vector_b64: None,
                    xml_string: None,
                    xml_len: None,
                    content_json: None,
                    yrs: None,
                },
                Err("invalid base64".into()),
            ),
            BundleValidation::EngineUnavailable
        );
    }

    #[test]
    fn validate_recovery_bundle_missing_engine_is_unavailable() {
        let outcome = validate_recovery_bundle_blocking(
            Path::new("/nonexistent/fvoci-collab-engine-for-tests"),
            Limits::for_tests(),
            &[1, 2, 3],
            &[],
            &[9, 9, 9],
        );
        assert_eq!(outcome, BundleValidation::EngineUnavailable);
    }

    #[test]
    fn validate_recovery_bundle_empty_candidate_is_rejected() {
        let outcome = validate_recovery_bundle_blocking(
            Path::new("/tmp/unused-engine-bin"),
            Limits::for_tests(),
            &[1, 2, 3],
            &[],
            &[],
        );
        assert_eq!(outcome, BundleValidation::Rejected);
    }

    // Native admission is also exercised by collab_projection in the DB gate.
    // Keep this out of the dependency-free gate instead of silently succeeding
    // when the helper has not been built.
    #[cfg(feature = "db-tests")]
    #[test]
    fn validate_recovery_bundle_huge_varint_memory_is_rejected() {
        let engine_bin = crate::collab::config::collab_engine_path_for_tests()
            .expect("native admission test requires FVOCI_COLLAB_ENGINE or a built local helper");
        let snapshot = std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("crates/collab-engine/fixtures/delete_only_base.v1"),
        )
        .expect("fixture");
        let candidate = vec![0xff, 0xff, 0xff, 0xff, 0x0f];
        let outcome = validate_recovery_bundle_blocking(
            &engine_bin,
            Limits::for_tests(),
            &snapshot,
            &[],
            &candidate,
        );
        assert_eq!(
            outcome,
            BundleValidation::Rejected,
            "native Memory limit from crafted varint must be policy rejection"
        );
    }
}
