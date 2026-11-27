//! What an organization's administrators see and change of it: the organization, its
//! members, its workspaces, and the audit log.

use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::organization::{OrgRole, WorkspaceRole};
use crate::person_name::PersonName;

/// An organization as its administrators see it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Organization {
    /// The organization's identifier.
    pub id: String,
    /// The organization's name; empty if it was given none.
    pub name: String,
    /// Whether its boards may be read by a public link. Off, every link is closed and
    /// none is made; on again, the links made before open again.
    pub public_links: bool,
    /// Whether members may copy boards, wikis, and courses out of this organization
    /// into another organization or into their personal space.
    #[serde(default = "default_true")]
    pub allow_copy_out: bool,
    /// Whether members may give its workspaces to personal API tokens, while it takes
    /// only sessions made through its provider. One that takes any session lets them
    /// whatever this says.
    #[serde(default)]
    pub allow_api_tokens: bool,
    /// The workspace somebody who joins the organization gets into. It cannot be
    /// deleted.
    pub default_workspace_id: String,
}

const fn default_true() -> bool {
    true
}

/// A change of an organization: what is left out stays as it is.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrganizationPatch {
    /// The new name, at most [`MAX_NAME_CHARS`] characters; may be empty.
    #[serde(default)]
    pub name: Option<String>,
    /// Whether boards may be read by a public link.
    #[serde(default)]
    pub public_links: Option<bool>,
    /// Whether members may copy containers out to external organizations or personal spaces.
    #[serde(default)]
    pub allow_copy_out: Option<bool>,
    /// Whether members may give its workspaces to personal API tokens while it takes
    /// only sessions made through its provider. Switched off, the tokens that reach
    /// them open nothing of it until it is switched on again.
    #[serde(default)]
    pub allow_api_tokens: Option<bool>,
}

/// The most characters a name of an organization or a workspace has.
pub const MAX_NAME_CHARS: usize = 100;

/// A member of an organization, as its administrators list them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OrgMember {
    /// The user's identifier.
    pub user_id: String,
    /// The user's login name.
    pub username: String,
    /// The name the user shows; empty for none.
    pub display_name: String,
    /// The parts of the user's name, and their title and credentials: the
    /// administrators see the user's name.
    #[serde(flatten)]
    pub name: PersonName,
    /// What the user is in the organization.
    pub role: OrgRole,
}

/// The role a member of an organization is given: the body of a change of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrgRoleChange {
    /// The role the member is to have from now on.
    pub role: OrgRole,
}

/// A workspace of an organization as its administrators list it: whether or not they
/// are in it, and whether or not it is in the trash. Nothing of what is in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OrgWorkspace {
    /// The workspace's identifier.
    pub id: String,
    /// The workspace's name; empty if it was given none.
    pub name: String,
    /// How many members it has, its guests among them.
    pub member_count: i64,
    /// Whether it is the organization's default workspace, which cannot be deleted.
    pub is_default: bool,
    /// When it was put in the trash, in milliseconds since the epoch; absent while it is
    /// in use.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::serde_util::positive_millis"
    )]
    #[schema(nullable = false)]
    pub deleted_at: Option<i64>,
    /// What the administrator who asks is in it; `null` for no member.
    pub role: Option<WorkspaceRole>,
    /// Whether the administrator who asks is in it as the organization's administrator,
    /// having joined it as one: leaving gives back what they were before.
    pub joined_as_org_admin: bool,
}

/// The name of a workspace: the body of making one and of renaming one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceName {
    /// The name, 1 to [`MAX_NAME_CHARS`] characters, not only spaces. Spaces around it
    /// are dropped. One workspace in use of the organization has it.
    pub name: String,
}

/// What a record of the audit log says was done.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuditAction {
    /// An administrator of a workspace made themselves a member of a board whose
    /// content they could not read before. The target is the board.
    SelfAddedToContainer,
    /// An administrator of a workspace who could not read a board made somebody else
    /// (the subject) a member of it. The target is the board.
    AddedToContainer,
    /// An administrator of the organization joined the workspace as its administrator.
    /// The target is the workspace.
    OrgAdminJoined,
    /// An administrator of the organization who had joined the workspace left it.
    OrgAdminLeft,
    /// Who had joined the workspace as the organization's administrator (the subject)
    /// was no administrator of it any more, and was taken out: the actor made them
    /// none.
    OrgAdminRevoked,
    /// A member of the organization (the subject) was made an administrator of it.
    /// The target is the organization.
    OrgAdminGranted,
    /// An administrator of the organization (the subject) was made a member. The
    /// target is the organization.
    OrgAdminRemoved,
    /// A member of the organization (the subject) was taken out of it by an
    /// administrator. The target is the organization.
    OrgMemberRemoved,
    /// A workspace (the target) was made.
    WorkspaceCreated,
    /// A workspace (the target) was renamed.
    WorkspaceRenamed,
    /// A workspace (the target) was put in the trash.
    WorkspaceDeleted,
    /// A workspace (the target) was brought back from the trash.
    WorkspaceRestored,
    /// Public links were turned off for the organization (the target).
    PublicLinksDisabled,
    /// Public links were turned on for the organization (the target).
    PublicLinksEnabled,
    /// The organization's provider settings were stored.
    OidcProviderConfigured,
    /// The organization's provider was removed.
    OidcProviderRemoved,
    /// The organization began requiring its provider's sign-in.
    SsoRequired,
    /// The organization began accepting password sessions again.
    SsoReleased,
    /// Settings of the provider's sign-in were saved aside while the organization
    /// takes no password, to be tried before they are taken.
    OidcProviderPendingSaved,
    /// The pending settings became the provider's, by a sign-in through them that
    /// ended well: the actor is who signed in.
    OidcProviderPendingApplied,
    /// The pending settings were thrown away.
    OidcProviderPendingDiscarded,
    /// The server operator made a link to set the password of a member (the subject)
    /// anew. The target is the organization; one is written in each organization
    /// the person is in.
    PasswordResetLinkMade,
    /// The server operator set the password of a member (the subject). The target is
    /// the organization; one is written in each organization the person is in.
    PasswordSetByOperator,
    /// The provider's issuer changed, and the provider before (`detail` is its
    /// issuer) is kept for a while, for its people to carry their accounts over.
    OidcFormerProviderKept,
    /// An administrator ended the keeping of the former provider (`detail` is its
    /// issuer) before its time.
    OidcFormerProviderEnded,
    /// The time the former provider (`detail` is its issuer) was kept for ran out:
    /// the server's own doing.
    OidcFormerProviderLapsed,
    /// The trial that made the pending settings the provider's tied who the new
    /// issuer (`detail`) vouches for (`identity`) to the account of the administrator
    /// who tried them (the actor and the subject), who had signed in through the
    /// provider before a moment earlier.
    OidcIdentityBound,
    /// Somebody with a place in the organization (the actor and the subject) carried
    /// their account over: who the new issuer (`detail`) vouches for (`identity`) is
    /// tied to the account the former provider vouched for.
    OidcIdentityCarriedOver,
    /// The server operator confirmed the email address of a registration made by a
    /// link for members, which made its account (the subject) a member. The target is
    /// the organization it joined.
    EmailVerifiedByOperator,
    /// A member copied a container (the target) out of the organization into another
    /// organization or their own personal space (`detail` names the target org kind).
    ContainerCopiedOut,
    /// Copying containers out of the organization was disabled.
    CopyOutDisabled,
    /// Copying containers out of the organization was enabled.
    CopyOutEnabled,
    /// The organization, which takes only sessions made through its provider, began
    /// giving its workspaces to personal API tokens (`allowApiTokens`).
    ApiTokensEnabled,
    /// The organization stopped giving its workspaces to personal API tokens.
    ApiTokensDisabled,
    /// An administrator took a personal API token of a member (the subject) out of the
    /// organization (the target): it reaches none of its workspaces any more. `detail`
    /// is the characters the token is shown by.
    ApiTokenRevoked,
}

impl AuditAction {
    /// Every action, in the order of the declaration.
    pub const ALL: [Self; 35] = [
        Self::SelfAddedToContainer,
        Self::AddedToContainer,
        Self::OrgAdminJoined,
        Self::OrgAdminLeft,
        Self::OrgAdminRevoked,
        Self::OrgAdminGranted,
        Self::OrgAdminRemoved,
        Self::OrgMemberRemoved,
        Self::WorkspaceCreated,
        Self::WorkspaceRenamed,
        Self::WorkspaceDeleted,
        Self::WorkspaceRestored,
        Self::PublicLinksDisabled,
        Self::PublicLinksEnabled,
        Self::OidcProviderConfigured,
        Self::OidcProviderRemoved,
        Self::SsoRequired,
        Self::SsoReleased,
        Self::OidcProviderPendingSaved,
        Self::OidcProviderPendingApplied,
        Self::OidcProviderPendingDiscarded,
        Self::PasswordResetLinkMade,
        Self::PasswordSetByOperator,
        Self::OidcFormerProviderKept,
        Self::OidcFormerProviderEnded,
        Self::OidcFormerProviderLapsed,
        Self::OidcIdentityBound,
        Self::OidcIdentityCarriedOver,
        Self::EmailVerifiedByOperator,
        Self::ContainerCopiedOut,
        Self::CopyOutDisabled,
        Self::CopyOutEnabled,
        Self::ApiTokensEnabled,
        Self::ApiTokensDisabled,
        Self::ApiTokenRevoked,
    ];

    /// The wire and database form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SelfAddedToContainer => "self_added_to_container",
            Self::AddedToContainer => "added_to_container",
            Self::OrgAdminJoined => "org_admin_joined",
            Self::OrgAdminLeft => "org_admin_left",
            Self::OrgAdminRevoked => "org_admin_revoked",
            Self::OrgAdminGranted => "org_admin_granted",
            Self::OrgAdminRemoved => "org_admin_removed",
            Self::OrgMemberRemoved => "org_member_removed",
            Self::WorkspaceCreated => "workspace_created",
            Self::WorkspaceRenamed => "workspace_renamed",
            Self::WorkspaceDeleted => "workspace_deleted",
            Self::WorkspaceRestored => "workspace_restored",
            Self::PublicLinksDisabled => "public_links_disabled",
            Self::PublicLinksEnabled => "public_links_enabled",
            Self::OidcProviderConfigured => "oidc_provider_configured",
            Self::OidcProviderRemoved => "oidc_provider_removed",
            Self::SsoRequired => "sso_required",
            Self::SsoReleased => "sso_released",
            Self::OidcProviderPendingSaved => "oidc_provider_pending_saved",
            Self::OidcProviderPendingApplied => "oidc_provider_pending_applied",
            Self::OidcProviderPendingDiscarded => "oidc_provider_pending_discarded",
            Self::PasswordResetLinkMade => "password_reset_link_made",
            Self::PasswordSetByOperator => "password_set_by_operator",
            Self::OidcFormerProviderKept => "oidc_former_provider_kept",
            Self::OidcFormerProviderEnded => "oidc_former_provider_ended",
            Self::OidcFormerProviderLapsed => "oidc_former_provider_lapsed",
            Self::OidcIdentityBound => "oidc_identity_bound",
            Self::OidcIdentityCarriedOver => "oidc_identity_carried_over",
            Self::EmailVerifiedByOperator => "email_verified_by_operator",
            Self::ContainerCopiedOut => "container_copied_out",
            Self::CopyOutDisabled => "copy_out_disabled",
            Self::CopyOutEnabled => "copy_out_enabled",
            Self::ApiTokensEnabled => "api_tokens_enabled",
            Self::ApiTokensDisabled => "api_tokens_disabled",
            Self::ApiTokenRevoked => "api_token_revoked",
        }
    }
}

/// Text that names no action of the audit log.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0:?} is no audit action")]
pub struct UnknownAuditAction(pub String);

impl FromStr for AuditAction {
    type Err = UnknownAuditAction;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|action| action.as_str() == text)
            .ok_or_else(|| UnknownAuditAction(text.to_owned()))
    }
}

/// A user as a record of the audit log names them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuditPerson {
    /// The user's identifier.
    pub id: String,
    /// The user's login name; empty if the account is gone.
    pub username: String,
    /// The name the user shows; empty for none.
    pub display_name: String,
}

/// Who did what a record of the audit log says: a person by their account, named as it
/// is now (an account that is gone by its identifier alone), the server's operator,
/// who acts through the local administration socket without one, or the server
/// itself, for what its timers do. `kind` says which.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuditRecordActor {
    /// A person, whose identifier stays in the record when the account goes.
    Person(AuditPerson),
    /// The server's operator.
    ServerOperator,
    /// The server itself: what ends when its time runs out.
    Server,
}

/// One record of the audit log: who did what, to what and whom, and when.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuditRecord {
    /// The record's identifier.
    pub id: String,
    /// What was done.
    pub action: AuditAction,
    /// Who did it.
    pub actor: AuditRecordActor,
    /// Whom it was done to, for an action that names somebody; `null` otherwise.
    pub subject: Option<AuditPerson>,
    /// What it was done to: a board, a workspace or the organization.
    pub target_id: String,
    /// The name of the target, where the reader may know it: the title of a board
    /// they see, the name of a workspace. `null` otherwise.
    pub target_name: Option<String>,
    /// When, in milliseconds since the epoch.
    pub created_at: i64,
    /// The issuer a record of a provider's move names (`oidc_former_provider_…`,
    /// `oidc_identity_…`); `null` for the others.
    pub detail: Option<String>,
    /// Who the identity a record of `oidc_identity_bound` or
    /// `oidc_identity_carried_over` tied is: the email address its provider gave, or
    /// its subject where it gave none, as it was then. `null` for the others, and in
    /// the records of a workspace: the organization's are read by its administrators
    /// alone.
    pub identity: Option<String>,
}

impl std::fmt::Debug for AuditRecord {
    /// Leaves out who an identity it tied is: an address, kept out of the logs.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuditRecord")
            .field("id", &self.id)
            .field("action", &self.action)
            .field("actor", &self.actor)
            .field("subject", &self.subject)
            .field("target_id", &self.target_id)
            .field("target_name", &self.target_name)
            .field("created_at", &self.created_at)
            .field("detail", &self.detail)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_action_reads_back_from_its_text_and_nothing_else_does() {
        for action in AuditAction::ALL {
            assert_eq!(action.as_str().parse(), Ok(action), "{action:?}");
            assert_eq!(
                serde_json::to_value(action).expect("an action is JSON"),
                serde_json::Value::from(action.as_str()),
                "{action:?}"
            );
        }
        assert!("joined".parse::<AuditAction>().is_err());
    }
}
