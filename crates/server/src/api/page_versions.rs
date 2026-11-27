//! The versions kept of a page's body: listing them, reading one, keeping one now, and
//! going back to one.
//!
//! A page's body is kept as a card's is (`crates/server/src/collab`), and what is given
//! and done is the card's (`App::body_versions`, `App::body_version`,
//! `App::keep_body_version_now`, `App::restore_body_version`); only the routes are a wiki's, under the handle that
//! reaches wikis (`InWiki`), so that no operation of a card's versions reaches a page.

use axum::extract::Path;
use fvoci_model::{BodyVersion, BodyVersionContent, KeptBodyVersion, Page};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::body_versions::{
    get_body_version, keep_body_version, list_body_versions, restore_body_version,
};
use super::contract::params::{MemberWorkspaceId, PageId, PageVersionNumber, WikiId};
use super::contract::tag;
use super::wikis::{READ_DENIED, page_with_permission};
use super::workspace::InWiki;
use super::{ApiResult, json_response};
use super::{Delegated, Read};
use crate::app::App;
use crate::permissions::ContainerPermission;

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new()
        .routes(routes!(
            handle_get_page_body_versions,
            handle_keep_page_body_version
        ))
        .routes(routes!(handle_get_page_body_version))
        .routes(routes!(handle_restore_page_body_version))
}

/// The versions kept of a page's body
///
/// As `getCardBodyVersions` gives a card's: the newest first, each with when it was
/// kept and who wrote the body up to it, without the bodies (`getPageBodyVersion`).
/// For who reads the wiki, whatever their role: its members, and of an open wiki
/// whoever works in the workspace. A page in the trash is not there (404); its versions are kept while it is there, and go when it is gone for good.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/wikis/{wikiId}/pages/{pageId}/body-versions",
    operation_id = "getPageBodyVersions",
    tag = tag::WIKIS,
    params(MemberWorkspaceId, WikiId, PageId),
    responses((status = 200, description = "The versions, the newest first.", body = Vec<BodyVersion>)),
)]
async fn handle_get_page_body_versions(
    Delegated(InWiki(ws), ..): Delegated<Read, InWiki>,
    Path((_, wiki_id, page_id)): Path<(String, String, String)>,
) -> ApiResult {
    let page = page_with_permission(
        &ws,
        &wiki_id,
        &page_id,
        ContainerPermission::View,
        READ_DENIED,
    )
    .await?;
    list_body_versions(&ws.app, &page.id).await
}

/// Keep a page's body as a version now
///
/// As `keepCardBodyVersion` keeps a card's: the body the page has, with what those who
/// edit it together typed, by hand; nothing of a body that is its newest version
/// already (`unchanged`), nor within a minute of a version kept by hand (`recent`). For
/// editors of the wiki (403 for its other members). A page in the trash is not there
/// (404).
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/wikis/{wikiId}/pages/{pageId}/body-versions",
    operation_id = "keepPageBodyVersion",
    tag = tag::WIKIS,
    params(MemberWorkspaceId, WikiId, PageId),
    responses((status = 200, description = "What became of the body.", body = KeptBodyVersion)),
)]
async fn handle_keep_page_body_version(
    InWiki(ws): InWiki,
    Path((_, wiki_id, page_id)): Path<(String, String, String)>,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    let page = page_with_permission(
        &ws,
        &wiki_id,
        &page_id,
        ContainerPermission::ManageCards,
        "access denied to keep a version of the page's body",
    )
    .await?;
    let kept = keep_body_version(&ws.app, &page.id).await?;
    tracing::debug!(wiki_id, page_id, user_id, outcome = ?kept.outcome, "KeepPageBodyVersion");
    Ok(json_response(&kept))
}

/// A version kept of a page's body
///
/// The version with its body, as `getPage` gives a body. For whom
/// `getPageBodyVersions` is for. A number the page has no version of, or has none of
/// any more, is 404.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/wikis/{wikiId}/pages/{pageId}/body-versions/{version}",
    operation_id = "getPageBodyVersion",
    tag = tag::WIKIS,
    params(MemberWorkspaceId, WikiId, PageId, PageVersionNumber),
    responses((status = 200, description = "The version.", body = BodyVersionContent)),
)]
async fn handle_get_page_body_version(
    Delegated(InWiki(ws), ..): Delegated<Read, InWiki>,
    Path((_, wiki_id, page_id, version)): Path<(String, String, String, String)>,
) -> ApiResult {
    let page = page_with_permission(
        &ws,
        &wiki_id,
        &page_id,
        ContainerPermission::View,
        READ_DENIED,
    )
    .await?;
    get_body_version(&ws.app, &page.id, &version).await
}

/// Go back to a version of a page's body
///
/// Writes the body of the version to the page again, as `restoreCardBodyVersion` does
/// to a card: those who edit the page together are sent the body as a new document, the
/// body it replaces is kept as a version first, and the body written is the page's
/// newest version, by the caller. A page that has that body already is left as it is.
/// For editors of the wiki (403 for its other members). A number the page has no
/// version of, or has none of any more, is 404; a version whose body may not be stored
/// any more, or names a file (an image, a file block) the wiki no longer has, is 409.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/wikis/{wikiId}/pages/{pageId}/body-versions/{version}/restore",
    operation_id = "restorePageBodyVersion",
    tag = tag::WIKIS,
    params(MemberWorkspaceId, WikiId, PageId, PageVersionNumber),
    responses((status = 200, description = "The page as it is now.", body = Page)),
)]
async fn handle_restore_page_body_version(
    InWiki(ws): InWiki,
    Path((_, wiki_id, page_id, version)): Path<(String, String, String, String)>,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    let page = page_with_permission(
        &ws,
        &wiki_id,
        &page_id,
        ContainerPermission::ManageCards,
        "access denied to restore a version of the page's body",
    )
    .await?;
    let block = restore_body_version(&ws.app, &page, &version, user_id).await?;
    tracing::debug!(wiki_id, page_id, user_id, "RestorePageBodyVersion");
    Ok(json_response(&Page::from(block)))
}
