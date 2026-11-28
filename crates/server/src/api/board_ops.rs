//! Atomic card endpoints of a board: pre-copied card duplication, multi-card view moves,
//! and batch card patch, delete and restore.

use axum::body::Bytes;
use axum::extract::Path;
use fvoci_model::{
    Card, CardBatchIds, CardBatchPatch, CardMovesRequest, CardMovesResult, MAX_CARD_BATCH,
    RestoredCards,
};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::contract::params::{
    AsTemplate, BoardId, CardId, DisableNotify, MemberWorkspaceId, ViewId,
};
use super::contract::tag;
use super::query::{AsTemplateQuery, DisableNotifyQuery, QueryParams};
use super::workspace::InWorkspace;
use super::{
    ApiResult, BLOCK_FIELDS, created, decode_object, json_response, no_content,
    require_container_permission,
};
use super::{Delegated, Write};
use crate::app::{App, ChangeNotification};
use crate::error::AppError;
use crate::permissions::ContainerPermission;

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new()
        .routes(routes!(handle_duplicate_card))
        .routes(routes!(handle_move_view_cards))
        .routes(routes!(handle_batch_patch_cards))
        .routes(routes!(handle_batch_delete_cards))
        .routes(routes!(handle_batch_restore_cards))
}

/// Duplicate a card atomically
///
/// Pre-copies referenced storage files under `loose_objects` marks and commits the
/// duplicated card, sub-cards, attachment blocks, `files` records, and initial
/// `body_versions` in a single database transaction. If any storage file copy fails, no
/// cloned blocks or dangling `fileId` references are stored.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/boards/{boardId}/cards/{cardId}/duplicate",
    operation_id = "duplicateCard",
    tag = tag::CARDS,
    params(MemberWorkspaceId, BoardId, CardId, AsTemplate),
    responses((status = 201, description = "The duplicated card.", body = Card)),
)]
async fn handle_duplicate_card(
    ws: InWorkspace,
    Path((_, board_id, card_id)): Path<(String, String, String)>,
    QueryParams(query): QueryParams<AsTemplateQuery>,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    require_container_permission(
        &ws.app,
        user_id,
        &board_id,
        ContainerPermission::ManageCards,
        "access denied to duplicate card",
    )
    .await?;
    let card = ws
        .app
        .duplicate_card(&board_id, &card_id, user_id, query.duplicate_as())
        .await?;
    Ok(created(&card))
}

/// Move one or more cards within a board view
///
/// Updates `cardOrder` on `viewId` and optionally assigns `groupValue` to
/// `groupPropertyId` on `1..=200` distinct cards in a single database transaction,
/// validating card property types/options and emitting assignee notifications.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/boards/{boardId}/views/{viewId}/card-moves",
    operation_id = "moveViewCards",
    tag = tag::CARDS,
    params(MemberWorkspaceId, BoardId, ViewId, DisableNotify),
    request_body = CardMovesRequest,
    responses((status = 200, description = "Updated view and modified cards.", body = CardMovesResult)),
)]
async fn handle_move_view_cards(
    ws: InWorkspace,
    Path((_, board_id, view_id)): Path<(String, String, String)>,
    QueryParams(query): QueryParams<DisableNotifyQuery>,
    body: Bytes,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    require_container_permission(
        &ws.app,
        user_id,
        &board_id,
        ContainerPermission::ManageCards,
        "access denied to move cards",
    )
    .await?;
    let request: CardMovesRequest = decode_object(&body)?;
    let result = ws
        .app
        .move_cards_in_view(&board_id, &view_id, &request, user_id, query.notification())
        .await?;
    Ok(json_response(&result))
}

/// Change several cards alike
///
/// Sets and removes the same property values of `1..=200` distinct cards of the board,
/// all or none in one transaction: each card is changed as `patchCard` with only
/// `updatedProperties` and `deletedProperties` would change it, its values checked alike
/// against the board's properties, and whom it is newly given to told alike. More cards
/// are 400 with the `limit`. A card of another board, or a block that is no card, is not
/// there (404); a template of the board's cards is 400. For the board's editors.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/boards/{boardId}/cards/batch-patch",
    operation_id = "batchPatchCards",
    tag = tag::CARDS,
    params(MemberWorkspaceId, BoardId, DisableNotify),
    request_body = CardBatchPatch,
    responses((status = 200, description = "The cards as they are now, in the order of `cardIds`.", body = Vec<Card>)),
)]
async fn handle_batch_patch_cards(
    Delegated(ws, ..): Delegated<Write, InWorkspace>,
    Path((_, board_id)): Path<(String, String)>,
    QueryParams(query): QueryParams<DisableNotifyQuery>,
    body: Bytes,
) -> ApiResult {
    let cards = batch_patch_cards_from(&ws, &board_id, &body, query.notification()).await?;
    Ok(json_response(&cards))
}

/// What `batchPatchCards` does with `body` on the board `board_id`: the cards as they
/// are after it, in the order of `cardIds`.
///
/// # Errors
///
/// As `batchPatchCards` answers.
async fn batch_patch_cards_from(
    ws: &InWorkspace,
    board_id: &str,
    body: &Bytes,
    notification: ChangeNotification,
) -> Result<Vec<Card>, AppError> {
    // Asked before the body is read, as every route asks.
    may_patch_cards(ws, board_id).await?;
    let batch: CardBatchPatch = decode_object(body)?;
    patch_cards_asked(ws, board_id, &batch, notification).await
}

/// Patches the cards `batch` names on the board `board_id`, as `batchPatchCards` does
/// with the body it read: for what writes as the route does (the MCP server's
/// `save_cards`).
///
/// # Errors
///
/// As `batchPatchCards` answers.
pub(super) async fn batch_patch_cards_with(
    ws: &InWorkspace,
    board_id: &str,
    batch: &CardBatchPatch,
    notification: ChangeNotification,
) -> Result<Vec<Card>, AppError> {
    may_patch_cards(ws, board_id).await?;
    patch_cards_asked(ws, board_id, batch, notification).await
}

async fn may_patch_cards(ws: &InWorkspace, board_id: &str) -> Result<(), AppError> {
    require_container_permission(
        &ws.app,
        &ws.caller.user_id,
        board_id,
        ContainerPermission::ManageCards,
        "access denied to patch cards",
    )
    .await
}

/// [`batch_patch_cards_with`] once its caller may patch the board's cards.
async fn patch_cards_asked(
    ws: &InWorkspace,
    board_id: &str,
    batch: &CardBatchPatch,
    notification: ChangeNotification,
) -> Result<Vec<Card>, AppError> {
    if batch.card_ids.len() > MAX_CARD_BATCH {
        return Err(AppError::bad_request(format_args!(
            "at most {MAX_CARD_BATCH} cards are patched at once"
        ))
        .with_field("cardIds")
        .with_limit(u64::try_from(MAX_CARD_BATCH).unwrap_or(u64::MAX)));
    }
    batch.check_request()?;
    // The store names what it refuses as a block has it; the batch has it under another
    // name, as `patchCard` does.
    ws.app
        .patch_cards(board_id, batch, &ws.caller.user_id, notification)
        .await
        .map_err(|error| error.moved(BLOCK_FIELDS, "/updatedProperties"))
}

/// Delete multiple cards atomically
///
/// Deletes `1..=200` distinct cards on `boardId` in one transaction so each card remains
/// individually restorable from the trash.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/boards/{boardId}/cards/batch-delete",
    operation_id = "batchDeleteCards",
    tag = tag::CARDS,
    params(MemberWorkspaceId, BoardId, DisableNotify),
    request_body = CardBatchIds,
    responses((status = 204, description = "The cards are deleted.")),
)]
async fn handle_batch_delete_cards(
    ws: InWorkspace,
    Path((_, board_id)): Path<(String, String)>,
    QueryParams(query): QueryParams<DisableNotifyQuery>,
    body: Bytes,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    require_container_permission(
        &ws.app,
        user_id,
        &board_id,
        ContainerPermission::ManageCards,
        "access denied to delete cards",
    )
    .await?;
    let batch: CardBatchIds = decode_object(&body)?;
    ws.app
        .batch_delete_cards(&board_id, &batch.card_ids, user_id, query.notification())
        .await?;
    Ok(no_content())
}

/// Restore multiple deleted cards atomically
///
/// Restores `1..=200` distinct deleted cards on `boardId` in one transaction and returns
/// all restored cards (including restored sub-cards).
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/boards/{boardId}/cards/batch-restore",
    operation_id = "batchRestoreCards",
    tag = tag::CARDS,
    params(MemberWorkspaceId, BoardId),
    request_body = CardBatchIds,
    responses((status = 200, description = "The restored cards.", body = RestoredCards)),
)]
async fn handle_batch_restore_cards(
    ws: InWorkspace,
    Path((_, board_id)): Path<(String, String)>,
    body: Bytes,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    require_container_permission(
        &ws.app,
        user_id,
        &board_id,
        ContainerPermission::ManageCards,
        "access denied to restore cards",
    )
    .await?;
    let batch: CardBatchIds = decode_object(&body)?;
    let restored = ws
        .app
        .batch_restore_cards(&board_id, &batch.card_ids, user_id)
        .await?;
    Ok(json_response(&restored))
}
