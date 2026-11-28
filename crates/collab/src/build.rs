//! Making the Y document of a stored body.

mod summary;
mod wire;

use std::sync::Arc;

use fvoci_model::Body;
use serde_json::Value;
use yrs::updates::decoder::Decode as _;
use yrs::{
    Any, ClientID, Doc, ID, Number, OffsetKind, Options, Transact as _, TransactionMut, Update,
    Xml as _, XmlElementPrelim, XmlElementRef, XmlFragment, XmlTextPrelim,
};

use self::summary::prefill_block_cache;
use self::wire::{CacheMode, MAX_WRITER, MIN_RANDOM_WRITER, encode_body_with_plan, scan_body_plan};
use crate::block_cache::element_id;
use crate::doc::StateCounts;
use crate::reading::count_strict_with_cache;
use crate::take::DocCache;
use crate::update::{UpdateError, state_of};
use crate::{FIELD, READING_STEPS};

/// Output of [`build_body_doc_counted`]: a `Doc`, its canonical v1 state bytes, exact
/// counts, and prefilled incremental caches.
pub(crate) struct BuiltBodyDoc {
    pub(crate) doc: Doc,
    pub(crate) state: Vec<u8>,
    pub(crate) counts: StateCounts,
    pub(crate) cache: Option<Box<DocCache>>,
}

/// An empty Y document for a body, whose root is the fragment the editors are bound to.
/// A card without a body starts from this: an editor shows its empty paragraph over an
/// empty fragment and writes when somebody types.
#[must_use]
pub fn new_doc() -> Doc {
    doc_of_client(None)
}

/// A Y document that counts the places in a text as the editors do, in UTF-16 code units,
/// and keeps what was deleted instead of collecting it.
///
/// The library's garbage collection, given changes that no editor makes (one whose
/// origins lie in a parent's map), left a change pointing at memory it had freed, and
/// the next encoding of the document read it: the state came out different each time,
/// or the process ended. Without it, what was deleted stays in the document as it was
/// written; [`crate::UpdateLimits::state_bytes`] bounds it, and a document made again
/// from its body ([`from_body`]) starts without it.
pub(crate) fn doc_of_client(client: Option<u64>) -> Doc {
    let options = Options {
        offset_kind: OffsetKind::Utf16,
        skip_gc: true,
        ..client.map_or_else(Options::default, |id| {
            Options::with_client_id(ClientID::new(id))
        })
    };
    let doc = Doc::with_options(options);
    doc.get_or_insert_xml_fragment(FIELD);
    doc
}

/// Makes the Y document of a stored body, in the shape the editor's binding makes of it:
/// an element per node named for its type, the neighbouring text nodes of a line in one
/// text with their marks as its formatting, and an attribute the body leaves out
/// written with the value an editor gives it.
///
/// The server makes it when a body is first opened for editing together; no editor does.
/// The body is written by a client (or a small descending run of clients for very large
/// bodies) whose identifier is taken at random, anew each time: an update made against
/// another document, or against an earlier making of this one, builds on changes this
/// document does not have and is refused ([`crate::UpdateError::MissingChanges`]) instead
/// of landing somewhere in it.
#[must_use]
pub fn from_body(body: &Body) -> Doc {
    from_body_by(body, random_writer())
}

/// [`from_body`] with the identifier of the writing client given. The same body and
/// writer give the same document, byte for byte; two documents by the same writer take
/// each other's updates as their own, which is why [`from_body`] gives each its own.
/// A writer is a whole number JavaScript keeps, of 53 bits: the bits above are left
/// out, as a document by a larger one could not be read again.
///
/// Large bodies with more than `MAX_CLIENT_CHANGES / 4` changes split deterministically
/// across strictly descending client IDs starting from `writer` (shifted upward only when
/// `writer` is too small to fit the descending run down to zero inclusive), keeping
/// `reading_steps` at zero.
#[must_use]
pub fn from_body_by(body: &Body, writer: u64) -> Doc {
    let plan = scan_body_plan(body.document());
    if plan.can_build_direct() {
        return build_unmarked_doc(body, writer & MAX_WRITER, None);
    }
    let encoded = encode_body_with_plan(body, writer, plan, CacheMode::None);
    doc_from_encoded(&encoded.state, encoded.last_client)
}

/// Builds a [`Doc`] with prefilled [`DocCache`] when callers do not immediately need the
/// encoded state bytes or [`StateCounts`] (`BodyDoc::from_body`).
pub(crate) fn build_body_doc_fast(body: &Body) -> (Doc, Option<Box<DocCache>>) {
    let plan = scan_body_plan(body.document());
    if plan.can_build_direct() {
        let mut top_ids = Vec::new();
        let doc = build_unmarked_doc(body, random_writer(), Some(&mut top_ids));
        let cache = prefill_block_cache(body.document(), &top_ids).map(|blocks| {
            Box::new(DocCache {
                blocks,
                reading: None,
            })
        });
        return (doc, cache);
    }
    let encoded = encode_body_with_plan(body, random_writer(), plan, CacheMode::Full);
    let doc = doc_from_encoded(&encoded.state, encoded.last_client);
    let reading = (!encoded.needs_verify)
        .then_some(encoded.reading_cache)
        .flatten();
    let cache = prefill_block_cache(body.document(), &encoded.top_level_ids)
        .map(|blocks| Box::new(DocCache { blocks, reading }));
    (doc, cache)
}

/// Builds a [`Doc`] together with its canonical v1 state bytes, exact [`StateCounts`],
/// and prefilled [`DocCache`] (`BlockCache` + `ReadingCache`) in one linear pass.
///
/// # Errors
///
/// Returns [`UpdateError::Unreadable`] if the resulting state violates `scan` framing
/// rules (e.g., an unvalidated `Body::from_stored` with >53-bit integers or deeply nested
/// attributes).
pub(crate) fn build_body_doc_counted(body: &Body) -> Result<BuiltBodyDoc, UpdateError> {
    let plan = scan_body_plan(body.document());
    if plan.can_build_direct() {
        let mut top_ids = Vec::new();
        let doc = build_unmarked_doc(body, random_writer(), Some(&mut top_ids));
        let state = state_of(&doc);
        let (read, reading) = count_strict_with_cache(&state, READING_STEPS)
            .map_err(|refusal| UpdateError::Unreadable(refusal.reason().to_owned()))?;
        let counts = StateCounts {
            changes: read.walked.changes,
            client_changes: read.walked.most,
            steps_ahead: read.ahead,
        };
        let cache = prefill_block_cache(body.document(), &top_ids)
            .map(|blocks| Box::new(DocCache { blocks, reading }));
        return Ok(BuiltBodyDoc {
            doc,
            state,
            counts,
            cache,
        });
    }
    let encoded = encode_body_with_plan(body, random_writer(), plan, CacheMode::Full);
    let doc = doc_from_encoded(&encoded.state, encoded.last_client);
    let (state, counts, reading) = if encoded.needs_verify {
        let state = state_of(&doc);
        let (read, reading) = count_strict_with_cache(&state, READING_STEPS)
            .map_err(|refusal| UpdateError::Unreadable(refusal.reason().to_owned()))?;
        let counts = StateCounts {
            changes: read.walked.changes,
            client_changes: read.walked.most,
            steps_ahead: read.ahead,
        };
        (state, counts, reading)
    } else {
        (encoded.state, encoded.counts, encoded.reading_cache)
    };
    let cache = prefill_block_cache(body.document(), &encoded.top_level_ids)
        .map(|blocks| Box::new(DocCache { blocks, reading }));
    Ok(BuiltBodyDoc {
        doc,
        state,
        counts,
        cache,
    })
}

/// Direct single-transaction `Doc` builder for unmarked single-client bodies, producing
/// wire bytes identical to [`wire::encode_body_with_plan`] without wire round-tripping.
fn build_unmarked_doc(body: &Body, client: u64, mut top_ids: Option<&mut Vec<ID>>) -> Doc {
    let doc = doc_of_client(Some(client));
    let root = doc.get_or_insert_xml_fragment(FIELD);
    {
        let mut txn = doc.transact_mut();
        push_unmarked_nodes(
            &mut txn,
            &root,
            content_of(body.document()),
            top_ids.as_deref_mut(),
        );
    }
    if let Some(ids) = top_ids {
        ids.reverse();
    }
    doc
}

fn push_unmarked_nodes(
    txn: &mut TransactionMut<'_>,
    parent: &impl XmlFragment,
    nodes: &[Value],
    mut top_ids: Option<&mut Vec<ID>>,
) {
    let mut rest = nodes;
    while let Some(last) = rest.last() {
        let stretch = if is_text(last) {
            rest.iter().rev().take_while(|node| is_text(node)).count()
        } else {
            1
        };
        let (before, these) = rest.split_at(rest.len() - stretch);
        match these {
            [node] if !is_text(node) => {
                let kind = node.get("type").and_then(Value::as_str).unwrap_or_default();
                let element = parent.push_front(txn, XmlElementPrelim::empty(kind));
                if let Some(ids) = top_ids.as_deref_mut()
                    && let Some(id) = element_id(&element)
                {
                    ids.push(id);
                }
                fill_unmarked_element(txn, &element, node);
            }
            [only_text] => {
                parent.push_front(txn, XmlTextPrelim::new(letters_of(only_text)));
            }
            texts => {
                let whole: String = texts.iter().map(letters_of).collect();
                parent.push_front(txn, XmlTextPrelim::new(whole));
            }
        }
        rest = before;
    }
}

fn fill_unmarked_element(txn: &mut TransactionMut<'_>, element: &XmlElementRef, node: &Value) {
    push_unmarked_nodes(txn, element, content_of(node), None);
    write_attributes(txn, element, node);
}

/// Writes the attributes of a node's element as an editor gives them: each the node
/// has, and those it leaves out with the value an editor writes for them.
pub(crate) fn write_attributes(
    txn: &mut TransactionMut<'_>,
    element: &XmlElementRef,
    node: &Value,
) {
    let kind = node.get("type").and_then(Value::as_str).unwrap_or_default();
    let given = node.get("attrs");
    for attribute in crate::schema::node_attributes(kind).unwrap_or(&[]) {
        let value = match given.and_then(|attrs| attrs.get(attribute.name)) {
            Some(value) => Some(any_of(value)),
            None => match attribute.fallback {
                crate::schema::Fallback::None => None,
                crate::schema::Fallback::Integer(number) => Some(Any::Number(Number::Int(number))),
                crate::schema::Fallback::Boolean(truth) => Some(Any::Bool(truth)),
                crate::schema::Fallback::Text(text) => Some(Any::from(text)),
            },
        };
        if let Some(value) = value {
            element.insert_attribute(txn, attribute.name, value);
        }
    }
}

/// Random 32-bit writer ID with at least `MIN_RANDOM_WRITER` headroom for descending
/// multi-client splits while remaining within 32 bits (`<= u32::MAX`, strictly below
/// `repair::FIRST_WRITER`).
fn random_writer() -> u64 {
    let span = u64::from(u32::MAX) - MIN_RANDOM_WRITER + 1;
    MIN_RANDOM_WRITER + ((ClientID::random().get() & 0xffff_ffff) % span)
}

/// Decodes a freshly self-encoded v1 state into a `Doc` whose own client ID is `client`.
fn doc_from_encoded(state: &[u8], client: u64) -> Doc {
    let doc = doc_of_client(Some(client));
    if state != Update::EMPTY_V1 {
        let update = Update::decode_v1(state).expect("self-encoded body state decodes");
        doc.transact_mut()
            .apply_update(update)
            .expect("self-encoded body state applies");
    }
    doc
}

/// The content of a node; none if it has none.
pub(crate) fn content_of(node: &Value) -> &[Value] {
    node.get("content")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

pub(crate) fn is_text(node: &Value) -> bool {
    node.get("type").and_then(Value::as_str) == Some("text")
}

/// The letters of a text node.
pub(crate) fn letters_of(node: &Value) -> &str {
    node.get("text").and_then(Value::as_str).unwrap_or_default()
}

/// A JSON value as a value of the shared content. A number that is whole is written as
/// a whole number, however the JSON wrote it (`1`, `1.0`, `1e0`): JavaScript has one
/// kind of number, and the editors write a whole one as an integer.
pub(crate) fn any_of(value: &Value) -> Any {
    match value {
        Value::Null => Any::Null,
        Value::Bool(truth) => Any::Bool(*truth),
        Value::Number(number) => Any::Number(number.as_i64().map_or_else(
            || Number::try_i64(number.as_f64().unwrap_or(f64::NAN)),
            Number::Int,
        )),
        Value::String(text) => Any::String(Arc::from(text.as_str())),
        Value::Array(items) => Any::Array(items.iter().map(any_of).collect()),
        Value::Object(members) => Any::Map(Arc::new(
            members
                .iter()
                .map(|(name, member)| (name.clone(), any_of(member)))
                .collect(),
        )),
    }
}

/// Puts the element of an unmarked node, with all it holds, among the children of a
/// parent at `index`, as [`from_body`] writes one. `index` is at most the parent's length.
#[cfg(feature = "test-util")]
pub(crate) fn insert_node(
    txn: &mut TransactionMut<'_>,
    parent: &XmlElementRef,
    index: u32,
    node: &Value,
) {
    debug_assert!(
        !scan_body_plan(node).has_marks,
        "insert_node only supports unmarked nodes"
    );
    let kind = node.get("type").and_then(Value::as_str).unwrap_or_default();
    let element = parent.insert(txn, index, XmlElementPrelim::empty(kind));
    fill_unmarked_element(txn, &element, node);
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn direct_unmarked_path_is_byte_identical_to_wire_encoder() {
        let doc_json = json!({
            "type": "doc",
            "content": [
                {"type": "heading", "attrs": {"level": 2}, "content": [
                    {"type": "text", "text": "Title 한글😀"}
                ]},
                {"type": "paragraph", "content": [
                    {"type": "text", "text": "first "},
                    {"type": "text", "text": "second"},
                    {"type": "hardBreak"},
                    {"type": "inlineMath", "attrs": {"latex": "x^2 + y^2"}},
                    {"type": "mention", "attrs": {"userId": "01930000-0000-7000-8000-00000000c001"}},
                    {"type": "reference", "attrs": {"id": "01930000-0000-7000-8000-00000000b001"}}
                ]},
                {"type": "orderedList", "content": [
                    {"type": "listItem", "content": [{"type": "paragraph"}]}
                ]},
                {"type": "taskList", "content": [
                    {"type": "taskItem", "attrs": {"checked": true}, "content": [
                        {"type": "paragraph", "content": [{"type": "text", "text": "done"}]}
                    ]}
                ]},
                {"type": "codeBlock", "attrs": {"language": "rust"}, "content": [
                    {"type": "text", "text": "fn main() {}"}
                ]},
                {"type": "blockMath", "attrs": {"latex": "\\int_0^1 x\\,dx"}},
                {"type": "image", "attrs": {"fileId": "img-1", "alt": "photo"}},
                {"type": "file", "attrs": {"fileId": "doc-1"}},
                {"type": "horizontalRule"},
                {"type": "table", "content": [
                    {"type": "tableRow", "content": [
                        {"type": "tableHeader", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "H"}]}]},
                        {"type": "tableCell", "attrs": {"colspan": 2}, "content": [{"type": "paragraph"}]}
                    ]}
                ]}
            ]
        });
        let body = Body::parse(doc_json)
            .expect("valid body")
            .expect("non-empty body");
        let plan = scan_body_plan(body.document());
        assert!(
            plan.can_build_direct(),
            "unmarked small body uses direct path"
        );

        for writer in [0_u64, 1, 7, 1_048_576, u64::MAX] {
            let direct_doc = build_unmarked_doc(&body, writer & MAX_WRITER, None);
            let direct_state = state_of(&direct_doc);
            let encoded = encode_body_with_plan(&body, writer, plan, CacheMode::Full);
            let wire_doc = doc_from_encoded(&encoded.state, encoded.last_client);
            assert_eq!(
                direct_state, encoded.state,
                "writer {writer}: direct state matches wire bytes"
            );
            assert_eq!(
                state_of(&wire_doc),
                encoded.state,
                "writer {writer}: wire doc re-encodes identically"
            );
        }
    }

    #[test]
    fn wire_encoder_matches_state_of_doc_byte_for_byte() {
        let doc_json = json!({
            "type": "doc",
            "content": [
                {"type": "paragraph", "content": [
                    {"type": "text", "text": "plain "},
                    {"type": "text", "text": "bold", "marks": [{"type": "bold"}]},
                    {"type": "text", "text": " link😀", "marks": [
                        {"type": "link", "attrs": {"href": "https://example.com/a\"b\\c\n한글"}},
                        {"type": "bold"},
                        {"type": "italic"}
                    ]},
                    {"type": "text", "text": " next", "marks": [
                        {"type": "link", "attrs": {"href": "https://example.com/other"}},
                        {"type": "strike"},
                        {"type": "code"}
                    ]},
                    {"type": "text", "text": " tail"}
                ]}
            ]
        });
        let body = Body::parse(doc_json)
            .expect("valid marked body")
            .expect("non-empty body");
        let plan = scan_body_plan(body.document());
        assert!(!plan.can_build_direct(), "marked body uses wire encoder");
        let encoded = encode_body_with_plan(&body, 42, plan, CacheMode::Full);
        let doc = doc_from_encoded(&encoded.state, encoded.last_client);
        assert_eq!(
            state_of(&doc),
            encoded.state,
            "encoded marked state matches doc state byte for byte"
        );
        assert!(
            encoded.reading_cache.is_some(),
            "reading cache built alongside wire encoding"
        );
    }
}
