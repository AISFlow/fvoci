//! Reactions: an emoji that someone who may comment in a board, a wiki or a course
//! answers one of its comments, cards, pages or units with, and takes off again. The
//! lists of comments and the reads of a card, a page and a unit give them
//! (`CommentWithReactions` and the others); the container's members are told of them
//! as they change (`reactionsChanged`). A public link's reader is given none.

use axum::extract::Path;
use fvoci_model::{Block, ContainerKinds, Emoji};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::contract::params::{
    MemberWorkspaceId, ReactedBlockId, ReactedContainerId, ReactionEmoji,
};
use super::contract::tag;
use super::workspace::{InContainer, InWorkspace};
use super::{ApiResult, Delegated, Write, no_content};
use crate::app::{App, Reached};
use crate::error::AppError;
use crate::permissions::ContainerPermission;

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new().routes(routes!(handle_add_reaction, handle_remove_reaction))
}

const DENIED: &str = "access denied to react in the container";

/// React to a comment, a card, a page or a unit
///
/// Adds the user's reaction with the emoji to the block: a comment, a card, a page or a
/// unit of a board, a wiki or a course. For its members from the commenter's role up
/// (whoever reads an open board without joining it is refused with 403, as a viewer
/// is); a diary's entries are their owner's alone, and a diary is not there (404). One
/// that is there already is no error, and nothing changes. A block has at most 20
/// different emoji: one more is 400 with the reason `too_many_reactions` and the
/// `limit`, and nothing changes; anybody's emoji that is there is taken. Neither the
/// block's `updatedAt` nor who changed it last changes, and nobody is notified; the
/// members who read the container are told the block's reactions as they are now
/// (`reactionsChanged`).
#[utoipa::path(
    put,
    path = "/w/{workspaceId}/containers/{containerId}/blocks/{blockId}/reactions/{emoji}",
    operation_id = "addReaction",
    tag = tag::CONTAINERS,
    params(MemberWorkspaceId, ReactedContainerId, ReactedBlockId, ReactionEmoji),
    responses((status = 204, description = "The reaction is there.")),
)]
async fn handle_add_reaction(
    Delegated(InContainer(ws), ..): Delegated<Write, InContainer>,
    Path((_, container_id, block_id, emoji)): Path<(String, String, String, String)>,
) -> ApiResult {
    let (app, on, block, emoji) = reacted(ws, &container_id, &block_id, &emoji).await?;
    app.add_reaction(&on, &block, &emoji, DENIED).await?;
    Ok(no_content())
}

/// Take a reaction off
///
/// Takes the user's reaction with the emoji off the block, for whom `addReaction`
/// takes it from. One that is not there is no error, and nothing changes. As adding
/// one, nothing of the block changes, and the members who read the container are told
/// its reactions as they are now (`reactionsChanged`).
#[utoipa::path(
    delete,
    path = "/w/{workspaceId}/containers/{containerId}/blocks/{blockId}/reactions/{emoji}",
    operation_id = "removeReaction",
    tag = tag::CONTAINERS,
    params(MemberWorkspaceId, ReactedContainerId, ReactedBlockId, ReactionEmoji),
    responses((status = 204, description = "The reaction is not there.")),
)]
async fn handle_remove_reaction(
    Delegated(InContainer(ws), ..): Delegated<Write, InContainer>,
    Path((_, container_id, block_id, emoji)): Path<(String, String, String, String)>,
) -> ApiResult {
    let (app, on, block, emoji) = reacted(ws, &container_id, &block_id, &emoji).await?;
    app.remove_reaction(&on, &block, &emoji, DENIED).await?;
    Ok(no_content())
}

/// The workspace's handle reaching no diary, the container of the path as the caller
/// reached it, the block of the path, and the emoji. Whether the caller may react there
/// to the block, [`App::add_reaction`] checks.
///
/// # Errors
///
/// Returns 400 for an emoji that is none; what [`Reached::require`] refuses one with
/// who may not comment in the container, before the block is looked for; and 404 for
/// a block that is not there.
async fn reacted(
    ws: InWorkspace,
    container_id: &str,
    block_id: &str,
    emoji: &str,
) -> Result<(App, Reached, Block, Emoji), AppError> {
    let emoji: Emoji = emoji
        .parse()
        .map_err(|error| AppError::bad_request(error).with_parameter("emoji"))?;
    let app = ws.app.reaching(ContainerKinds::SHARED);
    let on = app.reach(&ws.caller.user_id, container_id).await?;
    on.require(ContainerPermission::CommentCards, DENIED)?;
    let block = match app.block(block_id).await {
        Ok(block) => block,
        Err(error) if error.is_not_found() => return Err(AppError::not_found("block")),
        Err(error) => return Err(error),
    };
    Ok((app, on, block, emoji))
}
