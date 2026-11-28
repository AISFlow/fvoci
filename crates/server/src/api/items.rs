//! The items of a container that have a body of their own, as a route names them: a card
//! of a board, a page of a wiki, a unit of a course, an entry of a diary. The versions of
//! their bodies and the comments on them are the same operations on each
//! (`body_versions`, `comments`); what differs is the kind ([`ItemKind`]): the wall its
//! routes enter by, the names of its path, and how its container and the item are read.
//!
//! A token reaches an item only through a wall that lets tokens through: [`ItemOf`] is a
//! [`Wall`] only where its kind's wall is one. A diary's wall ([`InDiary`]) is none, so
//! no route that takes `Delegated` of an entry compiles.

use std::future::{Future, ready};
use std::marker::PhantomData;

use axum::extract::{FromRequestParts, RawPathParams};
use axum::http::request::Parts;
use fvoci_model::{Block, ContainerKinds, DiaryEntry, Page, Unit};

use super::card_comments::card_on_board;
use super::cards::card_reached;
use super::comments::CommentParentKind;
use super::courses::content_unit_reached;
use super::delegated::{Admits, NotAWall, Wall};
use super::diaries::entry_reached;
use super::wikis::page_reached;
use super::workspace::{InCourse, InDiary, InWiki, InWorkspace};
use super::{ApiResult, json_response};
use crate::app::{App, Reached};
use crate::error::AppError;
use crate::permissions::ContainerPermission;

/// A kind of item that has a body of its own: what its routes enter by and are named by,
/// and how the item is read.
pub(super) trait ItemKind: Send + Sync + 'static {
    /// The wall the kind's routes enter the workspace by.
    type In: Into<InWorkspace> + Send;
    /// The containers that hold the kind: what a comment on one is written in.
    const KINDS: ContainerKinds;
    /// The path parameter that names the container of the item: `wikiId`.
    const CONTAINER_PARAM: &'static str;
    /// The path parameter that names the item: `pageId`.
    const ITEM_PARAM: &'static str;
    /// What the item is called: `page`.
    const WHAT: &'static str;
    /// What one who may not read the item is refused it with.
    const READ_DENIED: &'static str;
    /// What one who may not edit the item is refused keeping a version of its body with.
    const KEEP_DENIED: &'static str;
    /// What one who may not edit the item is refused going back to a version with.
    const RESTORE_DENIED: &'static str;

    /// The container `path` names, as `ws`'s caller reached it, and the item of it, for
    /// a caller who has `permission` there: the kind asks in its own order, and refuses
    /// who has not with `denial`.
    fn reach(
        ws: &InWorkspace,
        path: &ItemPath,
        permission: ContainerPermission,
        denial: &str,
    ) -> impl Future<Output = Result<(Reached, Block), AppError>> + Send;

    /// What a route that wrote the item's body answers with: the item as it is now.
    fn answer(app: &App, item: Block) -> impl Future<Output = ApiResult> + Send;
}

/// The identifiers a route names an item by.
#[derive(Debug, Clone)]
pub(super) struct ItemPath {
    /// The container, if the route names it.
    container_id: Option<String>,
    item_id: String,
    comment_id: Option<String>,
    version: Option<String>,
}

impl ItemPath {
    /// The item `item_id` of `container_id`, named other than by a route's path: by a
    /// tool of the MCP server.
    pub(super) fn named(container_id: &str, item_id: &str) -> Self {
        Self {
            container_id: Some(container_id.to_owned()),
            item_id: item_id.to_owned(),
            comment_id: None,
            version: None,
        }
    }

    /// The container the route names.
    ///
    /// # Errors
    ///
    /// Returns an internal error for a route that names none.
    pub(super) fn container_id(&self) -> Result<&str, AppError> {
        self.container_id
            .as_deref()
            .ok_or_else(|| AppError::internal("route missing container path parameter"))
    }

    /// The item the route names.
    pub(super) fn item_id(&self) -> &str {
        &self.item_id
    }

    /// The comment on the item the route names.
    ///
    /// # Errors
    ///
    /// Returns an internal error for a route that names none.
    pub(super) fn comment_id(&self) -> Result<&str, AppError> {
        self.comment_id
            .as_deref()
            .ok_or_else(|| AppError::internal("route missing commentId path parameter"))
    }

    /// Whether the route names a comment on the item.
    pub(super) fn names_a_comment(&self) -> bool {
        self.comment_id.is_some()
    }

    /// The version of the item's body the route names, as the path has it.
    ///
    /// # Errors
    ///
    /// Returns an internal error for a route that names none.
    pub(super) fn version(&self) -> Result<&str, AppError> {
        self.version
            .as_deref()
            .ok_or_else(|| AppError::internal("route missing version path parameter"))
    }
}

/// The item of the kind `K` a route names, in the workspace entered by the kind's wall:
/// nothing of the container or the item is read yet, as what the caller is to be
/// allowed there is the route's to say ([`ItemOf::reach`]).
#[derive(Debug)]
pub(super) struct ItemOf<K> {
    pub(super) ws: InWorkspace,
    pub(super) path: ItemPath,
    _kind: PhantomData<K>,
}

impl<K: ItemKind> ItemOf<K> {
    /// The item the request names in the workspace `ws` its wall entered.
    async fn within(ws: InWorkspace, parts: &mut Parts, app: &App) -> Result<Self, AppError> {
        // The wall has read the path: none of it is anything but text.
        let params = RawPathParams::from_request_parts(parts, app)
            .await
            .map_err(|_| AppError::bad_request("invalid UTF-8 in path parameter"))?;
        let (mut container_id, mut item_id, mut comment_id, mut version) = (None, None, None, None);
        for (name, value) in &params {
            let slot = match name {
                _ if name == K::CONTAINER_PARAM => &mut container_id,
                _ if name == K::ITEM_PARAM => &mut item_id,
                "commentId" => &mut comment_id,
                "version" => &mut version,
                _ => continue,
            };
            *slot = Some(value.to_owned());
        }
        let item_id =
            item_id.ok_or_else(|| AppError::internal("route missing item path parameter"))?;
        let path = ItemPath {
            container_id,
            item_id,
            comment_id,
            version,
        };
        Ok(Self {
            ws,
            path,
            _kind: PhantomData,
        })
    }

    /// The container the path names, as the caller reached it, and the item, for a
    /// caller who has `permission` there; who has not is refused with `denial`, as the
    /// kind asks ([`ItemKind::reach`]).
    ///
    /// # Errors
    ///
    /// Returns what the kind's [`ItemKind::reach`] refuses with: 404 for an item that is
    /// not there.
    pub(super) async fn reach(
        &self,
        permission: ContainerPermission,
        denial: &str,
    ) -> Result<(Reached, Block), AppError> {
        K::reach(&self.ws, &self.path, permission, denial).await
    }
}

/// For a session or a token, where the kind's wall lets a token through: of a kind whose
/// wall is no [`Wall`] (a diary's) no `Delegated` compiles.
impl<K: ItemKind> Wall for ItemOf<K>
where
    K::In: Wall,
{
    async fn enter(parts: &mut Parts, app: &App, admits: Admits) -> Result<Self, AppError> {
        let wall = K::In::enter(parts, app, admits).await?;
        Self::within(wall.into(), parts, app).await
    }
}

/// For a session alone, by the kind's wall as it takes one.
impl<K: ItemKind> FromRequestParts<App> for ItemOf<K>
where
    K::In: FromRequestParts<App, Rejection = AppError>,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, app: &App) -> Result<Self, Self::Rejection> {
        let wall = K::In::from_request_parts(parts, app).await?;
        Self::within(wall.into(), parts, app).await
    }
}

// No `Wall` of an entry: a token never reaches a diary.
const _: () = <ItemOf<EntryItem> as NotAWall<_>>::CHECKED;

/// A card of a board. Its body's versions are named by the card alone
/// (`/cards/{cardId}`), whose board is the one the card is on; its comments by the
/// board and the card.
#[derive(Debug)]
pub(super) struct CardItem;

impl ItemKind for CardItem {
    type In = InWorkspace;
    const KINDS: ContainerKinds = ContainerKinds::BOARDS;
    const CONTAINER_PARAM: &'static str = "boardId";
    const ITEM_PARAM: &'static str = "cardId";
    const WHAT: &'static str = "card";
    const READ_DENIED: &'static str = "access denied to fetch card";
    const KEEP_DENIED: &'static str = "access denied to keep a version of the card's body";
    const RESTORE_DENIED: &'static str = "access denied to restore a version of the card's body";

    async fn reach(
        ws: &InWorkspace,
        path: &ItemPath,
        permission: ContainerPermission,
        denial: &str,
    ) -> Result<(Reached, Block), AppError> {
        let user_id = ws.caller.user_id.as_str();
        let Some(board_id) = path.container_id.as_deref() else {
            // The card first, then its board: the path says nothing of a board.
            return card_reached(&ws.app, user_id, path.item_id(), permission, denial).await;
        };
        let on = ws.app.reach(user_id, board_id).await?;
        on.require(permission, denial)?;
        let card = card_on_board(ws, board_id, path.item_id()).await?;
        Ok((on, card))
    }

    async fn answer(app: &App, item: Block) -> ApiResult {
        Ok(json_response(&app.card_of_block(&item).await?))
    }
}

impl CommentParentKind for CardItem {
    const COMMENTS_DENIED: &'static str = "access denied to fetch card comments";
    const CREATE_DENIED: &'static str = "access denied to comment on the card";
    const CHANGES_DENIED: &'static str = "access denied to change the comments of the card";
}

/// A page of a wiki.
#[derive(Debug)]
pub(super) struct PageItem;

impl ItemKind for PageItem {
    type In = InWiki;
    const KINDS: ContainerKinds = ContainerKinds::WIKIS;
    const CONTAINER_PARAM: &'static str = "wikiId";
    const ITEM_PARAM: &'static str = "pageId";
    const WHAT: &'static str = "page";
    const READ_DENIED: &'static str = super::wikis::READ_DENIED;
    const KEEP_DENIED: &'static str = "access denied to keep a version of the page's body";
    const RESTORE_DENIED: &'static str = "access denied to restore a version of the page's body";

    async fn reach(
        ws: &InWorkspace,
        path: &ItemPath,
        permission: ContainerPermission,
        denial: &str,
    ) -> Result<(Reached, Block), AppError> {
        page_reached(ws, path.container_id()?, path.item_id(), permission, denial).await
    }

    fn answer(_: &App, item: Block) -> impl Future<Output = ApiResult> + Send {
        ready(Ok(json_response(&Page::from(item))))
    }
}

impl CommentParentKind for PageItem {
    const COMMENTS_DENIED: &'static str = Self::READ_DENIED;
    const CREATE_DENIED: &'static str = "access denied to comment on the page";
    const CHANGES_DENIED: &'static str = "access denied to change the comments of the page";
}

/// A unit of a course, not a chapter: a chapter has no body.
#[derive(Debug)]
pub(super) struct UnitItem;

impl ItemKind for UnitItem {
    type In = InCourse;
    const KINDS: ContainerKinds = ContainerKinds::COURSES;
    const CONTAINER_PARAM: &'static str = "courseId";
    const ITEM_PARAM: &'static str = "unitId";
    const WHAT: &'static str = "unit";
    const READ_DENIED: &'static str = super::courses::READ_DENIED;
    const KEEP_DENIED: &'static str = "access denied to keep a version of the unit's body";
    const RESTORE_DENIED: &'static str = "access denied to restore a version of the unit's body";

    async fn reach(
        ws: &InWorkspace,
        path: &ItemPath,
        permission: ContainerPermission,
        denial: &str,
    ) -> Result<(Reached, Block), AppError> {
        content_unit_reached(ws, path.container_id()?, path.item_id(), permission, denial).await
    }

    fn answer(_: &App, item: Block) -> impl Future<Output = ApiResult> + Send {
        ready(Ok(json_response(&Unit::from(item))))
    }
}

impl CommentParentKind for UnitItem {
    const COMMENTS_DENIED: &'static str = Self::READ_DENIED;
    const CREATE_DENIED: &'static str = "access denied to comment on the unit";
    const CHANGES_DENIED: &'static str = "access denied to change the comments of the unit";
}

/// An entry of a diary: a user's own, which no token reaches ([`InDiary`]).
#[derive(Debug)]
pub(super) struct EntryItem;

impl ItemKind for EntryItem {
    type In = InDiary;
    const KINDS: ContainerKinds = ContainerKinds::DIARIES;
    const CONTAINER_PARAM: &'static str = "diaryId";
    const ITEM_PARAM: &'static str = "entryId";
    const WHAT: &'static str = "entry";
    const READ_DENIED: &'static str = super::diaries::READ_DENIED;
    const KEEP_DENIED: &'static str = "access denied to keep a version of the entry's body";
    const RESTORE_DENIED: &'static str = "access denied to restore a version of the entry's body";

    async fn reach(
        ws: &InWorkspace,
        path: &ItemPath,
        permission: ContainerPermission,
        denial: &str,
    ) -> Result<(Reached, Block), AppError> {
        entry_reached(ws, path.container_id()?, path.item_id(), permission, denial).await
    }

    fn answer(_: &App, item: Block) -> impl Future<Output = ApiResult> + Send {
        ready(Ok(json_response(&DiaryEntry::from(item))))
    }
}
