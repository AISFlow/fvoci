//! Teams belong to a workspace. Nonguest members see them; its administrators manage them.

use super::contract::{
    params::{PeopleWorkspaceId, TeamId, UserId},
    tag,
};
use super::workspace::InWorkspace;
use super::{ApiResult, decode_object, json_response, no_content};
use crate::{
    app::App,
    error::AppError,
    permissions::{self, WorkspacePermission},
};
use axum::{body::Bytes, extract::Path};
use fvoci_model::{Team, TeamName};
use serde::Deserialize;
use utoipa_axum::{router::OpenApiRouter, routes};

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new()
        .routes(routes!(list_teams, create_team))
        .routes(routes!(get_team, rename_team, delete_team))
        .routes(routes!(add_team_member, remove_team_member))
}

fn managing(ws: InWorkspace) -> Result<InWorkspace, AppError> {
    let ws = ws.require_people()?;
    if !permissions::allows_in_workspace(ws.actor(), WorkspacePermission::ManageMembers) {
        return Err(AppError::forbidden(
            "only workspace administrators manage teams",
        ));
    }
    Ok(ws)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TeamPlace {
    team_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MemberPlace {
    team_id: String,
    user_id: String,
}

/// List the workspace's teams
///
/// Includes the login and display names of their nonguest members, ordered by user
/// identifier. Teams are ordered by name in any letter case, by code point and not by
/// any language's alphabet (`Émile` after `zeta`), then by identifier, alike on every
/// database. Guests are answered 404.
#[utoipa::path(get, path = "/w/{workspaceId}/teams", operation_id = "getTeams", tag = tag::WORKSPACES,
    params(PeopleWorkspaceId), responses((status = 200, description = "Teams.", body = Vec<Team>)))]
async fn list_teams(ws: InWorkspace) -> ApiResult {
    let ws = ws.require_people()?;
    Ok(json_response(&ws.app.teams(&ws.workspace_id).await?))
}

/// Create an empty team
///
/// For workspace administrators. The name is read as an organization's at sign-up:
/// surrounding
/// whitespace is removed, and a name that is then empty, longer than 64 characters or
/// holds a control character or one that shows nothing or turns the text around is 400
/// at `/name` with the reason `required`, `too_long` or `invalid_characters`. The name of
/// another team of this workspace, in any letter case, is 409 with the reason
/// `team_name_taken`.
#[utoipa::path(post, path = "/w/{workspaceId}/teams", operation_id = "createTeam", tag = tag::WORKSPACES,
    params(PeopleWorkspaceId), request_body = TeamName,
    responses((status = 200, description = "The new team.", body = Team)))]
async fn create_team(ws: InWorkspace, body: Bytes) -> ApiResult {
    let ws = managing(ws)?;
    let request: TeamName = decode_object(&body)?;
    Ok(json_response(
        &ws.app
            .create_team(&ws.workspace_id, &request.name, &ws.caller.user_id)
            .await?,
    ))
}

/// Read a team in this workspace
///
/// A team outside the workspace or a guest is answered 404.
#[utoipa::path(get, path = "/w/{workspaceId}/teams/{teamId}", operation_id = "getTeam", tag = tag::WORKSPACES,
    params(PeopleWorkspaceId, TeamId), responses((status = 200, description = "The team.", body = Team)))]
async fn get_team(ws: InWorkspace, Path(place): Path<TeamPlace>) -> ApiResult {
    let ws = ws.require_people()?;
    Ok(json_response(
        &ws.app.team(&ws.workspace_id, &place.team_id).await?,
    ))
}

/// Rename a team
///
/// For workspace administrators. The same name rules as creation apply. The team is
/// checked before the request body, so a team in another workspace is always 404.
#[utoipa::path(put, path = "/w/{workspaceId}/teams/{teamId}", operation_id = "renameTeam", tag = tag::WORKSPACES,
    params(PeopleWorkspaceId, TeamId), request_body = TeamName,
    responses((status = 200, description = "The renamed team.", body = Team)))]
async fn rename_team(ws: InWorkspace, Path(place): Path<TeamPlace>, body: Bytes) -> ApiResult {
    let ws = managing(ws)?;
    ws.app.team(&ws.workspace_id, &place.team_id).await?;
    let request: TeamName = decode_object(&body)?;
    Ok(json_response(
        &ws.app
            .rename_team(
                &ws.workspace_id,
                &place.team_id,
                &request.name,
                &ws.caller.user_id,
            )
            .await?,
    ))
}

/// Delete a team and its board roles
///
/// For workspace administrators. This does not delete the people's accounts or
/// change their membership of the workspace or their direct roles on boards.
#[utoipa::path(delete, path = "/w/{workspaceId}/teams/{teamId}", operation_id = "deleteTeam", tag = tag::WORKSPACES,
    params(PeopleWorkspaceId, TeamId), responses((status = 204, description = "The team is deleted.")))]
async fn delete_team(ws: InWorkspace, Path(place): Path<TeamPlace>) -> ApiResult {
    let ws = managing(ws)?;
    ws.app.team(&ws.workspace_id, &place.team_id).await?;
    ws.app
        .delete_team(&ws.workspace_id, &place.team_id, &ws.caller.user_id)
        .await?;
    Ok(no_content())
}

/// Add a nonguest workspace member to a team
///
/// For workspace administrators. Idempotent. Somebody outside the workspace is 404;
/// adding a workspace guest is 409, reason `guest_cannot_join_team`. Removing a
/// workspace member or making them a guest removes them from every team of that
/// workspace. A person joined as organization administrator cannot add themselves:
/// 403, reason `team_self_joining`. Other administrators may add them.
#[utoipa::path(put, path = "/w/{workspaceId}/teams/{teamId}/members/{userId}", operation_id = "addTeamMember", tag = tag::WORKSPACES,
    params(PeopleWorkspaceId, TeamId, UserId), responses((status = 204, description = "The person is in the team.")))]
async fn add_team_member(ws: InWorkspace, Path(place): Path<MemberPlace>) -> ApiResult {
    let ws = managing(ws)?;
    ws.app.team(&ws.workspace_id, &place.team_id).await?;
    ws.app
        .add_team_member(
            &ws.workspace_id,
            &place.team_id,
            &place.user_id,
            &ws.caller.user_id,
        )
        .await?;
    Ok(no_content())
}

/// Remove a person from a team
///
/// For workspace administrators. Idempotent within a team that exists.
#[utoipa::path(delete, path = "/w/{workspaceId}/teams/{teamId}/members/{userId}", operation_id = "removeTeamMember", tag = tag::WORKSPACES,
    params(PeopleWorkspaceId, TeamId, UserId), responses((status = 204, description = "The person is out of the team.")))]
async fn remove_team_member(ws: InWorkspace, Path(place): Path<MemberPlace>) -> ApiResult {
    let ws = managing(ws)?;
    ws.app.team(&ws.workspace_id, &place.team_id).await?;
    ws.app
        .remove_team_member(
            &ws.workspace_id,
            &place.team_id,
            &place.user_id,
            &ws.caller.user_id,
        )
        .await?;
    Ok(no_content())
}
