use serde_json::json;

use super::*;
use crate::date_value::DateValueError;

fn object(value: Value) -> JsonMap {
    match value {
        Value::Object(object) => object,
        other => panic!("not an object: {other}"),
    }
}

fn def(value: Value) -> CardPropertyDefinition {
    serde_json::from_value(value).expect("valid definition")
}

#[test]
fn a_view_of_a_property_no_longer_defined_names_nobody_but_the_title_stays() {
    let people = PersonProperties::of(&[
        def(json!({"id": "stage", "type": "select", "options": [{"id": "s1", "label": "S1"}]})),
        def(json!({"id": "note", "type": "text"})),
    ]);
    let mut fields = object(json!({
        "groupBy": "owner",
        "hiddenGroups": ["u1", ""],
        "groupCalculations": {"u1": {"calculation": "count"}},
        "filter": {"operation": "and", "filters": [
            {"propertyId": "owner", "condition": "includes", "values": ["u1"]},
            {"propertyId": "title", "condition": "contains", "values": ["beans"]},
            {"propertyId": "stage", "condition": "includes", "values": ["s1"]},
        ]},
    }));
    people.hide_view_fields(&mut fields);
    assert_eq!(
        Value::Object(fields),
        json!({
            "groupBy": "owner",
            "hiddenGroups": [""],
            "groupCalculations": {},
            "filter": {"operation": "and", "filters": [
                {"propertyId": "owner", "condition": "includes", "values": [""]},
                {"propertyId": "title", "condition": "contains", "values": ["beans"]},
                {"propertyId": "stage", "condition": "includes", "values": ["s1"]},
            ]},
        }),
        "a deleted property may have named users; the title and a select do not"
    );
    let mut by_stage = object(json!({"groupBy": "stage", "hiddenGroups": ["s1", "u1"]}));
    people.hide_view_fields(&mut by_stage);
    let kept = &by_stage["hiddenGroups"];
    assert_eq!(
        kept,
        &json!(["s1"]),
        "a select's options stay, anything else goes"
    );
    // Grouped by a text, the app groups by a property of its choosing.
    let mut by_note = object(json!({"groupBy": "note", "collapsedGroups": ["u1", ""]}));
    people.hide_view_fields(&mut by_note);
    assert_eq!(
        by_note["collapsedGroups"],
        json!([""]),
        "only the empty group stays"
    );
}

#[test]
fn the_users_of_person_properties_are_emptied_one_by_one() {
    let people = PersonProperties::of(&[
        def(json!({"id": "owner", "type": "person"})),
        def(json!({"id": "people", "type": "multiPerson"})),
        def(json!({"id": "by", "type": "createdBy"})),
        def(json!({"id": "note", "type": "text"})),
        def(json!({"type": "person"})),
    ]);
    let mut values = object(json!({
        "owner": "u1", "people": ["u1", "u2"], "note": "u1", "deleted": "u1", "by": null,
    }));
    people.hide_values(&mut values);
    assert_eq!(
        Value::Object(values),
        json!({"owner": "", "people": ["", ""], "note": "u1", "by": null}),
        "the count of people stays, who they are goes, and so do undefined values"
    );

    let mut fields = object(json!({
        "groupBy": "owner",
        "hiddenGroups": ["u1", ""],
        "visibleGroups": ["u2"],
        "collapsedGroups": [""],
        "groupCalculations": {"u1": {"calculation": "count"}, "": {"calculation": "sum"}},
        "cardOrder": ["c1"],
        "filter": {"operation": "and", "filters": [
            {"propertyId": "owner", "condition": "includes", "values": ["u1"]},
            {"operation": "or", "filters": [
                {"propertyId": "by", "condition": "includes", "values": ["u1", "u2"]},
                {"propertyId": "note", "condition": "includes", "values": ["u1"]},
            ]},
        ]},
    }));
    people.hide_view_fields(&mut fields);
    assert_eq!(
        Value::Object(fields),
        json!({
            "groupBy": "owner",
            "hiddenGroups": [""],
            "visibleGroups": [],
            "collapsedGroups": [""],
            "groupCalculations": {"": {"calculation": "sum"}},
            "cardOrder": ["c1"],
            "filter": {"operation": "and", "filters": [
                {"propertyId": "owner", "condition": "includes", "values": [""]},
                {"operation": "or", "filters": [
                    {"propertyId": "by", "condition": "includes", "values": ["", ""]},
                    {"propertyId": "note", "condition": "includes", "values": ["u1"]},
                ]},
            ]},
        }),
        "in nested groups too, and the groups of users go"
    );

    let stages = PersonProperties::of(&[def(
        json!({"id": "stage", "type": "select", "options": [{"id": "o1", "label": "Open"}]}),
    )]);
    let mut by_select = object(json!({"groupBy": "stage", "hiddenGroups": ["o1"]}));
    stages.hide_view_fields(&mut by_select);
    assert_eq!(
        Value::Object(by_select),
        json!({"groupBy": "stage", "hiddenGroups": ["o1"]}),
        "groups of options stay"
    );
}

#[test]
fn strings_lists_of_strings_and_null_are_property_values() {
    let values = object(json!({
        "text": "hello", "empty": "", "many": ["a", "b"], "none": [], "unset": null,
    }));
    assert_eq!(validate_card_property_values(&values), Ok(()), "{values:?}");
    assert_eq!(
        validate_card_property_values(&JsonMap::new()),
        Ok(()),
        "no values at all"
    );
}

#[test]
fn an_object_is_a_date_value_or_refused_as_one() {
    let values = object(json!({"d": {"from": "2026-03-05"}, "t": {"from": 1}}));
    assert_eq!(validate_card_property_values(&values), Ok(()));
    let values = object(json!({"p1": {"nested": "x"}}));
    assert_eq!(
        validate_card_property_values(&values),
        Err(CardPropertyError::InvalidDateValue {
            id: "p1".to_owned(),
            reason: DateValueError::UnknownMember,
        })
    );
}

#[test]
fn date_values_are_the_values_of_date_properties_only() {
    let definitions = [
        CardPropertyDefinition::new("due", "Due", PropertyType::Date),
        CardPropertyDefinition::new("note", "Note", PropertyType::Text),
    ];
    let accepted = object(json!({
        "due": {"from": "2026-03-05"}, "note": "x", "other": ["a"],
    }));
    assert_eq!(
        validate_card_property_kinds(&accepted, &definitions),
        Ok(())
    );
    let unset = object(json!({"due": null}));
    assert_eq!(validate_card_property_kinds(&unset, &definitions), Ok(()));
    for (values, error) in [
        (
            json!({"due": "{\"from\":1}"}),
            CardPropertyError::NotADateValue {
                id: "due".to_owned(),
            },
        ),
        (
            json!({"due": ["2026-03-05"]}),
            CardPropertyError::NotADateValue {
                id: "due".to_owned(),
            },
        ),
        (
            json!({"note": {"from": "2026-03-05"}}),
            CardPropertyError::DateValueOfOtherProperty {
                id: "note".to_owned(),
            },
        ),
        (
            json!({"undefined": {"from": "2026-03-05"}}),
            CardPropertyError::DateValueOfOtherProperty {
                id: "undefined".to_owned(),
            },
        ),
    ] {
        assert_eq!(
            validate_card_property_kinds(&object(values.clone()), &definitions),
            Err(error),
            "{values}"
        );
    }
}

#[test]
fn relation_links_validate_shape_and_belong_to_relation_properties_only() {
    let valid = object(json!({
        "rel": [{"card": "c1"}, {"card": "c2", "kind": "ss", "lagDays": -3}],
    }));
    assert_eq!(validate_card_property_values(&valid), Ok(()));
    assert_eq!(
        validate_card_property_values(&object(json!({"rel": [{"card": "c1", "kind": "bad"}]}))),
        Err(CardPropertyError::InvalidRelationLink {
            id: "rel".to_owned(),
            reason: RelationLinkError::InvalidKind,
        })
    );
    assert_eq!(
        validate_card_property_values(&object(json!({"rel": ["a", {"card": "c1"}]}))),
        Err(CardPropertyError::InvalidValue {
            id: "rel".to_owned(),
        })
    );
    let definitions = [
        CardPropertyDefinition::new("rel", "Rel", PropertyType::Relation),
        CardPropertyDefinition::new("note", "Note", PropertyType::Text),
    ];
    assert_eq!(validate_card_property_kinds(&valid, &definitions), Ok(()));
    assert_eq!(
        validate_card_property_kinds(
            &object(json!({"note": [{"card": "c1", "kind": "fs", "lagDays": 0}]})),
            &definitions,
        ),
        Err(CardPropertyError::RelationValueOfOtherProperty {
            id: "note".to_owned(),
        })
    );
    let mut stale = object(json!({
        "note": [{"card": "c1", "kind": "ss", "lagDays": 2}],
        "rel": [{"card": "c2", "kind": "ff", "lagDays": -1}],
    }));
    assert_eq!(clear_values_of_another_kind(&mut stale, &definitions), 1);
    assert_eq!(stale["note"], Value::Null);
    assert_eq!(
        stale["rel"],
        json!([{"card": "c2", "kind": "ff", "lagDays": -1}])
    );
}

#[test]
fn other_shapes_are_no_property_values() {
    for value in [
        json!(3),
        json!(1.5),
        json!(true),
        json!(["a", 1]),
        json!([["a"]]),
        json!([null]),
    ] {
        let values = object(json!({"p1": value}));
        assert_eq!(
            validate_card_property_values(&values),
            Err(CardPropertyError::InvalidValue {
                id: "p1".to_owned()
            }),
            "value {value}"
        );
    }
}

#[test]
fn the_properties_of_a_block_are_an_object_of_values_or_absent() {
    assert_eq!(
        validate_block_properties(&object(json!({"icon": "x"}))),
        Ok(()),
        "no properties"
    );
    assert_eq!(
        validate_block_properties(&object(json!({"properties": {"p1": "v"}}))),
        Ok(()),
        "an object of values"
    );
    for properties in [json!("text"), json!(["a"]), json!(7), json!(null)] {
        assert_eq!(
            validate_block_properties(&object(json!({"properties": properties}))),
            Err(CardPropertyError::ValuesNotObject),
            "properties {properties}"
        );
    }
    assert_eq!(
        validate_block_properties(&object(json!({"properties": {"p1": 1}}))),
        Err(CardPropertyError::InvalidValue {
            id: "p1".to_owned()
        }),
        "an object with a malformed value"
    );
}

#[test]
fn well_formed_definitions_pass() {
    let definitions = [
        def(json!({"id": "p1"})),
        def(json!({"id": "p2", "name": "Status", "type": "select", "options": []})),
        def(json!({
            "id": "p3", "name": "", "type": "multiSelect",
            "options": [
                {"id": "o1", "label": "Open", "color": "red"},
                {"id": "o2", "label": ""},
            ],
        })),
    ];
    assert_eq!(
        validate_card_property_definitions(&definitions),
        Ok(()),
        "{definitions:?}"
    );
    assert_eq!(validate_card_property_definitions(&[]), Ok(()), "none");
}

#[test]
fn property_color_and_type_words_match_all_in_order() {
    assert_eq!(
        PropertyColor::WORDS,
        PropertyColor::ALL.map(PropertyColor::as_str),
        "PropertyColor::WORDS must match PropertyColor::ALL in order"
    );
    assert_eq!(
        PropertyType::WORDS,
        PropertyType::ALL.map(PropertyType::as_str),
        "PropertyType::WORDS must match PropertyType::ALL in order"
    );
}

#[test]
fn property_colors_and_types_reject_legacy_and_unknown_wire_strings() {
    for color in PropertyColor::ALL {
        assert_eq!(color.as_str().parse::<PropertyColor>(), Ok(color));
        assert_eq!(serde_json::to_value(color).expect("json"), color.as_str());
    }
    for legacy in [
        concat!("prop", "ColorDefault"),
        concat!("prop", "ColorRed"),
        "Red",
        "",
    ] {
        assert!(legacy.parse::<PropertyColor>().is_err(), "{legacy:?}");
        assert!(
            serde_json::from_value::<PropertyColor>(json!(legacy)).is_err(),
            "{legacy:?}"
        );
    }
    for kind in PropertyType::ALL {
        assert_eq!(kind.as_str().parse::<PropertyType>(), Ok(kind));
        assert_eq!(serde_json::to_value(kind).expect("json"), kind.as_str());
    }
    for bad_kind in ["unknown", "Text", "multiselect", ""] {
        assert!(bad_kind.parse::<PropertyType>().is_err(), "{bad_kind:?}");
        assert!(
            serde_json::from_value::<PropertyType>(json!(bad_kind)).is_err(),
            "{bad_kind:?}"
        );
    }
    for bad_option in [
        json!({"id": "o1", "value": "Open", "color": "red"}),
        json!({"id": "o1", "label": "Open", "value": "Legacy", "color": "red"}),
        json!({"id": "o1", "label": "Open", "extra": true}),
    ] {
        assert!(
            serde_json::from_value::<PropertyOption>(bad_option.clone()).is_err(),
            "unknown or legacy option fields must be rejected: {bad_option}"
        );
    }
    for bad_def in [
        json!({"id": "p1", "propertyType": "date"}),
        json!({"id": "p1", "value": "Due"}),
        json!({"id": "p1", "extra": 1}),
    ] {
        assert!(
            serde_json::from_value::<CardPropertyDefinition>(bad_def.clone()).is_err(),
            "unknown definition fields must be rejected: {bad_def}"
        );
    }
}

#[test]
fn missing_or_duplicate_definition_ids_are_refused() {
    assert_eq!(
        validate_card_property_definitions(&[
            def(json!({"id": "p1"})),
            def(json!({"name": "no id"})),
        ]),
        Err(CardPropertyError::MissingId { position: 2 }),
        "the second definition, named by its place"
    );
    assert_eq!(
        validate_card_property_definitions(&[
            def(json!({"id": "p1", "name": "first"})),
            def(json!({"id": "p2"})),
            def(json!({"id": "p1", "name": "again"})),
        ]),
        Err(CardPropertyError::DuplicateId {
            id: "p1".to_owned()
        }),
        "a value could not say which of the two it belongs to"
    );
    assert_eq!(
        validate_card_property_definitions(&[def(json!({
            "id": "p1",
            "type": "select",
            "options": [{"id": "o1", "label": "One"}, {"id": "", "label": "Two"}],
        }))]),
        Err(CardPropertyError::MissingOptionId {
            id: "p1".to_owned(),
            position: 2,
        }),
        "empty option id is refused with its 1-based position"
    );
    assert_eq!(
        validate_card_property_definitions(&[def(json!({
            "id": "p1",
            "type": "select",
            "options": [{"id": "o1", "label": "One"}, {"id": "o1", "label": "Again"}],
        }))]),
        Err(CardPropertyError::DuplicateOptionId {
            id: "p1".to_owned(),
            option_id: "o1".to_owned(),
        }),
        "duplicate option id within one property is refused"
    );
}
