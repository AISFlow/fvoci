//! Tests for block ID regeneration and modification-metadata stamping.

use fvoci_model::{Block, JsonMap, generate_block_ids, new_id, stamp_modification_metadata};
use serde_json::{Value, json};

fn block(id: &str, board_id: &str, parent_id: &str) -> Block {
    Block {
        id: id.to_owned(),
        container_id: board_id.to_owned(),
        parent_id: parent_id.to_owned(),
        ..Block::default()
    }
}

fn fields(value: Value) -> JsonMap {
    match value {
        Value::Object(map) => map,
        other => panic!("test fields must be a JSON object, got {other}"),
    }
}

fn id() -> String {
    new_id()
}

/// "Should generate a new ID for a single block with no references"
#[test]
fn generate_block_ids_single_block_with_no_references() {
    let block_id = id();
    let mut blocks = vec![block(&block_id, "", "")];

    generate_block_ids(&mut blocks);

    assert_ne!(blocks[0].id, block_id);
    assert_eq!(blocks[0].container_id, "");
    assert_eq!(blocks[0].parent_id, "");
}

/// "Should generate a new ID for a single block with references"
#[test]
fn generate_block_ids_single_block_with_references() {
    let (block_id, board_id, parent_id) = (id(), id(), id());
    let mut blocks = vec![block(&block_id, &board_id, &parent_id)];

    generate_block_ids(&mut blocks);

    assert_ne!(blocks[0].id, block_id);
    assert_eq!(blocks[0].container_id, board_id);
    assert_eq!(blocks[0].parent_id, parent_id);
}

/// "Should generate IDs and link multiple blocks with existing references"
#[test]
fn generate_block_ids_links_multiple_blocks_with_existing_references() {
    let (block_id1, board_id1, parent_id1) = (id(), id(), id());
    let (block_id2, parent_id2) = (id(), id());
    let board_id2 = block_id1.clone();
    let mut blocks = vec![
        block(&block_id1, &board_id1, &parent_id1),
        block(&block_id2, &board_id2, &parent_id2),
    ];

    generate_block_ids(&mut blocks);

    assert_ne!(blocks[0].id, block_id1);
    assert_eq!(blocks[0].container_id, board_id1);
    assert_eq!(blocks[0].parent_id, parent_id1);

    assert_ne!(blocks[1].id, block_id2);
    assert_ne!(blocks[1].container_id, board_id2);
    assert_eq!(blocks[1].parent_id, parent_id2);

    // The first block was referenced, so the reference follows its new identifier.
    assert_eq!(blocks[1].container_id, blocks[0].id);
}

/// "Should generate new IDs but not modify nonexisting references"
#[test]
fn generate_block_ids_does_not_modify_nonexisting_references() {
    let (block_id1, parent_id1) = (id(), id());
    let (block_id2, board_id2) = (id(), id());
    let mut blocks = vec![
        block(&block_id1, "", &parent_id1),
        block(&block_id2, &board_id2, ""),
    ];

    generate_block_ids(&mut blocks);

    // Only the identifiers changed.
    assert_ne!(blocks[0].id, block_id1);
    assert_eq!(blocks[0].container_id, "");
    assert_eq!(blocks[0].parent_id, parent_id1);

    assert_ne!(blocks[1].id, block_id2);
    assert_eq!(blocks[1].container_id, board_id2);
    assert_eq!(blocks[1].parent_id, "");
}

/// "Should modify correctly multiple blocks with existing and nonexisting references"
#[test]
fn generate_block_ids_multiple_blocks_with_existing_and_nonexisting_references() {
    let (block_id1, board_id1, parent_id1) = (id(), id(), id());
    let first = block(&block_id1, &board_id1, &parent_id1);

    // Linked to 1.
    let (block_id2, parent_id2) = (id(), id());
    let board_id2 = block_id1.clone();
    let second = block(&block_id2, &board_id2, &parent_id2);

    // Linked to 2.
    let (block_id3, parent_id3) = (id(), id());
    let board_id3 = block_id2.clone();
    let third = block(&block_id3, &board_id3, &parent_id3);

    // Linked to 1.
    let (block_id4, parent_id4) = (id(), id());
    let board_id4 = block_id1.clone();
    let fourth = block(&block_id4, &board_id4, &parent_id4);

    // The blocks are shuffled.
    let mut blocks = vec![fourth, second, first, third];

    generate_block_ids(&mut blocks);

    // Block 1.
    assert_ne!(blocks[2].id, block_id1);
    assert_eq!(blocks[2].container_id, board_id1);
    assert_eq!(blocks[2].parent_id, parent_id1);

    // Block 2.
    assert_ne!(blocks[1].id, block_id2);
    assert_ne!(blocks[1].container_id, board_id2);
    assert_eq!(
        blocks[1].container_id, blocks[2].id,
        "block 2 links to block 1"
    );
    assert_eq!(blocks[1].parent_id, parent_id2);

    // Block 3.
    assert_ne!(blocks[3].id, block_id3);
    assert_ne!(blocks[3].container_id, board_id3);
    assert_eq!(
        blocks[3].container_id, blocks[1].id,
        "block 3 links to block 2"
    );
    assert_eq!(blocks[3].parent_id, parent_id3);

    // Block 4.
    assert_ne!(blocks[0].id, block_id4);
    assert_ne!(blocks[0].container_id, board_id4);
    assert_eq!(
        blocks[0].container_id, blocks[2].id,
        "block 4 links to block 1"
    );
    assert_eq!(blocks[0].parent_id, parent_id4);
}

/// "Should update content order"
#[test]
fn generate_block_ids_updates_content_order() {
    let (block_id1, board_id1, parent_id1) = (id(), id(), id());
    let (block_id2, board_id2, parent_id2) = (id(), id(), id());
    let mut blocks = vec![
        block(&block_id1, &board_id1, &parent_id1),
        Block {
            fields: fields(json!({ "contentOrder": [block_id1] })),
            ..block(&block_id2, &board_id2, &parent_id2)
        },
    ];

    generate_block_ids(&mut blocks);

    assert_ne!(blocks[0].id, block_id1);
    assert_eq!(blocks[0].container_id, board_id1);
    assert_eq!(blocks[0].parent_id, parent_id1);

    assert_ne!(blocks[1].id, block_id2);
    assert_eq!(blocks[1].container_id, board_id2);
    assert_eq!(blocks[1].parent_id, parent_id2);

    // Block 1 was referenced in block 2's content order, which follows the new identifier.
    assert_eq!(blocks[1].fields["contentOrder"], json!([blocks[0].id]));
}

/// "Should update content order when it contain slices"
#[test]
fn generate_block_ids_updates_content_order_containing_lists() {
    let (block_id1, board_id1, parent_id1) = (id(), id(), id());
    let (block_id2, block_id3) = (id(), id());
    let (block_id4, board_id2, parent_id2) = (id(), id(), id());
    let mut blocks = vec![
        block(&block_id1, &board_id1, &parent_id1),
        block(&block_id2, &board_id1, &parent_id1),
        block(&block_id3, &board_id1, &parent_id1),
        Block {
            fields: fields(json!({ "contentOrder": [block_id1, [block_id2, block_id3]] })),
            ..block(&block_id4, &board_id2, &parent_id2)
        },
    ];

    generate_block_ids(&mut blocks);

    assert_ne!(blocks[0].id, block_id1);
    assert_eq!(blocks[0].container_id, board_id1);
    assert_eq!(blocks[0].parent_id, parent_id1);

    assert_ne!(blocks[3].id, block_id4);
    assert_eq!(blocks[3].container_id, board_id2);
    assert_eq!(blocks[3].parent_id, parent_id2);

    assert_ne!(blocks[1].id, block_id2);
    assert_ne!(blocks[2].id, block_id3);
    assert_eq!(
        blocks[3].fields["contentOrder"],
        json!([blocks[0].id, [blocks[1].id, blocks[2].id]])
    );
}

/// "Should update Id of default template view"
#[test]
fn generate_block_ids_updates_default_template_id() {
    let (block_id1, board_id1, parent_id1) = (id(), id(), id());
    let (block_id2, board_id2, parent_id2) = (id(), id(), id());
    let mut blocks = vec![
        block(&block_id1, &board_id1, &parent_id1),
        Block {
            fields: fields(json!({ "defaultTemplateId": block_id1 })),
            ..block(&block_id2, &board_id2, &parent_id2)
        },
    ];

    generate_block_ids(&mut blocks);

    assert_ne!(blocks[0].id, block_id1);
    assert_eq!(blocks[0].container_id, board_id1);
    assert_eq!(blocks[0].parent_id, parent_id1);

    assert_ne!(blocks[1].id, block_id2);
    assert_eq!(blocks[1].container_id, board_id2);
    assert_eq!(blocks[1].parent_id, parent_id2);

    assert_eq!(blocks[1].fields["defaultTemplateId"], json!(blocks[0].id));
}

/// `cardOrder` is rewritten like `contentOrder`.
#[test]
fn generate_block_ids_updates_card_order() {
    let (card_id, view_id, board_id) = (id(), id(), id());
    let mut blocks = vec![
        block(&card_id, &board_id, &board_id),
        Block {
            fields: fields(json!({ "cardOrder": [card_id, "not-in-this-set"] })),
            ..block(&view_id, &board_id, &card_id)
        },
    ];

    generate_block_ids(&mut blocks);

    assert_eq!(
        blocks[1].fields["cardOrder"],
        json!([blocks[0].id, "not-in-this-set"])
    );
}

/// A card that only a view's `cardOrder` names, as a card whose content is its body is,
/// keeps its place in the order. The view is on the board, as views are, so nothing else
/// refers to the card.
#[test]
fn generate_block_ids_follows_a_card_only_the_card_order_names() {
    let (card_id, view_id, board_id) = (id(), id(), id());
    let mut blocks = vec![
        block(&card_id, &board_id, &board_id),
        Block {
            fields: fields(json!({ "cardOrder": [card_id] })),
            ..block(&view_id, &board_id, &board_id)
        },
    ];

    generate_block_ids(&mut blocks);

    assert_ne!(blocks[0].id, card_id, "the card has a new identifier");
    assert_eq!(
        blocks[1].fields["cardOrder"],
        json!([blocks[0].id]),
        "and the view's order names it"
    );
}

/// Fields of an unexpected type are left unchanged during ID rewriting.
#[test]
fn generate_block_ids_leaves_fields_of_unexpected_type() {
    let weird = json!({ "contentOrder": "not a list", "cardOrder": 7, "defaultTemplateId": [] });
    let mut blocks = vec![Block {
        fields: fields(weird.clone()),
        ..block(&id(), "", "")
    }];

    generate_block_ids(&mut blocks);

    assert_eq!(Value::Object(blocks[0].fields.clone()), weird);
}

/// Stamping who changed a block and when: base case.
#[test]
fn stamp_modification_metadata_base_case() {
    let mut blocks = vec![Block::default()];
    assert_eq!(blocks[0].modified_by, "");
    assert_eq!(blocks[0].updated_at, 0);

    stamp_modification_metadata("user_id_1", &mut blocks, 1_700_000_000_000);

    assert_eq!(blocks[0].modified_by, "user_id_1");
    assert_eq!(blocks[0].updated_at, 1_700_000_000_000);
}

/// The new identifiers come in the order of the blocks, whichever of them something
/// refers to: a list of blocks is by creation time and then by identifier, so blocks
/// that are stored in the same millisecond come back in the order they were sent in.
#[test]
fn generate_block_ids_gives_identifiers_in_the_order_of_the_blocks() {
    for _ in 0..20 {
        let mut blocks: Vec<Block> = (0..40)
            .map(|index| Block {
                id: format!("block-{index}"),
                // Every third block is the parent of the one before it, and so is
                // referred to; the others are not.
                parent_id: if index % 3 == 1 {
                    format!("block-{}", index + 1)
                } else {
                    String::new()
                },
                ..Block::default()
            })
            .collect();
        generate_block_ids(&mut blocks);
        let ids: Vec<&str> = blocks.iter().map(|block| block.id.as_str()).collect();
        assert!(
            ids.windows(2).all(|pair| pair[0] < pair[1]),
            "the identifiers are not in the order of the blocks: {ids:?}"
        );
        assert_eq!(blocks[1].parent_id, blocks[2].id, "references follow");
    }
}

/// A block may come without an identifier: it gets one of its own, and a block without
/// a parent does not become the child of it.
#[test]
fn generate_block_ids_takes_no_identifier_as_no_reference() {
    let mut blocks = vec![
        block("", "board", ""),
        block("", "board", ""),
        block("named", "board", ""),
    ];
    generate_block_ids(&mut blocks);
    assert!(blocks.iter().all(|block| block.parent_id.is_empty()));
    assert!(blocks.iter().all(|block| block.container_id == "board"));
    assert_ne!(blocks[0].id, blocks[1].id);
    assert!(
        blocks
            .iter()
            .all(|block| !block.id.is_empty() && block.id != "named")
    );
}

fn card(id: &str, board_id: &str, related: &Value) -> Block {
    Block {
        block_type: fvoci_model::BlockType::Card,
        fields: fields(json!({"properties": {"rel": related.clone(), "text": ["a", "b"]}})),
        ..block(id, board_id, board_id)
    }
}

fn related(block: &Block) -> &Value {
    &block.fields["properties"]["rel"]
}

/// A card relation names the new identifiers of the cards renewed with it; what else it
/// names a request keeps and a copy drops. Another property's values are no references.
#[test]
fn renew_block_ids_renames_what_card_relations_name() {
    use fvoci_model::{
        CardPropertyDefinition, PropertyType, RelationProperties, UnmatchedRelation,
        renew_block_ids,
    };

    let relations = RelationProperties::of_board(
        "board",
        &[CardPropertyDefinition::new(
            "rel",
            "Rel",
            PropertyType::Relation,
        )],
    );
    let view = Block {
        block_type: fvoci_model::BlockType::View,
        ..block("view", "board", "board")
    };
    let given = vec![
        card(
            "a",
            "board",
            &json!([
                {"card": "b", "kind": "ss", "lagDays": -3},
                {"card": "outside", "kind": "ff", "lagDays": 4},
                {"card": "view", "kind": "sf", "lagDays": 0},
                3,
            ]),
        ),
        card(
            "b",
            "board",
            &json!([{"card": "a", "kind": "fs", "lagDays": 2}]),
        ),
        view,
        card(
            "elsewhere",
            "other",
            &json!([{"card": "a", "kind": "fs", "lagDays": 0}]),
        ),
    ];

    let mut kept = given.clone();
    renew_block_ids(&mut kept, &relations, UnmatchedRelation::Kept);
    assert_eq!(
        related(&kept[0]),
        &json!([
            {"card": kept[1].id, "kind": "ss", "lagDays": -3},
            {"card": "outside", "kind": "ff", "lagDays": 4},
            {"card": kept[2].id, "kind": "sf", "lagDays": 0},
            3,
        ]),
        "a request keeps what it names outside, and renames any block of its own while keeping kind and lagDays"
    );
    assert_eq!(
        related(&kept[1]),
        &json!([{"card": kept[0].id, "kind": "fs", "lagDays": 2}])
    );
    assert_eq!(
        related(&kept[3]),
        &json!([{"card": "a", "kind": "fs", "lagDays": 0}]),
        "a board without relations has its values as they are"
    );
    assert_eq!(kept[0].fields["properties"]["text"], json!(["a", "b"]));

    let mut copied = given;
    renew_block_ids(&mut copied, &relations, UnmatchedRelation::Dropped);
    assert_eq!(
        related(&copied[0]),
        &json!([{"card": copied[1].id, "kind": "ss", "lagDays": -3}, 3]),
        "a copy names the copies of the cards while preserving kind and lagDays"
    );
    assert_eq!(
        related(&copied[1]),
        &json!([{"card": copied[0].id, "kind": "fs", "lagDays": 2}])
    );
}

/// A copy of several boards names, in a card's relation, the copies of the cards of the
/// card's own board only, and at most the limit of what is left.
#[test]
fn renew_block_ids_keeps_a_copied_relation_on_its_board() {
    use fvoci_model::{
        CardPropertyDefinition, PropertyType, RELATION_LIMIT, RelationProperties,
        UnmatchedRelation, renew_block_ids,
    };

    let definitions = [CardPropertyDefinition::new(
        "rel",
        "Rel",
        PropertyType::Relation,
    )];
    let mut relations = RelationProperties::of_board("one", &definitions);
    relations.add_board("two", &definitions);
    let mut named: Vec<Value> = (0..RELATION_LIMIT)
        .map(|_| json!({"card": id(), "kind": "fs", "lagDays": 0}))
        .collect();
    named.insert(0, json!({"card": "theirs", "kind": "ss", "lagDays": 1}));
    named.push(json!({"card": "mine", "kind": "ff", "lagDays": -2}));
    let mut blocks = vec![
        card("a", "one", &Value::Array(named)),
        card("mine", "one", &json!([])),
        card(
            "theirs",
            "two",
            &json!([{"card": "a", "kind": "sf", "lagDays": 3}]),
        ),
    ];
    renew_block_ids(&mut blocks, &relations, UnmatchedRelation::Dropped);
    assert_eq!(
        related(&blocks[0]),
        &json!([{"card": blocks[1].id, "kind": "ff", "lagDays": -2}]),
        "another board's card goes, and what is left is within the limit with kind and lagDays kept"
    );
    assert_eq!(related(&blocks[2]), &json!([]));
}

/// A body that names a block renewed with it names that block by its new identifier,
/// whichever way unmatched relations go, and what else it names it names as before: a
/// copy points at the copies, and at what was not copied.
#[test]
fn renew_block_ids_renames_what_bodies_name_among_the_blocks() {
    use fvoci_model::{Body, RelationProperties, UnmatchedRelation, renew_block_ids};

    let naming = |targets: &[&str]| {
        let line: Vec<Value> = targets
            .iter()
            .map(|target| json!({"type": "reference", "attrs": {"id": target}}))
            .collect();
        Body::parse(json!({"type": "doc", "content": [{"type": "paragraph", "content": line}]}))
            .expect("a body")
    };
    for unmatched in [UnmatchedRelation::Kept, UnmatchedRelation::Dropped] {
        let (first, second, comment, outside) = (id(), id(), id(), id());
        let mut blocks = vec![
            Block {
                body: naming(&[&second, &outside, &first]),
                ..block(&first, "board", "")
            },
            block(&second, "board", ""),
            Block {
                body: naming(&[&first]),
                ..block(&comment, "board", &second)
            },
        ];
        renew_block_ids(&mut blocks, &RelationProperties::new(), unmatched);
        let named = |block: &Block| -> Vec<String> {
            let body = block.body.as_ref().expect("the body stays");
            body.reference_ids()
                .into_iter()
                .map(str::to_owned)
                .collect()
        };
        assert!(
            blocks
                .iter()
                .all(|block| block.id != first && block.id != second)
        );
        assert_eq!(
            named(&blocks[0]),
            [blocks[1].id.clone(), outside, blocks[0].id.clone()],
            "{unmatched:?}"
        );
        assert_eq!(named(&blocks[2]), [blocks[0].id.clone()], "{unmatched:?}");
        assert_eq!(blocks[2].parent_id, blocks[1].id);
    }
}

/// A board view of a body shows the copy of its view when the view is copied with it,
/// and the view it showed when not.
#[test]
fn renew_block_ids_renames_the_views_bodies_show_among_the_blocks() {
    use fvoci_model::{Body, RelationProperties, UnmatchedRelation, renew_block_ids};

    let (card, view, outside) = (id(), id(), id());
    let shown = |view: &str| json!({"type": "boardView", "attrs": {"viewId": view}});
    let mut blocks = vec![
        Block {
            body: Body::parse(json!({"type": "doc", "content": [shown(&view), shown(&outside)]}))
                .expect("a body"),
            ..block(&card, "board", "")
        },
        Block {
            block_type: fvoci_model::BlockType::View,
            ..block(&view, "board", "")
        },
    ];
    renew_block_ids(
        &mut blocks,
        &RelationProperties::new(),
        UnmatchedRelation::Kept,
    );
    assert_ne!(blocks[1].id, view);
    let body = blocks[0].body.as_ref().expect("the body stays");
    assert_eq!(
        body.board_view_ids(),
        [blocks[1].id.as_str(), outside.as_str()]
    );
}
