//! Searching boards.

use fvoci_model::{Container, ContainerSearchField, PublicContainers};
use serde::Deserialize;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::contract::params::{SearchField, SearchTerm, WorkspaceId};
use super::contract::tag;
use super::query::QueryParams;
use super::{Delegated, Read};

use super::workspace::InWorkspace;
use super::{ApiResult, json_response};
use crate::app::App;
use crate::error::AppError;

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
struct BoardSearchQuery {
    q: String,
    field: String,
}

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new().routes(routes!(handle_search_boards))
}

/// Search the boards of a workspace
///
/// The boards of the workspace the user finds (a member of them, or open where the user
/// works), whose title matches the term: a title matches if every word of the term, as
/// spaces divide it, is somewhere in it, whatever the case of the letters (of every
/// letter, not only ASCII); `%` and `_` are characters like any other. With
/// `field=property_name` those whose own `properties` have a member of exactly that
/// name, whatever its value (`null` too). A `field` that is neither is 400. Templates are
/// never found, and an empty term finds nothing. Oldest first.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/boards/search",
    operation_id = "searchWorkspaceBoards",
    tag = tag::CONTAINERS,
    params(WorkspaceId, SearchTerm, SearchField),
    responses((status = 200, description = "The boards that match.", body = Vec<Container>)),
)]
async fn handle_search_boards(
    Delegated(ws, ..): Delegated<Read, InWorkspace>,
    QueryParams(query): QueryParams<BoardSearchQuery>,
) -> ApiResult {
    let ws = ws.require_no_guest()?;
    let term = query.q;
    let search_field = if query.field.is_empty() {
        ContainerSearchField::Title
    } else {
        query
            .field
            .parse::<ContainerSearchField>()
            .map_err(AppError::bad_request)?
    };
    let workspace_id = &ws.workspace_id;

    let mut boards = search_boards(&ws.app, &ws.caller.user_id, &term, search_field).await?;
    boards.retain(|board| board.workspace_id == *workspace_id);
    tracing::debug!(workspace_id, boards_count = boards.len(), "SearchBoards");
    Ok(json_response(&boards))
}

/// Searches the boards the user may find. An empty term finds nothing.
async fn search_boards(
    app: &App,
    user_id: &str,
    term: &str,
    search_field: ContainerSearchField,
) -> Result<Vec<Container>, AppError> {
    if term.is_empty() {
        return Ok(Vec::new());
    }
    app.search_containers_for_user(term, search_field, user_id, PublicContainers::Include)
        .await
}
