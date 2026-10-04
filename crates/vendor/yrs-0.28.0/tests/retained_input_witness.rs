//! External-policy witness using only public visitor views. This validates
//! available content and exact exposed metadata for represented unsplit items.
//! Unknown/inherited/split metadata is explicitly unproved. This is not archive
//! semantic closure, authorized capture, historical pairing or Complete.
use crate::retained::*;
use crate::{Any, Doc, ReadTxn, Transact, Update, ID};
use std::ops::ControlFlow;

#[derive(Clone, Debug, PartialEq)]
enum Payload {
    Text(Vec<u16>),
    Values(Vec<Any>),
    Json(Vec<String>),
    Binary(Vec<u8>),
    Embed(Any),
    Format(String, Any),
    Type(TypeKind, Option<String>),
}
#[derive(Clone, Debug, PartialEq)]
enum Identity {
    Root(String),
    Nested(ID),
    Unknown,
}
#[derive(Clone, Debug, PartialEq)]
enum OwnedParent {
    Root,
    Nested(Identity),
    Unavailable,
}
#[derive(Clone, Debug, PartialEq)]
struct Owner {
    kind: TypeKind,
    xml_tag: Option<String>,
    parent: OwnedParent,
    deleted: bool,
}
#[derive(Clone, Debug, PartialEq)]
struct Structure {
    parent: Identity,
    map_key: Option<String>,
    origin: Option<ID>,
    right_origin: Option<ID>,
    // Input exposes no resolved owner descriptor. Retain it on loaded items,
    // but do not invent an input owner/ancestry proof from this observation.
    owner: Option<Owner>,
}
fn string(s: &str, b: &mut Budget) -> Result<String, &'static str> {
    b.charge(s.len())?;
    Ok(s.to_owned())
}
fn identity(i: BranchIdentity<'_>, b: &mut Budget) -> Result<Identity, &'static str> {
    Ok(match i {
        BranchIdentity::Root(s) => Identity::Root(string(s, b)?),
        BranchIdentity::Nested(id) => Identity::Nested(id),
    })
}
fn input_structure(
    parent: InputParent<'_>,
    map_key: Option<&str>,
    origin: Option<ID>,
    right_origin: Option<ID>,
    b: &mut Budget,
) -> Result<Structure, &'static str> {
    b.charge(1)?;
    Ok(Structure {
        parent: match parent {
            InputParent::Named(s) => Identity::Root(string(s, b)?),
            InputParent::Id(id) => Identity::Nested(id),
            InputParent::Branch(i) => identity(i, b)?,
            InputParent::Unknown => Identity::Unknown,
        },
        map_key: map_key.map(|s| string(s, b)).transpose()?,
        origin,
        right_origin,
        owner: None,
    })
}
fn loaded_structure(
    owner: OwnerView<'_>,
    map_key: Option<&str>,
    origin: Option<ID>,
    right_origin: Option<ID>,
    b: &mut Budget,
) -> Result<Structure, &'static str> {
    b.charge(1)?;
    let (parent, owner) = match owner {
        OwnerView::Unavailable => (Identity::Unknown, None),
        OwnerView::Resolved(v) => (
            identity(v.id, b)?,
            Some(Owner {
                kind: v.kind,
                xml_tag: v.xml_tag.map(|s| string(s, b)).transpose()?,
                parent: match v.parent {
                    ParentView::Root => OwnedParent::Root,
                    ParentView::Nested(i) => OwnedParent::Nested(identity(i, b)?),
                    ParentView::Unavailable => OwnedParent::Unavailable,
                },
                deleted: v.deleted,
            }),
        ),
    };
    Ok(Structure {
        parent,
        map_key: map_key.map(|s| string(s, b)).transpose()?,
        origin,
        right_origin,
        owner,
    })
}
#[derive(Clone, Debug)]
struct Record {
    id: ID,
    len: u32,
    payload: Payload,
    structure: Structure,
    deleted: Option<bool>,
}
#[derive(Default, Debug)]
struct Inventory {
    available: Vec<Record>,
    unavailable: Vec<(ID, u32)>,
    // Deleted Items have exposed metadata even when their original payload is
    // unavailable. GC has only an interval; keep those distinctions observable.
    unavailable_items: Vec<(ID, Structure, Option<bool>)>,
}
struct Budget(usize);
impl Budget {
    fn charge(&mut self, n: usize) -> Result<(), &'static str> {
        self.0 = self.0.checked_add(n).ok_or("witness-budget")?;
        if self.0 > 4096 {
            return Err("witness-budget");
        }
        Ok(())
    }
    fn any(&mut self, v: &Any, depth: usize) -> Result<(), &'static str> {
        if depth > 16 {
            return Err("witness-depth");
        }
        self.charge(1)?;
        match v {
            Any::String(s) => self.charge(s.len()),
            Any::Buffer(b) => self.charge(b.len()),
            Any::Array(v) => {
                self.charge(v.len())?;
                for x in v.iter() {
                    self.any(x, depth + 1)?;
                }
                Ok(())
            }
            Any::Map(m) => {
                self.charge(m.capacity())?;
                for (k, v) in m.iter() {
                    self.charge(k.len())?;
                    self.any(v, depth + 1)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}
fn copy(c: RetainedContent<'_>, b: &mut Budget) -> Result<Option<Payload>, &'static str> {
    b.charge(1)?;
    Ok(Some(match c {
        RetainedContent::String(s) => {
            b.charge(s.len().checked_mul(3).ok_or("witness-budget")?)?;
            Payload::Text(s.encode_utf16().collect())
        }
        RetainedContent::Values(v) => {
            b.charge(v.len())?;
            for x in v {
                b.any(x, 0)?;
            }
            Payload::Values(v.to_vec())
        }
        RetainedContent::JsonValues(v) => {
            b.charge(v.len())?;
            for s in v {
                b.charge(s.len())?;
            }
            Payload::Json(v.to_vec())
        }
        RetainedContent::Binary(v) => {
            b.charge(v.len())?;
            Payload::Binary(v.to_vec())
        }
        RetainedContent::Embed(v) => {
            b.any(v, 0)?;
            Payload::Embed(v.clone())
        }
        RetainedContent::Format { key, value } => {
            b.charge(key.len())?;
            b.any(value, 0)?;
            Payload::Format(key.to_owned(), value.clone())
        }
        RetainedContent::Type { kind, xml_tag } => {
            b.charge(xml_tag.map_or(0, str::len))?;
            Payload::Type(kind, xml_tag.map(str::to_owned))
        }
        RetainedContent::Deleted { .. } => return Ok(None),
        RetainedContent::Subdocument => return Err("unsupported-input-identity"),
    }))
}
fn input(update: &Update, b: &mut Budget) -> Result<Inventory, &'static str> {
    let mut inventory = Inventory::default();
    let r = visit_input(
        update,
        VisitLimits {
            max_blocks: 128,
            ..VisitLimits::default()
        },
        |e| {
            let result = (|| {
                b.charge(1)?;
                match e {
                    InputEntry::Item {
                        id,
                        native_clock_len,
                        content,
                        parent,
                        map_key,
                        origin,
                        right_origin,
                    } => {
                        let structure = input_structure(parent, map_key, origin, right_origin, b)?;
                        if let Some(payload) = copy(content, b)? {
                            inventory.available.push(Record {
                                id,
                                len: native_clock_len,
                                payload,
                                structure,
                                deleted: None,
                            });
                        } else {
                            inventory.unavailable_items.push((id, structure, None));
                            inventory.unavailable.push((id, native_clock_len));
                        }
                    }
                    InputEntry::Gc {
                        id,
                        native_clock_len,
                    } => inventory.unavailable.push((id, native_clock_len)),
                    InputEntry::Skip { .. } => return Err("input-skip"),
                }
                Ok(())
            })();
            match result {
                Ok(()) => ControlFlow::Continue(()),
                Err(e) => ControlFlow::Break(e),
            }
        },
    )
    .map_err(|_| "input-visitor")?;
    match r {
        ControlFlow::Continue(()) => Ok(inventory),
        ControlFlow::Break(e) => Err(e),
    }
}
fn integrated(doc: &Doc, b: &mut Budget) -> Result<Inventory, &'static str> {
    let tx = doc.transact();
    let s = summary(&tx);
    if s.pending_update || s.pending_deletions {
        return Err("pending");
    }
    let mut inventory = Inventory::default();
    let r = tx
        .visit_retained(
            VisitLimits {
                max_blocks: 128,
                ..VisitLimits::default()
            },
            |e| {
                let result = (|| {
                    b.charge(1)?;
                    match e {
                        RetainedEvent::Block(RetainedEntry::Item {
                            id,
                            native_clock_len,
                            content,
                            owner,
                            map_key,
                            origin,
                            right_origin,
                            deleted,
                        }) => {
                            let structure =
                                loaded_structure(owner, map_key, origin, right_origin, b)?;
                            if let Some(payload) = copy(content, b)? {
                                inventory.available.push(Record {
                                    id,
                                    len: native_clock_len,
                                    payload,
                                    structure,
                                    deleted: Some(deleted),
                                });
                            } else {
                                inventory
                                    .unavailable_items
                                    .push((id, structure, Some(deleted)));
                                inventory.unavailable.push((id, native_clock_len));
                            }
                        }
                        RetainedEvent::Block(RetainedEntry::Gc {
                            id,
                            native_clock_len,
                        }) => inventory.unavailable.push((id, native_clock_len)),
                        RetainedEvent::Block(RetainedEntry::Skip { .. }) => {
                            return Err("integrated-skip")
                        }
                        RetainedEvent::Root(_) => {}
                    }
                    Ok(())
                })();
                match result {
                    Ok(()) => ControlFlow::Continue(()),
                    Err(e) => ControlFlow::Break(e),
                }
            },
        )
        .map_err(|_| "integrated-visitor")?;
    match r {
        ControlFlow::Continue(()) => Ok(inventory),
        ControlFlow::Break(e) => Err(e),
    }
}
fn slice_equal(a: &Record, a_offset: u32, z: &Record, z_offset: u32, len: u32) -> bool {
    let ao = a_offset as usize;
    let zo = z_offset as usize;
    let n = len as usize;
    match (&a.payload, &z.payload) {
        (Payload::Text(a), Payload::Text(z)) => {
            a.get(ao..ao + n) == z.get(zo..zo + n) && a.get(ao..ao + n).is_some()
        }
        (Payload::Values(a), Payload::Values(z)) => {
            a.get(ao..ao + n) == z.get(zo..zo + n) && a.get(ao..ao + n).is_some()
        }
        (Payload::Json(a), Payload::Json(z)) => {
            a.get(ao..ao + n) == z.get(zo..zo + n) && a.get(ao..ao + n).is_some()
        }
        _ => {
            a_offset == 0 && z_offset == 0 && a.len == len && z.len == len && a.payload == z.payload
        }
    }
}
fn prove_available(
    input: &Inventory,
    integrated: &Inventory,
    b: &mut Budget,
) -> Result<(), &'static str> {
    for a in &input.available {
        let mut clock = a.id.clock;
        let end = clock.checked_add(a.len).ok_or("range-overflow")?;
        while clock < end {
            let mut matched = None;
            for z in &integrated.available {
                b.charge(1)?;
                let ze = z.id.clock.checked_add(z.len).ok_or("range-overflow")?;
                if a.id.client == z.id.client && z.id.clock <= clock && clock < ze {
                    if matched.is_some() {
                        return Err("ambiguous-input-identity");
                    }
                    matched = Some((z, ze));
                }
            }
            let (z, ze) = matched.ok_or("input-to-load-loss")?;
            let n = end.min(ze) - clock;
            b.charge(n as usize)?;
            if !slice_equal(a, clock - a.id.clock, z, clock - z.id.clock, n) {
                return Err("input-content-mismatch");
            }
            clock += n;
        }
    }
    // Known unavailable input is not a reason to reject by itself, but every
    // clock must still be present as unavailable in the loaded store.
    for (id, len) in &input.unavailable {
        let mut clock = id.clock;
        let end = clock.checked_add(*len).ok_or("range-overflow")?;
        while clock < end {
            let mut covered = None;
            for (z, zlen) in &integrated.unavailable {
                b.charge(1)?;
                let ze = z.clock.checked_add(*zlen).ok_or("range-overflow")?;
                if id.client == z.client && z.clock <= clock && clock < ze {
                    if covered.is_some() {
                        return Err("ambiguous-unavailable-interval");
                    }
                    covered = Some(ze);
                }
            }
            clock = end.min(covered.ok_or("unavailable-interval-loss")?);
        }
    }
    Ok(())
}
fn prove(input: &Inventory, loaded: &Inventory, b: &mut Budget) -> Result<(), &'static str> {
    prove_available(input, loaded, b)?;
    for a in &input.available {
        b.charge(1)?;
        let z = loaded.available.iter().find(|z| z.id == a.id);
        let z = z.ok_or("input-structure-unproved")?;
        if a.len != z.len
            || a.structure.parent == Identity::Unknown
            || z.structure.parent == Identity::Unknown
        {
            // A normalized/split/Unknown header needs a maintained resolution
            // proof. This witness has none; never copy the SDK integrator.
            return Err("input-structure-unproved");
        }
        if a.structure.parent != z.structure.parent
            || a.structure.map_key != z.structure.map_key
            || a.structure.origin != z.structure.origin
            || a.structure.right_origin != z.structure.right_origin
        {
            return Err("input-structure-mismatch");
        }
    }
    Ok(())
}
#[test]
fn retained_input_split_merge_unicode_and_identical_duplicate() {
    use crate::updates::decoder::Decode;
    use crate::updates::encoder::Encode;
    use crate::{ClientID, StateVector, Text};
    let source = super::doc(10);
    let text = source.get_or_insert_text("text");
    text.insert(&mut source.transact_mut(), 0, "한글🙂");
    let baseline = super::bytes(&source);
    let cut = source.transact().state_vector();
    text.insert(&mut source.transact_mut(), 4, " 끝🧪");
    let tail = crate::diff_updates_v1(&super::bytes(&source), &cut.encode_v1()).unwrap();
    let dest = super::doc(999);
    dest.get_or_insert_text("text");
    let mut budget = Budget(0);
    let mut captures = Vec::new();
    for bytes in [&baseline, &tail, &baseline] {
        let update = Update::decode_v1(bytes).unwrap();
        captures.push(input(&update, &mut budget).unwrap());
        dest.transact_mut().apply_update(update).unwrap();
    }
    let after = integrated(&dest, &mut budget).unwrap();
    assert!(after
        .available
        .iter()
        .any(|r| r.id == super::id(10, 0) && r.len == 8));
    for i in &captures {
        prove_available(i, &after, &mut budget).unwrap();
        assert_eq!(
            prove(i, &after, &mut budget),
            Err("input-structure-unproved")
        );
    }
    assert_eq!(dest.transact().state_vector().get(&ClientID::new(10)), 8);
    let _ = StateVector::default(); // public types only in consumer.
}
#[test]
fn retained_input_gc_is_disclosed_but_available_converted_to_gc_fails() {
    use crate::block::{Block, BlockRange, Item, ItemContent};
    use crate::types::TypePtr;
    use crate::updates::encoder::{Encode, Encoder, EncoderV1};
    let mut already_gc = Update::default();
    already_gc
        .blocks
        .add_block(Block::GC(BlockRange::new(super::id(10, 0), 2)));
    let mut encoder = EncoderV1::new();
    already_gc.encode(&mut encoder);
    let encoded = encoder.to_vec();
    let dest = super::doc(999);
    let mut b = Budget(0);
    let capture = input(&already_gc, &mut b).unwrap();
    super::apply(&dest, &encoded);
    let loaded = integrated(&dest, &mut b).unwrap();
    assert_eq!(capture.unavailable, [(super::id(10, 0), 2)]);
    prove_available(&capture, &loaded, &mut b).unwrap(); // unavailable coverage, not Complete.
    let missing = Inventory::default();
    assert_eq!(
        prove_available(&capture, &missing, &mut b),
        Err("unavailable-interval-loss")
    );
    let mut truncated = Inventory::default();
    truncated.unavailable.push((super::id(10, 0), 1));
    assert_eq!(
        prove_available(&capture, &truncated, &mut b),
        Err("unavailable-interval-loss")
    );

    let mut incoming = Update::default();
    incoming.blocks.add_block(Block::Item(
        Item::new(
            super::id(20, 0),
            None,
            None,
            None,
            None,
            TypePtr::Named("metadata".into()),
            Some("not-a-type".into()),
            ItemContent::Deleted(1),
        )
        .unwrap(),
    ));
    incoming.blocks.add_block(Block::Item(
        Item::new(
            super::id(20, 1),
            None,
            None,
            None,
            None,
            TypePtr::ID(super::id(20, 0)),
            Some("ref".into()),
            ItemContent::Any(vec![Any::from("must-not-disappear")]),
        )
        .unwrap(),
    ));
    let mut e = EncoderV1::new();
    incoming.encode(&mut e);
    let bytes = e.to_vec();
    let capture = input(&incoming, &mut b).unwrap();
    let dest = super::doc(998);
    super::apply(&dest, &bytes);
    let loaded = integrated(&dest, &mut b).unwrap();
    assert_eq!(prove(&capture, &loaded, &mut b), Err("input-to-load-loss"));
}
#[test]
fn retained_input_conflicting_duplicate_must_not_pass_by_interval_coverage() {
    use crate::block::{Block, Item, ItemContent};
    use crate::types::TypePtr;
    let source = super::doc(10);
    use crate::Map;
    source
        .get_or_insert_map("metadata")
        .insert(&mut source.transact_mut(), "ref", "real");
    let mut u = Update::default();
    u.blocks.add_block(Block::Item(
        Item::new(
            super::id(10, 0),
            None,
            None,
            None,
            None,
            TypePtr::Named("metadata".into()),
            Some("ref".into()),
            ItemContent::Any(vec![Any::from("forged-old-ref")]),
        )
        .unwrap(),
    ));
    let mut b = Budget(0);
    let capture = input(&u, &mut b).unwrap();
    source.transact_mut().apply_update(u).unwrap();
    let loaded = integrated(&source, &mut b).unwrap();
    assert_eq!(
        prove(&capture, &loaded, &mut b),
        Err("input-content-mismatch")
    );
}
#[test]
fn retained_input_witness_budget_and_unsupported_identity_are_explicit() {
    let mut b = Budget(4096);
    assert_eq!(b.charge(1), Err("witness-budget"));
    let mut b = Budget(0);
    assert_eq!(
        copy(RetainedContent::Subdocument, &mut b),
        Err("unsupported-input-identity")
    );
}

#[test]
fn retained_input_non_type_parent_is_rejected_by_standard_integrator() {
    use crate::block::{Block, Item, ItemContent};
    use crate::types::TypePtr;
    let mut incoming = Update::default();
    incoming.blocks.add_block(Block::Item(
        Item::new(
            super::id(20, 0),
            None,
            None,
            None,
            None,
            TypePtr::Named("metadata".into()),
            Some("not-a-type".into()),
            ItemContent::Any(vec![Any::from("value")]),
        )
        .unwrap(),
    ));
    incoming.blocks.add_block(Block::Item(
        Item::new(
            super::id(20, 1),
            None,
            None,
            None,
            None,
            TypePtr::ID(super::id(20, 0)),
            Some("ref".into()),
            ItemContent::Any(vec![Any::from("must-not-disappear")]),
        )
        .unwrap(),
    ));
    let d = super::doc(999);
    let error = d.transact_mut().apply_update(incoming).unwrap_err();
    assert!(
        matches!(error, crate::error::UpdateError::InvalidParent(i, _) if i == super::id(20, 0))
    );
    // Invalid input never yields an availability proof or Complete claim.
}

// All controls share literal ID A10:0 and payload; SDK duplicate integration
// leaves the first stored item unchanged. Content equality alone is insufficient.
fn same_payload_duplicate(root: &str, key: &str) -> Update {
    use crate::block::{Block, Item, ItemContent};
    use crate::types::TypePtr;
    let mut update = Update::default();
    update.blocks.add_block(Block::Item(
        Item::new(
            super::id(10, 0),
            None,
            None,
            None,
            None,
            TypePtr::Named(root.into()),
            Some(key.into()),
            ItemContent::Any(vec![Any::from("literal-same-payload")]),
        )
        .unwrap(),
    ));
    update
}
fn duplicate_destination() -> Doc {
    use crate::Map;
    let d = super::doc(10);
    d.get_or_insert_map("metadata")
        .insert(&mut d.transact_mut(), "id", "literal-same-payload");
    d
}
#[test]
fn retained_input_identical_duplicate_preserves_known_structure() {
    let d = duplicate_destination();
    let u = same_payload_duplicate("metadata", "id");
    let mut b = Budget(0);
    let before = input(&u, &mut b).unwrap();
    assert_eq!(
        before.available[0].structure.parent,
        Identity::Root("metadata".into())
    );
    assert_eq!(before.available[0].structure.map_key.as_deref(), Some("id"));
    d.transact_mut().apply_update(u).unwrap();
    let after = integrated(&d, &mut b).unwrap();
    assert_eq!(after.available[0].deleted, Some(false));
    assert_eq!(
        after.available[0].structure.owner,
        Some(Owner {
            kind: TypeKind::Map,
            xml_tag: None,
            parent: OwnedParent::Root,
            deleted: false
        })
    );
    prove(&before, &after, &mut b).unwrap();
}
#[test]
fn retained_input_equal_payload_changed_named_root_is_mismatch() {
    let d = duplicate_destination();
    let u = same_payload_duplicate("foreign-root", "id");
    let mut b = Budget(0);
    let before = input(&u, &mut b).unwrap();
    d.transact_mut().apply_update(u).unwrap();
    let after = integrated(&d, &mut b).unwrap();
    prove_available(&before, &after, &mut b).unwrap();
    assert_eq!(
        prove(&before, &after, &mut b),
        Err("input-structure-mismatch")
    );
}
#[test]
fn retained_input_equal_payload_changed_map_key_is_mismatch() {
    let d = duplicate_destination();
    let u = same_payload_duplicate("metadata", "ref");
    let mut b = Budget(0);
    let before = input(&u, &mut b).unwrap();
    d.transact_mut().apply_update(u).unwrap();
    let after = integrated(&d, &mut b).unwrap();
    prove_available(&before, &after, &mut b).unwrap();
    assert_eq!(
        prove(&before, &after, &mut b),
        Err("input-structure-mismatch")
    );
}
#[test]
fn retained_input_equal_payload_unknown_parent_is_unproved() {
    use crate::block::{Block, Item, ItemContent};
    use crate::types::TypePtr;
    let d = duplicate_destination();
    let mut u = Update::default();
    u.blocks.add_block(Block::Item(
        Item::new(
            super::id(10, 0),
            None,
            None,
            None,
            None,
            TypePtr::Unknown,
            Some("id".into()),
            ItemContent::Any(vec![Any::from("literal-same-payload")]),
        )
        .unwrap(),
    ));
    let mut b = Budget(0);
    let before = input(&u, &mut b).unwrap();
    d.transact_mut().apply_update(u).unwrap();
    let after = integrated(&d, &mut b).unwrap();
    prove_available(&before, &after, &mut b).unwrap();
    assert_eq!(
        prove(&before, &after, &mut b),
        Err("input-structure-unproved")
    );
}

#[test]
fn retained_input_deleted_interval_and_metadata_remain_observable() {
    use crate::block::{Block, Item, ItemContent};
    use crate::types::TypePtr;
    let mut update = Update::default();
    update.blocks.add_block(Block::Item(
        Item::new(
            super::id(10, 0),
            None,
            None,
            None,
            None,
            TypePtr::Named("metadata".into()),
            Some("old".into()),
            ItemContent::Deleted(2),
        )
        .unwrap(),
    ));
    let mut b = Budget(0);
    let before = input(&update, &mut b).unwrap();
    assert_eq!(before.unavailable, [(super::id(10, 0), 2)]);
    assert_eq!(
        before.unavailable_items[0].1.parent,
        Identity::Root("metadata".into())
    );
    assert_eq!(
        before.unavailable_items[0].1.map_key.as_deref(),
        Some("old")
    );
    let d = super::doc(999);
    d.transact_mut().apply_update(update).unwrap();
    let after = integrated(&d, &mut b).unwrap();
    prove_available(&before, &after, &mut b).unwrap();
    assert_eq!(
        prove_available(&before, &Inventory::default(), &mut b),
        Err("unavailable-interval-loss")
    );
}
