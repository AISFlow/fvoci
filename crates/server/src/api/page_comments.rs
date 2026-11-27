//! The comments on a page of a wiki: listing, writing, changing and deleting them, and
//! what one said before.

use axum::body::Bytes;
use axum::extract::Path;
use fvoci_model::{Block, Body, BodyChange, CommentVersion, ContainerKinds};
use serde::Deserialize;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::comments::{
    CommentParentKind, CommenterOf, comments_on, create_comment_on, created_comment,
    delete_comment_on, existing_comment_on, find_live_comment_on, get_comment_versions_on,
    patch_comment_on, restore_comment_on,
};
use super::contract::params::{CommentId, MemberWorkspaceId, PageId, WikiId};
use super::contract::tag;
use super::wikis::{READ_DENIED, page_with_permission};
use super::workspace::{InWiki, InWorkspace};
use super::{ApiResult, json_response};
use super::{Delegated, Read, Write};
use crate::app::{App, ChangeNotification};
use crate::error::AppError;
use crate::permissions::ContainerPermission;

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

const CREATE_DENIED: &str = "access denied to comment on the page";
const CHANGES_DENIED: &str = "access denied to change the comments of the page";

pub(super) struct PageCommentParent;

impl CommentParentKind for PageCommentParent {
    const CONTAINER_PARAM: &'static str = "wikiId";
    const PARENT_PARAM: &'static str = "pageId";
    const CREATE_DENIED: &'static str = CREATE_DENIED;
    const CHANGES_DENIED: &'static str = CHANGES_DENIED;

    fn kinds() -> ContainerKinds {
        ContainerKinds::WIKIS
    }

    async fn load_parent(
        ws: &InWorkspace,
        wiki_id: &str,
        page_id: &str,
    ) -> Result<Block, AppError> {
        ws.app.page_of(wiki_id, page_id).await
    }
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
/// parent is the page; each says who wrote it (`createdBy`), and one whose text was
/// changed when that last happened (`editedAt`). For who reads the wiki: its members,
/// and of an open wiki whoever works in the workspace. A page in the trash is not
/// there (404).
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/wikis/{wikiId}/pages/{pageId}/comments",
    operation_id = "getPageComments",
    tag = tag::WIKIS,
    params(MemberWorkspaceId, WikiId, PageId),
    responses((status = 200, description = "The comments, oldest first.", body = Vec<Block>)),
)]
async fn handle_get_page_comments(
    Delegated(InWiki(ws), ..): Delegated<Read, InWiki>,
    Path((_, wiki_id, page_id)): Path<(String, String, String)>,
) -> ApiResult {
    Ok(json_response(
        &page_comments(&ws, &wiki_id, &page_id).await?,
    ))
}

/// The comments on the page `page_id` of the wiki `wiki_id`, as `getPageComments`
/// lists them.
///
/// # Errors
///
/// As `getPageComments` answers.
pub(super) async fn page_comments(
    ws: &InWorkspace,
    wiki_id: &str,
    page_id: &str,
) -> Result<Vec<Block>, AppError> {
    let page =
        page_with_permission(ws, wiki_id, page_id, ContainerPermission::View, READ_DENIED).await?;
    comments_on(ws, wiki_id, &page.id).await
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
    Delegated(commenter, ..): Delegated<Write, CommenterOf<PageCommentParent>>,
    body: Bytes,
) -> ApiResult {
    let comment = create_comment_on::<NewPageComment>(
        &commenter.ws,
        &commenter.container_id,
        commenter.parent,
        "page",
        &body,
        ChangeNotification::Send,
    )
    .await?;
    Ok(created_comment(&comment))
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
    Delegated(commenter, ..): Delegated<Write, CommenterOf<PageCommentParent>>,
    body: Bytes,
) -> ApiResult {
    let comment_id = commenter.comment_id()?;
    let comment = existing_comment_on(&commenter.ws, &commenter.parent, comment_id).await?;
    let patched = patch_comment_on::<PageCommentPatch>(
        &commenter.ws,
        &commenter.container_id,
        &comment,
        CHANGES_DENIED,
        &body,
        ChangeNotification::Send,
        |patch| patch.read_at,
    )
    .await?;
    Ok(json_response(&patched))
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
    Delegated(commenter, ..): Delegated<Write, CommenterOf<PageCommentParent>>,
) -> ApiResult {
    let comment_id = commenter.comment_id()?;
    let comment = existing_comment_on(&commenter.ws, &commenter.parent, comment_id).await?;
    delete_comment_on(
        &commenter.ws,
        &comment,
        CHANGES_DENIED,
        ChangeNotification::Send,
    )
    .await
}

/// Bring a deleted comment on a page back
///
/// A comment deleted on its own comes back onto its page, as `restoreBlock` brings a
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
async fn handle_restore_page_comment(commenter: CommenterOf<PageCommentParent>) -> ApiResult {
    let comment_id = commenter.comment_id()?;
    let comment =
        restore_comment_on(&commenter.ws, &commenter.parent, comment_id, CHANGES_DENIED).await?;
    Ok(json_response(&comment))
}

/// What a comment on a page said before
///
/// The earlier texts of the comment, as `getCommentVersions` gives a card's comment's:
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
    Delegated(InWiki(ws), ..): Delegated<Read, InWiki>,
    Path((_, wiki_id, page_id, comment_id)): Path<(String, String, String, String)>,
) -> ApiResult {
    let page = page_with_permission(
        &ws,
        &wiki_id,
        &page_id,
        ContainerPermission::View,
        READ_DENIED,
    )
    .await?;
    let live = find_live_comment_on(&ws, &page, &comment_id).await?;
    get_comment_versions_on(&ws, &page, live, &comment_id).await
}
