//! Where a block may be by its type: a page on a wiki, at its top or below a page of
//! it, and a wiki has no block but its pages and the comments on them; an entry on a
//! diary, below nothing, and a diary has no block but its entries; a chapter at the top
//! of a course and a unit at the top of a course or below a chapter, and a course has no
//! block but its chapters, its units and the comments on its units. Every block that is
//! written is checked here by [`check`].

use fvoci_model::{Block, BlockError, BlockType, ContainerKind};

use crate::error::StoreError;

/// Checks that a block that is written is of a type its container's kind has, that a
/// page is below nothing but a page, an entry below nothing, a chapter below nothing, a
/// unit below nothing or a chapter, and that what a wiki or a course has besides its
/// pages or chapters and units is comments on them. The block's container is held, so
/// the parent that was found stays as it is until the block is stored. `kind` is the
/// kind of the block's container, and `parent_type` the type of the block's parent on
/// that container, `None` if it has none.
pub(crate) fn check(
    block: &Block,
    kind: ContainerKind,
    parent_type: Option<&BlockType>,
) -> Result<(), StoreError> {
    let below_page = parent_type.is_none_or(|parent| *parent == BlockType::Page);
    let below_chapter = parent_type.is_none_or(|parent| *parent == BlockType::Chapter);
    let refused = match (kind, &block.block_type) {
        (ContainerKind::Diary, BlockType::Entry) | (ContainerKind::Course, BlockType::Chapter)
            if parent_type.is_none() =>
        {
            return Ok(());
        }
        (ContainerKind::Diary, BlockType::Entry) => BlockError::EntryBelowBlock,
        (ContainerKind::Diary, _) => BlockError::NotEntryOnDiary,
        (ContainerKind::Course, BlockType::Chapter) => BlockError::ChapterBelowBlock,
        (ContainerKind::Course, BlockType::Unit) if below_chapter => return Ok(()),
        (ContainerKind::Course, BlockType::Unit) => BlockError::UnitParentNotChapter,
        // Where a comment goes, on a unit of the course, is `comment_places`' to say
        // (chapters take no comments): its parent is what a request gets wrong.
        (ContainerKind::Course, BlockType::Comment) => return Ok(()),
        (ContainerKind::Course, _) => BlockError::NotUnitOnCourse,
        (ContainerKind::Board | ContainerKind::Wiki, BlockType::Entry) => BlockError::EntryOffDiary,
        (ContainerKind::Board | ContainerKind::Wiki, BlockType::Unit | BlockType::Chapter) => {
            BlockError::UnitOffCourse
        }
        (ContainerKind::Board, BlockType::Page) => BlockError::PageOffWiki,
        // Where a comment goes, on a page of the wiki, is `comment_places`' to say, as for
        // a comment of a board: its parent is what a request gets wrong.
        (ContainerKind::Board, _) | (ContainerKind::Wiki, BlockType::Comment) => return Ok(()),
        (ContainerKind::Wiki, BlockType::Page) if below_page => return Ok(()),
        (ContainerKind::Wiki, BlockType::Page) => BlockError::PageNotBelowPage,
        (ContainerKind::Wiki, _) => BlockError::NotPageOnWiki,
    };
    Err(StoreError::InvalidBlock {
        id: block.id.clone(),
        source: refused,
    })
}
