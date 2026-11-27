//! The carets of a body edited together, as their frames carry them
//! (`contracts/websocket.md`): an awareness update of y-protocols.
//!
//! ```text
//! varuint count    how many entries follow
//! varuint client   whose state the entry is
//! varuint clock    which telling of that state: a later one takes an earlier one's place
//! varuint length   of what follows
//! bytes            the state as JSON, in UTF-8; `null` for a state that is gone
//! ```
//!
//! What a client sends is read for one thing: where its caret is. Whose the caret is,
//! what number it is known under and which telling it is are the server's to say
//! ([`told`]): nothing a client wrote of those is sent on.

use bytes::{BufMut as _, Bytes, BytesMut};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::frame::{put_varuint, take_varuint};

/// The most bytes of an awareness update a client sends that is read: a caret is two
/// places in the document, a few hundred bytes with whatever else the client's
/// library says of itself.
pub(super) const MOST_BYTES: usize = 1024;

/// The largest number of a client or a clock of a Y document that is sent on: the
/// largest whole number a browser counts with.
const MOST_NUMBER: u64 = (1 << 53) - 1;

/// What a client's awareness state is read for: its caret. Whatever else it says (a
/// name, a colour, a user) is passed over unread, be it what no reader would keep.
#[derive(Debug, Deserialize)]
struct State {
    #[serde(default)]
    cursor: Option<Value>,
}

/// A caret: the two ends of a selection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct Cursor {
    anchor: Position,
    head: Position,
}

/// A relative position of Yjs as its JSON: the item it is next to, or the type it is
/// in. What it does not say is left out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct Position {
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    kind: Option<Id>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tname: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    item: Option<Id>,
    #[serde(skip_serializing_if = "Option::is_none")]
    assoc: Option<i8>,
}

/// The identifier of an item of a Y document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
struct Id {
    client: u64,
    clock: u64,
}

/// The members of `value`, if it is an object that has none but those of `known`.
fn members<'object>(value: &'object Value, known: &[&str]) -> Option<&'object Map<String, Value>> {
    let object = value.as_object()?;
    object
        .keys()
        .all(|key| known.contains(&key.as_str()))
        .then_some(object)
}

/// What a place says that is no place.
struct NoPlace;

/// A member that may be left out, or `null`: read with `read` if it says something.
fn optional<T>(
    object: &Map<String, Value>,
    name: &str,
    read: impl FnOnce(&Value) -> Option<T>,
) -> Result<Option<T>, NoPlace> {
    match object.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => read(value).map(Some).ok_or(NoPlace),
    }
}

impl Id {
    fn read(value: &Value) -> Option<Self> {
        let object = members(value, &["client", "clock"])?;
        let number = |name: &str| object.get(name)?.as_u64().filter(|n| *n <= MOST_NUMBER);
        Some(Self {
            client: number("client")?,
            clock: number("clock")?,
        })
    }
}

impl Position {
    fn read(value: &Value) -> Option<Self> {
        let object = members(value, &["type", "tname", "item", "assoc"])?;
        // A root type other than the body's is none of the document: named, the
        // editor's library would make it in the document of whoever is told.
        let name = |value: &Value| {
            let name = value.as_str()?;
            (name == fvoci_collab::FIELD).then(|| name.to_owned())
        };
        let side = |value: &Value| i8::try_from(value.as_i64()?).ok();
        let place = Self {
            kind: optional(object, "type", Id::read).ok()?,
            tname: optional(object, "tname", name).ok()?,
            item: optional(object, "item", Id::read).ok()?,
            assoc: optional(object, "assoc", side).ok()?,
        };
        // A place that names nothing is none: the editor's library cannot find it.
        (place.kind.is_some() || place.tname.is_some() || place.item.is_some()).then_some(place)
    }
}

impl Cursor {
    fn read(value: &Value) -> Option<Self> {
        let object = members(value, &["anchor", "head"])?;
        Some(Self {
            anchor: Position::read(object.get("anchor")?)?,
            head: Position::read(object.get("head")?)?,
        })
    }
}

/// What a client said of its caret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Said {
    /// Where it is.
    Shown(Cursor),
    /// It has none (any more): its state is gone, or names no caret.
    Withdrawn,
}

/// Reads what a client said of its caret from the awareness update it sent. `None` for
/// what is no update of exactly one state to its last byte, an object or `null` as an
/// editor's library writes it, and for a caret that is not two places of the body.
pub(super) fn said(update: Bytes) -> Option<Said> {
    let mut rest = update;
    if take_varuint(&mut rest)? != 1 {
        return None;
    }
    // Whose it says it is, and which telling: the server's to say, not read.
    take_varuint(&mut rest)?;
    take_varuint(&mut rest)?;
    let length = usize::try_from(take_varuint(&mut rest)?).ok()?;
    if rest.len() != length {
        return None;
    }
    let cursor = match rest.first() {
        Some(b'{') => serde_json::from_slice::<State>(&rest).ok()?.cursor,
        _ if rest.as_ref() == b"null" => None,
        _ => return None,
    };
    match cursor {
        Some(cursor) => Cursor::read(&cursor).map(Said::Shown),
        None => Some(Said::Withdrawn),
    }
}

/// The awareness state the others are told for a caret of `user_id`: whose it is, as
/// the server knows it, and where.
pub(super) fn state_of(user_id: &str, cursor: &Cursor) -> String {
    #[derive(Serialize)]
    struct Owner<'user> {
        id: &'user str,
    }
    #[derive(Serialize)]
    struct Told<'caret> {
        user: Owner<'caret>,
        cursor: &'caret Cursor,
    }
    let told = Told {
        user: Owner { id: user_id },
        cursor,
    };
    serde_json::to_string(&told).expect("a caret is strings and numbers, which JSON has")
}

/// The awareness update that tells a caret: the number the server knows it under,
/// which telling of it this is, and its state (`None`: it is gone).
pub(super) fn told(caret: u64, clock: u64, state: Option<&str>) -> Bytes {
    let state = state.unwrap_or("null");
    let mut out = BytesMut::with_capacity(state.len() + 24);
    put_varuint(&mut out, 1);
    put_varuint(&mut out, caret);
    put_varuint(&mut out, clock);
    put_varuint(&mut out, u64::try_from(state.len()).unwrap_or(u64::MAX));
    out.put_slice(state.as_bytes());
    out.freeze()
}

#[cfg(test)]
mod tests {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;
    use serde::Deserialize;

    use super::*;

    /// `contracts/caret-vectors.json`: carets as the editor's libraries write them,
    /// and what the server tells of each.
    #[derive(Deserialize)]
    struct Vectors {
        cases: Vec<Case>,
        refused: Vec<Refused>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Case {
        name: String,
        said: String,
        user_id: String,
        number: u64,
        clock: u64,
        state: Option<String>,
        told: String,
    }

    #[derive(Deserialize)]
    struct Refused {
        name: String,
        said: String,
    }

    fn vectors() -> Vectors {
        let file = include_str!("../../../../contracts/caret-vectors.json");
        serde_json::from_str(file).expect("the vectors are JSON of this shape")
    }

    fn bytes(base64: &str) -> Bytes {
        Bytes::from(STANDARD.decode(base64).expect("base64"))
    }

    #[test]
    fn a_caret_is_told_as_the_vectors_have_it() {
        let vectors = vectors();
        assert!(vectors.cases.len() >= 8, "the cases are there");
        for case in vectors.cases {
            let said = said(bytes(&case.said)).unwrap_or_else(|| panic!("{}: read", case.name));
            let state = match &said {
                Said::Shown(cursor) => Some(state_of(&case.user_id, cursor)),
                Said::Withdrawn => None,
            };
            assert_eq!(state, case.state, "{}", case.name);
            assert_eq!(
                told(case.number, case.clock, state.as_deref()),
                bytes(&case.told),
                "{}",
                case.name
            );
        }
    }

    #[test]
    fn what_is_no_caret_is_not_read() {
        let vectors = vectors();
        assert!(vectors.refused.len() >= 13, "the refused are there");
        for refused in vectors.refused {
            assert_eq!(said(bytes(&refused.said)), None, "{}", refused.name);
        }
    }
}
