//! References in copies: a copy of a board (also as a template, and a board made from
//! one), an imported archive and a copy of a card name the copies of what was copied
//! with them, and what was not copied as before. What names a block is counted for the
//! copies as for anything written.

use std::collections::HashMap;

use fvoci_model::{Block, BlockType, Container, ContainersAndBlocks};
use serde_json::json;

use crate::common::{Client, Setup, TEST_WORKSPACE_ID, TEST_WS, TestServer};
use crate::export::import;
use crate::references::{Made, made};
use crate::references_shared::references_in;
use crate::references_to::{card_names, card_of, comment_naming};

/// A board whose card `A` names the card `B` beside it, a card and a page that are
/// elsewhere, and itself; a comment on `B` names `A`.
struct Named {
    board: String,
    a: String,
    b: String,
    elsewhere: Made,
}

async fn named_board(th: &TestServer) -> Named {
    let user = &th.client;
    let elsewhere = made(user, TEST_WS, "private", "elsewhere").await;
    let board: Container = user
        .post(
            &format!("{TEST_WS}/boards"),
            &json!({"workspaceId": TEST_WORKSPACE_ID, "visibility": "private", "title": "Named"}),
        )
        .await
        .created_json();
    let a = card_of(user, TEST_WS, &board.id, "A").await;
    let b = card_of(user, TEST_WS, &board.id, "B").await;
    card_names(
        user,
        TEST_WS,
        &a,
        &[&b, &elsewhere.card, &a, &elsewhere.page],
    )
    .await;
    comment_naming(user, &board.id, &b, &[&a]).await;
    Named {
        board: board.id,
        a,
        b,
        elsewhere,
    }
}

/// The blocks of a board that have a body.
async fn with_bodies(client: &Client, board_id: &str) -> Vec<Block> {
    let blocks: Vec<Block> = client
        .get(&format!("{TEST_WS}/boards/{board_id}/blocks?all=true"))
        .await
        .ok_json();
    blocks
        .into_iter()
        .filter(|block| block.body.is_some())
        .collect()
}

/// What the cards of a board name, by the title of the card that names: each card of
/// the board that is named by its title, anything else by its identifier.
async fn named_by_title(client: &Client, board_id: &str) -> HashMap<String, Vec<String>> {
    let blocks: Vec<Block> = client
        .get(&format!("{TEST_WS}/boards/{board_id}/blocks?all=true"))
        .await
        .ok_json();
    let titles: HashMap<&str, &str> = blocks
        .iter()
        .filter(|block| block.block_type == BlockType::Card)
        .map(|block| (block.id.as_str(), block.title.as_str()))
        .collect();
    blocks
        .iter()
        .filter(|block| block.block_type == BlockType::Card)
        .map(|block| {
            let body = serde_json::to_value(&block.body).expect("JSON");
            let named = references_in(&body)
                .into_iter()
                .map(|id| {
                    titles
                        .get(id.as_str())
                        .map_or(id.clone(), |title| (*title).to_owned())
                })
                .collect();
            (block.title.clone(), named)
        })
        .collect()
}

/// The identifiers of what names the block, as the user is told.
async fn naming(client: &Client, block_id: &str) -> Vec<String> {
    let answer = client
        .get(&format!("{TEST_WS}/references/to/{block_id}"))
        .await;
    answer.check_ok();
    let listed = answer.value();
    let listed = listed.as_array().expect("a list").iter();
    listed
        .map(|linked| linked["id"].as_str().expect("an id").to_owned())
        .collect()
}

/// Checks that the board `copy_id` is a copy of the named board as far as what its
/// cards name goes: `A` names the `B` and the `A` of the copy and what is elsewhere,
/// and that is counted.
async fn check_names_its_own(th: &TestServer, source: &Named, copy_id: &str, what: &str) {
    let user = &th.client;
    let elsewhere = &source.elsewhere;
    let named = named_by_title(user, copy_id).await;
    assert_eq!(
        named["A"],
        ["B", elsewhere.card.as_str(), "A", elsewhere.page.as_str()],
        "{what}"
    );
    let copies = with_bodies(user, copy_id).await;
    let id_of = |title: &str| {
        let card = copies.iter().find(|block| block.title == title);
        card.map(|card| card.id.clone())
    };
    let copy_of_a = id_of("A").expect("the copy of A");
    assert!(copy_of_a != source.a, "{what}");
    let cards: Vec<Block> = user
        .get(&format!("{TEST_WS}/boards/{copy_id}/blocks?type=card"))
        .await
        .ok_json();
    let copy_of_b = cards
        .iter()
        .find(|card| card.title == "B")
        .expect("the copy of B");
    assert_eq!(
        naming(user, &copy_of_b.id).await,
        [copy_of_a.as_str()],
        "{what}"
    );
    // What is elsewhere is named by the source and by the copy.
    let of_elsewhere = naming(user, &elsewhere.card).await;
    assert!(
        of_elsewhere.contains(&source.a) && of_elsewhere.contains(&copy_of_a),
        "{what}: {of_elsewhere:?}"
    );
    // The source names what it named.
    assert_eq!(naming(user, &source.b).await, [source.a.as_str()], "{what}");
}

#[tokio::test]
async fn a_copy_of_a_board_names_its_own_cards_in_its_bodies() {
    let th = Setup::native().start().await.init_basic().await;
    let source = named_board(&th).await;
    let copy: ContainersAndBlocks = th
        .client
        .post_empty(&format!("{TEST_WS}/containers/{}/duplicate", source.board))
        .await
        .created_json();
    check_names_its_own(&th, &source, &copy.containers[0].id, "a copy").await;
    th.teardown().await;
}

#[tokio::test]
async fn a_template_and_a_board_made_from_it_name_their_own_cards_in_their_bodies() {
    let th = Setup::native().start().await.init_basic().await;
    let source = named_board(&th).await;
    let template: ContainersAndBlocks = th
        .client
        .post_empty(&format!(
            "{TEST_WS}/containers/{}/duplicate?asTemplate=true",
            source.board
        ))
        .await
        .created_json();
    let template = &template.containers[0];
    let made: ContainersAndBlocks = th
        .client
        .post_empty(&format!("{TEST_WS}/containers/{}/duplicate", template.id))
        .await
        .created_json();
    check_names_its_own(&th, &source, &template.id, "a template").await;
    check_names_its_own(&th, &source, &made.containers[0].id, "made from it").await;
    th.teardown().await;
}

#[tokio::test]
async fn an_imported_archive_names_the_imported_cards_in_its_bodies() {
    let th = Setup::native().start().await.init_basic().await;
    let source = named_board(&th).await;
    let archive = th
        .client
        .get(&format!(
            "{TEST_WS}/containers/{}/archive/export",
            source.board
        ))
        .await;
    archive.check_ok();
    import(&th.client, TEST_WORKSPACE_ID, &archive.body)
        .await
        .check_ok();
    let boards: Vec<Container> = th.client.get(&format!("{TEST_WS}/boards")).await.ok_json();
    let imported = boards
        .iter()
        .find(|each| each.id != source.board && each.title == "Named")
        .expect("the imported board");
    check_names_its_own(&th, &source, &imported.id, "an import").await;
    // A comment comes with its card, and names the card that came with it.
    let comments: Vec<Block> = with_bodies(&th.client, &imported.id)
        .await
        .into_iter()
        .filter(|block| block.block_type == BlockType::Comment)
        .collect();
    let named = references_in(&serde_json::to_value(&comments[0].body).expect("JSON"));
    let cards: Vec<Block> = th
        .client
        .get(&format!(
            "{TEST_WS}/boards/{}/blocks?type=card",
            imported.id
        ))
        .await
        .ok_json();
    let copy_of_a = cards.iter().find(|card| card.title == "A");
    assert_eq!(named, [copy_of_a.expect("the copy of A").id.as_str()]);
    th.teardown().await;
}

/// An archive of a workspace holds its boards: a body that names a card of another
/// board of the archive names the copy of that card, whichever of the two boards the
/// archive has first, and that is counted for the copies.
#[tokio::test]
async fn an_imported_archive_of_boards_names_the_copies_on_its_other_boards() {
    let th = Setup::native().start().await.init_basic().await;
    let user = &th.client;
    let board_of = async |title: &str| -> String {
        let board: Container = user
            .post(
                &format!("{TEST_WS}/boards"),
                &json!({"workspaceId": TEST_WORKSPACE_ID, "visibility": "private", "title": title}),
            )
            .await
            .created_json();
        board.id
    };
    let (one, two) = (board_of("One").await, board_of("Two").await);
    let a = card_of(user, TEST_WS, &one, "A").await;
    let b = card_of(user, TEST_WS, &two, "B").await;
    card_names(user, TEST_WS, &a, &[&b]).await;
    card_names(user, TEST_WS, &b, &[&a]).await;
    let archive = user.get(&format!("{TEST_WS}/archive/export")).await;
    archive.check_ok();
    import(user, TEST_WORKSPACE_ID, &archive.body)
        .await
        .check_ok();

    let boards: Vec<Container> = user.get(&format!("{TEST_WS}/boards")).await.ok_json();
    let copy_of = async |title: &str, source: &str, card: &str| -> Block {
        let board = boards
            .iter()
            .find(|each| each.title == title && each.id != source);
        let board = board.expect("the imported board");
        let blocks = with_bodies(user, &board.id).await;
        let copy = blocks.into_iter().find(|block| block.title == card);
        copy.expect("the copy of the card")
    };
    let (copy_of_a, copy_of_b) = (
        copy_of("One", &one, "A").await,
        copy_of("Two", &two, "B").await,
    );
    let names = |block: &Block| references_in(&serde_json::to_value(&block.body).expect("JSON"));
    assert_eq!(names(&copy_of_a), [copy_of_b.id.as_str()]);
    assert_eq!(names(&copy_of_b), [copy_of_a.id.as_str()]);
    assert_eq!(naming(user, &copy_of_b.id).await, [copy_of_a.id.as_str()]);
    assert_eq!(naming(user, &copy_of_a.id).await, [copy_of_b.id.as_str()]);
    // The sources name each other as they did, and nothing of the copies names them.
    assert_eq!(naming(user, &b).await, [a.as_str()]);
    assert_eq!(naming(user, &a).await, [b.as_str()]);
    th.teardown().await;
}

/// A copy of a card on its own board names itself where its source named itself, and
/// everything else as its source did: the card beside it was not copied.
#[tokio::test]
async fn a_copy_of_a_card_names_itself_and_what_its_source_named() {
    let th = Setup::native().start().await.init_basic().await;
    let source = named_board(&th).await;
    let copies: Vec<Block> = th
        .client
        .post_empty(&format!(
            "{TEST_WS}/boards/{}/blocks/{}/duplicate",
            source.board, source.a
        ))
        .await
        .created_json();
    let copy = &copies[0];
    let elsewhere = &source.elsewhere;
    let body = th
        .client
        .get(&format!("{TEST_WS}/cards/{}", copy.id))
        .await
        .value();
    assert_eq!(
        references_in(&body["body"]),
        [
            source.b.as_str(),
            elsewhere.card.as_str(),
            copy.id.as_str(),
            elsewhere.page.as_str()
        ]
    );
    assert_eq!(
        naming(&th.client, &source.b).await,
        [source.a.as_str(), copy.id.as_str()]
    );
    th.teardown().await;
}

/// Blocks that are made in one request and name each other by the identifiers the
/// request gave them name each other by the identifiers the server gave them.
#[tokio::test]
async fn blocks_made_together_name_each_other_by_their_new_identifiers() {
    let th = Setup::native().start().await.init_basic().await;
    let source = named_board(&th).await;
    let (first, second) = (fvoci_model::new_id(), fvoci_model::new_id());
    let reference = |id: &str| json!({"type": "reference", "attrs": {"id": id}});
    let body = json!({"type": "doc", "content": [{"type": "paragraph", "content": [
        reference(&second), reference(&source.b),
    ]}]});
    let created: serde_json::Value = th
        .client
        .post(
            &format!("{TEST_WS}/containers-and-blocks"),
            &json!({
                "containers": [{"id": "nb", "workspaceId": TEST_WORKSPACE_ID, "visibility": "private", "title": "Together"}],
                "blocks": [
                    {"id": first, "containerId": "nb", "type": "card", "title": "first",
                        "body": body},
                    {"id": second, "containerId": "nb", "type": "card", "title": "second"},
                ],
            }),
        )
        .await
        .created_json();
    let made: Vec<Block> = serde_json::from_value(created["blocks"].clone()).expect("blocks");
    assert!(made[0].id != first && made[1].id != second);
    assert_eq!(naming(&th.client, &made[1].id).await, [made[0].id.as_str()]);
    let of_b = naming(&th.client, &source.b).await;
    assert!(of_b.contains(&made[0].id), "{of_b:?}");
    th.teardown().await;
}
