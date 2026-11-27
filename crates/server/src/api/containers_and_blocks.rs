//! Boards and blocks created, patched and deleted together in one request.

use std::collections::{HashMap, HashSet};

use axum::body::Bytes;
use fvoci_model::{
    Block, Container, ContainersAndBlocks, DeleteContainersAndBlocks, PatchContainersAndBlocks,
    generate_containers_and_blocks_ids,
};

use serde_json::Value;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::contract::bodies::NewContainersAndBlocks;
use super::contract::params::MemberWorkspaceId;
use super::contract::tag;
use super::workspace::InWorkspace;

use super::blocks::{RawNewBlock, check_patch_block_types};
use super::containers::{BoardObject, check_listed_boards};
use super::{ApiResult, json_response, no_content};
use super::{BlockAction, require_block_patch_permission, require_block_permission};
use super::{decode_json, json_object};
use super::{decode_object, require_granted};
use super::{element_pointer, objects_mut, put_patch_bodies, take_block_bodies, take_patch_bodies};
use crate::app::{App, ContainerAccess};
use crate::error::AppError;
use crate::permissions::{ContainerPermission, Grant};

/// The JSON Pointers to the blocks of a `ContainersAndBlocks` and to the block patches of a
/// `PatchContainersAndBlocks`.
const BLOCKS: &str = "/blocks";
const PATCHES: &str = "/blockPatches";

/// The refusal of a change to a block of a board the user may not change cards on.
const MODIFYING_CARDS_DENIED: &str = "access denied to modifying cards";

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new().routes(routes!(
        handle_create_containers_and_blocks,
        handle_patch_containers_and_blocks,
        handle_delete_containers_and_blocks
    ))
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
struct RawContainersAndBlocks {
    containers: Vec<Container>,
    blocks: Vec<RawNewBlock>,
}

/// Make boards with their blocks
///
/// Makes boards and their blocks together, all or none, under identifiers the server
/// makes; the identifiers of the request only say which block belongs to which board
/// and to which other block. The request needs at least one board and one block; every
/// board needs an `id`, and all boards are of one workspace. The user becomes the admin of
/// each board, and each board that is no template is filed under the user's default
/// category. They are boards: one whose `kind` is not `board` is 400 at its `kind`.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/containers-and-blocks",
    params(MemberWorkspaceId),
    operation_id = "createContainersAndBlocks",
    tag = tag::CONTAINERS,
    request_body = NewContainersAndBlocks,
    responses((
        status = 201,
        description = "The boards in the order of the request, and the blocks board by board in that order, each board's in the order of the request.",
        body = ContainersAndBlocks,
    )),
)]
async fn handle_create_containers_and_blocks(ws: InWorkspace, body: Bytes) -> ApiResult {
    let user_id = &ws.caller.user_id;
    // Whoever works in the workspace makes boards in it.
    ws.require_container_maker("access denied to create board")?;
    let mut object = json_object(decode_json(&body)?)?;
    // Each board's type and minimum role are refused as `createBoard` refuses them.
    check_listed_boards(&object, "containers", BoardObject::Board)?;
    let bodies = take_block_bodies(objects_mut(&mut object, "blocks"), Some(BLOCKS));
    let mut raw: RawContainersAndBlocks =
        serde_json::from_value(Value::Object(object)).map_err(AppError::bad_request)?;
    // The boards are new and have no files, so a body that names a file is refused, without
    // looking up anything the request names.
    for (index, (block, body)) in raw.blocks.iter_mut().zip(bodies?).enumerate() {
        if body
            .as_ref()
            .is_some_and(|body| !body.file_ids().is_empty())
        {
            return Err(AppError::bad_request(
                "an image or a file block of the body names no file of the board: a new board has none",
            )
            .with_field("body")
            .under(&element_pointer(Some(BLOCKS), index)));
        }
        block.set_body_unchecked(body);
    }

    if raw.containers.is_empty() {
        return Err(AppError::bad_request("at least one board is required"));
    }

    for (index, container) in raw.containers.iter_mut().enumerate() {
        ws.place_container(container, "workspaceId")
            .map_err(|error| error.under(&element_pointer(Some("/containers"), index)))?;
        if container.id.is_empty() {
            return Err(AppError::bad_request(
                "boards need an ID to be referenced from the blocks",
            ));
        }
    }
    let container_ids: HashSet<&str> = raw
        .containers
        .iter()
        .map(|container| container.id.as_str())
        .collect();

    let mut blocks = Vec::with_capacity(raw.blocks.len());
    for (index, raw_block) in raw.blocks.into_iter().enumerate() {
        let on_board = |container_id: &str| container_ids.contains(container_id);
        let block = raw_block
            .into_block(on_board, "the block is on no board of the request")
            .map_err(|(field, problem)| {
                AppError::bad_request(problem)
                    .with_field(field)
                    .under(&element_pointer(Some(BLOCKS), index))
            })?;
        blocks.push(block);
    }

    let request = generate_containers_and_blocks_ids(ContainersAndBlocks {
        containers: raw.containers,
        blocks,
    })
    .map_err(AppError::bad_request)?;
    // The store names a block it refuses by its identifier.
    let ids: Vec<String> = request
        .blocks
        .iter()
        .map(|block| block.id.clone())
        .collect();
    let created = ws
        .app
        .create_containers_and_blocks_with_admin(request, user_id)
        .await
        .map_err(|error| error.among_blocks(ids.iter().map(String::as_str), BLOCKS))?;
    Ok(super::created(&created))
}

/// Change boards and blocks together
///
/// Applies a patch to each of several boards of one workspace and to blocks of those
/// boards, all or none. The request names at least one board, and every block it names
/// is on one of its boards: blocks alone are not changed this way. Each board needs
/// what `patchContainer` needs, each block what `patchBlock` needs; a board's patch that
/// names a `kind` is 400 at `/containerPatches/{index}/kind`.
#[utoipa::path(
    patch,
    path = "/w/{workspaceId}/containers-and-blocks",
    params(MemberWorkspaceId),
    operation_id = "patchContainersAndBlocks",
    tag = tag::CONTAINERS,
    request_body = PatchContainersAndBlocks,
    responses((
        status = 200,
        description = "The boards and the blocks as they are now, each in the order of the request.",
        body = ContainersAndBlocks,
    )),
)]
async fn handle_patch_containers_and_blocks(ws: InWorkspace, body: Bytes) -> ApiResult {
    let user_id = &ws.caller.user_id;
    let mut object = json_object(decode_json(&body)?)?;
    // Each patch's type and minimum role are refused as `patchContainer` refuses them.
    check_listed_boards(&object, "containerPatches", BoardObject::Patch)?;
    check_patch_block_types(objects_mut(&mut object, "blockPatches"), Some(PATCHES))?;
    let names_a_kind: Vec<bool> = objects_mut(&mut object, "containerPatches")
        .map(|patch| patch.is_some_and(|patch| patch.contains_key("kind")))
        .collect();
    let changes = take_patch_bodies(objects_mut(&mut object, "blockPatches"), Some(PATCHES));
    let mut patches: PatchContainersAndBlocks =
        serde_json::from_value(Value::Object(object)).map_err(AppError::bad_request)?;
    patches.validate().map_err(AppError::bad_request)?;
    put_patch_bodies(&mut patches.block_patches, changes?, Some(PATCHES))?;

    let mut workspace_id = String::new();
    let mut container_ids = HashSet::new();
    let mut access_by_container: HashMap<&str, ContainerAccess> = HashMap::new();
    for (index, (container_id, patch)) in patches
        .container_ids
        .iter()
        .zip(&patches.container_patches)
        .enumerate()
    {
        container_ids.insert(container_id.as_str());

        let names_a_kind = names_a_kind.get(index).copied().unwrap_or_default();
        let access = if let Some(found) = access_by_container.get(container_id.as_str()) {
            found
        } else {
            let loaded = ws.app.container_access(user_id, container_id).await?;
            let grant = loaded.as_ref().map_or(Grant::NONE, |found| found.grant);
            require_granted(
                grant,
                ContainerPermission::ManageProperties,
                "access denied to modifying board properties",
            )?;
            let found = loaded.ok_or_else(|| AppError::not_found("board"))?;
            access_by_container
                .entry(container_id.as_str())
                .or_insert(found)
        };
        require_granted(
            access.grant,
            ContainerPermission::ManageProperties,
            "access denied to modifying board properties",
        )?;
        // As `patchContainer`: the kind of a board never changes.
        if names_a_kind {
            return Err(AppError::bad_request("the kind of a board never changes")
                .with_pointer(format!("/containerPatches/{index}/kind")));
        }
        if patch.visibility.is_some() || patch.minimum_role.is_some() {
            require_granted(
                access.grant,
                ContainerPermission::ManageType,
                "access denied to modifying board type",
            )?;
        }

        if workspace_id.is_empty() {
            workspace_id.clone_from(&access.container.workspace_id);
        }
        if workspace_id != access.container.workspace_id {
            return Err(AppError::bad_request("mismatched workspace ID"));
        }
    }

    let blocks_by_id =
        load_blocks_of_containers(&ws.app, &patches.block_ids, &container_ids).await?;
    require_block_patches(&ws.app, user_id, &patches, &blocks_by_id).await?;
    let block_patches = patches.block_ids.iter().zip(&patches.block_patches);
    for (index, (block_id, patch)) in block_patches.enumerate() {
        if let Some(body) = patch.body.new_body() {
            let block = blocks_by_id
                .get(block_id.as_str())
                .ok_or_else(not_a_block_of_the_containers)?;
            ws.app
                .check_body_files(&block.container_id, Some(body))
                .await
                .map_err(|error| error.under(&element_pointer(Some(PATCHES), index)))?;
        }
    }

    let result = ws
        .app
        .patch_containers_and_blocks(&patches, user_id)
        .await
        .map_err(|error| {
            error
                .moved(super::blocks::BLOCK_FIELDS, super::blocks::PATCH_FIELDS)
                .among_blocks(patches.block_ids.iter().map(String::as_str), PATCHES)
        })?;
    Ok(json_response(&result))
}

/// The one answer for a block that does not exist and for one of a board the request
/// does not list.
fn not_a_block_of_the_containers() -> AppError {
    AppError::bad_request("the request names a block that is not on its boards")
}

/// Loads every block in `block_ids` in a single store query and verifies that each belongs
/// to `container_ids`. Missing, non-identifier, or off-container IDs all return the same
/// [`not_a_block_of_the_containers`] refusal so nothing leaks about other boards (`1.2.2`).
async fn load_blocks_of_containers(
    app: &App,
    block_ids: &[String],
    container_ids: &HashSet<&str>,
) -> Result<HashMap<String, Block>, AppError> {
    if block_ids.is_empty() {
        return Ok(HashMap::new());
    }
    if !block_ids.iter().all(|id| fvoci_model::is_id(id)) {
        return Err(not_a_block_of_the_containers());
    }
    let Some(blocks) = app.blocks_by_ids(block_ids).await? else {
        return Err(not_a_block_of_the_containers());
    };
    let mut by_id = HashMap::with_capacity(blocks.len());
    for block in blocks {
        if !container_ids.contains(block.container_id.as_str()) {
            return Err(not_a_block_of_the_containers());
        }
        by_id.insert(block.id.clone(), block);
    }
    if block_ids.iter().any(|id| !by_id.contains_key(id.as_str())) {
        return Err(not_a_block_of_the_containers());
    }
    Ok(by_id)
}

/// Checks that every block is on one of the boards, and that the user may delete it as
/// `deleteBlock` would. The same answer for a block that does not exist and for one on
/// a board the request does not list, so that nothing is said about blocks of other
/// boards.
async fn require_blocks_to_delete(
    app: &App,
    user_id: &str,
    block_ids: &[String],
    container_ids: &HashSet<&str>,
) -> Result<(), AppError> {
    let blocks_by_id = load_blocks_of_containers(app, block_ids, container_ids).await?;
    for block_id in block_ids {
        let block = blocks_by_id
            .get(block_id.as_str())
            .ok_or_else(not_a_block_of_the_containers)?;
        super::require_delete_permission(app, user_id, block, MODIFYING_CARDS_DENIED).await?;
    }
    Ok(())
}

/// Checks that every block the request patches is on one of its boards, and that the
/// user may apply its patch to it as `patchBlock` would.
async fn require_block_patches(
    app: &App,
    user_id: &str,
    patches: &PatchContainersAndBlocks,
    blocks_by_id: &HashMap<String, Block>,
) -> Result<(), AppError> {
    for (block_id, patch) in patches.block_ids.iter().zip(&patches.block_patches) {
        let block = blocks_by_id
            .get(block_id.as_str())
            .ok_or_else(not_a_block_of_the_containers)?;
        require_block_permission(
            app,
            user_id,
            block,
            BlockAction::Change,
            MODIFYING_CARDS_DENIED,
        )
        .await?;
        require_block_patch_permission(app, user_id, block, patch, MODIFYING_CARDS_DENIED).await?;
    }
    Ok(())
}

/// Delete boards and blocks together
///
/// Deletes several boards of one workspace, all or none. The request names at least one
/// board, and every block it names must be on one of its boards and be the user's to
/// delete as `deleteBlock` has it; the blocks are checked and nothing more, since a
/// board takes all of its blocks along. The boards go to the trash as `deleteContainer`
/// puts them there, and `restoreContainer` brings each back with its blocks. For the
/// admins of every board named.
#[utoipa::path(
    delete,
    path = "/w/{workspaceId}/containers-and-blocks",
    params(MemberWorkspaceId),
    operation_id = "deleteContainersAndBlocks",
    tag = tag::CONTAINERS,
    request_body = DeleteContainersAndBlocks,
    responses((status = 204, description = "The boards and blocks are deleted.")),
)]
async fn handle_delete_containers_and_blocks(ws: InWorkspace, body: Bytes) -> ApiResult {
    let user_id = &ws.caller.user_id;
    let request: DeleteContainersAndBlocks = decode_object(&body)?;
    request.validate().map_err(AppError::bad_request)?;

    let mut workspace_id = String::new();
    let mut container_ids = HashSet::new();
    for container_id in &request.containers {
        container_ids.insert(container_id.as_str());
        let loaded = ws.app.container_access(user_id, container_id).await?;
        let grant = loaded.as_ref().map_or(Grant::NONE, |found| found.grant);
        require_granted(
            grant,
            ContainerPermission::Delete,
            "access denied to delete board",
        )?;
        let access = loaded.ok_or_else(|| AppError::not_found("board"))?;
        if workspace_id.is_empty() {
            workspace_id.clone_from(&access.container.workspace_id);
        }
        if workspace_id != access.container.workspace_id {
            return Err(AppError::bad_request(
                "all boards should belong to the same workspace",
            ));
        }
    }

    require_blocks_to_delete(&ws.app, user_id, &request.blocks, &container_ids).await?;

    ws.app
        .delete_containers_and_blocks(&request, user_id)
        .await?;
    Ok(no_content())
}
