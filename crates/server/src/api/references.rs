//! References: what the bodies of a workspace name.

use axum::body::Bytes;
use axum::extract::Path;
use fvoci_model::{LinkedBlock, is_id};
use serde::Deserialize;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::contract::bodies::ReferenceIds;
use super::contract::params::{BlockId, MemberWorkspaceId, Page, PerPage, TitleTerm};
use super::contract::tag;
use super::query::{PagingQuery, QueryParams};
use super::workspace::InLinked;
use super::{ApiResult, decode_json, json_response};
use super::{Delegated, Read};
use crate::app::App;
use crate::error::AppError;

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
struct ReferenceSearchQuery {
    q: String,
}

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new()
        .routes(routes!(handle_resolve_references))
        .routes(routes!(handle_get_references_to))
        .routes(routes!(handle_search_references))
}

/// What references name
///
/// A body names a card, a page or a unit by its identifier alone (the node `reference`),
/// and no body, however it is read, says what that is called: this does, for the user who
/// asks and as things are now. Of the identifiers of the body, the cards, pages and units
/// of this workspace on a board, a wiki or a course the user reads (as `getCard`,
/// `getPage` and `getUnit` ask), each once, in the order they were first named.
///
/// Whatever else an identifier names is left out, and the answer does not tell why:
/// nothing at all, something in the trash, something of another workspace or
/// organization, a card, a page or a unit the user does not read, or a block that is none
/// of those (such as a chapter of a course).
/// The body names at most 100 identifiers (more is 400, as an entry that has not the
/// form of an identifier is); an empty list answers an empty list.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/references/resolve",
    operation_id = "resolveReferences",
    tag = tag::REFERENCES,
    params(MemberWorkspaceId),
    request_body = ReferenceIds,
    responses((status = 200, description = "What the user is shown of them.", body = Vec<LinkedBlock>)),
)]
async fn handle_resolve_references(
    Delegated(InLinked(ws), ..): Delegated<Read, InLinked>,
    body: Bytes,
) -> ApiResult {
    let ids: Vec<String> = decode_json(&body)?;
    let linked = ws.app.resolve_references(&ws.caller.user_id, &ids).await?;
    Ok(json_response(&linked))
}

/// What names a card, a page or a unit
///
/// The cards, pages and units of this workspace whose body names the block of the path
/// with a reference, or which have a comment whose body does, on a board, a wiki or a
/// course the user reads; the block itself is never among them. Oldest first. What names
/// it from where the user does not read is left out and takes no place in a page: the
/// answer does not say how many there are, nor whether there are any.
///
/// 404 unless the block is a card, a page or a unit the user reads, as
/// `resolveReferences` tells of it: for anything else, as for an identifier of nothing.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/references/to/{blockId}",
    operation_id = "getReferencesTo",
    tag = tag::REFERENCES,
    params(MemberWorkspaceId, BlockId, Page, PerPage),
    responses((status = 200, description = "What names it, of the page.", body = Vec<LinkedBlock>)),
)]
async fn handle_get_references_to(
    Delegated(InLinked(ws), ..): Delegated<Read, InLinked>,
    Path((_, block_id)): Path<(String, String)>,
    QueryParams(query): QueryParams<PagingQuery>,
) -> ApiResult {
    let asked = query.into_list_page()?;
    // What has not the form of an identifier names nothing, and is answered so.
    let naming = if is_id(&block_id) {
        ws.app
            .references_to(&ws.caller.user_id, &ws.workspace_id, &block_id, asked)
            .await?
    } else {
        None
    };
    let naming = naming.ok_or_else(|| AppError::not_found("block"))?;
    Ok(json_response(&naming))
}

/// Find what a reference may name
///
/// The cards, pages and units of this workspace, on a board, a wiki or a course the user
/// reads, whose title has every word of `q` in it, as spaces divide it and whatever the
/// case of the letters (of every letter, not only ASCII); `%` and `_` are characters like
/// any other. At most 20, the one changed last first; without `q`, the 20 changed last.
/// Bodies are not looked into. What is in the trash, what is on a template, a chapter of
/// a course, and a block whose `isTemplate` is `true` (for a card, a template of its
/// board's cards) are never found. A `q` of more than 200 characters is 400.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/references/search",
    operation_id = "searchReferences",
    tag = tag::REFERENCES,
    params(MemberWorkspaceId, TitleTerm),
    responses((status = 200, description = "What was found.", body = Vec<LinkedBlock>)),
)]
async fn handle_search_references(
    Delegated(InLinked(ws), ..): Delegated<Read, InLinked>,
    QueryParams(query): QueryParams<ReferenceSearchQuery>,
) -> ApiResult {
    let found = ws
        .app
        .search_references(&ws.caller.user_id, &ws.workspace_id, &query.q)
        .await?;
    Ok(json_response(&found))
}
