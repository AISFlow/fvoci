//! Blocks: reading and changing them, and telling websocket listeners and webhooks.

use fvoci_model::{Block, BlockPatch, BlockPatchBatch, BlockType, CommentVersion};
use fvoci_store::{BlockFilter, ByAgent, DeletedBlock};

use super::App;
use super::changes::Changes;
use crate::collab::{Deletion, OverEditors};
use crate::error::{AppError, OrMissing};

/// Whether the people a change to a block newly names (those its body mentions, those
/// a card is given to) are told of it. A client that writes again what was said before
/// (a copy, a change taken back or made again, via `disableNotify`) suppresses it while
/// its writer watches what it writes new all the same ([`Self::Suppress`]).
///
/// Websocket broadcasts and webhooks are not notifications: they happen either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChangeNotification {
    /// Report the change.
    #[default]
    Send,
    /// Do not report the change (`disableNotify`): tell nobody, while the writer still
    /// watches newly written cards, pages or comments.
    Suppress,
}

/// What happened to a block, as the notification service names it.
#[derive(Debug, Clone, Copy)]
pub(super) enum ChangeAction {
    Add,
    Update,
    Delete,
}

impl ChangeAction {
    fn as_str(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Update => "update",
            Self::Delete => "delete",
        }
    }
}

impl App {
    /// The database for a write of blocks that tells of itself as `notification`
    /// says: one that is suppressed tells nobody whom it newly names. A handle an
    /// agent writes through ([`App::writing_for`]) tells of the write as the agent's.
    pub(super) fn store_telling(&self, notification: ChangeNotification) -> fvoci_store::Store {
        let store = match &self.reach.agent {
            Some(delegation) => self.store().by_agent(delegation.by_agent()),
            None => self.store().clone(),
        };
        match notification {
            ChangeNotification::Send => store,
            ChangeNotification::Suppress => store.quietly(),
        }
    }

    /// Returns the blocks of a board, optionally only those with the given parent or of
    /// the given type, in no particular order.
    ///
    /// An empty board identifier finds nothing.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the query fails.
    pub async fn blocks(
        &self,
        container_id: &str,
        parent_id: Option<&str>,
        block_type: Option<&BlockType>,
    ) -> Result<Vec<Block>, AppError> {
        if container_id.is_empty() {
            return Ok(Vec::new());
        }
        let mut filter = BlockFilter::container(container_id);
        if let Some(parent_id) = parent_id {
            filter = filter.with_parent(parent_id);
        }
        if let Some(&block_type) = block_type {
            filter = filter.with_type(block_type);
        }
        Ok(self.store().get_blocks(&filter).await?)
    }

    /// Returns all blocks of a board, in no particular order.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the query fails.
    pub async fn blocks_for_container(&self, container_id: &str) -> Result<Vec<Block>, AppError> {
        Ok(self
            .store()
            .get_blocks(&BlockFilter::container(container_id))
            .await?)
    }

    /// Returns the block with the given identifier: only a block of a board of the kinds
    /// the handle reaches ([`App::reaching`]) and, for a handle of one workspace
    /// ([`App::in_workspace`]), of that workspace or of a built-in template.
    ///
    /// # Errors
    ///
    /// Returns a not-found error if there is no such block.
    pub async fn block(&self, block_id: &str) -> Result<Block, AppError> {
        // The handle finds the blocks of the boards it reaches only, in the one read.
        let found = self
            .store()
            .get_block_within(block_id, self.reach.workspace.as_deref(), self.reach.kinds)
            .await;
        found.or_missing("block")
    }

    /// Returns the block of the board of this type, as [`App::block`] reaches it: a
    /// block of another board, or of another type, is as missing as one that does not
    /// exist, since the path names the board and what it holds.
    ///
    /// # Errors
    ///
    /// Returns a not-found error naming the type if the board has no such block.
    pub async fn block_of_board(
        &self,
        board_id: &str,
        block_id: &str,
        block_type: BlockType,
    ) -> Result<Block, AppError> {
        let block = self.block(block_id).await.or_missing(block_type.as_str())?;
        if block.container_id == board_id && block.block_type == block_type {
            Ok(block)
        } else {
            Err(AppError::not_found(block_type.as_str()))
        }
    }

    /// Brings a block of the board of this type back from the trash, as
    /// [`App::undelete_block`] does, where it went on its own: a block that went there
    /// with another (an attachment with its card) comes back with that one. With
    /// `parent_id`, only a block that was below that block.
    ///
    /// # Errors
    ///
    /// Returns a not-found error naming the type unless the trash has such a deletion of
    /// the block, and the errors of [`App::undelete_block`].
    pub(super) async fn undelete_block_of_board(
        &self,
        board_id: &str,
        block_id: &str,
        block_type: BlockType,
        parent_id: Option<&str>,
        user_id: &str,
    ) -> Result<Block, AppError> {
        let deletion = self.deletion(block_id).await?;
        let deleted_alone = deletion.first().is_some_and(|top| {
            top.id == block_id
                && top.container_id == board_id
                && top.block_type == block_type
                && parent_id.is_none_or(|parent_id| top.parent_id == parent_id)
        });
        if !deleted_alone {
            return Err(AppError::not_found(block_type.as_str()));
        }
        self.undelete_block(block_id, user_id).await
    }

    /// Coordinates a block write with active collaborative rooms: when
    /// `whole_body_cards` is empty, runs under structure barriers on `containers`;
    /// otherwise runs via [`crate::collab::Collab::write_whole`].
    pub(crate) async fn write_blocks<T, F, Fut>(
        &self,
        containers: Vec<String>,
        whole_body_cards: Vec<(String, String)>,
        write: F,
    ) -> Result<T, AppError>
    where
        T: Send + 'static,
        F: FnOnce(Self) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, AppError>> + Send + 'static,
    {
        if whole_body_cards.is_empty() {
            let _structure = self
                .collab()
                .changing_structures(self, containers.iter().map(String::as_str))
                .await;
            write(self.clone()).await
        } else {
            self.collab()
                .write_whole(self, whole_body_cards, write)
                .await
        }
    }

    /// Inserts a block, or updates the block that already has its identifier, and tells
    /// websocket listeners and webhooks in the background.
    ///
    /// `block` is updated as [`fvoci_store::Store::insert_block`] describes.
    ///
    /// # Errors
    ///
    /// Returns a not-found error if the block's board does not exist and
    /// a bad request if the title or the fields are too large.
    pub(super) async fn insert_block(
        &self,
        block: &mut Block,
        modified_by: &str,
        notification: ChangeNotification,
    ) -> Result<(), AppError> {
        let container = self.store().get_container(&block.container_id).await?;
        // A card stored again writes its body whole, around its document.
        let whole_body_cards = if block.block_type.has_room() {
            vec![(block.id.clone(), block.container_id.clone())]
        } else {
            Vec::new()
        };
        let (mut stored, modified_by) = (block.clone(), modified_by.to_owned());
        *block = self
            .write_blocks(
                vec![block.container_id.clone()],
                whole_body_cards,
                async move |app| {
                    app.store_telling(notification)
                        .insert_block(&mut stored, &modified_by)
                        .await?;
                    Ok(stored)
                },
            )
            .await?;
        log_notification(notification, ChangeAction::Add, &block.id);
        self.tell(Changes::in_workspace(&container.workspace_id).changed([block.clone()]));
        Ok(())
    }

    /// Inserts several blocks of one board as [`App::insert_block`] does, telling
    /// websocket listeners about each right after it is stored and webhooks in the
    /// background. No blocks are no error.
    ///
    /// The blocks are stored atomically (all or none).
    ///
    /// # Errors
    ///
    /// Returns a bad request if the blocks belong to more than one board or a title or
    /// the fields of a block are too large, and a not-found error if the board does not
    /// exist.
    pub(super) async fn insert_blocks(
        &self,
        blocks: &mut [Block],
        modified_by: &str,
        notification: ChangeNotification,
    ) -> Result<(), AppError> {
        let Some(first) = blocks.first() else {
            return Ok(());
        };
        let container_id = first.container_id.clone();
        if blocks
            .iter()
            .any(|block| block.container_id != container_id)
        {
            return Err(AppError::bad_request(
                "the blocks belong to more than one board",
            ));
        }
        let container = self.store().get_container(&container_id).await?;

        // Store all blocks atomically so a failure leaves none persisted. Cards stored
        // again write their bodies whole, around their documents.
        let cards: Vec<(String, String)> = blocks
            .iter()
            .filter(|block| block.block_type.has_room())
            .map(|block| (block.id.clone(), container_id.clone()))
            .collect();
        let (mut stored, modified_by) = (blocks.to_vec(), modified_by.to_owned());
        let stored = self
            .write_blocks(vec![container_id], cards, async move |app| {
                app.store_telling(notification)
                    .insert_blocks(&mut stored, &modified_by)
                    .await?;
                Ok(stored)
            })
            .await?;
        blocks.clone_from_slice(&stored);
        // Told before the answer: the client of the write has heard of every block then.
        self.tell_now(Changes::in_workspace(&container.workspace_id).changed(stored))
            .await;
        for block in blocks.iter() {
            log_notification(notification, ChangeAction::Add, &block.id);
        }
        Ok(())
    }

    /// Applies a patch to a block and returns the patched block; websocket listeners
    /// and webhooks are told in the background.
    ///
    /// # Errors
    ///
    /// Returns a not-found error if the block or its board does not exist and
    /// a bad request if the patched title or fields are too large.
    pub(super) async fn patch_block(
        &self,
        block_id: &str,
        patch: &BlockPatch,
        modified_by: &str,
        notification: ChangeNotification,
    ) -> Result<Block, AppError> {
        self.patch_block_as(block_id, patch, modified_by, notification, None)
            .await
    }

    /// As [`App::patch_block`], by `modified_by` themselves, or by an agent through a
    /// personal API token of theirs (`agent`). An agent's body written whole is refused
    /// while people may write in it, as it would lose what they typed and have not
    /// sent, and is kept as the agent's version after the body before.
    ///
    /// # Errors
    ///
    /// As [`App::patch_block`], and 409 `room_has_active_editors` for an agent.
    pub(super) async fn patch_block_as(
        &self,
        block_id: &str,
        patch: &BlockPatch,
        modified_by: &str,
        notification: ChangeNotification,
        agent: Option<ByAgent<'_>>,
    ) -> Result<Block, AppError> {
        let old_block = self.store().get_block(block_id).await?;
        let container = self.store().get_container(&old_block.container_id).await?;
        // A whole body is written around the document of a body edited together.
        let whole_body_cards = if patch.body.is_keep() || !old_block.block_type.has_room() {
            Vec::new()
        } else {
            vec![(block_id.to_owned(), old_block.container_id.clone())]
        };
        let over = match agent {
            Some(_) => OverEditors::Refuse,
            None => OverEditors::Replace,
        };
        let agent = agent.map(|agent| {
            (
                agent.token_id.to_owned(),
                agent.token_name.to_owned(),
                agent.notify_self,
            )
        });
        let (block_id, patch, modified_by) =
            (block_id.to_owned(), patch.clone(), modified_by.to_owned());
        let write = async move |app: Self| {
            let store = app.store_telling(notification);
            Ok(match &agent {
                Some((token_id, token_name, notify_self)) => {
                    let agent = ByAgent {
                        token_id,
                        token_name,
                        notify_self: *notify_self,
                    };
                    store
                        .patch_block_by_agent(&block_id, &patch, &modified_by, agent)
                        .await?
                }
                None => store.patch_block(&block_id, &patch, &modified_by).await?,
            })
        };
        let block = if whole_body_cards.is_empty() {
            self.write_blocks(
                vec![old_block.container_id.clone()],
                whole_body_cards,
                write,
            )
            .await?
        } else {
            self.collab()
                .write_whole_over(self, whole_body_cards, over, write)
                .await?
        };
        log_notification(notification, ChangeAction::Update, &block.id);
        // The next card completing it made, told after it.
        let made = cards_made_after(
            std::slice::from_ref(&old_block),
            std::slice::from_ref(&block),
        );
        self.tell(
            Changes::in_workspace(&container.workspace_id)
                .changed([block.clone()])
                .reread(made),
        );
        Ok(block)
    }

    /// Applies each patch of a batch to the block at the same position, all or none;
    /// websocket listeners in `workspace_id` and webhooks are told in the background.
    ///
    /// The blocks may belong to different boards. The members of each block's board who
    /// listen to `workspace_id` are told, so a workspace other than the boards' own reaches only
    /// the listeners of the blocks themselves.
    ///
    /// # Errors
    ///
    /// Returns a not-found error unless every identifier names exactly one block
    /// (an identifier listed twice counts as missing), a bad request if a
    /// patched title or the patched fields are too large, and an internal error if
    /// there are fewer patches than identifiers.
    pub(super) async fn patch_blocks(
        &self,
        workspace_id: &str,
        batch: &BlockPatchBatch,
        modified_by: &str,
        notification: ChangeNotification,
    ) -> Result<(), AppError> {
        // Verify that every referenced block exists before writing patches.
        let blocks = self.store().get_blocks_by_ids(&batch.block_ids).await?;
        // Whole bodies are written around the documents of bodies edited together.
        let card_ids =
            crate::collab::cards_written_whole(&blocks, &batch.block_ids, &batch.block_patches);
        let containers: Vec<String> = blocks
            .iter()
            .map(|block| block.container_id.clone())
            .collect();
        let (owned_batch, modified_by) = (batch.clone(), modified_by.to_owned());
        self.write_blocks(containers, card_ids, async move |app| {
            Ok(app
                .store_telling(notification)
                .patch_blocks(&owned_batch, &modified_by)
                .await?)
        })
        .await?;
        for block_id in &batch.block_ids {
            log_notification(notification, ChangeAction::Update, block_id);
        }
        // Read again as they are told: one deleted meanwhile is not told of.
        let told = Changes::in_workspace(workspace_id).reread(batch.block_ids.clone());
        self.tell_with_next_cards(workspace_id, told, blocks);
        Ok(())
    }

    /// Tells of `told` in the background and then, where cards of `before` repeat, of
    /// the next cards that completing them made: after the cards they follow, read as
    /// they are told.
    fn tell_with_next_cards(&self, workspace_id: &str, told: Changes, before: Vec<Block>) {
        let repeating: Vec<String> = before
            .iter()
            .filter(|block| block.fields.contains_key(fvoci_model::RECURRENCE_FIELD))
            .map(|block| block.id.clone())
            .collect();
        if repeating.is_empty() {
            self.tell(told);
            return;
        }
        let (app, workspace_id) = (self.clone(), workspace_id.to_owned());
        self.spawn_background(async move {
            app.tell_now(told).await;
            let mut after = Vec::with_capacity(repeating.len());
            for block_id in &repeating {
                match app.store().get_block(block_id).await {
                    Ok(block) => after.push(block),
                    // Deleted meanwhile: its next card, if it made one, is not told of.
                    Err(error) if error.is_not_found() => {}
                    Err(error) => {
                        tracing::error!(%error, block_id, "cannot read a completed card to tell of its next");
                        return;
                    }
                }
            }
            let made = cards_made_after(&before, &after);
            app.tell_now(Changes::in_workspace(&workspace_id).reread(made))
                .await;
        });
    }

    /// Deletes a block and everything below it; websocket listeners are told in the
    /// background. Webhooks are not told about deletions.
    ///
    /// # Errors
    ///
    /// Returns a not-found error if the block or its board does not exist.
    pub(super) async fn delete_block(
        &self,
        block_id: &str,
        modified_by: &str,
        notification: ChangeNotification,
    ) -> Result<(), AppError> {
        let block = self.store().get_block(block_id).await?;
        let container = self.store().get_container(&block.container_id).await?;
        let (removed, by) = (block_id.to_owned(), modified_by.to_owned());
        let deleted = self
            .collab()
            .remove(self, Deletion::Block(removed.clone()), async move |app| {
                Ok(app.store().delete_block_family(&removed, &by).await?)
            })
            .await?;
        log_notification(notification, ChangeAction::Delete, block_id);
        // The deleted block, and the cards below a card that went to the trash with it,
        // so that a board's clients drop the cards below it too.
        let child_cards: Vec<Block> = deleted
            .into_iter()
            .filter(|each| each.id != block.id && each.block_type == BlockType::Card)
            .collect();
        let told = std::iter::once(block).chain(child_cards);
        self.tell(Changes::in_workspace(&container.workspace_id).deleted(told));
        Ok(())
    }

    /// Returns a block that is in the trash, or `None` if no block of that identifier
    /// is there.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the trash cannot be read.
    pub async fn deleted_block(&self, block_id: &str) -> Result<Option<DeletedBlock>, AppError> {
        self.store().get_deleted_block(block_id).await.found()
    }

    /// Returns the blocks of the deletion a block of the trash went with, which restoring
    /// the block restores together: the one that was deleted first, then those it took
    /// along. Empty if no block of that identifier is there.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the trash cannot be read.
    pub async fn deletion(&self, block_id: &str) -> Result<Vec<Block>, AppError> {
        Ok(self
            .store()
            .get_deletion(block_id)
            .await
            .found()?
            .unwrap_or_default())
    }

    /// Returns who deleted a block of the trash: the user whose deletion put it there,
    /// empty if nobody's did or it is not there.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the trash cannot be read.
    pub async fn deleted_by(&self, block_id: &str) -> Result<String, AppError> {
        let deleted = self.store().get_deleted_block(block_id).await.found()?;
        Ok(deleted.map_or_default(|deleted| deleted.deleted_by))
    }

    /// Returns the blocks below a block, at any depth: what deleting it takes along.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the blocks cannot be read.
    pub async fn blocks_below(&self, block_id: &str) -> Result<Vec<Block>, AppError> {
        Ok(self.store().get_blocks_below(block_id).await?)
    }

    /// Returns the earlier versions of a comment's body, the first first.
    ///
    /// # Errors
    ///
    /// Returns the store's error if they cannot be read.
    pub async fn comment_versions(&self, block_id: &str) -> Result<Vec<CommentVersion>, AppError> {
        Ok(self.store().get_comment_versions(block_id).await?)
    }

    /// Restores a block from the trash, with everything that was deleted along with
    /// it, and returns the block; websocket listeners and webhooks are told in the
    /// background for every restored block. A top-level card in the deletion whose parent
    /// card is no longer live is re-rooted to the top level.
    ///
    /// # Errors
    ///
    /// Returns a not-found error if the block is not in the trash or has nothing to
    /// come back to: its board, or (for non-card blocks) the block it was on, does not
    /// exist. Returns a conflict error if a card cannot be restored under its live parent
    /// (for example, because the parent became a template or the subtree would exceed
    /// [`fvoci_model::MAX_CARD_DEPTH`]).
    pub(super) async fn undelete_block(
        &self,
        block_id: &str,
        modified_by: &str,
    ) -> Result<Block, AppError> {
        let Some(container_id) = self.store().find_any_block_container_id(block_id).await? else {
            return Err(AppError::not_found("block"));
        };
        let restored = {
            let _structure = self.collab().changing_structure(self, &container_id).await;
            self.store()
                .undelete_block_family(block_id, modified_by)
                .await
        };
        let restored = restored.or_missing("block")?;
        let block = restored
            .iter()
            .find(|each| each.id == block_id)
            .cloned()
            .ok_or_else(|| AppError::not_found("block"))?;
        let container = self.store().get_container(&block.container_id).await?;
        // An undeletion is always reported; there is no flag for it.
        log_notification(ChangeNotification::Send, ChangeAction::Add, block_id);
        // The block, and the cards that came back with it.
        let told = restored
            .into_iter()
            .filter(|each| each.id == block_id || each.block_type == BlockType::Card);
        self.tell(Changes::in_workspace(&container.workspace_id).changed(told));
        Ok(block)
    }
}

/// The raw writes of blocks, which ask nobody whether they may: for the fixtures of the
/// integration tests, which set the scene a request then meets. A route writes a block
/// through what it reached ([`super::Reached`]), never through these.
#[cfg(feature = "test-util")]
impl App {
    /// `App::insert_block`, which only the logic layer calls otherwise.
    ///
    /// # Errors
    ///
    /// What `App::insert_block` returns.
    #[doc(hidden)]
    pub async fn insert_block_unchecked(
        &self,
        block: &mut Block,
        modified_by: &str,
        notification: ChangeNotification,
    ) -> Result<(), AppError> {
        self.insert_block(block, modified_by, notification).await
    }

    /// `App::insert_blocks`, which only the logic layer calls otherwise.
    ///
    /// # Errors
    ///
    /// What `App::insert_blocks` returns.
    #[doc(hidden)]
    pub async fn insert_blocks_unchecked(
        &self,
        blocks: &mut [Block],
        modified_by: &str,
        notification: ChangeNotification,
    ) -> Result<(), AppError> {
        self.insert_blocks(blocks, modified_by, notification).await
    }

    /// `App::patch_block`, which only the logic layer calls otherwise.
    ///
    /// # Errors
    ///
    /// What `App::patch_block` returns.
    #[doc(hidden)]
    pub async fn patch_block_unchecked(
        &self,
        block_id: &str,
        patch: &BlockPatch,
        modified_by: &str,
        notification: ChangeNotification,
    ) -> Result<Block, AppError> {
        self.patch_block(block_id, patch, modified_by, notification)
            .await
    }

    /// `App::patch_blocks`, which only the logic layer calls otherwise.
    ///
    /// # Errors
    ///
    /// What `App::patch_blocks` returns.
    #[doc(hidden)]
    pub async fn patch_blocks_unchecked(
        &self,
        workspace_id: &str,
        batch: &BlockPatchBatch,
        modified_by: &str,
        notification: ChangeNotification,
    ) -> Result<(), AppError> {
        self.patch_blocks(workspace_id, batch, modified_by, notification)
            .await
    }

    /// `App::delete_block`, which only the logic layer calls otherwise.
    ///
    /// # Errors
    ///
    /// What `App::delete_block` returns.
    #[doc(hidden)]
    pub async fn delete_block_unchecked(
        &self,
        block_id: &str,
        modified_by: &str,
        notification: ChangeNotification,
    ) -> Result<(), AppError> {
        self.delete_block(block_id, modified_by, notification).await
    }

    /// `App::undelete_block`, which only the logic layer calls otherwise.
    ///
    /// # Errors
    ///
    /// What `App::undelete_block` returns.
    #[doc(hidden)]
    pub async fn undelete_block_unchecked(
        &self,
        block_id: &str,
        modified_by: &str,
    ) -> Result<Block, AppError> {
        self.undelete_block(block_id, modified_by).await
    }
}

/// Logs a debug notification line for a block change event when enabled.
pub(super) fn log_notification(
    notification: ChangeNotification,
    action: ChangeAction,
    block_id: &str,
) {
    if notification == ChangeNotification::Send {
        tracing::debug!(action = action.as_str(), block_id, "Block change event");
    }
}

/// The cards that changes made after the cards they completed: of each card of `after`
/// that is of `before` too, the next card it names (`recurredTo`) where it named
/// another, or none, before.
pub(super) fn cards_made_after(before: &[Block], after: &[Block]) -> Vec<String> {
    let next_of = |block: &Block| {
        block
            .fields
            .get(fvoci_model::RECURRED_TO_FIELD)
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    };
    after
        .iter()
        .filter_map(|card| {
            let next = next_of(card)?;
            let was = before.iter().find(|old| old.id == card.id)?;
            (next_of(was).as_deref() != Some(next.as_str())).then_some(next)
        })
        .collect()
}
