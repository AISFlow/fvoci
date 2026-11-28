//! Wikis: listing and making them, and their pages.
//!
//! A wiki is a board of the kind `wiki`. What is the same for a board and a wiki is
//! asked under `/containers/{containerId}` (`InContainer`); here is what only a wiki has, and the
//! handle these routes work with reaches wikis and no board (`InWiki`).

use axum::body::Bytes;
use axum::extract::Path;
use fvoci_model::{
    Block, Container, ContainerKind, ContainerVisibility, DeletedPage, MinimumRole, NewPage, Page,
    PageDuplicate, PageMove, PageNode, PagePatch, PageWithReactions,
};
use serde::Deserialize;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::cards::{BodyFormatValue, BodyQuery};
use super::contract::params::Page as PageNumber;
use super::contract::params::{BodyFormat, MemberWorkspaceId, PageId, PerPage, WikiId};
use super::contract::tag;
use super::query::{PagingQuery, QueryParams};
use super::workspace::{InWiki, InWorkspace};
use super::{ApiResult, created, decode_object, decode_with_document, json_response, no_content};
use super::{Delegated, Read, Write};
use crate::app::{
    App, CREATE_PAGE_DENIED, DELETE_PAGE_DENIED, DUPLICATE_PAGE_DENIED, MOVE_PAGE_DENIED,
    PAGE_TRASH_DENIED, PATCH_PAGE_DENIED, Reached,
};
use crate::error::AppError;
use crate::permissions::ContainerPermission;

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new()
        .routes(routes!(handle_get_wikis, handle_create_wiki))
        .routes(routes!(handle_get_page_tree, handle_create_page))
        .routes(routes!(
            handle_get_page,
            handle_patch_page,
            handle_delete_page
        ))
        .routes(routes!(handle_move_page))
        .routes(routes!(handle_duplicate_page))
        .routes(routes!(handle_get_deleted_pages))
        .routes(routes!(handle_restore_page))
}

/// A wiki to make.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NewWiki {
    /// Whether the wiki is `open`, for whoever works in the workspace to read, and to
    /// join to write in it, or `private`, for whom it is shared with.
    visibility: ContainerVisibility,
    /// The role a user gets who joins the wiki. Left out, `editor`.
    #[serde(default)]
    minimum_role: MinimumRole,
    /// The wiki's title. Left out, empty.
    #[serde(default)]
    title: String,
    /// The wiki's description. Left out, empty.
    #[serde(default)]
    description: String,
    /// The wiki's icon, usually one emoji. Left out, none.
    #[serde(default)]
    icon: String,
}

/// The wikis of a workspace
///
/// The wikis of the workspace that the user is a member of, and the workspace's open
/// wikis; a guest of the workspace gets only the former, an administrator every wiki
/// of the workspace (the pages of a private wiki they are no member of stay 404 to
/// them). Each is a board of the `kind` `wiki`. Oldest first. 404 unless the user is a
/// member of the workspace.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/wikis",
    operation_id = "getWikis",
    tag = tag::WIKIS,
    params(MemberWorkspaceId),
    responses((status = 200, description = "The wikis.", body = Vec<Container>)),
)]
async fn handle_get_wikis(Delegated(InWiki(ws), ..): Delegated<Read, InWiki>) -> ApiResult {
    let wikis = ws
        .app
        .workspace_containers(&ws.caller.user_id, &ws.workspace_id, ws.role)
        .await?;
    Ok(json_response(&wikis))
}

/// Make a wiki
///
/// The user becomes the wiki's admin, and the wiki is filed under the user's default
/// category, as a board is. Whoever works in the workspace makes wikis in it; a guest
/// is refused (403). A member the request does not know is 400.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/wikis",
    operation_id = "createWiki",
    tag = tag::WIKIS,
    params(MemberWorkspaceId),
    request_body = NewWiki,
    responses((status = 201, description = "The wiki that was made.", body = Container)),
)]
async fn handle_create_wiki(InWiki(ws): InWiki, body: Bytes) -> ApiResult {
    ws.require_container_maker("access denied to create wiki")?;
    let NewWiki {
        visibility,
        minimum_role,
        title,
        description,
        icon,
    } = decode_object(&body)?;
    let wiki = Container {
        workspace_id: ws.workspace_id.clone(),
        visibility,
        kind: ContainerKind::Wiki,
        minimum_role,
        title,
        description,
        icon,
        ..Container::default()
    };
    let wiki = ws
        .app
        .create_container_with_admin(wiki, &ws.caller.user_id)
        .await?;
    Ok(created(&wiki))
}

/// What a member of the workspace who may not read the wiki is refused a page with.
pub(super) const READ_DENIED: &str = "access denied to fetch page";

/// Returns the page of the wiki for a handler of `/wikis/{wikiId}/pages/{pageId}`, if
/// the user has the permission on the wiki: its block, read once.
///
/// Who may not read the wiki is told what [`Reached::require`] tells; a block that is no
/// page of this wiki is no page, whatever else it is.
pub(super) async fn page_with_permission(
    ws: &InWorkspace,
    wiki_id: &str,
    page_id: &str,
    permission: ContainerPermission,
    denial: &str,
) -> Result<Block, AppError> {
    let (_, page) = page_reached(ws, wiki_id, page_id, permission, denial).await?;
    Ok(page)
}

/// The wiki as the caller reached it and its page, as [`page_with_permission`] finds
/// them: for a route that writes the page, which the wiki's [`Reached`] is asked for.
pub(super) async fn page_reached(
    ws: &InWorkspace,
    wiki_id: &str,
    page_id: &str,
    permission: ContainerPermission,
    denial: &str,
) -> Result<(Reached, Block), AppError> {
    let on = ws.app.reach(&ws.caller.user_id, wiki_id).await?;
    on.require(permission, denial)?;
    let page = ws.app.page_of(wiki_id, page_id).await?;
    Ok((on, page))
}

/// The pages of a wiki
///
/// Every page of the wiki without its body, for the wiki's members: each names the page
/// it is below (`parentId`, empty at the top of the wiki), which is how they are a
/// tree. By `order` as strings compare by code unit, then by `id`: the pages of one
/// parent are in their order when they are taken out of the list as it is.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/wikis/{wikiId}/pages",
    operation_id = "getPageTree",
    tag = tag::WIKIS,
    params(MemberWorkspaceId, WikiId),
    responses((status = 200, description = "The pages.", body = Vec<PageNode>)),
)]
async fn handle_get_page_tree(
    Delegated(InWiki(ws), ..): Delegated<Read, InWiki>,
    Path((_, wiki_id)): Path<(String, String)>,
) -> ApiResult {
    Ok(json_response(&page_tree_read(&ws, &wiki_id).await?))
}

/// The pages of the wiki `wiki_id` without their bodies, as `getPageTree` lists them.
///
/// # Errors
///
/// As `getPageTree` answers.
pub(super) async fn page_tree_read(
    ws: &InWorkspace,
    wiki_id: &str,
) -> Result<Vec<PageNode>, AppError> {
    let on = ws.app.reach(&ws.caller.user_id, wiki_id).await?;
    on.require(ContainerPermission::View, "access denied to fetch pages")?;
    ws.app.page_tree(wiki_id).await
}

/// Make a page
///
/// The server chooses the identifier, the creator, the times and the `order`. For
/// editors of the wiki. The page goes below `parentId`, a page of the same wiki (400 at
/// `/parentId` otherwise), or at the top of the wiki; among the pages of that parent it
/// goes right after the page `after` (400 at `/after` if that is no page of the same
/// parent), first if `after` is empty, and last if it is left out. Its body is written
/// by `patchPage`, or together in its room (`joinDoc`).
///
/// The tree is held to the organization's limits: a page deeper than its depth is 400
/// at `/parentId` with the reason `page_too_deep`, and one more page than a wiki may
/// have (those in the trash not counted) is 400 with the reason `too_many_pages`, each
/// with the `limit`.
///
/// The wiki's members are told with `blockChanged`, whose block of the `type` `page`
/// has the icon and the order among its `fields`. Where the pages of the parent had no
/// room left between them (two of one order, or orders grown long), they are numbered
/// anew, and each that changed is told of too.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/wikis/{wikiId}/pages",
    operation_id = "createPage",
    tag = tag::WIKIS,
    params(MemberWorkspaceId, WikiId),
    request_body = NewPage,
    responses((status = 201, description = "The page that was made.", body = Page)),
)]
async fn handle_create_page(
    Delegated(InWiki(ws), ..): Delegated<Write, InWiki>,
    Path((_, wiki_id)): Path<(String, String)>,
    body: Bytes,
) -> ApiResult {
    Ok(created(&create_page_from(&ws, &wiki_id, &body).await?))
}

/// Makes the page `new` describes in the wiki `wiki_id`, as `createPage` does with the
/// body it read: for what writes as the route does.
///
/// # Errors
///
/// As `createPage` answers.
pub(super) async fn create_page_with(
    ws: &InWorkspace,
    wiki_id: &str,
    new: &NewPage,
) -> Result<Page, AppError> {
    let on = ws.app.reach(&ws.caller.user_id, wiki_id).await?;
    let page = ws.app.create_page(&on, new).await?;
    tracing::debug!(wiki_id, page_id = %page.id, user_id = on.user_id(), "CreatePage");
    Ok(page)
}

/// Changes the page `page_id` of the wiki `wiki_id` as `patch` says, as `patchPage` does
/// with the body it read: for what writes as the route does.
///
/// # Errors
///
/// As `patchPage` answers.
pub(super) async fn patch_page_with(
    ws: &InWorkspace,
    wiki_id: &str,
    page_id: &str,
    patch: PagePatch,
) -> Result<Page, AppError> {
    let editing = ContainerPermission::ManageCards;
    let (on, page) = page_reached(ws, wiki_id, page_id, editing, PATCH_PAGE_DENIED).await?;
    let patched = ws
        .app
        .patch_page(&on, &page, patch, ws.caller.agent())
        .await?;
    tracing::debug!(wiki_id, page_id, user_id = on.user_id(), "PatchPage");
    Ok(patched)
}

/// Makes the page `body` describes, as `createPage` takes it, in the wiki `wiki_id`.
///
/// # Errors
///
/// As `createPage` answers.
pub(super) async fn create_page_from(
    ws: &InWorkspace,
    wiki_id: &str,
    body: &Bytes,
) -> Result<Page, AppError> {
    let on = ws.app.reach(&ws.caller.user_id, wiki_id).await?;
    // Asked before the body is read, as every route asks.
    on.require(ContainerPermission::ManageCards, CREATE_PAGE_DENIED)?;
    let new: NewPage = decode_object(body)?;
    let page = ws.app.create_page(&on, &new).await?;
    tracing::debug!(wiki_id, page_id = %page.id, user_id = on.user_id(), "CreatePage");
    Ok(page)
}

/// A page
///
/// The page with its body, and its `reactions` where it has any, for the members of its
/// wiki.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/wikis/{wikiId}/pages/{pageId}",
    operation_id = "getPage",
    tag = tag::WIKIS,
    params(MemberWorkspaceId, WikiId, PageId, BodyFormat),
    responses((status = 200, description = "The page.", body = PageWithReactions)),
)]
async fn handle_get_page(
    Delegated(InWiki(ws), ..): Delegated<Read, InWiki>,
    Path((_, wiki_id, page_id)): Path<(String, String, String)>,
    QueryParams(query): QueryParams<BodyQuery>,
) -> ApiResult {
    let block = page_with_permission(
        &ws,
        &wiki_id,
        &page_id,
        ContainerPermission::View,
        READ_DENIED,
    )
    .await?;
    let reactions = ws.app.reactions_to(&block.id).await?;
    let mut page = Page::from(block);
    if query.format == BodyFormatValue::Markdown {
        ws.app
            .put_page_in_markdown(ws.markdown_reader(), &mut page)
            .await?;
    }
    Ok(json_response(&PageWithReactions { page, reactions }))
}

/// Change a page
///
/// Its title, its icon, or its whole body, for editors of the wiki. A new body is sent
/// with `readAt`, the `updatedAt` of the page as it was read: 409 if the page has
/// changed since, and nothing is changed. Where the body is being edited together, what
/// the editors typed is written to the page first, so a writer who read it before is
/// refused and reads it again. Where the page is in the tree does not change here.
#[utoipa::path(
    patch,
    path = "/w/{workspaceId}/wikis/{wikiId}/pages/{pageId}",
    operation_id = "patchPage",
    tag = tag::WIKIS,
    params(MemberWorkspaceId, WikiId, PageId),
    request_body = PagePatch,
    responses((status = 200, description = "The page as it is now.", body = Page)),
)]
async fn handle_patch_page(
    Delegated(InWiki(ws), ..): Delegated<Write, InWiki>,
    Path((_, wiki_id, page_id)): Path<(String, String, String)>,
    body: Bytes,
) -> ApiResult {
    Ok(json_response(
        &patch_page_from(&ws, &wiki_id, &page_id, &body).await?,
    ))
}

/// Changes the page `page_id` of the wiki `wiki_id` as `body` says, as `patchPage`
/// takes it.
///
/// # Errors
///
/// As `patchPage` answers.
pub(super) async fn patch_page_from(
    ws: &InWorkspace,
    wiki_id: &str,
    page_id: &str,
    body: &Bytes,
) -> Result<Page, AppError> {
    let editing = ContainerPermission::ManageCards;
    let (on, page) = page_reached(ws, wiki_id, page_id, editing, PATCH_PAGE_DENIED).await?;
    let (mut patch, document): (PagePatch, _) = decode_with_document(body)?;
    patch.body = document;
    let patched = ws
        .app
        .patch_page(&on, &page, patch, ws.caller.agent())
        .await?;
    tracing::debug!(wiki_id, page_id, user_id = on.user_id(), "PatchPage");
    Ok(patched)
}

/// Move a page
///
/// The page goes, with every page below it, below `parentId` (a page of the same wiki;
/// left out or empty, the top of the wiki), right after the page `after` among the
/// pages there, first if `after` is empty, and last if it is left out, as for
/// `createPage`. For editors of the wiki. A page moves within its wiki only. 400 at
/// `/parentId` if the parent is no page of the wiki or is the page itself or a page
/// below it, and with the reason `page_too_deep` (and the `limit`) if a page would be
/// deeper than the organization's limit; 400 at `/after` if that is no other page of
/// the parent. Nothing is changed then.
///
/// The wiki's members are told with `blockChanged` of the page, and of every page of
/// the parent numbered anew to leave it room; the pages below it keep their parents
/// and are not told of.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/wikis/{wikiId}/pages/{pageId}/move",
    operation_id = "movePage",
    tag = tag::WIKIS,
    params(MemberWorkspaceId, WikiId, PageId),
    request_body = PageMove,
    responses((status = 200, description = "The page where it is now.", body = Page)),
)]
async fn handle_move_page(
    InWiki(ws): InWiki,
    Path((_, wiki_id, page_id)): Path<(String, String, String)>,
    body: Bytes,
) -> ApiResult {
    let editing = ContainerPermission::ManageCards;
    let (on, page) = page_reached(&ws, &wiki_id, &page_id, editing, MOVE_PAGE_DENIED).await?;
    let to: PageMove = decode_object(&body)?;
    let moved = ws.app.move_page(&on, &page, &to).await?;
    tracing::debug!(wiki_id, page_id, user_id = on.user_id(), "MovePage");
    Ok(json_response(&moved))
}

/// Duplicate a page
///
/// Copies the page with every page below it, within its wiki, for editors of the wiki:
/// the copy goes right after the page among its siblings, titled `title` if given (the
/// page's own title otherwise; a client names it in its reader's language), and the
/// pages below it are copied below it as they are, with their titles, icons, places and
/// bodies, what is typed in them now included. The images and file blocks in the bodies
/// get files of their own, as a card's duplicate's do. The versions of the bodies and
/// the comments are not copied. The tree is held to the organization's limits: 400 with
/// the reason `page_too_deep` or `too_many_pages` (with the `limit`) if the copies
/// would be deeper, or the wiki larger, than they let it be; 400 at `/title` for a
/// title longer than 16383 characters. Nothing is changed then.
///
/// The wiki's members are told with `blockChanged` of each copy, a page before those
/// below it, and of each page of the parent numbered anew to leave the copy room.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/wikis/{wikiId}/pages/{pageId}/duplicate",
    operation_id = "duplicatePage",
    tag = tag::WIKIS,
    params(MemberWorkspaceId, WikiId, PageId),
    request_body = PageDuplicate,
    responses((status = 201, description = "The copy of the page.", body = Page)),
)]
async fn handle_duplicate_page(
    InWiki(ws): InWiki,
    Path((_, wiki_id, page_id)): Path<(String, String, String)>,
    body: Bytes,
) -> ApiResult {
    let editing = ContainerPermission::ManageCards;
    let denial = DUPLICATE_PAGE_DENIED;
    let (on, page) = page_reached(&ws, &wiki_id, &page_id, editing, denial).await?;
    let copy: PageDuplicate = decode_object(&body)?;
    let made = ws.app.duplicate_page(&on, &page, &copy).await?;
    tracing::debug!(wiki_id, page_id, user_id = on.user_id(), "DuplicatePage");
    Ok(created(&made))
}

/// Delete a page
///
/// The page goes to the trash with every page below it and the comments on them, for
/// editors of the wiki. The wiki's members are told with `blockChanged` of the page
/// alone, with `deletedAt` set, as for any block that is deleted; what went with
/// it is not told of one by one.
#[utoipa::path(
    delete,
    path = "/w/{workspaceId}/wikis/{wikiId}/pages/{pageId}",
    operation_id = "deletePage",
    tag = tag::WIKIS,
    params(MemberWorkspaceId, WikiId, PageId),
    responses((status = 204, description = "The page is deleted.")),
)]
async fn handle_delete_page(
    InWiki(ws): InWiki,
    Path((_, wiki_id, page_id)): Path<(String, String, String)>,
) -> ApiResult {
    let editing = ContainerPermission::ManageCards;
    let (on, page) = page_reached(&ws, &wiki_id, &page_id, editing, DELETE_PAGE_DENIED).await?;
    ws.app.delete_page(&on, &page).await?;
    tracing::debug!(wiki_id, page_id, user_id = on.user_id(), "DeletePage");
    Ok(no_content())
}

/// The deleted pages of a wiki
///
/// One page of the wiki's pages that are in the trash, each with its identifier, its
/// title and icon, the page it was below, when it was deleted, by whom, and how many
/// pages below it went with it; `restorePage` brings one back with those until the
/// trash is emptied of it (`trash_retention_days`, as for a board's cards). A page that
/// went with the page it was below comes back with that page, and is not listed. For
/// the wiki's editors. The last deleted first; the pages cut that order.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/wikis/{wikiId}/trash/pages",
    operation_id = "getDeletedPages",
    tag = tag::WIKIS,
    params(MemberWorkspaceId, WikiId, PageNumber, PerPage),
    responses((status = 200, description = "The deleted pages of the wiki.", body = Vec<DeletedPage>)),
)]
async fn handle_get_deleted_pages(
    InWiki(ws): InWiki,
    Path((_, wiki_id)): Path<(String, String)>,
    QueryParams(query): QueryParams<PagingQuery>,
) -> ApiResult {
    let on = ws.app.reach(&ws.caller.user_id, &wiki_id).await?;
    // Asked before the query is read, as every route asks.
    on.require(ContainerPermission::ManageCards, PAGE_TRASH_DENIED)?;
    let page = query.into_list_page()?;
    Ok(json_response(&ws.app.deleted_pages(&on, page).await?))
}

/// Bring back a page
///
/// The page comes back from the wiki's trash with every page that was deleted along
/// with it, for editors of the wiki: below the page it was below, at the place it had
/// (right after the pages before it there, if a page made since has that place), or,
/// that page gone (in the trash, or gone for good), last at the top of the wiki.
/// The tree is held to the organization's limits: 400 with the reason `page_too_deep`
/// or `too_many_pages` (with the `limit`) if the pages would be deeper, or the wiki
/// larger, than they let it be. A page that is not in this wiki's trash, or went there
/// with the page it was below (it comes back with that page), is 404. The comments on
/// the pages come back with them; the limits count pages. The wiki's members are told
/// with `blockChanged` of each page and comment that came back, a page before what is
/// below it, and of each page numbered anew to leave it room.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/wikis/{wikiId}/pages/{pageId}/restore",
    operation_id = "restorePage",
    tag = tag::WIKIS,
    params(MemberWorkspaceId, WikiId, PageId),
    responses((status = 200, description = "The page as it is again.", body = Page)),
)]
async fn handle_restore_page(
    InWiki(ws): InWiki,
    Path((_, wiki_id, page_id)): Path<(String, String, String)>,
) -> ApiResult {
    let on = ws.app.reach(&ws.caller.user_id, &wiki_id).await?;
    let page = ws.app.undelete_page(&on, &page_id).await?;
    tracing::debug!(wiki_id, page_id, user_id = on.user_id(), "UndeletePage");
    Ok(json_response(&page))
}
