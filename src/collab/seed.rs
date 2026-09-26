//! Tiptap JSON → Yjs updateV1 seed (source `tiptapJsonToYUpdate`) in the
//! isolated `collab-engine` child (`SeedFromTiptap`). Yrs stays out of the
//! server binary; each call is a one-shot child in its own
//! [`ChildSlotKind::Seed`] pool, so seed bursts wait (bounded) for each other
//! instead of taking the primary room/revision headroom.
//!
//! Callers pass JSON that already passed `prepare_derived_body` (Tiptap doc,
//! ≤ `DOCUMENT_MAX_BODY_BYTES`). The update only seeds a body write: live
//! documents still go through the room's `ReplaceFromUpdate`, new documents
//! through `append_collab_update`.

use std::path::PathBuf;
use std::time::Duration;

use collab_engine::limits::Limits;
use collab_engine::outcome::{EngineStatus, LimitKind};
use collab_engine::process::{ChildSlotKind, EngineSession, SpawnRequest};
use collab_engine::protocol::Request;
use serde_json::Value;

/// Bounded wait for a seed child slot before `Unavailable` (503).
pub const SEED_SLOT_WAIT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone)]
pub struct SeedEngine {
    engine_bin: PathBuf,
    limits: Limits,
}

#[derive(Debug, thiserror::Error)]
pub enum SeedError {
    /// Not a document the editor schema accepts (Node helper `invalid_input`).
    #[error("invalid tiptap document: {0}")]
    InvalidInput(String),
    #[error("tiptap document or seed update exceeds limits: {0}")]
    TooLarge(String),
    /// No seed child slot after [`SEED_SLOT_WAIT`] / engine could not start.
    #[error("collab engine unavailable")]
    Unavailable,
    #[error("seed failed: {0}")]
    Failed(String),
}

impl SeedEngine {
    pub fn new(engine_bin: PathBuf, limits: Limits) -> Self {
        Self { engine_bin, limits }
    }

    /// `FVOCI_COLLAB_ENGINE` (+ collab limits), for callers without a hub.
    pub fn from_env() -> Option<Self> {
        crate::collab::CollabConfig::from_env().map(|cfg| Self::new(cfg.engine_bin, cfg.limits))
    }

    pub fn from_hub(hub: &crate::collab::CollabHub) -> Self {
        Self::new(hub.engine_bin(), hub.limits())
    }

    pub async fn tiptap_to_yjs_update(&self, content_json: &Value) -> Result<Vec<u8>, SeedError> {
        let text = serde_json::to_string(content_json)
            .map_err(|e| SeedError::InvalidInput(e.to_string()))?;
        let (engine_bin, limits) = (self.engine_bin.clone(), self.limits);
        tokio::task::spawn_blocking(move || seed_blocking(engine_bin, limits, text))
            .await
            .map_err(|e| SeedError::Failed(e.to_string()))?
    }
}

fn seed_blocking(
    engine_bin: PathBuf,
    limits: Limits,
    content_json: String,
) -> Result<Vec<u8>, SeedError> {
    let mut session = EngineSession::spawn(SpawnRequest {
        engine_bin,
        limits,
        slot_kind: ChildSlotKind::Seed,
        slot_wait: Some(SEED_SLOT_WAIT),
        test_hang_ms: None,
        test_exit_after_read: None,
        test_close_stdout_hang_ms: None,
        test_exit_after_write: None,
    })
    .map_err(|_| SeedError::Unavailable)?;
    let report = session.call(&Request::SeedFromTiptap {
        content_json,
        encoding: 1,
    });
    match report.outcome {
        EngineStatus::Ok {
            update_b64: Some(b64),
            ..
        } => collab_engine::b64::decode(&b64).map_err(|e| SeedError::Failed(e.to_string())),
        EngineStatus::Malformed { detail } => Err(SeedError::InvalidInput(detail)),
        EngineStatus::ResourceLimit {
            kind: LimitKind::Input | LimitKind::Output | LimitKind::Frame,
            detail,
        } => Err(SeedError::TooLarge(detail)),
        other => Err(SeedError::Failed(format!("{other:?}"))),
    }
}
