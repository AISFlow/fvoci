//! The attachments of a card: listing, adding, deleting and restoring them. An attachment
//! is a block of type `attachment` below its card; these routes reach no block of
//! another type, and no attachment of another card.

use axum::body::Bytes;
use axum::extract::Path;
use fvoci_model::{Block, BlockType, NewCardAttachment};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::contract::params::{AttachmentId, BoardId, CardId, DisableNotify, MemberWorkspaceId};
use super::contract::tag;
use super::query::{DisableNotifyQuery, QueryParams};
use super::workspace::InWorkspace;
use super::{
    ApiResult, Delegated, Read, Write, created, decode_object, json_response, no_content,
    require_container_permission,
};
use crate::app::{ATTACH_DENIED, App};
use crate::permissions::ContainerPermission;

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new()
        .routes(routes!(
            handle_get_card_attachments,
            handle_create_card_attachment
        ))
        .routes(routes!(handle_delete_card_attachment))
        .routes(routes!(handle_restore_card_attachment))
}

/// The attachments of a card
///
/// For whoever may read the board: its members, and for an open or built-in template
/// everyone who works in its workspace. A card of another board, and a block that is no
/// card, are not there (404). Oldest first.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/boards/{boardId}/cards/{cardId}/attachments",
    operation_id = "getCardAttachments",
    tag = tag::CARDS,
    params(MemberWorkspaceId, BoardId, CardId),
    responses((status = 200, description = "The attachments.", body = Vec<Block>)),
)]
async fn handle_get_card_attachments(
    Delegated(ws, ..): Delegated<Read, InWorkspace>,
    Path((_, board_id, card_id)): Path<(String, String, String)>,
) -> ApiResult {
    require_container_permission(
        &ws.app,
        &ws.caller.user_id,
        &board_id,
        ContainerPermission::View,
        "access denied to board",
    )
    .await?;
    ws.app
        .block_of_board(&board_id, &card_id, BlockType::Card)
        .await?;
    let attachments = ws
        .app
        .blocks(&board_id, Some(&card_id), Some(&BlockType::Attachment))
        .await?;
    Ok(json_response(&attachments))
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
    let on = ws.app.reach(&ws.caller.user_id, &board_id).await?;
    // Asked before the body is read, as every route asks.
    on.require(ContainerPermission::ManageCards, ATTACH_DENIED)?;
    let request: NewCardAttachment = decode_object(&body)?;
    let attachment = ws
        .app
        .create_card_attachment(&on, &card_id, &request, query.notification())
        .await?;
    Ok(created(&attachment))
}

/// Delete an attachment of a card
///
/// The attachment goes to the trash, and `restoreCardAttachment` brings it back until
/// the trash is emptied of it; its file stays with the board. For the board's editors.
/// An attachment of another card is not there (404).
#[utoipa::path(
    delete,
    path = "/w/{workspaceId}/boards/{boardId}/cards/{cardId}/attachments/{attachmentId}",
    operation_id = "deleteCardAttachment",
    tag = tag::CARDS,
    params(MemberWorkspaceId, BoardId, CardId, AttachmentId, DisableNotify),
    responses((status = 204, description = "The attachment is deleted.")),
)]
async fn handle_delete_card_attachment(
    ws: InWorkspace,
    Path((_, board_id, card_id, attachment_id)): Path<(String, String, String, String)>,
    QueryParams(query): QueryParams<DisableNotifyQuery>,
) -> ApiResult {
    let on = ws.app.reach(&ws.caller.user_id, &board_id).await?;
    ws.app
        .delete_card_attachment(&on, &card_id, &attachment_id, query.notification())
        .await?;
    Ok(no_content())
}

/// Bring a deleted attachment back
///
/// Restores an attachment of the card from the trash, where it went on its own: one that
/// went there with its card comes back with the card (`batchRestoreCards`). For the
/// board's editors. An attachment that is not in the trash, one of another card and a
/// block of another type are not there to restore (404).
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/boards/{boardId}/cards/{cardId}/attachments/{attachmentId}/restore",
    operation_id = "restoreCardAttachment",
    tag = tag::CARDS,
    params(MemberWorkspaceId, BoardId, CardId, AttachmentId),
    responses((status = 200, description = "The attachment as it is again.", body = Block)),
)]
async fn handle_restore_card_attachment(
    ws: InWorkspace,
    Path((_, board_id, card_id, attachment_id)): Path<(String, String, String, String)>,
) -> ApiResult {
    let on = ws.app.reach(&ws.caller.user_id, &board_id).await?;
    let attachment = ws
        .app
        .restore_card_attachment(&on, &card_id, &attachment_id)
        .await?;
    Ok(json_response(&attachment))
}
