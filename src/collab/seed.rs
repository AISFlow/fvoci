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
        self.seed_request(Request::SeedFromTiptap {
            content_json: text,
            encoding: 1,
        })
        .await
    }

    /// Body seed for a NEW independent copy. Only configured block ids change;
    /// resource references retain their canonical meaning. The caller owns new
    /// document identity, current authority and request hashing before new ids.
    /// Await completion: dropping this future does not cancel spawn_blocking.
    pub async fn tiptap_to_independent_yjs_update(
        &self,
        content_json: &Value,
    ) -> Result<Vec<u8>, SeedError> {
        let text = serde_json::to_string(content_json)
            .map_err(|e| SeedError::InvalidInput(e.to_string()))?;
        self.seed_request(Request::SeedIndependentFromTiptap {
            content_json: text,
            encoding: 1,
        })
        .await
    }

    async fn seed_request(&self, request: Request) -> Result<Vec<u8>, SeedError> {
        let (engine_bin, limits) = (self.engine_bin.clone(), self.limits);
        tokio::task::spawn_blocking(move || seed_blocking(engine_bin, limits, request))
            .await
            .map_err(|e| SeedError::Failed(e.to_string()))?
    }
}

fn seed_blocking(
    engine_bin: PathBuf,
    limits: Limits,
    request: Request,
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
    let report = session.call(&request);
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

#[cfg(all(test, feature = "db-tests"))]
mod independent_seed_tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn awaited_independent_parent_seed_preserves_targets_and_default_ids() {
        let engine_bin = crate::collab::config::require_collab_engine_for_tests();
        let limits = Limits::for_tests();
        let engine = SeedEngine::new(engine_bin.clone(), limits);
        let input = json!({"type":"doc","content":[
            {"type":"paragraph","attrs":{"id":"source-block"},"content":[
                {"type":"text","text":"copy 😀"},
                {"type":"mention","attrs":{"entity":"document","id":"document-target","label":"Doc"}}
            ]},
            {"type":"attachment","attrs":{"id":"attachment-target","name":"file.txt"}},
            {"type":"embed","attrs":{"id":"source-embed","ref":"embed-target"}}
        ]});
        let original = input.clone();
        let legacy = engine
            .tiptap_to_yjs_update(&input)
            .await
            .expect("legacy awaited seed");
        let first = engine
            .tiptap_to_independent_yjs_update(&input)
            .await
            .expect("first awaited seed");
        let second = engine
            .tiptap_to_independent_yjs_update(&input)
            .await
            .expect("second awaited seed");
        let mut copied_ids = std::collections::HashSet::new();
        for (index, bytes) in [legacy, first, second].into_iter().enumerate() {
            let captured = crate::collab::revision::capture_revision_offline(
                engine_bin.clone(),
                limits,
                bytes,
                vec![],
            )
            .expect("fresh child readback");
            let body = captured.content_json;
            assert_eq!(body["content"][0]["content"][0]["text"], "copy 😀");
            assert_eq!(
                body["content"][0]["content"][1]["attrs"]["id"],
                "document-target"
            );
            assert_eq!(body["content"][1]["attrs"]["id"], "attachment-target");
            assert_eq!(body["content"][2]["attrs"]["ref"], "embed-target");
            for (position, source_id) in [(0, "source-block"), (2, "source-embed")] {
                let id = body["content"][position]["attrs"]["id"]
                    .as_str()
                    .expect("block id");
                if index == 0 {
                    assert_eq!(id, source_id);
                } else {
                    assert_ne!(id, source_id);
                    assert!(copied_ids.insert(id.to_owned()));
                }
            }
        }
        assert_eq!(input, original);
    }

    #[tokio::test]
    async fn independent_parent_reports_invalid_and_oversize_then_healthy_progress() {
        let engine = SeedEngine::new(
            crate::collab::config::require_collab_engine_for_tests(),
            Limits {
                max_project_json_bytes: 128,
                ..Limits::for_tests()
            },
        );
        assert!(matches!(
            engine
                .tiptap_to_independent_yjs_update(&json!({"content":[{"type":"nope"}]}))
                .await,
            Err(SeedError::InvalidInput(_))
        ));
        let large = json!({"content":[{"type":"paragraph","content":[{"type":"text","text":"x".repeat(1024)}]}]});
        assert!(matches!(
            engine.tiptap_to_independent_yjs_update(&large).await,
            Err(SeedError::TooLarge(_))
        ));
        assert!(!engine
            .tiptap_to_independent_yjs_update(&json!({"content":[{"type":"paragraph"}]}))
            .await
            .expect("healthy subsequent operation")
            .is_empty());
    }
}
