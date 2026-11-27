//! Regression tests for linear `from_body`, descending multi-client splitting, and
//! prefilled incremental caches (`BlockCache` + `ReadingCache`).

mod support;

use fvoci_collab::{
    BodyDoc, MAX_CLIENT_CHANGES, MAX_CLIENTS, MAX_TOTAL_CHANGES, PLACING_STEPS, READING_STEPS,
    UpdateError, UpdateLimits, from_body, from_body_by, from_state, new_doc, reading_steps,
    state_counts, state_of, take_update,
};
use fvoci_model::{BODY_MAX_BYTES, Body};
use serde_json::{Value, json};
use support::shapes::{self, SHAPES, typed, unknown};
use support::{ROOMY, accepted_bodies, body_of, body_vectors, edited, json_of};
use yrs::updates::decoder::Decode as _;
use yrs::{ReadTxn as _, Transact as _, Update, XmlFragment as _, XmlTextPrelim};

const SERVER_LIMITS: UpdateLimits = UpdateLimits {
    update_bytes: 64 * 1024,
    state_bytes: 16 * 1024 * 1024,
    body_bytes: BODY_MAX_BYTES,
    placing_steps: Some(PLACING_STEPS),
    clients: Some(MAX_CLIENTS),
    client_changes: Some(MAX_CLIENT_CHANGES),
    total_changes: Some(MAX_TOTAL_CHANGES),
    reading_steps: Some(READING_STEPS),
};

/// `MAX_CLIENT_CHANGES / 4`: per-client split target for multi-client `from_body`.
const EXPECTED_CHANGES_PER_CLIENT: u64 = MAX_CLIENT_CHANGES / 4;

fn verify_counted_body_and_first_edits(
    name: &str,
    document: &Value,
    expected_json: Option<&Value>,
    limits: &UpdateLimits,
) {
    let body = body_of(document);
    let made = BodyDoc::from_body_counted(Some(&body)).expect("valid body counts");

    assert_eq!(
        made.state,
        made.doc.state(),
        "{name}: counted state matches doc.state() byte for byte"
    );
    let expected_counts = state_counts(&made.state).expect("counted state scans");
    assert_eq!(
        made.counts, expected_counts,
        "{name}: precomputed StateCounts matches state_counts"
    );
    assert_eq!(
        reading_steps(&made.state, READING_STEPS),
        Ok(0),
        "{name}: freshly built state has zero reading_steps"
    );

    let cold = BodyDoc::from_state(&made.state, limits)
        .unwrap_or_else(|err| panic!("{name}: from_state failed: {err}"));
    assert_eq!(cold.repair.deleted, 0, "{name}: zero repairs on from_state");
    assert_eq!(cold.repair.mended, 0, "{name}: zero mends on from_state");
    assert_eq!(cold.repair.update, None, "{name}: no repair update");
    assert_eq!(cold.reading_steps, 0, "{name}: zero reading_steps on open");
    let made_body = made.doc.body().expect("body projects");
    assert_eq!(
        made_body,
        cold.doc.body().expect("cold body projects"),
        "{name}: made body equals cold projected body"
    );
    if let Some(expected) = expected_json {
        assert_eq!(
            json_of(made_body.as_ref()),
            *expected,
            "{name}: round-trip body equals canonical vector JSON"
        );
    }
    assert_eq!(
        made.doc.file_ids(),
        cold.doc.file_ids(),
        "{name}: prefilled file_ids match cold BlockCache"
    );

    // First live edit immediately after `from_body_counted` must take the incremental
    // fast path (`state.is_none()`) and agree with scratch `take_update`.
    let raw = from_state(&made.state, limits).expect("raw doc opens").doc;
    let edit = typed(&raw);
    let scratch = take_update(&raw, &edit, limits).expect("scratch take");
    let taken = made
        .doc
        .take_in_place(None, made.counts.changes, &edit, limits)
        .unwrap_or_else(|err| panic!("{name}: first take_in_place failed: {err}"));
    assert!(
        taken.state.is_none(),
        "{name}: first edit after from_body_counted stayed on incremental fast path"
    );
    assert_eq!(
        taken.state_bytes,
        taken.doc.state().len(),
        "{name}: incremental state_bytes exact"
    );
    assert_eq!(
        taken.doc.state(),
        state_of(&scratch.doc),
        "{name}: incremental state matches scratch"
    );
    assert_eq!(
        taken.reading_steps, scratch.reading_steps,
        "{name}: incremental reading_steps matches scratch"
    );
    assert_eq!(
        taken.doc.file_ids(),
        cold_files(&taken.doc.state(), limits),
        "{name}: post-edit file_ids exact"
    );

    // Also verify a deletion-only repair immediately after `from_body_counted` stays fast.
    let made_for_repair = BodyDoc::from_body_counted(Some(&body)).expect("valid body counts");
    let raw2 = from_state(&made_for_repair.state, limits)
        .expect("raw2 opens")
        .doc;
    let bad = unknown(&raw2);
    let scratch_repair = take_update(&raw2, &bad, limits).expect("scratch repair");
    let repaired = made_for_repair
        .doc
        .take_in_place(None, made_for_repair.counts.changes, &bad, limits)
        .unwrap_or_else(|err| panic!("{name}: first repair take_in_place failed: {err}"));
    assert_eq!(repaired.repair.deleted, 1, "{name}: unknown block deleted");
    assert!(
        repaired.state.is_none(),
        "{name}: first deletion-only repair stayed on incremental fast path"
    );
    assert_eq!(
        repaired.state_bytes,
        repaired.doc.state().len(),
        "{name}: repaired state_bytes exact"
    );
    assert_eq!(
        repaired.doc.body().expect("repaired body"),
        scratch_repair.repair.body,
        "{name}: repaired body matches scratch"
    );
}

fn cold_files(state: &[u8], limits: &UpdateLimits) -> Vec<String> {
    BodyDoc::from_state(state, limits)
        .expect("cold open")
        .doc
        .file_ids()
}

#[test]
fn prefilled_caches_match_cold_build_across_all_contract_vectors() {
    for one in body_vectors() {
        verify_counted_body_and_first_edits(
            &format!("ydoc-vector/{}", one.vector.name),
            &one.body,
            Some(&one.vector.json),
            &ROOMY,
        );
    }
    for (name, body) in accepted_bodies() {
        verify_counted_body_and_first_edits(&format!("body-vector/{name}"), &body, None, &ROOMY);
    }
}

#[test]
fn from_body_none_prefills_empty_caches_and_fast_paths_first_edit() {
    let made = BodyDoc::from_body_counted(None).expect("empty body counts");
    assert_eq!(made.state, Update::EMPTY_V1, "empty state bytes");
    assert_eq!(made.counts.changes, 0, "zero changes");
    assert_eq!(made.counts.client_changes, 0, "zero client_changes");
    assert_eq!(made.counts.steps_ahead, 0, "zero steps_ahead");
    assert!(made.doc.file_ids().is_empty(), "zero file_ids");

    let update = typed(&new_doc());
    let taken = made
        .doc
        .take_in_place(None, 0, &update, &SERVER_LIMITS)
        .expect("first edit into empty doc");
    assert!(
        taken.state.is_none(),
        "first edit into empty BodyDoc takes incremental fast path"
    );
    assert_eq!(
        taken.state_bytes,
        taken.doc.state().len(),
        "empty doc post-edit state_bytes exact"
    );
}

#[test]
fn multi_client_split_obeys_descending_order_and_zero_reading_steps() {
    for name in [
        "marks",
        "marks and breaks",
        "marks and formulas",
        "marked line",
    ] {
        let document = shapes::shape(name);
        let body = body_of(&document);
        let made = BodyDoc::from_body_counted(Some(&body)).expect("valid shape counts");

        assert_eq!(
            made.state,
            made.doc.state(),
            "{name}: state matches doc.state()"
        );
        assert!(
            made.doc.clients() >= 2 && made.doc.clients() <= 6,
            "{name}: expected 2..=6 split clients, got {}",
            made.doc.clients()
        );
        assert!(
            made.counts.client_changes <= EXPECTED_CHANGES_PER_CLIENT,
            "{name}: per-client changes {} exceeded split target {}",
            made.counts.client_changes,
            EXPECTED_CHANGES_PER_CLIENT
        );
        assert_eq!(
            reading_steps(&made.state, READING_STEPS),
            Ok(0),
            "{name}: multi-client split must have reading_steps == 0"
        );
        assert!(
            made.counts.steps_ahead.saturating_mul(4) < READING_STEPS.saturating_mul(3),
            "{name}: steps_ahead {} too close to remake mark",
            made.counts.steps_ahead
        );

        // Verify the underlying Doc has contiguous descending client IDs and sets its own
        // client_id to the lowest chunk so subsequent local writes keep reading_steps == 0.
        let doc = from_body(&body);
        let mut clients: Vec<u64> = doc
            .transact()
            .state_vector()
            .iter()
            .map(|(c, _)| c.get())
            .collect();
        clients.sort_unstable();
        assert!(
            clients.len() >= 2 && clients.len() <= 6,
            "{name}: from_body client count"
        );
        for pair in clients.windows(2) {
            assert_eq!(
                pair[0] + 1,
                pair[1],
                "{name}: split client IDs must be contiguous descending"
            );
        }
        assert_eq!(
            doc.client_id().get(),
            clients[0],
            "{name}: Doc own client_id must equal lowest split client"
        );

        // Verify cold open + first live edit + deletion-only repair on the split doc.
        verify_counted_body_and_first_edits(name, &document, None, &SERVER_LIMITS);
    }
}

#[test]
fn from_body_by_small_writer_is_deterministic_and_zero_reading_steps() {
    let small_marked = body_of(&json!({
        "type": "doc",
        "content": [{
            "type": "paragraph",
            "content": [{"type": "text", "text": "hello", "marks": [{"type": "bold"}]}]
        }]
    }));
    let small_doc = from_body_by(&small_marked, 1);
    let small_clients: Vec<u64> = small_doc
        .transact()
        .state_vector()
        .iter()
        .map(|(c, _)| c.get())
        .collect();
    assert_eq!(
        small_clients,
        vec![1],
        "small body keeps exact writer ID without shifting"
    );

    let large = body_of(&shapes::shape("marked line"));
    for writer in [0_u64, 1, 2, 1_000_000] {
        let first = state_of(&from_body_by(&large, writer));
        let second = state_of(&from_body_by(&large, writer));
        assert_eq!(
            first, second,
            "writer {writer}: from_body_by is byte-for-byte deterministic"
        );
        assert_eq!(
            reading_steps(&first, READING_STEPS),
            Ok(0),
            "writer {writer}: zero reading_steps without underflow"
        );
        let counts = state_counts(&first).expect("state_counts succeeds");
        assert!(
            counts.client_changes <= EXPECTED_CHANGES_PER_CLIENT,
            "writer {writer}: client_changes {} <= {}",
            counts.client_changes,
            EXPECTED_CHANGES_PER_CLIENT
        );
    }
}

#[test]
fn marked_line_large_round_trip_and_live_edits() {
    let document = shapes::shape("marked line");
    let body = body_of(&document);
    let made = BodyDoc::from_body_counted(Some(&body)).expect("marked line counts");
    let raw = from_state(&made.state, &SERVER_LIMITS)
        .expect("raw opens")
        .doc;
    let rebuilt = from_body(&made.doc.body().expect("projects").expect("non-empty"));
    assert_eq!(
        made.doc.body().expect("made body"),
        fvoci_collab::testing::to_body(&rebuilt).expect("rebuilt body"),
        "marked line round-trips to a fixed point"
    );

    // Edit at the start, middle, and end of the giant marked XmlText, plus an unknown block.
    let start_edit = edited(&raw, |root, txn| {
        if let Some(yrs::XmlOut::Element(p)) = root.get(txn, 0)
            && let Some(yrs::XmlOut::Text(t)) = p.get(txn, 0)
        {
            yrs::Text::insert(&t, txn, 0, "HEAD ");
        }
    });
    let taken1 = made
        .doc
        .take_in_place(None, made.counts.changes, &start_edit, &SERVER_LIMITS)
        .expect("head insert into marked line");
    let scratch1 = take_update(&raw, &start_edit, &SERVER_LIMITS).expect("scratch head insert");
    assert!(
        taken1.state.is_none(),
        "head insert into multi-client marked line stays fast"
    );
    assert_eq!(
        taken1.doc.state(),
        state_of(&scratch1.doc),
        "head insert matches scratch"
    );

    let mid_edit = edited(&scratch1.doc, |root, txn| {
        if let Some(yrs::XmlOut::Element(p)) = root.get(txn, 0)
            && let Some(yrs::XmlOut::Text(t)) = p.get(txn, 0)
        {
            let mid = yrs::Text::len(&t, txn) / 2;
            yrs::Text::insert(&t, txn, mid, " MID ");
        }
    });
    let taken2 = taken1
        .doc
        .take_in_place(None, taken1.changes, &mid_edit, &SERVER_LIMITS)
        .expect("middle insert into marked line");
    let scratch2 = take_update(&scratch1.doc, &mid_edit, &SERVER_LIMITS).expect("scratch mid");
    assert_eq!(
        taken2.doc.state(),
        state_of(&scratch2.doc),
        "middle insert matches scratch"
    );

    // Append a trailing unmarked paragraph after the huge marked paragraph.
    let tail_edit = edited(&scratch2.doc, |root, txn| {
        let p = root.push_back(txn, yrs::XmlElementPrelim::empty("paragraph"));
        p.push_back(txn, XmlTextPrelim::new("TAIL"));
    });
    let taken3 = taken2
        .doc
        .take_in_place(None, taken2.changes, &tail_edit, &SERVER_LIMITS)
        .expect("tail paragraph after marked line");
    let scratch3 = take_update(&scratch2.doc, &tail_edit, &SERVER_LIMITS).expect("scratch tail");
    assert_eq!(
        taken3.doc.state(),
        state_of(&scratch3.doc),
        "tail paragraph matches scratch"
    );
    assert_eq!(
        taken3.doc.body().expect("body projects"),
        scratch3.repair.body,
        "final marked line body matches scratch"
    );
}

#[test]
fn unmarked_shapes_prefill_caches_and_fast_path_first_edit() {
    for name in SHAPES.iter().copied().filter(|n| !n.contains("mark")) {
        let document = shapes::shape(name);
        verify_counted_body_and_first_edits(name, &document, None, &SERVER_LIMITS);
    }
}

#[test]
fn from_stored_mark_attrs_null_and_multi_key_keep_state_and_cache_exact() {
    let cases = [
        (
            "mark attrs null",
            json!({
                "type": "doc",
                "content": [{
                    "type": "paragraph",
                    "content": [{
                        "type": "text",
                        "text": "hello",
                        "marks": [{"type": "bold", "attrs": null}]
                    }]
                }]
            }),
        ),
        (
            "multi-key mark attrs",
            json!({
                "type": "doc",
                "content": [{
                    "type": "paragraph",
                    "content": [{
                        "type": "text",
                        "text": "linked",
                        "marks": [{"type": "link", "attrs": {"href": "https://example.com", "extra": 1}}]
                    }]
                }]
            }),
        ),
    ];
    for (name, doc_json) in cases {
        let body = Body::from_stored(doc_json);
        let made = BodyDoc::from_body_counted(Some(&body))
            .unwrap_or_else(|err| panic!("{name}: counted failed: {err}"));
        assert_eq!(
            made.state,
            made.doc.state(),
            "{name}: MadeBody.state must equal MadeBody.doc.state()"
        );
        assert_eq!(
            made.counts,
            state_counts(&made.state).expect("counts"),
            "{name}: counts exact"
        );
        let raw = from_state(&made.state, &SERVER_LIMITS)
            .expect("from_state")
            .doc;
        let edit = typed(&raw);
        let scratch = take_update(&raw, &edit, &SERVER_LIMITS).expect("scratch");
        let taken = made
            .doc
            .take_in_place(None, made.counts.changes, &edit, &SERVER_LIMITS)
            .unwrap_or_else(|err| panic!("{name}: first edit failed: {err}"));
        assert_eq!(
            taken.state_bytes,
            taken.doc.state().len(),
            "{name}: post-edit state_bytes exact"
        );
        assert_eq!(
            taken.doc.body().expect("body"),
            scratch.repair.body,
            "{name}: post-edit body matches scratch"
        );
        assert_eq!(
            taken.doc.state().len(),
            state_of(&scratch.doc).len(),
            "{name}: post-edit state length matches scratch"
        );
    }
}

#[test]
fn from_stored_allowlist_invalid_nodes_do_not_bypass_repair() {
    let invalid_cases = [
        (
            "heading level 9",
            json!({"type": "doc", "content": [
                {"type": "paragraph", "content": [{"type": "text", "text": "ok"}]},
                {"type": "heading", "attrs": {"level": 9}, "content": [{"type": "text", "text": "bad"}]}
            ]}),
        ),
        (
            "javascript href",
            json!({"type": "doc", "content": [
                {"type": "paragraph", "content": [{"type": "text", "text": "ok"}]},
                {"type": "paragraph", "content": [{
                    "type": "text",
                    "text": "xss",
                    "marks": [{"type": "link", "attrs": {"href": "javascript:alert(1)"}}]
                }]}
            ]}),
        ),
        (
            "non-root listItem",
            json!({"type": "doc", "content": [
                {"type": "paragraph", "content": [{"type": "text", "text": "ok"}]},
                {"type": "listItem", "content": [{"type": "paragraph"}]}
            ]}),
        ),
        (
            "tableRow empty text",
            json!({"type": "doc", "content": [
                {"type": "table", "content": [
                    {"type": "tableRow", "content": [
                        {"type": "text", "text": ""},
                        {"type": "tableCell", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "c"}]}]}
                    ]}
                ]}
            ]}),
        ),
        (
            "nul byte in text",
            json!({"type": "doc", "content": [
                {"type": "paragraph", "content": [{"type": "text", "text": "ok"}]},
                {"type": "paragraph", "content": [{"type": "text", "text": "bad\0char"}]}
            ]}),
        ),
    ];
    for (name, doc_json) in invalid_cases {
        let body = Body::from_stored(doc_json);
        let made = BodyDoc::from_body_counted(Some(&body))
            .unwrap_or_else(|err| panic!("{name}: from_body_counted failed: {err}"));
        let raw = yrs::Doc::with_options(yrs::Options {
            offset_kind: yrs::OffsetKind::Utf16,
            skip_gc: true,
            ..yrs::Options::default()
        });
        raw.get_or_insert_xml_fragment(fvoci_collab::FIELD);
        raw.transact_mut()
            .apply_update(Update::decode_v1(&made.state).expect("decode"))
            .expect("apply");
        let edit = if name == "tableRow empty text" {
            // Empty XmlText inside a tableRow is ignored by `project` when the table is
            // otherwise valid, and must be cleaned when another block triggers repair.
            unknown(&raw)
        } else {
            typed(&raw)
        };
        let scratch = take_update(&raw, &edit, &SERVER_LIMITS)
            .unwrap_or_else(|err| panic!("{name}: scratch take failed: {err}"));
        let taken = made
            .doc
            .take_in_place(None, made.counts.changes, &edit, &SERVER_LIMITS)
            .unwrap_or_else(|err| panic!("{name}: incremental take failed: {err}"));
        assert_eq!(
            taken.repair.deleted, scratch.repair.deleted,
            "{name}: repair deletions must match scratch (not bypassed by prefilled cache)"
        );
        assert!(
            taken.repair.deleted > 0,
            "{name}: invalid stored block must be repaired, not cached as valid"
        );
        if name == "tableRow empty text" {
            assert_eq!(
                taken.repair.deleted, 2,
                "{name}: both unknown block and empty row text deleted"
            );
        }
        assert_eq!(
            taken.doc.body().expect("body"),
            scratch.repair.body,
            "{name}: repaired body matches scratch"
        );
    }
}

#[test]
fn from_stored_over_53_bit_or_deep_attrs_return_unreadable_without_panicking() {
    let over_53 = (1_i64 << 53) + 1;
    let mut deep = json!("leaf");
    for _ in 0..12 {
        deep = json!([deep]);
    }
    let bad_cases = [
        (
            "unmarked >53-bit attr",
            json!({"type": "doc", "content": [
                {"type": "heading", "attrs": {"level": over_53}}
            ]}),
        ),
        (
            "marked >53-bit attr",
            json!({"type": "doc", "content": [
                {"type": "heading", "attrs": {"level": over_53}, "content": [
                    {"type": "text", "text": "x", "marks": [{"type": "bold"}]}
                ]}
            ]}),
        ),
        (
            "unmarked deep attr",
            json!({"type": "doc", "content": [
                {"type": "image", "attrs": {"fileId": deep}}
            ]}),
        ),
        (
            "marked deep attr",
            json!({"type": "doc", "content": [
                {"type": "heading", "attrs": {"level": deep}, "content": [
                    {"type": "text", "text": "x", "marks": [{"type": "bold"}]}
                ]}
            ]}),
        ),
    ];
    for (name, doc_json) in bad_cases {
        let body = Body::from_stored(doc_json);
        let err = BodyDoc::from_body_counted(Some(&body)).expect_err(&format!(
            "{name}: must return Unreadable instead of panicking"
        ));
        assert!(
            matches!(err, UpdateError::Unreadable(_)),
            "{name}: expected UpdateError::Unreadable, got {err:?}"
        );
    }
}

#[test]
fn from_body_fast_path_and_file_ids_work_for_unmarked_and_marked() {
    let unmarked = body_of(&json!({
        "type": "doc",
        "content": [
            {"type": "image", "attrs": {"fileId": "img-a", "alt": "a"}},
            {"type": "paragraph", "content": [{"type": "text", "text": "plain"}]},
            {"type": "file", "attrs": {"fileId": "file-b"}}
        ]
    }));
    let marked = body_of(&json!({
        "type": "doc",
        "content": [
            {"type": "file", "attrs": {"fileId": "file-1"}},
            {"type": "paragraph", "content": [
                {"type": "text", "text": "hi", "marks": [{"type": "bold"}]}
            ]},
            {"type": "image", "attrs": {"fileId": "img-2", "alt": "b"}}
        ]
    }));
    for (name, body, expected_files, expect_fast_first_edit) in [
        ("unmarked", unmarked, vec!["img-a", "file-b"], false),
        ("marked", marked, vec!["file-1", "img-2"], true),
    ] {
        let doc = BodyDoc::from_body(Some(&body));
        let expected_owned: Vec<String> = expected_files.into_iter().map(str::to_owned).collect();
        assert_eq!(doc.file_ids(), expected_owned, "{name}: prefilled file_ids");
        let state = doc.state();
        let changes = state_counts(&state).expect("counts").changes;
        let raw = from_state(&state, &SERVER_LIMITS).expect("raw").doc;
        let edit = typed(&raw);
        let taken = doc
            .take_in_place(None, changes, &edit, &SERVER_LIMITS)
            .unwrap_or_else(|err| panic!("{name}: first edit failed: {err}"));
        if expect_fast_first_edit {
            assert!(
                taken.state.is_none(),
                "{name}: marked BodyDoc::from_body prefills ReadingCache too"
            );
        }
        assert_eq!(
            taken.doc.file_ids(),
            expected_owned,
            "{name}: file_ids kept"
        );
    }
}

#[test]
fn multi_client_u64_max_masking_and_cross_chunk_string_split() {
    let body = body_of(&shapes::shape("marked line"));
    let doc = from_body_by(&body, u64::MAX);
    let state = state_of(&doc);
    assert_eq!(
        reading_steps(&state, READING_STEPS),
        Ok(0),
        "u64::MAX writer masks to 53 bits with zero reading_steps"
    );
    let mut clients: Vec<u64> = doc
        .transact()
        .state_vector()
        .iter()
        .map(|(c, _)| c.get())
        .collect();
    clients.sort_unstable();
    assert!(clients.len() >= 2, "marked line splits across clients");
    assert_eq!(
        *clients.last().expect("max client"),
        (1_u64 << 53) - 1,
        "highest client equals 53-bit MAX_WRITER"
    );

    // Build a multi-client marked body with multi-char text spans so a live edit splits a
    // String item right across a chunk boundary (where right_origin is in a higher client).
    let mut runs = Vec::with_capacity(42_000);
    for i in 0..42_000 {
        if i % 2 == 0 {
            runs.push(json!({
                "type": "text",
                "text": "abcd",
                "marks": [
                    {"type": "link", "attrs": {"href": "/"}},
                    {"type": "bold"},
                    {"type": "italic"},
                    {"type": "strike"},
                    {"type": "code"}
                ]
            }));
        } else {
            runs.push(json!({"type": "text", "text": "efgh"}));
        }
    }
    let split_body = body_of(&json!({
        "type": "doc",
        "content": [{"type": "paragraph", "content": runs}]
    }));
    let made = BodyDoc::from_body_counted(Some(&split_body)).expect("split body counts");
    assert!(
        made.doc.clients() >= 2,
        "multi-char marked doc splits clients"
    );
    let raw = from_state(&made.state, &SERVER_LIMITS)
        .expect("raw opens")
        .doc;
    let split_edit = edited(&raw, |root, txn| {
        if let Some(yrs::XmlOut::Element(p)) = root.get(txn, 0)
            && let Some(yrs::XmlOut::Text(t)) = p.get(txn, 0)
        {
            let offset = (yrs::Text::len(&t, txn) / 2) + 3;
            yrs::Text::insert(&t, txn, offset, "X");
        }
    });
    let scratch = take_update(&raw, &split_edit, &SERVER_LIMITS).expect("scratch split");
    let taken = made
        .doc
        .take_in_place(None, made.counts.changes, &split_edit, &SERVER_LIMITS)
        .expect("incremental split");
    assert_eq!(
        taken.doc.state(),
        state_of(&scratch.doc),
        "cross-chunk string split matches scratch"
    );
    assert_eq!(
        taken.state_bytes,
        taken.doc.state().len(),
        "cross-chunk string split state_bytes exact"
    );
}

#[test]
fn adjacent_same_mark_coalescing_and_link_href_change_with_bold_held_open() {
    let doc_json = json!({
        "type": "doc",
        "content": [{
            "type": "paragraph",
            "content": [
                {"type": "text", "text": "part1-", "marks": [
                    {"type": "bold"},
                    {"type": "link", "attrs": {"href": "https://example.com/a"}}
                ]},
                {"type": "text", "text": "part2-", "marks": [
                    {"type": "link", "attrs": {"href": "https://example.com/a"}},
                    {"type": "bold"}
                ]},
                {"type": "text", "text": "part3", "marks": [
                    {"type": "bold"},
                    {"type": "link", "attrs": {"href": "https://example.com/b"}}
                ]}
            ]
        }]
    });
    let body = body_of(&doc_json);
    let made = BodyDoc::from_body_counted(Some(&body)).expect("counted");
    assert_eq!(made.state, made.doc.state(), "canonical bytes match");
    assert_eq!(
        reading_steps(&made.state, READING_STEPS),
        Ok(0),
        "zero reading_steps"
    );
    let projected = made.doc.body().expect("body").expect("non-empty");
    let content = projected.document()["content"][0]["content"]
        .as_array()
        .expect("paragraph content");
    assert_eq!(content.len(), 2, "identical-mark runs coalesced into one");
    assert_eq!(content[0]["text"], "part1-part2-", "coalesced text");
    assert_eq!(content[1]["text"], "part3", "changed href starts new run");
    assert_eq!(
        content[1]["marks"],
        json!([
            {"type": "link", "attrs": {"href": "https://example.com/b"}},
            {"type": "bold"}
        ]),
        "bold remained open across link href transition"
    );
}
