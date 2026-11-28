//! What each tool takes: the types its `arguments` are read into, which its input
//! schema is made from (`Tool::input_schema`), so that what a tool says it takes and
//! what it reads are one thing.

use fvoci_model::{JsonMap, Recurrence};
use serde::{Deserialize, Deserializer};
use utoipa::ToSchema;

/// The arguments of a tool that takes none: an empty object.
#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct NoArguments {}

/// A workspace whose overview or day to read.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct WorkspaceArguments {
    /// The workspace, as `list_workspaces` names it.
    pub(super) workspace_id: String,
}

/// People to find in a workspace.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct FindPeopleArguments {
    /// The workspace, as `list_workspaces` names it.
    pub(super) workspace_id: String,
    /// Text their login name or name holds, in any case; left out, the ten oldest
    /// accounts.
    #[serde(default)]
    pub(super) q: Option<String>,
}

/// What a search finds.
#[derive(Debug, Clone, Copy, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub(super) enum ContentKind {
    /// Cards of boards.
    Card,
    /// Pages of wikis.
    Page,
    /// Units of courses.
    Unit,
}

impl ContentKind {
    /// The word `searchWorkspaceContent` takes for it.
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Card => "card",
            Self::Page => "page",
            Self::Unit => "unit",
        }
    }
}

/// A search of what a workspace says.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SearchArguments {
    /// The workspace, as `list_workspaces` names it.
    pub(super) workspace_id: String,
    /// The words, every one of which a hit's title and body hold together; `#12` finds
    /// the cards numbered 12. At most 200 characters.
    pub(super) q: String,
    /// Only what is of this kind.
    #[serde(default)]
    #[schema(inline)]
    pub(super) kind: Option<ContentKind>,
    /// Only what is in this board, wiki or course: the `limit` is of its hits.
    #[serde(default)]
    pub(super) container_id: Option<String>,
    /// How many hits at most, 1 to 50; 20 if left out.
    #[serde(default)]
    #[schema(minimum = 1, maximum = 50)]
    pub(super) limit: Option<u32>,
}

/// The order cards are listed in.
#[derive(Debug, Clone, Copy, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum CardOrder {
    /// The oldest first.
    CreatedAsc,
    /// The most recently changed first.
    UpdatedDesc,
    /// By their number.
    NumberAsc,
}

impl CardOrder {
    /// The word `getCards` takes for it.
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::CreatedAsc => "created_asc",
            Self::UpdatedDesc => "updated_desc",
            Self::NumberAsc => "number_asc",
        }
    }
}

/// The cards of a board to list.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ListCardsArguments {
    /// The workspace, as `list_workspaces` names it.
    pub(super) workspace_id: String,
    /// The board, as `get_workspace_overview` names it.
    pub(super) board_id: String,
    /// Text a card's title holds.
    #[serde(default)]
    pub(super) q: Option<String>,
    /// Only the sub-cards of this card; `root` for the cards at the top.
    #[serde(default)]
    pub(super) parent_id: Option<String>,
    /// Only the cards given to this person (`me` for the caller), by a property of
    /// people.
    #[serde(default)]
    pub(super) assignee_id: Option<String>,
    /// Only the cards whose value of this property is `propertyValue`; sent with it.
    #[serde(default)]
    pub(super) property_id: Option<String>,
    /// The value `propertyId` is to have: an option's identifier for a select, a
    /// person's for people, the text for text.
    #[serde(default)]
    pub(super) property_value: Option<String>,
    /// Only the cards changed at or after this time, in milliseconds since the Unix
    /// epoch.
    #[serde(default)]
    pub(super) updated_since: Option<i64>,
    /// The order; the oldest first if left out.
    #[serde(default)]
    #[schema(inline)]
    pub(super) sort: Option<CardOrder>,
    /// The page, from 1.
    #[serde(default)]
    #[schema(minimum = 1)]
    pub(super) page: Option<u32>,
    /// How many cards a page has, from 1 to 1000; 50 if left out.
    #[serde(default)]
    #[schema(minimum = 1, maximum = 1000)]
    pub(super) per_page: Option<u32>,
}

/// What the item is: a card, a page or a unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub(super) enum ItemKind {
    /// A card of a board.
    Card,
    /// A page of a wiki.
    Page,
    /// A unit (or chapter) of a course.
    Unit,
}

/// An item to read.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct GetItemArguments {
    /// The workspace, as `list_workspaces` names it.
    pub(super) workspace_id: String,
    // Described by its type: a field's own description would wrap the enum in an
    // `allOf` with an object, which no value is.
    #[schema(inline)]
    pub(super) kind: ItemKind,
    /// The item's identifier. A card is named by it, or by `containerId` and `number`.
    #[serde(default)]
    pub(super) id: Option<String>,
    /// The board, wiki or course the item is in: needed for a page, a unit, and a card
    /// named by its number.
    #[serde(default)]
    pub(super) container_id: Option<String>,
    /// The card's number on its board, as `#12` names it.
    #[serde(default)]
    #[schema(minimum = 1)]
    pub(super) number: Option<u32>,
}

/// A wiki or a course whose outline to read.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct GetOutlineArguments {
    /// The workspace, as `list_workspaces` names it.
    pub(super) workspace_id: String,
    /// The wiki or the course.
    pub(super) container_id: String,
}

/// An item whose comments to list, or to comment on.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ListCommentsArguments {
    /// The workspace, as `list_workspaces` names it.
    pub(super) workspace_id: String,
    // Described by its type: a field's own description would wrap the enum in an
    // `allOf` with an object, which no value is.
    #[schema(inline)]
    pub(super) kind: ItemKind,
    /// The board, wiki or course it is in.
    pub(super) container_id: String,
    /// The card, page or unit.
    pub(super) item_id: String,
}

/// A card whose activity to read.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ListActivityArguments {
    /// The workspace, as `list_workspaces` names it.
    pub(super) workspace_id: String,
    /// The card.
    pub(super) card_id: String,
    /// The page, from 1.
    #[serde(default)]
    #[schema(minimum = 1)]
    pub(super) page: Option<u32>,
    /// How many lines a page has, from 1 to 1000; 50 if left out.
    #[serde(default)]
    #[schema(minimum = 1, maximum = 1000)]
    pub(super) per_page: Option<u32>,
}

/// Cards of one board to change alike.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SaveCardsArguments {
    /// The workspace, as `list_workspaces` names it.
    pub(super) workspace_id: String,
    /// The board the cards are on.
    pub(super) board_id: String,
    /// The cards: 1 to 200 distinct identifiers of cards of the board that are no
    /// templates.
    #[schema(min_items = 1, max_items = 200)]
    pub(super) card_ids: Vec<String>,
    /// Values to set on each, by property identifier, as `save_card` takes them. The
    /// cards' other values stay.
    #[serde(default)]
    #[schema(value_type = Option<Object>)]
    pub(super) properties: Option<JsonMap>,
    /// Properties whose value the cards are to lose.
    #[serde(default)]
    pub(super) deleted_properties: Option<Vec<String>>,
}

/// The files of a board or wiki, or of one card.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ListFilesArguments {
    /// The workspace, as `list_workspaces` names it.
    pub(super) workspace_id: String,
    /// The board or wiki.
    pub(super) container_id: String,
    /// Only the files this card of the board shows: those its body names and those
    /// attached to it.
    #[serde(default, rename = "cardId")]
    pub(super) card: Option<String>,
}

/// A card to make or change.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SaveCardArguments {
    /// The workspace, as `list_workspaces` names it.
    pub(super) workspace_id: String,
    /// The board to make the card on; left out when `cardId` names one to change.
    #[serde(default)]
    pub(super) board_id: Option<String>,
    /// The card to change; left out, a card is made.
    #[serde(default)]
    pub(super) card_id: Option<String>,
    /// The title to give it; left out, a card that is changed keeps its own.
    #[serde(default)]
    pub(super) title: Option<String>,
    /// The icon: one emoji, or empty for none.
    #[serde(default)]
    pub(super) icon: Option<String>,
    /// The card it is a sub-card of, on the same board; empty for the top of the board.
    #[serde(default)]
    pub(super) parent_id: Option<String>,
    /// Values to set, by property identifier (`cardProperties` of the board in
    /// `get_workspace_overview`): an option's identifier for a select, a list of them
    /// for a multi-select, a list of person identifiers for people, `YYYY-MM-DD` for a
    /// date, text for text, a number for a number, `true` or `false` for a checkbox.
    /// The card's other values stay.
    #[serde(default)]
    #[schema(value_type = Option<Object>)]
    pub(super) properties: Option<JsonMap>,
    /// Properties whose value the card is to lose.
    #[serde(default)]
    pub(super) deleted_properties: Option<Vec<String>>,
    /// The whole body in Markdown, written over the body there is: refused while
    /// people may type in it (`room_has_active_editors`), and while it has blocks its
    /// Markdown does not hold (`complex_blocks_use_append`). To change a card's body,
    /// sent with `readAt`. Not with `appendMarkdown`.
    #[serde(default)]
    pub(super) body_markdown: Option<String>,
    /// The `updatedAt` of the card as it was read: the change is refused (409) if the
    /// card changed since. Sent with what is written whole; `appendMarkdown` alone keeps
    /// what changed since and takes none.
    #[serde(default)]
    pub(super) read_at: Option<i64>,
    /// Markdown to add after the last of the body, written as one more editor writes:
    /// whoever types in the body meanwhile keeps what they typed. Added after the
    /// other changes. Not with `bodyMarkdown`.
    #[serde(default)]
    pub(super) append_markdown: Option<String>,
    /// How the card repeats: `{"every": 1, "unit": "week", "anchor": "due"}` (`every`
    /// 1 to 365; `unit` `day`, `week`, `month` or `year`; counted from the due date,
    /// `due`, or from the day it is completed, `done`); `null` makes it stop. When a
    /// card that repeats is completed, the server makes the next card, due one period
    /// on. Only on a board with a rule of done and a named due date property
    /// (`recurrence_needs_board_rules`). Left out, a card that is changed keeps its own.
    #[serde(default, deserialize_with = "given_or_null")]
    #[schema(value_type = Option<RecurrenceShape>, inline)]
    #[expect(
        clippy::option_option,
        reason = "left out keeps how the card repeats, `null` makes it stop"
    )]
    pub(super) recurrence: Option<Option<Recurrence>>,
}

/// How a card repeats.
// The shape of a `Recurrence` in an input schema, which refers to no other: what
// `save_card` reads it as is the model's.
#[derive(ToSchema)]
#[expect(dead_code, reason = "only its schema is used")]
struct RecurrenceShape {
    #[schema(minimum = 1, maximum = 365)]
    every: u16,
    #[schema(inline)]
    unit: fvoci_model::RecurrenceUnit,
    #[schema(inline)]
    anchor: fvoci_model::RecurrenceAnchor,
}

/// A member that may be left out (`None`), `null` (`Some(None)`) or a value.
#[expect(
    clippy::option_option,
    reason = "a member left out keeps what is there, `null` takes it away"
)]
fn given_or_null<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

/// Whether it is a page of a wiki or a unit of a course.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub(super) enum PageOrUnit {
    /// A page of a wiki.
    Page,
    /// A unit (or chapter) of a course.
    Unit,
}

/// What a new entry of a course's outline is.
#[derive(Debug, Clone, Copy, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub(super) enum OutlineEntry {
    /// A unit, with a body.
    Unit,
    /// A chapter: a title at the top of the course that units go below, with no body.
    Chapter,
}

/// The caller's own progress on a unit.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ProgressArguments {
    /// Whether they finished it.
    #[serde(default)]
    pub(super) completed: Option<bool>,
    /// Where they are in it, in their own words.
    #[serde(default)]
    pub(super) last_position: Option<String>,
}

/// A page or a unit to make or change, or progress on a unit to record.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SavePageOrUnitArguments {
    /// The workspace, as `list_workspaces` names it.
    pub(super) workspace_id: String,
    // Described by its type: a field's own description would wrap the enum in an
    // `allOf` with an object, which no value is.
    #[schema(inline)]
    pub(super) kind: PageOrUnit,
    /// The wiki or the course.
    pub(super) container_id: String,
    /// The page or unit to change; left out, one is made.
    #[serde(default)]
    pub(super) id: Option<String>,
    /// The title; a new one made without it has none.
    #[serde(default)]
    pub(super) title: Option<String>,
    /// A page's icon: one emoji, or empty for none.
    #[serde(default)]
    pub(super) icon: Option<String>,
    /// Where a new one goes: below this page, or this chapter of the course; at the top
    /// if left out. Not for one that is changed.
    #[serde(default)]
    pub(super) parent_id: Option<String>,
    /// Among those of its parent, right after this one; last if left out.
    #[serde(default)]
    pub(super) after: Option<String>,
    /// What a new entry of a course is; a unit if left out.
    #[serde(default)]
    #[schema(inline)]
    pub(super) unit_kind: Option<OutlineEntry>,
    /// The whole body in Markdown, written over the body there is: refused while
    /// people may type in it (`room_has_active_editors`), and while it has blocks its
    /// Markdown does not hold (`complex_blocks_use_append`). To change one's body, sent
    /// with `readAt`. Not with `appendMarkdown`.
    #[serde(default)]
    pub(super) body_markdown: Option<String>,
    /// The `updatedAt` of the page or unit as it was read: the change is refused (409)
    /// if it changed since. Sent with what is written whole; `appendMarkdown` alone
    /// keeps what changed since and takes none.
    #[serde(default)]
    pub(super) read_at: Option<i64>,
    /// Markdown to add after the last of the body, as one more editor writes: whoever
    /// types in it meanwhile keeps what they typed. Not with `bodyMarkdown`.
    #[serde(default)]
    pub(super) append_markdown: Option<String>,
    /// The caller's own progress on the unit, recorded after the rest: only in a
    /// course they joined (403 otherwise).
    #[serde(default)]
    #[schema(inline)]
    pub(super) progress: Option<ProgressArguments>,
}

/// A comment to write.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PostCommentArguments {
    /// The workspace, as `list_workspaces` names it.
    pub(super) workspace_id: String,
    // Described by its type: a field's own description would wrap the enum in an
    // `allOf` with an object, which no value is.
    #[schema(inline)]
    pub(super) kind: ItemKind,
    /// The board, wiki or course it is in.
    pub(super) container_id: String,
    /// The card, page or unit.
    pub(super) item_id: String,
    /// The comment in Markdown. A person is mentioned as `@[name](fvoci-user://<id>)`
    /// (`find_people` gives the identifier).
    pub(super) markdown: String,
}
