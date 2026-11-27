//! Board members: listing, adding, changing, removing, joining and leaving.

use axum::body::Bytes;
use axum::extract::Path;
use fvoci_model::{
    ContainerMember, ContainerMemberPatch, ContainerRole, ContainerVisibility, WorkspaceRole,
};
use serde::Deserialize;
use serde_json::Value;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::contract::bodies::NewMember;
use super::contract::params::{MemberWorkspaceId, PeopleContainerId, UserId};
use super::contract::tag;
use super::workspace::InShared;

use super::{ApiResult, json_response, no_content};
use super::{decode_object_with, refusal, require_container_permission};
use crate::app::App;
use crate::error::AppError;
use crate::permissions::ContainerPermission;

/// The refusal to add, change or remove a member.
const MODIFY_MEMBERS_DENIED: &str = "access denied to modify board members";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddMemberRequest {
    #[serde(default)]
    user_id: String,
}

fn parse_member_role(value: Option<Value>) -> Result<Option<ContainerRole>, AppError> {
    match value {
        None => Ok(None),
        Some(Value::Null) => Err(AppError::bad_request("role cannot be null").with_field("role")),
        Some(raw) => serde_json::from_value::<ContainerRole>(raw)
            .map(Some)
            .map_err(|err| AppError::bad_request(err).with_field("role")),
    }
}

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new()
        .routes(routes!(handle_get_members_for_container, handle_add_member))
        .routes(routes!(handle_update_member, handle_delete_member))
        .routes(routes!(handle_join_container))
        .routes(routes!(handle_leave_container))
}

/// The members of a board
///
/// For the board's members and the workspace's administrators. Who left the workspace
/// is not listed, whatever role they had on the board. By the identifier of the user: a
/// membership has no time of its own, so this is not the order in which the users came
/// to the board.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/containers/{containerId}/members",
    operation_id = "getMembers",
    tag = tag::MEMBERS,
    params(MemberWorkspaceId, PeopleContainerId),
    responses((status = 200, description = "The members.", body = Vec<ContainerMember>)),
)]
async fn handle_get_members_for_container(
    InShared(ws): InShared,
    Path((_, container_id)): Path<(String, String)>,
) -> ApiResult {
    require_container_permission(
        &ws.app,
        &ws.caller.user_id,
        &container_id,
        ContainerPermission::ListMembers,
        "access denied to board members",
    )
    .await?;

    let mut members = ws.app.effective_container_members(&container_id).await?;
    if ws.actor().workspace == Some(WorkspaceRole::Guest) {
        for member in &mut members {
            member.role = member.effective_role();
            member.inherited_role = None;
            member.synthetic = false;
        }
    }
    Ok(json_response(&members))
}

/// Make a user a member of a board
///
/// The board's admins add members with the `role` the request names. On an open board
/// editors who work in the workspace add members too, with the board's minimum role,
/// whatever else the request names; a request of theirs that asks for `admin` is
/// 403. A guest of the workspace is given no role above editor: asking for
/// `admin` for one is 403 with the reason `guest_role_limit`. A user who is a
/// member already stays as they are. The workspace's administrators manage the members
/// of every board of it; one who could not read a board and gives somebody a role on
/// it, themselves or another, is recorded in the workspace's audit log.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/containers/{containerId}/members",
    operation_id = "addMember",
    tag = tag::MEMBERS,
    params(MemberWorkspaceId, PeopleContainerId),
    request_body = NewMember,
    responses((status = 200, description = "The membership as it is now.", body = ContainerMember)),
)]
async fn handle_add_member(
    InShared(ws): InShared,
    Path((_, container_id)): Path<(String, String)>,
    body: Bytes,
) -> ApiResult {
    let user_id = &ws.caller.user_id;
    ws.app
        .require_add_member_permission(user_id, &container_id, MODIFY_MEMBERS_DENIED)
        .await?;
    let (request, raw_role): (AddMemberRequest, Option<Value>) =
        decode_object_with(&body, |members| members.remove("role"))?;
    let requested_role = parse_member_role(raw_role)?;
    let added = ws
        .app
        .add_member(
            user_id,
            &container_id,
            &request.user_id,
            requested_role,
            MODIFY_MEMBERS_DENIED,
        )
        .await?;
    Ok(json_response(&added))
}

/// Join an open board
///
/// Makes the user a member of an open board, with the board's minimum role. A member
/// below that role is raised to it; a member with that role or a higher one stays as
/// they are. A template cannot be joined, and a private board is not there for who is
/// no member of it.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/containers/{containerId}/join",
    operation_id = "joinContainer",
    tag = tag::MEMBERS,
    params(MemberWorkspaceId, PeopleContainerId),
    responses((status = 200, description = "The membership as it is now.", body = ContainerMember)),
)]
async fn handle_join_container(
    InShared(ws): InShared,
    Path((_, container_id)): Path<(String, String)>,
) -> ApiResult {
    let user_id = &ws.caller.user_id;
    let access = ws
        .app
        .container_access(user_id, &container_id)
        .await?
        .ok_or_else(|| AppError::not_found("board"))?;
    let (container, grant) = (access.container, access.grant);

    // Joining is restricted to open boards; workspace administrators follow the same rule.
    if container.visibility != ContainerVisibility::Open {
        // A private board exists only for its members, who have nothing to join.
        return Err(refusal(grant, "cannot join a non Open board"));
    }
    // An open board is there for who works in its workspace and its members, and a
    // built-in template for everybody. To anybody else it answers as a board that does
    // not exist: joining is no way into a workspace.
    if !grant.sees() {
        return Err(AppError::not_found("board"));
    }
    // An open template is for everybody to copy, and for its members to change. Joining
    // made anyone an editor of it, of the built-in templates too: of the board every new
    // user's first board is copied from.
    if container.is_template {
        return Err(AppError::forbidden("a template cannot be joined"));
    }
    // A guest of the workspace was invited to the open board and has no more to join:
    // to them, as to anybody who does not work in the workspace, there is nothing to
    // join, which joining would raise to the board's minimum role.
    if !grant.allows(ContainerPermission::Join) {
        return Err(AppError::not_found("board"));
    }

    Ok(json_response(
        &ws.app.join_container(&container_id, user_id).await?,
    ))
}

/// Leave a board
///
/// Ends the user's own membership of a board. The last admin of a board cannot leave
/// it: that is 409 with the reason `last_admin`.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/containers/{containerId}/leave",
    operation_id = "leaveContainer",
    tag = tag::MEMBERS,
    params(MemberWorkspaceId, PeopleContainerId),
    responses((status = 204, description = "The user is no member any more.")),
)]
async fn handle_leave_container(
    InShared(ws): InShared,
    Path((_, container_id)): Path<(String, String)>,
) -> ApiResult {
    let user_id = &ws.caller.user_id;
    require_container_permission(
        &ws.app,
        user_id,
        &container_id,
        ContainerPermission::View,
        "access denied to board",
    )
    .await?;
    // The board may be deleted between the two lookups; then there is nothing to leave.
    ws.app.container(&container_id).await?;

    ws.app
        .delete_container_member(&container_id, user_id)
        .await?;
    Ok(no_content())
}

/// Change the role of a member
///
/// Replaces the `role` of a member of the board. For the board's admins. The last admin
/// of a board cannot be made less: that is 409 with the reason `last_admin`. A guest of
/// the workspace is made no more than an editor: `admin` for one is 403 with the
/// reason `guest_role_limit`. A user who is no member of the board, or of its workspace
/// any more, is 404.
#[utoipa::path(
    put,
    path = "/w/{workspaceId}/containers/{containerId}/members/{userId}",
    operation_id = "updateMember",
    tag = tag::MEMBERS,
    params(MemberWorkspaceId, PeopleContainerId, UserId),
    request_body = ContainerMemberPatch,
    responses((status = 200, description = "The membership as it is now.", body = ContainerMember)),
)]
async fn handle_update_member(
    InShared(ws): InShared,
    Path((_, container_id, member_user_id)): Path<(String, String, String)>,
    body: Bytes,
) -> ApiResult {
    let access = ws
        .app
        .container_access(&ws.caller.user_id, &container_id)
        .await?
        .ok_or_else(|| AppError::not_found("board"))?;
    if !access.grant.allows(ContainerPermission::ManageRoles) {
        return Err(refusal(access.grant, MODIFY_MEMBERS_DENIED));
    }
    // Check target container access before decoding body so absent board stays 404.
    ws.app
        .container_access(&member_user_id, &container_id)
        .await?
        .ok_or_else(|| AppError::not_found("board"))?;
    let (_, raw_role): (serde::de::IgnoredAny, Option<Value>) =
        decode_object_with(&body, |members| members.remove("role"))?;
    let role = parse_member_role(raw_role)?;
    let updated = ws
        .app
        .update_member(
            &ws.caller.user_id,
            &container_id,
            &member_user_id,
            role,
            MODIFY_MEMBERS_DENIED,
        )
        .await?;
    Ok(json_response(&updated))
}

/// Remove a member from a board
///
/// For the board's admins. The last admin of a board cannot be removed: that is 409
/// with the reason `last_admin`. Removing a user who is no member is no error, and is
/// answered 204 like a removal.
#[utoipa::path(
    delete,
    path = "/w/{workspaceId}/containers/{containerId}/members/{userId}",
    operation_id = "removeMember",
    tag = tag::MEMBERS,
    params(MemberWorkspaceId, PeopleContainerId, UserId),
    responses((status = 204, description = "The user is no member any more.")),
)]
async fn handle_delete_member(
    InShared(ws): InShared,
    Path((_, container_id, member_user_id)): Path<(String, String, String)>,
) -> ApiResult {
    require_container_permission(
        &ws.app,
        &ws.caller.user_id,
        &container_id,
        ContainerPermission::ManageRoles,
        MODIFY_MEMBERS_DENIED,
    )
    .await?;

    ws.app.container(&container_id).await?;
    ws.app
        .delete_container_member(&container_id, &member_user_id)
        .await?;
    Ok(no_content())
}
