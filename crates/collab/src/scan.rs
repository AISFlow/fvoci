//! A look through the bytes of an update before the Y library reads them.
//!
//! The library (yrs 0.28.2) reads a value that is a list in a list in a list by calling
//! itself once a level, without a limit: an update of twenty thousand bytes that nests
//! a value ten thousand deep ends the stack of the thread that reads it, and with it
//! the process, which no handler can catch. It also sets aside a table for as many
//! clients as the first bytes name before it reads on, so that five bytes naming four
//! thousand million clients cost it seconds and gigabytes. And where a clock runs past
//! 32 bits or a client identifier past 53, it wraps the number around, or panics in a
//! build that checks its arithmetic.
//!
//! So an update is walked here first, in the same framing the library reads (version 1
//! of the Y update encoding), and refused if a value in it nests deeper than any
//! attribute of a body does, if it names more than it has bytes for, or if it holds
//! anything this walk does not know: where the walk and the library could read the
//! same bytes differently, the update does not reach the library. Nor does what the
//! library takes and then writes in a form it does not read again (texts of JSON, a
//! whole number past 53 bits): the document that took it could not be stored. A
//! body's update needs none of what is refused (a subdocument, a kind of content from
//! another version of the encoding, a hole in a client's changes, a clock past 31
//! bits, a number JavaScript does not keep).

use std::collections::{HashMap, HashSet};

/// How deep a value in an update may nest. The values of a body are attributes:
/// numbers, truths and texts, which do not nest at all.
const MAX_VALUE_DEPTH: usize = 8;

/// The largest client identifier: 53 bits, the whole numbers JavaScript keeps exactly.
const MAX_CLIENT: u64 = (1 << 53) - 1;

const MAX_U32: u64 = 0xffff_ffff;

/// The largest whole number of a value: 53 bits, as for a client.
const MAX_SAFE_WHOLE: u64 = MAX_CLIENT;

/// The furthest a clock may run. The library counts some distances between clocks as
/// signed numbers of 32 bits.
const MAX_CLOCK: u64 = 0x7fff_ffff;

/// How often a member of one node (an attribute) may be written within a single
/// update without naming the value it replaces. An editor names it on every write
/// but the first; only editors that write the first value at once write it so more
/// than once. The library places each such write by walking every value the member
/// had before, which a document keeps, as it is made without garbage collection:
/// written over and over in one update (while across updates `MAX_CLIENT_CHANGES`,
/// `MAX_TOTAL_CHANGES` and `READING_STEPS` cap total growth), a member would cost
/// every later reading of the document the square of its writes.
const MAX_FIRST_WRITES: u32 = 8;

/// Why an update is not handed to the library.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// The bytes end before what they announce.
    CutShort,
    /// A number is longer or larger than the field it is in takes.
    Number,
    /// It names more clients, changes or values than it has bytes for.
    Count,
    /// A value nests deeper than [`MAX_VALUE_DEPTH`].
    TooDeep,
    /// It holds a subdocument, or content of a kind version 1 of the encoding lacks.
    Unknown,
    /// It leaves a stretch of a client's clock out, or gives a client's changes twice.
    Gap,
    /// It writes a member of the root, or a member of a node more often than
    /// [`MAX_FIRST_WRITES`] without the value that it replaces.
    Member,
    /// A change names neighbours or a parent that do not belong together (itself,
    /// different parents or members, a non-type parent, or inverted inside one change).
    Origin,
    /// The library read its changes to another length than this walk.
    Lengths,
}

impl Refusal {
    pub(crate) fn reason(self) -> &'static str {
        match self {
            Self::CutShort => "it ends before what it announces",
            Self::Number => "a number in it is longer or larger than its field takes",
            Self::Count => "it names more than it has bytes for",
            Self::TooDeep => "a value in it nests too deep",
            Self::Unknown => "it holds content no body has",
            Self::Gap => "it leaves a gap in a client's changes",
            Self::Member => "it writes a member no editor writes so",
            Self::Origin => "a change names neighbours that do not belong together",
            Self::Lengths => "its changes are not as long as they are written",
        }
    }
}

/// The identifier of a change: the client and its clock.
pub(crate) type Id = (u64, u64);

/// Where a change that names no neighbour says it belongs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Parent<'bytes> {
    /// A root, by its name.
    Root(&'bytes [u8]),
    /// A shared type, by the change that made it.
    Change(Id),
}

/// What a change holds, as far as its length tells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Content<'bytes> {
    /// A stretch that was collected: no item, only its length.
    Collected(u64),
    /// Deleted content, by its length.
    Deleted(u64),
    /// Letters, as UTF-8.
    Letters(&'bytes [u8]),
    /// Values, as many.
    Values(u64),
    /// A shared type (element, text, array, map, fragment), one place long.
    Type,
    /// What is one place long and is no shared type: bytes, an embedded value, formatting.
    One,
}

impl Content<'_> {
    /// How far the library moves the client's clock on for it: letters count in
    /// UTF-16 code units, values each for one.
    pub(crate) fn length(&self) -> u64 {
        match *self {
            Self::Collected(length) | Self::Deleted(length) | Self::Values(length) => length,
            Self::Letters(letters) => letters
                .iter()
                // Every character begins with one byte that continues none, and one
                // of four bytes is two code units.
                .map(|byte| u64::from(byte & 0xc0 != 0x80) + u64::from(*byte >= 0xf0))
                .sum(),
            Self::Type | Self::One => 1,
        }
    }

    /// The length it says it has instead of holding what it is long of.
    fn said(&self) -> u64 {
        match *self {
            Self::Collected(length) | Self::Deleted(length) => length,
            Self::Letters(_) | Self::Values(_) | Self::Type | Self::One => 0,
        }
    }
}

/// One change of an update, as the walk reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Change<'bytes> {
    /// The change to its left when it was made, if it names one.
    pub(crate) left: Option<Id>,
    /// The change to its right when it was made, if it names one.
    pub(crate) right: Option<Id>,
    /// Where it belongs, which is written only where it names no neighbour.
    pub(crate) parent: Option<Parent<'bytes>>,
    /// The member of its parent it is a value of, written where the parent is.
    pub(crate) member: Option<&'bytes [u8]>,
    /// What it holds.
    pub(crate) content: Content<'bytes>,
}

/// What a walk found of an update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Walked {
    /// The clock at which each client's changes in it begin.
    pub(crate) begins: Vec<Id>,
    /// How many changes it holds.
    pub(crate) changes: u64,
    /// The most changes it holds of one client.
    pub(crate) most: u64,
}

/// Walks an update, and gives the clock at which each client's changes in it begin.
///
/// # Errors
///
/// Returns why the update must not be handed to the Y library.
pub(crate) fn check(update: &[u8]) -> Result<Walked, Refusal> {
    walk(update, |_, _| {})
}

/// Walks an update as [`check`] does, and hands `each` every change with the
/// identifier of its first place.
///
/// # Errors
///
/// As [`check`].
pub(crate) fn walk<'bytes>(
    update: &'bytes [u8],
    mut each: impl FnMut(Id, &Change<'bytes>),
) -> Result<Walked, Refusal> {
    walk_detailed(update, |id, change, _| each(id, change), |_, _, _| {})
}

/// Walks an update as [`walk`] does, also passing each change's exact wire byte length
/// and each `(client, start, len)` range of the delete set.
///
/// # Errors
///
/// As [`check`].
pub(crate) fn walk_detailed<'bytes>(
    update: &'bytes [u8],
    mut each_change: impl FnMut(Id, &Change<'bytes>, usize),
    mut each_delete: impl FnMut(u64, u32, u32),
) -> Result<Walked, Refusal> {
    let mut walk = Walk {
        bytes: update,
        at: 0,
        first_writes: HashMap::new(),
    };
    let length = u64::try_from(update.len()).unwrap_or(u64::MAX);
    let mut begins: Vec<Id> = Vec::new();
    // The clients seen: a state names one for each that wrote, tens of thousands in a
    // document edited for long, so that looking through `begins` for each would take
    // the square of that.
    let mut clients: HashSet<u64> = HashSet::new();
    let mut count = 0_u64;
    let mut most = 0_u64;
    for _ in 0..walk.count()? {
        let changes = walk.count()?;
        let client = walk.number(MAX_CLIENT)?;
        // The clock the changes begin at. Each change moves it on by its length: one
        // that says its length (`long`), or one that is no longer than its bytes.
        let clock = walk.number(MAX_CLOCK)?;
        let mut long = 0_u64;
        let mut at = clock;
        for _ in 0..changes {
            let start = walk.at;
            let change = walk.change()?;
            let wire_bytes = walk.at - start;
            each_change((client, at), &change, wire_bytes);
            long = long.saturating_add(change.content.said());
            at = at.saturating_add(change.content.length());
        }
        if clock.saturating_add(long).saturating_add(length) > MAX_CLOCK {
            return Err(Refusal::Number);
        }
        if !clients.insert(client) {
            return Err(Refusal::Gap);
        }
        begins.push((client, clock));
        count = count.saturating_add(changes);
        most = most.max(changes);
    }
    // What was deleted: for each client, stretches of its clock.
    let mut deleters: HashSet<u64> = HashSet::new();
    for _ in 0..walk.count()? {
        let del_client = walk.number(MAX_CLIENT)?;
        if !deleters.insert(del_client) {
            return Err(Refusal::Gap);
        }
        for _ in 0..walk.count()? {
            let clock = walk.number(MAX_CLOCK)?;
            let stretch = walk.number(MAX_CLOCK)?;
            if clock + stretch > MAX_CLOCK {
                return Err(Refusal::Number);
            }
            each_delete(
                del_client,
                u32::try_from(clock).unwrap_or(u32::MAX),
                u32::try_from(stretch).unwrap_or(u32::MAX),
            );
        }
    }
    Ok(Walked {
        begins,
        changes: count,
        most,
    })
}

/// Whether a change names a place inside `[begin, end)` as a neighbour or as its parent.
pub(crate) fn names_self(client: u64, begin: u64, end: u64, change: &Change<'_>) -> bool {
    let inside =
        |(other_client, clock): Id| other_client == client && (begin..end).contains(&clock);
    change.left.is_some_and(inside)
        || change.right.is_some_and(inside)
        || matches!(change.parent, Some(Parent::Change(id)) if inside(id))
}

struct Walk<'bytes> {
    bytes: &'bytes [u8],
    at: usize,
    /// How often each member of a node was written without the value it replaces, by
    /// the node's identifier and the member's name.
    first_writes: HashMap<((u64, u64), &'bytes [u8]), u32>,
}

/// A list or a map that is being read: how many of its entries have not begun.
struct Open {
    left: u64,
    map: bool,
}

impl<'bytes> Walk<'bytes> {
    fn byte(&mut self) -> Result<u8, Refusal> {
        let byte = *self.bytes.get(self.at).ok_or(Refusal::CutShort)?;
        self.at += 1;
        Ok(byte)
    }

    fn skip(&mut self, length: u64) -> Result<(), Refusal> {
        let length = usize::try_from(length).map_err(|_| Refusal::CutShort)?;
        let end = self.at.checked_add(length).ok_or(Refusal::CutShort)?;
        if end > self.bytes.len() {
            return Err(Refusal::CutShort);
        }
        self.at = end;
        Ok(())
    }

    /// A number in the variable-length form: seven bits a byte, the lowest first, the
    /// high bit set on every byte but the last. One that is past `max` is refused: the
    /// library would wrap it around and go on from another number than this walk.
    fn number(&mut self, max: u64) -> Result<u64, Refusal> {
        let mut number = 0_u64;
        for shift in (0..=56).step_by(7) {
            let byte = self.byte()?;
            number |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return if number <= max {
                    Ok(number)
                } else {
                    Err(Refusal::Number)
                };
            }
        }
        Err(Refusal::Number)
    }

    /// How many of something follow. Each takes a byte at least.
    fn count(&mut self) -> Result<u64, Refusal> {
        let count = self.number(MAX_U32)?;
        let left = u64::try_from(self.bytes.len() - self.at).unwrap_or(u64::MAX);
        if count > left {
            return Err(Refusal::Count);
        }
        Ok(count)
    }

    /// A text or bytes: the length, then that many bytes, which it gives.
    fn text(&mut self) -> Result<&'bytes [u8], Refusal> {
        let length = self.number(MAX_U32)?;
        let from = self.at;
        self.skip(length)?;
        Ok(&self.bytes[from..self.at])
    }

    /// The identifier of a change: the client and its clock.
    fn id(&mut self) -> Result<Id, Refusal> {
        Ok((self.number(MAX_CLIENT)?, self.number(MAX_U32)?))
    }

    /// One change: a struct of the update.
    fn change(&mut self) -> Result<Change<'bytes>, Refusal> {
        const SKIPPED: u8 = 10;
        const COLLECTED: u8 = 0;
        let info = self.byte()?;
        let mut change = Change {
            left: None,
            right: None,
            parent: None,
            member: None,
            content: Content::One,
        };
        match info {
            // A stretch that is left out: what a merge of updates writes for changes it
            // does not have. A client sends its own changes, without a hole.
            SKIPPED => return Err(Refusal::Gap),
            COLLECTED => {
                change.content = Content::Collected(self.number(MAX_CLOCK)?);
                return Ok(change);
            }
            _ => {}
        }
        if info & 0x80 != 0 {
            change.left = Some(self.id()?);
        }
        if info & 0x40 != 0 {
            change.right = Some(self.id()?);
        }
        if change.left.is_none() && change.right.is_none() {
            // The parent: a root by its name, or a change by its identifier; and the
            // member of it that is written, if it is one.
            let parent = if self.number(MAX_U32)? == 1 {
                Parent::Root(self.text()?)
            } else {
                Parent::Change(self.id()?)
            };
            change.parent = Some(parent);
            if info & 0x20 != 0 {
                let member = self.text()?;
                let Parent::Change(node) = parent else {
                    return Err(Refusal::Member);
                };
                let writes = self.first_writes.entry((node, member)).or_insert(0);
                *writes += 1;
                if *writes > MAX_FIRST_WRITES {
                    return Err(Refusal::Member);
                }
                change.member = Some(member);
            }
        }
        change.content = self.content(info & 0x0f)?;
        Ok(change)
    }

    /// What a change of a kind holds, read to its end.
    fn content(&mut self, kind: u8) -> Result<Content<'bytes>, Refusal> {
        match kind {
            // Deleted content: its length.
            1 => Ok(Content::Deleted(self.number(MAX_CLOCK)?)),
            // Bytes, and an embedded value written as JSON.
            3 | 5 => {
                self.text()?;
                Ok(Content::One)
            }
            // A text.
            4 => Ok(Content::Letters(self.text()?)),
            // Formatting: the name and the value written as JSON. A mark name with a
            // control character or line terminator is refused outright so server and
            // editor regexes never split `name--hash` differently.
            6 => {
                let name = self.text()?;
                if has_forbidden_mark_char(name) {
                    return Err(Refusal::Member);
                }
                self.text()?;
                Ok(Content::One)
            }
            // A shared type; an XML element has its name. An XML hook (5), which the
            // library writes without the name that the editors' library reads after
            // it, and a type of no kind (15), which the editors' library has not, would
            // leave a document no browser can open.
            7 => match self.byte()? {
                3 => {
                    self.text()?;
                    Ok(Content::Type)
                }
                0..=2 | 4 | 6 => Ok(Content::Type),
                _ => Err(Refusal::Unknown),
            },
            // Values.
            8 => {
                let count = self.count()?;
                for _ in 0..count {
                    self.value()?;
                }
                Ok(Content::Values(count))
            }
            // Texts of JSON (2), a kind of content of older versions of the encoding
            // that no editor writes: the library reads one text more than the number
            // before them says and writes as many as it says, so that a document that
            // took them could not be read again. A subdocument (9), and what version 1
            // of the encoding does not have.
            _ => Err(Refusal::Unknown),
        }
    }

    /// A whole number of a value: the sign and six bits, then seven bits a byte.
    ///
    /// One past 53 bits is refused. JavaScript does not keep it, and the library, which
    /// reads it, writes it back as a float where a float holds it exactly and otherwise
    /// as a number of 64 bits, which this walk refuses: a document that took it could
    /// not be read again. Every whole number up to 53 bits is written back in a form
    /// this walk reads. Nine bytes hold 62
    /// bits; the library reads more, and a tenth could spell the one number that has
    /// no negative, which it negates, so the number is refused by its length as well.
    fn whole_number(&mut self) -> Result<(), Refusal> {
        const MAX_BYTES: u32 = 9;
        let first = self.byte()?;
        let mut magnitude = u64::from(first & 0x3f);
        let mut more = first & 0x80 != 0;
        let mut shift = 6;
        for _ in 1..MAX_BYTES {
            if !more {
                break;
            }
            let byte = self.byte()?;
            magnitude |= u64::from(byte & 0x7f) << shift;
            more = byte & 0x80 != 0;
            shift += 7;
        }
        if more || magnitude > MAX_SAFE_WHOLE {
            return Err(Refusal::Number);
        }
        Ok(())
    }

    /// One value, with all it holds, read without a call a level.
    fn value(&mut self) -> Result<(), Refusal> {
        let mut open: Vec<Open> = Vec::new();
        loop {
            match self.byte()? {
                // Undefined, null, false and true.
                120 | 121 | 126 | 127 => {}
                125 => self.whole_number()?,
                124 => self.skip(4)?,
                123 => self.skip(8)?,
                // Bytes and a text.
                116 | 119 => {
                    self.text()?;
                }
                tag @ (117 | 118) => {
                    if open.len() >= MAX_VALUE_DEPTH {
                        return Err(Refusal::TooDeep);
                    }
                    let left = self.count()?;
                    open.push(Open {
                        left,
                        map: tag == 118,
                    });
                }
                // A whole number of 64 bits (122), which JavaScript reads as another
                // kind of number than the library and no editor writes, or no value.
                _ => return Err(Refusal::Unknown),
            }
            // What was read ends every list it was the last entry of; the next entry
            // of the innermost list that has one begins.
            loop {
                let Some(innermost) = open.last_mut() else {
                    return Ok(());
                };
                if innermost.left == 0 {
                    open.pop();
                    continue;
                }
                innermost.left -= 1;
                if innermost.map {
                    self.text()?;
                }
                break;
            }
        }
    }
}

/// Whether a formatting mark key contains invalid UTF-8, a control character, or a
/// Unicode line terminator (`U+2028`, `U+2029`) that JavaScript `.` does not match.
fn has_forbidden_mark_char(bytes: &[u8]) -> bool {
    let Ok(text) = str::from_utf8(bytes) else {
        return true;
    };
    text.chars()
        .any(|ch| ch.is_control() || matches!(ch, '\u{2028}' | '\u{2029}'))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An update of one change by client 5 under the root "default": values.
    fn of_values(values: &[u8], count: u8) -> Vec<u8> {
        let mut bytes = vec![1, 1, 5, 0, 8, 1, 7];
        bytes.extend_from_slice(b"default");
        bytes.push(count);
        bytes.extend_from_slice(values);
        bytes.push(0);
        bytes
    }

    fn begins(update: &[u8]) -> Result<Vec<Id>, Refusal> {
        check(update).map(|walked| walked.begins)
    }

    fn nested(depth: usize) -> Vec<u8> {
        let mut value = [117, 1].repeat(depth);
        value.push(126);
        of_values(&value, 1)
    }

    #[test]
    fn a_value_may_nest_a_little_and_no_deeper() {
        assert_eq!(begins(&nested(0)), Ok(vec![(5, 0)]));
        assert!(check(&nested(MAX_VALUE_DEPTH)).is_ok(), "at the limit");
        assert_eq!(check(&nested(MAX_VALUE_DEPTH + 1)), Err(Refusal::TooDeep));
        assert_eq!(check(&nested(100_000)), Err(Refusal::TooDeep));
        // Deep in a map, behind entries that are not deep.
        let mut value = vec![118, 2, 1, b'a', 119, 2, b'u', b'u', 1, b'b'];
        value.extend([117, 1].repeat(MAX_VALUE_DEPTH));
        value.push(120);
        assert_eq!(check(&of_values(&value, 1)), Err(Refusal::TooDeep));
    }

    /// A whole number of a value in the form `125` takes: the sign, then the
    /// magnitude six bits and seven bits a byte.
    fn whole(number: i64) -> Vec<u8> {
        let magnitude = number.unsigned_abs();
        let sign = if number < 0 { 0x40 } else { 0 };
        let low = u8::try_from(magnitude & 0x3f).expect("six bits");
        let mut rest = magnitude >> 6;
        let mut bytes = vec![125, low | sign | if rest > 0 { 0x80 } else { 0 }];
        while rest > 0 {
            let seven = u8::try_from(rest & 0x7f).expect("seven bits");
            rest >>= 7;
            bytes.push(seven | if rest > 0 { 0x80 } else { 0 });
        }
        bytes
    }

    #[test]
    fn a_whole_number_is_taken_up_to_53_bits() {
        let safe = i64::try_from(MAX_SAFE_WHOLE).expect("53 bits");
        for sign in [1, -1] {
            for magnitude in [0, 63, 64, 0x7fff_ffff, safe] {
                let value = whole(sign * magnitude);
                assert!(check(&of_values(&value, 1)).is_ok(), "{value:?}");
            }
            for magnitude in [safe + 1, (1 << 62) - 1] {
                let value = whole(sign * magnitude);
                assert_eq!(
                    check(&of_values(&value, 1)),
                    Err(Refusal::Number),
                    "{value:?}"
                );
            }
        }
        // Zero written long, in all the bytes there are and in one more.
        let mut long = vec![125, 0x80];
        long.extend([0x80; 7]);
        long.push(0);
        assert!(check(&of_values(&long, 1)).is_ok(), "nine bytes");
        long.insert(2, 0x80);
        assert_eq!(check(&of_values(&long, 1)), Err(Refusal::Number), "ten");
    }

    #[test]
    fn letters_that_look_like_lists_are_letters() {
        // The text "uuuu…": bytes 117, inside a text, where they nest nothing.
        let mut value = vec![119, 100];
        value.extend([117; 100]);
        assert!(check(&of_values(&value, 1)).is_ok(), "letters in a value");
        let mut change = vec![1, 1, 5, 0, 4, 1, 7];
        change.extend_from_slice(b"default");
        change.push(100);
        change.extend([117; 100]);
        change.push(0);
        assert!(check(&change).is_ok(), "letters in a text");
    }

    #[test]
    fn every_kind_of_value_is_walked_to_its_end() {
        let values = [
            vec![127],
            vec![126],
            vec![125, 0x7f],
            vec![125, 0xff, 0x01],
            vec![124, 0, 0, 0, 0],
            vec![123, 0, 0, 0, 0, 0, 0, 0, 0],
            vec![121],
            vec![120],
            vec![119, 2, b'h', b'i'],
            vec![118, 1, 1, b'k', 117, 2, 126, 127],
            vec![117, 0],
            vec![116, 3, 1, 2, 3],
        ];
        let all: Vec<u8> = values.concat();
        let count = u8::try_from(values.len()).expect("a few values");
        assert!(check(&of_values(&all, count)).is_ok(), "every kind");
        // One value more than there is: the walk runs into what follows and stops.
        assert!(check(&of_values(&all, count + 2)).is_err(), "cut short");
    }

    #[test]
    fn what_is_named_but_not_there_is_refused() {
        assert_eq!(check(&[]), Err(Refusal::CutShort));
        assert_eq!(begins(&[0, 0]), Ok(Vec::new()), "the empty update");
        // Four thousand million clients, in five bytes.
        assert_eq!(
            check(&[0xff, 0xff, 0xff, 0xff, 0x0f, 0, 0]),
            Err(Refusal::Count)
        );
        // One client with more changes than bytes.
        assert_eq!(check(&[1, 200, 5, 0, 0, 1, 0]), Err(Refusal::Count));
        // A number longer than its field takes.
        assert_eq!(
            check(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x01]),
            Err(Refusal::Number)
        );
        // A clock that runs past 31 bits: in what was deleted, and in the changes.
        let past = [0xff, 0xff, 0xff, 0xff, 0x07];
        let at_most = [0xfe, 0xff, 0xff, 0xff, 0x07];
        assert_eq!(
            check(&[&[0, 1, 5, 1][..], &past, &[1]].concat()),
            Err(Refusal::Number)
        );
        assert!(
            check(&[&[0, 1, 5, 1][..], &at_most, &[1]].concat()).is_ok(),
            "the last clock"
        );
        assert_eq!(
            check(&[&[1, 1, 5][..], &past, &[0, 1, 0]].concat()),
            Err(Refusal::Number)
        );
        assert_eq!(
            check(&[&[1, 1, 5, 0x80, 0x80, 0x80, 0x80, 0x08][..], &[0, 1, 0]].concat()),
            Err(Refusal::Number),
            "a clock that begins at 2 to the 31"
        );
        // A whole number of ten bytes: the least there is, which has no negative.
        let least = [
            125, 0xc0, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x02,
        ];
        assert_eq!(check(&of_values(&least, 1)), Err(Refusal::Number));
        // A hole in a client's changes, and a client given twice.
        assert_eq!(check(&[1, 1, 5, 0, 10, 3, 0]), Err(Refusal::Gap));
        assert_eq!(
            check(&[2, 1, 5, 0, 0, 1, 1, 5, 9, 0, 1, 0]),
            Err(Refusal::Gap)
        );
        // A client given twice in the delete set is refused as well.
        assert_eq!(
            check(&[0, 2, 5, 1, 0, 1, 5, 1, 2, 1]),
            Err(Refusal::Gap),
            "duplicate client in delete set"
        );
        // A formatting mark whose name contains a newline or line separator is refused.
        for bad_mark in [
            &b"x\nlink--abcdefgh"[..],
            &b"x\rlink--abcdefgh"[..],
            "x\u{2028}link--abcdefgh".as_bytes(),
            "x\u{2029}link--abcdefgh".as_bytes(),
            &b"bold\0"[..],
        ] {
            let mut fmt = vec![1, 1, 5, 0, 6, 1, 7];
            fmt.extend_from_slice(b"default");
            fmt.push(u8::try_from(bad_mark.len()).expect("short mark"));
            fmt.extend_from_slice(bad_mark);
            fmt.extend_from_slice(&[4, b't', b'r', b'u', b'e', 0]);
            assert_eq!(check(&fmt), Err(Refusal::Member), "{bad_mark:?}");
        }
        assert_eq!(
            check(&of_values(&[122, 0, 0, 0, 0, 0, 0, 0, 2], 1)),
            Err(Refusal::Unknown),
            "a whole number of 64 bits"
        );
        // Texts of JSON, however many the number says.
        for count in [0, 1] {
            let mut json = vec![1, 1, 5, 0, 2, 1, 7];
            json.extend_from_slice(b"default");
            json.extend_from_slice(&[count, 1, b'1', 1, b'2', 0]);
            assert_eq!(check(&json), Err(Refusal::Unknown), "{count}");
        }
        // An XML hook and a type of no kind, which the editors' library cannot read.
        for kind in [5, 15] {
            let mut shared = vec![1, 1, 5, 0, 7, 1, 7];
            shared.extend_from_slice(b"default");
            shared.extend_from_slice(&[kind, 0]);
            assert_eq!(check(&shared), Err(Refusal::Unknown), "{kind}");
        }
        // A member of the root, which no editor writes.
        let mut member = vec![1, 1, 5, 0, 0x28, 1, 7];
        member.extend_from_slice(b"default");
        member.extend_from_slice(&[1, b'a', 1, 126, 0]);
        assert_eq!(check(&member), Err(Refusal::Member));
        // A member of a node written without the value it replaces, as often as is
        // allowed and once more.
        let first_writes = |times: u8| {
            let mut bytes = vec![1, times, 5, 0];
            for _ in 0..times {
                bytes.extend_from_slice(&[0x28, 0, 7, 0, 1, b'a', 1, 126]);
            }
            bytes.push(0);
            bytes
        };
        let allowed = u8::try_from(MAX_FIRST_WRITES).expect("a few");
        assert!(
            check(&first_writes(allowed)).is_ok(),
            "as often as is allowed"
        );
        assert_eq!(check(&first_writes(allowed + 1)), Err(Refusal::Member));
        // A subdocument.
        let mut subdocument = vec![1, 1, 5, 0, 9, 1, 7];
        subdocument.extend_from_slice(b"default");
        subdocument.extend_from_slice(&[1, b'g', 118, 0, 0]);
        assert_eq!(check(&subdocument), Err(Refusal::Unknown));
    }

    #[test]
    fn a_change_that_names_itself_as_neighbour_or_parent_is_detected() {
        let mut saw = Vec::new();
        // Client 5, clock 0, text "abc" (clocks 0..3) naming (5, 1) as left origin.
        let self_left = vec![1, 1, 5, 0, 0x84, 5, 1, 3, b'a', b'b', b'c', 0];
        walk(&self_left, |(client, begin), change| {
            saw.push(names_self(
                client,
                begin,
                begin + change.content.length(),
                change,
            ));
        })
        .expect("walked");
        assert_eq!(saw, vec![true]);
        // Same, naming (5, 2) as right origin.
        saw.clear();
        let self_right = vec![1, 1, 5, 0, 0x44, 5, 2, 3, b'a', b'b', b'c', 0];
        walk(&self_right, |(client, begin), change| {
            saw.push(names_self(
                client,
                begin,
                begin + change.content.length(),
                change,
            ));
        })
        .expect("walked");
        assert_eq!(saw, vec![true]);
        // Naming itself (5, 0) as parent.
        saw.clear();
        let self_parent = vec![1, 1, 5, 0, 0x04, 0, 5, 0, 1, b'a', 0];
        walk(&self_parent, |(client, begin), change| {
            saw.push(names_self(
                client,
                begin,
                begin + change.content.length(),
                change,
            ));
        })
        .expect("walked");
        assert_eq!(saw, vec![true]);
    }
}
