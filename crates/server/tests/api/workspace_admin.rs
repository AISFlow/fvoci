//! An administrator of a workspace lists and manages its private boards (sees them,
//! changes their members, deletes and restores them) and reads what is on one only as
//! its member: making themselves one is recorded.

use fvoci_model::AuditAction;
use fvoci_model::{Container, ContainerVisibility, WorkspaceRole};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use crate::boards::{member, new_board};
use crate::common::TEST_WS;
use crate::common::{ApiResponse, Client, Setup, TEST_WORKSPACE_ID, TestServer, UNKNOWN_ID};

/// The problem of an answer without what identifies the request.
fn answer(response: &ApiResponse) -> (StatusCode, Value) {
    let mut body = response.value();
    if let Some(object) = body.as_object_mut() {
        object.remove("instance");
        object.remove("requestId");
    }
    (response.status, body)
}

/// Makes the second user an administrator of the test workspace; returns their identifier.
pub(crate) async fn workspace_admin(th: &TestServer) -> String {
    let user_id = th.client2.me().await.id;
    th.app
        .store()
        .execute_unchecked(&format!(
            "UPDATE workspace_members SET role = '{}' WHERE user_id = '{user_id}' \
             AND workspace_id = '{TEST_WORKSPACE_ID}'",
            WorkspaceRole::Admin.as_str()
        ))
        .await
        .expect("the second user administers the test workspace");
    user_id
}

/// A private board of the first user with one card; returns the board and the card.
async fn private_board_with_a_card(th: &TestServer) -> (Container, String) {
    let board: Container = th
        .client
        .create_board(&Container {
            description: "The plan".to_owned(),
            ..new_board(TEST_WORKSPACE_ID, ContainerVisibility::Private, "Private")
        })
        .await
        .created_json();
    let card = th
        .client
        .post(
            &format!("{TEST_WS}/boards/{}/cards", board.id),
            &json!({"title": "Secret"}),
        )
        .await;
    card.check_created();
    let card_id = card.value()["id"].as_str().expect("a card").to_owned();
    (board, card_id)
}

/// The content routes of a board, each with the same route of a board that is not there.
fn content(board_id: &str, card_id: &str) -> Vec<(Method, String, String)> {
    let get = |hidden: String, absent: String| (Method::GET, hidden, absent);
    vec![
        get(
            format!("{TEST_WS}/boards/{board_id}/blocks"),
            format!("{TEST_WS}/boards/{UNKNOWN_ID}/blocks"),
        ),
        get(
            format!("{TEST_WS}/boards/{board_id}/cards"),
            format!("{TEST_WS}/boards/{UNKNOWN_ID}/cards"),
        ),
        get(
            format!("{TEST_WS}/cards/{card_id}"),
            format!("{TEST_WS}/cards/{UNKNOWN_ID}"),
        ),
        get(
            format!("{TEST_WS}/boards/{board_id}/cards/by-number/1"),
            format!("{TEST_WS}/boards/{UNKNOWN_ID}/cards/by-number/1"),
        ),
        get(
            format!("{TEST_WS}/containers/{board_id}/public-link"),
            format!("{TEST_WS}/containers/{UNKNOWN_ID}/public-link"),
        ),
        get(
            format!("{TEST_WS}/containers/{board_id}/archive/export"),
            format!("{TEST_WS}/containers/{UNKNOWN_ID}/archive/export"),
        ),
        (
            Method::POST,
            format!("{TEST_WS}/containers/{board_id}/duplicate"),
            format!("{TEST_WS}/containers/{UNKNOWN_ID}/duplicate"),
        ),
        (
            Method::POST,
            format!("{TEST_WS}/boards/{board_id}/cards"),
            format!("{TEST_WS}/boards/{UNKNOWN_ID}/cards"),
        ),
    ]
}

async fn send(client: &Client, method: &Method, path: &str) -> ApiResponse {
    Client::send(client.request(method.clone(), path).json(&json!({}))).await
}

#[tokio::test]
async fn a_workspace_admin_lists_and_manages_a_private_board_and_reads_none_of_it() {
    let th = Setup::native().start().await.init_basic().await;
    let admin_id = workspace_admin(&th).await;
    let (board, card_id) = private_board_with_a_card(&th).await;
    let third = th.add_user("third").await.me().await.id;

    let listed: Vec<Container> = th.client2.get(&format!("{TEST_WS}/boards")).await.ok_json();
    let seen = th
        .client2
        .get(&format!("{TEST_WS}/containers/{}", board.id))
        .await;
    let members = th
        .client2
        .get(&format!("{TEST_WS}/containers/{}/members", board.id))
        .await;
    let mut hidden = Vec::new();
    for (method, path, absent) in content(&board.id, &card_id) {
        let response = send(&th.client2, &method, &path).await;
        let missing = send(&th.client2, &method, &absent).await;
        if response.status != StatusCode::NOT_FOUND || answer(&response) != answer(&missing) {
            hidden.push(format!("{method} {path}: {:?}", answer(&response)));
        }
    }
    let added = th
        .client2
        .add_member(&member(
            &board.id,
            &third,
            fvoci_model::ContainerRole::Viewer,
        ))
        .await;
    let deleted = th
        .client2
        .delete(&format!("{TEST_WS}/containers/{}", board.id))
        .await;
    let in_the_trash: Vec<Value> = th
        .client2
        .get(&format!("{TEST_WS}/trash/containers"))
        .await
        .ok_json();
    let restored = th
        .client2
        .post(
            &format!("{TEST_WS}/containers/{}/restore", board.id),
            &json!({}),
        )
        .await;
    let recorded = th
        .app
        .store()
        .get_audit_log(TEST_WORKSPACE_ID)
        .await
        .expect("the log is read");
    th.teardown().await;

    let listed = listed
        .iter()
        .find(|listed| listed.id == board.id)
        .expect("the board is listed");
    assert_eq!(
        (listed.title.as_str(), listed.description.as_str()),
        ("Private", "")
    );
    seen.check_ok();
    assert_eq!(
        seen.value()["description"],
        json!(""),
        "its description is content"
    );
    members.check_ok();
    assert!(hidden.is_empty(), "content reached:\n{}", hidden.join("\n"));
    added.check_ok();
    deleted.check_no_content();
    assert!(
        in_the_trash
            .iter()
            .any(|item| item["id"] == json!(board.id)),
        "{in_the_trash:?}"
    );
    restored.check_ok();
    assert_eq!(restored.value()["description"], json!(""));
    // Who manages a board without reading it and gives somebody else a role on it is
    // recorded: the other might be an account of their own. Nothing else is.
    let recorded: Vec<_> = recorded
        .iter()
        .map(|entry| {
            (
                entry.action,
                entry.actor.person_id().unwrap_or_default(),
                entry.target_id.as_str(),
                entry.subject_id.as_str(),
            )
        })
        .collect();
    assert_eq!(
        recorded,
        [(
            AuditAction::AddedToContainer,
            admin_id.as_str(),
            board.id.as_str(),
            third.as_str()
        )],
        "only the member the admin added is recorded"
    );
}

#[tokio::test]
async fn a_workspace_admin_who_makes_themselves_a_member_reads_the_board_and_is_recorded_once() {
    let th = Setup::native().start().await.init_basic().await;
    let admin = workspace_admin(&th).await;
    let (board, card_id) = private_board_with_a_card(&th).await;
    let as_viewer = member(&board.id, &admin, fvoci_model::ContainerRole::Viewer);

    th.client2.add_member(&as_viewer).await.check_ok();
    let card = th.client2.get(&format!("{TEST_WS}/cards/{card_id}")).await;
    // Adding themselves again changes nothing, and records nothing.
    th.client2.add_member(&as_viewer).await.check_ok();
    let recorded = th
        .app
        .store()
        .get_audit_log(TEST_WORKSPACE_ID)
        .await
        .expect("the log is read");
    th.teardown().await;

    card.check_ok();
    assert_eq!(recorded.len(), 1, "{recorded:?}");
    let entry = &recorded[0];
    assert_eq!(
        (
            entry.action,
            entry.actor.person_id().unwrap_or_default(),
            entry.target_id.as_str(),
            entry.workspace_id.as_str()
        ),
        (
            AuditAction::SelfAddedToContainer,
            admin.as_str(),
            board.id.as_str(),
            TEST_WORKSPACE_ID
        )
    );
    assert!(entry.created_at > 0, "{entry:?}");
}

#[tokio::test]
async fn a_member_who_is_no_admin_neither_lists_nor_manages_a_private_board() {
    let th = Setup::native().start().await.init_basic().await;
    let (board, _) = private_board_with_a_card(&th).await;
    let missing = th
        .client2
        .get(&format!("{TEST_WS}/containers/{UNKNOWN_ID}/members"))
        .await;

    let listed: Vec<Container> = th.client2.get(&format!("{TEST_WS}/boards")).await.ok_json();
    let members = th
        .client2
        .get(&format!("{TEST_WS}/containers/{}/members", board.id))
        .await;
    let deleted = th
        .client2
        .delete(&format!("{TEST_WS}/containers/{}", board.id))
        .await;
    th.teardown().await;

    assert!(
        listed.iter().all(|listed| listed.id != board.id),
        "{listed:?}"
    );
    assert_eq!(answer(&members), answer(&missing));
    deleted.check_not_found();
}

/// The names of the entries of the workspace's archive as the client exports it.
async fn exported(client: &Client) -> Vec<String> {
    let archive = client.get(&format!("{TEST_WS}/archive/export")).await;
    archive.check_ok();
    let archive =
        zip::ZipArchive::new(std::io::Cursor::new(archive.body)).expect("the export is an archive");
    archive
        .file_names()
        .filter_map(Result::ok)
        .map(|name| name.to_string())
        .collect()
}

#[tokio::test]
async fn a_workspace_admin_who_asks_without_a_role_reads_nothing_until_giving_one_is_recorded() {
    let th = Setup::native().start().await.init_basic().await;
    let admin = workspace_admin(&th).await;
    let (board, card_id) = private_board_with_a_card(&th).await;
    // A request without a role is refused, opens nothing, and exports nothing.
    let refused = th
        .client2
        .post(
            &format!("{TEST_WS}/containers/{}/members", board.id),
            &json!({"userId": admin}),
        )
        .await;
    refused.check_bad_request();
    let before = th.client2.get(&format!("{TEST_WS}/cards/{card_id}")).await;
    let archived = exported(&th.client2).await;

    let raised = th
        .client2
        .post(
            &format!("{TEST_WS}/containers/{}/members", board.id),
            &json!({"userId": admin, "role": "viewer"}),
        )
        .await;
    let after = th.client2.get(&format!("{TEST_WS}/cards/{card_id}")).await;
    let recorded = th
        .app
        .store()
        .get_audit_log(TEST_WORKSPACE_ID)
        .await
        .expect("the log is read");
    th.teardown().await;

    before.check_not_found();
    assert!(
        archived.iter().all(|name| !name.contains(&board.id)),
        "{archived:?}"
    );
    raised.check_ok();
    after.check_ok();
    let recorded: Vec<_> = recorded
        .iter()
        .map(|entry| {
            (
                entry.action,
                entry.actor.person_id().unwrap_or_default(),
                entry.target_id.as_str(),
            )
        })
        .collect();
    assert_eq!(
        recorded,
        [(
            AuditAction::SelfAddedToContainer,
            admin.as_str(),
            board.id.as_str()
        )]
    );
}

#[tokio::test]
async fn a_workspace_admin_who_joins_an_open_board_is_not_recorded() {
    let th = Setup::native().start().await.init_basic().await;
    let admin = workspace_admin(&th).await;
    let board: Container = th
        .client
        .create_board(&new_board(
            TEST_WORKSPACE_ID,
            ContainerVisibility::Open,
            "Open",
        ))
        .await
        .created_json();

    let added = th
        .client2
        .add_member(&member(
            &board.id,
            &admin,
            fvoci_model::ContainerRole::Viewer,
        ))
        .await;
    let recorded = th
        .app
        .store()
        .get_audit_log(TEST_WORKSPACE_ID)
        .await
        .expect("the log is read");
    th.teardown().await;

    added.check_ok();
    assert!(
        recorded.is_empty(),
        "an open board is read by joining it: {recorded:?}"
    );
}

#[tokio::test]
async fn a_workspace_admin_lists_a_template_they_only_manage_by_name_and_adding_no_role_is_not_recorded()
 {
    let th = Setup::native().start().await.init_basic().await;
    let admin = workspace_admin(&th).await;
    let template: Container = th
        .client
        .create_board(&Container {
            is_template: true,
            description: "How we plan".to_owned(),
            ..new_board(TEST_WORKSPACE_ID, ContainerVisibility::Private, "Template")
        })
        .await
        .created_json();

    // A request without a role is refused, opens nothing, and is not recorded.
    th.client2
        .post(
            &format!("{TEST_WS}/containers/{}/members", template.id),
            &json!({"userId": admin}),
        )
        .await
        .check_bad_request();
    let templates: Vec<Container> = th
        .client2
        .get(&format!("{TEST_WS}/templates"))
        .await
        .ok_json();
    let recorded = th
        .app
        .store()
        .get_audit_log(TEST_WORKSPACE_ID)
        .await
        .expect("the log is read");
    th.teardown().await;

    // Listed to be managed, with its name only (as_managed).
    let listed = templates
        .iter()
        .find(|listed| listed.id == template.id)
        .expect("the administrator lists the template");
    assert_eq!(
        (listed.title.as_str(), listed.description.as_str()),
        ("Template", "")
    );
    assert!(recorded.is_empty(), "{recorded:?}");
}
