//! Updates written field by field, in version 1 of the Y update encoding: what a client
//! that is no editor can send. Nothing here goes through the Y library, so an update
//! can say what the library itself would never write.

/// The identifier of a change: the client and its clock.
pub(crate) type Id = (u64, u64);

/// What a change holds.
#[derive(Debug, Clone)]
pub(crate) enum Content {
    /// Kind 1: content that was deleted, by its length.
    Deleted(u64),
    /// Kind 2: texts of JSON. `count` is the number written before them, which need
    /// not be how many there are.
    Json { count: u64, texts: Vec<Vec<u8>> },
    /// Kind 3: bytes.
    Binary(Vec<u8>),
    /// Kind 4: letters, as bytes that need not be UTF-8.
    Text(Vec<u8>),
    /// Kind 5: an embedded value, written as JSON.
    Embed(Vec<u8>),
    /// Kind 6: formatting, a name and a value written as JSON.
    Format(Vec<u8>, Vec<u8>),
    /// Kind 7: a shared type by its number; an XML element (3) has a name.
    Type(u8, Option<Vec<u8>>),
    /// Kind 8: values, each already encoded. `count` is the number written before them.
    Any { count: u64, values: Vec<Vec<u8>> },
    /// Any kind, with the bytes that follow the head of the change.
    Raw(u8, Vec<u8>),
}

impl Content {
    fn kind(&self) -> u8 {
        match self {
            Self::Deleted(_) => 1,
            Self::Json { .. } => 2,
            Self::Binary(_) => 3,
            Self::Text(_) => 4,
            Self::Embed(_) => 5,
            Self::Format(..) => 6,
            Self::Type(..) => 7,
            Self::Any { .. } => 8,
            Self::Raw(kind, _) => *kind,
        }
    }

    /// Letters.
    pub(crate) fn text(letters: &str) -> Self {
        Self::Text(letters.as_bytes().to_vec())
    }

    /// An XML element of a name.
    pub(crate) fn element(name: &str) -> Self {
        Self::Type(3, Some(name.as_bytes().to_vec()))
    }

    /// An XML text (type 6): letters and their marks, within an element.
    pub(crate) fn xml_text() -> Self {
        Self::Type(6, None)
    }

    /// Values, as many as are given.
    pub(crate) fn values(values: Vec<Vec<u8>>) -> Self {
        Self::Any {
            count: u64::try_from(values.len()).expect("a count fits"),
            values,
        }
    }

    /// How far the library moves a client's clock on for this content, if it reads it
    /// as it is meant.
    pub(crate) fn length(&self) -> u64 {
        match self {
            Self::Deleted(length) => *length,
            Self::Json { texts, .. } => u64::try_from(texts.len()).expect("a count fits"),
            Self::Text(bytes) => String::from_utf8_lossy(bytes)
                .encode_utf16()
                .count()
                .try_into()
                .expect("a count fits"),
            Self::Any { values, .. } => u64::try_from(values.len()).expect("a count fits"),
            Self::Binary(_)
            | Self::Embed(_)
            | Self::Format(..)
            | Self::Type(..)
            | Self::Raw(..) => 1,
        }
    }
}

/// Where a change that names no neighbour says it belongs.
#[derive(Debug, Clone)]
pub(crate) enum Parent {
    /// A root, by its name.
    Root(Vec<u8>),
    /// A shared type, by the change that made it.
    Change(Id),
}

impl Parent {
    /// The root that holds the body.
    pub(crate) fn body() -> Self {
        Self::Root(b"default".to_vec())
    }
}

/// One change of a client.
#[derive(Debug, Clone)]
pub(crate) enum Change {
    /// A stretch that was collected: only its length is left.
    Collected(u64),
    /// A stretch that is left out.
    Skipped(u64),
    /// A change that holds something: where it was put, and what it holds.
    Item {
        /// The change to its left when it was made.
        left: Option<Id>,
        /// The change to its right when it was made.
        right: Option<Id>,
        /// Its parent, which is written only where it names no neighbour.
        parent: Parent,
        /// The member of the parent it is the value of.
        key: Option<Vec<u8>>,
        /// What it holds.
        content: Content,
    },
}

impl Change {
    /// An item under a parent, at its end as far as the writer says.
    pub(crate) fn under(parent: Parent, content: Content) -> Self {
        Self::Item {
            left: None,
            right: None,
            parent,
            key: None,
            content,
        }
    }

    /// An item to the right of another.
    pub(crate) fn after(left: Id, content: Content) -> Self {
        Self::Item {
            left: Some(left),
            right: None,
            parent: Parent::body(),
            key: None,
            content,
        }
    }

    /// An item that is the value of a member of its parent.
    pub(crate) fn member(parent: Parent, key: &str, content: Content) -> Self {
        Self::Item {
            left: None,
            right: None,
            parent,
            key: Some(key.as_bytes().to_vec()),
            content,
        }
    }

    /// How far the library moves the client's clock on for this change.
    pub(crate) fn length(&self) -> u64 {
        match self {
            Self::Collected(length) | Self::Skipped(length) => *length,
            Self::Item { content, .. } => content.length(),
        }
    }
}

/// The changes of one client in an update, from a clock on.
#[derive(Debug, Clone)]
pub(crate) struct Run {
    pub(crate) client: u64,
    pub(crate) clock: u64,
    pub(crate) changes: Vec<Change>,
}

/// An update: the changes of clients, and what was deleted.
#[derive(Debug, Clone, Default)]
pub(crate) struct Update {
    pub(crate) runs: Vec<Run>,
    /// For each client, stretches of its clock: where each begins and how long it is.
    pub(crate) deleted: Vec<(u64, Vec<(u64, u64)>)>,
}

impl Update {
    /// The changes of one client and nothing deleted.
    pub(crate) fn of(client: u64, clock: u64, changes: Vec<Change>) -> Self {
        Self {
            runs: vec![Run {
                client,
                clock,
                changes,
            }],
            deleted: Vec::new(),
        }
    }

    /// The bytes of the update.
    pub(crate) fn bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        number(&mut out, count_of(self.runs.len()));
        for run in &self.runs {
            number(&mut out, count_of(run.changes.len()));
            number(&mut out, run.client);
            number(&mut out, run.clock);
            for change in &run.changes {
                change_bytes(&mut out, change);
            }
        }
        number(&mut out, count_of(self.deleted.len()));
        for (client, stretches) in &self.deleted {
            number(&mut out, *client);
            number(&mut out, count_of(stretches.len()));
            for (clock, length) in stretches {
                number(&mut out, *clock);
                number(&mut out, *length);
            }
        }
        out
    }
}

fn count_of(count: usize) -> u64 {
    u64::try_from(count).expect("a count fits")
}

/// A number in the variable-length form: seven bits a byte, the lowest first.
pub(crate) fn number(out: &mut Vec<u8>, mut number: u64) {
    while number >= 0x80 {
        out.push(number.to_le_bytes()[0] | 0x80);
        number >>= 7;
    }
    out.push(number.to_le_bytes()[0]);
}

/// Bytes with their length before them.
pub(crate) fn bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    number(out, count_of(bytes.len()));
    out.extend_from_slice(bytes);
}

fn id(out: &mut Vec<u8>, (client, clock): Id) {
    number(out, client);
    number(out, clock);
}

fn change_bytes(out: &mut Vec<u8>, change: &Change) {
    match change {
        Change::Collected(length) => {
            out.push(0);
            number(out, *length);
        }
        Change::Skipped(length) => {
            out.push(10);
            number(out, *length);
        }
        Change::Item {
            left,
            right,
            parent,
            key,
            content,
        } => {
            let named = left.is_none() && right.is_none();
            let mut info = content.kind();
            if left.is_some() {
                info |= 0x80;
            }
            if right.is_some() {
                info |= 0x40;
            }
            if key.is_some() {
                info |= 0x20;
            }
            out.push(info);
            if let Some(left) = left {
                id(out, *left);
            }
            if let Some(right) = right {
                id(out, *right);
            }
            if named {
                match parent {
                    Parent::Root(name) => {
                        out.push(1);
                        bytes(out, name);
                    }
                    Parent::Change(change) => {
                        out.push(0);
                        id(out, *change);
                    }
                }
                if let Some(key) = key {
                    bytes(out, key);
                }
            }
            content_bytes(out, content);
        }
    }
}

fn content_bytes(out: &mut Vec<u8>, content: &Content) {
    match content {
        Content::Deleted(length) => number(out, *length),
        Content::Json { count, texts } => {
            number(out, *count);
            for text in texts {
                bytes(out, text);
            }
        }
        Content::Binary(given) | Content::Text(given) | Content::Embed(given) => bytes(out, given),
        Content::Format(name, value) => {
            bytes(out, name);
            bytes(out, value);
        }
        Content::Type(kind, name) => {
            out.push(*kind);
            if let Some(name) = name {
                bytes(out, name);
            }
        }
        Content::Any { count, values } => {
            number(out, *count);
            for value in values {
                out.extend_from_slice(value);
            }
        }
        Content::Raw(_, rest) => out.extend_from_slice(rest),
    }
}

/// Values as an update holds them.
pub(crate) mod value {
    /// A string (tag 119): its length, then its letters in UTF-8.
    pub(crate) fn text(letters: &str) -> Vec<u8> {
        let mut out = vec![119];
        super::bytes(&mut out, letters.as_bytes());
        out
    }

    /// A whole number, in the form with a sign and six bits in its first byte.
    pub(crate) fn whole(number: i64) -> Vec<u8> {
        let mut out = vec![125];
        let mut rest = number.unsigned_abs();
        let sign = if number < 0 { 0x40 } else { 0 };
        let more = if rest > 0x3f { 0x80 } else { 0 };
        out.push(more | sign | (rest.to_le_bytes()[0] & 0x3f));
        rest >>= 6;
        while rest > 0 {
            let more = if rest > 0x7f { 0x80 } else { 0 };
            out.push(more | (rest.to_le_bytes()[0] & 0x7f));
            rest >>= 7;
        }
        out
    }

    /// A list (tag 117): how many values it holds, then the values.
    pub(crate) fn list(items: &[Vec<u8>]) -> Vec<u8> {
        let mut out = vec![117];
        super::number(&mut out, super::count_of(items.len()));
        for item in items {
            out.extend_from_slice(item);
        }
        out
    }

    /// A map (tag 118): how many members it has, then each one's key and value.
    pub(crate) fn map(members: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut out = vec![118];
        super::number(&mut out, super::count_of(members.len()));
        for (name, member) in members {
            super::bytes(&mut out, name.as_bytes());
            out.extend_from_slice(member);
        }
        out
    }

    /// Bytes.
    pub(crate) fn buffer(given: &[u8]) -> Vec<u8> {
        let mut out = vec![116];
        super::bytes(&mut out, given);
        out
    }

    /// True.
    pub(crate) fn truth() -> Vec<u8> {
        vec![120]
    }

    /// JSON's `null` (tag 126).
    pub(crate) fn null() -> Vec<u8> {
        vec![126]
    }
}
