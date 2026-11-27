//! Shared content the vectors do not have, because what an editor does with it is not
//! what the server does, or because only the server's limits speak of it. The Y
//! documents are written here, by hand.

mod support;

use std::sync::Arc;

use fvoci_collab::testing::{repair, to_body};
use fvoci_collab::{FIELD, from_body, from_body_by, new_doc, state_of};
use fvoci_model::{BODY_MAX_BYTES, BODY_MAX_DEPTH, BODY_MAX_NODES, BodyError};
use serde_json::{Value, json};
use support::{body_of, json_of};
use yrs::{
    Any, Array, ArrayRef, Doc, Text, Transact, TransactionMut, Xml, XmlElementPrelim,
    XmlElementRef, XmlFragment, XmlFragmentRef, XmlTextPrelim,
};

/// A new document and its root, written by `write`.
fn written(write: impl FnOnce(&mut TransactionMut<'_>, &XmlFragmentRef)) -> Doc {
    let doc = new_doc();
    {
        let root = doc.get_or_insert_xml_fragment(FIELD);
        let mut txn = doc.transact_mut();
        write(&mut txn, &root);
    }
    doc
}

/// Appends a paragraph of one text.
fn paragraph(txn: &mut TransactionMut<'_>, parent: &impl XmlFragment, text: &str) -> XmlElementRef {
    let paragraph = parent.push_back(txn, XmlElementPrelim::empty("paragraph"));
    paragraph.push_back(txn, XmlTextPrelim::new(text));
    paragraph
}

fn text_paragraph(text: &str) -> Value {
    json!({"type": "paragraph", "content": [{"type": "text", "text": text}]})
}

fn document(blocks: &[Value]) -> Value {
    json!({"type": "doc", "content": blocks})
}

/// The body a repair leaves, as JSON, after checking that the document was refused
/// before and holds that body after.
fn repaired(doc: &Doc) -> Value {
    assert!(to_body(doc).is_err(), "refused as it is");
    let repair = repair(doc).expect("a repair");
    assert!(repair.deleted > 0, "something was deleted");
    assert_eq!(
        to_body(doc).expect("a body after the repair"),
        repair.body,
        "the document is the body given"
    );
    json_of(repair.body.as_ref())
}

#[test]
fn a_document_nothing_was_written_into_is_no_body() {
    assert_eq!(to_body(&new_doc()), Ok(None));
    assert_eq!(to_body(&Doc::new()), Ok(None), "not even a root");
    let nothing = repair(&new_doc()).expect("a repair");
    assert_eq!(
        (nothing.deleted, nothing.update, nothing.body),
        (0, None, None)
    );
}

/// An editor wraps a node at the root that the document may not hold; the server
/// deletes it.
#[test]
fn a_node_the_document_may_not_hold_is_deleted_at_the_root() {
    let doc = written(|txn, root| {
        let entry = root.push_back(txn, XmlElementPrelim::empty("listItem"));
        paragraph(txn, &entry, "entry");
        paragraph(txn, root, "kept");
        let mention = root.push_back(txn, XmlElementPrelim::empty("mention"));
        mention.insert_attribute(txn, "userId", "01930000-0000-7000-8000-00000000c002");
        root.push_back(txn, XmlElementPrelim::empty("hardBreak"));
        let task = root.push_back(txn, XmlElementPrelim::empty("taskItem"));
        paragraph(txn, &task, "task");
        let inner = root.push_back(txn, XmlElementPrelim::empty("doc"));
        paragraph(txn, &inner, "inner");
    });
    assert_eq!(repaired(&doc), document(&[text_paragraph("kept")]));
}

/// What fails inside a node that is deleted at the root goes with it, once.
#[test]
fn a_node_deleted_at_the_root_takes_what_failed_inside_it() {
    let doc = written(|txn, root| {
        paragraph(txn, root, "kept");
        let entry = root.push_back(txn, XmlElementPrelim::empty("listItem"));
        paragraph(txn, &entry, "entry");
        entry.push_back(txn, XmlElementPrelim::empty("bulletList"));
        entry.push_back(txn, XmlElementPrelim::empty("video"));
        paragraph(txn, root, "also kept");
    });
    assert!(to_body(&doc).is_err(), "refused as it is");
    let repair = repair(&doc).expect("a repair");
    assert_eq!(repair.deleted, 1, "the entry, with all it held");
    assert_eq!(
        json_of(repair.body.as_ref()),
        document(&[text_paragraph("kept"), text_paragraph("also kept")])
    );
}

/// A text at the root makes an editor's binding throw. The server refuses it, and a
/// repair deletes it.
#[test]
fn a_text_at_the_root_is_refused_and_deleted() {
    let doc = written(|txn, root| {
        root.push_back(txn, XmlTextPrelim::new("loose"));
        paragraph(txn, root, "kept");
    });
    assert!(
        matches!(to_body(&doc), Err(BodyError::InvalidNode { ref path, .. }) if path == "/content/0"),
        "{:?}",
        to_body(&doc)
    );
    assert_eq!(repaired(&doc), document(&[text_paragraph("kept")]));
}

/// Also a text without letters: the binding throws on a text at the root whatever it
/// holds, though it would add nothing to the body.
#[test]
fn an_empty_text_at_the_root_is_refused_and_deleted() {
    let doc = written(|txn, root| {
        root.push_back(txn, XmlTextPrelim::new(""));
        paragraph(txn, root, "kept");
        root.push_back(txn, XmlTextPrelim::new(""));
    });
    assert_eq!(
        to_body(&doc),
        Err(BodyError::InvalidNode {
            path: "/content/0".to_owned(),
            problem: "a text stands at the root",
        })
    );
    assert_eq!(repaired(&doc), document(&[text_paragraph("kept")]));
    assert_eq!(
        repair(&doc).expect("a second repair").deleted,
        0,
        "once is enough"
    );
}

/// Many nodes that fail under one parent, alone and in stretches, between ones that
/// stay: each goes, and each that stays is where it was.
#[test]
fn many_failing_children_of_one_parent_are_all_deleted() {
    let doc = written(|txn, root| {
        for index in (0..3000_u32).rev() {
            // Two of every five stay; the three between them are a stretch that goes.
            if index % 5 < 2 {
                let line = root.push_front(txn, XmlElementPrelim::empty("paragraph"));
                line.push_front(txn, XmlTextPrelim::new(index.to_string()));
            } else {
                root.push_front(txn, XmlElementPrelim::empty("video"));
            }
        }
    });
    let repair = {
        assert!(to_body(&doc).is_err(), "refused as it is");
        repair(&doc).expect("a repair")
    };
    assert_eq!(repair.deleted, 1800);
    let kept: Vec<Value> = (0..3000_u32)
        .filter(|index| index % 5 < 2)
        .map(|index| text_paragraph(&index.to_string()))
        .collect();
    assert!(
        json_of(repair.body.as_ref()) == document(&kept),
        "the 1200 that stay, in order"
    );
}

/// A long run of failing children is one stretch and goes at once; a document that
/// fails in more separate places than a repair deletes is refused as it is.
#[test]
fn a_repair_deletes_a_long_run_and_refuses_a_thousand_separate_ones() {
    let run = written(|txn, root| {
        paragraph(txn, root, "kept");
        for _ in 0..100_000 {
            root.push_front(txn, XmlElementPrelim::empty("video"));
        }
    });
    let repair = repair(&run).expect("a repair");
    assert_eq!(repair.deleted, 100_000);
    assert_eq!(
        json_of(repair.body.as_ref()),
        document(&[text_paragraph("kept")])
    );

    let alternating = |failing: u32| {
        written(|txn, root| {
            for _ in 0..failing {
                root.push_front(txn, XmlElementPrelim::empty("horizontalRule"));
                root.push_front(txn, XmlElementPrelim::empty("video"));
            }
        })
    };
    assert_eq!(
        repair_of(&alternating(1000)),
        Ok(1000),
        "a thousand stretches"
    );
    let too_many = alternating(1001);
    let before = state_of(&too_many);
    assert_eq!(
        fvoci_collab::testing::repair(&too_many),
        Err(BodyError::InvalidNode {
            path: String::new(),
            problem: "more separate parts of the document fail than a repair deletes",
        })
    );
    assert_eq!(state_of(&too_many), before, "nothing was deleted");
}

/// How many nodes a repair deleted, or why it did not repair.
fn repair_of(doc: &Doc) -> Result<usize, BodyError> {
    fvoci_collab::testing::repair(doc).map(|repair| repair.deleted)
}

/// Values where the children of a node belong have no JSON at all. An editor passes
/// over them; the server refuses the document, and a repair deletes the node that
/// holds them.
#[test]
fn a_node_that_holds_values_is_refused_and_deleted_whole() {
    let doc = written(|txn, root| {
        paragraph(txn, root, "one");
        let two = paragraph(txn, root, "two");
        let inside: &ArrayRef = two.as_ref();
        inside.push_back(txn, true);
        inside.push_back(txn, "a value");
        paragraph(txn, root, "three");
    });
    assert_eq!(
        to_body(&doc),
        Err(BodyError::InvalidNode {
            path: "/content/1".to_owned(),
            problem: "holds something that is neither a node nor a text",
        })
    );
    assert_eq!(
        repaired(&doc),
        document(&[text_paragraph("one"), text_paragraph("three")])
    );
}

/// At the root there is no node to delete in their place: the document is refused, and
/// so is its repair, which deletes nothing.
#[test]
fn a_root_that_holds_values_is_refused_and_not_repaired() {
    let doc = written(|txn, root| {
        paragraph(txn, root, "one");
        let values: &ArrayRef = root.as_ref();
        values.push_back(txn, 7);
        root.push_back(txn, XmlElementPrelim::empty("bulletList"));
    });
    let before = state_of(&doc);
    let refusal = BodyError::InvalidNode {
        path: String::new(),
        problem: "holds something that is neither a node nor a text",
    };
    assert_eq!(to_body(&doc), Err(refusal));
    assert_eq!(
        repair(&doc),
        Err(BodyError::InvalidNode {
            path: String::new(),
            problem: "the root holds something that is neither a node nor a text",
        })
    );
    assert_eq!(state_of(&doc), before, "nothing was deleted");
}

/// Letters put straight under the root or a node, with one outside the basic plane:
/// the library's own listing of such a parent never comes back. Reading and repairing
/// do, here.
#[test]
fn letters_where_nodes_belong_are_refused_without_being_listed() {
    // One change by client 5: the text "😀" under the root "default".
    let mut under_root = vec![1, 1, 5, 0, 4, 1, 7];
    under_root.extend_from_slice(b"default");
    under_root.extend_from_slice(&[4, 0xf0, 0x9f, 0x98, 0x80, 0]);
    let doc = fvoci_collab::testing::with_update(&new_doc(), &under_root, &support::ROOMY)
        .expect("the update is taken: it is the document it leaves that is refused");
    assert!(
        matches!(to_body(&doc), Err(BodyError::InvalidNode { .. })),
        "refused"
    );
    assert!(
        matches!(repair(&doc), Err(BodyError::InvalidNode { .. })),
        "not repaired"
    );

    // The same text under the paragraph of a body: the paragraph is the first change
    // of the client that wrote the body, client 1.
    let body = from_body_by(&body_of(&document(&[text_paragraph("kept")])), 1);
    let under_paragraph = [1, 1, 5, 0, 4, 0, 1, 0, 4, 0xf0, 0x9f, 0x98, 0x80, 0];
    let doc = fvoci_collab::testing::with_update(&body, &under_paragraph, &support::ROOMY)
        .expect("the update is taken");
    assert!(
        matches!(to_body(&doc), Err(BodyError::InvalidNode { .. })),
        "refused"
    );
    let repair = repair(&doc).expect("a repair");
    assert_eq!(
        (repair.deleted, repair.body),
        (1, None),
        "the paragraph goes whole"
    );
}

/// An editor keeps an element named `text` or deletes the line it is in, depending on
/// what stands beside it. The server deletes the element.
#[test]
fn an_element_named_text_is_deleted_and_its_line_stays() {
    let doc = written(|txn, root| {
        let line = paragraph(txn, root, "kept");
        line.push_back(txn, XmlElementPrelim::empty("text"));
    });
    assert_eq!(repaired(&doc), document(&[text_paragraph("kept")]));
}

/// Something in a text that is no letter (a binding may embed a value or a shared type
/// there): the text goes whole.
#[test]
fn a_text_that_holds_something_that_is_no_letter_is_deleted_whole() {
    let doc = written(|txn, root| {
        let line = paragraph(txn, root, "kept");
        let odd = line.push_back(txn, XmlTextPrelim::new("letters"));
        odd.insert_embed(txn, 3, Any::Bool(true));
        line.push_back(txn, XmlElementPrelim::empty("hardBreak"));
    });
    assert!(
        matches!(to_body(&doc), Err(BodyError::InvalidNode { problem, .. })
            if problem == "text holds something that is no text"),
        "{:?}",
        to_body(&doc)
    );
    assert_eq!(
        repaired(&doc),
        document(&[json!({"type": "paragraph", "content": [
            {"type": "text", "text": "kept"},
            {"type": "hardBreak"},
        ]})])
    );
}

/// The deletions a repair makes are the ones the vectors' editors make, and no more:
/// what stands beside a failing node stays, however deep.
#[test]
fn a_repair_deletes_only_what_fails() {
    let doc = written(|txn, root| {
        let quote = root.push_back(txn, XmlElementPrelim::empty("blockquote"));
        paragraph(txn, &quote, "before");
        let list = quote.push_back(txn, XmlElementPrelim::empty("bulletList"));
        let good = list.push_back(txn, XmlElementPrelim::empty("listItem"));
        paragraph(txn, &good, "good");
        let bad = list.push_back(txn, XmlElementPrelim::empty("listItem"));
        let heading = bad.push_back(txn, XmlElementPrelim::empty("heading"));
        heading.insert_attribute(txn, "level", Any::from(9.0));
        let also_good = list.push_back(txn, XmlElementPrelim::empty("listItem"));
        paragraph(txn, &also_good, "also good");
        paragraph(txn, &quote, "after");
    });
    let repair = {
        assert!(to_body(&doc).is_err(), "refused as it is");
        repair(&doc).expect("a repair")
    };
    // The heading, and then the entry it left without a paragraph to start with. The
    // heading goes with the entry: one deletion.
    assert_eq!(repair.deleted, 1);
    let entry = |text: &str| json!({"type": "listItem", "content": [text_paragraph(text)]});
    assert_eq!(
        json_of(repair.body.as_ref()),
        document(&[json!({"type": "blockquote", "content": [
            text_paragraph("before"),
            {"type": "bulletList", "content": [entry("good"), entry("also good")]},
            text_paragraph("after"),
        ]})])
    );
}

/// A Y document nested as deep as a body may be, and one level deeper.
fn nested(depth: usize) -> Doc {
    written(|txn, root| {
        let mut parent: XmlElementRef = root.push_back(txn, XmlElementPrelim::empty("blockquote"));
        // The document, this quote and the paragraph at the bottom are three levels.
        for _ in 3..depth {
            parent = parent.push_back(txn, XmlElementPrelim::empty("blockquote"));
        }
        parent.push_back(txn, XmlElementPrelim::empty("paragraph"));
    })
}

#[test]
fn a_document_past_a_limit_is_refused_and_no_repair_deletes_from_it() {
    assert!(
        to_body(&nested(BODY_MAX_DEPTH)).is_ok(),
        "{BODY_MAX_DEPTH} levels"
    );
    let breaks = |count: usize| {
        written(|txn, root| {
            let line = root.push_back(txn, XmlElementPrelim::empty("paragraph"));
            for _ in 0..count {
                line.push_back(txn, XmlElementPrelim::empty("hardBreak"));
            }
        })
    };
    // The document and the paragraph are two of the nodes.
    assert!(
        to_body(&breaks(BODY_MAX_NODES - 2)).is_ok(),
        "{BODY_MAX_NODES} nodes"
    );
    let letters = |count: usize| {
        written(|txn, root| {
            paragraph(txn, root, &"a".repeat(count));
        })
    };
    let frame = serde_json::to_vec(&document(&[text_paragraph("")]))
        .expect("JSON")
        .len();
    assert!(
        to_body(&letters(BODY_MAX_BYTES - frame)).is_ok(),
        "{BODY_MAX_BYTES} bytes"
    );

    for (doc, limit) in [
        (nested(BODY_MAX_DEPTH + 1), BodyError::TooDeep),
        (breaks(BODY_MAX_NODES - 1), BodyError::TooManyNodes),
        (letters(BODY_MAX_BYTES - frame + 1), BodyError::TooLarge),
    ] {
        let before = state_of(&doc);
        assert_eq!(to_body(&doc), Err(limit.clone()));
        assert_eq!(
            repair(&doc),
            Err(limit.clone()),
            "no repair brings it under the limit"
        );
        assert_eq!(state_of(&doc), before, "{limit}: nothing was deleted");
    }
}

/// A repair that would leave a document past a limit deletes nothing, though it found
/// what to delete.
#[test]
fn a_repair_that_would_leave_too_much_deletes_nothing() {
    let doc = written(|txn, root| {
        // Two texts that are more than a body together, and neither alone.
        paragraph(txn, root, &"a".repeat(BODY_MAX_BYTES * 3 / 5));
        root.push_back(txn, XmlElementPrelim::empty("bulletList"));
        paragraph(txn, root, &"b".repeat(BODY_MAX_BYTES * 3 / 5));
    });
    let before = state_of(&doc);
    assert_eq!(repair(&doc), Err(BodyError::TooLarge));
    assert_eq!(state_of(&doc), before, "nothing was deleted");
}

/// What goes does not count: a document within the limits but for a node that fails
/// is repaired, as an editor repairs it.
#[test]
fn nodes_that_go_do_not_count_towards_the_limit() {
    const STAYING: usize = BODY_MAX_NODES * 3 / 4;
    let doc = written(|txn, root| {
        let unknown = root.push_front(txn, XmlElementPrelim::empty("video"));
        // More nodes than a body has with the node that goes, and fewer without it.
        for _ in 0..BODY_MAX_NODES * 3 / 10 {
            unknown.push_front(txn, XmlElementPrelim::empty("paragraph"));
        }
        for _ in 0..STAYING {
            root.push_front(txn, XmlElementPrelim::empty("horizontalRule"));
        }
    });
    assert_eq!(to_body(&doc), Err(BodyError::TooManyNodes));
    let repair = repair(&doc).expect("a repair");
    assert_eq!(repair.deleted, 1, "the node nobody knows, with all it held");
    let blocks = json_of(repair.body.as_ref())["content"]
        .as_array()
        .map(Vec::len);
    assert_eq!(blocks, Some(STAYING));
}

/// An editor leaves an empty text in a paragraph it emptied. It is no node, also at
/// the deepest level a body has.
#[test]
fn an_empty_text_at_the_deepest_level_is_no_level_more() {
    let doc = written(|txn, root| {
        let mut parent: XmlElementRef = root.push_back(txn, XmlElementPrelim::empty("blockquote"));
        for _ in 3..BODY_MAX_DEPTH {
            parent = parent.push_back(txn, XmlElementPrelim::empty("blockquote"));
        }
        let line = parent.push_back(txn, XmlElementPrelim::empty("paragraph"));
        line.push_back(txn, XmlTextPrelim::new(""));
    });
    assert!(
        to_body(&doc).expect("a body").is_some(),
        "{BODY_MAX_DEPTH} levels"
    );
    assert_eq!(repair(&doc).expect("a repair").deleted, 0);
}

/// A document far past the limits is refused at the limit, without being walked to
/// its end.
#[test]
fn a_document_far_past_a_limit_is_refused_at_the_limit() {
    let deep = nested(4000);
    assert_eq!(to_body(&deep), Err(BodyError::TooDeep));
    assert_eq!(repair(&deep), Err(BodyError::TooDeep));
}

/// The body with the most nodes and the largest body are made into Y documents and
/// projected back.
#[test]
fn the_bodies_at_the_limits_go_there_and_back() {
    let breaks = vec![json!({"type": "hardBreak"}); BODY_MAX_NODES - 2];
    let most = document(&[json!({"type": "paragraph", "content": breaks})]);
    let frame = serde_json::to_vec(&document(&[text_paragraph("")]))
        .expect("JSON")
        .len();
    let largest = document(&[text_paragraph(&"a".repeat(BODY_MAX_BYTES - frame))]);
    for body in [most, largest] {
        let doc = from_body(&body_of(&body));
        let back = to_body(&doc).expect("a body");
        assert!(json_of(back.as_ref()) == body, "the same body comes back");
    }
}

/// An attribute may hold anything, also what no body may: it is refused as a value
/// that is not allowed, never taken for another.
#[test]
fn a_value_of_a_kind_no_attribute_has_is_refused() {
    for value in [
        Any::Array(Arc::from([Any::from(1.0)])),
        Any::Buffer(Arc::from([1_u8, 2])),
        Any::from(f64::NAN),
        Any::from(f64::INFINITY),
        Any::from(2.5),
        Any::Map(Arc::default()),
    ] {
        let doc = written(|txn, root| {
            let heading = root.push_back(txn, XmlElementPrelim::empty("heading"));
            heading.insert_attribute(txn, "level", value.clone());
        });
        assert!(
            matches!(to_body(&doc), Err(BodyError::InvalidNode { .. })),
            "{value:?}: {:?}",
            to_body(&doc)
        );
        assert_eq!(repair(&doc).expect("a repair").body, None, "{value:?}");
    }
    // A whole number written as a fraction is the whole number: JavaScript has one kind.
    let doc = written(|txn, root| {
        let heading = root.push_back(txn, XmlElementPrelim::empty("heading"));
        heading.insert_attribute(txn, "level", Any::from(2.0));
    });
    let body = to_body(&doc).expect("a body");
    assert_eq!(
        json_of(body.as_ref()).to_string(),
        r#"{"content":[{"attrs":{"level":2},"type":"heading"}],"type":"doc"}"#
    );
}
