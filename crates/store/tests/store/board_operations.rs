//! Store-level invariants for atomic board operations, `'Today'` queries, `due_soon`
//! personal-space notifications, and Gantt dependency cycle rejection.

use fvoci_model::{
    Block, BlockError, BlockPatch, BlockType, CardMovesRequest, CardPropertyDefinition,
    ContainerPatch, DoneWhen, NewBoardView, PropertyChanges, PropertyColor, PropertyOption,
    PropertyType, RelationError, RelationProblem, ViewType, new_id,
};
use fvoci_store::StoreError;
use serde_json::json;

use crate::common::{self};

#[tokio::test]
async fn board_ops_views_card_moves_done_when_and_gantt_cycle_rejection() {
    let test = common::setup().await;
    let alice = common::user(&test, "alice").await;
    let _bob = common::user(&test, "bob").await;
    let boards = common::create_test_boards(&test, &test.workspace_id, &alice, 1).await;
    let board_id = &boards[0].id;

    let status_id = new_id();
    let todo_id = new_id();
    let done_id = new_id();
    let dep_id = new_id();
    let due_id = new_id();
    let owner_id = new_id();

    test.patch_container(
        board_id,
        &ContainerPatch {
            updated_card_properties: vec![
                CardPropertyDefinition {
                    id: status_id.clone(),
                    name: "Status".to_owned(),
                    property_type: PropertyType::Select,
                    options: vec![
                        PropertyOption::new(todo_id.clone(), "Todo", PropertyColor::Gray),
                        PropertyOption::new(done_id.clone(), "Done", PropertyColor::Green),
                    ],
                },
                CardPropertyDefinition::new(dep_id.clone(), "Depends on", PropertyType::Relation),
                CardPropertyDefinition::new(due_id.clone(), "Due", PropertyType::Date),
                CardPropertyDefinition::new(owner_id.clone(), "Owner", PropertyType::Person),
            ],
            done_when: Some(Some(DoneWhen {
                property_id: status_id.clone(),
                option_ids: vec![done_id.clone()],
            })),
            ..ContainerPatch::default()
        },
        &alice,
    )
    .await
    .expect("patched board");

    let mut gantt_fields = fvoci_model::JsonMap::new();
    gantt_fields.insert(
        "dependencyPropertyId".to_owned(),
        serde_json::Value::String(dep_id.clone()),
    );
    let view_write = test
        .create_board_view(
            board_id,
            &NewBoardView {
                title: "Timeline".to_owned(),
                view_type: ViewType::Gantt,
                after: None,
                fields: Some(gantt_fields),
            },
            &alice,
        )
        .await
        .expect("created gantt view");

    let mut c1 = Block {
        id: new_id(),
        container_id: board_id.clone(),
        block_type: BlockType::Card,
        title: "Card 1".to_owned(),
        ..Block::default()
    };
    let mut c2 = Block {
        id: new_id(),
        container_id: board_id.clone(),
        block_type: BlockType::Card,
        title: "Card 2".to_owned(),
        ..Block::default()
    };
    test.insert_blocks(std::slice::from_mut(&mut c1), &alice)
        .await
        .expect("c1");
    test.insert_blocks(std::slice::from_mut(&mut c2), &alice)
        .await
        .expect("c2");

    assert_cycle(&test, &c1.id, &c2.id, &dep_id, &alice).await;
    verify_card_moves_and_batch_ops(
        &test,
        board_id,
        &view_write.view.id,
        &c1.id,
        &c2.id,
        status_id,
        done_id,
        &alice,
    )
    .await;

    test.teardown().await;
}

async fn assert_cycle(
    test: &common::TestStore,
    c1_id: &str,
    c2_id: &str,
    dep_id: &str,
    alice: &str,
) {
    let mut props_2 = fvoci_model::JsonMap::new();
    props_2.insert(dep_id.to_owned(), json!([{"card": c1_id}]));
    test.patch_block(
        c2_id,
        &BlockPatch {
            property_changes: PropertyChanges {
                updated: props_2,
                deleted: Vec::new(),
            },
            ..BlockPatch::default()
        },
        alice,
    )
    .await
    .expect("c2 -> c1 allowed");

    let mut props_1 = fvoci_model::JsonMap::new();
    props_1.insert(dep_id.to_owned(), json!([{"card": c2_id}]));
    let cycle_err = test
        .patch_block(
            c1_id,
            &BlockPatch {
                property_changes: PropertyChanges {
                    updated: props_1,
                    deleted: Vec::new(),
                },
                ..BlockPatch::default()
            },
            alice,
        )
        .await
        .expect_err("cycle rejected");
    assert!(
        matches!(
            cycle_err,
            StoreError::InvalidBlock {
                source: BlockError::InvalidRelation(RelationError {
                    problem: RelationProblem::Cycle,
                    ..
                }),
                ..
            }
        ),
        "expected RelationProblem::Cycle, got {cycle_err:?}"
    );
}

#[expect(
    clippy::too_many_arguments,
    reason = "test helper groups board, view, card, and status identifiers"
)]
async fn verify_card_moves_and_batch_ops(
    test: &common::TestStore,
    board_id: &str,
    view_id: &str,
    c1_id: &str,
    c2_id: &str,
    status_id: String,
    done_id: String,
    alice: &str,
) {
    let (moved_view, moved_cards) = test
        .move_cards_in_view(
            board_id,
            view_id,
            &CardMovesRequest {
                card_ids: vec![c2_id.to_owned()],
                group_property_id: Some(status_id),
                group_value: Some(json!(done_id)),
                after_card_id: Some(String::new()),
            },
            alice,
        )
        .await
        .expect("card-moves");
    assert_eq!(moved_cards.len(), 1, "one card updated");
    assert_eq!(
        moved_view.fields.get("cardOrder"),
        Some(&json!([c2_id, c1_id])),
        "view cardOrder updated"
    );

    let ids = [c1_id.to_owned(), c2_id.to_owned()];
    let deleted = test
        .batch_delete_cards(board_id, &ids, alice)
        .await
        .expect("batch delete");
    assert_eq!(deleted.len(), 2, "two cards deleted");
    let restored = test
        .batch_restore_cards(board_id, &ids, alice)
        .await
        .expect("batch restore");
    assert_eq!(restored.len(), 2, "two cards restored");
}
