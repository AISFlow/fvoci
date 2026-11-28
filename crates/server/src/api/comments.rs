//! The comments on an item ([`ItemKind`]): a card, a page or a unit. Each route of them
//! is the same for every kind (`card_comments`, `page_comments`, `unit_comments` name
//! them), and is written here once.

use axum::body::Bytes;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use fvoci_model::{
    Block, BlockType, Body, BodyChange, CommentWithReactions, RefusalReason, Refusals,
};
use serde::de::DeserializeOwned;

use super::delegated::{Admits, Wall};
use super::items::{ItemKind, ItemOf, ItemPath};
use super::workspace::InWorkspace;
use super::{ApiResult, BlockAction, created, decode_with_document, json_response, no_content};
pub(super) use crate::app::is_comment_on;
use crate::app::{App, ChangeNotification, Reached};
use crate::error::{AppError, OrMissing};
use crate::permissions::ContainerPermission;

/// Refusal message when a comment body is empty or omitted.
const TEXT_MISSING: &str = "a comment has a text";

/// Refusal message when `readAt` is omitted on a comment patch.
const READ_AT_MISSING: &str =
    "a comment is changed with readAt, the updatedAt of the comment as it was read";

/// A kind of item that is commented on: what one is refused its comments with.
pub(super) trait CommentParentKind: ItemKind {
    /// What one who may not read the item is refused its comments with.
    const COMMENTS_DENIED: &'static str;
    /// What one who may not comment on the item is refused writing one with.
    const CREATE_DENIED: &'static str;
    /// What one who may not comment on the item is refused changing one with.
    const CHANGES_DENIED: &'static str;
}

/// Who comments on the item a route names, or changes a comment on it: one who may
/// comment in its container ([`ContainerPermission::CommentCards`]), on a live item of
/// the kind `K`. Asked as the request is taken, before its body is read.
#[derive(Debug)]
pub(super) struct CommenterOf<K> {
    pub(super) ws: InWorkspace,
    path: ItemPath,
    /// The container, as the commenter reached it: what the comment writes ask.
    pub(super) on: Reached,
    pub(super) parent: Block,
    _kind: std::marker::PhantomData<K>,
}

impl<K: CommentParentKind> CommenterOf<K> {
    /// Who comments on the item `path` names (or changes the comment it names on it), in
    /// the workspace `ws` was entered for: one who may comment there, on a live item of
    /// the kind `K`.
    ///
    /// # Errors
    ///
    /// Returns what [`Reached::require`] refuses with, and 404 for an item that is not
    /// there.
    pub(super) async fn admit(ws: InWorkspace, path: ItemPath) -> Result<Self, AppError> {
        let ws = ws.reaching(K::KINDS);
        let denial = if path.names_a_comment() {
            K::CHANGES_DENIED
        } else {
            K::CREATE_DENIED
        };
        let (on, parent) = K::reach(&ws, &path, ContainerPermission::CommentCards, denial).await?;
        Ok(Self {
            ws,
            path,
            on,
            parent,
            _kind: std::marker::PhantomData,
        })
    }

    /// The live comment on the item the route names.
    async fn comment(&self) -> Result<Block, AppError> {
        find_live_comment_on(&self.ws, &self.parent, self.path.comment_id()?)
            .await?
            .ok_or_else(|| AppError::not_found("comment"))
    }
}

impl<K: CommentParentKind> FromRequestParts<App> for CommenterOf<K>
where
    K::In: FromRequestParts<App, Rejection = AppError>,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, app: &App) -> Result<Self, Self::Rejection> {
        let ItemOf { ws, path, .. } = ItemOf::<K>::from_request_parts(parts, app).await?;
        Self::admit(ws, path).await
    }
}

/// For a token where the kind's wall lets one through, as [`ItemOf`] is.
impl<K: CommentParentKind> Wall for CommenterOf<K>
where
    K::In: Wall,
{
    async fn enter(parts: &mut Parts, app: &App, admits: Admits) -> Result<Self, AppError> {
        let ItemOf { ws, path, .. } = ItemOf::<K>::enter(parts, app, admits).await?;
        Self::admit(ws, path).await
    }
}

/// The comments on the item `path` names, oldest first, for who may read it.
///
/// # Errors
///
/// Returns what the kind's [`ItemKind::reach`] refuses with, and the store's error.
pub(super) async fn comments_of<K: CommentParentKind>(
    ws: &InWorkspace,
    path: &ItemPath,
) -> Result<Vec<Block>, AppError> {
    let (on, parent) = K::reach(ws, path, ContainerPermission::View, K::COMMENTS_DENIED).await?;
    let mut comments = ws
        .app
        .blocks(
            on.container_id(),
            Some(&parent.id),
            Some(&BlockType::Comment),
        )
        .await?;
    comments.sort_by(|one, other| (one.created_at, &one.id).cmp(&(other.created_at, &other.id)));
    Ok(comments)
}

/// The comments on the item a route names, as [`comments_of`] reads them, each with the
/// reactions to it: the members' lists (`getCardComments` and the others; the MCP tools
/// and a public link's reader read none).
pub(super) async fn list_comments_of<K: CommentParentKind>(item: ItemOf<K>) -> ApiResult {
    let comments = comments_of::<K>(&item.ws, &item.path).await?;
    Ok(json_response(
        &with_their_reactions(&item.ws, comments).await?,
    ))
}

/// The comments, each with the reactions to it.
///
/// # Errors
///
/// Returns the store's error.
async fn with_their_reactions(
    ws: &InWorkspace,
    comments: Vec<Block>,
) -> Result<Vec<CommentWithReactions>, AppError> {
    let mut reactions = {
        let ids: Vec<&str> = comments.iter().map(|comment| comment.id.as_str()).collect();
        ws.app.reactions_of(&ids).await?
    };
    Ok(comments
        .into_iter()
        .map(|comment| CommentWithReactions {
            reactions: reactions.remove(&comment.id).unwrap_or_default(),
            comment,
        })
        .collect())
}

/// Writes a comment on the item the commenter was admitted to from the request's body,
/// decoded as `T` says ([`App::create_comment`]), and answers with it as stored (201).
pub(super) async fn create_comment_of<K: CommentParentKind, T: DeserializeOwned>(
    commenter: CommenterOf<K>,
    raw_body: &Bytes,
    notification: ChangeNotification,
) -> ApiResult {
    let (_new, document): (T, _) = decode_with_document(raw_body)?;
    let text = text_of(document)?;
    let asked = (K::WHAT, K::CREATE_DENIED);
    let comment = commenter
        .ws
        .app
        .create_comment(&commenter.on, &commenter.parent, text, asked, notification)
        .await?;
    Ok(created(&comment))
}

/// Writes a new text over the comment the route names, from the request's body decoded
/// as `T` says, whose `readAt` `read_at_of` gives ([`App::patch_comment`]).
pub(super) async fn patch_comment_of<K: CommentParentKind, T: DeserializeOwned>(
    commenter: CommenterOf<K>,
    raw_body: &Bytes,
    notification: ChangeNotification,
    read_at_of: impl FnOnce(&T) -> Option<i64>,
) -> ApiResult {
    let comment = commenter.comment().await?;
    let on = &commenter.on;
    // Asked before the body is read, as every route asks.
    on.require_on_comment(&comment, BlockAction::Change, K::CHANGES_DENIED)?;
    let (change, document): (T, _) = decode_with_document(raw_body)?;
    let Some(read_at) = read_at_of(&change) else {
        let mut refused = Refusals::new();
        refused.refuse("readAt", RefusalReason::Required, READ_AT_MISSING);
        return Err(refused.into());
    };
    let text = text_of(document)?;
    let patched = commenter
        .ws
        .app
        .patch_comment(
            on,
            &comment,
            (text, read_at),
            K::CHANGES_DENIED,
            notification,
        )
        .await?;
    Ok(json_response(&patched))
}

/// Deletes the comment the route names ([`App::delete_comment`]).
pub(super) async fn delete_comment_of<K: CommentParentKind>(
    commenter: CommenterOf<K>,
    notification: ChangeNotification,
) -> ApiResult {
    let comment = commenter.comment().await?;
    commenter
        .ws
        .app
        .delete_comment(&commenter.on, &comment, K::CHANGES_DENIED, notification)
        .await?;
    Ok(no_content())
}

/// Brings the comment the route names back from the trash onto its item
/// ([`App::restore_comment`]).
pub(super) async fn restore_comment_of<K: CommentParentKind>(
    commenter: CommenterOf<K>,
) -> ApiResult {
    let comment = commenter
        .ws
        .app
        .restore_comment(
            &commenter.on,
            &commenter.parent,
            commenter.path.comment_id()?,
            K::CHANGES_DENIED,
        )
        .await?;
    Ok(json_response(&comment))
}

/// The earlier versions of the comment the route names on its item, live or deleted on
/// its own, for who may read the item.
pub(super) async fn comment_versions_of<K: CommentParentKind>(item: ItemOf<K>) -> ApiResult {
    let ItemOf { ws, path, .. } = item;
    let (_, parent) = K::reach(&ws, &path, ContainerPermission::View, K::COMMENTS_DENIED).await?;
    let comment_id = path.comment_id()?;
    let live = find_live_comment_on(&ws, &parent, comment_id).await?;
    let id = match live {
        Some(comment) => comment.id,
        None => {
            ws.app
                .deleted_block(comment_id)
                .await?
                .map(|deleted| deleted.block)
                .filter(|block| is_comment_on(block, &parent))
                .ok_or_else(|| AppError::not_found("comment"))?
                .id
        }
    };
    let versions = ws.app.comment_versions(&id).await?;
    Ok(json_response(&versions))
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

/// Resolves a live comment under an already-authorized `parent` block (`None` when absent
/// or not a comment on `parent`).
pub(super) async fn find_live_comment_on(
    ws: &InWorkspace,
    parent: &Block,
    comment_id: &str,
) -> Result<Option<Block>, AppError> {
    let found = ws.app.block(comment_id).await.found()?;
    Ok(found.filter(|block| is_comment_on(block, parent)))
}
