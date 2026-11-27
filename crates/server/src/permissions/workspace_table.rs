//! The table of what somebody may do in a workspace beside its boards: who invites its
//! members and its guests, who manages who is in it, and what a change of role makes of
//! a member. Beside `table.rs`, which is the boards', and reads its actors.

use fvoci_model::{ContainerRole, WorkspaceRole};

use super::ContainerScope::{BuiltIn, InvitedOnly, InvitedOnlyTemplate, OpenTemplate, OpenToRead};
use super::WorkspacePermission::{self, InviteMembers, ManageMembers, SwitchGuestInvitations};
use super::table::{
    GUEST, MEMBER, ORG_ADMIN_IN_WORKSPACE, ORG_ADMIN_ONLY, OUTSIDE_GUEST, REMOVED, STRANGER,
    WORKSPACE_ADMIN,
};
use super::{Actor, ContainerFacts, allows_in_workspace, grant, invites_guests, may_become};

/// What a member of a workspace does in it beside its boards: its administrators invite
/// its members, say whether guests are invited and manage who is in it, and nobody
/// else does. `(case, actor, what they may do)`; a permission a
/// row does not name is one the user does not have.
#[test]
fn the_work_of_a_workspace() {
    const ADMINISTERS: &[WorkspacePermission] =
        &[InviteMembers, SwitchGuestInvitations, ManageMembers];
    let rows: [(&str, Actor, &[WorkspacePermission]); 8] = [
        ("a workspace admin", WORKSPACE_ADMIN, ADMINISTERS),
        ("a member", MEMBER, &[]),
        ("a guest", GUEST, &[]),
        ("a guest from outside the organization", OUTSIDE_GUEST, &[]),
        ("an org admin outside the workspace", ORG_ADMIN_ONLY, &[]),
        (
            "an org admin who is a member of the workspace",
            ORG_ADMIN_IN_WORKSPACE,
            &[],
        ),
        ("who was removed", REMOVED, &[]),
        ("a stranger", STRANGER, &[]),
    ];
    for (case, actor, allowed) in rows {
        let got: Vec<WorkspacePermission> = WorkspacePermission::ALL
            .into_iter()
            .filter(|permission| allows_in_workspace(actor, *permission))
            .collect();
        assert_eq!(got, allowed, "{case}");
    }
}

/// What a change of a member's role makes of them: every pair of roles, and no other.
#[test]
fn what_a_member_of_a_workspace_is_made_by_a_change_of_role() {
    use WorkspaceRole::{Admin, Guest, Member};
    let rows = [
        (Admin, Admin, true),
        (Admin, Member, true),
        (Admin, Guest, false),
        (Member, Admin, true),
        (Member, Member, true),
        (Member, Guest, true),
        (Guest, Admin, false),
        (Guest, Member, false),
        (Guest, Guest, true),
    ];
    for (from, to, allowed) in rows {
        assert_eq!(may_become(from, to), allowed, "{from:?} to {to:?}");
    }
}

/// Guests are invited to a board by who adds its members: its admins, and the
/// administrators of its workspace, on a board they have no role on too. Nobody
/// invites one while the workspace does not let guests be invited, and nobody to a
/// template.
#[test]
fn who_invites_guests_to_a_board() {
    let workspace = |guest_invitations| fvoci_model::Workspace {
        guest_invitations,
        ..fvoci_model::Workspace::default()
    };
    let rows = [
        (
            "an admin of the board",
            MEMBER,
            Some(ContainerRole::Admin),
            true,
        ),
        (
            "an editor of the board",
            MEMBER,
            Some(ContainerRole::Editor),
            false,
        ),
        (
            "a member of the workspace who is not on it",
            MEMBER,
            None,
            false,
        ),
        (
            "a workspace admin who is not on it",
            WORKSPACE_ADMIN,
            None,
            true,
        ),
        (
            "a workspace admin who views it",
            WORKSPACE_ADMIN,
            Some(ContainerRole::Viewer),
            true,
        ),
        (
            "a guest who edits it",
            GUEST,
            Some(ContainerRole::Editor),
            false,
        ),
        (
            "a guest with a role above a guest's left on it",
            GUEST,
            Some(ContainerRole::Admin),
            false,
        ),
        (
            "who was removed, its admin once",
            REMOVED,
            Some(ContainerRole::Admin),
            false,
        ),
        ("a stranger", STRANGER, None, false),
    ];
    for (case, actor, role, invites) in rows {
        for scope in [InvitedOnly, OpenToRead] {
            let granted = grant(
                actor,
                ContainerFacts {
                    team_role: None,
                    scope,
                    role,
                },
            );
            assert_eq!(
                invites_guests(granted, scope, &workspace(true)),
                invites,
                "{case}, on a board that is {scope:?}"
            );
            assert!(
                !invites_guests(granted, scope, &workspace(false)),
                "{case}: nobody while the workspace invites no guests"
            );
        }
        // A template takes no guests, whoever asks.
        for scope in [InvitedOnlyTemplate, OpenTemplate, BuiltIn] {
            let granted = grant(
                actor,
                ContainerFacts {
                    team_role: None,
                    scope,
                    role,
                },
            );
            assert!(
                !invites_guests(granted, scope, &workspace(true)),
                "{case}, on a template that is {scope:?}"
            );
        }
    }
}
