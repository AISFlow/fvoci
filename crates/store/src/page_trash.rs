//! The trash of a wiki's pages: listing the pages deleted with what went with them, and
//! bringing one back with the pages below it.
//!
//! A page is deleted as any block is ([`crate::Store::delete_block`]): it goes to the
//! trash with every page below it, as one deletion. It comes back the same way, all of
//! the deletion together, held to the wiki's limits; where the page it was below is
//! gone (in the trash, or gone for good), it comes back at the top of the wiki, last.

use fvoci_model::{
    Block, BlockError, BlockType, DeletedPage, PageLimits, PageNode, PageTree, Place,
};

use crate::blocks::Paging;
use crate::db::{Id, Scope};
use crate::pages::{page_tree, write_place};
use crate::trash::{block_from_json, block_not_in_trash, take_blocks_out_of_trash};
use crate::{Store, StoreError, containers};

/// What bringing a page back changed: the pages that came back, a parent before what is
/// below it, the page asked for first; and the pages numbered anew to leave it room.
#[derive(Debug, Clone, PartialEq)]
pub struct PageRestore {
    /// The pages brought back, as they are stored, and the comments on them that went
    /// with them.
    pub pages: Vec<Block>,
    /// The pages numbered anew, as they are stored.
    pub renumbered: Vec<Block>,
}

impl Store {
    /// The pages of the wiki in the trash at the top of what was deleted with them, the
    /// last deleted first and then by identifier: the page of them `paging` names, or
    /// every one without it. A page that went with the page it was below comes back with
    /// that page, and is not listed.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn get_deleted_pages(
        &self,
        wiki_id: &str,
        paging: Option<Paging>,
    ) -> Result<Vec<DeletedPage>, StoreError> {
        self.with_conn(async |scope| {
            // The comments on the pages went with them and come back with them, and are
            // no pages below.
            let mut sql = scope.sql(
                "SELECT top.block, top.deleted_by, top.deleted_at, \
                 (SELECT COUNT(*) FROM deleted_blocks went \
                 WHERE went.deletion_id = top.deletion_id AND ",
            );
            push_block_types_in_trash(&mut sql, "went.block_type", &[BlockType::Page]);
            sql.push(") FROM deleted_blocks top WHERE top.container_id = ")
                .bind(Id(wiki_id))
                .push(" AND ");
            push_block_types_in_trash(&mut sql, "top.block_type", &[BlockType::Page]);
            sql.push(
                " AND NOT EXISTS (SELECT 1 FROM deleted_blocks above \
                 WHERE above.deletion_id = top.deletion_id AND above.id = top.parent_id) \
                 ORDER BY top.deleted_at DESC, top.id",
            );
            sql.push_page(paging);
            let mut pages = Vec::new();
            for row in &scope.fetch_all(sql).await? {
                let block = match block_from_json(row, 0) {
                    Ok(block) => block,
                    Err(StoreError::Corrupt { column, reason }) => {
                        tracing::error!(
                            wiki_id,
                            column,
                            %reason,
                            "skipping corrupt deleted page in trash listing"
                        );
                        continue;
                    }
                    Err(error) => return Err(error),
                };
                let went = u32::try_from(row.int(3)?).unwrap_or(u32::MAX);
                let node = PageNode::from(block);
                pages.push(DeletedPage {
                    id: node.id,
                    title: node.title,
                    icon: node.icon,
                    parent_id: node.parent_id,
                    deleted_at: row.int(2)?,
                    deleted_by: row.id(1)?,
                    below: went.saturating_sub(1),
                });
            }
            Ok(pages)
        })
        .await
    }

    /// Brings a page of the wiki back from the trash with every page that was deleted
    /// along with it, held to `limits`: below the page it was below, or, that page gone,
    /// last at the top of the wiki. Marks them as modified by `user_id` now.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if the trash of the wiki has no such page at the
    /// top of what was deleted with it, and
    /// [`StoreError::InvalidBlock`] if the pages would be deeper, or the wiki larger,
    /// than `limits` let them.
    pub async fn undelete_page(
        &self,
        wiki_id: &str,
        page_id: &str,
        limits: PageLimits,
        user_id: &str,
    ) -> Result<PageRestore, StoreError> {
        self.transaction(async |scope| {
            // The wiki first, then the trash read under its lock: a page restored
            // meanwhile is not in it any more.
            containers::lock_container_of_kind(scope, wiki_id)
                .await?
                .ok_or_else(block_not_in_trash)?;
            let deletion = deletion_of(scope, wiki_id, page_id, &[BlockType::Page]).await?;
            undelete(scope, wiki_id, page_id, deletion, limits, user_id).await
        })
        .await
    }
}

/// Appends a parameterized SQL predicate checking that `column` (`block_type` of
/// `deleted_blocks`) matches one of `types`.
pub(crate) fn push_block_types_in_trash(
    sql: &mut crate::db::Sql,
    column: &'static str,
    types: &[BlockType],
) {
    if types.is_empty() {
        sql.push("1 = 0");
        return;
    }
    sql.push([column, " IN ("])
        .bind_list(types.iter().copied().map(BlockType::as_str))
        .push(")");
}

/// The deletion of the container the block went with, if the block is in the container's
/// trash at the top of it and has one of `types`.
pub(crate) async fn deletion_of(
    scope: &mut Scope<'_>,
    container_id: &str,
    block_id: &str,
    types: &[BlockType],
) -> Result<Deletion, StoreError> {
    let mut sql = scope.sql("SELECT deletion_id, block FROM deleted_blocks WHERE id = ");
    sql.bind(Id(block_id))
        .push_bind(" AND container_id = ", Id(container_id));
    let row = scope
        .fetch_optional(sql)
        .await?
        .ok_or_else(block_not_in_trash)?;
    let block = block_from_json(&row, 1)?;
    if !types.contains(&block.block_type) {
        return Err(block_not_in_trash());
    }
    let deletion_id = row.id(0)?;
    let mut sql = scope.sql("SELECT block FROM deleted_blocks WHERE deletion_id = ");
    sql.bind(Id(&deletion_id));
    let family = scope
        .fetch_all(sql)
        .await?
        .iter()
        .map(|row| block_from_json(row, 0))
        .collect::<Result<Vec<_>, _>>()?;
    // One that went with the block it was below comes back with that block, not by itself.
    if family.iter().any(|one| one.id == block.parent_id) {
        return Err(block_not_in_trash());
    }
    Ok(Deletion {
        id: deletion_id,
        blocks: family,
    })
}

/// A deletion in the trash: its identifier, and the blocks that went with it.
pub(crate) struct Deletion {
    pub(crate) id: String,
    pub(crate) blocks: Vec<Block>,
}

/// Brings the deletion back, the wiki held: its pages at the top of it below their
/// parent if it is there, last at the top otherwise; at the place it had, unless a page
/// made since has that place.
async fn undelete(
    scope: &mut Scope<'_>,
    wiki_id: &str,
    page_id: &str,
    deletion: Deletion,
    limits: PageLimits,
    user_id: &str,
) -> Result<PageRestore, StoreError> {
    let invalid = |source| StoreError::InvalidBlock {
        id: page_id.to_owned(),
        source,
    };
    let tree = page_tree(scope, wiki_id).await?;
    let coming: PageTree = deletion
        .blocks
        .iter()
        .filter(|block| block.block_type == BlockType::Page)
        .cloned()
        .map(PageNode::from)
        .collect();
    let mut tops: Vec<Block> = deletion
        .blocks
        .into_iter()
        .filter(|block| !coming.contains(&block.parent_id))
        .collect();
    let mut renumbered = Vec::new();
    for top in &mut tops {
        let parent_gone = !top.parent_id.is_empty() && !tree.contains(&top.parent_id);
        if parent_gone {
            top.parent_id.clear();
        }
        let adding = coming.below(&top.id).len();
        tree.check_room(&top.parent_id, coming.height(&top.id), adding, limits)
            .map_err(invalid)?;
        let place = if parent_gone {
            // Last at the top of the wiki.
            Some(tree.place("", None, "").map_err(invalid)?)
        } else {
            place_taken(&tree, top).map_err(invalid)?
        };
        if let Some(place) = place {
            renumbered.extend(write_place(scope, top, place).await?);
        }
    }
    let now = scope.now_millis();
    let mut pages = take_blocks_out_of_trash(scope, &deletion.id, user_id, now, |block| {
        if let Some(top) = tops.iter().find(|top| top.id == block.id) {
            block.parent_id.clone_from(&top.parent_id);
            block.fields.clone_from(&top.fields);
        }
    })
    .await?;
    // Emptied of it meanwhile (`purge_trash` takes no lock): nothing is written.
    if !pages.iter().any(|block| block.id == page_id) {
        return Err(block_not_in_trash());
    }
    // The page asked for first, then parents before what is below them: a comment is
    // one below its page.
    pages.sort_by_key(|block| {
        let depth = if block.block_type == BlockType::Comment {
            coming.depth(&block.parent_id) + 1
        } else {
            coming.depth(&block.id)
        };
        (block.id != page_id, depth)
    });
    Ok(PageRestore { pages, renumbered })
}

/// Where a block coming back below its parent goes when a sibling has taken its order
/// meanwhile: right after the siblings before it. None where its order is its own.
pub(crate) fn place_taken(tree: &PageTree, top: &Block) -> Result<Option<Place>, BlockError> {
    let node = PageNode::from(top.clone());
    let siblings = tree.siblings(&node.parent_id, "");
    if !siblings.iter().any(|sibling| sibling.order == node.order) {
        return Ok(None);
    }
    let key = (node.order.as_str(), node.id.as_str());
    let after = siblings
        .iter()
        .take_while(|sibling| (sibling.order.as_str(), sibling.id.as_str()) < key)
        .last()
        .map_or("", |sibling| sibling.id.as_str());
    tree.place(&node.parent_id, Some(after), "").map(Some)
}
