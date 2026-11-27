//! Team grants are rechecked at the write and cannot outlive temporary administration.

use crate::boards::{board_with_admin, new_board};
use crate::common::{ROOT_WORKSPACE_ID, Setup, TestServer};
use crate::org_admin::{let_in, make_workspace, org_of};
use fvoci_model::{
    AuditAction, ContainerMember, ContainerRole, ContainerVisibility, MinimumRole, Team,
    WorkspaceMember, WorkspaceRole,
};
use fvoci_store::{Acceptance, Acceptor, Inviter, InviterOfContainer, NewLink, Standing};
use reqwest::StatusCode;
use serde_json::json;

#[tokio::test]
async fn a_revoked_team_role_invalidates_direct_grants_and_invitation_acceptance() {
    let th = Setup::native().start().await.init_basic().await;
    let manager = th.client.me().await.id;
    let owner = th.client2.me().await.id;
    let board = board_with_admin(
        &th,
        new_board(
            ROOT_WORKSPACE_ID,
            ContainerVisibility::Private,
            "Racing access",
        ),
        &owner,
    )
    .await;
    let team = th
        .app
        .create_team(ROOT_WORKSPACE_ID, "Readers", &manager)
        .await
        .unwrap();
    th.app
        .add_team_member(ROOT_WORKSPACE_ID, &team.id, &manager, &manager)
        .await
        .unwrap();
    th.app
        .set_container_team(
            ROOT_WORKSPACE_ID,
            &board.id,
            &team.id,
            Some(MinimumRole::Viewer),
            &manager,
        )
        .await
        .unwrap();
    let access = th
        .app
        .container_access(&manager, &board.id)
        .await
        .unwrap()
        .unwrap();
    assert!(access.grant.reads());
    let requested = ContainerMember {
        container_id: board.id.clone(),
        user_id: manager.clone(),
        role: ContainerRole::Viewer,
        synthetic: false,
        inherited_role: None,
    };
    th.app
        .remove_team_member(ROOT_WORKSPACE_ID, &team.id, &manager, &manager)
        .await
        .unwrap();
    assert!(
        th.app
            .add_member_asked(&requested, &manager, &access)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        th.app
            .store()
            .get_member_for_container(&board.id, &manager)
            .await
            .unwrap_err()
            .is_not_found()
    );

    stale_invitation(&th, &manager, &board.id, &team.id).await;
    let before = th
        .app
        .store()
        .get_audit_log(ROOT_WORKSPACE_ID)
        .await
        .unwrap()
        .len();
    th.client
        .post(
            &format!("/w/{ROOT_WORKSPACE_ID}/containers/{}/members", board.id),
            &json!({"userId": manager, "role": "viewer"}),
        )
        .await
        .check_ok();
    let records = th
        .app
        .store()
        .get_audit_log(ROOT_WORKSPACE_ID)
        .await
        .unwrap();
    assert_eq!(
        records.len(),
        before + 1,
        "a fresh request records management-only access"
    );
    assert!(records.iter().any(|r| r.target_id == board.id
        && r.actor.person_id() == Some(manager.as_str())
        && r.action == AuditAction::SelfAddedToContainer));
    th.teardown().await;
}

async fn stale_invitation(th: &TestServer, manager: &str, board_id: &str, team_id: &str) {
    let colleague = th.add_user("colleague").await.me().await.id;
    th.app
        .store()
        .add_member(&ContainerMember {
            container_id: board_id.to_owned(),
            user_id: colleague.clone(),
            role: ContainerRole::Viewer,
            synthetic: false,
            inherited_role: None,
        })
        .await
        .expect("store a colleague viewer on the board");
    th.app
        .add_team_member(ROOT_WORKSPACE_ID, team_id, manager, manager)
        .await
        .expect("restore team reading");
    let access = th
        .app
        .container_access(manager, board_id)
        .await
        .expect("read access facts")
        .expect("the board exists");
    th.app
        .store()
        .create_invitation(&NewLink {
            workspace_id: ROOT_WORKSPACE_ID,
            guest_of: Some((board_id, ContainerRole::Viewer)),
            token_hash: "stale-team-inviter",
            created_by: manager,
            now: 1,
            expires_at: 100,
            max_uses: None,
            mailed: None,
        })
        .await
        .expect("create the invitation");
    th.app
        .remove_team_member(ROOT_WORKSPACE_ID, team_id, manager, manager)
        .await
        .expect("revoke the team role");
    let colleague_update = ContainerMember {
        container_id: board_id.to_owned(),
        user_id: colleague,
        role: ContainerRole::Editor,
        synthetic: false,
        inherited_role: None,
    };
    assert!(
        th.app
            .update_member_asked(&colleague_update, manager, &access)
            .await
            .expect("check the stale role change")
            .is_none(),
        "a revoked team role cannot authorize a direct role change"
    );
    let acceptor = th.stranger().await;
    let result = th
        .app
        .store()
        .accept_invitation(
            "stale-team-inviter",
            Acceptor::Account(&acceptor),
            2,
            Inviter {
                standing: Standing {
                    user_id: manager,
                    workspace_role: access.actor.workspace,
                    member: access.member.as_ref(),
                    team_role: access.team_role,
                },
                of_container: InviterOfContainer::Member,
            },
        )
        .await
        .expect("recheck the inviter");
    assert_eq!(
        result,
        Acceptance::InviterChanged,
        "the maker lost inherited access"
    );
    assert!(
        th.app
            .store()
            .get_member_for_container(board_id, &acceptor)
            .await
            .expect_err("the invitation added nobody")
            .is_not_found(),
        "the stale invitation must leave no board membership"
    );
}

#[tokio::test]
async fn a_joined_organization_admin_cannot_add_self_but_another_admin_can() {
    let th = Setup::native().start().await.init_basic().await;
    let org = org_of(&th).await;
    let manager = th.client.me().await.id;
    let other = th.client2.me().await.id;
    let colleague = th.stranger().await;
    let workspace = make_workspace(&th, &org, "Standing teams").await;
    let_in(&th, &workspace, &manager, WorkspaceRole::Member).await;
    let_in(&th, &workspace, &other, WorkspaceRole::Admin).await;
    let_in(&th, &workspace, &colleague, WorkspaceRole::Member).await;
    let path = format!("/w/{workspace}/teams");
    let prior: Team = th
        .client2
        .post(&path, &json!({"name":"Prior"}))
        .await
        .ok_json();
    th.client2
        .put(
            &format!("{path}/{}/members/{manager}", prior.id),
            &json!({}),
        )
        .await
        .check_no_content();
    let joining = format!("/org/{org}/workspaces/{workspace}");
    th.client
        .post_empty(&format!("{joining}/join"))
        .await
        .check_ok();
    let (team, board) = independently_added(&th, &workspace, &manager, &other, &colleague).await;
    th.client
        .post_empty(&format!("{joining}/leave"))
        .await
        .check_ok();
    for id in [&team.id, &prior.id] {
        let read: Team = th.client.get(&format!("{path}/{id}")).await.ok_json();
        assert!(read.members.iter().any(|m| m.user_id == manager));
    }
    let members: Vec<WorkspaceMember> = th
        .client2
        .get(&format!("/w/{workspace}/members"))
        .await
        .ok_json();
    let restored = members.iter().find(|m| m.user_id == manager).unwrap();
    assert!(
        th.app
            .container_grant(&manager, &board.id)
            .await
            .unwrap()
            .reads()
    );
    assert_eq!(restored.role, WorkspaceRole::Member);
    assert!(!restored.joined_as_org_admin);
    th.teardown().await;
}

async fn independently_added(
    th: &TestServer,
    workspace: &str,
    manager: &str,
    other: &str,
    colleague: &str,
) -> (Team, fvoci_model::Container) {
    let store = th.app.store();
    let path = format!("/w/{workspace}/teams");
    let members: Vec<WorkspaceMember> = th
        .client
        .get(&format!("/w/{workspace}/members"))
        .await
        .ok_json();
    assert!(
        members
            .iter()
            .find(|m| m.user_id == manager)
            .expect("the joined administrator is listed")
            .joined_as_org_admin,
        "the workspace response exposes temporary administration"
    );
    let team: Team = th
        .client
        .post(&path, &json!({"name":"New"}))
        .await
        .ok_json();
    let board = board_with_admin(
        th,
        new_board(workspace, ContainerVisibility::Private, "Independent grant"),
        colleague,
    )
    .await;
    th.app
        .set_container_team(
            workspace,
            &board.id,
            &team.id,
            Some(MinimumRole::Viewer),
            other,
        )
        .await
        .expect("assign the team to the board");
    let before = store
        .get_audit_log(workspace)
        .await
        .expect("read the audit before refusal")
        .len();
    th.client
        .put(
            &format!("{path}/{}/members/{}", team.id, crate::common::UNKNOWN_ID),
            &json!({}),
        )
        .await
        .check_status(StatusCode::NOT_FOUND);
    let mine = format!("{path}/{}/members/{manager}", team.id);
    let refused = th.client.put(&mine, &json!({})).await;
    refused.check_status(StatusCode::FORBIDDEN);
    assert_eq!(
        refused.value()["reason"],
        "team_self_joining",
        "the refusal names the rule"
    );
    assert_eq!(
        refused.value()["type"],
        "/problems/team-self-joining",
        "the rule has a problem page"
    );
    assert_eq!(
        store
            .get_audit_log(workspace)
            .await
            .expect("read the audit after refusal")
            .len(),
        before,
        "the refused self-add leaves no audit entry"
    );
    th.client
        .put(
            &format!("{path}/{}/members/{colleague}", team.id),
            &json!({}),
        )
        .await
        .check_no_content();
    th.client2.put(&mine, &json!({})).await.check_no_content();
    let records = store
        .get_audit_log(workspace)
        .await
        .expect("read the independent grant audit");
    assert!(
        records.iter().any(|r| r.target_id == board.id
            && r.actor.person_id() == Some(other)
            && r.subject_id == manager
            && r.action == AuditAction::AddedToContainer),
        "the other administrator is recorded granting access"
    );
    (team, board)
}
