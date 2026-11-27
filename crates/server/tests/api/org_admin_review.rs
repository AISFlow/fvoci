//! Independent review regressions for organization administration and its older entry points.

use fvoci_model::{Container, ContainerMember, ContainerVisibility, UserWorkspace, WorkspaceRole};
use serde_json::{Value, json};

use crate::common::{Setup, TestServer};

/// A new workspace, with the second user as an ordinary member and the first joined
/// through the recorded organization route.
async fn workspace_with_members(ts: &TestServer) -> (String, String) {
    let own: Vec<UserWorkspace> = ts.client.get("/users/me/workspaces").await.ok_json();
    let org = own
        .first()
        .expect("the administrator's organization")
        .org_id
        .clone();
    let made: Value = ts
        .client
        .post(
            &format!("/org/{org}/workspaces"),
            &json!({"name": "Review workspace"}),
        )
        .await
        .created_json();
    let workspace = made["id"]
        .as_str()
        .expect("the workspace identifier")
        .to_owned();
    ts.client
        .post_empty(&format!("/org/{org}/workspaces/{workspace}/join"))
        .await
        .check_ok();
    ts.app
        .store()
        .add_workspace_member(&workspace, &ts.client2.me().await.id, WorkspaceRole::Member)
        .await
        .expect("an ordinary workspace member");
    (org, workspace)
}

/// Promoting an organization administrator through the instance route must not also
/// silently perform the separately recorded workspace joining.
#[tokio::test]
async fn instance_promotion_does_not_grant_unrecorded_workspace_administration() {
    let ts = Setup::native().start().await.init_basic().await;
    let (org, workspace) = workspace_with_members(&ts).await;
    let promoted = ts.client2.me().await.id;
    ts.client
        .put(
            &format!("/users/{promoted}/admin"),
            &json!({"isAdmin": true}),
        )
        .await
        .check_ok();
    let workspaces: Vec<Value> = ts
        .client2
        .get(&format!("/org/{org}/workspaces"))
        .await
        .ok_json();
    let records: Vec<Value> = ts
        .client2
        .get(&format!("/w/{workspace}/audit"))
        .await
        .ok_json();
    ts.teardown().await;

    let ours = workspaces
        .iter()
        .find(|row| row["id"] == workspace)
        .expect("the workspace is listed");
    assert_eq!(
        ours["role"],
        json!("member"),
        "organization promotion alone must preserve the workspace role; records: {records:?}"
    );
}

/// An administration gained through the instance route must be removed when that
/// same administrator is demoted through the organization's route.
#[tokio::test]
async fn organization_demotion_revokes_workspace_power_granted_by_the_instance_route() {
    let ts = Setup::native().start().await.init_basic().await;
    let (org, workspace) = workspace_with_members(&ts).await;
    let promoted = ts.client2.me().await.id;
    let board: Container = ts
        .client
        .create_board(&Container {
            workspace_id: workspace.clone(),
            visibility: ContainerVisibility::Private,
            title: "Private plans".to_owned(),
            ..Container::default()
        })
        .await
        .created_json();
    ts.client
        .put(
            &format!("/users/{promoted}/admin"),
            &json!({"isAdmin": true}),
        )
        .await
        .check_ok();
    ts.client
        .put(
            &format!("/org/{org}/members/{promoted}/role"),
            &json!({"role": "member"}),
        )
        .await
        .check_no_content();
    assert!(
        !ts.client2.me().await.is_admin,
        "the organization route demoted the instance role"
    );
    let joined = ts
        .client2
        .add_member(&ContainerMember {
            container_id: board.id,
            user_id: promoted,
            role: fvoci_model::ContainerRole::Viewer,
            synthetic: false,
            inherited_role: None,
        })
        .await;
    ts.teardown().await;

    joined.check_not_found();
}
