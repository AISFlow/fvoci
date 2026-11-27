//! What an organization's administrators change of it: the roles of its members, its
//! workspaces and their trash, joining a workspace as its administrator, and the audit
//! log that records it and is never changed.

use fvoci_model::InstanceRole;
use fvoci_model::{
    AuditAction, Block, BlockType, Container, ContainerMember, ContainerVisibility, FileRecord,
    OrgRole, Workspace, WorkspaceRole, new_id,
};
use fvoci_store::{
    AuditEntry, AuditPlace, DbType, Joining, LooseState, OrgRoleChange, Paging, Purged, Standing,
    Store, StoreError, Taken, Tenant, WorkspaceTrashing,
};

use crate::common::{self, TestStore};

/// A user who is a member of the test's organization with the role: a user made through
/// its handle joins it. An administrator of it administers the instance, as the two
/// are one for now.
async fn org_member(test: &TestStore, username: &str, role: OrgRole) -> String {
    let user_id = common::user(test, username).await;
    for statement in [
        format!(
            "UPDATE organization_members SET role = '{}' WHERE user_id = '{user_id}'",
            role.as_str()
        ),
        format!(
            "UPDATE users SET is_admin = {} WHERE id = '{user_id}'",
            role == OrgRole::Admin
        ),
    ] {
        test.execute_unchecked(&statement)
            .await
            .expect("the user has the role in the organization");
    }
    user_id
}

/// A workspace of the test's organization with the name, made by `actor_id`.
async fn named_workspace(store: &Store, name: &str, actor_id: &str) -> String {
    let workspace = Workspace {
        id: new_id(),
        title: name.to_owned(),
        ..Workspace::default()
    };
    store
        .create_named_workspace(&workspace, actor_id)
        .await
        .expect("the workspace is made");
    workspace.id
}

/// A private board of the workspace whose admin is `user_id`.
async fn private_board(store: &Store, workspace_id: &str, user_id: &str) -> String {
    let mut board = Container {
        id: new_id(),
        workspace_id: workspace_id.to_owned(),
        visibility: ContainerVisibility::Private,
        ..Container::default()
    };
    common::works_in(store, workspace_id, user_id).await;
    store
        .insert_container_with_admin(&mut board, user_id)
        .await
        .expect("the board is stored");
    board.id
}

fn file_of(board_id: &str, stored_name: &str) -> FileRecord {
    FileRecord {
        id: new_id(),
        container_id: board_id.to_owned(),
        stored_name: stored_name.to_owned(),
        name: format!("uploaded as {stored_name}"),
        extension: ".png".to_owned(),
        size: 2048,
        path: format!("boards/{board_id}/{stored_name}"),
        created_at: 1,
        ..FileRecord::default()
    }
}

/// Puts the test's organization out of use (`deleted_at` set), or back in use (`None`).
async fn set_org_deleted(test: &TestStore, deleted_at: Option<i64>) {
    let value = deleted_at.map_or_else(|| "NULL".to_owned(), |at| at.to_string());
    let statement = format!(
        "UPDATE organizations SET deleted_at = {value} WHERE id = '{}'",
        test.org_id
    );
    test.execute_unchecked(&statement)
        .await
        .expect("the organization's use changes");
}

/// What the workspace's audit log says, as `(action, actor, subject)`.
async fn log_of(store: &Store, workspace_id: &str) -> Vec<(AuditAction, String, String)> {
    store
        .get_audit_log(workspace_id)
        .await
        .expect("the log is read")
        .into_iter()
        .map(|entry| {
            (
                entry.action,
                entry.actor.person_id().unwrap_or_default().to_owned(),
                entry.subject_id,
            )
        })
        .collect()
}

/// What the records of the organization itself say, as `(action, target)`.
async fn organization_records(store: &Store) -> Vec<(AuditAction, String)> {
    let every = Paging {
        page: 0,
        per_page: std::num::NonZeroU32::new(1000).expect("a page size"),
    };
    store
        .audit_records(AuditPlace::Organization, every)
        .await
        .expect("the organization's records")
        .into_iter()
        .map(|record| (record.action, record.target_id))
        .collect()
}

/// What the user is in the workspace; `None` for no member.
async fn role_in(store: &Store, workspace_id: &str, user_id: &str) -> Option<WorkspaceRole> {
    store
        .resolve_workspace(user_id, workspace_id)
        .await
        .expect("the membership is read")
        .map(|access| access.workspace_role)
}

/// A workspace in the trash goes for good as its boards do: they, the boards it had in
/// the trash, their blocks and their files go, the objects of the files are doomed,
/// and its members, categories and audit log go with it. Another workspace's board and
/// file stay, and so does what is recorded of the organization.
#[tokio::test]
async fn a_workspace_in_the_trash_goes_for_good_as_its_boards_do() {
    let test = common::setup().await;
    let admin = org_member(&test, "admin", OrgRole::Admin).await;
    let doomed = named_workspace(&test, "Doomed", &admin).await;
    let kept = named_workspace(&test, "Kept", &admin).await;
    let board = private_board(&test, &doomed, &admin).await;
    let trashed_before = private_board(&test, &doomed, &admin).await;
    let other = private_board(&test, &kept, &admin).await;
    let files = [
        file_of(&board, "7a.png"),
        file_of(&trashed_before, "7b.png"),
    ];
    for file in files.iter().chain([&file_of(&other, "7c.png")]) {
        test.save_file_record(file)
            .await
            .expect("the file is recorded");
    }
    test.delete_container(&trashed_before, &admin)
        .await
        .expect("a board of it is in the trash already");
    assert_eq!(
        test.join_workspace_as_org_admin(&doomed, &admin)
            .await
            .expect("joined"),
        Joining::Joined
    );

    // In use, or in the trash since the cutoff or later, it stays.
    let none = test
        .purge_workspace(&doomed, i64::MAX)
        .await
        .expect("nothing to purge");
    assert_eq!(none, Purged::default(), "a workspace in use stays");
    assert_eq!(
        test.trash_workspace(&doomed, &admin)
            .await
            .expect("trashed"),
        WorkspaceTrashing::Done
    );
    assert_eq!(
        test.purge_workspace(&doomed, 1).await.expect("too early"),
        Purged::default(),
        "trashed later than the cutoff"
    );

    // Also of an organization no longer in use, whose trash is still emptied.
    set_org_deleted(&test, Some(5)).await;
    let Purged {
        records,
        files: mut gone,
    } = test
        .purge_workspace(&doomed, i64::MAX)
        .await
        .expect("the workspace is purged");
    set_org_deleted(&test, None).await;
    gone.sort_by(|a, b| a.path.cmp(&b.path));
    assert_eq!(gone, files, "the files of both of its boards");
    assert_eq!(records, 2, "its two boards");
    let doomed_paths: Vec<String> = test
        .loose_objects()
        .await
        .expect("the listed objects")
        .into_iter()
        .filter(|object| object.state == LooseState::Doomed)
        .map(|object| object.path)
        .collect();
    assert_eq!(
        doomed_paths,
        files
            .iter()
            .map(|file| file.path.clone())
            .collect::<Vec<_>>(),
        "the objects of its files are doomed, and no other"
    );
    let workspaces = test.org_workspaces(&admin).await.expect("listed");
    assert!(
        workspaces.iter().all(|workspace| workspace.id != doomed),
        "{workspaces:?}"
    );
    assert!(
        log_of(&test, &doomed).await.is_empty(),
        "its audit log went"
    );
    for board_id in [&board, &trashed_before] {
        let error = test.get_container(board_id).await.expect_err("gone");
        assert!(error.is_not_found(), "{error:?}");
        let error = test
            .get_deleted_container(board_id)
            .await
            .expect_err("gone");
        assert!(error.is_not_found(), "{error:?}");
    }
    test.get_container(&other)
        .await
        .expect("the other board stays");
    test.get_file_record(&other, "7c.png")
        .await
        .expect("the other board's file stays");
    let of_the_organization = organization_records(&test).await;
    assert!(
        of_the_organization.contains(&(AuditAction::WorkspaceDeleted, doomed.clone())),
        "what was done to the organization stays: {of_the_organization:?}"
    );
    test.teardown().await;
}

/// A record of the audit log is not changed, and is not removed while its workspace is
/// there, past the store too; nor is a record of the organization.
#[tokio::test]
async fn a_record_of_the_audit_log_is_never_changed_or_removed() {
    let test = common::setup().await;
    let admin = org_member(&test, "admin", OrgRole::Admin).await;
    let workspace = named_workspace(&test, "Kept", &admin).await;
    test.join_workspace_as_org_admin(&workspace, &admin)
        .await
        .expect("joined");

    for statement in [
        "UPDATE audit_log SET action = 'org_admin_left'",
        "UPDATE audit_log SET created_at = 0",
        "DELETE FROM audit_log",
        "DELETE FROM audit_log WHERE workspace_id IS NULL",
    ] {
        let refused = test.execute_unchecked(statement).await;
        assert!(refused.is_err(), "{statement}: {refused:?}");
    }
    assert_eq!(
        log_of(&test, &workspace).await,
        [(AuditAction::OrgAdminJoined, admin.clone(), String::new())]
    );
    test.teardown().await;
}

/// An administrator of the organization who joins a workspace is its administrator,
/// and leaving gives back what they were: a member with their role, and the role on a
/// board they had before, without the roles they gave themselves meanwhile. One who
/// was no member is none again. Each step is recorded in the workspace.
#[tokio::test]
async fn leaving_a_workspace_joined_gives_back_what_one_was() {
    let test = common::setup().await;
    let admin = org_member(&test, "admin", OrgRole::Admin).await;
    let owner = org_member(&test, "owner", OrgRole::Member).await;
    let workspace = named_workspace(&test, "Work", &admin).await;
    for (user, role) in [
        (&admin, WorkspaceRole::Member),
        (&owner, WorkspaceRole::Member),
    ] {
        test.add_workspace_member(&workspace, user, role)
            .await
            .expect("a member");
    }
    let had = private_board(&test, &workspace, &admin).await;
    let taken = private_board(&test, &workspace, &owner).await;

    assert_eq!(
        test.join_workspace_as_org_admin(&workspace, &admin)
            .await
            .expect("joined"),
        Joining::Joined
    );
    assert_eq!(
        test.join_workspace_as_org_admin(&workspace, &admin)
            .await
            .expect("joined again"),
        Joining::Already,
        "nothing changes for its administrator"
    );
    assert_eq!(
        role_in(&test, &workspace, &admin).await,
        Some(WorkspaceRole::Admin)
    );
    let as_viewer = ContainerMember {
        container_id: taken.clone(),
        user_id: admin.clone(),
        role: fvoci_model::ContainerRole::Viewer,
        synthetic: false,
        inherited_role: None,
    };
    test.add_member_recorded(
        &as_viewer,
        &AuditEntry::new(
            &workspace,
            &admin,
            AuditAction::SelfAddedToContainer,
            &taken,
        ),
    )
    .await
    .expect("the admin makes themselves a member of a private board");

    assert!(
        test.leave_workspace_as_org_admin(&workspace, &admin)
            .await
            .expect("left")
    );
    assert!(
        !test
            .leave_workspace_as_org_admin(&workspace, &owner)
            .await
            .expect("not joined"),
        "who did not join stays as they are"
    );
    assert_eq!(
        role_in(&test, &workspace, &admin).await,
        Some(WorkspaceRole::Member),
        "a member again"
    );
    test.get_member_for_container(&had, &admin)
        .await
        .expect("the role they had before stays");
    let error = test
        .get_member_for_container(&taken, &admin)
        .await
        .expect_err("the role they gave themselves goes");
    assert!(error.is_not_found(), "{error:?}");
    assert_eq!(
        log_of(&test, &workspace).await,
        [
            (AuditAction::OrgAdminJoined, admin.clone(), String::new()),
            (
                AuditAction::SelfAddedToContainer,
                admin.clone(),
                String::new()
            ),
            (AuditAction::OrgAdminLeft, admin.clone(), String::new()),
        ]
    );
    test.teardown().await;
}

/// Who stops administering the organization is taken out of every workspace they
/// joined as its administrator, in use or in the trash: what they were before, or no
/// member. Each workspace records who did it to whom; the organization records the
/// change of the role.
#[tokio::test]
async fn who_stops_administering_the_organization_is_taken_out_of_what_they_joined() {
    let test = common::setup().await;
    let admin = org_member(&test, "admin", OrgRole::Admin).await;
    let other_admin = org_member(&test, "other.admin", OrgRole::Admin).await;
    let was_member = named_workspace(&test, "Was member", &admin).await;
    let was_none = named_workspace(&test, "Was none", &admin).await;
    let in_the_trash = named_workspace(&test, "In the trash", &admin).await;
    test.add_workspace_member(&was_member, &admin, WorkspaceRole::Guest)
        .await
        .expect("a guest of it");
    for workspace in [&was_member, &was_none, &in_the_trash] {
        test.join_workspace_as_org_admin(workspace, &admin)
            .await
            .expect("joined");
    }
    test.trash_workspace(&in_the_trash, &other_admin)
        .await
        .expect("trashed");

    assert_eq!(
        test.set_org_role(&admin, OrgRole::Member, &other_admin)
            .await
            .expect("the role is changed"),
        OrgRoleChange::Done
    );
    assert_eq!(
        role_in(&test, &was_member, &admin).await,
        Some(WorkspaceRole::Guest),
        "what they were before"
    );
    assert_eq!(role_in(&test, &was_none, &admin).await, None);
    test.restore_workspace(&in_the_trash, &other_admin)
        .await
        .expect("restored");
    assert_eq!(
        role_in(&test, &in_the_trash, &admin).await,
        None,
        "the one in the trash too"
    );
    for workspace in [&was_member, &was_none, &in_the_trash] {
        assert_eq!(
            log_of(&test, workspace).await,
            [
                (AuditAction::OrgAdminJoined, admin.clone(), String::new()),
                (
                    AuditAction::OrgAdminRevoked,
                    other_admin.clone(),
                    admin.clone()
                ),
            ]
        );
    }
    assert_eq!(
        test.join_workspace_as_org_admin(&was_none, &admin)
            .await
            .expect("asked"),
        Joining::NoOrgAdmin,
        "a member of the organization does not join so"
    );
    test.teardown().await;
}

/// The organization's last administrator stays one, and a user who is no member of it
/// is told so.
#[tokio::test]
async fn the_last_administrator_of_the_organization_stays_one() {
    let test = common::setup().await;
    let admin = org_member(&test, "admin", OrgRole::Admin).await;
    let member = org_member(&test, "member", OrgRole::Member).await;
    let stranger = new_id();

    assert_eq!(
        test.set_org_role(&admin, OrgRole::Member, &admin)
            .await
            .expect("asked"),
        OrgRoleChange::LastAdmin
    );
    assert_eq!(
        test.set_org_role(&stranger, OrgRole::Admin, &admin)
            .await
            .expect("asked"),
        OrgRoleChange::NotAMember
    );
    assert_eq!(
        test.set_org_role(&member, OrgRole::Admin, &admin)
            .await
            .expect("asked"),
        OrgRoleChange::Done
    );
    assert_eq!(
        test.set_org_role(&admin, OrgRole::Member, &member)
            .await
            .expect("asked"),
        OrgRoleChange::Done,
        "with another administrator, one steps down"
    );
    assert_eq!(
        test.set_org_role(&member, OrgRole::Member, &member)
            .await
            .expect("asked"),
        OrgRoleChange::LastAdmin
    );
    test.teardown().await;
}

/// A name is one workspace's among those in use: one in the trash leaves it to be
/// taken, and comes back only under a name nobody took meanwhile. The default
/// workspace is not put in the trash.
#[tokio::test]
async fn a_name_is_one_workspaces_in_use_and_the_default_one_stays() {
    let test = common::setup().await;
    let admin = org_member(&test, "admin", OrgRole::Admin).await;
    let first = named_workspace(&test, "Design", &admin).await;
    let again = Workspace {
        id: new_id(),
        title: "Design".to_owned(),
        ..Workspace::default()
    };
    let taken = test
        .create_named_workspace(&again, &admin)
        .await
        .expect_err("the name is taken");
    assert!(
        matches!(taken, StoreError::Conflict(Taken::WorkspaceName)),
        "{taken:?}"
    );
    test.trash_workspace(&first, &admin).await.expect("trashed");
    let second = named_workspace(&test, "Design", &admin).await;
    let refused = test
        .restore_workspace(&first, &admin)
        .await
        .expect_err("its name is taken");
    assert!(
        matches!(refused, StoreError::Conflict(Taken::WorkspaceName)),
        "{refused:?}"
    );
    test.rename_workspace(&second, "Design 2", &admin)
        .await
        .expect("renamed");
    test.restore_workspace(&first, &admin)
        .await
        .expect("restored under its name");

    assert_eq!(
        test.trash_workspace(&test.workspace_id, &admin)
            .await
            .expect("asked"),
        WorkspaceTrashing::Default
    );
    assert_eq!(
        test.trash_workspace(&new_id(), &admin)
            .await
            .expect("asked"),
        WorkspaceTrashing::NotFound
    );
    test.teardown().await;
}

/// Whatever a joined administrator made themselves on the workspace's boards goes when
/// the joining ends: a role raised on a board they read before is lowered again, and the
/// membership of a board made meanwhile goes. What they held before stays.
#[tokio::test]
async fn the_roles_on_boards_go_back_to_what_they_were_when_one_joined() {
    let test = common::setup().await;
    let admin = org_member(&test, "admin", OrgRole::Admin).await;
    let owner = org_member(&test, "owner", OrgRole::Member).await;
    let workspace = named_workspace(&test, "Work", &admin).await;
    for user in [&admin, &owner] {
        test.add_workspace_member(&workspace, user, WorkspaceRole::Member)
            .await
            .expect("a member");
    }
    let read = private_board(&test, &workspace, &owner).await;
    let as_viewer = ContainerMember {
        container_id: read.clone(),
        user_id: admin.clone(),
        role: fvoci_model::ContainerRole::Viewer,
        synthetic: false,
        inherited_role: None,
    };
    test.add_member(&as_viewer).await.expect("a viewer of it");

    test.join_workspace_as_org_admin(&workspace, &admin)
        .await
        .expect("joined");
    test.change_member(&ContainerMember {
        role: fvoci_model::ContainerRole::Admin,
        ..as_viewer.clone()
    })
    .await
    .expect("the admin raises their own role");
    let made = private_board(&test, &workspace, &admin).await;
    test.leave_workspace_as_org_admin(&workspace, &admin)
        .await
        .expect("left");

    let held = test
        .get_member_for_container(&read, &admin)
        .await
        .expect("the role they had stays");
    assert_eq!(
        held.role,
        fvoci_model::ContainerRole::Viewer,
        "a viewer again: {held:?}"
    );
    let error = test
        .get_member_for_container(&made, &admin)
        .await
        .expect_err("the board made meanwhile is not theirs");
    assert!(error.is_not_found(), "{error:?}");
    test.teardown().await;
}

/// A board deleted while one was joined keeps in the trash what they held when they
/// joined, and no more: one they raised their role on comes back with the role they had,
/// and one they made comes back without them.
#[tokio::test]
async fn a_board_deleted_while_one_was_joined_comes_back_as_it_was_for_them() {
    let test = common::setup().await;
    let admin = org_member(&test, "admin", OrgRole::Admin).await;
    let owner = org_member(&test, "owner", OrgRole::Member).await;
    let workspace = named_workspace(&test, "Work", &admin).await;
    for user in [&admin, &owner] {
        test.add_workspace_member(&workspace, user, WorkspaceRole::Member)
            .await
            .expect("a member");
    }
    let read = private_board(&test, &workspace, &owner).await;
    let as_viewer = ContainerMember {
        container_id: read.clone(),
        user_id: admin.clone(),
        role: fvoci_model::ContainerRole::Viewer,
        synthetic: false,
        inherited_role: None,
    };
    test.add_member(&as_viewer).await.expect("a viewer of it");
    let theirs_before = private_board(&test, &workspace, &admin).await;
    test.delete_container(&theirs_before, &admin)
        .await
        .expect("deleted before the joining");

    test.join_workspace_as_org_admin(&workspace, &admin)
        .await
        .expect("joined");
    test.change_member(&ContainerMember {
        role: fvoci_model::ContainerRole::Admin,
        ..as_viewer.clone()
    })
    .await
    .expect("the admin raises their own role");
    let made = private_board(&test, &workspace, &admin).await;
    for board in [&read, &made] {
        test.delete_container(board, &admin).await.expect("deleted");
    }
    test.leave_workspace_as_org_admin(&workspace, &admin)
        .await
        .expect("left");
    for board in [&read, &made, &theirs_before] {
        test.undelete_container(board, &owner)
            .await
            .expect("restored");
    }
    let kept = test
        .get_member_for_container(&theirs_before, &admin)
        .await
        .expect("a board of theirs from before stays theirs");
    assert_eq!(
        kept.role,
        fvoci_model::ContainerRole::Admin,
        "its admin still: {kept:?}"
    );

    let held = test
        .get_member_for_container(&read, &admin)
        .await
        .expect("the role they had comes back");
    assert_eq!(
        held.role,
        fvoci_model::ContainerRole::Viewer,
        "a viewer again: {held:?}"
    );
    let error = test
        .get_member_for_container(&made, &admin)
        .await
        .expect_err("the board made meanwhile is not theirs");
    assert!(error.is_not_found(), "{error:?}");
    test.teardown().await;
}

/// A change of the organization is made by its administrator alone, also one whose
/// request was let in before they stopped being one; and a role on a board is given as
/// its manager only by who still administers the workspace.
#[tokio::test]
async fn who_administers_no_more_changes_nothing() {
    let test = common::setup().await;
    let admin = org_member(&test, "admin", OrgRole::Admin).await;
    let former = org_member(&test, "former", OrgRole::Member).await;
    let workspace = named_workspace(&test, "Work", &admin).await;
    let board = private_board(&test, &workspace, &admin).await;

    let refused = [
        test.set_org_role(&former, OrgRole::Admin, &former)
            .await
            .map(|_| ()),
        test.trash_workspace(&workspace, &former).await.map(|_| ()),
        test.rename_workspace(&workspace, "Mine", &former).await,
        test.update_organization(
            &fvoci_model::OrganizationPatch {
                public_links: Some(false),
                ..fvoci_model::OrganizationPatch::default()
            },
            &former,
        )
        .await
        .map(|_| ()),
    ];
    for refusal in refused {
        let error = refusal.expect_err("refused");
        assert!(error.is_not_found(), "{error:?}");
    }
    let added = test
        .add_member_recorded(
            &ContainerMember {
                container_id: board.clone(),
                user_id: former.clone(),
                role: fvoci_model::ContainerRole::Viewer,
                synthetic: false,
                inherited_role: None,
            },
            &AuditEntry::new(
                &workspace,
                &former,
                AuditAction::SelfAddedToContainer,
                &board,
            ),
        )
        .await
        .expect_err("no manager of it");
    assert!(added.is_not_found(), "{added:?}");
    assert_eq!(
        log_of(&test, &workspace).await,
        Vec::new(),
        "nothing is recorded"
    );
    test.teardown().await;
}

/// The organization's administrators are the instance's for now: one made a member of
/// the organization stops administering the instance, unless they are its last
/// administrator, and a change through the instance is recorded as one through the
/// organization.
#[tokio::test]
async fn the_instance_and_the_organization_keep_one_administration() {
    let test = common::setup().await;
    let admin = org_member(&test, "admin", OrgRole::Admin).await;
    let other = org_member(&test, "other", OrgRole::Member).await;

    test.set_user_admin(&other, InstanceRole::Administrator, &admin)
        .await
        .expect("made the instance's administrator");
    assert_eq!(
        test.set_org_role(&admin, OrgRole::Member, &other)
            .await
            .expect("asked"),
        OrgRoleChange::Done
    );
    let stepped_down = test.get_user_by_id(&admin).await.expect("the user");
    assert!(!stepped_down.is_admin, "no administrator of the instance");
    assert_eq!(
        organization_records(&test).await,
        [
            (AuditAction::OrgAdminRemoved, test.org_id.clone()),
            (AuditAction::OrgAdminGranted, test.org_id.clone()),
        ],
        "both changes are recorded, the newest first"
    );
    test.teardown().await;
}

/// Of several organizations, the instance's administrators are the oldest one's, the
/// instance's own: a change of a role in another changes nothing of the instance, either
/// way, and is never refused for the instance's sake. In the instance's own, who stops
/// administering it stops administering the instance.
#[tokio::test]
async fn the_instance_is_administered_through_its_own_organization_only() {
    // An SQLite database holds one organization.
    if common::db_type() != DbType::Postgres {
        return;
    }
    let test = common::setup().await;
    // The only administrator of the instance, through the test's organization (the
    // oldest), and a plain member of the other.
    let instance_admin = org_member(&test, "instance", OrgRole::Admin).await;
    let (other_org, _) = common::organization(&test.scoped(Tenant::Unset), "Another").await;
    let theirs = test.for_org(&other_org).expect("an identifier");
    let their_admin = common::user(&theirs, "their-admin").await;
    let member = common::user(&theirs, "member").await;
    for statement in [
        format!(
            "UPDATE organization_members SET role = 'admin' \
             WHERE org_id = '{other_org}' AND user_id = '{their_admin}'"
        ),
        format!(
            "INSERT INTO organization_members (org_id, user_id, role, created_at) \
             VALUES ('{other_org}', '{instance_admin}', 'member', 1)"
        ),
    ] {
        // Through the other organization's handle: its wall takes its rows only.
        theirs
            .execute_unchecked(&statement)
            .await
            .expect("the roles in the other organization");
    }

    for (user, role) in [
        (&member, OrgRole::Admin),
        (&instance_admin, OrgRole::Admin),
        (&instance_admin, OrgRole::Member),
    ] {
        assert_eq!(
            theirs
                .set_org_role(user, role, &their_admin)
                .await
                .expect("asked"),
            OrgRoleChange::Done,
            "{role:?}"
        );
    }
    let made = test.get_user_by_id(&member).await.expect("the user");
    assert!(!made.is_admin, "no administrator of the instance");
    let kept = test
        .get_user_by_id(&instance_admin)
        .await
        .expect("the user");
    assert!(kept.is_admin, "still the instance's administrator");

    let plain = org_member(&test, "plain", OrgRole::Member).await;
    assert_eq!(
        test.set_org_role(&plain, OrgRole::Admin, &instance_admin)
            .await
            .expect("asked"),
        OrgRoleChange::Done
    );
    let promoted = test.get_user_by_id(&plain).await.expect("the user");
    assert!(promoted.is_admin, "the instance's administrator too");
    let second = org_member(&test, "second", OrgRole::Admin).await;
    assert_eq!(
        test.set_org_role(&second, OrgRole::Member, &instance_admin)
            .await
            .expect("asked"),
        OrgRoleChange::Done
    );
    let stepped_down = test.get_user_by_id(&second).await.expect("the user");
    assert!(
        !stepped_down.is_admin,
        "the instance's administrator no more"
    );
    test.teardown().await;
}

/// A card of the board with a body edited together, which names the card itself: its
/// document, a logged update, a version kept of the body, and what the body names.
async fn card_with_body(store: &Store, board_id: &str, user_id: &str) -> String {
    let id = new_id();
    let mut card = Block {
        body: crate::body_links::body_naming("kept", &[&id]),
        id,
        container_id: board_id.to_owned(),
        block_type: BlockType::Card,
        ..Block::default()
    };
    common::insert_blocks(store, std::slice::from_mut(&mut card), user_id).await;
    store
        .create_body_doc(&card.id, b"a state", b"a digest")
        .await
        .expect("a document is stored");
    let as_read = crate::body_docs::as_read(store, &card.id).await;
    store
        .append_body_updates(
            &card.id,
            &[(b"an update", user_id)],
            as_read,
            &[],
            &[user_id],
            fvoci_store::BodyBefore::Leave,
        )
        .await
        .expect("an update is logged");
    store
        .keep_body_version(&card.id)
        .await
        .expect("a version is kept");
    card.id
}

/// A workspace removed for good takes the documents of its cards' bodies, their logs,
/// the versions kept of the bodies and what the bodies name with it, as a board removed
/// for good does;
/// another workspace's stay.
#[tokio::test]
async fn a_workspace_purged_for_good_takes_the_documents_of_its_bodies() {
    let test = common::setup().await;
    let admin = org_member(&test, "admin", OrgRole::Admin).await;
    let doomed = named_workspace(&test, "Doomed", &admin).await;
    let kept = named_workspace(&test, "Kept", &admin).await;
    let doomed_board = private_board(&test, &doomed, &admin).await;
    let kept_board = private_board(&test, &kept, &admin).await;
    let gone = card_with_body(&test, &doomed_board, &admin).await;
    let stays = card_with_body(&test, &kept_board, &admin).await;

    assert_eq!(
        test.trash_workspace(&doomed, &admin)
            .await
            .expect("trashed"),
        WorkspaceTrashing::Done
    );
    test.purge_workspace(&doomed, i64::MAX)
        .await
        .expect("the workspace is purged");

    assert!(
        test.get_body_doc(&gone).await.expect("read").is_none(),
        "its card's document went"
    );
    assert!(
        test.get_body_doc(&stays).await.expect("read").is_some(),
        "the other workspace's stays"
    );
    for table in [
        "body_docs",
        "body_updates",
        "body_versions",
        "body_version_editors",
        "body_links",
    ] {
        assert_eq!(
            test.count_rows(table).await.expect("counted"),
            1,
            "{table}: the other workspace's alone"
        );
    }
    test.teardown().await;
}

/// Demoted through the instance's route, an administrator of the organization is taken
/// out of what they joined as through the organization's, on the record, and keeps a
/// workspace they administer in their own right: no workspace role follows the instance.
#[tokio::test]
async fn the_instances_route_takes_back_what_was_joined_as_the_organizations_does() {
    let test = common::setup().await;
    let admin = org_member(&test, "admin", OrgRole::Admin).await;
    let other = org_member(&test, "other", OrgRole::Admin).await;
    let joined = named_workspace(&test, "Joined", &admin).await;
    let own = named_workspace(&test, "Own", &admin).await;
    for (workspace, role) in [
        (&joined, WorkspaceRole::Member),
        (&own, WorkspaceRole::Admin),
    ] {
        test.add_workspace_member(workspace, &other, role)
            .await
            .expect("a member");
    }
    assert_eq!(
        test.join_workspace_as_org_admin(&joined, &other)
            .await
            .expect("asked"),
        Joining::Joined
    );

    test.set_user_admin(&other, InstanceRole::User, &admin)
        .await
        .expect("no administrator of the instance any more");
    assert_eq!(
        role_in(&test, &joined, &other).await,
        Some(WorkspaceRole::Member),
        "what they were before joining"
    );
    assert_eq!(
        role_in(&test, &own, &other).await,
        Some(WorkspaceRole::Admin),
        "their own administration stays"
    );
    assert_eq!(
        log_of(&test, &joined).await,
        [
            (AuditAction::OrgAdminJoined, other.clone(), String::new()),
            (AuditAction::OrgAdminRevoked, admin.clone(), other.clone()),
        ]
    );
    assert_eq!(
        organization_records(&test).await.first(),
        Some(&(AuditAction::OrgAdminRemoved, test.org_id.clone())),
        "the change of the role is recorded, the newest, as the organization's route does"
    );
    test.teardown().await;
}

/// A change of a board's members that was let in on what its asker was is made only if
/// they still are that when it is written: an administrator of the organization whose
/// joining was taken back meanwhile adds nobody to the board they made while joined, and
/// gives themselves nothing back. Who still is what they were changes as ever.
#[tokio::test]
async fn a_change_let_in_before_a_joining_was_taken_back_is_not_made() {
    let test = common::setup().await;
    let admin = org_member(&test, "admin", OrgRole::Admin).await;
    let other_admin = org_member(&test, "other.admin", OrgRole::Admin).await;
    let owner = org_member(&test, "owner", OrgRole::Member).await;
    let accomplice = org_member(&test, "accomplice", OrgRole::Member).await;
    let workspace = named_workspace(&test, "Work", &admin).await;
    for user in [&owner, &accomplice] {
        test.add_workspace_member(&workspace, user, WorkspaceRole::Member)
            .await
            .expect("a member");
    }
    test.join_workspace_as_org_admin(&workspace, &admin)
        .await
        .expect("joined");
    let made = private_board(&test, &workspace, &admin).await;
    let seen = test
        .get_member_for_container(&made, &admin)
        .await
        .expect("its admin");
    let stale = Standing {
        user_id: &admin,
        workspace_role: Some(WorkspaceRole::Admin),
        member: Some(&seen),
        team_role: None,
    };
    test.set_org_role(&admin, OrgRole::Member, &other_admin)
        .await
        .expect("taken back");

    let as_admin = |user_id: &str| ContainerMember {
        container_id: made.clone(),
        user_id: user_id.to_owned(),
        role: fvoci_model::ContainerRole::Admin,
        synthetic: false,
        inherited_role: None,
    };
    let added = test
        .add_member_asked(&as_admin(&accomplice), stale)
        .await
        .expect_err("not as they were");
    assert!(added.is_not_found(), "{added:?}");
    let changed = test
        .change_member_asked(&as_admin(&admin), stale)
        .await
        .expect_err("not as they were");
    assert!(changed.is_not_found(), "{changed:?}");
    for user in [&accomplice, &admin] {
        let found = test.get_member_for_container(&made, user).await;
        assert!(found.is_err_and(|error| error.is_not_found()), "{user}");
    }

    let theirs = private_board(&test, &workspace, &owner).await;
    let owner_row = test
        .get_member_for_container(&theirs, &owner)
        .await
        .expect("its admin");
    let holds = Standing {
        user_id: &owner,
        workspace_role: Some(WorkspaceRole::Member),
        member: Some(&owner_row),
        team_role: None,
    };
    let viewer = ContainerMember {
        container_id: theirs.clone(),
        user_id: accomplice.clone(),
        role: fvoci_model::ContainerRole::Viewer,
        synthetic: false,
        inherited_role: None,
    };
    assert_eq!(
        test.add_member_asked(&viewer, holds)
            .await
            .expect("as they were"),
        None,
        "a new membership"
    );
    test.teardown().await;
}

/// A workspace that goes for good takes its boards through the trash: each is noted
/// for the search server, which is to hold nothing of them.
#[tokio::test]
async fn the_boards_of_a_workspace_that_goes_for_good_are_noted_for_the_search() {
    let test = common::setup().await;
    test.keep_search_outbox();
    let admin = org_member(&test, "admin", OrgRole::Admin).await;
    let doomed = named_workspace(&test, "Doomed", &admin).await;
    let board = private_board(&test, &doomed, &admin).await;
    assert_eq!(
        test.trash_workspace(&doomed, &admin)
            .await
            .expect("trashed"),
        WorkspaceTrashing::Done
    );
    let in_the_trash = test
        .due_search_changes(i64::MAX, 10)
        .await
        .expect("the list is read");

    test.purge_workspace(&doomed, i64::MAX)
        .await
        .expect("purged");
    let gone: Vec<(fvoci_store::SearchTarget, String)> = test
        .due_search_changes(i64::MAX, 10)
        .await
        .expect("the list is read")
        .into_iter()
        .map(|change| (change.target, change.id))
        .collect();
    test.teardown().await;

    assert_eq!(
        in_the_trash,
        [],
        "in the trash it is there for nobody, as it is"
    );
    assert_eq!(gone, [(fvoci_store::SearchTarget::Container, board)]);
}
