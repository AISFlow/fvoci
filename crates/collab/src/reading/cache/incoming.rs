//! O(|update|) pre-apply origin and partial-overlap validation against [`ReadingCache`].
//!
//! Replaces `reading::check_update_origins(prior_state, known, update)` when `ReadingCache`
//! is populated, blocking all misplaced origins, non-type parents, cross-parent/member
//! neighbours, inverted same-group origins, and partial-overlap `yrs` 0.28.1 panic bombs
//! without scanning `prior_state`.

use std::collections::HashMap;

use yrs::StateVector;

use super::{ROOT_BIT, ReadingCache, SUB_NONE, TEMP_BRANCH_BIT, TEMP_ROOT_BIT};
use crate::reading::origins::{OverlapParent, PendingChange, collect_incoming};
use crate::reading::{Holds, NONE, clock, index, small};
use crate::scan::{self, Content, Id, Parent, Refusal};

/// Offset for temporary incoming member IDs so they never collide with cached `u8` members.
const TEMP_SUB_BASE: u32 = 0x1000;

/// Unified placement of a resolved item (either existing in [`ReadingCache`] or newly
/// resolved in the incoming mini-replay).
#[derive(Debug, Clone, Copy)]
struct ResolvedPiece {
    parent: u32,
    sub: u32,
    holds: Holds,
    /// Origin space for group comparison:
    /// - `GroupTag::Cached(client_idx, piece_idx)` for cached items
    /// - `GroupTag::Incoming { group, prior_tail_client }` for incoming items
    group: GroupTag,
    /// `true` iff this item is an existing item that continues into the client's tail group.
    client_id: u64,
}

#[derive(Debug, Clone, Copy)]
enum GroupTag {
    Cached {
        client_idx: usize,
        piece_idx: u32,
    },
    Incoming {
        group: u32,
        /// `Some((cached_client_idx, last_cached_piece_idx))` if this incoming group
        /// continues the squashable chain of the cached client's final piece.
        joins_cached_tail: Option<(usize, u32)>,
    },
}

struct IncomingPiece<'bytes> {
    client: u64,
    begin: u32,
    end: u32,
    left: Option<Id>,
    right: Option<Id>,
    parent_named: Option<Parent<'bytes>>,
    member_named: Option<&'bytes [u8]>,
    holds: Holds,
    group: u32,
    joins_cached_tail: Option<(usize, u32)>,
    taken: IncomingTaken,
    parent: u32,
    sub: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IncomingTaken {
    Waiting,
    Item,
    Gone,
}

struct IncomingRun {
    client_id: u64,
    first: usize,
    after: usize,
    next: usize,
    clock: u32,
    started: bool,
    stacked: bool,
}

impl ReadingCache {
    /// Validates every incoming change's origins and partial-overlap anchors in `update`
    /// against `self` in `O(|update|)` time without touching `prior_state`, matching
    /// [`crate::reading::check_update_origins`] 100%.
    pub(crate) fn check_incoming_origins(
        &self,
        known: &StateVector,
        update: &[u8],
    ) -> Result<(), Refusal> {
        let collected = collect_incoming(known, update)?;
        let mut mini = MiniReplay::build(self, collected.changes)?;
        mini.run()?;
        mini.verify_overlaps(&collected.overlaps)
    }

    fn lookup_cached_piece(&self, (client, at): Id) -> Option<(u32, ResolvedPiece)> {
        let &client_idx = self.client_map.get(&client)?;
        let (_, p_idx) = self.runs[index(client_idx)].locate_clock(&self.pieces, clock(at))?;
        let piece = self.pieces.get(index(p_idx));
        if !piece.taken_item() {
            return None;
        }
        Some((
            p_idx,
            ResolvedPiece {
                parent: piece.parent,
                sub: if piece.sub() == SUB_NONE {
                    NONE
                } else {
                    u32::from(piece.sub())
                },
                holds: piece.holds(),
                group: GroupTag::Cached {
                    client_idx: index(client_idx),
                    piece_idx: p_idx,
                },
                client_id: client,
            },
        ))
    }
}

fn content_holds(content: &Content<'_>) -> Holds {
    match content {
        Content::Collected(_) => Holds::Collected,
        Content::Deleted(_) => Holds::Deleted,
        Content::Letters(_) => Holds::Letters,
        Content::Values(_) => Holds::Values,
        Content::Type => Holds::Type,
        Content::One => Holds::One,
    }
}

struct MiniReplay<'a, 'bytes> {
    cache: &'a ReadingCache,
    pieces: Vec<IncomingPiece<'bytes>>,
    runs: Vec<IncomingRun>,
    run_map: HashMap<u64, usize>,
    temp_roots: HashMap<&'bytes [u8], u32>,
    temp_members: HashMap<&'bytes [u8], u32>,
}

impl<'a, 'bytes> MiniReplay<'a, 'bytes> {
    fn build(
        cache: &'a ReadingCache,
        incoming: HashMap<u64, Vec<PendingChange<'bytes>>>,
    ) -> Result<Self, Refusal> {
        let mut clients: Vec<(u64, Vec<PendingChange<'bytes>>)> = incoming.into_iter().collect();
        clients.sort_unstable_by_key(|(c, _)| *c);
        let mut mini = Self {
            cache,
            pieces: Vec::new(),
            runs: Vec::with_capacity(clients.len()),
            run_map: HashMap::with_capacity(clients.len()),
            temp_roots: HashMap::new(),
            temp_members: HashMap::new(),
        };
        let mut group = 0_u32;
        for (client_id, pending_list) in clients {
            mini.append_client_pieces(client_id, pending_list, &mut group)?;
        }
        Ok(mini)
    }

    fn append_client_pieces(
        &mut self,
        client_id: u64,
        pending_list: Vec<PendingChange<'bytes>>,
        group: &mut u32,
    ) -> Result<(), Refusal> {
        let first = self.pieces.len();
        let cached_tail = self.cache.client_map.get(&client_id).and_then(|&c_idx| {
            let run = &self.cache.runs[index(c_idx)];
            if run.is_empty() {
                return None;
            }
            let last_p = run.piece_at_block_index(run.block_count - 1)?;
            let piece = self.cache.pieces.get(index(last_p));
            Some((index(c_idx), last_p, piece, run.end_clock))
        });
        let initial_clock = cached_tail.map_or(0, |(_, _, _, end)| end);
        let mut prev_end = initial_clock;
        let mut prev_squashable =
            cached_tail.is_some_and(|(_, _, piece, _)| piece.holds().is_squashable());
        let mut current_tail_link = cached_tail.map(|(ci, pi, _, _)| (ci, pi));

        for pending in pending_list {
            let len = pending.change.content.length();
            if len == 0 {
                continue;
            }
            let end = pending.begin.saturating_add(len);
            if scan::names_self(client_id, pending.begin, end, &pending.change) {
                return Err(Refusal::Origin);
            }
            let holds = content_holds(&pending.change.content);
            let begin_u32 = clock(pending.begin);
            let end_u32 = clock(end);
            let chains_prev = holds.is_squashable()
                && prev_squashable
                && begin_u32 == prev_end
                && prev_end > 0
                && pending.change.left == Some((client_id, u64::from(prev_end - 1)));
            let joins_cached_tail = if chains_prev {
                current_tail_link
            } else {
                *group = group.saturating_add(1);
                current_tail_link = None;
                None
            };
            self.pieces.push(IncomingPiece {
                client: client_id,
                begin: begin_u32,
                end: end_u32,
                left: pending.change.left,
                right: pending.change.right,
                parent_named: pending.change.parent,
                member_named: pending.change.member,
                holds,
                group: *group,
                joins_cached_tail,
                taken: IncomingTaken::Waiting,
                parent: NONE,
                sub: NONE,
            });
            prev_end = end_u32;
            prev_squashable = holds.is_squashable();
        }
        if self.pieces.len() > first {
            let run_idx = self.runs.len();
            self.run_map.insert(client_id, run_idx);
            self.runs.push(IncomingRun {
                client_id,
                first,
                after: self.pieces.len(),
                next: first,
                clock: initial_clock,
                started: false,
                stacked: false,
            });
            *group = group.saturating_add(1);
        }
        Ok(())
    }

    fn run(&mut self) -> Result<(), Refusal> {
        let mut order: Vec<usize> = (0..self.runs.len()).collect();
        order.sort_unstable_by_key(|&idx| self.runs[idx].client_id);
        let mut stack: Vec<(usize, usize)> = Vec::new();
        let mut latest: Option<usize> = None;

        loop {
            let mut cur = match stack.pop() {
                Some((r_idx, p_idx)) => {
                    self.runs[r_idx].stacked = false;
                    (r_idx, p_idx)
                }
                None => loop {
                    if let Some(r_idx) = latest
                        && self.runs[r_idx].next < self.runs[r_idx].after
                    {
                        let p_idx = self.runs[r_idx].next;
                        self.runs[r_idx].next += 1;
                        break (r_idx, p_idx);
                    }
                    let Some(r_idx) = order.pop() else {
                        return Ok(());
                    };
                    self.runs[r_idx].started = true;
                    latest = Some(r_idx);
                },
            };

            while let Some(missing_run) = self.missing(cur.1)? {
                stack.push(cur);
                self.runs[cur.0].stacked = true;
                let target = &mut self.runs[missing_run];
                if target.started || target.stacked || target.next >= target.after {
                    return Err(Refusal::Gap);
                }
                let next_p = target.next;
                target.next += 1;
                cur = (missing_run, next_p);
            }

            self.take(cur.0, cur.1)?;
        }
    }

    fn missing(&self, p_idx: usize) -> Result<Option<usize>, Refusal> {
        let piece = &self.pieces[p_idx];
        if piece.holds == Holds::Collected {
            return Ok(None);
        }
        let parent = match piece.parent_named {
            Some(Parent::Change(id)) => Some(id),
            _ => None,
        };
        for id in [piece.left, piece.right, parent].into_iter().flatten() {
            let dep_clock = clock(id.1);
            if let Some(&run_idx) = self.run_map.get(&id.0) {
                if dep_clock >= self.runs[run_idx].clock {
                    return Ok(Some(run_idx));
                }
            } else {
                let known_end = self
                    .cache
                    .client_map
                    .get(&id.0)
                    .map_or(0, |&ci| self.cache.runs[index(ci)].end_clock);
                if dep_clock >= known_end {
                    return Err(Refusal::Gap);
                }
            }
        }
        Ok(None)
    }

    fn take(&mut self, run_idx: usize, p_idx: usize) -> Result<(), Refusal> {
        let end = self.pieces[p_idx].end;
        self.runs[run_idx].clock = end;
        let holds = self.pieces[p_idx].holds;
        if holds == Holds::Collected {
            self.pieces[p_idx].taken = IncomingTaken::Gone;
            return Ok(());
        }
        let left_id = self.pieces[p_idx].left;
        let right_id = self.pieces[p_idx].right;
        if left_id.is_some() && left_id == right_id {
            return Err(Refusal::Origin);
        }
        let left_res = left_id.and_then(|id| self.lookup_item(id));
        let right_res = right_id.and_then(|id| self.lookup_item(id));
        if left_id.is_some() && right_id.is_some() {
            match (left_res, right_res) {
                (Some(l), Some(r)) => {
                    if l.parent != r.parent || l.sub != r.sub {
                        return Err(Refusal::Origin);
                    }
                    if right_id <= left_id && self.same_group(l, r) {
                        return Err(Refusal::Origin);
                    }
                }
                (None, None) => {}
                (Some(_), None) | (None, Some(_)) => return Err(Refusal::Origin),
            }
        }
        let parent_named = self.pieces[p_idx].parent_named;
        let member_named = self.pieces[p_idx].member_named;
        if let Some(Parent::Change(pid)) = parent_named
            && let Some(parent_res) = self.lookup_item(pid)
            && !matches!(parent_res.holds, Holds::Type | Holds::Deleted)
        {
            return Err(Refusal::Origin);
        }
        let place = self.place_of(parent_named, member_named, left_res, right_res)?;
        match place {
            Some((parent, sub)) => {
                let piece = &mut self.pieces[p_idx];
                piece.taken = IncomingTaken::Item;
                piece.parent = parent;
                piece.sub = sub;
            }
            None => {
                self.pieces[p_idx].taken = IncomingTaken::Gone;
            }
        }
        Ok(())
    }

    fn place_of(
        &mut self,
        parent_named: Option<Parent<'bytes>>,
        member_named: Option<&'bytes [u8]>,
        left: Option<ResolvedPiece>,
        right: Option<ResolvedPiece>,
    ) -> Result<Option<(u32, u32)>, Refusal> {
        let sub = match member_named {
            None => NONE,
            Some(name) => self.resolve_member_id(name),
        };
        Ok(match parent_named {
            Some(Parent::Root(name)) => Some((self.resolve_root_id(name), sub)),
            Some(Parent::Change(pid)) => match self.lookup_item_with_branch(pid) {
                None => None,
                Some((branch_id, res)) => match res.holds {
                    Holds::Type => Some((branch_id, sub)),
                    Holds::Deleted | Holds::Collected => None,
                    Holds::Letters | Holds::Values | Holds::One => return Err(Refusal::Unknown),
                },
            },
            None => match (left, right) {
                (Some(l), _) => {
                    let eff_sub = match l.sub {
                        NONE => right.map_or(NONE, |r| r.sub),
                        s => s,
                    };
                    Some((l.parent, eff_sub))
                }
                (None, Some(r)) => Some((r.parent, r.sub)),
                (None, None) => None,
            },
        })
    }

    fn verify_overlaps(&mut self, overlaps: &[OverlapParent<'bytes>]) -> Result<(), Refusal> {
        for overlap in overlaps {
            let Some(anchor) = self.lookup_item(overlap.anchor) else {
                return Err(Refusal::Origin);
            };
            let expected = (anchor.parent, anchor.sub);
            if let Some(parent) = overlap.parent {
                let target = self.resolve_overlap_target(parent, overlap.member);
                if target != Some(expected) {
                    return Err(Refusal::Origin);
                }
            } else if let Some(left) = overlap.left {
                let Some(left_piece) = self.lookup_item(left) else {
                    return Err(Refusal::Origin);
                };
                if (left_piece.parent, left_piece.sub) != expected {
                    return Err(Refusal::Origin);
                }
            }
        }
        Ok(())
    }

    fn resolve_overlap_target(
        &mut self,
        parent: Parent<'bytes>,
        member: Option<&'bytes [u8]>,
    ) -> Option<(u32, u32)> {
        let branch = match parent {
            Parent::Root(name) => self.resolve_root_id(name),
            Parent::Change(id) => {
                let (branch_id, res) = self.lookup_item_with_branch(id)?;
                if res.holds != Holds::Type {
                    return None;
                }
                branch_id
            }
        };
        let sub = match member {
            None => NONE,
            Some(name) => self.resolve_member_id(name),
        };
        Some((branch, sub))
    }

    fn resolve_root_id(&mut self, name: &'bytes [u8]) -> u32 {
        if let Some(&idx) = self.cache.roots.get(name) {
            return ROOT_BIT | idx;
        }
        let next = TEMP_ROOT_BIT | small(self.temp_roots.len());
        *self.temp_roots.entry(name).or_insert(next)
    }

    fn resolve_member_id(&mut self, name: &'bytes [u8]) -> u32 {
        if let Some(&idx) = self.cache.member_map.get(name) {
            return u32::from(idx);
        }
        let next = TEMP_SUB_BASE + small(self.temp_members.len());
        *self.temp_members.entry(name).or_insert(next)
    }

    fn lookup_item(&self, id: Id) -> Option<ResolvedPiece> {
        self.lookup_item_with_branch(id).map(|(_, res)| res)
    }

    fn lookup_item_with_branch(&self, id: Id) -> Option<(u32, ResolvedPiece)> {
        let at = clock(id.1);
        if let Some(&run_idx) = self.run_map.get(&id.0) {
            let run = &self.runs[run_idx];
            let slice = &self.pieces[run.first..run.after];
            let within = slice.partition_point(|p| p.begin <= at);
            if let Some(pos) = within.checked_sub(1) {
                let p_idx = run.first + pos;
                let piece = &self.pieces[p_idx];
                if at < piece.end {
                    if piece.taken != IncomingTaken::Item {
                        return None;
                    }
                    return Some((
                        TEMP_BRANCH_BIT | small(p_idx),
                        ResolvedPiece {
                            parent: piece.parent,
                            sub: piece.sub,
                            holds: piece.holds,
                            group: GroupTag::Incoming {
                                group: piece.group,
                                joins_cached_tail: piece.joins_cached_tail,
                            },
                            client_id: piece.client,
                        },
                    ));
                }
            }
        }
        self.cache.lookup_cached_piece(id)
    }

    fn same_group(&self, a: ResolvedPiece, b: ResolvedPiece) -> bool {
        if a.client_id != b.client_id {
            return false;
        }
        match (a.group, b.group) {
            (
                GroupTag::Cached {
                    client_idx: ca,
                    piece_idx: pa,
                },
                GroupTag::Cached { piece_idx: pb, .. },
            ) => self.cache.same_group_pieces(ca, pa, pb),
            (GroupTag::Incoming { group: ga, .. }, GroupTag::Incoming { group: gb, .. }) => {
                ga == gb
            }
            (
                GroupTag::Cached {
                    client_idx,
                    piece_idx,
                },
                GroupTag::Incoming {
                    joins_cached_tail: Some((tc, tp)),
                    ..
                },
            )
            | (
                GroupTag::Incoming {
                    joins_cached_tail: Some((tc, tp)),
                    ..
                },
                GroupTag::Cached {
                    client_idx,
                    piece_idx,
                },
            ) => client_idx == tc && self.cache.same_group_pieces(client_idx, piece_idx, tp),
            _ => false,
        }
    }
}
