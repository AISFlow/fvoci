//! Test-only public-API witnesses. Passing these does not prove an archive inventory complete.
use std::collections::BTreeSet;
use yrs::types::xml::Attributes;
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::{Encode, Encoder, EncoderV1};
use yrs::{
    Any, BranchID, ClientID, Doc, GetString, IdSet, Options, Out, ReadTxn, Snapshot, StateVector,
    Text, Transact, Update, Xml, XmlElementPrelim, XmlFragment, XmlTextPrelim, ID,
};

/// Transparent test-only trait decoration. Yrs owns every byte read, varint,
/// block dispatch and integration; these are method-call observations only.
/// In particular a LeftId call can also be a nested-parent read and does not
/// assert a resolved semantic origin/owner. No header or tag is interpreted.
#[derive(Clone, PartialEq)]
enum TypedCall {
    LeftId(ID),
    RightId(ID),
    Key(std::sync::Arc<str>),
    Any(Any),
    Json(Any),
}
struct Observe<'a> {
    inner: yrs::updates::decoder::DecoderV1<'a>,
    calls: Vec<TypedCall>,
}
impl yrs::encoding::read::Read for Observe<'_> {
    fn read_exact(&mut self, len: usize) -> Result<&[u8], yrs::encoding::read::Error> {
        yrs::encoding::read::Read::read_exact(&mut self.inner, len)
    }
    fn read_u8(&mut self) -> Result<u8, yrs::encoding::read::Error> {
        yrs::encoding::read::Read::read_u8(&mut self.inner)
    }
}
macro_rules! forward_decoder_read {
    ($name:ident, $ty:ty) => {
        fn $name(&mut self) -> Result<$ty, yrs::encoding::read::Error> {
            yrs::updates::decoder::Decoder::$name(&mut self.inner)
        }
    };
}
macro_rules! observe_decoder_read {
    ($name:ident, $ty:ty, $variant:ident) => {
        fn $name(&mut self) -> Result<$ty, yrs::encoding::read::Error> {
            let value = yrs::updates::decoder::Decoder::$name(&mut self.inner)?;
            self.calls.push(TypedCall::$variant(value.clone()));
            Ok(value)
        }
    };
}
impl yrs::updates::decoder::Decoder for Observe<'_> {
    fn reset_ds_cur_val(&mut self) {
        yrs::updates::decoder::Decoder::reset_ds_cur_val(&mut self.inner);
    }
    forward_decoder_read!(read_ds_clock, u32);
    forward_decoder_read!(read_ds_len, u32);
    forward_decoder_read!(read_client, ClientID);
    forward_decoder_read!(read_info, u8);
    forward_decoder_read!(read_parent_info, bool);
    forward_decoder_read!(read_type_ref, u8);
    forward_decoder_read!(read_len, u32);
    observe_decoder_read!(read_left_id, ID, LeftId);
    observe_decoder_read!(read_right_id, ID, RightId);
    observe_decoder_read!(read_key, std::sync::Arc<str>, Key);
    observe_decoder_read!(read_any, Any, Any);
    observe_decoder_read!(read_json, Any, Json);
    fn read_to_end(&mut self) -> Result<&[u8], yrs::encoding::read::Error> {
        yrs::updates::decoder::Decoder::read_to_end(&mut self.inner)
    }
}
fn observed(encoded: &[u8]) -> (Update, Vec<TypedCall>) {
    let mut decoder = Observe {
        inner: yrs::updates::decoder::DecoderV1::from(encoded),
        calls: Vec::new(),
    };
    let update = Update::decode(&mut decoder).unwrap();
    (update, decoder.calls)
}

fn isolated(source: &Doc, id: ID) -> Vec<u8> {
    let available = source.transact().state_vector();
    assert!(id.clock < available.get(&id.client));
    let mut hi = available.clone();
    hi.set_min(id.client, id.clock + 1);
    let mut remote: StateVector = available
        .iter()
        .filter(|(c, _)| **c != id.client)
        .map(|(c, end)| (*c, *end))
        .collect();
    if id.clock > 0 {
        remote.set_max(id.client, id.clock);
    }
    let encoded = yrs::diff_updates_v1(&split_prefix(source, hi), &remote.encode_v1()).unwrap();
    let ranges = Update::decode_v1(&encoded).unwrap().insertions(true);
    // Identity is external to decoder callbacks and established by maintained typed ranges.
    for (&client, &end) in available.iter() {
        for clock in 0..end {
            assert_eq!(
                ranges.contains(&ID::new(client, clock)),
                ID::new(client, clock) == id
            );
        }
    }
    encoded
}

fn doc(client: u64) -> Doc {
    Doc::with_options(Options {
        client_id: ClientID::new(client),
        skip_gc: true,
        offset_kind: yrs::OffsetKind::Utf16,
        ..Options::default()
    })
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
fn prefix(d: &Doc, sv: StateVector) -> Vec<u8> {
    // Never put clock zero in a snapshot: locked Yrs subtracts one internally.
    assert!(sv.iter().all(|(_, n)| *n > 0));
    let mut enc = EncoderV1::new();
    d.transact()
        .encode_state_from_snapshot(&Snapshot::new(sv, IdSet::default()), &mut enc)
        .unwrap();
    enc.to_vec()
}
fn split_prefix(d: &Doc, sv: StateVector) -> Vec<u8> {
    // Public Text::diff_range invokes the library's snapshot splitting on a disposable clone.
    let inspection = doc(998);
    apply(&inspection, &bytes(d));
    let snap = Snapshot::new(sv.clone(), IdSet::default());
    // Fixtures independently declare root types; native bytes do not encode a named root type.
    for (name, out) in d.transact().root_refs() {
        if matches!(out, Out::YText(_)) {
            inspection.get_or_insert_text(name);
        }
    }
    let mut selected;
    {
        let tx = inspection.transact();
        selected = tx
            .root_refs()
            .map(|(_, out)| out)
            .find(|out| matches!(out, Out::YText(_) | Out::YXmlText(_)));
        if selected.is_none() {
            'clients: for (&client, &end) in tx.state_vector().iter() {
                for clock in 0..end {
                    if let Some(branch) = BranchID::get_nested(&tx, &ID::new(client, clock)) {
                        let out: Out = branch.into();
                        if matches!(out, Out::YText(_) | Out::YXmlText(_)) {
                            selected = Some(out);
                            break 'clients;
                        }
                    }
                }
            }
        }
    }
    let mut tx = inspection.transact_mut();
    match selected {
        Some(Out::YText(t)) => {
            let _ = t.diff_range(&mut tx, Some(&snap), None, |_| ());
        }
        Some(Out::YXmlText(t)) => {
            let _ = t.diff_range(&mut tx, Some(&snap), None, |_| ());
        }
        _ => {}
    }
    let mut enc = EncoderV1::new();
    tx.encode_state_from_snapshot(&snap, &mut enc).unwrap();
    enc.to_vec()
}
fn xml(d: &Doc, client: u64, clock: u32) -> yrs::XmlElementRef {
    let branch =
        BranchID::get_nested(&d.transact(), &ID::new(ClientID::new(client), clock)).unwrap();
    match <_ as Into<Out>>::into(branch) {
        Out::YXmlElement(x) => x,
        _ => panic!("expected XML element"),
    }
}
fn text(d: &Doc) -> yrs::XmlTextRef {
    let branch = BranchID::get_nested(&d.transact(), &ID::new(ClientID::new(10), 4)).unwrap();
    match <_ as Into<Out>>::into(branch) {
        Out::YXmlText(x) => x,
        _ => panic!("expected XML text"),
    }
}

fn fixture() -> Doc {
    let a = doc(10);
    let root = a.get_or_insert_xml_fragment("prosemirror");
    {
        let mut tx = a.transact_mut();
        let p = root.push_back(&mut tx, XmlElementPrelim::empty("paragraph"));
        let m = p.push_back(&mut tx, XmlElementPrelim::empty("mention"));
        m.insert_attribute(&mut tx, "entity", "document");
        m.insert_attribute(&mut tx, "id", "doc-initial");
        p.push_back(&mut tx, XmlTextPrelim::new("한글🙂"));
    }
    // Independent expected native branch clocks, rather than reader-derived expectations.
    assert_eq!(
        <yrs::XmlElementRef as AsRef<yrs::branch::Branch>>::as_ref(&xml(&a, 10, 0)).id(),
        BranchID::Nested(ID::new(ClientID::new(10), 0))
    );
    assert_eq!(
        <yrs::XmlElementRef as AsRef<yrs::branch::Branch>>::as_ref(&xml(&a, 10, 1)).id(),
        BranchID::Nested(ID::new(ClientID::new(10), 1))
    );
    assert_eq!(text(&a).get_string(&a.transact()), "한글🙂");
    let baseline = bytes(&a);
    for (client, target, mark) in [
        (20, "doc-concurrent-B", "/documents/doc-mark-B"),
        (30, "doc-concurrent-C", "/documents/doc-mark-C"),
    ] {
        let peer = doc(client);
        apply(&peer, &baseline);
        let m = xml(&peer, 10, 1);
        let t = text(&peer);
        let mut tx = peer.transact_mut();
        m.insert_attribute(&mut tx, "id", target);
        t.format(
            &mut tx,
            0,
            1,
            [(
                "link".into(),
                Any::from(
                    [(String::from("href"), Any::from(mark))]
                        .into_iter()
                        .collect::<std::collections::HashMap<_, _>>(),
                ),
            )]
            .into_iter()
            .collect(),
        );
        drop(tx);
        apply(&a, &bytes(&peer));
    }
    let m = xml(&a, 10, 1);
    m.remove_attribute(&mut a.transact_mut(), &"id");
    root.remove_range(&mut a.transact_mut(), 0, 1);
    a
}

fn refs(d: &Doc) -> BTreeSet<String> {
    let mut result = BTreeSet::new();
    let sv = d.transact().state_vector();
    for (&client, &end) in sv.iter() {
        for clock in 0..end {
            let tx = d.transact();
            let Some(branch) = BranchID::get_nested(&tx, &ID::new(client, clock)) else {
                continue;
            };
            match <_ as Into<Out>>::into(branch) {
                Out::YXmlElement(x) if x.tag().as_ref() == "mention" => {
                    let attrs: Vec<_> =
                        Attributes::<_, yrs::Transaction>::new(x.as_ref(), &tx).collect();
                    let entity = attrs.iter().find(|(k, _)| *k == "entity");
                    let target = attrs.iter().find(|(k, _)| *k == "id");
                    if let (
                        Some((_, Out::Any(Any::String(kind)))),
                        Some((_, Out::Any(Any::String(id)))),
                    ) = (entity, target)
                    {
                        if kind.as_ref() == "document" {
                            result.insert(id.to_string());
                        }
                    }
                    // Compile typed ancestry even for retained/deleted handles.
                    let _ = x.parent();
                }
                Out::YXmlText(x) => {
                    drop(tx);
                    // diff_range may split items: this is always a disposable inspection Doc.
                    for run in x.diff_range(&mut d.transact_mut(), None, None, |_| ()) {
                        if let Some(attrs) = run.attributes {
                            if let Some(Any::Map(link)) = attrs.get("link") {
                                if let Some(Any::String(value)) = link.get("href") {
                                    result.insert(value.to_string());
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    result
}

// Minimal monotone closure witness, deliberately test-only, not a production inventory.
fn close(
    source: &Doc,
    mut wanted: StateVector,
    work: &mut u32,
    cap: u32,
) -> Result<Doc, &'static str> {
    let available = source.transact().state_vector();
    loop {
        // Charge both the disposable canonical clone used by splitting and the prefix reconstruction.
        if work.checked_add(2).is_none_or(|next| next > cap) {
            return Err("reconstruction budget");
        }
        *work += 2;
        let encoded = split_prefix(source, wanted.clone());
        let update = Update::decode_v1(&encoded).unwrap();
        let coverage = update.insertions(true);
        if update
            .state_vector()
            .iter()
            .any(|(c, n)| *n > wanted.get(c))
        {
            return Err("snapshot prefix exceeds requested clocks");
        }
        for (&client, &end) in wanted.iter() {
            if (0..end).any(|clock| !coverage.contains(&ID::new(client, clock))) {
                return Err("insertion hole (GC availability remains separate)");
            }
        }
        let probe = doc(999);
        apply(&probe, &encoded);
        let tx = probe.transact();
        if tx.store().pending_ds().is_some() {
            return Err("pending deletion");
        }
        let Some(pending) = tx.store().pending_update() else {
            drop(tx);
            return Ok(probe);
        };

        let mut progress = false;
        for (&client, &first_missing) in pending.missing.iter() {
            // Missing is the first unavailable clock, not the eventual dependency clock.
            let next = first_missing.checked_add(1).ok_or("clock overflow")?;
            if next > available.get(&client) {
                return Err("dependency outside source");
            }
            if next > wanted.get(&client) {
                wanted.set_max(client, next);
                progress = true;
            }
        }
        if !progress {
            eprintln!(
                "closure no-progress wanted={wanted:?} missing={:?}",
                pending.missing
            );
            return Err("no dependency progress");
        }
    }
}

#[test]
fn public_causal_prefix_witness_recovers_concurrent_losers_and_deleted_ancestry() {
    let a = fixture();
    let original = bytes(&a);
    let available = a.transact().state_vector();
    let expected: BTreeSet<String> = [
        "doc-initial",
        "doc-concurrent-B",
        "doc-concurrent-C",
        "/documents/doc-mark-B",
        "/documents/doc-mark-C",
    ]
    .map(String::from)
    .into_iter()
    .collect();
    let full = doc(999);
    apply(&full, &prefix(&a, available.clone()));
    assert!(!refs(&full).contains("doc-concurrent-B"));
    let mut found = BTreeSet::new();
    let mut work = 0;
    for (&client, &end) in available.iter() {
        // Test-only declared boundaries: clock8 cuts the emoji surrogate pair.
        for clock in (1..=end).filter(|n| !(client == ClientID::new(10) && *n == 8)) {
            let sv = [(client, clock)].into_iter().collect();
            found.extend(refs(&close(&a, sv, &mut work, 256).unwrap()));
        }
    }
    assert_eq!(found, expected);
    assert_eq!(
        bytes(&a),
        original,
        "canonical native store must remain untouched"
    );
    assert!(
        BranchID::get_nested(&a.transact(), &ID::new(ClientID::new(10), 0))
            .unwrap()
            .is_deleted()
    );
    eprintln!(
        "native_history tiny causal witness: reconstructions={work}, bytes={}, refs={}",
        original.len(),
        found.len()
    );
}

#[test]
fn public_known_clock_diff_isolates_ranges_and_snapshot_marks() {
    let a = fixture();
    let available = a.transact().state_vector();
    let mut hi = available.clone();
    hi.set_min(ClientID::new(20), 1);
    let encoded = prefix(&a, hi);
    let remote: StateVector = available
        .iter()
        .filter(|(c, _)| **c != ClientID::new(20))
        .map(|(c, n)| (*c, *n))
        .collect();
    let isolated = yrs::diff_updates_v1(&encoded, &remote.encode_v1()).unwrap();
    let insertions = Update::decode_v1(&isolated).unwrap().insertions(true);
    assert!(insertions.contains(&ID::new(ClientID::new(20), 0)));
    assert!(!insertions.contains(&ID::new(ClientID::new(20), 1)));
    assert!(!insertions.contains(&ID::new(ClientID::new(10), 0)));
    // Standalone isolation is not association: its inherited owner/key requires missing origins.
    let standalone = doc(999);
    apply(&standalone, &isolated);
    assert!(standalone.transact().store().pending_update().is_some());
    let disposable = doc(999);
    apply(&disposable, &bytes(&a));
    let snap = Snapshot::new(
        [(ClientID::new(10), 9)].into_iter().collect(),
        IdSet::default(),
    );
    let runs =
        text(&disposable).diff_range(&mut disposable.transact_mut(), Some(&snap), None, |_| ());
    let joined: String = runs
        .iter()
        .filter_map(|run| match &run.insert {
            Out::Any(Any::String(s)) => Some(s.as_ref()),
            _ => None,
        })
        .collect();
    assert_eq!(joined, "한글🙂");
}

#[test]
fn public_far_origin_charges_each_first_missing_retry_and_refuses_budget_excess() {
    let source = doc(10);
    use yrs::Map;
    let m = source.get_or_insert_map("metadata");
    for i in 0..30 {
        m.insert(&mut source.transact_mut(), format!("k{i}"), "old");
    }
    let peer = doc(20);
    apply(&peer, &bytes(&source));
    peer.get_or_insert_map("metadata")
        .insert(&mut peer.transact_mut(), "k29", "new");
    apply(&source, &bytes(&peer));
    let initial: StateVector = [(ClientID::new(20), 1)].into_iter().collect();
    let mut work = 0;
    let limited = close(&source, initial.clone(), &mut work, 4);
    assert!(
        matches!(limited, Err("reconstruction budget")),
        "result={:?}, work={work}",
        limited.as_ref().err()
    );
    assert_eq!(work, 4);
    let mut work = 0;
    let closed = close(&source, initial, &mut work, 64).unwrap();
    assert!(
        work > 20,
        "missing hints must not be misread as the far dependency clock"
    );
    assert!(!closed.transact().has_missing_updates());
    eprintln!("native_history far origin: reconstructions={work}");
}

#[test]
fn public_snapshot_string_prefix_excess_is_a_completeness_blocker() {
    let source = doc(10);
    source.get_or_insert_text("text").insert(
        &mut source.transact_mut(),
        0,
        "한글🙂abcdefghijklmnopqrstuvwxyz",
    );
    let wanted: StateVector = [(ClientID::new(10), 1)].into_iter().collect();
    let encoded = prefix(&source, wanted.clone());
    let actual = Update::decode_v1(&encoded).unwrap().state_vector();
    assert_eq!(actual.get(&ClientID::new(10)), 30);
    assert!(actual.get(&ClientID::new(10)) > wanted.get(&ClientID::new(10)));
    // This is a precise pinned-library observation, not an accepted prefix inventory.
    eprintln!("native_history unsplit string requested=1 encoded=30: must reject or prove standard splitting");
}

#[test]
fn public_snapshot_split_uses_library_for_disposable_utf16_prefix() {
    let source = doc(10);
    source.get_or_insert_text("text").insert(
        &mut source.transact_mut(),
        0,
        "한글🙂abcdefghijklmnopqrstuvwxyz",
    );
    let original = bytes(&source);
    for end in [1, 2, 4, 5, 30] {
        let wanted: StateVector = [(ClientID::new(10), end)].into_iter().collect();
        let encoded = split_prefix(&source, wanted);
        assert_eq!(
            Update::decode_v1(&encoded)
                .unwrap()
                .state_vector()
                .get(&ClientID::new(10)),
            end
        );
    }
    assert_eq!(bytes(&source), original);
    let bad_boundary = split_prefix(&source, [(ClientID::new(10), 3)].into_iter().collect());
    let probe = doc(999);
    apply(&probe, &bad_boundary);
    assert_eq!(
        probe.transact().state_vector().get(&ClientID::new(10)),
        4,
        "clock3 bisects UTF16 surrogate and exceeds requested bound"
    );
    eprintln!(
        "native_history half-surrogate materialization={:?}",
        probe
            .get_or_insert_text("text")
            .get_string(&probe.transact())
    );
}

#[test]
fn public_prefix_materialization_can_invent_cross_entity_reference_association() {
    let source = doc(10);
    let root = source.get_or_insert_xml_fragment("prosemirror");
    {
        let mut tx = source.transact_mut();
        let p = root.push_back(&mut tx, XmlElementPrelim::empty("paragraph"));
        let m = p.push_back(&mut tx, XmlElementPrelim::empty("mention"));
        m.insert_attribute(&mut tx, "entity", "task");
        m.insert_attribute(&mut tx, "id", "task-original");
    }
    let before = source.transact().snapshot();
    {
        let m = xml(&source, 10, 1);
        let mut tx = source.transact_mut();
        // One ordinary transaction changes a typed mention, so this intermediate pair is not an authored state.
        m.insert_attribute(&mut tx, "entity", "document");
        m.insert_attribute(&mut tx, "id", "doc-new");
    }
    let after = source.transact().snapshot();
    assert_eq!(before.state_map.get(&ClientID::new(10)), 4);
    assert_eq!(after.state_map.get(&ClientID::new(10)), 6);
    let intermediate = doc(999);
    apply(
        &intermediate,
        &split_prefix(&source, [(ClientID::new(10), 5)].into_iter().collect()),
    );
    assert!(
        refs(&intermediate).contains("task-original"),
        "prefix invents document-kind association for task UUID"
    );
    for snap in [before, after] {
        let state = doc(999);
        apply(&state, &split_prefix(&source, snap.state_map));
        assert!(!refs(&state).contains("task-original"));
    }
    eprintln!("native_history association counterexample: partial transaction pairs document entity with old task target");
}

#[test]
fn public_pending_false_does_not_prove_skip_coverage_and_gc_is_separate() {
    use yrs::Map;
    let source = doc(10);
    let m = source.get_or_insert_map("metadata");
    m.insert(&mut source.transact_mut(), "a", "first");
    m.insert(&mut source.transact_mut(), "b", "second");
    let suffix = yrs::diff_updates_v1(
        &bytes(&source),
        &[(ClientID::new(10), 1)]
            .into_iter()
            .collect::<StateVector>()
            .encode_v1(),
    )
    .unwrap();
    let hole = doc(999);
    apply(&hole, &suffix);
    assert!(!hole.transact().has_missing_updates());
    // Public state_vector stops at the Skip; pending=false alone misses this retained suffix.
    assert_eq!(hole.transact().state_vector().get(&ClientID::new(10)), 0);
    let actual = Update::decode_v1(&bytes(&hole)).unwrap().insertions(true);
    assert!(
        !actual.contains(&ID::new(ClientID::new(10), 0)),
        "Skip must not count as coverage"
    );
    assert!(actual.contains(&ID::new(ClientID::new(10), 1)));

    let gc = Doc::with_options(Options {
        client_id: ClientID::new(40),
        skip_gc: false,
        ..Options::default()
    });
    let root = gc.get_or_insert_xml_fragment("prosemirror");
    root.push_back(&mut gc.transact_mut(), XmlElementPrelim::empty("mention"));
    root.remove_range(&mut gc.transact_mut(), 0, 1);
    let encoded = bytes(&gc);
    assert!(Update::decode_v1(&encoded)
        .unwrap()
        .insertions(true)
        .contains(&ID::new(ClientID::new(40), 0)));
    let retained = doc(999);
    apply(&retained, &encoded);
    assert!(
        BranchID::get_nested(&retained.transact(), &ID::new(ClientID::new(40), 0)).is_none(),
        "insertion interval coverage does not certify retained typed content"
    );
}

#[test]
fn public_delete_only_tail_has_pending_deletion_without_pending_update() {
    let source = doc(10);
    let t = source.get_or_insert_text("text");
    t.insert(&mut source.transact_mut(), 0, "한🙂");
    let baseline = source.transact().state_vector();
    t.remove_range(&mut source.transact_mut(), 0, 3);
    let tail = yrs::diff_updates_v1(&bytes(&source), &baseline.encode_v1()).unwrap();
    assert!(Update::decode_v1(&tail).unwrap().state_vector().is_empty());
    let missing = doc(999);
    apply(&missing, &tail);
    let tx = missing.transact();
    assert!(tx.store().pending_update().is_none());
    assert!(tx.store().pending_ds().is_some());
    assert!(tx.has_missing_updates());
}

#[test]
fn public_decoder_decorator_observes_value_but_not_inherited_xml_owner_key() {
    let source = fixture();
    let canonical = bytes(&source);
    // Independently declared B20:0 replaces the ID attribute of mention A10:1,
    // inheriting map key "id" from non-type item A10:3 across clients.
    let expected_item = ID::new(ClientID::new(20), 0);
    let expected_owner = ID::new(ClientID::new(10), 1);
    let inherited_from = ID::new(ClientID::new(10), 3);
    let encoded = isolated(&source, expected_item);
    let (decoded, calls) = observed(&encoded);
    assert!(decoded.insertions(true).contains(&expected_item));
    assert!(calls.contains(&TypedCall::Any(Any::from("doc-concurrent-B"))));
    assert!(calls.contains(&TypedCall::LeftId(inherited_from)));
    assert!(!calls.iter().any(|call| matches!(call, TypedCall::Key(_))));
    assert!(BranchID::get_nested(&source.transact(), &expected_owner).is_some());
    assert!(
        BranchID::get_nested(&source.transact(), &inherited_from).is_none(),
        "public branch lookup cannot resolve a non-type map version's owner/key"
    );
    assert_eq!(bytes(&source), canonical);
    eprintln!("native_history decorator: isolated retained value+left ID observed; inherited owner/key unavailable as typed item metadata");
}

#[test]
fn public_decoder_decorator_sees_nonrendering_format_without_resolved_owner() {
    let source = doc(10);
    let root = source.get_or_insert_xml_fragment("prosemirror");
    let t = root.push_back(&mut source.transact_mut(), XmlTextPrelim::new("가"));
    let mark = Any::from(
        [(
            String::from("href"),
            Any::from("/documents/doc-nonrendered"),
        )]
        .into_iter()
        .collect::<std::collections::HashMap<_, _>>(),
    );
    t.format(
        &mut source.transact_mut(),
        0,
        0,
        [("link".into(), mark.clone())].into_iter().collect(),
    );
    assert_eq!(source.transact().state_vector().get(&ClientID::new(10)), 4);
    let canonical = bytes(&source);
    for run in t.diff_range(&mut source.transact_mut(), None, None, |_| ()) {
        assert!(run
            .attributes
            .as_ref()
            .is_none_or(|attrs| !attrs.contains_key("link")));
    }
    // Independently declared text ownerA10:0/startFormatA10:2/endFormatA10:3.
    let start = ID::new(ClientID::new(10), 2);
    let encoded = isolated(&source, start);
    let (_, calls) = observed(&encoded);
    assert!(calls.contains(&TypedCall::Key("link".into())));
    assert!(calls.iter().any(
        |call| matches!(call, TypedCall::Json(value) | TypedCall::Any(value) if *value == mark)
    ));
    assert!(calls.contains(&TypedCall::RightId(ID::new(ClientID::new(10), 1))));
    assert!(BranchID::get_nested(&source.transact(), &ID::new(ClientID::new(10), 1)).is_none());
    assert_eq!(bytes(&source), canonical);
    eprintln!("native_history decorator: nonrendering Format key/value retained; right-origin string ID supplies no typed owner");
}

#[test]
fn public_decoder_decorator_does_not_supply_cross_field_semantic_transaction_pair() {
    let source = doc(10);
    let root = source.get_or_insert_xml_fragment("prosemirror");
    {
        let mut tx = source.transact_mut();
        let p = root.push_back(&mut tx, XmlElementPrelim::empty("paragraph"));
        let m = p.push_back(&mut tx, XmlElementPrelim::empty("mention"));
        m.insert_attribute(&mut tx, "entity", "task");
        m.insert_attribute(&mut tx, "id", "task-original");
    }
    {
        let m = xml(&source, 10, 1);
        let mut tx = source.transact_mut();
        m.insert_attribute(&mut tx, "entity", "document");
        m.insert_attribute(&mut tx, "id", "doc-new");
    }
    let canonical = bytes(&source);
    let (_, calls) = observed(&isolated(&source, ID::new(ClientID::new(10), 5)));
    assert!(calls.contains(&TypedCall::Any(Any::from("doc-new"))));
    assert!(calls.contains(&TypedCall::LeftId(ID::new(ClientID::new(10), 3))));
    assert!(!calls.contains(&TypedCall::Any(Any::from("document"))));
    assert!(!calls.iter().any(|call| matches!(call, TypedCall::Key(_))));
    assert_eq!(bytes(&source), canonical);
    eprintln!("native_history decorator: isolated ID value does not establish its associated entity/version or transaction boundary");
}
