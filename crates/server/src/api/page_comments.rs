//! The comments on a page of a wiki: listing, writing, changing and deleting them, and
//! what one said before.

use axum::body::Bytes;
use fvoci_model::{Block, Body, BodyChange, CommentVersion, CommentWithReactions};
use serde::Deserialize;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::comments::{
    CommenterOf, comment_versions_of, create_comment_of, delete_comment_of, list_comments_of,
    patch_comment_of, restore_comment_of,
};
use super::contract::params::{CommentId, MemberWorkspaceId, PageId, WikiId};
use super::contract::tag;
use super::items::{ItemOf, PageItem};

use super::{ApiResult, Delegated, Read, Write};
use crate::app::{App, ChangeNotification};

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new()
        .routes(routes!(
            handle_get_page_comments,
            handle_create_page_comment
        ))
        .routes(routes!(
            handle_patch_page_comment,
            handle_delete_page_comment
        ))
        .routes(routes!(handle_get_page_comment_versions))
        .routes(routes!(handle_restore_page_comment))
}

/// A comment to write on a page.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NewPageComment {
    /// The comment's text, as a page's body is written; neither empty nor `null`.
    #[serde(default)]
    #[schema(value_type = Body)]
    #[expect(
        dead_code,
        reason = "body is extracted via decode_with_document while ToSchema exposes it"
    )]
    body: BodyChange,
}

/// What a comment on a page is changed to: its text, written over the comment as it was
/// read.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PageCommentPatch {
    /// The comment's new text, as a page's body is written; neither empty nor `null`.
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

/// The comments on a page
///
/// Every comment on the page, oldest first, as blocks of the `type` `comment` whose
/// parent is the page; each says who wrote it (`createdBy`), one whose text was
/// changed when that last happened (`editedAt`), and one that has `reactions` who
/// answered it with which emoji. For who reads the wiki: its members, and of an open
/// wiki whoever works in the workspace. A page in the trash is not there (404).
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/wikis/{wikiId}/pages/{pageId}/comments",
    operation_id = "getPageComments",
    tag = tag::WIKIS,
    params(MemberWorkspaceId, WikiId, PageId),
    responses((
        status = 200,
        description = "The comments, oldest first.",
        body = Vec<CommentWithReactions>,
    )),
)]
async fn handle_get_page_comments(
    Delegated(page, ..): Delegated<Read, ItemOf<PageItem>>,
) -> ApiResult {
    list_comments_of(page).await
}

/// Write a comment on a page
///
/// For the wiki's members from the commenter's role up. The server chooses the
/// identifier, the author and the times. Every image of the text shows a file of the
/// wiki (400 otherwise), and the text has no file block (400). Who reads the wiki is
/// told with `blockChanged`, as of a comment on a card.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/wikis/{wikiId}/pages/{pageId}/comments",
    operation_id = "createPageComment",
    tag = tag::WIKIS,
    params(MemberWorkspaceId, WikiId, PageId),
    request_body = NewPageComment,
    responses((status = 201, description = "The comment as it was stored.", body = Block)),
)]
async fn handle_create_page_comment(
    Delegated(commenter, ..): Delegated<Write, CommenterOf<PageItem>>,
    body: Bytes,
) -> ApiResult {
    create_comment_of::<_, NewPageComment>(commenter, &body, ChangeNotification::Send).await
}

/// Change a comment on a page
///
/// Writes a new text over the comment, as the card's comments are changed: by its
/// author from the commenter's role up, and the comment of somebody else by the wiki's
/// admins only (403, `access denied to change other users' comments`); an editor may
/// not change it. The text it had is kept (`getPageCommentVersions`), and the comment's
/// `editedAt` says when it last changed. The text has no file block (400).
#[utoipa::path(
    patch,
    path = "/w/{workspaceId}/wikis/{wikiId}/pages/{pageId}/comments/{commentId}",
    operation_id = "patchPageComment",
    tag = tag::WIKIS,
    params(MemberWorkspaceId, WikiId, PageId, CommentId),
    request_body = PageCommentPatch,
    responses((status = 200, description = "The comment as it is now.", body = Block)),
)]
async fn handle_patch_page_comment(
    Delegated(commenter, ..): Delegated<Write, CommenterOf<PageItem>>,
    body: Bytes,
) -> ApiResult {
    patch_comment_of::<_, PageCommentPatch>(commenter, &body, ChangeNotification::Send, |patch| {
        patch.read_at
    })
    .await
}

/// Delete a comment on a page
///
/// The comment goes to the trash, as a comment on a card does: by its author from the
/// commenter's role up, and the comment of somebody else by the wiki's admins only
/// (403, `access denied to delete other users' comments`). Who reads the wiki is told
/// with `blockChanged` with `deletedAt` set.
#[utoipa::path(
    delete,
    path = "/w/{workspaceId}/wikis/{wikiId}/pages/{pageId}/comments/{commentId}",
    operation_id = "deletePageComment",
    tag = tag::WIKIS,
    params(MemberWorkspaceId, WikiId, PageId, CommentId),
    responses((status = 204, description = "The comment is deleted.")),
)]
async fn handle_delete_page_comment(
    Delegated(commenter, ..): Delegated<Write, CommenterOf<PageItem>>,
) -> ApiResult {
    delete_comment_of(commenter, ChangeNotification::Send).await
}

/// Bring a deleted comment on a page back
///
/// A comment deleted on its own comes back onto its page, as `restoreCardComment` brings a
/// card's comment back: for its author from the commenter's role up if they deleted it
/// themselves, and for the wiki's admins whoever wrote or deleted it.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/wikis/{wikiId}/pages/{pageId}/comments/{commentId}/restore",
    operation_id = "restorePageComment",
    tag = tag::WIKIS,
    params(MemberWorkspaceId, WikiId, PageId, CommentId),
    responses((status = 200, description = "The comment as it is again.", body = Block)),
)]
async fn handle_restore_page_comment(commenter: CommenterOf<PageItem>) -> ApiResult {
    restore_comment_of(commenter).await
}

/// What a comment on a page said before
///
/// The earlier texts of the comment, as `getCardCommentVersions` gives a card's comment's:
/// every version but the one it has now, the first one first, with who wrote it and
/// when.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/wikis/{wikiId}/pages/{pageId}/comments/{commentId}/versions",
    operation_id = "getPageCommentVersions",
    tag = tag::WIKIS,
    params(MemberWorkspaceId, WikiId, PageId, CommentId),
    responses((status = 200, description = "The earlier versions, the first one first.", body = Vec<CommentVersion>)),
)]
async fn handle_get_page_comment_versions(
    Delegated(page, ..): Delegated<Read, ItemOf<PageItem>>,
) -> ApiResult {
    comment_versions_of(page).await
}
