//! Shared helpers and `CommenterOf` extractor for comment endpoints on room-bearing
//! blocks (`card_comments`, `page_comments`, `unit_comments`).

use std::future::Future;
use std::marker::PhantomData;

use axum::body::Bytes;
use axum::extract::{FromRequestParts, RawPathParams};
use axum::http::request::Parts;
use fvoci_model::{
    Block, BlockPatch, BlockType, Body, BodyChange, ContainerKinds, RefusalReason, Refusals, new_id,
};
use serde::de::DeserializeOwned;

use super::delegated::{Admits, Wall};
use super::workspace::InWorkspace;
use super::{
    ApiResult, BlockAction, created, decode_with_document, json_response, no_content,
    require_block_permission, require_container_permission, require_delete_permission,
    require_deletion_permission,
};
use crate::app::{App, ChangeNotification};
use crate::error::AppError;
use crate::permissions::ContainerPermission;

/// Refusal message when a comment body is empty or omitted.
const TEXT_MISSING: &str = "a comment has a text";

/// Refusal message when `readAt` is omitted on a comment patch.
const READ_AT_MISSING: &str =
    "a comment is changed with readAt, the updatedAt of the comment as it was read";

/// Domain descriptor for a commentable parent block kind (`card`, `page`, `unit`).
pub(super) trait CommentParentKind: Send + Sync + 'static {
    const CONTAINER_PARAM: &'static str;
    const PARENT_PARAM: &'static str;
    const CREATE_DENIED: &'static str;
    const CHANGES_DENIED: &'static str;

    fn kinds() -> ContainerKinds;

    fn load_parent(
        ws: &InWorkspace,
        container_id: &str,
        parent_id: &str,
    ) -> impl Future<Output = Result<Block, AppError>> + Send;
}

/// Request-parts extractor verifying workspace entry, `CommentCards` container
/// permission, and existence of the live parent block (`card`, `page`, or `unit`).
pub(super) struct CommenterOf<K> {
    pub(super) ws: InWorkspace,
    pub(super) container_id: String,
    pub(super) parent: Block,
    pub(super) comment_id: Option<String>,
    _kind: PhantomData<K>,
}

impl<K: CommentParentKind> CommenterOf<K> {
    pub(super) fn comment_id(&self) -> Result<&str, AppError> {
        self.comment_id
            .as_deref()
            .ok_or_else(|| AppError::internal("route missing commentId path parameter"))
    }
}

impl<K: CommentParentKind> FromRequestParts<App> for CommenterOf<K> {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, app: &App) -> Result<Self, Self::Rejection> {
        Self::enter(parts, app, Admits::Sessions).await
    }
}

impl<K: CommentParentKind> Wall for CommenterOf<K> {
    async fn enter(parts: &mut Parts, app: &App, admits: Admits) -> Result<Self, AppError> {
        let ws = InWorkspace::enter(parts, app, admits)
            .await?
            .reaching(K::kinds());
        let params = RawPathParams::from_request_parts(parts, app)
            .await
            .map_err(|_| AppError::bad_request("invalid UTF-8 in path parameter"))?;
        let mut container_id = None;
        let mut parent_id = None;
        let mut comment_id = None;
        for (name, value) in &params {
            if name == K::CONTAINER_PARAM {
                container_id = Some(value.to_owned());
            } else if name == K::PARENT_PARAM {
                parent_id = Some(value.to_owned());
            } else if name == "commentId" {
                comment_id = Some(value.to_owned());
            }
        }
        let container_id = container_id
            .ok_or_else(|| AppError::internal("route missing container path parameter"))?;
        let parent_id =
            parent_id.ok_or_else(|| AppError::internal("route missing parent path parameter"))?;
        Self::admit(ws, container_id, &parent_id, comment_id).await
    }
}

impl<K: CommentParentKind> CommenterOf<K> {
    /// Who comments on the block `parent_id` of the container `container_id` (or
    /// changes the comment `comment_id` on it), in the workspace `ws` was entered for:
    /// one who may comment there, on a live parent of the kind `K`.
    ///
    /// # Errors
    ///
    /// Returns what `require_container_permission` refuses with, and 404 for a parent
    /// that is not there.
    pub(super) async fn admit(
        ws: InWorkspace,
        container_id: String,
        parent_id: &str,
        comment_id: Option<String>,
    ) -> Result<Self, AppError> {
        let ws = ws.reaching(K::kinds());
        let denial = if comment_id.is_some() {
            K::CHANGES_DENIED
        } else {
            K::CREATE_DENIED
        };
        require_container_permission(
            &ws.app,
            &ws.caller.user_id,
            &container_id,
            ContainerPermission::CommentCards,
            denial,
        )
        .await?;
        let parent = K::load_parent(&ws, &container_id, parent_id).await?;
        Ok(Self {
            ws,
            container_id,
            parent,
            comment_id,
            _kind: PhantomData,
        })
    }
}

/// Extracts the non-empty [`Body`] from a comment request's [`BodyChange`].
pub(super) fn text_of(change: BodyChange) -> Result<Body, AppError> {
    match change {
        BodyChange::Set(body) => Ok(body),
        BodyChange::Keep | BodyChange::Clear => {
            let mut refused = Refusals::new();
            refused.refuse("body", RefusalReason::Required, TEXT_MISSING);
            Err(refused.into())
        }
    }
}

/// Whether `block` is a comment directly on `parent` within the same container.
pub(super) fn is_comment_on(block: &Block, parent: &Block) -> bool {
    block.block_type == BlockType::Comment
        && block.container_id == parent.container_id
        && block.parent_id == parent.id
}

/// Resolves a live comment under an already-authorized `parent` block (`None` when absent
/// or not a comment on `parent`).
pub(super) async fn find_live_comment_on(
    ws: &InWorkspace,
    parent: &Block,
    comment_id: &str,
) -> Result<Option<Block>, AppError> {
    match ws.app.block(comment_id).await {
        Ok(block) if is_comment_on(&block, parent) => Ok(Some(block)),
        Ok(_) => Ok(None),
        Err(error) if error.is_not_found() => Ok(None),
        Err(error) => Err(error),
    }
}

/// Resolves a required live comment under an already-authorized `parent` block (404 when
/// absent).
pub(super) async fn existing_comment_on(
    ws: &InWorkspace,
    parent: &Block,
    comment_id: &str,
) -> Result<Block, AppError> {
    find_live_comment_on(ws, parent, comment_id)
        .await?
        .ok_or_else(|| AppError::not_found("comment"))
}

/// The comments on an already-authorized `parent` block, oldest first.
///
/// # Errors
///
/// Returns the store's error.
pub(super) async fn comments_on(
    ws: &InWorkspace,
    container_id: &str,
    parent_id: &str,
) -> Result<Vec<Block>, AppError> {
    let mut comments = ws
        .app
        .blocks(container_id, Some(parent_id), Some(&BlockType::Comment))
        .await?;
    comments.sort_by(|one, other| (one.created_at, &one.id).cmp(&(other.created_at, &other.id)));
    Ok(comments)
}

/// Creates a comment on an already-authorized `parent` block from a decoded payload `T`.
pub(super) async fn create_comment_on<T: DeserializeOwned>(
    ws: &InWorkspace,
    container_id: &str,
    parent: Block,
    parent_what: &'static str,
    raw_body: &Bytes,
    notification: ChangeNotification,
) -> Result<Block, AppError> {
    let user_id = ws.caller.user_id.as_str();
    let (_new, document): (T, _) = decode_with_document(raw_body)?;
    let text = text_of(document)?;
    ws.app.check_body_files(container_id, Some(&text)).await?;
    // Signed by the agent that writes it, if one does, as the store writes it.
    let mut comment = Block {
        id: new_id(),
        container_id: container_id.to_owned(),
        parent_id: parent.id,
        block_type: BlockType::Comment,
        body: Some(text),
        ..Block::default()
    };
    if let Err(error) = ws
        .app
        .insert_block(&mut comment, user_id, notification)
        .await
    {
        return Err(match ws.app.block(&comment.parent_id).await {
            Err(gone) if gone.is_not_found() => AppError::not_found(parent_what),
            _ => error,
        });
    }
    Ok(comment)
}

/// Patches an already-resolved `comment` block using `read_at_of` to extract `readAt`.
pub(super) async fn patch_comment_on<T: DeserializeOwned>(
    ws: &InWorkspace,
    container_id: &str,
    comment: &Block,
    changes_denied: &str,
    raw_body: &Bytes,
    notification: ChangeNotification,
    read_at_of: impl FnOnce(&T) -> Option<i64>,
) -> Result<Block, AppError> {
    let user_id = ws.caller.user_id.as_str();
    require_block_permission(
        &ws.app,
        user_id,
        comment,
        BlockAction::Change,
        changes_denied,
    )
    .await?;
    let (change, document): (T, _) = decode_with_document(raw_body)?;
    let Some(read_at) = read_at_of(&change) else {
        let mut refused = Refusals::new();
        refused.refuse("readAt", RefusalReason::Required, READ_AT_MISSING);
        return Err(refused.into());
    };
    let text = text_of(document)?;
    ws.app.check_body_files(container_id, Some(&text)).await?;
    let patch = BlockPatch {
        body: BodyChange::Set(text),
        read_at: Some(read_at),
        ..BlockPatch::default()
    };
    ws.app
        .patch_block(&comment.id, &patch, user_id, notification)
        .await
}

/// Deletes an already-resolved `comment` block after checking comment delete permission.
pub(super) async fn delete_comment_on(
    ws: &InWorkspace,
    comment: &Block,
    changes_denied: &str,
    notification: ChangeNotification,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    require_delete_permission(&ws.app, user_id, comment, changes_denied).await?;
    ws.app
        .delete_block(&comment.id, user_id, notification)
        .await?;
    Ok(no_content())
}

/// Restores a comment deleted on its own onto an already-authorized live `parent` block.
pub(super) async fn restore_comment_on(
    ws: &InWorkspace,
    parent: &Block,
    comment_id: &str,
    changes_denied: &str,
) -> Result<Block, AppError> {
    let user_id = ws.caller.user_id.as_str();
    let deletion = ws.app.deletion(comment_id).await?;
    let Some((top, below)) = deletion
        .split_first()
        .filter(|(top, _)| top.id == comment_id && is_comment_on(top, parent))
    else {
        return Err(AppError::not_found("comment"));
    };
    let deleted_by = ws.app.deleted_by(&top.id).await?;
    require_deletion_permission(
        &ws.app,
        user_id,
        top,
        below,
        BlockAction::Undelete {
            deleted_by: &deleted_by,
        },
        changes_denied,
    )
    .await?;
    ws.app.undelete_block(comment_id, user_id).await
}

/// Lists prior versions of a live or singly-deleted comment on `parent`.
pub(super) async fn get_comment_versions_on(
    ws: &InWorkspace,
    parent: &Block,
    live: Option<Block>,
    comment_id: &str,
) -> ApiResult {
    let id = match live {
        Some(comment) => comment.id,
        None => {
            ws.app
                .deleted_block(comment_id)
                .await?
                .map(|deleted| deleted.block)
                .filter(|block| is_comment_on(block, parent))
                .ok_or_else(|| AppError::not_found("comment"))?
                .id
        }
    };
    let versions = ws.app.comment_versions(&id).await?;
    Ok(json_response(&versions))
}

pub(super) fn created_comment(comment: &Block) -> axum::response::Response {
    created(comment)
}
