//! Validation and ID regeneration for combined container-and-block payloads.

use fvoci_model::{
    Block, BlockPatch, BlockType, Container, ContainerPatch, ContainerVisibility,
    ContainersAndBlocks, ContainersAndBlocksError, DeleteContainersAndBlocks,
    PatchContainersAndBlocks, generate_containers_and_blocks_ids, renew_containers_and_blocks_ids,
    renew_containers_and_blocks_ids_told,
};

fn board(id: &str, visibility: ContainerVisibility, title: &str) -> Container {
    Container {
        id: id.to_owned(),
        visibility,
        title: title.to_owned(),
        ..Container::default()
    }
}

fn block(id: &str, board_id: &str, block_type: BlockType) -> Block {
    Block {
        id: id.to_owned(),
        container_id: board_id.to_owned(),
        block_type,
        ..Block::default()
    }
}

fn ids(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn title_patch(title: &str) -> BlockPatch {
    BlockPatch {
        title: Some(title.to_owned()),
        ..BlockPatch::default()
    }
}

/// Validating boards and blocks together: no boards.
#[test]
fn containers_and_blocks_validate_no_containers() {
    let bab = ContainersAndBlocks {
        containers: vec![],
        blocks: vec![
            block("block-id-1", "board-id-1", BlockType::Card),
            block("block-id-2", "board-id-2", BlockType::Card),
        ],
    };
    assert_eq!(bab.validate(), Err(ContainersAndBlocksError::NoContainers));
}

/// Validating boards and blocks together: no blocks.
#[test]
fn containers_and_blocks_validate_no_blocks() {
    let bab = ContainersAndBlocks {
        containers: vec![
            board("board-id-1", ContainerVisibility::Open, ""),
            board("board-id-2", ContainerVisibility::Private, ""),
        ],
        blocks: vec![],
    };
    assert_eq!(bab.validate(), Err(ContainersAndBlocksError::NoBlocks));
}

/// Validating boards and blocks together: block that doesn't belong to the boards.
#[test]
fn containers_and_blocks_validate_block_that_does_not_belong_to_the_boards() {
    let bab = ContainersAndBlocks {
        containers: vec![
            board("board-id-1", ContainerVisibility::Open, ""),
            board("board-id-2", ContainerVisibility::Private, ""),
        ],
        blocks: vec![
            block("block-id-1", "board-id-1", BlockType::Card),
            block("block-id-3", "board-id-3", BlockType::Card),
            block("block-id-2", "board-id-2", BlockType::Card),
        ],
    };
    assert_eq!(
        bab.validate(),
        Err(ContainersAndBlocksError::BlockWithoutContainer {
            block_id: "block-id-3".to_owned()
        })
    );
}

/// Validating boards and blocks together: valid boards and blocks.
#[test]
fn containers_and_blocks_validate_valid() {
    let bab = ContainersAndBlocks {
        containers: vec![
            board("board-id-1", ContainerVisibility::Open, ""),
            board("board-id-2", ContainerVisibility::Private, ""),
        ],
        blocks: vec![
            block("block-id-1", "board-id-1", BlockType::Card),
            block("block-id-3", "board-id-2", BlockType::Card),
            block("block-id-2", "board-id-2", BlockType::Card),
        ],
    };
    assert_eq!(bab.validate(), Ok(()));
}

/// Generating the identifiers of boards and blocks: invalid boards and blocks.
#[test]
fn generate_containers_and_blocks_ids_invalid_boards_and_blocks() {
    let bab = ContainersAndBlocks {
        containers: vec![],
        blocks: vec![
            block("block-id-1", "board-id-1", BlockType::Card),
            block("block-id-2", "board-id-2", BlockType::Card),
        ],
    };
    assert_eq!(
        generate_containers_and_blocks_ids(bab),
        Err(ContainersAndBlocksError::NoContainers)
    );
}

/// Boards that are stored already get new identifiers also when they have no blocks,
/// and a block of a board that is not among them is left out.
#[test]
fn renew_containers_and_blocks_ids_takes_boards_without_blocks() {
    let renewed = renew_containers_and_blocks_ids(ContainersAndBlocks {
        containers: vec![board("board-id-1", ContainerVisibility::Open, "title")],
        blocks: vec![],
    });
    assert_eq!(renewed.containers.len(), 1, "the board is kept");
    assert_ne!(
        renewed.containers[0].id, "board-id-1",
        "under a new identifier"
    );
    assert!(renewed.blocks.is_empty(), "without blocks");

    let renewed = renew_containers_and_blocks_ids(ContainersAndBlocks {
        containers: vec![board("board-id-1", ContainerVisibility::Open, "title")],
        blocks: vec![
            block("block-id-1", "board-id-1", BlockType::Card),
            block("block-id-2", "board-id-2", BlockType::Card),
        ],
    });
    assert_eq!(renewed.blocks.len(), 1, "the block of no board is left out");
    assert_eq!(
        renewed.blocks[0].container_id, renewed.containers[0].id,
        "attached"
    );
}

/// A block that was asked for is told by its old identifier, whether or not anything
/// among the blocks names it; a block that was not asked for is not told, nor is what
/// is no block of the boards.
#[test]
fn renew_containers_and_blocks_ids_told_tells_the_new_identifiers_asked_for() {
    let told = ["block-id-1", "block-id-3"];
    let renewed = renew_containers_and_blocks_ids_told(
        ContainersAndBlocks {
            containers: vec![board("board-id-1", ContainerVisibility::Open, "title")],
            blocks: vec![
                block("block-id-1", "board-id-1", BlockType::Card),
                block("block-id-2", "board-id-1", BlockType::Card),
            ],
        },
        |id| told.contains(&id),
    );
    let blocks = &renewed.containers_and_blocks.blocks;
    assert_eq!(renewed.renamed.len(), 1, "{:?}", renewed.renamed);
    assert_eq!(renewed.renamed["block-id-1"], blocks[0].id);
    assert_ne!(blocks[0].id, "block-id-1", "under a new identifier");
}

/// Generating the identifiers of boards and blocks: correctly generates IDs for all the boards and
/// links the blocks to them, with new IDs too.
#[test]
fn generate_containers_and_blocks_ids_links_blocks_to_new_board_ids() {
    let bab = ContainersAndBlocks {
        containers: vec![
            board("board-id-1", ContainerVisibility::Open, "board1"),
            board("board-id-2", ContainerVisibility::Private, "board2"),
            board("board-id-3", ContainerVisibility::Open, "board3"),
        ],
        blocks: vec![
            block("block-id-1", "board-id-1", BlockType::Card),
            block("block-id-2", "board-id-2", BlockType::View),
            block("block-id-3", "board-id-2", BlockType::Comment),
        ],
    };

    let result = generate_containers_and_blocks_ids(bab).expect("the input is valid");

    let board_by_title = |title: &str| -> &Container {
        result
            .containers
            .iter()
            .find(|board| board.title == title)
            .unwrap_or_else(|| panic!("no board titled {title}"))
    };
    let block_by_type = |block_type: BlockType| -> &Block {
        result
            .blocks
            .iter()
            .find(|block| block.block_type == block_type)
            .unwrap_or_else(|| panic!("no block of type {block_type}"))
    };

    // Every board and block has a fresh identifier, and the blocks point at the new
    // identifiers of their boards.
    let board1 = board_by_title("board1");
    assert!(!board1.id.is_empty(), "board1 has no id");
    assert_ne!(board1.id, "board-id-1");
    let board2 = board_by_title("board2");
    assert!(!board2.id.is_empty(), "board2 has no id");
    assert_ne!(board2.id, "board-id-2");
    let board3 = board_by_title("board3");
    assert!(!board3.id.is_empty(), "board3 has no id");
    assert_ne!(board3.id, "board-id-3");

    let block1 = block_by_type(BlockType::Card);
    assert!(!block1.id.is_empty(), "block1 has no id");
    assert_ne!(block1.id, "block-id-1");
    assert_eq!(block1.container_id, board1.id);

    let block2 = block_by_type(BlockType::View);
    assert!(!block2.id.is_empty(), "block2 has no id");
    assert_ne!(block2.id, "block-id-2");
    assert_eq!(block2.container_id, board2.id);

    let block3 = block_by_type(BlockType::Comment);
    assert!(!block3.id.is_empty(), "block3 has no id");
    assert_ne!(block3.id, "block-id-3");
    assert_eq!(block3.container_id, board2.id);
}

/// Validating a patch of boards and blocks: no board ids.
#[test]
fn patch_containers_and_blocks_validate_no_container_ids() {
    let pbab = PatchContainersAndBlocks {
        container_ids: vec![],
        container_patches: vec![],
        block_ids: ids(&["block-id-1"]),
        block_patches: vec![title_patch("new title"), title_patch("other title")],
    };
    assert_eq!(pbab.validate(), Err(ContainersAndBlocksError::NoContainers));
}

/// Validating a patch of boards and blocks: mismatched board IDs and patches.
#[test]
fn patch_containers_and_blocks_validate_mismatched_container_ids_and_patches() {
    let pbab = PatchContainersAndBlocks {
        container_ids: ids(&["board-id-1", "board-id-2"]),
        container_patches: vec![ContainerPatch {
            title: Some("new title".to_owned()),
            ..ContainerPatch::default()
        }],
        block_ids: ids(&["block-id-1"]),
        block_patches: vec![title_patch("new title")],
    };
    assert_eq!(
        pbab.validate(),
        Err(ContainersAndBlocksError::ContainerIdsAndPatchesMismatch)
    );
}

/// Validating a patch of boards and blocks: mismatched block IDs and patches.
#[test]
fn patch_containers_and_blocks_validate_mismatched_block_ids_and_patches() {
    let pbab = PatchContainersAndBlocks {
        container_ids: ids(&["board-id-1", "board-id-2"]),
        container_patches: vec![
            ContainerPatch {
                title: Some("new title".to_owned()),
                ..ContainerPatch::default()
            },
            ContainerPatch {
                description: Some("new description".to_owned()),
                ..ContainerPatch::default()
            },
        ],
        block_ids: ids(&["block-id-1"]),
        block_patches: vec![title_patch("new title"), title_patch("other title")],
    };
    assert_eq!(
        pbab.validate(),
        Err(ContainersAndBlocksError::BlockIdsAndPatchesMismatch)
    );
}

/// Validating a patch of boards and blocks: valid.
#[test]
fn patch_containers_and_blocks_validate_valid() {
    let pbab = PatchContainersAndBlocks {
        container_ids: ids(&["board-id-1", "board-id-2"]),
        container_patches: vec![
            ContainerPatch {
                title: Some("new title".to_owned()),
                ..ContainerPatch::default()
            },
            ContainerPatch {
                description: Some("new description".to_owned()),
                ..ContainerPatch::default()
            },
        ],
        block_ids: ids(&["block-id-1"]),
        block_patches: vec![title_patch("new title")],
    };
    assert_eq!(pbab.validate(), Ok(()));
}

/// Validates delete payloads: requires at least one container ID while allowing empty block IDs.
#[test]
fn delete_containers_and_blocks_validate_requires_only_containers() {
    let no_boards = DeleteContainersAndBlocks {
        containers: vec![],
        blocks: ids(&["block-id-1"]),
    };
    assert_eq!(
        no_boards.validate(),
        Err(ContainersAndBlocksError::NoContainers)
    );

    let no_blocks = DeleteContainersAndBlocks {
        containers: ids(&["board-id-1", "board-id-2"]),
        blocks: vec![],
    };
    assert_eq!(no_blocks.validate(), Ok(()));

    let both = DeleteContainersAndBlocks {
        containers: ids(&["board-id-1", "board-id-2"]),
        blocks: ids(&["block-id-1"]),
    };
    assert_eq!(both.validate(), Ok(()));
}

/// A delete request built from created boards and blocks lists their identifiers in order.
#[test]
fn delete_containers_and_blocks_from_containers_and_blocks() {
    let bab = ContainersAndBlocks {
        containers: vec![board("board-id-1", ContainerVisibility::Open, "")],
        blocks: vec![
            block("block-id-1", "board-id-1", BlockType::Card),
            block("block-id-2", "board-id-1", BlockType::View),
        ],
    };
    assert_eq!(
        DeleteContainersAndBlocks::from(&bab),
        DeleteContainersAndBlocks {
            containers: ids(&["board-id-1"]),
            blocks: ids(&["block-id-1", "block-id-2"]),
        }
    );
}
