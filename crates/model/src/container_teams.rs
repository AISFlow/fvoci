//! Workspace teams assigned to a board.

use serde::{Deserialize, Serialize};

/// A workspace team and the role its members inherit on a board.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ContainerTeam {
    /// The group, including its current nonguest members.
    pub team: crate::Team,
    /// No team may administer a board.
    pub role: crate::MinimumRole,
}

/// The role given to a team on a board.
#[derive(Debug, Clone, Copy, Deserialize, utoipa::ToSchema)]
pub struct ContainerTeamRole {
    /// The team's role, at most editor.
    pub role: crate::MinimumRole,
}
