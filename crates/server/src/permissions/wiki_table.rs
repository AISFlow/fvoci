//! An open wiki is read by whoever works in its workspace, joined or not; it is written
//! in by its members alone. Every other rule is the one of a board.

use fvoci_model::ContainerRole::{self, Admin, Commenter, Editor, Viewer};

use super::ContainerPermission::{
    CommentCards, Delete, Join, ListMembers, ManageCards, ManageProperties, ManageRoles, See, View,
};
use super::ContainerScope::{BuiltIn, InvitedOnlyTemplate, OpenTemplate};
use super::ContainerScope::{InvitedOnly, OpenToRead};
use super::table::{
    GUEST, MEMBER, ORG_ADMIN_IN_WORKSPACE, ORG_ADMIN_ONLY, OUTSIDE_GUEST, REMOVED, STRANGER,
    WORKSPACE_ADMIN,
};
use super::{
    Actor, ContainerFacts, ContainerPermission, ContainerScope, DocMode, doc_mode, grant,
    told_without_a_role,
};
use fvoci_model::WorkspaceRole;

/// What who works in the workspace may do on an open wiki they did not join: read it,
/// list who is on it, and join it.
const READS_AND_JOINS: &[ContainerPermission] = &[See, Join, View, ListMembers];

fn granted(
    actor: Actor,
    scope: ContainerScope,
    role: Option<ContainerRole>,
) -> Vec<ContainerPermission> {
    let grant = grant(
        actor,
        ContainerFacts {
            scope,
            role,
            team_role: None,
        },
    );
    ContainerPermission::ALL
        .into_iter()
        .filter(|permission| grant.allows(*permission))
        .collect()
}

fn exactly(permissions: &[ContainerPermission]) -> Vec<ContainerPermission> {
    ContainerPermission::ALL
        .into_iter()
        .filter(|permission| permissions.contains(permission))
        .collect()
}

#[test]
fn whoever_works_in_the_workspace_reads_an_open_wiki_without_joining_it() {
    for (case, actor) in [
        ("a member", MEMBER),
        (
            "an organization's administrator who joined",
            ORG_ADMIN_IN_WORKSPACE,
        ),
    ] {
        assert_eq!(
            granted(actor, OpenToRead, None),
            exactly(READS_AND_JOINS),
            "{case}"
        );
    }
    let admin = exactly(&[See, Join, View, ListMembers, ManageRoles, Delete]);
    assert_eq!(
        granted(WORKSPACE_ADMIN, OpenToRead, None),
        admin,
        "the workspace's administrator reads it as well as manages it"
    );
}

#[test]
fn an_open_wiki_is_written_in_by_its_members_alone() {
    for actor in [MEMBER, WORKSPACE_ADMIN] {
        let unjoined = granted(actor, OpenToRead, None);
        for writing in [CommentCards, ManageCards, ManageProperties] {
            assert!(!unjoined.contains(&writing), "{writing:?}");
        }
    }
    let editor = granted(MEMBER, OpenToRead, Some(Editor));
    assert!(editor.contains(&ManageCards));
    assert!(editor.contains(&Join), "joining raises to the minimum role");
}

#[test]
fn an_open_wiki_is_nothing_to_a_guest_or_to_who_is_not_in_the_workspace() {
    for (case, actor) in [
        ("a guest", GUEST),
        ("a guest of no organization", OUTSIDE_GUEST),
        ("an organization's administrator only", ORG_ADMIN_ONLY),
        ("who was removed", REMOVED),
        ("a stranger", STRANGER),
    ] {
        assert_eq!(granted(actor, OpenToRead, None), Vec::new(), "{case}");
    }
    assert_eq!(
        granted(GUEST, OpenToRead, Some(Editor)),
        granted(GUEST, InvitedOnly, Some(Editor)),
        "a guest invited to it is on it as on any wiki"
    );
}

#[test]
fn a_role_on_an_open_wiki_is_the_role_on_a_wiki_for_the_invited_only() {
    for role in [Viewer, Commenter, Editor, Admin] {
        let mut on_open = granted(MEMBER, OpenToRead, Some(role));
        on_open.retain(|permission| *permission != Join);
        assert_eq!(
            on_open,
            granted(MEMBER, InvitedOnly, Some(role)),
            "{role:?}"
        );
    }
}

#[test]
fn an_open_board_shares_open_to_read_with_wikis_and_courses() {
    assert_eq!(granted(MEMBER, OpenToRead, None), exactly(READS_AND_JOINS));
}

#[test]
fn an_open_wiki_in_the_trash_is_open_to_nobody() {
    assert_eq!(OpenToRead.in_the_trash(), InvitedOnly);
    assert_eq!(granted(MEMBER, OpenToRead.in_the_trash(), None), Vec::new());
}

#[test]
fn who_has_not_joined_an_open_wiki_is_in_the_rooms_of_its_pages_to_read() {
    let facts = |role| ContainerFacts {
        scope: OpenToRead,
        role,
        team_role: None,
    };
    assert_eq!(doc_mode(grant(MEMBER, facts(None))), Some(DocMode::Read));
    assert_eq!(
        doc_mode(grant(MEMBER, facts(Some(Viewer)))),
        Some(DocMode::Read)
    );
    assert_eq!(
        doc_mode(grant(MEMBER, facts(Some(Editor)))),
        Some(DocMode::Write)
    );
    assert_eq!(doc_mode(grant(GUEST, facts(None))), None);
}

#[test]
fn whoever_reads_an_open_wiki_without_joining_it_is_no_mere_manager_of_it() {
    let admin = grant(
        WORKSPACE_ADMIN,
        ContainerFacts {
            scope: OpenToRead,
            role: None,
            team_role: None,
        },
    );
    assert!(!admin.manages_only());
    assert!(!admin.hides(View));
}

#[test]
fn the_changes_of_an_open_wiki_are_told_to_whoever_works_there_and_of_nothing_else() {
    for scope in [
        OpenToRead,
        InvitedOnly,
        OpenTemplate,
        InvitedOnlyTemplate,
        BuiltIn,
    ] {
        for role in [
            WorkspaceRole::Admin,
            WorkspaceRole::Member,
            WorkspaceRole::Guest,
        ] {
            let told = scope == OpenToRead && role != WorkspaceRole::Guest;
            assert_eq!(
                told_without_a_role(scope, role),
                told,
                "{scope:?}, {role:?}"
            );
        }
    }
}

#[test]
fn open_courses_share_the_open_to_read_scope_with_open_wikis() {
    use fvoci_model::{Container, ContainerKind, ContainerVisibility};

    let container = |kind, visibility| Container {
        kind,
        visibility,
        ..Container::default()
    };
    assert_eq!(
        ContainerScope::of(&container(ContainerKind::Wiki, ContainerVisibility::Open)),
        OpenToRead
    );
    assert_eq!(
        ContainerScope::of(&container(ContainerKind::Course, ContainerVisibility::Open)),
        OpenToRead
    );
    assert_eq!(
        ContainerScope::of(&container(
            ContainerKind::Course,
            ContainerVisibility::Private
        )),
        InvitedOnly
    );
    assert_eq!(
        ContainerScope::of(&container(ContainerKind::Board, ContainerVisibility::Open)),
        OpenToRead
    );
    assert_eq!(
        ContainerScope::of(&container(ContainerKind::Diary, ContainerVisibility::Open)),
        InvitedOnly
    );
}
