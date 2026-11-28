//! Lists an agent reads to find its way: the containers of a workspace by kind, by
//! visibility, by title and by membership, and the files of a board or of one card.

use std::collections::BTreeSet;

use axum::extract::Path;
use fvoci_model::{BlockType, Container, ContainerKind, ContainerVisibility, ListedFile};
use serde::Deserialize;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::cards::card_with_permission;
use super::contract::params::{
    BoardId, CardId, ContainerId, ContainerKindFilter, ContainerText, ContainerVisibilityFilter,
    JoinedOnly, MemberWorkspaceId,
};
use super::contract::tag;
use super::files::require_file_reader;
use super::query::QueryParams;
use super::workspace::{InContainer, InWorkspace};
use super::{ApiResult, Delegated, Read, TRUE, json_response};

use crate::app::App;
use crate::error::AppError;
use crate::permissions::ContainerPermission;

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new()
        .routes(routes!(handle_list_workspace_containers))
        .routes(routes!(handle_list_container_files))
        .routes(routes!(handle_list_card_files))
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
struct ContainersQuery {
    kind: Option<ContainerKind>,
    visibility: Option<ContainerVisibility>,
    q: String,
    joined_only: String,
}

/// The containers of a workspace
///
/// The boards, wikis, courses and diaries of the workspace the caller reads, as
/// `getWorkspaceBoards` lists its boards (an administrator also those they only manage), of
/// the `kind` and `visibility` asked for, whose title holds `q`; with `joinedOnly=true`
/// only those the caller is a member of. Oldest first. A personal API token is given
/// no diary.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/containers",
    operation_id = "listWorkspaceContainers",
    tag = tag::CONTAINERS,
    params(
        MemberWorkspaceId,
        ContainerKindFilter,
        ContainerVisibilityFilter,
        ContainerText,
        JoinedOnly
    ),
    responses((status = 200, description = "The containers.", body = Vec<Container>)),
)]
async fn handle_list_workspace_containers(
    Delegated(InContainer(ws), ..): Delegated<Read, InContainer>,
    QueryParams(query): QueryParams<ContainersQuery>,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    let containers = if query.joined_only == TRUE {
        ws.app.joined_containers(user_id, &ws.workspace_id).await?
    } else {
        ws.app
            .workspace_containers(user_id, &ws.workspace_id, ws.role)
            .await?
    };
    let text = query.q.to_lowercase();
    let chosen: Vec<Container> = containers
        .into_iter()
        .filter(|container| query.kind.is_none_or(|kind| container.kind == kind))
        .filter(|container| {
            query
                .visibility
                .is_none_or(|visibility| container.visibility == visibility)
        })
        .filter(|container| container.title.to_lowercase().contains(&text))
        .collect();
    Ok(json_response(&chosen))
}

/// The files of a board
///
/// The files uploaded to the board or wiki, for whoever reads it, oldest first: what a
/// body names by `fileId` (in Markdown `fvoci-file://{fileId}`). A file no body or
/// attachment shows any more is listed until the server lets it go.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/containers/{containerId}/files",
    operation_id = "listContainerFiles",
    tag = tag::FILES,
    params(MemberWorkspaceId, ContainerId),
    responses((status = 200, description = "The files.", body = Vec<ListedFile>)),
)]
async fn handle_list_container_files(
    Delegated(InContainer(ws), ..): Delegated<Read, InContainer>,
    Path((_, container_id)): Path<(String, String)>,
) -> ApiResult {
    Ok(json_response(
        &files_of_container(&ws, &container_id).await?,
    ))
}

/// The files of the board or wiki `container_id`, as `listContainerFiles` lists them.
///
/// # Errors
///
/// As `listContainerFiles` answers.
pub(super) async fn files_of_container(
    ws: &InWorkspace,
    container_id: &str,
) -> Result<Vec<ListedFile>, AppError> {
    require_file_reader(&ws.app, &ws.caller.user_id, container_id).await?;
    ws.app.files_of_container(container_id).await
}

/// The files of a card
///
/// The files the card shows, for the members of its board: those its body names and
/// those attached to it, each once, oldest first.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/boards/{boardId}/cards/{cardId}/files",
    operation_id = "listCardFiles",
    tag = tag::FILES,
    params(MemberWorkspaceId, BoardId, CardId),
    responses((status = 200, description = "The files.", body = Vec<ListedFile>)),
)]
async fn handle_list_card_files(
    Delegated(ws, ..): Delegated<Read, InWorkspace>,
    Path((_, board_id, card_id)): Path<(String, String, String)>,
) -> ApiResult {
    Ok(json_response(
        &files_of_card(&ws, &board_id, &card_id).await?,
    ))
}

/// The files the card `card_id` of the board `board_id` shows, as `listCardFiles` lists
/// them.
///
/// # Errors
///
/// As `listCardFiles` answers.
pub(super) async fn files_of_card(
    ws: &InWorkspace,
    board_id: &str,
    card_id: &str,
) -> Result<Vec<ListedFile>, AppError> {
    let card = card_with_permission(
        &ws.app,
        &ws.caller.user_id,
        card_id,
        ContainerPermission::View,
        "access denied to fetch card",
    )
    .await?;
    if card.container_id != board_id {
        return Err(AppError::not_found("card"));
    }
    let mut named: BTreeSet<String> = card
        .body
        .as_ref()
        .map_or_default(|body| body.file_ids().into_iter().map(str::to_owned).collect());
    let attachments = ws
        .app
        .blocks(board_id, Some(&card.id), Some(&BlockType::Attachment))
        .await?;
    named.extend(attachments.iter().filter_map(|attachment| {
        attachment
            .fields
            .get("fileId")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    }));
    let mut files = Vec::with_capacity(named.len());
    for file_id in named {
        if let Some(record) = ws.app.file_record(board_id, &file_id).await? {
            files.push(ListedFile::from(record));
        }
    }
    files.sort_by(|a, b| (a.created_at, &a.file_id).cmp(&(b.created_at, &b.file_id)));
    Ok(files)
}
