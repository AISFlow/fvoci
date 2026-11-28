//! Lists of what is in the trash: the deleted cards of a board, and the deleted boards
//! a user may bring back.

use axum::extract::Path;
use fvoci_model::DeletedItem;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::contract::params::MemberWorkspaceId;
use super::contract::params::{BoardId, Page, PerPage, WorkspaceId};
use super::contract::tag;
use super::query::{PagingQuery, QueryParams};
use super::workspace::{InContainer, InWorkspace};
use super::{ApiResult, json_response, require_container_permission};
use crate::app::App;
use crate::permissions::ContainerPermission;

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new()
        .routes(routes!(handle_get_deleted_cards))
        .routes(routes!(handle_get_deleted_containers))
}

/// The deleted cards of a board
///
/// One page of the board's cards that are in the trash, each with its identifier, its
/// title, when it was deleted and by whom; `batchRestoreCards` brings one back until the
/// trash is emptied of it. A card that went with the block it was on (another card, a
/// view) comes back with that block, and is not listed. For the board's editors. The
/// last deleted first; the pages cut that order.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/boards/{boardId}/trash/cards",
    operation_id = "getDeletedCards",
    tag = tag::CARDS,
    params(MemberWorkspaceId, BoardId, Page, PerPage),
    responses((status = 200, description = "The deleted cards of the page.", body = Vec<DeletedItem>)),
)]
async fn handle_get_deleted_cards(
    ws: InWorkspace,
    Path((_, board_id)): Path<(String, String)>,
    QueryParams(query): QueryParams<PagingQuery>,
) -> ApiResult {
    require_container_permission(
        &ws.app,
        &ws.caller.user_id,
        &board_id,
        ContainerPermission::ManageCards,
        "access denied to the trash of the board",
    )
    .await?;
    let page = query.into_list_page()?;
    Ok(json_response(&ws.app.deleted_cards(&board_id, page).await?))
}

/// The deleted boards of a workspace
///
/// One page of the workspace's boards that are in the trash and that the user was an admin
/// of when they were deleted, or every one of them for an administrator of the
/// workspace, which is who `restoreContainer` lets bring them back: each
/// with its identifier, its title, when it was deleted and by whom. The last deleted
/// first; the pages cut that order. For users of the workspace.
///
/// The wikis in the trash are in the same list, which go there and come back as boards
/// do (`deleteContainer`, `restoreContainer`): each entry says its `kind`.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/trash/containers",
    operation_id = "getDeletedContainers",
    tag = tag::CONTAINERS,
    params(WorkspaceId, Page, PerPage),
    responses((status = 200, description = "The deleted boards of the page.", body = Vec<DeletedItem>)),
)]
async fn handle_get_deleted_containers(
    InContainer(ws): InContainer,
    QueryParams(query): QueryParams<PagingQuery>,
) -> ApiResult {
    let ws = ws.require_no_guest()?;
    let page = query.into_list_page()?;
    Ok(json_response(
        &ws.app
            .deleted_containers(&ws.workspace_id, &ws.caller.user_id, ws.role, page)
            .await?,
    ))
}
