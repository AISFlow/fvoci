//! Duplicating a page of a wiki with every page below it, within the wiki, and a wiki
//! whole.
//!
//! The copies are new pages: new identifiers, made by whoever duplicates them, now. Each
//! keeps its title (the copy of the page asked for takes the title it is given), its
//! icon, its place among the pages copied with it and its body; the versions of the
//! bodies and the comments are not copied. The images of the bodies still show the
//! source page's files: the caller gives the copies files of their own afterwards, as a
//! card's duplicate does.

use std::collections::HashMap;

use fvoci_model::{
    Block, ContainerKind, ContainerMember, ContainersAndBlocks, PageLimits, generate_block_ids,
};

use crate::blocks::{CopyChanges, CopyTitles, fetch_blocks, insert_blocks, select_blocks};
use crate::containers_and_blocks::duplicate_container;
use crate::db::{Id, Scope};
use crate::file_refs::NewBodyFiles;
use crate::notification_noting::Telling;
use crate::pages::{page_tree, write_place};
use crate::{Store, StoreError, containers};
use fvoci_model::DuplicateAs;

/// What duplicating a page made: the copies, the copy of the page asked for first and
/// a parent before what is below it; and the pages numbered anew to leave it room.
#[derive(Debug, Clone, PartialEq)]
pub struct PageCopies {
    /// The copies, as they are stored.
    pub pages: Vec<Block>,
    /// The siblings numbered anew, as they are stored.
    pub renumbered: Vec<Block>,
}

impl Store {
    /// Copies a page of the wiki with every page below it, the copy right after the page
    /// among its siblings, titled `title` if given (the page's own title otherwise),
    /// held to `limits`. The wiki is held from before the tree is read.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if the wiki has no such page, and
    /// [`StoreError::InvalidBlock`] if the copies would be deeper, or the wiki larger,
    /// than `limits` let them, or the title may not be stored.
    pub async fn duplicate_page(
        &self,
        wiki_id: &str,
        page_id: &str,
        title: Option<&str>,
        limits: PageLimits,
        user_id: &str,
    ) -> Result<PageCopies, StoreError> {
        let not_found = || StoreError::not_found(format_args!("page ID={page_id} of {wiki_id}"));
        let invalid = |source| StoreError::InvalidBlock {
            id: page_id.to_owned(),
            source,
        };
        self.transaction(async |scope| {
            containers::lock_container_of_kind(scope, wiki_id)
                .await?
                .ok_or_else(not_found)?;
            let tree = page_tree(scope, wiki_id).await?;
            let parent_id = tree.parent_of(page_id).ok_or_else(not_found)?.to_owned();
            let family: Vec<String> = tree.below(page_id).into_iter().map(str::to_owned).collect();
            tree.check_room(&parent_id, tree.height(page_id), family.len(), limits)
                .map_err(invalid)?;
            let mut copies = family_of(scope, wiki_id, page_id, &family).await?;
            if let Some(title) = title {
                title.clone_into(&mut copies[0].title);
            }
            copies[0].validate().map_err(invalid)?;
            let place = tree.place(&parent_id, Some(page_id), "").map_err(invalid)?;
            let renumbered = write_place(scope, &mut copies[0], place).await?;
            // New identifiers, and each copy below the copy of its parent.
            generate_block_ids(&mut copies);
            // A copy names whom its original named: nobody is told again.
            let nobody = Telling::Nobody;
            insert_blocks(scope, &mut copies, user_id, NewBodyFiles::Unchecked, nobody).await?;
            Ok(PageCopies {
                pages: copies,
                renumbered,
            })
        })
        .await
    }
}

impl Store {
    /// Copies a wiki with its pages as they stand, as a board is copied
    /// ([`Store::duplicate_container`]): new identifiers, the references of their bodies to
    /// each other pointed at the copies, a private wiki of `user_id`, who administers
    /// it, in `to_workspace` or else in the workspace of the original. The comments on
    /// the pages and the versions of their bodies are not copied, nor the pages in the
    /// trash. The copy is held to `limits`, which may have been lowered since the
    /// original grew; the wiki is held from before its tree is read.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if there is no such wiki, or if `user_id` does
    /// not work in the copy's workspace, [`StoreError::InvalidBlock`] if the copy would
    /// be deeper or larger than `limits` let a wiki be, and the store's error if the copy
    /// cannot be written.
    pub async fn duplicate_wiki(
        &self,
        wiki_id: &str,
        user_id: &str,
        to_workspace: Option<&str>,
        limits: PageLimits,
        titles: &CopyTitles,
    ) -> Result<(ContainersAndBlocks, Vec<ContainerMember>), StoreError> {
        self.transaction(async |scope| {
            if containers::lock_container_of_kind(scope, wiki_id).await?
                != Some(ContainerKind::Wiki)
            {
                return Err(StoreError::not_found(format_args!("wiki ID={wiki_id}")));
            }
            page_tree(scope, wiki_id)
                .await?
                .check_whole(limits)
                .map_err(|source| StoreError::InvalidBlock {
                    id: wiki_id.to_owned(),
                    source,
                })?;
            duplicate_container(
                scope,
                wiki_id,
                user_id,
                to_workspace,
                DuplicateAs::Copy,
                titles,
                CopyChanges::default(),
            )
            .await
        })
        .await
    }
}

/// How many pages are asked for at a time: a wiki's limit may be configured past what
/// one statement may bind.
const PAGES_AT_ONCE: usize = 500;

/// The page and the pages below it as they are stored, the page first and then in the
/// order of `family` (a parent before what is below it).
async fn family_of(
    scope: &mut Scope<'_>,
    wiki_id: &str,
    page_id: &str,
    family: &[String],
) -> Result<Vec<Block>, StoreError> {
    let mut found = Vec::with_capacity(family.len());
    for ids in family.chunks(PAGES_AT_ONCE) {
        let mut sql = select_blocks(scope);
        sql.push_bind(" WHERE container_id = ", Id(wiki_id))
            .push(" AND type = 'page' AND id IN (")
            .bind_list(ids.iter().map(|id| Id(id)))
            .push(")");
        found.extend(fetch_blocks(scope, sql).await?);
    }
    let rank: HashMap<&str, usize> = family
        .iter()
        .enumerate()
        .map(|(at, id)| (id.as_str(), at))
        .collect();
    found.sort_by_cached_key(|block| rank.get(block.id.as_str()).copied());
    if found.first().is_none_or(|block| block.id != page_id) {
        return Err(StoreError::not_found(format_args!("page ID={page_id}")));
    }
    // Who made them and when is the copy's: the insert says so.
    Ok(found)
}
