//! Reading stored states and settling documents by re-reading their encoded states
//! (`same_state`), run at open, fold, and close rather than on every live update.

use yrs::updates::decoder::Decode;
use yrs::{ClientID, Doc, ReadTxn, Transact, Update};

use crate::build::doc_of_client;
use crate::reading::{self, ReadingCache};
use crate::scan::{self, Refusal};
use crate::update::{
    Outcome, Settled, UpdateError, UpdateLimits, check_applied, check_client_changes,
    check_total_changes, state_of,
};

/// The Y document whose state is `state`, read as any update is read: past the walk
/// (`scan`), with every change it builds on, and with nothing left waiting. It writes as
/// `writer`, or as a client new to it if `None`.
///
/// # Errors
///
/// Returns why the state is no whole document.
pub(crate) fn doc_of_state(state: &[u8], writer: Option<u64>) -> Result<Doc, String> {
    let walked = scan::check(state).map_err(|refusal| refusal.reason().to_owned())?;
    read_walked(state, &walked, writer)
}

/// [`doc_of_state`], with what the walk found of the state: how many changes it holds,
/// and the most of one client.
pub(crate) fn read_state(state: &[u8]) -> Result<(Doc, scan::Walked), String> {
    let walked = scan::check(state).map_err(|refusal| refusal.reason().to_owned())?;
    read_walked(state, &walked, None).map(|doc| (doc, walked))
}

/// The Y document of a state the walk has taken, which writes as `writer`, or as a
/// client new to it if `None`.
fn read_walked(state: &[u8], walked: &scan::Walked, writer: Option<u64>) -> Result<Doc, String> {
    if walked.begins.iter().any(|(_, clock)| *clock > 0) {
        return Err(UpdateError::MissingChanges.to_string());
    }
    let update = Update::decode_v1(state).map_err(|error| error.to_string())?;
    let doc = doc_of_client(writer);
    {
        let mut txn = doc.transact_mut();
        txn.apply_update(update)
            .map_err(|error| error.to_string())?;
        if txn.has_missing_updates() {
            return Err(UpdateError::MissingChanges.to_string());
        }
    }
    Ok(doc)
}

/// A document read from a state, with what was counted of the state before the library
/// read it.
struct Counted {
    doc: Doc,
    /// How many changes the state holds.
    changes: u64,
    /// The most changes of one client.
    client_changes: u64,
    /// The steps the reading took; 0 where they are not counted.
    reading_steps: u64,
    /// The most steps it may take once one more client has written; 0 likewise.
    steps_ahead: u64,
    /// Synchronized Pass 5–6 cache built during the strict count pass, if cacheable.
    reading_cache: Option<Box<ReadingCache>>,
}

/// What [`count_and_read`] did with a state.
enum ReadOutcome {
    /// Within [`UpdateLimits::reading_steps`] (or uncounted) and read back into a fresh
    /// `Doc` (after dropping the prior `Doc`).
    Read(Counted),
    /// Past [`UpdateLimits::reading_steps`]: the library did not read it, and the prior
    /// `Doc` comes back unsettled.
    PastLimit {
        doc: Doc,
        changes: u64,
        client_changes: u64,
        steps: u64,
        steps_ahead: u64,
        limit: u64,
    },
}

/// Counts `state` against [`UpdateLimits::reading_steps`], [`UpdateLimits::client_changes`],
/// [`UpdateLimits::total_changes`], and strict origin placement; when within the reading-step
/// limit, drops `doc` before reading `state` back into a fresh `Doc` so only one `Doc` is live
/// in memory at a time. A state past the limit does not reach the library and keeps `doc` in
/// [`ReadOutcome::PastLimit`].
fn count_and_read(
    doc: Doc,
    state: &[u8],
    limits: &UpdateLimits,
) -> Result<ReadOutcome, UpdateError> {
    let unreadable = |refusal: Refusal| UpdateError::LeavesUnreadable(refusal.reason().to_owned());
    let Some(limit) = limits.reading_steps else {
        let read = reading::check_origins(state).map_err(unreadable)?;
        check_client_changes(read.walked.most, limits)?;
        check_total_changes(read.walked.changes, limits)?;
        drop(doc);
        let again =
            read_walked(state, &read.walked, None).map_err(UpdateError::LeavesUnreadable)?;
        check_applied(&again)?;
        check_ends(&again, &read.ends).map_err(unreadable)?;
        return Ok(ReadOutcome::Read(Counted {
            doc: again,
            changes: read.walked.changes,
            client_changes: read.walked.most,
            reading_steps: 0,
            steps_ahead: 0,
            reading_cache: None,
        }));
    };
    let (read, reading_cache) =
        reading::count_strict_with_cache(state, limit).map_err(unreadable)?;
    check_client_changes(read.walked.most, limits)?;
    check_total_changes(read.walked.changes, limits)?;
    if read.steps > limit {
        return Ok(ReadOutcome::PastLimit {
            doc,
            changes: read.walked.changes,
            client_changes: read.walked.most,
            steps: read.steps,
            steps_ahead: read.ahead,
            limit,
        });
    }
    drop(doc);
    let again = read_walked(state, &read.walked, None).map_err(UpdateError::LeavesUnreadable)?;
    check_applied(&again)?;
    check_ends(&again, &read.ends).map_err(unreadable)?;
    Ok(ReadOutcome::Read(Counted {
        doc: again,
        changes: read.walked.changes,
        client_changes: read.walked.most,
        reading_steps: read.steps,
        steps_ahead: read.ahead,
        reading_cache,
    }))
}

/// Validates and reads `state` back into a fresh `Doc` without dropping any caller-owned
/// `Doc` beforehand, returning an error if `state` exceeds `state_bytes`,
/// `client_changes`, `total_changes`, `reading_steps`, fails strict origin checks, or fails
/// post-apply invariants.
fn count_and_read_fresh(state: &[u8], limits: &UpdateLimits) -> Result<Counted, UpdateError> {
    if state.len() > limits.state_bytes {
        return Err(UpdateError::DocumentTooLarge {
            size: state.len(),
            limit: limits.state_bytes,
        });
    }
    let unreadable = |refusal: Refusal| UpdateError::LeavesUnreadable(refusal.reason().to_owned());
    let (read, reading_steps, steps_ahead, reading_cache) =
        if let Some(limit) = limits.reading_steps {
            let (read, reading_cache) =
                reading::count_strict_with_cache(state, limit).map_err(unreadable)?;
            check_client_changes(read.walked.most, limits)?;
            check_total_changes(read.walked.changes, limits)?;
            if read.steps > limit {
                return Err(UpdateError::TooManyReadingSteps {
                    steps: read.steps,
                    limit,
                });
            }
            let steps = read.steps;
            let ahead = read.ahead;
            (read, steps, ahead, reading_cache)
        } else {
            let read = reading::check_origins(state).map_err(unreadable)?;
            check_client_changes(read.walked.most, limits)?;
            check_total_changes(read.walked.changes, limits)?;
            (read, 0, 0, None)
        };
    let doc = read_walked(state, &read.walked, None).map_err(UpdateError::LeavesUnreadable)?;
    check_applied(&doc)?;
    check_ends(&doc, &read.ends).map_err(unreadable)?;
    Ok(Counted {
        doc,
        changes: read.walked.changes,
        client_changes: read.walked.most,
        reading_steps,
        steps_ahead,
        reading_cache,
    })
}

/// Verifies that the library read every client's changes to the same length as
/// `reading::count_strict` / `reading::check_origins`: different lengths would mean
/// neighbours lay in other places.
pub(crate) fn check_ends(doc: &Doc, expected: &[(u64, u64)]) -> Result<(), Refusal> {
    let ends = doc.transact().state_vector();
    let as_the_library = ends.len() == expected.len()
        && expected
            .iter()
            .all(|(client, end)| u64::from(ends.get(&ClientID::new(*client))) == *end);
    if as_the_library {
        Ok(())
    } else {
        Err(Refusal::Lengths)
    }
}

/// Opens a stored state directly, enforcing strict origin placement and counting its
/// reading steps only once when `state` is already canonical (`same_state`). A state
/// past [`UpdateLimits::reading_steps`] is refused before `yrs` reads it.
pub(crate) fn from_state_settled(
    state: &[u8],
    limits: &UpdateLimits,
) -> Result<Settled, UpdateError> {
    if state.len() > limits.state_bytes {
        return Err(UpdateError::DocumentTooLarge {
            size: state.len(),
            limit: limits.state_bytes,
        });
    }
    let map_refusal = |refusal: Refusal| match refusal {
        Refusal::Gap => UpdateError::MissingChanges,
        other => UpdateError::Unreadable(other.reason().to_owned()),
    };
    let (read, steps, ahead, reading_cache) = if let Some(limit) = limits.reading_steps {
        let (read, reading_cache) =
            reading::count_strict_with_cache(state, limit).map_err(map_refusal)?;
        let (steps, ahead) = (read.steps, read.ahead);
        (read, steps, ahead, reading_cache)
    } else {
        let read = reading::check_origins(state).map_err(map_refusal)?;
        (read, 0, 0, None)
    };
    if let Some(clients_limit) = limits.clients
        && read.walked.begins.len() > clients_limit
    {
        return Err(UpdateError::TooManyClients {
            clients: read.walked.begins.len(),
            limit: clients_limit,
        });
    }
    check_client_changes(read.walked.most, limits)?;
    check_total_changes(read.walked.changes, limits)?;
    if let Some(limit) = limits.reading_steps
        && steps > limit
    {
        return Err(UpdateError::TooManyReadingSteps { steps, limit });
    }
    let update =
        Update::decode_v1(state).map_err(|error| UpdateError::Unreadable(error.to_string()))?;
    let doc = doc_of_client(None);
    {
        let mut txn = doc.transact_mut();
        txn.apply_update(update)
            .map_err(|error| UpdateError::Unappliable(error.to_string()))?;
    }
    check_applied(&doc)?;
    check_ends(&doc, &read.ends)
        .map_err(|refusal| UpdateError::LeavesUnreadable(refusal.reason().to_owned()))?;
    let encoded = state_of(&doc);
    if same_state(&encoded, state) {
        let state_bytes = encoded.len();
        let reading_cache = reading_cache.filter(|rc| {
            let kept = rc.state_bytes() == state_bytes;
            debug_assert!(
                kept,
                "reading cache counted {} bytes of a {state_bytes}-byte state",
                rc.state_bytes()
            );
            kept
        });
        return Ok(Settled {
            doc,
            state: Some(encoded),
            state_bytes,
            changes: read.walked.changes,
            client_changes: read.walked.most,
            reading_steps: steps,
            steps_ahead: ahead,
            outcome: Outcome::Settled,
            needs_settle: read.needs_settle,
            doc_rebuilt: true,
            delta: None,
            mutated: true,
            placing_steps: 0,
            reading_cache,
        });
    }
    settled_from(doc, encoded, limits)
}

/// Replays accepted updates `since` onto a checkpoint `base` in a single decode pass
/// without performing extra canonicalizing re-decodes.
pub(crate) fn replay_tail<U: AsRef<[u8]>>(base: &[u8], since: &[U]) -> Result<Doc, UpdateError> {
    let doc = doc_of_state(base, None).map_err(UpdateError::DocumentUnreadable)?;
    for entry in since {
        let bytes = entry.as_ref();
        scan::check(bytes)
            .map_err(|refusal| UpdateError::Unreadable(refusal.reason().to_owned()))?;
        let update =
            Update::decode_v1(bytes).map_err(|error| UpdateError::Unreadable(error.to_string()))?;
        let mut txn = doc.transact_mut();
        txn.apply_update(update)
            .map_err(|error| UpdateError::Unappliable(error.to_string()))?;
        if txn.has_missing_updates() {
            return Err(UpdateError::MissingChanges);
        }
    }
    Ok(doc)
}

/// Rebuilds a document from a checkpoint `base` and the accepted updates and repairs
/// `since` it, enforcing [`crate::READING_STEPS`] before every canonicalizing re-decode.
pub(crate) fn rebuild<U: AsRef<[u8]>>(base: &[u8], since: &[U]) -> Result<Doc, UpdateError> {
    let doc = replay_tail(base, since)?;
    if since.is_empty() {
        return Ok(doc);
    }
    let state = state_of(&doc);
    drop(doc);
    let check_steps = |bytes: &[u8]| -> Result<(), UpdateError> {
        let (read, _) = reading::count_strict_with_cache(bytes, crate::READING_STEPS)
            .map_err(|refusal| UpdateError::LeavesUnreadable(refusal.reason().to_owned()))?;
        if read.steps > crate::READING_STEPS {
            return Err(UpdateError::TooManyReadingSteps {
                steps: read.steps,
                limit: crate::READING_STEPS,
            });
        }
        Ok(())
    };
    check_steps(&state)?;
    let again = doc_of_state(&state, None).map_err(UpdateError::LeavesUnreadable)?;
    let state_again = state_of(&again);
    if same_state(&state_again, &state) {
        return Ok(again);
    }
    drop(again);
    check_steps(&state_again)?;
    let twice = doc_of_state(&state_again, None).map_err(UpdateError::LeavesUnreadable)?;
    let state_settled = state_of(&twice);
    if !same_state(&state_settled, &state_again) {
        return Err(UpdateError::LeavesUnreadable(
            "its state reads to a document whose state reads to another".to_owned(),
        ));
    }
    Ok(twice)
}

/// A document as a server that stored it reads it: its state within the limit of size,
/// read again within the steps a reading may take, and reading again to the same changes
/// and deletions at the latest the second time ([`same_state`]). Where reading its state
/// would take more steps than [`UpdateLimits::reading_steps`], returns `doc` unsettled
/// with [`Outcome::Remake`] instead of handing the slow state to the library.
pub(crate) fn settled_sized(doc: Doc, limits: &UpdateLimits) -> Result<Settled, UpdateError> {
    let state = state_of(&doc);
    settled_from(doc, state, limits)
}

/// [`settled_sized`] given `doc` and its already-encoded `state`.
pub(crate) fn settled_from(
    doc: Doc,
    state: Vec<u8>,
    limits: &UpdateLimits,
) -> Result<Settled, UpdateError> {
    if state.len() > limits.state_bytes {
        return Err(UpdateError::DocumentTooLarge {
            size: state.len(),
            limit: limits.state_bytes,
        });
    }
    let again = match count_and_read(doc, &state, limits)? {
        ReadOutcome::Read(again) => again,
        ReadOutcome::PastLimit {
            doc,
            changes,
            client_changes,
            steps,
            steps_ahead,
            limit,
        } => {
            return Ok(rebuilt_remake(
                doc,
                state,
                changes,
                client_changes,
                steps,
                steps_ahead,
                limit,
            ));
        }
    };
    let state_again = state_of(&again.doc);
    if same_state(&state_again, &state) {
        return Ok(rebuilt_settled(again, state_again, 0));
    }
    drop(state);
    if state_again.len() > limits.state_bytes {
        return Err(UpdateError::DocumentTooLarge {
            size: state_again.len(),
            limit: limits.state_bytes,
        });
    }
    // A first reading may write the same document otherwise (neighbouring changes it
    // joins); a second must not. What it writes is what is stored and read from then
    // on, and a change it joined is cut again by every reading: counted as well.
    let read_twice = match count_and_read(again.doc, &state_again, limits)? {
        ReadOutcome::Read(read_twice) => read_twice,
        ReadOutcome::PastLimit {
            doc,
            changes,
            client_changes,
            steps,
            steps_ahead,
            limit,
        } => {
            return Ok(rebuilt_remake(
                doc,
                state_again,
                changes,
                again.client_changes.max(client_changes),
                steps,
                steps_ahead,
                limit,
            ));
        }
    };
    let state_settled = state_of(&read_twice.doc);
    if !same_state(&state_settled, &state_again) {
        return Err(UpdateError::LeavesUnreadable(
            "its state reads to a document whose state reads to another".to_owned(),
        ));
    }
    Ok(rebuilt_settled(
        read_twice,
        state_settled,
        again.client_changes,
    ))
}

fn rebuilt_settled(counted: Counted, state: Vec<u8>, prior_client_changes: u64) -> Settled {
    let state_bytes = state.len();
    let reading_cache = counted.reading_cache.filter(|rc| {
        let kept = rc.state_bytes() == state_bytes;
        debug_assert!(
            kept,
            "reading cache counted {} bytes of a {state_bytes}-byte state",
            rc.state_bytes()
        );
        kept
    });
    Settled {
        doc: counted.doc,
        state: Some(state),
        state_bytes,
        changes: counted.changes,
        client_changes: counted.client_changes.max(prior_client_changes),
        reading_steps: counted.reading_steps,
        steps_ahead: counted.steps_ahead,
        outcome: Outcome::Settled,
        needs_settle: false,
        doc_rebuilt: true,
        delta: None,
        mutated: true,
        placing_steps: 0,
        reading_cache,
    }
}

fn rebuilt_remake(
    doc: Doc,
    state: Vec<u8>,
    changes: u64,
    client_changes: u64,
    steps: u64,
    steps_ahead: u64,
    limit: u64,
) -> Settled {
    let state_bytes = state.len();
    Settled {
        doc,
        state: Some(state),
        state_bytes,
        changes,
        client_changes,
        reading_steps: steps,
        steps_ahead,
        outcome: Outcome::Remake { steps, limit },
        needs_settle: true,
        doc_rebuilt: true,
        delta: None,
        mutated: true,
        placing_steps: 0,
        reading_cache: None,
    }
}

/// Whether [`settle_keeping`] converged on pass 1 with an unchanged state (so a prior
/// [`crate::block_cache::BlockCache`] remains exact) or required a second pass that
/// re-canonicalized blocks (requiring cache rebuild).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettlePass {
    Unchanged,
    Converged,
}

/// Settles `doc` from its already-encoded `state` (at fold or close) while keeping `doc`
/// intact on any error so the caller can project its body and remake it without losing
/// updates.
pub(crate) fn settle_keeping(
    doc: Doc,
    state: Vec<u8>,
    limits: &UpdateLimits,
) -> Result<(Settled, SettlePass), (Doc, UpdateError)> {
    let again = match count_and_read_fresh(&state, limits) {
        Ok(again) => again,
        Err(error) => return Err((doc, error)),
    };
    let state_again = state_of(&again.doc);
    if same_state(&state_again, &state) {
        drop(doc);
        return Ok((
            rebuilt_settled(again, state_again, 0),
            SettlePass::Unchanged,
        ));
    }
    drop(state);
    let first_client_changes = again.client_changes;
    drop(again);
    let read_twice = match count_and_read_fresh(&state_again, limits) {
        Ok(read_twice) => read_twice,
        Err(error) => return Err((doc, error)),
    };
    let state_settled = state_of(&read_twice.doc);
    if !same_state(&state_settled, &state_again) {
        return Err((
            doc,
            UpdateError::LeavesUnreadable(
                "its state reads to a document whose state reads to another".to_owned(),
            ),
        ));
    }
    drop(doc);
    Ok((
        rebuilt_settled(read_twice, state_settled, first_client_changes),
        SettlePass::Converged,
    ))
}

/// Whether two states represent the same changes and deletions regardless of hash-table
/// encoding order (the library writes each client's deletions in table order). Misplaced
/// origins that could read non-deterministically are refused earlier during strict origin
/// validation.
#[must_use]
pub fn same_state(one: &[u8], other: &[u8]) -> bool {
    one == other
        || matches!(
            (Update::decode_v1(one), Update::decode_v1(other)),
            (Ok(one), Ok(other)) if one == other
        )
}
