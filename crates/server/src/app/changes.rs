//! What a committed write changed, told once it is committed: the one place that tells
//! the websocket listeners of a workspace that a block changed
//! ([`Hub::broadcast_block_change`](crate::realtime::Hub::broadcast_block_change)) and
//! queues the block for the operator's webhooks ([`App::tell_webhooks`]).
//!
//! What is written in the write's transaction (its audit, its search outbox, whom it
//! notifies) is not here: that is stored with the write, or not at all.

use fvoci_model::{Block, Container, ContainerMember};

use super::App;

/// What a committed write changed, to tell its workspace's listeners of in turn: the
/// containers it made, then its blocks in the order they were added, then the
/// memberships it made. Told by [`App::tell`] or [`App::tell_now`].
#[derive(Debug)]
#[must_use = "a write that is not told leaves its listeners with what was there before"]
pub(crate) struct Changes {
    workspace_id: String,
    containers: Vec<Container>,
    blocks: Vec<Told>,
    members: Vec<ContainerMember>,
}

/// One block of [`Changes`], as it is told.
#[derive(Debug)]
enum Told {
    /// The block as the write left it.
    Changed(Block, Hooks),
    /// The block of this identifier, read again as it is told: one deleted meanwhile is
    /// not told of.
    Reread(String),
    /// A block deleted: its listeners are told that it went.
    Deleted(Block),
}

/// Whether the operator's webhooks are told of a block its listeners are told of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hooks {
    /// They are.
    Told,
    /// They are not: a block of a container copied whole.
    Silent,
}

impl Changes {
    /// Nothing changed yet in the workspace `workspace_id`, whose listeners are told.
    pub(crate) fn in_workspace(workspace_id: &str) -> Self {
        Self {
            workspace_id: workspace_id.to_owned(),
            containers: Vec::new(),
            blocks: Vec::new(),
            members: Vec::new(),
        }
    }

    /// The blocks as the write left them, told to the listeners and the webhooks.
    pub(crate) fn changed(mut self, blocks: impl IntoIterator<Item = Block>) -> Self {
        let told = blocks
            .into_iter()
            .map(|block| Told::Changed(block, Hooks::Told));
        self.blocks.extend(told);
        self
    }

    /// The blocks of containers copied whole, told to the listeners alone: the webhooks
    /// are not told of such a copy.
    pub(crate) fn copied(mut self, blocks: impl IntoIterator<Item = Block>) -> Self {
        let told = blocks
            .into_iter()
            .map(|block| Told::Changed(block, Hooks::Silent));
        self.blocks.extend(told);
        self
    }

    /// The blocks of these identifiers, read again as they are told, to the listeners
    /// and the webhooks: one deleted meanwhile is left out, and a block that cannot be
    /// read ends the telling of blocks.
    pub(crate) fn reread(mut self, block_ids: impl IntoIterator<Item = String>) -> Self {
        self.blocks.extend(block_ids.into_iter().map(Told::Reread));
        self
    }

    /// The blocks deleted: their listeners are told that they went, and nobody else.
    pub(crate) fn deleted(mut self, blocks: impl IntoIterator<Item = Block>) -> Self {
        self.blocks.extend(blocks.into_iter().map(Told::Deleted));
        self
    }

    /// The containers made, told of first, and the memberships made of them, told of
    /// last.
    pub(crate) fn made(
        mut self,
        containers: impl IntoIterator<Item = Container>,
        members: impl IntoIterator<Item = ContainerMember>,
    ) -> Self {
        self.containers.extend(containers);
        self.members.extend(members);
        self
    }

    fn is_empty(&self) -> bool {
        self.containers.is_empty() && self.blocks.is_empty() && self.members.is_empty()
    }
}

impl App {
    /// Tells of `changes` in the background, as [`App::tell_now`] does, so that the
    /// write that made them answers without waiting for its listeners.
    pub(crate) fn tell(&self, changes: Changes) {
        if changes.is_empty() {
            return;
        }
        let app = self.clone();
        self.spawn_background(async move { app.tell_now(changes).await });
    }

    /// Tells the listeners of the workspace of `changes`, in their order, and queues
    /// each block they are told of for the webhooks where it is theirs to hear: before
    /// the write answers, for a write whose client is to have heard of it by then.
    pub(crate) async fn tell_now(&self, changes: Changes) {
        let Changes {
            workspace_id,
            containers,
            blocks,
            members,
        } = changes;
        let hub = self.ws();
        for container in &containers {
            hub.broadcast_container_change(&workspace_id, container)
                .await;
        }
        for told in blocks {
            let (block, hooks) = match told {
                Told::Changed(block, hooks) => (block, hooks),
                Told::Reread(block_id) => match self.store().get_block(&block_id).await {
                    Ok(block) => (block, Hooks::Told),
                    Err(error) if error.is_not_found() => continue,
                    Err(error) => {
                        tracing::error!(%error, block_id, "cannot read a changed block to tell of it");
                        break;
                    }
                },
                Told::Deleted(block) => {
                    hub.broadcast_block_delete(
                        &workspace_id,
                        &block.id,
                        &block.container_id,
                        block.block_type,
                    )
                    .await;
                    continue;
                }
            };
            hub.broadcast_block_change(&workspace_id, &block).await;
            if hooks == Hooks::Told {
                self.tell_webhooks(&block);
            }
        }
        for member in &members {
            hub.broadcast_member_change(&workspace_id, &member.container_id, member)
                .await;
        }
    }
}

#[cfg(test)]
mod tests;
