//! The request bodies whose shape differs from what the server answers.
//!
//! The server decodes most bodies into the type it also answers with: a member that is
//! left out is taken as empty, and members the server sets itself are read and thrown
//! away. The contract says less and means it: these types list the members a request is
//! for, and which of them it needs. Nothing is decoded into them. A member that is
//! optional here may be left out; it may be `null` only where its schema says so (a
//! card's `body`, `regenerate`, a `displayName` or a setting to change).

#![expect(
    dead_code,
    reason = "these types describe request bodies; nothing is decoded into them"
)]

use fvoci_model::{
    BlockType, Body, CardOpen, CardPropertyDefinition, CategoryType, ContainerRole,
    ContainerVisibility, Credential, Density, DoneWhen, HourCycle, JsonMap, MinimumRole, NameShown,
    Theme, Title, WeekStart,
};
use utoipa::ToSchema;

/// The identifiers of the users asked for: at least one, at most
/// [`MAX_USER_IDS`](crate::api::MAX_USER_IDS).
#[derive(Debug)]
pub(crate) struct UserIds;

impl utoipa::PartialSchema for UserIds {
    fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        utoipa::openapi::schema::ArrayBuilder::new()
            .items(String::schema())
            .min_items(Some(1))
            .max_items(Some(crate::api::MAX_USER_IDS))
            .description(Some(
                "The identifiers of the users asked for: at least one, at most 200.",
            ))
            .into()
    }
}

impl ToSchema for UserIds {}

/// The identifiers the references of a body name: at most
/// [`MAX_REFERENCE_IDS`](crate::app::MAX_REFERENCE_IDS).
#[derive(Debug)]
pub(crate) struct ReferenceIds;

impl utoipa::PartialSchema for ReferenceIds {
    fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        utoipa::openapi::schema::ArrayBuilder::new()
            .items(String::schema())
            .max_items(Some(crate::app::MAX_REFERENCE_IDS))
            .description(Some(format!(
                "The identifiers the references name, each the `id` of a node `reference`: \
                 at most {}.",
                crate::app::MAX_REFERENCE_IDS
            )))
            .into()
    }
}

impl ToSchema for ReferenceIds {}

/// A board to make.
#[derive(Debug, ToSchema)]
#[schema(rename_all = "camelCase")]
pub(crate) struct NewBoard {
    /// Only where boards and blocks are made together, and needed there: a name by
    /// which the blocks of the request say which board they belong to. The server gives
    /// the board another identifier. Where one board is made, a board with an `id` is
    /// refused.
    #[schema(nullable = false)]
    id: Option<String>,
    /// Whether the board is open or private.
    visibility: ContainerVisibility,
    /// The role a user gets who joins the board. Left out, `editor`.
    #[schema(nullable = false)]
    minimum_role: Option<MinimumRole>,
    /// The board's title.
    #[schema(nullable = false)]
    title: Option<String>,
    /// The board's description.
    #[schema(nullable = false)]
    description: Option<String>,
    /// The board's icon, usually one emoji.
    #[schema(nullable = false)]
    icon: Option<String>,
    /// Whether the description is shown in the interface.
    #[schema(nullable = false)]
    show_description: Option<bool>,
    /// Whether the board is a template.
    #[schema(nullable = false)]
    is_template: Option<bool>,
    /// The version of the built-in template the board came from.
    #[schema(nullable = false)]
    template_version: Option<i64>,
    /// Free-form properties of the board itself.
    #[schema(value_type = Option<Object>, nullable = false)]
    properties: Option<JsonMap>,
    /// The property definitions of the board's cards, in display order. Each needs a
    /// non-empty `id` that no other has.
    #[schema(nullable = false)]
    card_properties: Option<Vec<CardPropertyDefinition>>,
    /// Explicit card `date` property identifier driving `'Today'` and `due_soon` alerts.
    due_date_property_id: Option<String>,
    /// Rule defining when a card on the board counts as completed (`done`).
    done_when: Option<DoneWhen>,
    /// When `true` on a blank non-template board, populates default status, assignee, and
    /// due-date properties, `doneWhen`, and an initial Kanban view.
    #[schema(nullable = false)]
    scaffold: Option<bool>,
}

/// A block to insert.
#[derive(Debug, ToSchema)]
#[schema(rename_all = "camelCase")]
pub(crate) struct NewBlock {
    /// A name for the block within the request. The server gives every block a new
    /// identifier, and where one block of the request names another (as its `parentId`,
    /// or in the fields `contentOrder`, `cardOrder` and `defaultTemplateId`) it puts
    /// the new identifier there. Blocks that come under one name get one identifier,
    /// which cannot be stored twice; a block that no other names needs no `id`.
    #[schema(nullable = false)]
    id: Option<String>,
    /// The board the block belongs to: the board of the path, or, where boards and
    /// blocks are made together, the `id` of one of the request's boards.
    container_id: String,
    /// The block this one is below: a block of the same board, or the name of another
    /// block of the request. Left out for a block directly on its board.
    #[schema(nullable = false)]
    parent_id: Option<String>,
    /// The block type (`view`, `comment` or `attachment` on `/boards/{boardId}/blocks`;
    /// `card` is created via `createCard`, or in bulk via `createContainersAndBlocks`).
    #[schema(rename = "type")]
    block_type: BlockType,
    /// The display title.
    #[schema(nullable = false)]
    title: Option<String>,
    /// Type-specific data, free-form: serialized, at most 64,000 characters for a card,
    /// a comment or an attachment and 800,000 for a view. The `properties` of a card
    /// among them are values by property identifier: a string, a list of strings, a date
    /// value, a list of relation link objects (`{"card", "kind", "lagDays"}`), or `null`.
    #[schema(value_type = Option<Object>, nullable = false)]
    fields: Option<JsonMap>,
}

/// Boards and their blocks, to make together.
#[derive(Debug, ToSchema)]
#[schema(rename_all = "camelCase")]
pub(crate) struct NewContainersAndBlocks {
    /// The boards, at least one. Each needs an `id`.
    containers: Vec<NewBoard>,
    /// The blocks, at least one. The `containerId` of each is the `id` of one of `containers`.
    blocks: Vec<NewBlock>,
}

/// A card to make.
#[derive(Debug, ToSchema)]
#[schema(rename_all = "camelCase")]
pub(crate) struct NewCard {
    /// The board of the path, if it is there at all.
    #[schema(nullable = false)]
    board_id: Option<String>,
    /// The identifier of the parent card on the same board; left out, empty, or `null`
    /// for a top-level card (depth 1). At most three levels deep; templates cannot have a
    /// parent or sub-cards.
    parent_id: Option<String>,
    /// The display title.
    #[schema(nullable = false)]
    title: Option<String>,
    /// The card's icon; at most one grapheme.
    #[schema(nullable = false)]
    icon: Option<String>,
    /// Whether the card is a template.
    #[schema(nullable = false)]
    is_template: Option<bool>,
    /// Property values by property identifier: a string, a list of strings, a date value,
    /// a list of relation link objects (`{"card", "kind", "lagDays"}`), or `null`.
    #[schema(value_type = Option<Object>, nullable = false)]
    properties: Option<JsonMap>,
    /// The card's body; left out or `null` for none.
    body: Option<Body>,
    /// The card's body in Markdown, as `Card.bodyMarkdown` says, in place of `body`.
    #[schema(nullable = false)]
    body_markdown: Option<String>,
}

/// A sidebar category to make.
#[derive(Debug, ToSchema)]
#[schema(rename_all = "camelCase")]
pub(crate) struct NewCategory {
    /// The display name; not blank.
    name: String,
    /// The user of the session: a user makes categories for themselves only.
    user_id: String,
    /// The workspace of the path.
    workspace_id: String,
    /// Whether the category is collapsed in the sidebar.
    #[schema(nullable = false)]
    collapsed: Option<bool>,
    /// The position of the category among the user's categories. It is stored 10
    /// higher: see `createCategory`.
    #[schema(nullable = false)]
    sort_order: Option<i64>,
    /// Left out, `custom`. A category that is made as `system` can be neither renamed
    /// nor deleted afterwards.
    #[schema(rename = "type")]
    #[schema(nullable = false)]
    category_type: Option<CategoryType>,
}

/// What a sidebar category is changed to. The name and the collapsed state are what
/// changes; a system category keeps its name.
#[derive(Debug, ToSchema)]
#[schema(rename_all = "camelCase")]
pub(crate) struct CategoryUpdate {
    /// The category of the path.
    id: String,
    /// The display name; not blank.
    name: String,
    /// The user of the session.
    user_id: String,
    /// The workspace of the path.
    workspace_id: String,
    /// Whether the category is collapsed in the sidebar. Left out, it is not.
    #[schema(nullable = false)]
    collapsed: Option<bool>,
}

/// A user to make a member of a board, and the role to give them.
#[derive(Debug, ToSchema)]
#[schema(rename_all = "camelCase")]
pub(crate) struct NewMember {
    /// The identifier of the user, who must be able to see the board's workspace.
    user_id: String,
    /// The role the user is to have on the board. Required when who asks manages the
    /// board's roles (`admin` included); on an open board an editor who does not manage
    /// roles grants the board's `minimumRole` (and asking for `admin` is 403).
    #[schema(nullable = false)]
    role: Option<ContainerRole>,
}

/// Whether a container's public link is to be enabled.
#[derive(Debug, ToSchema)]
#[schema(rename_all = "camelCase")]
pub(crate) struct PublicLinkRequest {
    /// `true` enables the public link, `false` turns it off. The read token a container
    /// had stops working when the link is turned off.
    enabled: bool,
    /// With `enabled`, `true` gives the container a new read token; the old one stops
    /// working.
    regenerate: Option<bool>,
}

/// A subscription to make.
#[derive(Debug, ToSchema)]
#[schema(rename_all = "camelCase")]
pub(crate) struct NewSubscription {
    /// The type of the block subscribed to.
    block_type: BlockType,
    /// The identifier of the block subscribed to.
    block_id: String,
    /// The user of the session: a user subscribes for themselves only.
    user_id: String,
}

/// Whether a user is to be an administrator of the instance.
#[derive(Debug, ToSchema)]
#[schema(rename_all = "camelCase")]
pub(crate) struct AdminChange {
    /// `true` makes the user an administrator, `false` ends that.
    is_admin: bool,
}

/// What a user changes of their name. A member that is left out stays as it is.
#[derive(Debug, ToSchema)]
#[schema(rename_all = "camelCase")]
pub(crate) struct AccountChange {
    /// The given name, in the letters the person writes it in: at most 64 UTF-16 code
    /// units without control characters. Empty or `null` for none.
    first_name: Option<String>,
    /// The family name, in the letters the person writes it in; as `firstName`.
    last_name: Option<String>,
    /// The given name in Latin letters; as `firstName`.
    latin_first_name: Option<String>,
    /// The family name in Latin letters; as `firstName`.
    latin_last_name: Option<String>,
    /// The name the person wrote to be shown by; as `firstName`.
    custom_name: Option<String>,
    /// Which name the person is shown by; `null` leaves it as it is.
    name_shown: Option<NameShown>,
    /// The title written before the name; `null` for none.
    title: Option<Title>,
    /// The degrees and qualifications written after the name: at most three, each once,
    /// in the order they are written. `null` or empty for none.
    #[schema(max_items = 3)]
    credentials: Option<Vec<Credential>>,
}

/// The settings to change. A member that is left out stays as it is; one that is `null`
/// goes back to its default.
#[derive(Debug, ToSchema)]
#[schema(rename_all = "camelCase")]
pub(crate) struct SettingsChange {
    /// `auto`, for the time zone of the device, or the IANA name of a time zone, such
    /// as `Asia/Seoul`.
    time_zone: Option<String>,
    /// The day a week starts on.
    week_start: Option<WeekStart>,
    /// How the hours of a day are written.
    hour_cycle: Option<HourCycle>,
    /// The colors of the interface.
    theme: Option<Theme>,
    /// Where a card opens.
    card_open: Option<CardOpen>,
    /// How much room the interface gives each thing.
    density: Option<Density>,
    /// Whether a device that can do so answers touches with a vibration.
    haptics: Option<bool>,
    /// `auto`, for the language of the browser, or a language tag such as `ko` or
    /// `en-US`, in the shape the browsers' `Intl` reads (Unicode BCP 47), at most 64
    /// characters; the server keeps no list of languages, and keeps a tag as given.
    #[schema(max_length = 64)]
    language: Option<String>,
    /// Whether the user is told when somebody mentions them (`mention`).
    notify_mentions: Option<bool>,
    /// Whether the user is told when somebody gives them a card (`assignment`).
    notify_assignments: Option<bool>,
    /// Whether the user is told of the new comments on what they watch (`comment`).
    notify_comments: Option<bool>,
    /// Whether the user is told when a card or review is due soon (`due_soon`).
    notify_due: Option<bool>,
}

/// The bytes of a file, as the body of an answer.
#[derive(Debug, ToSchema)]
#[schema(value_type = String, format = Binary)]
pub(crate) struct Binary(Vec<u8>);

/// A form with one file.
#[derive(Debug, ToSchema)]
pub(crate) struct FileForm {
    /// The file. An upload needs the part to have a file name; an archive to import
    /// does not.
    #[schema(value_type = String, format = Binary)]
    file: Vec<u8>,
}
