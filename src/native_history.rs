//! Retained native history of one document/task: capture association,
//! stable content digest, canonical tail/receipt shape, the isolated child
//! inventory/projection/revision replay, and the fail-closed retained closure
//! policy. Shared by the native archive and same-installation callers.
use std::collections::BTreeSet;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub const MAX_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_GRAPH_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_ENTRIES: usize = 10_000;

#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    #[error("native archive limit exceeded")]
    Limit,
    #[error("invalid native archive: {0}")]
    Invalid(String),
    #[error("incomplete native archive: {0}")]
    Unsupported(String),
    #[error("native archive worker unavailable")]
    Worker,
    #[error("native archive operation cancelled")]
    Cancelled,
}

/// Static, bounded refusal reasons that may appear in server logs. Archive
/// and child details can echo user-controlled bytes (e.g. a corrupt container
/// diagnostic), so anything not in this list is logged only by category.
const LOG_REASONS: &[&str] = &[
    "attachment must be stored and not infected",
    "base64",
    "baseline collection item reconciliation",
    "baseline collection reconciliation",
    "body schema or budget",
    "body schema or size",
    "capture tail association",
    "child graph result",
    "child input",
    "child output",
    "collection people",
    "collection views",
    "collections",
    "comments",
    "confirmation hash",
    "container, entry or hash",
    "cutoff",
    "dependencies",
    "derived body disagreement",
    "document parent cycle",
    "document schema version",
    "document tags",
    "duplicate native reports",
    "entry allowlist",
    "external issue links",
    "file open",
    "file read",
    "file size",
    "format version or required models",
    "graph affiliation or identity",
    "graph report budget",
    "labels",
    "milestone or recurrence",
    "milestones",
    "missing creation activity",
    "missing entry",
    "missing file",
    "missing native state for captured body",
    "missing or extra native reports",
    "missing or rebound native report",
    "missing staged key",
    "multiple authors or grants",
    "native dependencies or inventory load",
    "native encoding",
    "native history continuity",
    "native inventory capture association",
    "native projection",
    "native record schema",
    "native report schema or size",
    "native unavailable interval",
    "native/body disagreement",
    "non-baseline task activity",
    "personal input commands",
    "private single-author project required",
    "record",
    "record schema",
    "record serialization",
    "record timestamp",
    "recurrence",
    "reference id",
    "reference outside selected closure",
    "retained application link requires scoped destination mapping",
    "retained changed-kind uncertainty requires explicit confirmation",
    "retained native unavailable intervals require explicit captured confirmation",
    "retained typed dependency outside selected closure",
    "retained typed reference identifier",
    "revision id",
    "revision native load",
    "revision schema",
    "revision snapshot",
    "revision/native disagreement",
    "stars",
    "storage readback hash",
    "storage write",
    "tail",
    "tampered native report semantics",
    "target",
    "task parent cycle",
    "task schema version",
    "task time",
    "time entries",
    "truncated file",
    "view query",
    "view references",
    "views",
];

pub fn log_reason(error: &ArchiveError) -> &'static str {
    let known = |detail: &str| LOG_REASONS.iter().copied().find(|reason| *reason == detail);
    match error {
        ArchiveError::Limit => "limit",
        ArchiveError::Worker => "worker",
        ArchiveError::Cancelled => "cancelled",
        ArchiveError::Invalid(detail) => known(detail).unwrap_or("invalid:unclassified"),
        ArchiveError::Unsupported(detail) => known(detail).unwrap_or("unsupported:unclassified"),
    }
}

pub fn decode(value: &str) -> Result<Vec<u8>, ArchiveError> {
    if value.len() > MAX_BYTES / 3 * 4 + 4 {
        return Err(ArchiveError::Limit);
    }
    STANDARD
        .decode(value)
        .map_err(|_| ArchiveError::Invalid("base64".into()))
}

/// One native target as captured: state bytes, retained tail, stored body and
/// every revision snapshot of that target.
pub struct NativeTargetInput {
    pub kind: String,
    pub id: Uuid,
    pub encoding: i16,
    pub cutoff: i64,
    pub tail_seq: i64,
    pub snapshot: Vec<u8>,
    /// (seq, op_id, payload) in sequence order.
    pub tail: Vec<(i64, Uuid, Vec<u8>)>,
    pub body: Value,
    /// (revision id, encoding, snapshot bytes, stored JSON).
    pub revisions: Vec<(Uuid, i16, Vec<u8>, Value)>,
}

/// SHA256 is capture association/integrity, not source authenticity. Length
/// frames prevent field-boundary ambiguity; revision bytes bind the same cut.
pub fn target_binding(
    workspace: Uuid,
    actor: Uuid,
    captured_at: &str,
    target: &NativeTargetInput,
) -> String {
    fn field(h: &mut Sha256, bytes: &[u8]) {
        h.update((bytes.len() as u64).to_le_bytes());
        h.update(bytes);
    }
    let mut h = Sha256::new();
    field(&mut h, b"fvoci-native-archive-inventory-v1");
    field(&mut h, workspace.as_bytes());
    field(&mut h, actor.as_bytes());
    field(&mut h, captured_at.as_bytes());
    field(&mut h, target.kind.as_bytes());
    field(&mut h, target.id.as_bytes());
    field(&mut h, &target.encoding.to_le_bytes());
    field(&mut h, &target.cutoff.to_le_bytes());
    field(&mut h, &target.tail_seq.to_le_bytes());
    field(&mut h, &target.snapshot);
    for (seq, op, bytes) in &target.tail {
        field(&mut h, &seq.to_le_bytes());
        field(&mut h, op.as_bytes());
        field(&mut h, bytes);
    }
    for (id, encoding, bytes, _) in &target.revisions {
        field(&mut h, id.as_bytes());
        field(&mut h, &encoding.to_le_bytes());
        field(&mut h, bytes);
    }
    hex::encode(h.finalize())
}

/// The canonical retained-history shape the archive validator also enforces:
/// a contiguous tail after the cutoff, each tail operation backed by its exact
/// receipt, and unique receipts no later than the tail.
/// Receipts are (op_id, seq, payload_len, payload_sha256, actor).
pub fn check_canonical_history(
    cutoff: i64,
    tail_seq: i64,
    tail: &[(i64, Uuid, Vec<u8>)],
    receipts: &[(Uuid, i64, i64, Vec<u8>, Uuid)],
) -> Result<(), ArchiveError> {
    let invalid = || ArchiveError::Invalid("native history continuity".into());
    if cutoff < 0 || tail_seq < cutoff {
        return Err(invalid());
    }
    let mut ops = BTreeSet::new();
    for (op, seq, len, sha, _) in receipts {
        if !ops.insert(*op)
            || *seq < 1
            || *seq > tail_seq
            || !(1..=8 * 1024 * 1024).contains(len)
            || sha.len() != 32
        {
            return Err(invalid());
        }
    }
    let mut seq = cutoff;
    for (row_seq, op, bytes) in tail {
        seq = seq.checked_add(1).ok_or_else(invalid)?;
        if *row_seq != seq
            || bytes.is_empty()
            || !receipts.iter().any(|(r_op, r_seq, len, sha, _)| {
                r_op == op
                    && *r_seq == seq
                    && *len == bytes.len() as i64
                    && sha.as_slice() == Sha256::digest(bytes).as_slice()
            })
        {
            return Err(invalid());
        }
    }
    if seq != tail_seq {
        return Err(invalid());
    }
    Ok(())
}

/// Retained revision row metadata a same-installation move keeps as is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetainedRevisionMeta {
    pub reason: String,
    pub created_by: Option<Uuid>,
    /// RFC 3339, microseconds, UTC (the stored timestamptz precision).
    pub created_at: String,
    pub text: String,
    /// Migration 050 restore provenance: all set for a `restore` row, all
    /// `None` otherwise (the row's own constraint).
    pub restored_from_id: Option<Uuid>,
    pub restore_correlation_id: Option<Uuid>,
    pub restore_base_tail_seq: Option<i64>,
    pub restore_committed_tail_seq: Option<i64>,
}

/// Stable content digest of one target: stored body, state, retained tail,
/// every receipt and every revision snapshot/JSON/metadata, length framed.
/// Unlike the capture binding it carries no workspace, actor or read time.
/// `revision_meta` is parallel to `target.revisions`.
pub fn target_content_digest(
    target: &NativeTargetInput,
    receipts: &[(Uuid, i64, i64, Vec<u8>, Uuid)],
    revision_meta: &[RetainedRevisionMeta],
) -> String {
    fn field(h: &mut Sha256, bytes: &[u8]) {
        h.update((bytes.len() as u64).to_le_bytes());
        h.update(bytes);
    }
    let json = |value: &Value| serde_json::to_vec(value).unwrap_or_default();
    let mut h = Sha256::new();
    // v2: retained revision metadata includes the 050 restore provenance.
    field(&mut h, b"fvoci-native-history-content-v2");
    field(&mut h, target.kind.as_bytes());
    field(&mut h, target.id.as_bytes());
    field(&mut h, &json(&target.body));
    field(&mut h, &target.encoding.to_le_bytes());
    field(&mut h, &target.cutoff.to_le_bytes());
    field(&mut h, &target.tail_seq.to_le_bytes());
    field(&mut h, &target.snapshot);
    field(&mut h, &(target.tail.len() as u64).to_le_bytes());
    for (seq, op, bytes) in &target.tail {
        field(&mut h, &seq.to_le_bytes());
        field(&mut h, op.as_bytes());
        field(&mut h, bytes);
    }
    field(&mut h, &(receipts.len() as u64).to_le_bytes());
    for (op, seq, len, sha, actor) in receipts {
        field(&mut h, op.as_bytes());
        field(&mut h, &seq.to_le_bytes());
        field(&mut h, &len.to_le_bytes());
        field(&mut h, sha);
        field(&mut h, actor.as_bytes());
    }
    field(&mut h, &(target.revisions.len() as u64).to_le_bytes());
    for (id, encoding, bytes, content) in &target.revisions {
        field(&mut h, id.as_bytes());
        field(&mut h, &encoding.to_le_bytes());
        field(&mut h, bytes);
        field(&mut h, &json(content));
    }
    field(&mut h, &(revision_meta.len() as u64).to_le_bytes());
    for meta in revision_meta {
        field(&mut h, meta.reason.as_bytes());
        match meta.created_by {
            Some(id) => field(&mut h, id.as_bytes()),
            None => field(&mut h, b""),
        }
        field(&mut h, meta.created_at.as_bytes());
        field(&mut h, meta.text.as_bytes());
        // Presence-marked so an absent value never equals any present one.
        let optional = |h: &mut Sha256, value: Option<Vec<u8>>| match value {
            Some(bytes) => field(h, &[&[1u8][..], &bytes].concat()),
            None => field(h, &[0u8]),
        };
        optional(
            &mut h,
            meta.restored_from_id.map(|id| id.as_bytes().to_vec()),
        );
        optional(
            &mut h,
            meta.restore_correlation_id.map(|id| id.as_bytes().to_vec()),
        );
        optional(
            &mut h,
            meta.restore_base_tail_seq.map(|n| n.to_le_bytes().to_vec()),
        );
        optional(
            &mut h,
            meta.restore_committed_tail_seq
                .map(|n| n.to_le_bytes().to_vec()),
        );
    }
    hex::encode(h.finalize())
}

/// Report schema/association and the engine's own bounded work, independent
/// of any archive or transfer acceptance policy.
pub fn check_report_bounds(
    report: &collab_engine::archive_history::NativeArchiveInventory,
    binding: &str,
    limits: &collab_engine::Limits,
) -> Result<(), ArchiveError> {
    use collab_engine::archive_history::report_wire_fits;
    if report.schema_version != 1 || report.binding != binding {
        return Err(ArchiveError::Invalid(
            "native inventory capture association".into(),
        ));
    }
    if !report_wire_fits(report, limits.max_project_json_bytes)
        || report.work.steps > u64::from(limits.max_project_nodes)
        || report.work.blocks > limits.max_project_nodes
        || report.work.owned_bytes > limits.max_load_bytes
        || report.work.inspected_bytes > limits.max_load_bytes.saturating_mul(4)
    {
        return Err(ArchiveError::Limit);
    }
    for range in &report.unavailable {
        if range.len == 0 || range.id.clock.checked_add(range.len).is_none() {
            return Err(ArchiveError::Invalid("native unavailable interval".into()));
        }
    }
    Ok(())
}

/// Why retained native history cannot be carried by a selected closure. The
/// checks are ordered: an earlier class wins over a later one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetainedHistoryBlocker {
    /// The engine could not prove complete retained identity coverage.
    Incomplete,
    /// Retained native intervals are unavailable (GC/skip/unreachable).
    Unavailable,
    /// A retained changed-kind reference is only potentially a reference.
    Potential,
    /// A retained link points at this application's API.
    ApplicationLink,
    /// A retained typed reference does not carry a UUID.
    ReferenceIdentifier,
    /// A retained typed reference names an object outside the closure.
    OutsideClosure,
}

impl RetainedHistoryBlocker {
    pub fn reason(self) -> &'static str {
        match self {
            Self::Incomplete => "retained native identity/coverage incomplete",
            Self::Unavailable => {
                "retained native unavailable intervals require explicit captured confirmation"
            }
            Self::Potential => "retained changed-kind uncertainty requires explicit confirmation",
            Self::ApplicationLink => {
                "retained application link requires scoped destination mapping"
            }
            Self::ReferenceIdentifier => "retained typed reference identifier",
            Self::OutsideClosure => "retained typed dependency outside selected closure",
        }
    }
}

/// Fail-closed retained-history closure policy shared by archive validation
/// and same-installation callers. `present` answers whether a typed object is
/// inside the caller's fixed closure; external non-application links pass.
pub fn retained_closure_blocker(
    report: &collab_engine::archive_history::NativeArchiveInventory,
    present: impl Fn(collab_engine::archive_history::ReferenceKind, Uuid) -> bool,
) -> Option<RetainedHistoryBlocker> {
    use collab_engine::archive_history::{ReferenceCertainty, ReferenceKind};
    if !report.complete || !report.diagnostics.is_empty() {
        return Some(RetainedHistoryBlocker::Incomplete);
    }
    // Preserving source loss does not expose it through the current preflight
    // or transfer DTO or bind it to the person's confirmation.
    if !report.unavailable.is_empty() {
        return Some(RetainedHistoryBlocker::Unavailable);
    }
    for reference in &report.references {
        // Uncertainty must be exposed to real selected-scope confirmation before
        // this branch can accept it. The current public DTO/UI seam is ungranted.
        if reference.certainty == ReferenceCertainty::Potential {
            return Some(RetainedHistoryBlocker::Potential);
        }
        if reference.kind == ReferenceKind::LinkHref {
            if reference.value.contains("/api/v1/") {
                return Some(RetainedHistoryBlocker::ApplicationLink);
            }
            continue;
        }
        let Ok(id) = Uuid::parse_str(&reference.value) else {
            return Some(RetainedHistoryBlocker::ReferenceIdentifier);
        };
        if !present(reference.kind, id) {
            return Some(RetainedHistoryBlocker::OutsideClosure);
        }
    }
    None
}

/// Stored body versus native projection. The only accepted difference is the
/// product's own empty pair: a document/task created with the canonical empty
/// body whose room was opened (empty native state) but never edited, so no
/// derived body was projected yet. Both derive empty text.
pub(crate) fn same_body(projected: &Value, stored: &Value) -> bool {
    projected == stored
        || (projected == &serde_json::json!({"type": "doc", "content": []})
            && stored == &crate::db::documents::empty_document_json())
}

/// Bounded isolated-child validation of one native target: ArchiveLoad with
/// the capture binding (report checked by `accept` before anything else),
/// projection equal to the stored body, then every revision snapshot restored
/// over the same state projects exactly its stored JSON. Blocking: callers run
/// it on a blocking thread; `stop` is checked between child calls and every
/// child is killed/reaped on drop.
pub fn inspect_native_target(
    target: &NativeTargetInput,
    binding: &str,
    engine_bin: &std::path::Path,
    limits: collab_engine::Limits,
    stop: &tokio_util::sync::CancellationToken,
    accept: &mut dyn FnMut(
        &collab_engine::archive_history::NativeArchiveInventory,
    ) -> Result<(), ArchiveError>,
) -> Result<collab_engine::archive_history::NativeArchiveInventory, ArchiveError> {
    use collab_engine::{
        outcome::EngineStatus,
        process::{ChildSlotKind, EngineSession, SpawnRequest},
        protocol::Request,
    };
    if target.encoding != 1 || target.revisions.iter().any(|r| r.1 != 1) {
        return Err(ArchiveError::Unsupported("native encoding".into()));
    }
    let session = || {
        EngineSession::spawn(SpawnRequest {
            engine_bin: engine_bin.to_path_buf(),
            limits,
            slot_kind: ChildSlotKind::Primary,
            slot_wait: None,
            test_hang_ms: None,
            test_exit_after_read: None,
            test_close_stdout_hang_ms: None,
            test_exit_after_write: None,
        })
        .map_err(|_| ArchiveError::Worker)
    };
    let projection =
        |child: &mut EngineSession| match child.call(&Request::Project { encoding: 1 }).outcome {
            EngineStatus::Ok {
                pending: false,
                content_json: Some(value),
                ..
            } => Ok(value),
            _ => Err(ArchiveError::Invalid("native projection".into())),
        };
    if stop.is_cancelled() {
        return Err(ArchiveError::Cancelled);
    }
    let tail: Vec<Vec<u8>> = target
        .tail
        .iter()
        .map(|(_, _, bytes)| bytes.clone())
        .collect();
    let mut child = session()?;
    let report = match child
        .call(&Request::ArchiveLoad {
            snapshot_b64: Some(target.snapshot.clone()),
            tail_b64: tail.clone(),
            encoding: 1,
            capture_binding: binding.to_owned(),
        })
        .outcome
    {
        EngineStatus::Ok {
            applied: true,
            pending: false,
            skip_gc: true,
            native_archive_inventory: Some(report),
            ..
        } => {
            accept(&report)?;
            report
        }
        EngineStatus::ResourceLimit { .. } => return Err(ArchiveError::Limit),
        _ => {
            return Err(ArchiveError::Invalid(
                "native dependencies or inventory load".into(),
            ))
        }
    };
    if !same_body(&projection(&mut child)?, &target.body) {
        return Err(ArchiveError::Invalid("native/body disagreement".into()));
    }
    drop(child);
    let revision_load = Request::Load {
        snapshot_b64: Some(target.snapshot.clone()),
        tail_b64: tail,
        encoding: 1,
    };
    for (_, _, snapshot, content_json) in &target.revisions {
        if stop.is_cancelled() {
            return Err(ArchiveError::Cancelled);
        }
        let mut child = session()?;
        if !matches!(
            child.call(&revision_load).outcome,
            EngineStatus::Ok {
                applied: true,
                pending: false,
                ..
            }
        ) {
            return Err(ArchiveError::Invalid("revision native load".into()));
        }
        let update = match child
            .call(&Request::ArchiveRestoreFromSnapshot {
                snap_b64: snapshot.clone(),
                encoding: 1,
            })
            .outcome
        {
            EngineStatus::Ok {
                pending: false,
                update_b64: Some(bytes),
                ..
            } => decode(&bytes)?,
            EngineStatus::Malformed { detail } => return Err(ArchiveError::Invalid(detail)),
            _ => return Err(ArchiveError::Invalid("revision snapshot".into())),
        };
        if !matches!(
            child
                .call(&Request::Apply {
                    update_b64: update,
                    encoding: 1
                })
                .outcome,
            EngineStatus::Ok {
                applied: true,
                pending: false,
                ..
            }
        ) || projection(&mut child)? != *content_json
        {
            return Err(ArchiveError::Invalid("revision/native disagreement".into()));
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn native_archive_log_reason_never_echoes_unlisted_details() {
        let marker = "W7-SYNTHETIC-MARKER-7f3c2a";
        for error in [
            ArchiveError::Invalid(format!("child corrupt: {marker}")),
            ArchiveError::Unsupported(marker.into()),
        ] {
            assert!(!log_reason(&error).contains(marker), "{error:?}");
        }
        assert_eq!(
            log_reason(&ArchiveError::Invalid(marker.to_string())),
            "invalid:unclassified"
        );
        assert_eq!(
            log_reason(&ArchiveError::Invalid("native/body disagreement".into())),
            "native/body disagreement"
        );
        assert_eq!(
            log_reason(&ArchiveError::Unsupported("collections".into())),
            "collections"
        );
        assert_eq!(log_reason(&ArchiveError::Limit), "limit");
    }

    #[test]
    fn native_archive_body_comparison_accepts_only_the_product_empty_pair() {
        let empty_native = json!({"type": "doc", "content": []});
        let created = crate::db::documents::empty_document_json();
        assert!(same_body(&empty_native, &created));
        assert!(same_body(&created, &created));
        // Any authored content, or the reverse direction, is still a disagreement.
        let authored = json!({"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"한글"}]}]});
        assert!(!same_body(&empty_native, &authored));
        assert!(!same_body(&authored, &created));
        assert!(!same_body(&created, &empty_native));
    }

    #[test]
    fn native_history_closure_blocker_is_typed_and_ordered() {
        use collab_engine::archive_history::*;
        let inside = Uuid::parse_str("10000000-0000-4000-8000-000000000002").unwrap();
        let reference = |kind, certainty, value: &str| RetainedReference {
            owner: NativeId {
                client: 10,
                clock: 0,
            },
            declaration: NativeId {
                client: 10,
                clock: 2,
            },
            kind,
            certainty,
            value: value.into(),
        };
        let mut report = NativeArchiveInventory {
            binding: String::new(),
            schema_version: 1,
            complete: true,
            references: vec![
                reference(
                    ReferenceKind::Document,
                    ReferenceCertainty::FixedKind,
                    "10000000-0000-4000-8000-000000000002",
                ),
                reference(
                    ReferenceKind::LinkHref,
                    ReferenceCertainty::FixedKind,
                    "https://example.org/api/v2/x",
                ),
            ],
            unavailable: vec![],
            diagnostics: vec![],
            work: InventoryWork::default(),
        };
        let closure =
            |kind: ReferenceKind, id: Uuid| kind == ReferenceKind::Document && id == inside;
        assert_eq!(retained_closure_blocker(&report, closure), None);
        assert_eq!(
            retained_closure_blocker(&report, |_, _| false),
            Some(RetainedHistoryBlocker::OutsideClosure)
        );
        // A task with the same UUID is not the selected document.
        report.references[0].kind = ReferenceKind::Task;
        assert_eq!(
            retained_closure_blocker(&report, closure),
            Some(RetainedHistoryBlocker::OutsideClosure)
        );
        report.references[0].kind = ReferenceKind::Document;
        report.references[0].value = "not-a-uuid".into();
        assert_eq!(
            retained_closure_blocker(&report, closure),
            Some(RetainedHistoryBlocker::ReferenceIdentifier)
        );
        report.references[0].value = inside.to_string();
        report.references[1].value = "https://example.org/api/v1/workspaces".into();
        assert_eq!(
            retained_closure_blocker(&report, closure),
            Some(RetainedHistoryBlocker::ApplicationLink)
        );
        report.references[0].certainty = ReferenceCertainty::Potential;
        assert_eq!(
            retained_closure_blocker(&report, closure),
            Some(RetainedHistoryBlocker::Potential)
        );
        report.unavailable = vec![UnavailableRange {
            id: NativeId {
                client: 10,
                clock: 4,
            },
            len: 1,
            kind: UnavailableKind::Gc,
        }];
        assert_eq!(
            retained_closure_blocker(&report, closure),
            Some(RetainedHistoryBlocker::Unavailable)
        );
        report.complete = false;
        assert_eq!(
            retained_closure_blocker(&report, closure),
            Some(RetainedHistoryBlocker::Incomplete)
        );
        // Every typed reason is a bounded static log literal.
        for blocker in [
            RetainedHistoryBlocker::Unavailable,
            RetainedHistoryBlocker::Potential,
            RetainedHistoryBlocker::ApplicationLink,
            RetainedHistoryBlocker::ReferenceIdentifier,
            RetainedHistoryBlocker::OutsideClosure,
        ] {
            assert_eq!(
                log_reason(&ArchiveError::Unsupported(blocker.reason().into())),
                blocker.reason()
            );
        }
    }
    #[test]
    fn native_history_content_digest_is_stable_and_byte_sensitive() {
        let actor = Uuid::from_u128(7);
        let op = Uuid::from_u128(1);
        let base = || NativeTargetInput {
            kind: "document".into(),
            id: Uuid::from_u128(2),
            encoding: 1,
            cutoff: 0,
            tail_seq: 1,
            snapshot: vec![0, 0],
            tail: vec![(1, op, vec![1, 2, 3])],
            body: json!({"type":"doc","content":[]}),
            revisions: vec![(Uuid::from_u128(3), 1, vec![0, 0], json!({"type":"doc"}))],
        };
        let receipts = vec![(op, 1, 3, Sha256::digest([1, 2, 3]).to_vec(), actor)];
        let meta = vec![RetainedRevisionMeta {
            reason: "session".into(),
            created_by: None,
            created_at: "2026-10-03T00:00:00.000001Z".into(),
            text: String::new(),
            restored_from_id: None,
            restore_correlation_id: None,
            restore_base_tail_seq: None,
            restore_committed_tail_seq: None,
        }];
        let digest = target_content_digest(&base(), &receipts, &meta);
        assert_eq!(digest, target_content_digest(&base(), &receipts, &meta));
        let changes: [fn(&mut RetainedRevisionMeta); 9] = [
            |m: &mut RetainedRevisionMeta| m.reason = "manual".into(),
            |m: &mut RetainedRevisionMeta| m.created_by = Some(Uuid::from_u128(7)),
            |m: &mut RetainedRevisionMeta| m.created_at = "2026-10-03T00:00:00.000002Z".into(),
            |m: &mut RetainedRevisionMeta| m.text = "x".into(),
            |m: &mut RetainedRevisionMeta| m.restored_from_id = Some(Uuid::from_u128(9)),
            |m: &mut RetainedRevisionMeta| m.restore_correlation_id = Some(Uuid::from_u128(10)),
            |m: &mut RetainedRevisionMeta| m.restore_base_tail_seq = Some(0),
            |m: &mut RetainedRevisionMeta| m.restore_committed_tail_seq = Some(0),
            |m: &mut RetainedRevisionMeta| m.restore_committed_tail_seq = Some(1),
        ];
        // Every change moves the digest, and the same bytes in a different
        // provenance field (base vs committed tail 0) do not collide.
        let mut seen = std::collections::BTreeSet::from([digest.clone()]);
        for change in changes {
            let mut changed = meta.clone();
            change(&mut changed[0]);
            assert!(seen.insert(target_content_digest(&base(), &receipts, &changed)));
        }
        let mut changed = base();
        changed.tail[0].2[0] = 9; // same seq and length, different byte
        assert_ne!(target_content_digest(&changed, &receipts, &meta), digest);
        let mut changed = base();
        changed.revisions[0].3 = json!({"type":"doc","content":[]});
        assert_ne!(target_content_digest(&changed, &receipts, &meta), digest);
        let mut changed = base();
        changed.revisions[0].2 = vec![0, 1];
        assert_ne!(target_content_digest(&changed, &receipts, &meta), digest);
        let mut other = receipts.clone();
        other[0].4 = Uuid::from_u128(8);
        assert_ne!(target_content_digest(&base(), &other, &meta), digest);
        // The capture binding moves with workspace/actor/time; content does not.
        assert_ne!(
            target_binding(Uuid::from_u128(4), actor, "2026-10-03T00:00:00Z", &base()),
            target_binding(Uuid::from_u128(4), actor, "2026-10-03T00:00:01Z", &base())
        );
    }
    #[test]
    fn native_history_canonical_shape_requires_contiguous_receipted_tail() {
        let actor = Uuid::parse_str("20000000-0000-4000-8000-000000000001").unwrap();
        let (a, b) = (Uuid::from_u128(1), Uuid::from_u128(2));
        let receipt = |op, seq, bytes: &[u8]| {
            (
                op,
                seq,
                bytes.len() as i64,
                Sha256::digest(bytes).to_vec(),
                actor,
            )
        };
        let tail = vec![(3, a, vec![1, 2]), (4, b, vec![3])];
        // Compacted operations keep receipts below the cutoff.
        let compacted = receipt(Uuid::from_u128(9), 1, &[7]);
        let receipts = vec![
            compacted.clone(),
            receipt(a, 3, &[1, 2]),
            receipt(b, 4, &[3]),
        ];
        check_canonical_history(2, 4, &tail, &receipts).unwrap();
        check_canonical_history(4, 4, &[], &receipts).unwrap();
        let continuity = |result: Result<(), ArchiveError>| {
            assert!(
                matches!(result, Err(ArchiveError::Invalid(m)) if m == "native history continuity")
            )
        };
        continuity(check_canonical_history(1, 4, &tail, &receipts));
        continuity(check_canonical_history(2, 5, &tail, &receipts));
        continuity(check_canonical_history(2, 4, &tail, &receipts[..2]));
        let mut changed = receipts.clone();
        changed[2].3[0] ^= 1;
        continuity(check_canonical_history(2, 4, &tail, &changed));
        let mut changed = receipts.clone();
        changed[1].2 = 3;
        continuity(check_canonical_history(2, 4, &tail, &changed));
        let mut duplicate = receipts.clone();
        duplicate.push(receipt(a, 3, &[1, 2]));
        continuity(check_canonical_history(2, 4, &tail, &duplicate));
        let mut late = receipts.clone();
        late.push(receipt(Uuid::from_u128(3), 5, &[1]));
        continuity(check_canonical_history(2, 4, &tail, &late));
        let gap = vec![(3, a, vec![1, 2]), (5, b, vec![3])];
        continuity(check_canonical_history(2, 5, &gap, &receipts));
    }
}
