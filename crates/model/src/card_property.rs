//! Card property definitions and card property values.
//!
//! A board defines the properties of its cards (`cardProperties` as
//! [`CardPropertyDefinition`]s), and every card holds a value per property
//! (`fields.properties` of its block): a value is text, a list of texts, a date value, a
//! list of relation links, or `null`. Anything else is refused before it is stored, so that
//! one member cannot break the board for the others.

use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::JsonMap;
use crate::date_value::{DateValueError, check_date_value};
use crate::refusal::{RefusalReason, Refusals};
use crate::relation::{RelationLinkError, parse_relation_link};

/// The `type` of a property definition whose values are date values.
pub const DATE_PROPERTY_TYPE: &str = "date";

/// Chip color shown for a [`PropertyOption`].
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, utoipa::ToSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum PropertyColor {
    /// Neutral chip color when none was chosen.
    #[default]
    Default,
    /// Muted gray chip color.
    Gray,
    /// Warm brown chip color.
    Brown,
    /// Orange chip color.
    Orange,
    /// Yellow chip color.
    Yellow,
    /// Green chip color.
    Green,
    /// Blue chip color.
    Blue,
    /// Purple chip color.
    Purple,
    /// Pink chip color.
    Pink,
    /// Red chip color.
    Red,
}

impl PropertyColor {
    /// Every option color in canonical order.
    pub const ALL: [Self; 10] = [
        Self::Default,
        Self::Gray,
        Self::Brown,
        Self::Orange,
        Self::Yellow,
        Self::Green,
        Self::Blue,
        Self::Purple,
        Self::Pink,
        Self::Red,
    ];

    /// The wire words in the order of [`Self::ALL`].
    pub const WORDS: &[&str] = &[
        "default", "gray", "brown", "orange", "yellow", "green", "blue", "purple", "pink", "red",
    ];

    /// Returns the wire and database form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Gray => "gray",
            Self::Brown => "brown",
            Self::Orange => "orange",
            Self::Yellow => "yellow",
            Self::Green => "green",
            Self::Blue => "blue",
            Self::Purple => "purple",
            Self::Pink => "pink",
            Self::Red => "red",
        }
    }
}

impl std::fmt::Display for PropertyColor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error returned when a string names no [`PropertyColor`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("the text names no property color")]
pub struct ParsePropertyColorError;

impl FromStr for PropertyColor {
    type Err = ParsePropertyColorError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|color| color.as_str() == text)
            .ok_or(ParsePropertyColorError)
    }
}

/// The kind of values cards hold for a [`CardPropertyDefinition`].
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, utoipa::ToSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum PropertyType {
    /// Free-form plain text.
    #[default]
    Text,
    /// Numeric value stored as text.
    Number,
    /// Single choice from [`CardPropertyDefinition::options`].
    Select,
    /// Zero or more choices from [`CardPropertyDefinition::options`].
    MultiSelect,
    /// Calendar date, date range, or millisecond timestamp.
    Date,
    /// Single assigned workspace user ID.
    Person,
    /// Multiple assigned workspace user IDs.
    MultiPerson,
    /// Uploaded file attachment reference.
    File,
    /// Boolean flag stored as `"true"` or empty.
    Checkbox,
    /// Web address.
    Url,
    /// Email address.
    Email,
    /// Telephone number.
    Phone,
    /// Automatic card creation timestamp (`createdAt`).
    CreatedTime,
    /// Automatic card creator (`createdBy`).
    CreatedBy,
    /// Automatic card modification timestamp (`updatedAt`).
    UpdatedTime,
    /// Automatic card last modifier (`modifiedBy`).
    UpdatedBy,
    /// Dependency links to other cards on the same board.
    Relation,
}

impl PropertyType {
    /// Every property kind in canonical order.
    pub const ALL: [Self; 17] = [
        Self::Text,
        Self::Number,
        Self::Select,
        Self::MultiSelect,
        Self::Date,
        Self::Person,
        Self::MultiPerson,
        Self::File,
        Self::Checkbox,
        Self::Url,
        Self::Email,
        Self::Phone,
        Self::CreatedTime,
        Self::CreatedBy,
        Self::UpdatedTime,
        Self::UpdatedBy,
        Self::Relation,
    ];

    /// The wire words in the order of [`Self::ALL`].
    pub const WORDS: &[&str] = &[
        "text",
        "number",
        "select",
        "multiSelect",
        "date",
        "person",
        "multiPerson",
        "file",
        "checkbox",
        "url",
        "email",
        "phone",
        "createdTime",
        "createdBy",
        "updatedTime",
        "updatedBy",
        "relation",
    ];

    /// Returns the wire and database form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Number => "number",
            Self::Select => "select",
            Self::MultiSelect => "multiSelect",
            Self::Date => "date",
            Self::Person => "person",
            Self::MultiPerson => "multiPerson",
            Self::File => "file",
            Self::Checkbox => "checkbox",
            Self::Url => "url",
            Self::Email => "email",
            Self::Phone => "phone",
            Self::CreatedTime => "createdTime",
            Self::CreatedBy => "createdBy",
            Self::UpdatedTime => "updatedTime",
            Self::UpdatedBy => "updatedBy",
            Self::Relation => "relation",
        }
    }

    /// Whether values or filters on this property name workspace users.
    #[must_use]
    pub const fn names_people(self) -> bool {
        matches!(
            self,
            Self::Person | Self::MultiPerson | Self::CreatedBy | Self::UpdatedBy
        )
    }

    /// Whether values on this property record whom a card is assigned to.
    #[must_use]
    pub const fn is_assigned_person(self) -> bool {
        matches!(self, Self::Person | Self::MultiPerson)
    }
}

impl std::fmt::Display for PropertyType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error returned when a string names no [`PropertyType`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("the text names no property type")]
pub struct ParsePropertyTypeError;

impl FromStr for PropertyType {
    type Err = ParsePropertyTypeError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.as_str() == text)
            .ok_or(ParsePropertyTypeError)
    }
}

/// A selectable option of a `select` or `multiSelect` [`CardPropertyDefinition`].
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PropertyOption {
    /// The option's identifier within the property.
    pub id: String,
    /// The option's display text.
    pub label: String,
    /// The chip color shown for this option. Left out in a request, `default`.
    #[serde(default)]
    pub color: PropertyColor,
}

impl PropertyOption {
    /// Constructs a [`PropertyOption`] with `id`, `label`, and `color`.
    #[must_use]
    pub fn new(id: impl Into<String>, label: impl Into<String>, color: PropertyColor) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            color,
        }
    }

    /// Whether any string of the option contains `U+0000`.
    #[must_use]
    pub(crate) fn holds_nul(&self) -> bool {
        self.id.contains('\0') || self.label.contains('\0')
    }
}

/// A property definition of a board's cards (`Container::card_properties`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CardPropertyDefinition {
    /// The property's identifier on its board; non-empty and unique on the board.
    #[serde(default)]
    pub id: String,
    /// The property's display name. Left out in a request, empty.
    #[serde(default)]
    pub name: String,
    /// The kind of values cards hold for this property. Left out in a request, `text`.
    #[serde(default, rename = "type")]
    pub property_type: PropertyType,
    /// Selectable options when `type` is `select` or `multiSelect`. Left out, empty.
    #[serde(default)]
    pub options: Vec<PropertyOption>,
}

impl CardPropertyDefinition {
    /// Constructs a [`CardPropertyDefinition`] with no options.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        property_type: PropertyType,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            property_type,
            options: Vec::new(),
        }
    }

    /// Whether any string of the definition or its options contains `U+0000`.
    #[must_use]
    pub(crate) fn holds_nul(&self) -> bool {
        self.id.contains('\0')
            || self.name.contains('\0')
            || self.options.iter().any(PropertyOption::holds_nul)
    }
}

/// Reasons card property data is rejected before it is stored.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CardPropertyError {
    /// The properties of a block are not a JSON object.
    #[error("card properties must be an object of property values")]
    ValuesNotObject,
    /// A property value is neither a string, nor a list of strings, nor a date value,
    /// nor a list of relation links, nor `null`.
    #[error(
        "the value of card property {id:?} must be a string, a list of strings, a date value or a list of relation links"
    )]
    InvalidValue {
        /// The identifier of the property.
        id: String,
    },
    /// A property value is an object that is no date value.
    #[error("the date value of card property {id:?} is refused: {reason}")]
    InvalidDateValue {
        /// The identifier of the property.
        id: String,
        /// What is wrong with it.
        reason: DateValueError,
    },
    /// A property value is a list containing an invalid relation link object.
    #[error("the relation link of card property {id:?} is refused: {reason}")]
    InvalidRelationLink {
        /// The identifier of the property.
        id: String,
        /// What is wrong with it.
        reason: RelationLinkError,
    },
    /// The value of a date property is not a date value.
    #[error("card property {id:?} is a date: its value is a date value or null")]
    NotADateValue {
        /// The identifier of the property.
        id: String,
    },
    /// A date value is the value of a property that is no date.
    #[error("card property {id:?} is not a date property, so its value cannot be a date value")]
    DateValueOfOtherProperty {
        /// The identifier of the property.
        id: String,
    },
    /// A list of relation links is the value of a property that is no card relation.
    #[error(
        "card property {id:?} is not a relation property, so its value cannot be a list of relation links"
    )]
    RelationValueOfOtherProperty {
        /// The identifier of the property.
        id: String,
    },
    /// A property definition has no `id`, or one that is empty.
    #[error("card property definition {position} needs a non-empty string as id")]
    MissingId {
        /// Which definition of the list it is, counted from 1.
        position: usize,
    },
    /// Two property definitions have the same `id`.
    #[error("two card property definitions have the id {id:?}")]
    DuplicateId {
        /// The identifier that is there twice.
        id: String,
    },
    /// An option of a property definition has an empty `id`.
    #[error("option {position} of card property {id:?} needs a non-empty string as id")]
    MissingOptionId {
        /// The identifier of the property definition.
        id: String,
        /// Which option of the definition it is, counted from 1.
        position: usize,
    },
    /// Two options of a property definition share the same `id`.
    #[error("two options of card property {id:?} have the id {option_id:?}")]
    DuplicateOptionId {
        /// The identifier of the property definition.
        id: String,
        /// The option identifier that is there twice.
        option_id: String,
    },
    /// The value of a typed property (`number`, `email`, `url`, `checkbox`, `select`,
    /// `multiSelect`, `person`, `multiPerson`) does not match that property's rules.
    #[error("the value of card property {id:?} must be {expected}")]
    InvalidTypedValue {
        /// The identifier of the property.
        id: String,
        /// Expected format description.
        expected: &'static str,
    },
}

/// Checks the property values of a card: each is a string, a list of strings, a date
/// value, a list of relation links, or `null`. Whether a date or relation value belongs
/// to a property of that kind is for [`validate_card_property_kinds`], which needs the
/// board's definitions.
///
/// # Errors
///
/// Returns [`CardPropertyError::InvalidValue`] for the first value of another shape,
/// [`CardPropertyError::InvalidDateValue`] for an object that is no date value, and
/// [`CardPropertyError::InvalidRelationLink`] for an invalid relation link object.
pub fn validate_card_property_values(values: &JsonMap) -> Result<(), CardPropertyError> {
    values
        .iter()
        .try_for_each(|(id, value)| check_property_value(id, value))
}

/// Checks the value of one property of a card.
fn check_property_value(id: &str, value: &Value) -> Result<(), CardPropertyError> {
    match value {
        Value::Null | Value::String(_) => Ok(()),
        Value::Array(items) => {
            if items.iter().all(Value::is_string) {
                return Ok(());
            }
            if items.iter().all(Value::is_object) {
                for item in items {
                    parse_relation_link(item).map_err(|reason| {
                        CardPropertyError::InvalidRelationLink {
                            id: id.to_owned(),
                            reason,
                        }
                    })?;
                }
                return Ok(());
            }
            Err(CardPropertyError::InvalidValue { id: id.to_owned() })
        }
        Value::Object(date) => {
            check_date_value(date).map_err(|reason| CardPropertyError::InvalidDateValue {
                id: id.to_owned(),
                reason,
            })
        }
        Value::Bool(_) | Value::Number(_) => {
            Err(CardPropertyError::InvalidValue { id: id.to_owned() })
        }
    }
}

/// Checks the property values a request carries as its member `member` (`properties`
/// of a card, `updatedProperties` of a patch), as [`validate_card_property_values`]
/// does, and tells of every value that is refused, not the first alone.
///
/// # Errors
///
/// Returns every value of another shape, each at `/<member>/<property>`.
pub(crate) fn check_card_property_values<'v>(
    values: impl IntoIterator<Item = (&'v String, &'v Value)>,
    member: &str,
) -> Result<(), Refusals> {
    let mut refused = Refusals::new();
    for (id, value) in values {
        if let Err(error) = check_property_value(id, value) {
            refused.refuse_within(member, id, RefusalReason::Invalid, error.to_string());
        }
    }
    refused.into_result()
}

/// Checks that the date values of a card are the values of its board's date properties,
/// that a date property has a date value or `null`, and that a list of relation link
/// objects is only the value of a relation property. A value of a property the board
/// does not define is neither a date value nor a relation link list.
///
/// # Errors
///
/// Returns [`CardPropertyError::NotADateValue`],
/// [`CardPropertyError::DateValueOfOtherProperty`], or
/// [`CardPropertyError::RelationValueOfOtherProperty`] for the first value in the wrong
/// place.
pub fn validate_card_property_kinds(
    values: &JsonMap,
    definitions: &[CardPropertyDefinition],
) -> Result<(), CardPropertyError> {
    for (id, value) in values {
        let definition = definitions.iter().find(|def| def.id == *id);
        if definition.is_some_and(|def| def.property_type == PropertyType::Date) {
            if !matches!(value, Value::Null | Value::Object(_)) {
                return Err(CardPropertyError::NotADateValue { id: id.clone() });
            }
            continue;
        }
        if value.is_object() {
            return Err(CardPropertyError::DateValueOfOtherProperty { id: id.clone() });
        }
        if is_relation_links_array(value)
            && !definition.is_some_and(|def| def.property_type == PropertyType::Relation)
        {
            return Err(CardPropertyError::RelationValueOfOtherProperty { id: id.clone() });
        }
        if let Some(def) = definition {
            check_typed_property_value(def, value)?;
        }
    }
    Ok(())
}

fn check_typed_property_value(
    definition: &CardPropertyDefinition,
    value: &Value,
) -> Result<(), CardPropertyError> {
    if value.is_null() {
        return Ok(());
    }
    let bad = |expected: &'static str| {
        Err(CardPropertyError::InvalidTypedValue {
            id: definition.id.clone(),
            expected,
        })
    };
    match definition.property_type {
        PropertyType::Number => match value.as_str() {
            Some(text) if is_number_entry(text) => Ok(()),
            _ => bad("a number text or empty string"),
        },
        PropertyType::Email => match value.as_str() {
            Some(text) if is_email_entry(text) => Ok(()),
            _ => bad("an email address or empty string"),
        },
        PropertyType::Url => match value.as_str() {
            Some(text) if is_url_entry(text) => Ok(()),
            _ => bad("a URL without whitespace or empty string"),
        },
        PropertyType::Checkbox => match value.as_str() {
            Some("" | "true" | "false") => Ok(()),
            _ => bad("\"true\", \"false\" or empty string"),
        },
        PropertyType::Select => match value.as_str() {
            Some(text)
                if text.is_empty() || definition.options.iter().any(|opt| opt.id == text) =>
            {
                Ok(())
            }
            _ => bad("one of the property's option IDs or empty string"),
        },
        PropertyType::MultiSelect => match value {
            Value::String(text) if text.is_empty() => Ok(()),
            Value::Array(items)
                if items.iter().all(|item| {
                    item.as_str().is_some_and(|text| {
                        text.is_empty() || definition.options.iter().any(|opt| opt.id == text)
                    })
                }) =>
            {
                Ok(())
            }
            _ => bad("a list of the property's option IDs"),
        },
        PropertyType::Person => match value.as_str() {
            Some(_) => Ok(()),
            None => bad("a user ID or empty string"),
        },
        PropertyType::MultiPerson => match value {
            Value::String(text) if text.is_empty() => Ok(()),
            Value::Array(items) if items.iter().all(Value::is_string) => Ok(()),
            _ => bad("a list of user IDs"),
        },
        PropertyType::Date
        | PropertyType::Relation
        | PropertyType::Text
        | PropertyType::Phone
        | PropertyType::File
        | PropertyType::CreatedTime
        | PropertyType::CreatedBy
        | PropertyType::UpdatedTime
        | PropertyType::UpdatedBy => Ok(()),
    }
}

fn is_number_entry(text: &str) -> bool {
    if text.is_empty() {
        return true;
    }
    let trimmed = text.trim_start();
    let digits = trimmed.strip_prefix(['+', '-']).unwrap_or(trimmed);
    digits.bytes().next().is_some_and(|b| b.is_ascii_digit())
}

fn is_email_entry(text: &str) -> bool {
    if text.is_empty() {
        return true;
    }
    let Some((local, host)) = text.split_once('@') else {
        return false;
    };
    if local.is_empty()
        || host.contains('@')
        || local.chars().any(|c| {
            c.is_whitespace()
                || matches!(
                    c,
                    '@' | '<' | '>' | '(' | ')' | '[' | ']' | '\\' | ',' | ';' | ':' | '"'
                )
        })
    {
        return false;
    }
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() < 2 {
        return false;
    }
    let (tld, labels) = parts.split_last().expect("at least 2 parts");
    labels
        .iter()
        .all(|label| !label.is_empty() && label.chars().all(|c| c.is_alphanumeric() || c == '-'))
        && tld.chars().count() >= 2
        && tld.chars().all(char::is_alphabetic)
}

fn is_url_entry(text: &str) -> bool {
    let address = text.trim();
    address.is_empty()
        || (!address.chars().any(char::is_whitespace) && address.chars().any(char::is_alphanumeric))
}

/// Whether a value is a non-empty list of relation link objects.
fn is_relation_links_array(value: &Value) -> bool {
    value
        .as_array()
        .is_some_and(|items| !items.is_empty() && items.iter().all(Value::is_object))
}

/// Sets to `null` the values that no longer fit the kind of their property: those of a
/// property that became or stopped being a date, stopped being a relation, or that the
/// board no longer defines while holding a date or relation link value. For what is
/// copied from stored cards, which may be older than a change of the definitions.
/// Returns how many were cleared.
pub fn clear_values_of_another_kind(
    values: &mut JsonMap,
    definitions: &[CardPropertyDefinition],
) -> usize {
    let mut cleared = 0;
    for (id, value) in values.iter_mut() {
        let definition = definitions.iter().find(|def| def.id == *id);
        let is_date = definition.is_some_and(|def| def.property_type == PropertyType::Date);
        // A value of another kind; a malformed one is left to be refused.
        let mut other_kind = match value {
            Value::Object(_) => !is_date,
            Value::String(_) => is_date,
            Value::Array(_) => {
                is_date
                    || (is_relation_links_array(value)
                        && !definition
                            .is_some_and(|def| def.property_type == PropertyType::Relation))
            }
            Value::Null | Value::Bool(_) | Value::Number(_) => false,
        };
        if !other_kind
            && let Some(def) = definition
            && !matches!(
                def.property_type,
                PropertyType::Date | PropertyType::Relation
            )
            && check_property_value(id, value).is_ok()
        {
            if def.property_type == PropertyType::MultiSelect
                && let Value::Array(items) = value
            {
                let before = items.len();
                items.retain(|item| {
                    item.as_str()
                        .is_some_and(|opt_id| def.options.iter().any(|opt| opt.id == opt_id))
                });
                if items.len() != before {
                    cleared += 1;
                }
            } else if check_typed_property_value(def, value).is_err() {
                other_kind = true;
            }
        }
        if other_kind {
            *value = Value::Null;
            cleared += 1;
        }
    }
    cleared
}

/// Checks the `properties` among a block's fields, if there are any: a JSON object of
/// valid property values.
///
/// # Errors
///
/// Returns [`CardPropertyError::ValuesNotObject`] if `properties` is there and is not an
/// object, or the error of [`validate_card_property_values`].
pub fn validate_block_properties(fields: &JsonMap) -> Result<(), CardPropertyError> {
    match fields.get("properties") {
        None => Ok(()),
        Some(Value::Object(values)) => validate_card_property_values(values),
        Some(_) => Err(CardPropertyError::ValuesNotObject),
    }
}

/// Checks the card property definitions of a board: each has a non-empty `id` that no
/// earlier definition of the board has, and each option within a definition has a
/// non-empty `id` unique within that definition.
///
/// # Errors
///
/// Returns [`CardPropertyError::MissingId`], [`CardPropertyError::DuplicateId`],
/// [`CardPropertyError::MissingOptionId`], or [`CardPropertyError::DuplicateOptionId`]
/// for the first offending definition or option.
pub fn validate_card_property_definitions(
    definitions: &[CardPropertyDefinition],
) -> Result<(), CardPropertyError> {
    let mut seen = std::collections::HashSet::with_capacity(definitions.len());
    for (index, definition) in definitions.iter().enumerate() {
        if definition.id.is_empty() {
            return Err(CardPropertyError::MissingId {
                position: index + 1,
            });
        }
        if !seen.insert(definition.id.as_str()) {
            return Err(CardPropertyError::DuplicateId {
                id: definition.id.clone(),
            });
        }
        let mut seen_options = std::collections::HashSet::with_capacity(definition.options.len());
        for (opt_idx, option) in definition.options.iter().enumerate() {
            if option.id.is_empty() {
                return Err(CardPropertyError::MissingOptionId {
                    id: definition.id.clone(),
                    position: opt_idx + 1,
                });
            }
            if !seen_options.insert(option.id.as_str()) {
                return Err(CardPropertyError::DuplicateOptionId {
                    id: definition.id.clone(),
                    option_id: option.id.clone(),
                });
            }
        }
    }
    Ok(())
}

/// The users that the values of a card name in the properties of its board that hold
/// whom the card is given to (`person`, `multiPerson`), each once, in the order of the
/// definitions. A value is a user, or a list of users; anything else names nobody.
#[must_use]
pub fn assigned_users<'v>(
    values: &'v JsonMap,
    definitions: &[CardPropertyDefinition],
) -> Vec<&'v str> {
    let mut users: Vec<&str> = Vec::new();
    for definition in definitions {
        if definition.id.is_empty() || !definition.property_type.is_assigned_person() {
            continue;
        }
        let named: Vec<&str> = match values.get(&definition.id) {
            Some(Value::String(user)) => vec![user.as_str()],
            Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };
        for user in named {
            if !user.is_empty() && !users.contains(&user) {
                users.push(user);
            }
        }
    }
    users
}

/// The members of a view's `fields` that are lists of the groups (the options, or for a
/// person property the users) the view shows, hides or folds.
const VIEW_GROUP_LISTS: [&str; 3] = ["visibleGroups", "hiddenGroups", "collapsedGroups"];

/// The member of a view's `fields` that is keyed by the groups the view shows.
const VIEW_GROUP_KEYED: &str = "groupCalculations";

/// The property of a view's filter that is the card's title, defined by no board.
const TITLE: &str = "title";

/// The properties of a board as whoever reads it by its read token alone may be told of
/// them: which ones it defines, which of those name users, and the options of those a
/// view may group by without naming anybody.
///
/// Such a reader is not told who the server's users are: the values of person properties go
/// to them emptied, one empty string for each user, so that how many people a card has
/// still shows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PersonProperties {
    defined: BTreeSet<String>,
    people: BTreeSet<String>,
    /// The identifiers of the options of each `select` property.
    options: BTreeMap<String, BTreeSet<String>>,
}

impl PersonProperties {
    /// The properties among a board's definitions.
    #[must_use]
    pub fn of(definitions: &[CardPropertyDefinition]) -> Self {
        let mut properties = Self::default();
        for definition in definitions {
            if definition.id.is_empty() {
                continue;
            }
            properties.defined.insert(definition.id.clone());
            if definition.property_type.names_people() {
                properties.people.insert(definition.id.clone());
            }
            if definition.property_type == PropertyType::Select {
                let ids = definition
                    .options
                    .iter()
                    .filter(|option| !option.id.is_empty())
                    .map(|option| option.id.clone())
                    .collect();
                properties.options.insert(definition.id.clone(), ids);
            }
        }
        properties
    }

    /// Empties the users that a card's property values name: a text becomes the empty
    /// text, and each text of a list the empty text. A value of a property the board no
    /// longer defines goes: a person property that was deleted leaves its users behind,
    /// and nothing shows such a value.
    pub fn hide_values(&self, values: &mut JsonMap) {
        values.retain(|id, _| self.defined.contains(id));
        for (id, value) in values.iter_mut() {
            if self.people.contains(id) {
                hide_users(value);
            }
        }
    }

    /// Empties the users that a view's `fields` name. In the clauses of its filter
    /// (`filter`, a group `{operation, filters}` whose filters are groups or clauses
    /// `{propertyId, condition, values}`) the values compared with a person property
    /// become empty strings. A view grouped by a person property (`groupBy`) keeps of
    /// its lists of groups (`visibleGroups`, `hiddenGroups`, `collapsedGroups`)
    /// and of `groupCalculations` only the group of the cards without a value, the
    /// empty string: an emptied user would be taken for it.
    ///
    /// A property the board no longer defines may have been a person property, whose
    /// users the view still names: it counts as one. The card's title (`title`), which
    /// is no definition, names nobody. The lists of groups keep only the options of the
    /// `select` property the view groups by, and the empty string: a view grouped by
    /// anything else (or by nothing) is grouped by the app by a property of its choosing,
    /// a person property among them, whose users the lists may hold.
    pub fn hide_view_fields(&self, fields: &mut JsonMap) {
        fields
            .retain(|member, _| crate::view_fields::SHARED_VIEW_MEMBERS.contains(&member.as_str()));
        if let Some(filter) = fields.get_mut("filter") {
            self.hide_filter_values(filter);
        }
        let options = fields
            .get("groupBy")
            .and_then(Value::as_str)
            .and_then(|id| self.options.get(id))
            .cloned();
        let shown = |group: &str| {
            group.is_empty() || options.as_ref().is_some_and(|ids| ids.contains(group))
        };
        for list in VIEW_GROUP_LISTS {
            match fields.get_mut(list) {
                Some(Value::Array(groups)) => {
                    groups.retain(|group| group.as_str().is_some_and(shown));
                }
                Some(_) => {
                    fields.remove(list);
                }
                None => {}
            }
        }
        match fields.get_mut(VIEW_GROUP_KEYED) {
            Some(Value::Object(groups)) => {
                groups.retain(|group, _| shown(group));
            }
            Some(_) => {
                fields.remove(VIEW_GROUP_KEYED);
            }
            None => {}
        }
    }

    /// Whether the values of the property may be users: it is a person property, or
    /// one the board does not define (any more).
    fn may_name_people(&self, id: &str) -> bool {
        self.people.contains(id) || !self.defined.contains(id)
    }

    /// See [`Self::hide_view_fields`].
    fn hide_filter_values(&self, filter: &mut Value) {
        if let Some(filters) = filter.get_mut("filters").and_then(Value::as_array_mut) {
            for filter in filters {
                self.hide_filter_values(filter);
            }
            return;
        }
        let names_a_person = filter
            .get("propertyId")
            .and_then(Value::as_str)
            .is_some_and(|id| id != TITLE && self.may_name_people(id));
        if names_a_person && let Some(values) = filter.get_mut("values") {
            hide_users(values);
        }
    }
}

/// Empties a text, or each text of a list; leaves anything else as it is.
fn hide_users(value: &mut Value) {
    match value {
        Value::String(user) => user.clear(),
        Value::Array(users) => {
            for user in users.iter_mut() {
                if let Value::String(user) = user {
                    user.clear();
                }
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::Object(_) => {}
    }
}

#[cfg(test)]
mod tests;
