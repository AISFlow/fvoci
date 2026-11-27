//! Team board roles. A team belongs to this workspace and never receives admin.

use axum::{body::Bytes, extract::Path};
use fvoci_model::{ContainerTeam, ContainerTeamRole};
use serde::Deserialize;
use utoipa_axum::{router::OpenApiRouter, routes};

use super::contract::{
    params::{PeopleContainerId, PeopleWorkspaceId, TeamId},
    tag,
};
use super::workspace::InShared;
use super::{ApiResult, decode_object, json_response, no_content, require_container_permission};
use crate::{app::App, permissions::ContainerPermission};

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new()
        .routes(routes!(list))
        .routes(routes!(assign, remove))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContainerPlace {
    container_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TeamPlace {
    container_id: String,
    team_id: String,
}

/// List the teams assigned to a board
///
/// For nonguest members allowed to list board members. Includes the team's current
/// people and its role. Ordered by team name in any letter case, by code point, then by
/// identifier.
/// Direct memberships remain separate.
#[utoipa::path(get, path = "/w/{workspaceId}/containers/{containerId}/teams", operation_id = "getContainerTeams", tag = tag::MEMBERS,
    params(PeopleWorkspaceId, PeopleContainerId),
    responses((status = 200, description = "Assigned teams.", body = Vec<ContainerTeam>)))]
async fn list(InShared(ws): InShared, Path(place): Path<ContainerPlace>) -> ApiResult {
    let ws = ws.require_people()?;
    require_container_permission(
        &ws.app,
        &ws.caller.user_id,
        &place.container_id,
        ContainerPermission::ListMembers,
        "access denied to board members",
    )
    .await?;
    Ok(json_response(
        &ws.app
            .container_teams(&ws.workspace_id, &place.container_id)
            .await?,
    ))
}

/// Give or change a team's board role
///
/// For board administrators and workspace administrators. The role is viewer,
/// commenter, or editor; admin is rejected. The board and team must belong to this
/// workspace and are checked before decoding the body. Managed access is audited.
#[utoipa::path(put, path = "/w/{workspaceId}/containers/{containerId}/teams/{teamId}", operation_id = "setContainerTeam", tag = tag::MEMBERS,
    params(PeopleWorkspaceId, PeopleContainerId, TeamId), request_body = ContainerTeamRole,
    responses((status = 204, description = "The team has this role.")))]
async fn assign(InShared(ws): InShared, Path(place): Path<TeamPlace>, body: Bytes) -> ApiResult {
    let ws = ws.require_people()?;
    require_container_permission(
        &ws.app,
        &ws.caller.user_id,
        &place.container_id,
        ContainerPermission::ManageRoles,
        "access denied to modify board members",
    )
    .await?;
    ws.app.team(&ws.workspace_id, &place.team_id).await?;
    let request: ContainerTeamRole = decode_object(&body)?;
    ws.app
        .set_container_team(
            &ws.workspace_id,
            &place.container_id,
            &place.team_id,
            Some(request.role),
            &ws.caller.user_id,
        )
        .await?;
    Ok(no_content())
}

/// Remove a team's board role
///
/// For board administrators and workspace administrators. Direct roles and the
/// roles inherited from other teams remain. Idempotent for an existing team.
#[utoipa::path(delete, path = "/w/{workspaceId}/containers/{containerId}/teams/{teamId}", operation_id = "removeContainerTeam", tag = tag::MEMBERS,
    params(PeopleWorkspaceId, PeopleContainerId, TeamId), responses((status = 204, description = "The team has no role on this board.")))]
async fn remove(InShared(ws): InShared, Path(place): Path<TeamPlace>) -> ApiResult {
    let ws = ws.require_people()?;
    require_container_permission(
        &ws.app,
        &ws.caller.user_id,
        &place.container_id,
        ContainerPermission::ManageRoles,
        "access denied to modify board members",
    )
    .await?;
    ws.app.team(&ws.workspace_id, &place.team_id).await?;
    ws.app
        .set_container_team(
            &ws.workspace_id,
            &place.container_id,
            &place.team_id,
            None,
            &ws.caller.user_id,
        )
        .await?;
    Ok(no_content())
}
