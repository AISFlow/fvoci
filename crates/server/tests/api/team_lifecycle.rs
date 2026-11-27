//! Board trash and default template access do not freeze a team's membership.

use crate::boards::{board_with_admin, new_board};
use crate::common::{ROOT_WORKSPACE_ID, Setup, TestServer};
use fvoci_model::{
    AuditAction, Block, BlockType, ContainerTeam, ContainerVisibility, Subscription, Team, new_id,
};
use reqwest::StatusCode;
use serde_json::json;

async fn assigned_team(th: &TestServer, board_path: &str, user: &str, name: &str) -> Team {
    let team: Team = th
        .client
        .post(
            &format!("/w/{ROOT_WORKSPACE_ID}/teams"),
            &json!({"name":name}),
        )
        .await
        .ok_json();
    th.client
        .put(&member_path(&team, user), &json!({}))
        .await
        .check_no_content();
    th.client
        .put(
            &format!("{board_path}/teams/{}", team.id),
            &json!({"role":"editor"}),
        )
        .await
        .check_no_content();
    team
}

fn member_path(team: &Team, user: &str) -> String {
    format!("/w/{ROOT_WORKSPACE_ID}/teams/{}/members/{user}", team.id)
}

#[tokio::test]
async fn board_restore_keeps_surviving_teams_and_uses_their_current_people() {
    let th = Setup::native().start().await.init_basic().await;
    let owner = th.client.me().await.id;
    let user = th.client2.me().await.id;
    let board = board_with_admin(
        &th,
        new_board(
            ROOT_WORKSPACE_ID,
            ContainerVisibility::Private,
            "Restore teams",
        ),
        &owner,
    )
    .await;
    let path = format!("/w/{ROOT_WORKSPACE_ID}/containers/{}", board.id);
    let surviving = assigned_team(&th, &path, &user, "Surviving").await;
    let deleted = assigned_team(&th, &path, &user, "Deleted").await;
    th.client2.get(&path).await.check_ok();
    th.client.delete(&path).await.check_no_content();
    th.client2
        .get(&path)
        .await
        .check_status(StatusCode::NOT_FOUND);
    th.client2
        .post(&format!("{path}/restore"), &json!({}))
        .await
        .check_status(StatusCode::NOT_FOUND);
    th.client
        .delete(&member_path(&surviving, &user))
        .await
        .check_no_content();
    let newcomer = th.stranger().await;
    th.client
        .put(&member_path(&surviving, &newcomer), &json!({}))
        .await
        .check_no_content();
    th.client
        .delete(&format!("/w/{ROOT_WORKSPACE_ID}/teams/{}", deleted.id))
        .await
        .check_no_content();
    th.client
        .post(&format!("{path}/restore"), &json!({}))
        .await
        .check_ok();
    let assigned: Vec<ContainerTeam> = th.client.get(&format!("{path}/teams")).await.ok_json();
    assert_eq!(assigned.len(), 1, "deleted teams stay deleted");
    assert_eq!(assigned[0].team.id, surviving.id);
    assert_eq!(assigned[0].role.as_str(), "editor");
    th.client2
        .get(&path)
        .await
        .check_status(StatusCode::NOT_FOUND);
    assert!(
        th.app
            .container_grant(&newcomer, &board.id)
            .await
            .unwrap()
            .reads(),
        "membership is read at restoration, not snapshotted at deletion"
    );
    th.teardown().await;
}

#[tokio::test]
async fn team_removal_preserves_template_subscriptions_until_workspace_membership_ends() {
    for departure in [false, true] {
        let th = Setup::native().start().await.init_basic().await;
        let owner = th.client.me().await.id;
        let user = th.client2.me().await.id;
        let mut template = new_board(
            ROOT_WORKSPACE_ID,
            ContainerVisibility::Open,
            "Open template",
        );
        template.is_template = true;
        let board = board_with_admin(&th, template, &owner).await;
        let path = format!("/w/{ROOT_WORKSPACE_ID}/containers/{}", board.id);
        let team = assigned_team(&th, &path, &user, "Template editors").await;
        let mut block = Block {
            id: new_id(),
            container_id: board.id.clone(),
            block_type: BlockType::Card,
            ..Block::default()
        };
        th.app
            .store()
            .insert_block(&mut block, &owner)
            .await
            .unwrap();
        th.client2
            .post(
                &format!("/w/{ROOT_WORKSPACE_ID}/subscriptions"),
                &json!({"blockType":"card", "blockId":block.id, "userId":user}),
            )
            .await
            .check_created();
        let subscriptions_path = format!("/w/{ROOT_WORKSPACE_ID}/subscriptions/{user}");
        th.client
            .delete(&member_path(&team, &user))
            .await
            .check_no_content();
        th.client2.get(&path).await.check_ok();
        let subscriptions: Vec<Subscription> = th.client2.get(&subscriptions_path).await.ok_json();
        assert_eq!(
            subscriptions.len(),
            1,
            "default template viewing still permits the subscription"
        );
        let membership = format!("/w/{ROOT_WORKSPACE_ID}/members/{user}");
        if departure {
            th.client.delete(&membership).await.check_no_content();
        } else {
            th.client
                .put(&membership, &json!({"role":"guest"}))
                .await
                .check_ok();
        }
        th.client2
            .get(&path)
            .await
            .check_status(StatusCode::NOT_FOUND);
        if departure {
            th.client2
                .get(&subscriptions_path)
                .await
                .check_status(StatusCode::NOT_FOUND);
        } else {
            th.client2.get(&subscriptions_path).await.check_ok();
        }
        let subscriptions = th
            .app
            .store()
            .get_subscriptions_in_workspace(&user, ROOT_WORKSPACE_ID)
            .await
            .unwrap();
        assert!(
            subscriptions.is_empty(),
            "the former member has no default template access"
        );
        th.teardown().await;
    }
}

async fn empty_team(th: &TestServer, name: &str) -> Team {
    th.client
        .post(
            &format!("/w/{ROOT_WORKSPACE_ID}/teams"),
            &json!({"name":name}),
        )
        .await
        .ok_json()
}

/// The board's records of people put on it: (action, actor, subject).
async fn records_of(th: &TestServer, board_id: &str) -> Vec<(&'static str, String, String)> {
    th.app
        .store()
        .get_audit_log(ROOT_WORKSPACE_ID)
        .await
        .expect("the audit log is read")
        .into_iter()
        .filter(|r| {
            r.target_id == board_id
                && matches!(
                    r.action,
                    AuditAction::SelfAddedToContainer | AuditAction::AddedToContainer
                )
        })
        .map(|r| {
            let actor = r.actor.person_id().unwrap_or_default().to_owned();
            (r.action.as_str(), actor, r.subject_id)
        })
        .collect()
}

#[tokio::test]
async fn a_manager_who_restores_a_board_through_its_teams_puts_everyone_it_opens_to_on_the_record()
{
    let th = Setup::native().start().await.init_basic().await;
    let manager = th.client.me().await.id;
    let owner = th.client2.me().await.id;
    let colleague = th.stranger().await;
    let board = board_with_admin(
        &th,
        new_board(
            ROOT_WORKSPACE_ID,
            ContainerVisibility::Private,
            "Restored managed",
        ),
        &owner,
    )
    .await;
    let path = format!("/w/{ROOT_WORKSPACE_ID}/containers/{}", board.id);
    // The manager's team sorts first: letting the manager in must not hide the second.
    let mine = empty_team(&th, "Joined in the trash").await;
    let theirs = empty_team(&th, "Colleagues").await;
    for team in [&mine, &theirs] {
        th.client2
            .put(
                &format!("{path}/teams/{}", team.id),
                &json!({"role":"viewer"}),
            )
            .await
            .check_no_content();
    }
    th.client2.delete(&path).await.check_no_content();
    for (team, user) in [(&mine, &manager), (&theirs, &colleague)] {
        th.client
            .put(&member_path(team, user), &json!({}))
            .await
            .check_no_content();
    }
    assert_eq!(
        records_of(&th, &board.id).await,
        Vec::new(),
        "a board in the trash opens nothing"
    );
    th.client
        .post(&format!("{path}/restore"), &json!({}))
        .await
        .check_ok();
    th.client.get(&path).await.check_ok();
    let mut records = records_of(&th, &board.id).await;
    records.sort();
    let mut expected = vec![
        (
            AuditAction::SelfAddedToContainer.as_str(),
            manager.clone(),
            String::new(),
        ),
        (
            AuditAction::AddedToContainer.as_str(),
            manager.clone(),
            colleague.clone(),
        ),
    ];
    expected.sort();
    assert_eq!(
        records, expected,
        "both whom the teams let in are on the record"
    );
    th.teardown().await;
}

#[tokio::test]
async fn an_admin_of_the_board_who_restores_it_records_nobody() {
    let th = Setup::native().start().await.init_basic().await;
    let manager = th.client.me().await.id;
    let board = board_with_admin(
        &th,
        new_board(
            ROOT_WORKSPACE_ID,
            ContainerVisibility::Private,
            "Restored by its admin",
        ),
        &manager,
    )
    .await;
    let path = format!("/w/{ROOT_WORKSPACE_ID}/containers/{}", board.id);
    let team = empty_team(&th, "With the admin").await;
    th.client
        .put(&member_path(&team, &manager), &json!({}))
        .await
        .check_no_content();
    th.client
        .put(
            &format!("{path}/teams/{}", team.id),
            &json!({"role":"viewer"}),
        )
        .await
        .check_no_content();
    th.client.delete(&path).await.check_no_content();
    th.client
        .post(&format!("{path}/restore"), &json!({}))
        .await
        .check_ok();
    assert_eq!(
        records_of(&th, &board.id).await,
        Vec::new(),
        "who reads the board as its admin again is let in by nothing new"
    );
    th.teardown().await;
}

#[tokio::test]
async fn giving_a_team_the_role_it_has_or_taking_one_it_lacks_changes_nobody() {
    use fvoci_model::MinimumRole::{Editor, Viewer};
    let th = Setup::native().start().await.init_basic().await;
    let manager = th.client.me().await.id;
    let user = th.client2.me().await.id;
    let board = board_with_admin(
        &th,
        new_board(ROOT_WORKSPACE_ID, ContainerVisibility::Private, "Unchanged"),
        &manager,
    )
    .await;
    let team = th
        .app
        .create_team(ROOT_WORKSPACE_ID, "Steady", &manager)
        .await
        .expect("the team is made");
    th.app
        .add_team_member(ROOT_WORKSPACE_ID, &team.id, &user, &manager)
        .await
        .expect("the person joins the team");
    for (role, changed, why) in [
        (None, 0, "no role to take away"),
        (Some(Editor), 1, "a role given"),
        (Some(Editor), 0, "the role it has"),
        (Some(Viewer), 1, "a role lowered"),
        (None, 1, "the role taken away"),
        (None, 0, "no role left to take away"),
    ] {
        let changes = th
            .app
            .store()
            .set_container_team(ROOT_WORKSPACE_ID, &board.id, &team.id, role, &manager)
            .await
            .expect("the role is set");
        assert_eq!(changes.len(), changed, "{why}: {changes:?}");
    }
    th.teardown().await;
}

#[tokio::test]
async fn adding_a_person_already_in_the_team_changes_nobody() {
    use fvoci_model::MinimumRole::Editor;
    let th = Setup::native().start().await.init_basic().await;
    let manager = th.client.me().await.id;
    let user = th.client2.me().await.id;
    let board = board_with_admin(
        &th,
        new_board(ROOT_WORKSPACE_ID, ContainerVisibility::Private, "Once"),
        &manager,
    )
    .await;
    let team = th
        .app
        .create_team(ROOT_WORKSPACE_ID, "Already", &manager)
        .await
        .expect("the team is made");
    th.app
        .set_container_team(
            ROOT_WORKSPACE_ID,
            &board.id,
            &team.id,
            Some(Editor),
            &manager,
        )
        .await
        .expect("the team has a role on the board");
    for (changed, why) in [(1, "a person joins"), (0, "the person is in it already")] {
        let changes = th
            .app
            .store()
            .add_team_member(ROOT_WORKSPACE_ID, &team.id, &user, &manager)
            .await
            .expect("the person is in the team");
        assert_eq!(changes.len(), changed, "{why}: {changes:?}");
    }
    th.teardown().await;
}
