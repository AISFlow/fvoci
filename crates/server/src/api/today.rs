//! Account-wide (`GET /users/me/today`) and workspace (`GET /w/{workspaceId}/today`)
//! `'Today'` hub endpoints.

use axum::extract::State;
use fvoci_model::{AccountToday, WorkspaceToday};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::contract::params::WorkspaceId;
use super::contract::tag;
use super::workspace::InContainer;
use super::{ApiResult, RequiredSession, json_response};
use super::{Delegated, Read};
use crate::app::App;

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new()
        .routes(routes!(handle_get_account_today))
        .routes(routes!(handle_get_workspace_today))
}

/// Today across all open workspaces
///
/// Aggregates incomplete cards assigned to the caller that are due today, due soon, or
/// overdue across every workspace opened by the current session, together with scheduled
/// units on enrolled courses, pending spaced-repetition reviews due today or earlier, and
/// upcoming study meetings within 48 hours. Workspaces locked behind an unfulfilled SSO
/// provider or personal-space password confirmation are omitted and counted in
/// `lockedWorkspaceCount`.
#[utoipa::path(
    get,
    path = "/users/me/today",
    operation_id = "getAccountToday",
    tag = tag::USERS,
    responses((status = 200, description = "Today's items across session-opened workspaces.", body = AccountToday)),
)]
async fn handle_get_account_today(
    State(app): State<App>,
    RequiredSession(session): RequiredSession,
) -> ApiResult {
    let today = app.account_today(&session).await?;
    Ok(json_response(&today))
}

/// Today in one workspace
///
/// Returns the caller's due/overdue incomplete cards, scheduled course units, due
/// reviews, and upcoming study meetings inside `workspaceId`. 404 for guests and
/// non-members.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/today",
    operation_id = "getWorkspaceToday",
    tag = tag::WORKSPACES,
    params(WorkspaceId),
    responses((status = 200, description = "Today's items in the workspace.", body = WorkspaceToday)),
)]
async fn handle_get_workspace_today(
    Delegated(InContainer(ws), ..): Delegated<Read, InContainer>,
) -> ApiResult {
    let ws = ws.require_no_guest()?;
    let today = ws
        .app
        .workspace_today(&ws.workspace_id, &ws.caller.user_id)
        .await?;
    Ok(json_response(&today))
}
