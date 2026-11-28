//! The views of a board and the attachments of its cards, written for who reached the
//! board ([`Reached`]): each write checks what it takes and that the block it touches is
//! of that board and of its type.

use fvoci_model::{Block, BlockPatch, BlockType};

use super::{App, ChangeNotification, Reached};
use crate::error::AppError;
use crate::permissions::ContainerPermission;

/// What who may not edit the board is refused changing its views with.
pub const VIEWS_DENIED: &str = "access denied to change board views";
/// What who may not change the board's properties is refused making a view with.
pub const CREATE_VIEW_DENIED: &str = "access denied to create board view";
/// What who may not change the board's properties is refused moving a view with.
pub const MOVE_VIEW_DENIED: &str = "access denied to move board view";
/// What who may not change the board's properties is refused copying a view with.
pub const DUPLICATE_VIEW_DENIED: &str = "access denied to duplicate board view";
/// What who may not edit the board is refused attaching a file to a card with.
pub const ATTACH_DENIED: &str = "access denied to attach file to card";
/// What who may not edit the board is refused changing the attachments of a card with.
pub const ATTACHMENTS_DENIED: &str = "access denied to change card attachments";

impl App {
    /// Changes `view`, a view of the board `on` reached that the request found by its
    /// identifier ([`App::block_of_board`]), as `patch` says, for the board's editors.
    ///
    /// # Errors
    ///
    /// Returns [`VIEWS_DENIED`] as [`Reached::require`] does, not found for a block that
    /// is no view of the board, and what `App::patch_block` returns.
    pub async fn patch_view(
        &self,
        on: &Reached,
        view: &Block,
        patch: &BlockPatch,
        notification: ChangeNotification,
    ) -> Result<Block, AppError> {
        on.require(ContainerPermission::ManageCards, VIEWS_DENIED)?;
        if !on.holds(view) || view.block_type != BlockType::View {
            return Err(AppError::not_found(BlockType::View.as_str()));
        }
        self.patch_block(&view.id, patch, on.user_id(), notification)
            .await
    }

    /// Deletes the view `view_id` of the board `on` reached, for the board's editors.
    ///
    /// # Errors
    ///
    /// Returns [`VIEWS_DENIED`] as [`Reached::require`] does, not found for a block that
    /// is no view of the board, and what `App::delete_block` returns.
    pub async fn delete_view(
        &self,
        on: &Reached,
        view_id: &str,
        notification: ChangeNotification,
    ) -> Result<(), AppError> {
        on.require(ContainerPermission::ManageCards, VIEWS_DENIED)?;
        self.block_of_board(on.container_id(), view_id, BlockType::View)
            .await?;
        self.delete_block(view_id, on.user_id(), notification).await
    }

    /// Brings the view `view_id` of the board `on` reached back from the trash, for the
    /// board's editors.
    ///
    /// # Errors
    ///
    /// Returns [`VIEWS_DENIED`] as [`Reached::require`] does, and what
    /// `App::undelete_block_of_board` returns.
    pub async fn restore_view(&self, on: &Reached, view_id: &str) -> Result<Block, AppError> {
        on.require(ContainerPermission::ManageCards, VIEWS_DENIED)?;
        let (board_id, user_id) = (on.container_id(), on.user_id());
        self.undelete_block_of_board(board_id, view_id, BlockType::View, None, user_id)
            .await
    }

    /// Deletes the attachment `attachment_id` of the card `card_id` of the board `on`
    /// reached, for the board's editors.
    ///
    /// # Errors
    ///
    /// Returns [`ATTACHMENTS_DENIED`] as [`Reached::require`] does, not found for a card
    /// that is not of the board or an attachment that is not of the card, and what
    /// `App::delete_block` returns.
    pub async fn delete_card_attachment(
        &self,
        on: &Reached,
        card_id: &str,
        attachment_id: &str,
        notification: ChangeNotification,
    ) -> Result<(), AppError> {
        on.require(ContainerPermission::ManageCards, ATTACHMENTS_DENIED)?;
        let board_id = on.container_id();
        self.block_of_board(board_id, card_id, BlockType::Card)
            .await?;
        let attachment = self
            .block_of_board(board_id, attachment_id, BlockType::Attachment)
            .await?;
        if attachment.parent_id != card_id {
            return Err(AppError::not_found(BlockType::Attachment.as_str()));
        }
        self.delete_block(attachment_id, on.user_id(), notification)
            .await
    }

    /// Brings the attachment `attachment_id` of the card `card_id` of the board `on`
    /// reached back from the trash, where it went on its own, for the board's editors.
    ///
    /// # Errors
    ///
    /// Returns [`ATTACHMENTS_DENIED`] as [`Reached::require`] does, not found for a card
    /// that is not of the board, and what `App::undelete_block_of_board` returns.
    pub async fn restore_card_attachment(
        &self,
        on: &Reached,
        card_id: &str,
        attachment_id: &str,
    ) -> Result<Block, AppError> {
        on.require(ContainerPermission::ManageCards, ATTACHMENTS_DENIED)?;
        let board_id = on.container_id();
        self.block_of_board(board_id, card_id, BlockType::Card)
            .await?;
        let kind = BlockType::Attachment;
        self.undelete_block_of_board(board_id, attachment_id, kind, Some(card_id), on.user_id())
            .await
    }
}
