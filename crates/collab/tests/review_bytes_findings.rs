//! Updates from anybody that passed the walk before the Y library and still broke what
//! is stored or what the server relies on, each kept as a test of its own.
//!
//! Each test here states what the server that calls this crate relies on. One that
//! fails names a defect; what it shows is in the message of the assertion.

mod support;

use fvoci_collab::testing::{repair, to_body, with_update};
use fvoci_collab::{
    TakeError, UpdateError, from_body_by, from_state, new_doc, state_of, take_update,
};
use support::explore::session;
use support::wire::{Change, Content, Parent, Update, value};
use support::{ROOMY, from_base64};

/// A paragraph under the body's root by client 5, and the update that makes it.
fn paragraph() -> Update {
    Update::of(
        5,
        0,
        vec![Change::under(Parent::body(), Content::element("paragraph"))],
    )
}

/// The library reads one text of JSON more than the number before them says, and
/// writes exactly as many as it says: what it took, it cannot read back. The walk
/// follows the reading, so an update of twelve bytes is taken, and the state of the
/// document it leaves is no update any more. The next update for that document, from
/// anybody, panics where the document is copied.
#[test]
fn an_update_with_texts_of_json_leaves_a_state_that_can_be_read_again() {
    let update = Update::of(
        5,
        0,
        vec![Change::under(
            Parent::body(),
            Content::Json {
                count: 0,
                texts: vec![b"1".to_vec()],
            },
        )],
    )
    .bytes();
    let Ok(with) = with_update(&new_doc(), &update, &ROOMY) else {
        // Refused: the document never holds what it cannot write back.
        return;
    };
    let stored = state_of(&with);
    assert!(
        with_update(&new_doc(), &stored, &ROOMY).is_ok(),
        "the update {update:?} was taken, and the state it leaves ({stored:?}) is refused"
    );
}

/// The same, where it ends the server: the document that took the update is given the
/// next one, an empty update. (This test dies of the panic in `copy_of`: "a document's
/// own state is an update".)
#[test]
fn the_update_after_one_with_texts_of_json_does_not_panic() {
    let update = Update::of(
        5,
        0,
        vec![Change::under(
            Parent::body(),
            Content::Json {
                count: 0,
                texts: vec![b"1".to_vec()],
            },
        )],
    )
    .bytes();
    let Ok(with) = with_update(&new_doc(), &update, &ROOMY) else {
        return;
    };
    let empty = Update::default().bytes();
    let _ = with_update(&with, &empty, &ROOMY);
}

/// A whole number of more than 53 bits is taken in the short form (nine bytes hold 62
/// bits), and written back in the form of 64 bits, which the walk refuses: the state
/// of a document that took the update is refused by `with_update`.
#[test]
fn an_update_with_a_large_whole_number_leaves_a_state_that_is_taken_again() {
    let mut update = paragraph();
    update.runs[0].changes.push(Change::member(
        Parent::Change((5, 0)),
        "n",
        Content::values(vec![value::whole((1 << 53) + 1)]),
    ));
    let update = update.bytes();
    let Ok(with) = with_update(&new_doc(), &update, &ROOMY) else {
        return;
    };
    assert!(to_body(&with).is_ok(), "the paragraph is a body");
    let stored = state_of(&with);
    assert_eq!(
        with_update(&new_doc(), &stored, &ROOMY).map(|_| ()),
        Ok::<(), UpdateError>(()),
        "the update was taken, and the state it leaves is refused"
    );
}

/// The first case the search found (seed 111): an update leaves a text that holds
/// something that is no text, and with what fails deleted the root would hold
/// something that is neither a node nor a text. No repair mends that: the repair
/// fails, and deletes nothing, where it used to make its deletions first.
#[test]
fn a_repair_that_fails_leaves_the_document_as_it_was() {
    let before = from_base64(concat!(
        "ASQHAAcBB2RlZmF1bHQDC29yZGVyZWRMaXN0BwAHAAMIbGlzdEl0ZW0HAAcBAwlwYXJhZ3JhcGgHAAcC",
        "BgQABwMHd29yZAkiXIQHChvDqfCfkanigI3wn5Gp4oCN8J+Rp+KAjfCfkaZGBwQEbGluaxR7ImhyZWYi",
        "OiIvYm9hcmRzLzUifcYHCgcLBGxpbmsEbnVsbCgABwAFc3RhcnQBfQFHBwADCXBhcmFncmFwaAcABxoG",
        "BAAHGwEKhAccBQk8Yj5hRgccBGxpbmsUeyJocmVmIjoiL2JvYXJkcy8wIn3GBxwHHQRsaW5rBG51bGxG",
        "ByIGaXRhbGljAnt9wQcjBx0BRgckBnN0cmlrZQJ7fcEHJQcdAUYHJgRjb2RlAnt9wQcnBx0BhgchBml0",
        "YWxpYwRudWxshgcqBnN0cmlrZQRudWxshgcrBGNvZGUEbnVsbEcHGwMKaW5saW5lTWF0aCgABy0FbGF0",
        "ZXgBdxwJ8J+RqeKAjfCfkanigI3wn5Gn4oCN8J+RpsOpRwctBgQABy8cCSJc8J+RqeKAjfCfkanigI3w",
        "n5Gn4oCN8J+RpkYHMARib2xkAnt9hgc9BGJvbGQEbnVsbEYHPgZpdGFsaWMCe32GBz8GaXRhbGljBG51",
        "bGxGB0AGc3RyaWtlAnt9hgdBBnN0cmlrZQRudWxsRgdCBGNvZGUCe32GB0MEY29kZQRudWxsAQcDJQEn",
        "ASkB",
    ));
    let update = from_base64(concat!(
        "AwAHRgYDAIEHMQEGAQdkZWZhdWx0DmJvbGQtLWFiY2RlZmdoC1sxLFsyLFszXV1dgQMHAIcCAQYBAQdk",
        "ZWZhdWx0/////wMHAAMCBgMCAIQHFAZhd29yZCAHAQdkZWZhdWx0Awh0YXNrSXRlbUQHOwLDqQA=",
    ));
    let doc = with_update(&new_doc(), &before, &ROOMY).expect("a body");
    let with = with_update(&doc, &update, &ROOMY).expect("taken");
    let refused = to_body(&with);
    assert!(refused.is_err(), "no body as it is");
    let state = state_of(&with);
    assert!(repair(&with).is_err(), "no repair mends it");
    assert_eq!(state_of(&with), state, "the repair deleted nothing");
    assert_eq!(to_body(&with), refused, "the same refusal");
    // What the server calls refuses the update, and the document stays.
    let state = state_of(&doc);
    assert!(matches!(
        take_update(&doc, &update, &ROOMY),
        Err(TakeError::Body(_))
    ));
    assert_eq!(state_of(&doc), state, "the document is as it was");
}

/// The search found it at seed 19562: a client deletes one letter, the document then
/// needs a repair, and the repaired copy, as the library held it, put the letters of
/// a text in another order than the same document read from its state. An editor that
/// took the update and the deletions held another body than the server stored.
#[test]
fn a_repaired_document_is_the_one_its_state_reads_to() {
    let (failure, _) = session(19_562, 14);
    assert!(failure.is_none(), "{failure:?}");
}

/// The review found it at seed 1109793 (before the search's bodies changed): an update
/// whose text the library places in one of two places in turn, each time the document
/// is read from its state, so that the same stored bytes would open to two bodies.
/// Such an update is not taken. Here it is taken (the bodies of the search have
/// changed since), and this holds that the document opens to the same body each time.
#[test]
fn a_taken_document_opens_to_the_same_body_each_time() {
    let before = from_base64(concat!(
        "ASEHAAcBB2RlZmF1bHQDC29yZGVyZWRMaXN0BwAHAAMIbGlzdEl0ZW0HAAcBAwlwYXJhZ3JhcGhHBwED",
        "CGxpc3RJdGVtBwAHAwMJcGFyYWdyYXBoBwAHBAYEAAcFIPCfkanigI3wn5Gp4oCN8J+Rp+KAjfCfkabt",
        "lZzquIAKhAcTCzxiPiJc7ZWc6riARgcGBGJvbGQCe33GBxMHFARib2xkBG51bGxGBxsGc3RyaWtlAnt9",
        "xgccBxQGc3RyaWtlBG51bGxGBx0EY29kZQJ7fcYHHgcUBGNvZGUEbnVsbMYHEwccBGxpbmsUeyJocmVm",
        "IjoiL2JvYXJkcy83In2GBxoEbGluawRudWxshgciBGJvbGQEbnVsbMYHEwchBml0YWxpYwJ7fYYHIwZp",
        "dGFsaWMEbnVsbEcHBQMKaW5saW5lTWF0aCgAByYFbGF0ZXgBdwRhPGI+KAAHAAVzdGFydAF9BEcHAAMJ",
        "cGFyYWdyYXBoBwAHKQYEAAcqCyDtlZzquIB3b3JkRgcrBGJvbGQCe32GBzEEYm9sZARudWxsRgcyBml0",
        "YWxpYwJ7fYYHMwZpdGFsaWMEbnVsbEYHNARjb2RlAnt9hgc1BGNvZGUEbnVsbEcHKQMFaW1hZ2UoAAc4",
        "BmZpbGVJZAF3CGZpbGUucG5nAQcBHAE=",
    ));
    let update =
        from_base64("AgEAAMEHEgcLBQMBAAcBB2RlZmF1bHQDCGxpc3RJdGVtiAAAAXtDQAAAAAAAAIQAAAV3b3JkIAA=");
    let opened = from_state(&before, &ROOMY).expect("a body");
    let body = opened.repair.body.expect("a body");
    // The document as the server made it of its body, by the writer the search gives,
    // before it wrote each mark in a pass of its own: what it stored so is still opened.
    let base = support::doc_of(&before);
    assert_eq!(state_of(&base), before);
    // Made of the same body now, it is a document of that body.
    let made = from_body_by(&body, 7);
    assert_eq!(to_body(&made), Ok(Some(body)), "the same body");
    if let Ok(taken) = take_update(&base, &update, &ROOMY) {
        let mut state = state_of(&taken.doc);
        for time in 0..3 {
            let opened = from_state(&state, &ROOMY).expect("opened");
            assert_eq!(opened.repair.body, taken.repair.body, "opened {time} times");
            state = state_of(&opened.doc);
        }
    }
}

/// Found by `review_bytes_explore` at seeds 29343 and 37207: non-YATA right origins
/// (`left = None` before an item that itself has a left origin) can flip formatting
/// order between live apply and reload or oscillate block squashing across reloads.
/// Both must either be refused cleanly or leave an identical body after repair and
/// reopen.
#[test]
fn non_yata_right_origins_do_not_diverge_or_oscillate() {
    for seed in [29_343, 37_207] {
        let (failure, _) = session(seed, 14);
        assert!(failure.is_none(), "seed {seed}: {failure:?}");
    }
}
