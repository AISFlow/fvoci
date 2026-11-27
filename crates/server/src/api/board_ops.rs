//! Atomic board card and view endpoints: pre-copied card duplication, multi-card view
//! moves, batch card delete/restore, verified card attachments, and fractional-indexed
//! board view creation, reordering, and duplication.

use axum::body::Bytes;
use axum::extract::Path;
use axum::http::HeaderMap;
use fvoci_model::{
    Block, BoardViewDuplicate, BoardViewMove, BoardViewWrite, Card, CardBatchIds, CardMovesRequest,
    CardMovesResult, NewBoardView, NewCardAttachment, RestoredCards,
};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::contract::params::{
    AcceptLanguage, AsTemplate, BoardId, CardId, DisableNotify, MemberWorkspaceId, ViewId,
};
use super::contract::tag;
use super::query::{AsTemplateQuery, DisableNotifyQuery, QueryParams};
use super::workspace::InWorkspace;
use super::{
    ApiResult, accept_language, created, decode_object, json_response, no_content,
    require_container_permission,
};
use super::{Delegated, Write};
use crate::app::App;
use crate::permissions::ContainerPermission;

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new()
        .routes(routes!(handle_duplicate_card))
        .routes(routes!(handle_move_view_cards))
        .routes(routes!(handle_batch_delete_cards))
        .routes(routes!(handle_batch_restore_cards))
        .routes(routes!(handle_create_card_attachment))
        .routes(routes!(handle_create_board_view))
        .routes(routes!(handle_move_board_view))
        .routes(routes!(handle_duplicate_board_view))
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

/// Attach an uploaded file to a card
///
/// Verifies under the board lock that `fileId` belongs to `boardId`, sets the attachment
/// block's trusted filename and size from `files`, and stores the attachment block.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/boards/{boardId}/cards/{cardId}/attachments",
    operation_id = "createCardAttachment",
    tag = tag::CARDS,
    params(MemberWorkspaceId, BoardId, CardId, DisableNotify),
    request_body = NewCardAttachment,
    responses((status = 201, description = "The created attachment block.", body = Block)),
)]
async fn handle_create_card_attachment(
    Delegated(ws, ..): Delegated<Write, InWorkspace>,
    Path((_, board_id, card_id)): Path<(String, String, String)>,
    QueryParams(query): QueryParams<DisableNotifyQuery>,
    body: Bytes,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    require_container_permission(
        &ws.app,
        user_id,
        &board_id,
        ContainerPermission::ManageCards,
        "access denied to attach file to card",
    )
    .await?;
    let request: NewCardAttachment = decode_object(&body)?;
    let attachment = ws
        .app
        .create_card_attachment(&board_id, &card_id, &request, user_id, query.notification())
        .await?;
    Ok(created(&attachment))
}

/// Create a board view
///
/// Creates a view on `boardId` using server-side fractional indexing (`after`).
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/boards/{boardId}/views",
    operation_id = "createBoardView",
    tag = tag::CONTAINERS,
    params(MemberWorkspaceId, BoardId),
    request_body = NewBoardView,
    responses((status = 201, description = "The created view and any rebalanced companion views.", body = BoardViewWrite)),
)]
async fn handle_create_board_view(
    ws: InWorkspace,
    Path((_, board_id)): Path<(String, String)>,
    body: Bytes,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    require_container_permission(
        &ws.app,
        user_id,
        &board_id,
        ContainerPermission::ManageProperties,
        "access denied to create board view",
    )
    .await?;
    let request: NewBoardView = decode_object(&body)?;
    let write = ws
        .app
        .create_board_view(&board_id, &request, user_id)
        .await?;
    Ok(created(&write))
}

/// Move a board view
///
/// Reorders `viewId` among the views of `boardId` using server-side fractional indexing.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/boards/{boardId}/views/{viewId}/move",
    operation_id = "moveBoardView",
    tag = tag::CONTAINERS,
    params(MemberWorkspaceId, BoardId, ViewId),
    request_body = BoardViewMove,
    responses((status = 200, description = "The moved view and any rebalanced companion views.", body = BoardViewWrite)),
)]
async fn handle_move_board_view(
    ws: InWorkspace,
    Path((_, board_id, view_id)): Path<(String, String, String)>,
    body: Bytes,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    require_container_permission(
        &ws.app,
        user_id,
        &board_id,
        ContainerPermission::ManageProperties,
        "access denied to move board view",
    )
    .await?;
    let request: BoardViewMove = decode_object(&body)?;
    let write = ws
        .app
        .move_board_view(&board_id, &view_id, &request, user_id)
        .await?;
    Ok(json_response(&write))
}

/// Duplicate a board view
///
/// Copies `viewId` on `boardId` and places the copy using server-side fractional
/// indexing.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/boards/{boardId}/views/{viewId}/duplicate",
    operation_id = "duplicateBoardView",
    tag = tag::CONTAINERS,
    params(MemberWorkspaceId, BoardId, ViewId, AcceptLanguage),
    request_body = BoardViewDuplicate,
    responses((status = 201, description = "The duplicated view and any rebalanced companion views.", body = BoardViewWrite)),
)]
async fn handle_duplicate_board_view(
    ws: InWorkspace,
    Path((_, board_id, view_id)): Path<(String, String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    require_container_permission(
        &ws.app,
        user_id,
        &board_id,
        ContainerPermission::ManageProperties,
        "access denied to duplicate board view",
    )
    .await?;
    let request: BoardViewDuplicate = decode_object(&body)?;
    let write = ws
        .app
        .duplicate_board_view(
            &board_id,
            &view_id,
            &request,
            user_id,
            accept_language(&headers),
        )
        .await?;
    Ok(created(&write))
}
