//! Team grants reach the board and its discovery paths, and disappear at the source.

use crate::boards::{board_with_admin, new_board};
use crate::common::{ROOT_WORKSPACE_ID, Setup, TestServer};
use crate::references::{HIDDEN, check_as_nothing, made, named, resolved};
use fvoci_model::{
    AuditAction, Container, ContainerMember, ContainerRole, ContainerVisibility, Team,
};
use reqwest::StatusCode;
use serde_json::json;

async fn group(th: &TestServer, user: &str, name: &str) -> Team {
    let team: Team = th
        .client
        .post(
            &format!("/w/{ROOT_WORKSPACE_ID}/teams"),
            &json!({"name":name}),
        )
        .await
        .ok_json();
    th.client
        .put(
            &format!("/w/{ROOT_WORKSPACE_ID}/teams/{}/members/{user}", team.id),
            &json!({}),
        )
        .await
        .check_no_content();
    team
}

#[tokio::test]
async fn team_roles_open_edit_find_and_export_a_board_then_end_when_removed() {
    let th = Setup::native().start().await.init_basic().await;
    let owner = th.client.me().await.id;
    let user = th.client2.me().await.id;
    let board = board_with_admin(
        &th,
        new_board(
            ROOT_WORKSPACE_ID,
            ContainerVisibility::Private,
            "Inherited design",
        ),
        &owner,
    )
    .await;
    let path = format!("/w/{ROOT_WORKSPACE_ID}/containers/{}", board.id);
    let team = group(&th, &user, "Design").await;
    let assignment = format!("{path}/teams/{}", team.id);
    th.client2
        .get(&path)
        .await
        .check_status(StatusCode::NOT_FOUND);
    th.client
        .put(&assignment, &json!({"role":"admin"}))
        .await
        .check_status(StatusCode::BAD_REQUEST);
    th.client
        .put(&assignment, &json!({"role":"editor"}))
        .await
        .check_no_content();
    th.client2.get(&path).await.check_ok();
    th.client2
        .patch(&path, &json!({"title":"Inherited edit"}))
        .await
        .check_ok();
    th.client2
        .put(&assignment, &json!({"role":"viewer"}))
        .await
        .check_status(StatusCode::FORBIDDEN);
    let members: Vec<ContainerMember> = th.client2.get("/users/me/memberships").await.ok_json();
    let membership = members
        .iter()
        .find(|m| m.container_id == board.id)
        .expect("inherited board in memberships");
    assert!(membership.synthetic);
    assert_eq!(membership.role, ContainerRole::Editor);
    assert_eq!(membership.effective_role(), ContainerRole::Editor);
    for suffix in ["/boards", "/boards/search?q=Inherited"] {
        let listed: Vec<Container> = th
            .client2
            .get(&format!("/w/{ROOT_WORKSPACE_ID}{suffix}"))
            .await
            .ok_json();
        assert!(
            listed.iter().any(|b| b.id == board.id),
            "{suffix}: team board is discoverable"
        );
    }
    th.client2
        .get(&format!("{path}/archive/export"))
        .await
        .check_ok();
    th.client
        .delete(&format!(
            "/w/{ROOT_WORKSPACE_ID}/teams/{}/members/{user}",
            team.id
        ))
        .await
        .check_no_content();
    th.client2
        .get(&path)
        .await
        .check_status(StatusCode::NOT_FOUND);
    th.client2
        .get(&format!("{path}/archive/export"))
        .await
        .check_status(StatusCode::NOT_FOUND);
    let members: Vec<ContainerMember> = th.client2.get("/users/me/memberships").await.ok_json();
    assert!(!members.iter().any(|m| m.container_id == board.id));
    th.teardown().await;
}

#[tokio::test]
async fn the_highest_direct_or_team_role_survives_until_its_source_ends() {
    let th = Setup::native().start().await.init_basic().await;
    let owner = th.client.me().await.id;
    let user = th.client2.me().await.id;
    let board = board_with_admin(
        &th,
        new_board(ROOT_WORKSPACE_ID, ContainerVisibility::Private, "Roles"),
        &owner,
    )
    .await;
    let path = format!("/w/{ROOT_WORKSPACE_ID}/containers/{}", board.id);
    let first = group(&th, &user, "Readers").await;
    let second = group(&th, &user, "Editors").await;
    for (team, role) in [(&first, "commenter"), (&second, "editor")] {
        th.client
            .put(&format!("{path}/teams/{}", team.id), &json!({"role":role}))
            .await
            .check_no_content();
    }
    th.client
        .post(
            &format!("{path}/members"),
            &json!({"userId":user,"role":"viewer"}),
        )
        .await
        .check_ok();
    assert!(
        th.app
            .container_grant(&user, &board.id)
            .await
            .unwrap()
            .reads()
    );
    th.client2
        .patch(&path, &json!({"title":"Can edit"}))
        .await
        .check_ok();
    th.client
        .delete(&format!("/w/{ROOT_WORKSPACE_ID}/teams/{}", second.id))
        .await
        .check_no_content();
    th.client2
        .patch(&path, &json!({"title":"Cannot edit"}))
        .await
        .check_status(StatusCode::FORBIDDEN);
    let members: Vec<ContainerMember> = th.client2.get(&format!("{path}/members")).await.ok_json();
    let member = members.iter().find(|m| m.user_id == user).unwrap();
    assert_eq!(
        member.role,
        ContainerRole::Viewer,
        "direct role stays separate"
    );
    assert_eq!(member.effective_role(), ContainerRole::Commenter);
    th.client
        .delete(&format!("{path}/teams/{}", first.id))
        .await
        .check_no_content();
    th.client2.get(&path).await.check_ok();
    th.client
        .delete(&format!("{path}/members/{user}"))
        .await
        .check_no_content();
    th.client2
        .get(&path)
        .await
        .check_status(StatusCode::NOT_FOUND);
    th.teardown().await;
}

#[tokio::test]
async fn a_manager_cannot_gain_reading_through_a_team_without_a_record() {
    let th = Setup::native().start().await.init_basic().await;
    let manager = th.client.me().await.id;
    let owner = th.client2.me().await.id;
    let board = board_with_admin(
        &th,
        new_board(ROOT_WORKSPACE_ID, ContainerVisibility::Private, "Managed"),
        &owner,
    )
    .await;
    let path = format!("/w/{ROOT_WORKSPACE_ID}/containers/{}", board.id);
    let colleague = th.stranger().await;
    let others = group(&th, &colleague, "Other readers").await;
    let assignment = format!("{path}/teams/{}", others.id);
    for _ in 0..2 {
        th.client
            .put(&assignment, &json!({"role":"viewer"}))
            .await
            .check_no_content();
    }
    let records = th
        .app
        .store()
        .get_audit_log(ROOT_WORKSPACE_ID)
        .await
        .unwrap();
    let added = records
        .iter()
        .filter(|r| r.target_id == board.id && r.action == AuditAction::AddedToContainer)
        .collect::<Vec<_>>();
    assert_eq!(
        added.len(),
        1,
        "an idempotent assignment records the new access once"
    );
    assert_eq!(added[0].actor.person_id(), Some(manager.as_str()));
    assert_eq!(added[0].subject_id, colleague);
    let team = group(&th, &manager, "Managers").await;
    th.client
        .put(
            &format!("{path}/teams/{}", team.id),
            &json!({"role":"viewer"}),
        )
        .await
        .check_no_content();
    let records = th
        .app
        .store()
        .get_audit_log(ROOT_WORKSPACE_ID)
        .await
        .unwrap();
    assert_eq!(
        records
            .iter()
            .filter(|r| r.target_id == board.id && r.action == AuditAction::SelfAddedToContainer)
            .count(),
        1
    );
    th.client
        .delete(&format!(
            "/w/{ROOT_WORKSPACE_ID}/teams/{}/members/{manager}",
            team.id
        ))
        .await
        .check_no_content();
    assert!(
        th.app
            .container_grant(&manager, &board.id)
            .await
            .unwrap()
            .manages_only()
    );
    th.client
        .put(
            &format!("/w/{ROOT_WORKSPACE_ID}/teams/{}/members/{manager}", team.id),
            &json!({}),
        )
        .await
        .check_no_content();
    let records = th
        .app
        .store()
        .get_audit_log(ROOT_WORKSPACE_ID)
        .await
        .unwrap();
    assert_eq!(
        records
            .iter()
            .filter(|r| r.target_id == board.id && r.action == AuditAction::SelfAddedToContainer)
            .count(),
        2
    );
    th.teardown().await;
}

#[tokio::test]
async fn losing_the_last_team_role_ends_subscriptions_and_live_board_messages() {
    use crate::board_broadcasts::WorkspaceSocket;
    use fvoci_model::{Block, BlockType, Subscription, new_id};
    let th = Setup::native().start().await.init_basic().await;
    let owner = th.client.me().await.id;
    let user = th.client2.me().await.id;
    let board = board_with_admin(
        &th,
        new_board(ROOT_WORKSPACE_ID, ContainerVisibility::Private, "Signals"),
        &owner,
    )
    .await;
    let path = format!("/w/{ROOT_WORKSPACE_ID}/containers/{}", board.id);
    let team = group(&th, &user, "Listeners").await;
    th.client
        .put(
            &format!("{path}/teams/{}", team.id),
            &json!({"role":"editor"}),
        )
        .await
        .check_no_content();
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
    let mut socket = WorkspaceSocket::subscribe(&th, &th.client2, ROOT_WORKSPACE_ID, &user).await;
    th.client
        .patch(&path, &json!({"title":"Heard"}))
        .await
        .check_ok();
    socket
        .expect("containerChanged", "/container/title", "Heard")
        .await;
    th.client
        .delete(&format!(
            "/w/{ROOT_WORKSPACE_ID}/teams/{}/members/{user}",
            team.id
        ))
        .await
        .check_no_content();
    socket
        .expect("containerMemberRemoved", "/member/userId", &user)
        .await;
    socket.drain().await;
    th.client
        .patch(&path, &json!({"title":"Not heard"}))
        .await
        .check_ok();
    assert!(
        socket.drain().await.is_empty(),
        "a workspace subscription no longer receives this board"
    );
    let subscriptions: Vec<Subscription> = th
        .client2
        .get(&format!("/w/{ROOT_WORKSPACE_ID}/subscriptions/{user}"))
        .await
        .ok_json();
    assert!(
        subscriptions.is_empty(),
        "stored card subscriptions are gone too"
    );
    th.teardown().await;
}

#[tokio::test]
async fn workspace_demotion_or_departure_revokes_team_access_without_restoring_it() {
    for departure in [false, true] {
        let th = Setup::native().start().await.init_basic().await;
        let owner = th.client.me().await.id;
        let user = th.client2.me().await.id;
        let board = board_with_admin(
            &th,
            new_board(ROOT_WORKSPACE_ID, ContainerVisibility::Private, "Revoked"),
            &owner,
        )
        .await;
        let path = format!("/w/{ROOT_WORKSPACE_ID}/containers/{}", board.id);
        let team = group(&th, &user, "Former members").await;
        th.client
            .put(
                &format!("{path}/teams/{}", team.id),
                &json!({"role":"editor"}),
            )
            .await
            .check_no_content();
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
        th.client2
            .get(&format!("{path}/teams"))
            .await
            .check_status(StatusCode::NOT_FOUND);
        th.client2
            .post(
                "/invitations/accept",
                &json!({"token":th.member_link().await}),
            )
            .await
            .check_ok();
        th.client2
            .get(&path)
            .await
            .check_status(StatusCode::NOT_FOUND);
        th.teardown().await;
    }
}

#[tokio::test]
async fn a_private_template_is_listed_while_a_team_role_grants_it() {
    let th = Setup::native().start().await.init_basic().await;
    let manager = th.client.me().await.id;
    let owner = th.client2.me().await.id;
    let mut template = new_board(
        ROOT_WORKSPACE_ID,
        ContainerVisibility::Private,
        "Team template",
    );
    template.is_template = true;
    let board = board_with_admin(&th, template, &owner).await;
    // The ordinary workspace member administers the board and can assign a team.
    let team = group(&th, &owner, "Template authors").await;
    let assignment = format!(
        "/w/{ROOT_WORKSPACE_ID}/containers/{}/teams/{}",
        board.id, team.id
    );
    th.client2
        .put(&assignment, &json!({"role":"editor"}))
        .await
        .check_no_content();
    // Give the direct admin role to the manager, then remove the owner's direct role.
    th.client2
        .post(
            &format!("/w/{ROOT_WORKSPACE_ID}/containers/{}/members", board.id),
            &json!({"userId": manager, "role": "admin"}),
        )
        .await
        .check_ok();
    th.client
        .delete(&format!(
            "/w/{ROOT_WORKSPACE_ID}/containers/{}/members/{owner}",
            board.id
        ))
        .await
        .check_no_content();
    let path = format!("/w/{ROOT_WORKSPACE_ID}/templates");
    let listed: Vec<Container> = th.client2.get(&path).await.ok_json();
    assert!(
        listed.iter().any(|b| b.id == board.id),
        "a team-only template is discoverable"
    );
    th.client.delete(&assignment).await.check_no_content();
    let listed: Vec<Container> = th.client2.get(&path).await.ok_json();
    assert!(
        !listed.iter().any(|b| b.id == board.id),
        "revoked template is gone"
    );
    th.teardown().await;
}

#[tokio::test]
async fn a_template_turned_private_or_open_says_nothing_of_its_team_readers() {
    use crate::board_broadcasts::WorkspaceSocket;
    let th = Setup::native().start().await.init_basic().await;
    let owner = th.client.me().await.id;
    let user = th.client2.me().await.id;
    let template = board_with_admin(
        &th,
        Container {
            is_template: true,
            ..new_board(
                ROOT_WORKSPACE_ID,
                ContainerVisibility::Open,
                "Team template",
            )
        },
        &owner,
    )
    .await;
    let team = group(&th, &user, "Template editors").await;
    th.client
        .put(
            &format!(
                "/w/{ROOT_WORKSPACE_ID}/containers/{}/teams/{}",
                template.id, team.id
            ),
            &json!({"role":"editor"}),
        )
        .await
        .check_no_content();
    let outsider = th.stranger().await;
    let mut socket = WorkspaceSocket::subscribe(&th, &th.client, ROOT_WORKSPACE_ID, &owner).await;
    let about = |messages: &[serde_json::Value], action: &str, who: &str| {
        messages
            .iter()
            .filter(|message| {
                message["type"] == json!(action) && message["member"]["userId"] == json!(who)
            })
            .count()
    };
    for (kind, action) in [
        ("private", "containerMemberRemoved"),
        ("open", "containerMemberChanged"),
    ] {
        th.client
            .patch(
                &format!("/w/{ROOT_WORKSPACE_ID}/containers/{}", template.id),
                &json!({"visibility": kind}),
            )
            .await
            .check_ok();
        socket
            .expect("containerChanged", "/container/id", &template.id)
            .await;
        let messages = socket.drain().await;
        assert_eq!(
            about(&messages, action, &outsider),
            1,
            "who reads it only as a template hears of it: {messages:?}"
        );
        assert_eq!(
            about(&messages, action, &user),
            0,
            "a team editor keeps their role when the template becomes {kind}: {messages:?}"
        );
    }
    th.teardown().await;
}

/// A board a person reads through a team is listed among their memberships as their
/// own boards are: not to a password session once its organization takes only
/// sessions made through its provider.
#[tokio::test]
async fn a_board_of_a_team_is_listed_only_to_a_session_its_organization_takes() {
    use crate::common::Client;
    use crate::fake_oidc::FakeProvider;
    use crate::oidc::{org_of, set_provider, with_key_and_private_addresses};
    use crate::sso::{Passwords, admin_through_provider, require_sso};

    let fake = FakeProvider::start().await;
    let th = Setup::local_mode()
        .trusting_oidc_provider(&fake.certificate_der)
        .configure(with_key_and_private_addresses)
        .start()
        .await
        .init_basic()
        .await;
    let org_id = org_of(&th).await;
    set_provider(&th, &org_id, &fake).await;
    let owner = th.client.me().await.id;
    let user = th.client2.me().await.id;
    let board = board_with_admin(
        &th,
        new_board(ROOT_WORKSPACE_ID, ContainerVisibility::Private, "Walled"),
        &owner,
    )
    .await;
    let team = group(&th, &user, "Walled readers").await;
    th.client
        .put(
            &format!(
                "/w/{ROOT_WORKSPACE_ID}/containers/{}/teams/{}",
                board.id, team.id
            ),
            &json!({"role":"viewer"}),
        )
        .await
        .check_no_content();
    let listed = async |client: &Client| -> bool {
        let members: Vec<ContainerMember> = client.get("/users/me/memberships").await.ok_json();
        members.iter().any(|member| member.container_id == board.id)
    };
    assert!(listed(&th.client2).await, "the team's board is listed");

    let admin = admin_through_provider(&th, &fake, &org_id).await;
    require_sso(&admin, &org_id, &fake, Passwords::Refused)
        .await
        .check_ok();
    assert!(
        !listed(&th.client2).await,
        "a password session lists no board of the organization, a team's neither"
    );
    th.teardown().await;
    fake.stop();
}

/// What a reference names is told to whoever holds a role through a team as to one who
/// holds it by name, and no longer once the team is taken off the board.
#[tokio::test]
async fn a_team_role_is_told_what_references_name_until_it_is_taken_off() {
    let th = Setup::native().start().await.init_basic().await;
    let at = format!("/w/{ROOT_WORKSPACE_ID}");
    let own = made(&th.client2, &at, "private", "own").await;
    let private = made(&th.client, &at, "private", HIDDEN).await;
    let member = th.client2.me().await.id;
    let team = group(&th, &member, "Readers").await;
    let on_the_board = format!("{at}/containers/{}/teams/{}", private.board, team.id);

    check_as_nothing(&th.client2, &at, &private.both(), &own.card, "no role").await;
    th.client
        .put(&on_the_board, &json!({"role":"viewer"}))
        .await
        .check_no_content();
    assert_eq!(
        named(&resolved(&th.client2, &at, &private.both()).await),
        [private.card.as_str()],
        "the team is on the board and not on the wiki"
    );
    th.client.delete(&on_the_board).await.check_no_content();
    check_as_nothing(&th.client2, &at, &private.both(), &own.card, "taken off").await;
    th.teardown().await;
}

#[tokio::test]
async fn team_members_record_course_progress_and_appear_in_summary() {
    let th = Setup::native().start().await.init_basic().await;
    let at = format!("/w/{ROOT_WORKSPACE_ID}");
    let owner = th.client.me().await.id;
    let member = th.client2.me().await.id;
    let course = th
        .client
        .post(
            &format!("{at}/courses"),
            &json!({"visibility": "private", "title": "Team Course"}),
        )
        .await;
    course.check_created();
    let course_id = course.value()["id"].as_str().expect("course id").to_owned();
    let unit = th
        .client
        .post(
            &format!("{at}/courses/{course_id}/units"),
            &json!({"title": "U1"}),
        )
        .await;
    unit.check_created();
    let unit_id = unit.value()["id"].as_str().expect("unit id").to_owned();

    let team = group(&th, &member, "Learners").await;
    let assigned = format!("{at}/containers/{course_id}/teams/{}", team.id);
    th.client
        .put(&assigned, &json!({"role": "viewer"}))
        .await
        .check_no_content();

    th.client2
        .put(
            &format!("{at}/courses/{course_id}/units/{unit_id}/progress"),
            &json!({"completed": true}),
        )
        .await
        .check_ok();
    let summary: fvoci_model::CourseProgressSummary = th
        .client
        .get(&format!("{at}/courses/{course_id}/progress/summary"))
        .await
        .ok_json();
    assert_eq!(summary.total_units, 1);
    assert!(
        summary
            .members
            .iter()
            .any(|m| m.user_id == member && m.completed_units == 1),
        "team learner counted in summary: {summary:?}"
    );
    assert!(
        summary
            .members
            .iter()
            .any(|m| m.user_id == owner && m.completed_units == 0),
        "owner counted in summary: {summary:?}"
    );
    assert_eq!(summary.members.len(), 2, "owner + team learner");

    // Holding both a direct role and an inherited team role must not duplicate the member.
    th.client
        .post(
            &format!("{at}/containers/{course_id}/members"),
            &json!({"userId": member, "role": "viewer"}),
        )
        .await
        .check_ok();
    let both: fvoci_model::CourseProgressSummary = th
        .client
        .get(&format!("{at}/courses/{course_id}/progress/summary"))
        .await
        .ok_json();
    assert_eq!(both.members.len(), 2, "direct + team role deduplicated");
    assert!(
        both.members
            .iter()
            .any(|m| m.user_id == member && m.completed_units == 1),
        "deduplicated learner keeps completed count: {both:?}"
    );
    th.client
        .delete(&format!("{at}/containers/{course_id}/members/{member}"))
        .await
        .check_no_content();

    th.client.delete(&assigned).await.check_no_content();
    th.client2
        .get(&format!("{at}/courses/{course_id}/progress"))
        .await
        .check_not_found();
    let after: fvoci_model::CourseProgressSummary = th
        .client
        .get(&format!("{at}/courses/{course_id}/progress/summary"))
        .await
        .ok_json();
    assert_eq!(after.members.len(), 1);
    assert_eq!(after.members[0].user_id, owner);
    th.teardown().await;
}
