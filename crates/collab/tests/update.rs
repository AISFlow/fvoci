//! Taking a client's update: what is refused before anything is projected, and that
//! the document an update is tried on stays as it was.

mod support;

use std::time::{Duration, Instant};

use fvoci_collab::testing::{changes_of, copy_of, to_body, with_update};
use fvoci_collab::{
    BodyDoc, FIELD, MAX_CLIENTS, TakeError, UpdateError, UpdateLimits, from_body, from_state,
    new_doc, state_of, take_update,
};
use serde_json::json;
use support::wire::{Change, Content, Parent, Run};
use support::{ROOMY, body_of, json_of};
use yrs::updates::decoder::Decode;
use yrs::{
    Doc, GetString, ReadTxn, StateVector, Text, Transact, Update, XmlElementPrelim, XmlFragment,
    XmlTextPrelim,
};

/// A Y document of one paragraph, as the server makes it.
fn opened(text: &str) -> Doc {
    from_body(&body_of(&json!({"type": "doc", "content": [
        {"type": "paragraph", "content": [{"type": "text", "text": text}]},
    ]})))
}

/// What a client that holds `doc` sends after appending a paragraph: the update of
/// that change alone.
fn appended(doc: &Doc, text: &str) -> Vec<u8> {
    let client = copy_of(doc).expect("the document can be read again");
    let before = client.transact().state_vector();
    {
        let root = client.get_or_insert_xml_fragment(FIELD);
        let mut txn = client.transact_mut();
        let paragraph = root.push_back(&mut txn, XmlElementPrelim::empty("paragraph"));
        paragraph.push_back(&mut txn, XmlTextPrelim::new(text));
    }
    client.transact().encode_diff_v1(&before)
}

fn texts(doc: &Doc) -> Vec<String> {
    let body = to_body(doc).expect("a body");
    json_of(body.as_ref())["content"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|block| {
            block["content"][0]["text"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .collect()
}

#[test]
fn an_update_is_applied_to_a_copy_and_the_document_stays() {
    let doc = opened("first");
    let before = state_of(&doc);
    let update = appended(&doc, "second");
    let with = with_update(&doc, &update, &ROOMY).expect("the update is taken");
    assert_eq!(texts(&with), ["first", "second"]);
    assert_eq!(state_of(&doc), before, "the document itself is as it was");
    assert_eq!(texts(&doc), ["first"]);
    // The same update again changes nothing.
    let again = with_update(&with, &update, &ROOMY).expect("the update is taken again");
    assert_eq!(state_of(&again), state_of(&with));
}

#[test]
fn a_copy_shares_nothing_with_its_document() {
    let doc = opened("first");
    let copy = copy_of(&doc).expect("the document can be read again");
    assert_eq!(state_of(&copy), state_of(&doc));
    {
        let root = copy.get_or_insert_xml_fragment(FIELD);
        let mut txn = copy.transact_mut();
        root.remove_range(&mut txn, 0, 1);
    }
    assert_eq!(to_body(&copy), Ok(None));
    assert_eq!(texts(&doc), ["first"]);
}

#[test]
fn an_update_over_the_limit_or_that_is_none_is_refused() {
    let doc = opened("first");
    let update = appended(&doc, "second");
    let tight = UpdateLimits {
        update_bytes: update.len() - 1,
        ..ROOMY
    };
    assert_eq!(
        with_update(&doc, &update, &tight).map(|_| ()),
        Err(UpdateError::TooLarge {
            size: update.len(),
            limit: update.len() - 1,
        })
    );
    let exact = UpdateLimits {
        update_bytes: update.len(),
        ..ROOMY
    };
    assert!(with_update(&doc, &update, &exact).is_ok(), "at the limit");

    for bytes in [&b""[..], &b"\xff\xff\xff"[..], &update[..update.len() / 2]] {
        assert!(
            matches!(
                with_update(&doc, bytes, &ROOMY),
                Err(UpdateError::Unreadable(_))
            ),
            "{bytes:?}"
        );
    }
}

/// A client's second change without its first: the second waits in the document
/// instead of taking effect, and is refused.
#[test]
fn an_update_that_builds_on_changes_the_document_lacks_is_refused() {
    let doc = opened("first");
    let client = copy_of(&doc).expect("the document can be read again");
    let root = client.get_or_insert_xml_fragment(FIELD);
    let start = client.transact().state_vector();
    {
        let mut txn = client.transact_mut();
        let paragraph = root.push_back(&mut txn, XmlElementPrelim::empty("paragraph"));
        paragraph.push_back(&mut txn, XmlTextPrelim::new("second"));
    }
    let middle = client.transact().state_vector();
    {
        let mut txn = client.transact_mut();
        let paragraph = root.push_back(&mut txn, XmlElementPrelim::empty("paragraph"));
        paragraph.push_back(&mut txn, XmlTextPrelim::new("third"));
    }
    let first_change = client.transact().encode_diff_v1(&start);
    let second_change = client.transact().encode_diff_v1(&middle);
    assert_ne!(first_change, second_change);

    assert_eq!(
        with_update(&doc, &second_change, &ROOMY).map(|_| ()),
        Err(UpdateError::MissingChanges)
    );
    let with = with_update(&doc, &first_change, &ROOMY).expect("in order");
    assert_eq!(texts(&with), ["first", "second", "third"]);
}

/// Changes of a client nobody has heard of that begin at clock 100: the library would
/// take them and leave the hundred before them open.
#[test]
fn an_update_that_begins_after_a_hole_is_refused() {
    let doc = opened("first");
    let text_under_root = |clock: u8| {
        let mut bytes = vec![1, 1, 9, clock, 7, 1, 7];
        bytes.extend_from_slice(b"default");
        bytes.extend_from_slice(&[3, 9]);
        bytes.extend_from_slice(b"paragraph");
        bytes.push(0);
        bytes
    };
    assert_eq!(
        with_update(&doc, &text_under_root(100), &ROOMY).map(|_| ()),
        Err(UpdateError::MissingChanges)
    );
    let with = with_update(&doc, &text_under_root(0), &ROOMY).expect("from the beginning");
    assert_eq!(texts(&with).len(), 2, "the paragraph the update adds");
}

/// An update made against one making of a body does not land in another, though both
/// hold the same body: each is written by a client of its own.
#[test]
fn an_update_for_another_document_is_refused() {
    let one = opened("the same body");
    let other = opened("the same body");
    let typed = {
        let client = copy_of(&one).expect("the document can be read again");
        let before = client.transact().state_vector();
        let root = client.get_or_insert_xml_fragment(FIELD);
        let mut txn = client.transact_mut();
        let Some(yrs::types::xml::XmlOut::Element(line)) = root.first_child() else {
            panic!("the body is a paragraph");
        };
        let Some(yrs::types::xml::XmlOut::Text(text)) = line.first_child() else {
            panic!("the paragraph is a text");
        };
        text.insert(&mut txn, 4, "typed ");
        drop(txn);
        client.transact().encode_diff_v1(&before)
    };
    let with = with_update(&one, &typed, &ROOMY).expect("taken where it was made");
    assert_eq!(texts(&with), ["the typed same body"]);
    assert_eq!(
        with_update(&other, &typed, &ROOMY).map(|_| ()),
        Err(UpdateError::MissingChanges)
    );
}

/// An update may write only into the fragment that holds the body.
#[test]
fn an_update_that_writes_to_another_root_is_refused() {
    let doc = opened("first");
    let other_root = {
        let client = Doc::new();
        let text = client.get_or_insert_text("notes");
        text.insert(&mut client.transact_mut(), 0, "aside");
        assert_eq!(text.get_string(&client.transact()), "aside");
        client
            .transact()
            .encode_state_as_update_v1(&StateVector::default())
    };
    assert_eq!(
        with_update(&doc, &other_root, &ROOMY).map(|_| ()),
        Err(UpdateError::UnexpectedRoot("notes".to_owned()))
    );
}

/// A Y document keeps a trace of what was deleted, so it is the document's size that
/// is limited, not only the body's.
#[test]
fn an_update_that_leaves_the_document_too_large_is_refused() {
    let doc = opened("first");
    let size = state_of(&doc).len();
    let update = appended(&doc, &"x".repeat(1000));
    let limits = |state_bytes| UpdateLimits {
        state_bytes,
        ..ROOMY
    };
    let grown = with_update(&doc, &update, &ROOMY).expect("taken with room");
    let larger = state_of(&grown).len();
    assert!(
        larger > size + 1000,
        "the document grew by the text and more"
    );
    assert!(
        with_update(&doc, &update, &limits(larger)).is_ok(),
        "at the limit"
    );
    assert_eq!(
        with_update(&doc, &update, &limits(larger - 1)).map(|_| ()),
        Err(UpdateError::DocumentTooLarge {
            size: larger,
            limit: larger - 1,
        })
    );
}

/// The state of a document is an update that brings an empty one to the same state,
/// here and in the Y library's own reading.
#[test]
fn the_state_of_a_document_is_an_update() {
    let doc = opened("first");
    let state = state_of(&doc);
    assert!(Update::decode_v1(&state).is_ok(), "the library reads it");
    let from_state = with_update(&new_doc(), &state, &ROOMY).expect("taken");
    assert_eq!(state_of(&from_state), state);
}

/// A Y document can be handed from one task to another and used from several: the room
/// that will hold one does not have to stay on a thread.
#[test]
fn a_document_may_cross_threads() {
    fn assert_send_and_sync<T: Send + Sync + 'static>() {}
    assert_send_and_sync::<Doc>();
    let doc = opened("first");
    let moved = std::thread::spawn(move || texts(&doc))
        .join()
        .expect("the thread");
    assert_eq!(moved, ["first"]);
}

/// What the room that holds a document pays for each update, measured on bodies of
/// text and of nodes up to the largest (the most text, and the most nodes), each with
/// room left for a paragraph more, and printed: run with
/// `--release -- --nocapture` to read it. Nothing is asserted about the times.
#[test]
fn the_cost_of_taking_an_update_into_the_largest_bodies() {
    use fvoci_model::{BODY_MAX_BYTES, BODY_MAX_NODES};
    use std::time::Instant;

    let frame =
        r#"{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":""}]}]}"#;
    let of_text = |bytes: usize| {
        json!({"type": "doc", "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": "a".repeat(bytes - 2 * frame.len())}]},
        ]})
    };
    let paragraphs =
        |count: usize| json!({"type": "doc", "content": vec![json!({"type": "paragraph"}); count]});

    for (name, document) in [
        ("4 KiB of text", of_text(4 * 1024)),
        ("64 KiB of text", of_text(64 * 1024)),
        ("the most text", of_text(BODY_MAX_BYTES)),
        ("1,000 nodes", paragraphs(1000)),
        ("the most nodes", paragraphs(BODY_MAX_NODES - 3)),
    ] {
        let body = body_of(&document);
        let timed = Instant::now();
        let doc = from_body(&body);
        let made = timed.elapsed();
        let timed = Instant::now();
        let state = state_of(&doc);
        let encoded = timed.elapsed();
        let timed = Instant::now();
        let copy = copy_of(&doc).expect("the document can be read again");
        let copied = timed.elapsed();
        let timed = Instant::now();
        let projected = to_body(&copy).expect("a body");
        let read = timed.elapsed();
        assert!(projected.is_some(), "{name}: a body");
        let update = appended(&doc, "typed");
        let timed = Instant::now();
        let with = with_update(&doc, &update, &ROOMY).expect("taken");
        let taken = timed.elapsed();
        assert_eq!(
            texts(&with).last().map(String::as_str),
            Some("typed"),
            "{name}"
        );
        println!(
            "{name}: state {} bytes; from_body {made:?}, state_of {encoded:?}, copy_of {copied:?}, \
             to_body {read:?}, with_update of {} bytes {taken:?}",
            state.len(),
            update.len()
        );
    }
}

/// An update of the clients `clients`, each of one change of deleted content after the
/// change of the client before; the first after `after`, or at the front of the body.
fn of_clients(clients: std::ops::RangeInclusive<u64>, after: Option<(u64, u64)>) -> Vec<u8> {
    let mut before = after;
    let runs = clients
        .map(|client| {
            let change = match before {
                None => Change::under(Parent::body(), Content::Deleted(1)),
                Some(left) => Change::after(left, Content::Deleted(1)),
            };
            before = Some((client, 0));
            Run {
                client,
                clock: 0,
                changes: vec![change],
            }
        })
        .collect();
    support::wire::Update {
        runs,
        deleted: Vec::new(),
    }
    .bytes()
}

/// The clients a document names with an update are those it names and those of the
/// update it does not, and an update that leaves more than the limit is refused.
#[test]
fn an_update_is_refused_past_the_clients_a_document_may_name() {
    let limits = UpdateLimits {
        clients: Some(100),
        ..ROOMY
    };
    let sixty = with_update(&new_doc(), &of_clients(1..=60, None), &limits).expect("60");
    let hundred = with_update(&sixty, &of_clients(61..=100, Some((60, 0))), &limits).expect("100");
    assert_eq!(
        with_update(&sixty, &of_clients(61..=101, Some((60, 0))), &limits).err(),
        Some(UpdateError::TooManyClients {
            clients: 101,
            limit: 100,
        })
    );
    // A client the document names already is not counted again.
    let again = support::wire::Update::of(1, 1, vec![Change::after((1, 0), Content::Deleted(1))]);
    with_update(&hundred, &again.bytes(), &limits).expect("a client it names");
}

/// The changes of a client are counted in the document an update leaves, deleted ones
/// too, and an update that leaves a client with more than the limit is refused: the
/// library moves every later change of a client each time it cuts one.
#[test]
fn an_update_is_refused_past_the_changes_a_client_may_have() {
    let limits = UpdateLimits {
        client_changes: Some(100),
        ..ROOMY
    };
    // Changes of deleted content that each name the body as their parent and no
    // neighbour: none is joined to the one before it.
    let of = |clock: u64, count: usize| {
        support::wire::Update::of(
            1,
            clock,
            vec![Change::under(Parent::body(), Content::Deleted(1)); count],
        )
        .bytes()
    };
    let sixty = with_update(&new_doc(), &of(0, 60), &limits).expect("60");
    let hundred = with_update(&sixty, &of(60, 40), &limits).expect("100");
    assert_eq!(
        with_update(&hundred, &of(100, 1), &limits).err(),
        Some(UpdateError::TooManyChanges {
            changes: 101,
            limit: 100,
        })
    );
    // Another client's changes are its own.
    let other = support::wire::Update::of(
        2,
        0,
        vec![Change::under(Parent::body(), Content::Deleted(1)); 100],
    );
    with_update(&hundred, &other.bytes(), &limits).expect("another client's hundred");
}

/// An update of a hundred thousand clients is walked in the time it takes to read it,
/// and refused before the document is read, which would take the square of them.
#[test]
fn an_update_of_many_clients_is_refused_before_the_document_is_read() {
    const CLIENTS: u64 = 100_000;
    let many = of_clients(1..=CLIENTS, None);
    let start = Instant::now();
    assert_eq!(changes_of(&many), CLIENTS);
    let refused = with_update(&new_doc(), &many, &ROOMY);
    let took = start.elapsed();
    assert_eq!(
        refused.err(),
        Some(UpdateError::TooManyClients {
            clients: usize::try_from(CLIENTS).expect("a count fits"),
            limit: MAX_CLIENTS,
        })
    );
    assert!(
        took < Duration::from_secs(3),
        "walked and refused in {took:?}"
    );
}

/// Whole-document `total_changes` counts changes across all clients together (deleted ones
/// too) and is enforced identically by `with_update`, `take_update`, `from_state`,
/// `BodyDoc::from_state`, and `BodyDoc::settle`, both with and without `reading_steps`.
#[test]
fn an_update_is_refused_past_the_total_changes_a_document_may_have() {
    let limits = UpdateLimits {
        client_changes: Some(100),
        total_changes: Some(100),
        ..ROOMY
    };
    let uncounted = UpdateLimits {
        reading_steps: None,
        ..limits
    };
    let of_client = |client: u64, count: usize| {
        support::wire::Update::of(
            client,
            0,
            vec![Change::under(Parent::body(), Content::Deleted(1)); count],
        )
        .bytes()
    };

    // Two clients each stay well under `client_changes` (60 and 40 <= 100), reaching
    // exactly 100 total changes.
    let sixty = with_update(&new_doc(), &of_client(1, 60), &limits).expect("60 total");
    let hundred = with_update(&sixty, &of_client(2, 40), &limits).expect("100 total at exact cap");
    let one_more = of_client(3, 1);

    // Client 3 has only 1 change (< client_changes), yet total reaches 101 (> 100).
    assert_eq!(
        with_update(&hundred, &one_more, &limits).err(),
        Some(UpdateError::TooManyTotalChanges {
            changes: 101,
            limit: 100,
        })
    );
    assert_eq!(
        with_update(&hundred, &one_more, &uncounted).err(),
        Some(UpdateError::TooManyTotalChanges {
            changes: 101,
            limit: 100,
        }),
        "reading_steps: None settle path also enforces total_changes"
    );
    assert_eq!(
        take_update(&hundred, &one_more, &limits).err(),
        Some(TakeError::Update(UpdateError::TooManyTotalChanges {
            changes: 101,
            limit: 100,
        }))
    );

    // `from_state` and `BodyDoc::from_state` enforce `total_changes` both with and without
    // `reading_steps`.
    let state_100 = state_of(&hundred);
    assert!(from_state(&state_100, &limits).is_ok(), "100 fits cap 100");
    assert!(
        BodyDoc::from_state(&state_100, &limits).is_ok(),
        "BodyDoc::from_state fits cap 100"
    );
    let tight = UpdateLimits {
        total_changes: Some(99),
        ..limits
    };
    let tight_uncounted = UpdateLimits {
        reading_steps: None,
        ..tight
    };
    for check_limits in [tight, tight_uncounted] {
        assert_eq!(
            from_state(&state_100, &check_limits).err(),
            Some(TakeError::Update(UpdateError::TooManyTotalChanges {
                changes: 100,
                limit: 99,
            }))
        );
        assert_eq!(
            BodyDoc::from_state(&state_100, &check_limits).err(),
            Some(TakeError::Update(UpdateError::TooManyTotalChanges {
                changes: 100,
                limit: 99,
            }))
        );
    }

    // `BodyDoc::settle` enforces `total_changes` and returns the intact document on failure.
    let opened = BodyDoc::from_state(&state_100, &limits).expect("open at 100");
    let settle_err = opened
        .doc
        .settle(state_100, &tight)
        .expect_err("settle over total_changes refused");
    assert_eq!(
        settle_err.error,
        UpdateError::TooManyTotalChanges {
            changes: 100,
            limit: 99,
        }
    );
}
