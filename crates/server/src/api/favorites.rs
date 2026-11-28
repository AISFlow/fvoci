//! The stars of the user of the session in a workspace: what they keep at hand at the
//! head of their sidebar and of their search.

use axum::body::Bytes;
use axum::extract::Path;
use fvoci_model::Favorite;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::contract::params::{FavoriteTargetId, MemberWorkspaceId};
use super::contract::tag;
use super::workspace::InLinked;
use super::{ApiResult, decode_json, json_response, no_content};
use crate::app::App;

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new()
        .routes(routes!(handle_get_favorites))
        .routes(routes!(handle_put_favorite, handle_delete_favorite))
        .routes(routes!(handle_reorder_favorites))
}

/// The user's favorites
///
/// The stars of the user of the session in the workspace, in their order (as
/// `reorderFavorites` put them, a new one after the others): on boards, wikis and
/// courses, and on cards, pages and units of them. Each is asked again whether the user
/// reads what it is on, as every route to it asks, and is listed with that as it is
/// now: its title, its icon and where it is. What they do not read now is left out, and
/// nothing says that it was there: what is in the trash, what is of a board they left
/// or that went private, what is gone; once it is there to them again, so is its star.
/// Nobody else's stars are listed anywhere, to anybody. A user has at most 100 in a
/// workspace. A session alone: a personal API token is refused.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/favorites",
    operation_id = "getFavorites",
    tag = tag::SIDEBAR,
    params(MemberWorkspaceId),
    responses((status = 200, description = "The favorites, in the user's order.", body = Vec<Favorite>)),
)]
async fn handle_get_favorites(InLinked(ws): InLinked) -> ApiResult {
    let favorites = ws
        .app
        .favorites(&ws.caller.user_id, &ws.workspace_id)
        .await?;
    Ok(json_response(&favorites))
}

/// Put a star on something
///
/// Puts a star of the user of the session on a board, a wiki or a course of the
/// workspace they read, or on a card, a page or a unit of one, after their other stars;
/// the server finds which it is. One that is there stays where it is. 404 for what the
/// user does not read, for a diary and an entry of one, for a template, and for an
/// identifier of nothing, all alike. Past 100 in the workspace it is 400 with the reason
/// `too_many_favorites`. The user's connections to the workspace are told
/// (`favoritesChanged`). A session alone: a personal API token is refused.
#[utoipa::path(
    put,
    path = "/w/{workspaceId}/favorites/{targetId}",
    operation_id = "putFavorite",
    tag = tag::SIDEBAR,
    params(MemberWorkspaceId, FavoriteTargetId),
    responses((status = 204, description = "The star is on it.")),
)]
async fn handle_put_favorite(
    InLinked(ws): InLinked,
    Path((_, target_id)): Path<(String, String)>,
) -> ApiResult {
    ws.app
        .add_favorite(&ws.caller.user_id, &ws.workspace_id, &target_id)
        .await?;
    Ok(no_content())
}

/// Take a star off
///
/// Takes the star of the user of the session off what it is on. 404 where they have no
/// star on it in this workspace, whatever the identifier names (taken off already, in
/// another workspace, or nothing): one answer for all. The user's connections to the
/// workspace are told where one went (`favoritesChanged`). A session alone: a personal
/// API token is refused.
#[utoipa::path(
    delete,
    path = "/w/{workspaceId}/favorites/{targetId}",
    operation_id = "deleteFavorite",
    tag = tag::SIDEBAR,
    params(MemberWorkspaceId, FavoriteTargetId),
    responses((status = 204, description = "There is no star on it.")),
)]
async fn handle_delete_favorite(
    InLinked(ws): InLinked,
    Path((_, target_id)): Path<(String, String)>,
) -> ApiResult {
    ws.app
        .remove_favorite(&ws.caller.user_id, &ws.workspace_id, &target_id)
        .await?;
    Ok(no_content())
}

/// Put the favorites in order
///
/// Puts the stars of the user of the session in the workspace in the order of the body,
/// a list of what they are on by identifier: those it names first, as it names them,
/// and the others after them in the order they had. An identifier of no star of theirs
/// is passed over. 400 for more than 100 identifiers, and for an entry of no form of
/// one. The user's connections to the workspace are told (`favoritesChanged`). A
/// session alone: a personal API token is refused.
#[utoipa::path(
    put,
    path = "/w/{workspaceId}/favorite-order",
    operation_id = "reorderFavorites",
    tag = tag::SIDEBAR,
    params(MemberWorkspaceId),
    request_body = Vec<String>,
    responses((status = 204, description = "They are in that order.")),
)]
async fn handle_reorder_favorites(InLinked(ws): InLinked, body: Bytes) -> ApiResult {
    let target_ids: Vec<String> = decode_json(&body)?;
    ws.app
        .reorder_favorites(&ws.caller.user_id, &ws.workspace_id, &target_ids)
        .await?;
    Ok(no_content())
}
