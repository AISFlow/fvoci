//! The bodies that what a worker pays for a document is measured of (`cost.rs`), the
//! updates sent to them, and how a measurement is timed.
//!
//! Each body is as large as a body may be, or as `FVOCI_COST_NODES` and
//! `FVOCI_COST_BYTES` say where they are less, so that one build measures smaller
//! bodies as well; `FVOCI_COST_SHAPES` names the shapes measured, by their names
//! between commas, and each is made only as it is measured.

use std::fmt;
use std::time::{Duration, Instant};

use fvoci_model::{BODY_MAX_BYTES, BODY_MAX_DEPTH, BODY_MAX_NODES};
use serde_json::{Value, json};
use yrs::{Doc, XmlElementPrelim, XmlFragment as _, XmlTextPrelim};

use super::edited;
use super::honest::{self, Between};

/// The nodes a shape leaves free: a paragraph with a text is appended to it.
const SPARE_NODES: usize = 4;

/// The shapes of [`shape`], by name.
pub(crate) const SHAPES: [&str; 10] = [
    "flat",
    "lists",
    "deep",
    "text",
    "wide",
    "breaks",
    "marks",
    "marks and breaks",
    "marks and formulas",
    "marked line",
];

/// The nodes the bodies measured have: as many as a body may, or `FVOCI_COST_NODES`
/// where that is less.
pub(crate) fn nodes() -> usize {
    within("FVOCI_COST_NODES", 10_000, BODY_MAX_NODES)
}

/// The bytes the bodies measured have: as many as a body may, or `FVOCI_COST_BYTES`
/// where that is less.
pub(crate) fn bytes() -> usize {
    within("FVOCI_COST_BYTES", 1024 * 1024, BODY_MAX_BYTES)
}

/// The number the variable `name` holds, at most `limit`; `limit` where it is not set.
///
/// # Panics
///
/// Panics if it holds no number, or one below `least`, which the shapes have no room
/// in.
fn within(name: &str, least: usize, limit: usize) -> usize {
    let Ok(value) = std::env::var(name) else {
        return limit;
    };
    let value: usize = value
        .parse()
        .unwrap_or_else(|error| panic!("{name}={value:?} is no number: {error}"));
    assert!(value >= least, "{name}={value} is below {least}");
    value.min(limit)
}

/// The shapes measured, each made as it is reached: those `FVOCI_COST_SHAPES` names
/// of `among`, between commas, or all of them.
///
/// # Panics
///
/// Panics if it names a shape that is none of `among`.
pub(crate) fn measured(among: &[&'static str]) -> impl Iterator<Item = (&'static str, Value)> {
    let only: Option<Vec<String>> = std::env::var("FVOCI_COST_SHAPES")
        .ok()
        .map(|only| only.split(',').map(|one| one.trim().to_owned()).collect());
    if let Some(only) = &only {
        for one in only {
            assert!(
                among.contains(&one.as_str()),
                "FVOCI_COST_SHAPES names {one:?}, none of {among:?}"
            );
        }
    }
    let chosen: Vec<&'static str> = among
        .iter()
        .copied()
        .filter(|name| {
            only.as_ref()
                .is_none_or(|only| only.iter().any(|one| one == name))
        })
        .collect();
    chosen.into_iter().map(|name| (name, shape(name)))
}

fn paragraph(text: &str) -> Value {
    json!({"type": "paragraph", "content": [{"type": "text", "text": text}]})
}

pub(crate) fn document(blocks: &[Value]) -> Value {
    json!({"type": "doc", "content": blocks})
}

/// A chain of quotes down to the deepest a body goes, with a text at its end, and how
/// many nodes it is.
fn chain() -> (Value, usize) {
    let mut node = paragraph("deep");
    // The document, the quotes, the paragraph and its text.
    let quotes = BODY_MAX_DEPTH - 3;
    for _ in 0..quotes {
        node = json!({"type": "blockquote", "content": [node]});
    }
    (node, quotes + 2)
}

/// The body of the shape `name` ([`SHAPES`]), as large as [`nodes`] and [`bytes`] let it
/// be but for [`SPARE_NODES`].
///
/// # Panics
///
/// Panics if `name` is none of [`SHAPES`].
pub(crate) fn shape(name: &str) -> Value {
    let nodes = nodes() - SPARE_NODES - 1;
    let bytes = bytes() - 1024;
    match name {
        "flat" => document(&vec![paragraph("a paragraph"); nodes / 2]),
        "lists" => {
            let items: Vec<Value> = (0..10)
                .map(|_| json!({"type": "listItem", "content": [paragraph("entry")]}))
                .collect();
            let list = json!({"type": "bulletList", "content": items});
            document(&vec![list; nodes / 31])
        }
        "deep" => {
            let (chain, chain_nodes) = chain();
            document(&vec![chain; nodes / chain_nodes])
        }
        "text" => {
            let long = "가나다라마바사아자차".repeat(100);
            let long_bytes = serde_json::to_vec(&paragraph(&long)).expect("JSON").len() + 1;
            document(&vec![paragraph(&long); bytes / long_bytes])
        }
        "wide" => document(&vec![json!({"type": "paragraph"}); nodes]),
        "breaks" => document(&[
            json!({"type": "paragraph", "content": vec![json!({"type": "hardBreak"}); nodes - 1]}),
        ]),
        "marks" => honest::marked(Between::Letter, nodes, bytes),
        "marks and breaks" => honest::marked(Between::Break, nodes, bytes),
        "marks and formulas" => honest::marked(Between::Math, nodes, bytes),
        "marked line" => honest::marked_paragraph(nodes, bytes),
        other => panic!("no shape is named {other:?}"),
    }
}

/// The update of a paragraph typed at the end.
pub(crate) fn typed(doc: &Doc) -> Vec<u8> {
    edited(doc, |root, txn| {
        let paragraph = root.push_back(txn, XmlElementPrelim::empty("paragraph"));
        paragraph.push_back(txn, XmlTextPrelim::new("typed"));
    })
}

/// The update of a node no body has at the end: taking it repairs the document.
pub(crate) fn unknown(doc: &Doc) -> Vec<u8> {
    edited(doc, |root, txn| {
        root.push_back(txn, XmlElementPrelim::empty("nothing-a-body-has"));
    })
}

/// How many times a measurement is taken.
pub(crate) const RUNS: usize = 3;

/// The times of [`RUNS`] runs of one measurement: the middle one, and the least and
/// the most, which are how far the others were from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Runs {
    pub(crate) least: Duration,
    pub(crate) middle: Duration,
    pub(crate) most: Duration,
}

impl Runs {
    /// The runs of `times`, in any order.
    ///
    /// # Panics
    ///
    /// Panics if there are none.
    pub(crate) fn of(times: &[Duration]) -> Self {
        let mut sorted = times.to_vec();
        sorted.sort_unstable();
        let (Some(&least), Some(&most)) = (sorted.first(), sorted.last()) else {
            panic!("a measurement is taken at least once");
        };
        Self {
            least,
            middle: sorted[sorted.len() / 2],
            most,
        }
    }
}

impl fmt::Display for Runs {
    /// The middle in milliseconds, and the least and the most after it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} ({}–{})",
            millis(self.middle),
            millis(self.least),
            millis(self.most)
        )
    }
}

/// Runs `run` [`RUNS`] times, and gives the times with what the middle run gave.
pub(crate) fn timed<T>(mut run: impl FnMut() -> T) -> (Runs, T) {
    let mut runs: Vec<(Duration, T)> = (0..RUNS)
        .map(|_| {
            let start = Instant::now();
            let value = run();
            (start.elapsed(), value)
        })
        .collect();
    let times: Vec<Duration> = runs.iter().map(|(took, _)| *took).collect();
    runs.sort_by_key(|(took, _)| *took);
    (Runs::of(&times), runs.swap_remove(RUNS / 2).1)
}

pub(crate) fn millis(took: Duration) -> String {
    format!("{:.1}", took.as_secs_f64() * 1000.0)
}

pub(crate) fn mega(bytes: usize) -> f64 {
    f64::from(u32::try_from(bytes).expect("a size of megabytes")) / 1e6
}

pub(crate) fn nodes_of(node: &Value) -> usize {
    1 + node["content"]
        .as_array()
        .map_or(0, |children| children.iter().map(nodes_of).sum())
}
