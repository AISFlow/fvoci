//! A shared board, wiki or course as whoever holds its link reads it:
//! `/shared/{containerId}`, with its read token and no session. A board's link reads its
//! views, cards and comments; a wiki's its tree and pages; a course's its outline and
//! units; and none reads the routes of another. All three read their files.
//!
//! These routes look at nothing of the request but the container, the token and the
//! client's address: a session that comes along is not read, so who is signed in reads
//! a shared container as anybody else with the link does. What they answer names no user:
//! who made or changed the container, a block, a card, a page or a unit, the values of
//! person properties, the people of a view's filter, and the mentions of a body are
//! emptied. Nor does it name anything that is not of the container: a reference in a
//! body keeps its identifier for a card of the board, a page of the wiki, or a unit of
//! the course, and goes out with an empty one for anything else.
//!
//! A token that opens nothing, a container that is not shared and a container in the
//! trash answer alike, 404. Each such answer, and a read that fails before its token is
//! looked at, counts against the client's address (`App::shared_reads`), so that tokens
//! cannot be guessed at speed.

use axum::extract::rejection::RawPathParamsRejection;
use axum::extract::{FromRequestParts, Path, RawPathParams};
use axum::http::HeaderMap;
use axum::http::request::Parts;
use fvoci_model::{
    Block, Card, Container, ContainerKind, FileRecord, Page, PageNode, PersonProperties, Unit,
    UnitNode,
};
use serde::Deserialize;
use serde_json::Value;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::blocks::{BlocksFilterQuery, asked_blocks};
use super::cards::{CardsListQuery, cards_page};
use super::contract::bodies::Binary;
use super::contract::params::{
    AllBlocks, BlockTypeFilter, ContainerId, FileId, OneBlockId, Page as PageNumber, PageId,
    ParentId, PerPage, Preview, SharedReadToken, UnitId,
};
use super::contract::tag;
use super::files::{container_file_record, serve_container_file};
use super::query::{QueryParams, parse_query_map};
use super::{ApiResult, json_response};
use crate::app::App;
use crate::client_address::ClientAddress;
use crate::error::AppError;

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
struct SharedTokenQuery {
    read_token: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
struct SharedBlocksQuery {
    read_token: String,
    all: String,
    block_id: String,
    parent_id: String,
    #[serde(rename = "type")]
    block_type: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
struct SharedCardsQuery {
    read_token: String,
    page: String,
    per_page: String,
    preview: String,
}

/// The path parameter that names the shared board.
const CONTAINER_ID: &str = "containerId";

/// The query parameter that carries the read token.
const READ_TOKEN: &str = "readToken";

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new()
        .routes(routes!(handle_get_shared_container))
        .routes(routes!(handle_get_shared_blocks))
        .routes(routes!(handle_get_shared_cards))
        .routes(routes!(handle_get_shared_file))
        .routes(routes!(handle_get_shared_file_record))
        .routes(routes!(handle_get_shared_page_tree))
        .routes(routes!(handle_get_shared_page))
        .routes(routes!(handle_get_shared_unit_tree))
        .routes(routes!(handle_get_shared_unit))
}

/// The board, wiki or course a request under `/shared/{containerId}` names, opened by the
/// read token of the request: the application behind the wall of its own organization,
/// reaching its kind alone.
///
/// Extraction fails with 404 unless the token opens the container, and with 429 for an
/// address that has had too many such answers of late.
#[derive(Debug)]
pub(crate) struct InSharedContainer {
    /// The application, reaching the rows of the board's organization.
    app: App,
    /// The board the path names.
    container_id: String,
}

impl FromRequestParts<App> for InSharedContainer {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, app: &App) -> Result<Self, Self::Rejection> {
        let address = parts
            .extensions
            .get::<ClientAddress>()
            .and_then(|address| address.0);
        app.shared_reads().admit(address)?;
        let container_id = match RawPathParams::from_request_parts(parts, app).await {
            Ok(params) => params
                .iter()
                .find(|(name, _)| *name == CONTAINER_ID)
                .map(|(_, value)| value.to_owned())
                .ok_or_else(|| AppError::internal("the route names no board"))?,
            // A path that is no text names no board that is shared.
            Err(RawPathParamsRejection::InvalidUtf8InPathParam(_)) => String::new(),
            Err(_) => return Err(AppError::internal("the route has no path parameters")),
        };
        let query_map = parse_query_map(parts.uri.query());
        let token = query_map
            .get(READ_TOKEN)
            .and_then(Value::as_str)
            .unwrap_or_default();
        let Some(app) = app.enter_shared_container(&container_id, token).await? else {
            app.shared_reads().failed(address);
            return Err(not_shared());
        };
        Ok(Self { app, container_id })
    }
}

/// What a board that is not there for the holder of the link answers.
fn not_shared() -> AppError {
    AppError::not_found("board")
}

impl InSharedContainer {
    /// Whether what the link opens is of `kind`: a board's link reads only blocks and
    /// cards, a wiki's only pages, and a course's only units; any other kind's routes
    /// answer as not there (404). Every route but the container itself and its files
    /// asks it.
    fn require_kind(&self, kind: ContainerKind) -> Result<(), AppError> {
        if self.app.kinds().holds(kind) {
            Ok(())
        } else {
            Err(not_shared())
        }
    }

    /// The board, as the holder of the link reads it: naming nobody, and not its
    /// workspace, which is there only for its members. A board that went between the
    /// token's check and this read is not there either.
    async fn container(&self) -> Result<Container, AppError> {
        let mut container = self.app.container(&self.container_id).await?;
        container.hide_people();
        container.workspace_id.clear();
        Ok(container)
    }
}

/// A shared board, wiki or course
///
/// The board, wiki or course (its `kind` says which) whose read token the request has; no
/// session is needed, and one that comes along is not looked at. Who made it or
/// changed it last is not told (`createdBy` and `modifiedBy` are empty), nor its
/// workspace (`workspaceId` is empty). 404 unless the token is its own and it is shared
/// and not in the trash, and its organization has public links on; 429 for an address
/// that has had many such answers within a minute.
#[utoipa::path(
    get,
    path = "/shared/{containerId}",
    operation_id = "getSharedContainer",
    tag = tag::PUBLIC_LINKS,
    params(ContainerId, SharedReadToken),
    security(()),
    responses((status = 200, description = "The board, wiki or course.", body = Container)),
)]
async fn handle_get_shared_container(
    shared: InSharedContainer,
    QueryParams(_): QueryParams<SharedTokenQuery>,
) -> ApiResult {
    Ok(json_response(&shared.container().await?))
}

/// The blocks of a shared board
///
/// What `getBlocks` gives of the board, with the same parameters, read with its read
/// token as `getSharedContainer` is; a wiki's or a course's token reads no blocks (404). No block names a
/// user: `createdBy` and `modifiedBy` are empty, so is the `userId` of every mention in
/// a body, and so is each user that
/// the board's person properties (`person`, `multiPerson`, `createdBy`, `updatedBy`)
/// name in a card's `fields.properties` and in the clauses of a view's `fields.filter`;
/// a list keeps its length, so how many people a card has still shows. A card's values
/// of properties the board does not define are left out, and a view grouped by a person
/// property keeps of `visibleGroups`, `hiddenGroups`, `collapsedGroups` and
/// `groupCalculations` only the group of the cards without a value (`""`). A reference
/// in a body keeps its `id` if it names a card of this board, and has an empty one for
/// anything else: of what is elsewhere in the workspace nothing is told, nor whether
/// there is anything. Oldest
/// first; the blocks that one request made come in the order that request had them in.
#[utoipa::path(
    get,
    path = "/shared/{containerId}/blocks",
    operation_id = "getSharedBlocks",
    tag = tag::PUBLIC_LINKS,
    params(ContainerId, SharedReadToken, AllBlocks, OneBlockId, ParentId, BlockTypeFilter),
    security(()),
    responses((status = 200, description = "The blocks.", body = Vec<Block>)),
)]
async fn handle_get_shared_blocks(
    shared: InSharedContainer,
    QueryParams(query): QueryParams<SharedBlocksQuery>,
) -> ApiResult {
    shared.require_kind(ContainerKind::Board)?;
    let container = shared.container().await?;
    let filter = BlocksFilterQuery {
        all: query.all,
        block_id: query.block_id,
        parent_id: query.parent_id,
        block_type: query.block_type,
    };
    let mut blocks = asked_blocks(&shared.app, &shared.container_id, &filter).await?;
    let people = container.person_properties();
    let bodies = blocks.iter().filter_map(|block| block.body.as_ref());
    let shown = shared
        .app
        .cards_named_on(&shared.container_id, bodies)
        .await?;
    for block in &mut blocks {
        block.hide_people(&people);
        block.clear_references_but(&shown);
    }
    Ok(json_response(&blocks))
}

/// The cards of a shared board
///
/// What `getCards` gives of the board, read with its read token as `getSharedContainer` is;
/// a wiki's or a course's token reads no cards (404).
/// No card names a user: `createdBy` and `modifiedBy` are empty, the values of person
/// properties are emptied, and values of properties the board does not define are left
/// out. The preview's text has no mentions in it, and its image `getSharedFile` gives.
/// A card comes without its body, so with nothing a body names. Oldest first; the
/// pages cut that order.
#[utoipa::path(
    get,
    path = "/shared/{containerId}/cards",
    operation_id = "getSharedCards",
    tag = tag::PUBLIC_LINKS,
    params(ContainerId, SharedReadToken, PageNumber, PerPage, Preview),
    security(()),
    responses((status = 200, description = "The cards of the page.", body = Vec<Card>)),
)]
async fn handle_get_shared_cards(
    shared: InSharedContainer,
    QueryParams(query): QueryParams<SharedCardsQuery>,
) -> ApiResult {
    shared.require_kind(ContainerKind::Board)?;
    let container = shared.container().await?;
    let list_query = CardsListQuery {
        page: query.page,
        per_page: query.per_page,
        preview: query.preview,
        ..CardsListQuery::default()
    };
    let mut cards = cards_page(&shared.app, &shared.container_id, &list_query).await?;
    let people = container.person_properties();
    for card in &mut cards {
        card.hide_people(&people);
    }
    Ok(json_response(&cards))
}

/// A file of a shared board, wiki or course
///
/// What `getFile` gives of a file of the board, wiki or course, read with its read token
/// as `getSharedContainer` is: a file of another container is not there.
#[utoipa::path(
    get,
    path = "/shared/{containerId}/files/{fileId}",
    operation_id = "getSharedFile",
    tag = tag::PUBLIC_LINKS,
    params(ContainerId, FileId, SharedReadToken),
    security(()),
    responses(
        (status = 200, description = "The file.", body = inline(Binary), content_type = "application/octet-stream"),
        (status = 206, description = "The range of the file that was asked for, as `getFile` answers it.", body = inline(Binary), content_type = "application/octet-stream"),
    ),
)]
async fn handle_get_shared_file(
    shared: InSharedContainer,
    Path((_, file_id)): Path<(String, String)>,
    QueryParams(_): QueryParams<SharedTokenQuery>,
    headers: HeaderMap,
) -> ApiResult {
    serve_container_file(&shared.app, &shared.container_id, &file_id, &headers).await
}

/// What is known about a file of a shared board, wiki or course
///
/// What `getFileRecord` gives of a file of the board, wiki or course, read with its read
/// token as `getSharedContainer` is.
#[utoipa::path(
    get,
    path = "/shared/{containerId}/files/{fileId}/info",
    operation_id = "getSharedFileRecord",
    tag = tag::PUBLIC_LINKS,
    params(ContainerId, FileId, SharedReadToken),
    security(()),
    responses((status = 200, description = "What is known about the file.", body = FileRecord)),
)]
async fn handle_get_shared_file_record(
    shared: InSharedContainer,
    Path((_, file_id)): Path<(String, String)>,
    QueryParams(_): QueryParams<SharedTokenQuery>,
) -> ApiResult {
    Ok(json_response(
        &container_file_record(&shared.app, &shared.container_id, &file_id).await?,
    ))
}

/// The pages of a shared wiki
///
/// What `getPageTree` gives of the wiki, read with its read token as `getSharedContainer`
/// is: every page without its body, naming nobody. By `order` as strings compare by
/// code unit, then by `id`. A board's or a course's token reads no pages (404).
#[utoipa::path(
    get,
    path = "/shared/{containerId}/pages",
    operation_id = "getSharedPageTree",
    tag = tag::PUBLIC_LINKS,
    params(ContainerId, SharedReadToken),
    security(()),
    responses((status = 200, description = "The pages.", body = Vec<PageNode>)),
)]
async fn handle_get_shared_page_tree(
    shared: InSharedContainer,
    QueryParams(_): QueryParams<SharedTokenQuery>,
) -> ApiResult {
    shared.require_kind(ContainerKind::Wiki)?;
    Ok(json_response(
        &shared.app.page_tree(&shared.container_id).await?,
    ))
}

/// A page of a shared wiki
///
/// What `getPage` gives of a page of the wiki, read with its read token as
/// `getSharedContainer` is. No page names a user: `createdBy` and `modifiedBy` are empty,
/// and so is the `userId` of every mention in its body. A reference in the body keeps
/// its `id` if it names a page of this wiki, and has an empty one for anything else: of
/// what is elsewhere in the workspace nothing is told, nor whether there is anything.
/// Its comments and its versions are not read by the link. A page of another wiki, one
/// in the trash, and a board's or a course's token are 404.
#[utoipa::path(
    get,
    path = "/shared/{containerId}/pages/{pageId}",
    operation_id = "getSharedPage",
    tag = tag::PUBLIC_LINKS,
    params(ContainerId, PageId, SharedReadToken),
    security(()),
    responses((status = 200, description = "The page.", body = Page)),
)]
async fn handle_get_shared_page(
    shared: InSharedContainer,
    Path((_, page_id)): Path<(String, String)>,
    QueryParams(_): QueryParams<SharedTokenQuery>,
) -> ApiResult {
    shared.require_kind(ContainerKind::Wiki)?;
    let mut page = shared.app.page_of(&shared.container_id, &page_id).await?;
    let shown = shared
        .app
        .pages_named_on(&shared.container_id, page.body.as_ref())
        .await?;
    // A wiki has no properties of people: the writers and the mentions are what name one.
    page.hide_people(&PersonProperties::default());
    page.clear_references_but(&shown);
    Ok(json_response(&Page::from(page)))
}

/// The chapters and units of a shared course
///
/// What `getUnitTree` gives of the course, read with its read token as
/// `getSharedContainer` is: every chapter and unit without its body, naming nobody. By
/// `order` as strings compare by code unit, then by `id`. A board's or a wiki's token
/// reads no units (404).
#[utoipa::path(
    get,
    path = "/shared/{containerId}/units",
    operation_id = "getSharedUnitTree",
    tag = tag::PUBLIC_LINKS,
    params(ContainerId, SharedReadToken),
    security(()),
    responses((status = 200, description = "The chapters and units.", body = Vec<UnitNode>)),
)]
async fn handle_get_shared_unit_tree(
    shared: InSharedContainer,
    QueryParams(_): QueryParams<SharedTokenQuery>,
) -> ApiResult {
    shared.require_kind(ContainerKind::Course)?;
    Ok(json_response(
        &shared.app.unit_tree(&shared.container_id).await?,
    ))
}

/// A unit of a shared course
///
/// What `getUnit` gives of a unit of the course, read with its read token as
/// `getSharedContainer` is. No unit names a user: `createdBy` and `modifiedBy` are
/// empty, and so is the `userId` of every mention in its body. A reference in the body
/// keeps its `id` if it names a unit of this course, and has an empty one for anything
/// else: of what is elsewhere in the workspace nothing is told, nor whether there is
/// anything. Its comments and its versions are not read by the link. A chapter, a unit
/// of another course, one in the trash, and a board's or wiki's token are 404.
#[utoipa::path(
    get,
    path = "/shared/{containerId}/units/{unitId}",
    operation_id = "getSharedUnit",
    tag = tag::PUBLIC_LINKS,
    params(ContainerId, UnitId, SharedReadToken),
    security(()),
    responses((status = 200, description = "The unit.", body = Unit)),
)]
async fn handle_get_shared_unit(
    shared: InSharedContainer,
    Path((_, unit_id)): Path<(String, String)>,
    QueryParams(_): QueryParams<SharedTokenQuery>,
) -> ApiResult {
    shared.require_kind(ContainerKind::Course)?;
    let mut unit = shared
        .app
        .content_unit_of(&shared.container_id, &unit_id)
        .await?;
    let shown = shared
        .app
        .units_named_on(&shared.container_id, unit.body.as_ref())
        .await?;
    // A course has no properties of people: the writers and the mentions are what name one.
    unit.hide_people(&PersonProperties::default());
    unit.clear_references_but(&shown);
    Ok(json_response(&Unit::from(unit)))
}
