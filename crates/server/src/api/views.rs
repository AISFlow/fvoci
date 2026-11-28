//! The views of a board: listing, reading, creating, changing, moving, duplicating,
//! deleting and restoring them. A view is a block of type `view`; these routes reach no
//! block of another type, and no view of another board.

use axum::body::Bytes;
use axum::extract::Path;
use axum::http::HeaderMap;
use fvoci_model::{
    Block, BlockPatch, BlockType, BoardViewDuplicate, BoardViewMove, BoardViewPatch,
    BoardViewWrite, NewBoardView,
};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::contract::params::{AcceptLanguage, BoardId, DisableNotify, MemberWorkspaceId, ViewId};
use super::contract::tag;
use super::query::{DisableNotifyQuery, QueryParams};
use super::workspace::InWorkspace;
use super::{
    ApiResult, BLOCK_FIELDS, Delegated, Read, accept_language, created, decode_object,
    json_response, no_content, require_container_permission,
};
use crate::app::{App, CREATE_VIEW_DENIED, DUPLICATE_VIEW_DENIED, MOVE_VIEW_DENIED, VIEWS_DENIED};
use crate::permissions::ContainerPermission;

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new()
        .routes(routes!(handle_get_board_views, handle_create_board_view))
        .routes(routes!(
            handle_get_board_view,
            handle_patch_board_view,
            handle_delete_board_view
        ))
        .routes(routes!(handle_restore_board_view))
        .routes(routes!(handle_move_board_view))
        .routes(routes!(handle_duplicate_board_view))
}

/// The JSON Pointer to the member of a view patch that changes the view's fields.
const PATCH_FIELDS: &str = "/updatedFields";

/// The views of a board
///
/// For whoever may read the board: its members, and for an open or built-in template
/// everyone who works in its workspace. Oldest first; the order the board shows them in
/// is their `fields.order`.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/boards/{boardId}/views",
    operation_id = "getBoardViews",
    tag = tag::CONTAINERS,
    params(MemberWorkspaceId, BoardId),
    responses((status = 200, description = "The views.", body = Vec<Block>)),
)]
async fn handle_get_board_views(
    Delegated(ws, ..): Delegated<Read, InWorkspace>,
    Path((_, board_id)): Path<(String, String)>,
) -> ApiResult {
    require_container_permission(
        &ws.app,
        &ws.caller.user_id,
        &board_id,
        ContainerPermission::View,
        "access denied to board",
    )
    .await?;
    let views = ws
        .app
        .blocks(&board_id, None, Some(&BlockType::View))
        .await?;
    Ok(json_response(&views))
}

/// A view of a board
///
/// For whoever may read the board, as `getBoardViews`. A block of the board that is no
/// view, and a view of another board, are not there (404).
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/boards/{boardId}/views/{viewId}",
    operation_id = "getBoardView",
    tag = tag::CONTAINERS,
    params(MemberWorkspaceId, BoardId, ViewId),
    responses((status = 200, description = "The view.", body = Block)),
)]
async fn handle_get_board_view(
    Delegated(ws, ..): Delegated<Read, InWorkspace>,
    Path((_, board_id, view_id)): Path<(String, String, String)>,
) -> ApiResult {
    require_container_permission(
        &ws.app,
        &ws.caller.user_id,
        &board_id,
        ContainerPermission::View,
        "access denied to board",
    )
    .await?;
    let view = ws
        .app
        .block_of_board(&board_id, &view_id, BlockType::View)
        .await?;
    Ok(json_response(&view))
}

/// Change a view of a board
///
/// Sets and removes fields of the view and changes its title; the fields are checked as
/// a view's are (400 at `/updatedFields/...`). For the board's editors. With `readAt`,
/// a view that changed since is 409 and nothing changes. The view's `modifiedBy` and
/// `updatedAt` say who changed it and when.
#[utoipa::path(
    patch,
    path = "/w/{workspaceId}/boards/{boardId}/views/{viewId}",
    operation_id = "patchBoardView",
    tag = tag::CONTAINERS,
    params(MemberWorkspaceId, BoardId, ViewId, DisableNotify),
    request_body = BoardViewPatch,
    responses((status = 200, description = "The view as it is now.", body = Block)),
)]
async fn handle_patch_board_view(
    ws: InWorkspace,
    Path((_, board_id, view_id)): Path<(String, String, String)>,
    QueryParams(query): QueryParams<DisableNotifyQuery>,
    body: Bytes,
) -> ApiResult {
    let on = ws.app.reach(&ws.caller.user_id, &board_id).await?;
    // Asked before the body is read, as every route asks, and so is the view.
    on.require(ContainerPermission::ManageCards, VIEWS_DENIED)?;
    let view = ws
        .app
        .block_of_board(&board_id, &view_id, BlockType::View)
        .await?;
    let patch: BoardViewPatch = decode_object(&body)?;
    patch.check_request()?;
    // The store names what it refuses as a block has it; the patch has it under another
    // name.
    let view = ws
        .app
        .patch_view(&on, &view, &BlockPatch::from(patch), query.notification())
        .await
        .map_err(|error| error.moved(BLOCK_FIELDS, PATCH_FIELDS))?;
    Ok(json_response(&view))
}

/// Delete a view of a board
///
/// The view goes to the trash, and `restoreBoardView` brings it back until the trash is
/// emptied of it. For the board's editors.
#[utoipa::path(
    delete,
    path = "/w/{workspaceId}/boards/{boardId}/views/{viewId}",
    operation_id = "deleteBoardView",
    tag = tag::CONTAINERS,
    params(MemberWorkspaceId, BoardId, ViewId, DisableNotify),
    responses((status = 204, description = "The view is deleted.")),
)]
async fn handle_delete_board_view(
    ws: InWorkspace,
    Path((_, board_id, view_id)): Path<(String, String, String)>,
    QueryParams(query): QueryParams<DisableNotifyQuery>,
) -> ApiResult {
    let on = ws.app.reach(&ws.caller.user_id, &board_id).await?;
    ws.app
        .delete_view(&on, &view_id, query.notification())
        .await?;
    Ok(no_content())
}

/// Bring a deleted view back
///
/// Restores a view of the board from the trash. For the board's editors. A view that is
/// not in the trash, one of another board and a block of another type are not there to
/// restore (404).
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/boards/{boardId}/views/{viewId}/restore",
    operation_id = "restoreBoardView",
    tag = tag::CONTAINERS,
    params(MemberWorkspaceId, BoardId, ViewId),
    responses((status = 200, description = "The view as it is again.", body = Block)),
)]
async fn handle_restore_board_view(
    ws: InWorkspace,
    Path((_, board_id, view_id)): Path<(String, String, String)>,
) -> ApiResult {
    let on = ws.app.reach(&ws.caller.user_id, &board_id).await?;
    let view = ws.app.restore_view(&on, &view_id).await?;
    Ok(json_response(&view))
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
    let on = ws.app.reach(&ws.caller.user_id, &board_id).await?;
    // Asked before the body is read, as every route asks.
    on.require(ContainerPermission::ManageProperties, CREATE_VIEW_DENIED)?;
    let request: NewBoardView = decode_object(&body)?;
    let write = ws.app.create_board_view(&on, &request).await?;
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
    let on = ws.app.reach(&ws.caller.user_id, &board_id).await?;
    // Asked before the body is read, as every route asks.
    on.require(ContainerPermission::ManageProperties, MOVE_VIEW_DENIED)?;
    let request: BoardViewMove = decode_object(&body)?;
    let write = ws.app.move_board_view(&on, &view_id, &request).await?;
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
    let on = ws.app.reach(&ws.caller.user_id, &board_id).await?;
    // Asked before the body is read, as every route asks.
    on.require(ContainerPermission::ManageProperties, DUPLICATE_VIEW_DENIED)?;
    let request: BoardViewDuplicate = decode_object(&body)?;
    let write = ws
        .app
        .duplicate_board_view(&on, &view_id, &request, accept_language(&headers))
        .await?;
    Ok(created(&write))
}
