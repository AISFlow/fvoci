//! The forms requests are read in: a member that is `null` only where it may be, the
//! paging of lists from 0, and a block on its board that has no parent.

use fvoci_model::{Block, BlockType, ContainerVisibility};
use reqwest::StatusCode;
use serde_json::json;

use crate::blocks::{
    blocks_of, blocks_path, create_board_in_store, insert, insert_one, new_block, titles,
};
use crate::common::{Client, Setup, TEST_WORKSPACE_ID, TEST_WS};

/// A request member that is no `Option` refuses `null`, as it refuses a value of another
/// type; one that is (a block's `body`, a patch's `title`) takes it as none.
#[tokio::test]
async fn null_is_refused_where_the_contract_does_not_allow_it() {
    let th = Setup::one_user().start().await;
    let board = create_board_in_store(&th, ContainerVisibility::Open).await;
    let anchor = insert_one(&th.client, &new_block(&board.id, BlockType::Card, "anchor")).await;
    let path = blocks_path(&board.id);
    let block = |member: &str, value: serde_json::Value| {
        let mut block = json!({
            "id": "x1",
            "containerId": board.id,
            "parentId": anchor.id,
            "type": "comment",
            "title": "t",
        });
        block[member] = value;
        json!([block])
    };

    for member in ["title", "fields", "parentId", "createdAt", "type"] {
        let response = th.client.post(&path, &block(member, json!(null))).await;
        response.check_bad_request();
        assert_eq!(
            response.problem_type(),
            "/problems/bad-request",
            "a null {member}"
        );
    }
    assert_eq!(
        blocks_of(&th.client, &board.id).await.len(),
        1,
        "only the anchor card was stored"
    );

    let made: Vec<Block> = th
        .client
        .post(&path, &block("body", json!(null)))
        .await
        .created_json();
    assert_eq!(made[0].body, None, "a null body is no body");

    let patch_path = format!("{path}/{}", made[0].id);
    for patch in [
        json!({"updatedFields": null}),
        json!({"deletedFields": null}),
    ] {
        th.client
            .patch(&patch_path, &patch)
            .await
            .check_bad_request();
    }
    let response = th.client.patch(&patch_path, &json!({"title": null})).await;
    assert!(
        response.status.is_success(),
        "a null title of a patch changes nothing: {}",
        response.text()
    );
    assert_eq!(
        blocks_of(&th.client, &board.id)
            .await
            .iter()
            .find(|b| b.id == made[0].id)
            .expect("comment exists")
            .title,
        "t"
    );

    th.teardown().await;
}

/// A new card and a patch of a card refuse `null` for their members that are no
/// `Option`, and a patch takes it as none for those that are.
#[tokio::test]
async fn null_is_refused_by_cards_where_the_contract_does_not_allow_it() {
    let th = Setup::one_user().start().await;
    let board = create_board_in_store(&th, ContainerVisibility::Open).await;
    let cards = format!("{TEST_WS}/boards/{}/cards", board.id);

    for member in ["title", "properties", "icon"] {
        let response = th.client.post(&cards, &json!({ member: null })).await;
        response.check_bad_request();
    }
    let card: serde_json::Value = th.client.post(&cards, &json!({})).await.created_json();
    let path = format!("{TEST_WS}/cards/{}", card["id"].as_str().expect("an id"));
    for member in ["updatedProperties", "deletedProperties"] {
        let response = th.client.patch(&path, &json!({ member: null })).await;
        response.check_bad_request();
    }
    let response = th
        .client
        .patch(&path, &json!({"title": null, "icon": null}))
        .await;
    assert!(response.status.is_success(), "{}", response.text());

    th.teardown().await;
}

/// `page` is a whole number from 0, `perPage` one from 1 to 1000 and 50 when left out:
/// any other is refused at the parameter, both told of when both are wrong.
#[tokio::test]
async fn the_pages_of_a_list_are_of_1_to_1000() {
    let th = Setup::one_user().start().await;
    let board = create_board_in_store(&th, ContainerVisibility::Open).await;
    let cards: Vec<Block> = (0..60)
        .map(|index| new_block(&board.id, BlockType::Card, &format!("card {index}")))
        .collect();
    insert(&th.client, &board.id, &cards).await;
    let cards_path = format!("{TEST_WS}/boards/{}/cards", board.id);
    let count = |query: &str| {
        let path = format!("{cards_path}?{query}");
        let client = &th.client;
        async move {
            client
                .get(&path)
                .await
                .ok_json::<Vec<serde_json::Value>>()
                .len()
        }
    };

    assert_eq!(count("").await, 50, "left out, 50");
    assert_eq!(count("perPage=1").await, 1, "the least");
    assert_eq!(count("perPage=1000").await, 60, "the most: every card here");
    assert_eq!(count("page=1").await, 10, "the second page of 50");
    assert_eq!(count("page=1&perPage=59").await, 1, "the second page of 59");
    assert_eq!(
        count("page=9&perPage=50").await,
        0,
        "a page past the last is empty"
    );
    let largest = "page=2147483647&perPage=1000";
    assert_eq!(
        count(largest).await,
        0,
        "the last page there can be is empty"
    );

    let trash_path = format!("{TEST_WS}/boards/{}/trash/cards", board.id);
    for path in [&cards_path, &trash_path] {
        for (query, refused, detail) in [
            ("page=-1", "page", "below 0"),
            ("page=2147483648", "page", "above 2147483647"),
            ("perPage=0", "perPage", "below 1"),
            ("perPage=-1", "perPage", "below 1"),
            ("perPage=1001", "perPage", "above 1000"),
        ] {
            let response = th.client.get(&format!("{path}?{query}")).await;
            response.check_bad_request();
            assert_eq!(
                response.refused_at(),
                format!("parameter {refused}"),
                "{path}?{query}"
            );
            assert!(
                response.text().contains(detail),
                "{query}: {}",
                response.text()
            );
        }
        let response = th.client.get(&format!("{path}?page=-1&perPage=0")).await;
        response.check_bad_request();
        assert_eq!(
            response.refused_places(),
            ["parameter page", "parameter perPage"],
            "both are told of"
        );
    }

    th.teardown().await;
}

/// A block directly on its board has no parent: the board's identifier is no parent, as
/// the identifier of no block is none, when a block is made, when it is moved, and when
/// the blocks of a board are narrowed to those below one.
#[tokio::test]
async fn the_board_is_no_parent_of_its_blocks() {
    let th = Setup::one_user().start().await;
    let board = create_board_in_store(&th, ContainerVisibility::Open).await;
    let path = blocks_path(&board.id);

    let on_board = Block {
        parent_id: board.id.clone(),
        ..new_block(&board.id, BlockType::View, "under the board")
    };
    let response = th.client.insert_blocks(&board.id, &[on_board]).await;
    assert_eq!(
        response.status,
        StatusCode::BAD_REQUEST,
        "{}",
        response.text()
    );
    assert_eq!(response.refused_at(), "/0/parentId");

    let card = insert_one(&th.client, &new_block(&board.id, BlockType::Card, "card")).await;
    assert_eq!(card.parent_id, "", "a block on its board has no parent");
    let response = th
        .client
        .patch(
            &format!("{path}/{}", card.id),
            &json!({"parentId": board.id}),
        )
        .await;
    response.check_bad_request();
    assert_eq!(response.refused_at(), "/parentId");

    let below_board: Vec<Block> = th
        .client
        .get(&format!("{path}?parentId={}", board.id))
        .await
        .ok_json();
    assert!(
        titles(&below_board).is_empty(),
        "no block is below the board"
    );
    assert_eq!(titles(&blocks_of(&th.client, &board.id).await), ["card"]);

    th.teardown().await;
}

/// Every list answered in pages reads `page` and `perPage` alike: 1 and 1000 are taken,
/// as is leaving them out; 0 and 1001 are refused at the parameter.
#[tokio::test]
async fn every_list_takes_1_to_1000_to_a_page() {
    let th = Setup::native().start().await.init_basic().await;
    let org = crate::org_admin::org_of(&th).await;
    let made = th
        .client
        .post(
            &format!("{TEST_WS}/wikis"),
            &json!({"visibility": "private"}),
        )
        .await;
    made.check_created();
    let wiki = made.value()["id"].as_str().expect("an id").to_owned();
    let board = create_board_in_store(&th, ContainerVisibility::Open).await;
    let card = insert_one(&th.client, &new_block(&board.id, BlockType::Card, "card")).await;
    for path in [
        format!("/org/{org}/members"),
        format!("/org/{org}/audit"),
        format!("{TEST_WS}/audit"),
        format!("{TEST_WS}/wikis/{wiki}/trash/pages"),
        format!("{TEST_WS}/trash/containers"),
        format!("{TEST_WS}/boards/{}/cards", board.id),
        format!("{TEST_WS}/boards/{}/trash/cards", board.id),
        format!("{TEST_WS}/references/to/{}", card.id),
    ] {
        takes_1_to_1000_to_a_page(&th.client, &format!("{path}?")).await;
    }

    th.teardown().await;
}

/// The cards of a board read by its link take a page as every list does.
#[tokio::test]
async fn the_cards_of_a_shared_board_take_1_to_1000_to_a_page() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let board = create_board_in_store(&th, ContainerVisibility::Open).await;
    insert_one(&th.client, &new_block(&board.id, BlockType::Card, "card")).await;
    let token = th.share(&board.id).await;
    let path = format!("/shared/{}/cards?readToken={token}&", board.id);
    takes_1_to_1000_to_a_page(&th.anonymous(), &path).await;

    th.teardown().await;
}

/// Checks that the list of `path` (ending in `?` or `&`) answers pages of 1 to 1000 and
/// refuses the rest at the parameter.
async fn takes_1_to_1000_to_a_page(client: &Client, path: &str) {
    for query in ["", "perPage=1", "perPage=1000", "page=3&perPage=1000"] {
        client.get(&format!("{path}{query}")).await.check_ok();
    }
    for (query, refused) in [
        ("page=-1", "page"),
        ("perPage=0", "perPage"),
        ("perPage=1001", "perPage"),
        ("perPage=2147483648", "perPage"),
    ] {
        let response = client.get(&format!("{path}{query}")).await;
        response.check_bad_request();
        assert_eq!(
            response.refused_at(),
            format!("parameter {refused}"),
            "{path}{query}"
        );
    }
}

/// The board is no parent in a batch of patches, nor where boards and blocks are made
/// together, where the blocks name their board by the request's own name for it.
#[tokio::test]
async fn the_board_is_no_parent_in_a_batch_or_with_new_boards() {
    let th = Setup::one_user().start().await;
    let board = create_board_in_store(&th, ContainerVisibility::Open).await;
    let card = insert_one(&th.client, &new_block(&board.id, BlockType::Card, "card")).await;

    let response = th
        .client
        .patch(
            &blocks_path(&board.id),
            &json!({"blockIds": [card.id], "blockPatches": [{"parentId": board.id}]}),
        )
        .await;
    response.check_bad_request();
    assert_eq!(response.refused_at(), "/blockPatches/0/parentId");
    assert_eq!(blocks_of(&th.client, &board.id).await[0].parent_id, "");

    let response = th
        .client
        .post(
            &format!("{TEST_WS}/containers-and-blocks"),
            &json!({
                "containers": [{"id": "b", "workspaceId": crate::common::TEST_WORKSPACE_ID, "visibility": "private"}],
                "blocks": [{"id": "x", "containerId": "b", "parentId": "b", "type": "card",
                            "createdAt": 1, "updatedAt": 1}],
            }),
        )
        .await;
    response.check_bad_request();
    assert_eq!(response.refused_at(), "/blocks/0/parentId");

    th.teardown().await;
}

/// A category's `type` and `name` are strings: `null` is neither, and refused as a blank
/// `type` is.
#[tokio::test]
async fn null_is_refused_by_categories() {
    let th = Setup::one_user().start().await;
    let user = th.client.me().await;
    let path = format!("{TEST_WS}/categories");
    for (member, blank) in [("type", json!(null)), ("name", json!(null))] {
        let mut body =
            json!({"name": "Named", "userId": user.id, "workspaceId": TEST_WORKSPACE_ID});
        body[member] = blank;
        let response = th.client.post(&path, &body).await;
        response.check_bad_request();
        assert!(
            response.text().contains("invalid type: null"),
            "{member}: {}",
            response.text()
        );
    }
    let body =
        json!({"name": "Blank", "userId": user.id, "workspaceId": TEST_WORKSPACE_ID, "type": ""});
    th.client.post(&path, &body).await.check_bad_request();

    th.teardown().await;
}
