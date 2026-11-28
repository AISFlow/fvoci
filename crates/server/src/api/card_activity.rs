//! The activity of a card (`getCardActivity`): who made it and changed its title, a
//! value of a property, its parent card or whether it is a template, or brought it
//! back from the trash, and when.

use axum::extract::Path;
use fvoci_model::CardEvent;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::cards::card_with_permission;
use super::contract::params::{CardId, MemberWorkspaceId, Page, PerPage};
use super::contract::tag;
use super::query::{PagingQuery, QueryParams};
use super::workspace::InWorkspace;
use super::{ApiResult, Delegated, Read, json_response};
use crate::app::App;
use crate::error::AppError;
use crate::permissions::ContainerPermission;

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new().routes(routes!(handle_get_card_activity))
}

/// The activity of a card
///
/// What was done to the card, the newest first: its making (new, a copy, from a
/// template, by an import, or as the next of a card that repeats), each change of its
/// title, of the value of a property, of its parent card and of whether it is a
/// template, and its coming back from the trash. Its body is not: its versions say what
/// it was (`getCardBodyVersions`); nor are its comments, its attachments, or what a copy
/// or an import wrote over a card that was there.
///
/// A line says what changed as the card held it: a title as it was written, the value
/// of a property as the card stores it (the identifier of an option or of a person, a
/// date value), the parent card by its identifier (`null` for the top of the board),
/// whether it was a template; the names are the client's to read now, so that what was
/// renamed since reads as it is called now and what is gone reads as gone. Who did it
/// is `actorId`, and `actorTokenName` the name of the personal API token an agent did it
/// through; both are `null` for what the server did itself and once the account is
/// erased. The same person changing the same thing again within five minutes, with
/// nobody else's change of it between, changes their line (`updatedAt`) rather than
/// adding one, and changing it back to what it was takes the line away. A card keeps
/// its 500 newest lines; they stay while it is in the trash and go with it for good.
///
/// For the members of the card's board, whatever their role; to a user who is no
/// member of it there is no such card, and a card in the trash is not there (404). Not
/// for a reader by the board's read token (`/shared`).
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/cards/{cardId}/activity",
    operation_id = "getCardActivity",
    tag = tag::CARDS,
    params(MemberWorkspaceId, CardId, Page, PerPage),
    responses((status = 200, description = "The lines, the newest first.", body = Vec<CardEvent>)),
)]
async fn handle_get_card_activity(
    Delegated(ws, ..): Delegated<Read, InWorkspace>,
    Path((_, card_id)): Path<(String, String)>,
    QueryParams(query): QueryParams<PagingQuery>,
) -> ApiResult {
    Ok(json_response(
        &card_activity_of(&ws, &card_id, query).await?,
    ))
}

/// What `getCardActivity` answers of the card `card_id` for the page `query` names. The
/// MCP server's `list_activity` answers it too.
///
/// # Errors
///
/// As `getCardActivity` answers.
pub(super) async fn card_activity_of(
    ws: &InWorkspace,
    card_id: &str,
    query: PagingQuery,
) -> Result<Vec<CardEvent>, AppError> {
    let page = query.into_list_page()?;
    let card = card_with_permission(
        &ws.app,
        &ws.caller.user_id,
        card_id,
        ContainerPermission::View,
        "access denied to fetch the activity of the card",
    )
    .await?;
    ws.app.card_activity(&card.id, page).await
}
