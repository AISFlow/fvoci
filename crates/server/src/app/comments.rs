//! The comments on a card, a page or a unit, written for who reached its container
//! ([`Reached`]): each write asks the permission it takes there itself, so that nothing
//! writes a comment without having read who asks.

use fvoci_model::{Block, BlockPatch, BlockType, Body, BodyChange, new_id};

use super::{App, BlockAction, ChangeNotification, Reached};
use crate::error::AppError;
use crate::permissions::ContainerPermission;

/// Whether `block` is a comment directly on `parent` within the same container.
#[must_use]
pub fn is_comment_on(block: &Block, parent: &Block) -> bool {
    block.block_type == BlockType::Comment
        && block.container_id == parent.container_id
        && block.parent_id == parent.id
}

impl App {
    /// Writes a comment saying `text` on `parent`, a live block of the container `on`
    /// reached, for who may comment there.
    ///
    /// # Errors
    ///
    /// Returns `denial` as [`Reached::require`] does, not found `parent_what` for a parent
    /// that is not of the container or is gone by the time the comment is written, a bad
    /// request for a text that names a file the container does not have, and what
    /// `App::insert_block` returns.
    pub async fn create_comment(
        &self,
        on: &Reached,
        parent: &Block,
        text: Body,
        (parent_what, denial): (&str, &str),
        notification: ChangeNotification,
    ) -> Result<Block, AppError> {
        on.require(ContainerPermission::CommentCards, denial)?;
        if !on.holds(parent) {
            return Err(AppError::not_found(parent_what));
        }
        self.check_body_files(on.container_id(), Some(&text))
            .await?;
        // Signed by the agent that writes it, if one does, as the store writes it.
        let mut comment = Block {
            id: new_id(),
            container_id: on.container_id().to_owned(),
            parent_id: parent.id.clone(),
            block_type: BlockType::Comment,
            body: Some(text),
            ..Block::default()
        };
        if let Err(error) = self
            .insert_block(&mut comment, on.user_id(), notification)
            .await
        {
            return Err(match self.block(&comment.parent_id).await {
                Err(gone) if gone.is_not_found() => AppError::not_found(parent_what),
                _ => error,
            });
        }
        Ok(comment)
    }

    /// Changes `comment`, a live comment of the container `on` reached, to say `text`,
    /// as it was when it was read at `read_at`: its author's from the commenter's role
    /// up, and somebody else's the admins' only ([`Reached::require_on_comment`]).
    ///
    /// # Errors
    ///
    /// Returns the refusal of [`Reached::require_on_comment`] (`denial` for the author),
    /// not found for a block that is no comment of the container, a bad request for a
    /// text that names a file the container does not have, and what
    /// `App::patch_block` returns.
    pub async fn patch_comment(
        &self,
        on: &Reached,
        comment: &Block,
        (text, read_at): (Body, i64),
        denial: &str,
        notification: ChangeNotification,
    ) -> Result<Block, AppError> {
        on.require_on_comment(comment, BlockAction::Change, denial)?;
        if !on.holds(comment) || comment.block_type != BlockType::Comment {
            return Err(AppError::not_found("comment"));
        }
        self.check_body_files(on.container_id(), Some(&text))
            .await?;
        let patch = BlockPatch {
            body: BodyChange::Set(text),
            read_at: Some(read_at),
            ..BlockPatch::default()
        };
        self.patch_block(&comment.id, &patch, on.user_id(), notification)
            .await
    }

    /// Deletes `comment`, a live comment of the container `on` reached, with the
    /// replies below it, for who may delete each of them
    /// ([`Reached::require_on_deletion`]).
    ///
    /// # Errors
    ///
    /// Returns the first refusal, not found for a block that is no comment of the
    /// container, and what `App::delete_block` returns.
    pub async fn delete_comment(
        &self,
        on: &Reached,
        comment: &Block,
        denial: &str,
        notification: ChangeNotification,
    ) -> Result<(), AppError> {
        if !on.holds(comment) || comment.block_type != BlockType::Comment {
            return Err(AppError::not_found("comment"));
        }
        let below = self.blocks_below(&comment.id).await?;
        on.require_on_deletion(comment, &below, BlockAction::Delete, denial)?;
        self.delete_block(&comment.id, on.user_id(), notification)
            .await
    }

    /// Brings the comment `comment_id` on `parent`, a live block of the container `on`
    /// reached, back from the trash, where it went on its own, with the replies that
    /// went with it, for who may bring each of them back
    /// ([`Reached::require_on_deletion`]).
    ///
    /// # Errors
    ///
    /// Returns `denial` for who may not comment on the container, before the trash is
    /// looked in; not found unless the trash has that comment on `parent` at the top of
    /// a deletion; the first refusal of what goes back; and what
    /// `App::undelete_block` returns.
    pub async fn restore_comment(
        &self,
        on: &Reached,
        parent: &Block,
        comment_id: &str,
        denial: &str,
    ) -> Result<Block, AppError> {
        // Asked first, as every write of a comment asks: what is in the trash is not
        // told apart for who may not comment there.
        on.require(ContainerPermission::CommentCards, denial)?;
        let deletion = self.deletion(comment_id).await?;
        let Some((top, below)) = deletion
            .split_first()
            .filter(|(top, _)| top.id == comment_id && is_comment_on(top, parent))
            .filter(|(top, _)| on.holds(top))
        else {
            return Err(AppError::not_found("comment"));
        };
        let deleted_by = self.deleted_by(&top.id).await?;
        let action = BlockAction::Undelete {
            deleted_by: &deleted_by,
        };
        on.require_on_deletion(top, below, action, denial)?;
        self.undelete_block(comment_id, on.user_id()).await
    }
}
