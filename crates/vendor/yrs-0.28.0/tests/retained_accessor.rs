// Focused same-version prototype tests. This is not an archive/DB/UI acceptance.
use crate::retained::*;
use crate::updates::decoder::Decode;
use crate::updates::encoder::Encode;
use crate::{
    Any, BranchID, ClientID, Doc, Map, Options, ReadTxn, StateVector, Text, Transact, Update, Xml,
    XmlElementPrelim, XmlFragment, XmlTextPrelim, ID,
};
use std::collections::{BTreeSet, HashMap};
use std::ops::ControlFlow;

fn doc(client: u64) -> Doc {
    Doc::with_options(Options {
        client_id: ClientID::new(client),
        offset_kind: crate::OffsetKind::Utf16,
        skip_gc: true,
        cleanup_formatting: false,
        ..Options::default()
    })
}
fn id(client: u64, clock: u32) -> ID {
    ID::new(ClientID::new(client), clock)
}
fn bytes(d: &Doc) -> Vec<u8> {
    d.transact()
        .encode_state_as_update_v1(&StateVector::default())
}
fn apply(d: &Doc, b: &[u8]) {
    d.transact_mut()
        .apply_update(Update::decode_v1(b).unwrap())
        .unwrap();
}
fn walk(d: &Doc, cb: impl for<'a> FnMut(RetainedEvent<'a>) -> ControlFlow<()>) {
    assert_eq!(
        d.transact()
            .visit_retained(VisitLimits::default(), cb)
            .unwrap(),
        ControlFlow::Continue(())
    );
}
fn xml(d: &Doc, client: u64, clock: u32) -> crate::XmlElementRef {
    let tx = d.transact();
    let branch = BranchID::get_nested(&tx, &id(client, clock)).unwrap();
    let out: crate::Out = branch.into();
    match out {
        crate::Out::YXmlElement(x) => x,
        _ => panic!("literal fixture type mismatch"),
    }
}

#[test]
fn retained_owned_copies_and_nonmutation_unicode() {
    let d = doc(10);
    let t = d.get_or_insert_text("text");
    t.insert(&mut d.transact_mut(), 0, "한글🙂");
    let before = bytes(&d);
    let snapshot = d.transact().snapshot().encode_v1();
    let sv = d.transact().state_vector();
    let mut strings = Vec::new();
    walk(&d, |e| {
        if let RetainedEvent::Block(RetainedEntry::Item {
            id: i,
            native_clock_len,
            content: RetainedContent::String(s),
            owner,
            ..
        }) = e
        {
            assert_eq!(i, id(10, 0));
            assert_eq!(native_clock_len, 4);
            assert!(matches!(
                owner,
                OwnerView::Resolved(BranchView {
                    id: BranchIdentity::Root("text"),
                    parent: ParentView::Root,
                    ..
                })
            ));
            strings.push(s.to_owned());
        }
        ControlFlow::Continue(())
    });
    assert_eq!(strings, ["한글🙂"]);
    assert_eq!(bytes(&d), before);
    assert_eq!(d.transact().state_vector(), sv);
    assert_eq!(d.transact().snapshot().encode_v1(), snapshot);
}

#[test]
fn retained_losing_map_versions_original_owner_key_and_deleted() {
    let a = doc(10);
    let m = a.get_or_insert_map("metadata");
    m.insert(&mut a.transact_mut(), "ref", "old");
    let baseline = bytes(&a);
    for (client, value) in [(20, "losing-B"), (30, "winner-C")] {
        let peer = doc(client);
        apply(&peer, &baseline);
        peer.get_or_insert_map("metadata")
            .insert(&mut peer.transact_mut(), "ref", value);
        apply(&a, &bytes(&peer));
    }
    m.remove(&mut a.transact_mut(), "ref");
    let mut actual = Vec::new();
    walk(&a, |e| {
        if let RetainedEvent::Block(RetainedEntry::Item {
            id,
            owner,
            map_key: Some("ref"),
            deleted,
            content: RetainedContent::Values(v),
            ..
        }) = e
        {
            assert!(deleted);
            assert!(matches!(
                owner,
                OwnerView::Resolved(BranchView {
                    id: BranchIdentity::Root("metadata"),
                    ..
                })
            ));
            actual.push((id, v[0].clone()));
        }
        ControlFlow::Continue(())
    });
    actual.sort_by_key(|(i, _)| i.client);
    assert_eq!(
        actual,
        [
            (id(10, 0), Any::from("old")),
            (id(20, 0), Any::from("losing-B")),
            (id(30, 0), Any::from("winner-C"))
        ]
    );
}

#[test]
fn retained_deleted_ancestor_and_root_are_distinct() {
    let d = doc(10);
    let root = d.get_or_insert_xml_fragment("prosemirror");
    {
        let mut tx = d.transact_mut();
        let p = root.push_back(&mut tx, XmlElementPrelim::empty("paragraph"));
        let m = p.push_back(&mut tx, XmlElementPrelim::empty("mention"));
        m.insert_attribute(&mut tx, "entity", "document");
        m.insert_attribute(&mut tx, "id", "old");
    }
    root.remove_range(&mut d.transact_mut(), 0, 1);
    let mut found = false;
    let mut roots = 0;
    walk(&d, |e| {
        match e {
            RetainedEvent::Root(v) => {
                assert_eq!(v.parent, ParentView::Root);
                roots += 1;
            }
            RetainedEvent::Block(RetainedEntry::Item {
                id: i,
                owner: OwnerView::Resolved(v),
                map_key: Some("id"),
                ..
            }) if i == id(10, 3) => {
                assert_eq!(v.id, BranchIdentity::Nested(id(10, 1)));
                assert_eq!(
                    v.parent,
                    ParentView::Nested(BranchIdentity::Nested(id(10, 0)))
                );
                assert!(v.deleted);
                found = true;
            }
            _ => {}
        }
        ControlFlow::Continue(())
    });
    assert!(found);
    assert_eq!(roots, 1);
}

#[test]
fn retained_nonrendering_format_has_original_non_type_owner() {
    let d = doc(10);
    let root = d.get_or_insert_xml_fragment("prosemirror");
    let text = root.push_back(&mut d.transact_mut(), XmlTextPrelim::new("가"));
    let mark = Any::from(HashMap::from([(
        "href".to_owned(),
        Any::from("/documents/old"),
    )]));
    text.format(
        &mut d.transact_mut(),
        0,
        0,
        [("link".into(), mark.clone())].iter().cloned().collect(),
    );
    let canonical = bytes(&d);
    let mut seen = false;
    walk(&d, |e| {
        if let RetainedEvent::Block(RetainedEntry::Item {
            id: i,
            owner: OwnerView::Resolved(v),
            content: RetainedContent::Format { key: "link", value },
            ..
        }) = e
        {
            if i == id(10, 2) {
                assert_eq!(*value, mark);
                assert_eq!(v.id, BranchIdentity::Nested(id(10, 0)));
                seen = true;
            }
        }
        ControlFlow::Continue(())
    });
    assert!(seen);
    assert_eq!(bytes(&d), canonical);
}

#[test]
fn retained_changed_kind_envelope_never_claims_authored_pair() {
    let d = doc(10);
    let root = d.get_or_insert_xml_fragment("prosemirror");
    {
        let mut tx = d.transact_mut();
        let p = root.push_back(&mut tx, XmlElementPrelim::empty("paragraph"));
        let embed = p.push_back(&mut tx, XmlElementPrelim::empty("embed"));
        embed.insert_attribute(&mut tx, "entity", "task");
        embed.insert_attribute(&mut tx, "ref", "task-original");
    }
    let before = d.transact().snapshot();
    {
        let embed = xml(&d, 10, 1);
        let mut tx = d.transact_mut();
        embed.insert_attribute(&mut tx, "entity", "document");
        embed.insert_attribute(&mut tx, "ref", "doc-new");
    }
    let after = d.transact().snapshot();
    assert_eq!(before.state_map.get(&ClientID::new(10)), 4);
    assert_eq!(after.state_map.get(&ClientID::new(10)), 6);
    let mut kinds = BTreeSet::new();
    let mut values = BTreeSet::new();
    walk(&d, |e| {
        if let RetainedEvent::Block(RetainedEntry::Item {
            owner: OwnerView::Resolved(v),
            map_key: Some(key),
            content: RetainedContent::Values(items),
            ..
        }) = e
        {
            if v.id == BranchIdentity::Nested(id(10, 1)) {
                assert!(matches!(v.xml_tag, Some("embed")));
                if let [Any::String(s)] = items {
                    if key == "entity" {
                        kinds.insert(s.to_string());
                    }
                    if key == "ref" {
                        values.insert(s.to_string());
                    }
                }
            }
        }
        ControlFlow::Continue(())
    });
    assert_eq!(kinds, BTreeSet::from(["task".into(), "document".into()]));
    assert_eq!(
        values,
        BTreeSet::from(["task-original".into(), "doc-new".into()])
    );
    assert!(kinds.len().checked_mul(values.len()).unwrap() <= 4);
    let selected = BTreeSet::from([("task", "task-original"), ("document", "doc-new")]);
    let mut potential_unresolved = BTreeSet::new();
    for k in &kinds {
        for v in &values {
            if !selected.contains(&(k.as_str(), v.as_str())) {
                potential_unresolved.insert((k.as_str(), v.as_str()));
            }
        }
    }
    assert_eq!(
        potential_unresolved,
        BTreeSet::from([("document", "task-original"), ("task", "doc-new")])
    );
    // This envelope is labelled potential; no actual pair reconstruction occurs.
}

#[test]
fn retained_break_and_precharged_capacity_and_cap_plus_one() {
    let d = doc(10);
    let m = d.get_or_insert_map("metadata");
    m.insert(&mut d.transact_mut(), "a", "one");
    m.insert(&mut d.transact_mut(), "b", "two");
    let mut calls = 0;
    let r = d
        .transact()
        .visit_retained(VisitLimits::default(), |_| {
            calls += 1;
            ControlFlow::Break("cancelled")
        })
        .unwrap();
    assert_eq!(r, ControlFlow::Break("cancelled"));
    assert_eq!(calls, 1);
    let summary = summary(&d.transact());
    assert!(summary.client_table_capacity >= summary.client_count);
    assert!(summary.root_table_capacity >= summary.root_count);
    let limits = VisitLimits {
        max_table_capacity: 0,
        ..VisitLimits::default()
    };
    assert_eq!(
        d.transact()
            .visit_retained(limits, |_| { panic!("capacity rejected before callback") }),
        Err::<ControlFlow<()>, _>(VisitError::TableCapacityLimit)
    );
    let limits = VisitLimits {
        max_blocks: 1,
        ..VisitLimits::default()
    };
    assert_eq!(
        d.transact()
            .visit_retained(limits, |_| ControlFlow::<()>::Continue(())),
        Err(VisitError::BlockLimit)
    );
}

#[test]
fn retained_skip_client_state_vector_zero_still_visits_suffix() {
    let source = doc(10);
    let m = source.get_or_insert_map("metadata");
    m.insert(&mut source.transact_mut(), "a", "first");
    m.insert(&mut source.transact_mut(), "b", "second");
    let suffix = crate::diff_updates_v1(
        &bytes(&source),
        &[(ClientID::new(10), 1)]
            .iter()
            .copied()
            .collect::<StateVector>()
            .encode_v1(),
    )
    .unwrap();
    let d = doc(999);
    apply(&d, &suffix);
    assert_eq!(d.transact().state_vector().get(&ClientID::new(10)), 0);
    let mut skipped = false;
    let mut item = false;
    walk(&d, |e| {
        match e {
            RetainedEvent::Block(RetainedEntry::Skip {
                id: i,
                native_clock_len: 1,
            }) if i == id(10, 0) => skipped = true,
            RetainedEvent::Block(RetainedEntry::Item { id: i, .. }) if i == id(10, 1) => {
                item = true
            }
            _ => {}
        }
        ControlFlow::Continue(())
    });
    assert!(skipped && item);
}

#[test]
fn retained_summary_distinguishes_pending_deletion() {
    let source = doc(10);
    let text = source.get_or_insert_text("text");
    text.insert(&mut source.transact_mut(), 0, "한🙂");
    let sv = source.transact().state_vector();
    text.remove_range(&mut source.transact_mut(), 0, 3);
    let tail = crate::diff_updates_v1(&bytes(&source), &sv.encode_v1()).unwrap();
    let d = doc(999);
    apply(&d, &tail);
    let s = summary(&d.transact());
    assert!(!s.pending_update);
    assert!(s.pending_deletions);
}

#[test]
fn retained_input_borrowed_types_without_inherited_key_claim() {
    let d = doc(10);
    let map = d.get_or_insert_map("metadata");
    map.insert(&mut d.transact_mut(), "ref", "before");
    let sv = d.transact().state_vector();
    map.insert(&mut d.transact_mut(), "ref", "after");
    let tail = crate::diff_updates_v1(&bytes(&d), &sv.encode_v1()).unwrap();
    let update = Update::decode_v1(&tail).unwrap();
    let mut seen = false;
    assert_eq!(
        visit_input(&update, VisitLimits::default(), |e| {
            if let InputEntry::Item {
                id: i,
                parent,
                map_key,
                origin,
                content: RetainedContent::Values(v),
                ..
            } = e
            {
                assert_eq!(i, id(10, 1));
                assert_eq!(parent, InputParent::Unknown);
                assert_eq!(map_key, None);
                assert_eq!(origin, Some(id(10, 0)));
                assert_eq!(v, &[Any::from("after")]);
                seen = true;
            }
            ControlFlow::<()>::Continue(())
        })
        .unwrap(),
        ControlFlow::Continue(())
    );
    assert!(seen);
}

// Internal library vectors use its existing typed Block/Item construction and
// standard encoder, never a test-local wire/header parser.
#[test]
fn retained_ranges_reject_empty_overflow_without_effects() {
    use crate::block::{Block, BlockRange};
    for (clock, len, expected) in [
        (0, 0, VisitError::EmptyRange { id: id(10, 0) }),
        (
            u32::MAX,
            1,
            VisitError::RangeOverflow {
                id: id(10, u32::MAX),
            },
        ),
    ] {
        let mut u = Update::default();
        u.blocks
            .add_block(Block::GC(BlockRange::new(id(10, clock), len)));
        assert_eq!(
            visit_input(&u, VisitLimits::default(), |_| ControlFlow::<()>::Continue(
                ()
            )),
            Err(expected)
        );
    }
}

#[path = "retained_input_witness.rs"]
mod input_witness;

#[test]
fn retained_input_all_content_variants_are_typed_not_omitted() {
    use crate::block::{Block, Item, ItemContent};
    use crate::branch::Branch;
    use crate::types::{TypePtr, TypeRef};
    let contents = vec![
        ItemContent::Any(vec![Any::from("value")]),
        ItemContent::JSON(vec!["{}".into()]),
        ItemContent::String("한🙂".into()),
        ItemContent::Embed(Any::from("embedded")),
        ItemContent::Format("link".into(), Box::new(Any::Null)),
        ItemContent::Binary(vec![0, 1, 255]),
        ItemContent::Type(Branch::new(TypeRef::XmlElement("mention".into()))),
        ItemContent::Type(Branch::new(TypeRef::XmlHook)),
        ItemContent::Type(Branch::new(TypeRef::Undefined)),
        ItemContent::Doc(None, doc(55)),
        ItemContent::Deleted(2),
    ];
    let mut u = Update::default();
    let mut clock = 0;
    for content in contents {
        let item = Item::new(
            id(10, clock),
            None,
            None,
            None,
            None,
            TypePtr::Named("source".into()),
            None,
            content,
        )
        .unwrap();
        clock += item.len();
        u.blocks.add_block(Block::Item(item));
    }
    let mut labels = Vec::new();
    let result = visit_input(&u, VisitLimits::default(), |entry| {
        if let InputEntry::Item { content, .. } = entry {
            labels.push(match content {
                RetainedContent::Values(_) => "values",
                RetainedContent::JsonValues(_) => "json",
                RetainedContent::String(_) => "string",
                RetainedContent::Embed(_) => "embed",
                RetainedContent::Format { .. } => "format",
                RetainedContent::Binary(_) => "binary",
                RetainedContent::Type {
                    kind: TypeKind::XmlElement,
                    xml_tag: Some("mention"),
                } => "xml-type",
                RetainedContent::Type {
                    kind: TypeKind::XmlHook,
                    ..
                } => "hook",
                RetainedContent::Type {
                    kind: TypeKind::Unknown,
                    ..
                } => "unknown",
                RetainedContent::Subdocument => "subdocument",
                RetainedContent::Deleted { .. } => "deleted",
                _ => panic!("unexpected typed view"),
            });
        }
        ControlFlow::<()>::Continue(())
    })
    .unwrap();
    assert_eq!(result, ControlFlow::Continue(()));
    assert_eq!(
        labels,
        [
            "values",
            "json",
            "string",
            "embed",
            "format",
            "binary",
            "xml-type",
            "hook",
            "unknown",
            "subdocument",
            "deleted"
        ]
    );
}

#[test]
fn retained_nested_parent_unavailable_is_not_root() {
    use crate::block::{Item, ItemContent};
    use crate::branch::Branch;
    use crate::types::{TypePtr, TypeRef};
    let item = Item::new(
        id(10, 1),
        None,
        None,
        None,
        None,
        TypePtr::Unknown,
        None,
        ItemContent::Type(Branch::new(TypeRef::XmlElement("mention".into()))),
    )
    .unwrap();
    if let ItemContent::Type(branch) = &item.content {
        let v = super::branch_view(branch).unwrap();
        assert_eq!(v.id, BranchIdentity::Nested(id(10, 1)));
        assert_eq!(v.parent, ParentView::Unavailable);
        assert_ne!(v.parent, ParentView::Root);
    } else {
        panic!("typed fixture");
    }
}
