use std::path::PathBuf;

use collab_engine::limits::Limits;
use collab_engine::outcome::EngineStatus;
use collab_engine::process::{EngineSession, SpawnRequest};
use collab_engine::protocol::Request;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::collab::room::{CapturedRevision, RevisionCaptureError};

/// Validated forward edit of the existing native history, never a JSON reseed.
pub(crate) struct PreparedOffBody {
    pub complete_v1: Vec<u8>,
    pub start_complete_v1: Option<Vec<u8>>,
    pub captured: CapturedRevision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OffBodyPrepareError {
    Invalid,
    Unavailable,
    Cancelled,
}

pub(crate) fn prepare_off_body(
    engine_bin: PathBuf,
    limits: Limits,
    snapshot: Vec<u8>,
    tail: Vec<Vec<u8>>,
    update: Vec<u8>,
    compact_start: bool,
    cancelled: &AtomicBool,
) -> Result<PreparedOffBody, OffBodyPrepareError> {
    let check = || {
        if cancelled.load(Ordering::Acquire) {
            Err(OffBodyPrepareError::Cancelled)
        } else {
            Ok(())
        }
    };
    check()?;
    let mut session = EngineSession::spawn(SpawnRequest {
        engine_bin,
        limits,
        slot_kind: collab_engine::process::ChildSlotKind::Primary,
        slot_wait: None,
        test_hang_ms: None,
        test_exit_after_read: None,
        test_close_stdout_hang_ms: None,
        test_exit_after_write: None,
    })
    .map_err(|_| OffBodyPrepareError::Unavailable)?;
    if !matches!(
        session
            .call(&Request::Load {
                snapshot_b64: Some(snapshot),
                tail_b64: tail,
                encoding: 1,
            })
            .outcome,
        EngineStatus::Ok {
            applied: true,
            pending: false,
            ..
        }
    ) {
        return Err(OffBodyPrepareError::Unavailable);
    }
    check()?;
    // At the accepted tail/load limit, compact only the original history before
    // appending. A post-edit snapshot must never be published at the old tail.
    let start_complete_v1 = if compact_start {
        match session.call(&Request::Snapshot).outcome {
            EngineStatus::Ok {
                update_b64: Some(value),
                pending: false,
                ..
            } => Some(
                collab_engine::b64::decode(&value).map_err(|_| OffBodyPrepareError::Unavailable)?,
            ),
            _ => return Err(OffBodyPrepareError::Unavailable),
        }
    } else {
        None
    };
    check()?;
    match session
        .call(&Request::Apply {
            update_b64: update,
            encoding: 1,
        })
        .outcome
    {
        EngineStatus::Ok {
            applied: true,
            pending: false,
            ..
        } => {}
        EngineStatus::Malformed { .. }
        | EngineStatus::Unsupported { .. }
        | EngineStatus::Ok { .. } => return Err(OffBodyPrepareError::Invalid),
        _ => return Err(OffBodyPrepareError::Unavailable),
    }
    check()?;
    let mut bytes = |request| -> Result<Vec<u8>, OffBodyPrepareError> {
        check()?;
        match session.call(&request).outcome {
            EngineStatus::Ok {
                update_b64: Some(value),
                pending: false,
                ..
            } => collab_engine::b64::decode(&value).map_err(|_| OffBodyPrepareError::Unavailable),
            _ => Err(OffBodyPrepareError::Unavailable),
        }
    };
    let complete_v1 = bytes(Request::Snapshot)?;
    let y_snapshot = bytes(Request::RevisionSnapshot)?;
    check()?;
    let content_json = match session.call(&Request::Project { encoding: 1 }).outcome {
        EngineStatus::Ok {
            content_json: Some(value),
            pending: false,
            ..
        } => value,
        _ => return Err(OffBodyPrepareError::Unavailable),
    };
    check()?;
    crate::collab::derived_body::prepare_derived_body(content_json.clone())
        .map_err(|_| OffBodyPrepareError::Invalid)?;
    Ok(PreparedOffBody {
        complete_v1,
        start_complete_v1,
        captured: CapturedRevision {
            y_snapshot,
            content_json,
        },
    })
}

pub fn capture_revision_offline(
    engine_bin: PathBuf,
    limits: Limits,
    snapshot: Vec<u8>,
    tail: Vec<Vec<u8>>,
) -> Result<CapturedRevision, RevisionCaptureError> {
    let mut session = EngineSession::spawn(SpawnRequest {
        engine_bin,
        limits,
        slot_kind: collab_engine::process::ChildSlotKind::Primary,
        slot_wait: None,
        test_hang_ms: None,
        test_exit_after_read: None,
        test_close_stdout_hang_ms: None,
        test_exit_after_write: None,
    })
    .map_err(|_| RevisionCaptureError::Unavailable)?;
    let load = session.call(&Request::Load {
        snapshot_b64: Some(snapshot),
        tail_b64: tail,
        encoding: 1,
    });
    if !load.outcome.is_applied_ok() {
        return Err(RevisionCaptureError::Unavailable);
    }
    let snap = match session.call(&Request::RevisionSnapshot).outcome {
        EngineStatus::Ok {
            update_b64: Some(bytes),
            ..
        } => collab_engine::b64::decode(&bytes).map_err(|_| RevisionCaptureError::Unavailable)?,
        _ => return Err(RevisionCaptureError::Unavailable),
    };
    let content_json = match session.call(&Request::Project { encoding: 1 }).outcome {
        EngineStatus::Ok {
            content_json: Some(json),
            ..
        } => json,
        _ => return Err(RevisionCaptureError::Unavailable),
    };
    Ok(CapturedRevision {
        y_snapshot: snap,
        content_json,
    })
}

/// Semantic snapshot equality in an isolated child (source `Y.equalSnapshots`).
pub fn revision_snapshots_equal_offline(
    engine_bin: PathBuf,
    limits: Limits,
    left: &[u8],
    right: &[u8],
) -> Result<bool, RevisionCaptureError> {
    if left == right {
        return Ok(true);
    }
    let mut session = EngineSession::spawn(SpawnRequest {
        engine_bin,
        limits,
        slot_kind: collab_engine::process::ChildSlotKind::Primary,
        slot_wait: None,
        test_hang_ms: None,
        test_exit_after_read: None,
        test_close_stdout_hang_ms: None,
        test_exit_after_write: None,
    })
    .map_err(|_| RevisionCaptureError::Unavailable)?;
    match session
        .call(&Request::RevisionSnapshotsEqual {
            left_b64: left.to_vec(),
            right_b64: right.to_vec(),
        })
        .outcome
    {
        EngineStatus::Ok {
            update_b64: Some(bytes),
            ..
        } => {
            let decoded = collab_engine::b64::decode(&bytes)
                .map_err(|_| RevisionCaptureError::Unavailable)?;
            Ok(decoded.first() == Some(&1))
        }
        EngineStatus::Malformed { .. } | EngineStatus::ResourceLimit { .. } => {
            Err(RevisionCaptureError::Unavailable)
        }
        _ => Err(RevisionCaptureError::Unavailable),
    }
}

pub fn prepare_revision_text(content_json: &Value) -> Result<String, RevisionCaptureError> {
    crate::collab::derived_body::prepare_derived_body(content_json.clone())
        .map(|body| body.text().to_string())
        .map_err(|_| RevisionCaptureError::Unavailable)
}

/// Tiptap JSON of persisted collab state (snapshot + tail) in an isolated
/// helper, without a room (source `contentJsonFromPersistedYjs`).
pub fn project_persisted_offline(
    engine_bin: PathBuf,
    limits: Limits,
    snapshot: Vec<u8>,
    tail: Vec<Vec<u8>>,
) -> Result<Value, RevisionCaptureError> {
    let mut session = EngineSession::spawn(SpawnRequest {
        engine_bin,
        limits,
        slot_kind: collab_engine::process::ChildSlotKind::Primary,
        slot_wait: None,
        test_hang_ms: None,
        test_exit_after_read: None,
        test_close_stdout_hang_ms: None,
        test_exit_after_write: None,
    })
    .map_err(|_| RevisionCaptureError::Unavailable)?;
    let load = session.call(&Request::Load {
        snapshot_b64: Some(snapshot),
        tail_b64: tail,
        encoding: 1,
    });
    if !load.outcome.is_applied_ok() {
        return Err(RevisionCaptureError::Unavailable);
    }
    match session.call(&Request::Project { encoding: 1 }).outcome {
        EngineStatus::Ok {
            content_json: Some(json),
            ..
        } => Ok(json),
        _ => Err(RevisionCaptureError::Unavailable),
    }
}
