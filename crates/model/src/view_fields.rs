//! Formal validation and fractional-index ordering for board view `fields`
//! (`BlockType::View`).

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::JsonMap;
use crate::block::Block;
use crate::card_property::{CardPropertyDefinition, PropertyType};
use crate::date_value::is_calendar_date;
use crate::id::is_id;
use crate::page::{DIGITS, PAGE_ORDER_FIELD, is_order, order_between};

/// Closed allowlist of the 17 member names a board view's `fields` may carry.
pub const SHARED_VIEW_MEMBERS: [&str; 17] = [
    "viewType",
    "sort",
    "shownProperties",
    "visibleGroups",
    "hiddenGroups",
    "collapsedGroups",
    "filter",
    "cardOrder",
    "columnWidths",
    "columnCalculations",
    "groupCalculations",
    "defaultTemplateId",
    "groupBy",
    "dateProperty",
    "dependencyPropertyId",
    "ganttZoom",
    "order",
];

/// Maximum nesting depth of nested `FilterGroup`s inside a view's `filter`.
const MAX_FILTER_DEPTH: usize = 8;

/// Longest fractional order string before sibling views are renumbered evenly.
pub const VIEW_ORDER_MAX_CHARS: usize = 32;

/// Dynamic person filter token meaning the signed-in viewer.
pub const FILTER_PERSON_ME: &str = "me";

/// Dynamic date filter token meaning the signed-in viewer's local calendar day.
pub const FILTER_DATE_TODAY: &str = "today";

/// Layout kind of a board view.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, utoipa::ToSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum ViewType {
    /// Kanban board grouped into columns.
    #[default]
    Board,
    /// Spreadsheet-like table of cards and properties.
    Table,
    /// Visual card gallery.
    Gallery,
    /// Month/week calendar by a date property.
    Calendar,
    /// Timeline chart with optional dependency arrows.
    Gantt,
}

impl ViewType {
    /// Every layout kind in wire order.
    pub const ALL: [Self; 5] = [
        Self::Board,
        Self::Table,
        Self::Gallery,
        Self::Calendar,
        Self::Gantt,
    ];

    /// Wire spelling of the view layout kind.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Board => "board",
            Self::Table => "table",
            Self::Gallery => "gallery",
            Self::Calendar => "calendar",
            Self::Gantt => "gantt",
        }
    }
}

impl fmt::Display for ViewType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Unrecognized [`ViewType`] wire string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("viewType must be one of board, table, gallery, calendar or gantt")]
pub struct ParseViewTypeError;

impl FromStr for ViewType {
    type Err = ParseViewTypeError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.as_str() == text)
            .ok_or(ParseViewTypeError)
    }
}

/// Zoom axis scale of a Gantt view.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, utoipa::ToSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum GanttZoom {
    /// One column per calendar day.
    Day,
    /// One column per week.
    #[default]
    Week,
    /// One column per month.
    Month,
}

impl GanttZoom {
    /// Wire spelling of the Gantt zoom level.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
        }
    }
}

impl FromStr for GanttZoom {
    type Err = ();

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "day" => Ok(Self::Day),
            "week" => Ok(Self::Week),
            "month" => Ok(Self::Month),
            _ => Err(()),
        }
    }
}

const FILTER_CONDITIONS: [&str; 15] = [
    "includes",
    "notIncludes",
    "isEmpty",
    "isNotEmpty",
    "isSet",
    "isNotSet",
    "is",
    "contains",
    "notContains",
    "startsWith",
    "notStartsWith",
    "endsWith",
    "notEndsWith",
    "isBefore",
    "isAfter",
];

/// Why a board view's `fields` was refused.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ViewFieldsError {
    /// The `fields` map carries an unrecognized member name.
    #[error("unknown view field {field:?}")]
    UnknownField {
        /// Offending member name.
        field: String,
    },
    /// A known view field has a value of the wrong shape.
    #[error("the view field {field} must be {expected}")]
    InvalidField {
        /// View field name.
        field: &'static str,
        /// Expected wire shape.
        expected: &'static str,
    },
}

/// Validates all present members of a board view's `fields` against the closed 17-key
/// `BoardViewFields` schema. `null` on an optional member is accepted as clearing it.
///
/// # Errors
///
/// Returns [`ViewFieldsError`] for the first unknown key or malformed member value.
pub fn validate_view_fields(fields: &JsonMap) -> Result<(), ViewFieldsError> {
    for (name, value) in fields {
        if !SHARED_VIEW_MEMBERS.contains(&name.as_str()) {
            return Err(ViewFieldsError::UnknownField {
                field: name.clone(),
            });
        }
        if value.is_null() {
            continue;
        }
        check_view_member(name.as_str(), value)?;
    }
    Ok(())
}

fn check_view_member(name: &str, value: &Value) -> Result<(), ViewFieldsError> {
    let bad = |field: &'static str, expected: &'static str| {
        Err(ViewFieldsError::InvalidField { field, expected })
    };
    match name {
        "viewType" => match value.as_str().and_then(|s| s.parse::<ViewType>().ok()) {
            Some(_) => Ok(()),
            None => bad(
                "viewType",
                "one of board, table, gallery, calendar or gantt",
            ),
        },
        "sort" => match value.as_array() {
            Some(items) if items.iter().all(is_sort_option) => Ok(()),
            _ => bad("sort", "a list of {propertyId, reversed} objects"),
        },
        "shownProperties" => check_string_list("shownProperties", value),
        "visibleGroups" => check_string_list("visibleGroups", value),
        "hiddenGroups" => check_string_list("hiddenGroups", value),
        "collapsedGroups" => check_string_list("collapsedGroups", value),
        "cardOrder" => check_string_list("cardOrder", value),
        "filter" => {
            if is_filter_group(value, 0) {
                Ok(())
            } else {
                bad("filter", "a filter group {operation, filters}")
            }
        }
        "columnWidths" => match value.as_object() {
            Some(map)
                if map
                    .values()
                    .all(|v| v.as_f64().is_some_and(|n| n.is_finite() && n >= 0.0)) =>
            {
                Ok(())
            }
            _ => bad("columnWidths", "an object of non-negative numbers"),
        },
        "columnCalculations" => match value.as_object() {
            Some(map) if map.values().all(Value::is_string) => Ok(()),
            _ => bad("columnCalculations", "an object of strings"),
        },
        "groupCalculations" => match value.as_object() {
            Some(map) if map.values().all(is_group_calculation) => Ok(()),
            _ => bad(
                "groupCalculations",
                "an object of {calculation, propertyId} entries",
            ),
        },
        "defaultTemplateId" => check_string("defaultTemplateId", value),
        "groupBy" => check_string("groupBy", value),
        "dateProperty" => check_string("dateProperty", value),
        "dependencyPropertyId" => check_string("dependencyPropertyId", value),
        "ganttZoom" => match value.as_str().and_then(|s| s.parse::<GanttZoom>().ok()) {
            Some(_) => Ok(()),
            None => bad("ganttZoom", "one of day, week or month"),
        },
        "order" => match value.as_str() {
            Some(order) if order.is_empty() || is_order(order) => Ok(()),
            _ => bad("order", "an order the server made"),
        },
        _ => Err(ViewFieldsError::UnknownField {
            field: name.to_owned(),
        }),
    }
}

fn check_string(field: &'static str, value: &Value) -> Result<(), ViewFieldsError> {
    if value.is_string() {
        Ok(())
    } else {
        Err(ViewFieldsError::InvalidField {
            field,
            expected: "a string",
        })
    }
}

fn check_string_list(field: &'static str, value: &Value) -> Result<(), ViewFieldsError> {
    if value
        .as_array()
        .is_some_and(|items| items.iter().all(Value::is_string))
    {
        Ok(())
    } else {
        Err(ViewFieldsError::InvalidField {
            field,
            expected: "a list of strings",
        })
    }
}

fn is_sort_option(value: &Value) -> bool {
    let Some(map) = value.as_object() else {
        return false;
    };
    map.len() == 2
        && map.get("propertyId").is_some_and(Value::is_string)
        && map.get("reversed").is_some_and(Value::is_boolean)
}

fn is_group_calculation(value: &Value) -> bool {
    let Some(map) = value.as_object() else {
        return false;
    };
    map.len() == 2
        && map.get("calculation").is_some_and(Value::is_string)
        && map.get("propertyId").is_some_and(Value::is_string)
}

fn is_filter_group(value: &Value, depth: usize) -> bool {
    if depth > MAX_FILTER_DEPTH {
        return false;
    }
    let Some(map) = value.as_object() else {
        return false;
    };
    if map.len() != 2 {
        return false;
    }
    let Some(op) = map.get("operation").and_then(Value::as_str) else {
        return false;
    };
    if !matches!(op, "and" | "or") {
        return false;
    }
    let Some(filters) = map.get("filters").and_then(Value::as_array) else {
        return false;
    };
    filters
        .iter()
        .all(|item| is_filter_clause(item) || is_filter_group(item, depth + 1))
}

fn is_filter_clause(value: &Value) -> bool {
    let Some(map) = value.as_object() else {
        return false;
    };
    if map.len() != 3 {
        return false;
    }
    let Some(_property_id) = map.get("propertyId").and_then(Value::as_str) else {
        return false;
    };
    let Some(condition) = map.get("condition").and_then(Value::as_str) else {
        return false;
    };
    if !FILTER_CONDITIONS.contains(&condition) {
        return false;
    }
    map.get("values")
        .and_then(Value::as_array)
        .is_some_and(|items| items.iter().all(Value::is_string))
}

/// Validates filter clause values against the board's `card_properties`, explicitly
/// allowing the dynamic `'me'` token on person properties and `'today'` on date
/// properties.
///
/// # Errors
///
/// Returns [`ViewFieldsError::InvalidField`] if a person or date filter clause carries
/// a non-empty value that is neither a valid target nor its allowed dynamic token.
pub fn validate_view_filter_values(
    fields: &JsonMap,
    definitions: &[CardPropertyDefinition],
) -> Result<(), ViewFieldsError> {
    let Some(filter) = fields.get("filter") else {
        return Ok(());
    };
    if filter.is_null() {
        return Ok(());
    }
    check_filter_values_node(filter, definitions)
}

fn check_filter_values_node(
    node: &Value,
    definitions: &[CardPropertyDefinition],
) -> Result<(), ViewFieldsError> {
    let Some(map) = node.as_object() else {
        return Ok(());
    };
    if let Some(filters) = map.get("filters").and_then(Value::as_array) {
        for child in filters {
            check_filter_values_node(child, definitions)?;
        }
        return Ok(());
    }
    let Some(property_id) = map.get("propertyId").and_then(Value::as_str) else {
        return Ok(());
    };
    let Some(definition) = definitions.iter().find(|def| def.id == property_id) else {
        return Ok(());
    };
    let Some(values) = map.get("values").and_then(Value::as_array) else {
        return Ok(());
    };
    if definition.property_type.names_people() {
        for item in values {
            if let Some(text) = item.as_str()
                && !text.is_empty()
                && text != FILTER_PERSON_ME
                && !is_id(text)
            {
                return Err(ViewFieldsError::InvalidField {
                    field: "filter",
                    expected: "person filter values must be user IDs or \"me\"",
                });
            }
        }
    } else if matches!(
        definition.property_type,
        PropertyType::Date | PropertyType::CreatedTime | PropertyType::UpdatedTime
    ) {
        for item in values {
            if let Some(text) = item.as_str()
                && !text.is_empty()
                && text != FILTER_DATE_TODAY
                && !is_calendar_date(text)
            {
                return Err(ViewFieldsError::InvalidField {
                    field: "filter",
                    expected: "date filter values must be calendar dates (YYYY-MM-DD) or \"today\"",
                });
            }
        }
    }
    Ok(())
}

/// Drops unknown or malformed members from a stored view's `fields` so legacy stored
/// rows can still be read, patched on other fields, or duplicated. Returns removed keys.
pub fn drop_malformed_view_fields(fields: &mut JsonMap) -> Vec<String> {
    let malformed: Vec<String> = fields
        .iter()
        .filter(|(name, value)| {
            !SHARED_VIEW_MEMBERS.contains(&name.as_str())
                || (!value.is_null() && check_view_member(name.as_str(), value).is_err())
        })
        .map(|(name, _)| name.clone())
        .collect();
    for key in &malformed {
        fields.remove(key);
    }
    malformed
}

/// Returns the relation property IDs read by any live Gantt view among `views`.
#[must_use]
pub fn gantt_dependency_property_ids<'a>(
    views: impl IntoIterator<Item = &'a JsonMap>,
    definitions: &'a [CardPropertyDefinition],
) -> HashSet<&'a str> {
    let relation_ids: HashSet<&str> = definitions
        .iter()
        .filter(|def| !def.id.is_empty() && def.property_type == PropertyType::Relation)
        .map(|def| def.id.as_str())
        .collect();
    let mut active = HashSet::new();
    for fields in views {
        if fields.get("viewType").and_then(Value::as_str) != Some("gantt") {
            continue;
        }
        if let Some(dep_id) = fields
            .get("dependencyPropertyId")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            && let Some(&matched) = relation_ids.get(dep_id)
        {
            active.insert(matched);
        }
    }
    active
}

/// Sorts board view blocks in display order: views with a valid `fields.order` first
/// (ordered by `(order, id)`), followed by unplaced views ordered by `(title, id)`.
pub fn sort_board_views(views: &mut [Block]) {
    views.sort_by(|a, b| {
        let a_order = view_order_of(&a.fields);
        let b_order = view_order_of(&b.fields);
        match (a_order, b_order) {
            (Some(ao), Some(bo)) => (ao, a.id.as_str()).cmp(&(bo, b.id.as_str())),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => {
                (a.title.as_str(), a.id.as_str()).cmp(&(b.title.as_str(), b.id.as_str()))
            }
        }
    });
}

/// Returns the valid fractional `order` string of a view's `fields`, if present.
#[must_use]
pub fn view_order_of(fields: &JsonMap) -> Option<&str> {
    fields
        .get(PAGE_ORDER_FIELD)
        .and_then(Value::as_str)
        .filter(|order| is_order(order))
}

/// Computes the fractional `order` for inserting or moving a view among `others`
/// (already sorted via [`sort_board_views`], excluding the moved view) at `index`
/// (`0..=others.len()`), returning `(target_order, companion_orders)` where
/// `companion_orders` maps sibling `view_id -> new_order` for any unplaced views or
/// ties/overflows that must be numbered in the same transaction.
#[must_use]
pub fn place_view_at(others: &[Block], index: usize) -> (String, BTreeMap<String, String>) {
    let to = index.min(others.len());
    let mut shown: Vec<String> = Vec::with_capacity(others.len());
    let mut last: Option<String> = None;
    let mut companions = BTreeMap::new();
    for item in others {
        let current = view_order_of(&item.fields).map(str::to_owned);
        let assigned = current.clone().unwrap_or_else(|| {
            order_between(last.as_deref(), None).unwrap_or_else(|_| "V".to_owned())
        });
        if current.is_none() {
            companions.insert(item.id.clone(), assigned.clone());
        }
        last = Some(assigned.clone());
        shown.push(assigned);
    }
    let before = to
        .checked_sub(1)
        .and_then(|i| shown.get(i))
        .map(String::as_str);
    let after = shown.get(to).map(String::as_str);
    if let Ok(place) = order_between(before, after)
        && place.len() <= VIEW_ORDER_MAX_CHARS
    {
        return (place, companions);
    }
    let spread = spread_view_orders(others.len() + 1);
    companions.clear();
    for (idx, item) in others.iter().enumerate() {
        let given = &spread[if idx < to { idx } else { idx + 1 }];
        if view_order_of(&item.fields) != Some(given.as_str()) {
            companions.insert(item.id.clone(), given.clone());
        }
    }
    (spread[to].clone(), companions)
}

/// Generates `count` evenly spaced fractional orders over `DIGITS`.
#[must_use]
pub fn spread_view_orders(count: usize) -> Vec<String> {
    if count == 0 {
        return Vec::new();
    }
    let base = u128::try_from(DIGITS.len()).unwrap_or(62);
    let slots = u128::try_from(count)
        .unwrap_or(u128::MAX - 1)
        .saturating_add(1);
    let mut width: u32 = 1;
    while base.saturating_pow(width) < slots.saturating_mul(2) {
        width += 1;
    }
    let span = base.saturating_pow(width);
    let width_usize = usize::try_from(width).unwrap_or(1);
    (0..count)
        .map(|idx| {
            let idx_u128 = u128::try_from(idx).unwrap_or(0);
            let mut value = (idx_u128.saturating_add(1) * span) / slots;
            let mut chars = vec![b'0'; width_usize];
            for pos in (0..width_usize).rev() {
                let digit = usize::try_from(value % base).unwrap_or(0);
                chars[pos] = DIGITS[digit];
                value /= base;
            }
            while chars.last() == Some(&b'0') && chars.len() > 1 {
                chars.pop();
            }
            String::from_utf8(chars).unwrap_or_else(|_| "V".to_owned())
        })
        .collect()
}

/// Request body for creating a board view (`POST /w/{ws}/boards/{boardId}/views`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct NewBoardView {
    /// Display title of the view.
    pub title: String,
    /// Layout kind (`board`, `table`, `gallery`, `calendar`, `gantt`).
    pub view_type: ViewType,
    /// Optional sibling view ID to place the new view immediately after (`""` for first,
    /// omitted or `null` for last).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    /// Optional initial `BoardViewFields` entries (`groupBy`, `shownProperties`, etc.).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Object>)]
    pub fields: Option<JsonMap>,
}

/// Request body for moving a board view (`POST .../views/{viewId}/move`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct BoardViewMove {
    /// Sibling view ID to place this view immediately after; `""` or `null` moves it to
    /// the first position.
    pub after: Option<String>,
}

/// Request body for duplicating a board view (`POST .../views/{viewId}/duplicate`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct BoardViewDuplicate {
    /// Optional custom title for the duplicated view; when omitted, a localized copy
    /// title is derived from the source view.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Optional sibling view ID to place the duplicate after; defaults to right after the
    /// source view.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
}

/// Response returned when creating, moving, or duplicating a board view.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BoardViewWrite {
    /// The created, moved, or duplicated view block.
    pub view: Block,
    /// All sibling views whose `order` was initialized or renumbered in the same
    /// transaction (empty when none needed renumbering).
    pub companion_views: Vec<Block>,
}
