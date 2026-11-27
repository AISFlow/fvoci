//! Units and chapters of a course: the tree nodes, a full unit or chapter as its
//! operations give and take it, and the limits of a course.
//!
//! A unit is a block of the type `unit` ([`crate::BlockType::Unit`]) and a chapter is a
//! block of the type `chapter` ([`crate::BlockType::Chapter`]), both on a course
//! ([`crate::ContainerKind::Course`]). A chapter is a title-only heading at the top of
//! its course with no body and no collaborative room; a unit may sit at the top of the
//! course or below a chapter, and carries an editable body. Both share the fractional
//! ordering field ([`PAGE_ORDER_FIELD`]) and tree placement of wiki pages.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::body::{Body, BodyChange};
use crate::course_schedule::{
    AmountUnit, FieldChange, UNIT_AMOUNT_FIELD, UNIT_AMOUNT_UNIT_FIELD, UNIT_SCHEDULED_DATE_FIELD,
    refuse_unit_amount, refuse_unit_scheduled_date, unit_amount, unit_amount_unit,
    unit_scheduled_date,
};
use crate::page::{PAGE_ORDER_FIELD, refuse_title, text_field};
use crate::refusal::{RefusalReason, Refusals};
use crate::{Block, BlockPatch, BlockType, JsonMap};

/// How many units a course may have, those in the trash not counted. Chapters do not
/// count toward this limit ([`CHAPTERS_PER_COURSE`]).
pub const UNITS_PER_COURSE: u32 = 2_000;

/// How many chapters a course may have, those in the trash not counted.
pub const CHAPTERS_PER_COURSE: u32 = 200;

/// How deep a course's tree may go: chapters and top-level units at depth 1, units
/// inside a chapter at depth 2.
pub const COURSE_DEPTH: u32 = 2;

/// How many units and chapters a course may hold, those in the trash not counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CourseLimits {
    /// The most units the course may have.
    pub units: u32,
    /// The most chapters the course may have.
    pub chapters: u32,
}

impl CourseLimits {
    /// The server's limits for a course.
    pub const DEFAULT: Self = Self {
        units: UNITS_PER_COURSE,
        chapters: CHAPTERS_PER_COURSE,
    };
}

impl Default for CourseLimits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Whether an entry of a course's outline is a unit (with a body) or a chapter (a
/// top-level title-only heading that groups units).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, utoipa::ToSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum UnitKind {
    /// A unit of study: may sit at the top of the course or below a chapter, and holds a
    /// body edited directly or together in its room.
    #[default]
    Unit,
    /// A chapter: a title-only heading at the top of the course that groups units below
    /// it. It has no body and no room.
    Chapter,
}

impl UnitKind {
    /// Returns the wire and database form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unit => "unit",
            Self::Chapter => "chapter",
        }
    }

    /// The block type stored for this kind.
    #[must_use]
    pub const fn block_type(self) -> BlockType {
        match self {
            Self::Unit => BlockType::Unit,
            Self::Chapter => BlockType::Chapter,
        }
    }

    /// The course entry kind of `block_type`, if it is one.
    #[must_use]
    pub fn of_block(block_type: &BlockType) -> Option<Self> {
        match block_type {
            BlockType::Unit => Some(Self::Unit),
            BlockType::Chapter => Some(Self::Chapter),
            _ => None,
        }
    }
}

impl std::fmt::Display for UnitKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error returned when a block is neither a unit nor a chapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("the block is neither a unit nor a chapter")]
pub struct NotUnitBlockError;

impl TryFrom<&BlockType> for UnitKind {
    type Error = NotUnitBlockError;

    fn try_from(block_type: &BlockType) -> Result<Self, Self::Error> {
        Self::of_block(block_type).ok_or(NotUnitBlockError)
    }
}

/// A unit or chapter as the outline of its course lists it: without its body.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct UnitNode {
    /// The unit's or chapter's identifier.
    pub id: String,
    /// The identifier of the chapter it is below; empty for a chapter and for a unit at
    /// the top of its course.
    pub parent_id: String,
    /// Whether this node is a `unit` or a `chapter`.
    pub kind: UnitKind,
    /// The unit's or chapter's title.
    pub title: String,
    /// Its place among the siblings of the same parent: listed by it as strings compare
    /// (by code unit, not by a locale), and by `id` where two have the same. The server
    /// makes it; a client reads it only to sort.
    pub order: String,
    /// Optional study amount (`1..=100_000`) for a unit; always `null` for a chapter.
    #[schema(minimum = 1, maximum = 100_000)]
    pub amount: Option<u32>,
    /// Optional unit of study amount (`minutes`, `pages`, or `problems`) for a unit;
    /// always `null` for a chapter.
    pub amount_unit: Option<AmountUnit>,
    /// Optional shared scheduled date (`YYYY-MM-DD`) for a unit; always `null` for a
    /// chapter.
    pub scheduled_date: Option<String>,
    /// Creation time in milliseconds since the Unix epoch.
    pub created_at: i64,
    /// Last modification time in milliseconds since the Unix epoch.
    pub updated_at: i64,
}

impl From<Block> for UnitNode {
    fn from(block: Block) -> Self {
        let kind = UnitKind::of_block(&block.block_type).unwrap_or_default();
        let (amount, amount_unit, scheduled_date) = if kind == UnitKind::Unit {
            (
                unit_amount(&block.fields),
                unit_amount_unit(&block.fields),
                unit_scheduled_date(&block.fields),
            )
        } else {
            (None, None, None)
        };
        Self {
            kind,
            order: text_field(&block.fields, PAGE_ORDER_FIELD),
            amount,
            amount_unit,
            scheduled_date,
            id: block.id,
            parent_id: block.parent_id,
            title: block.title,
            created_at: block.created_at,
            updated_at: block.updated_at,
        }
    }
}

/// Puts course nodes in the order their outline lists them in: by [`UnitNode::order`],
/// and by identifier where two have the same.
pub fn sort_units(units: &mut [UnitNode]) {
    units.sort_by(|one, other| (&one.order, &one.id).cmp(&(&other.order, &other.id)));
}

/// A unit or chapter of a course.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct Unit {
    /// The unit's or chapter's identifier.
    pub id: String,
    /// The identifier of the course it is of.
    pub course_id: String,
    /// The identifier of the chapter it is below; empty for a chapter and for a unit at
    /// the top of its course.
    pub parent_id: String,
    /// Whether this entry is a `unit` or a `chapter`.
    pub kind: UnitKind,
    /// The title of the unit or chapter, as its outline shows it.
    pub title: String,
    /// Its place among the siblings of the same parent, as in `getUnitTree`.
    pub order: String,
    /// Optional study amount (`1..=100_000`) for a unit; always `null` for a chapter.
    #[schema(minimum = 1, maximum = 100_000)]
    pub amount: Option<u32>,
    /// Optional unit of study amount (`minutes`, `pages`, or `problems`) for a unit;
    /// always `null` for a chapter.
    pub amount_unit: Option<AmountUnit>,
    /// Optional shared scheduled date (`YYYY-MM-DD`) for a unit; always `null` for a
    /// chapter.
    pub scheduled_date: Option<String>,
    /// The unit's body; `null` for a unit without one, and always `null` for a chapter.
    pub body: Option<Body>,
    /// The identifier of the user who made it.
    pub created_by: String,
    /// The identifier of the user who last changed it.
    pub modified_by: String,
    /// Creation time in milliseconds since the Unix epoch.
    pub created_at: i64,
    /// Last modification time in milliseconds since the Unix epoch: what `readAt` of a
    /// change of the unit names.
    pub updated_at: i64,
}

impl From<Block> for Unit {
    fn from(block: Block) -> Self {
        let kind = UnitKind::of_block(&block.block_type).unwrap_or_default();
        let (amount, amount_unit, scheduled_date) = if kind == UnitKind::Unit {
            (
                unit_amount(&block.fields),
                unit_amount_unit(&block.fields),
                unit_scheduled_date(&block.fields),
            )
        } else {
            (None, None, None)
        };
        Self {
            kind,
            order: text_field(&block.fields, PAGE_ORDER_FIELD),
            amount,
            amount_unit,
            scheduled_date,
            id: block.id,
            course_id: block.container_id,
            parent_id: block.parent_id,
            title: block.title,
            body: block.body,
            created_by: block.created_by,
            modified_by: block.modified_by,
            created_at: block.created_at,
            updated_at: block.updated_at,
        }
    }
}

/// A unit or chapter in the trash, at the top of what went with it, as the trash of a
/// course lists it: `restoreUnit` brings it back, and for a chapter brings back the
/// units deleted along with it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct DeletedUnit {
    /// The unit's or chapter's identifier.
    pub id: String,
    /// Whether this deleted entry is a `unit` or a `chapter`.
    pub kind: UnitKind,
    /// The title when it was deleted.
    pub title: String,
    /// The chapter it was below; empty for a chapter or a top-level unit. If that
    /// chapter is gone when the unit is restored, the unit comes back at the top of the
    /// course.
    pub parent_id: String,
    /// When it was deleted, in milliseconds since the Unix epoch.
    pub deleted_at: i64,
    /// Who deleted it, who may no longer exist; empty if not known.
    pub deleted_by: String,
    /// How many units below this chapter went with it and come back with it; zero for a
    /// unit.
    pub below: u32,
}

/// A unit or chapter to make in a course.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct NewUnit {
    /// Whether to make a `unit` or a `chapter`. Left out, `unit`.
    pub kind: UnitKind,
    /// The title. Left out, empty.
    pub title: String,
    /// The identifier of the chapter the unit goes below: a chapter of the same course.
    /// Left out or empty, it goes at the top of the course. A chapter must leave this
    /// empty (400 at `/parentId` otherwise).
    pub parent_id: String,
    /// The identifier of the sibling it goes right after: a sibling of the same parent.
    /// Empty for the first place; left out or `null`, the last place.
    pub after: Option<String>,
    /// Optional study amount (`1..=100_000`) for a unit; refused on a chapter (400 at
    /// `/amount`).
    #[schema(value_type = Option<u32>, minimum = 1, maximum = 100_000)]
    pub amount: Option<i64>,
    /// Optional unit of study amount (`minutes`, `pages`, or `problems`) for a unit;
    /// refused on a chapter (400 at `/amountUnit`).
    pub amount_unit: Option<AmountUnit>,
    /// Optional shared scheduled date (`YYYY-MM-DD`) for a unit; refused on a chapter
    /// (400 at `/scheduledDate`).
    pub scheduled_date: Option<String>,
}

/// What a chapter refuses when schedule or study-amount members are supplied.
const CHAPTER_SCHEDULE_FORBIDDEN: &str = "chapters cannot have a study amount or scheduled date";

impl NewUnit {
    /// Checks the shape of the request.
    ///
    /// # Errors
    ///
    /// Returns refusals for an overlong title, study-amount or schedule fields supplied on
    /// a chapter, an out-of-range `amount`, or a malformed `scheduledDate`.
    pub fn check_request(&self) -> Result<(), Refusals> {
        let mut refused = Refusals::new();
        refuse_title(&mut refused, Some(&self.title));
        if self.kind == UnitKind::Chapter {
            if self.amount.is_some() {
                refused.refuse("amount", RefusalReason::Invalid, CHAPTER_SCHEDULE_FORBIDDEN);
            }
            if self.amount_unit.is_some() {
                refused.refuse(
                    "amountUnit",
                    RefusalReason::Invalid,
                    CHAPTER_SCHEDULE_FORBIDDEN,
                );
            }
            if self.scheduled_date.is_some() {
                refused.refuse(
                    "scheduledDate",
                    RefusalReason::Invalid,
                    CHAPTER_SCHEDULE_FORBIDDEN,
                );
            }
        } else {
            refuse_unit_amount(&mut refused, self.amount);
            refuse_unit_scheduled_date(&mut refused, self.scheduled_date.as_deref());
        }
        refused.into_result()
    }

    /// Returns the unit-specific `fields` map populated from this request.
    ///
    /// Includes any supplied schedule/amount fields even when `self.kind` is a chapter so
    /// [`Block::validate`](crate::Block::validate) enforces `CHAPTER_FIELD_KINDS` at the
    /// store boundary if [`Self::check_request`] was bypassed.
    #[must_use]
    pub fn to_fields(&self) -> JsonMap {
        let mut fields = JsonMap::new();
        if let Some(amount) = self.amount {
            fields.insert(UNIT_AMOUNT_FIELD.to_owned(), Value::from(amount));
        }
        if let Some(amount_unit) = self.amount_unit {
            fields.insert(
                UNIT_AMOUNT_UNIT_FIELD.to_owned(),
                Value::from(amount_unit.as_str()),
            );
        }
        if let Some(scheduled_date) = &self.scheduled_date {
            fields.insert(
                UNIT_SCHEDULED_DATE_FIELD.to_owned(),
                Value::from(scheduled_date.clone()),
            );
        }
        fields
    }
}

/// Where a unit or chapter moves to, within its course. Moving a chapter keeps its units
/// below it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct UnitMove {
    /// The identifier of the chapter it goes below: a chapter of the same course. Left
    /// out or empty, it goes to the top of the course. A chapter must leave this empty
    /// (400 at `/parentId` otherwise).
    pub parent_id: String,
    /// The identifier of the sibling it goes right after: another sibling of that
    /// parent. Empty for the first place; left out or `null`, the last place.
    pub after: Option<String>,
}

/// What a new body without `readAt` is refused with.
const READ_AT_MISSING: &str =
    "a new body is sent with readAt, the updatedAt of the unit as it was read";

/// What a unit or chapter is changed to. Absent members stay as they are. Where the
/// node stands in the course does not change here (`moveUnit`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct UnitPatch {
    /// The new title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The new body of a unit; `null` takes the body away. Sent only with `readAt`. A
    /// chapter has no body (400 at `/body`).
    #[serde(skip_serializing_if = "BodyChange::is_keep")]
    #[schema(value_type = Option<Body>)]
    pub body: BodyChange,
    /// The `updatedAt` of the unit as the client read it: if the unit has changed since,
    /// the patch is refused with 409 and changes nothing. Needed with `body`, and
    /// checked whenever it is sent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_at: Option<i64>,
    /// The new study amount (`1..=100_000`) of a unit; `null` clears it. Refused on a
    /// chapter (400 at `/amount`).
    #[serde(skip_serializing_if = "FieldChange::is_keep")]
    #[schema(value_type = Option<u32>, minimum = 1, maximum = 100_000)]
    pub amount: FieldChange<i64>,
    /// The new unit of study amount (`minutes`, `pages`, or `problems`) of a unit; `null`
    /// clears it. Refused on a chapter (400 at `/amountUnit`).
    #[serde(skip_serializing_if = "FieldChange::is_keep")]
    #[schema(value_type = Option<AmountUnit>)]
    pub amount_unit: FieldChange<AmountUnit>,
    /// The new shared scheduled date (`YYYY-MM-DD`) of a unit; `null` clears it. Refused
    /// on a chapter (400 at `/scheduledDate`).
    #[serde(skip_serializing_if = "FieldChange::is_keep")]
    #[schema(value_type = Option<String>)]
    pub scheduled_date: FieldChange<String>,
}

impl UnitPatch {
    /// Checks the shape of the patch as a request sends it, assuming a unit target.
    ///
    /// # Errors
    ///
    /// Returns every member that is refused: an overlong title, a new body without
    /// `readAt`, an out-of-range `amount`, or a malformed `scheduledDate`.
    pub fn check_request(&self) -> Result<(), Refusals> {
        self.check_request_for(UnitKind::Unit)
    }

    /// Checks the shape of the patch against the target node's [`UnitKind`].
    ///
    /// # Errors
    ///
    /// Returns every refused member, including study-amount or schedule changes attempted
    /// on a chapter.
    pub fn check_request_for(&self, kind: UnitKind) -> Result<(), Refusals> {
        let mut refused = Refusals::new();
        if !self.body.is_keep() && self.read_at.is_none() {
            refused.refuse("readAt", RefusalReason::Required, READ_AT_MISSING);
        }
        refuse_title(&mut refused, self.title.as_deref());
        if kind == UnitKind::Chapter {
            if !self.amount.is_keep() {
                refused.refuse("amount", RefusalReason::Invalid, CHAPTER_SCHEDULE_FORBIDDEN);
            }
            if !self.amount_unit.is_keep() {
                refused.refuse(
                    "amountUnit",
                    RefusalReason::Invalid,
                    CHAPTER_SCHEDULE_FORBIDDEN,
                );
            }
            if !self.scheduled_date.is_keep() {
                refused.refuse(
                    "scheduledDate",
                    RefusalReason::Invalid,
                    CHAPTER_SCHEDULE_FORBIDDEN,
                );
            }
        } else {
            if let FieldChange::Set(amount) = self.amount {
                refuse_unit_amount(&mut refused, Some(amount));
            }
            if let FieldChange::Set(ref date) = self.scheduled_date {
                refuse_unit_scheduled_date(&mut refused, Some(date));
            }
        }
        refused.into_result()
    }

    /// The patch of the underlying block that does what this patch says.
    #[must_use]
    pub fn into_block_patch(self) -> BlockPatch {
        let mut patch = BlockPatch {
            title: self.title,
            body: self.body,
            read_at: self.read_at,
            ..BlockPatch::default()
        };
        match self.amount {
            FieldChange::Keep => {}
            FieldChange::Clear => patch.deleted_fields.push(UNIT_AMOUNT_FIELD.to_owned()),
            FieldChange::Set(amount) => {
                patch
                    .updated_fields
                    .insert(UNIT_AMOUNT_FIELD.to_owned(), Value::from(amount));
            }
        }
        match self.amount_unit {
            FieldChange::Keep => {}
            FieldChange::Clear => patch.deleted_fields.push(UNIT_AMOUNT_UNIT_FIELD.to_owned()),
            FieldChange::Set(unit) => {
                patch.updated_fields.insert(
                    UNIT_AMOUNT_UNIT_FIELD.to_owned(),
                    Value::from(unit.as_str()),
                );
            }
        }
        match self.scheduled_date {
            FieldChange::Keep => {}
            FieldChange::Clear => patch
                .deleted_fields
                .push(UNIT_SCHEDULED_DATE_FIELD.to_owned()),
            FieldChange::Set(date) => {
                patch
                    .updated_fields
                    .insert(UNIT_SCHEDULED_DATE_FIELD.to_owned(), Value::from(date));
            }
        }
        patch
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::{BLOCK_TITLE_MAX_CHARS, BlockError};

    #[test]
    fn unit_has_a_room_and_chapter_has_no_body_and_no_room() {
        let unit: BlockType = "unit".parse().expect("known type");
        let chapter: BlockType = "chapter".parse().expect("known type");
        assert_eq!(unit, BlockType::Unit);
        assert_eq!(chapter, BlockType::Chapter);
        assert_eq!(unit.as_str(), "unit");
        assert_eq!(chapter.as_str(), "chapter");
        assert!(unit.has_body() && unit.has_room() && !unit.is_current() && unit.takes_comments());
        assert!(
            !chapter.has_body()
                && !chapter.has_room()
                && !chapter.is_current()
                && !chapter.takes_comments()
        );
        assert_eq!(
            BlockError::type_not_current(&unit),
            BlockError::UnitByBlocks
        );
        assert_eq!(
            BlockError::type_not_current(&chapter),
            BlockError::UnitByBlocks
        );
    }

    #[test]
    fn unit_and_chapter_validate_order_and_chapter_refuses_body() {
        let with = |block_type: BlockType, field: &str, value: Value| {
            let mut block = Block {
                container_id: "c".to_owned(),
                block_type,
                ..Block::default()
            };
            block.fields.insert(field.to_owned(), value);
            block.validate()
        };
        for block_type in [BlockType::Unit, BlockType::Chapter] {
            assert_eq!(with(block_type, PAGE_ORDER_FIELD, json!("V1")), Ok(()));
            assert_eq!(with(block_type, PAGE_ORDER_FIELD, json!(null)), Ok(()));
            for bad in [json!(5), json!(""), json!("V0"), json!("a b")] {
                assert!(
                    matches!(
                        with(block_type, PAGE_ORDER_FIELD, bad.clone()),
                        Err(BlockError::InvalidUnitField { field, .. }) if field == PAGE_ORDER_FIELD
                    ),
                    "{block_type}: {bad}"
                );
            }
        }
        let chapter_with_body = Block {
            container_id: "c".to_owned(),
            block_type: BlockType::Chapter,
            body: Some(Body::from_stored(json!({"type": "doc", "content": []}))),
            ..Block::default()
        };
        assert_eq!(
            chapter_with_body.validate(),
            Err(BlockError::BodyNotAllowed)
        );
    }

    #[test]
    fn units_are_converted_sorted_and_patched_through_blocks() {
        let mut block = Block {
            id: "u".to_owned(),
            container_id: "c".to_owned(),
            parent_id: "ch".to_owned(),
            block_type: BlockType::Unit,
            title: "Lesson 1".to_owned(),
            created_by: "maker".to_owned(),
            modified_by: "editor".to_owned(),
            created_at: 1,
            updated_at: 2,
            ..Block::default()
        };
        block.fields.insert(PAGE_ORDER_FIELD.to_owned(), json!("V"));
        let node = UnitNode::from(block.clone());
        let unit = Unit::from(block);
        assert_eq!(
            (
                unit.course_id.as_str(),
                unit.parent_id.as_str(),
                unit.kind,
                unit.order.as_str()
            ),
            ("c", "ch", UnitKind::Unit, "V")
        );
        assert_eq!(
            (node.id, node.kind, node.order),
            (unit.id, unit.kind, unit.order)
        );

        let long = "t".repeat(BLOCK_TITLE_MAX_CHARS + 1);
        let refused = UnitPatch {
            title: Some(long.clone()),
            body: BodyChange::Clear,
            ..UnitPatch::default()
        }
        .check_request()
        .expect_err("refused");
        assert_eq!(refused.len(), 2, "{refused}");
        let refused = NewUnit {
            title: long,
            ..NewUnit::default()
        }
        .check_request()
        .expect_err("refused");
        assert_eq!(refused.len(), 1, "{refused}");

        let node = |id: &str, order: &str| UnitNode {
            id: id.to_owned(),
            order: order.to_owned(),
            ..UnitNode::default()
        };
        let mut units = [node("b", "V"), node("c", "1"), node("a", "V")];
        sort_units(&mut units);
        let ids: Vec<&str> = units.iter().map(|one| one.id.as_str()).collect();
        assert_eq!(ids, ["c", "a", "b"]);
    }
}
