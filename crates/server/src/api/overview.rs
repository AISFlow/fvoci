//! A workspace in one answer, as an agent reads it first: what `get_workspace_overview`
//! of the MCP server answers too.

use fvoci_model::WorkspaceOverview;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::contract::params::MemberWorkspaceId;
use super::contract::tag;
use super::workspace::{InContainer, InWorkspace};
use super::{ApiResult, Delegated, Read, json_response};
use crate::app::{App, OverviewReader};
use crate::error::AppError;

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new().routes(routes!(handle_get_workspace_overview))
}

/// The overview of a workspace for whoever entered it, on the handle they entered it
/// with.
///
/// # Errors
///
/// As `App::workspace_overview`.
pub(super) async fn overview_of(ws: &InWorkspace) -> Result<WorkspaceOverview, AppError> {
    let reader = OverviewReader {
        user_id: &ws.caller.user_id,
        role: ws.role,
        kind: ws.kind,
    };
    ws.app.workspace_overview(&ws.workspace_id, reader).await
}

/// A workspace in one answer
///
/// The workspace (its name, its organization, what that organization is and what the
/// caller is in the workspace), the caller (their login name and the name they are
/// shown by, not their email), and every container of the workspace the caller reads:
/// those they are a member of, by a role of their own or of a group they are in, and its open ones
/// unless they are a guest of it. Oldest first. An administrator is not given the
/// private containers they only manage. Each says what the caller is on it (`joined`,
/// `myRole`, absent where they are no member: an open container they read all the same),
/// and a board the properties its cards take (`cardProperties`), joined or not: what
/// `getCards` filters by and `patchCard` writes. A personal API token is given no
/// diary. 404 unless the caller is a member of the workspace.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/overview",
    operation_id = "getWorkspaceOverview",
    tag = tag::WORKSPACES,
    params(MemberWorkspaceId),
    responses((status = 200, description = "The workspace, the caller and the containers they read.", body = WorkspaceOverview)),
)]
async fn handle_get_workspace_overview(
    Delegated(InContainer(ws), ..): Delegated<Read, InContainer>,
) -> ApiResult {
    Ok(json_response(&overview_of(&ws).await?))
}
