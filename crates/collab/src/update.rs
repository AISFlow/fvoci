//! Taking an update from a client.

mod in_place;

use std::collections::HashSet;

use thiserror::Error;
use yrs::updates::decoder::Decode;
use yrs::{ClientID, Doc, ID, Out, ReadTxn, StateVector, Transact, Update};

use in_place::{ApplyTarget, apply_and_check};
#[cfg(feature = "test-util")]
pub(crate) use in_place::{CaptureMode, TxnUpdateMode, apply_and_capture};
pub(crate) use in_place::{InPlaceDoc, UpdateInPlaceError, with_update_in_place};

use crate::FIELD;
use crate::block_cache::ApplyDelta;
use crate::placing::{self, Held, Sent};
use crate::reading::{self, ReadingCache};
use crate::scan::{self, Content, Id, Refusal};
use crate::settle::{check_ends, read_state, settled_from};

/// Whether [`apply_and_check`] captures an [`ApplyDelta`] for incremental block-cache
/// updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeltaMode {
    Skip,
    Capture,
}

/// The sizes an update may have and leave.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpdateLimits {
    /// The largest update, in bytes as it arrives.
    pub update_bytes: usize,
    /// The largest Y document an update may leave, in bytes as it is stored (a version
    /// 1 update of the whole document). The documents of this crate keep what was
    /// deleted (they are made without garbage collection), so a document grows with
    /// every edit even where the body does not.
    pub state_bytes: usize,
    /// The largest body an update may leave, in bytes of its stored JSON. Every body is
    /// held to [`fvoci_model::BODY_MAX_BYTES`] as well, so a larger value is that.
    pub body_bytes: usize,
    /// How many steps the Y library may have to take to place the changes of one
    /// update: changes of the document it walks past, and letters of changes it cuts,
    /// 256 a step ([`PLACING_STEPS`] for what a client sends); `None` for no count, for
    /// what was taken before and is read again. With a count, the deletions of each
    /// client must also come in order, without overlapping, as an editor writes them.
    pub placing_steps: Option<u64>,
    /// How many clients the document an update leaves may name ([`MAX_CLIENTS`] for
    /// what a client sends); `None` for no count, for what was taken before.
    pub clients: Option<usize>,
    /// How many changes one client may have in the document an update leaves, deleted
    /// ones too ([`MAX_CLIENT_CHANGES`] for what a client sends); `None` for no count,
    /// for what was taken before.
    pub client_changes: Option<u64>,
    /// How many changes the document an update leaves (or a stored state opened) may
    /// hold across all clients together, deleted ones too ([`MAX_TOTAL_CHANGES`]);
    /// `None` for no count.
    pub total_changes: Option<u64>,
    /// How many steps the Y library may take to read the document an update leaves
    /// from its state, as it does for every update after it and every time the
    /// document is opened ([`READING_STEPS`]); `None` for no count. The steps are
    /// counted from the state before the library reads it.
    pub reading_steps: Option<u64>,
}

/// How many clients a document an update a client sends leaves may name
/// ([`UpdateLimits::clients`]). The Y library reads the changes of a state client by
/// client and looks for each among the others, so that reading a state takes the
/// square of the clients it names: a letter typed into a document of 10,000 clients is
/// taken in 0.1 s, of 20,000 in 0.9 s and of 40,000 in 4.9 s (release build). An
/// editor takes a client for each time it opens the document, so a document grows to
/// it only in years; what holds one makes it again from its body, with one client,
/// well before.
pub const MAX_CLIENTS: usize = 16 * 1024;

/// How many changes one client may have in a document an update a client sends leaves,
/// deleted ones too ([`UpdateLimits::client_changes`]). The Y library keeps the changes
/// of a client in one array, and moves every later one of them each time it cuts one in
/// two: 17,500 cuts of one letter each, as many as an update of 64 KiB holds, took 3 s
/// where the client cut has 250,000 changes, 6.9 s at 520,000 and 26 s at 1,170,000
/// (release build, an idle machine), and 1.1 s where as many were spread over a hundred
/// clients. [`crate::from_body`] splits large marked bodies across contiguous descending
/// clients of at most `MAX_CLIENT_CHANGES / 4` (131,072) changes each so no client starts
/// near the three-quarters remake mark.
pub const MAX_CLIENT_CHANGES: u64 = 512 * 1024;

/// How many changes a document may hold across all clients together, deleted ones too
/// ([`UpdateLimits::total_changes`]). Bounds total CRDT block count against multi-client
/// fragmentation while staying above the worst-case allowlist-valid 100,000-node / 8 MiB
/// body (~660,372 changes) with over 120,000 changes of editing headroom.
pub const MAX_TOTAL_CHANGES: u64 = 768 * 1024;

/// How many steps the Y library may take to place the changes of one update a client
/// sends ([`UpdateLimits::placing_steps`]). A step past a change takes 50 to 110 ns
/// where the update is read once, and is taken again every time the library reads
/// the document again, where the change is of a client below the others of its list:
/// as the update is taken, as what it leaves is read again, and as it is repaired. An
/// update of 3 million steps of those, repaired, was taken in 1.9 to 2.0 s, and as many cuts
/// of a text of a million letters of three bytes in 0.3 s (release build, an idle
/// machine): within a fifth of the time a worker has for its answer. Steps that are
/// taken again at every reading are counted for the document as well
/// ([`READING_STEPS`]): an update of more of them than a document may have is refused
/// once it is applied, in under a second. An editor's update is a handful of steps. Past it are updates of a hundred changes that each go into a
/// list of tens of thousands, or of hundreds of cuts of a text of a million letters.
pub const PLACING_STEPS: u64 = 3 * 1024 * 1024;

/// How many steps the Y library may take to read a document from its state: the
/// document an update a client sends leaves, and one that was stored
/// ([`UpdateLimits::reading_steps`]). The library places a change by walking past the
/// changes between the neighbours it names, and walks so again every time it reads the
/// document from its state: once as what an update leaves is settled, again if a repair
/// changes the document, and once as a canonical document is opened. A step takes 0.13 to 0.24 µs in a
/// list of 50,000 (release build, a machine that does other work besides, at which
/// all the times here were measured; a busier one took twice as long), so each
/// reading of such a document at this many steps takes 0.15 to 0.25 s more than the
/// document's size asks: a paragraph typed
/// into a list of 50,000 at these steps was taken in 0.31 s where it took 0.06 s,
/// repaired in 0.74 s where it took 0.23 s, and the document was opened in 0.30 s
/// where it took 0.10 s. With the slowest update of the largest body (2.3 s, repaired)
/// that is within half the time a worker has for its answer (10 s). A step takes
/// longer the more changes a document has that lie far from their neighbours, 0.5 µs
/// in a list of 200,000 such and 0.7 to 0.9 µs in one of 700,000
/// (`tests/cost.rs`, `scattered`): such a document is slow by its size before its
/// steps are counted, 1.5 s a reading, which the limits of size and of a client's
/// changes are for, not this one.
///
/// What editors write is placed in a step or two where two people wrote at one place:
/// the largest bodies, edited, are under 5,000 steps (the changes of the body's
/// client, looked at once each while an edit waits for the one it names), and a
/// body's own document is none. Past it are documents with changes made to walk: 62
/// changes of 383 bytes that name the last of 50,000 children to their right and
/// nothing to their left are three million steps.
///
/// And past it are documents that thousands of sessions wrote one after the other,
/// with no change made for it. Each session is a client, whose changes wait for those
/// of the session it wrote after, and the library looks through all that wait each
/// time one does: 400 sessions of a paragraph are 289 steps, 400 of 50 changes
/// 15,000, and from a few thousand sessions on a session whose client is read before
/// the others brings the steps past the limit with its first paragraph, whatever
/// they were (4,000 sessions of 20 changes, 2,500 of 100: `tests/reading.rs`).
///
/// Before it comes to that, what holds a document goes by the steps the changes it
/// has may take once one more client is read first ([`crate::steps_ahead`]: each
/// change looked at once for each client), and makes the document again from its
/// body, which has no steps and one client: at an eighth of this ahead as the
/// document is closed with nobody holding it, and at three quarters under its
/// editors. What an update adds is not in that count (a change it writes, or cuts in
/// two, is a step for every 128 clients), so an update can still leave a document
/// past this limit: [`crate::take_update`] still takes it (repairing in place if
/// needed, without reading the slow state again) and marks [`crate::Taken::remake`] so
/// what holds it writes its body and makes it again from that without losing the
/// update.
///
/// A stored state past it is refused before the library reads it, in the time of the
/// count, which stops a little past the limit whatever the state holds: one that was
/// stored before the steps were counted, as every update taken since leaves a
/// document within it (or is made again from its body before its state is stored).
pub const READING_STEPS: u64 = 1024 * 1024;

/// Why an update is refused.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum UpdateError {
    /// The update is larger than [`UpdateLimits::update_bytes`].
    #[error("the update is {size} bytes, more than the {limit} allowed")]
    TooLarge {
        /// The size of the update.
        size: usize,
        /// The limit it is past: [`UpdateLimits::update_bytes`].
        limit: usize,
    },
    /// The bytes are no update.
    #[error("the update cannot be read: {0}")]
    Unreadable(String),
    /// The update cannot be applied to the document.
    #[error("the update cannot be applied: {0}")]
    Unappliable(String),
    /// The deletions of a client in the update go back or overlap, which Yjs never
    /// writes: the library would walk the client's changes again for each.
    #[error("the deletions of client {client} go back to clock {start} from {end}")]
    DeletionsOutOfOrder {
        /// The Yjs client whose deletions they are, by its number.
        client: u64,
        /// Where the range that goes back starts.
        start: u32,
        /// Where the range before it ends.
        end: u32,
    },
    /// The update builds on changes the document does not have: a client's changes
    /// begin later than where the document has them, or part of the update waits for
    /// changes of another client instead of taking effect. A client sends what it has
    /// in order; one that leaves a gap is not in step with the document.
    #[error("the update builds on changes the document does not have")]
    MissingChanges,
    /// The update writes outside the fragment that holds the body.
    #[error("the update writes to the root {0:?}, which is not the body's")]
    UnexpectedRoot(String),
    /// The changes of the update would take the library more steps to place than
    /// [`UpdateLimits::placing_steps`]: each change that names a neighbour the document
    /// has to its left, or none, is counted what it may walk past, and each change it
    /// cuts its length.
    #[error(
        "the changes of the update would take {steps} steps to place in the document, \
         more than the {limit} allowed"
    )]
    PlacedPastTooMany {
        /// The steps its changes may take to place, counted no further than is needed
        /// to know that they are more than the limit.
        steps: u64,
        /// The limit it is past: [`UpdateLimits::placing_steps`].
        limit: u64,
    },
    /// With the update the document would name more clients than
    /// [`UpdateLimits::clients`].
    #[error(
        "with the update the document would name {clients} clients, more than the {limit} allowed"
    )]
    TooManyClients {
        /// The clients it would name.
        clients: usize,
        /// The limit it is past: [`UpdateLimits::clients`].
        limit: usize,
    },
    /// With the update a client of the document would have more changes than
    /// [`UpdateLimits::client_changes`].
    #[error(
        "with the update a client of the document would have {changes} changes, more than \
         the {limit} allowed"
    )]
    TooManyChanges {
        /// The most changes a client would have, deleted ones too.
        changes: u64,
        /// The limit it is past: [`UpdateLimits::client_changes`].
        limit: u64,
    },
    /// With the update the document would hold more changes across all clients than
    /// [`UpdateLimits::total_changes`].
    #[error(
        "with the update the document would have {changes} changes, more than the {limit} allowed"
    )]
    TooManyTotalChanges {
        /// The total changes across all clients, deleted ones too.
        changes: u64,
        /// The limit it is past: [`UpdateLimits::total_changes`].
        limit: u64,
    },
    /// With the update the library would take more steps to read the document from its
    /// state than [`UpdateLimits::reading_steps`].
    #[error(
        "with the update the document would take {steps} steps to read, more than the \
         {limit} allowed"
    )]
    TooManyReadingSteps {
        /// The steps it would take, counted no further than is needed to know that
        /// they are more than the limit.
        steps: u64,
        /// The limit it is past: [`UpdateLimits::reading_steps`].
        limit: u64,
    },
    /// With the update the document would be larger than [`UpdateLimits::state_bytes`].
    #[error("with the update the document would be {size} bytes, more than the {limit} allowed")]
    DocumentTooLarge {
        /// The size the document would have.
        size: usize,
        /// The limit it is past: [`UpdateLimits::state_bytes`].
        limit: usize,
    },
    /// With the update the document could not be read again from what is stored of it:
    /// the library took something that it writes in another form than it read it.
    #[error("with the update the document could not be read again: {0}")]
    LeavesUnreadable(String),
    /// The document the update was given for cannot be read again from its own state,
    /// so that no update can be taken for it. Nothing this crate takes leaves one so.
    #[error("the document cannot be read again from its own state: {0}")]
    DocumentUnreadable(String),
}

/// The whole of a Y document as a version 1 update: what is stored, and what brings an
/// empty document to the same state.
#[must_use]
pub fn state_of(doc: &Doc) -> Vec<u8> {
    doc.transact()
        .encode_state_as_update_v1(&StateVector::default())
}

/// A copy of a Y document that shares nothing with it, which writes as `writer`, or as
/// a client new to it if `None`. (Cloning a `Doc` gives a second handle to the same
/// document.)
///
/// # Errors
///
/// Returns [`UpdateError::DocumentUnreadable`] if the document's own state cannot be
/// read again as any update is read.
#[cfg(any(test, feature = "test-util"))]
pub(crate) fn copy_of(doc: &Doc, writer: Option<u64>) -> Result<Doc, UpdateError> {
    crate::settle::doc_of_state(&state_of(doc), writer).map_err(UpdateError::DocumentUnreadable)
}

/// Risky patterns and newly inserted shared-type IDs spotted while scanning an incoming
/// update against the document's state vector.
#[derive(Debug, Default)]
pub(crate) struct ReadFlags {
    /// At least one non-GC item starts before the document's known clock for its client
    /// and extends past it, so `yrs`'s `BlockSet::exclude` trims it in place.
    pub(crate) partial_overlap: bool,
    /// Either `partial_overlap` is true or the update has two consecutive items of the
    /// same client sharing a `right_origin` that could squash in `commit()`, requiring a
    /// pre-apply linear origin check before handing the update to `yrs`.
    pub(crate) check_origins_before_apply: bool,
    /// Shared-type (`Content::Type`) IDs newly introduced without an explicit wire
    /// `member` (`begin >= floor`, `member == None`).
    pub(crate) new_types: Vec<ID>,
}

/// Reads the bytes of an update for a document, after the walk that keeps from the
/// library what it does not survive (`scan`), with how many clients the document names
/// with it and any risky patterns that need pre-apply origin checks.
pub(crate) fn read(doc: &Doc, update: &[u8]) -> Result<(Update, usize, ReadFlags), UpdateError> {
    // A client's changes go on from where the document has them: the library would
    // take changes that begin later and leave a hole before them.
    let known = doc.transact().state_vector();
    let mut flags = ReadFlags::default();
    let mut invalid_origin = false;
    let mut has_both_origins = false;
    let mut has_squash_pair = false;
    let mut multi_unit_clients: HashSet<u64> = HashSet::new();
    let mut referenced_clients: HashSet<u64> = HashSet::new();
    let mut prev_in_client: Option<(u64, u64, Option<Id>)> = None;
    let walked = scan::walk(update, |(client, begin), change| {
        if change.left.is_some() && change.left == change.right {
            invalid_origin = true;
        }
        let len = change.content.length();
        if len == 0 {
            // `yrs::Update::decode` drops zero-length items without advancing the clock,
            // so keep `prev_in_client` intact across them.
            return;
        }
        if matches!(change.content, Content::Collected(_)) {
            prev_in_client = None;
            return;
        }
        let end = begin.saturating_add(len);
        let floor = u64::from(known.get(&ClientID::new(client)));
        if end > floor {
            if matches!(change.content, Content::Type)
                && change.member.is_none()
                && let Ok(clock) = u32::try_from(begin)
            {
                flags.new_types.push(ID::new(ClientID::new(client), clock));
            }
            if scan::names_self(client, begin.max(floor), end, change) {
                invalid_origin = true;
            }
            if change.left.is_some() && change.right.is_some() {
                has_both_origins = true;
            }
            if end - begin.max(floor) >= 2 {
                multi_unit_clients.insert(client);
            }
            if let Some((other, _)) = change.left {
                referenced_clients.insert(other);
            }
            if let Some((other, _)) = change.right {
                referenced_clients.insert(other);
            }
        }
        if begin < floor && end > floor {
            flags.partial_overlap = true;
            flags.check_origins_before_apply = true;
        }
        if let Some((prev_client, prev_end, prev_right)) = prev_in_client
            && prev_client == client
            && begin == prev_end
            && change.left == Some((client, prev_end - 1))
            && change.right == prev_right
        {
            has_squash_pair = true;
        }
        prev_in_client = Some((client, end, change.right));
    })
    .map_err(|refusal| UpdateError::Unreadable(refusal.reason().to_owned()))?;
    if invalid_origin {
        return Err(UpdateError::Unreadable(Refusal::Origin.reason().to_owned()));
    }
    let mut clients = known.len();
    for (client, clock) in walked.begins {
        let client = ClientID::new(client);
        if clock > u64::from(known.get(&client)) {
            return Err(UpdateError::MissingChanges);
        }
        if !known.contains_client(&client) {
            clients += 1;
        }
    }
    let update =
        Update::decode_v1(update).map_err(|error| UpdateError::Unreadable(error.to_string()))?;
    if has_both_origins
        && (has_squash_pair
            || multi_unit_clients.iter().any(|client| {
                referenced_clients.contains(client)
                    || update.delete_set().get(&ClientID::new(*client)).is_some()
            }))
    {
        flags.check_origins_before_apply = true;
    }
    Ok((update, clients, flags))
}

/// Checks post-apply invariants on `doc`: no waiting changes and no unexpected roots.
pub(crate) fn check_applied(doc: &Doc) -> Result<(), UpdateError> {
    let txn = doc.transact();
    if txn.has_missing_updates() {
        return Err(UpdateError::MissingChanges);
    }
    for (name, root) in txn.root_refs() {
        if name != FIELD || !matches!(root, Out::YXmlFragment(_)) {
            return Err(UpdateError::UnexpectedRoot(name.to_owned()));
        }
    }
    Ok(())
}

/// Maps a pre-apply [`Refusal`] from [`reading::check_update_origins`] to an
/// [`UpdateError`], preserving [`UpdateError::MissingChanges`] for out-of-step updates.
fn origin_refusal(refusal: Refusal) -> UpdateError {
    match refusal {
        Refusal::Gap => UpdateError::MissingChanges,
        other => UpdateError::Unreadable(other.reason().to_owned()),
    }
}

/// Encodes `doc`'s state, checks `state_bytes`, enforces strict origin placement, counts
/// reading steps and client changes, and verifies client end clocks — without dropping
/// and re-decoding `doc` (settle re-read runs at open, fold, and close instead).
///
/// When `limits.reading_steps` is `None` (so conflict-list reachability is not walked) or
/// when `reading::count_strict` detects non-YATA origins, falls back to [`settled_from`]
/// so non-deterministic states are still caught.
pub(crate) fn counted_sized(doc: Doc, limits: &UpdateLimits) -> Result<Settled, UpdateError> {
    let state = state_of(&doc);
    let state_bytes = state.len();
    if state_bytes > limits.state_bytes {
        return Err(UpdateError::DocumentTooLarge {
            size: state_bytes,
            limit: limits.state_bytes,
        });
    }
    let unreadable = |refusal: Refusal| UpdateError::LeavesUnreadable(refusal.reason().to_owned());
    let Some(limit) = limits.reading_steps else {
        // Without reading-step counting, `Replay::resolve` does not walk conflict lists
        // to detect unreachable `right_origin`s; settle directly to preserve invariants.
        let mut settled = settled_from(doc, state, limits)?;
        settled.needs_settle = true;
        return Ok(settled);
    };
    let (read, reading_cache) =
        reading::count_strict_with_cache(&state, limit).map_err(unreadable)?;
    check_client_changes(read.walked.most, limits)?;
    check_total_changes(read.walked.changes, limits)?;
    check_ends(&doc, &read.ends).map_err(unreadable)?;
    if read.steps > limit {
        return Ok(Settled {
            doc,
            state: Some(state),
            state_bytes,
            changes: read.walked.changes,
            client_changes: read.walked.most,
            reading_steps: read.steps,
            steps_ahead: read.ahead,
            outcome: Outcome::Remake {
                steps: read.steps,
                limit,
            },
            needs_settle: read.needs_settle,
            doc_rebuilt: false,
            delta: None,
            mutated: true,
            placing_steps: 0,
            reading_cache: None,
        });
    }
    if read.needs_settle {
        let mut settled = settled_from(doc, state, limits)?;
        settled.needs_settle = true;
        return Ok(settled);
    }
    Ok(Settled {
        doc,
        state: Some(state),
        state_bytes,
        changes: read.walked.changes,
        client_changes: read.walked.most,
        reading_steps: read.steps,
        steps_ahead: read.ahead,
        outcome: Outcome::Settled,
        needs_settle: false,
        doc_rebuilt: false,
        delta: None,
        mutated: true,
        placing_steps: 0,
        reading_cache,
    })
}

/// The Y document as it would be with a client's update: a copy of `doc` with the
/// update applied and strictly checked. `doc` itself is not changed.
///
/// What comes back has still to be projected and, where that fails, repaired: this only
/// refuses what cannot be taken at all. [`crate::take_update`] does all of it, and says
/// what a server that calls it keeps in mind about time.
///
/// Not everything an update may write is read by the projection: attributes the schema
/// does not have, attributes of a text. No editor reads them
/// either. They stay in the document, and [`UpdateLimits::state_bytes`] is what bounds
/// them.
///
/// # Errors
///
/// Returns an [`UpdateError`] if the update is too large, is no update, cannot be
/// applied, leaves a part of itself waiting for changes the document does not have,
/// writes to another root than the body's, leaves the document too large or with
/// misplaced origins, or if `doc` itself cannot be read again.
#[cfg(any(test, feature = "test-util"))]
pub(crate) fn with_update(
    doc: &Doc,
    update: &[u8],
    limits: &UpdateLimits,
) -> Result<Doc, UpdateError> {
    with_update_sized(doc, update, limits).map(|settled| settled.doc)
}

/// [`with_update`], with the encoded state and counts of what comes back.
pub(crate) fn with_update_sized(
    doc: &Doc,
    update: &[u8],
    limits: &UpdateLimits,
) -> Result<Settled, UpdateError> {
    if update.len() > limits.update_bytes {
        return Err(UpdateError::TooLarge {
            size: update.len(),
            limit: limits.update_bytes,
        });
    }
    let (sent, clients, flags) = read(doc, update)?;
    // Before the document is read, which takes the square of its clients.
    if let Some(limit) = limits.clients
        && clients > limit
    {
        return Err(UpdateError::TooManyClients { clients, limit });
    }
    let state = state_of(doc);
    let (copy, walked) = read_state(&state).map_err(UpdateError::DocumentUnreadable)?;
    let mut placing_steps = 0_u64;
    if let Some(limit) = limits.placing_steps {
        deletions_in_order(&sent)?;
        let steps = charged(&copy, (&state, walked.changes), (update, &sent), limit)?;
        if steps > limit {
            return Err(UpdateError::PlacedPastTooMany { steps, limit });
        }
        placing_steps = steps;
    }
    if flags.check_origins_before_apply {
        let known = copy.transact().state_vector();
        reading::check_update_origins(&state, &known, update).map_err(origin_refusal)?;
    }
    let result = apply_and_check(
        ApplyTarget {
            doc: copy,
            reading_cache: None,
            prior_state: Some(&state),
            placing_steps,
        },
        sent,
        flags,
        limits,
        DeltaMode::Skip,
    );
    drop(state);
    result
}

/// Refuses an update whose deletions of a client go back or overlap. The library walks
/// the blocks of a client from the start of each range to its end, deleted or not, in
/// the order the ranges are written: one range written 16,000 times over a client of
/// 75,000 blocks took 7 to 13 s. Yjs writes the ranges of a client in order and merged,
/// so an editor's update always passes.
fn deletions_in_order(update: &Update) -> Result<(), UpdateError> {
    for (client, ranges) in update.delete_set().iter() {
        let mut end = 0;
        for range in ranges.iter() {
            if range.start < end {
                return Err(UpdateError::DeletionsOutOfOrder {
                    client: client.get(),
                    start: range.start,
                    end,
                });
            }
            end = range.end;
        }
    }
    Ok(())
}

/// What the changes of an update are charged for the document of a state
/// ([`placing::charge`]): `copy` is the document as the library read it from `state`,
/// which holds `changes`, and `sent` the update as the library read it from `update`.
pub(crate) fn charged(
    copy: &Doc,
    (state, changes): (&[u8], u64),
    (update, sent): (&[u8], &Update),
    limit: u64,
) -> Result<u64, UpdateError> {
    let held = Held {
        state,
        changes,
        ends: &copy.transact().state_vector(),
    };
    let sent = Sent {
        update,
        covered: &sent.insertions(true),
        deleted: sent.delete_set(),
    };
    placing::charge(&held, &sent, limit)
        .map_err(|refusal| UpdateError::Unreadable(refusal.reason().to_owned()))
}

/// What the changes of `update` are charged for `doc`, counted list by list whatever
/// the update holds.
#[cfg(any(test, feature = "test-util"))]
pub(crate) fn placing_steps(doc: &Doc, update: &[u8]) -> Result<u64, UpdateError> {
    let (sent, _, _) = read(doc, update)?;
    let state = state_of(doc);
    let (copy, walked) = read_state(&state).map_err(UpdateError::DocumentUnreadable)?;
    // Past a limit of nothing every update is counted list by list.
    charged(&copy, (&state, walked.changes), (update, &sent), 0)
}

/// Whether validating or settling a state stayed within [`UpdateLimits::reading_steps`]
/// or stopped because reading its state would take more steps than allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Within [`UpdateLimits::reading_steps`] (or uncounted).
    Settled,
    /// Reading its state would take `steps` steps, past `limit`. Must be made again
    /// from its body before its state is stored or another update is taken.
    Remake { steps: u64, limit: u64 },
}

/// A document with its encoded state bytes, total change count, the most changes of one
/// of its clients, the steps the library takes to read it from its state, and the most
/// it may take once one more client has written into it (0 where they are not counted).
#[derive(Debug)]
pub(crate) struct Settled {
    pub(crate) doc: Doc,
    pub(crate) state: Option<Vec<u8>>,
    pub(crate) state_bytes: usize,
    pub(crate) changes: u64,
    pub(crate) client_changes: u64,
    pub(crate) reading_steps: u64,
    pub(crate) steps_ahead: u64,
    pub(crate) outcome: Outcome,
    pub(crate) needs_settle: bool,
    /// True when `doc` was replaced by a freshly decoded `Doc` during settling (so any
    /// prior in-place `BlockCache` must be rebuilt from scratch).
    pub(crate) doc_rebuilt: bool,
    /// Captured in-place mutation delta when `DeltaMode::Capture` was requested and `doc`
    /// stayed in place (`!doc_rebuilt`).
    pub(crate) delta: Option<ApplyDelta>,
    /// Whether the applied update actually mutated the document.
    pub(crate) mutated: bool,
    /// Placing steps charged to this update (`0` when `limits.placing_steps` is `None`).
    pub(crate) placing_steps: u64,
    /// Synchronized Pass 5–6 [`ReadingCache`] when the state is conflict-free and cacheable.
    pub(crate) reading_cache: Option<Box<ReadingCache>>,
}

/// Refuses a state whose client with the most changes is past
/// [`UpdateLimits::client_changes`].
pub(crate) fn check_client_changes(changes: u64, limits: &UpdateLimits) -> Result<(), UpdateError> {
    if let Some(limit) = limits.client_changes
        && changes > limit
    {
        return Err(UpdateError::TooManyChanges { changes, limit });
    }
    Ok(())
}

/// Refuses a state whose total changes across all clients are past
/// [`UpdateLimits::total_changes`].
pub(crate) fn check_total_changes(changes: u64, limits: &UpdateLimits) -> Result<(), UpdateError> {
    if let Some(limit) = limits.total_changes
        && changes > limit
    {
        return Err(UpdateError::TooManyTotalChanges { changes, limit });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use yrs::{Any, Number, Xml, XmlElementPrelim, XmlFragment};

    use super::*;
    use crate::build::new_doc;
    use crate::settle::doc_of_state;

    const LIMITS: UpdateLimits = UpdateLimits {
        update_bytes: 1024,
        state_bytes: 1024,
        body_bytes: 1024,
        placing_steps: Some(PLACING_STEPS),
        clients: Some(MAX_CLIENTS),
        client_changes: Some(MAX_CLIENT_CHANGES),
        total_changes: Some(MAX_TOTAL_CHANGES),
        reading_steps: Some(READING_STEPS),
    };

    /// A document that holds what the library writes in a form the walk refuses: a
    /// whole number past 53 bits, put in here instead of by an update, as a part of the
    /// library that the walk does not know of could.
    fn unreadable() -> Doc {
        let doc = new_doc();
        let root = doc.get_or_insert_xml_fragment(FIELD);
        let mut txn = doc.transact_mut();
        let element = root.push_back(&mut txn, XmlElementPrelim::empty("paragraph"));
        element.insert_attribute(&mut txn, "n", Any::Number(Number::Int((1 << 53) + 1)));
        drop(txn);
        doc
    }

    #[test]
    fn a_document_that_cannot_be_read_again_is_refused_and_does_not_panic() {
        let doc = unreadable();
        assert!(
            doc_of_state(&state_of(&doc), None).is_err(),
            "its state is not read"
        );
        assert!(
            matches!(copy_of(&doc, None), Err(UpdateError::DocumentUnreadable(_))),
            "no copy"
        );
        let empty = [0, 0];
        assert!(
            matches!(
                with_update(&doc, &empty, &LIMITS),
                Err(UpdateError::DocumentUnreadable(_))
            ),
            "no update is taken for it"
        );
    }

    #[test]
    fn a_state_is_read_again_only_whole() {
        let doc = new_doc();
        assert!(
            doc_of_state(&state_of(&doc), None).is_ok(),
            "the empty document"
        );
        // A change of client 5 that begins at clock 3: the changes before it are missing.
        let mut later = vec![1, 1, 5, 3, 4, 1, 7];
        later.extend_from_slice(b"default");
        later.extend_from_slice(&[1, b'a', 0]);
        assert!(doc_of_state(&later, None).is_err(), "a state with a hole");
    }
}
