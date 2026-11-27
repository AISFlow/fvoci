//! Workspaces: where the members of an organization work, as the routes under a workspace
//! answer one.

use serde::{Deserialize, Serialize};

use crate::JsonMap;

/// A workspace of an organization.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct Workspace {
    /// The workspace's identifier.
    pub id: String,
    /// The workspace's title.
    pub title: String,
    /// Free-form workspace settings.
    #[schema(value_type = Object)]
    pub settings: JsonMap,
    /// The identifier of the user who last modified the workspace.
    pub modified_by: String,
    /// Last modification time in milliseconds since the Unix epoch.
    pub updated_at: i64,
    /// Whether the admins of the workspace's boards may invite guests to them.
    pub guest_invitations: bool,
}

/// A request to turn the invitations of guests to a workspace's boards on or off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, utoipa::ToSchema)]
pub struct GuestInvitationsChange {
    /// Whether the admins of the workspace's boards may invite guests to them.
    pub enabled: bool,
}

impl Default for Workspace {
    /// A workspace with nothing set, in which guests may be invited.
    fn default() -> Self {
        Self {
            id: String::new(),
            title: String::new(),
            settings: JsonMap::new(),
            modified_by: String::new(),
            updated_at: 0,
            guest_invitations: true,
        }
    }
}
