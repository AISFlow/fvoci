//! The worker's loop: a request in, its answer out, until the server closes the pipe.

mod checkpoint;

use std::collections::HashMap;
use std::io::{Read, Write};

use fvoci_collab::{
    BodyDoc, BodyTakeError, MAX_CLIENT_CHANGES, MAX_CLIENTS, MAX_TOTAL_CHANGES, PLACING_STEPS,
    READING_STEPS, SettleError, TakeError, UpdateLimits, same_state,
};
use fvoci_model::{BODY_MAX_BYTES, Body};

use self::checkpoint::{
    Checkpoint, Held, SNAPSHOT_REPLAY_STEPS, SNAPSHOT_SINCE_GAP, TakeMode, count, doc_meta,
    json_of, logged_refused, refused, size_of, take_refused, too_many_bytes,
};
use super::frame::{FrameError, read_frame, write_frame};
use super::wire::{
    ASIDE_DOCUMENTS, Answer, DocNumber, Limits, Refusal, Refused, Request, Response, Share, Source,
    encode_response,
};

/// The largest frame before the limits are known: the one that brings them.
const CONFIGURE_FRAME_BYTES: u64 = 64;

/// How many accepted updates and repairs a held document may keep in its rollback log
/// (`Held::since`) before further `Request::Take` updates are refused until a fold
/// rebases it. Kept above 1,000 so direct worker bursts without folds still pass while
/// keeping rollback replay bounded.
const MAX_SINCE_ENTRIES: usize = 2048;

/// How many entries in `Held::since` cause the worker to ask the server for a fold
/// (`Answer::Taken::fold`), well before [`MAX_SINCE_ENTRIES`] is reached.
const FOLD_SINCE_ENTRIES: usize = 400;

/// Answers the requests of `input` on `output`, in order, until `input` ends.
///
/// # Errors
///
/// Returns why a frame could not be read or written, or a request read: the server
/// sent something no server of this build sends, or a pipe broke. The worker ends
/// then, and the server starts another.
pub fn serve(input: &mut impl Read, output: &mut impl Write) -> Result<(), FrameError> {
    let Some(first) = read_frame(input, CONFIGURE_FRAME_BYTES)? else {
        return Ok(());
    };
    let Ok(Request::Configure(limits)) = Request::decode(&first) else {
        return Err(malformed());
    };
    let mut worker = Worker::new(limits);
    let frame_bytes = limits.frame_bytes();
    write_frame(output, &encode_response(&Ok(Answer::Done)), frame_bytes)?;
    while let Some(frame) = read_frame(input, frame_bytes)? {
        let request = Request::decode(&frame).map_err(|_| malformed())?;
        let response = worker.answer(request)?;
        write_frame(output, &encode_response(&response), frame_bytes)?;
    }
    Ok(())
}

fn malformed() -> FrameError {
    FrameError::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "the server sent no request",
    ))
}

/// The documents a worker holds, and the limits it holds them to.
#[derive(Debug)]
struct Worker {
    limits: Limits,
    update_limits: UpdateLimits,
    docs: HashMap<DocNumber, Held>,
    /// The bytes of the states and rollback logs (`since`) of all held documents.
    held_bytes: u64,
}

impl Worker {
    fn new(limits: Limits) -> Self {
        let size = |bytes: u64| usize::try_from(bytes).unwrap_or(usize::MAX);
        Self {
            limits,
            update_limits: UpdateLimits {
                update_bytes: size(limits.update_bytes),
                state_bytes: size(limits.state_bytes),
                body_bytes: size(limits.body_bytes),
                placing_steps: Some(PLACING_STEPS),
                clients: Some(MAX_CLIENTS),
                client_changes: Some(MAX_CLIENT_CHANGES),
                total_changes: Some(MAX_TOTAL_CHANGES),
                reading_steps: Some(READING_STEPS),
            },
            docs: HashMap::new(),
            held_bytes: 0,
        }
    }

    /// Hard cap on `Held::since_bytes` for one document before `Take` is refused.
    fn max_since_bytes(&self) -> u64 {
        self.limits.state_bytes
    }

    /// Soft threshold on `Held::since_bytes` that asks the server to fold soon.
    fn fold_since_bytes(&self) -> u64 {
        (self.limits.state_bytes / 2).max(1)
    }

    fn answer(&mut self, request: Request) -> Result<Response, FrameError> {
        match request {
            Request::Configure(_) => Ok(Err(refused(
                Refusal::OutOfOrder,
                "the worker is configured once",
            ))),
            Request::Open { doc, source, share } => Ok(self.open(doc, source, share)),
            Request::Take { doc, update } => self.take(
                doc,
                &update,
                self.update_limits,
                take_refused,
                Refusal::OutOfOrder,
                TakeMode::Live,
            ),
            Request::Retake { doc, update } => self.take(
                doc,
                &update,
                UpdateLimits {
                    total_changes: None,
                    ..self.logged_limits()
                },
                logged_refused,
                Refusal::Unappliable,
                TakeMode::Retake,
            ),
            Request::Append { doc, nodes } => self.append(doc, &nodes),
            Request::StateVector(doc) => Ok(self
                .usable_doc(doc, Refusal::OutOfOrder)
                .map(|body| Answer::Bytes(body.state_vector()))),
            Request::Diff { doc, state_vector } => {
                Ok(self.usable_doc(doc, Refusal::OutOfOrder).and_then(|body| {
                    body.diff(&state_vector)
                        .map(Answer::Bytes)
                        .map_err(|error| refused(Refusal::Unreadable, &error.to_string()))
                }))
            }
            Request::State(doc) => Ok(self.state_and_rebase(doc)),
            Request::Body(doc) => Ok(self.doc(doc).and_then(|body| {
                body.body()
                    .map(|body| Answer::Body(body.map(|body| json_of(&body))))
                    .map_err(|error| refused(Refusal::NoBody, &error.to_string()))
            })),
            Request::Close(doc) => {
                if let Some(held) = self.docs.remove(&doc) {
                    self.held_bytes = self.held_bytes.saturating_sub(held.accounted_bytes());
                }
                Ok(Ok(Answer::Done))
            }
            #[cfg(feature = "test-util")]
            Request::FailNextSettle(doc) => Ok(self.fail_next_settle(doc)),
        }
    }

    #[cfg(feature = "test-util")]
    fn fail_next_settle(&mut self, doc: DocNumber) -> Response {
        let held = self
            .docs
            .get_mut(&doc)
            .ok_or_else(|| refused(Refusal::UnknownDocument, "no document is open under it"))?;
        held.fail_next_settle = true;
        Ok(Answer::Done)
    }

    /// Limits used when settling an unsettled document on `Request::State` or during
    /// non-canonical `Dirty` rollback: allows logged/larger states within `held_bytes`
    /// while preserving `reading_steps` and strict origin checks.
    fn settle_limits(&self) -> UpdateLimits {
        UpdateLimits {
            reading_steps: self.update_limits.reading_steps,
            ..self.logged_limits()
        }
    }

    /// Transitions a document whose settle check failed into `must_remake` mode while
    /// keeping `doc_body` intact for `Request::Body` and reclaiming `since_bytes`.
    fn mark_unsettled_for_remake(
        &mut self,
        doc: DocNumber,
        doc_body: BodyDoc,
        old_state_bytes: u64,
        old_changes: u64,
        share: Share,
        base_held: u64,
    ) {
        self.held_bytes = base_held.saturating_add(old_state_bytes);
        self.docs.insert(
            doc,
            Held {
                doc: doc_body,
                base: Vec::new(),
                since: Vec::new(),
                since_bytes: 0,
                last_state: None,
                snapshot_since_start: 0,
                since_replay_steps: 0,
                state_bytes: old_state_bytes,
                changes: old_changes,
                share,
                must_remake: true,
                settled: false,
                #[cfg(feature = "test-util")]
                fail_next_settle: false,
            },
        );
    }

    /// Settles (if unsettled) and returns the document's current canonical state bytes,
    /// rebasing its checkpoint and reclaiming `since_bytes` from [`Self::held_bytes`].
    ///
    /// If settling fails or would overflow `held_bytes`, keeps the live document intact
    /// in `must_remake` mode and returns [`Answer::Remake`] so the server projects its
    /// body and remakes it into a fresh 0-step document without losing updates.
    fn state_and_rebase(&mut self, doc: DocNumber) -> Response {
        let force_fail = {
            let held = self
                .docs
                .get_mut(&doc)
                .ok_or_else(|| refused(Refusal::UnknownDocument, "no document is open under it"))?;
            if held.must_remake {
                return Err(refused(
                    Refusal::OutOfOrder,
                    "the document must be made again from its body before another request",
                ));
            }
            #[cfg(feature = "test-util")]
            let force_fail = {
                let fail = held.fail_next_settle;
                held.fail_next_settle = false;
                fail
            };
            #[cfg(not(feature = "test-util"))]
            let force_fail = false;
            if held.settled && !force_fail {
                let freed = held.rebase();
                self.held_bytes = self.held_bytes.saturating_sub(freed);
                return Ok(Answer::Bytes(held.base.clone()));
            }
            force_fail
        };
        let held = self.docs.remove(&doc).expect("checked open above");
        let base_held = self.held_bytes.saturating_sub(held.accounted_bytes());
        let old_state_bytes = held.state_bytes;
        let old_changes = held.changes;
        let share = held.share;
        if force_fail {
            self.mark_unsettled_for_remake(
                doc,
                held.doc,
                old_state_bytes,
                old_changes,
                share,
                base_held,
            );
            return Ok(Answer::Remake);
        }
        let state_to_settle = if held.snapshot_since_start == held.since.len() {
            held.last_state.unwrap_or(held.base)
        } else {
            held.doc.state()
        };
        match held.doc.settle(state_to_settle, &self.settle_limits()) {
            Ok(settled) => {
                let new_state_bytes = count(settled.state_bytes);
                if base_held.saturating_add(new_state_bytes) > self.limits.held_bytes {
                    self.mark_unsettled_for_remake(
                        doc,
                        settled.doc,
                        old_state_bytes,
                        old_changes,
                        share,
                        base_held,
                    );
                    return Ok(Answer::Remake);
                }
                self.held_bytes = base_held.saturating_add(new_state_bytes);
                let reply = settled.state.clone();
                self.docs.insert(
                    doc,
                    Held {
                        doc: settled.doc,
                        base: settled.state,
                        since: Vec::new(),
                        since_bytes: 0,
                        last_state: None,
                        snapshot_since_start: 0,
                        since_replay_steps: 0,
                        state_bytes: new_state_bytes,
                        changes: settled.changes,
                        share,
                        must_remake: false,
                        settled: true,
                        #[cfg(feature = "test-util")]
                        fail_next_settle: false,
                    },
                );
                Ok(Answer::Bytes(reply))
            }
            Err(SettleError { doc: intact, .. }) => {
                self.mark_unsettled_for_remake(
                    doc,
                    intact,
                    old_state_bytes,
                    old_changes,
                    share,
                    base_held,
                );
                Ok(Answer::Remake)
            }
        }
    }

    /// Takes an update into a document in place within `limits`; `refuse` maps a
    /// [`TakeError`] to a refusal, and `on_remake` is returned if the held document is
    /// waiting to be remade from its body. On post-mutation refusal (or memory/since-cap
    /// overflow), restores the document in O(1) from its checkpoint state (settling a
    /// non-canonical unsettled checkpoint if needed).
    fn take(
        &mut self,
        doc: DocNumber,
        update: &[u8],
        limits: UpdateLimits,
        refuse: fn(&TakeError) -> Refused,
        on_remake: Refusal,
        mode: TakeMode,
    ) -> Result<Response, FrameError> {
        {
            let Some(held) = self.docs.get(&doc) else {
                return Ok(Err(refused(
                    Refusal::UnknownDocument,
                    "no document is open under it",
                )));
            };
            if held.must_remake {
                return Ok(Err(refused(
                    on_remake,
                    "the document must be made again from its body before another request",
                )));
            }
        }
        let held = self.docs.remove(&doc).expect("checked open above");
        let Held {
            doc: body_doc,
            base,
            since,
            since_bytes,
            last_state,
            snapshot_since_start,
            since_replay_steps,
            state_bytes: old_state_bytes,
            changes: old_changes,
            share,
            must_remake: _,
            settled,
            #[cfg(feature = "test-util")]
            fail_next_settle,
        } = held;
        let checkpoint = Checkpoint {
            base,
            since,
            since_bytes,
            last_state,
            snapshot_since_start,
            since_replay_steps,
            state_bytes: old_state_bytes,
            changes: old_changes,
            share,
            settled,
            #[cfg(feature = "test-util")]
            fail_next_settle,
        };
        let prior_state = checkpoint.exact_current_state();
        let changed = match body_doc.take_in_place(prior_state, old_changes, update, &limits) {
            Ok(changed) => changed,
            Err(BodyTakeError::Untouched { doc: intact, error }) => {
                self.docs.insert(doc, checkpoint.into_held(intact));
                return Ok(Err(refuse(&error)));
            }
            Err(BodyTakeError::Dirty(error)) => {
                let mut refusal = refuse(&error);
                refusal.rolled_back = true;
                return self.restore_or_close(doc, checkpoint, refusal);
            }
        };
        self.commit_taken(doc, update, mode, checkpoint, changed)
    }

    /// Adds `nodes`, the JSON of a list of a body's top-level nodes, after the last of a
    /// document, written by a client of its own ([`fvoci_collab::BodyDoc::append_update`]),
    /// and takes that update as a client's is taken, within the limits of a document:
    /// the update is the server's, which an update's own limit does not hold.
    fn append(&mut self, doc: DocNumber, nodes: &[u8]) -> Result<Response, FrameError> {
        let nodes: Vec<serde_json::Value> = match serde_json::from_slice(nodes) {
            Ok(nodes) => nodes,
            Err(error) => {
                return Ok(Err(refused(
                    Refusal::Unreadable,
                    &format!("the nodes are no list of JSON: {error}"),
                )));
            }
        };
        let update = match self.usable_doc(doc, Refusal::OutOfOrder) {
            Ok(body) => match body.append_update(&nodes) {
                Ok(update) => update,
                Err(error) => return Ok(Err(refused(Refusal::Unappliable, &error.to_string()))),
            },
            Err(refusal) => return Ok(Err(refusal)),
        };
        let limits = UpdateLimits {
            update_bytes: self.update_limits.state_bytes,
            ..self.update_limits
        };
        let taken = self.take(
            doc,
            &update,
            limits,
            take_refused,
            Refusal::OutOfOrder,
            TakeMode::Live,
        )?;
        Ok(taken.map(|taken| Answer::Appended {
            update,
            taken: Box::new(taken),
        }))
    }

    /// Restores `doc` after a post-mutation refusal, or, if restoration fails, closes `doc`
    /// alone in production builds so the server reopens it from what is stored while the
    /// worker keeps serving its other documents (`test-util` builds still fail loudly).
    fn restore_or_close(
        &mut self,
        doc: DocNumber,
        checkpoint: Checkpoint,
        refusal: Refused,
    ) -> Result<Response, FrameError> {
        match self.restore_after_dirty(doc, checkpoint) {
            Ok(()) => Ok(Err(refusal)),
            Err(error) => {
                debug_assert!(
                    !self.docs.contains_key(&doc),
                    "failed dirty restore must remove document"
                );
                #[cfg(feature = "test-util")]
                return Err(error);
                #[cfg(not(feature = "test-util"))]
                return Ok(Err(refused(Refusal::Closed, &error.to_string())));
            }
        }
    }

    /// Commits a successful in-place `TakenInPlace` into `self.docs`, checking `since` and
    /// `held_bytes` budgets and rolling back via [`Self::restore_after_dirty`] if either
    /// would be exceeded.
    fn commit_taken(
        &mut self,
        doc: DocNumber,
        update: &[u8],
        mode: TakeMode,
        mut checkpoint: Checkpoint,
        changed: fvoci_collab::TakenInPlace,
    ) -> Result<Response, FrameError> {
        let base_held = self.held_bytes.saturating_sub(checkpoint.accounted_bytes());
        if changed.remake {
            return self.commit_remake(doc, base_held, checkpoint, changed);
        }
        let state_bytes = count(changed.state_bytes);
        let mutated = changed.mutated;
        let added_entries = if mutated {
            1 + usize::from(changed.repair.update.is_some())
        } else {
            0
        };
        let added_bytes = if mutated {
            size_of(update).saturating_add(changed.repair.update.as_deref().map_or(0, size_of))
        } else {
            0
        };
        let next_since_bytes = checkpoint.since_bytes.saturating_add(added_bytes);
        let next_since_len = checkpoint.since.len().saturating_add(added_entries);
        let since_over =
            next_since_bytes > self.max_since_bytes() || next_since_len > MAX_SINCE_ENTRIES;
        let budget_over = base_held
            .saturating_add(state_bytes)
            .saturating_add(next_since_bytes)
            > self.limits.held_bytes;
        if mode == TakeMode::Retake && (since_over || budget_over) {
            return self.commit_retake_rebase(doc, base_held, state_bytes, checkpoint, changed);
        }
        if since_over || budget_over {
            drop(changed);
            let message = if since_over {
                "the document holds as many un-folded updates as it may"
            } else {
                "the worker holds as many bytes of documents as it may"
            };
            return self.restore_or_close(
                doc,
                checkpoint,
                refused(Refusal::TooManyDocuments, message),
            );
        }
        let (clients, files) = doc_meta(&changed);
        let repair = changed.repair.update;
        if mutated {
            checkpoint.since.push(update.to_vec());
            if let Some(repair_bytes) = &repair {
                checkpoint.since.push(repair_bytes.clone());
            }
            checkpoint.since_bytes = next_since_bytes;
            checkpoint.settled = false;
            if let Some(encoded) = changed.state {
                checkpoint.take_snapshot(encoded);
            } else {
                checkpoint.since_replay_steps = checkpoint
                    .since_replay_steps
                    .saturating_add(changed.placing_steps.max(1));
                let gap = checkpoint
                    .since
                    .len()
                    .saturating_sub(checkpoint.snapshot_since_start);
                if gap >= SNAPSHOT_SINCE_GAP
                    || checkpoint.since_replay_steps > SNAPSHOT_REPLAY_STEPS
                {
                    checkpoint.take_snapshot(changed.doc.state());
                }
            }
        }
        self.held_bytes = base_held
            .saturating_add(state_bytes)
            .saturating_add(checkpoint.since_bytes);
        let fold = checkpoint.since_bytes >= self.fold_since_bytes()
            || checkpoint.since.len() >= FOLD_SINCE_ENTRIES;
        checkpoint.state_bytes = state_bytes;
        checkpoint.changes = changed.changes;
        self.docs.insert(doc, checkpoint.into_held(changed.doc));
        Ok(Ok(Answer::Taken {
            repair,
            state_bytes,
            clients,
            changes: changed.changes,
            client_changes: changed.client_changes,
            steps_ahead: changed.steps_ahead,
            files,
            remake: false,
            fold,
        }))
    }

    /// Rebases a replayed (`Retake`) document onto its current state when its `since` log
    /// would overflow its cap or `held_bytes`, rolling back if `state_bytes` alone exceeds
    /// `held_bytes`.
    fn commit_retake_rebase(
        &mut self,
        doc: DocNumber,
        base_held: u64,
        state_bytes: u64,
        mut checkpoint: Checkpoint,
        changed: fvoci_collab::TakenInPlace,
    ) -> Result<Response, FrameError> {
        if base_held.saturating_add(state_bytes) > self.limits.held_bytes {
            drop(changed);
            return self.restore_or_close(doc, checkpoint, too_many_bytes());
        }
        let (clients, files) = doc_meta(&changed);
        let repair = changed.repair.update;
        let state = changed.state.unwrap_or_else(|| changed.doc.state());
        checkpoint.base = state;
        checkpoint.since = Vec::new();
        checkpoint.since_bytes = 0;
        checkpoint.last_state = None;
        checkpoint.snapshot_since_start = 0;
        checkpoint.since_replay_steps = 0;
        checkpoint.state_bytes = state_bytes;
        checkpoint.changes = changed.changes;
        checkpoint.settled = false;
        self.held_bytes = base_held.saturating_add(state_bytes);
        self.docs.insert(doc, checkpoint.into_held(changed.doc));
        Ok(Ok(Answer::Taken {
            repair,
            state_bytes,
            clients,
            changes: changed.changes,
            client_changes: changed.client_changes,
            steps_ahead: changed.steps_ahead,
            files,
            remake: false,
            fold: false,
        }))
    }

    /// Commits an unsettled `remake: true` update: checks `held_bytes`, drops `base` and
    /// `since` (the document only allows `Body` and `Close` until remade), and stores it.
    fn commit_remake(
        &mut self,
        doc: DocNumber,
        base_held: u64,
        checkpoint: Checkpoint,
        changed: fvoci_collab::TakenInPlace,
    ) -> Result<Response, FrameError> {
        let state_bytes = count(changed.state_bytes);
        if base_held.saturating_add(state_bytes) > self.limits.held_bytes {
            drop(changed);
            return self.restore_or_close(doc, checkpoint, too_many_bytes());
        }
        let (clients, files) = doc_meta(&changed);
        self.held_bytes = base_held.saturating_add(state_bytes);
        self.docs.insert(
            doc,
            Held {
                doc: changed.doc,
                base: Vec::new(),
                since: Vec::new(),
                since_bytes: 0,
                last_state: None,
                snapshot_since_start: 0,
                since_replay_steps: 0,
                state_bytes,
                changes: changed.changes,
                share: checkpoint.share,
                must_remake: true,
                settled: false,
                #[cfg(feature = "test-util")]
                fail_next_settle: false,
            },
        );
        Ok(Ok(Answer::Taken {
            repair: changed.repair.update,
            state_bytes,
            clients,
            changes: changed.changes,
            client_changes: changed.client_changes,
            steps_ahead: changed.steps_ahead,
            files,
            remake: true,
            fold: false,
        }))
    }

    /// Rebuilds a document from `checkpoint.snapshot_state()` + at most
    /// `SNAPSHOT_SINCE_GAP - 1` trailing entries of `since` after a post-mutation refusal,
    /// settling and rebasing onto `base` (reclaiming `since_bytes` and leaving `settled`
    /// true so subsequent `Dirty` refusals stay O(1)) when a non-empty tail or
    /// non-canonical unsettled snapshot had to be replayed.
    ///
    /// When `checkpoint` was unsettled and `yrs` squashes adjacent blocks on read-back
    /// (`!same_state` on pass 1), settles to its canonical state while staying within
    /// `held_bytes`. Any restoration error, divergence, or `held_bytes` overflow releases
    /// accounted bytes and fails the worker closed (`Err(FrameError)`).
    fn restore_after_dirty(
        &mut self,
        doc: DocNumber,
        checkpoint: Checkpoint,
    ) -> Result<(), FrameError> {
        let old_accounted = checkpoint.accounted_bytes();
        let base_held = self.held_bytes.saturating_sub(old_accounted);
        let tail = checkpoint.tail_since_snapshot();
        if tail.is_empty() {
            let expected = checkpoint.snapshot_state();
            let rebuilt = BodyDoc::replay_checkpoint::<&[u8]>(expected, &[]).map_err(|error| {
                self.held_bytes = base_held;
                FrameError::Io(std::io::Error::other(format!(
                    "failed to restore document after refusal: {error}"
                )))
            })?;
            let rebuilt_state = rebuilt.state();
            if same_state(&rebuilt_state, expected) {
                self.docs.insert(doc, checkpoint.into_held(rebuilt));
                return Ok(());
            }
            if checkpoint.settled {
                self.held_bytes = base_held;
                return Err(FrameError::Io(std::io::Error::other(
                    "restored document state diverged from settled checkpoint",
                )));
            }
            drop(rebuilt);
            return self.install_settled_restore(doc, base_held, &checkpoint, &rebuilt_state);
        }
        let replayed =
            BodyDoc::replay_checkpoint(checkpoint.snapshot_state(), tail).map_err(|error| {
                self.held_bytes = base_held;
                FrameError::Io(std::io::Error::other(format!(
                    "failed to restore document after refusal: {error}"
                )))
            })?;
        let replayed_state = replayed.state();
        drop(replayed);
        self.install_settled_restore(doc, base_held, &checkpoint, &replayed_state)
    }

    fn install_settled_restore(
        &mut self,
        doc: DocNumber,
        base_held: u64,
        checkpoint: &Checkpoint,
        rebuilt_state: &[u8],
    ) -> Result<(), FrameError> {
        let held = usize::try_from(self.limits.held_bytes).unwrap_or(usize::MAX);
        let restore_limits = UpdateLimits {
            update_bytes: rebuilt_state.len(),
            state_bytes: held.max(rebuilt_state.len()),
            body_bytes: BODY_MAX_BYTES,
            placing_steps: None,
            clients: None,
            client_changes: None,
            total_changes: None,
            reading_steps: self.update_limits.reading_steps,
        };
        let settled = BodyDoc::from_state(rebuilt_state, &restore_limits).map_err(|error| {
            self.held_bytes = base_held;
            FrameError::Io(std::io::Error::other(format!(
                "restored unsettled document state failed to settle: {error}"
            )))
        })?;
        if settled.repair.update.is_some() {
            self.held_bytes = base_held;
            return Err(FrameError::Io(std::io::Error::other(
                "restored unsettled document required repair during settle",
            )));
        }
        let new_state_bytes = count(settled.state_bytes);
        let settled_state = settled.state.unwrap_or_else(|| settled.doc.state());
        if base_held.saturating_add(new_state_bytes) > self.limits.held_bytes {
            self.held_bytes = base_held;
            return Err(FrameError::Io(std::io::Error::other(
                "restored document state exceeded worker held_bytes budget",
            )));
        }
        self.held_bytes = base_held.saturating_add(new_state_bytes);
        self.docs.insert(
            doc,
            Held {
                doc: settled.doc,
                base: settled_state,
                since: Vec::new(),
                since_bytes: 0,
                last_state: None,
                snapshot_since_start: 0,
                since_replay_steps: 0,
                state_bytes: new_state_bytes,
                changes: settled.changes,
                share: checkpoint.share,
                must_remake: false,
                settled: true,
                #[cfg(feature = "test-util")]
                fail_next_settle: checkpoint.fail_next_settle,
            },
        );
        Ok(())
    }

    /// The limits of what the server stored before: a state as large as the worker's
    /// memory for documents, a body as large as a body may be, and its changes placed
    /// past as many as they are. Its total changes ([`MAX_TOTAL_CHANGES`]) and reading
    /// steps ([`READING_STEPS`]) are still checked before the library decodes a stored
    /// state: one past either is refused unread, while logged updates replayed via
    /// `Request::Retake` relax `total_changes` (and return `remake` past
    /// [`READING_STEPS`]) so the server projects their body before remaking losslessly.
    fn logged_limits(&self) -> UpdateLimits {
        let held = usize::try_from(self.limits.held_bytes).unwrap_or(usize::MAX);
        UpdateLimits {
            update_bytes: held,
            state_bytes: held,
            body_bytes: self.update_limits.body_bytes,
            placing_steps: None,
            clients: None,
            client_changes: None,
            total_changes: self.update_limits.total_changes,
            reading_steps: Some(READING_STEPS),
        }
    }

    fn doc(&self, doc: DocNumber) -> Result<&BodyDoc, Refused> {
        self.docs
            .get(&doc)
            .map(|held| &held.doc)
            .ok_or_else(|| refused(Refusal::UnknownDocument, "no document is open under it"))
    }

    /// Returns a held document only if it is not waiting to be remade from its body
    /// (`!held.must_remake`), refusing with `on_remake` otherwise so a document past
    /// [`READING_STEPS`] or with a failed settle check is not served for diffs or state
    /// vectors. Documents with `held.settled == false` remain usable for normal live
    /// requests until settled at fold/close.
    fn usable_doc(&self, doc: DocNumber, on_remake: Refusal) -> Result<&BodyDoc, Refused> {
        let held = self
            .docs
            .get(&doc)
            .ok_or_else(|| refused(Refusal::UnknownDocument, "no document is open under it"))?;
        if held.must_remake {
            return Err(refused(
                on_remake,
                "the document must be made again from its body before another request",
            ));
        }
        Ok(&held.doc)
    }

    /// Holds `held` under `doc`, in place of what was held there, if the states and
    /// rollback logs of all held documents stay within [`Limits::held_bytes`].
    fn hold(&mut self, doc: DocNumber, held: Held) -> Result<(), Refused> {
        let before = self.docs.get(&doc).map_or(0, Held::accounted_bytes);
        let after = self
            .held_bytes
            .saturating_sub(before)
            .saturating_add(held.accounted_bytes());
        if after > self.limits.held_bytes {
            return Err(refused(
                Refusal::TooManyDocuments,
                "the worker holds as many bytes of documents as it may",
            ));
        }
        self.held_bytes = after;
        self.docs.insert(doc, held);
        Ok(())
    }

    fn open_body(&self, json: Option<Vec<u8>>) -> Result<fvoci_collab::MadeBody, Refused> {
        let body = json
            .map(|json| serde_json::from_slice(&json).map(Body::from_stored))
            .transpose()
            .map_err(|error| refused(Refusal::NoBody, &error.to_string()))?;
        let made = BodyDoc::from_body_counted(body.as_ref())
            .map_err(|error| take_refused(&TakeError::Update(error)))?;
        let state_bytes = size_of(&made.state);
        if state_bytes > self.limits.state_bytes {
            return Err(refused(
                Refusal::DocumentTooLarge,
                &format!(
                    "the document of the body is {state_bytes} bytes, more than the {} allowed",
                    self.limits.state_bytes
                ),
            ));
        }
        if let Some(limit) = self.update_limits.total_changes
            && made.counts.changes > limit
        {
            return Err(refused(
                Refusal::DocumentTooLarge,
                &format!(
                    "the document of the body has {} changes, more than the {limit} allowed",
                    made.counts.changes
                ),
            ));
        }
        Ok(made)
    }

    fn open(&mut self, doc: DocNumber, source: Source, share: Share) -> Response {
        if self.docs.contains_key(&doc) {
            return Err(refused(
                Refusal::UnknownDocument,
                "a document is open under it already",
            ));
        }
        let of_share = self
            .docs
            .values()
            .filter(|held| held.share == share)
            .count();
        let limit = match share {
            Share::Kept => self.limits.documents,
            Share::Aside => u64::try_from(ASIDE_DOCUMENTS).unwrap_or(u64::MAX),
        };
        if u64::try_from(of_share).unwrap_or(u64::MAX) >= limit {
            return Err(refused(
                Refusal::TooManyDocuments,
                "the worker holds as many documents of that share as it may",
            ));
        }
        let (opened, base, reply_state, counts) = match source {
            Source::State(stored) => {
                // Stored states are checked against `total_changes` and `reading_steps`
                // pre-decode; other lowered limits are remade after replay.
                let changed = BodyDoc::from_state(&stored, &self.logged_limits())
                    .map_err(|error| logged_refused(&error))?;
                let state = changed.state.unwrap_or_else(|| changed.doc.state());
                let repaired = changed.repair.update.is_some().then(|| state.clone());
                let counts = (changed.changes, changed.client_changes, changed.steps_ahead);
                (changed.doc, state, repaired, counts)
            }
            Source::Body(json) => {
                let made = self.open_body(json)?;
                let base = made.state.clone();
                (
                    made.doc,
                    base,
                    Some(made.state),
                    (
                        made.counts.changes,
                        made.counts.client_changes,
                        made.counts.steps_ahead,
                    ),
                )
            }
        };
        let (changes, client_changes, steps_ahead) = counts;
        let state_bytes = size_of(&base);
        let clients = count(opened.clients());
        self.hold(
            doc,
            Held {
                doc: opened,
                base,
                since: Vec::new(),
                since_bytes: 0,
                last_state: None,
                snapshot_since_start: 0,
                since_replay_steps: 0,
                state_bytes,
                changes,
                share,
                must_remake: false,
                settled: true,
                #[cfg(feature = "test-util")]
                fail_next_settle: false,
            },
        )?;
        Ok(Answer::Opened {
            state: reply_state,
            state_bytes,
            clients,
            changes,
            client_changes,
            steps_ahead,
        })
    }
}

#[cfg(test)]
mod tests;
