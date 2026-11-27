//! Inspection helpers for [`super::Replay`] used by tests that verify step counting
//! and list layout equivalence against `yrs`.

use super::{NONE, OriginPolicy, Piece, Replay, StepsMode, index, small};
use crate::scan::{Id, Refusal};

/// A list as the reading here leaves it: the children of a parent, or the values a
/// member of it had.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct List {
    /// The change that made the parent; `None` for a root.
    pub parent: Option<Id>,
    /// The member, for a list of values.
    pub member: Option<Vec<u8>>,
    /// The pieces from the first to the last, each as its client and the clocks it
    /// begins and ends at.
    pub pieces: Vec<(u64, u64, u64)>,
}

impl Replay<'_> {
    /// The lists as they stand, for the tests that hold them to the library's.
    pub(super) fn lists(&self) -> Vec<List> {
        let from = |mut at: u32| {
            let mut pieces = Vec::new();
            while let Some(piece) = self.pieces.get(index(at)) {
                pieces.push((
                    self.clients[index(piece.client)],
                    u64::from(piece.begin),
                    u64::from(piece.end),
                ));
                at = piece.right;
            }
            pieces
        };
        let id_of = |piece: &Piece| (self.clients[index(piece.client)], u64::from(piece.begin));
        let parent_of = |branch: u32| self.pieces.get(index(branch)).map(id_of);
        let mut lists: Vec<List> = self
            .roots
            .iter()
            .map(|start| List {
                parent: None,
                member: None,
                pieces: from(*start),
            })
            .collect();
        for (number, piece) in self.pieces.iter().enumerate() {
            if piece.taken == super::Taken::Item && piece.start != NONE {
                lists.push(List {
                    parent: parent_of(small(number)),
                    member: None,
                    pieces: from(piece.start),
                });
            }
        }
        for ((branch, member), last) in &self.members {
            let mut first = *last;
            while self.pieces[index(first)].left != NONE {
                first = self.pieces[index(first)].left;
            }
            let name = self
                .names
                .members
                .iter()
                .find(|(_, number)| *number == member);
            lists.push(List {
                parent: parent_of(*branch),
                member: name.map(|(name, _)| name.to_vec()),
                pieces: from(first),
            });
        }
        lists
    }
}

/// How many entries of the stack of waiting changes the reading of a state looks
/// through, counted to the end, with how many changes and clients the state has: no
/// more than each change once for each client but one ([`Replay::ahead`]).
pub(crate) fn looked(state: &[u8]) -> Result<(u64, (u64, u64)), Refusal> {
    let (_, mut replay) = Replay::read(state, OriginPolicy::Raw)?;
    replay.run(StepsMode::Count(u64::MAX))?;
    Ok((replay.looked, replay.sizes()))
}

/// The steps the library takes to read a state, counted to the end, and the lists the
/// reading here leaves ([`List`]).
pub(crate) fn lists(state: &[u8]) -> Result<(u64, Vec<List>), Refusal> {
    let (_, mut replay) = Replay::read(state, OriginPolicy::Raw)?;
    replay.run(StepsMode::Count(u64::MAX))?;
    Ok((replay.steps(), replay.lists()))
}
