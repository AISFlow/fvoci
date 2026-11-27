//! The members of a workspace: who is in it, what each is there, and taking one out.
//!
//! For the workspace's administrators. A guest of the workspace is told that it is not
//! there, as on the rest of it.

use axum::body::Bytes;
use axum::extract::Path;
use fvoci_model::{WorkspaceMember, WorkspaceRoleChange};
use serde::Deserialize;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::contract::params::{PeopleWorkspaceId, UserId, WorkerWorkspaceId};
use super::contract::tag;
use super::workspace::InWorkspace;
use super::{ApiResult, decode_object, json_response, no_content};
use crate::app::App;
use crate::error::AppError;
use crate::permissions::{self, WorkspacePermission};

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new()
        .routes(routes!(handle_get_workspace_members))
        .routes(routes!(
            handle_set_workspace_role,
            handle_remove_workspace_member
        ))
}

/// The workspace for who manages its members: an administrator of it.
///
/// # Errors
///
/// Returns 404 for a guest of the workspace, and 403 for anybody else who is no
/// administrator of it.
fn managing(ws: InWorkspace) -> Result<InWorkspace, AppError> {
    let ws = ws.require_no_guest()?;
    if !permissions::allows_in_workspace(ws.actor(), WorkspacePermission::ManageMembers) {
        return Err(AppError::forbidden(
            "only the administrators of the workspace manage its members",
        ));
    }
    Ok(ws)
}

/// The workspace for who changes what somebody is in it, or takes somebody out: an
/// administrator of it, where there are people besides. A user's own space has its
/// owner in it and nobody else, and its owner stays what they are there.
///
/// # Errors
///
/// Returns 404 for the workspace of a user's own space and for a guest, and 403 for
/// anybody else who is no administrator of the workspace.
fn managing_people(ws: InWorkspace) -> Result<InWorkspace, AppError> {
    managing(ws.require_people()?)
}

/// The member a request names in its path.
#[derive(Debug, Deserialize)]
struct MemberPlace {
    #[serde(rename = "userId")]
    user_id: String,
}

/// The members of a workspace
///
/// Everybody who is in the workspace, its guests too, each with their login name, the
/// name they are shown by and what they are in the workspace (`admin`, `member` or
/// `guest`). The oldest membership first, and then by the identifier of the user.
///
/// For the workspace's administrators: anybody else who works there is refused (403),
/// and a guest of the workspace is told that it is not there (404).
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/members",
    operation_id = "getWorkspaceMembers",
    tag = tag::WORKSPACES,
    params(WorkerWorkspaceId),
    responses((status = 200, description = "The members.", body = Vec<WorkspaceMember>)),
)]
async fn handle_get_workspace_members(ws: InWorkspace) -> ApiResult {
    let ws = managing(ws)?;
    Ok(json_response(
        &ws.app.workspace_members(&ws.workspace_id).await?,
    ))
}

/// Change what a member is in a workspace
///
/// Makes a member an administrator, an administrator a member, or a member a guest,
/// and answers the member; what they are already they are made again, and nothing
/// changes. A guest becomes a member by accepting an invitation for members, and an
/// administrator a guest in two steps: those are 400, at `/role`.
///
/// Who becomes a guest keeps the boards they are on, as an editor at most. A board
/// they were the only admin of passes to the user who makes the change, who is its
/// admin from then on: no board is left without one.
///
/// The workspace keeps an administrator: the last one is not made anything else (409,
/// reason `last_admin`). Somebody who is no member of the workspace is 404.
///
/// For the workspace's administrators: anybody else who works there is refused (403),
/// and a guest of the workspace is told that it is not there (404). The workspace of
/// a user's own space is 404: its owner is what they are there, and nobody else is in
/// it.
#[utoipa::path(
    put,
    path = "/w/{workspaceId}/members/{userId}",
    operation_id = "setWorkspaceRole",
    tag = tag::WORKSPACES,
    params(PeopleWorkspaceId, UserId),
    request_body = WorkspaceRoleChange,
    responses((status = 200, description = "The member.", body = WorkspaceMember)),
)]
async fn handle_set_workspace_role(
    ws: InWorkspace,
    Path(place): Path<MemberPlace>,
    body: Bytes,
) -> ApiResult {
    let ws = managing_people(ws)?;
    let change: WorkspaceRoleChange = decode_object(&body)?;
    let member = ws
        .app
        .change_workspace_role(
            &ws.workspace_id,
            &place.user_id,
            change.role,
            &ws.caller.user_id,
        )
        .await?;
    Ok(json_response(&member))
}

/// Take a member out of a workspace
///
/// The workspace is not there for the user from then on (404), and nothing in it:
/// their roles on its boards go with them, and a link they made lets nobody in. What
/// they wrote stays, under their name. Their account stays, and they come back by an
/// invitation, with nothing of before.
///
/// A board they were the only admin of passes to the user who takes them out, who is
/// its admin from then on: no board is left without one.
///
/// The workspace keeps an administrator: its last one is not taken out (409, reason
/// `last_admin`), and an administrator is taken out by another, not by themselves
/// (403). Somebody who is no member of the workspace is 404.
///
/// For the workspace's administrators: anybody else who works there is refused (403),
/// and a guest of the workspace is told that it is not there (404). The workspace of
/// a user's own space is 404: nobody is taken out of it.
#[utoipa::path(
    delete,
    path = "/w/{workspaceId}/members/{userId}",
    operation_id = "removeWorkspaceMember",
    tag = tag::WORKSPACES,
    params(PeopleWorkspaceId, UserId),
    responses((status = 204, description = "The member is out of the workspace.")),
)]
async fn handle_remove_workspace_member(
    ws: InWorkspace,
    Path(place): Path<MemberPlace>,
) -> ApiResult {
    let ws = managing_people(ws)?;
    ws.app
        .remove_workspace_member(&ws.workspace_id, &place.user_id, &ws.caller.user_id)
        .await?;
    Ok(no_content())
}
