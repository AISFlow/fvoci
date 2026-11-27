//! What an agent reads first of a workspace (`getWorkspaceOverview`): the workspace,
//! who reads it, and the containers they read, with the properties a board's cards
//! take, in one answer.

use serde::{Deserialize, Serialize};

use crate::{
    CardPropertyDefinition, ContainerKind, ContainerRole, ContainerVisibility, OrgKind,
    WorkspaceRole,
};

/// A workspace, its containers and who reads them, as an agent finds its way by.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceOverview {
    /// The workspace, with its organization and what the reader is in it.
    pub workspace: OverviewWorkspace,
    /// Who reads it.
    pub me: OverviewPerson,
    /// The containers they read, oldest first.
    pub containers: Vec<OverviewContainer>,
}

/// The workspace of an overview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OverviewWorkspace {
    /// Its identifier.
    pub id: String,
    /// The identifier of its organization.
    pub org_id: String,
    /// Its name; empty for the workspace of the user's own space, which a client names
    /// in the reader's language.
    pub name: String,
    /// What its organization is: one people work in together, or the user's own space.
    pub kind: OrgKind,
    /// What the reader is in it.
    pub my_workspace_role: WorkspaceRole,
}

/// Who reads an overview, by the name they are shown by.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OverviewPerson {
    /// Their identifier.
    pub id: String,
    /// Their login name.
    pub username: String,
    /// The name they are shown by: their display name, or the login name if they have
    /// none.
    pub name: String,
}

/// A container of an overview: what it is and what the reader is on it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OverviewContainer {
    /// Its identifier.
    pub id: String,
    /// What it is for.
    pub kind: ContainerKind,
    /// Its title.
    pub title: String,
    /// Its icon, usually one emoji; empty for none.
    pub icon: String,
    /// Whether it is open to whoever works in the workspace or private to its members.
    pub visibility: ContainerVisibility,
    /// Whether the reader is a member of it, by a role of their own or of a group they
    /// are in: an open container they read without being one is `false`.
    pub joined: bool,
    /// The reader's role on it, the higher of their own and their groups'; absent where
    /// they are no member.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub my_role: Option<ContainerRole>,
    /// When it last changed, in milliseconds since the Unix epoch.
    pub updated_at: i64,
    /// The properties of the cards of a board, in display order: what a card's
    /// `properties` are keyed by. Absent for any other kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card_properties: Option<Vec<CardPropertyDefinition>>,
}
