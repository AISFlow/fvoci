//! Cards: the card view of blocks of type `card`.

use axum::body::Bytes;
use axum::extract::Path;
use fvoci_model::{
    Block, BlockType, BodyVersion, BodyVersionContent, Card, CardPatch, CardWithReactions,
    CompactCard, KeptBodyVersion,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::contract::bodies::NewCard;
use super::contract::params::MemberWorkspaceId;
use super::contract::params::{
    BoardId, BodyFormat, BodyVersionNumber, CardAssignee, CardId, CardNumber, CardParent,
    CardPropertyId, CardPropertyValue, CardSort, CardText, Compact, DisableNotify, Page, PerPage,
    Preview, UpdatedSince,
};
use super::contract::tag;
use super::query::{DisableNotifyQuery, QueryParams, int_param, paging_from_strings};
use super::workspace::InWorkspace;
use super::{Delegated, Read, Write};

use super::body_versions::{
    body_version_of, body_versions_of, keep_body_version_of, restore_body_version_of,
};
use super::items::{CardItem, ItemOf};
use super::{ApiResult, BLOCK_FIELDS, TRUE, created, json_response};
use super::{decode_with_document, require_container_permission, require_permission_through};
use crate::app::{
    App, CREATE_CARD_DENIED, CardPreviews, CardsFilter, CardsOrder, CardsUnder, ChangeNotification,
    NewCardRequest, PATCH_CARD_DENIED, Reached,
};
use crate::error::{AppError, OrMissing};
use crate::permissions::ContainerPermission;

/// How a card's body is given (`format`).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum BodyFormatValue {
    #[default]
    Json,
    Markdown,
}

/// The query of what is read with its body as `format` says (`getCard`, `getPage`,
/// `getUnit`).
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct BodyQuery {
    pub(super) format: BodyFormatValue,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct CardsListQuery {
    pub(super) page: String,
    pub(super) per_page: String,
    pub(super) preview: String,
    pub(super) q: String,
    pub(super) parent_id: String,
    pub(super) assignee_id: String,
    pub(super) property_id: String,
    pub(super) property_value: String,
    pub(super) updated_since: String,
    pub(super) sort: String,
    pub(super) compact: String,
}

impl CardsListQuery {
    /// Which cards the query asks for, `me` being `user_id`.
    fn filter(&self, user_id: &str) -> Result<CardsFilter, AppError> {
        let given = |text: &str| (!text.is_empty()).then(|| text.to_owned());
        let value = match (given(&self.property_id), given(&self.property_value)) {
            (Some(property), Some(value)) => Some((property, value)),
            (None, None) => None,
            (_, None) => {
                return Err(refused_parameter(
                    "propertyValue",
                    "is sent with propertyId",
                ));
            }
            (None, _) => {
                return Err(refused_parameter(
                    "propertyId",
                    "is sent with propertyValue",
                ));
            }
        };
        let order = match self.sort.as_str() {
            "" | "created_asc" => CardsOrder::CreatedFirst,
            "updated_desc" => CardsOrder::UpdatedLast,
            "number_asc" => CardsOrder::Numbered,
            _ => {
                return Err(refused_parameter(
                    "sort",
                    "is created_asc, updated_desc or number_asc",
                ));
            }
        };
        Ok(CardsFilter {
            text: given(&self.q),
            under: match self.parent_id.as_str() {
                "" => None,
                "root" => Some(CardsUnder::Top),
                parent => Some(CardsUnder::Card(parent.to_owned())),
            },
            given_to: match self.assignee_id.as_str() {
                "" => None,
                "me" => Some(user_id.to_owned()),
                user => Some(user.to_owned()),
            },
            value,
            updated_since: given(&self.updated_since)
                .map(|since| int_param(&since, "updatedSince", 0))
                .transpose()?,
            order,
        })
    }
}

fn refused_parameter(name: &str, rule: &str) -> AppError {
    AppError::bad_request(format_args!("the `{name}` parameter {rule}")).with_parameter(name)
}

/// The cards of a page of a board, whole or in short (`compact=true`).
#[derive(Debug, Serialize, ToSchema)]
#[serde(untagged)]
pub(crate) enum CardsOfPage {
    /// The cards.
    Whole(Vec<Card>),
    /// The cards in short.
    Compact(Vec<CompactCard>),
}

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new()
        .routes(routes!(handle_create_card, handle_get_cards))
        .routes(routes!(handle_get_card, handle_patch_card))
        .routes(routes!(handle_get_card_by_number))
        .routes(routes!(
            handle_get_card_body_versions,
            handle_keep_card_body_version
        ))
        .routes(routes!(handle_get_card_body_version))
        .routes(routes!(handle_restore_card_body_version))
}

/// Returns the card with the given identifier for a handler of `/cards/{cardId}`, if
/// the user has the permission on the card's board: its block, read once.
///
/// The path names a card and no board. So for a user who may not read the card's board
/// there is no such card, and neither is there for anyone if the block is no card.
pub(super) async fn card_with_permission(
    app: &App,
    user_id: &str,
    card_id: &str,
    permission: ContainerPermission,
    denial: &str,
) -> Result<Block, AppError> {
    let (_, block) = card_reached(app, user_id, card_id, permission, denial).await?;
    Ok(block)
}

/// The card's board as the user reached it and the card, as [`card_with_permission`]
/// finds them: for a route that writes the card, which the board's [`Reached`] is asked
/// for.
pub(super) async fn card_reached(
    app: &App,
    user_id: &str,
    card_id: &str,
    permission: ContainerPermission,
    denial: &str,
) -> Result<(Reached, Block), AppError> {
    const WHAT: &str = "card";
    let block = app.block(card_id).await.or_missing(WHAT)?;
    let on = app.reach(user_id, &block.container_id).await?;
    on.require_through(permission, denial, WHAT)?;
    if block.block_type != BlockType::Card {
        return Err(AppError::not_found(WHAT));
    }
    Ok((on, block))
}

/// Make a card
///
/// The server chooses the identifier, the creator and the times. For editors.
///
/// `parentId` places an ordinary card below another live, non-template card of the same
/// board, at most three levels deep (top-level = 1, child = 2, grandchild = 3); empty,
/// `null`, or omitted makes a top-level card. Template cards (`isTemplate: true`) cannot
/// have a parent or sub-cards. Invalid parents or cycles answer 400 at `/parentId`
/// (depth > 3 with reason `card_too_deep` and `limit: 3`).
///
/// `recurrence` makes the card repeat, only on a board with a `doneWhen` and a
/// `dueDatePropertyId`: elsewhere 400 at `/recurrence` with reason
/// `recurrence_needs_board_rules`.
/// A template keeps its recurrence and makes no next card.
///
/// The value of a card relation property (of the `type` `relation`) is `null` or a list
/// of relation link objects (`{"card": "<cardId>", "kind": "fs" | "ss" | "ff" | "sf",
/// "lagDays": -3650..=3650}`), naming other cards of the board, in the trash or not,
/// each once (even if `kind` differs), at most 50 (a value over it from before may be
/// written with fewer of its cards, not with another). `kind` defaults to `"fs"` and
/// `lagDays` defaults to `0` when omitted, unknown fields and plain string arrays are
/// refused with 400, and stored links always include all three fields normalized. Any
/// other refusal is 400 with the reason `relation_not_a_list`, `relation_to_itself`,
/// `relation_repeated`, `relation_too_many` (with the `limit`), `relation_not_a_card`
/// (a block of the board that is no card), `relation_cycle` (ordering dependencies form
/// a cycle on the board) or `relation_card_not_found`: no card of that
/// identifier on the board, which is also the answer for a card of another board. Every
/// write of a card checks so (`createCard`, `patchCard`, `batchPatchCards`)
/// the values it changes, and does not ask again for a card that a value named before.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/boards/{boardId}/cards",
    operation_id = "createCard",
    tag = tag::CARDS,
    params(MemberWorkspaceId, BoardId, DisableNotify),
    request_body = NewCard,
    responses((status = 201, description = "The card that was made.", body = Card)),
)]
async fn handle_create_card(
    Delegated(ws, ..): Delegated<Write, InWorkspace>,
    Path((_, board_id)): Path<(String, String)>,
    QueryParams(query): QueryParams<DisableNotifyQuery>,
    body: Bytes,
) -> ApiResult {
    let card = create_card_from(&ws, &board_id, &body, query.notification()).await?;
    Ok(created(&card))
}

/// Makes the card `body` describes, as `createCard` takes it, on the board `board_id`
/// of the workspace `ws` was entered for.
///
/// # Errors
///
/// As `createCard` answers.
pub(super) async fn create_card_from(
    ws: &InWorkspace,
    board_id: &str,
    body: &Bytes,
    notification: ChangeNotification,
) -> Result<Card, AppError> {
    let on = ws.app.reach(&ws.caller.user_id, board_id).await?;
    // Asked before the body is read, as every route asks.
    on.require(ContainerPermission::ManageCards, CREATE_CARD_DENIED)?;
    let (card, document): (Card, _) = decode_with_document(body)?;
    create_card_as(ws, &on, NewCardRequest { card, document }, notification).await
}

/// Makes the card `request` describes on the board `board_id`, as `createCard` does with
/// the body it read: for what writes as the route does.
///
/// # Errors
///
/// As `createCard` answers.
pub(super) async fn create_card_with(
    ws: &InWorkspace,
    board_id: &str,
    request: NewCardRequest,
    notification: ChangeNotification,
) -> Result<Card, AppError> {
    let on = ws.app.reach(&ws.caller.user_id, board_id).await?;
    create_card_as(ws, &on, request, notification).await
}

/// Makes the card `request` describes on the board `on` reached ([`App::create_card_on`]),
/// for `createCard` and for what writes as it does.
///
/// # Errors
///
/// As `createCard` answers.
pub(super) async fn create_card_as(
    ws: &InWorkspace,
    on: &Reached,
    request: NewCardRequest,
    notification: ChangeNotification,
) -> Result<Card, AppError> {
    // What the store refuses of the card's values it names as the block's `fields`.
    let card = ws
        .app
        .create_card_on(on, request, notification)
        .await
        .map_err(|error| error.moved(BLOCK_FIELDS, "/properties"))?;
    let (board_id, user_id) = (on.container_id(), on.user_id());
    tracing::debug!(board_id, card_id = %card.id, user_id, "CreateCard");
    Ok(card)
}

/// The cards of a board
///
/// One page of the board's cards, for the board's members. Oldest first; the pages cut
/// that order. With `preview=true` each card has its `preview` (the first image and the beginning of the
/// text of its body), which a list without it leaves out.
///
/// `q`, `parentId`, `assigneeId`, `propertyId` with `propertyValue` and `updatedSince`
/// choose the cards the page is of, all of them that are sent; `sort` orders them
/// otherwise. With `compact=true` each card is given in short (`CompactCard`).
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/boards/{boardId}/cards",
    operation_id = "getCards",
    tag = tag::CARDS,
    params(
        MemberWorkspaceId,
        BoardId,
        Page,
        PerPage,
        Preview,
        CardText,
        CardParent,
        CardAssignee,
        CardPropertyId,
        CardPropertyValue,
        UpdatedSince,
        CardSort,
        Compact
    ),
    responses((status = 200, description = "The cards of the page.", body = CardsOfPage)),
)]
async fn handle_get_cards(
    Delegated(ws, ..): Delegated<Read, InWorkspace>,
    Path((_, board_id)): Path<(String, String)>,
    QueryParams(query): QueryParams<CardsListQuery>,
) -> ApiResult {
    let cards = cards_of_board(&ws, &board_id, &query).await?;
    if query.compact == TRUE {
        let compact = cards.into_iter().map(CompactCard::from).collect();
        return Ok(json_response(&CardsOfPage::Compact(compact)));
    }
    Ok(json_response(&CardsOfPage::Whole(cards)))
}

/// The cards of the board `board_id` that `query` chooses, as `getCards` describes
/// them, for whoever reads the board; whole, whatever `compact` says.
///
/// # Errors
///
/// As `getCards` answers.
pub(super) async fn cards_of_board(
    ws: &InWorkspace,
    board_id: &str,
    query: &CardsListQuery,
) -> Result<Vec<Card>, AppError> {
    let user_id = &ws.caller.user_id;
    require_container_permission(
        &ws.app,
        user_id,
        board_id,
        ContainerPermission::View,
        "access denied to fetch cards",
    )
    .await?;
    let filter = query.filter(user_id)?;
    let cards = if filter.is_everything() {
        cards_page(&ws.app, board_id, query).await?
    } else {
        let page = paging_from_strings(&query.page, &query.per_page)?;
        ws.app
            .filtered_cards(board_id, &filter, page, previews_of(query))
            .await?
    };
    tracing::debug!(board_id, user_id, count = cards.len(), "GetCards");
    Ok(cards)
}

/// The page of the board's cards that the query of a request asks for, as `getCards`
/// describes it; for whoever may read the board.
pub(super) async fn cards_page(
    app: &App,
    board_id: &str,
    query: &CardsListQuery,
) -> Result<Vec<Card>, AppError> {
    let page = paging_from_strings(&query.page, &query.per_page)?;
    app.cards_for_board(board_id, page, previews_of(query))
        .await
}

fn previews_of(query: &CardsListQuery) -> CardPreviews {
    if query.preview == TRUE {
        CardPreviews::With
    } else {
        CardPreviews::Without
    }
}

/// Change a card
///
/// For editors of the card's board. To a user who is no member of that board there is
/// no such card. The relation values it changes are checked as `createCard` says.
/// `parentId` reparents the card within the same board (empty or `null` moves it to the
/// top level; left out keeps its current parent), checked for cycles and maximum depth 3
/// including any sub-cards below it (400 at `/parentId`, `card_too_deep` when depth > 3).
///
/// `recurrence` is checked as `createCard` says (`null` makes the card stop repeating). The
/// change that makes a card that repeats done makes its next card in the same write, once:
/// the card names it in `recurredTo` and hands it the recurrence.
///
/// `bodyMarkdown` writes the body whole from Markdown, as `getCard` with
/// `format=markdown` reads it: refused with 409 `complex_blocks_use_append` while the body
/// has blocks its Markdown does not hold as they are. With a personal API token a body
/// written whole (`body` or `bodyMarkdown`) is refused with 409 `room_has_active_editors`
/// while people may write in the card's body, as it would lose what they typed and have
/// not sent: `appendCardMarkdown` adds to it instead.
#[utoipa::path(
    patch,
    path = "/w/{workspaceId}/cards/{cardId}",
    operation_id = "patchCard",
    tag = tag::CARDS,
    params(MemberWorkspaceId, CardId, DisableNotify),
    request_body = CardPatch,
    responses((status = 200, description = "The card as it is now.", body = Card)),
)]
async fn handle_patch_card(
    Delegated(ws, ..): Delegated<Write, InWorkspace>,
    Path((_, card_id)): Path<(String, String)>,
    QueryParams(query): QueryParams<DisableNotifyQuery>,
    body: Bytes,
) -> ApiResult {
    let patched = patch_card_from(&ws, &card_id, &body, query.notification()).await?;
    Ok(json_response(&patched))
}

/// Changes the card `card_id` as `body` says, as `patchCard` takes it, for whoever
/// entered its workspace as `ws`.
///
/// # Errors
///
/// As `patchCard` answers.
pub(super) async fn patch_card_from(
    ws: &InWorkspace,
    card_id: &str,
    body: &Bytes,
    notification: ChangeNotification,
) -> Result<Card, AppError> {
    let user_id = ws.caller.user_id.as_str();
    let editing = ContainerPermission::ManageCards;
    let (on, card) = card_reached(&ws.app, user_id, card_id, editing, PATCH_CARD_DENIED).await?;
    // The block, read to check the permission, is the card as it was.
    let (mut patch, document): (CardPatch, _) = decode_with_document(body)?;
    patch.body = document;
    patch_card_as(ws, &on, &card, patch, notification).await
}

/// Changes the card `card_id` as `patch` says, as `patchCard` does with the body it read:
/// for what writes as the route does.
///
/// # Errors
///
/// As `patchCard` answers.
pub(super) async fn patch_card_with(
    ws: &InWorkspace,
    card_id: &str,
    patch: CardPatch,
    notification: ChangeNotification,
) -> Result<Card, AppError> {
    let user_id = ws.caller.user_id.as_str();
    let editing = ContainerPermission::ManageCards;
    let (on, card) = card_reached(&ws.app, user_id, card_id, editing, PATCH_CARD_DENIED).await?;
    patch_card_as(ws, &on, &card, patch, notification).await
}

/// Changes `card` of the board `on` reached as `patch` says ([`App::patch_card_on`]),
/// as the agent of the caller's token if one writes, for `patchCard` and for what
/// writes as it does.
///
/// # Errors
///
/// As `patchCard` answers.
pub(super) async fn patch_card_as(
    ws: &InWorkspace,
    on: &Reached,
    card: &Block,
    patch: CardPatch,
    notification: ChangeNotification,
) -> Result<Card, AppError> {
    let patched = ws
        .app
        .patch_card_on(on, card, patch, notification, ws.caller.agent())
        .await
        .map_err(|error| error.moved(BLOCK_FIELDS, "/updatedProperties"))?;
    let user_id = on.user_id();
    tracing::debug!(board_id = %patched.board_id, card_id = %patched.id, user_id, "PatchCard");
    Ok(patched)
}

/// A card
///
/// For the members of the card's board, with its `reactions` where it has any. To a
/// user who is no member of it there is no such card.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/cards/{cardId}",
    operation_id = "getCard",
    tag = tag::CARDS,
    params(MemberWorkspaceId, CardId, BodyFormat),
    responses((status = 200, description = "The card.", body = CardWithReactions)),
)]
async fn handle_get_card(
    Delegated(ws, ..): Delegated<Read, InWorkspace>,
    Path((_, card_id)): Path<(String, String)>,
    QueryParams(query): QueryParams<BodyQuery>,
) -> ApiResult {
    let card = card_read(&ws, &card_id, query.format).await?;
    Ok(json_response(&with_its_reactions(&ws, card).await?))
}

/// The card with the reactions to it, as `getCard` gives it.
///
/// # Errors
///
/// Returns the store's error.
async fn with_its_reactions(ws: &InWorkspace, card: Card) -> Result<CardWithReactions, AppError> {
    let reactions = ws.app.reactions_to(&card.id).await?;
    Ok(CardWithReactions { card, reactions })
}

/// The card `card_id`, with its body as `format` says, as `getCard` gives it.
///
/// # Errors
///
/// As `getCard` answers.
pub(super) async fn card_read(
    ws: &InWorkspace,
    card_id: &str,
    format: BodyFormatValue,
) -> Result<Card, AppError> {
    let user_id = ws.caller.user_id.as_str();
    let block = card_with_permission(
        &ws.app,
        user_id,
        card_id,
        ContainerPermission::View,
        "access denied to fetch card",
    )
    .await?;
    let mut card = ws.app.card_of_block(&block).await?;
    if format == BodyFormatValue::Markdown {
        ws.app
            .put_card_in_markdown(ws.markdown_reader(), &mut card)
            .await?;
    }
    tracing::debug!(board_id = %card.board_id, card_id = %card.id, user_id, "GetCard");
    Ok(card)
}

/// A card by its number
///
/// The card of the board that has the number (`Card.number`, as `#12` names the card),
/// for the members of the board, as `getCard` gives it. One answer, 404, says that there
/// is no such card, whatever the reason: no card of the board has the number (the card
/// is in the trash, is gone for good, or is a template of the board's cards), the
/// number is none, the board is one the user does not read (a private board they are
/// no member of, one they only manage, one they are no guest of), or is no board of
/// this workspace (of another workspace or organization, a wiki, or none at all).
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/boards/{boardId}/cards/by-number/{number}",
    operation_id = "getCardByNumber",
    tag = tag::CARDS,
    params(MemberWorkspaceId, BoardId, CardNumber),
    responses((status = 200, description = "The card.", body = CardWithReactions)),
)]
async fn handle_get_card_by_number(
    Delegated(ws, ..): Delegated<Read, InWorkspace>,
    Path((_, board_id, number)): Path<(String, String, String)>,
) -> ApiResult {
    let card = card_by_number_read(&ws, &board_id, &number).await?;
    Ok(json_response(&with_its_reactions(&ws, card).await?))
}

/// The card of the board `board_id` that has `number`, as `getCardByNumber` gives it.
///
/// # Errors
///
/// As `getCardByNumber` answers.
pub(super) async fn card_by_number_read(
    ws: &InWorkspace,
    board_id: &str,
    number: &str,
) -> Result<Card, AppError> {
    const WHAT: &str = "card";
    let user_id = ws.caller.user_id.as_str();
    let denial = "access denied to fetch card";
    let view = ContainerPermission::View;
    require_permission_through(&ws.app, user_id, board_id, view, denial, WHAT).await?;
    // A number as `Card.number` writes it, so that a card has one path: not `+7` or `07`.
    let number = number
        .parse::<u32>()
        .ok()
        .filter(|parsed| parsed.to_string() == number)
        .ok_or_else(|| AppError::not_found(WHAT))?;
    let block = ws
        .app
        .card_by_number(board_id, number)
        .await?
        .ok_or_else(|| AppError::not_found(WHAT))?;
    let card = ws.app.card_of_block(&block).await?;
    tracing::debug!(board_id, card_id = %card.id, user_id, "GetCardByNumber");
    Ok(card)
}

/// The versions kept of a card's body
///
/// The server keeps the body of a card as a version each time it changed: when the
/// last of those who wrote it since the newest version leaves it, at an interval
/// while it is edited on (its setting `collab_version_minutes`, ten minutes unless
/// set otherwise), and before and after a write of the whole body (`patchCard`). The
/// newest first, each with when it was kept and who wrote the body
/// up to it, without the bodies (`getCardBodyVersion`). The server keeps the newest
/// of them (its setting `collab_body_versions`, fifty unless set otherwise), so the
/// numbers may leave the lowest out. Empty for a card whose body never changed since
/// versions were kept. For the members of the card's board, whatever
/// their role; to a user who is no member of it there is no such card, and a reader by
/// the board's read token has no session to ask with. A card in the trash is not
/// there (404); its versions come back with it, and go when it is gone for good.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/cards/{cardId}/body-versions",
    operation_id = "getCardBodyVersions",
    tag = tag::CARDS,
    params(MemberWorkspaceId, CardId),
    responses((status = 200, description = "The versions, the newest first.", body = Vec<BodyVersion>)),
)]
async fn handle_get_card_body_versions(
    Delegated(card, ..): Delegated<Read, ItemOf<CardItem>>,
) -> ApiResult {
    body_versions_of(card).await
}

/// Keep a card's body as a version now
///
/// Keeps the body the card has as a version, by hand (`cause` `manual`), as an editor
/// asks for one: what those who edit the card together typed is written to it first, so
/// that the version is their body of that moment, and what they see and type goes on as
/// it was. The version names who wrote the body since the one before, as every version
/// does, and is as of now; the caller is not named for asking. It counts with the
/// others against the versions kept, the oldest going. After it, the next version the
/// server keeps on its own is of what is typed after it.
///
/// Nothing is kept (`outcome` `unchanged`, `version` the newest) of a body that is the
/// card's newest version already, nor (`outcome` `recent`, `version` that one) within
/// a minute of a version kept by hand, the server then keeping what was typed since on
/// its own. For editors of the card's board, as `restoreCardBodyVersion` is (403 for
/// its other members); to a user who is no member of it there is no such card, and a
/// card in the trash is not there (404). 503 (`document_unavailable`) if what was typed
/// cannot be written to the card now.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/cards/{cardId}/body-versions",
    operation_id = "keepCardBodyVersion",
    tag = tag::CARDS,
    params(MemberWorkspaceId, CardId),
    responses((status = 200, description = "What became of the body.", body = KeptBodyVersion)),
)]
async fn handle_keep_card_body_version(card: ItemOf<CardItem>) -> ApiResult {
    keep_body_version_of(card).await
}

/// A version kept of a card's body
///
/// The version with its body, as `getCard` gives a body. For whom
/// `getCardBodyVersions` is for. A number the card has no version of, or has none of
/// any more, is 404.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/cards/{cardId}/body-versions/{version}",
    operation_id = "getCardBodyVersion",
    tag = tag::CARDS,
    params(MemberWorkspaceId, CardId, BodyVersionNumber),
    responses((status = 200, description = "The version.", body = BodyVersionContent)),
)]
async fn handle_get_card_body_version(
    Delegated(card, ..): Delegated<Read, ItemOf<CardItem>>,
) -> ApiResult {
    body_version_of(card).await
}

/// Go back to a version of a card's body
///
/// Writes the body of the version to the card again, as a write of the whole body by
/// the caller: those who edit the card together are sent the body as a new document
/// (`contracts/websocket.md`), the body it replaces is kept as a version first, with
/// what they typed, and the body written is the card's newest version, by the caller.
/// The versions before stay as they are. A card that has that body already is left as
/// it is. For editors of the card's board (403 for its other members); to a user who is
/// no member of it there is no such card. A number the card has no version of, or has
/// none of any more, is 404. A version whose body may not be stored any more, or names
/// a file (an image, a file block) the board no longer has, is 409.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/cards/{cardId}/body-versions/{version}/restore",
    operation_id = "restoreCardBodyVersion",
    tag = tag::CARDS,
    params(MemberWorkspaceId, CardId, BodyVersionNumber),
    responses((status = 200, description = "The card as it is now.", body = Card)),
)]
async fn handle_restore_card_body_version(card: ItemOf<CardItem>) -> ApiResult {
    restore_body_version_of(card).await
}
