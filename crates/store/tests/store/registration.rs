//! Who registers through a handle of an organization, and what being the instance's
//! administrator makes of them in the organization and its workspaces.

use fvoci_model::{InstanceRole, OrgKind, OrgRole, WorkspaceRole, new_id};
use fvoci_store::Tenant;

use crate::common;

/// Whoever registers through a handle of an organization joins it and its default
/// workspace, the first user as an administrator of both; an administrator of the
/// instance made later is the organization's only while one, their workspace roles
/// kept. A user stored through a handle that names no organization is in none but
/// their own space.
#[tokio::test]
async fn whoever_registers_joins_the_organization_and_its_default_workspace() {
    let test = common::setup().await;
    let unset = test.scoped(Tenant::Unset);
    let second_workspace = common::workspace(&test).await;
    let first = common::user(&test, "first").await;
    let second = common::user(&test, "second").await;
    let outside = common::user(&unset, "outside").await;

    let access = async |user_id: &str, workspace_id: &str| {
        unset
            .resolve_workspace(user_id, workspace_id)
            .await
            .expect("the workspace is resolved")
            .map(|access| {
                assert_eq!(access.org_id, test.org_id, "whose the workspace is");
                (access.org_role, access.workspace_role)
            })
    };
    let admin = Some((Some(OrgRole::Admin), WorkspaceRole::Admin));
    let member = Some((Some(OrgRole::Member), WorkspaceRole::Member));
    assert_eq!(
        access(&first, &test.workspace_id).await,
        admin,
        "the first user"
    );
    assert_eq!(access(&second, &test.workspace_id).await, member);
    assert_eq!(
        access(&second, &second_workspace).await,
        None,
        "the default workspace only"
    );
    assert_eq!(access(&outside, &test.workspace_id).await, None);
    assert_eq!(access(&first, &new_id()).await, None, "no such workspace");
    assert_eq!(access(&first, "no identifier").await, None);
    let of_outside = unset
        .workspaces_of_user(&outside, fvoci_store::Making::with_password())
        .await;
    let kinds: Vec<OrgKind> = of_outside
        .expect("the workspaces are listed")
        .iter()
        .map(|workspace| workspace.kind)
        .collect();
    assert_eq!(kinds, [OrgKind::Personal], "of their own space only");

    // A member is let into the second workspace, as a guest.
    test.add_workspace_member(&second_workspace, &second, WorkspaceRole::Guest)
        .await
        .expect("the member is added");
    let listed = unset
        .workspaces_of_user(&second, fvoci_store::Making::with_password())
        .await
        .expect("the workspaces are listed");
    let (own, listed) = listed.split_last().expect("the user's own space, last");
    assert_eq!(own.kind, OrgKind::Personal, "{own:?}");
    let listed: Vec<(&str, WorkspaceRole)> = listed
        .iter()
        .map(|workspace| {
            assert_eq!(workspace.org_id, test.org_id);
            (workspace.id.as_str(), workspace.role)
        })
        .collect();
    assert_eq!(
        listed,
        [
            (test.workspace_id.as_str(), WorkspaceRole::Member),
            (second_workspace.as_str(), WorkspaceRole::Guest)
        ],
        "oldest workspace first"
    );
    let missing = test
        .add_workspace_member(&new_id(), &second, WorkspaceRole::Member)
        .await
        .expect_err("no such workspace");
    assert!(missing.is_not_found(), "got {missing:?}");

    // The role in the organization follows the instance's administrators, both ways;
    // a member's and a guest's workspace roles do not (entering one is a joining).
    let roles = [
        (InstanceRole::Administrator, OrgRole::Admin),
        (InstanceRole::User, OrgRole::Member),
    ];
    let kept = [
        (&test.workspace_id, WorkspaceRole::Member),
        (&second_workspace, WorkspaceRole::Guest),
    ];
    for (role, org_role) in roles {
        test.set_user_admin(&second, role, &first)
            .await
            .expect("set by the first, an administrator");
        for (workspace, kept) in kept {
            let found = access(&second, workspace).await;
            assert_eq!(found, Some((Some(org_role), kept)), "{role:?}");
        }
    }

    // Somebody the organization takes in without a workspace is in none of them.
    test.add_organization_member(&outside, OrgRole::Member)
        .await
        .expect("the user is added");
    assert_eq!(access(&outside, &test.workspace_id).await, None);
    // And somebody let into a workspace without belonging to the organization is in
    // the workspace, as nothing in the organization.
    let visitor = common::user(&unset, "visitor").await;
    test.add_workspace_member(&second_workspace, &visitor, WorkspaceRole::Guest)
        .await
        .expect("the visitor is let in");
    assert_eq!(
        access(&visitor, &second_workspace).await,
        Some((None, WorkspaceRole::Guest))
    );

    test.teardown().await;
}
