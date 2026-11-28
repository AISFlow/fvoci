//! The activity of a card: what was done to it, by whom and when, as
//! `getCardActivity` lists it.
//!
//! A line says what changed as the values were then: a title as it was written, a value
//! of a property as the card held it (the identifier of an option or of a person, a date
//! value), a parent card by its identifier. Names are read when the line is shown, so
//! that what was renamed since reads as it is called now and what is gone reads as
//! gone, as a reference in a body names its card by the identifier alone.

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::JsonMap;
use crate::block::Block;

/// What was done to a card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum CardEventKind {
    /// It was made: new, a copy, from a template, by an import, or as the next of a
    /// card that repeats.
    Created,
    /// Its title changed.
    Title,
    /// The value of one of its properties changed (`propertyId`).
    Property,
    /// It moved below another card, or to the top of its board.
    Parent,
    /// It became a template of the board's cards, or stopped being one.
    Template,
    /// It came back from the trash.
    Restored,
}

impl CardEventKind {
    /// Every kind, as the database's check lists them.
    pub const ALL: [Self; 6] = [
        Self::Created,
        Self::Title,
        Self::Property,
        Self::Parent,
        Self::Template,
        Self::Restored,
    ];

    /// The kind as it is written: in the JSON and in the database.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Title => "title",
            Self::Property => "property",
            Self::Parent => "parent",
            Self::Template => "template",
            Self::Restored => "restored",
        }
    }
}

impl fmt::Display for CardEventKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The error of reading a name that is no [`CardEventKind`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("no kind of a card's activity is called so")]
pub struct ParseCardEventKindError(());

impl FromStr for CardEventKind {
    type Err = ParseCardEventKindError;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.as_str() == name)
            .ok_or(ParseCardEventKindError(()))
    }
}

/// One line of a card's activity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CardEvent {
    /// The line's identifier.
    pub id: String,
    /// What was done.
    pub kind: CardEventKind,
    /// The property whose value changed, for `property`; `null` otherwise. It may be a
    /// property the board no longer has.
    pub property_id: Option<String>,
    /// What it was: the title, the value of the property as the card held it (`null`
    /// for none), the parent card's identifier (`null` for the top of the board), or
    /// whether the card was a template. `null` for `created` and `restored`.
    pub before: Value,
    /// What it became, as `before` says it.
    pub after: Value,
    /// Who did it; `null` for nobody the server names: a copy made by the server, or
    /// an account erased since.
    pub actor_id: Option<String>,
    /// The name of the personal API token an agent did it through, as the token was
    /// called then; `null` for a person's own doing, and once the token's owner is
    /// erased.
    pub actor_token_name: Option<String>,
    /// When it was done, in milliseconds since the Unix epoch.
    pub created_at: i64,
    /// When it was last done again: the same person changing the same thing again
    /// within five minutes changes the line rather than adding one.
    pub updated_at: i64,
}

/// A change of a card that its activity records, as [`card_changes`] finds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardChange {
    /// What changed: [`CardEventKind::Title`], [`CardEventKind::Property`],
    /// [`CardEventKind::Parent`] or [`CardEventKind::Template`].
    pub kind: CardEventKind,
    /// The property, for [`CardEventKind::Property`].
    pub property_id: Option<String>,
    /// What it was.
    pub before: Value,
    /// What it became.
    pub after: Value,
}

/// The changes of a card from `before` to `after` that its activity records: its title,
/// its parent, whether it is a template, then the value of each property whose value
/// differs, by the property's identifier. A property without a value and one whose
/// value is `null` are the same. Its body, icon and fields of no property are not
/// recorded: the versions of the body say what the body was.
#[must_use]
pub fn card_changes(before: &Block, after: &Block) -> Vec<CardChange> {
    let mut changes = Vec::new();
    let mut push = |kind, property_id: Option<&str>, was: Value, is: Value| {
        if was != is {
            changes.push(CardChange {
                kind,
                property_id: property_id.map(str::to_owned),
                before: was,
                after: is,
            });
        }
    };
    push(
        CardEventKind::Title,
        None,
        Value::from(before.title.as_str()),
        Value::from(after.title.as_str()),
    );
    push(
        CardEventKind::Parent,
        None,
        parent_value(&before.parent_id),
        parent_value(&after.parent_id),
    );
    push(
        CardEventKind::Template,
        None,
        Value::Bool(before.is_template()),
        Value::Bool(after.is_template()),
    );
    let (was, is) = (properties_of(before), properties_of(after));
    let named: BTreeSet<&String> = was.keys().chain(is.keys()).collect();
    for id in named {
        let value = |values: &JsonMap| values.get(id).cloned().unwrap_or(Value::Null);
        push(CardEventKind::Property, Some(id), value(was), value(is));
    }
    changes
}

/// A parent as a line says it: `null` for the top of the board.
fn parent_value(parent_id: &str) -> Value {
    if parent_id.is_empty() {
        Value::Null
    } else {
        Value::from(parent_id)
    }
}

/// The values of a card's properties; none if its `properties` is no object.
fn properties_of(block: &Block) -> &JsonMap {
    static NONE: std::sync::LazyLock<JsonMap> = std::sync::LazyLock::new(JsonMap::new);
    block
        .fields
        .get("properties")
        .and_then(Value::as_object)
        .unwrap_or(&NONE)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::block::BlockType;

    fn card(title: &str, fields: Value) -> Block {
        let Value::Object(fields) = fields else {
            panic!("fields are an object");
        };
        Block {
            id: "card".to_owned(),
            container_id: "board".to_owned(),
            block_type: BlockType::Card,
            title: title.to_owned(),
            fields,
            ..Block::default()
        }
    }

    #[test]
    fn a_card_unchanged_has_no_change() {
        let one = card("Plan", json!({"properties": {"s": "todo"}}));
        assert_eq!(card_changes(&one, &one.clone()), []);
    }

    #[test]
    fn each_change_is_one_line_title_parent_template_then_properties_by_identifier() {
        let before = card(
            "Plan",
            json!({"properties": {"b": "x", "a": ["u1"], "gone": "1"}}),
        );
        let mut after = card(
            "Plan again",
            json!({"isTemplate": true, "properties": {"a": ["u1", "u2"], "b": "x", "new": {"from": "2026-03-05"}}}),
        );
        after.parent_id = "parent".to_owned();
        let changes = card_changes(&before, &after);
        let kinds: Vec<(CardEventKind, Option<&str>)> = changes
            .iter()
            .map(|change| (change.kind, change.property_id.as_deref()))
            .collect();
        assert_eq!(
            kinds,
            [
                (CardEventKind::Title, None),
                (CardEventKind::Parent, None),
                (CardEventKind::Template, None),
                (CardEventKind::Property, Some("a")),
                (CardEventKind::Property, Some("gone")),
                (CardEventKind::Property, Some("new")),
            ]
        );
        assert_eq!(changes[0].before, json!("Plan"));
        assert_eq!(changes[1].before, Value::Null, "the top of the board");
        assert_eq!(changes[1].after, json!("parent"));
        assert_eq!(
            (&changes[2].before, &changes[2].after),
            (&json!(false), &json!(true))
        );
        assert_eq!(
            changes[3].after,
            json!(["u1", "u2"]),
            "the identifiers as held"
        );
        assert_eq!(changes[4].after, Value::Null, "a value taken away");
        assert_eq!(changes[5].before, Value::Null, "a value given");
    }

    #[test]
    fn a_value_of_null_is_none() {
        let before = card("t", json!({"properties": {"a": null}}));
        let after = card("t", json!({}));
        assert_eq!(card_changes(&before, &after), []);
    }

    #[test]
    fn the_kinds_are_read_as_they_are_written() {
        for kind in CardEventKind::ALL {
            assert_eq!(kind.as_str().parse::<CardEventKind>(), Ok(kind));
            assert_eq!(
                serde_json::to_value(kind).expect("serialized"),
                json!(kind.as_str())
            );
        }
        assert!("comment".parse::<CardEventKind>().is_err());
    }
}
