//! Card relations: a card property of the `type` `relation` whose value names other
//! cards of the same board, with a dependency kind and lag in calendar days.
//!
//! A value is `null` or a list of [`RelationLink`] objects
//! (`{"card": "<cardId>", "kind": "fs" | "ss" | "ff" | "sf", "lagDays": <i32>}`),
//! each naming a distinct other card, never the card itself, at most
//! [`RELATION_LIMIT`] of them. When omitted in a request, `kind` defaults to `"fs"` and
//! `lagDays` defaults to `0`; stored values always include all three fields normalized.
//! What this module checks needs nothing but the value; whether each identifier is a
//! card of the board is the store's to ask
//! ([`RelationProblem::NotACard`], [`RelationProblem::CardNotFound`]).

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::hash::BuildHasher;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::JsonMap;
use crate::card_property::{CardPropertyDefinition, PropertyType};

/// The most cards one relation value names.
pub const RELATION_LIMIT: usize = 50;

/// Minimum allowed lag in whole calendar days (`-10` years).
pub const RELATION_LAG_MIN: i32 = -3650;

/// Maximum allowed lag in whole calendar days (`+10` years).
pub const RELATION_LAG_MAX: i32 = 3650;

/// Dependency kind between a predecessor card and a successor card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RelationKind {
    /// Finish-to-start (default): successor starts after predecessor finishes.
    #[default]
    Fs,
    /// Start-to-start: successor starts after predecessor starts.
    Ss,
    /// Finish-to-finish: successor finishes after predecessor finishes.
    Ff,
    /// Start-to-finish: successor finishes after predecessor starts.
    Sf,
}

impl RelationKind {
    /// Wire string representation of this relation kind.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fs => "fs",
            Self::Ss => "ss",
            Self::Ff => "ff",
            Self::Sf => "sf",
        }
    }
}

impl fmt::Display for RelationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error returned when a string names no known [`RelationKind`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("relation kind must be one of \"fs\", \"ss\", \"ff\" or \"sf\"")]
pub struct ParseRelationKindError(());

impl FromStr for RelationKind {
    type Err = ParseRelationKindError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "fs" => Ok(Self::Fs),
            "ss" => Ok(Self::Ss),
            "ff" => Ok(Self::Ff),
            "sf" => Ok(Self::Sf),
            _ => Err(ParseRelationKindError(())),
        }
    }
}

/// One entry in a card relation property value.
///
/// Requests may omit `kind` (defaults to [`RelationKind::Fs`]) or `lagDays` (defaults
/// to `0`). Stored values are always normalized with all three fields present. Construct
/// validated instances from JSON via [`parse_relation_link`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelationLink {
    /// Identifier of the related card on the same board.
    pub card: String,
    /// Dependency kind (`fs`, `ss`, `ff`, `sf`).
    pub kind: RelationKind,
    /// Lag in whole calendar days (`RELATION_LAG_MIN..=RELATION_LAG_MAX`).
    pub lag_days: i32,
}

impl RelationLink {
    /// Creates a finish-to-start link with zero lag days.
    #[must_use]
    pub fn new(card: impl Into<String>) -> Self {
        Self {
            card: card.into(),
            kind: RelationKind::Fs,
            lag_days: 0,
        }
    }

    /// Serializes the link to its canonical 3-field JSON object (`card`, `kind`, `lagDays`).
    #[must_use]
    pub fn to_value(&self) -> Value {
        let mut map = JsonMap::with_capacity(3);
        map.insert("card".to_owned(), Value::String(self.card.clone()));
        map.insert(
            "kind".to_owned(),
            Value::String(self.kind.as_str().to_owned()),
        );
        map.insert("lagDays".to_owned(), Value::from(self.lag_days));
        Value::Object(map)
    }
}

/// Why a single JSON element is not a valid [`RelationLink`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RelationLinkError {
    /// The item is not a JSON object.
    #[error("a relation link must be an object with card, and optional kind and lagDays")]
    NotAnObject,
    /// The object contains an unrecognized field.
    #[error("unknown field {field:?} in a relation link")]
    UnknownField {
        /// The unexpected field name.
        field: String,
    },
    /// The `card` field is missing, not a string, or empty.
    #[error("card must be a non-empty string")]
    MissingOrInvalidCard,
    /// The `kind` field is not one of `"fs"`, `"ss"`, `"ff"`, or `"sf"`.
    #[error("kind must be one of \"fs\", \"ss\", \"ff\" or \"sf\"")]
    InvalidKind,
    /// The `lagDays` field is not a whole integer in `RELATION_LAG_MIN..=RELATION_LAG_MAX`.
    #[error("lagDays must be an integer between {RELATION_LAG_MIN} and {RELATION_LAG_MAX}")]
    InvalidLagDays,
}

/// Validates and normalizes a single JSON value as a [`RelationLink`].
///
/// Rejects non-objects (including legacy string card IDs), unknown fields, empty `card`,
/// unknown or non-string `kind`, and non-integer or out-of-range `lagDays`.
///
/// # Errors
///
/// Returns the [`RelationLinkError`] describing the first violation.
pub fn parse_relation_link(value: &Value) -> Result<RelationLink, RelationLinkError> {
    let Some(map) = value.as_object() else {
        return Err(RelationLinkError::NotAnObject);
    };
    for key in map.keys() {
        if !matches!(key.as_str(), "card" | "kind" | "lagDays") {
            return Err(RelationLinkError::UnknownField { field: key.clone() });
        }
    }
    let Some(card) = map.get("card").and_then(Value::as_str) else {
        return Err(RelationLinkError::MissingOrInvalidCard);
    };
    if card.is_empty() {
        return Err(RelationLinkError::MissingOrInvalidCard);
    }
    let kind = match map.get("kind") {
        None => RelationKind::Fs,
        Some(Value::String(raw)) => raw
            .parse::<RelationKind>()
            .map_err(|_| RelationLinkError::InvalidKind)?,
        Some(_) => return Err(RelationLinkError::InvalidKind),
    };
    let lag_days = match map.get("lagDays") {
        None => 0,
        Some(Value::Number(number)) => {
            let Some(lag) = number.as_i64().and_then(|raw| i32::try_from(raw).ok()) else {
                return Err(RelationLinkError::InvalidLagDays);
            };
            if !(RELATION_LAG_MIN..=RELATION_LAG_MAX).contains(&lag) {
                return Err(RelationLinkError::InvalidLagDays);
            }
            lag
        }
        Some(_) => return Err(RelationLinkError::InvalidLagDays),
    };
    Ok(RelationLink {
        card: card.to_owned(),
        kind,
        lag_days,
    })
}

/// Extracts the `card` identifier string from a stored relation link JSON object, if present.
#[must_use]
pub fn relation_link_card(item: &Value) -> Option<&str> {
    item.as_object()?.get("card")?.as_str()
}

/// Converts a slice of [`RelationLink`] values into a normalized JSON array.
#[must_use]
pub fn relation_links_to_value(links: &[RelationLink]) -> Value {
    Value::Array(links.iter().map(RelationLink::to_value).collect())
}

/// What is wrong with a relation value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationProblem {
    /// The value is neither `null` nor a list of valid relation link objects.
    NotAList,
    /// The value names the card it is the value of.
    ToItself,
    /// The value names a card twice (even with different kinds or lag).
    Repeated,
    /// The value names more than [`RELATION_LIMIT`] cards, and one the value it replaces
    /// did not name.
    TooMany,
    /// The value names a block of the board that is no card.
    NotACard,
    /// The value names no card of the board, in the trash or out of it: there is none
    /// of that identifier, or it is another board's. The two are not told apart.
    CardNotFound,
    /// Adding or updating the relation on a Gantt dependency property would close a
    /// directed cycle (`A -> ... -> A`) among live cards on the board.
    Cycle,
}

/// A relation value that is refused, with the property it is the value of.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{}", message(.property, *.problem))]
pub struct RelationError {
    /// The identifier of the property.
    pub property: String,
    /// What is wrong with the value.
    pub problem: RelationProblem,
}

fn message(property: &str, problem: RelationProblem) -> String {
    match problem {
        RelationProblem::NotAList => format!(
            "the value of card relation {property:?} must be a list of relation links or null"
        ),
        RelationProblem::ToItself => format!("card relation {property:?} names the card itself"),
        RelationProblem::Repeated => format!("card relation {property:?} names a card twice"),
        RelationProblem::TooMany => {
            format!("card relation {property:?} names more than {RELATION_LIMIT} cards")
        }
        RelationProblem::NotACard => {
            format!("card relation {property:?} names a block of the board that is no card")
        }
        RelationProblem::CardNotFound => {
            format!("card relation {property:?} names a card that is not on the board")
        }
        RelationProblem::Cycle => {
            format!("card relation {property:?} would create a dependency cycle")
        }
    }
}

impl RelationError {
    /// The refusal of the value of `property`.
    #[must_use]
    pub fn new(property: &str, problem: RelationProblem) -> Self {
        Self {
            property: property.to_owned(),
            problem,
        }
    }
}

/// The identifiers of the relation properties among a board's definitions.
#[must_use]
pub fn relation_property_ids(definitions: &[CardPropertyDefinition]) -> HashSet<&str> {
    definitions
        .iter()
        .filter(|definition| {
            !definition.id.is_empty() && definition.property_type == PropertyType::Relation
        })
        .map(|definition| definition.id.as_str())
        .collect()
}

/// Checks a relation value of the card `card_id` that a write gives it, and returns the
/// normalized links it names (`None` for `null`). `had` is what card identifiers the
/// value it replaces named: a value over [`RELATION_LIMIT`], stored before the rules,
/// may be written again with any of its cards left out or in another order, and is
/// refused once it names one it did not.
///
/// # Errors
///
/// Returns the [`RelationError`] of the first rule the value breaks: not `null` or a
/// list of valid relation link objects, more than [`RELATION_LIMIT`] links with one
/// `had` lacks, the card itself, or a card named twice (regardless of kind).
pub fn check_relation_value<S: BuildHasher>(
    card_id: &str,
    property: &str,
    value: &Value,
    had: &HashSet<&str, S>,
) -> Result<Option<Vec<RelationLink>>, RelationError> {
    let refuse = |problem| Err(RelationError::new(property, problem));
    let items = match value {
        Value::Null => return Ok(None),
        Value::Array(items) => items,
        Value::Bool(_) | Value::Number(_) | Value::String(_) | Value::Object(_) => {
            return refuse(RelationProblem::NotAList);
        }
    };
    let mut links = Vec::with_capacity(items.len());
    for item in items {
        let Ok(link) = parse_relation_link(item) else {
            return refuse(RelationProblem::NotAList);
        };
        links.push(link);
    }
    if links.len() > RELATION_LIMIT && links.iter().any(|link| !had.contains(link.card.as_str())) {
        return refuse(RelationProblem::TooMany);
    }
    if links.iter().any(|link| link.card == card_id) {
        return refuse(RelationProblem::ToItself);
    }
    let mut seen = HashSet::with_capacity(links.len());
    if !links.iter().all(|link| seen.insert(link.card.as_str())) {
        return refuse(RelationProblem::Repeated);
    }
    Ok(Some(links))
}

/// Makes the relation values of a card that is copied ones a write takes: a value that
/// is no list becomes `null`, and of a list what is no valid [`RelationLink`], the card
/// itself, and what names a card a second time go, while valid links are normalized
/// preserving `kind` and `lagDays`. For copies of stored cards, whose values may be
/// older than the rules or than a change of the definitions. Which of the cards named
/// are there, and so what is left of [`RELATION_LIMIT`], is for the copy to settle.
pub fn tidy_copied_relations(
    values: &mut JsonMap,
    card_id: &str,
    definitions: &[CardPropertyDefinition],
) {
    let relations = relation_property_ids(definitions);
    for (id, value) in values.iter_mut() {
        if !relations.contains(id.as_str()) {
            continue;
        }
        let Value::Array(items) = value else {
            if !value.is_null() {
                *value = Value::Null;
            }
            continue;
        };
        let mut seen = HashSet::with_capacity(items.len());
        let mut cleaned = Vec::with_capacity(items.len());
        for item in items.iter() {
            if let Ok(link) = parse_relation_link(item)
                && link.card != card_id
                && seen.insert(link.card.clone())
            {
                cleaned.push(link.to_value());
            }
        }
        *items = cleaned;
    }
}

/// The card relation properties of boards, by board identifier: whose values name
/// cards that get new identifiers along with them ([`crate::renew_block_ids`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RelationProperties {
    by_board: HashMap<String, HashSet<String>>,
}

impl RelationProperties {
    /// No board has relation properties.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The relation properties among one board's definitions.
    #[must_use]
    pub fn of_board(board_id: &str, definitions: &[CardPropertyDefinition]) -> Self {
        let mut properties = Self::new();
        properties.add_board(board_id, definitions);
        properties
    }

    /// Adds the relation properties among a board's definitions.
    pub fn add_board(&mut self, board_id: &str, definitions: &[CardPropertyDefinition]) {
        let ids: HashSet<String> = relation_property_ids(definitions)
            .into_iter()
            .map(str::to_owned)
            .collect();
        if !ids.is_empty() {
            self.by_board.insert(board_id.to_owned(), ids);
        }
    }

    /// The relation properties of a board, if it has any.
    pub(crate) fn of(&self, board_id: &str) -> Option<&HashSet<String>> {
        self.by_board.get(board_id)
    }
}

/// Returns whether setting `card_id`'s predecessors in a Gantt dependency relation to
/// `predecessors` closes a directed cycle (`predecessor -> ... -> card_id`) given the
/// board's full predecessor map (`card -> predecessors`, after applying the write).
///
/// Only newly introduced or modified edges from `card_id` are tested: if `card_id` is
/// reachable forward from `card_id` (equivalently, `card_id` is an ancestor of one of its
/// `predecessors`), a cycle is closed.
#[must_use]
pub fn closes_relation_cycle<S: BuildHasher>(
    predecessors_by_card: &HashMap<String, Vec<String>, S>,
    card_id: &str,
    predecessors: &[String],
) -> bool {
    let mut visited: HashSet<&str> = HashSet::new();
    let mut stack: Vec<&str> = predecessors.iter().map(String::as_str).collect();
    while let Some(current) = stack.pop() {
        if current == card_id {
            return true;
        }
        if !visited.insert(current) {
            continue;
        }
        if let Some(before) = predecessors_by_card.get(current) {
            for next in before {
                stack.push(next.as_str());
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn object(value: Value) -> JsonMap {
        match value {
            Value::Object(object) => object,
            other => panic!("not an object: {other}"),
        }
    }

    fn problem_of(value: &Value) -> Option<RelationProblem> {
        check_relation_value("me", "rel", value, &HashSet::new())
            .err()
            .map(|error| error.problem)
    }

    fn link_json(card: &str) -> Value {
        json!({"card": card, "kind": "fs", "lagDays": 0})
    }

    #[test]
    fn parse_link_normalizes_defaults_and_accepts_all_kinds_and_lag_bounds() {
        assert_eq!(
            parse_relation_link(&json!({"card": "a"})),
            Ok(RelationLink::new("a"))
        );
        assert_eq!(
            parse_relation_link(&json!({"card": "a", "kind": "ss"})),
            Ok(RelationLink {
                card: "a".to_owned(),
                kind: RelationKind::Ss,
                lag_days: 0,
            })
        );
        assert_eq!(
            parse_relation_link(&json!({"card": "a", "lagDays": -3650})),
            Ok(RelationLink {
                card: "a".to_owned(),
                kind: RelationKind::Fs,
                lag_days: RELATION_LAG_MIN,
            })
        );
        for (raw_kind, expected_kind, lag) in [
            ("fs", RelationKind::Fs, 0),
            ("ss", RelationKind::Ss, -5),
            ("ff", RelationKind::Ff, 12),
            ("sf", RelationKind::Sf, RELATION_LAG_MAX),
        ] {
            let parsed = parse_relation_link(&json!({
                "card": "a",
                "kind": raw_kind,
                "lagDays": lag,
            }))
            .expect("valid link");
            assert_eq!(parsed.kind, expected_kind);
            assert_eq!(parsed.lag_days, lag);
            assert_eq!(
                parsed.to_value(),
                json!({"card": "a", "kind": raw_kind, "lagDays": lag})
            );
        }
    }

    #[test]
    fn parse_link_rejects_legacy_strings_unknown_fields_and_bad_values() {
        assert_eq!(
            parse_relation_link(&json!("a")),
            Err(RelationLinkError::NotAnObject)
        );
        assert_eq!(
            parse_relation_link(&json!({"card": "a", "extra": 1})),
            Err(RelationLinkError::UnknownField {
                field: "extra".to_owned()
            })
        );
        for bad_card in [json!({}), json!({"card": ""}), json!({"card": 1})] {
            assert_eq!(
                parse_relation_link(&bad_card),
                Err(RelationLinkError::MissingOrInvalidCard),
                "{bad_card}"
            );
        }
        for bad_kind in [
            json!({"card": "a", "kind": "FS"}),
            json!({"card": "a", "kind": "unknown"}),
            json!({"card": "a", "kind": null}),
            json!({"card": "a", "kind": 1}),
        ] {
            assert_eq!(
                parse_relation_link(&bad_kind),
                Err(RelationLinkError::InvalidKind),
                "{bad_kind}"
            );
        }
        for bad_lag in [
            json!({"card": "a", "lagDays": RELATION_LAG_MIN - 1}),
            json!({"card": "a", "lagDays": RELATION_LAG_MAX + 1}),
            json!({"card": "a", "lagDays": 1.5}),
            json!({"card": "a", "lagDays": 1.0}),
            json!({"card": "a", "lagDays": "0"}),
            json!({"card": "a", "lagDays": null}),
            json!({"card": "a", "lagDays": true}),
        ] {
            assert_eq!(
                parse_relation_link(&bad_lag),
                Err(RelationLinkError::InvalidLagDays),
                "{bad_lag}"
            );
        }
    }

    #[test]
    fn a_value_is_null_or_a_list_of_other_cards_each_once() {
        assert_eq!(
            check_relation_value("me", "rel", &json!(null), &HashSet::new()),
            Ok(None)
        );
        assert_eq!(
            check_relation_value(
                "me",
                "rel",
                &json!([{"card": "a"}, {"card": "b", "kind": "ss", "lagDays": -2}]),
                &HashSet::new(),
            ),
            Ok(Some(vec![
                RelationLink::new("a"),
                RelationLink {
                    card: "b".to_owned(),
                    kind: RelationKind::Ss,
                    lag_days: -2,
                },
            ]))
        );
        assert_eq!(problem_of(&json!("a")), Some(RelationProblem::NotAList));
        assert_eq!(
            problem_of(&json!({"card": "a"})),
            Some(RelationProblem::NotAList)
        );
        assert_eq!(
            problem_of(&json!(["a", "b"])),
            Some(RelationProblem::NotAList),
            "legacy string arrays are refused"
        );
        assert_eq!(
            problem_of(&json!([{"card": "a"}, 1])),
            Some(RelationProblem::NotAList)
        );
        assert_eq!(
            problem_of(&json!([{"card": "a"}, {"card": "me"}])),
            Some(RelationProblem::ToItself)
        );
        assert_eq!(
            problem_of(&json!([
                {"card": "a", "kind": "fs"},
                {"card": "b"},
                {"card": "a", "kind": "ss"}
            ])),
            Some(RelationProblem::Repeated),
            "same card is repeated even with a different kind"
        );
    }

    #[test]
    fn a_value_names_at_most_the_limit() {
        let links = |count: usize| -> Value {
            Value::Array(
                (0..count)
                    .map(|index| link_json(&format!("c{index}")))
                    .collect(),
            )
        };
        assert!(check_relation_value("me", "rel", &links(RELATION_LIMIT), &HashSet::new()).is_ok());
        assert_eq!(
            problem_of(&links(RELATION_LIMIT + 1)),
            Some(RelationProblem::TooMany)
        );
        let stored: Vec<String> = (0..RELATION_LIMIT + 2)
            .map(|index| format!("c{index}"))
            .collect();
        let had: HashSet<&str> = stored.iter().map(String::as_str).collect();
        let fewer = Value::Array(stored[1..].iter().map(|id| link_json(id)).collect());
        assert!(
            check_relation_value("me", "rel", &fewer, &had).is_ok(),
            "a value over the limit from before the rules may lose cards"
        );
        let other = Value::Array(
            stored[2..]
                .iter()
                .map(|id| link_json(id))
                .chain(std::iter::once(link_json("new")))
                .collect(),
        );
        assert_eq!(
            check_relation_value("me", "rel", &other, &had).map_err(|error| error.problem),
            Err(RelationProblem::TooMany),
            "and may not take in another"
        );
    }

    #[test]
    fn the_refusal_names_the_property_and_not_the_cards() {
        let error = check_relation_value("me", "rel", &json!([{"card": "me"}]), &HashSet::new())
            .expect_err("the card itself is refused");
        assert_eq!(error.property, "rel");
        assert!(error.to_string().contains("\"rel\""), "{error}");
        assert!(!error.to_string().contains("me\""), "{error}");
    }

    #[test]
    fn relation_properties_are_those_of_the_type() {
        let definitions = [
            CardPropertyDefinition::new("rel", "Rel", PropertyType::Relation),
            CardPropertyDefinition::new("text", "Text", PropertyType::Text),
            CardPropertyDefinition::new("untyped", "", PropertyType::default()),
        ];
        assert_eq!(relation_property_ids(&definitions), HashSet::from(["rel"]));
        let properties = RelationProperties::of_board("b", &definitions);
        assert!(properties.of("b").is_some_and(|ids| ids.contains("rel")));
        assert!(properties.of("other").is_none());
        assert!(
            RelationProperties::of_board("b", &definitions[1..])
                .of("b")
                .is_none(),
            "a board without relation properties is not listed"
        );
    }

    #[test]
    fn copied_relations_are_normalized_and_preserve_kind_and_lag() {
        let definitions = [
            CardPropertyDefinition::new("rel", "Rel", PropertyType::Relation),
            CardPropertyDefinition::new("gone", "Gone", PropertyType::Relation),
            CardPropertyDefinition::new("text", "Text", PropertyType::Text),
        ];
        let many: Vec<Value> = (0..60)
            .map(|index| json!({"card": format!("c{index}"), "kind": "ff", "lagDays": 2}))
            .collect();
        let mut values = object(json!({
            "rel": [
                {"card": "a", "kind": "ss", "lagDays": -3},
                {"card": "me"},
                {"card": "a", "kind": "ff", "lagDays": 5},
                "legacy-id",
                {"card": "bad", "kind": "nope"},
                {"card": ""},
                {"card": "b"},
            ],
            "gone": "left by a text",
            "text": ["me", "me"],
            "unknown": ["me"],
        }));
        tidy_copied_relations(&mut values, "me", &definitions);
        assert_eq!(
            Value::Object(values),
            json!({
                "rel": [
                    {"card": "a", "kind": "ss", "lagDays": -3},
                    {"card": "b", "kind": "fs", "lagDays": 0},
                ],
                "gone": null,
                "text": ["me", "me"],
                "unknown": ["me"],
            })
        );
        let mut values = object(json!({ "rel": many }));
        tidy_copied_relations(&mut values, "me", &definitions);
        assert_eq!(
            values["rel"].as_array().map(Vec::len),
            Some(60),
            "what is past the limit is for the copy to cut, once it dropped what it cannot name"
        );
    }
}
