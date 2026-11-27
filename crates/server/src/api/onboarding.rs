//! A new account's first board.

use axum::http::HeaderMap;
use serde::Serialize;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::contract::params::{AcceptLanguage, WorkspaceId};
use super::contract::tag;

use super::workspace::InWorkspace;
use super::{ApiResult, accept_language, json_response};
use crate::app::App;

/// What asking for a first board answers: where it is.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
struct WelcomeBoard {
    /// The identifier of the new board.
    board_id: String,
}

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new().routes(routes!(handle_onboard))
}

/// Make the welcome board
///
/// Copies the built-in welcome board for the user, as a private board of their own in
/// the workspace: what a new account starts on. It is written in the language of the
/// user's settings; where that is left to the device, or is one the board is not
/// written in, in the one `Accept-Language` prefers; failing both, in English. Every
/// request makes another copy.
///
/// Whoever starts an organization asks for it. An account that comes into an
/// organization as a member (by a link, by a registration made by a link, or by
/// signing in through the organization's provider) is given one without asking, in
/// the workspace it comes into, unless it has one: an account is given one once,
/// whatever it comes into after, and a guest none.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/onboard",
    operation_id = "onboard",
    tag = tag::WORKSPACES,
    params(WorkspaceId, AcceptLanguage),
    responses((status = 200, description = "Where the new board is.", body = WelcomeBoard)),
)]
async fn handle_onboard(ws: InWorkspace, headers: HeaderMap) -> ApiResult {
    let ws = ws.require_no_guest()?;
    let (app, user_id, workspace_id) = (&ws.app, ws.caller.user_id.as_str(), &ws.workspace_id);
    let board_id = app
        .create_welcome_board(user_id, workspace_id, accept_language(&headers))
        .await?;
    Ok(json_response(&WelcomeBoard { board_id }))
}
