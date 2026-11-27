//! What the worker that holds a document pays for it at the limits of a body, measured
//! and printed. Nothing here runs with the other tests: each is ignored, and is run by
//! name in a release build,
//!
//! ```text
//! cargo test --release -p fvoci-collab --test cost -- --ignored --nocapture times
//! cargo test --release -p fvoci-collab --test cost -- --ignored --nocapture passes
//! cargo test --release -p fvoci-collab --test cost -- --ignored --nocapture memory
//! cargo test --release -p fvoci-collab --test cost -- --ignored --nocapture honest
//! cargo test --release -p fvoci-collab --test cost -- --ignored --nocapture remade
//! cargo test --release -p fvoci-collab --test cost -- --ignored --nocapture reading
//! cargo test --release -p fvoci-collab --test cost -- --ignored --nocapture accumulated
//! cargo test --release -p fvoci-collab --test cost -- --ignored --nocapture at_its_steps
//! cargo test --release -p fvoci-collab --test cost -- --ignored --nocapture scattered
//! ```
//!
//! The times are of one thread: the middle of three runs, with the least and the most
//! after it. The bodies are as large as a body may be ([`support::shapes`] says how to
//! measure smaller ones and only some of them).

mod support;

use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use fvoci_collab::testing::{
    CacheProbe, CacheValidation, changes_of, charged, placing_steps, to_body, with_update,
};
use fvoci_collab::{
    BodyDoc, MAX_CLIENT_CHANGES, MAX_CLIENTS, MAX_TOTAL_CHANGES, PLACING_STEPS, READING_STEPS,
    UpdateLimits, from_body, from_state, new_doc, state_of, take_update,
};
use fvoci_model::{BODY_MAX_BYTES, BODY_MAX_NODES, Body};
use serde_json::json;
use support::shapes::{
    self, RUNS, Runs, SHAPES, document, mega, millis, nodes_of, timed, typed, unknown,
};
use support::slow::{
    ABOVE, LIST, crowding, made, named, scattered_list, slow_states, state_of_runs,
};
use support::wire::{Change, Content, Parent, Update};
use support::{body_of, edited, honest};
use yrs::{Doc, ReadTxn as _, Transact as _, XmlElementPrelim, XmlFragment as _};

/// The limits the server holds an update to with its settings as they come.
const LIMITS: UpdateLimits = UpdateLimits {
    update_bytes: 64 * 1024,
    state_bytes: 16 * 1024 * 1024,
    body_bytes: BODY_MAX_BYTES,
    placing_steps: Some(PLACING_STEPS),
    clients: Some(MAX_CLIENTS),
    client_changes: Some(MAX_CLIENT_CHANGES),
    total_changes: Some(MAX_TOTAL_CHANGES),
    reading_steps: Some(READING_STEPS),
};

/// Runs `run` [`RUNS`] times, preparing fresh input with `setup` outside the timer, and
/// returns the times with what the middle run returned.
fn timed_with<S, T>(mut setup: impl FnMut() -> S, mut run: impl FnMut(S) -> T) -> (Runs, T) {
    let mut runs: Vec<(Duration, T)> = (0..RUNS)
        .map(|_| {
            let input = setup();
            let start = Instant::now();
            let value = run(input);
            (start.elapsed(), value)
        })
        .collect();
    let times: Vec<Duration> = runs.iter().map(|(took, _)| *took).collect();
    runs.sort_by_key(|(took, _)| *took);
    (Runs::of(&times), runs.swap_remove(RUNS / 2).1)
}

#[test]
#[ignore = "a measurement, run by name in a release build"]
fn times() {
    println!(
        "| shape | nodes | body | state | from_body | project | take | take+repair | from_state | lists |"
    );
    println!("| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |");
    for (name, document) in shapes::measured(&SHAPES) {
        let body = body_of(&document);
        let body_bytes = serde_json::to_vec(body.document()).expect("JSON").len();
        let (made, doc) = timed(|| from_body(&body));
        let state = state_of(&doc);
        let changes = changes_of(&state);
        let (projected, read) = timed(|| to_body(&doc).expect("a body"));
        assert!(read.is_some(), "{name}: a body");
        let update = typed(&doc);
        let (taken, with) = timed_with(
            || BodyDoc::from_state(&state, &LIMITS).expect("opened").doc,
            |held| {
                held.take_in_place(None, changes, &update, &LIMITS)
                    .expect("taken")
            },
        );
        assert_eq!(with.repair.deleted, 0, "{name}: nothing to repair");
        let update = unknown(&doc);
        let (repaired, with) = timed_with(
            || BodyDoc::from_state(&state, &LIMITS).expect("opened").doc,
            |held| {
                held.take_in_place(None, changes, &update, &LIMITS)
                    .expect("repaired")
            },
        );
        assert_eq!(with.repair.deleted, 1, "{name}: the unknown node went");
        let (opened, _) = timed(|| from_state(&state, &LIMITS).expect("opened"));
        // Reading the lists of the document, which an update of many changes pays
        // on top of being taken.
        let (lists, _) = timed(|| placing_steps(&doc, &update).expect("counted"));
        println!(
            "| {name} | {} | {:.2} MB | {:.2} MB | {made} | {projected} | {taken} | {repaired} | {opened} | {lists} |",
            nodes_of(&document),
            mega(body_bytes),
            mega(state.len()),
        );
    }
}

/// The shapes whose passes are measured: of the most nodes in paragraphs, in depth, in
/// marks and in one text, and of the most bytes.
const PASSED: [&str; 5] = ["flat", "deep", "marks", "marked line", "text"];

/// Who pays for a pass of [`take_by_passes`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Paid {
    /// [`take_update`], within the time a worker has for its answer.
    InTheTake,
    /// The worker, after the take or later.
    ByTheWorker,
    /// No pass of its own: a sum, a whole, or what is told of a pass.
    Told,
}

impl Paid {
    fn by(self) -> &'static str {
        match self {
            Self::InTheTake => "take",
            Self::ByTheWorker => "worker",
            Self::Told => "",
        }
    }
}

/// The passes of one take, in order, each with who pays for it and how long it took.
#[derive(Debug, Default)]
struct Passes(Vec<(&'static str, Paid, Duration)>);

impl Passes {
    /// Runs `run` as the pass `name`, and gives what it gave.
    fn time<T>(&mut self, name: &'static str, paid: Paid, run: impl FnOnce() -> T) -> T {
        let start = Instant::now();
        let value = black_box(run());
        self.0.push((name, paid, start.elapsed()));
        value
    }

    /// The time of the passes `take_update` pays for.
    fn in_the_take(&self) -> Duration {
        self.0
            .iter()
            .filter(|(_, paid, _)| *paid == Paid::InTheTake)
            .map(|(_, _, took)| *took)
            .sum()
    }
}

/// Takes `update` into `doc` in place as [`BodyDoc::take_in_place`] does an update that
/// needs no repair, one pass at a time, and the passes the worker makes after it
/// (`collab_worker/serve.rs`); with the state and the body of the document it leaves.
///
/// Passes 1–4 of the old copy-first take (`state_of`, `scan`, `decode`, `apply to copy`)
/// and Passes 7–10 of the per-update settle re-read (`drop`, `decode`, `apply`,
/// `state_of`, `same_state`) are gone: live updates charge against the cached prior
/// state, apply in place, encode once, and count steps/origins in place while settle
/// re-reads run only at open, fold, and close.
fn take_by_passes(
    doc: Doc,
    mut probe: CacheProbe,
    prior_state: &[u8],
    prior_changes: u64,
    update: &[u8],
) -> (Passes, Vec<u8>, Option<Body>, Vec<String>) {
    let mut passes = Passes::default();
    let take = Paid::InTheTake;
    passes.time(
        "4a. the update read and charged (update::read, placing::charge)",
        take,
        || charged(&doc, prior_state, prior_changes, update, PLACING_STEPS).expect("charged"),
    );
    // Past no limit the charge is its cheap count, which it gives where that is within
    // the limit, and reads the state's clocks, and maybe its lists, for otherwise.
    let cheap = charged(&doc, prior_state, prior_changes, update, u64::MAX).expect("charged")
        <= PLACING_STEPS;
    // Decoded already in the take, by `update::read`.
    let prepared = CacheProbe::prepare_update(&doc, update).expect("the update reads");
    let applied = passes.time("4b. the update applied in place (+ delta)", take, || {
        CacheProbe::apply_prepared(&doc, prepared).expect("the update applies")
    });
    let (state_bytes, read_steps) = passes.time("5–6. incremental reading cache", take, || {
        probe
            .count_reading(&applied, &LIMITS)
            .expect("incremental reading cache succeeds")
    });
    assert!(
        read_steps <= READING_STEPS,
        "read within its steps: {read_steps}"
    );
    passes.time("11–13. incremental block validate & size", take, || {
        let CacheValidation::Valid(bytes) = probe.validate(&doc, &applied) else {
            panic!("incremental block validation succeeds");
        };
        bytes
    });
    let files = passes.time("14. file_ids (cached)", Paid::ByTheWorker, || {
        probe.file_ids(&doc)
    });
    let state = state_of(&doc);
    assert_eq!(state_bytes, state.len(), "incremental state_bytes exact");
    let body = passes.time("Request::Body: to_body and JSON", Paid::ByTheWorker, || {
        let body = to_body(&doc).expect("a body");
        if let Some(body) = &body {
            black_box(serde_json::to_vec(body.document()).expect("JSON"));
        }
        body
    });
    passes.time("held document dropped", Paid::ByTheWorker, || {
        drop(probe);
        drop(doc);
    });
    passes.0.push((
        if cheap {
            "(the charge: the cheap count, the state's clocks and lists not read)"
        } else {
            "(the charge: past the cheap count, the state's clocks read and maybe its lists)"
        },
        Paid::Told,
        Duration::ZERO,
    ));
    (passes, state, body, files)
}

/// How long each pass of an ordinary in-place take takes, for the shapes of [`PASSED`]:
/// a paragraph typed at the end, taken pass by pass ([`take_by_passes`]), beside
/// [`BodyDoc::take_in_place`] and [`take_update`].
#[test]
#[ignore = "a measurement, run by name in a release build"]
fn passes() {
    println!("| shape | pass | paid by | ms |");
    println!("| --- | --- | --- | --- |");
    for (name, document) in shapes::measured(&PASSED) {
        let doc = from_body(&body_of(&document));
        let state = state_of(&doc);
        let changes = changes_of(&state);
        println!(
            "| {name} | the document: {} nodes, a state of {:.2} MB, {} changes of its writer | | |",
            nodes_of(&document),
            mega(state.len()),
            changes,
        );
        drop(document);
        let update = typed(&doc);
        let mut rounds = Vec::new();
        let mut wholes = Vec::new();
        for round in 0..RUNS {
            let whole = || {
                let held = BodyDoc::from_state(&state, &LIMITS).expect("opened").doc;
                let start = Instant::now();
                let taken = held
                    .take_in_place(None, changes, &update, &LIMITS)
                    .expect("taken");
                (start.elapsed(), taken)
            };
            let by_passes = || {
                let fresh = from_state(&state, &LIMITS).expect("opened").doc;
                let probe = CacheProbe::build(&fresh, &state).expect("cached");
                take_by_passes(fresh, probe, &state, changes, &update)
            };
            let ((took, taken), (passes, got_state, body, files)) = if round % 2 == 0 {
                let by_passes = by_passes();
                (whole(), by_passes)
            } else {
                let whole = whole();
                (whole, by_passes())
            };
            let start = Instant::now();
            let with = with_update(&doc, &update, &LIMITS).expect("taken");
            let with_update = start.elapsed();
            assert_eq!(taken.repair.deleted, 0, "{name}: nothing to repair");
            assert_eq!(got_state.len(), taken.state_bytes, "{name}: state_bytes");
            assert!(got_state == taken.doc.state(), "{name}: same state");
            assert_eq!(body, taken.doc.body().expect("a body"), "{name}: same body");
            assert_eq!(files, taken.doc.file_ids(), "{name}: same file_ids");
            drop((taken, with));
            wholes.push((took, with_update, passes.in_the_take()));
            rounds.push(passes);
        }
        let row = |pass: &str, paid: Paid, runs: Runs| {
            println!("| {name} | {pass} | {} | {runs} |", paid.by());
        };
        for (at, (pass, paid, _)) in rounds[0].0.iter().enumerate() {
            assert!(
                rounds.iter().all(|passes| passes.0[at].0 == *pass),
                "{name}: every round makes the same passes"
            );
            if *paid == Paid::Told {
                println!("| {name} | {pass} | | |");
                continue;
            }
            let times: Vec<Duration> = rounds.iter().map(|passes| passes.0[at].2).collect();
            row(pass, *paid, Runs::of(&times));
        }
        let of = |each: fn(&(Duration, Duration, Duration)) -> Duration| {
            Runs::of(&wholes.iter().map(each).collect::<Vec<_>>())
        };
        row(
            "the passes the take pays for",
            Paid::Told,
            of(|whole| whole.2),
        );
        row("take_in_place", Paid::InTheTake, of(|whole| whole.0));
        // Less than nothing where the passes, each timed alone, took longer.
        let mut rest: Vec<f64> = wholes
            .iter()
            .map(|whole| (whole.0.as_secs_f64() - whole.2.as_secs_f64()) * 1000.0)
            .collect();
        rest.sort_by(f64::total_cmp);
        println!(
            "| {name} | take_in_place less its passes | | {:.1} ({:.1}–{:.1}) |",
            rest[RUNS / 2],
            rest[0],
            rest[RUNS - 1]
        );
        row("with_update (copy+1–6)", Paid::Told, of(|whole| whole.1));
    }
}

/// How long what editors send to the largest documents takes ([`support::honest`]),
/// and making such a document again from its body, as the restore of a version does:
/// each within [`honest::LONGEST`] for the documents of the shapes people write
/// ([`honest::WRITTEN`]).
#[test]
#[ignore = "a measurement, run by name in a release build"]
fn honest() {
    println!("| document | edit | bytes | ms |");
    println!("| --- | --- | --- | --- |");
    let mut slow = Vec::new();
    for (name, document) in honest::bodies_within(shapes::nodes(), shapes::bytes()) {
        let body = body_of(&document);
        let (made, (state, changes)) = timed(|| {
            let made = BodyDoc::from_body(Some(&body));
            let state = made.state();
            let changes = fvoci_collab::client_changes(&state).expect("a state made here");
            (state.len(), changes)
        });
        println!(
            "| {name} | made again from its body: a state of {state} B, {changes} changes of its writer | | {made} |",
        );
        let held = honest::WRITTEN.contains(&name);
        if held && made.middle > honest::LONGEST {
            slow.push((name, "made again from its body"));
        }
        let doc = honest::document(&document);
        for edit in honest::edits(&doc, &LIMITS) {
            let edit_state = state_of(&edit.doc);
            let edit_changes = changes_of(&edit_state);
            let (took, _) = timed_with(
                || {
                    BodyDoc::from_state(&edit_state, &LIMITS)
                        .expect("opened")
                        .doc
                },
                |doc| {
                    doc.take_in_place(None, edit_changes, &edit.update, &LIMITS)
                        .expect("taken")
                },
            );
            println!(
                "| {name} | {} | {} | {took} |",
                edit.name,
                edit.update.len()
            );
            if held && took.middle > honest::LONGEST {
                slow.push((name, edit.name));
            }
        }
    }
    assert!(slow.is_empty(), "past {:?}: {slow:?}", honest::LONGEST);
}

/// The paragraphs [`remade`] types one after the other.
const UPDATES: usize = 5;

/// Whether the server makes a document again from its body at once after an update
/// (`OpenDoc::crowded`, which `keep_up` asks after every update; its marks are read
/// here the same way: three
/// quarters of the clients a document may name, of the changes a client may have, or
/// of the steps a reading may take once one more client wrote), with the changes a
/// client may have at `client_changes`.
fn crowds(clients: usize, (changes, client_changes): (u64, u64), steps_ahead: u64) -> bool {
    let three_quarters =
        |count: u64, limit: u64| count.saturating_mul(4) >= limit.saturating_mul(3);
    let count = |number: usize| u64::try_from(number).expect("a count fits");
    three_quarters(count(clients), count(MAX_CLIENTS))
        || three_quarters(changes, client_changes)
        || three_quarters(steps_ahead, READING_STEPS)
}

/// The fewest of `limit` that are three quarters of it, as [`crowds`] tells.
fn mark(limit: u64) -> u64 {
    limit.saturating_mul(3).div_ceil(4)
}

/// How many of [`UPDATES`] paragraphs typed one after the other into the document of
/// each body of [`honest::bodies_within`] the size measured ([`shapes::nodes`], [`shapes::bytes`]) have the server make it again from its body
/// ([`crowds`]), and whether one is refused: a body whose own document is past the mark
/// is made again at every update, as `keep_up` asks of a crowded document not whether
/// it grew since (it asks so only of one near the limit of its state, which a
/// document made again a few paragraphs before is not).
/// `FVOCI_COST_CLIENT_CHANGES` gives the changes a client may have in place of
/// [`MAX_CLIENT_CHANGES`].
#[test]
#[ignore = "a measurement, run by name in a release build"]
fn remade() {
    let client_changes =
        std::env::var("FVOCI_COST_CLIENT_CHANGES").map_or(MAX_CLIENT_CHANGES, |value| {
            value
                .parse()
                .unwrap_or_else(|error| panic!("FVOCI_COST_CLIENT_CHANGES={value:?}: {error}"))
        });
    let limits = UpdateLimits {
        client_changes: Some(client_changes),
        ..LIMITS
    };
    println!(
        "changes a client may have: {client_changes}, made again from {}",
        mark(client_changes)
    );
    println!(
        "| document | changes of its writer | of the mark | updates taken | made again | refused |"
    );
    println!("| --- | --- | --- | --- | --- | --- |");
    for (name, document) in honest::bodies_within(shapes::nodes(), shapes::bytes()) {
        let body = body_of(&document);
        drop(document);
        let mut doc = from_body(&body);
        let changes = fvoci_collab::client_changes(&state_of(&doc)).expect("a state made here");
        let (mut taken, mut remade, mut refused) = (0, 0, String::new());
        for _ in 0..UPDATES {
            match take_update(&doc, &typed(&doc), &limits) {
                Ok(with) => {
                    taken += 1;
                    let clients = with.doc.transact().state_vector().len();
                    doc = if crowds(
                        clients,
                        (with.client_changes, client_changes),
                        with.steps_ahead,
                    ) {
                        remade += 1;
                        from_body(with.repair.body.as_ref().expect("a body"))
                    } else {
                        with.doc
                    };
                }
                Err(error) => {
                    refused = error.to_string();
                    break;
                }
            }
        }
        let share = |count: u64| f64::from(u32::try_from(count).expect("a count of changes fits"));
        let of_the_mark = share(changes) / share(mark(client_changes));
        println!("| {name} | {changes} | {of_the_mark:.2} | {taken} | {remade} | {refused} |");
    }
}

/// A document of `children` empty paragraphs typed one after the other by one client:
/// each names the one before it to its left, so that the library goes past each by
/// what it passed already, which takes longer than past children made from a body.
fn typed_in_order(children: usize) -> Doc {
    let changes = (0..u64::try_from(children).expect("a count fits"))
        .map(|clock| match clock.checked_sub(1) {
            None => Change::under(Parent::body(), Content::element("paragraph")),
            Some(before) => Change::after((7, before), Content::element("paragraph")),
        })
        .collect();
    let roomy = UpdateLimits {
        update_bytes: usize::MAX,
        state_bytes: usize::MAX,
        placing_steps: None,
        clients: None,
        client_changes: None,
        total_changes: None,
        reading_steps: None,
        ..LIMITS
    };
    take_update(&new_doc(), &Update::of(7, 0, changes).bytes(), &roomy)
        .expect("typed")
        .doc
}

/// How long one update that is made to be slow takes, by the children of the root it
/// is walked through, how they were made, and the changes it holds.
#[test]
#[ignore = "a measurement, run by name in a release build"]
fn crowded() {
    println!("| children | made | changes | update | outcome | ms |");
    println!("| --- | --- | --- | --- | --- | --- |");
    let filter: Option<usize> = std::env::var("FVOCI_COST_CHILDREN")
        .ok()
        .and_then(|children| children.parse().ok());
    for children in [1_000, 10_000, 25_000, BODY_MAX_NODES - 2] {
        if filter.is_some_and(|only| only != children) {
            continue;
        }
        let body = body_of(&document(&vec![json!({"type": "paragraph"}); children]));
        for (made, doc) in [
            ("from a body", from_body(&body)),
            ("typed in order", typed_in_order(children)),
        ] {
            for row in crowd(children, made, &doc) {
                println!("{row}");
            }
        }
        for row in walked_again(children, &typed_in_order(children)) {
            println!("{row}");
        }
    }
}

/// How long the most changes the limit takes of a client below the document's take,
/// each named after the middle child and before a child ahead of it, so that the
/// library walks on through the second half: once as the update is taken and again
/// every time it reads the document again. With a node no body has, the update is
/// repaired, which reads it twice more. A row of the table each.
fn walked_again(children: usize, doc: &Doc) -> Vec<String> {
    let children = u64::try_from(children).expect("a count fits");
    let walking = |count: u64, repaired: bool| {
        let mut changes: Vec<Change> = (0..count)
            .map(|right| Change::Item {
                left: Some((7, children / 2)),
                right: Some((7, right)),
                parent: Parent::body(),
                key: None,
                content: Content::Deleted(1),
            })
            .collect();
        if repaired {
            changes.push(Change::after(
                (7, children - 1),
                Content::element("nothing-a-body-has"),
            ));
        }
        Update::of(1, 0, changes).bytes()
    };
    let within =
        |count: u64| placing_steps(doc, &walking(count, true)).expect("counted") <= PLACING_STEPS;
    let (mut most, mut past) = (0_u64, children / 2);
    while past - most > 1 {
        let count = most.midpoint(past);
        if within(count) {
            most = count;
        } else {
            past = count;
        }
    }
    [
        ("a client below", false),
        ("a client below, repaired", true),
    ]
    .into_iter()
    .map(|(made, repaired)| {
        let update = walking(most, repaired);
        let start = Instant::now();
        let outcome = match take_update(doc, &update, &LIMITS) {
            Ok(_) => "taken".to_owned(),
            Err(error) => format!("refused: {error}"),
        };
        format!(
            "| {children} | {made} | {most} | {} B | {outcome} | {} |",
            update.len(),
            millis(start.elapsed())
        )
    })
    .collect()
}

/// How long updates that crowd the root of `doc`, of `children`, take: a row of the
/// table each.
fn crowd(children: usize, made: &str, doc: &Doc) -> Vec<String> {
    // The most changes the limit takes: each walks the children and each other.
    let most = (1..usize::MAX)
        .take_while(|changes: &usize| {
            let steps = changes * (children + changes);
            u64::try_from(steps).is_ok_and(|steps| steps <= PLACING_STEPS)
        })
        .last()
        .unwrap_or(1);
    [100, most, 1_000, 5_400]
        .into_iter()
        .map(|count| {
            // Above every client the server takes at random (32 bits), so that each
            // change is placed after the children and not before them.
            let update = crowding(1 << 40, count);
            let start = Instant::now();
            let outcome = match take_update(doc, &update, &LIMITS) {
                Ok(_) => "taken".to_owned(),
                Err(error) => format!("refused: {error}"),
            };
            format!(
                "| {children} | {made} | {count} | {} B | {outcome} | {} |",
                update.len(),
                millis(start.elapsed())
            )
        })
        .collect()
}

/// The allocator of this test: the system's, which counts what is allocated now and
/// the most that was, so that the memory of a document is read without what the
/// allocator keeps for itself.
struct Counting;

static ALLOCATED: AtomicUsize = AtomicUsize::new(0);
static MOST: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every call is handed on to the system's allocator with the arguments it was
// given; the counters beside it hold no memory.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let now = ALLOCATED.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
        MOST.fetch_max(now, Ordering::Relaxed);
        // SAFETY: the caller keeps the contract of `alloc` for this layout.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        ALLOCATED.fetch_sub(layout.size(), Ordering::Relaxed);
        // SAFETY: the caller gives a block this allocator gave it with this layout.
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static COUNTING: Counting = Counting;

/// What is allocated now, in bytes.
fn allocated() -> usize {
    ALLOCATED.load(Ordering::Relaxed)
}

/// The most that is allocated while `run` runs, above what was allocated before it.
fn most_during<T>(run: impl FnOnce() -> T) -> usize {
    let before = allocated();
    MOST.store(before, Ordering::Relaxed);
    drop(run());
    MOST.load(Ordering::Relaxed).saturating_sub(before)
}

/// The memory of each shape: what the document a worker holds takes, and the most an
/// in-place update takes above it while it is taken, with a repair and without. Bytes
/// the program asked for; what the allocator keeps around them comes on top.
#[test]
#[ignore = "a measurement, run by name in a release build"]
fn memory() {
    println!("| shape | state | changes | held | take | take+repair |");
    println!("| --- | --- | --- | --- | --- | --- |");
    for (name, document) in shapes::measured(&SHAPES) {
        let state = state_of(&from_body(&body_of(&document)));
        drop(document);
        let sample = from_state(&state, &LIMITS).expect("opened").doc;
        let typed = typed(&sample);
        let unknown = unknown(&sample);
        drop(sample);

        let before = allocated();
        let opened = BodyDoc::from_state(&state, &LIMITS).expect("opened");
        let doc = opened.doc;
        let changes = opened.changes;
        drop(opened.repair);
        drop(opened.state);
        let holding = allocated().saturating_sub(before);
        let taking = most_during(|| {
            doc.take_in_place(None, changes, &typed, &LIMITS)
                .expect("taken")
        });

        let opened = BodyDoc::from_state(&state, &LIMITS).expect("opened");
        let doc = opened.doc;
        let changes = opened.changes;
        drop(opened.repair);
        drop(opened.state);
        let repairing = most_during(|| {
            doc.take_in_place(None, changes, &unknown, &LIMITS)
                .expect("repaired")
        });

        let per = |bytes: usize| mega(bytes) / mega(state.len());
        println!(
            "| {name} | {:.2} MB | {} | {:.1} MB ({:.1}x) | +{:.1} MB ({:.1}x) | +{:.1} MB ({:.1}x) |",
            mega(state.len()),
            changes_of(&state),
            mega(holding),
            per(holding),
            mega(taking),
            per(taking),
            mega(repairing),
            per(repairing),
        );
    }
}

/// How long the library takes to read a state, the middle of three readings.
fn read_by_the_library(state: &[u8]) -> Duration {
    use yrs::updates::decoder::Decode as _;
    use yrs::{ReadTxn as _, Transact as _};

    timed(|| {
        let doc = new_doc();
        let update = yrs::Update::decode_v1(state).expect("an update");
        let mut txn = doc.transact_mut();
        txn.apply_update(update).expect("applied");
        assert!(!txn.has_missing_updates(), "changes are left waiting");
    })
    .0
    .middle
}

/// How long the library takes for a step where the changes of a list lie far from
/// their neighbours: changes that walk a list of [`scattered_list`], as many as make
/// about three quarters of the steps a document may take, beside the list without
/// them. The time of a step grows with the list, and so does the reading without.
#[test]
#[ignore = "a measurement, run by name in a release build"]
fn scattered() {
    println!(
        "| list | changes | bytes | read ms | without ms | steps | ns a step | counted in ms |"
    );
    println!("| --- | --- | --- | --- | --- | --- | --- | --- |");
    for children in [50_000, 200_000, 390_000, 700_000] {
        let changes = (READING_STEPS * 3 / 4 / children).max(1);
        let without = read_by_the_library(&state_of_runs(scattered_list(children).0));
        let (mut runs, last) = scattered_list(children);
        runs.push(made(ABOVE, changes, |_| named(None, Some(last))));
        let state = state_of_runs(runs);
        let took = read_by_the_library(&state);
        let (counting, steps) =
            timed(|| fvoci_collab::reading_steps(&state, u64::MAX).expect("a state"));
        let more = took.saturating_sub(without).as_nanos();
        println!(
            "| {children} | {changes} | {} | {} | {} | {steps} | {} | {} |",
            state.len(),
            millis(took),
            millis(without),
            more.checked_div(u128::from(steps)).unwrap_or(0),
            counting,
        );
    }
}

/// How long the library takes to read a state made to be slow, beside the steps that
/// are counted of it before: a way to make it slow whose time grows where the steps do
/// not is not counted. `FVOCI_COST_SLOW` names the ways measured, by a part of their
/// names; with 8,000 changes all of them take ten minutes.
#[test]
#[ignore = "a measurement, run by name in a release build"]
fn reading() {
    println!("| made slow by | changes | bytes | read ms | steps | ns a step | counted in ms |");
    println!("| --- | --- | --- | --- | --- | --- | --- |");
    let read = read_by_the_library;
    let only = std::env::var("FVOCI_COST_SLOW").ok();
    let measured = |name: &str| {
        only.as_ref()
            .is_none_or(|only| name.contains(only.as_str()))
    };
    let mut base: Vec<Duration> = Vec::new();
    for changes in [0, 62, 744, 8_000] {
        for (at, (name, state)) in slow_states(changes).into_iter().enumerate() {
            if !measured(name) {
                base.resize(base.len().max(at + 1), Duration::ZERO);
                continue;
            }
            let took = read(&state);
            let (counting, steps) =
                timed(|| fvoci_collab::reading_steps(&state, u64::MAX).expect("a state"));
            if changes == 0 {
                base.resize(at, Duration::ZERO);
                base.push(took);
            }
            // Over what the state takes without the changes made for it.
            let more = took.saturating_sub(base[at]).as_nanos();
            let a_step = more.checked_div(u128::from(steps)).unwrap_or(0);
            println!(
                "| {name} | {changes} | {} | {} | {steps} | {a_step} | {} |",
                state.len(),
                millis(took),
                counting,
            );
        }
    }
}

/// The most changes that each name the last of [`LIST`] typed paragraphs to their
/// right an update of a client above may hold for `doc`, from `clock` on.
fn most_before_the_last(doc: &Doc, clock: u64) -> (u64, Vec<u8>) {
    let update = |changes: u64| {
        let each = (0..changes).map(|_| named(None, Some((7, LIST - 1))));
        Update::of(ABOVE, clock, each.collect()).bytes()
    };
    let within =
        |changes: u64| placing_steps(doc, &update(changes)).expect("counted") <= PLACING_STEPS;
    let (mut most, mut past) = (1_u64, 2 * PLACING_STEPS / LIST + 2);
    while past - most > 1 {
        let changes = most.midpoint(past);
        if within(changes) {
            most = changes;
        } else {
            past = changes;
        }
    }
    (most, update(most))
}

/// How long an update and an opening take after updates at the limit were sent one
/// after the other to a document of [`LIST`] paragraphs typed in order, each of as
/// many changes as an update may hold that walk the list at every reading: as they
/// were taken before the steps of a reading were counted, and now.
#[test]
#[ignore = "a measurement, run by name in a release build"]
fn accumulated() {
    let uncounted = UpdateLimits {
        reading_steps: None,
        ..LIMITS
    };
    let stored = |limits: &UpdateLimits| UpdateLimits {
        placing_steps: None,
        clients: None,
        client_changes: None,
        reading_steps: limits.reading_steps,
        ..LIMITS
    };
    println!("| limits | update | bytes | take ms | outcome | steps to read | open ms |");
    println!("| --- | --- | --- | --- | --- | --- | --- |");
    for (name, limits) in [("before", uncounted), ("now", LIMITS)] {
        let mut doc = typed_in_order(usize::try_from(LIST).expect("a count fits"));
        let mut clock = 0;
        for round in 1..=12 {
            let (most, update) = most_before_the_last(&doc, clock);
            let start = Instant::now();
            let taken = take_update(&doc, &update, &limits);
            let take = start.elapsed();
            let outcome = match taken {
                Ok(taken) if taken.remake => {
                    format!("taken (remake, {} steps)", taken.reading_steps)
                }
                Ok(taken) => {
                    doc = taken.doc;
                    clock += most;
                    "taken".to_owned()
                }
                Err(error) => format!("refused: {error}"),
            };
            let state = state_of(&doc);
            let steps = fvoci_collab::reading_steps(&state, u64::MAX).expect("a state");
            let start = Instant::now();
            from_state(&state, &stored(&limits)).expect("opened");
            println!(
                "| {name} | {round} | {} | {} | {outcome} | {steps} | {} |",
                update.len(),
                millis(take),
                millis(start.elapsed()),
            );
        }
    }
}

/// How long what an editor sends takes on a document that is at the steps it may take
/// to read, and its opening: [`LIST`] paragraphs typed in order, and as many changes
/// that walk the list at every reading as are taken, four an update.
#[test]
#[ignore = "a measurement, run by name in a release build"]
fn at_its_steps() {
    let stored = UpdateLimits {
        placing_steps: None,
        clients: None,
        client_changes: None,
        ..LIMITS
    };
    let four = |clock: u64| {
        let each = (0..4).map(|_| named(None, Some((7, LIST - 1))));
        Update::of(ABOVE, clock, each.collect()).bytes()
    };
    let paragraph = |doc: &Doc| {
        edited(doc, |root, txn| {
            root.push_back(txn, XmlElementPrelim::empty("paragraph"));
        })
    };
    println!("| changes that walk | steps to read | update | take ms | outcome | open ms |");
    println!("| --- | --- | --- | --- | --- | --- |");
    let mut doc = typed_in_order(usize::try_from(LIST).expect("a count fits"));
    let mut clock = 0;
    loop {
        let state = state_of(&doc);
        let steps = fvoci_collab::reading_steps(&state, u64::MAX).expect("a state");
        let (open, _) = timed(|| from_state(&state, &stored).expect("opened"));
        for (name, update) in [
            ("a paragraph typed", paragraph(&doc)),
            ("a paragraph typed, repaired", unknown(&doc)),
        ] {
            let (take, taken) = timed(|| take_update(&doc, &update, &LIMITS).is_ok());
            let outcome = if taken { "taken" } else { "refused" };
            println!("| {clock} | {steps} | {name} | {take} | {outcome} | {open} |");
        }
        let Ok(taken) = take_update(&doc, &four(clock), &LIMITS) else {
            break;
        };
        if taken.remake {
            break;
        }
        doc = taken.doc;
        clock += 4;
    }
}

/// Measures the accept-and-remake path in ms: a 2,047-session document with 415
/// paragraphs deleted and then 600 paragraphs deleted (taken with `remake: true`, plus
/// remaking via `from_body`), and the `before_the_last` walking update on a
/// 49,998-paragraph document.
#[test]
#[ignore = "a measurement, run by name in a release build"]
fn remake_past_steps() {
    const SESSIONS: u64 = 2_047;
    const READ_FIRST: u64 = 3_000;
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
        runs.push(support::wire::Run {
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
    let stored_limits = UpdateLimits {
        placing_steps: None,
        clients: None,
        client_changes: None,
        ..LIMITS
    };
    let doc = from_state(&state, &stored_limits).expect("opened").doc;
    let deletes = |from: u64, count: u64, runs: Vec<support::wire::Run>| {
        let deleted = (from..from + count)
            .map(|session| {
                let client = SESSIONS - session;
                let stretches = (0..49_u64).map(|odd| (3 + odd * 2, 1)).collect();
                (client, stretches)
            })
            .collect();
        Update { runs, deleted }.bytes()
    };
    let read_first = vec![support::wire::Run {
        client: READ_FIRST,
        clock: 0,
        changes: vec![Change::after((1, 102), Content::element("paragraph"))],
    }];
    let first = deletes(0, 415, read_first);
    let after_first = take_update(&doc, &first, &LIMITS).expect("first taken");
    assert!(!after_first.remake, "first stays within reading steps");
    let second = deletes(415, 600, Vec::new());

    let (take_remake, taken) =
        timed(|| take_update(&after_first.doc, &second, &LIMITS).expect("taken"));
    assert!(taken.remake, "second triggers remake");
    let body = taken.repair.body.expect("body");
    let (remake_body, _) = timed(|| from_body(&body));
    let (take_and_remake, _) = timed(|| {
        let taken = take_update(&after_first.doc, &second, &LIMITS).expect("taken");
        assert!(taken.remake, "second triggers remake");
        from_body(taken.repair.body.as_ref().expect("body"))
    });

    let typed = typed_in_order(usize::try_from(LIST).expect("fits"));
    let (_, walkers) = most_before_the_last(&typed, 0);
    let (walk_take, walk_taken) = timed(|| take_update(&typed, &walkers, &LIMITS).expect("taken"));
    assert!(walk_taken.remake, "walkers trigger remake");
    let (walk_remake, _) = timed(|| from_body(walk_taken.repair.body.as_ref().expect("body")));
    let (walk_total, _) = timed(|| {
        let taken = take_update(&typed, &walkers, &LIMITS).expect("taken");
        assert!(taken.remake, "walkers trigger remake");
        from_body(taken.repair.body.as_ref().expect("body"))
    });

    println!("| case | take_update (remake) ms | from_body ms | take + from_body ms |");
    println!("| --- | --- | --- | --- |");
    println!("| 2047 sessions, 415+600 del | {take_remake} | {remake_body} | {take_and_remake} |");
    println!("| 49,998 paras + walkers | {walk_take} | {walk_remake} | {walk_total} |");
}
