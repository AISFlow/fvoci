//! Properties over bodies made from seeds: a body goes into a Y document and comes
//! back as it went (in the stored form, which is no larger), and a Y document from
//! which children were deleted at random, as a merge of edits may delete them, is
//! repaired to a body.

mod support;

use fvoci_collab::testing::{repair, to_body, with_update};
use fvoci_collab::{FIELD, from_body, from_body_by, state_of};
use serde_json::Value;
use support::random::{Random, document};
use support::{ROOMY, body_of, doc_of, json_of, stored_form};
use yrs::types::xml::XmlOut;
use yrs::{Doc, Transact, TransactionMut, XmlFragment};

/// How many bodies each property is tried on.
const CASES: u64 = 400;

#[test]
fn a_body_comes_back_as_it_went() {
    for seed in 0..CASES {
        let document = document(&mut Random::new(seed));
        let body = body_of(&document);
        let doc = from_body_by(&body, seed + 1);
        let state = state_of(&doc);

        let stored = stored_form(&document);
        let back = to_body(&doc).unwrap_or_else(|error| panic!("seed {seed}: {error}"));
        assert_eq!(json_of(back.as_ref()), stored, "seed {seed}");
        let travelled =
            to_body(&doc_of(&state)).unwrap_or_else(|error| panic!("seed {seed}: {error}"));
        assert_eq!(
            json_of(travelled.as_ref()),
            stored,
            "seed {seed}, as an update"
        );
        assert!(
            size_of(&stored) <= size_of(&document),
            "seed {seed}: no larger than it went"
        );

        assert_eq!(
            state_of(&from_body_by(&body, seed + 1)),
            state,
            "seed {seed}: made the same again"
        );
        let nothing = repair(&doc).unwrap_or_else(|error| panic!("seed {seed}: {error}"));
        assert_eq!(
            (nothing.deleted, nothing.mended, &nothing.update),
            (0, 0, &None),
            "seed {seed}: nothing to repair"
        );
        assert_eq!(
            state_of(&doc),
            state,
            "seed {seed}: a repair of a body changes nothing"
        );
    }
}

/// The size of a body as JSON, as its limit is measured.
fn size_of(json: &Value) -> usize {
    serde_json::to_vec(json).expect("JSON").len()
}

/// Every element of the document, the root's children first.
fn elements(txn: &TransactionMut<'_>, parent: &impl XmlFragment, found: &mut Vec<XmlOut>) {
    for child in parent.children(txn) {
        if let XmlOut::Element(element) = &child {
            elements(txn, element, found);
        }
        found.push(child);
    }
}

/// Deletes children at random places, as edits that are merged may: an entry of a
/// list, the paragraph an entry starts with, all a quote holds.
fn remove_at_random(doc: &Doc, random: &mut Random) {
    let root = doc.get_or_insert_xml_fragment(FIELD);
    let mut txn = doc.transact_mut();
    for _ in 0..=random.below(4) {
        let mut all = Vec::new();
        elements(&txn, &root, &mut all);
        let parents: Vec<&XmlOut> = all
            .iter()
            .filter(|node| matches!(node, XmlOut::Element(element) if element.len(&txn) > 0))
            .collect();
        if parents.is_empty() {
            return;
        }
        if let XmlOut::Element(parent) = *random.pick(&parents) {
            let children = usize::try_from(parent.len(&txn)).expect("a count of children");
            let index = u32::try_from(random.below(children)).expect("an index");
            parent.remove_range(&mut txn, index, 1);
        }
    }
}

#[test]
fn a_document_with_children_deleted_at_random_is_repaired_to_a_body() {
    let mut repaired = 0;
    let mut mended = 0;
    for seed in 0..CASES {
        let mut random = Random::new(seed);
        let document: Value = document(&mut random);
        let doc = from_body(&body_of(&document));
        remove_at_random(&doc, &mut random);
        let before = state_of(&doc);

        let repair = repair(&doc).unwrap_or_else(|error| panic!("seed {seed}: {error}"));
        let now = to_body(&doc).unwrap_or_else(|error| panic!("seed {seed}: {error}"));
        assert_eq!(
            now, repair.body,
            "seed {seed}: the document is the body given"
        );
        if repair.deleted == 0 && repair.mended == 0 {
            assert_eq!(
                state_of(&doc),
                before,
                "seed {seed}: nothing to repair, nothing changed"
            );
            continue;
        }
        repaired += 1;

        // Another holder of the document as it was, handed the changes alone, and
        // one that repairs on its own, end with the same body.
        let changes = repair.update.as_deref().expect("the changes as an update");
        let handed = with_update(&doc_of(&before), changes, &ROOMY)
            .unwrap_or_else(|error| panic!("seed {seed}: {error}"));
        assert_eq!(
            to_body(&handed),
            Ok(repair.body.clone()),
            "seed {seed}: handed the changes"
        );
        let own = doc_of(&before);
        let theirs = fvoci_collab::testing::repair(&own)
            .unwrap_or_else(|error| panic!("seed {seed}: {error}"));
        assert_eq!(theirs.body, repair.body, "seed {seed}: repaired on its own");
        // And both together: deleting what is deleted changes nothing. Not so for a
        // mend, which writes: two that mend a table each put its cells in, which is why
        // the server alone mends (`mend.rs`).
        if repair.mended == 0 {
            let both = with_update(&own, changes, &ROOMY)
                .unwrap_or_else(|error| panic!("seed {seed}: {error}"));
            assert_eq!(to_body(&both), Ok(repair.body.clone()), "seed {seed}: both");
        } else {
            mended += 1;
        }

        let again = fvoci_collab::testing::repair(&doc)
            .unwrap_or_else(|error| panic!("seed {seed}: {error}"));
        assert_eq!(
            (again.deleted, again.mended),
            (0, 0),
            "seed {seed}: once is enough"
        );
    }
    assert!(
        repaired > CASES / 10,
        "only {repaired} of {CASES} needed a repair"
    );
    assert!(
        mended > CASES / 40,
        "only {mended} of {CASES} mended a table"
    );
}
