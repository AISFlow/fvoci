//! Who may do what on a board, as a function of facts and nothing else.
//!
//! What a user may do on a board follows from three facts: what the user is in the
//! board's workspace and organization ([`Actor`]), the board's scope, and the user's
//! role on the board ([`ContainerFacts`]). [`grant`] turns them into a [`Grant`]. Nothing
//! here reads the database: `App::actor` (what `Store::resolve_workspace` answers) and `App::container_access`
//! are where the facts are read, and every check of a route goes through them.
//!
//! The table of `permissions/table.rs` pins every rule of this file, one row per
//! sentence of what the product promises.

use fvoci_model::{
    Container, ContainerKind, ContainerMember, ContainerPermissions, ContainerRole,
    ContainerVisibility, OrgRole, Workspace, WorkspaceRole,
};
use fvoci_store::{Looking, Reach};

#[cfg(test)]
mod table;
#[cfg(test)]
mod team_table;
#[cfg(test)]
mod wiki_table;
#[cfg(test)]
mod workspace_table;

/// What a user is where a board lives: in the board's organization and in its
/// workspace. A user who is no member of the workspace has no `workspace` role, and for
/// that user the workspace and everything in it is not there, whatever else is true.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Actor {
    /// What the user is in the workspace's organization; `None` for somebody who is no
    /// member of it. It gives nothing on a board: an organization's administrators
    /// manage the organization, not what its workspaces hold.
    pub org: Option<OrgRole>,
    /// What the user is in the board's workspace; `None` for somebody who is no member
    /// of it, among them who was one and was removed.
    pub workspace: Option<WorkspaceRole>,
}

impl Actor {
    /// Whether the user works in the workspace: a member of it who is no guest. A guest
    /// is on the boards they were invited to, and sees nothing else of the workspace.
    #[must_use]
    pub fn works_there(self) -> bool {
        matches!(
            self.workspace,
            Some(WorkspaceRole::Admin | WorkspaceRole::Member)
        )
    }

    /// Whether the user administers the organization: what the routes of an
    /// organization (`/org/{orgId}`) take. It opens nothing of its workspaces' boards.
    #[must_use]
    pub fn administers_the_organization(self) -> bool {
        matches!(self.org, Some(OrgRole::Admin))
    }
}

/// Something a member of a workspace can be allowed to do in it, beside what is on its
/// boards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspacePermission {
    /// Make a link that lets somebody in as a member of the workspace, and see and
    /// revoke the links others made.
    InviteMembers,
    /// Turn on and off whether the admins of the workspace's boards invite guests to
    /// them.
    SwitchGuestInvitations,
    /// List the members of the workspace, change what one of them is in it, and
    /// remove one.
    ManageMembers,
}

impl WorkspacePermission {
    /// Every permission, in the order of the declaration.
    pub const ALL: [Self; 3] = [
        Self::InviteMembers,
        Self::SwitchGuestInvitations,
        Self::ManageMembers,
    ];
}

/// Whether the user may do this in the workspace: its administrators invite its
/// members and say whether guests are invited. What the user is in the organization
/// adds nothing.
#[must_use]
pub const fn allows_in_workspace(actor: Actor, permission: WorkspacePermission) -> bool {
    match permission {
        WorkspacePermission::InviteMembers
        | WorkspacePermission::SwitchGuestInvitations
        | WorkspacePermission::ManageMembers => {
            matches!(actor.workspace, Some(WorkspaceRole::Admin))
        }
    }
}

/// Whether a member of a workspace who is `from` there is made `to` by a change of
/// their role: an administrator a member and a member an administrator, and a member a
/// guest. A guest becomes a member by an invitation for members, and an administrator
/// a guest in two steps. What a member is already they may be made again.
#[must_use]
pub const fn may_become(from: WorkspaceRole, to: WorkspaceRole) -> bool {
    matches!(
        (from, to),
        (
            WorkspaceRole::Admin,
            WorkspaceRole::Admin | WorkspaceRole::Member
        ) | (WorkspaceRole::Member, _)
            | (WorkspaceRole::Guest, WorkspaceRole::Guest)
    )
}

/// Whether a user who may do `grant` on a board of `scope` invites guests to it:
/// whoever adds members to the board (its admins, and the administrators of its
/// workspace), while the workspace lets guests be invited to its boards. A template
/// takes no guests.
#[must_use]
pub const fn invites_guests(grant: Grant, scope: ContainerScope, workspace: &Workspace) -> bool {
    match scope {
        ContainerScope::InvitedOnly | ContainerScope::OpenToRead => {
            workspace.guest_invitations && grant.allows(ContainerPermission::ManageRoles)
        }
        ContainerScope::BuiltIn
        | ContainerScope::InvitedOnlyTemplate
        | ContainerScope::OpenTemplate => false,
    }
}

/// Who a board is there for, besides its members.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerScope {
    /// An open board, wiki, or course: whoever works in the workspace reads and searches
    /// it without joining, and joins it to write or enroll.
    OpenToRead,
    /// A board for the invited only: nobody but its members knows it is there.
    InvitedOnly,
    /// An open template of the workspace: whoever works there reads it, to copy it.
    OpenTemplate,
    /// A template for the invited only, as [`ContainerScope::InvitedOnly`].
    InvitedOnlyTemplate,
    /// A template the server ships with: every user's to read and copy.
    BuiltIn,
}

impl ContainerScope {
    /// The scope of a board as it is stored.
    #[must_use]
    pub fn of(container: &Container) -> Self {
        match (
            container.is_builtin,
            container.is_template,
            container.visibility,
        ) {
            (true, _, _) => Self::BuiltIn,
            (false, false, ContainerVisibility::Open) => match container.kind {
                ContainerKind::Board | ContainerKind::Wiki | ContainerKind::Course => {
                    Self::OpenToRead
                }
                // No diary is stored as open (`Container::check_storable`); one that
                // were would be nobody's but its members' all the same.
                ContainerKind::Diary => Self::InvitedOnly,
            },
            (false, false, ContainerVisibility::Private) => Self::InvitedOnly,
            (false, true, ContainerVisibility::Open) => Self::OpenTemplate,
            (false, true, ContainerVisibility::Private) => Self::InvitedOnlyTemplate,
        }
    }

    /// The scope of the board once it is in the trash: open to nobody but who was its
    /// member. The trash is no list of the workspace's boards.
    #[must_use]
    pub fn in_the_trash(self) -> Self {
        match self {
            Self::OpenToRead | Self::InvitedOnly => Self::InvitedOnly,
            Self::OpenTemplate | Self::InvitedOnlyTemplate => Self::InvitedOnlyTemplate,
            Self::BuiltIn => Self::BuiltIn,
        }
    }

    /// Whether whoever works in the workspace sees the board without being its member.
    fn open_to_the_workspace(self) -> bool {
        matches!(self, Self::OpenToRead | Self::OpenTemplate)
    }
}

/// What is known of a board for one user: its scope and the user's role on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContainerFacts {
    /// Who the board is there for besides its members.
    pub scope: ContainerScope,
    /// The user's role on the board, as stored; `None` for who is no member.
    pub role: Option<ContainerRole>,
    /// The highest team role; ignored for guests and capped at editor.
    pub team_role: Option<ContainerRole>,
}

/// Something a user can be allowed to do on a board.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerPermission {
    /// Know that the board is there: read the board itself, find it in a list. Who may
    /// not is told that the board does not exist; who may and is refused something
    /// else is told it is refused.
    See,
    /// Become a member of an open board with its minimum role.
    Join,
    /// Read what is on the board: its cards, blocks, files and comments, its archive.
    /// Copying a board takes this too.
    View,
    /// List the board's members.
    ListMembers,
    /// Comment on cards.
    CommentCards,
    /// Create, change and delete cards and their content.
    ManageCards,
    /// Change the board's title, properties and views.
    ManageProperties,
    /// Change the board between open and private, and its minimum role.
    ManageType,
    /// Add and remove members and change their roles.
    ManageRoles,
    /// Delete the board, and bring it back from the trash.
    Delete,
    /// Turn public sharing on and off.
    Share,
    /// Change and delete comments written by others.
    ManageOthersComments,
}

impl ContainerPermission {
    /// Every permission, in the order of the declaration.
    pub const ALL: [Self; 12] = [
        Self::See,
        Self::Join,
        Self::View,
        Self::ListMembers,
        Self::CommentCards,
        Self::ManageCards,
        Self::ManageProperties,
        Self::ManageType,
        Self::ManageRoles,
        Self::Delete,
        Self::Share,
        Self::ManageOthersComments,
    ];

    /// The bit of the permission in a [`Grant`].
    const fn bit(self) -> u16 {
        let place = match self {
            Self::See => 0,
            Self::Join => 1,
            Self::View => 2,
            Self::CommentCards => 3,
            Self::ManageCards => 4,
            Self::ManageProperties => 5,
            Self::ManageType => 6,
            Self::ManageRoles => 7,
            Self::Delete => 8,
            Self::Share => 9,
            Self::ManageOthersComments => 10,
            Self::ListMembers => 11,
        };
        1 << place
    }

    /// Whether the permission is over what is on the board: its cards, comments and
    /// files, and the link that shows them. Refused it, who only manages the board is
    /// told that it is not there ([`Grant::hides`]).
    #[must_use]
    pub const fn is_content(self) -> bool {
        matches!(
            self,
            Self::View
                | Self::CommentCards
                | Self::ManageCards
                | Self::ManageOthersComments
                | Self::Share
        )
    }
}

/// The bit of a [`Grant`] that says the user sees the board only as one who manages it:
/// an administrator of the workspace on a board for the invited only that they are no
/// reader of. No permission has it.
const MANAGES_ONLY: u16 = 1 << 15;

/// What one user may do on one board.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[must_use]
pub struct Grant(u16);

impl Grant {
    /// Nothing at all: the board is not there for the user.
    pub const NONE: Self = Self(0);

    /// The grant of exactly these permissions.
    const fn of(permissions: &[ContainerPermission]) -> Self {
        let mut bits = 0;
        let mut index = 0;
        while index < permissions.len() {
            bits |= permissions[index].bit();
            index += 1;
        }
        Self(bits)
    }

    /// This grant with those of `other` added.
    const fn with(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Whether the grant has the permission.
    #[must_use]
    pub const fn allows(self, permission: ContainerPermission) -> bool {
        self.0 & permission.bit() != 0
    }

    /// Whether the user knows the board is there: refused something, they are told so
    /// (403); otherwise the board does not exist for them (404).
    #[must_use]
    pub const fn sees(self) -> bool {
        self.allows(ContainerPermission::See)
    }

    /// Whether a refusal of `permission` tells that the board is not there (404) rather
    /// than that it is refused (403): for who does not see the board, and for who only
    /// manages it and asks for its content.
    #[must_use]
    pub const fn hides(self, permission: ContainerPermission) -> bool {
        !self.sees() || (self.manages_only() && permission.is_content())
    }

    /// Whether the user sees the board only to manage it: an administrator of the
    /// workspace who reads none of it. What is on it, and what the board says of itself
    /// beyond its name and kind, is not for them.
    #[must_use]
    pub const fn manages_only(self) -> bool {
        self.0 & MANAGES_ONLY != 0
    }

    /// Whether the user reads what is on the board: refused something reached through
    /// a card or a block, they are told so (403); otherwise that card or block does not
    /// exist for them (404).
    #[must_use]
    pub const fn reads(self) -> bool {
        self.allows(ContainerPermission::View)
    }

    /// Whether the user reads what is on the board, or may join it to read it: whoever
    /// works in the workspace of an open board they have not joined. What happens on the
    /// board is told to them, and watched by them, as to a reader: what they are told of
    /// they may read by joining.
    #[must_use]
    pub const fn reads_on_joining(self) -> bool {
        self.reads() || self.allows(ContainerPermission::Join)
    }

    /// Projects this caller-specific grant into the wire [`ContainerPermissions`] flags.
    #[must_use]
    pub const fn to_container_permissions(self) -> ContainerPermissions {
        ContainerPermissions {
            can_edit: self.allows(ContainerPermission::ManageCards),
            can_comment: self.allows(ContainerPermission::CommentCards),
            can_manage_properties: self.allows(ContainerPermission::ManageProperties),
            can_manage_visibility: self.allows(ContainerPermission::ManageType),
            can_manage_roles: self.allows(ContainerPermission::ManageRoles),
            can_delete: self.allows(ContainerPermission::Delete),
            can_share: self.allows(ContainerPermission::Share),
            can_manage_others_comments: self.allows(ContainerPermission::ManageOthersComments),
        }
    }
}

/// How a user is in the room of a card's body that is edited together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocMode {
    /// The user receives what the others write; what they send is refused.
    Read,
    /// The user writes as well.
    Write,
}

impl DocMode {
    /// The value of the member `mode` of the websocket's messages.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
        }
    }
}

/// How the holder of `grant` on a card's board enters the room of the card's body:
/// writing for who may change the board's cards, reading for who reads the board, and
/// not at all (`None`) for anybody else, who is told what one is told of a card that
/// is not there. A read token of a shared board is no grant: its readers stay outside.
#[must_use]
pub const fn doc_mode(grant: Grant) -> Option<DocMode> {
    if grant.allows(ContainerPermission::ManageCards) {
        Some(DocMode::Write)
    } else if grant.reads() {
        Some(DocMode::Read)
    } else {
        None
    }
}

/// What a user does to a comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentAction<'a> {
    /// Change it.
    Change,
    /// Delete it.
    Delete,
    /// Bring it back from the trash, where it went by the deletion of `deleted_by`
    /// (empty if nobody's).
    Undelete {
        /// Who deleted it.
        deleted_by: &'a str,
    },
}

/// The permission doing `action` to a comment of `author_id` takes of `user_id`: a
/// comment is its author's to change and delete, and to bring back if they deleted it
/// themselves ([`ContainerPermission::CommentCards`]); a comment of somebody else, and
/// one of their own that somebody else deleted, is the board's admins'
/// ([`ContainerPermission::ManageOthersComments`]).
#[must_use]
pub fn comment_permission(
    user_id: &str,
    author_id: &str,
    action: CommentAction<'_>,
) -> ContainerPermission {
    let undoes_their_own = match action {
        CommentAction::Change | CommentAction::Delete => true,
        CommentAction::Undelete { deleted_by } => deleted_by == user_id,
    };
    if author_id == user_id && undoes_their_own {
        ContainerPermission::CommentCards
    } else {
        ContainerPermission::ManageOthersComments
    }
}

/// What is known of one user (the person) for another who looks at them in a
/// workspace the seer is a member of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Person {
    /// The seer.
    Themselves,
    /// A member of the workspace now.
    Member {
        /// The two hold roles on a board of the workspace.
        shares_a_container: bool,
        /// The person authored the board, a block, or a kept comment version that
        /// the seer reads.
        wrote_where_seer_reads: bool,
    },
    /// No member of the workspace: who left it, with what they wrote and any role
    /// left on its boards, or never was in it.
    Outside {
        /// As for a member.
        wrote_where_seer_reads: bool,
    },
}

/// Whether the seer, `seer` in the workspace, sees the person, `looking` for what:
/// whoever works there sees its members, a guest the members who share a board with it.
/// Who left the workspace is seen by name only, and only where they wrote on a board
/// the seer reads, so that what they wrote keeps its author; a role left on a board
/// shows nobody. The store's queries of people are this rule in SQL
/// (`fvoci_store::Sight`).
#[must_use]
pub const fn sees_person(seer: WorkspaceRole, person: Person, looking: Looking) -> bool {
    let by_name = matches!(looking, Looking::ByName);
    match person {
        Person::Themselves => true,
        Person::Member {
            shares_a_container,
            wrote_where_seer_reads,
        } => {
            !matches!(seer, WorkspaceRole::Guest)
                || shares_a_container
                || (by_name && wrote_where_seer_reads)
        }
        Person::Outside {
            wrote_where_seer_reads,
        } => by_name && wrote_where_seer_reads,
    }
}

/// What a role on a board lets its holder do, each role all that the one below it may.
const fn role_grant(role: ContainerRole) -> Grant {
    use ContainerPermission::{
        CommentCards, Delete, ListMembers, ManageCards, ManageOthersComments, ManageProperties,
        ManageRoles, ManageType, See, Share, View,
    };
    let viewer = Grant::of(&[See, View, ListMembers]);
    let commenter = viewer.with(Grant::of(&[CommentCards]));
    let editor = commenter.with(Grant::of(&[ManageCards, ManageProperties]));
    match role {
        ContainerRole::Viewer => viewer,
        ContainerRole::Commenter => commenter,
        ContainerRole::Editor => editor,
        ContainerRole::Admin => editor.with(Grant::of(&[
            ManageType,
            ManageRoles,
            Delete,
            Share,
            ManageOthersComments,
        ])),
    }
}

/// The highest role a member of a workspace may have on its boards: a guest may edit,
/// and no more.
#[must_use]
pub const fn highest_container_role(workspace_role: WorkspaceRole) -> ContainerRole {
    match workspace_role {
        WorkspaceRole::Admin | WorkspaceRole::Member => ContainerRole::Admin,
        WorkspaceRole::Guest => ContainerRole::Editor,
    }
}

/// Whether a member of a workspace in `workspace_role` may be given `role` on one of
/// its boards: a guest no role above editor.
#[must_use]
pub fn may_hold(workspace_role: WorkspaceRole, role: ContainerRole) -> bool {
    role <= highest_container_role(workspace_role)
}

/// Whom a member of a workspace, in `workspace_role`, sees of its people in lists,
/// searches and profiles: whoever works there sees its members, and a guest who is on
/// a board with them.
#[must_use]
pub const fn people_reach(workspace_role: WorkspaceRole) -> Reach {
    match workspace_role {
        WorkspaceRole::Admin | WorkspaceRole::Member => Reach::Workspace,
        WorkspaceRole::Guest => Reach::SharedContainers,
    }
}

/// What the user may do on the board.
///
/// - A user who is in no role in the board's workspace may do nothing: a role left on
///   a board after leaving the workspace opens nothing. A template the server ships
///   with is read and copied by everyone who works in the workspace.
/// - A member of the board may do what the role lets them, a guest of the workspace no
///   more than an editor.
/// - Whoever works in the workspace sees its open boards and may join those that are
///   no template, and reads its open templates. An open wiki they read and may join
///   without joining it; only joined do they write in it.
/// - An administrator of the workspace sees its boards, lists and changes their
///   members, and deletes and restores them; what is on a board for the invited only
///   they read as its member, and are told it is not there otherwise
///   ([`Grant::hides`]).
/// - What the user is in the organization adds nothing.
pub fn grant(actor: Actor, facts: ContainerFacts) -> Grant {
    use ContainerPermission::{Delete, Join, ListMembers, ManageRoles, See, View};
    if facts.scope == ContainerScope::BuiltIn {
        return if actor.works_there() {
            Grant::of(&[See, View, ListMembers])
        } else {
            Grant::NONE
        };
    }
    let Some(workspace_role) = actor.workspace else {
        return Grant::NONE;
    };
    let team_role = actor
        .works_there()
        .then_some(facts.team_role)
        .flatten()
        .map(|role| role.min(ContainerRole::Editor));
    let as_member = facts.role.max(team_role).map_or(Grant::NONE, |role| {
        role_grant(role.min(highest_container_role(workspace_role)))
    });
    let as_worker = match (actor.works_there(), facts.scope) {
        (true, ContainerScope::OpenToRead) => Grant::of(&[See, Join, View, ListMembers]),
        (true, ContainerScope::OpenTemplate) => Grant::of(&[See, View, ListMembers]),
        _ => Grant::NONE,
    };
    let as_manager = if workspace_role == WorkspaceRole::Admin {
        Grant::of(&[See, ListMembers, ManageRoles, Delete])
    } else {
        Grant::NONE
    };
    let granted = as_member.with(as_worker).with(as_manager);
    if as_manager != Grant::NONE && !granted.reads() && !facts.scope.open_to_the_workspace() {
        Grant(granted.0 | MANAGES_ONLY)
    } else {
        granted
    }
}

/// Whether a member of a workspace in `role` is told the changes of one of its boards
/// of `scope` that they hold no role on: of an open wiki, whoever works there, who reads
/// it without joining it. An open template is read to be copied: what changes on it is
/// told to its members, as on any board.
#[must_use]
pub fn told_without_a_role(scope: ContainerScope, role: WorkspaceRole) -> bool {
    let actor = Actor {
        org: None,
        workspace: Some(role),
    };
    let facts = ContainerFacts {
        scope,
        role: None,
        team_role: None,
    };
    scope == ContainerScope::OpenToRead && grant(actor, facts).reads()
}

/// Whether a user who is `role` in a workspace (`None`: no member) reads its audit log:
/// whoever works there, an administrator of the organization who joined it among them.
/// A guest does not: what the log names is the workspace's, not a board's.
#[must_use]
pub const fn reads_the_audit_log(role: Option<WorkspaceRole>) -> bool {
    matches!(role, Some(WorkspaceRole::Admin | WorkspaceRole::Member))
}

/// The membership with `role` as its direct role.
#[must_use]
pub fn with_role(member: ContainerMember, role: ContainerRole) -> ContainerMember {
    member.with_role(role)
}

/// The effective role a membership grants: the higher of its direct `role` and any
/// `inherited_role`.
#[must_use]
pub fn role_of(member: &ContainerMember) -> ContainerRole {
    member.effective_role()
}
