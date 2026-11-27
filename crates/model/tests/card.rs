//! Tests for converting between `Block` and `Card` and applying `CardPatch`.

use fvoci_model::{Block, BlockType, Card, CardError, CardPatch, JsonMap, new_id};
use serde_json::{Value, json};

const NOW: i64 = 1_660_046_400_000;

fn sample_fields() -> JsonMap {
    let fields = json!({
        "contentOrder": [
            "acdxa8r8aht85pyoeuj1ed7tu8w",
            "73urm1huoupd4idzkdq5yaeuyay",
            "ay6sogs9owtd9xbyn49qt3395ko"
        ],
        "icon": "🎨",
        "isTemplate": false,
        "properties": {
            "aa7swu9zz3ofdkcna3h867cum4y": "212-444-1234",
            "af6fcbb8-ca56-4b73-83eb-37437b9a667d": "77c539af-309c-4db1-8329-d20ef7e9eacd",
            "aiwt9ibi8jjrf9hzi1xzk8no8mo": "foo",
            "aj65h4s6ghr6wgh3bnhqbzzmiaa": "77",
            "ajy6xbebzopojaenbnmfpgtdwso": "{\"from\":1660046400000}",
            "amc8wnk1xqj54rymkoqffhtw7ie": "zhqsoeqs1pg9i8gk81k9ryy83h",
            "aooz77t119y7xtfmoyeiy4up75c": "someone@example.com",
            "auskzaoaccsn55icuwarf4o3tfe": "https://www.google.com",
            "aydsk41h6cs1z7nmghaw16jqcia": [
                "aw565znut6zphbxqhbwyawiuggy",
                "aefd3pxciomrkur4rc6smg1usoc",
                "a6c96kwrqaskbtochq9wunmzweh",
                "atyexeuq993fwwb84bxoqixxqqr"
            ],
            "d6b1249b-bc18-45fc-889e-bec48fce80ef": "9a090e33-b110-4268-8909-132c5002c90e",
            "d9725d14-d5a8-48e5-8de1-6f8c004a9680": "3245a32d-f688-463b-87f4-8e7142c1b397"
        }
    });
    match fields {
        Value::Object(map) => map,
        other => panic!("sample fields must be an object, got {other}"),
    }
}

fn sample_block() -> Block {
    let board_id = new_id();
    let user_id = new_id();
    Block {
        id: new_id(),
        // A card is directly on its board: it has no parent.
        parent_id: String::new(),
        created_by: user_id.clone(),
        modified_by: user_id,
        block_type: BlockType::Card,
        title: "My card title".to_owned(),
        fields: sample_fields(),
        body: None,
        created_at: NOW,
        updated_at: NOW,
        deleted_at: None,
        container_id: board_id,
        number: Some(7),
        edited_at: 0,
    }
}

/// `TestBlock2Card` / "Good block"
#[test]
fn card_from_block_good_block() {
    let block = sample_block();

    let card = Card::from_block(&block, NOW).expect("the block is a well-formed card");

    assert_eq!(card.id, block.id);
    assert_eq!(card.number, Some(7), "the card has the block's number");
    assert_eq!(
        Block::from(card.clone()).number,
        Some(7),
        "and its block the card's"
    );
    assert!(
        serde_json::to_value(&card).expect("a card serializes")["contentOrder"].is_null(),
        "a card's content is its body: the stored contentOrder is not read"
    );
    assert_eq!(json!(card.icon), block.fields["icon"]);
    assert_eq!(json!(card.is_template), block.fields["isTemplate"]);
    assert_eq!(Value::Object(card.properties), block.fields["properties"]);
}

/// `TestBlock2Card` / "Not a card"
#[test]
fn card_from_block_not_a_card() {
    let view = Block {
        block_type: BlockType::View,
        ..Block::default()
    };
    assert_eq!(Card::from_block(&view, NOW), Err(CardError::NotCardBlock));
}

#[test]
fn card_from_block_fills_missing_fields_and_times() {
    let block = Block {
        id: "card-id".to_owned(),
        container_id: "board-id".to_owned(),
        block_type: BlockType::Card,
        ..Block::default()
    };

    let card = Card::from_block(&block, NOW).expect("a card block with no fields is valid");

    assert_eq!(card.icon, "");
    assert!(!card.is_template, "isTemplate defaults to false");
    assert!(card.properties.is_empty(), "properties default to empty");
    assert_eq!((card.created_at, card.updated_at), (NOW, NOW));
}

/// A `contentOrder` that a card stored before has, of any shape, is not read: a card's
/// content is its body.
#[test]
fn card_from_block_does_not_read_a_stored_content_order() {
    for content_order in [json!("abc"), json!(["a", ["b", "c"]]), json!(["a"])] {
        let mut fields = fvoci_model::JsonMap::new();
        fields.insert("contentOrder".to_owned(), content_order);
        let block = Block {
            block_type: BlockType::Card,
            fields,
            ..Block::default()
        };
        assert!(Card::from_block(&block, NOW).is_ok(), "{:?}", block.fields);
    }
}

#[test]
fn card_from_block_rejects_fields_of_the_wrong_type() {
    let cases = [
        (json!({ "icon": 1 }), "icon"),
        (json!({ "isTemplate": "yes" }), "isTemplate"),
        (json!({ "properties": [] }), "properties"),
    ];
    for (fields, field) in cases {
        let Value::Object(fields) = fields else {
            panic!("case fields must be an object");
        };
        let block = Block {
            block_type: BlockType::Card,
            fields,
            ..Block::default()
        };
        assert_eq!(
            Card::from_block(&block, NOW),
            Err(CardError::InvalidFieldType { field }),
            "field {field}"
        );
    }
}

#[test]
fn card_into_block_round_trips() {
    let mut block = sample_block();
    let card = Card::from_block(&block, NOW).expect("the block is a well-formed card");

    // What is not a member of a card does not come back: a stored `contentOrder`.
    block.fields.remove("contentOrder");
    assert_eq!(Block::from(card), block);
}

#[test]
fn card_validate_reports_the_first_problem() {
    let valid = Card {
        id: "card-id".to_owned(),
        board_id: "board-id".to_owned(),
        icon: "🎨".to_owned(),
        created_at: NOW,
        updated_at: NOW,
        ..Card::default()
    };
    assert_eq!(valid.validate(), Ok(()));

    let cases = [
        (
            Card {
                id: String::new(),
                ..valid.clone()
            },
            "the card has no identifier",
        ),
        (
            Card {
                board_id: String::new(),
                ..valid.clone()
            },
            "the card is on no board",
        ),
        (
            Card {
                icon: "🎨🎨".to_owned(),
                ..valid.clone()
            },
            "an icon is at most one grapheme",
        ),
        (
            Card {
                created_at: 0,
                ..valid.clone()
            },
            "the card has no createdAt",
        ),
        (
            Card {
                updated_at: 0,
                ..valid
            },
            "the card has no updatedAt",
        ),
    ];
    for (card, message) in cases {
        assert_eq!(
            card.validate(),
            Err(CardError::Invalid(message)),
            "{message}"
        );
    }
}

#[test]
fn card_icon_may_be_one_multi_scalar_grapheme() {
    // A family emoji is seven scalar values joined into one grapheme cluster.
    let card = Card {
        id: "card-id".to_owned(),
        board_id: "board-id".to_owned(),
        icon: "👨‍👩‍👧‍👦".to_owned(),
        created_at: NOW,
        updated_at: NOW,
        ..Card::default()
    };
    assert_eq!(card.validate(), Ok(()));
}

#[test]
fn card_populate_keeps_existing_values() {
    let mut card = Card {
        id: "card-id".to_owned(),
        created_at: 5,
        ..Card::default()
    };

    card.populate(NOW);

    assert_eq!(card.id, "card-id");
    assert_eq!((card.created_at, card.updated_at), (5, NOW));

    let mut blank = Card::default();
    blank.populate(NOW);
    assert!(fvoci_model::is_id(&blank.id), "new card id {}", blank.id);
}

#[test]
fn card_patch_apply_merges_properties() {
    let mut card = Card {
        title: "old".to_owned(),
        icon: "a".to_owned(),
        ..Card::default()
    };
    card.properties.insert("kept".to_owned(), json!("1"));
    card.properties.insert("replaced".to_owned(), json!("2"));

    let mut patch = CardPatch {
        title: Some("new".to_owned()),
        icon: None,
        ..CardPatch::default()
    };
    patch
        .updated_properties
        .insert("replaced".to_owned(), json!("3"));
    patch
        .updated_properties
        .insert("added".to_owned(), json!(["4"]));
    patch.deleted_properties = vec!["gone".to_owned(), "added".to_owned()];
    card.properties.insert("gone".to_owned(), json!("5"));
    patch.apply(&mut card);

    assert_eq!(card.title, "new");
    assert_eq!(card.icon, "a");
    assert_eq!(
        Value::Object(card.properties),
        json!({ "kept": "1", "replaced": "3" }),
        "a property both set and deleted loses its value"
    );
}

#[test]
fn card_patch_to_block_patch() {
    let mut patch = CardPatch {
        title: Some("new".to_owned()),
        icon: Some("🎨".to_owned()),
        ..CardPatch::default()
    };
    patch.updated_properties.insert("p".to_owned(), json!("v"));

    let block_patch = patch.to_block_patch().expect("the patch is valid");

    assert_eq!(block_patch.title.as_deref(), Some("new"));
    assert_eq!(block_patch.parent_id, None);
    assert_eq!(
        Value::Object(block_patch.updated_fields),
        json!({ "icon": "🎨" }),
        "the values are not set as a whole field"
    );
    assert_eq!(
        Value::Object(block_patch.property_changes.updated.clone()),
        json!({ "p": "v" }),
        "they are merged"
    );
    assert!(block_patch.deleted_fields.is_empty(), "nothing is deleted");

    let empty = CardPatch::default()
        .to_block_patch()
        .expect("an empty patch is valid");
    assert!(
        empty.updated_fields.is_empty(),
        "an empty patch updates no field"
    );
}

/// Deleting a value of a block without values leaves it without them, not with an
/// empty object.
#[test]
fn deleting_from_no_values_adds_none() {
    let mut block = fvoci_model::Block::default();
    let patch = CardPatch {
        deleted_properties: vec!["x".to_owned()],
        ..CardPatch::default()
    }
    .to_block_patch()
    .expect("the patch is valid");
    patch.apply(&mut block);
    assert!(block.fields.is_empty(), "{:?}", block.fields);
}

#[test]
fn card_patch_rejects_an_icon_of_two_graphemes() {
    let patch = CardPatch {
        icon: Some("ab".to_owned()),
        ..CardPatch::default()
    };
    let want = Err(CardError::Invalid("an icon is at most one grapheme"));
    assert_eq!(patch.validate(), want);
    assert_eq!(patch.to_block_patch().map(|_| ()), want);
}

/// A card for whoever reads with a read token alone names no users: not its creator or
/// writer, not the users of its person properties, not those mentioned in its body; the
/// values of properties the board does not define go, and the others stay.
#[test]
fn a_card_hides_its_people() {
    use fvoci_model::{CardPropertyDefinition, PropertyType};
    let definitions = [
        CardPropertyDefinition::new("owner", "Owner", PropertyType::Person),
        CardPropertyDefinition::new("people", "People", PropertyType::MultiPerson),
        CardPropertyDefinition::new("status", "Status", PropertyType::Select),
    ];
    let people = fvoci_model::PersonProperties::of(&definitions);
    let mut card = Card {
        created_by: "u1".to_owned(),
        modified_by: "u2".to_owned(),
        properties: match json!({"owner": "u1", "people": ["u1", "u2"], "status": "done", "gone": "u3"})
        {
            Value::Object(map) => map,
            _ => unreachable!("an object"),
        },
        ..Card::default()
    };

    card.hide_people(&people);

    assert_eq!(
        (card.created_by.as_str(), card.modified_by.as_str()),
        ("", "")
    );
    assert_eq!(
        Value::Object(card.properties),
        json!({"owner": "", "people": ["", ""], "status": "done"})
    );
}

/// A preview's text is at most 200 UTF-16 code units, cut between characters as they are
/// seen, without white space at its end.
#[test]
fn a_preview_is_cut_between_characters() {
    use fvoci_model::CardPreview;
    let text = |text: &str| CardPreview::new(String::new(), text).text;
    assert_eq!(text(""), "");
    assert_eq!(text("short  "), "short");
    let exact = "a".repeat(200);
    assert_eq!(text(&exact), exact, "200 units are whole");
    assert_eq!(text(&format!("{exact}b")), exact, "the 201st goes");
    let letters = "가".repeat(199);
    assert_eq!(
        text(&format!("{letters}🎉")),
        letters,
        "an emoji of two units is not split"
    );
    assert_eq!(
        text(&format!("{}🇰🇷", "a".repeat(198))),
        "a".repeat(198),
        "a flag is not split"
    );
    assert_eq!(
        text(&format!("{}e\u{301}", "a".repeat(199))),
        "a".repeat(199),
        "nor a letter and its accent"
    );
    assert_eq!(
        text(&format!("{} more", "a".repeat(199))),
        "a".repeat(199),
        "no space at the end"
    );
}

/// A card has a number on its board, unless it is a template of the board's cards; no
/// other block has one.
#[test]
fn only_a_card_that_is_no_template_takes_a_number() {
    let with = |block_type: BlockType, is_template: Value| {
        let mut block = Block {
            block_type,
            ..sample_block()
        };
        block.fields.insert("isTemplate".to_owned(), is_template);
        block.takes_number()
    };
    assert!(with(BlockType::Card, json!(false)));
    assert!(with(BlockType::Card, Value::Null), "null is no template");
    assert!(!with(BlockType::Card, json!(true)), "a template");
    let mut without_field = sample_block();
    without_field.fields.remove("isTemplate");
    assert!(without_field.takes_number(), "a card without the field");
    for other in [
        BlockType::View,
        BlockType::Comment,
        BlockType::Attachment,
        BlockType::Page,
    ] {
        assert!(!with(other, json!(false)), "{other}");
    }
}
