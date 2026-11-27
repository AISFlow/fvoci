//! Formal comment endpoints on a card of a board: listing, writing, changing, deleting
//! and restoring comments, and reading earlier comment versions.

use axum::body::Bytes;
use axum::extract::Path;
use fvoci_model::{Block, BlockType, Body, BodyChange, CommentVersion, ContainerKinds};
use serde::Deserialize;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::comments::{
    CommentParentKind, CommenterOf, comments_on, create_comment_on, created_comment,
    delete_comment_on, existing_comment_on, find_live_comment_on, get_comment_versions_on,
    patch_comment_on, restore_comment_on,
};
use super::contract::params::{BoardId, CardCommentId, CardId, DisableNotify, MemberWorkspaceId};
use super::contract::tag;
use super::query::{DisableNotifyQuery, QueryParams};
use super::workspace::InWorkspace;
use super::{ApiResult, json_response, require_container_permission};
use super::{Delegated, Read, Write};
use crate::app::App;
use crate::error::AppError;
use crate::permissions::ContainerPermission;

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new()
        .routes(routes!(
            handle_get_card_comments,
            handle_create_card_comment
        ))
        .routes(routes!(
            handle_patch_card_comment,
            handle_delete_card_comment
        ))
        .routes(routes!(handle_get_card_comment_versions))
        .routes(routes!(handle_restore_card_comment))
}

const READ_DENIED: &str = "access denied to fetch card comments";
const CREATE_DENIED: &str = "access denied to comment on the card";
const CHANGES_DENIED: &str = "access denied to change the comments of the card";

pub(super) struct CardCommentParent;

impl CommentParentKind for CardCommentParent {
    const CONTAINER_PARAM: &'static str = "boardId";
    const PARENT_PARAM: &'static str = "cardId";
    const CREATE_DENIED: &'static str = CREATE_DENIED;
    const CHANGES_DENIED: &'static str = CHANGES_DENIED;

    fn kinds() -> ContainerKinds {
        ContainerKinds::BOARDS
    }

    async fn load_parent(
        ws: &InWorkspace,
        board_id: &str,
        card_id: &str,
    ) -> Result<Block, AppError> {
        card_on_board(ws, board_id, card_id).await
    }
}

/// The card `card_id` of the board `board_id`, live; 404 for anything else.
///
/// # Errors
///
/// Returns 404 for a block that is no card of the board, and the store's error.
pub(super) async fn card_on_board(
    ws: &InWorkspace,
    board_id: &str,
    card_id: &str,
) -> Result<Block, AppError> {
    let block = ws.app.block(card_id).await.map_err(|err| {
        if err.is_not_found() {
            AppError::not_found("card")
        } else {
            err
        }
    })?;
    if block.container_id != board_id || block.block_type != BlockType::Card {
        return Err(AppError::not_found("card"));
    }
    Ok(block)
}

/// A comment to write on a card.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NewCardComment {
    /// The comment's text, as a card's body is written; neither empty nor `null`.
    #[serde(default)]
    #[schema(value_type = Body)]
    #[expect(
        dead_code,
        reason = "body is extracted via decode_with_document while ToSchema exposes it"
    )]
    body: BodyChange,
}

/// What a comment on a card is changed to: its text, written over the comment as it was
/// read.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CardCommentPatch {
    /// The comment's new text, as a card's body is written; neither empty nor `null`.
    #[serde(default)]
    #[schema(value_type = Body)]
    #[expect(
        dead_code,
        reason = "body is extracted via decode_with_document while ToSchema exposes it"
    )]
    body: BodyChange,
    /// The `updatedAt` of the comment as the client read it: if the comment has changed
    /// since, the patch is refused with 409 and changes nothing.
    #[schema(value_type = i64)]
    read_at: Option<i64>,
}

/// The comments on a card
///
/// Every comment on the card, oldest first, as blocks of the `type` `comment` whose
/// parent is the card. For who reads the board: its members, and of an open board
/// whoever works in the workspace. A card in the trash is not there (404).
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/boards/{boardId}/cards/{cardId}/comments",
    operation_id = "getCardComments",
    tag = tag::CARDS,
    params(MemberWorkspaceId, BoardId, CardId),
    responses((status = 200, description = "The comments, oldest first.", body = Vec<Block>)),
)]
async fn handle_get_card_comments(
    Delegated(ws, ..): Delegated<Read, InWorkspace>,
    Path((_, board_id, card_id)): Path<(String, String, String)>,
) -> ApiResult {
    Ok(json_response(
        &card_comments(&ws, &board_id, &card_id).await?,
    ))
}

/// The comments on the card `card_id` of the board `board_id`, as `getCardComments`
/// lists them.
///
/// # Errors
///
/// As `getCardComments` answers.
pub(super) async fn card_comments(
    ws: &InWorkspace,
    board_id: &str,
    card_id: &str,
) -> Result<Vec<Block>, AppError> {
    require_container_permission(
        &ws.app,
        &ws.caller.user_id,
        board_id,
        ContainerPermission::View,
        READ_DENIED,
    )
    .await?;
    let card = card_on_board(ws, board_id, card_id).await?;
    comments_on(ws, board_id, &card.id).await
}

/// Write a comment on a card
///
/// For the board's members from the commenter's role up (whoever reads an open board
/// without joining it is refused with 403). The server chooses the identifier, the
/// author and the times. Every image of the text shows a file of the board (400
/// otherwise), and the text has no file block (400). Written with a personal API token,
/// the comment names the token in `fields.agentTokenName`; the server alone sets that
/// member, on every route, as the comment's text is written.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/boards/{boardId}/cards/{cardId}/comments",
    operation_id = "createCardComment",
    tag = tag::CARDS,
    params(MemberWorkspaceId, BoardId, CardId, DisableNotify),
    request_body = NewCardComment,
    responses((status = 201, description = "The comment as it was stored.", body = Block)),
)]
async fn handle_create_card_comment(
    Delegated(commenter, ..): Delegated<Write, CommenterOf<CardCommentParent>>,
    QueryParams(query): QueryParams<DisableNotifyQuery>,
    body: Bytes,
) -> ApiResult {
    let comment = create_comment_on::<NewCardComment>(
        &commenter.ws,
        &commenter.container_id,
        commenter.parent,
        "card",
        &body,
        query.notification(),
    )
    .await?;
    Ok(created_comment(&comment))
}

/// Change a comment on a card
///
/// Writes a new text over the comment: by its author from the commenter's role up, and
/// the comment of somebody else by the board's admins only (403).
#[utoipa::path(
    patch,
    path = "/w/{workspaceId}/boards/{boardId}/cards/{cardId}/comments/{commentId}",
    operation_id = "patchCardComment",
    tag = tag::CARDS,
    params(MemberWorkspaceId, BoardId, CardId, CardCommentId, DisableNotify),
    request_body = CardCommentPatch,
    responses((status = 200, description = "The comment as it is now.", body = Block)),
)]
async fn handle_patch_card_comment(
    Delegated(commenter, ..): Delegated<Write, CommenterOf<CardCommentParent>>,
    QueryParams(query): QueryParams<DisableNotifyQuery>,
    body: Bytes,
) -> ApiResult {
    let comment_id = commenter.comment_id()?;
    let comment = existing_comment_on(&commenter.ws, &commenter.parent, comment_id).await?;
    let patched = patch_comment_on::<CardCommentPatch>(
        &commenter.ws,
        &commenter.container_id,
        &comment,
        CHANGES_DENIED,
        &body,
        query.notification(),
        |patch| patch.read_at,
    )
    .await?;
    Ok(json_response(&patched))
}

/// Delete a comment on a card
///
/// Moves the comment to the trash: by its author from the commenter's role up, and the
/// comment of somebody else by the board's admins only (403).
#[utoipa::path(
    delete,
    path = "/w/{workspaceId}/boards/{boardId}/cards/{cardId}/comments/{commentId}",
    operation_id = "deleteCardComment",
    tag = tag::CARDS,
    params(MemberWorkspaceId, BoardId, CardId, CardCommentId, DisableNotify),
    responses((status = 204, description = "The comment is deleted.")),
)]
async fn handle_delete_card_comment(
    Delegated(commenter, ..): Delegated<Write, CommenterOf<CardCommentParent>>,
    QueryParams(query): QueryParams<DisableNotifyQuery>,
) -> ApiResult {
    let comment_id = commenter.comment_id()?;
    let comment = existing_comment_on(&commenter.ws, &commenter.parent, comment_id).await?;
    delete_comment_on(
        &commenter.ws,
        &comment,
        CHANGES_DENIED,
        query.notification(),
    )
    .await
}

/// Restore a deleted comment on a card
///
/// Restores a comment deleted on its own onto its card: for its author from the
/// commenter's role up if they deleted it themselves, and for the board's admins whoever
/// wrote or deleted it.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/boards/{boardId}/cards/{cardId}/comments/{commentId}/restore",
    operation_id = "restoreCardComment",
    tag = tag::CARDS,
    params(MemberWorkspaceId, BoardId, CardId, CardCommentId),
    responses((status = 200, description = "The comment as it is again.", body = Block)),
)]
async fn handle_restore_card_comment(commenter: CommenterOf<CardCommentParent>) -> ApiResult {
    let comment_id = commenter.comment_id()?;
    let comment =
        restore_comment_on(&commenter.ws, &commenter.parent, comment_id, CHANGES_DENIED).await?;
    Ok(json_response(&comment))
}

/// Earlier versions of a comment on a card
///
/// Returns every prior version of the comment's body (excluding its current text), the
/// first version first, for who reads the board.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/boards/{boardId}/cards/{cardId}/comments/{commentId}/versions",
    operation_id = "getCardCommentVersions",
    tag = tag::CARDS,
    params(MemberWorkspaceId, BoardId, CardId, CardCommentId),
    responses((status = 200, description = "The earlier versions, the first one first.", body = Vec<CommentVersion>)),
)]
async fn handle_get_card_comment_versions(
    Delegated(ws, ..): Delegated<Read, InWorkspace>,
    Path((_, board_id, card_id, comment_id)): Path<(String, String, String, String)>,
) -> ApiResult {
    require_container_permission(
        &ws.app,
        &ws.caller.user_id,
        &board_id,
        ContainerPermission::View,
        READ_DENIED,
    )
    .await?;
    let card = card_on_board(&ws, &board_id, &card_id).await?;
    let live = find_live_comment_on(&ws, &card, &comment_id).await?;
    get_comment_versions_on(&ws, &card, live, &comment_id).await
}
