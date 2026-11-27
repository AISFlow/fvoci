//! The reading of a state as it is counted before the library reads it: the lists it
//! leaves are the library's, whatever the changes name, and a document whose reading
//! takes more steps than a document may is not made by updates, nor read from what
//! is stored.

mod support;

use std::collections::HashMap;

use fvoci_collab::testing::{looked, placing_steps, reading};
use fvoci_collab::{
    PLACING_STEPS, READING_STEPS, TakeError, UpdateError, UpdateLimits, from_body, from_state,
    new_doc, reading_steps, state_of, steps_ahead, take_update,
};
use fvoci_model::BODY_MAX_NODES;
use serde_json::json;
use support::honest::SERVED;
use support::random::Random;
use support::wire::{Change, Content, Id, Parent, Run, Update};
use support::{body_of, edited};
use yrs::branch::{Branch, BranchID, BranchPtr};
use yrs::updates::decoder::Decode;
use yrs::{
    Any, Array, ArrayRef, Doc, Map, MapRef, Out, ReadTxn, Transact, XmlElementPrelim, XmlFragment,
    XmlTextPrelim,
};

/// What a place of a list holds, as both readings can name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Label {
    /// A shared type, by the change that made it.
    Type(Id),
    Letter(char),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Element,
    Text,
    /// Letters, from this one on.
    Letters(u32),
    Deleted,
    Collected,
}

/// A change of a state that is made up.
#[derive(Debug, Clone)]
struct Planned {
    id: Id,
    length: u64,
    kind: Kind,
    /// The parent and the member it is a value of.
    member: Option<(Id, &'static str)>,
}

impl Planned {
    fn is_type(&self) -> bool {
        matches!(self.kind, Kind::Element | Kind::Text)
    }

    fn content(&self) -> Content {
        match self.kind {
            Kind::Element => Content::element("p"),
            Kind::Text => Content::xml_text(),
            Kind::Letters(first) => Content::text(&letters(first, self.length)),
            Kind::Deleted | Kind::Collected => Content::Deleted(self.length),
        }
    }
}

fn letters(first: u32, length: u64) -> String {
    (0..u32::try_from(length).expect("a few letters"))
        .map(|at| char::from_u32(first + at).expect("a letter of the basic plane"))
        .collect()
}

const CLIENTS: [u64; 7] = [1, 2, 7, 300, 1 << 20, (1 << 40) + 5, (1 << 52) + 1];
const MEMBERS: [&str; 2] = ["a", "b"];

/// A state of a few clients whose changes name each other as no editor's do: mostly
/// changes planned before them, now and then any.
fn state(random: &mut Random) -> (Vec<Planned>, Vec<u8>) {
    let clients: Vec<u64> = {
        let mut pool = CLIENTS.to_vec();
        (0..2 + random.below(4))
            .map(|_| pool.swap_remove(random.below(pool.len())))
            .collect()
    };
    let mut clocks: HashMap<u64, u64> = HashMap::new();
    let mut letter = 0x4e00_u32;
    let mut planned: Vec<Planned> = Vec::new();
    for _ in 0..10 + random.below(80) {
        let client = *random.pick(&clients);
        let (kind, length) = match random.below(20) {
            0..=5 => (Kind::Element, 1),
            6..=8 => (Kind::Text, 1),
            9..=16 => {
                let length = 1 + random.below(4);
                let first = letter;
                letter += u32::try_from(length).expect("a few");
                (Kind::Letters(first), length)
            }
            17 | 18 => (Kind::Deleted, 1 + random.below(3)),
            _ => (Kind::Collected, 1 + random.below(2)),
        };
        let length = u64::try_from(length).expect("a few");
        let clock = clocks.entry(client).or_insert(0);
        planned.push(Planned {
            id: (client, *clock),
            length,
            kind,
            member: None,
        });
        *clock += length;
    }
    let types: Vec<Id> = planned
        .iter()
        .filter(|change| change.is_type())
        .map(|change| change.id)
        .collect();
    for change in &mut planned {
        if matches!(change.kind, Kind::Letters(_)) && !types.is_empty() && random.chance(5) {
            change.member = Some((*random.pick(&types), *random.pick(&MEMBERS)));
        }
    }
    let mut runs: HashMap<u64, Vec<Change>> = HashMap::new();
    let mut first_writes: HashMap<(Id, &str), u32> = HashMap::new();
    for (at, change) in planned.iter().enumerate() {
        let made = written(random, &planned, at, &mut first_writes);
        runs.entry(change.id.0).or_default().push(made);
    }
    let update = Update {
        runs: clients
            .iter()
            .filter_map(|client| {
                Some(Run {
                    client: *client,
                    clock: 0,
                    changes: runs.remove(client)?,
                })
            })
            .collect(),
        deleted: Vec::new(),
    };
    (planned, update.bytes())
}

/// A place of one of `among`, chosen from those planned before `at` mostly.
fn place(random: &mut Random, planned: &[Planned], among: &[usize], at: usize) -> Option<Id> {
    let before: Vec<usize> = among.iter().copied().filter(|other| *other < at).collect();
    let pool = if before.is_empty() || random.chance(60) {
        among
    } else {
        &before
    };
    if pool.is_empty() {
        return None;
    }
    let named = &planned[*random.pick(pool)];
    let offset =
        u64::try_from(random.below(usize::try_from(named.length).expect("a few"))).expect("a few");
    Some((named.id.0, named.id.1 + offset))
}

/// The change `at` as it is written: with the neighbours or the parent it names.
fn written(
    random: &mut Random,
    planned: &[Planned],
    at: usize,
    first_writes: &mut HashMap<(Id, &'static str), u32>,
) -> Change {
    let change = &planned[at];
    if change.kind == Kind::Collected {
        return Change::Collected(change.length);
    }
    if let Some((parent, key)) = change.member {
        // A value names an earlier value of its member, or the member.
        let values: Vec<usize> = (0..at)
            .filter(|other| planned[*other].member == change.member)
            .collect();
        let writes = first_writes.entry((parent, key)).or_insert(0);
        if values.is_empty() || (*writes < 8 && random.chance(3)) {
            *writes += 1;
            return Change::member(Parent::Change(parent), key, change.content());
        }
        let before = &planned[*random.pick(&values)];
        return Change::after(
            (before.id.0, before.id.1 + before.length - 1),
            change.content(),
        );
    }
    let others: Vec<usize> = (0..planned.len())
        .filter(|other| *other != at && planned[*other].member.is_none())
        .collect();
    let types: Vec<usize> = others
        .iter()
        .copied()
        .filter(|other| planned[*other].is_type())
        .collect();
    let left = (!random.chance(5))
        .then(|| place(random, planned, &others, at))
        .flatten();
    let right = random
        .chance(3)
        .then(|| place(random, planned, &others, at))
        .flatten();
    if random.chance(4) || (left.is_none() && right.is_none()) {
        let parent = match place(random, planned, &types, at) {
            Some(parent) if random.chance(2) => Parent::Change(parent),
            _ => Parent::body(),
        };
        return Change::under(parent, change.content());
    }
    Change::Item {
        left,
        right,
        parent: Parent::body(),
        key: None,
        content: change.content(),
    }
}

/// The lists of a document as the library holds them, for every parent that can be
/// reached from the root: its children, and the value of each member.
#[derive(Debug, Default, PartialEq, Eq)]
struct Lists {
    children: HashMap<Option<Id>, Vec<Label>>,
    values: HashMap<(Id, &'static str), Label>,
}

fn id_of(branch: &Branch) -> Id {
    match branch.id() {
        BranchID::Nested(id) => (id.client.get(), u64::from(id.clock)),
        BranchID::Root(name) => panic!("the root {name} is no child"),
    }
}

fn label_of(out: &Out) -> (Label, Option<BranchPtr>) {
    match out {
        Out::Any(Any::String(letter)) => {
            // A child is one letter; the value of a member is the letters of the
            // change that is its last, of which the last one is told.
            let letter = letter.chars().last().expect("a letter");
            (Label::Letter(letter), None)
        }
        Out::YXmlElement(element) => {
            let branch: &Branch = element.as_ref();
            (Label::Type(id_of(branch)), Some(BranchPtr::from(branch)))
        }
        Out::YXmlText(text) => {
            let branch: &Branch = text.as_ref();
            (Label::Type(id_of(branch)), Some(BranchPtr::from(branch)))
        }
        other => panic!("no state here holds {other:?}"),
    }
}

fn of_library(doc: &Doc) -> Lists {
    let root = doc.get_or_insert_xml_fragment("default");
    let txn = doc.transact();
    let mut lists = Lists::default();
    let root: &Branch = root.as_ref();
    let mut open: Vec<(Option<Id>, BranchPtr)> = vec![(None, BranchPtr::from(root))];
    while let Some((parent, branch)) = open.pop() {
        let mut children = Vec::new();
        for out in ArrayRef::from(branch).iter(&txn) {
            let (label, nested) = label_of(&out);
            children.push(label);
            if let (Label::Type(id), Some(nested)) = (label, nested) {
                open.push((Some(id), nested));
            }
        }
        lists.children.insert(parent, children);
        let Some(parent) = parent else { continue };
        for key in MEMBERS {
            if let Some(value) = MapRef::from(branch).get(&txn, key) {
                lists.values.insert((parent, key), label_of(&value).0);
            }
        }
    }
    lists
}

/// The lists the reading here leaves, for the parents the library reaches.
fn of_reading(planned: &[Planned], state: &[u8], reached: &Lists) -> Lists {
    let (_, read) = reading(state).expect("a whole state");
    let labels = |pieces: &[(u64, u64, u64)]| -> Vec<Label> {
        let mut labels = Vec::new();
        for (client, begin, end) in pieces {
            let change = planned
                .iter()
                .find(|change| {
                    change.id.0 == *client
                        && (change.id.1..change.id.1 + change.length).contains(begin)
                })
                .expect("a change of the state");
            match change.kind {
                Kind::Element | Kind::Text => labels.push(Label::Type(change.id)),
                Kind::Letters(first) => {
                    let whole: Vec<char> = letters(first, change.length).chars().collect();
                    let from = usize::try_from(begin - change.id.1).expect("a few");
                    let to = usize::try_from(end - change.id.1).expect("a few");
                    labels.extend(whole[from..to].iter().copied().map(Label::Letter));
                }
                Kind::Deleted => {}
                Kind::Collected => panic!("what was collected is in no list"),
            }
        }
        labels
    };
    let mut lists = Lists::default();
    for parent in reached.children.keys() {
        lists.children.insert(*parent, Vec::new());
    }
    for list in &read {
        if !reached.children.contains_key(&list.parent) {
            continue;
        }
        match (&list.member, list.parent) {
            (None, parent) => {
                lists.children.insert(parent, labels(&list.pieces));
            }
            (Some(member), Some(parent)) => {
                let key = MEMBERS
                    .into_iter()
                    .find(|key| key.as_bytes() == member.as_slice())
                    .expect("a member of the state");
                let last = labels(&list.pieces).last().copied().expect("a value");
                lists.values.insert((parent, key), last);
            }
            (Some(_), None) => panic!("the root has no members"),
        }
    }
    lists
}

#[test]
fn the_lists_of_a_state_are_read_here_as_the_library_reads_them() {
    let (mut whole, mut waiting) = (0, 0);
    let mut most_looked = 0;
    for seed in 0..40000 {
        let mut random = Random::new(seed);
        let (planned, state) = state(&mut random);
        let doc = new_doc();
        let update = yrs::Update::decode_v1(&state).expect("an update");
        let missing = {
            let mut txn = doc.transact_mut();
            txn.apply_update(update).expect("applied");
            txn.has_missing_updates()
        };
        if missing {
            waiting += 1;
            assert_eq!(
                reading(&state).err(),
                Some("it leaves a gap in a client's changes"),
                "seed {seed}: the library left changes waiting"
            );
            continue;
        }
        whole += 1;
        // The stack is looked through no more often than each change once for each
        // client but one, which is what the steps ahead count for it.
        let (stack, (changes, clients)) = looked(&state).expect("a whole state");
        assert!(
            stack <= changes * clients.saturating_sub(1),
            "seed {seed}: {stack} looked through, {changes} changes of {clients} clients"
        );
        most_looked = most_looked.max(stack);
        let steps = reading(&state).expect("a whole state").0;
        let ahead = steps_ahead(&state).expect("a whole state");
        assert!(steps <= ahead, "seed {seed}: {steps} steps, {ahead} ahead");
        let library = of_library(&doc);
        let read = of_reading(&planned, &state, &library);
        for (parent, children) in &library.children {
            assert_eq!(
                read.children.get(parent),
                Some(children),
                "seed {seed}: the children of {parent:?}, of {planned:?}"
            );
        }
        assert_eq!(
            of_reading(&planned, &state, &library),
            library,
            "seed {seed}"
        );
    }
    assert!(
        whole > 1000 && waiting > 100,
        "{whole} whole, {waiting} waiting"
    );
    assert!(most_looked > 50, "{most_looked} looked through at the most");
}

/// The limits of the server but for the steps a reading may take: what it took before
/// they were counted.
const UNCOUNTED: UpdateLimits = UpdateLimits {
    reading_steps: None,
    ..SERVED
};

/// The limits a worker reads what was stored with.
const STORED: UpdateLimits = UpdateLimits {
    placing_steps: None,
    clients: None,
    client_changes: None,
    ..SERVED
};

/// The client that typed the list.
const TYPIST: u64 = 7;

/// A document of `children` empty paragraphs typed one after the other by one client.
fn typed_in_order(children: u64) -> Doc {
    let changes = (0..children)
        .map(|clock| match clock.checked_sub(1) {
            None => Change::under(Parent::body(), Content::element("paragraph")),
            Some(before) => Change::after((TYPIST, before), Content::element("paragraph")),
        })
        .collect();
    let roomy = UpdateLimits {
        update_bytes: usize::MAX,
        placing_steps: None,
        ..SERVED
    };
    take_update(&new_doc(), &Update::of(TYPIST, 0, changes).bytes(), &roomy)
        .expect("typed")
        .doc
}

/// An update of `changes` changes of `client` from `clock` on, each deleted content of
/// one place that names the last of `children` typed children to its right and nothing
/// to its left: the library walks the whole list to place each, a client above the
/// typist's, every time it reads the document.
fn before_the_last(client: u64, clock: u64, changes: u64, children: u64) -> Vec<u8> {
    let each = || Change::Item {
        left: None,
        right: Some((TYPIST, children - 1)),
        parent: Parent::body(),
        key: None,
        content: Content::Deleted(1),
    };
    Update::of(client, clock, (0..changes).map(|_| each()).collect()).bytes()
}

/// The most changes of [`before_the_last`] from `clock` on an update may hold for
/// `doc` ([`PLACING_STEPS`]).
fn most_placed(doc: &Doc, clock: u64, children: u64) -> u64 {
    let within = |changes: u64| {
        let update = before_the_last(ABOVE, clock, changes, children);
        placing_steps(doc, &update).expect("counted") <= PLACING_STEPS
    };
    let (mut most, mut past) = (1_u64, 2 * PLACING_STEPS / children + 2);
    assert!(within(most) && !within(past), "{most} to {past} changes");
    while past - most > 1 {
        let changes = most.midpoint(past);
        if within(changes) {
            most = changes;
        } else {
            past = changes;
        }
    }
    most
}

const ABOVE: u64 = 1 << 40;

/// As many paragraphs as a body may have, typed in order.
const CHILDREN: u64 = 49_998;

fn steps_of(doc: &Doc) -> u64 {
    reading(&state_of(doc)).expect("a whole state").0
}

/// Twelve updates, each of as many changes as an update may hold that walk a list of
/// 49,998 paragraphs every time the document is read, were taken one after the other:
/// each added three million steps (0.8 s) to every update after it and to every
/// opening, and after the twelfth no worker opened the document in its time. Now an
/// update that leaves a valid body past the steps is returned unsettled with `remake`
/// set so the holder writes its body and makes it again from that (0 steps), while
/// one that also needs a repair is still refused; fewer changes are taken settled
/// until the document is at its steps, and then marked for remake: the stored
/// document stays within its steps and is opened.
#[test]
fn updates_at_the_limit_one_after_the_other_leave_a_document_read_in_its_steps() {
    let typed = typed_in_order(CHILDREN);
    let most = most_placed(&typed, 0, CHILDREN);
    assert!(most >= 60, "{most} changes in an update");
    let mut doc = typed;
    let mut clock = 0;
    for round in 0..12 {
        let update = before_the_last(ABOVE, clock, most, CHILDREN);
        assert!(update.len() < 512, "{} bytes", update.len());
        let taken = take_update(&doc, &update, &SERVED).expect("taken for remake");
        assert!(taken.remake, "round {round}: past the steps, must remake");
        assert!(
            taken.reading_steps > READING_STEPS
                && taken.reading_steps <= READING_STEPS + CHILDREN + most,
            "round {round}: {} steps counted",
            taken.reading_steps
        );
        assert_eq!(taken.repair.deleted, 0);
        assert_eq!(taken.repair.mended, 0);
        assert!(taken.repair.update.is_none());
        let remade = from_body(taken.repair.body.as_ref().expect("a body"));
        assert_eq!(steps_of(&remade), 0, "round {round}: remade takes no steps");
        let steps = steps_of(&doc);
        assert!(steps <= READING_STEPS, "round {round}: {steps} steps");
    }
    assert_eq!(
        clock, 0,
        "none of those updates left an unsettled document held"
    );
    // Past the steps with something to repair: repaired in place and marked for remake
    // without re-reading the slow state, and remaking from its body takes 0 steps.
    let mut needs_repair: Vec<Change> = (0..most)
        .map(|_| Change::Item {
            left: None,
            right: Some((TYPIST, CHILDREN - 1)),
            parent: Parent::body(),
            key: None,
            content: Content::Deleted(1),
        })
        .collect();
    needs_repair.push(Change::after(
        (TYPIST, CHILDREN - 1),
        Content::element("nothing-a-body-has"),
    ));
    let repaired_past = take_update(&doc, &Update::of(ABOVE, 0, needs_repair).bytes(), &SERVED)
        .expect("past the steps with a repair is taken and marked for remake");
    assert!(repaired_past.remake, "must remake after in-place repair");
    assert_eq!(repaired_past.repair.deleted, 1, "unknown node deleted");
    assert!(
        repaired_past.repair.update.is_some(),
        "repair update emitted"
    );
    assert!(
        repaired_past.reading_steps > READING_STEPS
            && repaired_past.reading_steps <= READING_STEPS + CHILDREN + most + 1,
        "{} steps counted",
        repaired_past.reading_steps
    );
    let remade = from_body(repaired_past.repair.body.as_ref().expect("a body"));
    assert_eq!(steps_of(&remade), 0, "remade after repair takes 0 steps");
    // Fewer changes in an update, until the document is at its steps.
    let few = 4;
    let mut taken_updates = 0;
    for round in 0..12 {
        let update = before_the_last(ABOVE, clock, few, CHILDREN);
        let taken = take_update(&doc, &update, &SERVED).expect("taken");
        if taken.remake {
            assert!(
                taken.reading_steps > READING_STEPS,
                "round {round}: {} steps",
                taken.reading_steps
            );
        } else {
            assert_eq!(taken.reading_steps, steps_of(&taken.doc), "round {round}");
            doc = taken.doc;
            clock += few;
            taken_updates += 1;
        }
        let steps = steps_of(&doc);
        assert!(steps <= READING_STEPS, "round {round}: {steps} steps");
    }
    assert_eq!(
        taken_updates,
        READING_STEPS / (few * CHILDREN),
        "{clock} changes"
    );
    let stored = state_of(&doc);
    let opened = from_state(&stored, &STORED).expect("opened");
    assert!(!opened.remake);
    assert_eq!(opened.reading_steps, steps_of(&doc));
    // What an editor writes is still taken.
    let typed = edited(&doc, |root, txn| {
        root.push_back(txn, XmlElementPrelim::empty("paragraph"));
    });
    let taken = take_update(&doc, &typed, &SERVED).expect("a paragraph typed");
    assert!(!taken.remake);
}

/// What was taken before the steps of a reading were counted is stored: a document
/// that updates at the limit left would take longer to read than a document may, and
/// is refused before the library reads it, counted no further than past the limit. One
/// that fewer changes left, under the limit, is opened with its steps told, so that
/// what holds it makes it again from its body.
#[test]
fn a_stored_document_past_the_steps_it_may_take_is_not_read() {
    let mut doc = typed_in_order(CHILDREN);
    let mut stored = Vec::new();
    let mut changes = 0;
    for _ in 0..3 {
        let most = most_placed(&doc, changes, CHILDREN);
        let update = before_the_last(ABOVE, changes, most, CHILDREN);
        doc = take_update(&doc, &update, &UNCOUNTED)
            .expect("taken before the steps were counted")
            .doc;
        changes += most;
        stored.push((changes, state_of(&doc)));
    }
    for (changes, state) in &stored {
        let refused = from_state(state, &STORED).expect_err("past the steps");
        let TakeError::Update(UpdateError::TooManyReadingSteps { steps, limit }) = refused else {
            panic!("{changes} changes: {refused}");
        };
        assert_eq!(limit, READING_STEPS);
        assert!(steps <= limit + CHILDREN + changes, "{steps} steps counted");
        // The whole of it, counted to the end.
        let steps = reading_steps(state, u64::MAX).expect("a state");
        assert!(walks_the_list(steps, *changes), "{steps} steps");
    }
    // At nine tenths of the limit: past where a document is made again, and opened.
    let changes = READING_STEPS * 9 / 10 / CHILDREN;
    let under = take_update(
        &typed_in_order(CHILDREN),
        &before_the_last(ABOVE, 0, changes, CHILDREN),
        &UNCOUNTED,
    )
    .expect("taken before the steps were counted");
    let opened = from_state(&state_of(&under.doc), &STORED).expect("opened");
    assert!(walks_the_list(opened.reading_steps, changes));
    assert!(
        opened.reading_steps * 4 > READING_STEPS * 3 && opened.reading_steps <= READING_STEPS,
        "{} steps",
        opened.reading_steps
    );
}

/// Whether `steps` are those of `changes` changes that each walk the list of
/// [`CHILDREN`], and no more than each other besides.
fn walks_the_list(steps: u64, changes: u64) -> bool {
    (changes * (CHILDREN - 1)..=changes * (CHILDREN + changes) + CHILDREN).contains(&steps)
}

/// The widest document a body makes, of as many paragraphs as a body may have, is read
/// in no steps: each child names the next to its right and stands before it. It is
/// opened from its state, and what an editor writes into it is taken, in a few steps.
#[test]
fn the_widest_document_of_a_body_is_read_in_no_steps() {
    let body = body_of(&json!({
        "type": "doc",
        "content": vec![json!({"type": "paragraph"}); BODY_MAX_NODES - 2],
    }));
    let doc = from_body(&body);
    let state = state_of(&doc);
    assert_eq!(reading(&state).expect("a whole state").0, 0);
    let opened = from_state(&state, &SERVED).expect("opened");
    assert_eq!(opened.reading_steps, 0);
    let typed = edited(&doc, |root, txn| {
        root.insert(txn, 25_000, XmlElementPrelim::empty("paragraph"));
    });
    let taken = take_update(&doc, &typed, &SERVED).expect("a paragraph typed");
    assert!(
        taken.reading_steps * 64 < READING_STEPS,
        "{} steps",
        taken.reading_steps
    );
}

/// A state that is no whole document is not counted: changes that name what the state
/// does not hold, which the library would leave waiting.
#[test]
fn a_state_that_names_what_it_does_not_hold_is_not_counted() {
    let state = Update::of(
        TYPIST,
        0,
        vec![Change::after((ABOVE, 3), Content::element("paragraph"))],
    )
    .bytes();
    assert_eq!(
        reading(&state),
        Err("it leaves a gap in a client's changes")
    );
    assert!(reading_steps(&state, 0).is_err());
}

/// A document that editors open one after the other, each writing a paragraph after
/// what the one before wrote, is read in few steps: each takes a client of its own,
/// whose changes wait for the client's before as the library reads the state, and the
/// library looks through those that wait.
#[test]
fn what_editors_write_one_session_after_the_other_is_read_in_few_steps() {
    const SESSIONS: u64 = 400;
    let mut doc = from_body(&body_of(&json!({
        "type": "doc",
        "content": [{"type": "paragraph", "content": [{"type": "text", "text": "first"}]}],
    })));
    let mut steps = 0;
    for session in 0..SESSIONS {
        let typed = edited(&doc, |root, txn| {
            let paragraph = root.push_back(txn, XmlElementPrelim::empty("paragraph"));
            paragraph.push_back(txn, XmlTextPrelim::new("typed"));
        });
        let taken = match take_update(&doc, &typed, &SERVED) {
            Ok(taken) => taken,
            Err(error) => panic!("session {session}: {error}"),
        };
        steps = taken.reading_steps;
        doc = taken.doc;
    }
    assert!(
        steps * 64 < READING_STEPS,
        "{steps} steps after {SESSIONS} sessions"
    );
    println!("{steps} steps after {SESSIONS} sessions");
}

/// The state of `sessions` sessions that each wrote `each` paragraphs after the last
/// of the session before, the first under the root, with the client of each session by
/// its number; and after them the first paragraph of one more session, of the client
/// `next`.
fn sessions_then(sessions: u64, each: u64, client_of: impl Fn(u64) -> u64, next: u64) -> Vec<u8> {
    sessions_written(sessions, each, client_of, Some(next))
}

/// As [`sessions_then`], with or without the one more session.
fn sessions_written(
    sessions: u64,
    each: u64,
    client_of: impl Fn(u64) -> u64,
    next: Option<u64>,
) -> Vec<u8> {
    let paragraph = || Content::element("paragraph");
    let last_of = |session: u64| (client_of(session), each - 1);
    let mut runs: Vec<Run> = (0..sessions)
        .map(|session| {
            let client = client_of(session);
            let first = match session.checked_sub(1) {
                None => Change::under(Parent::body(), paragraph()),
                Some(before) => Change::after(last_of(before), paragraph()),
            };
            let rest = (1..each).map(|clock| Change::after((client, clock - 1), paragraph()));
            Run {
                client,
                clock: 0,
                changes: std::iter::once(first).chain(rest).collect(),
            }
        })
        .collect();
    if let Some(next) = next {
        runs.push(Run {
            client: next,
            clock: 0,
            changes: vec![Change::after(last_of(sessions - 1), paragraph())],
        });
    }
    Update {
        runs,
        deleted: Vec::new(),
    }
    .bytes()
}

/// Where what editors write reaches the steps a document may take with no change made
/// for it: thousands of sessions that each wrote after the one before. The library
/// reads the clients from the highest down, and a change waits for the client it
/// names, which waits for the one it names: a session whose client is read before all
/// the others has its first paragraph wait for every change of them, and the library
/// looks through all that wait each time. Such an update is refused, and the document
/// made again from its body, with one client. With a client read after the others the
/// same paragraph is no steps, and so are the sessions before it, read in their order.
#[test]
fn a_session_read_before_the_thousands_it_wrote_after_takes_a_document_past_its_steps() {
    for (sessions, each) in [(4_000, 20), (2_500, 100)] {
        // Each session's client is below the one before: read in the order they wrote.
        let client_of = |session: u64| 1_000_000 - session;
        let read_last = sessions_then(sessions, each, client_of, 1);
        let steps = reading_steps(&read_last, u64::MAX).expect("a state");
        assert_eq!(
            steps, 0,
            "{sessions} sessions of {each}, read as they wrote"
        );
        let read_first = sessions_then(sessions, each, client_of, 2_000_000);
        let steps = reading_steps(&read_first, u64::MAX).expect("a state");
        assert!(
            steps > READING_STEPS,
            "{sessions} sessions of {each}: {steps} steps"
        );
    }
    // A few hundred sessions are far from it, whichever is read first.
    let steps = reading_steps(
        &sessions_then(400, 50, |session| 1_000_000 - session, 2_000_000),
        u64::MAX,
    )
    .expect("a state");
    assert!(
        steps * 16 < READING_STEPS,
        "400 sessions of 50: {steps} steps"
    );
}

/// What one more session may leave a document with is counted before it writes
/// ([`steps_ahead`]): every change of the document once for each of its clients, as
/// each may wait behind each. A document of sessions that wrote one after the other
/// and are read in that order takes no steps, and is that many ahead; the first
/// paragraph of a session read before the others leaves it with no more than that,
/// and past the steps a document may take where the update was refused. What holds a
/// document makes it again from its body at an eighth of those steps ahead as it is
/// closed: 410 sessions of 100 changes, 916 of 20, 1,449 of 8. One session short of
/// that, the session read first leaves it far from the limit.
#[test]
fn what_one_more_session_may_leave_a_document_with_is_counted_before_it_writes() {
    let client_of = |session: u64| 1_000_000 - session;
    let steps = |state: &[u8]| reading_steps(state, u64::MAX).expect("a state");
    let ahead = |state: &[u8]| steps_ahead(state).expect("a state");
    for (sessions, each) in [(4_000, 20), (8_000, 8), (2_500, 100)] {
        let before = sessions_written(sessions, each, client_of, None);
        assert_eq!(steps(&before), 0, "{sessions} sessions of {each}");
        let after = steps(&sessions_then(sessions, each, client_of, 2_000_000));
        let ahead = ahead(&before);
        assert!(
            after > READING_STEPS && after <= ahead,
            "{sessions} sessions of {each}: {after} steps with one read first, {ahead} ahead"
        );
    }
    for (sessions, each) in [(410, 100), (916, 20), (1_449, 8)] {
        let at_rest = READING_STEPS / 8;
        let made_again = sessions_written(sessions, each, client_of, None);
        assert!(ahead(&made_again) >= at_rest, "{sessions} of {each}");
        let before = sessions_written(sessions - 1, each, client_of, None);
        let ahead = ahead(&before);
        let after = steps(&sessions_then(sessions - 1, each, client_of, 2_000_000));
        assert!(
            after <= ahead && ahead < at_rest,
            "{} sessions of {each}: {after} steps with one read first, {ahead} ahead",
            sessions - 1
        );
    }
}

/// A document of 2,047 sessions under the mark at rest (`worn`: 2,048 clients and an
/// eighth of `READING_STEPS` ahead): clients descend from `2_047` down to `1`, each
/// writing a paragraph of 100 alternating letters (`ab` repeated) and a trailing
/// paragraph at clock `102` that the next session writes after. At rest, clients are
/// read highest-first with 0 reading steps; once a higher client writes after `(1, 102)`,
/// every session waits behind the one before it until clock `102` is reached.
fn document_of_2047_sessions() -> Doc {
    const SESSIONS: u64 = 2_047;
    let hundred = "ab".repeat(50);
    let mut runs = Vec::with_capacity(usize::try_from(SESSIONS).expect("fits"));
    for session in 0..SESSIONS {
        let client = SESSIONS - session;
        let paragraph = if session == 0 {
            Change::under(Parent::body(), Content::element("paragraph"))
        } else {
            Change::after((client + 1, 102), Content::element("paragraph"))
        };
        let xml_text = Change::under(Parent::Change((client, 0)), Content::xml_text());
        let letters = Change::under(Parent::Change((client, 1)), Content::text(&hundred));
        let tail = Change::after((client, 0), Content::element("paragraph"));
        runs.push(Run {
            client,
            clock: 0,
            changes: vec![paragraph, xml_text, letters, tail],
        });
    }
    let state = Update {
        runs,
        deleted: Vec::new(),
    }
    .bytes();
    assert_eq!(
        reading_steps(&state, u64::MAX).expect("state"),
        0,
        "document at rest takes 0 reading steps"
    );
    let ahead = steps_ahead(&state).expect("state");
    assert!(ahead < READING_STEPS / 8, "under mark at rest: {ahead}");
    from_state(&state, &STORED).expect("opened").doc
}

/// Whether [`delete_every_other_letter`] also appends a paragraph as a client read
/// before all 2,047 sessions.
#[derive(Clone, Copy)]
enum ReadFirst {
    Append,
    None,
}

/// Deletes every second letter (`b`, odd indices 1, 3, ..., 97) of `paragraphs`
/// hundred-letter paragraphs belonging to descending sessions starting at
/// `first_session` (`0` is client `2_047`), and optionally inserts one paragraph as
/// `READ_FIRST` (`3_000`) after `(1, 102)` so it is read first and forces all 2,047
/// sessions onto the waiting stack.
fn delete_every_other_letter(
    first_session: u64,
    paragraphs: u64,
    read_first: ReadFirst,
) -> Vec<u8> {
    const SESSIONS: u64 = 2_047;
    const READ_FIRST: u64 = 3_000;
    let runs = match read_first {
        ReadFirst::Append => vec![Run {
            client: READ_FIRST,
            clock: 0,
            changes: vec![Change::after((1, 102), Content::element("paragraph"))],
        }],
        ReadFirst::None => Vec::new(),
    };
    let deleted = (first_session..first_session + paragraphs)
        .map(|session| {
            let client = SESSIONS - session;
            let stretches = (0..49_u64).map(|odd| (3 + odd * 2, 1)).collect();
            (client, stretches)
        })
        .collect();
    Update { runs, deleted }.bytes()
}

/// Two-step deletions into a 2,047-session document under the mark at rest: a session
/// read before all others deletes every other letter in 415 paragraphs (taken settled,
/// staying under three quarters of `READING_STEPS` ahead) and then in 600 more
/// paragraphs. The second update pushes reading steps past `READING_STEPS`: it is
/// accepted with `remake: true`, its body holds all 1,015 edited paragraphs, and
/// remaking from that body leaves 0 steps. Combining the same update with an invalid
/// node (requiring a repair) is also repaired in place and marked for remake, and
/// `client_changes` / `body_bytes` limits are not bypassed.
#[test]
fn two_step_deletions_into_2047_sessions_are_taken_and_marked_for_remake() {
    let doc = document_of_2047_sessions();
    let first = delete_every_other_letter(0, 415, ReadFirst::Append);
    assert!(first.len() <= SERVED.update_bytes, "{} bytes", first.len());
    let after_first = take_update(&doc, &first, &SERVED).expect("first 415 taken");
    assert!(!after_first.remake, "first stays within reading steps");
    assert!(
        after_first.steps_ahead * 4 < READING_STEPS * 3,
        "still under three quarters ahead: {}",
        after_first.steps_ahead
    );

    let second = delete_every_other_letter(415, 600, ReadFirst::None);
    assert!(
        second.len() <= SERVED.update_bytes,
        "{} bytes",
        second.len()
    );
    let after_second = take_update(&after_first.doc, &second, &SERVED).expect("second 600 taken");
    assert!(after_second.remake, "second pushes past reading steps");
    assert!(
        after_second.reading_steps > READING_STEPS,
        "{} steps",
        after_second.reading_steps
    );
    assert!(
        after_second.steps_ahead > READING_STEPS,
        "{} ahead",
        after_second.steps_ahead
    );
    assert_eq!(after_second.repair.deleted, 0, "no repair deletions");
    assert!(after_second.repair.update.is_none(), "no repair update");

    let body = after_second.repair.body.as_ref().expect("a body");
    let expected = format!("{}b", "a".repeat(50));
    let content = body.document()["content"].as_array().expect("content");
    assert_eq!(content.len(), 2_047 * 2 + 1, "all paragraphs kept");
    for index in 0..1_015 {
        assert_eq!(
            content[index * 2]["content"][0]["text"].as_str(),
            Some(expected.as_str()),
            "paragraph {index} kept its deletions"
        );
    }

    let remade = from_body(body);
    assert_eq!(steps_of(&remade), 0, "remade from body has 0 reading steps");
    assert_eq!(
        fvoci_collab::testing::to_body(&remade).expect("body"),
        Some(body.clone()),
        "remade document projects the same body"
    );

    // Limits still hold when `reading_steps` is exceeded: body_bytes and client_changes.
    let tight_body = UpdateLimits {
        body_bytes: 1_024,
        ..SERVED
    };
    assert!(
        matches!(
            take_update(&after_first.doc, &second, &tight_body),
            Err(TakeError::BodyTooLarge { .. })
        ),
        "body_bytes limit is still enforced"
    );
    let tight_changes = UpdateLimits {
        client_changes: Some(50),
        ..SERVED
    };
    assert!(
        matches!(
            take_update(&after_first.doc, &second, &tight_changes),
            Err(TakeError::Update(UpdateError::TooManyChanges { .. }))
        ),
        "client_changes limit is still enforced"
    );

    assert_repair_past_reading_steps_taken(&after_first.doc);
}

fn assert_repair_past_reading_steps_taken(after_first: &yrs::Doc) {
    // With an invalid node needing repair, the update past reading steps is now repaired
    // in place and marked for remake without re-reading the slow state.
    let second_with_repair = Update {
        runs: vec![Run {
            client: 3_000,
            clock: 1,
            changes: vec![Change::after(
                (3_000, 0),
                Content::element("nothing-a-body-has"),
            )],
        }],
        deleted: (415..1_015_u64)
            .map(|session| {
                let client = 2_047 - session;
                let stretches = (0..49_u64).map(|odd| (3 + odd * 2, 1)).collect();
                (client, stretches)
            })
            .collect(),
    };
    let repaired_past = take_update(after_first, &second_with_repair.bytes(), &SERVED)
        .expect("repair + reading steps is taken and marked for remake");
    assert!(repaired_past.remake, "must remake");
    assert_eq!(repaired_past.repair.deleted, 1, "unknown node deleted");
    assert!(
        repaired_past.repair.update.is_some(),
        "repair update emitted"
    );
    assert!(
        repaired_past.reading_steps > READING_STEPS,
        "{} steps",
        repaired_past.reading_steps
    );
    let remade = from_body(repaired_past.repair.body.as_ref().expect("a body"));
    assert_eq!(steps_of(&remade), 0, "remade takes 0 steps");
}
