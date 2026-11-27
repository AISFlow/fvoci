//! Blocks: reading, inserting, patching, deleting, restoring and duplicating.

use axum::body::Bytes;
use axum::extract::Path;
use fvoci_model::{
    Block, BlockError, BlockPatch, BlockPatchBatch, BlockType, Body, CommentVersion, Container,
    JsonMap, RelationProperties, UnmatchedRelation, renew_block_ids, stamp_modification_metadata,
};
use serde::Deserialize;
use serde_json::Value;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::contract::bodies::NewBlock;
use super::contract::params::MemberWorkspaceId;
use super::contract::params::{
    AllBlocks, AsTemplate, BlockId, BlockTypeFilter, BoardId, DisableNotify, OneBlockId, ParentId,
    SourceBoardId,
};
use super::contract::tag;
use super::query::{AsTemplateQuery, DisableNotifyQuery, QueryParams, non_empty_opt};
use super::workspace::InWorkspace;
use super::{Delegated, Read};

use super::{ApiResult, TRUE, created, decode_json, json_response, no_content};
use super::{
    BlockAction, require_block_patch_permission_with_grant, require_block_permission_with_grant,
    require_container_permission, require_delete_permission, require_deletion_permission,
    require_granted,
};
use super::{
    decode_object_with, element_pointer, objects_mut, put_patch_bodies, take_block_bodies,
    take_patch_bodies,
};
use crate::app::{App, ChangeNotification};
use crate::auth::now_millis;
use crate::error::AppError;
use crate::permissions::ContainerPermission;

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct BlocksFilterQuery {
    pub(super) all: String,
    pub(super) block_id: String,
    pub(super) parent_id: String,
    #[serde(rename = "type")]
    pub(super) block_type: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
struct CreateBlocksQuery {
    source_board_id: String,
    disable_notify: String,
}

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new()
        .routes(routes!(
            handle_get_blocks,
            handle_post_blocks,
            handle_patch_blocks
        ))
        .routes(routes!(handle_delete_block, handle_patch_block))
        .routes(routes!(handle_restore_block))
        .routes(routes!(handle_get_comment_versions))
        .routes(routes!(handle_duplicate_block))
}

/// The refusal to change what is on a board.
const CHANGES_DENIED: &str = "access denied to make board changes";

/// The JSON Pointers to the fields of a block and to the member of a patch that changes
/// them.
pub(super) const BLOCK_FIELDS: &str = "/fields";
pub(super) const PATCH_FIELDS: &str = "/updatedFields";

/// The JSON Pointer to the patches of a `BlockPatchBatch`.
const PATCHES: &str = "/blockPatches";

/// Returns the block if it belongs to the board. A block of another board is as missing
/// as one that does not exist: the path names the board the request is about.
async fn block_on_container(
    app: &App,
    block_id: &str,
    container_id: &str,
) -> Result<Block, AppError> {
    let block = app.block(block_id).await?;
    if block.container_id == container_id {
        Ok(block)
    } else {
        Err(AppError::not_found("block"))
    }
}

/// Checks that a signed-in user may read the blocks of a board: the members of the
/// board, and for an open or built-in template everyone who works in its workspace
/// besides.
async fn require_read_access(ws: &InWorkspace, container: &Container) -> Result<(), AppError> {
    let (app, user_id) = (&ws.app, ws.caller.user_id.as_str());
    require_container_permission(
        app,
        user_id,
        &container.id,
        ContainerPermission::View,
        "access denied to board",
    )
    .await
}

/// Checks that the user may read the board that inserted blocks copy their files from,
/// so a caller cannot copy files out of an unreadable board. Whoever may read the
/// board's blocks may copy from it.
async fn require_source_board_access(
    ws: &InWorkspace,
    source_board_id: &str,
) -> Result<(), AppError> {
    // The same answer for a board that is not there and one the user may not read. A
    // store that failed is neither: it is answered as what it is.
    let refused = |error: AppError| {
        if error.is_refusal() {
            AppError::not_found("source board")
        } else {
            error
        }
    };
    let board = ws.app.container(source_board_id).await.map_err(refused)?;
    require_read_access(ws, &board).await.map_err(refused)
}

/// The blocks of a board
///
/// For the board's members, and for an open or built-in template everyone who works in
/// its workspace.
///
/// `all` asks for every block and `blockId` for one, which is 404 if the board has no
/// such block. Otherwise `parentId` and `type` narrow the list, and without either the
/// list is every block of the board. A parameter with an empty value counts as left
/// out. Oldest first; the blocks that one request made come in the order that request
/// had them in.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/boards/{boardId}/blocks",
    operation_id = "getBlocks",
    tag = tag::BLOCKS,
    params(MemberWorkspaceId, BoardId, AllBlocks, OneBlockId, ParentId, BlockTypeFilter),
    responses((status = 200, description = "The blocks.", body = Vec<Block>)),
)]
async fn handle_get_blocks(
    Delegated(ws, ..): Delegated<Read, InWorkspace>,
    Path((_, board_id)): Path<(String, String)>,
    QueryParams(query): QueryParams<BlocksFilterQuery>,
) -> ApiResult {
    let board = ws.app.container(&board_id).await?;
    require_read_access(&ws, &board).await?;
    let blocks = asked_blocks(&ws.app, &board_id, &query).await?;
    tracing::debug!(board_id, block_count = blocks.len(), "GetBlocks");
    Ok(json_response(&blocks))
}

/// The blocks of a board that the query of a request asks for, as `getBlocks` describes
/// them; for whoever may read the board.
pub(super) async fn asked_blocks(
    app: &App,
    container_id: &str,
    query: &BlocksFilterQuery,
) -> Result<Vec<Block>, AppError> {
    if !query.all.is_empty() {
        app.blocks_for_container(container_id).await
    } else if let Some(block_id) = non_empty_opt(&query.block_id) {
        Ok(vec![
            block_on_container(app, &block_id, container_id).await?,
        ])
    } else {
        let parent_id = non_empty_opt(&query.parent_id);
        let block_type = match non_empty_opt(&query.block_type) {
            None => None,
            Some(raw) => match raw.parse::<BlockType>() {
                Ok(parsed) => Some(parsed),
                Err(_) => return Ok(Vec::new()),
            },
        };
        app.blocks(container_id, parent_id.as_deref(), block_type.as_ref())
            .await
    }
}

/// Checks that the images and file blocks of the blocks' bodies name files of the
/// board, or of the board named by `sourceBoardId`, and copies the files the blocks
/// show from that board.
async fn check_and_copy_body_files(
    ws: &InWorkspace,
    board_id: &str,
    source_board_id: &str,
    blocks: &mut [Block],
) -> Result<(), AppError> {
    let (app, user_id) = (&ws.app, ws.caller.user_id.as_str());
    if !source_board_id.is_empty()
        && blocks
            .iter()
            .all(|block| block.block_type == BlockType::Comment)
        && blocks.iter().any(|block| !block.shown_files().is_empty())
    {
        require_container_permission(
            app,
            user_id,
            board_id,
            ContainerPermission::ManageCards,
            CHANGES_DENIED,
        )
        .await?;
    }
    let files_board_id = if source_board_id.is_empty() {
        board_id
    } else {
        require_source_board_access(ws, source_board_id).await?;
        source_board_id
    };
    for (index, block) in blocks.iter().enumerate() {
        app.check_body_files(files_board_id, block.body.as_ref())
            .await
            .map_err(|error| error.under(&element_pointer(Some(""), index)))?;
    }
    if !source_board_id.is_empty() {
        app.copy_card_files_into(source_board_id, blocks).await;
    }

    Ok(())
}

/// Gives each new block the body the request had for it. A body its type may not have
/// ([`BlockType::check_body`]) is refused here, before anything is copied for it, at the
/// block's place in the request.
fn give_bodies(blocks: &mut [RawNewBlock], bodies: Vec<Option<Body>>) -> Result<(), AppError> {
    for (index, (block, body)) in blocks.iter_mut().zip(bodies).enumerate() {
        block
            .set_body_checked(body)
            .map_err(|refused| AppError::from(refused).under(&element_pointer(Some(""), index)))?;
    }
    Ok(())
}

/// Insert blocks
///
/// Inserts blocks under identifiers the server makes; the identifiers of the request
/// only link its blocks to each other. All blocks are inserted or none, and they are
/// listed afterwards in the order the request had them in. Comments need the role of a
/// commenter, everything else that of an editor. A comment is on a card of the board,
/// and no block is below a comment (400 at the block's `parentId`). A comment's body
/// has no file block (400 at its `body`), and nothing is copied for one that has.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/boards/{boardId}/blocks",
    operation_id = "createBlocks",
    tag = tag::BLOCKS,
    params(MemberWorkspaceId, BoardId, SourceBoardId, DisableNotify),
    request_body = Vec<NewBlock>,
    responses((
        status = 201,
        description = "The blocks as they were stored, in the order of the request.",
        body = Vec<Block>,
    )),
)]
async fn handle_post_blocks(
    ws: InWorkspace,
    Path((_, board_id)): Path<(String, String)>,
    QueryParams(query): QueryParams<CreateBlocksQuery>,
    body: Bytes,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    let notification = if query.source_board_id.is_empty() {
        if query.disable_notify == TRUE {
            ChangeNotification::Suppress
        } else {
            ChangeNotification::Send
        }
    } else {
        ChangeNotification::Copied
    };
    require_container_permission(
        &ws.app,
        user_id,
        &board_id,
        ContainerPermission::View,
        CHANGES_DENIED,
    )
    .await?;
    let mut raw_values: Vec<Value> = decode_json(&body)?;
    let bodies = take_block_bodies(raw_values.iter_mut().map(Value::as_object_mut), Some(""))?;
    let mut raw_blocks: Vec<RawNewBlock> =
        serde_json::from_value(Value::Array(raw_values)).map_err(AppError::bad_request)?;
    give_bodies(&mut raw_blocks, bodies)?;
    let place = |index: usize| element_pointer(Some(""), index);

    let mut has_comments = false;
    let mut has_contents = false;
    let mut blocks = Vec::with_capacity(raw_blocks.len());
    for (index, raw_block) in raw_blocks.into_iter().enumerate() {
        let on_board = |container_id: &str| container_id == board_id;
        let block = raw_block
            .into_block(on_board, "the block is on another board than the path")
            .map_err(|(member, problem)| {
                AppError::bad_request(problem)
                    .with_field(member)
                    .under(&place(index))
            })?;
        if block.block_type == BlockType::Card {
            return Err(AppError::from(BlockError::CardByBlocks).under(&place(index)));
        }
        if block.block_type == BlockType::Comment {
            has_comments = true;
        } else {
            has_contents = true;
        }
        blocks.push(block);
    }
    if has_contents {
        require_container_permission(
            &ws.app,
            user_id,
            &board_id,
            ContainerPermission::ManageCards,
            CHANGES_DENIED,
        )
        .await?;
    }
    if has_comments {
        require_container_permission(
            &ws.app,
            user_id,
            &board_id,
            ContainerPermission::CommentCards,
            "access denied to post card comments",
        )
        .await?;
    }

    renew_request_ids(&ws.app, &board_id, &mut blocks).await?;
    stamp_modification_metadata(user_id, &mut blocks, now_millis());

    check_and_copy_body_files(&ws, &board_id, &query.source_board_id, &mut blocks).await?;

    // The store names a block it refuses by its identifier.
    let ids: Vec<String> = blocks.iter().map(|block| block.id.clone()).collect();
    ws.app
        .insert_blocks(&mut blocks, user_id, notification)
        .await
        .map_err(|error| error.among_blocks(ids.iter().map(String::as_str), ""))?;
    tracing::debug!(block_count = blocks.len(), "POST Blocks");
    Ok(created(&blocks))
}

/// How the `type` member of a new block object was written in the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum RawBlockType {
    /// Left out or empty (`""`).
    #[default]
    Missing,
    /// A non-empty string outside [`BlockType`].
    Unknown,
    /// A known [`BlockType`].
    Known(BlockType),
}

fn deserialize_new_block_type<'de, D>(deserializer: D) -> Result<RawBlockType, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize as _;
    let raw = String::deserialize(deserializer)?;
    if raw.is_empty() {
        return Ok(RawBlockType::Missing);
    }
    Ok(raw
        .parse::<BlockType>()
        .map_or(RawBlockType::Unknown, RawBlockType::Known))
}

fn non_null_i64<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize as _;
    i64::deserialize(deserializer).map(Some)
}

fn number_as_sent<'de, D>(deserializer: D) -> Result<Option<u32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize as _;
    let sent = Value::deserialize(deserializer)?;
    Ok(sent
        .as_u64()
        .and_then(|number| u32::try_from(number).ok())
        .filter(|number| *number > 0))
}

/// Request representation of a newly created block before its board, `type`, and
/// timestamps are validated: a missing or unknown `type` cannot become a [`Block`].
#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RawNewBlock {
    #[serde(default)]
    id: String,
    #[serde(default)]
    parent_id: String,
    #[serde(default)]
    created_by: String,
    #[serde(default)]
    modified_by: String,
    #[serde(
        default,
        rename = "type",
        deserialize_with = "deserialize_new_block_type"
    )]
    raw_type: RawBlockType,
    #[serde(default)]
    title: String,
    #[serde(default)]
    fields: JsonMap,
    #[serde(skip)]
    body: Option<Body>,
    #[serde(default)]
    created_at: i64,
    #[serde(default)]
    updated_at: i64,
    #[serde(default, deserialize_with = "non_null_i64")]
    deleted_at: Option<i64>,
    #[serde(default)]
    container_id: String,
    #[serde(default, deserialize_with = "number_as_sent")]
    number: Option<u32>,
    #[serde(default)]
    edited_at: i64,
}

impl RawNewBlock {
    /// Checks `body` against the requested block type and attaches it.
    fn set_body_checked(&mut self, body: Option<Body>) -> Result<(), BlockError> {
        match self.raw_type {
            RawBlockType::Known(bt) => bt.check_body(body.as_ref())?,
            RawBlockType::Missing | RawBlockType::Unknown if body.is_some() => {
                return Err(BlockError::BodyNotAllowed);
            }
            RawBlockType::Missing | RawBlockType::Unknown => {}
        }
        self.body = body;
        Ok(())
    }

    /// Attaches an already extracted `body` without `check_body` (for routes where the
    /// store runs body validation after container checks).
    pub(super) fn set_body_unchecked(&mut self, body: Option<Body>) {
        self.body = body;
    }

    /// Validates the block's board (`on_board` / `elsewhere`), its `type`, and its
    /// timestamps in contract order, producing a [`Block`] only when all pass.
    pub(super) fn into_block(
        self,
        on_board: impl Fn(&str) -> bool,
        elsewhere: &'static str,
    ) -> Result<Block, (&'static str, String)> {
        if !on_board(&self.container_id) {
            return Err(("containerId", elsewhere.to_owned()));
        }
        let block_type = match self.raw_type {
            RawBlockType::Missing => return Err(("type", "the block has no type".to_owned())),
            RawBlockType::Unknown => return Err(("type", BlockError::TypeNotCurrent.to_string())),
            RawBlockType::Known(bt) if !bt.is_current() => {
                return Err(("type", BlockError::type_not_current(&bt).to_string()));
            }
            RawBlockType::Known(bt) => bt,
        };
        Ok(Block {
            id: self.id,
            parent_id: self.parent_id,
            created_by: self.created_by,
            modified_by: self.modified_by,
            block_type,
            title: self.title,
            fields: self.fields,
            body: self.body,
            created_at: self.created_at,
            updated_at: self.updated_at,
            deleted_at: self.deleted_at,
            container_id: self.container_id,
            number: self.number,
            edited_at: self.edited_at,
        })
    }
}

/// Refuses any block patch that attempts to change `type` (blocks keep their creation
/// type for life), naming `type` at the patch's JSON Pointer.
pub(super) fn check_patch_block_types<'a>(
    patches: impl IntoIterator<Item = Option<&'a mut JsonMap>>,
    list: Option<&str>,
) -> Result<(), AppError> {
    for (index, patch) in patches.into_iter().enumerate() {
        let Some(patch) = patch else { continue };
        if patch.remove("type").is_some() {
            return Err(AppError::bad_request(BlockError::TypeChanges)
                .with_field("type")
                .under(&element_pointer(list, index)));
        }
    }
    Ok(())
}

/// Gives the blocks of a request to a board new identifiers ([`renew_block_ids`]): a
/// card relation value names the request's cards by their new identifiers, and keeps
/// what else it names, for the store to check.
async fn renew_request_ids(
    app: &App,
    container_id: &str,
    blocks: &mut [Block],
) -> Result<(), AppError> {
    let relations = if blocks
        .iter()
        .any(|block| block.block_type == BlockType::Card)
    {
        RelationProperties::of_board(
            container_id,
            &app.container(container_id).await?.card_properties,
        )
    } else {
        RelationProperties::new()
    };
    renew_block_ids(blocks, &relations, UnmatchedRelation::Kept);
    Ok(())
}

/// Delete a block
///
/// The block goes to the trash with the blocks below it, and `restoreBlock` brings it
/// back until the trash is emptied of it. For editors; a comment is its author's to
/// delete from the commenter's role up, and a comment of somebody else the board's
/// admins' only (403, `access denied to delete other users' comments`), also as a
/// reply below a comment the user deletes.
#[utoipa::path(
    delete,
    path = "/w/{workspaceId}/boards/{boardId}/blocks/{blockId}",
    operation_id = "deleteBlock",
    tag = tag::BLOCKS,
    params(MemberWorkspaceId, BoardId, BlockId, DisableNotify),
    responses((status = 204, description = "The block is deleted.")),
)]
async fn handle_delete_block(
    ws: InWorkspace,
    Path((_, board_id, block_id)): Path<(String, String, String)>,
    QueryParams(query): QueryParams<DisableNotifyQuery>,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    let notification = query.notification();
    // Nobody below the commenter's role deletes anything, whatever the block is.
    require_container_permission(
        &ws.app,
        user_id,
        &board_id,
        ContainerPermission::CommentCards,
        CHANGES_DENIED,
    )
    .await?;
    let block = block_on_container(&ws.app, &block_id, &board_id).await?;
    require_delete_permission(&ws.app, user_id, &block, CHANGES_DENIED).await?;
    ws.app
        .delete_block(&block_id, user_id, notification)
        .await?;
    tracing::debug!(board_id, block_id, "DELETE Block");
    Ok(no_content())
}

/// Bring a deleted block back
///
/// Restores a block of the board from the trash, with the blocks that went there with
/// it. A block whose parent is gone has nowhere to come back to: 404; one whose parent
/// may no longer hold it (a comment off a card, a block below a comment): 409. For who may
/// delete what comes back, as `deleteBlock` has it: editors, a comment its author from
/// the commenter's role up if they deleted it themselves, and a comment of somebody
/// else the board's admins only (403, `access denied to undelete other users' comments`),
/// as one of the user's that somebody else deleted (403, `a comment somebody else
/// deleted is brought back by the board's admins`). What decides is what was
/// deleted, as `deleteBlock` decided it: a comment with every reply that went with it,
/// so a block below somebody else's comment is brought back by the admins only, and a
/// card alone, so a comment that went with its card comes back for whoever may bring
/// back the card.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/boards/{boardId}/blocks/{blockId}/restore",
    operation_id = "restoreBlock",
    tag = tag::BLOCKS,
    params(MemberWorkspaceId, BoardId, BlockId),
    responses((status = 200, description = "The block as it is again.", body = Block)),
)]
async fn handle_restore_block(
    ws: InWorkspace,
    Path((_, board_id, block_id)): Path<(String, String, String)>,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    // Nobody below the commenter's role brings anything back, whatever the block is.
    require_container_permission(
        &ws.app,
        user_id,
        &board_id,
        ContainerPermission::CommentCards,
        CHANGES_DENIED,
    )
    .await?;
    let board = ws.app.container(&board_id).await?;
    // What is restored is what the trash has of this board: the whole deletion the
    // block went with, all of one board. A block that exists, one that never did and
    // one of another board are all not there to restore.
    let deletion = ws.app.deletion(&block_id).await?;
    let Some((top, below)) = deletion
        .split_first()
        .filter(|(top, _)| top.container_id == board.id)
    else {
        return Err(AppError::not_found("block"));
    };
    // Only who may delete it brings it back, as it was deleted: from the block at the
    // top (a block below somebody else's comment brings that comment back). A comment
    // somebody else deleted is not its author's to bring back, but the board's admins'.
    let deleted_by = ws.app.deleted_by(&top.id).await?;
    require_deletion_permission(
        &ws.app,
        user_id,
        top,
        below,
        BlockAction::Undelete {
            deleted_by: &deleted_by,
        },
        CHANGES_DENIED,
    )
    .await?;

    let undeleted = ws.app.undelete_block(&block_id, user_id).await?;
    tracing::debug!(block_id, "UNDELETE Block");
    Ok(json_response(&undeleted))
}

/// What a comment said before
///
/// The earlier versions of a comment's body: every version but the one it has now, with
/// who wrote it and when. The first one first; it is the text the comment was posted
/// with, by its author. The server keeps the first version and the newest ones, up to
/// its setting `max_comment_versions` in all, so the numbers of the versions may leave
/// gaps. Empty for a comment whose body was never changed; the comment's `editedAt` says
/// whether it was. For the board's members, also in the trash: the versions go only
/// when the comment is gone for good. Not for a reader by the board's read token. A
/// block that is no comment is not there (404).
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/boards/{boardId}/blocks/{blockId}/versions",
    operation_id = "getCommentVersions",
    tag = tag::BLOCKS,
    params(MemberWorkspaceId, BoardId, BlockId),
    responses((status = 200, description = "The earlier versions, the first one first.", body = Vec<CommentVersion>)),
)]
async fn handle_get_comment_versions(
    Delegated(ws, ..): Delegated<Read, InWorkspace>,
    Path((_, board_id, block_id)): Path<(String, String, String)>,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    require_container_permission(
        &ws.app,
        user_id,
        &board_id,
        ContainerPermission::View,
        "access denied to board",
    )
    .await?;
    let comment = match block_on_container(&ws.app, &block_id, &board_id).await {
        Ok(block) => block,
        Err(error) if error.is_not_found() => ws
            .app
            .deleted_block(&block_id)
            .await?
            .map(|deleted| deleted.block)
            .filter(|block| block.container_id == board_id)
            .ok_or_else(|| AppError::not_found("comment"))?,
        Err(error) => return Err(error),
    };
    if comment.block_type != BlockType::Comment {
        return Err(AppError::not_found("comment"));
    }
    let versions = ws.app.comment_versions(&block_id).await?;
    Ok(json_response(&versions))
}

/// Change a block
///
/// For editors. A comment is changed by its author with the commenter's role, and the
/// comment of somebody else by the board's admins only (as `deleteBlock` has it): an
/// editor may not change it. The commenter's role changes a comment's text (its body);
/// moving a comment to another card (`parentId`) or making it a block of another type
/// is for editors, and making a block of somebody else a comment for the board's admins.
/// A comment goes only on a card of the board, no block goes below a comment (400 at
/// `parentId`), and a card with comments stays a card (400 at `type`). A comment's body
/// has no file block, nor does a block made a comment (400 at `body`). The block's
/// `modifiedBy` and `updatedAt` say who changed it and when.
#[utoipa::path(
    patch,
    path = "/w/{workspaceId}/boards/{boardId}/blocks/{blockId}",
    operation_id = "patchBlock",
    tag = tag::BLOCKS,
    params(MemberWorkspaceId, BoardId, BlockId, DisableNotify),
    request_body = BlockPatch,
    responses((status = 204, description = "The block is changed.")),
)]
async fn handle_patch_block(
    ws: InWorkspace,
    Path((_, board_id, block_id)): Path<(String, String, String)>,
    QueryParams(query): QueryParams<DisableNotifyQuery>,
    body: Bytes,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    let notification = query.notification();
    // Who cannot even comment changes nothing, whatever the block.
    let grant = ws.app.container_grant(user_id, &board_id).await?;
    require_granted(grant, ContainerPermission::CommentCards, CHANGES_DENIED)?;
    let block = block_on_container(&ws.app, &block_id, &board_id).await?;
    require_block_permission_with_grant(
        grant,
        user_id,
        &block,
        BlockAction::Change,
        CHANGES_DENIED,
    )?;
    let (mut patch, changes): (BlockPatch, _) = decode_object_with(&body, |patch| {
        check_patch_block_types([Some(&mut *patch)], None)?;
        take_patch_bodies([Some(patch)], None)
    })?;
    put_patch_bodies([&mut patch], changes?, None)?;
    require_block_patch_permission_with_grant(grant, &block, &patch, CHANGES_DENIED)?;
    ws.app
        .check_body_files(&board_id, patch.body.new_body())
        .await?;

    // The store names what it refuses as a block has it; a patch has it under another
    // name.
    ws.app
        .patch_block(&block_id, &patch, user_id, notification)
        .await
        .map_err(|error| error.moved(BLOCK_FIELDS, PATCH_FIELDS))?;
    tracing::debug!(board_id, block_id, "PATCH Block");
    Ok(no_content())
}

/// Change several blocks
///
/// Applies a patch to each of several blocks of the board, all or none. A body that
/// names no block changes nothing and is answered 204 too. Each block needs what
/// `patchBlock` needs: the editor's role, for a comment its author's commenter role or
/// for the comment of somebody else the board's admin role.
#[utoipa::path(
    patch,
    path = "/w/{workspaceId}/boards/{boardId}/blocks",
    operation_id = "patchBlocks",
    tag = tag::BLOCKS,
    params(MemberWorkspaceId, BoardId, DisableNotify),
    request_body = BlockPatchBatch,
    responses((status = 204, description = "The blocks are changed.")),
)]
async fn handle_patch_blocks(
    ws: InWorkspace,
    Path((_, board_id)): Path<(String, String)>,
    QueryParams(query): QueryParams<DisableNotifyQuery>,
    body: Bytes,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    let notification = query.notification();
    // Who cannot even comment changes nothing, whatever the blocks.
    let grant = ws.app.container_grant(user_id, &board_id).await?;
    require_granted(grant, ContainerPermission::CommentCards, CHANGES_DENIED)?;
    let board = ws.app.container(&board_id).await?;
    let (mut batch, changes): (BlockPatchBatch, _) = decode_object_with(&body, |batch| {
        check_patch_block_types(objects_mut(batch, "blockPatches"), Some(PATCHES))?;
        take_patch_bodies(objects_mut(batch, "blockPatches"), Some(PATCHES))
    })?;
    put_patch_bodies(&mut batch.block_patches, changes?, Some(PATCHES))?;
    if batch.block_ids.len() != batch.block_patches.len() {
        return Err(
            AppError::bad_request("blockIds and blockPatches differ in length")
                .with_field("blockIds"),
        );
    }
    let mut seen = std::collections::HashSet::new();
    for (index, block_id) in batch.block_ids.iter().enumerate() {
        if !seen.insert(block_id.as_str()) {
            return Err(AppError::bad_request("a block is listed twice")
                .with_pointer(format!("/blockIds/{index}")));
        }
    }
    if !batch.block_ids.is_empty() {
        let Some(fetched) = ws.app.blocks_by_ids(&batch.block_ids).await? else {
            return Err(AppError::not_found("block"));
        };
        let by_id: std::collections::HashMap<&str, &Block> = fetched
            .iter()
            .map(|block| (block.id.as_str(), block))
            .collect();
        for (block_id, patch) in batch.block_ids.iter().zip(&batch.block_patches) {
            let Some(&block) = by_id
                .get(block_id.as_str())
                .filter(|block| block.container_id == board_id)
            else {
                return Err(AppError::not_found("block"));
            };
            require_block_permission_with_grant(
                grant,
                user_id,
                block,
                BlockAction::Change,
                CHANGES_DENIED,
            )?;
            require_block_patch_permission_with_grant(grant, block, patch, CHANGES_DENIED)?;
        }
    }
    for (index, patch) in batch.block_patches.iter().enumerate() {
        ws.app
            .check_body_files(&board_id, patch.body.new_body())
            .await
            .map_err(|error| error.under(&element_pointer(Some(PATCHES), index)))?;
    }

    ws.app
        .patch_blocks(&board.workspace_id, &batch, user_id, notification)
        .await
        .map_err(|error| {
            error
                .moved(BLOCK_FIELDS, PATCH_FIELDS)
                .among_blocks(batch.block_ids.iter().map(String::as_str), PATCHES)
        })?;
    tracing::debug!(patches = batch.block_ids.len(), "PATCH Blocks");
    Ok(no_content())
}

/// Copy a block
///
/// Copies a block and the blocks directly below it, except comments. A comment cannot
/// be copied. For editors.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/boards/{boardId}/blocks/{blockId}/duplicate",
    operation_id = "duplicateBlock",
    tag = tag::BLOCKS,
    params(MemberWorkspaceId, BoardId, BlockId, AsTemplate),
    responses((
        status = 201,
        description = "The copies: the copy of the block first, then those of the blocks directly below it, oldest first.",
        body = Vec<Block>,
    )),
)]
async fn handle_duplicate_block(
    ws: InWorkspace,
    Path((_, board_id, block_id)): Path<(String, String, String)>,
    QueryParams(query): QueryParams<AsTemplateQuery>,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    let duplicate_as = query.duplicate_as();

    require_container_permission(
        &ws.app,
        user_id,
        &board_id,
        ContainerPermission::View,
        "access denied to modify board cards",
    )
    .await?;
    let board = ws.app.container(&board_id).await?;
    let block = block_on_container(&ws.app, &block_id, &board.id).await?;
    if block.block_type == BlockType::Comment {
        require_container_permission(
            &ws.app,
            user_id,
            &board_id,
            ContainerPermission::CommentCards,
            "access denied to comment on board cards",
        )
        .await?;
        // Comments are never copied: the store leaves them out of every duplication.
        return Err(AppError::bad_request("a comment cannot be duplicated"));
    }
    require_container_permission(
        &ws.app,
        user_id,
        &board_id,
        ContainerPermission::ManageCards,
        "access denied to modify board cards",
    )
    .await?;

    tracing::debug!(board_id, block_id, "DuplicateBlock");
    let blocks = ws
        .app
        .duplicate_block(&board_id, &block_id, user_id, duplicate_as)
        .await?;
    Ok(created(&blocks))
}
