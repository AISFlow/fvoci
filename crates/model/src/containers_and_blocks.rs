//! Requests that operate on several boards and blocks at once.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::block::{Block, BlockPatch};
use crate::block_ids::{UnmatchedRelation, renew_block_ids_told};
use crate::container::{Container, ContainerPatch};
use crate::id::new_id;
use crate::relation::RelationProperties;

/// Reasons a containers-and-blocks request is rejected.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ContainersAndBlocksError {
    /// The request lists no board.
    #[error("at least one board is required")]
    NoContainers,
    /// The request lists no block.
    #[error("at least one block is required")]
    NoBlocks,
    /// The number of board identifiers and board patches differ.
    #[error("board ids and patches need to match")]
    ContainerIdsAndPatchesMismatch,
    /// The number of block identifiers and block patches differ.
    #[error("block ids and patches need to match")]
    BlockIdsAndPatchesMismatch,
    /// A block names a board that is not part of the request.
    #[error("block {block_id} doesn't belong to any board")]
    BlockWithoutContainer {
        /// The identifier of the offending block.
        block_id: String,
    },
}

/// Boards and the blocks that belong to them, created together.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default)]
pub struct ContainersAndBlocks {
    /// The boards.
    pub containers: Vec<Container>,
    /// The blocks; each must belong to one of `containers`.
    pub blocks: Vec<Block>,
}

impl ContainersAndBlocks {
    /// Checks that there is at least one board and one block, and that every block belongs
    /// to one of the boards.
    ///
    /// # Errors
    ///
    /// Returns the first [`ContainersAndBlocksError`] found, checking boards, then blocks, then
    /// each block's board in order.
    pub fn validate(&self) -> Result<(), ContainersAndBlocksError> {
        if self.containers.is_empty() {
            return Err(ContainersAndBlocksError::NoContainers);
        }
        if self.blocks.is_empty() {
            return Err(ContainersAndBlocksError::NoBlocks);
        }
        let container_ids: HashSet<&str> = self
            .containers
            .iter()
            .map(|container| container.id.as_str())
            .collect();
        match self
            .blocks
            .iter()
            .find(|block| !container_ids.contains(block.container_id.as_str()))
        {
            Some(block) => Err(ContainersAndBlocksError::BlockWithoutContainer {
                block_id: block.id.clone(),
            }),
            None => Ok(()),
        }
    }
}

/// The identifiers of boards and blocks to delete together.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default)]
pub struct DeleteContainersAndBlocks {
    /// The identifiers of the boards.
    pub containers: Vec<String>,
    /// The identifiers of the blocks.
    pub blocks: Vec<String>,
}

impl DeleteContainersAndBlocks {
    /// Checks that at least one board is listed. Blocks are optional.
    ///
    /// # Errors
    ///
    /// Returns [`ContainersAndBlocksError::NoContainers`] if no board is listed.
    pub fn validate(&self) -> Result<(), ContainersAndBlocksError> {
        if self.containers.is_empty() {
            return Err(ContainersAndBlocksError::NoContainers);
        }
        Ok(())
    }
}

impl From<&ContainersAndBlocks> for DeleteContainersAndBlocks {
    fn from(containers_and_blocks: &ContainersAndBlocks) -> Self {
        Self {
            containers: containers_and_blocks
                .containers
                .iter()
                .map(|container| container.id.clone())
                .collect(),
            blocks: containers_and_blocks
                .blocks
                .iter()
                .map(|block| block.id.clone())
                .collect(),
        }
    }
}

/// Patches for several boards and blocks, applied together.
///
/// Identifiers and patches are matched by position.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default)]
pub struct PatchContainersAndBlocks {
    /// The identifiers of the boards to patch.
    #[serde(rename = "containerIds")]
    pub container_ids: Vec<String>,
    /// The board patches, one per board identifier.
    #[serde(rename = "containerPatches")]
    pub container_patches: Vec<ContainerPatch>,
    /// The identifiers of the blocks to patch.
    #[serde(rename = "blockIds")]
    pub block_ids: Vec<String>,
    /// The block patches, one per block identifier.
    #[serde(rename = "blockPatches")]
    pub block_patches: Vec<BlockPatch>,
}

impl PatchContainersAndBlocks {
    /// Checks that at least one board is patched and that identifiers and patches pair up.
    ///
    /// # Errors
    ///
    /// Returns a [`ContainersAndBlocksError`] if there is no board identifier, or the board or
    /// block identifiers and patches differ in number.
    pub fn validate(&self) -> Result<(), ContainersAndBlocksError> {
        if self.container_ids.is_empty() {
            return Err(ContainersAndBlocksError::NoContainers);
        }
        if self.container_ids.len() != self.container_patches.len() {
            return Err(ContainersAndBlocksError::ContainerIdsAndPatchesMismatch);
        }
        if self.block_ids.len() != self.block_patches.len() {
            return Err(ContainersAndBlocksError::BlockIdsAndPatchesMismatch);
        }
        Ok(())
    }
}

/// Gives every board and block of a request a new identifier, keeping blocks attached
/// to their boards and to each other.
///
/// Blocks come back grouped by board, in the order of `containers`.
///
/// # Errors
///
/// Returns the [`ContainersAndBlocksError`] from [`ContainersAndBlocks::validate`] if the input is
/// not valid.
pub fn generate_containers_and_blocks_ids(
    containers_and_blocks: ContainersAndBlocks,
) -> Result<ContainersAndBlocks, ContainersAndBlocksError> {
    containers_and_blocks.validate()?;
    Ok(renew_ids(containers_and_blocks, UnmatchedRelation::Kept, &|_| false).containers_and_blocks)
}

/// Gives every board and block a new identifier, keeping blocks attached to their boards
/// and to each other. For boards and blocks that are stored already, such as a board
/// that is copied: unlike a request, they may be boards without blocks.
///
/// Blocks come back grouped by board, in the order of `containers`. A block of a board that
/// is not among `containers` is left out. A card relation value names the copies of the cards it named
/// that are among `blocks`, and no other card.
#[must_use]
pub fn renew_containers_and_blocks_ids(
    containers_and_blocks: ContainersAndBlocks,
) -> ContainersAndBlocks {
    renew_ids(containers_and_blocks, UnmatchedRelation::Dropped, &|_| {
        false
    })
    .containers_and_blocks
}

/// Boards and blocks with new identifiers, and what some of the blocks were renamed to.
#[derive(Debug, Clone)]
pub struct Renewed {
    /// The boards and blocks, as [`renew_containers_and_blocks_ids`] gives them.
    pub containers_and_blocks: ContainersAndBlocks,
    /// The new identifier of each block that was asked for, by its old one.
    pub renamed: HashMap<String, String>,
}

/// Does what [`renew_containers_and_blocks_ids`] does and tells the new identifier of each
/// block that `told` picks by its old one. One board of an archive of several is
/// renewed so: a body of another board that names one of its blocks is pointed at the
/// copy with it.
#[must_use]
pub fn renew_containers_and_blocks_ids_told(
    containers_and_blocks: ContainersAndBlocks,
    told: impl Fn(&str) -> bool,
) -> Renewed {
    renew_ids(containers_and_blocks, UnmatchedRelation::Dropped, &told)
}

/// Gives every board and block a new identifier, as the two functions above say; a card
/// relation value names the cards among the blocks by their new identifiers, and what
/// else it names is kept or dropped as `unmatched` says.
fn renew_ids(
    containers_and_blocks: ContainersAndBlocks,
    unmatched: UnmatchedRelation,
    told: &dyn Fn(&str) -> bool,
) -> Renewed {
    let ContainersAndBlocks {
        mut containers,
        blocks,
    } = containers_and_blocks;

    let mut blocks_by_container: HashMap<String, Vec<Block>> = HashMap::new();
    for block in blocks {
        blocks_by_container
            .entry(block.container_id.clone())
            .or_default()
            .push(block);
    }

    let mut blocks = Vec::new();
    for container in &mut containers {
        let new_container_id = new_id();
        // Validation does not reject two boards sharing an identifier; the first one
        // takes the blocks so they are not emitted twice.
        for mut block in blocks_by_container
            .remove(&container.id)
            .unwrap_or_default()
        {
            block.container_id.clone_from(&new_container_id);
            blocks.push(block);
        }
        container.id = new_container_id;
    }
    let mut relations = RelationProperties::new();
    for container in &containers {
        relations.add_board(&container.id, &container.card_properties);
    }
    let renamed = renew_block_ids_told(&mut blocks, &relations, unmatched, told);

    Renewed {
        containers_and_blocks: ContainersAndBlocks { containers, blocks },
        renamed,
    }
}
