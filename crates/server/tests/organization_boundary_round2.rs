//! A second independent pass over the legacy routes and multi-organization flows.

#[path = "api/common.rs"]
mod common;

#[path = "api/fake_oidc.rs"]
mod fake_oidc;

#[path = "api/fake_search.rs"]
mod fake_search;

#[path = "api/fake_smtp.rs"]
mod fake_smtp;

#[path = "organization_boundary_round2/leaving.rs"]
mod leaving;

#[path = "organization_boundary_round2/own_space.rs"]
mod own_space;

#[path = "organization_boundary_round2/routes.rs"]
mod routes;

#[path = "organization_boundary_round2/workspace_app.rs"]
mod workspace_app;

#[path = "organization_boundary_round2/workspace_operations.rs"]
mod workspace_operations;

#[path = "organization_boundary_round2/member_operations.rs"]
mod member_operations;

#[path = "organization_boundary_round2/media.rs"]
mod media;

#[path = "organization_boundary_round2/transfers.rs"]
mod transfers;

#[path = "organization_boundary_round2/webhooks.rs"]
mod webhooks;

#[path = "organization_boundary_round2/wiki_transfers.rs"]
mod wiki_transfers;

use fvoci_model::{Container, OrgRole, Workspace, WorkspaceRole, new_id};
use fvoci_store::DbType;
use fvoci_store::testing::db_type_of_the_tests;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use crate::common::ROOT_WS;
use common::{ApiResponse, Client, ROOT_WORKSPACE_ID, Setup, TestServer, UNKNOWN_ID};

fn open(config: &mut fvoci_server::config::Configuration) {
    "open".clone_into(&mut config.org_creation);
}

/// The complete problem response, except the two fields identifying this request.
fn answer(response: &ApiResponse) -> (StatusCode, Value) {
    let mut body: Value = serde_json::from_slice(&response.body).unwrap_or(Value::Null);
    if let Some(object) = body.as_object_mut() {
        object.remove("instance");
        object.remove("requestId");
    }
    (response.status, body)
}

async fn uploaded(client: &Client, board_id: &str, bytes: &[u8]) -> ApiResponse {
    let file = reqwest::multipart::Part::bytes(bytes.to_vec()).file_name("private.txt");
    Client::send(
        client
            .request(
                Method::POST,
                &format!("{ROOT_WS}/containers/{board_id}/files"),
            )
            .multipart(reqwest::multipart::Form::new().part("file", file)),
    )
    .await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AfterRemoval {
    NoOrganization,
    OtherOrganization,
}

struct FormerMember {
    ts: TestServer,
    board: Container,
    card_id: String,
    file_id: String,
    upload_id: String,
    user_id: String,
}

/// Establish real resources through HTTP, then remove only workspace admission.
/// The board roles and the real login session deliberately remain in place.
async fn former_member(destination: AfterRemoval) -> FormerMember {
    let setup = if destination == AfterRemoval::OtherOrganization {
        Setup::native().configure(open)
    } else {
        Setup::native()
    };
    let ts = setup.start().await.init_basic().await;
    let user_id = ts.client2.me().await.id;
    let board: Container = ts
        .client2
        .post(
            &format!("{ROOT_WS}/boards"),
            &json!({
                "workspaceId": ROOT_WORKSPACE_ID, "visibility": "private", "title": "Workspace secret"
            }),
        )
        .await
        .created_json();
    let card = ts
        .client2
        .post(
            &format!("{ROOT_WS}/boards/{}/cards", board.id),
            &json!({
                "title": "Confidential card"
            }),
        )
        .await;
    card.check_created();
    let card_id = card.value()["id"].as_str().expect("a card ID").to_owned();
    let file = uploaded(&ts.client2, &board.id, b"confidential file bytes").await;
    file.check_created();
    let file_id = file.value()["fileId"]
        .as_str()
        .expect("a file ID")
        .to_owned();
    let upload = ts
        .client2
        .post(
            &format!("{ROOT_WS}/containers/{}/uploads", board.id),
            &json!({
                "name": "after-removal.txt", "size": 0
            }),
        )
        .await;
    upload.check_created();
    let upload_id = upload.value()["uploadId"]
        .as_str()
        .expect("an upload ID")
        .to_owned();
    for table in ["workspace_members", "organization_members"] {
        ts.app
            .store()
            .execute_unchecked(&format!("DELETE FROM {table} WHERE user_id = '{user_id}'"))
            .await
            .expect("admission is removed without changing board roles");
    }
    if destination == AfterRemoval::OtherOrganization {
        let workspace = Workspace {
            id: new_id(),
            ..Workspace::default()
        };
        let org_id = ts
            .app
            .store()
            .create_organization("Only remaining organization", &workspace)
            .await
            .expect("a second organization");
        let store = ts.app.store().for_org(&org_id).expect("an org ID");
        store
            .add_organization_member(&user_id, OrgRole::Member)
            .await
            .expect("membership only in the second organization");
        store
            .add_workspace_member(&workspace.id, &user_id, WorkspaceRole::Member)
            .await
            .expect("its workspace membership");
    }
    ts.client2
        .get(&format!("{ROOT_WS}/boards"))
        .await
        .check_not_found();
    FormerMember {
        ts,
        board,
        card_id,
        file_id,
        upload_id,
        user_id,
    }
}

fn read_paths(board: &Container, card_id: &str, file_id: &str) -> Vec<(String, String)> {
    vec![
        (
            format!("{ROOT_WS}/containers/{}", board.id),
            format!("{ROOT_WS}/containers/{UNKNOWN_ID}"),
        ),
        (
            format!("{ROOT_WS}/boards/{}/blocks", board.id),
            format!("{ROOT_WS}/boards/{UNKNOWN_ID}/blocks"),
        ),
        (
            format!("{ROOT_WS}/boards/{}/cards", board.id),
            format!("{ROOT_WS}/boards/{UNKNOWN_ID}/cards"),
        ),
        (
            format!("{ROOT_WS}/containers/{}/members", board.id),
            format!("{ROOT_WS}/containers/{UNKNOWN_ID}/members"),
        ),
        (
            format!("{ROOT_WS}/containers/{}/public-link", board.id),
            format!("{ROOT_WS}/containers/{UNKNOWN_ID}/public-link"),
        ),
        (
            format!("{ROOT_WS}/boards/{}/trash/cards", board.id),
            format!("{ROOT_WS}/boards/{UNKNOWN_ID}/trash/cards"),
        ),
        (
            format!("{ROOT_WS}/containers/{}/archive/export", board.id),
            format!("{ROOT_WS}/containers/{UNKNOWN_ID}/archive/export"),
        ),
        (
            format!("{ROOT_WS}/cards/{card_id}"),
            format!("{ROOT_WS}/cards/{UNKNOWN_ID}"),
        ),
        (
            format!("{ROOT_WS}/containers/{}/files/{file_id}", board.id),
            format!("{ROOT_WS}/containers/{UNKNOWN_ID}/files/{file_id}"),
        ),
        (
            format!("{ROOT_WS}/containers/{}/files/{file_id}/info", board.id),
            format!("{ROOT_WS}/containers/{UNKNOWN_ID}/files/{file_id}/info"),
        ),
    ]
}

async fn id_routes_after_removal(destination: AfterRemoval) {
    let former = former_member(destination).await;
    let FormerMember {
        ts,
        board,
        card_id,
        file_id,
        upload_id,
        user_id,
    } = former;
    let mut violations = Vec::new();
    // Inventory of read routes reached by a board, card, file or block identifier.
    // None has a read token; every successful response here crosses the workspace wall.
    for (hidden, absent) in read_paths(&board, &card_id, &file_id) {
        let response = ts.client2.get(&hidden).await;
        let missing = ts.client2.get(&absent).await;
        if response.status != StatusCode::NOT_FOUND || answer(&response) != answer(&missing) {
            violations.push(format!(
                "GET {hidden}: {} versus absent {}",
                response.status, missing.status
            ));
        }
    }
    // Inventory of write routes which otherwise trust the remaining board role.
    for (method, path, body) in [
        (
            Method::PATCH,
            format!("{ROOT_WS}/containers/{}", board.id),
            json!({"title": "Modified after removal"}),
        ),
        (
            Method::PATCH,
            format!("{ROOT_WS}/cards/{card_id}"),
            json!({"title": "Card modified after removal"}),
        ),
        (
            Method::POST,
            format!("{ROOT_WS}/boards/{}/cards", board.id),
            json!({"title": "New card after removal"}),
        ),
        (
            Method::PUT,
            format!("{ROOT_WS}/boards/{}/property-order", board.id),
            json!([]),
        ),
        (
            Method::POST,
            format!("{ROOT_WS}/containers/{}/uploads", board.id),
            json!({"name": "new.txt", "size": 0}),
        ),
        (
            Method::POST,
            format!("{ROOT_WS}/uploads/{upload_id}/complete"),
            json!({}),
        ),
        (
            Method::POST,
            format!("{ROOT_WS}/subscriptions"),
            json!({"blockId": card_id, "blockType": "card", "userId": user_id}),
        ),
    ] {
        let response = Client::send(ts.client2.request(method.clone(), &path).json(&body)).await;
        if response.status != StatusCode::NOT_FOUND {
            violations.push(format!(
                "{method} {path}: {} {}",
                response.status,
                response.text()
            ));
        }
    }
    let file = uploaded(&ts.client2, &board.id, b"new file after removal").await;
    if file.status != StatusCode::NOT_FOUND {
        violations.push(format!("POST board/files: {}", file.status));
    }
    let stored_title = ts
        .app
        .store()
        .get_container(&board.id)
        .await
        .expect("the board remains")
        .title;
    ts.teardown().await;
    assert!(
        violations.is_empty(),
        "former member still reaches workspace resources (destination={destination:?}, stored title={stored_title:?}):\n{}",
        violations.join("\n")
    );
}

#[tokio::test]
async fn identifier_routes_reject_a_user_removed_from_the_workspace() {
    Box::pin(id_routes_after_removal(AfterRemoval::NoOrganization)).await;
}

#[tokio::test]
async fn identifier_routes_reject_a_user_who_now_belongs_only_to_another_org() {
    if db_type_of_the_tests() == DbType::Postgres {
        Box::pin(id_routes_after_removal(AfterRemoval::OtherOrganization)).await;
    }
}

#[tokio::test]
async fn a_former_workspace_admin_cannot_restore_its_deleted_board() {
    let ts = Setup::native().start().await.init_basic().await;
    let user_id = ts.client2.me().await.id;
    let board: Container = ts
        .client2
        .post(
            &format!("{ROOT_WS}/boards"),
            &json!({
                "workspaceId": ROOT_WORKSPACE_ID, "visibility": "private", "title": "Deleted workspace secret"
            }),
        )
        .await
        .created_json();
    ts.client2
        .delete(&format!("{ROOT_WS}/containers/{}", board.id))
        .await
        .check_no_content();
    ts.app
        .store()
        .execute_unchecked(&format!(
            "DELETE FROM workspace_members WHERE user_id = '{user_id}'"
        ))
        .await
        .expect("the workspace membership ends");
    let response = ts
        .client2
        .post(
            &format!("{ROOT_WS}/containers/{}/restore", board.id),
            &json!({}),
        )
        .await;
    let restored = ts.app.store().get_container(&board.id).await.is_ok();
    ts.teardown().await;
    assert_eq!(
        (response.status, restored),
        (StatusCode::NOT_FOUND, false),
        "the trash snapshot must not grant a former workspace member access"
    );
}
