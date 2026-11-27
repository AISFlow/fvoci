//! Bytes from anybody: whatever arrives as an update, nothing here panics. The update
//! is refused, or it is taken and the document it leaves is projected and repaired
//! like any other.

mod support;

use fvoci_collab::testing::{repair, to_body, with_update};
use fvoci_collab::{new_doc, state_of};
use support::random::Random;
use support::{ROOMY, vectors};
use yrs::Doc;

/// Everything the server does with an update, on a document that holds `base`.
fn take(base: &Doc, update: &[u8]) {
    let Ok(with) = with_update(base, update, &ROOMY) else {
        return;
    };
    let first = to_body(&with);
    let before = state_of(&with);
    if let Ok(repaired) = repair(&with) {
        assert_eq!(
            to_body(&with),
            Ok(repaired.body),
            "a repaired document is its body"
        );
        if first.is_ok() {
            assert_eq!(
                state_of(&with),
                before,
                "a repair of a body changes nothing"
            );
        }
    } else {
        assert!(first.is_err(), "a body needs no repair");
        assert_eq!(
            state_of(&with),
            before,
            "a repair that fails deletes nothing"
        );
    }
    // What would be stored can be read again.
    let stored = state_of(&with);
    assert!(
        with_update(&new_doc(), &stored, &ROOMY).is_ok(),
        "the state is an update"
    );
}

/// Updates no editor sends, to be damaged with the rest: letters and values put
/// straight under the root and under a node, a text holding a value.
fn odd_updates() -> Vec<Vec<u8>> {
    let root = |info: u8| {
        let mut bytes = vec![1, 1, 5, 0, info, 1, 7];
        bytes.extend_from_slice(b"default");
        bytes
    };
    let mut letters = root(4);
    letters.extend_from_slice(&[6, b'a', 0xf0, 0x9f, 0x98, 0x80, b'b', 0]);
    let mut values = root(8);
    values.extend_from_slice(&[3, 126, 125, 7, 119, 1, b'u', 0]);
    // A paragraph under the root, then letters, a value and a text under the paragraph.
    let mut inside = vec![1, 4, 5, 0, 7, 1, 7];
    inside.extend_from_slice(b"default");
    inside.extend_from_slice(&[3, 9]);
    inside.extend_from_slice(b"paragraph");
    inside.extend_from_slice(&[4, 0, 5, 0, 5, 0xf0, 0x9f, 0x98, 0x80, b'x']);
    inside.extend_from_slice(&[0x80 | 8, 5, 3, 1, 120]);
    inside.extend_from_slice(&[0x80 | 7, 5, 4, 6]);
    inside.push(0);
    vec![letters, values, inside]
}

#[test]
fn bytes_at_random_are_refused_or_taken_without_a_panic() {
    let mut random = Random::new(1);
    let empty = new_doc();
    for _ in 0..150_000 {
        let length = random.below(200);
        let mut bytes = random.bytes(length);
        // Half of them begin as an update of one client with a few structs does, so
        // that more than the first bytes are read.
        if random.chance(2) {
            let head = [1, 1 + random.byte() % 4, random.byte() % 8, 0];
            bytes.splice(0..bytes.len().min(head.len()), head);
        }
        take(&empty, &bytes);
    }
}

/// Every update of the vectors, damaged: a byte changed, a stretch cut out, bytes put
/// in, cut short, or two of them run together.
#[test]
fn damaged_updates_are_refused_or_taken_without_a_panic() {
    let updates: Vec<Vec<u8>> = ["documents", "bodies", "readings", "refused", "repairs"]
        .iter()
        .flat_map(|section| vectors(section))
        .map(|vector| vector.update)
        .chain(odd_updates())
        .collect();
    let mut random = Random::new(2);
    let empty = new_doc();
    for update in &updates {
        // Also on a document that holds the undamaged update already.
        let holder = with_update(&empty, update, &ROOMY).expect("an undamaged update is taken");
        take(&empty, update);
        for _ in 0..400 {
            let mut bytes = update.clone();
            for _ in 0..=random.below(3) {
                let at = random.below(bytes.len().max(1));
                match random.below(7) {
                    // A value nested deep, put in anywhere: wherever the library would
                    // read it as one, it must have been refused before.
                    6 => {
                        let mut deep = [117, 1].repeat(9 + random.below(20_000));
                        deep.push(126);
                        bytes.splice(at..at, deep);
                    }
                    0 | 1 => {
                        if let Some(byte) = bytes.get_mut(at) {
                            *byte = random.byte();
                        }
                    }
                    2 => {
                        if let Some(byte) = bytes.get_mut(at) {
                            *byte ^= 1 << random.below(8);
                        }
                    }
                    3 => {
                        let end = (at + random.below(8)).min(bytes.len());
                        bytes.drain(at..end);
                    }
                    4 => {
                        let length = random.below(6);
                        let more = random.bytes(length);
                        bytes.splice(at..at, more);
                    }
                    _ => {
                        let other = random.pick(&updates);
                        let from = random.below(other.len().max(1));
                        bytes.truncate(at);
                        bytes.extend_from_slice(other.get(from..).unwrap_or_default());
                    }
                }
            }
            take(&empty, &bytes);
            take(&holder, &bytes);
        }
    }
}

/// An update of one struct under the body's root: a value that is a list in a list,
/// `depth` deep.
fn nested_value(depth: usize) -> Vec<u8> {
    // One client, one struct, client 5 at clock 0; the struct holds values, has no
    // neighbours and names its parent, the root "default"; one value.
    let mut bytes = vec![1, 1, 5, 0, 8, 1, 7];
    bytes.extend_from_slice(b"default");
    bytes.push(1);
    for _ in 0..depth {
        // A list of one.
        bytes.extend_from_slice(&[117, 1]);
    }
    // Null at the bottom, and nobody deleted anything.
    bytes.extend_from_slice(&[126, 0]);
    bytes
}

/// The same value, wherever a change may hold one: after another change, with a
/// neighbour to its left, as an attribute.
fn nested_values_elsewhere(depth: usize) -> Vec<Vec<u8>> {
    let mut deep = [117, 1].repeat(depth);
    deep.push(126);
    let root = |info: u8| {
        let mut bytes = vec![info, 1, 7];
        bytes.extend_from_slice(b"default");
        bytes
    };
    // Two changes: a text "x" under the root, then values to its right.
    let mut after_another = vec![1, 2, 5, 0];
    after_another.extend(root(4));
    after_another.extend([1, b'x']);
    after_another.extend([0x80 | 8, 5, 0, 1]);
    after_another.extend(&deep);
    after_another.push(0);
    // An attribute of a node: a paragraph under the root (5:0), then its member "k".
    let mut attribute = vec![1, 2, 5, 0];
    attribute.extend(root(7));
    attribute.extend([3, 9]);
    attribute.extend_from_slice(b"paragraph");
    attribute.extend([0x20 | 8, 0, 5, 0, 1, b'k', 1]);
    attribute.extend(&deep);
    attribute.push(0);
    // The second of two values, behind a text of the letters that open a list.
    let mut second = vec![1, 1, 5, 0];
    second.extend(root(8));
    second.extend([2, 119, 4, 117, 117, 118, 118]);
    second.extend(&deep);
    second.push(0);
    vec![after_another, attribute, second]
}

/// The Y library reads a value that nests by calling itself once a level, and an
/// update that nests one deep enough ends the process. It does not get to read one.
#[test]
fn a_value_nested_deep_is_refused_before_the_library_reads_it() {
    let empty = new_doc();
    for depth in [9, 1000, 20_000, 700_000] {
        let updates = std::iter::once(nested_value(depth)).chain(nested_values_elsewhere(depth));
        for (index, update) in updates.enumerate() {
            assert_eq!(
                with_update(&empty, &update, &ROOMY).map(|_| ()),
                Err(fvoci_collab::UpdateError::Unreadable(
                    "a value in it nests too deep".to_owned()
                )),
                "depth {depth}, update {index}"
            );
        }
    }
    // The same updates with a value that nests a little are taken, so the refusal
    // above is of the nesting and of nothing else in them.
    for (index, update) in std::iter::once(nested_value(3))
        .chain(nested_values_elsewhere(3))
        .enumerate()
    {
        assert!(
            with_update(&empty, &update, &ROOMY).is_ok(),
            "update {index}"
        );
    }
}

/// What the refusal above is for, shown in a process of its own: the library, handed
/// such an update directly, does not come back. (When this test fails the library has
/// learnt to refuse deep values itself, and the walk before it can go.)
#[test]
fn the_library_alone_does_not_survive_a_value_nested_deep() {
    let this = std::env::current_exe().expect("the path of this test");
    let child = std::process::Command::new(this)
        .args([
            "nesting_probe",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("FVOCI_NESTING", "200000")
        .output()
        .expect("the test runs itself");
    let said = String::from_utf8_lossy(&child.stderr);
    assert!(!child.status.success(), "the child came back: {said}");
    assert!(
        said.contains("overflowed its stack"),
        "the child said: {said}"
    );
}

/// The body of the test above, run in a process of its own with `FVOCI_NESTING=<depth>`;
/// without it, nothing.
#[test]
fn nesting_probe() {
    use yrs::updates::decoder::Decode as _;
    let Some(depth) = std::env::var("FVOCI_NESTING")
        .ok()
        .and_then(|depth| depth.parse().ok())
    else {
        return;
    };
    let bytes = nested_value(depth);
    let decoded = yrs::Update::decode_v1(&bytes);
    println!("depth {depth}: decoded {}", decoded.is_ok());
    drop(decoded);
    println!("depth {depth}: dropped");
}
