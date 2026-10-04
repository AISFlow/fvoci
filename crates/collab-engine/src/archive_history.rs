//! Archive-only retained native inventory. Parent types do not depend on Yrs.
//! An incomplete identity/classifier is an explicit result, never partial Complete.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeId {
    pub client: u64,
    pub clock: u32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceKind {
    Document,
    Task,
    Attachment,
    LinkHref,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceCertainty {
    FixedKind,
    Potential,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetainedReference {
    pub owner: NativeId,
    pub declaration: NativeId,
    pub kind: ReferenceKind,
    pub certainty: ReferenceCertainty,
    pub value: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IncompleteReason {
    UnknownInputStructure,
    NormalizedInputStructure,
    StructuralMismatch,
    InputContentLoss,
    UnavailableIntervalLoss,
    UnavailableMetadata,
    Pending,
    Skip,
    UnknownRoot,
    UnknownType,
    UnavailableOwner,
    Ancestry,
    UnknownSchemaSlot,
    UnavailableClassifier,
    UnsupportedReferenceKind,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryDiagnostic {
    pub reason: IncompleteReason,
    pub id: Option<NativeId>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnavailableKind {
    Gc,
    Deleted,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnavailableRange {
    pub id: NativeId,
    pub len: u32,
    pub kind: UnavailableKind,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryWork {
    pub steps: u64,
    pub inspected_bytes: u64,
    pub owned_bytes: u64,
    pub blocks: u32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeArchiveInventory {
    pub binding: String,
    pub schema_version: u16,
    pub complete: bool,
    pub references: Vec<RetainedReference>,
    pub unavailable: Vec<UnavailableRange>,
    pub diagnostics: Vec<InventoryDiagnostic>,
    pub work: InventoryWork,
}

#[cfg(feature = "worker")]
pub(crate) mod worker {
    use super::*;
    use crate::{
        limits::Limits,
        outcome::{EngineStatus, LimitKind},
    };
    use std::{collections::BTreeMap, ops::ControlFlow, time::Instant};
    use yrs::retained::{
        self, BranchIdentity, BranchView, InputEntry, InputParent, OwnerView, ParentView,
        RetainedContent, RetainedEntry, RetainedEvent, TypeKind, VisitLimits,
    };
    use yrs::{Any, ReadTxn, Update, ID};

    fn id(v: ID) -> NativeId {
        NativeId {
            client: v.client.get(),
            clock: v.clock,
        }
    }
    fn limited(kind: LimitKind) -> EngineStatus {
        EngineStatus::ResourceLimit {
            kind,
            detail: "native archive inventory budget".into(),
        }
    }
    pub(crate) struct Budget {
        limits: Limits,
        start: Instant,
        pub work: InventoryWork,
        report_wire_reserved: u64,
        report_owned_reserved: u64,
    }
    impl Budget {
        pub fn new(limits: Limits) -> Self {
            Self {
                limits,
                start: Instant::now(),
                work: InventoryWork::default(),
                report_wire_reserved: 0,
                report_owned_reserved: 0,
            }
        }
        fn tick(&self) -> Result<(), EngineStatus> {
            if self.start.elapsed().as_millis() > u128::from(self.limits.timeout_ms) {
                return Err(limited(LimitKind::Time));
            }
            Ok(())
        }
        fn step(&mut self, n: usize) -> Result<(), EngineStatus> {
            self.tick()?;
            self.work.steps = self
                .work
                .steps
                .checked_add(n as u64)
                .ok_or_else(|| limited(LimitKind::Ops))?;
            if self.work.steps > u64::from(self.limits.max_project_nodes) {
                return Err(limited(LimitKind::Ops));
            }
            Ok(())
        }
        fn inspect(&mut self, n: usize) -> Result<(), EngineStatus> {
            self.tick()?;
            self.work.inspected_bytes = self
                .work
                .inspected_bytes
                .checked_add(n as u64)
                .ok_or_else(|| limited(LimitKind::Memory))?;
            if self.work.inspected_bytes > self.limits.max_load_bytes.saturating_mul(4) {
                return Err(limited(LimitKind::Memory));
            }
            Ok(())
        }
        fn own(&mut self, n: usize) -> Result<(), EngineStatus> {
            self.tick()?;
            self.work.owned_bytes = self
                .work
                .owned_bytes
                .checked_add(n as u64)
                .ok_or_else(|| limited(LimitKind::Memory))?;
            if self.work.owned_bytes > self.limits.max_load_bytes {
                return Err(limited(LimitKind::Memory));
            }
            Ok(())
        }
        // Account bounded standard-library buffers and decode work after
        // encoding; the existing child AS/RSS watchdog bounds SDK allocation.
        // This is not a claim of decoder pre-allocation enforcement.
        pub(crate) fn delegated_native_bytes(&mut self, n: usize) -> Result<(), EngineStatus> {
            self.step(1)?;
            self.inspect(n)?;
            self.own(n.checked_mul(2).ok_or_else(|| limited(LimitKind::Memory))?)
        }
        fn report_owned(&mut self, n: usize) -> Result<(), EngineStatus> {
            self.tick()?;
            let next = self
                .report_owned_reserved
                .checked_add(n as u64)
                .ok_or_else(|| limited(LimitKind::Output))?;
            if next > self.limits.max_project_json_bytes {
                return Err(limited(LimitKind::Output));
            }
            self.report_owned_reserved = next;
            Ok(())
        }
        fn report_item<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), EngineStatus> {
            // Standard serialization counts exact escaping without allocating a
            // serialized copy or an owned report field. Reserve its comma too.
            let remaining = self
                .limits
                .max_project_json_bytes
                .saturating_sub(self.report_wire_reserved)
                .saturating_sub(1);
            let mut writer = ReportCounter {
                budget: self,
                count: 0,
                max: remaining,
                failure: None,
            };
            let result = serde_json::to_writer(&mut writer, value);
            let count = writer.count;
            let failure = writer.failure.take();
            drop(writer);
            if let Some(failure) = failure {
                return Err(failure);
            }
            if result.is_err() {
                return Err(limited(LimitKind::Output));
            }
            self.report_wire_reserved = self
                .report_wire_reserved
                .checked_add(count + 1)
                .ok_or_else(|| limited(LimitKind::Output))?;
            Ok(())
        }
        fn start_report(&mut self, binding: &str) -> Result<(), EngineStatus> {
            // Covers object/array names, punctuation and worst-width integer
            // work counters. Final exact serialization remains authoritative.
            const ENVELOPE: u64 = 512;
            if ENVELOPE >= self.limits.max_project_json_bytes {
                return Err(limited(LimitKind::Output));
            }
            self.report_wire_reserved = ENVELOPE;
            self.report_item(binding)?;
            self.report_owned(std::mem::size_of::<NativeArchiveInventory>())?;
            self.report_owned(binding.len())
        }
        fn report_reserve<T>(&mut self, v: &mut Vec<T>) -> Result<(), EngineStatus> {
            if v.len() != v.capacity() {
                return Ok(());
            }
            let size = std::mem::size_of::<T>().max(1);
            let remaining = self
                .limits
                .max_project_json_bytes
                .saturating_sub(self.report_owned_reserved);
            let n = 32usize
                .min((self.limits.max_project_nodes as usize).saturating_sub(v.len()))
                .min(usize::try_from(remaining / size as u64).unwrap_or(usize::MAX));
            if n == 0 {
                return Err(limited(LimitKind::Output));
            }
            let bytes = n
                .checked_mul(size)
                .ok_or_else(|| limited(LimitKind::Output))?;
            self.report_owned(bytes)?;
            self.own(bytes)?;
            v.try_reserve_exact(n)
                .map_err(|_| limited(LimitKind::Memory))
        }
        fn report_string(&mut self, value: &str) -> Result<String, EngineStatus> {
            self.report_owned(value.len())?;
            self.string(value)
        }
        fn string(&mut self, s: &str) -> Result<String, EngineStatus> {
            self.inspect(s.len())?;
            self.own(s.len())?;
            Ok(s.to_owned())
        }
        fn lookup(&mut self, len: usize) -> Result<(), EngineStatus> {
            self.step((len.max(1).ilog2() + 1) as usize)
        }
        fn any(&mut self, v: &Any, depth: u32) -> Result<(), EngineStatus> {
            self.step(1)?;
            if depth > self.limits.max_project_depth {
                return Err(limited(LimitKind::Stack));
            }
            match v {
                Any::String(s) => self.inspect(s.len()),
                Any::Buffer(s) => self.inspect(s.len()),
                Any::Array(a) => {
                    self.step(a.len())?;
                    for v in a.iter() {
                        self.any(v, depth + 1)?;
                    }
                    Ok(())
                }
                Any::Map(m) => {
                    self.step(m.capacity())?;
                    for (k, v) in m.iter() {
                        self.inspect(k.len())?;
                        self.any(v, depth + 1)?;
                    }
                    Ok(())
                }
                _ => Ok(()),
            }
        }
        fn reserve<T>(&mut self, v: &mut Vec<T>) -> Result<(), EngineStatus> {
            if v.len() == v.capacity() {
                let n =
                    32usize.min((self.limits.max_project_nodes as usize).saturating_sub(v.len()));
                if n == 0 {
                    return Err(limited(LimitKind::Memory));
                }
                self.own(
                    n.checked_mul(std::mem::size_of::<T>())
                        .ok_or_else(|| limited(LimitKind::Memory))?,
                )?;
                v.try_reserve_exact(n)
                    .map_err(|_| limited(LimitKind::Memory))?;
            }
            Ok(())
        }
    }
    struct ReportCounter<'a> {
        budget: &'a mut Budget,
        count: u64,
        max: u64,
        failure: Option<EngineStatus>,
    }
    impl std::io::Write for ReportCounter<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let checked = self
                .budget
                .step(1)
                .and_then(|_| self.budget.inspect(bytes.len()));
            if let Err(failure) = checked {
                self.failure = Some(failure);
                return Err(std::io::Error::other("archive report work"));
            }
            let Some(next) = self.count.checked_add(bytes.len() as u64) else {
                self.failure = Some(limited(LimitKind::Output));
                return Err(std::io::Error::other("archive report length"));
            };
            if next > self.max {
                self.failure = Some(limited(LimitKind::Output));
                return Err(std::io::Error::other("archive report bound"));
            }
            self.count = next;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
    enum Identity {
        Root(String),
        Nested(NativeId),
        Unknown,
    }
    #[derive(Clone, Debug, PartialEq, Eq)]
    enum Parent {
        Root,
        Nested(Identity),
        Unavailable,
    }
    #[derive(Clone, Debug, PartialEq, Eq)]
    struct Owner {
        identity: Identity,
        kind: TypeKind,
        tag: Option<String>,
        parent: Parent,
        deleted: bool,
    }
    #[derive(Clone, Debug, PartialEq)]
    enum Payload {
        Text(String),
        Values(Vec<Any>),
        Json(Vec<String>),
        Binary(Vec<u8>),
        Embed(Any),
        Format(String, Any),
        Type(TypeKind, Option<String>),
        Deleted,
        Gc,
        Skip,
        Subdocument,
    }
    #[derive(Clone, Debug)]
    struct Record {
        id: NativeId,
        len: u32,
        parent: Identity,
        key: Option<String>,
        origin: Option<NativeId>,
        right_origin: Option<NativeId>,
        owner: Option<Owner>,
        deleted: Option<bool>,
        payload: Payload,
    }
    impl Record {
        fn unavailable(&self) -> bool {
            matches!(self.payload, Payload::Deleted | Payload::Gc)
        }
        fn end(&self) -> Result<u32, EngineStatus> {
            self.id
                .clock
                .checked_add(self.len)
                .ok_or_else(|| EngineStatus::Malformed {
                    detail: "native clock range".into(),
                })
        }
    }
    fn identity(v: BranchIdentity<'_>, b: &mut Budget) -> Result<Identity, EngineStatus> {
        Ok(match v {
            BranchIdentity::Root(s) => Identity::Root(b.string(s)?),
            BranchIdentity::Nested(v) => Identity::Nested(id(v)),
        })
    }
    fn copy_identity(v: &Identity, b: &mut Budget) -> Result<Identity, EngineStatus> {
        Ok(match v {
            Identity::Root(s) => Identity::Root(b.string(s)?),
            Identity::Nested(v) => Identity::Nested(*v),
            Identity::Unknown => Identity::Unknown,
        })
    }
    fn owner(v: OwnerView<'_>, b: &mut Budget) -> Result<Option<Owner>, EngineStatus> {
        match v {
            OwnerView::Unavailable => Ok(None),
            OwnerView::Resolved(v) => Ok(Some(branch(v, b)?)),
        }
    }
    fn branch(v: BranchView<'_>, b: &mut Budget) -> Result<Owner, EngineStatus> {
        b.step(1)?;
        Ok(Owner {
            identity: identity(v.id, b)?,
            kind: v.kind,
            tag: v.xml_tag.map(|s| b.string(s)).transpose()?,
            parent: match v.parent {
                ParentView::Root => Parent::Root,
                ParentView::Nested(v) => Parent::Nested(identity(v, b)?),
                ParentView::Unavailable => Parent::Unavailable,
            },
            deleted: v.deleted,
        })
    }
    fn payload(v: RetainedContent<'_>, b: &mut Budget) -> Result<Payload, EngineStatus> {
        b.step(1)?;
        Ok(match v {
            RetainedContent::String(s) => Payload::Text(b.string(s)?),
            RetainedContent::Values(v) => {
                b.step(v.len())?;
                b.own(std::mem::size_of_val(v))?;
                for x in v {
                    b.any(x, 0)?;
                }
                Payload::Values(v.to_vec())
            }
            RetainedContent::JsonValues(v) => {
                b.step(v.len())?;
                b.own(std::mem::size_of_val(v))?;
                let mut a = Vec::with_capacity(v.len());
                for s in v {
                    a.push(b.string(s)?);
                }
                Payload::Json(a)
            }
            RetainedContent::Binary(v) => {
                b.inspect(v.len())?;
                b.own(v.len())?;
                Payload::Binary(v.to_vec())
            }
            RetainedContent::Embed(v) => {
                b.any(v, 0)?;
                Payload::Embed(v.clone())
            }
            RetainedContent::Format { key, value } => {
                b.any(value, 0)?;
                Payload::Format(b.string(key)?, value.clone())
            }
            RetainedContent::Type { kind, xml_tag } => {
                Payload::Type(kind, xml_tag.map(|s| b.string(s)).transpose()?)
            }
            RetainedContent::Deleted { .. } => Payload::Deleted,
            RetainedContent::Subdocument => Payload::Subdocument,
        })
    }
    pub(crate) struct InputLedger {
        // Available payloads are owned by the opaque SDK witness only.
        records: Vec<Record>,
        witnesses: Vec<retained::OwnedInputWitness>,
        proof_failure: Option<IncompleteReason>,
    }
    impl InputLedger {
        pub fn new() -> Self {
            Self {
                records: Vec::new(),
                witnesses: Vec::new(),
                proof_failure: None,
            }
        }
        pub fn capture(&mut self, u: &Update, b: &mut Budget) -> Result<(), EngineStatus> {
            let limits = witness_limits(b);
            let mut failure = None;
            let captured = retained::capture_owned_input(u, limits, &mut |cost| {
                witness_charge(b, cost, &mut failure)
            });
            if let Some(error) = failure {
                return Err(error);
            }
            match captured {
                Ok(witness) => {
                    b.reserve(&mut self.witnesses)?;
                    self.witnesses.push(witness);
                }
                Err(retained::WitnessError::Budget) => return Err(limited(LimitKind::Memory)),
                Err(error) => self.proof_failure = Some(witness_diagnostic(error)),
            }
            let result = retained::visit_input(u, visit_limits(b), |e| {
                let result = (|| {
                    b.step(1)?;
                    // Retain only loss metadata; a refused capture is never a
                    // silently omitted input of a complete report.
                    if matches!(&e, InputEntry::Item { content, .. }
                        if !matches!(content, RetainedContent::Deleted { .. } | RetainedContent::Subdocument)) {
                        return Ok(());
                    }
                    b.reserve(&mut self.records)?;
                    let r = match e {
                        InputEntry::Item {
                            id: v,
                            native_clock_len: len,
                            parent,
                            map_key,
                            origin,
                            right_origin,
                            content,
                        } => Record {
                            id: id(v),
                            len,
                            parent: match parent {
                                InputParent::Named(s) => Identity::Root(b.string(s)?),
                                InputParent::Id(v) => Identity::Nested(id(v)),
                                InputParent::Branch(v) => identity(v, b)?,
                                InputParent::Unknown => Identity::Unknown,
                            },
                            key: map_key.map(|s| b.string(s)).transpose()?,
                            origin: origin.map(id),
                            right_origin: right_origin.map(id),
                            owner: None,
                            deleted: None,
                            payload: payload(content, b)?,
                        },
                        InputEntry::Gc {
                            id: v,
                            native_clock_len: len,
                        } => marker(id(v), len, Payload::Gc),
                        InputEntry::Skip {
                            id: v,
                            native_clock_len: len,
                        } => marker(id(v), len, Payload::Skip),
                    };
                    self.records.push(r);
                    Ok(())
                })();
                match result {
                    Ok(()) => ControlFlow::Continue(()),
                    Err(e) => ControlFlow::Break(e),
                }
            })
            .map_err(|_| limited(LimitKind::Memory))?;
            match result {
                ControlFlow::Continue(()) => Ok(()),
                ControlFlow::Break(e) => Err(e),
            }
        }
    }
    fn witness_limits(b: &Budget) -> retained::WitnessLimits {
        retained::WitnessLimits {
            max_blocks: b.limits.max_project_nodes as usize,
            max_roots: b.limits.max_project_depth as usize,
            max_table_capacity: b.limits.max_project_nodes as usize,
            max_depth: b.limits.max_project_depth as usize,
            max_native_bytes: b.limits.max_output_bytes as usize,
        }
    }
    fn witness_charge(
        b: &mut Budget,
        cost: retained::WitnessCost,
        failure: &mut Option<EngineStatus>,
    ) -> bool {
        if failure.is_some() {
            return false;
        }
        match b
            .step(cost.work)
            .and_then(|_| b.inspect(cost.inspected_bytes))
            .and_then(|_| b.own(cost.owned_bytes))
        {
            Ok(()) => true,
            Err(error) => {
                *failure = Some(error);
                false
            }
        }
    }
    fn witness_diagnostic(error: retained::WitnessError) -> IncompleteReason {
        match error {
            retained::WitnessError::Unsupported("pending") => IncompleteReason::Pending,
            retained::WitnessError::Unsupported("Skip current" | "Skip input") => {
                IncompleteReason::Skip
            }
            retained::WitnessError::Unsupported(_) => IncompleteReason::InputContentLoss,
            _ => IncompleteReason::StructuralMismatch,
        }
    }
    fn marker(id: NativeId, len: u32, payload: Payload) -> Record {
        Record {
            id,
            len,
            parent: Identity::Unknown,
            key: None,
            origin: None,
            right_origin: None,
            owner: None,
            deleted: Some(true),
            payload,
        }
    }
    fn visit_limits(b: &Budget) -> VisitLimits {
        VisitLimits {
            max_blocks: b.limits.max_project_nodes as usize,
            max_roots: b.limits.max_project_depth as usize,
            max_table_capacity: b.limits.max_project_nodes as usize,
        }
    }
    fn diagnostic(
        report: &mut NativeArchiveInventory,
        reason: IncompleteReason,
        id: Option<NativeId>,
        b: &mut Budget,
    ) -> Result<(), EngineStatus> {
        b.step(1)?;
        let item = InventoryDiagnostic { reason, id };
        b.report_item(&item)?;
        b.report_reserve(&mut report.diagnostics)?;
        report.diagnostics.push(item);
        report.complete = false;
        Ok(())
    }
    fn containing<'a>(
        records: &'a BTreeMap<NativeId, Record>,
        at: NativeId,
        b: &mut Budget,
    ) -> Result<Option<&'a Record>, EngineStatus> {
        b.lookup(records.len())?;
        match records.range(..=at).next_back() {
            Some((_, r)) if r.id.client == at.client && at.clock < r.end()? => Ok(Some(r)),
            _ => Ok(None),
        }
    }
    fn unavailable_covered(
        a: &Record,
        loaded: &BTreeMap<NativeId, Record>,
        b: &mut Budget,
    ) -> Result<bool, EngineStatus> {
        let end = a.end()?;
        let mut at = a.id;
        while at.clock < end {
            b.step(1)?;
            let Some(z) = containing(loaded, at, b)? else {
                return Ok(false);
            };
            if !z.unavailable() {
                return Ok(false);
            }
            at.clock = end.min(z.end()?);
        }
        Ok(true)
    }
    pub(crate) fn inventory<T: ReadTxn>(
        txn: &T,
        mut ledger: InputLedger,
        binding: &str,
        b: &mut Budget,
    ) -> Result<NativeArchiveInventory, EngineStatus> {
        // Same canonical read transaction across proof and classification.
        let mut proof_diagnostic = ledger.proof_failure;
        if proof_diagnostic.is_none() {
            let limits = witness_limits(b);
            let mut failure = None;
            let proof = retained::verify_owned_witnesses(
                txn,
                std::mem::take(&mut ledger.witnesses),
                limits,
                &mut |cost| witness_charge(b, cost, &mut failure),
            );
            if let Some(error) = failure {
                return Err(error);
            }
            match proof {
                Ok(_) => {}
                Err(retained::WitnessError::Budget) => return Err(limited(LimitKind::Memory)),
                Err(error) => proof_diagnostic = Some(witness_diagnostic(error)),
            }
        }
        b.start_report(binding)?;
        let mut report = NativeArchiveInventory {
            binding: b.string(binding)?,
            schema_version: 1,
            complete: true,
            references: Vec::new(),
            unavailable: Vec::new(),
            diagnostics: Vec::new(),
            work: InventoryWork::default(),
        };
        if let Some(reason) = proof_diagnostic {
            diagnostic(&mut report, reason, None, b)?;
        }
        let summary = retained::summary(txn);
        if summary.pending_update || summary.pending_deletions {
            diagnostic(&mut report, IncompleteReason::Pending, None, b)?;
        }
        let mut records = BTreeMap::new();
        let mut roots = Vec::new();
        let result = txn
            .visit_retained(visit_limits(b), |event| {
                let result = (|| {
                    b.step(1)?;
                    match event {
                        RetainedEvent::Root(v) => {
                            b.reserve(&mut roots)?;
                            roots.push(branch(v, b)?);
                        }
                        RetainedEvent::Block(entry) => {
                            // Conservative scratch accounting includes tree node overhead.
                            b.own(std::mem::size_of::<Record>().saturating_mul(2))?;
                            let r = match entry {
                                RetainedEntry::Item {
                                    id: v,
                                    native_clock_len: len,
                                    owner: vowner,
                                    map_key,
                                    origin,
                                    right_origin,
                                    deleted,
                                    content,
                                } => {
                                    let owner = owner(vowner, b)?;
                                    let parent = match owner.as_ref() {
                                        Some(v) => copy_identity(&v.identity, b)?,
                                        None => Identity::Unknown,
                                    };
                                    Record {
                                        id: id(v),
                                        len,
                                        parent,
                                        key: map_key.map(|s| b.string(s)).transpose()?,
                                        origin: origin.map(id),
                                        right_origin: right_origin.map(id),
                                        owner,
                                        deleted: Some(deleted),
                                        payload: payload(content, b)?,
                                    }
                                }
                                RetainedEntry::Gc {
                                    id: v,
                                    native_clock_len: len,
                                } => marker(id(v), len, Payload::Gc),
                                RetainedEntry::Skip {
                                    id: v,
                                    native_clock_len: len,
                                } => marker(id(v), len, Payload::Skip),
                            };
                            b.work.blocks = b
                                .work
                                .blocks
                                .checked_add(1)
                                .ok_or_else(|| limited(LimitKind::Ops))?;
                            if records.insert(r.id, r).is_some() {
                                return Err(EngineStatus::Malformed {
                                    detail: "duplicate retained ID".into(),
                                });
                            }
                        }
                    }
                    Ok(())
                })();
                match result {
                    Ok(()) => ControlFlow::Continue(()),
                    Err(e) => ControlFlow::Break(e),
                }
            })
            .map_err(|_| limited(LimitKind::Memory))?;
        if let ControlFlow::Break(e) = result {
            return Err(e);
        }
        for root in &roots {
            b.step(1)?;
            if !matches!(&root.identity,Identity::Root(s) if s==crate::FRAGMENT)
                || !matches!(root.kind, TypeKind::Unknown | TypeKind::XmlFragment)
                || root.parent != Parent::Root
            {
                diagnostic(&mut report, IncompleteReason::UnknownRoot, None, b)?;
            }
        }
        // No source availability is inferred from a rendered Project or SV.
        for a in &ledger.records {
            b.step(1)?;
            if matches!(a.payload, Payload::Skip | Payload::Subdocument) {
                diagnostic(&mut report, IncompleteReason::Skip, Some(a.id), b)?;
                continue;
            }
            if a.unavailable() {
                if !unavailable_covered(a, &records, b)? {
                    diagnostic(
                        &mut report,
                        IncompleteReason::UnavailableIntervalLoss,
                        Some(a.id),
                        b,
                    )?;
                }
                if matches!(a.payload, Payload::Deleted) {
                    b.lookup(records.len())?;
                    match records.get(&a.id) {
                        Some(z)
                            if z.len == a.len
                                && matches!(z.payload, Payload::Deleted)
                                && a.parent != Identity::Unknown
                                && a.parent == z.parent
                                && a.key == z.key
                                && a.origin == z.origin
                                && a.right_origin == z.right_origin => {}
                        _ => diagnostic(
                            &mut report,
                            IncompleteReason::UnavailableMetadata,
                            Some(a.id),
                            b,
                        )?,
                    }
                }
                continue;
            }
            // Available-input normalized identity and coverage were proved
            // above. Only loss markers remain in the consumer ledger.
        }
        // Reverse coverage prevents unexplained unavailable ranges being silently
        // added by standard load. Scans are charged and stop at the work bound.
        for z in records.values().filter(|z| z.unavailable()) {
            let mut at = z.id.clock;
            let end = z.end()?;
            while at < end {
                let mut covered = at;
                for a in ledger.records.iter().filter(|a| a.unavailable()) {
                    b.step(1)?;
                    if a.id.client == z.id.client && a.id.clock <= at && at < a.end()? {
                        covered = covered.max(a.end()?);
                    }
                }
                if covered == at {
                    diagnostic(
                        &mut report,
                        IncompleteReason::UnavailableIntervalLoss,
                        Some(z.id),
                        b,
                    )?;
                    break;
                }
                at = end.min(covered);
            }
            let item = UnavailableRange {
                id: z.id,
                len: z.len,
                kind: if matches!(z.payload, Payload::Gc) {
                    UnavailableKind::Gc
                } else {
                    UnavailableKind::Deleted
                },
            };
            b.report_item(&item)?;
            b.report_reserve(&mut report.unavailable)?;
            report.unavailable.push(item);
        }
        let mut kinds = BTreeMap::<NativeId, u8>::new();
        for z in records.values() {
            b.step(1)?;
            if z.key.as_deref() != Some("entity") {
                continue;
            }
            let Identity::Nested(owner_id) = z.parent else {
                continue;
            };
            let bits = match one_string(&z.payload, b)? {
                Some("document") => 1,
                Some("task") => 2,
                _ => 4,
            };
            b.lookup(kinds.len())?;
            b.own(std::mem::size_of::<(NativeId, u8)>().saturating_mul(4))?;
            *kinds.entry(owner_id).or_default() |= bits;
        }
        for z in records.values() {
            classify(z, &records, &kinds, &mut report, b)?;
        }
        report.work = b.work.clone();
        bound_report(&report, b)?;
        Ok(report)
    }
    #[derive(Clone, Copy)]
    struct HistoryRange {
        id: NativeId,
        end: u32,
        available: bool,
        deleted: bool,
    }
    fn snapshot_failure(detail: &str) -> EngineStatus {
        EngineStatus::Malformed {
            detail: format!("archive saved snapshot: {detail}"),
        }
    }
    fn normalized_deletions(set: &yrs::IdSet, b: &mut Budget) -> Result<yrs::IdSet, EngineStatus> {
        let mut normalized = yrs::IdSet::new();
        b.step(set.len())?;
        for (client, ranges) in set.iter() {
            b.step(ranges.len())?;
            for range in ranges.iter() {
                let len = range
                    .end
                    .checked_sub(range.start)
                    .filter(|len| *len != 0)
                    .ok_or_else(|| snapshot_failure("empty or reversed snapshot DS range"))?;
                b.lookup(normalized.len())?;
                // Standard IdSet::insert coalesces adjacency/overlap. Charge
                // the existing range-vector scan/move envelope before insertion
                // instead of permitting quadratic work outside the step cap.
                b.step(normalized.get(client).map_or(0, |r| r.len()) + 1)?;
                b.own(128)?;
                normalized.insert(yrs::ID::new(*client, range.start), len);
            }
        }
        b.tick()?;
        Ok(normalized)
    }
    fn history_covers(
        rows: &BTreeMap<NativeId, HistoryRange>,
        client: u64,
        start: u32,
        end: u32,
        require_deleted: bool,
        b: &mut Budget,
    ) -> Result<(), EngineStatus> {
        let mut at = start;
        while at < end {
            b.step(1)?;
            b.lookup(rows.len())?;
            let key = NativeId { client, clock: at };
            let Some((_, row)) = rows.range(..=key).next_back() else {
                return Err(snapshot_failure("missing captured native history"));
            };
            if row.id.client != client || row.id.clock > at || row.end <= at {
                return Err(snapshot_failure("gap or unknown captured client"));
            }
            if !row.available {
                return Err(snapshot_failure("unavailable required native history"));
            }
            if require_deleted && !row.deleted {
                return Err(snapshot_failure("DS requests uncaptured deletion"));
            }
            at = end.min(row.end);
        }
        Ok(())
    }
    /// Snapshot policy over standard decoded SV/DS and the captured held store.
    /// No private store handles, input framing, clocks inferred from rendered JSON
    /// or mutation of the native item history.
    pub(crate) fn prove_snapshot<T: ReadTxn>(
        txn: &T,
        snapshot: &yrs::Snapshot,
        b: &mut Budget,
    ) -> Result<(), EngineStatus> {
        let summary = retained::summary(txn);
        if summary.pending_update || summary.pending_deletions {
            return Err(snapshot_failure(
                "captured store has pending native history",
            ));
        }
        b.step(snapshot.state_map.table_capacity())?;
        b.own(
            snapshot
                .state_map
                .table_capacity()
                .checked_mul(std::mem::size_of::<(u64, u32)>() * 4)
                .ok_or_else(|| limited(LimitKind::Memory))?,
        )?;
        // IdSet uses BTreeMap, so its public len/ordered range iterators bound
        // traversal directly. There is no sparse hash table to scan here.
        b.step(snapshot.delete_set.len())?;
        b.own(
            snapshot
                .delete_set
                .len()
                .checked_mul(std::mem::size_of::<(u64, usize)>() * 4)
                .ok_or_else(|| limited(LimitKind::Memory))?,
        )?;
        let mut rows = BTreeMap::new();
        let result = txn
            .visit_retained(visit_limits(b), |event| {
                let result = (|| {
                    b.step(1)?;
                    let RetainedEvent::Block(entry) = event else {
                        return Ok(());
                    };
                    let (native, len, available, deleted) = match entry {
                        RetainedEntry::Item {
                            id,
                            native_clock_len,
                            content,
                            deleted,
                            ..
                        } => (
                            id,
                            native_clock_len,
                            !matches!(
                                content,
                                RetainedContent::Deleted { .. } | RetainedContent::Subdocument
                            ),
                            deleted,
                        ),
                        RetainedEntry::Gc {
                            id,
                            native_clock_len,
                        }
                        | RetainedEntry::Skip {
                            id,
                            native_clock_len,
                        } => (id, native_clock_len, false, true),
                    };
                    b.lookup(rows.len())?;
                    b.own(std::mem::size_of::<HistoryRange>() * 4)?;
                    let native = id(native);
                    let end = native
                        .clock
                        .checked_add(len)
                        .ok_or_else(|| snapshot_failure("native clock overflow"))?;
                    if rows
                        .insert(
                            native,
                            HistoryRange {
                                id: native,
                                end,
                                available,
                                deleted,
                            },
                        )
                        .is_some()
                    {
                        return Err(snapshot_failure("duplicate captured native range"));
                    }
                    Ok(())
                })();
                match result {
                    Ok(()) => ControlFlow::Continue(()),
                    Err(st) => ControlFlow::Break(st),
                }
            })
            .map_err(|_| limited(LimitKind::Ops))?;
        if let ControlFlow::Break(st) = result {
            return Err(st);
        }
        for (client, clock) in snapshot.state_map.iter() {
            b.step(1)?;
            // A genuine empty snapshot has no SV entries. A zero or unknown
            // named client is not proof of captured inserted history.
            if *clock == 0 {
                return Err(snapshot_failure("zero required client clock"));
            }
            history_covers(&rows, client.get(), 0, *clock, false, b)?;
        }
        for (client, ranges) in snapshot.delete_set.iter() {
            b.step(1)?;
            b.step(ranges.len())?;
            b.own(
                ranges
                    .len()
                    .checked_mul(std::mem::size_of::<std::ops::Range<u32>>() * 4)
                    .ok_or_else(|| limited(LimitKind::Memory))?,
            )?;
            if !snapshot.state_map.contains_client(client) {
                return Err(snapshot_failure("DS client absent from snapshot SV"));
            }
            let end = snapshot.state_map.get(client);
            for range in ranges.iter() {
                b.step(1)?;
                if range.start >= range.end || range.end > end {
                    return Err(snapshot_failure("DS outside required snapshot clock"));
                }
                history_covers(&rows, client.get(), range.start, range.end, true, b)?;
            }
        }
        b.tick()
    }
    /// Check all actual output intervals, including clients invisible to a
    /// state vector because a Skip/gap stops its contiguous prefix. The output
    /// must describe precisely the decoded recorded snapshot, not a clamped or
    /// enlarged cut that happens to render matching metadata.
    pub(crate) fn prove_reconstruction<T: ReadTxn>(
        txn: &T,
        snapshot: &yrs::Snapshot,
        b: &mut Budget,
    ) -> Result<(), EngineStatus> {
        let summary = retained::summary(txn);
        if summary.pending_update || summary.pending_deletions {
            return Err(snapshot_failure(
                "reconstructed store has pending native history",
            ));
        }
        let mut rows = BTreeMap::new();
        let result = txn
            .visit_retained(visit_limits(b), |event| {
                let result = (|| {
                    b.step(1)?;
                    let RetainedEvent::Block(entry) = event else {
                        return Ok(());
                    };
                    let (native, len, available, deleted) = match entry {
                        RetainedEntry::Item {
                            id,
                            native_clock_len,
                            content,
                            deleted,
                            ..
                        } => (
                            id,
                            native_clock_len,
                            !matches!(
                                content,
                                RetainedContent::Deleted { .. } | RetainedContent::Subdocument
                            ),
                            deleted,
                        ),
                        RetainedEntry::Gc {
                            id,
                            native_clock_len,
                        }
                        | RetainedEntry::Skip {
                            id,
                            native_clock_len,
                        } => (id, native_clock_len, false, true),
                    };
                    if !available {
                        return Err(snapshot_failure("unavailable reconstructed history"));
                    }
                    let end = native
                        .clock
                        .checked_add(len)
                        .ok_or_else(|| snapshot_failure("output clock overflow"))?;
                    b.step(1)?;
                    if !snapshot.state_map.contains_client(&native.client)
                        || end > snapshot.state_map.get(&native.client)
                    {
                        return Err(snapshot_failure("output exceeds saved snapshot clocks"));
                    }
                    if len == 0 {
                        return Err(snapshot_failure("empty reconstructed native range"));
                    }
                    b.lookup(rows.len())?;
                    b.own(std::mem::size_of::<HistoryRange>() * 4)?;
                    let native = id(native);
                    if rows
                        .insert(
                            native,
                            HistoryRange {
                                id: native,
                                end,
                                available,
                                deleted,
                            },
                        )
                        .is_some()
                    {
                        return Err(snapshot_failure("duplicate reconstructed native range"));
                    }
                    Ok(())
                })();
                match result {
                    Ok(()) => ControlFlow::Continue(()),
                    Err(st) => ControlFlow::Break(st),
                }
            })
            .map_err(|_| limited(LimitKind::Ops))?;
        if let ControlFlow::Break(st) = result {
            return Err(st);
        }
        for (client, clock) in snapshot.state_map.iter() {
            b.step(1)?;
            history_covers(&rows, client.get(), 0, *clock, false, b)?;
        }
        // Standard snapshot construction allocates SV/DS metadata only. Charge
        // a conservative per-block tree/vector envelope before that allocation.
        b.own(
            rows.len()
                .checked_mul(128)
                .ok_or_else(|| limited(LimitKind::Memory))?,
        )?;
        b.own(
            summary
                .client_table_capacity
                .checked_mul(std::mem::size_of::<(u64, u32)>() * 4)
                .ok_or_else(|| limited(LimitKind::Memory))?,
        )?;
        let actual = txn.snapshot();
        b.step(
            actual
                .state_map
                .table_capacity()
                .saturating_add(snapshot.state_map.table_capacity()),
        )?;
        for set in [&actual.delete_set, &snapshot.delete_set] {
            b.step(set.len())?;
            for (_, ranges) in set.iter() {
                b.step(ranges.len())?;
            }
        }
        b.tick()?;
        // Standard decode preserves raw adjacent DS entries; the actual store
        // snapshot uses standard coalescing insertion. Compare their sets after
        // the same bounded SDK normalization, not physical range segmentation.
        let expected_deletions = normalized_deletions(&snapshot.delete_set, b)?;
        if actual.state_map != snapshot.state_map || actual.delete_set != expected_deletions {
            return Err(snapshot_failure(
                "reconstructed SV/DS differs from saved snapshot",
            ));
        }
        b.tick()
    }
    fn ancestry(
        z: &Record,
        all: &BTreeMap<NativeId, Record>,
        b: &mut Budget,
    ) -> Result<bool, EngineStatus> {
        let Some(mut current) = z.owner.as_ref() else {
            return Ok(false);
        };
        let mut depth = 0;
        loop {
            b.step(1)?;
            match &current.identity {
                Identity::Root(name) => {
                    b.inspect(name.len())?;
                    return Ok(name == crate::FRAGMENT
                        && current.parent == Parent::Root
                        && !current.deleted
                        && current.tag.is_none()
                        && matches!(current.kind, TypeKind::Unknown | TypeKind::XmlFragment));
                }
                Identity::Unknown => return Ok(false),
                Identity::Nested(current_id) => {
                    if depth == b.limits.max_project_depth {
                        return Ok(false);
                    }
                    depth += 1;
                    b.lookup(all.len())?;
                    let Some(node) = all.get(current_id) else {
                        return Ok(false);
                    };
                    let Payload::Type(kind, tag) = &node.payload else {
                        return Ok(false);
                    };
                    b.inspect(
                        tag.as_ref()
                            .map_or(0, String::len)
                            .saturating_add(current.tag.as_ref().map_or(0, String::len)),
                    )?;
                    if *kind != current.kind
                        || *tag != current.tag
                        || node.deleted != Some(current.deleted)
                    {
                        return Ok(false);
                    }
                    let Some(next) = node.owner.as_ref() else {
                        return Ok(false);
                    };
                    let Parent::Nested(expected) = &current.parent else {
                        return Ok(false);
                    };
                    for identity in [expected, &next.identity, &node.parent] {
                        if let Identity::Root(name) = identity {
                            b.inspect(name.len())?;
                        }
                    }
                    if expected != &next.identity || node.parent != next.identity {
                        return Ok(false);
                    }
                    // The declaring Type item's owner is exactly the immediate
                    // parent, never that parent's declaring item's owner.
                    current = next;
                }
            }
        }
    }
    fn one_string<'a>(p: &'a Payload, b: &mut Budget) -> Result<Option<&'a str>, EngineStatus> {
        match p {
            Payload::Values(v) if v.len() == 1 => match &v[0] {
                Any::String(s) => {
                    b.inspect(s.len())?;
                    Ok(Some(s.as_ref()))
                }
                Any::Null | Any::Undefined => Ok(None),
                _ => Ok(None),
            },
            _ => Ok(None),
        }
    }
    fn reference(
        report: &mut NativeArchiveInventory,
        owner: NativeId,
        declaration: NativeId,
        kind: ReferenceKind,
        certainty: ReferenceCertainty,
        value: &str,
        b: &mut Budget,
    ) -> Result<(), EngineStatus> {
        b.step(1)?;
        #[derive(Serialize)]
        struct BorrowedReference<'a> {
            owner: NativeId,
            declaration: NativeId,
            kind: ReferenceKind,
            certainty: ReferenceCertainty,
            value: &'a str,
        }
        b.report_item(&BorrowedReference {
            owner,
            declaration,
            kind,
            certainty,
            value,
        })?;
        b.report_reserve(&mut report.references)?;
        let value = b.report_string(value)?;
        report.references.push(RetainedReference {
            owner,
            declaration,
            kind,
            certainty,
            value,
        });
        Ok(())
    }
    fn classify(
        z: &Record,
        all: &BTreeMap<NativeId, Record>,
        kinds: &BTreeMap<NativeId, u8>,
        report: &mut NativeArchiveInventory,
        b: &mut Budget,
    ) -> Result<(), EngineStatus> {
        b.step(1)?;
        if matches!(z.payload, Payload::Gc) {
            return Ok(());
        }
        if matches!(z.payload, Payload::Skip | Payload::Subdocument) {
            return diagnostic(report, IncompleteReason::Skip, Some(z.id), b);
        }
        if z.deleted.is_none() {
            return diagnostic(report, IncompleteReason::UnavailableMetadata, Some(z.id), b);
        }
        let Some(owner) = z.owner.as_ref() else {
            return diagnostic(report, IncompleteReason::UnavailableOwner, Some(z.id), b);
        };
        if !ancestry(z, all, b)? {
            diagnostic(report, IncompleteReason::Ancestry, Some(z.id), b)?;
        }
        if !matches!(
            owner.kind,
            TypeKind::XmlElement | TypeKind::XmlText | TypeKind::XmlFragment | TypeKind::Unknown
        ) {
            diagnostic(report, IncompleteReason::UnknownType, Some(z.id), b)?;
        }
        if matches!(
            z.payload,
            Payload::Type(
                TypeKind::Unknown
                    | TypeKind::Array
                    | TypeKind::Map
                    | TypeKind::XmlHook
                    | TypeKind::Subdocument
                    | TypeKind::WeakLink,
                _
            )
        ) {
            diagnostic(report, IncompleteReason::UnknownType, Some(z.id), b)?;
        }
        if matches!(owner.kind, TypeKind::Unknown | TypeKind::XmlFragment)
            && !matches!(&owner.identity,Identity::Root(s) if s==crate::FRAGMENT)
        {
            diagnostic(report, IncompleteReason::UnknownType, Some(z.id), b)?;
        }
        if let Payload::Type(kind, tag) = &z.payload {
            if *kind == TypeKind::XmlFragment
                || (*kind == TypeKind::XmlElement
                    && tag.as_ref().is_none_or(|tag| {
                        !crate::seed::SCHEMA_NODES.iter().any(|(s, _)| *s == tag)
                    }))
            {
                diagnostic(report, IncompleteReason::UnknownType, Some(z.id), b)?;
            }
        }
        let tag = owner.tag.as_deref();
        if let Some(tag) = tag {
            if !crate::seed::SCHEMA_NODES.iter().any(|(s, _)| *s == tag) {
                diagnostic(report, IncompleteReason::UnknownType, Some(z.id), b)?;
            }
        }
        if let Payload::Format(key, value) = &z.payload {
            let mark = crate::project::yattr2markname(key);
            b.step(crate::seed::SCHEMA_MARKS.len())?;
            if !crate::seed::SCHEMA_MARKS.iter().any(|(s, _)| *s == mark) {
                diagnostic(report, IncompleteReason::UnknownSchemaSlot, Some(z.id), b)?;
            }
            if mark == "link" {
                if let Any::Map(fields) = value {
                    b.step(fields.capacity())?;
                    if let Some(Any::String(href)) = fields.get("href") {
                        let owner_id = match owner.identity {
                            Identity::Nested(v) => v,
                            _ => z.id,
                        };
                        reference(
                            report,
                            owner_id,
                            z.id,
                            ReferenceKind::LinkHref,
                            ReferenceCertainty::FixedKind,
                            href,
                            b,
                        )?;
                    }
                }
            }
        }
        let Some(key) = z.key.as_deref() else {
            return Ok(());
        };
        let Some(tag) = tag else {
            return diagnostic(report, IncompleteReason::UnknownSchemaSlot, Some(z.id), b);
        };
        let Some((_, attrs)) = crate::seed::SCHEMA_NODES.iter().find(|(s, _)| *s == tag) else {
            return Ok(());
        };
        b.step(attrs.len())?;
        if !attrs.iter().any(|(s, _)| *s == key) {
            return diagnostic(report, IncompleteReason::UnknownSchemaSlot, Some(z.id), b);
        }
        if !matches!(
            (tag, key),
            ("mention", "id") | ("embed", "ref") | ("attachment", "id")
        ) {
            return Ok(());
        }
        let Some(value) = one_string(&z.payload, b)? else {
            return diagnostic(
                report,
                IncompleteReason::UnavailableClassifier,
                Some(z.id),
                b,
            );
        };
        let Identity::Nested(owner_id) = owner.identity else {
            return diagnostic(report, IncompleteReason::UnavailableOwner, Some(z.id), b);
        };
        if tag == "attachment" {
            return reference(
                report,
                owner_id,
                z.id,
                ReferenceKind::Attachment,
                ReferenceCertainty::FixedKind,
                value,
                b,
            );
        }
        b.lookup(kinds.len())?;
        let bits = kinds.get(&owner_id).copied().unwrap_or(0);
        if bits == 0 {
            return diagnostic(
                report,
                IncompleteReason::UnavailableClassifier,
                Some(z.id),
                b,
            );
        }
        if bits & 4 != 0 {
            diagnostic(
                report,
                IncompleteReason::UnsupportedReferenceKind,
                Some(z.id),
                b,
            )?;
        }
        let certainty = if bits & 3 == 3 {
            ReferenceCertainty::Potential
        } else {
            ReferenceCertainty::FixedKind
        };
        if bits & 1 != 0 {
            reference(
                report,
                owner_id,
                z.id,
                ReferenceKind::Document,
                certainty,
                value,
                b,
            )?;
        }
        if bits & 2 != 0 {
            reference(
                report,
                owner_id,
                z.id,
                ReferenceKind::Task,
                certainty,
                value,
                b,
            )?;
        }
        Ok(())
    }
    /// Diagnostic cost witness (run explicitly): measures what the archive
    /// inventory and revision-restore proofs charge for one ordinary near-cap
    /// document under a test-only unbounded budget. It changes no product
    /// limit; it attributes the totals to phases and to charge shapes.
    #[cfg(test)]
    mod cost_witness {
        use super::*;
        use std::collections::HashMap;
        use yrs::updates::decoder::Decode;
        use yrs::updates::encoder::{Encoder, EncoderV1};
        use yrs::Transact;

        #[derive(Default, Debug)]
        struct Tally {
            calls: usize,
            work: usize,
            inspected: usize,
            owned: usize,
            shapes: HashMap<(usize, usize, usize), usize>,
        }
        impl Tally {
            fn add(&mut self, c: &retained::WitnessCost) -> bool {
                self.calls += 1;
                self.work += c.work;
                self.inspected += c.inspected_bytes;
                self.owned += c.owned_bytes;
                *self
                    .shapes
                    .entry((c.work, c.inspected_bytes, c.owned_bytes))
                    .or_default() += 1;
                true
            }
            fn top(&self, by: fn(&(usize, usize, usize)) -> usize) -> Vec<String> {
                let mut v: Vec<_> = self.shapes.iter().collect();
                v.sort_by_key(|(shape, count)| std::cmp::Reverse(by(shape) * **count));
                v.into_iter()
                    .take(6)
                    .map(|((w, i, o), n)| format!("{n}x(work {w}, inspected {i}, owned {o})"))
                    .collect()
            }
            fn line(&self, phase: &str) -> String {
                format!(
                    "{phase}: calls {} work {} inspected {} owned {}; top owned {:?}; top work {:?}",
                    self.calls,
                    self.work,
                    self.inspected,
                    self.owned,
                    self.top(|s| s.2),
                    self.top(|s| s.0)
                )
            }
        }
        fn unbounded() -> Limits {
            Limits {
                max_load_bytes: u64::MAX / 16,
                max_output_bytes: u64::MAX / 16,
                max_project_nodes: u32::MAX,
                max_project_json_bytes: u64::MAX / 16,
                ..Limits::default()
            }
        }
        fn work(b: &Budget) -> String {
            format!(
                "steps {} inspected {} owned {} blocks {}",
                b.work.steps, b.work.inspected_bytes, b.work.owned_bytes, b.work.blocks
            )
        }

        #[test]
        #[ignore = "diagnostic cost witness; run explicitly with --ignored --nocapture"]
        fn native_archive_near_cap_cost_witness() {
            let paragraphs: Vec<serde_json::Value> = (0..850)
                .map(|i| serde_json::json!({"type":"paragraph","content":[{"type":"text","text":format!("{i:04}{}", "a".repeat(996))}]}))
                .collect();
            let body = serde_json::json!({"type":"doc","content":paragraphs});
            let bytes = crate::seed::tiptap_to_yjs_update(&body, &Limits::default()).unwrap();
            let update = Update::decode_v1(&bytes).unwrap();
            let mut entries = 0usize;
            let mut text_bytes = 0usize;
            let completion =
                retained::visit_input(&update, visit_limits(&Budget::new(unbounded())), |e| {
                    entries += 1;
                    if let InputEntry::Item {
                        content: RetainedContent::String(s),
                        ..
                    } = e
                    {
                        text_bytes += s.len();
                    }
                    ControlFlow::<()>::Continue(())
                })
                .unwrap();
            assert_eq!(completion, std::ops::ControlFlow::Continue(()));
            let defaults = Limits::default();
            println!(
                "corpus: update {} bytes, input entries {entries}, text {text_bytes} bytes; product budget owned<={} inspected<={} steps<={}",
                bytes.len(),
                defaults.max_load_bytes,
                defaults.max_load_bytes * 4,
                defaults.max_project_nodes
            );

            // Phase 1: owned input witness capture alone.
            let wl = witness_limits(&Budget::new(unbounded()));
            let mut capture = Tally::default();
            let witness = retained::capture_owned_input(&update, wl, &mut |c| capture.add(&c));
            assert!(witness.is_ok(), "{:?}", witness.err());
            println!("{}", capture.line("capture_owned_input"));

            // Phase 2: the full ledger capture (witness + input visit) in a Budget.
            let mut b = Budget::new(unbounded());
            let mut ledger = InputLedger::new();
            ledger.capture(&update, &mut b).unwrap();
            println!("ledger.capture budget: {}", work(&b));

            // Phase 3: witness verification against the applied store.
            let doc = crate::engine::new_doc();
            doc.transact_mut()
                .apply_update(Update::decode_v1(&bytes).unwrap())
                .unwrap();
            let mut verify = Tally::default();
            {
                let txn = doc.transact();
                let mut l2 = InputLedger::new();
                let mut b2 = Budget::new(unbounded());
                l2.capture(&update, &mut b2).unwrap();
                let proof = retained::verify_owned_witnesses(
                    &txn,
                    std::mem::take(&mut l2.witnesses),
                    wl,
                    &mut |c| verify.add(&c),
                );
                assert!(proof.is_ok(), "{:?}", proof.err());
            }
            println!("{}", verify.line("verify_owned_witnesses"));

            // Phase 4: the whole inventory (verify + classification) after capture.
            {
                let txn = doc.transact();
                let before = work(&b);
                let report = inventory(&txn, ledger, &"0".repeat(64), &mut b).unwrap();
                println!(
                    "inventory budget: before [{before}] after [{}]; report complete {} refs {}",
                    work(&b),
                    report.complete,
                    report.references.len()
                );
            }

            // Phase 5: revision restore proofs for this state's own snapshot.
            let snapshot = doc.transact().snapshot();
            let mut b3 = Budget::new(unbounded());
            prove_snapshot(&doc.transact(), &snapshot, &mut b3).unwrap();
            println!("prove_snapshot budget: {}", work(&b3));
            let complete = doc
                .transact()
                .encode_state_as_update_v1(&yrs::StateVector::default());
            b3.delegated_native_bytes(complete.len()).unwrap();
            let scratch = crate::engine::new_doc();
            scratch
                .transact_mut()
                .apply_update(Update::decode_v1(&complete).unwrap())
                .unwrap();
            let rebuilt = {
                let mut txn = scratch.transact_mut();
                txn.materialize_snapshot(&snapshot);
                let mut encoder = EncoderV1::new();
                txn.encode_state_from_snapshot(&snapshot, &mut encoder)
                    .unwrap();
                encoder.to_vec()
            };
            b3.delegated_native_bytes(rebuilt.len()).unwrap();
            let reconstructed = crate::engine::new_doc();
            reconstructed
                .transact_mut()
                .apply_update(Update::decode_v1(&rebuilt).unwrap())
                .unwrap();
            let before = work(&b3);
            prove_reconstruction(&reconstructed.transact(), &snapshot, &mut b3).unwrap();
            println!(
                "restore: complete {} bytes, rebuilt {} bytes; prove_reconstruction budget before [{before}] after [{}]",
                complete.len(),
                rebuilt.len(),
                work(&b3)
            );
        }
    }

    #[cfg(test)]
    mod regression_tests {
        use super::*;
        fn empty_report(b: &mut Budget) -> NativeArchiveInventory {
            let binding = "0000000000000000000000000000000000000000000000000000000000000000";
            b.start_report(binding).unwrap();
            NativeArchiveInventory {
                binding: b.string(binding).unwrap(),
                schema_version: 1,
                complete: true,
                references: vec![],
                unavailable: vec![],
                diagnostics: vec![],
                work: InventoryWork::default(),
            }
        }
        fn append(
            report: &mut NativeArchiveInventory,
            value: &str,
            b: &mut Budget,
        ) -> Result<(), EngineStatus> {
            reference(
                report,
                NativeId {
                    client: 10,
                    clock: 0,
                },
                NativeId {
                    client: 10,
                    clock: 2,
                },
                ReferenceKind::LinkHref,
                ReferenceCertainty::FixedKind,
                value,
                b,
            )
        }
        #[test]
        fn native_archive_report_large_and_escaping_values_fail_before_field_storage() {
            for (cap, value) in [
                (1024, "x".repeat(1025)),
                (4096, "\u{0001}".repeat(600)),
                (1_048_576, "x".repeat(1_048_577)),
            ] {
                let mut b = Budget::new(Limits {
                    max_project_json_bytes: cap,
                    ..Limits::default()
                });
                let mut report = empty_report(&mut b);
                let before = b.report_owned_reserved;
                assert!(matches!(
                    append(&mut report, &value, &mut b),
                    Err(EngineStatus::ResourceLimit {
                        kind: LimitKind::Output,
                        ..
                    })
                ));
                assert_eq!(report.references.capacity(), 0);
                assert_eq!(b.report_owned_reserved, before);
                assert!(b.report_wire_reserved <= cap);
            }
        }
        #[test]
        fn native_archive_report_small_budget_and_cumulative_arrays_stay_bounded() {
            let mut b = Budget::new(Limits {
                max_project_json_bytes: 1024,
                ..Limits::default()
            });
            let mut report = empty_report(&mut b);
            append(&mut report, "https://example.test/한글🙂", &mut b).unwrap();
            bound_report(&report, &mut b).unwrap();
            for mode in 0..3 {
                let cap = 4096;
                let mut b = Budget::new(Limits {
                    max_project_json_bytes: cap,
                    ..Limits::default()
                });
                let mut report = empty_report(&mut b);
                let mut failed = false;
                for clock in 0..200 {
                    let result = match mode {
                        0 => append(&mut report, "https://example.test/a", &mut b),
                        1 => diagnostic(
                            &mut report,
                            IncompleteReason::Ancestry,
                            Some(NativeId { client: 10, clock }),
                            &mut b,
                        ),
                        _ => {
                            let item = UnavailableRange {
                                id: NativeId { client: 10, clock },
                                len: 1,
                                kind: UnavailableKind::Gc,
                            };
                            b.report_item(&item)
                                .and_then(|_| b.report_reserve(&mut report.unavailable))
                                .map(|_| report.unavailable.push(item))
                        }
                    };
                    if let Err(EngineStatus::ResourceLimit {
                        kind: LimitKind::Output,
                        ..
                    }) = result
                    {
                        failed = true;
                        break;
                    }
                    result.unwrap();
                }
                assert!(failed);
                assert!(b.report_owned_reserved <= cap && b.report_wire_reserved <= cap);
                let storage = std::mem::size_of::<NativeArchiveInventory>()
                    + report.binding.len()
                    + report.references.capacity() * std::mem::size_of::<RetainedReference>()
                    + report
                        .references
                        .iter()
                        .map(|r| r.value.len())
                        .sum::<usize>()
                    + report.unavailable.capacity() * std::mem::size_of::<UnavailableRange>()
                    + report.diagnostics.capacity() * std::mem::size_of::<InventoryDiagnostic>();
                assert!(storage as u64 <= b.report_owned_reserved);
            }
        }
        #[test]
        fn native_archive_history_ranges_reject_gap_unknown_gc_and_uncaptured_ds() {
            let mut rows = BTreeMap::new();
            let native = NativeId {
                client: 10,
                clock: 1,
            };
            rows.insert(
                native,
                HistoryRange {
                    id: native,
                    end: 3,
                    available: true,
                    deleted: false,
                },
            );
            assert!(
                history_covers(&rows, 10, 0, 3, false, &mut Budget::new(Limits::default()))
                    .is_err()
            );
            assert!(
                history_covers(&rows, 99, 1, 3, false, &mut Budget::new(Limits::default()))
                    .is_err()
            );
            assert!(
                history_covers(&rows, 10, 1, 3, true, &mut Budget::new(Limits::default())).is_err()
            );
            history_covers(&rows, 10, 1, 3, false, &mut Budget::new(Limits::default())).unwrap();
            rows.get_mut(&native).unwrap().available = false;
            rows.get_mut(&native).unwrap().deleted = true;
            assert!(
                history_covers(&rows, 10, 1, 3, true, &mut Budget::new(Limits::default())).is_err()
            );
        }
        #[test]
        fn native_archive_inventory_actual_gc_reverse_and_forward_loss_controls() {
            use yrs::{updates::decoder::Decode, Doc, Transact};
            // Independent literal native GC client10/clock0/len1.
            let expected = UnavailableRange {
                id: NativeId {
                    client: 10,
                    clock: 0,
                },
                len: 1,
                kind: UnavailableKind::Gc,
            };
            for mode in 0..3 {
                let update = Update::decode_v1(&[1, 1, 10, 0, 0, 1, 0]).unwrap();
                let mut budget = Budget::new(Limits::default());
                let mut ledger = InputLedger::new();
                ledger.capture(&update, &mut budget).unwrap();
                let d = Doc::new();
                d.transact_mut().apply_update(update).unwrap();
                match mode {
                    1 => ledger.records.clear(),
                    2 => ledger.records[0].len = 2,
                    _ => {}
                }
                let report = inventory(
                    &d.transact(),
                    ledger,
                    "0000000000000000000000000000000000000000000000000000000000000000",
                    &mut budget,
                )
                .unwrap();
                assert_eq!(report.unavailable, vec![expected.clone()]);
                assert!(
                    !report.complete,
                    "actual GC cannot prove complete native history"
                );
                assert!(report
                    .diagnostics
                    .iter()
                    .any(|d| d.reason == IncompleteReason::InputContentLoss));
                if mode != 0 {
                    assert!(report
                        .diagnostics
                        .iter()
                        .any(|d| d.reason == IncompleteReason::UnavailableIntervalLoss));
                }
            }
        }
        fn chain(depth: u32, deleted: bool) -> (Record, BTreeMap<NativeId, Record>) {
            let root = Owner {
                identity: Identity::Root(crate::FRAGMENT.into()),
                kind: TypeKind::XmlFragment,
                tag: None,
                parent: Parent::Root,
                deleted: false,
            };
            let mut all = BTreeMap::new();
            let mut parent = root;
            for clock in 0..depth {
                let native = NativeId { client: 10, clock };
                let current = Owner {
                    identity: Identity::Nested(native),
                    kind: TypeKind::XmlElement,
                    tag: Some("blockquote".into()),
                    parent: Parent::Nested(parent.identity.clone()),
                    deleted,
                };
                all.insert(
                    native,
                    Record {
                        id: native,
                        len: 1,
                        parent: parent.identity.clone(),
                        key: None,
                        origin: None,
                        right_origin: None,
                        owner: Some(parent),
                        deleted: Some(deleted),
                        payload: Payload::Type(TypeKind::XmlElement, Some("blockquote".into())),
                    },
                );
                parent = current;
            }
            let z = Record {
                id: NativeId {
                    client: 10,
                    clock: depth,
                },
                len: 1,
                parent: parent.identity.clone(),
                key: None,
                origin: None,
                right_origin: None,
                owner: Some(parent),
                deleted: Some(deleted),
                payload: Payload::Text("한글🙂".into()),
            };
            (z, all)
        }
        #[test]
        fn native_archive_ancestry_each_edge_depth_deleted_and_metadata_controls() {
            for deleted in [false, true] {
                for (cap, depth, expected) in [
                    (3, 3, true),
                    (3, 4, false),
                    (128, 128, true),
                    (128, 129, false),
                ] {
                    let (z, all) = chain(depth, deleted);
                    let mut b = Budget::new(Limits {
                        max_project_depth: cap,
                        ..Limits::default()
                    });
                    assert_eq!(ancestry(&z, &all, &mut b).unwrap(), expected);
                }
            }
            let (z, all) = chain(3, true);
            let first = NativeId {
                client: 10,
                clock: 0,
            };
            let last = NativeId {
                client: 10,
                clock: 2,
            };
            for mode in 0..5 {
                let mut broken = all.clone();
                match mode {
                    0 => {
                        broken.remove(&first);
                    }
                    1 => {
                        broken.get_mut(&last).unwrap().parent = Identity::Root("wrong".into());
                    }
                    2 => {
                        broken.get_mut(&last).unwrap().payload =
                            Payload::Type(TypeKind::XmlText, None);
                    }
                    3 => {
                        broken.get_mut(&last).unwrap().deleted = Some(false);
                    }
                    _ => {
                        // A consistent descriptor cycle 0→2→1→0 must terminate
                        // at the exact edge bound, rather than skip ancestors.
                        let last_owner = z.owner.clone();
                        let node = broken.get_mut(&first).unwrap();
                        node.owner = last_owner;
                        node.parent = Identity::Nested(last);
                        broken
                            .get_mut(&NativeId {
                                client: 10,
                                clock: 1,
                            })
                            .unwrap()
                            .owner
                            .as_mut()
                            .unwrap()
                            .parent = Parent::Nested(Identity::Nested(last));
                    }
                }
                assert!(!ancestry(&z, &broken, &mut Budget::new(Limits::default())).unwrap());
            }
        }
    }
    pub(crate) fn bound_report(
        report: &NativeArchiveInventory,
        budget: &mut Budget,
    ) -> Result<(), EngineStatus> {
        let max = budget.limits.max_project_json_bytes;
        let mut writer = ReportCounter {
            budget,
            count: 0,
            max,
            failure: None,
        };
        let result = serde_json::to_writer(&mut writer, report);
        if let Some(failure) = writer.failure {
            return Err(failure);
        }
        if result.is_err() {
            return Err(limited(LimitKind::Output));
        }
        Ok(())
    }
}

/// Counting serialization bounds the report without allocating a JSON copy.
/// This helper is available to the parent without worker/Yrs features.
pub fn report_wire_fits(report: &NativeArchiveInventory, max: u64) -> bool {
    report_wire_bytes(report, max).is_some()
}
pub fn report_wire_bytes(report: &NativeArchiveInventory, max: u64) -> Option<u64> {
    use std::io::{self, Write};
    struct Counter {
        written: u64,
        max: u64,
    }
    impl Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let total = self
                .written
                .checked_add(bytes.len() as u64)
                .ok_or_else(|| io::Error::other("archive output"))?;
            if total > self.max {
                return Err(io::Error::other("archive output"));
            }
            self.written = total;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter { written: 0, max };
    serde_json::to_writer(&mut counter, report).ok()?;
    Some(counter.written)
}

#[cfg(test)]
mod wire_tests {
    use super::*;
    use crate::{
        limits::Limits,
        outcome::{EngineStatus, LimitKind},
        protocol::{preflight_wire_json, Request},
    };
    const BINDING: &str = "0000000000000000000000000000000000000000000000000000000000000000";
    #[test]
    fn native_archive_wire_caps_apply_before_base64_decode() {
        let limits = Limits {
            max_input_bytes: 16,
            max_output_bytes: 16,
            max_load_bytes: 32,
            ..Limits::default()
        };
        let req = Request::ArchiveLoad {
            snapshot_b64: Some(vec![0; 17]),
            tail_b64: Vec::new(),
            encoding: 1,
            capture_binding: BINDING.into(),
        };
        assert!(matches!(
            req.preflight(&limits),
            Err(EngineStatus::ResourceLimit {
                kind: LimitKind::Input,
                ..
            })
        ));
        let raw = serde_json::json!({"op":"archive_load","capture_binding":BINDING,"snapshot_b64":"AAAAAAAAAAAAAAAAAAAAAAAAAAAA"});
        assert!(matches!(
            preflight_wire_json(&raw, &limits),
            Err(EngineStatus::ResourceLimit {
                kind: LimitKind::Input,
                ..
            })
        ));
        let request = Request::ArchiveRestoreFromSnapshot {
            snap_b64: vec![0; 17],
            encoding: 1,
        };
        assert!(matches!(
            request.preflight(&limits),
            Err(EngineStatus::ResourceLimit {
                kind: LimitKind::Input,
                ..
            })
        ));
        let raw = serde_json::json!({"op":"archive_restore_from_snapshot","snap_b64":"AAAAAAAAAAAAAAAAAAAAAAAAAAAA"});
        assert!(matches!(
            preflight_wire_json(&raw, &limits),
            Err(EngineStatus::ResourceLimit {
                kind: LimitKind::Input,
                ..
            })
        ));
        let raw =
            serde_json::json!({"op":"archive_load","capture_binding":"wrong","snapshot_b64":"!"});
        assert!(matches!(
            preflight_wire_json(&raw, &limits),
            Err(EngineStatus::Malformed { .. })
        ));
    }
    #[test]
    fn native_archive_wire_has_separate_optional_typed_result() {
        let old = crate::outcome::EngineStatus::ping_ok();
        let value = serde_json::to_value(&old).unwrap();
        assert!(value.get("native_archive_inventory").is_none());
        assert_eq!(
            serde_json::from_value::<crate::outcome::EngineStatus>(value).unwrap(),
            old
        );
        let report = NativeArchiveInventory {
            binding: BINDING.into(),
            schema_version: 1,
            complete: false,
            references: Vec::new(),
            unavailable: Vec::new(),
            diagnostics: vec![InventoryDiagnostic {
                reason: IncompleteReason::UnknownInputStructure,
                id: None,
            }],
            work: InventoryWork::default(),
        };
        assert!(!report_wire_fits(&report, 1));
        assert!(report_wire_fits(&report, 1024));
        let expected_report = serde_json::json!({
            "binding": BINDING,
            "schema_version": 1,
            "complete": false,
            "references": [],
            "unavailable": [],
            "diagnostics": [{"reason": "unknown_input_structure", "id": null}],
            "work": {"blocks": 0, "steps": 0, "inspected_bytes": 0, "owned_bytes": 0}
        });
        assert_eq!(serde_json::to_value(&report).unwrap(), expected_report);
        let mut populated = crate::outcome::EngineStatus::ping_ok();
        let crate::outcome::EngineStatus::Ok {
            native_archive_inventory,
            ..
        } = &mut populated
        else {
            panic!("ping result");
        };
        *native_archive_inventory = Some(Box::new(report));
        let wire = serde_json::to_value(&populated).unwrap();
        assert_eq!(wire["native_archive_inventory"], expected_report);
        assert!(wire.get("content_json").is_none());
        assert_eq!(
            serde_json::from_value::<crate::outcome::EngineStatus>(wire).unwrap(),
            populated
        );
        println!(
            "inventory header={} status={} bytes",
            std::mem::size_of::<NativeArchiveInventory>(),
            std::mem::size_of::<crate::outcome::EngineStatus>()
        );
    }
}

#[cfg(all(test, feature = "worker"))]
mod native_tests {
    use super::*;
    use crate::{
        engine::CollabEngine,
        limits::Limits,
        outcome::{EngineStatus, LimitKind},
        protocol::Request,
    };
    use yrs::{
        ClientID, Doc, Options, ReadTxn, StateVector, Text, Transact, Xml, XmlElementPrelim,
        XmlFragment, XmlTextPrelim,
    };
    const BINDING: &str = "0000000000000000000000000000000000000000000000000000000000000000";
    const TARGET: &str = "11111111-1111-4111-8111-111111111111";
    fn doc() -> Doc {
        Doc::with_options(Options {
            client_id: ClientID::new(10),
            skip_gc: true,
            offset_kind: yrs::OffsetKind::Utf16,
            cleanup_formatting: false,
            ..Options::default()
        })
    }
    fn encode(d: &Doc) -> Vec<u8> {
        d.transact()
            .encode_state_as_update_v1(&StateVector::default())
    }
    fn mention(root: &str, key: &str) -> Doc {
        let d = doc();
        let f = d.get_or_insert_xml_fragment(root);
        let mut tx = d.transact_mut();
        let element = f.push_back(
            &mut tx,
            XmlElementPrelim::new("mention", std::iter::empty::<yrs::types::xml::XmlIn>()),
        );
        element.insert_attribute(&mut tx, "entity", "document");
        element.insert_attribute(&mut tx, key, TARGET);
        element.insert_attribute(&mut tx, "label", "한글🙂");
        drop(tx);
        d
    }
    fn snapshot_document() -> (Doc, yrs::XmlTextRef) {
        // Literal native identity/clock contract before any archive operation:
        // client10 paragraph0, XmlText1, "A"2; original snapshot SV=3.
        let d = doc();
        let root = d.get_or_insert_xml_fragment("prosemirror");
        let mut tx = d.transact_mut();
        let paragraph = root.push_back(
            &mut tx,
            XmlElementPrelim::new("paragraph", std::iter::empty::<yrs::types::xml::XmlIn>()),
        );
        let text = paragraph.push_back(&mut tx, XmlTextPrelim::new("A"));
        drop(tx);
        assert_eq!(d.transact().state_vector().get(&ClientID::new(10)), 3);
        (d, text)
    }
    fn snapshot_engine(bytes: Vec<u8>) -> CollabEngine {
        let mut engine = CollabEngine::new(Limits::default());
        assert!(matches!(
            engine.handle(&Request::Load {
                snapshot_b64: Some(bytes),
                tail_b64: vec![],
                encoding: 1
            }),
            EngineStatus::Ok {
                applied: true,
                pending: false,
                ..
            }
        ));
        engine
    }
    #[test]
    fn native_archive_saved_snapshot_literal_history_and_strict_rejections() {
        let valid = vec![0, 1, 10, 3];
        let (d, text) = snapshot_document();
        text.insert(&mut d.transact_mut(), 1, "한글🙂");
        assert_eq!(d.transact().state_vector().get(&ClientID::new(10)), 7);
        let captured = encode(&d);
        let mut engine = snapshot_engine(captured.clone());
        let before = engine.complete_snapshot();
        let restored = engine.handle(&Request::ArchiveRestoreFromSnapshot {
            snap_b64: valid.clone(),
            encoding: 1,
        });
        let EngineStatus::Ok {
            update_b64: Some(update),
            pending: false,
            ..
        } = restored
        else {
            panic!("literal snapshot: {restored:?}");
        };
        let EngineStatus::Ok {
            update_b64: Some(before),
            ..
        } = before
        else {
            panic!("complete before archive restore");
        };
        let EngineStatus::Ok {
            update_b64: Some(after),
            ..
        } = engine.complete_snapshot()
        else {
            panic!("complete after archive restore");
        };
        assert_eq!(
            after, before,
            "archive reconstruction mutated the live source"
        );
        let bytes = crate::b64::decode(&update).unwrap();
        assert!(matches!(
            engine.handle(&Request::Apply {
                update_b64: bytes,
                encoding: 1
            }),
            EngineStatus::Ok { applied: true, .. }
        ));
        let EngineStatus::Ok {
            content_json: Some(content),
            ..
        } = engine.handle(&Request::Project { encoding: 1 })
        else {
            panic!("projection");
        };
        assert_eq!(
            content,
            serde_json::json!({"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"A"}]}]})
        );
        for invalid in [
            vec![],
            vec![0],
            vec![0, 1, 10, 3, 255],
            vec![0, 1, 10, 8],
            vec![0, 1, 99, 1],
            vec![1, 10, 1, 2, 1, 1, 10, 7],
            vec![1, 10, 1, 6, 2, 1, 10, 7],
        ] {
            let mut engine = snapshot_engine(captured.clone());
            assert!(
                matches!(
                    engine.handle(&Request::ArchiveRestoreFromSnapshot {
                        snap_b64: invalid.clone(),
                        encoding: 1
                    }),
                    EngineStatus::Malformed { .. }
                ),
                "invalid snapshot {invalid:?}"
            );
        }
        let mut engine = snapshot_engine(captured);
        assert!(matches!(
            engine.handle(&Request::ArchiveRestoreFromSnapshot {
                snap_b64: vec![0; (Limits::default().max_input_bytes + 1) as usize],
                encoding: 1
            }),
            EngineStatus::ResourceLimit {
                kind: LimitKind::Input,
                ..
            }
        ));
    }
    #[test]
    fn native_archive_saved_snapshot_reconstructed_interval_ds_and_surrogate_controls() {
        use yrs::{updates::decoder::Decode, updates::encoder::Encode, Map};
        let (d, text) = snapshot_document();
        text.insert(&mut d.transact_mut(), 1, "한글🙂");
        let captured = encode(&d);
        let mut engine = snapshot_engine(captured);
        let before = engine.complete_snapshot();
        // Native clocks: paragraph0/text1/A2/한3/글4/emoji5..7. Cut6
        // bisects the emoji; standard materialization must not widen it to7.
        assert!(matches!(
            engine.handle(&Request::ArchiveRestoreFromSnapshot {
                snap_b64: vec![0, 1, 10, 6], encoding: 1,
            }),
            EngineStatus::Malformed { detail } if detail.contains("output exceeds saved snapshot")
        ));
        let EngineStatus::Ok {
            update_b64: Some(before),
            ..
        } = before
        else {
            panic!("before")
        };
        let EngineStatus::Ok {
            update_b64: Some(after),
            ..
        } = engine.complete_snapshot()
        else {
            panic!("after")
        };
        assert_eq!(after, before, "failed cut mutated the live source");

        let (d, _) = snapshot_document();
        let snapshot = yrs::Snapshot::decode_v1(&[0, 1, 10, 3]).unwrap();
        worker::prove_reconstruction(
            &d.transact(),
            &snapshot,
            &mut worker::Budget::new(Limits::default()),
        )
        .unwrap();
        let deleted = yrs::Snapshot::decode_v1(&[1, 10, 1, 2, 1, 1, 10, 3]).unwrap();
        assert!(matches!(
            worker::prove_reconstruction(&d.transact(), &deleted, &mut worker::Budget::new(Limits::default())),
            Err(EngineStatus::Malformed { detail }) if detail.contains("SV/DS differs")
        ));
        // An extra independently named root must fail even though the document
        // fragment still renders exactly A. No aggregate/body-only allowance.
        d.get_or_insert_text("other")
            .insert(&mut d.transact_mut(), 0, "B");
        assert!(matches!(
            worker::prove_reconstruction(&d.transact(), &snapshot, &mut worker::Budget::new(Limits::default())),
            Err(EngineStatus::Malformed { detail }) if detail.contains("output exceeds saved snapshot")
        ));

        // Reuse the prepared standard Skip witness: independent map versions
        // at0/1; public suffix diff carries only clock1 and a Skip at0.
        let source = doc();
        let map = source.get_or_insert_map("metadata");
        map.insert(&mut source.transact_mut(), "a", "A");
        map.insert(&mut source.transact_mut(), "b", "B");
        let cut = StateVector::from_iter([(ClientID::new(10), 1)]);
        let suffix = yrs::diff_updates_v1(&encode(&source), &cut.encode_v1()).unwrap();
        let hole = doc();
        hole.transact_mut()
            .apply_update(yrs::Update::decode_v1(&suffix).unwrap())
            .unwrap();
        assert_eq!(hole.transact().state_vector().get(&ClientID::new(10)), 0);
        assert!(!yrs::retained::summary(&hole.transact()).pending_update);
        let empty = yrs::Snapshot::decode_v1(&[0, 0]).unwrap();
        assert!(matches!(
            worker::prove_reconstruction(&hole.transact(), &empty, &mut worker::Budget::new(Limits::default())),
            Err(EngineStatus::Malformed { detail }) if detail.contains("unavailable reconstructed history")
        ));
    }
    #[test]
    fn native_archive_saved_snapshot_adjacent_ds_semantic_equivalence() {
        use yrs::updates::decoder::Decode;
        // Independently declared paragraph0/text1/A2/B3, both characters
        // actually deleted; canonical DS[2,4), SV4. No rendered-body oracle.
        let (d, text) = snapshot_document();
        text.insert(&mut d.transact_mut(), 1, "B");
        text.remove_range(&mut d.transact_mut(), 0, 2);
        let canonical_bytes = vec![1, 10, 1, 2, 2, 1, 10, 4];
        let adjacent_bytes = vec![1, 10, 2, 2, 1, 3, 1, 1, 10, 4];
        let canonical = yrs::Snapshot::decode_v1(&canonical_bytes).unwrap();
        let adjacent = yrs::Snapshot::decode_v1(&adjacent_bytes).unwrap();
        assert_eq!(d.transact().snapshot(), canonical);
        // This retained control demonstrates why the former raw IdSet Eq
        // refused a valid representation of the same recorded deleted clocks.
        assert_ne!(adjacent.delete_set, canonical.delete_set);
        let captured = encode(&d);
        for bytes in [canonical_bytes, adjacent_bytes] {
            let mut engine = snapshot_engine(captured.clone());
            let EngineStatus::Ok {
                update_b64: Some(before),
                ..
            } = engine.complete_snapshot()
            else {
                panic!("before")
            };
            assert!(matches!(
                engine.handle(&Request::ArchiveRestoreFromSnapshot {
                    snap_b64: bytes,
                    encoding: 1
                }),
                EngineStatus::Ok {
                    update_b64: Some(_),
                    pending: false,
                    ..
                }
            ));
            let EngineStatus::Ok {
                update_b64: Some(after),
                ..
            } = engine.complete_snapshot()
            else {
                panic!("after")
            };
            assert_eq!(before, after);
        }
        worker::prove_reconstruction(
            &d.transact(),
            &adjacent,
            &mut worker::Budget::new(Limits::default()),
        )
        .unwrap();
    }
    #[test]
    fn native_archive_saved_snapshot_adjacent_ds_missing_excess_controls() {
        use yrs::updates::decoder::Decode;
        let (d, text) = snapshot_document();
        text.insert(&mut d.transact_mut(), 1, "B");
        text.remove_range(&mut d.transact_mut(), 0, 2);
        let captured = encode(&d);
        // Missing B's deletion and excess XmlText deletion must still fail.
        for bytes in [
            vec![1, 10, 1, 2, 1, 1, 10, 4],
            vec![1, 10, 1, 1, 3, 1, 10, 4],
        ] {
            let snapshot = yrs::Snapshot::decode_v1(&bytes).unwrap();
            assert!(matches!(
                worker::prove_reconstruction(&d.transact(), &snapshot, &mut worker::Budget::new(Limits::default())),
                Err(EngineStatus::Malformed { detail }) if detail.contains("SV/DS differs")
            ));
        }
        let mut engine = snapshot_engine(captured);
        // Extra deletion of the undeleted text type is invalid captured history.
        assert!(matches!(
            engine.handle(&Request::ArchiveRestoreFromSnapshot { snap_b64: vec![1, 10, 1, 1, 3, 1, 10, 4], encoding: 1 }),
            EngineStatus::Malformed { detail } if detail.contains("uncaptured deletion")
        ));
    }
    #[test]
    fn native_archive_saved_snapshot_deleted_ranges_gc_and_reconstruction_pending() {
        use yrs::{updates::decoder::Decode, XmlOut};
        let (d, text) = snapshot_document();
        text.remove_range(&mut d.transact_mut(), 0, 1);
        let mut engine = snapshot_engine(encode(&d));
        // DS client10 [2,3), SV3: the retained original A was actually deleted.
        assert!(matches!(
            engine.handle(&Request::ArchiveRestoreFromSnapshot {
                snap_b64: vec![1, 10, 1, 2, 1, 1, 10, 3],
                encoding: 1
            }),
            EngineStatus::Ok {
                update_b64: Some(_),
                pending: false,
                ..
            }
        ));
        // Independently authored one-clock GC input, not a missing live text scan.
        let mut engine = snapshot_engine(vec![1, 1, 10, 0, 0, 1, 0]);
        assert!(
            matches!(engine.handle(&Request::ArchiveRestoreFromSnapshot { snap_b64: vec![0,1,10,1], encoding: 1 }),
            EngineStatus::Malformed { detail } if detail.contains("unavailable required"))
        );
        let (d, _) = snapshot_document();
        let options = Options {
            client_id: ClientID::new(11),
            skip_gc: true,
            ..Options::default()
        };
        let peer = Doc::with_options(options);
        peer.transact_mut()
            .apply_update(yrs::Update::decode_v1(&encode(&d)).unwrap())
            .unwrap();
        let root = peer.get_or_insert_xml_fragment("prosemirror");
        let text = {
            let tx = peer.transact();
            let Some(XmlOut::Element(paragraph)) = root.get(&tx, 0) else {
                panic!("paragraph");
            };
            let Some(XmlOut::Text(text)) = paragraph.get(&tx, 0) else {
                panic!("text");
            };
            text
        };
        let before = peer.transact().state_vector();
        text.insert(&mut peer.transact_mut(), 1, "B");
        let delta = peer.transact().encode_state_as_update_v1(&before);
        let mut pending = CollabEngine::new(Limits::default());
        assert!(matches!(
            pending.handle(&Request::Load {
                snapshot_b64: Some(delta),
                tail_b64: vec![],
                encoding: 1
            }),
            EngineStatus::Ok { pending: true, .. }
        ));
        assert!(
            matches!(pending.handle(&Request::ArchiveRestoreFromSnapshot { snap_b64: vec![0,1,11,1], encoding: 1 }),
            EngineStatus::Malformed { detail } if detail.contains("pending native history"))
        );
        let mut engine = snapshot_engine(encode(&peer));
        // Current store has both peers; this historical cut omits the parent/
        // origin client10, so reconstructed client11 must not remain pending.
        assert!(
            matches!(engine.handle(&Request::ArchiveRestoreFromSnapshot { snap_b64: vec![0,1,11,1], encoding: 1 }),
            EngineStatus::Malformed { detail } if detail.contains("pending native history"))
        );
    }
    #[test]
    fn native_archive_inventory_actual_sdk_parent_depth_and_deleted_subtree() {
        for deleted in [false, true] {
            for (cap, depth, expected) in [
                (3, 3, true),
                (3, 4, false),
                (128, 128, true),
                (128, 129, false),
            ] {
                // Independent clock contract before encoding: client10, every
                // nested type occupies one clock; deepest XmlText is depth-1.
                let expected_owner = NativeId {
                    client: 10,
                    clock: depth - 1,
                };
                let d = doc();
                let root = d.get_or_insert_xml_fragment("prosemirror");
                let mut tx = d.transact_mut();
                let mut branch = root.push_back(
                    &mut tx,
                    XmlElementPrelim::new(
                        "blockquote",
                        std::iter::empty::<yrs::types::xml::XmlIn>(),
                    ),
                );
                for _ in 1..depth - 1 {
                    branch = branch.push_back(
                        &mut tx,
                        XmlElementPrelim::new(
                            "blockquote",
                            std::iter::empty::<yrs::types::xml::XmlIn>(),
                        ),
                    );
                }
                branch.push_back(&mut tx, XmlTextPrelim::new("한글🙂"));
                drop(tx);
                let mut seen = false;
                let completion = d
                    .transact()
                    .visit_retained(yrs::retained::VisitLimits::default(), |event| {
                        if let yrs::retained::RetainedEvent::Block(
                            yrs::retained::RetainedEntry::Item {
                                owner: yrs::retained::OwnerView::Resolved(owner),
                                content: yrs::retained::RetainedContent::String("한글🙂"),
                                ..
                            },
                        ) = event
                        {
                            assert_eq!(
                                owner.id,
                                yrs::retained::BranchIdentity::Nested(yrs::ID::new(
                                    ClientID::new(expected_owner.client),
                                    expected_owner.clock
                                ))
                            );
                            seen = true;
                        }
                        std::ops::ControlFlow::<()>::Continue(())
                    })
                    .unwrap();
                assert_eq!(completion, std::ops::ControlFlow::Continue(()));
                assert!(seen);
                let baseline = encode(&d);
                let mut tails = vec![];
                if deleted {
                    let sv = d.transact().state_vector();
                    root.remove_range(&mut d.transact_mut(), 0, 1);
                    tails.push(d.transact().encode_state_as_update_v1(&sv));
                }
                let mut engine = CollabEngine::new(Limits {
                    max_project_depth: cap,
                    ..Limits::default()
                });
                let outcome = engine.handle(&Request::ArchiveLoad {
                    snapshot_b64: Some(baseline),
                    tail_b64: tails,
                    encoding: 1,
                    capture_binding: BINDING.into(),
                });
                let EngineStatus::Ok {
                    native_archive_inventory: Some(report),
                    ..
                } = outcome
                else {
                    panic!("depth fixture outcome: {outcome:?}");
                };
                assert_eq!(
                    report.complete, expected,
                    "deleted={deleted} depth={depth} {:?}",
                    report.diagnostics
                );
                if !expected {
                    assert!(report
                        .diagnostics
                        .iter()
                        .any(|d| d.reason == IncompleteReason::Ancestry));
                }
            }
        }
    }
    fn load(snapshot: Vec<u8>, tail: Vec<Vec<u8>>) -> NativeArchiveInventory {
        let mut engine = CollabEngine::new(Limits::default());
        match engine.handle(&Request::ArchiveLoad {
            snapshot_b64: Some(snapshot),
            tail_b64: tail,
            encoding: 1,
            capture_binding: BINDING.into(),
        }) {
            EngineStatus::Ok {
                applied: true,
                pending: false,
                native_archive_inventory: Some(v),
                ..
            } => *v,
            other => panic!("expected typed inventory, got {other:?}"),
        }
    }
    #[test]
    fn native_archive_inventory_known_owner_literal_ref_and_identical_replay() {
        let d = mention("prosemirror", "id");
        let bytes = encode(&d);
        let report = load(bytes.clone(), vec![bytes]);
        assert!(report.complete, "{:?}", report.diagnostics);
        assert_eq!(report.binding, BINDING);
        assert_eq!(
            report.references,
            vec![RetainedReference {
                owner: NativeId {
                    client: 10,
                    clock: 0
                },
                declaration: NativeId {
                    client: 10,
                    clock: 2
                },
                kind: ReferenceKind::Document,
                certainty: ReferenceCertainty::FixedKind,
                value: TARGET.into()
            }]
        );
    }
    #[test]
    fn native_archive_inventory_equal_payload_changed_root_and_key_are_incomplete() {
        let d = mention("prosemirror", "id");
        for forged in [mention("foreign-root", "id"), mention("prosemirror", "ref")] {
            let report = load(encode(&d), vec![encode(&forged)]);
            assert!(!report.complete);
            assert!(report
                .diagnostics
                .iter()
                .any(|d| d.reason == IncompleteReason::StructuralMismatch));
        }
    }
    #[test]
    fn native_archive_inventory_unknown_normalized_tail_is_proved() {
        let d = doc();
        let f = d.get_or_insert_xml_fragment("prosemirror");
        let text = f.push_back(&mut d.transact_mut(), XmlTextPrelim::new("한글🙂"));
        let baseline = encode(&d);
        let cut = d.transact().state_vector();
        text.insert(&mut d.transact_mut(), 4, " 끝🧪");
        let tail = d.transact().encode_state_as_update_v1(&cut);
        let report = load(baseline, vec![tail]);
        assert!(report.complete, "{:?}", report.diagnostics);
        assert!(report.diagnostics.is_empty());
        // This proves retained native intervals, not acceptance of a user
        // archive without the separate Project/schema/authorization checks.
    }
    #[test]
    fn native_archive_inventory_ordinary_paragraph_unicode_marks_deletes_complete() {
        // Literal paragraph10:0, XmlText10:1, deleted retainedA10:2,
        // Korean/emoji2..7, appended7..11 and Format11/12 before output.
        let d = doc();
        let root = d.get_or_insert_xml_fragment("prosemirror");
        let paragraph = root.push_back(&mut d.transact_mut(), XmlElementPrelim::empty("paragraph"));
        let text = paragraph.push_back(&mut d.transact_mut(), XmlTextPrelim::new("A한글🙂"));
        let baseline = encode(&d);
        let cut = d.transact().state_vector();
        text.insert(&mut d.transact_mut(), 5, " 끝🧪");
        text.format(
            &mut d.transact_mut(),
            1,
            2,
            std::collections::HashMap::from([("bold".into(), yrs::Any::Bool(true))]),
        );
        text.remove_range(&mut d.transact_mut(), 0, 1);
        {
            use std::ops::ControlFlow;
            use yrs::retained::{RetainedContent, RetainedEntry, RetainedEvent, VisitLimits};
            let tx = d.transact();
            let mut literal_a = false;
            let mut format = Vec::new();
            let completion = tx
                .visit_retained(VisitLimits::default(), |event| {
                    if let RetainedEvent::Block(RetainedEntry::Item {
                        id,
                        deleted,
                        content,
                        ..
                    }) = event
                    {
                        if id.client.get() == 10 && id.clock == 2 {
                            literal_a = deleted && matches!(content, RetainedContent::String("A"));
                        }
                        if let RetainedContent::Format { key: "bold", value } = content {
                            format.push((id.clock, value.clone()));
                        }
                    }
                    ControlFlow::<()>::Continue(())
                })
                .unwrap();
            assert_eq!(completion, std::ops::ControlFlow::Continue(()));
            assert!(literal_a);
            assert_eq!(
                format,
                vec![(11, yrs::Any::Bool(true)), (12, yrs::Any::Null)]
            );
        }
        let tail = d.transact().encode_state_as_update_v1(&cut);
        let before = encode(&d);
        let report = load(baseline, vec![tail]);
        assert!(report.complete, "{:?}", report.diagnostics);
        assert!(report.diagnostics.is_empty() && report.unavailable.is_empty());
        assert_eq!(encode(&d), before);
    }
    #[test]
    fn native_archive_inventory_child_fence_and_scratch_budget() {
        let d = mention("prosemirror", "id");
        let bytes = encode(&d);
        let req = Request::ArchiveLoad {
            snapshot_b64: Some(bytes.clone()),
            tail_b64: Vec::new(),
            encoding: 1,
            capture_binding: BINDING.into(),
        };
        let mut engine = CollabEngine::new(Limits::default());
        assert!(matches!(engine.handle(&req), EngineStatus::Ok { .. }));
        assert!(matches!(
            engine.handle(&req),
            EngineStatus::Malformed { .. }
        ));
        let mut small = CollabEngine::new(Limits {
            max_project_nodes: 1,
            ..Limits::default()
        });
        assert!(matches!(
            small.handle(&req),
            EngineStatus::ResourceLimit {
                kind: LimitKind::Ops | LimitKind::Memory,
                ..
            }
        ));
    }
    #[test]
    fn native_archive_inventory_rejects_trailing_native_bytes() {
        let d = mention("prosemirror", "id");
        let mut bytes = encode(&d);
        bytes.push(0);
        let mut engine = CollabEngine::new(Limits::default());
        let req = Request::ArchiveLoad {
            snapshot_b64: Some(bytes),
            tail_b64: Vec::new(),
            encoding: 1,
            capture_binding: BINDING.into(),
        };
        assert!(matches!(
            engine.handle(&req),
            EngineStatus::Malformed { .. }
        ));
    }
}
