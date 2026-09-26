//! Deal with several changes in events, batching them together.
//!
//! Preferably, changes should be kept to a minimum.
//! Sometimes, it’s needed to change the list of events, because parsing can be
//! messy, and it helps to expose a cleaner interface of events to the compiler
//! and other users.
//! It can also help to merge many adjacent similar events.
//! And, in other cases, it’s needed to parse subcontent: pass some events
//! through another tokenizer and inject the result.

use crate::event::Event;
use alloc::{collections::BTreeMap, vec, vec::Vec};

/// Shift `previous` and `next` links according to `jumps`.
///
/// This fixes links in case there are events removed or added between them.
fn shift_links(events: &mut [Event], jumps: &[(usize, usize, usize)]) {
    let mut jump_index = 0;
    let mut index = 0;
    let mut add = 0;
    let mut rm = 0;

    while index < events.len() {
        let rm_curr = rm;

        while jump_index < jumps.len() && jumps[jump_index].0 <= index {
            add = jumps[jump_index].2;
            rm = jumps[jump_index].1;
            jump_index += 1;
        }

        // Ignore items that will be removed.
        if rm > rm_curr {
            index += rm - rm_curr;
        } else {
            if let Some(link) = &events[index].link {
                if let Some(next) = link.next {
                    events[next].link.as_mut().unwrap().previous = Some(index + add - rm);

                    while jump_index < jumps.len() && jumps[jump_index].0 <= next {
                        add = jumps[jump_index].2;
                        rm = jumps[jump_index].1;
                        jump_index += 1;
                    }

                    events[index].link.as_mut().unwrap().next = Some(next + add - rm);
                    index = next;
                    continue;
                }
            }

            index += 1;
        }
    }
}

/// Tracks a bunch of edits.
#[derive(Debug)]
pub struct EditMap {
    /// Record of changes.
    map: Vec<(usize, usize, Vec<Event>)>,
    /// Position in `map` of the entry for each `at` (keeps `add` O(log n)).
    index: BTreeMap<usize, usize>,
}

impl EditMap {
    /// Create a new edit map.
    pub fn new() -> EditMap {
        EditMap {
            map: vec![],
            index: BTreeMap::new(),
        }
    }
    /// Create an edit: a remove and/or add at a certain place.
    pub fn add(&mut self, index: usize, remove: usize, add: Vec<Event>) {
        add_impl(self, index, remove, add, false);
    }
    /// Create an edit: but insert `add` before existing additions.
    pub fn add_before(&mut self, index: usize, remove: usize, add: Vec<Event>) {
        add_impl(self, index, remove, add, true);
    }
    /// Done, change the events.
    pub fn consume(&mut self, events: &mut Vec<Event>) {
        self.map
            .sort_unstable_by(|a, b| a.0.partial_cmp(&b.0).unwrap());

        if self.map.is_empty() {
            return;
        }

        // Calculate jumps: where items in the current list move to.
        let mut jumps = Vec::with_capacity(self.map.len());
        let mut index = 0;
        let mut add_acc = 0;
        let mut remove_acc = 0;
        while index < self.map.len() {
            let (at, remove, add) = &self.map[index];
            remove_acc += remove;
            add_acc += add.len();
            jumps.push((*at, remove_acc, add_acc));
            index += 1;
        }

        shift_links(events, &jumps);

        let len_before = events.len();
        let mut index = self.map.len();
        let mut vecs = Vec::with_capacity(index * 2 + 1);
        while index > 0 {
            index -= 1;
            vecs.push(events.split_off(self.map[index].0 + self.map[index].1));
            vecs.push(self.map[index].2.split_off(0));
            events.truncate(self.map[index].0);
        }
        vecs.push(events.split_off(0));

        events.reserve(len_before + add_acc - remove_acc);

        while let Some(mut slice) = vecs.pop() {
            events.append(&mut slice);
        }

        self.map.truncate(0);
        self.index.clear();
    }
}

/// Create an edit.
fn add_impl(edit_map: &mut EditMap, at: usize, remove: usize, mut add: Vec<Event>, before: bool) {
    if remove == 0 && add.is_empty() {
        return;
    }

    if let Some(&index) = edit_map.index.get(&at) {
        edit_map.map[index].1 += remove;

        if before {
            add.append(&mut edit_map.map[index].2);
            edit_map.map[index].2 = add;
        } else {
            edit_map.map[index].2.append(&mut add);
        }

        return;
    }

    edit_map.index.insert(at, edit_map.map.len());
    edit_map.map.push((at, remove, add));
}

// FVOCI patch (see `PATCHES.md`): the indexed `add_impl` must behave exactly
// like the upstream linear scan it replaces.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Content, Kind, Link, Name, Point};
    use alloc::{format, string::String};

    /// Upstream 1.0.0 `add_impl`, kept verbatim as the reference.
    fn upstream_add(map: &mut Vec<(usize, usize, Vec<Event>)>, at: usize, remove: usize, mut add: Vec<Event>, before: bool) {
        let mut index = 0;
        if remove == 0 && add.is_empty() {
            return;
        }
        while index < map.len() {
            if map[index].0 == at {
                map[index].1 += remove;
                if before {
                    add.append(&mut map[index].2);
                    map[index].2 = add;
                } else {
                    map[index].2.append(&mut add);
                }
                return;
            }
            index += 1;
        }
        map.push((at, remove, add));
    }

    fn event(tag: usize, link: Option<Link>) -> Event {
        Event {
            kind: if tag % 2 == 0 { Kind::Enter } else { Kind::Exit },
            name: Name::Data,
            point: Point {
                line: 1,
                column: 1,
                index: tag,
                vs: 0,
            },
            link,
        }
    }

    /// Events with a chain of links (0 -> 4 -> 8 -> ...) so `shift_links` runs.
    fn events(len: usize) -> Vec<Event> {
        (0..len)
            .map(|i| {
                let link = (i % 4 == 0).then(|| Link {
                    previous: i.checked_sub(4),
                    next: (i + 4 < len).then_some(i + 4),
                    content: Content::Text,
                });
                event(i, link)
            })
            .collect()
    }

    fn dump(events: &[Event]) -> String {
        format!("{events:?}")
    }

    type Edit = (usize, usize, usize, bool);

    fn check(len: usize, edits: &[Edit]) {
        let adds = |n: usize, at: usize| (0..n).map(|k| event(1000 + at * 10 + k, None)).collect::<Vec<_>>();
        let mut patched_events = events(len);
        let mut patched = EditMap::new();
        let mut reference_events = events(len);
        let mut reference = EditMap::new();
        for &(at, remove, n, before) in edits {
            if before {
                patched.add_before(at, remove, adds(n, at));
            } else {
                patched.add(at, remove, adds(n, at));
            }
            upstream_add(&mut reference.map, at, remove, adds(n, at), before);
        }
        patched.consume(&mut patched_events);
        reference.consume(&mut reference_events);
        assert_eq!(dump(&patched_events), dump(&reference_events), "{edits:?}");
        assert!(patched.map.is_empty() && patched.index.is_empty());
    }

    #[test]
    fn matches_upstream_linear_scan() {
        // Out-of-order positions, repeated positions (before and after), removals,
        // empty no-op edits, and a removal-only edit.
        check(24, &[(8, 0, 2, false), (4, 1, 1, false), (8, 0, 1, true), (8, 2, 0, false), (0, 0, 0, false), (20, 0, 3, true), (4, 0, 2, true), (12, 1, 0, false)]);
        check(8, &[(0, 0, 1, false), (0, 0, 1, false), (0, 0, 1, true), (7, 1, 0, false)]);
        check(16, &[(12, 0, 1, false), (8, 0, 1, false), (4, 0, 1, false), (0, 0, 1, false)]);
        let many: Vec<Edit> = (0..200).map(|i| ((i * 37) % 100, usize::from(i % 7 == 0), i % 3, i % 2 == 0)).collect();
        check(120, &many);
    }

    #[test]
    fn consume_clears_the_index() {
        let mut map = EditMap::new();
        let mut first = events(8);
        map.add(4, 0, vec![event(900, None)]);
        map.consume(&mut first);
        // A second round at the same position must not reuse the old entry.
        let mut second = events(8);
        map.add(4, 0, vec![event(901, None)]);
        map.consume(&mut second);
        assert_eq!(second.len(), 9);
        assert_eq!(second[4].point.index, 901);
    }
}
