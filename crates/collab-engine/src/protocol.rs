use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::b64;
use crate::limits::Limits;
use crate::outcome::{EngineStatus, LimitKind};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    Ping,
    /// Load last committed completeV1 snapshot, then apply tail updates in order.
    Load {
        #[serde(default, with = "b64::option")]
        snapshot_b64: Option<Vec<u8>>,
        #[serde(default, with = "b64::vec_of")]
        tail_b64: Vec<Vec<u8>>,
        #[serde(default = "encoding_v1")]
        encoding: u8,
    },
    /// Apply a candidate updateV1. Parent must not treat success as durable.
    Apply {
        #[serde(with = "b64")]
        update_b64: Vec<u8>,
        #[serde(default = "encoding_v1")]
        encoding: u8,
    },
    /// Sync update from a remote state vector (`encode_state_as_update_v1`).
    Sync {
        #[serde(with = "b64")]
        state_vector_b64: Vec<u8>,
        #[serde(default = "encoding_v1")]
        encoding: u8,
    },
    /// Complete V1 snapshot including pending updates and the delete set.
    Snapshot,
    Inspect,
    /// Read-only Tiptap JSON projection of the `prosemirror` XmlFragment.
    /// Does not mutate the Doc. Counts toward the child op budget.
    Project {
        #[serde(default = "encoding_v1")]
        encoding: u8,
    },
    /// Yrs Snapshot (state vector + delete set). Product revision capture.
    /// Does not mutate the Doc. Bytes are returned in `update_b64`.
    RevisionSnapshot,
    /// Compute a forward updateV1 that replaces the live `prosemirror` fragment
    /// with the fragment reconstructed from a Yrs Snapshot. Does not mutate the
    /// live Doc; the parent persists then applies the returned update.
    RestoreFromSnapshot {
        #[serde(with = "b64")]
        snap_b64: Vec<u8>,
        #[serde(default = "encoding_v1")]
        encoding: u8,
    },
    /// Compute a forward updateV1 that replaces the live `prosemirror` fragment
    /// with the fragment of a standalone Doc built from `update_b64` (an external
    /// body write seeded from Tiptap JSON). Does not mutate the live Doc; the
    /// parent persists then applies the returned update.
    ReplaceFromUpdate {
        #[serde(with = "b64")]
        update_b64: Vec<u8>,
        #[serde(default = "encoding_v1")]
        encoding: u8,
    },
    /// Stateless: encode Tiptap JSON (compact JSON text in `content_json`) as the
    /// updateV1 of a fresh Doc (source `tiptapJsonToYUpdate`). Does not touch the
    /// session Doc. JSON travels as a string so the envelope adds no nesting
    /// level to the child's serde_json recursion limit.
    SeedFromTiptap {
        content_json: String,
        #[serde(default = "encoding_v1")]
        encoding: u8,
    },
}

fn encoding_v1() -> u8 {
    1
}

impl Request {
    pub fn encoding(&self) -> u8 {
        match self {
            Self::Load { encoding, .. }
            | Self::Apply { encoding, .. }
            | Self::Sync { encoding, .. }
            | Self::Project { encoding }
            | Self::RestoreFromSnapshot { encoding, .. }
            | Self::ReplaceFromUpdate { encoding, .. }
            | Self::SeedFromTiptap { encoding, .. } => *encoding,
            Self::Ping | Self::Snapshot | Self::Inspect | Self::RevisionSnapshot => 1,
        }
    }

    /// Binary payload size before JSON/base64 expansion.
    pub fn payload_bytes(&self) -> u64 {
        match self {
            Self::Ping
            | Self::Snapshot
            | Self::Inspect
            | Self::Project { .. }
            | Self::RevisionSnapshot => 0,
            Self::Apply { update_b64, .. } => update_b64.len() as u64,
            Self::Sync {
                state_vector_b64, ..
            } => state_vector_b64.len() as u64,
            Self::RestoreFromSnapshot { snap_b64, .. } => snap_b64.len() as u64,
            Self::ReplaceFromUpdate { update_b64, .. } => update_b64.len() as u64,
            Self::SeedFromTiptap { content_json, .. } => content_json.len() as u64,
            Self::Load {
                snapshot_b64,
                tail_b64,
                ..
            } => load_total_bytes(snapshot_b64.as_deref().unwrap_or(&[]), tail_b64),
        }
    }

    pub fn tail_rows(&self) -> usize {
        match self {
            Self::Load { tail_b64, .. } => tail_b64.len(),
            _ => 0,
        }
    }

    /// Cap blobs/rows before `serde_json::to_vec` allocates a base64 copy.
    pub fn preflight(&self, limits: &Limits) -> Result<(), EngineStatus> {
        cap_load_parts(
            match self {
                Self::Load {
                    snapshot_b64,
                    tail_b64,
                    ..
                } => Some((snapshot_b64.as_deref().unwrap_or(&[]), tail_b64.as_slice())),
                _ => None,
            },
            self.payload_bytes(),
            self.tail_rows(),
            limits,
        )
    }
}

pub fn load_total_bytes(snapshot: &[u8], tail: &[Vec<u8>]) -> u64 {
    tail.iter().fold(snapshot.len() as u64, |acc, u| {
        acc.saturating_add(u.len() as u64)
    })
}

fn cap_blob(what: &str, len: u64, limits: &Limits) -> Result<(), EngineStatus> {
    if len > limits.max_input_bytes {
        return Err(EngineStatus::ResourceLimit {
            kind: LimitKind::Input,
            detail: format!(
                "{what} {len} bytes exceeds {}-byte per-blob limit",
                limits.max_input_bytes
            ),
        });
    }
    Ok(())
}

pub fn cap_load_parts(
    load: Option<(&[u8], &[Vec<u8>])>,
    payload_bytes: u64,
    tail_rows: usize,
    limits: &Limits,
) -> Result<(), EngineStatus> {
    if let Some((snapshot, tail)) = load {
        if tail.len() > limits.max_tail_updates {
            return Err(EngineStatus::ResourceLimit {
                kind: LimitKind::Ops,
                detail: format!(
                    "tail {} exceeds max {}",
                    tail.len(),
                    limits.max_tail_updates
                ),
            });
        }
        cap_blob("snapshot", snapshot.len() as u64, limits)?;
        for (i, upd) in tail.iter().enumerate() {
            cap_blob(&format!("tail[{i}]"), upd.len() as u64, limits)?;
        }
        if payload_bytes > limits.max_load_bytes {
            return Err(EngineStatus::ResourceLimit {
                kind: LimitKind::Input,
                detail: format!(
                    "load snapshot+tail {payload_bytes} bytes exceeds {}-byte aggregate limit",
                    limits.max_load_bytes
                ),
            });
        }
        let _ = tail_rows;
        return Ok(());
    }
    cap_blob("payload", payload_bytes, limits)
}

/// Inspect raw JSON (child) and refuse Load tails before base64 decode copies.
pub fn preflight_wire_json(v: &Value, limits: &Limits) -> Result<(), EngineStatus> {
    let Some(obj) = v.as_object() else {
        return Ok(());
    };
    if obj.get("op").and_then(Value::as_str) != Some("load") {
        if let Some(s) = obj.get("update_b64").and_then(Value::as_str) {
            cap_b64_field("update_b64", s, limits)?;
        }
        if let Some(s) = obj.get("state_vector_b64").and_then(Value::as_str) {
            cap_b64_field("state_vector_b64", s, limits)?;
        }
        if let Some(s) = obj.get("snap_b64").and_then(Value::as_str) {
            cap_b64_field("snap_b64", s, limits)?;
        }
        return Ok(());
    }
    let snap = obj
        .get("snapshot_b64")
        .and_then(Value::as_str)
        .unwrap_or("");
    let empty = Vec::new();
    let tail = obj
        .get("tail_b64")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    if tail.len() > limits.max_tail_updates {
        return Err(EngineStatus::ResourceLimit {
            kind: LimitKind::Ops,
            detail: format!(
                "tail {} exceeds max {}",
                tail.len(),
                limits.max_tail_updates
            ),
        });
    }
    let snap_n = b64::decoded_len_estimate(snap.len());
    cap_blob("snapshot", snap_n, limits)?;
    let mut total = snap_n;
    for (i, item) in tail.iter().enumerate() {
        let Some(s) = item.as_str() else {
            return Err(EngineStatus::Malformed {
                detail: format!("tail_b64[{i}] is not a string"),
            });
        };
        let n = b64::decoded_len_estimate(s.len());
        cap_blob(&format!("tail[{i}]"), n, limits)?;
        total = total.saturating_add(n);
        if total > limits.max_load_bytes {
            return Err(EngineStatus::ResourceLimit {
                kind: LimitKind::Input,
                detail: format!(
                    "load snapshot+tail estimate {total} exceeds {}-byte aggregate limit",
                    limits.max_load_bytes
                ),
            });
        }
    }
    Ok(())
}

fn cap_b64_field(name: &str, s: &str, limits: &Limits) -> Result<(), EngineStatus> {
    cap_blob(name, b64::decoded_len_estimate(s.len()), limits)
}
