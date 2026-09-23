use yrs::error::{Error as YrsError, UpdateError};
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::{Encode, Encoder, EncoderV1};
use yrs::{
    Doc, GetString, OffsetKind, Options, ReadTxn, Snapshot, StateVector, Transact, Update,
    XmlFragment, XmlFragmentRef,
};

use crate::b64;
use crate::limits::Limits;
use crate::outcome::{EngineStatus, LimitKind, UnsupportedReason};
use crate::protocol::Request;

pub const FRAGMENT: &str = "prosemirror";

pub fn new_doc() -> Doc {
    // Options::default() uses with_guid_and_client_id: cleanup_formatting=false.
    // small-client (crate feature) keeps 32-bit ClientID for Yjs 13.6.32.
    let opts = Options {
        skip_gc: true,
        offset_kind: OffsetKind::Utf16,
        ..Options::default()
    };
    Doc::with_options(opts)
}

pub struct CollabEngine {
    doc: Doc,
    limits: Limits,
    ops: u32,
    mutated: bool,
}

impl CollabEngine {
    pub fn new(limits: Limits) -> Self {
        Self {
            doc: new_doc(),
            limits,
            ops: 0,
            mutated: false,
        }
    }

    pub fn handle(&mut self, req: &Request) -> EngineStatus {
        if let Err(detail) = self.limits.validate() {
            return EngineStatus::WorkerFailure {
                reason: crate::outcome::WorkerFailureReason::InvalidLimits,
                detail,
            };
        }
        if req.encoding() != 1 {
            return EngineStatus::Unsupported {
                reason: UnsupportedReason::EncodingV2,
                detail: format!(
                    "encoding {} unsupported (expected COLLAB_STATE_ENCODING_V1=1)",
                    req.encoding()
                ),
            };
        }
        match req {
            Request::Ping => EngineStatus::ping_ok(),
            Request::Load {
                snapshot_b64,
                tail_b64,
                ..
            } => {
                if let Err(st) = Request::preflight(req, &self.limits) {
                    return st;
                }
                self.load(snapshot_b64.as_deref().unwrap_or(&[]), tail_b64)
            }
            Request::Apply { update_b64, .. } => self.apply(update_b64),
            Request::Sync {
                state_vector_b64, ..
            } => self.sync(state_vector_b64),
            Request::Snapshot => self.complete_snapshot(),
            Request::Inspect => self.inspect(),
        }
    }

    fn bump_op(&mut self) -> Result<(), EngineStatus> {
        if self.ops >= self.limits.max_ops {
            return Err(EngineStatus::ResourceLimit {
                kind: LimitKind::Ops,
                detail: format!(
                    "child op count {} reached max {}",
                    self.ops, self.limits.max_ops
                ),
            });
        }
        self.ops += 1;
        Ok(())
    }

    fn cap_input(&self, bytes: &[u8], what: &str) -> Result<(), EngineStatus> {
        if bytes.len() as u64 > self.limits.max_input_bytes {
            return Err(EngineStatus::ResourceLimit {
                kind: LimitKind::Input,
                detail: format!(
                    "{what} {} bytes exceeds {}-byte limit",
                    bytes.len(),
                    self.limits.max_input_bytes
                ),
            });
        }
        Ok(())
    }

    fn cap_output(&self, bytes: &[u8], what: &str) -> Result<(), EngineStatus> {
        if bytes.len() as u64 > self.limits.max_output_bytes {
            return Err(EngineStatus::ResourceLimit {
                kind: LimitKind::Output,
                detail: format!(
                    "{what} {} bytes exceeds {}-byte limit",
                    bytes.len(),
                    self.limits.max_output_bytes
                ),
            });
        }
        Ok(())
    }

    pub fn load(&mut self, snapshot: &[u8], tail: &[Vec<u8>]) -> EngineStatus {
        if self.mutated {
            return EngineStatus::Malformed {
                detail: "load allowed once per child session and not after apply; recycle and spawn a fresh session to load another snapshot".into(),
            };
        }
        if let Err(st) = self.bump_op() {
            return st;
        }
        self.mutated = true;
        if let Err(st) = crate::protocol::cap_load_parts(
            Some((snapshot, tail)),
            crate::protocol::load_total_bytes(snapshot, tail),
            tail.len(),
            &self.limits,
        ) {
            return st;
        }
        if !snapshot.is_empty() {
            if let Err(st) = self.apply_v1(snapshot) {
                return st;
            }
        }
        for (i, upd) in tail.iter().enumerate() {
            if let Err(st) = self.apply_v1(upd) {
                return load_tail_error(i, st);
            }
        }
        self.ok_applied(None)
    }

    pub fn apply(&mut self, update: &[u8]) -> EngineStatus {
        if let Err(st) = self.bump_op() {
            return st;
        }
        if let Err(st) = self.apply_v1(update) {
            return st;
        }
        self.mutated = true;
        // Success is admissible only when the authoritative completeV1 (pending +
        // delete set) still fits a reloadable per-blob/output cap. Oversize is
        // not applied-ok: the parent must recycle this child before DB admit.
        // The bytes themselves are not returned here; `Snapshot` supplies them.
        if let Err(st) = self.complete_v1_fits() {
            return st;
        }
        self.ok_applied(None)
    }

    pub fn sync(&mut self, state_vector: &[u8]) -> EngineStatus {
        if let Err(st) = self.bump_op() {
            return st;
        }
        if let Err(st) = self.cap_input(state_vector, "state_vector") {
            return st;
        }
        let sv = match StateVector::decode_v1(state_vector) {
            Ok(sv) => sv,
            Err(err) => return classify_decode(err.into(), "state_vector"),
        };
        let txn = self.doc.transact();
        let bytes = txn.encode_state_as_update_v1(&sv);
        drop(txn);
        if let Err(st) = self.cap_output(&bytes, "sync_update") {
            return st;
        }
        self.ok_applied(Some(bytes))
    }

    pub fn complete_snapshot(&mut self) -> EngineStatus {
        if let Err(st) = self.bump_op() {
            return st;
        }
        self.encode_complete_v1()
    }

    fn encode_complete_v1(&self) -> EngineStatus {
        let txn = self.doc.transact();
        let bytes = txn.encode_state_as_update_v1(&StateVector::default());
        drop(txn);
        if let Err(st) = self.cap_output(&bytes, "complete_v1") {
            return st;
        }
        self.ok_applied(Some(bytes))
    }

    fn complete_v1_fits(&self) -> Result<(), EngineStatus> {
        let txn = self.doc.transact();
        let bytes = txn.encode_state_as_update_v1(&StateVector::default());
        drop(txn);
        self.cap_output(&bytes, "complete_v1")
    }

    pub fn inspect(&mut self) -> EngineStatus {
        if let Err(st) = self.bump_op() {
            return st;
        }
        let txn = self.doc.transact();
        let pending = txn.has_missing_updates();
        let sv = txn.state_vector().encode_v1();
        let (xml_len, xml_string) = xml_view(&txn);
        EngineStatus::Ok {
            applied: false,
            pending,
            durable: false,
            skip_gc: true,
            offset_kind: "utf16".into(),
            encoding: 1,
            fragment: FRAGMENT.into(),
            update_b64: None,
            state_vector_b64: Some(b64::encode(&sv)),
            xml_string: Some(xml_string),
            xml_len: Some(xml_len),
            yrs: Some(crate::YRS_VERSION.into()),
        }
    }

    /// Restore a past revision using a Yrs/Yjs snapshot against the current Doc.
    /// Used by tests; not a product persist path.
    pub fn encode_from_revision_snapshot(
        &self,
        snap_bytes: &[u8],
    ) -> Result<Vec<u8>, EngineStatus> {
        self.cap_input(snap_bytes, "revision_snapshot")?;
        let snap = Snapshot::decode_v1(snap_bytes)
            .map_err(|err| classify_decode(err.into(), "snapshot"))?;
        let txn = self.doc.transact();
        let mut encoder = EncoderV1::new();
        txn.encode_state_from_snapshot(&snap, &mut encoder)
            .map_err(|err| classify_decode(err, "encode_state_from_snapshot"))?;
        let bytes = encoder.to_vec();
        self.cap_output(&bytes, "revision_restore")?;
        Ok(bytes)
    }

    fn apply_v1(&mut self, bytes: &[u8]) -> Result<(), EngineStatus> {
        self.cap_input(bytes, "update")?;
        if bytes.is_empty() {
            return Err(EngineStatus::Malformed {
                detail: "empty updateV1".into(),
            });
        }
        let update =
            Update::decode_v1(bytes).map_err(|err| classify_decode(err.into(), "decode_v1"))?;
        let mut txn = self.doc.transact_mut();
        txn.apply_update(update).map_err(classify_apply)?;
        Ok(())
    }

    fn ok_applied(&self, update: Option<Vec<u8>>) -> EngineStatus {
        let txn = self.doc.transact();
        let pending = txn.has_missing_updates();
        EngineStatus::Ok {
            applied: true,
            pending,
            durable: false,
            skip_gc: true,
            offset_kind: "utf16".into(),
            encoding: 1,
            fragment: FRAGMENT.into(),
            update_b64: update.map(|b| b64::encode(&b)),
            state_vector_b64: None,
            xml_string: None,
            xml_len: None,
            yrs: Some(crate::YRS_VERSION.into()),
        }
    }

    pub fn pending(&self) -> bool {
        self.doc.transact().has_missing_updates()
    }
}

fn xml_view<T: ReadTxn>(txn: &T) -> (u32, String) {
    match txn.get_xml_fragment(FRAGMENT) {
        Some(xml) => xml_len_string(txn, &xml),
        None => (0, String::new()),
    }
}

fn xml_len_string<T: ReadTxn>(txn: &T, xml: &XmlFragmentRef) -> (u32, String) {
    (xml.len(txn), xml.get_string(txn))
}

fn classify_decode(err: YrsError, what: &str) -> EngineStatus {
    match &err {
        YrsError::Gc => {
            return EngineStatus::Unsupported {
                reason: UnsupportedReason::SnapshotRestore,
                detail: format!("{what}: {err} (skip_gc must stay true)"),
            };
        }
        YrsError::ReadError(read) => {
            if matches!(read, yrs::encoding::read::Error::NotEnoughMemory(_)) {
                return EngineStatus::ResourceLimit {
                    kind: LimitKind::Memory,
                    detail: format!("{what}: {err}"),
                };
            }
        }
        YrsError::UpdateError(_) => {}
    }
    EngineStatus::Malformed {
        detail: format!("{what}: {err}"),
    }
}

fn classify_apply(err: UpdateError) -> EngineStatus {
    EngineStatus::Malformed {
        detail: format!("apply_update: {err}"),
    }
}

fn load_tail_error(index: usize, st: EngineStatus) -> EngineStatus {
    match st {
        EngineStatus::ResourceLimit { .. } | EngineStatus::Unsupported { .. } => st,
        other => EngineStatus::Malformed {
            detail: format!("tail[{index}]: {}", status_detail(&other)),
        },
    }
}

fn status_detail(status: &EngineStatus) -> String {
    match status {
        EngineStatus::Malformed { detail }
        | EngineStatus::Unsupported { detail, .. }
        | EngineStatus::ResourceLimit { detail, .. }
        | EngineStatus::WorkerFailure { detail, .. } => detail.clone(),
        EngineStatus::Ok { .. } => "ok".into(),
    }
}

#[cfg(test)]
mod classify_tests {
    use super::classify_decode;
    use crate::outcome::EngineStatus;
    use yrs::encoding::read::Error as ReadError;

    #[test]
    fn unexpected_value_is_malformed() {
        let status = classify_decode(ReadError::UnexpectedValue.into(), "decode_v1");
        assert!(
            matches!(status, EngineStatus::Malformed { ref detail } if detail.contains("unexpected value")),
            "{status:?}"
        );
    }

    #[test]
    fn gc_error_is_unsupported_restore() {
        let status = classify_decode(yrs::error::Error::Gc, "encode_state_from_snapshot");
        assert!(
            matches!(
                status,
                EngineStatus::Unsupported {
                    reason: crate::outcome::UnsupportedReason::SnapshotRestore,
                    ..
                }
            ),
            "{status:?}"
        );
    }

    #[test]
    fn load_tail_keeps_resource_limit_status() {
        let inner = EngineStatus::ResourceLimit {
            kind: crate::outcome::LimitKind::Memory,
            detail: "decode_v1: not enough memory".into(),
        };
        let out = super::load_tail_error(2, inner.clone());
        assert_eq!(out, inner);
    }

    #[test]
    fn load_tail_wraps_malformed() {
        let out = super::load_tail_error(
            1,
            EngineStatus::Malformed {
                detail: "empty updateV1".into(),
            },
        );
        assert!(
            matches!(out, EngineStatus::Malformed { ref detail } if detail.starts_with("tail[1]:")),
            "{out:?}"
        );
    }
}
