//! Independent correctness checks over two edits, each valid before they are merged.

mod support;

use fvoci_collab::testing::{repair, to_body, with_update};
use fvoci_collab::{FIELD, Repair, from_body_by, state_of};
use fvoci_model::{BODY_MAX_BYTES, BodyError};
use serde_json::{Value, json};
use support::{ROOMY, body_of, json_of};
use yrs::types::Attrs;
use yrs::types::xml::XmlOut;
use yrs::updates::decoder::Decode;
use yrs::{
    Any, ClientID, Doc, OffsetKind, Options, ReadTxn, Text, Transact, TransactionMut, Update,
    XmlElementRef, XmlFragment, XmlFragmentRef,
};

fn paragraph(letters: &str) -> Value {
    if letters.is_empty() {
        json!({"type": "paragraph"})
    } else {
        json!({"type": "paragraph", "content": [{"type": "text", "text": letters}]})
    }
}

fn document(content: Vec<Value>) -> Value {
    let mut document = json!({"type": "doc"});
    document["content"] = Value::Array(content);
    document
}

fn fork(base: &Doc, client: u64) -> Doc {
    // Without garbage collection, as the server's documents are: a replica that
    // collected what was deleted writes its state in other bytes.
    let doc = Doc::with_options(Options {
        offset_kind: OffsetKind::Utf16,
        skip_gc: true,
        ..Options::with_client_id(ClientID::new(client))
    });
    doc.get_or_insert_xml_fragment(FIELD);
    doc.transact_mut()
        .apply_update(Update::decode_v1(&state_of(base)).expect("a body's state"))
        .expect("the state applies");
    doc
}

fn element(txn: &impl ReadTxn, root: &XmlFragmentRef, path: &[usize]) -> XmlElementRef {
    let (&first, rest) = path.split_first().expect("a nonempty path");
    let Some(XmlOut::Element(mut node)) = root.children(txn).nth(first) else {
        panic!("no element at {path:?}");
    };
    for &index in rest {
        let Some(XmlOut::Element(next)) = node.children(txn).nth(index) else {
            panic!("no element at {path:?}");
        };
        node = next;
    }
    node
}

fn edited(
    base: &Doc,
    client: u64,
    change: impl FnOnce(&mut TransactionMut<'_>, &XmlFragmentRef),
) -> (Doc, Vec<u8>) {
    let side = fork(base, client);
    let before = side.transact().state_vector();
    {
        let root = side.get_or_insert_xml_fragment(FIELD);
        change(&mut side.transact_mut(), &root);
    }
    assert!(
        to_body(&side).is_ok(),
        "edit {client} alone must leave a valid body"
    );
    let delta = side.transact().encode_diff_v1(&before);
    (side, delta)
}

fn apply(base: &Doc, delta: &[u8]) -> Doc {
    with_update(base, delta, &ROOMY).expect("a normal edit applies")
}

// Garbage collection can coalesce adjacent blocks when a state is copied. Such
// encodings need not be byte-identical: clocks, deletions and visible content must be.
fn assert_same_document(left: &Doc, right: &Doc, context: &str) {
    assert_eq!(to_body(left), to_body(right), "{context}: visible document");
    assert_eq!(
        left.transact().state_vector(),
        right.transact().state_vector(),
        "{context}: clocks"
    );
    let a = Update::decode_v1(&state_of(left)).expect("a document state");
    let b = Update::decode_v1(&state_of(right)).expect("a document state");
    assert_eq!(a.delete_set(), b.delete_set(), "{context}: deletions");
}

fn assert_idempotent(doc: &Doc, first: &Repair) {
    let before = state_of(doc);
    let again = repair(doc).expect("a second repair succeeds");
    assert_eq!(again.deleted, 0, "once is enough");
    assert_eq!(again.update, None, "a second repair writes nothing");
    assert_eq!(again.body, first.body, "the body stays");
    assert_eq!(state_of(doc), before, "the shared state stays");
}

/// Exercise both arrival orders, both editing replicas, and independent repairers.
/// Compare the shared state as well as JSON, and replay each repair's emitted update.
fn assert_repaired_merge(base: &Doc, a: &(Doc, Vec<u8>), b: &(Doc, Vec<u8>), expected: &Value) {
    let ab = apply(&apply(base, &a.1), &b.1);
    let ba = apply(&apply(base, &b.1), &a.1);
    let at_a = apply(&a.0, &b.1);
    let at_b = apply(&b.0, &a.1);
    let before = state_of(&ab);
    for doc in [&ab, &ba, &at_a, &at_b] {
        assert_eq!(
            state_of(doc),
            before,
            "both arrival orders and replicas agree"
        );
        assert!(to_body(doc).is_err(), "the merge needs repair");
    }
    let repairs: Vec<Repair> = [&ab, &ba, &at_a, &at_b]
        .into_iter()
        .map(|doc| {
            let fixed = repair(doc).expect("the merged document is repaired");
            assert_eq!(json_of(fixed.body.as_ref()), *expected, "the repaired body");
            assert_eq!(
                to_body(doc),
                Ok(fixed.body.clone()),
                "repair changed the document itself"
            );
            assert!(fixed.deleted > 0, "the repair deletes something");
            assert_idempotent(doc, &fixed);
            fixed
        })
        .collect();
    let after = state_of(&ab);
    for doc in [&ba, &at_a, &at_b] {
        assert_eq!(
            state_of(doc),
            after,
            "independent repairs converge in shared state"
        );
    }
    for fixed in repairs {
        let update = fixed.update.expect("a repair emits its deletions");
        let replay = apply(&fork(base, 9), &a.1);
        let replay = apply(&replay, &b.1);
        let replay = apply(&replay, &update);
        assert_same_document(&replay, &ab, "a peer receiving the repair converges");
        assert_same_document(&apply(&replay, &update), &ab, "replaying is harmless");
        assert_same_document(&apply(&ab, &update), &ab, "independent repairs commute");
    }
}

#[test]
fn concurrent_deletions_empty_all_three_list_kinds_and_their_ancestors() {
    for (list, entry) in [
        ("bulletList", "listItem"),
        ("orderedList", "listItem"),
        ("taskList", "taskItem"),
    ] {
        for nested in [false, true] {
            let item = |letters: &str| {
                let mut item = json!({"type": entry, "content": [paragraph(letters)]});
                if entry == "taskItem" {
                    item["attrs"] = json!({"checked": false});
                }
                item
            };
            let mut node = json!({"type": list, "content": [item("one"), item("two")]});
            if list == "orderedList" {
                node["attrs"] = json!({"start": 0});
            }
            if nested {
                node = json!({"type": "blockquote", "content": [node]});
            }
            let base = from_body_by(&body_of(&document(vec![paragraph("kept"), node])), 1);
            let path: &[usize] = if nested { &[1, 0] } else { &[1] };
            let a = edited(&base, 2, |txn, root| {
                element(txn, root, path).remove_range(txn, 0, 1);
            });
            let b = edited(&base, 3, |txn, root| {
                element(txn, root, path).remove_range(txn, 1, 1);
            });
            assert_repaired_merge(&base, &a, &b, &document(vec![paragraph("kept")]));
        }
    }
}

#[test]
fn concurrent_deletions_remove_both_leading_paragraphs_of_list_entries() {
    for (list, entry) in [("bulletList", "listItem"), ("taskList", "taskItem")] {
        let item = |content: Vec<Value>| {
            let mut item = json!({"type": entry, "content": content});
            if entry == "taskItem" {
                item["attrs"] = json!({"checked": true});
            }
            item
        };
        let kept = item(vec![paragraph("kept entry")]);
        let input = document(vec![json!({"type": list, "content": [
            item(vec![paragraph("first"), paragraph("second"),
                json!({"type": "blockquote", "content": [paragraph("tail")]})]),
            kept,
        ]})]);
        let base = from_body_by(&body_of(&input), 1);
        let a = edited(&base, 2, |txn, root| {
            element(txn, root, &[0, 0]).remove_range(txn, 0, 1);
        });
        let b = edited(&base, 3, |txn, root| {
            element(txn, root, &[0, 0]).remove_range(txn, 1, 1);
        });
        let expected = document(vec![json!({"type": list, "content": [kept]})]);
        assert_repaired_merge(&base, &a, &b, &expected);
    }
}

#[test]
fn concurrent_marks_keep_overlaps_and_distinct_link_targets_without_a_repair() {
    let input = document(vec![paragraph("a😀e\u{301}한글z")]);
    let base = from_body_by(&body_of(&input), 1);
    for (left, right) in [("bold", "italic"), ("bold", "code"), ("link", "link")] {
        let edit = |client, name: &str, start, length, target| {
            edited(&base, client, |txn, root| {
                let line = element(txn, root, &[0]);
                let Some(XmlOut::Text(text)) = line.first_child() else {
                    panic!("the paragraph has text");
                };
                let value = if name == "link" {
                    Any::from_json(&format!("{{\"href\":\"{target}\"}}")).expect("link attrs")
                } else {
                    Any::from_json("{}").expect("empty attrs")
                };
                let attrs: Attrs = [(name.into(), value)].into_iter().collect();
                text.format(txn, start, length, attrs);
            })
        };
        let a = edit(2, left, 1, 4, "/a");
        let b = edit(3, right, 3, 4, "/b");
        let ab = apply(&apply(&base, &a.1), &b.1);
        let ba = apply(&apply(&base, &b.1), &a.1);
        assert_eq!(state_of(&ab), state_of(&ba), "formatting updates commute");
        let body = to_body(&ab)
            .expect("overlapping marks remain valid")
            .expect("nonempty");
        assert_eq!(body.text(), "a😀e\u{301}한글z");
        let runs = body.document()["content"][0]["content"]
            .as_array()
            .expect("text runs");
        let expected = if left == "link" {
            json!([
                {"type": "text", "text": "a"},
                {"type": "text", "text": "😀", "marks": [{"type": "link", "attrs": {"href": "/a"}}]},
                {"type": "text", "text": "e\u{301}", "marks": [{"type": "link", "attrs": {"href": "/b"}}]},
                {"type": "text", "text": "한글z"},
            ])
        } else {
            json!([
                {"type": "text", "text": "a"},
                {"type": "text", "text": "😀", "marks": [{"type": left}]},
                {"type": "text", "text": "e\u{301}", "marks": [{"type": left}, {"type": right}]},
                {"type": "text", "text": "한글", "marks": [{"type": right}]},
                {"type": "text", "text": "z"},
            ])
        };
        assert_eq!(
            Value::from(runs.clone()),
            expected,
            "{left}/{right}: exact UTF-16 spans"
        );
        let before = state_of(&ab);
        let fixed = repair(&ab).expect("a valid merge needs no deletion");
        assert_eq!(fixed.deleted, 0);
        assert_eq!(fixed.update, None);
        assert_eq!(fixed.body, Some(body));
        assert_eq!(state_of(&ab), before);
        assert_idempotent(&ab, &fixed);
    }
}

#[test]
fn a_concurrent_delete_and_insert_preserve_surrogate_pairs_and_combining_characters() {
    let base = from_body_by(&body_of(&document(vec![paragraph("a😀b")])), 1);
    let a = edited(&base, 2, |txn, root| {
        let line = element(txn, root, &[0]);
        let Some(XmlOut::Text(text)) = line.first_child() else {
            panic!("text");
        };
        text.remove_range(txn, 1, 2);
    });
    let b = edited(&base, 3, |txn, root| {
        let line = element(txn, root, &[0]);
        let Some(XmlOut::Text(text)) = line.first_child() else {
            panic!("text");
        };
        text.insert(txn, 3, "e\u{301}𝒳");
    });
    for merged in [apply(&a.0, &b.1), apply(&b.0, &a.1)] {
        let fixed = repair(&merged).expect("a valid text merge");
        assert_eq!(
            json_of(fixed.body.as_ref()),
            document(vec![paragraph("ae\u{301}𝒳b")])
        );
        assert_eq!(fixed.deleted, 0);
        assert_idempotent(&merged, &fixed);
    }
}

#[test]
fn merged_valid_edits_over_the_body_limit_are_refused_without_changing_the_document() {
    // This is the documented refusal policy, not a failing regression: repairing
    // structure must not silently discard valid text to fit a storage limit.
    let base = from_body_by(&body_of(&document(vec![paragraph("seed")])), 1);
    let append = |client, letters: &str| {
        edited(&base, client, |txn, root| {
            let line = root.push_back(txn, yrs::XmlElementPrelim::empty("paragraph"));
            line.push_back(txn, yrs::XmlTextPrelim::new(letters));
        })
    };
    let a = append(2, &"a".repeat(BODY_MAX_BYTES / 2));
    let b = append(3, &"b".repeat(BODY_MAX_BYTES / 2));
    let merged = apply(&a.0, &b.1);
    assert_eq!(to_body(&merged), Err(BodyError::TooLarge));
    let before = state_of(&merged);
    assert_eq!(repair(&merged), Err(BodyError::TooLarge));
    assert_eq!(
        state_of(&merged),
        before,
        "a refused repair leaves the state alone"
    );
    assert_eq!(
        repair(&merged),
        Err(BodyError::TooLarge),
        "the refusal is stable"
    );
    assert_eq!(state_of(&merged), before);
}
