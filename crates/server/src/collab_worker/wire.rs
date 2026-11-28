//! The requests the server sends its worker and the answers, as the bytes of a frame.
//!
//! A request is a byte that says what it is and its fields in order; an answer is a
//! byte that says what it is (or that the request was refused, and why) and its
//! fields. A number is 8 bytes, the lowest first; bytes are their count as such a
//! number and then themselves, and a count is held to what is left of the frame before
//! anything is made for it. The worker and the server are the same build of one
//! executable, so the form has no version.

use thiserror::Error;

/// The sizes the worker holds what it is sent to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The largest update, in bytes.
    pub update_bytes: u64,
    /// The largest document, in bytes of its state.
    pub state_bytes: u64,
    /// The largest body, in bytes of its JSON.
    pub body_bytes: u64,
    /// How many documents the worker holds at once.
    pub documents: u64,
    /// The bytes of the states of all the documents the worker holds, together: what
    /// keeps its memory within its limit, as a document in memory takes more than its
    /// state and an update makes a copy of it.
    pub held_bytes: u64,
}

impl Limits {
    /// The largest frame either side sends or takes with these limits: a state, the
    /// largest thing a frame carries, and room for the fields around it. A state stored
    /// under a larger limit of a state than this one is as large as the worker's memory
    /// for documents may be, so that it is opened all the same.
    #[must_use]
    pub fn frame_bytes(&self) -> u64 {
        self.state_bytes
            .max(self.held_bytes)
            .max(self.update_bytes)
            .max(self.body_bytes)
            .saturating_add(FRAME_OVERHEAD)
    }
}

/// What a frame carries besides its largest field: a tag, a document and a count or two.
const FRAME_OVERHEAD: u64 = 1024;

/// A document of the worker, by the number the server gave it when it opened it.
pub type DocNumber = u64;

/// How many documents a worker holds at once besides its `documents`, opened only to
/// write what their log holds to their card and closed at once ([`Share::Aside`]): a
/// worker that holds as many documents as it may still writes a log so.
pub const ASIDE_DOCUMENTS: usize = 4;

/// Which of a worker's shares of documents an opened document is counted in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Share {
    /// Kept open for the rooms and the requests, within the worker's `documents`.
    Kept,
    /// Opened only to write its log to its card, and closed at once, within
    /// [`ASIDE_DOCUMENTS`].
    Aside,
}

/// Where a document is opened from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A stored state.
    State(Vec<u8>),
    /// The JSON of a stored body; `None` for a card without one.
    Body(Option<Vec<u8>>),
}

/// A request to the worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// The limits, sent once and first.
    Configure(Limits),
    /// Opens a document under a number that no open document has.
    Open {
        /// The number.
        doc: DocNumber,
        /// What it is opened from.
        source: Source,
        /// Which share it is counted in.
        share: Share,
    },
    /// Takes a client's update into a document.
    Take {
        /// The document.
        doc: DocNumber,
        /// The update, version 1.
        update: Vec<u8>,
    },
    /// Takes again an update the server logged when the document took it before: with
    /// no limit of the size of an update or of the state (what was taken is taken),
    /// only the worker's memory.
    Retake {
        /// The document.
        doc: DocNumber,
        /// The update, version 1.
        update: Vec<u8>,
    },
    /// Adds nodes after the last of a document, written by a client that has written
    /// nothing in it, and takes that as an update ([`Answer::Appended`]): what an agent
    /// adds to a body while its editors type in it.
    Append {
        /// The document.
        doc: DocNumber,
        /// The nodes, the JSON of a list of a body's top-level nodes.
        nodes: Vec<u8>,
    },
    /// The state vector of a document.
    StateVector(DocNumber),
    /// What a document has that a holder of a state vector has not.
    Diff {
        /// The document.
        doc: DocNumber,
        /// The state vector, version 1.
        state_vector: Vec<u8>,
    },
    /// The whole state of a document, as it is stored.
    State(DocNumber),
    /// The body of a document, as JSON.
    Body(DocNumber),
    /// Forgets a document.
    Close(DocNumber),
    /// Forces the next `Request::State` settle check for `doc` to fail and return
    /// [`Answer::Remake`], for tests of lossless settle-failure recovery. Built with the
    /// feature `test-util` only.
    #[cfg(feature = "test-util")]
    FailNextSettle(DocNumber),
}

/// What the worker answers a request it did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// Done, with nothing to say: to a configuration, a closing or a stall.
    Done,
    /// A document is open.
    Opened {
        /// Its state, if it is not what it was opened from: made from a body, or
        /// repaired.
        state: Option<Vec<u8>>,
        /// The size of its state.
        state_bytes: u64,
        /// How many clients it names.
        clients: u64,
        /// How many changes it holds across all clients, deleted ones too.
        changes: u64,
        /// The most changes one of its clients has, deleted ones too.
        client_changes: u64,
        /// The most steps the Y library may take to read it once one more client has
        /// written into it ([`fvoci_collab::steps_ahead`]).
        steps_ahead: u64,
    },
    /// An update was taken.
    Taken {
        /// What a repair changed, as an update for every editor; `None` if nothing.
        repair: Option<Vec<u8>>,
        /// The size of the document's state now.
        state_bytes: u64,
        /// How many clients it names now.
        clients: u64,
        /// How many changes it holds across all clients now, deleted ones too.
        changes: u64,
        /// The most changes one of its clients has now, deleted ones too.
        client_changes: u64,
        /// The most steps the Y library may take to read it once one more client has
        /// written into it, now.
        steps_ahead: u64,
        /// The files the images and file blocks of its body name now, by their stored
        /// names.
        files: Vec<String>,
        /// How many board views its body has now: a room whose body shows none (a
        /// diary's entry) logs no update that leaves one.
        board_views: u64,
        /// Whether reading the document's state would take more steps than it may
        /// ([`fvoci_collab::READING_STEPS`]), so it was not read back from its state
        /// and must be made again from its body before its state is stored or another
        /// update is taken.
        remake: bool,
        /// Whether the updates held since the last checkpoint have grown enough that
        /// the server should fold the document's state soon.
        fold: bool,
    },
    /// Nodes were added ([`Request::Append`]).
    Appended {
        /// The update that added them, version 1, for every editor.
        update: Vec<u8>,
        /// What taking it came to: an [`Answer::Taken`].
        taken: Box<Answer>,
    },
    /// Bytes: a state vector, a diff or a state.
    Bytes(Vec<u8>),
    /// The JSON of a body; `None` for no body.
    Body(Option<Vec<u8>>),
    /// Returned by [`Request::State`] when settling an unsettled document fails: the live
    /// document is kept intact for [`Request::Body`] and [`Request::Close`] only, and the
    /// server must write its body and remake it from that without losing updates.
    Remake,
}

/// Why the worker refused a request. The document is as it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The update is larger than the limit.
    UpdateTooLarge,
    /// The bytes are no update, or no state vector.
    Unreadable,
    /// The update builds on changes the document does not have.
    MissingChanges,
    /// The update writes outside the body, or cannot be applied, or leaves a document
    /// that cannot be read again.
    Unappliable,
    /// The document would be larger than the limit.
    DocumentTooLarge,
    /// The document would take more steps to read than it may: with an update that is
    /// refused for it, it is made again from its body, which takes none.
    TooManySteps,
    /// The document would be no body.
    NoBody,
    /// The body would be larger than the limit.
    BodyTooLarge,
    /// A table in the body would have more rows or columns than it may.
    TableTooLarge,
    /// The worker holds as many documents, or as many bytes of them, as it may.
    TooManyDocuments,
    /// No document is open under the number, or one already is.
    UnknownDocument,
    /// The request is not one the worker takes now (a second configuration, a request
    /// before the first).
    OutOfOrder,
    /// The document could not be restored after a post-mutation refusal and was closed
    /// alone so the server reopens it from what is stored without ending the worker.
    Closed,
}

/// The answer to a request.
pub type Response = Result<Answer, Refused>;

/// A refusal with what the worker said of it, for the log.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{refusal:?}: {message}")]
pub struct Refused {
    /// Why.
    pub refusal: Refusal,
    /// What the worker said.
    pub message: String,
    /// Whether the worker applied the update in place and had to rebuild the document
    /// from its checkpoint to refuse it.
    pub rolled_back: bool,
}

/// Bytes that are no request or answer.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("the frame is no {what}")]
pub struct Malformed {
    what: &'static str,
}

impl Request {
    /// The bytes of the request.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Writer::default();
        match self {
            Self::Configure(limits) => {
                out.tag(1);
                for number in [
                    limits.update_bytes,
                    limits.state_bytes,
                    limits.body_bytes,
                    limits.documents,
                    limits.held_bytes,
                ] {
                    out.number(number);
                }
            }
            Self::Open { doc, source, share } => {
                out.tag(2).number(*doc);
                match source {
                    Source::State(state) => out.tag(0).bytes(state),
                    Source::Body(body) => out.tag(1).optional(body.as_deref()),
                };
                out.tag(match share {
                    Share::Kept => 0,
                    Share::Aside => 1,
                });
            }
            Self::Take { doc, update } => {
                out.tag(3).number(*doc).bytes(update);
            }
            Self::Retake { doc, update } => {
                out.tag(10).number(*doc).bytes(update);
            }
            Self::Append { doc, nodes } => {
                out.tag(12).number(*doc).bytes(nodes);
            }
            Self::StateVector(doc) => {
                out.tag(4).number(*doc);
            }
            Self::Diff { doc, state_vector } => {
                out.tag(5).number(*doc).bytes(state_vector);
            }
            Self::State(doc) => {
                out.tag(6).number(*doc);
            }
            Self::Body(doc) => {
                out.tag(7).number(*doc);
            }
            Self::Close(doc) => {
                out.tag(8).number(*doc);
            }
            #[cfg(feature = "test-util")]
            Self::FailNextSettle(doc) => {
                out.tag(11).number(*doc);
            }
        }
        out.0
    }

    /// Reads a request.
    ///
    /// # Errors
    ///
    /// Returns [`Malformed`] for bytes that are no request.
    pub fn decode(bytes: &[u8]) -> Result<Self, Malformed> {
        let mut input = Reader::new(bytes, "request");
        let request = match input.tag()? {
            1 => Self::Configure(Limits {
                update_bytes: input.number()?,
                state_bytes: input.number()?,
                body_bytes: input.number()?,
                documents: input.number()?,
                held_bytes: input.number()?,
            }),
            2 => {
                let doc = input.number()?;
                let source = match input.tag()? {
                    0 => Source::State(input.bytes()?),
                    1 => Source::Body(input.optional()?),
                    _ => return Err(input.malformed()),
                };
                let share = match input.tag()? {
                    0 => Share::Kept,
                    1 => Share::Aside,
                    _ => return Err(input.malformed()),
                };
                Self::Open { doc, source, share }
            }
            3 => Self::Take {
                doc: input.number()?,
                update: input.bytes()?,
            },
            10 => Self::Retake {
                doc: input.number()?,
                update: input.bytes()?,
            },
            12 => Self::Append {
                doc: input.number()?,
                nodes: input.bytes()?,
            },
            4 => Self::StateVector(input.number()?),
            5 => Self::Diff {
                doc: input.number()?,
                state_vector: input.bytes()?,
            },
            6 => Self::State(input.number()?),
            7 => Self::Body(input.number()?),
            8 => Self::Close(input.number()?),
            #[cfg(feature = "test-util")]
            11 => Self::FailNextSettle(input.number()?),
            _ => return Err(input.malformed()),
        };
        input.end()?;
        Ok(request)
    }
}

/// The tag of a refusal, after which come its reason and message.
const REFUSED: u8 = 0x7f;

impl Refusal {
    /// Every refusal, for the tests of its byte.
    #[cfg(test)]
    const ALL: [Self; 13] = [
        Self::UpdateTooLarge,
        Self::Unreadable,
        Self::MissingChanges,
        Self::Unappliable,
        Self::DocumentTooLarge,
        Self::NoBody,
        Self::BodyTooLarge,
        Self::TooManyDocuments,
        Self::UnknownDocument,
        Self::OutOfOrder,
        Self::TooManySteps,
        Self::TableTooLarge,
        Self::Closed,
    ];

    /// The byte of the refusal in an answer.
    fn byte(self) -> u8 {
        match self {
            Self::UpdateTooLarge => 0,
            Self::Unreadable => 1,
            Self::MissingChanges => 2,
            Self::Unappliable => 3,
            Self::DocumentTooLarge => 4,
            Self::NoBody => 5,
            Self::BodyTooLarge => 6,
            Self::TooManyDocuments => 7,
            Self::UnknownDocument => 8,
            Self::OutOfOrder => 9,
            Self::TooManySteps => 10,
            Self::TableTooLarge => 11,
            Self::Closed => 12,
        }
    }

    /// The refusal of a byte in an answer.
    fn of_byte(byte: u8) -> Option<Self> {
        Some(match byte {
            0 => Self::UpdateTooLarge,
            1 => Self::Unreadable,
            2 => Self::MissingChanges,
            3 => Self::Unappliable,
            4 => Self::DocumentTooLarge,
            5 => Self::NoBody,
            6 => Self::BodyTooLarge,
            7 => Self::TooManyDocuments,
            8 => Self::UnknownDocument,
            9 => Self::OutOfOrder,
            10 => Self::TooManySteps,
            11 => Self::TableTooLarge,
            12 => Self::Closed,
            _ => return None,
        })
    }
}

/// The bytes of an answer.
#[must_use]
pub fn encode_response(response: &Response) -> Vec<u8> {
    let mut out = Writer::default();
    match response {
        Ok(Answer::Done) => {
            out.tag(1);
        }
        Ok(Answer::Opened {
            state,
            state_bytes,
            clients,
            changes,
            client_changes,
            steps_ahead,
        }) => {
            out.tag(2)
                .optional(state.as_deref())
                .number(*state_bytes)
                .number(*clients)
                .number(*changes)
                .number(*client_changes)
                .number(*steps_ahead);
        }
        Ok(Answer::Taken {
            repair,
            state_bytes,
            clients,
            changes,
            client_changes,
            steps_ahead,
            files,
            board_views,
            remake,
            fold,
        }) => {
            out.tag(3)
                .optional(repair.as_deref())
                .number(*state_bytes)
                .number(*clients)
                .number(*changes)
                .number(*client_changes)
                .number(*steps_ahead)
                .names(files)
                .number(*board_views)
                .tag(u8::from(*remake))
                .tag(u8::from(*fold));
        }
        Ok(Answer::Bytes(bytes)) => {
            out.tag(4).bytes(bytes);
        }
        Ok(Answer::Body(body)) => {
            out.tag(5).optional(body.as_deref());
        }
        Ok(Answer::Remake) => {
            out.tag(6);
        }
        Ok(Answer::Appended { update, taken }) => {
            out.tag(7)
                .bytes(update)
                .bytes(&encode_response(&Ok((**taken).clone())));
        }
        Err(refused) => {
            out.tag(REFUSED)
                .tag(refused.refusal.byte())
                .bytes(refused.message.as_bytes())
                .tag(u8::from(refused.rolled_back));
        }
    }
    out.0
}

/// Reads an answer.
///
/// # Errors
///
/// Returns [`Malformed`] for bytes that are no answer.
pub fn decode_response(bytes: &[u8]) -> Result<Response, Malformed> {
    let mut input = Reader::new(bytes, "answer");
    let response = match input.tag()? {
        1 => Ok(Answer::Done),
        2 => Ok(Answer::Opened {
            state: input.optional()?,
            state_bytes: input.number()?,
            clients: input.number()?,
            changes: input.number()?,
            client_changes: input.number()?,
            steps_ahead: input.number()?,
        }),
        3 => Ok(Answer::Taken {
            repair: input.optional()?,
            state_bytes: input.number()?,
            clients: input.number()?,
            changes: input.number()?,
            client_changes: input.number()?,
            steps_ahead: input.number()?,
            files: input.names()?,
            board_views: input.number()?,
            remake: input.flag()?,
            fold: input.flag()?,
        }),
        4 => Ok(Answer::Bytes(input.bytes()?)),
        5 => Ok(Answer::Body(input.optional()?)),
        6 => Ok(Answer::Remake),
        7 => {
            let update = input.bytes()?;
            match decode_response(&input.bytes()?)? {
                Ok(taken @ Answer::Taken { .. }) => Ok(Answer::Appended {
                    update,
                    taken: Box::new(taken),
                }),
                _ => return Err(input.malformed()),
            }
        }
        REFUSED => {
            let refusal = Refusal::of_byte(input.tag()?).ok_or_else(|| input.malformed())?;
            let message = String::from_utf8_lossy(&input.bytes()?).into_owned();
            let rolled_back = input.flag()?;
            Err(Refused {
                refusal,
                message,
                rolled_back,
            })
        }
        _ => return Err(input.malformed()),
    };
    input.end()?;
    Ok(response)
}

/// Writes the fields of a frame.
#[derive(Debug, Default)]
struct Writer(Vec<u8>);

impl Writer {
    fn tag(&mut self, tag: u8) -> &mut Self {
        self.0.push(tag);
        self
    }

    fn number(&mut self, number: u64) -> &mut Self {
        self.0.extend_from_slice(&number.to_le_bytes());
        self
    }

    fn bytes(&mut self, bytes: &[u8]) -> &mut Self {
        self.number(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
        self.0.extend_from_slice(bytes);
        self
    }

    fn optional(&mut self, bytes: Option<&[u8]>) -> &mut Self {
        match bytes {
            None => self.tag(0),
            Some(bytes) => self.tag(1).bytes(bytes),
        }
    }

    fn names(&mut self, names: &[String]) -> &mut Self {
        self.number(u64::try_from(names.len()).unwrap_or(u64::MAX));
        for name in names {
            self.bytes(name.as_bytes());
        }
        self
    }
}

/// Reads the fields of a frame.
#[derive(Debug)]
struct Reader<'frame> {
    rest: &'frame [u8],
    what: &'static str,
}

impl<'frame> Reader<'frame> {
    fn new(bytes: &'frame [u8], what: &'static str) -> Self {
        Self { rest: bytes, what }
    }

    fn malformed(&self) -> Malformed {
        Malformed { what: self.what }
    }

    fn take(&mut self, count: usize) -> Result<&'frame [u8], Malformed> {
        if count > self.rest.len() {
            return Err(self.malformed());
        }
        let (taken, rest) = self.rest.split_at(count);
        self.rest = rest;
        Ok(taken)
    }

    fn tag(&mut self) -> Result<u8, Malformed> {
        Ok(self.take(1)?[0])
    }

    fn flag(&mut self) -> Result<bool, Malformed> {
        match self.tag()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(self.malformed()),
        }
    }

    fn number(&mut self) -> Result<u64, Malformed> {
        let bytes: [u8; 8] = self.take(8)?.try_into().map_err(|_| self.malformed())?;
        Ok(u64::from_le_bytes(bytes))
    }

    /// Bytes, their count held to what is left before anything is made for them.
    fn bytes(&mut self) -> Result<Vec<u8>, Malformed> {
        let count = usize::try_from(self.number()?).map_err(|_| self.malformed())?;
        Ok(self.take(count)?.to_vec())
    }

    fn optional(&mut self) -> Result<Option<Vec<u8>>, Malformed> {
        match self.tag()? {
            0 => Ok(None),
            1 => self.bytes().map(Some),
            _ => Err(self.malformed()),
        }
    }

    /// Names, as text; their count held to what is left (each takes eight bytes at
    /// least) before anything is made for them.
    fn names(&mut self) -> Result<Vec<String>, Malformed> {
        let count = usize::try_from(self.number()?).map_err(|_| self.malformed())?;
        if count > self.rest.len() / 8 {
            return Err(self.malformed());
        }
        let mut names = Vec::with_capacity(count);
        for _ in 0..count {
            let name = String::from_utf8(self.bytes()?).map_err(|_| self.malformed())?;
            names.push(name);
        }
        Ok(names)
    }

    fn end(&self) -> Result<(), Malformed> {
        if self.rest.is_empty() {
            Ok(())
        } else {
            Err(self.malformed())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_request_reads_back_as_it_was_written() {
        let requests = [
            Request::Configure(Limits {
                update_bytes: 1,
                state_bytes: 2,
                body_bytes: 3,
                documents: 4,
                held_bytes: 5,
            }),
            Request::Open {
                doc: 7,
                source: Source::State(vec![1, 2, 3]),
                share: Share::Aside,
            },
            Request::Open {
                doc: 8,
                source: Source::Body(None),
                share: Share::Kept,
            },
            Request::Open {
                doc: 9,
                source: Source::Body(Some(b"{}".to_vec())),
                share: Share::Kept,
            },
            Request::Take {
                doc: 1,
                update: vec![0, 0],
            },
            Request::Retake {
                doc: 1,
                update: vec![0, 1],
            },
            Request::Append {
                doc: 1,
                nodes: b"[]".to_vec(),
            },
            Request::StateVector(2),
            Request::Diff {
                doc: 3,
                state_vector: vec![0],
            },
            Request::State(4),
            Request::Body(5),
            Request::Close(6),
            #[cfg(feature = "test-util")]
            Request::FailNextSettle(7),
        ];
        for request in requests {
            assert_eq!(
                Request::decode(&request.encode()),
                Ok(request.clone()),
                "{request:?}"
            );
        }
    }

    #[test]
    fn every_answer_reads_back_as_it_was_written() {
        let mut responses = vec![
            Ok(Answer::Done),
            Ok(Answer::Appended {
                update: vec![1, 2],
                taken: Box::new(Answer::Taken {
                    repair: Some(vec![3]),
                    state_bytes: 9,
                    clients: 2,
                    changes: 4,
                    client_changes: 3,
                    steps_ahead: 1,
                    files: vec!["a.png".to_owned()],
                    board_views: 0,
                    remake: false,
                    fold: false,
                }),
            }),
            Ok(Answer::Opened {
                state: Some(vec![1]),
                state_bytes: 1,
                clients: 1,
                changes: 8,
                client_changes: 5,
                steps_ahead: 9,
            }),
            Ok(Answer::Opened {
                state: None,
                state_bytes: 2,
                clients: 0,
                changes: 0,
                client_changes: 0,
                steps_ahead: 0,
            }),
            Ok(Answer::Taken {
                repair: None,
                state_bytes: 3,
                clients: 2,
                changes: 650_000,
                client_changes: 600_000,
                steps_ahead: 800_000,
                files: Vec::new(),
                board_views: 0,
                remake: false,
                fold: true,
            }),
            Ok(Answer::Taken {
                repair: Some(vec![6]),
                state_bytes: 4,
                clients: 70_000,
                changes: 70_000,
                client_changes: 1,
                steps_ahead: 0,
                files: vec!["7a.png".to_owned(), "7b.png".to_owned()],
                board_views: 10,
                remake: true,
                fold: false,
            }),
            Ok(Answer::Bytes(vec![4, 5])),
            Ok(Answer::Body(Some(b"{}".to_vec()))),
            Ok(Answer::Body(None)),
            Ok(Answer::Remake),
        ];
        responses.extend(Refusal::ALL.iter().flat_map(|refusal| {
            [false, true].map(|rolled_back| {
                Err(Refused {
                    refusal: *refusal,
                    message: "why".to_owned(),
                    rolled_back,
                })
            })
        }));
        for response in responses {
            assert_eq!(
                decode_response(&encode_response(&response)),
                Ok(response.clone()),
                "{response:?}"
            );
        }
    }

    #[test]
    fn a_count_past_the_end_of_the_frame_is_refused_before_anything_is_made_for_it() {
        let mut take = vec![3];
        take.extend_from_slice(&1_u64.to_le_bytes());
        take.extend_from_slice(&u64::MAX.to_le_bytes());
        assert!(Request::decode(&take).is_err(), "a count of 2^64 - 1");
        let mut short = vec![3];
        short.extend_from_slice(&1_u64.to_le_bytes());
        short.extend_from_slice(&3_u64.to_le_bytes());
        short.extend_from_slice(&[1, 2]);
        assert!(Request::decode(&short).is_err(), "one byte short");
        assert!(Request::decode(&[]).is_err(), "nothing");
        assert!(Request::decode(&[42]).is_err(), "no request is 42");
        let mut long = Request::State(1).encode();
        long.push(0);
        assert!(Request::decode(&long).is_err(), "a byte after the end");
        let mut bad_fold = encode_response(&Ok(Answer::Taken {
            repair: None,
            state_bytes: 1,
            clients: 1,
            changes: 1,
            client_changes: 1,
            steps_ahead: 0,
            files: Vec::new(),
            board_views: 0,
            remake: false,
            fold: false,
        }));
        *bad_fold.last_mut().expect("fold flag byte") = 2;
        assert!(decode_response(&bad_fold).is_err(), "fold flag past 0 or 1");
        let len = bad_fold.len();
        bad_fold[len - 1] = 0;
        bad_fold[len - 2] = 2;
        assert!(
            decode_response(&bad_fold).is_err(),
            "remake flag past 0 or 1"
        );
    }
}
