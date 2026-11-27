//! What editors send to the largest documents: the edits that are taken whatever the
//! document within the limits of a body, and the time they are held to.

use std::time::Duration;

use fvoci_collab::{
    MAX_CLIENT_CHANGES, MAX_CLIENTS, MAX_TOTAL_CHANGES, PLACING_STEPS, READING_STEPS, UpdateLimits,
    take_update,
};
use fvoci_model::{BODY_MAX_BYTES, BODY_MAX_NODES};
use serde_json::{Value, json};
use yrs::{Doc, Text, XmlElementPrelim, XmlFragment, XmlOut, XmlTextPrelim};

use super::{body_of, edited};

/// The limits the server holds an update to with its settings as they come.
pub(crate) const SERVED: UpdateLimits = UpdateLimits {
    update_bytes: 64 * 1024,
    state_bytes: 16 * 1024 * 1024,
    body_bytes: BODY_MAX_BYTES,
    placing_steps: Some(PLACING_STEPS),
    clients: Some(MAX_CLIENTS),
    client_changes: Some(MAX_CLIENT_CHANGES),
    total_changes: Some(MAX_TOTAL_CHANGES),
    reading_steps: Some(READING_STEPS),
};

/// The longest an editor's update may take on a document at the limits, and the
/// making of such a document from its body (release build, an idle machine).
pub(crate) const LONGEST: Duration = Duration::from_millis(500);

/// The documents of [`bodies`] held to [`LONGEST`]: those of the shapes people write.
/// The bodies of the most changes ([`marked`]) are no such documents; how long their
/// edits take is measured and told, not held.
pub(crate) const WRITTEN: [&str; 2] = ["100,000 nodes", "8 MiB"];

/// The paragraphs of the largest paste: what fills an update of 64 KiB.
const PASTED: u32 = 350;

/// The letters of each pasted paragraph.
const PASTED_LETTERS: usize = 120;

/// The nodes and bytes the largest paste adds to a body, and its undo once more.
fn paste_room() -> (usize, usize) {
    let pasted = usize::try_from(PASTED).expect("a count fits");
    (2 * pasted + 10, 2 * pasted * (PASTED_LETTERS + 64))
}

fn paragraph(text: &str) -> Value {
    json!({"type": "paragraph", "content": [{"type": "text", "text": text}]})
}

/// The bodies of the documents edited, by name: as many nodes as a body may have, as
/// many bytes, and as many changes, but for the room the largest paste takes.
pub(crate) fn bodies() -> Vec<(&'static str, Value)> {
    bodies_within(BODY_MAX_NODES, BODY_MAX_BYTES)
}

/// [`bodies`] of `max_nodes` and `max_bytes`, for measurements of smaller bodies. The
/// names are those of the bodies at the limits.
///
/// # Panics
///
/// Panics if they leave no room for the largest paste.
pub(crate) fn bodies_within(max_nodes: usize, max_bytes: usize) -> Vec<(&'static str, Value)> {
    let (room_nodes, room_bytes) = paste_room();
    assert!(
        max_nodes > 2 * room_nodes && max_bytes > 2 * room_bytes,
        "{max_nodes} nodes and {max_bytes} bytes are no more than twice the room of the largest paste"
    );
    let nodes = (max_nodes - room_nodes) / 2;
    let long = "가나다라마바사아자차".repeat(100);
    let long_bytes = serde_json::to_vec(&paragraph(&long)).expect("JSON").len() + 1;
    let long_count = (max_bytes - room_bytes) / long_bytes;
    vec![
        (
            "100,000 nodes",
            json!({"type": "doc", "content": vec![paragraph("a paragraph of the document"); nodes]}),
        ),
        (
            "8 MiB",
            json!({"type": "doc", "content": vec![paragraph(&long); long_count]}),
        ),
    ]
    .into_iter()
    .chain(Between::ALL.map(|between| {
        let name = match between {
            Between::Letter => "marks",
            Between::Break => "marks and breaks",
            Between::Math => "marks and formulas",
        };
        let body = marked(
            between,
            max_nodes - room_nodes,
            max_bytes - room_bytes,
        );
        (name, body)
    }))
    .collect()
}

/// What stands between the letters of every mark in [`marked`].
#[derive(Debug, Clone, Copy)]
pub(crate) enum Between {
    /// A letter of no mark.
    Letter,
    /// A line break: of the shapes tried, the most changes of the client that writes it
    /// (each letter its own text).
    Break,
    /// A formula: of the shapes tried, the most memory to take an update for.
    Math,
}

impl Between {
    pub(crate) const ALL: [Self; 3] = [Self::Letter, Self::Break, Self::Math];

    fn node(self) -> Value {
        match self {
            Self::Letter => json!({"type": "text", "text": "a"}),
            Self::Break => json!({"type": "hardBreak"}),
            Self::Math => json!({"type": "inlineMath", "attrs": {"latex": "x"}}),
        }
    }
}

/// The bodies that make the most changes of the client that writes them, within
/// `nodes` and `bytes`: paragraphs of 200 nodes, letters each with every mark in turn
/// with what stands `between`. Every mark is a change where it begins and another where
/// it ends.
pub(crate) fn marked(between: Between, nodes: usize, bytes: usize) -> Value {
    marked_by(between, Some(200), nodes, bytes)
}

/// [`marked`] by letters in a single paragraph, as long as `nodes` and `bytes` let it
/// be: the marks of one text, which the library finds its places in by walking from the
/// start of the text.
pub(crate) fn marked_paragraph(nodes: usize, bytes: usize) -> Value {
    marked_by(Between::Letter, None, nodes, bytes)
}

/// Paragraphs of `length` nodes each, or one of as many as fit.
fn marked_by(between: Between, length: Option<usize>, nodes: usize, bytes: usize) -> Value {
    let every = json!({"type": "text", "text": "a", "marks": [
        {"type": "bold"}, {"type": "italic"}, {"type": "strike"}, {"type": "code"},
        {"type": "link", "attrs": {"href": "/"}},
    ]});
    let other = between.node();
    let size = |value: &Value| serde_json::to_vec(value).expect("JSON").len() + 1;
    // The document's own bytes and node.
    let (bytes, nodes) = (bytes - 64, nodes - 1);
    let length = length.unwrap_or_else(|| {
        let pair = size(&every) + size(&other);
        // What a paragraph is without its content, which the letters do not have.
        let wrapper = size(&json!({"type": "paragraph", "content": []}));
        (nodes - 1).min((bytes - wrapper) / pair * 2)
    });
    let content: Vec<Value> = [every, other].into_iter().cycle().take(length).collect();
    let paragraph = json!({"type": "paragraph", "content": content});
    let count = (bytes / size(&paragraph)).min(nodes / (length + 1));
    json!({"type": "doc", "content": vec![paragraph; count]})
}

/// The document of a body, as the server makes it.
pub(crate) fn document(body: &Value) -> Doc {
    fvoci_collab::from_body(&body_of(body))
}

/// The text of the paragraph at `index` of the root.
fn text_at(
    root: &yrs::XmlFragmentRef,
    txn: &yrs::TransactionMut<'_>,
    index: u32,
) -> yrs::XmlTextRef {
    let Some(XmlOut::Element(paragraph)) = root.get(txn, index) else {
        panic!("a paragraph at {index}");
    };
    let Some(XmlOut::Text(text)) = paragraph.get(txn, 0) else {
        panic!("a text in the paragraph at {index}");
    };
    text
}

fn paste(root: &yrs::XmlFragmentRef, txn: &mut yrs::TransactionMut<'_>, at: u32, count: u32) {
    let letters = "pasted ".repeat(PASTED_LETTERS / 7 + 1)[..PASTED_LETTERS].to_owned();
    for offset in 0..count {
        let paragraph = root.insert(txn, at + offset, XmlElementPrelim::empty("paragraph"));
        paragraph.push_back(txn, XmlTextPrelim::new(letters.as_str()));
    }
}

/// An edit an editor sends, and the document it is sent to.
pub(crate) struct Edit {
    pub(crate) name: &'static str,
    pub(crate) doc: Doc,
    pub(crate) update: Vec<u8>,
}

/// What editors send to `doc`, whose root holds paragraphs of one text each, within
/// `limits`: a letter typed, a paste of a few paragraphs, the largest paste, its undo,
/// the undo of a deletion as large, and what an editor that was offline for a while
/// sends when it is back.
pub(crate) fn edits(doc: &Doc, limits: &UpdateLimits) -> Vec<Edit> {
    let children = {
        let copy = fvoci_collab::testing::copy_of(doc).expect("read again");
        let root = copy.get_or_insert_xml_fragment(fvoci_collab::FIELD);
        let txn = yrs::Transact::transact(&copy);
        root.len(&txn)
    };
    let middle = children / 2;
    let typed = edited(doc, |root, txn| {
        text_at(root, txn, middle).insert(txn, 3, "x");
    });
    let pasted = edited(doc, |root, txn| paste(root, txn, middle, 5));
    let largest = edited(doc, |root, txn| paste(root, txn, middle, PASTED));
    let with_largest = take_update(doc, &largest, limits)
        .expect("the largest paste is taken")
        .doc;
    let undone = edited(&with_largest, |root, txn| {
        root.remove_range(txn, middle, PASTED);
    });
    // As many paragraphs as were pasted, or the second half of a document of fewer.
    let deleting = PASTED.min(children - middle);
    let deleted = edited(doc, |root, txn| root.remove_range(txn, middle, deleting));
    let without = take_update(doc, &deleted, limits)
        .expect("a deletion as large is taken")
        .doc;
    // What the undo of a deletion sends: what was deleted, written anew.
    let written_anew = edited(&without, |root, txn| paste(root, txn, middle, PASTED));
    // Offline for a while: letters typed in fifty paragraphs, twenty paragraphs written
    // and thirty deleted, all over the document.
    let back = edited(doc, |root, txn| {
        let spread = |count: u32, at: u32| (at + 1) * (children - 100) / (count + 1);
        for at in 0..50 {
            text_at(root, txn, spread(50, at)).insert(txn, 3, "typed offline");
        }
        for at in (0..20).rev() {
            paste(root, txn, spread(20, at), 1);
        }
        for at in (0..30).rev() {
            root.remove_range(txn, spread(30, at) + 3, 1);
        }
    });
    vec![
        Edit {
            name: "a letter typed",
            doc: copy(doc),
            update: typed,
        },
        Edit {
            name: "a paste of five paragraphs",
            doc: copy(doc),
            update: pasted,
        },
        Edit {
            name: "the largest paste",
            doc: copy(doc),
            update: largest,
        },
        Edit {
            name: "its undo",
            doc: with_largest,
            update: undone,
        },
        Edit {
            name: "the undo of as large a deletion",
            doc: without,
            update: written_anew,
        },
        Edit {
            name: "back from offline",
            doc: copy(doc),
            update: back,
        },
    ]
}

fn copy(doc: &Doc) -> Doc {
    fvoci_collab::testing::copy_of(doc).expect("read again")
}
