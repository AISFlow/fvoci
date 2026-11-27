//! Strict origin placement checks for [`super::Replay`] and pre-apply validation for
//! incoming updates that carry risky overlap or consecutive right-origin pairs.

use std::collections::HashMap;

use yrs::{ClientID, StateVector};

use super::{Holds, NO_ID, NONE, Named, OriginPolicy, Piece, Replay, StepsMode, index, number_in};
use crate::scan::{self, Change, Content, Id, Parent, Refusal};

/// An incoming change from an update trimmed against the document's known state vector.
pub(super) struct PendingChange<'bytes> {
    pub(super) begin: u64,
    pub(super) change: Change<'bytes>,
}

/// Trimmed incoming changes and partial-overlap anchors collected from one update.
pub(super) struct CollectedIncoming<'bytes> {
    pub(super) changes: HashMap<u64, Vec<PendingChange<'bytes>>>,
    pub(super) overlaps: Vec<OverlapParent<'bytes>>,
}

/// A partial overlap whose original parent/member or left origin must agree with the
/// document item at `(client, known - 1)` where `yrs`'s `BlockSet::exclude` re-anchors it.
pub(super) struct OverlapParent<'bytes> {
    pub(super) anchor: Id,
    pub(super) parent: Option<Parent<'bytes>>,
    pub(super) member: Option<&'bytes [u8]>,
    pub(super) left: Option<Id>,
}

impl<'bytes> Replay<'bytes> {
    /// Assigns monotonic group IDs to contiguous pieces of the same client belonging to
    /// the same wire change or contiguous squashable chain (`Letters`, `Values`,
    /// `Deleted`), so inverted same-block origins are caught in O(1).
    pub(super) fn assign_groups(&mut self) {
        let mut group = 0_u32;
        for at in 0..self.pieces.len() {
            if at > 0 {
                let prev = &self.pieces[at - 1];
                let curr = &self.pieces[at];
                let same_client = curr.client == prev.client;
                let prev_last =
                    (u64::from(prev.client) << 32) | u64::from(prev.end.saturating_sub(1));
                let squashable_chain = curr.holds.is_squashable()
                    && prev.holds.is_squashable()
                    && curr.begin == prev.end
                    && curr.origin == prev_last;
                if !(same_client && (curr.continues || squashable_chain)) {
                    group = group.saturating_add(1);
                }
            }
            self.pieces[at].group = group;
        }
    }

    /// Rejects misplaced origins and invalid parents on `change`:
    /// - identical `origin == right_origin`;
    /// - one neighbour resolving to a live item while the other is collected/gone;
    /// - `origin` and `right_origin` in different parents or members (list vs attribute);
    /// - `right_origin <= origin` within the same wire change or contiguous squashable
    ///   client run (which triggers `yrs`'s in-place double-split non-determinism);
    /// - `Named::Item` parent resolving to non-branch content (`Letters`, `Values`, `One`).
    ///
    /// Changes whose parent or both neighbours were garbage-collected/deleted (`Deleted`
    /// parent or `Collected`/gone neighbours) are allowed because `yrs` deterministically
    /// drops such orphaned edits to `GC` during concurrent browser merges.
    pub(super) fn check_piece_origins(
        &mut self,
        change: &Piece,
        origin_item: Option<u32>,
        right: Option<u32>,
    ) -> Result<(), Refusal> {
        if change.holds == Holds::Collected {
            return Ok(());
        }
        if change.origin != NO_ID && change.origin == change.right_origin {
            return Err(Refusal::Origin);
        }
        if change.origin == NO_ID
            && let Some(right_idx) = right
        {
            let right_piece = &self.pieces[index(right_idx)];
            if right_piece.origin != NO_ID || right_piece.continues {
                self.needs_settle = true;
            }
        }
        if change.origin != NO_ID && change.right_origin != NO_ID {
            match (origin_item, right) {
                (Some(left), Some(right)) => {
                    let left_piece = &self.pieces[index(left)];
                    let right_piece = &self.pieces[index(right)];
                    if left_piece.parent != right_piece.parent || left_piece.sub != right_piece.sub
                    {
                        return Err(Refusal::Origin);
                    }
                    if left_piece.group == right_piece.group && change.right_origin <= change.origin
                    {
                        return Err(Refusal::Origin);
                    }
                }
                (None, None) => {}
                (Some(_), None) | (None, Some(_)) => return Err(Refusal::Origin),
            }
        }
        if let Named::Item(parent_id) = change.named
            && let Some(parent) = self.item_at(parent_id)
            && !matches!(
                self.pieces[index(parent)].holds,
                Holds::Type | Holds::Deleted
            )
        {
            return Err(Refusal::Origin);
        }
        Ok(())
    }

    /// Resolves `(parent, member)` of a change in `self` after `run(StepsMode::Skip)`.
    fn resolve_target(
        &self,
        parent: Parent<'bytes>,
        member: Option<&'bytes [u8]>,
    ) -> Option<(u32, u32)> {
        let branch = match parent {
            Parent::Root(name) => {
                let root = *self.names.roots.get(name)?;
                super::small(self.pieces.len()) + root
            }
            Parent::Change(id) => {
                let client = *self.names.clients.get(&id.0)?;
                let packed = (u64::from(client) << 32) | u64::from(super::clock(id.1));
                let item = self.item_at(packed)?;
                if self.pieces[index(item)].holds != Holds::Type {
                    return None;
                }
                item
            }
        };
        let sub = match member {
            None => NONE,
            Some(name) => *self.names.members.get(name)?,
        };
        Some((branch, sub))
    }

    fn item_of_id(&self, id: Id) -> Option<u32> {
        let client = *self.names.clients.get(&id.0)?;
        let packed = (u64::from(client) << 32) | u64::from(super::clock(id.1));
        self.item_at(packed)
    }
}

/// Validates every incoming change's origins and partial-overlap anchors against
/// `prior_state` in linear time before `yrs::TransactionMut::apply_update` runs.
///
/// Called only when `update::read` flags a risky pattern (consecutive right-origin pairs
/// that could trigger `yrs`'s `squash_left` map panic, or partial overlaps of non-GC
/// items).
pub(crate) fn check_update_origins(
    prior_state: &[u8],
    known: &StateVector,
    update: &[u8],
) -> Result<(), Refusal> {
    let collected = collect_incoming(known, update)?;
    let mut replay = merge_into_replay(prior_state, collected.changes, &collected.overlaps)?;
    replay.assign_groups();
    replay.run(StepsMode::Skip)?;
    verify_overlaps(&replay, &collected.overlaps)
}

/// Walks `update`, trimming changes against `known` as `yrs`'s `BlockSet::exclude` does
/// and recording partial overlaps whose original placement must match their new anchor.
pub(super) fn collect_incoming<'bytes>(
    known: &StateVector,
    update: &'bytes [u8],
) -> Result<CollectedIncoming<'bytes>, Refusal> {
    let mut changes: HashMap<u64, Vec<PendingChange<'bytes>>> = HashMap::new();
    let mut overlaps: Vec<OverlapParent<'bytes>> = Vec::new();
    scan::walk(update, |(client, begin), change| {
        let len = change.content.length();
        if len == 0 {
            return;
        }
        let end = begin.saturating_add(len);
        let floor = u64::from(known.get(&ClientID::new(client)));
        if end <= floor {
            return;
        }
        let trimmed = if begin < floor {
            let remaining = end - floor;
            if matches!(change.content, Content::Collected(_)) {
                PendingChange {
                    begin: floor,
                    change: Change {
                        left: None,
                        right: None,
                        parent: None,
                        member: None,
                        content: Content::Collected(remaining),
                    },
                }
            } else {
                let anchor = (client, floor - 1);
                overlaps.push(OverlapParent {
                    anchor,
                    parent: change.parent,
                    member: change.member,
                    left: change.left,
                });
                PendingChange {
                    begin: floor,
                    change: Change {
                        left: Some(anchor),
                        right: change.right,
                        parent: None,
                        member: None,
                        content: trim_content(change.content, remaining),
                    },
                }
            }
        } else {
            PendingChange {
                begin,
                change: *change,
            }
        };
        changes.entry(client).or_default().push(trimmed);
    })?;
    Ok(CollectedIncoming { changes, overlaps })
}

/// Builds a [`Replay`] holding every client's prior changes followed contiguously by its
/// trimmed incoming changes.
fn merge_into_replay<'bytes>(
    prior_state: &'bytes [u8],
    mut incoming: HashMap<u64, Vec<PendingChange<'bytes>>>,
    overlaps: &[OverlapParent<'bytes>],
) -> Result<Replay<'bytes>, Refusal> {
    let mut replay = Replay::empty(OriginPolicy::Strict);
    let mut current_client: Option<u64> = None;
    let walked = scan::walk(prior_state, |(client, begin), change| {
        if current_client != Some(client) {
            if let Some(prev) = current_client.take()
                && let Some(extra) = incoming.remove(&prev)
            {
                for item in extra {
                    replay.keep((prev, item.begin), &item.change);
                }
            }
            current_client = Some(client);
        }
        replay.keep((client, begin), change);
    })?;
    if walked.begins.iter().any(|(_, begin)| *begin > 0) {
        return Err(Refusal::Gap);
    }
    if let Some(prev) = current_client
        && let Some(extra) = incoming.remove(&prev)
    {
        for item in extra {
            replay.keep((prev, item.begin), &item.change);
        }
    }
    let mut rest: Vec<(u64, Vec<PendingChange<'bytes>>)> = incoming.into_iter().collect();
    rest.sort_unstable_by_key(|(client, _)| *client);
    for (client, extra) in rest {
        for item in extra {
            replay.keep((client, item.begin), &item.change);
        }
    }
    for overlap in overlaps {
        if let Some(Parent::Root(name)) = overlap.parent {
            number_in(&mut replay.names.roots, name);
        }
        if let Some(name) = overlap.member {
            number_in(&mut replay.names.members, name);
        }
    }
    replay.roots = vec![NONE; replay.names.roots.len()];
    Ok(replay)
}

/// Verifies every partial overlap's original parent/member or left neighbour lies in the
/// same parent and member as the anchor item where `BlockSet::exclude` attaches it.
fn verify_overlaps(replay: &Replay<'_>, overlaps: &[OverlapParent<'_>]) -> Result<(), Refusal> {
    for overlap in overlaps {
        let Some(anchor_piece) = replay.item_of_id(overlap.anchor) else {
            return Err(Refusal::Origin);
        };
        let expected = (
            replay.pieces[index(anchor_piece)].parent,
            replay.pieces[index(anchor_piece)].sub,
        );
        if let Some(parent) = overlap.parent {
            if replay.resolve_target(parent, overlap.member) != Some(expected) {
                return Err(Refusal::Origin);
            }
        } else if let Some(left) = overlap.left {
            let Some(left_piece) = replay.item_of_id(left) else {
                return Err(Refusal::Origin);
            };
            let actual = (
                replay.pieces[index(left_piece)].parent,
                replay.pieces[index(left_piece)].sub,
            );
            if actual != expected {
                return Err(Refusal::Origin);
            }
        }
    }
    Ok(())
}

/// Preserves the content kind and remaining clock length of a partially-overlapping
/// change after trimming `known - begin` clocks off its prefix.
fn trim_content(content: Content<'_>, remaining: u64) -> Content<'static> {
    match content {
        Content::Collected(_) => Content::Collected(remaining),
        Content::Deleted(_) => Content::Deleted(remaining),
        Content::Letters(_) | Content::Values(_) => Content::Values(remaining),
        Content::Type => Content::Type,
        Content::One => Content::One,
    }
}
