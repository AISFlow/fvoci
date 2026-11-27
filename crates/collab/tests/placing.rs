//! An update is held to how far the Y library may have to walk through the document to
//! place its changes ([`UpdateLimits::placing_steps`]): what is made to walk a long
//! list over and over is refused, and what an editor sends is taken, in the largest
//! documents too.

mod support;

use fvoci_collab::testing::{placing_steps, to_body};
use fvoci_collab::{
    PLACING_STEPS, READING_STEPS, TakeError, UpdateError, UpdateLimits, from_body, from_body_by,
    from_state, new_doc, state_of, take_update,
};
use fvoci_model::BODY_MAX_NODES;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::{Duration, Instant};
use support::wire::{Change, Content, Parent, Update, value};
use support::{ROOMY, body_of, doc_of, edited, json_of, vectors};

use yrs::types::Attrs;
use yrs::{
    Any, Doc, Text, TransactionMut, Xml, XmlElementPrelim, XmlElementRef, XmlFragment,
    XmlFragmentRef, XmlOut, XmlTextPrelim, XmlTextRef,
};

/// A client above every client the server takes at random (32 bits): the library
/// places its changes after those of the server that claim the same place.
const ABOVE: u64 = 1 << 40;

/// As many children of the root as a body has room for.
const CHILDREN: usize = BODY_MAX_NODES - 2;

fn paragraph(text: &str) -> Value {
    json!({"type": "paragraph", "content": [{"type": "text", "text": text}]})
}

fn opened(blocks: &[Value]) -> Doc {
    from_body(&body_of(&json!({"type": "doc", "content": blocks})))
}

/// The document with the most children under its root: empty paragraphs.
fn widest() -> Doc {
    opened(&vec![json!({"type": "paragraph"}); CHILDREN])
}

/// An update of `count` changes of one client that each name the root as their parent
/// and no neighbour: the library walks past every child of the root to place each.
fn crowding(count: usize) -> Vec<u8> {
    let changes = (0..count)
        .map(|_| Change::under(Parent::body(), Content::Deleted(1)))
        .collect();
    Update::of(ABOVE, 0, changes).bytes()
}

fn count(number: usize) -> u64 {
    u64::try_from(number).expect("a count fits")
}

/// What `changes` that each walk a list of `list` changes of the document, and each
/// other, are charged.
fn charged(changes: usize, list: usize) -> u64 {
    count(changes) * (count(list) + count(changes))
}

/// The fewest changes that are past the limit where each walks a list of `list`.
fn changes_past_the_limit(list: usize) -> usize {
    (1..usize::MAX)
        .find(|changes| charged(*changes, list) > PLACING_STEPS)
        .expect("a number of changes is past it")
}

#[test]
fn changes_made_to_walk_the_children_of_a_node_are_refused_and_fewer_are_taken() {
    let doc = widest();
    let before = state_of(&doc);
    let many = changes_past_the_limit(CHILDREN);
    let taken = take_update(&doc, &crowding(many - 1), &ROOMY).expect("within the limit");
    assert_eq!(taken.repair.deleted, 0);
    let refused = take_update(&doc, &crowding(many), &ROOMY).expect_err("past the limit");
    assert_eq!(
        refused,
        TakeError::Update(UpdateError::PlacedPastTooMany {
            steps: charged(many, CHILDREN),
            limit: PLACING_STEPS,
        })
    );
    assert_eq!(state_of(&doc), before, "the document is as it was");
}

/// Changes the library places from the front of a list are counted the list whichever
/// neighbours they name: each the same child to its right, the root by a change that
/// goes into the list of another change of the update.
#[test]
fn the_changes_are_counted_whichever_neighbours_they_name() {
    let doc = widest();
    let (server, _) = first_run(&state_of(&doc));
    let many = changes_past_the_limit(CHILDREN);
    let child = |number: usize| (server, count(number));
    let right_of_one: Vec<Change> = (0..many)
        .map(|_| Change::Item {
            left: None,
            right: Some(child(7)),
            parent: Parent::body(),
            key: None,
            content: Content::Deleted(1),
        })
        .collect();
    // The first names the root; each of the others names the one before it to its
    // right and nothing to its left, so that it is placed from the front of the root.
    let through_their_own: Vec<Change> = (0..many)
        .map(|number| Change::Item {
            left: None,
            right: number.checked_sub(1).map(|before| (ABOVE, count(before))),
            parent: Parent::body(),
            key: None,
            content: Content::Deleted(1),
        })
        .collect();
    for (name, changes) in [
        ("right of one child", right_of_one),
        ("through changes of their own", through_their_own),
    ] {
        let update = Update::of(ABOVE, 0, changes).bytes();
        let steps = placing_steps(&doc, &update).expect("counted");
        assert!(steps >= charged(many, CHILDREN), "{name}: {steps} steps");
        assert!(
            matches!(
                take_update(&doc, &update, &ROOMY),
                Err(TakeError::Update(UpdateError::PlacedPastTooMany { .. }))
            ),
            "{name}"
        );
    }
}

/// The children of a document made from a body name no neighbour to their left (the
/// last goes in first, each other at the front), so that a change placed after one of
/// them walks past nothing: each such change is charged a few steps, as many as there
/// are, and what an editor inserts at places all over a long list is taken.
#[test]
fn changes_after_children_made_from_a_body_are_charged_what_stands_below_them() {
    const INSERTED: usize = 340;
    let doc = widest();
    let (server, _) = first_run(&state_of(&doc));
    let many = changes_past_the_limit(CHILDREN);
    let after_each: Vec<Change> = (0..many)
        .map(|number| Change::after((server, count(number)), Content::Deleted(1)))
        .collect();
    let update = Update::of(ABOVE, 0, after_each).bytes();
    let steps = placing_steps(&doc, &update).expect("counted");
    assert!(steps <= count(4 * many), "{steps} steps for {many} changes");
    let start = Instant::now();
    take_update(&doc, &update, &ROOMY).expect("taken");
    let took = start.elapsed();
    assert!(took < Duration::from_secs(5), "taken in {took:?}");

    let doc = opened(&vec![json!({"type": "paragraph"}); CHILDREN - INSERTED]);
    let scattered = edited(&doc, |root, txn| {
        // From the end back, so that each place is of the list as it was.
        for at in (0..INSERTED).rev() {
            let index = u32::try_from(1 + at * 140).expect("a place fits");
            root.insert(txn, index, XmlElementPrelim::empty("paragraph"));
        }
    });
    let steps = placing_steps(&doc, &scattered).expect("counted");
    assert!(
        steps <= count(8 * INSERTED),
        "{steps} steps for {INSERTED} paragraphs"
    );
    take_update(&doc, &scattered, &ROOMY).expect("taken");
}

/// A change that says again the first of a client's changes the document has, and goes
/// on past them, is cut by the library where the document's end: what is left is
/// placed after that change, whatever the parent and the neighbours the update names,
/// and walks past what stands below it. It is charged so, and refused where that is
/// too much, without the walk.
#[test]
fn a_change_that_goes_on_past_what_the_document_has_is_charged_where_the_rest_goes() {
    const CLIENTS: u64 = 1000;
    const TYPED: u64 = 20_000;
    let first = 100_000;
    let clients = || (0..CLIENTS).map(|at| first - at);
    // A paragraph of each client after the one before, and paragraphs of another after
    // them, each after the one before: each stands below those before it.
    let mut before = None;
    let runs = clients()
        .map(|client| {
            let change = match before {
                None => Change::under(Parent::body(), Content::element("paragraph")),
                Some(left) => Change::after(left, Content::element("paragraph")),
            };
            before = Some((client, 0));
            support::wire::Run {
                client,
                clock: 0,
                changes: vec![change],
            }
        })
        .collect();
    let doc = take_update(
        &new_doc(),
        &Update {
            runs,
            deleted: Vec::new(),
        }
        .bytes(),
        &ROOMY,
    )
    .expect("a paragraph of each client")
    .doc;
    let last = before.expect("a paragraph");
    let typed = (0..TYPED)
        .map(|clock| {
            let left = clock.checked_sub(1).map_or(last, |before| (1, before));
            Change::after(left, Content::element("paragraph"))
        })
        .collect();
    let doc = take_update(&doc, &Update::of(1, 0, typed).bytes(), &ROOMY)
        .expect("paragraphs typed after them")
        .doc;
    // Each client's paragraph again and one place more, under a parent of its own.
    let runs = clients()
        .map(|client| support::wire::Run {
            client,
            clock: 0,
            changes: vec![Change::under(
                Parent::Change((client, 0)),
                Content::Deleted(2),
            )],
        })
        .collect();
    let past = Update {
        runs,
        deleted: Vec::new(),
    }
    .bytes();
    let steps = placing_steps(&doc, &past).expect("counted");
    assert!(steps >= CLIENTS * TYPED, "{steps} steps");
    let start = Instant::now();
    let refused = take_update(&doc, &past, &ROOMY);
    let took = start.elapsed();
    assert!(
        matches!(
            refused,
            Err(TakeError::Update(UpdateError::PlacedPastTooMany { .. }))
        ),
        "{refused:?}"
    );
    assert!(took < Duration::from_secs(5), "refused in {took:?}");
}

/// The client and the first clock of the first run of a state.
fn first_run(state: &[u8]) -> (u64, u64) {
    // One run: the count of runs, the count of its changes, its client and its clock.
    let mut at = 1;
    let mut number = || {
        let mut value = 0_u64;
        let mut shift = 0;
        loop {
            let byte = state[at];
            at += 1;
            value |= u64::from(byte & 0x7f) << shift;
            shift += 7;
            if byte & 0x80 == 0 {
                return value;
            }
        }
    };
    let _changes = number();
    (number(), number())
}

/// A change that alone follows a change of the same update is placed in a step: what
/// is pasted is one change placed in the document and the rest after it.
#[test]
fn what_follows_a_change_of_its_own_update_alone_is_a_step() {
    const PASTED: usize = 2000;
    let doc = widest();
    let pasted = edited(&doc, |root, txn| {
        for _ in 0..PASTED {
            let entry = root.push_back(txn, XmlElementPrelim::empty("paragraph"));
            entry.push_back(txn, XmlTextPrelim::new("pasted"));
        }
    });
    let steps = placing_steps(&doc, &pasted).expect("counted");
    // The first paragraph goes after the last child, which nothing stands below; each
    // other is a step, and each text is alone in its paragraph.
    assert!(steps < count(8 * PASTED), "{steps} steps");
    // Past the nodes of a body, which is another refusal than this one.
    assert!(matches!(
        take_update(&doc, &pasted, &ROOMY),
        Err(TakeError::Body(fvoci_model::BodyError::TooManyNodes))
    ));
}

/// Changes that all name one change of their own update to their left are placed
/// past each other, and are charged it whatever the document holds.
#[test]
fn changes_that_follow_one_change_of_their_update_together_are_charged_each_other() {
    const FOLLOWERS: usize = 3000;
    let doc = opened(&[paragraph("a small document")]);
    let mut changes = vec![Change::under(Parent::body(), Content::Deleted(1))];
    changes.extend((0..FOLLOWERS).map(|_| Change::after((ABOVE, 0), Content::Deleted(1))));
    let update = Update::of(ABOVE, 0, changes).bytes();
    let steps = placing_steps(&doc, &update).expect("counted");
    assert!(steps >= charged(FOLLOWERS, 0), "{steps} steps");
    assert!(matches!(
        take_update(&doc, &update, &ROOMY),
        Err(TakeError::Update(UpdateError::PlacedPastTooMany { .. }))
    ));
}

/// Changes that name each other to their left, two and two, are placed nowhere by the
/// library, which waits for each to come before the other. Counting them does not
/// go round: it is done in the time it takes to read the document, and the update is
/// refused.
#[test]
fn changes_that_name_each_other_are_counted_without_going_round() {
    const PAIRS: usize = 20_000;
    let doc = widest();
    let changes = (0..2 * PAIRS)
        .map(|at| Change::after((ABOVE, count(at ^ 1)), Content::Deleted(1)))
        .collect();
    let update = Update::of(ABOVE, 0, changes).bytes();
    let start = Instant::now();
    let steps = placing_steps(&doc, &update).expect("counted");
    let took = start.elapsed();
    assert_eq!(
        steps,
        count(2 * PAIRS),
        "a step each, as what follows alone"
    );
    assert!(took < Duration::from_secs(2), "counted in {took:?}");
    assert!(take_update(&doc, &update, &ROOMY).is_err(), "not taken");
}

/// What an update says again of what the document has is left out by the library,
/// and a change that names it is placed in the document: it does not follow a change
/// of its own update, and is charged what stands below it there.
#[test]
fn a_change_after_what_the_document_has_already_is_placed_in_the_document() {
    const CHAIN: usize = 400;
    let chain = || {
        let mut changes = vec![Change::under(Parent::body(), Content::Deleted(1))];
        changes.extend(
            (1..CHAIN).map(|at| Change::after((ABOVE, count(at - 1)), Content::Deleted(1))),
        );
        changes
    };
    let first = Update::of(ABOVE, 0, chain()).bytes();
    let doc = take_update(&widest(), &first, &ROOMY).expect("a chain").doc;
    // The chain again, and after each of its changes one more.
    let mut changes = chain();
    changes.extend((0..CHAIN).map(|at| Change::after((ABOVE, count(at)), Content::Deleted(1))));
    let again = Update::of(ABOVE, 0, changes).bytes();
    let steps = placing_steps(&doc, &again).expect("counted");
    // Each walks past the rest of the chain, not a step as one that follows a change
    // of its own update.
    let below = count(CHAIN * (CHAIN + 1) / 2);
    assert!(steps >= below, "{steps} steps");
    assert!(take_update(&doc, &again, &ROOMY).is_ok(), "taken");
    // The same update is taken where it is no more than the document has.
    let whole = take_update(&doc, &first, &ROOMY).expect("said again");
    assert_eq!(state_of(&whole.doc), state_of(&doc));
}

/// The first `count` elements under a root, in order.
fn elements(root: &XmlFragmentRef, txn: &TransactionMut<'_>, count: usize) -> Vec<XmlElementRef> {
    root.children(txn)
        .filter_map(|child| match child {
            XmlOut::Element(element) => Some(element),
            _ => None,
        })
        .take(count)
        .collect()
}

/// The texts of the first `count` elements under a root.
fn texts(root: &XmlFragmentRef, txn: &TransactionMut<'_>, count: usize) -> Vec<XmlTextRef> {
    elements(root, txn, count)
        .into_iter()
        .filter_map(|line| match line.first_child() {
            Some(XmlOut::Text(text)) => Some(text),
            _ => None,
        })
        .collect()
}

/// What an editor does to many nodes at once is many changes that each go into a list
/// of their own, short however large the document is: letters typed into every
/// paragraph, a mark over every paragraph, an attribute of every paragraph.
#[test]
fn an_edit_of_every_paragraph_of_a_large_document_is_taken() {
    const PARAGRAPHS: usize = 2000;
    let mut blocks = vec![paragraph("a line of the document"); PARAGRAPHS];
    // A mark over the middle of a text makes three texts of it: room for those.
    blocks.extend(vec![
        json!({"type": "paragraph"});
        CHILDREN - 4 * PARAGRAPHS
    ]);
    let doc = opened(&blocks);
    let typed = edited(&doc, |root, txn| {
        for text in texts(root, txn, PARAGRAPHS) {
            text.insert(txn, 2, "가😀");
        }
    });
    let marked = edited(&doc, |root, txn| {
        let bold = Attrs::from([("bold".into(), Any::Map(Arc::default()))]);
        for text in texts(root, txn, PARAGRAPHS) {
            text.format(txn, 2, 6, bold.clone());
        }
    });
    let aligned = edited(&doc, |root, txn| {
        for line in elements(root, txn, PARAGRAPHS) {
            line.insert_attribute(txn, "textAlign", "center");
        }
    });
    for (name, update) in [
        ("typed", &typed),
        ("marked", &marked),
        ("aligned", &aligned),
    ] {
        let steps = placing_steps(&doc, update).expect("counted");
        println!("{name}: {steps} steps");
        assert!(
            steps <= count(2 * PARAGRAPHS * 30),
            "{name}: {steps} steps, each change in the short list of its paragraph"
        );
        let taken = take_update(&doc, update, &ROOMY);
        assert!(taken.is_ok(), "{name}: {:?}", taken.err());
    }
    let taken = take_update(&doc, &typed, &ROOMY).expect("typed");
    assert_eq!(
        json_of(taken.repair.body.as_ref())["content"][0]["content"][0]["text"],
        "a 가😀line of the document"
    );
}

/// Letters are counted as the library counts them, in UTF-16 code units: a change
/// that follows letters of its own update, of one byte and of four, is found to
/// follow them and is not counted.
#[test]
fn a_change_after_letters_of_its_own_update_is_found_there() {
    let doc = widest();
    for letters in ["plain", "가나다", "😀😀", "a가😀"] {
        let length = count(letters.encode_utf16().count());
        let update = Update::of(
            ABOVE,
            0,
            vec![
                Change::under(Parent::body(), Content::xml_text()),
                Change::under(Parent::Change((ABOVE, 0)), Content::text(letters)),
                // After the last letter, which is found only where the letters before
                // it are counted as the library counts them.
                Change::after((ABOVE, length), Content::text("x")),
            ],
        )
        .bytes();
        let steps = placing_steps(&doc, &update).expect("counted");
        assert!(
            steps < count(CHILDREN + 20),
            "{letters}: {steps} steps, the text alone is placed in the document"
        );
    }
}

/// What the document held before, and what was taken before, is read again whatever
/// it is charged: a stored state is the update of an empty document, and an update
/// that was logged is taken without the count.
#[test]
fn what_was_taken_before_is_read_again() {
    let doc = widest();
    let update = crowding(changes_past_the_limit(CHILDREN));
    let before = UpdateLimits {
        placing_steps: None,
        clients: None,
        client_changes: None,
        total_changes: None,
        reading_steps: None,
        ..ROOMY
    };
    let taken = take_update(&doc, &update, &before).expect("taken without the count");
    let state = state_of(&taken.doc);
    let opened = from_state(&state, &ROOMY).expect("a stored state is opened");
    assert_eq!(state_of(&opened.doc), state);
}

/// The lengths this walk reads are the library's, for every document of the vectors:
/// an update placed in each is counted, which reads the whole state.
#[test]
fn every_document_of_the_vectors_is_read_as_long_as_the_library_reads_it() {
    for section in ["documents", "bodies", "readings", "refused", "repairs"] {
        for vector in vectors(section) {
            let doc = doc_of(&vector.update);
            assert!(
                placing_steps(&doc, &crowding(1)).is_ok(),
                "{section}: {}",
                vector.name
            );
        }
    }
}

#[test]
fn a_body_is_still_a_body_after_changes_within_the_limit() {
    let doc = opened(&[paragraph("one"), paragraph("two")]);
    let taken = take_update(&doc, &crowding(50), &ROOMY).expect("taken");
    assert_eq!(
        to_body(&taken.doc).expect("a body"),
        to_body(&doc).expect("a body"),
        "deleted content adds nothing to the body"
    );
}

/// The client of the documents made here from a body by a known client.
const WRITER: u64 = 5;

/// A document of one paragraph of `letters` letters, made by [`WRITER`]: the paragraph
/// is its clock 0, its text 1, and the letters one change from 2 on.
fn one_text(letters: u64) -> Doc {
    let text = "a".repeat(usize::try_from(letters).expect("a length fits"));
    from_body_by(
        &body_of(&json!({"type": "doc", "content": [paragraph(&text)]})),
        WRITER,
    )
}

/// An update that deletes a place at each of `clocks` of [`WRITER`], in that order.
fn deleting(clocks: impl Iterator<Item = u64>) -> Vec<u8> {
    Update {
        runs: Vec::new(),
        deleted: vec![(WRITER, clocks.map(|clock| (clock, 1)).collect())],
    }
    .bytes()
}

fn refused_past_the_limit(doc: &Doc, update: &[u8]) -> Duration {
    let start = Instant::now();
    let refused = take_update(doc, update, &ROOMY).expect_err("past the limit");
    let took = start.elapsed();
    assert!(
        matches!(
            refused,
            TakeError::Update(UpdateError::PlacedPastTooMany { .. })
        ),
        "{refused:?}"
    );
    took
}

/// The library cuts a change in two where a deletion ends inside it, counting its
/// letters from its start to the cut and copying both halves: each cut is charged the
/// length of the change it falls in, and so many cuts of a long text are refused
/// before the library is asked. From its end back to its start, which the library takes
/// the longest for, they are refused for their order before they are counted. As many
/// in short paragraphs are taken.
#[test]
fn cuts_inside_a_long_text_are_charged_its_length() {
    const LETTERS: u64 = 400_000;
    const DELETIONS: u64 = 1400;
    let doc = one_text(LETTERS);
    let spread = |count: u64| (1..=count).map(move |at| 2 + at * (LETTERS / (count + 1)));
    let few = deleting(spread(10));
    let steps = placing_steps(&doc, &few).expect("counted");
    // Each cuts the text before and after the letter it deletes.
    assert!(steps >= 2 * 10 * LETTERS / 256, "{steps} steps");
    take_update(&doc, &few, &ROOMY).expect("a few are taken");
    let longer = placing_steps(&one_text(2 * LETTERS), &few).expect("counted");
    assert!(
        longer >= 2 * steps - 10,
        "{longer} steps in twice the text, {steps} in it"
    );

    let took = refused_past_the_limit(&doc, &deleting(spread(DELETIONS)));
    assert!(took < Duration::from_secs(1), "refused in {took:?}");
    let start = Instant::now();
    let backwards = take_update(&doc, &deleting(spread(DELETIONS).rev()), &ROOMY).map(|_| ());
    let took = start.elapsed();
    assert!(
        matches!(
            backwards,
            Err(TakeError::Update(UpdateError::DeletionsOutOfOrder { .. }))
        ),
        "{backwards:?}"
    );
    assert!(took < Duration::from_secs(1), "refused in {took:?}");

    // As many letters deleted, one in each of as many paragraphs: what a replacement of
    // a word wherever it is does.
    let short = vec![paragraph(&"a".repeat(100)); usize::try_from(DELETIONS).expect("fits")];
    let doc = from_body_by(&body_of(&json!({"type": "doc", "content": short})), WRITER);
    // Each paragraph is its element, its text and a hundred letters.
    let each = deleting((0..DELETIONS).map(|paragraph| paragraph * 102 + 50));
    let steps = placing_steps(&doc, &each).expect("counted");
    assert!(steps <= 2 * DELETIONS, "{steps} steps");
    take_update(&doc, &each, &ROOMY).expect("taken");
}

/// The library copies every value of a run of values it cuts, and holds more memory
/// for it than for letters: a cut inside one is charged more than a cut inside as many
/// letters. No editor makes a run of values; a client may send one.
#[test]
fn cuts_inside_a_run_of_values_are_charged_more_than_inside_letters() {
    const VALUES: u64 = 20_000;
    let made = Update::of(
        WRITER,
        0,
        vec![
            Change::under(Parent::body(), Content::element("heading")),
            Change::member(
                Parent::Change((WRITER, 0)),
                "level",
                Content::values(vec![
                    value::whole(1);
                    usize::try_from(VALUES).expect("fits")
                ]),
            ),
        ],
    )
    .bytes();
    let doc = take_update(&new_doc(), &made, &ROOMY).expect("made").doc;
    // The values are clocks 1 on.
    let one = deleting([1 + VALUES / 2].into_iter());
    let steps = placing_steps(&doc, &one).expect("counted");
    let as_letters =
        placing_steps(&one_text(VALUES), &deleting([2 + VALUES / 2].into_iter())).expect("counted");
    assert!(
        steps >= 256 * as_letters,
        "{steps} steps, {as_letters} for letters"
    );
    take_update(&doc, &one, &ROOMY).expect("one is taken");
    let many = PLACING_STEPS / steps + 1;
    let spread = (1..=many).map(|at| 1 + at * (VALUES / (many + 1)));
    let took = refused_past_the_limit(&doc, &deleting(spread));
    assert!(took < Duration::from_secs(1), "refused in {took:?}");
}

/// The library walks the blocks of a client from the start of each range of its
/// deletions to its end, in the order the ranges are written: ranges that go back or
/// overlap make it walk the same blocks again and again, and are refused. Ranges in
/// order are taken, also where one ends where the next begins. Naming the same client
/// more than once in the delete set is refused during wire scanning before decoding.
#[test]
fn deletions_of_a_client_that_go_back_or_overlap_are_refused() {
    const LETTERS: u64 = 1000;
    let doc = one_text(LETTERS);
    // The letters are clocks 2 on.
    let taken = |deleted: Vec<(u64, Vec<(u64, u64)>)>| {
        let update = Update {
            runs: Vec::new(),
            deleted,
        };
        take_update(&doc, &update.bytes(), &ROOMY).map(|taken| {
            let body = to_body(&taken.doc).expect("a body").expect("not empty");
            body.document()["content"][0]["content"][0]["text"]
                .as_str()
                .map_or(0, str::len)
        })
    };
    let refused = |deleted: Vec<(u64, Vec<(u64, u64)>)>| {
        let refused = taken(deleted.clone());
        assert!(
            matches!(
                refused,
                Err(TakeError::Update(UpdateError::DeletionsOutOfOrder { .. }))
            ),
            "{deleted:?}: {refused:?}"
        );
    };
    let left = taken(vec![(WRITER, vec![(10, 1), (11, 1), (20, 5)])]).expect("in order");
    assert_eq!(left, 993);
    refused(vec![(WRITER, vec![(10, 5), (12, 1)])]);
    refused(vec![(WRITER, vec![(20, 1), (10, 1)])]);
    refused(vec![(WRITER, vec![(2, LETTERS); 1000])]);

    for duplicate_client in [
        vec![(WRITER, vec![(2, LETTERS); 1000]), (WRITER, vec![(10, 1)])],
        vec![(WRITER, vec![(10, 1)]), (WRITER, vec![(20, 1), (10, 1)])],
    ] {
        let result = taken(duplicate_client.clone());
        assert!(
            matches!(result, Err(TakeError::Update(UpdateError::Unreadable(_)))),
            "duplicate delete-set client must be refused at scan: {duplicate_client:?} -> {result:?}"
        );
    }
}

/// A document of `children` empty paragraphs typed by `client` one after the other:
/// each names the one before it to its left.
fn typed_in_order(client: u64, children: u64) -> Doc {
    let changes = (0..children)
        .map(|clock| match clock.checked_sub(1) {
            None => Change::under(Parent::body(), Content::element("paragraph")),
            Some(before) => Change::after((client, before), Content::element("paragraph")),
        })
        .collect();
    take_update(&new_doc(), &Update::of(client, 0, changes).bytes(), &ROOMY)
        .expect("typed")
        .doc
}

/// A paragraph a client below the document's inserts with `left` and `right` as its
/// neighbours.
fn between(left: u64, right: u64) -> Change {
    Change::Item {
        left: Some((7, left)),
        right: Some((7, right)),
        parent: Parent::body(),
        key: None,
        content: Content::element("paragraph"),
    }
}

/// What is inserted between two changes, the second of which names the first to its
/// left, is placed by the library in a step: the walk stops at once at the right one.
/// It is charged so, not what stands below the left one: an editor that inserts at many
/// places of a long list typed in order is taken. Where the right neighbour stands
/// before the left one, the walk goes on through what stands after the left one, and
/// is charged it.
#[test]
fn a_change_between_two_typed_one_after_the_other_is_charged_a_step() {
    const TYPED: u64 = 20_000;
    const INSERTED: u64 = 500;
    let doc = typed_in_order(7, TYPED);
    let spread = |at: u64| at * (TYPED / INSERTED);
    let inserted = Update::of(
        1,
        0,
        (0..INSERTED)
            .map(|at| between(spread(at), spread(at) + 1))
            .collect(),
    )
    .bytes();
    let steps = placing_steps(&doc, &inserted).expect("counted");
    // The left neighbour, the right one, which both are named, and the change.
    assert!(steps <= 3 * INSERTED, "{steps} steps");
    take_update(&doc, &inserted, &ROOMY).expect("taken");

    let middle = TYPED / 2;
    let crossed = Update::of(1, 0, (0..INSERTED).map(|at| between(middle, at)).collect()).bytes();
    let steps = placing_steps(&doc, &crossed).expect("counted");
    assert!(steps >= INSERTED * (TYPED - middle - 1), "{steps} steps");
}

/// The changes of a client below the others of their list are walked past again every
/// time the library reads the document again: once as the update is taken and twice
/// as what it leaves is read again, and twice more where it is repaired. An update of
/// them at the limit of what an update may walk leaves a document that takes three
/// times the steps to read that a document may ([`READING_STEPS`]), and is refused once
/// it is applied; the most of them that leave a document within its steps, repaired,
/// are taken. Each in less than half of the time a worker has for its answer (10 s).
#[test]
fn an_update_of_a_client_below_the_document_is_refused_or_taken_in_time() {
    let doc = typed_in_order(7, count(CHILDREN));
    let middle = count(CHILDREN) / 2;
    let walking = |changes: u64| {
        let mut walking: Vec<Change> = (0..changes)
            .map(|right| {
                let mut change = between(middle, right);
                if let Change::Item { content, .. } = &mut change {
                    *content = Content::Deleted(1);
                }
                change
            })
            .collect();
        // What no body has, which the update is repaired of.
        walking.push(Change::after(
            (7, count(CHILDREN) - 1),
            Content::element("nothing-a-body-has"),
        ));
        Update::of(1, 0, walking).bytes()
    };
    let most_within = |within: &dyn Fn(u64) -> bool| {
        let (mut most, mut past) = (1_u64, 2 * PLACING_STEPS / count(CHILDREN) + 2);
        assert!(!within(past), "{past} changes are within the limit");
        while past - most > 1 {
            let changes = most.midpoint(past);
            if within(changes) {
                most = changes;
            } else {
                past = changes;
            }
        }
        most
    };
    // The most changes an update may walk: each walks about half the list.
    // Past reading steps with an unknown node to repair: repaired in place and marked
    // for remake without re-reading the slow state.
    let placed = most_within(&|changes| {
        placing_steps(&doc, &walking(changes)).expect("counted") <= PLACING_STEPS
    });
    assert!(
        placed >= PLACING_STEPS / count(CHILDREN),
        "{placed} changes"
    );
    let start = Instant::now();
    let over = take_update(&doc, &walking(placed), &ROOMY).expect("repaired and marked for remake");
    let took = start.elapsed();
    assert!(over.remake, "past reading steps: must remake");
    assert_eq!(over.repair.deleted, 1, "unknown node repaired in place");
    assert!(
        over.reading_steps > READING_STEPS,
        "{} steps",
        over.reading_steps
    );
    assert!(took < Duration::from_secs(5), "taken in {took:?}");
    // The most changes that leave a document settled within its steps: a third as many.
    let read = most_within(&|changes| {
        placing_steps(&doc, &walking(changes)).expect("counted") <= READING_STEPS
    });
    let most = (1..=read)
        .rev()
        .find(|changes| {
            take_update(&doc, &walking(*changes), &ROOMY).is_ok_and(|taken| !taken.remake)
        })
        .expect("a change is taken settled");
    assert!(
        most * 4 > placed && read - most < 3,
        "{most} of {read} changes"
    );
    let start = Instant::now();
    let taken = take_update(&doc, &walking(most), &ROOMY).expect("taken");
    let took = start.elapsed();
    assert!(!taken.remake, "within reading steps: settled");
    assert_eq!(taken.repair.deleted, 1);
    assert!(
        taken.reading_steps * 10 > READING_STEPS * 9,
        "{} steps",
        taken.reading_steps
    );
    assert!(took < Duration::from_secs(5), "taken in {took:?}");
}
