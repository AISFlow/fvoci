//! Card hierarchy HTTP API, WebSocket broadcasts, and archive export/import roundtrips.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::Write as _;

use fvoci_model::{BlockType, Card, ContainerVisibility};
use reqwest::StatusCode;
use serde_json::{Value, json};
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

use crate::board_broadcasts::WorkspaceSocket;
use crate::common::{Client, Setup, TEST_WORKSPACE_ID, TEST_WS};
use crate::export::import;

fn cards_path(board_id: &str) -> String {
    format!("{TEST_WS}/boards/{board_id}/cards")
}

fn card_path(card_id: &str) -> String {
    format!("{TEST_WS}/cards/{card_id}")
}

async fn make_card(client: &Client, board_id: &str, body: Value) -> Card {
    client
        .post(&cards_path(board_id), &body)
        .await
        .created_json()
}

fn build_zip(board_id: &str, container_jsonl: &str) -> Vec<u8> {
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut cursor);
        let options = SimpleFileOptions::default();
        zip.start_file("version.json", options).expect("version");
        zip.write_all(br#"{"version":1,"date":1700000000000}"#)
            .expect("write version");
        zip.start_file(format!("{board_id}/container.jsonl"), options)
            .expect("jsonl");
        zip.write_all(container_jsonl.as_bytes())
            .expect("write jsonl");
        zip.finish().expect("finish zip");
    }
    cursor.into_inner()
}

#[tokio::test]
async fn card_api_creates_reads_and_patches_parent_id() {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;

    let d1 = make_card(&th.client, &board.id, json!({ "title": "d1" })).await;
    assert_eq!(d1.parent_id, "", "root card parentId is empty");
    let d2 = make_card(
        &th.client,
        &board.id,
        json!({ "title": "d2", "parentId": d1.id }),
    )
    .await;
    assert_eq!(d2.parent_id, d1.id, "depth 2 parentId");
    let d3 = make_card(
        &th.client,
        &board.id,
        json!({ "title": "d3", "parentId": d2.id }),
    )
    .await;
    assert_eq!(d3.parent_id, d2.id, "depth 3 parentId");

    // GET single and GET list expose parentId.
    let fetched: Card = th.client.get(&card_path(&d3.id)).await.ok_json();
    assert_eq!(fetched.parent_id, d2.id, "GET single exposes parentId");
    let listed: Vec<Card> = th.client.get(&cards_path(&board.id)).await.ok_json();
    let by_id: HashMap<&str, &str> = listed
        .iter()
        .map(|card| (card.id.as_str(), card.parent_id.as_str()))
        .collect();
    assert_eq!(by_id[d1.id.as_str()], "", "listed d1 parentId");
    assert_eq!(by_id[d2.id.as_str()], d1.id, "listed d2 parentId");
    assert_eq!(by_id[d3.id.as_str()], d2.id, "listed d3 parentId");

    // Patching without parentId keeps parentId; "" and null clear it to top level.
    let renamed: Card = th
        .client
        .patch(&card_path(&d3.id), &json!({ "title": "d3-renamed" }))
        .await
        .ok_json();
    assert_eq!(renamed.parent_id, d2.id, "omitted parentId preserved");

    let cleared_null: Card = th
        .client
        .patch(&card_path(&d3.id), &json!({ "parentId": null }))
        .await
        .ok_json();
    assert_eq!(cleared_null.parent_id, "", "null parentId clears to root");

    let moved_back: Card = th
        .client
        .patch(&card_path(&d3.id), &json!({ "parentId": d2.id }))
        .await
        .ok_json();
    assert_eq!(moved_back.parent_id, d2.id, "reparent back under d2");

    let cleared_empty: Card = th
        .client
        .patch(&card_path(&d3.id), &json!({ "parentId": "" }))
        .await
        .ok_json();
    assert_eq!(cleared_empty.parent_id, "", "empty parentId clears to root");

    th.teardown().await;
}

#[tokio::test]
async fn card_api_rejects_invalid_hierarchy_operations() {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let other_board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;

    let d1 = make_card(&th.client, &board.id, json!({ "title": "d1" })).await;
    let d2 = make_card(
        &th.client,
        &board.id,
        json!({ "title": "d2", "parentId": d1.id }),
    )
    .await;
    let d3 = make_card(
        &th.client,
        &board.id,
        json!({ "title": "d3", "parentId": d2.id }),
    )
    .await;

    // Depth 4 on create returns 400 with reason card_too_deep, limit 3, pointer /parentId.
    let too_deep = th
        .client
        .post(
            &cards_path(&board.id),
            &json!({ "title": "d4", "parentId": d3.id }),
        )
        .await;
    assert_eq!(
        too_deep.status,
        StatusCode::BAD_REQUEST,
        "depth 4 rejected with 400"
    );
    let problem: Value = too_deep.json();
    assert_eq!(problem["reason"], json!("card_too_deep"), "depth 4 reason");
    assert_eq!(problem["limit"], json!(3), "depth 4 limit");
    assert_eq!(
        problem["errors"][0]["pointer"],
        json!("/parentId"),
        "depth 4 pointer"
    );

    // Foreign board card, template card, template with parent, self, and cycles are all 400.
    let foreign = make_card(&th.client, &other_board.id, json!({ "title": "f" })).await;
    let tmpl = make_card(
        &th.client,
        &board.id,
        json!({ "title": "tmpl", "isTemplate": true }),
    )
    .await;
    for bad in [&foreign.id, &tmpl.id] {
        let resp = th
            .client
            .post(
                &cards_path(&board.id),
                &json!({ "title": "x", "parentId": bad }),
            )
            .await;
        assert_eq!(resp.status, StatusCode::BAD_REQUEST, "bad parent rejected");
        assert_eq!(
            resp.json::<Value>()["errors"][0]["pointer"],
            json!("/parentId"),
            "bad parent pointer"
        );
    }
    let tmpl_with_parent = th
        .client
        .post(
            &cards_path(&board.id),
            &json!({ "title": "t2", "isTemplate": true, "parentId": d1.id }),
        )
        .await;
    assert_eq!(
        tmpl_with_parent.status,
        StatusCode::BAD_REQUEST,
        "template card with parent rejected"
    );

    for bad_target in [&d1.id, &d2.id, &d3.id] {
        let resp = th
            .client
            .patch(&card_path(&d1.id), &json!({ "parentId": bad_target }))
            .await;
        assert_eq!(
            resp.status,
            StatusCode::BAD_REQUEST,
            "self/cycle reparent rejected"
        );
        assert_eq!(
            resp.json::<Value>()["errors"][0]["pointer"],
            json!("/parentId"),
            "self/cycle pointer"
        );
    }

    th.teardown().await;
}

#[tokio::test]
async fn ws_broadcasts_subcard_deletion_and_restoration() {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let me = th.client.me().await.id;
    let d1 = make_card(&th.client, &board.id, json!({ "title": "d1" })).await;
    let d2 = make_card(
        &th.client,
        &board.id,
        json!({ "title": "d2", "parentId": d1.id }),
    )
    .await;
    let d3 = make_card(
        &th.client,
        &board.id,
        json!({ "title": "d3", "parentId": d2.id }),
    )
    .await;

    let mut socket = WorkspaceSocket::subscribe(&th, &th.client, TEST_WORKSPACE_ID, &me).await;

    crate::blocks::trash_card(&th.client, &board.id, &d1.id)
        .await
        .check_no_content();
    for id in [&d1.id, &d2.id, &d3.id] {
        let msg = socket.expect("blockChanged", "/block/id", id).await;
        assert!(
            msg["block"]["deletedAt"].as_i64() > Some(0),
            "sub-card {id} delete broadcast: {msg}"
        );
    }

    let restored: fvoci_model::RestoredCards =
        crate::blocks::restore_card(&th.client, &board.id, &d1.id)
            .await
            .ok_json();
    assert!(
        restored.cards.iter().any(|card| card.id == d1.id),
        "the restore answers the top card"
    );
    for (id, expected_parent) in [
        (&d1.id, ""),
        (&d2.id, d1.id.as_str()),
        (&d3.id, d2.id.as_str()),
    ] {
        let msg = socket.expect("blockChanged", "/block/id", id).await;
        assert_eq!(msg["block"].get("deletedAt"), None, "restored deletedAt");
        assert_eq!(
            msg["block"]["parentId"],
            json!(expected_parent),
            "restored parentId"
        );
    }

    th.teardown().await;
}

#[tokio::test]
async fn export_import_roundtrip_preserves_card_hierarchy() {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;

    // Create child before parent, then reparent so creation order is reversed.
    let child = make_card(&th.client, &board.id, json!({ "title": "child" })).await;
    let parent = make_card(&th.client, &board.id, json!({ "title": "parent" })).await;
    th.client
        .patch(&card_path(&child.id), &json!({ "parentId": parent.id }))
        .await
        .check_ok();
    let _grand = make_card(
        &th.client,
        &board.id,
        json!({ "title": "grand", "parentId": child.id }),
    )
    .await;

    let exported = th
        .client
        .get(&format!("{TEST_WS}/containers/{}/archive/export", board.id))
        .await;
    exported.check_ok();
    import(&th.client, TEST_WORKSPACE_ID, &exported.body)
        .await
        .check_ok();

    let user_id = th.client.me().await.id;
    let boards = th
        .app
        .containers_for_user_and_workspace(
            &user_id,
            TEST_WORKSPACE_ID,
            fvoci_model::PublicContainers::Include,
        )
        .await
        .expect("workspace containers");
    let imported = boards
        .iter()
        .find(|container| container.id != board.id)
        .expect("imported board");
    let imported_cards: Vec<Card> = th.client.get(&cards_path(&imported.id)).await.ok_json();
    let by_title: HashMap<&str, &Card> = imported_cards
        .iter()
        .map(|card| (card.title.as_str(), card))
        .collect();
    assert_eq!(by_title["parent"].parent_id, "", "imported root parentId");
    assert_eq!(
        by_title["child"].parent_id, by_title["parent"].id,
        "imported child remapped to new parent id"
    );
    assert_eq!(
        by_title["grand"].parent_id, by_title["child"].id,
        "imported grand remapped to new child id"
    );
    assert_ne!(by_title["parent"].id, parent.id, "parent got new id");
    assert_ne!(by_title["child"].id, child.id, "child got new id");

    th.teardown().await;
}

#[tokio::test]
async fn hostile_archives_are_rejected_without_storing_boards() {
    let th = Setup::native().start().await.init_basic().await;
    let user_id = th.client.me().await.id;
    let count_before = th
        .app
        .containers_for_user_and_workspace(
            &user_id,
            TEST_WORKSPACE_ID,
            fvoci_model::PublicContainers::Include,
        )
        .await
        .expect("workspace containers before")
        .len();

    let board_line = json!({
        "type": "container",
        "data": { "id": "b1", "visibility": "private", "kind": "board", "title": "Bad", "createdAt": 1, "updatedAt": 1 }
    });
    let card_line = |id: &str, parent_id: &str, is_template: bool| {
        json!({
            "type": "block",
            "data": {
                "id": id,
                "containerId": "b1",
                "parentId": parent_id,
                "type": BlockType::Card,
                "title": id,
                "fields": { "isTemplate": is_template },
                "createdAt": 1,
                "updatedAt": 1
            }
        })
    };

    let hostile_cases: [(&str, Vec<Value>); 5] = [
        (
            "depth-4",
            vec![
                card_line("c1", "", false),
                card_line("c2", "c1", false),
                card_line("c3", "c2", false),
                card_line("c4", "c3", false),
            ],
        ),
        (
            "duplicate-id-depth-bypass",
            vec![
                card_line("c2", "", false),
                card_line("c1", "", false),
                card_line("c2", "c1", false),
                card_line("c3", "c2", false),
                card_line("c4", "c3", false),
            ],
        ),
        (
            "2-cycle",
            vec![card_line("c1", "c2", false), card_line("c2", "c1", false)],
        ),
        (
            "3-cycle",
            vec![
                card_line("c1", "c3", false),
                card_line("c2", "c1", false),
                card_line("c3", "c2", false),
            ],
        ),
        (
            "under-template",
            vec![card_line("t1", "", true), card_line("c1", "t1", false)],
        ),
    ];

    for (name, lines) in hostile_cases {
        let mut jsonl = format!("{board_line}\n");
        for line in lines {
            writeln!(jsonl, "{line}").expect("write jsonl line");
        }
        let zip = build_zip("b1", &jsonl);
        let resp = import(&th.client, TEST_WORKSPACE_ID, &zip).await;
        assert_eq!(
            resp.status,
            StatusCode::BAD_REQUEST,
            "hostile archive {name} must be rejected"
        );
    }

    let boards_after = th
        .app
        .containers_for_user_and_workspace(
            &user_id,
            TEST_WORKSPACE_ID,
            fvoci_model::PublicContainers::Include,
        )
        .await
        .expect("workspace containers after");
    assert_eq!(
        boards_after.len(),
        count_before,
        "hostile archives must leave no boards behind"
    );

    th.teardown().await;
}
