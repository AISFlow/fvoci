//! Updates made at random from what an update is made of (clients, changes, their
//! neighbours and parents, what they hold, what was deleted), and what the server does
//! with each, so that the search can be run again and further
//! (`tests/review_bytes_explore.rs` says how).
//!
//! A session is a document and a row of such updates, each made against the document
//! as the ones before left it, so that an update names changes that are there. Every
//! session comes from one seed.

use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::{Duration, Instant};

use fvoci_collab::testing::{repair, to_body, with_update};
use fvoci_collab::{TakeError, from_body_by, from_state, new_doc, state_of, take_update};
use yrs::{Doc, ReadTxn, Transact};

use super::random::{Random, document};
use super::wire::{Change, Content, Id, Parent, Run, Update, value};
use super::{ROOMY, body_of, to_base64, vectors};

/// What went wrong in a session.
#[derive(Debug, Clone)]
pub(crate) struct Failure {
    pub(crate) seed: u64,
    /// The update of the session it happened at, counted from 0.
    pub(crate) step: usize,
    /// The panic: where, and what it said.
    pub(crate) what: String,
    /// The document before the update, as its state in base64.
    pub(crate) before: String,
    /// The update, in base64.
    pub(crate) update: String,
}

/// The slowest update of a session.
#[derive(Debug, Clone, Default)]
pub(crate) struct Slowest {
    pub(crate) took: Duration,
    pub(crate) seed: u64,
    pub(crate) step: usize,
    pub(crate) update_bytes: usize,
    pub(crate) state_bytes: usize,
}

thread_local! {
    /// Where the last panic of this thread was, noted by the hook.
    static LAST_PANIC: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Makes panics note where they were instead of printing, on the threads that search.
pub(crate) fn note_panics() {
    std::panic::set_hook(Box::new(|info| {
        let place = info.location().map_or_else(
            || "nowhere".to_owned(),
            |location| format!("{}:{}", location.file(), location.line()),
        );
        let said = info
            .payload()
            .downcast_ref::<&str>()
            .map(|said| (*said).to_owned())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_default();
        // The file of a dependency is named from the registry on: no path of this machine.
        let place = place
            .rsplit_once("/registry/src/")
            .map_or(place.as_str(), |(_, rest)| rest)
            .to_owned();
        // The first line, and not all of a state that an assertion prints.
        let said: String = said
            .lines()
            .next()
            .unwrap_or_default()
            .chars()
            .take(160)
            .collect();
        LAST_PANIC.with(|last| *last.borrow_mut() = Some(format!("{place}: {said}")));
    }));
}

/// Everything the server does with an update, on a document that holds `base`: the
/// document [`take_update`] gives, if the update is taken.
///
/// # Panics
///
/// Panics if what a server relies on does not hold: the document an update is given
/// for is as it was, the document taken is its body, the deletions of a repair bring
/// an editor that took the update to the same body, the state of the document taken
/// is opened again to the same body, and a repair that fails deletes nothing.
pub(crate) fn take(base: &Doc, update: &[u8]) -> Option<Doc> {
    let before = state_of(base);
    let taken = take_update(base, update, &ROOMY);
    assert_eq!(
        state_of(base),
        before,
        "the document an update is given for is as it was"
    );
    let taken = match taken {
        Ok(taken) => taken,
        Err(TakeError::Body(_)) => {
            // A repair of the document with the update failed: made in place, it
            // deletes nothing.
            let with = with_update(base, update, &ROOMY).expect("the update alone is taken");
            let with_state = state_of(&with);
            assert!(to_body(&with).is_err(), "a body needs no repair");
            if repair(&with).is_err() {
                assert_eq!(
                    state_of(&with),
                    with_state,
                    "a repair that fails deletes nothing"
                );
            }
            return None;
        }
        Err(_) => return None,
    };
    let body = taken.repair.body.clone();
    assert_eq!(
        to_body(&taken.doc),
        Ok(body.clone()),
        "the document taken is its body"
    );
    if let Some(deletions) = &taken.repair.update {
        let editor = with_update(base, update, &ROOMY).expect("the update alone is taken");
        let editor = with_update(&editor, deletions, &ROOMY)
            .expect("the deletions of a repair are an update");
        assert_eq!(
            to_body(&editor),
            Ok(body.clone()),
            "the deletions of a repair give an editor the same body"
        );
    } else {
        assert_eq!(
            taken.repair.deleted, 0,
            "without deletions nothing was deleted"
        );
    }
    let opened = from_state(&state_of(&taken.doc), &ROOMY).expect("the state is opened again");
    assert_eq!(opened.repair.body, body, "the state opens to the same body");
    assert_eq!(opened.repair.deleted, 0, "the state opens without a repair");
    Some(taken.doc)
}

/// What a session knows of its document: how far each client's clock has run, and the
/// changes that made a shared type.
#[derive(Debug, Default)]
struct World {
    clocks: Vec<(u64, u64)>,
    types: Vec<Id>,
}

impl World {
    fn read(&mut self, doc: &Doc) {
        self.clocks = doc
            .transact()
            .state_vector()
            .iter()
            .map(|(client, clock)| (client.get(), u64::from(*clock)))
            .collect();
        self.clocks.sort_unstable();
    }

    fn clock_of(&self, client: u64) -> u64 {
        self.clocks
            .iter()
            .find(|(known, _)| *known == client)
            .map_or(0, |(_, clock)| *clock)
    }
}

const CLIENTS: [u64; 6] = [1, 2, 3, 5, 0, (1 << 53) - 1];
const NAMES: [&str; 14] = [
    "paragraph",
    "heading",
    "bulletList",
    "listItem",
    "taskList",
    "taskItem",
    "blockquote",
    "codeBlock",
    "hardBreak",
    "mention",
    "reference",
    "file",
    "video",
    "",
];
const KEYS: [&str; 9] = [
    "level", "checked", "start", "language", "userId", "x", "", "latex", "id",
];
const LETTERS: [&str; 9] = ["a", "word ", "한글", "😀", "👩‍👩‍👧‍👦", "𝒳", "\n", "é", ""];
const MARKS: [&str; 7] = [
    "bold",
    "italic",
    "link",
    "code",
    "strike",
    "bold--abcdefgh",
    "glow",
];
const JSON: [&str; 10] = [
    "{}",
    "null",
    "true",
    "{\"href\":\"/boards/1\"}",
    "{\"href\":\"javascript:alert(1)\"}",
    "1e400",
    "18446744073709551616",
    "\"text\"",
    "[1,[2,[3]]]",
    "{",
];

/// A number below `bound`, which is not zero.
fn below(random: &mut Random, bound: u64) -> u64 {
    random.next() % bound
}

/// An identifier that is in the document, or near one, or nowhere.
fn some_id(random: &mut Random, world: &World, own: &[Id]) -> Id {
    match random.below(20) {
        0..=11 if !world.clocks.is_empty() => {
            let (client, clock) = *random.pick(&world.clocks);
            (client, below(random, clock.max(1)))
        }
        12..=14 if !world.types.is_empty() => *random.pick(&world.types),
        15..=17 if !own.is_empty() => *random.pick(own),
        18 if !world.clocks.is_empty() => {
            // One past the end of a client's changes, or a little further.
            let (client, clock) = *random.pick(&world.clocks);
            (client, clock + below(random, 3))
        }
        _ => {
            let client = *random.pick(&CLIENTS);
            let clock = *random.pick(&[0, 1, 7, 0x7fff_ffff, 0xffff_ffff, 0x8000_0000]);
            (client, clock)
        }
    }
}

fn some_value(random: &mut Random, depth: usize) -> Vec<u8> {
    match random.below(12) {
        0 | 1 => value::text(random.pick(&LETTERS)),
        2 => value::whole(i64::try_from(random.below(10)).expect("fits") - 3),
        3 => {
            let number = i64::try_from(random.next() >> 2).expect("fits");
            value::whole(if random.chance(2) { -number } else { number })
        }
        4 => value::truth(),
        5 => value::null(),
        6 => value::buffer(&random.bytes(3)),
        7 => {
            let mut float = vec![123];
            float.extend(random.pick(&[
                f64::NAN.to_be_bytes(),
                f64::INFINITY.to_be_bytes(),
                2.5_f64.to_be_bytes(),
                (-0.0_f64).to_be_bytes(),
                9_007_199_254_740_993_f64.to_be_bytes(),
            ]));
            float
        }
        8 => vec![*random.pick(&[127, 121, 120])],
        9 | 10 if depth < 9 => {
            let items: Vec<Vec<u8>> = (0..random.below(3))
                .map(|_| some_value(random, depth + 1))
                .collect();
            if random.chance(2) {
                value::list(&items)
            } else {
                let members: Vec<(&str, Vec<u8>)> = items
                    .into_iter()
                    .map(|item| (*random.pick(&KEYS), item))
                    .collect();
                value::map(&members)
            }
        }
        _ => value::text("x"),
    }
}

fn some_json(random: &mut Random) -> Vec<u8> {
    if random.chance(12) {
        let depth = *random.pick(&[100, 127, 128, 129, 500]);
        let mut deep = "[".repeat(depth);
        deep.push_str(&"]".repeat(depth));
        return deep.into_bytes();
    }
    random.pick(&JSON).as_bytes().to_vec()
}

fn some_letters(random: &mut Random) -> Vec<u8> {
    let mut letters: String = (0..=random.below(3))
        .map(|_| *random.pick(&LETTERS))
        .collect();
    if random.chance(30) {
        letters = letters.repeat(200);
    }
    let mut bytes = letters.into_bytes();
    if random.chance(40) {
        // Bytes that are no UTF-8.
        bytes.extend([0xed, 0xa0, 0x80]);
    }
    bytes
}

fn some_content(random: &mut Random) -> Content {
    match random.below(40) {
        0..=7 => Content::element(random.pick(&NAMES)),
        8..=11 => Content::xml_text(),
        12 => Content::Type(*random.pick(&[0, 1, 2, 4, 5, 15, 7, 9, 200]), None),
        13..=22 => Content::Text(some_letters(random)),
        23..=26 => {
            let values: Vec<Vec<u8>> = (0..=random.below(3))
                .map(|_| some_value(random, 0))
                .collect();
            let mut content = Content::values(values);
            if let (true, Content::Any { count, .. }) = (random.chance(20), &mut content) {
                *count = *random.pick(&[0, 1, 5, 0xffff_ffff]);
            }
            content
        }
        27..=30 => Content::Format(random.pick(&MARKS).as_bytes().to_vec(), some_json(random)),
        31..=33 => Content::Deleted(*random.pick(&[1, 1, 2, 5, 0, 0x7fff_0000, 0x3fff_ffff])),
        34 | 35 => Content::Embed(some_json(random)),
        36 => Content::Binary(random.bytes(4)),
        37 => Content::Json {
            count: below(random, 3),
            texts: (0..random.below(4))
                .map(|_| random.pick(&JSON).as_bytes().to_vec())
                .collect(),
        },
        38 => {
            let length = random.below(6);
            Content::Raw(*random.pick(&[9, 0x0b, 0x0c, 0x0f]), random.bytes(length))
        }
        _ => Content::Text(b"x".to_vec()),
    }
}

fn some_change(random: &mut Random, world: &World, own: &[Id], made_here: &[Id]) -> Change {
    match random.below(40) {
        0 => return Change::Collected(*random.pick(&[1, 1, 3, 0, 0x7fff_0000])),
        1 if random.chance(3) => return Change::Skipped(1 + below(random, 3)),
        _ => {}
    }
    let parent = match random.below(20) {
        9..=13 if !made_here.is_empty() => Parent::Change(*random.pick(made_here)),
        14..=16 if !world.types.is_empty() => Parent::Change(*random.pick(&world.types)),
        17 => Parent::Root(
            random
                .pick(&["", "x", "default\0", "prosemirror"])
                .as_bytes()
                .to_vec(),
        ),
        18 => Parent::Change(some_id(random, world, own)),
        _ => Parent::body(),
    };
    let left = random.chance(3).then(|| some_id(random, world, own));
    let right = random.chance(6).then(|| some_id(random, world, own));
    let key = random
        .chance(6)
        .then(|| random.pick(&KEYS).as_bytes().to_vec());
    Change::Item {
        left,
        right,
        parent,
        key,
        content: some_content(random),
    }
}

/// An update against the document a world describes, with the changes in it that make
/// a shared type.
fn some_update(random: &mut Random, world: &World) -> (Update, Vec<Id>) {
    let mut update = Update::default();
    let mut made: Vec<Id> = Vec::new();
    let mut own: Vec<Id> = Vec::new();
    let mut clients: Vec<u64> = Vec::new();
    for _ in 0..random.below(4) {
        let client = match random.below(6) {
            0 | 1 if !world.clocks.is_empty() => random.pick(&world.clocks).0,
            _ => *random.pick(&CLIENTS),
        };
        // A client twice in an update is refused at once; rarely, to see that it is.
        if clients.contains(&client) && !random.chance(30) {
            continue;
        }
        clients.push(client);
        let at = world.clock_of(client);
        let mut clock = match random.below(30) {
            0 => at.saturating_sub(1 + below(random, 3)),
            1 => at + 1 + below(random, 3),
            2 => *random.pick(&[0, 0x7fff_fff0, 0xffff_ffff]),
            _ => at,
        };
        let begin = clock;
        let mut changes = Vec::new();
        for _ in 0..random.below(7) {
            let change = some_change(random, world, &own, &made);
            if matches!(
                &change,
                Change::Item {
                    content: Content::Type(..),
                    ..
                }
            ) {
                made.push((client, clock));
            }
            own.push((client, clock));
            clock = clock.saturating_add(change.length());
            changes.push(change);
        }
        update.runs.push(Run {
            client,
            clock: begin,
            changes,
        });
    }
    if random.chance(2) {
        for _ in 0..=random.below(2) {
            let (client, clock) = if world.clocks.is_empty() || random.chance(8) {
                (*random.pick(&CLIENTS), 4)
            } else {
                *random.pick(&world.clocks)
            };
            let stretches = (0..=random.below(3))
                .map(|_| {
                    let begin = below(random, clock.max(1));
                    let length = match random.below(12) {
                        0 => 0,
                        1 => clock + 2,
                        2 => 0x7fff_ffff - begin,
                        _ => 1 + below(random, 4),
                    };
                    (begin, length)
                })
                .collect();
            update.deleted.push((client, stretches));
        }
    }
    (update, made)
}

/// The bytes of an update, now and then damaged after they were written.
fn damaged(random: &mut Random, mut bytes: Vec<u8>) -> Vec<u8> {
    if !random.chance(8) {
        return bytes;
    }
    for _ in 0..=random.below(2) {
        let at = random.below(bytes.len().max(1));
        match random.below(4) {
            0 => {
                if let Some(byte) = bytes.get_mut(at) {
                    *byte = random.byte();
                }
            }
            1 => {
                if let Some(byte) = bytes.get_mut(at) {
                    *byte ^= 1 << random.below(8);
                }
            }
            2 => {
                let end = (at + random.below(4)).min(bytes.len());
                bytes.drain(at..end);
            }
            _ => {
                let length = random.below(4);
                let more = random.bytes(length);
                bytes.splice(at..at, more);
            }
        }
    }
    bytes
}

/// The document a session begins with: an empty one, a body made here, or a vector.
fn some_document(random: &mut Random) -> Doc {
    match random.below(4) {
        0 => new_doc(),
        1 | 2 => from_body_by(&body_of(&document(random)), 7),
        _ => {
            let section = *random.pick(&["documents", "bodies", "readings", "repairs"]);
            let all = vectors(section);
            let vector = random.pick(&all);
            with_update(&new_doc(), &vector.update, &ROOMY).expect("a vector is taken")
        }
    }
}

/// Runs the session of a seed: `steps` updates at most, to the first failure. Gives the
/// failure, if any, and the update that took longest.
#[expect(
    clippy::print_stdout,
    reason = "FVOCI_EXPLORE_SHOW prints each update, for the one who runs the search"
)]
pub(crate) fn session(seed: u64, steps: usize) -> (Option<Failure>, Slowest) {
    let mut random = Random::new(seed);
    let mut doc = some_document(&mut random);
    let mut world = World::default();
    let mut slowest = Slowest::default();
    for step in 0..steps {
        world.read(&doc);
        let (update, made) = some_update(&mut random, &world);
        let bytes = damaged(&mut random, update.bytes());
        let before = state_of(&doc);
        if std::env::var_os("FVOCI_EXPLORE_SHOW").is_some() {
            println!("seed {seed} update {step} next: {}", to_base64(&bytes));
        }
        let started = Instant::now();
        let taken = catch_unwind(AssertUnwindSafe(|| take(&doc, &bytes)));
        let took = started.elapsed();
        if took > slowest.took {
            slowest = Slowest {
                took,
                seed,
                step,
                update_bytes: bytes.len(),
                state_bytes: before.len(),
            };
        }
        if std::env::var_os("FVOCI_EXPLORE_SHOW").is_some() {
            println!(
                "seed {seed} update {step}: {} on {}: {}",
                to_base64(&bytes),
                to_base64(&before),
                match &taken {
                    Ok(Some(_)) => "taken",
                    Ok(None) => "refused",
                    Err(_) => "panicked",
                }
            );
        }
        match taken {
            Ok(Some(with)) => {
                doc = with;
                world.types.extend(made);
            }
            Ok(None) => {}
            Err(_) => {
                let what = LAST_PANIC
                    .with(|last| last.borrow_mut().take())
                    .unwrap_or_else(|| "a panic the hook did not note".to_owned());
                let failure = Failure {
                    seed,
                    step,
                    what,
                    before: to_base64(&before),
                    update: to_base64(&bytes),
                };
                return (Some(failure), slowest);
            }
        }
    }
    (None, slowest)
}
