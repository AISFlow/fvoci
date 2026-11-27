//! The deleted boards of a workspace, as their former admins find them.

use crate::common::TEST_WS;
use crate::common::{Setup, TEST_WORKSPACE_ID};
use fvoci_model::{Container, ContainerVisibility};
use serde_json::{Value, json};
use std::time::Duration;

/// The deleted boards of a workspace are listed for whoever was an admin of each when it
/// was deleted, who may bring it back; a member who was no admin does not see it. The
/// last deleted first.
#[tokio::test]
async fn a_deleted_board_is_listed_for_its_former_admins() {
    let th = Setup::native().start().await.init_basic().await;
    let user2 = th.client2.me().await;
    let titled = |title: &str| Container {
        workspace_id: TEST_WORKSPACE_ID.to_owned(),
        visibility: ContainerVisibility::Private,
        title: title.to_owned(),
        ..Container::default()
    };
    let mine: Container = th.client.create_board(&titled("mine")).await.created_json();
    let theirs: Container = th
        .client2
        .create_board(&titled("theirs"))
        .await
        .created_json();
    let also_mine: Container = th
        .client
        .create_board(&titled("also mine"))
        .await
        .created_json();
    th.app
        .add_member_to_container(&fvoci_model::ContainerMember {
            container_id: theirs.id.clone(),
            user_id: th.client.me().await.id,
            role: fvoci_model::ContainerRole::Editor,
            synthetic: false,
            inherited_role: None,
        })
        .await
        .expect("user1 edits their board");
    for (client, board) in [
        (&th.client, &mine),
        (&th.client2, &theirs),
        (&th.client, &also_mine),
    ] {
        client
            .delete(&format!("{TEST_WS}/containers/{}", board.id))
            .await
            .check_no_content();
        // A millisecond apart at least, so that their order is that of time.
        tokio::time::sleep(Duration::from_millis(3)).await;
    }
    let trash = format!("{TEST_WS}/trash/containers");
    let names_of = |listed: &[Value]| -> Vec<Value> {
        listed.iter().map(|entry| entry["title"].clone()).collect()
    };

    let listed: Vec<Value> = th.client.get(&trash).await.ok_json();
    assert_eq!(
        names_of(&listed),
        [json!("also mine"), json!("mine")],
        "where user1 was an admin, the last deleted first"
    );
    assert_eq!(listed[1]["id"], json!(mine.id));
    assert_eq!(listed[1]["deletedBy"], json!(th.client.me().await.id));
    let members: Vec<&String> = listed[1].as_object().expect("an entry").keys().collect();
    assert_eq!(members, ["deletedAt", "deletedBy", "id", "kind", "title"]);
    assert!(
        listed[1]["deletedAt"].as_i64().unwrap_or(0) > 0
            && listed[0]["deletedAt"].as_i64() > listed[1]["deletedAt"].as_i64(),
        "{listed:?}"
    );
    let listed: Vec<Value> = th.client2.get(&trash).await.ok_json();
    assert_eq!(names_of(&listed), [json!("theirs")]);
    assert_eq!(listed[0]["deletedBy"], json!(user2.id));

    let page: Vec<Value> = th
        .client
        .get(&format!("{trash}?page=1&perPage=1"))
        .await
        .ok_json();
    assert_eq!(names_of(&page), [json!("mine")], "the pages cut that order");
    let every: Vec<Value> = th
        .client
        .get(&format!("{trash}?perPage=1000"))
        .await
        .ok_json();
    assert_eq!(names_of(&every), [json!("also mine"), json!("mine")]);
    th.client
        .get(&format!("{trash}?page=x"))
        .await
        .check_bad_request();
    th.anonymous().get(&trash).await.check_unauthorized();

    // Brought back, a board is no longer listed.
    th.client
        .post_empty(&format!("{TEST_WS}/containers/{}/restore", mine.id))
        .await
        .check_ok();
    let listed: Vec<Value> = th.client.get(&trash).await.ok_json();
    assert_eq!(names_of(&listed), [json!("also mine")]);
}
