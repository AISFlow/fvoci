//! Review of the workspace teams: what a team's name may be, and that the other two ways
//! a team role ends (deleting the team, taking the team off the board) end what the
//! role opened, as taking a person out of the team does.

use crate::boards::{board_with_admin, new_board};
use crate::common::{ROOT_WORKSPACE_ID, Setup, TestServer};
use fvoci_model::{Block, BlockType, ContainerVisibility, Subscription, Team, new_id};
use reqwest::StatusCode;
use serde_json::json;

fn teams_path() -> String {
    format!("/w/{ROOT_WORKSPACE_ID}/teams")
}

/// A team's name is bounded as the names of workspaces and organizations are: a name
/// of ten thousand characters is refused, on making the team and on renaming it.
#[tokio::test]
async fn a_team_name_has_a_length_limit() {
    let th = Setup::native().start().await.init_basic().await;
    let long = "x".repeat(10_000);
    th.client
        .post(&teams_path(), &json!({ "name": long }))
        .await
        .check_status(StatusCode::BAD_REQUEST);
    let team: Team = th
        .client
        .post(&teams_path(), &json!({"name":"Short"}))
        .await
        .ok_json();
    th.client
        .put(
            &format!("{}/{}", teams_path(), team.id),
            &json!({ "name": long }),
        )
        .await
        .check_status(StatusCode::BAD_REQUEST);
    th.teardown().await;
}

/// A team's name is read by the rule of an organization's name: each refusal is about
/// `/name`, with the reason the web app chooses its words by, and the name typed is
/// kept as it was (the request changes nothing).
#[tokio::test]
async fn a_team_name_is_read_as_an_organization_name_is() {
    let th = Setup::native().start().await.init_basic().await;
    let longest = "가".repeat(64);
    let made: Team = th
        .client
        .post(&teams_path(), &json!({ "name": format!("  {longest}  ") }))
        .await
        .ok_json();
    assert_eq!(made.name, longest, "the white space around is dropped");
    let item = format!("{}/{}", teams_path(), made.id);
    for (name, reason) in [
        ("가".repeat(65), "too_long"),
        ("   ".to_owned(), "required"),
        ("a\u{202E}b".to_owned(), "invalid_characters"),
        ("a\u{200B}".to_owned(), "invalid_characters"),
        ("two\nlines".to_owned(), "invalid_characters"),
        ("nul\0".to_owned(), "invalid_characters"),
    ] {
        for refused in [
            th.client
                .post(&teams_path(), &json!({ "name": name }))
                .await,
            th.client.put(&item, &json!({ "name": name })).await,
        ] {
            refused.check_status(StatusCode::BAD_REQUEST);
            assert_eq!(refused.refused_at(), "/name", "{name:?}");
            assert_eq!(refused.value()["errors"][0]["reason"], reason, "{name:?}");
        }
    }
    let teams: Vec<Team> = th.client.get(&teams_path()).await.ok_json();
    let names: Vec<&str> = teams.iter().map(|team| team.name.as_str()).collect();
    assert_eq!(names, [longest.as_str()], "nothing refused is stored");
    th.teardown().await;
}

/// Names that differ only in letter case are one name in a workspace, on making a
/// team and on renaming another; a team may change the case of its own name.
#[tokio::test]
async fn a_team_name_is_taken_in_any_letter_case() {
    let th = Setup::native().start().await.init_basic().await;
    let design: Team = th
        .client
        .post(&teams_path(), &json!({"name":"Design"}))
        .await
        .ok_json();
    let taken = th
        .client
        .post(&teams_path(), &json!({"name":"DESIGN"}))
        .await;
    taken.check_status(StatusCode::CONFLICT);
    assert_eq!(taken.error_reason(), "team_name_taken");
    assert_eq!(taken.problem_type(), "/problems/team-name-taken");
    assert_eq!(taken.refused_at(), "/name");
    let other: Team = th
        .client
        .post(&teams_path(), &json!({"name":"Research"}))
        .await
        .ok_json();
    let taken = th
        .client
        .put(
            &format!("{}/{}", teams_path(), other.id),
            &json!({"name":"design"}),
        )
        .await;
    taken.check_status(StatusCode::CONFLICT);
    assert_eq!(taken.error_reason(), "team_name_taken");
    assert_eq!(taken.problem_type(), "/problems/team-name-taken");
    assert_eq!(taken.refused_at(), "/name");
    let renamed: Team = th
        .client
        .put(
            &format!("{}/{}", teams_path(), design.id),
            &json!({"name":"design"}),
        )
        .await
        .ok_json();
    assert_eq!(renamed.name, "design");
    // Folded beyond ASCII: the sharp s and its capitals, the final sigma, the long s.
    for (first, second) in [("Straße", "STRASSE"), ("ΟΔΟΣ", "οδοσ"), ("ſun", "SUN")] {
        th.client
            .post(&teams_path(), &json!({ "name": first }))
            .await
            .ok_json::<Team>();
        th.client
            .post(&teams_path(), &json!({ "name": second }))
            .await
            .check_status(StatusCode::CONFLICT);
    }
    th.teardown().await;
}

/// The second user is in a team with the editor role on a private board of the first
/// user, and follows a card of it.
async fn following_through_a_team(th: &TestServer, name: &str) -> (String, Team) {
    let owner = th.client.me().await.id;
    let user = th.client2.me().await.id;
    let board = board_with_admin(
        th,
        new_board(ROOT_WORKSPACE_ID, ContainerVisibility::Private, name),
        &owner,
    )
    .await;
    let team: Team = th
        .client
        .post(&teams_path(), &json!({ "name": name }))
        .await
        .ok_json();
    th.client
        .put(
            &format!("{}/{}/members/{user}", teams_path(), team.id),
            &json!({}),
        )
        .await
        .check_no_content();
    th.client
        .put(
            &format!(
                "/w/{ROOT_WORKSPACE_ID}/containers/{}/teams/{}",
                board.id, team.id
            ),
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
        .expect("the card is stored");
    th.client2
        .post(
            &format!("/w/{ROOT_WORKSPACE_ID}/subscriptions"),
            &json!({"blockType":"card", "blockId":block.id, "userId":user}),
        )
        .await
        .check_created();
    (board.id, team)
}

async fn assert_board_is_gone_for_the_second_user(th: &TestServer, board_id: &str, how: &str) {
    let user = th.client2.me().await.id;
    th.client2
        .get(&format!("/w/{ROOT_WORKSPACE_ID}/containers/{board_id}"))
        .await
        .check_status(StatusCode::NOT_FOUND);
    let subscriptions: Vec<Subscription> = th
        .client2
        .get(&format!("/w/{ROOT_WORKSPACE_ID}/subscriptions/{user}"))
        .await
        .ok_json();
    assert!(
        subscriptions.is_empty(),
        "{how}: the card subscriptions end with the role, {subscriptions:?}"
    );
}

#[tokio::test]
async fn deleting_the_team_ends_what_its_board_role_opened() {
    let th = Setup::native().start().await.init_basic().await;
    let (board_id, team) = following_through_a_team(&th, "Deleted later").await;
    th.client
        .delete(&format!("{}/{}", teams_path(), team.id))
        .await
        .check_no_content();
    assert_board_is_gone_for_the_second_user(&th, &board_id, "the team deleted").await;
    th.teardown().await;
}

#[tokio::test]
async fn taking_the_team_off_the_board_ends_what_its_role_opened() {
    let th = Setup::native().start().await.init_basic().await;
    let (board_id, team) = following_through_a_team(&th, "Taken off").await;
    th.client
        .delete(&format!(
            "/w/{ROOT_WORKSPACE_ID}/containers/{board_id}/teams/{}",
            team.id
        ))
        .await
        .check_no_content();
    assert_board_is_gone_for_the_second_user(&th, &board_id, "the team taken off").await;
    th.teardown().await;
}
