//! The table of who may do what: roles × scopes × permissions. Each row is one case of
//! what the product promises, and the whole grant of the case is pinned: a permission a
//! row does not name is one the user does not have.

use fvoci_model::{ContainerRole, OrgRole, WorkspaceRole};

use super::ContainerPermission::{
    CommentCards, Delete, Join, ListMembers, ManageCards, ManageOthersComments, ManageProperties,
    ManageRoles, ManageType, See, Share, View,
};
use super::ContainerScope::{BuiltIn, InvitedOnly, InvitedOnlyTemplate, OpenTemplate, OpenToRead};
use super::{
    Actor, ContainerFacts, ContainerPermission, ContainerScope, DocMode, Grant, doc_mode, grant,
    highest_container_role,
};

const VIEWER: &[ContainerPermission] = &[See, View, ListMembers];
const COMMENTER: &[ContainerPermission] = &[See, View, ListMembers, CommentCards];
const EDITOR: &[ContainerPermission] = &[
    See,
    View,
    ListMembers,
    CommentCards,
    ManageCards,
    ManageProperties,
];
const ADMIN: &[ContainerPermission] = &[
    See,
    View,
    ListMembers,
    CommentCards,
    ManageCards,
    ManageProperties,
    ManageType,
    ManageRoles,
    Delete,
    Share,
    ManageOthersComments,
];
const NOTHING: &[ContainerPermission] = &[];
/// What a workspace's administrator does with a board they have no role on.
const MANAGE: &[ContainerPermission] = &[See, ListMembers, ManageRoles, Delete];

const fn actor(org: Option<OrgRole>, workspace: Option<WorkspaceRole>) -> Actor {
    Actor { org, workspace }
}

pub(super) const MEMBER: Actor = actor(Some(OrgRole::Member), Some(WorkspaceRole::Member));
pub(super) const WORKSPACE_ADMIN: Actor = actor(Some(OrgRole::Member), Some(WorkspaceRole::Admin));
pub(super) const GUEST: Actor = actor(Some(OrgRole::Member), Some(WorkspaceRole::Guest));
pub(super) const OUTSIDE_GUEST: Actor = actor(None, Some(WorkspaceRole::Guest));
pub(super) const ORG_ADMIN_ONLY: Actor = actor(Some(OrgRole::Admin), None);
pub(super) const ORG_ADMIN_IN_WORKSPACE: Actor =
    actor(Some(OrgRole::Admin), Some(WorkspaceRole::Member));
/// An organization's administrator who joined the workspace as its administrator.
const ORG_ADMIN_JOINED: Actor = actor(Some(OrgRole::Admin), Some(WorkspaceRole::Admin));
pub(super) const REMOVED: Actor = actor(Some(OrgRole::Member), None);
pub(super) const STRANGER: Actor = actor(None, None);

/// One case: who, on what board, with which role there, may do exactly this.
struct Row {
    case: &'static str,
    actor: Actor,
    scope: ContainerScope,
    role: Option<ContainerRole>,
    allowed: &'static [ContainerPermission],
}

const fn row(
    case: &'static str,
    actor: Actor,
    scope: ContainerScope,
    role: Option<ContainerRole>,
    allowed: &'static [ContainerPermission],
) -> Row {
    Row {
        case,
        actor,
        scope,
        role,
        allowed,
    }
}

const ROWS: &[Row] = &[
    // The roles of a board, for a member of its workspace.
    row(
        "a viewer views",
        MEMBER,
        InvitedOnly,
        Some(ContainerRole::Viewer),
        VIEWER,
    ),
    row(
        "a commenter comments",
        MEMBER,
        InvitedOnly,
        Some(ContainerRole::Commenter),
        COMMENTER,
    ),
    row(
        "an editor edits",
        MEMBER,
        InvitedOnly,
        Some(ContainerRole::Editor),
        EDITOR,
    ),
    row(
        "an admin manages",
        MEMBER,
        InvitedOnly,
        Some(ContainerRole::Admin),
        ADMIN,
    ),
    row(
        "a role is the same on an open board",
        MEMBER,
        OpenToRead,
        Some(ContainerRole::Viewer),
        &[See, View, ListMembers, Join],
    ),
    // A workspace is unknown to who is no member of it.
    row(
        "a stranger: no invited-only board",
        STRANGER,
        InvitedOnly,
        None,
        NOTHING,
    ),
    row(
        "a stranger: no open board",
        STRANGER,
        OpenToRead,
        None,
        NOTHING,
    ),
    row(
        "a stranger: no open template",
        STRANGER,
        OpenTemplate,
        None,
        NOTHING,
    ),
    row(
        "a stranger with a stored role: nothing",
        STRANGER,
        InvitedOnly,
        Some(ContainerRole::Admin),
        NOTHING,
    ),
    // Whoever works in the workspace reads and may join its open boards.
    row(
        "a member reads and may join an open board",
        MEMBER,
        OpenToRead,
        None,
        &[See, Join, View, ListMembers],
    ),
    row(
        "a member: no invited-only board",
        MEMBER,
        InvitedOnly,
        None,
        NOTHING,
    ),
    row(
        "a member reads an open template",
        MEMBER,
        OpenTemplate,
        None,
        VIEWER,
    ),
    row(
        "a member: no invited-only template",
        MEMBER,
        InvitedOnlyTemplate,
        None,
        NOTHING,
    ),
    row(
        "an editor of an open template edits it",
        MEMBER,
        OpenTemplate,
        Some(ContainerRole::Editor),
        EDITOR,
    ),
    // A template the server ships with is read and copied by everyone who works there.
    row(
        "a member reads a built-in template",
        MEMBER,
        BuiltIn,
        None,
        VIEWER,
    ),
    row(
        "a guest: no built-in template",
        GUEST,
        BuiltIn,
        None,
        NOTHING,
    ),
    row(
        "a stranger: no built-in template",
        STRANGER,
        BuiltIn,
        None,
        NOTHING,
    ),
    // A guest sees the boards they were invited to, one by one, as no more than an editor.
    row("a guest: no open board", GUEST, OpenToRead, None, NOTHING),
    row(
        "a guest: no open template",
        GUEST,
        OpenTemplate,
        None,
        NOTHING,
    ),
    row(
        "a guest: no invited-only board",
        GUEST,
        InvitedOnly,
        None,
        NOTHING,
    ),
    row(
        "an invited guest views",
        GUEST,
        InvitedOnly,
        Some(ContainerRole::Viewer),
        VIEWER,
    ),
    row(
        "an invited guest edits",
        GUEST,
        InvitedOnly,
        Some(ContainerRole::Editor),
        EDITOR,
    ),
    row(
        "a guest on an open board is no more than invited",
        GUEST,
        OpenToRead,
        Some(ContainerRole::Editor),
        EDITOR,
    ),
    row(
        "a guest stored as admin is an editor: no link, no members",
        GUEST,
        InvitedOnly,
        Some(ContainerRole::Admin),
        EDITOR,
    ),
    row(
        "a guest from outside the organization alike",
        OUTSIDE_GUEST,
        InvitedOnly,
        Some(ContainerRole::Admin),
        EDITOR,
    ),
    // A workspace's administrator lists and manages its boards: sees them, lists and
    // changes their members, deletes and restores them. What is on a board for the
    // invited only they read as its member, and is not there for them otherwise.
    row(
        "a workspace admin manages an invited-only board",
        WORKSPACE_ADMIN,
        InvitedOnly,
        None,
        MANAGE,
    ),
    row(
        "a workspace admin manages an invited-only template",
        WORKSPACE_ADMIN,
        InvitedOnlyTemplate,
        None,
        MANAGE,
    ),
    row(
        "a workspace admin reads, manages and may join an open board",
        WORKSPACE_ADMIN,
        OpenToRead,
        None,
        &[See, Join, View, ListMembers, ManageRoles, Delete],
    ),
    row(
        "a workspace admin reads an open template",
        WORKSPACE_ADMIN,
        OpenTemplate,
        None,
        &[See, View, ListMembers, ManageRoles, Delete],
    ),
    row(
        "a workspace admin who made themselves a viewer reads",
        WORKSPACE_ADMIN,
        InvitedOnly,
        Some(ContainerRole::Viewer),
        &[See, View, ListMembers, ManageRoles, Delete],
    ),
    row(
        "a workspace admin restores a board in the trash",
        WORKSPACE_ADMIN,
        IN_THE_TRASH,
        None,
        MANAGE,
    ),
    row(
        "a workspace admin of no built-in template",
        WORKSPACE_ADMIN,
        BuiltIn,
        None,
        VIEWER,
    ),
    // An organization's administrator manages the organization; a workspace they are no
    // member of is not there for them.
    row(
        "an org admin outside the workspace: nothing",
        ORG_ADMIN_ONLY,
        OpenToRead,
        None,
        NOTHING,
    ),
    row(
        "an org admin outside the workspace, stored as board admin: nothing",
        ORG_ADMIN_ONLY,
        InvitedOnly,
        Some(ContainerRole::Admin),
        NOTHING,
    ),
    row(
        "an org admin in the workspace is what the workspace role is",
        ORG_ADMIN_IN_WORKSPACE,
        OpenToRead,
        None,
        &[See, Join, View, ListMembers],
    ),
    row(
        "an org admin outside the workspace manages none of its boards",
        ORG_ADMIN_ONLY,
        InvitedOnly,
        None,
        NOTHING,
    ),
    // Joined as the workspace's administrator, they are one: a board for the invited
    // only they manage and read as its member alone, which is recorded.
    row(
        "an org admin who joined manages an invited-only board and reads none of it",
        ORG_ADMIN_JOINED,
        InvitedOnly,
        None,
        MANAGE,
    ),
    row(
        "an org admin who joined reads an invited-only board as its member",
        ORG_ADMIN_JOINED,
        InvitedOnly,
        Some(ContainerRole::Viewer),
        &[See, View, ListMembers, ManageRoles, Delete],
    ),
    // Who was removed from the workspace opens nothing with the roles left on its boards.
    row(
        "removed: an admin role left opens nothing",
        REMOVED,
        InvitedOnly,
        Some(ContainerRole::Admin),
        NOTHING,
    ),
    row(
        "removed: an open board is gone",
        REMOVED,
        OpenToRead,
        Some(ContainerRole::Editor),
        NOTHING,
    ),
    row(
        "removed: an open template is gone",
        REMOVED,
        OpenTemplate,
        None,
        NOTHING,
    ),
    // A board in the trash is open to nobody but who was its member.
    row(
        "trash: an open board is no member's",
        MEMBER,
        IN_THE_TRASH,
        None,
        NOTHING,
    ),
    row(
        "trash: its admin brings it back",
        MEMBER,
        IN_THE_TRASH,
        Some(ContainerRole::Admin),
        ADMIN,
    ),
    row(
        "trash: who has left the workspace brings nothing back",
        REMOVED,
        IN_THE_TRASH,
        Some(ContainerRole::Admin),
        NOTHING,
    ),
];

const IN_THE_TRASH: ContainerScope = InvitedOnly;

/// Who only manages a board is told that its content is not there; who sees it any
/// other way is told that it is refused.
#[test]
fn who_only_manages_a_board_is_told_its_content_is_not_there() {
    let managed = grant(
        WORKSPACE_ADMIN,
        ContainerFacts {
            team_role: None,
            scope: InvitedOnly,
            role: None,
        },
    );
    for permission in [View, CommentCards, ManageCards, ManageOthersComments, Share] {
        assert!(managed.hides(permission), "{permission:?}");
    }
    for permission in [ManageProperties, ManageType, Join] {
        assert!(!managed.hides(permission), "{permission:?}");
    }
    let open = grant(
        WORKSPACE_ADMIN,
        ContainerFacts {
            team_role: None,
            scope: OpenToRead,
            role: None,
        },
    );
    assert!(
        !open.hides(View),
        "an open board is refused as to any worker"
    );
    let member = grant(
        MEMBER,
        ContainerFacts {
            team_role: None,
            scope: InvitedOnly,
            role: Some(ContainerRole::Viewer),
        },
    );
    assert!(!member.hides(ManageCards), "a viewer is refused");
    let stranger = grant(
        STRANGER,
        ContainerFacts {
            team_role: None,
            scope: InvitedOnly,
            role: None,
        },
    );
    assert!(stranger.hides(Delete), "no board for a stranger");
}

#[test]
fn the_trash_is_open_to_nobody() {
    assert_eq!(OpenToRead.in_the_trash(), IN_THE_TRASH);
    assert_eq!(OpenTemplate.in_the_trash(), InvitedOnlyTemplate);
    assert_eq!(InvitedOnly.in_the_trash(), InvitedOnly);
}

fn exactly(permissions: &[ContainerPermission]) -> Vec<ContainerPermission> {
    ContainerPermission::ALL
        .into_iter()
        .filter(|permission| permissions.contains(permission))
        .collect()
}

fn granted(grant: Grant) -> Vec<ContainerPermission> {
    ContainerPermission::ALL
        .into_iter()
        .filter(|permission| grant.allows(*permission))
        .collect()
}

#[test]
fn every_row_of_the_table_holds() {
    let mut wrong = Vec::new();
    for row in ROWS {
        let got = granted(grant(
            row.actor,
            ContainerFacts {
                team_role: None,
                scope: row.scope,
                role: row.role,
            },
        ));
        let want = exactly(row.allowed);
        if got != want {
            wrong.push(format!("{}: got {got:?}, want {want:?}", row.case));
        }
    }
    assert!(
        wrong.is_empty(),
        "rows that do not hold:\n{}",
        wrong.join("\n")
    );
}

#[test]
fn who_sees_is_who_is_refused_and_who_reads_is_who_sees() {
    // A refusal tells that the board is there: whoever may do anything on it sees it,
    // and whoever reads it sees it.
    for row in ROWS {
        let grant = grant(
            row.actor,
            ContainerFacts {
                team_role: None,
                scope: row.scope,
                role: row.role,
            },
        );
        if grant != Grant::NONE {
            assert!(grant.sees(), "{}: allowed something unseen", row.case);
        }
        if grant.reads() {
            assert!(grant.sees(), "{}: reads what it does not see", row.case);
        }
    }
}

#[test]
fn a_guest_is_given_no_more_than_editor() {
    use super::may_hold;
    for role in [
        ContainerRole::Viewer,
        ContainerRole::Commenter,
        ContainerRole::Editor,
    ] {
        assert!(
            may_hold(WorkspaceRole::Guest, role),
            "a guest may be {role:?}"
        );
    }
    assert!(
        !may_hold(WorkspaceRole::Guest, ContainerRole::Admin),
        "no guest is a board admin"
    );
    assert!(
        may_hold(WorkspaceRole::Member, ContainerRole::Admin),
        "a member may be"
    );
    assert!(
        may_hold(WorkspaceRole::Admin, ContainerRole::Admin),
        "a workspace admin may be"
    );
    assert_eq!(
        highest_container_role(WorkspaceRole::Guest),
        ContainerRole::Editor
    );
    assert_eq!(
        highest_container_role(WorkspaceRole::Member),
        ContainerRole::Admin
    );
    assert_eq!(
        highest_container_role(WorkspaceRole::Admin),
        ContainerRole::Admin
    );
}

#[test]
fn a_guest_sees_of_the_people_who_is_on_its_boards() {
    use fvoci_store::Reach;
    assert_eq!(
        super::people_reach(WorkspaceRole::Guest),
        Reach::SharedContainers
    );
    assert_eq!(super::people_reach(WorkspaceRole::Member), Reach::Workspace);
    assert_eq!(super::people_reach(WorkspaceRole::Admin), Reach::Workspace);
}

#[test]
fn effective_role_takes_the_higher_of_direct_and_inherited_role() {
    use fvoci_model::{ContainerMember, MinimumRole};
    let member = |role, inherited_role| ContainerMember {
        inherited_role,
        ..ContainerMember::new("", "", role)
    };
    assert_eq!(
        super::role_of(&member(ContainerRole::Viewer, None)),
        ContainerRole::Viewer
    );
    assert_eq!(
        super::role_of(&member(ContainerRole::Viewer, Some(MinimumRole::Editor))),
        ContainerRole::Editor
    );
    assert_eq!(
        super::role_of(&member(ContainerRole::Editor, Some(MinimumRole::Viewer))),
        ContainerRole::Editor
    );
    assert_eq!(
        super::role_of(&member(ContainerRole::Admin, Some(MinimumRole::Editor))),
        ContainerRole::Admin
    );
}

/// A comment is its author's to change, delete, and bring back if they deleted it; one
/// of somebody else, or one of theirs that somebody else deleted, is the board's
/// admins'. `(case, user, author, action, permission it takes)`.
#[test]
fn a_comment_is_brought_back_by_who_may_undo_its_deletion() {
    use super::{CommentAction, comment_permission};
    let rows = [
        (
            "the author changes it",
            "ann",
            "ann",
            CommentAction::Change,
            CommentCards,
        ),
        (
            "the author deletes it",
            "ann",
            "ann",
            CommentAction::Delete,
            CommentCards,
        ),
        (
            "the author brings back what they deleted",
            "ann",
            "ann",
            CommentAction::Undelete { deleted_by: "ann" },
            CommentCards,
        ),
        (
            "the author does not bring back what an admin deleted",
            "ann",
            "ann",
            CommentAction::Undelete { deleted_by: "bob" },
            ManageOthersComments,
        ),
        (
            "nor what nobody deleted",
            "ann",
            "ann",
            CommentAction::Undelete { deleted_by: "" },
            ManageOthersComments,
        ),
        (
            "somebody else's is changed by an admin",
            "bob",
            "ann",
            CommentAction::Change,
            ManageOthersComments,
        ),
        (
            "somebody else's is deleted by an admin",
            "bob",
            "ann",
            CommentAction::Delete,
            ManageOthersComments,
        ),
        (
            "an admin brings back what they deleted",
            "bob",
            "ann",
            CommentAction::Undelete { deleted_by: "bob" },
            ManageOthersComments,
        ),
        (
            "an admin brings back what its author deleted",
            "bob",
            "ann",
            CommentAction::Undelete { deleted_by: "ann" },
            ManageOthersComments,
        ),
    ];
    for (case, user, author, action, permission) in rows {
        assert_eq!(
            comment_permission(user, author, action),
            permission,
            "{case}"
        );
    }
}

/// Who sees whom: whoever works in the workspace its members, a guest the members it
/// shares a board with; who left is seen by name only, where they wrote on a board the
/// seer reads, and a role left on a board shows nobody. `(case, seer, person, among
/// people, by name)`.
#[test]
fn who_sees_whom_of_the_people() {
    use fvoci_store::Looking;

    use super::{Person, sees_person};
    const MEMBER: WorkspaceRole = WorkspaceRole::Member;
    const GUEST: WorkspaceRole = WorkspaceRole::Guest;
    let member = |shares_a_container, wrote_where_seer_reads| Person::Member {
        shares_a_container,
        wrote_where_seer_reads,
    };
    let outside = |wrote_where_seer_reads| Person::Outside {
        wrote_where_seer_reads,
    };
    let rows = [
        ("oneself", MEMBER, Person::Themselves, true, true),
        ("a guest itself", GUEST, Person::Themselves, true, true),
        (
            "a member of the workspace",
            MEMBER,
            member(false, false),
            true,
            true,
        ),
        (
            "an admin sees the members",
            WorkspaceRole::Admin,
            member(false, false),
            true,
            true,
        ),
        (
            "a guest: a member it shares no board with",
            GUEST,
            member(false, false),
            false,
            false,
        ),
        (
            "a guest: a member on its board",
            GUEST,
            member(true, false),
            true,
            true,
        ),
        (
            "a guest: a member who wrote on its board",
            GUEST,
            member(false, true),
            false,
            true,
        ),
        // A role left on a board after leaving the workspace shows nobody: the
        // person is outside, and the role is no fact of the rule.
        (
            "who left, with a role left on a board",
            MEMBER,
            outside(false),
            false,
            false,
        ),
        (
            "who left and wrote on a board read",
            MEMBER,
            outside(true),
            false,
            true,
        ),
        (
            "who left and wrote, to a guest who reads it",
            GUEST,
            outside(true),
            false,
            true,
        ),
        (
            "who left and wrote nothing the guest reads",
            GUEST,
            outside(false),
            false,
            false,
        ),
    ];
    for (case, seer, person, among, by_name) in rows {
        assert_eq!(
            (
                sees_person(seer, person, Looking::AmongPeople),
                sees_person(seer, person, Looking::ByName)
            ),
            (among, by_name),
            "{case}"
        );
    }
}

/// An organization's administrator, and nobody else, takes the routes of the
/// organization; it gives nothing on a board (the rows above).
#[test]
fn who_administers_the_organization() {
    let rows = [
        ("an org admin outside every workspace", ORG_ADMIN_ONLY, true),
        ("an org admin in a workspace", ORG_ADMIN_IN_WORKSPACE, true),
        (
            "an org admin who joined a workspace",
            ORG_ADMIN_JOINED,
            true,
        ),
        ("a workspace admin", WORKSPACE_ADMIN, false),
        ("a member", MEMBER, false),
        ("a stranger", STRANGER, false),
    ];
    for (case, actor, administers) in rows {
        assert_eq!(actor.administers_the_organization(), administers, "{case}");
    }
}

/// How one enters the room of a card's body that is edited together: writing for who
/// may change the board's cards, reading for who reads the board, and not at all for
/// anybody else. `(case, who, scope, role on the board, mode)`.
const ROOM_ROWS: &[RoomRow] = &[
    room(
        "an admin writes",
        MEMBER,
        InvitedOnly,
        Some(ContainerRole::Admin),
        WRITE,
    ),
    room(
        "an editor writes",
        MEMBER,
        InvitedOnly,
        Some(ContainerRole::Editor),
        WRITE,
    ),
    room(
        "a commenter reads",
        MEMBER,
        InvitedOnly,
        Some(ContainerRole::Commenter),
        READ,
    ),
    room(
        "a viewer reads",
        MEMBER,
        InvitedOnly,
        Some(ContainerRole::Viewer),
        READ,
    ),
    room(
        "an invited guest who edits writes",
        GUEST,
        InvitedOnly,
        Some(ContainerRole::Editor),
        WRITE,
    ),
    room(
        "an invited guest who views reads",
        GUEST,
        InvitedOnly,
        Some(ContainerRole::Viewer),
        READ,
    ),
    room(
        "an editor of an open template writes",
        MEMBER,
        OpenTemplate,
        Some(ContainerRole::Editor),
        WRITE,
    ),
    room(
        "whoever works there reads an open template",
        MEMBER,
        OpenTemplate,
        None,
        READ,
    ),
    // Whoever works in the workspace reads an open board in its rooms without joining.
    room(
        "a member who has not joined reads",
        MEMBER,
        OpenToRead,
        None,
        READ,
    ),
    room(
        "a member: no invited-only board",
        MEMBER,
        InvitedOnly,
        None,
        None,
    ),
    room("a guest who was not invited", GUEST, OpenToRead, None, None),
    // To manage a board is not to read it.
    room(
        "a workspace admin who only manages",
        WORKSPACE_ADMIN,
        InvitedOnly,
        None,
        None,
    ),
    room(
        "an organization's admin outside the workspace",
        ORG_ADMIN_ONLY,
        OpenToRead,
        None,
        None,
    ),
    // A role left on a board after leaving the workspace opens nothing.
    room(
        "who left the workspace, with a role left",
        REMOVED,
        InvitedOnly,
        Some(ContainerRole::Admin),
        None,
    ),
    room("a stranger", STRANGER, OpenToRead, None, None),
    room(
        "a stranger with a stored role",
        STRANGER,
        InvitedOnly,
        Some(ContainerRole::Editor),
        None,
    ),
];

/// One case: who, on what board, with which role there, enters the room how.
struct RoomRow {
    case: &'static str,
    actor: Actor,
    scope: ContainerScope,
    role: Option<ContainerRole>,
    mode: Option<DocMode>,
}

const fn room(
    case: &'static str,
    actor: Actor,
    scope: ContainerScope,
    role: Option<ContainerRole>,
    mode: Option<DocMode>,
) -> RoomRow {
    RoomRow {
        case,
        actor,
        scope,
        role,
        mode,
    }
}

const READ: Option<DocMode> = Some(DocMode::Read);
const WRITE: Option<DocMode> = Some(DocMode::Write);

#[test]
fn who_enters_the_room_of_a_body_and_how() {
    for row in ROOM_ROWS {
        let facts = ContainerFacts {
            team_role: None,
            scope: row.scope,
            role: row.role,
        };
        assert_eq!(doc_mode(grant(row.actor, facts)), row.mode, "{}", row.case);
    }
    assert_eq!(doc_mode(Grant::NONE), None, "no grant, as a read token is");
}

/// Who reads the audit log of a workspace: whoever works there, its administrators and
/// members, an administrator of the organization who joined it among them. A guest
/// does not, nor does whoever is no member, an administrator of the organization who
/// did not join it among them. `(case, what the user is in the workspace, reads)`.
#[test]
fn who_reads_the_audit_log_of_a_workspace() {
    use super::reads_the_audit_log;
    let rows = [
        ("its administrator", WORKSPACE_ADMIN, true),
        ("an org admin who joined it", ORG_ADMIN_JOINED, true),
        ("a member", MEMBER, true),
        ("an org admin who is a member", ORG_ADMIN_IN_WORKSPACE, true),
        ("a guest of the organization", GUEST, false),
        ("a guest from outside", OUTSIDE_GUEST, false),
        ("an org admin who did not join it", ORG_ADMIN_ONLY, false),
        ("who was removed", REMOVED, false),
        ("a stranger", STRANGER, false),
    ];
    for (case, actor, reads) in rows {
        assert_eq!(reads_the_audit_log(actor.workspace), reads, "{case}");
    }
}
