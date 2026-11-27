//! Teams are scoped to one workspace and contain only its nonguest members.

use crate::common::{ROOT_WORKSPACE_ID, Setup, UNKNOWN_ID};
use fvoci_model::Team;
use reqwest::StatusCode;
use serde_json::json;

fn path() -> String {
    format!("/w/{ROOT_WORKSPACE_ID}/teams")
}

#[tokio::test]
async fn administrators_manage_teams_and_members_read_them() {
    let th = Setup::native().start().await.init_basic().await;
    let second = th.client2.me().await;
    let team: Team = th
        .client
        .post(&path(), &json!({"name":"  Design  "}))
        .await
        .ok_json();
    assert_eq!(team.name, "Design");
    let item = format!("{}/{}", path(), team.id);
    for invalid in ["\0", "Design\0ops"] {
        th.client
            .post(&path(), &json!({"name": invalid}))
            .await
            .check_status(StatusCode::BAD_REQUEST);
        th.client
            .put(&item, &json!({"name": invalid}))
            .await
            .check_status(StatusCode::BAD_REQUEST);
    }

    let membership = format!("{item}/members/{}", second.id);
    th.client
        .post(&path(), &json!({"name":"Design"}))
        .await
        .check_status(StatusCode::CONFLICT);
    th.client
        .post(&path(), &json!({"name":"  "}))
        .await
        .check_status(StatusCode::BAD_REQUEST);
    th.client2
        .post(&path(), &json!({"name":"Other"}))
        .await
        .check_status(StatusCode::FORBIDDEN);
    th.client
        .put(&membership, &json!({}))
        .await
        .check_status(StatusCode::NO_CONTENT);
    th.client
        .put(&membership, &json!({}))
        .await
        .check_status(StatusCode::NO_CONTENT);
    let read: Team = th.client2.get(&item).await.ok_json();
    assert_eq!(read.members.len(), 1);
    assert_eq!(read.members[0].user_id, second.id);
    th.client2
        .delete(&membership)
        .await
        .check_status(StatusCode::FORBIDDEN);
    th.client2
        .put(&item, &json!({"name":"Other"}))
        .await
        .check_status(StatusCode::FORBIDDEN);
    th.client2
        .delete(&item)
        .await
        .check_status(StatusCode::FORBIDDEN);
    let renamed: Team = th
        .client
        .put(&item, &json!({"name":"Research"}))
        .await
        .ok_json();
    assert_eq!(renamed.name, "Research");
    th.client
        .delete(&membership)
        .await
        .check_status(StatusCode::NO_CONTENT);
    let read: Team = th.client.get(&item).await.ok_json();
    assert_eq!(read.members, Vec::new());
    th.client
        .delete(&item)
        .await
        .check_status(StatusCode::NO_CONTENT);
    th.client
        .get(&item)
        .await
        .check_status(StatusCode::NOT_FOUND);
    th.teardown().await;
}

#[tokio::test]
async fn guests_cannot_see_or_join_teams_and_promoting_restores_no_membership() {
    let th = Setup::native().start().await.init_basic().await;
    let second = th.client2.me().await;
    let team: Team = th
        .client
        .post(&path(), &json!({"name":"Design"}))
        .await
        .ok_json();
    let item = format!("{}/{}", path(), team.id);
    let membership = format!("{item}/members/{}", second.id);
    th.client
        .put(&membership, &json!({}))
        .await
        .check_status(StatusCode::NO_CONTENT);
    th.client
        .put(
            &format!("/w/{ROOT_WORKSPACE_ID}/members/{}", second.id),
            &json!({"role":"guest"}),
        )
        .await
        .check_ok();
    let read: Team = th.client.get(&item).await.ok_json();
    assert!(
        read.members.is_empty(),
        "becoming a guest removes the stored membership"
    );
    th.client2
        .get(&path())
        .await
        .check_status(StatusCode::NOT_FOUND);
    th.client2
        .get(&item)
        .await
        .check_status(StatusCode::NOT_FOUND);
    let guest_add = th.client.put(&membership, &json!({})).await;
    guest_add.check_status(StatusCode::CONFLICT);
    assert_eq!(guest_add.error_reason(), "guest_cannot_join_team");
    th.client
        .put(&format!("{item}/members/{UNKNOWN_ID}"), &json!({}))
        .await
        .check_status(StatusCode::NOT_FOUND);
    th.client2
        .post(
            "/invitations/accept",
            &json!({"token": th.member_link().await}),
        )
        .await
        .check_ok();
    let promoted: Team = th.client2.get(&item).await.ok_json();
    assert!(
        promoted.members.is_empty(),
        "becoming a member again restores no former team membership"
    );
    th.client
        .delete(&item)
        .await
        .check_status(StatusCode::NO_CONTENT);
    th.client
        .get(&item)
        .await
        .check_status(StatusCode::NOT_FOUND);
    th.teardown().await;
}

#[tokio::test]
async fn leaving_a_workspace_removes_team_memberships() {
    let th = Setup::native().start().await.init_basic().await;
    let second = th.client2.me().await;
    let team: Team = th
        .client
        .post(&path(), &json!({"name":"Writers"}))
        .await
        .ok_json();
    let item = format!("{}/{}", path(), team.id);
    th.client
        .put(&format!("{item}/members/{}", second.id), &json!({}))
        .await
        .check_status(StatusCode::NO_CONTENT);
    th.client
        .delete(&format!("/w/{ROOT_WORKSPACE_ID}/members/{}", second.id))
        .await
        .check_status(StatusCode::NO_CONTENT);
    let read: Team = th.client.get(&item).await.ok_json();
    assert_eq!(read.members, Vec::new());
    th.client2
        .get(&item)
        .await
        .check_status(StatusCode::NOT_FOUND);
    th.client2
        .post(
            "/invitations/accept",
            &json!({"token": th.member_link().await}),
        )
        .await
        .check_ok();
    let returned: Team = th.client2.get(&item).await.ok_json();
    assert!(
        returned.members.is_empty(),
        "rejoining restores no former team membership"
    );
    th.teardown().await;
}
