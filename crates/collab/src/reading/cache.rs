//! Incremental cache for Pass 5 (`state_of`) and Pass 6 (`reading::count_strict`).
//!
//! Avoids re-encoding the full Yjs document and re-walking all blocks on every live
//! conflict-free edit and deletion-only repair, while maintaining 100% exact equivalence
//! with `state_of` + `reading::count_strict` + `reading::check_update_origins`.
//!
//! Any edit that introduces a YATA conflict, non-YATA origin, unsupported split, or
//! non-`Letters` squash returns [`FastApply::Fallback`] (consuming the cache so no
//! stale or partial cache can ever survive) and lets the caller fall back to full
//! `counted_sized` and rebuild a fresh cache.

mod apply;
mod build;
mod direct;
mod incoming;
mod picker;

use std::collections::{HashMap, HashSet};

use yrs::IdSet;

pub(crate) use build::count_strict_with_cache;
pub(crate) use direct::{DirectItem, ReadingCacheBuilder};

use super::{Holds, clock, index, small};
use crate::scan::Id;

/// High bit set on [`CompactPiece::parent`] when the parent is a root branch.
const ROOT_BIT: u32 = 0x8000_0000;
/// High bit reserved for temporary incoming roots in `incoming::MiniReplay`.
pub(super) const TEMP_ROOT_BIT: u32 = 0x4000_0000;
/// High bit reserved for temporary incoming branch pieces in `incoming::MiniReplay`.
pub(super) const TEMP_BRANCH_BIT: u32 = 0x2000_0000;
/// Exclusive upper bound on cached piece and root indices, guaranteeing bit-disjointness
/// with [`ROOT_BIT`], [`TEMP_ROOT_BIT`], and [`TEMP_BRANCH_BIT`].
pub(super) const MAX_CACHED_IDS: u32 = TEMP_BRANCH_BIT;
/// Maximum number of clients supported by the 15-bit `client_idx` in [`CompactPiece`].
const MAX_CACHED_CLIENTS: usize = 0x7fff;
/// Maximum number of distinct member names supported by the 8-bit `sub` (`0xFF` = `NONE`).
const MAX_CACHED_MEMBERS: usize = 255;
/// Sentinel `sub` value in [`CompactPiece`] meaning no member (`NONE`).
const SUB_NONE: u8 = u8::MAX;
/// Sentinel `utf8_len` after splitting a mixed-UTF-8 string whose whole length was `< 128`
/// bytes: every further split also has `utf8_len < 128` (`varint_len == 1`), so split byte
/// deltas stay exact, while same-client squash onto this piece falls back.
const UNKNOWN_UTF8_UNDER_128: u32 = u32::MAX;
/// Cap on per-client segment count before falling back to keep lookups O(1).
const MAX_RUN_SEGMENTS: usize = 2048;

/// Two-tier storage for [`CompactPiece`]: an exact-sized immutable-length `base` slice
/// built at open/settle (zero slack, never reallocates) plus a small `extra` vector for
/// pieces appended or split during live updates.
#[derive(Debug)]
struct PieceStore {
    base: Box<[CompactPiece]>,
    extra: Vec<CompactPiece>,
}

impl PieceStore {
    fn empty() -> Self {
        Self {
            base: Box::new([]),
            extra: Vec::new(),
        }
    }

    fn from_base(base: Vec<CompactPiece>) -> Self {
        Self {
            base: base.into_boxed_slice(),
            extra: Vec::new(),
        }
    }

    fn len(&self) -> usize {
        self.base.len() + self.extra.len()
    }

    #[inline]
    fn get(&self, idx: usize) -> CompactPiece {
        if idx < self.base.len() {
            self.base[idx]
        } else {
            self.extra[idx - self.base.len()]
        }
    }

    #[inline]
    fn get_mut(&mut self, idx: usize) -> &mut CompactPiece {
        if idx < self.base.len() {
            &mut self.base[idx]
        } else {
            &mut self.extra[idx - self.base.len()]
        }
    }

    fn push(&mut self, piece: CompactPiece) {
        self.extra.push(piece);
    }

    /// Binary-searches the contiguous piece range `[first .. first + len)` for the first
    /// piece whose `begin > at`.
    fn partition_point_begin(&self, first: usize, len: usize, at: u32) -> usize {
        let base_len = self.base.len();
        if first + len <= base_len {
            self.base[first..first + len].partition_point(|piece| piece.begin <= at)
        } else if first >= base_len {
            let lo = first - base_len;
            self.extra[lo..lo + len].partition_point(|piece| piece.begin <= at)
        } else {
            // Segment spans the boundary between `base` and `extra`.
            let mut lo = 0_usize;
            let mut hi = len;
            while lo < hi {
                let mid = lo + (hi - lo) / 2;
                if self.get(first + mid).begin <= at {
                    lo = mid + 1;
                } else {
                    hi = mid;
                }
            }
            lo
        }
    }
}

/// Packed boolean flag bits stored in the top 6 bits of [`CompactPiece::meta`].
#[derive(Debug, Clone, Copy, Default)]
struct PieceFlags(u32);

impl PieceFlags {
    const TAKEN_ITEM: u32 = 1 << 26;
    const ORIGIN_NONE: u32 = 1 << 27;
    const LEFT_NONE: u32 = 1 << 28;
    const DELETED: u32 = 1 << 29;
    const HAS_EXTRA: u32 = 1 << 30;
    const HAS_CHILD: u32 = 1 << 31;
    const ALL_MASK: u32 = Self::TAKEN_ITEM
        | Self::ORIGIN_NONE
        | Self::LEFT_NONE
        | Self::DELETED
        | Self::HAS_EXTRA
        | Self::HAS_CHILD;

    const fn from_bits(bits: u32) -> Self {
        Self(bits & Self::ALL_MASK)
    }
}

/// 16-byte compact piece stored in [`ReadingCache::pieces`].
#[derive(Debug, Clone, Copy)]
struct CompactPiece {
    begin: u32,
    parent: u32,
    right: u32,
    meta: u32,
}

impl CompactPiece {
    const CLIENT_MASK: u32 = 0x7fff;
    const SUB_SHIFT: u32 = 15;
    const SUB_MASK: u32 = 0xff;
    const HOLDS_SHIFT: u32 = 23;
    const HOLDS_MASK: u32 = 0x7;

    fn new(
        begin: u32,
        parent: u32,
        right: u32,
        client_idx: u16,
        sub: u8,
        holds: Holds,
        flags: PieceFlags,
    ) -> Self {
        let mut meta = u32::from(client_idx) & Self::CLIENT_MASK;
        meta |= u32::from(sub) << Self::SUB_SHIFT;
        meta |= u32::from(holds_to_u8(holds)) << Self::HOLDS_SHIFT;
        meta |= flags.0;
        Self {
            begin,
            parent,
            right,
            meta,
        }
    }

    fn client_idx(self) -> usize {
        usize::try_from(self.meta & Self::CLIENT_MASK).unwrap_or(usize::MAX)
    }

    fn sub(self) -> u8 {
        u8::try_from((self.meta >> Self::SUB_SHIFT) & Self::SUB_MASK).unwrap_or(SUB_NONE)
    }

    fn holds(self) -> Holds {
        u8_to_holds(u8::try_from((self.meta >> Self::HOLDS_SHIFT) & Self::HOLDS_MASK).unwrap_or(5))
    }

    fn taken_item(self) -> bool {
        self.meta & PieceFlags::TAKEN_ITEM != 0
    }

    fn origin_is_none(self) -> bool {
        self.meta & PieceFlags::ORIGIN_NONE != 0
    }

    fn left_is_none(self) -> bool {
        self.meta & PieceFlags::LEFT_NONE != 0
    }

    fn clear_left_is_none(&mut self) {
        self.meta &= !PieceFlags::LEFT_NONE;
    }

    fn deleted(self) -> bool {
        self.meta & PieceFlags::DELETED != 0
    }

    fn set_deleted(&mut self) {
        self.meta |= PieceFlags::DELETED;
    }

    fn has_extra(self) -> bool {
        self.meta & PieceFlags::HAS_EXTRA != 0
    }

    fn has_child(self) -> bool {
        self.meta & PieceFlags::HAS_CHILD != 0
    }

    fn set_has_child(&mut self) {
        self.meta |= PieceFlags::HAS_CHILD;
    }
}

fn holds_to_u8(holds: Holds) -> u8 {
    match holds {
        Holds::Collected => 0,
        Holds::Deleted => 1,
        Holds::Letters => 2,
        Holds::Values => 3,
        Holds::Type => 4,
        Holds::One => 5,
    }
}

fn u8_to_holds(code: u8) -> Holds {
    match code {
        0 => Holds::Collected,
        1 => Holds::Deleted,
        2 => Holds::Letters,
        3 => Holds::Values,
        4 => Holds::Type,
        _ => Holds::One,
    }
}

/// Sparse metadata stored only for `Holds::Letters` pieces or pieces with `len > 1`.
#[derive(Debug, Clone, Copy)]
struct PieceExtra {
    len: u32,
    utf8_len: u32,
    /// True if a `Holds::Letters` piece contains any 4-byte UTF-8 (surrogate pair) scalar;
    /// splitting it at an arbitrary UTF-16 offset may trigger `yrs::split_surrogate`
    /// (`U+FFFD` replacement, +2 UTF-8 bytes), so splits on such pieces fall back.
    has_non_bmp: bool,
    right_origin: Option<(u64, u32)>,
}

/// Contiguous slice of piece indices in [`ReadingCache::pieces`] belonging to one client.
#[derive(Debug, Clone, Copy)]
struct RunSegment {
    start_idx: u32,
    first_clock: u32,
    first_piece: u32,
    len: u32,
}

/// One cross-client dependency point of a block of a client, for compressed `BlockPicker`.
#[derive(Debug, Clone, Copy)]
pub(super) struct DepPoint {
    pub(super) begin: u32,
    pub(super) end: u32,
    pub(super) deps: [(u32, u32); 3],
}

/// Per-client block run tracked by [`ReadingCache`].
#[derive(Debug)]
struct ClientRun {
    client_id: u64,
    segments: Vec<RunSegment>,
    block_count: u32,
    payload_bytes: usize,
    end_clock: u32,
    dep_points: Vec<DepPoint>,
    squash_groups: Vec<(u32, u32)>,
}

impl ClientRun {
    fn new(client_id: u64) -> Self {
        Self {
            client_id,
            segments: Vec::new(),
            block_count: 0,
            payload_bytes: 0,
            end_clock: 0,
            dep_points: Vec::new(),
            squash_groups: Vec::new(),
        }
    }

    fn is_empty(&self) -> bool {
        self.block_count == 0
    }

    fn push_piece(&mut self, piece_idx: u32, begin: u32, end: u32) {
        if let Some(last) = self.segments.last_mut()
            && last.first_piece + last.len == piece_idx
        {
            last.len += 1;
        } else {
            self.segments.push(RunSegment {
                start_idx: self.block_count,
                first_clock: begin,
                first_piece: piece_idx,
                len: 1,
            });
        }
        self.block_count += 1;
        self.end_clock = end;
    }

    fn locate_clock(&self, pieces: &PieceStore, at: u32) -> Option<(u32, u32)> {
        if self.is_empty() || at >= self.end_clock {
            return None;
        }
        let seg_idx = self
            .segments
            .partition_point(|seg| seg.first_clock <= at)
            .checked_sub(1)?;
        let seg = self.segments[seg_idx];
        let within = pieces
            .partition_point_begin(index(seg.first_piece), index(seg.len), at)
            .checked_sub(1)?;
        let offset = small(within);
        Some((seg.start_idx + offset, seg.first_piece + offset))
    }

    fn piece_at_block_index(&self, block_idx: u32) -> Option<u32> {
        if block_idx >= self.block_count {
            return None;
        }
        let seg_idx = self
            .segments
            .partition_point(|seg| seg.start_idx <= block_idx)
            .checked_sub(1)?;
        let seg = self.segments[seg_idx];
        Some(seg.first_piece + (block_idx - seg.start_idx))
    }

    fn insert_after_with_pieces(
        &mut self,
        pieces: &PieceStore,
        block_idx: u32,
        new_piece: u32,
        new_clock: u32,
    ) -> bool {
        if self.segments.len() + 2 > MAX_RUN_SEGMENTS {
            return false;
        }
        let seg_idx = self
            .segments
            .partition_point(|seg| seg.start_idx <= block_idx)
            .saturating_sub(1);
        let seg = self.segments[seg_idx];
        let offset = block_idx - seg.start_idx;
        let left_len = offset + 1;
        let right_len = seg.len - left_len;
        let inserted = RunSegment {
            start_idx: block_idx + 1,
            first_clock: new_clock,
            first_piece: new_piece,
            len: 1,
        };
        if right_len == 0 {
            self.segments.insert(seg_idx + 1, inserted);
            for tail in &mut self.segments[seg_idx + 2..] {
                tail.start_idx += 1;
            }
        } else {
            let right_first = seg.first_piece + left_len;
            let right = RunSegment {
                start_idx: block_idx + 2,
                first_clock: pieces.get(index(right_first)).begin,
                first_piece: right_first,
                len: right_len,
            };
            self.segments[seg_idx].len = left_len;
            let insert_at = seg_idx + 1;
            self.segments
                .splice(insert_at..insert_at, [inserted, right]);
            for tail in &mut self.segments[seg_idx + 3..] {
                tail.start_idx += 1;
            }
        }
        self.block_count += 1;
        true
    }

    fn note_squash_group(&mut self, start: u32, end: u32) {
        let pos = self.squash_groups.partition_point(|&(s, _)| s < start);
        let idx = if pos > 0 && self.squash_groups[pos - 1].1 > start {
            self.squash_groups[pos - 1].1 = self.squash_groups[pos - 1].1.max(end);
            pos - 1
        } else if pos < self.squash_groups.len() && end > self.squash_groups[pos].0 {
            self.squash_groups[pos].0 = self.squash_groups[pos].0.min(start);
            self.squash_groups[pos].1 = self.squash_groups[pos].1.max(end);
            pos
        } else {
            self.squash_groups.insert(pos, (start, end));
            return;
        };
        while idx + 1 < self.squash_groups.len()
            && self.squash_groups[idx].1 > self.squash_groups[idx + 1].0
        {
            let next_end = self.squash_groups.remove(idx + 1).1;
            self.squash_groups[idx].1 = self.squash_groups[idx].1.max(next_end);
        }
    }

    fn insert_dep_point(&mut self, point: DepPoint) {
        let pos = self.dep_points.partition_point(|dp| dp.begin < point.begin);
        if pos < self.dep_points.len() && self.dep_points[pos].begin == point.begin {
            self.dep_points[pos] = point;
        } else {
            self.dep_points.insert(pos, point);
        }
    }
}

/// Summary metrics produced by a successful incremental [`ReadingCache`] update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ApplySummary {
    pub(crate) state_bytes: usize,
    pub(crate) changes: u64,
    pub(crate) client_changes: u64,
    pub(crate) reading_steps: u64,
    pub(crate) steps_ahead: u64,
    pub(crate) remake: bool,
}

/// Outcome of [`ReadingCache::try_apply`] or [`ReadingCache::try_apply_deletions`].
#[derive(Debug)]
pub(crate) enum FastApply {
    /// Incremental validation and accounting succeeded with exact metrics.
    Accepted {
        cache: Option<Box<ReadingCache>>,
        summary: ApplySummary,
    },
    /// Resulting state exceeds `limits.state_bytes`.
    TooLarge { size: usize, limit: usize },
    /// Resulting max changes of one client exceeds `limits.client_changes`.
    TooManyChanges { changes: u64, limit: u64 },
    /// Resulting total changes across all clients exceeds `limits.total_changes`.
    TooManyTotalChanges { changes: u64, limit: u64 },
    /// Could not take the fast path; cache consumed and caller must fall back to full scan.
    Fallback,
}

/// Incremental Pass 5 + Pass 6 cache attached to a [`crate::BodyDoc`].
#[derive(Debug)]
pub(crate) struct ReadingCache {
    pieces: PieceStore,
    extras: HashMap<u32, PieceExtra>,
    runs: Vec<ClientRun>,
    client_map: HashMap<u64, u32>,
    roots: HashMap<Box<[u8]>, u32>,
    root_has_child: Vec<bool>,
    member_map: HashMap<Box<[u8]>, u8>,
    members: HashSet<(u32, u8)>,
    delete_set: IdSet,
    total_changes: u64,
    max_client_changes: u64,
    non_empty_clients: u64,
}

impl ReadingCache {
    /// Creates a cache representing an empty Y document (`[0, 0]`, 2 bytes).
    #[must_use]
    pub(crate) fn empty() -> Self {
        Self {
            pieces: PieceStore::empty(),
            extras: HashMap::new(),
            runs: Vec::new(),
            client_map: HashMap::new(),
            roots: HashMap::new(),
            root_has_child: Vec::new(),
            member_map: HashMap::new(),
            members: HashSet::new(),
            delete_set: IdSet::new(),
            total_changes: 0,
            max_client_changes: 0,
            non_empty_clients: 0,
        }
    }

    /// Exact cut charge (`len * cut_charge(holds)`) if `id` falls strictly inside an
    /// existing block of `self`, matching `placing::Clocks::cut_inside` in `O(log N)`.
    #[must_use]
    pub(crate) fn cut_inside(&self, (client, at): Id) -> Option<u64> {
        let &client_idx = self.client_map.get(&client)?;
        let at_u32 = u32::try_from(at).ok()?;
        let (_, p_idx) = self.runs[index(client_idx)].locate_clock(&self.pieces, at_u32)?;
        let piece = self.pieces.get(index(p_idx));
        if piece.begin >= at_u32 {
            return None;
        }
        let len = u64::from(self.piece_len(p_idx));
        let per_unit = match piece.holds() {
            Holds::Letters => 1,
            Holds::Values => crate::placing::VALUE_PLACES,
            Holds::Collected | Holds::Deleted | Holds::Type | Holds::One => 0,
        };
        Some(len.saturating_mul(per_unit))
    }

    /// Exact byte length of `state_of(&doc)` for the cached document state.
    #[must_use]
    pub(crate) fn state_bytes(&self) -> usize {
        let mut blocks_bytes = 0_usize;
        for run in &self.runs {
            if run.is_empty() {
                continue;
            }
            blocks_bytes += varint_len(u64::from(run.block_count));
            blocks_bytes += varint_len(run.client_id);
            blocks_bytes += 1;
            blocks_bytes += run.payload_bytes;
        }
        varint_len(self.non_empty_clients) + blocks_bytes + delete_set_encoded_len(&self.delete_set)
    }

    fn piece_len(&self, piece_idx: u32) -> u32 {
        let piece = self.pieces.get(index(piece_idx));
        if piece.has_extra() {
            self.extras[&piece_idx].len
        } else {
            1
        }
    }

    fn piece_end(&self, piece_idx: u32) -> u32 {
        self.pieces
            .get(index(piece_idx))
            .begin
            .saturating_add(self.piece_len(piece_idx))
    }

    fn has_child(&self, parent: u32) -> bool {
        if parent & ROOT_BIT != 0 {
            self.root_has_child[index(parent & !ROOT_BIT)]
        } else {
            self.pieces.get(index(parent)).has_child()
        }
    }

    fn set_has_child(&mut self, parent: u32) {
        if parent & ROOT_BIT != 0 {
            self.root_has_child[index(parent & !ROOT_BIT)] = true;
        } else {
            self.pieces.get_mut(index(parent)).set_has_child();
        }
    }

    fn get_or_add_client(&mut self, client_id: u64) -> Option<u32> {
        if let Some(&idx) = self.client_map.get(&client_id) {
            return Some(idx);
        }
        if self.runs.len() >= MAX_CACHED_CLIENTS {
            return None;
        }
        let idx = small(self.runs.len());
        self.client_map.insert(client_id, idx);
        self.runs.push(ClientRun::new(client_id));
        Some(idx)
    }

    fn get_or_add_root(&mut self, name: &[u8]) -> Option<u32> {
        if let Some(&idx) = self.roots.get(name) {
            return Some(ROOT_BIT | idx);
        }
        if self.root_has_child.len() >= usize::try_from(MAX_CACHED_IDS).ok()? {
            return None;
        }
        let idx = small(self.root_has_child.len());
        self.roots.insert(Box::from(name), idx);
        self.root_has_child.push(false);
        Some(ROOT_BIT | idx)
    }

    fn get_or_add_member(&mut self, name: &[u8]) -> Option<u8> {
        if let Some(&idx) = self.member_map.get(name) {
            return Some(idx);
        }
        if self.member_map.len() >= MAX_CACHED_MEMBERS {
            return None;
        }
        let idx = u8::try_from(self.member_map.len()).ok()?;
        self.member_map.insert(Box::from(name), idx);
        Some(idx)
    }

    fn locate_id(&self, (client, at): Id) -> Option<(u32, u32)> {
        let &client_idx = self.client_map.get(&client)?;
        self.runs[index(client_idx)].locate_clock(&self.pieces, clock(at))
    }

    fn same_group_pieces(&self, client_idx: usize, a_idx: u32, b_idx: u32) -> bool {
        if a_idx == b_idx {
            return true;
        }
        let a_begin = self.pieces.get(index(a_idx)).begin;
        let b_begin = self.pieces.get(index(b_idx)).begin;
        let (lo, hi) = if a_begin <= b_begin {
            (a_begin, b_begin)
        } else {
            (b_begin, a_begin)
        };
        let groups = &self.runs[client_idx].squash_groups;
        let pos = groups.partition_point(|&(s, _)| s <= lo);
        pos > 0 && groups[pos - 1].1 > hi
    }
}

/// Exact LEB128 varint byte length used by `yrs::updates::encoder::EncoderV1`.
pub(super) fn varint_len(mut n: u64) -> usize {
    let mut len = 1;
    while n >= 0x80 {
        n >>= 7;
        len += 1;
    }
    len
}

/// Exact byte length of `IdSet::encode(&mut EncoderV1)`.
pub(super) fn delete_set_encoded_len(ds: &IdSet) -> usize {
    let mut clients = 0_u64;
    let mut bytes = 0_usize;
    for (client, ranges) in ds.iter() {
        clients += 1;
        bytes += varint_len(client.get());
        bytes += varint_len(u64::try_from(ranges.len()).unwrap_or(u64::MAX));
        for range in ranges.iter() {
            bytes += varint_len(u64::from(range.start));
            bytes += varint_len(u64::from(range.end - range.start));
        }
    }
    varint_len(clients) + bytes
}

fn split_payload_delta(
    client_id: u64,
    split_clock: u32,
    extra: PieceExtra,
    offset: u32,
) -> Option<(u32, u32, isize)> {
    let (b1, b2, varint_delta) = split_utf8_bytes(extra.utf8_len, extra.len, offset)?;
    let mut a2_header = 1 + varint_len(client_id) + varint_len(u64::from(split_clock - 1));
    if let Some((rc, rk)) = extra.right_origin {
        a2_header += varint_len(rc) + varint_len(u64::from(rk));
    }
    let delta_payload = isize::try_from(a2_header).ok()?.checked_add(varint_delta)?;
    Some((b1, b2, delta_payload))
}

fn varint_split_delta(b1: u32, b2: u32, whole: u32) -> Option<isize> {
    let after = isize::try_from(varint_len(u64::from(b1)) + varint_len(u64::from(b2))).ok()?;
    let before = isize::try_from(varint_len(u64::from(whole))).ok()?;
    Some(after - before)
}

/// Computes exact `(b1, b2, varint_len(b1) + varint_len(b2) - varint_len(b))` when
/// splitting a `String` of `utf8_len` UTF-8 bytes and `utf16_len` UTF-16 code units at
/// UTF-16 `offset`.
fn split_utf8_bytes(utf8_len: u32, utf16_len: u32, offset: u32) -> Option<(u32, u32, isize)> {
    if utf8_len == UNKNOWN_UTF8_UNDER_128 {
        return Some((UNKNOWN_UTF8_UNDER_128, UNKNOWN_UTF8_UNDER_128, 1));
    }
    if utf8_len == utf16_len {
        let b1 = offset;
        let b2 = utf16_len - offset;
        return Some((b1, b2, varint_split_delta(b1, b2, utf8_len)?));
    }
    if utf16_len.checked_mul(3) == Some(utf8_len) {
        let b1 = offset.checked_mul(3)?;
        let b2 = utf8_len - b1;
        return Some((b1, b2, varint_split_delta(b1, b2, utf8_len)?));
    }
    if utf8_len < 128 {
        return Some((UNKNOWN_UTF8_UNDER_128, UNKNOWN_UTF8_UNDER_128, 1));
    }
    None
}

#[cfg(test)]
mod tests {
    use yrs::updates::encoder::{Encode as _, Encoder as _, EncoderV1};
    use yrs::{ClientID, ID};

    use super::*;

    #[test]
    fn compact_piece_is_sixteen_bytes() {
        assert_eq!(
            std::mem::size_of::<CompactPiece>(),
            16,
            "CompactPiece must remain 16 bytes for memory density"
        );
    }

    #[test]
    fn varint_and_delete_set_encoded_len_match_yrs_encoder_v1() {
        for value in [
            0_u64,
            1,
            127,
            128,
            16_383,
            16_384,
            (1 << 21) - 1,
            1 << 21,
            (1 << 53) - 1,
        ] {
            let mut enc = EncoderV1::new();
            yrs::encoding::write::Write::write_var(&mut enc, value);
            assert_eq!(varint_len(value), enc.to_vec().len(), "value={value}");
        }
        let mut ds = IdSet::new();
        for (c, s, l) in [
            (1_u64, 0_u32, 10_u32),
            (1, 200, 300),
            (999_999_999, 16_384, 128),
            ((1 << 53) - 1, 0, 1),
        ] {
            ds.insert(ID::new(ClientID::new(c), s), l);
        }
        let mut enc = EncoderV1::new();
        ds.encode(&mut enc);
        assert_eq!(delete_set_encoded_len(&ds), enc.to_vec().len());
    }

    #[test]
    fn split_utf8_bytes_handles_ascii_korean_short_mixed_and_long_mixed() {
        // 4,000,000 is 4 varint bytes (> 2^21 - 1); 2,000,000 is 3 bytes => 3 + 3 - 4 = 2.
        assert_eq!(
            split_utf8_bytes(4_000_000, 4_000_000, 2_000_000),
            Some((2_000_000, 2_000_000, 2))
        );
        assert_eq!(split_utf8_bytes(600, 200, 100), Some((300, 300, 2)));
        assert_eq!(
            split_utf8_bytes(80, 50, 20),
            Some((UNKNOWN_UTF8_UNDER_128, UNKNOWN_UTF8_UNDER_128, 1))
        );
        assert_eq!(split_utf8_bytes(200, 100, 50), None);
    }

    #[test]
    fn try_apply_deletions_enforces_total_changes_on_piece_split() {
        use yrs::{Doc, OffsetKind, Options, Transact as _, XmlElementPrelim};
        use yrs::{XmlFragment as _, XmlTextPrelim};

        let doc = Doc::with_options(Options {
            offset_kind: OffsetKind::Utf16,
            skip_gc: true,
            ..Options::with_client_id(ClientID::new(10))
        });
        let root = doc.get_or_insert_xml_fragment(crate::FIELD);
        {
            let mut txn = doc.transact_mut();
            let p = root.push_back(&mut txn, XmlElementPrelim::empty("paragraph"));
            p.push_back(&mut txn, XmlTextPrelim::new("abcdef"));
        }
        let state = crate::state_of(&doc);
        let limits = crate::UpdateLimits {
            update_bytes: 64 * 1024,
            state_bytes: 16 * 1024 * 1024,
            body_bytes: fvoci_model::BODY_MAX_BYTES,
            placing_steps: Some(crate::PLACING_STEPS),
            clients: Some(crate::MAX_CLIENTS),
            client_changes: Some(crate::MAX_CLIENT_CHANGES),
            total_changes: Some(4),
            reading_steps: Some(crate::READING_STEPS),
        };
        let mut ds = IdSet::new();
        // Delete `"cd"` (clocks 4..6) inside `"abcdef"` (clocks 2..8): splits 3 -> 5 changes.
        ds.insert(ID::new(ClientID::new(10), 4), 2);

        let (_, cache) =
            count_strict_with_cache(&state, crate::READING_STEPS).expect("cache builds");
        let refused = cache
            .expect("cache present")
            .try_apply_deletions(&ds, &limits)
            .expect("deletion check succeeds");
        assert!(
            matches!(
                refused,
                FastApply::TooManyTotalChanges {
                    changes: 5,
                    limit: 4
                }
            ),
            "expected TooManyTotalChanges(5 > 4), got {refused:?}"
        );

        let (_, cache) =
            count_strict_with_cache(&state, crate::READING_STEPS).expect("cache builds");
        let exact = crate::UpdateLimits {
            total_changes: Some(5),
            ..limits
        };
        let accepted = cache
            .expect("cache present")
            .try_apply_deletions(&ds, &exact)
            .expect("deletion check succeeds");
        match accepted {
            FastApply::Accepted { summary, .. } => assert_eq!(summary.changes, 5),
            other => panic!("expected Accepted at exact cap 5, got {other:?}"),
        }
    }
}
