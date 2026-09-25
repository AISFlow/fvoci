use std::path::PathBuf;

use collab_engine::limits::Limits;
use collab_engine::outcome::EngineStatus;
use collab_engine::process::{EngineSession, SpawnRequest};
use collab_engine::protocol::Request;
use serde_json::Value;

use crate::collab::room::{CapturedRevision, RevisionCaptureError};

pub fn capture_revision_offline(
    engine_bin: PathBuf,
    limits: Limits,
    snapshot: Vec<u8>,
    tail: Vec<Vec<u8>>,
) -> Result<CapturedRevision, RevisionCaptureError> {
    let mut session = EngineSession::spawn(SpawnRequest {
        engine_bin,
        limits,
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

pub fn prepare_revision_text(content_json: &Value) -> Result<String, RevisionCaptureError> {
    crate::collab::derived_body::prepare_derived_body(content_json.clone())
        .map(|body| body.text().to_string())
        .map_err(|_| RevisionCaptureError::Unavailable)
}
