//! Where the blocks of an archive may be, checked as the store checks a write before
//! anything of the archive is stored: a block's parent on its own board, a comment on a
//! card or a page with nothing below it (`fvoci_store`'s `comment_places`), of a board
//! the types a write of blocks makes, of a wiki its pages and the comments on them, the
//! pages a tree within the organization's limits (`page_places`, [`PageTree`]). Each
//! check takes time in proportion to the blocks, whatever their shape.

use std::collections::{HashMap, HashSet};

use fvoci_model::{
    BlockError, BlockType, CHAPTERS_PER_COURSE, COURSE_DEPTH, Container, ContainerKind,
    ContainersAndBlocks, DiaryDate, MAX_CARD_DEPTH, PageLimits, PageNode, PageTree,
    UNITS_PER_COURSE,
};
use fvoci_store::StoreError;

use crate::app::pages::refused_without_member;
use crate::auth::now_millis;
use crate::error::AppError;

/// Lightweight structural projection of a block used by [`check_places`] so streaming
/// imports can verify tree, parent, and diary rules without retaining block bodies in RAM.
#[derive(Debug, Clone, Copy)]
pub(super) struct BlockPlace<'a> {
    pub(super) id: &'a str,
    pub(super) container_id: &'a str,
    pub(super) parent_id: &'a str,
    pub(super) block_type: &'a BlockType,
    pub(super) is_template: bool,
    pub(super) diary_date: Option<DiaryDate>,
}

/// Checks the blocks of each board of `containers_and_blocks` against the kind of their
/// board and against each other, as they are read from an archive (each with its board
/// and its parent by their identifiers in the archive). A block of a board whose parent
/// the archive does not have is left to the caller, which drops what nothing leads to;
/// of a wiki, it is refused. A block that is on no board of them is held to a board's
/// types.
///
/// # Errors
///
/// Returns a bad request, with the store's message, for a block of a type its board's
/// kind does not take (a page of a board, anything but a page or a comment of a wiki, a
/// type no write makes), a parent on another board, a comment on anything but a card or
/// a page, a block below a comment, a page below anything but a page; and for a wiki, a
/// parent the archive does not have, two blocks of one identifier, pages that are each
/// other's parents, and pages more or deeper than `limits` let a wiki have.
pub(super) fn check(
    containers_and_blocks: &ContainersAndBlocks,
    limits: PageLimits,
) -> Result<(), AppError> {
    let places: Vec<BlockPlace<'_>> = containers_and_blocks
        .blocks
        .iter()
        .map(|block| BlockPlace {
            id: &block.id,
            container_id: &block.container_id,
            parent_id: &block.parent_id,
            block_type: &block.block_type,
            is_template: block.is_template(),
            diary_date: (block.block_type == BlockType::Entry)
                .then(|| fvoci_model::entry_date(block))
                .flatten(),
        })
        .collect();
    check_places(&containers_and_blocks.containers, &places, limits)
}

/// Same rules as [`check`], operating over lightweight [`BlockPlace`] metadata collected
/// while streaming `container.jsonl` line by line.
pub(super) fn check_places(
    containers: &[Container],
    blocks: &[BlockPlace<'_>],
    limits: PageLimits,
) -> Result<(), AppError> {
    // The first block of an identifier is the one named, as the copy names it.
    let mut placed: HashMap<&str, BlockPlace<'_>> = HashMap::with_capacity(blocks.len());
    let mut on_container: HashMap<&str, Vec<BlockPlace<'_>>> =
        HashMap::with_capacity(containers.len());
    for &block in blocks {
        placed.entry(block.id).or_insert(block);
        on_container
            .entry(block.container_id)
            .or_default()
            .push(block);
    }
    let container_ids: HashSet<&str> = containers
        .iter()
        .map(|container| container.id.as_str())
        .collect();
    for container in containers {
        let mine = on_container
            .remove(container.id.as_str())
            .unwrap_or_default();
        let place = Place {
            container_id: &container.id,
            kind: container.kind,
            placed: &placed,
            containers: &container_ids,
        };
        for &block in &mine {
            place.check(block)?;
        }
        match container.kind {
            ContainerKind::Wiki => check_tree(&container.id, &mine, limits)?,
            ContainerKind::Diary => check_days(&container.id, &mine)?,
            ContainerKind::Course => check_course(&container.id, &mine)?,
            ContainerKind::Board => check_board_cards(&container.id, &mine)?,
        }
    }
    // What is on none of the boards is dropped later, unless something leads to it; of a
    // type no write makes, it is refused all the same.
    for block in on_container.into_values().flatten() {
        check_type(block, ContainerKind::Board)?;
    }
    Ok(())
}

/// The refusal of `block` as the store refuses it.
fn refused(block: BlockPlace<'_>, source: BlockError) -> AppError {
    StoreError::InvalidBlock {
        id: block.id.to_owned(),
        source,
    }
    .into()
}

/// The board the blocks are checked on, and the rest of the archive they are checked
/// against.
struct Place<'a> {
    container_id: &'a str,
    kind: ContainerKind,
    /// Every block of the archive by its identifier: the first of an identifier.
    placed: &'a HashMap<&'a str, BlockPlace<'a>>,
    /// The boards of the archive.
    containers: &'a HashSet<&'a str>,
}

impl Place<'_> {
    /// Checks one block of the board: its type, and its parent as a write would find it.
    fn check(&self, block: BlockPlace<'_>) -> Result<(), AppError> {
        check_type(block, self.kind)?;
        // A block directly on its board has no parent; the board is none, and is refused
        // as another board of the archive is.
        let parent_id = block.parent_id;
        if self.kind == ContainerKind::Diary && !parent_id.is_empty() {
            return Err(refused(block, BlockError::EntryBelowBlock));
        }
        if self.kind == ContainerKind::Course
            && *block.block_type == BlockType::Chapter
            && !parent_id.is_empty()
        {
            return Err(refused(block, BlockError::ChapterBelowBlock));
        }
        if self.kind == ContainerKind::Board {
            if *block.block_type == BlockType::Attachment && parent_id.is_empty() {
                return Err(refused(block, BlockError::AttachmentNotOnCard));
            }
            if *block.block_type == BlockType::Card {
                if block.is_template && !parent_id.is_empty() {
                    return Err(refused(block, BlockError::TemplateCardBelowBlock));
                }
                if !parent_id.is_empty() && parent_id == block.id {
                    return Err(refused(block, BlockError::ParentBelowBlock));
                }
            }
        }
        let parent_block = if parent_id.is_empty() {
            None
        } else if let Some(parent) = self.placed.get(parent_id) {
            if parent.container_id != self.container_id {
                return Err(refused(block, BlockError::ParentNotOnContainer));
            }
            Some(*parent)
        } else if self.containers.contains(parent_id)
            || matches!(self.kind, ContainerKind::Wiki | ContainerKind::Course)
        {
            // Another board of the archive, or what a wiki's or course's tree does not have.
            return Err(refused(block, BlockError::ParentNotOnContainer));
        } else {
            // What nothing leads to is dropped: no write is asked of it.
            return Ok(());
        };
        let parent_type = parent_block.map(|parent| parent.block_type);
        // In the store's order: where a page or unit may be, then where a comment may be,
        // then board card parent rules.
        let refusal = match (block.block_type, parent_type) {
            (BlockType::Page, Some(parent)) if *parent != BlockType::Page => {
                Some(BlockError::PageNotBelowPage)
            }
            (BlockType::Unit, Some(parent)) if *parent != BlockType::Chapter => {
                Some(BlockError::UnitParentNotChapter)
            }
            (BlockType::Comment, parent)
                if !parent.copied().is_some_and(BlockType::takes_comments) =>
            {
                Some(BlockError::CommentNotOnCardOrPage)
            }
            (_, Some(BlockType::Comment)) => Some(BlockError::ParentIsComment),
            (BlockType::View, Some(_)) => Some(BlockError::ViewBelowBlock),
            (_, Some(BlockType::View)) => Some(BlockError::ParentIsView),
            (BlockType::Attachment, parent) if parent != Some(&BlockType::Card) => {
                Some(BlockError::AttachmentNotOnCard)
            }
            (_, Some(BlockType::Attachment)) => Some(BlockError::ParentIsAttachment),
            (BlockType::Card, Some(parent)) if *parent != BlockType::Card => {
                Some(BlockError::CardNotBelowCard)
            }
            (BlockType::Card, Some(BlockType::Card))
                if parent_block.is_some_and(|parent| parent.is_template) =>
            {
                Some(BlockError::CardBelowTemplate)
            }
            _ => None,
        };
        refusal.map_or(Ok(()), |source| Err(refused(block, source)))
    }
}

/// Checks that a block is of a type a board of `kind` takes: of a board, what a write of
/// blocks makes and no page, entry, unit or chapter; of a wiki, a page or a comment; of a
/// diary, an entry; of a course, a chapter, a unit or a comment.
fn check_type(block: BlockPlace<'_>, kind: ContainerKind) -> Result<(), AppError> {
    let refusal = match (kind, block.block_type) {
        (ContainerKind::Course, BlockType::Unit | BlockType::Chapter | BlockType::Comment)
        | (ContainerKind::Diary, BlockType::Entry) => None,
        (ContainerKind::Course, _) => Some(BlockError::NotUnitOnCourse),
        (ContainerKind::Diary, _) => Some(BlockError::NotEntryOnDiary),
        (ContainerKind::Board | ContainerKind::Wiki, BlockType::Entry) => {
            Some(BlockError::EntryOffDiary)
        }
        (ContainerKind::Board | ContainerKind::Wiki, BlockType::Unit | BlockType::Chapter) => {
            Some(BlockError::UnitOffCourse)
        }
        (ContainerKind::Board, BlockType::Page) => Some(BlockError::PageOffWiki),
        (ContainerKind::Board, block_type) if !block_type.is_current() => {
            Some(BlockError::TypeNotCurrent)
        }
        (ContainerKind::Wiki, BlockType::Page | BlockType::Comment) | (ContainerKind::Board, _) => {
            None
        }
        (ContainerKind::Wiki, _) => Some(BlockError::NotPageOnWiki),
    };
    refusal.map_or(Ok(()), |source| Err(refused(block, source)))
}

/// Checks that the entries of the diary are no more than a diary may have, and each of
/// a day of its own that an entry may be made on today: what the store refuses one
/// entry at a time and a request for an entry is refused, said of the archive before
/// anything of it is stored. An entry that says no day is the store's to refuse.
fn check_days(diary_id: &str, blocks: &[BlockPlace<'_>]) -> Result<(), AppError> {
    check_days_at(diary_id, blocks, DiaryDate::of_utc_millis(now_millis()))
}

/// Checks diary entry count, future-date reach (at most `ENTRY_DAYS_AHEAD` past
/// `today`), and per-day uniqueness for an archive diary.
fn check_days_at(
    diary_id: &str,
    blocks: &[BlockPlace<'_>],
    today: Option<DiaryDate>,
) -> Result<(), AppError> {
    let limit = fvoci_model::ENTRIES_PER_DIARY;
    if blocks.len() > usize::try_from(limit).unwrap_or(usize::MAX) {
        return Err(StoreError::InvalidBlock {
            id: diary_id.to_owned(),
            source: BlockError::TooManyEntries { limit },
        }
        .into());
    }
    let mut days = HashSet::new();
    for block in blocks {
        let Some(date) = block.diary_date else {
            continue;
        };
        // No export holds such a day: none was ever made.
        if today.is_some_and(|today| !date.is_within_reach_of(today)) {
            return Err(AppError::bad_request(format_args!(
                "the archive's diary {diary_id} has an entry of {date}, a day that is yet to come"
            )));
        }
        if !days.insert(date) {
            // Of the archive, which is wrong in itself: no day of a diary that is
            // there is taken.
            return Err(AppError::bad_request(format_args!(
                "the archive's diary {diary_id} has two entries of {date}: a day has one entry"
            )));
        }
    }
    Ok(())
}

/// Checks that the pages of the wiki are a tree within `limits`: each identifier once,
/// each page reached from the top, no more and no deeper pages than a wiki may have. The
/// parents are those [`Place::check`] took: a page of the wiki, or none.
fn check_tree(
    wiki_id: &str,
    blocks: &[BlockPlace<'_>],
    limits: PageLimits,
) -> Result<(), AppError> {
    let pages: Vec<BlockPlace<'_>> = blocks
        .iter()
        .copied()
        .filter(|block| *block.block_type == BlockType::Page)
        .collect();
    // Of the whole wiki: as a copy of it is refused, naming no member of a request.
    let refused_whole = |source| {
        refused_without_member(StoreError::InvalidBlock {
            id: wiki_id.to_owned(),
            source,
        })
    };
    // Counted before anything is walked: an archive of many pages is refused at once.
    if usize::try_from(limits.pages).is_ok_and(|most| pages.len() > most) {
        return Err(refused_whole(BlockError::TooManyPages {
            limit: limits.pages,
        }));
    }
    let mut ids = HashSet::with_capacity(blocks.len());
    if let Some(twice) = blocks.iter().find(|block| !ids.insert(block.id)) {
        return Err(AppError::bad_request(format_args!(
            "the archive's wiki {wiki_id} has two blocks of the identifier {}",
            twice.id
        )));
    }
    let tree: PageTree = pages
        .iter()
        .map(|page| PageNode {
            id: page.id.to_owned(),
            parent_id: page.parent_id.to_owned(),
            ..PageNode::default()
        })
        .collect();
    // A page that the top does not lead to is below itself, or below one that is.
    let mut reached: HashSet<&str> = HashSet::with_capacity(tree.len());
    for top in tree.siblings("", "") {
        reached.extend(tree.below(&top.id));
    }
    if let Some(&looped) = pages.iter().find(|page| !reached.contains(page.id)) {
        return Err(refused(looped, BlockError::ParentBelowBlock));
    }
    tree.check_whole(limits).map_err(refused_whole)
}

/// Checks that the cards of a board in an archive have unique identifiers, no parent
/// cycles, and stay within [`MAX_CARD_DEPTH`] levels.
fn check_board_cards(board_id: &str, blocks: &[BlockPlace<'_>]) -> Result<(), AppError> {
    let mut ids = HashSet::with_capacity(blocks.len());
    if let Some(twice) = blocks.iter().find(|block| !ids.insert(block.id)) {
        return Err(AppError::bad_request(format_args!(
            "the archive's board {board_id} has two blocks of the identifier {}",
            twice.id
        )));
    }
    let cards: HashMap<&str, BlockPlace<'_>> = blocks
        .iter()
        .copied()
        .filter(|block| *block.block_type == BlockType::Card)
        .map(|block| (block.id, block))
        .collect();
    let max_depth = usize::try_from(MAX_CARD_DEPTH).unwrap_or(usize::MAX);
    for &block in blocks {
        if *block.block_type != BlockType::Card || block.parent_id.is_empty() {
            continue;
        }
        let mut depth = 1usize;
        let mut visited: HashSet<&str> = HashSet::from([block.id]);
        let mut cursor = block.parent_id;
        while !cursor.is_empty() {
            if !visited.insert(cursor) {
                return Err(refused(block, BlockError::ParentBelowBlock));
            }
            let Some(parent) = cards.get(cursor) else {
                break;
            };
            depth += 1;
            if depth > max_depth {
                return Err(refused(
                    block,
                    BlockError::CardTooDeep {
                        limit: MAX_CARD_DEPTH,
                    },
                ));
            }
            cursor = parent.parent_id;
        }
    }
    Ok(())
}

/// Checks that the chapters and units of a course form a valid tree of depth at most
/// [`COURSE_DEPTH`] within [`UNITS_PER_COURSE`] and [`CHAPTERS_PER_COURSE`], with unique
/// identifiers and no parent cycles.
fn check_course(course_id: &str, blocks: &[BlockPlace<'_>]) -> Result<(), AppError> {
    let chapters = blocks
        .iter()
        .filter(|block| *block.block_type == BlockType::Chapter)
        .count();
    if usize::try_from(CHAPTERS_PER_COURSE).is_ok_and(|most| chapters > most) {
        return Err(StoreError::InvalidBlock {
            id: course_id.to_owned(),
            source: BlockError::TooManyChapters {
                limit: CHAPTERS_PER_COURSE,
            },
        }
        .into());
    }
    let units = blocks
        .iter()
        .filter(|block| *block.block_type == BlockType::Unit)
        .count();
    if usize::try_from(UNITS_PER_COURSE).is_ok_and(|most| units > most) {
        return Err(StoreError::InvalidBlock {
            id: course_id.to_owned(),
            source: BlockError::TooManyUnits {
                limit: UNITS_PER_COURSE,
            },
        }
        .into());
    }
    let mut ids = HashSet::with_capacity(blocks.len());
    if let Some(twice) = blocks.iter().find(|block| !ids.insert(block.id)) {
        return Err(AppError::bad_request(format_args!(
            "the archive's course {course_id} has two blocks of the identifier {}",
            twice.id
        )));
    }
    let outline: Vec<BlockPlace<'_>> = blocks
        .iter()
        .copied()
        .filter(|block| matches!(block.block_type, BlockType::Unit | BlockType::Chapter))
        .collect();
    let tree: PageTree = outline
        .iter()
        .map(|block| PageNode {
            id: block.id.to_owned(),
            parent_id: block.parent_id.to_owned(),
            ..PageNode::default()
        })
        .collect();
    let mut reached: HashSet<&str> = HashSet::with_capacity(tree.len());
    for top in tree.siblings("", "") {
        reached.extend(tree.below(&top.id));
    }
    if let Some(&looped) = outline.iter().find(|block| !reached.contains(block.id)) {
        return Err(refused(looped, BlockError::ParentBelowBlock));
    }
    let max_depth = usize::try_from(COURSE_DEPTH).unwrap_or(usize::MAX);
    if let Some(&deep) = outline
        .iter()
        .find(|block| tree.depth(block.id) > max_depth)
    {
        return Err(refused(deep, BlockError::UnitParentNotChapter));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use fvoci_model::{Block, Container};

    use super::*;
    use crate::error::Reason;

    /// Blocks of one board: each its identifier, its parent and its type.
    type Blocks<'a> = &'a [(&'a str, &'a str, BlockType)];

    fn of(container_id: &str, kind: ContainerKind, blocks: Blocks<'_>) -> ContainersAndBlocks {
        let container = Container {
            id: container_id.to_owned(),
            kind,
            ..Container::default()
        };
        let blocks = blocks
            .iter()
            .map(|(id, parent, block_type)| Block {
                id: (*id).to_owned(),
                container_id: container_id.to_owned(),
                parent_id: (*parent).to_owned(),
                block_type: *block_type,
                ..Block::default()
            })
            .collect();
        ContainersAndBlocks {
            containers: vec![container],
            blocks,
        }
    }

    const LIMITS: PageLimits = PageLimits { depth: 2, pages: 3 };

    fn refusal(kind: ContainerKind, blocks: Blocks<'_>) -> String {
        check(&of("w", kind, blocks), LIMITS)
            .err()
            .map_or_default(|error| error.to_string())
    }

    #[test]
    fn a_tree_of_pages_and_their_comments_within_the_limits_is_taken() {
        let blocks = [
            ("a", "", BlockType::Page),
            ("b", "a", BlockType::Page),
            ("c", "", BlockType::Page),
            ("x", "b", BlockType::Comment),
        ];
        assert!(check(&of("w", ContainerKind::Wiki, &blocks), LIMITS).is_ok());
    }

    #[test]
    fn what_the_store_refuses_of_a_wiki_is_refused_with_its_message() {
        let page = || BlockType::Page;
        let comment = || BlockType::Comment;
        let cases: [(Blocks<'_>, BlockError); 8] = [
            (&[("a", "", BlockType::Card)], BlockError::NotPageOnWiki),
            (&[("a", "w", page())], BlockError::ParentNotOnContainer),
            (
                &[("a", "", page()), ("b", "x", page())],
                BlockError::ParentNotOnContainer,
            ),
            (&[("a", "", comment())], BlockError::CommentNotOnCardOrPage),
            (
                &[
                    ("a", "", page()),
                    ("x", "a", comment()),
                    ("y", "x", comment()),
                ],
                BlockError::CommentNotOnCardOrPage,
            ),
            (
                &[("a", "b", page()), ("b", "a", page())],
                BlockError::ParentBelowBlock,
            ),
            (&[("a", "a", page())], BlockError::ParentBelowBlock),
            (
                &[("a", "", page()), ("x", "a", comment()), ("b", "x", page())],
                BlockError::PageNotBelowPage,
            ),
        ];
        for (blocks, said) in cases {
            let refused = refusal(ContainerKind::Wiki, blocks);
            assert!(refused.contains(&said.to_string()), "{blocks:?}: {refused}");
        }
        let twice = refusal(ContainerKind::Wiki, &[("a", "", page()), ("a", "", page())]);
        assert!(twice.contains("two blocks"), "{twice}");
    }

    #[test]
    fn what_the_store_refuses_of_a_board_is_refused_and_what_nothing_leads_to_is_left() {
        let cases: [(Blocks<'_>, BlockError); 5] = [
            (&[("p", "", BlockType::Page)], BlockError::PageOffWiki),
            (
                &[("c", "w", BlockType::Card)],
                BlockError::ParentNotOnContainer,
            ),
            (
                &[("x", "", BlockType::Comment)],
                BlockError::CommentNotOnCardOrPage,
            ),
            (
                &[
                    ("c", "", BlockType::Card),
                    ("x", "c", BlockType::Comment),
                    ("v", "x", BlockType::View),
                ],
                BlockError::ParentIsComment,
            ),
            (
                &[("c", "other", BlockType::Card)],
                BlockError::ParentNotOnContainer,
            ),
        ];
        for (blocks, said) in cases {
            let mut archive = of("w", ContainerKind::Board, blocks);
            archive.containers.push(Container {
                id: "other".to_owned(),
                ..Container::default()
            });
            let refused = check(&archive, LIMITS).err().map(|error| error.to_string());
            let refused = refused.unwrap_or_default();
            assert!(refused.contains(&said.to_string()), "{blocks:?}: {refused}");
        }
        let card = BlockType::Card;
        let dropped_later = [("c", "gone", card), ("x", "c", BlockType::Comment)];
        assert!(check(&of("w", ContainerKind::Board, &dropped_later), LIMITS).is_ok());
        // A parent on another board of the same file.
        let mut two = of("w", ContainerKind::Board, &[("c", "", card)]);
        let other = of("o", ContainerKind::Board, &[("d", "c", card)]);
        two.containers.extend(other.containers);
        two.blocks.extend(other.blocks);
        let refused = check(&two, LIMITS).expect_err("a parent of another board");
        assert!(
            refused
                .to_string()
                .contains(&BlockError::ParentNotOnContainer.to_string())
        );
    }

    #[test]
    fn a_tree_past_the_limits_is_refused_with_them() {
        let page = || BlockType::Page;
        let deep = [("a", "", page()), ("b", "a", page()), ("c", "b", page())];
        let error = check(&of("w", ContainerKind::Wiki, &deep), LIMITS).expect_err("too deep");
        assert_eq!(error.reason(), Some(Reason::PageTooDeep));
        let wide = [
            ("a", "", page()),
            ("b", "", page()),
            ("c", "", page()),
            ("d", "", page()),
        ];
        let error = check(&of("w", ContainerKind::Wiki, &wide), LIMITS).expect_err("too many");
        assert_eq!(error.reason(), Some(Reason::TooManyPages));
    }

    #[test]
    fn a_chain_of_many_pages_is_refused_by_their_count_at_once() {
        let ids: Vec<String> = (0..200_000).map(|at| format!("p{at}")).collect();
        let blocks: Vec<(&str, &str, BlockType)> = ids
            .iter()
            .enumerate()
            .map(|(at, id)| {
                let parent = if at == 0 { "" } else { ids[at - 1].as_str() };
                (id.as_str(), parent, BlockType::Page)
            })
            .collect();
        let archive = of("w", ContainerKind::Wiki, &blocks);
        let limits = PageLimits {
            depth: 10,
            pages: 5_000,
        };
        let error = check(&archive, limits).expect_err("too many");
        assert_eq!(error.reason(), Some(Reason::TooManyPages));
    }

    #[test]
    fn a_course_outline_within_limits_is_taken_and_violations_are_refused() {
        let chapter = || BlockType::Chapter;
        let unit = || BlockType::Unit;
        let comment = || BlockType::Comment;
        let valid = [
            ("ch1", "", chapter()),
            ("u1", "ch1", unit()),
            ("u2", "", unit()),
            ("cm1", "u1", comment()),
            ("cm2", "u2", comment()),
        ];
        assert!(check(&of("c", ContainerKind::Course, &valid), LIMITS).is_ok());

        let cases: [(Blocks<'_>, BlockError); 9] = [
            (&[("a", "", BlockType::Card)], BlockError::NotUnitOnCourse),
            (&[("a", "", BlockType::Page)], BlockError::NotUnitOnCourse),
            (
                &[("ch1", "", chapter()), ("ch2", "ch1", chapter())],
                BlockError::ChapterBelowBlock,
            ),
            (
                &[("u1", "", unit()), ("ch1", "u1", chapter())],
                BlockError::ChapterBelowBlock,
            ),
            (
                &[("u1", "", unit()), ("u2", "u1", unit())],
                BlockError::UnitParentNotChapter,
            ),
            (
                &[("u1", "missing", unit())],
                BlockError::ParentNotOnContainer,
            ),
            (
                &[("cm1", "", comment())],
                BlockError::CommentNotOnCardOrPage,
            ),
            (
                &[("ch1", "", chapter()), ("cm1", "ch1", comment())],
                BlockError::CommentNotOnCardOrPage,
            ),
            (
                &[
                    ("u1", "", unit()),
                    ("cm1", "u1", comment()),
                    ("cm2", "cm1", comment()),
                ],
                BlockError::CommentNotOnCardOrPage,
            ),
        ];
        for (blocks, said) in cases {
            let refused = refusal(ContainerKind::Course, blocks);
            assert!(refused.contains(&said.to_string()), "{blocks:?}: {refused}");
        }
        for other in [ContainerKind::Board, ContainerKind::Wiki] {
            for block_type in [unit(), chapter()] {
                let refused = refusal(other, &[("u1", "", block_type)]);
                assert!(
                    refused.contains(&BlockError::UnitOffCourse.to_string()),
                    "{other:?}: {refused}"
                );
            }
        }
        let twice = refusal(
            ContainerKind::Course,
            &[("u1", "", unit()), ("u1", "", unit())],
        );
        assert!(twice.contains("two blocks"), "{twice}");

        // 200 chapters + 2,000 units is allowed; one more of either is refused.
        let chapter_ids: Vec<String> = (0..CHAPTERS_PER_COURSE).map(|i| format!("c{i}")).collect();
        let unit_ids: Vec<String> = (0..UNITS_PER_COURSE).map(|i| format!("u{i}")).collect();
        let mut at_limit: Vec<(&str, &str, BlockType)> = chapter_ids
            .iter()
            .map(|id| (id.as_str(), "", chapter()))
            .collect();
        at_limit.extend(
            unit_ids
                .iter()
                .map(|id| (id.as_str(), chapter_ids[0].as_str(), unit())),
        );
        assert!(check(&of("c", ContainerKind::Course, &at_limit), LIMITS).is_ok());

        let mut extra_chapter = at_limit.clone();
        extra_chapter.push(("c_extra", "", chapter()));
        let error = check(&of("c", ContainerKind::Course, &extra_chapter), LIMITS)
            .expect_err("too many chapters");
        assert_eq!(error.reason(), Some(Reason::TooManyChapters));

        let mut extra_unit = at_limit;
        extra_unit.push(("u_extra", "", unit()));
        let error = check(&of("c", ContainerKind::Course, &extra_unit), LIMITS)
            .expect_err("too many units");
        assert_eq!(error.reason(), Some(Reason::TooManyUnits));
    }

    #[test]
    fn diary_archive_days_enforce_reach_uniqueness_and_entry_limit() {
        let today = "2026-11-21".parse::<DiaryDate>().expect("today");
        let entry_type = BlockType::Entry;
        let make_entry = |id: &'static str, date: &'static str| BlockPlace {
            id,
            container_id: "d",
            parent_id: "",
            block_type: &entry_type,
            is_template: false,
            diary_date: Some(date.parse::<DiaryDate>().expect("date")),
        };
        let within = make_entry("e1", "2026-11-23");
        assert!(check_days_at("d", &[within], Some(today)).is_ok());

        let too_far = make_entry("e2", "2026-11-24");
        let err = check_days_at("d", &[within, too_far], Some(today))
            .expect_err("three days ahead is refused");
        assert!(err.to_string().contains("2026-11-24"), "{err}");

        let dup_a = make_entry("e3", "2026-11-20");
        let dup_b = make_entry("e4", "2026-11-20");
        let err = check_days_at("d", &[dup_a, dup_b], Some(today))
            .expect_err("duplicate date is refused");
        assert!(err.to_string().contains("two entries"), "{err}");

        let too_many_count = usize::try_from(fvoci_model::ENTRIES_PER_DIARY).expect("usize") + 1;
        let many: Vec<BlockPlace<'_>> = vec![within; too_many_count];
        let err = check_days_at("d", &many, Some(today)).expect_err("over entry limit");
        assert_eq!(err.reason(), Some(Reason::TooManyEntries));
    }
}
