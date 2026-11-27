//! Blocks: the basic unit of board content (cards, views, comments and attachments),
//! of a wiki's (pages), of a diary's (entries), and of a course's (chapters and units).

use std::collections::HashSet;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::JsonMap;
use crate::body::{Body, BodyChange};
use crate::card_property::{CardPropertyError, PersonProperties, validate_block_properties};
use crate::course_schedule::{
    UNIT_AMOUNT_FIELD, UNIT_AMOUNT_UNIT_FIELD, UNIT_SCHEDULED_DATE_FIELD, is_amount_unit_value,
    is_unit_amount_value,
};
use crate::diary::{
    ENTRY_BODY_UPDATED_AT_FIELD, ENTRY_DATE_FIELD, ENTRY_MOOD_FIELD, ENTRY_PLACE_FIELD,
    ENTRY_WEATHER_FIELD, is_body_updated_at_value, is_date_value, is_mood_value, is_note_value,
};
use crate::page::{PAGE_ICON_FIELD, PAGE_ORDER_FIELD, is_order_value};
use crate::relation::RelationError;
use crate::view_fields::{ViewFieldsError, drop_malformed_view_fields, validate_view_fields};

/// Maximum number of Unicode scalar values allowed in a block title (`16_383`).
pub const BLOCK_TITLE_MAX_CHARS: usize = 16_383;

/// Maximum number of characters in a view's `fields` once serialized to JSON.
pub const BLOCK_FIELDS_MAX_CHARS: usize = 800_000;

/// Maximum number of characters in the `fields` of a card, a comment or an attachment
/// once serialized to JSON. A card's content is its body.
pub const CARD_FIELDS_MAX_CHARS: usize = 64_000;

/// The member of a comment's `fields` that names the personal API token an agent wrote
/// the comment's text through: the server sets it as the text is written, by an agent or
/// by a person (who leaves none), whatever the writer sent, and the comment shows its
/// author's agent by that name.
pub const COMMENT_AGENT_TOKEN_NAME: &str = "agentTokenName";

/// The type of a block.
///
/// A block of a board is `card`, `view`, `comment` or `attachment`, a wiki has `page`
/// and `comment`, a diary has `entry`, and a course has `chapter`, `unit` and `comment`.
// See `BlockType::is_current`, `crate::Page`, `crate::DiaryEntry`, `crate::Unit`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, utoipa::ToSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum BlockType {
    /// A card of a board, with the values of its properties and a body.
    #[default]
    Card,
    /// A board view.
    View,
    /// A comment on a card, a page or a unit.
    // See `BlockType::takes_comments`.
    Comment,
    /// A file attachment on a card.
    Attachment,
    /// A page of a wiki.
    // See `crate::Page`.
    Page,
    /// The entry of a day of a diary.
    // See `crate::DiaryEntry`.
    Entry,
    /// A unit of a course.
    // See `crate::Unit`.
    Unit,
    /// A chapter of a course: a title-only heading at the top of the course with no body
    /// and no room.
    // See `crate::Unit`.
    Chapter,
}

impl BlockType {
    /// Every block type, in contract order.
    pub const ALL: [Self; 8] = [
        Self::Card,
        Self::View,
        Self::Comment,
        Self::Attachment,
        Self::Page,
        Self::Entry,
        Self::Unit,
        Self::Chapter,
    ];

    /// The wire words in the order of [`Self::ALL`].
    pub const WORDS: &[&str] = &[
        "card",
        "view",
        "comment",
        "attachment",
        "page",
        "entry",
        "unit",
        "chapter",
    ];

    /// The types whose body is edited together, in a room of the block's own: a card,
    /// a page, an entry and a unit. A comment's body is written whole by its author, and
    /// a chapter has no body.
    pub const WITH_ROOM: [Self; 4] = [Self::Card, Self::Page, Self::Entry, Self::Unit];

    /// Returns `true` for the types that may have a body: a card and a comment, a page, an
    /// entry and a unit.
    #[must_use]
    pub const fn has_body(self) -> bool {
        matches!(
            self,
            Self::Card | Self::Comment | Self::Page | Self::Entry | Self::Unit
        )
    }

    /// Checks that a block of this type may have `body`: only a type that has a body
    /// ([`BlockType::has_body`]) has one, and a comment's has no file block.
    ///
    /// # Errors
    ///
    /// Returns [`BlockError::BodyNotAllowed`] for a body of a type without one,
    /// [`BlockError::FileBlockInComment`] for a comment's body with a file block, and
    /// [`BlockError::BodyTooLarge`] if the serialized body exceeds
    /// [`crate::BODY_MAX_BYTES`].
    pub fn check_body(self, body: Option<&Body>) -> Result<(), BlockError> {
        let Some(body) = body else {
            return Ok(());
        };
        if !self.has_body() {
            return Err(BlockError::BodyNotAllowed);
        }
        if self == Self::Comment && body.has_file_blocks() {
            return Err(BlockError::FileBlockInComment);
        }
        body.check_size().map_err(|_| BlockError::BodyTooLarge)
    }

    /// Returns `true` for the types a comment may be on: a card of a board, a page of
    /// a wiki and a unit of a course. A diary entry and a chapter take none.
    #[must_use]
    pub const fn takes_comments(self) -> bool {
        matches!(self, Self::Card | Self::Page | Self::Unit)
    }

    /// Returns `true` for the types whose body has a room ([`Self::WITH_ROOM`]).
    #[must_use]
    pub fn has_room(self) -> bool {
        Self::WITH_ROOM.contains(&self)
    }

    /// Returns `true` for the types a block may be given by a write of blocks: a card,
    /// a view, a comment and an attachment. A page is no such
    /// type: the operations of pages make and change it, which keep it in its place in
    /// the tree of its wiki. Nor is an entry: the operations of diaries make it, which
    /// keep a day to one. Nor is a unit or chapter: the operations of courses make it.
    #[must_use]
    pub const fn is_current(self) -> bool {
        matches!(
            self,
            Self::Card | Self::View | Self::Comment | Self::Attachment
        )
    }

    /// Returns the maximum number of characters in the serialized `fields` of a block
    /// of this type.
    #[must_use]
    pub const fn fields_max_chars(self) -> usize {
        match self {
            Self::Card
            | Self::Comment
            | Self::Attachment
            | Self::Page
            | Self::Entry
            | Self::Unit
            | Self::Chapter => CARD_FIELDS_MAX_CHARS,
            Self::View => BLOCK_FIELDS_MAX_CHARS,
        }
    }

    /// Returns the wire and database form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Card => "card",
            Self::View => "view",
            Self::Comment => "comment",
            Self::Attachment => "attachment",
            Self::Page => "page",
            Self::Entry => "entry",
            Self::Unit => "unit",
            Self::Chapter => "chapter",
        }
    }

    /// Maps an exact wire string to a known type, if it is one.
    fn known(name: &str) -> Option<Self> {
        Some(match name {
            "card" => Self::Card,
            "view" => Self::View,
            "comment" => Self::Comment,
            "attachment" => Self::Attachment,
            "page" => Self::Page,
            "entry" => Self::Entry,
            "unit" => Self::Unit,
            "chapter" => Self::Chapter,
            _ => return None,
        })
    }
}

impl fmt::Display for BlockType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error returned when a string names no block type the server recognizes. It does not
/// say the string: that is whatever a client wrote.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("the text names no block type")]
pub struct ParseBlockTypeError(());

impl FromStr for BlockType {
    type Err = ParseBlockTypeError;

    /// Parses one of the known block types, written exactly as [`BlockType::as_str`]
    /// writes it: `Card` is no type.
    fn from_str(name: &str) -> Result<Self, Self::Err> {
        Self::known(name).ok_or(ParseBlockTypeError(()))
    }
}

/// Reasons a [`Block`] is rejected before it is stored.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BlockError {
    /// The block names no board.
    #[error("the block names no board")]
    EmptyContainerId,
    /// The block's parent is not a block of the block's board.
    #[error("the parent is not a block of the board")]
    ParentNotOnContainer,
    /// The block's new parent is the block itself or a block below it.
    #[error("the parent is the block itself or below it")]
    ParentBelowBlock,
    /// A comment that is new, newly a comment or moved is not below a card, a page or a
    /// unit of its container ([`BlockType::takes_comments`]).
    #[error(
        "the parent of a comment is a card of the board, a page of the wiki or a unit of the course"
    )]
    CommentNotOnCardOrPage,
    /// A block that is new or moved is below a comment: a comment has no blocks below
    /// it.
    #[error("a comment is the parent of no block")]
    ParentIsComment,
    /// A write attempts to change the type of an existing block.
    #[error("a block's type does not change")]
    TypeChanges,
    /// The title is longer than [`BLOCK_TITLE_MAX_CHARS`].
    #[error("block title size limit exceeded")]
    TitleSizeLimitExceeded,
    /// The serialized fields are longer than the limit of the block's type
    /// ([`BlockType::fields_max_chars`]).
    #[error("block fields size limit exceeded")]
    FieldsSizeLimitExceeded,
    /// A new block, or a patch, gives a type that no new block has
    /// ([`BlockType::is_current`]); see [`BlockError::type_not_current`].
    #[error(
        "a block is a `card`, a `view`, a `comment` or an `attachment`: a card's content is its body"
    )]
    TypeNotCurrent,
    /// The `properties` among the fields are not well-formed card property values.
    #[error(transparent)]
    InvalidCardProperties(#[from] CardPropertyError),
    /// A card relation value names what it may not name
    /// ([`crate::check_relation_value`]), or a block that is no card of the board.
    #[error(transparent)]
    InvalidRelation(#[from] RelationError),
    /// A block that is no card, comment, page, entry or unit has a body.
    #[error("only a card, a comment, a page, an entry or a unit has a body")]
    BodyNotAllowed,
    /// An image or a file block of the body that the write makes the block show newly
    /// names no file that the block's board or wiki has when the write is stored.
    #[error("an image or a file block of the body names no file of its board or wiki")]
    BodyFileNotOfContainer,
    /// A comment's body has a file block: files go in the body of a card or a page.
    #[error("a comment's body has no file block")]
    FileBlockInComment,
    /// The serialized body exceeds [`crate::BODY_MAX_BYTES`].
    #[error("{}", crate::BodyError::TooLarge)]
    BodyTooLarge,
    /// The title holds U+0000, which PostgreSQL cannot store in text.
    #[error("the title holds the character U+0000")]
    NulInTitle,
    /// A field of a card has a value of the wrong kind: `icon` a string, `isTemplate` a
    /// boolean (or either `null`).
    #[error("the field {field} of a card must be {expected}")]
    InvalidCardField {
        /// The field, by its name in the JSON: `icon` or `isTemplate`.
        field: &'static str,
        /// What its value must be.
        expected: &'static str,
    },
    /// A field of a page has a value of the wrong kind: `icon` a string, `order` an
    /// order ([`crate::order_between`]), or either `null`.
    #[error("the field {field} of a page must be {expected}")]
    InvalidPageField {
        /// The field, by its name in the JSON: `icon` or `order`.
        field: &'static str,
        /// What its value must be.
        expected: &'static str,
    },
    /// A write of blocks gives the type of a card on `/boards/{boardId}/blocks`: cards
    /// are created through `createCard`.
    #[error("a card is made by createCard")]
    CardByBlocks,
    /// A write of blocks gives the type of a page, as the type of a new block or of a
    /// patch: see [`BlockError::type_not_current`].
    #[error("a page is made and changed by the operations of pages")]
    PageByBlocks,
    /// A page is stored on a board: a page is a block of a wiki.
    #[error("a page is a block of a wiki, not of a board")]
    PageOffWiki,
    /// A block that is no page is stored on a wiki, and no comment on a page of it: a
    /// wiki has pages and the comments on them only.
    #[error("the blocks of a wiki are pages and the comments on them")]
    NotPageOnWiki,
    /// The parent of a page is no page: a page is at the top of its wiki or below a
    /// page of it.
    #[error("the parent of a page is a page of its wiki")]
    PageNotBelowPage,
    /// The page a new page is to go right after is no page of the same parent.
    #[error("the page to go after is no page of the same parent")]
    AfterNotSibling,
    /// A page would be deeper in its wiki than the limit: a page at the top is at
    /// depth 1 ([`crate::PageLimits`]).
    #[error("the page would be deeper in its wiki than {limit}")]
    PageTooDeep {
        /// The deepest a page may be.
        limit: u32,
    },
    /// The wiki would have more pages than it may, those in the trash not counted.
    #[error("the wiki would have more than {limit} pages")]
    TooManyPages {
        /// The most pages the wiki may have.
        limit: u32,
    },
    /// A field of an entry is missing or has a value it may not have: `date` a day
    /// ([`crate::DiaryDate`]), `mood` a mood ([`crate::Mood`]), `weather` and `place`
    /// short texts, and the title the day as `date` has it.
    #[error("the field {field} of an entry must be {expected}")]
    InvalidEntryField {
        /// The field, by its name in the JSON: `date`, `mood`, `weather`, `place` or
        /// `title`.
        field: &'static str,
        /// What its value must be.
        expected: &'static str,
    },
    /// A write of blocks gives the type of an entry, as the type of a new block or of a
    /// patch: see [`BlockError::type_not_current`].
    #[error("an entry is made and changed by the operations of diaries")]
    EntryByBlocks,
    /// An entry is stored on a board or a wiki: an entry is a block of a diary.
    #[error("an entry is a block of a diary")]
    EntryOffDiary,
    /// A block that is no entry is stored on a diary: a diary has its entries only.
    #[error("the blocks of a diary are its entries")]
    NotEntryOnDiary,
    /// An entry is stored below a block: an entry is directly on its diary.
    #[error("an entry is below no block")]
    EntryBelowBlock,
    /// An entry is stored with another day than it was made with.
    #[error("the day of an entry does not change")]
    EntryDateChanges,
    /// An entry is stored, or comes back from the trash, on a day of its diary that has
    /// an entry: a diary has one entry a day.
    #[error("the diary has an entry of {date} already: a day has one entry")]
    DayTaken {
        /// The day, as an entry has it.
        date: String,
    },
    /// The diary would have more entries than it may, those in the trash not counted.
    #[error("the diary would have more than {limit} entries")]
    TooManyEntries {
        /// The most entries the diary may have.
        limit: u32,
    },
    /// A field of a unit or chapter has a value of the wrong kind: `order` must be an
    /// order ([`crate::order_between`]) or `null`.
    #[error("the field {field} of a unit must be {expected}")]
    InvalidUnitField {
        /// The field, by its name in the JSON: `order`.
        field: &'static str,
        /// What its value must be.
        expected: &'static str,
    },
    /// A write of blocks gives the type of a unit or chapter: see
    /// [`BlockError::type_not_current`].
    #[error("a unit or a chapter is made and changed by the operations of courses")]
    UnitByBlocks,
    /// A unit or chapter is stored on a container that is no course.
    #[error("a unit or a chapter is a block of a course")]
    UnitOffCourse,
    /// A block that is neither a chapter, a unit nor a comment on a unit is stored on a
    /// course.
    #[error("the blocks of a course are its chapters, its units and the comments on its units")]
    NotUnitOnCourse,
    /// A unit is stored below a block that is no chapter of its course.
    #[error("the parent of a unit is a chapter of its course")]
    UnitParentNotChapter,
    /// A chapter is stored below another block: a chapter sits only at the top of its
    /// course.
    #[error("a chapter is below no block")]
    ChapterBelowBlock,
    /// The course would have more units than it may, those in the trash not counted.
    #[error("the course would have more than {limit} units")]
    TooManyUnits {
        /// The most units the course may have.
        limit: u32,
    },
    /// The parent of a card is no ordinary card of its board.
    #[error("the parent of a card is a card of its board")]
    CardNotBelowCard,
    /// A card is placed below a template card.
    #[error("a card template cannot be the parent of a card")]
    CardBelowTemplate,
    /// A template card is given a parent card.
    #[error("a card template cannot have a parent card")]
    TemplateCardBelowBlock,
    /// A card that has sub-cards is turned into a template card.
    #[error("a card with sub-cards cannot become a template")]
    TemplateCardHasChildren,
    /// A card, or a sub-card below it, would be deeper on its board than the limit.
    #[error("the card would be deeper on its board than {limit}")]
    CardTooDeep {
        /// The deepest a card may be.
        limit: u32,
    },
    /// The course would have more chapters than it may, those in the trash not counted.
    #[error("the course would have more than {limit} chapters")]
    TooManyChapters {
        /// The most chapters the course may have.
        limit: u32,
    },
    /// A view block carries an unknown or malformed member in `fields`.
    #[error(transparent)]
    InvalidViewField(#[from] ViewFieldsError),
    /// A view block is placed below another block instead of at the top of its board.
    #[error("a view is below no block")]
    ViewBelowBlock,
    /// Another block is placed below a view block.
    #[error("a view is the parent of no block")]
    ParentIsView,
    /// An attachment block is placed on a board without a live card parent.
    #[error("the parent of an attachment is a card of its board")]
    AttachmentNotOnCard,
    /// Another block is placed below an attachment block.
    #[error("an attachment is the parent of no block")]
    ParentIsAttachment,
}

/// The fields of a unit whose value has one kind.
const UNIT_FIELD_KINDS: [FieldKind; 4] = [
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
const CHAPTER_FIELD_KINDS: [FieldKind; 4] = [
    (PAGE_ORDER_FIELD, "an order the server made", is_order_value),
    (UNIT_AMOUNT_FIELD, "absent on a chapter", |_| false),
    (UNIT_AMOUNT_UNIT_FIELD, "absent on a chapter", |_| false),
    (UNIT_SCHEDULED_DATE_FIELD, "absent on a chapter", |_| false),
];

/// The fields of an entry whose value has one kind, as [`CARD_FIELD_KINDS`] has a
/// card's. Its `date` is there besides, which [`Block::validate`] asks for by itself: an
/// entry is not without it.
const ENTRY_FIELD_KINDS: [FieldKind; 5] = [
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
const PAGE_FIELD_KINDS: [FieldKind; 2] = [
    (PAGE_ICON_FIELD, "a string", Value::is_string),
    (PAGE_ORDER_FIELD, "an order the server made", is_order_value),
];

/// One rule of [`CARD_FIELD_KINDS`] or [`PAGE_FIELD_KINDS`]: the field, its kind as a
/// message names it, and whether a value is of that kind.
type FieldKind = (&'static str, &'static str, fn(&Value) -> bool);

/// The fields of a card whose value has one kind. Anything else breaks the reading of
/// the card ([`crate::Card::from_block`]). `null` is taken as absent and is of every
/// kind.
const CARD_FIELD_KINDS: [FieldKind; 2] = [
    ("icon", "a string", Value::is_string),
    ("isTemplate", "a boolean", Value::is_boolean),
];

/// Whether `value` may be the value of the card field `field` ([`CARD_FIELD_KINDS`]).
fn is_card_field_of_its_kind(field: &str, value: &Value) -> bool {
    value.is_null()
        || CARD_FIELD_KINDS
            .iter()
            .find(|(name, _, _)| *name == field)
            .is_none_or(|(_, _, is_of_kind)| is_of_kind(value))
}

impl BlockError {
    /// The refusal of `block_type` as the type of a new block or of a patch.
    #[must_use]
    pub fn type_not_current(block_type: &BlockType) -> Self {
        match block_type {
            BlockType::Page => Self::PageByBlocks,
            BlockType::Entry => Self::EntryByBlocks,
            BlockType::Unit | BlockType::Chapter => Self::UnitByBlocks,
            _ => Self::TypeNotCurrent,
        }
    }
}

/// The basic data unit of a board.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Block {
    /// The block's identifier.
    #[serde(default)]
    pub id: String,
    /// The identifier of the parent block; empty for root blocks.
    #[serde(default)]
    pub parent_id: String,
    /// The identifier of the user who created the block.
    #[serde(default)]
    pub created_by: String,
    /// The identifier of the user who last modified the block.
    #[serde(default)]
    pub modified_by: String,
    /// The block type: `card`, `view`, `comment` or `attachment` on a board, `page`
    /// and `comment` (on a page) on a wiki, `entry` on a diary, and `chapter`, `unit`
    /// and `comment` (on a unit) on a course. No request of blocks makes a block of
    /// another type or patches one into it (400 naming `type`), nor a page, an entry,
    /// a unit or a chapter: `createPage` makes pages, `createPageComment` the comments
    /// of a wiki, `createDiaryEntry` entries, and `createUnit` chapters and units.
    #[serde(rename = "type")]
    pub block_type: BlockType,
    /// The display title. A title with U+0000 is refused (400 naming `title`).
    #[serde(default)]
    pub title: String,
    /// Type-specific data, free-form: serialized, at most 64,000 characters for a card, a
    /// comment, an attachment, a page, an entry, a unit or a chapter and 800,000 for a
    /// view. The `icon` of a card is a string and its `isTemplate` a boolean, or either
    /// `null` (400 naming `fields` otherwise). A page has its `icon` and its `order`
    /// here, as `Page` gives them, an entry its `date`, `mood`, `weather` and `place`,
    /// as `DiaryEntry` gives them, and a unit or chapter its `order`, as `Unit` gives it.
    #[serde(default)]
    #[schema(value_type = Object)]
    pub fields: JsonMap,
    /// The body of a card, a comment, a page, an entry or a unit; `null` for a block
    /// without one. Only those have one: a body on another block is refused (400 naming
    /// `body`). Every image of it shows a file of the block's board. Whoever reads it by
    /// the board's link gets every mention with an empty `userId`: a shape that is only
    /// sent, which a write would have refused. A change message (`blockChanged`) of a
    /// card, a page, an entry or a unit has it `null`, with a summary of it beside the
    /// block.
    #[serde(default)]
    pub body: Option<Body>,
    /// Creation time in milliseconds since the Unix epoch.
    #[serde(default)]
    pub created_at: i64,
    /// Last modification time in milliseconds since the Unix epoch.
    #[serde(default)]
    pub updated_at: i64,
    /// Deletion time in milliseconds since the Unix epoch; absent while not deleted.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::serde_util::positive_millis"
    )]
    #[schema(nullable = false)]
    pub deleted_at: Option<i64>,
    /// The identifier of the board the block belongs to.
    #[serde(default)]
    pub container_id: String,
    /// A card's number on its board, as `Card.number` gives it; absent for a template
    /// of the board's cards and for every other block. The server gives it and ignores
    /// a client's, whatever it is.
    #[schema(minimum = 1)]
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "number_as_sent"
    )]
    pub number: Option<u32>,
    /// For a comment whose body was changed: when that last happened, in milliseconds
    /// since the Unix epoch. Absent for a comment never changed and for every other
    /// block. The server sets it, and ignores a client's. What the body said before is
    /// `getCommentVersions`'.
    #[serde(default, skip_serializing_if = "crate::serde_util::is_zero")]
    pub edited_at: i64,
}

/// A card's number as it is read: what is no number a card can have is none, so that
/// what a client sends there, which the server does not take, is never refused either.
///
/// # Errors
///
/// Returns the deserializer's error if what is there is no JSON.
pub(crate) fn number_as_sent<'de, D>(deserializer: D) -> Result<Option<u32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let sent = Value::deserialize(deserializer)?;
    Ok(sent
        .as_u64()
        .and_then(|number| u32::try_from(number).ok())
        .filter(|number| *number > 0))
}

impl Block {
    /// Whether the block's `isTemplate` is the JSON `true`, of a block of any type: for
    /// a card, that it is a template of its board's cards. A block without the field, or
    /// with another value, is no template.
    #[must_use]
    pub fn is_template(&self) -> bool {
        self.fields.get("isTemplate") == Some(&Value::Bool(true))
    }

    /// Whether the block has a number on its board: a card that is no template of its
    /// board's cards ([`Block::is_template`]). The first card stored on a board gets 1
    /// and each one stored after it the next number; a card keeps its number while it is
    /// one, and a number is never given again, also once its card is gone. A card that
    /// is copied or imported is a new card with a new number; a card that comes back
    /// from the trash has its own. A card made a template keeps its number in storage so
    /// turning it back into a card restores its old number without consuming a new one,
    /// while hiding it until then; a block that stops being a card loses its number.
    #[must_use]
    pub fn takes_number(&self) -> bool {
        self.block_type == BlockType::Card && !self.is_template()
    }

    /// The card's number as shown to clients: `Some(number)` only while the block takes
    /// a number ([`Self::takes_number`]). A card template preserves its stored number in
    /// the database but hides it from responses until it becomes an ordinary card again.
    #[must_use]
    pub fn shown_number(&self) -> Option<u32> {
        self.takes_number().then_some(self.number).flatten()
    }

    /// Clears `number` when this block does not currently take one
    /// ([`Self::takes_number`]), so responses and broadcasts never expose a preserved
    /// card number on a template.
    pub fn hide_inactive_number(&mut self) {
        if !self.takes_number() {
            self.number = None;
        }
    }

    /// Checks that the block may be stored.
    ///
    /// # Errors
    ///
    /// Returns a [`BlockError`] if the board identifier is empty, the title or the
    /// serialized fields exceed their size limits (the latter by type), the title holds
    /// U+0000, the fields hold `properties` that are not well-formed card property
    /// values, or the block is a card whose `icon` or `isTemplate` is of another kind
    /// (a string and a boolean, or `null`), or an entry without a `date` that is a day
    /// and its title, or with a `mood`, `weather` or `place` it may not have
    /// ([`BlockError::InvalidEntryField`]), a block of a type without a body has one
    /// ([`BlockError::BodyNotAllowed`]), or a comment's body has a file block
    /// ([`BlockError::FileBlockInComment`]). `properties` among a block's fields always
    /// means card property values, whatever the block's type.
    pub fn validate(&self) -> Result<(), BlockError> {
        if self.container_id.is_empty() {
            return Err(BlockError::EmptyContainerId);
        }
        if self.title.chars().count() > BLOCK_TITLE_MAX_CHARS {
            return Err(BlockError::TitleSizeLimitExceeded);
        }
        if self.title.contains('\0') {
            return Err(BlockError::NulInTitle);
        }
        let of_another_kind = |kinds: &[FieldKind]| {
            kinds.iter().copied().find(|(field, _, is_of_kind)| {
                let value = self.fields.get(*field);
                value.is_some_and(|value| !value.is_null() && !is_of_kind(value))
            })
        };
        match self.block_type {
            BlockType::Card => {
                if let Some((field, expected, _)) = of_another_kind(&CARD_FIELD_KINDS) {
                    return Err(BlockError::InvalidCardField { field, expected });
                }
            }
            BlockType::Page => {
                if let Some((field, expected, _)) = of_another_kind(&PAGE_FIELD_KINDS) {
                    return Err(BlockError::InvalidPageField { field, expected });
                }
            }
            BlockType::Entry => {
                if let Some((field, expected, _)) = of_another_kind(&ENTRY_FIELD_KINDS) {
                    return Err(BlockError::InvalidEntryField { field, expected });
                }
                // An entry is of a day, and is called by it.
                let date = self.fields.get(ENTRY_DATE_FIELD).and_then(Value::as_str);
                let (field, expected) = match date {
                    None => (ENTRY_DATE_FIELD, "a day, as `2026-11-21`"),
                    Some(date) if date != self.title => ("title", "the day of the entry"),
                    Some(_) => return self.validate_fields(),
                };
                return Err(BlockError::InvalidEntryField { field, expected });
            }
            BlockType::Unit => {
                if let Some((field, expected, _)) = of_another_kind(&UNIT_FIELD_KINDS) {
                    return Err(BlockError::InvalidUnitField { field, expected });
                }
            }
            BlockType::Chapter => {
                if let Some((field, expected, _)) = of_another_kind(&CHAPTER_FIELD_KINDS) {
                    return Err(BlockError::InvalidUnitField { field, expected });
                }
            }
            _ => {}
        }
        self.validate_fields()
    }

    /// The checks of [`Self::validate`] that are the same for a block of any type: the
    /// size of its fields, its property values and whether it may have its body.
    fn validate_fields(&self) -> Result<(), BlockError> {
        // Serializing a map with string keys cannot fail; if it ever did, the fields would
        // not be storable either.
        match serde_json::to_string(&self.fields) {
            Ok(fields_json)
                if fields_json.chars().count() <= self.block_type.fields_max_chars() => {}
            _ => return Err(BlockError::FieldsSizeLimitExceeded),
        }
        if self.block_type == BlockType::View {
            validate_view_fields(&self.fields)?;
        }
        validate_block_properties(&self.fields)?;
        self.block_type.check_body(self.body.as_ref())
    }

    /// Removes the fields that a block can no longer be stored with but that one stored
    /// before may hold: `properties` that are no object, for a card an `icon` or
    /// `isTemplate` of another kind, and for a view any unknown or malformed member.
    /// Returns their names. A stored block is made so before it is read as a card,
    /// patched or copied: what is refused is what a request writes, not what was there.
    pub fn drop_malformed_fields(&mut self) -> Vec<String> {
        if self.block_type == BlockType::View {
            let mut dropped = drop_malformed_view_fields(&mut self.fields);
            if self
                .fields
                .get("properties")
                .is_some_and(|v| !v.is_object())
            {
                self.fields.remove("properties");
                dropped.push("properties".to_owned());
            }
            return dropped;
        }
        let is_card = self.block_type == BlockType::Card;
        let malformed: Vec<String> = self
            .fields
            .iter()
            .filter(|(field, value)| match field.as_str() {
                "properties" => !value.is_object(),
                field => is_card && !is_card_field_of_its_kind(field, value),
            })
            .map(|(field, _)| field.clone())
            .collect();
        for field in &malformed {
            self.fields.remove(field);
        }
        malformed
    }

    /// The block as it goes to whoever reads it by the board's link, signed in or not, who
    /// is nobody the server knows: no `createdBy` or `modifiedBy`, no user in the mentions
    /// of the body, and in `fields` none that the board's person properties name
    /// (`people`): in a card's `properties` ([`PersonProperties::hide_values`]) and in a
    /// view's filter and groups ([`PersonProperties::hide_view_fields`]). A user's
    /// identifier tells when they signed up.
    pub fn hide_people(&mut self, people: &PersonProperties) {
        self.created_by.clear();
        self.modified_by.clear();
        if let Some(body) = self.body.as_mut() {
            body.clear_mentions();
        }
        if let Some(Value::Object(values)) = self.fields.get_mut("properties") {
            people.hide_values(values);
        }
        if self.block_type == BlockType::View {
            people.hide_view_fields(&mut self.fields);
        }
    }

    /// Empties what the references of the body name, but for the cards of `shown`: the
    /// block as it goes to whoever reads its board by the link, with
    /// [`Self::hide_people`]. `shown` is the cards of that board; of anything else of
    /// the workspace that reader is told nothing ([`Body::clear_references_but`]).
    ///
    /// [`Body::clear_references_but`]: crate::Body::clear_references_but
    pub fn clear_references_but(&mut self, shown: &HashSet<String>) {
        if let Some(body) = self.body.as_mut() {
            body.clear_references_but(shown);
        }
    }

    /// The stored names of the files the block shows, each once: the file of an
    /// attachment block (`fileId`, or failing that `attachmentId`), and the images and
    /// file blocks of the body. The store's migration of file references
    /// (`0003_file_refs.sql`) put down the same rule in SQL, with the image blocks of
    /// then.
    #[must_use]
    pub fn shown_files(&self) -> Vec<String> {
        if self.block_type == BlockType::Attachment {
            return self
                .attachment_file()
                .map(str::to_owned)
                .into_iter()
                .collect();
        }
        self.body
            .as_ref()
            .map(|body| body.file_ids().into_iter().map(str::to_owned).collect())
            .unwrap_or_default()
    }

    /// The stored name of the file of an attachment block: its `fileId`, or failing that
    /// its `attachmentId`; `None` for an empty name and for a block of another type.
    #[must_use]
    pub fn attachment_file(&self) -> Option<&str> {
        if self.block_type != BlockType::Attachment {
            return None;
        }
        ["fileId", "attachmentId"]
            .iter()
            .find_map(|key| self.fields.get(*key).and_then(serde_json::Value::as_str))
            .filter(|name| !name.is_empty())
    }
}

/// A partial update of a [`Block`]. Absent members leave the block unchanged. A block's
/// [`BlockType`] is fixed when the block is created and cannot be patched.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct BlockPatch {
    /// The new parent identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    /// The new title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Fields to set, replacing any existing value under the same key.
    #[schema(value_type = Object)]
    pub updated_fields: JsonMap,
    /// Keys of fields to remove.
    pub deleted_fields: Vec<String>,
    /// The new body of a card or a comment; `null` takes the body away. Sent only with
    /// `readAt` (400 naming `readAt` without it).
    #[serde(skip_serializing_if = "BodyChange::is_keep")]
    #[schema(value_type = Option<Body>)]
    pub body: BodyChange,
    /// The `updatedAt` of the block as the client read it. If the block has changed
    /// since, the patch is refused.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_at: Option<i64>,
    /// Card property values to set and remove, merged into the values the block has
    /// when it is patched: what `patchCard` sends. Not a member of a block patch's JSON.
    #[serde(skip)]
    pub property_changes: PropertyChanges,
}

/// Changes to the property values of a card, by property identifier: the values to set,
/// then the ones to remove. The values that are named in neither stay.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PropertyChanges {
    /// Values to set, each replacing the value of its property.
    pub updated: JsonMap,
    /// Properties whose value is removed.
    pub deleted: Vec<String>,
}

impl PropertyChanges {
    /// Sets the updated values in `values`, then removes the deleted ones.
    pub fn apply_to(&self, values: &mut JsonMap) {
        for (id, value) in &self.updated {
            values.insert(id.clone(), value.clone());
        }
        for id in &self.deleted {
            values.remove(id);
        }
    }

    /// Applies the changes to the `properties` field of a block's fields, made an empty
    /// object first if it is absent or no object and there is a value to set.
    fn apply_to_fields(&self, fields: &mut JsonMap) {
        if self.updated.is_empty() && !fields.get("properties").is_some_and(Value::is_object) {
            return;
        }
        let properties = fields
            .entry("properties")
            .or_insert_with(|| Value::Object(JsonMap::new()));
        if !properties.is_object() {
            *properties = Value::Object(JsonMap::new());
        }
        if let Value::Object(values) = properties {
            self.apply_to(values);
        }
    }
}

impl BlockPatch {
    /// Applies the patch to `block`.
    ///
    /// Updated fields are written before deleted fields are removed, so a key listed in
    /// both ends up removed.
    pub fn apply(&self, block: &mut Block) {
        if let Some(parent_id) = &self.parent_id {
            block.parent_id.clone_from(parent_id);
        }
        if let Some(title) = &self.title {
            block.title.clone_from(title);
        }
        for (key, value) in &self.updated_fields {
            block.fields.insert(key.clone(), value.clone());
        }
        for key in &self.deleted_fields {
            block.fields.remove(key);
        }
        self.property_changes.apply_to_fields(&mut block.fields);
        self.body.apply_to(&mut block.body);
    }
}

/// A batch of block identifiers and the patch to apply to each, matched by position.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct BlockPatchBatch {
    /// The identifiers of the blocks to patch.
    pub block_ids: Vec<String>,
    /// The patches, one per identifier.
    pub block_patches: Vec<BlockPatch>,
}

/// Marks every block as modified by `user_id` at `now` (milliseconds since the epoch).
pub fn stamp_modification_metadata(user_id: &str, blocks: &mut [Block], now: i64) {
    for block in blocks {
        user_id.clone_into(&mut block.modified_by);
        block.updated_at = now;
    }
}
