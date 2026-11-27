//! How many steps the Y library takes to read a document from its state, counted from
//! the bytes before it is asked to.
//!
//! The library (yrs 0.28.2) reads a state change by change, and places each among the
//! others of its list by the neighbours it names. Where the change to its left is not
//! followed by the change to its right, it walks on from the left one past every
//! change that names the same left neighbour, or one it walked past, until it meets
//! the right one or a change that is neither (`Item::resolve_conflict`). What an editor
//! writes is placed without a walk, or in a step or two where two people wrote at one
//! place. An update made for it names the last child of a long list to its right and
//! nothing to its left, with a client above the list's: each such change walks the
//! whole list, and walks it again every time the document is read, as an update is
//! taken and as the document is opened. An update is charged what its changes may walk
//! once ([`crate::placing`]); what they cost every later reading is counted here, for
//! the document: 62 changes of 383 bytes added 0.8 s to every reading of a document of
//! 50,000 paragraphs, and a dozen such updates made a document no worker opened in
//! its time.
//!
//! So the reading is done here first, on what the walk ([`crate::scan`]) reads of the
//! bytes, as the library does it:
//!
//! - It takes the clients from the highest down, and each client's changes in their
//!   order. A change that names one the document does not have yet waits on a stack
//!   while the other client's next change is taken (`BlockPicker`); the library looks
//!   through the stack each time, which is counted.
//! - It finds the neighbours a change names, cutting the change one lies inside. The
//!   changes are cut here beforehand, at every place a change names: a walk goes past
//!   two pieces where the library goes past one change, and takes them as it takes the
//!   one. Each cut is charged what the library pays for it: the changes of that client
//!   it moves, and the letters it copies.
//! - It takes the parent of a change from its neighbours, and puts the change in no
//!   list where it has none.
//! - It walks as above, keeping the changes it walked past in two sets, one of which
//!   it empties whenever the change moves right: emptying is charged the most the set
//!   held, as the table it clears is that large.
//!
//! The count is of the walk here, so it is the library's or more: a change of the
//! library that this cuts in two is two steps. It is the same every time, whatever the
//! machine does besides. Counting stops a little past the count it is asked for, so
//! that a state made to be counted for long is not.
//!
//! Measured (`tests/cost.rs`, `reading`, release build), the library takes 0.13 to
//! 0.24 µs for a step counted here where the steps are of its walk in a list of 50,000,
//! and less where they are of what is charged beside it. A step takes longer the more
//! changes the document has that lie far from their neighbours, as the walk goes
//! from one to the next wherever it is: 0.5 µs in a list of 200,000 such, 0.7 to 0.9 µs
//! in one of 700,000 (`scattered`). That is the document's size, which the count does
//! not weigh: a document of that many changes is slow to read with no steps at all.
//! The lists the reading here leaves are held to the library's own, state by state
//! (`tests/reading.rs`). What editors write into the largest bodies is a few thousand
//! steps at the most, of the changes that wait for another client's
//! (`tests/honest.rs`), and a body's own document is none; a document of thousands of
//! sessions that each wrote after the one before is as many steps as one made to be
//! walked, and as slow ([`crate::READING_STEPS`]).

mod cache;
#[cfg(any(test, feature = "test-util"))]
mod inspect;
mod origins;

use std::collections::HashMap;

use crate::placing::{PLACES_PER_STEP, VALUE_PLACES};
use crate::scan::{self, Change, Content, Id, Parent, Refusal, Walked};

pub(crate) use cache::{
    DirectItem, FastApply, ReadingCache, ReadingCacheBuilder, count_strict_with_cache,
};
#[cfg(any(test, feature = "test-util"))]
pub use inspect::List;
#[cfg(any(test, feature = "test-util"))]
pub(crate) use inspect::{lists, looked};
pub(crate) use origins::check_update_origins;

/// How many entries of the stack of waiting changes the library looks through in the
/// time of a step of its walk: it compares the client of each, at 1.4 ns (8,000 clients
/// that each waited for the one below, the lowest for the last of 100,000 changes, were
/// read in 1.1 s), so these take 0.18 µs. With it the clients a document may name
/// ([`crate::MAX_CLIENTS`]), each waiting for the one below, are the steps a document
/// may take to read ([`crate::READING_STEPS`]): the two limits are of the same time.
pub(crate) const LOOKED_PER_STEP: u64 = 128;

/// How many changes of a client the library moves, cutting one of them, in the time of
/// a step of its walk: it moves every later one in the client's array, at 0.5 ns each
/// (8,000 cuts of the first of a client's 400,000 changes took 1.6 s).
const MOVED_PER_STEP: u64 = 128;

/// How many changes the set the library empties may have held for the emptying to
/// take the time of a step of its walk: it clears a table of a byte or two for each
/// (a walk that kept 25,000 changes and then emptied the set at each of 25,000 more
/// took half as long again as one that emptied nothing).
const EMPTIED_PER_STEP: u64 = 4096;

/// No piece, where a piece is named by its number.
const NONE: u32 = u32::MAX;

/// No identifier, where one is packed ([`Replay::pack`]).
const NO_ID: u64 = u64::MAX;

/// What reading a state takes, and what the walk found of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Read {
    /// What the walk found of the state.
    pub(crate) walked: Walked,
    /// The steps the library takes to read it: counted exactly up to the count asked
    /// for, and past it no further than is needed to know that it is past.
    pub(crate) steps: u64,
    /// The most steps its reading may take once one more client has written into it
    /// ([`Replay::ahead`]).
    pub(crate) ahead: u64,
    /// Where each client's changes end in it, by the lengths counted here.
    pub(crate) ends: Vec<Id>,
    /// Whether any change in the state used non-YATA origins (e.g. `left == None` before
    /// a non-head right neighbour, or a right neighbour unreachable from `left` through
    /// conflicting items), requiring a settle re-read rather than skipping Passes 7–10.
    pub(crate) needs_settle: bool,
}

/// Whether [`Replay`] enforces strict origin placement checks alongside counting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OriginPolicy {
    /// Models raw `yrs` placement without rejecting misplaced origins (used by
    /// raw-equivalence tests and public step queries).
    Raw,
    /// Refuses misplaced origins (`Refusal::Origin`): dangling/non-item neighbours,
    /// non-type parents, cross-parent or cross-member neighbours, and identical or
    /// inverted neighbours within the same change/squashable client run.
    Strict,
}

/// Whether [`Replay::run`] resolves linked-list conflicts to count reading steps or
/// only tracks parent/member placement in O(N) time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StepsMode {
    Count(u64),
    Skip,
}

/// Counts the steps the library takes to read `state` into an empty document, exactly
/// up to `stop_past` and no further than a little past it.
///
/// # Errors
///
/// Returns why the state must not be handed to the library ([`scan::check`]), and
/// [`Refusal::Gap`] for a state that is no whole document: one whose changes name
/// changes it does not hold, or each other in a ring, which the library would leave
/// waiting.
pub(crate) fn count(state: &[u8], stop_past: u64) -> Result<Read, Refusal> {
    count_with(state, OriginPolicy::Raw, StepsMode::Count(stop_past))
}

/// Validates every change's origins and parent in `state` in linear time without
/// walking conflict lists (for limits with `reading_steps: None`).
pub(crate) fn check_origins(state: &[u8]) -> Result<Read, Refusal> {
    count_with(state, OriginPolicy::Strict, StepsMode::Skip)
}

fn count_with(state: &[u8], origins: OriginPolicy, steps: StepsMode) -> Result<Read, Refusal> {
    let (walked, mut replay) = Replay::read(state, origins)?;
    replay.run(steps)?;
    Ok(Read {
        walked,
        steps: replay.steps(),
        ahead: replay.ahead(),
        ends: replay.ends(),
        needs_settle: replay.needs_settle,
    })
}

/// What a change holds, as far as the reading asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Holds {
    /// A stretch that was collected: no item.
    Collected,
    /// Deleted content, which is the parent of nothing.
    Deleted,
    /// Letters, which are no parent.
    Letters,
    /// Values, which are no parent either.
    Values,
    /// A shared type, which may be a parent.
    Type,
    /// What is one place long and is no shared type (bytes, embed, formatting).
    One,
}

impl Holds {
    /// Whether `yrs` may squash adjacent items of this content of the same client into
    /// a single block (exposing the double-split bug if `right_origin <= origin`).
    fn is_squashable(self) -> bool {
        matches!(self, Self::Letters | Self::Values | Self::Deleted)
    }
}

/// Where a change says it belongs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Named {
    /// With the neighbours it names.
    Neighbours,
    /// In a root, by its number.
    Root(u32),
    /// In a shared type, by the packed identifier of the change that made it.
    Item(u64),
}

/// Whether the library has taken a piece.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Taken {
    /// Not yet.
    Waiting,
    /// As an item, in a list.
    Item,
    /// As no item: it was collected, or has no parent.
    Gone,
}

/// A change of the state, or a piece of one that changes of the state cut it into.
#[derive(Debug, Clone)]
struct Piece {
    /// Its client, by its number here.
    client: u32,
    begin: u32,
    end: u32,
    /// The neighbours the change names, packed.
    origin: u64,
    right_origin: u64,
    named: Named,
    /// The member the change says it is a value of, by its number.
    member: u32,
    holds: Holds,
    /// Whether it is a piece of the change before it, and no first piece.
    continues: bool,
    /// Whether the library has cut its change where this piece begins.
    cut: bool,
    taken: Taken,
    /// The pieces next to it in its list.
    left: u32,
    right: u32,
    /// The list it is in: the branch, and the member it is a value of.
    parent: u32,
    sub: u32,
    /// Its first child, if it is a shared type.
    start: u32,
    /// The item its left neighbour lies in, found as it was taken.
    origin_item: u32,
    /// The marks of the last walk that went past it ([`Replay::mark`]).
    before: u32,
    conflicting: u32,
    /// Monotonic group ID for pieces of the same wire change or contiguous squashable
    /// client run, so inverted same-block origins are caught in O(1).
    group: u32,
}

/// The changes of one client.
#[derive(Debug, Clone, Copy)]
struct Run {
    /// Its pieces, by their numbers; none where `first` is [`NONE`].
    first: u32,
    after: u32,
    /// The piece the library takes next.
    next: u32,
    /// The clock after the last change the library has taken.
    clock: u32,
    /// Whether the library has begun to take its changes in their turn: a change that
    /// waits for one of them then waits for good.
    started: bool,
    /// Whether one of its changes waits on the stack.
    stacked: bool,
}

impl Run {
    const EMPTY: Self = Self {
        first: NONE,
        after: 0,
        next: 0,
        clock: 0,
        started: false,
        stacked: false,
    };

    fn is_empty(&self) -> bool {
        self.first == NONE
    }
}

/// The reading of a state.
#[derive(Debug)]
struct Replay<'bytes> {
    pieces: Vec<Piece>,
    /// The identifier of each client, by its number here.
    clients: Vec<u64>,
    /// The changes of each client, by its number.
    runs: Vec<Run>,
    /// The first child of each root.
    roots: Vec<u32>,
    /// The value each member of a branch has: the last of its list.
    members: HashMap<(u32, u32), u32>,
    /// The changes that wait for another client's.
    stack: Vec<u32>,
    /// The last mark given out ([`Self::mark`]).
    marks: u32,
    /// Changes walked past.
    walked: u64,
    /// Entries of the stack looked through.
    looked: u64,
    /// Changes moved for cuts.
    moved: u64,
    /// Places of changes copied for cuts.
    copied: u64,
    /// What the emptied sets had held.
    emptied: u64,
    origins: OriginPolicy,
    /// Set when any piece uses non-YATA origins so live takes fall back to a settle pass.
    needs_settle: bool,
    names: Names<'bytes>,
}

/// The numbers of the clients, roots and members a state names.
#[derive(Debug, Default)]
struct Names<'bytes> {
    clients: HashMap<u64, u32>,
    roots: HashMap<&'bytes [u8], u32>,
    members: HashMap<&'bytes [u8], u32>,
}

fn number_in<Key: std::hash::Hash + Eq>(numbers: &mut HashMap<Key, u32>, key: Key) -> u32 {
    let next = small(numbers.len());
    *numbers.entry(key).or_insert(next)
}

/// A count as a number of 32 bits: a state has fewer changes than bytes, and no state
/// read here has 4 GiB.
fn small(count: usize) -> u32 {
    u32::try_from(count).unwrap_or(NONE)
}

fn index(number: u32) -> usize {
    usize::try_from(number).unwrap_or(usize::MAX)
}

fn clock(clock: u64) -> u32 {
    // A clock is at most u32::MAX here (`scan::id`); state clocks are at most 31 bits.
    u32::try_from(clock).unwrap_or(u32::MAX)
}

impl<'bytes> Replay<'bytes> {
    /// An identifier as one number: the client by its number here, and the clock.
    fn pack(&mut self, (client, at): Id) -> u64 {
        let client = self.client(client);
        (u64::from(client) << 32) | u64::from(clock(at))
    }

    fn client(&mut self, client: u64) -> u32 {
        let number = number_in(&mut self.names.clients, client);
        if index(number) == self.clients.len() {
            self.clients.push(client);
            self.runs.push(Run::EMPTY);
        }
        number
    }

    fn empty(origins: OriginPolicy) -> Self {
        Self {
            pieces: Vec::new(),
            clients: Vec::new(),
            runs: Vec::new(),
            roots: Vec::new(),
            members: HashMap::new(),
            stack: Vec::new(),
            marks: 0,
            walked: 0,
            looked: 0,
            moved: 0,
            copied: 0,
            emptied: 0,
            origins,
            needs_settle: false,
            names: Names::default(),
        }
    }

    /// Walks a state and keeps of each change what the reading asks, cut at every
    /// place a change names.
    fn read(state: &'bytes [u8], origins: OriginPolicy) -> Result<(Walked, Self), Refusal> {
        let mut replay = Self::empty(origins);
        let mut self_named = false;
        let walked = scan::walk(state, |(client, begin), change| {
            if origins == OriginPolicy::Strict {
                let len = change.content.length();
                if len > 0 && scan::names_self(client, begin, begin.saturating_add(len), change) {
                    self_named = true;
                }
            }
            replay.keep((client, begin), change);
        })?;
        if self_named {
            return Err(Refusal::Origin);
        }
        // A state begins every client's changes at their beginning.
        if walked.begins.iter().any(|(_, begin)| *begin > 0) {
            return Err(Refusal::Gap);
        }
        replay.roots = vec![NONE; replay.names.roots.len()];
        replay.cut();
        Ok((walked, replay))
    }

    /// Keeps a change of the state, unless it is of no length: the library drops one.
    fn keep(&mut self, (client, begin): Id, change: &Change<'bytes>) {
        let length = change.content.length();
        if length == 0 {
            return;
        }
        let holds = match change.content {
            Content::Collected(_) => Holds::Collected,
            Content::Deleted(_) => Holds::Deleted,
            Content::Letters(_) => Holds::Letters,
            Content::Values(_) => Holds::Values,
            Content::Type => Holds::Type,
            Content::One => Holds::One,
        };
        let named = match change.parent {
            None => Named::Neighbours,
            Some(Parent::Root(name)) => Named::Root(number_in(&mut self.names.roots, name)),
            Some(Parent::Change(id)) => Named::Item(self.pack(id)),
        };
        let piece = Piece {
            client: self.client(client),
            begin: clock(begin),
            end: clock(begin.saturating_add(length)),
            origin: change.left.map_or(NO_ID, |id| self.pack(id)),
            right_origin: change.right.map_or(NO_ID, |id| self.pack(id)),
            named,
            member: change
                .member
                .map_or(NONE, |name| number_in(&mut self.names.members, name)),
            holds,
            continues: false,
            cut: false,
            taken: Taken::Waiting,
            left: NONE,
            right: NONE,
            parent: NONE,
            sub: NONE,
            start: NONE,
            origin_item: NONE,
            before: 0,
            conflicting: 0,
            group: 0,
        };
        let number = small(self.pieces.len());
        let run = &mut self.runs[index(piece.client)];
        if run.is_empty() {
            run.first = number;
            run.next = number;
        }
        run.after = number + 1;
        self.pieces.push(piece);
    }

    /// Cuts the changes at every place a change names: after the neighbour it names to
    /// its left and before the one to its right, as the library cuts them when it
    /// finds those neighbours. Each cut inside a change is charged the changes of its
    /// client, which the library moves, and the length of the change, which it copies.
    fn cut(&mut self) {
        const CLOCK_BITS: u64 = 0xffff_ffff;
        let mut cuts: Vec<u64> = Vec::new();
        for piece in &self.pieces {
            if piece.origin != NO_ID {
                // Origin clocks are bounded to u32 by `scan::id`, not to 31 bits: at
                // u32::MAX the next clock would carry into the client's number, and
                // there is nothing after it to cut.
                if piece.origin & CLOCK_BITS != CLOCK_BITS {
                    cuts.push(piece.origin + 1);
                }
            }
            if piece.right_origin != NO_ID {
                cuts.push(piece.right_origin);
            }
        }
        cuts.sort_unstable();
        cuts.dedup();
        let whole = std::mem::take(&mut self.pieces);
        let mut pieces: Vec<Piece> = Vec::with_capacity(whole.len());
        let mut cut = cuts.iter().copied().peekable();
        // The pieces of each client follow each other in the order of their clocks, and
        // the clients in the order of their numbers but for those first named as a
        // neighbour: the cuts of each client are found again where its pieces begin.
        let mut client = NONE;
        let mut cuts_of_client = 0_u64;
        for piece in whole {
            if piece.client != client {
                self.moved_for(client, cuts_of_client, pieces.len());
                cuts_of_client = 0;
                client = piece.client;
                let from = u64::from(client) << 32;
                let at = cuts.partition_point(|cut| *cut < from);
                cut = cuts[at..].iter().copied().peekable();
                let run = &mut self.runs[index(client)];
                run.first = small(pieces.len());
                run.next = run.first;
            }
            // What a cut inside it copies: its places, by what it holds.
            let places = u64::from(piece.end - piece.begin).saturating_mul(match piece.holds {
                Holds::Letters => 1,
                Holds::Values => VALUE_PLACES,
                Holds::Collected | Holds::Deleted | Holds::Type | Holds::One => 0,
            });
            let mut rest = piece;
            let here = u64::from(client) << 32;
            while let Some(at) = cut.peek().copied() {
                if at >= (here | u64::from(rest.end)) {
                    break;
                }
                cut.next();
                let at = clock(at & 0xffff_ffff);
                if at <= rest.begin || rest.holds == Holds::Collected {
                    continue;
                }
                self.copied = self.copied.saturating_add(places);
                cuts_of_client += 1;
                let cut_off = Piece {
                    end: at,
                    ..rest.clone()
                };
                rest = Piece {
                    begin: at,
                    continues: true,
                    ..rest
                };
                pieces.push(cut_off);
            }
            pieces.push(rest);
            self.runs[index(client)].after = small(pieces.len());
        }
        self.moved_for(client, cuts_of_client, pieces.len());
        self.pieces = pieces;
        self.assign_groups();
    }

    /// Charges the cuts of a client's changes: each moves no more than the pieces the
    /// client has in the end, which are the last of `pieces` so far.
    fn moved_for(&mut self, client: u32, cuts: u64, pieces: usize) {
        let Some(run) = self.runs.get(index(client)) else {
            return;
        };
        let of_client = u64::from(small(pieces).saturating_sub(run.first));
        self.moved = self.moved.saturating_add(cuts.saturating_mul(of_client));
    }

    /// The steps counted so far.
    fn steps(&self) -> u64 {
        self.steps_with(self.looked)
    }

    /// The steps counted so far, had `looked` entries of the stack been looked through.
    fn steps_with(&self, looked: u64) -> u64 {
        self.walked
            .saturating_add(looked.div_ceil(LOOKED_PER_STEP))
            .saturating_add(self.moved.div_ceil(MOVED_PER_STEP))
            .saturating_add(self.copied.div_ceil(PLACES_PER_STEP))
            .saturating_add(self.emptied.div_ceil(EMPTIED_PER_STEP))
    }

    /// How many changes the reading takes (a change cut into pieces is one), and how
    /// many clients have any.
    fn sizes(&self) -> (u64, u64) {
        let changes = self.pieces.iter().filter(|piece| !piece.continues).count();
        let clients = self.runs.iter().filter(|run| !run.is_empty()).count();
        (u64::from(small(changes)), u64::from(small(clients)))
    }

    /// The most steps the reading of the changes the state has may take once one more
    /// client is read before the others: the steps counted, with the stack looked
    /// through as often as it can be, each change once for each client.
    ///
    /// Every round of the reading ([`Self::run`]) takes one change, after it took at
    /// most one off the stack. A round may put several on it and a change may be put
    /// on it more than once, but each time one is put on it, it is taken off in a
    /// later round, of which there are as many as changes: no more are put on it, all
    /// told, than there are changes. It holds one change of a client at the most, and
    /// none of the client waited for, so each is put on a stack of fewer changes than
    /// there are clients; with one more client, of as many as there are now. Which
    /// changes wait is a matter of which client is read first, the one of the highest
    /// identifier, and a client draws its identifier by chance: a document read in few
    /// steps is read in these if the next client's is the highest and it wrote after
    /// the last of a line of clients that each wrote after the one before.
    ///
    /// Not in it is what that client, or any later update, adds to the changes: each
    /// change it writes, and each change it cuts in two by writing or deleting inside
    /// it (the state stored after it has them cut), is one more to look at for each
    /// client.
    fn ahead(&self) -> u64 {
        let (changes, clients) = self.sizes();
        self.steps_with(changes.saturating_mul(clients))
    }

    /// Where each client's changes end.
    fn ends(&self) -> Vec<Id> {
        self.runs
            .iter()
            .zip(&self.clients)
            .filter(|(run, _)| !run.is_empty())
            .map(|(run, client)| {
                let last = &self.pieces[index(run.after - 1)];
                (*client, u64::from(last.end))
            })
            .collect()
    }

    /// Takes the changes as the library takes them (`Update::integrate`), until all
    /// are taken or more than `stop_past` steps are counted.
    fn run(&mut self, steps: StepsMode) -> Result<(), Refusal> {
        // The clients from the highest identifier down.
        let mut order: Vec<u32> = (0..small(self.runs.len()))
            .filter(|client| !self.runs[index(*client)].is_empty())
            .collect();
        order.sort_unstable_by_key(|client| self.clients[index(*client)]);
        let mut latest = NONE;
        loop {
            let block = match self.stack.pop() {
                Some(block) => {
                    let client = self.pieces[index(block)].client;
                    self.runs[index(client)].stacked = false;
                    block
                }
                None => loop {
                    if latest != NONE
                        && let Some(block) = self.next_of(latest)
                    {
                        break block;
                    }
                    let Some(client) = order.pop() else {
                        return Ok(());
                    };
                    self.runs[index(client)].started = true;
                    latest = client;
                },
            };
            let mut block = block;
            // A change that names one the document does not have waits, and the next
            // change of that one's client is taken first.
            while let Some(missing) = self.missing(block) {
                let client = self.pieces[index(block)].client;
                self.stack.push(block);
                self.runs[index(client)].stacked = true;
                self.looked = self
                    .looked
                    .saturating_add(u64::from(small(self.stack.len())));
                let run = self.runs[index(missing)];
                if run.started || run.stacked {
                    return Err(Refusal::Gap);
                }
                block = self.next_of(missing).ok_or(Refusal::Gap)?;
            }
            self.take(block, steps)?;
            if let StepsMode::Count(stop_past) = steps
                && self.steps() > stop_past
            {
                return Ok(());
            }
        }
    }

    /// The next change of a client, by its first piece, taken off its changes.
    fn next_of(&mut self, client: u32) -> Option<u32> {
        let run = &mut self.runs[index(client)];
        if run.is_empty() || run.next >= run.after {
            return None;
        }
        let block = run.next;
        let mut after = block + 1;
        while after < run.after && self.pieces[index(after)].continues {
            after += 1;
        }
        run.next = after;
        Some(block)
    }

    /// The client whose changes a change waits for, if it names one the document
    /// does not have (`Update::missing_dependency`).
    fn missing(&self, block: u32) -> Option<u32> {
        let piece = &self.pieces[index(block)];
        if piece.holds == Holds::Collected {
            return None;
        }
        let parent = match piece.named {
            Named::Item(id) => id,
            Named::Neighbours | Named::Root(_) => NO_ID,
        };
        [piece.origin, piece.right_origin, parent]
            .into_iter()
            .filter(|id| *id != NO_ID)
            .find_map(|id| {
                let (client, at) = unpack(id);
                (at >= self.runs[index(client)].clock).then_some(client)
            })
    }

    /// The item that covers the place of a packed identifier the document has.
    fn item_at(&self, id: u64) -> Option<u32> {
        if id == NO_ID {
            return None;
        }
        let (client, at) = unpack(id);
        let run = self.runs[index(client)];
        if run.is_empty() {
            return None;
        }
        let pieces = &self.pieces[index(run.first)..index(run.after)];
        let within = pieces.partition_point(|piece| piece.begin <= at);
        let number = run.first + small(within.checked_sub(1)?);
        let piece = &self.pieces[index(number)];
        (at < piece.end && piece.taken == Taken::Item).then_some(number)
    }

    /// The first child of a branch: of a shared type by its piece, or of a root.
    fn start_of(&mut self, branch: u32) -> &mut u32 {
        let pieces = self.pieces.len();
        match self.pieces.get_mut(index(branch)) {
            Some(piece) => &mut piece.start,
            None => &mut self.roots[index(branch) - pieces],
        }
    }

    /// The first value a member of a branch had, found from its last as the library
    /// finds it, a step for each value.
    fn first_value(&mut self, branch: u32, member: u32) -> u32 {
        let Some(mut value) = self.members.get(&(branch, member)).copied() else {
            return NONE;
        };
        while self.pieces[index(value)].left != NONE {
            value = self.pieces[index(value)].left;
            self.walked = self.walked.saturating_add(1);
        }
        value
    }

    /// A mark no walk has used.
    fn mark(&mut self) -> u32 {
        match self.marks.checked_add(1) {
            Some(mark) => self.marks = mark,
            // More walks than there are marks: past any count.
            None => self.walked = u64::MAX,
        }
        self.marks
    }

    /// Takes a change, by its first piece, as the library takes one that names
    /// nothing the document does not have (`TransactionMut::integrate_item`).
    fn take(&mut self, first: u32, steps: StepsMode) -> Result<(), Refusal> {
        let mut after = first + 1;
        while self
            .pieces
            .get(index(after))
            .is_some_and(|piece| piece.continues)
        {
            after += 1;
        }
        let last = after - 1;
        let change = self.pieces[index(first)].clone();
        self.runs[index(change.client)].clock = self.pieces[index(last)].end;
        let origin_item = self.item_at(change.origin);
        let right = self.item_at(change.right_origin);
        if self.origins == OriginPolicy::Strict {
            self.check_piece_origins(&change, origin_item, right)?;
        }
        let left = match steps {
            StepsMode::Count(_) => self.left_of(origin_item, right),
            StepsMode::Skip => origin_item,
        };
        let place = if change.holds == Holds::Collected {
            None
        } else {
            self.place_of(&change, left, right)?
        };
        let Some((parent, sub)) = place else {
            for piece in &mut self.pieces[index(first)..index(after)] {
                piece.taken = Taken::Gone;
            }
            return Ok(());
        };
        if steps == StepsMode::Skip {
            for piece in &mut self.pieces[index(first)..index(after)] {
                piece.taken = Taken::Item;
                piece.parent = parent;
                piece.sub = sub;
            }
            return Ok(());
        }
        let conflict = match (left, right) {
            (None, None) => true,
            (None, Some(right)) => self.pieces[index(right)].left != NONE,
            (Some(left), _) => self.pieces[index(left)].right != right.unwrap_or(NONE),
        };
        let left_then = if conflict {
            self.resolve(&change, left, right, (parent, sub))
        } else {
            left
        };
        let next = match left_then {
            Some(left) => std::mem::replace(&mut self.pieces[index(left)].right, first),
            None if sub != NONE => self.first_value(parent, sub),
            None => std::mem::replace(self.start_of(parent), first),
        };
        for number in first..after {
            let piece = &mut self.pieces[index(number)];
            piece.taken = Taken::Item;
            piece.parent = parent;
            piece.sub = sub;
            piece.left = if number == first {
                left_then.unwrap_or(NONE)
            } else {
                number - 1
            };
            piece.right = if number == last { next } else { number + 1 };
        }
        self.pieces[index(first)].origin_item = origin_item.unwrap_or(NONE);
        if next != NONE {
            self.pieces[index(next)].left = last;
        } else if sub != NONE {
            self.members.insert((parent, sub), last);
        }
        Ok(())
    }

    /// The item the library takes for the left neighbour of a change, of the item its
    /// left neighbour lies in and the item its right neighbour begins: the first, but
    /// where the right neighbour lies in the same change at or before the left one and
    /// the library has not cut the change anywhere from there to the left one. It
    /// cuts after the left neighbour first and holds what is before the cut, then cuts
    /// that before the right neighbour, and what it holds is what is before that cut.
    /// The cuts it makes for the two are noted.
    fn left_of(&mut self, origin_item: Option<u32>, right: Option<u32>) -> Option<u32> {
        let uncut = |piece: &Piece| piece.continues && !piece.cut;
        let left = match (origin_item, right) {
            (Some(left), Some(right))
                if right <= left && self.pieces[index(right)..=index(left)].iter().all(uncut) =>
            {
                Some(right - 1)
            }
            _ => origin_item,
        };
        if let Some(after) = origin_item.and_then(|item| self.pieces.get_mut(index(item) + 1)) {
            after.cut = true;
        }
        if let Some(right) = right {
            self.pieces[index(right)].cut = true;
        }
        left
    }

    /// The list a change goes into: the branch, and the member it is a value of.
    /// `None` where the library finds it no parent and keeps it as no item.
    fn place_of(
        &self,
        change: &Piece,
        left: Option<u32>,
        right: Option<u32>,
    ) -> Result<Option<(u32, u32)>, Refusal> {
        let sub_of = |piece: Option<u32>| piece.map_or(NONE, |piece| self.pieces[index(piece)].sub);
        Ok(match change.named {
            Named::Root(root) => Some((small(self.pieces.len()) + root, change.member)),
            Named::Item(id) => match self.item_at(id) {
                None => None,
                Some(parent) => match self.pieces[index(parent)].holds {
                    Holds::Type => Some((parent, change.member)),
                    Holds::Deleted | Holds::Collected => None,
                    // The library refuses the update.
                    Holds::Letters | Holds::Values | Holds::One => return Err(Refusal::Unknown),
                },
            },
            Named::Neighbours => match (left, right) {
                (Some(left), _) => {
                    let sub = match sub_of(Some(left)) {
                        NONE => sub_of(right),
                        sub => sub,
                    };
                    Some((self.pieces[index(left)].parent, sub))
                }
                (None, Some(right)) => {
                    Some((self.pieces[index(right)].parent, sub_of(Some(right))))
                }
                (None, None) => None,
            },
        })
    }

    /// The neighbour a change that is not where its neighbours left room for it goes
    /// after (`Item::resolve_conflict`): the walk, a step for every piece it goes past.
    fn resolve(
        &mut self,
        change: &Piece,
        left: Option<u32>,
        right: Option<u32>,
        (parent, sub): (u32, u32),
    ) -> Option<u32> {
        let mut at = match left {
            Some(left) => self.pieces[index(left)].right,
            None if sub != NONE => self.first_value(parent, sub),
            None => *self.start_of(parent),
        };
        let client = self.clients[index(change.client)];
        let right = right.unwrap_or(NONE);
        let mut left = left;
        let walk = self.mark();
        let mut conflicting = self.mark();
        let (mut held, mut most) = (0_u64, 0_u64);
        let mut held_break = false;
        while at != NONE && at != right {
            self.walked = self.walked.saturating_add(1);
            held += 1;
            most = most.max(held);
            let piece = &mut self.pieces[index(at)];
            piece.before = walk;
            piece.conflicting = conflicting;
            let piece = &self.pieces[index(at)];
            let next = piece.right;
            // A piece that is no first one names the piece before it.
            let (origin, origin_item) = if piece.continues {
                (
                    (u64::from(piece.client) << 32) | u64::from(piece.begin - 1),
                    at - 1,
                )
            } else {
                (piece.origin, piece.origin_item)
            };
            let moves = if origin == change.origin {
                if self.clients[index(piece.client)] < client {
                    true
                } else if piece.right_origin == change.right_origin {
                    held_break = true;
                    break;
                } else {
                    false
                }
            } else {
                let Some(named) = self.pieces.get(index(origin_item)) else {
                    if right != NONE {
                        self.needs_settle = true;
                    }
                    break;
                };
                if named.before != walk {
                    if right != NONE {
                        self.needs_settle = true;
                    }
                    break;
                }
                named.conflicting != conflicting
            };
            if moves {
                left = Some(at);
                self.emptied = self.emptied.saturating_add(most);
                conflicting = self.mark();
                held = 0;
            }
            at = next;
        }
        if right != NONE && at == NONE && !held_break {
            self.needs_settle = true;
        }
        left
    }
}

fn unpack(id: u64) -> (u32, u32) {
    (
        u32::try_from(id >> 32).unwrap_or(NONE),
        u32::try_from(id & 0xffff_ffff).unwrap_or(u32::MAX),
    )
}
