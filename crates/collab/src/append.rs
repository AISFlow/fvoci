//! An update that adds nodes after the last of a body's document, written by a client
//! that has written nothing in it: for an agent that adds to a body while its editors
//! type in it. The editors take the update as one of another editor's, and what they
//! typed meanwhile stays where they typed it.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;
use yrs::types::Attrs;
use yrs::updates::decoder::Decode as _;
use yrs::{
    Any, ClientID, ReadTxn as _, StateVector, Text as _, Transact as _, TransactionMut, Update,
    XmlElementPrelim, XmlFragment, XmlTextPrelim,
};

use crate::FIELD;
use crate::build::{any_of, content_of, doc_of_client, is_text, letters_of, write_attributes};
use crate::doc::BodyDoc;
use crate::schema;
use crate::update::UpdateError;

/// The least client an append writes as: past every client of 32 bits, which the
/// editors (as the Y library has them) and a body's making take theirs from.
pub const FIRST_APPENDER: u64 = 1 << 32;

/// Past the last client an append writes as: a whole number JavaScript keeps.
const APPENDERS_END: u64 = 1 << 53;

impl BodyDoc {
    /// The update, version 1, that adds `nodes` (nodes of a body's top level, as its
    /// JSON has them) after the document's last, written by a client the document has
    /// not seen, taken at random from [`FIRST_APPENDER`] up. The document is not
    /// changed: the update is taken into it as any other ([`BodyDoc::take_in_place`]),
    /// which checks what it leaves.
    ///
    /// # Errors
    ///
    /// Returns [`UpdateError::Unreadable`] if the document's own state does not read
    /// again, which no document of this type's making is.
    pub fn append_update(&self, nodes: &[Value]) -> Result<Vec<u8>, UpdateError> {
        let state = self.state();
        let seen = self.state_vector_of_doc();
        let scratch = doc_of_client(Some(fresh_client(&seen)));
        if state != Update::EMPTY_V1 {
            let update = Update::decode_v1(&state)
                .map_err(|error| UpdateError::Unreadable(error.to_string()))?;
            scratch
                .transact_mut()
                .apply_update(update)
                .map_err(|error| UpdateError::Unreadable(error.to_string()))?;
        }
        let root = scratch.get_or_insert_xml_fragment(FIELD);
        {
            let mut txn = scratch.transact_mut();
            let mut index = root.len(&txn);
            for node in nodes {
                insert_node(&mut txn, &root, index, node);
                index = index.saturating_add(1);
            }
        }
        Ok(scratch.transact().encode_diff_v1(&seen))
    }
}

/// A client from [`FIRST_APPENDER`] up that `seen` does not name.
fn fresh_client(seen: &StateVector) -> u64 {
    let span = APPENDERS_END - FIRST_APPENDER;
    loop {
        let client = FIRST_APPENDER + ClientID::random().get() % span;
        if !seen.contains_client(&ClientID::new(client)) {
            return client;
        }
    }
}

/// Puts the element of `node`, with all it holds, among the children of `parent` at
/// `index`; a text node as a text of its own.
fn insert_node(txn: &mut TransactionMut<'_>, parent: &impl XmlFragment, index: u32, node: &Value) {
    if is_text(node) {
        let text = parent.insert(txn, index, XmlTextPrelim::new(""));
        write_run(txn, &text, node);
        return;
    }
    let kind = node.get("type").and_then(Value::as_str).unwrap_or_default();
    let element = parent.insert(txn, index, XmlElementPrelim::empty(kind));
    push_front(txn, &element, content_of(node));
    write_attributes(txn, &element, node);
}

/// Puts `nodes` before the children `parent` has, in their order: the neighbouring
/// text nodes as one text with their marks as its formatting, as an editor writes
/// them. Each is put first, the last of them first, so that a run of text is written
/// where no formatting lies before it and takes only its own marks.
fn push_front(txn: &mut TransactionMut<'_>, parent: &impl XmlFragment, nodes: &[Value]) {
    let mut rest = nodes;
    while let Some(last) = rest.last() {
        if is_text(last) {
            let stretch = rest.iter().rev().take_while(|node| is_text(node)).count();
            let (before, these) = rest.split_at(rest.len() - stretch);
            let text = parent.push_front(txn, XmlTextPrelim::new(""));
            for run in these.iter().rev() {
                write_run(txn, &text, run);
            }
            rest = before;
        } else {
            let kind = last.get("type").and_then(Value::as_str).unwrap_or_default();
            let element = parent.push_front(txn, XmlElementPrelim::empty(kind));
            push_front(txn, &element, content_of(last));
            write_attributes(txn, &element, last);
            rest = &rest[..rest.len() - 1];
        }
    }
}

/// Writes the letters of a text node at the start of `text`, formatted with its marks.
fn write_run(txn: &mut TransactionMut<'_>, text: &yrs::XmlTextRef, node: &Value) {
    let letters = letters_of(node);
    let marks = formatting_of(node);
    if marks.is_empty() {
        text.insert(txn, 0, letters);
    } else {
        text.insert_with_attributes(txn, 0, letters, marks);
    }
}

/// The marks of a text node as the formatting of its letters: each by its name, with
/// the attributes the schema gives it.
fn formatting_of(node: &Value) -> Attrs {
    let marks = node
        .get("marks")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    marks
        .iter()
        .filter_map(|mark| {
            let name = mark.get("type").and_then(Value::as_str)?;
            let given = mark.get("attrs");
            let attributes: HashMap<String, Any> = schema::mark_attributes(name)
                .iter()
                .filter_map(|attribute| {
                    let value = given?.get(*attribute).filter(|value| !value.is_null())?;
                    Some(((*attribute).to_owned(), any_of(value)))
                })
                .collect();
            Some((Arc::from(name), Any::Map(Arc::new(attributes))))
        })
        .collect()
}
