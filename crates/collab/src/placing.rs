//! How far the Y library may have to walk through a document to place the changes of
//! an update, counted before it is asked to.
//!
//! The library (yrs 0.28.1) places a change by the neighbours it names. Where the
//! change to its left is not followed by the change to its right, it walks on from
//! the left one: past every change that names the same left neighbour, and past every
//! change that names one it has walked past, until it meets the right one or a change
//! that is neither. Every change it walks past stands in the same list as the change
//! it places (the children of one node, the letters of one text, the values one
//! attribute had), so the walk is no longer than that list, with what was deleted
//! from it, which a document keeps. An editor's change is placed in a step or two; an
//! update made for it has each of its changes walk a whole list (5,400 changes of 59
//! kB walked the 50,000 children of one node for twelve seconds).
//!
//! So an update is charged, for each of its changes, the most the library may walk to
//! place it, and is refused where that is more than [`UpdateLimits::placing_steps`]
//! together:
//!
//! - A change that alone names a left neighbour of the same update, at the end of the
//!   change it names, is placed in one step: nothing else stands after that
//!   neighbour that the walk goes past. What is typed or pasted is one change placed
//!   in the document and the rest after it, each so.
//! - Every other change is charged the list it goes into: what the document holds of
//!   it before the update, the changes of the update that go into it, and the pieces
//!   the neighbours they name may cut its changes into.
//! - Or, if it is less, what stands below the left neighbour it names: the walk passes
//!   only changes that name that neighbour to their left, or one it passed already
//!   (yrs `Item::resolve_conflict`), so no more than the changes that name it, the
//!   changes that name those, and so on, and the pieces they are cut into. A change
//!   inserted where the document was made from a body, whose children name no left
//!   neighbour, is charged a few steps so. Nor what stands below the neighbour it
//!   names to its right, where that names the same place to its left and begins where
//!   it is named: the walk stops at it, and all of that comes after it. What is
//!   inserted between two paragraphs typed one after the other is charged a step so.
//!
//! A change that begins before where the document has its client's changes is cut
//! there by the library, and what is left is placed after the last of those it has,
//! whatever neighbour the change names (yrs `Item::integrate` with an offset): it is
//! charged so.
//!
//! Where a neighbour a change names, or an end of what an update deletes, falls inside
//! a change and not at its edge, the library cuts that change in two (yrs
//! `ItemContent::splice`): it counts letters from the start of the change to the cut,
//! a nanosecond each, and copies both halves. Each such cut is charged the length of
//! the change it falls in, a step for every [`PLACES_PER_STEP`] letters, and a run of
//! values [`VALUE_PLACES`] times as much: fourteen thousand deletions of a letter each,
//! from the end of a text of four million back to its start, took 25 seconds before
//! they were charged (deletions that go back are refused for their order now).
//!
//! The lists are read from the document's state, which names for each change its
//! neighbours and, where it names none, its parent: a change belongs to the list of
//! the neighbour it names. Reading them is a walk of the whole state, so it is left out
//! where every change that is charged a list, charged the whole document, and every
//! cut, charged as a run of values as long as the longest change of the document, are
//! within the limit already: an editor's update, which has a handful, unless the
//! document holds a change of more than about 800,000 letters.
//!
//! [`UpdateLimits::placing_steps`]: crate::UpdateLimits::placing_steps

use std::collections::{HashMap, HashSet};

use yrs::{ClientID, ID, IdSet, StateVector};

use crate::scan::{self, Change, Content, Id, Parent, Refusal};

/// How many letters the library goes through cutting a change in the time of a step
/// of its walk. It counts them at 0.8 ns each, 1.4 ns for letters of three bytes, and
/// copies them at a twentieth of that; a step, walked again where the document is read
/// again, takes up to 600 ns.
pub(crate) const PLACES_PER_STEP: u64 = 256;

/// What a place of a run of values is charged as letters, cutting it: the library
/// copies every value of the run, at 30 ns each, and keeps 47 bytes of it for each cut
/// until the update is taken (yrs 0.28.1). So many cuts of a run of 20,000 values took
/// 190 MB.
pub(crate) const VALUE_PLACES: u64 = 2 * PLACES_PER_STEP;

/// A document as its state, with what the library read of it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Held<'bytes> {
    /// The state, which the walk has taken ([`scan::check`]).
    pub(crate) state: &'bytes [u8],
    /// How many changes the walk found in it ([`scan::Walked::changes`]).
    pub(crate) changes: u64,
    /// How far the library read each client's changes in it.
    pub(crate) ends: &'bytes StateVector,
}

/// An update, with what the library read of it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Sent<'bytes> {
    /// The update, which the walk has taken.
    pub(crate) update: &'bytes [u8],
    /// The places the library read its changes to cover.
    pub(crate) covered: &'bytes IdSet,
    /// The places it deletes.
    pub(crate) deleted: &'bytes IdSet,
}

/// What the changes of an update are charged for a document: no less than the changes
/// the library walks past to place them. An update charged more than `limit` is
/// charged no more exactly than that it is past it.
///
/// # Errors
///
/// Returns [`Refusal::Lengths`] if the update, or the state, is not read here as long
/// as the library read it: the neighbours a change names would be found in another
/// place here than there.
pub(crate) fn charge(held: &Held<'_>, sent: &Sent<'_>, limit: u64) -> Result<u64, Refusal> {
    charge_with_state(held.changes, held.ends, None, || held.state, sent, limit)
}

/// As [`charge`], using `cut_inside_cached` (when `Some`) to compute cut costs in
/// `O(|cuts| log N)` without encoding or scanning `state`, and computing the document's
/// encoded state lazily via `state_bytes` only if `Lists::of` or uncached cut lookup is
/// needed.
pub(crate) fn charge_lazy<'a>(
    changes: u64,
    ends: &StateVector,
    cut_inside_cached: Option<&dyn Fn(Id) -> Option<u64>>,
    state_bytes: impl FnOnce() -> &'a [u8],
    sent: &Sent<'_>,
    limit: u64,
) -> Result<u64, Refusal> {
    charge_with_state(changes, ends, cut_inside_cached, state_bytes, sent, limit)
}

fn charge_with_state<'a>(
    changes: u64,
    ends: &StateVector,
    cut_inside_cached: Option<&dyn Fn(Id) -> Option<u64>>,
    state_bytes: impl FnOnce() -> &'a [u8],
    sent: &Sent<'_>,
    limit: u64,
) -> Result<u64, Refusal> {
    let update = Placed::read(sent, ends)?;
    let cuts = cuts(&update, sent.deleted);
    // No change of the document is longer than the most it has of a client.
    let longest = ends
        .iter()
        .map(|(_, clock)| u64::from(*clock))
        .chain(update.clocks.ends.iter().copied())
        .max()
        .unwrap_or(0);
    let walks = update.at_most(changes);
    let at_most = walks.saturating_add(steps_of(
        count(cuts.len())
            .saturating_mul(longest)
            .saturating_mul(VALUE_PLACES),
    ));
    if at_most <= limit {
        return Ok(at_most);
    }
    if let Some(cut_fn) = cut_inside_cached {
        let cutting = steps_of(
            cuts.iter()
                .map(|cut| {
                    let known = u64::from(ends.get(&ClientID::new(cut.0)));
                    if cut.1 < known {
                        cut_fn(*cut)
                    } else {
                        update.new_item_of(*cut).and(update.clocks.cut_inside(*cut))
                    }
                    .unwrap_or(0)
                })
                .fold(0, u64::saturating_add),
        );
        if walks.saturating_add(cutting) <= limit {
            return Ok(walks.saturating_add(cutting));
        }
    }
    let state = state_bytes();
    let clocks = Clocks::read(
        state,
        |_, _| {},
        |run| u64::from(ends.get(&ClientID::new(run.client))) == run.end,
    )?;
    let cutting = steps_of(
        cuts.iter()
            .map(|cut| update.cut_inside(&clocks, *cut))
            .fold(0, u64::saturating_add),
    );
    if walks.saturating_add(cutting) <= limit {
        return Ok(walks.saturating_add(cutting));
    }
    Ok(Lists::of(state, &clocks, &update)?
        .charge()
        .saturating_add(cutting))
}

fn count(number: usize) -> u64 {
    u64::try_from(number).unwrap_or(u64::MAX)
}

/// The steps the cutting of so many letters is charged ([`PLACES_PER_STEP`]).
fn steps_of(places: u64) -> u64 {
    places.div_ceil(PLACES_PER_STEP)
}

/// What a cut inside a change is charged a place of it, by what the change holds:
/// letters each one, values [`VALUE_PLACES`]. Deleted content and what was collected
/// are cut by their length alone, and what is one place long is never cut inside.
fn cut_charge(content: &Content<'_>) -> u64 {
    match content {
        Content::Letters(_) => 1,
        Content::Values(_) => VALUE_PLACES,
        Content::Collected(_) | Content::Deleted(_) | Content::Type | Content::One => 0,
    }
}

/// Where the library may cut a change in two for an update, as the place a cut goes
/// before: after the neighbour each change that comes names to its left, before the
/// one it names to its right, and at both ends of every stretch the update deletes.
fn cuts(update: &Placed<'_>, deleted: &IdSet) -> HashSet<Id> {
    let mut cuts = HashSet::new();
    for (change, standing) in update.changes.iter().zip(&update.standings) {
        if *standing != Standing::Left {
            cuts.extend(
                change
                    .left
                    .map(|(client, clock)| (client, clock.saturating_add(1))),
            );
            cuts.extend(change.right);
        }
    }
    for (client, ranges) in deleted.iter() {
        for range in ranges.iter() {
            let client = client.get();
            cuts.insert((client, u64::from(range.start)));
            cuts.insert((client, u64::from(range.end)));
        }
    }
    cuts
}

/// The changes of one client in a state or an update: the clocks they cover, and
/// which they are among all the changes.
#[derive(Debug, Clone, Copy)]
struct Run {
    client: u64,
    begin: u64,
    end: u64,
    /// The numbers of its first change and of the change after its last.
    first: usize,
    after: usize,
}

/// Where the changes of a state or an update begin, in the order they are written: a
/// change is found by an identifier of any place it covers.
#[derive(Debug)]
struct Clocks {
    /// By client. A state or an update has one run a client.
    runs: Vec<Run>,
    begins: Vec<u64>,
    /// The clock after the last place of each.
    ends: Vec<u64>,
    /// Whether each is an item: not a stretch that was collected.
    items: Vec<bool>,
    /// What a cut inside each is charged a place of it ([`cut_charge`]).
    cut_charges: Vec<u64>,
}

impl Clocks {
    /// Reads where the changes of a state or an update begin, and hands `each` every
    /// one. `as_the_library` says of each client's run whether the library read it as
    /// far.
    fn read<'bytes>(
        bytes: &'bytes [u8],
        mut each: impl FnMut(Id, &Change<'bytes>),
        as_the_library: impl Fn(&Run) -> bool,
    ) -> Result<Self, Refusal> {
        let mut clocks = Self {
            runs: Vec::new(),
            begins: Vec::new(),
            ends: Vec::new(),
            items: Vec::new(),
            cut_charges: Vec::new(),
        };
        scan::walk(bytes, |(client, clock), change| {
            let end = clock.saturating_add(change.content.length());
            let number = clocks.begins.len();
            match clocks.runs.last_mut() {
                Some(run) if run.client == client => {
                    run.end = end;
                    run.after = number + 1;
                }
                _ => clocks.runs.push(Run {
                    client,
                    begin: clock,
                    end,
                    first: number,
                    after: number + 1,
                }),
            }
            clocks.begins.push(clock);
            clocks.ends.push(end);
            clocks.items.push(is_item(change));
            clocks.cut_charges.push(cut_charge(&change.content));
            each((client, clock), change);
        })?;
        // A length counted otherwise than the library counts it would find every later
        // change of that client under the wrong identifiers.
        if !clocks.runs.iter().all(as_the_library) {
            return Err(Refusal::Lengths);
        }
        clocks.runs.sort_unstable_by_key(|run| run.client);
        Ok(clocks)
    }

    /// The change that covers the place of an identifier, by its number among these.
    fn block_of(&self, (client, clock): Id) -> Option<usize> {
        let run = self
            .runs
            .binary_search_by_key(&client, |run| run.client)
            .ok()
            .map(|at| self.runs[at])?;
        if !(run.begin..run.end).contains(&clock) {
            return None;
        }
        // The last change of the run that begins at or before the clock; changes of no
        // length before it begin there as well and cover nothing.
        let within = self.begins[run.first..run.after].partition_point(|begin| *begin <= clock);
        within.checked_sub(1).map(|at| run.first + at)
    }

    /// What a cut before the place of an identifier is charged, where it falls inside
    /// one of these changes and not at its start: the length of the change, by what it
    /// holds.
    fn cut_inside(&self, id: Id) -> Option<u64> {
        let at = self.block_of(id)?;
        let begin = self.begins[at];
        (begin < id.1).then(|| {
            self.ends[at]
                .saturating_sub(begin)
                .saturating_mul(self.cut_charges[at])
        })
    }
}

fn is_item(change: &Change<'_>) -> bool {
    !matches!(change.content, Content::Collected(_))
}

/// How the library places a change of an update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Standing {
    /// Not at all: a stretch that was collected, or what the document has already.
    Left,
    /// In one step, after a change of the same update that nothing else follows.
    Follows,
    /// By a walk through its list.
    Walks,
}

/// The changes of an update, with how the library places each.
#[derive(Debug)]
struct Placed<'bytes> {
    clocks: Clocks,
    changes: Vec<Change<'bytes>>,
    /// The client of each change.
    clients: Vec<u64>,
    standings: Vec<Standing>,
    /// How far the document has each client's changes: what an update says again
    /// from before there, the library leaves out.
    known: &'bytes StateVector,
}

impl<'bytes> Placed<'bytes> {
    fn read(sent: &Sent<'bytes>, known: &'bytes StateVector) -> Result<Self, Refusal> {
        let covered = |client: u64, clock: u64| {
            u32::try_from(clock).is_ok_and(|clock| {
                sent.covered
                    .contains(&ID::new(ClientID::new(client), clock))
            })
        };
        let mut changes = Vec::new();
        let mut clients = Vec::new();
        let clocks = Clocks::read(
            sent.update,
            |(client, _), change| {
                changes.push(*change);
                clients.push(client);
            },
            |run| {
                !covered(run.client, run.end)
                    && (run.end == run.begin || covered(run.client, run.end - 1))
            },
        )?;
        let mut placed = Self {
            clocks,
            changes,
            clients,
            standings: Vec::new(),
            known,
        };
        placed.trim();
        placed.standings = placed.standings();
        Ok(placed)
    }

    /// Names, for each change that begins before where the document has its client's
    /// changes and ends past it, the neighbour the library places what is left of it
    /// after: the last of those changes the document has.
    fn trim(&mut self) {
        for at in 0..self.changes.len() {
            let client = self.clients[at];
            let known = self.known(client);
            if self.clocks.items[at] && self.clocks.begins[at] < known && known < self.end_of(at) {
                self.changes[at].left = Some((client, known - 1));
            }
        }
    }

    fn known(&self, client: u64) -> u64 {
        u64::from(self.known.get(&ClientID::new(client)))
    }

    /// The change of this update that covers a place the document has not, by its
    /// number.
    fn new_item_of(&self, id: Id) -> Option<usize> {
        let at = self.clocks.block_of(id)?;
        (self.clocks.items[at] && id.1 >= self.known(id.0)).then_some(at)
    }

    /// The clock after the last place of a change.
    fn end_of(&self, at: usize) -> u64 {
        self.clocks.begins[at].saturating_add(self.changes[at].content.length())
    }

    /// What a cut before the place of an identifier is charged ([`Clocks::cut_inside`]):
    /// in a change of the document, or else in one of this update that comes.
    fn cut_inside(&self, held: &Clocks, cut: Id) -> u64 {
        match held.block_of(cut) {
            Some(_) => held.cut_inside(cut),
            None => self.new_item_of(cut).and(self.clocks.cut_inside(cut)),
        }
        .unwrap_or(0)
    }

    fn standings(&self) -> Vec<Standing> {
        let comes = |at: usize| {
            let known = self.known(self.clients[at]);
            self.clocks.items[at] && !(self.clocks.begins[at] < known && self.end_of(at) <= known)
        };
        // How many changes that come name each place of the update to their left.
        let mut followers: HashMap<Id, u32> = HashMap::new();
        for (at, change) in self.changes.iter().enumerate() {
            if let Some(left) = change.left.filter(|_| comes(at)) {
                *followers.entry(left).or_insert(0) += 1;
            }
        }
        (0..self.changes.len())
            .map(|at| {
                if !comes(at) {
                    return Standing::Left;
                }
                let alone_after_a_whole_change = self.changes[at].left.is_some_and(|left| {
                    followers.get(&left) == Some(&1)
                        && self
                            .new_item_of(left)
                            .is_some_and(|named| self.end_of(named) == left.1 + 1)
                });
                if alone_after_a_whole_change {
                    Standing::Follows
                } else {
                    Standing::Walks
                }
            })
            .collect()
    }

    fn standing(&self, standing: Standing) -> u64 {
        count(
            self.standings
                .iter()
                .filter(|other| **other == standing)
                .count(),
        )
    }

    /// What the update is charged at most, whatever lists its changes go into: each
    /// that walks, the whole document and three times the update (its changes, and
    /// the two pieces more each may cut a change into).
    fn at_most(&self, held_changes: u64) -> u64 {
        let walks = self.standing(Standing::Walks);
        let follows = self.standing(Standing::Follows);
        let coming = walks.saturating_add(follows);
        let list = held_changes.saturating_add(coming.saturating_mul(3));
        walks.saturating_mul(list).saturating_add(follows)
    }
}

/// Where a change belongs, as far as is known of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    /// In the list of another change, by its number: the neighbour it names.
    With(usize),
    /// In a list, by its number.
    In(usize),
    /// In none: it was collected, or names a neighbour that is no item.
    Nowhere,
}

/// How much of a list there is.
#[derive(Debug, Clone, Copy, Default)]
struct Size {
    /// The changes of the document in it.
    held: u64,
    /// The changes of the update that go into it.
    coming: u64,
    /// The neighbours in it that changes of the update name: each may cut a change
    /// of it in two.
    cuts: u64,
}

/// The lists of a document and of an update for it: the changes of the document come
/// first, by their numbers, and those of the update after them.
#[derive(Debug)]
struct Lists<'placed, 'bytes> {
    held: &'placed Clocks,
    sent: &'placed Placed<'bytes>,
    places: Vec<Place>,
    sizes: Vec<Size>,
    /// For each change, the item that covers the left neighbour it names: what the
    /// library places it after.
    lefts: Vec<Option<usize>>,
    /// For each change, the place it names to its left.
    left_places: Vec<Option<Id>>,
    /// For each change, the weight of what stands below it ([`weights`]).
    weights: Vec<Weight>,
}

/// The lists by what their first changes name: a parent, and a member of it for the
/// values of an attribute.
type Named<'bytes> = HashMap<(Parent<'bytes>, Option<&'bytes [u8]>), usize>;

impl<'placed, 'bytes> Lists<'placed, 'bytes> {
    fn of(
        state: &'bytes [u8],
        held: &'placed Clocks,
        sent: &'placed Placed<'bytes>,
    ) -> Result<Self, Refusal> {
        let all = held.begins.len() + sent.changes.len();
        let mut lists = Self {
            held,
            sent,
            places: Vec::with_capacity(all),
            sizes: Vec::new(),
            lefts: Vec::with_capacity(all),
            left_places: Vec::with_capacity(all),
            weights: Vec::new(),
        };
        let mut named = Named::new();
        scan::walk(state, |_, change| {
            let place = lists.named_place(change, &mut named);
            let left = lists.left_of(change);
            lists.places.push(place);
            lists.lefts.push(left);
            lists.left_places.push(change.left);
        })?;
        for (change, standing) in sent.changes.iter().zip(&sent.standings) {
            let (place, left) = match standing {
                Standing::Left => (Place::Nowhere, None),
                Standing::Follows | Standing::Walks => {
                    (lists.named_place(change, &mut named), lists.left_of(change))
                }
            };
            lists.places.push(place);
            lists.lefts.push(left);
            lists.left_places.push(change.left);
        }
        lists.sizes = vec![Size::default(); named.len()];
        let cuts = lists.count();
        lists.weights = weights(&lists.lefts, &cuts);
        Ok(lists)
    }

    /// Counts what each list holds and has coming, and how often the changes of the
    /// update name each change as a neighbour: the cuts of each.
    fn count(&mut self) -> Vec<u64> {
        let held = self.held.begins.len();
        for number in 0..held {
            if let Some(list) = self.list_of(number) {
                self.sizes[list].held += 1;
            }
        }
        let mut cuts = vec![0_u64; self.places.len()];
        let sent = self.sent;
        for (at, change) in sent.changes.iter().enumerate() {
            if sent.standings[at] == Standing::Left {
                continue;
            }
            if let Some(list) = self.list_of(held + at) {
                self.sizes[list].coming += 1;
            }
            for neighbour in [change.left, change.right].into_iter().flatten() {
                let Some(number) = self.item_of(neighbour) else {
                    continue;
                };
                cuts[number] += 1;
                if let Some(list) = self.list_of(number) {
                    self.sizes[list].cuts += 1;
                }
            }
        }
        cuts
    }

    /// The number of the item that covers the place of an identifier, in the document
    /// or among the changes of the update that come.
    fn item_of(&self, id: Id) -> Option<usize> {
        match self.held.block_of(id) {
            Some(at) => self.held.items[at].then_some(at),
            None => Some(self.held.begins.len() + self.sent.new_item_of(id)?),
        }
    }

    /// The item a change names to its left, if it is an item.
    fn left_of(&self, change: &Change<'bytes>) -> Option<usize> {
        if !is_item(change) {
            return None;
        }
        self.item_of(change.left?)
    }

    /// Where a change says it belongs: with the neighbour it names, the left one
    /// before the right one as the library takes its parent from them, or in the list
    /// of the parent it names.
    fn named_place(&self, change: &Change<'bytes>, named: &mut Named<'bytes>) -> Place {
        if !is_item(change) {
            return Place::Nowhere;
        }
        let neighbour = [change.left, change.right]
            .into_iter()
            .flatten()
            .find_map(|id| self.item_of(id));
        match (neighbour, change.parent) {
            (Some(number), _) => Place::With(number),
            (None, Some(parent)) => {
                let lists = named.len();
                Place::In(*named.entry((parent, change.member)).or_insert(lists))
            }
            (None, None) => Place::Nowhere,
        }
    }

    /// The list a change is in, found through the neighbours it names, and noted for
    /// every change on the way.
    fn list_of(&mut self, number: usize) -> Option<usize> {
        let mut passed = Vec::new();
        let mut at = number;
        let list = loop {
            match self.places.get(at).copied() {
                Some(Place::In(list)) => break Some(list),
                Some(Place::Nowhere) | None => break None,
                Some(Place::With(neighbour)) => {
                    // In no list until its list is found: changes of an update that
                    // name each other through their neighbours, which the library
                    // places nowhere, are met again here and are in none.
                    self.places[at] = Place::Nowhere;
                    passed.push(at);
                    at = neighbour;
                }
            }
        };
        let found = list.map_or(Place::Nowhere, Place::In);
        for number in passed {
            self.places[number] = found;
        }
        list
    }

    /// What the library may walk past to place a change after the left neighbour it
    /// names: the pieces of that neighbour after the place named and the changes below
    /// it, but for the change itself and what stands below it, which come after it,
    /// what stands below the right neighbour the walk stops at ([`Self::below_right`]),
    /// and the change it stops at. `None` where that is not known: it names no item to
    /// its left, or changes name each other in a ring.
    fn below_left(&self, number: usize) -> Option<u64> {
        let left = self.lefts[number]?;
        match (self.weights[left], self.weights[number]) {
            (Weight::Of(whole), Weight::Of(own)) => Some(
                whole
                    .saturating_sub(own)
                    .saturating_sub(self.below_right(number, left))
                    .saturating_add(1),
            ),
            _ => None,
        }
    }

    /// The weight of what stands below the right neighbour a change names, where the
    /// walk that places the change stops at that neighbour before any of it: the
    /// neighbour names the same place to its left as the change, so that it stands
    /// below the same change, and begins at the place the change names, so that no
    /// piece of it is cut off before. 0 where it is not so.
    fn below_right(&self, number: usize, left: usize) -> u64 {
        let held = self.held.begins.len();
        let Some(change) = number
            .checked_sub(held)
            .and_then(|at| self.sent.changes.get(at))
        else {
            return 0;
        };
        let (Some(left_place), Some(right_place)) = (change.left, change.right) else {
            return 0;
        };
        let Some(right) = self.item_of(right_place) else {
            return 0;
        };
        let begins = match right.checked_sub(held) {
            Some(at) => self.sent.clocks.begins[at],
            None => self.held.begins[right],
        };
        let stops_there = right != left
            && right != number
            && begins == right_place.1
            && self.left_places[right] == Some(left_place);
        match self.weights[right] {
            Weight::Of(weight) if stops_there => weight,
            Weight::Of(_) | Weight::Unknown => 0,
        }
    }

    /// What the changes of the update are charged: one step each that follows a
    /// change of its own update alone, and each other the list it goes into or, if it
    /// is less, what stands below the neighbour it names to its left.
    fn charge(mut self) -> u64 {
        let held = self.held.begins.len();
        let sent = self.sent;
        let mut charged = 0_u64;
        for (at, standing) in sent.standings.iter().enumerate() {
            let steps = match standing {
                Standing::Left => 0,
                Standing::Follows => 1,
                Standing::Walks => {
                    let through = self.list_of(held + at).map_or(0, |list| {
                        let size = self.sizes[list];
                        size.held
                            .saturating_add(size.coming)
                            .saturating_add(size.cuts)
                    });
                    self.below_left(held + at)
                        .map_or(through, |below| below.min(through))
                }
            };
            charged = charged.saturating_add(steps);
        }
        charged
    }
}

/// The weight of what stands below a change: itself, the pieces the changes of an
/// update may cut it into, and the weights of the changes that name it to their left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Weight {
    Of(u64),
    /// In a ring of changes that name each other to their left, or with one below it:
    /// the library places none of the ring.
    Unknown,
}

/// The weight of each change ([`Weight`]), by the item each names to its left and the
/// cuts of each: added up from the changes no change names, so that every change is
/// read once.
fn weights(lefts: &[Option<usize>], cuts: &[u64]) -> Vec<Weight> {
    let mut weights: Vec<u64> = cuts.iter().map(|cuts| cuts.saturating_add(1)).collect();
    // How many of the changes that name each one to their left are not added yet.
    let mut waiting = vec![0_u32; lefts.len()];
    for left in lefts.iter().flatten() {
        waiting[*left] += 1;
    }
    let mut ready: Vec<usize> = (0..lefts.len()).filter(|at| waiting[*at] == 0).collect();
    while let Some(at) = ready.pop() {
        let Some(left) = lefts[at] else {
            continue;
        };
        weights[left] = weights[left].saturating_add(weights[at]);
        waiting[left] -= 1;
        if waiting[left] == 0 {
            ready.push(left);
        }
    }
    weights
        .into_iter()
        .zip(waiting)
        .map(|(weight, waiting)| {
            if waiting == 0 {
                Weight::Of(weight)
            } else {
                Weight::Unknown
            }
        })
        .collect()
}
