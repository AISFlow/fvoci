//! What the server does with an update from a client, or with a stored state, in the
//! one order that keeps its document whole.

use fvoci_model::{Body, BodyError};
use thiserror::Error;
use yrs::{Doc, ReadTxn as _, Transact as _};

use crate::block_cache::BlockCache;
use crate::project::{json_size_of, to_body};
use crate::reading::{FastApply, ReadingCache};
use crate::repair::{
    InPlaceRepair, Repair, build_or_repair_cache, repaired, validate_or_repair_in_place,
};
use crate::settle::{from_state_settled, settled_sized};
use crate::update::{
    DeltaMode, InPlaceDoc, Outcome, Settled, UpdateError, UpdateInPlaceError, UpdateLimits,
    counted_sized, state_of, with_update_in_place, with_update_sized,
};

/// Whether post-repair validation only encodes and counts in place (`CountOnly`, used on
/// live updates) or performs a full settle re-read (`Settle`, used when opening stored
/// states).
#[derive(Clone, Copy, PartialEq, Eq)]
enum SettleMode {
    CountOnly,
    Settle,
}

/// A document that took an update, with its body.
#[derive(Debug)]
#[non_exhaustive]
pub struct Taken {
    /// The document with the update, and with what a repair changed.
    pub doc: Doc,
    /// The body of `doc`, and what a repair changed to make it one: the changes, as
    /// an update, are for the editors that hold the document, which did not make them.
    pub repair: Repair,
    /// The whole state of `doc` ([`crate::state_of`]), already encoded.
    pub state: Vec<u8>,
    /// The size of the state of `doc` ([`crate::state_of`]), in bytes.
    pub state_bytes: usize,
    /// How many changes `doc` holds in total.
    pub changes: u64,
    /// The most changes one client has in `doc`, deleted ones too
    /// ([`UpdateLimits::client_changes`]).
    pub client_changes: u64,
    /// The steps the Y library takes to read `doc` from its state
    /// ([`UpdateLimits::reading_steps`]); 0 where the limits have no count of them.
    pub reading_steps: u64,
    /// The most steps the library may take to read `doc` once one more client has
    /// written into it ([`crate::steps_ahead`]); 0 where the limits have no count.
    pub steps_ahead: u64,
    /// Whether `doc` must be made again from its body ([`crate::from_body`]) before
    /// its state is stored or another update is taken: reading its state would take
    /// more steps than [`UpdateLimits::reading_steps`], so `doc` was not read back
    /// from its state.
    pub remake: bool,
}

/// Why an update, or a stored state, is not taken. The document it was given for is as
/// it was.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum TakeError {
    /// The update cannot be taken at all.
    #[error(transparent)]
    Update(#[from] UpdateError),
    /// With the update the document is no body, and no repair makes it one.
    #[error(transparent)]
    Body(#[from] BodyError),
    /// With the update the body would be larger than [`UpdateLimits::body_bytes`].
    #[error("with the update the body would be {size} bytes, more than the {limit} allowed")]
    BodyTooLarge {
        /// The size the body would have, in bytes of its stored JSON.
        size: usize,
        /// The limit it is past: [`UpdateLimits::body_bytes`].
        limit: usize,
    },
}

/// Synchronized incremental caches carried by a [`crate::BodyDoc`]: top-level block
/// summaries ([`BlockCache`]) and Pass 5–6 wire/reading state ([`ReadingCache`]).
#[derive(Debug)]
pub(crate) struct DocCache {
    pub(crate) blocks: BlockCache,
    pub(crate) reading: Option<Box<ReadingCache>>,
}

impl DocCache {
    pub(crate) fn empty() -> Self {
        Self {
            blocks: BlockCache::new(),
            reading: Some(Box::new(ReadingCache::empty())),
        }
    }
}

/// Why [`take_update_in_place`] refused an update.
pub(crate) enum TakeInPlaceError {
    /// Refused before `doc` was changed; the untouched document and cache come back.
    Untouched(Doc, Option<Box<DocCache>>, TakeError),
    /// Refused while or after applying the update or repairing; `doc` may have been
    /// changed and was dropped.
    Dirty(TakeError),
}

/// Result of an in-place or cached-open operation: holds the validated `Doc` and its
/// synchronized [`DocCache`] without materializing a full [`Body`] JSON tree.
#[derive(Debug)]
pub(crate) struct TakenCache {
    pub(crate) doc: Doc,
    pub(crate) cache: Box<DocCache>,
    pub(crate) repair: InPlaceRepair,
    pub(crate) state: Option<Vec<u8>>,
    pub(crate) state_bytes: usize,
    pub(crate) changes: u64,
    pub(crate) client_changes: u64,
    pub(crate) reading_steps: u64,
    pub(crate) steps_ahead: u64,
    pub(crate) remake: bool,
    pub(crate) mutated: bool,
    pub(crate) placing_steps: u64,
}

/// Takes a client's update into a document, as the server does with every one: the
/// update is applied to a copy, strictly checked for valid origins, limits, and reading
/// steps, projected to a body, and repaired where that fails; only a copy that is a body
/// within the limits comes back. `doc` itself is never changed.
///
/// The deletions of a repair ([`Repair::update`]) go to every editor that holds the
/// document, the one that sent the update too: they did not make them.
///
/// How long this takes grows with the document as well as with the update: the copy
/// reads the whole state, and the walk refuses the known ways to make that reading cost
/// the square of the state (a member written over and over). Full settle re-reads
/// (`same_state`) run when opening stored states ([`from_state`]), on fold/close
/// ([`crate::BodyDoc::settle`]), and on non-YATA or partially overlapping updates, rather
/// than on every ordinary live edit. And the Y library places a change among the
/// others of its list by walking past them, so that an update made for it has every
/// change walk a long list (59 kB of changes that each named the root of 50,000
/// children took 12 seconds): [`UpdateLimits::placing_steps`] is what an update may
/// be made to walk. The library walks so again every time it reads the document from
/// its state, here and as the document is opened, so that updates within that limit
/// added up to a document no worker opened in its time:
/// [`UpdateLimits::reading_steps`] is what the document an update leaves may take to
/// read, counted from its state before the library reads it. Not every way to make
/// the library slow may be counted: what calls this also gives it a time
/// to take an update in, and holds back the sender of one that takes longer. It keeps
/// [`UpdateLimits::update_bytes`] and [`UpdateLimits::state_bytes`] small and does not
/// hold anything others wait for while it runs.
///
/// # Errors
///
/// Returns [`TakeError::Update`] if the update cannot be taken at all (see
/// [`UpdateError`]: among them one whose changes would take more steps to place than
/// [`UpdateLimits::placing_steps`]), [`TakeError::Body`] if the document with it is no
/// body and a repair does not make it one, and [`TakeError::BodyTooLarge`] if its body
/// is past [`UpdateLimits::body_bytes`].
///
/// A document keeps what was deleted from it, so it grows with every edit: typing and
/// deleting 10 kB a hundred times leaves a body of two letters in a state of 1 MB. Once
/// it is near [`UpdateLimits::state_bytes`], every update is refused with
/// [`UpdateError::DocumentTooLarge`], deletions too. Changes that make the state slow
/// to read stay in it as well: once an update would take the document past
/// [`UpdateLimits::reading_steps`], it is still taken (and repaired in place if needed,
/// without reading the slow state again) and returned with [`Taken::remake`] set so
/// what holds the document writes its body and makes it again from that
/// ([`crate::from_body`]) without losing the update. [`Taken::steps_ahead`] tells what
/// holds the document how near it may be after one more client wrote, so that it is
/// made again beforehand; making it again starts without what was deleted and in no
/// reading steps, and has the editors that hold the old one load the new one: an update
/// made against the old document is refused by the new one
/// ([`UpdateError::MissingChanges`]).
pub fn take_update(doc: &Doc, update: &[u8], limits: &UpdateLimits) -> Result<Taken, TakeError> {
    let with = with_update_sized(doc, update, limits)?;
    to_taken(with, limits, SettleMode::CountOnly)
}

/// Takes a client's update into `doc` **in place**, without copying `doc` before
/// applying or before repairing, and incrementally updates `cache` for only the dirty
/// top-level blocks. Callers pass the cached `prior_state` and `prior_changes` of `doc`
/// and must rebuild `doc` from their checkpoint on [`TakeInPlaceError::Dirty`].
pub(crate) fn take_update_in_place(
    doc: Doc,
    cache: Option<Box<DocCache>>,
    prior_state: Option<&[u8]>,
    prior_changes: u64,
    update: &[u8],
    limits: &UpdateLimits,
) -> Result<TakenCache, TakeInPlaceError> {
    let (blocks, reading) = match cache {
        Some(boxed) => {
            let DocCache { blocks, reading } = *boxed;
            (Some(blocks), reading)
        }
        None => (None, None),
    };
    let delta_mode = if blocks.is_some() {
        DeltaMode::Capture
    } else {
        DeltaMode::Skip
    };
    let with = match with_update_in_place(
        InPlaceDoc {
            doc,
            reading_cache: reading,
            prior_state,
            prior_changes,
        },
        update,
        limits,
        delta_mode,
    ) {
        Ok(settled) => settled,
        Err(UpdateInPlaceError::Untouched(doc, reading, error)) => {
            let restored = blocks.map(|blocks| Box::new(DocCache { blocks, reading }));
            return Err(TakeInPlaceError::Untouched(doc, restored, error.into()));
        }
        Err(UpdateInPlaceError::Dirty(error)) => {
            return Err(TakeInPlaceError::Dirty(error.into()));
        }
    };
    to_taken_in_place(with, blocks, limits, SettleMode::CountOnly).map_err(TakeInPlaceError::Dirty)
}

/// Opens a stored state directly into a cached [`TakenCache`] (block-by-block validation,
/// no whole-document JSON tree held at once).
pub(crate) fn from_state_cached(
    state: &[u8],
    limits: &UpdateLimits,
) -> Result<TakenCache, TakeError> {
    let limits = UpdateLimits {
        update_bytes: state.len(),
        placing_steps: None,
        clients: None,
        client_changes: None,
        ..*limits
    };
    let with = from_state_settled(state, &limits)?;
    let taken = to_taken_in_place(with, None, &limits, SettleMode::Settle)?;
    if taken.remake {
        return Err(UpdateError::TooManyReadingSteps {
            steps: taken.reading_steps,
            limit: limits.reading_steps.unwrap_or_default(),
        }
        .into());
    }
    Ok(taken)
}

/// Opens the document of a stored state ([`crate::state_of`]), with the same checks
/// and repair as [`take_update`], leaving out per-update limits (`update_bytes`,
/// `placing_steps`, `clients`, `client_changes`) while keeping `state_bytes`,
/// `body_bytes`, `total_changes`, and `reading_steps`: a stored state past
/// [`UpdateLimits::total_changes`] or [`UpdateLimits::reading_steps`] is refused before
/// the library decodes it.
///
/// # Errors
///
/// As [`take_update`].
pub fn from_state(state: &[u8], limits: &UpdateLimits) -> Result<Taken, TakeError> {
    let limits = UpdateLimits {
        update_bytes: state.len(),
        placing_steps: None,
        clients: None,
        client_changes: None,
        ..*limits
    };
    let with = from_state_settled(state, &limits)?;
    let taken = to_taken(with, &limits, SettleMode::Settle)?;
    if taken.remake {
        return Err(UpdateError::TooManyReadingSteps {
            steps: taken.reading_steps,
            limit: limits.reading_steps.unwrap_or_default(),
        }
        .into());
    }
    Ok(taken)
}

/// The document an update left, as a body: as it is, or repaired in place.
///
/// A repaired document is held to the limits of clients, size, changes, and reading
/// steps. Live updates re-encode and strictly count in place (`SettleMode::CountOnly`,
/// keeping the body projected by `repaired` without a redundant projection); opening a
/// stored state (`SettleMode::Settle`) performs a full settle re-read when within
/// [`UpdateLimits::reading_steps`].
fn to_taken(with: Settled, limits: &UpdateLimits, mode: SettleMode) -> Result<Taken, TakeError> {
    let mut repair = repaired(&with.doc)?;
    let settled = if repair.update.is_some() {
        if repair.mended > 0
            && let Some(limit) = limits.clients
        {
            let clients = with.doc.transact().state_vector().len();
            if clients > limit {
                return Err(UpdateError::TooManyClients { clients, limit }.into());
            }
        }
        let effective_mode = if with.needs_settle {
            SettleMode::Settle
        } else {
            mode
        };
        match effective_mode {
            SettleMode::CountOnly => {
                let again = counted_sized(with.doc, limits)?;
                if again.needs_settle {
                    repair.body = None;
                    repair.body = to_body(&again.doc)?;
                }
                again
            }
            SettleMode::Settle => {
                // Drop the pre-settle body before `settled_sized` and the post-settle
                // projection so two full `Body` trees are never live at once.
                repair.body = None;
                let again = settled_sized(with.doc, limits)?;
                repair.body = to_body(&again.doc)?;
                again
            }
        }
    } else {
        with
    };
    finish_taken(settled, repair, limits)
}

/// Validates (and repairs if needed) `with.doc` using the incremental [`BlockCache`],
/// avoiding whole-document projection and JSON tree materialization on the hot path.
fn to_taken_in_place(
    mut with: Settled,
    blocks: Option<BlockCache>,
    limits: &UpdateLimits,
    mode: SettleMode,
) -> Result<TakenCache, TakeError> {
    let delta = with.delta.take();
    let placing_steps = with.placing_steps;
    let mut mutated = with.mutated;
    let (mut blocks, repair, repair_ds) = match (blocks, delta, with.doc_rebuilt) {
        (Some(mut blocks), Some(delta), false) => {
            let (repair, repair_ds) = validate_or_repair_in_place(&with.doc, &mut blocks, &delta)?;
            (blocks, repair, repair_ds)
        }
        (old_blocks, _, _) => {
            // Drop any prior cache before rebuilding so two caches are never live at once.
            drop(old_blocks);
            build_or_repair_cache(&with.doc)?
        }
    };
    let settled = if repair.update.is_some() {
        mutated = true;
        if repair.mended > 0
            && let Some(limit) = limits.clients
        {
            let clients = with.doc.transact().state_vector().len();
            if clients > limit {
                return Err(UpdateError::TooManyClients { clients, limit }.into());
            }
        }
        let effective_mode = if with.needs_settle {
            SettleMode::Settle
        } else {
            mode
        };
        recount_after_repair(
            with,
            &mut blocks,
            &repair,
            &repair_ds,
            limits,
            effective_mode,
        )?
    } else {
        with
    };
    if let Some(size) = blocks.body_bytes()
        && size > limits.body_bytes
    {
        return Err(TakeError::BodyTooLarge {
            size,
            limit: limits.body_bytes,
        });
    }
    Ok(TakenCache {
        doc: settled.doc,
        cache: Box::new(DocCache {
            blocks,
            reading: settled.reading_cache,
        }),
        repair,
        state: settled.state,
        state_bytes: settled.state_bytes,
        changes: settled.changes,
        client_changes: settled.client_changes,
        reading_steps: settled.reading_steps,
        steps_ahead: settled.steps_ahead,
        remake: matches!(settled.outcome, Outcome::Remake { .. }),
        mutated,
        placing_steps,
    })
}

/// Re-validates Pass 5–6 after an in-place repair, using [`ReadingCache::try_apply_deletions`]
/// on deletion-only repairs (`repair.mended == 0`) before falling back to full `counted_sized`.
fn recount_after_repair(
    mut with: Settled,
    blocks: &mut BlockCache,
    repair: &InPlaceRepair,
    repair_ds: &yrs::IdSet,
    limits: &UpdateLimits,
    mode: SettleMode,
) -> Result<Settled, TakeError> {
    match mode {
        SettleMode::CountOnly => {
            if repair.mended == 0
                && let Some(rc) = with.reading_cache.take()
            {
                match rc.try_apply_deletions(repair_ds, limits) {
                    Ok(FastApply::Accepted { cache, summary }) => {
                        let outcome = if summary.remake {
                            Outcome::Remake {
                                steps: summary.reading_steps,
                                limit: limits.reading_steps.unwrap_or_default(),
                            }
                        } else {
                            Outcome::Settled
                        };
                        return Ok(Settled {
                            doc: with.doc,
                            state: None,
                            state_bytes: summary.state_bytes,
                            changes: summary.changes,
                            client_changes: summary.client_changes,
                            reading_steps: summary.reading_steps,
                            steps_ahead: summary.steps_ahead,
                            outcome,
                            needs_settle: false,
                            doc_rebuilt: false,
                            delta: None,
                            mutated: true,
                            placing_steps: with.placing_steps,
                            reading_cache: cache,
                        });
                    }
                    Ok(FastApply::TooLarge { size, limit }) => {
                        return Err(UpdateError::DocumentTooLarge { size, limit }.into());
                    }
                    Ok(FastApply::TooManyChanges { changes, limit }) => {
                        return Err(UpdateError::TooManyChanges { changes, limit }.into());
                    }
                    Ok(FastApply::TooManyTotalChanges { changes, limit }) => {
                        return Err(UpdateError::TooManyTotalChanges { changes, limit }.into());
                    }
                    Ok(FastApply::Fallback) | Err(_) => {}
                }
            }
            let again = counted_sized(with.doc, limits)?;
            if again.doc_rebuilt {
                *blocks = BlockCache::build(&again.doc)?;
            }
            Ok(again)
        }
        SettleMode::Settle => {
            // Free the pre-settle caches before re-decoding so peak memory stays minimal.
            blocks.clear();
            drop(with.reading_cache);
            let again = settled_sized(with.doc, limits)?;
            *blocks = BlockCache::build(&again.doc)?;
            Ok(again)
        }
    }
}

/// Checks `repair.body` against [`UpdateLimits::body_bytes`] and builds [`Taken`].
fn finish_taken(
    settled: Settled,
    repair: Repair,
    limits: &UpdateLimits,
) -> Result<Taken, TakeError> {
    if let Some(body) = &repair.body {
        let size = size_of(body);
        if size > limits.body_bytes {
            return Err(TakeError::BodyTooLarge {
                size,
                limit: limits.body_bytes,
            });
        }
    }
    let state = settled.state.unwrap_or_else(|| state_of(&settled.doc));
    let state_bytes = state.len();
    Ok(Taken {
        doc: settled.doc,
        repair,
        state,
        state_bytes,
        changes: settled.changes,
        client_changes: settled.client_changes,
        reading_steps: settled.reading_steps,
        steps_ahead: settled.steps_ahead,
        remake: matches!(settled.outcome, Outcome::Remake { .. }),
    })
}

/// The size of a body as JSON without whitespace, as its limit is measured.
pub(crate) fn size_of(body: &Body) -> usize {
    json_size_of(body.document())
}
