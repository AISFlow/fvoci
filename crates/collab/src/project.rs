//! Projecting a Y document to the JSON that is stored.

use std::fmt::Write as _;

use fvoci_model::{BODY_MAX_DEPTH, BODY_MAX_NODES, Body, BodyError};
use serde_json::{Value, json};
use yrs::{Doc, ReadTxn, Transact, XmlElementRef, XmlFragment, XmlFragmentRef};

use crate::FIELD;
use crate::read::{self, Child, Content, Pushed};

/// Projects a Y document to the body an editor would store from it, and checks it as
/// any body is checked: against the allowlist and the limits of size, depth and nodes.
/// `None` is no body: an empty document, or one empty paragraph.
///
/// The JSON is the stored form: an attribute an editor fills in is there (but for the
/// start of an ordered list at 1 and a cell's span of 1, which a body leaves out), one
/// no editor reads is not, an empty `attrs` is left out, the marks of a text are in the
/// editor's order, and neighbouring texts with the same marks are one. So the body of a
/// document made from a body ([`crate::from_body`]) is no larger than that body.
///
/// # Errors
///
/// Returns the [`BodyError`] a body of that JSON is refused with. Shared content that
/// has no JSON at all (letters or a value where a node belongs, a fragment inside the
/// document, something in a text that is no letter) is refused as
/// [`BodyError::InvalidNode`].
pub(crate) fn to_body(doc: &Doc) -> Result<Option<Body>, BodyError> {
    let txn = doc.transact();
    // A document nothing was ever written into has no root yet, and no body.
    txn.get_xml_fragment(FIELD)
        .map_or(Ok(None), |root| project(&txn, &root))
}

/// Projects the fragment at the root.
pub(crate) fn project<T: ReadTxn>(
    txn: &T,
    root: &XmlFragmentRef,
) -> Result<Option<Body>, BodyError> {
    projected(txn, root)?.map_or(Ok(None), Body::parse)
}

/// The JSON of the fragment at the root, before it is checked as a body: the walk of
/// [`project`] alone. `None` for no body.
pub(crate) fn projected<T: ReadTxn>(
    txn: &T,
    root: &XmlFragmentRef,
) -> Result<Option<Value>, BodyError> {
    // A text at the root, even one without letters, makes an editor's binding throw.
    if let Some(index) = read::children(txn, root)
        .unwrap_or_default()
        .iter()
        .position(|child| matches!(child, Child::Text(_)))
    {
        return Err(BodyError::InvalidNode {
            path: format!("/content/{index}"),
            problem: "a text stands at the root",
        });
    }
    let mut walk = Walk {
        txn,
        nodes: 1,
        path: Vec::new(),
        has_empty_row_text: false,
    };
    let content = walk.content(root, 2, ParentKind::Other)?;
    if content.is_empty() {
        return Ok(None);
    }
    Ok(Some(json!({"type": "doc", "content": content})))
}

/// Projects a single top-level `element` (at depth 2 under the root), returning its JSON
/// node, the number of JSON nodes in its subtree, and whether any `tableRow` inside it
/// holds an empty `XmlText` child (which `repair::Survey::row` removes during a repair).
pub(crate) fn project_element<T: ReadTxn>(
    txn: &T,
    element: &XmlElementRef,
) -> Result<(Value, usize, bool), BodyError> {
    let mut walk = Walk {
        txn,
        nodes: 1,
        path: vec![0],
        has_empty_row_text: false,
    };
    walk.count(2)?;
    let kind = ParentKind::of_tag(element.tag().as_ref());
    let inner = walk.content(element, 3, kind)?;
    let node = read::element_node(txn, element, inner);
    Ok((node, walk.nodes - 1, walk.has_empty_row_text))
}

/// The compact JSON size of a [`Value`] in bytes, without allocating the JSON string.
pub(crate) fn json_size_of(value: &Value) -> usize {
    /// Counts what is written to it.
    struct Count(usize);

    impl std::io::Write for Count {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.saturating_add(bytes.len());
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let mut count = Count(0);
    // Writing JSON to a counter does not fail: a `Value` is always JSON.
    let _ = serde_json::to_writer(&mut count, value);
    count.0
}

/// Whether the parent whose children are being walked is a `tableRow`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ParentKind {
    Other,
    TableRow,
}

impl ParentKind {
    fn of_tag(tag: &str) -> Self {
        if tag == "tableRow" {
            Self::TableRow
        } else {
            Self::Other
        }
    }
}

/// A walk over shared content: how many nodes it has seen, and where it is.
struct Walk<'txn, T> {
    txn: &'txn T,
    /// The nodes seen so far, the document included. Kept so that the walk stops at the
    /// limit instead of building the JSON of a document that is refused anyway.
    nodes: usize,
    /// The index of each node on the way down, in the JSON.
    path: Vec<usize>,
    /// Set when a `tableRow` parent contains an empty `XmlText` child.
    has_empty_row_text: bool,
}

impl<T: ReadTxn> Walk<'_, T> {
    /// A refusal of the node the walk is at.
    fn invalid(&self, problem: &'static str) -> BodyError {
        let mut path = String::new();
        for index in &self.path {
            // Writing to a string cannot fail.
            let _ = write!(path, "/content/{index}");
        }
        BodyError::InvalidNode { path, problem }
    }

    /// The content of an element or the root, whose children are at `depth`.
    fn content(
        &mut self,
        parent: &impl XmlFragment,
        depth: usize,
        parent_kind: ParentKind,
    ) -> Result<Vec<Value>, BodyError> {
        let Some(children) = read::children(self.txn, parent) else {
            return Err(self.invalid("holds something that is neither a node nor a text"));
        };
        let mut content = Content::default();
        for child in children {
            self.path.push(content.len());
            match child {
                Child::Element(element) => {
                    self.count(depth)?;
                    let inner = self.content(
                        &element,
                        depth + 1,
                        ParentKind::of_tag(element.tag().as_ref()),
                    )?;
                    content.push(read::element_node(self.txn, &element, inner));
                }
                Child::Text(text) => {
                    let Some(nodes) = read::text_nodes(self.txn, &text) else {
                        return Err(self.invalid("text holds something that is no text"));
                    };
                    if parent_kind == ParentKind::TableRow && nodes.is_empty() {
                        self.has_empty_row_text = true;
                    }
                    // A text without letters is no node: an editor leaves one in a
                    // paragraph it emptied.
                    for node in nodes {
                        // A text that joins the one before it is no node of its own.
                        if content.push(node) == Pushed::New {
                            self.count(depth)?;
                        }
                    }
                }
                Child::Other => {
                    return Err(self.invalid("shared content is neither a node nor a text"));
                }
            }
            self.path.pop();
        }
        Ok(content.into_nodes())
    }

    /// Counts one more node, at `depth`, and stops the walk past a limit.
    fn count(&mut self, depth: usize) -> Result<(), BodyError> {
        if depth > BODY_MAX_DEPTH {
            return Err(BodyError::TooDeep);
        }
        self.nodes += 1;
        if self.nodes > BODY_MAX_NODES {
            return Err(BodyError::TooManyNodes);
        }
        Ok(())
    }
}
