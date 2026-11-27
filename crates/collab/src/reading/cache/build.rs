//! Building a [`ReadingCache`] in the same single scan as `reading::count_strict`.

use yrs::{ClientID, ID, IdSet};

use super::{
    ClientRun, CompactPiece, DepPoint, MAX_CACHED_CLIENTS, MAX_CACHED_IDS, MAX_CACHED_MEMBERS,
    PieceExtra, PieceFlags, PieceStore, ROOT_BIT, ReadingCache, SUB_NONE,
};
use crate::reading::{
    Holds, NO_ID, NONE, Named, OriginPolicy, Piece, Read, Replay, StepsMode, Taken, index, small,
    unpack,
};
use crate::scan::{self, Content, Refusal};

/// Extra per-state details collected during `read_detailed` for building a [`ReadingCache`].
struct ReplayDetails {
    client_payload_bytes: Vec<usize>,
    /// `(utf8_len, has_non_bmp)` per piece (nonzero only for `Holds::Letters`).
    piece_text_meta: Vec<(u32, bool)>,
    delete_set: IdSet,
}

/// As [`crate::reading::count_strict`], also building a [`ReadingCache`] in the same
/// single pass when `state` is conflict-free, cut-free, and within `stop_past`.
pub(crate) fn count_strict_with_cache(
    state: &[u8],
    stop_past: u64,
) -> Result<(Read, Option<Box<ReadingCache>>), Refusal> {
    let (walked, mut replay, details) = read_detailed(state, OriginPolicy::Strict)?;
    replay.run(StepsMode::Count(stop_past))?;
    let cache = from_replay(&replay, details, state.len(), stop_past);
    let read = Read {
        walked,
        steps: replay.steps(),
        ahead: replay.ahead(),
        ends: replay.ends(),
        needs_settle: replay.needs_settle,
    };
    Ok((read, cache))
}

/// Walks `state` as `Replay::read` does while recording per-client wire payload bytes,
/// `Letters` UTF-8 lengths, and the delete set in the same single scan.
fn read_detailed(
    state: &[u8],
    origins: OriginPolicy,
) -> Result<(scan::Walked, Replay<'_>, ReplayDetails), Refusal> {
    let mut replay = Replay::empty(origins);
    let mut self_named = false;
    let mut client_payload_bytes: Vec<usize> = Vec::new();
    let mut piece_text_meta: Vec<(u32, bool)> = Vec::new();
    let mut delete_set = IdSet::new();
    let walked = scan::walk_detailed(
        state,
        |(client, begin), change, wire_bytes| {
            let len = change.content.length();
            if origins == OriginPolicy::Strict
                && len > 0
                && scan::names_self(client, begin, begin.saturating_add(len), change)
            {
                self_named = true;
            }
            let client_idx = replay.client(client);
            if client_payload_bytes.len() <= index(client_idx) {
                client_payload_bytes.resize(index(client_idx) + 1, 0);
            }
            client_payload_bytes[index(client_idx)] =
                client_payload_bytes[index(client_idx)].saturating_add(wire_bytes);
            let before = replay.pieces.len();
            replay.keep((client, begin), change);
            if replay.pieces.len() > before {
                let meta = match change.content {
                    Content::Letters(bytes) => (
                        u32::try_from(bytes.len()).unwrap_or(u32::MAX),
                        bytes.iter().any(|&b| b >= 0xf0),
                    ),
                    _ => (0, false),
                };
                piece_text_meta.push(meta);
            }
        },
        |del_client, start, len| {
            if len > 0 {
                delete_set.insert(ID::new(ClientID::new(del_client), start), len);
            }
        },
    )?;
    if self_named {
        return Err(Refusal::Origin);
    }
    if walked.begins.iter().any(|(_, begin)| *begin > 0) {
        return Err(Refusal::Gap);
    }
    replay.roots = vec![NONE; replay.names.roots.len()];
    replay.cut();
    let details = ReplayDetails {
        client_payload_bytes,
        piece_text_meta,
        delete_set,
    };
    Ok((walked, replay, details))
}

/// Builds a [`ReadingCache`] from a completed, conflict-free, cut-free [`Replay`].
///
/// Returns `None` if the state had conflicts, cuts, `needs_settle`, exceeded `stop_past`,
/// exceeded compact index ranges, or failed exact `state_bytes` verification.
fn from_replay(
    replay: &Replay<'_>,
    details: ReplayDetails,
    state_len: usize,
    stop_past: u64,
) -> Option<Box<ReadingCache>> {
    if replay.steps() > stop_past
        || replay.needs_settle
        || replay.walked > 0
        || replay.moved > 0
        || replay.copied > 0
        || replay.emptied > 0
        || replay.clients.len() > MAX_CACHED_CLIENTS
        || replay.names.members.len() > MAX_CACHED_MEMBERS
        || replay.pieces.len() >= usize::try_from(MAX_CACHED_IDS).ok()?
        || replay.names.roots.len() >= usize::try_from(MAX_CACHED_IDS).ok()?
    {
        return None;
    }
    let mut cache = ReadingCache::empty();
    init_tables(&mut cache, replay)?;
    populate_runs_and_pieces(&mut cache, replay, &details)?;
    apply_initial_deletes(&mut cache, details.delete_set)?;
    if cache.state_bytes() != state_len {
        return None;
    }
    Some(Box::new(cache))
}

fn init_tables(cache: &mut ReadingCache, replay: &Replay<'_>) -> Option<()> {
    cache.runs.reserve(replay.clients.len());
    cache.client_map.reserve(replay.clients.len());
    for (idx, &client_id) in replay.clients.iter().enumerate() {
        let num = small(idx);
        cache.client_map.insert(client_id, num);
        cache.runs.push(ClientRun::new(client_id));
    }
    cache.roots.reserve(replay.names.roots.len());
    cache.root_has_child = vec![false; replay.names.roots.len()];
    for (&name, &root_idx) in &replay.names.roots {
        cache.roots.insert(Box::from(name), root_idx);
        cache.root_has_child[index(root_idx)] = replay.roots[index(root_idx)] != NONE;
    }
    cache.member_map.reserve(replay.names.members.len());
    for (&name, &member_idx) in &replay.names.members {
        let idx = u8::try_from(member_idx).ok()?;
        cache.member_map.insert(Box::from(name), idx);
    }
    let piece_count = small(replay.pieces.len());
    for &(branch, sub) in replay.members.keys() {
        let parent = translate_parent(branch, piece_count);
        let sub_u8 = u8::try_from(sub).ok()?;
        cache.members.insert((parent, sub_u8));
    }
    Some(())
}

fn populate_runs_and_pieces(
    cache: &mut ReadingCache,
    replay: &Replay<'_>,
    details: &ReplayDetails,
) -> Option<()> {
    let piece_count = small(replay.pieces.len());
    let mut base_pieces = Vec::with_capacity(replay.pieces.len());
    for (idx, piece) in replay.pieces.iter().enumerate() {
        if piece.continues || piece.taken == Taken::Waiting {
            return None;
        }
        let p_idx = small(idx);
        let len = piece.end.checked_sub(piece.begin)?;
        let needs_extra = piece.holds == Holds::Letters || len > 1;
        if needs_extra {
            let right_origin = (piece.right_origin != NO_ID).then(|| {
                let (rc, rk) = unpack(piece.right_origin);
                (replay.clients[index(rc)], rk)
            });
            let (utf8_len, has_non_bmp) = if piece.holds == Holds::Letters {
                details.piece_text_meta[idx]
            } else {
                (0, false)
            };
            cache.extras.insert(
                p_idx,
                PieceExtra {
                    len,
                    utf8_len,
                    has_non_bmp,
                    right_origin,
                },
            );
        }
        let parent = translate_parent(piece.parent, piece_count);
        let sub = if piece.sub == NONE {
            SUB_NONE
        } else {
            u8::try_from(piece.sub).ok()?
        };
        let mut flag_bits = 0_u32;
        if piece.taken == Taken::Item {
            flag_bits |= PieceFlags::TAKEN_ITEM;
        }
        if piece.origin == NO_ID {
            flag_bits |= PieceFlags::ORIGIN_NONE;
        }
        if piece.left == NONE {
            flag_bits |= PieceFlags::LEFT_NONE;
        }
        if needs_extra {
            flag_bits |= PieceFlags::HAS_EXTRA;
        }
        if piece.start != NONE {
            flag_bits |= PieceFlags::HAS_CHILD;
        }
        let compact = CompactPiece::new(
            piece.begin,
            parent,
            piece.right,
            u16::try_from(piece.client).ok()?,
            sub,
            piece.holds,
            PieceFlags::from_bits(flag_bits),
        );
        base_pieces.push(compact);
        let run = &mut cache.runs[index(piece.client)];
        run.push_piece(p_idx, piece.begin, piece.end);
        if let Some(dp) = cross_client_dep_from_replay(piece) {
            run.dep_points.push(dp);
        }
        if idx > 0 {
            let prev = &replay.pieces[idx - 1];
            if prev.client == piece.client && prev.group == piece.group {
                run.note_squash_group(prev.begin, piece.end);
            }
        }
    }
    cache.pieces = PieceStore::from_base(base_pieces);
    for (idx, run) in cache.runs.iter_mut().enumerate() {
        run.payload_bytes = details.client_payload_bytes.get(idx).copied().unwrap_or(0);
        if !run.is_empty() {
            cache.non_empty_clients += 1;
            let count = u64::from(run.block_count);
            cache.total_changes += count;
            cache.max_client_changes = cache.max_client_changes.max(count);
        }
    }
    Some(())
}

fn apply_initial_deletes(cache: &mut ReadingCache, delete_set: IdSet) -> Option<()> {
    for (client_id, ranges) in delete_set.iter() {
        let &client_idx = cache.client_map.get(&client_id.get())?;
        let run = &cache.runs[index(client_idx)];
        for range in ranges.iter() {
            let mut clock_at = range.start;
            while clock_at < range.end {
                let (_, p_idx) = run.locate_clock(&cache.pieces, clock_at)?;
                let p_begin = cache.pieces.get(index(p_idx)).begin;
                let p_end = cache.piece_end(p_idx);
                if p_begin < range.start || p_end > range.end {
                    return None;
                }
                cache.pieces.get_mut(index(p_idx)).set_deleted();
                clock_at = p_end;
            }
        }
    }
    cache.delete_set = delete_set;
    Some(())
}

fn translate_parent(branch: u32, piece_count: u32) -> u32 {
    if branch == NONE {
        NONE
    } else if branch >= piece_count {
        ROOT_BIT | (branch - piece_count)
    } else {
        branch
    }
}

fn cross_client_dep_from_replay(piece: &Piece) -> Option<DepPoint> {
    if piece.holds == Holds::Collected {
        return None;
    }
    let parent = match piece.named {
        Named::Item(id) => id,
        Named::Neighbours | Named::Root(_) => NO_ID,
    };
    let mut deps = [(NONE, 0_u32); 3];
    let mut any = false;
    for (slot, id) in [piece.origin, piece.right_origin, parent]
        .into_iter()
        .enumerate()
    {
        if id != NO_ID {
            let (dep_client, dep_clock) = unpack(id);
            if dep_client != piece.client {
                deps[slot] = (dep_client, dep_clock);
                any = true;
            }
        }
    }
    any.then_some(DepPoint {
        begin: piece.begin,
        end: piece.end,
        deps,
    })
}
