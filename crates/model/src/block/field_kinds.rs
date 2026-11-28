//! The fields of blocks whose value has one kind, by the type of the block: what
//! [`Block::validate`](super::Block::validate) asks of the fields of a card, a page, an
//! entry, a unit and a chapter, and what a card's reading drops.

use serde_json::Value;

use crate::course_schedule::{
    UNIT_AMOUNT_FIELD, UNIT_AMOUNT_UNIT_FIELD, UNIT_SCHEDULED_DATE_FIELD, is_amount_unit_value,
    is_unit_amount_value,
};
use crate::diary::{
    ENTRY_BODY_UPDATED_AT_FIELD, ENTRY_DATE_FIELD, ENTRY_MOOD_FIELD, ENTRY_PLACE_FIELD,
    ENTRY_WEATHER_FIELD, is_body_updated_at_value, is_date_value, is_mood_value, is_note_value,
};
use crate::page::{PAGE_ICON_FIELD, PAGE_ORDER_FIELD, is_order_value};
use crate::recurrence::{is_recurred_to_value, is_recurrence_value};

/// The fields of a unit whose value has one kind.
pub(super) const UNIT_FIELD_KINDS: [FieldKind; 4] = [
    (PAGE_ORDER_FIELD, "an order the server made", is_order_value),
    (
        UNIT_AMOUNT_FIELD,
        "an integer from 1 to 100000",
        is_unit_amount_value,
    ),
    (
        UNIT_AMOUNT_UNIT_FIELD,
        "one of minutes, pages, or problems",
        is_amount_unit_value,
    ),
    (
        UNIT_SCHEDULED_DATE_FIELD,
        "a calendar date as YYYY-MM-DD",
        is_date_value,
    ),
];

/// The fields of a chapter: `order` only; study amount and scheduled date are forbidden.
pub(super) const CHAPTER_FIELD_KINDS: [FieldKind; 4] = [
    (PAGE_ORDER_FIELD, "an order the server made", is_order_value),
    (UNIT_AMOUNT_FIELD, "absent on a chapter", |_| false),
    (UNIT_AMOUNT_UNIT_FIELD, "absent on a chapter", |_| false),
    (UNIT_SCHEDULED_DATE_FIELD, "absent on a chapter", |_| false),
];

/// The fields of an entry whose value has one kind, as [`CARD_FIELD_KINDS`] has a
/// card's. Its `date` is there besides, which [`Block::validate`](super::Block::validate) asks for by itself: an
/// entry is not without it.
pub(super) const ENTRY_FIELD_KINDS: [FieldKind; 5] = [
    (ENTRY_DATE_FIELD, "a day, as `2026-11-21`", is_date_value),
    (ENTRY_MOOD_FIELD, "a mood", is_mood_value),
    (
        ENTRY_WEATHER_FIELD,
        "a text of at most 100 characters",
        is_note_value,
    ),
    (
        ENTRY_PLACE_FIELD,
        "a text of at most 100 characters",
        is_note_value,
    ),
    (
        ENTRY_BODY_UPDATED_AT_FIELD,
        "a time in milliseconds",
        is_body_updated_at_value,
    ),
];

/// The fields of a page whose value has one kind, as [`CARD_FIELD_KINDS`] has a card's.
pub(super) const PAGE_FIELD_KINDS: [FieldKind; 2] = [
    (PAGE_ICON_FIELD, "a string", Value::is_string),
    (PAGE_ORDER_FIELD, "an order the server made", is_order_value),
];

/// One rule of [`CARD_FIELD_KINDS`] or [`PAGE_FIELD_KINDS`]: the field, its kind as a
/// message names it, and whether a value is of that kind.
pub(super) type FieldKind = (&'static str, &'static str, fn(&Value) -> bool);

/// The fields of a card whose value has one kind. Anything else breaks the reading of
/// the card ([`crate::Card::from_block`]). `null` is taken as absent and is of every
/// kind.
pub(super) const CARD_FIELD_KINDS: [FieldKind; 4] = [
    ("icon", "a string", Value::is_string),
    ("isTemplate", "a boolean", Value::is_boolean),
    ("recurrence", "a recurrence", is_recurrence_value),
    ("recurredTo", "a card identifier", is_recurred_to_value),
];

/// Whether `value` may be the value of the card field `field` ([`CARD_FIELD_KINDS`]).
pub(super) fn is_card_field_of_its_kind(field: &str, value: &Value) -> bool {
    value.is_null()
        || CARD_FIELD_KINDS
            .iter()
            .find(|(name, _, _)| *name == field)
            .is_none_or(|(_, _, is_of_kind)| is_of_kind(value))
}
