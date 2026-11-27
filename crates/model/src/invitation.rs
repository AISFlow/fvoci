//! Invitations: the links that let somebody into a workspace, as a member of it or as a
//! guest of one of its boards.
//!
//! A link carries a token, which is told once, to who makes the link, and never again:
//! the server keeps its digest.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::auth::NewAccount;
use crate::container::{ContainerKind, ContainerRole};
use crate::organization::WorkspaceRole;

/// How long a link lets somebody in after it was made: seven days, in milliseconds.
pub const INVITATION_LIFETIME_MILLIS: i64 = 7 * 24 * 60 * 60 * 1000;

/// How often the link to a board for a guest is used: once.
pub const GUEST_INVITATION_USES: i64 = 1;

/// A link that lets somebody into a workspace.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Invitation {
    /// The invitation's identifier, by which it is revoked. It lets nobody in.
    pub id: String,
    /// The workspace the link leads into.
    pub workspace_id: String,
    /// What whoever accepts becomes in the workspace: `member`, or `guest`.
    pub role: WorkspaceRole,
    /// For a guest: the board or the wiki they are invited to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container_id: Option<String>,
    /// For a guest: their role on that board, `viewer`, `commenter` or `editor`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<String>)]
    pub container_role: Option<ContainerRole>,
    /// The identifier of the user who made the link.
    pub created_by: String,
    /// When the link was made, in milliseconds since the Unix epoch.
    pub created_at: i64,
    /// When the link stops letting anybody in, in milliseconds since the Unix epoch.
    pub expires_at: i64,
    /// How often the link may be used; left out for one that is used as often as it is
    /// until it ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_uses: Option<i64>,
    /// How often the link was used.
    pub uses: i64,
    /// For a guest link made for one recipient of a mailed guest link: the link it was
    /// made like. Revoking that one revokes this one while nobody used it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    /// For such a link: the address it was mailed to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipient: Option<String>,
    /// The token of the link: in the answer that makes the link, and in no other.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

impl fmt::Debug for Invitation {
    /// Leaves out the token and the recipient's address.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Invitation")
            .field("id", &self.id)
            .field("workspace_id", &self.workspace_id)
            .field("role", &self.role)
            .field("container_id", &self.container_id)
            .field("container_role", &self.container_role)
            .field("created_by", &self.created_by)
            .field("expires_at", &self.expires_at)
            .field("max_uses", &self.max_uses)
            .field("uses", &self.uses)
            .field("parent_id", &self.parent_id)
            .finish_non_exhaustive()
    }
}

/// A request to make a link.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NewInvitation {
    /// What whoever accepts becomes in the workspace: `member`, or `guest`.
    pub role: WorkspaceRole,
    /// For a guest: the board or the wiki they are invited to. Not for a member.
    #[serde(default)]
    pub container_id: Option<String>,
    /// For a guest: their role on that board, `viewer`, `commenter` or `editor`; an
    /// editor if left out. Not for a member.
    #[serde(default)]
    #[schema(value_type = Option<String>)]
    pub container_role: Option<ContainerRole>,
}

/// A request that carries the token of a link and nothing else.
#[derive(Clone, PartialEq, Eq, Default, Deserialize, utoipa::ToSchema)]
#[serde(default)]
pub struct InvitationToken {
    /// The token of the link.
    pub token: String,
}

impl fmt::Debug for InvitationToken {
    /// Leaves out the token.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InvitationToken").finish_non_exhaustive()
    }
}

/// What a link shows of where it leads, to whoever holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InvitationPreview {
    /// The name of the workspace the link leads into; empty if it was given none.
    pub workspace_name: String,
    /// The name of who made the link: the name they are shown by, or their login name
    /// if they have none.
    pub inviter_name: String,
    /// For a guest: the title of the board or the wiki they are invited to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container_title: Option<String>,
    /// For a guest: which of the two it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container_kind: Option<ContainerKind>,
}

/// A request to accept a link.
#[derive(Clone, PartialEq, Eq, Default, Deserialize, utoipa::ToSchema)]
#[serde(default)]
pub struct AcceptInvitation {
    /// The token of the link.
    pub token: String,
    /// The account to make and let in, for a request without a session. A request with
    /// a session lets in the account of the session, and names none.
    pub account: Option<NewAccount>,
    /// The language tag of the screen the request comes from (`ko`, `en-GB`): the
    /// language of the mail that confirms a new account's address, where the server asks
    /// for that (English, Korean); English for any other. A new account's welcome
    /// board is written in it too, if it is English or Korean.
    pub language: String,
}

impl fmt::Debug for AcceptInvitation {
    /// Leaves out the token.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AcceptInvitation")
            .field("account", &self.account)
            .finish_non_exhaustive()
    }
}

/// Where an accepted link let somebody in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AcceptedInvitation {
    /// The workspace the link let them into.
    pub workspace_id: String,
    /// For a guest: the board or the wiki they were invited to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container_id: Option<String>,
    /// For a guest: which of the two it is, so that they are sent to its own address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container_kind: Option<ContainerKind>,
}

/// A request to email an existing invitation to one or more recipients.
#[derive(Clone, Deserialize, utoipa::ToSchema)]
pub struct InvitationMail {
    /// The secret of the existing invitation, sent only in the request body.
    pub token: String,
    /// Email addresses, each delivered separately; duplicates are sent once.
    pub recipients: Vec<String>,
    /// The sender's screen language, used for a recipient when neither the recipient's
    /// nor the sender's setting names a language the mail is written in.
    #[serde(default)]
    pub language: String,
}

impl fmt::Debug for InvitationMail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InvitationMail").finish_non_exhaustive()
    }
}
