use yrs::error::{Error as YrsError, UpdateError};
use yrs::types::text::YChange;
use yrs::types::xml::XmlIn;
use yrs::types::Attrs;
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::{Encode, Encoder, EncoderV1};
use yrs::{
    Any, Doc, GetString, OffsetKind, Options, Out, ReadTxn, Snapshot, StateVector, Text, Transact,
    TransactionMut, Update, Xml, XmlElementPrelim, XmlElementRef, XmlFragment, XmlFragmentRef,
    XmlOut, XmlTextPrelim, XmlTextRef,
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
            Request::ArchiveLoad {
                snapshot_b64,
                tail_b64,
                capture_binding,
                ..
            } => {
                if let Err(st) = Request::preflight(req, &self.limits) {
                    return st;
                }
                self.archive_load(
                    snapshot_b64.as_deref().unwrap_or(&[]),
                    tail_b64,
                    capture_binding,
                )
            }
            Request::Apply { update_b64, .. } => self.apply(update_b64),
            Request::Sync {
                state_vector_b64, ..
            } => self.sync(state_vector_b64),
            Request::Snapshot => self.complete_snapshot(),
            Request::Inspect => self.inspect(),
            Request::Project { .. } => self.project(),
            Request::RevisionSnapshot => self.revision_snapshot(),
            Request::RevisionSnapshotsEqual {
                left_b64,
                right_b64,
            } => {
                if let Err(st) = Request::preflight(req, &self.limits) {
                    return st;
                }
                self.revision_snapshots_equal(left_b64, right_b64)
            }
            Request::RestoreFromSnapshot { snap_b64, .. } => self.restore_from_snapshot(snap_b64),
            Request::ArchiveRestoreFromSnapshot { snap_b64, .. } => {
                if let Err(st) = req.preflight(&self.limits) {
                    return st;
                }
                self.archive_restore_from_snapshot(snap_b64)
            }
            Request::ReplaceFromUpdate { update_b64, .. } => self.replace_from_update(update_b64),
            Request::SeedFromTiptap { content_json, .. } => self.seed_from_tiptap(content_json),
            Request::SeedIndependentFromTiptap { content_json, .. } => {
                self.seed_independent_from_tiptap(content_json)
            }
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

    fn archive_load(&mut self, snapshot: &[u8], tail: &[Vec<u8>], binding: &str) -> EngineStatus {
        if self.mutated {
            return EngineStatus::Malformed {
                detail: "archive load requires a fresh child".into(),
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
        let mut budget = crate::archive_history::worker::Budget::new(self.limits);
        let mut ledger = crate::archive_history::worker::InputLedger::new();
        for bytes in std::iter::once(snapshot)
            .filter(|s| !s.is_empty())
            .chain(tail.iter().map(Vec::as_slice))
        {
            if let Err(st) = self.cap_input(bytes, "archive native blob") {
                return st;
            }
            if bytes.is_empty() {
                return EngineStatus::Malformed {
                    detail: "empty archive tail".into(),
                };
            }
            use yrs::updates::decoder::{Decoder, DecoderV1};
            let mut decoder = DecoderV1::from(bytes);
            let update = match Update::decode(&mut decoder) {
                Ok(u) => u,
                Err(e) => return classify_decode(e.into(), "archive native decode"),
            };
            match decoder.read_to_end() {
                Ok([]) => {}
                _ => {
                    return EngineStatus::Malformed {
                        detail: "archive native trailing bytes".into(),
                    }
                }
            }

            if let Err(st) = ledger.capture(&update, &mut budget) {
                return st;
            }
            if let Err(e) = self.doc.transact_mut().apply_update(update) {
                return classify_apply(e);
            }
        }
        let inventory = {
            // One canonical read scope: owned witness/clone proof precedes
            // read-only classification inside this archive-only call.
            let txn = self.doc.transact();
            match crate::archive_history::worker::inventory(&txn, ledger, binding, &mut budget) {
                Ok(v) => v,
                Err(st) => return st,
            }
        };
        let mut status = self.ok_applied(None);
        if let EngineStatus::Ok {
            native_archive_inventory,
            ..
        } = &mut status
        {
            // start_report already charges the fixed inventory header; moving it
            // into this Box keeps the typed wire result and owned vectors intact.
            *native_archive_inventory = Some(Box::new(inventory));
        }
        status
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
            content_json: None,
            yrs: Some(crate::YRS_VERSION.into()),
            native_archive_inventory: None,
        }
    }

    /// Read-only Tiptap JSON. Does not set `mutated`, so a later `load` is still allowed.
    pub fn project(&mut self) -> EngineStatus {
        if let Err(st) = self.bump_op() {
            return st;
        }
        let txn = self.doc.transact();
        let pending = txn.has_missing_updates();
        let content_json = match crate::project::project_prosemirror(&txn, &self.limits) {
            Ok(json) => json,
            Err(st) => return st,
        };
        EngineStatus::Ok {
            applied: false,
            pending,
            durable: false,
            skip_gc: true,
            offset_kind: "utf16".into(),
            encoding: 1,
            fragment: FRAGMENT.into(),
            update_b64: None,
            state_vector_b64: None,
            xml_string: None,
            xml_len: None,
            content_json: Some(content_json),
            yrs: Some(crate::YRS_VERSION.into()),
            native_archive_inventory: None,
        }
    }

    /// Encode a Yrs Snapshot (state vector + delete set) of the live Doc.
    /// Does not mutate the Doc. Bytes are returned in `update_b64`.
    pub fn revision_snapshots_equal(&mut self, left: &[u8], right: &[u8]) -> EngineStatus {
        if let Err(st) = self.bump_op() {
            return st;
        }
        if let Err(st) = self.cap_input(left, "left_b64") {
            return st;
        }
        if let Err(st) = self.cap_input(right, "right_b64") {
            return st;
        }
        if left == right {
            return self.ok_applied(Some(vec![1]));
        }
        let left_snap = match Snapshot::decode_v1(left) {
            Ok(snap) => snap,
            Err(err) => return classify_decode(err.into(), "left snapshot"),
        };
        let right_snap = match Snapshot::decode_v1(right) {
            Ok(snap) => snap,
            Err(err) => return classify_decode(err.into(), "right snapshot"),
        };
        let equal = left_snap == right_snap;
        self.ok_applied(Some(if equal { vec![1] } else { vec![0] }))
    }

    pub fn revision_snapshot(&mut self) -> EngineStatus {
        if let Err(st) = self.bump_op() {
            return st;
        }
        let txn = self.doc.transact();
        let bytes = txn.snapshot().encode_v1();
        drop(txn);
        if let Err(st) = self.cap_output(&bytes, "revision_snapshot") {
            return st;
        }
        self.ok_applied(Some(bytes))
    }

    /// Compute a forward updateV1 that replaces the live fragment with the
    /// fragment reconstructed from `snap_bytes`. Does not mutate `self.doc`.
    pub fn restore_from_snapshot(&mut self, snap_bytes: &[u8]) -> EngineStatus {
        if let Err(st) = self.bump_op() {
            return st;
        }
        match self.encode_restore_update(snap_bytes) {
            Ok(bytes) => {
                if let Err(st) = self.cap_output(&bytes, "revision_restore") {
                    return st;
                }
                self.ok_applied(Some(bytes))
            }
            Err(st) => st,
        }
    }

    fn archive_restore_from_snapshot(&mut self, snap_bytes: &[u8]) -> EngineStatus {
        if let Err(st) = self.bump_op() {
            return st;
        }
        if let Err(st) = self.cap_input(snap_bytes, "archive revision snapshot") {
            return st;
        }
        use yrs::updates::decoder::{Decoder, DecoderV1};
        let mut decoder = DecoderV1::from(snap_bytes);
        let snapshot = match Snapshot::decode(&mut decoder) {
            Ok(snapshot) => snapshot,
            Err(err) => return classify_decode(err.into(), "archive snapshot"),
        };
        if !matches!(decoder.read_to_end(), Ok([])) {
            return EngineStatus::Malformed {
                detail: "archive snapshot trailing bytes".into(),
            };
        }
        let mut budget = crate::archive_history::worker::Budget::new(self.limits);
        if let Err(st) = crate::archive_history::worker::prove_snapshot(
            &self.doc.transact(),
            &snapshot,
            &mut budget,
        ) {
            return st;
        }
        let bytes = match self.encode_restore_snapshot(&snapshot, Some(&mut budget)) {
            Ok(v) => v,
            Err(st) => return st,
        };
        if let Err(st) = self.cap_output(&bytes, "archive revision restore") {
            return st;
        }
        self.ok_applied(Some(bytes))
    }

    fn encode_restore_update(&self, snap_bytes: &[u8]) -> Result<Vec<u8>, EngineStatus> {
        self.cap_input(snap_bytes, "revision_snapshot")?;
        let snap = Snapshot::decode_v1(snap_bytes)
            .map_err(|err| classify_decode(err.into(), "snapshot"))?;
        self.encode_restore_snapshot(&snap, None)
    }

    fn encode_restore_snapshot(
        &self,
        snap: &Snapshot,
        archive: Option<&mut crate::archive_history::worker::Budget>,
    ) -> Result<Vec<u8>, EngineStatus> {
        let reconstructed = if let Some(budget) = archive {
            // Only this disposable native clone is physically materialized. The
            // encoding must run before its mutable transaction drops/remerges.
            let complete = self
                .doc
                .transact()
                .encode_state_as_update_v1(&StateVector::default());
            self.cap_output(&complete, "archive snapshot scratch source")?;
            budget.delegated_native_bytes(complete.len())?;
            let scratch = new_doc();
            if !complete.is_empty() {
                apply_bytes_to_doc(&scratch, &complete)?;
            }
            drop(complete);
            let bytes = {
                let mut txn = scratch.transact_mut();
                txn.materialize_snapshot(snap);
                let mut encoder = EncoderV1::new();
                txn.encode_state_from_snapshot(snap, &mut encoder)
                    .map_err(|err| classify_decode(err, "archive encode_state_from_snapshot"))?;
                encoder.to_vec()
            };
            drop(scratch);
            self.cap_output(&bytes, "archive reconstructed snapshot")?;
            budget.delegated_native_bytes(bytes.len())?;
            let reconstructed = new_doc();
            if !bytes.is_empty() {
                apply_bytes_to_doc(&reconstructed, &bytes)?;
            }
            crate::archive_history::worker::prove_reconstruction(
                &reconstructed.transact(),
                snap,
                budget,
            )?;
            reconstructed
        } else {
            reconstruct_doc_from_snapshot(&self.doc, snap)?
        };
        let work = clone_doc(&self.doc)?;
        let dest = work.get_or_insert_xml_fragment(FRAGMENT);
        let bytes = {
            let src_txn = reconstructed.transact();
            let mut dst_txn = work.transact_mut();
            replace_fragment_from(&src_txn, &mut dst_txn, &dest)?;
            dst_txn.encode_update_v1()
        };
        Ok(bytes)
    }

    /// Compute a forward updateV1 that replaces the live fragment with the
    /// fragment of a standalone Doc built from `update_bytes`. Same copy as
    /// revision restore; does not mutate `self.doc`.
    pub fn replace_from_update(&mut self, update_bytes: &[u8]) -> EngineStatus {
        if let Err(st) = self.bump_op() {
            return st;
        }
        match self.encode_replace_update(update_bytes) {
            Ok(bytes) => {
                if let Err(st) = self.cap_output(&bytes, "replace_body") {
                    return st;
                }
                self.ok_applied(Some(bytes))
            }
            Err(st) => st,
        }
    }

    fn encode_replace_update(&self, update_bytes: &[u8]) -> Result<Vec<u8>, EngineStatus> {
        self.cap_input(update_bytes, "replace_body")?;
        if update_bytes.is_empty() {
            return Err(EngineStatus::Malformed {
                detail: "empty replace_body updateV1".into(),
            });
        }
        let source = new_doc();
        apply_bytes_to_doc(&source, update_bytes)?;
        if source.transact().has_missing_updates() {
            return Err(EngineStatus::Malformed {
                detail: "replace_body update has missing dependencies".into(),
            });
        }
        let work = clone_doc(&self.doc)?;
        let dest = work.get_or_insert_xml_fragment(FRAGMENT);
        let bytes = {
            let src_txn = source.transact();
            let mut dst_txn = work.transact_mut();
            replace_fragment_from(&src_txn, &mut dst_txn, &dest)?;
            dst_txn.encode_update_v1()
        };
        Ok(bytes)
    }

    /// Stateless Tiptap JSON → updateV1 of a fresh Doc. The JSON text is capped at
    /// the product body size (`max_project_json_bytes`, compact serde_json, as
    /// the parent's `prepare_derived_body` measures it).
    pub fn seed_from_tiptap(&mut self, content_json: &str) -> EngineStatus {
        self.seed_from_tiptap_mode(content_json, false)
    }

    /// Independent new-document copy, never a live/history replacement.
    pub fn seed_independent_from_tiptap(&mut self, content_json: &str) -> EngineStatus {
        self.seed_from_tiptap_mode(content_json, true)
    }

    fn seed_from_tiptap_mode(&mut self, content_json: &str, independent: bool) -> EngineStatus {
        if let Err(st) = self.bump_op() {
            return st;
        }
        if content_json.len() as u64 > self.limits.max_project_json_bytes {
            return EngineStatus::ResourceLimit {
                kind: LimitKind::Input,
                detail: format!(
                    "seed content_json {} bytes exceeds {}-byte limit",
                    content_json.len(),
                    self.limits.max_project_json_bytes
                ),
            };
        }
        let json: serde_json::Value = match serde_json::from_str(content_json) {
            Ok(v) => v,
            Err(err) => {
                return EngineStatus::Malformed {
                    detail: format!("seed: content_json: {err}"),
                };
            }
        };
        let seeded = if independent {
            crate::seed::tiptap_to_independent_yjs_update(&json, &self.limits)
        } else {
            crate::seed::tiptap_to_yjs_update(&json, &self.limits)
        };
        match seeded {
            Ok(bytes) => EngineStatus::Ok {
                applied: false,
                pending: false,
                durable: false,
                skip_gc: true,
                offset_kind: "utf16".into(),
                encoding: 1,
                fragment: FRAGMENT.into(),
                update_b64: Some(b64::encode(&bytes)),
                state_vector_b64: None,
                xml_string: None,
                xml_len: None,
                content_json: None,
                yrs: Some(crate::YRS_VERSION.into()),
                native_archive_inventory: None,
            },
            Err(st) => st,
        }
    }

    /// Restore a past revision using a Yrs/Yjs snapshot against the current Doc.
    /// Used by tests; not a product persist path. Returns past STATE, not a forward update.
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
            content_json: None,
            yrs: Some(crate::YRS_VERSION.into()),
            native_archive_inventory: None,
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

/// Whether two encoded Yrs revision snapshots describe the same document state
/// (source `Y.equalSnapshots`). Returns false when either operand fails to decode.
pub fn revision_snapshots_semantically_equal(a: &[u8], b: &[u8]) -> bool {
    match (Snapshot::decode_v1(a), Snapshot::decode_v1(b)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn reconstruct_doc_from_snapshot(live: &Doc, snap: &Snapshot) -> Result<Doc, EngineStatus> {
    let txn = live.transact();
    let mut encoder = EncoderV1::new();
    txn.encode_state_from_snapshot(snap, &mut encoder)
        .map_err(|err| classify_decode(err, "encode_state_from_snapshot"))?;
    drop(txn);
    let bytes = encoder.to_vec();
    let reconstructed = new_doc();
    if !bytes.is_empty() {
        apply_bytes_to_doc(&reconstructed, &bytes)?;
    }
    Ok(reconstructed)
}

fn clone_doc(live: &Doc) -> Result<Doc, EngineStatus> {
    let complete = {
        let txn = live.transact();
        txn.encode_state_as_update_v1(&StateVector::default())
    };
    let cloned = new_doc();
    if !complete.is_empty() {
        apply_bytes_to_doc(&cloned, &complete)?;
    }
    Ok(cloned)
}

fn apply_bytes_to_doc(doc: &Doc, bytes: &[u8]) -> Result<(), EngineStatus> {
    let update =
        Update::decode_v1(bytes).map_err(|err| classify_decode(err.into(), "decode_v1"))?;
    doc.transact_mut()
        .apply_update(update)
        .map_err(classify_apply)?;
    Ok(())
}

fn replace_fragment_from<T: ReadTxn>(
    src_txn: &T,
    dst_txn: &mut TransactionMut,
    dest: &XmlFragmentRef,
) -> Result<(), EngineStatus> {
    let dest_len = dest.len(dst_txn);
    if dest_len > 0 {
        dest.remove_range(dst_txn, 0, dest_len);
    }
    let Some(src) = src_txn.get_xml_fragment(FRAGMENT) else {
        return Ok(());
    };
    copy_xml_children(src_txn, &src, dst_txn, dest)
}

fn copy_xml_children<T: ReadTxn, D: XmlFragment>(
    src_txn: &T,
    src: &impl XmlFragment,
    dst_txn: &mut TransactionMut,
    dest: &D,
) -> Result<(), EngineStatus> {
    for index in 0..src.len(src_txn) {
        let child = src
            .get(src_txn, index)
            .ok_or_else(|| EngineStatus::Malformed {
                detail: format!("restore: missing Xml child at {index}"),
            })?;
        match child {
            XmlOut::Element(el) => copy_xml_element(src_txn, &el, dst_txn, dest)?,
            XmlOut::Text(text) => copy_xml_text_node(src_txn, &text, dst_txn, dest)?,
            XmlOut::Fragment(_) => {
                return Err(EngineStatus::Malformed {
                    detail: "restore: nested XmlFragment is unsupported".into(),
                });
            }
        }
    }
    Ok(())
}

fn copy_xml_element<T: ReadTxn, D: XmlFragment>(
    src_txn: &T,
    src: &XmlElementRef,
    dst_txn: &mut TransactionMut,
    dest: &D,
) -> Result<(), EngineStatus> {
    let tag = src.try_tag().ok_or_else(|| EngineStatus::Malformed {
        detail: "restore: XmlElement missing tag".into(),
    })?;
    let idx = dest.len(dst_txn);
    dest.push_back(
        dst_txn,
        XmlElementPrelim::new(tag.as_ref(), std::iter::empty::<XmlIn>()),
    );
    let XmlOut::Element(inserted) =
        dest.get(dst_txn, idx)
            .ok_or_else(|| EngineStatus::Malformed {
                detail: "restore: inserted XmlElement missing".into(),
            })?
    else {
        return Err(EngineStatus::Malformed {
            detail: "restore: inserted child is not XmlElement".into(),
        });
    };
    for (key, value) in src.attributes(src_txn) {
        if let Out::Any(any) = value {
            if matches!(any, Any::Undefined) {
                continue;
            }
            inserted.insert_attribute(dst_txn, key, any);
        }
    }
    copy_xml_children(src_txn, src, dst_txn, &inserted)
}

fn copy_xml_text_node<T: ReadTxn, D: XmlFragment>(
    src_txn: &T,
    src: &XmlTextRef,
    dst_txn: &mut TransactionMut,
    dest: &D,
) -> Result<(), EngineStatus> {
    let idx = dest.len(dst_txn);
    dest.push_back(dst_txn, XmlTextPrelim::new(""));
    let XmlOut::Text(inserted) = dest
        .get(dst_txn, idx)
        .ok_or_else(|| EngineStatus::Malformed {
            detail: "restore: inserted XmlText missing".into(),
        })?
    else {
        return Err(EngineStatus::Malformed {
            detail: "restore: inserted child is not XmlText".into(),
        });
    };
    let mut pieces: Vec<(String, Option<Attrs>)> = Vec::new();
    for diff in src.diff(src_txn, YChange::identity) {
        let insert = match diff.insert {
            Out::Any(Any::String(s)) => s.to_string(),
            other => {
                return Err(EngineStatus::Malformed {
                    detail: format!("restore: XmlText insert is not a string ({other:?})"),
                });
            }
        };
        let attrs = diff.attributes.map(|attrs| {
            let mut filtered = Attrs::new();
            for (key, value) in attrs.iter() {
                if key.as_ref() != "ychange" {
                    filtered.insert(key.clone(), value.clone());
                }
            }
            filtered
        });
        pieces.push((insert, attrs));
    }
    let full: String = pieces.iter().map(|(text, _)| text.as_str()).collect();
    if !full.is_empty() {
        inserted.insert(dst_txn, 0, full.as_str());
    }
    let mut offset = 0u32;
    for (insert, attrs) in pieces {
        let len = insert.encode_utf16().count() as u32;
        if let Some(attrs) = attrs {
            if !attrs.is_empty() && len > 0 {
                inserted.format(dst_txn, offset, len, attrs);
            }
        }
        offset = offset.saturating_add(len);
    }
    Ok(())
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
mod independent_seed_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn independent_request_leaves_loaded_session_history_untouched() {
        let source = json!({"content":[{"type":"paragraph","attrs":{"id":"source-block"},"content":[{"type":"text","text":"retained"}]}]});
        let update =
            crate::seed::tiptap_to_yjs_update(&source, &Limits::for_tests()).expect("source");
        let mut engine = CollabEngine::new(Limits::for_tests());
        assert!(engine
            .handle(&Request::Load {
                snapshot_b64: Some(update),
                tail_b64: vec![],
                encoding: 1
            })
            .is_applied_ok());
        // A real deletion makes source history observably different from a fresh copy.
        let fragment = engine.doc.get_or_insert_xml_fragment(FRAGMENT);
        fragment.remove_range(&mut engine.doc.transact_mut(), 0, 1);
        let before = engine.doc.transact().snapshot();
        assert!(!before.delete_set.is_empty());
        let native_before = engine
            .doc
            .transact()
            .encode_state_as_update_v1(&StateVector::default());
        let reply = engine.handle(&Request::SeedIndependentFromTiptap {
            content_json: source.to_string(),
            encoding: 1,
        });
        let bytes = match reply {
            EngineStatus::Ok {
                applied: false,
                pending: false,
                durable: false,
                update_b64: Some(bytes),
                ..
            } => b64::decode(&bytes).expect("seed bytes"),
            other => panic!("independent reply: {other:?}"),
        };
        let copy = new_doc();
        copy.transact_mut()
            .apply_update(Update::decode_v1(&bytes).expect("decode"))
            .expect("apply");
        let snapshot = copy.transact().snapshot();
        assert!(snapshot.delete_set.is_empty());
        assert_eq!(snapshot.state_map.len(), 1);
        for (client, _) in snapshot.state_map.iter() {
            assert!(
                !before.state_map.contains_client(client),
                "copy retained source client"
            );
        }
        let projected = crate::project::project_prosemirror(&copy.transact(), &Limits::for_tests())
            .expect("project");
        assert_eq!(projected["content"][0]["content"][0]["text"], "retained");
        assert_ne!(projected["content"][0]["attrs"]["id"], "source-block");
        assert_eq!(engine.doc.transact().snapshot(), before);
        assert_eq!(
            engine
                .doc
                .transact()
                .encode_state_as_update_v1(&StateVector::default()),
            native_before
        );
        assert!(
            engine.mutated,
            "loaded session mutation bookkeeping retained"
        );
    }

    #[test]
    fn independent_request_obeys_encoding_body_output_and_operation_limits() {
        let request = Request::SeedIndependentFromTiptap {
            content_json:json!({"content":[{"type":"paragraph","content":[{"type":"text","text":"x".repeat(4096)}]}]}).to_string(), encoding:1,
        };
        let mut body = CollabEngine::new(Limits {
            max_project_json_bytes: 128,
            ..Limits::for_tests()
        });
        assert!(matches!(
            body.handle(&request),
            EngineStatus::ResourceLimit {
                kind: LimitKind::Input,
                ..
            }
        ));
        let mut output = CollabEngine::new(Limits {
            max_input_bytes: 1024,
            max_output_bytes: 1024,
            ..Limits::for_tests()
        });
        assert!(matches!(
            output.handle(&request),
            EngineStatus::ResourceLimit {
                kind: LimitKind::Output,
                ..
            }
        ));
        let mut ops = CollabEngine::new(Limits {
            max_ops: 1,
            ..Limits::for_tests()
        });
        let small = Request::SeedIndependentFromTiptap {
            content_json: "{}".into(),
            encoding: 1,
        };
        assert!(matches!(ops.handle(&small), EngineStatus::Ok { .. }));
        assert!(matches!(
            ops.handle(&small),
            EngineStatus::ResourceLimit {
                kind: LimitKind::Ops,
                ..
            }
        ));
        assert!(matches!(
            CollabEngine::new(Limits::for_tests()).handle(&Request::SeedIndependentFromTiptap {
                content_json: "{}".into(),
                encoding: 2
            }),
            EngineStatus::Unsupported {
                reason: UnsupportedReason::EncodingV2,
                ..
            }
        ));
        assert!(matches!(
            CollabEngine::new(Limits::for_tests()).handle(&Request::SeedIndependentFromTiptap {
                content_json: "{".into(),
                encoding: 1
            }),
            EngineStatus::Malformed { .. }
        ));
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
    fn huge_varint_load_tail_is_memory_limit() {
        let mut engine = super::CollabEngine::new(crate::limits::Limits::for_tests());
        let status = engine.handle(&crate::protocol::Request::Load {
            encoding: 1,
            snapshot_b64: None,
            tail_b64: vec![vec![0xff, 0xff, 0xff, 0xff, 0x0f]],
        });
        assert!(
            matches!(
                status,
                EngineStatus::ResourceLimit {
                    kind: crate::outcome::LimitKind::Memory,
                    ..
                }
            ),
            "crafted update must exercise Memory, not merely any rejection: {status:?}"
        );
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
