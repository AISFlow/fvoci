//! Equivalence and property tests verifying that incremental top-level block caching
//! (`BodyDoc::take_in_place` / `BodyDoc::from_state` / `BodyDoc::file_ids`) matches full
//! scratch projection (`take_update` / `to_body` / `Body::file_ids`) byte-for-byte,
//! JSON-for-JSON, and error-for-error across edits, repairs, refusals, settles, and
//! adversarial wire updates.

mod support;

use std::sync::Arc;

use fvoci_collab::testing::{Editor, to_body};
use fvoci_collab::{
    BodyDoc, BodyTakeError, FIELD, TakeError, UpdateLimits, from_state, new_doc, state_counts,
    state_of, take_update,
};
use fvoci_model::{
    BODY_MAX_BYTES, BODY_MAX_DEPTH, BODY_MAX_NODES, Body, BodyError, TABLE_MAX_COLUMNS,
    TABLE_MAX_ROWS,
};
use serde_json::json;
use support::random::{self, Random};
use support::wire::{Change, Content, Parent, Update, value};
use support::{ROOMY, body_of};
use yrs::types::Attrs;
use yrs::updates::decoder::Decode as _;
use yrs::{
    Any, Array as _, ArrayRef, ClientID, Doc, OffsetKind, Options, ReadTxn as _, Text as _,
    Transact as _, TransactionMut, Update as YrsUpdate, Xml, XmlElementPrelim, XmlFragment as _,
    XmlFragmentRef, XmlOut, XmlTextPrelim,
};

fn expected_files(body: Option<&Body>) -> Vec<String> {
    body.map_or_default(|body| body.file_ids().into_iter().map(str::to_owned).collect())
}

fn expected_mentions(body: Option<&Body>) -> Vec<String> {
    body.map_or_default(|body| body.mention_ids().into_iter().map(str::to_owned).collect())
}

fn json_bytes(body: Option<&Body>) -> Option<Vec<u8>> {
    body.map(|body| serde_json::to_vec(body.document()).expect("body is JSON"))
}

/// Applies `update` to both `held` (`BodyDoc::take_in_place`, incremental cache) and a
/// fresh scratch reference (`take_update` + full `to_body`), asserting 100% equivalence on
/// success (state, repair update/deleted/mended, JSON bytes, `file_ids`, `mention_ids`,
/// counts, `remake`) and on refusal (exact `TakeError` match + clean cache preservation
/// or rollback).
fn assert_step_equivalent(
    held: BodyDoc,
    state: &mut Vec<u8>,
    changes: &mut u64,
    update: &[u8],
    limits: &UpdateLimits,
    context: &str,
) -> BodyDoc {
    let ref_doc = from_state(state, &ROOMY).expect("ref state opens").doc;
    let ref_result = take_update(&ref_doc, update, limits);

    match (
        held.take_in_place(None, *changes, update, limits),
        ref_result,
    ) {
        (Ok(got), Ok(expected)) => {
            let got_state = got.state.unwrap_or_else(|| got.doc.state());
            assert_eq!(got_state, expected.state, "{context}: state bytes");
            assert_eq!(got.changes, expected.changes, "{context}: changes");
            assert_eq!(
                got.client_changes, expected.client_changes,
                "{context}: client_changes"
            );
            assert_eq!(
                got.state_bytes,
                got_state.len(),
                "{context}: cached state_bytes == encoded len"
            );
            assert_eq!(
                got.state_bytes, expected.state_bytes,
                "{context}: state_bytes"
            );
            assert_eq!(
                got.reading_steps, expected.reading_steps,
                "{context}: reading_steps"
            );
            assert_eq!(
                got.steps_ahead, expected.steps_ahead,
                "{context}: steps_ahead"
            );
            assert_eq!(got.remake, expected.remake, "{context}: remake");
            assert_eq!(
                got.repair.deleted, expected.repair.deleted,
                "{context}: repair.deleted"
            );
            assert_eq!(
                got.repair.mended, expected.repair.mended,
                "{context}: repair.mended"
            );
            assert_eq!(
                got.repair.update, expected.repair.update,
                "{context}: repair.update"
            );
            let got_body = got.doc.body().expect("incremental doc projects");
            assert_eq!(
                json_bytes(got_body.as_ref()),
                json_bytes(expected.repair.body.as_ref()),
                "{context}: projected JSON bytes"
            );
            assert_eq!(
                got.doc.file_ids(),
                expected_files(expected.repair.body.as_ref()),
                "{context}: cached file_ids"
            );
            assert_eq!(
                expected_mentions(got_body.as_ref()),
                expected_mentions(expected.repair.body.as_ref()),
                "{context}: mention_ids"
            );
            *state = got_state;
            *changes = got.changes;
            got.doc
        }
        (Err(BodyTakeError::Untouched { doc, error }), Err(expected_err)) => {
            assert_eq!(error, expected_err, "{context}: untouched error");
            assert_eq!(doc.state(), *state, "{context}: untouched state");
            let ref_body = to_body(&ref_doc).expect("ref body");
            assert_eq!(
                doc.file_ids(),
                expected_files(ref_body.as_ref()),
                "{context}: untouched cache file_ids intact"
            );
            doc
        }
        (Err(BodyTakeError::Dirty(error)), Err(expected_err)) => {
            assert_eq!(error, expected_err, "{context}: dirty error");
            let rebuilt = BodyDoc::rebuild::<&[u8]>(state, &[]).expect("rebuild");
            assert_eq!(rebuilt.state(), *state, "{context}: rebuilt state");
            rebuilt
        }
        (got, expected) => {
            panic!("{context}: outcome mismatch: got {got:?}, expected {expected:?}");
        }
    }
}

/// Builds an editor update against `state` using client `client` that applies `edit`.
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

/// Inserts a rich block of kind `choice` into `root` at `at` (covering paragraphs,
/// headings, nested lists, task lists, blockquotes, mermaid code blocks, horizontal rules,
/// images, files, blockMath, inlineMath, mentions, references, marks, tables, and repairs).
fn insert_rich_block(
    txn: &mut TransactionMut<'_>,
    root: &XmlFragmentRef,
    at: u32,
    choice: usize,
    seq: usize,
) {
    match choice % 12 {
        0 => {
            let p = root.insert(txn, at, XmlElementPrelim::empty("paragraph"));
            let t = p.push_back(txn, XmlTextPrelim::new(format!("para-{seq} 한글😀 ")));
            let bold = Attrs::from([(Arc::from("bold"), Any::Map(Arc::default()))]);
            t.format(txn, 0, 4, bold);
            let m = p.push_back(txn, XmlElementPrelim::empty("mention"));
            m.insert_attribute(txn, "userId", "01930000-0000-7000-8000-00000000c001");
            let im = p.push_back(txn, XmlElementPrelim::empty("inlineMath"));
            im.insert_attribute(txn, "latex", "x^2+y^2");
        }
        1 => {
            let h = root.insert(txn, at, XmlElementPrelim::empty("heading"));
            h.insert_attribute(txn, "level", i64::try_from((seq % 3) + 1).expect("1..=3"));
            h.push_back(txn, XmlTextPrelim::new(format!("heading-{seq}")));
        }
        2 => {
            let list = root.insert(txn, at, XmlElementPrelim::empty("bulletList"));
            let item = list.push_back(txn, XmlElementPrelim::empty("listItem"));
            let p = item.push_back(txn, XmlElementPrelim::empty("paragraph"));
            p.push_back(txn, XmlTextPrelim::new(format!("item-{seq}")));
            let nested = item.push_back(txn, XmlElementPrelim::empty("orderedList"));
            nested.insert_attribute(txn, "start", if seq.is_multiple_of(2) { 1_i64 } else { 5 });
            let sub = nested.push_back(txn, XmlElementPrelim::empty("listItem"));
            let sp = sub.push_back(txn, XmlElementPrelim::empty("paragraph"));
            sp.push_back(txn, XmlTextPrelim::new("nested"));
        }
        3 => {
            let tl = root.insert(txn, at, XmlElementPrelim::empty("taskList"));
            let ti = tl.push_back(txn, XmlElementPrelim::empty("taskItem"));
            ti.insert_attribute(txn, "checked", seq.is_multiple_of(2));
            let p = ti.push_back(txn, XmlElementPrelim::empty("paragraph"));
            p.push_back(txn, XmlTextPrelim::new(format!("task-{seq}")));
        }
        4 => {
            let bq = root.insert(txn, at, XmlElementPrelim::empty("blockquote"));
            let p = bq.push_back(txn, XmlElementPrelim::empty("paragraph"));
            p.push_back(txn, XmlTextPrelim::new(format!("quote-{seq}")));
            let img = bq.push_back(txn, XmlElementPrelim::empty("image"));
            img.insert_attribute(txn, "fileId", format!("nested-{}.png", seq % 4));
            img.insert_attribute(txn, "alt", format!("alt-{seq}"));
        }
        5 => {
            let cb = root.insert(txn, at, XmlElementPrelim::empty("codeBlock"));
            cb.insert_attribute(txn, "language", "mermaid");
            cb.push_back(txn, XmlTextPrelim::new("graph TD\n  A-->B"));
        }
        6 => {
            root.insert(txn, at, XmlElementPrelim::empty("horizontalRule"));
        }
        7 => {
            let img = root.insert(txn, at, XmlElementPrelim::empty("image"));
            img.insert_attribute(txn, "fileId", format!("pic-{}.png", seq % 5));
        }
        8 => {
            let file = root.insert(txn, at, XmlElementPrelim::empty("file"));
            file.insert_attribute(txn, "fileId", format!("doc-{}.pdf", seq % 5));
        }
        9 => {
            let math = root.insert(txn, at, XmlElementPrelim::empty("blockMath"));
            math.insert_attribute(txn, "latex", format!("\\sum_{{i=1}}^{{{seq}}} i"));
        }
        10 => {
            let table = root.insert(txn, at, XmlElementPrelim::empty("table"));
            for r in 0..2 {
                let row = table.push_back(txn, XmlElementPrelim::empty("tableRow"));
                for c in 0..2 {
                    let tag = if r == 0 { "tableHeader" } else { "tableCell" };
                    let cell = row.push_back(txn, XmlElementPrelim::empty(tag));
                    let p = cell.push_back(txn, XmlElementPrelim::empty("paragraph"));
                    p.push_back(txn, XmlTextPrelim::new(format!("r{r}c{c}-{seq}")));
                }
            }
        }
        _ => {
            // Triggers repair: unknown node + invalid heading level or bad link.
            if seq.is_multiple_of(3) {
                root.insert(txn, at, XmlElementPrelim::empty("unknown-widget"));
            } else if seq % 3 == 1 {
                let bad_h = root.insert(txn, at, XmlElementPrelim::empty("heading"));
                bad_h.insert_attribute(txn, "level", 9_i64);
                bad_h.push_back(txn, XmlTextPrelim::new("bad heading"));
            } else {
                root.insert(txn, at, XmlTextPrelim::new("stray root text"));
            }
        }
    }
}

/// Mutates an existing block in place (text insert/delete/format, attribute edit, table
/// row/column/mend, or deleting a block) so existing cached entries are dirtied in place.
fn mutate_existing_block(
    txn: &mut TransactionMut<'_>,
    root: &XmlFragmentRef,
    rng: &mut Random,
    seq: usize,
) {
    let len = root.len(txn);
    if len == 0 {
        insert_rich_block(txn, root, 0, seq, seq);
        return;
    }
    let idx = u32::try_from(rng.below(usize::try_from(len).expect("len"))).expect("u32");
    match rng.below(5) {
        0 => {
            root.remove_range(txn, idx, 1);
        }
        1 => {
            // Insert a new block at front, middle, or back.
            let at = match rng.below(3) {
                0 => 0,
                1 => idx,
                _ => len,
            };
            insert_rich_block(txn, root, at, rng.below(12), seq);
        }
        2 => {
            // Mutate text or marks inside an existing element if it has an XmlText child.
            if let Some(XmlOut::Element(el)) = root.get(txn, idx) {
                match el.tag().as_ref() {
                    "paragraph" | "heading" => {
                        if let Some(XmlOut::Text(text)) = el.first_child() {
                            let end = text.len(txn);
                            if end > 0 && rng.chance(2) {
                                let italic =
                                    Attrs::from([(Arc::from("italic"), Any::Map(Arc::default()))]);
                                text.format(txn, 0, end.min(3), italic);
                            } else {
                                text.insert(txn, end, &format!("+{seq}"));
                            }
                        } else {
                            el.push_back(txn, XmlTextPrelim::new(format!("added-{seq}")));
                        }
                    }
                    "image" => {
                        el.insert_attribute(txn, "fileId", format!("updated-{}.png", seq % 5));
                    }
                    "table" => {
                        // Add a short 1-cell row to force table grid mending (`Mend::Pad`).
                        let row = el.push_back(txn, XmlElementPrelim::empty("tableRow"));
                        let cell = row.push_back(txn, XmlElementPrelim::empty("tableCell"));
                        cell.push_back(txn, XmlElementPrelim::empty("paragraph"));
                    }
                    "taskList" => {
                        if let Some(XmlOut::Element(item)) = el.first_child() {
                            item.insert_attribute(txn, "checked", seq.is_multiple_of(2));
                        }
                    }
                    _ => insert_rich_block(txn, root, idx, rng.below(12), seq),
                }
            }
        }
        3 => {
            // Multi-block edit touching two separate blocks in one transaction.
            insert_rich_block(txn, root, 0, 7, seq);
            let new_len = root.len(txn);
            insert_rich_block(txn, root, new_len, 8, seq + 1);
        }
        _ => {
            // Bad link inside an existing or new paragraph (triggers in-block repair).
            let p = root.insert(txn, idx, XmlElementPrelim::empty("paragraph"));
            let t = p.push_back(txn, XmlTextPrelim::new("click"));
            let mut link_attrs = std::collections::HashMap::new();
            link_attrs.insert("href".to_owned(), Any::from("javascript:alert(1)"));
            let bad_link = Attrs::from([(Arc::from("link"), Any::Map(Arc::new(link_attrs)))]);
            t.format(txn, 0, 5, bad_link);
        }
    }
}

/// Property test: across 20 seeds × 25 rich multi-client steps (inserts, deletes, marks,
/// nested lists, task lists, blockquotes, mermaid diagrams, math, images, files, tables,
/// repairs, mends, refusals, rebuilds, and settles), incremental `BodyDoc::take_in_place`
/// agrees 100% with full scratch `take_update` + `to_body` at every single step.
#[test]
fn incremental_block_cache_matches_full_projection_across_rich_edit_sequences() {
    for seed in 1..=20_u64 {
        let mut rng = Random::new(seed * 97 + 13);
        let initial = body_of(&random::document(&mut rng));
        // Alternate cold-open via `from_body` vs cached-open via `from_state`.
        let mut held = if seed.is_multiple_of(2) {
            let base_state = BodyDoc::from_body(Some(&initial)).state();
            BodyDoc::from_state(&base_state, &ROOMY)
                .expect("from_state")
                .doc
        } else {
            BodyDoc::from_body(Some(&initial))
        };
        let mut state = held.state();
        let mut changes = state_counts(&state).expect("counts").changes;

        for step in 0..25_usize {
            let ctx = format!("seed {seed} step {step}");
            // Every 5th step, merge two concurrent client updates into one.
            let update = if step % 5 == 4 {
                let u1 = peer_edit(&state, 100 + seed, |txn, root| {
                    mutate_existing_block(txn, root, &mut rng, step * 2);
                });
                let u2 = peer_edit(&state, 200 + seed, |txn, root| {
                    mutate_existing_block(txn, root, &mut rng, step * 2 + 1);
                });
                yrs::merge_updates_v1([&u1, &u2]).expect("merge")
            } else {
                peer_edit(&state, 300 + seed, |txn, root| {
                    mutate_existing_block(txn, root, &mut rng, step);
                })
            };

            held = assert_step_equivalent(held, &mut state, &mut changes, &update, &ROOMY, &ctx);

            // Periodically exercise refusals (Untouched and Dirty) and `settle`.
            match step % 8 {
                2 => {
                    // Pre-apply Untouched refusal: invalid bytes leave cache untouched.
                    held = assert_step_equivalent(
                        held,
                        &mut state,
                        &mut changes,
                        &[0xff, 0x01, 0x02],
                        &ROOMY,
                        &format!("{ctx} untouched-junk"),
                    );
                }
                4 => {
                    // Post-apply Dirty refusal: tight body_bytes must report exact size.
                    let grow = Editor::join(500 + seed, &state)
                        .append_paragraph("exceeds tight body_bytes limit");
                    let tight = UpdateLimits {
                        body_bytes: 5,
                        ..ROOMY
                    };
                    held = assert_step_equivalent(
                        held,
                        &mut state,
                        &mut changes,
                        &grow,
                        &tight,
                        &format!("{ctx} dirty-body-too-large"),
                    );
                }
                6 => {
                    // Settle at fold/close and continue editing incrementally afterward.
                    let settled = held.settle(state.clone(), &ROOMY).expect("settle");
                    state = settled.state;
                    changes = settled.changes;
                    held = settled.doc;
                }
                _ => {}
            }
        }
    }
}

/// Regression test for the `tableRow` empty-`XmlText` subtlety: a clean table containing
/// an empty `XmlText` inside a `tableRow` passes `to_body`, but `repair::Survey::row`
/// deletes that empty text whenever ANY repair runs on the document. The incremental
/// cache must preserve `has_empty_row_text` and match full `take_update` byte-for-byte.
#[test]
fn clean_table_with_empty_row_text_matches_full_repair_when_another_block_fails() {
    let doc = new_doc();
    {
        let root = doc.get_or_insert_xml_fragment(FIELD);
        let mut txn = doc.transact_mut();
        let table = root.push_back(&mut txn, XmlElementPrelim::empty("table"));
        let row = table.push_back(&mut txn, XmlElementPrelim::empty("tableRow"));
        row.push_back(&mut txn, XmlTextPrelim::new(""));
        let cell = row.push_back(&mut txn, XmlElementPrelim::empty("tableCell"));
        let p = cell.push_back(&mut txn, XmlElementPrelim::empty("paragraph"));
        p.push_back(&mut txn, XmlTextPrelim::new("cell"));
        let p2 = root.push_back(&mut txn, XmlElementPrelim::empty("paragraph"));
        p2.push_back(&mut txn, XmlTextPrelim::new("second"));
    }
    let mut state = state_of(&doc);
    let mut changes = state_counts(&state).expect("counts").changes;
    let opened = BodyDoc::from_state(&state, &ROOMY).expect("valid initial state");
    assert_eq!(opened.repair.deleted, 0, "empty row text passes to_body");
    let mut held = opened.doc;

    // 1. Ordinary edit to another paragraph leaves the clean table untouched.
    let edit1 = peer_edit(&state, 10, |txn, root| {
        let p = root.push_back(txn, XmlElementPrelim::empty("paragraph"));
        p.push_back(txn, XmlTextPrelim::new("ordinary"));
    });
    held = assert_step_equivalent(held, &mut state, &mut changes, &edit1, &ROOMY, "ordinary");

    // 2. Adding an invalid block at the end triggers repair: full `Survey::row` deletes
    //    the empty `XmlText` in the untouched table AND deletes the unknown block!
    let edit2 = peer_edit(&state, 11, |txn, root| {
        root.push_back(txn, XmlElementPrelim::empty("nothing-a-body-has"));
    });
    let _held = assert_step_equivalent(
        held,
        &mut state,
        &mut changes,
        &edit2,
        &ROOMY,
        "repair deletes both unknown block and clean table empty row text",
    );
}

/// Verifies `file_ids` document-order preservation across insertions before/after earlier
/// clients, duplicates across blocks, in-place `fileId` edits, and deletions, plus empty
/// document transitions (0 blocks, 1 empty paragraph with `body_bytes: 1`, 2 empty paragraphs).
#[test]
fn file_ids_document_order_and_empty_document_transitions_match_full_projection() {
    let mut held = BodyDoc::new();
    let mut state = held.state();
    let mut changes = state_counts(&state).expect("counts").changes;

    // 1. Insert 1 empty paragraph with body_bytes=1: empty doc (`None`) must still pass!
    let empty_para = peer_edit(&state, 10, |txn, root| {
        root.push_back(txn, XmlElementPrelim::empty("paragraph"));
    });
    let tiny = UpdateLimits {
        body_bytes: 1,
        ..ROOMY
    };
    held = assert_step_equivalent(
        held,
        &mut state,
        &mut changes,
        &empty_para,
        &tiny,
        "one empty paragraph is None even with body_bytes=1",
    );

    // 2. A second empty paragraph makes a non-empty 68-byte body: fails body_bytes=67,
    //    passes body_bytes=68.
    let second_empty = peer_edit(&state, 11, |txn, root| {
        root.push_back(txn, XmlElementPrelim::empty("paragraph"));
    });
    held = assert_step_equivalent(
        held,
        &mut state,
        &mut changes,
        &second_empty,
        &UpdateLimits {
            body_bytes: 67,
            ..ROOMY
        },
        "two empty paragraphs refused at 67 bytes",
    );
    held = assert_step_equivalent(
        held,
        &mut state,
        &mut changes,
        &second_empty,
        &UpdateLimits {
            body_bytes: 68,
            ..ROOMY
        },
        "two empty paragraphs accepted at 68 bytes",
    );

    // 3. Insert images/files out of client-ID order (higher client ID at index 0!) to
    //    prove `file_ids` follows document order, not `BTreeMap<ID, _>` key order.
    let add_b = peer_edit(&state, 5, |txn, root| {
        let img = root.push_back(txn, XmlElementPrelim::empty("image"));
        img.insert_attribute(txn, "fileId", "b.png");
    });
    held = assert_step_equivalent(held, &mut state, &mut changes, &add_b, &ROOMY, "add b");

    let prepend_a = peer_edit(&state, 999, |txn, root| {
        let f = root.push_front(txn, XmlElementPrelim::empty("file"));
        f.insert_attribute(txn, "fileId", "a.pdf");
    });
    held = assert_step_equivalent(
        held,
        &mut state,
        &mut changes,
        &prepend_a,
        &ROOMY,
        "prepend a with higher client id",
    );
    assert_eq!(held.file_ids(), vec!["a.pdf", "b.png"]);

    // 4. Duplicate fileId in a third block + delete all blocks back to empty root.
    let dup_a = peer_edit(&state, 20, |txn, root| {
        let img = root.push_back(txn, XmlElementPrelim::empty("image"));
        img.insert_attribute(txn, "fileId", "a.pdf");
    });
    held = assert_step_equivalent(held, &mut state, &mut changes, &dup_a, &ROOMY, "dup a");
    assert_eq!(held.file_ids(), vec!["a.pdf", "b.png"]);

    let clear_all = peer_edit(&state, 21, |txn, root| {
        let len = root.len(txn);
        root.remove_range(txn, 0, len);
    });
    held = assert_step_equivalent(
        held,
        &mut state,
        &mut changes,
        &clear_all,
        &tiny,
        "clear back to empty root",
    );
    assert_eq!(held.file_ids(), Vec::<String>::new());
    assert_eq!(held.body().expect("empty body"), None);
}

/// Verifies exact error equivalence on node, depth, byte, and root-value limits, including
/// the case where deleting an invalid block brings a document back under `BODY_MAX_NODES`.
#[test]
fn limit_boundaries_and_invalid_root_values_match_full_projection() {
    // Document at BODY_MAX_NODES - 1 nodes (1 doc + 99,998 horizontalRules = 99,999).
    let near_full = json!({
        "type": "doc",
        "content": vec![json!({"type": "horizontalRule"}); BODY_MAX_NODES - 2],
    });
    let base_doc = BodyDoc::from_body(Some(&body_of(&near_full)));
    let mut state = base_doc.state();
    let mut changes = state_counts(&state).expect("counts").changes;
    let mut held = BodyDoc::from_state(&state, &ROOMY).expect("open").doc;

    // Adding 2 valid horizontalRules exceeds BODY_MAX_NODES (100,001 > 100,000) -> TooManyNodes.
    let over_nodes = peer_edit(&state, 10, |txn, root| {
        root.push_back(txn, XmlElementPrelim::empty("horizontalRule"));
        root.push_back(txn, XmlElementPrelim::empty("horizontalRule"));
    });
    held = assert_step_equivalent(
        held,
        &mut state,
        &mut changes,
        &over_nodes,
        &ROOMY,
        "over BODY_MAX_NODES",
    );

    // Adding 1 valid horizontalRule + 2 invalid nodes (50,002 raw nodes, 50,000 after repair)
    // must repair and succeed identically to `take_update`!
    let repairable_over = peer_edit(&state, 11, |txn, root| {
        root.push_back(txn, XmlElementPrelim::empty("horizontalRule"));
        root.push_back(txn, XmlElementPrelim::empty("unknown-a"));
        root.push_back(txn, XmlElementPrelim::empty("unknown-b"));
    });
    held = assert_step_equivalent(
        held,
        &mut state,
        &mut changes,
        &repairable_over,
        &ROOMY,
        "repair deletes invalid blocks to stay within BODY_MAX_NODES",
    );

    // TooDeep (depth 61) -> exact BodyError::TooDeep match.
    let deep = peer_edit(&state, 12, |txn, root| {
        let mut parent = root.push_back(txn, XmlElementPrelim::empty("blockquote"));
        for _ in 2..BODY_MAX_DEPTH {
            parent = parent.push_back(txn, XmlElementPrelim::empty("blockquote"));
        }
        parent.push_back(txn, XmlElementPrelim::empty("paragraph"));
    });
    held = assert_step_equivalent(held, &mut state, &mut changes, &deep, &ROOMY, "too deep");

    // Raw value inserted directly into root -> unrepairable InvalidNode match.
    let bad_root_val = peer_edit(&state, 13, |txn, root| {
        let arr: &ArrayRef = root.as_ref();
        arr.push_back(txn, 42);
    });
    let _held = assert_step_equivalent(
        held,
        &mut state,
        &mut changes,
        &bad_root_val,
        &ROOMY,
        "raw value in root",
    );

    // BODY_MAX_BYTES boundary check.
    let doc_small = BodyDoc::new();
    let mut small_state = doc_small.state();
    let mut small_changes = state_counts(&small_state).expect("counts").changes;
    let huge = peer_edit(&small_state, 14, |txn, root| {
        let p = root.push_back(txn, XmlElementPrelim::empty("paragraph"));
        p.push_back(txn, XmlTextPrelim::new("x".repeat(BODY_MAX_BYTES + 10)));
    });
    let roomy_state = UpdateLimits {
        update_bytes: BODY_MAX_BYTES + 1024,
        state_bytes: BODY_MAX_BYTES + 1024,
        body_bytes: BODY_MAX_BYTES,
        ..ROOMY
    };
    let ref_err = take_update(&new_doc(), &huge, &roomy_state).expect_err("over BODY_MAX_BYTES");
    assert_eq!(ref_err, TakeError::Body(BodyError::TooLarge));
    let _ = assert_step_equivalent(
        doc_small,
        &mut small_state,
        &mut small_changes,
        &huge,
        &roomy_state,
        "over BODY_MAX_BYTES",
    );
}

fn random_adversarial_update(
    state: &[u8],
    doc_now: &Doc,
    rng: &mut Random,
    step: usize,
) -> Vec<u8> {
    let client = 1 + u64::try_from(rng.below(6)).expect("fits");
    let clock = u64::from(
        doc_now
            .transact()
            .state_vector()
            .get(&yrs::ClientID::new(client)),
    );
    match rng.below(10) {
        0 => Update::of(
            client,
            clock,
            vec![Change::under(
                Parent::body(),
                Content::element(if rng.chance(2) {
                    "paragraph"
                } else {
                    "bogus-node"
                }),
            )],
        )
        .bytes(),
        1 => Update::of(
            client,
            clock,
            vec![Change::under(Parent::body(), Content::xml_text())],
        )
        .bytes(),
        2 => Update::of(
            client,
            clock,
            vec![Change::under(
                Parent::body(),
                Content::values(vec![value::whole(7)]),
            )],
        )
        .bytes(),
        3 => Update::of(
            client,
            clock,
            vec![Change::under(
                Parent::body(),
                Content::Format(b"bold".to_vec(), b"true".to_vec()),
            )],
        )
        .bytes(),
        4 => {
            let content = match rng.below(3) {
                0 => Content::Binary(vec![1, 2, 3]),
                1 => Content::Embed(b"{}".to_vec()),
                _ => Content::Text(b"stray".to_vec()),
            };
            Update::of(client, clock, vec![Change::under(Parent::body(), content)]).bytes()
        }
        5 => {
            let kind = *rng.pick(&[0_u8, 1, 2, 4]);
            Update::of(
                client,
                clock,
                vec![Change::under(Parent::body(), Content::Type(kind, None))],
            )
            .bytes()
        }
        6 => {
            let sv = doc_now.transact().state_vector();
            let entries: Vec<(u64, u32)> = sv.iter().map(|(c, k)| (c.get(), *k)).collect();
            let (del_client, del_end) = *rng.pick(&entries);
            let start = u64::from(del_end).min(u64::try_from(rng.below(10)).expect("fits"));
            let len = 1 + u64::try_from(rng.below(4)).expect("fits");
            Update {
                runs: Vec::new(),
                deleted: vec![(del_client, vec![(start, len)])],
            }
            .bytes()
        }
        7 | 8 => peer_edit(state, 50 + client, |txn, root| {
            mutate_existing_block(txn, root, rng, step);
        }),
        _ => {
            let mut raw = peer_edit(state, 70 + client, |txn, root| {
                insert_rich_block(txn, root, 0, rng.below(12), step);
            });
            if rng.chance(2) && !raw.is_empty() {
                let at = rng.below(raw.len());
                raw[at] ^= 0x55;
            }
            raw
        }
    }
}

/// Adversarial fuzz test: drives raw malformed/crafted wire updates (partial overlaps,
/// out-of-order deletes, stray root values, non-countable `Format`, `Binary`, `Embed`,
/// `Text`, non-element shared types, unknown types, repairs, multi-client merges)
/// through BOTH `BodyDoc::take_in_place` and full `take_update`, asserting 100% agreement.
#[test]
fn adversarial_wire_updates_agree_between_incremental_and_scratch_take() {
    for seed in 1..=80_u64 {
        let mut rng = Random::new(seed * 1_000_003 + 7);
        let init = body_of(&random::document(&mut rng));
        let mut state = BodyDoc::from_body(Some(&init)).state();
        let mut changes = state_counts(&state).expect("counts").changes;
        let mut held = BodyDoc::from_state(&state, &ROOMY).expect("open").doc;

        for step in 0..12_usize {
            let doc_now: Doc = from_state(&state, &ROOMY).expect("ref").doc;
            let update = random_adversarial_update(&state, &doc_now, &mut rng, step);
            held = assert_step_equivalent(
                held,
                &mut state,
                &mut changes,
                &update,
                &ROOMY,
                &format!("adversarial seed {seed} step {step}"),
            );
        }
    }
}

/// Regression for non-countable `ItemContent::Format` at root (scenarios a & b):
/// (a) prepending `Format` before an existing block,
/// (b) `Format` in an empty root followed by appending a valid block.
#[test]
fn format_item_at_root_prepend_and_append_cannot_bypass_cache() {
    let format_bold = || Content::Format(b"bold".to_vec(), b"true".to_vec());

    // (a) Prepend `Format("bold", "true")` before an existing block (lower client ID).
    let base_a = Update::of(
        10,
        0,
        vec![Change::under(Parent::body(), Content::element("paragraph"))],
    )
    .bytes();
    let mut state_a = BodyDoc::from_state(&base_a, &ROOMY)
        .expect("open a")
        .state
        .expect("from_state encodes");
    let mut changes_a = state_counts(&state_a).expect("counts").changes;
    let held_a = BodyDoc::from_state(&state_a, &ROOMY).expect("open a").doc;
    let prepend_fmt = Update::of(1, 0, vec![Change::under(Parent::body(), format_bold())]).bytes();
    assert!(
        take_update(
            &from_state(&state_a, &ROOMY).expect("ref").doc,
            &prepend_fmt,
            &ROOMY
        )
        .is_err(),
        "(a) scratch take_update refuses leading Format at root"
    );
    let _ = assert_step_equivalent(
        held_a,
        &mut state_a,
        &mut changes_a,
        &prepend_fmt,
        &ROOMY,
        "(a) prepend Format before block",
    );

    // (b) Empty root + Format (valid: root.len() == 0), then append valid block after it.
    let mut held_b = BodyDoc::new();
    let mut state_b = held_b.state();
    let mut changes_b = state_counts(&state_b).expect("counts").changes;
    let fmt_only = Update::of(1, 0, vec![Change::under(Parent::body(), format_bold())]).bytes();
    held_b = assert_step_equivalent(
        held_b,
        &mut state_b,
        &mut changes_b,
        &fmt_only,
        &ROOMY,
        "(b1) Format alone in empty root",
    );
    let after_fmt = Update::of(
        10,
        0,
        vec![Change::after((1, 0), Content::element("paragraph"))],
    )
    .bytes();
    assert!(
        take_update(
            &from_state(&state_b, &ROOMY).expect("ref").doc,
            &after_fmt,
            &ROOMY
        )
        .is_err(),
        "(b2) scratch take_update refuses block after leading Format"
    );
    let _ = assert_step_equivalent(
        held_b,
        &mut state_b,
        &mut changes_b,
        &after_fmt,
        &ROOMY,
        "(b2) append block after Format in empty root",
    );
}

/// Regression for non-countable `ItemContent::Format` at root (scenarios c & d):
/// (c) deleting the first block of `[Block1, Format, Block2]` to expose `Format` at start,
/// (d) repairing `[InvalidBlock, Format, ValidBlock]` so deleting `InvalidBlock` exposes
///     `Format` at start (`refresh_cache_after_repair` + `BodyDoc::from_state`).
#[test]
fn format_item_at_root_exposed_by_delete_or_repair_cannot_bypass_cache() {
    let format_bold = || Content::Format(b"bold".to_vec(), b"true".to_vec());

    // (c) Valid `[Block1, Format, Block2]` then delete Block1, exposing Format at start.
    let base_c = Update::of(
        10,
        0,
        vec![
            Change::under(Parent::body(), Content::element("paragraph")),
            Change::after((10, 0), format_bold()),
            Change::after((10, 1), Content::element("horizontalRule")),
        ],
    )
    .bytes();
    let opened_c = BodyDoc::from_state(&base_c, &ROOMY).expect("(c) valid initial doc");
    let mut state_c = opened_c.state.expect("from_state encodes");
    let mut changes_c = opened_c.changes;
    let held_c = opened_c.doc;
    let del_first = Update {
        runs: Vec::new(),
        deleted: vec![(10, vec![(0, 1)])],
    }
    .bytes();
    assert!(
        take_update(
            &from_state(&state_c, &ROOMY).expect("ref").doc,
            &del_first,
            &ROOMY
        )
        .is_err(),
        "(c) scratch take_update refuses deleting Block1 to expose Format"
    );
    let _ = assert_step_equivalent(
        held_c,
        &mut state_c,
        &mut changes_c,
        &del_first,
        &ROOMY,
        "(c) delete Block1 exposing Format at root.start",
    );

    // (d) `[InvalidBlock, Format, ValidBlock]` repaired via `BodyDoc::from_state` AND via
    //     `BodyDoc::take_in_place`: deleting `InvalidBlock` exposes `Format` at root.start,
    //     so both must refuse identically to scratch `from_state` / `take_update`.
    let raw_d = Update::of(
        10,
        0,
        vec![
            Change::under(Parent::body(), Content::element("unknown-block")),
            Change::after((10, 0), format_bold()),
            Change::after((10, 1), Content::element("horizontalRule")),
        ],
    )
    .bytes();
    let ref_open_d = from_state(&raw_d, &ROOMY).expect_err("(d) scratch from_state refuses");
    let got_open_d = BodyDoc::from_state(&raw_d, &ROOMY).expect_err("(d) BodyDoc refuses");
    assert_eq!(got_open_d, ref_open_d, "(d) from_state error matches");

    let held_d = BodyDoc::new();
    let mut state_d = held_d.state();
    let mut changes_d = state_counts(&state_d).expect("counts").changes;
    let _ = assert_step_equivalent(
        held_d,
        &mut state_d,
        &mut changes_d,
        &raw_d,
        &ROOMY,
        "(d) take_in_place repair exposing Format at root.start",
    );
}

/// Verifies `BodyDoc::settle` preserves the cache across an unchanged pass-1 settle,
/// clears and rebuilds cleanly across a pass-2 converging (non-canonical) settle, and
/// keeps the original cache intact on `SettleError`.
#[test]
fn settle_unchanged_converged_and_failed_preserve_exact_equivalence() {
    // 1. Canonical (unchanged pass 1) settle with file blocks, then more edits.
    let mut held = BodyDoc::new();
    let mut state = held.state();
    let mut changes = state_counts(&state).expect("counts").changes;
    let add_img = peer_edit(&state, 10, |txn, root| {
        let img = root.push_back(txn, XmlElementPrelim::empty("image"));
        img.insert_attribute(txn, "fileId", "settled-a.png");
    });
    held = assert_step_equivalent(held, &mut state, &mut changes, &add_img, &ROOMY, "add img");
    let settled = held.settle(state.clone(), &ROOMY).expect("pass-1 settle");
    assert_eq!(settled.doc.file_ids(), vec!["settled-a.png"]);
    state = settled.state;
    changes = settled.changes;
    held = settled.doc;
    let prepend_file = peer_edit(&state, 20, |txn, root| {
        let file = root.push_front(txn, XmlElementPrelim::empty("file"));
        file.insert_attribute(txn, "fileId", "settled-0.pdf");
    });
    held = assert_step_equivalent(
        held,
        &mut state,
        &mut changes,
        &prepend_file,
        &ROOMY,
        "edit after unchanged settle",
    );
    assert_eq!(held.file_ids(), vec!["settled-0.pdf", "settled-a.png"]);

    // 2. Failed settle returns intact `BodyDoc` with intact cache and `file_ids`.
    let tight = UpdateLimits {
        state_bytes: 1,
        ..ROOMY
    };
    let err = held
        .settle(state.clone(), &tight)
        .expect_err("tight settle fails");
    assert_eq!(err.doc.file_ids(), vec!["settled-0.pdf", "settled-a.png"]);

    // 3. Non-canonical state (two adjacent text items of client 7) passed to `settle`
    //    squashes into one canonical item on pass 1 (`!same_state(&state_again, &state)`),
    //    requiring pass-2 convergence (`SettlePass::Converged`) which clears `cache` to
    //    `None`. Both the `cache: None` fallback (`file_ids` / `body`) and follow-up
    //    `take_in_place` edits (which rebuild the cache cleanly) must match scratch.
    let non_canonical = Update::of(
        7,
        0,
        vec![
            Change::under(Parent::body(), Content::element("paragraph")),
            Change::under(Parent::Change((7, 0)), Content::xml_text()),
            Change::under(Parent::Change((7, 1)), Content::text("ab")),
            Change::after((7, 3), Content::text("cd")),
        ],
    )
    .bytes();
    let held_nc = BodyDoc::new();
    let mut state_nc = held_nc.state();
    let mut changes_nc = state_counts(&state_nc).expect("counts").changes;
    let held_nc = assert_step_equivalent(
        held_nc,
        &mut state_nc,
        &mut changes_nc,
        &non_canonical,
        &ROOMY,
        "non-canonical initial take",
    );
    let converged = held_nc
        .settle(non_canonical.clone(), &ROOMY)
        .expect("pass-2 converge settle");
    assert!(
        !fvoci_collab::same_state(&converged.state, &non_canonical),
        "non-canonical state must squash on reload (exercising SettlePass::Converged)"
    );
    assert_eq!(converged.doc.file_ids(), Vec::<String>::new());
    state_nc = converged.state;
    changes_nc = converged.changes;
    let follow_up = peer_edit(&state_nc, 99, |txn, root| {
        let img = root.push_back(txn, XmlElementPrelim::empty("image"));
        img.insert_attribute(txn, "fileId", "after-converge.png");
    });
    let held_nc = assert_step_equivalent(
        converged.doc,
        &mut state_nc,
        &mut changes_nc,
        &follow_up,
        &ROOMY,
        "edit after pass-2 converged settle",
    );
    assert_eq!(held_nc.file_ids(), vec!["after-converge.png"]);
}

/// Verifies an update is refused with `TakeError::Body(BodyError::TableTooLarge)` on both
/// `take_in_place` and `take_update`, rolls back cleanly, and allows a subsequent valid edit.
fn assert_table_too_large_and_recovers(
    initial: &serde_json::Value,
    bad_update: impl FnOnce(&[u8]) -> Vec<u8>,
    client_after: u64,
    label: &str,
) {
    let initial_body = body_of(initial);
    let initial_state = BodyDoc::from_body(Some(&initial_body)).state();
    let opened = BodyDoc::from_state(&initial_state, &ROOMY).expect("initial table opens");
    let mut state = opened.state.unwrap_or_else(|| opened.doc.state());
    let mut changes = opened.changes;
    let update = bad_update(&state);
    let ref_err = take_update(
        &from_state(&state, &ROOMY).expect("ref").doc,
        &update,
        &ROOMY,
    )
    .expect_err(label);
    assert!(
        matches!(ref_err, TakeError::Body(BodyError::TableTooLarge { .. })),
        "expected TableTooLarge for {label}, got {ref_err:?}"
    );
    let held = assert_step_equivalent(opened.doc, &mut state, &mut changes, &update, &ROOMY, label);
    let valid_after = Editor::join(client_after, &state).append_paragraph("still clean");
    let _held = assert_step_equivalent(
        held,
        &mut state,
        &mut changes,
        &valid_after,
        &ROOMY,
        &format!("valid edit after {label}"),
    );
}

/// Oversized table updates (> `TABLE_MAX_ROWS` rows, > `TABLE_MAX_COLUMNS` columns, or
/// colspans summing past `TABLE_MAX_COLUMNS`) return `TakeError::Body(BodyError::TableTooLarge)`
/// identically on `take_in_place` and `take_update`, preserving document and cache invariants.
#[test]
fn table_too_large_refuses_in_place_and_matches_take_update() {
    let cell = json!({"type": "tableCell", "content": [{"type": "paragraph"}]});
    let row_1 = json!({"type": "tableRow", "content": [&cell]});
    let max_rows = json!({
        "type": "doc",
        "content": [{"type": "table", "content": vec![row_1; usize::from(TABLE_MAX_ROWS)]}],
    });
    assert_table_too_large_and_recovers(
        &max_rows,
        |state| Editor::join(301, state).add_table_row(1),
        302,
        "201st table row",
    );

    let row_20 = json!({
        "type": "tableRow",
        "content": vec![cell.clone(); usize::from(TABLE_MAX_COLUMNS)],
    });
    let max_cols = json!({
        "type": "doc",
        "content": [{"type": "table", "content": [&row_20, &row_20]}],
    });
    assert_table_too_large_and_recovers(
        &max_cols,
        |state| Editor::join(303, state).add_table_column(),
        304,
        "21st table column",
    );

    let row_19 = json!({
        "type": "tableRow",
        "content": vec![cell; usize::from(TABLE_MAX_COLUMNS) - 1],
    });
    let cols_19 = json!({
        "type": "doc",
        "content": [{"type": "table", "content": [row_19]}],
    });
    assert_table_too_large_and_recovers(
        &cols_19,
        |state| {
            peer_edit(state, 305, |txn, root| {
                let Some(XmlOut::Element(table)) = root.get(txn, 0) else {
                    panic!("table");
                };
                let Some(XmlOut::Element(row0)) = table.get(txn, 0) else {
                    panic!("row0");
                };
                let Some(XmlOut::Element(cell0)) = row0.get(txn, 0) else {
                    panic!("cell0");
                };
                cell0.insert_attribute(txn, "colspan", 3_i64);
            })
        },
        306,
        "colspan pushing width past TABLE_MAX_COLUMNS",
    );
}

/// Regression tests for `XmlFragment` root map entries (`parent_sub = Some("k")`):
/// 1. Single update combining an invalid root list item (`XmlText("evil")`) with a valid
///    `XmlElement("paragraph")` / `image` / `file` map entry on `root` (and cross-parent
///    `(left = root item, right = element attr)` attempts) matches `take_update` 100% and
///    never accepts `"evil"` or leaks map-entry `file_ids()`.
/// 2. Single update inserting only a map-entry `XmlElement("paragraph")` on `root` matches
///    cold `take_update` 100% without diverging on rollback or cache invariants.
#[test]
fn root_map_entry_injection_matches_scratch_take_and_never_leaks_files() {
    let initial = body_of(&json!({
        "type": "doc",
        "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": "P1"}]},
            {"type": "image", "attrs": {"fileId": "legit.png"}},
        ],
    }));
    let opened = BodyDoc::from_state(&BodyDoc::from_body(Some(&initial)).state(), &ROOMY)
        .expect("initial opens");
    let mut state = opened.state.expect("from_state encodes");
    let mut changes = opened.changes;
    let mut held = opened.doc;

    // Scenario 1a: wire update injecting `XmlText("evil")` list child + valid `paragraph`
    // and `image`/`file` map entries (`key = Some(b"k")` / `Some(b"f")`) under `root`.
    let scenario_1_wire = Update::of(
        40,
        0,
        vec![
            Change::under(Parent::body(), Content::xml_text()),
            Change::under(Parent::Change((40, 0)), Content::text("evil")),
            Change::member(Parent::body(), "k", Content::element("paragraph")),
            Change::member(Parent::body(), "f", Content::element("image")),
            Change::member(
                Parent::Change((40, 6)),
                "fileId",
                Content::values(vec![value::text("evil-leaked.png")]),
            ),
        ],
    )
    .bytes();
    held = assert_step_equivalent(
        held,
        &mut state,
        &mut changes,
        &scenario_1_wire,
        &ROOMY,
        "scenario 1a: XmlText(evil) + map-entry paragraph/image on root",
    );
    assert_eq!(held.file_ids(), vec!["legit.png"]);

    // Scenario 1b: cross-parent `(left = root list block, right = image fileId attribute)`
    // attempting to smuggle `parent_sub = Some("fileId")` onto `root` via `yrs` neighbor copy.
    let doc_ref = from_state(&state, &ROOMY).expect("ref").doc;
    let client_id = doc_ref
        .transact()
        .state_vector()
        .iter()
        .next()
        .map(|(c, _)| c.get())
        .expect("initial writer");
    let cross_parent = Update::of(
        41,
        0,
        vec![
            Change::under(Parent::body(), Content::xml_text()),
            Change::Item {
                left: Some((client_id, 0)),
                right: Some((client_id, 1)),
                parent: Parent::body(),
                key: None,
                content: Content::element("file"),
            },
            Change::member(
                Parent::Change((41, 1)),
                "fileId",
                Content::values(vec![value::text("cross-leaked.pdf")]),
            ),
        ],
    )
    .bytes();
    held = assert_step_equivalent(
        held,
        &mut state,
        &mut changes,
        &cross_parent,
        &ROOMY,
        "scenario 1b: cross-parent neighbor parent_sub copy onto root",
    );
    assert_eq!(held.file_ids(), vec!["legit.png"]);
}

/// Scenario 2: standalone map-entry `XmlElement("paragraph")` on `root` matches cold
/// `take_update` 100% without false `Dirty` divergence, and subsequent valid edits work.
#[test]
fn standalone_root_map_entry_matches_scratch_take_and_recovers() {
    let initial = body_of(&json!({
        "type": "doc",
        "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": "P1"}]},
            {"type": "image", "attrs": {"fileId": "legit.png"}},
        ],
    }));
    let opened = BodyDoc::from_state(&BodyDoc::from_body(Some(&initial)).state(), &ROOMY)
        .expect("initial opens");
    let mut state = opened.state.expect("from_state encodes");
    let mut changes = opened.changes;
    let mut held = opened.doc;

    let scenario_2_wire = Update::of(
        42,
        0,
        vec![Change::member(
            Parent::body(),
            "k",
            Content::element("paragraph"),
        )],
    )
    .bytes();
    held = assert_step_equivalent(
        held,
        &mut state,
        &mut changes,
        &scenario_2_wire,
        &ROOMY,
        "scenario 2: standalone map-entry paragraph on root",
    );
    assert_eq!(held.file_ids(), vec!["legit.png"]);

    let clean = Editor::join(43, &state).append_paragraph("clean follow-up");
    let held = assert_step_equivalent(
        held,
        &mut state,
        &mut changes,
        &clean,
        &ROOMY,
        "clean edit after map-entry attempts",
    );
    assert_eq!(held.file_ids(), vec!["legit.png"]);
}
