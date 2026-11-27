//! Board completion rules (`DoneWhen`), caller-scoped effective permissions
//! (`ContainerPermissions`), atomic multi-card operations, sidebar mutations, and
//! cross-workspace container copy payloads.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::JsonMap;
use crate::block::Block;
use crate::card::Card;
use crate::card_property::{CardPropertyDefinition, PropertyColor, PropertyOption, PropertyType};
use crate::container::Container;
use crate::date_value::{check_date_value, is_calendar_date, utc_date};
use crate::id::new_id;

/// Maximum number of card identifiers accepted in one multi-card move, delete, or restore.
pub const MAX_CARD_BATCH: usize = 200;

/// Rule on a board defining when one of its cards counts as completed (`done`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DoneWhen {
    /// Identifier of a `select`, `multiSelect`, or `checkbox` property on the board.
    pub property_id: String,
    /// Option identifiers of `property_id` that mark a card complete when `property_id`
    /// is `select` or `multiSelect`. Empty when `property_id` is `checkbox` (where
    /// `"true"` marks completion).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub option_ids: Vec<String>,
}

impl DoneWhen {
    /// Whether any string in the rule contains `U+0000`.
    #[must_use]
    pub(crate) fn holds_nul(&self) -> bool {
        self.property_id.contains('\0') || self.option_ids.iter().any(|opt| opt.contains('\0'))
    }
}

/// Extracted due-date indexing facts for a card on a board.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardDuePoint {
    /// Indexed `YYYY-MM-DD` date (calendar date directly, or UTC date of a timestamp).
    pub due_at: String,
    /// Exact epoch milliseconds when the date property holds a timestamp rather than a
    /// calendar day.
    pub due_millis: Option<i64>,
    /// Canonical string key identifying this specific due point for `card_due_notices`.
    pub due_key: String,
}

/// Evaluates whether a card's `properties` satisfy `done_when` against `definitions`.
#[must_use]
pub fn is_card_done(
    values: &JsonMap,
    definitions: &[CardPropertyDefinition],
    done_when: Option<&DoneWhen>,
) -> bool {
    let Some(rule) = done_when else {
        return false;
    };
    let Some(def) = definitions.iter().find(|d| d.id == rule.property_id) else {
        return false;
    };
    let Some(value) = values.get(&rule.property_id) else {
        return false;
    };
    match def.property_type {
        PropertyType::Checkbox => value.as_str() == Some("true"),
        PropertyType::Select => value.as_str().is_some_and(|chosen| {
            !chosen.is_empty() && rule.option_ids.iter().any(|id| id == chosen)
        }),
        PropertyType::MultiSelect => value.as_array().is_some_and(|items| {
            items.iter().any(|item| {
                item.as_str()
                    .is_some_and(|chosen| rule.option_ids.iter().any(|id| id == chosen))
            })
        }),
        _ => false,
    }
}

/// Extracts the [`CardDuePoint`] of a card's `properties` using the board's effective
/// due-date property (`due_date_property_id` or else the board's first `date` property).
#[must_use]
pub fn extract_card_due_point(
    values: &JsonMap,
    due_property_id: Option<&str>,
) -> Option<CardDuePoint> {
    let prop_id = due_property_id?;
    let obj = values.get(prop_id)?.as_object()?;
    check_date_value(obj).ok()?;
    let target = obj.get("to").unwrap_or_else(|| &obj["from"]);
    if let Some(day) = target.as_str().filter(|s| is_calendar_date(s)) {
        return Some(CardDuePoint {
            due_at: day.to_owned(),
            due_millis: None,
            due_key: format!("d:{day}"),
        });
    }
    let millis = target.as_i64()?;
    let due_at = utc_date(millis)?;
    Some(CardDuePoint {
        due_at,
        due_millis: Some(millis),
        due_key: format!("t:{millis}"),
    })
}

/// Language used when generating localized default property labels on a blank board.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoardLocale {
    /// English labels.
    English,
    /// Korean labels.
    Korean,
}

/// Whether a duplicate is an ordinary copy or a template.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DuplicateAs {
    /// An ordinary copy.
    Copy,
    /// A template, which new boards are made from; a wiki is never one.
    Template,
}

/// The properties a new blank board starts with, so that Today works with no setting up:
/// a `status` whose last option is done, an `assignee` and a `due` date.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlankBoardScaffolding {
    /// A select of to do, in progress and done, which the first view groups by.
    pub status: CardPropertyDefinition,
    /// The person a card is given to.
    pub assignee: CardPropertyDefinition,
    /// The day a card is due.
    pub due: CardPropertyDefinition,
    /// When a card is done: its `status` is done.
    pub done_when: DoneWhen,
}

impl BlankBoardScaffolding {
    /// The board's property definitions, in the order the board lists them.
    #[must_use]
    pub fn properties(&self) -> Vec<CardPropertyDefinition> {
        vec![self.status.clone(), self.assignee.clone(), self.due.clone()]
    }
}

/// The properties of a new blank board ([`BlankBoardScaffolding`]), named in `locale`.
#[must_use]
pub fn default_blank_board_scaffolding(locale: BoardLocale) -> BlankBoardScaffolding {
    let (status_name, todo_label, doing_label, done_label, assignee_name, due_name) =
        if locale == BoardLocale::Korean {
            ("상태", "할 일", "진행 중", "완료", "담당", "마감일")
        } else {
            (
                "Status",
                "To do",
                "In progress",
                "Done",
                "Assignee",
                "Due date",
            )
        };
    let done_id = new_id();
    let status = CardPropertyDefinition {
        id: new_id(),
        name: status_name.to_owned(),
        property_type: PropertyType::Select,
        options: vec![
            PropertyOption::new(new_id(), todo_label, PropertyColor::Gray),
            PropertyOption::new(new_id(), doing_label, PropertyColor::Blue),
            PropertyOption::new(done_id.clone(), done_label, PropertyColor::Green),
        ],
    };
    let done_when = DoneWhen {
        property_id: status.id.clone(),
        option_ids: vec![done_id],
    };
    BlankBoardScaffolding {
        status,
        assignee: CardPropertyDefinition::new(new_id(), assignee_name, PropertyType::Person),
        due: CardPropertyDefinition::new(new_id(), due_name, PropertyType::Date),
        done_when,
    }
}

/// Caller-specific effective permissions on a container.
///
/// Kept strictly separate from [`Container`] so WebSocket `containerChanged` broadcasts
/// never leak or overwrite another viewer's permissions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each capability flag is a distinct boolean permission in the HTTP wire contract"
)]
pub struct ContainerPermissions {
    /// Whether the caller may create, edit, move, and delete cards/pages/units.
    pub can_edit: bool,
    /// Whether the caller may post and edit their own comments.
    pub can_comment: bool,
    /// Whether the caller may edit container metadata, properties, and views.
    pub can_manage_properties: bool,
    /// Whether the caller may change container visibility (`open` / `private`) and `minimumRole`.
    pub can_manage_visibility: bool,
    /// Whether the caller may add, update, or remove container members and group roles.
    pub can_manage_roles: bool,
    /// Whether the caller may delete or restore the container itself.
    pub can_delete: bool,
    /// Whether the caller may manage public read-token sharing links on the container.
    pub can_share: bool,
    /// Whether the caller may delete comments authored by other users.
    pub can_manage_others_comments: bool,
}

/// Request body for moving one or more cards within a board view (`card-moves`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct CardMovesRequest {
    /// Ordered card identifiers to move (`1..=200`, no duplicates).
    pub card_ids: Vec<String>,
    /// Optional grouping property identifier whose value is updated on every moved card.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_property_id: Option<String>,
    /// Value to assign to `groupPropertyId` (`null` or empty clears the property).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_value: Option<Value>,
    /// Optional live card on the same board after which `cardIds` are placed in the
    /// view's `cardOrder`; `""` places them at the start, omitted/`null` appends or
    /// preserves relative placement when none is specified.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_card_id: Option<String>,
}

/// Response returned by `POST /w/{ws}/boards/{boardId}/views/{viewId}/card-moves`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CardMovesResult {
    /// Updated view block carrying the new `cardOrder`.
    pub view: Block,
    /// Updated cards when `groupPropertyId` modified their properties.
    pub cards: Vec<Card>,
}

/// Request body carrying `1..=200` distinct card identifiers for batch delete or restore.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct CardBatchIds {
    /// Card identifiers (`1..=200`, no duplicates).
    pub card_ids: Vec<String>,
}

/// Response returned by `POST /w/{ws}/boards/{boardId}/cards/batch-restore`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RestoredCards {
    /// All cards restored (including restored sub-cards), ready to merge into client state.
    pub cards: Vec<Card>,
}

/// Request body for creating a verified attachment block on a card.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NewCardAttachment {
    /// Uploaded file identifier belonging to the same board (`files.id`).
    pub file_id: String,
    /// The name the attachment is shown under, such as that of an edit kept as a file of
    /// its own beside the one it was made from; the file's name when left out or blank.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// Partial update of a sidebar category (`PATCH /w/{ws}/categories/{categoryId}`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct CategoryPatch {
    /// Optional new display name (non-blank).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Optional collapsed state in the sidebar.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collapsed: Option<bool>,
}

/// Atomic move and/or visibility update of a container in a sidebar category
/// (`PUT /w/{ws}/categories/{categoryId}/containers/{containerId}`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct CategoryContainerUpdate {
    /// Optional 0-based position within the target category's container order.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
    /// Optional hidden state of the container in the sidebar.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
}

/// Request body for copying a container to another workspace (`copy-to-workspace`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CopyContainerToWorkspaceRequest {
    /// Destination workspace identifier (in the same or another organization, including
    /// the caller's personal space).
    pub target_workspace_id: String,
    /// Optional custom title for the copied container; defaults to a localized copy title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// Response returned by `POST /w/{ws}/containers/{containerId}/copy-to-workspace`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CopiedContainerResult {
    /// Newly created container in `targetWorkspaceId`.
    pub container: Container,
    /// Number of missing source file references stripped during the copy.
    pub missing_files_count: usize,
}
