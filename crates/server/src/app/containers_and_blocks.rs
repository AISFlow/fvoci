//! Boards and blocks created, patched and deleted together.

use std::collections::HashMap;

use fvoci_model::{
    ContainerMember, ContainersAndBlocks, DeleteContainersAndBlocks, PatchContainersAndBlocks,
};

use fvoci_store::NewBodyFiles;

use super::containers::CreatorMembership;
use super::{App, Former};
use crate::collab::{AccessScope, Deletion};
use crate::error::AppError;
use crate::permissions::ContainerScope;

/// Who read a board the request did not list: nobody (the store refused it).
const NOBODY: Former<'static> = Former {
    members: &[],
    scope: ContainerScope::InvitedOnly,
};

impl App {
    /// Stores the boards and blocks of an archive as they are, identifiers included,
    /// tells the boards' members and the webhooks, and files each board that is not a
    /// template under the default category of `user_id`. The boards start without
    /// members. The images and file blocks of the bodies name the archive's files, which
    /// the import stores after them and points them at: the board's files are not asked
    /// of them ([`fvoci_store::NewBodyFiles::Unchecked`]).
    ///
    /// # Errors
    ///
    /// Returns a bad request if a block is too large, the store's error if nothing could
    /// be stored, or an error if a board cannot be filed under the user's default
    /// category; in the last case the boards and blocks exist nonetheless.
    pub async fn create_imported_containers_and_blocks(
        &self,
        containers_and_blocks: ContainersAndBlocks,
        user_id: &str,
    ) -> Result<ContainersAndBlocks, AppError> {
        self.create_containers_and_blocks_as(
            containers_and_blocks,
            user_id,
            CreatorMembership::None,
        )
        .await
    }

    /// Stores boards and their blocks as [`App::create_imported_containers_and_blocks`]
    /// does and makes `user_id` an admin and editor of each board. Each image and file
    /// block of a body names a file of its board
    /// ([`fvoci_store::NewBodyFiles::Recorded`]).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`App::create_imported_containers_and_blocks`].
    pub async fn create_containers_and_blocks_with_admin(
        &self,
        containers_and_blocks: ContainersAndBlocks,
        user_id: &str,
    ) -> Result<ContainersAndBlocks, AppError> {
        self.create_containers_and_blocks_as(
            containers_and_blocks,
            user_id,
            CreatorMembership::Admin,
        )
        .await
    }

    async fn create_containers_and_blocks_as(
        &self,
        mut created: ContainersAndBlocks,
        user_id: &str,
        membership: CreatorMembership,
    ) -> Result<ContainersAndBlocks, AppError> {
        for (index, container) in created.containers.iter().enumerate() {
            self.require_makes(container)
                .map_err(|error| error.under(&format!("/containers/{index}")))?;
        }
        let members = match membership {
            CreatorMembership::Admin => {
                let _structure = self
                    .collab()
                    .changing_structures(self, created.containers.iter().map(|c| c.id.as_str()))
                    .await;
                self.store()
                    .create_containers_and_blocks_with_admin(&mut created, user_id)
                    .await?
            }
            CreatorMembership::None => {
                self.store()
                    .create_containers_and_blocks(&mut created, user_id, NewBodyFiles::Unchecked)
                    .await?;
                Vec::new()
            }
        };

        // One check for every board made: an import makes many.
        if !created.containers.is_empty() {
            let made = created.containers.iter().map(|container| &container.id);
            self.collab()
                .recheck_access(self, AccessScope::containers(made))
                .await;
        }
        super::blocks::hide_inactive_numbers(&mut created.blocks);
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
        for container in &created.containers {
            self.ws()
                .broadcast_container_change(workspace_id, container)
                .await;
        }
        for block in &created.blocks {
            self.ws().broadcast_block_change(workspace_id, block).await;
            self.tell_webhooks(block);
        }
        for member in members {
            self.ws()
                .broadcast_member_change(workspace_id, &member.container_id, member)
                .await;
        }
    }

    /// Applies board and block patches, all or none, and tells the boards' members and
    /// the webhooks. Returns the patched boards and blocks in the order of the request.
    ///
    /// # Errors
    ///
    /// Returns a not-found error if a board or block does not exist or a block is listed
    /// twice, a bad request if a patched block is too large, or the store's error.
    pub async fn patch_containers_and_blocks(
        &self,
        patches: &PatchContainersAndBlocks,
        user_id: &str,
    ) -> Result<ContainersAndBlocks, AppError> {
        // Load referenced blocks up front so a missing or duplicate block ID fails before writing.
        let blocks = if patches.block_ids.is_empty() {
            Vec::new()
        } else {
            self.store().get_blocks_by_ids(&patches.block_ids).await?
        };

        let access_changed: Vec<&String> = patches
            .container_ids
            .iter()
            .zip(&patches.container_patches)
            .filter(|(_, patch)| super::containers::changes_access(patch))
            .map(|(container_id, _)| container_id)
            .collect();
        if !access_changed.is_empty() {
            self.collab().narrowing_access(
                self,
                &AccessScope::containers(access_changed.iter().copied()),
            );
        }

        // Whole bodies are written around the documents of bodies edited together.
        let card_ids =
            crate::collab::cards_written_whole(&blocks, &patches.block_ids, &patches.block_patches);
        let mut result = if card_ids.is_empty() {
            let touched = patches
                .container_ids
                .iter()
                .map(String::as_str)
                .chain(blocks.iter().map(|block| block.container_id.as_str()));
            let _structure = self.collab().changing_structures(self, touched).await;
            self.store()
                .patch_containers_and_blocks(patches, user_id)
                .await?
        } else {
            let (patches, user_id) = (patches.clone(), user_id.to_owned());
            self.collab()
                .write_whole(self, card_ids, async move |app| {
                    Ok(app
                        .store()
                        .patch_containers_and_blocks(&patches, &user_id)
                        .await?)
                })
                .await?
        };
        super::blocks::hide_inactive_numbers(&mut result.blocks);

        if !access_changed.is_empty() {
            self.collab()
                .recheck_access(self, AccessScope::containers(access_changed))
                .await;
        }
        let app = self.clone();
        let broadcast = result.clone();
        self.spawn_background(async move {
            let workspace_id = broadcast
                .containers
                .first()
                .map_or("", |container| container.workspace_id.as_str());
            for block in &broadcast.blocks {
                app.ws().broadcast_block_change(workspace_id, block).await;
                app.tell_webhooks(block);
            }
            for container in &broadcast.containers {
                app.ws()
                    .broadcast_container_change(&container.workspace_id, container)
                    .await;
            }
        });

        Ok(result)
    }

    /// Deletes blocks and boards, all or none, and tells the boards' members.
    ///
    /// # Errors
    ///
    /// Returns a bad request if no board is listed, a not-found error if a board or
    /// block does not exist, or the store's error.
    pub async fn delete_containers_and_blocks(
        &self,
        request: &DeleteContainersAndBlocks,
        user_id: &str,
    ) -> Result<(), AppError> {
        let Some(first_container_id) = request.containers.first() else {
            return Err(AppError::bad_request(
                fvoci_model::ContainersAndBlocksError::NoContainers,
            ));
        };
        let workspace_id = self
            .store()
            .get_container(first_container_id)
            .await?
            .workspace_id;

        // The memberships go with the boards, so who is told is who was a member; and
        // the blocks are gone afterwards, so which board each was on is read first.
        let mut readers_before = HashMap::with_capacity(request.containers.len());
        for container_id in &request.containers {
            let members = self
                .store()
                .effective_container_members(container_id)
                .await?;
            let scope = ContainerScope::of(&self.store().get_container(container_id).await?);
            readers_before.insert(container_id.as_str(), (members, scope));
        }
        let mut deleted_blocks = Vec::with_capacity(request.blocks.len());
        for block_id in &request.blocks {
            let block = self.store().get_block(block_id).await?;
            deleted_blocks.push((block.id, block.container_id, block.block_type));
        }

        let (removed, by) = (request.clone(), user_id.to_owned());
        self.collab()
            .remove(
                self,
                Deletion::Containers(removed.containers.clone()),
                async move |app| {
                    Ok(app
                        .store()
                        .delete_containers_and_blocks(&removed, &by)
                        .await?)
                },
            )
            .await?;

        if !request.containers.is_empty() {
            self.collab()
                .recheck_access(self, AccessScope::containers(&request.containers))
                .await;
        }
        let former = |container_id: &str| {
            readers_before
                .get(container_id)
                .map_or(NOBODY, |(members, scope)| Former {
                    members,
                    scope: *scope,
                })
        };
        for (block_id, container_id, block_type) in &deleted_blocks {
            // A block of a board the request does not list was refused by the store.
            self.ws()
                .broadcast_block_delete_with_container(
                    &workspace_id,
                    block_id,
                    container_id,
                    *block_type,
                    former(container_id),
                )
                .await;
        }
        for container_id in &request.containers {
            self.ws()
                .broadcast_container_delete(&workspace_id, container_id, former(container_id))
                .await;
            // The board's sharing went with it; so does whoever followed it by its link,
            // as with `deleteContainer`.
            self.ws().end_readers_of_container(container_id);
        }
        Ok(())
    }
}
