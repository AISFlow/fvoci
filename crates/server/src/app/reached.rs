//! A container as one caller reached it, and the checks of what they may do there.
//!
//! A route that names a container asks [`App::reach`] once and checks what it is
//! asked by the [`Reached`] it gives: the facts of the permission are read once per
//! request, and what a refusal says (403 or 404, and in what words) is said here
//! alone. The writes of a container's content take the [`Reached`] and check it
//! themselves, so that nothing writes there without having read who asks.

use fvoci_model::{Block, BlockType, Container};

use super::{App, ContainerAccess};
use crate::error::AppError;
use crate::permissions::{self, CommentAction, ContainerPermission, Grant};

/// A container as one caller reached it through a handle: the row and what they may do
/// there, read once ([`App::container_access`]).
///
/// Made only by [`App::reach`], so that holding one is having read it. A container that
/// is not there for the caller is reached too, with nothing granted: the check that
/// refuses says what is missing, the container or what the request named through it.
#[derive(Debug, Clone)]
pub struct Reached {
    /// Who reached it.
    user_id: String,
    /// The identifier the request named it by.
    container_id: String,
    /// The container and what the caller may do there; `None` if it is not there for
    /// them.
    access: Option<ContainerAccess>,
}

impl App {
    /// The container `container_id` as `user_id` reaches it through this handle: the
    /// row and what they may do there, or nothing granted if it is not there for them
    /// (of another workspace than the handle's, of a kind it does not reach, or none at
    /// all). The same facts are read whether it is there or not
    /// ([`App::container_access`]).
    ///
    /// # Errors
    ///
    /// Returns the store's error if a fact cannot be read.
    pub async fn reach(&self, user_id: &str, container_id: &str) -> Result<Reached, AppError> {
        let access = self.container_access(user_id, container_id).await?;
        Ok(Reached {
            user_id: user_id.to_owned(),
            container_id: container_id.to_owned(),
            access,
        })
    }
}

impl Reached {
    /// Who reached the container.
    #[must_use]
    pub fn user_id(&self) -> &str {
        &self.user_id
    }

    /// The identifier the request named the container by.
    #[must_use]
    pub fn container_id(&self) -> &str {
        &self.container_id
    }

    /// The container as it is stored; `None` if it is not there for the caller.
    #[must_use]
    pub fn container(&self) -> Option<&Container> {
        self.access.as_ref().map(|access| &access.container)
    }

    /// The container and what the caller is and may do there; `None` if it is not
    /// there for them.
    #[must_use]
    pub fn access(&self) -> Option<&ContainerAccess> {
        self.access.as_ref()
    }

    /// Whether `block` is of the container: what a write of a block that a request found
    /// by its identifier asks, so that the container reached is the one written to.
    #[must_use]
    pub fn holds(&self, block: &Block) -> bool {
        block.container_id == self.container_id
    }

    /// What the caller may do on the container: nothing if it is not there for them.
    pub fn grant(&self) -> Grant {
        self.access
            .as_ref()
            .map_or(Grant::NONE, |access| access.grant)
    }

    /// Checks that the caller has `permission` on the container the request names, and
    /// gives the container they have it on.
    ///
    /// # Errors
    ///
    /// Returns 403 (`denial`) for who sees the container ([`Grant::sees`]); for anybody
    /// else, and for the content of a container the caller only manages
    /// ([`Grant::hides`]), not found: the board is not there.
    pub fn require(
        &self,
        permission: ContainerPermission,
        denial: &str,
    ) -> Result<&Container, AppError> {
        require_granted(self.grant(), permission, denial)?;
        // Granted something, the caller has the container: nothing is granted on none.
        self.container().ok_or_else(|| AppError::not_found("board"))
    }

    /// Checks that the caller has `permission` on a container the request does not name
    /// itself but reaches through `what`, such as a card of the board, and gives the
    /// container.
    ///
    /// # Errors
    ///
    /// Returns not found `what` for a caller who may not read the container's contents:
    /// the identifier in the request says nothing about a board, so not even an open
    /// board is given away. A reader without the permission is refused (403, `denial`).
    pub fn require_through(
        &self,
        permission: ContainerPermission,
        denial: &str,
        what: &str,
    ) -> Result<&Container, AppError> {
        let grant = self.grant();
        if grant.allows(permission) {
            self.container().ok_or_else(|| AppError::not_found(what))
        } else if grant.reads() {
            Err(AppError::forbidden(denial))
        } else {
            Err(AppError::not_found(what))
        }
    }

    /// Checks that the caller sees the container at all ([`Grant::sees`]).
    ///
    /// # Errors
    ///
    /// Returns not found (the board) for anybody else.
    pub fn require_visible(&self) -> Result<(), AppError> {
        if self.grant().sees() {
            Ok(())
        } else {
            Err(AppError::not_found("board"))
        }
    }

    /// What the caller is told who may not do what they ask of the container, if it is
    /// not its content: refused (403, `denial`) if they see it, not found if they do not.
    #[must_use]
    pub fn refusal(&self, denial: &str) -> AppError {
        refusal(self.grant(), denial)
    }

    /// Checks that the caller may do `action` to `comment`, a comment on the container
    /// ([`permissions::comment_permission`]): their own from the commenter's role up,
    /// somebody else's the admins' only.
    ///
    /// # Errors
    ///
    /// Returns what [`Self::require`] returns, in `denial` for the author and in the
    /// action's own words for anybody else.
    pub fn require_on_comment(
        &self,
        comment: &Block,
        action: BlockAction<'_>,
        denial: &str,
    ) -> Result<(), AppError> {
        let (permission, denial) = comment_permission(&self.user_id, comment, action, denial);
        require_granted(self.grant(), permission, denial)
    }

    /// Checks that the caller may do `action` to `block` of the container: a comment as
    /// [`Self::require_on_comment`] says, and any other block for its editors.
    ///
    /// # Errors
    ///
    /// Returns what [`Self::require`] returns.
    pub fn require_on_block(
        &self,
        block: &Block,
        action: BlockAction<'_>,
        denial: &str,
    ) -> Result<(), AppError> {
        match block.block_type {
            BlockType::Comment => self.require_on_comment(block, action, denial),
            _ => require_granted(self.grant(), ContainerPermission::ManageCards, denial),
        }
    }

    /// Checks that the caller may delete `top` and what goes with it, or bring them back
    /// (`action`): `below` are the blocks it takes along. The top decides for a card, or
    /// any block that is no comment: the comments on a card go with it. A comment takes
    /// the replies below it, so who deletes it, or brings it back, must be allowed that
    /// for each of them too: nobody takes away the comment of somebody else by deleting
    /// their own. The top's permission is asked first. For a comment this is a guard:
    /// the store keeps anything from being below one.
    ///
    /// # Errors
    ///
    /// Returns the first refusal, as [`Self::require_on_block`] says it.
    pub fn require_on_deletion(
        &self,
        top: &Block,
        below: &[Block],
        action: BlockAction<'_>,
        denial: &str,
    ) -> Result<(), AppError> {
        let replies = if top.block_type == BlockType::Comment {
            below
        } else {
            &[]
        };
        for block in std::iter::once(top).chain(replies) {
            self.require_on_block(block, action, denial)?;
        }
        Ok(())
    }
}

/// Checks that `grant`, on the board a request names, has the permission: refused
/// (403, `denial`) if the user sees the board, not found (404) if not, and not found
/// for the content of a board the user only manages (`Grant::hides`).
///
/// # Errors
///
/// Returns the refusal.
pub(crate) fn require_granted(
    grant: Grant,
    permission: ContainerPermission,
    denial: &str,
) -> Result<(), AppError> {
    if grant.allows(permission) {
        Ok(())
    } else if grant.hides(permission) {
        Err(AppError::not_found("board"))
    } else {
        Err(AppError::forbidden(denial))
    }
}

/// What a user is told who may not do what they ask on the board named in the path, if
/// it is not its content: refused (403, `denial`) if they see the board, not found
/// (404) if they do not.
pub(crate) fn refusal(grant: Grant, denial: &str) -> AppError {
    if grant.sees() {
        AppError::forbidden(denial)
    } else {
        AppError::not_found("board")
    }
}

/// The refusal of a comment to its author who did not delete it.
const SOMEBODY_ELSE_DELETED: &str =
    "a comment somebody else deleted is brought back by the board's admins";

/// What a user does to a block, which a comment of somebody else lets only the board's
/// admins do, each refused in its own words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockAction<'a> {
    /// Change it (`patchCardComment`, `patchPageComment`, `patchUnitComment`).
    Change,
    /// Delete it (`deleteCardComment`, `deletePageComment`, `deleteUnitComment`).
    Delete,
    /// Bring it back from the trash (`restoreCardComment`, `restorePageComment`,
    /// `restoreUnitComment`), where the deletion of `deleted_by` put it.
    Undelete {
        /// Who deleted it; empty if nobody did.
        deleted_by: &'a str,
    },
}

impl<'a> BlockAction<'a> {
    /// The refusal of this to a comment of somebody else.
    fn others_comments_denial(self) -> &'static str {
        match self {
            Self::Change => "access denied to change other users' comments",
            Self::Delete => "access denied to delete other users' comments",
            Self::Undelete { .. } => "access denied to undelete other users' comments",
        }
    }

    /// What it is to a comment.
    fn on_comment(self) -> CommentAction<'a> {
        match self {
            Self::Change => CommentAction::Change,
            Self::Delete => CommentAction::Delete,
            Self::Undelete { deleted_by } => CommentAction::Undelete { deleted_by },
        }
    }
}

/// The permission that doing `action` to a comment needs
/// ([`permissions::comment_permission`]), and the refusal without it: `denial` for the
/// author, the action's own words for anybody else, and for the author of a comment
/// somebody else deleted that it is the board's admins' to bring back.
fn comment_permission<'a>(
    user_id: &str,
    block: &Block,
    action: BlockAction<'_>,
    denial: &'a str,
) -> (ContainerPermission, &'a str) {
    let permission =
        permissions::comment_permission(user_id, &block.created_by, action.on_comment());
    match action {
        _ if permission == ContainerPermission::CommentCards => (permission, denial),
        BlockAction::Undelete { deleted_by }
            if block.created_by == user_id && !deleted_by.is_empty() =>
        {
            (permission, SOMEBODY_ELSE_DELETED)
        }
        _ => (permission, action.others_comments_denial()),
    }
}
