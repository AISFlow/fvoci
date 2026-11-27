//! States and updates made for the Y library to be slow with, which what it pays for
//! them is measured of (`cost.rs`): changes that walk long lists, cut long changes, or
//! wait for each other.

use super::wire::{Change, Content, Parent, Run, Update};

/// An update of `count` changes of one client, each deleted content of one place that
/// names the root as its parent and no neighbour: the library walks past every child
/// of the root that names none to its left either, which is all of them where the
/// server made the document from a body, to place each.
pub(crate) fn crowding(client: u64, count: usize) -> Vec<u8> {
    let changes = (0..count)
        .map(|_| Change::under(Parent::body(), Content::Deleted(1)))
        .collect();
    Update::of(client, 0, changes).bytes()
}

/// A change of deleted content with the neighbours it names.
pub(crate) fn named(left: Option<(u64, u64)>, right: Option<(u64, u64)>) -> Change {
    Change::Item {
        left,
        right,
        parent: Parent::body(),
        key: None,
        content: Content::Deleted(1),
    }
}

/// The changes of a client that types `children` paragraphs in order after `first`, or
/// as the first children of the root.
fn typing(client: u64, first: Change, children: u64) -> Run {
    let changes = std::iter::once(first)
        .chain((1..children).map(|clock| Change::after((client, clock - 1), Content::element("p"))))
        .collect();
    Run {
        client,
        clock: 0,
        changes,
    }
}

pub(crate) fn state_of_runs(runs: Vec<Run>) -> Vec<u8> {
    Update {
        runs,
        deleted: Vec::new(),
    }
    .bytes()
}

pub(crate) const LIST: u64 = 49_998;
pub(crate) const ABOVE: u64 = 1 << 40;

/// `changes` changes of a client, each made by its number.
pub(crate) fn made(client: u64, changes: u64, each: impl Fn(u64) -> Change) -> Run {
    Run {
        client,
        clock: 0,
        changes: (0..changes).map(each).collect(),
    }
}

fn paragraph_under_root() -> Change {
    Change::under(Parent::body(), Content::element("p"))
}

/// States whose changes walk a list of [`LIST`] paragraphs typed in order.
fn walked_states(changes: u64) -> Vec<(&'static str, Vec<u8>)> {
    let typed = || typing(7, paragraph_under_root(), LIST);
    let half = LIST / 2;
    // A list whose first half is typed after a change that names the first change of
    // the second half to its right, by a client above the one that walks: the walk
    // keeps all of the first half, and empties what it keeps at every change of the
    // second, whose last change it names to its right, so that it is read after it.
    let kept_then_emptied = {
        let low = typing(5, paragraph_under_root(), half);
        let first = Change::Item {
            left: None,
            right: Some((5, 0)),
            parent: Parent::body(),
            key: None,
            content: Content::element("p"),
        };
        let high = typing(9, first, half);
        let walking = made(8, changes, |_| named(None, Some((5, half - 1))));
        vec![high, walking, low]
    };
    // As a body's own document is made: each child before the one made before it,
    // which it names to its right.
    let from_a_body = made(3, LIST, |clock| match clock.checked_sub(1) {
        None => paragraph_under_root(),
        Some(after) => Change::Item {
            left: None,
            right: Some((3, after)),
            parent: Parent::body(),
            key: None,
            content: Content::element("p"),
        },
    });
    vec![
        (
            "typed in order, before the last, a client above",
            state_of_runs(vec![
                typed(),
                made(ABOVE, changes, |_| named(None, Some((7, LIST - 1)))),
            ]),
        ),
        (
            "typed in order, from the middle to before it, a client below",
            state_of_runs(vec![
                typed(),
                made(1, changes, |at| {
                    named(Some((7, half)), Some((7, at % half)))
                }),
            ]),
        ),
        (
            "typed in order, no neighbour, a client above",
            state_of_runs(vec![typed(), made(ABOVE, changes, |_| named(None, None))]),
        ),
        (
            "half kept, then emptied at every change",
            state_of_runs(kept_then_emptied),
        ),
        (
            "made from a body, before the last, a client above",
            state_of_runs(vec![
                from_a_body,
                made(ABOVE, changes, |_| named(None, Some((3, 0)))),
            ]),
        ),
    ]
}

/// States whose changes cut a long change of letters.
fn cut_states(changes: u64) -> Vec<(&'static str, Vec<u8>)> {
    let letters = |count: u64| "a".repeat(usize::try_from(count).expect("a count fits"));
    let lettered = |more: Vec<Change>| {
        let mut all = vec![Change::under(Parent::body(), Content::text(&letters(LIST)))];
        all.extend(more);
        Run {
            client: 7,
            clock: 0,
            changes: all,
        }
    };
    // A client of many changes none of which is joined to the one before it, the first
    // of them long, and changes that each name a place inside the first one, and the
    // last of the many to their right, so that they are read after all of them.
    let many: u64 = 400_000;
    let long = changes + 2;
    let mut of_many = vec![Change::under(Parent::body(), Content::text(&letters(long)))];
    of_many.extend((0..many).map(|_| Change::under(Parent::body(), Content::Deleted(1))));
    let moved = vec![
        Run {
            client: 7,
            clock: 0,
            changes: of_many,
        },
        made(9, changes, |at| {
            named(Some((7, at)), Some((7, long + many - 1)))
        }),
    ];
    // Letters and a change after them, and changes that each name a letter further on
    // to their left and the change after the letters to their right: each cuts what is
    // left of the letters, and all stand after them, so that the letters are joined
    // again and cut again at every reading.
    let cut_again = vec![
        lettered(vec![Change::after((7, LIST - 1), Content::element("p"))]),
        made(9, changes, |at| {
            named(Some((7, at % (LIST - 2))), Some((7, LIST)))
        }),
    ];
    vec![
        (
            "letters cut from the end down",
            state_of_runs(vec![
                lettered(Vec::new()),
                made(9, changes, |at| {
                    named(Some((7, LIST - 2 - at % (LIST - 2))), None)
                }),
            ]),
        ),
        (
            "a first change cut, 400,000 changes after it",
            state_of_runs(moved),
        ),
        (
            "letters cut from the start up, joined again",
            state_of_runs(cut_again),
        ),
    ]
}

/// Clients that each wait for the one below, the lowest of them for the last change
/// of a client of many.
fn waiting_state(changes: u64) -> (&'static str, Vec<u8>) {
    let chained = changes.min(16_000);
    let of_many: u64 = 100_000;
    let mut runs = vec![made(7, of_many, |_| {
        Change::under(Parent::body(), Content::Deleted(1))
    })];
    runs.extend((0..chained).map(|at| {
        let waits_for = if at == 0 {
            (7, of_many - 1)
        } else {
            (1000 + at - 1, 0)
        };
        made(1000 + at, 1, |_| named(Some(waits_for), None))
    }));
    (
        "clients that wait for each other, 100,000 changes below",
        state_of_runs(runs),
    )
}

/// The states the library is slow to read, by what makes it slow, with `changes`
/// changes made for it.
pub(crate) fn slow_states(changes: u64) -> Vec<(&'static str, Vec<u8>)> {
    let mut states = walked_states(changes);
    states.extend(cut_states(changes));
    states.push(waiting_state(changes));
    states
}

/// A list of `children` deleted places that two clients make in turn, each put after
/// a place made before it, whichever, as an editor puts it between that place and the
/// one after it; and the place that is last in the list, which is the last made, so
/// that what names it is read after all of the list.
pub(crate) fn scattered_list(children: u64) -> (Vec<Run>, (u64, u64)) {
    const NONE: usize = usize::MAX;
    let id = |made: usize| {
        let made = u64::try_from(made).expect("a count fits");
        (7 + made % 2, made / 2)
    };
    let children = usize::try_from(children).expect("a count fits");
    let mut seed = 0x2545_f491_4f6c_dd1d_u64;
    let mut next: Vec<usize> = Vec::with_capacity(children);
    let mut changes = [Vec::new(), Vec::new()];
    let mut last = 0;
    for made in 0..children {
        let change = if made == 0 {
            next.push(NONE);
            named(None, None)
        } else {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let anywhere = usize::try_from(seed >> 33).expect("31 bits fit") % made;
            let after = if made + 2 < children { anywhere } else { last };
            let right = next[after];
            next.push(right);
            next[after] = made;
            if right == NONE {
                last = made;
            }
            named(Some(id(after)), (right != NONE).then(|| id(right)))
        };
        changes[made % 2].push(change);
    }
    let runs = changes
        .into_iter()
        .zip([7, 8])
        .map(|(changes, client)| Run {
            client,
            clock: 0,
            changes,
        })
        .collect();
    (runs, id(last))
}
