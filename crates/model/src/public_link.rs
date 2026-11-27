//! Public link access to a board through a read token.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::serde_util::is_zero;

/// What to do about the public link of a board.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PublicLinkChange {
    /// Enable the public link. A board whose public link is enabled already keeps its token.
    On,
    /// Enable the public link under a new token: the old link stops working.
    OnWithNewToken,
    /// Disable the public link and forget its token.
    Off,
}

/// The public link state of a board.
///
/// The JSON form is what is answered to those who may manage the board's public link:
/// whether it is enabled, the read token while it is, and when the state last changed.
/// A board that never had a public link is `{"enabled": false}`.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct PublicLink {
    /// The identifier of the board.
    #[serde(skip)]
    pub container_id: String,
    /// Whether the public link is enabled.
    pub enabled: bool,
    /// The read token: whoever has it reads the board. The server makes it. Empty, and
    /// left out of the JSON form, while the public link is disabled.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub token: String,
    /// The identifier of the user who last changed the public link state.
    #[serde(skip)]
    pub modified_by: String,
    /// When the state last changed, in milliseconds since the Unix epoch. Zero, and
    /// left out of the JSON form, for a board that never had a public link.
    #[serde(skip_serializing_if = "is_zero")]
    pub updated_at: i64,
}

impl fmt::Debug for PublicLink {
    /// Leaves out the read token.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PublicLink")
            .field("container_id", &self.container_id)
            .field("enabled", &self.enabled)
            .field("modified_by", &self.modified_by)
            .field("updated_at", &self.updated_at)
            .finish_non_exhaustive()
    }
}
