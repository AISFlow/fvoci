//! Card relations in copies: a copy of a board (also as a template, and a board made
//! from one) and an imported archive name the copies of the cards their values named,
//! and leave out what they cannot name so. A copy of a card on its own board names the
//! cards its source named.

use std::collections::HashMap;

use fvoci_model::{Block, Card, Container, ContainersAndBlocks, new_id};
use serde_json::{Value, json};

use crate::common::TEST_WS;
use crate::common::{Client, Setup, TEST_WORKSPACE_ID, TestServer};
use crate::export::import;

const RELATION: &str = "rel";

/// A board whose cards name each other: `B` names `A` and `C`, `A` names a card that is
/// in the trash (`gone`) and, as an older server stored it, one that never was.
struct Named {
    board: Container,
    /// The cards by title.
    cards: HashMap<String, String>,
    view: String,
}

async fn named_board(th: &TestServer) -> Named {
    let client = &th.client;
    let board: Container = client
        .post(
            &format!("{TEST_WS}/boards"),
            &json!({
                "workspaceId": TEST_WORKSPACE_ID, "visibility": "private", "title": "Named",
                "cardProperties": [{"id": RELATION, "name": "After", "type": "relation"}],
            }),
        )
        .await
        .created_json();
    let mut cards = HashMap::new();
    for title in ["A", "B", "C", "gone"] {
        let card: Card = client
            .post(
                &format!("{TEST_WS}/boards/{}/cards", board.id),
                &json!({"title": title}),
            )
            .await
            .created_json();
        cards.insert(title.to_owned(), card.id);
    }
    let id = |title: &str| cards[title].clone();
    let view: Value = client
        .post(
            &format!("{TEST_WS}/boards/{}/views", board.id),
            &json!({
                "title": "Gantt", "viewType": "gantt",
                "fields": {"dependencyPropertyId": RELATION},
            }),
        )
        .await
        .created_json();
    client
        .patch(
            &format!("{TEST_WS}/cards/{}", id("B")),
            &json!({
                "updatedProperties": {
                    RELATION: [
                        {"card": id("A"), "kind": "ss", "lagDays": -3},
                        {"card": id("C"), "kind": "ff", "lagDays": 7},
                    ]
                }
            }),
        )
        .await
        .check_ok();
    client
        .patch(
            &format!("{TEST_WS}/cards/{}", id("A")),
            &json!({
                "updatedProperties": {
                    RELATION: [{"card": id("gone"), "kind": "sf", "lagDays": 2}]
                }
            }),
        )
        .await
        .check_ok();
    crate::blocks::trash_card(client, &board.id, &id("gone"))
        .await
        .check_no_content();
    let stale = json!({
        "properties": {
            RELATION: [
                {"card": id("gone"), "kind": "sf", "lagDays": 2},
                {"card": new_id(), "kind": "fs", "lagDays": 0},
                {"card": id("A"), "kind": "ss", "lagDays": 1},
            ]
        }
    });
    th.app
        .store()
        .set_block_fields_unchecked(&id("A"), stale.as_object().expect("an object"))
        .await
        .expect("written as an older server would have");
    Named {
        board,
        cards,
        view: view["view"]["id"]
            .as_str()
            .expect("the view's identifier")
            .to_owned(),
    }
}

/// The relation values of a board's cards by title, with each link's `card` replaced by
/// its title (`?` for what names no card of the board) and `kind`/`lagDays` preserved.
async fn relations_by_title(client: &Client, board_id: &str) -> HashMap<String, Value> {
    let blocks = crate::blocks::blocks_of(client, board_id).await;
    let titles: HashMap<&str, &str> = blocks
        .iter()
        .filter(|block| block.block_type == fvoci_model::BlockType::Card)
        .map(|block| (block.id.as_str(), block.title.as_str()))
        .collect();
    blocks
        .iter()
        .filter(|block| block.block_type == fvoci_model::BlockType::Card)
        .map(|block| {
            let value = match block.fields["properties"].get(RELATION) {
                Some(Value::Array(links)) => Value::Array(
                    links
                        .iter()
                        .map(|link| {
                            let card = link
                                .get("card")
                                .and_then(Value::as_str)
                                .and_then(|id| titles.get(id))
                                .copied()
                                .unwrap_or("?");
                            json!({
                                "card": card,
                                "kind": link.get("kind").cloned().unwrap_or(Value::Null),
                                "lagDays": link.get("lagDays").cloned().unwrap_or(Value::Null),
                            })
                        })
                        .collect(),
                ),
                other => other.cloned().unwrap_or(Value::Null),
            };
            (block.title.clone(), value)
        })
        .collect()
}

fn expected_b_links() -> Value {
    json!([
        {"card": "A", "kind": "ss", "lagDays": -3},
        {"card": "C", "kind": "ff", "lagDays": 7},
    ])
}

fn dependency_of(blocks: &[Block]) -> Option<&Value> {
    blocks
        .iter()
        .find(|block| block.block_type == fvoci_model::BlockType::View)
        .and_then(|view| view.fields.get("dependencyPropertyId"))
}

#[tokio::test]
async fn a_copy_of_a_board_names_its_own_cards() {
    let th = Setup::native().start().await.init_basic().await;
    let Named { board, .. } = named_board(&th).await;

    let copy: ContainersAndBlocks = th
        .client
        .post_empty(&format!("{TEST_WS}/containers/{}/duplicate", board.id))
        .await
        .created_json();
    let copied = &copy.containers[0];
    let values = relations_by_title(&th.client, &copied.id).await;
    assert_eq!(
        values["B"],
        expected_b_links(),
        "the copies of A and C with kind and lagDays preserved"
    );
    assert_eq!(
        values["A"],
        json!([]),
        "a card in the trash is not copied, and a card that never was is not named"
    );
    assert!(!values.contains_key("gone"));
    assert_eq!(
        dependency_of(&copy.blocks),
        Some(&json!(RELATION)),
        "the view names the same property"
    );
    th.teardown().await;
}

#[tokio::test]
async fn a_template_and_a_board_made_from_it_name_their_own_cards() {
    let th = Setup::native().start().await.init_basic().await;
    let Named { board, cards, .. } = named_board(&th).await;

    let template: ContainersAndBlocks = th
        .client
        .post_empty(&format!(
            "{TEST_WS}/containers/{}/duplicate?asTemplate=true",
            board.id
        ))
        .await
        .created_json();
    let template = &template.containers[0];
    let made: ContainersAndBlocks = th
        .client
        .post_empty(&format!("{TEST_WS}/containers/{}/duplicate", template.id))
        .await
        .created_json();
    let made = &made.containers[0];
    for board_id in [&template.id, &made.id] {
        let values = relations_by_title(&th.client, board_id).await;
        assert_eq!(values["B"], expected_b_links(), "{board_id}");
    }
    let blocks = crate::blocks::blocks_of(&th.client, &made.id).await;
    let named_ids: Vec<&str> = blocks
        .iter()
        .filter_map(|block| block.fields.get("properties")?.get(RELATION))
        .filter_map(Value::as_array)
        .flatten()
        .filter_map(|link| link.get("card").and_then(Value::as_str))
        .collect();
    assert!(
        named_ids
            .iter()
            .all(|id| !cards.values().any(|source| source.as_str() == *id)),
        "no card of the source is named: {named_ids:?}"
    );
    th.teardown().await;
}

#[tokio::test]
async fn an_imported_archive_names_the_imported_cards() {
    let th = Setup::native().start().await.init_basic().await;
    let Named { board, .. } = named_board(&th).await;

    let archive = th
        .client
        .get(&format!("{TEST_WS}/containers/{}/archive/export", board.id))
        .await;
    archive.check_ok();
    import(&th.client, TEST_WORKSPACE_ID, &archive.body)
        .await
        .check_ok();
    let boards: Vec<Container> = th.client.get(&format!("{TEST_WS}/boards")).await.ok_json();
    let imported = boards
        .iter()
        .find(|each| each.id != board.id && each.title == board.title)
        .expect("the imported board");
    let values = relations_by_title(&th.client, &imported.id).await;
    assert_eq!(values["B"], expected_b_links());
    assert_eq!(
        values["A"],
        json!([]),
        "what is not in the archive is left out"
    );
    th.teardown().await;
}

#[tokio::test]
async fn a_copy_of_a_card_names_the_cards_its_source_named() {
    let th = Setup::native().start().await.init_basic().await;
    let Named {
        board, cards, view, ..
    } = named_board(&th).await;

    for (source, expected) in [
        ("B", expected_b_links()),
        ("A", json!([{"card": "gone", "kind": "sf", "lagDays": 2}])),
    ] {
        let copy: Value = th
            .client
            .post_empty(&format!(
                "{TEST_WS}/boards/{}/cards/{}/duplicate",
                board.id, cards[source]
            ))
            .await
            .created_json();
        let resolved: Vec<Value> = copy["properties"][RELATION]
            .as_array()
            .expect("a list")
            .iter()
            .map(|link| {
                let id = link.get("card").and_then(Value::as_str).unwrap_or("");
                let title = cards
                    .iter()
                    .find(|(_, card)| card.as_str() == id)
                    .map_or("?", |(title, _)| title.as_str());
                json!({
                    "card": title,
                    "kind": link.get("kind").cloned().unwrap_or(Value::Null),
                    "lagDays": link.get("lagDays").cloned().unwrap_or(Value::Null),
                })
            })
            .collect();
        assert_eq!(
            Value::Array(resolved),
            expected,
            "{source}: the same cards with kind and lagDays preserved; what never was and the source itself are left out"
        );
    }
    let blocks = crate::blocks::blocks_of(&th.client, &board.id).await;
    assert!(blocks.iter().any(|block| block.id == view));
    th.teardown().await;
}

/// A relation is a card's: another block's value, as an older server stored it, names
/// nothing and does not stop a copy.
#[tokio::test]
async fn a_relation_value_of_a_block_that_is_no_card_does_not_stop_a_copy() {
    let th = Setup::native().start().await.init_basic().await;
    let Named { board, cards, .. } = named_board(&th).await;
    let fields = json!({"properties": {RELATION: [
        {"card": cards["A"], "kind": "fs", "lagDays": 0},
        {"card": new_id(), "kind": "ss", "lagDays": 1},
    ]}});
    let mut odd = Block {
        id: new_id(),
        container_id: board.id.clone(),
        parent_id: cards["A"].clone(),
        block_type: fvoci_model::BlockType::Attachment,
        title: "Odd".to_owned(),
        fields: fields.as_object().expect("an object").clone(),
        created_at: 1,
        updated_at: 1,
        ..Block::default()
    };
    th.app
        .insert_block_unchecked(
            &mut odd,
            &th.client.me().await.id,
            fvoci_server::app::ChangeNotification::Suppress,
        )
        .await
        .expect("stored as an older server would have");
    let copy: ContainersAndBlocks = th
        .client
        .post_empty(&format!("{TEST_WS}/containers/{}/duplicate", board.id))
        .await
        .created_json();
    let values = relations_by_title(&th.client, &copy.containers[0].id).await;
    assert_eq!(values["B"], expected_b_links());
    th.teardown().await;
}
