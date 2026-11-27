//! In-place update application (`with_update_in_place`) using incremental
//! [`crate::reading::ReadingCache`] and lazy state encoding.

use yrs::{BranchID, Doc, ID, ReadTxn as _, StateVector, Transact as _, Update};

use super::{
    DeltaMode, Outcome, ReadFlags, Settled, UpdateError, UpdateLimits, check_applied,
    counted_sized, deletions_in_order, origin_refusal, read, state_of,
};
use crate::block_cache::ApplyDelta;
use crate::placing::{self, Sent};
use crate::reading::{self, FastApply, ReadingCache};
use crate::settle::{same_state, settled_sized};

/// Whether [`apply_and_capture`] also encodes `txn.encode_update_v1()` for incremental
/// [`ReadingCache`] updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TxnUpdateMode {
    Skip,
    Capture,
}

/// What [`apply_and_capture`] captures from the committed transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CaptureMode {
    pub(crate) delta: DeltaMode,
    pub(crate) txn_update: TxnUpdateMode,
}

/// Output of applying one transaction in place.
pub(crate) struct AppliedTxn {
    pub(crate) delta: Option<ApplyDelta>,
    pub(crate) txn_update: Option<Vec<u8>>,
    pub(crate) before_sv: StateVector,
    pub(crate) after_sv: StateVector,
    pub(crate) mutated: bool,
}

/// Target state passed to [`apply_and_check`].
pub(super) struct ApplyTarget<'a> {
    pub(super) doc: Doc,
    pub(super) reading_cache: Option<Box<ReadingCache>>,
    pub(super) prior_state: Option<&'a [u8]>,
    pub(super) placing_steps: u64,
}

/// Held document and prior accounting state passed to [`with_update_in_place`].
pub(crate) struct InPlaceDoc<'a> {
    pub(crate) doc: Doc,
    pub(crate) reading_cache: Option<Box<ReadingCache>>,
    pub(crate) prior_state: Option<&'a [u8]>,
    pub(crate) prior_changes: u64,
}

/// Applies a decoded `sent` update to `doc`, commits the transaction, and captures its
/// [`ApplyDelta`] (changed branches, delete set, and newly inserted shared-type IDs) for
/// incremental block-cache updates, plus optionally `txn.encode_update_v1()` for
/// incremental [`ReadingCache`] updates.
pub(crate) fn apply_and_capture(
    doc: &Doc,
    sent: Update,
    new_types: Vec<ID>,
    capture: CaptureMode,
) -> Result<AppliedTxn, UpdateError> {
    let mut txn = doc.transact_mut();
    let before_sv = txn.state_vector();
    txn.apply_update(sent)
        .map_err(|error| UpdateError::Unappliable(error.to_string()))?;
    txn.commit();
    let after_sv = txn.state_vector();
    let mutated = before_sv != after_sv || !txn.delete_set().is_empty();
    let txn_update = (capture.txn_update == TxnUpdateMode::Capture).then(|| txn.encode_update_v1());
    let delta = (capture.delta == DeltaMode::Capture).then(|| {
        let changed_branches = txn
            .changed_parent_types()
            .iter()
            .filter_map(|branch| match branch.id() {
                BranchID::Nested(id) => Some(id),
                BranchID::Root(_) => None,
            })
            .collect();
        let delete_set = txn.delete_set().clone();
        ApplyDelta {
            changed_branches,
            delete_set,
            new_types,
        }
    });
    Ok(AppliedTxn {
        delta,
        txn_update,
        before_sv,
        after_sv,
        mutated,
    })
}

/// Applies `sent` to `target.doc` in place, verifies post-apply invariants, and validates
/// the resulting state (using `reading_cache` incrementally when available, or falling
/// back to `counted_sized` / `settled_sized`).
pub(super) fn apply_and_check(
    target: ApplyTarget<'_>,
    sent: Update,
    flags: ReadFlags,
    limits: &UpdateLimits,
    delta_mode: DeltaMode,
) -> Result<Settled, UpdateError> {
    let ApplyTarget {
        doc,
        reading_cache,
        prior_state,
        placing_steps,
    } = target;
    let capture = CaptureMode {
        delta: if delta_mode == DeltaMode::Capture && !flags.partial_overlap {
            DeltaMode::Capture
        } else {
            DeltaMode::Skip
        },
        txn_update: if reading_cache.is_some()
            && !flags.partial_overlap
            && limits.reading_steps.is_some()
        {
            TxnUpdateMode::Capture
        } else {
            TxnUpdateMode::Skip
        },
    };
    let applied = apply_and_capture(&doc, sent, flags.new_types, capture)?;
    check_applied(&doc)?;
    if flags.partial_overlap {
        drop(reading_cache);
        let mut settled = settled_sized(doc, limits)?;
        settled.placing_steps = placing_steps;
        settled.mutated = applied.mutated
            || prior_state.is_none_or(|prior| {
                settled
                    .state
                    .as_deref()
                    .is_none_or(|now| !same_state(now, prior))
            });
        return Ok(settled);
    }
    if let (Some(rc), Some(txn_update)) = (reading_cache, applied.txn_update.as_deref()) {
        match rc.try_apply(txn_update, &applied.before_sv, &applied.after_sv, limits) {
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
                    doc,
                    state: None,
                    state_bytes: summary.state_bytes,
                    changes: summary.changes,
                    client_changes: summary.client_changes,
                    reading_steps: summary.reading_steps,
                    steps_ahead: summary.steps_ahead,
                    outcome,
                    needs_settle: false,
                    doc_rebuilt: false,
                    delta: applied.delta,
                    mutated: applied.mutated,
                    placing_steps,
                    reading_cache: cache,
                });
            }
            Ok(FastApply::TooLarge { size, limit }) => {
                return Err(UpdateError::DocumentTooLarge { size, limit });
            }
            Ok(FastApply::TooManyChanges { changes, limit }) => {
                return Err(UpdateError::TooManyChanges { changes, limit });
            }
            Ok(FastApply::TooManyTotalChanges { changes, limit }) => {
                return Err(UpdateError::TooManyTotalChanges { changes, limit });
            }
            Ok(FastApply::Fallback) | Err(_) => {}
        }
    }
    let mut settled = counted_sized(doc, limits)?;
    settled.placing_steps = placing_steps;
    settled.mutated = applied.mutated
        || (settled.doc_rebuilt
            && prior_state.is_none_or(|prior| {
                settled
                    .state
                    .as_deref()
                    .is_none_or(|now| !same_state(now, prior))
            }));
    if !settled.doc_rebuilt {
        settled.delta = applied.delta;
    }
    Ok(settled)
}

/// Why [`with_update_in_place`] refused an update.
pub(crate) enum UpdateInPlaceError {
    /// Refused before `doc` was changed; the untouched document and reading cache come back.
    Untouched(Doc, Option<Box<ReadingCache>>, UpdateError),
    /// Refused while or after applying the update; `doc` may have been changed and was
    /// dropped.
    Dirty(UpdateError),
}

/// Takes a client's update into `held.doc` **in place**, using `held.reading_cache` (or
/// lazily `held.prior_state` / `state_of(&doc)` only when needed) to validate origins,
/// charge placing steps, and count Pass 5–6 without encoding or copying `doc` on the hot
/// path.
///
/// Checks that fail before `apply_update` return [`UpdateInPlaceError::Untouched`] with
/// `doc` and `reading_cache` intact; failures during or after `apply_update` drop `doc`
/// and return [`UpdateInPlaceError::Dirty`], telling the caller to rebuild from its
/// checkpoint.
pub(crate) fn with_update_in_place(
    held: InPlaceDoc<'_>,
    update: &[u8],
    limits: &UpdateLimits,
    delta_mode: DeltaMode,
) -> Result<Settled, UpdateInPlaceError> {
    let InPlaceDoc {
        doc,
        reading_cache,
        prior_state,
        prior_changes,
    } = held;
    if update.len() > limits.update_bytes {
        return Err(UpdateInPlaceError::Untouched(
            doc,
            reading_cache,
            UpdateError::TooLarge {
                size: update.len(),
                limit: limits.update_bytes,
            },
        ));
    }
    let (sent, clients, flags) = match read(&doc, update) {
        Ok(read) => read,
        Err(error) => return Err(UpdateInPlaceError::Untouched(doc, reading_cache, error)),
    };
    if let Some(limit) = limits.clients
        && clients > limit
    {
        return Err(UpdateInPlaceError::Untouched(
            doc,
            reading_cache,
            UpdateError::TooManyClients { clients, limit },
        ));
    }
    let mut encoded_prior: Option<Vec<u8>> = None;
    let mut placing_steps = 0_u64;
    if let Some(limit) = limits.placing_steps {
        if let Err(error) = deletions_in_order(&sent) {
            return Err(UpdateInPlaceError::Untouched(doc, reading_cache, error));
        }
        let steps = match charged_lazy(
            &doc,
            reading_cache.as_deref(),
            prior_state,
            &mut encoded_prior,
            prior_changes,
            (update, &sent),
            limit,
        ) {
            Ok(steps) => steps,
            Err(error) => return Err(UpdateInPlaceError::Untouched(doc, reading_cache, error)),
        };
        if steps > limit {
            return Err(UpdateInPlaceError::Untouched(
                doc,
                reading_cache,
                UpdateError::PlacedPastTooMany { steps, limit },
            ));
        }
        placing_steps = steps;
    }
    if flags.check_origins_before_apply {
        let known = doc.transact().state_vector();
        let origin_check = if let Some(rc) = &reading_cache {
            rc.check_incoming_origins(&known, update)
        } else {
            let state_ref: &[u8] = match prior_state {
                Some(bytes) => bytes,
                None => encoded_prior
                    .get_or_insert_with(|| state_of(&doc))
                    .as_slice(),
            };
            reading::check_update_origins(state_ref, &known, update)
        };
        if let Err(refusal) = origin_check {
            return Err(UpdateInPlaceError::Untouched(
                doc,
                reading_cache,
                origin_refusal(refusal),
            ));
        }
    }
    let effective_prior = prior_state.or(encoded_prior.as_deref());
    apply_and_check(
        ApplyTarget {
            doc,
            reading_cache,
            prior_state: effective_prior,
            placing_steps,
        },
        sent,
        flags,
        limits,
        delta_mode,
    )
    .map_err(UpdateInPlaceError::Dirty)
}

/// As [`super::charged`], encoding `copy`'s state lazily into `encoded_cache` only when
/// `prior_state` is `None` and the cheap bound exceeds `limit`.
fn charged_lazy<'a>(
    copy: &Doc,
    reading_cache: Option<&ReadingCache>,
    prior_state: Option<&'a [u8]>,
    encoded_cache: &'a mut Option<Vec<u8>>,
    changes: u64,
    (update, sent): (&[u8], &Update),
    limit: u64,
) -> Result<u64, UpdateError> {
    let ends = copy.transact().state_vector();
    let sent_view = Sent {
        update,
        covered: &sent.insertions(true),
        deleted: sent.delete_set(),
    };
    let cut_fn = |id| reading_cache.and_then(|rc| rc.cut_inside(id));
    let cut_opt: Option<&dyn Fn(crate::scan::Id) -> Option<u64>> =
        reading_cache.is_some().then_some(&cut_fn);
    placing::charge_lazy(
        changes,
        &ends,
        cut_opt,
        || match prior_state {
            Some(bytes) => bytes,
            None => encoded_cache
                .get_or_insert_with(|| state_of(copy))
                .as_slice(),
        },
        &sent_view,
        limit,
    )
    .map_err(|refusal| UpdateError::Unreadable(refusal.reason().to_owned()))
}
