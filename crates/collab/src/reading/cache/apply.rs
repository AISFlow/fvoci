//! Incremental transaction apply and deletion-only repair updates for [`ReadingCache`].

use std::collections::{HashMap, VecDeque};

use yrs::{ClientID, ID, IdSet, StateVector};

use super::{
    ApplySummary, CompactPiece, DepPoint, FastApply, MAX_CACHED_IDS, PieceExtra, PieceFlags,
    ReadingCache, SUB_NONE, UNKNOWN_UTF8_UNDER_128, split_payload_delta, varint_len,
};
use crate::reading::{Holds, LOOKED_PER_STEP, NONE, clock, index, small};
use crate::scan::{self, Content, Id, Parent, Refusal};
use crate::update::UpdateLimits;

enum ResolvedRef {
    Item(u32),
    Gone,
    Fallback,
}

enum SquashOutcome {
    NotSquashed,
    Squashed,
    Fallback,
}

struct ParsedBlock<'bytes> {
    client: u64,
    begin: u32,
    end: u32,
    left: Option<Id>,
    right: Option<Id>,
    parent: Option<Parent<'bytes>>,
    member: Option<&'bytes [u8]>,
    holds: Holds,
    utf8_len: usize,
    has_non_bmp: bool,
    wire_bytes: usize,
}

impl ReadingCache {
    /// Incrementally updates `self` for a committed transaction (`txn_update` from
    /// `txn.encode_update_v1()` after `txn.commit()`), returning exact Pass 5 + Pass 6
    /// metrics or [`FastApply::Fallback`].
    pub(crate) fn try_apply(
        mut self: Box<Self>,
        txn_update: &[u8],
        before_sv: &StateVector,
        after_sv: &StateVector,
        limits: &UpdateLimits,
    ) -> Result<FastApply, Refusal> {
        let Some(step_limit) = limits.reading_steps else {
            return Ok(FastApply::Fallback);
        };
        let Some((new_blocks, txn_deletes)) = parse_txn_update(txn_update, before_sv)? else {
            return Ok(FastApply::Fallback);
        };
        if !self.integrate_new_blocks(new_blocks, &txn_deletes) {
            return Ok(FastApply::Fallback);
        }
        if !self.apply_delete_ranges(&txn_deletes) {
            return Ok(FastApply::Fallback);
        }
        if !self.matches_state_vector(after_sv) {
            return Ok(FastApply::Fallback);
        }
        self.finish_apply(limits, step_limit)
    }

    /// Incrementally updates `self` for a deletion-only in-place repair (`repair.mended == 0`),
    /// eliminating the second Pass 5 + Pass 6 scan after repair.
    pub(crate) fn try_apply_deletions(
        mut self: Box<Self>,
        repair_ds: &IdSet,
        limits: &UpdateLimits,
    ) -> Result<FastApply, Refusal> {
        let Some(step_limit) = limits.reading_steps else {
            return Ok(FastApply::Fallback);
        };
        let mut ranges = Vec::new();
        for (client, id_ranges) in repair_ds.iter() {
            for r in id_ranges.iter() {
                if r.end > r.start {
                    ranges.push((client.get(), r.start, r.end));
                }
            }
        }
        if !self.apply_delete_ranges(&ranges) {
            return Ok(FastApply::Fallback);
        }
        self.finish_apply(limits, step_limit)
    }

    fn finish_apply(
        self: Box<Self>,
        limits: &UpdateLimits,
        step_limit: u64,
    ) -> Result<FastApply, Refusal> {
        let state_bytes = self.state_bytes();
        if state_bytes > limits.state_bytes {
            return Ok(FastApply::TooLarge {
                size: state_bytes,
                limit: limits.state_bytes,
            });
        }
        let looked = self.compute_looked(step_limit)?;
        if let Some(limit) = limits.client_changes
            && self.max_client_changes > limit
        {
            return Ok(FastApply::TooManyChanges {
                changes: self.max_client_changes,
                limit,
            });
        }
        if let Some(limit) = limits.total_changes
            && self.total_changes > limit
        {
            return Ok(FastApply::TooManyTotalChanges {
                changes: self.total_changes,
                limit,
            });
        }
        let reading_steps = looked.div_ceil(LOOKED_PER_STEP);
        let steps_ahead = self
            .total_changes
            .saturating_mul(self.non_empty_clients)
            .div_ceil(LOOKED_PER_STEP);
        let remake = reading_steps > step_limit;
        let summary = ApplySummary {
            state_bytes,
            changes: self.total_changes,
            client_changes: self.max_client_changes,
            reading_steps,
            steps_ahead,
            remake,
        };
        Ok(FastApply::Accepted {
            cache: (!remake).then_some(self),
            summary,
        })
    }

    fn matches_state_vector(&self, sv: &StateVector) -> bool {
        let sv_len = u64::try_from(sv.len()).unwrap_or(u64::MAX);
        if self.non_empty_clients != sv_len {
            return false;
        }
        for run in &self.runs {
            if run.is_empty() {
                continue;
            }
            if sv.get(&ClientID::new(run.client_id)) != run.end_clock {
                return false;
            }
        }
        true
    }

    fn integrate_new_blocks(
        &mut self,
        new_blocks: Vec<ParsedBlock<'_>>,
        txn_deletes: &[(u64, u32, u32)],
    ) -> bool {
        if new_blocks.is_empty() {
            return true;
        }
        let delete_index = DeleteIndex::of(txn_deletes);
        for block in &new_blocks {
            if (block.holds.is_squashable() || block.holds == Holds::Collected)
                && delete_index.overlaps(block.client, block.begin, block.end)
            {
                return false;
            }
        }
        let Some(ordered) = order_new_blocks(self, new_blocks) else {
            return false;
        };
        for block in ordered {
            if !self.integrate_one_block(&block, &delete_index) {
                return false;
            }
        }
        true
    }

    fn integrate_one_block(&mut self, block: &ParsedBlock<'_>, delete_index: &DeleteIndex) -> bool {
        if block.holds == Holds::Collected {
            return false;
        }
        let Some(client_idx) = self.get_or_add_client(block.client) else {
            return false;
        };
        if block.begin != self.runs[index(client_idx)].end_clock {
            return false;
        }
        if block.left.is_some() && block.left == block.right {
            return false;
        }
        // Same-piece inverted origins (`Refusal::Origin`) would split `origin_item` during
        // `resolve_for_right`; fall back before splitting anything. Distinct pieces of the
        // same client with `right < left` (e.g. `push_front` order from `from_body`) are
        // completely valid and conflict-free.
        if let (Some((lc, lk)), Some((rc, rk))) = (block.left, block.right)
            && lc == rc
            && rk <= lk
            && self.locate_id((lc, lk)) == self.locate_id((rc, rk))
        {
            return false;
        }
        let origin_item = match block.left {
            None => None,
            Some(id) => match self.resolve_for_left(id) {
                ResolvedRef::Item(idx) => Some(idx),
                ResolvedRef::Gone => None,
                ResolvedRef::Fallback => return false,
            },
        };
        let right_item = match block.right {
            None => None,
            Some(id) => match self.resolve_for_right(id) {
                ResolvedRef::Item(idx) => Some(idx),
                ResolvedRef::Gone => None,
                ResolvedRef::Fallback => return false,
            },
        };
        if !self.check_origin_invariants(block, origin_item, right_item) {
            return false;
        }
        let Some((parent, sub)) = self.resolve_place(block, origin_item, right_item) else {
            return false;
        };
        if !self.is_conflict_free(parent, sub, origin_item, right_item) {
            return false;
        }
        match self.try_squash_onto_prev(client_idx, block, origin_item, right_item, delete_index) {
            SquashOutcome::Squashed => true,
            SquashOutcome::Fallback => false,
            SquashOutcome::NotSquashed => {
                self.append_new_piece(client_idx, block, origin_item, right_item, parent, sub)
            }
        }
    }

    fn is_conflict_free(
        &self,
        parent: u32,
        sub: u8,
        origin_item: Option<u32>,
        right_item: Option<u32>,
    ) -> bool {
        match (origin_item, right_item) {
            (None, None) => {
                if sub == SUB_NONE {
                    !self.has_child(parent)
                } else {
                    !self.members.contains(&(parent, sub))
                }
            }
            (None, Some(r)) => {
                !(sub != SUB_NONE && self.members.contains(&(parent, sub)))
                    && self.pieces.get(index(r)).left_is_none()
            }
            (Some(l), right) => self.pieces.get(index(l)).right == right.unwrap_or(NONE),
        }
    }

    fn check_origin_invariants(
        &self,
        block: &ParsedBlock<'_>,
        origin_item: Option<u32>,
        right_item: Option<u32>,
    ) -> bool {
        if block.left.is_none()
            && let Some(r_idx) = right_item
            && !self.pieces.get(index(r_idx)).origin_is_none()
        {
            return false;
        }
        if block.left.is_some() && block.right.is_some() {
            match (origin_item, right_item) {
                (Some(l), Some(r)) => {
                    let lp = self.pieces.get(index(l));
                    let rp = self.pieces.get(index(r));
                    if lp.parent != rp.parent || lp.sub() != rp.sub() {
                        return false;
                    }
                    if lp.client_idx() == rp.client_idx()
                        && block.right <= block.left
                        && self.same_group_pieces(lp.client_idx(), l, r)
                    {
                        return false;
                    }
                }
                (None, None) => {}
                (Some(_), None) | (None, Some(_)) => return false,
            }
        }
        if let Some(Parent::Change(parent_id)) = block.parent
            && let Some((_, p_idx)) = self.locate_id(parent_id)
            && self.pieces.get(index(p_idx)).taken_item()
            && !matches!(
                self.pieces.get(index(p_idx)).holds(),
                Holds::Type | Holds::Deleted
            )
        {
            return false;
        }
        true
    }

    fn resolve_place(
        &mut self,
        block: &ParsedBlock<'_>,
        origin_item: Option<u32>,
        right_item: Option<u32>,
    ) -> Option<(u32, u8)> {
        let member = match block.member {
            None => SUB_NONE,
            Some(name) => self.get_or_add_member(name)?,
        };
        match block.parent {
            Some(Parent::Root(name)) => Some((self.get_or_add_root(name)?, member)),
            Some(Parent::Change(id)) => {
                let (_, p_idx) = self.locate_id(id)?;
                let piece = self.pieces.get(index(p_idx));
                if !piece.taken_item() || piece.holds() != Holds::Type {
                    return None;
                }
                Some((p_idx, member))
            }
            None => match (origin_item, right_item) {
                (Some(l), _) => {
                    let lp = self.pieces.get(index(l));
                    let sub = match lp.sub() {
                        SUB_NONE => {
                            right_item.map_or(SUB_NONE, |r| self.pieces.get(index(r)).sub())
                        }
                        sub => sub,
                    };
                    Some((lp.parent, sub))
                }
                (None, Some(r)) => {
                    let rp = self.pieces.get(index(r));
                    Some((rp.parent, rp.sub()))
                }
                (None, None) => None,
            },
        }
    }

    /// Checks whether `block` squashes onto `prev` in `yrs::TransactionMut::commit`,
    /// updating `prev` in place when supported (`Holds::Letters`) or signaling fallback.
    fn try_squash_onto_prev(
        &mut self,
        client_idx: u32,
        block: &ParsedBlock<'_>,
        origin_item: Option<u32>,
        right_item: Option<u32>,
        delete_index: &DeleteIndex,
    ) -> SquashOutcome {
        let run = &self.runs[index(client_idx)];
        if run.is_empty() {
            return SquashOutcome::NotSquashed;
        }
        let Some(prev_idx) = run.piece_at_block_index(run.block_count - 1) else {
            return SquashOutcome::Fallback;
        };
        let prev = self.pieces.get(index(prev_idx));
        let prev_end = self.piece_end(prev_idx);
        let Some(prev_last) = prev_end.checked_sub(1) else {
            return SquashOutcome::NotSquashed;
        };
        if origin_item != Some(prev_idx)
            || block.left != Some((block.client, u64::from(prev_last)))
            || prev.deleted()
            || delete_index.overlaps(block.client, prev_last, prev_end)
            || prev.holds() != block.holds
            || prev.right != right_item.unwrap_or(NONE)
            || !block.holds.is_squashable()
        {
            return SquashOutcome::NotSquashed;
        }
        if block.holds != Holds::Letters {
            return SquashOutcome::Fallback;
        }
        let Some(prev_extra) = self.extras.get(&prev_idx).copied() else {
            return SquashOutcome::Fallback;
        };
        let block_right_origin = block.right.map(|(rc, rk)| (rc, clock(rk)));
        if prev_extra.right_origin != block_right_origin {
            return SquashOutcome::NotSquashed;
        }
        if prev_extra.utf8_len == UNKNOWN_UTF8_UNDER_128 {
            return SquashOutcome::Fallback;
        }
        let Ok(added_utf8) = u32::try_from(block.utf8_len) else {
            return SquashOutcome::Fallback;
        };
        let Some(new_utf8) = prev_extra.utf8_len.checked_add(added_utf8) else {
            return SquashOutcome::Fallback;
        };
        let Some(new_len) = prev_extra.len.checked_add(block.end - block.begin) else {
            return SquashOutcome::Fallback;
        };
        let delta_payload = block
            .utf8_len
            .saturating_add(varint_len(u64::from(new_utf8)))
            .saturating_sub(varint_len(u64::from(prev_extra.utf8_len)));
        let Some(extra) = self.extras.get_mut(&prev_idx) else {
            return SquashOutcome::Fallback;
        };
        extra.len = new_len;
        extra.utf8_len = new_utf8;
        extra.has_non_bmp |= block.has_non_bmp;
        let run = &mut self.runs[index(client_idx)];
        run.payload_bytes = run.payload_bytes.saturating_add(delta_payload);
        run.end_clock = block.end;
        if let Some(dp) = run.dep_points.last_mut()
            && dp.begin == prev.begin
        {
            dp.end = block.end;
        }
        if let Some(sg) = run.squash_groups.last_mut()
            && sg.1 == prev_end
        {
            sg.1 = block.end;
        }
        SquashOutcome::Squashed
    }

    fn prev_group_start(&self, client_idx: u32, block: &ParsedBlock<'_>) -> Option<u32> {
        let run = &self.runs[index(client_idx)];
        if run.is_empty() || !block.holds.is_squashable() {
            return None;
        }
        let prev = run.piece_at_block_index(run.block_count - 1)?;
        let prev_p = self.pieces.get(index(prev));
        let prev_end = self.piece_end(prev);
        (prev_p.holds().is_squashable()
            && block.begin == prev_end
            && block.left == Some((block.client, u64::from(prev_end - 1))))
        .then_some(prev_p.begin)
    }

    fn append_new_piece(
        &mut self,
        client_idx: u32,
        block: &ParsedBlock<'_>,
        origin_item: Option<u32>,
        right_item: Option<u32>,
        parent: u32,
        sub: u8,
    ) -> bool {
        if self.pieces.len() >= usize::try_from(MAX_CACHED_IDS).unwrap_or(usize::MAX) {
            return false;
        }
        let Ok(client_u16) = u16::try_from(client_idx) else {
            return false;
        };
        let p_idx = small(self.pieces.len());
        let len = block.end - block.begin;
        let needs_extra = block.holds == Holds::Letters || len > 1;
        if needs_extra {
            let Ok(utf8_len) = u32::try_from(block.utf8_len) else {
                return false;
            };
            self.extras.insert(
                p_idx,
                PieceExtra {
                    len,
                    utf8_len: if block.holds == Holds::Letters {
                        utf8_len
                    } else {
                        0
                    },
                    has_non_bmp: block.holds == Holds::Letters && block.has_non_bmp,
                    right_origin: block.right.map(|(rc, rk)| (rc, clock(rk))),
                },
            );
        }
        let is_deleted = block.holds == Holds::Deleted;
        let next = if let Some(l) = origin_item {
            std::mem::replace(&mut self.pieces.get_mut(index(l)).right, p_idx)
        } else {
            if sub == SUB_NONE {
                self.set_has_child(parent);
            }
            right_item.unwrap_or(NONE)
        };
        if next != NONE {
            self.pieces.get_mut(index(next)).clear_left_is_none();
        } else if sub != SUB_NONE {
            self.members.insert((parent, sub));
        }
        let mut flag_bits = PieceFlags::TAKEN_ITEM;
        if block.left.is_none() {
            flag_bits |= PieceFlags::ORIGIN_NONE;
        }
        if origin_item.is_none() {
            flag_bits |= PieceFlags::LEFT_NONE;
        }
        if is_deleted {
            flag_bits |= PieceFlags::DELETED;
        }
        if needs_extra {
            flag_bits |= PieceFlags::HAS_EXTRA;
        }
        self.pieces.push(CompactPiece::new(
            block.begin,
            parent,
            next,
            client_u16,
            sub,
            block.holds,
            PieceFlags::from_bits(flag_bits),
        ));
        if is_deleted {
            self.delete_set
                .insert(ID::new(ClientID::new(block.client), block.begin), len);
        }
        let dep_point = self.cross_client_dep_from_block(client_idx, block);
        let prev_for_group = self.prev_group_start(client_idx, block);
        let run = &mut self.runs[index(client_idx)];
        let was_empty = run.is_empty();
        run.push_piece(p_idx, block.begin, block.end);
        run.payload_bytes = run.payload_bytes.saturating_add(block.wire_bytes);
        if let Some(dp) = dep_point {
            run.insert_dep_point(dp);
        }
        if let Some(prev_begin) = prev_for_group {
            run.note_squash_group(prev_begin, block.end);
        }
        if was_empty {
            self.non_empty_clients += 1;
        }
        self.total_changes += 1;
        self.max_client_changes = self.max_client_changes.max(u64::from(run.block_count));
        true
    }

    fn cross_client_dep_from_block(
        &self,
        client_idx: u32,
        block: &ParsedBlock<'_>,
    ) -> Option<DepPoint> {
        let mut deps = [(NONE, 0_u32); 3];
        let mut any = false;
        let parent_id = match block.parent {
            Some(Parent::Change(id)) => Some(id),
            _ => None,
        };
        for (slot, maybe_id) in [block.left, block.right, parent_id].into_iter().enumerate() {
            if let Some((dep_client, dep_clock)) = maybe_id
                && let Some(&dep_idx) = self.client_map.get(&dep_client)
                && dep_idx != client_idx
            {
                deps[slot] = (dep_idx, clock(dep_clock));
                any = true;
            }
        }
        any.then_some(DepPoint {
            begin: block.begin,
            end: block.end,
            deps,
        })
    }

    fn resolve_for_left(&mut self, (client, at): Id) -> ResolvedRef {
        let Some(&client_idx) = self.client_map.get(&client) else {
            return ResolvedRef::Gone;
        };
        let at_u32 = clock(at);
        let Some((block_idx, p_idx)) =
            self.runs[index(client_idx)].locate_clock(&self.pieces, at_u32)
        else {
            return ResolvedRef::Gone;
        };
        if !self.pieces.get(index(p_idx)).taken_item() {
            return ResolvedRef::Gone;
        }
        let p_end = self.piece_end(p_idx);
        if at_u32 + 1 < p_end && !self.split_piece_at(client_idx, block_idx, p_idx, at_u32 + 1) {
            return ResolvedRef::Fallback;
        }
        ResolvedRef::Item(p_idx)
    }

    fn resolve_for_right(&mut self, (client, at): Id) -> ResolvedRef {
        let Some(&client_idx) = self.client_map.get(&client) else {
            return ResolvedRef::Gone;
        };
        let at_u32 = clock(at);
        let Some((block_idx, p_idx)) =
            self.runs[index(client_idx)].locate_clock(&self.pieces, at_u32)
        else {
            return ResolvedRef::Gone;
        };
        if !self.pieces.get(index(p_idx)).taken_item() {
            return ResolvedRef::Gone;
        }
        let p_begin = self.pieces.get(index(p_idx)).begin;
        if at_u32 > p_begin {
            if !self.split_piece_at(client_idx, block_idx, p_idx, at_u32) {
                return ResolvedRef::Fallback;
            }
            let Some((_, right_idx)) =
                self.runs[index(client_idx)].locate_clock(&self.pieces, at_u32)
            else {
                return ResolvedRef::Fallback;
            };
            return ResolvedRef::Item(right_idx);
        }
        ResolvedRef::Item(p_idx)
    }

    /// Splits a `Holds::Letters` piece `p_idx` at `split_clock`, updating `pieces`, `extras`,
    /// `segments`, `payload_bytes`, `dep_points`, `squash_groups`, and change counts with
    /// exact `yrs::ItemPtr::splice` wire-size math.
    fn split_piece_at(
        &mut self,
        client_idx: u32,
        block_idx: u32,
        p_idx: u32,
        split_clock: u32,
    ) -> bool {
        if self.pieces.len() >= usize::try_from(MAX_CACHED_IDS).unwrap_or(usize::MAX) {
            return false;
        }
        let piece = self.pieces.get(index(p_idx));
        if piece.holds() != Holds::Letters || !piece.has_extra() {
            return false;
        }
        let extra = self.extras[&p_idx];
        if extra.has_non_bmp {
            return false;
        }
        let offset = split_clock.saturating_sub(piece.begin);
        if offset == 0 || offset >= extra.len {
            return false;
        }
        let right_len = extra.len - offset;
        let client_id = self.runs[index(client_idx)].client_id;
        let Some((b1, b2, delta_payload)) =
            split_payload_delta(client_id, split_clock, extra, offset)
        else {
            return false;
        };
        let Ok(client_u16) = u16::try_from(piece.client_idx()) else {
            return false;
        };
        let mut flag_bits = PieceFlags::HAS_EXTRA;
        if piece.taken_item() {
            flag_bits |= PieceFlags::TAKEN_ITEM;
        }
        if piece.deleted() {
            flag_bits |= PieceFlags::DELETED;
        }
        let new_idx = small(self.pieces.len());
        self.pieces.push(CompactPiece::new(
            split_clock,
            piece.parent,
            piece.right,
            client_u16,
            piece.sub(),
            Holds::Letters,
            PieceFlags::from_bits(flag_bits),
        ));
        self.pieces.get_mut(index(p_idx)).right = new_idx;
        self.extras.insert(
            p_idx,
            PieceExtra {
                len: offset,
                utf8_len: b1,
                has_non_bmp: false,
                right_origin: extra.right_origin,
            },
        );
        self.extras.insert(
            new_idx,
            PieceExtra {
                len: right_len,
                utf8_len: b2,
                has_non_bmp: false,
                right_origin: extra.right_origin,
            },
        );
        let right_dp = extra.right_origin.and_then(|(rc, rk)| {
            let &dep_idx = self.client_map.get(&rc)?;
            (dep_idx != client_idx).then_some(DepPoint {
                begin: split_clock,
                end: split_clock + right_len,
                deps: [(NONE, 0), (dep_idx, rk), (NONE, 0)],
            })
        });
        let run = &mut self.runs[index(client_idx)];
        if !run.insert_after_with_pieces(&self.pieces, block_idx, new_idx, split_clock) {
            return false;
        }
        run.payload_bytes = run.payload_bytes.saturating_add_signed(delta_payload);
        run.note_squash_group(piece.begin, piece.begin + extra.len);
        if let Ok(dp_pos) = run
            .dep_points
            .binary_search_by_key(&piece.begin, |dp| dp.begin)
        {
            run.dep_points[dp_pos].end = split_clock;
        }
        if let Some(dp) = right_dp {
            run.insert_dep_point(dp);
        }
        self.total_changes += 1;
        self.max_client_changes = self.max_client_changes.max(u64::from(run.block_count));
        true
    }

    fn apply_delete_ranges(&mut self, ranges: &[(u64, u32, u32)]) -> bool {
        for &(client_id, start, end) in ranges {
            if start >= end {
                continue;
            }
            let Some(&client_idx) = self.client_map.get(&client_id) else {
                return false;
            };
            if end > self.runs[index(client_idx)].end_clock {
                return false;
            }
            let mut at = start;
            while at < end {
                let Some((mut b_idx, mut p_idx)) =
                    self.runs[index(client_idx)].locate_clock(&self.pieces, at)
                else {
                    return false;
                };
                let piece = self.pieces.get(index(p_idx));
                let p_end = self.piece_end(p_idx);
                if piece.deleted() {
                    at = p_end;
                    continue;
                }
                if piece.begin < at {
                    if !self.split_piece_at(client_idx, b_idx, p_idx, at) {
                        return false;
                    }
                    let Some(next) = self.runs[index(client_idx)].locate_clock(&self.pieces, at)
                    else {
                        return false;
                    };
                    (b_idx, p_idx) = next;
                }
                let p_end = self.piece_end(p_idx);
                if p_end > end && !self.split_piece_at(client_idx, b_idx, p_idx, end) {
                    return false;
                }
                self.pieces.get_mut(index(p_idx)).set_deleted();
                if self.adjacent_deleted_squash_risk(client_idx, b_idx, p_idx) {
                    return false;
                }
                at = self.piece_end(p_idx);
            }
            self.delete_set
                .insert(ID::new(ClientID::new(client_id), start), end - start);
        }
        true
    }

    fn adjacent_deleted_squash_risk(&self, client_idx: u32, b_idx: u32, p_idx: u32) -> bool {
        let holds = self.pieces.get(index(p_idx)).holds();
        if !holds.is_squashable() {
            return false;
        }
        let run = &self.runs[index(client_idx)];
        let left_risk = b_idx > 0
            && run.piece_at_block_index(b_idx - 1).is_some_and(|p| {
                let piece = self.pieces.get(index(p));
                piece.deleted() && piece.holds() == holds
            });
        let right_risk = b_idx + 1 < run.block_count
            && run.piece_at_block_index(b_idx + 1).is_some_and(|p| {
                let piece = self.pieces.get(index(p));
                piece.deleted() && piece.holds() == holds
            });
        left_risk || right_risk
    }
}

type ParsedTxn<'bytes> = (Vec<ParsedBlock<'bytes>>, Vec<(u64, u32, u32)>);

fn parse_txn_update<'bytes>(
    txn_update: &'bytes [u8],
    before_sv: &StateVector,
) -> Result<Option<ParsedTxn<'bytes>>, Refusal> {
    let mut blocks = Vec::new();
    let mut deletes = Vec::new();
    let mut bad = false;
    let walked = scan::walk_detailed(
        txn_update,
        |(client, begin), change, wire_bytes| {
            let len = change.content.length();
            if len == 0 {
                return;
            }
            let end = begin.saturating_add(len);
            if scan::names_self(client, begin, end, change) {
                bad = true;
                return;
            }
            let (holds, utf8_len, has_non_bmp) = match change.content {
                Content::Collected(_) => (Holds::Collected, 0, false),
                Content::Deleted(_) => (Holds::Deleted, 0, false),
                Content::Letters(bytes) => (
                    Holds::Letters,
                    bytes.len(),
                    bytes.iter().any(|&b| b >= 0xf0),
                ),
                Content::Values(_) => (Holds::Values, 0, false),
                Content::Type => (Holds::Type, 0, false),
                Content::One => (Holds::One, 0, false),
            };
            blocks.push(ParsedBlock {
                client,
                begin: clock(begin),
                end: clock(end),
                left: change.left,
                right: change.right,
                parent: change.parent,
                member: change.member,
                holds,
                utf8_len,
                has_non_bmp,
                wire_bytes,
            });
        },
        |client, start, len| {
            if len > 0 {
                deletes.push((client, start, start.saturating_add(len)));
            }
        },
    )?;
    if bad {
        return Err(Refusal::Origin);
    }
    for (client, begin) in walked.begins {
        if begin != u64::from(before_sv.get(&ClientID::new(client))) {
            return Ok(None);
        }
    }
    Ok(Some((blocks, deletes)))
}

struct PendingRun<'bytes> {
    client_id: u64,
    blocks: VecDeque<ParsedBlock<'bytes>>,
    clock: u32,
    started: bool,
    stacked: bool,
}

fn order_new_blocks<'bytes>(
    cache: &ReadingCache,
    blocks: Vec<ParsedBlock<'bytes>>,
) -> Option<Vec<ParsedBlock<'bytes>>> {
    if blocks.len() <= 1 || blocks.iter().all(|b| b.client == blocks[0].client) {
        return Some(blocks);
    }
    let total = blocks.len();
    let mut by_client: HashMap<u64, VecDeque<ParsedBlock<'bytes>>> = HashMap::new();
    for block in blocks {
        by_client.entry(block.client).or_default().push_back(block);
    }
    let mut runs: Vec<PendingRun<'bytes>> = by_client
        .into_iter()
        .map(|(client_id, blocks)| {
            let clock = cache
                .client_map
                .get(&client_id)
                .map_or(0, |&idx| cache.runs[index(idx)].end_clock);
            PendingRun {
                client_id,
                blocks,
                clock,
                started: false,
                stacked: false,
            }
        })
        .collect();
    runs.sort_unstable_by_key(|run| run.client_id);
    let run_map: HashMap<u64, usize> = runs
        .iter()
        .enumerate()
        .map(|(idx, run)| (run.client_id, idx))
        .collect();
    let mut order: Vec<usize> = (0..runs.len()).collect();
    let mut stack: Vec<(usize, ParsedBlock<'bytes>)> = Vec::new();
    let mut ordered = Vec::with_capacity(total);
    let mut latest: Option<usize> = None;

    loop {
        let mut cur = match stack.pop() {
            Some((r_idx, block)) => {
                runs[r_idx].stacked = false;
                (r_idx, block)
            }
            None => loop {
                if let Some(r_idx) = latest
                    && let Some(block) = runs[r_idx].blocks.pop_front()
                {
                    break (r_idx, block);
                }
                let Some(r_idx) = order.pop() else {
                    return (ordered.len() == total).then_some(ordered);
                };
                runs[r_idx].started = true;
                latest = Some(r_idx);
            },
        };

        while let Some(missing_run) = first_missing_run(&cur.1, &run_map, &runs) {
            runs[cur.0].stacked = true;
            stack.push(cur);
            let target = &mut runs[missing_run];
            if target.started || target.stacked {
                return None;
            }
            let block = target.blocks.pop_front()?;
            cur = (missing_run, block);
        }

        runs[cur.0].clock = cur.1.end;
        ordered.push(cur.1);
    }
}

fn first_missing_run(
    block: &ParsedBlock<'_>,
    run_map: &HashMap<u64, usize>,
    runs: &[PendingRun<'_>],
) -> Option<usize> {
    let parent_id = match block.parent {
        Some(Parent::Change(id)) => Some(id),
        _ => None,
    };
    [block.left, block.right, parent_id]
        .into_iter()
        .flatten()
        .find_map(|(dep_c, dep_k)| {
            let &r_idx = run_map.get(&dep_c)?;
            (clock(dep_k) >= runs[r_idx].clock).then_some(r_idx)
        })
}

/// Per-client sorted, merged deletion intervals for `O(log D)` overlap queries.
struct DeleteIndex(HashMap<u64, Vec<(u32, u32)>>);

impl DeleteIndex {
    fn of(deletes: &[(u64, u32, u32)]) -> Self {
        if deletes.is_empty() {
            return Self(HashMap::new());
        }
        let mut by: HashMap<u64, Vec<(u32, u32)>> = HashMap::new();
        for &(client, start, end) in deletes {
            if start < end {
                by.entry(client).or_default().push((start, end));
            }
        }
        for ranges in by.values_mut() {
            ranges.sort_unstable();
            ranges.dedup_by(|next, kept| {
                (next.0 <= kept.1)
                    .then(|| kept.1 = kept.1.max(next.1))
                    .is_some()
            });
        }
        Self(by)
    }

    fn overlaps(&self, client: u64, begin: u32, end: u32) -> bool {
        let Some(ranges) = self.0.get(&client) else {
            return false;
        };
        let at = ranges.partition_point(|&(_, de)| de <= begin);
        ranges.get(at).is_some_and(|&(ds, _)| ds < end)
    }
}
