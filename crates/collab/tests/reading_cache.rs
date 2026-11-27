//! Equivalence and security tests for the incremental Pass 5–6 [`ReadingCache`],
//! compressed `BlockPicker`, `check_incoming_origins`, and deletion-only repair fast path.

mod support;

use fvoci_collab::testing::Editor;
use fvoci_collab::{
    BodyDoc, BodyTakeError, FIELD, MAX_CLIENT_CHANGES, MAX_CLIENTS, PLACING_STEPS, READING_STEPS,
    TakeError, UpdateError, UpdateLimits, from_body, from_state, new_doc, reading_steps,
    state_counts, state_of, take_update,
};
use fvoci_model::BODY_MAX_BYTES;
use serde_json::json;
use support::wire::{Change, Content, Parent, Run, Update};
use support::{ROOMY, body_of};
use yrs::updates::decoder::Decode as _;
use yrs::{
    Any, ClientID, Doc, OffsetKind, Options, ReadTxn as _, Text as _, Transact as _,
    TransactionMut, Update as YrsUpdate, Xml as _, XmlElementPrelim, XmlFragment as _,
    XmlFragmentRef, XmlOut, XmlTextPrelim,
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

#[derive(Clone, Copy)]
enum PathExpect {
    Fast,
    Fallback,
}

fn step_and_assert(
    held: BodyDoc,
    state: &mut Vec<u8>,
    changes: &mut u64,
    update: &[u8],
    limits: &UpdateLimits,
    expect_path: PathExpect,
    label: &str,
) -> BodyDoc {
    let ref_doc = from_state(state, &ROOMY).expect("ref open").doc;
    let expected = take_update(&ref_doc, update, limits).expect("ref takes");
    let got = held
        .take_in_place(None, *changes, update, limits)
        .expect("in-place takes");
    let want_fast = matches!(expect_path, PathExpect::Fast);
    assert_eq!(
        got.state.is_none(),
        want_fast,
        "{label}: expected fast_path={want_fast}, got state.is_some()={}",
        got.state.is_some()
    );
    let got_state = got.state.unwrap_or_else(|| got.doc.state());
    assert_eq!(got_state, expected.state, "{label}: state bytes");
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
    assert_eq!(got.remake, expected.remake, "{label}: remake");
    assert_eq!(got.repair.update, expected.repair.update, "{label}: repair");
    assert_eq!(
        got.doc.body().expect("body"),
        expected.repair.body,
        "{label}: body"
    );
    *state = got_state;
    *changes = got.changes;
    got.doc
}

fn push_para_text(state: &[u8], client: u64, text: &str) -> Vec<u8> {
    peer_edit(state, client, |txn, root| {
        let p = root.push_back(txn, XmlElementPrelim::empty("paragraph"));
        p.push_back(txn, XmlTextPrelim::new(text));
    })
}

fn insert_in_para(state: &[u8], client: u64, para_idx: u32, pos: u32, text: &str) -> Vec<u8> {
    peer_edit(state, client, |txn, root| {
        let Some(XmlOut::Element(p)) = root.get(txn, para_idx) else {
            panic!("missing paragraph {para_idx}");
        };
        let Some(XmlOut::Text(t)) = p.first_child() else {
            panic!("missing text in paragraph {para_idx}");
        };
        t.insert(txn, pos, text);
    })
}

/// Consecutive same-client typing (`Letters` squash) crossing the 127 -> 128 UTF-8 varint
/// boundary, plus pure ASCII and pure Korean (3-byte BMP) fast-path mid-string splits.
#[test]
fn letters_squash_and_ascii_korean_splits_match_scratch() {
    let mut held = BodyDoc::new();
    let mut state = held.state();
    let mut changes = state_counts(&state).expect("counts").changes;

    let peer = Doc::with_options(Options {
        offset_kind: OffsetKind::Utf16,
        skip_gc: true,
        ..Options::with_client_id(ClientID::new(10))
    });
    let root = peer.get_or_insert_xml_fragment(FIELD);
    let u_init = {
        let before = peer.transact().state_vector();
        {
            let mut txn = peer.transact_mut();
            let p = root.push_back(&mut txn, XmlElementPrelim::empty("paragraph"));
            p.push_back(&mut txn, XmlTextPrelim::new("a".repeat(126)));
        }
        peer.transact().encode_diff_v1(&before)
    };
    held = step_and_assert(
        held,
        &mut state,
        &mut changes,
        &u_init,
        &ROOMY,
        PathExpect::Fast,
        "init 126 ascii",
    );

    for (i, ch) in ["b", "c", "d", "efg"].into_iter().enumerate() {
        let u = {
            let before = peer.transact().state_vector();
            {
                let mut txn = peer.transact_mut();
                let Some(XmlOut::Element(p)) = root.first_child() else {
                    panic!("p");
                };
                let Some(XmlOut::Text(t)) = p.first_child() else {
                    panic!("t");
                };
                let end = t.len(&txn);
                t.insert(&mut txn, end, ch);
            }
            peer.transact().encode_diff_v1(&before)
        };
        held = step_and_assert(
            held,
            &mut state,
            &mut changes,
            &u,
            &ROOMY,
            PathExpect::Fast,
            &format!("same-client squash step {i} across 128B varint"),
        );
    }

    let split_ascii = insert_in_para(&state, 20, 0, 64, "X");
    held = step_and_assert(
        held,
        &mut state,
        &mut changes,
        &split_ascii,
        &ROOMY,
        PathExpect::Fast,
        "split 132B pure ASCII",
    );

    let add_korean = push_para_text(&state, 30, &"가나다라마바사아자차".repeat(6));
    held = step_and_assert(
        held,
        &mut state,
        &mut changes,
        &add_korean,
        &ROOMY,
        PathExpect::Fast,
        "add 180B Korean",
    );
    let split_korean = insert_in_para(&state, 31, 1, 30, "하");
    let _ = step_and_assert(
        held,
        &mut state,
        &mut changes,
        &split_korean,
        &ROOMY,
        PathExpect::Fast,
        "split 180B pure Korean",
    );
}

/// Short (<128B) mixed-UTF-8 double-split stays on fast path; long (>=128B) mixed-UTF-8
/// split and 4-byte emoji mid-surrogate delete fall back cleanly and recover to fast path.
#[test]
fn mixed_utf8_and_surrogate_splits_match_scratch() {
    let mut held = BodyDoc::new();
    let mut state = held.state();
    let mut changes = state_counts(&state).expect("counts").changes;

    let add_short_mixed = push_para_text(&state, 40, "hello 한글 world 테스트");
    held = step_and_assert(
        held,
        &mut state,
        &mut changes,
        &add_short_mixed,
        &ROOMY,
        PathExpect::Fast,
        "add short mixed",
    );
    for (step, (cid, pos)) in [(41_u64, 7_u32), (42, 3)].into_iter().enumerate() {
        let u = insert_in_para(&state, cid, 0, pos, "!");
        held = step_and_assert(
            held,
            &mut state,
            &mut changes,
            &u,
            &ROOMY,
            PathExpect::Fast,
            &format!("short mixed split {step}"),
        );
    }

    let add_long_mixed = push_para_text(&state, 50, &"abc가나다".repeat(20));
    held = step_and_assert(
        held,
        &mut state,
        &mut changes,
        &add_long_mixed,
        &ROOMY,
        PathExpect::Fast,
        "add long mixed",
    );
    let split_long_mixed = insert_in_para(&state, 51, 1, 50, "*");
    held = step_and_assert(
        held,
        &mut state,
        &mut changes,
        &split_long_mixed,
        &ROOMY,
        PathExpect::Fallback,
        "split long mixed falls back cleanly",
    );
    let after_fallback = Editor::join(52, &state).append_paragraph("fast again");
    held = step_and_assert(
        held,
        &mut state,
        &mut changes,
        &after_fallback,
        &ROOMY,
        PathExpect::Fast,
        "cache recovered to fast path after fallback",
    );

    let add_emoji = push_para_text(&state, 60, "😀😀");
    held = step_and_assert(
        held,
        &mut state,
        &mut changes,
        &add_emoji,
        &ROOMY,
        PathExpect::Fast,
        "add emoji",
    );
    // Delete clock 4..5 of client 60 (mid-surrogate inside second emoji: 0=p, 1=text, 2..6=😀😀).
    let split_surrogate = Update {
        runs: Vec::new(),
        deleted: vec![(60, vec![(4, 1)])],
    }
    .bytes();
    held = step_and_assert(
        held,
        &mut state,
        &mut changes,
        &split_surrogate,
        &ROOMY,
        PathExpect::Fallback,
        "surrogate split falls back cleanly",
    );
    let after_surrogate = Editor::join(61, &state).append_paragraph("fast after surrogate");
    let _ = step_and_assert(
        held,
        &mut state,
        &mut changes,
        &after_surrogate,
        &ROOMY,
        PathExpect::Fast,
        "fast path resumes after surrogate fallback",
    );
}

/// Deletion-only repair (`repair.mended == 0`) uses `ReadingCache::try_apply_deletions`
/// (`state: None`) and matches scratch `take_update` on all metrics, while no-op updates
/// set `mutated == false`.
#[test]
fn deletion_only_repair_and_noop_updates_use_fast_path_and_track_mutated() {
    let base_doc = from_body(&body_of(&json!({
        "type": "doc",
        "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": "one"}]},
            {"type": "paragraph", "content": [{"type": "text", "text": "two"}]}
        ]
    })));
    let mut state = state_of(&base_doc);
    let mut changes = state_counts(&state).expect("counts").changes;
    let mut held = BodyDoc::from_state(&state, &ROOMY).expect("open").doc;

    let bad = Editor::join(200, &state).append_element("unknown-widget");
    held = step_and_assert(
        held,
        &mut state,
        &mut changes,
        &bad,
        &ROOMY,
        PathExpect::Fast,
        "deletion-only repair takes fast path",
    );

    // Re-sending the exact same already-applied update is a no-op (`mutated == false`).
    let dup = held
        .take_in_place(None, changes, &bad, &ROOMY)
        .expect("duplicate accepted");
    assert!(
        !dup.mutated,
        "duplicate already-known update is not mutated"
    );
    assert!(dup.state.is_none(), "no-op stays on fast path");
    assert_eq!(dup.state_bytes, state.len());
}

/// Compressed `BlockPicker` matches `Replay::run` across multi-client dependency chains
/// (both below and above `stop_past`), triggering exact `remake: true` and exact
/// `reading_steps` / `steps_ahead`.
#[test]
fn compressed_block_picker_matches_exact_reading_steps_and_stop_past() {
    // Client 1 creates 600 paragraphs (1,800 changes). Client 2 (higher ID!) appends after
    // client 1's last paragraph -> `looked` = 1,800 -> `steps` = ceil(1800 / 128) = 15.
    let mut state = state_of(&new_doc());
    let u1 = peer_edit(&state, 1, |txn, root| {
        for _ in 0..600 {
            let p = root.push_back(txn, XmlElementPrelim::empty("paragraph"));
            p.push_back(txn, XmlTextPrelim::new("x"));
        }
    });
    let opened = BodyDoc::from_state(&u1, &ROOMY).expect("open u1");
    state = opened.state.expect("state");
    let mut changes = opened.changes;
    let held = opened.doc;

    let u2 = Editor::join(2, &state).append_paragraph("after 1");
    let ref_steps_full = reading_steps(
        &take_update(&from_state(&state, &ROOMY).unwrap().doc, &u2, &ROOMY)
            .unwrap()
            .state,
        u64::MAX,
    )
    .expect("ref steps");
    assert!(
        ref_steps_full > 10,
        "expected >10 steps, got {ref_steps_full}"
    );

    // Test at exact `limit = ref_steps_full`, `limit = ref_steps_full - 1`, and `limit = 5`.
    for limit in [ref_steps_full, ref_steps_full - 1, 5] {
        let lim = UpdateLimits {
            reading_steps: Some(limit),
            ..ROOMY
        };
        let fresh_held = BodyDoc::from_state(&state, &ROOMY).expect("open").doc;
        let mut s = state.clone();
        let mut c = changes;
        let _ = step_and_assert(
            fresh_held,
            &mut s,
            &mut c,
            &u2,
            &lim,
            PathExpect::Fast,
            &format!("stop_past={limit}"),
        );
    }

    // Also add a 3rd client (ID 3 > 2 > 1) depending on client 2 -> 2-level stack!
    let mut s = state;
    let held = step_and_assert(
        held,
        &mut s,
        &mut changes,
        &u2,
        &ROOMY,
        PathExpect::Fast,
        "u2",
    );
    let u3 = Editor::join(3, &s).append_paragraph("after 2");
    for limit in [READING_STEPS, 10, 1] {
        let lim = UpdateLimits {
            reading_steps: Some(limit),
            ..ROOMY
        };
        let fresh = BodyDoc::from_state(&s, &ROOMY).expect("open s").doc;
        let mut s2 = s.clone();
        let mut c2 = changes;
        let _ = step_and_assert(
            fresh,
            &mut s2,
            &mut c2,
            &u3,
            &lim,
            PathExpect::Fast,
            &format!("3-client chain stop_past={limit}"),
        );
    }
    drop(held);
}

/// `ReadingCache::check_incoming_origins` refuses pre-apply (`Untouched`) on inverted
/// same-group and partial-overlap `yrs` panic bombs without scanning `prior_state` or
/// polluting `ReadingCache` (subsequent valid edit still hits fast path!).
#[test]
fn cached_check_incoming_origins_blocks_panic_bombs_untouched_and_preserves_cache() {
    let base = Update::of(
        10,
        0,
        vec![
            Change::under(Parent::body(), Content::element("paragraph")),
            Change::under(Parent::Change((10, 0)), Content::xml_text()),
            Change::under(Parent::Change((10, 1)), Content::Text(b"abcdef".to_vec())),
        ],
    )
    .bytes();
    let opened = BodyDoc::from_state(&base, &ROOMY).expect("open base");
    let state = opened.state.expect("state");
    let changes = opened.changes;
    let held = opened.doc;

    // Bomb 1: inverted same-group origins inside client 10's text block (right < left) +
    // consecutive squashable pair triggering `check_origins_before_apply`.
    let bomb = Update::of(
        20,
        0,
        vec![
            Change::Item {
                left: Some((10, 5)),
                right: Some((10, 3)),
                parent: Parent::body(),
                key: None,
                content: Content::Text(b"x".to_vec()),
            },
            Change::Item {
                left: Some((20, 0)),
                right: Some((10, 3)),
                parent: Parent::body(),
                key: None,
                content: Content::Text(b"y".to_vec()),
            },
        ],
    )
    .bytes();
    let served = UpdateLimits {
        update_bytes: 64 * 1024,
        state_bytes: 16 * 1024 * 1024,
        body_bytes: BODY_MAX_BYTES,
        placing_steps: Some(PLACING_STEPS),
        clients: Some(MAX_CLIENTS),
        client_changes: Some(MAX_CLIENT_CHANGES),
        total_changes: Some(fvoci_collab::MAX_TOTAL_CHANGES),
        reading_steps: Some(READING_STEPS),
    };
    let held = match held.take_in_place(None, changes, &bomb, &served) {
        Err(BodyTakeError::Untouched { doc, error }) => {
            assert!(
                matches!(error, TakeError::Update(UpdateError::Unreadable(_))),
                "expected Unreadable origin refusal, got {error:?}"
            );
            doc
        }
        other => panic!("expected Untouched refusal, got {other:?}"),
    };

    // Bomb 2: partial overlap re-anchoring a mismatched parent onto client 10's text block.
    let partial_bomb = Update::of(
        10,
        6,
        vec![Change::Item {
            left: None,
            right: None,
            parent: Parent::body(),
            key: None,
            content: Content::Text(b"zzz".to_vec()),
        }],
    )
    .bytes();
    let held = match held.take_in_place(None, changes, &partial_bomb, &served) {
        Err(BodyTakeError::Untouched { doc, error }) => {
            assert!(
                matches!(error, TakeError::Update(UpdateError::Unreadable(_))),
                "expected Unreadable partial-overlap refusal, got {error:?}"
            );
            doc
        }
        other => panic!("expected Untouched refusal on partial overlap, got {other:?}"),
    };

    // After two Untouched refusals, cache is still intact and takes the fast path (`state: None`).
    let valid = Editor::join(30, &state).append_paragraph("still fast");
    let mut s = state;
    let mut c = changes;
    let _ = step_and_assert(
        held,
        &mut s,
        &mut c,
        &valid,
        &served,
        PathExpect::Fast,
        "valid edit after Untouched refusals stays on fast path",
    );
}

/// Regression test for `placing::charge_lazy` priority: an update that overlaps an existing
/// long held block with a 0-charge `Content::Collected` run and cuts inside it must still
/// be charged against the held block (refused with `PlacedPastTooMany`), matching `take_update`.
#[test]
fn placing_charge_lazy_cannot_be_bypassed_by_collected_overlap() {
    let base = Update::of(
        10,
        0,
        vec![
            Change::under(Parent::body(), Content::element("paragraph")),
            Change::under(Parent::Change((10, 0)), Content::xml_text()),
            Change::under(Parent::Change((10, 1)), Content::Text(vec![b'a'; 60_000])),
        ],
    )
    .bytes();
    let opened = BodyDoc::from_state(&base, &ROOMY).expect("open");
    let state = opened.state.expect("state");
    let changes = opened.changes;
    let held = opened.doc;

    // Overlap client 10's [0..60_002) with a 0-charge GC run and delete 200 single clocks
    // strictly inside the 60,000-letter block: 200 * 60_000 / 256 = 46,875 placing steps.
    let mut del_ranges = Vec::new();
    for i in 0..200_u64 {
        del_ranges.push((100 + i * 200, 1));
    }
    let attack = Update {
        runs: vec![Run {
            client: 10,
            clock: 0,
            changes: vec![Change::Collected(60_002)],
        }],
        deleted: vec![(10, del_ranges)],
    }
    .bytes();
    let tight = UpdateLimits {
        placing_steps: Some(1_000),
        ..ROOMY
    };
    let ref_err = take_update(&from_state(&state, &ROOMY).unwrap().doc, &attack, &tight)
        .expect_err("scratch refuses");
    assert!(matches!(
        ref_err,
        TakeError::Update(UpdateError::PlacedPastTooMany { .. })
    ));
    let got_err = held
        .take_in_place(None, changes, &attack, &tight)
        .expect_err("in-place must refuse");
    let BodyTakeError::Untouched { error, .. } = got_err else {
        panic!("expected Untouched PlacedPastTooMany, got {got_err:?}");
    };
    assert_eq!(error, ref_err);
}

/// Regression test for `ClientRun::note_squash_group` strict-overlap boundary: two distinct
/// touching squash groups `[2..7)` ("world") and `[7..12)` ("hello") remain distinct even
/// after both are split, so inserting between `(10, 11)` and `(10, 2)` is accepted.
#[test]
fn touching_distinct_squash_groups_are_not_merged_in_origin_check() {
    let base = Update::of(
        10,
        0,
        vec![
            Change::under(Parent::body(), Content::element("paragraph")),
            Change::under(Parent::Change((10, 0)), Content::xml_text()),
            Change::under(Parent::Change((10, 1)), Content::Text(b"world".to_vec())),
            Change::Item {
                left: None,
                right: Some((10, 2)),
                parent: Parent::Change((10, 1)),
                key: None,
                content: Content::Text(b"hello".to_vec()),
            },
        ],
    )
    .bytes();
    let opened = BodyDoc::from_state(&base, &ROOMY).expect("open");
    let mut state = opened.state.expect("state");
    let mut changes = opened.changes;
    let mut held = opened.doc;

    // Peer splits both [2..7) and [7..12) so both intervals enter `squash_groups`.
    let split_both = peer_edit(&state, 20, |txn, root| {
        let Some(XmlOut::Element(p)) = root.first_child() else {
            panic!("p");
        };
        let Some(XmlOut::Text(t)) = p.first_child() else {
            panic!("t");
        };
        t.insert(txn, 2, "A");
        t.insert(txn, 8, "B");
    });
    held = step_and_assert(
        held,
        &mut state,
        &mut changes,
        &split_both,
        &ROOMY,
        PathExpect::Fast,
        "split both touching groups",
    );

    // Client 30 inserts a squashable pair between 'o' (10, 11) and 'w' (10, 2), triggering
    // `check_origins_before_apply`: must be accepted because [2..7) and [7..12) are distinct.
    let between = Update::of(
        30,
        0,
        vec![
            Change::Item {
                left: Some((10, 11)),
                right: Some((10, 2)),
                parent: Parent::Change((10, 1)),
                key: None,
                content: Content::Text(b" ".to_vec()),
            },
            Change::Item {
                left: Some((30, 0)),
                right: Some((10, 2)),
                parent: Parent::Change((10, 1)),
                key: None,
                content: Content::Text(b"!".to_vec()),
            },
        ],
    )
    .bytes();
    let _ = step_and_assert(
        held,
        &mut state,
        &mut changes,
        &between,
        &ROOMY,
        PathExpect::Fast,
        "valid edit between distinct touching groups",
    );
}

fn peer_txn(
    peer: &Doc,
    root: &XmlFragmentRef,
    edit: impl FnOnce(&mut TransactionMut<'_>, &XmlFragmentRef),
) -> Vec<u8> {
    let before = peer.transact().state_vector();
    {
        let mut txn = peer.transact_mut();
        edit(&mut txn, root);
    }
    peer.transact().encode_diff_v1(&before)
}

/// Regression test for attributed elements (`has_child` gating on `sub == SUB_NONE`),
/// same-client attribute overwrites, and same-client single-txn delete+replace (`Letters`)
/// all staying on the fast path and matching scratch `take_update`.
#[test]
fn attribute_writes_overwrites_and_same_txn_text_replace_stay_fast() {
    let mut held = BodyDoc::new();
    let mut state = held.state();
    let mut changes = state_counts(&state).expect("counts").changes;
    let peer = Doc::with_options(Options {
        offset_kind: OffsetKind::Utf16,
        skip_gc: true,
        ..Options::with_client_id(ClientID::new(70))
    });
    let root = peer.get_or_insert_xml_fragment(FIELD);
    let make_heading = peer_txn(&peer, &root, |txn, r| {
        let h = r.push_back(txn, XmlElementPrelim::empty("heading"));
        h.insert_attribute(txn, "level", Any::from(1_i64));
        h.push_back(txn, XmlTextPrelim::new("한"));
    });
    held = step_and_assert(
        held,
        &mut state,
        &mut changes,
        &make_heading,
        &ROOMY,
        PathExpect::Fast,
        "create heading with attr before child",
    );
    for level in [2_i64, 3] {
        let edit_attr = peer_txn(&peer, &root, |txn, r| {
            let Some(XmlOut::Element(h)) = r.first_child() else {
                panic!("h");
            };
            h.insert_attribute(txn, "level", Any::from(level));
        });
        held = step_and_assert(
            held,
            &mut state,
            &mut changes,
            &edit_attr,
            &ROOMY,
            PathExpect::Fast,
            &format!("overwrite heading level={level}"),
        );
    }
    let replace_text = peer_txn(&peer, &root, |txn, r| {
        let Some(XmlOut::Element(h)) = r.first_child() else {
            panic!("h");
        };
        let Some(XmlOut::Text(t)) = h.first_child() else {
            panic!("t");
        };
        t.remove_range(txn, 0, 1);
        t.insert(txn, 0, "a");
    });
    held = step_and_assert(
        held,
        &mut state,
        &mut changes,
        &replace_text,
        &ROOMY,
        PathExpect::Fast,
        "same-txn delete+replace text",
    );
    let append_after = peer_txn(&peer, &root, |txn, r| {
        let Some(XmlOut::Element(h)) = r.first_child() else {
            panic!("h");
        };
        let Some(XmlOut::Text(t)) = h.first_child() else {
            panic!("t");
        };
        t.insert(txn, 1, "b");
    });
    let _ = step_and_assert(
        held,
        &mut state,
        &mut changes,
        &append_after,
        &ROOMY,
        PathExpect::Fast,
        "append after same-txn replace stays fast",
    );
}

/// Regression test for O(N) `order_new_blocks`: 1,500 clients chained in ascending ID order
/// (`1 -> 2 -> ... -> 1500`, 0 `reading_steps`) integrates linearly on the fast path.
#[test]
fn multi_client_chain_orders_linearly_on_fast_path() {
    let mut runs = Vec::with_capacity(1_500);
    for c in 1_u64..=1_500 {
        let change = if c == 1_500 {
            Change::under(Parent::body(), Content::element("paragraph"))
        } else {
            Change::Item {
                left: Some((c + 1, 0)),
                right: None,
                parent: Parent::body(),
                key: None,
                content: Content::element("paragraph"),
            }
        };
        runs.push(Run {
            client: c,
            clock: 0,
            changes: vec![change],
        });
    }
    let update = Update {
        runs,
        deleted: Vec::new(),
    }
    .bytes();
    let held = BodyDoc::new();
    let mut state = held.state();
    let mut changes = state_counts(&state).expect("counts").changes;
    let _ = step_and_assert(
        held,
        &mut state,
        &mut changes,
        &update,
        &ROOMY,
        PathExpect::Fast,
        "1500-client chain in one update",
    );
}

fn assert_total_changes_verdict_matches(
    state: &[u8],
    changes: u64,
    update: &[u8],
    expected_total: u64,
    label: &str,
) {
    let tight = UpdateLimits {
        total_changes: Some(expected_total - 1),
        ..ROOMY
    };
    let cold_doc = from_state(state, &ROOMY).expect("cold open").doc;
    let cold_err = take_update(&cold_doc, update, &tight).expect_err("cold refuses");
    assert_eq!(
        cold_err,
        TakeError::Update(UpdateError::TooManyTotalChanges {
            changes: expected_total,
            limit: expected_total - 1,
        }),
        "{label}: cold error"
    );
    let fast_doc = BodyDoc::from_state(state, &ROOMY).expect("cached open").doc;
    let fast_err = match fast_doc
        .take_in_place(None, changes, update, &tight)
        .expect_err("fast path refuses over total_changes")
    {
        BodyTakeError::Untouched { error, .. } | BodyTakeError::Dirty(error) => error,
    };
    assert_eq!(
        fast_err, cold_err,
        "{label}: fast refusal matches cold scan"
    );
}

/// Verifies that `ReadingCache::try_apply` and `ReadingCache::try_apply_deletions` return
/// the exact same `Ok` / `Err(TooManyTotalChanges)` verdicts as cold `take_update`, across
/// multi-client appends, exact boundaries, mid-text deletion splits, and deletion-only
/// repairs.
#[test]
fn total_changes_limit_matches_between_fast_cache_and_cold_scan() {
    let body = body_of(&json!({
        "type": "doc",
        "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": "abcdef"}]}
        ]
    }));
    let made = BodyDoc::from_body_counted(Some(&body)).expect("counted body");
    let mut state = made.state;
    let mut changes = made.counts.changes;
    let mut held = made.doc;

    let add_second = peer_edit(&state, 20, |txn, root| {
        let p = root.push_back(txn, XmlElementPrelim::empty("paragraph"));
        p.push_back(txn, XmlTextPrelim::new("ghijkl"));
    });
    held = step_and_assert(
        held,
        &mut state,
        &mut changes,
        &add_second,
        &ROOMY,
        PathExpect::Fast,
        "second client paragraph",
    );

    // 1. Mid-text deletion splits `"ghijkl"`, increasing `total_changes`.
    let split_del = peer_edit(&state, 30, |txn, root| {
        let Some(XmlOut::Element(p)) = root.get(txn, 1) else {
            panic!("second paragraph");
        };
        let Some(XmlOut::Text(t)) = p.first_child() else {
            panic!("text");
        };
        t.remove_range(txn, 2, 2);
    });
    let cold_split = take_update(
        &from_state(&state, &ROOMY).expect("cold").doc,
        &split_del,
        &ROOMY,
    )
    .expect("cold split");
    assert!(
        cold_split.changes > changes,
        "split increases total_changes"
    );
    assert_total_changes_verdict_matches(&state, changes, &split_del, cold_split.changes, "split");

    let exact_split = UpdateLimits {
        total_changes: Some(cold_split.changes),
        ..ROOMY
    };
    held = step_and_assert(
        held,
        &mut state,
        &mut changes,
        &split_del,
        &exact_split,
        PathExpect::Fast,
        "mid-text deletion accepted at exact total_changes cap",
    );

    // 2. Unknown block repaired by deletion-only `try_apply_deletions`.
    let bad_block = peer_edit(&state, 40, |txn, root| {
        root.push_back(txn, XmlElementPrelim::empty("unknownWidget"));
    });
    let cold_repaired = take_update(
        &from_state(&state, &ROOMY).expect("cold 2").doc,
        &bad_block,
        &ROOMY,
    )
    .expect("cold repair");
    assert!(cold_repaired.repair.deleted > 0);
    assert_total_changes_verdict_matches(
        &state,
        changes,
        &bad_block,
        cold_repaired.changes,
        "repair",
    );

    let exact_repair = UpdateLimits {
        total_changes: Some(cold_repaired.changes),
        ..ROOMY
    };
    let _ = step_and_assert(
        held,
        &mut state,
        &mut changes,
        &bad_block,
        &exact_repair,
        PathExpect::Fast,
        "deletion-only repair accepted at exact total_changes cap",
    );
}

/// Table-mend repair (`repair.mended > 0`): the raw deletion update adds 0 changes
/// (fitting `total_changes`), then mending the missing cell adds new changes and hits
/// `TooManyTotalChanges` inside `recount_after_repair` identically to cold `take_update`.
#[test]
fn total_changes_limit_matches_on_table_mend_repair() {
    let cell = json!({"type": "tableCell", "content": [{"type": "paragraph"}]});
    let body = body_of(&json!({
        "type": "doc",
        "content": [{
            "type": "table",
            "content": [
                {"type": "tableRow", "content": [cell, cell]},
                {"type": "tableRow", "content": [cell, cell]}
            ]
        }]
    }));
    let made = BodyDoc::from_body_counted(Some(&body)).expect("counted body");
    let mut state = made.state;
    let mut changes = made.counts.changes;
    let held = made.doc;

    let break_row = peer_edit(&state, 50, |txn, root| {
        let Some(XmlOut::Element(table)) = root.get(txn, 0) else {
            panic!("table");
        };
        let Some(XmlOut::Element(row)) = table.get(txn, 1) else {
            panic!("second row");
        };
        row.remove_range(txn, 1, 1);
    });
    let cold_mended = take_update(
        &from_state(&state, &ROOMY).expect("cold").doc,
        &break_row,
        &ROOMY,
    )
    .expect("cold mend repair");
    assert!(cold_mended.repair.mended > 0, "expected table cell mend");
    assert!(
        cold_mended.changes > changes,
        "table mend adds changes past the pre-repair total ({changes} -> {})",
        cold_mended.changes
    );
    assert_total_changes_verdict_matches(
        &state,
        changes,
        &break_row,
        cold_mended.changes,
        "table-mend",
    );

    let exact_mended = UpdateLimits {
        total_changes: Some(cold_mended.changes),
        ..ROOMY
    };
    let _ = step_and_assert(
        held,
        &mut state,
        &mut changes,
        &break_row,
        &exact_mended,
        PathExpect::Fallback,
        "table-mend repair accepted at exact total_changes cap",
    );
}
