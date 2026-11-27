//! Tests for zero-clone node-level repair (`crates/collab/src/repair.rs`, `judge.rs`,
//! `table.rs`), verifying exact parity with scratch projection/repair across deep trees,
//! 50k-node shapes, exposed `Format` inside non-root elements, whole-table grid mending,
//! and stray `tableRow` limit boundaries.

mod support;

use fvoci_collab::testing::to_body;
use fvoci_collab::{
    BodyDoc, BodyTakeError, FIELD, TakeError, from_body, from_state, state_counts, state_of,
    take_update,
};
use fvoci_model::{BODY_MAX_DEPTH, BODY_MAX_NODES, BodyError, TABLE_MAX_COLUMNS};
use serde_json::{Value, json};
use support::wire::{Change, Content, Parent, Update};
use support::{ROOMY, body_of};
use yrs::updates::decoder::Decode as _;
use yrs::{
    ClientID, Doc, OffsetKind, Options, ReadTxn as _, Transact as _, TransactionMut,
    Update as YrsUpdate, Xml as _, XmlElementPrelim, XmlFragment as _, XmlFragmentRef, XmlOut,
    XmlTextPrelim,
};

fn peer_edit(
    state: &[u8],
    client: u64,
    edit: impl FnOnce(&mut TransactionMut<'_>, &XmlFragmentRef),
) -> Vec<u8> {
    let options = Options {
        offset_kind: OffsetKind::Utf16,
        skip_gc: true,
        ..Options::with_client_id(ClientID::new(client))
    };
    let peer = Doc::with_options(options);
    peer.get_or_insert_xml_fragment(FIELD);
    let initial = YrsUpdate::decode_v1(state).expect("state decodes");
    peer.transact_mut()
        .apply_update(initial)
        .expect("state applies");
    let before = peer.transact().state_vector();
    {
        let root = peer.get_or_insert_xml_fragment(FIELD);
        let mut txn = peer.transact_mut();
        edit(&mut txn, &root);
    }
    peer.transact().encode_diff_v1(&before)
}

fn assert_repair_parity(state: &[u8], update: &[u8], label: &str) {
    let changes = state_counts(state).expect("counts").changes;
    let ref_doc = from_state(state, &ROOMY).expect("ref open").doc;
    let expected = take_update(&ref_doc, update, &ROOMY);
    let held = BodyDoc::from_state(state, &ROOMY).expect("held open").doc;
    let got = held.take_in_place(None, changes, update, &ROOMY);
    match (got, expected) {
        (Ok(got), Ok(expected)) => {
            let got_state = got.state.unwrap_or_else(|| got.doc.state());
            assert_eq!(got_state, expected.state, "{label}: state");
            assert_eq!(
                got.state_bytes, expected.state_bytes,
                "{label}: state_bytes"
            );
            assert_eq!(got.changes, expected.changes, "{label}: changes");
            assert_eq!(
                got.client_changes, expected.client_changes,
                "{label}: client_changes"
            );
            assert_eq!(
                got.reading_steps, expected.reading_steps,
                "{label}: reading_steps"
            );
            assert_eq!(
                got.steps_ahead, expected.steps_ahead,
                "{label}: steps_ahead"
            );
            assert_eq!(
                got.repair.deleted, expected.repair.deleted,
                "{label}: repair.deleted"
            );
            assert_eq!(
                got.repair.mended, expected.repair.mended,
                "{label}: repair.mended"
            );
            assert_eq!(
                got.repair.update, expected.repair.update,
                "{label}: repair.update"
            );
            assert_eq!(
                got.doc.body().expect("body"),
                expected.repair.body,
                "{label}: body"
            );
            let expected_files: Vec<String> = expected
                .repair
                .body
                .as_ref()
                .map(|b| b.file_ids().into_iter().map(str::to_owned).collect())
                .unwrap_or_default();
            assert_eq!(got.doc.file_ids(), expected_files, "{label}: file_ids");
        }
        (Err(BodyTakeError::Untouched { error, .. } | BodyTakeError::Dirty(error)), Err(exp)) => {
            assert_eq!(error, exp, "{label}: error mismatch");
        }
        (got, expected) => panic!("{label}: outcome mismatch: got={got:?}, expected={expected:?}"),
    }
}

/// Deep blockquote chain at `BODY_MAX_DEPTH` with an invalid inner child repaired in place
/// at the node level (keeping all valid ancestors intact without cloning subtrees).
#[test]
fn deep_blockquote_chain_repairs_inner_invalid_child_in_place() {
    // 58 nested blockquotes -> paragraph (depth 60) -> empty.
    let mut inner = json!({"type": "paragraph", "content": [{"type": "text", "text": "leaf"}]});
    for _ in 0..(BODY_MAX_DEPTH - 3) {
        inner = json!({"type": "blockquote", "content": [inner]});
    }
    let doc_json = json!({"type": "doc", "content": [inner]});
    let doc = from_body(&body_of(&doc_json));
    let state = state_of(&doc);

    // Insert an invalid sibling next to the deepest paragraph (inside the deepest blockquote).
    let update = peer_edit(&state, 42, |txn, root| {
        let Some(XmlOut::Element(mut cur)) = root.first_child() else {
            panic!("root blockquote");
        };
        for _ in 0..(BODY_MAX_DEPTH - 4) {
            let Some(XmlOut::Element(next)) = cur.first_child() else {
                panic!("nested blockquote");
            };
            cur = next;
        }
        cur.push_back(txn, XmlElementPrelim::empty("invalid-deep-node"));
    });
    assert_repair_parity(&state, &update, "deep blockquote inner node repair");
}

/// Large 10,000-node paragraph with `hardBreak` children repairs one invalid inline child
/// at the node level without quadratic cloning and agrees byte-for-byte with scratch.
#[test]
fn large_breaks_paragraph_repairs_single_invalid_inline_child() {
    let mut content: Vec<Value> = Vec::with_capacity(10_000);
    for _ in 0..10_000 {
        content.push(json!({"type": "hardBreak"}));
    }
    let doc_json = json!({
        "type": "doc",
        "content": [{"type": "paragraph", "content": content}],
    });
    let doc = from_body(&body_of(&doc_json));
    let state = state_of(&doc);

    let update = peer_edit(&state, 55, |txn, root| {
        let Some(XmlOut::Element(para)) = root.first_child() else {
            panic!("paragraph");
        };
        para.insert(txn, 5_000, XmlElementPrelim::empty("bogus-inline"));
    });
    assert_repair_parity(
        &state,
        &update,
        "large breaks paragraph inner inline repair",
    );
}

/// Regression for cutting index 0 inside a non-root element `[blockquote: [InvalidPara,
/// Format, ValidPara]]`: deleting `InvalidPara` exposes non-countable `ItemContent::Format`
/// at `blockquote.first_child()`, so `refresh_cache_after_repair` must refuse identically to
/// scratch `from_state` and `take_update`.
#[test]
fn exposed_format_at_index_zero_inside_non_root_element_cannot_bypass_repair() {
    let format_bold = Content::Format(b"bold".to_vec(), b"true".to_vec());
    let raw = Update::of(
        10,
        0,
        vec![
            // (10, 0) blockquote under root
            Change::under(Parent::body(), Content::element("blockquote")),
            // (10, 1) invalid child at index 0 of blockquote
            Change::under(Parent::Change((10, 0)), Content::element("bogus-child")),
            // (10, 2) non-countable Format right after bogus-child
            Change::after((10, 1), format_bold),
            // (10, 3) valid paragraph right after Format
            Change::after((10, 2), Content::element("paragraph")),
        ],
    )
    .bytes();

    let ref_open = from_state(&raw, &ROOMY).expect_err("scratch from_state refuses");
    let got_open = BodyDoc::from_state(&raw, &ROOMY).expect_err("BodyDoc::from_state refuses");
    assert_eq!(got_open, ref_open, "from_state error parity");

    let empty = BodyDoc::new().state();
    assert_repair_parity(
        &empty,
        &raw,
        "take_in_place repair exposing Format inside blockquote",
    );
}

/// Whole-table repair preserves `colspan`/`rowspan` grid mending (`Mend::Put`, `Mend::Pad`,
/// `Mend::Spans`), kept cell inner `file_ids`, and `tableRow` empty `XmlText` cleanup.
#[test]
fn whole_table_repair_mends_grid_preserves_cell_files_and_cleans_empty_row_text() {
    let base_json = json!({
        "type": "doc",
        "content": [{
            "type": "table",
            "content": [
                {
                    "type": "tableRow",
                    "content": [
                        {
                            "type": "tableHeader",
                            "attrs": {"colspan": 2},
                            "content": [
                                {"type": "paragraph", "content": [{"type": "text", "text": "H"}]},
                                {"type": "image", "attrs": {"fileId": "cell-header.png"}}
                            ]
                        }
                    ]
                },
                {
                    "type": "tableRow",
                    "content": [
                        {
                            "type": "tableCell",
                            "content": [
                                {"type": "file", "attrs": {"fileId": "cell-left.pdf"}}
                            ]
                        },
                        {
                            "type": "tableCell",
                            "content": [
                                {"type": "paragraph", "content": [{"type": "text", "text": "ok"}]}
                            ]
                        }
                    ]
                }
            ]
        }]
    });
    let doc = from_body(&body_of(&base_json));
    let state = state_of(&doc);

    // Concurrently: (1) empty a cell in row 1 so Mend::Put replaces it, (2) add a short row
    // so Mend::Pad pads it, (3) widen header colspan past table width so Mend::Spans clamps it,
    // and (4) insert an empty XmlText into row 0.
    let update = peer_edit(&state, 77, |txn, root| {
        let Some(XmlOut::Element(table)) = root.first_child() else {
            panic!("table");
        };
        let Some(XmlOut::Element(row0)) = table.get(txn, 0) else {
            panic!("row0");
        };
        row0.insert(txn, 0, XmlTextPrelim::new(""));
        if let Some(XmlOut::Element(header)) = row0.get(txn, 1) {
            header.insert_attribute(txn, "colspan", 5_i64);
        }
        let Some(XmlOut::Element(row1)) = table.get(txn, 1) else {
            panic!("row1");
        };
        if let Some(XmlOut::Element(cell1)) = row1.get(txn, 1) {
            cell1.remove_range(txn, 0, 1);
        }
        let row2 = table.push_back(txn, XmlElementPrelim::empty("tableRow"));
        let c = row2.push_back(txn, XmlElementPrelim::empty("tableCell"));
        c.push_back(txn, XmlElementPrelim::empty("paragraph"));
    });

    assert_repair_parity(
        &state,
        &update,
        "whole table mend + cell files + empty row text",
    );
}

/// A standalone `tableRow` outside a `table` with more than `TABLE_MAX_COLUMNS` cells
/// raises `BodyError::TableTooLarge` (matching scratch `passes(..., Place::TableRow)`).
#[test]
fn stray_table_row_wider_than_table_max_columns_returns_table_too_large() {
    let base = BodyDoc::new().state();
    let update = peer_edit(&base, 88, |txn, root| {
        let row = root.push_back(txn, XmlElementPrelim::empty("tableRow"));
        for _ in 0..=TABLE_MAX_COLUMNS {
            let cell = row.push_back(txn, XmlElementPrelim::empty("tableCell"));
            cell.push_back(txn, XmlElementPrelim::empty("paragraph"));
        }
    });
    let expected = take_update(&from_state(&base, &ROOMY).unwrap().doc, &update, &ROOMY);
    assert!(
        matches!(
            expected,
            Err(TakeError::Body(BodyError::TableTooLarge { .. }))
        ),
        "expected TableTooLarge, got {expected:?}"
    );
    assert_repair_parity(&base, &update, "stray tableRow > TABLE_MAX_COLUMNS");
}

/// A `tableRow` inside a `table` that exceeds BOTH `BODY_MAX_NODES` and `TABLE_MAX_COLUMNS`
/// preserves exact `Body::parse` error precedence (`TooManyNodes` before `TableTooLarge`).
#[test]
fn table_row_limit_precedence_matches_body_parse() {
    let base = BodyDoc::new().state();
    let update = peer_edit(&base, 89, |txn, root| {
        let table = root.push_back(txn, XmlElementPrelim::empty("table"));
        let row = table.push_back(txn, XmlElementPrelim::empty("tableRow"));
        // 21 cells, first cell holding BODY_MAX_NODES horizontalRules -> hits TooManyNodes first.
        let first = row.push_back(txn, XmlElementPrelim::empty("tableCell"));
        for _ in 0..BODY_MAX_NODES {
            first.push_back(txn, XmlElementPrelim::empty("horizontalRule"));
        }
        for _ in 0..TABLE_MAX_COLUMNS {
            let cell = row.push_back(txn, XmlElementPrelim::empty("tableCell"));
            cell.push_back(txn, XmlElementPrelim::empty("paragraph"));
        }
    });
    let ref_doc = from_state(&base, &ROOMY).unwrap().doc;
    let ref_err = take_update(&ref_doc, &update, &ROOMY).expect_err("exceeds limits");
    assert_eq!(ref_err, TakeError::Body(BodyError::TooManyNodes));
    let _ = to_body(&ref_doc);
    assert_repair_parity(&base, &update, "tableRow TooManyNodes before TableTooLarge");
}

/// If root child 0 fails a limit check (`TooDeep`) and a later root child is a raw non-XML
/// item (`String`), `Survey::root_cached` still returns `BodyError::InvalidNode` first,
/// matching `read::children`'s full root scan precedence.
#[test]
fn corrupt_root_non_xml_item_takes_precedence_over_earlier_child_limit_error() {
    use support::wire::{Change, Content, Parent, Update};

    let mut base_changes = vec![Change::under(
        Parent::body(),
        Content::element("blockquote"),
    )];
    for depth in 0..=fvoci_model::BODY_MAX_DEPTH {
        let parent_clock = u64::try_from(depth).expect("small depth");
        base_changes.push(Change::under(
            Parent::Change((10, parent_clock)),
            Content::element("blockquote"),
        ));
    }
    let last_clock = u64::try_from(fvoci_model::BODY_MAX_DEPTH + 1).expect("small depth");
    base_changes.push(Change::under(
        Parent::Change((10, last_clock)),
        Content::element("paragraph"),
    ));
    // Trailing raw string at root (neither XmlElement nor XmlText).
    base_changes.push(Change::Item {
        left: Some((10, 0)),
        right: None,
        parent: Parent::body(),
        key: None,
        content: Content::Text(b"raw-non-xml".to_vec()),
    });
    let update = Update::of(10, 0, base_changes).bytes();
    let base = BodyDoc::new().state();
    let ref_doc = from_state(&base, &ROOMY).unwrap().doc;
    let expected = take_update(&ref_doc, &update, &ROOMY).expect_err("corrupt root");
    assert!(
        matches!(expected, TakeError::Body(BodyError::InvalidNode { .. })),
        "expected InvalidNode, got {expected:?}"
    );
    assert_repair_parity(&base, &update, "corrupt root precedes TooDeep child");
}
