//! Boards and blocks created together.

use fvoci_model::{Container, ContainerMember, ContainersAndBlocks};
use fvoci_store::{NewContainers, StoreError};

use super::{App, Changes};
use crate::collab::AccessScope;
use crate::error::AppError;

impl App {
    /// Stores boards and their blocks, identifiers included, tells the boards' members
    /// and the webhooks, makes `user_id` an admin and editor of each board, and files each
    /// board that is not a template under the default category of `user_id`. Each image
    /// and file block of a body names a file of its board
    /// ([`fvoci_store::NewBodyFiles::Recorded`]).
    ///
    /// # Errors
    ///
    /// Returns a bad request if a block is too large, the store's error if nothing could
    /// be stored, or an error if a board cannot be filed under the user's default
    /// category; in the last case the boards and blocks exist nonetheless.
    pub async fn create_containers_and_blocks_with_admin(
        &self,
        mut created: ContainersAndBlocks,
        user_id: &str,
    ) -> Result<ContainersAndBlocks, AppError> {
        // Where a refusal points is where the board's request named it: the boards are
        // asked for one at a time (`createBoard`), not as a list.
        for container in &created.containers {
            self.require_makes(container)?;
        }
        let members = {
            let _structure = self
                .collab()
                .changing_structures(self, created.containers.iter().map(|c| c.id.as_str()))
                .await;
            self.store()
                .create_containers(NewContainers::whole(&mut created), user_id)
                .await
                .map_err(StoreError::from)?
        };

        self.recheck_made(&created.containers).await;
        self.announce_created(&created, &members).await;

        for container in &created.containers {
            if !container.is_template {
                self.add_containers_to_default_category(
                    user_id,
                    &container.workspace_id,
                    std::slice::from_ref(&container.id),
                )
                .await?;
            }
        }
        Ok(created)
    }

    /// Has the rooms of the boards `containers`, made just now, ask again who may be in
    /// them, before the write that made them answers.
    pub(crate) async fn recheck_made(&self, containers: &[Container]) {
        if containers.is_empty() {
            return;
        }
        let made = containers.iter().map(|container| &container.id);
        self.collab()
            .recheck_access(self, AccessScope::containers(made))
            .await;
    }

    /// Tells the boards' members and the webhooks about boards and blocks that were
    /// stored together, and about the memberships made with them, before answering.
    pub(crate) async fn announce_created(
        &self,
        created: &ContainersAndBlocks,
        members: &[ContainerMember],
    ) {
        // All new boards belong to one workspace; if none were created there is nobody to tell.
        let workspace_id = created
            .containers
            .first()
            .map_or("", |container| container.workspace_id.as_str());
        let changes = Changes::in_workspace(workspace_id)
            .made(created.containers.iter().cloned(), members.iter().cloned())
            .changed(created.blocks.iter().cloned());
        self.tell_now(changes).await;
    }
}
