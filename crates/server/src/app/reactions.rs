//! Reactions to the comments, cards, pages and units of a container: adding one, taking
//! one off, reading them, and telling the container's members what they are now
//! (`reactionsChanged`). Nobody is notified of a reaction, and no webhook is told.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use fvoci_model::{Block, BlockType, Container, Emoji, REACTION_KINDS_PER_BLOCK, Reaction};
use fvoci_store::Reacted;

use super::{App, Reached};
use crate::error::{AppError, Reason};
use crate::permissions::ContainerPermission;

/// The types of the blocks that are answered with reactions.
const REACTED_TO: [BlockType; 4] = [
    BlockType::Comment,
    BlockType::Card,
    BlockType::Page,
    BlockType::Unit,
];

impl App {
    /// Adds the reaction `emoji` of the one who reached the container `on` to `block`,
    /// and tells the container's members if it was not there.
    ///
    /// # Errors
    ///
    /// Returns what [`Reached::require`] refuses one with who may not comment there
    /// (`denial`); 404 for a block that is not of the container or is not answered with
    /// reactions, and for one that is gone or a user no longer in its workspace; 400
    /// with the reason `too_many_reactions` if the block has as many other emoji as a
    /// block may; and the store's error.
    pub(crate) async fn add_reaction(
        &self,
        on: &Reached,
        block: &Block,
        emoji: &Emoji,
        denial: &str,
    ) -> Result<(), AppError> {
        let container = reacted_on(on, block, denial)?;
        match self
            .store()
            .add_reaction(&block.id, on.user_id(), emoji)
            .await?
        {
            Reacted::Added => self.announce_reactions(container, block),
            Reacted::AlreadyThere => {}
            Reacted::TooManyKinds => return Err(too_many_reactions()),
        }
        Ok(())
    }

    /// Takes the reaction `emoji` of the one who reached the container `on` off
    /// `block`, and tells the container's members if it was there.
    ///
    /// # Errors
    ///
    /// Returns what [`App::add_reaction`] refuses one with who may not react to the
    /// block, and the store's error.
    pub(crate) async fn remove_reaction(
        &self,
        on: &Reached,
        block: &Block,
        emoji: &Emoji,
        denial: &str,
    ) -> Result<(), AppError> {
        let container = reacted_on(on, block, denial)?;
        if self
            .store()
            .remove_reaction(&block.id, on.user_id(), emoji)
            .await?
        {
            self.announce_reactions(container, block);
        }
        Ok(())
    }

    /// The reactions to those of `block_ids` that have any, by block
    /// ([`fvoci_store::Store::reactions_of`]).
    ///
    /// # Errors
    ///
    /// Returns the store's error.
    pub(crate) async fn reactions_of(
        &self,
        block_ids: &[&str],
    ) -> Result<HashMap<String, Vec<Reaction>>, AppError> {
        Ok(self.store().reactions_of(block_ids).await?)
    }

    /// The reactions to `block_id`, the emoji it was first answered with first.
    ///
    /// # Errors
    ///
    /// Returns the store's error.
    pub(crate) async fn reactions_to(&self, block_id: &str) -> Result<Vec<Reaction>, AppError> {
        let mut reactions = self.reactions_of(&[block_id]).await?;
        Ok(reactions.remove(block_id).unwrap_or_default())
    }

    /// Tells the members of `container`, the block's, its reactions in the background,
    /// as they are when they are told: what was read last of the block is told last, so
    /// two changes made one after the other leave every listener with the second. The
    /// blocks take their turns apart ([`TellingTurns`]).
    fn announce_reactions(&self, container: &Container, block: &Block) {
        let app = self.clone();
        let workspace_id = container.workspace_id.clone();
        let (container_id, block_id) = (container.id.clone(), block.id.clone());
        self.spawn_background(async move {
            let turns = &app.inner.telling_reactions;
            let turn = turns.of(&block_id);
            {
                let _held = turn.lock().await;
                match app.reactions_to(&block_id).await {
                    Ok(reactions) => {
                        app.ws()
                            .broadcast_reactions_change(
                                &workspace_id,
                                &container_id,
                                &block_id,
                                &reactions,
                            )
                            .await;
                    }
                    Err(error) => {
                        tracing::error!(%error, block_id, "cannot read reactions to tell them");
                    }
                }
            }
            turns.done(&block_id, turn);
        });
    }
}

/// The turns of telling the reactions to a block: one at a time for each block, so that
/// what is read last is told last, and any number of blocks at once. A block is kept
/// while a telling of it holds or waits for its turn.
#[derive(Debug, Default)]
pub(super) struct TellingTurns {
    blocks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl TellingTurns {
    /// The turn of `block_id`, to wait for.
    fn of(&self, block_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut blocks = self.blocks.lock().expect("the turns are not poisoned");
        Arc::clone(blocks.entry(block_id.to_owned()).or_default())
    }

    /// Gives back `turn`, the turn of `block_id` that a telling took and let go of; the
    /// block is forgotten if no other telling holds or waits for it (the table's and
    /// this one are the only holders).
    fn done(&self, block_id: &str, turn: Arc<tokio::sync::Mutex<()>>) {
        let mut blocks = self.blocks.lock().expect("the turns are not poisoned");
        if Arc::strong_count(&turn) == 2 {
            blocks.remove(block_id);
        }
        drop(turn);
    }

    /// How many blocks are kept: none once every telling is done.
    #[cfg(test)]
    fn kept(&self) -> usize {
        self.blocks
            .lock()
            .expect("the turns are not poisoned")
            .len()
    }
}

/// The container reached, if its caller may react there to `block`: one who may comment
/// in it, on a comment, a card, a page or a unit of it.
///
/// # Errors
///
/// Returns what [`Reached::require`] refuses with, and 404 for any other block.
fn reacted_on<'on>(
    on: &'on Reached,
    block: &Block,
    denial: &str,
) -> Result<&'on Container, AppError> {
    let container = on.require(ContainerPermission::CommentCards, denial)?;
    if on.holds(block) && REACTED_TO.contains(&block.block_type) {
        Ok(container)
    } else {
        Err(AppError::not_found("block"))
    }
}

/// What a reaction with one emoji more than a block may have is answered with.
fn too_many_reactions() -> AppError {
    AppError::bad_request(format_args!(
        "a block is answered with at most {REACTION_KINDS_PER_BLOCK} different emoji"
    ))
    .with_reason(Reason::TooManyReactions)
    .with_limit(u64::try_from(REACTION_KINDS_PER_BLOCK).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::TellingTurns;

    #[tokio::test]
    async fn a_block_takes_its_turns_one_at_a_time_and_is_forgotten_after() {
        let turns = TellingTurns::default();
        let first = turns.of("a");
        let held = first.lock().await;
        let second = turns.of("a");
        assert!(second.try_lock().is_err(), "the block's next telling waits");
        let other = turns.of("b");
        assert!(other.try_lock().is_ok(), "another block's telling does not");
        turns.done("b", other);
        drop(held);
        turns.done("a", first);
        assert_eq!(turns.kept(), 1, "kept while a telling waits for it");
        turns.done("a", second);
        assert_eq!(turns.kept(), 0, "forgotten when every telling is done");
    }
}
