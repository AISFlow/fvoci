//! Checkpoint state, refusal mapping, and lazy-snapshot helpers for [`super::Worker`].

use fvoci_collab::{BodyDoc, TakeError, TakenInPlace, UpdateError};
use fvoci_model::{Body, BodyError};

use super::super::wire::{Refusal, Refused, Share};

/// How many accepted updates/repairs may accumulate after the last state snapshot before
/// the worker takes a fresh lazy snapshot (`last_state`), bounding rollback replay work.
pub(super) const SNAPSHOT_SINCE_GAP: usize = 64;

/// Maximum accumulated placing steps since the last snapshot before taking a fresh
/// lazy snapshot (`last_state`), preventing slow updates from amplifying rollback work.
pub(super) const SNAPSHOT_REPLAY_STEPS: u64 = 16_384;

/// A document, its checkpoint (`base`) and accepted updates/repairs since it (`since`)
/// for exact rollback after a rejected in-place update, an optional intermediate state
/// snapshot (`last_state` at `since[..snapshot_since_start]`), its latest state bytes
/// and change count, the share it is counted in, and whether it must be remade from its
/// body before further requests.
#[derive(Debug)]
pub(super) struct Held {
    pub(super) doc: BodyDoc,
    /// Checkpoint state as of opening, making, or the last rebase (`Request::State`,
    /// `Retake` rebase, or settled `Dirty` restore). Cleared to `Vec::new()` as soon as
    /// `last_state` is populated or `must_remake` is set so two full state copies are
    /// never held at once.
    pub(super) base: Vec<u8>,
    /// Accepted updates and repairs applied since the last rebase, replayed from
    /// `snapshot_since_start` when an in-place update is refused after mutating `doc`.
    pub(super) since: Vec<Vec<u8>>,
    /// Total bytes of every entry in `since`, counted in [`super::Worker::held_bytes`].
    pub(super) since_bytes: u64,
    /// Snapshot state as of `since[..snapshot_since_start]` once a lazy or pre-encoded
    /// snapshot has replaced `base`; `None` means `base` holds the snapshot (at
    /// `snapshot_since_start == 0`).
    pub(super) last_state: Option<Vec<u8>>,
    /// Index into `since` at which `snapshot_state()` was captured (`<= since.len()`).
    pub(super) snapshot_since_start: usize,
    /// Accumulated placing steps across `&since[snapshot_since_start..]`.
    pub(super) since_replay_steps: u64,
    pub(super) state_bytes: u64,
    pub(super) changes: u64,
    pub(super) share: Share,
    pub(super) must_remake: bool,
    /// Whether the current state (`base` when `last_state` is `None` and
    /// `snapshot_since_start == since.len()`) has already been verified by a settle
    /// re-read (`same_state`). True on open/make and after `Request::State` settles;
    /// cleared by mutating `Take`/`Retake`.
    pub(super) settled: bool,
    #[cfg(feature = "test-util")]
    pub(super) fail_next_settle: bool,
}

impl Held {
    /// Total bytes accounted against [`super::Worker::held_bytes`] for this document.
    pub(super) fn accounted_bytes(&self) -> u64 {
        self.state_bytes.saturating_add(self.since_bytes)
    }

    /// Promotes `last_state` into `base`, clears `since` (releasing its backing array),
    /// and returns the `since_bytes` freed for [`super::Worker::held_bytes`].
    pub(super) fn rebase(&mut self) -> u64 {
        if let Some(last) = self.last_state.take() {
            self.base = last;
        }
        self.since = Vec::new();
        self.snapshot_since_start = 0;
        self.since_replay_steps = 0;
        let freed = self.since_bytes;
        self.since_bytes = 0;
        freed
    }
}

/// Whether a `take` is a live client update or a replay of a logged update.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum TakeMode {
    Live,
    Retake,
}

/// The saved fields of a [`Held`] document while its [`BodyDoc`] is taken in place.
pub(super) struct Checkpoint {
    pub(super) base: Vec<u8>,
    pub(super) since: Vec<Vec<u8>>,
    pub(super) since_bytes: u64,
    pub(super) last_state: Option<Vec<u8>>,
    pub(super) snapshot_since_start: usize,
    pub(super) since_replay_steps: u64,
    pub(super) state_bytes: u64,
    pub(super) changes: u64,
    pub(super) share: Share,
    pub(super) settled: bool,
    #[cfg(feature = "test-util")]
    pub(super) fail_next_settle: bool,
}

impl Checkpoint {
    /// Records a fresh intermediate state snapshot covering all current entries of
    /// `since`, releasing `base`'s buffer so two full state copies are never held at once.
    pub(super) fn take_snapshot(&mut self, state: Vec<u8>) {
        self.base = Vec::new();
        self.last_state = Some(state);
        self.snapshot_since_start = self.since.len();
        self.since_replay_steps = 0;
    }

    /// Returns the exact current state if the latest snapshot covers all of `since`, or
    /// `None` when `&since[snapshot_since_start..]` is non-empty.
    pub(super) fn exact_current_state(&self) -> Option<&[u8]> {
        (self.snapshot_since_start == self.since.len()).then(|| self.snapshot_state())
    }

    /// Returns the latest checkpoint snapshot state (`last_state` if taken, else `base`).
    pub(super) fn snapshot_state(&self) -> &[u8] {
        self.last_state.as_deref().unwrap_or(&self.base)
    }

    /// Accepted updates/repairs applied since [`Self::snapshot_state`].
    pub(super) fn tail_since_snapshot(&self) -> &[Vec<u8>] {
        &self.since[self.snapshot_since_start..]
    }

    pub(super) fn accounted_bytes(&self) -> u64 {
        self.state_bytes.saturating_add(self.since_bytes)
    }

    pub(super) fn into_held(self, doc: BodyDoc) -> Held {
        Held {
            doc,
            base: self.base,
            since: self.since,
            since_bytes: self.since_bytes,
            last_state: self.last_state,
            snapshot_since_start: self.snapshot_since_start,
            since_replay_steps: self.since_replay_steps,
            state_bytes: self.state_bytes,
            changes: self.changes,
            share: self.share,
            must_remake: false,
            settled: self.settled,
            #[cfg(feature = "test-util")]
            fail_next_settle: self.fail_next_settle,
        }
    }
}

pub(super) fn doc_meta(changed: &TakenInPlace) -> (u64, Vec<String>) {
    let clients = count(changed.doc.clients());
    let files = changed.doc.file_ids();
    (clients, files)
}

pub(super) fn too_many_bytes() -> Refused {
    refused(
        Refusal::TooManyDocuments,
        "the worker holds as many bytes of documents as it may",
    )
}

pub(super) fn size_of(bytes: &[u8]) -> u64 {
    count(bytes.len())
}

pub(super) fn count(number: usize) -> u64 {
    u64::try_from(number).unwrap_or(u64::MAX)
}

pub(super) fn json_of(body: &Body) -> Vec<u8> {
    // A `Value` is always JSON.
    serde_json::to_vec(body.document()).unwrap_or_default()
}

pub(super) fn refused(refusal: Refusal, message: &str) -> Refused {
    Refused {
        refusal,
        message: message.to_owned(),
        rolled_back: false,
    }
}

/// The refusal of an update or a state that the document does not take.
pub(super) fn take_refused(error: &TakeError) -> Refused {
    let refusal = match error {
        TakeError::Update(UpdateError::TooLarge { .. }) => Refusal::UpdateTooLarge,
        // Deletions out of order are what no encoder writes.
        TakeError::Update(UpdateError::Unreadable(_) | UpdateError::DeletionsOutOfOrder { .. }) => {
            Refusal::Unreadable
        }
        TakeError::Update(UpdateError::MissingChanges) => Refusal::MissingChanges,
        // A document of too many clients, or of too many changes, is one too large to
        // work on in time.
        TakeError::Update(
            UpdateError::DocumentTooLarge { .. }
            | UpdateError::TooManyClients { .. }
            | UpdateError::TooManyChanges { .. }
            | UpdateError::TooManyTotalChanges { .. },
        ) => Refusal::DocumentTooLarge,
        TakeError::Update(UpdateError::TooManyReadingSteps { .. }) => Refusal::TooManySteps,
        TakeError::Body(BodyError::TableTooLarge { .. }) => Refusal::TableTooLarge,
        TakeError::Body(BodyError::TooLarge | BodyError::TooManyNodes | BodyError::TooDeep)
        | TakeError::BodyTooLarge { .. } => Refusal::BodyTooLarge,
        TakeError::Body(_) => Refusal::NoBody,
        // What cannot be applied, and what would be placed past too much of the
        // document (`UpdateError::PlacedPastTooMany`): the room says a refused update.
        _ => Refusal::Unappliable,
    };
    refused(refusal, &error.to_string())
}

/// The refusal of what the server stored or logged before and this worker does not
/// take again.
pub(super) fn logged_refused(error: &TakeError) -> Refused {
    match error {
        TakeError::Update(
            UpdateError::TooManyReadingSteps { .. } | UpdateError::TooManyTotalChanges { .. },
        ) => refused(Refusal::Unappliable, &error.to_string()),
        other => take_refused(other),
    }
}
