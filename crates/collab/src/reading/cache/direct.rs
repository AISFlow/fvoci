//! Direct `O(N)` [`ReadingCache`] builder used when constructing a [`crate::BodyDoc`]
//! from a stored [`fvoci_model::Body`] without re-scanning or replaying its encoded state.

use super::{
    ClientRun, CompactPiece, DepPoint, MAX_CACHED_CLIENTS, MAX_CACHED_IDS, MAX_CACHED_MEMBERS,
    PieceExtra, PieceFlags, PieceStore, ROOT_BIT, ReadingCache, SUB_NONE,
};
use crate::reading::{Holds, NONE, index, small};

/// One wire item recorded into [`ReadingCacheBuilder`] as `build::wire` emits it in
/// `push_front` order (`origin == None`, strictly descending `ClientID` chunks).
#[derive(Debug, Clone, Copy)]
pub(crate) struct DirectItem<'a> {
    pub(crate) client_idx: u16,
    pub(crate) begin: u32,
    pub(crate) len: u32,
    /// `ReadingCacheBuilder::ROOT_DEFAULT` for root children, or the parent's `piece_idx`.
    pub(crate) parent: u32,
    /// `(right_client_idx, right_clock, right_piece_idx)` when this item names a right
    /// neighbour (`HAS_RIGHT_ORIGIN`), or `None` for the first item emitted in its parent.
    pub(crate) right: Option<(u16, u32, u32)>,
    /// `(parent_client_idx, parent_clock)` when `right` is `None` and the parent is a
    /// nested shared type (`Named::Item`).
    pub(crate) parent_dep: Option<(u16, u32)>,
    /// Attribute name when writing a map member on an element (`HAS_PARENT_SUB`).
    pub(crate) member: Option<&'a [u8]>,
    pub(crate) holds: Holds,
    pub(crate) utf8_len: u32,
    pub(crate) has_non_bmp: bool,
    pub(crate) wire_bytes: usize,
}

/// Incremental builder that constructs an exact [`ReadingCache`] alongside `build::wire`.
///
/// Every item emitted by `build::wire` has `left == None`, points only at already-emitted
/// items from the same or higher `ClientID`, and has zero conflicts, zero cuts, and zero
/// deletions. Calling [`Self::finish`] still verifies `cache.state_bytes() == expected`
/// before returning `Some`.
#[derive(Debug)]
pub(crate) struct ReadingCacheBuilder {
    cache: ReadingCache,
    base_pieces: Vec<CompactPiece>,
    failed: bool,
}

impl ReadingCacheBuilder {
    /// Encoded parent ID for the body's root XML fragment (`FIELD`).
    pub(crate) const ROOT_DEFAULT: u32 = ROOT_BIT;

    /// Creates a new builder pre-registered with `root_name` at index 0 (`ROOT_DEFAULT`),
    /// pre-allocating space for `estimated_items` pieces.
    #[must_use]
    pub(crate) fn with_capacity(root_name: &[u8], estimated_items: usize) -> Self {
        let mut cache = ReadingCache::empty();
        cache.roots.insert(Box::from(root_name), 0);
        cache.root_has_child.push(false);
        let max_ids = usize::try_from(MAX_CACHED_IDS).unwrap_or(usize::MAX);
        Self {
            cache,
            base_pieces: Vec::with_capacity(estimated_items.min(max_ids)),
            failed: false,
        }
    }

    /// Registers the next client chunk in descending `ClientID` order.
    pub(crate) fn start_client(&mut self, client_id: u64) -> Option<u16> {
        if self.failed {
            return None;
        }
        if self.cache.runs.len() >= MAX_CACHED_CLIENTS
            || self.cache.client_map.contains_key(&client_id)
        {
            self.failed = true;
            return None;
        }
        let idx = small(self.cache.runs.len());
        let Ok(idx_u16) = u16::try_from(idx) else {
            self.failed = true;
            return None;
        };
        self.cache.client_map.insert(client_id, idx);
        self.cache.runs.push(ClientRun::new(client_id));
        Some(idx_u16)
    }

    /// Records one freshly encoded item and returns its piece index.
    pub(crate) fn record_item(&mut self, item: DirectItem<'_>) -> Option<u32> {
        let result = self.try_record_item(item);
        if result.is_none() {
            self.failed = true;
        }
        result
    }

    fn try_record_item(&mut self, item: DirectItem<'_>) -> Option<u32> {
        if self.failed {
            return None;
        }
        let max_ids = usize::try_from(MAX_CACHED_IDS).unwrap_or(usize::MAX);
        if self.base_pieces.len() >= max_ids {
            return None;
        }
        let sub = self.intern_member(item.member)?;
        let p_idx = small(self.base_pieces.len());
        let needs_extra = item.holds == Holds::Letters || item.len > 1;
        if needs_extra {
            self.record_extra(p_idx, &item)?;
        }
        let right_idx = self.link_parent_and_right(&item, sub)?;
        let end = item.begin.saturating_add(item.len);
        let run = self.cache.runs.get_mut(usize::from(item.client_idx))?;
        let mut flag_bits =
            PieceFlags::TAKEN_ITEM | PieceFlags::ORIGIN_NONE | PieceFlags::LEFT_NONE;
        if needs_extra {
            flag_bits |= PieceFlags::HAS_EXTRA;
        }
        self.base_pieces.push(CompactPiece::new(
            item.begin,
            item.parent,
            right_idx,
            item.client_idx,
            sub,
            item.holds,
            PieceFlags::from_bits(flag_bits),
        ));
        run.push_piece(p_idx, item.begin, end);
        run.payload_bytes = run.payload_bytes.saturating_add(item.wire_bytes);
        if let Some(dp) = cross_client_dep(&item, end) {
            run.dep_points.push(dp);
        }
        Some(p_idx)
    }

    fn intern_member(&mut self, member: Option<&[u8]>) -> Option<u8> {
        let Some(name) = member else {
            return Some(SUB_NONE);
        };
        if let Some(&idx) = self.cache.member_map.get(name) {
            return Some(idx);
        }
        if self.cache.member_map.len() < MAX_CACHED_MEMBERS
            && let Ok(idx) = u8::try_from(self.cache.member_map.len())
        {
            self.cache.member_map.insert(Box::from(name), idx);
            Some(idx)
        } else {
            None
        }
    }

    fn record_extra(&mut self, p_idx: u32, item: &DirectItem<'_>) -> Option<()> {
        let right_origin = match item.right {
            None => None,
            Some((rc, rk, _)) => Some((self.cache.runs.get(usize::from(rc))?.client_id, rk)),
        };
        self.cache.extras.insert(
            p_idx,
            PieceExtra {
                len: item.len,
                utf8_len: if item.holds == Holds::Letters {
                    item.utf8_len
                } else {
                    0
                },
                has_non_bmp: item.holds == Holds::Letters && item.has_non_bmp,
                right_origin,
            },
        );
        Some(())
    }

    fn link_parent_and_right(&mut self, item: &DirectItem<'_>, sub: u8) -> Option<u32> {
        let right_idx = match item.right {
            Some((_, _, r_idx)) => {
                self.base_pieces.get_mut(index(r_idx))?.clear_left_is_none();
                r_idx
            }
            None => NONE,
        };
        if sub == SUB_NONE {
            if item.parent & ROOT_BIT != 0 {
                let slot = self
                    .cache
                    .root_has_child
                    .get_mut(index(item.parent & !ROOT_BIT))?;
                *slot = true;
            } else {
                self.base_pieces
                    .get_mut(index(item.parent))?
                    .set_has_child();
            }
        } else {
            self.cache.members.insert((item.parent, sub));
        }
        Some(right_idx)
    }

    /// Finalizes the [`ReadingCache`], returning `None` if any limit was exceeded or if
    /// the tracked wire size disagrees with `expected_state_bytes`.
    #[must_use]
    pub(crate) fn finish(mut self, expected_state_bytes: usize) -> Option<Box<ReadingCache>> {
        if self.failed {
            return None;
        }
        if self.base_pieces.is_empty() {
            return (expected_state_bytes == 2).then(|| Box::new(ReadingCache::empty()));
        }
        self.cache.pieces = PieceStore::from_base(self.base_pieces);
        for run in &self.cache.runs {
            if !run.is_empty() {
                self.cache.non_empty_clients += 1;
                let count = u64::from(run.block_count);
                self.cache.total_changes += count;
                self.cache.max_client_changes = self.cache.max_client_changes.max(count);
            }
        }
        if self.cache.state_bytes() != expected_state_bytes {
            return None;
        }
        Some(Box::new(self.cache))
    }
}

fn cross_client_dep(item: &DirectItem<'_>, end: u32) -> Option<DepPoint> {
    let mut deps = [(NONE, 0_u32); 3];
    let mut any = false;
    if let Some((rc, rk, _)) = item.right {
        if rc != item.client_idx {
            deps[1] = (u32::from(rc), rk);
            any = true;
        }
    } else if let Some((pc, pk)) = item.parent_dep
        && pc != item.client_idx
    {
        deps[2] = (u32::from(pc), pk);
        any = true;
    }
    any.then_some(DepPoint {
        begin: item.begin,
        end,
        deps,
    })
}
