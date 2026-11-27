//! Compressed `BlockPicker` for [`super::ReadingCache`].
//!
//! Computes the exact `looked` count of [`crate::reading::Replay::run`] (including exact
//! early-stop past `stop_past` and `Refusal::Gap` cycle detection) in `O(cross-client deps)`
//! time instead of `O(total blocks)`.

use super::{DepPoint, ReadingCache};
use crate::reading::{LOOKED_PER_STEP, NONE, index, small};
use crate::scan::Refusal;

#[derive(Debug, Clone, Copy)]
struct PickerState {
    /// Next block index to consume in this client's run (`0..block_count`).
    cur_idx: u32,
    /// Cursor into `run.dep_points` (index of first `DepPoint` with `begin >= cur_begin`).
    dp_cursor: usize,
    /// Clock after the last taken block of this client.
    clock: u32,
    started: bool,
    stacked: bool,
}

#[derive(Debug, Clone, Copy)]
struct StackEntry {
    client: u32,
    end_clock: u32,
    deps: [(u32, u32); 3],
}

impl ReadingCache {
    /// Computes the exact `looked` count that `Replay::run(StepsMode::Count(stop_past))`
    /// would produce on this state, returning [`Refusal::Gap`] on any dependency cycle or
    /// missing clock.
    pub(super) fn compute_looked(&self, stop_past: u64) -> Result<u64, Refusal> {
        let mut order: Vec<u32> = (0..small(self.runs.len()))
            .filter(|&c| !self.runs[index(c)].is_empty())
            .collect();
        order.sort_unstable_by_key(|&c| self.runs[index(c)].client_id);

        let mut states: Vec<PickerState> = (0..self.runs.len())
            .map(|_| PickerState {
                cur_idx: 0,
                dp_cursor: 0,
                clock: 0,
                started: false,
                stacked: false,
            })
            .collect();
        let mut stack: Vec<StackEntry> = Vec::new();
        let mut latest = NONE;
        let mut looked = 0_u64;
        let max_looked = stop_past.saturating_mul(LOOKED_PER_STEP);

        loop {
            let mut block = match stack.pop() {
                Some(entry) => {
                    states[index(entry.client)].stacked = false;
                    entry
                }
                None => loop {
                    if latest != NONE
                        && let Some(next_entry) = self.pop_next_or_skip_clean(latest, &mut states)
                    {
                        break next_entry;
                    }
                    let Some(client) = order.pop() else {
                        return Ok(looked);
                    };
                    states[index(client)].started = true;
                    latest = client;
                },
            };

            while let Some((missing_client, dep_clock)) = first_missing(&block, &states) {
                let target_state = states[index(missing_client)];
                if target_state.started || target_state.stacked {
                    return Err(Refusal::Gap);
                }
                let missing_run = &self.runs[index(missing_client)];
                let Some((target_idx, _)) = missing_run.locate_clock(&self.pieces, dep_clock)
                else {
                    return Err(Refusal::Gap);
                };
                if target_idx < target_state.cur_idx {
                    return Err(Refusal::Gap);
                }
                let next_dp_idx = self.next_dep_block_index(missing_client, target_state.dp_cursor);
                let clean_end = target_idx.saturating_add(1).min(next_dp_idx);
                if target_state.cur_idx < clean_end {
                    let count = u64::from(clean_end - target_state.cur_idx);
                    let depth = u64::try_from(stack.len() + 1).unwrap_or(u64::MAX);
                    let added = count.saturating_mul(depth);
                    if looked.saturating_add(added) > max_looked {
                        let remaining = max_looked.saturating_sub(looked);
                        let stop_after = remaining / depth + 1;
                        return Ok(looked.saturating_add(stop_after.saturating_mul(depth)));
                    }
                    looked = looked.saturating_add(added);
                    let last_taken_piece = missing_run
                        .piece_at_block_index(clean_end - 1)
                        .ok_or(Refusal::Gap)?;
                    let st = &mut states[index(missing_client)];
                    st.cur_idx = clean_end;
                    st.clock = self.piece_end(last_taken_piece);
                    // Re-evaluate `first_missing(&block)` on the next iteration of `while`.
                    continue;
                }
                // Next block of `missing_client` is a `DepPoint`: execute one exact step.
                stack.push(block);
                states[index(block.client)].stacked = true;
                looked = looked.saturating_add(u64::try_from(stack.len()).unwrap_or(u64::MAX));
                block = self
                    .pop_one_block(missing_client, &mut states)
                    .ok_or(Refusal::Gap)?;
            }

            // Take `block`.
            states[index(block.client)].clock = block.end_clock;
            if looked > max_looked {
                return Ok(looked);
            }
        }
    }

    /// When the stack is empty, fast-forwards all clean blocks of `client` with 0 `looked`
    /// until the next `DepPoint`, returning that `DepPoint` as a [`StackEntry`], or `None`
    /// once `client` finishes.
    fn pop_next_or_skip_clean(
        &self,
        client: u32,
        states: &mut [PickerState],
    ) -> Option<StackEntry> {
        let run = &self.runs[index(client)];
        let st = &mut states[index(client)];
        if st.cur_idx >= run.block_count {
            return None;
        }
        if let Some(dp) = run.dep_points.get(st.dp_cursor).copied() {
            let (dp_idx, _) = run.locate_clock(&self.pieces, dp.begin)?;
            if st.cur_idx < dp_idx {
                let before_piece = run.piece_at_block_index(dp_idx - 1)?;
                st.cur_idx = dp_idx;
                st.clock = self.piece_end(before_piece);
            }
            st.cur_idx = dp_idx + 1;
            st.dp_cursor += 1;
            Some(StackEntry {
                client,
                end_clock: dp.end,
                deps: dp.deps,
            })
        } else {
            st.cur_idx = run.block_count;
            st.clock = run.end_clock;
            None
        }
    }

    /// Pops one exact block of `client` (used when `cur_idx` sits at a `DepPoint`).
    fn pop_one_block(&self, client: u32, states: &mut [PickerState]) -> Option<StackEntry> {
        let run = &self.runs[index(client)];
        let st = &mut states[index(client)];
        if st.cur_idx >= run.block_count {
            return None;
        }
        let block_idx = st.cur_idx;
        let p_idx = run.piece_at_block_index(block_idx)?;
        let p_begin = self.pieces.get(index(p_idx)).begin;
        let p_end = self.piece_end(p_idx);
        st.cur_idx += 1;
        let deps = match run.dep_points.get(st.dp_cursor) {
            Some(DepPoint { begin, deps, .. }) if *begin == p_begin => {
                st.dp_cursor += 1;
                *deps
            }
            _ => [(NONE, 0); 3],
        };
        Some(StackEntry {
            client,
            end_clock: p_end,
            deps,
        })
    }

    fn next_dep_block_index(&self, client: u32, dp_cursor: usize) -> u32 {
        let run = &self.runs[index(client)];
        let Some(dp) = run.dep_points.get(dp_cursor) else {
            return run.block_count;
        };
        run.locate_clock(&self.pieces, dp.begin)
            .map_or(run.block_count, |(idx, _)| idx)
    }
}

fn first_missing(block: &StackEntry, states: &[PickerState]) -> Option<(u32, u32)> {
    block
        .deps
        .into_iter()
        .filter(|&(client, _)| client != NONE)
        .find(|&(client, dep_clock)| dep_clock >= states[index(client)].clock)
}
