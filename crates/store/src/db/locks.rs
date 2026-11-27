//! Transaction-scoped lock ranks and compile-time lock witnesses.

use std::marker::PhantomData;

/// Global lock acquisition rank within a transaction (`debug_assert!` checked).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum LockRank {
    Instance,
    LinkTurn,
    Organization,
    /// The row of an organization's provider, held before the account that signs in,
    /// tries settings or makes a token through it.
    Provider,
    Workspace,
    Container,
    DeletedContainers,
    Block,
    WorkspaceMembers,
    Users,
}

/// Compile-time witness that [`crate::users::lock_instance`] holds the instance row in
/// this transaction scope.
#[derive(Debug, Clone, Copy)]
pub(crate) struct InstanceHeld<'c>(PhantomData<fn(&'c ()) -> &'c ()>);

impl InstanceHeld<'_> {
    pub(crate) const fn new() -> Self {
        Self(PhantomData)
    }
}

/// Compile-time witness that an organization row is held in this transaction scope.
#[derive(Debug, Clone, Copy)]
pub(crate) struct OrganizationHeld<'c>(PhantomData<fn(&'c ()) -> &'c ()>);

impl OrganizationHeld<'_> {
    pub(crate) const fn new() -> Self {
        Self(PhantomData)
    }
}

/// Compile-time witness that [`crate::trash::hold_deleted_containers`] holds a
/// workspace's trashed containers in this transaction scope.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DeletedContainersHeld<'c>(PhantomData<fn(&'c ()) -> &'c ()>);

impl DeletedContainersHeld<'_> {
    pub(crate) const fn new() -> Self {
        Self(PhantomData)
    }
}

/// The lock a pass of `rank` goes under, for the ranks a transaction takes again after
/// deeper locks: the rows of several organizations under the turn of several links, in
/// any order, since the turn lets one such transaction through at a time; and the
/// workspaces of an organization under its row, one pass a workspace in the order of
/// their keys, each with what it holds under it.
#[cfg(debug_assertions)]
const fn pass_under(rank: LockRank) -> Option<LockRank> {
    match rank {
        LockRank::Organization => Some(LockRank::LinkTurn),
        LockRank::Workspace => Some(LockRank::Organization),
        _ => None,
    }
}

/// Records the lock of `key` at `rank`, asserting that it comes after what `locks` holds:
/// a deeper rank than the last lock, or the same rank and a greater key (any key for
/// [`LockRank::WorkspaceMembers`]). A new pass ([`pass_under`]) is checked against the
/// previous pass under the same lock, and nothing recorded is forgotten: what each pass
/// takes is checked within it, and a lock that is not a new pass against the last.
/// Nothing is a new pass once [`LockRank::Users`] is held.
#[cfg(debug_assertions)]
pub(super) fn push_checked_lock(locks: &mut Vec<(LockRank, String)>, rank: LockRank, key: &str) {
    if locks.iter().any(|(r, k)| *r == rank && k == key) {
        return;
    }
    let holds_users = locks.iter().any(|(r, _)| *r >= LockRank::Users);
    if !holds_users
        && let Some(under) = pass_under(rank)
        && let Some(start) = locks.iter().rposition(|(r, _)| *r == under)
    {
        if rank == LockRank::Organization {
            locks.push((rank, key.to_owned()));
            return;
        }
        if let Some((_, previous)) = locks[start + 1..].iter().rev().find(|(r, _)| *r == rank) {
            debug_assert!(
                previous.as_str() < key,
                "out-of-order {rank:?} pass: after ({rank:?}, {previous:?}), acquiring ({rank:?}, {key:?}); all={locks:?}",
            );
            locks.push((rank, key.to_owned()));
            return;
        }
    }
    if let Some((last_rank, last_key)) = locks.last() {
        debug_assert!(
            rank > *last_rank
                || (rank == *last_rank
                    && (rank == LockRank::WorkspaceMembers || last_key.as_str() < key)),
            "lock rank violation: holding ({last_rank:?}, {last_key:?}), acquiring ({rank:?}, {key:?}); all={locks:?}",
        );
    }
    locks.push((rank, key.to_owned()));
}

#[cfg(all(test, debug_assertions))]
mod tests {
    use super::LockRank::{
        Block, Container, DeletedContainers, Instance, LinkTurn, Organization, Users, Workspace,
        WorkspaceMembers,
    };
    use super::{LockRank, push_checked_lock};

    /// Records `sequence` in its order, as a transaction takes its locks.
    fn take(sequence: &[(LockRank, &str)]) -> Vec<(LockRank, String)> {
        let mut locks = Vec::new();
        for (rank, key) in sequence {
            push_checked_lock(&mut locks, *rank, key);
        }
        locks
    }

    #[test]
    fn each_workspace_of_an_organization_is_a_pass_with_its_own_order() {
        // As an account leaves its workspaces: each one's boards, trash and members.
        let locks = take(&[
            (Organization, "o"),
            (Workspace, "w1"),
            (Container, "c5"),
            (Container, "c7"),
            (DeletedContainers, "w1"),
            (WorkspaceMembers, "c7"),
            (WorkspaceMembers, "c5"),
            (Workspace, "w2"),
            (Container, "c1"),
            (Block, "b1"),
            (Workspace, "w3"),
        ]);
        assert_eq!(locks.len(), 11, "nothing is forgotten: {locks:?}");
    }

    #[test]
    fn the_turn_takes_organizations_in_any_order() {
        take(&[
            (Instance, "1"),
            (LinkTurn, "links"),
            (Organization, "o2"),
            (Workspace, "w9"),
            (Container, "c9"),
            (Organization, "o1"),
            (Workspace, "w1"),
        ]);
    }

    #[test]
    #[should_panic(expected = "out-of-order Workspace pass")]
    fn workspaces_are_passed_in_the_order_of_their_keys() {
        take(&[
            (Organization, "o"),
            (Workspace, "w2"),
            (Container, "c1"),
            (Workspace, "w1"),
        ]);
    }

    #[test]
    #[should_panic(expected = "lock rank violation")]
    fn a_board_is_not_taken_before_the_first_workspace() {
        take(&[(Organization, "o"), (Container, "c1"), (Workspace, "w1")]);
    }

    #[test]
    #[should_panic(expected = "lock rank violation")]
    fn the_boards_of_a_pass_are_taken_in_order() {
        take(&[
            (Organization, "o"),
            (Workspace, "w1"),
            (Container, "c2"),
            (Container, "c1"),
        ]);
    }

    #[test]
    #[should_panic(expected = "lock rank violation")]
    fn the_boards_of_a_later_pass_are_taken_in_order() {
        take(&[
            (Organization, "o"),
            (Workspace, "w1"),
            (Container, "c9"),
            (Workspace, "w2"),
            (Container, "c2"),
            (Container, "c1"),
        ]);
    }

    #[test]
    #[should_panic(expected = "lock rank violation")]
    fn no_workspace_is_passed_without_its_organization() {
        take(&[(Workspace, "w1"), (Container, "c1"), (Workspace, "w2")]);
    }

    #[test]
    #[should_panic(expected = "lock rank violation")]
    fn no_pass_begins_once_an_account_is_held() {
        take(&[
            (Organization, "o"),
            (Workspace, "w1"),
            (Users, "u"),
            (Workspace, "w2"),
        ]);
    }
}
