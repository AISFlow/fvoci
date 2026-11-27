//! Which open boards a list of the boards of a workspace holds besides a user's own.

/// Whether a board query also returns the open boards the user is not a member of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicContainers {
    /// Open boards are returned whether or not the user is a member.
    Include,
    /// Only boards the user is a member of are returned.
    Exclude,
    /// Open wikis and open courses are returned whether or not the user is a member, as
    /// whoever works in the workspace reads them; an open board only if the user is its
    /// member.
    OpenToRead,
}
