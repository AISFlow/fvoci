//! What editors send to the largest documents is taken: a letter typed, pastes, undos
//! and what an editor sends when it is back from offline, into documents of as many
//! nodes, and as many bytes, as a body may have. How long each takes is measured in a
//! release build (`cost.rs`, `honest`). None of it comes near the steps a document may
//! take to read: a body's own document takes none, and an edit a few thousand.

mod support;

use std::time::{Duration, Instant};

use fvoci_collab::{
    BodyDoc, MAX_CLIENT_CHANGES, MAX_TOTAL_CHANGES, READING_STEPS, client_changes, reading_steps,
    state_counts, steps_ahead, take_update,
};
use fvoci_model::{BODY_MAX_BYTES, BODY_MAX_NODES};
use support::body_of;
use support::honest::{Between, SERVED, bodies, document, edits, marked, marked_paragraph};

/// The bodies of the most changes of one client and the largest states there are (of
/// the ways tried: letters of every mark in turn with letters of none, line breaks or
/// formulas, of every mark but the link, of a few, in a paragraph or in many) are made
/// into documents under three quarters of the changes a client may have, and within
/// the state a document may be, and are read in no steps: a document made from a body is
/// not made again at once (`OpenDoc::crowded`), and is held.
#[test]
fn the_bodies_of_the_most_changes_fit_a_document() {
    for between in Between::ALL {
        let body = body_of(&marked(between, BODY_MAX_NODES, BODY_MAX_BYTES));
        let state = BodyDoc::from_body(Some(&body)).state();
        let counts = state_counts(&state).expect("a state made here");
        let changes = client_changes(&state).expect("a state made here");
        assert!(
            changes * 4 < MAX_CLIENT_CHANGES * 3,
            "{between:?}: {changes} changes of one client"
        );
        assert!(
            counts.changes <= MAX_TOTAL_CHANGES,
            "{between:?}: {} total changes > {MAX_TOTAL_CHANGES}",
            counts.changes
        );
        let bytes = state.len();
        assert!(
            bytes <= SERVED.state_bytes,
            "{between:?}: {bytes} bytes of state"
        );
        let steps = reading_steps(&state, READING_STEPS).expect("a state made here");
        assert_eq!(steps, 0, "{between:?}: steps to read it");
        // Nor made again each time it is closed (`OpenDoc::worn`, an eighth): one
        // more client may leave it with each of its changes looked at once.
        let ahead = steps_ahead(&state).expect("a state made here");
        println!("{between:?}: {ahead} steps ahead");
        assert!(
            ahead * 16 < READING_STEPS,
            "{between:?}: {ahead} steps one more client may leave it with"
        );
    }
}

/// One paragraph of letters in turn with every mark and with none, as many as a body
/// may have, is made into a document well within a worker's time: each mark is written
/// in one pass over the text (0.6 s for 49,600 letters, release). Its runs still each
/// cut the text and move the changes after the cut, more than the text's length. Written
/// a letter at a time, each walked the text from its start, and 8,000 such letters took
/// 9 s.
#[test]
fn a_paragraph_of_many_marks_is_made_well_within_a_workers_time() {
    let document = marked_paragraph(BODY_MAX_NODES, BODY_MAX_BYTES);
    let letters = document["content"][0]["content"]
        .as_array()
        .map_or(0, Vec::len);
    assert!(letters > 40_000, "{letters} letters");
    let body = body_of(&document);
    let start = Instant::now();
    let doc = BodyDoc::from_body(Some(&body));
    let took = start.elapsed();
    assert!(
        took < Duration::from_secs(5),
        "{letters} letters made in {took:?}"
    );
    let state = doc.state();
    let changes = client_changes(&state).expect("a state made here");
    assert!(
        changes * 4 < MAX_CLIENT_CHANGES * 3,
        "{changes} changes of one client"
    );
    let steps = reading_steps(&state, READING_STEPS).expect("a state made here");
    assert_eq!(steps, 0, "steps to read it");
}

/// What editors send is taken, and leaves a document far from the steps a document may
/// take to read.
#[test]
fn what_editors_send_to_the_largest_documents_is_taken() {
    for (name, body) in bodies() {
        let doc = document(&body);
        for edit in edits(&doc, &SERVED) {
            assert!(
                edit.update.len() <= SERVED.update_bytes,
                "{name}, {}: {} bytes",
                edit.name,
                edit.update.len()
            );
            let taken = match take_update(&edit.doc, &edit.update, &SERVED) {
                Ok(taken) => taken,
                Err(error) => panic!("{name}, {}: {error}", edit.name),
            };
            // Far from the steps at which a document is made again (three quarters):
            // the changes of the body's client, each looked at once while the edit
            // waits for the one it names.
            assert!(
                taken.reading_steps * 64 < READING_STEPS,
                "{name}, {}: {} steps to read it",
                edit.name,
                taken.reading_steps
            );
            // And from where a document is made again as it is closed (an eighth of
            // them, by what one more client may leave it with).
            assert!(
                taken.steps_ahead * 8 < READING_STEPS,
                "{name}, {}: {} steps ahead",
                edit.name,
                taken.steps_ahead
            );
        }
    }
}

/// Regression test for the worst-case allowlist-valid 100,000-node / 8 MiB body:
/// 1 doc + 1 heading + 49,999 5-mark texts interleaved with `inlineMath` and `hardBreak`
/// nodes (~660.3k changes). Verifies `Body::parse` accepts it, `BodyDoc::from_body_counted`
/// fits within `MAX_TOTAL_CHANGES` (`768 * 1024`) and `16 MiB state_bytes` with >120k
/// changes of editing headroom, `reading_steps == 0`, and a live edit succeeds fast.
#[test]
fn worst_case_valid_marked_math_body_opens_and_edits_within_total_changes() {
    use serde_json::json;

    let marked_text = json!({
        "type": "text",
        "text": "a",
        "marks": [
            {"type": "bold"},
            {"type": "italic"},
            {"type": "strike"},
            {"type": "code"},
            {"type": "link", "attrs": {"href": "/"}},
        ]
    });
    let inline_math = json!({"type": "inlineMath", "attrs": {"latex": "x"}});
    let hard_break = json!({"type": "hardBreak"});
    let math_count = 10_350_usize;
    let total_pairs = 49_996_usize;
    let mut inline = Vec::with_capacity(BODY_MAX_NODES - 4);
    for idx in 0..total_pairs {
        inline.push(marked_text.clone());
        if idx < math_count {
            inline.push(inline_math.clone());
        } else {
            inline.push(hard_break.clone());
        }
    }
    inline.push(marked_text);
    // Nodes: 1 doc + 1 heading + 49,996*2 + 1 = 99,995 (room for append_paragraph = +2).
    let doc_json = json!({
        "type": "doc",
        "content": [{"type": "heading", "attrs": {"level": 1}, "content": inline}]
    });
    let body = body_of(&doc_json);
    let made = BodyDoc::from_body_counted(Some(&body)).expect("worst-case valid body builds");
    assert!(
        made.counts.changes > 660_000,
        "constructed body exceeds 660k changes: {}",
        made.counts.changes
    );
    assert!(
        made.counts.changes + 120_000 <= MAX_TOTAL_CHANGES,
        "leaves >120k changes headroom under MAX_TOTAL_CHANGES ({}): {}",
        MAX_TOTAL_CHANGES,
        made.counts.changes
    );
    assert!(
        made.state.len() <= SERVED.state_bytes,
        "state fits 16 MiB: {} bytes",
        made.state.len()
    );
    assert_eq!(reading_steps(&made.state, READING_STEPS), Ok(0));
    assert!(
        made.counts.steps_ahead * 16 < READING_STEPS,
        "steps_ahead {} well below READING_STEPS/16",
        made.counts.steps_ahead
    );

    let editor = fvoci_collab::testing::Editor::join(999, &made.state);
    let edit = editor.append_paragraph("tail");
    let taken = made
        .doc
        .take_in_place(None, made.counts.changes, &edit, &SERVED)
        .expect("live edit succeeds on worst-case valid body");
    assert!(
        taken.state.is_none(),
        "live edit stays on incremental fast path"
    );
}
