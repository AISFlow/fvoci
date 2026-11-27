//! Reading shared content as an editor reads it: the children of an element, the runs of
//! a text with their marks, and the JSON node of an element.

use serde_json::{Map, Value};
use yrs::types::Attrs;
use yrs::types::text::YChange;
use yrs::types::xml::XmlOut;
use yrs::{Any, Number, Out, ReadTxn, Text, Xml, XmlElementRef, XmlFragment, XmlTextRef};

use crate::schema::{self, AtFallback, Attribute, Fallback};

/// What an element or the root holds at one place.
#[derive(Debug)]
pub(crate) enum Child {
    /// A node.
    Element(XmlElementRef),
    /// Text: the neighbouring text nodes of a line, with their marks.
    Text(XmlTextRef),
    /// A fragment inside the document, which no editor writes and none reads.
    Other,
}

/// The children of an element or the root, in order, followed from one to the next:
/// the library's own way of listing a parent's content does not come back from some
/// content no editor writes (a letter outside the basic plane in a string put straight
/// into the parent). Whatever is no shared XML type is passed over, and the walk ends
/// at a fragment inside the document, so a parent that holds either has more in its
/// length than the walk finds ([`holds_only_xml`]). It makes no list.
pub(crate) struct Siblings<'txn, T> {
    txn: &'txn T,
    next: Option<XmlOut>,
}

impl<'txn, T: ReadTxn> Siblings<'txn, T> {
    /// The children of `parent`, from the first.
    pub(crate) fn of(txn: &'txn T, parent: &impl XmlFragment) -> Self {
        Self {
            txn,
            next: parent.first_child(),
        }
    }
}

impl<T: ReadTxn> Iterator for Siblings<'_, T> {
    type Item = Child;

    fn next(&mut self) -> Option<Child> {
        let node = self.next.take()?;
        self.next = match &node {
            XmlOut::Element(element) => element.siblings(self.txn).next(),
            XmlOut::Text(text) => text.siblings(self.txn).next(),
            XmlOut::Fragment(_) => None,
        };
        Some(match node {
            XmlOut::Element(element) => Child::Element(element),
            XmlOut::Text(text) => Child::Text(text),
            XmlOut::Fragment(_) => Child::Other,
        })
    }
}

/// Whether `parent` holds only shared XML types: its length counts no more than
/// [`Siblings`] finds. No editor writes anything else into a parent.
pub(crate) fn holds_only_xml<T: ReadTxn>(txn: &T, parent: &impl XmlFragment) -> bool {
    u32::try_from(Siblings::of(txn, parent).count()).ok() == Some(parent.len(txn))
}

/// The children of an element or the root, in order; the place of each in the list is
/// the index it is deleted by. `None` if the parent holds anything that is no shared
/// XML type ([`holds_only_xml`]).
pub(crate) fn children<T: ReadTxn>(txn: &T, parent: &impl XmlFragment) -> Option<Vec<Child>> {
    let found: Vec<Child> = Siblings::of(txn, parent).collect();
    (u32::try_from(found.len()).ok() == Some(parent.len(txn))).then_some(found)
}

/// The text nodes of a text, as JSON: one per run of letters with the same marks. `None`
/// if the text holds something that is no letter.
pub(crate) fn text_nodes<T: ReadTxn>(txn: &T, text: &XmlTextRef) -> Option<Vec<Value>> {
    text.diff(txn, YChange::identity)
        .into_iter()
        .map(|run| {
            let Out::Any(Any::String(letters)) = run.insert else {
                return None;
            };
            let mut node = Map::new();
            node.insert("type".to_owned(), Value::from("text"));
            let marks = run.attributes.map(|attributes| marks_of(&attributes));
            if let Some(marks) = marks.filter(|marks| !marks.is_empty()) {
                node.insert("marks".to_owned(), Value::Array(marks));
            }
            node.insert("text".to_owned(), Value::from(&*letters));
            Some(Value::Object(node))
        })
        .collect()
}

/// The marks of a run, as JSON, in the order the editor stores them. A mark the schema
/// does not have comes last and keeps its name, for the allowlist to refuse.
fn marks_of(formatting: &Attrs) -> Vec<Value> {
    let mut marks: Vec<(usize, &str, Value)> = formatting
        .iter()
        // A member without a value is a mark that was taken off.
        .filter(|(_, value)| !matches!(value, Any::Null | Any::Undefined))
        .map(|(member, value)| {
            let name = mark_name(member);
            let mut mark = Map::new();
            mark.insert("type".to_owned(), Value::from(name));
            let attrs: Map<String, Value> = match value {
                Any::Map(given) => schema::mark_attributes(name)
                    .iter()
                    .filter_map(|attribute| {
                        let value = stored_value(given.get(*attribute)?)?;
                        Some(((*attribute).to_owned(), value))
                    })
                    .collect(),
                _ => Map::new(),
            };
            if !attrs.is_empty() {
                mark.insert("attrs".to_owned(), Value::Object(attrs));
            }
            let rank = schema::mark_rank(name).unwrap_or(usize::MAX);
            (rank, name, Value::Object(mark))
        })
        .collect();
    marks.sort_by(|one, other| (one.0, one.1).cmp(&(other.0, other.1)));
    marks.into_iter().map(|(_, _, mark)| mark).collect()
}

/// The name of the mark a formatting member stands for. The binding names a mark that
/// may lie over another of its kind `name--hash`, the hash being eight letters of
/// base64; the mark is the name before it. Any member containing control characters or
/// Unicode line terminators (`U+2028`, `U+2029`) is never stripped so a newline before
/// `--hash` cannot disguise an invalid name as a known mark.
fn mark_name(member: &str) -> &str {
    const HASH_LENGTH: usize = 8;
    if member
        .chars()
        .any(|ch| ch.is_control() || matches!(ch, '\u{2028}' | '\u{2029}'))
    {
        return member;
    }
    let Some(split) = member.len().checked_sub(HASH_LENGTH + 2) else {
        return member;
    };
    match (member.get(..split), member.get(split..)) {
        (Some(name), Some(suffix))
            if suffix.starts_with("--")
                && suffix.bytes().skip(2).all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'=')
                }) =>
        {
            name
        }
        _ => member,
    }
}

/// The JSON node of an element whose content has been read already: its type, the
/// attributes the schema has for it, and the content. An attribute the shared content
/// does not have gets the value an editor gives it, unless the stored form leaves that
/// value out (the start of an ordered list at 1); one the schema does not have is left
/// out, as an editor does not read it.
pub(crate) fn element_node<T: ReadTxn>(
    txn: &T,
    element: &XmlElementRef,
    content: Vec<Value>,
) -> Value {
    let kind: &str = element.tag();
    let mut node = Map::new();
    node.insert("type".to_owned(), Value::from(kind));
    let mut attrs = Map::new();
    for attribute in schema::node_attributes(kind).unwrap_or(&[]) {
        let value = match element.get_attribute(txn, attribute.name) {
            // Written as undefined: as good as not written.
            None | Some(Out::Any(Any::Undefined)) => match attribute.fallback {
                Fallback::None => None,
                Fallback::Integer(number) => Some(Value::from(number)),
                Fallback::Boolean(truth) => Some(Value::from(truth)),
            },
            Some(Out::Any(value)) => stored_value(&value),
            // A shared type as the value of an attribute: nothing an attribute may be.
            Some(_) => Some(Value::Array(Vec::new())),
        };
        if let Some(value) = value.filter(|value| !is_left_out(*attribute, value)) {
            attrs.insert(attribute.name.to_owned(), value);
        }
    }
    if !attrs.is_empty() {
        node.insert("attrs".to_owned(), Value::Object(attrs));
    }
    if !content.is_empty() {
        node.insert("content".to_owned(), Value::Array(content));
    }
    Value::Object(node)
}

/// Whether the stored form leaves out an attribute of this value: its fallback, where
/// that is left out.
fn is_left_out(attribute: Attribute, value: &Value) -> bool {
    attribute.at_fallback == AtFallback::LeftOut
        && match attribute.fallback {
            Fallback::None => false,
            Fallback::Integer(number) => value.as_i64() == Some(number),
            Fallback::Boolean(truth) => value.as_bool() == Some(truth),
        }
}

/// A value of the shared content as it is stored in JSON; `None` for no value, which is
/// stored by leaving the member out.
fn stored_value(value: &Any) -> Option<Value> {
    match value {
        Any::Null | Any::Undefined => None,
        other => Some(json_of(other)),
    }
}

/// A value of the shared content as JSON. What JSON cannot hold (bytes, a number that is
/// no number) becomes an empty list, which no attribute may be.
fn json_of(value: &Any) -> Value {
    match value {
        Any::Null | Any::Undefined => Value::Null,
        Any::Bool(truth) => Value::from(*truth),
        Any::Number(Number::Int(number)) => Value::from(*number),
        Any::Number(Number::Float(number)) => match Number::try_i64(*number) {
            // JavaScript has one kind of number: 2.0 is stored as 2.
            Number::Int(whole) => Value::from(whole),
            Number::Float(fraction) => {
                serde_json::Number::from_f64(fraction).map_or_else(nothing_allowed, Value::Number)
            }
        },
        Any::String(text) => Value::from(&**text),
        Any::Buffer(_) => nothing_allowed(),
        Any::Array(items) => Value::Array(items.iter().map(json_of).collect()),
        Any::Map(members) => Value::Object(
            members
                .iter()
                .map(|(name, member)| (name.clone(), json_of(member)))
                .collect(),
        ),
    }
}

/// A value that no rule of the allowlist takes.
fn nothing_allowed() -> Value {
    Value::Array(Vec::new())
}

/// What became of a node that was added to a parent's content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pushed {
    /// It is a node of its own.
    New,
    /// It was text and joined the text before it.
    Joined,
}

/// The nodes of one parent as they are stored: neighbouring text nodes with the same
/// marks are one, as an editor joins them.
#[derive(Debug, Default)]
pub(crate) struct Content {
    nodes: Vec<Value>,
}

impl Content {
    /// Adds a node; a text node joins the one before it if their marks are the same.
    pub(crate) fn push(&mut self, node: Value) -> Pushed {
        if let Some(letters) = text_of(&node)
            && let Some(last) = self.nodes.last_mut()
            && text_of(last).is_some()
            && last.get("marks") == node.get("marks")
            && let Some(Value::String(joined)) = last.get_mut("text")
        {
            joined.push_str(letters);
            return Pushed::Joined;
        }
        self.nodes.push(node);
        Pushed::New
    }

    /// How many nodes there are.
    pub(crate) fn len(&self) -> usize {
        self.nodes.len()
    }

    /// The nodes, the texts of the same marks next to each other joined.
    pub(crate) fn into_nodes(self) -> Vec<Value> {
        self.nodes
    }
}

/// The letters of a text node; `None` for any other node.
fn text_of(node: &Value) -> Option<&str> {
    (node.get("type").and_then(Value::as_str) == Some("text"))
        .then(|| node.get("text").and_then(Value::as_str))
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_member_named_with_a_hash_is_the_mark_before_it() {
        for (member, mark) in [
            ("bold", "bold"),
            ("bold--abcdefgh", "bold"),
            ("italic--AB+/=012", "italic"),
            ("a--b--abcdefgh", "a--b"),
            // Not eight letters of base64 behind the two hyphens: a name of its own.
            ("bold--abcdefg", "bold--abcdefg"),
            ("bold--abcdefghi", "bold--abcdefghi"),
            ("bold--abcd?fgh", "bold--abcd?fgh"),
            ("bold-abcdefgh", "bold-abcdefgh"),
            ("--abcdefgh", ""),
            ("볼드--abcdefgh", "볼드"),
            ("b--한글abcd", "b--한글abcd"),
            ("x\nlink--abcdefgh", "x\nlink--abcdefgh"),
            ("x\rlink--abcdefgh", "x\rlink--abcdefgh"),
            ("x\u{2028}link--abcdefgh", "x\u{2028}link--abcdefgh"),
            ("x\u{2029}link--abcdefgh", "x\u{2029}link--abcdefgh"),
            ("", ""),
        ] {
            assert_eq!(mark_name(member), mark, "{member:?}");
        }
    }

    #[test]
    fn numbers_are_stored_as_javascript_writes_them() {
        let two = Any::Number(Number::Float(2.0));
        assert_eq!(json_of(&two).to_string(), "2");
        let half = Any::Number(Number::Float(2.5));
        assert_eq!(json_of(&half).to_string(), "2.5");
        let not_a_number = Any::Number(Number::Float(f64::NAN));
        assert_eq!(json_of(&not_a_number), nothing_allowed());
    }
}
